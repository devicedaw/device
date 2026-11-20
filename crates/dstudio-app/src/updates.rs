//! Updates: Options › Check for Updates… and the "Update available" pill in the title bar.
//!
//! Once at launch (quietly: nothing is shown when off-line) and from the menu, the app asks GitHub
//! for the latest published release and compares its version with the running one. When a newer
//! one exists, a blue "Update available" pill appears left of Report a bug. Clicking it downloads
//! this platform's package on a worker thread (the pill fills up and shows the percentage), checks
//! it against the release's SHA256SUMS.txt (and on macOS the app's code signature), and stages it.
//! The pill then reads "Restart to update": the app quits (asking to save first), a small helper
//! waits for it to exit, puts the new version in place and starts it.
//!
//! Where the app can't replace itself (a development build, the portable Windows .zip, a folder
//! the user can't write to), the pill and the dialog open the release's page instead.
//!
//! Nothing personal is sent: the requests carry the app's version in the User-Agent and nothing else.

use std::cmp::Ordering;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};
use std::time::Duration;

use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::controller::{Controller, JobResult};

/// The newest published release (GitHub leaves out drafts and pre-releases).
/// `DSTUDIO_UPDATE_URL` overrides it (tests and local runs); `DSTUDIO_UPDATE_URL=off` turns the
/// launch check off.
pub const LATEST_URL: &str = "https://api.github.com/repos/devicedaw/device/releases/latest";
const SUMS_NAME: &str = "SHA256SUMS.txt";
const CURRENT: &str = env!("CARGO_PKG_VERSION");

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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Asset {
    pub name: String,
    pub url: String,
    pub size: u64,
}

/// What the app needs from a release.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Release {
    /// "0.3.0" (the tag without its `v`).
    pub version: String,
    /// The release's web page (the fallback when the app can't install it itself).
    pub page: String,
    /// This platform's package, when the release has one.
    pub package: Option<Asset>,
    /// SHA256SUMS.txt.
    pub sums: Option<Asset>,
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
    assets: Vec<ApiAsset>,
}

#[derive(Deserialize)]
struct ApiAsset {
    name: String,
    browser_download_url: String,
    #[serde(default)]
    size: u64,
}

/// The release package this platform installs from (tools/release, the names without a version).
pub fn package_name(os: &str, arch: &str) -> Option<&'static str> {
    match (os, arch) {
        ("macos", _) => Some("Device-macos-universal.dmg"),
        ("windows", "x86_64") => Some("Device-windows-x64-setup.exe"),
        ("linux", "x86_64") => Some("Device-linux-x86_64.tar.gz"),
        ("linux", "aarch64") => Some("Device-linux-aarch64.tar.gz"),
        _ => None,
    }
}

/// Reads GitHub's release JSON. Drafts and pre-releases are refused (the API leaves them out, a
/// test server might not).
pub fn parse_release(json: &str, os: &str, arch: &str) -> Result<Release, String> {
    let r: ApiRelease = serde_json::from_str(json).map_err(|e| format!("Unexpected answer from the update server ({e})."))?;
    if r.draft || r.prerelease {
        return Err("The latest release is not published yet.".into());
    }
    let version = r.tag_name.trim().trim_start_matches('v').to_string();
    if Version::parse(&version).is_none() {
        return Err(format!("The latest release has an unreadable version ({}).", r.tag_name));
    }
    let find = |name: &str| {
        r.assets.iter().find(|a| a.name == name).map(|a| Asset { name: a.name.clone(), url: a.browser_download_url.clone(), size: a.size })
    };
    Ok(Release { version, page: r.html_url.clone(), package: package_name(os, arch).and_then(find), sums: find(SUMS_NAME) })
}

fn agent(timeout: Duration) -> ureq::Agent {
    ureq::Agent::config_builder()
        .http_status_as_error(false)
        .timeout_connect(Some(Duration::from_secs(10)))
        .timeout_global(Some(timeout))
        .user_agent(format!("Device/{CURRENT}"))
        .build()
        .into()
}

const OFFLINE: &str = "Couldn't reach the update server. Check your internet connection and try again.";

/// Downloads only over HTTPS (plain HTTP to this machine for tests).
fn allowed(url: &str) -> bool {
    url.starts_with("https://") || url.starts_with("http://127.0.0.1:")
}

/// Asks for the latest release. `Ok(None)`: the repository has no published release yet.
pub fn fetch_latest(url: &str) -> Result<Option<Release>, String> {
    if !allowed(url) {
        return Err("The update address must use HTTPS.".into());
    }
    let mut resp = agent(Duration::from_secs(20))
        .get(url)
        .header("accept", "application/vnd.github+json")
        .call()
        .map_err(|_| OFFLINE.to_string())?;
    match resp.status().as_u16() {
        200 => {}
        404 => return Ok(None),
        s => return Err(format!("The update server couldn't answer (HTTP {s}). Please try again later.")),
    }
    let text = resp.body_mut().with_config().limit(4 << 20).read_to_string().map_err(|_| OFFLINE.to_string())?;
    parse_release(&text, std::env::consts::OS, std::env::consts::ARCH).map(Some)
}

