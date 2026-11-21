//! Control Device from AI apps over the Model Context Protocol (spec 023).
//!
//! Two halves in one executable:
//!
//! * **Relay** (`device-studio mcp`): what an AI app such as Claude Desktop starts. It speaks MCP
//!   (JSON-RPC 2.0, one message per line) on stdin/stdout, answers `initialize` and `tools/list`
//!   itself, and forwards each `tools/call` to the running Device. It opens no window.
//! * **Server** (inside the app, Settings › General › Let AI apps control Device): a loopback TCP
//!   listener. A connection first sends `{"secret", "client"}`; the secret is the one in the
//!   discovery file ([discovery_path], 0600 on macOS and Linux), so only programs that can read
//!   this user's files get in. Each call is handed to the UI thread as [JobResult::Mcp] and
//!   answered from there ([Controller::mcp_handle]); the audio thread is never involved.
//!
//! Every edit goes through `assistant::plan` and `commit`: validated first, one undo step
//! ("AI App Edit"), nothing applied when any action is invalid.

use crate::controller::{Controller, JobResult};
use dstudio_core::assistant::{self, Mode};
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Sender};
use std::time::Duration;

/// The undo label for every edit an AI app makes.
pub const UNDO_LABEL: &str = "AI App Edit";
/// MCP protocol revision the relay answers `initialize` with.
const PROTOCOL_VERSION: &str = "2025-06-18";
/// How long a call waits for the app. Long enough for the user to answer "Allow?".
const CALL_TIMEOUT: Duration = Duration::from_secs(120);
const NOT_RUNNING: &str =
    "Device isn't reachable. Open Device and turn on Settings › General › Let AI apps control Device, then try again.";

/// The discovery file: where the running app writes its port and secret.
/// `DSTUDIO_MCP_FILE` overrides it (tests).
pub fn discovery_path() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("DSTUDIO_MCP_FILE") {
        return Some(PathBuf::from(p));
    }
    Some(crate::prefs::support_dir()?.join("mcp.json"))
}

/// `get_builtin_devices`: the whole catalogue with every parameter is far larger than an AI app
/// will take as one tool result, so by default it is a short list, and parameters come on request.
pub fn builtin_devices(args: &Value) -> Value {
    let all = assistant::builtin_catalog();
    let all = all.as_array().cloned().unwrap_or_default();
    if let Some(ids) = args.get("ids").and_then(Value::as_array) {
        let ids: Vec<&str> = ids.iter().filter_map(Value::as_str).collect();
        return all.into_iter().filter(|d| d["id"].as_str().is_some_and(|id| ids.contains(&id))).collect();
    }
    let q = args.get("query").and_then(Value::as_str).unwrap_or("").to_lowercase();
    all.iter()
        .filter(|d| {
            q.is_empty() || ["id", "name", "kind", "category"].iter().any(|k| d[*k].as_str().is_some_and(|v| v.to_lowercase().contains(&q)))
        })
        .map(|d| json!({"id": d["id"], "name": d["name"], "kind": d["kind"], "category": d["category"]}))
        .collect()
}

/// The tools an AI app sees (`tools/list`).
pub fn tools() -> Value {
    let actions = assistant::tool_schema()["properties"]["actions"].clone();
    let empty = json!({"type": "object", "properties": {}, "additionalProperties": false});
    json!([
        {
            "name": "get_project",
            "description": "The open Device project: tempo, time signature, tracks with their clips, MIDI notes, \
                devices (effects and instrument, with changed parameters), sends and the project's audio files. \
                Positions are ticks (960 per quarter note). `next_id_hint` is the id the next new object gets. \
                Read this before apply_actions so you use real ids.",
            "inputSchema": empty,
            "annotations": {"readOnlyHint": true},
        },
        {
            "name": "get_builtin_devices",
            "description": "The built-in effects and instruments you can add with an `add_builtin_device` action. \
                Without `ids` it lists each device's id, name, kind and category (narrow it with `query`, e.g. \
                \"reverb\" or \"Dynamics\"). Pass `ids` to get those devices' parameters as \
                [id, min, max, default, unit] before setting them.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": {"type": "string", "description": "Only devices whose id, name, kind or category contains this (any case)."},
                    "ids": {"type": "array", "items": {"type": "string"}, "description": "Device ids to return in full, with parameters."},
                },
                "additionalProperties": false,
            },
            "annotations": {"readOnlyHint": true},
        },
        {
            "name": "apply_actions",
            "description": "Change the project. All actions are checked together first and applied as ONE undo step \
                (\"AI App Edit\"); if any action is invalid nothing changes and the error says which one. \
                To refer to a track or clip you create in the same call, give it the id `next_id_hint` from \
                get_project (then +1 for each further new object) instead of 0. `highlight` points at a control \
                in Device's window; `explain` is ignored here (say it to the user yourself).",
            "inputSchema": {"type": "object", "required": ["actions"], "properties": {"actions": actions}},
        },
        {"name": "undo", "description": "Undo the last change in Device (any change, not only yours).", "inputSchema": empty},
        {"name": "redo", "description": "Redo the last undone change.", "inputSchema": empty},
        {
            "name": "transport",
            "description": "Start or stop playback.",
            "inputSchema": {"type": "object", "required": ["action"], "properties": {"action": {"enum": ["play", "stop"]}}},
        },
    ])
}

