//! "Report a bug" and "Share your feedback" (the two pills at the right of the top bar).
//!
//! Both open one modal form (`FeedbackModal` in ui/dialogs.slint). Sending posts JSON to
//! the feedback service (services/feedback in this repository), which files it as an issue
//! where the maintainers read it. Nothing secret ships in the app: the service holds the
//! credentials. The request runs on a worker thread and comes back through `poll_jobs`.
//!
//! What is sent: the message, the app version, an optional reply-to email, and, only when
//! "Include system info" is on (the default for bug reports), the OS, CPU architecture,
//! audio device settings and the last status-line messages. The form shows that text
//! before it is sent.

use std::cell::Cell;
use std::collections::VecDeque;
use std::time::Duration;

use serde_json::{Value, json};

use crate::controller::{Controller, JobResult};

/// Where reports go. `DSTUDIO_FEEDBACK_URL` overrides it (tests and local runs of the service).
pub const DEFAULT_URL: &str = "https://device-feedback.vercel.app/api/feedback";
/// Status-line messages kept for a bug report's "recent activity".
const RECENT_MAX: usize = 30;
pub const MESSAGE_MAX: usize = 10_000;
pub const EMAIL_MAX: usize = 254;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Kind {
    #[default]
    Bug,
    Feedback,
}

impl Kind {
    pub fn wire(self) -> &'static str {
        match self {
            Kind::Bug => "bug",
            Kind::Feedback => "feedback",
        }
    }
}

#[derive(Default)]
pub struct FeedbackForm {
    pub open: bool,
    pub kind: Kind,
    pub include_system: bool,
    /// Set while a request is in flight; the Send button is disabled.
    pub sending: bool,
    /// The last failure, shown above the buttons.
    pub error: String,
    /// New contents for the UI's text fields (message, email), applied on the next push:
    /// empty after a report was sent, a sample draft in the review states.
    pub set_fields: Cell<Option<(String, String)>>,
    /// Recent status-line messages, oldest first.
    pub recent: VecDeque<String>,
}

impl FeedbackForm {
    pub fn remember(&mut self, msg: &str) {
        if self.recent.len() == RECENT_MAX {
            self.recent.pop_front();
        }
        self.recent.push_back(msg.to_string());
    }
}

/// Checks the form before anything is sent; the error is shown in the form.
pub fn validate(message: &str, email: &str) -> Result<(), String> {
    let message = message.trim();
    let email = email.trim();
    if message.chars().count() < 3 {
        return Err("Please describe what happened first.".into());
    }
    if message.chars().count() > MESSAGE_MAX {
        return Err(format!("Please keep it under {MESSAGE_MAX} characters."));
    }
    if !email.is_empty() && !looks_like_email(email) {
        return Err("That email address doesn't look right.".into());
    }
    Ok(())
}

fn looks_like_email(s: &str) -> bool {
    if s.len() > EMAIL_MAX || s.chars().any(|c| c.is_whitespace() || "<>()[]\",;:".contains(c)) {
        return false;
    }
    match s.split_once('@') {
        Some((user, domain)) => {
            !user.is_empty() && !domain.contains('@') && domain.split('.').count() >= 2 && domain.split('.').all(|p| !p.is_empty())
        }
        None => false,
    }
}

/// The JSON body the service expects (services/feedback/lib/report.mjs).
pub fn payload(kind: Kind, message: &str, email: &str, system: Option<(&str, &str)>) -> Value {
    let (system, log) = system.unwrap_or(("", ""));
    json!({
        "kind": kind.wire(),
        "message": message.trim(),
        "email": email.trim(),
        "version": env!("CARGO_PKG_VERSION"),
        "system": system,
        "log": log,
    })
}

/// Posts a report. `Ok` once the service has accepted it; `Err` carries a sentence for the form.
pub fn send(url: &str, body: &Value) -> Result<(), String> {
    let agent: ureq::Agent =
        ureq::Agent::config_builder().http_status_as_error(false).timeout_global(Some(Duration::from_secs(20))).build().into();
    let mut resp = agent
        .post(url)
        .header("content-type", "application/json")
        .send_json(body)
        .map_err(|_| "Couldn't reach the server. Check your internet connection and try again.".to_string())?;
    let status = resp.status().as_u16();
    if (200..300).contains(&status) {
        return Ok(());
    }
    let text = resp.body_mut().read_to_string().unwrap_or_default();
    let msg = serde_json::from_str::<Value>(&text)
        .ok()
        .and_then(|v| v.get("error").and_then(Value::as_str).map(str::to_string))
        .unwrap_or_else(|| format!("The server couldn't take the report (HTTP {status}). Please try again later."));
    Err(msg)
}

