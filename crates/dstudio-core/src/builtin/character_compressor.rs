//! Character Compressor: parameter ids, ranges, defaults, tapers, the model
//! overlays, detents, the panel arrangement and the factory presets.
//!
//! Six models share one parameter set. The model decides which knobs the card
//! shows (`Layout::shown_when`), the static curve (`character_transfer`, also
//! drawn on the Ratio knob), the snap points of Ratio / Attack / Release and
//! some readouts (FET dial positions, Bus VCA auto release). Picking a model
//! writes that model's starting values in the same undo step.

use super::*;
use crate::device::PluginKind;

const MODELS: &[&str] = &["Clean", "Vari-Mu", "Bus VCA", "FET", "Opto", "RMS VCA"];
const OFF_ON: &[&str] = &["Off", "On"];

pub const CLEAN: u32 = 0;
pub const VARI_MU: u32 = 1;
pub const BUS_VCA: u32 = 2;
pub const FET: u32 = 3;
pub const OPTO: u32 = 4;
pub const RMS_VCA: u32 = 5;

/// FET ratios from this value read "All" (all buttons in).
const FET_ALL: f32 = 25.0;
/// Opto Comp / Limit latch: ratios written by each state and the switch point.
const OPTO_COMP: f32 = 3.0;
const OPTO_LIMIT: f32 = 8.0;
const OPTO_LIMIT_FROM: f32 = 5.5;
/// Bus VCA: releases above this read "AUTO" and switch Adapt on.
const AUTO_RELEASE_FROM: f32 = 1500.0;

const FET_ATTACK: &[f32] = &[0.8, 0.55, 0.35, 0.2, 0.1, 0.05, 0.02];
const FET_RELEASE: &[f32] = &[1100.0, 800.0, 550.0, 350.0, 200.0, 110.0, 50.0];
/// Vari-Mu time-constant positions (attack ms, release ms).
const VARI_MU_PAIRS: &[(f32, f32)] = &[(0.2, 300.0), (0.2, 800.0), (0.4, 2000.0), (0.8, 5000.0), (0.2, 2000.0), (0.4, 300.0)];

fn ratio_text(v: f32) -> String {
    if v >= 29.5 {
        "\u{221e}:1".into()
    } else if v >= 10.0 {
        format!("{v:.0}:1")
    } else {
        format!("{v:.1}:1")
    }
}

fn attack_text(v: f32) -> String {
    if v < 10.0 {
        format!("{v:.2} ms")
    } else if v < 100.0 {
        format!("{v:.1} ms")
    } else {
        format!("{v:.0} ms")
    }
}

fn release_text(v: f32) -> String {
    if v < 100.0 { format!("{v:.1} ms") } else { format!("{v:.0} ms") }
}

fn percent_text(v: f32) -> String {
    format!("{:.0}%", v.round() + 0.0)
}

const DB: Format = Format::Custom(TextFn(db_text));
const PCT: Format = Format::Custom(TextFn(percent_text));

macro_rules! level_band {
    ($n:literal, $freq:literal) => {
        [
            ParamDef::choice(concat!("level", $n, "_enabled"), concat!("Level ", $n, " Enabled"), OFF_ON, 0)
                .help("Turns this weighting band of the level detector on."),
            ParamDef::new(
                concat!("level", $n, "_freq"),
                concat!("Level ", $n, " Freq"),
                20.0,
                20000.0,
                $freq,
                Taper::Linear { skew: 0.25 },
                "Hz",
                Format::Freq,
            )
            .help("Centre of this weighting band."),
            ParamDef::new(
                concat!("level", $n, "_q"),
                concat!("Level ", $n, " Q"),
                0.1,
                12.0,
                1.0,
                Taper::Linear { skew: 0.35 },
                "",
                Format::Plain(2),
            )
            .help("Width of this weighting band."),
            ParamDef::new(concat!("level", $n, "_weight"), concat!("Level ", $n, " Weight"), -24.0, 0.0, 0.0, LINEAR, "dB", Format::Db)
                .step(0.1)
                .help("How much less this band counts towards the long-term level."),
        ]
    };
}

const L1: [ParamDef; 4] = level_band!(1, 150.0);
const L2: [ParamDef; 4] = level_band!(2, 600.0);
const L3: [ParamDef; 4] = level_band!(3, 2500.0);
const L4: [ParamDef; 4] = level_band!(4, 8000.0);

