//! Autosave and crash recovery (spec 025, S0.3).
//!
//! While a project has unsaved changes, a backup copy of it is written every
//! [`INTERVAL`] to `<support dir>/Autosave/<pid>.dsproj` (the same atomic project write
//! as Save), with `<pid>.json` beside it saying which project it belongs to and when it
//! was written. The backup goes away once the project has no unsaved changes (it was
//! saved, or another project was opened), and when the app quits normally.
//!
//! So a backup that is still there at launch, from a process that is no longer running,
//! is from a session that crashed (or was killed). The next launch opens it as an
//! unsaved project, under the original project's folder when it had one, and says so in
//! the status line: Save keeps it, closing without saving throws it away. The recovered
//! backup takes over this session's name, so it is not lost if this session crashes too.
//!
//! Self-tests and screenshot runs never autosave or recover (no support dir is given).

use crate::controller::Controller;
use dstudio_core::persist;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// How often a project with unsaved changes is backed up.
pub const INTERVAL: Duration = Duration::from_secs(120);

/// What `<pid>.json` says about the backup next to it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Meta {
    /// The project's folder, or None for a project that was never saved.
    pub project_dir: Option<String>,
    /// When the backup was written (seconds since 1970).
    pub written: u64,
}

#[derive(Debug, Default)]
pub struct Autosave {
    /// `<support dir>/Autosave`; None turns autosave off (self-tests, screenshot runs).
    pub dir: Option<PathBuf>,
    /// The project revision the current backup holds; None when there is no backup.
    pub saved_rev: Option<u64>,
    /// When the project last went from saved to unsaved.
    pub dirty_since: Option<Instant>,
    pub last_write: Option<Instant>,
    /// Overrides [`INTERVAL`] (tests).
    pub interval: Option<Duration>,
}

fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// `<dir>/<pid>.dsproj` and `<dir>/<pid>.json`.
pub fn paths(dir: &Path, pid: u32) -> (PathBuf, PathBuf) {
    (dir.join(format!("{pid}.{}", persist::EXTENSION)), dir.join(format!("{pid}.json")))
}

pub fn remove(dir: &Path, pid: u32) {
    let (project, meta) = paths(dir, pid);
    let _ = std::fs::remove_file(meta);
    let _ = std::fs::remove_dir_all(project);
}

#[cfg(unix)]
fn running(pid: u32) -> bool {
    let Ok(pid) = libc::pid_t::try_from(pid) else { return false };
    if pid <= 0 {
        return false;
    }
    // SAFETY: signal 0 only checks that the process exists; nothing is sent.
    let r = unsafe { libc::kill(pid, 0) };
    r == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

#[cfg(not(unix))]
fn running(_pid: u32) -> bool {
    // A second copy of the app running at once is rare on Windows; its backup would be
    // offered as a recovered copy, which loses nothing.
    false
}

/// A backup left by a session that is no longer running, newest first: (pid, meta).
pub fn leftover(dir: &Path, own_pid: u32) -> Option<(u32, Meta)> {
    let mut found: Vec<(u32, Meta)> = std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let pid: u32 = name.strip_suffix(".json")?.parse().ok()?;
            let meta: Meta = serde_json::from_slice(&std::fs::read(e.path()).ok()?).ok()?;
            (pid != own_pid && !running(pid) && paths(dir, pid).0.join(persist::PROJECT_FILE).exists()).then_some((pid, meta))
        })
        .collect();
    found.sort_by_key(|(_, m)| std::cmp::Reverse(m.written));
    found.into_iter().next()
}

/// Gives the backup of `from` to `to` (this session), replacing any backup `to` has.
pub fn adopt(dir: &Path, from: u32, to: u32) -> std::io::Result<PathBuf> {
    remove(dir, to);
    let (fp, fm) = paths(dir, from);
    let (tp, tm) = paths(dir, to);
    std::fs::rename(&fp, &tp)?;
    std::fs::rename(&fm, &tm)?;
    Ok(tp)
}

impl Controller {
    /// UI timer: back up the project once it has had unsaved changes for [`INTERVAL`], and
    /// again every interval while it changes; drop the backup once nothing is unsaved.
    pub fn autosave_tick(&mut self) {
        let Some(dir) = self.autosave.dir.clone() else { return };
        let pid = std::process::id();
        if !self.doc.is_dirty() {
            if self.autosave.saved_rev.take().is_some() {
                remove(&dir, pid);
            }
            self.autosave.dirty_since = None;
            return;
        }
        let now = Instant::now();
        let since = *self.autosave.dirty_since.get_or_insert(now);
        let rev = self.doc.revision();
        let interval = self.autosave.interval.unwrap_or(INTERVAL);
        let last = self.autosave.last_write.filter(|t| *t > since).unwrap_or(since);
        // Not while recording: the take joins the project when the pass ends.
        if self.rec.recording || self.autosave.saved_rev == Some(rev) || now.duration_since(last) < interval {
            return;
        }
        self.autosave.last_write = Some(now);
        match self.write_backup(&dir, pid) {
            Ok(()) => self.autosave.saved_rev = Some(rev),
            Err(e) => eprintln!("autosave: {e}"),
        }
    }