/// Bytes received and expected (0: not known yet) of a download, read by the UI every frame.
#[derive(Default)]
pub struct Progress {
    pub received: AtomicU64,
    pub total: AtomicU64,
}

impl Progress {
    /// 0..1, or 0 while the size is not known.
    pub fn fraction(&self) -> f32 {
        let total = self.total.load(AtomicOrdering::Relaxed);
        if total == 0 { 0.0 } else { (self.received.load(AtomicOrdering::Relaxed) as f64 / total as f64).clamp(0.0, 1.0) as f32 }
    }
}

/// Streams `url` into `dest`, counting bytes into `progress`. Returns the file's SHA-256 (hex).
pub fn download(url: &str, dest: &Path, progress: &Progress) -> Result<String, String> {
    if !allowed(url) {
        return Err("The download address must use HTTPS.".into());
    }
    let mut resp = agent(Duration::from_secs(60 * 30)).get(url).call().map_err(|_| OFFLINE.to_string())?;
    let status = resp.status().as_u16();
    if status != 200 {
        return Err(format!("The download failed (HTTP {status}). Please try again later."));
    }
    let total = resp.body().content_length().unwrap_or(0);
    progress.total.store(total, AtomicOrdering::Relaxed);
    progress.received.store(0, AtomicOrdering::Relaxed);
    let mut file = std::fs::File::create(dest).map_err(|e| format!("Couldn't save the update: {e}"))?;
    let mut reader = resp.body_mut().with_config().limit(2 << 30).reader();
    let mut hash = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = reader.read(&mut buf).map_err(|_| "The download was interrupted. Please try again.".to_string())?;
        if n == 0 {
            break;
        }
        hash.update(&buf[..n]);
        file.write_all(&buf[..n]).map_err(|e| format!("Couldn't save the update: {e}"))?;
        progress.received.fetch_add(n as u64, AtomicOrdering::Relaxed);
    }
    file.sync_all().map_err(|e| format!("Couldn't save the update: {e}"))?;
    let received = progress.received.load(AtomicOrdering::Relaxed);
    if total > 0 && received != total {
        return Err("The download was interrupted. Please try again.".into());
    }
    Ok(hash.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

/// The SHA-256 that `sha256sum` output (SHA256SUMS.txt) lists for `name`.
pub fn expected_sum(sums: &str, name: &str) -> Option<String> {
    sums.lines().find_map(|l| {
        let (hash, file) = l.trim().split_once(char::is_whitespace)?;
        let file = file.trim_start().trim_start_matches('*');
        (file == name && hash.len() == 64 && hash.chars().all(|c| c.is_ascii_hexdigit())).then(|| hash.to_ascii_lowercase())
    })
}

fn fetch_text(url: &str) -> Result<String, String> {
    if !allowed(url) {
        return Err("The download address must use HTTPS.".into());
    }
    let mut resp = agent(Duration::from_secs(60)).get(url).call().map_err(|_| OFFLINE.to_string())?;
    if resp.status().as_u16() != 200 {
        return Err(format!("The checksum list couldn't be downloaded (HTTP {}).", resp.status().as_u16()));
    }
    resp.body_mut().with_config().limit(1 << 20).read_to_string().map_err(|_| OFFLINE.to_string())
}

// ------------------------------------------------------------------ installing

/// How this copy of the app can be replaced.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    /// macOS: the `.app` bundle the app runs from.
    MacApp { bundle: PathBuf },
    /// Windows, installed with the Inno Setup installer: the installed exe, and whether the
    /// install is for all users (not under the user's local app data).
    WindowsInstaller { exe: PathBuf, all_users: bool },
    /// Linux, installed with the package's install.sh: its prefix (`<prefix>/lib/device-studio`).
    LinuxPrefix { prefix: PathBuf },
}

