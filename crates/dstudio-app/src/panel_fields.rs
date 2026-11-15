//! Custom field views of the built-in device cards (View::Custom(name)):
//! the delay's echo train and repeat EQ, the echo's tail, the reverb's tail
//! picture and EQ wash, the pitch shifter's interval field and the grains
//! capture field.
//!
//! Rust draws everything as paths, texts and grip pills in well coordinates
//! (ui/device_panel.slint FieldView only renders them) and handles every
//! pointer event: a field drag sends live values to the engine and commits
//! one undo step on release, like a knob drag (src/device_panel.rs).

use crate::controller::Controller;
use crate::device_panel::{Z, s};
use crate::{PanelCanvas, PanelGrip, PanelPath, PanelText};
use dstudio_core::builtin::{DeviceDef, View, delay, echo, grains, pitch_shifter, reverb};
use dstudio_core::{Command, Device, DeviceId};
use slint::{Color, ModelRc, VecModel};
use std::collections::{HashMap, VecDeque};
use std::fmt::Write;
use std::rc::Rc;
use std::time::Instant;

/// Card-relative field geometry (unscaled pt at 100%).
const PAD: f32 = 8.0;
const FIELD_Y: f32 = 63.0;
const FIELD_Y_NO_PICKER: f32 = 37.0;
const FIELD_BOTTOM: f32 = 214.0;
const CAPTION_H: f32 = 19.0;
const DRAG_SLOP: f32 = 3.0;
const BANDS: usize = 24;
/// Wet spectrum bands after the delay / reverb readouts (dsp::SpectrumTap, 20 Hz .. 20 kHz).
const WASH_BANDS: usize = crate::panel_graph::BANDS;
const TRAIL: usize = 12;
const MINUS: char = '\u{2212}';

/// Live engine state of one custom-view device, refreshed by the poll tick.
#[derive(Debug, Clone, Default)]
pub struct Live {
    ro: Vec<f32>,
    /// Smoothed band levels (dB) of the wet spectrum (delay, reverb EQ wash).
    wash: Vec<f32>,
    /// Grains input level history (time, dB), newest last.
    levels: VecDeque<(Instant, f32)>,
    /// Grains frames (seconds back, semitones, level), newest first.
    trail: VecDeque<Vec<(f32, f32, f32)>>,
}

/// What a field press grabbed.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Grab {
    DelayTime { right: bool },
    DelayBand(usize),
    Pitch,
    EchoHead { head: usize, generation: f32 },
    EchoIntensity,
    ReverbPre,
    ReverbCut { high: bool },
    ReverbDecay,
    ReverbEdge { high: bool },
    ReverbBand,
    GrainsPosition { offset: f32 },
    GrainsSpray,
}

/// A field drag in progress (well coordinates).
#[derive(Debug, Clone)]
pub struct FieldDrag {
    pub device: DeviceId,
    grab: Grab,
    x0: f32,
    y0: f32,
    x: f32,
    y: f32,
    fine: bool,
    /// Visible span at the press (seconds), frozen while dragging.
    span: f32,
    /// (param index, value before the drag) of every parameter the drag may write.
    start: Vec<(usize, f32)>,
    /// Live values (param index, value).
    pub values: Vec<(usize, f32)>,
    active: bool,
}

/// Field interaction state kept by the panel.
#[derive(Debug, Default)]
pub struct FieldState {
    pub drag: Option<FieldDrag>,
    hover: Option<(DeviceId, Option<Grab>)>,
    live: HashMap<DeviceId, Live>,
    /// Card view (Echoes / EQ, Tail / EQ) per device: UI state, not a parameter.
    pub views: HashMap<DeviceId, usize>,
    /// Selected repeat-EQ band per delay.
    bands: HashMap<DeviceId, usize>,
    /// Wash image and its well box per device and surface (card, window), set while it is built.
    pub(crate) washes: std::cell::RefCell<HashMap<(DeviceId, bool), Wash>>,
    /// A wash was added, redrawn or removed since the last push to the UI.
    pub(crate) washes_dirty: std::cell::Cell<bool>,
}

/// Field wash image, its box in well coordinates, and the inputs it was drawn from (bits).
#[derive(Debug, Clone)]
pub(crate) struct Wash {
    pub img: slint::Image,
    pub rect: [f32; 4],
    key: Vec<u32>,
}

// ------------------------------------------------------------------ drawing helpers

fn model<T: Clone + 'static>(v: Vec<T>) -> ModelRc<T> {
    ModelRc::from(Rc::new(VecModel::from(v)))
}

fn ink(a: f32) -> Color {
    Color::from_argb_u8((a.clamp(0.0, 1.0) * 255.0) as u8, 255, 255, 255)
}

const NONE: Color = Color::from_argb_u8(0, 0, 0, 0);

#[derive(Default)]
struct Canvas {
    paths: Vec<PanelPath>,
    texts: Vec<PanelText>,
    pills: Vec<PanelGrip>,
    /// Intensity image, its box (x, y, w, h) in well coordinates and the inputs it was drawn from.
    wash: Option<Wash>,
    /// Drawn for the device's window (its wash is kept apart from the card's).
    window: bool,
}

impl Canvas {
    fn stroke(&mut self, cmd: String, color: Color, width: f32) {
        if !cmd.is_empty() {
            self.paths.push(PanelPath { commands: s(cmd), stroke: color, width, fill: NONE, fill2: NONE });
        }
    }
    fn fill(&mut self, cmd: String, color: Color) {
        if !cmd.is_empty() {
            self.paths.push(PanelPath { commands: s(cmd), stroke: NONE, width: 0.0, fill: color, fill2: color });
        }
    }
    /// Text whose vertical centre is y; align 0 left, 1 centre, 2 right within w.
    #[allow(clippy::too_many_arguments)]
    fn text(&mut self, x: f32, y: f32, w: f32, text: impl Into<String>, color: Color, size: f32, align: i32) {
        self.texts.push(PanelText { x, y: y - size * 0.65, w, h: size * 1.3, text: s(text.into()), color, size, weight: 600, align });
    }
    /// Grip pill with its left edge at x (kept inside the well), centred on y.
    fn pill(&mut self, x: f32, y: f32, w: f32, label: &str, value: &str, hot: bool) {
        let width = pill_width(label, value);
        self.pills.push(PanelGrip { x: x.min(w - width - 2.0).max(2.0), y, label: s(label), value: s(value), hot, ..Default::default() });
    }
}

fn pill_width(label: &str, value: &str) -> f32 {
    ((label.chars().count() + value.chars().count()) as f32 * 6.1 + 34.0) * Z
}

fn seg(out: &mut String, x0: f32, y0: f32, x1: f32, y1: f32) {
    let _ = write!(out, "M{x0:.2} {y0:.2} L{x1:.2} {y1:.2} ");
}

fn rect(out: &mut String, x: f32, y: f32, w: f32, h: f32) {
    if w > 0.0 && h > 0.0 {
        let _ = write!(out, "M{x:.2} {y:.2} h{w:.2} v{h:.2} h{:.2} Z ", -w);
    }
}

fn circle(out: &mut String, cx: f32, cy: f32, r: f32) {
    let _ = write!(out, "M{:.2} {cy:.2} a{r:.2} {r:.2} 0 1 0 {:.2} 0 a{r:.2} {r:.2} 0 1 0 {:.2} 0 Z ", cx - r, 2.0 * r, -2.0 * r);
}

/// Horizontal capsule from x0 to x1 centred on y, t thick.
fn capsule(out: &mut String, x0: f32, x1: f32, y: f32, t: f32) {
    let r = t / 2.0;
    let len = (x1 - x0 - t).max(0.0);
    let _ =
        write!(out, "M{:.2} {:.2} h{len:.2} a{r:.2} {r:.2} 0 0 1 0 {t:.2} h{:.2} a{r:.2} {r:.2} 0 0 1 0 {:.2} Z ", x0 + r, y - r, -len, -t);
}

fn polyline(out: &mut String, pts: &[(f32, f32)]) {
    for (i, (x, y)) in pts.iter().enumerate() {
        let _ = write!(out, "{}{x:.1} {y:.1} ", if i == 0 { "M" } else { "L" });
    }
}

fn dashed_h(out: &mut String, x0: f32, x1: f32, y: f32, dash: f32, gap: f32) {
    let mut x = x0;
    while x < x1 {
        seg(out, x, y, (x + dash).min(x1), y);
        x += dash + gap;
    }
}

fn dashed_v(out: &mut String, x: f32, y0: f32, y1: f32, dash: f32, gap: f32) {
    let mut y = y0;
    while y < y1 {
        seg(out, x, y, x, (y + dash).min(y1));
        y += dash + gap;
    }
}

/// First value of spans that is at least need, else the last.
fn pick_span(spans: &[f32], need: f32) -> f32 {
    spans.iter().copied().find(|s| *s >= need).unwrap_or(spans[spans.len() - 1])
}

/// Ruler step on a 1/2/5 series so labels are at least min_px apart.
fn ruler_step(span: f32, width: f32, min_px: f32) -> f32 {
    for e in -2..4 {
        for m in [1.0, 2.0, 5.0] {
            let step = m * 10f32.powi(e);
            if width * step / span >= min_px {
                return step;
            }
        }
    }
    span
}

fn seconds_label(t: f32) -> String {
    let text = format!("{:.2}", (t * 100.0).round() / 100.0);
    text.trim_end_matches('0').trim_end_matches('.').to_string()
}

fn db_label(db: f32) -> String {
    if db > 0.0 {
        format!("+{}", db as i32)
    } else if db < 0.0 {
        format!("{MINUS}{}", -db as i32)
    } else {
        "0".into()
    }
}

/// Log-frequency axis: x of hz between lo and hi over [x0, x1], and back.
fn log_x(hz: f32, lo: f32, hi: f32, x0: f32, x1: f32) -> f32 {
    x0 + (hz.max(1.0) / lo).ln() / (hi / lo).ln() * (x1 - x0)
}

fn log_hz(x: f32, lo: f32, hi: f32, x0: f32, x1: f32) -> f32 {
    lo * (hi / lo).powf((x - x0) / (x1 - x0))
}

