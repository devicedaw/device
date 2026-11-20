//! Built-in "Synth" instrument panel (bottom panel, Devices tab, instrument
//! tracks): the synth-v2 device (dstudio_core::builtin::synth_v2) with its nine
//! pages, the modulation sources drawer, the modulation matrix, macros and the
//! four effect slots.
//!
//! Every control is a parameter of the track's Synth device, addressed by its
//! index in the definition (Studio.synth-params is indexed the same way).
//! Continuous drags send live values to the engine (se_builtin_set_param, no
//! model change) and commit ONE Command::SetDeviceParam on release (a
//! Batch of them when a gesture moves several parameters); switches, resets,
//! presets and route / mapping edits are single commands, so every gesture is
//! one undo step.
//!
//! This file lays the pages out (ui/synth.slint only draws element kinds) and
//! computes the pictures (oscillator waves, filter response, envelopes, LFOs,
//! glide, unison field, effect curves) as SVG paths from the current values.
//! They are drawing models of our own DSP (engine/src/devices/synth_v2.cpp),
//! not measurements.

use crate::controller::{BottomTab, Controller, card_color, sync_model};
use crate::{
    DeviceCard, MenuItem, Studio, SynthArc, SynthDisplay, SynthFaderItem, SynthKnobItem, SynthLabel, SynthLatchItem, SynthMapRow,
    SynthParam, SynthPillItem, SynthRouteRow, SynthScopeItem, SynthSegItem, SynthTextItem,
};
use dstudio_core::builtin::synth_v2::{self as sv, SYNTH_V2};
use dstudio_core::builtin::{DevicePreset, ParamDef};
use dstudio_core::{AutomationTarget, Command, DeviceId, TrackId, TrackKind};
use slint::{Color, ModelRc, VecModel};
use std::collections::{BTreeMap, HashMap};
use std::f32::consts::PI;
use std::rc::Rc;
use std::sync::OnceLock;

/// Category list (left column), in order.
pub const PAGES: [&str; 9] = ["Oscillators", "Sub & Noise", "Filter", "Envelopes", "LFOs", "Modulation", "Macros", "Voice", "Effects"];

pub mod page {
    pub const OSC: usize = 0;
    pub const SUB_NOISE: usize = 1;
    pub const FILTER: usize = 2;
    pub const ENV: usize = 3;
    pub const LFO: usize = 4;
    pub const MODULATION: usize = 5;
    pub const MACROS: usize = 6;
    pub const VOICE: usize = 7;
    pub const EFFECTS: usize = 8;
}

/// Picture slots (Studio.synth-displays).
pub mod disp {
    pub const OSC1: usize = 0;
    pub const OSC2: usize = 1;
    pub const SUB: usize = 2;
    pub const NOISE: usize = 3;
    pub const FILTER: usize = 4;
    pub const FILTER_MOD: usize = 5;
    pub const AMP_ENV: usize = 6;
    pub const ENV2: usize = 7;
    pub const LFO1: usize = 8;
    pub const LFO2: usize = 9;
    pub const GLIDE: usize = 10;
    pub const UNISON: usize = 11;
    pub const FX_CURVE: usize = 12;
    pub const FX_WAVE: usize = 13;
    pub const COUNT: usize = 14;
}

/// Picture sizes in points (path viewbox = element size). The card is drawn
/// at a scale of 0.844.
pub const DISP_W: f32 = 245.0;
pub const WIDE_W: f32 = 295.5;
pub const DISP_H: f32 = 128.0;
pub const FX_CURVE_W: f32 = 136.0;
pub const FX_GAP: f32 = 4.0;
pub const FX_WAVE_W: f32 = 418.0;

/// Module origins, the separator and the rows of the two-module pages.
const M1: f32 = 117.5;
const M2: f32 = 434.5;
const SPLIT: f32 = 424.5;
const HEAD_Y: f32 = 32.0;
const PIC_Y: f32 = 57.5;
const KNOB_Y: f32 = 212.0;
const FADER_DX: f32 = 273.3;
const FADER_Y: f32 = 61.5;

/// Dial columns: four per module (75.5 pt apart) or five (60 pt apart).
fn col4(m: f32, i: usize) -> f32 {
    m + 35.0 + 75.5 * i as f32
}
fn col5(m: f32, i: usize) -> f32 {
    m + 27.5 + 60.0 * i as f32
}

/// Mod drawer colours by source index (None first).
pub const SOURCE_COLORS: [u32; 10] = [0x6f7075, 0xa271b3, 0xc9708f, 0xd9c96a, 0x5fb5a3, 0xd8875a, 0x9cbf63, 0x6b98d8, 0x7fb0d0, 0xc8c8cc];
/// Macro colour (knob values, mapping ranges, the selection ring).
pub const MACRO_COLOR: u32 = 0xe9799a;

pub const MENU_ROW: f32 = 20.5;
pub const MENU_PAD: f32 = 4.0;

/// Parameter index by id (the definition is static).
pub fn ix(id: &str) -> usize {
    static MAP: OnceLock<HashMap<&'static str, usize>> = OnceLock::new();
    MAP.get_or_init(|| SYNTH_V2.params.iter().enumerate().map(|(i, p)| (p.id, i)).collect())
        .get(id)
        .copied()
        .unwrap_or_else(|| panic!("synth-v2 has no parameter {id}"))
}

fn pdef(i: usize) -> &'static ParamDef {
    &SYNTH_V2.params[i]
}

fn rgb(c: u32) -> Color {
    Color::from_rgb_u8((c >> 16) as u8, (c >> 8) as u8, c as u8)
}

/// What an open popup list edits.
#[derive(Debug, Clone, PartialEq)]
pub enum MenuTarget {
    Presets,
    /// The choices of a parameter.
    Choice(usize),
    /// Env 2 / LFO n "Targets" (source index).
    Targets(usize),
    /// Free / Sync of a sync parameter.
    Timebase(usize),
    AddMapping,
    MacroName(usize),
    /// Right click on a control: macros and modulation for one parameter.
    Knob(usize),
    RouteSource(usize),
    RouteTarget(usize),
    AddRoute,
    FxType(usize),
}

#[derive(Debug, Clone)]
pub struct SynthMenu {
    pub target: MenuTarget,
    pub pos: (f32, f32),
    pub w: f32,
    /// (label, check / hint, action; None = group heading) rows.
    rows: Vec<(String, String, Option<MenuAction>)>,
}

#[derive(Debug, Clone, PartialEq)]
enum MenuAction {
    Preset(usize),
    Set(Vec<(usize, f32)>),
    Page(usize),
    /// Show (or create) the automation lane of this parameter.
    Automate(usize),
}

/// Values heard by the engine during a gesture, not yet a command:
/// (parameter, value before the gesture, current value).
#[derive(Debug, Clone)]
struct Live {
    device: DeviceId,
    vals: Vec<(usize, f32, f32)>,
}

/// A drag in a picture: the handle and the values it started from.
#[derive(Debug, Clone)]
struct ScopeDrag {
    slot: usize,
    handle: usize,
    start: (f32, f32),
    norms: Vec<(usize, f32)>,
}

/// A source chip pressed in the drawer: start point, pointer, dragging.
#[derive(Debug, Clone, Copy)]
struct RailDrag {
    src: usize,
    start: (f32, f32),
    pos: (f32, f32),
    dragging: bool,
}

/// First live-modulation readout of the synth (one per modulation target).
const RO_MOD: usize = 20;
/// Source value readouts follow the per-target ones: Env 2 through Random.
const SOURCES: usize = 9;

#[derive(Debug, Default)]
pub struct SynthUi {
    pub page: usize,
    pub menu: Option<SynthMenu>,
    /// Effects page tab (1..=4).
    pub fx_slot: usize,
    pub macro_sel: usize,
    /// Lit source (its targets get a ring), 0 = none.
    pub lit: usize,
    live: Option<Live>,
    scope: Option<ScopeDrag>,
    rail: Option<RailDrag>,
    /// Dial centres of the page last laid out (drop targets for the drawer).
    knob_pos: Vec<(usize, f32, f32)>,
    /// What the page models were last built from.
    pushed: Option<(Vec<f32>, [usize; 4], u32, Option<String>, u32)>,
    models: Option<Models>,
    /// Live modulation per target (engine readouts), empty at rest.
    mods: Vec<f32>,
    /// Each source's live value (engine readouts, Env 2 first), empty at rest.
    srcs: Vec<f32>,
    /// What the pictures were last drawn from (values with live modulation, page, slot, bpm).
    drawn: Option<(Vec<f32>, usize, usize, u32)>,
}

impl SynthUi {
    /// Forget a gesture in progress (undo, redo, track change).
    pub fn drop_gesture(&mut self) {
        self.live = None;
        self.scope = None;
        self.rail = None;
    }
}

// ------------------------------------------------------------------ modulation facts

/// Routes in use: (slot 1..=32, source, target, amount, on).
pub fn routes(v: &[f32]) -> Vec<(usize, usize, usize, f32, bool)> {
    (1..=sv::MOD_SLOTS)
        .filter_map(|n| {
            let b = ix("mod1_source") + 4 * (n - 1);
            let (s, t) = (v[b].round() as usize, v[b + 1].round() as usize);
            (s != 0 || t != 0).then_some((n, s, t, v[b + 2], v[b + 3] >= 0.5))
        })
        .collect()
}

fn fx_types(v: &[f32]) -> [usize; 4] {
    let t = |s: usize| v[ix(&sv::fx_param(s, "type"))].round() as usize;
    [t(1), t(2), t(3), t(4)]
}

/// Parameter a target index moves, if it has a knob.
pub fn target_index(t: usize, fx: [usize; 4]) -> Option<usize> {
    if let Some(id) = sv::target_param(t) {
        return Some(ix(id));
    }
    match t {
        22..=25 => Some(ix(&sv::fx_param(t - 21, "mix"))),
        26..=29 => Some(ix(&sv::fx_param(t - 25, sv::fx_amount_param(fx[t - 26])))),
        _ => None,
    }
}

/// Modulation target of a parameter index.
fn target_for(i: usize, fx: [usize; 4]) -> Option<usize> {
    sv::target_of(pdef(i).id, fx)
}

/// Macro mappings in use: (slot 1..=16, macro 1..=8, target index, from, to).
pub fn mappings(v: &[f32]) -> Vec<(usize, usize, usize, f32, f32)> {
    (1..=sv::MAP_SLOTS)
        .filter_map(|n| {
            let b = ix("map1_macro") + 4 * (n - 1);
            let m = v[b].round() as usize;
            let late = v[ix("map1_late_param") + n - 1].round() as usize;
            let t = sv::map_target(v[b + 1].round() as usize, late)?;
            (m > 0).then_some((n, m, t, v[b + 2], v[b + 3]))
        })
        .collect()
}

/// One ring around a dial: a modulator's colour, the normalized span it
/// sweeps and where it holds the parameter now (live, else the knob's value).
#[derive(Debug, Clone, Copy, PartialEq)]
struct Ring {
    color: u32,
    lo: f32,
    hi: f32,
    at: f32,
}

/// Live modulation the dials draw: summed per target and each source's value
/// (engine readouts; empty at rest).
#[derive(Debug, Default, Clone, Copy)]
struct LiveMod<'a> {
    mods: &'a [f32],
    srcs: &'a [f32],
}

/// Normalized offset of x (amount x source) on target t for parameter i at plain
/// value v: octaves on log targets, the plain range otherwise.
fn mod_offset(v: f32, i: usize, t: usize, x: f32) -> f32 {
    let p = pdef(i);
    let depth = sv::TARGET_DEPTH[t];
    if depth == 1.0 || p.min <= 0.0 {
        let span = if depth == 1.0 { 1.0 } else { depth / (p.max - p.min) };
        x * span
    } else {
        p.to_normalized((v * 2f32.powf(x * depth)).clamp(p.min, p.max)) - p.to_normalized(v)
    }
}

/// The rings of parameter i: one per source (its routes
/// to i summed), in source order, then its macro; at most three.
fn mod_rings(v: &[f32], i: usize, c: &Ctx) -> Vec<Ring> {
    let base = pdef(i).to_normalized(v[i]);
    let mut by_source: Vec<(usize, usize, f32)> = Vec::new();
    for &(_, s, t, a, on) in &c.routes {
        if !on || s == 0 || target_index(t, c.fx) != Some(i) {
            continue;
        }
        match by_source.iter_mut().find(|e| e.0 == s) {
            Some(e) => e.2 += a,
            None => by_source.push((s, t, a)),
        }
    }
    by_source.sort_by_key(|e| e.0);
    let mut out: Vec<Ring> = by_source
        .iter()
        .map(|&(s, t, a)| {
            let off = |x: f32| mod_offset(v[i], i, t, x);
            let (d0, d1) = if sv::bipolar_source(s) { (off(-a.abs()), off(a.abs())) } else { (0.0, off(a)) };
            let at = c.live.srcs.get(s - 1).map_or(base, |&x| base + off(a * x));
            Ring {
                color: SOURCE_COLORS[s.min(9)],
                lo: (base + d0.min(d1)).clamp(0.0, 1.0),
                hi: (base + d0.max(d1)).clamp(0.0, 1.0),
                at: at.clamp(0.0, 1.0),
            }
        })
        .collect();
    for &(_, m, t, from, to) in &c.maps {
        if t == i {
            let held = v[ix("macro1") + m - 1].clamp(0.0, 1.0);
            out.push(Ring { color: MACRO_COLOR, lo: from.min(to), hi: from.max(to), at: from + (to - from) * held });
        }
    }
    out.truncate(sv::MAX_SOURCES_PER_TARGET);
    out
}

/// The span the routes to parameter i sweep together and, while the engine
/// reports modulation, where they hold it now (the band and dot on the
/// dial's own ring).
fn mod_band(v: &[f32], i: usize, c: &Ctx) -> Option<(f32, f32, Option<f32>)> {
    let base = pdef(i).to_normalized(v[i]);
    let (mut up, mut down, mut target) = (0.0f32, 0.0f32, None);
    for &(_, s, t, a, on) in &c.routes {
        if !on || s == 0 || target_index(t, c.fx) != Some(i) {
            continue;
        }
        target = Some(t);
        if sv::bipolar_source(s) {
            up += mod_offset(v[i], i, t, a.abs());
            down -= mod_offset(v[i], i, t, -a.abs());
        } else if a > 0.0 {
            up += mod_offset(v[i], i, t, a);
        } else {
            down -= mod_offset(v[i], i, t, a);
        }
    }
    let t = target?;
    let depth = sv::TARGET_DEPTH[t];
    let now = c.live.mods.get(t).filter(|_| depth != 0.0).map(|&m| (base + mod_offset(v[i], i, t, m / depth)).clamp(0.0, 1.0));
    Some(((base - down).clamp(0.0, 1.0), (base + up).clamp(0.0, 1.0), now))
}

/// Arc around a dial (knob-local coordinates, centre (30, 19)) from lo to hi
/// at radius r; empty when the span is.
fn knob_arc(lo: f32, hi: f32, r: f32) -> String {
    if hi - lo < 0.002 {
        return String::new();
    }
    let pt = |n: f32| {
        let a = (-135.0 + 270.0 * n).to_radians();
        (30.0 + r * a.sin(), 19.0 - r * a.cos())
    };
    let (p0, p1) = (pt(lo), pt(hi));
    let large = (hi - lo) * 270.0 > 180.0;
    format!("M{:.2} {:.2} A{r} {r} 0 {} 1 {:.2} {:.2}", p0.0, p0.1, large as u8, p1.0, p1.1)
}

/// Page a parameter is shown on (mapping groups, "show" menu rows).
fn page_of(id: &str) -> usize {
    if id.starts_with("osc") || id == "cross_fm" {
        page::OSC
    } else if id.starts_with("sub_") || id.starts_with("noise_") {
        page::SUB_NOISE
    } else if ["filter_type", "cutoff", "resonance", "filter_drive", "high_pass", "key_track", "filter_env", "filter_velocity"]
        .contains(&id)
    {
        page::FILTER
    } else if id.starts_with("amp_") || id.starts_with("env2_") || id == "volume" {
        page::ENV
    } else if id.starts_with("lfo") {
        page::LFO
    } else if id.starts_with("fx") {
        page::EFFECTS
    } else if id.starts_with("macro") || id.starts_with("map") {
        page::MACROS
    } else if id.starts_with("mod") && id != "mod_wheel" {
        page::MODULATION
    } else {
        page::VOICE
    }
}

/// Mapping list group of a parameter: the effect slot for FX settings, else its page.
fn group_of(id: &str) -> String {
    match page_of(id) {
        page::EFFECTS => format!("Effect {}", &id[2..3]),
        page::SUB_NOISE => {
            if id.starts_with("sub") {
                "Sub".into()
            } else {
                "Noise".into()
            }
        }
        page::ENV => {
            if id.starts_with("env2") {
                "Env 2".into()
            } else {
                "Amp Envelope".into()
            }
        }
        p => PAGES[p].trim_end_matches('s').to_string(),
    }
}

/// Target menu groups (MOD_TARGETS indices).
const TARGET_GROUPS: [(&str, &[usize]); 7] = [
    ("Pitch", &[1, 2, 3]),
    ("Oscillators", &[4, 5, 6, 7, 8, 9, 10]),
    ("Filter", &[11, 12, 13, 14]),
    ("Amp", &[15, 16]),
    ("Voice", &[17]),
    ("LFOs", &[18, 19, 20, 21]),
    ("Effects", &[22, 23, 24, 25, 26, 27, 28, 29]),
];

/// Default amount of a new route by target.
fn default_amount(t: usize) -> f32 {
    match t {
        1..=3 => 0.0125,
        11 | 14 | 18 | 19 => 0.25,
        _ => 0.3,
    }
}

/// Macro name list groups (indices into the name choices; 0 is "Macro n").
const NAME_GROUPS: [(&str, &[&str]); 5] = [
    ("Tone", &["Brightness", "Tone", "Color", "Air", "Edge", "Focus", "Warmth", "Body"]),
    ("Character", &["Bite", "Grit", "Drive", "Dirt", "Fizz", "Growl", "Squelch", "Character", "Texture", "Thickness", "Tension"]),
    ("Motion", &["Movement", "Wobble", "Sweep", "Motion", "Speed", "Vibrato", "Detune", "Glide", "Pitch"]),
    ("Space", &["Width", "Space", "Spread", "Shimmer", "Depth", "Wet", "Bloom"]),
    ("Envelope", &["Swell", "Punch", "Attack", "Release", "Decay"]),
];

// ------------------------------------------------------------------ pictures