/// Where the running app is installed, from the path of its executable, if it is a layout the
/// release packages make. `local_app_data` is `%LOCALAPPDATA%` on Windows.
pub fn target_for(os: &str, exe: &Path, local_app_data: Option<&Path>) -> Option<Target> {
    match os {
        "macos" => {
            // <bundle>.app/Contents/MacOS/device-studio
            let macos = exe.parent()?;
            let contents = macos.parent()?;
            let bundle = contents.parent()?;
            let ok = macos.file_name()? == "MacOS"
                && contents.file_name()? == "Contents"
                && bundle.extension().is_some_and(|e| e == "app");
            ok.then(|| Target::MacApp { bundle: bundle.to_path_buf() })
        }
        "windows" => {
            let dir = exe.parent()?;
            // The portable .zip has no uninstaller; only an installed copy is updated in place.
            if !dir.join("unins000.exe").is_file() {
                return None;
            }
            let all_users = !local_app_data.is_some_and(|l| dir.starts_with(l));
            Some(Target::WindowsInstaller { exe: exe.to_path_buf(), all_users })
        }
        "linux" => {
            // <prefix>/lib/device-studio/device-studio
            let lib_dir = exe.parent()?;
            let lib = lib_dir.parent()?;
            let ok = lib_dir.file_name()? == "device-studio" && lib.file_name()? == "lib";
            let prefix = lib.parent()?.to_path_buf();
            ok.then_some(Target::LinuxPrefix { prefix })
        }
        _ => None,
    }
}

fn writable(dir: &Path) -> bool {
    let probe = dir.join(format!(".dstudio-update-probe-{}", std::process::id()));
    let ok = std::fs::File::create(&probe).is_ok();
    let _ = std::fs::remove_file(&probe);
    ok
}

/// This copy's install target, if the app can update it in place.
pub fn install_target() -> Option<Target> {
    let exe = std::env::current_exe().ok()?.canonicalize().ok()?;
    let local = std::env::var_os("LOCALAPPDATA").map(PathBuf::from);
    let t = target_for(std::env::consts::OS, &exe, local.as_deref())?;
    let ok = match &t {
        Target::MacApp { bundle } => bundle.parent().is_some_and(writable),
        Target::WindowsInstaller { .. } => true, // an all-users install asks for permission
        Target::LinuxPrefix { prefix } => writable(&prefix.join("lib/device-studio")),
    };
    ok.then_some(t)
}