pub static CHARACTER_COMPRESSOR: DeviceDef = DeviceDef {
    id: "character-compressor",
    name: "Character Compressor",
    kind: PluginKind::Effect,
    category: "Dynamics",
    purpose: "A compressor with six circuit models, from transparent to coloured: tube, console bus, FET, optical and RMS behaviour with matching drive.",
    params: &[
        ParamDef::choice("model", "Model", MODELS, 0)
            .not_automatable()
            .help("Circuit model: sets the curve, timing behaviour and colour, and which controls apply."),
        ParamDef::new("threshold", "Threshold", -60.0, 0.0, -18.0, LINEAR, "dB", DB)
            .step(0.1)
            .help("Level above which gain reduction starts."),
        ParamDef::new("ratio", "Ratio", 1.0, 30.0, 4.0, Taper::Linear { skew: 0.35 }, ":1", Format::Custom(TextFn(ratio_text)))
            .help("How strongly level above the threshold is reduced."),
        ParamDef::new("knee", "Knee", 0.0, 24.0, 6.0, LINEAR, "dB", DB)
            .step(0.1)
            .help("Width of the soft transition around the threshold (Clean model)."),
        ParamDef::new("attack", "Attack", 0.02, 250.0, 10.0, Taper::Linear { skew: 0.22 }, "ms", Format::Custom(TextFn(attack_text)))
            .help("How fast reduction sets in."),
        ParamDef::new("release", "Release", 20.0, 8000.0, 200.0, Taper::Linear { skew: 0.3 }, "ms", Format::Custom(TextFn(release_text)))
            .help("How fast reduction lets go."),
        ParamDef::choice("adapt", "Adapt", OFF_ON, 0).help("Program-dependent release: sustained reduction recovers more slowly."),
        ParamDef::new("hold", "Hold", 0.0, 500.0, 0.0, Taper::Linear { skew: 0.5 }, "ms", Format::Ms(2))
            .help("Time the reduction is held before it releases."),
        ParamDef::new("range", "Range", 0.0, 24.0, 24.0, LINEAR, "dB", Format::DbNoPlus).step(0.1).help("Largest gain reduction allowed."),
        ParamDef::new("drive", "Drive", 0.0, 2.0, 1.0, LINEAR, "", Format::Plain(2))
            .help("Amount of the model's saturation; 1 is the model's normal colour."),
        ParamDef::new("mix", "Mix", 0.0, 200.0, 100.0, LINEAR, "%", PCT)
            .help("Blend of dry and compressed signal; above 100% the compression is exaggerated."),
        ParamDef::new("makeup", "Makeup", -12.0, 24.0, 0.0, LINEAR, "dB", Format::Db).step(0.1).help("Gain after compression."),
        ParamDef::choice("auto_gain", "Auto Gain", OFF_ON, 0).help("Adds make-up gain that follows the threshold and ratio."),
        ParamDef::new("lookahead", "Lookahead", 0.0, 20.0, 0.0, LINEAR, "ms", Format::MsAuto)
            .step(0.1)
            .help("Delays the audio so the detector reacts before peaks arrive (adds latency)."),
        ParamDef::choice("oversampling", "Oversampling", &["2x", "4x"], 0).help("Oversampling of the saturation stage."),
        ParamDef::new("stereo_link", "Stereo Link", 0.0, 100.0, 100.0, LINEAR, "%", PCT)
            .help("How much both channels share one gain reduction."),
        ParamDef::choice("ms_mode", "M/S Mode", &["Stereo", "Mid-Side"], 0).help("Compress left / right or mid / side."),
        ParamDef::choice("sc_source", "SC Source", &["Internal", "External"], 0)
            .not_automatable()
            .help("Set while a sidechain source is picked."),
        ParamDef::choice("sc_hpf_on", "SC HPF", OFF_ON, 0).help("High-pass filter in the detector path."),
        ParamDef::new("sc_hpf_freq", "SC HPF Freq", 20.0, 500.0, 80.0, Taper::Linear { skew: 0.5 }, "Hz", Format::Freq)
            .help("Cutoff of the detector high-pass filter."),
        ParamDef::new("sc_eq_freq", "SC EQ Freq", 100.0, 10000.0, 1000.0, Taper::Linear { skew: 0.3 }, "Hz", Format::Freq)
            .help("Centre of the detector EQ band."),
        ParamDef::new("sc_eq_gain", "SC EQ Gain", -12.0, 12.0, 0.0, LINEAR, "dB", Format::Db)
            .step(0.1)
            .help("Boost or cut of the detector EQ band."),
        ParamDef::new("sc_eq_q", "SC EQ Q", 0.3, 4.0, 1.0, Taper::Linear { skew: 0.5 }, "", Format::Plain(2))
            .help("Width of the detector EQ band."),
        ParamDef::choice("sc_listen", "SC Listen", OFF_ON, 0).reset_on_load().help("Play the detector signal instead of the output."),
        ParamDef::choice("bypass", "Bypass", OFF_ON, 0).not_automatable().help("Internal bypass switch."),
        ParamDef::choice("level_enabled", "Level", OFF_ON, 0)
            .help("Slow level rider that moves the long-term level towards the threshold."),
        ParamDef::new("level_amount", "Level Amount", 0.0, 100.0, 50.0, LINEAR, "%", PCT)
            .help("How much of the long-term level difference the rider corrects."),
        ParamDef::new("level_response", "Level Response", 1.0, 30.0, 8.0, Taper::Linear { skew: 0.4 }, "s", Format::Plain(0))
            .help("Averaging time of the level rider."),
        ParamDef::new("level_tilt", "Level Tilt", -6.0, 6.0, 0.0, LINEAR, "dB", Format::Db)
            .step(0.1)
            .help("Tilts the level detector towards highs (positive) or lows (negative)."),
        L1[0],
        L1[1],
        L1[2],
        L1[3],
        L2[0],
        L2[1],
        L2[2],
        L2[3],
        L3[0],
        L3[1],
        L3[2],
        L3[3],
        L4[0],
        L4[1],
        L4[2],
        L4[3],
        ParamDef::choice("delta_monitor", "Delta Monitor", OFF_ON, 0)
            .not_automatable()
            .reset_on_load()
            .help("Play only what the compressor removes."),
    ],
    layout: Layout {
        picker: &[Control::Dropdown("model"), Control::Sidechain],
        view: Some(View::Strip(Strip::new("threshold").range(0.0, -60.0))),
        sliders: &[Control::Slider("makeup")],
        clusters: &[
            &[Control::Knob("threshold"), Control::Knob("ratio")],
            &[Control::Knob("knee"), Control::Knob("attack"), Control::Knob("release")],
            &[Control::Knob("drive")],
            &[Control::Knob("mix")],
        ],
        faces: &[("ratio", Face::Transfer)],
        min_height: 203.0,
        shown_when: &[
            ("knee", "model", &[CLEAN]),
            ("attack", "model", &[CLEAN, VARI_MU, BUS_VCA, FET]),
            ("release", "model", &[CLEAN, VARI_MU, BUS_VCA, FET]),
            ("drive", "model", &[VARI_MU, BUS_VCA, FET, OPTO, RMS_VCA]),
        ],
        fixed_width: 0.0,
        ..Layout::EMPTY
    },
    factory: &[
        FactoryPreset {
            name: "Clear Lead Vocal",
            values: &[("model", 4.0), ("threshold", -27.0), ("ratio", 3.0), ("drive", 0.9), ("makeup", 9.5)],
        },
        FactoryPreset {
            name: "Forward Rap Vocal",
            values: &[
                ("model", 3.0),
                ("threshold", -21.0),
                ("ratio", 8.0),
                ("attack", 0.55),
                ("release", 200.0),
                ("drive", 0.7),
                ("makeup", 8.0),
            ],
        },
        FactoryPreset {
            name: "Tube Bus Cream",
            values: &[
                ("model", 1.0),
                ("threshold", -16.0),
                ("ratio", 2.0),
                ("attack", 0.4),
                ("release", 2000.0),
                ("drive", 1.2),
                ("makeup", 2.5),
            ],
        },
        FactoryPreset {
            name: "Console Bus Lock",
            values: &[
                ("model", 2.0),
                ("threshold", -13.0),
                ("ratio", 4.0),
                ("attack", 10.0),
                ("release", 2000.0),
                ("adapt", 1.0),
                ("drive", 0.6),
                ("makeup", 2.0),
            ],
        },
        FactoryPreset {
            name: "Snare Crack",
            values: &[
                ("model", 3.0),
                ("threshold", -18.0),
                ("ratio", 4.0),
                ("attack", 0.8),
                ("release", 110.0),
                ("drive", 1.1),
                ("makeup", 5.0),
            ],
        },
        FactoryPreset {
            name: "Room Mic Smash",
            values: &[
                ("model", 3.0),
                ("threshold", -34.0),
                ("ratio", 30.0),
                ("attack", 0.05),
                ("release", 50.0),
                ("drive", 1.6),
                ("makeup", 16.0),
                ("mix", 70.0),
            ],
        },
        FactoryPreset {
            name: "Steady Bass Line",
            values: &[("model", 5.0), ("threshold", -26.0), ("ratio", 4.0), ("drive", 1.1), ("makeup", 9.0)],
        },
        FactoryPreset {
            name: "Parallel Thump",
            values: &[("model", 5.0), ("threshold", -34.0), ("ratio", 8.0), ("drive", 1.3), ("makeup", 14.0), ("mix", 40.0)],
        },
        FactoryPreset {
            name: "Soft Piano Even",
            values: &[("model", 4.0), ("threshold", -31.0), ("ratio", 3.0), ("drive", 1.0), ("makeup", 10.0)],
        },
        FactoryPreset {
            name: "Acoustic Guitar Polish",
            values: &[
                ("model", 1.0),
                ("threshold", -22.0),
                ("ratio", 4.0),
                ("attack", 0.2),
                ("release", 800.0),
                ("drive", 0.9),
                ("makeup", 4.0),
            ],
        },
        FactoryPreset {
            name: "Transparent Peak Shave",
            values: &[
                ("model", 0.0),
                ("threshold", -4.0),
                ("ratio", 20.0),
                ("knee", 3.0),
                ("attack", 0.5),
                ("release", 90.0),
                ("lookahead", 3.0),
            ],
        },
        FactoryPreset {
            name: "Gentle Clean Leveling",
            values: &[
                ("model", 0.0),
                ("threshold", -24.0),
                ("ratio", 2.0),
                ("knee", 12.0),
                ("attack", 25.0),
                ("release", 400.0),
                ("adapt", 1.0),
                ("makeup", 3.5),
            ],
        },
        FactoryPreset {
            name: "Ignore The Low End",
            values: &[
                ("model", 0.0),
                ("threshold", -20.0),
                ("ratio", 3.5),
                ("attack", 12.0),
                ("release", 160.0),
                ("sc_hpf_on", 1.0),
                ("sc_hpf_freq", 150.0),
                ("makeup", 3.0),
            ],
        },
        FactoryPreset {
            name: "Side Channel Control",
            values: &[
                ("model", 0.0),
                ("threshold", -26.0),
                ("ratio", 2.5),
                ("release", 250.0),
                ("ms_mode", 1.0),
                ("stereo_link", 0.0),
                ("makeup", 4.0),
            ],
        },
        FactoryPreset {
            name: "Broadcast Rider",
            values: &[
                ("model", 0.0),
                ("threshold", -18.0),
                ("ratio", 2.5),
                ("attack", 15.0),
                ("release", 220.0),
                ("adapt", 1.0),
                ("level_enabled", 1.0),
                ("level_amount", 65.0),
                ("level_response", 10.0),
                ("level1_enabled", 1.0),
                ("level1_freq", 120.0),
                ("level1_q", 0.8),
                ("level1_weight", -10.0),
                ("makeup", 2.0),
            ],
        },
        FactoryPreset {
            name: "Long-Term Leveler",
            values: &[
                ("model", 0.0),
                ("threshold", -10.0),
                ("ratio", 1.0),
                ("level_enabled", 1.0),
                ("level_amount", 75.0),
                ("level_response", 16.0),
                ("level_tilt", 1.5),
                ("level2_enabled", 1.0),
                ("level2_freq", 3000.0),
                ("level2_q", 0.9),
                ("level2_weight", -8.0),
            ],
        },
    ],
    sidechain: true,
    samples: 0,
};