fn path(points: &[(f32, f32)]) -> String {
    let mut s = String::with_capacity(points.len() * 14);
    for (i, (x, y)) in points.iter().enumerate() {
        s.push_str(if i == 0 { "M" } else { " L" });
        s.push_str(&format!("{x:.2} {y:.2}"));
    }
    if s.is_empty() { "M0 0".into() } else { s }
}

/// Small circles (handles) at the given points.
fn circles(points: &[(f32, f32)], r: f32) -> String {
    let mut s = String::new();
    for (x, y) in points {
        s.push_str(&format!("M{:.2} {y:.2} a{r} {r} 0 1 0 {:.2} 0 a{r} {r} 0 1 0 {:.2} 0 ", x - r, 2.0 * r, -2.0 * r));
    }
    if s.is_empty() { "M0 0".into() } else { s }
}

fn display(points: &[(f32, f32)], base_y: f32) -> SynthDisplay {
    let mut fill = points.to_vec();
    if let (Some(first), Some(last)) = (points.first(), points.last()) {
        fill.push((last.0, base_y));
        fill.push((first.0, base_y));
    }
    let mut f = path(&fill);
    f.push_str(" Z");
    SynthDisplay {
        line: path(points).into(),
        fill: f.into(),
        ghost: "M0 0".into(),
        dots: "M0 0".into(),
        grid: "M0 0".into(),
        marks: "M0 0".into(),
        dim: false,
    }
}

fn label(slot: usize, x: f32, y: f32, text: impl Into<String>, align: i32) -> SynthLabel {
    SynthLabel { slot: slot as i32, x, y, text: text.into().into(), align, accent: false }
}

fn saw(t: f32) -> f32 {
    2.0 * t - 1.0
}

fn sin_t(t: f32) -> f32 {
    (2.0 * PI * t).sin()
}

/// One oscillator sample as the Synth pictures draw it (phase in turns; pm in turns):
/// Sine folds into a sine of itself, Triangle leans into a saw, Saw thickens with two offset
/// copies, Square narrows its pulse (DC removed), Sync replays the saw 2^(3·shape) times
/// a cycle and FM bends the phase by a sine.
pub fn osc_sample(wave: usize, shape: f32, ph: f32, pm: f32) -> f32 {
    let t = (ph + pm).rem_euclid(1.0);
    let sh = shape.clamp(0.0, 1.0);
    match wave {
        0 => {
            let y = sin_t(t);
            if sh <= 5e-4 {
                return y;
            }
            let folded = (0.5 * PI * (1.0 + 4.5 * sh) * y).sin();
            y + (3.0 * sh).min(1.0) * (folded - y)
        }
        1 => {
            let tri = if t < 0.5 { 4.0 * t - 1.0 } else { 3.0 - 4.0 * t };
            tri + sh * (saw(t) - tri)
        }
        2 => {
            if sh <= 5e-4 {
                return saw(t);
            }
            let mix = 0.7 * sh;
            (saw(t) + mix * (saw((t + 0.37).rem_euclid(1.0)) + saw((t + 0.71).rem_euclid(1.0)))) / (1.0 + 1.1 * mix)
        }
        3 => {
            let w = 0.5 - 0.45 * sh;
            (if t < w { 1.0 } else { -1.0 }) - (2.0 * w - 1.0)
        }
        4 => saw((t * 2f32.powf(3.0 * sh)).rem_euclid(1.0)),
        _ => sin_t(t + 2.2 * sh * sin_t(t) * 0.25),
    }
}

fn wave_points(w: f32, h: f32, cycles: f32, amp: f32, f: impl Fn(f32) -> f32) -> Vec<(f32, f32)> {
    let mid = h / 2.0;
    let n = (w * 2.0) as usize;
    (0..=n).map(|i| (i as f32 / n as f32 * w, mid - f(i as f32 / n as f32 * cycles).clamp(-1.2, 1.2) * amp * mid * 0.8)).collect()
}

fn osc_display(v: &[f32], k: usize) -> SynthDisplay {
    let o = |id: &str| v[ix(&format!("osc{}_{id}", k + 1))];
    let (wave, shape, level) = (o("wave").round() as usize, o("shape"), o("level"));
    let xfm = if k == 0 { v[ix("cross_fm")] } else { 0.0 };
    let note = |n: usize| {
        let p = |id: &str| v[ix(&format!("osc{n}_{id}"))];
        12.0 * (p("octave") - 2.0) + p("semi") - 12.0 + p("fine") * 0.01
    };
    let ratio = 2f32.powf((note(2) - note(1)) / 12.0);
    let (w2, s2) = (v[ix("osc2_wave")].round() as usize, v[ix("osc2_shape")]);
    // The trace is scaled by the level (at least 4 %) inside a 10 px inset (card scale).
    let amp = (DISP_H / 2.0 - 10.0 * 0.844) * level.clamp(0.04, 1.0) / (DISP_H / 2.0 * 0.8);
    let mut d = display(
        &wave_points(DISP_W, DISP_H, 2.0, amp, |ph| {
            let pm = if xfm > 0.0 { xfm * xfm * 0.8 * osc_sample(w2, s2, ph * ratio, 0.0) } else { 0.0 };
            osc_sample(wave, shape, ph, pm)
        }),
        DISP_H / 2.0,
    );
    d.dim = level <= 0.001;
    d
}

fn sub_display(v: &[f32]) -> SynthDisplay {
    let wave = v[ix("sub_waveform")].round() as usize;
    let h = v[ix("sub_shape")];
    let tune = v[ix("sub_tune")] - 24.0;
    let amp = 0.04 + 0.96 * v[ix("sub_level")];
    let cycles = 4.0 * 2f32.powf(tune / 12.0);
    display(
        &wave_points(DISP_W, DISP_H, cycles, amp, |p| {
            let t = p.rem_euclid(1.0);
            let y = match wave {
                1 => 1.0 - 4.0 * (t - 0.5).abs(),
                2 => saw(t),
                3 => {
                    if t < 0.5 {
                        1.0
                    } else {
                        -1.0
                    }
                }
                _ => sin_t(t),
            };
            (y + h * (0.7 * sin_t(2.0 * t) + 0.45 * sin_t(3.0 * t))) / (1.0 + 0.6 * h)
        }),
        DISP_H / 2.0,
    )
}

/// Noise of a type through the colour tilt (drawing model, fixed seed).
fn noise_trace(kind: usize, color: f32, seed: u32, n: usize) -> Vec<f32> {
    let mut s = seed;
    let mut rnd = || {
        s ^= s << 13;
        s ^= s >> 17;
        s ^= s << 5;
        s as f32 / u32::MAX as f32 * 2.0 - 1.0
    };
    let (mut b0, mut b1, mut b2, mut br, mut hold, mut crack, mut tone) = (0.0f32, 0.0f32, 0.0f32, 0.0f32, 0.0f32, 0.0f32, 0.0f32);
    let a = if color <= 0.5 {
        1.0 - (-2.0 * PI * 150.0 * 2f32.powf(color * 14.0) / 48_000.0).exp()
    } else {
        1.0 - (-2.0 * PI * 30.0 * 2f32.powf((color - 0.5) * 14.0) / 48_000.0).exp()
    };
    (0..n)
        .map(|i| {
            let w = rnd();
            let x = match kind {
                1 => {
                    b0 = 0.99765 * b0 + w * 0.099046;
                    b1 = 0.963 * b1 + w * 0.2965164;
                    b2 = 0.57 * b2 + w * 1.0526913;
                    (b0 + b1 + b2 + w * 0.1848) * 0.3
                }
                2 => {
                    br = 0.97 * br + 0.25 * w;
                    br
                }
                3 => w * (0.4 + 0.6 * sin_t(i as f32 / 160.0).abs()),
                4 => {
                    let sq = |f: f32| if (i as f32 * f / 4000.0).rem_euclid(1.0) < 0.5 { 1.0 } else { -1.0 };
                    0.3 * (sq(410.6) * sq(608.8) + sq(739.2) * sq(1045.4) + sq(1080.0) * sq(1600.0))
                }
                5 => {
                    if i % 6 == 0 {
                        hold = w;
                    }
                    hold
                }
                6 => {
                    if rnd() > 0.96 {
                        crack = 1.4 * w;
                    }
                    let y = crack;
                    crack *= 0.5;
                    y
                }
                _ => w * 0.6,
            };
            tone += a * (x - tone);
            if color <= 0.5 { tone } else { (x - tone) * (1.0 + 2.0 * (color - 0.5)) }
        })
        .collect()
}

fn noise_display(v: &[f32]) -> SynthDisplay {
    let kind = v[ix("noise_type")].round() as usize;
    let color = v[ix("noise_color")];
    let amp = 0.04 + 0.96 * v[ix("noise_level")];
    let n = WIDE_W as usize;
    let mid = DISP_H / 2.0;
    let to_pts = |tr: Vec<f32>| -> Vec<(f32, f32)> {
        tr.iter().enumerate().map(|(i, y)| (i as f32 * DISP_W / n as f32, mid - y.clamp(-1.0, 1.0) * amp * mid * 0.8)).collect()
    };
    let a = noise_trace(kind, color, 0x9e37_79b9, n);
    let mut d = display(&to_pts(a.clone()), mid);
    let width = v[ix("noise_width")];
    if width > 0.0 {
        let b = noise_trace(kind, color, 0x2545_f491, n);
        d.ghost = path(&to_pts(a.iter().zip(&b).map(|(x, y)| x + width * (y - x)).collect())).into();
    }
    d
}

/// Filter magnitude (dB) of the drawing model: LP24 = four one-poles with
/// resonance feedback, the others second order with damping 2 - 1.94 r;
/// times the one-pole high-pass.
pub fn filter_db(kind: usize, cutoff: f32, res: f32, hp: f32, f: f32) -> f32 {
    use cplx::C;
    let s = C(0.0, f / cutoff);
    let one = C(1.0, 0.0);
    let h = if kind == 0 {
        let k = 4.0 * res.clamp(0.0, 1.0);
        let p = (one + s) * (one + s) * (one + s) * (one + s);
        C(1.0 + k, 0.0) / (p + C(k, 0.0))
    } else {
        let d = (2.0 - 1.94 * res.clamp(0.0, 1.0)).max(0.06);
        let den = s * s + C(d, 0.0) * s + one;
        let num = match kind {
            1 => one,
            2 => C(d, 0.0) * s,
            3 => s * s,
            _ => s * s + one,
        };
        num / den
    };
    let sh = C(0.0, f / hp.max(1.0));
    let hpf = if hp > 10.5 { sh / (sh + one) } else { one };
    20.0 * (h * hpf).abs().max(1e-9).log10()
}

/// Minimal complex arithmetic for the response plots.
mod cplx {
    #[derive(Clone, Copy)]
    pub struct C(pub f32, pub f32);
    impl std::ops::Add for C {
        type Output = C;
        fn add(self, o: C) -> C {
            C(self.0 + o.0, self.1 + o.1)
        }
    }
    impl std::ops::Mul for C {
        type Output = C;
        fn mul(self, o: C) -> C {
            C(self.0 * o.0 - self.1 * o.1, self.0 * o.1 + self.1 * o.0)
        }
    }
    impl std::ops::Div for C {
        type Output = C;
        fn div(self, o: C) -> C {
            let d = (o.0 * o.0 + o.1 * o.1).max(1e-30);
            C((self.0 * o.0 + self.1 * o.1) / d, (self.1 * o.0 - self.0 * o.1) / d)
        }
    }
    impl C {
        pub fn abs(self) -> f32 {
            (self.0 * self.0 + self.1 * self.1).sqrt()
        }
    }
}

pub const FILTER_DB: (f32, f32) = (-48.0, 24.0);

fn freq_x(f: f32, w: f32) -> f32 {
    (f / 20.0).log10() / 3.0 * w
}

fn filter_display(v: &[f32], labels: &mut Vec<SynthLabel>) -> SynthDisplay {
    let (kind, fc, res, hp) = (v[ix("filter_type")].round() as usize, v[ix("cutoff")], v[ix("resonance")], v[ix("high_pass")]);
    let (lo, hi) = FILTER_DB;
    let y = |db: f32| (hi - db.clamp(lo - 4.0, hi)) / (hi - lo) * DISP_H;
    let n = (WIDE_W * 1.5) as usize;
    let pts: Vec<(f32, f32)> = (0..=n)
        .map(|i| {
            let x = i as f32 / n as f32 * WIDE_W;
            (x, y(filter_db(kind, fc, res, hp, 20.0 * 1000f32.powf(x / WIDE_W))))
        })
        .collect();
    let mut d = display(&pts, DISP_H + 2.0);
    let mut handles = vec![(freq_x(fc, WIDE_W), y(filter_db(kind, fc, res, hp, fc)))];
    if hp > 10.5 {
        handles.push((freq_x(hp, WIDE_W), DISP_H - 7.0));
    }
    d.dots = circles(&handles, 3.0).into();
    let mut grid = String::new();
    for db in [12.0, 0.0, -24.0] {
        grid.push_str(&format!("M0 {:.2} L{WIDE_W} {:.2} ", y(db), y(db)));
    }
    for f in [100.0, 1000.0, 10_000.0] {
        let x = freq_x(f, WIDE_W);
        grid.push_str(&format!("M{x:.2} 0 L{x:.2} {DISP_H} "));
    }
    d.grid = grid.into();
    let s = disp::FILTER;
    for (db, t) in [(12.0, "+12"), (0.0, "0"), (-24.0, "−24")] {
        labels.push(label(s, 4.0, y(db) - 7.0, t, 0));
    }
    for (f, t) in [(20.0, "Hz"), (100.0, "100"), (1000.0, "1k"), (10_000.0, "10k")] {
        labels.push(label(s, freq_x(f, WIDE_W) + 4.0, DISP_H - 14.0, t, 0));
    }
    d
}

/// ADSR level at time t (s) with the key released at off (drawing model of
/// the engine envelope: convex attack, -60 dB decay / release times).
fn adsr(a: f32, d: f32, s: f32, r: f32, off: f32, t: f32) -> f32 {
    let att = |t: f32| (1.3 * (1.0 - (-t / (a.max(1e-4) / (1.3f32 / 0.3).ln())).exp())).min(1.0);
    let held = |t: f32| if t < a { att(t) } else { s + (1.0 - s) * (-(t - a) * 6.9 / d.max(1e-3)).exp() };
    if t < off { held(t) } else { held(off) * (-(t - off) * 6.9 / r.max(1e-3)).exp() }
}

/// Time span of an envelope picture: the smallest of 2, 4, 10, 20, 50 x a
/// decade in which attack + decay + release fit in 85%.
fn env_span(total: f32) -> f32 {
    for decade in [0.01f32, 0.1, 1.0, 10.0] {
        for m in [2.0f32, 4.0, 10.0, 20.0, 50.0] {
            if total <= 0.85 * m * decade {
                return m * decade;
            }
        }
    }
    500.0
}

/// Seconds only ("0", "0.5 s", "1 s"), for the filter envelope picture.
fn fmt_secs_plain(t: f32) -> String {
    if t == 0.0 {
        "0".into()
    } else if t.fract().abs() < 1e-3 {
        format!("{t:.0} s")
    } else {
        format!("{t:.1} s")
    }
}

fn fmt_secs(t: f32) -> String {
    if t == 0.0 {
        "0".into()
    } else if t < 1.0 {
        format!("{:.0} ms", t * 1000.0)
    } else if t.fract().abs() < 1e-3 {
        format!("{t:.0} s")
    } else {
        format!("{t:.1} s")
    }
}

/// Time axis labels (at most 5 ticks); returns the tick grid lines.
fn time_ticks(slot: usize, span: f32, w: f32, labels: &mut Vec<SynthLabel>, h: f32) -> String {
    time_ticks_between(slot, span, 8.0, w - 8.0, labels, h, fmt_secs)
}

/// Time ticks for a picture whose time axis runs from x0 (t = 0) to x1 (t = span).
fn time_ticks_between(slot: usize, span: f32, x0: f32, x1: f32, labels: &mut Vec<SynthLabel>, h: f32, fmt: fn(f32) -> String) -> String {
    let step =
        [0.005f32, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.0, 5.0, 10.0, 20.0].into_iter().find(|s| span / s <= 5.0).unwrap_or(50.0);
    let mut grid = String::new();
    let mut t = 0.0;
    while t < span * 0.96 {
        let x = x0 + t / span * (x1 - x0);
        labels.push(label(slot, x, h - 14.0, fmt(t), 0));
        if t > 0.0 {
            grid.push_str(&format!("M{x:.2} 0 L{x:.2} {:.2} ", h - 16.0));
        }
        t += step;
    }
    if grid.is_empty() { "M0 0".into() } else { grid }
}

fn env_display(v: &[f32], pre: &str, slot: usize, w: f32, labels: &mut Vec<SynthLabel>) -> SynthDisplay {
    let g = |k: &str| v[ix(&format!("{pre}{k}"))];
    let (a, d, s, r) = (g("attack"), g("decay"), g("sustain"), g("release"));
    let span = env_span(a + d + r);
    let off = a + d + 0.15 * span;
    let (x0, x1) = (8.0, w - 8.0);
    let (top, bot) = (8.0, DISP_H - 22.0);
    let x = |t: f32| x0 + t / span * (x1 - x0);
    let y = |l: f32| bot - l * (bot - top);
    let n = 240;
    let pts: Vec<(f32, f32)> = (0..=n).map(|i| span * i as f32 / n as f32).map(|t| (x(t), y(adsr(a, d, s, r, off, t)))).collect();
    let mut dsp = display(&pts, bot);
    dsp.dots = circles(&[(x(a), y(1.0)), (x(a + d), y(adsr(a, d, s, r, off, a + d))), (x((off + r).min(span)), y(0.0))], 3.0).into();
    dsp.ghost = format!("M{:.2} {top} L{:.2} {bot}", x(off), x(off)).into();
    dsp.grid = time_ticks(slot, span, w, labels, DISP_H).into();
    dsp
}

/// LFO cycle length in seconds (synced: the note value at the project tempo).
fn lfo_period(v: &[f32], k: usize, bpm: f32) -> f32 {
    let p = |id: &str| v[ix(&format!("lfo{k}_{id}"))];
    if p("sync") >= 0.5 {
        const BEATS: [f32; 17] =
            [32.0, 16.0, 8.0, 4.0, 3.0, 2.0, 4.0 / 3.0, 1.5, 1.0, 2.0 / 3.0, 0.75, 0.5, 1.0 / 3.0, 0.375, 0.25, 1.0 / 6.0, 0.125];
        BEATS[(p("note").round() as usize).min(16)] * 60.0 / bpm.max(1.0)
    } else {
        1.0 / p("rate").max(1e-3)
    }
}

