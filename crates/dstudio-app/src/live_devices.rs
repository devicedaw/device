//! Built-in effects as Live devices, for Export › Project with "Convert effects to Live devices"
//! (spec 019). Each effect that has a Live device doing the same job becomes that device with
//! every setting mapped to Live's units; Live's other settings keep the values a new device has.
//! The parameter lists follow the element order Live saves (structure only, learned from sets
//! saved by Live).

use dstudio_core::Device;
use dstudio_core::builtin::{self, DeviceDef};

/// One element of a Live device.
#[derive(Debug, Clone, PartialEq)]
pub enum LiveParam {
    /// A continuous parameter: value, Live's controller range, and whether it has a modulation
    /// target (choices stored as numbers have a range and no modulation target).
    Float(&'static str, f64, (f64, f64), bool),
    /// An on/off switch.
    Bool(&'static str, bool),
    /// A plain value element (`<Name Value="…" />`).
    Value(&'static str, String),
    /// A group of elements.
    Group(&'static str, Vec<LiveParam>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct LiveDevice {
    /// Live's element name, e.g. "Compressor2".
    pub tag: &'static str,
    /// The device's name in Live, for the export notes.
    pub name: &'static str,
    pub params: Vec<LiveParam>,
    /// Settings that were outside Live's range and clamped, for the export notes.
    pub clamped: Vec<String>,
}

use LiveParam::{Bool, Float, Group, Value};

fn db_to_lin(db: f64) -> f64 {
    10f64.powf(db / 20.0)
}

/// Value of built-in parameter `id` of `d` (its default when unset).
fn val(d: &Device, def: &DeviceDef, id: &str) -> f64 {
    def.param(id).map_or(0.0, |p| def.value(d, p) as f64)
}

struct Clamp<'a> {
    effect: &'a str,
    out: Vec<String>,
}

impl Clamp<'_> {
    /// `v` clamped to `range`, noting a change.
    fn fit(&mut self, what: &str, v: f64, range: (f64, f64), shown: impl Fn(f64) -> String) -> f64 {
        let c = v.clamp(range.0, range.1);
        if (c - v).abs() > 1e-9 {
            self.out.push(format!("{} {what} {} (Live's limit {})", self.effect, shown(v), shown(c)));
        }
        c
    }
}

/// A sidechain group with no source (Compressor, Gate).
fn no_sidechain() -> LiveParam {
    Group(
        "SideChain",
        vec![
            Bool("OnOff", false),
            Group(
                "RoutedInput",
                vec![
                    Group(
                        "Routable",
                        vec![
                            Value("Target", "AudioIn/None".into()),
                            Value("UpperDisplayString", "No Output".into()),
                            Value("LowerDisplayString", String::new()),
                            Group(
                                "MpeSettings",
                                vec![
                                    Value("ZoneType", "0".into()),
                                    Value("FirstNoteChannel", "1".into()),
                                    Value("LastNoteChannel", "15".into()),
                                ],
                            ),
                        ],
                    ),
                    Float("Volume", 1.0, (0.0003162277571, 15.8489332), true),
                ],
            ),
            Float("DryWet", 1.0, (0.0, 1.0), true),
        ],
    )
}

fn compressor(d: &Device, def: &DeviceDef) -> Result<LiveDevice, String> {
    if val(d, def, "sidechainEnable") > 0.5 || d.sidechain.is_some() {
        return Err("its sidechain".into());
    }
    let mut c = Clamp { effect: "Compressor", out: Vec::new() };
    let db = |v: f64| format!("{v:.1} dB");
    let threshold = c.fit("threshold", val(d, def, "threshold"), (-70.0, 6.0), db);
    let gain = c.fit("gain", val(d, def, "makeup_gain") + val(d, def, "output_gain"), (-36.0, 36.0), db);
    let knee = c.fit("knee", val(d, def, "knee"), (0.0, 18.0), db);
    // Live's detector: Peak for the Clean and Punch circuits, RMS for Opto and Smooth.
    let model = match val(d, def, "style").round() as i32 {
        1 | 3 => 1.0,
        _ => 0.0,
    };
    let params = vec![
        Float("Threshold", db_to_lin(threshold), (0.0003162277571, 1.99526238), true),
        Float("Ratio", val(d, def, "ratio"), (1.0, 340282326356119256160033759537265639424.0), true),
        Float("ExpansionRatio", 1.15, (1.0, 2.0), true),
        Float("Attack", val(d, def, "attack"), (0.01, 1000.0), true),
        Float("Release", val(d, def, "release"), (1.0, 3000.0), true),
        Bool("AutoReleaseControlOnOff", false),
        Float("Gain", gain, (-36.0, 36.0), true),
        Bool("GainCompensation", false),
        Float("DryWet", val(d, def, "mix") / 100.0, (0.0, 1.0), true),
        Float("Model", model, (0.0, 2.0), false),
        Float("LegacyModel", 1.0, (0.0, 2.0), false),
        Bool("LogEnvelope", true),
        Float("LegacyEnvFollowerMode", 0.0, (0.0, 2.0), false),
        Float("Knee", knee, (0.0, 18.0), true),
        Float("LookAhead", 0.0, (0.0, 2.0), false),
        Bool("SideListen", false),
        no_sidechain(),
        Group(
            "SideChainEq",
            vec![
                Bool("On", false),
                Float("Mode", 4.0, (0.0, 5.0), false),
                Float("Freq", 800.0, (30.0, 15000.0), true),
                Float("Q", 0.7071067691, (0.1, 12.0), true),
                Float("Gain", 0.0, (-15.0, 15.0), true),
            ],
        ),
        Value("Live8LegacyMode", "false".into()),
        Value("ViewMode", "2".into()),
        Value("IsOutputCurveVisible", "false".into()),
        Value("RmsTimeShort", "8".into()),
        Value("RmsTimeLong", "250".into()),
        Value("ReleaseTimeShort", "15".into()),
        Value("ReleaseTimeLong", "1500".into()),
        Value("CrossfaderSmoothingTime", "10".into()),
    ];
    Ok(LiveDevice { tag: "Compressor2", name: "Compressor", params, clamped: c.out })
}

fn utility(d: &Device, def: &DeviceDef) -> Result<LiveDevice, String> {
    let on = |id: &str| val(d, def, id) > 0.5;
    // Device's order is Stereo, Left, Right, Swap; Live's Left, Stereo, Right, Swap.
    let channel = match val(d, def, "channel_mode").round() as i32 {
        1 => 0.0,
        2 => 2.0,
        3 => 3.0,
        _ => 1.0,
    };
    let params = vec![
        Bool("PhaseInvertL", on("phase_l")),
        Bool("PhaseInvertR", on("phase_r")),
        Float("ChannelMode", channel, (0.0, 3.0), false),
        Float("StereoWidth", val(d, def, "width") / 100.0, (0.0, 4.0), true),
        Float("MidSideBalance", 1.0, (0.0, 2.0), true),
        Value("MidSideBalanceOn", "false".into()),
        Bool("Mono", on("mono")),
        Bool("BassMono", on("bass_mono")),
        Value("BassMonoAudition", on("bass_mono_audition").to_string()),
        Float("BassMonoFrequency", val(d, def, "bass_mono_freq"), (50.0, 500.0), true),
        Float("Balance", val(d, def, "balance") / 50.0, (-1.0, 1.0), true),
        Float("Gain", db_to_lin(val(d, def, "gain")), (0.0, 56.2341309), true),
        Float("LegacyGain", 0.0, (-35.0, 35.0), true),
        Bool("Mute", on("mute")),
        Bool("DcFilter", on("dc_filter")),
        Value("LegacyMode", "false".into()),
    ];
    Ok(LiveDevice { tag: "StereoGain", name: "Utility", params, clamped: Vec::new() })
}

/// Live's EQ Eight filter modes: 0 low cut 48, 1 low cut 12, 2 low shelf, 3 bell, 4 notch,
/// 5 high shelf, 6 high cut 12, 7 high cut 48.
fn eq8_mode(kind: i32, hp_slope: i32, lp_slope: i32) -> f64 {
    match kind {
        1 => 2.0,
        2 => 5.0,
        3 => 4.0,
        4 => {
            if hp_slope >= 1 {
                0.0
            } else {
                1.0
            }
        }
        5 => {
            if lp_slope >= 1 {
                7.0
            } else {
                6.0
            }
        }
        _ => 3.0,
    }
}

fn channel_eq(d: &Device, def: &DeviceDef) -> Result<LiveDevice, String> {
    let mut c = Clamp { effect: "Channel EQ", out: Vec::new() };
    let hp = val(d, def, "hp_slope").round() as i32;
    let lp = val(d, def, "lp_slope").round() as i32;
    let mut params = Vec::new();
    for (b, tag) in BAND_TAGS.iter().enumerate() {
        let id = |s: &str| format!("band{b}_{s}");
        let kind = val(d, def, &id("type")).round() as i32;
        let gain = c.fit(&format!("band {} gain", b + 1), val(d, def, &id("gain2")), (-15.0, 15.0), |v| format!("{v:.1} dB"));
        let q = c.fit(&format!("band {} Q", b + 1), val(d, def, &id("q")), (0.1, 18.0), |v| format!("{v:.2}"));
        let side = |on: bool, mode: f64, freq: f64, gain: f64, q: f64| {
            vec![
                Bool("IsOn", on),
                Float("Mode", mode, (0.0, 7.0), false),
                Float("Freq", freq, (10.0, 22000.0), true),
                Float("Gain", gain, (-15.0, 15.0), true),
                Float("Q", q, (0.1, 18.0), true),
            ]
        };
        let a = side(val(d, def, &id("enabled")) > 0.5, eq8_mode(kind, hp, lp), val(d, def, &id("freq")), gain, q);
        // The B side (mid/side editing) is unused: a copy of A, as Live keeps it in stereo mode.
        let bside = a.clone();
        params.push(Group(tag, vec![Group("ParameterA", a), Group("ParameterB", bside)]));
    }
    params.extend([
        Value("Precision", "0".into()),
        Value("Mode", "0".into()),
        Value("EditMode", "false".into()),
        Value("SelectedBand", "0".into()),
        Float("GlobalGain", c.fit("output gain", val(d, def, "output_gain"), (-12.0, 12.0), |v| format!("{v:.1} dB")), (-12.0, 12.0), true),
        Float("Scale", 1.0, (-2.0, 2.0), true),
        Bool("AdaptiveQ", false),
    ]);
    Ok(LiveDevice { tag: "Eq8", name: "EQ Eight", params, clamped: c.out })
}

const BAND_TAGS: [&str; 8] = ["Bands.0", "Bands.1", "Bands.2", "Bands.3", "Bands.4", "Bands.5", "Bands.6", "Bands.7"];

/// The Live device for effect `d`, or why it has none.
pub fn convert(d: &Device) -> Result<LiveDevice, String> {
    let Some(def) = builtin::def_of(d) else { return Err(format!("{} is a plugin", d.display_name())) };
    match def.id {
        "compressor" => compressor(d, def),
        "utility" => utility(d, def),
        "channeleq" => channel_eq(d, def),
        _ => Err(format!("{} has no Live device", def.name)),
    }
    .map_err(|why| if why.contains(def.name) { why } else { format!("{} ({why})", def.name) })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn find<'a>(p: &'a [LiveParam], name: &str) -> Option<&'a LiveParam> {
        p.iter().find(|x| match x {
            Float(n, ..) | Bool(n, _) | Value(n, _) | Group(n, _) => *n == name,
        })
    }
    fn float(p: &[LiveParam], name: &str) -> f64 {
        match find(p, name) {
            Some(Float(_, v, ..)) => *v,
            other => panic!("{name}: {other:?}"),
        }
    }

