//! Clip-level editing: insert MIDI clip (⌘⇧M), duplicate (⌘D), the internal
//! clipboard (⌘X / ⌘C / ⌘V), rename, the clip context menu, and pushing the
//! Clip tab / piano-roll view models to Slint. Every edit is one Command.

use crate::controller::{BottomTab, ClipboardItem, Controller, MenuAction, sync_model, track_color};
use crate::pianoroll::{self, GRID_Y, ROW_H};
use crate::{MenuItem, PatternRow, PreviewNote, RulerLabel, SeqCell, SeqNum, SeqRow, ShapeRow, Studio, YLabel};
use dstudio_core::midi::{self, MidiClip};
use dstudio_core::steps::StepPattern;
use dstudio_core::{ClipId, Command, PPQ, Ticks, TrackId, TrackKind};

/// Longest gap between the two clicks of a double-click in the arrangement lanes (the macOS default).
const LANE_DOUBLE_CLICK: std::time::Duration = std::time::Duration::from_millis(500);

/// Context-menu geometry (window points).
pub const MENU_W: f32 = 209.0;
pub const MENU_PAD: f32 = 3.4;
pub const MENU_ROW: f32 = 20.4;
pub const MENU_SEP: f32 = 10.6;

/// Clip editor dropdowns, in `Studio.clip-menu` order: Speed, Warp, Mode (audio toolbar),
/// Grid (piano roll footer), Type, Genre (clip sidebar). Speed, Warp and Mode edit the clip's
/// warp (`dstudio_core::warp`, presets and modes in the same order); Type and Genre pick the
/// sidebar's library part (dstudio_core::library::GENRES has a feel for each).
/// Genre lists a fixed set of entries.
const CLIP_MENUS: [&[&str]; 10] = [
    &["0.5x", "0.75x", "1x", "1.25x", "1.5x", "2x"],
    &["On", "Off"],
    &["Auto", "Drums", "Melody", "Vocal", "Re-pitch"],
    &["1/4", "1/8", "1/16", "1/32", "1/4T", "1/8T", "1/16T", "1/32T"],
    &["Chords", "Bass", "Lead", "Arp"],
    &dstudio_core::library::GENRES,
    &["8", "12", "16", "24", "32", "36", "48", "64"],
    &["1/1", "1/2", "1/4", "1/8", "1/16", "1/32", "1/8T", "1/16T", "1/32T"],
    &dstudio_core::scale::ROOTS,
    &dstudio_core::scale::SCALES,
];
const GRID_DENOMS: [u32; 8] = [4, 8, 16, 32, 6, 12, 24, 48];

fn grid_label(denominator: u32) -> String {
    GRID_DENOMS.iter().position(|&d| d == denominator).map_or(format!("1/{denominator}"), |i| CLIP_MENUS[3][i].to_owned())
}

fn slint_color(c: [f32; 3]) -> slint::Color {
    slint::Color::from_rgb_f32(c[0], c[1], c[2])
}

impl MenuAction {
    pub(crate) fn label(self) -> &'static str {
        match self {
            MenuAction::Cut => "Cut",
            MenuAction::Copy => "Copy",
            MenuAction::Duplicate => "Duplicate",
            MenuAction::Split => "Split",
            MenuAction::Consolidate => "Consolidate",
            MenuAction::Bounce => "Bounce",
            MenuAction::Takes => "Takes",
            MenuAction::Delete => "Delete",
            MenuAction::Disable => "Disable",
            MenuAction::Enable => "Enable",
            MenuAction::ShowInFinder => "Show in Finder",
            MenuAction::Rename => "Rename…",
            MenuAction::Resample => "Resample",
            MenuAction::SendToSampler => "Send to Sampler",
            MenuAction::ExportMidi => "Export MIDI",
            MenuAction::Export => "Export audio to Finder",
            MenuAction::Transpose => "Transpose",
            MenuAction::Quantize => "Quantize",
            MenuAction::Humanize => "Humanize",
            MenuAction::ToPattern => "Convert to Step Pattern",
            MenuAction::ToMidi => "Convert to MIDI Clip",
            MenuAction::Ask => "Ask",
            MenuAction::Paste => "Paste",
            MenuAction::InsertMidiClip => "Insert MIDI clip",
            MenuAction::SectionRename => "Rename…",
            MenuAction::SectionColor => "Color",
            MenuAction::SectionInsert => "Insert",
            MenuAction::SectionDelete => "Delete section",
            MenuAction::SectionClear => "Clear all sections",
            MenuAction::TrackRename => "Rename…",
            MenuAction::TrackDuplicate => "Duplicate track",
            MenuAction::TrackDuplicateClips => "Duplicate track with clips",
            MenuAction::TrackColor => "Color",
            MenuAction::TrackDelete => "Delete track",
            MenuAction::TrackGroup => "Group track",
            MenuAction::TrackUngroup => "Ungroup",
            MenuAction::CountInOff => "No count-in",
            MenuAction::CountIn1 => "Count-in: 1 bar",
            MenuAction::CountIn2 => "Count-in: 2 bars",
        }
    }
    pub(crate) fn shortcut(self) -> &'static str {
        match self {
            MenuAction::Cut => "⌘ X",
            MenuAction::Copy => "⌘ C",
            MenuAction::Duplicate => "⌘ D",
            MenuAction::Split => "⌘ E",
            MenuAction::Consolidate => "⌘ J",
            MenuAction::Delete => "Del",
            MenuAction::Disable | MenuAction::Enable => "0",
            MenuAction::Ask => "⌘ K",
            MenuAction::Paste => "⌘ V",
            MenuAction::InsertMidiClip => "⇧⌘ M",
            MenuAction::SectionDelete | MenuAction::TrackDelete => "Del",
            MenuAction::TrackRename => "⌘ R",
            MenuAction::TrackGroup => "⌘ G",
            _ => "",
        }
    }
    /// A separator is drawn above these entries.
    fn separated(self) -> bool {
        matches!(
            self,
            MenuAction::Delete
                | MenuAction::Disable
                | MenuAction::Enable
                | MenuAction::Resample
                | MenuAction::ShowInFinder
                | MenuAction::Transpose
                | MenuAction::Ask
                | MenuAction::SectionRename
                | MenuAction::SectionDelete
                | MenuAction::SectionClear
                | MenuAction::TrackDuplicate
                | MenuAction::TrackDelete
        )
    }
    fn submenu(self) -> bool {
        matches!(
            self,
            MenuAction::Bounce
                | MenuAction::Takes
                | MenuAction::Transpose
                | MenuAction::SectionColor
                | MenuAction::SectionInsert
                | MenuAction::TrackColor
        )
    }
}

/// Takes submenu rows: "Take 1", "Take 2", ... with a tick on the take that plays.
fn take_rows(clip: Option<&dstudio_core::Clip>) -> Vec<(String, String, bool)> {
    let Some(c) = clip else { return Vec::new() };
    let cur = c.current_take();
    (0..c.takes.len()).map(|i| (format!("Take {}", i + 1), if cur == Some(i) { "✓".into() } else { String::new() }, true)).collect()
}

/// Bounce submenu: (label, shortcut text, enabled). Two entries;
/// DStudio builds Bounce to new track. Bounce track in place needs a track that changes kind (MIDI to
/// audio, its instrument and devices removed), which the model cannot do yet, so it stays greyed.
const BOUNCE: [(&str, &str, bool); 2] = [("Bounce to new track", "⌘ B", true), ("Bounce track in place", "", false)];

/// Transpose submenu: (label, shortcut text, semitones).
const TRANSPOSE: [(&str, &str, i32); 4] =
    [("Octave up", "+12", 12), ("Semitone up", "+1", 1), ("Semitone down", "−1", -1), ("Octave down", "−12", -12)];

pub(crate) fn layout_items(items: impl Iterator<Item = (String, String, bool, bool, bool)>) -> (Vec<MenuItem>, f32) {
    let mut y = MENU_PAD;
    let mut out = Vec::new();
    for (i, (label, shortcut, enabled, separator, submenu)) in items.enumerate() {
        let separator = separator && i > 0;
        if separator {
            y += MENU_SEP;
        }
        out.push(MenuItem { swatch: Default::default(), label: label.into(), shortcut: shortcut.into(), enabled, separator, submenu, y });
        y += MENU_ROW;
    }
    (out, y + MENU_PAD)
}

impl Controller {
    // ---------------------------------------------------------------- clip commands

