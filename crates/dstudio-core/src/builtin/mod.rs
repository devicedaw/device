//! Built-in devices: one declarative definition per device.
//!
//! A [`DeviceDef`] lists a device's parameters (id, label, range, default,
//! taper, unit, display format, text values), its panel layout and its factory
//! presets. Everything else is derived from it:
//!
//! * model state: a built-in device is an ordinary [`Device`] in a track's
//!   chain (`PluginFormat::Builtin`, `plugin.id` = definition id) whose plain
//!   parameter values live in `Device::params`; add / remove / reorder /
//!   bypass are the existing chain commands;
//! * edits: `Command::SetDeviceParam` (validated against the definition,
//!   knob drags coalesce per parameter) and `Command::SetDeviceParams`
//!   (preset loads, one undo step);
//! * persistence: values are saved with the project by id, so adding a
//!   parameter to a definition later keeps old projects loading (missing ids
//!   read as the default);
//! * automation: `AutomationTarget::Builtin` lanes hold plain values in the
//!   parameter's unit; LFO / macro targets come from [`mod_targets`];
//! * engine: the snapshot sends the values in definition order and the engine
//!   runs the processor registered under the same id (engine/src/devices);
//! * UI: the generic device panel (crates/dstudio-app/src/device_panel.rs).
//!
//! Normalisation: log parameters map 0..1 to
//! `min * (max / min)^n`, linear ones to `min + (max - min) * n^(1 / skew)`;
//! stepped linear parameters snap in the plain domain. Discrete parameters
//! store the choice index as their plain value (`min` = 0, `max` = choices - 1).

mod auto_filter;
mod autopanner;
mod bitcrusher;
mod channel_eq;
mod character_compressor;
mod character_eq;
pub mod character_mod;
mod chorus;
mod compressor;
mod declipper;
mod deesser;
mod dehummer;
pub mod delay;
mod denoiser;
mod distortion;
pub mod drum_rack;
pub mod echo;
pub mod eq;
mod flanger;
mod gate;
mod glue_compressor;
pub mod grains;
pub mod lfo;
mod limiter;
mod mastering_limiter;
mod multiband_compressor;
pub mod pad_sampler;
mod phaser;
mod pitch_corrector;
pub mod pitch_shifter;
pub mod rack;
pub mod reverb;
mod ring_mod;
pub mod sampler;
pub mod shaper;
mod soft_clipper;
mod spectrum;
mod tape_saturation;
pub mod trance_gate;
mod transient_shaper;
mod tremolo;
mod tuner;

pub use tuner::TUNING_STRINGS;
pub mod synth_v2;
mod utility;
mod widener;

pub use bitcrusher::crush_picture;
pub use distortion::distortion_transfer;
pub use soft_clipper::{clipper_open, clipper_transfer};
pub use tape_saturation::tape_transfer;
pub use widener::widener_response;

pub use auto_filter::{FilterSetting, dj_map, filter_response};
pub use character_compressor::character_transfer;
pub use multiband_compressor::{MULTIBAND_BANDS, multiband_gain};

use crate::device::{Device, DeviceId, PluginFormat, PluginKind, PluginRef};
use crate::model::Track;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// How a parameter's plain range maps to the 0..1 control range.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Taper {
    /// `min + (max - min) * n^(1 / skew)` (skew 1 = straight line).
    Linear { skew: f32 },
    /// `min * (max / min)^n`; needs `min > 0`.
    Log,
}

pub const LINEAR: Taper = Taper::Linear { skew: 1.0 };

/// Display and text-entry format of a value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    /// Signed decibels, one decimal: "−24.0 dB", "+3.0 dB".
    Db,
    /// A cut stored as a positive amount, shown as a negative level: 8 -> "−8.0 dB".
    /// Typed text may carry either sign.
    DbCut,
    /// Milliseconds, whole numbers when the range spans 20 ms or more ("10 ms", "1500 ms"), else one decimal.
    Time,
    /// Hertz: "4.0 kHz" from 999.5 Hz, "440 Hz" from 10 Hz, one decimal from 1 Hz, else two.
    Hz,
    /// The generic Hz text: two decimals below 1 Hz ("0.50 Hz"),
    /// one below 10 Hz ("5.0 Hz"), whole numbers below 999.5 Hz ("440 Hz"),
    /// then kilohertz with one decimal ("2.2 kHz"). LFO rates, carriers.
    Freq,
    /// Milliseconds: one decimal below 100 ms ("1.0 ms"), whole numbers from 100 ms ("150 ms").
    MsAuto,
    /// Number with this many decimals, an explicit sign and the unit, but a bare zero:
    /// "+12 st", "−7 st", "0 st" (`Signed` shows "+0 st").
    SignedBareZero(u8),
    /// Whole degrees: "90°".
    Degrees,
    /// Compression ratio: one decimal when the range spans less than 20 ("4.0:1"), else whole numbers ("10:1").
    Ratio,
    /// Milliseconds with a fixed number of decimals ("0.50 ms", "100 ms"); typed seconds are converted.
    Ms(u8),
    /// Percent, at most one decimal: "100%", "12.5%", "−40%".
    Percent,
    /// Whole percent: "30%".
    Percent0,
    /// Decibels, one decimal, a minus sign below −0.05 and never a plus: "0.0 dB", "−1.5 dB".
    DbNoPlus,
    /// Signed decibels like `Db`, but the minimum of the range reads "−∞ dB" (off).
    DbOrInf,
    /// Multiplier: one decimal below 10 ("2.5x"), whole numbers above ("12x").
    Factor,
    /// Left / right balance: "C" at the centre, else "25L" / "25R".
    Balance,
    /// Percent with an explicit sign, whole numbers: "+35%", "−100%", "0%".
    SignedPercent,
    /// Choice name from `ParamDef::choices`.
    Choice,
    /// Number with this many decimals and the unit appended when not empty
    /// (a multiplier unit "x" attaches without a space: "8.0x").
    Plain(u8),
    /// Signed number (plain value × `scale`) with the unit: "+0 st", "−12 ct",
    /// "+24 st" for an envelope depth of 0.5 shown in semitones (scale 48).
    Signed { decimals: u8, scale: u16 },
    /// Pan position -1..1: "C", "25L", "40R" (rounded percent).
    Pan,
    /// MIDI note number as a name, middle C (60) = "C3".
    Note,
    /// A 0..1 value shown as a percent: "30%", "12.5%".
    Fraction,
    /// A -1..1 value shown as a signed percent: "+25%", "−10%", "0%".
    SignedFraction,
    /// Seconds, shown in ms below one second ("4.0 ms", "900 ms") and in s above ("1.20 s").
    Seconds,
    /// Hertz with three significant digits: "0.18 Hz", "10.0 Hz", "440 Hz", "2.40 kHz", "13.5 kHz".
    Hz3,
    /// Signed semitones with one decimal: "+12.0 st", "−3.5 st".
    Semitones,
    /// Signed whole cents: "+7 ct", "0 ct", "−12 ct".
    Cents,
    /// LFO rate: "2.00 Hz" below 10 Hz, "12.5 Hz" above.
    Rate,
    /// Device-specific text (`ParamDef::parse` reads numbers with the
    /// parameter's unit: "250 ms", "1.2 s", "2k" for Hz).
    Custom(TextFn),
}

/// Display function of `Format::Custom` (compared by address).
#[derive(Clone, Copy)]
pub struct TextFn(pub fn(f32) -> String);

impl PartialEq for TextFn {
    fn eq(&self, other: &TextFn) -> bool {
        self.0 as usize == other.0 as usize
    }
}
impl Eq for TextFn {}
impl std::fmt::Debug for TextFn {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("TextFn")
    }
}