    fn write_backup(&mut self, dir: &Path, pid: u32) -> Result<(), String> {
        // Plugin instances own their state: pull it into the model first, as Save does.
        let states = self.engine.plugin_states();
        self.doc.sync_device_states(&states);
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        let (project, meta) = paths(dir, pid);
        persist::save(self.project(), &project).map_err(|e| e.to_string())?;
        let m = Meta { project_dir: self.project_dir.as_ref().map(|p| p.to_string_lossy().into_owned()), written: now_secs() };
        let tmp = meta.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec(&m).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, &meta).map_err(|e| e.to_string())
    }

    /// At launch: open the backup a crashed session left, as unsaved changes to its project.
    /// True when one was recovered.
    pub fn autosave_recover(&mut self) -> bool {
        let Some(dir) = self.autosave.dir.clone() else { return false };
        let pid = std::process::id();
        let Some((from, meta)) = leftover(&dir, pid) else { return false };
        let backup = match adopt(&dir, from, pid) {
            Ok(p) => p,
            Err(e) => {
                eprintln!("autosave: cannot take over the backup of process {from}: {e}");
                return false;
            }
        };
        if !self.open(&backup) {
            return false;
        }
        // open() remembered the backup as a recent project and made it the project's folder.
        let b = backup.to_string_lossy().into_owned();
        if let Some(i) = self.settings.prefs.recent_projects.iter().position(|p| *p == b) {
            self.settings.prefs.recent_projects.remove(i);
            if i < self.settings.prefs.recent_resolved.len() {
                self.settings.prefs.recent_resolved.remove(i);
            }
            self.save_prefs();
        }
        self.project_dir = meta.project_dir.map(PathBuf::from);
        self.doc.mark_edited();
        self.autosave.saved_rev = Some(self.doc.revision());
        self.autosave.last_write = Some(Instant::now());
        let when = ago(meta.written);
        self.set_status(
            format!("Device quit unexpectedly. Recovered your unsaved changes from {when}: save to keep them."),
            20,
        );
        self.ui_dirty = true;
        true
    }
}

/// How long ago `written` (seconds since 1970) was, in words.
fn ago(written: u64) -> String {
    let ago = now_secs().saturating_sub(written) / 60;
    match ago {
        0 => "less than a minute ago".into(),
        1 => "a minute ago".into(),
        n if n < 120 => format!("{n} minutes ago"),
        n => format!("{} hours ago", n / 60),
    }
}

#[cfg(test)]
mod tests {
    use super::{Meta, adopt, leftover, paths, remove};

    fn scratch(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("dstudio-autosave-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn backup(dir: &std::path::Path, pid: u32, written: u64) {
        let (p, m) = paths(dir, pid);
        dstudio_core::persist::save(&dstudio_core::Project::default(), &p).unwrap();
        let meta = Meta { project_dir: Some("/songs/a.dsproj".into()), written };
        std::fs::write(m, serde_json::to_vec(&meta).unwrap()).unwrap();
    }

    #[test]
    fn a_backup_from_a_finished_process_is_recovered_and_taken_over() {
        let dir = scratch("leftover");
        let me = std::process::id();
        // Two dead sessions (pids above any system's pid limit) and our own backup.
        backup(&dir, 2_000_000_001, 100);
        backup(&dir, 2_000_000_002, 200);
        backup(&dir, me, 300);
        let (pid, meta) = leftover(&dir, me).expect("a leftover backup");
        assert_eq!((pid, meta.written), (2_000_000_002, 200), "the newest one from a session that is gone");
        let taken = adopt(&dir, pid, me).unwrap();
        assert_eq!(taken, paths(&dir, me).0);
        assert!(!paths(&dir, pid).0.exists() && !paths(&dir, pid).1.exists());
        assert!(dstudio_core::persist::load(&taken).is_ok());
        remove(&dir, me);
        remove(&dir, 2_000_000_001);
        assert!(leftover(&dir, me).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn unsaved_changes_are_backed_up_and_the_backup_goes_once_saved() {
        use crate::controller::Controller;
        use std::sync::Arc;
        use std::time::Duration;
        let dir = scratch("tick");
        let engine = Arc::new(dstudio_engine::Engine::new(dstudio_engine::Config::default()).unwrap());
        let mut c = Controller::new(engine, dstudio_core::Project::default());
        c.autosave.dir = Some(dir.clone());
        c.autosave.interval = Some(Duration::ZERO);
        let backup = paths(&dir, std::process::id()).0;
        c.autosave_tick();
        assert!(!backup.exists(), "nothing unsaved: no backup");
        c.execute(dstudio_core::Command::SetTempo { bpm: 97.0, at: None });
        c.autosave_tick();
        let saved = dstudio_core::persist::load(&backup).expect("a backup of the unsaved project").project;
        assert_eq!(saved.transport.bpm, 97.0);
        let song = dir.join("Song.dsproj");
        c.save_to(&song);
        c.autosave_tick();
        assert!(!backup.exists(), "saved: the backup goes");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
