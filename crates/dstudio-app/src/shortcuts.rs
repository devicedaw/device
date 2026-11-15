//! Settings › Shortcuts: every keyboard shortcut, grouped by section, with its keys and the
//! context it works in. Click a row's keys to record new ones (Escape cancels), × leaves the
//! action unmapped, Reset to defaults drops every change. Changes apply at once and are saved
//! with the app preferences (`Preferences::shortcuts`), never in a project.

use crate::controller::Controller;
use crate::{ShortcutKey, ShortcutRow, Studio};
use dstudio_core::keymap::{Chord, Condition, Keymap};
use slint::{ModelRc, SharedString, VecModel};

/// Settings sidebar index of the page.
pub const SHORTCUTS_PAGE: usize = 4;

/// Row heights in the list (design units): a section header block (its bar plus the gaps
/// around it) and an action row.
pub const HEADER_H: f32 = 34.0;
pub const ROW_H: f32 = 27.2;

/// The page's sections in order, each with its actions (keymap id, label). Only actions
/// the app runs are listed; `view.escape` (closes menus and dialogs) is not rebindable.
pub const SECTIONS: &[(&str, &[(&str, &str)])] = &[
    (
        "Transport",
        &[
            ("transport.play_pause", "Play / pause"),
            ("transport.stop_return", "Stop and return to start"),
            ("transport.record", "Start / stop recording"),
            ("transport.loop_selection", "Loop selection"),
            ("transport.loop_toggle", "Loop on / off"),
            ("transport.tap_tempo", "Tap tempo"),
        ],
    ),
    (
        "Editing",
        &[
            ("edit.undo", "Undo last action"),
            ("edit.redo", "Redo last action"),
            ("clip.split_at_marker", "Split clip at insert marker"),
            ("clip.reverse", "Reverse selected clip"),
            ("clip.consolidate", "Consolidate clips"),
            ("clip.bounce_to_track", "Bounce to new track"),
            ("clip.duplicate", "Duplicate selected clips"),
            ("section.duplicate", "Duplicate selected sections"),
            ("clip.insert_midi", "Insert empty MIDI clip"),
            ("clip.cut", "Cut selected clips"),
            ("edit.copy", "Copy selected clips"),
            ("edit.paste", "Paste"),
            ("clip.delete", "Delete selected clips"),
            ("clip.disable", "Mute clip"),
            ("clip.octave_up", "Transpose clip up one octave"),
            ("clip.octave_down", "Transpose clip down one octave"),
            ("clip.transpose_up", "Transpose clip up one semitone"),
            ("clip.transpose_down", "Transpose clip down one semitone"),
            ("clip.quantize", "Quantize notes"),
        ],
    ),
    (
        "Tracks",
        &[
            ("track.group", "Group tracks"),
            ("track.ungroup", "Ungroup tracks"),
            ("track.duplicate_with_clips", "Duplicate tracks with clips"),
            ("track.duplicate", "Duplicate tracks"),
            ("track.add_instrument", "Add instrument track"),
            ("track.add_audio", "Add audio track"),
            ("track.arm", "Arm / disarm selected track"),
            ("view.toggle_automation", "Show / hide automation"),
            ("track.mute", "Mute track"),
            ("track.solo", "Solo selected track"),
            ("track.monitor", "Input monitoring on / off"),
            ("track.fold", "Fold / unfold selected tracks"),
            ("track.rename", "Rename track"),
            ("track.delete", "Delete track"),
        ],
    ),
    (
        "Navigation",
        &[
            ("view.toggle_bottom_panel", "Show / hide bottom panel"),
            ("view.toggle_mixer", "Show / hide mixer"),
            ("view.fit_width", "Zoom to fit the song"),
            ("view.fit_height", "Fit track heights to view"),
            ("draw_mode.toggle", "Draw mode (automation pencil)"),
            ("track.select_prev", "Select previous track"),
            ("track.select_next", "Select next track"),
            ("view.zoom_in", "Zoom timeline in"),
        ],
    ),
    (
        "AI & Other",
        &[
            ("file.new", "New project"),
            ("file.open", "Open project"),
            ("file.save", "Save project"),
            ("file.save_as", "Save project as"),
            ("file.import_audio", "Import audio"),
            ("file.export_audio", "Export audio"),
            ("app.settings", "Open settings"),
            ("view.toggle_assistant", "Open or close the assistant"),
        ],
    ),
    (
        "MIDI Editor",
        &[
            ("view.musical_typing", "Musical typing on / off"),
            ("midi.transpose_up", "Move notes up a semitone"),
            ("midi.transpose_down", "Move notes down a semitone"),
            ("midi.octave_up", "Move notes up an octave"),
            ("midi.octave_down", "Move notes down an octave"),
            ("midi.velocity_up", "Raise note velocity by 1"),
            ("midi.velocity_down", "Lower note velocity by 1"),
            ("midi.velocity_up_10", "Raise note velocity by 10"),
            ("midi.velocity_down_10", "Lower note velocity by 10"),
            ("midi.nudge_left", "Move notes left a grid step"),
            ("midi.nudge_right", "Move notes right a grid step"),
            ("midi.nudge_bar_left", "Move notes left a bar"),
            ("midi.nudge_bar_right", "Move notes right a bar"),
            ("midi.select_all", "Select all notes"),
            ("midi.delete_notes", "Delete selected notes"),
            ("midi.reverse", "Reverse selected notes"),
            ("midi.paint_pen", "MIDI paint pen"),
        ],
    ),
];