fn run(cmd: &str, args: &[&std::ffi::OsStr]) -> Result<String, String> {
    let out = std::process::Command::new(cmd).args(args).output().map_err(|e| format!("{cmd}: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr))
    } else {
        Err(format!("{cmd} failed: {}", String::from_utf8_lossy(&out.stderr).trim()))
    }
}

/// The signing team of a macOS app ("" when it is signed ad hoc).
fn mac_team(app: &Path) -> String {
    run("codesign", &["-dv".as_ref(), "--verbose=2".as_ref(), app.as_os_str()])
        .ok()
        .and_then(|t| t.lines().find_map(|l| l.strip_prefix("TeamIdentifier=").map(str::to_string)))
        .filter(|t| t != "not set")
        .unwrap_or_default()
}

/// Gets a downloaded package ready to install, on the worker thread. Returns what `restart`
/// puts in place: the new `.app` next to the old one (macOS), the installer (Windows), or the
/// unpacked folder with install.sh (Linux).
pub fn stage(target: &Target, package: &Path, work: &Path) -> Result<PathBuf, String> {
    match target {
        Target::MacApp { bundle } => {
            let mount = work.join("mount");
            let _ = std::fs::create_dir_all(&mount);
            run(
                "hdiutil",
                &["attach".as_ref(), "-nobrowse".as_ref(), "-readonly".as_ref(), "-noautoopen".as_ref(), "-mountpoint".as_ref(), mount.as_os_str(), package.as_os_str()],
            )?;
            let result = (|| {
                let app = mount.join("Device.app");
                if !app.is_dir() {
                    return Err("The update package has no Device.app.".to_string());
                }
                run("codesign", &["--verify".as_ref(), "--deep".as_ref(), "--strict".as_ref(), app.as_os_str()])
                    .map_err(|_| "The update's signature is not valid, so it was not installed.".to_string())?;
                let ours = mac_team(bundle);
                if !ours.is_empty() && mac_team(&app) != ours {
                    return Err("The update is signed by someone else, so it was not installed.".into());
                }
                let parent = bundle.parent().ok_or("No folder to install into.")?;
                let staged = parent.join(".Device-update.app");
                let _ = std::fs::remove_dir_all(&staged);
                run("ditto", &[app.as_os_str(), staged.as_os_str()])?;
                Ok(staged)
            })();
            let _ = run("hdiutil", &["detach".as_ref(), "-quiet".as_ref(), mount.as_os_str()]);
            result
        }
        Target::WindowsInstaller { .. } => Ok(package.to_path_buf()),
        Target::LinuxPrefix { .. } => {
            let dir = work.join("unpacked");
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
            run("tar", &["-xzf".as_ref(), package.as_os_str(), "-C".as_ref(), dir.as_os_str()])?;
            std::fs::read_dir(&dir)
                .map_err(|e| e.to_string())?
                .flatten()
                .map(|e| e.path())
                .find(|p| p.join("install.sh").is_file())
                .ok_or_else(|| "The update package has no install.sh.".to_string())
        }
    }
}

/// macOS: wait for the app to quit, swap the bundles (putting the old one back if that fails),
/// start the new one. Arguments: pid, bundle, staged bundle.
const MAC_HELPER: &str = r#"pid=$1; app=$2; new=$3
while kill -0 "$pid" 2>/dev/null; do sleep 0.2; done
old="$app.previous"
rm -rf "$old"
if mv "$app" "$old"; then
  if mv "$new" "$app"; then rm -rf "$old"; else mv "$old" "$app"; fi
fi
open "$app""#;

/// Linux: wait for the app to quit, run the new package's install.sh into the same prefix, start
/// it. Arguments: pid, unpacked folder, prefix.
const LINUX_HELPER: &str = r#"pid=$1; dir=$2; prefix=$3
while kill -0 "$pid" 2>/dev/null; do sleep 0.2; done
"$dir/install.sh" "$prefix" >/dev/null 2>&1
rm -rf "$dir"
exec "$prefix/lib/device-studio/device-studio" >/dev/null 2>&1"#;

/// A PowerShell single-quoted string.
pub fn ps_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

/// The helper that installs `staged` once process `pid` has exited, then starts the new version.
pub fn helper_command(target: &Target, staged: &Path, pid: u32) -> (String, Vec<String>) {
    let s = |p: &Path| p.to_string_lossy().into_owned();
    match target {
        Target::MacApp { bundle } => {
            ("/bin/sh".into(), vec!["-c".into(), MAC_HELPER.into(), "sh".into(), pid.to_string(), s(bundle), s(staged)])
        }
        Target::LinuxPrefix { prefix } => {
            ("/bin/sh".into(), vec!["-c".into(), LINUX_HELPER.into(), "sh".into(), pid.to_string(), s(staged), s(prefix)])
        }
        Target::WindowsInstaller { exe, all_users } => {
            // Silent Inno Setup upgrade of the same AppId (it reuses the install folder); the
            // installer doesn't restart the app when silent, so the helper does.
            let scope = if *all_users { "/ALLUSERS" } else { "/CURRENTUSER" };
            let script = format!(
                "Wait-Process -Id {pid} -ErrorAction SilentlyContinue; \
                 Start-Process -FilePath {} -ArgumentList '/SILENT','/SP-','/SUPPRESSMSGBOXES','/NORESTART','{scope}' -Wait; \
                 Start-Process -FilePath {}",
                ps_quote(&s(staged)),
                ps_quote(&s(exe)),
            );
            (
                "powershell.exe".into(),
                vec!["-NoProfile".into(), "-NonInteractive".into(), "-ExecutionPolicy".into(), "Bypass".into(), "-Command".into(), script],
            )
        }
    }
}

fn spawn_helper(target: &Target, staged: &Path) -> Result<(), String> {
    let (prog, args) = helper_command(target, staged, std::process::id());
    let mut cmd = std::process::Command::new(prog);
    cmd.args(args).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd.spawn().map(|_| ()).map_err(|e| format!("Couldn't start the installer: {e}"))
}

/// Opens a web page in the default browser.
pub fn open_page(url: &str) -> Result<(), String> {
    let mut cmd = match std::env::consts::OS {
        "macos" => std::process::Command::new("open"),
        "windows" => {
            let mut c = std::process::Command::new("explorer");
            c.arg(url);
            return c.spawn().map(|_| ()).map_err(|e| e.to_string());
        }
        _ => std::process::Command::new("xdg-open"),
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
    Downloading(Release),
    /// Downloaded, checked and staged; "Restart to update".
    Ready(Release, PathBuf),
}

#[derive(Default)]
pub struct Updates {
    pub state: State,
    /// Shown in the dialog (a failed check from the menu, a failed download).
    pub error: String,
    pub progress: Arc<Progress>,
    /// How the running copy can be replaced; `None`: updates open the release page.
    pub target: Option<Target>,
    pub(crate) target_known: bool,
    /// Restart waits for the unsaved-changes prompt.
    pub restart_pending: bool,
    /// The pill's last percentage, so the UI is refreshed only when it changes.
    shown_percent: Option<u32>,
    /// Test hook: the helper is not started and the app doesn't quit; set to the staged path.
    pub dry_run_restart: Option<PathBuf>,
    /// Self-tests and review states: a check answers this at once instead of asking the network.
    pub check_override: Option<Result<Option<Release>, String>>,
}

/// A made-up newer release for self-tests and review states (nothing is downloaded from it).
pub fn sample_release() -> Release {
    let v = Version::parse(CURRENT).map(|v| format!("{}.{}.0", v.core[0], v.core[1] + 1)).unwrap_or_else(|| "99.0.0".into());
    Release { page: format!("https://github.com/devicedaw/device/releases/tag/v{v}"), version: v, package: None, sums: None }
}

impl Updates {
    pub fn release(&self) -> Option<&Release> {
        match &self.state {
            State::Available(r) | State::Downloading(r) | State::Ready(r, _) => Some(r),
            _ => None,
        }
    }
}

impl Controller {
    fn update_target(&mut self) -> Option<Target> {
        if !self.updates.target_known {
            self.updates.target = install_target();
            self.updates.target_known = true;
        }
        self.updates.target.clone()
    }

    /// Checks for a newer release on a worker thread. `manual` (Options › Check for Updates…)
    /// opens the dialog and shows the result; the launch check shows only the pill.
    pub fn updates_check(&mut self, manual: bool) {
        if manual {
            self.updates_open = true;
            self.updates.error.clear();
            self.ui_dirty = true;
        }
        // A known update is not checked again.
        if matches!(self.updates.state, State::Available(_) | State::Downloading(_) | State::Ready(..) | State::Checking) {
            return;
        }
        if let Some(result) = self.updates.check_override.clone() {
            self.finish_update_check(manual, result);
            return;
        }
        let Some(url) = latest_url() else {
            if manual {
                self.updates.error = "Update checks are turned off (DSTUDIO_UPDATE_URL=off).".into();
            }
            return;
        };
        if manual {
            self.updates.state = State::Checking;
        }
        let tx = self.jobs_tx.clone();
        self.pending_jobs += 1;
        let spawned = std::thread::Builder::new().name("update-check".into()).spawn(move || {
            let _ = tx.send(JobResult::UpdateChecked { manual, result: fetch_latest(&url) });
        });
        if spawned.is_err() {
            self.pending_jobs -= 1;
            self.updates.state = State::Idle;
        }
    }

    pub fn finish_update_check(&mut self, manual: bool, result: Result<Option<Release>, String>) {
        match result {
            Ok(Some(r)) if is_newer(&r.version, CURRENT) => self.updates.state = State::Available(r),
            Ok(_) => {
                if manual || self.updates.state == State::Checking {
                    self.updates.state = State::UpToDate;
                }
            }
            Err(e) => {
                if self.updates.state == State::Checking {
                    self.updates.state = State::Idle;
                }
                if manual {
                    self.updates.error = e;
                }
            }
        }
        self.ui_dirty = true;
    }

    /// The title-bar pill: download (or open the page), nothing while downloading, restart when ready.
    pub fn update_pill_clicked(&mut self) {
        if self.modal_open() {
            return;
        }
        match self.updates.state.clone() {
            State::Available(r) => self.update_download(r),
            State::Ready(..) => self.update_restart(),
            State::Downloading(_) => {
                self.updates_open = true;
                self.ui_dirty = true;
            }
            _ => {}
        }
    }

    /// The dialog's main button (Update / Download / Restart / OK).
    pub fn updates_primary(&mut self) {
        match self.updates.state.clone() {
            State::Available(r) => {
                self.updates_open = false;
                self.update_download(r);
            }
            State::Ready(..) => {
                self.updates_open = false;
                self.update_restart();
            }
            _ => self.updates_open = false,
        }
        self.ui_dirty = true;
    }

    fn update_download(&mut self, r: Release) {
        let target = self.update_target();
        let (Some(target), Some(package), Some(sums)) = (target, r.package.clone(), r.sums.clone()) else {
            // Can't install in place: the page has the packages.
            match open_page(&r.page) {
                Ok(()) => self.set_status(format!("Opened the download page for Device {}", r.version), 4),
                Err(e) => self.set_status(format!("Couldn't open {}: {e}", r.page), 6),
            }
            return;
        };
        let progress = Arc::new(Progress::default());
        progress.total.store(package.size, AtomicOrdering::Relaxed);
        self.updates.progress = progress.clone();
        self.updates.shown_percent = None;
        self.updates.error.clear();
        self.updates.state = State::Downloading(r.clone());
        let tx = self.jobs_tx.clone();
        self.pending_jobs += 1;
        let work = std::env::temp_dir().join(format!("dstudio-update-{}", r.version));
        let spawned = std::thread::Builder::new().name("update-download".into()).spawn(move || {
            let result = (|| {
                let _ = std::fs::remove_dir_all(&work);
                std::fs::create_dir_all(&work).map_err(|e| format!("Couldn't save the update: {e}"))?;
                let file = work.join(&package.name);
                let got = download(&package.url, &file, &progress)?;
                let want = expected_sum(&fetch_text(&sums.url)?, &package.name)
                    .ok_or("The release doesn't list a checksum for this package, so it was not installed.")?;
                if got != want {
                    let _ = std::fs::remove_file(&file);
                    return Err("The download is damaged (its checksum doesn't match). Please try again.".to_string());
                }
                stage(&target, &file, &work)
            })();
            let _ = tx.send(JobResult::UpdateDownloaded(result));
        });
        if spawned.is_err() {
            self.pending_jobs -= 1;
            self.updates.state = State::Available(r);
        }
        self.ui_dirty = true;
    }

    pub fn finish_update_download(&mut self, result: Result<PathBuf, String>) {
        let Some(r) = self.updates.release().cloned() else { return };
        match result {
            Ok(staged) => {
                self.updates.state = State::Ready(r.clone(), staged);
                self.set_status(format!("Device {} is ready. Restart to update.", r.version), 6);
            }
            Err(e) => {
                self.updates.state = State::Available(r);
                self.set_status(format!("Update failed: {e}"), 8);
                self.updates.error = e;
            }
        }
        self.ui_dirty = true;
    }

    /// Restart to update: asks to save unsaved changes first.
    pub fn update_restart(&mut self) {
        if self.doc.is_dirty() {
            self.updates.restart_pending = true;
            self.unsaved_open = true;
            self.ui_dirty = true;
            return;
        }
        self.update_restart_now();
    }

    /// Starts the helper and quits; the helper installs the staged version and starts it.
    pub fn update_restart_now(&mut self) {
        self.updates.restart_pending = false;
        let State::Ready(r, staged) = self.updates.state.clone() else { return };
        if self.updates.dry_run_restart.is_some() {
            self.updates.dry_run_restart = Some(staged);
            return;
        }
        let Some(target) = self.update_target() else { return };
        match spawn_helper(&target, &staged) {
            Ok(()) => {
                eprintln!("update: restarting into Device {}", r.version);
                let _ = slint::quit_event_loop();
            }
            Err(e) => {
                self.updates.error = e.clone();
                self.set_status(format!("Update failed: {e}"), 8);
                self.ui_dirty = true;
            }
        }
    }

    /// Called every frame: refreshes the pill while a download runs.
    pub fn updates_tick(&mut self) {
        if matches!(self.updates.state, State::Downloading(_)) {
            let pct = (self.updates.progress.fraction() * 100.0).floor() as u32;
            if self.updates.shown_percent != Some(pct) {
                self.updates.shown_percent = Some(pct);
                self.ui_dirty = true;
            }
        }
    }

    /// The dialog's text and buttons: (text, primary button, whether there is a Later button).
    pub fn updates_dialog(&self) -> (String, String, bool) {
        let can_install = self.updates.target.is_some() || !self.updates.target_known;
        let err = |t: String| if self.updates.error.is_empty() { t } else { format!("{t}\n{}", self.updates.error) };
        match &self.updates.state {
            State::Checking => ("Checking for updates…".into(), String::new(), false),
            State::Available(r) => {
                let installable = r.package.is_some() && r.sums.is_some() && can_install;
                let text = format!("Device {} is available. You're running v{CURRENT}.", r.version);
                (err(text), if installable { "Update".into() } else { "Download".into() }, true)
            }
            State::Downloading(r) => (
                format!("Downloading Device {}… {}%", r.version, (self.updates.progress.fraction() * 100.0).floor() as u32),
                String::new(),
                false,
            ),
            State::Ready(r, _) => (format!("Device {} is ready. Restart Device to finish updating.", r.version), "Restart".into(), true),
            State::UpToDate => (format!("You're running Device v{CURRENT}, the latest version."), String::new(), false),
            State::Idle if !self.updates.error.is_empty() => (self.updates.error.clone(), String::new(), false),
            State::Idle => (format!("You're running Device v{CURRENT}."), String::new(), false),
        }
    }

    pub(crate) fn push_updates(&self, s: &crate::Studio) {
        let (pill, version) = match &self.updates.state {
            State::Available(r) => (1, r.version.as_str()),
            State::Downloading(r) => (2, r.version.as_str()),
            State::Ready(r, _) => (3, r.version.as_str()),
            _ => (0, ""),
        };
        s.set_update_pill(pill);
        s.set_update_version(version.into());
        s.set_update_progress(self.updates.progress.fraction());
        let (text, primary, later) = self.updates_dialog();
        s.set_updates_text(text.into());
        s.set_updates_primary(primary.into());
        s.set_updates_later(later);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    #[test]
    fn versions_compare_like_semver() {
        assert!(is_newer("0.3.0", "0.2.0"));
        assert!(is_newer("v0.2.1", "0.2.0"));
        assert!(is_newer("1.0.0", "0.99.99"));
        assert!(!is_newer("0.2.0", "0.2.0"));
        assert!(!is_newer("0.1.9", "0.2.0"));
        // A release beats its pre-releases; pre-releases order by their numbers.
        assert!(is_newer("0.3.0", "0.3.0-rc.1"));
        assert!(is_newer("0.3.0-rc.1", "0.3.0-beta.2"));
        assert!(is_newer("0.3.0-beta.10", "0.3.0-beta.9"));
        assert!(!is_newer("0.3.0-alpha.1", "0.3.0"));
        // A development build of 0.2.0 is older than 0.2.0 itself.
        assert!(is_newer("0.2.0", "0.2.0-dev.1a2b3c4"));
        assert!(!is_newer("nonsense", "0.2.0"));
        assert!(!is_newer("0.3", "0.2.0"));
    }

    fn release_json(tag: &str) -> String {
        format!(
            r#"{{"tag_name":"{tag}","html_url":"https://github.com/devicedaw/device/releases/tag/{tag}","draft":false,"prerelease":false,
               "assets":[{{"name":"Device-macos-universal.dmg","browser_download_url":"https://x/Device-macos-universal.dmg","size":10}},
                         {{"name":"Device-linux-aarch64.tar.gz","browser_download_url":"https://x/a.tgz","size":20}},
                         {{"name":"SHA256SUMS.txt","browser_download_url":"https://x/SHA256SUMS.txt","size":30}}]}}"#
        )
    }

    #[test]
    fn release_picks_this_platforms_package() {
        let r = parse_release(&release_json("v0.3.0"), "macos", "aarch64").unwrap();
        assert_eq!(r.version, "0.3.0");
        assert_eq!(r.page, "https://github.com/devicedaw/device/releases/tag/v0.3.0");
        assert_eq!(r.package.as_ref().map(|a| a.size), Some(10));
        assert_eq!(r.sums.as_ref().map(|a| a.name.as_str()), Some("SHA256SUMS.txt"));
        assert_eq!(parse_release(&release_json("v0.3.0"), "linux", "aarch64").unwrap().package.unwrap().url, "https://x/a.tgz");
        // No Windows package in this release: the page is the fallback.
        assert_eq!(parse_release(&release_json("v0.3.0"), "windows", "x86_64").unwrap().package, None);
        assert!(parse_release(&release_json("latest"), "macos", "aarch64").is_err());
        assert!(parse_release("{}", "macos", "aarch64").is_err());
        let pre = release_json("v0.3.0").replace(r#""prerelease":false"#, r#""prerelease":true"#);
        assert!(parse_release(&pre, "macos", "aarch64").is_err());
    }

    #[test]
    fn checksum_list_is_read_like_sha256sum_writes_it() {
        let a = "a".repeat(64);
        let b = "B".repeat(64);
        let sums = format!("{a}  Device-0.3.0-macos-universal.dmg\n{b} *Device-macos-universal.dmg\n");
        assert_eq!(expected_sum(&sums, "Device-macos-universal.dmg"), Some("b".repeat(64)));
        assert_eq!(expected_sum(&sums, "Device-0.3.0-macos-universal.dmg"), Some(a));
        assert_eq!(expected_sum(&sums, "Device-linux-x86_64.tar.gz"), None);
        assert_eq!(expected_sum("xyz  Device-macos-universal.dmg", "Device-macos-universal.dmg"), None);
    }

    #[test]
    fn install_targets_follow_the_release_layouts() {
        assert_eq!(
            target_for("macos", Path::new("/Applications/Device.app/Contents/MacOS/device-studio"), None),
            Some(Target::MacApp { bundle: "/Applications/Device.app".into() })
        );
        // A development build (target/release) can't update itself.
        assert_eq!(target_for("macos", Path::new("/src/device/target/release/device-studio"), None), None);
        assert_eq!(
            target_for("linux", Path::new("/home/me/.local/lib/device-studio/device-studio"), None),
            Some(Target::LinuxPrefix { prefix: "/home/me/.local".into() })
        );
        assert_eq!(target_for("linux", Path::new("/opt/Device/bin/device-studio"), None), None);
        // Windows needs the installer's uninstaller next to the exe (not the portable .zip).
        let dir = std::env::temp_dir().join(format!("dstudio-update-target-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let exe = dir.join("device-studio.exe");
        assert_eq!(target_for("windows", &exe, None), None);
        std::fs::write(dir.join("unins000.exe"), b"").unwrap();
        assert_eq!(target_for("windows", &exe, Some(&std::env::temp_dir())), Some(Target::WindowsInstaller { exe: exe.clone(), all_users: false }));
        assert_eq!(target_for("windows", &exe, Some(Path::new("/elsewhere"))), Some(Target::WindowsInstaller { exe, all_users: true }));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn helpers_wait_for_the_app_and_start_the_new_version() {
        let mac = Target::MacApp { bundle: "/Applications/Device.app".into() };
        let (prog, args) = helper_command(&mac, Path::new("/Applications/.Device-update.app"), 42);
        assert_eq!(prog, "/bin/sh");
        assert_eq!(&args[3..], ["42", "/Applications/Device.app", "/Applications/.Device-update.app"]);
        let win = Target::WindowsInstaller { exe: r"C:\Users\O'Neil\AppData\Local\Programs\Device\device-studio.exe".into(), all_users: false };
        let (prog, args) = helper_command(&win, Path::new(r"C:\Temp\setup.exe"), 7);
        assert_eq!(prog, "powershell.exe");
        let script = args.last().unwrap();
        assert!(script.starts_with("Wait-Process -Id 7 "), "{script}");
        assert!(script.contains("'/CURRENTUSER'") && script.contains(r"'C:\Temp\setup.exe'"), "{script}");
        assert!(script.contains(r"'C:\Users\O''Neil\AppData\Local\Programs\Device\device-studio.exe'"), "{script}");
    }

    /// The macOS helper really swaps a bundle once the waited-for process is gone.
    #[cfg(unix)]
    #[test]
    fn mac_helper_swaps_the_bundle() {
        let dir = std::env::temp_dir().join(format!("dstudio-update-swap-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let (app, new) = (dir.join("Device.app"), dir.join(".Device-update.app"));
        std::fs::create_dir_all(&app).unwrap();
        std::fs::create_dir_all(&new).unwrap();
        std::fs::write(app.join("v"), "old").unwrap();
        std::fs::write(new.join("v"), "new").unwrap();
        // A child that has already exited stands in for the app, and `true` for `open`.
        let script = MAC_HELPER.replace("open \"$app\"", "true");
        let gone = std::process::Command::new("true").spawn().unwrap();
        let pid = gone.id();
        let mut gone = gone;
        gone.wait().unwrap();
        let st = std::process::Command::new("/bin/sh")
            .args(["-c", &script, "sh", &pid.to_string()])
            .arg(&app)
            .arg(&new)
            .status()
            .unwrap();
        assert!(st.success());
        assert_eq!(std::fs::read_to_string(app.join("v")).unwrap(), "new");
        assert!(!new.exists() && !dir.join("Device.app.previous").exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Serves `routes` (path → (status, body)) to `n` requests on 127.0.0.1.
    fn serve(routes: Vec<(&'static str, &'static str, Vec<u8>)>, n: usize) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        std::thread::spawn(move || {
            for _ in 0..n {
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
                let text = String::from_utf8_lossy(&req).to_string();
                let path = text.split_whitespace().nth(1).unwrap_or("/").to_string();
                let (status, body) = routes
                    .iter()
                    .find(|(p, _, _)| *p == path)
                    .map(|(_, st, b)| (*st, b.clone()))
                    .unwrap_or(("404 Not Found", Vec::new()));
                let head = format!("HTTP/1.1 {status}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n", body.len());
                s.write_all(head.as_bytes()).unwrap();
                s.write_all(&body).unwrap();
            }
        });
        base
    }

    #[test]
    fn fetch_latest_reads_the_release_and_survives_no_release() {
        let base = serve(vec![("/latest", "200 OK", release_json("v9.0.0").into_bytes())], 2);
        let r = fetch_latest(&format!("{base}/latest")).unwrap().unwrap();
        assert_eq!(r.version, "9.0.0");
        assert!(is_newer(&r.version, CURRENT));
        // No published release yet: nothing to offer, no error.
        assert_eq!(fetch_latest(&format!("{base}/none")), Ok(None));
        // Off-line: a plain-language error.
        let dead = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap();
        assert!(fetch_latest(&format!("http://{dead}/latest")).unwrap_err().contains("internet connection"));
        assert!(fetch_latest("http://example.com/latest").is_err(), "plain HTTP is refused");
    }

    #[test]
    fn download_counts_bytes_and_hashes_them() {
        let body: Vec<u8> = (0..300_000u32).map(|i| (i % 251) as u8).collect();
        let want: String = Sha256::digest(&body).iter().map(|b| format!("{b:02x}")).collect();
        let base = serve(vec![("/pkg", "200 OK", body.clone())], 2);
        let dest = std::env::temp_dir().join(format!("dstudio-update-dl-{}", std::process::id()));
        let p = Progress::default();
        assert_eq!(download(&format!("{base}/pkg"), &dest, &p), Ok(want));
        assert_eq!(p.received.load(AtomicOrdering::Relaxed), body.len() as u64);
        assert_eq!(p.fraction(), 1.0);
        assert_eq!(std::fs::read(&dest).unwrap(), body);
        assert!(download(&format!("{base}/missing"), &dest, &p).unwrap_err().contains("HTTP 404"));
        let _ = std::fs::remove_file(&dest);
    }
}
