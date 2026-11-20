//! Project persistence.
//!
//! A project is a directory `Name.dsproj/` (portable, VCS-friendly) holding
//! `project.json`. Media stays where it is and is referenced by both a
//! project-relative and an absolute path. The JSON envelope carries a schema
//! version; `migrate` upgrades older documents step by step.
//!
//! SQLite is reserved for large, query-heavy metadata (sample library index,
//! analysis caches), not for the project document itself. See docs/architecture.md.

use crate::model::Project;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use thiserror::Error;

pub const FORMAT: &str = "device-studio-project";
/// The newest schema this build reads. Version 2 adds tempo changes (spec 015); a project without
/// them is still written as version 1 ([schema_version_for]), so builds before 2 keep opening it,
/// and refuse (never misread) one that has them.
pub const SCHEMA_VERSION: u32 = 2;
pub const PROJECT_FILE: &str = "project.json";
pub const EXTENSION: &str = "dsproj";

#[derive(Debug, Error)]
pub enum PersistError {
    #[error("i/o error at {path}: {source}")]
    Io { path: PathBuf, source: std::io::Error },
    #[error("malformed project file: {0}")]
    Format(#[from] serde_json::Error),
    #[error(
        "not a Device project (format = {0:?}); a project saved before the app was renamed has to be opened in an earlier build and saved again with Save As under a .dsproj name"
    )]
    WrongFormat(String),
    #[error("project schema {found} is newer than supported {supported}")]
    TooNew { found: u32, supported: u32 },
    #[error("damaged project file: {0}")]
    Damaged(String),
}

fn io(path: &Path) -> impl FnOnce(std::io::Error) -> PersistError + '_ {
    move |source| PersistError::Io { path: path.to_path_buf(), source }
}

#[derive(Serialize, Deserialize)]
struct Envelope {
    format: String,
    schema_version: u32,
    saved_with: String,
    project: serde_json::Value,
}

/// Result of loading: the project plus media that couldn't be found.
#[derive(Debug)]
pub struct Loaded {
    pub project: Project,
    pub missing_media: Vec<String>,
}

/// The schema a project is written with: the oldest that holds everything in it.
pub fn schema_version_for(project: &Project) -> u32 {
    if project.transport.tempo.is_empty() { 1 } else { 2 }
}

/// Save atomically (write temp file, fsync, rename).
pub fn save(project: &Project, dir: &Path) -> Result<(), PersistError> {
    fs::create_dir_all(dir).map_err(io(dir))?;
    let mut p = project.clone();
    let abs_dir = resolved(dir);
    for a in &mut p.assets {
        let target = Path::new(&a.absolute_path);
        a.relative_path = relative_to(&abs_dir, &fs::canonicalize(target).unwrap_or_else(|_| normalize(target)));
    }
    let env = Envelope {
        format: FORMAT.into(),
        schema_version: schema_version_for(&p),
        saved_with: concat!("dstudio-core ", env!("CARGO_PKG_VERSION")).into(),
        project: serde_json::to_value(&p)?,
    };
    let json = serde_json::to_vec_pretty(&env)?;
    let target = dir.join(PROJECT_FILE);
    let tmp = dir.join(format!(".{PROJECT_FILE}.tmp"));
    {
        let mut f = fs::File::create(&tmp).map_err(io(&tmp))?;
        f.write_all(&json).map_err(io(&tmp))?;
        f.sync_all().map_err(io(&tmp))?;
    }
    fs::rename(&tmp, &target).map_err(io(&target))?;
    Ok(())
}

/// Loads the project folder at `dir`: reads its envelope, rejects anything that is not the current
/// format or is newer than this build, migrates older schemas, and returns the project with any missing media files.
pub fn load(dir: &Path) -> Result<Loaded, PersistError> {
    let path = dir.join(PROJECT_FILE);
    let bytes = fs::read(&path).map_err(io(&path))?;
    let env: Envelope = serde_json::from_slice(&bytes)?;
    if env.format != FORMAT {
        return Err(PersistError::WrongFormat(env.format));
    }
    if env.schema_version > SCHEMA_VERSION {
        return Err(PersistError::TooNew { found: env.schema_version, supported: SCHEMA_VERSION });
    }
    let value = migrate(env.schema_version, env.project);
    let mut project = deserialize(value)?;
    repair(&mut project)?;
    // Resolve media: prefer the project-relative path (project was moved
    // together with its media), fall back to the absolute path.
    let abs_dir = resolved(dir);
    let mut missing = Vec::new();
    for a in &mut project.assets {
        let rel = a.relative_path.as_ref().map(|r| normalize(&abs_dir.join(r)));
        if let Some(r) = rel.filter(|r| r.exists()) {
            a.absolute_path = r.to_string_lossy().into_owned();
        } else if !Path::new(&a.absolute_path).exists() {
            missing.push(a.absolute_path.clone());
        }
    }
    Ok(Loaded { project, missing_media: missing })
}