pub fn url() -> String {
    std::env::var("DSTUDIO_FEEDBACK_URL").ok().filter(|u| !u.trim().is_empty()).unwrap_or_else(|| DEFAULT_URL.to_string())
}

/// A readable OS name and version, best effort.
fn os_version() -> String {
    let os = std::env::consts::OS;
    let run = |cmd: &str, args: &[&str]| {
        std::process::Command::new(cmd)
            .args(args)
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .filter(|s| !s.is_empty())
    };
    let v = match os {
        "macos" => run("sw_vers", &["-productVersion"]).map(|v| format!("macOS {v}")),
        "linux" => std::fs::read_to_string("/etc/os-release")
            .ok()
            .and_then(|t| t.lines().find_map(|l| l.strip_prefix("PRETTY_NAME=")).map(|v| v.trim_matches('"').to_string())),
        "windows" => run("cmd", &["/C", "ver"]),
        _ => None,
    };
    v.unwrap_or_else(|| os.to_string())
}

impl Controller {
    /// "Report a bug" / "Share your feedback": opens the form. A draft survives closing it.
    pub fn feedback_open(&mut self, kind: Kind) {
        if self.modal_open() && !self.feedback.open {
            return;
        }
        if self.feedback.kind != kind || !self.feedback.open {
            self.feedback.include_system = kind == Kind::Bug;
        }
        self.feedback.kind = kind;
        self.feedback.open = true;
        self.feedback.error.clear();
        self.ui_dirty = true;
    }

    pub fn feedback_close(&mut self) {
        self.feedback.open = false;
        self.feedback.error.clear();
        self.ui_dirty = true;
    }

    pub fn feedback_toggle_system(&mut self) {
        self.feedback.include_system = !self.feedback.include_system;
        self.ui_dirty = true;
    }

    /// The system info and recent activity a report would include (shown in the form).
    pub fn feedback_system_text(&self) -> (String, String) {
        let a = &self.settings.prefs.audio;
        let st = if self.settings.fake_devices { Default::default() } else { self.engine.device_status() };
        let output = a
            .output_name
            .clone()
            .unwrap_or_else(|| if st.output_name.is_empty() { "System default".into() } else { st.output_name.clone() });
        let rate = if a.sample_rate > 0.0 { a.sample_rate } else { st.sample_rate };
        let buffer = if a.buffer_size > 0 { a.buffer_size } else { st.buffer_size };
        let audio =
            if st.running { format!("{output}, {} Hz, {buffer} samples", rate.round() as i64) } else { format!("{output}, not running") };
        let system = format!("Device {}\n{} ({})\nAudio: {audio}", env!("CARGO_PKG_VERSION"), os_version(), std::env::consts::ARCH);
        let log = self.feedback.recent.iter().cloned().collect::<Vec<_>>().join("\n");
        (system, log)
    }

    /// Send: checks the fields, then posts on a worker thread.
    pub fn feedback_send(&mut self, message: &str, email: &str) {
        if self.feedback.sending {
            return;
        }
        if let Err(e) = validate(message, email) {
            self.feedback.error = e;
            self.ui_dirty = true;
            return;
        }
        let sys = self.feedback.include_system.then(|| self.feedback_system_text());
        let body = payload(self.feedback.kind, message, email, sys.as_ref().map(|(s, l)| (s.as_str(), l.as_str())));
        let url = url();
        let tx = self.jobs_tx.clone();
        self.feedback.sending = true;
        self.feedback.error.clear();
        self.pending_jobs += 1;
        let spawned = std::thread::Builder::new().name("feedback".into()).spawn(move || {
            let _ = tx.send(JobResult::Feedback(send(&url, &body)));
        });
        if let Err(e) = spawned {
            self.pending_jobs -= 1;
            self.feedback.sending = false;
            self.feedback.error = format!("Couldn't start sending: {e}");
        }
        self.ui_dirty = true;
    }

    /// Called from `poll_jobs` when the request finishes.
    pub fn finish_feedback(&mut self, result: Result<(), String>) {
        self.feedback.sending = false;
        match result {
            Ok(()) => {
                let thanks = match self.feedback.kind {
                    Kind::Bug => "Thanks! Your bug report was sent.",
                    Kind::Feedback => "Thanks! Your feedback was sent.",
                };
                self.feedback.open = false;
                self.feedback.error.clear();
                self.feedback.set_fields.set(Some(Default::default()));
                self.set_status(thanks, 4);
            }
            Err(e) => self.feedback.error = e,
        }
        self.ui_dirty = true;
    }

