//! Crash reports (spec 018, roadmap G14; Settings › Privacy › Send crash reports).
//!
//! When the app crashes, [install]'s handlers write a small record, `crash-report.json`, next to
//! the preferences. The next launch reads and deletes it ([take]) and, when the switch is on,
//! sends it once through the feedback service as kind `crash` (`Controller::send_crash_report`).
//!
//! - A Rust panic records the panic message, where it happened and a backtrace.
//! - On macOS and Linux a fatal signal (a native crash in the engine or a plugin) records only the
//!   signal's name: the handler may call nothing but async-signal-safe functions, so it writes
//!   text built at start-up with `open`/`write`/`close`, puts the previous handler back and
//!   raises the signal again, so the crash goes on to the system's crash reporter as before.
//! - Windows records panics only.
//!
//! Nothing personal is sent ([scrub]): home folder paths become `~`, the user name is replaced,
//! and the report has no email, project, audio device or recent activity. Runs that don't use
//! the preferences (self-test, `--state`, screenshots) never install the handlers or send.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::controller::{Controller, JobResult};

pub const FILE: &str = "crash-report.json";
/// The service's `log` limit is 20,000 characters; the backtrace is cut well below it.
const BACKTRACE_MAX: usize = 16_000;
const MESSAGE_MAX: usize = 2_000;

/// What the handlers write and the next launch sends.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CrashRecord {
    /// The version that crashed.
    pub version: String,
    pub arch: String,
    /// The panic message, or the signal's name for a native crash.
    pub message: String,
    /// `file:line:column` of a panic; empty for a signal.
    #[serde(default)]
    pub location: String,
    #[serde(default)]
    pub backtrace: String,
}

static PATH: OnceLock<PathBuf> = OnceLock::new();
/// A panic was recorded: the abort that may follow must not overwrite it with a bare signal.
static RECORDED: AtomicBool = AtomicBool::new(false);

/// Installs the panic hook and, on macOS and Linux, the fatal-signal handlers, recording into
/// `dir` (the preferences folder).
pub fn install(dir: &Path) {
    let path = dir.join(FILE);
    if PATH.set(path).is_err() {
        return;
    }
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        if !RECORDED.swap(true, Ordering::SeqCst) {
            let message = info
                .payload()
                .downcast_ref::<&str>()
                .map(|s| s.to_string())
                .or_else(|| info.payload().downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "panic".into());
            let location = info.location().map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column())).unwrap_or_default();
            let backtrace = std::backtrace::Backtrace::force_capture().to_string();
            let thread = std::thread::current().name().unwrap_or("unnamed").to_string();
            let record = CrashRecord {
                version: env!("CARGO_PKG_VERSION").into(),
                arch: std::env::consts::ARCH.into(),
                message: format!("{message} (thread {thread})"),
                location,
                backtrace,
            };
            if let Some(path) = PATH.get() {
                let _ = write(path, &record);
            }
        }
        previous(info);
    }));
    #[cfg(unix)]
    signals::install();
}

/// Writes `record` to `path`.
pub fn write(path: &Path, record: &CrashRecord) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, serde_json::to_vec(record).map_err(std::io::Error::other)?)
}

/// The record of a crash in an earlier run, if there is one, removed from `dir` so it is only
/// ever handled once. A file that doesn't parse is removed too.
pub fn take(dir: &Path) -> Option<CrashRecord> {
    let path = dir.join(FILE);
    let text = std::fs::read(&path).ok()?;
    let _ = std::fs::remove_file(&path);
    serde_json::from_slice(&text).ok()
}

/// Replaces home folder paths with `~` and the user name with `<user>`, in `text`.
pub fn scrub(text: &str, home: Option<&str>, user: Option<&str>) -> String {
    let mut s = text.to_string();
    if let Some(home) = home.map(|h| h.trim_end_matches(['/', '\\'])).filter(|h| h.len() > 1) {
        s = s.replace(home, "~");
    }
    // Other people's home folders too (a build machine's paths in a backtrace).
    for prefix in ["/Users/", "/home/", "C:\\Users\\", "C:/Users/"] {
        let mut out = String::with_capacity(s.len());
        let mut rest = s.as_str();
        while let Some(i) = rest.find(prefix) {
            out.push_str(&rest[..i]);
            let after = &rest[i + prefix.len()..];
            let end = after.find(['/', '\\', '\n', ' ', ':', ')']).unwrap_or(after.len());
            out.push('~');
            rest = &after[end..];
        }
        out.push_str(rest);
        s = out;
    }
    if let Some(user) = user.filter(|u| u.chars().count() >= 3) {
        s = replace_word(&s, user, "<user>");
    }
    s
}

/// Replaces `word` where it isn't part of a longer word (letters, digits or `_` around it).
fn replace_word(text: &str, word: &str, with: &str) -> String {
    let is_word = |c: char| c.is_alphanumeric() || c == '_';
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(i) = rest.find(word) {
        let before = rest[..i].chars().next_back();
        let after = rest[i + word.len()..].chars().next();
        out.push_str(&rest[..i]);
        if before.is_some_and(is_word) || after.is_some_and(is_word) {
            out.push_str(word);
        } else {
            out.push_str(with);
        }
        rest = &rest[i + word.len()..];
    }
    out.push_str(rest);
    out
}