/// LFO shape of the drawing model; random shapes step through a fixed
/// pseudo-random sequence.
pub fn lfo_value(shape: usize, ph: f32) -> f32 {
    let t = ph.rem_euclid(1.0);
    let rnd = |i: i64| -> f32 {
        let mut s = (i as u32).wrapping_mul(2_654_435_761).wrapping_add(0x6d2b_79f5);
        s ^= s >> 15;
        s = s.wrapping_mul(0x2c1b_3c6d);
        s ^= s >> 12;
        s as f32 / u32::MAX as f32 * 2.0 - 1.0
    };
    match shape {
        1 => {
            if t < 0.25 {
                4.0 * t
            } else if t < 0.75 {
                2.0 - 4.0 * t
            } else {
                4.0 * t - 4.0
            }
        }
        2 => 2.0 * t - 1.0,
        3 => 1.0 - 2.0 * t,
        4 => {
            if t < 0.5 {
                1.0
            } else {
                -1.0
            }
        }
        5 => rnd(ph.floor() as i64),
        6 => {
            let (a, b) = (rnd(ph.floor() as i64), rnd(ph.floor() as i64 + 1));
            a + (b - a) * (0.5 - 0.5 * (PI * t).cos())
        }
        _ => sin_t(t),
    }
}

fn lfo_display(v: &[f32], k: usize, bpm: f32, labels: &mut Vec<SynthLabel>) -> SynthDisplay {
    let p = |id: &str| v[ix(&format!("lfo{k}_{id}"))];
    let shape = p("shape").round() as usize;
    let cycles = if shape >= 5 { 8.0 } else { 2.0 };
    let span = lfo_period(v, k, bpm) * cycles;
    let depth = p("depth").max(0.03);
    let fade = p("fade");
    let (x0, x1) = (8.0, WIDE_W - 8.0);
    let mid = (DISP_H - 16.0) / 2.0;
    let n = 300;
    let pts: Vec<(f32, f32)> = (0..=n)
        .map(|i| {
            let u = i as f32 / n as f32;
            let ramp = if fade > 0.0 { (u * span / fade).min(1.0) } else { 1.0 };
            (x0 + u * (x1 - x0), mid - lfo_value(shape, u * cycles) * depth * ramp * (mid - 6.0))
        })
        .collect();
    let slot = if k == 1 { disp::LFO1 } else { disp::LFO2 };
    labels.push(label(slot, x0, DISP_H - 14.0, "0", 0));
    labels.push(label(slot, WIDE_W / 2.0, DISP_H - 14.0, fmt_secs(span / 2.0), 1));
    labels.push(label(slot, x1, DISP_H - 14.0, fmt_secs(span), 2));
    let mut d = display(&pts, mid);
    d.grid = format!("M{:.2} 0 L{:.2} {:.2} M0 {mid:.2} L{WIDE_W} {mid:.2}", WIDE_W / 2.0, WIDE_W / 2.0, DISP_H - 18.0).into();
    d
}

/// Filter envelope over time: the cutoff (log Hz) of a middle note, with
/// ghost curves for C2 and C6 (key tracking) and the velocity range.
fn filter_mod_display(v: &[f32], labels: &mut Vec<SynthLabel>) -> SynthDisplay {
    let g = |k: &str| v[ix(k)];
    let (a, d, s, r) = (g("env2_attack"), g("env2_decay"), g("env2_sustain"), g("env2_release"));
    let (amt, kt, vel, fc) = (g("filter_env"), g("key_track"), g("filter_velocity"), g("cutoff"));
    let span = 2.0;
    let off = span * 0.6;
    // A 5.15 octave window, 2.85 octaves above and 2.3 below the cutoff (inside 20 Hz .. 20 kHz).
    let hi = (fc.max(20.0).log2() + 2.85).clamp(20f32.log2() + 5.15, 20_000f32.log2());
    let lo = hi - 5.15;
    let (x0, x1) = (6.0, WIDE_W - 3.0);
    let (top, bot) = (0.0, DISP_H - 16.0);
    let x = |t: f32| x0 + t / span * (x1 - x0);
    let y = |hz: f32| bot - ((hz.max(1.0).log2() - lo) / (hi - lo)).clamp(-0.02, 1.02) * (bot - top);
    let level = |note: f32, velocity: f32, t: f32| {
        let vs = 1.0 - vel * (1.0 - velocity);
        fc * 2f32.powf(kt * (note - 60.0) / 12.0 + amt * vs * adsr(a, d, s, r, off, t) * 7.0)
    };
    let curve = |note: f32, velocity: f32| -> Vec<(f32, f32)> {
        (0..=200).map(|i| span * i as f32 / 200.0).map(|t| (x(t), y(level(note, velocity, t)))).collect()
    };
    let mut dsp = display(&curve(60.0, 1.0), bot);
    // Shaded band from the cutoff to the envelope's peak, across the whole picture.
    let (cut_y, peak_y) = (y(fc), y(fc * 2f32.powf(amt * 7.0)));
    dsp.fill = format!("M{x0:.2} {peak_y:.2} L{x1:.2} {peak_y:.2} L{x1:.2} {cut_y:.2} L{x0:.2} {cut_y:.2} Z").into();
    // C2 and C6 dashed, C3 and C5 solid, so the key tracking spread reads at a glance.
    let mut ghost = [36.0, 84.0].map(|n| dashed(&curve(n, 1.0), 3.0, 2.5)).join(" ");
    for n in [48.0, 72.0] {
        ghost.push(' ');
        ghost.push_str(&path(&curve(n, 1.0)));
    }
    dsp.ghost = ghost.into();
    dsp.marks = dashed(&[(x0, cut_y), (x1, cut_y)], 1.5, 1.5).into();
    let peak = (x(a), peak_y);
    dsp.dots = circles(&[peak], 3.0).into();
    let sl = disp::FILTER_MOD;
    let mut hgrid = String::new();
    // The four highest reference frequencies inside the window, each label sitting just
    // above its grid line; low cutoffs get 1k .. 100 instead of a lone 1k.
    let mut freq = Vec::new();
    for &(hz, t) in FILTER_MOD_FREQS.iter().filter(|&&(hz, _)| y(hz) - 12.0 >= 0.0 && y(hz) <= bot - 4.0).take(4) {
        freq.push(label(sl, 9.5, y(hz) - 12.0, t, 0));
        hgrid.push_str(&format!("M0 {:.2} L{WIDE_W} {:.2} ", y(hz), y(hz)));
    }
    // The "+N oct" readout sits up and right of the peak dot, moved right past any frequency
    // label it would cover.
    let text = format!("{}{:.1} oct", if amt < 0.0 { "−" } else { "+" }, (amt * 7.0).abs());
    let (mut rx, ry) = (peak.0 + 8.0, (peak.1 - 14.0).max(0.0));
    for f in &freq {
        if ry < f.y + 13.0 && f.y < ry + 13.0 && rx < f.x + label_w(&f.text) {
            rx = f.x + label_w(&f.text) + 3.0;
        }
    }
    let mut reach = label(sl, rx, ry, text, 0);
    reach.accent = true;
    labels.extend(freq);
    labels.push(reach);
    // Note names beside the C6 / C2 curves, kept inside the plot and above the time axis
    // when a curve runs off the top or bottom, with C2 at least a line below C6.
    let max_y = DISP_H - 14.0 - 13.0;
    let c6 = (y(level(84.0, 1.0, span * 0.5)) - 11.5).clamp(0.0, max_y - 13.0);
    let c2 = (y(level(36.0, 1.0, span * 0.5)) - 1.0).clamp(c6 + 13.0, max_y);
    labels.push(label(sl, x(span * 0.5), c6, "C6", 0));
    labels.push(label(sl, x(span * 0.5), c2, "C2", 0));
    dsp.grid = format!("{hgrid}{}", time_ticks_between(sl, span, x0, x1, labels, DISP_H, fmt_secs_plain)).into();
    dsp
}

/// Reference frequencies of the filter envelope picture, highest first.
const FILTER_MOD_FREQS: [(f32, &str); 8] =
    [(10_000.0, "10k"), (5_000.0, "5k"), (2_000.0, "2k"), (1_000.0, "1k"), (500.0, "500"), (200.0, "200"), (100.0, "100"), (50.0, "50")];

/// Rough width of a picture label (7.75 pt text, about 4.7 pt per character).
fn label_w(text: &str) -> f32 {
    text.chars().count() as f32 * 4.7
}

/// A polyline as dashes: `on` points drawn, `off` points skipped, measured along the line.
fn dashed(points: &[(f32, f32)], on: f32, off: f32) -> String {
    let mut s = String::new();
    let mut pos = 0.0f32;
    for w in points.windows(2) {
        let ((ax, ay), (bx, by)) = (w[0], w[1]);
        let len = (bx - ax).hypot(by - ay);
        let mut t = 0.0;
        while t < len {
            let phase = pos % (on + off);
            let step = if phase < on { on - phase } else { on + off - phase }.min(len - t).max(1e-3);
            if phase < on {
                let p = |u: f32| (ax + (bx - ax) * u / len, ay + (by - ay) * u / len);
                let ((px, py), (qx, qy)) = (p(t), p(t + step));
                s.push_str(&format!("M{px:.2} {py:.2} L{qx:.2} {qy:.2} "));
            }
            t += step;
            pos += step;
        }
    }
    if s.is_empty() { "M0 0".into() } else { s }
}

fn glide_display(v: &[f32], labels: &mut Vec<SynthLabel>) -> SynthDisplay {
    let glide = v[ix("glide")];
    let bend = v[ix("bend_range")];
    let (x0, x1) = (10.0, WIDE_W - 10.0);
    let (hi_y, lo_y) = (40.0, 90.0);
    // First note (grey) held under the second, which glides up from it.
    let start = x0 + 0.4 * (x1 - x0);
    let gw = glide / 2.0 * 0.5 * (x1 - x0);
    let mut pts = Vec::new();
    if gw > 0.0 {
        for i in 0..=40 {
            let u = i as f32 / 40.0;
            let e = (1.0 - (-3.0 * u).exp()) / (1.0 - (-3.0f32).exp());
            pts.push((start + u * gw, lo_y + (hi_y - lo_y) * e));
        }
    } else {
        pts.push((start, hi_y));
    }
    pts.push((x1, hi_y));
    let mut d = display(&pts, DISP_H);
    d.fill = "M0 0".into();
    let bh = bend / 24.0 * 34.0;
    d.ghost = format!("M{:.2} {:.2} L{:.2} {:.2}", x1 - 2.0, hi_y - bh, x1 - 2.0, hi_y + bh).into();
    d.grid = format!("M{x0:.2} {lo_y:.2} L{:.2} {lo_y:.2}", x0 + 0.6 * (x1 - x0)).into();
    d.dots = circles(&[(start, if gw > 0.0 { lo_y } else { hi_y })], 3.0).into();
    labels.push(label(disp::GLIDE, 8.0, 6.0, "Pitch", 0));
    labels.push(label(disp::GLIDE, WIDE_W - 8.0, DISP_H - 16.0, "Time", 2));
    d
}

fn unison_display(v: &[f32], labels: &mut Vec<SynthLabel>) -> SynthDisplay {
    let n = v[ix("unison_voices")].round() as usize + 1;
    let det = v[ix("unison_detune")];
    let spread = v[ix("unison_spread")];
    let (cx, cy) = (WIDE_W / 2.0, DISP_H / 2.0);
    let mut stems = String::new();
    let mut dots = Vec::new();
    for u in 0..n {
        let x = if n > 1 { 2.0 * u as f32 / (n - 1) as f32 - 1.0 } else { 0.0 };
        let px = cx + x * det * det * 0.6 * (WIDE_W / 2.0 - 16.0);
        let py = cy - x * spread * (cy - 18.0);
        stems.push_str(&format!("M{px:.2} {cy:.2} L{px:.2} {py:.2} "));
        dots.push((px, py));
    }
    let d = SynthDisplay {
        line: stems.into(),
        fill: "M0 0".into(),
        ghost: "M0 0".into(),
        dots: circles(&dots, 3.0).into(),
        grid: format!("M8 {cy:.2} L{:.2} {cy:.2} M{cx:.2} 6 L{cx:.2} {:.2}", WIDE_W - 8.0, DISP_H - 6.0).into(),
        marks: "M0 0".into(),
        dim: false,
    };
    let s = disp::UNISON;
    labels.push(label(s, 10.0, cy - 16.0, "Flat", 0));
    labels.push(label(s, WIDE_W - 10.0, cy - 16.0, "Sharp", 2));
    labels.push(label(s, cx + 4.0, 2.0, "L", 0));
    labels.push(label(s, cx + 4.0, DISP_H - 16.0, "R", 0));
    d
}

// ------------------------------------------------------------------ effect pictures

/// Effect setting of slot s (1..=4) by name.
fn fxv(v: &[f32], s: usize, name: &str) -> f32 {
    v[ix(&sv::fx_param(s, name))]
}

fn db_to_amp(db: f32) -> f32 {
    10f32.powf(db / 20.0)
}

fn amp_to_db(a: f32) -> f32 {
    20.0 * a.abs().max(1e-6).log10()
}

/// Sample shaper of the tape / distortion drawing models (input amplitude
/// to output amplitude).
fn shaper(v: &[f32], s: usize, ty: usize, x: f32) -> f32 {
    if ty == 0 {
        let g = 1.0 + 7.0 * fxv(v, s, "tape_drive");
        let soft = (g * x).tanh() / g.tanh().max(1e-3);
        let warm = fxv(v, s, "tape_warmth");
        (soft + 0.12 * warm * soft * soft) * db_to_amp(fxv(v, s, "tape_output"))
    } else {
        let g = 1.0 + 24.0 * fxv(v, s, "distortion_drive");
        let b = 0.4 * fxv(v, s, "distortion_bias");
        let y = match fxv(v, s, "distortion_mode").round() as usize {
            1 => ((g * x + b).clamp(-1.0, 1.0) * 1.4).tanh() / 1.4f32.tanh(),
            2 => {
                let u = g * x + b;
                if u >= 0.0 { u.tanh() } else { 0.7 * (u / 0.7).tanh() }
            }
            3 => (g * x + b).clamp(-1.0, 1.0),
            _ => (g * x + b).tanh(),
        } - b.tanh();
        y * db_to_amp(fxv(v, s, "distortion_level"))
    }
}

const FX_DB: (f32, f32) = (-36.0, 12.0);

/// Transfer curve (input dB -> output dB) for tape, distortion and the compressor.
fn fx_curve(v: &[f32], s: usize, ty: usize, labels: &mut Vec<SynthLabel>) -> SynthDisplay {
    let (lo, hi) = FX_DB;
    let (w, h) = (FX_CURVE_W, DISP_H);
    let (x0, y0, y1) = (22.0, 8.0, h - 8.0);
    let px = |db: f32| x0 + (db - lo) / (hi - lo) * (w - x0 - 4.0);
    let py = |db: f32| y1 - (db.clamp(lo, hi) - lo) / (hi - lo) * (y1 - y0);
    let n = 120;
    let pts: Vec<(f32, f32)> = (0..=n)
        .map(|i| {
            let din = lo + (hi - lo) * i as f32 / n as f32;
            let dout = if ty == 9 {
                let (t, r, k, m) = (
                    fxv(v, s, "compressor_threshold"),
                    fxv(v, s, "compressor_ratio"),
                    fxv(v, s, "compressor_knee"),
                    fxv(v, s, "compressor_makeup"),
                );
                din + crate::device_panel::soft_knee_gr(din, t, r, k) + m
            } else {
                amp_to_db(shaper(v, s, ty, db_to_amp(din)))
            };
            (px(din), py(dout))
        })
        .collect();
    let mut d = display(&pts, y1);
    d.ghost = format!("M{:.2} {:.2} L{:.2} {:.2}", px(lo), py(lo), px(hi), py(hi)).into();
    let mut grid = String::new();
    for db in [0.0f32, -12.0, -24.0] {
        grid.push_str(&format!("M{x0} {:.2} L{w} {:.2} M{:.2} {y0} L{:.2} {y1} ", py(db), py(db), px(db), px(db)));
    }
    d.grid = grid.into();
    for db in [12.0f32, 0.0, -12.0, -24.0, -36.0] {
        let t = if db > 0.0 {
            format!("+{db:.0}")
        } else if db < 0.0 {
            format!("\u{2212}{:.0}", -db)
        } else {
            "0".into()
        };
        labels.push(label(disp::FX_CURVE, 18.0, py(db) - 6.5, t, 2));
    }
    d
}