/// The context column: when a shortcut works.
pub fn condition_text(c: Condition) -> &'static str {
    match c {
        Condition::Always => "Always",
        Condition::TrackSelected => "Track selected",
        Condition::TrackHeaderSelected => "Track header selected",
        Condition::ClipSelected => "Clip selected",
        Condition::ClipsSelected => "Clips selected",
        Condition::ClipOrSectionSelected => "Clip or section selected",
        Condition::RangeSelected => "Range selected",
        Condition::SectionSelected => "Section selected",
        Condition::FxSlotSelected => "FX slot selected",
        Condition::MidiClipSelected => "MIDI clip selected",
        Condition::MidiEditorFocused => "MIDI editor focused",
        Condition::ClipOrDevicesFocused => "Clip or Devices focused",
        Condition::MixerOpen => "Mixer open",
        Condition::ChatFocused => "Chat focused",
        Condition::CatalogFocused => "Catalog focused",
    }
}

#[derive(Debug)]
pub struct ShortcutsUi {
    /// The live keymap: the defaults with the saved changes applied.
    pub map: Keymap,
    /// The action whose keys are being recorded ("Press keys").
    pub recording: Option<&'static str>,
    pub search: String,
    /// What each shown row is (None = a section header), in list order (set when the
    /// rows are pushed).
    pub shown: std::cell::RefCell<Vec<Option<&'static str>>>,
}

impl Default for ShortcutsUi {
    fn default() -> Self {
        ShortcutsUi { map: crate::app_keymap(), recording: None, search: String::new(), shown: Default::default() }
    }
}

/// The defaults with `saved` applied (unknown actions and chords that no longer parse are skipped).
pub fn keymap_with(saved: &std::collections::BTreeMap<String, Vec<String>>) -> Keymap {
    let mut map = crate::app_keymap();
    for (action, chords) in saved {
        let chords: Vec<Chord> = chords.iter().filter_map(|c| Chord::parse(c)).collect();
        map.set_chords(action, chords);
    }
    map
}

/// The saved form of `map`: the listed actions whose chords differ from the defaults.
pub fn changes(map: &Keymap) -> std::collections::BTreeMap<String, Vec<String>> {
    let defaults = crate::app_keymap();
    SECTIONS
        .iter()
        .flat_map(|(_, rows)| rows.iter())
        .filter_map(|(action, _)| {
            let now: Vec<&Chord> = map.chords_for(action);
            (now != defaults.chords_for(action)).then(|| (action.to_string(), now.iter().map(|c| c.spec()).collect()))
        })
        .collect()
}

