//! Parameter automation.
//!
//! A lane belongs to a track (or the master) and targets one parameter. Points
//! store **plain values in the target's own unit** (volume in dB, pan -1..1,
//! synth parameters in their `SynthParams` unit, plugin parameters normalized
//! 0..1), so labels need no conversion and lanes survive range changes in the
//! UI. Between points the value follows a curve shaped by the left point's
//! `curve` (see [`shape`]); this exact formula is the contract the engine
//! implements (engine/README.md › Automation).

use crate::device::DeviceId;
use crate::model::Ticks;
use serde::{Deserialize, Serialize};

/// Continuous built-in synth parameters that can be automated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SynthParam {
    Osc1Level,
    Osc1Shape,
    Osc1Fine,
    Osc2Level,
    Osc2Shape,
    Osc2Fine,
    CrossFm,
    SubLevel,
    NoiseLevel,
    FilterCutoff,
    FilterResonance,
    FilterKeyTrack,
    FilterEnvAmount,
    LfoRate,
    LfoToPitch,
    LfoToCutoff,
    LfoToAmp,
    Glide,
    UnisonDetune,
    Output,
}

impl SynthParam {
    pub const ALL: [SynthParam; 20] = [
        SynthParam::Osc1Level,
        SynthParam::Osc1Shape,
        SynthParam::Osc1Fine,
        SynthParam::Osc2Level,
        SynthParam::Osc2Shape,
        SynthParam::Osc2Fine,
        SynthParam::CrossFm,
        SynthParam::SubLevel,
        SynthParam::NoiseLevel,
        SynthParam::FilterCutoff,
        SynthParam::FilterResonance,
        SynthParam::FilterKeyTrack,
        SynthParam::FilterEnvAmount,
        SynthParam::LfoRate,
        SynthParam::LfoToPitch,
        SynthParam::LfoToCutoff,
        SynthParam::LfoToAmp,
        SynthParam::Glide,
        SynthParam::UnisonDetune,
        SynthParam::Output,
    ];

    /// (min, max) in the `SynthParams` unit (matches `SynthParams::validate`).
    pub fn range(self) -> (f32, f32) {
        use SynthParam::*;
        match self {
            Osc1Level | Osc2Level | Osc1Shape | Osc2Shape | CrossFm | SubLevel | NoiseLevel | FilterResonance | FilterKeyTrack
            | LfoToAmp => (0.0, 1.0),
            Osc1Fine | Osc2Fine => (-100.0, 100.0),
            FilterCutoff => (20.0, 20_000.0),
            FilterEnvAmount => (-1.0, 1.0),
            LfoRate => (0.01, 20.0),
            LfoToPitch => (0.0, 1200.0),
            LfoToCutoff => (0.0, 6.0),
            Glide => (0.0, 2.0),
            UnisonDetune => (0.0, 50.0),
            Output => (0.0, 2.0),
        }
    }