// ---------------------------------------------------------------- behaviour

fn model_of(def: &DeviceDef, d: &Device) -> u32 {
    def.param("model").map_or(0, |p| def.value(d, p).round() as u32)
}

fn value_of(def: &DeviceDef, d: &Device, id: &str) -> f32 {
    def.param(id).map_or(0.0, |p| def.value(d, p))
}

/// Values a model pick writes (the model's starting point).
fn overlay(model: u32) -> &'static [(&'static str, f32)] {
    match model {
        CLEAN => &[("threshold", -18.0), ("ratio", 4.0), ("knee", 6.0), ("attack", 10.0), ("release", 200.0)],
        VARI_MU => &[
            ("threshold", -24.0),
            ("ratio", 4.0),
            ("attack", 0.2),
            ("release", 2000.0),
            ("drive", 1.0),
            ("stereo_link", 100.0),
            ("mix", 100.0),
        ],
        BUS_VCA => &[
            ("threshold", -15.0),
            ("ratio", 4.0),
            ("attack", 30.0),
            ("release", 2000.0),
            ("adapt", 1.0),
            ("drive", 1.0),
            ("stereo_link", 100.0),
            ("mix", 100.0),
        ],
        FET => &[("ratio", 4.0), ("attack", 0.35), ("release", 200.0), ("drive", 1.0), ("mix", 100.0)],
        OPTO => &[("threshold", -30.0), ("ratio", 3.0), ("drive", 1.0), ("mix", 100.0)],
        _ => &[("threshold", -20.0), ("ratio", 4.0), ("drive", 1.0), ("mix", 100.0)],
    }
}

