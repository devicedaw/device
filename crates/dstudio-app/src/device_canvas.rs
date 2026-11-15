//! Field views of built-in device panels: the EQ body (View::Eq: band tabs,
//! response field with band nodes, type picker, value cells) and the Auto
//! Filter response field (View::Custom("auto-filter")).
//!
//! A field is drawn from paths, dots, texts and value pills built here in
//! well-local px (crate::PanelCanvas); the Slint side only draws them and
//! forwards pointer, wheel and key events back (Controller::panel_canvas...).
//! Hit-testing and gestures live here, next to the geometry that drew the
//! field. A field drag can move several parameters at once (an EQ node moves
//! frequency and gain): values go live to the engine while dragging
//! (se_builtin_set_param) and ONE command batch commits them on release, so
//! a gesture is one undo step. Escape / cancel restores the start values.

use crate::controller::Controller;
use crate::device_panel::Z;
use crate::{EqCell, EqGlyph, EqTab, PanelCanvas, PanelDot, PanelEq, PanelGrip, PanelPath, PanelText};
use dstudio_core::builtin::eq::{self, EqView, NodeDoubleClick, Shape};
use dstudio_core::builtin::{DeviceDef, FilterSetting, ParamDef, View, dj_map};
use dstudio_core::{Command, Device, DeviceId};
use slint::{Color, ModelRc, SharedString, VecModel};
use std::collections::HashMap;
use std::rc::Rc;

/// Card padding, gap between a field and the slider column (pt).
const PAD: f32 = 8.0;
const VIEW_GAP: f32 = 10.0;
/// EQ: field rows (card relative pt), band tab and type picker origin.
const EQ_FIELD_Y: f32 = 71.0;
const EQ_FIELD_BOTTOM: f32 = 274.0;
const EQ_TAB_X: f32 = 42.0;
pub const EQ_SLIDER_TOP: f32 = 75.0;
pub const EQ_SLIDER_BOTTOM: f32 = 235.0;
/// Left gutter of the dB labels (rules start after it).
const GUTTER: f32 = 28.0;
/// Auto Filter field (card relative pt): canvas, caption strip, axis strip, dB window.
const AF_Y: f32 = 63.0;
const AF_BOTTOM: f32 = 215.0;
const AF_CAPTION: f32 = 19.0;
const AF_AXIS: f32 = 15.0;
const AF_TOP_DB: f32 = 24.0;
const AF_BOTTOM_DB: f32 = -30.0;
/// Pointer travel before a press becomes a drag (px).
const DRAG_SLOP: f32 = 3.0;
const MINUS: char = '\u{2212}';

fn s(v: impl Into<SharedString>) -> SharedString {
    v.into()
}

fn model<T: Clone + 'static>(v: Vec<T>) -> ModelRc<T> {
    ModelRc::from(Rc::new(VecModel::from(v)))
}

fn rgba(c: Color, a: f32) -> Color {
    Color::from_argb_u8((a.clamp(0.0, 1.0) * 255.0).round() as u8, c.red(), c.green(), c.blue())
}

fn white(a: f32) -> Color {
    rgba(Color::from_rgb_u8(255, 255, 255), a)
}

const AXIS: Color = Color::from_rgb_u8(0x8f, 0x8f, 0x93);

/// What the pointer is over.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Hit {
    None,
    /// EQ band node.
    Node(usize),
    /// Q wing of an EQ band.
    Wing(usize),
    /// Empty EQ field (Character EQ creates a band there).
    Empty,
    /// Auto Filter corner handle (frequency / DJ position + resonance).
    Corner,
    /// Auto Filter sweep edge (true = high edge): LFO amount.
    Edge(bool),
    /// Auto Filter envelope pill: Env amount.
    Env,
}

/// A field drag: start values of the parameters it moves and their live values.
#[derive(Debug, Clone)]
struct FieldDrag {
    device: DeviceId,
    hit: Hit,
    /// Press point and the anchor the deltas run from (moved when Shift toggles).
    x0: f32,
    y0: f32,
    ax: f32,
    ay: f32,
    /// Node centre minus press point (EQ: the node does not jump to the pointer).
    grab: (f32, f32),
    start: Vec<(usize, f32)>,
    anchor: Vec<(usize, f32)>,
    live: Vec<(usize, f32)>,
    active: bool,
    fine: bool,
    alt: bool,
    /// Commit even without movement (a press that switched a band on).
    force: bool,
}

#[derive(Debug, Default)]
pub(crate) struct CanvasUi {
    hover: Option<(DeviceId, Hit)>,
    drag: Option<FieldDrag>,
    /// Selected EQ band per device (UI state, not saved).
    selected: HashMap<DeviceId, usize>,
    wheel: f32,
    /// Latest engine readouts of field devices (Character EQ dynamics, Auto Filter cutoff).
    pub(crate) readouts: HashMap<DeviceId, Vec<f32>>,
    /// The pointer events in flight come from a device window's display (its geometry maps
    /// them), not from the card.
    pub(crate) window: bool,
}

impl CanvasUi {
    /// Live value of parameter `index` while a field drag moves it.
    pub(crate) fn live(&self, device: DeviceId, index: usize) -> Option<f32> {
        let g = self.drag.as_ref().filter(|g| g.device == device)?;
        g.live.iter().find(|(i, _)| *i == index).map(|(_, v)| *v)
    }
}

/// Plot mapping in well-local px: log frequency 20 Hz..20 kHz across the
/// width, dB from top to bottom.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Plot {
    pub w: f32,
    pub h: f32,
    pub top_db: f32,
    pub bottom_db: f32,
    /// Strip along the bottom kept for the frequency labels (a card's Auto Filter field).
    pub axis: f32,
    /// Height above the plot's bottom where a switched-off band's node waits.
    pub park: f32,
}

impl Plot {
    pub fn x(&self, f: f64) -> f32 {
        (self.w as f64 * ((f.max(1.0) / 20.0).ln() / 1000f64.ln())) as f32
    }
    pub fn f(&self, x: f32) -> f64 {
        20.0 * 1000f64.powf((x / self.w).clamp(0.0, 1.0) as f64)
    }
    pub fn y(&self, db: f32) -> f32 {
        (self.top_db - db) / (self.top_db - self.bottom_db) * self.h
    }
    pub fn db(&self, y: f32) -> f32 {
        self.top_db - y / self.h * (self.top_db - self.bottom_db)
    }
}

fn path(pts: &[(f32, f32)]) -> String {
    use std::fmt::Write;
    let mut out = String::with_capacity(pts.len() * 16);
    for (i, (x, y)) in pts.iter().enumerate() {
        let _ = write!(out, "{}{x:.1} {y:.1}", if i == 0 { "M" } else { " L" });
    }
    out
}

/// Closed area between a curve and the horizontal line at `base`.
fn area(pts: &[(f32, f32)], base: f32) -> String {
    // A flat area has nothing to fill (and a zero-height fill leaves rendering fringes).
    if pts.len() < 2 || pts.iter().all(|p| (p.1 - base).abs() < 0.5) {
        return String::new();
    }
    format!("{} L{:.1} {base:.1} L{:.1} {base:.1} Z", path(pts), pts[pts.len() - 1].0, pts[0].0)
}

/// A polyline as dashes of `on` px with `off` px gaps along its length.
fn dashed(pts: &[(f32, f32)], on: f32, off: f32) -> String {
    use std::fmt::Write;
    let mut out = String::new();
    let mut phase = 0.0f32; // distance into the current on + off period
    for w in pts.windows(2) {
        let ((x0, y0), (x1, y1)) = (w[0], w[1]);
        let len = (x1 - x0).hypot(y1 - y0);
        let mut t = 0.0f32;
        while t < len {
            let drawing = phase < on;
            let step = if drawing { on - phase } else { on + off - phase }.min(len - t);
            if drawing && step > 0.0 {
                let (a, b) = (t / len, (t + step) / len);
                let _ = write!(
                    out,
                    "M{:.1} {:.1} L{:.1} {:.1} ",
                    x0 + (x1 - x0) * a,
                    y0 + (y1 - y0) * a,
                    x0 + (x1 - x0) * b,
                    y0 + (y1 - y0) * b
                );
            }
            t += step;
            phase = (phase + step) % (on + off);
            if step <= 0.0 {
                break;
            }
        }
    }
    out
}

fn vline(x: f32, y0: f32, y1: f32) -> String {
    format!("M{x:.1} {y0:.1} L{x:.1} {y1:.1}")
}

fn hline(y: f32, x0: f32, x1: f32) -> String {
    format!("M{x0:.1} {y:.1} L{x1:.1} {y:.1}")
}

fn stroke(commands: String, color: Color, width: f32) -> PanelPath {
    PanelPath { commands: s(commands), stroke: color, width, fill: Color::from_argb_u8(0, 0, 0, 0), fill2: Color::from_argb_u8(0, 0, 0, 0) }
}

fn fill(commands: String, color: Color) -> PanelPath {
    PanelPath { commands: s(commands), stroke: Color::from_argb_u8(0, 0, 0, 0), width: 0.0, fill: color, fill2: color }
}

