//! Graph views of the restoration devices (De-Clipper, De-Esser, De-Hummer,
//! De-Noiser): the `View::Custom` bodies their definitions name. Axes, drawing
//! laws and gestures follow the device specs; the curves are the responses the engine kernels implement.
//!
//! Slint (ui/device_panel.slint, DeviceCanvas with pointer = true) only draws what is built here and
//! forwards pointer events (Studio.panel-graph); hit-testing and the mapping
//! from pointer to parameters live here. A graph gesture sends live values to
//! the engine and commits ONE command on release (a Batch when it moved more
//! than one parameter), so it is one undo step; Escape restores the start
//! values. Alt-press on a zone resets the parameters it edits.
//!
//! The spectrum overlay is shared: kernels append `BANDS` log-spaced band
//! levels to their readouts (engine/src/devices/spectral.hpp) and this module
//! applies the display ballistics (5-tap smoothing, 45 ms rise, 130 ms fall,
//! 42 dB/s decay once frames stop) and maps them onto the graph.

use crate::controller::Controller;
use crate::device_panel::Z;
use crate::{PanelCanvas, PanelGrip, PanelPath, PanelText};
use dstudio_core::builtin::{DeviceDef, ParamDef, View};
use dstudio_core::{Command, Device, DeviceId};
use slint::{Color, ModelRc, SharedString, VecModel};
use std::collections::HashMap;
use std::f64::consts::PI;
use std::fmt::Write;
use std::rc::Rc;
use std::time::{Duration, Instant};

const MINUS: char = '\u{2212}';
/// Spectrum bands at the end of a kernel's readouts (spectral.hpp kSpectrumBands).
pub const BANDS: usize = 48;
const BAND_LO_HZ: f32 = 20.0;
const BAND_HI_HZ: f32 = 20000.0;
/// Spectrum display scale and ballistics (the shared analyser).
const SPEC_FLOOR_DB: f32 = -90.0;
const SPEC_SCALE_DB: f32 = 72.0;
const RISE_MS: f32 = 45.0;
const FALL_MS: f32 = 130.0;
const DECAY_DB_PER_S: f32 = 42.0;
const STALE: Duration = Duration::from_millis(100);
/// Responses are drawn at a fixed drawing rate.
const DRAW_RATE: f64 = 48000.0;
/// Caption zone above the well (pt).
const CAPTION: f32 = 19.0;
const DRAG_SLOP: f32 = 3.0;

/// Which graph a card shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    Declipper,
    Deesser,
    Dehummer,
    Denoiser,
}

impl Kind {
    pub(crate) fn of(def: &DeviceDef) -> Option<Kind> {
        match def.layout.view {
            Some(View::Custom("declipper-wave")) => Some(Kind::Declipper),
            Some(View::Custom("deesser-graph")) => Some(Kind::Deesser),
            Some(View::Custom("dehummer-graph")) => Some(Kind::Dehummer),
            Some(View::Custom("denoiser-graph")) => Some(Kind::Denoiser),
            _ => None,
        }
    }

    fn has_spectrum(self) -> bool {
        self != Kind::Declipper
    }
}

/// A draggable part of a graph.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Zone {
    /// The whole well: a two-axis relative drag (De-Clipper, De-Hummer).
    Plane,
    /// De-Esser band handle (frequency + reduction).
    Band,
    /// De-Esser band edges (bandwidth).
    EdgeLo,
    EdgeHi,
    /// De-Noiser lines.
    Threshold,
    Floor,
}

impl Zone {
    /// Parameters a zone edits.
    fn params(self, kind: Kind) -> &'static [&'static str] {
        match (kind, self) {
            (Kind::Declipper, _) => &["threshold", "ceiling"],
            (Kind::Dehummer, _) => &["frequency", "depth"],
            (Kind::Deesser, Zone::Band) => &["frequency", "reduction"],
            (Kind::Deesser, _) => &["bandwidth"],
            (Kind::Denoiser, Zone::Floor) => &["reduction"],
            (Kind::Denoiser, _) => &["threshold"],
        }
    }
    fn cursor(self, listen: bool) -> i32 {
        match self {
            Zone::Plane => 1,
            Zone::Band if listen => 2,
            Zone::Band => 1,
            Zone::EdgeLo | Zone::EdgeHi => 2,
            Zone::Threshold | Zone::Floor => 3,
        }
    }
}

#[derive(Debug, Clone)]
struct Drag {
    device: DeviceId,
    kind: Kind,
    zone: Zone,
    /// (parameter index, value) at the start and now.
    start: Vec<(usize, f32)>,
    values: Vec<(usize, f32)>,
    last: (f32, f32),
    /// Pointer at the press, and the pointer moved by the fine factor since then.
    press: (f32, f32),
    pos: (f32, f32),
    /// Pointer offset from the dragged handle at the press (keeps the grab point).
    grab: (f32, f32),
    travel: f32,
    active: bool,
}

#[derive(Debug, Clone)]
struct Spectrum {
    shown: [f32; BANDS],
    raw: [f32; BANDS],
    duck: f32,
    changed: Instant,
    polled: Instant,
}

#[derive(Debug, Default)]
pub struct GraphState {
    drag: Option<Drag>,
    hover: Option<(DeviceId, Zone)>,
    spectra: HashMap<DeviceId, Spectrum>,
    /// The last pointer event came from a device window (its menu is hit tested there).
    in_window: bool,
}

/// Plot size (pt) of a restoration graph in its expanded window (src/device_window.rs).
pub(crate) const WIN_PLOT: (f32, f32) = (994.0, 441.0);

thread_local! {
    static IN_WINDOW: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Draws and hit tests the graph at its window size until dropped.
pub(crate) struct WindowScope(bool);

impl WindowScope {
    pub(crate) fn new(on: bool) -> Self {
        Self(IN_WINDOW.with(|c| c.replace(on)))
    }
}

impl Drop for WindowScope {
    fn drop(&mut self) {
        IN_WINDOW.with(|c| c.set(self.0));
    }
}

fn in_window() -> bool {
    IN_WINDOW.with(|c| c.get())
}

/// Plot geometry in well pixels: x is a log frequency axis (or 0..1 for the
/// De-Clipper wave), y a dB axis (or amplitude).
#[derive(Debug, Clone, Copy)]
struct Geo {
    w: f32,
    h: f32,
    x0: f32,
    x1: f32,
    y0: f32,
    y1: f32,
    lo: f32,
    hi: f32,
    top: f32,
    bottom: f32,
}

impl Geo {
    fn x_of(&self, hz: f32) -> f32 {
        self.x0 + (hz / self.lo).ln() / (self.hi / self.lo).ln() * (self.x1 - self.x0)
    }
    fn hz_of(&self, x: f32) -> f32 {
        self.lo * (self.hi / self.lo).powf((x - self.x0) / (self.x1 - self.x0))
    }
    fn y_of(&self, db: f32) -> f32 {
        self.y0 + (self.top - db) / (self.top - self.bottom) * (self.y1 - self.y0)
    }
    fn db_of(&self, y: f32) -> f32 {
        self.top - (y - self.y0) / (self.y1 - self.y0) * (self.top - self.bottom)
    }
}

/// Graph rectangle on the card (pt, card relative): below the picker row when
/// there is one (the view row), else where the picker row would be.
fn placement(def: &DeviceDef) -> (f32, f32, f32, f32) {
    let y = if def.layout.picker.is_empty() { 38.5 } else { 63.0 };
    (8.0, y, def.layout.width() - 16.0, 216.5 - y)
}

fn geo(kind: Kind, def: &DeviceDef) -> Geo {
    let (_, _, w, h) = placement(def);
    let (w, h) = if in_window() { (WIN_PLOT.0 * Z, WIN_PLOT.1 * Z) } else { (w * Z, (h - CAPTION) * Z) };
    let full = |lo, hi, top, bottom| Geo { w, h, x0: 0.0, x1: w, y0: 0.0, y1: h, lo, hi, top, bottom };
    match kind {
        // Wave plot inset 8 pt in the well, amplitude axis +-1.25.
        Kind::Declipper => {
            Geo { w, h, x0: 8.0 * Z, x1: w - 8.0 * Z, y0: 8.0 * Z, y1: h - 8.0 * Z, lo: 1.0, hi: 2.0, top: 1.25, bottom: -1.25 }
        }
        Kind::Deesser => full(1000.0, 20000.0, 0.0, -30.0),
        Kind::Dehummer => full(20.0, 2000.0, 0.0, -60.0),
        Kind::Denoiser => full(20.0, 20000.0, 0.0, -80.0),
    }
}

fn s(v: impl Into<SharedString>) -> SharedString {
    v.into()
}

fn model<T: Clone + 'static>(v: Vec<T>) -> ModelRc<T> {
    ModelRc::from(Rc::new(VecModel::from(v)))
}