fn cut(s: &str, max: usize) -> String {
    if s.chars().count() <= max { s.to_string() } else { s.chars().take(max).collect::<String>() + "\n…" }
}

/// The JSON the feedback service takes for `record` (services/feedback/lib/report.mjs), with
/// the OS of the machine sending it (the one that crashed).
pub fn payload(record: &CrashRecord, os: &str, home: Option<&str>, user: Option<&str>) -> Value {
    let clean = |t: &str| scrub(t, home, user);
    let first = clean(&record.message).lines().next().unwrap_or("").trim().to_string();
    let mut log = clean(&record.location);
    if !record.backtrace.is_empty() {
        log = format!("{log}\n\n{}", cut(&clean(&record.backtrace), BACKTRACE_MAX));
    }
    json!({
        "kind": "crash",
        "message": cut(&format!("Device crashed: {first}\n\n{}", clean(&record.message)), MESSAGE_MAX),
        "email": "",
        "version": record.version,
        "system": format!("Device {}\n{os} ({})", record.version, record.arch),
        "log": log.trim(),
    })
}

fn home_and_user() -> (Option<String>, Option<String>) {
    let home = std::env::var("HOME").or_else(|_| std::env::var("USERPROFILE")).ok();
    let user = std::env::var("USER").or_else(|_| std::env::var("USERNAME")).ok();
    (home, user)
}

impl Controller {
    /// At launch (runs that use the preferences only): a crash recorded in `dir` by an earlier
    /// run is sent once when Settings › Privacy › Send crash reports is on, and deleted either way.
    pub fn send_crash_report(&mut self, dir: &Path) {
        let Some(record) = take(dir) else { return };
        if self.settings.prefs.crash_reports_off {
            return;
        }
        let (home, user) = home_and_user();
        let tx = self.jobs_tx.clone();
        self.pending_jobs += 1;
        let spawned = std::thread::Builder::new().name("crash-report".into()).spawn(move || {
            let body = payload(&record, &crate::feedback::os_version(), home.as_deref(), user.as_deref());
            let _ = tx.send(JobResult::CrashReport(crate::feedback::send(&crate::feedback::url(), &body)));
        });
        if spawned.is_err() {
            self.pending_jobs -= 1;
        }
    }

    /// Called from `poll_jobs`: a quiet line in the status bar, nothing on failure.
    pub fn finish_crash_report(&mut self, result: Result<(), String>) {
        if result.is_ok() {
            self.set_status("Sent a crash report from the last session. Thanks.", 5);
        }
    }

    /// Settings › Privacy › Send crash reports.
    pub fn privacy_toggle_crash_reports(&mut self) {
        self.settings.prefs.crash_reports_off = !self.settings.prefs.crash_reports_off;
        self.save_prefs();
        self.ui_dirty = true;
    }
}

/// A deliberate crash for checking the reports by hand: `DSTUDIO_CRASH_TEST=panic` or `segv`.
pub fn crash_test_from_env() {
    match std::env::var("DSTUDIO_CRASH_TEST").as_deref() {
        Ok("panic") => panic!("DSTUDIO_CRASH_TEST: a test panic"),
        #[cfg(unix)]
        // SAFETY: deliberately faults, to exercise the signal handler; the process dies.
        Ok("segv") => unsafe { std::ptr::write_volatile(std::ptr::dangling_mut::<u8>().wrapping_sub(1), 1) },
        _ => {}
    }
}

#[cfg(unix)]
mod signals {
    use super::{CrashRecord, PATH, RECORDED};
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    use std::sync::OnceLock;
    use std::sync::atomic::Ordering;

    const SIGNALS: [(libc::c_int, &str); 5] = [
        (libc::SIGSEGV, "SIGSEGV"),
        (libc::SIGBUS, "SIGBUS"),
        (libc::SIGILL, "SIGILL"),
        (libc::SIGFPE, "SIGFPE"),
        (libc::SIGABRT, "SIGABRT"),
    ];

    /// Everything the handler needs, built before any crash: the file's path, each signal's
    /// record, and the handler that was there before ours.
    struct Prepared {
        path: CString,
        records: Vec<(libc::c_int, Vec<u8>)>,
        previous: Vec<(libc::c_int, libc::sigaction)>,
    }
    // SAFETY: written once before the handlers are installed and only read afterwards.
    unsafe impl Sync for Prepared {}
    unsafe impl Send for Prepared {}

    static PREPARED: OnceLock<Prepared> = OnceLock::new();