/// Text whose x is its left (align 0), centre (1) or right (2) edge and y its centre line.
fn text(x: f32, y: f32, w: f32, t: impl Into<SharedString>, color: Color, size: f32, align: i32) -> PanelText {
    let left = match align {
        0 => x,
        1 => x - w / 2.0,
        _ => x - w,
    };
    PanelText { x: left, y: y - size * 0.7, w, h: size * 1.4, text: t.into(), color, size, weight: 600, align }
}

fn db_label(db: f32) -> String {
    if db > 0.0 {
        format!("+{db:.0}")
    } else if db < 0.0 {
        format!("{MINUS}{:.0}", -db)
    } else {
        "0".into()
    }
}

/// Grid of a response field: dB rungs (labels in the left gutter) and the
/// 100 / 1k / 10k rules with their labels along the bottom.
fn grid(p: &Plot, rungs: &[f32], axis_strip: f32, paths: &mut Vec<PanelPath>, texts: &mut Vec<PanelText>) {
    for &db in rungs {
        let y = p.y(db);
        let rule = if db == 0.0 { white(0.13) } else { white(0.05) };
        paths.push(stroke(hline(y, GUTTER * Z, p.w), rule, 1.0));
        texts.push(text(3.0 * Z, y - 5.5 * Z, 30.0 * Z, db_label(db), AXIS, 10.0 * Z, 0));
    }
    for (f, label) in [(100.0, "100"), (1000.0, "1k"), (10_000.0, "10k")] {
        let x = p.x(f);
        paths.push(stroke(vline(x, 0.0, p.h - axis_strip), white(0.05), 1.0));
        texts.push(text(x, p.h - 8.0 * Z, 30.0 * Z, label, AXIS, 10.0 * Z, 1));
    }
    texts.push(text(3.0 * Z, p.h - 8.0 * Z, 20.0 * Z, "Hz", AXIS, 10.0 * Z, 0));
}

/// Rungs of a window's response ruler: the top, then every 6 dB (12 dB on a span over 60).
fn win_ticks(top: f32, bottom: f32) -> Vec<f32> {
    let step = if top - bottom > 60.0 { 12.0 } else { 6.0 };
    let mut out = vec![top];
    let mut db = (top / step).floor() * step;
    while db >= bottom - 0.01 {
        if (db - top).abs() > 0.01 {
            out.push(db);
        }
        db -= step;
    }
    out
}

/// Chrome of a window's response plot: the frame's top and bottom rules, dB rungs (the
/// 0 dB one brighter) with their ruler left of the plot, the frequency rules with the axis
/// under the plot, its unit names and the transport word ("stopped") at the axis's end.
fn win_grid(p: &Plot, ticks: &[f32], stopped: bool, paths: &mut Vec<PanelPath>, texts: &mut Vec<PanelText>) {
    let (lo, mid, rule) = (white(0.3), white(0.55), white(0.06));
    paths.push(stroke(hline(0.5, 0.0, p.w), rule, 1.0));
    paths.push(stroke(hline(p.h - 0.5, 0.0, p.w), rule, 1.0));
    let (top, bottom) = (ticks.first().copied().unwrap_or(0.0), ticks.last().copied().unwrap_or(0.0));
    for &db in ticks {
        let y = p.y(db).round() + 0.5;
        if db != top && db != bottom {
            paths.push(stroke(hline(y, 0.0, p.w), if db == 0.0 { white(0.55 * 0.32) } else { rule }, 1.0));
        }
        texts.push(text(-6.0 * Z, y, 40.0 * Z, db_label(db), if db == 0.0 { mid } else { lo }, 11.0 * Z, 2));
    }
    let axis_y = p.h + 11.0 * Z;
    texts.push(text(-6.0 * Z, axis_y, 30.0 * Z, "dB", lo, 10.0 * Z, 2));
    for f in [50.0, 100.0, 200.0, 500.0, 1000.0, 2000.0, 5000.0, 10_000.0] {
        let x = p.x(f).round() + 0.5;
        paths.push(stroke(vline(x, 0.0, p.h), rule, 1.0));
        let label = if f >= 1000.0 { format!("{}k", f / 1000.0) } else { format!("{f}") };
        texts.push(text(x, axis_y, 40.0 * Z, label, lo, 10.0 * Z, 1));
    }
    texts.push(text(0.0, axis_y, 30.0 * Z, "Hz", lo, 10.0 * Z, 0));
    if stopped {
        texts.push(text(p.w, axis_y, 60.0 * Z, "stopped", mid, 10.0 * Z, 2));
    }
}

/// Legend entry glyphs of a window's response plot.
#[derive(Clone, Copy)]
enum Legend {
    /// The selected band's own curve (dotted).
    Band,
    /// Dynamic bands' reach (a region).
    Dynamic,
    /// The Auto Filter's sweep band (dimmed while it is not drawn).
    Sweep(bool),
    /// The response curve.
    Response,
}

/// The legend row under the axis: a glyph and a name per entry.
fn win_legend(p: &Plot, entries: &[(Legend, &str)], accent: Color, paths: &mut Vec<PanelPath>, texts: &mut Vec<PanelText>) {
    let y = p.h + 30.0 * Z;
    let (glyph, gap, entry_gap) = (14.0 * Z, 6.0 * Z, 16.0 * Z);
    let mut x = 0.0;
    for (kind, label) in entries {
        let dim = |c: Color, on: bool| if on { c } else { rgba(c, c.alpha() as f32 / 255.0 * 0.35) };
        let on = match kind {
            Legend::Band => {
                let mut d = String::new();
                let mut gx = x;
                while gx < x + glyph {
                    d.push_str(&format!("M{gx:.1} {y:.1} L{:.1} {y:.1} ", (gx + 1.5 * Z).min(x + glyph)));
                    gx += 3.5 * Z;
                }
                paths.push(stroke(d, rgba(accent, 0.6), 1.25 * Z));
                true
            }
            Legend::Dynamic => {
                paths.push(fill(format!("M{x:.1} {:.1} h{glyph:.1} v{:.1} h{:.1} Z", y - 4.0 * Z, 8.0 * Z, -glyph), rgba(accent, 0.18)));
                false
            }
            Legend::Sweep(shown) => {
                paths.push(fill(
                    format!("M{x:.1} {:.1} h{glyph:.1} v{:.1} h{:.1} Z", y - 4.0 * Z, 8.0 * Z, -glyph),
                    dim(rgba(accent, 0.21), *shown),
                ));
                paths.push(stroke(
                    format!("{} {}", vline(x + 0.5, y - 4.0 * Z, y + 4.0 * Z), vline(x + glyph - 0.5, y - 4.0 * Z, y + 4.0 * Z)),
                    dim(rgba(accent, 0.26), *shown),
                    1.0,
                ));
                *shown
            }
            Legend::Response => {
                paths.push(stroke(hline(y, x, x + glyph), accent, 1.75 * Z));
                true
            }
        };
        let tx = x + glyph + gap;
        let w = crate::text_measure::value_width(label) * Z;
        texts.push(text(tx, y, w + 10.0 * Z, *label, dim(MUTE_TEXT, on), 10.0 * Z, 0));
        x = tx + w + entry_gap;
    }
}

const MUTE_TEXT: Color = Color::from_rgb_u8(0xa6, 0xa8, 0xad);

// ---------------------------------------------------------------- EQ

/// Half width (octaves) of a band's Q wing.
fn wing_octaves(q: f32) -> f32 {
    1.0 / (2.0 * q.max(0.05))
}

/// Mini glyph (20 x 12 view box) of a band shape for the type picker.
pub(crate) fn glyph(shape: Shape) -> String {
    let mut b = eq::Band::new(shape, 1000.0, 9.0, std::f32::consts::FRAC_1_SQRT_2);
    match shape {
        Shape::Bell => b.q = 1.0,
        Shape::Notch => b.q = 4.0,
        Shape::DualLow => {
            b.freq = 300.0;
            b.low_atten_db = 12.0;
        }
        _ => {}
    }
    let pts: Vec<(f32, f32)> = (0..=28)
        .map(|i| {
            let x = 20.0 * i as f32 / 28.0;
            let f = 20.0 * 1000f64.powf(x as f64 / 20.0);
            let db = eq::band_db(&b, f, 48_000.0) as f32;
            (x, (6.0 - db * 5.0 / 12.0).clamp(0.6, 11.4))
        })
        .collect();
    path(&pts)
}

/// Text of a pass slope choice ("24 dB/oct" or "24" -> "24 dB/oct").
fn slope_text(p: &ParamDef, v: f32) -> String {
    let name = p.choices.get(v.round().max(0.0) as usize).copied().unwrap_or("");
    if name.contains("dB") { name.to_string() } else { format!("{name} dB/oct") }
}

struct Node {
    band: usize,
    x: f32,
    y: f32,
    r: f32,
    on: bool,
}

impl Controller {
    fn field_value(&self, d: &Device, def: &DeviceDef, id: &str) -> f32 {
        def.param(id).map(|p| self.panel_value(d, def, p)).unwrap_or(0.0)
    }