/// Wave picture: two cycles of a sine through the effect (dry dashed), or
/// the effect's own picture (response, echoes, decay, stereo field).
fn fx_wave(v: &[f32], s: usize, ty: usize, w: f32, bpm: f32, labels: &mut Vec<SynthLabel>) -> SynthDisplay {
    let h = DISP_H;
    let mid = h / 2.0;
    let slot = disp::FX_WAVE;
    let sine = |n: usize, f: &dyn Fn(f32) -> f32| -> Vec<(f32, f32)> {
        (0..=n)
            .map(|i| {
                let u = i as f32 / n as f32;
                (8.0 + u * (w - 16.0), mid - f((2.0 * PI * 2.0 * u).sin() * 0.9).clamp(-1.3, 1.3) * (mid - 14.0))
            })
            .collect()
    };
    let resp = |f: &dyn Fn(f32) -> f32, lo: f32, hi: f32| -> Vec<(f32, f32)> {
        let n = (w * 1.5) as usize;
        (0..=n)
            .map(|i| {
                let x = i as f32 / n as f32 * w;
                let hz = 20.0 * 1000f32.powf(x / w);
                (x, 6.0 + (hi - f(hz).clamp(lo - 3.0, hi)) / (hi - lo) * (h - 22.0))
            })
            .collect()
    };
    let hz_labels = |labels: &mut Vec<SynthLabel>| {
        for (f, t) in [(100.0, "100"), (1000.0, "1k"), (10_000.0, "10k")] {
            labels.push(label(slot, freq_x(f, w) + 4.0, h - 14.0, t, 0));
        }
    };
    let rate_hz = |name: &str| -> f32 {
        let sync = fxv(v, s, &format!("{name}_sync")) >= 0.5;
        if sync {
            const BEATS: [f32; 17] =
                [32.0, 16.0, 8.0, 4.0, 3.0, 2.0, 4.0 / 3.0, 1.5, 1.0, 2.0 / 3.0, 0.75, 0.5, 1.0 / 3.0, 0.375, 0.25, 1.0 / 6.0, 0.125];
            bpm / 60.0 / BEATS[(fxv(v, s, &format!("{name}_note")).round() as usize).min(16)]
        } else {
            fxv(v, s, &format!("{name}_rate"))
        }
    };
    match ty {
        0 | 1 | 9 | 2 => {
            let mut d = if ty == 2 {
                let bits = fxv(v, s, "bitcrusher_bits");
                let q = 2f32.powf(bits - 1.0).max(1.0);
                let hold = (fxv(v, s, "bitcrusher_downsample") * 0.6).max(1.0) as usize;
                let n = (w * 2.0) as usize;
                let mut held = 0.0;
                let pts: Vec<(f32, f32)> = (0..=n)
                    .map(|i| {
                        if i % hold == 0 {
                            let u = i as f32 / n as f32;
                            held = ((2.0 * PI * 2.0 * u).sin() * 0.9 * q).round() / q;
                        }
                        (8.0 + i as f32 / n as f32 * (w - 16.0), mid - held * (mid - 14.0))
                    })
                    .collect();
                display(&pts, mid)
            } else if ty == 9 {
                let (t, r, k, m) = (
                    fxv(v, s, "compressor_threshold"),
                    fxv(v, s, "compressor_ratio"),
                    fxv(v, s, "compressor_knee"),
                    fxv(v, s, "compressor_makeup"),
                );
                display(&sine((w * 2.0) as usize, &|x| x * db_to_amp(crate::device_panel::soft_knee_gr(amp_to_db(0.9), t, r, k) + m)), mid)
            } else {
                display(&sine((w * 2.0) as usize, &|x| shaper(v, s, ty, x)), mid)
            };
            d.ghost = path(&sine(200, &|x| x)).into();
            labels.push(label(slot, w - 10.0, 6.0, "Wave", 2));
            d
        }
        3..=5 => {
            let (name, depth) = match ty {
                3 => ("chorus", fxv(v, s, "chorus_depth") / 8.0),
                4 => ("flanger", fxv(v, s, "flanger_depth") / 10.0),
                _ => ("phaser", fxv(v, s, "phaser_depth")),
            };
            let fb = match ty {
                3 => fxv(v, s, "chorus_feedback"),
                4 => fxv(v, s, "flanger_feedback"),
                _ => fxv(v, s, "phaser_feedback"),
            };
            let curve = |sweep: f32| -> Vec<(f32, f32)> {
                if ty == 5 {
                    let stages = [2.0f32, 4.0, 6.0, 12.0][(fxv(v, s, "phaser_stages").round() as usize).min(3)];
                    let centre = 200.0 * 2f32.powf(4.0 * (0.5 + 0.5 * sweep * depth));
                    resp(
                        &|hz| {
                            let notches = (stages / 2.0).max(1.0);
                            let mut db = 0.0;
                            for k in 0..notches as usize {
                                let fc = centre * 2f32.powf(k as f32 * 1.2);
                                let x = (hz / fc).log2();
                                db -= (18.0 + 10.0 * fb.abs()) / (1.0 + 40.0 * x * x);
                            }
                            db
                        },
                        -30.0,
                        6.0,
                    )
                } else {
                    let base_ms = if ty == 3 { 7.0 } else { 0.6 };
                    let tau = (base_ms + (if ty == 3 { 8.0 } else { 10.0 }) * depth * (0.5 + 0.5 * sweep)) / 1000.0;
                    resp(
                        &|hz| {
                            let ph = 2.0 * PI * hz * tau;
                            let (re, im) = (1.0 + ph.cos() * 0.7, -ph.sin() * 0.7);
                            let fbm = 1.0 - fb * 0.6 * ph.cos();
                            amp_to_db((re * re + im * im).sqrt() / 1.7 / fbm.abs().max(0.2))
                        },
                        -30.0,
                        6.0,
                    )
                }
            };
            let mut d = display(&curve(1.0), h);
            d.ghost = path(&curve(-1.0)).into();
            hz_labels(labels);
            labels.push(label(slot, w - 10.0, 6.0, format!("{:.2} Hz", rate_hz(name)), 2));
            d
        }
        6 => {
            let kind = fxv(v, s, "filter_type").round() as usize;
            let (k, steep) = match kind {
                0 => (1, false),
                1 => (0, false),
                2 => (3, false),
                3 => (3, true),
                4 => (2, false),
                5 => (2, true),
                6 => (4, false),
                _ => (4, true),
            };
            let fc = fxv(v, s, "filter_cutoff");
            let res = fxv(v, s, "filter_resonance");
            let at = |c: f32| -> Vec<(f32, f32)> {
                resp(
                    &|hz| {
                        let db = filter_db(k, c, res, 1.0, hz);
                        if steep && k != 0 { 2.0 * db } else { db }
                    },
                    -48.0,
                    24.0,
                )
            };
            let mut d = display(&at(fc), h);
            let reach = (fxv(v, s, "filter_lfo_amount") + fxv(v, s, "filter_env").abs()) / 12.0;
            if reach > 0.01 {
                let mut g = path(&at((fc * 2f32.powf(reach)).min(20_000.0)));
                g.push(' ');
                g.push_str(&path(&at((fc / 2f32.powf(reach)).max(20.0))));
                d.ghost = g.into();
            }
            d.dots = circles(&[(freq_x(fc, w), 6.0 + (24.0 - filter_db(k, fc, res, 1.0, fc).clamp(-51.0, 24.0)) / 72.0 * (h - 22.0))], 3.0)
                .into();
            hz_labels(labels);
            d
        }
        7 => {
            let sync = fxv(v, s, "delay_sync") >= 0.5;
            const NOTE_BEATS: [f32; 13] = [0.125, 1.0 / 6.0, 0.25, 0.375, 1.0 / 3.0, 0.5, 0.75, 2.0 / 3.0, 1.0, 1.5, 2.0, 3.0, 4.0];
            let time = |side: &str| -> f32 {
                let link = fxv(v, s, "delay_link") >= 0.5;
                let side = if link { "l" } else { side };
                if sync {
                    NOTE_BEATS[(fxv(v, s, &format!("delay_note_{side}")).round() as usize).min(12)] * 60.0 / bpm.max(1.0)
                } else {
                    fxv(v, s, &format!("delay_time_{side}"))
                }
            };
            let (tl, tr) = (time("l"), time("r"));
            let fb = fxv(v, s, "delay_feedback");
            let span = env_span(tl.max(tr) * 6.0);
            let x = |t: f32| 8.0 + t / span * (w - 16.0);
            let mut taps = format!("M{:.2} {mid:.2} L{:.2} 10 ", x(0.0), x(0.0));
            let mut dots = vec![(x(0.0), 10.0)];
            for (t, up) in [(tl, true), (tr, false)] {
                let mut g = 1.0f32;
                let mut k = 1.0;
                while t * k < span * 0.98 && g > 0.02 {
                    let y = if up { mid - g * (mid - 10.0) } else { mid + g * (mid - 24.0) };
                    taps.push_str(&format!("M{:.2} {mid:.2} L{:.2} {y:.2} ", x(t * k), x(t * k)));
                    dots.push((x(t * k), y));
                    g *= fb;
                    k += 1.0;
                }
            }
            let mut d = SynthDisplay {
                line: taps.into(),
                fill: "M0 0".into(),
                ghost: format!("M8 {mid:.2} L{:.2} {mid:.2}", w - 8.0).into(),
                dots: circles(&dots, 2.5).into(),
                grid: "M0 0".into(),
                marks: "M0 0".into(),
                dim: false,
            };
            d.grid = time_ticks(slot, span, w, labels, h).into();
            labels.push(label(slot, w - 10.0, 6.0, "L", 2));
            labels.push(label(slot, w - 10.0, h - 30.0, "R", 2));
            d
        }
        8 => {
            let (size, decay, pre) = (fxv(v, s, "reverb_size"), fxv(v, s, "reverb_decay"), fxv(v, s, "reverb_pre_delay") / 1000.0);
            let damp = fxv(v, s, "reverb_damping");
            let span = env_span(pre + decay * 1.1);
            let x = |t: f32| 8.0 + t / span * (w - 16.0);
            let (top, bot) = (10.0, h - 22.0);
            let n = 300;
            let pts: Vec<(f32, f32)> = (0..=n)
                .map(|i| {
                    let t = span * i as f32 / n as f32;
                    let a = if t < pre {
                        0.0
                    } else {
                        (-(t - pre) * 6.9 / decay.max(0.05)).exp() * (1.0 - 0.3 * damp * ((t - pre) / decay.max(0.05)).min(1.0))
                    };
                    let build = ((t - pre).max(0.0) / (0.01 + 0.05 * size)).min(1.0);
                    (x(t), bot - a * build * (bot - top))
                })
                .collect();
            let mut d = display(&pts, bot);
            let mut early = String::new();
            for k in 1..8 {
                let t = pre + k as f32 * (0.004 + 0.012 * size);
                let a = (0.75 - 0.08 * k as f32).max(0.1);
                early.push_str(&format!("M{:.2} {bot:.2} L{:.2} {:.2} ", x(t), x(t), bot - a * (bot - top)));
            }
            d.ghost = early.into();
            d.grid = time_ticks(slot, span, w, labels, h).into();
            d
        }
        _ => {
            let width = fxv(v, s, "widener_width");
            let side = db_to_amp(fxv(v, s, "widener_side")) * width;
            let midg = db_to_amp(fxv(v, s, "widener_mid"));
            let (cx, cy) = (w / 2.0, mid);
            let r = mid - 12.0;
            let pts: Vec<(f32, f32)> = (0..=160)
                .map(|i| {
                    let a = 2.0 * PI * i as f32 / 160.0;
                    let (m, sd) = (a.cos() * midg, a.sin() * side * 0.5);
                    (cx + sd.clamp(-1.0, 1.0) * r * 1.6, cy - m.clamp(-1.2, 1.2) * r * 0.8)
                })
                .collect();
            let mut d = display(&pts, cy);
            d.fill = format!("{} Z", path(&pts)).into();
            d.ghost = format!("M{cx:.2} 8 L{cx:.2} {:.2} M{:.2} {cy:.2} L{:.2} {cy:.2}", h - 8.0, cx - r * 1.6, cx + r * 1.6).into();
            labels.push(label(slot, cx - r * 1.6, cy - 16.0, "L", 0));
            labels.push(label(slot, cx + r * 1.6, cy - 16.0, "R", 2));
            d
        }
    }
}

/// Effect types drawn with a transfer curve beside the wave.
fn fx_has_curve(ty: usize) -> bool {
    matches!(ty, 0 | 1 | 9)
}

/// The pictures (and their labels) of one page; the other slots stay empty.
pub fn displays(v: &[f32], bpm: f32, page_ix: usize, fx_slot: usize) -> (Vec<SynthDisplay>, Vec<SynthLabel>) {
    let mut labels = Vec::new();
    let empty = || SynthDisplay {
        line: "M0 0".into(),
        fill: "M0 0".into(),
        ghost: "M0 0".into(),
        dots: "M0 0".into(),
        grid: "M0 0".into(),
        marks: "M0 0".into(),
        dim: false,
    };
    let mut d: Vec<SynthDisplay> = (0..disp::COUNT).map(|_| empty()).collect();
    match page_ix {
        page::OSC => {
            d[disp::OSC1] = osc_display(v, 0);
            d[disp::OSC2] = osc_display(v, 1);
        }
        page::SUB_NOISE => {
            d[disp::SUB] = sub_display(v);
            d[disp::NOISE] = noise_display(v);
        }
        page::FILTER => {
            d[disp::FILTER] = filter_display(v, &mut labels);
            d[disp::FILTER_MOD] = filter_mod_display(v, &mut labels);
        }
        page::ENV => {
            d[disp::AMP_ENV] = env_display(v, "amp_", disp::AMP_ENV, DISP_W, &mut labels);
            d[disp::ENV2] = env_display(v, "env2_", disp::ENV2, WIDE_W, &mut labels);
        }
        page::LFO => {
            d[disp::LFO1] = lfo_display(v, 1, bpm, &mut labels);
            d[disp::LFO2] = lfo_display(v, 2, bpm, &mut labels);
        }
        page::VOICE => {
            d[disp::GLIDE] = glide_display(v, &mut labels);
            d[disp::UNISON] = unison_display(v, &mut labels);
        }
        page::EFFECTS => {}
        _ => return (d, labels),
    }
    if page_ix != page::EFFECTS {
        return (d, labels);
    }
    let s = fx_slot.clamp(1, 4);
    let ty = fxv(v, s, "type").round() as usize;
    if fx_has_curve(ty) {
        d[disp::FX_CURVE] = fx_curve(v, s, ty, &mut labels);
        d[disp::FX_WAVE] = fx_wave(v, s, ty, FX_WAVE_W, bpm, &mut labels);
    } else {
        d[disp::FX_WAVE] = fx_wave(v, s, ty, FX_CURVE_W + FX_GAP + FX_WAVE_W, bpm, &mut labels);
    }
    let off = fxv(v, s, "on") < 0.5;
    d[disp::FX_CURVE].dim = off;
    d[disp::FX_WAVE].dim = off;
    (d, labels)
}

// ------------------------------------------------------------------ page layout

/// Everything one page shows (card coordinates).
#[derive(Default)]
struct Page {
    knobs: Vec<SynthKnobItem>,
    faders: Vec<SynthFaderItem>,
    pills: Vec<SynthPillItem>,
    texts: Vec<SynthTextItem>,
    segs: Vec<SynthSegItem>,
    scopes: Vec<SynthScopeItem>,
    latches: Vec<SynthLatchItem>,
    routes: Vec<SynthRouteRow>,
    maps: Vec<SynthMapRow>,
    split: f32,
    add_y: f32,
    add_show: bool,
}

/// Values and modulation facts a page is built from.
struct Ctx<'a> {
    v: &'a [f32],
    routes: Vec<(usize, usize, usize, f32, bool)>,
    maps: Vec<(usize, usize, usize, f32, f32)>,
    fx: [usize; 4],
    accent: Color,
    lit: usize,
    live: LiveMod<'a>,
}

/// Radius of a dial's own ring centre line (32 pt ring, 1.76 pt stroke) and the
/// step out to each modulation ring (1.7 strokes), at the card's 776.5 / 920 scale.
const RING_R: f32 = 12.76;
const RING_STEP: f32 = 2.525;

/// Pill menu actions (ui/synth.slint Studio.synth-pill).
pub mod act {
    pub const CHOICE: i32 = 0;
    pub const PRESETS: i32 = 1;
    pub const TARGETS: i32 = 2;
    pub const TIMEBASE: i32 = 3;
    pub const MACRO_NAME: i32 = 4;
    pub const ADD_MAPPING: i32 = 5;
    pub const ROUTE_SOURCE: i32 = 6;
    pub const ROUTE_TARGET: i32 = 7;
    pub const ADD_ROUTE: i32 = 8;
    pub const FX_TAB: i32 = 9;
    pub const TOGGLE: i32 = 10;
}

/// Text styles (ui/synth.slint): module title, header caption.
const TITLE: i32 = 0;
const CAPTION: i32 = 1;

impl Page {
    fn knob_at(&mut self, c: &Ctx, i: usize, x: f32, y: f32, label: &str, accent: Color) {
        let rings = mod_rings(c.v, i, c);
        // Per ring (the ringed "xs" dial): the groove, the reach at 30 %, and a fill from
        // the dial's origin to where the source holds it now. Rings sit 1.7 strokes apart.
        let origin: f32 = if pdef(i).is_bipolar() { 0.5 } else { 0.0 };
        let arcs: Vec<SynthArc> = rings
            .iter()
            .enumerate()
            .flat_map(|(k, r)| {
                let radius = RING_R + RING_STEP * (k + 1) as f32;
                let col = rgb(r.color);
                [
                    SynthArc { d: knob_arc(0.0, 1.0, radius).into(), c: Color::from_argb_u8(0x0d, 0xff, 0xff, 0xff) },
                    SynthArc { d: knob_arc(r.lo, r.hi, radius).into(), c: col.with_alpha(0.3) },
                    SynthArc { d: knob_arc(origin.min(r.at), origin.max(r.at), radius).into(), c: col },
                ]
            })
            .collect();
        let dots: Vec<Color> = rings.iter().map(|r| rgb(r.color)).collect();
        let band = mod_band(c.v, i, c);
        let lit = c.lit > 0 && c.routes.iter().any(|&(_, s, t, _, _)| s == c.lit && target_index(t, c.fx) == Some(i));
        self.knobs.push(SynthKnobItem {
            idx: i as i32,
            x,
            y,
            label: label.into(),
            accent,
            dim: false,
            lit,
            arcs: ModelRc::new(VecModel::from(arcs)),
            dots: ModelRc::new(VecModel::from(dots)),
            band_on: band.is_some_and(|b| b.1 - b.0 > 0.002),
            band_lo: band.map_or(0.0, |b| b.0),
            band_hi: band.map_or(0.0, |b| b.1),
            dot_on: band.is_some_and(|b| b.2.is_some()),
            dot: band.and_then(|b| b.2).unwrap_or(0.0),
        });
    }

    fn knob(&mut self, c: &Ctx, id: &str, x: f32, label: &str) {
        self.knob_at(c, ix(id), x, KNOB_Y, label, c.accent);
    }

    fn fader(&mut self, id: &str, x: f32, label: &str) {
        self.faders.push(SynthFaderItem { idx: ix(id) as i32, x, y: FADER_Y, label: label.into() });
    }

    #[allow(clippy::too_many_arguments)]
    fn pill(&mut self, x: f32, y: f32, w: f32, label: &str, value: impl Into<String>, action: i32, arg: usize) {
        self.pills.push(SynthPillItem {
            x,
            y,
            w,
            label: label.into(),
            value: value.into().into(),
            action,
            arg: arg as i32,
            ring: -1,
            ring_idx: 0,
            outline: false,
            chevron: true,
        });
    }

    fn choice(&mut self, c: &Ctx, id: &str, x: f32, w: f32, label: &str) {
        let i = ix(id);
        self.pill(x, HEAD_Y, w, label, pdef(i).format(c.v[i]), act::CHOICE, i);
    }

    fn text(&mut self, x: f32, y: f32, text: &str, style: i32) {
        self.texts.push(SynthTextItem { x, y, w: 200.0, text: text.into(), style, align: 0 });
    }

    fn seg(&mut self, c: &Ctx, id: &str, x: f32, w: f32) {
        let i = ix(id);
        let p = pdef(i);
        let options: Vec<slint::SharedString> = p.choices.iter().map(|s| (*s).into()).collect();
        self.segs.push(SynthSegItem {
            idx: i as i32,
            x,
            y: HEAD_Y,
            w,
            options: ModelRc::new(VecModel::from(options)),
            sel: (c.v[i] - p.min).round() as i32,
            first: p.min as i32,
        });
    }