/// The Claude Desktop (and Cursor, Claude Code...) config entry that starts the relay.
pub fn setup_snippet() -> String {
    let exe = std::env::current_exe().map(|p| p.display().to_string()).unwrap_or_else(|_| "device-studio".into());
    serde_json::to_string_pretty(&json!({"mcpServers": {"device": {"command": exe, "args": ["mcp"]}}})).unwrap_or_default()
}

// ---------------------------------------------------------------- relay (device-studio mcp)

/// One MCP message in, at most one out. `call` forwards a `tools/call` to the app.
pub fn relay_message(msg: &Value, client: &mut String, call: &mut dyn FnMut(&str, &str, &Value) -> Result<Value, String>) -> Option<Value> {
    let id = msg.get("id").cloned()?; // notifications get no answer
    let method = msg.get("method").and_then(Value::as_str).unwrap_or("");
    let result = match method {
        "initialize" => {
            if let Some(name) = msg.pointer("/params/clientInfo/name").and_then(Value::as_str) {
                *client = name.chars().filter(|c| !c.is_control()).take(60).collect();
            }
            json!({
                "protocolVersion": PROTOCOL_VERSION,
                "capabilities": {"tools": {}},
                "serverInfo": {"name": "device", "title": "Device", "version": env!("CARGO_PKG_VERSION")},
                "instructions": "Device is a digital audio workstation. Call get_project first, then apply_actions to edit.",
            })
        }
        "ping" => json!({}),
        "tools/list" => json!({"tools": tools()}),
        "tools/call" => {
            let name = msg.pointer("/params/name").and_then(Value::as_str).unwrap_or("");
            let args = msg.pointer("/params/arguments").cloned().unwrap_or(json!({}));
            match call(client, name, &args) {
                Ok(v) => json!({"content": [{"type": "text", "text": v.to_string()}], "isError": false}),
                Err(e) => json!({"content": [{"type": "text", "text": e}], "isError": true}),
            }
        }
        _ => return Some(json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32601, "message": format!("unknown method {method}")}})),
    };
    Some(json!({"jsonrpc": "2.0", "id": id, "result": result}))
}

/// A connection to the running app, opened on the first call and again after a failure.
struct AppLink(Option<(TcpStream, BufReader<TcpStream>)>);

impl AppLink {
    fn call(&mut self, client: &str, name: &str, args: &Value) -> Result<Value, String> {
        if self.0.is_none() {
            self.0 = Some(connect(client).map_err(|_| NOT_RUNNING.to_string())?);
        }
        let result = (|| {
            let (w, r) = self.0.as_mut().ok_or(NOT_RUNNING)?;
            writeln!(w, "{}", json!({"name": name, "arguments": args})).map_err(|_| NOT_RUNNING)?;
            let mut line = String::new();
            if r.read_line(&mut line).map_err(|_| NOT_RUNNING)? == 0 {
                return Err(NOT_RUNNING.to_string());
            }
            let v: Value = serde_json::from_str(&line).map_err(|e| e.to_string())?;
            match v.get("error").and_then(Value::as_str) {
                Some(e) => Err(e.to_string()),
                None => Ok(v.get("ok").cloned().unwrap_or(Value::Null)),
            }
        })();
        if result.as_ref().is_err_and(|e| e == NOT_RUNNING) {
            self.0 = None;
        }
        result
    }
}

