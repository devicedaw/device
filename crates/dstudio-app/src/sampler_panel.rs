//! Sampler-family panels: the Sampler (sampler-steam), the Pad Sampler and
//! the Drum Rack (View::Custom "sampler" / "pad-sampler" / "drum-rack").
//!
//! One shared body (ui/device_panel.slint SamplerBody): a waveform well with
//! start / end / fade / loop flags, slices and playheads, the pick rows, a
//! tab bar with a picture canvas (envelopes, filter curve, glide, LFO, slice
//! pads) and the dial row. The Drum Rack adds the 4x4 pad grid and shows the
//! selected pad's sampler as a sub-card. Geometry is in pt at 100%
//! and drawn at [Z].
//!
//! Gestures follow the generic panel: a drag sends live values to the engine
//! and commits one command on release (one undo step); picture handles that
//! move two parameters commit one batch. Samples are loaded from a file
//! picker or an OS file drop (`ImportTarget::DeviceSample`), never from a
//! catalogue.

use crate::controller::{Controller, ImportTarget};
use crate::device_panel::Z;
use crate::{DeviceCard, PanelHandle, PanelKnob, PanelPad, PanelPick, PanelSampler};
use dstudio_core::builtin::{self, DeviceDef, Format, LINEAR, ParamDef, drum_rack, note_name};
use dstudio_core::{Command, Device, DeviceId, DeviceSample, Slice};
use slint::{ModelRc, SharedString, VecModel};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::rc::Rc;

/// Card widths at 100% (pt).
const SAMPLER_W: f32 = 798.0;
const PAD_W: f32 = 599.0;
const RACK_W: f32 = 920.0;
/// Drum rack: pad grid origin, cell, gap; divider; sub-card.
const GRID_X: f32 = 15.0;
const GRID_Y: f32 = 42.0;
const CELL: f32 = 62.5;
const CELL_GAP: f32 = 2.0;
const DIVIDER_X: f32 = 287.0;
const SUB_X: f32 = 305.0;
const SUB_Y: f32 = 43.0;
const SUB_H: f32 = 254.0;
/// Pseudo parameter indices of the selected slice's own values (Slice mode knobs).
pub const SLICE_GAIN: i32 = -101;
pub const SLICE_PAN: i32 = -102;
pub const SLICE_PITCH: i32 = -103;
const TABS: [&str; 5] = ["Sample", "Pitch", "Filter", "Amp", "Mod"];
/// Picker actions (PanelPick kind 6).
const ACT_VIEW: i32 = 1;
const ACT_ADD_SLICE: i32 = 2;
const ACT_CLEAR_SLICE: i32 = 3;

/// Value ranges of a slice's gain, pan and pitch (as definitions, for the knobs).
static SLICE_DEFS: [ParamDef; 3] = [
    ParamDef::new("sliceGain", "Gain", -60.0, 12.0, 0.0, LINEAR, "dB", Format::Db).step(0.1),
    ParamDef::new("slicePan", "Pan", -1.0, 1.0, 0.0, LINEAR, "", Format::Pan),
    ParamDef::new("slicePitch", "Pitch", -24.0, 24.0, 0.0, LINEAR, "st", Format::Signed { decimals: 0, scale: 1 }).step(1.0),
];

fn slice_def(index: i32) -> Option<&'static ParamDef> {
    SLICE_DEFS.get(usize::try_from(SLICE_GAIN - index).ok()?)
}

fn slice_field(s: &Slice, index: i32) -> f32 {
    match index {
        SLICE_GAIN => s.gain,
        SLICE_PAN => s.pan,
        _ => s.pitch,
    }
}

fn set_slice_field(s: &mut Slice, index: i32, v: f32) {
    match index {
        SLICE_GAIN => s.gain = v,
        SLICE_PAN => s.pan = v,
        _ => s.pitch = v,
    }
}

/// What a press in the waveform well grabbed.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Grip {
    /// A flag parameter (start, end, fade in, fade out, loop start) by index.
    Param(usize),
    /// The start of slice n.
    Edge(usize),
    /// Audition (note held while pressed).
    Note(u8),
}

#[derive(Debug, Clone, Copy)]
struct WellDrag {
    device: DeviceId,
    grip: Grip,
    start: f32,
    value: f32,
}

#[derive(Debug, Clone, Copy)]
struct SliceDrag {
    device: DeviceId,
    index: i32,
    start: f32,
    norm: f32,
    value: f32,
    travel: f32,
    active: bool,
}

/// UI state of the sampler panels (not part of the document).
#[derive(Debug, Default)]
pub struct SamplerUi {
    tab: HashMap<DeviceId, usize>,
    /// Drum rack: selected pad (0..16).
    pad: HashMap<DeviceId, u32>,
    /// Pad Sampler / selected rack pad: the layer shown (0..8).
    layer: HashMap<DeviceId, u32>,
    slice: HashMap<DeviceId, usize>,
    /// Filter picture shows the envelope instead of the response curve.
    env_view: HashSet<DeviceId>,
    /// Last click in the well (fraction of the sample), for "Add slice".
    cursor: HashMap<DeviceId, f32>,
    pub(crate) readouts: HashMap<DeviceId, Vec<f32>>,
    held: Vec<(DeviceId, u8)>,
    /// Values of parameters moved by a well or picture drag (index -> value).
    live: HashMap<(DeviceId, usize), f32>,
    /// Picture drag: (device, values before the drag).
    handle_drag: Option<(DeviceId, Vec<(usize, f32)>)>,
    well_drag: Option<WellDrag>,
    slice_drag: Option<SliceDrag>,
    /// Waveform paths by (engine asset, width, height).
    waves: std::cell::RefCell<HashMap<(u64, u32, u32), String>>,
}

fn s(v: impl Into<SharedString>) -> SharedString {
    v.into()
}

fn model<T: Clone + 'static>(v: Vec<T>) -> ModelRc<T> {
    ModelRc::from(Rc::new(VecModel::from(v)))
}

/// Is this one of the sampler-family devices?
/// The layers the arrows step through on `slot`'s pad: its filled layers
/// plus the first free one (to load the next sound into).
fn layer_stops(d: &Device, slot: u32) -> Vec<u32> {
    let layers = builtin::pad_sampler::LAYERS;
    let base = slot - slot % layers;
    let mut stops: Vec<u32> = (0..layers).filter(|l| d.sample(base + l).is_some()).collect();
    if let Some(free) = (0..layers).find(|l| d.sample(base + l).is_none()) {
        stops.push(free);
        stops.sort_unstable();
    }
    stops
}

pub fn is_sampler(def: &DeviceDef) -> bool {
    matches!(def.id, "sampler-steam" | "pad-sampler" | "drum-rack")
}

/// "x.xx s" (seconds format).
fn fmt_secs(v: f32) -> String {
    format!("{v:.2} s")
}

/// Seconds typed as "1.2", "1.2 s" or "350 ms".
fn parse_secs(text: &str) -> Option<f32> {
    let t = text.trim().to_lowercase();
    if let Some(ms) = t.strip_suffix("ms") {
        return ms.trim().parse::<f32>().ok().map(|v| v / 1000.0);
    }
    t.trim_end_matches('s').trim().parse().ok()
}

/// Filled path of a waveform overview from level-0 peak pairs: one column
/// per pixel, max edge left to right then min edge back.
pub fn wave_path(pairs: &[[f32; 2]], w: f32, h: f32) -> String {
    use std::fmt::Write;
    let cols = (w.max(1.0) as usize).clamp(1, 1600);
    if pairs.is_empty() {
        return String::new();
    }
    let mut mins = Vec::with_capacity(cols);
    let mut maxs = Vec::with_capacity(cols);
    for c in 0..cols {
        let a = c * pairs.len() / cols;
        let b = ((c + 1) * pairs.len() / cols).max(a + 1).min(pairs.len());
        let (lo, hi) = pairs[a..b].iter().fold((0f32, 0f32), |(lo, hi), p| (lo.min(p[0]), hi.max(p[1])));
        mins.push(lo.clamp(-1.0, 1.0));
        maxs.push(hi.clamp(-1.0, 1.0));
    }
    let mid = h / 2.0;
    let amp = h / 2.0 * 0.92;
    let x_of = |c: usize| c as f32 * w / cols as f32;
    let mut out = String::with_capacity(cols * 24);
    for (c, v) in maxs.iter().enumerate() {
        let _ = write!(out, "{}{:.1} {:.1} ", if c == 0 { "M" } else { "L" }, x_of(c), mid - v.max(0.004) * amp);
    }
    for (c, v) in mins.iter().enumerate().rev() {
        let _ = write!(out, "L{:.1} {:.1} ", x_of(c) + w / cols as f32, mid - v.min(-0.004) * amp);
    }
    out.push('Z');
    out
}

