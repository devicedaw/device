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
//! A recording pass writes its takes straight to disk, and the WAV header gets its sizes
//! only when the pass ends. So while audio is being recorded, `<pid>.takes.json` says where
//! the takes go and when the pass began. If the session dies mid-pass, the next launch gives
//! every take it finds there a header that matches the audio that reached the disk
//! ([`repair_take`]) and says where they are.
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

/// What `<pid>.takes.json` says about a recording pass in progress.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TakesMeta {
    /// The folder the takes are written to.
    pub dir: String,
    /// When the pass began (seconds since 1970).
    pub started: u64,
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
    /// `<pid>.takes.json` is written (audio is being recorded).
    pub takes_marked: bool,
    /// A backup being written on its own thread, and the revision it holds.
    pub pending: Option<(u64, std::thread::JoinHandle<Result<(), String>>)>,
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
    let _ = std::fs::remove_file(takes_path(dir, pid));
}

/// `<dir>/<pid>.takes.json`.
pub fn takes_path(dir: &Path, pid: u32) -> PathBuf {
    dir.join(format!("{pid}.takes.json"))
}

/// Size of the header the engine's take writer (`WavFloatWriter`) puts first: RIFF (12),
/// `fmt ` with 32-bit float (26), `fact` (12) and the `data` chunk header (8).
const TAKE_HEADER: u64 = 58;

/// Gives a take cut short by a crash a header that matches the audio on disk: the RIFF,
/// `fact` and `data` sizes, with any partial frame at the end dropped. Some(frames) when the
/// header was rewritten; None when the file is not a take or its header was already right.
pub fn repair_take(path: &Path) -> std::io::Result<Option<u64>> {
    use std::io::{Read, Seek, SeekFrom, Write};
    let mut f = std::fs::OpenOptions::new().read(true).write(true).open(path)?;
    let len = f.metadata()?.len();
    let mut h = [0u8; TAKE_HEADER as usize];
    if len < TAKE_HEADER || f.read_exact(&mut h).is_err() {
        return Ok(None);
    }
    let u16_at = |i: usize| u16::from_le_bytes([h[i], h[i + 1]]);
    let u32_at = |i: usize| u32::from_le_bytes([h[i], h[i + 1], h[i + 2], h[i + 3]]);
    let is_take = &h[0..4] == b"RIFF"
        && &h[8..12] == b"WAVE"
        && &h[12..16] == b"fmt "
        && u32_at(16) == 18
        && u16_at(20) == 3
        && u16_at(34) == 32
        && &h[38..42] == b"fact"
        && &h[50..54] == b"data";
    let channels = u64::from(u16_at(22));
    if !is_take || !(1..=2).contains(&channels) {
        return Ok(None);
    }
    let frame = channels * 4;
    let max_data = u64::from(u32::MAX) - TAKE_HEADER;
    let frames = (len - TAKE_HEADER).min(max_data) / frame;
    let data = frames * frame;
    if u64::from(u32_at(54)) == data && len == TAKE_HEADER + data {
        return Ok(None);
    }
    // Every size fits: data is at most u32::MAX - TAKE_HEADER.
    let (riff, frames32, data32) = ((TAKE_HEADER - 8 + data) as u32, frames as u32, data as u32);
    f.seek(SeekFrom::Start(4))?;
    f.write_all(&riff.to_le_bytes())?;
    f.seek(SeekFrom::Start(46))?;
    f.write_all(&frames32.to_le_bytes())?;
    f.seek(SeekFrom::Start(54))?;
    f.write_all(&data32.to_le_bytes())?;
    f.set_len(TAKE_HEADER + data)?;
    f.sync_all()?;
    Ok(Some(frames))
}

