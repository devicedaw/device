//! Shortcut presets (Settings › Shortcuts › Preset): the keys other DAWs use for the same
//! actions, so people can keep their habits. A preset is the default keymap with these
//! actions' keys replaced (an empty list leaves the action unmapped); the user's own
//! changes go on top of the chosen preset.

/// One preset: its name and the actions whose keys differ from the defaults.
pub struct Preset {
    pub name: &'static str,
    pub keys: &'static [(&'static str, &'static [&'static str])],
}

/// Logic Pro and GarageBand share their keys.
const LOGIC: &[(&str, &[&str])] = &[
    ("transport.record", &["r", "shift+r", "*"]),
    ("transport.loop_selection", &["u", "primary+u"]),
    ("transport.loop_toggle", &["c"]),
    ("clip.split_at_range", &["primary+alt+t"]),
    ("clip.split_at_marker", &["primary+t"]),
    ("clip.reverse", &["ctrl+shift+r"]),
    ("clip.bounce_to_track", &["ctrl+b"]),
    ("fx.duplicate", &["primary+r"]),
    ("range.duplicate", &["primary+r"]),
    ("clip.duplicate", &["primary+r"]),
    ("clip.quantize", &["q"]),
    ("track.duplicate_with_clips", &["primary+shift+alt+d"]),
    ("track.add_instrument", &["primary+alt+s"]),
    ("track.add_audio", &["primary+alt+a"]),
    ("track.arm", &["r", "ctrl+r"]),
    ("track.mute", &["m", "ctrl+m"]),
    ("track.fold", &[]),
    ("view.toggle_mixer", &["x"]),
];

pub static PRESETS: &[Preset] = &[
    Preset { name: "Device Studio", keys: &[] },
    Preset {
        name: "Ableton Live",
        keys: &[
            ("transport.record", &["r", "f9"]),
            ("midi.paint_pen", &["b"]),
            ("track.arm", &["c"]),
            ("track.mute", &["0"]),
            ("view.switch_bottom_tab", &["shift+tab", "f12"]),
            ("file.export_audio", &["primary+shift+r"]),
            ("view.musical_typing", &["m"]),
        ],
    },
    Preset { name: "Logic Pro", keys: LOGIC },
    Preset { name: "GarageBand", keys: LOGIC },
    Preset {
        name: "FL Studio",
        keys: &[
            ("transport.tap_tempo", &[]),
            ("clip.consolidate", &["primary+g", "primary+j"]),
            ("clip.bounce_to_track", &["primary+alt+c"]),
            ("fx.duplicate", &["primary+b"]),
            ("range.duplicate", &["primary+b"]),
            ("clip.duplicate", &["primary+b"]),
            ("section.duplicate", &["primary+b"]),
            ("midi.paint_pen", &["p", "shift+p"]),
            ("clip.transpose_up", &[]),
            ("clip.transpose_down", &[]),
            ("clip.quantize", &["alt+q", "primary+u", "q"]),
            ("clip.move_to_track_above", &["shift+up"]),
            ("clip.move_to_track_below", &["shift+down"]),
            ("track.group", &[]),
            ("track.ungroup", &[]),
            ("track.duplicate", &["primary+b"]),
            ("track.add_audio", &["shift+t"]),
            ("track.arm", &["r"]),
            ("track.mute", &["m"]),
            ("view.toggle_mixer", &["f9"]),
            ("view.musical_typing", &["primary+t"]),
            ("midi.transpose_up", &["shift+up", "up"]),
            ("midi.transpose_down", &["shift+down", "down"]),
            ("midi.octave_up", &["primary+up"]),
            ("midi.octave_down", &["primary+down"]),
            ("midi.velocity_up", &[]),
            ("midi.velocity_down", &[]),
            ("midi.nudge_left", &["shift+left", "left"]),
            ("midi.nudge_right", &["shift+right", "right"]),
            ("midi.nudge_bar_left", &[]),
            ("midi.nudge_bar_right", &[]),
            ("midi.glue", &["primary+g"]),
        ],
    },
    Preset {
        name: "Fender Studio Pro",
        keys: &[
            ("transport.record", &["r", "*"]),
            ("transport.loop_toggle", &["/"]),
            ("clip.split_at_range", &["primary+alt+x"]),
            ("clip.split_at_marker", &["alt+x"]),
            ("fx.duplicate", &["d"]),
            ("range.duplicate", &["d"]),
            ("clip.duplicate", &["d"]),
            ("section.duplicate", &["d"]),
            ("clip.insert_midi", &["primary+p"]),
            ("midi.paint_pen", &["5", "shift+5"]),
            ("track.duplicate", &["d"]),
            ("track.arm", &["r"]),
            ("track.mute", &["m"]),
            ("view.toggle_mixer", &["f3"]),
            ("view.musical_typing", &["capslock"]),
        ],
    },
    Preset {
        name: "Bitwig",
        keys: &[
            ("transport.loop_toggle", &["\\"]),
            ("midi.paint_pen", &["3", "shift+3"]),
            ("track.add_instrument", &["primary+t"]),
            ("track.add_audio", &[]),
            ("track.arm", &["a"]),
            ("track.mute", &["m"]),
            ("view.toggle_mixer", &["f4"]),
            ("view.musical_typing", &["capslock"]),
        ],
    },
    Preset {
        name: "Pro Tools",
        keys: &[
            ("transport.record", &["r", "f12"]),
            ("transport.loop_selection", &["primary+shift+l"]),
            ("transport.loop_toggle", &["primary+shift+l"]),
            ("clip.consolidate", &["shift+alt+3"]),
            ("clip.bounce_to_track", &[]),
            ("midi.paint_pen", &["f10", "primary+6"]),
            ("clip.quantize", &["alt+0", "q"]),
            ("track.duplicate_with_clips", &["shift+alt+d"]),
            ("track.duplicate", &[]),
            ("track.add_instrument", &[]),
            ("track.add_audio", &["primary+shift+n"]),
            ("track.arm", &["shift+r"]),
            ("track.mute", &["shift+m"]),
            ("track.solo", &["shift+s"]),
            ("track.monitor", &["shift+i"]),
            ("view.toggle_mixer", &["primary+="]),
            // ⌘= opens the mixer here, so the interface zooms with ⌘+ or ⌃=.
            ("view.ui_zoom_in", &["primary+plus", "ctrl+="]),
            ("view.fit_width", &["alt+a"]),
            ("view.musical_typing", &["shift+k"]),
        ],
    },
    Preset {
        name: "BandLab",
        keys: &[
            ("transport.loop_toggle", &["c"]),
            ("clip.split_at_range", &["s"]),
            ("clip.split_at_marker", &["s"]),
            ("track.duplicate_with_clips", &["primary+shift+d", "shift+d"]),
            ("track.arm", &["r"]),
            ("track.mute", &["shift+m"]),
            ("track.solo", &["shift+s"]),
        ],
    },
];