fn ink(a: f32) -> Color {
    Color::from_argb_f32(a, 1.0, 1.0, 1.0)
}

fn alpha(c: Color, a: f32) -> Color {
    Color::from_argb_u8((a.clamp(0.0, 1.0) * 255.0).round() as u8, c.red(), c.green(), c.blue())
}

fn none() -> Color {
    Color::from_argb_u8(0, 0, 0, 0)
}

fn layer(commands: String, stroke: Color, width: f32, fill: Color) -> PanelPath {
    PanelPath { commands: s(commands), stroke, width: width * Z, fill, fill2: fill }
}

/// Axis rule: position (well px), label, 0 dB emphasis.
struct Rule {
    pos: f32,
    text: SharedString,
    strong: bool,
}

/// Grip pill on a line; x < 0 places it 8 pt after the previous pill.
struct Pill {
    x: f32,
    y: f32,
    label: &'static str,
    value: String,
    hot: bool,
}

/// Approximate pill width (8 + dots 7 + 5 + label + 5 + value + 8 pt, 11 pt text).
fn pill_width(p: &Pill) -> f32 {
    (33.0 + 6.2 * p.label.chars().count() as f32 + 6.0 * p.value.chars().count() as f32) * Z
}

fn circle(x: f32, y: f32, r: f32) -> String {
    format!("M{:.2} {y:.2} a{r:.2} {r:.2} 0 1 0 {:.2} 0 a{r:.2} {r:.2} 0 1 0 {:.2} 0 Z", x - r, 2.0 * r, -2.0 * r)
}

fn polyline(pts: &[(f32, f32)]) -> String {
    let mut out = String::with_capacity(pts.len() * 16);
    for (i, (x, y)) in pts.iter().enumerate() {
        let _ = write!(out, "{}{x:.2} {y:.2}", if i == 0 { "M" } else { " L" });
    }
    out
}

/// A polyline closed down (or up) to the horizontal line at y.
fn area(pts: &[(f32, f32)], y: f32) -> String {
    match (pts.first(), pts.last()) {
        (Some(a), Some(b)) => format!("{} L{:.2} {y:.2} L{:.2} {y:.2} Z", polyline(pts), b.0, a.0),
        _ => String::new(),
    }
}

/// Dashes along a polyline (dash / gap lengths in px).
fn dashed(pts: &[(f32, f32)], dash: f32, gap: f32) -> String {
    let mut out = String::new();
    let (mut pos, mut on, mut left) = (None::<(f32, f32)>, true, dash);
    for w in pts.windows(2) {
        let (mut a, b) = (w[0], w[1]);
        let mut len = ((b.0 - a.0).powi(2) + (b.1 - a.1).powi(2)).sqrt();
        while len > 0.0 {
            let step = left.min(len);
            let t = step / len;
            let p = (a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t);
            if on {
                if pos.is_none() {
                    let _ = write!(out, "M{:.2} {:.2} ", a.0, a.1);
                }
                let _ = write!(out, "L{:.2} {:.2} ", p.0, p.1);
                pos = Some(p);
            }
            a = p;
            len -= step;
            left -= step;
            if left <= 1e-4 {
                on = !on;
                left = if on { dash } else { gap };
                pos = None;
            }
        }
    }
    out
}

/// Signed dB text with one decimal and a true minus sign ("−40.0 dB", "0.0 dB").
fn db_text(v: f32) -> String {
    let a = (v.abs() * 10.0).round() / 10.0;
    if a < 0.05 {
        "0.0 dB".into()
    } else if v < 0.0 {
        format!("{MINUS}{a:.1} dB")
    } else {
        format!("{a:.1} dB")
    }
}

fn axis_db(v: f32) -> String {
    if v == 0.0 { "0".into() } else { format!("{MINUS}{}", v.abs().round() as i32) }
}

// ---------------------------------------------------------------- responses

#[derive(Debug, Clone, Copy)]
struct C(f64, f64);

impl C {
    fn mul(self, o: C) -> C {
        C(self.0 * o.0 - self.1 * o.1, self.0 * o.1 + self.1 * o.0)
    }
    fn div(self, o: C) -> C {
        let d = o.0 * o.0 + o.1 * o.1;
        C((self.0 * o.0 + self.1 * o.1) / d, (self.1 * o.0 - self.0 * o.1) / d)
    }
    fn abs(self) -> f64 {
        self.0.hypot(self.1)
    }
}

/// Second-order band-pass (0 dB at the centre) or notch at f0 with quality q,
/// evaluated at f: the digital biquad pair whose sum is 1.
fn biquad(f0: f64, q: f64, f: f64, notch: bool) -> C {
    let w0 = 2.0 * PI * f0.min(0.49 * DRAW_RATE) / DRAW_RATE;
    let alpha = w0.sin() / (2.0 * q.max(1e-3));
    let w = 2.0 * PI * f / DRAW_RATE;
    let z1 = C(w.cos(), -w.sin());
    let z2 = C((2.0 * w).cos(), -(2.0 * w).sin());
    let c = w0.cos();
    let den = C(1.0 + alpha - 2.0 * c * z1.0 + (1.0 - alpha) * z2.0, -2.0 * c * z1.1 + (1.0 - alpha) * z2.1);
    let num = if notch { C(1.0 - 2.0 * c * z1.0 + z2.0, -2.0 * c * z1.1 + z2.1) } else { C(alpha * (1.0 - z2.0), -alpha * z2.1) };
    num.div(den)
}

/// De-Esser response in dB at f (the static cut at full Reduction, or the band in Listen).
pub(crate) fn deesser_db(f0: f32, q: f32, reduction: f32, listen: bool, mix: f32, f: f32) -> f32 {
    let bp = biquad(f0 as f64, q as f64, f as f64, false);
    let h = if listen {
        bp.abs()
    } else {
        let cut = mix as f64 * (1.0 - 10f64.powf(-reduction as f64 / 20.0));
        C(1.0 - cut * bp.0, -cut * bp.1).abs()
    };
    (20.0 * h.max(1e-9).log10()) as f32
}

