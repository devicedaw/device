//! Undo/redo history built on invertible [`Command`]s.

use crate::command::{Command, EditError};
use crate::model::{Project, TrackId};
use std::time::{Duration, Instant, SystemTime};

#[derive(Debug, Clone)]
struct Entry {
    label: String,
    /// Command that re-applies the edit.
    redo: Command,
    /// Command that reverts the edit.
    undo: Command,
    coalesce_key: Option<String>,
    at: Instant,
    /// Wall-clock time the edit was made (Undo History's Date and Time).
    made: SystemTime,
}

/// Where an edit came from; used for logging, AI attribution and tests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    User,
    Script,
    Assistant,
}

/// Owns the authoritative project and its undo history.
#[derive(Debug)]
pub struct Document {
    project: Project,
    undo: Vec<Entry>,
    redo: Vec<Entry>,
    /// Incremented on every change, including undo/redo. Consumers (engine
    /// snapshot publisher, UI) compare revisions to detect changes.
    revision: u64,
    saved_revision: u64,
    /// Changed in a way that is not an undo step (a plugin's own editor): unsaved until saved.
    edited_outside: bool,
    pub coalesce_window: Duration,
    pub max_history: usize,
    /// A gesture holds the last entry open ([`hold_coalescing`](Self::hold_coalescing)): a
    /// command with its coalesce key merges into it however long the pause since the last one.
    held: bool,
}

impl Document {
    pub fn new(project: Project) -> Self {
        Document {
            project,
            undo: Vec::new(),
            redo: Vec::new(),
            revision: 0,
            saved_revision: 0,
            edited_outside: false,
            coalesce_window: Duration::from_millis(750),
            max_history: 1000,
            held: false,
        }
    }