    pub fn name(self) -> &'static str {
        use SynthParam::*;
        match self {
            Osc1Level => "Osc 1 Level",
            Osc1Shape => "Osc 1 Shape",
            Osc1Fine => "Osc 1 Fine",
            Osc2Level => "Osc 2 Level",
            Osc2Shape => "Osc 2 Shape",
            Osc2Fine => "Osc 2 Fine",
            CrossFm => "Cross FM",
            SubLevel => "Sub Level",
            NoiseLevel => "Noise Level",
            FilterCutoff => "Cutoff",
            FilterResonance => "Resonance",
            FilterKeyTrack => "Key Track",
            FilterEnvAmount => "Env Amount",
            LfoRate => "LFO Rate",
            LfoToPitch => "LFO → Pitch",
            LfoToCutoff => "LFO → Cutoff",
            LfoToAmp => "LFO → Amp",
            Glide => "Glide",
            UnisonDetune => "Unison Detune",
            Output => "Output",
        }
    }

    /// Frequency-like parameters interpolate geometrically.
    pub fn logarithmic(self) -> bool {
        matches!(self, SynthParam::FilterCutoff | SynthParam::LfoRate)
    }

    /// Read the current static value from a `SynthParams`.
    pub fn get(self, p: &crate::instrument::SynthParams) -> f32 {
        use SynthParam::*;
        match self {
            Osc1Level => p.osc[0].level,
            Osc1Shape => p.osc[0].shape,
            Osc1Fine => p.osc[0].fine_cents,
            Osc2Level => p.osc[1].level,
            Osc2Shape => p.osc[1].shape,
            Osc2Fine => p.osc[1].fine_cents,
            CrossFm => p.cross_fm,
            SubLevel => p.sub_level,
            NoiseLevel => p.noise_level,
            FilterCutoff => p.filter.cutoff_hz,
            FilterResonance => p.filter.resonance,
            FilterKeyTrack => p.filter.key_track,
            FilterEnvAmount => p.filter.env_amount,
            LfoRate => p.lfo.rate_hz,
            LfoToPitch => p.lfo.to_pitch_cents,
            LfoToCutoff => p.lfo.to_cutoff_oct,
            LfoToAmp => p.lfo.to_amp,
            Glide => p.voice.glide,
            UnisonDetune => p.voice.unison_detune_cents,
            Output => p.output,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AutomationTarget {
    /// dB, -60..+6 (−60 is treated as silence).
    Volume,
    /// -1..1.
    Pan,
    /// Built-in synth parameter (instrument tracks without an instrument plugin).
    Synth { param: SynthParam },
    /// Plugin parameter by its format id, normalized 0..1. `name` is for display.
    Plugin { device: DeviceId, param: u32, name: String },
    /// Built-in device parameter (crate::builtin), plain value in the
    /// parameter's unit. `device_type` is the definition id.
    Builtin { device: DeviceId, device_type: String, param: String },
    /// Track mute: 1 = muted, 0 = playing (stepped).
    Mute,
    /// A device's power switch (insert or instrument): 1 = on, 0 = off
    /// (stepped). `name` is the device's display name, for the lane pill.
    DeviceOn { device: DeviceId, name: String },
    /// Channel-strip trim in dB (`Track::TRIM_RANGE_DB`).
    Trim,
    /// Channel-strip stereo width, 0..2 (1 = unchanged).
    Width,
    /// Channel-strip mono fold: 1 = mono (stepped).
    Mono,
    /// The song tempo in BPM (spec 015). Not a lane the project stores: the app shows the tempo
    /// map (`Transport::tempo`) through it on the master row, a point's `curve` 0 holding its tempo
    /// and anything else ramping linearly to the next point, and commits `Command::SetTempoMap`.
    /// `SetAutomationLane` refuses it.
    Tempo,
}

pub const VOLUME_MIN_DB: f32 = -60.0;
pub const VOLUME_MAX_DB: f32 = 6.0;

impl AutomationTarget {
    pub fn range(&self) -> (f32, f32) {
        match self {
            AutomationTarget::Volume => (VOLUME_MIN_DB, VOLUME_MAX_DB),
            AutomationTarget::Pan => (-1.0, 1.0),
            AutomationTarget::Synth { param } => param.range(),
            AutomationTarget::Plugin { .. } => (0.0, 1.0),
            AutomationTarget::Builtin { .. } => self.builtin_param().map(|p| (p.min, p.max)).unwrap_or((0.0, 1.0)),
            AutomationTarget::Mute | AutomationTarget::DeviceOn { .. } | AutomationTarget::Mono => (0.0, 1.0),
            AutomationTarget::Trim => crate::Track::TRIM_RANGE_DB,
            AutomationTarget::Width => crate::Track::WIDTH_RANGE,
            AutomationTarget::Tempo => (crate::command::MIN_BPM as f32, crate::command::MAX_BPM as f32),
        }
    }

    /// Stepped targets hold each point's value until the next point (switches
    /// and choices: no in-between values). Part of the engine contract.
    pub fn stepped(&self) -> bool {
        match self {
            AutomationTarget::Mute | AutomationTarget::DeviceOn { .. } | AutomationTarget::Mono => true,
            AutomationTarget::Builtin { .. } => self.builtin_param().is_some_and(|p| p.is_discrete()),
            _ => false,
        }
    }

    /// Channel-strip targets (volume, pan, mute, trim, width, mono): the track's own controls,
    /// not a device's.
    pub fn is_channel(&self) -> bool {
        matches!(
            self,
            AutomationTarget::Volume
                | AutomationTarget::Pan
                | AutomationTarget::Mute
                | AutomationTarget::Trim
                | AutomationTarget::Width
                | AutomationTarget::Mono
        )
    }

    pub fn logarithmic(&self) -> bool {
        match self {
            AutomationTarget::Synth { param } => param.logarithmic(),
            AutomationTarget::Builtin { .. } => self.builtin_param().is_some_and(|p| p.taper == crate::builtin::Taper::Log),
            _ => false,
        }
    }

    /// Definition of a built-in device target's parameter.
    pub fn builtin_param(&self) -> Option<&'static crate::builtin::ParamDef> {
        match self {
            AutomationTarget::Builtin { device_type, param, .. } => crate::builtin::find(device_type)?.param(param),
            _ => None,
        }
    }

    /// Category and parameter label, as on the lane pill ("Track", "Volume").
    pub fn labels(&self) -> (String, String) {
        match self {
            AutomationTarget::Volume => ("Track".into(), "Volume".into()),
            AutomationTarget::Pan => ("Track".into(), "Pan".into()),
            AutomationTarget::Synth { param } => ("Synth".into(), param.name().into()),
            AutomationTarget::Plugin { name, .. } => ("Plugin".into(), name.clone()),
            AutomationTarget::Builtin { device_type, param, .. } => match crate::builtin::find(device_type) {
                Some(def) => (def.name.into(), def.param(param).map(|p| p.name).unwrap_or(param).into()),
                None => ("Device".into(), param.clone()),
            },
            AutomationTarget::Mute => ("Track".into(), "Mute".into()),
            AutomationTarget::Trim => ("Track".into(), "Trim".into()),
            AutomationTarget::Width => ("Track".into(), "Width".into()),
            AutomationTarget::Mono => ("Track".into(), "Mono".into()),
            AutomationTarget::DeviceOn { name, .. } => (name.clone(), "On".into()),
            AutomationTarget::Tempo => ("Song".into(), "Tempo".into()),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct AutoPoint {
    pub time: Ticks,
    pub value: f32,
    /// Shape of the segment to the next point, -1..1 (0 = linear).
    #[serde(default)]
    pub curve: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AutomationLane {
    pub target: AutomationTarget,
    /// "Enable automation" power toggle; disabled lanes are kept but not played.
    pub enabled: bool,
    pub points: Vec<AutoPoint>,
}

/// Segment shape: maps t in 0..1 to 0..1. c > 0 bends toward the end (slow
/// start), c < 0 toward the start (fast start); c = 0 is linear.
/// THIS FORMULA IS THE ENGINE CONTRACT.
pub fn shape(t: f32, c: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    let c = c.clamp(-1.0, 1.0);
    if c.abs() < 1e-6 {
        t
    } else if c > 0.0 {
        t.powf(1.0 + 3.0 * c)
    } else {
        1.0 - (1.0 - t).powf(1.0 - 3.0 * c)
    }
}

/// Interpolate between two plain values (geometric for logarithmic targets).
pub fn lerp_value(a: f32, b: f32, s: f32, logarithmic: bool) -> f32 {
    if logarithmic && a > 0.0 && b > 0.0 { (a.ln() + (b.ln() - a.ln()) * s).exp() } else { a + (b - a) * s }
}

impl AutomationLane {
    pub fn new(target: AutomationTarget) -> Self {
        AutomationLane { target, enabled: true, points: Vec::new() }
    }

    /// Value at `t` ticks: before the first point the first value, after the
    /// last point the last value; None if the lane has no points.
    pub fn value_at(&self, t: Ticks) -> Option<f32> {
        let p = &self.points;
        let first = p.first()?;
        if t <= first.time {
            return Some(first.value);
        }
        let i = p.partition_point(|q| q.time <= t);
        if i >= p.len() {
            return Some(p[p.len() - 1].value);
        }
        let (a, b) = (p[i - 1], p[i]);
        if b.time == a.time {
            return Some(b.value);
        }
        if self.target.stepped() || (self.target == AutomationTarget::Tempo && a.curve == 0.0) {
            return Some(a.value);
        }
        if self.target == AutomationTarget::Tempo {
            // A tempo ramp is linear in BPM over beats (ticks).
            return Some(a.value + (b.value - a.value) * ((t - a.time) as f64 / (b.time - a.time) as f64) as f32);
        }
        let s = shape((t - a.time) as f32 / (b.time - a.time) as f32, a.curve);
        Some(lerp_value(a.value, b.value, s, self.target.logarithmic()))
    }

    pub fn validate(&self) -> Result<(), String> {
        let (lo, hi) = self.target.range();
        let mut prev = Ticks::MIN;
        for (i, q) in self.points.iter().enumerate() {
            if q.time < 0 {
                return Err(format!("point {i}: time < 0"));
            }
            if q.time < prev {
                return Err(format!("point {i}: points must be sorted by time"));
            }
            if !q.value.is_finite() || q.value < lo || q.value > hi {
                return Err(format!("point {i}: value {} outside {lo}..={hi}", q.value));
            }
            if !q.curve.is_finite() || q.curve.abs() > 1.0 {
                return Err(format!("point {i}: curve {} outside -1..=1", q.curve));
            }
            prev = q.time;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lane(points: &[(Ticks, f32, f32)]) -> AutomationLane {
        AutomationLane {
            target: AutomationTarget::Volume,
            enabled: true,
            points: points.iter().map(|&(time, value, curve)| AutoPoint { time, value, curve }).collect(),
        }
    }

    #[test]
    fn evaluation_holds_ends_and_interpolates() {
        let l = lane(&[(100, -12.0, 0.0), (200, 0.0, 0.0), (200, -6.0, 0.0)]);
        assert_eq!(l.value_at(0), Some(-12.0));
        assert_eq!(l.value_at(150), Some(-6.0));
        assert_eq!(l.value_at(200), Some(-6.0), "a vertical step takes the later point at its time");
        assert_eq!(l.value_at(10_000), Some(-6.0));
        assert_eq!(lane(&[]).value_at(5), None);
    }

    #[test]
    fn switch_lanes_hold_each_value() {
        let mut l = lane(&[(0, 0.0, 0.0), (100, 1.0, 0.0), (200, 0.0, 0.0)]);
        l.target = AutomationTarget::Mute;
        assert!(l.target.stepped());
        assert_eq!(l.value_at(99), Some(0.0), "no ramp between switch points");
        assert_eq!(l.value_at(100), Some(1.0));
        assert_eq!(l.value_at(199), Some(1.0));
        assert_eq!(l.value_at(200), Some(0.0));
        l.target = AutomationTarget::DeviceOn { device: DeviceId(7), name: "EQ".into() };
        assert_eq!(l.value_at(150), Some(1.0));
        assert_eq!(l.target.range(), (0.0, 1.0));
        assert_eq!(l.target.labels(), ("EQ".to_string(), "On".to_string()));
        assert_eq!(AutomationTarget::Mute.labels(), ("Track".to_string(), "Mute".to_string()));
        assert!(!AutomationTarget::Volume.stepped() && !AutomationTarget::Pan.stepped());
        assert!(AutomationTarget::Mono.stepped() && !AutomationTarget::Trim.stepped() && !AutomationTarget::Width.stepped());
        assert_eq!((AutomationTarget::Trim.range(), AutomationTarget::Width.range()), ((-24.0, 24.0), (0.0, 2.0)));
        assert!(AutomationTarget::Mono.is_channel() && !l.target.is_channel());
    }

    #[test]
    fn switch_targets_round_trip_through_json() {
        for t in [
            AutomationTarget::Mute,
            AutomationTarget::DeviceOn { device: DeviceId(3), name: "Delay".into() },
            AutomationTarget::Trim,
            AutomationTarget::Width,
            AutomationTarget::Mono,
        ] {
            let lane = AutomationLane { target: t.clone(), enabled: true, points: vec![AutoPoint { time: 0, value: 1.0, curve: 0.0 }] };
            let json = serde_json::to_string(&lane).unwrap();
            let back: AutomationLane = serde_json::from_str(&json).unwrap();
            assert_eq!(back, lane, "{json}");
        }
    }

    #[test]
    fn curve_shapes() {
        assert_eq!(shape(0.5, 0.0), 0.5);
        assert!(shape(0.5, 1.0) < 0.1 && shape(0.5, -1.0) > 0.9);
        for c in [-1.0, -0.3, 0.0, 0.4, 1.0] {
            assert_eq!((shape(0.0, c), shape(1.0, c)), (0.0, 1.0));
        }
        let geo = lerp_value(100.0, 10_000.0, 0.5, true);
        assert!((geo - 1000.0).abs() < 0.5);
    }

    #[test]
    fn validation() {
        assert!(lane(&[(0, -12.0, 0.0), (10, 0.0, 0.5)]).validate().is_ok());
        assert!(lane(&[(10, 0.0, 0.0), (0, 0.0, 0.0)]).validate().is_err());
        assert!(lane(&[(0, 12.0, 0.0)]).validate().is_err());
        assert!(lane(&[(0, 0.0, 2.0)]).validate().is_err());
        assert!(
            AutomationLane {
                target: AutomationTarget::Synth { param: SynthParam::FilterCutoff },
                enabled: true,
                points: vec![AutoPoint { time: 0, value: 10.0, curve: 0.0 }]
            }
            .validate()
            .is_err()
        );
    }
}