    /// ⌘⇧M: 1-bar empty MIDI clip at the playhead (snapped to the beat) on the
    /// selected instrument track. Opens the Clip tab (the clip is not selected). When the
    /// playhead is inside a clip, the new clip goes straight after it rather than on top, and it
    /// ends early rather than running into the next clip.
    pub fn insert_midi_clip(&mut self) -> Option<ClipId> {
        let p = self.project();
        let Some(t) = self.selected_track.and_then(|t| p.track(t)) else {
            self.set_status("Select an instrument track to insert a MIDI clip", 3);
            return None;
        };
        if t.kind != TrackKind::Instrument {
            self.set_status("MIDI clips go on instrument tracks", 3);
            return None;
        }
        let track = t.id;
        let at = ((self.playhead_ticks() as f64 / PPQ as f64).round() as Ticks * PPQ).max(0);
        let Some((start, max_len)) = self.free_span(track, at, at) else {
            self.set_status("No room for a clip at the playhead", 3);
            return None;
        };
        let id = self.add_empty_midi_clip(track, start, max_len)?;
        self.set_bottom_tab(BottomTab::Clip);
        Some(id)
    }

    /// An empty one-bar MIDI clip on instrument track `track` at `start` (a one-bar drum
    /// pattern on a Drum Rack track), cut short to `max_len` when given. One undo step.
    fn add_empty_midi_clip(&mut self, track: TrackId, start: Ticks, max_len: Option<Ticks>) -> Option<ClipId> {
        let p = self.project();
        let t = p.track(track)?;
        let rack = t.instrument_plugin.as_ref().is_some_and(crate::stepseq::is_drum_rack);
        let id = ClipId(p.next_id);
        let mut clip = if rack {
            MidiClip::empty_pattern(start, dstudio_core::steps::StepRows::Pads)
        } else {
            MidiClip::empty(start, p.ticks_per_bar())
        };
        if let Some(max) = max_len.filter(|&m| m > 0 && m < clip.length) {
            // A pattern's length is its steps; only a plain clip is cut short.
            if clip.pattern.is_none() {
                clip.length = max;
                clip.clip_loop = Some(midi::ClipLoop::whole(max));
            }
        }
        clip.id = id;
        self.execute(Command::AddMidiClip { track, clip }).then_some(id)
    }

    /// A press in the arrangement lanes: when it is the second press of a double-click (quick and
    /// near the first click), runs `lane_double` and returns whether that did something, in which
    /// case the press does nothing else and the playhead goes back to where it was before the
    /// first click. The gap is timed from when the first click's release was
    /// handled (`lane_release`), not from its press: the first click often selects a track, and
    /// rebuilding the panels for it can take longer than a double-click on a slow machine, which
    /// is also why this does not use Slint's own double-click.
    pub fn lane_press_double(&mut self, x: f32, y: f32) -> bool {
        let now = std::time::Instant::now();
        let double =
            self.lane_last_down.is_some_and(|(t, lx, ly)| now - t < LANE_DOUBLE_CLICK && (lx - x).abs() < 4.0 && (ly - y).abs() < 4.0);
        if !double {
            self.lane_last_down = Some((now, x, y));
            self.lane_playhead_before = self.playhead_state();
            self.lane_clip_before = self.selected_clip;
            return false;
        }
        self.lane_last_down = None;
        let done = self.lane_double(x, y);
        // The first click of the pair moved the playhead to the pointer; making or renaming
        // something is not a seek, so it goes back.
        if let Some(st) = self.lane_playhead_before.take().filter(|_| done) {
            self.restore_playhead(st);
        }
        // A press in the section lane also let go of the open clip; renaming a section is not a
        // reason to close it in the Clip tab.
        let clip = self.lane_clip_before.take();
        if done && self.rename_section.is_some() && self.selected_clip.is_none() && clip.is_some_and(|c| self.clip_track(c).is_some()) {
            self.select_clip(clip);
        }
        done
    }

    /// The release of a click in the arrangement lanes: restarts the double-click timer when it is
    /// the release of the press `lane_press_double` noted.
    pub fn lane_release(&mut self, x: f32, y: f32) {
        if let Some((t, lx, ly)) = self.lane_last_down.as_mut()
            && (*lx - x).abs() < 4.0
            && (*ly - y).abs() < 4.0
        {
            *t = std::time::Instant::now();
        }
    }

    /// Double-click in the arrangement. In the section lane it renames or inserts a section;
    /// on an empty spot of an instrument track's lane it makes a new one-bar MIDI clip there
    /// (`create_midi_clip_at`); on a clip it opens that clip in the Clip tab. Returns whether
    /// the double-click did something.
    pub fn lane_double(&mut self, x: f32, y: f32) -> bool {
        use crate::timeline::{Hit, hit_test};
        if self.section_double(x, y) {
            return true;
        }
        if self.auto.shown {
            return false;
        }
        match hit_test(&self.view, self.lanes_size, self.lane_tracks(), &self.clip_vis, x, y) {
            Hit::Track(i) => {
                let Some(t) = self.project().tracks.get(i) else { return false };
                if t.kind != TrackKind::Instrument {
                    self.set_status("MIDI clips go on instrument tracks: drop an audio file here, or record", 3);
                    return false;
                }
                let track = t.id;
                let pointer = (self.view.beat_at(x).max(0.0) * PPQ as f64).round() as Ticks;
                self.create_midi_clip_at(track, pointer).is_some()
            }
            Hit::Clip { index, .. } => {
                let Some(&id) = self.clip_ids.get(index) else { return false };
                self.select_clip(Some(id));
                self.set_bottom_tab(BottomTab::Clip);
                true
            }
            _ => false,
        }
    }

    /// A new one-bar MIDI clip on instrument track `track` in the bar under `pointer` (on the
    /// beat while snapping is off or bypassed), moved past a clip it would start inside and
    /// ending early rather than running into the next one. The clip is selected and opened in
    /// the Clip tab so notes can be drawn straight away.
    pub fn create_midi_clip_at(&mut self, track: TrackId, pointer: Ticks) -> Option<ClipId> {
        let at = self.lane_floor(pointer);
        let Some((start, max_len)) = self.free_span(track, at, pointer) else {
            self.set_status("No room for a clip here", 3);
            return None;
        };
        let id = self.add_empty_midi_clip(track, start, max_len)?;
        self.select_track(track);
        self.select_clip(Some(id));
        self.set_bottom_tab(BottomTab::Clip);
        Some(id)
    }

    /// The start of the bar at `pointer` (of the beat while snapping is off or bypassed): where
    /// clips made or pasted from the lane itself go.
    fn lane_floor(&self, pointer: Ticks) -> Ticks {
        let step = if self.snap_active() { self.project().ticks_per_bar() } else { PPQ };
        pointer.max(0).div_euclid(step.max(1)) * step
    }

    /// ⌘⇧M and Create › MIDI Clip: insert at the playhead and open the clip in the piano roll.
    pub fn insert_and_open_midi_clip(&mut self) -> Option<ClipId> {
        let id = self.insert_midi_clip()?;
        self.select_clip(Some(id));
        Some(id)
    }

    /// Right-click on an empty spot of track row `i`: Insert MIDI clip (instrument tracks) and
    /// Paste (when the copied clip fits the track), both acting at that spot.
    fn open_lane_menu(&mut self, i: usize, x: f32, wx: f32, wy: f32, win: (f32, f32)) {
        let Some(t) = self.project().tracks.get(i) else {
            self.menu_close();
            return;
        };
        let (track, kind) = (t.id, t.kind);
        let paste = match &self.clipboard {
            Some(ClipboardItem::Audio { .. }) => kind == TrackKind::Audio,
            Some(ClipboardItem::Midi { .. }) => kind == TrackKind::Instrument,
            None => false,
        };
        self.select_track(track);
        let pointer = (self.view.beat_at(x).max(0.0) * PPQ as f64).round() as Ticks;
        self.open_menu(vec![(MenuAction::InsertMidiClip, kind == TrackKind::Instrument), (MenuAction::Paste, paste)], wx, wy, win);
        self.menu.lane = Some((track, pointer));
    }