/// One parameter of a built-in device.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ParamDef {
    /// Stable id: persistence, automation, presets.
    pub id: &'static str,
    /// Full name (automation picker, menus, accessibility).
    pub name: &'static str,
    /// Short label on the panel control (often the same as `name`).
    pub label: &'static str,
    pub min: f32,
    pub max: f32,
    pub default: f32,
    pub taper: Taper,
    /// Plain-domain step (0 = continuous). Ignored for log parameters.
    pub step: f32,
    pub unit: &'static str,
    pub format: Format,
    /// Text values of a discrete parameter; the plain value is the index.
    pub choices: &'static [&'static str],
    /// Shown in the automation picker and as an LFO target.
    pub automatable: bool,
    /// Reset to the default when a preset loads (audition switches).
    pub reset_on_load: bool,
    /// A preset load leaves the current value alone (seeds, locks).
    pub preset_lock: bool,
    /// One-sentence description (tooltips, the assistant).
    pub help: &'static str,
    /// Knobs show and drag the reversed 0..1 position (note-length dials:
    /// longer notes sit higher).
    pub reversed: bool,
    /// Latch texts (off, on) of a two-state parameter in the picker row,
    /// e.g. ("Dry Off", "Dry On"); None = the choice names.
    pub latch: Option<(&'static str, &'static str)>,
    /// A latch that is on only while held (press writes 1, release 0).
    pub momentary: bool,
    /// While this (two-state) parameter is on, preset loads keep the current
    /// values of these parameters (and of this one).
    pub locks: &'static [&'static str],
    /// Text shown instead of the number at the top of the range ("Full"); "" = none.
    pub max_text: &'static str,
    /// Discrete parameters: values written together with each choice, by
    /// choice index (one undo step), e.g. a style that also sets its times.
    pub overlays: &'static [&'static [(&'static str, f32)]],
}

impl ParamDef {
    /// A continuous parameter with defaults for the optional fields.
    #[allow(clippy::too_many_arguments)]
    pub const fn new(
        id: &'static str,
        name: &'static str,
        min: f32,
        max: f32,
        default: f32,
        taper: Taper,
        unit: &'static str,
        format: Format,
    ) -> ParamDef {
        ParamDef {
            id,
            name,
            label: name,
            min,
            max,
            default,
            taper,
            step: 0.0,
            unit,
            format,
            choices: &[],
            automatable: true,
            reset_on_load: false,
            preset_lock: false,
            help: "",
            reversed: false,
            latch: None,
            momentary: false,
            locks: &[],
            max_text: "",
            overlays: &[],
        }
    }

    /// A discrete parameter: plain value = index into `choices`.
    pub const fn choice(id: &'static str, name: &'static str, choices: &'static [&'static str], default: u32) -> ParamDef {
        ParamDef {
            id,
            name,
            label: name,
            min: 0.0,
            max: (choices.len() - 1) as f32,
            default: default as f32,
            taper: LINEAR,
            step: 1.0,
            unit: "",
            format: Format::Choice,
            choices,
            automatable: true,
            reset_on_load: false,
            preset_lock: false,
            help: "",
            reversed: false,
            latch: None,
            momentary: false,
            locks: &[],
            max_text: "",
            overlays: &[],
        }
    }

    pub const fn label(mut self, label: &'static str) -> ParamDef {
        self.label = label;
        self
    }
    pub const fn step(mut self, step: f32) -> ParamDef {
        self.step = step;
        self
    }
    pub const fn help(mut self, help: &'static str) -> ParamDef {
        self.help = help;
        self
    }
    pub const fn not_automatable(mut self) -> ParamDef {
        self.automatable = false;
        self
    }
    pub const fn reset_on_load(mut self) -> ParamDef {
        self.reset_on_load = true;
        self
    }
    pub const fn max_text(mut self, text: &'static str) -> ParamDef {
        self.max_text = text;
        self
    }
    pub const fn overlays(mut self, overlays: &'static [&'static [(&'static str, f32)]]) -> ParamDef {
        self.overlays = overlays;
        self
    }

    /// The number a value stands for: the plain value, or for a discrete
    /// parameter the leading number of its choice name ("4:1" -> 4, "0.3 ms"
    /// -> 0.3); NaN for a choice without one ("Auto").
    pub fn number(&self, v: f32) -> f32 {
        if !self.is_discrete() {
            return v;
        }
        let name = self.choices.get(v.round().max(0.0) as usize).copied().unwrap_or("");
        let end = name.find(|c: char| !(c.is_ascii_digit() || c == '.')).unwrap_or(name.len());
        name[..end].parse().unwrap_or(f32::NAN)
    }

    fn at_max_text(&self, v: f32) -> bool {
        !self.max_text.is_empty() && v >= self.max - 5e-4 * (self.max - self.min)
    }
    pub const fn preset_locked(mut self) -> ParamDef {
        self.preset_lock = true;
        self
    }

    pub const fn reversed(mut self) -> ParamDef {
        self.reversed = true;
        self
    }
    pub const fn latch(mut self, off: &'static str, on: &'static str) -> ParamDef {
        self.latch = Some((off, on));
        self
    }
    pub const fn momentary(mut self) -> ParamDef {
        self.momentary = true;
        self
    }
    pub const fn locks(mut self, ids: &'static [&'static str]) -> ParamDef {
        self.locks = ids;
        self
    }
    /// UI / state plumbing written by gestures and presets: no automation, no LFO.
    pub const fn internal(self) -> ParamDef {
        self.not_automatable()
    }

    pub fn is_discrete(&self) -> bool {
        !self.choices.is_empty()
    }

    /// Normalized position the value arc / slider fill starts from: the 0 point when the
    /// range spans 0 (bipolar and offset ranges such as −12..24 dB), else the minimum.
    pub fn origin(&self) -> f32 {
        if self.min < 0.0 && self.max > 0.0 { self.to_normalized(0.0) } else { 0.0 }
    }

    /// Range symmetric around 0 (−x..x).
    pub fn is_bipolar(&self) -> bool {
        self.min < 0.0 && self.max > 0.0 && (self.min + self.max).abs() <= 1e-6 * (self.max - self.min)
    }

    /// Nearest valid plain value: clamped, stepped (round half up),
    /// discrete values rounded.
    pub fn snap(&self, v: f32) -> f32 {
        let v = if v.is_finite() { v.clamp(self.min, self.max) } else { self.default };
        if self.step <= 0.0 || self.taper == Taper::Log {
            return v;
        }
        // In f64 on the decimal step (0.1, not 0.100000001), so 120 steps of 0.1 from -12 land on 0.
        let dec = |x: f32| (x as f64 * 1e6).round() / 1e6;
        let k = ((v - self.min) / self.step + 0.501).floor() as f64;
        ((((dec(self.min) + k * dec(self.step)) * 1e6).round() / 1e6) as f32).clamp(self.min, self.max)
    }

    /// Plain value -> 0..1.
    pub fn to_normalized(&self, v: f32) -> f32 {
        let v = v.clamp(self.min, self.max);
        let n = match self.taper {
            Taper::Log => (v / self.min).ln() / (self.max / self.min).ln(),
            Taper::Linear { skew } => {
                let n = ((v - self.min) / (self.max - self.min)).clamp(0.0, 1.0);
                if skew != 1.0 && n > 0.0 { n.powf(skew) } else { n }
            }
        };
        n.clamp(0.0, 1.0)
    }

    /// 0..1 -> plain value (snapped).
    pub fn from_normalized(&self, n: f32) -> f32 {
        let n = if n.is_finite() { n.clamp(0.0, 1.0) } else { 0.0 };
        let v = match self.taper {
            Taper::Log => self.min * (self.max / self.min).powf(n),
            Taper::Linear { skew } => {
                let n = if skew != 1.0 && n > 0.0 { n.powf(1.0 / skew) } else { n };
                self.min + (self.max - self.min) * n
            }
        };
        self.snap(v)
    }

    /// Display text of a plain value.
    pub fn format(&self, v: f32) -> String {
        if self.at_max_text(v) {
            return self.max_text.to_string();
        }
        if self.format == Format::DbOrInf && v <= self.min + 1e-4 * (self.max - self.min) {
            return format!("{MINUS}\u{221e} dB");
        }
        format_value(self.format, v, self.unit, self.choices, self.max - self.min)
    }

    /// Parse typed text ("-12", "−12 dB", "250ms", "1.2 s", "2k", "Opto")
    /// into a snapped plain value. None when it can't be read.
    pub fn parse(&self, text: &str) -> Option<f32> {
        let t = text.trim();
        if t.is_empty() {
            return None;
        }
        if !self.max_text.is_empty() && t.eq_ignore_ascii_case(self.max_text) {
            return Some(self.max);
        }
        if self.is_discrete() {
            let norm = |s: &str| s.to_lowercase().replace('\u{2212}', "-");
            let lower = norm(t);
            if let Some(i) = self.choices.iter().position(|c| norm(c) == lower) {
                return Some(i as f32);
            }
            // Slope names are shown with or without their unit ("24" / "24 dB/oct").
            let bare = |s: &str| s.trim_end_matches("db/oct").trim().to_string();
            if let Some(i) = self.choices.iter().position(|c| bare(&c.to_lowercase()) == bare(&lower)) {
                return Some(i as f32);
            }
        }
        let t = t.replace('\u{2212}', "-").replace(',', ".");
        let lower = t.to_lowercase();
        if self.format == Format::DbOrInf && ["-inf", "-infinity", "-\u{221e}"].iter().any(|w| lower.trim_end_matches("db").trim() == *w) {
            return Some(self.min);
        }
        if self.format == Format::Balance {
            return parse_balance(&lower).map(|v| self.snap(v));
        }
        match self.format {
            Format::Pan => return parse_pan(&t).map(|v| self.snap(v)),
            Format::Note => return parse_note(&t).map(|v| self.snap(v)),
            _ => {}
        }
        let split = t.find(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-' || c == '+')).unwrap_or(t.len());
        let (num, unit) = t.split_at(split);
        let mut v: f32 = num.parse().ok()?;
        let unit = unit.trim().to_lowercase();
        let unit = unit.trim_end_matches(":1").trim();
        match (self.format, unit) {
            (_, "") => {}
            (Format::Time | Format::Ms(_), "s" | "sec" | "secs" | "second" | "seconds") => v *= 1000.0,
            (Format::Time | Format::Ms(_), "ms" | "msec" | "msecs") => {}
            (Format::Hz | Format::Freq | Format::Hz3, "k" | "khz") => v *= 1000.0,
            (Format::Hz | Format::Freq | Format::Hz3, "hz") => {}
            (Format::MsAuto, "ms" | "msec" | "msecs") => {}
            (Format::MsAuto, "s" | "sec" | "secs" | "second" | "seconds") => v *= 1000.0,
            (Format::Degrees, "°" | "deg" | "degrees") => {}
            (Format::SignedBareZero(_), u) if u == self.unit.to_lowercase() => {}
            (Format::Db | Format::DbNoPlus | Format::DbOrInf | Format::DbCut, "db")
            | (Format::Percent | Format::Percent0 | Format::SignedPercent, "%")
            | (Format::Factor, "x") => {}
            (Format::Fraction | Format::SignedFraction, "%" | "percent") => {}
            (Format::Seconds, "s" | "sec" | "secs" | "second" | "seconds") => {}
            (Format::Seconds, "ms" | "msec" | "msecs") => v /= 1000.0,
            (Format::Semitones, "st" | "semitones") | (Format::Cents, "ct" | "cents") => {}
            (Format::Plain(_) | Format::Custom(_), u) if u == self.unit.to_lowercase().trim_end_matches(":1") => {}
            (Format::Custom(_), "%") if self.unit == "%" => {}
            (Format::Custom(_), "s" | "sec" | "secs" | "second" | "seconds") if self.unit == "ms" => v *= 1000.0,
            (Format::Custom(_), "k" | "khz") if self.unit == "Hz" => v *= 1000.0,
            (Format::Rate, "hz") => {}
            (Format::Signed { .. }, u) if u == self.unit.to_lowercase() => {}
            _ => return None,
        }
        match self.format {
            Format::Fraction | Format::SignedFraction => v /= 100.0,
            Format::Signed { scale, .. } => v /= scale.max(1) as f32,
            _ => {}
        }
        if !v.is_finite() {
            return None;
        }
        if self.format == Format::DbCut {
            v = v.abs();
        }
        Some(self.snap(v))
    }
}

const MINUS: char = '\u{2212}';

/// Display text for a value (shared by the panel, automation labels and presets).
/// `range` (max - min) picks the decimals of `Format::Time`.
pub fn format_value(format: Format, v: f32, unit: &str, choices: &[&str], range: f32) -> String {
    match format {
        Format::Db => {
            let a = (v.abs() * 10.0).round() / 10.0;
            let sign = if v <= -0.05 { MINUS } else { '+' };
            format!("{sign}{a:.1} dB")
        }
        Format::DbCut => {
            let a = (v.abs() * 10.0).round() / 10.0;
            if a < 0.05 { "0.0 dB".into() } else { format!("{MINUS}{a:.1} dB") }
        }
        Format::Time => {
            let d = if range < 20.0 { 1 } else { 0 };
            format!("{v:.d$} ms")
        }
        Format::Hz => {
            if v >= 10_000.0 {
                format!("{:.1} kHz", v / 1000.0)
            } else if v >= 1000.0 {
                format!("{:.2} kHz", v / 1000.0)
            } else {
                format!("{v:.0} Hz")
            }
        }
        Format::DbNoPlus => {
            let a = (v.abs() * 10.0).round() / 10.0;
            if v <= -0.05 { format!("{MINUS}{a:.1} dB") } else { format!("{a:.1} dB") }
        }
        Format::DbOrInf => format_value(Format::Db, v, unit, choices, range),
        Format::Percent0 => format!("{:.0}%", v.round() + 0.0),
        Format::Factor => {
            if v < 9.95 {
                format!("{v:.1}x")
            } else {
                format!("{v:.0}x")
            }
        }
        Format::Balance => {
            let r = v.round();
            if r == 0.0 {
                "C".into()
            } else if r < 0.0 {
                format!("{:.0}L", -r)
            } else {
                format!("{r:.0}R")
            }
        }
        Format::Freq => {
            if v >= 999.5 {
                format!("{:.1} kHz", v / 1000.0)
            } else if v >= 9.995 {
                format!("{v:.0} Hz")
            } else if v >= 0.995 {
                format!("{v:.1} Hz")
            } else {
                format!("{v:.2} Hz")
            }
        }
        Format::MsAuto => {
            if v >= 99.95 {
                format!("{v:.0} ms")
            } else {
                format!("{v:.1} ms")
            }
        }
        Format::SignedBareZero(d) => {
            let a = format!("{:.*}", d as usize, v.abs());
            let zero = a.chars().all(|c| c == '0' || c == '.');
            let sign = if zero {
                String::new()
            } else if v < 0.0 {
                MINUS.to_string()
            } else {
                "+".into()
            };
            if unit.is_empty() { format!("{sign}{a}") } else { format!("{sign}{a} {unit}") }
        }
        Format::Degrees => format!("{:.0}°", v.round() + 0.0),
        Format::Ratio => {
            let d = if range < 20.0 { 1 } else { 0 };
            format!("{v:.d$}:1")
        }
        Format::Ms(d) => format!("{v:.*} ms", d as usize),
        Format::Percent => {
            let r = (v * 10.0).round() / 10.0;
            let s = if r.fract() == 0.0 { format!("{:.0}%", r.abs()) } else { format!("{:.1}%", r.abs()) };
            if r < 0.0 { format!("{MINUS}{s}") } else { s }
        }
        Format::SignedPercent => {
            let r = v.round();
            if r == 0.0 {
                "0%".into()
            } else if r < 0.0 {
                format!("{MINUS}{}%", -r)
            } else {
                format!("+{r}%")
            }
        }
        Format::Choice => {
            let i = v.round().max(0.0) as usize;
            choices.get(i).copied().unwrap_or("").to_string()
        }
        Format::Plain(d) => {
            let s = format!("{:.*}", d as usize, v).replace('-', &MINUS.to_string());
            match unit {
                "" => s,
                // Multipliers attach to the number ("8.0x").
                "x" => format!("{s}x"),
                _ => format!("{s} {unit}"),
            }
        }
        Format::Signed { decimals, scale } => {
            let x = v * scale.max(1) as f32;
            let p = 10f32.powi(decimals as i32);
            let r = (x * p).round() / p;
            let sign = if r < 0.0 { MINUS } else { '+' };
            let s = format!("{sign}{:.*}", decimals as usize, r.abs());
            if unit.is_empty() { s } else { format!("{s} {unit}") }
        }
        Format::Pan => {
            let pct = (v * 100.0).round() as i32;
            match pct {
                0 => "C".into(),
                p if p < 0 => format!("{}L", -p),
                p => format!("{p}R"),
            }
        }
        Format::Note => note_name(v.round() as i32),
        Format::Fraction => format_value(Format::Percent, v * 100.0, unit, choices, range),
        Format::SignedFraction => {
            let r = (v * 1000.0).round() / 10.0;
            let body = if r.fract() == 0.0 { format!("{:.0}%", r.abs()) } else { format!("{:.1}%", r.abs()) };
            if r == 0.0 {
                body
            } else if r < 0.0 {
                format!("{MINUS}{body}")
            } else {
                format!("+{body}")
            }
        }
        Format::Seconds => {
            if v < 1.0 {
                let ms = v * 1000.0;
                if ms < 99.95 { format!("{ms:.1} ms") } else { format!("{ms:.0} ms") }
            } else if v < 9.995 {
                format!("{v:.2} s")
            } else {
                format!("{v:.1} s")
            }
        }
        Format::Hz3 => {
            if v >= 9999.5 {
                format!("{:.1} kHz", v / 1000.0)
            } else if v >= 999.5 {
                format!("{:.2} kHz", v / 1000.0)
            } else if v >= 99.95 {
                format!("{v:.0} Hz")
            } else if v >= 9.995 {
                format!("{v:.1} Hz")
            } else {
                format!("{v:.2} Hz")
            }
        }
        Format::Semitones => {
            let a = (v.abs() * 10.0).round() / 10.0;
            let sign = if a == 0.0 {
                ""
            } else if v < 0.0 {
                "\u{2212}"
            } else {
                "+"
            };
            format!("{sign}{a:.1} st")
        }
        Format::Cents => {
            let r = v.round();
            if r == 0.0 {
                "0 ct".into()
            } else if r < 0.0 {
                format!("{MINUS}{} ct", -r)
            } else {
                format!("+{r} ct")
            }
        }
        Format::Rate => {
            if v < 10.0 {
                format!("{v:.2} Hz")
            } else {
                format!("{v:.1} Hz")
            }
        }
        Format::Custom(f) => (f.0)(v),
    }
}

const NOTE_NAMES: [&str; 12] = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];

/// MIDI note name with middle C (60) = "C3".
pub fn note_name(n: i32) -> String {
    format!("{}{}", NOTE_NAMES[n.rem_euclid(12) as usize], n.div_euclid(12) - 2)
}

/// "C3", "c#3", "Db-1", or a plain note number.
fn parse_note(t: &str) -> Option<f32> {
    let t = t.trim();
    if let Ok(n) = t.parse::<f32>() {
        return n.is_finite().then_some(n.round());
    }
    let mut chars = t.chars();
    let letter = chars.next()?.to_ascii_uppercase();
    let base = [('C', 0), ('D', 2), ('E', 4), ('F', 5), ('G', 7), ('A', 9), ('B', 11)].iter().find(|(c, _)| *c == letter)?.1;
    let rest = chars.as_str();
    let (acc, oct) = match rest.chars().next() {
        Some('#') => (1, &rest[1..]),
        Some('b') if rest.len() > 1 => (-1, &rest[1..]),
        _ => (0, rest),
    };
    let oct: i32 = oct.trim().parse().ok().filter(|o| (-10..=20).contains(o))?;
    Some(((oct + 2) * 12 + base + acc) as f32)
}

/// "C", "L25", "25L", "R40", "40R", or a signed percent ("-25" = 25L).
fn parse_pan(t: &str) -> Option<f32> {
    let t = t.trim().to_lowercase();
    if t == "c" || t == "center" || t == "centre" {
        return Some(0.0);
    }
    let num = |s: &str| s.trim().trim_end_matches('%').trim().parse::<f32>().ok().filter(|v| v.is_finite());
    if let Some(n) = t.strip_prefix('l').or_else(|| t.strip_suffix('l')) {
        return num(n).map(|v| -v.abs() / 100.0);
    }
    if let Some(n) = t.strip_prefix('r').or_else(|| t.strip_suffix('r')) {
        return num(n).map(|v| v.abs() / 100.0);
    }
    num(&t).map(|v| v / 100.0)
}

/// Decibels with one decimal and a U+2212 minus, no plus sign: "−18.0 dB", "6.0 dB".
pub fn db_text(v: f32) -> String {
    let a = (v.abs() * 10.0).round() / 10.0;
    if v <= -0.05 { format!("{MINUS}{a:.1} dB") } else { format!("{a:.1} dB") }
}

/// "C", "25L", "L25", "25 r", "-25" (left negative) -> balance value.
fn parse_balance(t: &str) -> Option<f32> {
    let t = t.trim();
    if t == "c" || t == "center" || t == "centre" {
        return Some(0.0);
    }
    let (side, num) = if let Some(n) = t.strip_suffix('l').or_else(|| t.strip_prefix('l')) {
        (-1.0, n)
    } else if let Some(n) = t.strip_suffix('r').or_else(|| t.strip_prefix('r')) {
        (1.0, n)
    } else {
        (1.0, t)
    };
    let v: f32 = num.trim().parse().ok()?;
    (v.is_finite() && (side > 0.0 || v >= 0.0)).then_some(side * v)
}

// ---------------------------------------------------------------- layout

/// A control on the generic device panel.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Control {
    /// Rotary knob: value above, label below (fx dial row).
    Knob(&'static str),
    /// Vertical slider (fx slider column, 62 pt wide).
    Slider(&'static str),
    /// Option pill with a menu (picker row): label + chosen value + chevron.
    Menu(&'static str),
    /// Value pill dragged like a knob (picker row), e.g. sidechain gain.
    Pill(&'static str),
    /// Two-state latch button (On/Off parameters). In the dial row it takes a
    /// knob cell (latch + caption); in the picker row it is a right-aligned
    /// pill reading "<label> <choice>" ("Auto Release On").
    Toggle(&'static str),
    /// Sidechain source picker (a track list, not a parameter).
    Sidechain,
    /// Option pill that opens a menu of the choices (picker row): label +
    /// chosen value + chevron, for lists too long for a segmented track.
    Dropdown(&'static str),
    /// Segmented picker whose segments show these labels instead of the
    /// parameter's choice names (e.g. a Sync Off/On parameter shown as Free / Sync).
    Segments(&'static str, &'static [&'static str]),
    /// Segmented switch between [`Layout::views`] (UI state of the card, not a parameter).
    Views,
    /// Flexible gap in the picker row: the controls after it align right.
    Gap,
    /// One knob cell showing the first of these parameters that is shown
    /// (`Layout::shown_when`), e.g. Rate in Hz or Rate Sync as a note value.
    Swap(&'static [&'static str]),
    /// Latch pill (fx latch) labelled with the parameter's label,
    /// filled while the two-state parameter is on; click toggles.
    Latch(&'static str),
    /// Menu pill over two discrete parameters: the choices of \`param\`, then
    /// the non-zero choices of \`mode\`. Picking a \`param\` entry writes it and
    /// sets \`mode\` to 0; picking a mode entry sets \`mode\` (one undo step).
    ModeDropdown { param: &'static str, mode: &'static str },
}

impl Control {
    /// The (first) parameter of the control.
    pub fn param(&self) -> Option<&'static str> {
        match *self {
            Control::Knob(p)
            | Control::Slider(p)
            | Control::Menu(p)
            | Control::Pill(p)
            | Control::Toggle(p)
            | Control::Dropdown(p)
            | Control::Segments(p, _)
            | Control::Latch(p) => Some(p),
            Control::ModeDropdown { param, .. } => Some(param),
            Control::Swap(ps) => ps.first().copied(),
            Control::Sidechain | Control::Views | Control::Gap => None,
        }
    }

    /// Every parameter the control can show.
    pub fn params(&self) -> Vec<&'static str> {
        match *self {
            Control::Swap(ps) => ps.to_vec(),
            Control::ModeDropdown { param, mode } => vec![param, mode],
            _ => self.param().into_iter().collect(),
        }
    }
}

/// A visualiser or custom view drawn by the panel code.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum View {
    /// Scrolling level / gain-reduction strip with a draggable threshold
    /// line bound to `threshold` (dynamics strip), plot 0..-36 dB.
    /// Shorthand for `View::Strip(Strip::new(threshold))`.
    DynamicsStrip { threshold: &'static str },
    /// Dynamics strip with its own plot range and an optional second line.
    Strip(Strip),
    /// A device-specific view implemented in the panel code under this name
    /// (escape hatch for EQ curves, shaper, trance gate grid...).
    Custom(&'static str),
    /// Parametric EQ body (band tabs, response field with band nodes, type
    /// picker and value cells), described by an [`eq::EqView`].
    Eq(&'static eq::EqView),
    /// Plot well with traces computed by the panel code (motion,
    /// response and sweep fields) that edits two parameters by dragging.
    Field(Field),
}

/// A plotted field in the view row (motion and
/// response fields): caption strip, tick labels, up to three traces, a fill,
/// an optional live LFO overlay, and an XY drag.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Field {
    /// Curve model in the panel code (crates/dstudio-app/src/device_fields.rs).
    pub plot: &'static str,
    /// Caption at the top left ("" = no caption strip; the well starts at the top).
    pub caption: &'static str,
    /// Horizontal drag: the first of these parameters that is shown (Rate / Rate Sync).
    pub x: &'static [&'static str],
    /// Vertical drag: the first of these parameters that is shown.
    pub y: &'static [&'static str],
    /// Width of the field in pt; the card is at least this plus
    /// the slider column and padding wide.
    pub width: f32,
}

impl Field {
    pub const fn new(
        plot: &'static str,
        caption: &'static str,
        x: &'static [&'static str],
        y: &'static [&'static str],
        width: f32,
    ) -> Field {
        Field { plot, caption, x, y, width }
    }
}

impl View {
    /// The dynamics strip of this view, if it is one.
    pub fn strip(&self) -> Option<Strip> {
        match *self {
            View::DynamicsStrip { threshold } => Some(Strip::new(threshold)),
            View::Strip(s) => Some(s),
            View::Field(_) | View::Custom(_) | View::Eq(_) => None,
        }
    }
}

/// Dynamics strip settings: draggable lines (dB parameters, capped at 0 dB)
/// and the plot range of the well.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Strip {
    /// First line (dashed): the threshold or ceiling the strip edits.
    pub line: &'static str,
    /// Optional second line (solid); Alt+drag moves both by the same dB.
    pub second: Option<&'static str>,
    /// Plot top and floor (dB). Lines below the floor are drawn dotted at it.
    pub top_db: f32,
    pub floor_db: f32,
}

impl Strip {
    pub const fn new(line: &'static str) -> Strip {
        Strip { line, second: None, top_db: 0.0, floor_db: -36.0 }
    }
    pub const fn range(mut self, top_db: f32, floor_db: f32) -> Strip {
        self.top_db = top_db;
        self.floor_db = floor_db;
        self
    }
    pub const fn second(mut self, line: &'static str) -> Strip {
        self.second = Some(line);
        self
    }
}

/// Drawing inside a knob instead of the plain body.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Face {
    /// Static transfer curve of a soft-knee compressor.
    SoftKneeCurve { threshold: &'static str, ratio: &'static str, knee: &'static str },
    /// Brick-wall limiter: output = min(input, ceiling).
    BrickWall { ceiling: &'static str },
    /// Downward expander / gate below the threshold.
    Expander { threshold: &'static str, ratio: &'static str },
    /// Bus VCA compressor: fixed 4 dB soft knee, ratio-dependent threshold
    /// offset, reduction limited by `range` (discrete ratios read as numbers).
    BusVca { threshold: &'static str, ratio: &'static str, range: &'static str },
    /// Mastering limiter "push": gain of ceiling - threshold below the
    /// threshold, pinned at the ceiling above it.
    PushWall { threshold: &'static str, ceiling: &'static str },
    /// Transfer curve computed by the device (`Behavior::transfer`).
    Transfer,
}

/// A transfer curve for a knob face: (input dB, output dB) points over
/// `lo..hi` and the input interval drawn as the highlighted bend.
#[derive(Debug, Clone, PartialEq)]
pub struct Transfer {
    pub lo: f32,
    pub hi: f32,
    pub points: Vec<(f32, f32)>,
    pub bend: (f32, f32),
}

/// A knob that becomes a two-state latch in some states (Character
/// Compressor's Opto ratio: Comp / Limit).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Latch {
    pub labels: [&'static str; 2],
    /// Value written by each state.
    pub values: [f32; 2],
    pub on: bool,
}

/// State-dependent panel behaviour of a device, looked up with [`behavior`]:
/// what depends on other parameters (mode-dependent detents, value text,
/// latches, computed transfer faces; linked writes are [`linked_writes`]). Devices without one use
/// the static definition only.
pub struct Behavior {
    /// Snap points (plain values) of a control: drags snap to the nearest one
    /// within 0.025 of the 0..1 range, arrow keys jump between them.
    pub detents: fn(&DeviceDef, &Device, &ParamDef) -> &'static [f32],
    /// Display text that depends on other parameters; None = `ParamDef::format`.
    pub text: fn(&DeviceDef, &Device, &ParamDef, f32) -> Option<String>,
    /// The control of `param` is a latch in this state.
    pub latch: fn(&DeviceDef, &Device, &ParamDef) -> Option<Latch>,
    /// Curve of a `Face::Transfer` knob face.
    pub transfer: fn(&DeviceDef, &Device) -> Option<Transfer>,
    /// Label that depends on other parameters (a dial called "Time L" while Link is off); None = `ParamDef::label`.
    pub label: fn(&DeviceDef, &Device, &ParamDef) -> Option<&'static str>,
    /// Typed text (trimmed, lower case) -> plain value, the inverse of `text`; None = `ParamDef::parse`.
    pub read: fn(&DeviceDef, &Device, &ParamDef, &str) -> Option<f32>,
}

impl Behavior {
    /// No state-dependent behaviour (for `..Behavior::NONE`).
    pub const NONE: Behavior = Behavior {
        detents: |_, _, _| &[],
        text: |_, _, _, _| None,
        latch: |_, _, _| None,
        transfer: |_, _| None,
        label: |_, _, _| None,
        read: |_, _, _, _| None,
    };
}

/// The state-dependent behaviour of a device, if it has one.
pub fn behavior(def: &DeviceDef) -> Option<&'static Behavior> {
    match def.id {
        "character-compressor" => Some(&character_compressor::BEHAVIOR),
        "delay" => Some(&delay::BEHAVIOR),
        "echo" => Some(&echo::BEHAVIOR),
        "grains" => Some(&grains::BEHAVIOR),
        "mastering-limiter" => Some(&mastering_limiter::BEHAVIOR),
        "reverb" => Some(&reverb::BEHAVIOR),
        _ => None,
    }
}

/// Panel layout, top to bottom: picker row, view row (view + slider column),
/// dial row (clusters of knobs). This is the fx card template;
/// devices with a different body use `View::Custom`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Layout {
    /// Picker row; a `Control::Sidechain` and everything after it align right.
    pub picker: &'static [Control],
    pub view: Option<View>,
    /// Slider column at the right of the view.
    pub sliders: &'static [Control],
    /// Dial row: clusters of knobs.
    pub clusters: &'static [&'static [Control]],
    /// Knob faces by parameter id.
    pub faces: &'static [(&'static str, Face)],
    /// Card body minimum height (pt).
    pub min_height: f32,
    /// (param, discrete param, values): `param` is shown only while the
    /// discrete parameter has one of `values` (mode-dependent layouts).
    pub shown_when: &'static [(&'static str, &'static str, &'static [u32])],
    /// Same shape as `shown_when`: the control is drawn dimmed and does not
    /// react while the discrete parameter has one of the values.
    pub inert_when: &'static [(&'static str, &'static str, &'static [u32])],
    /// Card width (pt) for bodies that are not sized by the dial row
    /// (`View::Custom` grids); 0 = derived from `clusters`.
    pub fixed_width: f32,
    /// Names of the card's views (picker `Control::Views`); the custom view draws the chosen one.
    pub views: &'static [&'static str],
    /// Dial-row cells spread evenly across the card instead of packing cluster by cluster.
    pub spread_dials: bool,
}