    /// Plot of the surface the pointer events in flight come from (card or window).
    fn eq_plot(&self, def: &DeviceDef, v: &EqView) -> (f32, f32, f32, Plot) {
        self.eq_plot_on(def, v, self.devices.panel.canvas.window)
    }

    fn eq_plot_on(&self, def: &DeviceDef, v: &EqView, win: bool) -> (f32, f32, f32, Plot) {
        if win && let Some((w, h)) = crate::device_window::plot_size(def) {
            return (0.0, 0.0, h, Plot { w, h, top_db: v.top_db, bottom_db: v.bottom_db, axis: 0.0, park: 1.0 * Z });
        }
        let width = def.layout.width();
        let right = width - PAD - v.slider_w - VIEW_GAP;
        let (x, y) = (PAD * Z, EQ_FIELD_Y * Z);
        let (w, h) = ((right - PAD) * Z, (EQ_FIELD_BOTTOM - EQ_FIELD_Y) * Z);
        (x, y, h, Plot { w, h, top_db: v.top_db, bottom_db: v.bottom_db, axis: 0.0, park: 19.0 * Z })
    }

    pub(crate) fn eq_selected(&self, d: &Device, v: &EqView) -> usize {
        self.devices.panel.canvas.selected.get(&d.id).copied().unwrap_or(v.first_band).min(v.bands.len() - 1)
    }

    fn eq_band(&self, d: &Device, def: &DeviceDef, v: &EqView, i: usize) -> eq::Band {
        v.band(def, i, &|id| self.field_value(d, def, id), false)
    }

    fn eq_nodes(&self, d: &Device, def: &DeviceDef, v: &EqView, p: &Plot) -> Vec<Node> {
        let sel = self.eq_selected(d, v);
        let val = |id: &str| self.field_value(d, def, id);
        (0..v.bands.len())
            .filter_map(|i| {
                let on = v.band_on(i, &val);
                let b = self.eq_band(d, def, v, i);
                let x = p.x(b.freq as f64).clamp(6.0 * Z, p.w - 6.0 * Z);
                // Character EQ shows enabled bands (and the selected one, parked); Channel EQ all.
                if !on && v.create_on_click && i != sel {
                    return None;
                }
                let y = if !on {
                    p.h - p.park
                } else if b.shape.has_gain() {
                    p.y(b.gain_db).clamp(0.0, p.h)
                } else {
                    p.y(0.0)
                };
                let r = if v.prominent_nodes {
                    let prom = (0..12)
                        .map(|k| eq::band_db(&b, 20.0 * 1000f64.powf(k as f64 / 11.0), self.rate).abs())
                        .fold(b.gain_db.abs() as f64, f64::max);
                    (3.5 + 3.0 * (prom as f32 / 12.0).min(1.0)) * Z
                } else {
                    3.0 * Z
                };
                Some(Node { band: i, x, y, r, on })
            })
            .collect()
    }

    /// Field, chrome and output slider of an EQ card.
    pub(crate) fn eq_parts(&self, d: &Device, def: &'static DeviceDef, v: &'static EqView, accent: Color) -> (PanelCanvas, PanelEq) {
        self.eq_parts_on(d, def, v, accent, false)
    }

    /// The display of an EQ's expanded window (src/device_window.rs).
    pub(crate) fn eq_window_field(&self, d: &Device, def: &'static DeviceDef, v: &'static EqView, accent: Color) -> PanelCanvas {
        self.eq_parts_on(d, def, v, accent, true).0
    }

