//! Context-aware keyboard shortcuts.
//!
//! Every binding has a *condition*, several actions may share a chord, and the
//! most specific satisfied condition wins (e.g. ⌘L = "Loop selection" when a
//! clip is selected, otherwise "Loop on/off"). `primary` is ⌘ on macOS and
//! Ctrl on Windows/Linux, so one preset serves every platform.

use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Chord {
    /// Key name: a letter/digit ("Z", "1"), or "Space", "Enter", "Delete",
    /// "Backspace", "Escape", "Tab", "Up", "Down", "Left", "Right", "PageUp",
    /// "PageDown", "Home", "End", "Comma", "Period", "F1".."F12".
    pub key: String,
    pub primary: bool,
    pub shift: bool,
    pub alt: bool,
    /// The physical Control key on macOS (only meaningful there).
    pub ctrl: bool,
}

impl Chord {
    /// Parse "primary+shift+z", "space", "alt+left" (case-insensitive).
    pub fn parse(s: &str) -> Option<Chord> {
        let mut c = Chord { key: String::new(), primary: false, shift: false, alt: false, ctrl: false };
        for part in s.split('+').map(str::trim) {
            match part.to_ascii_lowercase().as_str() {
                "primary" | "cmd" | "mod" => c.primary = true,
                "shift" => c.shift = true,
                "alt" | "opt" | "option" => c.alt = true,
                "ctrl" | "control" => c.ctrl = true,
                "" => return None,
                k => c.key = normalize_key(k)?,
            }
        }
        (!c.key.is_empty()).then_some(c)
    }
}

fn normalize_key(k: &str) -> Option<String> {
    let named = [
        "space",
        "enter",
        "delete",
        "backspace",
        "escape",
        "tab",
        "up",
        "down",
        "left",
        "right",
        "pageup",
        "pagedown",
        "home",
        "end",
        "comma",
        "period",
    ];
    if k == "plus" {
        return Some("+".into());
    }
    if k.chars().count() == 1 {
        return Some(k.to_ascii_uppercase());
    }
    if let Some(n) = named.iter().find(|n| **n == k) {
        let mut s = n.to_string();
        s[..1].make_ascii_uppercase();
        return Some(match s.as_str() {
            "Pageup" => "PageUp".into(),
            "Pagedown" => "PageDown".into(),
            _ => s,
        });
    }
    if let Some(num) = k.strip_prefix('f').and_then(|n| n.parse::<u8>().ok()).filter(|n| (1..=24).contains(n)) {
        return Some(format!("F{num}"));
    }
    None
}

impl Chord {
    /// The text form `Chord::parse` reads back ("primary+shift+z"), for saving a binding.
    pub fn spec(&self) -> String {
        let mut s = String::new();
        for (on, m) in [(self.primary, "primary+"), (self.ctrl, "ctrl+"), (self.shift, "shift+"), (self.alt, "alt+")] {
            if on {
                s.push_str(m);
            }
        }
        // "+" also separates the parts, so it is saved by name.
        s.push_str(if self.key == "+" { "plus" } else { &self.key }.to_ascii_lowercase().as_str());
        s
    }

    /// The badges a shortcut list shows for this chord, modifiers first in
    /// ⌘ ⌃ ⇧ ⌥ order, then the key ("Space", "↑", "L").
    pub fn badges(&self) -> Vec<String> {
        let mut out: Vec<String> = [(self.primary, "⌘"), (self.ctrl, "⌃"), (self.shift, "⇧"), (self.alt, "⌥")]
            .into_iter()
            .filter(|(on, _)| *on)
            .map(|(_, g)| g.to_string())
            .collect();
        out.push(
            match self.key.as_str() {
                "Up" => "↑",
                "Down" => "↓",
                "Left" => "←",
                "Right" => "→",
                "Comma" => ",",
                "Period" => ".",
                "Backspace" => "⌫",
                "Delete" => "⌦",
                "Escape" => "Esc",
                k => k,
            }
            .to_string(),
        );
        out
    }
}

impl fmt::Display for Chord {
    /// macOS-style glyphs, as shown in shortcut badges.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.ctrl {
            f.write_str("⌃")?;
        }
        if self.alt {
            f.write_str("⌥")?;
        }
        if self.shift {
            f.write_str("⇧")?;
        }
        if self.primary {
            f.write_str("⌘")?;
        }
        f.write_str(&self.key)
    }
}