/// Magnitude (dB) of an RBJ biquad at 48 kHz: 0 high-pass, 1 low shelf,
/// 2 peak, 3 high shelf, 4 low-pass.
fn biquad_db(kind: u8, f0: f32, q: f32, gain: f32, f: f32) -> f32 {
    use std::f64::consts::PI;
    let fs = 48000.0f64;
    let w0 = 2.0 * PI * (f0 as f64).clamp(10.0, 23000.0) / fs;
    let (sn, cs) = w0.sin_cos();
    let alpha = sn / (2.0 * (q as f64).max(0.05));
    let a = 10f64.powf(gain as f64 / 40.0);
    let k = 2.0 * a.sqrt() * alpha;
    let (b, den) = match kind {
        0 => ([(1.0 + cs) / 2.0, -(1.0 + cs), (1.0 + cs) / 2.0], [1.0 + alpha, -2.0 * cs, 1.0 - alpha]),
        4 => ([(1.0 - cs) / 2.0, 1.0 - cs, (1.0 - cs) / 2.0], [1.0 + alpha, -2.0 * cs, 1.0 - alpha]),
        2 => ([1.0 + alpha * a, -2.0 * cs, 1.0 - alpha * a], [1.0 + alpha / a, -2.0 * cs, 1.0 - alpha / a]),
        1 => (
            [a * ((a + 1.0) - (a - 1.0) * cs + k), 2.0 * a * ((a - 1.0) - (a + 1.0) * cs), a * ((a + 1.0) - (a - 1.0) * cs - k)],
            [(a + 1.0) + (a - 1.0) * cs + k, -2.0 * ((a - 1.0) + (a + 1.0) * cs), (a + 1.0) + (a - 1.0) * cs - k],
        ),
        _ => (
            [a * ((a + 1.0) + (a - 1.0) * cs + k), -2.0 * a * ((a - 1.0) + (a + 1.0) * cs), a * ((a + 1.0) + (a - 1.0) * cs - k)],
            [(a + 1.0) - (a - 1.0) * cs + k, 2.0 * ((a - 1.0) - (a + 1.0) * cs), (a + 1.0) - (a - 1.0) * cs - k],
        ),
    };
    let w = 2.0 * PI * f as f64 / fs;
    let (s1, c1) = w.sin_cos();
    let (s2, c2) = (2.0 * w).sin_cos();
    let num = (b[0] + b[1] * c1 + b[2] * c2).powi(2) + (b[1] * s1 + b[2] * s2).powi(2);
    let dd = (den[0] + den[1] * c1 + den[2] * c2).powi(2) + (den[1] * s1 + den[2] * s2).powi(2);
    (10.0 * (num / dd.max(1e-30)).log10()) as f32
}

/// Band type of each repeat-EQ band of the delay (High Pass .. Low Pass).
const DELAY_BAND_KIND: [u8; 6] = [0, 1, 2, 2, 3, 4];
pub(crate) const DELAY_BAND_IDS: [[&str; 4]; 6] = [
    ["eq0Freq", "eq0Gain", "eq0Q", "eq0Enabled"],
    ["eq1Freq", "eq1Gain", "eq1Q", "eq1Enabled"],
    ["eq2Freq", "eq2Gain", "eq2Q", "eq2Enabled"],
    ["eq3Freq", "eq3Gain", "eq3Q", "eq3Enabled"],
    ["eq4Freq", "eq4Gain", "eq4Q", "eq4Enabled"],
    ["eq5Freq", "eq5Gain", "eq5Q", "eq5Enabled"],
];

fn is_filter(b: usize) -> bool {
    DELAY_BAND_KIND[b] == 0 || DELAY_BAND_KIND[b] == 4
}

/// Delay EQ plot: dB axis +15 .. -30 over the well height minus the label strip.
fn delay_eq_y(db: f32, h: f32) -> f32 {
    let (top, bottom) = (6.0 * Z, h - 14.0 * Z);
    top + (15.0 - db) / 45.0 * (bottom - top)
}

/// Summed response (dB) of the enabled repeat-EQ bands at f.
fn delay_response(band: &dyn Fn(usize, usize) -> f32, f: f32) -> f32 {
    (0..6).filter(|b| band(*b, 3) >= 0.5).map(|b| biquad_db(DELAY_BAND_KIND[b], band(b, 0), band(b, 2), band(b, 1), f)).sum()
}

/// Handle of a delay EQ band: (freq, gain) for shelves and peaks, on the curve for the filters.
fn delay_handle(b: usize, band: &dyn Fn(usize, usize) -> f32, w: f32, h: f32) -> (f32, f32) {
    let f = band(b, 0);
    let db = if is_filter(b) { if band(b, 3) >= 0.5 { delay_response(band, f) } else { 0.0 } } else { band(b, 1) };
    (log_x(f, 20.0, 20000.0, 0.0, w).clamp(6.0 * Z, w - 6.0 * Z), delay_eq_y(db.clamp(-30.0, 15.0), h))
}

/// Reverb tail model constants per mode (Room, Hall, Plate, Spring, Shimmer, Haze).
const ARRIVAL_SPACING: [f32; 6] = [0.006, 0.012, 0.003, 0.022, 0.01, 0.0015];
const FIRST_ARRIVAL: [f32; 6] = [0.06, 0.045, 0.005, 0.04, 0.0, 0.0];

fn reverb_b(f: f32) -> f32 {
    0.5 * (1.0 + (f / 250.0).powi(2)).log2()
}

fn reverb_t60(decay: f32, damping: f32, f: f32) -> f32 {
    decay / (1.0 + damping * (reverb_b(f) - reverb_b(1000.0)).max(0.0) * 0.85)
}

/// Reverb output filter law (dB) at f; a Low Cut at 20 Hz is off.
fn reverb_filter_db(low: f32, high: f32, f: f32) -> f32 {
    let lo = if low <= 20.02 { 0.0 } else { -10.0 * (1.0 + (low / f).powi(2)).log10() };
    lo - 10.0 * (1.0 + (f / high).powi(2)).log10()
}

/// Band edge / centre frequency of the 24 reverb decay-picture bands (40 Hz .. 16 kHz).
fn band_hz(i: f32) -> f32 {
    40.0 * 400f32.powf(i / BANDS as f32)
}

/// Delay times (s) of the left and right sides.
fn delay_times(v: &dyn Fn(&str) -> f32, bpm: f32) -> (f32, f32) {
    let beat = 60.0 / bpm.max(1.0);
    let synced = v("sync") >= 0.5;
    let side = |ms: &str, div: &str| {
        if synced { delay::DIVISION_BEATS[(v(div).round() as usize).min(14)] * beat } else { v(ms).max(1.0) / 1000.0 }
    };
    let l = side("timeMs", "timeDivision");
    (l, if v("link") >= 0.5 { l } else { side("timeMsR", "timeDivisionR") })
}

/// Predicted echo train of the delay: (time s, right side, level).
fn delay_taps(tl: f32, tr: f32, feedback: f32, width: f32, until: f32) -> Vec<(f32, bool, f32)> {
    let fb = feedback.clamp(0.0, 0.97);
    let t = width / 200.0;
    let amount = if t <= 0.5 { 0.0 } else { (t - 0.5) * 2.0 };
    let time = |right: bool| if right { tr } else { tl };
    let mut open: VecDeque<(f32, bool, f32)> = VecDeque::from([(tl, false, 1.0), (tr, true, 1.0 - amount)]);
    let mut out = Vec::new();
    while let Some((at, right, level)) = open.pop_front() {
        if level < 0.02 || at > until || out.len() >= 128 {
            continue;
        }
        out.push((at, right, level));
        open.push_back((at + time(right), right, level * fb * (1.0 - amount)));
        open.push_back((at + time(!right), !right, level * fb * amount));
    }
    out.sort_by(|a, b| a.0.total_cmp(&b.0));
    out
}

fn delay_span(longest: f32, synced: bool, bpm: f32) -> f32 {
    if synced {
        let beat = 60.0 / bpm.max(1.0);
        pick_span(&[4.0, 8.0, 16.0, 32.0], 6.0 * longest / beat) * beat
    } else {
        pick_span(&[0.25, 0.5, 1.0, 2.0, 4.0, 8.0, 12.0], 6.0 * longest)
    }
}

/// Echo repeats: (head index, generation, time s, level dB) of every drawn bar.
fn echo_repeats(pic: &echo::Picture, span: f32) -> Vec<(usize, i32, f32, f32)> {
    let feeds = pic.heads.iter().any(|hd| hd.fb);
    let mut out = Vec::new();
    for (i, hd) in pic.heads.iter().enumerate().filter(|(_, hd)| hd.on) {
        for n in 1..=if feeds { 8 } else { 1 } {
            let db = 20.0 * (pic.pass_gain.powi(n - 1) * hd.gain).max(1e-9).log10();
            let t = hd.ms / 1000.0 * n as f32;
            if db < -35.5 || t > span {
                break;
            }
            out.push((i, n, t, db));
        }
    }
    out
}

fn echo_span(pic: &echo::Picture) -> f32 {
    let longest = pic.heads.iter().filter(|h| h.on).map(|h| h.ms / 1000.0).fold(0.0f32, f32::max);
    pick_span(&[0.5, 1.0, 1.5, 2.0, 3.0, 4.0, 6.0, 8.0, 12.0, 16.0], 8.0 * longest)
}

/// Echo plot box (left, right, top, bottom) in the well.
fn echo_box(w: f32, h: f32) -> (f32, f32, f32, f32) {
    (40.0 * Z, w - 6.0 * Z, 22.0 * Z, h - 6.0 * Z)
}

fn reverb_span(decay: f32, damping: f32, pre: f32) -> f32 {
    pick_span(&[0.5, 1.0, 2.0, 3.0, 4.0, 6.0, 8.0, 10.0], 0.25 + pre + reverb_t60(decay, damping, 1000.0).min(20.0) * 1.1)
}

/// Reverb tail plot box (left, right, top, bottom).
fn reverb_box(w: f32, h: f32) -> (f32, f32, f32, f32) {
    (26.0 * Z, w - 8.0 * Z, 8.0 * Z, h - 16.0 * Z)
}

fn reverb_y(hz: f32, top: f32, bottom: f32) -> f32 {
    bottom - (hz.max(1.0) / 40.0).ln() / 400f32.ln() * (bottom - top)
}

/// Grains reach (seconds): where grains read, how far Spray scatters them and how far they travel.
struct Reach {
    rhythm: bool,
    step: f32,
    position: f32,
    spray: f32,
    length: f32,
    travel: f32,
    span: f32,
}

fn grains_reach(v: &dyn Fn(&str) -> f32, bpm: f32) -> Reach {
    let beat = 60.0 / bpm.max(1.0);
    let rhythm = v("mode").round() as u32 == grains::RHYTHM;
    let frozen = v("freeze") >= 0.5;
    let step = beat * grains::RATE_BEATS[(v("rate").round() as usize).min(11)];
    let position =
        if rhythm { grains::POSITION_BEATS[(v("position_note").round() as usize).min(9)] * beat } else { v("position") / 1000.0 };
    let spray_ms = v("spray");
    let spray = if !rhythm {
        spray_ms / 1000.0
    } else if spray_ms > 0.5 {
        step.max(spray_ms / 1000.0)
    } else {
        0.0
    };
    let length = if rhythm { step * v("gate") / 100.0 } else { v("size") / 1000.0 };
    let ratio = 2f32.powf(v("pitch") / 12.0);
    let travel = if frozen { length * ratio } else { length * (ratio - 1.0).max(0.0) };
    let shown = position.max(if !frozen && ratio > 1.0 { travel + 0.025 * ratio } else { travel });
    let span = pick_span(&[0.5, 1.0, 2.0, 4.0, 8.0, 16.0, 20.0], shown + spray + travel * 1.2 + 0.05);
    Reach { rhythm, step, position: shown, spray, length, travel, span }
}