    #[allow(clippy::too_many_arguments)]
    fn scope(&mut self, slot: usize, x: f32, w: f32, grid: i32, zero: bool) {
        self.scopes.push(SynthScopeItem { slot: slot as i32, x, y: PIC_Y, w, h: DISP_H, grid, hgrid: 1, zero });
    }

    fn latch(&mut self, c: &Ctx, id: &str, x: f32, label: &str) {
        let i = ix(id);
        self.latches.push(SynthLatchItem { idx: i as i32, x, y: KNOB_Y, label: label.into(), on: c.v[i] >= 0.5 });
    }

    /// "Targets n" pill of a modulation source.
    fn targets(&mut self, c: &Ctx, x: f32, w: f32, src: usize) {
        let n = c.routes.iter().filter(|r| r.1 == src && r.2 != 0).count();
        self.pill(x, HEAD_Y, w, "", format!("Targets {n}"), act::TARGETS, src);
    }

    /// Free / Sync pill of a sync parameter.
    fn timebase(&mut self, c: &Ctx, sync: usize, x: f32) {
        self.pill(x, HEAD_Y, 44.7, "", if c.v[sync] >= 0.5 { "Sync" } else { "Free" }, act::TIMEBASE, sync);
    }
}

/// Approximate caption width (8.25 pt semibold) for the effect header row.
fn caption_w(text: &str) -> f32 {
    text.chars().count() as f32 * 5.0 + 2.0
}

/// Mapping row texts: the target's value at a normalized position.
fn value_at(i: usize, n: f32) -> String {
    let p = pdef(i);
    p.format(p.from_normalized(n))
}

fn layout(c: &Ctx, page_ix: usize, fx_slot: usize, macro_sel: usize) -> Page {
    let mut g = Page::default();
    let v = c.v;
    match page_ix {
        page::OSC => {
            g.split = SPLIT;
            for k in 0..2 {
                let m = [M1, M2][k];
                let n = k + 1;
                let id = |s: &str| format!("osc{n}_{s}");
                g.choice(c, &id("wave"), m, 80.0, &format!("Osc {n}"));
                g.text(m + 93.7, HEAD_Y, "Octave", CAPTION);
                g.choice(c, &id("octave"), m + 131.7, 36.5, "");
                g.text(m + 181.5, HEAD_Y, "Semitones", CAPTION);
                g.choice(c, &id("semi"), m + 232.7, 49.0, "");
                g.scope(disp::OSC1 + k, m, DISP_W, 4, true);
                g.fader(&id("level"), m + FADER_DX, "Level");
                g.knob(c, &id("shape"), col4(m, 0), "Shape");
                g.knob(c, &id("fine"), col4(m, 1), "Fine");
                if k == 0 {
                    g.knob(c, "cross_fm", col4(m, 2), "Cross FM");
                }
            }
        }
        page::SUB_NOISE => {
            g.split = SPLIT;
            g.text(M1, HEAD_Y, "Sub", TITLE);
            g.text(M1 + 32.1, HEAD_Y, "Wave", CAPTION);
            g.seg(c, "sub_waveform", M1 + 63.3, 178.0);
            g.scope(disp::SUB, M1, DISP_W, 4, false);
            g.fader("sub_level", M1 + FADER_DX, "Level");
            g.knob(c, "sub_tune", col4(M1, 0), "Tune");
            g.knob(c, "sub_shape", col4(M1, 1), "Shape");
            g.text(M2, HEAD_Y, "Noise", TITLE);
            g.text(M2 + 39.6, HEAD_Y, "Type", CAPTION);
            g.choice(c, "noise_type", M2 + 68.0, 56.0, "");
            g.scope(disp::NOISE, M2, DISP_W, 1, false);
            g.fader("noise_level", M2 + FADER_DX, "Level");
            g.knob(c, "noise_color", col4(M2, 0), "Color");
            g.knob(c, "noise_width", col4(M2, 1), "Width");
        }
        page::FILTER => {
            g.split = SPLIT;
            g.choice(c, "filter_type", M1, 98.0, "Filter");
            g.scope(disp::FILTER, M1, WIDE_W, 1, false);
            for (k, (id, l)) in [("cutoff", "Cutoff"), ("resonance", "Resonance"), ("filter_drive", "Drive"), ("high_pass", "High-pass")]
                .into_iter()
                .enumerate()
            {
                g.knob(c, id, col4(M1, k), l);
            }
            g.text(M2, HEAD_Y, "Filter Mods", TITLE);
            g.scope(disp::FILTER_MOD, M2, WIDE_W, 1, false);
            for (k, (id, l)) in
                [("filter_env", "Amount"), ("env2_attack", "Attack"), ("key_track", "Key Track"), ("filter_velocity", "Velocity")]
                    .into_iter()
                    .enumerate()
            {
                g.knob(c, id, col4(M2, k), l);
            }
        }
        page::ENV => {
            g.split = SPLIT;
            g.text(M1, HEAD_Y, "Amp Envelope", TITLE);
            g.scope(disp::AMP_ENV, M1, DISP_W, 1, false);
            g.fader("volume", M1 + FADER_DX, "Volume");
            for (k, (id, l)) in [
                ("amp_attack", "Attack"),
                ("amp_decay", "Decay"),
                ("amp_sustain", "Sustain"),
                ("amp_release", "Release"),
                ("amp_velocity", "Velocity"),
            ]
            .into_iter()
            .enumerate()
            {
                g.knob(c, id, col5(M1, k), l);
            }
            g.text(M2, HEAD_Y, "Env 2", TITLE);
            g.targets(c, M2 + 38.8, 60.8, sv::source::ENV2);
            g.scope(disp::ENV2, M2, WIDE_W, 1, false);
            for (k, (id, l)) in
                [("env2_attack", "Attack"), ("env2_decay", "Decay"), ("env2_sustain", "Sustain"), ("env2_release", "Release")]
                    .into_iter()
                    .enumerate()
            {
                g.knob(c, id, col5(M2, k), l);
            }
        }
        page::LFO => {
            g.split = SPLIT;
            for k in 1..=2 {
                let m = [M1, M2][k - 1];
                let id = |s: &str| format!("lfo{k}_{s}");
                g.choice(c, &id("shape"), m, 113.0, &format!("LFO {k}"));
                g.targets(c, m + 126.7, 59.0, if k == 1 { sv::source::LFO1 } else { sv::source::LFO2 });
                g.text(m + 201.7, HEAD_Y, "Timebase", CAPTION);
                g.timebase(c, ix(&id("sync")), m + 250.7);
                g.scope(if k == 1 { disp::LFO1 } else { disp::LFO2 }, m, WIDE_W, 1, false);
                let synced = v[ix(&id("sync"))] >= 0.5;
                g.knob(c, &id(if synced { "note" } else { "rate" }), col4(m, 0), if synced { "Note" } else { "Rate" });
                g.knob(c, &id("depth"), col4(m, 1), "Depth");
                g.knob(c, &id("fade"), col4(m, 2), "Fade In");
                g.latch(c, &id("retrigger"), col4(m, 3), "Retrigger");
            }
        }
        page::MODULATION => {
            let rs = &c.routes;
            for (row, &(n, s, t, a, on)) in rs.iter().enumerate() {
                let b = ix("mod1_source") + 4 * (n - 1);
                g.routes.push(SynthRouteRow {
                    slot: n as i32,
                    y: 2.5 + row as f32 * 20.25,
                    on,
                    on_idx: (b + 3) as i32,
                    source: s as i32,
                    source_text: sv::MOD_SOURCES[s.min(9)].into(),
                    target_text: sv::MOD_TARGETS[t.min(29)].into(),
                    amount: a,
                    amount_idx: (b + 2) as i32,
                    text: pdef(b + 2).format(a).into(),
                });
            }
            g.add_y = 2.5 + rs.len() as f32 * 20.25 + 5.8;
            g.add_show = rs.len() < sv::MOD_SLOTS;
        }
        page::MACROS => {
            g.split = SPLIT;
            g.text(M1, HEAD_Y, "Macros", TITLE);
            for m in 0..sv::MACROS {
                let i = ix(&format!("macro{}", m + 1));
                let name = pdef(i + sv::MACROS).format(v[i + sv::MACROS]);
                g.knob_at(c, i, 151.5 + 75.5 * (m % 4) as f32, 102.5 + 81.0 * (m / 4) as f32, &name, rgb(MACRO_COLOR));
            }
            let name_ix = ix("macro1_name") + macro_sel;
            g.text(M2, HEAD_Y, &pdef(name_ix).format(v[name_ix]), TITLE);
            let mut rows: Vec<(String, usize, usize, f32, f32)> =
                c.maps.iter().filter(|m| m.1 == macro_sel + 1).map(|&(n, _, t, f, to)| (group_of(pdef(t).id), n, t, f, to)).collect();
            rows.sort_by_key(|r| (page_of(pdef(r.2).id), r.0.clone(), r.2));
            let mut y = 0.4;
            let mut last = String::new();
            for (group, n, t, from, to) in rows {
                if group != last {
                    g.maps.push(SynthMapRow {
                        heading: true,
                        slot: 0,
                        y,
                        label: group.clone().into(),
                        from: 0.0,
                        to: 0.0,
                        from_idx: 0,
                        to_idx: 0,
                        from_text: "".into(),
                        to_text: "".into(),
                    });
                    y += 16.6;
                    last = group;
                }
                let b = ix("map1_macro") + 4 * (n - 1);
                g.maps.push(SynthMapRow {
                    heading: false,
                    slot: n as i32,
                    y,
                    label: short_name(pdef(t)).into(),
                    from,
                    to,
                    from_idx: (b + 2) as i32,
                    to_idx: (b + 3) as i32,
                    from_text: value_at(t, from).into(),
                    to_text: value_at(t, to).into(),
                });
                y += 23.6;
            }
            g.add_y = y;
            g.add_show = c.maps.len() < sv::MAP_SLOTS;
        }
        page::VOICE => {
            g.split = SPLIT;
            g.text(M1, HEAD_Y, "Voices", CAPTION);
            g.seg(c, "voice_mode", M1 + 35.5, 119.2);
            g.text(M1 + 167.7, HEAD_Y, "Priority", CAPTION);
            g.choice(c, "note_priority", M1 + 205.7, 43.0, "");
            g.scope(disp::GLIDE, M1, WIDE_W, 1, false);
            for (k, (id, l)) in
                [("glide", "Glide"), ("bend_range", "Bend"), ("drift", "Drift"), ("mod_wheel", "Mod Wheel")].into_iter().enumerate()
            {
                g.knob(c, id, col4(M1, k), l);
            }
            g.text(M2, HEAD_Y, "Unison", CAPTION);
            g.seg(c, "unison_voices", M2 + 36.0, 151.0);
            g.scope(disp::UNISON, M2, WIDE_W, 1, false);
            let single = v[ix("unison_voices")] < 0.5;
            for (k, (id, l)) in [("unison_detune", "Detune"), ("unison_spread", "Spread")].into_iter().enumerate() {
                g.knob(c, id, col4(M2, k), l);
                if let Some(last) = g.knobs.last_mut() {
                    last.dim = single;
                }
            }
        }
        _ => fx_layout(&mut g, c, fx_slot),
    }
    g
}

/// Name of a parameter inside its module: the effect prefix and slot dropped
/// ("FX 1 Tape Drive" -> "Tape Drive"), otherwise the full name.
fn short_name(p: &ParamDef) -> String {
    p.name.strip_prefix("FX ").and_then(|r| r.get(2..)).map(str::to_string).unwrap_or_else(|| p.name.to_string())
}

/// Effects page: four slot tabs, the type's header controls, pictures, mix and dials.
fn fx_layout(g: &mut Page, c: &Ctx, slot: usize) {
    let v = c.v;
    let s = slot.clamp(1, 4);
    let f = |name: &str| ix(&sv::fx_param(s, name));
    for j in 1..=4 {
        let on = v[ix(&sv::fx_param(j, "on"))] >= 0.5;
        let ty = c.fx[j - 1].min(10);
        g.pills.push(SynthPillItem {
            x: 118.2 + 93.3 * (j - 1) as f32,
            y: HEAD_Y,
            w: 87.0,
            label: "".into(),
            value: sv::FX_TYPES[ty].into(),
            action: act::FX_TAB,
            arg: j as i32,
            ring: on as i32,
            ring_idx: ix(&sv::fx_param(j, "on")) as i32,
            outline: j == s,
            chevron: true,
        });
    }
    let ty = c.fx[s - 1].min(10);
    let mut x = 500.5;
    let caption = |g: &mut Page, x: &mut f32, t: &str| {
        g.text(*x, HEAD_Y, t, CAPTION);
        *x += caption_w(t) + 6.0;
    };
    let choice = |g: &mut Page, x: &mut f32, name: &str, w: f32| {
        let i = f(name);
        g.pill(*x, HEAD_Y, w, "", pdef(i).format(v[i]), act::CHOICE, i);
        *x += w + 8.0;
    };
    let timebase = |g: &mut Page, x: &mut f32, name: &str| {
        g.timebase(c, f(name), *x);
        *x += 52.0;
    };
    match ty {
        0 => {
            caption(g, &mut x, "Machine");
            let i = f("tape_mode");
            let options: Vec<slint::SharedString> = sv::TAPE_MODES.iter().map(|s| (*s).into()).collect();
            g.segs.push(SynthSegItem {
                idx: i as i32,
                x: 544.0,
                y: HEAD_Y,
                w: 182.0,
                options: ModelRc::new(VecModel::from(options)),
                sel: v[i].round() as i32,
                first: 0,
            });
        }
        1 => {
            caption(g, &mut x, "Mode");
            choice(g, &mut x, "distortion_mode", 76.0);
        }
        3 => {
            caption(g, &mut x, "Voices");
            choice(g, &mut x, "chorus_voices", 40.0);
            timebase(g, &mut x, "chorus_sync");
        }
        4 => {
            caption(g, &mut x, "Mode");
            choice(g, &mut x, "flanger_mode", 74.0);
            timebase(g, &mut x, "flanger_sync");
        }
        5 => {
            caption(g, &mut x, "Stages");
            choice(g, &mut x, "phaser_stages", 40.0);
            timebase(g, &mut x, "phaser_sync");
        }
        6 => {
            caption(g, &mut x, "Type");
            choice(g, &mut x, "filter_type", 92.0);
            timebase(g, &mut x, "filter_lfo_sync");
        }
        7 => {
            caption(g, &mut x, "Mode");
            choice(g, &mut x, "delay_mode", 62.0);
            let link = f("delay_link");
            g.pills.push(SynthPillItem {
                x,
                y: HEAD_Y,
                w: 52.0,
                label: "".into(),
                value: "Link".into(),
                action: act::TOGGLE,
                arg: link as i32,
                ring: (v[link] >= 0.5) as i32,
                ring_idx: link as i32,
                outline: false,
                chevron: false,
            });
            x += 60.0;
            timebase(g, &mut x, "delay_sync");
        }
        8 => {
            caption(g, &mut x, "Space");
            let i = f("reverb_mode");
            let options: Vec<slint::SharedString> = sv::REVERB_MODES.iter().map(|s| (*s).into()).collect();
            g.segs.push(SynthSegItem {
                idx: i as i32,
                x,
                y: HEAD_Y,
                w: 726.0 - x,
                options: ModelRc::new(VecModel::from(options)),
                sel: v[i].round() as i32,
                first: 0,
            });
        }
        9 => {
            caption(g, &mut x, "Style");
            choice(g, &mut x, "compressor_style", 68.0);
        }
        _ => {}
    }
    if fx_has_curve(ty) {
        g.scope(disp::FX_CURVE, M1, FX_CURVE_W, 1, false);
        g.scope(disp::FX_WAVE, M1 + FX_CURVE_W + FX_GAP, FX_WAVE_W, 1, ty != 9);
    } else {
        g.scope(disp::FX_WAVE, M1, FX_CURVE_W + FX_GAP + FX_WAVE_W, 1, matches!(ty, 2 | 7));
    }
    g.faders.push(SynthFaderItem { idx: f("mix") as i32, x: M2 + FADER_DX, y: FADER_Y, label: "Mix".into() });
    let sync = |name: &str| v[f(name)] >= 0.5;
    let rate = |pre: &str| -> (String, &'static str) {
        if sync(&format!("{pre}_sync")) { (format!("{pre}_note"), "Note") } else { (format!("{pre}_rate"), "Rate") }
    };
    let mut dials: Vec<(String, String)> = Vec::new();
    let mut add = |id: &str, l: &str| dials.push((id.to_string(), l.to_string()));
    match ty {
        0 => ["drive", "warmth", "compression", "noise", "output"]
            .iter()
            .zip(["Drive", "Warmth", "Compression", "Noise", "Output"])
            .for_each(|(i, l)| add(&format!("tape_{i}"), l)),
        1 => ["drive", "tone", "bias", "level"]
            .iter()
            .zip(["Drive", "Tone", "Bias", "Level"])
            .for_each(|(i, l)| add(&format!("distortion_{i}"), l)),
        2 => ["bits", "downsample", "dither", "jitter"]
            .iter()
            .zip(["Bits", "Downsample", "Dither", "Jitter"])
            .for_each(|(i, l)| add(&format!("bitcrusher_{i}"), l)),
        3..=5 => {
            let pre = ["chorus", "flanger", "phaser"][ty - 3];
            let (r, l) = rate(pre);
            add(&r, l);
            add(&format!("{pre}_depth"), "Depth");
            if ty == 3 {
                add("chorus_width", "Width");
                add("chorus_feedback", "Feedback");
            } else {
                add(&format!("{pre}_feedback"), "Feedback");
                add(&format!("{pre}_width"), "Width");
            }
        }
        6 => {
            add("filter_cutoff", "Cutoff");
            add("filter_resonance", "Resonance");
            add("filter_drive", "Drive");
            add("filter_env", "Envelope");
            add("filter_lfo_amount", "LFO Amount");
            if sync("filter_lfo_sync") {
                add("filter_lfo_note", "LFO Note");
            } else {
                add("filter_lfo_rate", "LFO Rate");
            }
            add("filter_lfo_shape", "LFO Shape");
        }
        7 => {
            let (synced, linked) = (sync("delay_sync"), sync("delay_link"));
            let (kind, word) = if synced { ("note", "Note") } else { ("time", "Time") };
            if linked {
                add(&format!("delay_{kind}_l"), word);
            } else {
                add(&format!("delay_{kind}_l"), &format!("{word} L"));
                add(&format!("delay_{kind}_r"), &format!("{word} R"));
            }
            add("delay_feedback", "Feedback");
            add("delay_grit", "Grit");
            add("delay_mod_depth", "Mod Depth");
            add("delay_width", "Width");
        }
        8 => ["size", "decay", "pre_delay", "damping", "width", "low_cut", "high_cut"]
            .iter()
            .zip(["Size", "Decay", "Pre-Delay", "Damping", "Width", "Low Cut", "High Cut"])
            .for_each(|(i, l)| add(&format!("reverb_{i}"), l)),
        9 => ["threshold", "ratio", "attack", "release", "knee", "makeup"]
            .iter()
            .zip(["Threshold", "Ratio", "Attack", "Release", "Knee", "Makeup"])
            .for_each(|(i, l)| add(&format!("compressor_{i}"), l)),
        _ => ["width", "mid", "side", "low_cut"]
            .iter()
            .zip(["Width", "Mid", "Side", "Low Cut"])
            .for_each(|(i, l)| add(&format!("widener_{i}"), l)),
    }
    for (k, (id, l)) in dials.iter().enumerate() {
        g.knob_at(c, f(id), 153.6 + 76.8 * k as f32, KNOB_Y, l, c.accent);
    }
}