/// Whether a row matches the search text (its label, context or keys).
fn matches(query: &str, label: &str, context: &str, chords: &[&Chord]) -> bool {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return true;
    }
    let keys: String = chords.iter().map(|c| c.badges().concat()).collect::<Vec<_>>().join(" ").to_lowercase();
    label.to_lowercase().contains(&q) || context.to_lowercase().contains(&q) || keys.contains(&q)
}

impl Controller {
    /// ↑/↓ with a track selected: the track drawn above or below it on the timeline (tracks in
    /// a folded group and hidden return rows are skipped).
    pub fn select_track_step(&mut self, dir: i32) {
        let p = self.project();
        let rows: Vec<dstudio_core::TrackId> =
            p.tracks[..self.lane_tracks()].iter().filter(|t| !self.hidden_by_fold(t)).map(|t| t.id).collect();
        let Some(cur) = self.selected_track.and_then(|id| rows.iter().position(|t| *t == id)) else { return };
        let next = (cur as i32 + dir).clamp(0, rows.len() as i32 - 1) as usize;
        if next != cur {
            self.select_track(rows[next]);
            self.ui_dirty = true;
        }
    }

    /// U: fold or unfold the selected group (or the group the selected track plays into).
    pub fn fold_selected_group(&mut self) {
        let Some(t) = self.selected_track.and_then(|id| self.project().track(id)) else { return };
        let group = if t.is_group() { Some(t.id) } else { t.output.filter(|g| self.project().track(*g).is_some_and(|g| g.is_group())) };
        if let Some(g) = group {
            self.toggle_group_folded(g);
        }
    }

    /// ⌘⇧G: ungroup the selected group.
    pub fn ungroup_selected(&mut self) {
        if let Some(g) = self.selected_track.filter(|id| self.project().track(*id).is_some_and(|t| t.is_group())) {
            self.ungroup(g);
        }
    }

    /// Startup: apply the saved shortcut changes.
    pub fn shortcuts_load(&mut self) {
        self.settings.keys.map = keymap_with(&self.settings.prefs.shortcuts);
    }

    fn shortcuts_save(&mut self) {
        self.settings.prefs.shortcuts = changes(&self.settings.keys.map);
        self.save_prefs();
        self.ui_dirty = true;
    }

    /// Click on a row's keys: record new keys for it (a second click cancels).
    pub fn shortcut_record(&mut self, row: usize) {
        let Some(Some(action)) = self.settings.keys.shown.borrow().get(row).copied() else { return };
        self.settings.keys.recording = if self.settings.keys.recording == Some(action) { None } else { Some(action) };
        self.ui_dirty = true;
    }

    /// × on a row: the action keeps no keys ("Unmapped").
    pub fn shortcut_clear(&mut self, row: usize) {
        let Some(Some(action)) = self.settings.keys.shown.borrow().get(row).copied() else { return };
        self.settings.keys.map.set_chords(action, Vec::new());
        self.settings.keys.recording = None;
        self.shortcuts_save();
    }

    pub fn shortcuts_reset(&mut self) {
        self.settings.keys.map = crate::app_keymap();
        self.settings.keys.recording = None;
        self.shortcuts_save();
    }

    pub fn shortcuts_search(&mut self, text: &str) {
        self.settings.keys.search = text.to_string();
        self.ui_dirty = true;
    }

    /// A key pressed while recording: Escape cancels; any other chord becomes the action's
    /// only keys, and is taken off other actions that work in the same context (so one key
    /// never runs two actions). Returns false when nothing is being recorded.
    pub fn shortcut_key(&mut self, chord: &Chord) -> bool {
        let Some(action) = self.settings.keys.recording.take() else { return false };
        if chord.key == "Escape" && !chord.primary && !chord.shift && !chord.alt && !chord.ctrl {
            self.ui_dirty = true;
            return true;
        }
        let map = &mut self.settings.keys.map;
        let when = map.condition_of(action);
        for b in map.bindings.iter_mut().filter(|b| b.action != action && Some(b.when) == when) {
            b.chords.retain(|c| c != chord);
        }
        map.set_chords(action, vec![chord.clone()]);
        self.shortcuts_save();
        true
    }