fn detents(def: &DeviceDef, d: &Device, p: &ParamDef) -> &'static [f32] {
    match (model_of(def, d), p.id) {
        (VARI_MU, "ratio") => &[2.0, 4.0, 10.0, 20.0],
        (VARI_MU, "attack") => &[0.2, 0.4, 0.8],
        (VARI_MU, "release") => &[300.0, 800.0, 2000.0, 5000.0],
        (BUS_VCA, "ratio") => &[2.0, 4.0, 10.0],
        (BUS_VCA, "attack") => &[0.1, 0.3, 1.0, 3.0, 10.0, 30.0],
        (BUS_VCA, "release") => &[100.0, 300.0, 600.0, 1200.0, 2000.0],
        (FET, "ratio") => &[4.0, 8.0, 12.0, 20.0, 30.0],
        (FET, "attack") => &[0.02, 0.05, 0.1, 0.2, 0.35, 0.55, 0.8],
        (FET, "release") => &[50.0, 110.0, 200.0, 350.0, 550.0, 800.0, 1100.0],
        (OPTO, "ratio") => &[3.0, 8.0],
        (RMS_VCA, "ratio") => &[2.0, 4.0, 8.0, 30.0],
        _ => &[],
    }
}

/// Position (1-based) of the nearest value in a FET dial table.
fn dial(table: &[f32], v: f32) -> usize {
    1 + table.iter().enumerate().min_by(|a, b| (a.1 - v).abs().total_cmp(&(b.1 - v).abs())).map_or(0, |(i, _)| i)
}