/// The edges of the De-Esser band, where the band-pass is 3 dB down, in Hz: with
/// a = sin(w0) / (2Q), the edges sit at acos(cos(w0) / sqrt(1 + a²)) -+ atan(a).
pub(crate) fn deesser_edges(f0: f32, q: f32) -> (f32, f32) {
    let w0 = 2.0 * PI * f0 as f64 / DRAW_RATE;
    let a = w0.sin() / (2.0 * q.max(1e-4) as f64);
    let centre = (w0.cos() / (1.0 + a * a).sqrt()).clamp(-1.0, 1.0).acos();
    let hz = |w: f64| (w * DRAW_RATE / (2.0 * PI)) as f32;
    (hz((centre - a.atan()).max(0.001)), hz((centre + a.atan()).min(PI * 0.9999)))
}

/// Q that puts a band edge at edge_hz (inverse of [deesser_edges]): at an edge w,
/// |cos(w0) - cos(w)| / sin(w) = sin(w0) / (2Q).
pub(crate) fn deesser_q_for_edge(f0: f32, edge_hz: f32) -> f32 {
    let w0 = 2.0 * PI * f0 as f64 / DRAW_RATE;
    let w = (2.0 * PI * edge_hz.max(1.0) as f64 / DRAW_RATE).min(PI * 0.9999);
    let k = (w0.cos() - w.cos()).abs() / w.sin().max(1e-9);
    (w0.sin() / (2.0 * k.max(1e-9))) as f32
}

/// De-Hummer response in dB at f: the notch product blended with the dry.
pub(crate) fn dehummer_db(f0: f32, harmonics: u32, q: f32, depth: f32, mix: f32, f: f32) -> f32 {
    let a = 1.0 - 10f64.powf(depth as f64 / 20.0);
    let mut prod = C(1.0, 0.0);
    for k in 1..=harmonics.clamp(1, 8) {
        let fc = f0 as f64 * k as f64;
        if fc >= 0.5 * DRAW_RATE {
            break;
        }
        let n = biquad(fc, q as f64, f as f64, true);
        prod = prod.mul(C(1.0 - a + a * n.0, a * n.1));
    }
    let m = mix as f64;
    let h = C(1.0 - m + m * prod.0, m * prod.1).abs();
    (20.0 * h.max(1e-9).log10()) as f32
}

/// De-Clipper drawing law: (processed sample, output peak) for input x.
pub(crate) fn declipper_wave(x: f32, threshold_db: f32, ceiling_db: f32, mix: f32) -> (f32, f32) {
    let t = 10f32.powf(threshold_db / 20.0);
    let c = 10f32.powf(ceiling_db / 20.0);
    let peak = mix + (1.0 - mix) * t;
    let scale = if peak > c { c / peak } else { 1.0 };
    (scale * (mix * x + (1.0 - mix) * x.clamp(-t, t)), scale * peak)
}

fn log_points(lo: f32, hi: f32, n: usize) -> Vec<f32> {
    (0..=n).map(|i| lo * (hi / lo).powf(i as f32 / n as f32)).collect()
}

// ---------------------------------------------------------------- controller

impl Controller {
    fn graph_def(&self, id: DeviceId) -> Option<(&Device, &'static DeviceDef, Kind)> {
        let d = self.project().device(id)?;
        let def = dstudio_core::builtin::def_of(d)?;
        Some((d, def, Kind::of(def)?))
    }

    /// Live value of a parameter during a graph drag.
    pub(crate) fn graph_drag_value(&self, device: DeviceId, index: usize) -> Option<f32> {
        let g = self.devices.panel.graph.drag.as_ref().filter(|g| g.device == device)?;
        g.values.iter().find(|(i, _)| *i == index).map(|(_, v)| *v)
    }