    /// Where a new clip wanted at `at` can go on `track`, given the pointer was at `pointer`
    /// (in the gap between clips, or `at` itself for the playhead): the start moved past the clips
    /// that cover it, one after another, and the room left before the next clip (`None`: no clip
    /// after it). `None` when there is no room.
    fn free_span(&self, track: TrackId, at: Ticks, pointer: Ticks) -> Option<(Ticks, Option<Ticks>)> {
        let p = self.project();
        let t = p.track(track)?;
        let spans: Vec<(Ticks, Ticks)> = t
            .midi_clips
            .iter()
            .map(|c| (c.start, c.end()))
            .chain(t.clips.iter().map(|c| (c.start, c.start + p.clip_length_ticks(c))))
            .collect();
        let mut start = at;
        while let Some(end) = spans.iter().filter(|&&(s, e)| s <= start.max(pointer) && e > start).map(|&(_, e)| e).max() {
            start = end;
        }
        let next = spans.iter().filter(|&&(s, _)| s >= start).map(|&(s, _)| s).min();
        match next {
            Some(n) if n <= start => None,
            Some(n) => Some((start, Some(n - start))),
            None => Some((start, None)),
        }
    }

    /// ⌘R: reverse the selected clip. An audio clip plays backwards (pressing again plays it
    /// forwards); a MIDI clip's notes are mirrored in the clip, so the first note ends where the
    /// clip ends. A step pattern is left alone (its steps are reversed in the step editor).
    pub fn reverse_clip(&mut self) {
        let Some(id) = self.selected_clip else { return };
        // Each press is its own undo step: two quick reverses of one audio clip must not merge.
        self.doc.seal_coalescing();
        let p = self.project();
        if let Some(m) = p.midi_clip(id) {
            if m.pattern.is_some() {
                self.set_status("Reverse: convert the pattern to MIDI first", 3);
                return;
            }
            let notes = reversed_in_clip(&m.notes, m.length, m.clip_loop.as_ref());
            if notes != m.notes {
                self.execute(Command::Batch { label: "Reverse Clip".into(), commands: vec![Command::SetMidiNotes { clip: id, notes }] });
            }
        } else if let Some(c) = p.clip(id) {
            let warp = dstudio_core::ClipWarp { reverse: !c.warp.reverse, ..c.warp.clone() };
            self.set_clip_warp(id, warp, "Reverse Clip");
        }
    }

    /// ⌘D: copy of the selected clip right after itself (MIDI or audio). Selects the copy.
    pub fn duplicate_clip(&mut self) {
        if !self.extra_clips.is_empty() {
            return self.duplicate_clips();
        }
        let Some(id) = self.selected_clip else { return };
        let p = self.project();
        let new_id = ClipId(p.next_id);
        let cmd = if let Some((ti, ci)) = p.find_midi_clip(id) {
            let c = &p.tracks[ti].midi_clips[ci];
            Command::AddMidiClip { track: p.tracks[ti].id, clip: MidiClip { id: new_id, start: c.end(), ..c.clone() } }
        } else if let Some((ti, ci)) = p.find_clip(id) {
            let c = &p.tracks[ti].clips[ci];
            let len = p.clip_length_ticks(c);
            Command::AddClip { track: p.tracks[ti].id, clip: dstudio_core::Clip { id: new_id, start: c.start + len, ..c.clone() } }
        } else {
            return;
        };
        if let Command::AddMidiClip { .. } | Command::AddClip { .. } = cmd
            && self.execute(Command::Batch { label: "Duplicate Clip".into(), commands: vec![cmd] })
        {
            self.select_clip(Some(new_id));
        }
    }

    /// Whether the playhead lies inside clip id, which can be split there (audio
    /// clips that are not warped or reversed, and MIDI clips that are not patterns).
    pub(crate) fn can_split(&self, id: ClipId) -> bool {
        let p = self.project();
        let at = self.playhead_ticks();
        if let Some((ti, ci)) = p.find_clip(id) {
            let c = &p.tracks[ti].clips[ci];
            return !c.warp.enabled && !c.warp.reverse && at > c.start && at < c.start + p.clip_length_ticks(c);
        }
        p.midi_clip(id).is_some_and(|c| c.pattern.is_none() && at > c.start && at < c.end())
    }

    /// ⌘E: cut the selected clip in two at the playhead and select the right part.
    pub fn split_clip(&mut self) {
        let Some(id) = self.selected_clip else { return };
        if !self.can_split(id) {
            self.set_status("Move the playhead inside an audio clip (no warp or reverse) or a MIDI clip to split it", 3);
            return;
        }
        let new_id = ClipId(self.project().next_id);
        let at = self.playhead_ticks();
        if self.execute(Command::SplitClip { clip: id, at, new_id }) {
            self.select_clip(Some(new_id));
        }
    }

    /// 0: disable the selected clip (silent, stays on the timeline), or enable it again.
    pub fn toggle_clip_disabled(&mut self) {
        if !self.extra_clips.is_empty() {
            return self.toggle_clips_disabled();
        }
        let Some(id) = self.selected_clip else { return };
        let p = self.project();
        let disabled = p.find_clip(id).map(|(t, c)| p.tracks[t].clips[c].disabled).or_else(|| p.midi_clip(id).map(|c| c.disabled));
        if let Some(disabled) = disabled {
            self.execute(Command::SetClipEnabled { clip: id, enabled: disabled });
        }
    }

    /// Reveal the selected audio clip's source file in Finder.
    pub fn show_clip_in_finder(&mut self) {
        let Some(id) = self.selected_clip else { return };
        let p = self.project();
        let Some(path) = p.find_clip(id).and_then(|(t, c)| p.asset(p.tracks[t].clips[c].asset)).map(|a| a.absolute_path.clone()) else {
            return;
        };
        if !std::path::Path::new(&path).exists() {
            self.set_status("The audio file is missing", 3);
            return;
        }
        if let Err(e) = crate::clip_render::reveal(&path) {
            self.set_status(format!("Show in Finder failed: {e}"), 3);
        }
    }

    /// ⌘C: the selected clips (the focused one first) to the clipboard.
    pub fn copy_clip(&mut self) -> bool {
        let mut items = self.copy_items().into_iter();
        let Some(first) = items.next() else { return false };
        self.clipboard = Some(first);
        self.clipboard_rest = items.collect();
        true
    }

    pub fn cut_clip(&mut self) {
        if self.copy_clip() {
            self.delete_selection();
        }
    }

    /// ⌘V: paste at the playhead (beat-snapped) on the selected track when its kind
    /// matches, else on the clip's original track.
    pub fn paste_clip(&mut self) {
        self.paste_clip_at(None);
    }

    /// Paste on `target` (track, position) when it can take the clip, else on the selected track
    /// at the playhead (snapped to the beat), else on the track the clip was copied from.
    pub fn paste_clip_at(&mut self, target: Option<(TrackId, Ticks)>) {
        let Some(item) = self.clipboard.clone() else { return };
        let p = self.project();
        let at = target.map_or_else(|| ((self.playhead_ticks() as f64 / PPQ as f64).round() as Ticks * PPQ).max(0), |(_, at)| at);
        let new_id = ClipId(p.next_id);
        let (kind, orig) = match &item {
            ClipboardItem::Audio { track, .. } => (TrackKind::Audio, *track),
            ClipboardItem::Midi { track, .. } => (TrackKind::Instrument, *track),
        };
        let track = target
            .map(|(t, _)| t)
            .or(self.selected_track)
            .and_then(|t| p.track(t))
            .filter(|t| t.kind == kind)
            .map(|t| t.id)
            .or_else(|| p.track(orig).map(|t| t.id));
        let Some(track) = track else {
            self.set_status("Paste: no matching track", 3);
            return;
        };
        let (cmd, dt) = match item {
            ClipboardItem::Audio { clip, .. } => {
                let dt = at - clip.start;
                (Command::AddClip { track, clip: dstudio_core::Clip { id: new_id, start: at, ..clip } }, dt)
            }
            ClipboardItem::Midi { clip, .. } => {
                let dt = at - clip.start;
                (Command::AddMidiClip { track, clip: MidiClip { id: new_id, start: at, ..clip } }, dt)
            }
        };
        // The rest of a copied group lands on its own tracks, as far from the first as before.
        let (rest, rest_ids) = self.paste_rest(dt, new_id.0 + 1);
        let label = if rest.is_empty() { "Paste Clip".to_string() } else { format!("Paste {} Clips", rest.len() + 1) };
        if self.execute(Command::Batch { label, commands: std::iter::once(cmd).chain(rest).collect() }) {
            self.set_clip_selection(std::iter::once(new_id).chain(rest_ids).collect());
        }
    }