fn text(def: &DeviceDef, d: &Device, p: &ParamDef, v: f32) -> Option<String> {
    match (model_of(def, d), p.id) {
        (FET, "ratio") if v >= FET_ALL => Some("All".into()),
        (FET, "attack") => Some(format!("DIAL {}", dial(FET_ATTACK, v))),
        (FET, "release") => Some(format!("DIAL {}", dial(FET_RELEASE, v))),
        (BUS_VCA, "release") if v > AUTO_RELEASE_FROM => Some("AUTO".into()),
        _ => None,
    }
}

/// Other parameters a panel edit moves along (see `builtin::linked_writes`).
pub(super) fn linked(def: &DeviceDef, d: &Device, p: &ParamDef, v: f32) -> Vec<(&'static str, f32)> {
    let close = |a: f32, b: f32| (a - b).abs() <= 1e-3 * a.abs().max(1.0);
    match (model_of(def, d), p.id) {
        (_, "model") => overlay(v.round() as u32).to_vec(),
        // The Vari-Mu time constants are pairs: landing on a pair's attack
        // (release) moves the release (attack) to the closest matching pair.
        (VARI_MU, "attack") => {
            let rel = value_of(def, d, "release");
            VARI_MU_PAIRS
                .iter()
                .filter(|(a, _)| close(*a, v))
                .min_by(|x, y| (x.1 - rel).abs().total_cmp(&(y.1 - rel).abs()))
                .map(|(_, r)| vec![("release", *r)])
                .unwrap_or_default()
        }
        (VARI_MU, "release") => {
            let att = value_of(def, d, "attack");
            VARI_MU_PAIRS
                .iter()
                .filter(|(_, r)| close(*r, v))
                .min_by(|x, y| (x.0 - att).abs().total_cmp(&(y.0 - att).abs()))
                .map(|(a, _)| vec![("attack", *a)])
                .unwrap_or_default()
        }
        (BUS_VCA, "release") => vec![("adapt", if v > AUTO_RELEASE_FROM { 1.0 } else { 0.0 })],
        _ => Vec::new(),
    }
}

fn latch(def: &DeviceDef, d: &Device, p: &ParamDef) -> Option<Latch> {
    (model_of(def, d) == OPTO && p.id == "ratio").then(|| Latch {
        labels: ["Comp", "Limit"],
        values: [OPTO_COMP, OPTO_LIMIT],
        on: value_of(def, d, "ratio") >= OPTO_LIMIT_FROM,
    })
}