    /// The graph of a built-in card, if its definition names one.
    pub(crate) fn graph_canvas(&self, d: &Device, def: &'static DeviceDef, accent: Color) -> PanelCanvas {
        let Some(kind) = Kind::of(def) else { return PanelCanvas::default() };
        let (x, y, w, h) = placement(def);
        let g = geo(kind, def);
        let v = |id: &str| def.param(id).map_or(0.0, |p| self.panel_value(d, def, p));
        let st = &self.devices.panel.graph;
        let dragging = st.drag.as_ref().filter(|g| g.device == d.id).map(|g| g.zone);
        let hover = st.hover.filter(|(dev, _)| *dev == d.id).map(|(_, z)| z);
        let hot = |z: Zone| dragging.or(hover) == Some(z);
        let spectrum = st.spectra.get(&d.id);
        let well = Color::from_rgb_u8(0x1c, 0x1d, 0x21);
        let (mut layers, mut dots, mut pills, mut hrules, mut vrules) = (Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new());
        let (caption, value);
        let cursor = dragging.or(hover).map_or(0, |z| z.cursor(v("listen") >= 0.5));
        let rungs = |hr: &mut Vec<Rule>, steps: &[f32]| {
            for &db in steps {
                let last = db == g.bottom;
                hr.push(Rule {
                    pos: g.y_of(db).min(g.h - 1.0),
                    text: s(if last { String::new() } else { axis_db(db) }),
                    strong: db == 0.0,
                });
            }
        };
        let grid = |vr: &mut Vec<Rule>, lines: &[(f32, &str)]| {
            for &(hz, t) in lines {
                vr.push(Rule { pos: g.x_of(hz), text: s(t), strong: false });
            }
        };
        match kind {
            Kind::Declipper => {
                let (t, c, mix) = (v("threshold"), v("ceiling"), v("mix") / 100.0);
                let mid = (g.y0 + g.y1) / 2.0;
                let amp = (g.y1 - g.y0) / 2.0 / 1.25;
                for (yy, a) in [(mid - amp, 0.05), (mid + amp, 0.05), (mid, 0.13)] {
                    layers.push(layer(format!("M{:.2} {yy:.2} L{:.2} {yy:.2}", g.x0, g.x1), ink(a), 1.0 / Z, none()));
                }
                let n = 192;
                let xs: Vec<(f32, f32)> = (0..=n)
                    .map(|i| {
                        let u = i as f32 / n as f32;
                        (g.x0 + u * (g.x1 - g.x0), (2.0 * std::f32::consts::TAU * u).sin())
                    })
                    .collect();
                let to_y = |s: f32| mid - s.clamp(-1.25, 1.25) * amp;
                let input: Vec<(f32, f32)> = xs.iter().map(|(px, s)| (*px, to_y(*s))).collect();
                let out: Vec<(f32, f32)> = xs.iter().map(|(px, s)| (*px, to_y(declipper_wave(*s, t, c, mix).0))).collect();
                layers.push(layer(dashed(&input, 3.0 * Z, 3.0 * Z), ink(0.25), 1.0, none()));
                layers.push(layer(area(&out, mid), Color::from_argb_u8(0, 0, 0, 0), 0.0, alpha(accent, 0.18)));
                layers.push(layer(polyline(&out), accent, if hot(Zone::Plane) { 2.0 } else { 1.5 }, none()));
                let peak_db = 20.0 * declipper_wave(1.0, t, c, mix).1.max(1e-6).log10();
                caption = "Peak";
                let a = (peak_db.abs() * 10.0).round() / 10.0;
                value = if peak_db <= -0.05 { format!("{MINUS}{a:.1} dB") } else { format!("+{a:.1} dB") };
            }
            Kind::Deesser => {
                let (f0, q, red, listen, mix) = (v("frequency"), v("bandwidth"), v("reduction"), v("listen") >= 0.5, v("mix") / 100.0);
                rungs(&mut hrules, &[0.0, -10.0, -20.0, -30.0]);
                grid(&mut vrules, &[(2000.0, "2k"), (5000.0, "5k"), (10000.0, "10k")]);
                let (lo, hi) = deesser_edges(f0, q);
                let mut fs = log_points(g.lo, g.hi, 160);
                fs.extend([f0, lo, hi]);
                fs.sort_by(f32::total_cmp);
                let db_at = |f: f32| deesser_db(f0, q, red, listen, mix, f).clamp(g.bottom, 0.0);
                let pts: Vec<(f32, f32)> = fs.iter().map(|&f| (g.x_of(f), g.y_of(db_at(f)))).collect();
                if listen {
                    layers.push(layer(area(&pts, g.y1), Color::from_argb_u8(0, 0, 0, 0), 0.0, alpha(accent, 0.34)));
                } else {
                    layers.push(layer(area(&pts, g.y_of(0.0)), Color::from_argb_u8(0, 0, 0, 0), 0.0, ink(0.07)));
                    let duck = spectrum.map_or(0.0, |s| s.duck);
                    let amount = (-duck / red.max(0.1)).clamp(0.0, 1.0);
                    if amount > 0.01 {
                        layers.push(layer(area(&pts, g.y_of(0.0)), Color::from_argb_u8(0, 0, 0, 0), 0.0, alpha(accent, 0.34 * amount)));
                    }
                }
                layers.push(layer(polyline(&pts), ink(1.0), 1.5, none()));
                for (hz, z) in [(lo, Zone::EdgeLo), (hi, Zone::EdgeHi)] {
                    if hz > g.lo && hz < g.hi {
                        let ex = g.x_of(hz);
                        layers.push(layer(
                            format!("M{ex:.2} 0 L{ex:.2} {:.2}", g.h),
                            ink(if hot(z) { 0.55 } else { 0.24 }),
                            1.0 / Z,
                            none(),
                        ));
                    }
                }
                let dot_fill = if hot(Zone::Band) { Color::from_rgb_u8(0x29, 0x2a, 0x30) } else { well };
                dots.push(PanelPath {
                    commands: s(circle(g.x_of(f0), g.y_of(db_at(f0)), 5.0 * Z)),
                    stroke: ink(1.0),
                    width: 1.5 * Z,
                    fill: dot_fill,
                    fill2: dot_fill,
                });
                caption = "Band";
                value = if f0 >= 1000.0 { format!("{:.2} kHz", f0 / 1000.0) } else { format!("{f0:.0} Hz") };
            }
            Kind::Dehummer => {
                let (f0, harm, q, depth, mix) =
                    (v("frequency"), v("num_harmonics").round().clamp(1.0, 8.0) as u32, v("notch_width"), v("depth"), v("mix") / 100.0);
                rungs(&mut hrules, &[0.0, -20.0, -40.0, -60.0]);
                grid(&mut vrules, &[(50.0, "50"), (100.0, "100"), (200.0, "200"), (500.0, "500"), (1000.0, "1k")]);
                let mut fs = log_points(g.lo, g.hi, 240);
                for k in 1..=harm {
                    let fc = f0 * k as f32;
                    if fc <= g.hi {
                        fs.extend([fc * 0.998, fc, fc * 1.002]);
                    }
                }
                fs.sort_by(f32::total_cmp);
                let pts: Vec<(f32, f32)> =
                    fs.iter().map(|&f| (g.x_of(f), g.y_of(dehummer_db(f0, harm, q, depth, mix, f).clamp(g.bottom, 0.0)))).collect();
                layers.push(layer(polyline(&pts), ink(1.0), if hot(Zone::Plane) { 2.25 } else { 1.5 }, none()));
                caption = "Hum";
                value = format!("{f0:.1} Hz");
            }
            Kind::Denoiser => {
                let (t, red) = (v("threshold"), v("reduction"));
                let floor = (t + red).max(g.bottom);
                rungs(&mut hrules, &[0.0, -20.0, -40.0, -60.0, -80.0]);
                grid(&mut vrules, &[(100.0, "100"), (1000.0, "1k"), (10000.0, "10k")]);
                let (x0, ty, fy) = (26.0 * Z, g.y_of(t), g.y_of(floor));
                let rect = |a: f32, b: f32| format!("M{x0:.2} {a:.2} L{:.2} {a:.2} L{:.2} {b:.2} L{x0:.2} {b:.2} Z", g.w, g.w);
                layers.push(layer(rect(ty, fy), Color::from_argb_u8(0, 0, 0, 0), 0.0, alpha(accent, 0.16)));
                let duck = spectrum.map_or(0.0, |s| s.duck);
                if duck < -0.1 {
                    layers.push(layer(
                        rect(ty, g.y_of((t + duck).max(g.bottom))),
                        Color::from_argb_u8(0, 0, 0, 0),
                        0.0,
                        alpha(accent, 0.3),
                    ));
                }
                layers.push(layer(
                    format!("M{x0:.2} {ty:.2} L{:.2} {ty:.2}", g.w),
                    accent,
                    if hot(Zone::Threshold) { 2.0 } else { 1.5 },
                    none(),
                ));
                layers.push(layer(
                    format!("M{x0:.2} {fy:.2} L{:.2} {fy:.2}", g.w),
                    alpha(accent, 0.45),
                    if hot(Zone::Floor) { 1.5 } else { 1.0 },
                    none(),
                ));
                let inside = |y: f32| y.clamp(9.0 * Z, g.h - 9.0 * Z);
                pills.push(Pill { x: 30.0 * Z, y: inside(ty), label: "Threshold", value: db_text(t), hot: hot(Zone::Threshold) });
                pills.push(Pill { x: -1.0, y: inside(fy), label: "Reduction", value: db_text(red), hot: hot(Zone::Floor) });
                caption = "Noise gate";
                value = String::new();
            }
        }
        let win = in_window();
        if win {
            // The window's ladder and axis: finer rungs and more frequencies, labelled outside.
            let (step, ticks): (f32, &[(f32, &str)]) = match kind {
                Kind::Deesser => {
                    (5.0, &[(2000.0, "2k"), (3000.0, "3k"), (5000.0, "5k"), (7000.0, "7k"), (10000.0, "10k"), (15000.0, "15k")])
                }
                Kind::Dehummer => (
                    6.0,
                    &[
                        (30.0, "30"),
                        (50.0, "50"),
                        (70.0, "70"),
                        (100.0, "100"),
                        (200.0, "200"),
                        (300.0, "300"),
                        (500.0, "500"),
                        (700.0, "700"),
                        (1000.0, "1k"),
                        (2000.0, "2k"),
                    ],
                ),
                _ => (
                    10.0,
                    &[
                        (50.0, "50"),
                        (100.0, "100"),
                        (200.0, "200"),
                        (500.0, "500"),
                        (1000.0, "1k"),
                        (2000.0, "2k"),
                        (5000.0, "5k"),
                        (10000.0, "10k"),
                        (20000.0, "20k"),
                    ],
                ),
            };
            hrules.clear();
            vrules.clear();
            let mut db = g.top;
            while db >= g.bottom - 0.01 {
                let t = if db == 0.0 { "0 dB".to_string() } else { axis_db(db) };
                hrules.push(Rule { pos: g.y_of(db).min(g.h - 1.0), text: s(t), strong: db == 0.0 });
                db -= step;
            }
            for &(hz, t) in ticks {
                vrules.push(Rule { pos: g.x_of(hz).min(g.w - 1.0), text: s(t), strong: false });
            }
        }
        // Draw order: rules, spectrum, layers, axis labels, dots, grip pills.
        let (ww, wh) = if win { (g.w, g.h) } else { (w * Z, (h - CAPTION) * Z) };
        let rule = |commands: String, a: f32| PanelPath { commands: s(commands), stroke: ink(a), width: 1.0, fill: none(), fill2: none() };
        let mut paths: Vec<PanelPath> = Vec::new();
        paths.extend(
            hrules.iter().map(|r| rule(format!("M0 {:.2} L{ww:.2} {:.2}", r.pos + 0.5, r.pos + 0.5), if r.strong { 0.13 } else { 0.05 })),
        );
        paths.extend(vrules.iter().map(|r| rule(format!("M{:.2} 0 L{:.2} {wh:.2}", r.pos + 0.5, r.pos + 0.5), 0.05)));
        if let Some(sp) = spectrum.filter(|_| kind.has_spectrum()) {
            let pts = spectrum_points(&sp.shown, &g);
            if !pts.is_empty() {
                paths.push(layer(area(&pts, g.y1), none(), 0.0, ink(0.14)));
                paths.push(PanelPath { commands: s(polyline(&pts)), stroke: ink(0.5), width: 1.0, fill: none(), fill2: none() });
            }
        }
        paths.extend(layers);
        paths.extend(dots);
        let label_text = |x: f32, y: f32, w: f32, t: &SharedString, align: i32| PanelText {
            x,
            y,
            w,
            h: 12.0 * Z,
            text: t.clone(),
            color: Color::from_argb_u8(0x66, 255, 255, 255),
            size: 9.0 * Z,
            weight: 500,
            align,
        };
        let mut texts: Vec<PanelText> = if win {
            Vec::new()
        } else {
            hrules.iter().filter(|r| !r.text.is_empty()).map(|r| label_text(3.0 * Z, r.pos + 1.0, 40.0 * Z, &r.text, 0)).collect()
        };
        if !win {
            texts.extend(vrules.iter().map(|r| label_text(r.pos - 20.0 * Z, wh - 13.0 * Z, 40.0 * Z, &r.text, 1)));
        }
        let mut grips = Vec::new();
        let mut next_x = 0.0;
        for p in &pills {
            let x = if p.x < 0.0 { next_x } else { p.x };
            next_x = x + pill_width(p) + 8.0 * Z;
            grips.push(PanelGrip { x, y: p.y, label: s(p.label), value: s(p.value.as_str()), hot: p.hot, ..Default::default() });
        }
        if win {
            return self.graph_window_canvas(kind, d, def, accent, (ww, wh), &hrules, &vrules, (caption, value), paths, grips, cursor);
        }
        PanelCanvas {
            visible: true,
            well: true,
            x: x * Z,
            y: y * Z,
            w: ww,
            h: h * Z,
            well_y: CAPTION * Z,
            caption: s(caption),
            readout: s(value),
            paths: model(paths),
            texts: model(texts),
            grips: model(grips),
            zones: model(Vec::new()),
            pointer: true,
            interactive: false,
            cursor,
            readout_accent: true,
            ..Default::default()
        }
    }