/// Upgrade a project JSON value from `from` to the current schema.
fn migrate(from: u32, value: serde_json::Value) -> serde_json::Value {
    #[allow(clippy::match_single_binding)]
    match from {
        // v2 only adds `transport.tempo`, which v1 files don't have (it defaults to empty).
        _ => value,
    }
}

/// Deserializes the project. A float that was not finite when saved is written as `null`, which
/// would make the file unreadable; each such `null` loads as 0 instead (and [repair] then puts
/// the fields it knows back in range), so a project never becomes unloadable through a bad float.
fn deserialize(value: serde_json::Value) -> Result<Project, PersistError> {
    let null_float = |e: &serde_json::Error| e.is_data() && e.to_string().starts_with("invalid type: null, expected f");
    let first = match Project::deserialize(&value) {
        Ok(p) => return Ok(p),
        Err(e) if null_float(&e) => e,
        Err(e) => return Err(e.into()),
    };
    // Compact text (one line), so an error's column is a byte offset into it.
    let mut text = serde_json::to_string(&value)?;
    for _ in 0..text.matches("null").count() {
        match serde_json::from_str(&text) {
            Ok(p) => return Ok(p),
            Err(e) if null_float(&e) && e.line() == 1 && text.get(..e.column()).is_some_and(|t| t.ends_with("null")) => {
                text.replace_range(e.column() - 4..e.column(), "0");
            }
            Err(e) => return Err(e.into()),
        }
    }
    Err(first.into())
}

/// Ids from here on leave too little room to allocate new ones; `repair` moves a `next_id`
/// beyond it back and refuses a project using such an id.
const MAX_ID: u64 = u64::MAX / 2;
/// Latest timeline position a loaded clip, note or loop may have (far beyond any real song), so
/// positions can be added and converted to frames without overflowing.
const MAX_TICKS: crate::Ticks = 1 << 40;