/// Dial row metrics (pt).
pub const KNOB_CELL: f32 = 62.0;
pub const KNOB_GAP: f32 = 10.0;
pub const CLUSTER_GAP: f32 = 21.0;
pub const CARD_PAD: f32 = 16.0;

impl Layout {
    /// A layout with nothing in it (struct update base for definitions).
    pub const EMPTY: Layout = Layout {
        picker: &[],
        view: None,
        sliders: &[],
        clusters: &[],
        faces: &[],
        min_height: 0.0,
        shown_when: &[],
        inert_when: &[],
        fixed_width: 0.0,
        views: &[],
        spread_dials: false,
    };

    /// Card width from the dial clusters: cells * 62 + inner gaps * 10 +
    /// cluster gaps * 21, plus 16 of padding (the card sizing rule).
    /// With mode-dependent controls (`shown_when`) the card takes the width
    /// of its widest state, so it never changes size when the mode does.
    pub fn width(&self) -> f32 {
        if self.fixed_width > 0.0 {
            return self.fixed_width;
        }
        let dials = self.widest_row().0;
        // A field sets its own width; the view row is field + gap + slider column(s).
        let view = match self.view {
            Some(View::Field(f)) => f.width + self.sliders.len() as f32 * (KNOB_GAP + KNOB_CELL) + CARD_PAD,
            _ => 0.0,
        };
        dials.max(view)
    }