/// The preset called `name` (an unknown or empty name is the default).
pub fn find(name: &str) -> &'static Preset {
    PRESETS.iter().find(|p| p.name == name).unwrap_or(&PRESETS[0])
}

/// The default keymap with the preset's keys applied.
pub fn keymap(name: &str) -> dstudio_core::keymap::Keymap {
    let mut map = crate::app_keymap();
    for (action, chords) in find(name).keys {
        let chords = chords.iter().map(|c| dstudio_core::keymap::Chord::parse(c).expect("valid preset chord")).collect();
        map.set_chords(action, chords);
    }
    map
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_preset_parses_names_known_actions_and_has_no_conflicts() {
        let defaults = crate::app_keymap();
        for p in PRESETS {
            for (action, _) in p.keys {
                assert!(defaults.condition_of(action).is_some(), "{}: {action} is not an action", p.name);
            }
            let map = keymap(p.name);
            assert!(map.conflicts().is_empty(), "{}: {:?}", p.name, map.conflicts());
        }
    }

    #[test]
    fn presets_change_the_keys_they_list() {
        use dstudio_core::keymap::{Chord, Context};
        let ch = |s: &str| Chord::parse(s).unwrap();
        let ctx = Context::default();
        assert_eq!(keymap("Device Studio"), crate::app_keymap());
        assert_eq!(keymap("nonsense"), crate::app_keymap());
        assert_eq!(keymap("Logic Pro").resolve(&ch("c"), &ctx), Some("transport.loop_toggle"));
        assert_eq!(keymap("Logic Pro").resolve(&ch("x"), &ctx), Some("view.toggle_mixer"));
        assert_eq!(keymap("Ableton Live").resolve(&ch("f9"), &ctx), Some("transport.record"));
        assert_eq!(keymap("FL Studio").resolve(&ch("f9"), &ctx), Some("view.toggle_mixer"));
        assert_eq!(keymap("Pro Tools").resolve(&ch("primary+="), &ctx), Some("view.toggle_mixer"));
        assert!(keymap("Logic Pro").chords_for("track.fold").is_empty());
    }
}