    /// Rename = remove + re-add with the same id in one Batch (core has no RenameClip yet).
    pub fn rename_clip(&mut self, id: ClipId, name: &str) {
        let name = name.trim();
        self.rename = None;
        self.ui_dirty = true;
        if name.is_empty() {
            return;
        }
        let p = self.project();
        let cmds = if let Some((ti, ci)) = p.find_midi_clip(id) {
            let c = &p.tracks[ti].midi_clips[ci];
            if c.name == name {
                return;
            }
            vec![
                Command::RemoveMidiClip { clip: id },
                Command::AddMidiClip { track: p.tracks[ti].id, clip: MidiClip { name: name.into(), ..c.clone() } },
            ]
        } else if let Some((ti, ci)) = p.find_clip(id) {
            let c = &p.tracks[ti].clips[ci];
            if c.name == name {
                return;
            }
            vec![
                Command::RemoveClip { clip: id },
                Command::AddClip { track: p.tracks[ti].id, clip: dstudio_core::Clip { name: name.into(), ..c.clone() } },
            ]
        } else {
            return;
        };
        self.execute(Command::Batch { label: "Rename Clip".into(), commands: cmds });
    }

    /// Whole-clip MIDI transforms from the context menu.
    fn menu_midi(&mut self, id: ClipId, f: impl FnOnce(&Self, &MidiClip) -> Option<Vec<dstudio_core::Note>>, label: &str) {
        let Some(c) = self.project().midi_clip(id).cloned() else { return };
        if c.notes.is_empty() {
            self.set_status(format!("{label}: the clip has no notes"), 2);
            return;
        }
        match f(self, &c) {
            Some(mut notes) => {
                midi::sort_notes(&mut notes);
                if notes != c.notes {
                    self.execute(Command::Batch { label: label.into(), commands: vec![Command::SetMidiNotes { clip: id, notes }] });
                }
            }
            None => self.set_status(format!("{label}: notes would leave the MIDI range"), 3),
        }
    }

    // ---------------------------------------------------------------- context menu

    /// Right-click on the lanes (lane-local x/y for hit-testing, window wx/wy for placement).
    pub fn lane_context(&mut self, x: f32, y: f32, wx: f32, wy: f32, win: (f32, f32)) {
        use crate::timeline::{Hit, hit_test};
        if self.section_context(x, y, wx, wy, win) {
            return;
        }
        let hit = hit_test(&self.view, self.lanes_size, self.lane_tracks(), &self.clip_vis, x, y);
        if let Hit::Track(i) = hit
            && !self.auto.shown
        {
            self.open_lane_menu(i, x, wx, wy, win);
            return;
        }
        let Hit::Clip { index, .. } = hit else {
            self.menu_close();
            return;
        };
        let Some(&id) = self.clip_ids.get(index) else { return };
        self.open_clip_menu(id, wx, wy, win);
    }

    /// Select clip `id` and open its context menu at window point (wx, wy).
    pub fn open_clip_menu(&mut self, id: ClipId, wx: f32, wy: f32, win: (f32, f32)) {
        self.focus_clip(id);
        if let Some(t) = self.clip_track(id) {
            self.select_track(t);
        }
        let p = self.project();
        let midi = p.find_midi_clip(id).is_some();
        let pattern = p.midi_clip(id).is_some_and(|c| c.pattern.is_some());
        let disabled =
            p.find_clip(id).map(|(t, c)| p.tracks[t].clips[c].disabled).or_else(|| p.midi_clip(id).map(|c| c.disabled)).unwrap_or(false);
        let splittable = self.can_split(id);
        use MenuAction::*;
        let takes = p.clip(id).map_or(0, |c| c.takes.len());
        let mut items = vec![
            Cut,
            Copy,
            Duplicate,
            Split,
            Takes,
            Consolidate,
            Bounce,
            Delete,
            if disabled { Enable } else { Disable },
            Rename,
            Resample,
            SendToSampler,
            ShowInFinder,
            ExportMidi,
            Export,
            Transpose,
            Quantize,
            Humanize,
        ];
        if takes < 2 {
            items.retain(|&a| a != Takes);
        }
        if midi {
            items.retain(|&a| a != Resample && a != SendToSampler);
        } else {
            items.retain(|&a| a != ExportMidi);
        }
        if midi {
            items.push(if pattern { ToMidi } else { ToPattern });
        }
        items.push(Ask);
        // PLACEHOLDER entry (disabled): Ask.
        self.menu.items = items
            .iter()
            .map(|&a| {
                let enabled = match a {
                    Cut | Copy | Duplicate | Delete | Rename | Disable | Enable | Takes => true,
                    Split => splittable,
                    ShowInFinder => !midi,
                    Consolidate => self.can_render_clip(id, false),
                    Bounce | Export | Resample => self.can_render_clip(id, true),
                    SendToSampler => !midi,
                    Transpose | Quantize | Humanize | ToPattern | ToMidi | ExportMidi => midi,
                    _ => false,
                };
                (a, enabled)
            })
            .collect();
        let items = std::mem::take(&mut self.menu.items);
        self.open_menu(items, wx, wy, win);
        self.menu.clip = Some(id);
    }

    /// Open the context menu with `items` at window point (wx, wy), kept inside the window.
    pub fn open_menu(&mut self, items: Vec<(MenuAction, bool)>, wx: f32, wy: f32, win: (f32, f32)) {
        self.menu.items = items;
        let (_, h) = layout_items(self.menu.items.iter().map(|&(a, en)| (String::new(), String::new(), en, a.separated(), false)));
        let mx = (wx + 2.0).min(win.0 - MENU_W - 4.0).max(4.0);
        let my = (wy + 2.0).min(win.1 - h - 4.0).max(4.0);
        self.menu.open = true;
        self.menu.sub_open = false;
        self.menu.pos = (mx, my);
        self.menu.clip = None;
        self.menu.section = None;
        self.menu.lane = None;
        self.menu.track = None;
        self.menu.count_in = false;
        self.ui_dirty = true;
    }

    pub fn menu_close(&mut self) {
        if self.menu.open {
            self.menu.open = false;
            self.menu.sub_open = false;
            self.menu.clip = None;
            self.menu.section = None;
            self.menu.lane = None;
            self.menu.track = None;
            self.menu.count_in = false;
            self.ui_dirty = true;
        }
    }

    pub fn menu_activate(&mut self, index: usize) {
        let Some(&(action, enabled)) = self.menu.items.get(index) else { return };
        if !enabled {
            return;
        }
        if let Some(id) = self.menu.section {
            self.section_menu_activate(id, action);
            return;
        }
        if let Some(id) = self.menu.track {
            self.track_menu_activate(id, action);
            return;
        }
        if self.menu.count_in {
            self.count_in_menu_activate(action);
            return;
        }
        if let Some((track, at)) = self.menu.lane {
            self.menu_close();
            match action {
                MenuAction::InsertMidiClip => {
                    self.create_midi_clip_at(track, at);
                }
                MenuAction::Paste => {
                    let at = self.lane_floor(at);
                    self.paste_clip_at(Some((track, at)));
                }
                _ => {}
            }
            return;
        }
        let Some(id) = self.menu.clip else { return };
        if matches!(action, MenuAction::Transpose | MenuAction::Bounce | MenuAction::Takes) {
            self.menu.sub_open = !(self.menu.sub_open && self.menu.sub_for == Some(action));
            self.menu.sub_for = Some(action);
            self.ui_dirty = true;
            return;
        }
        self.menu_close();
        self.selected_clip = Some(id);
        match action {
            MenuAction::Cut => self.cut_clip(),
            MenuAction::Copy => {
                self.copy_clip();
            }
            MenuAction::Duplicate => self.duplicate_clip(),
            MenuAction::Delete => self.delete_selection(),
            MenuAction::Split => self.split_clip(),
            MenuAction::Consolidate => self.consolidate_clip(id),
            MenuAction::Resample => self.bounce_clip(id),
            MenuAction::SendToSampler => {
                self.send_to_sampler(id);
            }
            MenuAction::ExportMidi => self.export_midi_request(id),
            MenuAction::Export => self.export_clip_request(id),
            MenuAction::Disable | MenuAction::Enable => self.toggle_clip_disabled(),
            MenuAction::ShowInFinder => self.show_clip_in_finder(),
            MenuAction::Rename => {
                self.rename = Some(id);
                self.ui_dirty = true;
            }
            MenuAction::Quantize => {
                let (g, a) = (midi::grid_ticks(self.pr.grid), self.pr.quant_amount);
                self.menu_midi(id, |_, c| Some(midi::quantize(&c.notes, g, a)), "Quantize Notes");
            }
            MenuAction::Humanize => {
                let a = self.pr.humanize_amount;
                let seed = id.0 ^ self.doc.revision();
                self.menu_midi(id, |_, c| Some(midi::humanize(&c.notes, a, seed)), "Humanize Notes");
            }
            MenuAction::ToPattern | MenuAction::ToMidi => self.convert_clip(id),
            _ => self.set_status(format!("{}: not implemented yet", action.label()), 3),
        }
    }