    /// (dial-row width, knob cells) of the widest dial-row state.
    pub fn widest_row(&self) -> (f32, usize) {
        let mut switches: Vec<(&str, Vec<u32>)> = Vec::new();
        for (_, by, values) in self.shown_when {
            let i = switches.iter().position(|(b, _)| b == by).unwrap_or_else(|| {
                switches.push((by, vec![u32::MAX]));
                switches.len() - 1
            });
            switches[i].1.extend(values.iter().copied());
        }
        let combos: usize = switches.iter().map(|(_, v)| v.len()).product();
        (0..combos.max(1))
            .map(|mut k| {
                let state: Vec<(&str, u32)> = switches
                    .iter()
                    .map(|(by, vals)| {
                        let v = vals[k % vals.len()];
                        k /= vals.len();
                        (*by, v)
                    })
                    .collect();
                let shown = |c: &Control| {
                    c.param().is_none_or(|p| {
                        self.shown_when
                            .iter()
                            .filter(|(q, _, _)| *q == p)
                            .all(|(_, by, values)| state.iter().any(|(b, v)| b == by && values.contains(v)))
                    })
                };
                let sizes: Vec<usize> = self.clusters.iter().map(|c| c.iter().filter(|c| shown(c)).count()).filter(|n| *n > 0).collect();
                let cells: usize = sizes.iter().sum();
                let inner: usize = sizes.iter().map(|n| n - 1).sum();
                let gaps = sizes.len().saturating_sub(1);
                (cells as f32 * KNOB_CELL + inner as f32 * KNOB_GAP + gaps as f32 * CLUSTER_GAP + CARD_PAD, cells)
            })
            .fold((CARD_PAD, 0), |a, b| if b.0 > a.0 { b } else { a })
    }