/// Slice starts for a sample: Beat cuts every `division` (0 = 1/1 .. 5 =
/// 1/32 of a bar of 4/4) at `bpm`; Transient cuts where the level jumps
/// (sensitivity 0..100: higher finds more hits). Always starts at 0.
pub fn chop(
    pairs: &[[f32; 2]],
    frames_per_pair: f64,
    rate: f64,
    frames: u64,
    transient: bool,
    division: u32,
    sensitivity: f32,
    bpm: f64,
) -> Vec<Slice> {
    let mut out = vec![Slice::at(0.0)];
    if frames == 0 {
        return out;
    }
    if !transient {
        let secs = 4.0 / 2f64.powi(division.min(5) as i32) * 60.0 / bpm.max(1.0);
        let step = secs * rate / frames as f64;
        let mut x = step;
        while x < 0.999 && out.len() < dstudio_core::device::MAX_SLICES {
            out.push(Slice::at(x as f32));
            x += step;
        }
        return out;
    }
    let env: Vec<f32> = pairs.iter().map(|p| p[0].abs().max(p[1].abs())).collect();
    let ratio = 1.3 + 3.0 * (1.0 - sensitivity.clamp(0.0, 100.0) / 100.0);
    let floor = 0.02 + 0.1 * (1.0 - sensitivity.clamp(0.0, 100.0) / 100.0);
    let gap = (0.06 * rate / frames_per_pair).max(1.0) as usize;
    let win = (0.02 * rate / frames_per_pair).max(1.0) as usize;
    let mut last = 0usize;
    for i in win..env.len() {
        let before = env[i - win..i].iter().sum::<f32>() / win as f32;
        if env[i] > floor && env[i] > before * ratio && i - last >= gap {
            let at = (i as f64 * frames_per_pair / frames as f64) as f32;
            if at > out.last().map_or(0.0, |s| s.start) && at < 1.0 && out.len() < dstudio_core::device::MAX_SLICES {
                out.push(Slice::at(at));
                last = i;
            }
        }
    }
    out
}

/// Envelope picture geometry in a w x h box: attack, decay and release
/// stages get fixed shares of the width and a stage's time maps through its
/// parameter's taper; the sustain hold zone has a fixed width.
struct EnvBox {
    w: f32,
    h: f32,
}

impl EnvBox {
    const PAD: f32 = 8.0;
    fn stage(&self) -> (f32, f32, f32, f32) {
        let u = self.w - 2.0 * Self::PAD;
        (u * 0.25, u * 0.25, u * 0.18, u * 0.32)
    }
    fn top(&self) -> f32 {
        Self::PAD
    }
    fn bottom(&self) -> f32 {
        self.h - Self::PAD
    }
    fn level_y(&self, level: f32) -> f32 {
        self.bottom() - level.clamp(0.0, 1.0) * (self.bottom() - self.top())
    }
    /// (attack peak x, decay foot x, hold end x, release end x) for normalized times.
    fn xs(&self, a: f32, d: f32, r: f32) -> (f32, f32, f32, f32) {
        let (aw, dw, sw, rw) = self.stage();
        let xa = Self::PAD + a * aw;
        let xd = xa + d * dw;
        let xs = xd + sw;
        (xa, xd, xs, xs + r * rw)
    }
    /// Curve (attack linear, decay and release exponential, fall constant 5).
    fn path(&self, a: f32, d: f32, sus: f32, r: f32) -> (String, String) {
        let (xa, xd, xs, xr) = self.xs(a, d, r);
        let (top, bot, ys) = (self.top(), self.bottom(), self.level_y(sus));
        let mut pts = vec![(Self::PAD, bot), (xa, top)];
        let ex = |x0: f32, x1: f32, y0: f32, y1: f32, pts: &mut Vec<(f32, f32)>| {
            for i in 1..=24 {
                let t = i as f32 / 24.0;
                let k = (1.0 - (-5.0 * t).exp()) / (1.0 - (-5.0f32).exp());
                pts.push((x0 + (x1 - x0) * t, y0 + (y1 - y0) * k));
            }
        };
        ex(xa, xd, top, ys, &mut pts);
        pts.push((xs, ys));
        ex(xs, xr, ys, bot, &mut pts);
        let line = polyline(&pts);
        let fill = format!("{line} L{xr:.1} {bot:.1} L{:.1} {bot:.1} Z", Self::PAD);
        (fill, line)
    }
}

fn polyline(pts: &[(f32, f32)]) -> String {
    use std::fmt::Write;
    let mut out = String::with_capacity(pts.len() * 14);
    for (i, (x, y)) in pts.iter().enumerate() {
        let _ = write!(out, "{}{x:.1} {y:.1} ", if i == 0 { "M" } else { "L" });
    }
    out
}

/// Magnitude (dB) of the filter at f for type 0 LP12, 1 LP24, 2 HP12, 3 BP12.
pub fn filter_db(kind: u32, cutoff: f32, res: f32, f: f32) -> f32 {
    let q = 0.707 + res.clamp(0.0, 1.0) * 11.0;
    let x = f / cutoff.max(1.0);
    let den = ((1.0 - x * x).powi(2) + (x / q).powi(2)).sqrt().max(1e-6);
    let mag = match kind {
        0 => 1.0 / den,
        1 => 1.0 / (den * den),
        2 => x * x / den,
        _ => (x / q) / den,
    };
    20.0 * mag.max(1e-6).log10()
}

impl Controller {
    fn sp(&self) -> &SamplerUi {
        &self.devices.panel.sampler
    }

    fn sp_mut(&mut self) -> &mut SamplerUi {
        &mut self.devices.panel.sampler
    }

    /// The live value of a parameter during a well or picture drag.
    pub(crate) fn sampler_live(&self, device: DeviceId, index: usize) -> Option<f32> {
        self.sp().live.get(&(device, index)).copied()
    }

    /// Parameter id prefix of the pad the panel edits ("p3_" in a rack, "" else).
    fn pad_prefix(&self, d: &Device, def: &DeviceDef) -> String {
        if def.id == "drum-rack" { format!("p{}_", self.sp().pad.get(&d.id).copied().unwrap_or(0) + 1) } else { String::new() }
    }

    /// The sample slot the well shows: Sampler 0; Pad Sampler the shown
    /// layer; Drum Rack the selected pad's shown layer.
    fn shown_slot(&self, d: &Device, def: &DeviceDef) -> u32 {
        let layer = self.sp().layer.get(&d.id).copied().unwrap_or(0);
        match def.id {
            "drum-rack" => self.sp().pad.get(&d.id).copied().unwrap_or(0) * builtin::pad_sampler::LAYERS + layer,
            "pad-sampler" => layer,
            _ => 0,
        }
    }

    fn sample_secs(&self, sample: Option<&DeviceSample>) -> f32 {
        sample.and_then(|s| self.project().asset(s.asset)).map_or(0.0, |a| (a.frames as f64 / a.sample_rate) as f32)
    }

    fn level0(&self, sample: &DeviceSample) -> Option<&[[f32; 2]]> {
        self.level0_of(sample.asset)
    }

    fn level0_of(&self, asset: dstudio_core::AssetId) -> Option<&[[f32; 2]]> {
        let ea = *self.asset_map.get(&asset)?;
        let slot = self.peaks.slot(ea)?;
        self.peaks.data.get(slot.base as usize..(slot.base + slot.len0) as usize)
    }

    /// Slices shown for a sample (a slice edge being dragged included).
    fn shown_slices(&self, device: DeviceId, sample: &DeviceSample) -> Vec<Slice> {
        let mut v = sample.slices.clone();
        if let Some(g) = self.sp().well_drag.filter(|g| g.device == device)
            && let Grip::Edge(i) = g.grip
            && let Some(s) = v.get_mut(i)
        {
            s.start = g.value;
        }
        if let Some(g) = self.sp().slice_drag.filter(|g| g.device == device && g.active)
            && let Some(s) = v.get_mut(self.sp().slice.get(&device).copied().unwrap_or(0))
        {
            set_slice_field(s, g.index, g.value);
        }
        v
    }

    /// Card of a sampler-family device (replaces the generic body).
    pub(crate) fn sampler_card(&self, d: &Device, def: &'static DeviceDef, mut card: DeviceCard) -> DeviceCard {
        let mut sp = PanelSampler { visible: true, ..Default::default() };
        let w = match def.id {
            "sampler-steam" => {
                self.sampler_steam_body(d, def, &mut sp);
                SAMPLER_W
            }
            "drum-rack" => {
                self.rack_body(d, def, &mut sp);
                RACK_W
            }
            _ => {
                sp.kind = 1;
                self.pad_body(d, def, &mut sp, 0.0, 0.0);
                PAD_W
            }
        };
        card.width = w * Z;
        card.kind = s("Instrument");
        card.instrument = true;
        card.sampler = sp;
        card
    }