// ------------------------------------------------------------------ edits

/// Changes that clear modulation slot n.
fn route_clear(n: usize) -> Vec<(usize, f32)> {
    let b = ix("mod1_source") + 4 * (n - 1);
    vec![(b, 0.0), (b + 1, 0.0), (b + 2, 0.0), (b + 3, 1.0)]
}

/// Changes that clear macro mapping n.
fn map_clear(n: usize) -> Vec<(usize, f32)> {
    let b = ix("map1_macro") + 4 * (n - 1);
    vec![(b, 0.0), (b + 1, 0.0), (b + 2, 0.0), (b + 3, 1.0), (ix("map1_late_param") + n - 1, 0.0)]
}

/// Modulators (routes + macro) on a parameter.
fn sources_on(v: &[f32], i: usize) -> usize {
    let fx = fx_types(v);
    routes(v).iter().filter(|r| r.1 != 0 && target_index(r.2, fx) == Some(i)).count() + mappings(v).iter().filter(|m| m.2 == i).count()
}

/// A new route source -> target in the first free slot.
fn route_add(v: &[f32], src: usize, t: usize) -> Result<Vec<(usize, f32)>, &'static str> {
    let rs = routes(v);
    if rs.iter().any(|r| r.1 == src && r.2 == t) {
        return Err("That source already modulates this target");
    }
    if let Some(i) = target_index(t, fx_types(v))
        && sources_on(v, i) >= sv::MAX_SOURCES_PER_TARGET
    {
        return Err("A Synth knob takes at most 3 modulation sources");
    }
    let n = (1..=sv::MOD_SLOTS).find(|n| !rs.iter().any(|r| r.0 == *n)).ok_or("Slots: Full")?;
    let b = ix("mod1_source") + 4 * (n - 1);
    Ok(vec![(b, src as f32), (b + 1, t as f32), (b + 2, default_amount(t)), (b + 3, 1.0)])
}

/// A new mapping of macro m (1..=8) onto parameter i: from the current value
/// to a quarter of the range away.
fn map_add(v: &[f32], m: usize, i: usize) -> Result<Vec<(usize, f32)>, &'static str> {
    let maps = mappings(v);
    if maps.iter().any(|x| x.2 == i) {
        return Err("A Synth knob takes one macro");
    }
    if sources_on(v, i) >= sv::MAX_SOURCES_PER_TARGET {
        return Err("A Synth knob takes at most 3 modulation sources");
    }
    let (pc, late) = sv::map_choices_for(i).ok_or("That control can't be mapped")?;
    let n = (1..=sv::MAP_SLOTS).find(|n| !maps.iter().any(|x| x.0 == *n)).ok_or("Mappings: Full")?;
    let b = ix("map1_macro") + 4 * (n - 1);
    let cur = pdef(i).to_normalized(v[i]);
    let to = if cur + 0.25 <= 1.0 { cur + 0.25 } else { cur - 0.25 };
    Ok(vec![(b, m as f32), (b + 1, pc as f32), (b + 2, cur), (b + 3, to), (ix("map1_late_param") + n - 1, late as f32)])
}

/// Effect settings that belong to another type than the slot's.
fn idle_fx_param(id: &str, fx: [usize; 4]) -> bool {
    let Some(rest) = id.strip_prefix("fx") else { return false };
    let Some(slot) = rest.get(..1).and_then(|s| s.parse::<usize>().ok()) else { return false };
    let name = rest.get(2..).unwrap_or("");
    let ty = sv::FX_TYPES[fx[(slot - 1).min(3)].min(10)].to_lowercase();
    !(name == "mix" || name.starts_with(&ty))
}

type Row = (String, String, Option<MenuAction>);

fn heading(t: impl Into<String>) -> Row {
    (t.into(), String::new(), None)
}

fn check(b: bool) -> String {
    if b { "\u{2713}".into() } else { String::new() }
}

/// Persistent row models (rows update in place, so a dragged element keeps its state).
struct Models {
    labels: Rc<VecModel<SynthLabel>>,
    knobs: Rc<VecModel<SynthKnobItem>>,
    faders: Rc<VecModel<SynthFaderItem>>,
    pills: Rc<VecModel<SynthPillItem>>,
    texts: Rc<VecModel<SynthTextItem>>,
    segs: Rc<VecModel<SynthSegItem>>,
    scopes: Rc<VecModel<SynthScopeItem>>,
    latches: Rc<VecModel<SynthLatchItem>>,
    routes: Rc<VecModel<SynthRouteRow>>,
    maps: Rc<VecModel<SynthMapRow>>,
}

impl std::fmt::Debug for Models {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Models")
    }
}

/// Row models of a window, handed to Slint once.
fn new_models(s: &Studio) -> Models {
    let m = Models {
        labels: Rc::new(VecModel::default()),
        knobs: Rc::new(VecModel::default()),
        faders: Rc::new(VecModel::default()),
        pills: Rc::new(VecModel::default()),
        texts: Rc::new(VecModel::default()),
        segs: Rc::new(VecModel::default()),
        scopes: Rc::new(VecModel::default()),
        latches: Rc::new(VecModel::default()),
        routes: Rc::new(VecModel::default()),
        maps: Rc::new(VecModel::default()),
    };
    s.set_synth_labels(ModelRc::from(m.labels.clone()));
    s.set_synth_knobs(ModelRc::from(m.knobs.clone()));
    s.set_synth_faders(ModelRc::from(m.faders.clone()));
    s.set_synth_pills(ModelRc::from(m.pills.clone()));
    s.set_synth_texts(ModelRc::from(m.texts.clone()));
    s.set_synth_segs(ModelRc::from(m.segs.clone()));
    s.set_synth_scopes(ModelRc::from(m.scopes.clone()));
    s.set_synth_latches(ModelRc::from(m.latches.clone()));
    s.set_synth_routes(ModelRc::from(m.routes.clone()));
    s.set_synth_map_rows(ModelRc::from(m.maps.clone()));
    m
}

impl Controller {
    /// The Synth device the Devices tab shows (selected instrument track).
    pub fn synth_device(&self) -> Option<DeviceId> {
        let t = self.selected_track.and_then(|t| self.project().track(t))?;
        if t.kind != TrackKind::Instrument || self.bottom_tab != BottomTab::Devices {
            return None;
        }
        let d = t.instrument_plugin.as_ref()?;
        (dstudio_core::builtin::def_of(d).is_some_and(|def| def.id == SYNTH_V2.id)).then_some(d.id)
    }

    /// The instrument track whose Synth the Devices tab shows.
    pub fn synth_track(&self) -> Option<TrackId> {
        self.synth_device().and(self.selected_track)
    }

    /// Current values (live gesture values included).
    pub fn synth_values(&self) -> Option<(DeviceId, Vec<f32>)> {
        let id = self.synth_device()?;
        let mut v = SYNTH_V2.values(self.project().device(id)?);
        if let Some(live) = self.synth.live.as_ref().filter(|l| l.device == id) {
            for &(i, _, x) in &live.vals {
                v[i] = x;
            }
        }
        Some((id, v))
    }

    /// One undo step: SetDeviceParam for one change, a Batch of them for
    /// several (the preset name stays). Returns false when nothing changed.
    fn synth_commit(&mut self, device: DeviceId, changes: Vec<(usize, f32)>) -> bool {
        let Some(d) = self.project().device(device) else { return false };
        let cur = SYNTH_V2.values(d);
        let mut params = BTreeMap::new();
        for (i, x) in changes {
            let x = pdef(i).snap(x);
            if x != cur[i] {
                params.insert(pdef(i).id.to_string(), x);
            } else {
                params.remove(pdef(i).id);
            }
        }
        if params.is_empty() {
            return false;
        }
        let mut commands: Vec<Command> =
            params.into_iter().map(|(param, value)| Command::SetDeviceParam { device, param, value }).collect();
        let cmd =
            if commands.len() == 1 { commands.remove(0) } else { Command::Batch { label: "Device Parameters Change".into(), commands } };
        self.doc.seal_coalescing();
        let ok = self.execute(cmd);
        self.doc.seal_coalescing();
        self.ui_dirty = true;
        ok
    }

    /// A value heard during a gesture (no model change yet).
    fn synth_live(&mut self, device: DeviceId, i: usize, x: f32) {
        let x = pdef(i).snap(x);
        // Automation: armed + playing records the move; an active lane blocks it (the knob follows the lane).
        if let Some(t) = self.synth_track()
            && self.auto_intercept(
                Some(t),
                AutomationTarget::Builtin { device, device_type: SYNTH_V2.id.into(), param: pdef(i).id.into() },
                x,
            )
        {
            return;
        }
        let start = self.project().device(device).map(|d| SYNTH_V2.value(d, pdef(i))).unwrap_or(x);
        let live = self.synth.live.get_or_insert_with(|| Live { device, vals: Vec::new() });
        if live.device != device {
            *live = Live { device, vals: Vec::new() };
        }
        match live.vals.iter_mut().find(|e| e.0 == i) {
            Some(e) if e.2 == x => return,
            Some(e) => e.2 = x,
            None => live.vals.push((i, start, x)),
        }
        let _ = self.engine.builtin_set_param(device.0, i as u32, x);
        self.ui_dirty = true;
    }

    /// Gesture end: the heard values become one command. False when no gesture was live.
    fn synth_live_end(&mut self) -> bool {
        let wrote =
            self.auto_write_end(|w| matches!(&w.target, AutomationTarget::Builtin { device_type, .. } if device_type == SYNTH_V2.id));
        if wrote {
            self.ui_dirty = true;
        }
        let Some(live) = self.synth.live.take() else { return wrote };
        let changes: Vec<(usize, f32)> = live.vals.iter().filter(|e| e.1 != e.2).map(|e| (e.0, e.2)).collect();
        if !self.synth_commit(live.device, changes) {
            for (i, start, _) in live.vals {
                let _ = self.engine.builtin_set_param(live.device.0, i as u32, start);
            }
        }
        self.ui_dirty = true;
        true
    }

    /// Escape: drop a drawer drag, a picture drag or the lit source.
    pub fn synth_cancel(&mut self) -> bool {
        if self.synth.rail.take().is_some_and(|r| r.dragging) || self.synth.scope.take().is_some() {
            if let Some(live) = self.synth.live.take() {
                for (i, start, _) in live.vals {
                    let _ = self.engine.builtin_set_param(live.device.0, i as u32, start);
                }
            }
            self.ui_dirty = true;
            return true;
        }
        if self.synth.lit != 0 {
            self.synth.lit = 0;
            self.ui_dirty = true;
            return true;
        }
        false
    }

    pub fn synth_page(&mut self, page: usize) {
        self.synth.page = page.min(PAGES.len() - 1);
        self.synth.menu = None;
        self.ui_dirty = true;
    }

    pub fn synth_macro_select(&mut self, m: usize) {
        self.synth.macro_sel = m.min(sv::MACROS - 1);
        self.ui_dirty = true;
    }

    /// Dial / fader / bar drag (control position 0..1): heard only.
    pub fn synth_param(&mut self, i: usize, norm: f32) {
        let Some(device) = self.synth_device() else { return };
        let Some(p) = SYNTH_V2.params.get(i) else { return };
        if let Some(m) = i.checked_sub(ix("macro1")).filter(|m| *m < sv::MACROS) {
            self.synth.macro_sel = m;
        }
        self.synth_live(device, i, p.from_normalized(norm.clamp(0.0, 1.0)));
    }

    /// Gesture end. A press on a lit control that didn't move detaches the lit source.
    pub fn synth_param_end(&mut self, i: usize, moved: bool) {
        if self.synth_live_end() {
            return;
        }
        if let Some(m) = i.checked_sub(ix("macro1")).filter(|m| *m < sv::MACROS) {
            self.synth.macro_sel = m;
            self.ui_dirty = true;
        }
        let (lit, Some((device, v))) = (self.synth.lit, self.synth_values()) else { return };
        if moved || lit == 0 {
            return;
        }
        let fx = fx_types(&v);
        if let Some(r) = routes(&v).into_iter().find(|r| r.1 == lit && target_index(r.2, fx) == Some(i)) {
            self.synth_commit(device, route_clear(r.0));
        }
    }

    /// Double-click: the definition's default (one step).
    pub fn synth_param_reset(&mut self, i: usize) {
        self.synth.live = None;
        let Some(device) = self.synth_device() else { return };
        if let Some(p) = SYNTH_V2.params.get(i) {
            self.synth_commit(device, vec![(i, p.default)]);
        }
    }

    pub fn synth_set(&mut self, i: usize, x: f32) {
        let Some(device) = self.synth_device() else { return };
        if i < SYNTH_V2.params.len() {
            self.synth_commit(device, vec![(i, x)]);
        }
    }

    /// On/off switches. Turning the delay's Link off copies the left time to the right first.
    pub fn synth_toggle(&mut self, i: usize) {
        let Some((device, v)) = self.synth_values() else { return };
        let Some(p) = SYNTH_V2.params.get(i) else { return };
        let on = v[i] < 0.5;
        let mut changes = vec![(i, if on { 1.0 } else { 0.0 })];
        if let Some(pre) = p.id.strip_suffix("_delay_link")
            && !on
        {
            for k in ["time", "note"] {
                changes.push((ix(&format!("{pre}_delay_{k}_r")), v[ix(&format!("{pre}_delay_{k}_l"))]));
            }
        }
        self.synth_commit(device, changes);
    }

    /// Remove button of a route (0) or a mapping (1).
    pub fn synth_remove(&mut self, kind: i32, slot: usize) {
        let Some(device) = self.synth_device() else { return };
        let changes = match kind {
            0 if (1..=sv::MOD_SLOTS).contains(&slot) => route_clear(slot),
            1 if (1..=sv::MAP_SLOTS).contains(&slot) => map_clear(slot),
            _ => return,
        };
        self.synth_commit(device, changes);
    }

    pub fn synth_bypass(&mut self) {
        let Some(d) = self.synth_device().and_then(|id| self.project().device(id)) else { return };
        let (device, enabled) = (d.id, !d.enabled);
        if let Some((t, target)) = self.device_on_target(device)
            && self.auto_switch(Some(t), target, d.enabled)
        {
            self.ui_dirty = true;
            return;
        }
        self.doc.seal_coalescing();
        self.execute(Command::SetDeviceEnabled { device, enabled });
        self.ui_dirty = true;
    }

    pub fn synth_preset_step(&mut self, dir: i32) {
        if let Some(d) = self.synth_device() {
            self.synth.live = None;
            self.panel_preset_step(d, dir);
        }
    }

    fn synth_open(&mut self, target: MenuTarget, x: f32, y: f32, w: f32, rows: Vec<Row>) {
        self.synth.menu = (!rows.is_empty()).then(|| SynthMenu { target, pos: (x, y), w: w.max(110.0), rows });
        self.ui_dirty = true;
    }