    fn eq_parts_on(&self, d: &Device, def: &'static DeviceDef, v: &'static EqView, accent: Color, win: bool) -> (PanelCanvas, PanelEq) {
        let width = def.layout.width();
        let (fx, fy, fh, p) = self.eq_plot_on(def, v, win);
        let val = |id: &str| self.field_value(d, def, id);
        let sel = self.eq_selected(d, v);
        let fs = self.rate;

        let mut paths = Vec::new();
        let mut texts = Vec::new();
        let rungs: Vec<f32> = {
            let mut r = Vec::new();
            let mut db = (v.top_db / v.rung_db).floor() * v.rung_db;
            while db > v.bottom_db {
                if db < v.top_db {
                    r.push(db);
                }
                db -= v.rung_db;
            }
            r
        };
        if win {
            win_grid(&p, &win_ticks(v.top_db, v.bottom_db), !self.engine.transport().playing, &mut paths, &mut texts);
            let mut legend = vec![(Legend::Band, "Band")];
            if v.bands.iter().any(|b| b.dyn_on.is_some()) {
                legend.push((Legend::Dynamic, "Dynamic"));
            }
            legend.push((Legend::Response, "Response"));
            win_legend(&p, &legend, accent, &mut paths, &mut texts);
        } else {
            grid(&p, &rungs, 0.0, &mut paths, &mut texts);
        }

        // Bank detents along the bottom.
        if let Some(bp) = v.bank {
            for f in eq::bank(val(bp)).detents {
                let x = p.x(*f as f64);
                paths.push(stroke(vline(x, p.h - 4.0 * Z, p.h), white(0.3), 1.0));
            }
        }

        // Curves: total (every enabled band, output and auto gain) and the selected band's own.
        let freqs: Vec<f64> = eq::log_freqs(220).collect();
        let sections = v.all_sections(def, &val, fs);
        let mut offset = val(v.output) as f64;
        if v.auto_gain.is_some_and(|a| val(a) >= 0.5) {
            offset += eq::auto_gain_db(&sections, fs);
        }
        let total: Vec<(f32, f32)> =
            freqs.iter().map(|&f| (p.x(f), p.y((eq::sections_db(&sections, f, fs) + offset) as f32).clamp(-2.0, p.h + 2.0))).collect();
        let sel_band = self.eq_band(d, def, v, sel);
        if v.band_on(sel, &val) {
            let own = eq::sections(&sel_band, fs);
            let pts: Vec<(f32, f32)> =
                freqs.iter().map(|&f| (p.x(f), p.y(eq::sections_db(&own, f, fs) as f32).clamp(-2.0, p.h + 2.0))).collect();
            paths.push(fill(area(&pts, p.y(0.0)), rgba(accent, 0.16)));
        }
        paths.push(fill(area(&total, p.y(0.0)), rgba(accent, 0.07)));
        // Reach of dynamic bands: the curve with every dynamic band at its full range.
        if (0..v.bands.len()).any(|i| v.band_on(i, &val) && v.bands[i].dyn_on.is_some_and(|p| val(p) >= 0.5)) {
            let reach: Vec<eq::Section> =
                (0..v.bands.len()).filter(|i| v.band_on(*i, &val)).flat_map(|i| eq::sections(&v.band(def, i, &val, true), fs)).collect();
            let pts: Vec<(f32, f32)> =
                freqs.iter().map(|&f| (p.x(f), p.y((eq::sections_db(&reach, f, fs) + offset) as f32).clamp(-2.0, p.h + 2.0))).collect();
            paths.push(stroke(path(&pts), rgba(accent, 0.3), 1.0 * Z));
        }
        // Live picture (dynamic bands moved by the engine).
        if let Some(ro) = self.devices.panel.canvas.readouts.get(&d.id).filter(|r| r.iter().take(v.bands.len()).any(|x| x.abs() > 0.05)) {
            let live: Vec<eq::Section> = (0..v.bands.len())
                .filter(|i| v.band_on(*i, &val))
                .flat_map(|i| {
                    let mut b = self.eq_band(d, def, v, i);
                    b.gain_db += ro.get(i).copied().unwrap_or(0.0);
                    eq::sections(&b, fs)
                })
                .collect();
            let pts: Vec<(f32, f32)> =
                freqs.iter().map(|&f| (p.x(f), p.y((eq::sections_db(&live, f, fs) + offset) as f32).clamp(-2.0, p.h + 2.0))).collect();
            paths.push(stroke(path(&pts), rgba(accent, 0.45), 1.25 * Z));
        }
        paths.push(stroke(path(&total), accent, 1.75 * Z));

        // Q wings of the selected band.
        let field = &self.devices.panel.canvas;
        let hot = |h: Hit| field.drag.as_ref().is_some_and(|g| g.device == d.id && g.hit == h) || field.hover == Some((d.id, h));
        if v.band_on(sel, &val) && sel_band.shape != Shape::DualLow {
            let hw = wing_octaves(sel_band.q);
            let h = hot(Hit::Wing(sel));
            for side in [-1.0f32, 1.0] {
                let x = p.x(sel_band.freq as f64 * 2f64.powf((side * hw) as f64));
                if x > 0.5 && x < p.w - 0.5 {
                    paths.push(stroke(vline(x, 0.0, p.h), white(if h { 0.35 } else { 0.12 }), if h { 2.0 } else { 1.0 }));
                }
            }
        }

        // Nodes.
        let nodes = self.eq_nodes(d, def, v, &p);
        let mut dots = Vec::new();
        for n in &nodes {
            let is_sel = n.band == sel;
            let color = if n.on { accent } else { AXIS };
            if is_sel {
                dots.push(PanelDot { x: n.x, y: n.y, r: n.r + 3.5 * Z, fill: white(0.12), ring: 1.5 * Z, ring_color: color });
            }
            dots.push(PanelDot { x: n.x, y: n.y, r: n.r, fill: color, ring: 0.0, ring_color: color });
        }

        // Value pill: dragged, else hovered, else selected band.
        let pill_band = field
            .drag
            .as_ref()
            .filter(|g| g.device == d.id)
            .and_then(|g| match g.hit {
                Hit::Node(i) | Hit::Wing(i) => Some((i, true)),
                _ => None,
            })
            .or_else(|| match field.hover {
                Some((id, Hit::Node(i) | Hit::Wing(i))) if id == d.id => Some((i, false)),
                _ => None,
            })
            .unwrap_or((sel, false));
        let mut pills = Vec::new();
        // A window marks the selected band by its ring; its pill shows while a node is under the pointer.
        let show_pill = !win || pill_band.1 || field.hover.is_some_and(|h| h.0 == d.id);
        if let Some(n) = nodes.iter().find(|n| n.band == pill_band.0).filter(|_| show_pill) {
            let b = self.eq_band(d, def, v, n.band);
            let fp = def.param(v.bands[n.band].freq);
            let mut value = fp.map(|fp| fp.format(b.freq)).unwrap_or_default();
            if pill_band.1 && b.shape.has_gain() && n.on {
                value = format!(
                    "{value}  {}",
                    def.param(v.bands[n.band].gain).map(|g| g.format(val(v.bands[n.band].gain))).unwrap_or_default()
                );
            }
            let above = n.y - (n.r + 13.0 * Z);
            let y = if above < 9.0 * Z { n.y + n.r + 13.0 * Z } else { above };
            let x = n.x.clamp(45.0 * Z, p.w - 45.0 * Z);
            pills.push(PanelGrip {
                x,
                y,
                anchor: 1,
                label: s(format!("Band {}", n.band + 1)),
                value: s(value),
                hot: pill_band.1,
                plain: true,
            });
        }

        let cursor =
            match field.drag.as_ref().filter(|g| g.device == d.id).map(|g| g.hit).or(field.hover.filter(|h| h.0 == d.id).map(|h| h.1)) {
                Some(Hit::Wing(_)) => 2,
                Some(Hit::Node(_)) => 1,
                _ => 0,
            };
        let freq_text = def.param(v.bands[sel].freq).map(|fp| fp.format(sel_band.freq)).unwrap_or_default();
        let field_out = PanelCanvas {
            visible: true,
            x: fx,
            y: fy,
            w: p.w,
            h: if win { fh + crate::device_window::PLOT_B * Z } else { fh },
            well_y: 0.0,
            well: !win,
            paths: model(paths),
            texts: model(texts),
            dots: model(dots),
            grips: model(pills),
            pointer: true,
            cursor,
            label: s(format!("Band {}", sel + 1)),
            value: s(if v.band_on(sel, &val) {
                format!("{freq_text}, {:+.1} dB", sel_band.gain_db)
            } else {
                format!("{freq_text}, out of the chain")
            }),
            ..Default::default()
        };

        // Chrome: tabs, latch, bank, type glyphs, cells.
        let n = v.bands.len();
        let tab_w = if n > 8 { 26.5 } else { 22.0 };
        let tabs: Vec<EqTab> = (0..n).map(|i| EqTab { label: s((i + 1).to_string()), on: v.band_on(i, &val) }).collect();
        let latch_x = EQ_TAB_X + n as f32 * tab_w + 4.0 + 8.0;
        let shape = sel_band.shape;
        let glyphs: Vec<EqGlyph> = v
            .picker_order
            .iter()
            .filter_map(|sh| v.kind_of(*sh).map(|c| EqGlyph { commands: s(glyph(*sh)), name: s(sh.name()), choice: c as i32 }))
            .collect();
        let glyph_selected =
            v.picker_order.iter().filter(|sh| v.kind_of(**sh).is_some()).position(|sh| *sh == shape).map_or(-1, |i| i as i32);
        let idx = |id: &str| def.index_of(id).map_or(-1, |i| i as i32);
        let band = &v.bands[sel];
        let cell = |label: &str, id: &str, enabled: bool, text: Option<String>| {
            let p = def.param(id);
            EqCell {
                x: 0.0,
                w: 0.0,
                label: s(label),
                value: s(text.unwrap_or_else(|| p.map(|p| p.format(val(id))).unwrap_or_default())),
                param: idx(id),
                enabled,
                rate: 1.0 / 160.0,
            }
        };
        let second = match (shape, v.slope_param(sel, shape)) {
            (Shape::HighPass | Shape::LowPass, Some(sp)) => cell("Slope", sp, true, def.param(sp).map(|p| slope_text(p, val(sp)))),
            (Shape::DualLow, _) => cell("Boost", band.gain, true, None),
            _ => cell("Gain", band.gain, shape.has_gain(), None),
        };
        let third = match (shape, band.low_atten) {
            (Shape::DualLow, Some(la)) => cell("Cut", la, true, None),
            _ => cell("Q", band.q, true, None),
        };
        let mut cells = vec![cell("Freq", band.freq, true, None), second, third];
        let mut right = width - PAD;
        for i in (0..3).rev() {
            right -= v.cells[i];
            cells[i].x = right * Z;
            cells[i].w = v.cells[i] * Z;
            right -= 6.0;
        }
        let bank_w = 109.0;
        let eq_out = PanelEq {
            visible: true,
            tabs: model(tabs),
            selected: sel as i32,
            tab_w: tab_w * Z,
            on_param: idx(band.on),
            on: v.band_on(sel, &val),
            latch_x: latch_x * Z,
            bank_param: v.bank.map_or(-1, &idx),
            bank_value: s(v.bank.and_then(|b| def.param(b).map(|p| p.format(val(b)))).unwrap_or_default()),
            bank_x: (width - PAD - bank_w) * Z,
            bank_w: bank_w * Z,
            glyphs: model(glyphs),
            glyph_selected,
            type_param: idx(band.kind),
            cells: model(cells),
        };
        (field_out, eq_out)
    }

    fn eq_hit(&self, d: &Device, def: &DeviceDef, v: &EqView, x: f32, y: f32) -> Hit {
        let (_, _, _, p) = self.eq_plot(def, v);
        let nodes = self.eq_nodes(d, def, v, &p);
        let reach = |n: &Node| if v.prominent_nodes { (n.r + 4.0 * Z).max(13.0 * Z) } else { 10.0 * Z };
        if let Some(n) = nodes
            .iter()
            .filter(|n| (n.x - x).hypot(n.y - y) <= reach(n))
            .min_by(|a, b| (a.x - x).hypot(a.y - y).total_cmp(&(b.x - x).hypot(b.y - y)))
        {
            return Hit::Node(n.band);
        }
        let sel = self.eq_selected(d, v);
        let val = |id: &str| self.field_value(d, def, id);
        let b = self.eq_band(d, def, v, sel);
        if v.band_on(sel, &val) && b.shape != Shape::DualLow {
            let hw = wing_octaves(b.q);
            for side in [-1.0f64, 1.0] {
                let wx = p.x(b.freq as f64 * 2f64.powf(side * hw as f64));
                if (wx - x).abs() <= 4.0 * Z {
                    return Hit::Wing(sel);
                }
            }
        }
        Hit::Empty
    }

    // ---------------------------------------------------------------- Auto Filter

    fn af_plot(&self, def: &DeviceDef) -> (f32, f32, f32, Plot) {
        self.af_plot_on(def, self.devices.panel.canvas.window)
    }

    fn af_plot_on(&self, def: &DeviceDef, win: bool) -> (f32, f32, f32, Plot) {
        if win && let Some((w, h)) = crate::device_window::plot_size(def) {
            return (0.0, 0.0, h, Plot { w, h, top_db: AF_TOP_DB, bottom_db: AF_BOTTOM_DB, axis: 0.0, park: 0.0 });
        }
        let width = def.layout.width();
        let (x, y) = (PAD * Z, AF_Y * Z);
        let w = (width - 2.0 * PAD) * Z;
        let h = (AF_BOTTOM - AF_Y) * Z;
        (x, y, h, Plot { w, h: h - AF_CAPTION * Z, top_db: AF_TOP_DB, bottom_db: AF_BOTTOM_DB, axis: AF_AXIS * Z, park: 0.0 })
    }

    fn af_setting(&self, d: &Device, def: &DeviceDef) -> FilterSetting {
        let val = |id: &str| self.field_value(d, def, id);
        FilterSetting {
            kind: val("filter_type").round().max(0.0) as u32,
            slope24: val("slope") >= 0.5,
            cutoff: val("frequency"),
            resonance: val("resonance"),
            mix: val("mix") / 100.0,
            dj: (val("mode") >= 0.5).then(|| val("dj_position")),
        }
    }