fn transfer(def: &DeviceDef, d: &Device) -> Option<Transfer> {
    let model = model_of(def, d);
    let (thr, ratio, knee, range) =
        (value_of(def, d, "threshold"), value_of(def, d, "ratio"), value_of(def, d, "knee"), value_of(def, d, "range"));
    let (lo, hi) = (thr - 12.0, (thr + 24.0).max(0.0));
    let n = 48;
    let points = (0..=n)
        .map(|i| {
            let x = lo + (hi - lo) * i as f32 / n as f32;
            (x, x + character_transfer(model, x, thr, ratio, knee, range))
        })
        .collect();
    let (a, b) = bend(model, thr, ratio, knee);
    let bend = if range <= 0.0 { (thr, thr) } else { (a.max(lo), b.min(hi)) };
    Some(Transfer { lo, hi, points, bend })
}

pub static BEHAVIOR: Behavior = Behavior { detents, text, latch, transfer, ..Behavior::NONE };

// ---------------------------------------------------------------- static curves

fn slope(ratio: f32) -> f32 {
    if ratio >= 29.5 { -1.0 } else { 1.0 / ratio.max(1.0) - 1.0 }
}

/// Quadratic soft knee of width `k` around 0 dB overshoot, slope `s` above.
fn soft(over: f32, k: f32, s: f32) -> f32 {
    if k > 0.0 && over.abs() <= k / 2.0 {
        s * (over + k / 2.0).powi(2) / (2.0 * k)
    } else if over > 0.0 {
        s * over
    } else {
        0.0
    }
}

/// Vari-Mu knee width: wide at low ratios, 4 dB from 10:1.
fn vari_mu_knee(ratio: f32) -> f32 {
    4.0 + 16.0 * (10.0 - ratio.min(10.0)) / 9.0
}

/// Vari-Mu: the local ratio grows with the overshoot (measured from the
/// knee start, n = overshoot / 20 dB) and the curve blends into it across the
/// knee; integrated over 256 steps up to 20 dB above the threshold and
/// extended linearly beyond.
fn vari_mu(over: f32, ratio: f32) -> f32 {
    let k = vari_mu_knee(ratio);
    let local = |o: f32| {
        let n = ((o + k / 2.0) / 20.0).clamp(0.0, 1.0);
        let eff = (1.0 + (ratio.max(1.0) - 1.0) * n.powf(0.7)).max(1.0 + 19.0 * n * n * n);
        let w = ((o + k / 2.0) / k).clamp(0.0, 1.0);
        w * (1.0 / eff - 1.0)
    };
    let (start, end) = (-k / 2.0, 20.0);
    if over <= start {
        return 0.0;
    }
    let steps = 256;
    let h = (end - start) / steps as f32;
    let mut g = 0.0;
    let mut o = start;
    for _ in 0..steps {
        let next = o + h;
        if over <= next {
            return g + 0.5 * (local(o) + local(over)) * (over - o);
        }
        g += 0.5 * (local(o) + local(next)) * h;
        o = next;
    }
    g + local(end) * (over - end)
}

/// Gain change (dB, <= 0) of the Character Compressor's static curve for an
/// input level `x` (dB) in a model; the engine kernel uses the same laws.
pub fn character_transfer(model: u32, x: f32, thr: f32, ratio: f32, knee: f32, range: f32) -> f32 {
    let over = x - thr;
    let g = match model {
        CLEAN => soft(over, knee, slope(ratio)),
        VARI_MU => vari_mu(over, ratio),
        BUS_VCA => {
            let shift = if ratio < 10.0 { 3.0 * (10.0 - ratio) / 8.0 } else { 0.0 };
            soft(over + shift, 4.0, slope(ratio))
        }
        FET => {
            if ratio < FET_ALL {
                soft(over, 4.0, slope(ratio))
            } else {
                soft(over, 12.0 + 8.0 * (over / 4.0).clamp(0.0, 1.0), slope(ratio))
            }
        }
        OPTO => {
            let (r, k) = if ratio < OPTO_LIMIT_FROM { (OPTO_COMP, 10.0) } else { (OPTO_LIMIT, 6.0) };
            soft(over, k, slope(r))
        }
        _ => soft(over, 10.0 + 10.0 * ((ratio - 1.0) / 9.0).clamp(0.0, 1.0), slope(ratio)),
    };
    g.max(-range.max(0.0)).min(0.0)
}

