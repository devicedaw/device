//! Updates (spec 022): Options › Check for Updates… and the "Update available" pill in the title bar.
//!
//! Once at launch (quietly: nothing is shown when off-line or when GitHub refuses) and from the menu,
//! the app asks GitHub for the latest published release and compares its version with the running
//! one. When a newer one exists, a blue "Update available" pill appears left of Report a bug; it and
//! the menu open the Check for Updates dialog, which shows the new version and a short summary of
//! its notes, and Download opens the release's page in the browser.
//!
//! The launch check reuses the last answer for 12 hours (`update-check.json` next to the
//! preferences), so quick restarts don't ask GitHub again. Nothing personal is sent: the request
//! carries `User-Agent: Device/<version>` and GitHub's API headers, nothing else.

use std::cmp::Ordering;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::controller::{Controller, JobResult};

/// The newest published release (GitHub leaves out drafts and pre-releases).
/// `DSTUDIO_UPDATE_URL` overrides it (local runs against a test server); `DSTUDIO_UPDATE_URL=off`
/// turns every check off.
pub const LATEST_URL: &str = "https://api.github.com/repos/devicedaw/device/releases/latest";
const CURRENT: &str = env!("CARGO_PKG_VERSION");
/// How long a launch check reuses the last answer instead of asking GitHub.
pub const LAUNCH_INTERVAL_SECS: u64 = 12 * 60 * 60;
/// Release-note lines the dialog shows.
const NOTE_LINES: usize = 3;
const NOTE_CHARS: usize = 110;

pub fn latest_url() -> Option<String> {
    match std::env::var("DSTUDIO_UPDATE_URL") {
        Ok(u) if u.trim() == "off" => None,
        Ok(u) if !u.trim().is_empty() => Some(u.trim().to_string()),
        _ => Some(LATEST_URL.to_string()),
    }
}

// ------------------------------------------------------------------ versions

/// A SemVer version (`1.2.3`, `1.2.3-beta.2`); a leading `v` is allowed, build metadata ignored.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Version {
    core: [u64; 3],
    pre: Vec<String>,
}

impl Version {
    pub fn parse(s: &str) -> Option<Version> {
        let s = s.trim();
        let s = s.strip_prefix('v').unwrap_or(s);
        let s = s.split('+').next()?;
        let (core, pre) = match s.split_once('-') {
            Some((c, p)) => (c, p.split('.').map(str::to_string).collect::<Vec<_>>()),
            None => (s, Vec::new()),
        };
        let nums: Vec<u64> = core.split('.').map(|p| p.parse().ok()).collect::<Option<_>>()?;
        let [a, b, c] = nums[..] else { return None };
        if pre.iter().any(String::is_empty) {
            return None;
        }
        Some(Version { core: [a, b, c], pre })
    }

    pub fn is_prerelease(&self) -> bool {
        !self.pre.is_empty()
    }
}

impl Ord for Version {
    fn cmp(&self, o: &Self) -> Ordering {
        self.core.cmp(&o.core).then_with(|| match (self.pre.is_empty(), o.pre.is_empty()) {
            (true, true) => Ordering::Equal,
            (true, false) => Ordering::Greater,
            (false, true) => Ordering::Less,
            (false, false) => {
                for (a, b) in self.pre.iter().zip(&o.pre) {
                    let c = match (a.parse::<u64>(), b.parse::<u64>()) {
                        (Ok(x), Ok(y)) => x.cmp(&y),
                        (Ok(_), Err(_)) => Ordering::Less,
                        (Err(_), Ok(_)) => Ordering::Greater,
                        (Err(_), Err(_)) => a.cmp(b),
                    };
                    if c != Ordering::Equal {
                        return c;
                    }
                }
                self.pre.len().cmp(&o.pre.len())
            }
        })
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, o: &Self) -> Option<Ordering> {
        Some(self.cmp(o))
    }
}

/// Whether `latest` is newer than `current`. An unreadable version is never newer.
pub fn is_newer(latest: &str, current: &str) -> bool {
    matches!((Version::parse(latest), Version::parse(current)), (Some(l), Some(c)) if l > c)
}

// ------------------------------------------------------------------ releases