/// Repairs what a hand-edited, damaged or older file could hold that the commands would never
/// produce, so the project opens and every edit (and its undo) works: values out of the ranges
/// the commands accept are clamped, invalid notes dropped or fixed, and the id allocator moved past
/// every id in use. Valid projects are unchanged. Fails only when the ids are exhausted.
fn repair(project: &mut Project) -> Result<(), PersistError> {
    use crate::command::{MAX_BPM, MAX_GAIN, MIN_BPM};
    use crate::model::{TimeSignature, Track};
    let fin = |v: f32, lo: f32, hi: f32, default: f32| if v.is_finite() { v.clamp(lo, hi) } else { default };

    project.sanitize_routing();
    // Instrument tracks saved with the classic synth get the Synth device.
    crate::builtin::synth_v2::upgrade_project(project);

    // Transport: the same ranges SetTempo, SetTimeSignature and SetLoop accept, so undoing a
    // change to them can restore the loaded value.
    let t = &mut project.transport;
    t.bpm = if t.bpm.is_finite() && t.bpm > 0.0 { t.bpm.clamp(MIN_BPM, MAX_BPM) } else { 120.0 };
    let ts = t.time_signature;
    if ts.numerator == 0 || ts.numerator > 32 || ![1, 2, 4, 8, 16, 32].contains(&ts.denominator) {
        t.time_signature = TimeSignature { numerator: 4, denominator: 4 };
    }
    // Tempo changes: the ranges SetTempoMap accepts, sorted, one point per tick, and the tempo at
    // bar 1 kept as the song tempo.
    t.tempo.retain(|q| q.bpm.is_finite() && q.time >= 0);
    for q in &mut t.tempo {
        q.bpm = q.bpm.clamp(MIN_BPM, MAX_BPM);
        q.time = q.time.min(crate::tempo::MAX_TEMPO_TICK);
    }
    t.tempo.sort_by_key(|q| q.time);
    t.tempo.dedup_by_key(|q| q.time);
    t.tempo.truncate(crate::tempo::MAX_TEMPO_POINTS);
    if let Some(f) = t.tempo.first() {
        t.bpm = f.bpm;
    }
    if t.key.is_some_and(|k| k.root > 11) {
        t.key = None;
    }
    t.loop_start = t.loop_start.clamp(0, MAX_TICKS);
    if t.loop_end <= t.loop_start || t.loop_end > 2 * MAX_TICKS {
        let ts = t.time_signature;
        t.loop_end = t.loop_start + 8 * crate::PPQ * 4 * ts.numerator as i64 / ts.denominator as i64;
    }
    project.master_gain = fin(project.master_gain, 0.0, MAX_GAIN, 1.0);

    for track in &mut project.tracks {
        track.gain = fin(track.gain, 0.0, MAX_GAIN, 1.0);
        track.pan = fin(track.pan, -1.0, 1.0, 0.0);
        track.trim_db = fin(track.trim_db, Track::TRIM_RANGE_DB.0, Track::TRIM_RANGE_DB.1, 0.0);
        track.width = fin(track.width, Track::WIDTH_RANGE.0, Track::WIDTH_RANGE.1, 1.0);
        track.height = fin(track.height, Track::HEIGHT_RANGE.0, Track::HEIGHT_RANGE.1, 1.0);
        for c in &mut track.clips {
            c.start = c.start.clamp(0, MAX_TICKS);
            c.gain = fin(c.gain, 0.0, MAX_GAIN, 1.0);
        }
        for c in &mut track.midi_clips {
            c.start = c.start.clamp(0, MAX_TICKS);
            c.length = c.length.clamp(1, MAX_TICKS);
            // The engine refuses a whole session over one note it can't play, so notes the
            // commands would refuse are fixed where that keeps the note, and dropped otherwise.
            if crate::midi::validate_notes(&c.notes).is_err() {
                let mut ids = std::collections::HashSet::new();
                let mut next = c.notes.iter().map(|n| n.id).max().map_or(1, |m| m.saturating_add(1));
                c.notes.retain_mut(|n| {
                    n.velocity = n.velocity.clamp(1, 127);
                    n.length = n.length.min(MAX_TICKS);
                    if !ids.insert(n.id) {
                        n.id = next;
                        next = next.saturating_add(1);
                        ids.insert(n.id);
                    }
                    n.pitch <= 127 && (0..=MAX_TICKS).contains(&n.start) && n.length > 0
                });
            }
            // A step pattern the editor can't hold is dropped: the notes are the clip's content,
            // so the clip loads as plain MIDI with nothing lost.
            if c.pattern.as_ref().is_some_and(|p| p.validate().is_err()) {
                c.pattern = None;
            }
            // Likewise a pattern list the sidebar can't show: the clip keeps the notes it plays.
            if c.composer.as_ref().is_some_and(|k| k.validate().is_err()) {
                c.composer = None;
            }
        }
    }

    // Every id the shared allocator hands out: a stale `next_id` would hand out one in use.
    let ids = project.tracks.iter().flat_map(|t| {
        let clips = t.clips.iter().map(|c| c.id.0).chain(t.midi_clips.iter().map(|c| c.id.0));
        let devices = t.devices.iter().chain(t.instrument_plugin.as_ref()).map(|d| d.id.0);
        std::iter::once(t.id.0).chain(clips).chain(devices)
    });
    let max = ids.chain(project.assets.iter().map(|a| a.id.0)).chain(project.sections.iter().map(|s| s.id.0)).max().unwrap_or(0);
    if max >= MAX_ID {
        return Err(PersistError::Damaged(format!("id {max} leaves no room for new ids")));
    }
    if project.next_id <= max || project.next_id >= MAX_ID {
        project.next_id = max + 1;
    }
    if dedupe_ids(project) {
        // Sends, outputs and sidechains name the first track with an id; checked again now
        // that the later ones have their own.
        project.sanitize_routing();
    }
    Ok(())
}