fn connect(client: &str) -> Result<(TcpStream, BufReader<TcpStream>), String> {
    let path = discovery_path().ok_or("no discovery file")?;
    let d: Value = serde_json::from_str(&std::fs::read_to_string(path).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    let port = d.get("port").and_then(Value::as_u64).ok_or("no port")? as u16;
    let secret = d.get("secret").and_then(Value::as_str).ok_or("no secret")?;
    let mut s = TcpStream::connect(("127.0.0.1", port)).map_err(|e| e.to_string())?;
    s.set_read_timeout(Some(CALL_TIMEOUT + Duration::from_secs(5))).ok();
    writeln!(s, "{}", json!({"secret": secret, "client": client})).map_err(|e| e.to_string())?;
    let r = BufReader::new(s.try_clone().map_err(|e| e.to_string())?);
    Ok((s, r))
}

/// `device-studio mcp`: relay MCP on stdin/stdout until stdin closes. Returns the exit code.
pub fn run_relay() -> i32 {
    let stdin = std::io::stdin();
    let mut out = std::io::stdout().lock();
    let mut client = String::from("AI app");
    let mut link = AppLink(None);
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let reply = match serde_json::from_str::<Value>(&line) {
            Ok(msg) => relay_message(&msg, &mut client, &mut |c, n, a| link.call(c, n, a)),
            Err(e) => Some(json!({"jsonrpc": "2.0", "id": null, "error": {"code": -32700, "message": e.to_string()}})),
        };
        if let Some(reply) = reply
            && (writeln!(out, "{reply}").is_err() || out.flush().is_err())
        {
            break;
        }
    }
    0
}

// ---------------------------------------------------------------- server (inside the app)

/// A call from an AI app, waiting on the UI thread.
pub struct McpCall {
    pub client: String,
    pub name: String,
    pub args: Value,
    pub reply: Sender<Result<Value, String>>,
}

impl std::fmt::Debug for McpCall {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "McpCall({}, {})", self.client, self.name)
    }
}

/// The running listener; dropping it stops listening and removes the discovery file.
pub struct McpServer {
    stop: Arc<AtomicBool>,
    file: Option<PathBuf>,
    #[cfg_attr(not(test), allow(dead_code))]
    pub port: u16,
}

impl Drop for McpServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(f) = &self.file {
            let _ = std::fs::remove_file(f);
        }
    }
}

/// A 128-bit secret from the standard library's randomly keyed hasher (no extra dependency).
fn new_secret() -> String {
    use std::hash::{BuildHasher, Hasher};
    (0..2)
        .map(|i| {
            let mut h = std::collections::hash_map::RandomState::new().build_hasher();
            h.write_u64(i ^ std::process::id() as u64);
            format!("{:016x}", h.finish())
        })
        .collect()
}

/// Compare without stopping at the first difference.
fn same_secret(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.bytes().zip(b.bytes()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Start listening on loopback and write the discovery file (`file`, None = don't write one).
pub fn start(jobs: Sender<JobResult>, file: Option<PathBuf>) -> Result<McpServer, String> {
    let listener = TcpListener::bind(("127.0.0.1", 0)).map_err(|e| e.to_string())?;
    listener.set_nonblocking(true).map_err(|e| e.to_string())?;
    let port = listener.local_addr().map_err(|e| e.to_string())?.port();
    let secret = new_secret();
    if let Some(f) = &file {
        write_private(f, &json!({"port": port, "secret": secret, "pid": std::process::id()}).to_string())?;
    }
    let stop = Arc::new(AtomicBool::new(false));
    let stop2 = stop.clone();
    std::thread::Builder::new()
        .name("mcp-listen".into())
        .spawn(move || {
            while !stop2.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((s, _)) => {
                        let (jobs, secret, stop) = (jobs.clone(), secret.clone(), stop2.clone());
                        let _ = std::thread::Builder::new().name("mcp-conn".into()).spawn(move || serve(s, &jobs, &secret, &stop));
                    }
                    Err(_) => std::thread::sleep(Duration::from_millis(50)),
                }
            }
        })
        .map_err(|e| e.to_string())?;
    Ok(McpServer { stop, file, port })
}

fn write_private(path: &std::path::Path, text: &str) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let mut o = std::fs::OpenOptions::new();
    o.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.mode(0o600);
    }
    let mut f = o.open(path).map_err(|e| e.to_string())?;
    f.write_all(text.as_bytes()).map_err(|e| e.to_string())
}