/// What the app needs from a release.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Release {
    /// "0.3.0" (the tag without its `v`).
    pub version: String,
    /// The release's web page, which lists the packages.
    pub page: String,
    /// The first lines of the notes, shortened.
    #[serde(default)]
    pub notes: Vec<String>,
    /// How many more items the notes list.
    #[serde(default)]
    pub more: usize,
}

#[derive(Deserialize)]
struct ApiRelease {
    tag_name: String,
    html_url: String,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    body: Option<String>,
}

/// The release notes' items as plain one-line text: the bullets of the changelog section (the part
/// before GitHub's generated "What's Changed" list), first `NOTE_LINES` of them shortened, and how
/// many more there are.
pub fn notes_summary(body: &str) -> (Vec<String>, usize) {
    let mut items: Vec<String> = Vec::new();
    let mut open = false;
    for line in body.lines() {
        let t = line.trim_end();
        if t.starts_with("## ") {
            break;
        }
        if let Some(rest) = t.strip_prefix("- ").or_else(|| t.strip_prefix("* ")) {
            items.push(rest.trim().to_string());
            open = true;
        } else if open && t.starts_with(' ') && !t.trim().is_empty() {
            let last = items.last_mut().expect("open item");
            last.push(' ');
            last.push_str(t.trim());
        } else {
            open = false;
        }
    }
    let more = items.len().saturating_sub(NOTE_LINES);
    let lines = items.iter().take(NOTE_LINES).map(|i| shorten(&plain(i), NOTE_CHARS)).collect();
    (lines, more)
}