    /// Every control, in panel order.
    pub fn controls(&self) -> impl Iterator<Item = &Control> {
        self.picker.iter().chain(self.sliders.iter()).chain(self.clusters.iter().flat_map(|c| c.iter()))
    }
}

// ---------------------------------------------------------------- devices

/// A factory preset: values by parameter id (missing ids keep their default).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FactoryPreset {
    pub name: &'static str,
    pub values: &'static [(&'static str, f32)],
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DeviceDef {
    /// Stable id (`PluginRef::id`, engine registry key, preset folder).
    pub id: &'static str,
    pub name: &'static str,
    pub kind: PluginKind,
    /// Browser category ("Dynamics", "EQ & Filter", ...).
    pub category: &'static str,
    /// One-sentence purpose (browser, assistant).
    pub purpose: &'static str,
    /// Parameters in engine order: the engine receives values in this order
    /// and the processor declares the same ids (checked by a test).
    pub params: &'static [ParamDef],
    pub layout: Layout,
    pub factory: &'static [FactoryPreset],
    /// The device reads a sidechain input (another track) when one is picked.
    pub sidechain: bool,
    /// Sample slots for audio files the device plays (`Device::samples`);
    /// 0 for devices that play no files.
    pub samples: u32,
}

/// Ids of the hidden switch that follows a device's sidechain picker.
pub const SIDECHAIN_ENABLE_IDS: [&str; 3] = ["sidechainEnable", "sidechain", "sc_source"];

/// The vendor shown for built-in devices.
pub const VENDOR: &str = "Device";

/// Every built-in device, in browser order.
pub fn all() -> &'static [&'static DeviceDef] {
    static ALL: [&DeviceDef; 44] = [
        &compressor::COMPRESSOR,
        &distortion::DISTORTION,
        &tape_saturation::TAPE_SATURATION,
        &soft_clipper::SOFT_CLIPPER,
        &bitcrusher::BITCRUSHER,
        &widener::WIDENER,
        &utility::UTILITY,
        &channel_eq::CHANNEL_EQ,
        &character_eq::CHARACTER_EQ,
        &auto_filter::AUTO_FILTER,
        &glue_compressor::GLUE_COMPRESSOR,
        &limiter::LIMITER,
        &mastering_limiter::MASTERING_LIMITER,
        &gate::GATE,
        &transient_shaper::TRANSIENT_SHAPER,
        &chorus::CHORUS,
        &character_mod::CHARACTER_MOD,
        &flanger::FLANGER,
        &phaser::PHASER,
        &tremolo::TREMOLO,
        &autopanner::AUTOPANNER,
        &ring_mod::RING_MOD,
        &delay::DELAY,
        &echo::ECHO,
        &grains::GRAINS,
        &reverb::REVERB,
        &pitch_shifter::PITCH_SHIFTER,
        &character_compressor::CHARACTER_COMPRESSOR,
        &multiband_compressor::MULTIBAND_COMPRESSOR,
        &lfo::LFO,
        &rack::RACK,
        &sampler::SAMPLER,
        &drum_rack::DRUM_RACK,
        &pad_sampler::PAD_SAMPLER,
        &synth_v2::SYNTH_V2,
        &declipper::DECLIPPER,
        &deesser::DEESSER,
        &dehummer::DEHUMMER,
        &denoiser::DENOISER,
        &pitch_corrector::PITCH_CORRECTOR,
        &tuner::TUNER,
        &spectrum::SPECTRUM,
        &trance_gate::TRANCE_GATE,
        &shaper::SHAPER,
    ];
    &ALL
}

/// Other parameters a panel edit of `param` moves along in the same undo
/// step, and the undo label to use (e.g. Character Mod's circuit switch
/// carries per-circuit defaults). Empty for most devices.
pub fn linked_writes(def: &DeviceDef, device: &Device, param: &str, value: f32) -> (Vec<(&'static str, f32)>, Option<String>) {
    match def.id {
        "character-mod" => character_mod::switch_writes(def, device, param, value),
        "character-compressor" => (def.param(param).map(|p| character_compressor::linked(def, device, p, value)).unwrap_or_default(), None),
        // A choice's overlays (the mastering limiter's styles).
        _ => {
            let writes = def
                .choice_values(param, value)
                .into_iter()
                .filter(|(id, _)| id != param)
                .filter_map(|(id, v)| Some((def.param(&id)?.id, v)))
                .collect();
            (writes, None)
        }
    }
}

// ---------------------------------------------------------------- tempo sync

/// Note values of a synced LFO, slowest
/// first; the plain value of a Rate Sync parameter is the index.
pub const DIVISIONS: &[&str] = &[
    "8/1", "4/1", "2/1", "1/1", "1/2.", "1/2", "1/2T", "1/4.", "1/4", "1/4T", "1/8.", "1/8", "1/8T", "1/16.", "1/16", "1/16T", "1/32",
    "1/64.", "1/64", "1/64T",
];
/// Length of each of [DIVISIONS] in quarter notes.
pub const DIVISION_QUARTERS: &[f32] = &[
    32.0,
    16.0,
    8.0,
    4.0,
    3.0,
    2.0,
    4.0 / 3.0,
    1.5,
    1.0,
    2.0 / 3.0,
    0.75,
    0.5,
    1.0 / 3.0,
    0.375,
    0.25,
    1.0 / 6.0,
    0.125,
    0.093_75,
    0.0625,
    1.0 / 24.0,
];
/// Index of 1/4 in [DIVISIONS].
pub const DIVISION_QUARTER: u32 = 8;
/// Character Mod's note table, slowest first.
pub const MOD_NOTES: &[&str] = &[
    "2/1.", "2/1", "2/1T", "1/1.", "1/1", "1/1T", "1/2.", "1/2", "1/2T", "1/4.", "1/4", "1/4T", "1/8.", "1/8", "1/8T", "1/16.", "1/16",
    "1/16T", "1/32.", "1/32", "1/32T",
];
/// Length of each of [MOD_NOTES] in quarter notes.
pub const MOD_NOTE_QUARTERS: &[f32] = &[
    12.0,
    8.0,
    16.0 / 3.0,
    6.0,
    4.0,
    8.0 / 3.0,
    3.0,
    2.0,
    4.0 / 3.0,
    1.5,
    1.0,
    2.0 / 3.0,
    0.75,
    0.5,
    1.0 / 3.0,
    0.375,
    0.25,
    1.0 / 6.0,
    0.1875,
    0.125,
    1.0 / 12.0,
];
/// Free / Sync labels of a timebase picker.
pub const FREE_SYNC: &[&str] = &["Free", "Sync"];

/// LFO rate in Hz of a synced note value of `quarters` quarter notes at `bpm`.
pub fn synced_hz(quarters: f32, bpm: f32) -> f32 {
    let bpm = if bpm.is_finite() && bpm > 0.0 { bpm } else { 120.0 };
    bpm / 60.0 / quarters.max(1e-3)
}

/// Shown in the browser and the Add device lists. LFOs come from a knob's
/// Modulators menu or the device picker, chains from grouping or Chains.
pub fn listed(def: &DeviceDef) -> bool {
    def.id != crate::modulation::LFO_ID && def.id != crate::modulation::RACK_ID
}

pub fn find(id: &str) -> Option<&'static DeviceDef> {
    all().iter().copied().find(|d| d.id == id)
}

/// Definition of a device in the model, if it is a built-in.
pub fn def_of(d: &Device) -> Option<&'static DeviceDef> {
    if d.plugin.format == PluginFormat::Builtin { find(&d.plugin.id) } else { None }
}