/// Repairs the takes of recording passes that sessions no longer running left unfinished,
/// and forgets those passes: the files repaired, as (path, frames).
pub fn recover_takes(dir: &Path, own_pid: u32) -> Vec<(PathBuf, u64)> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut repaired = Vec::new();
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let Some(pid) = name.strip_suffix(".takes.json").and_then(|p| p.parse::<u32>().ok()) else { continue };
        if pid == own_pid || running(pid) {
            continue;
        }
        if let Some(meta) = std::fs::read(e.path()).ok().and_then(|b| serde_json::from_slice::<TakesMeta>(&b).ok()) {
            let started = UNIX_EPOCH + Duration::from_secs(meta.started.saturating_sub(5));
            for take in std::fs::read_dir(&meta.dir).into_iter().flatten().flatten() {
                let p = take.path();
                let is_wav = p.extension().is_some_and(|x| x.eq_ignore_ascii_case("wav"));
                let recent = take.metadata().and_then(|m| m.modified()).is_ok_and(|t| t >= started);
                if is_wav && recent {
                    match repair_take(&p) {
                        Ok(Some(frames)) => repaired.push((p, frames)),
                        Ok(None) => {}
                        Err(err) => eprintln!("autosave: cannot repair {}: {err}", p.display()),
                    }
                }
            }
        }
        let _ = std::fs::remove_file(e.path());
    }
    repaired.sort();
    repaired
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

/// Writes the backup and the note beside it (on the autosave thread).
fn write_backup(project: &dstudio_core::Project, project_dir: Option<String>, dir: &Path, pid: u32) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let (backup, meta) = paths(dir, pid);
    persist::save(project, &backup).map_err(|e| e.to_string())?;
    let m = Meta { project_dir, written: now_secs() };
    let tmp = meta.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec(&m).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, &meta).map_err(|e| e.to_string())
}

impl Controller {
    /// UI timer: back up the project once it has had unsaved changes for [`INTERVAL`], and
    /// again every interval while it changes; drop the backup once nothing is unsaved. The
    /// write runs on its own thread (a large project takes tens of milliseconds to write).
    pub fn autosave_tick(&mut self) {
        let Some(dir) = self.autosave.dir.clone() else { return };
        let pid = std::process::id();
        if self.autosave.takes_marked && !self.rec.recording {
            // The pass ended and the engine finished the takes' headers.
            let _ = std::fs::remove_file(takes_path(&dir, pid));
            self.autosave.takes_marked = false;
        }
        if let Some((rev, write)) = self.autosave.pending.take_if(|(_, w)| w.is_finished()) {
            match write.join() {
                Ok(Ok(())) => self.autosave.saved_rev = Some(rev),
                Ok(Err(e)) => eprintln!("autosave: {e}"),
                Err(_) => eprintln!("autosave: the backup write panicked"),
            }
        }
        if self.autosave.pending.is_some() {
            // One write at a time, and nothing is removed while one is under way.
            return;
        }
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
        // Plugin instances own their state: pull it into the model first, as Save does.
        let states = self.engine.plugin_states();
        self.doc.sync_device_states(&states);
        let project = self.project().clone();
        let project_dir = self.project_dir.as_ref().map(|p| p.to_string_lossy().into_owned());
        let write = std::thread::Builder::new().name("autosave".into()).spawn(move || write_backup(&project, project_dir, &dir, pid));
        match write {
            Ok(w) => self.autosave.pending = Some((rev, w)),
            Err(e) => eprintln!("autosave: {e}"),
        }
    }

    /// At a normal quit: let a backup being written finish, then remove it.
    pub fn autosave_finish(&mut self) {
        if let Some((_, write)) = self.autosave.pending.take() {
            let _ = write.join();
        }
        if let Some(dir) = &self.autosave.dir {
            remove(dir, std::process::id());
        }
    }

    /// A recording pass started writing takes to `takes_dir`: note it, so that the next
    /// launch can repair them if this session dies before the pass ends.
    pub fn autosave_mark_recording(&mut self, takes_dir: &Path) {
        let Some(dir) = self.autosave.dir.clone() else { return };
        let meta = TakesMeta { dir: takes_dir.to_string_lossy().into_owned(), started: now_secs() };
        let path = takes_path(&dir, std::process::id());
        let written = std::fs::create_dir_all(&dir).and_then(|()| std::fs::write(&path, serde_json::to_vec(&meta).unwrap_or_default()));
        match written {
            Ok(()) => self.autosave.takes_marked = true,
            Err(e) => eprintln!("autosave: {}: {e}", path.display()),
        }
    }

