//! Device chain presets: a chain head (`builtin::rack`) with its members,
//! saved as one unit and added back as one unit.
//!
//! * [ChainPreset] is what a saved chain file holds: the head and its members
//!   in order, with their parameters, macro names and macro links. Links that
//!   aim outside the chain are dropped when the preset is taken from a track.
//! * [ChainPreset::instantiate] gives the devices fresh ids (from a first id)
//!   and points the members and the macro links at them.
//! * [factory] lists DStudio's own chains, built from the built-in effects
//!   and grouped by folder ([FOLDERS]).

use crate::builtin::{self, rack};
use crate::device::{Device, DeviceId, ModLink, ModParam};
use crate::model::Track;
use crate::modulation;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Folders of the factory chains, in browser order.
pub const FOLDERS: [&str; 8] = ["Vocals", "Drums", "Bass", "Guitar", "Keys & Synths", "Pads & Textures", "FX & Glitch", "Mix & Master"];

/// File format version written by [ChainPreset::to_json].
pub const VERSION: u32 = 1;

/// A chain as saved: head first, then its members.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChainPreset {
    #[serde(default = "version")]
    pub version: u32,
    pub name: String,
    pub devices: Vec<Device>,
}

fn version() -> u32 {
    VERSION
}

impl ChainPreset {
    /// The chain headed by `head` on `track`, named `name`. None when `head`
    /// isn't a chain head there.
    pub fn from_track(track: &Track, head: DeviceId, name: &str) -> Option<ChainPreset> {
        let hi = track.devices.iter().position(|d| d.id == head && modulation::is_rack(d))?;
        let run = modulation::members(&track.devices, hi);
        let mut devices: Vec<Device> = std::iter::once(hi).chain(run).map(|i| track.devices[i].clone()).collect();
        let ids: Vec<DeviceId> = devices.iter().map(|d| d.id).collect();
        for d in &mut devices {
            d.links.retain(|l| ids[1..].contains(&l.target));
            d.sidechain = None;
        }
        devices[0].label = Some(name.to_string());
        devices[0].preset = Some(name.to_string());
        Some(ChainPreset { version: VERSION, name: name.to_string(), devices })
    }

    /// The devices with ids `first`, `first + 1`, …: members point at the new
    /// head, links at the new members. The head shows the preset name.
    pub fn instantiate(&self, first: u64) -> Vec<Device> {
        let map: HashMap<DeviceId, DeviceId> = self.devices.iter().enumerate().map(|(i, d)| (d.id, DeviceId(first + i as u64))).collect();
        let head = DeviceId(first);
        self.devices
            .iter()
            .enumerate()
            .map(|(i, d)| {
                let mut d = d.clone();
                d.id = DeviceId(first + i as u64);
                d.chain = (i > 0).then_some(head);
                d.links.retain_mut(|l| match map.get(&l.target) {
                    Some(t) if *t != head => {
                        l.target = *t;
                        true
                    }
                    _ => false,
                });
                if i == 0 {
                    d.label = Some(self.name.clone());
                    d.preset = Some(self.name.clone());
                }
                d
            })
            .collect()
    }

    /// Whether the preset is a usable chain: a head followed by effects only.
    pub fn is_valid(&self) -> bool {
        matches!(self.devices.first(), Some(h) if modulation::is_rack(h))
            && self.devices[1..].iter().all(|d| !modulation::is_rack(d) && !modulation::is_lfo(d))
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_default()
    }

    pub fn from_json(s: &str) -> Option<ChainPreset> {
        serde_json::from_str::<ChainPreset>(s).ok().filter(|p| p.is_valid() && !p.name.trim().is_empty())
    }
}

/// A file name for a chain called `name` (no path separators or dots in front).
pub fn file_name(name: &str) -> String {
    let base: String =
        name.trim().chars().map(|c| if c.is_alphanumeric() || matches!(c, ' ' | '-' | '_' | '&' | '(' | ')') { c } else { '_' }).collect();
    let base = base.trim_matches(|c: char| c == ' ' || c == '_');
    format!("{}.json", if base.is_empty() { "Chain" } else { base })
}

// ---------------------------------------------------------------- factory chains

/// One member of a factory chain: built-in id and plain values.
struct Member {
    id: &'static str,
    values: &'static [(&'static str, f32)],
}

/// One macro: name, position (0..100) and links (member index, parameter,
/// plain value at macro 0, plain value at macro 100).
struct Macro {
    name: &'static str,
    at: f32,
    links: &'static [(usize, &'static str, f32, f32)],
}

struct Factory {
    folder: &'static str,
    name: &'static str,
    members: &'static [Member],
    macros: &'static [Macro],
}

