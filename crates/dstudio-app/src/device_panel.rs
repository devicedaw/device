//! Built-in device panels: the generic card generated from a device
//! definition (dstudio_core::builtin::DeviceDef) and its gestures.
//!
//! Layout follows the fx card template: header, picker row, view row (a visualiser plus the slider column)
//! and the dial row of knob clusters. All lengths below are
//! 100%-zoom measurements; [Z] scales them to the 85% the UI is built at.
//!
//! Gestures: a knob, slider, pill or strip-line drag sends live values to the
//! engine (se_builtin_set_param, no model change) and commits ONE
//! Command::SetDeviceParam on release, so a drag is one undo step and the
//! session is republished once. Clicks, arrow keys, typed values, resets and
//! presets are single commands. Escape / pointer cancel restores the value
//! the drag started from.
//!
//! Views: the dynamics strip (View::Strip / View::DynamicsStrip: level history
//! from the engine's readouts, one or two draggable lines), the "gain-scope"
//! custom view, and View::Field, which plots a curve model from
//! device_fields.rs (traces, live LFO overlay from the engine's phase readout)
//! and edits two parameters by dragging. View::Custom(name) is the escape
//! hatch for device-specific bodies; the time/pitch cards' pointer fields live in
//! src/panel_fields.rs. The restoration devices' graphs (wave,
//! band, comb and gate views with handles, grip pills and a spectrum overlay)
//! live in src/panel_graph.rs.
//!
//! Picker row: Menu = segmented choice, Dropdown = menu pill, Toggle = latch
//! pill, Views = the card's view switch (UI state), Gap = flexible space.
//! Dial row: a Toggle is a latch cell (momentary while held when the
//! parameter says so); reversed parameters draw and drag their ring the other
//! way; inert parameters (Layout::inert_when) are dimmed and ignore gestures.

use crate::controller::{Controller, card_color};
use crate::device_canvas::{EQ_SLIDER_BOTTOM, EQ_SLIDER_TOP};
use crate::device_fields::{self, Fill, Live, Style};
use crate::devices::{DevMenu, MenuPick};
use crate::text_measure::{choice_width, name_width, value_width};
use crate::{DeviceCard, PanelCanvas, PanelEq, PanelField, PanelKnob, PanelPick, PanelSlider, PanelStrip, PanelTick};
use dstudio_core::builtin::{CLUSTER_GAP, Control, DeviceDef, Face, Field, KNOB_CELL, KNOB_GAP, ParamDef, Strip, Transfer, View};
use dstudio_core::{Command, Device, DeviceId, TrackId};
use slint::{ModelRc, SharedString, VecModel};
use std::borrow::Cow;
use std::collections::{HashMap, VecDeque};
use std::rc::Rc;

/// Card lengths are at 100% zoom; they are drawn at 85%.
pub const Z: f32 = 0.85;
/// Card padding, slider column width and the gap between view and sliders.
pub(crate) const PAD: f32 = 8.0;
const SLIDER_W: f32 = 62.0;
const VIEW_GAP: f32 = 10.0;
/// View row top, caption zone above the well, well bottom (card relative).
/// Cards without a picker row start the view row higher (VIEW_Y_NO_PICKER).
pub(crate) const VIEW_Y: f32 = 63.0;
pub(crate) const VIEW_Y_NO_PICKER: f32 = 37.0;
pub(crate) const CAPTION_H: f32 = 19.0;
pub(crate) const VIEW_BOTTOM: f32 = 214.0;
/// Dial row top (card relative).
pub(crate) const DIAL_Y: f32 = 229.0;
/// Slider rail top below the view row top.
const SLIDER_TOP: f32 = 23.0;
/// Dynamics strip history length (slots across the well).
pub(crate) const HISTORY: usize = 240;
/// Readout level of silence (below every strip floor).
const SILENT_DB: f32 = -120.0;
/// Custom view: scrolling output waveform with the applied-gain trace (transient shaper).
const GAIN_SCOPE: &str = "gain-scope";
/// Picker index of a card's view selector (not a parameter, e.g. the
/// multiband band tabs): \`panel_choice\` with it changes the view state.
pub(crate) const VIEW_SELECT: i32 = -3;
/// Detent snap distance of a drag, in the 0..1 range (4 / 160).
const DETENT_SNAP: f32 = 0.025;
/// Plot area inside a field's well: tick gutter on the left, top and bottom insets.
const FIELD_GUTTER: f32 = 24.0;
const FIELD_TOP: f32 = 4.0;
const FIELD_BOTTOM: f32 = 6.0;
/// Pointer travel before a press becomes a drag (px).
const DRAG_SLOP: f32 = 3.0;
/// Resonance given when DJ mode is entered with none.
const DJ_RESONANCE: f32 = 0.2;
const MINUS: char = '\u{2212}';

/// Live state of one parameter drag.
#[derive(Debug, Clone, Copy)]
struct ParamDrag {
    device: DeviceId,
    index: usize,
    start: f32,
    norm: f32,
    value: f32,
    travel: f32,
    /// Movement before the start threshold, applied once the drag starts.
    held: f32,
    active: bool,
    /// Strip Alt+drag: a second line moved by the same amount.
    linked: Option<Linked>,
}

#[derive(Debug, Clone, Copy)]
struct Linked {
    index: usize,
    start: f32,
    value: f32,
    /// Accumulated move of both lines (plain units, before clamping).
    delta: f32,
}

/// Smoothed level history of a dynamics strip (newest last).
#[derive(Debug, Clone, Default)]
struct MeterHistory {
    /// (input dB, output dB, gain reduction dB <= 0)
    slots: VecDeque<(f32, f32, f32)>,
    last: (f32, f32, f32),
}

/// Live state of a field drag: (parameter index, start, normalised, value) per axis.
#[derive(Debug, Clone, Copy)]
struct FieldDrag {
    device: DeviceId,
    axes: [Option<(usize, f32, f32, f32)>; 2],
    travel: f32,
    active: bool,
}

#[derive(Debug, Default)]
pub struct PanelState {
    drag: Option<ParamDrag>,
    field: Option<FieldDrag>,
    meters: HashMap<DeviceId, MeterHistory>,
    /// Gain scope histories (transient shaper) and the clock they run on.
    pub(crate) scopes: HashMap<DeviceId, crate::gain_scope::ScopeHistory>,
    scope_clock: Option<std::time::Instant>,
    /// Momentary latch held down: (device, param index).
    held: Option<(DeviceId, usize)>,
    pub fields: crate::panel_fields::FieldState,
    /// Card view state of devices with a custom view (selected band, meters).
    pub(crate) views: HashMap<DeviceId, crate::panel_multiband::ViewState>,
    pub(crate) sampler: crate::sampler_panel::SamplerUi,
    /// Field devices while the transport plays: (last LFO phase, elapsed cycles).
    lfo: HashMap<DeviceId, (f32, f64)>,
    /// Custom analyser views (pitch trace, tuner, spectrum).
    pub(crate) analyzers: crate::analyzer_views::AnalyzerState,
    pub(crate) editors: crate::device_editors::ViewState,
    /// Canvas views (EQ, Auto Filter): hover, drag and selection state.
    pub(crate) canvas: crate::device_canvas::CanvasUi,
    /// Field views (src/panel_graph.rs): drag, hover and spectrum state.
    pub(crate) graph: crate::panel_graph::GraphState,
}

pub(crate) fn s(v: impl Into<SharedString>) -> SharedString {
    v.into()
}

fn model<T: Clone + 'static>(v: Vec<T>) -> ModelRc<T> {
    ModelRc::from(Rc::new(VecModel::from(v)))
}

thread_local! {
    /// Models that only depend on the definition (choice names, strip rules), shared by
    /// every card so a refresh never swaps them under a pressed control.
    static STATIC_MODELS: std::cell::RefCell<HashMap<String, ModelRc<SharedString>>> = Default::default();
    static TICK_MODELS: std::cell::RefCell<HashMap<String, ModelRc<PanelTick>>> = Default::default();
}

fn choices_model(def: &DeviceDef, p: &ParamDef) -> ModelRc<SharedString> {
    labels_model(def, p, p.choices)
}

fn labels_model(def: &DeviceDef, p: &ParamDef, labels: &[&str]) -> ModelRc<SharedString> {
    STATIC_MODELS.with(|m| {
        m.borrow_mut()
            .entry(format!("{}/{}/{}", def.id, p.id, labels.join("|")))
            .or_insert_with(|| model(labels.iter().map(|c| s(*c)).collect()))
            .clone()
    })
}

/// Width of one segment of a choice track (pt): every segment is as wide as the
/// longest choice plus 12, at least 20.
pub(crate) fn segment_width(labels: &[&str]) -> f32 {
    let longest = labels.iter().map(|c| choice_width(c)).fold(0.0, f32::max);
    (longest + 12.0).ceil().max(20.0)
}

/// Width of a dropdown pill (pt): its longest choice measured in the 10 pt
/// semibold value face (tracking -0.4 per character), plus 30 for the left
/// padding and the chevron, rounded up.
fn dropdown_width(p: &ParamDef) -> f32 {
    let longest = p.choices.iter().map(|c| value_width(pill_text(c))).fold(0.0, f32::max);
    (30.0 + longest).ceil()
}

/// Off / on labels of a latch.
fn latch_labels(p: &ParamDef) -> (&'static str, &'static str) {
    p.latch.unwrap_or(("Off", "On"))
}

fn views_model(def: &DeviceDef) -> ModelRc<SharedString> {
    STATIC_MODELS.with(|m| {
        m.borrow_mut().entry(format!("{}/views", def.id)).or_insert_with(|| model(def.layout.views.iter().map(|c| s(*c)).collect())).clone()
    })
}

/// Choice text on a menu pill: list numbering ("4 · Heads 2+3") is left to the menu.
fn pill_text(text: &str) -> &str {
    match text.split_once(" · ") {
        Some((n, rest)) if !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()) => rest,
        _ => text,
    }
}

/// Keep a card's row models across refreshes: the Slint repeaters then update
/// their items in place instead of rebuilding them (a rebuilt knob would drop
/// the drag it is in the middle of).
pub(crate) fn reuse_card_models(old: &DeviceCard, new: &mut DeviceCard) {
    fn keep<T: Clone + PartialEq + 'static>(old: &ModelRc<T>, new: &mut ModelRc<T>) {
        let Some(vm) = slint::Model::as_any(old).downcast_ref::<VecModel<T>>() else { return };
        let rows: Vec<T> = (0..slint::Model::row_count(new)).filter_map(|i| slint::Model::row_data(new, i)).collect();
        crate::controller::sync_model(vm, rows);
        *new = old.clone();
    }
    if old.id != new.id || !old.builtin || !new.builtin {
        return;
    }
    keep(&old.knobs, &mut new.knobs);
    keep(&old.picks, &mut new.picks);
    keep(&old.sliders, &mut new.sliders);
    // EQ chrome: a rebuilt cell would drop its keyboard focus or an inline edit.
    keep(&old.eq.tabs, &mut new.eq.tabs);
    keep(&old.eq.glyphs, &mut new.eq.glyphs);
    keep(&old.eq.cells, &mut new.eq.cells);
    keep(&old.buttons, &mut new.buttons);
    keep(&old.bands.bands, &mut new.bands.bands);
    keep(&old.bands.xovers, &mut new.bands.xovers);
    // Field zones are touch areas: a rebuilt one would drop its drag.
    keep(&old.canvas.zones, &mut new.canvas.zones);
    keep(&old.canvas.paths, &mut new.canvas.paths);
    keep(&old.canvas.texts, &mut new.canvas.texts);
    keep(&old.canvas.grips, &mut new.canvas.grips);
}