    /// The graph at window size: the ladder left of the plot, the axis under it, the caption
    /// above and a legend under the axis, as on the other expanded windows.
    #[allow(clippy::too_many_arguments)]
    fn graph_window_canvas(
        &self,
        kind: Kind,
        d: &Device,
        def: &'static DeviceDef,
        accent: Color,
        (w, h): (f32, f32),
        hrules: &[Rule],
        vrules: &[Rule],
        (caption, value): (&'static str, String),
        mut paths: Vec<PanelPath>,
        grips: Vec<PanelGrip>,
        cursor: i32,
    ) -> PanelCanvas {
        let v = |id: &str| def.param(id).map_or(0.0, |p| self.panel_value(d, def, p));
        let dim = Color::from_argb_u8(0x52, 255, 255, 255);
        let text = |x: f32, y: f32, w: f32, t: SharedString, color: Color, size: f32, align: i32| PanelText {
            x: match align {
                1 => x - w / 2.0,
                2 => x - w,
                _ => x,
            },
            y: y - 8.0 * Z,
            w,
            h: 16.0 * Z,
            text: t,
            color,
            size: size * Z,
            weight: 500,
            align,
        };
        let mut texts: Vec<PanelText> = Vec::new();
        for r in hrules {
            texts.push(text(-8.0 * Z, r.pos, 44.0 * Z, r.text.clone(), dim, 10.0, 2));
        }
        let axis_y = h + 10.0 * Z;
        texts.push(text(6.0 * Z, axis_y, 30.0 * Z, s("Hz"), dim, 10.0, 0));
        for r in vrules {
            let right = r.pos > w - 12.0 * Z;
            texts.push(text(r.pos, axis_y, 40.0 * Z, r.text.clone(), dim, 10.0, if right { 2 } else { 1 }));
        }
        // Caption above the plot.
        let cy = -13.0 * Z;
        let value = if kind == Kind::Denoiser { db_text(v("threshold")) } else { value };
        texts.push(text(0.0, cy, 120.0 * Z, s(caption), Color::from_argb_u8(0x8c, 255, 255, 255), 12.0, 0));
        let at = (crate::text_measure::value_width(caption) + 12.0) * Z;
        texts.push(text(at, cy, 160.0 * Z, s(value), Color::from_argb_u8(0xeb, 255, 255, 255), 13.0, 0));
        // Legend under the axis: the response line, the band it takes out, the signal.
        let spectrum = self.devices.panel.graph.spectra.get(&d.id);
        let level = spectrum.and_then(|sp| sp.shown.iter().copied().reduce(f32::max)).filter(|db| *db > -120.0);
        let signal = match level {
            Some(db) if kind != Kind::Denoiser => format!("Signal  {} dBFS", axis_db(db.round())),
            _ => "Signal".to_string(),
        };
        let items: Vec<(u8, String)> = match kind {
            Kind::Deesser => vec![(0, "Reach".into()), (1, "Removing".into()), (2, signal)],
            Kind::Dehummer => vec![(0, "Notches".into()), (2, signal)],
            _ => vec![(1, "Gate".into()), (2, signal)],
        };
        let ly = h + 31.0 * Z;
        let (glyph, gap) = (14.0 * Z, 6.0 * Z);
        let mut x = 0.0;
        for (k, label) in items {
            let commands = match k {
                0 => format!("M{x:.1} {ly:.1} h{glyph:.1}"),
                _ => format!("M{x:.1} {:.1} h{glyph:.1} v{:.1} h{:.1} Z", ly - 4.0 * Z, 8.0 * Z, -glyph),
            };
            paths.push(match k {
                0 => PanelPath { commands: s(commands), stroke: ink(0.9), width: 1.5 * Z, fill: none(), fill2: none() },
                1 => layer(commands, none(), 0.0, alpha(accent, 0.34)),
                _ => layer(commands, none(), 0.0, ink(0.3)),
            });
            texts.push(text(x + glyph + gap, ly, 140.0 * Z, s(label.as_str()), Color::from_argb_u8(0xb3, 255, 255, 255), 9.0, 0));
            x += glyph + gap + crate::text_measure::value_width(&label) * 0.9 * Z + 16.0 * Z;
        }
        PanelCanvas {
            visible: true,
            w,
            h,
            paths: model(paths),
            texts: model(texts),
            grips: model(grips),
            pointer: true,
            cursor,
            label: s(caption),
            ..Default::default()
        }
    }

    /// The restoration graph drawn for its expanded window.
    pub(crate) fn graph_window(&self, d: &Device, def: &'static DeviceDef, accent: Color) -> PanelCanvas {
        let _w = WindowScope::new(true);
        self.graph_canvas(d, def, accent)
    }

    pub(crate) fn panel_poll_graphs(&mut self) {
        let Some(t) = self.selected_track.and_then(|t| self.project().track(t)) else { return };
        let ids: Vec<(DeviceId, bool, bool)> = t
            .devices
            .iter()
            .filter_map(|d| {
                dstudio_core::builtin::def_of(d)
                    .and_then(Kind::of)
                    .filter(|k| k.has_spectrum())
                    .map(|k| (d.id, k != Kind::Dehummer, d.enabled))
            })
            .collect();
        self.devices.panel.graph.spectra.retain(|id, _| ids.iter().any(|(i, _, _)| i == id));
        let now = Instant::now();
        let audio_running = self.engine.transport().playing || self.rec.recording || self.test_tone_active();
        let mut out = Vec::new();
        let mut dirty = false;
        for (id, has_duck, enabled) in ids {
            if self.engine.builtin_readouts(id.0, &mut out).is_err() || out.len() < BANDS {
                continue;
            }
            let mut raw = [SPEC_FLOOR_DB; BANDS];
            raw.copy_from_slice(&out[out.len() - BANDS..]);
            let duck = if has_duck { out[0].min(0.0) } else { 0.0 };
            let sp = self.devices.panel.graph.spectra.entry(id).or_insert_with(|| Spectrum {
                shown: [SPEC_FLOOR_DB; BANDS],
                raw: [f32::NAN; BANDS],
                duck: 0.0,
                changed: now,
                polled: now,
            });
            let dt = (now - sp.polled).as_secs_f32().min(0.25);
            sp.polled = now;
            if raw != sp.raw {
                sp.raw = raw;
                sp.changed = now;
            }
            let stale = !(audio_running && enabled) && now - sp.changed > STALE;
            let before = sp.shown;
            for (b, r) in sp.shown.iter_mut().zip(raw.iter()) {
                let target = r.max(SPEC_FLOOR_DB);
                *b = if stale {
                    (*b - DECAY_DB_PER_S * dt).max(SPEC_FLOOR_DB)
                } else {
                    let tau = if target > *b { RISE_MS } else { FALL_MS };
                    *b + (target - *b) * (1.0 - (-dt * 1000.0 / tau).exp())
                };
            }
            let d_before = sp.duck;
            sp.duck += (duck - sp.duck) * (1.0 - (-dt / 0.06).exp());
            if (sp.duck - d_before).abs() > 0.02
                || sp.shown.iter().zip(before.iter()).any(|(a, b)| (a - b).abs() > 0.05 && a.max(*b) > -SPEC_SCALE_DB - 1.0)
            {
                dirty = true;
            }
        }
        if dirty {
            self.ui_dirty = true;
        }
    }

    // ---------------------------------------------------------------- gestures

    fn graph_hit(&self, d: &Device, def: &'static DeviceDef, kind: Kind, x: f32, y: f32) -> Option<Zone> {
        let g = geo(kind, def);
        let v = |id: &str| def.param(id).map_or(0.0, |p| self.panel_value(d, def, p));
        if x < 0.0 || y < 0.0 || x > g.w || y > g.h {
            return None;
        }
        match kind {
            Kind::Declipper | Kind::Dehummer => Some(Zone::Plane),
            Kind::Deesser => {
                let (f0, q) = (v("frequency"), v("bandwidth"));
                let dy = deesser_db(f0, q, v("reduction"), v("listen") >= 0.5, v("mix") / 100.0, f0).clamp(g.bottom, 0.0);
                let (hx, hy) = (g.x_of(f0), g.y_of(dy));
                if (x - hx).hypot(y - hy) <= 11.0 * Z {
                    return Some(Zone::Band);
                }
                let (lo, hi) = deesser_edges(f0, q);
                let dist = |hz: f32| if hz > g.lo && hz < g.hi { (x - g.x_of(hz)).abs() } else { f32::INFINITY };
                let (dl, dh) = (dist(lo), dist(hi));
                let tol = 6.0 * Z;
                match (dl <= tol, dh <= tol) {
                    (true, true) => Some(if dl <= dh { Zone::EdgeLo } else { Zone::EdgeHi }),
                    (true, false) => Some(Zone::EdgeLo),
                    (false, true) => Some(Zone::EdgeHi),
                    _ => None,
                }
            }
            Kind::Denoiser => {
                let (t, red) = (v("threshold"), v("reduction"));
                let tol = 7.0 * Z;
                let inside = |y: f32| y.clamp(9.0 * Z, g.h - 9.0 * Z);
                if (y - inside(g.y_of((t + red).max(g.bottom)))).abs() <= tol {
                    Some(Zone::Floor)
                } else if (y - inside(g.y_of(t))).abs() <= tol {
                    Some(Zone::Threshold)
                } else {
                    None
                }
            }
        }
    }

    /// Graph pointer events (Studio.panel-graph). zone -1: hit-test the well; else a pill's zone.
    #[allow(clippy::too_many_arguments)]
    /// Right press on a canvas well: the parameter menu (modulate, map to a macro) of the
    /// handle under the pointer, as a right press on its knob opens.
    pub fn panel_graph_menu(&mut self, device: DeviceId, x: f32, y: f32, wx: f32, wy: f32) {
        let _w = WindowScope::new(self.devices.panel.graph.in_window);
        let param = match self.graph_def(device) {
            Some((d, def, kind)) => self.graph_hit(d, def, kind, x, y).and_then(|z| z.params(kind).iter().find_map(|id| def.index_of(id))),
            None => self.field_param_at(device, x, y),
        };
        if let Some(p) = param {
            self.mod_menu(device, p as i32, wx, wy);
        }
    }

    pub fn panel_graph(&mut self, device: DeviceId, phase: i32, x: f32, y: f32, fine: bool, alt: bool) {
        // Phase + 16: the event comes from the device's expanded window.
        let win = phase >= 16;
        let phase = phase % 16;
        self.devices.panel.graph.in_window = win;
        let _w = WindowScope::new(win);
        let Some((d, def, kind)) = self.graph_def(device) else {
            if matches!(phase, 2 | 3) && self.devices.panel.graph.drag.take().is_some() {
                self.ui_dirty = true;
            }
            return self.panel_field_pointer(device, phase, x, y, fine, alt);
        };
        let zone = self.graph_hit(d, def, kind, x, y);
        match phase {
            0 => {
                let Some(z) = zone else { return };
                let idx: Vec<usize> = z.params(kind).iter().filter_map(|id| def.index_of(id)).collect();
                if alt {
                    let params: Vec<(usize, f32)> = idx.iter().map(|&i| (i, def.params[i].default)).collect();
                    self.graph_commit(device, def, &params);
                    return;
                }
                let start: Vec<(usize, f32)> = idx.iter().map(|&i| (i, def.value(d, &def.params[i]))).collect();
                let g = geo(kind, def);
                let grab = match (kind, z) {
                    (Kind::Deesser, Zone::Band) => {
                        let v = |id: &str| def.param(id).map_or(0.0, |p| def.value(d, p));
                        let f0 = v("frequency");
                        let dy =
                            deesser_db(f0, v("bandwidth"), v("reduction"), v("listen") >= 0.5, v("mix") / 100.0, f0).clamp(g.bottom, 0.0);
                        (x - g.x_of(f0), y - g.y_of(dy))
                    }
                    _ => (0.0, 0.0),
                };
                self.devices.panel.graph.drag = Some(Drag {
                    device,
                    kind,
                    zone: z,
                    values: start.clone(),
                    start,
                    last: (x, y),
                    press: (x, y),
                    pos: (x, y),
                    grab,
                    travel: 0.0,
                    active: false,
                });
                self.devices.panel.graph.hover = Some((device, z));
                self.ui_dirty = true;
            }
            1 => self.graph_move(device, def, x, y, fine),
            2 => {
                let Some(g) = self.devices.panel.graph.drag.take() else { return };
                self.ui_dirty = true;
                if g.device == device && g.active {
                    self.graph_commit(device, def, &g.values);
                }
            }
            3 => {
                self.panel_cancel_graph_drag();
            }
            4 => {
                if self.devices.panel.graph.drag.is_some() {
                    return;
                }
                let next = zone.map(|z| (device, z));
                if self.devices.panel.graph.hover != next {
                    self.devices.panel.graph.hover = next;
                    self.ui_dirty = true;
                }
            }
            _ => {
                if self.devices.panel.graph.drag.is_none() && self.devices.panel.graph.hover.is_some_and(|(dev, _)| dev == device) {
                    self.devices.panel.graph.hover = None;
                    self.ui_dirty = true;
                }
            }
        }
    }

    fn graph_move(&mut self, device: DeviceId, def: &'static DeviceDef, x: f32, y: f32, fine: bool) {
        let Some(mut g) = self.devices.panel.graph.drag.take() else { return };
        if g.device != device {
            self.devices.panel.graph.drag = Some(g);
            return;
        }
        let (dx, dy) = (x - g.last.0, y - g.last.1);
        g.last = (x, y);
        g.travel += dx.abs() + dy.abs();
        if !g.active && g.travel < DRAG_SLOP {
            self.devices.panel.graph.drag = Some(g);
            return;
        }
        g.active = true;
        let geo = geo(g.kind, def);
        let k = if fine { 0.2 } else { 1.0 };
        g.pos = (g.pos.0 + k * dx, g.pos.1 + k * dy);
        let (x, y) = g.pos;
        let get0 = |g: &Drag, id: &str| def.index_of(id).and_then(|i| g.start.iter().find(|(j, _)| *j == i)).map(|(_, v)| *v);
        let get = |g: &Drag, id: &str| def.index_of(id).and_then(|i| g.values.iter().find(|(j, _)| *j == i)).map(|(_, v)| *v);
        let cur = |id: &str| def.param(id).map_or(0.0, |p| self.project().device(device).map_or(p.default, |d| def.value(d, p)));
        let set = |g: &mut Drag, id: &str, v: f32| {
            if let (Some(i), Some(p)) = (def.index_of(id), def.param(id)) {
                let v = p.snap(v);
                if let Some(e) = g.values.iter_mut().find(|(j, _)| *j == i) {
                    e.1 = v;
                }
            }
        };
        let nudge = |p: Option<&ParamDef>, v: f32, dn: f32| p.map_or(v, |p| p.from_normalized(p.to_normalized(v) + dn));
        match (g.kind, g.zone) {
            (Kind::Declipper, _) => {
                let (t, c) = (get(&g, "threshold").unwrap_or(0.0), get(&g, "ceiling").unwrap_or(0.0));
                if dx != 0.0 {
                    set(&mut g, "threshold", nudge(def.param("threshold"), t, k * dx / geo.w));
                }
                if dy != 0.0 {
                    set(&mut g, "ceiling", nudge(def.param("ceiling"), c, -k * dy / ((geo.y1 - geo.y0) * 0.2)));
                }
            }
            (Kind::Dehummer, _) => {
                let (f, depth) = (get(&g, "frequency").unwrap_or(60.0), get(&g, "depth").unwrap_or(-30.0));
                if dx != 0.0 {
                    set(&mut g, "frequency", f * (geo.hi / geo.lo).powf(k * dx / (geo.x1 - geo.x0)));
                }
                if dy != 0.0 {
                    set(&mut g, "depth", depth - k * dy / (geo.y1 - geo.y0) * (geo.top - geo.bottom));
                }
            }
            (Kind::Deesser, Zone::Band) => {
                let grab = g.grab;
                set(&mut g, "frequency", geo.hz_of(x - grab.0));
                if cur("listen") < 0.5 {
                    let mix = cur("mix") / 100.0;
                    if mix > 1e-3 {
                        let lin = 10f32.powf(geo.db_of(y - grab.1).clamp(geo.bottom, 0.0) / 20.0);
                        let gain = ((lin - (1.0 - mix)) / mix).clamp(1e-6, 1.0);
                        set(&mut g, "reduction", -20.0 * gain.log10());
                    }
                }
            }
            (Kind::Deesser, _) => set(&mut g, "bandwidth", deesser_q_for_edge(cur("frequency"), geo.hz_of(x))),
            (Kind::Denoiser, zone) => {
                let delta = -(y - g.press.1) / (geo.y1 - geo.y0) * (geo.top - geo.bottom);
                let id = if zone == Zone::Floor { "reduction" } else { "threshold" };
                let v = get0(&g, id).unwrap_or(0.0) + delta;
                set(&mut g, id, if zone == Zone::Floor { v.min(0.0) } else { v });
            }
        }
        for &(i, v) in &g.values {
            let _ = self.engine.builtin_set_param(device.0, i as u32, v);
        }
        self.devices.panel.graph.drag = Some(g);
        self.ui_dirty = true;
    }

    /// Commit changed values as one undo step.
    fn graph_commit(&mut self, device: DeviceId, def: &'static DeviceDef, values: &[(usize, f32)]) {
        let Some(d) = self.project().device(device) else { return };
        let mut commands: Vec<Command> = values
            .iter()
            .filter(|(i, v)| def.params.get(*i).is_some_and(|p| def.value(d, p) != *v))
            .map(|&(i, v)| Command::SetDeviceParam { device, param: def.params[i].id.to_string(), value: v })
            .collect();
        match commands.len() {
            0 => {}
            1 => {
                self.panel_commit(commands.remove(0));
            }
            _ => {
                self.panel_commit(Command::Batch { label: "Device Parameter Change".into(), commands });
            }
        }
    }

    /// Escape or a cancelled pointer: restore the values from before the graph drag.
    pub fn panel_cancel_graph_drag(&mut self) -> bool {
        let Some(g) = self.devices.panel.graph.drag.take() else { return false };
        self.ui_dirty = true;
        if self.project().device(g.device).is_none() {
            return false;
        }
        for &(i, v) in &g.start {
            let _ = self.engine.builtin_set_param(g.device.0, i as u32, v);
        }
        self.ui_dirty = true;
        true
    }
}

/// Spectrum trace points: 5-tap smoothing across bands, then the -72..0 dB
/// display scale over the plot height; empty when everything is under it.
fn spectrum_points(shown: &[f32; BANDS], g: &Geo) -> Vec<(f32, f32)> {
    let at = |i: isize| shown[i.clamp(0, BANDS as isize - 1) as usize];
    let sm: Vec<f32> =
        (0..BANDS as isize).map(|i| (at(i - 2) + 4.0 * at(i - 1) + 6.0 * at(i) + 4.0 * at(i + 1) + at(i + 2)) / 16.0).collect();
    if sm.iter().all(|v| *v <= -SPEC_SCALE_DB) {
        return Vec::new();
    }
    let band_hz = |i: usize| BAND_LO_HZ * (BAND_HI_HZ / BAND_LO_HZ).powf(i as f32 / (BANDS - 1) as f32);
    let y = |db: f32| g.y0 + (-db / SPEC_SCALE_DB).clamp(0.0, 1.0) * (g.y1 - g.y0);
    let mut pts = Vec::new();
    for (i, &level) in sm.iter().enumerate() {
        let f = band_hz(i);
        let prev = if i > 0 { band_hz(i - 1) } else { 0.0 };
        let next = if i + 1 < BANDS { band_hz(i + 1) } else { f32::MAX };
        // Bands inside the axis, plus one neighbour on each side clipped to the edge.
        if next < g.lo || prev > g.hi {
            continue;
        }
        pts.push((g.x_of(f).clamp(g.x0, g.x1), y(level)));
    }
    pts
}

#[cfg(test)]
mod tests {
    use super::*;