/// Gives a fresh id to every track, clip and device whose id an earlier one of its kind
/// already has (a merge or hand edit can leave two). The first keeps the id, so references
/// from other tracks stay where they were; a renumbered device's own track (chains, mod
/// links, automation lanes) follows it. Returns whether anything changed.
fn dedupe_ids(project: &mut Project) -> bool {
    use crate::automation::AutomationTarget;
    use crate::device::DeviceId;
    use crate::model::{ClipId, TrackId};
    use std::collections::{HashMap, HashSet};
    let (mut tracks, mut clips, mut devices) = (HashSet::new(), HashSet::new(), HashSet::new());
    let mut next = project.next_id;
    let mut changed = false;
    let mut fresh = || {
        let id = next;
        next += 1;
        id
    };
    for t in &mut project.tracks {
        if !tracks.insert(t.id) {
            t.id = TrackId(fresh());
            tracks.insert(t.id);
            changed = true;
        }
        for id in t.clips.iter_mut().map(|c| &mut c.id).chain(t.midi_clips.iter_mut().map(|c| &mut c.id)) {
            if !clips.insert(*id) {
                *id = ClipId(fresh());
                clips.insert(*id);
                changed = true;
            }
        }
        let mut map = HashMap::new();
        for d in t.devices.iter_mut().chain(t.instrument_plugin.as_mut()) {
            if !devices.insert(d.id) {
                let new = DeviceId(fresh());
                map.insert(d.id, new);
                d.id = new;
                devices.insert(new);
            }
        }
        if map.is_empty() {
            continue;
        }
        changed = true;
        let remap = |d: &mut DeviceId| {
            if let Some(n) = map.get(d) {
                *d = *n;
            }
        };
        for d in t.devices.iter_mut().chain(t.instrument_plugin.as_mut()) {
            if let Some(c) = d.chain.as_mut() {
                remap(c);
            }
            for l in &mut d.links {
                remap(&mut l.target);
            }
        }
        for lane in &mut t.automation {
            match &mut lane.target {
                AutomationTarget::Plugin { device, .. }
                | AutomationTarget::Builtin { device, .. }
                | AutomationTarget::DeviceOn { device, .. } => remap(device),
                _ => {}
            }
        }
    }
    project.next_id = next;
    changed
}

/// The project folder as an absolute path with symlinks and `..` resolved, the way media paths
/// are stored at import, so the relative media paths saved and resolved against it agree.
fn resolved(dir: &Path) -> PathBuf {
    fs::canonicalize(dir).unwrap_or_else(|_| normalize(&absolute(dir)))
}

fn absolute(p: &Path) -> PathBuf {
    if p.is_absolute() { p.to_path_buf() } else { std::env::current_dir().map(|c| c.join(p)).unwrap_or_else(|_| p.to_path_buf()) }
}

/// Lexically resolve `.` and `..` (no filesystem access, symlinks untouched).
fn normalize(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// Lexical relative path from `base` dir to `target` (both absolute).
fn relative_to(base: &Path, target: &Path) -> Option<String> {
    if !target.is_absolute() {
        return None;
    }
    let b: Vec<Component> = base.components().collect();
    let t: Vec<Component> = target.components().collect();
    // Different roots/prefixes (e.g. Windows drives) → no relative path.
    if b.first() != t.first() {
        return None;
    }
    let common = b.iter().zip(&t).take_while(|(x, y)| x == y).count();
    let mut rel = PathBuf::new();
    for _ in common..b.len() {
        rel.push("..");
    }
    for c in &t[common..] {
        rel.push(c.as_os_str());
    }
    // Store with forward slashes for portability across platforms.
    Some(rel.to_string_lossy().replace('\\', "/"))
}

#[cfg(test)]
mod tests {
    use super::*;
    /// An absolute path on this platform ("/a/b" on Unix, "C:/a/b" on Windows,
    /// where a path without a drive is not absolute).
    fn abs(p: &str) -> PathBuf {
        if cfg!(windows) { PathBuf::from(format!("C:{p}")) } else { PathBuf::from(p) }
    }

    #[test]
    fn relative_paths() {
        assert_eq!(relative_to(&abs("/a/b/p.dsproj"), &abs("/a/b/media/x.wav")).as_deref(), Some("../media/x.wav"));
        assert_eq!(relative_to(&abs("/a/p"), &abs("/a/p/x.wav")).as_deref(), Some("x.wav"));
    }
}