    fn with(id: &str, values: &[(&str, f32)]) -> Device {
        let mut d = builtin::find(id).unwrap().instantiate();
        for (k, v) in values {
            d.params.insert((*k).to_string(), *v);
        }
        d
    }

    #[test]
    fn compressor_settings_map_to_live_units() {
        let d = with(
            "compressor",
            &[
                ("threshold", -20.0),
                ("ratio", 3.0),
                ("attack", 15.0),
                ("release", 200.0),
                ("makeup_gain", 4.0),
                ("output_gain", -1.0),
                ("mix", 50.0),
                ("knee", 30.0),
                ("style", 1.0),
            ],
        );
        let l = convert(&d).unwrap();
        assert_eq!(l.tag, "Compressor2");
        assert!((float(&l.params, "Threshold") - 0.1).abs() < 1e-6, "-20 dB is 0.1");
        assert_eq!(float(&l.params, "Ratio"), 3.0);
        assert_eq!(float(&l.params, "Attack"), 15.0);
        assert_eq!(float(&l.params, "Release"), 200.0);
        assert_eq!(float(&l.params, "Gain"), 3.0, "make-up plus output");
        assert_eq!(float(&l.params, "DryWet"), 0.5);
        assert_eq!(float(&l.params, "Model"), 1.0, "Opto listens like RMS");
        assert_eq!(float(&l.params, "Knee"), 18.0);
        assert_eq!(l.clamped.len(), 1, "the knee: {:?}", l.clamped);
    }