    /// A pattern clip becomes plain MIDI (its repeats become notes); a MIDI clip becomes a
    /// pattern read from its notes (pads on a Drum Rack track, else pitch rows).
    pub fn convert_clip(&mut self, id: ClipId) {
        let Some(c) = self.project().midi_clip(id).cloned() else { return };
        let pattern = match c.pattern {
            Some(_) => None,
            None => {
                let p = StepPattern::from_notes(&c.notes, c.length, self.seq_rows_for(&c));
                if c.length > p.length() {
                    self.set_status(format!("Too long for a step pattern: {} steps at most", dstudio_core::steps::STEP_LIMIT), 3);
                    return;
                }
                // Notes kept past the clip end would be dropped by the pattern.
                if c.notes.iter().any(|n| n.start >= p.length()) {
                    self.set_status("Notes past the clip end: lengthen the clip first", 3);
                    return;
                }
                Some(p)
            }
        };
        self.execute(Command::SetStepPattern { clip: id, pattern });
    }

    pub fn submenu_activate(&mut self, index: usize) {
        if let Some(track) = self.menu.track {
            self.track_color_pick(track, index);
            return;
        }
        let Some(id) = self.menu.clip else { return };
        if self.menu.sub_for == Some(MenuAction::Takes) {
            if self.project().clip(id).is_some_and(|c| index < c.takes.len()) {
                self.menu_close();
                self.selected_clip = Some(id);
                self.execute(Command::SetClipTake { clip: id, take: index });
            }
            return;
        }
        if self.menu.sub_for == Some(MenuAction::Bounce) {
            if BOUNCE.get(index).is_some_and(|b| b.2) {
                self.menu_close();
                self.selected_clip = Some(id);
                self.bounce_clip(id);
            }
            return;
        }
        let Some(&(_, _, semis)) = TRANSPOSE.get(index) else { return };
        self.menu_close();
        self.menu_midi(id, |_, c| midi::transpose(&c.notes, semis), "Transpose Notes");
    }

    // ---------------------------------------------------------------- view models

    /// Clip tab, piano roll, context menu and rename popup → Slint.
    /// Open (or, when it is already open, close) a clip editor dropdown. Mode is
    /// disabled with Warp off.
    pub fn clip_menu_open(&mut self, menu: i32) {
        let warped = self.ac_clip().is_some_and(|c| c.warp.enabled);
        self.pr.menu = if (0..CLIP_MENUS.len() as i32).contains(&menu)
            && (menu != 2 || warped)
            && self.pr.menu != menu
            && self.clip_menu_shown(menu)
        {
            menu
        } else {
            -1
        };
        self.ui_dirty = true;
    }

    /// The editor that owns the dropdown is on screen: the audio clip editor for Speed / Warp /
    /// Mode, the piano roll for Grid, Type and Genre.
    pub(crate) fn clip_menu_shown(&self, menu: i32) -> bool {
        self.bottom_tab == BottomTab::Clip
            && match menu {
                ..=2 => self.ac_clip().is_some(),
                6 | 7 => self.seq_visible(),
                4 => self.pr_visible() && !self.composer_is_drums(),
                _ => self.pr_visible(),
            }
    }

    fn clip_menu_active(&self, menu: i32) -> i32 {
        let warp = self.ac_clip().map(|c| c.warp.clone()).unwrap_or_default();
        match menu {
            0 => dstudio_core::warp::SPEED_PRESETS.iter().position(|s| (s - warp.effective_speed()).abs() < 0.01).map_or(-1, |i| i as i32),
            1 => {
                if warp.enabled {
                    0
                } else {
                    1
                }
            }
            2 => dstudio_core::WarpMode::ALL.iter().position(|&m| m == warp.mode).map_or(-1, |i| i as i32),
            3 => GRID_DENOMS.iter().position(|&d| d == self.pr.grid).map_or(-1, |i| i as i32),
            6 | 7 => self.seq_menu_active(menu == 7),
            4 | 5 => self.composer_menu_active(menu == 5),
            8 | 9 => self.pr_scale().map_or(-1, |(_, k)| if menu == 8 { k.root as i32 } else { k.kind as i32 }),
            _ => 0,
        }
    }

    pub fn clip_menu_pick(&mut self, index: i32) {
        let menu = std::mem::replace(&mut self.pr.menu, -1);
        self.ui_dirty = true;
        if menu == 3 {
            if let Some(&d) = GRID_DENOMS.get(index.max(0) as usize) {
                self.pr.grid = d;
            }
            return;
        }
        if menu < 0 || index < 0 || index == self.clip_menu_active(menu) {
            return;
        }
        let i = index.max(0) as usize;
        match menu {
            4 => return self.composer_type(i),
            5 => return self.composer_genre(i),
            6 => return self.seq_set_length(i),
            7 => return self.seq_set_rate(i),
            8 => return self.pr_scale_root(i),
            9 => return self.pr_scale_kind(i),
            _ => {}
        }
        match menu {
            0 => self.warp_pick_speed(i),
            1 => self.warp_pick_enabled(index == 0),
            2 => self.warp_pick_mode(i),
            _ => {}
        }
    }

    fn push_composer(&self, s: &Studio) {
        let Some(v) = self.sidebar_view() else { return };
        s.set_composer_drums(v.drums);
        s.set_composer_part_text(CLIP_MENUS[4].get(v.part as usize).copied().unwrap_or("Chords").into());
        s.set_composer_genre_text(CLIP_MENUS[5].get(v.genre as usize).copied().unwrap_or("Any").into());
        s.set_composer_original(v.original);
        s.set_composer_create_text(v.create_label.into());
        s.set_composer_shape_ok(v.shape_ok);
        s.set_composer_shape_tab(v.shape_tab);
        s.set_shape_drawing(self.shape_ui.drawing);
        s.set_shape_brush(self.shape_ui.brush as i32);
        s.set_shape_has_curves(self.shape_has_curves());
        if *self.composer_shape_shown.borrow() != v.shape_rows {
            *self.composer_shape_shown.borrow_mut() = v.shape_rows.clone();
            let rows: Vec<ShapeRow> = v
                .shape_rows
                .into_iter()
                .map(|r| {
                    let c = r.key.color();
                    ShapeRow {
                        label: r.key.label().into(),
                        value: r.value.into(),
                        kind: r.kind as i32,
                        norm: r.norm,
                        bipolar: r.bipolar,
                        segs: r.segs,
                        seg: r.seg,
                        enabled: r.enabled,
                        tip: r.tip.into(),
                        selected: r.selected,
                        curved: r.curved,
                        color: slint::Color::from_rgb_u8((c >> 16) as u8, (c >> 8) as u8, c as u8),
                    }
                })
                .collect();
            self.composer_shape_model.set_vec(rows);
        }
        if *self.composer_rows_shown.borrow() == v.rows {
            return;
        }
        *self.composer_rows_shown.borrow_mut() = v.rows.clone();
        let rows: Vec<PatternRow> = v
            .rows
            .into_iter()
            .map(|r| PatternRow {
                name: r.name.into(),
                playing: r.playing,
                notes: slint::ModelRc::new(slint::VecModel::from(
                    r.preview.into_iter().map(|(x, y, w)| PreviewNote { x, y, w }).collect::<Vec<_>>(),
                )),
            })
            .collect();
        self.composer_row_model.set_vec(rows);
    }