    fn knob(&self, d: &Device, def: &DeviceDef, id: &str, x: f32, label: Option<&str>) -> Option<PanelKnob> {
        let i = def.index_of(id)?;
        let p = &def.params[i];
        let v = self.panel_value(d, def, p);
        let secs = matches!(p.id.rsplit('_').next(), Some("start" | "end" | "loopStart"))
            .then(|| self.sample_secs(d.sample(self.shown_slot(d, def))));
        Some(PanelKnob {
            param: i as i32,
            x: x * Z,
            label: s(label.unwrap_or(p.label)),
            value: s(secs.map_or_else(|| p.format(v), |t| fmt_secs(v * t))),
            norm: p.to_normalized(v),
            origin: p.origin(),
            ..Default::default()
        })
    }

    fn pick(&self, d: &Device, def: &DeviceDef, kind: i32, id: &str, label: Option<&str>, width: f32) -> Option<PanelPick> {
        let i = def.index_of(id)?;
        let p = &def.params[i];
        let v = self.panel_value(d, def, p);
        Some(PanelPick {
            kind,
            param: i as i32,
            label: s(label.unwrap_or(p.label)),
            choices: model(p.choices.iter().map(|c| s(*c)).collect()),
            selected: v.round() as i32,
            value: s(p.format(v)),
            enabled: true,
            width: width * Z,
            ..Default::default()
        })
    }

    fn action(&self, id: i32, label: &str) -> PanelPick {
        PanelPick { kind: 6, param: id, label: s(label), enabled: true, ..Default::default() }
    }

    /// Waveform well for `slot` with the flags of the parameters named by `prefix`.
    fn well(
        &self,
        d: &Device,
        def: &DeviceDef,
        sp: &mut PanelSampler,
        slot: u32,
        prefix: &str,
        (x, y, w, h): (f32, f32, f32, f32),
        slices: bool,
    ) {
        let (w, h) = (w * Z, h * Z);
        sp.well_x = x * Z;
        sp.well_y = y * Z;
        sp.well_w = w;
        sp.well_h = h;
        let sample = d.sample(slot);
        let asset = sample.and_then(|s| self.project().asset(s.asset));
        sp.empty_text = s("Drop a sample here or click to load");
        sp.hint = s("From Finder or the file picker");
        let (Some(sample), Some(_)) = (sample, asset) else { return };
        sp.loaded = true;
        sp.sample_name = s(&sample.name);
        if let Some(ea) = self.asset_map.get(&sample.asset) {
            let key = (*ea, w as u32, h as u32);
            let cached = self.sp().waves.borrow().get(&key).cloned();
            let path = cached.unwrap_or_else(|| {
                let p = self.level0(sample).map(|l0| wave_path(l0, w, h)).unwrap_or_default();
                let mut waves = self.sp().waves.borrow_mut();
                if waves.len() > 32 {
                    waves.clear();
                }
                waves.insert(key, p.clone());
                p
            });
            sp.wave = s(path);
        }
        let val = |id: &str| def.index_of(&format!("{prefix}{id}")).map(|i| (i, self.panel_value(d, def, &def.params[i])));
        let (Some((si, a)), Some((ei, b))) = (val("start"), val("end")) else { return };
        let (a, b) = (a.min(b), a.max(b));
        let fi = val("fadeIn").map_or(0.0, |v| v.1);
        let fo = val("fadeOut").map_or(0.0, |v| v.1);
        let (ax, bx) = (a * w, b * w);
        let fix = (a + (b - a) * fi / 100.0) * w;
        let fox = (b - (b - a) * fo / 100.0) * w;
        sp.start_x = ax;
        sp.end_x = bx;
        sp.fade_in_x = fix;
        sp.fade_out_x = fox;
        sp.dim = s(format!("M0 0 L{ax:.1} 0 L{ax:.1} {h:.1} L0 {h:.1} Z M{bx:.1} 0 L{w:.1} 0 L{w:.1} {h:.1} L{bx:.1} {h:.1} Z"));
        sp.fades = s(format!("M{ax:.1} 0 L{fix:.1} 0 L{ax:.1} {h:.1} Z M{fox:.1} 0 L{bx:.1} 0 L{bx:.1} {h:.1} Z"));
        let mut grips = vec![ax, bx, fix, fox];
        let _ = (si, ei);
        sp.loop_x = -1.0;
        if def.id == "sampler-steam"
            && self.choice(d, def, "mode") == 0
            && self.choice(d, def, "loop") == 1
            && let Some((_, l)) = val("loopStart")
        {
            sp.loop_x = l.clamp(a, b) * w;
            grips.push(sp.loop_x);
        }
        if slices {
            let v = self.shown_slices(d.id, sample);
            let sel = self.sp().slice.get(&d.id).copied().unwrap_or(0).min(v.len().saturating_sub(1));
            let mut path = String::new();
            for sl in &v {
                let sx = sl.start * w;
                path.push_str(&format!("M{sx:.1} 0 L{sx:.1} {h:.1} "));
                grips.push(sx);
            }
            sp.slices = s(path);
            if let Some(sl) = v.get(sel) {
                sp.sel_x = sl.start * w;
                sp.sel_w = (v.get(sel + 1).map_or(1.0, |n| n.start) - sl.start) * w;
            }
        }
        sp.grips = model(grips);
        // Playheads (engine readouts, fractions of the sample).
        let ro = self.sp().readouts.get(&d.id);
        let heads: Vec<f32> = match def.id {
            "sampler-steam" => ro.map(|r| r.iter().copied().filter(|p| *p >= 0.0).collect()).unwrap_or_default(),
            "pad-sampler" => ro.filter(|r| r.len() >= 3 && r[2] as i32 == slot as i32).map(|r| vec![r[1]]).unwrap_or_default(),
            _ => {
                let pad = (slot / builtin::pad_sampler::LAYERS) as usize;
                let layer = slot % builtin::pad_sampler::LAYERS;
                ro.and_then(|r| r.get(3 * pad..3 * pad + 3)).filter(|r| r[2] as i32 == layer as i32).map(|r| vec![r[1]]).unwrap_or_default()
            }
        };
        sp.heads = s(heads
            .iter()
            .filter(|p| (0.0..=1.0).contains(*p))
            .map(|p| format!("M{:.1} 0 L{:.1} {h:.1} ", p * w, p * w))
            .collect::<String>());
    }

    fn choice(&self, d: &Device, def: &DeviceDef, id: &str) -> i32 {
        def.param(id).map_or(0, |p| self.panel_value(d, def, p).round() as i32)
    }