    fn def(id: &str) -> &'static DeviceDef {
        dstudio_core::builtin::find(id).unwrap()
    }

    #[test]
    fn every_restoration_device_names_a_graph() {
        for (id, kind) in
            [("declipper", Kind::Declipper), ("deesser", Kind::Deesser), ("dehummer", Kind::Dehummer), ("denoiser", Kind::Denoiser)]
        {
            assert_eq!(Kind::of(def(id)), Some(kind));
        }
        assert_eq!(Kind::of(def("compressor")), None);
    }

    #[test]
    fn graph_placement_follows_the_picker_row() {
        assert_eq!(placement(def("declipper")), (8.0, 63.0, 228.0, 153.5));
        assert_eq!(placement(def("deesser")), (8.0, 38.5, 455.0, 178.0));
    }

    #[test]
    fn deesser_response_is_the_band_cut() {
        // At the centre: 1 - mix * (1 - g) * 1 = g -> -8 dB at full mix.
        assert!((deesser_db(7000.0, 1.0, 8.0, false, 1.0, 7000.0) + 8.0).abs() < 1e-3);
        assert!(deesser_db(7000.0, 1.0, 8.0, false, 1.0, 1000.0) > -0.6);
        // Half mix at the centre: 0.5 + 0.5 * g.
        let want = 20.0 * (0.5 + 0.5 * 10f32.powf(-8.0 / 20.0)).log10();
        assert!((deesser_db(7000.0, 1.0, 8.0, false, 0.5, 7000.0) - want).abs() < 1e-3);
        // Listen draws the band itself: 0 dB at the centre.
        assert!(deesser_db(7000.0, 1.0, 8.0, true, 1.0, 7000.0).abs() < 1e-3);
        assert!(deesser_db(7000.0, 1.0, 8.0, true, 1.0, 1500.0) < -10.0);
    }