/// Conditions a binding can carry, as listed in the shortcut editor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Condition {
    Always,
    TrackSelected,
    TrackHeaderSelected,
    ClipSelected,
    ClipsSelected,
    ClipOrSectionSelected,
    RangeSelected,
    SectionSelected,
    FxSlotSelected,
    MidiClipSelected,
    MidiEditorFocused,
    ClipOrDevicesFocused,
    MixerOpen,
    ChatFocused,
    CatalogFocused,
}

/// Snapshot of UI focus/selection used to evaluate conditions.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Context {
    pub track_selected: bool,
    pub track_header_selected: bool,
    pub clips_selected: usize,
    pub midi_clip_selected: bool,
    pub range_selected: bool,
    pub section_selected: bool,
    pub fx_slot_selected: bool,
    pub midi_editor_focused: bool,
    pub clip_or_devices_focused: bool,
    pub mixer_open: bool,
    pub chat_focused: bool,
    pub catalog_focused: bool,
    /// A text field has keyboard focus: single-key shortcuts must not fire.
    pub text_input_focused: bool,
}

impl Condition {
    pub fn holds(self, c: &Context) -> bool {
        match self {
            Condition::Always => true,
            Condition::TrackSelected => c.track_selected,
            Condition::TrackHeaderSelected => c.track_header_selected,
            Condition::ClipSelected => c.clips_selected == 1,
            Condition::ClipsSelected => c.clips_selected >= 1,
            Condition::ClipOrSectionSelected => c.clips_selected >= 1 || c.section_selected,
            Condition::RangeSelected => c.range_selected,
            Condition::SectionSelected => c.section_selected,
            Condition::FxSlotSelected => c.fx_slot_selected,
            Condition::MidiClipSelected => c.midi_clip_selected,
            Condition::MidiEditorFocused => c.midi_editor_focused,
            Condition::ClipOrDevicesFocused => c.clip_or_devices_focused,
            Condition::MixerOpen => c.mixer_open,
            Condition::ChatFocused => c.chat_focused,
            Condition::CatalogFocused => c.catalog_focused,
        }
    }