    fn sampler_steam_body(&self, d: &Device, def: &DeviceDef, sp: &mut PanelSampler) {
        let mode = self.choice(d, def, "mode");
        let tab = self.sp().tab.get(&d.id).copied().unwrap_or(0);
        sp.kind = 0;
        // Header: mode switch and warp.
        sp.head_x = 120.0 * Z;
        sp.head_picks =
            model([self.pick(d, def, 1, "mode", Some(""), 0.0), self.pick(d, def, 2, "warp", None, 96.0)].into_iter().flatten().collect());
        // Row 2: Play button; play mode, voicing, voices.
        sp.play_visible = true;
        sp.play_x = 7.0 * Z;
        sp.row_y = 41.0 * Z;
        sp.row_right = 790.0 * Z;
        let mut row = Vec::new();
        if mode != 0 {
            row.extend(self.pick(d, def, 2, "playMode", Some("Play"), 84.0));
        }
        row.extend(self.pick(d, def, 1, "voicing", Some(""), 0.0));
        if self.choice(d, def, "voicing") == 1 {
            row.extend(self.pick(d, def, 3, "voices", None, 40.0));
        }
        sp.row_picks = model(row);
        self.well(d, def, sp, 0, "", (7.0, 71.0, 783.0, 103.0), mode == 2);
        sp.can_remove = sp.loaded;
        // Tab bar and the tab's controls.
        sp.tabs_visible = true;
        sp.tabs_y = 183.0 * Z;
        sp.tab_pick = PanelPick {
            kind: 7,
            param: -1,
            choices: model(TABS.iter().map(|t| s(*t)).collect()),
            selected: tab as i32,
            enabled: true,
            ..Default::default()
        };
        sp.tab_right = 790.0 * Z;
        let kx = |i: usize| 7.0 + i as f32 * 72.0;
        let mut knobs = Vec::new();
        let mut picks = Vec::new();
        let mut k = |id: &str, label: Option<&str>| {
            let x = kx(knobs.len());
            knobs.extend(self.knob(d, def, id, x, label));
        };
        match tab {
            0 if mode == 2 => {
                let sample = d.sample(0);
                let sel = self.sp().slice.get(&d.id).copied().unwrap_or(0);
                let slices = sample.map(|s| self.shown_slices(d.id, s)).unwrap_or_default();
                for (n, index) in [SLICE_GAIN, SLICE_PAN, SLICE_PITCH].into_iter().enumerate() {
                    let p = slice_def(index).expect("slice def");
                    let v = match self.sp().slice_drag.filter(|g| g.device == d.id && g.index == index) {
                        Some(g) => g.value,
                        None => slices.get(sel).map_or(p.default, |sl| slice_field(sl, index)),
                    };
                    knobs.push(PanelKnob {
                        param: index,
                        x: kx(n) * Z,
                        label: s(p.label),
                        value: s(p.format(v)),
                        norm: p.to_normalized(v),
                        origin: p.origin(),
                        ..Default::default()
                    });
                }
                knobs.extend(self.knob(d, def, "sliceKey", kx(3), Some("Key")));
                picks.extend(self.pick(d, def, 2, "chop", None, 84.0));
                if self.choice(d, def, "chop") == 0 {
                    picks.extend(self.pick(d, def, 2, "division", None, 56.0));
                } else {
                    picks.extend(self.pick(d, def, 3, "sensitivity", None, 48.0));
                }
                picks.push(self.action(ACT_ADD_SLICE, "Add slice here"));
                picks.push(self.action(ACT_CLEAR_SLICE, "Clear slice"));
                picks.extend(self.pick(d, def, 5, "reverse", None, 0.0));
            }
            0 if mode == 0 => {
                for (id, l) in [("root", None), ("volume", None), ("pan", None), ("transpose", None)] {
                    k(id, l);
                }
                picks.extend(self.pick(d, def, 5, "loop", None, 0.0));
                picks.extend(self.pick(d, def, 5, "reverse", None, 0.0));
            }
            0 => {
                for id in ["start", "end", "fadeIn", "fadeOut"] {
                    k(id, None);
                }
                picks.extend(self.pick(d, def, 5, "reverse", None, 0.0));
            }
            1 => {
                for (id, l) in [("transpose", Some("Coarse")), ("fineTune", None), ("glide", None), ("bendRange", None)] {
                    k(id, l);
                }
                if self.choice(d, def, "voicing") == 0 {
                    picks.extend(self.pick(d, def, 5, "legato", None, 0.0));
                }
            }
            2 => {
                for id in ["cutoff", "resonance", "drive", "keyTrack"] {
                    k(id, None);
                }
                picks.extend(self.pick(d, def, 5, "filterOn", None, 0.0));
                picks.extend(self.pick(d, def, 2, "filterType", None, 62.0));
                picks.extend(self.pick(d, def, 3, "envDepth", None, 48.0));
                picks.push(self.action(ACT_VIEW, if self.sp().env_view.contains(&d.id) { "Env" } else { "Curve" }));
            }
            3 => {
                for id in ["ampAttack", "ampDecay", "ampSustain", "ampRelease"] {
                    k(id, None);
                }
                for id in ["volume", "pan", "velocity"] {
                    picks.extend(self.pick(d, def, 3, id, None, 48.0));
                }
            }
            _ => {
                for id in ["lfoRate", "lfoAmount", "lfoFadeIn", "lfoPhase"] {
                    k(id, None);
                }
                picks.extend(self.pick(d, def, 2, "lfoShape", None, 70.0));
                picks.extend(self.pick(d, def, 2, "lfoTarget", None, 70.0));
                picks.extend(self.pick(d, def, 5, "lfoKeyTrigger", Some("Key Trig"), 0.0));
            }
        }
        sp.knobs = model(knobs);
        sp.tab_picks = model(picks);
        sp.knob_y = 229.0 * Z;
        sp.pic_visible = true;
        sp.pic_x = 305.0 * Z;
        sp.pic_y = 217.0 * Z;
        sp.pic_w = 485.0 * Z;
        sp.pic_h = 87.0 * Z;
        self.picture(d, def, sp, tab, mode);
    }

    /// Picture canvas of the active tab.
    fn picture(&self, d: &Device, def: &DeviceDef, sp: &mut PanelSampler, tab: usize, mode: i32) {
        let (w, h) = (sp.pic_w, sp.pic_h);
        let n = |id: &str| def.param(id).map_or(0.0, |p| p.to_normalized(self.panel_value(d, def, p)));
        let v = |id: &str| def.param(id).map_or(0.0, |p| self.panel_value(d, def, p));
        let mut handles = Vec::new();
        let mut grid = String::new();
        for i in 1..4 {
            let y = h * i as f32 / 4.0;
            grid.push_str(&format!("M0 {y:.1} L{w:.1} {y:.1} "));
        }
        sp.pic_grid = s(grid);
        let (fill, line) = match tab {
            0 if mode == 2 => {
                // Slice pads: 2 rows of 8 from the slice key.
                let key = v("sliceKey") as i32;
                let count = d.sample(0).map_or(0, |s| s.slices.len().max(1));
                let sel = self.sp().slice.get(&d.id).copied().unwrap_or(0);
                let (pw, ph) = ((w - 7.0 * 3.0 * Z - 12.0 * Z) / 8.0, (h - 3.0 * Z - 12.0 * Z) / 2.0);
                let pads = (0..16)
                    .map(|i| PanelPad {
                        label: s(note_name(key + i as i32)),
                        name: s(if i < count { format!("{}", i + 1) } else { String::new() }),
                        filled: i < count,
                        selected: i == sel && i < count,
                        level: 0.0,
                        x: 6.0 * Z + (i % 8) as f32 * (pw + 3.0 * Z),
                        y: 6.0 * Z + (i / 8) as f32 * (ph + 3.0 * Z),
                        w: pw,
                        h: ph,
                    })
                    .collect();
                sp.pic_pads = model(pads);
                (String::new(), String::new())
            }
            0 => (String::new(), String::new()),
            1 => {
                // Glide: from the previous note up to the new one; the handle sets the time.
                let g = n("glide");
                let x1 = 8.0 + g * (w - 16.0);
                let (y0, y1) = (h * 0.78, h * 0.22);
                // An S-curve (cubic, both controls at the horizontal middle).
                let mx = 8.0 + (x1 - 8.0) * 0.5;
                handles.push(PanelHandle { x: x1, y: y1, active: g > 0.0 });
                let line = format!("M0 {y0:.1} L8 {y0:.1} C{mx:.1} {y0:.1} {mx:.1} {y1:.1} {x1:.1} {y1:.1} L{w:.1} {y1:.1}");
                (format!("{line} L{w:.1} {h:.1} L0 {h:.1} Z"), line)
            }
            2 if !self.sp().env_view.contains(&d.id) => {
                let (kind, cut, res) = (v("filterType") as u32, v("cutoff"), v("resonance"));
                let x_of = |f: f32| (f / 20.0).log10() / 3.0 * w;
                let y_of = |db: f32| (h * 0.5 - db / 24.0 * h * 0.45).clamp(0.0, h);
                let pts: Vec<(f32, f32)> = (0..=96)
                    .map(|i| {
                        let f = 20.0 * 1000f32.powf(i as f32 / 96.0);
                        (x_of(f), y_of(filter_db(kind, cut, res, f)))
                    })
                    .collect();
                handles.push(PanelHandle { x: x_of(cut), y: y_of(res * 24.0), active: true });
                let line = polyline(&pts);
                (format!("{line} L{w:.1} {h:.1} L0 {h:.1} Z"), line)
            }
            2 | 3 => {
                let pre = if tab == 2 { "filter" } else { "amp" };
                let e = EnvBox { w, h };
                let (a, dd, sus, r) =
                    (n(&format!("{pre}Attack")), n(&format!("{pre}Decay")), v(&format!("{pre}Sustain")), n(&format!("{pre}Release")));
                let (xa, xd, xs, xr) = e.xs(a, dd, r);
                let ys = e.level_y(sus);
                handles.push(PanelHandle { x: xa, y: e.top(), active: true });
                handles.push(PanelHandle { x: xd, y: ys, active: true });
                handles.push(PanelHandle { x: xr, y: e.bottom(), active: true });
                handles.push(PanelHandle { x: (xd + xs) / 2.0, y: ys, active: false });
                e.path(a, dd, sus, r)
            }
            _ => {
                let (shape, rate, amount, fade, phase) =
                    (v("lfoShape") as u32, v("lfoRate"), v("lfoAmount"), v("lfoFadeIn") / 1000.0, v("lfoPhase"));
                let span = 3.0 / rate.max(0.05);
                let pts: Vec<(f32, f32)> = (0..=192)
                    .map(|i| {
                        let t = i as f32 / 192.0 * span;
                        let ph = (t * rate + phase).fract();
                        let y = match shape {
                            0 => (ph * std::f32::consts::TAU).sin(),
                            1 => 1.0 - 4.0 * (ph - 0.5).abs(),
                            2 => {
                                if ph < 0.5 {
                                    1.0
                                } else {
                                    -1.0
                                }
                            }
                            _ => 1.0 - 2.0 * ph,
                        };
                        let env = if fade > 0.0 { (t / fade).min(1.0) } else { 1.0 };
                        (i as f32 / 192.0 * w, h / 2.0 - y * env * amount.max(0.04) * h * 0.42)
                    })
                    .collect();
                (String::new(), polyline(&pts))
            }
        };
        sp.pic_fill = s(fill);
        sp.pic_line = s(line);
        sp.handles = model(handles);
    }