impl DeviceDef {
    pub fn param(&self, id: &str) -> Option<&'static ParamDef> {
        self.params.iter().find(|p| p.id == id)
    }

    pub fn index_of(&self, id: &str) -> Option<usize> {
        self.params.iter().position(|p| p.id == id)
    }

    pub fn plugin_ref(&self) -> PluginRef {
        PluginRef {
            format: PluginFormat::Builtin,
            id: self.id.into(),
            path: None,
            name: self.name.into(),
            vendor: VENDOR.into(),
            version: String::new(),
            kind: self.kind,
        }
    }

    pub fn defaults(&self) -> BTreeMap<String, f32> {
        self.params.iter().map(|p| (p.id.to_string(), p.default)).collect()
    }

    /// A new instance with default values (id 0: allocated by `AddDevice`).
    pub fn instantiate(&self) -> Device {
        let mut d = Device::new(self.plugin_ref());
        d.params = self.defaults();
        d
    }

    /// Value of `param` on `device` (the default when the device doesn't store it).
    pub fn value(&self, device: &Device, param: &ParamDef) -> f32 {
        device.params.get(param.id).copied().map(|v| param.snap(v)).unwrap_or(param.default)
    }

    /// All values in engine order.
    pub fn values(&self, device: &Device) -> Vec<f32> {
        self.params.iter().map(|p| self.value(device, p)).collect()
    }

    /// Complete values of a factory preset (defaults filled in).
    pub fn factory_values(&self, preset: &FactoryPreset) -> BTreeMap<String, f32> {
        let mut v = self.defaults();
        for (id, x) in preset.values {
            if let Some(p) = self.param(id) {
                v.insert((*id).to_string(), p.snap(*x));
            }
        }
        v
    }

    /// Values a choice writes: the parameter itself plus its overlay for that
    /// choice (`ParamDef::overlays`), all snapped. Empty if `param` is unknown.
    pub fn choice_values(&self, param: &str, choice: f32) -> BTreeMap<String, f32> {
        let mut out = BTreeMap::new();
        let Some(p) = self.param(param) else { return out };
        let choice = p.snap(choice);
        out.insert(p.id.to_string(), choice);
        for (id, v) in p.overlays.get(choice.round() as usize).copied().unwrap_or(&[]) {
            if let Some(q) = self.param(id) {
                out.insert((*id).to_string(), q.snap(*v));
            }
        }
        out
    }

    /// The parameter that follows the sidechain picker (on while a source is picked).
    pub fn sidechain_enable(&self) -> Option<&'static ParamDef> {
        self.params.iter().find(|p| SIDECHAIN_ENABLE_IDS.contains(&p.id))
    }

    /// Display text of `param` = `v` on `device` (state-dependent text first).
    pub fn text(&self, device: &Device, p: &ParamDef, v: f32) -> String {
        behavior(self).and_then(|b| (b.text)(self, device, p, v)).unwrap_or_else(|| p.format(v))
    }

    /// Snap points of `param` in the device's current state.
    pub fn detents(&self, device: &Device, p: &ParamDef) -> &'static [f32] {
        behavior(self).map_or(&[], |b| (b.detents)(self, device, p))
    }

    /// Values written when `param` is set to `v`: the parameter itself plus
    /// its [`linked_writes`], all snapped.
    pub fn linked_values(&self, device: &Device, param: &str, v: f32) -> BTreeMap<String, f32> {
        let mut out = BTreeMap::new();
        let Some(p) = self.param(param) else { return out };
        let v = p.snap(v);
        for (id, x) in linked_writes(self, device, p.id, v).0 {
            if let Some(q) = self.param(id) {
                out.insert(id.to_string(), q.snap(x));
            }
        }
        out.insert(p.id.to_string(), v);
        out
    }

    /// Whether `param` is drawn inert (`Layout::inert_when`).
    pub fn is_inert(&self, device: &Device, param: &str) -> bool {
        self.layout
            .inert_when
            .iter()
            .filter(|(p, _, _)| *p == param)
            .any(|(_, by, values)| self.param(by).is_some_and(|b| values.contains(&(self.value(device, b).round() as u32))))
    }

    /// Panel label of a parameter in the device's current state.
    pub fn label_of(&self, device: &Device, p: &ParamDef) -> &'static str {
        behavior(self).and_then(|b| (b.label)(self, device, p)).unwrap_or(p.label)
    }

    /// Parse typed text for `p` in the device's current state into a snapped plain value.
    pub fn parse_of(&self, device: &Device, p: &ParamDef, text: &str) -> Option<f32> {
        let read = behavior(self).and_then(|b| (b.read)(self, device, p, &text.trim().to_lowercase().replace('\u{2212}', "-")));
        read.filter(|v| v.is_finite()).map(|v| p.snap(v)).or_else(|| p.parse(text))
    }

    /// Values of a preset as loaded onto `device`: `reset_on_load`
    /// parameters take their default, and while a lock parameter is on, it and
    /// the parameters it locks keep their current values.
    pub fn preset_load_values(&self, device: &Device, preset: &DevicePreset) -> BTreeMap<String, f32> {
        let mut v = preset.load_values(self);
        for lock in self.params.iter().filter(|p| !p.locks.is_empty() && self.value(device, p) >= 0.5) {
            for id in lock.locks.iter().copied().chain(std::iter::once(lock.id)) {
                if let Some(p) = self.param(id) {
                    v.insert(p.id.into(), self.value(device, p));
                }
            }
        }
        v
    }

    /// Whether `param` is visible in the device's current state (`Layout::shown_when`).
    pub fn is_shown(&self, device: &Device, param: &str) -> bool {
        self.layout
            .shown_when
            .iter()
            .filter(|(p, _, _)| *p == param)
            .all(|(_, by, values)| self.param(by).map(|b| values.contains(&(self.value(device, b).round() as u32))).unwrap_or(true))
    }
}

// ---------------------------------------------------------------- key follow

/// Key and scale (plain choice values) a device shows and runs with while it
/// follows the project key: devices with `key_follow` (Manual / Project),
/// `key` (pitch classes from C) and `scale` parameters. None while Key
/// Follow is Manual, or when the project has no key (the stored values
/// stay in force). A minor project key picks the scale choice "Minor",
/// otherwise "Major".
pub fn followed_key(def: &DeviceDef, device: &Device, key: Option<crate::model::ProjectKey>) -> Option<(f32, f32)> {
    let key = key?;
    let follow = def.param("key_follow")?;
    if def.value(device, follow) < 0.5 {
        return None;
    }
    let scale = def.param("scale")?;
    let want = if key.minor { "Minor" } else { "Major" };
    let s = scale.choices.iter().position(|c| *c == want)?;
    def.param("key")?;
    Some((f32::from(key.root.min(11)), s as f32))
}

/// The device as the engine should run it: a copy with the followed project
/// key written into `key` / `scale` (see [followed_key]); other devices
/// unchanged.
pub fn engine_device(device: &Device, key: Option<crate::model::ProjectKey>) -> Device {
    let mut d = device.clone();
    if let Some(def) = def_of(device)
        && let Some((k, s)) = followed_key(def, device, key)
    {
        d.params.insert("key".into(), k);
        d.params.insert("scale".into(), s);
    }
    d
}

/// Check one parameter edit against the definition: the parameter exists and
/// the value is finite and inside the range. Returns the snapped value.
pub fn validate_value(def: &DeviceDef, param: &str, v: f32) -> Result<f32, String> {
    let p = def.param(param).ok_or_else(|| format!("{} has no parameter {param:?}", def.name))?;
    let tol = 1e-4 * (p.max - p.min);
    if !v.is_finite() || v < p.min - tol || v > p.max + tol {
        return Err(format!("{} {v} outside {}..={}", p.name, p.min, p.max));
    }
    Ok(p.snap(v))
}

// ---------------------------------------------------------------- presets

/// A saved device preset (user presets are JSON files written by the app).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DevicePreset {
    /// Definition id the preset belongs to.
    pub device: String,
    pub name: String,
    pub values: BTreeMap<String, f32>,
}

impl DevicePreset {
    /// Capture a device's current values.
    pub fn capture(def: &DeviceDef, device: &Device, name: &str) -> DevicePreset {
        DevicePreset {
            device: def.id.into(),
            name: name.trim().into(),
            values: def.params.iter().map(|p| (p.id.to_string(), def.value(device, p))).collect(),
        }
    }

    pub fn factory(def: &DeviceDef, preset: &FactoryPreset) -> DevicePreset {
        DevicePreset { device: def.id.into(), name: preset.name.into(), values: def.factory_values(preset) }
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).expect("preset serialises")
    }

    /// Parse a preset file for `def`: unknown ids are dropped, values are
    /// snapped into range, missing ids take the default.
    pub fn from_json(def: &DeviceDef, text: &str) -> Result<DevicePreset, String> {
        let raw: DevicePreset = serde_json::from_str(text).map_err(|e| e.to_string())?;
        if raw.device != def.id {
            return Err(format!("preset is for {:?}, not {:?}", raw.device, def.id));
        }
        let mut values = def.defaults();
        for (k, v) in raw.values {
            if let Some(p) = def.param(&k) {
                values.insert(k, p.snap(v));
            }
        }
        Ok(DevicePreset { device: raw.device, name: raw.name, values })
    }

    /// Values to apply when loading: `reset_on_load` parameters go back to
    /// their default; `preset_lock` parameters are left out (they keep the
    /// device's current value).
    pub fn load_values(&self, def: &DeviceDef) -> BTreeMap<String, f32> {
        let mut v = self.values.clone();
        for p in def.params {
            if p.reset_on_load {
                v.insert(p.id.into(), p.default);
            } else if p.preset_lock {
                v.remove(p.id);
            }
        }
        v
    }
}

// ---------------------------------------------------------------- modulation

/// A parameter an LFO (or macro) can modulate.
#[derive(Debug, Clone, PartialEq)]
pub struct ModTarget {
    pub device: DeviceId,
    pub device_name: String,
    pub param: &'static str,
    pub param_name: &'static str,
}

/// LFO / macro targets on a track: every automatable parameter of its
/// built-in audio effects (audio-effect parameters only, not
/// instruments' or the LFO's own).
pub fn mod_targets(track: &Track) -> Vec<ModTarget> {
    track
        .devices
        .iter()
        .filter_map(|d| def_of(d).map(|def| (d, def)))
        .filter(|(_, def)| def.kind == PluginKind::Effect && listed(def))
        .flat_map(|(d, def)| {
            def.params.iter().filter(|p| p.automatable).map(move |p| ModTarget {
                device: d.id,
                device_name: def.name.into(),
                param: p.id,
                param_name: p.name,
            })
        })
        .collect()
}