    #[test]
    fn deesser_edges_invert() {
        for q in [0.3f32, 1.0, 2.5] {
            let (lo, hi) = deesser_edges(7000.0, q);
            assert!(lo < 7000.0 && hi > 7000.0);
            assert!((deesser_q_for_edge(7000.0, hi) - q).abs() < 1e-3, "q {q}");
            if lo > 1.0 {
                assert!((deesser_q_for_edge(7000.0, lo) - q).abs() < 1e-3, "q {q}");
            }
        }
    }

    #[test]
    fn dehummer_response_reaches_depth_at_each_notch() {
        for k in 1..=4 {
            let db = dehummer_db(60.0, 4, 5.0, -30.0, 1.0, 60.0 * k as f32);
            assert!(db < -29.0 && db > -31.5, "harmonic {k}: {db}");
        }
        assert!(dehummer_db(60.0, 4, 5.0, -30.0, 1.0, 1500.0) > -0.3);
        assert!(dehummer_db(60.0, 4, 5.0, -30.0, 0.0, 60.0).abs() < 1e-4);
    }

    #[test]
    fn declipper_picture_and_peak_caption() {
        // Defaults: Mix 100 % leaves the sine, scaled to the -0.3 dB ceiling (caption "−0.3 dB").
        let (y, peak) = declipper_wave(1.0, -1.0, -0.3, 1.0);
        assert!((20.0 * peak.log10() + 0.3).abs() < 1e-4);
        assert!((y - peak).abs() < 1e-6);
        // Mix 0: a hard clip at the threshold, normalised to the ceiling.
        let (y, peak) = declipper_wave(1.0, -6.0, 0.0, 0.0);
        assert!((peak - 10f32.powf(-6.0 / 20.0)).abs() < 1e-5);
        assert!((y - peak).abs() < 1e-6);
    }