    /// A pill or link: open its menu (or act directly).
    pub fn synth_pill(&mut self, action: i32, arg: usize, x: f32, y: f32, w: f32) {
        let Some((device, v)) = self.synth_values() else { return };
        let fx = fx_types(&v);
        let rs = routes(&v);
        let (target, rows): (MenuTarget, Vec<Row>) = match action {
            act::CHOICE => {
                let Some(p) = SYNTH_V2.params.get(arg) else { return };
                let rows = p
                    .choices
                    .iter()
                    .enumerate()
                    .map(|(k, c)| {
                        (c.to_string(), check((v[arg] - p.min).round() as usize == k), Some(MenuAction::Set(vec![(arg, p.min + k as f32)])))
                    })
                    .collect();
                (MenuTarget::Choice(arg), rows)
            }
            act::PRESETS => {
                let cur = self.project().device(device).and_then(|d| d.preset.clone());
                let mut rows = vec![heading("Factory sounds")];
                rows.extend(
                    SYNTH_V2
                        .factory
                        .iter()
                        .enumerate()
                        .map(|(k, f)| (f.name.to_string(), check(cur.as_deref() == Some(f.name)), Some(MenuAction::Preset(k)))),
                );
                (MenuTarget::Presets, rows)
            }
            act::TARGETS => {
                let src = arg;
                let mut rows: Vec<Row> = rs
                    .iter()
                    .filter(|r| r.1 == src && r.2 != 0)
                    .map(|r| {
                        (
                            sv::MOD_TARGETS[r.2.min(29)].to_string(),
                            "Show".to_string(),
                            Some(MenuAction::Page(target_index(r.2, fx).map_or(page::MODULATION, |i| page_of(pdef(i).id)))),
                        )
                    })
                    .collect();
                for (g, ts) in TARGET_GROUPS {
                    let add: Vec<Row> = ts
                        .iter()
                        .filter(|t| !rs.iter().any(|r| r.1 == src && r.2 == **t))
                        .map(|&t| match route_add(&v, src, t) {
                            Ok(ch) => (sv::MOD_TARGETS[t].to_string(), String::new(), Some(MenuAction::Set(ch))),
                            Err(e) => (format!("{}: {e}", sv::MOD_TARGETS[t]), String::new(), None),
                        })
                        .collect();
                    if !add.is_empty() {
                        rows.push(heading(format!("Add target: {g}")));
                        rows.extend(add);
                    }
                }
                (MenuTarget::Targets(src), rows)
            }
            act::TIMEBASE => {
                let on = v.get(arg).is_some_and(|x| *x >= 0.5);
                (
                    MenuTarget::Timebase(arg),
                    vec![
                        ("Free".into(), check(!on), Some(MenuAction::Set(vec![(arg, 0.0)]))),
                        ("Sync".into(), check(on), Some(MenuAction::Set(vec![(arg, 1.0)]))),
                    ],
                )
            }
            act::MACRO_NAME => {
                let m = arg.min(sv::MACROS - 1);
                let i = ix("macro1_name") + m;
                let p = pdef(i);
                let cur = v[i].round() as usize;
                let mut rows = vec![(p.choices[0].to_string(), check(cur == 0), Some(MenuAction::Set(vec![(i, 0.0)])))];
                for (g, names) in NAME_GROUPS {
                    rows.push(heading(g));
                    for n in names {
                        if let Some(k) = p.choices.iter().position(|c| c == n) {
                            rows.push((n.to_string(), check(cur == k), Some(MenuAction::Set(vec![(i, k as f32)]))));
                        }
                    }
                }
                (MenuTarget::MacroName(m), rows)
            }
            act::ADD_MAPPING => {
                let m = self.synth.macro_sel + 1;
                let mapped: Vec<usize> = mappings(&v).iter().map(|x| x.2).collect();
                let mut cands: Vec<usize> = (0..SYNTH_V2.params.len())
                    .filter(|&i| {
                        sv::macro_mappable(i)
                            && !mapped.contains(&i)
                            && !idle_fx_param(pdef(i).id, fx)
                            && page_of(pdef(i).id) != page::MODULATION
                    })
                    .collect();
                cands.sort_by_key(|&i| (page_of(pdef(i).id), group_of(pdef(i).id), i));
                let mut rows = Vec::new();
                let mut last = String::new();
                for i in cands {
                    let g = group_of(pdef(i).id);
                    if g != last {
                        rows.push(heading(g.clone()));
                        last = g;
                    }
                    rows.push(match map_add(&v, m, i) {
                        Ok(ch) => (short_name(pdef(i)), String::new(), Some(MenuAction::Set(ch))),
                        Err(_) => (short_name(pdef(i)), String::new(), None),
                    });
                }
                (MenuTarget::AddMapping, rows)
            }
            act::ROUTE_SOURCE | act::ROUTE_TARGET => {
                let n = arg.clamp(1, sv::MOD_SLOTS);
                let b = ix("mod1_source") + 4 * (n - 1);
                let (s, t) = (v[b].round() as usize, v[b + 1].round() as usize);
                if action == act::ROUTE_SOURCE {
                    let mut rows: Vec<Row> = (1..sv::MOD_SOURCES.len())
                        .map(|k| (sv::MOD_SOURCES[k].to_string(), check(s == k), Some(MenuAction::Set(vec![(b, k as f32)]))))
                        .collect();
                    rows.push(("Remove".into(), String::new(), Some(MenuAction::Set(route_clear(n)))));
                    (MenuTarget::RouteSource(n), rows)
                } else {
                    let mut rows = Vec::new();
                    let mut without = v.to_vec();
                    for (i, x) in route_clear(n) {
                        without[i] = x;
                    }
                    let fx = fx_types(&without);
                    for (g, ts) in TARGET_GROUPS {
                        rows.push(heading(g));
                        for &k in ts {
                            let mut ch = vec![(b + 1, k as f32)];
                            if t == 0 {
                                ch.push((b + 2, default_amount(k)));
                            }
                            let full = k != t && target_index(k, fx).is_some_and(|i| sources_on(&without, i) >= sv::MAX_SOURCES_PER_TARGET);
                            rows.push((sv::MOD_TARGETS[k].to_string(), check(t == k), (!full).then_some(MenuAction::Set(ch))));
                        }
                    }
                    (MenuTarget::RouteTarget(n), rows)
                }
            }
            act::ADD_ROUTE => {
                let Some(n) = (1..=sv::MOD_SLOTS).find(|n| !rs.iter().any(|r| r.0 == *n)) else {
                    self.set_status("Slots: Full".to_string(), 3);
                    return;
                };
                let b = ix("mod1_source") + 4 * (n - 1);
                let rows = (1..sv::MOD_SOURCES.len())
                    .map(|k| {
                        (
                            sv::MOD_SOURCES[k].to_string(),
                            String::new(),
                            Some(MenuAction::Set(vec![(b, k as f32), (b + 2, 0.3), (b + 3, 1.0)])),
                        )
                    })
                    .collect();
                (MenuTarget::AddRoute, rows)
            }
            act::FX_TAB => {
                let j = arg.clamp(1, 4);
                if self.synth.fx_slot != j {
                    self.synth.fx_slot = j;
                    self.synth.menu = None;
                    self.ui_dirty = true;
                    return;
                }
                let (ti, oi) = (ix(&sv::fx_param(j, "type")), ix(&sv::fx_param(j, "on")));
                let on = v[oi] >= 0.5;
                let mut rows = vec![("None".to_string(), check(!on), Some(MenuAction::Set(vec![(oi, 0.0)])))];
                rows.extend(
                    sv::FX_TYPES
                        .iter()
                        .enumerate()
                        .map(|(k, n)| (n.to_string(), check(on && fx[j - 1] == k), Some(MenuAction::Set(vec![(ti, k as f32), (oi, 1.0)])))),
                );
                (MenuTarget::FxType(j), rows)
            }
            act::TOGGLE => {
                self.synth_toggle(arg);
                return;
            }
            _ => return,
        };
        self.synth_open(target, x, y, w, rows);
    }

    /// Right press on a pill: the menu of the parameter it shows (choices, switches,
    /// effect types, route sources and targets); pills without one do nothing.
    pub fn synth_pill_menu(&mut self, action: i32, arg: usize, x: f32, y: f32) {
        let param = match action {
            act::CHOICE | act::TOGGLE | act::TIMEBASE => Some(arg),
            act::FX_TAB => (1..=4).contains(&arg).then(|| ix(&sv::fx_param(arg, "type"))),
            act::ROUTE_SOURCE | act::ROUTE_TARGET if (1..=sv::MOD_SLOTS).contains(&arg) => {
                Some(ix("mod1_source") + 4 * (arg - 1) + usize::from(action == act::ROUTE_TARGET))
            }
            _ => None,
        };
        if let Some(i) = param {
            self.synth_knob_menu(i, x, y);
        }
    }

    /// Right click on a control: Automate, reset, macro mappings and modulation sources.
    pub fn synth_knob_menu(&mut self, i: usize, x: f32, y: f32) {
        let Some((_, v)) = self.synth_values() else { return };
        let Some(p) = SYNTH_V2.params.get(i) else { return };
        let fx = fx_types(&v);
        let mut rows: Vec<Row> = Vec::new();
        if p.automatable {
            rows.push((format!("Automate {}", p.name), String::new(), Some(MenuAction::Automate(i))));
        }
        rows.push(("Reset to default".into(), p.format(p.default), Some(MenuAction::Set(vec![(i, p.default)]))));
        if sv::macro_mappable(i) {
            rows.push(heading("Macros"));
            let mapped = mappings(&v).into_iter().find(|m| m.2 == i);
            for m in 1..=sv::MACROS {
                let ni = ix("macro1_name") + m - 1;
                let name = pdef(ni).format(v[ni]);
                rows.push(match mapped {
                    Some(x) if x.1 == m => (format!("Remove from {name}"), check(true), Some(MenuAction::Set(map_clear(x.0)))),
                    Some(_) => (format!("Map to {name}"), String::new(), None),
                    None => match map_add(&v, m, i) {
                        Ok(ch) => (format!("Map to {name}"), String::new(), Some(MenuAction::Set(ch))),
                        Err(_) => (format!("Map to {name}"), String::new(), None),
                    },
                });
            }
            rows.push(("Open Macros page".into(), String::new(), Some(MenuAction::Page(page::MACROS))));
        }
        if let Some(t) = target_for(i, fx) {
            rows.push(heading("Mods"));
            let rs = routes(&v);
            for s in 1..sv::MOD_SOURCES.len() {
                let name = sv::MOD_SOURCES[s];
                rows.push(match rs.iter().find(|r| r.1 == s && r.2 == t) {
                    Some(r) => (format!("Remove {name}"), check(true), Some(MenuAction::Set(route_clear(r.0)))),
                    None => match route_add(&v, s, t) {
                        Ok(ch) => (format!("Add {name}"), String::new(), Some(MenuAction::Set(ch))),
                        Err(_) => (format!("Add {name}"), String::new(), None),
                    },
                });
            }
        }
        self.synth_open(MenuTarget::Knob(i), x, y, 190.0, rows);
    }

    pub fn synth_menu_pick(&mut self, k: usize) {
        let Some(m) = self.synth.menu.take() else { return };
        self.ui_dirty = true;
        let Some(device) = self.synth_device() else { return };
        match m.rows.get(k).and_then(|r| r.2.clone()) {
            Some(MenuAction::Preset(n)) if m.target == MenuTarget::Presets => {
                self.synth.live = None;
                self.panel_load_preset(device, n);
            }
            Some(MenuAction::Set(changes)) => {
                self.synth_commit(device, changes);
            }
            Some(MenuAction::Page(p)) => self.synth_page(p),
            Some(MenuAction::Automate(i)) => {
                if let Some(p) = SYNTH_V2.params.get(i) {
                    self.mod_automate(device, dstudio_core::ModParam::Builtin(p.id.to_string()));
                }
            }
            _ => {}
        }
    }

    pub fn synth_menu_dismiss(&mut self) {
        self.synth.menu = None;
        self.ui_dirty = true;
    }

    /// Modulation sources drawer: a click lights the source (and shows its page),
    /// a drag onto a control adds a route.
    pub fn synth_rail(&mut self, src: usize, phase: i32, x: f32, y: f32) {
        if !(1..sv::MOD_SOURCES.len()).contains(&src) {
            return;
        }
        match phase {
            0 => self.synth.rail = Some(RailDrag { src, start: (x, y), pos: (x, y), dragging: false }),
            1 => {
                if let Some(r) = self.synth.rail.as_mut() {
                    r.pos = (x, y);
                    if !r.dragging && (x - r.start.0).hypot(y - r.start.1) > 4.0 {
                        r.dragging = true;
                    }
                    self.ui_dirty = true;
                }
            }
            _ => {
                let Some(r) = self.synth.rail.take() else { return };
                self.ui_dirty = true;
                if !r.dragging {
                    self.synth.lit = if self.synth.lit == src { 0 } else { src };
                    if self.synth.lit != 0 {
                        self.synth_page(match src {
                            sv::source::ENV2 | sv::source::AMP_ENV => page::ENV,
                            sv::source::LFO1 | sv::source::LFO2 => page::LFO,
                            _ => page::MODULATION,
                        });
                    }
                    return;
                }
                let d2 = |k: &(usize, f32, f32)| (k.1 - x).hypot(k.2 - y);
                let Some(i) = self.synth.knob_pos.iter().filter(|k| d2(k) <= 24.0).min_by(|a, b| d2(a).total_cmp(&d2(b))).map(|k| k.0)
                else {
                    return;
                };
                let Some((device, v)) = self.synth_values() else { return };
                let Some(t) = target_for(i, fx_types(&v)) else {
                    self.set_status(format!("{} can't be modulated", pdef(i).name), 3);
                    return;
                };
                match route_add(&v, src, t) {
                    Ok(ch) => {
                        self.synth_commit(device, ch);
                    }
                    Err(e) => self.set_status(e.to_string(), 3),
                }
            }
        }
    }

    /// Drags in the pictures: filter corner / high-pass, filter envelope
    /// amount, envelope handles, LFO rate and depth, glide, unison spread.
    pub fn synth_scope_drag(&mut self, slot: usize, phase: i32, x: f32, y: f32) {
        let Some((device, v)) = self.synth_values() else { return };
        let n = |id: &str| (ix(id), pdef(ix(id)).to_normalized(v[ix(id)]));
        let env_pre = |slot: usize| if slot == disp::AMP_ENV { "amp_" } else { "env2_" };
        match phase {
            0 => {
                let (handle, norms) = match slot {
                    disp::FILTER => {
                        let hp = v[ix("high_pass")];
                        if hp > 10.5 && (x - freq_x(hp, WIDE_W)).abs() < 10.0 && y > DISP_H - 22.0 {
                            (1, vec![n("high_pass")])
                        } else {
                            (0, vec![n("cutoff"), n("resonance")])
                        }
                    }
                    disp::FILTER_MOD => (0, vec![n("filter_env")]),
                    disp::AMP_ENV | disp::ENV2 => {
                        let pre = env_pre(slot);
                        let g = |k: &str| v[ix(&format!("{pre}{k}"))];
                        let (a, d, r) = (g("attack"), g("decay"), g("release"));
                        let w = if slot == disp::AMP_ENV { DISP_W } else { WIDE_W };
                        let span = env_span(a + d + r);
                        let off = a + d + 0.15 * span;
                        let px = |t: f32| 8.0 + t / span * (w - 16.0);
                        let hs = [px(a), px(a + d), px((off + r).min(span))];
                        let h = (0..3).min_by(|p, q| (hs[*p] - x).abs().total_cmp(&(hs[*q] - x).abs())).unwrap_or(0);
                        (h, ["attack", "decay", "sustain", "release"].iter().map(|k| n(&format!("{pre}{k}"))).collect())
                    }
                    disp::LFO1 | disp::LFO2 => {
                        let k = if slot == disp::LFO1 { 1 } else { 2 };
                        let synced = v[ix(&format!("lfo{k}_sync"))] >= 0.5;
                        (0, vec![n(&format!("lfo{k}_{}", if synced { "note" } else { "rate" })), n(&format!("lfo{k}_depth"))])
                    }
                    disp::GLIDE => (0, vec![n("glide")]),
                    disp::UNISON => (0, vec![n("unison_detune"), n("unison_spread")]),
                    _ => return,
                };
                self.synth.scope = Some(ScopeDrag { slot, handle, start: (x, y), norms });
            }
            1 => {
                let Some(sd) = self.synth.scope.clone().filter(|s| s.slot == slot) else { return };
                let (dx, dy) = (x - sd.start.0, y - sd.start.1);
                let w = match slot {
                    disp::AMP_ENV => DISP_W,
                    _ => WIDE_W,
                };
                let h = DISP_H - 30.0;
                let set = |k: usize, d: f32| -> (usize, f32) {
                    let (i, start) = sd.norms[k];
                    (i, pdef(i).from_normalized((start + d).clamp(0.0, 1.0)))
                };
                let changes = match (slot, sd.handle) {
                    (disp::FILTER, 0) => vec![set(0, dx / w), set(1, -dy / h)],
                    (disp::FILTER, _) => vec![set(0, dx / (w * 200f32.ln() / 1000f32.ln()))],
                    (disp::FILTER_MOD, _) => vec![set(0, -dy / h)],
                    (disp::AMP_ENV | disp::ENV2, 0) => vec![set(0, dx / w)],
                    (disp::AMP_ENV | disp::ENV2, 1) => vec![set(1, dx / w), set(2, -dy / h)],
                    (disp::AMP_ENV | disp::ENV2, _) => vec![set(3, dx / w)],
                    (disp::LFO1 | disp::LFO2, _) => vec![set(0, dx / (w * 1.5)), set(1, -dy / h)],
                    (disp::GLIDE, _) => vec![set(0, dx / (w * 0.6))],
                    _ => vec![set(0, dx / (w / 2.0)), set(1, -dy / (DISP_H / 2.0))],
                };
                for (i, x) in changes {
                    self.synth_live(device, i, x);
                }
            }
            _ => {
                self.synth.scope = None;
                self.synth_live_end();
            }
        }
    }

    /// Engine readouts -> live modulation of the synth's pictures (called with the panel meters).
    /// Readout order (engine/src/devices/synth_v2.cpp): 20 status values, then one per target.
    pub(crate) fn synth_poll(&mut self) {
        let Some(id) = self.synth_device() else {
            self.synth.mods.clear();
            self.synth.srcs.clear();
            return;
        };
        let mut out = Vec::new();
        let ro_src = RO_MOD + sv::MOD_TARGETS.len();
        let quantize = |r: &[f32]| -> Vec<f32> {
            let q: Vec<f32> = r.iter().map(|m| (m * 1000.0).round() / 1000.0).collect();
            if q.iter().all(|m| *m == 0.0) { Vec::new() } else { q }
        };
        let (mods, srcs) = match self.engine.builtin_readouts(id.0, &mut out) {
            Ok(()) if out.len() >= ro_src + SOURCES => (quantize(&out[RO_MOD..ro_src]), quantize(&out[ro_src..ro_src + SOURCES])),
            _ => (Vec::new(), Vec::new()),
        };
        if mods != self.synth.mods || srcs != self.synth.srcs {
            self.synth.mods = mods;
            self.synth.srcs = srcs;
            self.ui_dirty = true;
        }
    }

    /// The synth's parameter values with the live modulation the pictures draw
    /// (oscillator pitch, shape, level and cross FM), as the engine applies it.
    fn picture_values(&self, v: &[f32]) -> Vec<f32> {
        let mut pv = v.to_vec();
        let m = |t: &str| sv::MOD_TARGETS.iter().position(|x| *x == t).and_then(|i| self.synth.mods.get(i)).copied().unwrap_or(0.0);
        for (id, t) in [
            ("osc1_shape", "Osc 1 Shape"),
            ("osc2_shape", "Osc 2 Shape"),
            ("osc1_level", "Osc 1 Level"),
            ("osc2_level", "Osc 2 Level"),
            ("cross_fm", "Cross FM"),
        ] {
            pv[ix(id)] = (v[ix(id)] + m(t)).clamp(0.0, 1.0);
        }
        pv[ix("osc1_semi")] += m("Osc 1 Pitch");
        pv[ix("osc2_semi")] += m("Osc 2 Pitch");
        pv
    }

    /// Draw the page's pictures; returns their labels.
    fn synth_pictures(&mut self, v: &[f32], bpm: f32) -> Vec<SynthLabel> {
        let pv = self.picture_values(v);
        let (pictures, labels) = displays(&pv, bpm, self.synth.page, self.synth.fx_slot);
        sync_model(&self.synth_displays_model, pictures);
        self.synth.drawn = Some((pv, self.synth.page, self.synth.fx_slot, bpm.to_bits()));
        labels
    }

    /// Redraw only the pictures when the live modulation moved them.
    fn synth_pictures_changed(&mut self, v: &[f32], bpm: f32) -> Option<Vec<SynthLabel>> {
        let pv = self.picture_values(v);
        if self.synth.drawn.as_ref() == Some(&(pv, self.synth.page, self.synth.fx_slot, bpm.to_bits())) {
            return None;
        }
        Some(self.synth_pictures(v, bpm))
    }

