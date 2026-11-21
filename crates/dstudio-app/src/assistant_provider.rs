//! Assistant providers: turn a user prompt + project context into raw
//! `AssistantTurn` JSON. Runs on a worker thread; never touches the
//! `Document` (the controller validates the result with
//! `dstudio_core::assistant::plan` before anything is executed).
//!
//! * [`Provider::Anthropic`]: the Anthropic Messages API over HTTPS (`ureq`).
//!   The key is read from `ANTHROPIC_API_KEY` only, kept in memory, sent only
//!   in the `x-api-key` header and never logged or displayed.
//! * [`Provider::Fixture`]: **TEST provider** (`--assistant-fixture <file>`,
//!   and the built-in self-test fixture). It returns canned `AssistantTurn`
//!   JSON in order and makes no network calls. It exists so the full
//!   prompt → plan → commit → undo pipeline can be exercised offline; it is
//!   not a user-facing feature.
//! * [`Provider::Unavailable`]: no key and no fixture. The UI disables
//!   sending; the rest of the DAW is unaffected.

use dstudio_core::assistant::{Mode, tool_schema};
use serde_json::{Value, json};
use std::sync::Mutex;
use std::time::Duration;

/// Default model (claude-api skill: "ALWAYS use `claude-opus-5-5` unless the
/// user explicitly names a different model"). Override with `DSTUDIO_ASSISTANT_MODEL`.
pub const DEFAULT_MODEL: &str = "claude-opus-5-5";
const API_URL: &str = "https://api.anthropic.com/v1/messages";
const API_VERSION: &str = "2026-10-21";
const TOOL_NAME: &str = "daw_turn";

pub enum Provider {
    Anthropic {
        key: String,
        model: String,
    },
    /// TEST provider: canned turns, returned in order (the last one repeats).
    Fixture {
        turns: Vec<String>,
        next: Mutex<usize>,
        label: String,
    },
    Unavailable,
}

impl std::fmt::Debug for Provider {
    // Hand-written so the API key can never end up in a debug print.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Provider::Anthropic { model, .. } => write!(f, "Anthropic({model})"),
            Provider::Fixture { label, turns, .. } => write!(f, "Fixture({label}, {} turns)", turns.len()),
            Provider::Unavailable => write!(f, "Unavailable"),
        }
    }
}

/// Everything the provider needs for one request (plain data, `Send`).
#[derive(Debug, Clone)]
pub struct Request {
    pub mode: Mode,
    pub prompt: String,
    /// `assistant::describe_state(project)`.
    pub state: Value,
    /// Human-readable selection context, e.g. `track 1 "Audio" (id 1)`.
    pub selection: Option<String>,
    /// Earlier turns as (user prompt, assistant reply text), oldest first.
    pub history: Vec<(String, String)>,
}

impl Provider {
    /// Environment-based provider: Anthropic if `ANTHROPIC_API_KEY` is set.
    pub fn from_env() -> Provider {
        match std::env::var("ANTHROPIC_API_KEY") {
            Ok(key) if !key.trim().is_empty() => {
                let model =
                    std::env::var("DSTUDIO_ASSISTANT_MODEL").ok().filter(|m| !m.trim().is_empty()).unwrap_or_else(|| DEFAULT_MODEL.into());
                Provider::Anthropic { key: key.trim().to_string(), model }
            }
            _ => Provider::Unavailable,
        }
    }

    /// Anthropic with `key` (from the keychain), the model as in [Self::from_env].
    pub fn from_key(key: &str) -> Provider {
        let model = std::env::var("DSTUDIO_ASSISTANT_MODEL").ok().filter(|m| !m.trim().is_empty()).unwrap_or_else(|| DEFAULT_MODEL.into());
        Provider::Anthropic { key: key.trim().to_string(), model }
    }

    /// TEST provider from JSON text: one `AssistantTurn` object or an array of them.
    pub fn fixture_from_json(text: &str, label: &str) -> Result<Provider, String> {
        let v: Value = serde_json::from_str(text).map_err(|e| format!("fixture {label}: {e}"))?;
        let turns = match v {
            Value::Array(items) => items.iter().map(|t| t.to_string()).collect::<Vec<_>>(),
            obj @ Value::Object(_) => vec![obj.to_string()],
            _ => return Err(format!("fixture {label}: expected an object or an array")),
        };
        if turns.is_empty() {
            return Err(format!("fixture {label}: empty"));
        }
        Ok(Provider::Fixture { turns, next: Mutex::new(0), label: label.into() })
    }