    fn push_step_sequencer(&self, s: &Studio) {
        s.set_seq_tabs(self.pr_clip().is_some_and(|c| c.pattern.is_some()));
        s.set_seq_visible(self.seq_visible());
        let Some(v) = self.seq_view() else { return };
        s.set_seq_lane(match self.pr.seq.lane {
            None => 0,
            Some(l) => dstudio_core::steps::StepLane::ALL.iter().position(|&a| a == l).map_or(0, |i| i as i32 + 1),
        });
        if let Some((len, rate, swing)) = self.seq_footer() {
            s.set_seq_length_text(len.into());
            s.set_seq_rate_text(rate.into());
            s.set_seq_swing_text(swing.into());
        }
        let g = v.geom;
        s.set_seq_col_pitch(g.pitch);
        s.set_seq_cell_w(g.cell_w);
        s.set_seq_cols(g.cols as i32);
        s.set_seq_row_count(g.rows as i32);
        s.set_seq_scroll_y(g.scroll);
        s.set_seq_can_double(self.seq_pattern().is_some_and(|p| p.doubled().is_some()));
        s.set_seq_can_clear(self.seq_pattern().is_some_and(|p| !p.cells.is_empty()));
        s.set_seq_page(g.page as i32);
        s.set_seq_pages(g.pages as i32);
        s.set_seq_follow(!self.pr.seq.follow_off);
        sync_model(
            &self.seq_row_model,
            v.rows
                .into_iter()
                .map(|r| SeqRow {
                    y: r.y,
                    number: r.number.into(),
                    name: r.name.into(),
                    muted: r.muted,
                    soloed: r.soloed,
                    audible: r.audible,
                })
                .collect(),
        );
        sync_model(
            &self.seq_cell_model,
            v.cells
                .into_iter()
                .map(|c| SeqCell {
                    x: c.x,
                    y: c.y,
                    w: c.w,
                    state: c.state,
                    lane: c.lane,
                    bar: c.bar,
                    marks: c.marks,
                    label: c.label.into(),
                    dim: c.dim,
                })
                .collect(),
        );
        sync_model(
            &self.seq_num_model,
            v.nums.into_iter().map(|n| SeqNum { x: n.x, text: n.text.into(), beat: n.beat, active: n.active }).collect(),
        );
    }

    pub fn push_clip_editor(&mut self, s: &Studio) {
        if self.ac_clip().is_some() {
            self.request_onsets();
            self.ensure_lane_peaks();
        }
        if self.pr.ac_edit.is_some_and(|(_, id)| self.ac_clip().map(|c| c.id) != Some(id)) {
            self.pr.ac_edit = None;
        }
        if self.pr.menu >= 0 && !self.clip_menu_shown(self.pr.menu) {
            self.pr.menu = -1;
        }
        s.set_clip_menu(self.pr.menu);
        if self.pr.menu >= 0 {
            let items: Vec<slint::SharedString> = CLIP_MENUS[self.pr.menu as usize].iter().map(|&t| t.into()).collect();
            s.set_clip_menu_items(slint::ModelRc::new(slint::VecModel::from(items)));
            s.set_clip_menu_active(self.clip_menu_active(self.pr.menu));
        }
        let p = self.doc.project();
        s.set_bottom_tab(if self.bottom_tab == BottomTab::Clip { 1 } else { 0 });
        let pr = self.pr_clip().cloned();
        s.set_pr_visible(pr.is_some());
        s.set_pr_pen(self.pr.pen);
        s.set_pr_vel_lane(self.pr.vel_lane);
        s.set_typing_on(self.typing.active);
        let (vz, hz) = self.pr_zoom_values();
        s.set_pr_vzoom(vz);
        s.set_pr_hzoom(hz);
        let ac = self.ac_clip().cloned();
        s.set_ac_visible(ac.is_some());
        // Clip header, range readouts and colour: shared by the piano roll and the audio clip editor.
        let open = match (&pr, &ac) {
            (Some(c), _) => {
                let len = self.pr.value_drag.as_ref().and_then(|d| d.length_preview()).unwrap_or(c.length);
                Some((c.name.clone(), len, p.find_midi_clip(c.id).map(|(t, _)| t)))
            }
            (None, Some(c)) => {
                s.set_ac_gain_text(format!("{:.1} dB", dstudio_core::model::gain_to_db(c.gain).max(-99.9)).into());
                s.set_ac_bpm_text(format!("{}", self.ac_original_bpm(c).round()).into());
                s.set_ac_speed_text(crate::warpedit::format_speed(c.warp.effective_speed()).into());
                s.set_ac_warped(c.warp.enabled);
                s.set_ac_mode_text(c.warp.mode.label().into());
                s.set_ac_reversed(c.warp.reverse);
                s.set_ac_transpose_text(self.ac_transpose_text(c).into());
                s.set_ac_transpose_enabled(Self::ac_transpose_editable(c));
                let edit = match self.pr.ac_edit.map(|e| e.0) {
                    Some(crate::warpedit::AcEdit::OriginalBpm) => 0,
                    Some(crate::warpedit::AcEdit::Transpose) => 1,
                    None => -1,
                };
                if edit != s.get_ac_edit_field() {
                    s.set_ac_edit_text(self.ac_edit_initial(c).into());
                    s.set_ac_edit_field(edit);
                }
                Some((c.name.clone(), p.clip_length_ticks(c), p.find_clip(c.id).map(|(t, _)| t)))
            }
            _ => None,
        };
        if let Some((name, len, ti)) = open {
            let tpb = p.ticks_per_bar();
            s.set_clip_name(name.into());
            if let Some(ti) = ti {
                let col = track_color(p.tracks[ti].color);
                s.set_clip_color(slint_color(col));
                s.set_clip_ink(slint_color(col.map(|v| v * 0.22)));
            }
            s.set_pr_start_text(pianoroll::fmt_pos(0, tpb).into());
            s.set_pr_end_text(pianoroll::fmt_pos(len, tpb).into());
            let looped = self.pr_loop_preview().filter(|l| l.on);
            s.set_pr_loop_on(looped.is_some());
            s.set_pr_loop_enabled(self.pr_clip().is_some_and(|c| c.pattern.is_none()) || ac.is_some());
            s.set_pr_position_text(pianoroll::fmt_pos(looped.map_or(0, |l| l.start), tpb).into());
            s.set_pr_length_text(pianoroll::fmt_len(looped.map_or(len, |l| l.len()), tpb).into());
            let v = self.pr_view();
            let bpb = tpb as f64 / PPQ as f64;
            let step = self.pr_steps().label_bars;
            let mut bars = Vec::new();
            let mut b = 0i64;
            loop {
                let x = v.x_of(b as f64 * bpb);
                if x > v.w || b > 4096 {
                    break;
                }
                if x >= v.gx - 2.0 {
                    bars.push(RulerLabel { x: x + 4.5, text: format!("{}", b + 1).into() });
                }
                b += step;
            }
            sync_model(&self.pr_bar_model, bars);
        }
        if let Some(c) = &pr {
            s.set_pr_has_notes(!c.notes.is_empty());
            s.set_pr_grid_text(grid_label(self.pr.grid).into());
            s.set_pr_quant_text(format!("{}%", (self.pr.quant_amount * 100.0).round()).into());
            s.set_pr_human_text(format!("{}%", (self.pr.humanize_amount * 100.0).round()).into());
            s.set_pr_vel_text(format!("{}", self.pr.velocity).into());
            let (scale, key) = self.pr_scale().unwrap_or_default();
            s.set_pr_scale_on(scale.highlight);
            s.set_pr_scale_fold(scale.highlight && scale.fold);
            s.set_pr_scale_root(key.root_name().into());
            s.set_pr_scale_type(key.kind_name().into());
            let v = self.pr_view();
            let shown: Vec<i32> = v.visible().collect();
            let keys: Vec<YLabel> = shown
                .into_iter()
                .filter(|&p| p % 12 == 0 || (scale.highlight && key.is_root(p)))
                .filter_map(|p| {
                    let y = v.y_of(p);
                    (y >= GRID_Y - 1.0 && y + ROW_H <= v.h + 1.0).then(|| YLabel { y, text: midi::note_name(p as u8).into() })
                })
                .collect();
            sync_model(&self.pr_key_model, keys);
        }
        self.push_step_sequencer(s);
        self.push_composer(s);
        // Context menu.
        s.set_menu_open(self.menu.open);
        if self.menu.open {
            let (items, h) = layout_items(
                self.menu.items.iter().map(|&(a, en)| (a.label().to_string(), self.menu_shortcut(a), en, a.separated(), a.submenu())),
            );
            let sub_for = self.menu.sub_for.unwrap_or(MenuAction::Transpose);
            let sub_y = items.iter().zip(&self.menu.items).find(|(_, (a, _))| *a == sub_for).map(|(m, _)| m.y).unwrap_or(0.0);
            sync_model(&self.menu_model, items);
            s.set_menu_x(self.menu.pos.0);
            s.set_menu_y(self.menu.pos.1);
            s.set_menu_h(h);
            s.set_sub_open(self.menu.sub_open);
            let (sub, sub_h) = if sub_for == MenuAction::TrackColor {
                self.track_color_rows()
            } else {
                let rows: Vec<(String, String, bool)> = if sub_for == MenuAction::Takes {
                    take_rows(self.menu.clip.and_then(|id| p.clip(id)))
                } else if sub_for == MenuAction::Bounce {
                    BOUNCE.iter().map(|&(l, k, on)| (l.to_string(), k.to_string(), on)).collect()
                } else {
                    TRANSPOSE.iter().map(|(l, k, _)| (l.to_string(), k.to_string(), true)).collect()
                };
                layout_items(rows.into_iter().map(|(l, k, on)| (l, k, on, false, false)))
            };
            // A tall submenu (track colours) moves up to stay inside the window.
            let win_h = self.devices.win.1;
            let room = if win_h > 0.0 { win_h - 4.0 - sub_h - self.menu.pos.1 } else { f32::MAX };
            s.set_sub_y((sub_y - MENU_PAD).min(room));
            sync_model(&self.sub_model, sub);
            s.set_sub_h(sub_h);
        }
        // Rename popup.
        let renaming = self
            .rename
            .and_then(|id| p.midi_clip(id).map(|c| c.name.clone()).or_else(|| p.clip(id).map(|c| c.name.clone())))
            .or_else(|| self.rename_section.and_then(|id| p.section_index(id)).map(|i| p.sections[i].name.clone()));
        s.set_rename_title(if self.rename_section.is_some() { "Rename section" } else { "Rename clip" }.into());
        if renaming.is_some() != s.get_rename_open() {
            if let Some(n) = &renaming {
                s.set_rename_text(n.clone().into());
            }
            s.set_rename_open(renaming.is_some());
        }
    }
}