    #[test]
    fn dashes_and_text() {
        assert_eq!(db_text(-40.0), "\u{2212}40.0 dB");
        assert_eq!(db_text(-0.02), "0.0 dB");
        assert_eq!(axis_db(-20.0), "\u{2212}20");
        let d = dashed(&[(0.0, 0.0), (10.0, 0.0)], 3.0, 3.0);
        assert_eq!(d.matches('M').count(), 2);
    }

    #[test]
    fn spectrum_trace_maps_bands_to_the_axis() {
        let g = geo(Kind::Denoiser, def("denoiser"));
        let mut bands = [SPEC_FLOOR_DB; BANDS];
        assert!(spectrum_points(&bands, &g).is_empty(), "silence draws nothing");
        bands[24] = -12.0;
        let pts = spectrum_points(&bands, &g);
        assert_eq!(pts.len(), BANDS);
        assert!(pts.iter().all(|(x, y)| *x >= g.x0 && *x <= g.x1 && *y >= g.y0 && *y <= g.y1));
    }

    /// Per-refresh cost of building each graph (`cargo test -p dstudio-app --release bench_graphs -- --ignored --nocapture`).
    #[test]
    #[ignore]
    fn bench_graphs() {
        let n = 2000;
        let t0 = Instant::now();
        for i in 0..n {
            for k in 1..=4 {
                std::hint::black_box(dehummer_db(60.0, 8, 5.0, -30.0, 1.0, 20.0 + (i * k) as f32));
            }
        }
        let hum = t0.elapsed().as_secs_f64() * 1e9 / (n * 4) as f64;
        let t0 = Instant::now();
        for i in 0..n {
            std::hint::black_box(deesser_db(7000.0, 1.0, 8.0, false, 1.0, 1000.0 + i as f32));
        }
        let ess = t0.elapsed().as_secs_f64() * 1e9 / n as f64;
        let g = geo(Kind::Denoiser, def("denoiser"));
        let bands = [-30.0f32; BANDS];
        let t0 = Instant::now();
        for _ in 0..n {
            std::hint::black_box(spectrum_points(std::hint::black_box(&bands), &g));
        }
        let spec = t0.elapsed().as_secs_f64() * 1e6 / n as f64;
        println!("graph model: de-hummer point (8 notches) {hum:.0} ns, de-esser point {ess:.0} ns, spectrum trace {spec:.2} us");
    }
}