/// x of each visible dial-row cell (card relative, unscaled). Knobs
/// are spaced evenly: the gap follows from the card width and the cell
/// count of the widest state, and the visible cells are centred (so a mode
/// with fewer knobs keeps the same spacing).
fn dial_xs(def: &DeviceDef, visible: &[&'static str]) -> Vec<(&'static str, f32)> {
    let (w, cells) = def.layout.widest_row();
    let gap = if cells > 1 { (w - 2.0 * PAD - cells as f32 * KNOB_CELL) / (cells - 1) as f32 } else { 0.0 };
    let n = visible.len() as f32;
    let x0 = (w - n * KNOB_CELL - (n - 1.0).max(0.0) * gap) / 2.0;
    visible.iter().enumerate().map(|(i, id)| (*id, x0 + i as f32 * (KNOB_CELL + gap))).collect()
}

/// Nearest detent within the snap distance of `norm` (plain value), if any.
fn snap_to_detent(p: &ParamDef, detents: &[f32], norm: f32, fine: bool) -> Option<f32> {
    let reach = if fine { DETENT_SNAP / 5.0 } else { DETENT_SNAP };
    detents
        .iter()
        .copied()
        .map(|v| (v, (p.to_normalized(v) - norm).abs()))
        .filter(|(_, d)| *d <= reach)
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(v, _)| v)
}

/// Detent tick marks just outside a knob ring (44 x 44 view box centred on the
/// 32 pt ring): from one ring stroke (1.76) to 2.5 strokes past the ring's centre line.
pub(crate) fn detent_ticks(p: &ParamDef, detents: &[f32]) -> String {
    const R: f32 = 16.0 - 0.88;
    let (inner, outer) = (R + 1.76, R + 1.76 * 2.5);
    let mut out = String::new();
    for v in detents {
        let a = (-135.0 + p.to_normalized(*v) * 270.0).to_radians();
        let (s, c) = a.sin_cos();
        out.push_str(&format!("M{:.2} {:.2} L{:.2} {:.2} ", 22.0 + inner * s, 22.0 - inner * c, 22.0 + outer * s, 22.0 - outer * c));
    }
    out
}

/// Right edge of the view row (card relative, unscaled): left of the slider column(s).
pub(crate) fn view_right(def: &DeviceDef) -> f32 {
    let width = def.layout.width();
    let n = def.layout.sliders.iter().filter(|c| c.param().and_then(|id| def.param(id)).is_some()).count();
    if n == 0 { width - PAD } else { width - PAD - SLIDER_W - (n - 1) as f32 * (SLIDER_W + VIEW_GAP) - VIEW_GAP }
}

/// Plot area of a field w pt wide (px at the 85% design zoom): width and height.
fn field_plot_size(f: &Field, w: f32) -> (f32, f32) {
    let well_y = if f.caption.is_empty() { 0.0 } else { CAPTION_H };
    ((w - FIELD_GUTTER) * Z, (VIEW_BOTTOM - VIEW_Y - well_y - FIELD_TOP - FIELD_BOTTOM) * Z)
}

/// x of each shown dial-row cell (card relative, unscaled). Hidden dials drop out and
/// empty clusters disappear; cells pack cluster by cluster, or spread evenly across
/// the card with Layout::spread_dials. Field cards (the motion / response
/// family) centre the dial row and space its knobs evenly across the row's width.
fn knob_xs(def: &DeviceDef, d: &Device) -> Vec<(&'static str, f32)> {
    let clusters: Vec<Vec<Vec<&'static str>>> = def
        .layout
        .clusters
        .iter()
        .map(|c| {
            c.iter()
                .map(|c| c.params().into_iter().filter(|p| def.is_shown(d, p)).collect::<Vec<_>>())
                .filter(|ps| !ps.is_empty())
                .collect::<Vec<_>>()
        })
        .filter(|c| !c.is_empty())
        .collect();
    let mut cells = Vec::new();
    if def.layout.spread_dials {
        let n: usize = clusters.iter().map(|c| c.len()).sum();
        let width = def.layout.width();
        let step = if n > 1 { (width - 2.0 * PAD - KNOB_CELL) / (n - 1) as f32 } else { 0.0 };
        let x0 = if n > 1 { PAD } else { (width - KNOB_CELL) / 2.0 };
        cells = clusters.into_iter().flatten().enumerate().map(|(i, ps)| (ps, x0 + i as f32 * step)).collect();
        return cells.into_iter().flat_map(|(ps, x)| ps.into_iter().map(move |p| (p, x))).collect();
    }
    let mut x = PAD;
    for (ci, cluster) in clusters.into_iter().enumerate() {
        if ci > 0 {
            x += CLUSTER_GAP - KNOB_GAP;
        }
        for c in cluster {
            cells.push((c, x));
            x += KNOB_CELL + KNOB_GAP;
        }
    }
    if matches!(def.layout.view, Some(View::Custom(_))) && cells.len() > 1 {
        // Custom bodies spread the cells across the whole card (fixed-width cards too).
        let step = (def.layout.width() - 2.0 * PAD - KNOB_CELL) / (cells.len() - 1) as f32;
        for (k, cell) in cells.iter_mut().enumerate() {
            cell.1 = PAD + k as f32 * step;
        }
    } else if matches!(def.layout.view, Some(View::Field(_))) && cells.len() > 1 {
        let row = x - KNOB_GAP - PAD;
        let (start, step) = ((def.layout.width() - row) / 2.0, (row - KNOB_CELL) / (cells.len() - 1) as f32);
        for (k, cell) in cells.iter_mut().enumerate() {
            cell.1 = start + k as f32 * step;
        }
    }
    // A swap cell holds several parameters; the shown one is drawn.
    cells.into_iter().flat_map(|(ps, x)| ps.into_iter().map(move |p| (p, x))).collect()
}

fn fmt_db(v: f32) -> String {
    let a = (v.abs() * 10.0).round() / 10.0;
    if a < 0.05 {
        "0.0 dB".into()
    } else if v < 0.0 {
        format!("{MINUS}{a:.1} dB")
    } else {
        format!("{a:.1} dB")
    }
}

/// Points as Slint path commands ("M x y L x y ...").
pub(crate) fn polyline(pts: &[(f32, f32)]) -> String {
    use std::fmt::Write;
    let mut out = String::with_capacity(pts.len() * 16);
    for (i, (x, y)) in pts.iter().enumerate() {
        let _ = write!(out, "{}{x:.1} {y:.1}", if i == 0 { "M" } else { " L" });
    }
    out
}

/// A smooth curve for meter traces and field curves: from the first point,
/// each inner point is the control of a quadratic to the midpoint after it, then a line
/// to the last point. `join` continues an open path with a line to the first point
/// instead of a move.
pub(crate) fn smooth(pts: &[(f32, f32)], join: bool) -> String {
    use std::fmt::Write;
    let mut out = String::with_capacity(pts.len() * 24);
    let Some(&(x0, y0)) = pts.first() else { return out };
    let _ = write!(out, "{}{x0:.1} {y0:.1}", if join { " L" } else { "M" });
    for w in pts.windows(2).skip(1) {
        let ((cx, cy), (nx, ny)) = (w[0], w[1]);
        let _ = write!(out, " Q{cx:.1} {cy:.1} {:.1} {:.1}", (cx + nx) / 2.0, (cy + ny) / 2.0);
    }
    if let Some(&(x, y)) = pts.last().filter(|_| pts.len() > 1) {
        let _ = write!(out, " L{x:.1} {y:.1}");
    }
    out
}

/// Dashed horizontal line from x0 to x1 at y (dash / gap lengths).
fn dashes(x0: f32, x1: f32, y: f32, dash: f32, gap: f32) -> String {
    let mut out = String::new();
    let mut x = x0;
    while x < x1 {
        out.push_str(&format!("M{x:.2} {y:.2} L{:.2} {y:.2} ", (x + dash).min(x1)));
        x += dash + gap;
    }
    out
}