    /// Higher = more specific; ties between satisfied bindings on one chord are conflicts.
    fn specificity(self) -> u8 {
        match self {
            Condition::Always => 0,
            Condition::TrackSelected | Condition::ClipOrDevicesFocused => 1,
            // The open mixer's strip keys (←/→, ↑/↓) win over the selected track's ↑/↓.
            Condition::MixerOpen
            | Condition::TrackHeaderSelected
            | Condition::ClipsSelected
            | Condition::ClipOrSectionSelected
            | Condition::CatalogFocused
            | Condition::ChatFocused => 2,
            Condition::ClipSelected | Condition::SectionSelected | Condition::FxSlotSelected => 3,
            Condition::RangeSelected | Condition::MidiClipSelected => 4,
            // The piano roll has focus: its keys win over the selected clip's (⇧↑ moves the
            // selected notes up an octave there, and transposes the whole clip on the timeline).
            Condition::MidiEditorFocused => 5,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Binding {
    /// Stable action id, e.g. "transport.play_pause".
    pub action: String,
    pub chords: Vec<Chord>,
    pub when: Condition,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Keymap {
    pub preset: String,
    pub bindings: Vec<Binding>,
}

impl Keymap {
    /// Resolve a pressed chord to an action in the given context.
    pub fn resolve(&self, chord: &Chord, ctx: &Context) -> Option<&str> {
        let bare_key = !chord.primary && !chord.ctrl && !chord.alt;
        if ctx.text_input_focused && bare_key && chord.key != "Escape" && chord.key != "Enter" {
            return None;
        }
        self.bindings
            .iter()
            .filter(|b| b.chords.contains(chord) && b.when.holds(ctx))
            .max_by_key(|b| b.when.specificity())
            .map(|b| b.action.as_str())
    }

    pub fn chords_for(&self, action: &str) -> Vec<&Chord> {
        self.bindings.iter().filter(|b| b.action == action).flat_map(|b| &b.chords).collect()
    }

    /// Replace the chords of `action` (all of them go on its first binding; an empty list
    /// leaves the action unmapped). Returns false when the keymap has no such action.
    pub fn set_chords(&mut self, action: &str, chords: Vec<Chord>) -> bool {
        let mut first = true;
        let mut found = false;
        for b in self.bindings.iter_mut().filter(|b| b.action == action) {
            found = true;
            b.chords = if first { chords.clone() } else { Vec::new() };
            first = false;
        }
        found
    }

    /// The condition of `action`'s first binding.
    pub fn condition_of(&self, action: &str) -> Option<Condition> {
        self.bindings.iter().find(|b| b.action == action).map(|b| b.when)
    }

    /// Pairs of actions that share a chord and an equally specific condition
    /// (both could fire in the same context with no winner).
    pub fn conflicts(&self) -> Vec<(String, String, Chord)> {
        let mut out = Vec::new();
        for (i, a) in self.bindings.iter().enumerate() {
            for b in &self.bindings[i + 1..] {
                if a.when.specificity() != b.when.specificity() || !overlap(a.when, b.when) {
                    continue;
                }
                for c in a.chords.iter().filter(|c| b.chords.contains(c)) {
                    out.push((a.action.clone(), b.action.clone(), c.clone()));
                }
            }
        }
        out
    }

    /// Default preset.
    pub fn default_preset() -> Keymap {
        let b = |action: &str, keys: &[&str], when: Condition| Binding {
            action: action.into(),
            chords: keys.iter().map(|k| Chord::parse(k).expect("valid default chord")).collect(),
            when,
        };
        use Condition::*;
        Keymap {
            preset: "Default".into(),
            bindings: vec![
                // Transport
                b("transport.play_pause", &["space"], Always),
                b("transport.stop_return", &["enter"], Always),
                b("transport.record", &["r"], Always),
                b("transport.loop_selection", &["primary+l"], ClipOrSectionSelected),
                b("transport.loop_toggle", &["primary+l"], Always),
                // Editing
                b("edit.undo", &["primary+z"], Always),
                b("edit.redo", &["primary+shift+z"], Always),
                b("clip.split_at_range", &["primary+e"], RangeSelected),
                b("clip.split_at_marker", &["primary+e"], ClipSelected),
                b("clip.reverse", &["primary+r"], ClipSelected),
                b("clip.consolidate", &["primary+j"], ClipsSelected),
                b("clip.bounce_to_track", &["primary+b"], ClipsSelected),
                b("fx.duplicate", &["primary+d"], FxSlotSelected),
                b("range.duplicate", &["primary+d"], RangeSelected),
                b("clip.duplicate", &["primary+d"], ClipsSelected),
                b("section.duplicate", &["primary+d"], SectionSelected),
                b("clip.insert_midi", &["primary+shift+m"], TrackSelected),
                b("clip.cut", &["primary+x"], ClipsSelected),
                b("edit.copy", &["primary+c"], ClipsSelected),
                b("edit.paste", &["primary+v"], Always),
                b("tool.scissors", &["t"], Always),
                b("midi.paint_pen", &["b", "e"], MidiEditorFocused),
                // Tracks / project
                b("clip.delete", &["delete", "backspace"], ClipsSelected),
                b("track.delete", &["delete", "backspace"], TrackHeaderSelected),
                b("track.add_audio", &["primary+t"], Always),
                b("track.add_instrument", &["primary+shift+t"], Always),
                // Mutes the selected clips, or the selected track when no clip is selected.
                b("track.mute", &["0", "m"], TrackSelected),
                b("track.solo", &["s"], TrackSelected),
                b("file.new", &["primary+n"], Always),
                b("file.save", &["primary+s"], Always),
                b("file.save_as", &["primary+shift+s"], Always),
                b("file.open", &["primary+o"], Always),
                b("file.import_audio", &["primary+i"], Always),
                b("file.export_audio", &["primary+shift+e"], Always),
                b("app.settings", &["primary+comma"], Always), // (tooltip)
                b("view.zoom_in", &["primary+period"], Always),
                b("view.musical_typing", &["k"], Always),          // (tooltip K / ⇧K)
                b("draw_mode.toggle", &["b"], Always),             // (tooltip "Draw mode (B)")
                b("assistant.ask", &["primary+k"], ClipsSelected), // (context menu "Ask ⌘K")
            ],
        }
    }
}

fn overlap(a: Condition, b: Condition) -> bool {
    // Conservative: different selection-type conditions at equal specificity
    // are treated as mutually exclusive (a clip, an FX slot and a section
    // cannot hold focus at the same time); identical conditions overlap.
    a == b
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ch(s: &str) -> Chord {
        Chord::parse(s).unwrap()
    }

    #[test]
    fn parsing_and_display() {
        assert_eq!(ch("cmd+shift+Z").to_string(), "⇧⌘Z");
        assert_eq!(ch("space").key, "Space");
        assert_eq!(ch("pagedown").key, "PageDown");
        assert!(Chord::parse("primary+").is_none());
        assert!(Chord::parse("hyper+q").is_none());
    }

    #[test]
    fn spec_round_trips_and_badges_read_in_order() {
        for s in ["primary+shift+z", "space", "alt+up", "ctrl+alt+k", "primary+comma", "primary+plus", "ctrl+-", "primary+="] {
            let c = ch(s);
            assert_eq!(Chord::parse(&c.spec()), Some(c.clone()), "{s}");
        }
        assert_eq!(ch("primary+shift+up").badges(), ["⌘", "⇧", "↑"]);
        assert_eq!(ch("shift+alt+down").badges(), ["⇧", "⌥", "↓"]);
        assert_eq!(ch("space").badges(), ["Space"]);
        assert_eq!(ch("primary+plus").badges(), ["⌘", "+"]);
    }

    #[test]
    fn set_chords_rebinds_and_unmaps() {
        let mut k = Keymap::default_preset();
        assert!(k.set_chords("transport.record", vec![ch("primary+shift+r")]));
        assert_eq!(k.resolve(&ch("primary+shift+r"), &Context::default()), Some("transport.record"));
        assert_eq!(k.resolve(&ch("r"), &Context::default()), None);
        assert!(k.set_chords("transport.record", Vec::new()));
        assert!(k.chords_for("transport.record").is_empty());
        assert!(!k.set_chords("no.such_action", Vec::new()));
        assert_eq!(k.condition_of("clip.reverse"), Some(Condition::ClipSelected));
    }

    #[test]
    fn most_specific_condition_wins() {
        let k = Keymap::default_preset();
        let none = Context::default();
        let clip = Context { clips_selected: 1, ..Default::default() };
        assert_eq!(k.resolve(&ch("primary+l"), &none), Some("transport.loop_toggle"));
        assert_eq!(k.resolve(&ch("primary+l"), &clip), Some("transport.loop_selection"));
        assert_eq!(k.resolve(&ch("primary+e"), &clip), Some("clip.split_at_marker"));
        let range = Context { range_selected: true, clips_selected: 1, ..Default::default() };
        assert_eq!(k.resolve(&ch("primary+e"), &range), Some("clip.split_at_range"));
        assert_eq!(k.resolve(&ch("primary+e"), &none), None);
        let two = Context { clips_selected: 2, ..Default::default() };
        assert_eq!(k.resolve(&ch("primary+d"), &two), Some("clip.duplicate"));
    }

    #[test]
    fn single_keys_are_suppressed_while_typing() {
        let k = Keymap::default_preset();
        let typing = Context { text_input_focused: true, ..Default::default() };
        assert_eq!(k.resolve(&ch("space"), &typing), None);
        assert_eq!(k.resolve(&ch("primary+z"), &typing), Some("edit.undo"));
    }

    #[test]
    fn default_preset_has_no_conflicts_and_round_trips() {
        let k = Keymap::default_preset();
        assert!(k.conflicts().is_empty(), "{:?}", k.conflicts());
        let json = serde_json::to_string(&k).unwrap();
        assert_eq!(serde_json::from_str::<Keymap>(&json).unwrap(), k);
    }

    #[test]
    fn conflicts_are_detected() {
        let mut k = Keymap::default_preset();
        k.bindings.push(Binding { action: "x".into(), chords: vec![ch("space")], when: Condition::Always });
        assert_eq!(k.conflicts().len(), 1);
    }
}