/// Apply a modulation offset (normalized units, -1..1) to a plain value:
/// in the normalized domain, clamped.
pub fn modulate(p: &ParamDef, plain: f32, offset: f32) -> f32 {
    p.from_normalized((p.to_normalized(plain) + offset).clamp(0.0, 1.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slope_choices_parse_with_or_without_the_unit() {
        let ce = find("character-eq").unwrap().param("band1_slope").unwrap();
        assert_eq!(ce.parse("24 dB/oct"), Some(3.0));
        assert_eq!(ce.parse("24"), Some(3.0));
        let ch = find("channeleq").unwrap().param("hp_slope").unwrap();
        assert_eq!(ch.parse("24"), Some(1.0));
        assert_eq!(ch.parse("36 dB/oct"), Some(2.0));
    }

    fn comp() -> &'static DeviceDef {
        find("compressor").unwrap()
    }

    #[test]
    fn every_definition_is_consistent() {
        for def in all() {
            let mut ids = std::collections::HashSet::new();
            for p in def.params {
                assert!(ids.insert(p.id), "{}: duplicate param id {}", def.id, p.id);
                assert!(p.min < p.max && p.default >= p.min && p.default <= p.max, "{}.{} range/default", def.id, p.id);
                assert_eq!(p.snap(p.default), p.default, "{}.{} default must be a valid value", def.id, p.id);
                if p.taper == Taper::Log {
                    assert!(p.min > 0.0, "{}.{} log needs min > 0", def.id, p.id);
                }
                if p.is_discrete() {
                    assert_eq!(p.max as usize + 1, p.choices.len());
                }
            }
            for c in def.layout.controls() {
                for id in c.params() {
                    assert!(def.param(id).is_some(), "{}: layout names unknown param {id}", def.id);
                }
            }
            if let Some(View::Field(f)) = def.layout.view {
                for id in f.x.iter().chain(f.y) {
                    assert!(def.param(id).is_some(), "{}: field names unknown param {id}", def.id);
                }
            }
            if let Some(View::Eq(v)) = def.layout.view {
                for id in v.param_ids() {
                    assert!(def.param(id).is_some(), "{}: EQ view names unknown param {id}", def.id);
                }
            }
            for (p, by, values) in def.layout.shown_when {
                let b = def.param(by).unwrap_or_else(|| panic!("{}: shown_when names unknown param {by}", def.id));
                assert!(def.param(p).is_some() && values.iter().all(|v| (*v as f32) <= b.max), "{}: shown_when {p}", def.id);
            }
            for f in def.factory {
                for (id, v) in f.values {
                    let p = def.param(id).unwrap_or_else(|| panic!("{} preset {}: unknown {id}", def.id, f.name));
                    assert!(*v >= p.min && *v <= p.max, "{} preset {}: {id} out of range", def.id, f.name);
                }
            }
            for p in def.params {
                assert!(p.overlays.is_empty() || p.overlays.len() == p.choices.len(), "{}.{}: one overlay per choice", def.id, p.id);
                for (id, v) in p.overlays.iter().flat_map(|o| o.iter()) {
                    let q = def.param(id).unwrap_or_else(|| panic!("{}.{} overlay names unknown {id}", def.id, p.id));
                    assert_eq!(q.snap(*v), *v, "{}.{} overlay {id}={v} must be a valid value", def.id, p.id);
                }
            }
            assert!(def.layout.width() > 0.0);
            if let Some(s) = def.layout.view.and_then(|v| v.strip()) {
                for line in std::iter::once(s.line).chain(s.second) {
                    let p = def.param(line).unwrap_or_else(|| panic!("{}: strip line {line} unknown", def.id));
                    assert!(p.max <= 0.0, "{}: strip lines are capped at 0 dB", def.id);
                }
                assert!(s.top_db > s.floor_db);
            }
        }
    }

    #[test]
    fn normalisation_round_trips() {
        let c = comp();
        for p in c.params {
            for i in 0..=20 {
                let n = i as f32 / 20.0;
                let v = p.from_normalized(n);
                assert!(v >= p.min && v <= p.max);
                if !p.is_discrete() && p.step == 0.0 {
                    assert!((p.to_normalized(v) - n).abs() < 1e-4, "{} n={n}", p.id);
                }
            }
        }
        let attack = c.param("attack").unwrap();
        assert!((attack.from_normalized(0.5) - 1000f32.sqrt()).abs() < 1e-3, "log midpoint is the geometric mean");
        let style = c.param("style").unwrap();
        assert_eq!(style.from_normalized(0.34), 1.0);
        assert_eq!(style.from_normalized(0.9), 3.0);
    }

    #[test]
    fn formatting_and_parsing() {
        let c = comp();
        let th = c.param("threshold").unwrap();
        assert_eq!(th.format(-24.0), "\u{2212}24.0 dB");
        assert_eq!(th.format(0.0), "+0.0 dB");
        assert_eq!(th.parse("-12"), Some(-12.0));
        assert_eq!(th.parse("\u{2212}6.5 dB"), Some(-6.5));
        assert_eq!(th.parse("-100"), Some(-60.0), "clamped");
        assert_eq!(th.parse("loud"), None);
        let rel = c.param("release").unwrap();
        assert_eq!(rel.format(100.0), "100 ms");
        assert_eq!(rel.format(1500.0), "1500 ms");
        assert_eq!(c.param("attack").unwrap().format(10.0), "10 ms");
        assert_eq!(rel.parse("1.2 s"), Some(1200.0));
        assert_eq!(rel.parse("250ms"), Some(250.0));
        assert_eq!(c.param("ratio").unwrap().format(4.0), "4.0:1");
        assert_eq!(c.param("ratio").unwrap().parse("8:1"), Some(8.0));
        assert_eq!(c.param("mix").unwrap().format(100.0), "100%");
        let style = c.param("style").unwrap();
        assert_eq!(style.format(1.0), "Opto");
        assert_eq!(style.parse("punch"), Some(2.0));
    }

    #[test]
    fn presets_round_trip_and_sanitise() {
        let c = comp();
        let mut d = c.instantiate();
        d.params.insert("threshold".into(), -30.0);
        let p = DevicePreset::capture(c, &d, "Mine");
        assert_eq!(DevicePreset::from_json(c, &p.to_json()).unwrap(), p);
        let odd = r#"{"device":"compressor","name":"x","values":{"threshold":-500,"bogus":1}}"#;
        let q = DevicePreset::from_json(c, odd).unwrap();
        assert_eq!(q.values["threshold"], -60.0);
        assert!(!q.values.contains_key("bogus"));
        assert_eq!(q.values.len(), c.params.len());
        assert!(DevicePreset::from_json(c, r#"{"device":"gate","name":"x","values":{}}"#).is_err());
    }

    #[test]
    fn modulation_formats_follow_the_reference() {
        let rate = find("tremolo").unwrap().param("rate").unwrap();
        assert_eq!(rate.format(0.5), "0.50 Hz");
        assert_eq!(rate.format(5.0), "5.0 Hz");
        assert_eq!(rate.format(12.4), "12 Hz");
        let f = find("ring-mod").unwrap().param("frequency").unwrap();
        assert_eq!(f.format(440.0), "440 Hz");
        assert_eq!(f.format(2199.99), "2.2 kHz");
        assert_eq!(f.parse("2.5k"), Some(2500.0));
        assert_eq!(f.parse("80 Hz"), Some(80.0));
        let rect = find("ring-mod").unwrap().param("rectify").unwrap();
        assert_eq!(rect.format(0.0), "0%");
        assert_eq!(rect.format(35.0), "+35%");
        assert_eq!(rect.format(-100.0), "−100%");
        assert_eq!(rect.parse("+35%"), Some(35.0));
        assert_eq!(rect.parse("−20"), Some(-20.0));
        let fb = find("flanger").unwrap().param("feedback").unwrap();
        assert_eq!(fb.format(-45.0), "−45%");
        assert_eq!(fb.format(60.0), "60%");
        let st = find("tremolo").unwrap().param("stereo_phase").unwrap();
        assert_eq!(st.format(90.0), "90 deg");
        let sync = find("chorus").unwrap().param("rate_sync").unwrap();
        assert_eq!(sync.format(8.0), "1/4");
        assert_eq!(sync.parse("1/8T"), Some(12.0));
        assert_eq!(find("flanger").unwrap().param("depth").unwrap().format(3.0), "3.0 ms");
    }

    #[test]
    fn tempo_tables_line_up() {
        assert_eq!(DIVISIONS.len(), DIVISION_QUARTERS.len());
        assert_eq!(MOD_NOTES.len(), MOD_NOTE_QUARTERS.len());
        assert_eq!(DIVISIONS[DIVISION_QUARTER as usize], "1/4");
        assert!((synced_hz(DIVISION_QUARTERS[DIVISION_QUARTER as usize], 120.0) - 2.0).abs() < 1e-6);
        assert!((synced_hz(MOD_NOTE_QUARTERS[10], 0.0) - 2.0).abs() < 1e-6, "bpm falls back to 120");
    }

    #[test]
    fn preset_lock_and_reset_on_load() {
        let def = find("character-mod").unwrap();
        let mut d = def.instantiate();
        d.params.insert("seed".into(), 77.0);
        d.params.insert("mono_audition".into(), 1.0);
        let p = DevicePreset::capture(def, &d, "x");
        let load = p.load_values(def);
        assert!(!load.contains_key("seed"), "seed is preset-locked: the device keeps its own");
        assert_eq!(load["mono_audition"], 0.0, "audition switches reset on load");
        assert_eq!(load["model"], 2.0);
    }

    #[test]
    fn field_cards_size_to_the_view_row() {
        let w = |id: &str| find(id).unwrap().layout.width();
        assert_eq!(w("tremolo"), 457.0);
        assert_eq!(w("autopanner"), 417.0);
        assert_eq!(w("chorus"), 377.0);
        assert_eq!(w("flanger"), 388.0);
        assert_eq!(w("phaser"), 377.0);
        assert_eq!(w("ring-mod"), 390.0);
        assert_eq!(w("character-mod"), 442.0);
    }

    #[test]
    fn width_follows_clusters() {
        assert_eq!(comp().layout.width(), 521.0, "reference: [2,5] clusters -> 505 + 16");
    }

    #[test]
    fn dynamics_formats() {
        let gate = find("gate").unwrap();
        assert_eq!(gate.param("ratio").unwrap().format(10.0), "10:1", "ranges of 20 or more print whole ratios");
        assert_eq!(gate.param("attack").unwrap().format(0.5), "0.50 ms");
        assert_eq!(gate.param("release").unwrap().format(100.0), "100.0 ms");
        assert_eq!(gate.param("hold").unwrap().format(50.0), "50 ms");
        assert_eq!(gate.param("attack").unwrap().parse("0.02 s"), Some(20.0));
        assert_eq!(gate.param("sidechain_hpf").unwrap().format(80.0), "80 Hz");
        let ts = find("transient-shaper").unwrap();
        let attack = ts.param("attack").unwrap();
        assert_eq!(attack.format(35.0), "+35%");
        assert_eq!(attack.format(-20.0), "\u{2212}20%");
        assert_eq!(attack.format(0.2), "0%");
        assert_eq!(attack.parse("-40%"), Some(-40.0));
        let sus = ts.param("sustain_time").unwrap();
        assert_eq!(sus.format(2000.0), "Full");
        assert_eq!(sus.format(sus.from_normalized(0.9999)), "Full");
        assert_eq!(sus.format(150.0), "150 ms");
        assert_eq!(sus.parse("full"), Some(2000.0));
        assert_eq!(ts.param("sensitivity").unwrap().label, "Sense");
        let ml = find("mastering-limiter").unwrap();
        assert_eq!(ml.param("threshold").unwrap().format(0.0), "0.0 dB");
        assert_eq!(ml.param("ceiling").unwrap().format(-1.0), "\u{2212}1.0 dB");
        assert_eq!(ml.param("release").unwrap().format(150.0), "150 ms");
        assert!((ml.param("release").unwrap().to_normalized(150.0) - 0.6223).abs() < 1e-3, "skew 0.25");
        assert!((ml.param("attack").unwrap().to_normalized(10.0) - 0.5012).abs() < 1e-3, "skew 0.3");
        let glue = find("glue-compressor").unwrap();
        assert_eq!(glue.param("release").unwrap().format(4.0), "Auto");
        assert_eq!(glue.param("range").unwrap().format(24.0), "24.0 dB");
        assert_eq!(glue.param("ratio").unwrap().number(2.0), 10.0);
        assert_eq!(glue.param("attack").unwrap().number(1.0), 0.3);
        assert!(glue.param("release").unwrap().number(4.0).is_nan());
        assert_eq!(glue.param("ratio").unwrap().parse("10:1"), Some(2.0));
    }

    #[test]
    fn choice_overlays_write_their_values() {
        let ml = find("mastering-limiter").unwrap();
        let v = ml.choice_values("style", 3.0);
        assert_eq!(v["style"], 3.0);
        assert_eq!(v["lookahead"], 3.0);
        assert_eq!(v["attack"], 30.0);
        assert_eq!(v["auto_release"], 0.0);
        assert_eq!(v.len(), 7);
        let comp = comp();
        assert_eq!(comp.choice_values("style", 2.0).len(), 1, "no overlays: only the choice");
        assert_eq!(find("glue-compressor").unwrap().sidechain_enable().map(|p| p.id), Some("sidechain"));
        assert_eq!(comp.sidechain_enable().map(|p| p.id), Some("sidechainEnable"));
    }

    #[test]
    fn dynamics_card_widths_follow_the_reference() {
        for (id, w) in
            [("limiter", 471.0), ("gate", 543.0), ("glue-compressor", 615.0), ("transient-shaper", 543.0), ("mastering-limiter", 388.0)]
        {
            assert_eq!(find(id).unwrap().layout.width(), w, "{id}");
        }
    }

    #[test]
    fn restoration_formats_match_the_reference_cards() {
        let deesser = find("deesser").unwrap();
        assert_eq!(deesser.param("frequency").unwrap().format(7000.0), "7.00 kHz");
        assert_eq!(deesser.param("bandwidth").unwrap().format(1.0), "1.00");
        let red = deesser.param("reduction").unwrap();
        assert_eq!(red.format(8.0), "\u{2212}8.0 dB");
        assert_eq!(red.format(0.0), "0.0 dB");
        assert_eq!(red.parse("-12 dB"), Some(12.0));
        assert_eq!(red.parse("6"), Some(6.0));
        assert_eq!(red.parse("\u{2212}40"), Some(30.0), "clamped");
        assert_eq!(deesser.param("listen").unwrap().format(0.0), "Off");
        assert!(deesser.param("listen").unwrap().reset_on_load);
        let hum = find("dehummer").unwrap();
        assert_eq!(hum.param("frequency").unwrap().format(60.0), "60.0 Hz");
        assert_eq!(hum.param("num_harmonics").unwrap().format(4.0), "4");
        assert_eq!(hum.param("num_harmonics").unwrap().snap(4.4), 4.0);
        assert_eq!(hum.param("notch_width").unwrap().format(5.0), "5.0");
        assert_eq!(hum.param("depth").unwrap().format(-30.0), "\u{2212}30.0 dB");
        let noise = find("denoiser").unwrap();
        assert_eq!(noise.param("frequency_smoothing").unwrap().label, "Smoothing");
        assert_eq!(noise.param("attack").unwrap().format(5.0), "5.0 ms");
        assert_eq!(noise.param("release").unwrap().format(50.0), "50 ms");
        assert_eq!(noise.param("attack").unwrap().parse("12 ms"), Some(12.0));
        let clip = find("declipper").unwrap();
        assert_eq!(clip.param("oversampling").unwrap().format(1.0), "4x");
        assert_eq!(clip.param("threshold").unwrap().format(-1.0), "\u{2212}1.0 dB");
    }

    #[test]
    fn restoration_defaults_match_the_reference_normalised_values() {
        // Normalised defaults.
        let cases: &[(&str, &str, f32)] = &[
            ("deesser", "frequency", 0.602_451_6),
            ("deesser", "bandwidth", 0.230_769_2),
            ("deesser", "threshold", 0.1),
            ("deesser", "reduction", 0.266_666_7),
            ("dehummer", "frequency", 0.666_666_7),
            ("dehummer", "num_harmonics", 0.428_571_4),
            ("dehummer", "notch_width", 0.473_684_2),
            ("dehummer", "depth", 0.5),
            ("denoiser", "threshold", 0.5),
            ("denoiser", "reduction", 0.666_666_7),
            ("denoiser", "attack", 0.629_488_3),
            ("denoiser", "release", 0.629_488_3),
            ("declipper", "threshold", 0.833_333_3),
            ("declipper", "ceiling", 0.95),
        ];
        for (dev, id, n) in cases {
            let p = find(dev).unwrap().param(id).unwrap();
            assert!((p.to_normalized(p.default) - n).abs() < 1e-5, "{dev}.{id}: {} vs {n}", p.to_normalized(p.default));
        }
    }

    #[test]
    fn restoration_card_widths() {
        assert_eq!(find("declipper").unwrap().layout.width(), 244.0);
        assert_eq!(find("deesser").unwrap().layout.width(), 471.0);
        assert_eq!(find("dehummer").unwrap().layout.width(), 388.0);
        assert_eq!(find("denoiser").unwrap().layout.width(), 460.0);
    }

    #[test]
    fn modulation_is_normalized_and_clamped() {
        let p = comp().param("attack").unwrap();
        assert_eq!(modulate(p, 1000.0, 0.5), 1000.0);
        assert!((modulate(p, 1.0, 0.5) - 1000f32.sqrt()).abs() < 1e-2);
    }

    #[test]
    fn registry_lists_the_drive_and_imaging_devices() {
        let ids: Vec<_> = all().iter().map(|d| d.id).collect();
        assert_eq!(ids[..7], ["compressor", "distortion", "tapesaturation", "soft-clipper", "bitcrusher", "widener", "utility"]);
        assert_eq!(find("widener").unwrap().layout.width(), 388.0);
        assert_eq!(find("utility").unwrap().layout.width(), 221.5);
        // Modulation sources (LFO, chain head) are not listed effects and show no presets;
        // Tuner and Spectrum ship none.
        for def in all().iter().filter(|d| listed(d) && !matches!(d.id, "tuner" | "spectrum")) {
            assert!(!def.factory.is_empty(), "{} has factory presets", def.id);
        }
    }

    #[test]
    fn new_formats_print_and_parse() {
        let u = find("utility").unwrap();
        let bal = u.param("balance").unwrap();
        assert_eq!(bal.format(0.0), "C");
        assert_eq!(bal.format(-25.0), "25L");
        assert_eq!(bal.format(25.0), "25R");
        assert_eq!(bal.parse("25L"), Some(-25.0));
        assert_eq!(bal.parse("10 r"), Some(10.0));
        assert_eq!(bal.parse("c"), Some(0.0));
        assert_eq!(u.param("width").unwrap().format(100.0), "100%");
        let f = u.param("bass_mono_freq").unwrap();
        assert_eq!(f.format(120.0), "120 Hz");
        assert_eq!(f.parse("200 Hz"), Some(200.0));
        let inf: Vec<_> = all().iter().flat_map(|d| d.params.iter()).filter(|p| p.format == Format::DbOrInf).collect();
        assert!(!inf.is_empty());
        for p in inf {
            assert!(p.format(p.min).contains('\u{221e}'), "{} at min shows -inf", p.id);
            assert_eq!(p.parse("-inf"), Some(p.min));
        }
    }

    #[test]
    fn drawn_models_behave() {
        use super::{distortion::distortion_transfer, soft_clipper::clipper_transfer, widener::widener_response};
        for mode in 0..4 {
            assert!(
                distortion_transfer(0.0, 50.0, 40.0, 0.0, 100.0, 0.0, mode).abs() < 1e-6,
                "mode {mode}: silence stays silent (bias has no DC)"
            );
            let (a, b) =
                (distortion_transfer(0.2, 50.0, 0.0, 0.0, 100.0, 0.0, mode), distortion_transfer(0.4, 50.0, 0.0, 0.0, 100.0, 0.0, mode));
            assert!(b >= a, "mode {mode} is monotonic");
        }
        assert!((distortion_transfer(0.3, 80.0, 0.0, 0.0, 0.0, 0.0, 0) - 0.3).abs() < 1e-6, "mix 0 is dry");
        for shape in 0..3 {
            for s in [0.0, 50.0, 100.0] {
                let y = clipper_transfer(4.0, 0.0, -6.0, s, shape, false, 100.0, 0.0);
                assert!(y <= 10f32.powf(-6.0 / 20.0) + 1e-4, "shape {shape} softness {s}: held under the ceiling ({y})");
            }
            assert!(
                (clipper_transfer(0.01, 0.0, 0.0, 0.0, shape, false, 100.0, 0.0) - 0.01).abs() < 1e-3,
                "transparent well below the ceiling"
            );
        }
        assert!(distortion_transfer(1.0, 100.0, 100.0, 0.0, 100.0, 0.0, 2).abs() < 2.0, "Tube stays bounded at full drive and bias");
        for shape in 0..3 {
            let y = clipper_transfer(1.0, 24.0, -12.0, 0.0, shape, false, 100.0, 0.0);
            assert!(y > 0.2 && y <= 10f32.powf(-12.0 / 20.0) + 1e-4, "shape {shape}: extreme drive keeps the ceiling ({y})");
        }
        let pic = super::bitcrusher::crush_picture(32.0, 2.5, 100.0, 0.0, 0.0);
        let changes = pic.windows(2).filter(|w| w[0].1 != w[1].1).count();
        assert!((changes as f32 - 480.0 / 2.5).abs() <= 2.0, "fractional downsample keeps its rate ({changes})");
        let sr = 48_000.0;
        let half = widener_response(120.0, 100.0, 0.0, 0.0, 120.0, 50.0, sr);
        assert!(half > 70.0 && half < 82.0, "partial mix sums the paths with their phase ({half})");
        assert!((widener_response(5000.0, 100.0, 0.0, 0.0, 20.0, 100.0, sr) - 100.0).abs() < 1.0, "100% is unchanged above the cut");
        assert!(
            widener_response(30.0, 200.0, 0.0, 0.0, 300.0, 100.0, sr) < widener_response(5000.0, 200.0, 0.0, 0.0, 300.0, 100.0, sr),
            "the low cut narrows the lows"
        );
        assert!((widener_response(5000.0, 200.0, 0.0, 0.0, 20.0, 0.0, sr) - 100.0).abs() < 1e-3, "mix 0 is dry");
    }
}