/// Markdown to plain text: links keep their text, emphasis and code marks go.
fn plain(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('[') {
        out.push_str(&rest[..i]);
        let after = &rest[i + 1..];
        match after.find("](").and_then(|j| after[j..].find(')').map(|k| (j, j + k))) {
            Some((j, end)) => {
                out.push_str(&after[..j]);
                rest = &after[end + 1..];
            }
            None => {
                out.push('[');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out.replace("**", "").replace('`', "")
}

/// The first sentence, or the first `max` characters ending in "…" at a word break.
fn shorten(s: &str, max: usize) -> String {
    let s = match s.find(". ") {
        Some(i) if i < max => &s[..=i],
        _ => s,
    };
    if s.chars().count() <= max {
        return s.to_string();
    }
    let cut: String = s.chars().take(max).collect();
    let cut = cut.rsplit_once(' ').map_or(cut.as_str(), |(a, _)| a).trim_end_matches([',', ';', ':']);
    format!("{cut}…")
}

/// Reads GitHub's release JSON. `Ok(None)`: nothing to offer (a draft or a pre-release, which the
/// API leaves out but a test server might not).
pub fn parse_release(json: &str) -> Result<Option<Release>, String> {
    let r: ApiRelease = serde_json::from_str(json).map_err(|e| format!("Unexpected answer from GitHub ({e})."))?;
    let version = r.tag_name.trim().trim_start_matches('v').to_string();
    let Some(v) = Version::parse(&version) else {
        return Err(format!("The latest release has an unreadable version ({}).", r.tag_name));
    };
    if r.draft || r.prerelease || v.is_prerelease() {
        return Ok(None);
    }
    if !r.html_url.starts_with("https://") {
        return Err("Unexpected answer from GitHub (no release page).".into());
    }
    let (notes, more) = notes_summary(r.body.as_deref().unwrap_or(""));
    Ok(Some(Release { version, page: r.html_url, notes, more }))
}

const OFFLINE: &str = "Couldn't reach GitHub. Check your internet connection and try again.";

/// Only HTTPS (plain HTTP to this machine for tests).
fn allowed(url: &str) -> bool {
    url.starts_with("https://") || url.starts_with("http://127.0.0.1:")
}

/// Minutes until GitHub accepts requests again, from `retry-after` (seconds) or
/// `x-ratelimit-reset` (Unix time), rounded up.
fn wait_minutes(retry_after: Option<&str>, reset: Option<&str>, now: u64) -> Option<u64> {
    let secs = retry_after
        .and_then(|s| s.trim().parse::<u64>().ok())
        .or_else(|| reset.and_then(|s| s.trim().parse::<u64>().ok()).map(|t| t.saturating_sub(now)))?;
    Some(secs.div_ceil(60).max(1))
}

/// Asks for the latest release. `Ok(None)`: no published full release yet.
pub fn fetch_latest(url: &str) -> Result<Option<Release>, String> {
    if !allowed(url) {
        return Err("The update address must use HTTPS.".into());
    }
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .http_status_as_error(false)
        .timeout_connect(Some(Duration::from_secs(10)))
        .timeout_global(Some(Duration::from_secs(20)))
        .user_agent(format!("Device/{CURRENT}"))
        .build()
        .into();
    let mut resp = agent
        .get(url)
        .header("accept", "application/vnd.github+json")
        .header("x-github-api-version", "2022-11-28")
        .call()
        .map_err(|_| OFFLINE.to_string())?;
    let header = |n: &str| resp.headers().get(n).and_then(|v| v.to_str().ok()).map(str::to_string);
    let status = resp.status().as_u16();
    let limited = status == 429 || (status == 403 && header("x-ratelimit-remaining").as_deref() == Some("0"));
    if limited {
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
        return Err(match wait_minutes(header("retry-after").as_deref(), header("x-ratelimit-reset").as_deref(), now) {
            Some(1) => "GitHub is limiting update checks from this network. Try again in a minute.".into(),
            Some(m) => format!("GitHub is limiting update checks from this network. Try again in about {m} minutes."),
            None => "GitHub is limiting update checks from this network. Try again later.".into(),
        });
    }
    match status {
        200 => {}
        404 => return Ok(None),
        s => return Err(format!("GitHub couldn't answer (HTTP {s}). Please try again later.")),
    }
    let text = resp.body_mut().with_config().limit(4 << 20).read_to_string().map_err(|_| OFFLINE.to_string())?;
    parse_release(&text)
}

// ------------------------------------------------------------------ the saved answer

/// The last successful check, so launches within `LAUNCH_INTERVAL_SECS` don't ask GitHub again.
#[derive(Debug, Default, PartialEq, Serialize, Deserialize)]
struct Saved {
    checked_at: u64,
    latest: Option<Release>,
}

/// `update-check.json` next to the preferences.
pub fn saved_path() -> Option<PathBuf> {
    Some(crate::prefs::support_dir()?.join("update-check.json"))
}

/// One check, on a worker thread. A launch check (`manual` false) answers from `saved` when the last
/// successful check is less than `LAUNCH_INTERVAL_SECS` old; a successful request is saved there.
pub fn check(url: &str, saved: Option<&Path>, manual: bool, now: u64) -> Result<Option<Release>, String> {
    if !manual
        && let Some(s) = saved.and_then(|p| std::fs::read_to_string(p).ok()).and_then(|t| serde_json::from_str::<Saved>(&t).ok())
        && s.checked_at <= now
        && now - s.checked_at < LAUNCH_INTERVAL_SECS
    {
        return Ok(s.latest);
    }
    let latest = fetch_latest(url)?;
    if let Some(p) = saved {
        let s = Saved { checked_at: now, latest: latest.clone() };
        if let (Some(dir), Ok(text)) = (p.parent(), serde_json::to_string_pretty(&s)) {
            let _ = std::fs::create_dir_all(dir);
            let _ = std::fs::write(p, text);
        }
    }
    Ok(latest)
}

/// Opens a web page in the default browser.
pub fn open_page(url: &str) -> Result<(), String> {
    if !url.starts_with("https://") {
        return Err("not a web address".into());
    }
    let mut cmd = if cfg!(target_os = "macos") {
        std::process::Command::new("open")
    } else if cfg!(windows) {
        std::process::Command::new("explorer")
    } else {
        std::process::Command::new("xdg-open")
    };
    cmd.arg(url).spawn().map(|_| ()).map_err(|e| e.to_string())
}

// ------------------------------------------------------------------ the controller's side

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum State {
    #[default]
    Idle,
    /// A check from the menu is running (a launch check shows nothing).
    Checking,
    UpToDate,
    Available(Release),
    /// A check from the menu failed: why, in plain words.
    Failed(String),
}

#[derive(Default)]
pub struct Updates {
    pub state: State,
    /// A check is on a worker thread.
    running: bool,
    /// Where the last answer is saved; None: never (self-test, review states).
    pub saved_path: Option<PathBuf>,
    /// Self-tests and review states: a check answers this at once instead of asking the network,
    /// and Download records the page in `opened` instead of opening a browser.
    pub check_override: Option<Result<Option<Release>, String>>,
    pub opened: Option<String>,
}

/// A made-up newer release for self-tests and review states.
pub fn sample_release() -> Release {
    let v = Version::parse(CURRENT).map_or_else(|| "99.0.0".into(), |v| format!("{}.{}.0", v.core[0], v.core[1] + 1));
    Release {
        page: format!("https://github.com/devicedaw/device/releases/tag/v{v}"),
        version: v,
        notes: vec![
            "Options › Check for Updates… looks for a newer release, and the title bar shows when there is one.".into(),
            "Tempo changes: a tempo lane on the master track, with steps and ramps.".into(),
            "Bounce Track in Place prints a whole track to audio where it is.".into(),
        ],
        more: 12,
    }
}

impl Controller {
    /// Checks for a newer release. `manual` (Options › Check for Updates…) opens the dialog and
    /// shows the result; the launch check runs only when Settings › General allows it and shows
    /// only the pill.
    pub fn updates_check(&mut self, manual: bool) {
        if manual {
            self.updates_open = true;
            self.ui_dirty = true;
        } else if !self.settings.prefs.general.check_updates_at_launch {
            return;
        }
        // A known update is not asked for again.
        if matches!(self.updates.state, State::Available(_)) {
            return;
        }
        if manual {
            self.updates.state = State::Checking;
        }
        if self.updates.running {
            return; // the running check answers the menu too
        }
        if let Some(result) = self.updates.check_override.clone() {
            self.finish_update_check(result);
            return;
        }
        let Some(url) = latest_url() else {
            if manual {
                self.updates.state = State::Failed("Update checks are turned off on this computer (DSTUDIO_UPDATE_URL=off).".into());
            }
            return;
        };
        let saved = self.updates.saved_path.clone();
        let tx = self.jobs_tx.clone();
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
        self.pending_jobs += 1;
        self.updates.running = true;
        let spawned = std::thread::Builder::new().name("update-check".into()).spawn(move || {
            let _ = tx.send(JobResult::UpdateChecked(check(&url, saved.as_deref(), manual, now)));
        });
        if spawned.is_err() {
            self.pending_jobs -= 1;
            self.updates.running = false;
            if manual {
                self.updates.state = State::Failed(OFFLINE.into());
            }
        }
    }

    /// A check finished. Shown in full only when the dialog asked (state Checking); a launch check
    /// shows just the pill.
    pub fn finish_update_check(&mut self, result: Result<Option<Release>, String>) {
        self.updates.running = false;
        let manual = self.updates.state == State::Checking;
        match result {
            Ok(Some(r)) if is_newer(&r.version, CURRENT) => self.updates.state = State::Available(r),
            Ok(_) if manual => self.updates.state = State::UpToDate,
            Err(e) if manual => self.updates.state = State::Failed(e),
            _ => {}
        }
        self.ui_dirty = true;
    }

    /// The title-bar pill: the dialog with the update.
    pub fn update_pill_clicked(&mut self) {
        if !self.modal_open() && matches!(self.updates.state, State::Available(_)) {
            self.updates_open = true;
            self.ui_dirty = true;
        }
    }

    /// The dialog's Download button: the release page in the browser.
    pub fn updates_download(&mut self) {
        let State::Available(r) = self.updates.state.clone() else { return };
        self.updates_open = false;
        self.ui_dirty = true;
        let opened = if self.updates.check_override.is_some() {
            self.updates.opened = Some(r.page.clone());
            Ok(())
        } else {
            open_page(&r.page)
        };
        match opened {
            Ok(()) => self.set_status(format!("Opened the download page for Device {}", r.version), 4),
            Err(e) => self.set_status(format!("Couldn't open {}: {e}", r.page), 6),
        }
    }

    pub(crate) fn push_updates(&self, s: &crate::Studio) {
        let (text, notes, more, download) = match &self.updates.state {
            State::Idle | State::Checking => ("Checking for updates…".to_string(), Vec::new(), String::new(), false),
            State::UpToDate => (format!("You're running Device v{CURRENT}, the latest version."), Vec::new(), String::new(), false),
            State::Failed(e) => (e.clone(), Vec::new(), String::new(), false),
            State::Available(r) => (
                format!("Device {} is available. You're running v{CURRENT}.", r.version),
                r.notes.clone(),
                match r.more {
                    0 => String::new(),
                    1 => "and 1 more change".into(),
                    n => format!("and {n} more changes"),
                },
                true,
            ),
        };
        s.set_updates_text(text.into());
        let notes: Vec<slint::SharedString> = notes.into_iter().map(Into::into).collect();
        s.set_updates_notes(slint::ModelRc::new(slint::VecModel::from(notes)));
        s.set_updates_more(more.into());
        s.set_updates_download(download);
        let version = match &self.updates.state {
            State::Available(r) => r.version.as_str(),
            _ => "",
        };
        s.set_update_version(version.into());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::{Arc, Mutex};

    #[test]
    fn versions_compare_like_semver() {
        assert!(is_newer("0.3.0", "0.2.0"));
        assert!(is_newer("v0.2.1", "0.2.0"));
        assert!(is_newer("0.10.0", "0.9.0"));
        assert!(is_newer("1.0.0", "0.99.99"));
        assert!(!is_newer("0.2.0", "0.2.0"));
        assert!(!is_newer("0.1.9", "0.2.0"));
        // A release beats its pre-releases; pre-releases order by their numbers.
        assert!(is_newer("0.3.0", "0.3.0-rc.1"));
        assert!(is_newer("0.3.0-rc.1", "0.3.0-beta.2"));
        assert!(is_newer("0.3.0-beta.10", "0.3.0-beta.9"));
        assert!(!is_newer("0.3.0-alpha.1", "0.3.0"));
        assert!(is_newer("0.2.0", "0.2.0-dev.1a2b3c4"));
        assert!(is_newer("0.2.1+build.5", "0.2.0"));
        assert!(!is_newer("nonsense", "0.2.0"));
        assert!(!is_newer("0.3", "0.2.0"));
        assert!(!is_newer("0.3.0-", "0.2.0"));
    }

    const NOTES: &str = "### Added\r\n\r\n- Linux: Device now plays without PipeWire. When no sound server runs, it goes straight\r\n  through ALSA.\r\n- **Chains** can be saved; see [the guide](https://example.com/g) and `Chains ›`.\r\n\r\n### Fixed\r\n\r\n- A fix whose text is long enough that it has to be shortened for the dialog because it goes on and on without a full stop anywhere\r\n- Another fix.\r\n\r\n\r\n## What's Changed\r\n* A pull request by someone in https://github.com/devicedaw/device/pull/1\r\n";

    fn release_json(tag: &str, prerelease: bool) -> String {
        serde_json::json!({
            "tag_name": tag,
            "html_url": format!("https://github.com/devicedaw/device/releases/tag/{tag}"),
            "draft": false,
            "prerelease": prerelease,
            "body": NOTES,
        })
        .to_string()
    }

    #[test]
    fn notes_are_summarised_from_the_changelog_section() {
        let (lines, more) = notes_summary(NOTES);
        assert_eq!(lines[0], "Linux: Device now plays without PipeWire.");
        assert_eq!(lines[1], "Chains can be saved; see the guide and Chains ›.");
        assert!(lines[2].ends_with('…') && lines[2].chars().count() <= NOTE_CHARS + 1, "{}", lines[2]);
        assert_eq!(more, 1, "GitHub's pull request list is not counted");
        assert_eq!(notes_summary(""), (Vec::new(), 0));
    }

    #[test]
    fn releases_and_prereleases() {
        let r = parse_release(&release_json("v0.3.0", false)).unwrap().unwrap();
        assert_eq!(r.version, "0.3.0");
        assert_eq!(r.page, "https://github.com/devicedaw/device/releases/tag/v0.3.0");
        assert_eq!((r.notes.len(), r.more), (3, 1));
        // Pre-releases are never offered, flagged or only tagged as one.
        assert_eq!(parse_release(&release_json("v0.3.0", true)), Ok(None));
        assert_eq!(parse_release(&release_json("v0.3.0-rc.1", false)), Ok(None));
        assert!(parse_release(&release_json("latest", false)).is_err());
        assert!(parse_release("{}").is_err());
    }

    /// Serves one canned answer per request on 127.0.0.1 and keeps each request's head.
    fn serve(answers: Vec<(&'static str, Vec<(&'static str, String)>, String)>) -> (String, Arc<Mutex<Vec<String>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = seen.clone();
        std::thread::spawn(move || {
            for (status, headers, body) in answers {
                let Ok((mut s, _)) = listener.accept() else { return };
                let mut req = Vec::new();
                let mut buf = [0u8; 4096];
                while !String::from_utf8_lossy(&req).contains("\r\n\r\n") {
                    let k = s.read(&mut buf).unwrap();
                    if k == 0 {
                        break;
                    }
                    req.extend_from_slice(&buf[..k]);
                }
                log.lock().unwrap().push(String::from_utf8_lossy(&req).to_string());
                let mut head = format!("HTTP/1.1 {status}\r\ncontent-length: {}\r\nconnection: close\r\n", body.len());
                for (k, v) in headers {
                    head.push_str(&format!("{k}: {v}\r\n"));
                }
                head.push_str("\r\n");
                s.write_all(head.as_bytes()).unwrap();
                s.write_all(body.as_bytes()).unwrap();
            }
        });
        (base, seen)
    }

    #[test]
    fn fetch_reads_the_release_and_reports_failures_plainly() {
        let (base, seen) = serve(vec![
            ("200 OK", vec![], release_json("v9.0.0", false)),
            ("404 Not Found", vec![], String::new()),
            ("403 Forbidden", vec![("x-ratelimit-remaining", "0".into()), ("retry-after", "600".into())], String::new()),
            ("429 Too Many Requests", vec![], String::new()),
            ("500 Internal Server Error", vec![], String::new()),
        ]);
        let url = format!("{base}/latest");
        let r = fetch_latest(&url).unwrap().unwrap();
        assert_eq!(r.version, "9.0.0");
        assert!(is_newer(&r.version, CURRENT));
        // What GitHub sees: a generic agent with the version, the API headers, nothing else.
        let head = seen.lock().unwrap()[0].to_ascii_lowercase();
        assert!(head.contains(&format!("user-agent: device/{CURRENT}\r\n")), "{head}");
        assert!(head.contains("accept: application/vnd.github+json"), "{head}");
        let names: Vec<&str> = head.lines().skip(1).filter_map(|l| l.split_once(':').map(|(n, _)| n.trim())).collect();
        for n in &names {
            assert!(["host", "user-agent", "accept", "accept-encoding", "x-github-api-version"].contains(n), "unexpected header {n}");
        }
        // No published release yet: nothing to offer, no error.
        assert_eq!(fetch_latest(&url), Ok(None));
        assert!(fetch_latest(&url).unwrap_err().contains("about 10 minutes"));
        assert!(fetch_latest(&url).unwrap_err().contains("limiting update checks"));
        assert!(fetch_latest(&url).unwrap_err().contains("HTTP 500"));
        // Off-line: a plain-language error.
        let dead = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap();
        assert!(fetch_latest(&format!("http://{dead}/latest")).unwrap_err().contains("internet connection"));
        assert!(fetch_latest("http://example.com/latest").is_err(), "plain HTTP is refused");
    }

    #[test]
    fn rate_limit_wait_is_rounded_up() {
        assert_eq!(wait_minutes(Some("61"), None, 0), Some(2));
        assert_eq!(wait_minutes(None, Some("1030"), 1000), Some(1));
        assert_eq!(wait_minutes(None, None, 0), None);
    }

    #[test]
    fn launch_checks_reuse_the_last_answer_for_12_hours() {
        let dir = std::env::temp_dir().join(format!("dstudio-update-saved-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let saved = dir.join("update-check.json");
        let (base, seen) = serve(vec![
            ("200 OK", vec![], release_json("v9.0.0", false)),
            ("200 OK", vec![], release_json("v9.1.0", false)),
            ("200 OK", vec![], release_json("v9.2.0", false)),
        ]);
        let url = format!("{base}/latest");
        let t = 1_800_000_000;
        let v = |r: Result<Option<Release>, String>| r.unwrap().unwrap().version;
        assert_eq!(v(check(&url, Some(&saved), false, t)), "9.0.0");
        // An hour later: answered from the file, no request.
        assert_eq!(v(check(&url, Some(&saved), false, t + 3600)), "9.0.0");
        assert_eq!(seen.lock().unwrap().len(), 1);
        // The menu always asks.
        assert_eq!(v(check(&url, Some(&saved), true, t + 3600)), "9.1.0");
        // 12 hours after the last request: asks again.
        assert_eq!(v(check(&url, Some(&saved), false, t + 3600 + LAUNCH_INTERVAL_SECS)), "9.2.0");
        assert_eq!(seen.lock().unwrap().len(), 3);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