/// One relay connection: the handshake, then calls until it closes or the server stops.
fn serve(s: TcpStream, jobs: &Sender<JobResult>, secret: &str, stop: &AtomicBool) {
    let _ = s.set_nonblocking(false);
    let _ = s.set_read_timeout(Some(Duration::from_millis(500)));
    let Ok(mut w) = s.try_clone() else { return };
    let mut r = BufReader::new(s);
    let mut line = String::new();
    let mut read = |line: &mut String| -> Option<()> {
        line.clear();
        loop {
            if stop.load(Ordering::Relaxed) {
                return None;
            }
            match r.read_line(line) {
                Ok(0) => return None,
                Ok(_) => return Some(()),
                Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => continue,
                Err(_) => return None,
            }
        }
    };
    if read(&mut line).is_none() {
        return;
    }
    let hello: Value = serde_json::from_str(&line).unwrap_or(Value::Null);
    if !hello.get("secret").and_then(Value::as_str).is_some_and(|got| same_secret(got, secret)) {
        let _ = writeln!(w, "{}", json!({"error": "Device refused the connection."}));
        let _ = w.shutdown(Shutdown::Both);
        return;
    }
    let client = hello.get("client").and_then(Value::as_str).unwrap_or("AI app").to_string();
    while read(&mut line).is_some() {
        let msg: Value = serde_json::from_str(&line).unwrap_or(Value::Null);
        let (tx, rx) = mpsc::channel();
        let call = McpCall {
            client: client.clone(),
            name: msg.get("name").and_then(Value::as_str).unwrap_or("").to_string(),
            args: msg.get("arguments").cloned().unwrap_or(json!({})),
            reply: tx,
        };
        let answer = if jobs.send(JobResult::Mcp(call)).is_err() {
            Err(NOT_RUNNING.to_string())
        } else {
            rx.recv_timeout(CALL_TIMEOUT).unwrap_or_else(|_| Err("Device didn't answer in time. Is a dialog open in Device?".into()))
        };
        let out = match answer {
            Ok(v) => json!({"ok": v}),
            Err(e) => json!({"error": e}),
        };
        if writeln!(w, "{out}").is_err() {
            return;
        }
    }
}

// ---------------------------------------------------------------- the UI thread's side

/// What the app knows about AI apps this run.
#[derive(Default)]
pub struct McpState {
    pub server: Option<McpServer>,
    /// The app waiting on "Allow …?" and its calls, answered when the user decides.
    pub asking: Option<(String, Vec<McpCall>)>,
    /// Apps the user refused this run.
    pub denied: Vec<String>,
    /// The last app that made a call (Settings shows it).
    pub last_client: Option<String>,
    /// Settings › General › Copy setup was pressed (the "Copied" note).
    pub copied: bool,
}

impl Controller {
    /// Start or stop the server to match the preference.
    pub fn mcp_sync(&mut self) {
        let want = self.settings.prefs.ai_apps;
        if want && self.mcp.server.is_none() {
            match start(self.jobs_tx.clone(), discovery_path()) {
                Ok(s) => self.mcp.server = Some(s),
                Err(e) => self.set_status(format!("Couldn't let AI apps connect: {e}"), 6),
            }
        } else if !want && self.mcp.server.is_some() {
            self.mcp.server = None;
            if let Some((_, calls)) = self.mcp.asking.take() {
                for c in calls {
                    let _ = c.reply.send(Err(NOT_RUNNING.into()));
                }
            }
        }
    }

    /// Settings › General › Copy setup was pressed (the config is copied by the UI).
    pub fn mcp_copied(&mut self) {
        self.mcp.copied = true;
        self.set_status("Copied. Paste it into your AI app's MCP settings (Claude Desktop: Settings › Developer › Edit Config)", 6);
        self.ui_dirty = true;
    }

    /// The line under the switch.
    pub fn mcp_status_text(&self) -> String {
        match (&self.mcp.server, &self.mcp.last_client) {
            (None, _) if self.settings.prefs.ai_apps => "Couldn't start; turn it off and on again.".into(),
            (None, _) => "AI apps such as Claude Desktop can read and edit your project once this is on.".into(),
            (Some(_), Some(app)) => format!("On. Last used by {app}."),
            (Some(_), None) => "On. Waiting for an AI app; Copy setup gives it what it needs.".into(),
        }
    }

    /// Settings › General › Let AI apps control Device.
    pub fn mcp_toggle(&mut self) {
        self.settings.prefs.ai_apps = !self.settings.prefs.ai_apps;
        self.save_prefs();
        self.mcp_sync();
        self.ui_dirty = true;
    }