    /// Pad sampler controls with the sub-card origin at (ox, oy) (pt).
    fn pad_body(&self, d: &Device, def: &DeviceDef, sp: &mut PanelSampler, ox: f32, oy: f32) {
        let prefix = self.pad_prefix(d, def);
        let slot = self.shown_slot(d, def);
        let base = slot - slot % builtin::pad_sampler::LAYERS;
        let filled = (base..base + builtin::pad_sampler::LAYERS).filter(|s| d.sample(*s).is_some()).count() as u32;
        let shown = layer_stops(d, slot).iter().position(|l| *l == slot % builtin::pad_sampler::LAYERS).unwrap_or(0) as u32;
        let row_y = if def.id == "drum-rack" { oy + 33.0 } else { 41.0 };
        sp.layer_visible = true;
        sp.layer_x = (ox + 12.0) * Z;
        sp.layer_y = row_y * Z;
        sp.layer_text = s(format!("{}/{}", shown + 1, filled.max(shown + 1)));
        sp.can_step = filled > 0;
        sp.layer_name = s(if def.id == "drum-rack" {
            format!("Pad {}", base / builtin::pad_sampler::LAYERS + 1)
        } else {
            d.sample(slot).map_or_else(|| "Pad".to_string(), |s| s.name.clone())
        });
        sp.mapped = s(if def.id == "drum-rack" {
            format!("Mapped to {}", note_name((drum_rack::FIRST_NOTE as u32 + base / builtin::pad_sampler::LAYERS) as i32))
        } else {
            "Mapped to all keys".to_string()
        });
        sp.row_y = row_y * Z;
        sp.row_right = (ox + 589.0) * Z;
        let id = |s: &str| format!("{prefix}{s}");
        sp.row_picks = model(
            [
                self.pick(d, def, 2, &id("warp"), None, 96.0),
                self.pick(d, def, 5, &id("reverse"), None, 0.0),
                self.pick(d, def, 5, &id("gate"), None, 0.0),
            ]
            .into_iter()
            .flatten()
            .collect(),
        );
        let (wy, wh) = if def.id == "drum-rack" { (oy + 65.0, 101.0) } else { (71.0, 103.0) };
        self.well(d, def, sp, slot, &prefix, (ox + 10.0, wy, 579.0, wh), false);
        sp.can_remove = sp.loaded;
        let knobs = ["gain", "pan", "transpose", "fadeIn", "fadeOut"]
            .iter()
            .enumerate()
            .filter_map(|(i, k)| self.knob(d, def, &id(k), ox + 56.0 + i as f32 * 121.75 - 31.0, None))
            .collect();
        sp.knobs = model(knobs);
        sp.knob_y = if def.id == "drum-rack" { (oy + 178.0) * Z } else { 229.0 * Z };
    }

    fn rack_body(&self, d: &Device, def: &DeviceDef, sp: &mut PanelSampler) {
        sp.kind = 2;
        let sel = self.sp().pad.get(&d.id).copied().unwrap_or(0);
        let ro = self.sp().readouts.get(&d.id);
        let layers = builtin::pad_sampler::LAYERS;
        let pads = (0..drum_rack::PADS)
            .map(|i| {
                let (col, row) = ((i % 4) as f32, (3 - i / 4) as f32);
                let first = (i * layers..(i + 1) * layers).find_map(|s| d.sample(s));
                PanelPad {
                    label: s(note_name(drum_rack::FIRST_NOTE as i32 + i as i32)),
                    name: s(first.map_or("", |s| s.name.as_str())),
                    filled: first.is_some(),
                    selected: i == sel,
                    level: ro.and_then(|r| r.get(3 * i as usize)).copied().unwrap_or(0.0).clamp(0.0, 1.0),
                    x: (GRID_X + col * (CELL + CELL_GAP)) * Z,
                    y: (GRID_Y + row * (CELL + CELL_GAP)) * Z,
                    w: CELL * Z,
                    h: CELL * Z,
                }
            })
            .collect();
        sp.pads = model(pads);
        sp.divider_x = DIVIDER_X * Z;
        sp.sub_visible = true;
        sp.sub_x = SUB_X * Z;
        sp.sub_y = SUB_Y * Z;
        sp.sub_w = PAD_W * Z;
        sp.sub_h = SUB_H * Z;
        sp.sub_title = s("Sampler");
        if let Some(i) = def.index_of(&format!("p{}_on", sel + 1)) {
            sp.sub_on_param = i as i32;
            sp.sub_on = self.panel_value(d, def, &def.params[i]) >= 0.5;
        }
        self.pad_body(d, def, sp, SUB_X, SUB_Y);
    }

    // ---------------------------------------------------------------- engine readouts

    /// Readouts of the selected track's sampler-family instrument (poll tick).
    pub(crate) fn sampler_poll(&mut self) {
        let Some(d) = self.selected_track.and_then(|t| self.project().track(t)).and_then(|t| t.instrument_plugin.as_ref()) else {
            self.sp_mut().readouts.clear();
            return;
        };
        if !builtin::def_of(d).is_some_and(is_sampler) {
            return;
        }
        let id = d.id;
        let mut out = Vec::new();
        if self.engine.builtin_readouts(id.0, &mut out).is_err() {
            return;
        }
        if self.sp().readouts.get(&id) != Some(&out) {
            self.sp_mut().readouts.insert(id, out);
            self.ui_dirty = true;
        }
    }

    // ---------------------------------------------------------------- gestures

    pub(crate) fn sampler_def(&self, device: DeviceId) -> Option<(&Device, &'static DeviceDef)> {
        let d = self.project().device(device)?;
        let def = builtin::def_of(d).filter(|def| is_sampler(def))?;
        Some((d, def))
    }

    pub fn sampler_tab(&mut self, device: DeviceId, tab: i32) {
        self.sp_mut().tab.insert(device, tab.clamp(0, 4) as usize);
        self.ui_dirty = true;
    }

    fn note(&mut self, device: DeviceId, note: u8, down: bool) {
        if down {
            if self.engine.builtin_note(device.0, note, 110).is_ok() {
                self.sp_mut().held.push((device, note));
            }
        } else {
            let held = std::mem::take(&mut self.sp_mut().held);
            for (d, n) in held {
                if d == device && n == note {
                    let _ = self.engine.builtin_note(d.0, n, 0);
                } else {
                    self.sp_mut().held.push((d, n));
                }
            }
        }
    }

    /// Release every note a panel gesture holds (pointer cancel, focus loss).
    pub fn sampler_release_all(&mut self) {
        for (d, n) in std::mem::take(&mut self.sp_mut().held) {
            let _ = self.engine.builtin_note(d.0, n, 0);
        }
    }

    /// Pad press / release: which 0 = drum rack pad (selects it), 1 = slice pad.
    pub fn sampler_pad(&mut self, device: DeviceId, which: i32, i: i32, down: bool) {
        let Some((d, def)) = self.sampler_def(device) else { return };
        let i = i.clamp(0, 15) as u32;
        let note = if which == 0 {
            drum_rack::FIRST_NOTE as u32 + i
        } else {
            let key = def.param("sliceKey").map_or(60.0, |p| def.value(d, p)) as u32;
            let n = d.sample(0).map_or(0, |s| s.slices.len());
            if down && (i as usize) < n.max(1) {
                self.sp_mut().slice.insert(device, i as usize);
            }
            key + i
        };
        if down && which == 0 && self.sp().pad.get(&device) != Some(&i) {
            self.sp_mut().pad.insert(device, i);
            self.sp_mut().layer.insert(device, 0);
        }
        self.ui_dirty = true;
        self.note(device, note.min(127) as u8, down);
    }

    pub fn sampler_play(&mut self, device: DeviceId, down: bool) {
        let Some((d, def)) = self.sampler_def(device) else { return };
        let v = |id: &str| def.param(id).map_or(60.0, |p| def.value(d, p));
        let note =
            if v("mode") == 2.0 { v("sliceKey") as usize + self.sp().slice.get(&device).copied().unwrap_or(0) } else { v("root") as usize };
        self.note(device, note.min(127) as u8, down);
    }

    pub fn sampler_layer(&mut self, device: DeviceId, dir: i32) {
        let Some((d, def)) = self.sampler_def(device) else { return };
        let slot = self.shown_slot(d, def);
        let stops = layer_stops(d, slot);
        let at = stops.iter().position(|l| *l == slot % builtin::pad_sampler::LAYERS).unwrap_or(0) as i32;
        let next = stops[(at + dir).rem_euclid(stops.len() as i32) as usize];
        self.sp_mut().layer.insert(device, next);
        self.ui_dirty = true;
    }