    pub fn project(&self) -> &Project {
        &self.project
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn is_dirty(&self) -> bool {
        self.revision != self.saved_revision || self.edited_outside
    }

    pub fn mark_saved(&mut self) {
        self.saved_revision = self.revision;
        self.edited_outside = false;
    }

    /// Something the project saves changed outside the undo history (a parameter moved in a
    /// plugin's own window): the project has unsaved changes, with no revision change.
    pub fn mark_edited(&mut self) {
        self.edited_outside = true;
    }

    /// Validate and execute a command, recording it for undo.
    pub fn execute(&mut self, cmd: Command, _origin: Origin) -> Result<(), EditError> {
        self.execute_at(cmd, Instant::now())
    }

    /// Like [`execute`](Self::execute) with an explicit timestamp (deterministic tests).
    pub fn execute_at(&mut self, cmd: Command, now: Instant) -> Result<(), EditError> {
        let mut cmd = cmd;
        let saved_next_id = self.project.next_id;
        // Resolve "allocate me" ids (0) up front so the stored redo command
        // reproduces exactly the same ids, including inside batches.
        assign_ids(&mut cmd, &mut self.project);
        let label = cmd.label();
        let key = cmd.coalesce_key();
        let redo = cmd.clone();
        let undo = match cmd.apply(&mut self.project) {
            Ok(u) => u,
            Err(e) => {
                self.project.next_id = saved_next_id;
                return Err(e);
            }
        };
        self.revision += 1;
        self.redo.clear();

        if let (Some(k), Some(last)) = (&key, self.undo.last_mut())
            && last.coalesce_key.as_deref() == Some(k.as_str())
            && (self.held || now.duration_since(last.at) <= self.coalesce_window)
        {
            // Keep the original undo, update redo to the newest value.
            last.redo = redo;
            last.at = now;
            return Ok(());
        }
        self.held = false;
        self.undo.push(Entry { label, redo, undo, coalesce_key: key, at: now, made: SystemTime::now() });
        if self.undo.len() > self.max_history {
            self.undo.remove(0);
        }
        Ok(())
    }

    /// Hold the last undo entry open for the gesture that made it (a knob drag): later commands
    /// with its coalesce key merge into it even after a pause longer than the coalescing window,
    /// until [`seal_coalescing`](Self::seal_coalescing) ends the gesture or another entry is made.
    pub fn hold_coalescing(&mut self) {
        self.held = true;
    }

    /// Mark the end of a gesture: the next command never merges into the
    /// current last undo entry, whatever its coalesce key or timing.
    pub fn seal_coalescing(&mut self) {
        self.held = false;
        if let Some(last) = self.undo.last_mut() {
            last.coalesce_key = None;
        }
    }

    /// Store plugin states pulled from the engine (before saving). Not an
    /// undo step and not a revision change: the engine already has the state.
    pub fn sync_device_states(&mut self, states: &[(crate::device::DeviceId, String)]) {
        for (id, st) in states {
            for t in &mut self.project.tracks {
                for d in t.devices.iter_mut().chain(t.instrument_plugin.iter_mut()) {
                    if d.id == *id {
                        d.state = Some(st.clone());
                    }
                }
            }
        }
    }

    /// Name the project after where it is saved (Save, Save As). Not an undo step and not a
    /// change to save: it follows the project folder's name.
    pub fn set_project_name(&mut self, name: &str) {
        if self.project.name != name {
            self.project.name = name.to_string();
        }
    }

    /// The project folder moved from `from` to `to` (renamed from the title bar): media inside
    /// it follows. Not an undo step and not a change to save: the saved relative paths are
    /// unchanged; only the absolute paths the open project uses move with the folder.
    pub fn relocate_media(&mut self, from: &std::path::Path, to: &std::path::Path) {
        self.rewrite_asset_paths(&mut |path: &mut String| {
            if let Ok(rest) = std::path::Path::new(path.as_str()).strip_prefix(from) {
                *path = to.join(rest).to_string_lossy().into_owned();
            }
        });
    }

    /// One media file moved from `from` to `to` (Save moving unsaved recordings into the
    /// project). Not an undo step: the project's assets and every Import Audio step in the
    /// undo history follow, so undoing and redoing a recording finds the file where it is now.
    pub fn relocate_asset_file(&mut self, from: &str, to: &str) {
        self.rewrite_asset_paths(&mut |path: &mut String| {
            if path == from {
                *path = to.to_owned();
            }
        });
    }

    fn rewrite_asset_paths(&mut self, f: &mut dyn FnMut(&mut String)) {
        fn walk(cmd: &mut Command, f: &mut dyn FnMut(&mut String)) {
            match cmd {
                Command::AddAsset { asset } => f(&mut asset.absolute_path),
                Command::Batch { commands, .. } => commands.iter_mut().for_each(|c| walk(c, f)),
                _ => {}
            }
        }
        for a in &mut self.project.assets {
            f(&mut a.absolute_path);
        }
        for e in self.undo.iter_mut().chain(self.redo.iter_mut()) {
            walk(&mut e.redo, f);
            walk(&mut e.undo, f);
        }
    }

    /// Record the modulation bases of plugin devices before saving (no undo
    /// step, like [Self::sync_device_states]); devices not listed get none.
    pub fn sync_mod_bases(&mut self, bases: &[(crate::device::DeviceId, Vec<(u32, f32)>)]) {
        for t in &mut self.project.tracks {
            for d in t.devices.iter_mut().chain(t.instrument_plugin.iter_mut()) {
                d.mod_bases = bases.iter().find(|(id, _)| *id == d.id).map(|(_, b)| b.clone()).unwrap_or_default();
            }
        }
    }

    /// Remember the bottom panel height for a panel kind (splitter drag). Saved with the
    /// project but not an undo step.
    /// Track row height scale (1.0 = default). A view change: saved with
    /// the project, never an undo step.
    pub fn set_track_height(&mut self, track: TrackId, height: f32) -> Result<(), EditError> {
        let (lo, hi) = crate::Track::HEIGHT_RANGE;
        if !(lo..=hi).contains(&height) {
            return Err(EditError::InvalidValue { field: "height", reason: format!("{height} outside {lo}..{hi}") });
        }
        let t = self.project.track_mut(track).ok_or(EditError::TrackNotFound(track))?;
        if t.height != height {
            t.height = height;
            self.revision += 1;
        }
        Ok(())
    }

    pub fn set_bottom_panel_height(&mut self, kind: &str, h: Option<f32>) {
        let map = &mut self.project.bottom_panel_heights;
        let changed = match h {
            Some(h) => map.insert(kind.to_string(), h) != Some(h),
            None => map.remove(kind).is_some(),
        };
        if changed {
            self.revision += 1;
        }
    }

    pub fn undo_depth(&self) -> usize {
        self.undo.len()
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    pub fn undo_label(&self) -> Option<&str> {
        self.undo.last().map(|e| e.label.as_str())
    }

    pub fn redo_label(&self) -> Option<&str> {
        self.redo.last().map(|e| e.label.as_str())
    }

    pub fn undo(&mut self) -> Result<bool, EditError> {
        self.held = false;
        let Some(mut e) = self.undo.pop() else { return Ok(false) };
        let redo = match e.undo.clone().apply(&mut self.project) {
            Ok(redo) => redo,
            Err(err) => {
                self.undo.push(e);
                return Err(err);
            }
        };
        e.redo = redo;
        e.coalesce_key = None; // never coalesce into an entry that was undone
        self.revision += 1;
        self.redo.push(e);
        Ok(true)
    }

    pub fn redo(&mut self) -> Result<bool, EditError> {
        self.held = false;
        let Some(mut e) = self.redo.pop() else { return Ok(false) };
        let undo = match e.redo.clone().apply(&mut self.project) {
            Ok(undo) => undo,
            Err(err) => {
                self.redo.push(e);
                return Err(err);
            }
        };
        e.undo = undo;
        self.revision += 1;
        self.undo.push(e);
        Ok(true)
    }

    /// Labels of the undo stack, oldest first (for an "Undo History" view).
    pub fn history_labels(&self) -> Vec<String> {
        self.undo.iter().map(|e| e.label.clone()).collect()
    }

    /// Every step Edit › Undo History lists, oldest first: the undo stack, then the redo stack
    /// with the next redo first. The first `undo_depth()` of them are applied.
    pub fn history_steps(&self) -> Vec<(&str, SystemTime)> {
        self.undo.iter().chain(self.redo.iter().rev()).map(|e| (e.label.as_str(), e.made)).collect()
    }
}

/// An edit label as shown in Undo / Redo: sentence case ("Track Volume Change" →
/// "Track volume change"). Words written in capitals (MIDI, EQ, LFO) keep them, also inside
/// hyphenated words ("MIDI-to-Audio" → "MIDI-to-audio").
pub fn sentence_case(label: &str) -> String {
    let mut first = true;
    label
        .split(' ')
        .map(|w| {
            w.split('-')
                .map(|part| {
                    let acronym = part.chars().filter(|c| c.is_alphabetic()).count() > 1 && !part.chars().any(|c| c.is_lowercase());
                    let keep = std::mem::take(&mut first) || acronym;
                    if keep { part.to_string() } else { part.to_lowercase() }
                })
                .collect::<Vec<_>>()
                .join("-")
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Replace zero ids with freshly allocated ones, recursively.
pub fn assign_ids(cmd: &mut Command, p: &mut Project) {
    use crate::model::{AssetId, ClipId, TrackId};
    match cmd {
        Command::AddTrack { track, .. } => {
            if track.id.0 == 0 {
                track.id = TrackId(p.alloc_id());
            }
            for c in &mut track.clips {
                if c.id.0 == 0 {
                    c.id = ClipId(p.alloc_id());
                }
            }
        }
        Command::AddClip { clip, .. } if clip.id.0 == 0 => clip.id = ClipId(p.alloc_id()),
        Command::AddMidiClip { clip, .. } if clip.id.0 == 0 => clip.id = ClipId(p.alloc_id()),
        Command::AddDevice { device, .. } if device.id.0 == 0 => device.id = crate::device::DeviceId(p.alloc_id()),
        Command::SetInstrumentPlugin { device: Some(device), .. } if device.id.0 == 0 => device.id = crate::device::DeviceId(p.alloc_id()),
        Command::AddAsset { asset } if asset.id.0 == 0 => asset.id = AssetId(p.alloc_id()),
        Command::Batch { commands, .. } => commands.iter_mut().for_each(|c| assign_ids(c, p)),
        _ => {}
    }
}

#[cfg(test)]
mod sentence_case_tests {
    #[test]
    fn labels_read_in_sentence_case() {
        use super::sentence_case;
        assert_eq!(sentence_case("Track Volume Change"), "Track volume change");
        assert_eq!(sentence_case("Insert MIDI Clip"), "Insert MIDI clip");
        assert_eq!(sentence_case("Move Effect into Chain"), "Move effect into chain");
        assert_eq!(sentence_case("Use Built-in Synth"), "Use built-in synth");
        assert_eq!(sentence_case("Mute"), "Mute");
        assert_eq!(sentence_case("Convert MIDI-to-Audio"), "Convert MIDI-to-audio");
        assert_eq!(sentence_case("Re-Pitch Clip"), "Re-pitch clip");
    }
}

#[cfg(test)]
mod relocate_tests {
    use super::{Document, Origin};
    use crate::command::Command;
    use crate::model::{Asset, AssetId, Project};

    fn asset(path: &str) -> Asset {
        Asset { id: AssetId(0), relative_path: None, absolute_path: path.into(), sample_rate: 48_000.0, channels: 1, frames: 10 }
    }

    #[test]
    fn a_moved_take_is_found_after_undo_and_redo() {
        let mut doc = Document::new(Project::default());
        doc.execute(Command::AddAsset { asset: asset("/tmp/Unsaved recordings/take.wav") }, Origin::User).unwrap();
        doc.relocate_asset_file("/tmp/Unsaved recordings/take.wav", "/songs/a.dsproj/media/take.wav");
        assert_eq!(doc.project().assets[0].absolute_path, "/songs/a.dsproj/media/take.wav");
        doc.undo().unwrap();
        assert!(doc.project().assets.is_empty());
        doc.redo().unwrap();
        assert_eq!(doc.project().assets[0].absolute_path, "/songs/a.dsproj/media/take.wav", "redo must not bring back the temp path");
    }
}

#[cfg(test)]
mod dirty_tests {
    use super::Document;
    use crate::model::Project;

    #[test]
    fn an_edit_outside_the_history_is_unsaved_until_saved() {
        let mut doc = Document::new(Project::default());
        assert!(!doc.is_dirty());
        let rev = doc.revision();
        doc.mark_edited();
        assert!(doc.is_dirty(), "a plugin-window change must count as unsaved");
        assert_eq!(doc.revision(), rev, "no revision change: nothing to republish");
        doc.mark_saved();
        assert!(!doc.is_dirty());
    }
}
