//! Mastering Limiter: parameter ids, ranges, defaults, tapers, the panel
//! arrangement and the factory presets.

use super::*;
use crate::device::PluginKind;

const OFF_ON: &[&str] = &["Off", "On"];
const STYLES: &[&str] = &["Clean", "Loud", "Gentle", "Dense"];
const STEREO: &[&str] = &["Stereo", "Mid-Side"];
const OVERSAMPLING: &[&str] = &["1x", "2x", "4x", "8x"];
const DITHER_BITS: &[&str] = &["Off", "24", "20", "16"];
const DITHER_SHAPE: &[&str] = &["Flat", "Light", "Normal", "Strong"];

/// What each style sets with it (picking a style rewrites
/// these, one undo step).
const STYLE_OVERLAYS: &[&[(&str, f32)]] = &[
    &[("lookahead", 2.0), ("attack", 10.0), ("release", 150.0), ("auto_release", 1.0), ("transient_link", 75.0), ("release_link", 100.0)],
    &[("lookahead", 1.5), ("attack", 10.0), ("release", 100.0), ("auto_release", 1.0), ("transient_link", 100.0), ("release_link", 100.0)],
    &[("lookahead", 5.0), ("attack", 0.0), ("release", 300.0), ("auto_release", 1.0), ("transient_link", 100.0), ("release_link", 100.0)],
    &[("lookahead", 3.0), ("attack", 30.0), ("release", 60.0), ("auto_release", 0.0), ("transient_link", 100.0), ("release_link", 100.0)],
];