/// Input interval of the knee (the highlighted bend of the face).
fn bend(model: u32, thr: f32, ratio: f32, knee: f32) -> (f32, f32) {
    let k = match model {
        CLEAN => knee,
        VARI_MU => vari_mu_knee(ratio),
        BUS_VCA => 4.0,
        FET if ratio >= FET_ALL => 20.0,
        FET => 4.0,
        OPTO if ratio < OPTO_LIMIT_FROM => 10.0,
        OPTO => 6.0,
        _ => 10.0 + 10.0 * ((ratio - 1.0) / 9.0).clamp(0.0, 1.0),
    };
    let shift = if model == BUS_VCA && ratio < 10.0 { 3.0 * (10.0 - ratio) / 8.0 } else { 0.0 };
    let m = 3.0 / 26.0 * 36.0;
    (thr - shift - k / 2.0 - m, thr - shift + k / 2.0 + m)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn def() -> &'static DeviceDef {
        &CHARACTER_COMPRESSOR
    }

    fn with(values: &[(&str, f32)]) -> Device {
        let mut d = def().instantiate();
        for (k, v) in values {
            d.params.insert((*k).into(), *v);
        }
        d
    }

    #[test]
    fn parameters_follow_the_reference_order() {
        assert_eq!(def().params.len(), 46);
        assert_eq!(def().index_of("level_tilt"), Some(28));
        assert_eq!(def().index_of("level1_enabled"), Some(29));
        assert_eq!(def().index_of("level4_weight"), Some(44));
        assert_eq!(def().index_of("delta_monitor"), Some(45));
        assert_eq!(def().param("model").unwrap().choices[OPTO as usize], "Opto");
    }

    #[test]
    fn card_is_as_wide_as_its_widest_model() {
        assert_eq!(def().layout.width(), 471.0, "reference: clusters [2,2,1,1] -> 471");
        assert_eq!(def().layout.widest_row().1, 6);
    }

    #[test]
    fn model_shows_its_controls() {
        let shown = |m: f32| -> Vec<&str> {
            let d = with(&[("model", m)]);
            ["knee", "attack", "release", "drive"].into_iter().filter(|p| def().is_shown(&d, p)).collect()
        };
        assert_eq!(shown(0.0), ["knee", "attack", "release"]);
        assert_eq!(shown(1.0), ["attack", "release", "drive"]);
        assert_eq!(shown(4.0), ["drive"]);
        assert_eq!(shown(5.0), ["drive"]);
    }

    #[test]
    fn value_text() {
        let d = def();
        let p = |id| d.param(id).unwrap();
        assert_eq!(p("threshold").format(-18.0), "\u{2212}18.0 dB");
        assert_eq!(p("knee").format(6.0), "6.0 dB");
        assert_eq!(p("ratio").format(4.0), "4.0:1");
        assert_eq!(p("ratio").format(12.0), "12:1");
        assert_eq!(p("ratio").format(30.0), "\u{221e}:1");
        assert_eq!(p("attack").format(0.35), "0.35 ms");
        assert_eq!(p("attack").format(10.0), "10.0 ms");
        assert_eq!(p("attack").format(150.0), "150 ms");
        assert_eq!(p("release").format(200.0), "200 ms");
        assert_eq!(p("release").format(50.0), "50.0 ms");
        assert_eq!(p("makeup").format(0.0), "+0.0 dB");
        assert_eq!(p("mix").format(100.0), "100%");
        assert_eq!(p("drive").format(1.0), "1.00");
        assert_eq!(p("sc_eq_freq").format(1000.0), "1.0 kHz");
        assert_eq!(p("attack").parse("1.5"), Some(p("attack").snap(1.5)));
        assert_eq!(p("release").parse("1.2 s"), Some(1200.0));
        assert_eq!(p("ratio").parse("8:1"), Some(8.0));
        assert_eq!(p("threshold").parse("\u{2212}24 dB"), Some(-24.0));
        assert_eq!(p("sc_hpf_freq").parse("0.2k"), Some(200.0));
        let fet = with(&[("model", 3.0)]);
        assert_eq!(d.text(&fet, p("attack"), 0.35), "DIAL 3");
        assert_eq!(d.text(&fet, p("release"), 1100.0), "DIAL 1");
        assert_eq!(d.text(&fet, p("ratio"), 30.0), "All");
        assert_eq!(d.text(&fet, p("ratio"), 20.0), "20:1");
        let vca = with(&[("model", 2.0)]);
        assert_eq!(d.text(&vca, p("release"), 2000.0), "AUTO");
        assert_eq!(d.text(&vca, p("release"), 1200.0), "1200 ms");
    }

    #[test]
    fn picking_a_model_writes_its_overlay() {
        let d = def().instantiate();
        let v = def().linked_values(&d, "model", 1.0);
        assert_eq!(v["model"], 1.0);
        assert_eq!(v["threshold"], -24.0);
        assert_eq!(v["attack"], def().param("attack").unwrap().snap(0.2));
        assert_eq!(v["release"], 2000.0);
        let v = def().linked_values(&d, "model", 2.0);
        assert_eq!(v["adapt"], 1.0);
        let v = def().linked_values(&d, "model", 0.0);
        assert_eq!((v["threshold"], v["ratio"], v["knee"]), (-18.0, 4.0, 6.0));
    }

    #[test]
    fn vari_mu_times_move_as_pairs_and_auto_release_switches_adapt() {
        let vm = with(&[("model", 1.0), ("attack", 0.2), ("release", 300.0)]);
        assert_eq!(def().linked_values(&vm, "release", 5000.0).get("attack"), Some(&def().param("attack").unwrap().snap(0.8)));
        assert_eq!(def().linked_values(&vm, "attack", 0.4).get("release"), Some(&300.0), "0.4 ms pairs with 300 ms or 2 s: the closer one");
        assert!(!def().linked_values(&vm, "attack", 0.3).contains_key("release"), "between positions: no pair");
        let vca = with(&[("model", 2.0)]);
        assert_eq!(def().linked_values(&vca, "release", 1800.0)["adapt"], 1.0);
        assert_eq!(def().linked_values(&vca, "release", 600.0)["adapt"], 0.0);
    }

    #[test]
    fn opto_ratio_is_a_comp_limit_latch() {
        let o = with(&[("model", 4.0), ("ratio", 3.0)]);
        let l = (BEHAVIOR.latch)(def(), &o, def().param("ratio").unwrap()).unwrap();
        assert_eq!((l.labels, l.values, l.on), (["Comp", "Limit"], [3.0, 8.0], false));
        let o = with(&[("model", 4.0), ("ratio", 6.0)]);
        assert!((BEHAVIOR.latch)(def(), &o, def().param("ratio").unwrap()).unwrap().on);
        assert!((BEHAVIOR.latch)(def(), &def().instantiate(), def().param("ratio").unwrap()).is_none());
        assert_eq!(def().detents(&o, def().param("ratio").unwrap()), [3.0, 8.0]);
    }

    #[test]
    fn static_curves() {
        // Clean: hard knee 4:1, 12 dB over -> -9 dB.
        assert!((character_transfer(CLEAN, -6.0, -18.0, 4.0, 0.0, 24.0) + 9.0).abs() < 1e-4);
        // Range limits the reduction.
        assert_eq!(character_transfer(CLEAN, 0.0, -40.0, 30.0, 0.0, 6.0), -6.0);
        // Opto snaps to 3:1 / 8:1.
        assert!((character_transfer(OPTO, 0.0, -30.0, 4.0, 0.0, 24.0) - character_transfer(OPTO, 0.0, -30.0, 3.0, 0.0, 24.0)).abs() < 1e-6);
        assert!(character_transfer(OPTO, 0.0, -30.0, 8.0, 0.0, 24.0) < character_transfer(OPTO, 0.0, -30.0, 4.0, 0.0, 24.0));
        // Bus VCA starts earlier at low ratios.
        assert!(character_transfer(BUS_VCA, -20.0, -18.0, 2.0, 0.0, 24.0) < 0.0);
        // Every model: no change far below the threshold, monotonic output.
        for m in 0..6 {
            assert_eq!(character_transfer(m, -50.0, -18.0, 4.0, 6.0, 24.0), 0.0, "model {m}");
            let mut last = f32::MIN;
            for i in 0..200 {
                let x = -40.0 + i as f32 * 0.25;
                let y = x + character_transfer(m, x, -18.0, 4.0, 6.0, 24.0);
                assert!(y >= last - 1e-3, "model {m} not monotonic at {x}");
                last = y;
            }
        }
        // Vari-Mu: the effective ratio grows with overshoot.
        let a = character_transfer(VARI_MU, -8.0, -18.0, 2.0, 0.0, 24.0) - character_transfer(VARI_MU, -13.0, -18.0, 2.0, 0.0, 24.0);
        let b = character_transfer(VARI_MU, 2.0, -18.0, 2.0, 0.0, 24.0) - character_transfer(VARI_MU, -3.0, -18.0, 2.0, 0.0, 24.0);
        assert!(b < a, "steeper higher up: {a} {b}");
    }

    #[test]
    fn transfer_face_spans_the_reference_range() {
        let t = transfer(def(), &def().instantiate()).unwrap();
        assert_eq!((t.lo, t.hi), (-30.0, 6.0));
        assert_eq!(t.points.len(), 49);
        assert!(t.bend.0 < -18.0 && t.bend.1 > -18.0);
    }
}