    /// Positions of the Auto Filter's interactive parts: corner (x, y),
    /// sweep edges (low x, high x, visible), env pill centre x.
    fn af_parts(&self, d: &Device, def: &DeviceDef, p: &Plot) -> ((f32, f32), (f32, f32, bool), f32) {
        let val = |id: &str| self.field_value(d, def, id);
        let st = self.af_setting(d, def);
        let c = st.cutoff as f64;
        let x = match st.dj {
            Some(pos) => (pos + 1.0) / 2.0 * p.w,
            None => p.x(c),
        }
        .clamp(6.0 * Z, p.w - 6.0 * Z);
        let at = match st.dj {
            Some(pos) => dj_map(pos).1 as f64,
            None => c,
        };
        let db = dstudio_core::builtin::filter_response(&st, at, self.rate) as f32;
        let y = p.y(db).clamp(5.0 * Z, p.h - p.axis - 5.0 * Z);
        let (env, lfo) = (val("env_amount"), val("lfo_amount"));
        let lo = (c * 2f64.powf(((env.min(0.0) - lfo) / 12.0) as f64)).clamp(20.0, 20_000.0);
        let hi = (c * 2f64.powf(((env.max(0.0) + lfo) / 12.0) as f64)).clamp(20.0, 20_000.0);
        let show = st.dj.is_none() && (env.max(0.0) + lfo) - (env.min(0.0) - lfo) >= 0.5;
        let env_x = p.x((c * 2f64.powf(env as f64 / 12.0)).clamp(20.0, 20_000.0));
        ((x, y), (p.x(lo), p.x(hi), show), env_x)
    }

    fn af_env_rect(&self, env_x: f32, corner: (f32, f32), p: &Plot) -> (f32, f32, f32, f32) {
        let w = 62.0 * Z;
        let mut x0 = env_x - w / 2.0;
        let y = 12.0 * Z;
        // Keep clear of the corner handle and inside the plot.
        if (corner.1 - y).abs() < 15.0 * Z && x0 < corner.0 + 15.0 * Z && x0 + w > corner.0 - 15.0 * Z {
            x0 = if corner.0 > p.w / 2.0 { corner.0 - 15.0 * Z - w } else { corner.0 + 15.0 * Z };
        }
        x0 = x0.clamp(4.0 * Z, p.w - 4.0 * Z - w);
        (x0, y - 8.0 * Z, w, 16.0 * Z)
    }

    pub(crate) fn auto_filter_field(&self, d: &Device, def: &'static DeviceDef, accent: Color) -> PanelCanvas {
        self.auto_filter_field_on(d, def, accent, false)
    }

    /// The display of the Auto Filter's expanded window (src/device_window.rs).
    pub(crate) fn auto_filter_window_field(&self, d: &Device, def: &'static DeviceDef, accent: Color) -> PanelCanvas {
        self.auto_filter_field_on(d, def, accent, true)
    }

    fn auto_filter_field_on(&self, d: &Device, def: &'static DeviceDef, accent: Color, win: bool) -> PanelCanvas {
        let (fx, fy, fh, p) = self.af_plot_on(def, win);
        let val = |id: &str| self.field_value(d, def, id);
        let st = self.af_setting(d, def);
        let fs = self.rate;
        let mut paths = Vec::new();
        let mut texts = Vec::new();
        if win {
            win_grid(&p, &win_ticks(AF_TOP_DB, AF_BOTTOM_DB), !self.engine.transport().playing, &mut paths, &mut texts);
        } else {
            let rungs: Vec<f32> =
                [12.0, 0.0, -12.0, -24.0].into_iter().filter(|db| p.y(*db) > 9.0 * Z && p.y(*db) < p.h - AF_AXIS * Z).collect();
            grid(&p, &rungs, AF_AXIS * Z, &mut paths, &mut texts);
        }

        let field = &self.devices.panel.canvas;
        let hot = |h: Hit| field.drag.as_ref().is_some_and(|g| g.device == d.id && g.hit == h) || field.hover == Some((d.id, h));
        let (corner, (lo_x, hi_x, sweep), env_x) = self.af_parts(d, def, &p);

        if sweep {
            paths.push(fill(format!("M{lo_x:.1} 0 L{hi_x:.1} 0 L{hi_x:.1} {:.1} L{lo_x:.1} {:.1} Z", p.h, p.h), rgba(accent, 0.07)));
            for (x, high) in [(lo_x, false), (hi_x, true)] {
                let h = hot(Hit::Edge(high));
                paths.push(stroke(vline(x, 0.0, p.h), rgba(accent, if h { 0.95 } else { 0.26 }), if h { 2.0 } else { 1.0 }));
            }
        }

        // Response: 160 log-spaced points plus the cutoff itself.
        let mut freqs: Vec<f64> = eq::log_freqs(160).collect();
        let cutoff = match st.dj {
            Some(pos) => dj_map(pos).1 as f64,
            None => st.cutoff as f64,
        };
        let at = freqs.partition_point(|f| *f < cutoff);
        freqs.insert(at, cutoff);
        let dbs: Vec<f32> = freqs.iter().map(|f| dstudio_core::builtin::filter_response(&st, *f, fs) as f32).collect();
        let prominence = (dbs.iter().fold(0.0f32, |m, v| m.max(v.abs())) / 15.0).min(1.0);
        let pts: Vec<(f32, f32)> = freqs.iter().zip(&dbs).map(|(f, db)| (p.x(*f), p.y(db.clamp(AF_BOTTOM_DB, AF_TOP_DB)))).collect();
        paths.push(fill(area(&pts, p.y(0.0)), rgba(accent, 0.08 + 0.2 * prominence)));
        paths.push(stroke(path(&pts), accent, 1.75 * Z));

        let mut dots = Vec::new();
        // Live cutoff (envelope / LFO moving it), from the engine.
        let moving = st.dj.is_none() && (val("env_amount").abs() > 0.0 || val("lfo_amount") > 0.0);
        if let Some(live) =
            field.readouts.get(&d.id).and_then(|r| r.first().copied()).filter(|c| moving && *c > 0.0 && (c / st.cutoff - 1.0).abs() > 0.005)
        {
            let x = p.x(live as f64);
            paths.push(stroke(vline(x, 0.0, p.h - p.axis), rgba(accent, 0.5), 1.0));
            let live_st = FilterSetting { cutoff: live, ..st };
            let y = p.y((dstudio_core::builtin::filter_response(&live_st, live as f64, fs) as f32).clamp(AF_BOTTOM_DB, AF_TOP_DB));
            dots.push(PanelDot { x, y, r: 2.75 * Z, fill: accent, ring: 0.0, ring_color: accent });
        }

        // Env pill (and its tie to the base cutoff), LFO pill beside a hot edge.
        let mut pills = Vec::new();
        let (ex, ey, ew, eh) = self.af_env_rect(env_x, corner, &p);
        let env_p = def.param("env_amount");
        if st.dj.is_none() {
            let base_x = p.x(st.cutoff as f64);
            if (env_x - base_x).abs() > 1.0 {
                paths.push(stroke(hline(ey + eh / 2.0, base_x.min(ex + ew / 2.0), base_x.max(ex + ew / 2.0)), rgba(accent, 0.3), 1.0));
            }
            pills.push(PanelGrip {
                x: ex,
                y: ey + eh / 2.0,
                anchor: 0,
                label: s("Env"),
                value: s(env_p.map(|e| e.format(val("env_amount"))).unwrap_or_default()),
                hot: hot(Hit::Env),
                plain: false,
            });
        }
        for (x, high) in [(lo_x, false), (hi_x, true)] {
            if sweep && hot(Hit::Edge(high)) {
                let lfo = def.param("lfo_amount").map(|l| l.format(val("lfo_amount"))).unwrap_or_default();
                let right = x + 4.0 * Z + 60.0 * Z < p.w;
                pills.push(PanelGrip {
                    x: if right { x + 4.0 * Z } else { x - 4.0 * Z },
                    y: ey + eh / 2.0 + 20.0 * Z,
                    anchor: if right { 0 } else { 2 },
                    label: s("LFO"),
                    value: s(lfo),
                    hot: true,
                    plain: true,
                });
            }
        }

        // Corner handle and its value pill.
        let corner_hot = hot(Hit::Corner);
        if corner_hot {
            dots.push(PanelDot {
                x: corner.0,
                y: corner.1,
                r: 8.0 * Z,
                fill: Color::from_argb_u8(0, 0, 0, 0),
                ring: 1.25 * Z,
                ring_color: rgba(accent, 0.5),
            });
        }
        dots.push(PanelDot { x: corner.0, y: corner.1, r: 5.0 * Z, fill: rgba(accent, 0.95), ring: 0.0, ring_color: accent });
        let (name, value) = self.af_corner_text(d, def);
        if corner_hot {
            let right = corner.0 + 14.0 * Z + 80.0 * Z < p.w;
            pills.push(PanelGrip {
                x: if right { corner.0 + 14.0 * Z } else { corner.0 - 14.0 * Z },
                y: corner.1,
                anchor: if right { 0 } else { 2 },
                label: s(name.clone()),
                value: s(value.clone()),
                hot: true,
                plain: false,
            });
        }

        let cursor =
            match field.drag.as_ref().filter(|g| g.device == d.id).map(|g| g.hit).or(field.hover.filter(|h| h.0 == d.id).map(|h| h.1)) {
                Some(Hit::Edge(_)) | Some(Hit::Env) => 2,
                Some(Hit::Corner) => 1,
                _ => 0,
            };
        if win {
            // The corner's value in the plot's top-left corner, the legend under the axis.
            let (caption, value) = match st.dj {
                Some(_) => ("Filter", value.clone()),
                None => ("Cutoff", value.clone()),
            };
            let cw = crate::text_measure::value_width(caption) * 1.1 * Z;
            texts.push(text(8.0 * Z, 12.0 * Z, 80.0 * Z, caption, accent, 11.0 * Z, 0));
            texts.push(text(8.0 * Z + cw + 5.0 * Z, 12.0 * Z, 80.0 * Z, value, accent, 11.0 * Z, 0));
            win_legend(&p, &[(Legend::Sweep(sweep), "Sweep"), (Legend::Response, "Response")], accent, &mut paths, &mut texts);
        }
        PanelCanvas {
            visible: true,
            x: fx,
            y: fy,
            w: p.w,
            h: if win { fh + crate::device_window::PLOT_B * Z } else { fh },
            well_y: if win { 0.0 } else { AF_CAPTION * Z },
            caption: s(if win { "" } else { "Response" }),
            well: !win,
            paths: model(paths),
            texts: model(texts),
            dots: model(dots),
            grips: model(pills),
            pointer: true,
            cursor,
            label: s("Filter corner"),
            value: s(format!("{name} {value}")),
            ..Default::default()
        }
    }