pub static MASTERING_LIMITER: DeviceDef = DeviceDef {
    id: "mastering-limiter",
    name: "Mastering Limiter",
    kind: PluginKind::Effect,
    category: "Dynamics",
    purpose: "Final-stage loudness: pushes the mix into a true-peak-safe ceiling with a two-stage limiter, oversampling and dither.",
    params: &[
        ParamDef::new("threshold", "Threshold", -30.0, 0.0, 0.0, LINEAR, "dB", Format::Plain(1))
            .step(0.1)
            .help("Level that is pushed up to the ceiling; lower values make the master louder."),
        ParamDef::new("ceiling", "Ceiling", -30.0, 0.0, -1.0, LINEAR, "dB", Format::Plain(1)).step(0.1).help("Highest output level."),
        ParamDef::new("release", "Release", 0.01, 1000.0, 150.0, Taper::Linear { skew: 0.25 }, "ms", Format::Ms(0))
            .help("How fast the limiter recovers."),
        ParamDef::choice("auto_release", "Auto Release", OFF_ON, 1)
            .help("Recovers fast after short peaks and slowly through sustained loud passages."),
        ParamDef::choice("style", "Style", STYLES, 0)
            .not_automatable()
            .overlays(STYLE_OVERLAYS)
            .help("Clean is transparent, Loud recovers fastest, Gentle is a single slow stage, Dense leans on the slow stage."),
        ParamDef::new("lookahead", "Lookahead", 0.1, 5.0, 2.0, LINEAR, "ms", Format::Ms(2))
            .step(0.01)
            .not_automatable()
            .help("How far ahead peaks are seen; longer is smoother, shorter is punchier."),
        ParamDef::new("attack", "Attack", 0.0, 100.0, 10.0, Taper::Linear { skew: 0.3 }, "ms", Format::Ms(1))
            .help("Speed of the slow stage; slower lets more transient through to the fast stage."),
        ParamDef::new("transient_link", "Transient Link", 0.0, 100.0, 75.0, LINEAR, "%", Format::Percent)
            .step(1.0)
            .help("How much the channels share the fast peak reduction."),
        ParamDef::new("release_link", "Release Link", 0.0, 100.0, 100.0, LINEAR, "%", Format::Percent)
            .step(1.0)
            .help("How much the channels share the slow reduction."),
        ParamDef::choice("ms_mode", "M/S Mode", STEREO, 0)
            .not_automatable()
            .help("Unlinked reduction works on mid and side instead of left and right."),
        ParamDef::choice("oversampling", "Oversampling", OVERSAMPLING, 2)
            .not_automatable()
            .help("Runs the limiter at a higher rate to reduce aliasing and inter-sample overs."),
        ParamDef::choice("true_peak", "True Peak", OFF_ON, 1).not_automatable().help("Holds the ceiling between samples (dBTP)."),
        ParamDef::choice("dither_bits", "Dither Bits", DITHER_BITS, 0).not_automatable().help("Word length of the dithered output."),
        ParamDef::choice("dither_shape", "Dither Shape", DITHER_SHAPE, 2)
            .not_automatable()
            .help("How far the dither noise is shaped towards high frequencies."),
        ParamDef::choice("bypass", "Bypass", OFF_ON, 0).not_automatable().help("Passes the signal through, delay compensated."),
        ParamDef::choice("unity_monitor", "Unity Monitor", OFF_ON, 0)
            .not_automatable()
            .reset_on_load()
            .help("Listen level-matched: removes the push so only the change in sound is heard."),
        ParamDef::choice("delta_monitor", "Delta Monitor", OFF_ON, 0)
            .not_automatable()
            .reset_on_load()
            .help("Listen to what the limiter removes."),
    ],
    layout: Layout {
        picker: &[Control::Menu("style"), Control::Toggle("auto_release")],
        view: Some(View::Strip(Strip::new("threshold").range(12.0, -36.0).second("ceiling"))),
        sliders: &[],
        clusters: &[
            &[Control::Knob("threshold"), Control::Knob("ceiling")],
            &[Control::Knob("release")],
            &[Control::Knob("lookahead"), Control::Knob("attack")],
        ],
        faces: &[("ceiling", Face::PushWall { threshold: "threshold", ceiling: "ceiling" })],
        fixed_width: 0.0,
        min_height: 203.0,
        shown_when: &[],
        ..Layout::EMPTY
    },
    factory: &[
        FactoryPreset { name: "Streaming Safe", values: &[("threshold", -3.0), ("ceiling", -1.5), ("lookahead", 3.0)] },
        FactoryPreset {
            name: "Open and Clear",
            values: &[("threshold", -2.0), ("ceiling", -1.0), ("release", 250.0), ("lookahead", 4.0), ("transient_link", 60.0)],
        },
        FactoryPreset {
            name: "Warm Master",
            values: &[
                ("threshold", -5.0),
                ("ceiling", -1.0),
                ("release", 400.0),
                ("style", 2.0),
                ("lookahead", 4.5),
                ("attack", 2.0),
                ("transient_link", 100.0),
            ],
        },
        FactoryPreset {
            name: "Club Pressure",
            values: &[
                ("threshold", -11.0),
                ("ceiling", -0.8),
                ("release", 80.0),
                ("style", 1.0),
                ("lookahead", 1.2),
                ("attack", 8.0),
                ("transient_link", 100.0),
            ],
        },
        FactoryPreset {
            name: "CD Master 16-bit",
            values: &[("threshold", -6.0), ("ceiling", -0.3), ("dither_bits", 3.0), ("dither_shape", 2.0)],
        },
        FactoryPreset {
            name: "Punch First",
            values: &[
                ("threshold", -6.0),
                ("ceiling", -1.0),
                ("release", 40.0),
                ("auto_release", 0.0),
                ("lookahead", 0.8),
                ("attack", 1.0),
                ("transient_link", 20.0),
            ],
        },
        FactoryPreset {
            name: "Thick and Even",
            values: &[
                ("threshold", -9.0),
                ("ceiling", -1.0),
                ("release", 90.0),
                ("auto_release", 0.0),
                ("style", 3.0),
                ("lookahead", 2.5),
                ("attack", 40.0),
                ("transient_link", 100.0),
            ],
        },
        FactoryPreset {
            name: "Wide Master",
            values: &[("threshold", -4.0), ("ceiling", -1.0), ("ms_mode", 1.0), ("transient_link", 40.0), ("release_link", 60.0)],
        },
    ],
    sidechain: false,
    samples: 0,
};

/// The ceiling reads in dBTP while True Peak limiting is on: "−1.0 dBTP".
fn text(def: &DeviceDef, d: &Device, p: &ParamDef, v: f32) -> Option<String> {
    let true_peak = def.param("true_peak").is_some_and(|tp| def.value(d, tp) >= 0.5);
    (p.id == "ceiling" && true_peak).then(|| if v < -0.05 { format!("\u{2212}{:.1} dBTP", -v) } else { "0.0 dBTP".into() })
}

pub(super) static BEHAVIOR: Behavior = Behavior { text, ..Behavior::NONE };

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ceiling_reads_true_peak_while_limiting_true_peak() {
        let def = &MASTERING_LIMITER;
        let mut d = def.instantiate();
        let ceiling = def.param("ceiling").unwrap();
        assert_eq!(def.text(&d, ceiling, -1.0), "\u{2212}1.0 dBTP");
        d.params.insert("true_peak".into(), 0.0);
        assert!(!def.text(&d, ceiling, -1.0).contains("TP"));
    }
}