    pub fn available(&self) -> bool {
        !matches!(self, Provider::Unavailable)
    }

    pub fn is_fixture(&self) -> bool {
        matches!(self, Provider::Fixture { .. })
    }

    /// Blocking: call on a worker thread. Returns raw `AssistantTurn` JSON.
    pub fn complete(&self, req: &Request) -> Result<String, String> {
        match self {
            Provider::Unavailable => Err("Add your Anthropic API key to use the assistant".into()),
            Provider::Fixture { turns, next, .. } => {
                let mut n = next.lock().map_err(|_| "fixture lock poisoned".to_string())?;
                let t = turns[(*n).min(turns.len() - 1)].clone();
                *n += 1;
                Ok(t)
            }
            Provider::Anthropic { key, model } => call_anthropic(key, model, req),
        }
    }
}

/// System prompt: what the DAW is, the turn format and the current mode's rules.
pub fn system_prompt(mode: Mode) -> String {
    let mode_rules = match mode {
        Mode::Assist => {
            "MODE: ASSIST. You may change the project with `edit`, `set_track_volume_db`, `add_builtin_device` and `load_sample` actions. \
             All edits in one turn are validated together and applied as ONE undo step; if any edit is invalid, nothing is applied. \
             Use real ids from the project state. To refer to a track you create in the same turn, give it an explicit id equal to \
             `next_id_hint` (then +1 for each further new object) instead of 0. You may also highlight controls and explain."
        }
        Mode::Teach => {
            "MODE: TEACH. You must NEVER change the project: do not emit `edit`, `set_track_volume_db`, `add_builtin_device` or `load_sample` actions (they are rejected and \
             the whole turn fails). Only `highlight` the controls the user should use (each with a short note on what to do) and \
             `explain` the steps in plain language, so the user learns to do it themselves."
        }
    };
    format!(
        "You are the assistant inside Device, a desktop digital audio workstation (DAW). The window has a header with a \
         transport bar (play, loop, bar/time readouts, BPM, key, time signature, metronome, master volume), a sound browser on the \
         left, a timeline with track headers (mute, solo, arm, pan knob, volume slider) and lanes holding audio clips, a master row, \
         and a bottom panel with the track inspector and devices, which can switch to a mixer. Time positions are in ticks (960 per \
         quarter note). Volumes in the state are in dB. Each track lists its effect chain (`devices`, in order) and its `instrument`, \
         each with its id and the parameters changed from their defaults; the built-in device list after these rules gives every \
         built-in effect and instrument you can add, with each parameter as [id, min, max, default, unit]; `assets` are the \
         project's audio files, which audio clips and sample slots can use; \
         `sends` go into return tracks.\n\n\
         Always answer by calling the `{TOOL_NAME}` tool exactly once with the actions to perform; put any prose for the user in \
         `explain` actions. Keep explanations short (one to three sentences). Highlight targets must be ids from the tool schema \
         (`UI target` list) or `track:<id>:<header|mute|solo|arm|pan|volume|lane>` for an existing track.\n\n{mode_rules}"
    )
}

fn user_content(req: &Request) -> String {
    let mut s = String::new();
    s.push_str("Project state (JSON):\n");
    s.push_str(&req.state.to_string());
    s.push_str("\n\nCurrent selection: ");
    s.push_str(req.selection.as_deref().unwrap_or("nothing selected"));
    s.push_str("\n\nRequest: ");
    s.push_str(&req.prompt);
    s
}

/// Models that reject forced `tool_choice` (`any`/`tool`) with a 400, per the
/// claude-api skill. For those we use `auto` + the system-prompt instruction.
fn rejects_forced_tool_choice(model: &str) -> bool {
    ["claude-opus-5-5", "claude-sonnet-5-5", "claude-fable-5-1", "claude-mythos-5-1"].iter().any(|m| model.starts_with(m))
}

/// Models that accept the server-side `fallbacks: "default"` parameter.
fn supports_default_fallback(model: &str) -> bool {
    ["claude-opus-5-5", "claude-opus-5", "claude-fable-5-1", "claude-sonnet-5-5"].contains(&model)
}