    /// A call from an AI app (poll_jobs).
    pub fn mcp_call(&mut self, call: McpCall) {
        self.mcp.last_client = Some(call.client.clone());
        self.ui_dirty = true;
        if self.mcp.server.is_none() {
            let _ = call.reply.send(Err(NOT_RUNNING.into()));
        } else if self.mcp.denied.contains(&call.client) {
            let _ = call.reply.send(Err(format!("You chose not to let {} change projects in Device this session.", call.client)));
        } else if self.settings.prefs.ai_apps_allowed.contains(&call.client) {
            let r = self.mcp_handle(&call.name, &call.args);
            let _ = call.reply.send(r);
        } else {
            match &mut self.mcp.asking {
                Some((client, calls)) if *client == call.client => calls.push(call),
                Some(_) => {
                    let _ = call.reply.send(Err("Device is asking about another app; try again in a moment.".into()));
                }
                None => self.mcp.asking = Some((call.client.clone(), vec![call])),
            }
        }
    }

    /// "Allow <app> to change your projects?" answered.
    pub fn mcp_answer(&mut self, allow: bool) {
        let Some((client, calls)) = self.mcp.asking.take() else { return };
        if allow {
            self.settings.prefs.ai_apps_allowed.push(client);
            self.save_prefs();
            for c in calls {
                let r = self.mcp_handle(&c.name, &c.args);
                let _ = c.reply.send(r);
            }
        } else {
            for c in &calls {
                let _ = c.reply.send(Err(format!("You chose not to let {client} change projects in Device.")));
            }
            self.mcp.denied.push(client);
        }
        self.ui_dirty = true;
    }

    /// Run one tool on the open project.
    pub fn mcp_handle(&mut self, name: &str, args: &Value) -> Result<Value, String> {
        match name {
            "get_project" => {
                let mut state = assistant::describe_state(self.project());
                state["next_id_hint"] = json!(self.project().next_id);
                Ok(state)
            }
            "get_builtin_devices" => Ok(builtin_devices(args)),
            "apply_actions" => {
                if self.midi_in.pass.is_some() || self.rec.recording {
                    return Err("Device is recording; try again when it stops.".into());
                }
                let actions = args.get("actions").cloned().unwrap_or(json!([]));
                let turn = assistant::parse_turn(&json!({"actions": actions}).to_string()).map_err(|e| match e {
                    assistant::AssistantError::Malformed(m) => {
                        format!("Nothing was changed: those actions don't match the apply_actions schema ({m})")
                    }
                    e => format!("Nothing was changed: {e}"),
                })?;
                let mut plan = assistant::plan(&self.doc, Mode::Assist, &turn).map_err(|e| format!("Nothing was changed: {e}"))?;
                if let Some(dstudio_core::Command::Batch { label, .. }) = &mut plan.batch {
                    *label = UNDO_LABEL.into();
                }
                if plan.batch.is_some() {
                    assistant::commit(&mut self.doc, &plan).map_err(|e| format!("Nothing was changed: {e}"))?;
                    self.after_change();
                }
                let until = std::time::Instant::now() + Duration::from_secs(6);
                self.assistant.highlights = plan
                    .highlights
                    .iter()
                    .map(|(t, n)| crate::assistant_panel::Highlight { target: t.clone(), note: n.clone(), until })
                    .collect();
                if !plan.summary.is_empty() {
                    self.set_status(plan.summary.join(" "), 4);
                }
                Ok(json!({"applied": plan.summary, "highlighted": plan.highlights.len(), "next_id_hint": self.project().next_id}))
            }
            "undo" | "redo" => {
                let undo = name == "undo";
                let step = if undo { self.doc.undo_label() } else { self.doc.redo_label() }.map(str::to_string);
                let Some(step) = step else {
                    return Err(format!("There is nothing to {name}."));
                };
                if undo {
                    self.undo()
                } else {
                    self.redo()
                }
                Ok(
                    json!({if undo { "undid" } else { "redid" }: step, "next_undo": self.doc.undo_label(), "next_redo": self.doc.redo_label()}),
                )
            }
            "transport" => {
                let play = args.get("action").and_then(Value::as_str) == Some("play");
                if play != self.engine.transport().playing {
                    if play { self.play_toggle() } else { self.stop_return() }
                }
                Ok(json!({"playing": self.engine.transport().playing}))
            }
            other => Err(format!("Device has no tool {other:?}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rpc(id: u64, method: &str, params: Value) -> Value {
        json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params})
    }

    #[test]
    fn the_relay_answers_the_handshake_and_lists_tools_without_the_app() {
        let mut client = String::new();
        let mut never = |_: &str, _: &str, _: &Value| -> Result<Value, String> { panic!("no call expected") };
        let init =
            relay_message(&rpc(1, "initialize", json!({"clientInfo": {"name": "Claude Desktop"}})), &mut client, &mut never).unwrap();
        assert_eq!(init["result"]["capabilities"]["tools"], json!({}));
        assert_eq!(client, "Claude Desktop");
        assert!(relay_message(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}), &mut client, &mut never).is_none());
        let list = relay_message(&rpc(2, "tools/list", json!({})), &mut client, &mut never).unwrap();
        let names: Vec<&str> = list["result"]["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert_eq!(names, ["get_project", "get_builtin_devices", "apply_actions", "undo", "redo", "transport"]);
        let bad = relay_message(&rpc(3, "resources/list", json!({})), &mut client, &mut never).unwrap();
        assert_eq!(bad["error"]["code"], -32601);
    }