    pub fn sampler_remove(&mut self, device: DeviceId) {
        let Some((d, def)) = self.sampler_def(device) else { return };
        let slot = self.shown_slot(d, def);
        if d.sample(slot).is_some() {
            self.execute(Command::SetDeviceSample { device, slot, sample: None });
        }
    }

    pub fn sampler_action(&mut self, device: DeviceId, action: i32) {
        let Some((d, _)) = self.sampler_def(device) else { return };
        match action {
            ACT_VIEW => {
                let ui = self.sp_mut();
                if !ui.env_view.remove(&device) {
                    ui.env_view.insert(device);
                }
                self.ui_dirty = true;
            }
            ACT_ADD_SLICE | ACT_CLEAR_SLICE => {
                let Some(sample) = d.sample(0).cloned() else { return self.set_status("Load a sample first", 3) };
                let mut slices = sample.slices.clone();
                if slices.is_empty() {
                    slices.push(Slice::at(0.0));
                }
                if action == ACT_ADD_SLICE {
                    let at = self.sp().cursor.get(&device).copied().unwrap_or(0.5).clamp(0.0, 0.999);
                    if slices.len() >= dstudio_core::device::MAX_SLICES || slices.iter().any(|s| (s.start - at).abs() < 1e-4) {
                        return;
                    }
                    let i = slices.partition_point(|s| s.start < at);
                    slices.insert(i, Slice::at(at));
                    self.sp_mut().slice.insert(device, i);
                } else {
                    let i = self.sp().slice.get(&device).copied().unwrap_or(0);
                    if i == 0 || i >= slices.len() {
                        return;
                    }
                    slices.remove(i);
                    self.sp_mut().slice.insert(device, i.saturating_sub(1));
                }
                self.execute(Command::SetDeviceSample { device, slot: 0, sample: Some(DeviceSample { slices, ..sample }) });
            }
            _ => {}
        }
    }

    /// Press / drag / release in the waveform well at `frac` of its width;
    /// `tol` is the grab distance as a fraction. Grabs the nearest flag or
    /// slice edge, else selects and auditions (slice mode, pads).
    pub fn sampler_well(&mut self, device: DeviceId, phase: i32, frac: f32, tol: f32, alt: bool) {
        let Some((d, def)) = self.sampler_def(device).map(|(d, def)| (d.clone(), def)) else { return };
        let d = &d;
        let frac = frac.clamp(0.0, 1.0);
        match phase {
            0 => {
                let prefix = self.pad_prefix(d, def);
                let slot = self.shown_slot(d, def);
                let mode = if def.id == "sampler-steam" { self.choice(d, def, "mode") } else { 1 };
                let mut grips: Vec<(Grip, f32)> = Vec::new();
                let idx = |id: &str| def.index_of(&format!("{prefix}{id}"));
                let val = |i: usize| self.panel_value(d, def, &def.params[i]);
                if let (Some(si), Some(ei)) = (idx("start"), idx("end")) {
                    let (a, b) = (val(si), val(ei));
                    grips.push((Grip::Param(si), a));
                    grips.push((Grip::Param(ei), b));
                    if let Some(i) = idx("fadeIn") {
                        grips.push((Grip::Param(i), a + (b - a) * val(i) / 100.0));
                    }
                    if let Some(i) = idx("fadeOut") {
                        grips.push((Grip::Param(i), b - (b - a) * val(i) / 100.0));
                    }
                    if mode == 0
                        && self.choice(d, def, "loop") == 1
                        && let Some(i) = idx("loopStart")
                    {
                        grips.push((Grip::Param(i), val(i).clamp(a, b)));
                    }
                }
                if mode == 2
                    && let Some(s) = d.sample(0)
                {
                    grips.extend(s.slices.iter().enumerate().skip(1).map(|(i, sl)| (Grip::Edge(i), sl.start)));
                }
                // Flags at the same spot (a fade or the loop start on a region edge): the
                // edge from outside the region or on it, the inner flag from inside.
                let (si, ei) = (idx("start"), idx("end"));
                let (a, b) = (si.map_or(0.0, val), ei.map_or(1.0, val));
                let wrong_side = |g: Grip| match g {
                    Grip::Param(i) if Some(i) == si => frac > a,
                    Grip::Param(i) if Some(i) == ei => frac < b,
                    Grip::Param(_) => !(a < frac && frac < b),
                    _ => false,
                };
                let near = grips
                    .iter()
                    .filter(|(_, x)| (x - frac).abs() <= tol)
                    .min_by(|p, q| (p.1 - frac).abs().total_cmp(&(q.1 - frac).abs()).then(wrong_side(p.0).cmp(&wrong_side(q.0))))
                    .copied();
                let start_of = |g: Grip, x: f32| match g {
                    Grip::Param(i) => val(i),
                    _ => x,
                };
                let near = near.map(|(g, x)| (g, x, start_of(g, x)));
                self.sp_mut().cursor.insert(device, frac);
                match near {
                    Some((Grip::Param(i), _, _)) if alt => self.panel_reset(device, i as i32),
                    Some((grip, _, start)) => self.sp_mut().well_drag = Some(WellDrag { device, grip, start, value: start }),
                    None => {
                        let note = if mode == 2 {
                            let slices = d.sample(0).map(|s| s.slices.clone()).unwrap_or_default();
                            let i = slices.partition_point(|s| s.start <= frac).saturating_sub(1);
                            self.sp_mut().slice.insert(device, i);
                            def.param("sliceKey").map_or(60.0, |p| def.value(d, p)) as usize + i
                        } else if def.id == "drum-rack" {
                            drum_rack::FIRST_NOTE as usize + (slot / builtin::pad_sampler::LAYERS) as usize
                        } else if def.id == "pad-sampler" {
                            60
                        } else {
                            def.param("root").map_or(60.0, |p| def.value(d, p)) as usize
                        };
                        let note = note.min(127) as u8;
                        self.sp_mut().well_drag = Some(WellDrag { device, grip: Grip::Note(note), start: 0.0, value: 0.0 });
                        self.note(device, note, true);
                    }
                }
                self.ui_dirty = true;
            }
            1 => {
                let Some(g) = self.sp().well_drag.filter(|g| g.device == device) else { return };
                let v = match g.grip {
                    Grip::Param(i) => {
                        let prefix = self.pad_prefix(d, def);
                        let get = |id: &str| def.index_of(&format!("{prefix}{id}")).map(|j| self.panel_value(d, def, &def.params[j]));
                        let (a, b) = (get("start").unwrap_or(0.0), get("end").unwrap_or(1.0));
                        let id = def.params[i].id.rsplit('_').next().unwrap_or("");
                        let span = (b - a).max(1e-4);
                        match id {
                            "start" => frac.min(b - 0.001),
                            "end" => frac.max(a + 0.001),
                            "fadeIn" => ((frac - a) / span * 100.0).clamp(0.0, 100.0),
                            "fadeOut" => ((b - frac) / span * 100.0).clamp(0.0, 100.0),
                            _ => frac.clamp(a, b),
                        }
                    }
                    Grip::Edge(i) => {
                        let sl = d.sample(0).map(|s| s.slices.clone()).unwrap_or_default();
                        let lo = sl.get(i.wrapping_sub(1)).map_or(0.0, |s| s.start) + 0.001;
                        let hi = sl.get(i + 1).map_or(0.999, |s| s.start - 0.001);
                        frac.clamp(lo, hi.max(lo))
                    }
                    Grip::Note(_) => return,
                };
                if let Grip::Param(i) = g.grip {
                    let v = def.params[i].snap(v);
                    let _ = self.engine.builtin_set_param(device.0, i as u32, v);
                    self.sp_mut().live.insert((device, i), v);
                }
                if let Some(w) = self.sp_mut().well_drag.as_mut() {
                    w.value = v;
                }
                self.ui_dirty = true;
            }
            _ => {
                let Some(g) = self.sp_mut().well_drag.take() else { return };
                self.ui_dirty = true;
                match g.grip {
                    Grip::Note(n) => self.note(device, n, false),
                    Grip::Param(i) => {
                        self.sp_mut().live.remove(&(device, i));
                        let p = &def.params[i];
                        if phase == 2 && g.value != g.start {
                            self.panel_commit(Command::SetDeviceParam { device, param: p.id.to_string(), value: p.snap(g.value) });
                        } else {
                            let _ = self.engine.builtin_set_param(device.0, i as u32, g.start);
                        }
                    }
                    Grip::Edge(i) => {
                        if phase == 2
                            && g.value != g.start
                            && let Some(mut sample) = d.sample(0).cloned()
                        {
                            if let Some(s) = sample.slices.get_mut(i) {
                                s.start = g.value;
                            }
                            self.execute(Command::SetDeviceSample { device, slot: 0, sample: Some(sample) });
                        }
                    }
                }
            }
        }
    }