/// Request body for the Messages API (pure; unit-tested).
pub fn build_body(model: &str, req: &Request) -> Value {
    let mut messages = Vec::new();
    for (u, a) in &req.history {
        messages.push(json!({"role": "user", "content": u}));
        messages.push(json!({"role": "assistant", "content": a}));
    }
    messages.push(json!({"role": "user", "content": user_content(req)}));
    let tool_choice = if rejects_forced_tool_choice(model) {
        json!({"type": "auto", "disable_parallel_tool_use": true})
    } else {
        json!({"type": "tool", "name": TOOL_NAME})
    };
    let mut body = json!({
        "model": model,
        "max_tokens": 16000,
        // The built-in device catalogue is long and never changes, so it is the cached part of the
        // prompt (with the tool schema before it); the rules above it are short.
        "system": [
            {"type": "text", "text": system_prompt(req.mode)},
            {"type": "text", "text": format!("Built-in devices (JSON):\n{}", dstudio_core::assistant::builtin_catalog()), "cache_control": {"type": "ephemeral"}},
        ],
        "tools": [{
            "name": TOOL_NAME,
            "description": "Perform one assistant turn in the DAW: a list of edits (Assist mode only), UI highlights and explanations.",
            "input_schema": tool_schema(),
        }],
        "tool_choice": tool_choice,
        "messages": messages,
    });
    if model.starts_with("claude-opus-5") || model.starts_with("claude-sonnet-5-5") || model.starts_with("claude-fable-5") {
        // Thinking stays adaptive (default); effort set explicitly (Opus 5.5 defaults to medium).
        body["output_config"] = json!({"effort": "medium"});
    }
    if supports_default_fallback(model) {
        body["fallbacks"] = json!("default");
    }
    body
}

/// Extract the turn from a Messages API response (pure; unit-tested).
pub fn parse_response(v: &Value) -> Result<String, String> {
    if let Some(err) = v.get("error") {
        return Err(format!("API error: {}", err.get("message").and_then(Value::as_str).unwrap_or("unknown")));
    }
    let stop = v.get("stop_reason").and_then(Value::as_str).unwrap_or("");
    if stop == "refusal" {
        return Err("The model declined this request.".into());
    }
    let content = v.get("content").and_then(Value::as_array).cloned().unwrap_or_default();
    if let Some(tu) = content
        .iter()
        .find(|b| b.get("type").and_then(Value::as_str) == Some("tool_use") && b.get("name").and_then(Value::as_str) == Some(TOOL_NAME))
    {
        let input = tu.get("input").cloned().unwrap_or(Value::Null);
        return Ok(input.to_string());
    }
    if stop == "max_tokens" {
        return Err("The response was cut off (max_tokens).".into());
    }
    // No tool call (possible with tool_choice auto): treat the text as an explanation only.
    let text: Vec<&str> = content
        .iter()
        .filter(|b| b.get("type").and_then(Value::as_str) == Some("text"))
        .filter_map(|b| b.get("text").and_then(Value::as_str))
        .collect();
    let text = text.join("\n").trim().to_string();
    if text.is_empty() {
        return Err("The model returned no actions.".into());
    }
    Ok(json!({"actions": [{"type": "explain", "text": text}]}).to_string())
}