    /// Name and value of the Auto Filter's corner (frequency, or the DJ position's shape and corner).
    pub(crate) fn af_corner_text_of(&self, d: &Device, def: &DeviceDef) -> (String, String) {
        self.af_corner_text(d, def)
    }

    fn af_corner_text(&self, d: &Device, def: &DeviceDef) -> (String, String) {
        let st = self.af_setting(d, def);
        let hz = |f: f32| dstudio_core::builtin::format_value(dstudio_core::builtin::Format::Freq, f, "Hz", &[], 0.0);
        match st.dj {
            Some(pos) => {
                let (kind, fc, scale, _) = dj_map(pos);
                let v = if scale <= 0.0 { "Off".to_string() } else { format!("{} {}", if kind == 0 { "LP" } else { "HP" }, hz(fc)) };
                ("Filter".into(), v)
            }
            None => ("Freq".into(), hz(st.cutoff)),
        }
    }

    fn af_hit(&self, d: &Device, def: &DeviceDef, x: f32, y: f32) -> Hit {
        let (_, _, _, p) = self.af_plot(def);
        let (corner, (lo, hi, sweep), env_x) = self.af_parts(d, def, &p);
        if self.af_setting(d, def).dj.is_none() {
            let (ex, ey, ew, eh) = self.af_env_rect(env_x, corner, &p);
            let m = 2.0 * Z;
            if x >= ex - m && x <= ex + ew + m && y >= ey - m && y <= ey + eh + m {
                return Hit::Env;
            }
        }
        if sweep && y >= 0.0 && y <= p.h {
            if (x - hi).abs() <= 5.0 * Z {
                return Hit::Edge(true);
            }
            if (x - lo).abs() <= 5.0 * Z {
                return Hit::Edge(false);
            }
        }
        if (corner.0 - x).hypot(corner.1 - y) <= 13.0 * Z {
            return Hit::Corner;
        }
        Hit::None
    }

    // ---------------------------------------------------------------- gestures

    /// EQ and Auto Filter cards: their canvas pointer events go to panel_canvas.
    pub(crate) fn is_canvas_device(&self, device: DeviceId) -> bool {
        self.field_kind(device).is_some()
    }

    fn field_kind(&self, device: DeviceId) -> Option<(&Device, &'static DeviceDef, Option<&'static EqView>)> {
        let d = self.project().device(device)?;
        let def = dstudio_core::builtin::def_of(d)?;
        match def.layout.view {
            Some(View::Eq(v)) => Some((d, def, Some(v))),
            Some(View::Custom("auto-filter")) => Some((d, def, None)),
            _ => None,
        }
    }