/// Notes mirrored in a clip of `length` ticks: a note that ended at the clip end starts at 0.
/// With the clip's loop on, the part before the loop, the loop itself and the part after it
/// (which never plays) are each mirrored in place, so every note that played still plays and
/// the loop repeats its own material backwards. A note that runs past the end of its part, where
/// playback cuts it (the loop end, or the clip end), is cut there first, so it stays inside the
/// part and reversing twice gives back what played. A note before the loop that plays on into
/// it has no mirror image that sounds the same, so it stays where it is, as do notes that start
/// outside the clip (they never play).
pub fn reversed_in_clip(notes: &[midi::Note], length: Ticks, clip_loop: Option<&midi::ClipLoop>) -> Vec<midi::Note> {
    // Each part with the point where playback cuts its notes.
    let regions: Vec<(Ticks, Ticks, Ticks)> = match clip_loop {
        Some(l) if l.on => {
            let (s, e) = (l.start.clamp(0, length), l.end.clamp(0, length));
            [(0, s, e), (s, e, e), (e, length, length)].into_iter().filter(|r| r.1 > r.0).collect()
        }
        _ => vec![(0, length, length)],
    };
    notes
        .iter()
        .map(|n| {
            let Some((a, b, cut)) = regions.iter().copied().find(|r| n.start >= r.0 && n.start < r.1) else { return *n };
            if n.start + n.length > b && cut > b {
                return *n;
            }
            let length = n.length.min(b - n.start);
            midi::Note { start: a + b - n.start - length, length, ..*n }
        })
        .collect()
}

#[cfg(test)]
mod reverse_tests {
    use super::*;

    #[test]
    fn reversing_mirrors_notes_in_the_clip() {
        let n = |id, start, length| midi::Note { id, pitch: 60, start, length, velocity: 100 };
        let notes = vec![n(1, 0, 240), n(2, 960, 480), n(3, 3600, 240)];
        let r = reversed_in_clip(&notes, 3840, None);
        assert_eq!(r.iter().map(|n| (n.id, n.start, n.length)).collect::<Vec<_>>(), [(1, 3600, 240), (2, 2400, 480), (3, 0, 240)]);
        assert_eq!(reversed_in_clip(&r, 3840, None), notes, "twice is the original");
        let past = reversed_in_clip(&[n(4, 3800, 240)], 3840, None);
        assert_eq!((past[0].start, past[0].length), (0, 40), "a note over the clip end is cut at it, then mirrored");
        assert_eq!(
            reversed_in_clip(&[n(5, 3840, 240), n(6, -10, 20)], 3840, None),
            [n(5, 3840, 240), n(6, -10, 20)],
            "notes outside the clip stay put"
        );
        let whole = midi::ClipLoop::whole(3840);
        assert_eq!(reversed_in_clip(&notes, 3840, Some(&whole)), r, "a loop over the whole clip changes nothing");
    }

    /// A loop that ends before the clip does: the reversed notes stay inside the loop, so the
    /// unrolled clip plays as many notes as before.
    #[test]
    fn reversing_keeps_a_short_loops_notes_inside_it() {
        let n = |id, start, length| midi::Note { id, pitch: 60, start, length, velocity: 100 };
        let notes = vec![n(1, 0, 240), n(2, 960, 240), n(3, 1440, 240)];
        let l = midi::ClipLoop { on: true, start: 960, end: 1920, span: 0 };
        let r = reversed_in_clip(&notes, 7680, Some(&l));
        assert_eq!(r.iter().map(|n| (n.id, n.start)).collect::<Vec<_>>(), [(1, 720), (2, 1680), (3, 1200)]);
        assert_eq!(l.unroll(&r, 7680).len(), l.unroll(&notes, 7680).len());
        assert!(r.iter().all(|n| n.start < l.end), "nothing moved past the loop end");
        assert_eq!(reversed_in_clip(&r, 7680, Some(&l)), notes, "twice is the original");
    }

    /// A note that crosses the loop end is cut at it (playback cuts it there too), so the
    /// reversed note stays in the loop and a second reversal puts it back where it played.
    #[test]
    fn reversing_cuts_a_note_at_its_loop_end() {
        let n = |id, start, length| midi::Note { id, pitch: 60, start, length, velocity: 100 };
        let l = midi::ClipLoop { on: true, start: 960, end: 1920, span: 0 };
        let r = reversed_in_clip(&[n(1, 1800, 240)], 7680, Some(&l));
        assert_eq!((r[0].start, r[0].length), (960, 120));
        let back = reversed_in_clip(&r, 7680, Some(&l));
        assert_eq!((back[0].start, back[0].length), (1800, 120));
        assert_eq!(l.unroll(&back, 7680), l.unroll(&[n(1, 1800, 240)], 7680), "it plays as it did");
    }

    /// A note before the loop that plays on into it stays put; the rest of its part still mirrors.
    #[test]
    fn reversing_keeps_a_note_over_the_loop_start() {
        let n = |id, start, length| midi::Note { id, pitch: 60, start, length, velocity: 100 };
        let l = midi::ClipLoop { on: true, start: 960, end: 1920, span: 0 };
        let notes = vec![n(1, 900, 240), n(2, 0, 120)];
        let r = reversed_in_clip(&notes, 7680, Some(&l));
        assert_eq!(r, [n(1, 900, 240), n(2, 840, 120)]);
        assert_eq!(reversed_in_clip(&r, 7680, Some(&l)), notes, "twice is the original");
        assert_eq!(
            l.unroll(&r, 7680).iter().find(|(m, _)| m.id == 1),
            l.unroll(&notes, 7680).iter().find(|(m, _)| m.id == 1),
            "it plays as it did"
        );
    }
}

#[cfg(test)]
mod lane_tests {
    use super::*;
    use crate::timeline::{Hit, hit_test};
    use dstudio_core::Project;

    fn controller() -> Controller {
        let engine = std::sync::Arc::new(dstudio_engine::Engine::new(dstudio_engine::Config::default()).unwrap());
        let mut c = Controller::new(engine, Project::default());
        c.lanes_size = (1200.0, 600.0);
        c.rebuild_vis();
        c
    }

    fn track(c: &Controller, kind: TrackKind) -> (usize, TrackId) {
        c.project().tracks.iter().enumerate().find(|(_, t)| t.kind == kind).map(|(i, t)| (i, t.id)).unwrap()
    }