    /// At launch: repair the takes a crashed session was recording, and open the backup it
    /// left as unsaved changes to its project. True when a project was recovered.
    pub fn autosave_recover(&mut self) -> bool {
        let Some(dir) = self.autosave.dir.clone() else { return false };
        let pid = std::process::id();
        let takes = recover_takes(&dir, pid);
        let takes_note = takes_note(&takes);
        let recovered = self.recover_backup(&dir, pid);
        let any = recovered.is_some();
        match (recovered, takes_note) {
            (Some(when), note) => {
                let note = note.map(|n| format!(" {n}")).unwrap_or_default();
                self.set_status(
                    format!("Device quit unexpectedly. Recovered your unsaved changes from {when}: save to keep them.{note}"),
                    20,
                );
            }
            (None, Some(note)) => self.set_status(format!("Device quit while recording. {note}"), 20),
            (None, None) => return false,
        }
        self.ui_dirty = true;
        any
    }

    /// Opens the newest backup a session that is no longer running left: how long ago it
    /// was written, in words.
    fn recover_backup(&mut self, dir: &Path, pid: u32) -> Option<String> {
        let (from, meta) = leftover(dir, pid)?;
        let backup = match adopt(dir, from, pid) {
            Ok(p) => p,
            Err(e) => {
                eprintln!("autosave: cannot take over the backup of process {from}: {e}");
                return None;
            }
        };
        if !self.open(&backup) {
            return None;
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
        Some(ago(meta.written))
    }
}

/// What the status line says about takes [`recover_takes`] repaired.
fn takes_note(takes: &[(PathBuf, u64)]) -> Option<String> {
    let (first, _) = takes.first()?;
    let folder = first.parent().map(|p| p.display().to_string()).unwrap_or_default();
    Some(match takes.len() {
        1 => format!("The take being recorded was saved up to the crash, in {folder}: drag it in from there."),
        n => format!("The {n} takes being recorded were saved up to the crash, in {folder}: drag them in from there."),
    })
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
    use super::{Meta, TakesMeta, adopt, leftover, paths, recover_takes, remove, repair_take, takes_path};

    /// One timer tick, then another once the backup it started is written.
    fn tick(c: &mut crate::controller::Controller) {
        c.autosave_tick();
        while c.autosave.pending.as_ref().is_some_and(|(_, w)| !w.is_finished()) {
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        c.autosave_tick();
    }

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
        tick(&mut c);
        assert!(!backup.exists(), "nothing unsaved: no backup");
        c.execute(dstudio_core::Command::SetTempo { bpm: 97.0, at: None });
        tick(&mut c);
        let saved = dstudio_core::persist::load(&backup).expect("a backup of the unsaved project").project;
        assert_eq!(saved.transport.bpm, 97.0);
        let song = dir.join("Song.dsproj");
        c.save_to(&song);
        tick(&mut c);
        assert!(!backup.exists(), "saved: the backup goes");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A take as the engine's `WavFloatWriter` leaves it when the app dies mid-pass: the
    /// header it wrote at the start (every size 0), then the audio that reached the disk.
    fn cut_take(path: &std::path::Path, channels: u16, samples: &[f32], extra: &[u8]) {
        let mut b = Vec::new();
        b.extend_from_slice(b"RIFF");
        b.extend_from_slice(&50u32.to_le_bytes());
        b.extend_from_slice(b"WAVEfmt ");
        b.extend_from_slice(&18u32.to_le_bytes());
        b.extend_from_slice(&3u16.to_le_bytes());
        b.extend_from_slice(&channels.to_le_bytes());
        b.extend_from_slice(&48_000u32.to_le_bytes());
        b.extend_from_slice(&(48_000 * u32::from(channels) * 4).to_le_bytes());
        b.extend_from_slice(&(channels * 4).to_le_bytes());
        b.extend_from_slice(&32u16.to_le_bytes());
        b.extend_from_slice(&0u16.to_le_bytes());
        b.extend_from_slice(b"fact");
        b.extend_from_slice(&4u32.to_le_bytes());
        b.extend_from_slice(&0u32.to_le_bytes());
        b.extend_from_slice(b"data");
        b.extend_from_slice(&0u32.to_le_bytes());
        assert_eq!(b.len(), 58);
        for s in samples {
            b.extend_from_slice(&s.to_le_bytes());
        }
        b.extend_from_slice(extra);
        std::fs::write(path, b).unwrap();
    }

    #[test]
    fn a_take_cut_short_gets_a_header_that_matches_its_audio() {
        let dir = scratch("repair");
        let take = dir.join("Audio 1.wav");
        let samples: Vec<f32> = (0..2_000).map(|i| (i as f32 * 0.01).sin()).collect();
        // Half a sample of a frame that was being written when the app died.
        cut_take(&take, 2, &samples, &[1, 2, 3]);
        let (_, _, before) = crate::wavgen::read_wav(&take).unwrap();
        assert!(before.is_empty(), "a cut-short take reads as silence");
        assert_eq!(repair_take(&take).unwrap(), Some(1_000));
        let (rate, ch, after) = crate::wavgen::read_wav(&take).unwrap();
        assert_eq!((rate, ch), (48_000, 2));
        assert_eq!(after, samples, "every whole frame on disk is kept");
        assert_eq!(std::fs::metadata(&take).unwrap().len(), 58 + 8_000, "the partial frame is dropped");
        assert_eq!(repair_take(&take).unwrap(), None, "a take with the right header is left alone");
        let other = dir.join("notes.wav");
        std::fs::write(&other, b"RIFF....WAVEsomething else entirely, longer than a take header").unwrap();
        assert_eq!(repair_take(&other).unwrap(), None, "only the engine's take format is touched");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_next_launch_repairs_the_takes_of_a_pass_that_never_ended() {
        let dir = scratch("takes");
        let takes = dir.join("media");
        std::fs::create_dir_all(&takes).unwrap();
        cut_take(&takes.join("Audio 1.wav"), 1, &[0.5; 480], &[]);
        cut_take(&takes.join("Audio 2.wav"), 1, &[0.25; 96], &[]);
        let dead = 2_000_000_003;
        let meta = TakesMeta { dir: takes.to_string_lossy().into_owned(), started: super::now_secs() };
        std::fs::write(takes_path(&dir, dead), serde_json::to_vec(&meta).unwrap()).unwrap();
        // This session's own pass is still running: it is not touched.
        let me = std::process::id();
        std::fs::write(takes_path(&dir, me), serde_json::to_vec(&meta).unwrap()).unwrap();
        let fixed = recover_takes(&dir, me);
        let names: Vec<_> = fixed.iter().map(|(p, n)| (p.file_name().unwrap().to_string_lossy().into_owned(), *n)).collect();
        assert_eq!(names, vec![("Audio 1.wav".into(), 480), ("Audio 2.wav".into(), 96)]);
        assert!(!takes_path(&dir, dead).exists(), "the finished pass is forgotten");
        assert!(takes_path(&dir, me).exists());
        assert!(recover_takes(&dir, me).is_empty());
        let note = super::takes_note(&fixed).unwrap();
        assert!(note.starts_with("The 2 takes being recorded were saved up to the crash, in "), "{note}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_crashed_sessions_changes_open_unsaved_and_go_when_declined() {
        use crate::controller::Controller;
        use std::sync::Arc;
        let dir = scratch("recover");
        let dead = 2_000_000_004;
        let (p, m) = paths(&dir, dead);
        let mut project = dstudio_core::Project::default();
        project.transport.bpm = 133.0;
        dstudio_core::persist::save(&project, &p).unwrap();
        let meta = Meta { project_dir: Some("/songs/Song.dsproj".into()), written: super::now_secs() - 300 };
        std::fs::write(m, serde_json::to_vec(&meta).unwrap()).unwrap();
        let engine = Arc::new(dstudio_engine::Engine::new(dstudio_engine::Config::default()).unwrap());
        let mut c = Controller::new(engine, dstudio_core::Project::default());
        c.autosave.dir = Some(dir.clone());
        assert!(c.autosave_recover());
        assert_eq!(c.project().transport.bpm, 133.0, "the backup's changes are open");
        assert!(c.doc.is_dirty(), "as unsaved changes");
        assert_eq!(c.project_dir.as_deref(), Some(std::path::Path::new("/songs/Song.dsproj")), "Save goes to the original project");
        let status = c.status_text().unwrap_or_default().to_owned();
        assert!(status.contains("Recovered your unsaved changes from 5 minutes ago"), "{status}");
        let mine = paths(&dir, std::process::id()).0;
        assert!(mine.exists() && !paths(&dir, dead).0.exists(), "this session took the backup over");
        // Don't Save: the changes, and with them the backup, are thrown away.
        c.new_project();
        tick(&mut c);
        assert!(!mine.exists());
        assert!(!c.autosave_recover(), "nothing left to recover");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