    pub(crate) fn push_feedback(&self, s: &crate::Studio) {
        let f = &self.feedback;
        s.set_feedback_open(f.open);
        s.set_feedback_bug(f.kind == Kind::Bug);
        s.set_feedback_system(f.include_system);
        s.set_feedback_sending(f.sending);
        s.set_feedback_error(f.error.as_str().into());
        if f.open && f.include_system {
            let (sys, log) = self.feedback_system_text();
            let text = if log.is_empty() { sys } else { format!("{sys}\n\nRecent activity:\n{log}") };
            s.set_feedback_system_text(text.into());
        }
        if let Some((message, email)) = f.set_fields.take() {
            s.set_feedback_message(message.into());
            s.set_feedback_email(email.into());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    #[test]
    fn validate_needs_a_message_and_a_sane_email() {
        assert!(validate("  ", "").is_err());
        assert!(validate("It crashed", "").is_ok());
        assert!(validate("It crashed", "me@example.com").is_ok());
        for bad in ["me", "me@", "@x.com", "me@x", "a b@x.com", "me@x..com"] {
            assert!(validate("It crashed", bad).is_err(), "{bad} should be rejected");
        }
        assert!(validate(&"x".repeat(MESSAGE_MAX + 1), "").is_err());
    }

    #[test]
    fn payload_matches_the_service_fields() {
        let p = payload(Kind::Bug, " Crash \n", " me@example.com ", Some(("Device 1", "Saved")));
        assert_eq!(p["kind"], "bug");
        assert_eq!(p["message"], "Crash");
        assert_eq!(p["email"], "me@example.com");
        assert_eq!(p["version"], env!("CARGO_PKG_VERSION"));
        assert_eq!(p["system"], "Device 1");
        assert_eq!(p["log"], "Saved");
        // Without system info nothing about the machine is sent.
        let p = payload(Kind::Feedback, "Nice", "", None);
        assert_eq!((p["kind"].as_str(), p["system"].as_str(), p["log"].as_str()), (Some("feedback"), Some(""), Some("")));
    }

    /// A one-shot HTTP server: answers the first request with `status` and `body`, and hands
    /// back the request it received.
    fn serve_once(status: &str, body: &'static str) -> (String, std::thread::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/api/feedback", listener.local_addr().unwrap());
        let status = status.to_string();
        let h = std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            let mut req = Vec::new();
            let mut buf = [0u8; 4096];
            // Read the headers, then Content-Length bytes of body.
            loop {
                let n = s.read(&mut buf).unwrap();
                req.extend_from_slice(&buf[..n]);
                let text = String::from_utf8_lossy(&req).to_string();
                if let Some(end) = text.find("\r\n\r\n") {
                    let len = text
                        .lines()
                        .find_map(|l| l.to_ascii_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse::<usize>().unwrap()))
                        .unwrap_or(0);
                    if req.len() >= end + 4 + len {
                        break;
                    }
                }
                if n == 0 {
                    break;
                }
            }
            let resp = format!(
                "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
            s.write_all(resp.as_bytes()).unwrap();
            String::from_utf8_lossy(&req).to_string()
        });
        (url, h)
    }

    #[test]
    fn send_posts_json_and_reports_success() {
        let (url, h) = serve_once("200 OK", r#"{"ok":true}"#);
        let body = payload(Kind::Feedback, "Love it", "", None);
        assert_eq!(send(&url, &body), Ok(()));
        let req = h.join().unwrap();
        assert!(req.starts_with("POST /api/feedback"), "{req}");
        let sent: Value = serde_json::from_str(req.split_once("\r\n\r\n").unwrap().1).unwrap();
        assert_eq!(sent["message"], "Love it", "{req}");
    }

    #[test]
    fn send_shows_the_server_error() {
        let (url, h) = serve_once("400 Bad Request", r#"{"error":"Please describe what happened."}"#);
        assert_eq!(send(&url, &payload(Kind::Bug, "abc", "", None)), Err("Please describe what happened.".into()));
        h.join().unwrap();
        // Nothing listening: a plain-language network error.
        let dead = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap();
        let err = send(&format!("http://{dead}/"), &json!({})).unwrap_err();
        assert!(err.contains("internet connection"), "{err}");
    }
}