    /// Params a picture handle moves: (x param, y param) indices.
    fn handle_params(&self, d: &Device, def: &DeviceDef, i: i32) -> (Option<usize>, Option<usize>, &'static str) {
        let tab = self.sp().tab.get(&d.id).copied().unwrap_or(0);
        let idx = |id: &str| def.index_of(id);
        match (tab, i) {
            (1, 0) => (idx("glide"), None, "Glide Change"),
            (2, 0) if !self.sp().env_view.contains(&d.id) => (idx("cutoff"), idx("resonance"), "Filter Change"),
            (2 | 3, _) => {
                let pre = if tab == 2 { "filter" } else { "amp" };
                let label = if tab == 2 { "Filter Envelope" } else { "Amp Envelope" };
                match i {
                    0 => (idx(&format!("{pre}Attack")), None, label),
                    1 => (idx(&format!("{pre}Decay")), idx(&format!("{pre}Sustain")), label),
                    2 => (idx(&format!("{pre}Release")), None, label),
                    _ => (None, idx(&format!("{pre}Sustain")), label),
                }
            }
            _ => (None, None, ""),
        }
    }

    /// Drag of picture handle `i` to (x, y) in picture coordinates (px).
    pub fn sampler_handle(&mut self, device: DeviceId, i: i32, phase: i32, x: f32, y: f32) {
        let Some((d, def)) = self.sampler_def(device) else { return };
        let (px, py, label) = self.handle_params(d, def, i);
        let params: Vec<usize> = px.into_iter().chain(py).collect();
        if params.is_empty() {
            return;
        }
        match phase {
            0 => {
                let before = params.iter().map(|&j| (j, def.value(d, &def.params[j]))).collect();
                self.sp_mut().handle_drag = Some((device, before));
            }
            1 => {
                if self.sp().handle_drag.as_ref().is_none_or(|(id, _)| *id != device) {
                    return;
                }
                let (w, h) = ((485.0 * Z), (87.0 * Z));
                let tab = self.sp().tab.get(&device).copied().unwrap_or(0);
                let n = |id: usize| def.params[id].to_normalized(self.panel_value(d, def, &def.params[id]));
                let mut set: Vec<(usize, f32)> = Vec::new();
                match tab {
                    1 => set.extend(px.map(|j| (j, def.params[j].from_normalized(((x - 8.0) / (w - 16.0)).clamp(0.0, 1.0))))),
                    2 if !self.sp().env_view.contains(&device) => {
                        set.extend(px.map(|j| (j, def.params[j].from_normalized((x / w).clamp(0.0, 1.0)))));
                        set.extend(py.map(|j| (j, ((h * 0.5 - y) / (h * 0.45)).clamp(0.0, 1.0))));
                    }
                    _ => {
                        let e = EnvBox { w, h };
                        let (aw, dw, _, rw) = e.stage();
                        let pre = if tab == 2 { "filter" } else { "amp" };
                        let a = def.index_of(&format!("{pre}Attack")).map_or(0.0, n);
                        let dd = def.index_of(&format!("{pre}Decay")).map_or(0.0, n);
                        let (xa, xd, xs, _) = e.xs(a, dd, 0.0);
                        let _ = xd;
                        let t = match i {
                            0 => (x - EnvBox::PAD) / aw,
                            1 => (x - xa) / dw,
                            _ => (x - xs) / rw,
                        };
                        if i < 3 {
                            set.extend(px.map(|j| (j, def.params[j].from_normalized(t.clamp(0.0, 1.0)))));
                        }
                        set.extend(py.map(|j| (j, ((e.bottom() - y) / (e.bottom() - e.top())).clamp(0.0, 1.0))));
                    }
                }
                for (j, v) in set {
                    let v = def.params[j].snap(v);
                    let _ = self.engine.builtin_set_param(device.0, j as u32, v);
                    self.sp_mut().live.insert((device, j), v);
                }
                self.ui_dirty = true;
            }
            _ => {
                let Some((_, before)) = self.sp_mut().handle_drag.take() else { return };
                let live: Vec<(usize, f32)> =
                    before.iter().filter_map(|(j, _)| self.sp_mut().live.remove(&(device, *j)).map(|v| (*j, v))).collect();
                self.ui_dirty = true;
                if phase != 2 {
                    for (j, v) in before {
                        let _ = self.engine.builtin_set_param(device.0, j as u32, v);
                    }
                    return;
                }
                let commands: Vec<Command> = live
                    .into_iter()
                    .filter(|(j, v)| before.iter().any(|(k, b)| k == j && b != v))
                    .map(|(j, v)| Command::SetDeviceParam { device, param: def.params[j].id.to_string(), value: v })
                    .collect();
                if !commands.is_empty() {
                    self.panel_commit(Command::Batch { label: label.into(), commands });
                }
            }
        }
    }

    /// Escape: cancel a well, handle or slice-knob gesture (restoring the
    /// engine values). True when one was active.
    pub(crate) fn sampler_cancel_drags(&mut self) -> bool {
        if let Some(g) = self.sp().well_drag {
            self.sampler_well(g.device, 3, 0.0, 0.0, false);
            return true;
        }
        if let Some((device, before)) = self.sp_mut().handle_drag.take() {
            for (j, v) in before {
                self.sp_mut().live.remove(&(device, j));
                let _ = self.engine.builtin_set_param(device.0, j as u32, v);
            }
            self.ui_dirty = true;
            return true;
        }
        if self.sp_mut().slice_drag.take().is_some() {
            self.ui_dirty = true;
            return true;
        }
        false
    }

    /// Slice knob gestures (pseudo parameters SLICE_*): drag phases as panel_drag.
    pub(crate) fn slice_drag(&mut self, device: DeviceId, index: i32, phase: i32, dy: f32, rate: f32, fine: bool) {
        let Some(p) = slice_def(index) else { return };
        let Some((d, _)) = self.sampler_def(device) else { return };
        let sel = self.sp().slice.get(&device).copied().unwrap_or(0);
        let Some(cur) = d.sample(0).and_then(|s| s.slices.get(sel)).map(|s| slice_field(s, index)) else { return };
        match phase {
            0 => {
                self.sp_mut().slice_drag =
                    Some(SliceDrag { device, index, start: cur, norm: p.to_normalized(cur), value: cur, travel: 0.0, active: false })
            }
            1 => {
                let Some(g) = self.sp_mut().slice_drag.as_mut().filter(|g| g.device == device && g.index == index) else { return };
                g.travel += dy.abs();
                if !g.active && g.travel < 3.0 {
                    return;
                }
                g.active = true;
                g.norm = (g.norm + dy * rate * if fine { 0.2 } else { 1.0 }).clamp(0.0, 1.0);
                g.value = p.snap(p.from_normalized(g.norm));
                self.ui_dirty = true;
            }
            2 => {
                let Some(g) = self.sp_mut().slice_drag.take() else { return };
                self.ui_dirty = true;
                if g.active && g.value != g.start {
                    self.set_slice_value(device, index, g.value);
                }
            }
            _ => {
                self.sp_mut().slice_drag = None;
                self.ui_dirty = true;
            }
        }
    }

    /// Set the selected slice's gain, pan or pitch (one undo step).
    pub(crate) fn set_slice_value(&mut self, device: DeviceId, index: i32, v: f32) {
        let Some(p) = slice_def(index) else { return };
        let Some((d, _)) = self.sampler_def(device) else { return };
        let sel = self.sp().slice.get(&device).copied().unwrap_or(0);
        let Some(mut sample) = d.sample(0).cloned() else { return };
        let Some(s) = sample.slices.get_mut(sel) else { return };
        set_slice_field(s, index, p.snap(v.clamp(p.min, p.max)));
        self.panel_commit(Command::SetDeviceSample { device, slot: 0, sample: Some(sample) });
    }

    /// Reset / step / typed value of a slice knob.
    pub(crate) fn slice_set(&mut self, device: DeviceId, index: i32, f: impl FnOnce(&ParamDef, f32) -> Option<f32>) {
        let Some(p) = slice_def(index) else { return };
        let Some((d, _)) = self.sampler_def(device) else { return };
        let sel = self.sp().slice.get(&device).copied().unwrap_or(0);
        let Some(cur) = d.sample(0).and_then(|s| s.slices.get(sel)).map(|s| slice_field(s, index)) else { return };
        if let Some(v) = f(p, cur) {
            self.set_slice_value(device, index, v);
        }
    }