const fn m(id: &'static str, values: &'static [(&'static str, f32)]) -> Member {
    Member { id, values }
}

const fn k(name: &'static str, at: f32, links: &'static [(usize, &'static str, f32, f32)]) -> Macro {
    Macro { name, at, links }
}

#[rustfmt::skip]
const FACTORY: &[Factory] = &[
    // ---- Vocals
    Factory { folder: "Vocals", name: "Lead Vocal Polish", members: &[
        m("deesser", &[("frequency", 6500.0), ("threshold", -30.0), ("reduction", 6.0)]),
        m("compressor", &[("threshold", -20.0), ("ratio", 3.5), ("attack", 8.0), ("release", 140.0), ("makeup_gain", 3.0)]),
        m("tapesaturation", &[("drive", 18.0), ("warmth", 35.0), ("compression", 20.0), ("mix", 60.0)]),
        m("reverb", &[("size", 35.0), ("decay", 1.4), ("predelay", 25.0), ("mix", 14.0), ("lowcut2", 200.0)]),
    ], macros: &[
        k("Sibilance", 30.0, &[(0, "reduction", 0.0, 16.0)]),
        k("Squash", 40.0, &[(1, "threshold", -8.0, -32.0)]),
        k("Warmth", 30.0, &[(2, "drive", 0.0, 50.0)]),
        k("Space", 20.0, &[(3, "mix", 0.0, 60.0)]),
    ] },
    Factory { folder: "Vocals", name: "Close & Warm", members: &[
        m("compressor", &[("threshold", -18.0), ("ratio", 2.5), ("attack", 20.0), ("release", 200.0), ("makeup_gain", 2.0)]),
        m("tapesaturation", &[("drive", 30.0), ("warmth", 60.0), ("compression", 30.0)]),
        m("reverb", &[("size", 20.0), ("decay", 0.8), ("damping", 70.0), ("mix", 10.0)]),
    ], macros: &[
        k("Body", 50.0, &[(1, "warmth", 20.0, 90.0)]),
        k("Room", 20.0, &[(2, "mix", 0.0, 40.0)]),
    ] },
    Factory { folder: "Vocals", name: "Wide Double", members: &[
        m("chorus", &[("rate", 0.35), ("depth", 2.5), ("mix", 35.0), ("width", 100.0)]),
        m("widener", &[("width2", 260.0), ("lowcut", 250.0)]),
        m("delay", &[("feedback", 20.0), ("mix", 12.0), ("timeMs", 90.0), ("timeMsR", 120.0)]),
    ], macros: &[
        k("Width", 50.0, &[(1, "width2", 100.0, 400.0)]),
        k("Thickness", 35.0, &[(0, "mix", 0.0, 100.0)]),
    ] },
    Factory { folder: "Vocals", name: "Phone Line", members: &[
        m("auto-filter", &[("frequency", 2600.0), ("resonance", 0.4), ("drive", 8.0)]),
        m("distortion", &[("drive", 35.0), ("tone", 3000.0), ("mix", 70.0)]),
        m("bitcrusher", &[("bitdepth", 10.0), ("downsample", 3.0), ("mix", 50.0)]),
    ], macros: &[
        k("Crunch", 35.0, &[(1, "drive", 0.0, 80.0)]),
        k("Lo-Fi", 50.0, &[(2, "mix", 0.0, 100.0)]),
    ] },
    Factory { folder: "Vocals", name: "Ad-Lib Throw", members: &[
        m("pitch-shifter", &[("semitones", 12.0), ("mix", 25.0)]),
        m("delay", &[("feedback", 45.0), ("mix", 30.0), ("timeMs", 375.0), ("timeMsR", 500.0)]),
        m("reverb", &[("size", 70.0), ("decay", 3.5), ("mix", 30.0), ("lowcut2", 300.0)]),
    ], macros: &[
        k("Octave", 25.0, &[(0, "mix", 0.0, 100.0)]),
        k("Echoes", 30.0, &[(1, "feedback", 0.0, 85.0)]),
        k("Space", 30.0, &[(2, "mix", 0.0, 80.0)]),
    ] },
    // ---- Drums
    Factory { folder: "Drums", name: "Punchy Kit", members: &[
        m("transient-shaper", &[("attack", 40.0), ("sustain", -15.0)]),
        m("glue-compressor", &[("threshold", -18.0), ("makeup", 3.0), ("mix", 80.0)]),
        m("soft-clipper", &[("input", 3.0), ("softness", 60.0)]),
    ], macros: &[
        k("Punch", 60.0, &[(0, "attack", 0.0, 80.0)]),
        k("Glue", 40.0, &[(1, "threshold", -4.0, -30.0)]),
        k("Clip", 25.0, &[(2, "input", 0.0, 12.0)]),
    ] },
    Factory { folder: "Drums", name: "Room Smash", members: &[
        m("compressor", &[("threshold", -35.0), ("ratio", 12.0), ("attack", 2.0), ("release", 80.0), ("makeup_gain", 10.0), ("mix", 40.0)]),
        m("distortion", &[("drive", 25.0), ("tone", 6000.0), ("mix", 40.0)]),
        m("reverb", &[("size", 30.0), ("decay", 0.9), ("mix", 18.0)]),
    ], macros: &[
        k("Smash", 40.0, &[(0, "mix", 0.0, 100.0)]),
        k("Dirt", 30.0, &[(1, "drive", 0.0, 70.0)]),
        k("Room", 25.0, &[(2, "mix", 0.0, 50.0)]),
    ] },
    Factory { folder: "Drums", name: "Dusty Break", members: &[
        m("bitcrusher", &[("bitdepth", 12.0), ("downsample", 2.0), ("mix", 60.0)]),
        m("auto-filter", &[("frequency", 7000.0), ("resonance", 0.2)]),
        m("tapesaturation", &[("drive", 40.0), ("warmth", 55.0), ("noise", -42.0)]),
        m("glue-compressor", &[("threshold", -14.0), ("makeup", 2.0)]),
    ], macros: &[
        k("Dust", 50.0, &[(0, "mix", 0.0, 100.0)]),
        k("Tone", 60.0, &[(1, "frequency", 1500.0, 20000.0)]),
        k("Tape", 40.0, &[(2, "drive", 0.0, 80.0)]),
    ] },
    Factory { folder: "Drums", name: "Tight Gate", members: &[
        m("gate", &[("threshold", -30.0), ("hold", 20.0), ("release", 60.0)]),
        m("transient-shaper", &[("attack", 25.0), ("sustain", -40.0)]),
    ], macros: &[
        k("Gate", 50.0, &[(0, "threshold", -60.0, -10.0)]),
        k("Decay", 40.0, &[(1, "sustain", 30.0, -80.0)]),
    ] },
    // ---- Bass
    Factory { folder: "Bass", name: "Solid Low End", members: &[
        m("compressor", &[("threshold", -22.0), ("ratio", 4.0), ("attack", 15.0), ("release", 120.0), ("makeup_gain", 3.0)]),
        m("tapesaturation", &[("drive", 25.0), ("warmth", 50.0)]),
        m("utility", &[("width", 100.0), ("bass_mono_freq", 150.0)]),
    ], macros: &[
        k("Control", 40.0, &[(0, "threshold", -6.0, -36.0)]),
        k("Harmonics", 30.0, &[(1, "drive", 0.0, 70.0)]),
    ] },
    Factory { folder: "Bass", name: "Growl", members: &[
        m("distortion", &[("drive", 55.0), ("tone", 2500.0), ("mix", 70.0)]),
        m("auto-filter", &[("frequency", 1800.0), ("resonance", 0.6), ("lfo_amount", 18.0), ("lfo_rate_free", 2.0)]),
        m("compressor", &[("threshold", -20.0), ("ratio", 6.0), ("attack", 5.0), ("release", 90.0), ("makeup_gain", 2.0)]),
    ], macros: &[
        k("Drive", 55.0, &[(0, "drive", 10.0, 100.0)]),
        k("Cutoff", 40.0, &[(1, "frequency", 200.0, 8000.0)]),
        k("Wobble", 25.0, &[(1, "lfo_amount", 0.0, 48.0)]),
    ] },
    Factory { folder: "Bass", name: "Sub Focus", members: &[
        m("auto-filter", &[("frequency", 400.0), ("resonance", 0.1)]),
        m("utility", &[("width", 0.0), ("gain", 2.0)]),
        m("limiter", &[("ceiling", -1.0), ("input_gain", 3.0)]),
    ], macros: &[
        k("Tone", 30.0, &[(0, "frequency", 80.0, 2000.0)]),
        k("Weight", 30.0, &[(2, "input_gain", 0.0, 12.0)]),
    ] },
    // ---- Guitar
    Factory { folder: "Guitar", name: "Clean Shimmer", members: &[
        m("compressor", &[("threshold", -20.0), ("ratio", 3.0), ("attack", 20.0), ("release", 150.0)]),
        m("chorus", &[("rate", 0.6), ("depth", 3.0), ("mix", 30.0)]),
        m("reverb", &[("size", 60.0), ("decay", 3.0), ("mix", 25.0), ("shimmer", 40.0)]),
    ], macros: &[
        k("Chorus", 30.0, &[(1, "mix", 0.0, 80.0)]),
        k("Space", 25.0, &[(2, "mix", 0.0, 70.0)]),
    ] },
    Factory { folder: "Guitar", name: "Crunch Amp", members: &[
        m("distortion", &[("drive", 45.0), ("tone", 3500.0), ("bias", 15.0)]),
        m("auto-filter", &[("frequency", 6000.0), ("resonance", 0.3)]),
        m("reverb", &[("size", 25.0), ("decay", 0.7), ("mix", 12.0)]),
    ], macros: &[
        k("Gain", 45.0, &[(0, "drive", 5.0, 95.0)]),
        k("Tone", 50.0, &[(0, "tone", 800.0, 9000.0)]),
        k("Room", 15.0, &[(2, "mix", 0.0, 50.0)]),
    ] },
    Factory { folder: "Guitar", name: "Slapback", members: &[
        m("tapesaturation", &[("drive", 20.0), ("warmth", 45.0)]),
        m("delay", &[("feedback", 15.0), ("mix", 28.0), ("timeMs", 110.0), ("timeMsR", 110.0), ("grit", 20.0)]),
    ], macros: &[
        k("Slap", 30.0, &[(1, "mix", 0.0, 60.0)]),
        k("Repeats", 15.0, &[(1, "feedback", 0.0, 70.0)]),
    ] },
    // ---- Keys & Synths
    Factory { folder: "Keys & Synths", name: "Vintage Keys", members: &[
        m("tremolo", &[("rate", 4.5), ("depth", 35.0), ("stereo_phase", 90.0)]),
        m("tapesaturation", &[("drive", 25.0), ("warmth", 55.0), ("noise", -50.0)]),
        m("chorus", &[("rate", 0.4), ("depth", 2.0), ("mix", 25.0)]),
    ], macros: &[
        k("Wobble", 35.0, &[(0, "depth", 0.0, 90.0)]),
        k("Age", 30.0, &[(1, "drive", 0.0, 70.0)]),
    ] },
    Factory { folder: "Keys & Synths", name: "Wide Lead", members: &[
        m("chorus", &[("rate", 0.8), ("depth", 3.5), ("mix", 30.0)]),
        m("delay", &[("feedback", 35.0), ("mix", 20.0), ("timeMs", 250.0), ("timeMsR", 375.0)]),
        m("widener", &[("width2", 240.0)]),
    ], macros: &[
        k("Width", 40.0, &[(2, "width2", 100.0, 400.0)]),
        k("Echo", 25.0, &[(1, "mix", 0.0, 60.0)]),
    ] },
    Factory { folder: "Keys & Synths", name: "Phase Drift", members: &[
        m("phaser", &[("rate", 0.25), ("depth", 70.0), ("feedback", 50.0), ("mix", 50.0)]),
        m("reverb", &[("size", 50.0), ("decay", 2.2), ("mix", 20.0)]),
    ], macros: &[
        k("Swirl", 50.0, &[(0, "depth", 0.0, 100.0)]),
        k("Speed", 20.0, &[(0, "rate", 0.05, 4.0)]),
    ] },
    // ---- Pads & Textures
    Factory { folder: "Pads & Textures", name: "Endless Pad", members: &[
        m("chorus", &[("rate", 0.2), ("depth", 4.0), ("mix", 40.0)]),
        m("reverb", &[("size", 90.0), ("decay", 8.0), ("mix", 45.0), ("shimmer", 60.0), ("lowcut2", 150.0)]),
    ], macros: &[
        k("Size", 80.0, &[(1, "decay", 1.0, 10.0)]),
        k("Wash", 45.0, &[(1, "mix", 0.0, 100.0)]),
    ] },
    Factory { folder: "Pads & Textures", name: "Grit Texture", members: &[
        m("bitcrusher", &[("bitdepth", 8.0), ("downsample", 6.0), ("mix", 45.0)]),
        m("auto-filter", &[("frequency", 3000.0), ("resonance", 0.5), ("lfo_amount", 24.0), ("lfo_rate_free", 0.2)]),
        m("reverb", &[("size", 70.0), ("decay", 4.0), ("mix", 35.0)]),
    ], macros: &[
        k("Grit", 45.0, &[(0, "mix", 0.0, 100.0)]),
        k("Motion", 30.0, &[(1, "lfo_amount", 0.0, 60.0)]),
    ] },
    Factory { folder: "Pads & Textures", name: "Pulse Pad", members: &[
        m("trance-gate", &[]),
        m("delay", &[("feedback", 40.0), ("mix", 25.0)]),
        m("reverb", &[("size", 60.0), ("decay", 3.0), ("mix", 30.0)]),
    ], macros: &[
        k("Echo", 25.0, &[(1, "mix", 0.0, 60.0)]),
        k("Space", 30.0, &[(2, "mix", 0.0, 80.0)]),
    ] },
    // ---- FX & Glitch
    Factory { folder: "FX & Glitch", name: "Riser Sweep", members: &[
        m("auto-filter", &[("frequency", 400.0), ("resonance", 0.7), ("drive", 6.0)]),
        m("reverb", &[("size", 80.0), ("decay", 5.0), ("mix", 30.0)]),
    ], macros: &[
        k("Sweep", 20.0, &[(0, "frequency", 100.0, 18000.0), (1, "mix", 10.0, 70.0)]),
        k("Resonance", 50.0, &[(0, "resonance", 0.0, 1.1)]),
    ] },
    Factory { folder: "FX & Glitch", name: "Tape Stop Echo", members: &[
        m("echo", &[("mix", 40.0), ("intensity", 6.0)]),
        m("auto-filter", &[("frequency", 5000.0)]),
    ], macros: &[
        k("Feedback", 50.0, &[(0, "intensity", 0.0, 10.0)]),
        k("Tone", 60.0, &[(1, "frequency", 500.0, 20000.0)]),
    ] },
    Factory { folder: "FX & Glitch", name: "Radio Fade", members: &[
        m("auto-filter", &[("frequency", 1800.0), ("resonance", 0.5)]),
        m("distortion", &[("drive", 30.0), ("mix", 50.0)]),
        m("utility", &[("width", 30.0)]),
    ], macros: &[
        k("Fade", 50.0, &[(0, "frequency", 20000.0, 800.0), (2, "width", 100.0, 0.0)]),
    ] },
    // ---- Mix & Master
    Factory { folder: "Mix & Master", name: "Bus Glue", members: &[
        m("glue-compressor", &[("threshold", -12.0), ("makeup", 2.0)]),
        m("tapesaturation", &[("drive", 12.0), ("warmth", 40.0), ("compression", 15.0)]),
    ], macros: &[
        k("Glue", 40.0, &[(0, "threshold", -2.0, -24.0)]),
        k("Color", 25.0, &[(1, "drive", 0.0, 50.0)]),
    ] },
    Factory { folder: "Mix & Master", name: "Loud & Clear", members: &[
        m("glue-compressor", &[("threshold", -10.0), ("makeup", 1.5)]),
        m("widener", &[("width2", 220.0), ("lowcut", 200.0)]),
        m("soft-clipper", &[("input", 2.0), ("softness", 70.0)]),
        m("mastering-limiter", &[("threshold", -6.0), ("ceiling", -1.0)]),
    ], macros: &[
        k("Loudness", 40.0, &[(3, "threshold", 0.0, -14.0)]),
        k("Width", 30.0, &[(1, "width2", 100.0, 300.0)]),
    ] },
    Factory { folder: "Mix & Master", name: "Gentle Master", members: &[
        m("compressor", &[("threshold", -14.0), ("ratio", 1.8), ("attack", 30.0), ("release", 250.0), ("makeup_gain", 1.0)]),
        m("limiter", &[("ceiling", -1.0), ("input_gain", 2.0)]),
    ], macros: &[
        k("Drive", 25.0, &[(1, "input_gain", 0.0, 10.0)]),
    ] },
    // ---- Vocals (more)
    Factory { folder: "Vocals", name: "Radio Ready Vox", members: &[
        m("channeleq", &[("band0_enabled", 1.0), ("band0_freq", 90.0), ("band5_freq", 3200.0), ("band5_gain2", 2.5)]),
        m("compressor", &[("threshold", -22.0), ("ratio", 4.0), ("attack", 5.0), ("release", 90.0), ("makeup_gain", 4.0)]),
        m("deesser", &[("frequency", 7200.0), ("threshold", -28.0), ("reduction", 7.0)]),
        m("limiter", &[("ceiling", -1.0), ("input_gain", 2.0)]),
    ], macros: &[
        k("Presence", 40.0, &[(0, "band5_gain2", 0.0, 6.0)]),
        k("Level", 40.0, &[(1, "threshold", -10.0, -34.0)]),
        k("Sibilance", 30.0, &[(2, "reduction", 0.0, 15.0)]),
    ] },
    Factory { folder: "Vocals", name: "Airy Pop Vocal", members: &[
        m("compressor", &[("threshold", -18.0), ("ratio", 3.0), ("attack", 12.0), ("release", 160.0), ("makeup_gain", 2.5)]),
        m("channeleq", &[("band6_gain2", 3.0)]),
        m("delay", &[("feedback", 25.0), ("mix", 10.0), ("timeMs", 220.0), ("timeMsR", 330.0)]),
        m("reverb", &[("size", 55.0), ("decay", 2.4), ("predelay", 35.0), ("mix", 16.0), ("lowcut2", 250.0)]),
    ], macros: &[
        k("Air", 45.0, &[(1, "band6_gain2", 0.0, 7.0)]),
        k("Echo", 20.0, &[(2, "mix", 0.0, 40.0)]),
        k("Space", 25.0, &[(3, "mix", 0.0, 50.0)]),
    ] },
    Factory { folder: "Vocals", name: "Rap Vocal Upfront", members: &[
        m("character-compressor", &[("threshold", -24.0), ("ratio", 6.0), ("attack", 3.0), ("release", 80.0), ("makeup", 5.0)]),
        m("deesser", &[("frequency", 6800.0), ("threshold", -30.0), ("reduction", 8.0)]),
        m("soft-clipper", &[("input", 2.0), ("softness", 70.0)]),
    ], macros: &[
        k("Punch", 50.0, &[(0, "threshold", -12.0, -36.0)]),
        k("Edge", 30.0, &[(2, "input", 0.0, 9.0)]),
    ] },
    Factory { folder: "Vocals", name: "Dream Vocal", members: &[
        m("chorus", &[("rate", 0.3), ("depth", 3.0), ("mix", 30.0)]),
        m("delay", &[("feedback", 40.0), ("mix", 22.0), ("timeMs", 410.0), ("timeMsR", 520.0), ("modDepth", 20.0)]),
        m("reverb", &[("size", 85.0), ("decay", 5.5), ("mix", 35.0), ("shimmer", 30.0), ("lowcut2", 300.0)]),
    ], macros: &[
        k("Haze", 40.0, &[(0, "mix", 0.0, 70.0)]),
        k("Trails", 35.0, &[(1, "feedback", 10.0, 75.0)]),
        k("Space", 40.0, &[(2, "mix", 10.0, 70.0)]),
    ] },
    Factory { folder: "Vocals", name: "Backing Choir", members: &[
        m("widener", &[("width2", 300.0), ("lowcut", 300.0)]),
        m("chorus", &[("rate", 0.25), ("depth", 5.0), ("mix", 45.0), ("voices", 2.0)]),
        m("reverb", &[("size", 65.0), ("decay", 3.0), ("mix", 30.0), ("lowcut2", 250.0)]),
    ], macros: &[
        k("Width", 55.0, &[(0, "width2", 100.0, 400.0)]),
        k("Voices", 40.0, &[(1, "mix", 0.0, 90.0)]),
        k("Hall", 30.0, &[(2, "mix", 0.0, 70.0)]),
    ] },
    Factory { folder: "Vocals", name: "Robot Voice", members: &[
        m("pitch-corrector", &[("speed", 100.0), ("retune_speed_ms", 0.0), ("humanize", 0.0)]),
        m("ring-mod", &[("frequency", 120.0), ("mix", 30.0)]),
        m("bitcrusher", &[("bitdepth", 10.0), ("downsample", 2.0), ("mix", 40.0)]),
    ], macros: &[
        k("Metal", 30.0, &[(1, "mix", 0.0, 80.0)]),
        k("Pitch", 20.0, &[(1, "frequency", 40.0, 800.0)]),
        k("Crush", 40.0, &[(2, "mix", 0.0, 100.0)]),
    ] },
    // ---- Drums (more)
    Factory { folder: "Drums", name: "Kick Weight", members: &[
        m("channeleq", &[("band1_freq", 60.0), ("band1_gain2", 3.0), ("band3_freq", 300.0), ("band3_gain2", -3.0)]),
        m("transient-shaper", &[("attack", 30.0), ("sustain", -10.0)]),
        m("soft-clipper", &[("input", 2.0), ("softness", 55.0)]),
    ], macros: &[
        k("Thump", 50.0, &[(0, "band1_gain2", 0.0, 8.0)]),
        k("Click", 40.0, &[(1, "attack", 0.0, 80.0)]),
        k("Clip", 20.0, &[(2, "input", 0.0, 10.0)]),
    ] },
    Factory { folder: "Drums", name: "Snare Crack", members: &[
        m("channeleq", &[("band3_freq", 200.0), ("band3_gain2", 2.5), ("band5_freq", 5000.0), ("band5_gain2", 3.0)]),
        m("compressor", &[("threshold", -20.0), ("ratio", 5.0), ("attack", 6.0), ("release", 70.0), ("makeup_gain", 3.0)]),
        m("reverb", &[("size", 30.0), ("decay", 0.9), ("mix", 15.0), ("mode2", 3.0)]),
    ], macros: &[
        k("Body", 40.0, &[(0, "band3_gain2", 0.0, 6.0)]),
        k("Snap", 40.0, &[(0, "band5_gain2", 0.0, 7.0)]),
        k("Room", 20.0, &[(2, "mix", 0.0, 50.0)]),
    ] },
    Factory { folder: "Drums", name: "Hat Silk", members: &[
        m("channeleq", &[("band0_enabled", 1.0), ("band0_freq", 350.0), ("band6_gain2", 2.0)]),
        m("transient-shaper", &[("attack", -15.0), ("sustain", 10.0)]),
        m("autopanner", &[("depth", 25.0), ("sync", 1.0)]),
    ], macros: &[
        k("Shine", 40.0, &[(0, "band6_gain2", 0.0, 6.0)]),
        k("Soften", 30.0, &[(1, "attack", 0.0, -60.0)]),
        k("Move", 25.0, &[(2, "depth", 0.0, 80.0)]),
    ] },
    Factory { folder: "Drums", name: "Parallel Crush", members: &[
        m("compressor", &[("threshold", -40.0), ("ratio", 20.0), ("attack", 1.0), ("release", 60.0), ("makeup_gain", 14.0), ("mix", 35.0)]),
        m("channeleq", &[("band1_gain2", 2.0), ("band6_gain2", 2.0)]),
    ], macros: &[
        k("Crush", 35.0, &[(0, "mix", 0.0, 100.0)]),
        k("Smile", 30.0, &[(1, "band1_gain2", 0.0, 6.0), (1, "band6_gain2", 0.0, 6.0)]),
    ] },
    Factory { folder: "Drums", name: "Lo-Fi Kit", members: &[
        m("bitcrusher", &[("bitdepth", 9.0), ("downsample", 4.0), ("mix", 50.0)]),
        m("tapesaturation", &[("drive", 45.0), ("warmth", 60.0), ("noise", -38.0)]),
        m("auto-filter", &[("frequency", 5000.0), ("resonance", 0.15)]),
    ], macros: &[
        k("Crunch", 50.0, &[(0, "mix", 0.0, 100.0)]),
        k("Tape", 45.0, &[(1, "drive", 0.0, 90.0)]),
        k("Muffle", 40.0, &[(2, "frequency", 20000.0, 1200.0)]),
    ] },
    Factory { folder: "Drums", name: "Big Room Drums", members: &[
        m("glue-compressor", &[("threshold", -16.0), ("makeup", 3.0)]),
        m("reverb", &[("size", 75.0), ("decay", 2.0), ("mix", 22.0), ("mode2", 2.0), ("lowcut2", 180.0)]),
        m("limiter", &[("ceiling", -1.0), ("input_gain", 3.0)]),
    ], macros: &[
        k("Glue", 40.0, &[(0, "threshold", -4.0, -30.0)]),
        k("Hall", 30.0, &[(1, "mix", 0.0, 60.0)]),
        k("Push", 30.0, &[(2, "input_gain", 0.0, 10.0)]),
    ] },
    Factory { folder: "Drums", name: "Ducked Loop", members: &[
        m("shaper", &[("vol_depth", 60.0)]),
        m("glue-compressor", &[("threshold", -12.0), ("makeup", 2.0)]),
    ], macros: &[
        k("Pump", 50.0, &[(0, "vol_depth", 0.0, 100.0)]),
        k("Glue", 30.0, &[(1, "threshold", -2.0, -24.0)]),
    ] },
    // ---- Bass (more)
    Factory { folder: "Bass", name: "Round Fingers", members: &[
        m("channeleq", &[("band1_freq", 80.0), ("band1_gain2", 2.0), ("band4_freq", 800.0), ("band4_gain2", 1.5)]),
        m("compressor", &[("threshold", -20.0), ("ratio", 3.0), ("attack", 25.0), ("release", 160.0), ("makeup_gain", 2.0)]),
        m("tapesaturation", &[("drive", 18.0), ("warmth", 65.0)]),
    ], macros: &[
        k("Bottom", 40.0, &[(0, "band1_gain2", 0.0, 6.0)]),
        k("Growl", 30.0, &[(0, "band4_gain2", 0.0, 6.0)]),
        k("Warmth", 35.0, &[(2, "drive", 0.0, 60.0)]),
    ] },
    Factory { folder: "Bass", name: "Reese Wide", members: &[
        m("chorus", &[("rate", 0.15), ("depth", 4.0), ("mix", 40.0)]),
        m("utility", &[("bass_mono", 1.0), ("bass_mono_freq", 140.0), ("width", 160.0)]),
        m("distortion", &[("drive", 25.0), ("tone", 3000.0), ("mix", 40.0)]),
    ], macros: &[
        k("Detune", 40.0, &[(0, "mix", 0.0, 80.0)]),
        k("Width", 40.0, &[(1, "width", 100.0, 250.0)]),
        k("Dirt", 30.0, &[(2, "mix", 0.0, 80.0)]),
    ] },
    Factory { folder: "Bass", name: "808 Thick", members: &[
        m("soft-clipper", &[("input", 4.0), ("softness", 45.0)]),
        m("tapesaturation", &[("drive", 30.0), ("warmth", 50.0)]),
        m("utility", &[("bass_mono", 1.0), ("bass_mono_freq", 200.0)]),
        m("limiter", &[("ceiling", -1.0), ("input_gain", 2.0)]),
    ], macros: &[
        k("Saturate", 40.0, &[(0, "input", 0.0, 12.0)]),
        k("Harmonics", 30.0, &[(1, "drive", 0.0, 70.0)]),
        k("Loud", 25.0, &[(3, "input_gain", 0.0, 9.0)]),
    ] },
    Factory { folder: "Bass", name: "Acid Squelch", members: &[
        m("auto-filter", &[("frequency", 900.0), ("resonance", 0.9), ("env_amount", 40.0), ("env_attack", 2.0), ("env_release", 180.0), ("drive", 10.0)]),
        m("distortion", &[("drive", 40.0), ("tone", 5000.0), ("mix", 50.0)]),
        m("delay", &[("feedback", 30.0), ("mix", 12.0), ("timeMs", 187.0), ("timeMsR", 250.0)]),
    ], macros: &[
        k("Cutoff", 35.0, &[(0, "frequency", 150.0, 6000.0)]),
        k("Squelch", 50.0, &[(0, "resonance", 0.2, 1.2)]),
        k("Acid", 40.0, &[(1, "mix", 0.0, 90.0)]),
    ] },
    Factory { folder: "Bass", name: "Funk Pluck", members: &[
        m("auto-filter", &[("frequency", 1200.0), ("resonance", 0.5), ("env_amount", 30.0), ("env_release", 120.0)]),
        m("compressor", &[("threshold", -18.0), ("ratio", 4.0), ("attack", 3.0), ("release", 80.0), ("makeup_gain", 3.0)]),
    ], macros: &[
        k("Quack", 45.0, &[(0, "env_amount", 0.0, 60.0)]),
        k("Snap", 40.0, &[(1, "threshold", -6.0, -30.0)]),
    ] },
    Factory { folder: "Bass", name: "Dub Sub", members: &[
        m("auto-filter", &[("frequency", 300.0), ("resonance", 0.2)]),
        m("tapesaturation", &[("drive", 20.0), ("warmth", 75.0)]),
        m("utility", &[("width", 0.0)]),
    ], macros: &[
        k("Depth", 30.0, &[(0, "frequency", 80.0, 1200.0)]),
        k("Warmth", 40.0, &[(1, "warmth", 20.0, 100.0)]),
    ] },
    Factory { folder: "Bass", name: "Fuzz Bass", members: &[
        m("distortion", &[("drive", 70.0), ("tone", 2200.0), ("mode", 2.0), ("mix", 60.0)]),
        m("channeleq", &[("band3_freq", 400.0), ("band3_gain2", -3.0)]),
        m("compressor", &[("threshold", -16.0), ("ratio", 4.0), ("attack", 10.0), ("release", 100.0)]),
    ], macros: &[
        k("Fuzz", 55.0, &[(0, "drive", 20.0, 100.0)]),
        k("Blend", 50.0, &[(0, "mix", 0.0, 100.0)]),
        k("Tone", 40.0, &[(0, "tone", 600.0, 6000.0)]),
    ] },
    // ---- Guitar (more)
    Factory { folder: "Guitar", name: "Acoustic Sparkle", members: &[
        m("channeleq", &[("band0_enabled", 1.0), ("band0_freq", 100.0), ("band3_gain2", -2.0), ("band6_gain2", 2.5)]),
        m("compressor", &[("threshold", -18.0), ("ratio", 2.5), ("attack", 15.0), ("release", 150.0), ("makeup_gain", 2.0)]),
        m("reverb", &[("size", 40.0), ("decay", 1.6), ("mix", 15.0)]),
    ], macros: &[
        k("Sparkle", 45.0, &[(0, "band6_gain2", 0.0, 7.0)]),
        k("Even", 40.0, &[(1, "threshold", -6.0, -30.0)]),
        k("Room", 25.0, &[(2, "mix", 0.0, 50.0)]),
    ] },
    Factory { folder: "Guitar", name: "Lead Stack", members: &[
        m("distortion", &[("drive", 65.0), ("tone", 3200.0), ("bias", 10.0)]),
        m("delay", &[("feedback", 30.0), ("mix", 18.0), ("timeMs", 380.0), ("timeMsR", 380.0)]),
        m("reverb", &[("size", 50.0), ("decay", 2.0), ("mix", 18.0)]),
    ], macros: &[
        k("Gain", 60.0, &[(0, "drive", 20.0, 100.0)]),
        k("Echo", 30.0, &[(1, "mix", 0.0, 50.0)]),
        k("Hall", 25.0, &[(2, "mix", 0.0, 50.0)]),
    ] },
    Factory { folder: "Guitar", name: "Surf Spring", members: &[
        m("tremolo", &[("rate", 5.5), ("depth", 40.0)]),
        m("reverb", &[("size", 45.0), ("decay", 2.6), ("damping", 30.0), ("mix", 35.0), ("mode2", 4.0)]),
    ], macros: &[
        k("Wobble", 40.0, &[(0, "depth", 0.0, 90.0)]),
        k("Drip", 45.0, &[(1, "mix", 10.0, 70.0)]),
    ] },
    Factory { folder: "Guitar", name: "Ambient Swell", members: &[
        m("compressor", &[("threshold", -28.0), ("ratio", 3.0), ("attack", 80.0), ("release", 300.0), ("makeup_gain", 4.0)]),
        m("delay", &[("feedback", 55.0), ("mix", 30.0), ("timeMs", 600.0), ("timeMsR", 800.0), ("modDepth", 25.0)]),
        m("reverb", &[("size", 95.0), ("decay", 9.0), ("mix", 50.0), ("shimmer", 50.0)]),
    ], macros: &[
        k("Swell", 50.0, &[(0, "attack", 10.0, 300.0)]),
        k("Repeats", 45.0, &[(1, "feedback", 10.0, 85.0)]),
        k("Cloud", 50.0, &[(2, "mix", 20.0, 90.0)]),
    ] },
    Factory { folder: "Guitar", name: "Funk Clean", members: &[
        m("compressor", &[("threshold", -24.0), ("ratio", 6.0), ("attack", 2.0), ("release", 60.0), ("makeup_gain", 5.0)]),
        m("auto-filter", &[("frequency", 1500.0), ("resonance", 0.6), ("env_amount", 35.0), ("filter_type", 2.0)]),
    ], macros: &[
        k("Squeeze", 50.0, &[(0, "threshold", -8.0, -36.0)]),
        k("Wah", 40.0, &[(1, "env_amount", 0.0, 60.0)]),
    ] },
    Factory { folder: "Guitar", name: "Doom Fuzz", members: &[
        m("distortion", &[("drive", 90.0), ("tone", 1600.0), ("mode", 2.0)]),
        m("auto-filter", &[("frequency", 4000.0), ("resonance", 0.2)]),
        m("reverb", &[("size", 60.0), ("decay", 2.5), ("mix", 15.0)]),
    ], macros: &[
        k("Fuzz", 80.0, &[(0, "drive", 30.0, 100.0)]),
        k("Dark", 40.0, &[(1, "frequency", 12000.0, 900.0)]),
    ] },
    Factory { folder: "Guitar", name: "Chorus Arps", members: &[
        m("chorus", &[("rate", 0.9), ("depth", 3.5), ("mix", 45.0), ("voices", 1.0)]),
        m("delay", &[("feedback", 35.0), ("mix", 20.0), ("timeMs", 300.0), ("timeMsR", 450.0)]),
    ], macros: &[
        k("Shimmer", 45.0, &[(0, "mix", 0.0, 90.0)]),
        k("Echo", 30.0, &[(1, "mix", 0.0, 60.0)]),
    ] },
    // ---- Keys & Synths (more)
    Factory { folder: "Keys & Synths", name: "Electric Piano Glow", members: &[
        m("autopanner", &[("depth", 40.0), ("rate", 3.0), ("sync", 0.0)]),
        m("tapesaturation", &[("drive", 20.0), ("warmth", 60.0)]),
        m("reverb", &[("size", 45.0), ("decay", 1.8), ("mix", 18.0)]),
    ], macros: &[
        k("Pan", 40.0, &[(0, "depth", 0.0, 90.0)]),
        k("Glow", 35.0, &[(1, "warmth", 10.0, 90.0)]),
        k("Room", 25.0, &[(2, "mix", 0.0, 50.0)]),
    ] },
    Factory { folder: "Keys & Synths", name: "Organ Spin", members: &[
        m("character-mod", &[("model", 8.0), ("depth", 60.0), ("mix", 70.0)]),
        m("distortion", &[("drive", 20.0), ("tone", 5000.0), ("mix", 40.0)]),
    ], macros: &[
        k("Spin", 50.0, &[(0, "depth", 0.0, 100.0)]),
        k("Growl", 30.0, &[(1, "drive", 0.0, 70.0)]),
    ] },
    Factory { folder: "Keys & Synths", name: "Supersaw Lift", members: &[
        m("widener", &[("width2", 280.0)]),
        m("channeleq", &[("band0_enabled", 1.0), ("band0_freq", 150.0), ("band6_gain2", 2.0)]),
        m("reverb", &[("size", 70.0), ("decay", 3.0), ("mix", 25.0)]),
    ], macros: &[
        k("Width", 50.0, &[(0, "width2", 100.0, 400.0)]),
        k("Bright", 40.0, &[(1, "band6_gain2", 0.0, 7.0)]),
        k("Space", 30.0, &[(2, "mix", 0.0, 60.0)]),
    ] },
    Factory { folder: "Keys & Synths", name: "Pluck Echoes", members: &[
        m("delay", &[("feedback", 45.0), ("mix", 28.0), ("timeMs", 250.0), ("timeMsR", 375.0), ("stereoWidth", 160.0)]),
        m("reverb", &[("size", 40.0), ("decay", 1.5), ("mix", 15.0)]),
    ], macros: &[
        k("Repeats", 40.0, &[(0, "feedback", 0.0, 80.0)]),
        k("Echo", 35.0, &[(0, "mix", 0.0, 60.0)]),
    ] },
    Factory { folder: "Keys & Synths", name: "Lo-Fi Keys", members: &[
        m("character-mod", &[("model", 4.0), ("depth", 35.0), ("mix", 60.0)]),
        m("bitcrusher", &[("bitdepth", 12.0), ("downsample", 2.0), ("mix", 35.0)]),
        m("auto-filter", &[("frequency", 6000.0), ("resonance", 0.1)]),
    ], macros: &[
        k("Wow", 35.0, &[(0, "depth", 0.0, 90.0)]),
        k("Crush", 35.0, &[(1, "mix", 0.0, 90.0)]),
        k("Muffle", 30.0, &[(2, "frequency", 20000.0, 1500.0)]),
    ] },
    Factory { folder: "Keys & Synths", name: "Arp Pulse", members: &[
        m("trance-gate", &[("mix", 70.0)]),
        m("delay", &[("feedback", 30.0), ("mix", 18.0)]),
    ], macros: &[
        k("Gate", 60.0, &[(0, "mix", 0.0, 100.0)]),
        k("Echo", 25.0, &[(1, "mix", 0.0, 50.0)]),
    ] },
    Factory { folder: "Keys & Synths", name: "Brass Bite", members: &[
        m("channeleq", &[("band4_freq", 1200.0), ("band4_gain2", 3.0)]),
        m("compressor", &[("threshold", -16.0), ("ratio", 3.0), ("attack", 8.0), ("release", 120.0), ("makeup_gain", 2.0)]),
        m("tapesaturation", &[("drive", 25.0), ("warmth", 45.0)]),
    ], macros: &[
        k("Bite", 40.0, &[(0, "band4_gain2", 0.0, 7.0)]),
        k("Drive", 30.0, &[(2, "drive", 0.0, 70.0)]),
    ] },
    Factory { folder: "Keys & Synths", name: "Bell Space", members: &[
        m("channeleq", &[("band0_enabled", 1.0), ("band0_freq", 250.0)]),
        m("delay", &[("feedback", 35.0), ("mix", 20.0), ("timeMs", 333.0), ("timeMsR", 500.0)]),
        m("reverb", &[("size", 80.0), ("decay", 4.5), ("mix", 30.0), ("shimmer", 25.0)]),
    ], macros: &[
        k("Echo", 30.0, &[(1, "mix", 0.0, 60.0)]),
        k("Space", 40.0, &[(2, "mix", 10.0, 70.0)]),
    ] },
    // ---- Pads & Textures (more)
    Factory { folder: "Pads & Textures", name: "Frozen Air", members: &[
        m("grains", &[("size", 400.0), ("density", 30.0), ("spray", 300.0), ("mix", 50.0)]),
        m("reverb", &[("size", 95.0), ("decay", 9.5), ("mix", 50.0)]),
    ], macros: &[
        k("Grain", 50.0, &[(0, "mix", 0.0, 100.0)]),
        k("Space", 50.0, &[(1, "mix", 10.0, 90.0)]),
    ] },
    Factory { folder: "Pads & Textures", name: "Slow Motion", members: &[
        m("phaser", &[("rate", 0.08), ("depth", 80.0), ("feedback", 45.0), ("mix", 45.0)]),
        m("chorus", &[("rate", 0.15), ("depth", 5.0), ("mix", 35.0)]),
        m("reverb", &[("size", 85.0), ("decay", 6.0), ("mix", 40.0)]),
    ], macros: &[
        k("Phase", 45.0, &[(0, "mix", 0.0, 90.0)]),
        k("Drift", 35.0, &[(1, "mix", 0.0, 80.0)]),
        k("Space", 40.0, &[(2, "mix", 10.0, 80.0)]),
    ] },
    Factory { folder: "Pads & Textures", name: "Warm Blanket", members: &[
        m("tapesaturation", &[("drive", 25.0), ("warmth", 80.0), ("noise", -45.0)]),
        m("auto-filter", &[("frequency", 4500.0), ("resonance", 0.1)]),
        m("reverb", &[("size", 60.0), ("decay", 3.5), ("damping", 70.0), ("mix", 30.0)]),
    ], macros: &[
        k("Warmth", 50.0, &[(0, "warmth", 20.0, 100.0)]),
        k("Soft", 40.0, &[(1, "frequency", 20000.0, 1500.0)]),
        k("Space", 30.0, &[(2, "mix", 0.0, 60.0)]),
    ] },
    Factory { folder: "Pads & Textures", name: "Breathing Pad", members: &[
        m("auto-filter", &[("frequency", 2000.0), ("resonance", 0.3), ("lfo_amount", 30.0), ("lfo_rate_free", 0.12)]),
        m("tremolo", &[("rate", 0.25), ("depth", 30.0), ("shape", 1.0)]),
        m("reverb", &[("size", 75.0), ("decay", 4.0), ("mix", 35.0)]),
    ], macros: &[
        k("Breath", 45.0, &[(0, "lfo_amount", 0.0, 60.0)]),
        k("Pulse", 30.0, &[(1, "depth", 0.0, 80.0)]),
    ] },
    Factory { folder: "Pads & Textures", name: "Glass Shimmer", members: &[
        m("pitch-shifter", &[("semitones", 12.0), ("mix", 20.0)]),
        m("reverb", &[("size", 90.0), ("decay", 7.0), ("mix", 45.0), ("shimmer", 80.0), ("lowcut2", 400.0)]),
    ], macros: &[
        k("Octave", 25.0, &[(0, "mix", 0.0, 70.0)]),
        k("Shimmer", 60.0, &[(1, "shimmer", 0.0, 100.0)]),
    ] },
    Factory { folder: "Pads & Textures", name: "Tape Memory", members: &[
        m("character-mod", &[("model", 4.0), ("depth", 50.0), ("mix", 60.0)]),
        m("tapesaturation", &[("drive", 35.0), ("warmth", 60.0), ("noise", -40.0)]),
        m("echo", &[("mix", 25.0), ("intensity", 4.0)]),
    ], macros: &[
        k("Wow", 45.0, &[(0, "depth", 0.0, 100.0)]),
        k("Age", 40.0, &[(1, "drive", 0.0, 80.0)]),
        k("Echo", 30.0, &[(2, "mix", 0.0, 60.0)]),
    ] },
    Factory { folder: "Pads & Textures", name: "Dark Drone", members: &[
        m("auto-filter", &[("frequency", 800.0), ("resonance", 0.4)]),
        m("distortion", &[("drive", 20.0), ("tone", 1500.0), ("mix", 30.0)]),
        m("reverb", &[("size", 100.0), ("decay", 10.0), ("mix", 55.0), ("damping", 80.0)]),
    ], macros: &[
        k("Depth", 40.0, &[(0, "frequency", 100.0, 4000.0)]),
        k("Grit", 30.0, &[(1, "mix", 0.0, 80.0)]),
        k("Abyss", 55.0, &[(2, "mix", 20.0, 90.0)]),
    ] },
    Factory { folder: "Pads & Textures", name: "Granular Haze", members: &[
        m("grains", &[("size", 180.0), ("density", 60.0), ("spray", 600.0), ("pitch", 12.0), ("spread", 7.0), ("mix", 40.0)]),
        m("chorus", &[("rate", 0.3), ("depth", 4.0), ("mix", 30.0)]),
    ], macros: &[
        k("Grains", 40.0, &[(0, "mix", 0.0, 100.0)]),
        k("Scatter", 50.0, &[(0, "spray", 0.0, 1500.0)]),
    ] },
    // ---- FX & Glitch (more)
    Factory { folder: "FX & Glitch", name: "Stutter Gate", members: &[
        m("trance-gate", &[("pattern", 3.0), ("mix", 100.0)]),
        m("bitcrusher", &[("bitdepth", 8.0), ("downsample", 3.0), ("mix", 30.0)]),
    ], macros: &[
        k("Gate", 80.0, &[(0, "mix", 0.0, 100.0)]),
        k("Crush", 30.0, &[(1, "mix", 0.0, 100.0)]),
    ] },
    Factory { folder: "FX & Glitch", name: "Glitch Grains", members: &[
        m("grains", &[("size", 40.0), ("density", 120.0), ("chance", 70.0), ("reverse", 40.0), ("mix", 60.0)]),
        m("bitcrusher", &[("bitdepth", 6.0), ("downsample", 8.0), ("mix", 30.0)]),
    ], macros: &[
        k("Glitch", 60.0, &[(0, "mix", 0.0, 100.0)]),
        k("Reverse", 40.0, &[(0, "reverse", 0.0, 100.0)]),
        k("Crush", 30.0, &[(1, "mix", 0.0, 100.0)]),
    ] },
    Factory { folder: "FX & Glitch", name: "Downlifter", members: &[
        m("auto-filter", &[("frequency", 12000.0), ("resonance", 0.5)]),
        m("pitch-shifter", &[("semitones", -12.0), ("mix", 30.0)]),
        m("reverb", &[("size", 85.0), ("decay", 5.0), ("mix", 35.0)]),
    ], macros: &[
        k("Fall", 20.0, &[(0, "frequency", 18000.0, 200.0), (2, "mix", 10.0, 70.0)]),
        k("Drop", 30.0, &[(1, "mix", 0.0, 80.0)]),
    ] },
    Factory { folder: "FX & Glitch", name: "Metal Ring", members: &[
        m("ring-mod", &[("frequency", 330.0), ("mix", 50.0)]),
        m("flanger", &[("rate", 0.2), ("depth", 6.0), ("feedback", 70.0), ("mix", 40.0)]),
    ], macros: &[
        k("Ring", 50.0, &[(0, "mix", 0.0, 100.0)]),
        k("Tone", 30.0, &[(0, "frequency", 50.0, 3000.0)]),
        k("Jet", 40.0, &[(1, "mix", 0.0, 90.0)]),
    ] },
    Factory { folder: "FX & Glitch", name: "Broken Speaker", members: &[
        m("auto-filter", &[("frequency", 3000.0), ("resonance", 0.7), ("filter_type", 2.0)]),
        m("distortion", &[("drive", 60.0), ("mode", 3.0), ("mix", 70.0)]),
        m("utility", &[("mono", 1.0)]),
    ], macros: &[
        k("Break", 55.0, &[(1, "drive", 10.0, 100.0)]),
        k("Band", 40.0, &[(0, "frequency", 600.0, 7000.0)]),
    ] },
    Factory { folder: "FX & Glitch", name: "Jet Sweep", members: &[
        m("flanger", &[("rate", 0.08), ("depth", 8.0), ("feedback", 80.0), ("mix", 55.0)]),
        m("reverb", &[("size", 50.0), ("decay", 2.0), ("mix", 20.0)]),
    ], macros: &[
        k("Sweep", 50.0, &[(0, "feedback", 0.0, 90.0)]),
        k("Speed", 20.0, &[(0, "rate", 0.02, 3.0)]),
    ] },
    Factory { folder: "FX & Glitch", name: "Underwater", members: &[
        m("auto-filter", &[("frequency", 600.0), ("resonance", 0.6), ("lfo_amount", 20.0), ("lfo_rate_free", 0.4)]),
        m("chorus", &[("rate", 0.5), ("depth", 6.0), ("mix", 50.0)]),
        m("reverb", &[("size", 70.0), ("decay", 3.0), ("damping", 85.0), ("mix", 35.0)]),
    ], macros: &[
        k("Depth", 50.0, &[(0, "frequency", 3000.0, 200.0)]),
        k("Waves", 40.0, &[(0, "lfo_amount", 0.0, 50.0)]),
    ] },
    Factory { folder: "FX & Glitch", name: "Freeze Pad", members: &[
        m("grains", &[("size", 900.0), ("density", 20.0), ("mix", 70.0), ("blur", 60.0)]),
        m("reverb", &[("size", 100.0), ("decay", 10.0), ("mix", 50.0)]),
    ], macros: &[
        k("Freeze", 60.0, &[(0, "mix", 0.0, 100.0)]),
        k("Blur", 60.0, &[(0, "blur", 0.0, 100.0)]),
    ] },
    // ---- Mix & Master (more)
    Factory { folder: "Mix & Master", name: "Vocal Bus", members: &[
        m("glue-compressor", &[("threshold", -10.0), ("makeup", 1.5)]),
        m("deesser", &[("frequency", 7000.0), ("threshold", -32.0), ("reduction", 4.0)]),
        m("reverb", &[("size", 45.0), ("decay", 1.6), ("mix", 8.0)]),
    ], macros: &[
        k("Glue", 35.0, &[(0, "threshold", -2.0, -20.0)]),
        k("Air", 20.0, &[(2, "mix", 0.0, 30.0)]),
    ] },
    Factory { folder: "Mix & Master", name: "Drum Bus Snap", members: &[
        m("glue-compressor", &[("threshold", -14.0), ("makeup", 2.5)]),
        m("transient-shaper", &[("attack", 20.0)]),
        m("soft-clipper", &[("input", 2.0), ("softness", 65.0)]),
    ], macros: &[
        k("Glue", 40.0, &[(0, "threshold", -4.0, -28.0)]),
        k("Snap", 35.0, &[(1, "attack", 0.0, 70.0)]),
        k("Clip", 25.0, &[(2, "input", 0.0, 9.0)]),
    ] },
    Factory { folder: "Mix & Master", name: "Mix Warmer", members: &[
        m("tapesaturation", &[("drive", 15.0), ("warmth", 55.0), ("compression", 20.0)]),
        m("channeleq", &[("band1_gain2", 1.0), ("band6_gain2", -1.0)]),
    ], macros: &[
        k("Tape", 30.0, &[(0, "drive", 0.0, 50.0)]),
        k("Tilt", 50.0, &[(1, "band1_gain2", -3.0, 4.0)]),
    ] },
    Factory { folder: "Mix & Master", name: "Mono Check", members: &[
        m("utility", &[("mono", 1.0)]),
        m("spectrum", &[]),
    ], macros: &[
        k("Width", 0.0, &[(0, "width", 100.0, 0.0)]),
    ] },
    Factory { folder: "Mix & Master", name: "Streaming Master", members: &[
        m("multiband-compressor", &[("low_threshold", -20.0), ("mid_threshold", -22.0), ("high_threshold", -24.0)]),
        m("widener", &[("width2", 210.0), ("lowcut", 180.0)]),
        m("mastering-limiter", &[("threshold", -6.0), ("ceiling", -1.0)]),
    ], macros: &[
        k("Tighten", 40.0, &[(0, "mid_threshold", -10.0, -32.0)]),
        k("Width", 25.0, &[(1, "width2", 100.0, 300.0)]),
        k("Loudness", 40.0, &[(2, "threshold", 0.0, -12.0)]),
    ] },
    Factory { folder: "Mix & Master", name: "Club Master", members: &[
        m("utility", &[("bass_mono", 1.0), ("bass_mono_freq", 150.0)]),
        m("glue-compressor", &[("threshold", -10.0), ("makeup", 2.0)]),
        m("soft-clipper", &[("input", 3.0), ("softness", 60.0)]),
        m("mastering-limiter", &[("threshold", -8.0), ("ceiling", -0.8)]),
    ], macros: &[
        k("Glue", 40.0, &[(1, "threshold", -2.0, -20.0)]),
        k("Clip", 30.0, &[(2, "input", 0.0, 10.0)]),
        k("Loudness", 50.0, &[(3, "threshold", 0.0, -14.0)]),
    ] },
    Factory { folder: "Mix & Master", name: "Clean Up", members: &[
        m("channeleq", &[("band0_enabled", 1.0), ("band0_freq", 40.0)]),
        m("dehummer", &[("depth", -30.0)]),
        m("denoiser", &[("reduction", -12.0)]),
    ], macros: &[
        k("Rumble", 30.0, &[(0, "band0_freq", 20.0, 150.0)]),
        k("Hum", 30.0, &[(1, "depth", 0.0, -60.0)]),
        k("Noise", 30.0, &[(2, "reduction", 0.0, -40.0)]),
    ] },
];

fn build(f: &Factory) -> ChainPreset {
    let mut devices = Vec::with_capacity(f.members.len() + 1);
    let mut head = rack::RACK.instantiate();
    head.id = DeviceId(1);
    devices.push(head);
    for (i, mem) in f.members.iter().enumerate() {
        let def = builtin::find(mem.id).unwrap_or_else(|| panic!("factory chain {}: unknown device {}", f.name, mem.id));
        let mut d = def.instantiate();
        d.id = DeviceId(2 + i as u64);
        d.chain = Some(DeviceId(1));
        for (p, v) in mem.values {
            let pd = def.param(p).unwrap_or_else(|| panic!("factory chain {}: {} has no {p}", f.name, mem.id));
            d.params.insert((*p).to_string(), pd.snap(*v));
        }
        devices.push(d);
    }
    for (slot, mac) in f.macros.iter().enumerate().take(rack::MACROS) {
        devices[0].params.insert(format!("macro{}", slot + 1), mac.at);
        if devices[0].macro_names.len() < slot + 1 {
            devices[0].macro_names.resize(slot + 1, String::new());
        }
        devices[0].macro_names[slot] = mac.name.to_string();
        for &(member, p, lo, hi) in mac.links {
            let def = builtin::find(f.members[member].id).unwrap();
            let pd = def.param(p).unwrap_or_else(|| panic!("factory chain {}: {} has no {p}", f.name, def.id));
            let (min, max) = (pd.to_normalized(lo), pd.to_normalized(hi));
            let target = DeviceId(2 + member as u64);
            // The member's own value follows the macro's position.
            let v = pd.from_normalized(min + (max - min) * mac.at / 100.0);
            devices[1 + member].params.insert(p.to_string(), pd.snap(v));
            devices[0].links.push(ModLink { slot: slot as u8, target, param: ModParam::Builtin(p.to_string()), min, max });
        }
    }
    ChainPreset { version: VERSION, name: f.name.to_string(), devices }
}

/// DStudio's factory chains as (folder, preset), in browser order.
pub fn factory() -> Vec<(&'static str, ChainPreset)> {
    FACTORY.iter().map(|f| (f.folder, build(f))).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ColorIndex;
    use crate::model::{Track, TrackId, TrackKind};

    #[test]
    fn factory_chains_are_valid_and_named_uniquely() {
        let all = factory();
        assert!(all.len() >= 80);
        let mut names = std::collections::HashSet::new();
        for (folder, p) in &all {
            assert!(FOLDERS.contains(folder), "{folder}");
            assert!(p.is_valid(), "{}", p.name);
            assert!(names.insert(p.name.clone()), "duplicate {}", p.name);
            // Every macro link aims at a member and stays inside 0..1.
            for l in &p.devices[0].links {
                assert!(p.devices[1..].iter().any(|d| d.id == l.target));
                assert!((0.0..=1.0).contains(&l.min) && (0.0..=1.0).contains(&l.max), "{}", p.name);
            }
        }
        for f in FOLDERS {
            assert!(all.iter().any(|(g, _)| *g == f), "empty folder {f}");
        }
    }

    #[test]
    fn instantiate_renumbers_members_and_links() {
        let p = factory().into_iter().find(|(_, p)| p.name == "Punchy Kit").unwrap().1;
        let ds = p.instantiate(40);
        assert_eq!(ds.iter().map(|d| d.id.0).collect::<Vec<_>>(), vec![40, 41, 42, 43]);
        assert!(ds[0].chain.is_none());
        assert!(ds[1..].iter().all(|d| d.chain == Some(DeviceId(40))));
        assert!(ds[0].links.iter().all(|l| (41..=43).contains(&l.target.0)));
        assert_eq!(ds[0].label.as_deref(), Some("Punchy Kit"));
    }

    #[test]
    fn from_track_round_trips_and_drops_outside_links() {
        let mut t = Track::new(TrackId(1), TrackKind::Audio, "A", ColorIndex::FIRST);
        let mut devs = factory().into_iter().find(|(_, p)| p.name == "Bus Glue").unwrap().1.instantiate(10);
        let outside = builtin::find("compressor").unwrap().instantiate();
        let mut outside = outside;
        outside.id = DeviceId(99);
        devs[0].links.push(ModLink { slot: 3, target: DeviceId(99), param: ModParam::Builtin("ratio".into()), min: 0.0, max: 1.0 });
        devs.push(outside);
        t.devices = devs;
        let saved = ChainPreset::from_track(&t, DeviceId(10), "My Glue").unwrap();
        assert_eq!(saved.devices.len(), 3);
        assert!(saved.devices[0].links.iter().all(|l| l.target != DeviceId(99)));
        let back = ChainPreset::from_json(&saved.to_json()).unwrap();
        assert_eq!(back, saved);
        assert!(ChainPreset::from_track(&t, DeviceId(11), "x").is_none(), "a member is not a head");
    }

    #[test]
    fn file_names_are_safe() {
        assert_eq!(file_name("My/Chain: v2"), "My_Chain_ v2.json");
        assert_eq!(file_name("../.."), "Chain.json");
        assert_eq!(file_name("Keys & Synths"), "Keys & Synths.json");
    }
}