    pub(crate) fn push_shortcuts(&self, s: &Studio) {
        let keys = &self.settings.keys;
        let mut rows = Vec::new();
        let mut shown = Vec::new();
        let mut y = 0.0f32;
        for (section, actions) in SECTIONS {
            let mut section_rows = Vec::new();
            for (action, label) in actions.iter() {
                let Some(when) = keys.map.condition_of(action) else { continue };
                let context = condition_text(when);
                let chords = keys.map.chords_for(action);
                if !matches(&keys.search, label, context, &chords) {
                    continue;
                }
                let chord_rows: Vec<ShortcutKey> = chords
                    .iter()
                    .map(|c| ShortcutKey {
                        parts: ModelRc::new(VecModel::from(c.badges().into_iter().map(SharedString::from).collect::<Vec<_>>())),
                    })
                    .collect();
                section_rows.push((
                    *action,
                    ShortcutRow {
                        header: false,
                        label: (*label).into(),
                        context: context.into(),
                        chords: ModelRc::new(VecModel::from(chord_rows)),
                        recording: keys.recording == Some(*action),
                        y: 0.0,
                        h: ROW_H,
                    },
                ));
            }
            if section_rows.is_empty() {
                continue;
            }
            rows.push(ShortcutRow {
                header: true,
                label: (*section).into(),
                context: SharedString::new(),
                chords: ModelRc::default(),
                recording: false,
                y,
                h: HEADER_H,
            });
            shown.push(None);
            y += HEADER_H;
            for (action, mut row) in section_rows {
                row.y = y;
                y += ROW_H;
                rows.push(row);
                shown.push(Some(action));
            }
        }
        let empty = shown.is_empty();
        *keys.shown.borrow_mut() = shown;
        s.set_shortcut_rows(ModelRc::new(VecModel::from(rows)));
        s.set_shortcut_list_h(y + 12.0);
        s.set_shortcut_recording(keys.recording.is_some());
        s.set_shortcut_reset_enabled(!self.settings.prefs.shortcuts.is_empty());
        s.set_shortcut_empty(empty);
        if s.get_shortcut_search().as_str() != keys.search {
            s.set_shortcut_search(keys.search.as_str().into());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_listed_action_is_in_the_keymap() {
        let map = crate::app_keymap();
        for (section, rows) in SECTIONS {
            for (action, label) in rows.iter() {
                assert!(map.condition_of(action).is_some(), "{section} › {label}: {action} has no binding");
            }
        }
    }

    #[test]
    fn saved_changes_round_trip() {
        let mut map = crate::app_keymap();
        assert!(changes(&map).is_empty());
        map.set_chords("transport.record", vec![Chord::parse("primary+shift+r").unwrap()]);
        map.set_chords("clip.cut", Vec::new());
        let saved = changes(&map);
        assert_eq!(saved.get("transport.record"), Some(&vec!["primary+shift+r".to_string()]));
        assert_eq!(saved.get("clip.cut"), Some(&Vec::new()));
        assert_eq!(saved.len(), 2);
        assert_eq!(keymap_with(&saved), map);
    }

    #[test]
    fn search_matches_label_context_and_keys() {
        let space = Chord::parse("space").unwrap();
        assert!(matches("play", "Play / pause", "Always", &[&space]));
        assert!(matches("SPACE", "Play / pause", "Always", &[&space]));
        assert!(matches("always", "Play / pause", "Always", &[&space]));
        assert!(!matches("undo", "Play / pause", "Always", &[&space]));
        assert!(matches("  ", "Play / pause", "Always", &[]));
    }
}