/// A polyline cut into dashes (on / off lengths measured along the line).
fn dashed(pts: &[(f32, f32)], on: f32, off: f32) -> String {
    use std::fmt::Write;
    let mut out = String::new();
    let (mut drawing, mut left) = (true, on);
    for seg in pts.windows(2) {
        let ((x0, y0), (x1, y1)) = (seg[0], seg[1]);
        let len = (x1 - x0).hypot(y1 - y0);
        let mut t = 0.0;
        while t < len {
            let step = left.min(len - t);
            let (a, b) = (t / len.max(1e-6), (t + step) / len.max(1e-6));
            if drawing {
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
            left -= step;
            if left <= 1e-6 {
                drawing = !drawing;
                left = if drawing { on } else { off };
            }
        }
    }
    out
}

/// Gain reduction (dB, <= 0) of the quadratic soft-knee gain computer.
pub fn soft_knee_gr(x: f32, threshold: f32, ratio: f32, knee: f32) -> f32 {
    let slope = 1.0 / ratio.max(1.0) - 1.0;
    let over = x - threshold;
    if knee > 0.0 && over.abs() <= knee / 2.0 {
        slope * (over + knee / 2.0).powi(2) / (2.0 * knee)
    } else if over > 0.0 {
        slope * over
    } else {
        0.0
    }
}

/// Static transfer curve (output dB for input dB) of a knob face, the input
/// level the axis is laid out around and the span its knee highlight covers.
/// None for \`Face::Transfer\`, whose curve the device computes.
pub(crate) fn face_law(face: Face, num: impl Fn(&str) -> f32) -> Option<(Box<dyn Fn(f32) -> f32>, f32, (f32, f32))> {
    Some(match face {
        Face::SoftKneeCurve { threshold, ratio, knee } => {
            let (t, r, k) = (num(threshold), num(ratio), num(knee).max(0.0));
            (Box::new(move |x| x + soft_knee_gr(x, t, r, k)), t, (t - k / 2.0, t + k / 2.0))
        }
        Face::BrickWall { ceiling } => {
            let c = num(ceiling);
            (Box::new(move |x: f32| x.min(c)), c, (c, c))
        }
        Face::Expander { threshold, ratio } => {
            let (t, r) = (num(threshold), num(ratio).max(1.0));
            (Box::new(move |x| if x < t { t + (x - t) * r } else { x }), t, (t, t))
        }
        Face::BusVca { threshold, ratio, range } => {
            let r = num(ratio);
            // The bus VCA's working threshold sits lower for the gentler ratios (as the kernel).
            let (t, depth) = (num(threshold) - 3.0 * (10.0 - r) / 8.0, num(range).max(0.0));
            (Box::new(move |x| x + soft_knee_gr(x, t, r, 4.0).max(-depth)), t, (t - 2.0, t + 2.0))
        }
        Face::PushWall { threshold, ceiling } => {
            let (t, c) = (num(threshold), num(ceiling));
            (Box::new(move |x| (x + c - t).min(c)), t, (t, t))
        }
        Face::Transfer => return None,
    })
}

/// Transfer-face paths in a 32 x 32 box: (curve, knee highlight, dashed unity line).
/// The input axis spans 12 dB below `at` to 24 dB above it (at least to 0 dB).
fn law_face(law: &dyn Fn(f32) -> f32, at: f32, knee: (f32, f32)) -> (String, String, String) {
    law_face_in(law, at - 12.0, (at + 24.0).max(0.0), knee)
}

/// A brick wall's face: 24 dB below the ceiling up to just above it (at least 0 dB), so
/// the curve runs up the diagonal and flattens near the top.
fn wall_face(law: &dyn Fn(f32) -> f32, ceiling: f32, knee: (f32, f32)) -> (String, String, String) {
    law_face_in(law, ceiling - 24.0, (ceiling + 1.0).max(0.0), knee)
}

fn law_face_in(law: &dyn Fn(f32) -> f32, lo: f32, hi: f32, (knee_lo, knee_hi): (f32, f32)) -> (String, String, String) {
    let (pad, span) = (3.0, 26.0);
    let px = |db: f32| pad + (db - lo) / (hi - lo) * span;
    let py = |db: f32| 32.0 - pad - ((db - lo) / (hi - lo) * span).clamp(-pad, span + pad);
    let point = |x: f32| (px(x), py(law(x)));
    let n = 48;
    let curve: Vec<(f32, f32)> = (0..=n).map(|i| point(lo + (hi - lo) * i as f32 / n as f32)).collect();
    let margin = 3.0 / span * (hi - lo);
    let (b0, b1) = ((knee_lo - margin).max(lo), (knee_hi + margin).min(hi));
    let bend: Vec<(f32, f32)> = (0..=16).map(|i| point(b0 + (b1 - b0) * i as f32 / 16.0)).collect();
    let mut unity = String::new();
    let mut t = 0.0;
    while t < span {
        let t1 = (t + 2.0).min(span);
        unity.push_str(&format!("M{:.2} {:.2} L{:.2} {:.2} ", pad + t, 32.0 - pad - t, pad + t1, 32.0 - pad - t1));
        t += 4.0;
    }
    (polyline(&curve), polyline(&bend), unity)
}

/// Face paths of a device-computed transfer curve in a 32 x 32 box: (curve,
/// bend highlight, dashed unity line), laid out like [soft_knee_face].
pub(crate) fn transfer_face(t: &Transfer) -> (String, String, String) {
    let (lo, hi) = (t.lo, t.hi.max(t.lo + 1.0));
    let (pad, span) = (3.0, 26.0);
    let px = |db: f32| pad + (db - lo) / (hi - lo) * span;
    let py = |db: f32| 32.0 - pad - ((db - lo) / (hi - lo) * span).clamp(-pad, span + pad);
    let curve: Vec<(f32, f32)> = t.points.iter().map(|(x, y)| (px(*x), py(*y))).collect();
    let bend: Vec<(f32, f32)> = t.points.iter().filter(|(x, _)| *x >= t.bend.0 && *x <= t.bend.1).map(|(x, y)| (px(*x), py(*y))).collect();
    let mut unity = String::new();
    let mut s = 0.0;
    while s < span {
        let s1 = (s + 2.0).min(span);
        unity.push_str(&format!("M{:.2} {:.2} L{:.2} {:.2} ", pad + s, 32.0 - pad - s, pad + s1, 32.0 - pad - s1));
        s += 4.0;
    }
    (polyline(&curve), if bend.len() > 1 { polyline(&bend) } else { String::new() }, unity)
}

/// Strip paths for a level history in a w x wh well: [input fill, gain-reduction
/// band (input to input + GR), input line, output line].
fn strip_paths(slots: &VecDeque<(f32, f32, f32)>, strip: Strip, w: f32, wh: f32) -> [String; 4] {
    let span = strip.top_db - strip.floor_db;
    let y_of = |db: f32| ((strip.top_db - db) / span).clamp(0.0, 1.0) * wh;
    let n = slots.len();
    let x_of = |i: usize| w - (n - 1 - i) as f32 * w / (HISTORY - 1) as f32;
    // Curves through every 3 pt or so (the newest point always kept): denser cubic traces cost
    // render time without a visible difference.
    let step = ((3.0 * Z) / (w / (HISTORY - 1) as f32)).ceil().max(1.0) as usize;
    let kept = |i: usize| (n - 1 - i).is_multiple_of(step) || i == 0;
    let input: Vec<(f32, f32)> = slots.iter().enumerate().filter(|(i, _)| kept(*i)).map(|(i, s)| (x_of(i), y_of(s.0))).collect();
    let output: Vec<(f32, f32)> = slots.iter().enumerate().filter(|(i, _)| kept(*i)).map(|(i, s)| (x_of(i), y_of(s.1))).collect();
    let audible = |v: &[(f32, f32)]| v.iter().any(|p| p.1 < wh - 0.5);
    let (mut in_fill, mut in_line, mut gr_fill, mut out_line) = (String::new(), String::new(), String::new(), String::new());
    if n > 1 && audible(&input) {
        in_line = smooth(&input, false);
        in_fill = format!("{in_line} L{:.2} {wh:.2} L{:.2} {wh:.2} Z", input[input.len() - 1].0, input[0].0);
    }
    if n > 1 && slots.iter().any(|s| s.2 < -0.1) {
        let floor: Vec<(f32, f32)> =
            slots.iter().enumerate().rev().filter(|(i, _)| kept(*i)).map(|(i, s)| (x_of(i), y_of(s.0 + s.2))).collect();
        gr_fill = format!("{}{} Z", smooth(&input, false), smooth(&floor, true));
    }
    if n > 1 && audible(&output) {
        out_line = smooth(&output, false);
    } else if in_line.is_empty() {
        // Silence: the level trace rests on the floor.
        in_line = polyline(&[(30.0 * Z, wh - 0.5), (w, wh - 0.5)]);
    }
    [in_fill, gr_fill, in_line, out_line]
}

/// The command for one parameter edit: with the device's linked writes
/// (Character Mod circuit switching) it becomes one labelled batch.
fn param_command(def: &DeviceDef, d: &Device, p: &ParamDef, value: f32) -> Command {
    let set = Command::SetDeviceParam { device: d.id, param: p.id.to_string(), value };
    let (writes, label) = dstudio_core::builtin::linked_writes(def, d, p.id, value);
    if writes.is_empty() {
        return set;
    }
    let mut commands = vec![set];
    commands.extend(writes.into_iter().map(|(id, v)| Command::SetDeviceParam { device: d.id, param: id.to_string(), value: v }));
    Command::Batch { label: label.unwrap_or_else(|| "Device Parameter Change".into()), commands }
}

impl Controller {
    pub(crate) fn panel_def(&self, id: DeviceId) -> Option<(&Device, &'static DeviceDef)> {
        let d = self.project().device(id)?;
        Some((d, dstudio_core::builtin::def_of(d)?))
    }

    /// Value shown for a parameter: the live drag value (knob, field, canvas or held latch), else the model value.
    pub(crate) fn panel_value(&self, d: &Device, def: &DeviceDef, p: &ParamDef) -> f32 {
        if let Some(v) = def.index_of(p.id).and_then(|i| self.sampler_live(d.id, i).or_else(|| self.graph_drag_value(d.id, i))) {
            return v;
        }
        let is = |i: usize| def.params.get(i).is_some_and(|q| q.id == p.id);
        match self.devices.panel.drag {
            Some(g) if g.device == d.id && is(g.index) => return g.value,
            Some(ParamDrag { device, linked: Some(l), .. }) if device == d.id && is(l.index) => return l.value,
            _ => {}
        }
        if let Some(v) = def.index_of(p.id).and_then(|i| self.devices.panel.canvas.live(d.id, i)) {
            return v;
        }
        if let Some(g) = self.devices.panel.fields.drag.as_ref().filter(|g| g.device == d.id)
            && let Some((_, v)) = g.values.iter().find(|(i, _)| is(*i))
        {
            return *v;
        }
        if self.devices.panel.held.is_some_and(|(dev, i)| dev == d.id && is(i)) {
            return p.max;
        }
        if let Some(f) = self.devices.panel.field.filter(|f| f.device == d.id)
            && let Some((_, _, _, v)) = f.axes.iter().flatten().find(|a| is(a.0))
        {
            return *v;
        }
        def.value(d, p)
    }

    /// Track colour of the card (fallback teal).
    pub(crate) fn card_accent(&self, d: &Device) -> slint::Color {
        let track = self.project().find_device(d.id).map(|(ti, _)| &self.project().tracks[ti]);
        track.map_or(slint::Color::from_rgb_u8(0x43, 0x9b, 0x94), |t| card_color(t.color))
    }

    pub(crate) fn panel_value_of(&self, d: &Device, def: &DeviceDef, p: &ParamDef) -> f32 {
        self.panel_value(d, def, p)
    }

    /// The device as the panel shows it: the model values, with a running
    /// drag's value and the values it links applied.
    fn live_device<'a>(&self, d: &'a Device, def: &DeviceDef) -> Cow<'a, Device> {
        match self.devices.panel.drag {
            Some(g) if g.device == d.id && g.active => {
                let Some(p) = def.params.get(g.index) else { return Cow::Borrowed(d) };
                let mut live = d.clone();
                live.params.extend(def.linked_values(d, p.id, g.value));
                Cow::Owned(live)
            }
            _ => Cow::Borrowed(d),
        }
    }

    pub(crate) fn live_device_of<'a>(&self, d: &'a Device, def: &DeviceDef) -> Cow<'a, Device> {
        self.live_device(d, def)
    }

    /// Transfer curve of a `Face::Transfer` knob.
    pub(crate) fn device_transfer(&self, live: &Device, def: &DeviceDef) -> Option<Transfer> {
        dstudio_core::builtin::behavior(def).and_then(|b| (b.transfer)(def, live)).or_else(|| self.view_transfer(live, def))
    }

    /// The card of a built-in device (generic panel).
    pub(crate) fn builtin_card(&self, d: &Device, def: &'static DeviceDef) -> DeviceCard {
        let width = def.layout.width();
        // Key and Scale that follow the project key are shown dimmed with the project's values.
        let followed = dstudio_core::builtin::followed_key(def, d, self.project().transport.key);
        let accent = self.card_accent(d);
        let idx = |id: &str| def.index_of(id).map_or(-1, |i| i as i32);
        let live = self.live_device(d, def);
        let live = live.as_ref();
        let number = |id: &str| def.param(id).map_or(0.0, |p| p.number(self.panel_value(d, def, p)));

        // Picker row; a Gap, the sidechain picker and everything after them align right.
        let mut picks = Vec::new();
        if let Some(pick) = self.view_picker(d, def) {
            picks.push(pick);
        }
        for (ci, c) in def.layout.picker.iter().enumerate() {
            let p = c.param().and_then(|id| def.param(id));
            match (c, p) {
                (_, Some(p)) if !def.is_shown(d, p.id) => {}
                (Control::Dropdown(_), Some(p)) => {
                    let follow = followed.filter(|_| p.id == "key" || p.id == "scale");
                    let v = follow.map_or_else(|| self.panel_value(d, def, p), |(k, sc)| if p.id == "key" { k } else { sc });
                    let text = def.text(live, p, v);
                    picks.push(PanelPick {
                        kind: 2,
                        param: idx(p.id),
                        label: s(def.label_of(d, p)),
                        value: s(pill_text(&text)),
                        selected: v.round() as i32,
                        enabled: true,
                        width: dropdown_width(p) * Z,
                        dim: follow.is_some(),
                        ..Default::default()
                    });
                }
                (Control::Latch(_), Some(p)) => picks.push(PanelPick {
                    kind: 5,
                    param: idx(p.id),
                    label: s(def.label_of(d, p)),
                    selected: self.panel_value(d, def, p).round() as i32,
                    enabled: true,
                    ..Default::default()
                }),
                // Latches align right unless the row places its own gap;
                // the label carries the state.
                (Control::Toggle(_), Some(p)) => {
                    if !def.layout.picker.contains(&Control::Gap) && !picks.iter().any(|q: &PanelPick| q.kind == 4) {
                        picks.push(PanelPick { kind: 4, ..Default::default() });
                    }
                    let on = self.panel_value(d, def, p) >= 0.5;
                    let (off_text, on_text) = match p.latch {
                        Some((off, on_text)) => (off.to_string(), on_text.to_string()),
                        None => (format!("{} Off", p.label), format!("{} On", p.label)),
                    };
                    // As wide as its wider state label + 16.76, rounded up.
                    // Reverb's Dry latch has a fixed 54 pt basis.
                    let width =
                        if def.id == "reverb" { 54.0 } else { (value_width(&off_text).max(value_width(&on_text)) + 16.76).ceil() } * Z;
                    let label = if on { on_text } else { off_text };
                    picks.push(PanelPick {
                        kind: 5,
                        param: idx(p.id),
                        label: s(label),
                        selected: on as i32,
                        enabled: true,
                        width,
                        ..Default::default()
                    });
                }
                (Control::Views, _) => picks.push(PanelPick {
                    kind: 7,
                    param: -1,
                    label: s("View"),
                    choices: views_model(def),
                    selected: self.card_view(d.id) as i32,
                    enabled: true,
                    width: segment_width(def.layout.views) * Z,
                    ..Default::default()
                }),
                (Control::Menu(_), Some(p)) => picks.push(PanelPick {
                    kind: 1,
                    param: idx(p.id),
                    label: s(def.label_of(d, p)),
                    choices: choices_model(def, p),
                    selected: self.panel_value(d, def, p).round() as i32,
                    enabled: true,
                    // Glue's sidechain filter is a switch bank: every position as wide as "Auto".
                    width: segment_width(if def.id == "glue-compressor" { &["Auto"] } else { p.choices }) * Z,
                    ..Default::default()
                }),
                (Control::Sidechain, _) => {
                    picks.push(PanelPick { kind: 4, ..Default::default() });
                    let source = match d.sidechain {
                        None => "None".to_string(),
                        Some(t) => self.project().track(t).map_or_else(|| "Removed".to_string(), |t| t.name.clone()),
                    };
                    // The pill is sized per card, not by the track names: at most 140, less
                    // where the row leaves less room. Other cards: the longest option + 30.
                    let w = match def.id {
                        "character-compressor" | "glue-compressor" => 140.0,
                        "echo" => 130.0,
                        "compressor" => 79.0,
                        _ => {
                            let own = self.project().find_device(d.id).map(|(ti, _)| self.project().tracks[ti].id);
                            let longest = self
                                .project()
                                .tracks
                                .iter()
                                .filter(|t| Some(t.id) != own)
                                .map(|t| value_width(&t.name))
                                .fold(value_width("None").max(value_width(&source)), f32::max);
                            (30.0 + longest).ceil()
                        }
                    };
                    picks.push(PanelPick {
                        kind: 2,
                        param: -1,
                        label: s("Sidechain"),
                        value: s(source),
                        enabled: true,
                        width: w * Z,
                        ..Default::default()
                    });
                }
                (Control::Segments(_, labels), Some(p)) => picks.push(PanelPick {
                    kind: 1,
                    param: idx(p.id),
                    label: s(def.label_of(d, p)),
                    choices: labels_model(def, p, labels),
                    selected: self.panel_value(d, def, p).round() as i32,
                    enabled: true,
                    width: segment_width(labels) * Z,
                    ..Default::default()
                }),
                (Control::Gap, _) => picks.push(PanelPick { kind: 4, ..Default::default() }),
                (Control::ModeDropdown { mode, .. }, Some(p)) => {
                    // The mode's second choice is one more entry of the type menu (Auto Filter: DJ).
                    let m = def.param(mode);
                    let alt = m.filter(|m| self.panel_value(d, def, m) >= 0.5);
                    let value =
                        alt.map_or_else(|| p.format(self.panel_value(d, def, p)), |m| m.choices.get(1).copied().unwrap_or("").to_string());
                    picks.push(PanelPick {
                        kind: 2,
                        param: idx(p.id),
                        label: s(p.label),
                        value: s(value),
                        enabled: true,
                        width: 57.0 * Z,
                        ..Default::default()
                    });
                }
                (Control::Pill(_), Some(p)) => picks.push(PanelPick {
                    kind: 3,
                    param: idx(p.id),
                    label: s(p.label),
                    value: s(def.text(live, p, self.panel_value(d, def, p))),
                    // A pill after the sidechain picker scales the key: inert without a source.
                    enabled: d.sidechain.is_some() || !def.layout.picker[..ci].contains(&Control::Sidechain),
                    // Wide enough for either end of the range, plus 18.
                    width: (18.0 + choice_width(&p.format(p.min)).max(choice_width(&p.format(p.max)))).ceil() * Z,
                    ..Default::default()
                }),
                _ => {}
            }
        }

        // Without a picker row the view row moves up into its place.
        let view_y = if def.layout.picker.is_empty() { VIEW_Y_NO_PICKER } else { VIEW_Y };
        // Without a gap the row's items stay on the left.
        if !picks.iter().any(|p| p.kind == 4) {
            picks.push(PanelPick { kind: 4, ..Default::default() });
        }
        // Captions take the table-measured width (rounded up), not the rendered text's.
        for p in &mut picks {
            p.label_w = name_width(&p.label).ceil() * Z;
        }

        // Slider column (an EQ's Output slider comes from its view).
        let eqv = match def.layout.view {
            Some(View::Eq(v)) => Some(v),
            _ => None,
        };
        let slider_w = eqv.map_or(SLIDER_W, |v| v.slider_w);
        let slider_x = width - PAD - slider_w;
        let sliders: Vec<PanelSlider> = def
            .layout
            .sliders
            .iter()
            .filter_map(|c| c.param())
            .chain(eqv.map(|v| v.output))
            .filter_map(|id| def.param(id))
            .enumerate()
            .map(|(i, p)| {
                let v = self.panel_value(d, def, p);
                let n = p.to_normalized(v);
                PanelSlider {
                    param: idx(p.id),
                    x: (slider_x - i as f32 * (slider_w + VIEW_GAP)) * Z,
                    label: s(def.label_of(d, p)),
                    value: s(def.text(live, p, v)),
                    norm: if p.reversed { 1.0 - n } else { n },
                    zero: p.origin(),
                }
            })
            .collect();
        let view_right = view_right(def);

        // View row.
        let (mut strip, mut field, mut canvas, mut eq) =
            (PanelStrip::default(), PanelField::default(), PanelCanvas::default(), PanelEq::default());
        match def.layout.view {
            Some(View::Field(f)) => field = self.field_view(d, def, &f, view_right - PAD),
            Some(View::Eq(v)) => (canvas, eq) = self.eq_parts(d, def, v, accent),
            Some(View::Custom("auto-filter")) => canvas = self.auto_filter_field(d, def, accent),
            Some(v) => {
                if let Some(st) = v.strip() {
                    strip = self.dynamics_strip(d, def, st, view_right - PAD, view_y);
                }
            }
            None => {}
        }
        let (slider_top, slider_bottom) = if eqv.is_some() { (EQ_SLIDER_TOP, EQ_SLIDER_BOTTOM) } else { (view_y + SLIDER_TOP, 174.0) };
        let bands = self.view_field(d, def, live);
        let body = match def.layout.view {
            // Pointer bodies with several grab targets (src/panel_fields.rs).
            Some(View::Custom(_)) if crate::panel_fields::field_name(def).is_some() => {
                crate::device_views::CustomBody { canvas: self.panel_field(d, def, view_right), ..Default::default() }
            }
            Some(View::Custom(name)) if name != "auto-filter" => {
                let view_y = if def.layout.picker.is_empty() { VIEW_Y_NO_PICKER } else { VIEW_Y };
                self.custom_body(d, def, name, (PAD, view_y, view_right - PAD, VIEW_BOTTOM - view_y))
            }
            _ => crate::device_views::CustomBody::default(),
        };

        // Dial row (or the custom body's knob placement), bound to the view's selection.
        let visible: Vec<&'static str> =
            def.layout.clusters.iter().flat_map(|c| c.iter()).flat_map(|c| c.params()).filter(|id| def.is_shown(d, id)).collect();
        // Field cards and the pointer-field cards (src/panel_fields.rs) lay out their own dial row
        // (knob_xs: clusters, Layout::spread_dials, inert / hidden cells); the rest spread evenly.
        let places: Vec<(&'static str, f32, f32)> = if !body.knobs.is_empty() {
            body.knobs.clone()
        } else if matches!(def.layout.view, Some(View::Field(_))) || crate::panel_fields::field_name(def).is_some() {
            knob_xs(def, d).into_iter().map(|(id, x)| (id, x, DIAL_Y)).collect()
        } else {
            dial_xs(def, &visible).into_iter().map(|(id, x)| (id, x, DIAL_Y)).collect()
        };
        let behavior = dstudio_core::builtin::behavior(def);
        let knobs = places
            .into_iter()
            .filter(|(id, _, _)| def.is_shown(d, id))
            .filter_map(|(slot, x, y)| {
                let id = self.view_bind(d, def, slot);
                let p = def.param(id)?;
                let v = self.panel_value(d, def, p);
                let n = p.to_normalized(v);
                let detents = def.detents(live, p);
                let latch = def.layout.clusters.iter().flat_map(|c| c.iter()).any(|c| *c == Control::Toggle(id));
                let (off_label, on_label) = latch_labels(p);
                let mut k = PanelKnob {
                    param: idx(id),
                    x: x * Z,
                    y: y * Z,
                    label: s(def.label_of(d, p)),
                    value: s(if latch { if v >= 0.5 { on_label } else { off_label }.to_string() } else { def.text(live, p, v) }),
                    norm: if p.reversed { 1.0 - n } else { n },
                    cell: body.cell * Z,
                    origin: p.origin(),
                    ticks: s(detent_ticks(p, detents)),
                    dim: body.dim.contains(&id),
                    latch,
                    on: latch && v >= 0.5,
                    momentary: p.momentary,
                    inert: def.is_inert(d, id),
                    ..Default::default()
                };
                if let Some(l) = behavior.and_then(|b| (b.latch)(def, live, p)) {
                    k.face = 2;
                    k.value = s(l.labels[l.on as usize]);
                    k.norm = if l.on { 1.0 } else { 0.0 };
                    k.ticks = s("");
                    return Some(k);
                }
                if def.layout.clusters.iter().any(|c| c.contains(&Control::Toggle(id))) {
                    k.face = 2; // latch capsule
                }
                match def.layout.faces.iter().find(|(f, _)| *f == slot).map(|(_, f)| *f) {
                    Some(Face::Transfer) => {
                        if let Some(t) = self.device_transfer(live, def) {
                            let (curve, bend, unity) = transfer_face(&t);
                            k.face = 1;
                            k.face_curve = s(curve);
                            k.face_bend = s(bend);
                            k.face_unity = s(unity);
                        }
                    }
                    Some(face) => {
                        let wall = matches!(face, Face::BrickWall { .. });
                        let Some((law, at, knee)) = face_law(face, number) else { return Some(k) };
                        let (curve, bend, unity) = if wall { wall_face(&law, at, knee) } else { law_face(&law, at, knee) };
                        k.face = 1;
                        k.face_curve = s(curve);
                        k.face_bend = s(bend);
                        k.face_unity = s(unity);
                    }
                    None => {}
                }
                Some(k)
            })
            .collect();

        let preset = match &d.preset {
            None => String::new(),
            Some(name) => {
                let dirty = def.factory.iter().find(|f| f.name == name).is_some_and(|f| {
                    let want = def.factory_values(f);
                    // The sidechain switch follows the panel's source pick, not the preset.
                    let enable = def.sidechain_enable().map(|p| p.id);
                    def.params
                        .iter()
                        .filter(|p| Some(p.id) != enable)
                        .any(|p| (def.value(d, p) - want.get(p.id).copied().unwrap_or(p.default)).abs() > 1e-4 * (p.max - p.min))
                });
                if dirty { format!("{name}*") } else { name.clone() }
            }
        };

        let mut card = DeviceCard {
            id: d.id.0 as i32,
            name: s(def.name),
            vendor: s(dstudio_core::builtin::VENDOR),
            format: s("Built-in"),
            kind: s("Audio effect"),
            enabled: self.device_on_now(d),
            builtin: true,
            width: width * Z,
            accent,
            preset: s(preset),
            knobs: model(knobs),
            picks: model(picks),
            sliders: model(sliders),
            strip,
            field,
            slider_top: slider_top * Z,
            slider_bottom: slider_bottom * Z,
            slider_w: slider_w * Z,
            canvas: if canvas.visible { canvas } else { body.canvas },
            eq,
            bands,
            buttons: model(body.buttons),
            ..Default::default()
        };
        if let Some(View::Custom(name)) = def.layout.view {
            self.custom_card(name, d, def, &mut card);
        }
        if crate::sampler_panel::is_sampler(def) {
            return self.sampler_card(d, def, card);
        }
        if matches!(def.layout.view, Some(View::Custom(_))) {
            self.mod_body(d, def, &mut card);
        }
        card
    }

    fn dynamics_strip(&self, d: &Device, def: &DeviceDef, st: Strip, w: f32, view_y: f32) -> PanelStrip {
        let (w, h) = (w * Z, (VIEW_BOTTOM - view_y) * Z);
        let well_y = CAPTION_H * Z;
        let wh = h - well_y;
        let (top, floor) = (st.top_db, st.floor_db);
        let span = top - floor;
        let y_of = |db: f32| ((top - db) / span).clamp(0.0, 1.0) * wh;
        // A tick every 12 dB from the first multiple of 12 at or below the top,
        // plus the floor when it is off that grid.
        let first = (top / 12.0).floor() * 12.0;
        let ticks: Vec<PanelTick> = (0..=((first - floor) / 12.0) as i32)
            .map(|i| first - 12.0 * i as f32)
            .chain(((first - floor) % 12.0 != 0.0).then_some(floor))
            .map(|db| {
                let text = if db == 0.0 {
                    "0".to_string()
                } else if db < 0.0 {
                    format!("{MINUS}{}", db.abs() as i32)
                } else {
                    format!("+{}", db as i32)
                };
                PanelTick { y: y_of(db), text: s(text), rule: db < top && db > floor }
            })
            .collect();

        let empty = MeterHistory::default();
        let hist = self.devices.panel.meters.get(&d.id).unwrap_or(&empty);
        let [in_fill, gr_fill, in_line, out_line] = strip_paths(&hist.slots, st, w, wh);
        let gr_now = hist.slots.iter().rev().take(30).map(|s| s.2).fold(0.0f32, f32::min);

        let p = def.param(st.line);
        let t = p.map(|p| self.panel_value(d, def, p)).unwrap_or(st.floor_db);
        let ty = y_of(t).min(wh - 1.0);
        let q = st.second.and_then(|id| def.param(id));
        let second = q.map(|q| self.panel_value(d, def, q)).unwrap_or(floor);
        let sy = y_of(second).min(wh - 1.0);
        PanelStrip {
            visible: true,
            x: PAD * Z,
            y: view_y * Z,
            w,
            h,
            well_y,
            caption: s("Reduction"),
            readout: s(fmt_db(gr_now)),
            ticks: TICK_MODELS
                .with(|m| m.borrow_mut().entry(format!("{}/{wh}/{top}/{floor}", def.id)).or_insert_with(|| model(ticks)).clone()),
            in_fill: s(in_fill),
            gr_fill: s(gr_fill),
            in_line: s(in_line),
            out_line: s(out_line),
            thresh_param: def.index_of(st.line).map_or(-1, |i| i as i32),
            thresh_y: ty,
            thresh_dash: s(dashes(30.0 * Z, w, ty, 6.0 * Z, 5.0 * Z)),
            thresh_label: s(p.map_or("", |p| p.label)),
            thresh_value: s(p.map(|p| p.format(t)).unwrap_or_default()),
            // The line follows the pointer: (dB per px) / (dB per unit of the 0..1 range).
            thresh_rate: p.map_or(0.0, |p| span / wh / (p.max - p.min)),
            second_param: q.and_then(|q| def.index_of(q.id)).map_or(-1, |i| i as i32),
            second_y: sy,
            second_line: s(if q.is_some() { polyline(&[(30.0 * Z, sy), (w, sy)]) } else { String::new() }),
            second_label: s(q.map_or("", |q| q.label)),
            second_value: s(q.map(|q| q.format(second)).unwrap_or_default()),
            second_rate: q.map_or(0.0, |q| span / wh / (q.max - q.min)),
        }
    }

    /// The first of a field axis' parameters that is shown (Rate or Rate Sync).
    pub(crate) fn field_axis(d: &Device, def: &DeviceDef, ids: &[&'static str]) -> Option<usize> {
        ids.iter().find(|id| def.is_shown(d, id)).and_then(|id| def.index_of(id))
    }

    /// A plotted field (View::Field) w pt wide: caption, ticks, traces, fill / band, live overlay.
    fn field_view(&self, d: &Device, def: &DeviceDef, f: &Field, w: f32) -> PanelField {
        let (pw, ph) = field_plot_size(f, w);
        let (w, h) = (w * Z, (VIEW_BOTTOM - VIEW_Y) * Z);
        let well_y = if f.caption.is_empty() { 0.0 } else { CAPTION_H * Z };
        let (px, py) = (FIELD_GUTTER * Z, FIELD_TOP * Z);
        let get = |id: &str| def.param(id).map_or(0.0, |p| self.panel_value(d, def, p));
        let input = device_fields::Input {
            get: &get,
            bpm: self.project().transport.bpm as f32,
            n: (pw.round() as usize).max(64),
            cycles: self.devices.panel.lfo.get(&d.id).map(|l| l.1),
            win: false,
        };
        let Some(plot) = device_fields::plot(f.plot, &input) else { return PanelField::default() };
        let at = |x: f32, y: f32| (px + x * pw, py + y * ph);
        let points = |ys: &[f32]| -> Vec<(f32, f32)> {
            ys.iter().enumerate().map(|(k, y)| at(k as f32 / (ys.len() - 1).max(1) as f32, *y)).collect()
        };
        let stroke = |t: &device_fields::Trace| match t.style {
            Style::Solid => polyline(&points(&t.ys)),
            Style::Dash { on, off, .. } => dashed(&points(&t.ys), on * Z, off * Z),
        };
        let alpha = |t: Option<&device_fields::Trace>| match t.map(|t| t.style) {
            Some(Style::Dash { alpha, .. }) => alpha,
            _ => 1.0,
        };
        let main = plot.traces.first().map(|t| points(&t.ys)).unwrap_or_default();
        let rest_y = plot.rest.map(|r| at(0.0, r).1);
        let fill = match (plot.fill, rest_y) {
            (Fill::ToRest, Some(ry)) if main.len() > 1 => {
                format!("{} L{:.2} {ry:.2} L{:.2} {ry:.2} Z", polyline(&main), main[main.len() - 1].0, main[0].0)
            }
            (Fill::Band, _) => {
                let (lo, hi) = plot.traces.iter().flat_map(|t| t.ys.iter()).fold((f32::MAX, f32::MIN), |(a, b), y| (a.min(*y), b.max(*y)));
                let (y0, y1) = (at(0.0, lo).1, at(0.0, hi).1);
                format!("M{px:.2} {y0:.2} L{:.2} {y0:.2} L{:.2} {y1:.2} L{px:.2} {y1:.2} Z", px + pw, px + pw)
            }
            _ => String::new(),
        };
        let ticks: Vec<PanelTick> =
            plot.ticks.iter().map(|(y, text)| PanelTick { y: at(0.0, *y).1, text: s(text.as_str()), rule: true }).collect();
        let (mut live, mut dot, mut dot_x, mut dot_y) = (String::new(), false, 0.0, 0.0);
        // At rest only the phase dot shows; the live curve and the dot's trail follow playback.
        let playing = self.engine.transport().playing;
        match &plot.live {
            Some(Live::Curve(ys)) if playing => live = polyline(&points(ys)),
            Some(Live::Dot { x, y, trail }) => {
                (dot, (dot_x, dot_y)) = (true, at(*x, *y));
                if playing && trail.len() > 1 {
                    live = polyline(&trail.iter().map(|(x, y)| at(*x, *y)).collect::<Vec<_>>());
                }
            }
            _ => {}
        }
        let xi = Self::field_axis(d, def, f.x);
        let yi = Self::field_axis(d, def, f.y);
        PanelField {
            visible: true,
            x: PAD * Z,
            y: VIEW_Y * Z,
            w,
            h,
            well_y,
            caption: s(f.caption),
            readout: s(plot.readout),
            ticks: model(ticks),
            rest: s(rest_y.map(|y| format!("M{px:.2} {y:.2} L{:.2} {y:.2}", px + pw)).unwrap_or_default()),
            fill: s(fill),
            band: plot.fill == Fill::Band,
            main: s(plot.traces.first().map(stroke).unwrap_or_default()),
            main_w: plot.widths.0 * Z,
            main_hot_w: plot.widths.1 * Z,
            alt: s(plot.traces.get(1).map(stroke).unwrap_or_default()),
            alt_alpha: alpha(plot.traces.get(1)),
            third: s(plot.traces.get(2).map(stroke).unwrap_or_default()),
            third_alpha: alpha(plot.traces.get(2)),
            live: s(live),
            dot,
            dot_x,
            dot_y,
            x_param: xi.map_or(-1, |i| i as i32),
            y_param: yi.map_or(-1, |i| i as i32),
            plot_x: px,
            plot_y: well_y + py,
            plot_w: pw,
            plot_h: ph,
        }
    }

    /// Engine readouts -> strip histories, view meters and field LFO phases of
    /// the selected track's built-in devices (every poll tick, ~30 ms).
    pub(crate) fn panel_poll_meters(&mut self) {
        self.sampler_poll();
        self.synth_poll();
        self.poll_window_meters();
        let Some(t) = self.selected_track.and_then(|t| self.project().track(t)) else { return };
        let strips: Vec<(DeviceId, Strip)> = t
            .devices
            .iter()
            .filter_map(|d| dstudio_core::builtin::def_of(d).and_then(|def| def.layout.view).and_then(|v| v.strip()).map(|st| (d.id, st)))
            .collect();
        // Field devices: follow the LFO phase readout; at rest the dot stays where the phase is.
        let playing = self.engine.transport().playing;
        let fields: Vec<DeviceId> = t
            .devices
            .iter()
            .filter(|d| dstudio_core::builtin::def_of(d).is_some_and(|def| matches!(def.layout.view, Some(View::Field(_)))))
            .map(|d| d.id)
            .collect();
        let views: Vec<DeviceId> = t
            .devices
            .iter()
            .filter(|d| {
                dstudio_core::builtin::def_of(d).is_some_and(|def| matches!(def.layout.view, Some(View::Custom(n)) if n != GAIN_SCOPE))
            })
            .map(|d| d.id)
            .collect();
        let canvases: Vec<DeviceId> = t
            .devices
            .iter()
            .filter(|d| {
                dstudio_core::builtin::def_of(d)
                    .is_some_and(|def| matches!(def.layout.view, Some(View::Eq(_)) | Some(View::Custom("auto-filter"))))
            })
            .map(|d| d.id)
            .collect();
        let scopes: Vec<DeviceId> = t
            .devices
            .iter()
            .filter(|d| dstudio_core::builtin::def_of(d).is_some_and(|def| def.layout.view == Some(View::Custom(GAIN_SCOPE))))
            .map(|d| d.id)
            .collect();
        self.devices.panel.meters.retain(|id, _| strips.iter().any(|(s, _)| s == id));
        self.devices.panel.scopes.retain(|id, _| scopes.contains(id));
        let mut out = Vec::new();
        self.devices.panel.canvas.readouts.retain(|id, _| canvases.contains(id));
        for id in canvases {
            if self.engine.builtin_readouts(id.0, &mut out).is_ok() && self.devices.panel.canvas.readouts.get(&id) != Some(&out) {
                self.devices.panel.canvas.readouts.insert(id, out.clone());
                self.ui_dirty = true;
            }
        }
        let now_ms = self.devices.panel.scope_clock.get_or_insert_with(std::time::Instant::now).elapsed().as_secs_f64() * 1e3;
        for id in scopes {
            // Hold view (the expanded window) freezes the scope.
            if self.win_held(id) {
                continue;
            }
            // Merge the newest column frame; the head moves while there is something to show.
            let seq = self.engine.builtin_analysis(id.0, &mut out).unwrap_or(0);
            let h = self.devices.panel.scopes.entry(id).or_default();
            let fresh = seq != 0 && seq != h.seq;
            if fresh {
                h.seq = seq;
                h.push(&out, now_ms, playing);
            }
            let rests = h.rests(playing);
            let moving = !rests && !h.empty();
            if !rests && (fresh || moving) {
                h.advance(now_ms);
            }
            if fresh || moving {
                self.ui_dirty = true;
            }
            if !moving {
                h.park();
            }
        }
        for (id, st) in strips {
            // Readout order of a strip device: input dB, output dB, gain reduction dB.
            let (i, o, g) = match self.engine.builtin_readouts(id.0, &mut out) {
                Ok(()) if out.len() >= 3 => (out[0], out[1], out[2].min(0.0)),
                _ => (SILENT_DB, SILENT_DB, 0.0),
            };
            let h = self.devices.panel.meters.entry(id).or_default();
            let silent = |s: (f32, f32, f32)| s.0 < st.floor_db && s.1 < st.floor_db && s.2 > -0.05;
            let prev = h.slots.back().copied().unwrap_or((st.floor_db - 1.0, st.floor_db - 1.0, 0.0));
            let next = (0.3 * prev.0 + 0.7 * i, 0.3 * prev.1 + 0.7 * o, 0.3 * prev.2 + 0.7 * g);
            if silent(next) && h.slots.iter().all(|s| silent(*s)) {
                // Flat at the floor: nothing to scroll.
                h.slots.clear();
                continue;
            }
            h.last = (i, o, g);
            if h.slots.len() == HISTORY {
                h.slots.pop_front();
            }
            h.slots.push_back(next);
            self.ui_dirty = true;
        }
        // View state (the selected band) lives as long as its device; meters only poll here.
        let gone: Vec<DeviceId> = self.devices.panel.views.keys().copied().filter(|id| self.project().device(*id).is_none()).collect();
        self.devices.panel.views.retain(|id, _| !gone.contains(id));
        for id in views {
            let r = self.engine.builtin_readouts(id.0, &mut out).ok().map(|_| out.clone()).unwrap_or_default();
            if self.devices.panel.views.entry(id).or_default().poll(&r) {
                self.ui_dirty = true;
            }
        }
        self.devices.panel.lfo.retain(|id, _| fields.contains(id));
        for id in fields {
            let Ok(()) = self.engine.builtin_readouts(id.0, &mut out) else { continue };
            let Some(phase) = out.first().copied().filter(|p| p.is_finite()) else { continue };
            // At rest the phase is read once, so the dot holds still and the panel is not rebuilt.
            let fresh = !self.devices.panel.lfo.contains_key(&id);
            let l = self.devices.panel.lfo.entry(id).or_insert((phase, phase as f64));
            if !fresh && (!playing || l.0 == phase) {
                continue;
            }
            // Polled every ~30 ms: the forward step since the last poll (under one cycle).
            l.1 += (phase - l.0).rem_euclid(1.0) as f64;
            l.0 = phase;
            self.ui_dirty = true;
        }
    }

    // ---------------------------------------------------------------- gestures

    pub fn panel_drag(&mut self, device: DeviceId, index: i32, phase: i32, dy: f32, rate: f32, fine: bool) {
        if phase == 0 && self.mod_map_click(device, index) {
            return;
        }
        if index <= crate::sampler_panel::SLICE_GAIN {
            return self.slice_drag(device, index, phase, dy, rate, fine);
        }
        if self.project().device(device).is_some_and(|d| dstudio_core::builtin::def_of(d).is_none()) {
            return self.card_drag(device, index as u32, phase, dy, rate, fine);
        }
        let Some((d, def)) = self.panel_def(device).map(|(d, def)| (d.clone(), def)) else { return };
        let d = &d;
        let Some(p) = usize::try_from(index).ok().and_then(|i| def.params.get(i)) else { return };
        let rack = dstudio_core::modulation::is_rack(d);
        match phase {
            0 => {
                let v = def.value(d, p);
                self.devices.panel.drag = Some(ParamDrag {
                    device,
                    index: index as usize,
                    start: v,
                    norm: p.to_normalized(v),
                    value: v,
                    travel: 0.0,
                    held: 0.0,
                    active: false,
                    linked: None,
                });
            }
            1 => {
                let Some(g) = self.devices.panel.drag.as_mut().filter(|g| g.device == device && g.index == index as usize) else { return };
                g.travel += dy.abs();
                g.held += dy;
                if !g.active && g.travel < DRAG_SLOP {
                    return;
                }
                g.active = true;
                let dir = if p.reversed { -1.0 } else { 1.0 };
                g.norm = (g.norm + dir * std::mem::take(&mut g.held) * rate * if fine { 0.2 } else { 1.0 }).clamp(0.0, 1.0);
                let v = if p.is_discrete() { p.from_normalized(g.norm).round() } else { p.from_normalized(g.norm) };
                let v = snap_to_detent(p, def.detents(d, p), g.norm, fine).unwrap_or(v);
                if v != g.value {
                    g.value = v;
                    // The engine gets the model's values with this edit (and what it links) on
                    // top, so links that stop applying mid-drag fall back; the model changes on release.
                    let mut live = def.values(d);
                    for (id, x) in def.linked_values(d, p.id, v) {
                        if let Some(i) = def.index_of(&id) {
                            live[i] = x;
                        }
                    }
                    for (i, x) in live.into_iter().enumerate() {
                        let _ = self.engine.builtin_set_param(device.0, i as u32, x);
                    }
                    self.ui_dirty = true;
                }
            }
            2 => {
                let clicked = self.devices.panel.drag.is_some_and(|g| g.device == device && !g.active);
                let Some(g) = self.devices.panel.drag.take() else { return };
                self.ui_dirty = true;
                // A click on a chain macro opens (or closes) its side panel.
                if rack && clicked {
                    return self.macro_select(device, index);
                }
                if g.active && g.value != g.start {
                    self.panel_commit(param_command(def, d, p, g.value));
                } else if g.active {
                    for (i, x) in def.values(d).into_iter().enumerate() {
                        let _ = self.engine.builtin_set_param(device.0, i as u32, x);
                    }
                }
            }
            _ => {
                self.panel_cancel_drag();
            }
        }
    }

    /// Parameter index of the running drag.
    pub(crate) fn panel_drag_index(&self) -> Option<i32> {
        self.devices.panel.drag.map(|g| g.index as i32)
    }

    /// True once the running drag has passed the start threshold.
    pub(crate) fn panel_drag_active(&self) -> bool {
        self.devices.panel.drag.is_some_and(|g| g.active)
    }

    /// Alt+drag on a two-line strip: both lines move by the same amount, clamped
    /// so neither leaves its range; one undo step. Alt-click without a move resets `index`.
    pub fn panel_drag_pair(&mut self, device: DeviceId, index: i32, other: i32, phase: i32, dy: f32, rate: f32, fine: bool) {
        let Some((d, def)) = self.panel_def(device) else { return };
        let param = |i: i32| usize::try_from(i).ok().and_then(|i| def.params.get(i).map(|p| (i, p)));
        let (Some((ia, pa)), Some((ib, pb))) = (param(index), param(other)) else { return };
        match phase {
            0 => {
                let (va, vb) = (def.value(d, pa), def.value(d, pb));
                let linked = Some(Linked { index: ib, start: vb, value: vb, delta: 0.0 });
                self.devices.panel.drag =
                    Some(ParamDrag { device, index: ia, start: va, norm: 0.0, value: va, travel: 0.0, held: 0.0, active: false, linked });
            }
            1 => {
                let Some(g) = self.devices.panel.drag.as_mut().filter(|g| g.device == device && g.index == ia) else { return };
                let Some(l) = g.linked.as_mut() else { return };
                g.travel += dy.abs();
                l.delta += dy * rate * if fine { 0.2 } else { 1.0 } * (pa.max - pa.min);
                if !g.active && g.travel < DRAG_SLOP {
                    return;
                }
                g.active = true;
                let delta = l.delta.clamp((pa.min - g.start).max(pb.min - l.start), (pa.max - g.start).min(pb.max - l.start));
                let (va, vb) = (pa.snap(g.start + delta), pb.snap(l.start + delta));
                if (va, vb) != (g.value, l.value) {
                    (g.value, l.value) = (va, vb);
                    let _ = self.engine.builtin_set_param(device.0, ia as u32, va);
                    let _ = self.engine.builtin_set_param(device.0, ib as u32, vb);
                    self.ui_dirty = true;
                }
            }
            2 => {
                let Some(g) = self.devices.panel.drag else { return };
                let Some(l) = g.linked else { return };
                let params = [(pa.id.to_string(), g.value), (pb.id.to_string(), l.value)].into_iter().collect();
                let cmd = (g.active && (g.value, l.value) != (g.start, l.start)).then(|| Command::SetDeviceParams {
                    device,
                    params,
                    preset: d.preset.clone(),
                });
                self.devices.panel.drag = None;
                self.ui_dirty = true;
                match cmd {
                    Some(cmd) => {
                        self.panel_commit(cmd);
                    }
                    None if !g.active => self.panel_reset(device, index),
                    None => {}
                }
            }
            _ => {
                self.panel_cancel_drag();
            }
        }
    }

    /// Escape or a cancelled pointer: restore the values from before the drag
    /// (the dragged one and any it linked), with no undo step.
    pub fn panel_cancel_drag(&mut self) -> bool {
        self.sampler_release_all();
        if self.sampler_cancel_drags() || self.panel_cancel_graph_drag() {
            return true;
        }
        let Some(g) = self.devices.panel.drag.take() else { return false };
        if let Some((d, def)) = self.panel_def(g.device) {
            let values = def.values(d);
            for (i, v) in values.into_iter().enumerate() {
                let _ = self.engine.builtin_set_param(g.device.0, i as u32, v);
            }
        }
        self.ui_dirty = true;
        true
    }

    fn panel_set(&mut self, device: DeviceId, index: i32, value: impl FnOnce(&ParamDef, f32, &[f32]) -> Option<f32>) {
        if index <= crate::sampler_panel::SLICE_GAIN {
            return self.slice_set(device, index, |p, v| value(p, v, &[]));
        }
        let Some((d, def)) = self.panel_def(device) else { return };
        let Some(p) = usize::try_from(index).ok().and_then(|i| def.params.get(i)) else { return };
        let cur = def.value(d, p);
        if let Some(v) = value(p, cur, def.detents(d, p)).map(|v| p.snap(v))
            && v != cur
        {
            let cmd = param_command(def, d, p, v);
            self.panel_commit(cmd);
        }
    }

    /// Field drag: x and y move the field's two parameters at once (travel =
    /// plot width / height for the whole range; a stepped parameter walks its
    /// steps). Phases as panel_drag; dx / dy in px since the last event (up +).
    pub fn panel_field_drag(&mut self, device: DeviceId, phase: i32, dx: f32, dy: f32, fine: bool) {
        let Some((d, def)) = self.panel_def(device) else { return };
        let Some(View::Field(f)) = def.layout.view else { return };
        let axes = [Self::field_axis(d, def, f.x), Self::field_axis(d, def, f.y)];
        let (pw, ph) = field_plot_size(&f, view_right(def) - PAD);
        self.axes_drag(device, axes, phase, [dx, dy], [1.0 / pw.max(1.0), 1.0 / ph.max(1.0)], fine);
    }

    /// Drag zone of a custom body (View::Custom): params [horizontal, vertical]
    /// (-1 = unused), deltas since the last event (px, right / up +) and the
    /// norm-per-px rate of each axis.
    pub fn panel_drag_xy(&mut self, device: DeviceId, params: [i32; 2], phase: i32, delta: [f32; 2], rate: [f32; 2], fine: bool) {
        let Some((_, def)) = self.panel_def(device) else { return };
        let axes = params.map(|i| usize::try_from(i).ok().filter(|&i| i < def.params.len()));
        self.axes_drag(device, axes, phase, delta, rate, fine);
    }

    /// Two-axis drag (fields and zones): live values go to the engine; release
    /// commits one command (a Batch when both axes moved): one undo step.
    fn axes_drag(&mut self, device: DeviceId, axes: [Option<usize>; 2], phase: i32, delta: [f32; 2], rate: [f32; 2], fine: bool) {
        let Some((d, def)) = self.panel_def(device) else { return };
        match phase {
            0 => {
                let axis = |i: Option<usize>| {
                    i.map(|i| {
                        let v = def.value(d, &def.params[i]);
                        (i, v, def.params[i].to_normalized(v), v)
                    })
                };
                self.devices.panel.field = Some(FieldDrag { device, axes: [axis(axes[0]), axis(axes[1])], travel: 0.0, active: false });
            }
            1 => {
                let Some(g) = self.devices.panel.field.as_mut().filter(|g| g.device == device) else { return };
                g.travel += delta[0].abs() + delta[1].abs();
                if !g.active && g.travel < DRAG_SLOP {
                    return;
                }
                g.active = true;
                let k = if fine { 0.2 } else { 1.0 };
                let mut sends = Vec::new();
                for (a, (dl, r)) in g.axes.iter_mut().zip(delta.into_iter().zip(rate)) {
                    let Some((i, _, norm, value)) = a else { continue };
                    let p = &def.params[*i];
                    *norm = (*norm + dl * r * k).clamp(0.0, 1.0);
                    let v = if p.is_discrete() { p.from_normalized(*norm).round() } else { p.from_normalized(*norm) };
                    if v != *value {
                        *value = v;
                        sends.push((*i, v));
                    }
                }
                for (i, v) in sends {
                    let _ = self.engine.builtin_set_param(device.0, i as u32, v);
                    self.ui_dirty = true;
                }
            }
            2 => {
                let Some(g) = self.devices.panel.field.take() else { return };
                self.ui_dirty = true;
                let commands: Vec<Command> = g
                    .axes
                    .iter()
                    .flatten()
                    .filter(|a| g.active && a.3 != a.1)
                    .map(|a| Command::SetDeviceParam { device, param: def.params[a.0].id.to_string(), value: a.3 })
                    .collect();
                match commands.len() {
                    0 => {}
                    1 => {
                        self.panel_commit(commands.into_iter().next().unwrap());
                    }
                    _ => {
                        self.panel_commit(Command::Batch { label: "Device Parameter Change".into(), commands });
                    }
                }
            }
            _ => {
                if let Some(g) = self.devices.panel.field.take() {
                    for a in g.axes.iter().flatten() {
                        let _ = self.engine.builtin_set_param(device.0, a.0 as u32, a.1);
                    }
                    self.ui_dirty = true;
                }
            }
        }
    }

    /// One gesture = one undo step: never merge with the previous edit of the same parameter.
    pub(crate) fn panel_commit(&mut self, cmd: Command) -> bool {
        let cmd = self.sampler_followups(cmd);
        self.doc.seal_coalescing();
        let ok = self.execute(cmd);
        self.doc.seal_coalescing();
        ok
    }

    pub fn panel_reset(&mut self, device: DeviceId, index: i32) {
        if self.project().device(device).is_some_and(|d| dstudio_core::builtin::def_of(d).is_none()) {
            return self.card_reset(device, index as u32);
        }
        self.panel_set(device, index, |p, _, _| Some(p.default));
    }

    /// Reset of a two-parameter field zone: one command (a Batch when both change), one undo step.
    pub fn panel_reset_xy(&mut self, device: DeviceId, params: [i32; 2]) {
        let Some((d, def)) = self.panel_def(device) else { return };
        let mut commands: Vec<Command> = params
            .iter()
            .filter_map(|&i| usize::try_from(i).ok().and_then(|i| def.params.get(i)))
            .filter(|p| def.value(d, p) != p.default)
            .map(|p| Command::SetDeviceParam { device, param: p.id.to_string(), value: p.default })
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

    /// Arrow keys: discrete parameters move one choice, continuous ones 1% of
    /// the range (0.2% fine), or to the next detent when the control has them.
    pub fn panel_step(&mut self, device: DeviceId, index: i32, delta: i32, fine: bool) {
        if self.project().device(device).is_some_and(|d| dstudio_core::builtin::def_of(d).is_none()) {
            return self.card_step(device, index as u32, delta, fine);
        }
        self.panel_set(device, index, |p, v, detents| {
            if p.momentary {
                return None;
            }
            let delta = if p.reversed { -delta } else { delta };
            if p.is_discrete() {
                return Some(v + delta as f32);
            }
            let tol = 1e-4 * (p.max - p.min);
            let next = if delta > 0 {
                detents.iter().copied().filter(|x| *x > v + tol).reduce(f32::min)
            } else {
                detents.iter().copied().filter(|x| *x < v - tol).reduce(f32::max)
            };
            if !detents.is_empty() {
                return next;
            }
            Some(p.from_normalized(p.to_normalized(v) + delta as f32 * if fine { 0.002 } else { 0.01 }))
        });
    }

    /// A choice (segment, menu row or latch state); `VIEW_SELECT` picks the
    /// card's view state instead of a parameter.
    pub fn panel_choice(&mut self, device: DeviceId, index: i32, choice: i32) {
        if self.project().device(device).is_some_and(|d| dstudio_core::builtin::def_of(d).is_none()) {
            return self.card_click(device, index as u32);
        }
        if index == VIEW_SELECT {
            self.panel_view(device, 1, choice as f32);
            return;
        }
        // Picking a Key or Scale while it shows the followed project key turns Follow off and
        // writes both values (the picked one and the one shown for the other) in one gesture.
        // Without a project key nothing is followed, so the pick is an ordinary choice.
        if let Some((d, def)) = self.panel_def(device)
            && let Some(p) = usize::try_from(index).ok().and_then(|i| def.params.get(i))
            && (p.id == "key" || p.id == "scale")
            && let Some((k, sc)) = dstudio_core::builtin::followed_key(def, d, self.project().transport.key)
        {
            let (k, sc) = if p.id == "key" { (choice as f32, sc) } else { (k, choice as f32) };
            let commands = [("key_follow", 0.0), ("key", k), ("scale", sc)]
                .into_iter()
                .filter_map(|(id, v)| def.param(id).map(|q| Command::SetDeviceParam { device, param: id.into(), value: q.snap(v) }))
                .collect();
            self.panel_commit(Command::Batch { label: "Device Parameter Change".into(), commands });
            return;
        }
        let latch = self.panel_def(device).and_then(|(d, def)| {
            let p = def.params.get(usize::try_from(index).ok()?)?;
            dstudio_core::builtin::behavior(def).and_then(|b| (b.latch)(def, d, p))
        });
        self.panel_set(device, index, |_, _, _| Some(latch.map_or(choice as f32, |l| l.values[choice.clamp(0, 1) as usize])));
    }

    pub fn panel_text(&mut self, device: DeviceId, index: i32, text: &str) {
        if self.project().device(device).is_some_and(|d| dstudio_core::builtin::def_of(d).is_none()) {
            return self.card_text(device, index as u32, text);
        }
        // Text the panel shows (e.g. "AUTO", "DIAL 3") committed unchanged is not an edit.
        let shown = self.panel_def(device).and_then(|(d, def)| {
            let p = def.params.get(usize::try_from(index).ok()?)?;
            Some(def.text(d, p, def.value(d, p)))
        });
        if shown.is_some_and(|s| s.trim() == text.trim()) {
            return;
        }
        let Some((d, def)) = self.panel_def(device) else { return };
        let Some(p) = usize::try_from(index).ok().and_then(|i| def.params.get(i)) else { return };
        let parsed = def.parse_of(d, p, text);
        let Some(v) = self.sampler_parse(device, p, text).or(parsed) else {
            let msg = format!("{}: can't read “{}”", p.name, text.trim());
            return self.set_status(msg, 3);
        };
        self.panel_set(device, index, |_, _, _| Some(v));
    }

    /// Latch: 0 click toggles (one undo step); a momentary parameter is on from 1 (press) to
    /// 2 (release / cancel), live in the engine only, with no model change.
    pub fn panel_latch(&mut self, device: DeviceId, index: i32, phase: i32) {
        let Some((_, def)) = self.panel_def(device) else { return };
        let Some(i) = usize::try_from(index).ok().filter(|i| *i < def.params.len()) else { return };
        let p = &def.params[i];
        match phase {
            0 => self.panel_set(device, index, |p, v, _| Some(if v >= 0.5 { p.min } else { p.max })),
            1 if p.momentary => {
                self.devices.panel.held = Some((device, i));
                let _ = self.engine.builtin_set_param(device.0, i as u32, p.max);
                self.ui_dirty = true;
            }
            2 if self.devices.panel.held.take().is_some() => {
                let _ = self.engine.builtin_set_param(device.0, i as u32, p.min);
                self.ui_dirty = true;
            }
            _ => {}
        }
    }

    /// Sidechain picker (-1), presets (-2) or the choices of parameter `which` (>= 0) at window x, y.
    pub fn panel_menu(&mut self, device: DeviceId, which: i32, x: f32, y: f32) {
        let Some((d, def)) = self.panel_def(device) else { return };
        let check = |on: bool| if on { "✓".to_string() } else { String::new() };
        let rows = match which {
            crate::device_window::MENU_ANALYZER => self.win_analyzer_rows(device),
            crate::device_window::MENU_TARGET => self.win_target_rows(device),
            crate::device_window::MENU_MB_BAND => self.win_mb_band_rows(device),
            crate::device_window::MENU_DELAY_BAND => self.win_delay_band_rows(device),
            crate::device_window::MENU_BANDS => match self.win_band_rows(device) {
                Some(rows) => rows,
                None => return,
            },
            -1 => {
                let own = self.project().find_device(device).map(|(ti, _)| self.project().tracks[ti].id);
                let mut rows = vec![("None".to_string(), check(d.sidechain.is_none()), Some(MenuPick::Sidechain(device, None)), false)];
                for t in self
                    .project()
                    .tracks
                    .iter()
                    .filter(|t| Some(t.id) != own && (d.sidechain == Some(t.id) || !self.sidechain_feeds_back(device, t.id)))
                {
                    rows.push((t.name.clone(), check(d.sidechain == Some(t.id)), Some(MenuPick::Sidechain(device, Some(t.id))), false));
                }
                rows
            }
            -2 => {
                let mut rows = vec![("Factory presets".to_string(), String::new(), None, false)];
                for (i, f) in def.factory.iter().enumerate() {
                    rows.push((f.name.to_string(), check(d.preset.as_deref() == Some(f.name)), Some(MenuPick::Preset(device, i)), false));
                }
                rows
            }
            param if param >= 0 => {
                let Some(p) = def.params.get(param as usize).filter(|p| p.is_discrete()) else { return };
                // Key and Scale check the followed project key, as their pills show it.
                let cur = dstudio_core::builtin::followed_key(def, d, self.project().transport.key)
                    .filter(|_| p.id == "key" || p.id == "scale")
                    .map_or_else(|| def.value(d, p), |(k, sc)| if p.id == "key" { k } else { sc })
                    .round() as usize;
                let mode = def.layout.picker.iter().find_map(|c| match c {
                    Control::ModeDropdown { param: q, mode } if *q == p.id => def.param(mode),
                    _ => None,
                });
                let in_mode = mode.is_some_and(|m| def.value(d, m) >= 0.5);
                let is_bank = matches!(def.layout.view, Some(View::Eq(v)) if v.bank == Some(p.id));
                let mut rows: Vec<_> = p
                    .choices
                    .iter()
                    .enumerate()
                    .map(|(i, c)| {
                        (
                            c.to_string(),
                            check(!in_mode && i == cur),
                            Some(if is_bank || mode.is_some() {
                                MenuPick::Param(device, param as usize, i as f32)
                            } else {
                                MenuPick::Choice(device, param, i as i32)
                            }),
                            false,
                        )
                    })
                    .collect();
                if let Some(m) = mode {
                    let name = m.choices.get(1).copied().unwrap_or("");
                    rows.push((
                        name.to_string(),
                        check(in_mode),
                        Some(MenuPick::Mode(device, param as usize, def.index_of(m.id).unwrap_or(0))),
                        false,
                    ));
                }
                rows
            }
            _ => return,
        };
        self.devices.menu = Some(DevMenu { pos: (x, y), w: 180.0, rows });
        self.ui_dirty = true;
    }

    /// True when `from` already listens to the track of `device` (directly or
    /// through other sidechains): picking it as a source would form a cycle.
    pub(crate) fn sidechain_feeds_back(&self, device: DeviceId, from: TrackId) -> bool {
        let own = self.project().find_device(device).map(|(ti, _)| self.project().tracks[ti].id);
        let mut stack = vec![from];
        let mut seen = std::collections::HashSet::new();
        while let Some(t) = stack.pop() {
            if Some(t) == own {
                return true;
            }
            if seen.insert(t)
                && let Some(tr) = self.project().track(t)
            {
                stack.extend(tr.devices.iter().filter_map(|x| x.sidechain));
            }
        }
        false
    }

    /// Previous / next factory preset (wraps; from no preset, next is the first).
    pub fn panel_preset_step(&mut self, device: DeviceId, dir: i32) {
        let Some((d, def)) = self.panel_def(device) else { return };
        let n = def.factory.len() as i32;
        if n == 0 {
            return;
        }
        let cur = d.preset.as_deref().and_then(|name| def.factory.iter().position(|f| f.name == name)).map(|i| i as i32);
        let next = match cur {
            Some(i) => (i + dir).rem_euclid(n),
            None if dir > 0 => 0,
            None => n - 1,
        };
        self.panel_load_preset(device, next as usize);
    }

    pub(crate) fn panel_load_preset(&mut self, device: DeviceId, n: usize) {
        let Some((d, def)) = self.panel_def(device) else { return };
        let Some(f) = def.factory.get(n) else { return };
        let mut params = def.preset_load_values(d, &dstudio_core::builtin::DevicePreset::factory(def, f));
        // The sidechain switch follows the source picked in the panel, not the preset.
        if let Some(p) = def.sidechain_enable() {
            params.insert(p.id.into(), def.value(d, p));
        }
        let cmd = self.sampler_followups(Command::SetDeviceParams { device, params, preset: Some(f.name.to_string()) });
        self.execute(cmd);
    }

    /// Sidechain source pick: the source and the device's enable switch in one undo step.
    pub(crate) fn panel_set_sidechain(&mut self, device: DeviceId, source: Option<TrackId>) {
        let Some((d, def)) = self.panel_def(device) else { return };
        if d.sidechain == source {
            return;
        }
        let mut commands = vec![Command::SetDeviceSidechain { device, source }];
        if let Some(p) = def.sidechain_enable() {
            commands.push(Command::SetDeviceParam { device, param: p.id.into(), value: if source.is_some() { 1.0 } else { 0.0 } });
        }
        self.execute(Command::Batch { label: "Sidechain Change".into(), commands });
    }
}

impl Controller {
    /// A parameter menu row: Bank re-snaps the EQ bands; a type menu that
    /// shares a mode switch also leaves the mode (Auto Filter: DJ -> Filter).
    pub(crate) fn panel_pick_param(&mut self, device: DeviceId, param: usize, value: f32) {
        let Some((d, def)) = self.panel_def(device) else { return };
        let Some(p) = def.params.get(param) else { return };
        if let Some(View::Eq(v)) = def.layout.view
            && v.bank == Some(p.id)
        {
            return self.panel_pick_bank(device, param, value);
        }
        let mode = def.layout.picker.iter().find_map(|c| match c {
            Control::ModeDropdown { param: q, mode } if *q == p.id => def.param(mode),
            _ => None,
        });
        let mut commands = vec![Command::SetDeviceParam { device, param: p.id.into(), value }];
        if let Some(m) = mode.filter(|m| def.value(d, m) >= 0.5) {
            commands.push(Command::SetDeviceParam { device, param: m.id.into(), value: 0.0 });
        } else if def.value(d, p) == value {
            return;
        }
        self.panel_commit(Command::Batch { label: "Device Parameter Change".into(), commands });
    }

    /// The mode entry of a type menu: switches the mode on. Entering Auto
    /// Filter's DJ mode with no resonance gives it a little, so the sweep is audible.
    pub(crate) fn panel_pick_mode(&mut self, device: DeviceId, mode: usize) {
        let Some((d, def)) = self.panel_def(device) else { return };
        let Some(m) = def.params.get(mode) else { return };
        if def.value(d, m) >= 0.5 {
            return;
        }
        let mut commands = vec![Command::SetDeviceParam { device, param: m.id.into(), value: 1.0 }];
        if let Some(r) = def.param("resonance").filter(|r| def.value(d, r) <= 0.0) {
            commands.push(Command::SetDeviceParam { device, param: r.id.into(), value: DJ_RESONANCE });
        }
        self.panel_commit(Command::Batch { label: "Device Parameter Change".into(), commands });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compressor_card_geometry_matches_the_reference_template() {
        let def = dstudio_core::builtin::find("compressor").unwrap();
        let ids: Vec<&'static str> = def.layout.clusters.iter().flat_map(|c| c.iter()).filter_map(|c| c.param()).collect();
        let xs: Vec<f32> = dial_xs(def, &ids).iter().map(|(_, x)| *x).collect();
        // Even spacing (knob centres 74 pt apart): gap (521 - 16 - 7 x 62) / 6.
        let step = KNOB_CELL + 71.0 / 6.0;
        for (i, x) in xs.iter().enumerate() {
            assert!((x - (PAD + i as f32 * step)).abs() < 1e-3, "knob {i} at {x}");
        }
        // The last knob cell ends at the card padding.
        assert!((xs[6] + KNOB_CELL + PAD - def.layout.width()).abs() < 1e-3);
    }

    #[test]
    fn mode_dependent_dial_rows_keep_their_spacing_and_centre() {
        let def = dstudio_core::builtin::find("character-compressor").unwrap();
        let six = dial_xs(def, &["threshold", "ratio", "knee", "attack", "release", "mix"]);
        assert!((six[0].1 - PAD).abs() < 1e-3 && (six[5].1 + KNOB_CELL + PAD - 471.0).abs() < 1e-3);
        let four = dial_xs(def, &["threshold", "ratio", "drive", "mix"]);
        let step = six[1].1 - six[0].1;
        assert!((four[1].1 - four[0].1 - step).abs() < 1e-3, "same gap");
        assert!(((four[0].1 + four[3].1 + KNOB_CELL) / 2.0 - 471.0 / 2.0).abs() < 1e-3, "centred");
    }

    #[test]
    fn detents_snap_within_reach() {
        let def = dstudio_core::builtin::find("character-compressor").unwrap();
        let ratio = def.param("ratio").unwrap();
        let n4 = ratio.to_normalized(4.0);
        assert_eq!(snap_to_detent(ratio, &[2.0, 4.0, 10.0], n4 + 0.02, false), Some(4.0));
        assert_eq!(snap_to_detent(ratio, &[2.0, 4.0, 10.0], n4 + 0.02, true), None, "fine drags snap closer");
        assert_eq!(snap_to_detent(ratio, &[], n4, false), None);
        assert_eq!(detent_ticks(ratio, &[2.0, 4.0]).matches('M').count(), 2);
    }

    #[test]
    fn transfer_face_paths() {
        let t = Transfer { lo: -30.0, hi: 6.0, points: vec![(-30.0, -30.0), (-18.0, -18.0), (6.0, -12.0)], bend: (-20.0, -16.0) };
        let (curve, bend, unity) = transfer_face(&t);
        assert!(curve.starts_with("M3.0 29.0"));
        assert_eq!(bend.matches('L').count(), 0, "a single bend point draws nothing");
        assert!(bend.is_empty());
        assert!(!unity.is_empty());
    }

    #[test]
    fn field_cards_centre_an_even_dial_row() {
        let xs = |id: &str| {
            let mut v: Vec<f32> = {
                let def = dstudio_core::builtin::find(id).unwrap();
                knob_xs(def, &def.instantiate())
            }
            .iter()
            .map(|(_, x)| (x * 100.0).round() / 100.0)
            .collect();
            v.dedup();
            v
        };
        // Tremolo knob centres 115 .. 342 pt, character mod 66 .. 376 pt.
        assert_eq!(xs("tremolo"), [84.0, 159.67, 235.33, 311.0]);
        assert_eq!(xs("character-mod"), [35.0, 112.5, 190.0, 267.5, 345.0]);
        assert_eq!(xs("ring-mod"), [9.0, 86.5, 164.0, 241.5, 319.0]);
    }

    #[test]
    fn character_switch_is_one_labelled_batch() {
        let def = dstudio_core::builtin::find("character-mod").unwrap();
        let d = def.instantiate();
        let p = def.param("model").unwrap();
        match param_command(def, &d, p, 7.0) {
            Command::Batch { label, commands } => {
                assert_eq!(label, "Character Change, with Hiss moved to its own defaults");
                assert_eq!(commands.len(), 2);
            }
            c => panic!("{c:?}"),
        }
        assert!(matches!(param_command(def, &d, def.param("depth").unwrap(), 70.0), Command::SetDeviceParam { .. }));
    }

    #[test]
    fn dashed_lines_alternate_along_the_path() {
        assert_eq!(dashed(&[(0.0, 0.0), (10.0, 0.0)], 4.0, 3.0), "M0.0 0.0 L4.0 0.0 M7.0 0.0 L10.0 0.0 ");
    }

    #[test]
    fn soft_knee_law() {
        assert_eq!(soft_knee_gr(-40.0, -24.0, 4.0, 6.0), 0.0);
        assert!((soft_knee_gr(-12.0, -24.0, 4.0, 6.0) - (-9.0)).abs() < 1e-5);
        // Continuous at both knee edges.
        assert!(soft_knee_gr(-27.0, -24.0, 4.0, 6.0).abs() < 1e-6);
        assert!((soft_knee_gr(-21.0, -24.0, 4.0, 6.0) - (-2.25)).abs() < 1e-5);
        assert_eq!(soft_knee_gr(-10.0, -24.0, 1.0, 0.0), 0.0);
    }

    /// Per-refresh model cost of a compressor card with a full meter history
    /// (`cargo test -p dstudio-app --release bench_panel_model -- --ignored --nocapture`).
    #[test]
    #[ignore]
    fn bench_panel_model() {
        let slots: VecDeque<(f32, f32, f32)> = (0..HISTORY)
            .map(|i| {
                let t = i as f32 / 10.0;
                (-12.0 + 6.0 * t.sin(), -14.0 + 3.0 * t.cos(), (-6.0 + 5.0 * t.sin()).min(0.0))
            })
            .collect();
        let n = 2000;
        let t0 = std::time::Instant::now();
        for _ in 0..n {
            std::hint::black_box(strip_paths(std::hint::black_box(&slots), Strip::new("threshold"), 368.0, 112.2));
        }
        let strip = t0.elapsed().as_secs_f64() * 1e6 / n as f64;
        let t0 = std::time::Instant::now();
        for i in 0..n {
            let t = -24.0 + (i % 7) as f32;
            let (law, at, knee) = face_law(Face::SoftKneeCurve { threshold: "t", ratio: "r", knee: "k" }, |id| match id {
                "t" => t,
                "r" => 4.0,
                _ => 6.0,
            })
            .unwrap();
            std::hint::black_box(law_face(&law, at, knee));
        }
        let face = t0.elapsed().as_secs_f64() * 1e6 / n as f64;
        let cc = dstudio_core::builtin::find("character-compressor").unwrap();
        let mut d = cc.instantiate();
        d.params.insert("model".into(), 1.0);
        let b = dstudio_core::builtin::behavior(cc).unwrap();
        let t0 = std::time::Instant::now();
        for i in 0..n {
            d.params.insert("threshold".into(), -24.0 + (i % 7) as f32);
            std::hint::black_box(transfer_face(&(b.transfer)(cc, &d).unwrap()));
        }
        let vm = t0.elapsed().as_secs_f64() * 1e6 / n as f64;
        println!(
            "panel model: strip paths (240-slot history) {strip:.2} us, transfer face {face:.2} us, Vari-Mu face {vm:.2} us per refresh"
        );
    }

    #[test]
    fn face_laws() {
        let num = |id: &str| match id {
            "t" => -10.0,
            "c" => -1.0,
            "r" => 4.0,
            "range" => 3.0,
            _ => 0.0,
        };
        let law = |f: Face| face_law(f, num).unwrap().0;
        let wall = law(Face::BrickWall { ceiling: "c" });
        assert_eq!((wall(-20.0), wall(5.0)), (-20.0, -1.0));
        let push = law(Face::PushWall { threshold: "t", ceiling: "c" });
        assert_eq!((push(-30.0), push(0.0)), (-21.0, -1.0));
        let gate = law(Face::Expander { threshold: "t", ratio: "r" });
        assert_eq!((gate(-12.0), gate(-5.0)), (-18.0, -5.0));
        // Bus VCA reduction stops at the range.
        assert_eq!(law(Face::BusVca { threshold: "t", ratio: "r", range: "range" })(10.0), 7.0);
    }

    #[test]
    fn strip_paths_and_dashes() {
        assert_eq!(polyline(&[(0.0, 1.0), (2.0, 3.0)]), "M0.0 1.0 L2.0 3.0");
        assert_eq!(dashes(0.0, 10.0, 5.0, 6.0, 5.0), "M0.00 5.00 L6.00 5.00 ");
        assert_eq!(fmt_db(-0.02), "0.0 dB");
        assert_eq!(fmt_db(-3.26), "\u{2212}3.3 dB");
    }
}