    pub fn install() {
        let Some(path) = PATH.get().and_then(|p| CString::new(p.as_os_str().as_bytes()).ok()) else { return };
        let records = SIGNALS
            .iter()
            .map(|&(sig, name)| {
                let r = CrashRecord {
                    version: env!("CARGO_PKG_VERSION").into(),
                    arch: std::env::consts::ARCH.into(),
                    message: format!("{name}: a native crash (in the engine or a plugin)"),
                    location: String::new(),
                    backtrace: String::new(),
                };
                (sig, serde_json::to_vec(&r).unwrap_or_default())
            })
            .collect();
        let mut previous = Vec::new();
        // SAFETY: plain sigaction calls with zeroed structs; the handler only uses
        // async-signal-safe functions (see on_signal).
        unsafe {
            for &(sig, _) in &SIGNALS {
                let mut old: libc::sigaction = std::mem::zeroed();
                if libc::sigaction(sig, std::ptr::null(), &mut old) == 0 {
                    previous.push((sig, old));
                }
            }
            if PREPARED.set(Prepared { path, records, previous }).is_err() {
                return;
            }
            for &(sig, _) in &SIGNALS {
                let mut act: libc::sigaction = std::mem::zeroed();
                act.sa_sigaction = on_signal as *const () as libc::sighandler_t;
                act.sa_flags = libc::SA_SIGINFO | libc::SA_ONSTACK;
                libc::sigemptyset(&mut act.sa_mask);
                libc::sigaction(sig, &act, std::ptr::null_mut());
            }
        }
    }

    extern "C" fn on_signal(sig: libc::c_int, _info: *mut libc::siginfo_t, _ctx: *mut libc::c_void) {
        let Some(p) = PREPARED.get() else { return };
        // SAFETY: open, write, close, sigaction and raise are async-signal-safe; the buffers
        // were built before the handler was installed and are never freed.
        unsafe {
            if !RECORDED.swap(true, Ordering::SeqCst)
                && let Some((_, bytes)) = p.records.iter().find(|(s, _)| *s == sig)
            {
                let fd = libc::open(p.path.as_ptr(), libc::O_WRONLY | libc::O_CREAT | libc::O_TRUNC, 0o600 as libc::c_uint);
                if fd >= 0 {
                    libc::write(fd, bytes.as_ptr().cast(), bytes.len());
                    libc::close(fd);
                }
            }
            // Put the previous handler (the system's, or Rust's stack-overflow check) back and
            // raise the signal again: it is delivered when this handler returns, and a fault
            // repeats anyway, so the crash ends as it would have without us.
            if let Some((_, old)) = p.previous.iter().find(|(s, _)| *s == sig) {
                libc::sigaction(sig, old, std::ptr::null_mut());
            } else {
                libc::signal(sig, libc::SIG_DFL);
            }
            libc::raise(sig);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record() -> CrashRecord {
        CrashRecord {
            version: "0.2.0".into(),
            arch: "aarch64".into(),
            message: "failed to open /Users/sam/Music/My Song.dsproj (thread main)".into(),
            location: "crates/dstudio-app/src/files.rs:10:5".into(),
            backtrace: "0: device_studio::files::open\n   at /Users/runner/work/device/src/files.rs:10:5\n1: std::rt\n   at /home/sam/.cargo/registry/x.rs:1".into(),
        }
    }

    #[test]
    fn home_paths_and_the_user_name_are_removed() {
        let s = scrub("/Users/sam/Music/a.wav by sam, not sammy, in C:\\Users\\Sam\\x and /home/kim/y", Some("/Users/sam"), Some("sam"));
        assert_eq!(s, "~/Music/a.wav by <user>, not sammy, in ~\\x and ~/y");
        assert_eq!(scrub("device studio", Some("/"), Some("dev")), "device studio", "a short name inside a word stays");
    }

    #[test]
    fn a_record_round_trips_through_its_file_once() {
        let dir = std::env::temp_dir().join(format!("dstudio-crash-test-{}", std::process::id()));
        write(&dir.join(FILE), &record()).unwrap();
        assert_eq!(take(&dir), Some(record()));
        assert_eq!(take(&dir), None, "taken once");
        std::fs::write(dir.join(FILE), "not json").unwrap();
        assert_eq!(take(&dir), None);
        assert!(!dir.join(FILE).exists(), "a broken record is removed too");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn the_payload_has_nothing_personal() {
        let p = payload(&record(), "macOS 15.1", Some("/Users/sam"), Some("sam"));
        assert_eq!(p["kind"], "crash");
        assert_eq!(p["email"], "");
        assert_eq!(p["version"], "0.2.0");
        assert_eq!(p["system"], "Device 0.2.0\nmacOS 15.1 (aarch64)");
        let message = p["message"].as_str().unwrap();
        assert!(message.starts_with("Device crashed: failed to open ~/Music/My Song.dsproj (thread main)"), "{message}");
        let all = p.to_string();
        assert!(!all.contains("sam") && !all.contains("runner"), "{all}");
        assert!(p["log"].as_str().unwrap().starts_with("crates/dstudio-app/src/files.rs:10:5\n\n0: device_studio::files::open"));
    }
}