    /// Pointer events of a field view (see ui/device_panel.slint CanvasView).
    pub fn panel_canvas(&mut self, device: DeviceId, phase: i32, x: f32, y: f32, alt: bool, shift: bool) {
        let Some((d, def, eqv)) = self.field_kind(device) else { return };
        // A window's canvas runs on under the plot (axis and legend): nothing to grab there.
        if self.devices.panel.canvas.window
            && phase == 0
            && let Some((_, h)) = crate::device_window::plot_size(def)
            && y > h + 4.0 * Z
        {
            return;
        }
        let hit = match eqv {
            Some(v) => self.eq_hit(d, def, v, x, y),
            None => self.af_hit(d, def, x, y),
        };
        match phase {
            0 => self.field_press(device, def, eqv, hit, x, y, alt, shift),
            1 => self.field_move(device, x, y, alt, shift),
            2 => self.field_release(),
            3 => {
                self.panel_cancel_canvas();
            }
            4 => {
                let h = (hit != Hit::None && hit != Hit::Empty).then_some((device, hit));
                if self.devices.panel.canvas.hover != h {
                    self.devices.panel.canvas.hover = h;
                    self.ui_dirty = true;
                }
            }
            5 => {
                if self.devices.panel.canvas.hover.is_some_and(|h| h.0 == device) {
                    self.devices.panel.canvas.hover = None;
                    self.ui_dirty = true;
                }
            }
            6 => self.field_double_click(device, def, eqv, hit),
            _ => {}
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn field_press(
        &mut self,
        device: DeviceId,
        def: &'static DeviceDef,
        eqv: Option<&'static EqView>,
        hit: Hit,
        x: f32,
        y: f32,
        alt: bool,
        shift: bool,
    ) {
        let Some(d) = self.project().device(device) else { return };
        let idx = |id: &str| def.index_of(id).unwrap_or(0);
        let mut params: Vec<usize> = Vec::new();
        let mut set: Vec<(usize, f32)> = Vec::new();
        let mut grab = (0.0, 0.0);
        let mut hit = hit;
        let mut select = None;
        match (eqv, hit) {
            (Some(v), Hit::Node(i)) | (Some(v), Hit::Wing(i)) => {
                select = Some(i);
                let b = &v.bands[i];
                params.extend([idx(b.freq), idx(b.gain), idx(b.q), idx(b.on)]);
                let (_, _, _, p) = self.eq_plot(def, v);
                if let Some(n) = self.eq_nodes(d, def, v, &p).into_iter().find(|n| n.band == i) {
                    grab = (n.x - x, n.y - y);
                    if !n.on && matches!(hit, Hit::Node(_)) {
                        // A parked band comes back at the clicked frequency.
                        set.push((idx(b.on), 1.0));
                        set.push((idx(b.freq), p.f(x) as f32));
                        grab = (0.0, if b.gain.is_empty() { 0.0 } else { p.y(self.field_value(d, def, b.gain)) - y });
                    }
                }
            }
            (Some(v), Hit::Empty) if v.create_on_click => {
                // Character EQ: the first free band starts here.
                let val = |id: &str| self.field_value(d, def, id);
                let Some(i) = (0..v.bands.len()).find(|i| !v.band_on(*i, &val)) else { return };
                let (_, _, _, p) = self.eq_plot(def, v);
                let b = &v.bands[i];
                let mut f = p.f(x) as f32;
                if let Some(bk) = v.bank {
                    f = eq::bank(val(bk)).snap(f);
                }
                params.extend([idx(b.freq), idx(b.gain), idx(b.q), idx(b.on)]);
                set.push((idx(b.on), 1.0));
                set.push((idx(b.freq), f));
                if v.shape(val(b.kind)).has_gain() {
                    set.push((idx(b.gain), p.db(y)));
                }
                select = Some(i);
                hit = Hit::Node(i);
            }
            (Some(_), _) => return,
            (None, Hit::Corner) => {
                let dj = self.field_value(d, def, "mode") >= 0.5;
                params.extend([idx(if dj { "dj_position" } else { "frequency" }), idx("resonance")]);
            }
            (None, Hit::Edge(_)) => params.push(idx("lfo_amount")),
            (None, Hit::Env) => params.push(idx("env_amount")),
            (None, _) => return,
        }
        let start: Vec<(usize, f32)> = params.iter().map(|&i| (i, def.value(d, &def.params[i]))).collect();
        let mut live = start.clone();
        if let Some(i) = select {
            self.devices.panel.canvas.selected.insert(device, i);
        }
        for (i, v) in &set {
            let v = def.params[*i].snap(*v);
            if let Some(e) = live.iter_mut().find(|e| e.0 == *i) {
                e.1 = v;
            }
            let _ = self.engine.builtin_set_param(device.0, *i as u32, v);
        }
        self.devices.panel.canvas.drag = Some(FieldDrag {
            device,
            hit,
            x0: x,
            y0: y,
            ax: x,
            ay: y,
            grab,
            anchor: live.clone(),
            start,
            live,
            active: false,
            fine: shift,
            alt,
            force: !set.is_empty(),
        });
        self.ui_dirty = true;
    }

    fn field_move(&mut self, device: DeviceId, x: f32, y: f32, alt: bool, shift: bool) {
        let Some((d, def, eqv)) = self.field_kind(device) else { return };
        let Some(mut g) = self.devices.panel.canvas.drag.clone().filter(|g| g.device == device) else { return };
        if !g.active && (x - g.x0).hypot(y - g.y0) < DRAG_SLOP {
            return;
        }
        g.active = true;
        if shift != g.fine {
            // Re-anchor so toggling fine mode never jumps: the grab offset
            // becomes the node's current position minus the pointer.
            let old = if g.fine { 0.2 } else { 1.0 };
            g.grab = (g.ax + g.grab.0 + (x - g.ax) * old - x, g.ay + g.grab.1 + (y - g.ay) * old - y);
            g.fine = shift;
            g.ax = x;
            g.ay = y;
            g.anchor = g.live.clone();
        }
        let scale = if g.fine { 0.2 } else { 1.0 };
        let anchor = |i: usize| g.anchor.iter().find(|e| e.0 == i).map_or(0.0, |e| e.1);
        let idx = |id: &str| def.index_of(id).unwrap_or(0);
        let mut out: Vec<(usize, f32)> = Vec::new();
        match (eqv, g.hit) {
            (Some(v), Hit::Node(i)) => {
                let (_, _, _, p) = self.eq_plot(def, v);
                let b = &v.bands[i];
                let shape = v.shape(self.field_value(d, def, b.kind));
                let free = alt || g.alt;
                let q_mode = free && v.shape(self.field_value(d, def, b.kind)) != Shape::DualLow;
                let fx = g.ax + g.grab.0 + (x - g.ax) * scale;
                let fy = g.ay + g.grab.1 + (y - g.ay) * scale;
                if !(q_mode && !v.create_on_click) {
                    let mut f = p.f(fx) as f32;
                    if let (Some(bk), false) = (v.bank, free) {
                        f = eq::bank(self.field_value(d, def, bk)).snap(f);
                    }
                    out.push((idx(b.freq), f));
                }
                if q_mode {
                    let q0 = anchor(idx(b.q));
                    out.push((idx(b.q), q0 * 2f32.powf((g.ay - y) * scale / (100.0 * Z))));
                } else if shape.has_gain() {
                    out.push((idx(b.gain), p.db(fy)));
                }
            }
            (Some(v), Hit::Wing(i)) => {
                // The wing shows the bank-voiced Q; the parameter holds it unvoiced.
                let (_, _, _, p) = self.eq_plot(def, v);
                let f = self.field_value(d, def, v.bands[i].freq) as f64;
                let q_scale = v.bank.map_or(1.0, |bk| eq::bank(self.field_value(d, def, bk)).q_scale);
                let q_at = |px: f32| 1.0 / (2.0 * (p.f(px) / f).log2().abs().max(0.01) as f32) / q_scale;
                let qi = idx(v.bands[i].q);
                out.push((qi, anchor(qi) * (q_at(x) / q_at(g.ax)).powf(scale)));
            }
            (None, Hit::Corner) => {
                let (_, _, _, p) = self.af_plot(def);
                let dj = self.field_value(d, def, "mode") >= 0.5;
                let dx = (x - g.ax) * scale;
                if dj {
                    let i = idx("dj_position");
                    out.push((i, anchor(i) + dx / p.w * 2.0));
                } else {
                    let i = idx("frequency");
                    out.push((i, p.f(p.x(anchor(i) as f64) + dx) as f32));
                }
                let r = idx("resonance");
                out.push((r, anchor(r) + (g.ay - y) * scale / (p.h - p.axis) * 1.25));
            }
            (None, Hit::Edge(high)) => {
                let (_, _, _, p) = self.af_plot(def);
                let delta = 12.0 * (p.f(x) / p.f(g.ax)).log2() as f32 * scale;
                let env = self.field_value(d, def, "env_amount");
                let lfo0 = anchor(idx("lfo_amount"));
                // Where the edge started (semitones from the cutoff), moved by the drag.
                let edge0 = if high { env.max(0.0) + lfo0 } else { env.min(0.0) - lfo0 };
                let edge = edge0 + delta;
                let lfo = if high { edge - env.max(0.0) } else { env.min(0.0) - edge };
                out.push((idx("lfo_amount"), lfo));
            }
            (None, Hit::Env) => {
                let (_, _, _, p) = self.af_plot(def);
                let delta = 12.0 * (p.f(x) / p.f(g.ax)).log2() as f32 * scale;
                let i = idx("env_amount");
                out.push((i, anchor(i) + delta));
            }
            _ => {}
        }
        let mut changed = false;
        for (i, v) in out {
            let v = def.params[i].snap(v);
            if let Some(e) = g.live.iter_mut().find(|e| e.0 == i)
                && e.1 != v
            {
                e.1 = v;
                let _ = self.engine.builtin_set_param(device.0, i as u32, v);
                changed = true;
            }
        }
        self.devices.panel.canvas.drag = Some(g);
        if changed {
            self.ui_dirty = true;
        }
    }

    fn field_release(&mut self) {
        let Some(g) = self.devices.panel.canvas.drag.take() else { return };
        self.ui_dirty = true;
        let Some(d) = self.project().device(g.device) else { return };
        let Some(def) = dstudio_core::builtin::def_of(d) else { return };
        let changes: Vec<Command> = g
            .live
            .iter()
            .filter(|(i, v)| g.start.iter().find(|s| s.0 == *i).is_some_and(|s| s.1 != *v))
            .map(|(i, v)| Command::SetDeviceParam { device: g.device, param: def.params[*i].id.to_string(), value: *v })
            .collect();
        if changes.is_empty() || !(g.active || g.force) {
            return;
        }
        self.doc.seal_coalescing();
        self.execute(Command::Batch { label: "Device Parameter Change".into(), commands: changes });
        self.doc.seal_coalescing();
    }

    /// Escape / pointer cancel during a field drag: back to the start values, no undo step.
    pub fn panel_cancel_canvas(&mut self) -> bool {
        let Some(g) = self.devices.panel.canvas.drag.take() else { return false };
        for (i, v) in &g.start {
            let _ = self.engine.builtin_set_param(g.device.0, *i as u32, *v);
        }
        self.ui_dirty = true;
        true
    }

    fn field_double_click(&mut self, device: DeviceId, def: &'static DeviceDef, eqv: Option<&'static EqView>, hit: Hit) {
        self.devices.panel.canvas.drag = None;
        let Some(d) = self.project().device(device) else { return };
        // Only parameters that actually move: an unchanged batch would add an empty undo step.
        let set = |ids: &[(&str, Option<f32>)]| -> Vec<Command> {
            ids.iter()
                .filter_map(|(id, v)| def.param(id).map(|p| (p, v.unwrap_or(p.default))))
                .filter(|(p, v)| def.value(d, p) != p.snap(*v))
                .map(|(p, v)| Command::SetDeviceParam { device, param: p.id.into(), value: v })
                .collect()
        };
        let cmds = match (eqv, hit) {
            (Some(v), Hit::Node(i)) if v.double_click == NodeDoubleClick::Park => set(&[(v.bands[i].on, Some(0.0))]),
            (None, Hit::Corner) => {
                let dj = def.value(d, def.param("mode").unwrap()) >= 0.5;
                set(&[(if dj { "dj_position" } else { "frequency" }, None), ("resonance", None)])
            }
            (None, Hit::Edge(_)) => set(&[("lfo_amount", None)]),
            (None, Hit::Env) => set(&[("env_amount", None)]),
            _ => return,
        };
        if cmds.is_empty() {
            return;
        }
        self.doc.seal_coalescing();
        self.execute(Command::Batch { label: "Device Parameter Change".into(), commands: cmds });
        self.doc.seal_coalescing();
    }

    /// Mouse wheel over an EQ node: Q x1.1 per tick up, x0.9 down (24 px of
    /// pixel scrolling per tick). Returns whether the field used it.
    pub fn panel_canvas_wheel(&mut self, device: DeviceId, x: f32, y: f32, dy: f32) -> bool {
        let Some((d, def, Some(v))) = self.field_kind(device) else { return false };
        let Hit::Node(i) = self.eq_hit(d, def, v, x, y) else { return false };
        let b = &v.bands[i];
        let (Some(qp), Some(onp)) = (def.param(b.q), def.param(b.on)) else { return true };
        let (q, off) = (def.value(d, qp), def.value(d, onp) < 0.5);
        let ui = &mut self.devices.panel.canvas;
        ui.wheel += dy;
        let ticks = (ui.wheel / 24.0).trunc();
        if ticks == 0.0 {
            return true;
        }
        ui.wheel -= ticks * 24.0;
        ui.selected.insert(device, i);
        let new_q = qp.snap(q * 1.1f32.powf(ticks));
        if !off && new_q == q {
            return true;
        }
        let set_q = Command::SetDeviceParam { device, param: qp.id.into(), value: new_q };
        if off {
            let on = Command::SetDeviceParam { device, param: onp.id.into(), value: 1.0 };
            self.execute(Command::Batch { label: "Device Parameter Change".into(), commands: vec![set_q, on] });
        } else {
            // Consecutive wheel ticks on one band coalesce into one undo step.
            self.execute(set_q);
        }
        true
    }

    /// Keys on a focused field: EQ arrows move the selected band (Left / Right
    /// frequency, through the bank's detents when it has them; Up / Down gain),
    /// Return / Space switch it, Delete removes it (Character EQ). Auto
    /// Filter arrows move the corner (frequency / position, resonance).
    pub fn panel_canvas_key(&mut self, device: DeviceId, key: &str, shift: bool, _alt: bool) -> bool {
        use slint::platform::Key;
        let is = |k: Key| key == SharedString::from(k).as_str();
        let Some((d, def, eqv)) = self.field_kind(device) else { return false };
        let idx = |id: &str| def.index_of(id).map_or(-1, |i| i as i32);
        let dir = if is(Key::RightArrow) || is(Key::UpArrow) { 1 } else { -1 };
        let horiz = is(Key::LeftArrow) || is(Key::RightArrow);
        let vert = is(Key::UpArrow) || is(Key::DownArrow);
        match eqv {
            Some(v) => {
                let sel = self.eq_selected(d, v);
                let b = &v.bands[sel];
                let val = |id: &str| self.field_value(d, def, id);
                if horiz {
                    if let Some(bk) = v.bank.filter(|bk| !eq::bank(val(bk)).detents.is_empty()) {
                        let det = eq::bank(val(bk)).detents;
                        let f = val(b.freq);
                        let next = if dir > 0 {
                            det.iter().copied().find(|x| *x > f * 1.001)
                        } else {
                            det.iter().rev().copied().find(|x| *x < f / 1.001)
                        };
                        if let Some(n) = next {
                            self.panel_commit_value(device, b.freq, n);
                        }
                    } else {
                        self.panel_step(device, idx(b.freq), dir, shift);
                    }
                    return true;
                }
                if vert {
                    if v.shape(val(b.kind)).has_gain() {
                        self.panel_step(device, idx(b.gain), dir, shift);
                    }
                    return true;
                }
                if is(Key::Return) || key == " " {
                    let on = v.band_on(sel, &val);
                    self.panel_commit_value(device, b.on, if on { 0.0 } else { 1.0 });
                    return true;
                }
                if v.create_on_click && (is(Key::Delete) || is(Key::Backspace)) {
                    if v.band_on(sel, &val) {
                        self.panel_commit_value(device, b.on, 0.0);
                    }
                    return true;
                }
                false
            }
            None => {
                let dj = self.field_value(d, def, "mode") >= 0.5;
                if horiz {
                    self.panel_step(device, idx(if dj { "dj_position" } else { "frequency" }), dir, shift);
                    return true;
                }
                if vert {
                    self.panel_step(device, idx("resonance"), dir, shift);
                    return true;
                }
                false
            }
        }
    }

    fn panel_commit_value(&mut self, device: DeviceId, param: &str, value: f32) {
        let unchanged = self.field_kind(device).and_then(|(d, def, _)| def.param(param).map(|p| p.snap(value) == def.value(d, p)));
        if unchanged != Some(false) {
            return;
        }
        self.doc.seal_coalescing();
        self.execute(Command::SetDeviceParam { device, param: param.into(), value });
        self.doc.seal_coalescing();
    }

    /// EQ chrome: 0 select band, 1 toggle the selected band, 2 type choice, 3 reset the type.
    pub fn panel_eq(&mut self, device: DeviceId, action: i32, value: i32) {
        let Some((d, def, Some(v))) = self.field_kind(device) else { return };
        let sel = self.eq_selected(d, v);
        let b = &v.bands[sel];
        match action {
            0 => {
                self.devices.panel.canvas.selected.insert(device, (value.max(0) as usize).min(v.bands.len() - 1));
                self.ui_dirty = true;
            }
            1 => {
                let on = v.band_on(sel, &|id| self.field_value(d, def, id));
                self.panel_commit_value(device, b.on, if on { 0.0 } else { 1.0 });
            }
            2 => self.panel_commit_value(device, b.kind, value.max(0) as f32),
            3 => {
                if let Some(p) = def.param(b.kind) {
                    self.panel_commit_value(device, b.kind, p.default);
                }
            }
            _ => {}
        }
    }

    /// The selected band's own response in a square card (an EQ window's control row):
    /// three octaves either side of its corner, the plot's dB range, a dashed line over
    /// an accent body, octave rules and the unit in the corner.
    pub(crate) fn eq_shape_card(&self, d: &Device, def: &DeviceDef, v: &EqView, accent: Color, size: f32) -> PanelCanvas {
        const OCTAVES: f32 = 3.0;
        let val = |id: &str| self.field_value(d, def, id);
        let sel = self.eq_selected(d, v);
        let on = v.band_on(sel, &val);
        let b = self.eq_band(d, def, v, sel);
        let inset = 2.0 * Z;
        let (w, h) = (size - 2.0 * inset, size - 2.0 * inset);
        let y_for = |db: f32| inset + (v.top_db - db.clamp(v.bottom_db, v.top_db)) / (v.top_db - v.bottom_db) * h;
        let y0 = y_for(0.0).round() + 0.5;
        let origin = white(0.55 * 0.32);
        let mut paths = vec![stroke(hline(y0, inset, inset + w), origin, 1.0)];
        for o in -2..=2 {
            let x = (inset + (o as f32 / OCTAVES + 1.0) * 0.5 * w).round() + 0.5;
            paths.push(stroke(vline(x, inset, inset + h), if o == 0 { origin } else { white(0.06) }, 1.0));
        }
        let dim = |c: Color| if on { c } else { rgba(c, c.alpha() as f32 / 255.0 * 0.35) };
        let sections = eq::sections(&b, self.rate);
        let pts: Vec<(f32, f32)> = (0..97)
            .map(|i| {
                let t = i as f32 / 96.0;
                let f = b.freq as f64 * 2f64.powf(((t * 2.0 - 1.0) * OCTAVES) as f64);
                (inset + t * w, y_for(eq::sections_db(&sections, f, self.rate) as f32))
            })
            .collect();
        if on {
            let prom = (pts.iter().map(|p| (p.1 - y0).abs()).fold(0.0f32, f32::max) / (h * 15.0 / (v.top_db - v.bottom_db))).min(1.0);
            paths.push(fill(area(&pts, y0), rgba(accent, 0.08 + 0.2 * prom)));
        }
        paths.push(stroke(dashed(&pts, 2.0 * Z, 3.0 * Z), dim(white(0.92)), 1.5 * Z));
        PanelCanvas {
            visible: true,
            w: size,
            h: size,
            paths: model(paths),
            texts: model(vec![text(inset, inset + h - 6.0 * Z, 30.0 * Z, "oct", white(0.3), 10.0 * Z, 0)]),
            ..Default::default()
        }
    }

    /// Bank pick (Character EQ): the bank and every enabled band's frequency
    /// moved onto the new bank's detents, in one undo step.
    pub(crate) fn panel_pick_bank(&mut self, device: DeviceId, param: usize, choice: f32) {
        let Some((d, def, Some(v))) = self.field_kind(device) else { return };
        let mut cmds = vec![Command::SetDeviceParam { device, param: def.params[param].id.into(), value: choice }];
        let same_bank = def.value(d, &def.params[param]) == choice;
        let bank = eq::bank(choice);
        let val = |id: &str| def.value(d, def.param(id).unwrap());
        for (i, b) in v.bands.iter().enumerate() {
            if v.band_on(i, &val) {
                let f = val(b.freq);
                let to = bank.snap(f);
                if to != f {
                    cmds.push(Command::SetDeviceParam { device, param: b.freq.into(), value: to });
                }
            }
        }
        if same_bank && cmds.len() == 1 {
            return;
        }
        self.doc.seal_coalescing();
        self.execute(Command::Batch { label: "Device Parameter Change".into(), commands: cmds });
        self.doc.seal_coalescing();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plot_maps_log_frequency_and_db() {
        let p = Plot { w: 400.0, h: 200.0, top_db: 24.0, bottom_db: -24.0, axis: 0.0, park: 19.0 * Z };
        assert!((p.x(20.0)).abs() < 1e-3 && (p.x(20_000.0) - 400.0).abs() < 1e-3);
        assert!((p.f(200.0) - 20.0 * 1000f64.sqrt()).abs() < 1e-6);
        assert_eq!(p.y(0.0), 100.0);
        assert_eq!(p.db(p.y(-12.0)), -12.0);
    }

    #[test]
    fn glyphs_and_labels() {
        assert!(glyph(Shape::Bell).starts_with("M0.0 6.0"));
        assert_eq!(db_label(-12.0), "\u{2212}12");
        assert_eq!(db_label(24.0), "+24");
        assert!((wing_octaves(0.6) - 0.8333).abs() < 1e-3);
    }
}