    /// Synth device view models -> Slint.
    pub fn push_synth(&mut self, s: &Studio) {
        let vals = self.synth_values();
        s.set_synth_visible(vals.is_some());
        s.set_synth_page(self.synth.page as i32);
        let Some((device, v)) = vals else {
            s.set_synth_menu_open(false);
            s.set_synth_ghost_show(false);
            return;
        };
        self.synth.fx_slot = self.synth.fx_slot.clamp(1, 4);
        let Some(d) = self.project().device(device) else { return };
        let (enabled, preset) = (self.device_on_now(d), d.preset.clone());
        let accent = self.project().find_device(device).map_or(rgb(0x439b94), |(ti, _)| card_color(self.project().tracks[ti].color));
        let bpm = self.bpm_now() as f32;
        // The dials' rings follow the live modulation, so it is part of what the page is built from.
        let built_from: Vec<f32> = v
            .iter()
            .chain(&self.synth.mods)
            .chain(&self.synth.srcs)
            .copied()
            .chain([self.synth.mods.len() as f32, self.synth.srcs.len() as f32])
            .collect();
        let key = (
            built_from,
            [self.synth.page, self.synth.fx_slot, self.synth.macro_sel, self.synth.lit],
            accent.as_argb_encoded(),
            preset.clone(),
            bpm.to_bits(),
        );
        if self.synth.pushed.as_ref() != Some(&key) {
            let modified = preset.as_deref().and_then(|name| SYNTH_V2.factory.iter().find(|f| f.name == name)).is_some_and(|f| {
                let want = DevicePreset::factory(&SYNTH_V2, f).load_values(&SYNTH_V2);
                SYNTH_V2.params.iter().enumerate().any(|(i, p)| want.get(p.id).is_some_and(|w| (w - v[i]).abs() > 1e-4 * (1.0 + w.abs())))
            });
            s.set_synth_preset(match &preset {
                Some(n) if modified => format!("{n} (modified)").into(),
                Some(n) => n.clone().into(),
                None => "Init".into(),
            });
            let params: Vec<SynthParam> = SYNTH_V2
                .params
                .iter()
                .zip(&v)
                .map(|(p, &x)| SynthParam { norm: p.to_normalized(x), text: p.format(x).into(), bipolar: p.is_bipolar(), auto: false })
                .collect();
            sync_model(&self.synth_params_model, params);
            let live = LiveMod { mods: &self.synth.mods, srcs: &self.synth.srcs };
            let c = Ctx { v: &v, routes: routes(&v), maps: mappings(&v), fx: fx_types(&v), accent, lit: self.synth.lit, live };
            let g = layout(&c, self.synth.page, self.synth.fx_slot, self.synth.macro_sel);
            let labels = self.synth_pictures(&v, bpm);
            self.synth.knob_pos = g
                .knobs
                .iter()
                .map(|k| (k.idx as usize, k.x, k.y))
                .chain(g.faders.iter().map(|f| (f.idx as usize, f.x, f.y + 45.0)))
                .collect();
            s.set_synth_split(g.split);
            s.set_synth_add_y(g.add_y);
            s.set_synth_add_show(g.add_show);
            if self.synth.models.is_none() {
                self.synth.models = Some(new_models(s));
            }
            if let Some(m) = &self.synth.models {
                sync_model(&m.labels, labels);
                sync_model(&m.knobs, g.knobs);
                sync_model(&m.faders, g.faders);
                sync_model(&m.pills, g.pills);
                sync_model(&m.texts, g.texts);
                sync_model(&m.segs, g.segs);
                sync_model(&m.scopes, g.scopes);
                sync_model(&m.latches, g.latches);
                sync_model(&m.routes, g.routes);
                sync_model(&m.maps, g.maps);
            }
            self.synth.pushed = Some(key);
        } else if let Some(labels) = self.synth_pictures_changed(&v, bpm)
            && let Some(m) = &self.synth.models
        {
            sync_model(&m.labels, labels);
        }
        s.set_synth_enabled(enabled);
        // The shared card header (and the folded strip) read the Synth as a device card.
        let folded = self.project().device(device).is_some_and(|d| d.folded);
        s.set_synth_card(DeviceCard {
            id: device.0 as i32,
            name: "Synth".into(),
            builtin: true,
            instrument: true,
            swappable: true,
            enabled,
            folded,
            width: if folded { crate::device_mod::FOLDED_W } else { crate::devices::SYNTH_W },
            accent,
            accent_light: crate::device_mod::is_light(accent),
            accent_text: crate::device_mod::accent_text(accent),
            preset: s.get_synth_preset(),
            ..Default::default()
        });
        s.set_synth_lit(self.synth.lit as i32);
        s.set_synth_lit_color(rgb(SOURCE_COLORS[self.synth.lit.min(9)]));
        s.set_synth_macro_sel(self.synth.macro_sel as i32);
        let ghost = self.synth.rail.filter(|r| r.dragging);
        s.set_synth_ghost_show(ghost.is_some());
        if let Some(r) = ghost {
            s.set_synth_ghost_x(r.pos.0);
            s.set_synth_ghost_y(r.pos.1);
            s.set_synth_ghost_src(r.src as i32);
        }
        s.set_synth_menu_open(self.synth.menu.is_some());
        if let Some(m) = &self.synth.menu {
            let grouped = m.rows.iter().any(|r| r.2.is_none());
            let rows: Vec<MenuItem> = m
                .rows
                .iter()
                .enumerate()
                .map(|(k, (label, hint, a))| MenuItem {
                    swatch: Default::default(),
                    label: label.clone().into(),
                    shortcut: hint.clone().into(),
                    enabled: a.is_some(),
                    separator: a.is_none() && k > 0,
                    submenu: grouped && a.is_some(),
                    y: MENU_PAD + k as f32 * MENU_ROW,
                })
                .collect();
            s.set_synth_menu_h(MENU_PAD * 2.0 + rows.len() as f32 * MENU_ROW);
            sync_model(&self.synth_menu_model, rows);
            s.set_synth_menu_x(m.pos.0);
            s.set_synth_menu_y(m.pos.1);
            s.set_synth_menu_w(m.w);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn defaults() -> Vec<f32> {
        SYNTH_V2.params.iter().map(|p| p.default).collect()
    }

    fn ctx(v: &[f32]) -> Ctx<'_> {
        Ctx { v, routes: routes(v), maps: mappings(v), fx: fx_types(v), accent: rgb(0x439b94), lit: 0, live: LiveMod::default() }
    }

    #[test]
    fn every_page_lays_out_inside_the_card() {
        let mut v = defaults();
        for ty in 0..sv::FX_TYPES.len() {
            v[ix("fx1_type")] = ty as f32;
            for p in 0..PAGES.len() {
                let g = layout(&ctx(&v), p, 1, 0);
                for k in &g.knobs {
                    assert!(
                        k.x > 107.0 && k.x < 740.0 && k.y > 30.0 && k.y < 250.0,
                        "page {p} type {ty}: knob {} at {} {}",
                        k.idx,
                        k.x,
                        k.y
                    );
                }
                for pl in &g.pills {
                    assert!(pl.x + pl.w < 742.0, "page {p} type {ty}: pill {} ends at {}", pl.value, pl.x + pl.w);
                }
                for s in &g.segs {
                    assert!(s.x + s.w < 742.0, "page {p}: switch ends at {}", s.x + s.w);
                }
                if p != page::MODULATION && p != page::MACROS {
                    assert!(!g.knobs.is_empty() && !g.scopes.is_empty(), "page {p} type {ty} is empty");
                }
            }
        }
    }

    #[test]
    fn pictures_are_finite_for_every_choice() {
        let mut v = defaults();
        for ty in 0..sv::FX_TYPES.len() {
            for slot in 1..=4 {
                v[ix(&sv::fx_param(slot, "type"))] = ty as f32;
                v[ix(&sv::fx_param(slot, "on"))] = 1.0;
            }
            for (i, p) in SYNTH_V2.params.iter().enumerate() {
                if p.is_discrete() && !p.id.starts_with("map") && !p.id.starts_with("mod") {
                    v[i] = p.max;
                }
            }
            for p in 0..PAGES.len() {
                let (d, labels) = displays(&v, 120.0, p, 1);
                for (k, x) in d.iter().enumerate() {
                    for s in [&x.line, &x.fill, &x.ghost, &x.dots, &x.grid] {
                        assert!(!s.contains("NaN") && !s.contains("inf"), "slot {k} type {ty}: {}", &s[..s.len().min(80)]);
                    }
                }
                assert!(labels.iter().all(|l| l.x.is_finite() && l.y.is_finite()));
            }
        }
    }

    /// Filter envelope picture settings that used to crowd its labels: fast and slow attacks,
    /// low and high cutoffs, positive and negative envelope amounts.
    fn filter_mod_cases() -> Vec<Vec<f32>> {
        let mut out = Vec::new();
        for cutoff in [20.0, 150.0, 1000.0, 6000.0, 20_000.0] {
            for amt in [-1.0, 0.25, 1.0] {
                for attack in [pdef(ix("env2_attack")).min, 0.3] {
                    for kt in [0.0, defaults()[ix("key_track")]] {
                        let mut v = defaults();
                        v[ix("cutoff")] = cutoff;
                        v[ix("filter_env")] = amt;
                        v[ix("env2_attack")] = attack;
                        v[ix("key_track")] = kt;
                        out.push(v);
                    }
                }
            }
        }
        out
    }

    #[test]
    fn filter_envelope_frequency_labels_stay_clear() {
        let bx = |l: &SynthLabel| (l.x, l.y, l.x + label_w(&l.text), l.y + 13.0);
        let is_freq = |l: &SynthLabel| FILTER_MOD_FREQS.iter().any(|&(_, t)| t == l.text.as_str());
        for v in filter_mod_cases() {
            let mut labels = Vec::new();
            filter_mod_display(&v, &mut labels);
            let hz: Vec<_> = labels.iter().filter(|l| is_freq(l)).collect();
            assert!(hz.len() >= 3, "cutoff {}: only {} frequency labels", v[ix("cutoff")], hz.len());
            for h in &hz {
                let (hx0, hy0, hx1, hy1) = bx(h);
                assert!(hy0 >= 0.0 && hy1 <= DISP_H - 14.0, "{} sits outside the plot at {hx0} {hy0}", h.text);
                for o in labels.iter().filter(|o| o.text != h.text) {
                    let (ox0, oy0, ox1, oy1) = bx(o);
                    let overlap = hx0 < ox1 && ox0 < hx1 && hy0 < oy1 && oy0 < hy1;
                    assert!(!overlap, "cutoff {} amount {}: {} overlaps {}", v[ix("cutoff")], v[ix("filter_env")], h.text, o.text);
                }
            }
        }
    }

    #[test]
    fn filter_envelope_note_labels_stay_in_the_plot() {
        let mut cases = filter_mod_cases();
        // Flat curves at the very top: both labels would clamp onto the same spot.
        let mut flat = defaults();
        flat[ix("cutoff")] = 20_000.0;
        flat[ix("filter_env")] = 0.0;
        flat[ix("key_track")] = 0.0;
        cases.push(flat);
        for v in cases {
            let mut labels = Vec::new();
            filter_mod_display(&v, &mut labels);
            let case = format!("cutoff {} amount {} key track {}", v[ix("cutoff")], v[ix("filter_env")], v[ix("key_track")]);
            let at = |t: &str| labels.iter().find(|l| l.text == t).unwrap().y;
            for n in ["C6", "C2"] {
                assert!(at(n) >= 0.0 && at(n) + 13.0 <= DISP_H - 14.0, "{case}: {n} at y {}", at(n));
            }
            assert!(at("C2") >= at("C6") + 13.0, "{case}: C2 at {} overlaps C6 at {}", at("C2"), at("C6"));
        }
    }

    /// Top and bottom y of a path's points.
    fn y_range(d: &str) -> (f32, f32) {
        let nums: Vec<f32> = d.split(|c: char| c.is_ascii_alphabetic() || c == ' ').filter_map(|n| n.parse().ok()).collect();
        nums.chunks(2).map(|p| p[1]).fold((f32::MAX, f32::MIN), |(a, b), y| (a.min(y), b.max(y)))
    }

    #[test]
    fn filter_envelope_band_runs_from_cutoff_to_peak() {
        for v in filter_mod_cases() {
            let mut labels = Vec::new();
            let d = filter_mod_display(&v, &mut labels);
            let (band_top, band_bot) = y_range(&d.fill);
            let (cut, _) = y_range(&d.marks);
            // The dot's path starts at its left edge, level with its centre.
            let peak: f32 = d.dots.trim_start_matches('M').split(' ').nth(1).unwrap().parse().unwrap();
            let (hi, lo) = (cut.min(peak), cut.max(peak));
            assert!((band_top - hi).abs() < 0.05 && (band_bot - lo).abs() < 0.05, "band {band_top}..{band_bot}, cutoff {cut}, peak {peak}");
        }
    }

    #[test]
    fn filter_envelope_outer_notes_are_dashed() {
        let d = filter_mod_display(&defaults(), &mut Vec::new());
        // Two solid 201-point curves plus many short dashes.
        assert!(d.ghost.matches('M').count() > 40, "expected dashed C2 / C6 curves");
        assert_eq!(dashed(&[(0.0, 0.0), (11.0, 0.0)], 3.0, 2.5), "M0.00 0.00 L3.00 0.00 M5.50 0.00 L8.50 0.00 ");
    }

    #[test]
    fn filter_envelope_time_axis_reads_in_seconds() {
        let mut labels = Vec::new();
        filter_mod_display(&defaults(), &mut labels);
        let ticks: Vec<_> = labels.iter().filter(|l| l.y > DISP_H - 20.0).map(|l| l.text.to_string()).collect();
        assert_eq!(ticks, ["0", "0.5 s", "1 s", "1.5 s"]);
    }

    #[test]
    fn filter_envelope_time_axis_matches_the_curve() {
        for v in filter_mod_cases() {
            let mut labels = Vec::new();
            let d = filter_mod_display(&v, &mut labels);
            let first_x: f32 = d.line.trim_start_matches('M').split(' ').next().unwrap().parse().unwrap();
            let zero = labels.iter().find(|l| l.text == "0").expect("a 0 s tick");
            assert!((zero.x - first_x).abs() < 0.01, "0 s tick at {} but the curve starts at {first_x}", zero.x);
        }
    }

    #[test]
    fn default_routes_and_limits() {
        let v = defaults();
        let rs = routes(&v);
        assert_eq!(rs.len(), 3, "three default routes");
        assert_eq!((rs[0].1, rs[0].2), (sv::source::LFO1, 4));
        // LFO 1 already moves Osc 1 Shape.
        assert!(route_add(&v, sv::source::LFO1, 4).is_err());
        let mut w = v.clone();
        for (i, x) in route_add(&w, sv::source::ENV2, 4).unwrap() {
            w[i] = x;
        }
        for (i, x) in route_add(&w, sv::source::KEY, 4).unwrap() {
            w[i] = x;
        }
        assert_eq!(route_add(&w, sv::source::RANDOM, 4), Err("A Synth knob takes at most 3 modulation sources"));
        let cut = ix("cutoff");
        let mut m = v.clone();
        for (i, x) in map_add(&m, 2, cut).unwrap() {
            m[i] = x;
        }
        let maps = mappings(&m);
        assert_eq!(maps.len(), 1);
        assert_eq!((maps[0].1, maps[0].2), (2, cut));
        assert!((maps[0].4 - maps[0].3 - 0.25).abs() < 1e-6 || (maps[0].3 - maps[0].4 - 0.25).abs() < 1e-6);
        assert!(map_add(&m, 3, cut).is_err(), "one macro per knob");
        for (i, x) in map_clear(maps[0].0) {
            m[i] = x;
        }
        assert!(mappings(&m).is_empty());
        let sub = ix("sub_shape");
        for (i, x) in map_add(&m, 1, sub).unwrap() {
            m[i] = x;
        }
        assert_eq!(mappings(&m)[0].2, sub, "late parameters map through the late list");
    }

    #[test]
    fn modulated_knobs_get_rings_and_dots() {
        let v = defaults();
        let g = layout(&ctx(&v), page::OSC, 1, 0);
        let shape = g.knobs.iter().find(|k| k.idx as usize == ix("osc1_shape")).unwrap();
        assert_eq!(slint::Model::row_count(&shape.dots), 1);
        let fine = g.knobs.iter().find(|k| k.idx as usize == ix("osc1_fine")).unwrap();
        assert_eq!(slint::Model::row_count(&fine.arcs), 0);
    }

    /// cargo test -p dstudio-app --release bench_synth_panel -- --ignored --nocapture
    #[test]
    #[ignore]
    fn bench_synth_panel() {
        let v = defaults();
        let n = 300;
        for (p, page) in PAGES.iter().enumerate() {
            let t0 = std::time::Instant::now();
            for _ in 0..n {
                std::hint::black_box(displays(&v, 120.0, p, 1));
            }
            let pics = t0.elapsed().as_secs_f64() * 1e6 / n as f64;
            let t0 = std::time::Instant::now();
            for _ in 0..n {
                std::hint::black_box(layout(&ctx(&v), p, 1, 0));
            }
            println!("{:<12} pictures {pics:8.1} us  layout {:6.1} us", page, t0.elapsed().as_secs_f64() * 1e6 / n as f64);
        }
    }

    #[test]
    fn idle_effect_settings() {
        assert!(idle_fx_param("fx1_chorus_rate", [0, 3, 7, 8]));
        assert!(!idle_fx_param("fx2_chorus_rate", [0, 3, 7, 8]));
        assert!(!idle_fx_param("fx1_mix", [0, 3, 7, 8]));
        assert!(!idle_fx_param("cutoff", [0; 4]));
    }

    #[test]
    fn filter_response_shape() {
        let lp = |f| filter_db(1, 1000.0, 0.0, 10.0, f);
        assert!(lp(50.0).abs() < 0.5 && lp(10_000.0) < -35.0);
        let hp = |f| filter_db(3, 1000.0, 0.0, 10.0, f);
        assert!(hp(50.0) < -45.0 && hp(10_000.0).abs() < 0.5);
        assert!(filter_db(0, 1000.0, 0.9, 10.0, 1000.0) > 6.0, "ladder resonance peak");
        assert!(filter_db(0, 1000.0, 0.0, 10.0, 10_000.0) < -70.0, "24 dB per octave");
    }

    #[test]
    fn oscillator_shapes_stay_in_range() {
        for w in 0..6 {
            for s in [0.0, 0.5, 1.0] {
                for i in 0..200 {
                    let x = osc_sample(w, s, i as f32 / 200.0, 0.0);
                    // A full-shape Square keeps its DC offset removed, so it reaches 1.9; the
                    // picture clips at 1.2.
                    assert!((-1.95..=1.95).contains(&x), "wave {w} shape {s}: {x}");
                }
            }
        }
        for shape in 0..7 {
            for i in 0..100 {
                assert!(lfo_value(shape, i as f32 / 25.0).abs() <= 1.0001);
            }
        }
    }
}