    #[test]
    fn builtin_devices_are_a_short_list_until_asked_for_parameters() {
        let list = builtin_devices(&json!({}));
        let list = list.as_array().unwrap();
        assert!(list.len() > 10 && list.iter().all(|d| d.get("params").is_none()));
        assert!(list.to_vec().iter().map(|d| d.to_string().len()).sum::<usize>() < 8_000, "the list stays small");
        let reverb = builtin_devices(&json!({"query": "REVERB"}));
        assert!(!reverb.as_array().unwrap().is_empty());
        assert!(reverb.as_array().unwrap().iter().all(|d| d.to_string().to_lowercase().contains("reverb")));
        let full = builtin_devices(&json!({"ids": ["compressor"]}));
        assert_eq!(full.as_array().unwrap().len(), 1);
        assert!(full[0]["params"].as_array().is_some_and(|p| !p.is_empty()));
    }

    #[test]
    fn a_tool_error_is_a_result_the_ai_app_can_read() {
        let mut client = "x".to_string();
        let mut down = |_: &str, _: &str, _: &Value| -> Result<Value, String> { Err(NOT_RUNNING.into()) };
        let r = relay_message(&rpc(4, "tools/call", json!({"name": "get_project", "arguments": {}})), &mut client, &mut down).unwrap();
        assert_eq!(r["result"]["isError"], true);
        assert!(r["result"]["content"][0]["text"].as_str().unwrap().contains("Let AI apps control Device"));
    }

    #[test]
    fn the_server_refuses_a_wrong_secret_and_forwards_calls_after_the_right_one() {
        let file = std::env::temp_dir().join(format!("dstudio-mcp-test-{}.json", std::process::id()));
        let (tx, rx) = mpsc::channel();
        let server = start(tx, Some(file.clone())).unwrap();
        let d: Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&file).unwrap().permissions().mode() & 0o777, 0o600);
        }

        let mut s = TcpStream::connect(("127.0.0.1", server.port)).unwrap();
        writeln!(s, "{}", json!({"secret": "0".repeat(32), "client": "x"})).unwrap();
        let mut line = String::new();
        BufReader::new(&s).read_line(&mut line).unwrap();
        assert!(line.contains("refused"), "{line}");

        // Answer calls on a stand-in UI thread.
        std::thread::spawn(move || {
            while let Ok(JobResult::Mcp(c)) = rx.recv() {
                let _ = c.reply.send(Ok(json!({"tool": c.name, "client": c.client})));
            }
        });
        let mut s = TcpStream::connect(("127.0.0.1", server.port)).unwrap();
        writeln!(s, "{}", json!({"secret": d["secret"], "client": "Test App"})).unwrap();
        writeln!(s, "{}", json!({"name": "get_project", "arguments": {}})).unwrap();
        let mut r = BufReader::new(s.try_clone().unwrap());
        line.clear();
        r.read_line(&mut line).unwrap();
        let v: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(v["ok"], json!({"tool": "get_project", "client": "Test App"}));

        drop(server);
        assert!(!file.exists(), "turning it off removes the discovery file");
    }

    #[test]
    fn the_setup_snippet_starts_the_relay() {
        let v: Value = serde_json::from_str(&setup_snippet()).unwrap();
        assert_eq!(v["mcpServers"]["device"]["args"], json!(["mcp"]));
    }
}