    /// Typed start / end / loop start in seconds ("1.2 s", "350 ms").
    pub(crate) fn sampler_parse(&self, device: DeviceId, p: &ParamDef, text: &str) -> Option<f32> {
        let (d, def) = self.sampler_def(device)?;
        if !matches!(p.id.rsplit('_').next(), Some("start" | "end" | "loopStart")) {
            return None;
        }
        let secs = self.sample_secs(d.sample(self.shown_slot(d, def)));
        if secs <= 0.0 || text.trim().ends_with('%') {
            return None;
        }
        Some((parse_secs(text)? / secs).clamp(0.0, 1.0))
    }

    /// A parameter command plus what it implies for the sampler: switching
    /// to Slice mode chops an unsliced sample; changing the chop settings
    /// chops again.
    pub(crate) fn sampler_followups(&self, cmd: Command) -> Command {
        let (device, set): (DeviceId, Vec<(&str, f32)>) = match &cmd {
            Command::SetDeviceParam { device, param, value } => (*device, vec![(param.as_str(), *value)]),
            Command::SetDeviceParams { device, params, .. } => (*device, params.iter().map(|(k, v)| (k.as_str(), *v)).collect()),
            _ => return cmd,
        };
        let Some((d, def)) = self.sampler_def(device) else { return cmd };
        if def.id != "sampler-steam" {
            return cmd;
        }
        let Some(sample) = d.sample(0) else { return cmd };
        let now = |id: &str| def.param(id).map_or(0.0, |p| def.value(d, p));
        let v = |id: &str| set.iter().find(|(k, _)| *k == id).map_or_else(|| now(id), |s| s.1);
        let chop_changed = ["chop", "division", "sensitivity"].iter().any(|id| v(id) != now(id));
        let rechop = match &cmd {
            Command::SetDeviceParam { param, .. } if matches!(param.as_str(), "chop" | "division" | "sensitivity") => true,
            _ => v("mode") == 2.0 && (sample.slices.is_empty() || chop_changed),
        };
        if !rechop {
            return cmd;
        }
        let Some(asset) = self.project().asset(sample.asset) else { return cmd };
        let slices = self.chop_for(asset, v("chop") == 1.0, v("division") as u32, v("sensitivity"));
        let sample = DeviceSample { slices, ..sample.clone() };
        let label = cmd.label();
        Command::Batch { label, commands: vec![cmd, Command::SetDeviceSample { device, slot: 0, sample: Some(sample) }] }
    }

    fn chop_for(&self, asset: &dstudio_core::Asset, transient: bool, division: u32, sensitivity: f32) -> Vec<Slice> {
        let frames = (asset.frames as f64 * self.rate / asset.sample_rate) as u64;
        let l0 = self.level0_of(asset.id).unwrap_or(&[]);
        chop(l0, crate::peaks::L0_FRAMES as f64, self.rate, frames, transient, division, sensitivity, self.project().transport.bpm)
    }

    // ---------------------------------------------------------------- loading

    /// Where a picked or dropped file goes on `device` (sample slot, and
    /// whether the next free layer of that pad is meant).
    pub fn sampler_load_target(&self, device: DeviceId) -> Option<ImportTarget> {
        let (d, def) = self.sampler_def(device)?;
        let slot = self.shown_slot(d, def);
        Some(ImportTarget::DeviceSample { device, slot, next_free: false })
    }

    /// The sampler of the selected track shown in the Devices panel, if any.
    pub fn selected_sampler(&self) -> Option<DeviceId> {
        let d = self.selected_track.and_then(|t| self.project().track(t))?.instrument_plugin.as_ref()?;
        builtin::def_of(d).filter(|def| is_sampler(def)).map(|_| d.id)
    }

    /// Files dropped on the window: onto the selected track's sampler (a
    /// drum pad takes them as new layers), else imported as clips.
    pub fn drop_files(&mut self, paths: Vec<PathBuf>) {
        let audio: Vec<PathBuf> = paths.into_iter().filter(|p| crate::files::is_audio(p)).collect();
        if audio.is_empty() {
            return self.set_status("Only audio files (WAV, AIFF, FLAC, Ogg Vorbis, MP3) can be loaded", 4);
        }
        let Some(device) = self.selected_sampler().filter(|_| self.bottom_tab == crate::controller::BottomTab::Devices) else {
            return self.import_files(audio, ImportTarget::Selected, None);
        };
        self.load_into_sampler(device, audio);
    }

    /// Load audio files into sampler `device`: the Sampler takes the first, a drum pad takes
    /// them all as new layers of the pad it shows.
    pub fn load_into_sampler(&mut self, device: DeviceId, audio: Vec<PathBuf>) {
        let Some((d, def)) = self.sampler_def(device) else { return };
        let target = if def.id == "sampler-steam" {
            ImportTarget::DeviceSample { device, slot: 0, next_free: false }
        } else {
            let slot = self.shown_slot(d, def);
            ImportTarget::DeviceSample { device, slot: slot - slot % builtin::pad_sampler::LAYERS, next_free: true }
        };
        let n = if def.id == "sampler-steam" { 1 } else { audio.len() };
        self.import_files(audio.into_iter().take(n).collect(), target, None);
    }

    /// The Devices panel's chain area, outlined while a file is dragged over a sampler.
    pub(crate) fn sampler_drop_rect(&self) -> Option<(f32, f32, f32, f32)> {
        let l = self.layout();
        let (x, chain) = (crate::devices::PANEL_X, crate::devices::CHAIN_AREA_X);
        Some((x + chain, l.bottom_y() + 27.0, l.w - 2.0 * x - chain, l.bottom_h() - 27.0))
    }

    /// Finish loading a file into a device: the asset and the sample in one
    /// undo step ("Load Sample"). Returns false when nothing was loaded.
    pub(crate) fn finish_device_sample(
        &mut self,
        asset: dstudio_core::Asset,
        name: String,
        device: DeviceId,
        slot: u32,
        next_free: bool,
    ) -> bool {
        let Some((d, def)) = self.sampler_def(device) else { return false };
        let slot = if next_free {
            let layers = builtin::pad_sampler::LAYERS;
            match (slot..slot + layers).find(|s| d.sample(*s).is_none()) {
                Some(s) => s,
                None => {
                    self.set_status("This pad already holds 8 sounds.", 4);
                    return false;
                }
            }
        } else {
            slot
        };
        let mut sample = DeviceSample { slot, asset: asset.id, name, slices: vec![] };
        let mode2 = def.id == "sampler-steam" && def.param("mode").is_some_and(|p| def.value(d, p) == 2.0);
        let (transient, division, sens) = {
            let v = |id: &str| def.param(id).map_or(0.0, |p| def.value(d, p));
            (v("chop") == 1.0, v("division") as u32, v("sensitivity"))
        };
        if mode2 {
            sample.slices = self.chop_for(&asset, transient, division, sens);
        }
        let ok = self.execute(Command::Batch {
            label: "Load Sample".into(),
            commands: vec![Command::AddAsset { asset }, Command::SetDeviceSample { device, slot, sample: Some(sample) }],
        });
        if ok && def.id != "sampler-steam" {
            self.sp_mut().layer.insert(device, slot % builtin::pad_sampler::LAYERS);
        }
        ok
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chop_by_beat_and_transient() {
        // 2 s at 48 kHz, 120 bpm: quarter notes are 0.5 s -> 4 slices.
        let s = chop(&[], 32.0, 48_000.0, 96_000, false, 2, 50.0, 120.0);
        let starts: Vec<f32> = s.iter().map(|s| s.start).collect();
        assert_eq!(starts, [0.0, 0.25, 0.5, 0.75]);
        // Two hits in silence.
        let mut pairs = vec![[0.0f32, 0.0f32]; 3000];
        pairs[1000..1100].fill([-0.8, 0.8]);
        pairs[2000..2100].fill([-0.6, 0.6]);
        let s = chop(&pairs, 32.0, 48_000.0, 96_000, true, 2, 50.0, 120.0);
        assert_eq!(s.len(), 3, "{s:?}");
        assert!((s[1].start - 1000.0 * 32.0 / 96_000.0).abs() < 1e-3);
    }

    #[test]
    fn waveform_path_and_formats() {
        let p = wave_path(&[[-0.5, 0.5], [-1.0, 1.0]], 2.0, 10.0);
        assert!(p.starts_with("M0.0 ") && p.ends_with('Z'));
        assert_eq!(fmt_secs(1.234), "1.23 s");
        assert_eq!(parse_secs("350 ms"), Some(0.35));
        assert_eq!(parse_secs("1.5 s"), Some(1.5));
        assert!(filter_db(0, 1000.0, 0.0, 100.0).abs() < 0.5);
        assert!(filter_db(0, 1000.0, 0.0, 10_000.0) < -30.0);
    }

    #[test]
    fn envelope_geometry_is_monotonic() {
        let e = EnvBox { w: 400.0, h: 80.0 };
        let (a, d, s, r) = e.xs(0.5, 0.5, 0.5);
        assert!(EnvBox::PAD < a && a < d && d < s && s < r && r <= 400.0);
    }
}