fn call_anthropic(key: &str, model: &str, req: &Request) -> Result<String, String> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .http_status_as_error(false)
        .timeout_global(Some(Duration::from_secs(180)))
        // The system's trusted certificates (Keychain, Windows store, the Linux CA bundle and
        // SSL_CERT_FILE), so the assistant also works behind a proxy that re-signs HTTPS with
        // a company certificate.
        .tls_config(ureq::tls::TlsConfig::builder().root_certs(ureq::tls::RootCerts::PlatformVerifier).build())
        .build()
        .into();
    let mut builder =
        agent.post(API_URL).header("x-api-key", key).header("anthropic-version", API_VERSION).header("content-type", "application/json");
    if supports_default_fallback(model) {
        builder = builder.header("anthropic-beta", "server-side-fallback-2026-07-01");
    }
    let body = build_body(model, req);
    let mut resp = builder.send_json(&body).map_err(|e| format!("Network error: {e}"))?;
    let status = resp.status().as_u16();
    let text = resp.body_mut().read_to_string().map_err(|e| format!("Network error reading the response: {e}"))?;
    let v: Value = serde_json::from_str(&text).map_err(|_| format!("Unexpected response from the API (HTTP {status})"))?;
    if status != 200 {
        let msg = v.pointer("/error/message").and_then(Value::as_str).unwrap_or("request failed");
        return Err(format!("API error (HTTP {status}): {msg}"));
    }
    parse_response(&v)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(mode: Mode) -> Request {
        Request {
            mode,
            prompt: "make it faster".into(),
            state: json!({"tempo_bpm": 120}),
            selection: Some("track 1".into()),
            history: vec![("hi".into(), "Hello.".into())],
        }
    }

    #[test]
    fn body_shape_for_default_model() {
        let b = build_body(DEFAULT_MODEL, &req(Mode::Teach));
        assert_eq!(b["model"], DEFAULT_MODEL);
        assert_eq!(b["tools"][0]["name"], TOOL_NAME);
        assert_eq!(b["tools"][0]["input_schema"], tool_schema());
        // Opus 5.5 rejects forced tool use: auto + one call at most.
        assert_eq!(b["tool_choice"]["type"], "auto");
        assert_eq!(b["fallbacks"], "default");
        assert!(b["system"][0]["text"].as_str().unwrap().contains("MODE: TEACH"));
        // The long, unchanging device catalogue is the cached block.
        assert_eq!(b["system"][1]["cache_control"]["type"], "ephemeral");
        assert!(b["system"][1]["text"].as_str().unwrap().contains("\"compressor\""));
        assert_eq!(b["messages"].as_array().unwrap().len(), 3);
        assert!(b.get("temperature").is_none());
    }

    #[test]
    fn older_models_get_forced_tool_choice() {
        let b = build_body("claude-opus-4-8", &req(Mode::Assist));
        assert_eq!(b["tool_choice"], json!({"type": "tool", "name": TOOL_NAME}));
        assert!(b.get("fallbacks").is_none());
    }

    #[test]
    fn response_parsing() {
        let ok = json!({"stop_reason": "tool_use", "content": [{"type": "text", "text": "x"}, {"type": "tool_use", "id": "t", "name": TOOL_NAME, "input": {"actions": []}}]});
        assert_eq!(parse_response(&ok).unwrap(), r#"{"actions":[]}"#);
        let refusal = json!({"stop_reason": "refusal", "content": []});
        assert!(parse_response(&refusal).is_err());
        let text = json!({"stop_reason": "end_turn", "content": [{"type": "text", "text": "Use the BPM field."}]});
        assert!(parse_response(&text).unwrap().contains("explain"));
    }

    #[test]
    fn fixture_returns_turns_in_order() {
        let p = Provider::fixture_from_json(r#"[{"actions":[]},{"actions":[{"type":"explain","text":"b"}]}]"#, "t").unwrap();
        let r = req(Mode::Assist);
        assert_eq!(p.complete(&r).unwrap(), r#"{"actions":[]}"#);
        assert!(p.complete(&r).unwrap().contains("\"b\""));
        assert!(p.complete(&r).unwrap().contains("\"b\""), "last turn repeats");
        assert!(format!("{:?}", Provider::Anthropic { key: "secret".into(), model: "m".into() }).find("secret").is_none());
    }
}

/// Real requests to the Anthropic API, end to end: prompt → provider → `plan` → `commit`, on a
/// fresh default project per prompt. Ignored by default (each run costs a few cents); run with
/// `DSTUDIO_TEST_ANTHROPIC_API_KEY` or `ANTHROPIC_API_KEY` set:
///
/// `cargo test -p dstudio-app --release live_api -- --ignored --nocapture --test-threads 1`
#[cfg(test)]
mod live_api {
    use super::*;
    use dstudio_core::assistant::{self, Plan};
    use dstudio_core::history::Document;
    use dstudio_core::{Project, TrackKind, builtin, gain_to_db};
    use std::time::Instant;

    /// `DSTUDIO_TEST_ANTHROPIC_API_KEY` first (a name no other tool reads, for environments that
    /// keep `ANTHROPIC_API_KEY` for themselves), then the app's own `ANTHROPIC_API_KEY`.
    fn provider() -> Option<Provider> {
        if let Ok(key) = std::env::var("DSTUDIO_TEST_ANTHROPIC_API_KEY")
            && !key.trim().is_empty()
        {
            return Some(Provider::from_key(&key));
        }
        match Provider::from_env() {
            Provider::Unavailable => {
                eprintln!("Neither DSTUDIO_TEST_ANTHROPIC_API_KEY nor ANTHROPIC_API_KEY is set: skipping the live assistant test");
                None
            }
            p => Some(p),
        }
    }

    /// One turn the way the panel sends it; the plan is committed in Assist mode.
    fn turn(p: &Provider, doc: &mut Document, mode: Mode, prompt: &str) -> Plan {
        let mut state = assistant::describe_state(doc.project());
        state["next_id_hint"] = json!(doc.project().next_id);
        let req = Request { mode, prompt: prompt.into(), state, selection: None, history: Vec::new() };
        let started = Instant::now();
        let raw = p.complete(&req).unwrap_or_else(|e| panic!("{prompt:?}: request failed: {e}"));
        let secs = started.elapsed().as_secs_f32();
        let parsed = assistant::parse_turn(&raw).unwrap_or_else(|e| panic!("{prompt:?}: {e}\n{raw}"));
        let plan = assistant::plan(doc, mode, &parsed).unwrap_or_else(|e| panic!("{prompt:?}: plan rejected: {e}\n{raw}"));
        assistant::commit(doc, &plan).unwrap_or_else(|e| panic!("{prompt:?}: commit failed: {e}"));
        eprintln!(
            "\n[{secs:.1} s] {prompt}\n  edits: {:?}\n  highlights: {:?}\n  said: {:?}",
            plan.summary, plan.highlights, plan.explanations
        );
        plan
    }

    fn builtin_on<'a>(doc: &'a Document, track: &str, id: &str) -> Option<&'a dstudio_core::Device> {
        let t = doc.project().tracks.iter().find(|t| t.name == track)?;
        t.devices.iter().find(|d| builtin::def_of(d).is_some_and(|def| def.id == id))
    }

    #[test]
    #[ignore = "calls the real Anthropic API"]
    fn live_api_turns_change_the_project() {
        let Some(p) = provider() else { return };

        let mut doc = Document::new(Project::default());
        turn(&p, &mut doc, Mode::Assist, "Add a new instrument track called Bass and write a 4-bar bassline in C minor on it.");
        let bass = doc
            .project()
            .tracks
            .iter()
            .find(|t| t.kind == TrackKind::Instrument && t.name.contains("Bass"))
            .expect("a Bass instrument track");
        let notes: usize = bass.midi_clips.iter().map(|c| c.notes.len()).sum();
        assert!(notes >= 4, "the bassline has {notes} notes");
        assert_eq!(doc.undo_label(), Some("Assistant Edit"), "one undo step");

        let mut doc = Document::new(Project::default());
        turn(&p, &mut doc, Mode::Assist, "Put a reverb on the Audio track with the mix at 45%.");
        let reverb = builtin_on(&doc, "Audio", "reverb").expect("a reverb on Audio");
        assert!((reverb.params["mix"] - 45.0).abs() < 0.5, "mix is {}", reverb.params["mix"]);

        let mut doc = Document::new(Project::default());
        turn(&p, &mut doc, Mode::Assist, "Add a compressor to the Audio track and set its ratio to 6:1.");
        let comp = builtin_on(&doc, "Audio", "compressor").expect("a compressor on Audio");
        assert!((comp.params["ratio"] - 6.0).abs() < 0.05, "ratio is {}", comp.params["ratio"]);

        let mut doc = Document::new(Project::default());
        turn(&p, &mut doc, Mode::Assist, "Set the tempo to 90 BPM and turn the Instrument track down to -6 dB.");
        assert_eq!(doc.project().transport.bpm, 90.0);
        let inst = doc.project().tracks.iter().find(|t| t.name == "Instrument").unwrap();
        assert!((gain_to_db(inst.gain) + 6.0).abs() < 0.1, "volume is {} dB", gain_to_db(inst.gain));

        let mut doc = Document::new(Project::default());
        let plan = turn(&p, &mut doc, Mode::Teach, "How do I change the tempo?");
        assert!(plan.batch.is_none(), "Point mode never edits");
        assert!(!plan.highlights.is_empty() || !plan.explanations.is_empty(), "Point mode shows or explains something");
    }
}