/// Grains plot: x of a time back from now.
fn grains_x(back: f32, span: f32, w: f32) -> f32 {
    let (left, right) = (4.0 * Z, w - 5.0 * Z);
    right - back / span * (right - left)
}

fn grains_back(x: f32, span: f32, w: f32) -> f32 {
    let (left, right) = (4.0 * Z, w - 5.0 * Z);
    (right - x) / (right - left) * span
}

fn cursor_of(g: Grab) -> i32 {
    match g {
        Grab::DelayTime { .. } | Grab::EchoHead { .. } | Grab::ReverbDecay | Grab::DelayBand(_) => 1,
        Grab::Pitch | Grab::ReverbPre | Grab::ReverbEdge { .. } | Grab::ReverbBand | Grab::GrainsPosition { .. } | Grab::GrainsSpray => 2,
        Grab::EchoIntensity | Grab::ReverbCut { .. } => 3,
    }
}

/// Hover identity of a grab (ignores the press offset and generation).
fn hover_key(g: Option<Grab>) -> Option<Grab> {
    g.map(|g| match g {
        Grab::GrainsPosition { .. } => Grab::GrainsPosition { offset: 0.0 },
        Grab::EchoHead { head, .. } => Grab::EchoHead { head, generation: 0.0 },
        other => other,
    })
}

/// Spectrum wash from the smoothed band levels (dB): a filled curve from the bottom.
fn wash(c: &mut Canvas, live: &Live, color: Color, x_of: &dyn Fn(f32) -> f32, bottom: f32, height: f32) {
    if live.wash.len() != WASH_BANDS || live.wash.iter().all(|b| *b < -90.0) {
        return;
    }
    let hz = |b: usize| 20.0 * 1000f32.powf(b as f32 / (WASH_BANDS - 1) as f32);
    let mut pts: Vec<(f32, f32)> = vec![(x_of(hz(0)), bottom)];
    for (b, db) in live.wash.iter().enumerate() {
        pts.push((x_of(hz(b)), bottom - ((db + 72.0) / 72.0).clamp(0.0, 1.0) * height * 0.85));
    }
    pts.push((x_of(hz(WASH_BANDS - 1)), bottom));
    let (first, last) = (pts[0], pts[pts.len() - 1]);
    let curve = crate::device_panel::smooth(&pts[1..pts.len() - 1], true);
    c.fill(format!("M{:.1} {:.1}{curve} L{:.1} {:.1} Z", first.0, first.1, last.0, last.1), color);
}

pub(crate) fn field_name(def: &DeviceDef) -> Option<&'static str> {
    match def.layout.view {
        Some(View::Custom(n @ ("delay" | "echo" | "reverb" | "pitch-shifter" | "grains"))) => Some(n),
        _ => None,
    }
}

/// Field rectangle of a custom-view card: card-relative (x, y, w, h) and well top, scaled.
pub(crate) fn field_rect(def: &DeviceDef, view_right: f32) -> (f32, f32, f32, f32, f32) {
    let top = if def.layout.picker.is_empty() { FIELD_Y_NO_PICKER } else { FIELD_Y };
    let well_y = if matches!(field_name(def), Some("pitch-shifter" | "grains")) { CAPTION_H } else { 0.0 };
    (PAD * Z, top * Z, (view_right - PAD) * Z, (FIELD_BOTTOM - top) * Z, well_y * Z)
}

impl Controller {
    fn bpm(&self) -> f32 {
        self.project().transport.bpm as f32
    }

    /// Value of a parameter as the field shows it (live drag value, else the model).
    fn fv_named(&self, d: &Device, def: &DeviceDef, id: &str) -> f32 {
        def.param(id).map_or(0.0, |p| self.panel_value(d, def, p))
    }

    pub(crate) fn panel_field(&self, d: &Device, def: &'static DeviceDef, view_right: f32) -> PanelCanvas {
        let (x, y, w, h, well_y) = field_rect(def, view_right);
        PanelCanvas { x, y, h, well_y, ..self.field_canvas(d, def, w, h - well_y, false) }
    }

    /// Size of the well a field pointer event comes from: the window's plot while the
    /// event comes from a device window, else the card's well.
    pub(crate) fn field_size(&self, def: &DeviceDef) -> (f32, f32) {
        if self.devices.panel.canvas.window
            && let Some(size) = crate::device_window::plot_size(def)
        {
            return size;
        }
        let (_, _, w, fh, well_y) = field_rect(def, crate::device_panel::view_right(def));
        (w, fh - well_y)
    }