    /// A lane point (x, y) in bar `bar` (from 0), beat `beat`, on track row `row`.
    fn lane_point(c: &Controller, row: usize, bar: i64, beat: f64) -> (f32, f32) {
        let beats = (bar * c.project().ticks_per_bar()) as f64 / PPQ as f64 + beat;
        let x = c.view.x_of(beats);
        let y = (0..600)
            .map(|y| y as f32)
            .find(|&y| matches!(hit_test(&c.view, c.lanes_size, c.lane_tracks(), &c.clip_vis, x, y), Hit::Track(i) if i == row));
        (x, y.expect("the track row is on screen"))
    }

    fn midi_clips(c: &Controller, t: TrackId) -> Vec<(Ticks, Ticks)> {
        c.project().track(t).unwrap().midi_clips.iter().map(|m| (m.start, m.length)).collect()
    }

    /// Double-clicking an empty spot of an instrument track makes a one-bar clip in that bar and
    /// opens it in the piano roll; on an audio track it makes nothing.
    #[test]
    fn double_click_on_an_empty_instrument_lane_makes_a_clip_ready_for_notes() {
        let mut c = controller();
        let bar = c.project().ticks_per_bar();
        let (row, inst) = track(&c, TrackKind::Instrument);
        let (x, y) = lane_point(&c, row, 2, 2.5);
        assert!(c.lane_double(x, y));
        assert_eq!(midi_clips(&c, inst), [(2 * bar, bar)]);
        let id = c.project().track(inst).unwrap().midi_clips[0].id;
        assert_eq!(c.selected_clip, Some(id));
        assert_eq!(c.pr_clip().map(|m| m.id), Some(id), "the piano roll shows the new clip");
        c.undo();
        assert!(c.project().track(inst).unwrap().midi_clips.is_empty(), "one undo step");

        let (row, audio) = track(&c, TrackKind::Audio);
        c.rebuild_vis();
        let (x, y) = lane_point(&c, row, 1, 0.0);
        assert!(!c.lane_double(x, y));
        assert!(c.project().track(audio).unwrap().midi_clips.is_empty() && c.project().track(audio).unwrap().clips.is_empty());
    }

    /// A clip made in a bar that another clip already covers part of fills only the free part.
    #[test]
    fn a_new_clip_fits_between_its_neighbours() {
        let mut c = controller();
        let bar = c.project().ticks_per_bar();
        let (_, inst) = track(&c, TrackKind::Instrument);
        for (id, start, len) in [(800, 0, bar + bar / 4), (801, 2 * bar - bar / 4, bar)] {
            c.execute(Command::AddMidiClip { track: inst, clip: MidiClip { id: ClipId(id), ..MidiClip::empty(start, len) } });
        }
        // Pointer in the gap in bar 1 (from 0): the clip starts where the first one ends and stops at the second.
        let id = c.create_midi_clip_at(inst, bar + bar / 2).expect("room in the gap");
        let m = c.project().midi_clip(id).unwrap();
        assert_eq!((m.start, m.length), (bar + bar / 4, bar / 2));
        assert_eq!(m.clip_loop.map(|l| (l.start, l.end)), Some((0, bar / 2)), "the loop follows the shorter length");
    }

    /// ⌘⇧M with the playhead inside a clip puts the new clip after it, never on top of it, and
    /// stops it at the next clip.
    #[test]
    fn insert_at_the_playhead_never_stacks_clips() {
        let mut c = controller();
        let bar = c.project().ticks_per_bar();
        let (_, inst) = track(&c, TrackKind::Instrument);
        c.select_track(inst);
        c.insert_midi_clip().expect("first clip");
        c.insert_midi_clip().expect("second clip, after the first");
        assert_eq!(midi_clips(&c, inst), [(0, bar), (bar, bar)], "the playhead stayed at 0");
        // A clip waiting a beat after the end of bar 3 leaves the third clip one beat.
        c.execute(Command::AddMidiClip { track: inst, clip: MidiClip { id: ClipId(900), ..MidiClip::empty(2 * bar + PPQ, bar) } });
        let id = c.insert_midi_clip().expect("third clip");
        let m = c.project().midi_clip(id).unwrap();
        assert_eq!((m.start, m.length), (2 * bar, PPQ));
        let spans = midi_clips(&c, inst);
        for (i, a) in spans.iter().enumerate() {
            for b in &spans[i + 1..] {
                assert!(a.0 + a.1 <= b.0 || b.0 + b.1 <= a.0, "clips {a:?} and {b:?} overlap");
            }
        }
    }

    /// A double-click that makes a clip is not a seek: the first click's move of the playhead is
    /// taken back. A single click still moves it.
    #[test]
    fn a_double_click_that_makes_a_clip_leaves_the_playhead_where_it_was() {
        let mut c = controller();
        let (row, inst) = track(&c, TrackKind::Instrument);
        c.seek_beats(1.0);
        let (x, y) = lane_point(&c, row, 3, 1.5);
        let click = |c: &mut Controller| {
            if !c.lane_press_double(x, y) {
                c.pointer_mods(0, x, y, false, false);
            }
            c.lane_release(x, y);
            c.pointer_mods(2, x, y, false, false);
        };
        click(&mut c);
        let bar = c.project().ticks_per_bar();
        assert_eq!(c.playhead_ticks(), 3 * bar + 3 * PPQ / 2, "a single click seeks");
        c.seek_beats(1.0);
        c.lane_last_down = None; // a while later
        click(&mut c);
        click(&mut c);
        assert_eq!(midi_clips(&c, inst), [(3 * bar, bar)], "the double-click made a clip");
        assert_eq!(c.playhead_ticks(), PPQ, "and left the playhead at beat 2");
    }

    /// Double-clicking a section to rename it keeps the clip that was open in the Clip tab.
    #[test]
    fn renaming_a_section_keeps_the_open_clip() {
        let mut c = controller();
        let (_, inst) = track(&c, TrackKind::Instrument);
        c.insert_section(None).expect("a section");
        c.create_midi_clip_at(inst, 0).expect("a clip");
        let open = c.selected_clip;
        assert!(c.pr_visible());
        let (x, y) = (c.view.x_of(1.0), (crate::sections::LANE_TOP + crate::sections::LANE_BOTTOM) / 2.0);
        for _ in 0..2 {
            if !c.lane_press_double(x, y) {
                c.pointer_mods(0, x, y, false, false);
            }
            c.lane_release(x, y);
            c.pointer_mods(2, x, y, false, false);
        }
        assert!(c.rename_section.is_some(), "the rename field is open");
        assert_eq!(c.selected_clip, open);
        assert!(c.pr_visible(), "the piano roll still shows the clip");
    }

    /// Saving names the project after its folder, and a saved project has no unsaved changes.
    #[test]
    fn saving_names_the_project_after_its_folder() {
        let mut c = controller();
        let dir = std::env::temp_dir().join(format!("dstudio-name-{}", std::process::id())).join("Verse Ideas.dsproj");
        c.save_to(&dir);
        assert_eq!(c.project().name, "Verse Ideas");
        assert!(!c.doc.is_dirty());
        let saved = dstudio_core::persist::load(&dir).expect("saved project loads");
        assert_eq!(saved.project.name, "Verse Ideas");
        let _ = std::fs::remove_dir_all(dir.parent().unwrap());
    }

    /// Right-click on an empty instrument lane: Insert MIDI clip makes the clip in that bar, and
    /// Paste (once a clip is copied) puts the copy there.
    #[test]
    fn the_empty_lane_menu_inserts_and_pastes_where_it_was_opened() {
        let mut c = controller();
        let bar = c.project().ticks_per_bar();
        let (row, inst) = track(&c, TrackKind::Instrument);
        let (x, y) = lane_point(&c, row, 3, 1.0);
        c.lane_context(x, y, 500.0, 300.0, (1400.0, 900.0));
        assert!(c.menu.open);
        assert_eq!(c.menu.items, [(MenuAction::InsertMidiClip, true), (MenuAction::Paste, false)], "nothing copied yet");
        c.menu_activate(0);
        assert_eq!(midi_clips(&c, inst), [(3 * bar, bar)]);

        assert!(c.copy_clip());
        c.rebuild_vis();
        let (x, y) = lane_point(&c, row, 5, 3.0);
        c.lane_context(x, y, 500.0, 300.0, (1400.0, 900.0));
        assert_eq!(c.menu.items[1], (MenuAction::Paste, true));
        c.menu_activate(1);
        assert_eq!(midi_clips(&c, inst), [(3 * bar, bar), (5 * bar, bar)]);
    }
}