    #[test]
    fn a_compressor_with_a_sidechain_has_no_live_device() {
        let d = with("compressor", &[("sidechainEnable", 1.0)]);
        assert!(convert(&d).unwrap_err().contains("sidechain"));
    }

    #[test]
    fn utility_and_channel_eq_map_their_settings() {
        let u = convert(&with("utility", &[("gain", -6.0), ("width", 150.0), ("balance", -25.0), ("channel_mode", 1.0), ("mono", 1.0)]))
            .unwrap();
        assert!((float(&u.params, "Gain") - 0.501187).abs() < 1e-5);
        assert_eq!((float(&u.params, "StereoWidth"), float(&u.params, "Balance"), float(&u.params, "ChannelMode")), (1.5, -0.5, 0.0));
        assert_eq!(find(&u.params, "Mono"), Some(&Bool("Mono", true)));
        let e = convert(&with(
            "channeleq",
            &[("band2_type", 0.0), ("band2_freq", 1000.0), ("band2_gain2", 20.0), ("band2_q", 2.0), ("band2_enabled", 1.0)],
        ))
        .unwrap();
        let Some(Group(_, sides)) = find(&e.params, "Bands.2") else { panic!("no band 3") };
        let Some(Group(_, a)) = find(sides, "ParameterA") else { panic!("no A side") };
        assert_eq!((float(a, "Mode"), float(a, "Freq"), float(a, "Gain"), float(a, "Q")), (3.0, 1000.0, 15.0, 2.0));
        assert_eq!(e.clamped.len(), 1, "the +20 dB gain: {:?}", e.clamped);
    }

    #[test]
    fn other_effects_have_none() {
        assert!(convert(&with("reverb", &[])).unwrap_err().contains("no Live device"));
    }
}