    /// A device's field drawn into a w x wh well (the card's, or its window's display).
    pub(crate) fn field_canvas(&self, d: &Device, def: &'static DeviceDef, w: f32, wh: f32, window: bool) -> PanelCanvas {
        let Some(name) = field_name(def) else { return PanelCanvas::default() };
        let mut c = Canvas { window, ..Canvas::default() };
        let fields = &self.devices.panel.fields;
        let view = self.card_view(d.id);
        let drag = fields.drag.as_ref().filter(|g| g.device == d.id && g.active);
        let hover = fields.hover.filter(|h| h.0 == d.id).and_then(|h| h.1);
        let hot = drag.map(|g| g.grab).or(hover);
        let empty = Live::default();
        let live = fields.live.get(&d.id).unwrap_or(&empty);
        let accent = self.card_accent(d);
        let v = |id: &str| self.fv_named(d, def, id);
        let (mut caption, mut value) = (String::new(), String::new());
        match name {
            "pitch-shifter" => {
                caption = "Interval".into();
                value = pitch_shifter::interval_text(v("semitones"), v("cents"));
                draw_pitch(&mut c, &v, accent, w, wh, hot.is_some());
            }
            "delay" if view == 1 => {
                let selected = self.delay_band(d.id);
                draw_delay_eq(&mut c, &v, live, accent, w, wh, hot, selected);
            }
            "delay" => self.draw_delay_echoes(&mut c, d, def, live, accent, w, wh, drag),
            "echo" => draw_echo(&mut c, &self.echo_picture(d, def), v("swell") / 100.0, accent, w, wh, drag, hot),
            "reverb" if view == 1 => draw_reverb_eq(&mut c, &v, live, accent, w, wh, hot),
            "reverb" => self.draw_reverb_tail(&mut c, d, def, live, accent, w, wh, drag, hot),
            "grains" => {
                caption = "Capture".into();
                value = if v("mode").round() as u32 == grains::RHYTHM {
                    grains::POSITION_NOTES[(v("position_note").round() as usize).min(9)].to_string()
                } else {
                    grains::ms_text(v("position"))
                };
                draw_grains(&mut c, &v, live, accent, self.bpm(), w, wh, drag, hot);
            }
            _ => {}
        }
        let cursor = match (hot, name) {
            (Some(g), _) => cursor_of(g),
            (None, "echo") => 3,
            _ => 0,
        };
        let mut washes = fields.washes.borrow_mut();
        let changed = match c.wash.take() {
            Some(w) => washes.insert((d.id, window), w.clone()).is_none_or(|old| old.key != w.key),
            None => washes.remove(&(d.id, window)).is_some(),
        };
        drop(washes);
        if changed {
            fields.washes_dirty.set(true);
        }
        PanelCanvas {
            visible: true,
            well: true,
            w,
            h: wh,
            caption: s(caption),
            readout: s(value),
            paths: model(c.paths),
            texts: model(c.texts),
            grips: model(c.pills),
            pointer: true,
            cursor,
            ..Default::default()
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_delay_echoes(
        &self,
        c: &mut Canvas,
        d: &Device,
        def: &DeviceDef,
        live: &Live,
        accent: Color,
        w: f32,
        h: f32,
        drag: Option<&FieldDrag>,
    ) {
        let v = |id: &str| self.fv_named(d, def, id);
        let (left, right, top, bottom) = (18.0 * Z, w - 8.0 * Z, 8.0 * Z, h - 16.0 * Z);
        let pw = right - left;
        let bpm = self.bpm();
        let (tl, tr) = delay_times(&v, bpm);
        let synced = v("sync") >= 0.5;
        let span = drag.map_or_else(|| delay_span(tl.max(tr), synced, bpm), |g| g.span);
        let x_of = |t: f32| left + t / span * pw;
        let lane = (bottom - top) / 2.0;
        let cy = [top + lane / 2.0, top + lane * 1.5];

        // Ruler: beat grid with bar numbers (synced) or seconds on a 1/2/5 series.
        let (mut grid, mut bars) = (String::new(), String::new());
        if synced {
            let beat = 60.0 / bpm.max(1.0);
            let mut b = 0;
            while b as f32 * beat <= span + 1e-4 {
                let x = x_of(b as f32 * beat);
                if b % 4 == 0 {
                    seg(&mut bars, x, top, x, bottom);
                    if x < right - 12.0 * Z {
                        c.text(x + 2.0 * Z, bottom + 8.0 * Z, 30.0 * Z, format!("{}", b / 4 + 1), ink(0.5), 10.0 * Z, 0);
                    }
                } else {
                    seg(&mut grid, x, top, x, bottom);
                }
                b += 1;
            }
        } else {
            let step = ruler_step(span, pw, 56.0 * Z);
            let mut t = 0.0;
            while t <= span + 1e-4 {
                seg(&mut grid, x_of(t), top, x_of(t), bottom);
                if x_of(t) < right - 30.0 * Z {
                    c.text(
                        x_of(t) + 2.0 * Z,
                        bottom + 8.0 * Z,
                        40.0 * Z,
                        if t == 0.0 { "0".into() } else { format!("{} s", seconds_label(t)) },
                        ink(0.5),
                        10.0 * Z,
                        0,
                    );
                }
                t += step;
            }
        }
        c.stroke(grid, ink(0.05), 1.0);
        c.stroke(bars, ink(0.1), 1.0);
        let mut lanes = String::new();
        for (i, y) in cy.iter().enumerate() {
            seg(&mut lanes, left, *y, right, *y);
            c.text(4.0 * Z, *y, 12.0 * Z, if i == 0 { "L" } else { "R" }, ink(0.5), 10.0 * Z, 0);
        }
        c.stroke(lanes, ink(0.06), 1.0);
        let mut ticks = String::new();
        for y in cy {
            rect(&mut ticks, left - 1.5 * Z, y - lane * 0.45, 3.0 * Z, lane * 0.9);
        }
        c.fill(ticks, ink(0.5));

        // Echo bars, grouped by alpha step.
        let mut groups: Vec<String> = vec![String::new(); 8];
        for (t, side, level) in delay_taps(tl, tr, v("feedback") / 100.0, v("stereoWidth"), span) {
            let bh = lane * 0.9 * level;
            rect(&mut groups[((0.3 + 0.7 * level) * 8.0).min(7.0) as usize], x_of(t) - 1.5 * Z, cy[side as usize] - bh / 2.0, 3.0 * Z, bh);
        }
        for (i, g) in groups.into_iter().enumerate() {
            c.fill(g, accent.with_alpha((i as f32 + 0.5) / 8.0));
        }
        // Live markers: the delay the engine runs now (top edge L, bottom edge R).
        if live.ro.first().is_some_and(|db| *db > -80.0) {
            let (ml, mr) = (live.ro.get(1).copied().unwrap_or(0.0) / 1000.0, live.ro.get(2).copied().unwrap_or(0.0) / 1000.0);
            let mut m = String::new();
            rect(&mut m, x_of(ml) - 1.0, 0.0, 2.0, 3.0 * Z);
            if (mr - ml).abs() > 1e-3 {
                rect(&mut m, x_of(mr) - 1.0, h - 3.0 * Z, 2.0, 3.0 * Z);
            }
            c.fill(m, accent);
        }
        // Pill on the first echo of the grabbed side.
        let right_side = matches!(drag.map(|g| g.grab), Some(Grab::DelayTime { right: true }));
        let id = match (synced, right_side) {
            (true, false) => "timeDivision",
            (true, true) => "timeDivisionR",
            (false, false) => "timeMs",
            (false, true) => "timeMsR",
        };
        if let Some(p) = def.param(id) {
            let t = if right_side { tr } else { tl };
            c.pill(
                x_of(t) - 9.0 * Z,
                cy[right_side as usize] - lane * 0.45 + 2.0 * Z,
                w,
                def.label_of(d, p),
                &def.text(d, p, v(id)),
                drag.is_some(),
            );
        }
    }

    pub(crate) fn echo_picture(&self, d: &Device, def: &DeviceDef) -> echo::Picture {
        // The picture follows live drag values: evaluate it on a copy carrying them.
        match self.devices.panel.fields.drag.as_ref().filter(|g| g.device == d.id && !g.values.is_empty()) {
            Some(g) => {
                let mut copy = d.clone();
                for (i, val) in &g.values {
                    copy.params.insert(def.params[*i].id.to_string(), *val);
                }
                echo::picture(def, &copy, self.bpm() as f64)
            }
            None => echo::picture(def, d, self.bpm() as f64),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_reverb_tail(
        &self,
        c: &mut Canvas,
        d: &Device,
        def: &DeviceDef,
        live: &Live,
        accent: Color,
        w: f32,
        h: f32,
        drag: Option<&FieldDrag>,
        hot: Option<Grab>,
    ) {
        let v = |id: &str| self.fv_named(d, def, id);
        let (left, right, top, bottom) = reverb_box(w, h);
        let (decay, damping, pre) = (v("decay"), v("damping") / 100.0, v("predelay") / 1000.0);
        let mode = (v("mode2").round() as usize).min(5);
        let size = v("size") / 100.0;
        let span = drag.map_or_else(|| reverb_span(decay, damping, pre), |g| g.span);
        let x_of = |t: f32| left + t / span * (right - left);
        let y_of = |hz: f32| reverb_y(hz, top, bottom);
        let (low, high) = (v("lowcut2"), v("highcut2"));
        let shimmer = if mode as u32 == reverb::SHIMMER { 0.6 } else { 0.0 };
        let arrivals: Vec<f32> = (1..=8)
            .map(|i| FIRST_ARRIVAL[mode] * (0.6 + size) / 1.1 + ARRIVAL_SPACING[mode] * (i as f32).powf(1.1) * (0.4 + 1.2 * size))
            .collect();
        let first = arrivals[0];

        // Wash: WASH_COLS columns x 24 bands of accent at the tail's intensity, drawn smoothed
        // (bilinear) over the plot, a low-resolution reverb field.
        const WASH_COLS: usize = 128;
        let key: Vec<u32> = [decay, damping, pre, mode as f32, size, span, low, high, left, top, right, bottom, shimmer]
            .iter()
            .map(|v| v.to_bits())
            .chain([u32::from_be_bytes([accent.red(), accent.green(), accent.blue(), accent.alpha()])])
            .collect();
        if let Some(w) = self.devices.panel.fields.washes.borrow().get(&(d.id, c.window)).filter(|w| w.key == key) {
            c.wash = Some(w.clone());
        }
        if c.wash.is_none() {
            let mut px = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::new(WASH_COLS as u32, BANDS as u32);
            let (ar, ag, ab) = (accent.red(), accent.green(), accent.blue());
            let pixels = px.make_mut_slice();
            for b in 0..BANDS {
                let f = band_hz(b as f32 + 0.5);
                let t60 = reverb_t60(decay, damping, f).max(0.01);
                let filt = 10f32.powf(reverb_filter_db(low, high, f) / 20.0);
                let level_at = |ci: usize| -> f32 {
                    let t = (ci as f32 + 0.5) / WASH_COLS as f32 * span;
                    let intensity = if t < pre {
                        0.0
                    } else if t < pre + first {
                        let u = (t - pre) / first.max(1e-4);
                        0.25 + 0.75 * u * u * (3.0 - 2.0 * u)
                    } else {
                        let k = (1.0 - (t - pre - first) / t60).max(0.0).powi(2);
                        if k < 0.01 { 0.0 } else { k }
                    };
                    let x2 = (ci as f32 + 0.5) * (right - left) / WASH_COLS as f32 / (2.0 * Z);
                    (intensity * 0.85 * (1.0 + shimmer * 0.3 * (0.45 * x2 + 1.3 * b as f32).sin()) * filt).clamp(0.0, 0.9)
                };
                let row = &mut pixels[(BANDS - 1 - b) * WASH_COLS..][..WASH_COLS];
                for (ci, p) in row.iter_mut().enumerate() {
                    *p = slint::Rgba8Pixel { r: ar, g: ag, b: ab, a: (level_at(ci) * 255.0).round() as u8 };
                }
            }
            c.wash = Some(Wash { img: slint::Image::from_rgba8(px), rect: [left, top, right - left, bottom - top], key });
        }

        // Axes.
        let mut grid = String::new();
        for (hz, t) in [(100.0, "100"), (1000.0, "1k"), (10000.0, "10k")] {
            seg(&mut grid, left, y_of(hz), right, y_of(hz));
            c.text(3.0 * Z, y_of(hz), 22.0 * Z, t, ink(0.6), 10.0 * Z, 0);
        }
        c.text(3.0 * Z, bottom + 9.0 * Z, 22.0 * Z, "Hz", ink(0.6), 10.0 * Z, 0);
        let step = ruler_step(span, right - left, 56.0 * Z);
        let mut t = 0.0;
        while t <= span + 1e-4 {
            if t > 0.0 {
                seg(&mut grid, x_of(t), top, x_of(t), bottom);
            }
            c.text(x_of(t) - 4.0 * Z, bottom + 9.0 * Z, 30.0 * Z, seconds_label(t), ink(0.6), 10.0 * Z, 0);
            t += step;
        }
        c.stroke(grid, ink(0.06), 1.0);

        // Markers: arrivals, T60 (dashed), the pre-delay line, the cut lines with their pills.
        let mut arr = String::new();
        for a in &arrivals {
            let x = x_of(pre + a);
            if x < right {
                seg(&mut arr, x, top + (bottom - top) * 0.15, x, bottom);
            }
        }
        c.stroke(arr, accent.with_alpha(0.6), 1.0);
        let t60x = x_of(pre + reverb_t60(decay, damping, 1000.0));
        if t60x < right {
            let mut dash = String::new();
            dashed_v(&mut dash, t60x, top, bottom, 4.0 * Z, 3.0 * Z);
            c.stroke(dash, if hot == Some(Grab::ReverbDecay) { accent } else { ink(0.3) }, 1.0);
        }
        let px = x_of(pre);
        let mut pl = String::new();
        seg(&mut pl, px, top, px, bottom);
        c.stroke(pl, ink(0.95), if hot == Some(Grab::ReverbPre) { 2.0 } else { 1.5 } * Z);
        let text = |id: &str| def.param(id).map(|p| def.text(d, p, v(id))).unwrap_or_default();
        c.pill(px, top + 9.0 * Z, w, "Pre-Delay", &text("predelay"), hot == Some(Grab::ReverbPre));
        for (high_cut, id, label) in [(true, "highcut2", "High Cut"), (false, "lowcut2", "Low Cut")] {
            let y = y_of(v(id)).clamp(top, bottom);
            let is_hot = hot == Some(Grab::ReverbCut { high: high_cut });
            let mut dash = String::new();
            dashed_h(&mut dash, left, right, y, 4.0 * Z, 3.0 * Z);
            c.stroke(dash, ink(if is_hot { 0.7 } else { 0.45 }), if is_hot { 1.5 } else { 1.0 } * Z);
            let value = text(id);
            c.pill(right - pill_width(label, &value) - 4.0 * Z, y.clamp(top + 9.0 * Z, bottom - 9.0 * Z), w, label, &value, is_hot);
        }
        // Live tail level bar at the right edge.
        if let Some(db) = live.ro.first().filter(|db| **db > -60.0) {
            let lv = ((db + 60.0) / 60.0).clamp(0.0, 1.0) * (bottom - top);
            let mut bar = String::new();
            rect(&mut bar, w - 3.0 * Z, bottom - lv, 3.0 * Z, lv);
            c.fill(bar, accent);
        }
    }

    // ------------------------------------------------------------- hit testing

    /// What a press at (x, y) in the well of device d grabs.
    #[allow(clippy::too_many_arguments)]
    fn field_hit(&self, d: &Device, def: &DeviceDef, name: &str, w: f32, h: f32, x: f32, y: f32) -> Option<Grab> {
        let v = |id: &str| self.fv_named(d, def, id);
        let view = self.card_view(d.id);
        match name {
            "pitch-shifter" => Some(Grab::Pitch),
            "delay" if view == 1 => {
                let band = |b: usize, f: usize| v(DELAY_BAND_IDS[b][f]);
                (0..6)
                    .map(|b| {
                        let (cx, cy) = delay_handle(b, &band, w, h);
                        (b, (cx - x).hypot(cy - y))
                    })
                    .filter(|(_, dist)| *dist <= 10.0 * Z)
                    .min_by(|a, b| a.1.total_cmp(&b.1))
                    .map(|(b, _)| Grab::DelayBand(b))
            }
            "delay" => Some(Grab::DelayTime { right: v("link") < 0.5 && y > h / 2.0 - 4.0 * Z }),
            "echo" => {
                let pic = self.echo_picture(d, def);
                let span = echo_span(&pic);
                let (left, right, _, _) = echo_box(w, h);
                let best = echo_repeats(&pic, span)
                    .into_iter()
                    .map(|(head, n, t, _)| (head, n, (left + t / span * (right - left) - x).abs()))
                    .filter(|r| r.2 <= 8.0 * Z)
                    .min_by(|a, b| a.2.total_cmp(&b.2));
                Some(best.map_or(Grab::EchoIntensity, |(head, n, _)| Grab::EchoHead { head, generation: n as f32 }))
            }
            "reverb" if view == 1 => {
                let x_of = |hz: f32| log_x(hz, 20.0, 20000.0, 0.0, w);
                let (xl, xh) = (x_of(v("lowcut2")), x_of(v("highcut2")));
                let by = 4.0 * Z + 11.0 * Z;
                if (x - xh).abs() <= 9.0 * Z && y < by + 10.0 * Z {
                    Some(Grab::ReverbEdge { high: true })
                } else if (x - xl).abs() <= 9.0 * Z && y < by + 10.0 * Z {
                    Some(Grab::ReverbEdge { high: false })
                } else if y < by + 26.0 * Z && x > xl - 40.0 * Z && x < xh + 40.0 * Z {
                    Some(Grab::ReverbBand)
                } else {
                    None
                }
            }
            "reverb" => {
                let (left, right, top, bottom) = reverb_box(w, h);
                let span = reverb_span(v("decay"), v("damping") / 100.0, v("predelay") / 1000.0);
                let px = left + v("predelay") / 1000.0 / span * (right - left);
                let cut_y = |id: &str| reverb_y(v(id), top, bottom).clamp(top, bottom);
                if (x - px).abs() <= 6.0 * Z || (y < top + 18.0 * Z && x >= px && x < px + pill_width("Pre-Delay", "100 ms")) {
                    Some(Grab::ReverbPre)
                } else if (y - cut_y("highcut2")).abs() <= 6.0 * Z {
                    Some(Grab::ReverbCut { high: true })
                } else if (y - cut_y("lowcut2")).abs() <= 6.0 * Z {
                    Some(Grab::ReverbCut { high: false })
                } else {
                    Some(Grab::ReverbDecay)
                }
            }
            "grains" => {
                let r = grains_reach(&v, self.bpm());
                let (xp, xs) = (grains_x(r.position, r.span, w), grains_x(r.position + r.spray, r.span, w));
                if r.spray > 0.0 && (x - xs).abs() <= 7.0 * Z {
                    Some(Grab::GrainsSpray)
                } else if (x - xp).abs() <= 7.0 * Z {
                    Some(Grab::GrainsPosition { offset: xp - x })
                } else {
                    Some(Grab::GrainsPosition { offset: 0.0 })
                }
            }
            _ => None,
        }
    }

    /// Parameters a grab may write, for start values, resets and commits.
    fn grab_params(&self, def: &DeviceDef, d: &Device, grab: Grab) -> Vec<&'static str> {
        let v = |id: &str| self.fv_named(d, def, id);
        let synced = v("sync") >= 0.5;
        match grab {
            Grab::DelayTime { right } => vec![
                match (synced, right) {
                    (true, false) => "timeDivision",
                    (true, true) => "timeDivisionR",
                    (false, false) => "timeMs",
                    (false, true) => "timeMsR",
                },
                "feedback",
            ],
            Grab::DelayBand(b) => DELAY_BAND_IDS[b].to_vec(),
            Grab::Pitch => vec!["semitones", "cents"],
            Grab::EchoHead { .. } if v("machine").round() as u32 == 2 => vec!["motor", "intensity", "note_l", "note_r"],
            Grab::EchoHead { .. } => {
                let mut ids = vec!["intensity", "heads", "heads_machine", "note_l", "note_r"];
                for n in 1..=6 {
                    for f in ["present", "on", "fb", "right", "pos", "level", "pan", "pan_pinned", "pos_pinned"] {
                        ids.extend(echo::head_param(n, f));
                    }
                }
                ids
            }
            Grab::EchoIntensity => vec!["intensity"],
            Grab::ReverbPre => vec!["predelay"],
            Grab::ReverbCut { high: true } | Grab::ReverbEdge { high: true } => vec!["highcut2"],
            Grab::ReverbCut { high: false } | Grab::ReverbEdge { high: false } => vec!["lowcut2"],
            Grab::ReverbDecay => vec!["decay", "damping"],
            Grab::ReverbBand => vec!["lowcut2", "highcut2"],
            Grab::GrainsPosition { .. } if v("mode").round() as u32 == grains::RHYTHM => vec!["position_note"],
            Grab::GrainsPosition { .. } => vec!["position"],
            Grab::GrainsSpray => vec!["spray"],
        }
    }

    /// Values a drag writes for the pointer at its current position, from the start values.
    fn drag_values(&self, def: &DeviceDef, d: &Device, g: &FieldDrag, w: f32, h: f32) -> Vec<(&'static str, f32)> {
        let start = |id: &str| def.param(id).map_or(0.0, |p| def.value(d, p));
        let (dx, dy) = (g.x - g.x0, g.y - g.y0);
        let fine = if g.fine { 0.2 } else { 1.0 };
        let norm_shift = |id: &'static str, delta: f32| -> Option<(&'static str, f32)> {
            let p = def.param(id)?;
            Some((id, p.from_normalized((p.to_normalized(start(id)) + delta * fine).clamp(0.0, 1.0))))
        };
        let bpm = self.bpm();
        let beat = 60.0 / bpm.max(1.0);
        match g.grab {
            Grab::Pitch if g.fine => vec![("cents", (start("cents") + dx * 0.5).clamp(-100.0, 100.0).round())],
            Grab::Pitch => vec![("semitones", (g.x / w * 49.0 - 24.5).round().clamp(-24.0, 24.0))],
            Grab::DelayTime { .. } => {
                let time_id = self.grab_params(def, d, g.grab)[0];
                let (pw, lanes) = (w - 26.0 * Z, h - 24.0 * Z);
                let synced = start("sync") >= 0.5;
                let t0 =
                    if synced { delay::DIVISION_BEATS[(start(time_id).round() as usize).min(14)] * beat } else { start(time_id) / 1000.0 };
                let t = (t0 + dx / pw * g.span * fine).max(0.001);
                let time = if synced {
                    let near = |i: &usize| (delay::DIVISION_BEATS[*i].ln() - (t / beat).ln()).abs();
                    (0..15).min_by(|a, b| near(a).total_cmp(&near(b))).unwrap_or(8) as f32
                } else {
                    (t * 1000.0).clamp(1.0, 12000.0)
                };
                vec![(time_id, time), ("feedback", (start("feedback") - dy / lanes * 100.0 * fine).clamp(0.0, 100.0))]
            }
            Grab::DelayBand(b) => {
                let ids = DELAY_BAND_IDS[b];
                let f = log_hz(g.x, 20.0, 20000.0, 0.0, w).clamp(20.0, 20000.0);
                let (top, bottom) = (6.0 * Z, h - 14.0 * Z);
                let gain = if is_filter(b) { start(ids[1]) } else { (15.0 - (g.y - top) / (bottom - top) * 45.0).clamp(-30.0, 15.0) };
                vec![(ids[0], f), (ids[1], gain), (ids[3], 1.0)]
            }
            Grab::EchoIntensity => vec![("intensity", (start("intensity") - dy / (h - 28.0 * Z) * 10.0 * fine).clamp(0.0, 10.0))],
            Grab::EchoHead { head, generation } => {
                let pic = echo::picture(def, d, bpm as f64);
                let intensity = ("intensity", (start("intensity") - dy / (h - 28.0 * Z) * 10.0 * fine).clamp(0.0, 10.0));
                let Some(hd) = pic.heads.get(head) else { return vec![intensity] };
                let (left, right, _, _) = echo_box(w, h);
                let ms = (hd.ms + dx / (right - left) * g.span * 1000.0 * fine / generation.max(1.0)).max(1.0);
                let side = hd.pan > 0.0;
                let first = pic.heads.iter().filter(|h| h.on && (h.pan > 0.0) == side).map(|h| h.mm).fold(f32::INFINITY, f32::min);
                if start("sync") >= 0.5 && hd.on && hd.mm == first {
                    let near = |i: &usize| (echo::note_beats(*i).ln() - (ms / 1000.0 / beat).ln()).abs();
                    let note = (0..echo::NOTES.len()).min_by(|a, b| near(a).total_cmp(&near(b))).unwrap_or(10) as f32;
                    return vec![(if side { "note_r" } else { "note_l" }, note), intensity];
                }
                if pic.machine == 2 {
                    let motor =
                        ((echo::CARTRIDGE.heads_mm[0] - ms / 1000.0 * echo::CARTRIDGE_SPEED) / echo::CARTRIDGE_TRAVEL).clamp(0.0, 1.0);
                    return vec![("motor", motor), intensity];
                }
                let mut out = echo::authored_patch(&pic, head, ms);
                out.push(intensity);
                out
            }
            Grab::ReverbPre => {
                let (left, right, _, _) = reverb_box(w, h);
                vec![("predelay", (start("predelay") + dx / (right - left) * g.span * 1000.0 * fine).clamp(0.0, 100.0))]
            }
            Grab::ReverbCut { high } => {
                let (_, _, top, bottom) = reverb_box(w, h);
                let id = if high { "highcut2" } else { "lowcut2" };
                vec![(id, (start(id) * (-dy * fine / ((bottom - top) / 400f32.ln())).exp()).clamp(20.0, 20000.0))]
            }
            Grab::ReverbDecay => {
                let (left, right, top, bottom) = reverb_box(w, h);
                norm_shift("decay", dx / (right - left)).into_iter().chain(norm_shift("damping", -dy / (bottom - top))).collect()
            }
            Grab::ReverbEdge { high } => {
                let f = log_hz(g.x0 + dx * fine, 20.0, 20000.0, 0.0, w).clamp(20.0, 20000.0);
                if high { vec![("highcut2", f.max(start("lowcut2") * 1.25))] } else { vec![("lowcut2", f.min(start("highcut2") / 1.25))] }
            }
            Grab::ReverbBand => {
                let (lo, hi) = (start("lowcut2"), start("highcut2"));
                let ratio = (dx * fine / w * 1000f32.ln()).exp().clamp(20.0 / lo, 20000.0 / hi);
                vec![("lowcut2", lo * ratio), ("highcut2", hi * ratio)]
            }
            Grab::GrainsPosition { offset } => {
                let back = grains_back(g.x + offset, g.span, w).max(0.0);
                if start("mode").round() as u32 == grains::RHYTHM {
                    let near = |i: &usize| (grains::POSITION_BEATS[*i] - back / beat).abs();
                    vec![("position_note", (0..10).min_by(|a, b| near(a).total_cmp(&near(b))).unwrap_or(3) as f32)]
                } else {
                    vec![("position", (back * 1000.0).clamp(0.0, 8000.0))]
                }
            }
            Grab::GrainsSpray => {
                let v = |id: &str| self.fv_named(d, def, id);
                let r = grains_reach(&v, bpm);
                vec![("spray", ((grains_back(g.x, g.span, w) - r.position) * 1000.0).clamp(0.0, 2000.0))]
            }
        }
    }

    /// Pointer on a field (Studio.panel-graph): 0 down, 1 move, 2 up, 3 cancel, 4 hover, 5 leave, 6 double-click.
    #[allow(clippy::too_many_arguments)]
    /// Parameter under (x, y) in a field well: the first one its handle moves.
    pub(crate) fn field_param_at(&self, device: DeviceId, x: f32, y: f32) -> Option<usize> {
        let d = self.project().device(device)?;
        let def = dstudio_core::builtin::def_of(d)?;
        let name = field_name(def)?;
        let (_, _, w, fh, well_y) = field_rect(def, crate::device_panel::view_right(def));
        let grab = self.field_hit(d, def, name, w, fh - well_y, x, y)?;
        self.grab_params(def, d, grab).into_iter().filter(|id| !id.ends_with("Enabled")).find_map(|id| def.index_of(id))
    }

    pub fn panel_field_pointer(&mut self, device: DeviceId, phase: i32, x: f32, y: f32, shift: bool, alt: bool) {
        let Some(d) = self.project().device(device).cloned() else { return };
        let Some(def) = dstudio_core::builtin::def_of(&d) else { return };
        let Some(name) = field_name(def) else { return };
        let (w, h) = self.field_size(def);
        match phase {
            0 => {
                let Some(grab) = self.field_hit(&d, def, name, w, h, x, y) else { return };
                if let Grab::DelayBand(b) = grab {
                    self.devices.panel.fields.bands.insert(device, b);
                    self.ui_dirty = true;
                }
                let ids = self.grab_params(def, &d, grab);
                if alt {
                    let params = ids
                        .iter()
                        .filter_map(|id| def.param(id))
                        .filter(|p| !p.id.ends_with("Enabled"))
                        .map(|p| (p.id.to_string(), p.default))
                        .collect();
                    return self.field_commit(device, params);
                }
                let start: Vec<(usize, f32)> =
                    ids.iter().filter_map(|id| def.index_of(id).map(|i| (i, def.value(&d, &def.params[i])))).collect();
                let v = |id: &str| def.param(id).map_or(0.0, |p| def.value(&d, p));
                let span = match name {
                    "delay" => {
                        let (tl, tr) = delay_times(&v, self.bpm());
                        delay_span(tl.max(tr), v("sync") >= 0.5, self.bpm())
                    }
                    "echo" => echo_span(&echo::picture(def, &d, self.bpm() as f64)),
                    "reverb" => reverb_span(v("decay"), v("damping") / 100.0, v("predelay") / 1000.0),
                    "grains" => grains_reach(&v, self.bpm()).span,
                    _ => 1.0,
                };
                self.devices.panel.fields.drag =
                    Some(FieldDrag { device, grab, x0: x, y0: y, x, y, fine: shift, span, start, values: Vec::new(), active: false });
            }
            1 => {
                let Some(g) = self.devices.panel.fields.drag.as_mut().filter(|g| g.device == device) else { return };
                if !g.active && (x - g.x0).hypot(y - g.y0) < DRAG_SLOP {
                    return;
                }
                g.active = true;
                g.x = x;
                g.y = y;
                g.fine = shift;
                let g = g.clone();
                let values: Vec<(usize, f32)> = self
                    .drag_values(def, &d, &g, w, h)
                    .into_iter()
                    .filter_map(|(id, val)| def.index_of(id).map(|i| (i, def.params[i].snap(val))))
                    .collect();
                for (i, val) in &values {
                    let prev = g.values.iter().find(|v| v.0 == *i).map_or_else(|| def.value(&d, &def.params[*i]), |v| v.1);
                    if prev != *val {
                        let _ = self.engine.builtin_set_param(device.0, *i as u32, *val);
                    }
                }
                // A parameter the drag no longer writes goes back to its start value.
                for (i, val) in &g.values {
                    if !values.iter().any(|v| v.0 == *i) {
                        let _ = self.engine.builtin_set_param(device.0, *i as u32, def.value(&d, &def.params[*i]));
                        let _ = val;
                    }
                }
                if let Some(drag) = self.devices.panel.fields.drag.as_mut() {
                    drag.values = values;
                }
                self.ui_dirty = true;
            }
            2 => {
                let Some(g) = self.devices.panel.fields.drag.take().filter(|g| g.device == device) else { return };
                self.ui_dirty = true;
                if g.active {
                    let params = g.values.iter().map(|(i, val)| (def.params[*i].id.to_string(), *val)).collect();
                    self.field_commit(device, params);
                }
            }
            3 => {
                self.panel_cancel_field();
            }
            4 => {
                let grab = if self.devices.panel.fields.drag.is_some() { None } else { self.field_hit(&d, def, name, w, h, x, y) };
                let fields = &mut self.devices.panel.fields;
                if fields.hover.is_none_or(|h| h.0 != device || hover_key(h.1) != hover_key(grab)) {
                    self.ui_dirty = true;
                }
                fields.hover = Some((device, grab));
            }
            6 => {
                let Some(grab) = self.field_hit(&d, def, name, w, h, x, y) else { return };
                let params = match grab {
                    Grab::DelayBand(b) => {
                        let id = DELAY_BAND_IDS[b][3];
                        let on = def.param(id).is_some_and(|p| def.value(&d, p) >= 0.5);
                        vec![(id.to_string(), if on { 0.0 } else { 1.0 })]
                    }
                    Grab::ReverbDecay | Grab::EchoIntensity | Grab::DelayTime { .. } => return,
                    Grab::EchoHead { .. } => {
                        let id = if self.fv_named(&d, def, "machine").round() as u32 == 2 { "motor" } else { "heads" };
                        def.param(id).map(|p| (p.id.to_string(), p.default)).into_iter().collect()
                    }
                    g => {
                        self.grab_params(def, &d, g).iter().filter_map(|id| def.param(id)).map(|p| (p.id.to_string(), p.default)).collect()
                    }
                };
                self.field_commit(device, params);
            }
            _ => {
                if self.devices.panel.fields.hover.take().is_some() {
                    self.ui_dirty = true;
                }
            }
        }
    }

    /// Cancel a field drag: restore the engine to the values from before it.
    pub fn panel_cancel_field(&mut self) -> bool {
        let Some(g) = self.devices.panel.fields.drag.take() else { return false };
        for (i, val) in &g.start {
            let _ = self.engine.builtin_set_param(g.device.0, *i as u32, *val);
        }
        self.ui_dirty = true;
        true
    }

    /// One undo step for everything a field gesture changed.
    fn field_commit(&mut self, device: DeviceId, params: Vec<(String, f32)>) {
        let Some((d, def)) = self.project().device(device).and_then(|d| dstudio_core::builtin::def_of(d).map(|def| (d, def))) else {
            return;
        };
        let mut commands: Vec<Command> = params
            .into_iter()
            .filter(|(id, val)| def.param(id).is_some_and(|p| def.value(d, p) != *val))
            .map(|(param, value)| Command::SetDeviceParam { device, param, value })
            .collect();
        match commands.len() {
            0 => {}
            1 => {
                self.panel_commit(commands.remove(0));
            }
            _ => {
                self.panel_commit(Command::Batch { label: "Device Edit".into(), commands });
            }
        }
    }

    /// Engine readouts of the custom-view devices on the selected track (poll tick).
    pub(crate) fn panel_poll_fields(&mut self) {
        let Some(t) = self.selected_track.and_then(|t| self.project().track(t)) else { return };
        let devs: Vec<(DeviceId, &'static str, bool)> = t
            .devices
            .iter()
            .filter_map(|d| {
                let def = dstudio_core::builtin::def_of(d)?;
                Some((d.id, field_name(def)?, def.param("freeze").is_some_and(|p| def.value(d, p) >= 0.5)))
            })
            .collect();
        let now = Instant::now();
        self.devices.panel.fields.live.retain(|id, _| devs.iter().any(|d| d.0 == *id));
        let mut ro = Vec::new();
        for (id, name, frozen) in devs {
            // A window's Hold view keeps the last picture.
            if self.win_held(id) || self.engine.builtin_readouts(id.0, &mut ro).is_err() {
                continue;
            }
            let live = self.devices.panel.fields.live.entry(id).or_default();
            let mut dirty = live.ro.len() != ro.len() || live.ro.iter().zip(&ro).any(|(a, b)| (a - b).abs() > 0.05 && *b > -90.0);
            match name {
                "delay" | "reverb" => {
                    let off = if name == "delay" { 3 } else { 1 };
                    if live.wash.len() != WASH_BANDS {
                        live.wash = vec![-120.0; WASH_BANDS];
                    }
                    for (b, level) in live.wash.iter_mut().enumerate() {
                        let target = ro.get(off + b).copied().unwrap_or(-120.0);
                        // Attack ~100 ms, release ~260 ms at the ~33 ms tick.
                        let next = *level + (target - *level) * if target > *level { 0.28 } else { 0.12 };
                        dirty |= (next - *level).abs() > 0.1 && next > -90.0;
                        *level = next;
                    }
                }
                "grains" => {
                    if !frozen {
                        live.levels.push_back((now, ro.first().copied().unwrap_or(-120.0)));
                        while live.levels.front().is_some_and(|(at, _)| now.duration_since(*at).as_secs_f32() > 21.0) {
                            live.levels.pop_front();
                        }
                        dirty |= live.levels.iter().any(|l| l.1 > -48.0);
                    }
                    let frame: Vec<(f32, f32, f32)> =
                        ro.get(2..).unwrap_or(&[]).chunks(3).filter(|g| g.len() == 3 && g[2] > 0.0).map(|g| (g[0], g[1], g[2])).collect();
                    dirty |= !frame.is_empty() || live.trail.iter().any(|f| !f.is_empty());
                    live.trail.push_front(frame);
                    live.trail.truncate(TRAIL);
                }
                _ => {}
            }
            live.ro.clone_from(&ro);
            if dirty {
                self.ui_dirty = true;
            }
        }
    }

    /// Shown view of a card with views (0 = the first).
    pub(crate) fn card_view(&self, device: DeviceId) -> usize {
        self.devices.panel.fields.views.get(&device).copied().unwrap_or(0)
    }

    /// Card view switch (Echoes / EQ, Tail / EQ).
    /// Card view switch (picker `Control::Views`, sampler tabs): UI state, no undo step.
    /// The delay's selected repeat-EQ band (its card's EQ view and window rack).
    pub(crate) fn delay_band(&self, device: DeviceId) -> usize {
        self.devices.panel.fields.bands.get(&device).copied().unwrap_or(2)
    }

    pub(crate) fn set_delay_band(&mut self, device: DeviceId, band: usize) {
        self.devices.panel.fields.bands.insert(device, band.min(5));
        self.ui_dirty = true;
    }

    pub fn card_view_select(&mut self, device: DeviceId, view: i32) {
        if self.sampler_def(device).is_some() {
            return self.sampler_tab(device, view);
        }
        self.devices.panel.fields.views.insert(device, view.max(0) as usize);
        self.ui_dirty = true;
    }
}

// ------------------------------------------------------------------ device fields

fn draw_pitch(c: &mut Canvas, v: &dyn Fn(&str) -> f32, accent: Color, w: f32, h: f32, hot: bool) {
    let x_of = |st: f32| (st + 24.5) / 49.0 * w;
    let main = h * 0.72;
    let (mut major, mut minor) = (String::new(), String::new());
    for st in -24..=24 {
        let x = x_of(st as f32);
        if st % 12 == 0 { seg(&mut major, x, 0.0, x, main) } else { seg(&mut minor, x, main - 6.0 * Z, x, main) }
    }
    c.stroke(major, ink(0.14), 1.0);
    c.stroke(minor, ink(0.06), 1.0);
    let mix = v("mix") / 100.0;
    let (x0, x1) = (x_of(0.0), x_of(v("semitones") + v("cents") / 100.0));
    if (x1 - x0).abs() > 0.5 {
        let mut band = String::new();
        rect(&mut band, x0.min(x1), 0.0, (x1 - x0).abs(), main);
        c.fill(band, accent.with_alpha(0.09));
        let by = main * 0.62;
        let mut bar = String::new();
        seg(&mut bar, x0, by, x1, by);
        seg(&mut bar, x0, by - 3.0 * Z, x0, by + 3.0 * Z);
        seg(&mut bar, x1, by - 3.0 * Z, x1, by + 3.0 * Z);
        c.stroke(bar, accent.with_alpha(0.55), 1.0);
    }
    let mut dry = String::new();
    seg(&mut dry, x0, 0.0, x0, main);
    c.stroke(dry, ink(0.25 + 0.75 * (1.0 - mix)), 1.4 * Z);
    let mut wet = String::new();
    seg(&mut wet, x1, 0.0, x1, main);
    c.stroke(wet, accent.with_alpha(0.25 + 0.75 * mix), if hot { 2.15 } else { 1.4 } * Z);
    // Formant strip: baseline and the formant marker (held at 0 st, or following the shift).
    let base_y = main + (h - main) * 0.55;
    let mut base = String::new();
    seg(&mut base, 0.0, base_y, w, base_y);
    c.stroke(base, ink(0.06), 1.0);
    let held = v("formant_preserve") >= 0.5;
    let fx = if held { x0 } else { x1 };
    if !held && (x1 - x0).abs() > 0.5 {
        let mut dash = String::new();
        dashed_h(&mut dash, x0.min(x1), x0.max(x1), base_y, 3.0 * Z, 3.0 * Z);
        c.stroke(dash, accent.with_alpha(0.3), 1.0);
    }
    let mut tri = String::new();
    let _ = write!(tri, "M{:.2} {:.2} L{:.2} {:.2} L{:.2} {:.2} Z", fx, base_y - 8.0 * Z, fx + 5.0 * Z, base_y, fx - 5.0 * Z, base_y);
    c.fill(tri, if held { accent.with_alpha(0.9) } else { ink(0.55) });
}

#[allow(clippy::too_many_arguments)]
fn draw_delay_eq(c: &mut Canvas, v: &dyn Fn(&str) -> f32, live: &Live, accent: Color, w: f32, h: f32, hot: Option<Grab>, selected: usize) {
    let bottom = h - 14.0 * Z;
    let y_of = |db: f32| delay_eq_y(db, h);
    let x_of = |hz: f32| log_x(hz, 20.0, 20000.0, 0.0, w);
    let mut rungs = String::new();
    for db in [12.0, 0.0, -12.0, -24.0] {
        seg(&mut rungs, 26.0 * Z, y_of(db), w, y_of(db));
        c.text(4.0 * Z, y_of(db), 22.0 * Z, db_label(db), ink(0.5), 9.5 * Z, 0);
    }
    for (hz, t) in [(100.0, "100"), (1000.0, "1k"), (10000.0, "10k")] {
        seg(&mut rungs, x_of(hz), 6.0 * Z, x_of(hz), bottom);
        c.text(x_of(hz) - 15.0 * Z, bottom + 7.0 * Z, 30.0 * Z, t, ink(0.5), 9.5 * Z, 1);
    }
    c.text(4.0 * Z, bottom + 7.0 * Z, 20.0 * Z, "Hz", ink(0.5), 9.5 * Z, 0);
    c.stroke(rungs, ink(0.05), 1.0);
    wash(c, live, accent.with_alpha(0.25), &x_of, bottom, bottom - 6.0 * Z);

    let band = |b: usize, f: usize| v(DELAY_BAND_IDS[b][f]);
    let n = (w / 2.0).max(2.0) as usize;
    let pts: Vec<(f32, f32)> = (0..=n)
        .map(|i| {
            let x = w * i as f32 / n as f32;
            (x, y_of(delay_response(&band, log_hz(x, 20.0, 20000.0, 0.0, w)).clamp(-32.0, 17.0)))
        })
        .collect();
    let mut area = String::new();
    polyline(&mut area, &pts);
    let _ = write!(area, "L{w:.1} {:.1} L0 {:.1} Z", y_of(0.0), y_of(0.0));
    c.fill(area, accent.with_alpha(0.12));
    let mut line = String::new();
    polyline(&mut line, &pts);
    c.stroke(line, accent, 1.5 * Z);

    let focus = match hot {
        Some(Grab::DelayBand(b)) => b,
        _ => selected,
    };
    for b in 0..6 {
        let (cx, cy) = delay_handle(b, &band, w, h);
        let on = band(b, 3) >= 0.5;
        let mut dot = String::new();
        circle(&mut dot, cx, cy, if b == focus { 5.5 } else { 4.5 } * Z);
        if on {
            c.fill(dot.clone(), accent);
        }
        c.stroke(dot, if b == focus { ink(0.95) } else { ink(if on { 0.6 } else { 0.35 }) }, if b == focus { 1.5 } else { 1.0 } * Z);
    }
    let (cx, cy) = delay_handle(focus, &band, w, h);
    let gain = if is_filter(focus) {
        String::new()
    } else {
        format!(" {}", dstudio_core::builtin::format_value(dstudio_core::builtin::Format::Db, band(focus, 1), "dB", &[], 45.0))
    };
    let off = if band(focus, 3) >= 0.5 { "" } else { " (off)" };
    c.pill(
        cx + 9.0 * Z,
        (cy - 14.0 * Z).max(9.0 * Z),
        w,
        delay::BANDS[focus].0,
        &format!("{}{gain}{off}", delay::freq_text(band(focus, 0))),
        matches!(hot, Some(Grab::DelayBand(_))),
    );
}

#[allow(clippy::too_many_arguments)]
fn draw_echo(c: &mut Canvas, pic: &echo::Picture, smear: f32, accent: Color, w: f32, h: f32, drag: Option<&FieldDrag>, hot: Option<Grab>) {
    let (left, right, top, bottom) = echo_box(w, h);
    let span = drag.map_or_else(|| echo_span(pic), |g| g.span);
    let x_of = |t: f32| left + t / span * (right - left);
    let y_of = |db: f32| top + (6.0 - db) / 42.0 * (bottom - top);
    let mut grid = String::new();
    for db in [6.0, 0.0, -12.0, -24.0, -36.0] {
        seg(&mut grid, left, y_of(db), right, y_of(db));
        if db < 6.0 {
            c.text(8.0 * Z, y_of(db), 28.0 * Z, db_label(db), ink(0.6), 10.0 * Z, 0);
        }
    }
    let step = if span <= 1.0 {
        0.25
    } else if span <= 3.0 {
        0.5
    } else if span <= 8.0 {
        1.0
    } else {
        2.0
    };
    let mut t = step;
    while t < span - 1e-4 {
        seg(&mut grid, x_of(t), top, x_of(t), bottom);
        t += step;
    }
    c.stroke(grid, ink(0.06), 1.0);

    let hot_head = match hot {
        Some(Grab::EchoHead { head, .. }) => Some(head),
        _ => None,
    };
    let (mut l_bars, mut r_bars, mut tails) = (String::new(), String::new(), String::new());
    for (i, n, t, db) in echo_repeats(pic, span) {
        let bw = if hot_head == Some(i) { 3.0 } else { 2.0 } * Z;
        let (x, y) = (x_of(t), y_of(db.min(6.0)));
        rect(if pic.heads[i].pan > 0.0 { &mut r_bars } else { &mut l_bars }, x - bw / 2.0, y, bw, bottom - y);
        if smear > 0.0 {
            let grow = 1.0 - (1.0 - 0.4 * smear).powi(n - 1);
            rect(&mut tails, x + bw / 2.0, y, (3.0 + smear * 10.0) * Z * (1.0 + grow), bottom - y);
        }
    }
    c.fill(tails, accent.with_alpha(0.15 * smear));
    c.fill(l_bars, accent.with_alpha(1.0 - 0.6 * smear));
    c.fill(r_bars, ink(0.85 * (1.0 - 0.6 * smear)));
    if let Some(i) = hot_head
        && let Some(hd) = pic.heads.get(i)
    {
        c.pill(x_of(hd.ms / 1000.0) + 6.0 * Z, top + 2.0 * Z, w, &format!("Head {}", i + 1), &delay::ms_text(hd.ms), drag.is_some());
    }
}

fn draw_reverb_eq(c: &mut Canvas, v: &dyn Fn(&str) -> f32, live: &Live, accent: Color, w: f32, h: f32, hot: Option<Grab>) {
    let (top, bottom) = (4.0 * Z, h - 14.0 * Z);
    let x_of = |hz: f32| log_x(hz, 20.0, 20000.0, 0.0, w);
    let y_of = |db: f32| top + (12.0 - db) / 60.0 * (bottom - top);
    let mut grid = String::new();
    for (hz, t) in [(100.0, "100"), (1000.0, "1k"), (10000.0, "10k")] {
        seg(&mut grid, x_of(hz), top, x_of(hz), bottom);
        c.text(x_of(hz) - 15.0 * Z, bottom + 7.0 * Z, 30.0 * Z, t, ink(0.5), 9.5 * Z, 1);
    }
    c.text(4.0 * Z, bottom + 7.0 * Z, 20.0 * Z, "Hz", ink(0.5), 9.5 * Z, 0);
    seg(&mut grid, 0.0, y_of(0.0), w, y_of(0.0));
    c.stroke(grid, ink(0.05), 1.0);

    let (low, high, damping) = (v("lowcut2"), v("highcut2"), v("damping") / 100.0);
    let n = (w / 2.0).max(2.0) as usize;
    let pts: Vec<(f32, f32)> = (0..=n)
        .map(|i| {
            let x = w * i as f32 / n as f32;
            let f = log_hz(x, 20.0, 20000.0, 0.0, w);
            (x, y_of((reverb_filter_db(low, high, f) - 10.0 * (1.0 + damping * reverb_b(f) * 0.85).log10()).max(-50.0)))
        })
        .collect();
    let mut area = String::new();
    polyline(&mut area, &pts);
    let _ = write!(area, "L{w:.1} {bottom:.1} L0 {bottom:.1} Z");
    c.fill(area, accent.with_alpha(0.14));
    let mut line = String::new();
    polyline(&mut line, &pts);
    c.stroke(line, accent.with_alpha(0.6), 1.0);
    wash(c, live, accent.with_alpha(0.38), &x_of, bottom, bottom - top);

    // Filter band: edges at Low Cut / High Cut, the bar between them and its pill.
    let (xl, xh) = (x_of(low), x_of(high));
    let by = top + 11.0 * Z;
    let mut bar = String::new();
    rect(&mut bar, xl, by - 1.0 * Z, xh - xl, 2.0 * Z);
    c.fill(bar, accent.with_alpha(if hot == Some(Grab::ReverbBand) { 1.0 } else { 0.8 }));
    for (high_edge, x) in [(false, xl), (true, xh)] {
        let mut dot = String::new();
        circle(&mut dot, x, by, 3.5 * Z);
        c.fill(dot.clone(), accent.with_alpha(if !high_edge && low <= 20.02 { 0.45 } else { 1.0 }));
        if hot == Some(Grab::ReverbEdge { high: high_edge }) {
            c.stroke(dot, ink(0.95), 1.2 * Z);
        }
    }
    let text = format!("{} \u{2013} {}", reverb::cut_text(low), reverb::cut_text(high));
    let pw = pill_width("Filter", &text);
    let x = if xh - xl > pw {
        (xl + xh - pw) / 2.0
    } else if xl > w - xh {
        xl - pw - 6.0 * Z
    } else {
        xh + 6.0 * Z
    };
    c.pill(x, by + 16.0 * Z, w, "Filter", &text, hot == Some(Grab::ReverbBand));
}

#[allow(clippy::too_many_arguments)]
fn draw_grains(
    c: &mut Canvas,
    v: &dyn Fn(&str) -> f32,
    live: &Live,
    accent: Color,
    bpm: f32,
    w: f32,
    h: f32,
    drag: Option<&FieldDrag>,
    hot: Option<Grab>,
) {
    let r = grains_reach(v, bpm);
    let span = drag.map_or(r.span, |g| g.span);
    let (left, right) = (4.0 * Z, w - 5.0 * Z);
    let x_of = |back: f32| grains_x(back, span, w);
    let cy = h / 2.0;
    let y_of = |st: f32| cy - st / 24.0 * h / 2.0 * 0.87;
    let frozen = v("freeze") >= 0.5;

    let mut grid = String::new();
    for st in [-12.0, 0.0, 12.0] {
        seg(&mut grid, left, y_of(st), right, y_of(st));
    }
    if r.rhythm && r.step > 0.0 {
        let mut t = r.step;
        for _ in 0..256 {
            if t >= span {
                break;
            }
            seg(&mut grid, x_of(t), 0.0, x_of(t), h);
            t += r.step;
        }
    }
    c.stroke(grid, ink(0.05), 1.0);

    // Reach shading: Spray .. Position, plus the travel of up-pitched or frozen grains.
    let (xp, xs) = (x_of(r.position), x_of(r.position + r.spray));
    let mut reach = String::new();
    rect(&mut reach, xs, 0.0, xp - xs, h);
    c.fill(reach, ink(0.08));
    if r.travel > 0.0 {
        let mut tint = String::new();
        let xt = x_of((r.position - r.travel).max(0.0));
        rect(&mut tint, xp, 0.0, xt - xp, h);
        c.fill(tint, accent.with_alpha(0.06));
    }

    // Captured input: peak level per 2 px column, mirrored about the centre.
    let now = Instant::now();
    let col = 2.0 * Z;
    let mut cols: Vec<f32> = vec![-120.0; ((right - left) / col) as usize + 1];
    for (at, db) in &live.levels {
        let i = ((right - x_of(now.duration_since(*at).as_secs_f32())) / col) as usize;
        if let Some(c) = cols.get_mut(i) {
            *c = c.max(*db);
        }
    }
    let (mut inside, mut outside) = (String::new(), String::new());
    for (i, db) in cols.iter().enumerate() {
        let a = ((db + 48.0) / 48.0).clamp(0.0, 1.0) * h / 2.0 * 0.72;
        if a >= 0.5 {
            let x = right - (i as f32 + 1.0) * col;
            rect(if x >= xs && x <= xp { &mut inside } else { &mut outside }, x, cy - a, col * 0.8, 2.0 * a);
        }
    }
    c.fill(outside, ink(if frozen { 0.3 } else { 0.18 }));
    c.fill(inside, accent.with_alpha(if frozen { 0.7 } else { 0.5 }));

    // Live grains with a short afterglow.
    let grain_px = (r.length / span * (right - left)).clamp(4.0 * Z, 36.0 * Z);
    for (age, frame) in live.trail.iter().enumerate() {
        let (mut body, mut heads) = (String::new(), String::new());
        let mut alpha = 0.0f32;
        for (back, st, lv) in frame {
            let (x, y) = (x_of(*back), y_of(*st));
            let t = (2.0 + lv * 2.5) * Z;
            capsule(&mut body, x - grain_px, x, y, t);
            if age == 0 {
                circle(&mut heads, x - t / 2.0, y, t * 0.6);
            }
            alpha = alpha.max(0.25 + lv * 0.65);
        }
        c.fill(body, accent.with_alpha(alpha * 0.8f32.powi(age as i32) * if age == 0 { 1.0 } else { 0.45 }));
        c.fill(heads, ink(alpha));
    }

    // Now line, then the Spray (dashed) and Position (solid, with a tab) handles.
    let mut now_line = String::new();
    if frozen {
        dashed_v(&mut now_line, right, 0.0, h, 4.0 * Z, 3.0 * Z);
        c.stroke(now_line, accent, 2.0 * Z);
    } else {
        seg(&mut now_line, right, 0.0, right, h);
        c.stroke(now_line, ink(0.3), 1.0);
    }
    if r.spray > 0.0 {
        let mut sp = String::new();
        dashed_v(&mut sp, xs, 0.0, h, 3.0 * Z, 3.0 * Z);
        c.stroke(sp, ink(if hot == Some(Grab::GrainsSpray) { 0.9 } else { 0.6 }), 1.0 * Z);
    }
    let pos_hot = matches!(hot, Some(Grab::GrainsPosition { .. }));
    let mut pl = String::new();
    seg(&mut pl, xp, 0.0, xp, h);
    c.stroke(pl, ink(0.95), if pos_hot { 2.0 } else { 1.5 } * Z);
    let mut tab = String::new();
    let _ = write!(
        tab,
        "M{:.2} {:.2} h{:.2} v{:.2} a{:.2} {:.2} 0 0 1 {:.2} 0 v{:.2} Z",
        xp - 3.5 * Z,
        6.0 * Z,
        7.0 * Z,
        9.0 * Z,
        3.5 * Z,
        3.5 * Z,
        -7.0 * Z,
        -9.0 * Z
    );
    c.fill(tab, ink(0.95));
    match drag.map(|g| g.grab) {
        Some(Grab::GrainsPosition { .. }) => c.pill(xp + 6.0 * Z, 26.0 * Z, w, "Position", &grains::ms_text(r.position * 1000.0), true),
        Some(Grab::GrainsSpray) => c.pill(xs + 6.0 * Z, 26.0 * Z, w, "Spray", &grains::ms_text(r.spray * 1000.0), true),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delay_taps_follow_feedback_and_ping_pong() {
        let taps = delay_taps(0.25, 0.25, 0.5, 100.0, 4.0);
        assert_eq!(taps[0], (0.25, false, 1.0));
        assert_eq!(taps[1], (0.25, true, 1.0));
        assert!((taps[2].2 - 0.5).abs() < 1e-6);
        // Full ping-pong: the right side starts silent and the repeats alternate.
        let pp = delay_taps(0.25, 0.25, 0.5, 200.0, 4.0);
        assert!(pp.iter().all(|t| t.2 >= 0.02));
        assert_eq!(pp.iter().find(|t| t.1).map(|t| t.0), Some(0.5));
    }

    #[test]
    fn spans_and_rulers() {
        // Dot 1/8 at 120 BPM: 6 echoes need 4.5 beats, so the span is 8 beats (4 s).
        assert_eq!(delay_span(0.375, true, 120.0), 4.0);
        assert_eq!(delay_span(0.5, false, 120.0), 4.0);
        assert_eq!(ruler_step(3.0, 400.0, 56.0), 0.5);
        assert_eq!(seconds_label(0.5), "0.5");
        assert_eq!(seconds_label(2.0), "2");
    }

    #[test]
    fn reverb_laws() {
        assert!((reverb_t60(2.0, 0.5, 1000.0) - 2.0).abs() < 1e-6);
        assert!(reverb_t60(2.0, 1.0, 8000.0) < 1.0);
        assert!(reverb_filter_db(20.0, 20000.0, 1000.0).abs() < 0.02);
        assert!((reverb_filter_db(1000.0, 20000.0, 1000.0) + 3.01).abs() < 0.05);
        // Hall defaults: 0.25 + 10 ms + 2 s * 1.1 = 2.46 s -> a 3 s span.
        assert_eq!(reverb_span(2.0, 0.5, 0.01), 3.0);
    }

    #[test]
    fn biquad_magnitudes() {
        assert!((biquad_db(2, 1000.0, 1.0, 6.0, 1000.0) - 6.0).abs() < 0.05);
        assert!((biquad_db(0, 1000.0, std::f32::consts::FRAC_1_SQRT_2, 0.0, 1000.0) + 3.01).abs() < 0.1);
        assert!(biquad_db(4, 1000.0, std::f32::consts::FRAC_1_SQRT_2, 0.0, 100.0).abs() < 0.05);
        assert!((biquad_db(1, 200.0, std::f32::consts::FRAC_1_SQRT_2, -12.0, 20.0) + 12.0).abs() < 0.3);
    }

    #[test]
    fn grains_reach_matches_the_reference_capture() {
        // Cloud defaults: Position 250 ms + Spray 40 ms fit a 0.5 s span.
        let v = |id: &str| match id {
            "position" => 250.0,
            "spray" => 40.0,
            "size" => 120.0,
            _ => 0.0,
        };
        let r = grains_reach(&v, 120.0);
        assert_eq!(r.span, 0.5);
        assert!((r.position - 0.25).abs() < 1e-6);
    }
}
