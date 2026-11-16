//! Custom views of the analyser-style built-ins (docs/devices.md, View::Custom):
//! the Pitch Corrector's pitch trace, the Tuner and the Spectrum.
//!
//! The engine publishes what these views show (readouts for the pitch trace
//! and the tuner, analysis frames for the spectrum); every poll tick
//! ([Controller::panel_poll_views], ~30 ms) folds the new data into a small
//! per-device state, and [Controller::analyzer_canvas] turns that state into a
//! [PanelCanvas] (paths and texts in well coordinates; boxes are rounded-rect
//! paths) that
//! ui/device_panel.slint draws. Lengths are the 100%-zoom card
//! measurements times [Z].

use crate::controller::Controller;
use crate::device_panel::{DIAL_Y, Z, polyline};
use crate::device_views::CustomBody;
use crate::{PanelCanvas, PanelPath, PanelText};
use dstudio_core::builtin::{DeviceDef, TUNING_STRINGS, View};
use dstudio_core::{Device, DeviceId};
use slint::{Color, ModelRc, SharedString, VecModel};
use std::collections::{HashMap, VecDeque};
use std::rc::Rc;
use std::time::{Duration, Instant};

/// Dial-row arrangement of an analyzer card (pt);
/// the card width is the definition's `Layout::fixed_width`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ViewMetrics {
    /// Knob cell width.
    pub cell: f32,
    /// Cells spread with the free space between them (else around them).
    pub between: bool,
}

pub fn metrics(view: &str) -> Option<ViewMetrics> {
    match view {
        "pitch-trace" => Some(ViewMetrics { cell: 76.0, between: false }),
        "tuner" => Some(ViewMetrics { cell: 62.0, between: false }),
        "spectrum" => Some(ViewMetrics { cell: 66.0, between: true }),
        _ => None,
    }
}

/// Left edge of knob cell `i` of `n` in a dial row spread over a card `width` wide (card relative, pt).
pub fn spread_x(m: &ViewMetrics, width: f32, i: usize, n: usize) -> f32 {
    let inner = width - 16.0;
    if m.between && n > 1 {
        8.0 + i as f32 * (inner - m.cell) / (n - 1) as f32
    } else {
        let slot = inner / n.max(1) as f32;
        8.0 + slot * i as f32 + (slot - m.cell) / 2.0
    }
}

/// Field geometry (card relative, pt): the view row of the template for the
/// trace and the spectrum (caption above a well), the whole body for the tuner.
const FIELD_X: f32 = 8.0;
const FIELD_Y: f32 = 63.0;
const FIELD_BOTTOM: f32 = 214.0;
const CAPTION_H: f32 = 19.0;
const TUNER_BOTTOM: f32 = 303.0;
/// Plot gutter left of the trace and spectrum grids.
const GUTTER: f32 = 24.0;

const TRACE_FRAMES: usize = 256;
const TRACE_WINDOW: f32 = 4.0;
const TRACE_RANGE_CT: f32 = 120.0;
const BANDS: usize = 256;
const FLOOR_DB: f32 = -120.0;
const SUCCESS: Color = Color::from_rgb_u8(0x4c, 0xaf, 0x50);
/// Spectrum stroke colours by Color Scheme.
const PALETTE: [Color; 6] = [
    Color::from_rgb_u8(0xd4, 0xa0, 0x4a),
    Color::from_rgb_u8(0xff, 0x7a, 0x3a),
    Color::from_rgb_u8(0x3f, 0xbc, 0x73),
    Color::from_rgb_u8(0xf0, 0x78, 0x60),
    Color::from_rgb_u8(0x7e, 0xc8, 0xe8),
    Color::from_rgb_u8(0xc8, 0xd0, 0xdc),
];
const NOTES: [&str; 12] = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];

#[derive(Debug, Clone, Copy)]
struct TraceFrame {
    t: Instant,
    voiced: bool,
    pitch: f32,
    shift: f32,
    target: f32,
}

#[derive(Debug, Clone, Default)]
struct TunerView {
    voiced: bool,
    note: i32,
    cents: f32,
    in_tune: bool,
    string: i32,
    holding: bool,
    /// Animated values: marker cents (110 ms ease), voice / tuned / hold fades.
    marker: f32,
    voice: f32,
    tuned: f32,
    hold: f32,
    last: Option<Instant>,
    /// Spoken text (live region): the newest wanted text, the one shown, when it changed.
    wanted: String,
    spoken: String,
    spoken_at: Option<Instant>,
}

#[derive(Debug, Clone, Default)]
struct SpectrumView {
    seq: u64,
    avg: Vec<f32>,
    side: Vec<f32>,
    peak: Vec<f32>,
    /// Smoothed curves as drawn.
    shown: Vec<f32>,
    shown_side: Vec<f32>,
}

#[derive(Debug, Default)]
pub struct AnalyzerState {
    traces: HashMap<DeviceId, VecDeque<TraceFrame>>,
    tuners: HashMap<DeviceId, TunerView>,
    spectra: HashMap<DeviceId, SpectrumView>,
}

fn s(v: impl Into<SharedString>) -> SharedString {
    v.into()
}

fn model<T: Clone + 'static>(v: Vec<T>) -> ModelRc<T> {
    ModelRc::from(Rc::new(VecModel::from(v)))
}

fn white(a: f32) -> Color {
    Color::from_argb_f32(a.clamp(0.0, 1.0), 1.0, 1.0, 1.0)
}

fn alpha(c: Color, a: f32) -> Color {
    c.with_alpha(a.clamp(0.0, 1.0))
}

fn mix(a: Color, b: Color, t: f32) -> Color {
    let t = t.clamp(0.0, 1.0);
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color::from_argb_u8(l(a.alpha(), b.alpha()), l(a.red(), b.red()), l(a.green(), b.green()), l(a.blue(), b.blue()))
}

const CLEAR: Color = Color::from_argb_u8(0, 0, 0, 0);

fn line(commands: String, stroke: Color, width: f32) -> PanelPath {
    PanelPath { commands: s(commands), stroke, width, fill: CLEAR, fill2: CLEAR }
}

fn area(commands: String, top: Color, bottom: Color) -> PanelPath {
    PanelPath { commands: s(commands), stroke: CLEAR, width: 0.0, fill: top, fill2: bottom }
}

/// Rounded rectangle as a path (fill, optional border stroke).
fn rounded(x: f32, y: f32, w: f32, h: f32, r: f32, fill: Color, border: Color, border_width: f32) -> PanelPath {
    let r = r.min(w / 2.0).min(h / 2.0);
    let (x1, y1) = (x + w, y + h);
    let commands = format!(
        "M{:.2} {y:.2} H{:.2} A{r:.2} {r:.2} 0 0 1 {x1:.2} {:.2} V{:.2} A{r:.2} {r:.2} 0 0 1 {:.2} {y1:.2} H{:.2} A{r:.2} {r:.2} 0 0 1 {x:.2} {:.2} V{:.2} A{r:.2} {r:.2} 0 0 1 {:.2} {y:.2} Z",
        x + r,
        x1 - r,
        y + r,
        y1 - r,
        x1 - r,
        x + r,
        y1 - r,
        y + r,
        x + r
    );
    PanelPath { commands: s(commands), stroke: border, width: border_width, fill, fill2: fill }
}

fn rect(x: f32, y: f32, w: f32, h: f32, radius: f32, fill: Color) -> PanelPath {
    rounded(x, y, w, h, radius, fill, CLEAR, 0.0)
}

/// Canvas of one view: boxes under the layers, the layers, boxes over them, texts.
#[allow(clippy::too_many_arguments)]
fn canvas(
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    well_y: f32,
    caption: &str,
    label: String,
    paths: [Vec<PanelPath>; 3],
    texts: Vec<PanelText>,
) -> PanelCanvas {
    PanelCanvas {
        visible: true,
        well: true,
        x,
        y,
        w,
        h,
        well_y,
        caption: s(caption),
        value: s(label),
        paths: model(paths.into_iter().flatten().collect()),
        texts: model(texts),
        ..Default::default()
    }
}

#[allow(clippy::too_many_arguments)]
fn text(x: f32, y: f32, w: f32, h: f32, t: &str, color: Color, size: f32, weight: i32, align: i32) -> PanelText {
    PanelText { x, y, w, h, text: s(t), color, size, weight, align }
}

/// "M x0 y L x1 y" segments for horizontal rules.
fn hline(out: &mut String, x0: f32, x1: f32, y: f32) {
    out.push_str(&format!("M{x0:.2} {y:.2} L{x1:.2} {y:.2} "));
}

fn vline(out: &mut String, x: f32, y0: f32, y1: f32) {
    out.push_str(&format!("M{x:.2} {y0:.2} L{x:.2} {y1:.2} "));
}

/// Dashed straight line (dash / gap lengths along the line).
fn dashed(x0: f32, y0: f32, x1: f32, y1: f32, on: f32, off: f32) -> String {
    let (dx, dy) = (x1 - x0, y1 - y0);
    let len = (dx * dx + dy * dy).sqrt();
    let mut out = String::new();
    if len <= 0.0 {
        return out;
    }
    let mut t = 0.0;
    while t < len {
        let t1 = (t + on).min(len);
        out.push_str(&format!("M{:.2} {:.2} L{:.2} {:.2} ", x0 + dx * t / len, y0 + dy * t / len, x0 + dx * t1 / len, y0 + dy * t1 / len));
        t += on + off;
    }
    out
}

/// Name of a MIDI note with its octave ("A4", "C#3").
pub fn note_name(midi: i32) -> String {
    format!("{}{}", NOTES[midi.rem_euclid(12) as usize], midi.div_euclid(12) - 1)
}

/// Spectrum bin smoothing: asymmetric one-pole
/// averaging over frames, then a gaussian blur across bins.
pub fn average_bins(avg: &mut Vec<f32>, frame: &[f32], averaging: f32) {
    let rise = 0.8 - 0.25 * averaging;
    let fall = 0.22 - 0.19 * averaging;
    if avg.len() != frame.len() {
        *avg = frame.iter().map(|v| if v.is_finite() { *v } else { FLOOR_DB }).collect();
        return;
    }
    for (a, &x) in avg.iter_mut().zip(frame) {
        let x = if x.is_finite() { x } else { FLOOR_DB };
        *a += (x - *a) * if x > *a { rise } else { fall };
    }
}

pub fn blur_bins(v: &[f32], smoothing: f32) -> Vec<f32> {
    let r = (9.0 * smoothing).round() as i32;
    if r <= 0 {
        return v.to_vec();
    }
    let sigma = r as f32;
    let k: Vec<f32> = (-r..=r).map(|i| (-(i * i) as f32 / (2.0 * sigma * sigma)).exp()).collect();
    let n = v.len() as i32;
    (0..n)
        .map(|i| {
            let (mut acc, mut wsum) = (0.0, 0.0);
            for (j, w) in (-r..=r).zip(&k) {
                let p = i + j;
                if p >= 0 && p < n {
                    acc += v[p as usize] * w;
                    wsum += w;
                }
            }
            acc / wsum
        })
        .collect()
}

/// Sharp sign in a g x g box at (x, y), as stroke paths.
fn sharp_glyph(x: f32, y: f32, g: f32) -> String {
    let mut out = String::new();
    for f in [0.36, 0.64] {
        out.push_str(&format!("M{:.2} {:.2} L{:.2} {:.2} ", x + g * (f + 0.05), y, x + g * (f - 0.05), y + g));
    }
    for f in [0.36, 0.64] {
        out.push_str(&format!("M{:.2} {:.2} L{:.2} {:.2} ", x, y + g * (f + 0.06), x + g, y + g * (f - 0.06)));
    }
    out
}

/// Flat sign in a (0.6 g) x g box at (x, y).
fn flat_glyph(x: f32, y: f32, g: f32) -> String {
    let w = 0.6 * g;
    format!(
        "M{x:.2} {y:.2} L{x:.2} {:.2} C{:.2} {:.2} {:.2} {:.2} {x:.2} {:.2}",
        y + g,
        x + w * 1.1,
        y + g * 0.78,
        x + w * 0.9,
        y + g * 0.35,
        y + g * 0.55
    )
}

impl Controller {
    /// Fold new engine data into the custom views of the selected track's devices.
    pub(crate) fn panel_poll_views(&mut self) {
        self.panel_poll_views_at(Instant::now());
    }

    /// [Self::panel_poll_views] at a given time (review renders step it per offline block).
    pub(crate) fn panel_poll_views_at(&mut self, now: Instant) {
        let Some(t) = self.selected_track.and_then(|t| self.project().track(t)) else { return };
        let ids: Vec<(DeviceId, &'static str)> = t
            .devices
            .iter()
            .filter_map(|d| match dstudio_core::builtin::def_of(d)?.layout.view {
                Some(View::Custom(v)) if metrics(v).is_some() => Some((d.id, v)),
                _ => None,
            })
            .collect();
        let st = &mut self.devices.panel.analyzers;
        st.traces.retain(|id, _| ids.iter().any(|(d, _)| d == id));
        st.tuners.retain(|id, _| ids.iter().any(|(d, _)| d == id));
        st.spectra.retain(|id, _| ids.iter().any(|(d, _)| d == id));
        let mut out = Vec::new();
        let mut dirty = false;
        for (id, view) in ids {
            match view {
                "pitch-trace" => {
                    let r = self
                        .engine
                        .builtin_readouts(id.0, &mut out)
                        .ok()
                        .filter(|_| out.len() >= 4)
                        .map(|_| (out[0], out[1], out[2], out[3]));
                    let frames = self.devices.panel.analyzers.traces.entry(id).or_default();
                    if let Some((v, p, sh, tg)) = r {
                        if frames.len() == TRACE_FRAMES {
                            frames.pop_front();
                        }
                        frames.push_back(TraceFrame { t: now, voiced: v >= 0.5, pitch: p, shift: sh, target: tg });
                    }
                    // Live while any voiced frame is still inside the window.
                    let window = Duration::from_secs_f32(TRACE_WINDOW);
                    dirty |= frames.iter().any(|f| f.voiced && now.duration_since(f.t) <= window);
                }
                "tuner" => {
                    let r = self.engine.builtin_readouts(id.0, &mut out).ok().filter(|_| out.len() >= 6).map(|_| out.clone());
                    let tv = self.devices.panel.analyzers.tuners.entry(id).or_default();
                    if let Some(r) = r {
                        tv.voiced = r[0] >= 0.5;
                        tv.note = r[1].round() as i32;
                        tv.cents = r[2].clamp(-50.0, 50.0);
                        tv.in_tune = r[3] >= 0.5;
                        tv.string = r[4].round() as i32;
                        tv.holding = r[5] >= 0.5;
                    }
                    dirty |= tuner_animate(tv, now);
                }
                _ => {
                    let def = dstudio_core::builtin::find("spectrum");
                    let averaging = def
                        .zip(self.project().device(id))
                        .and_then(|(def, d)| def.param("averaging").map(|p| self.panel_value(d, def, p) / 100.0))
                        .unwrap_or(0.3);
                    let smoothing = def
                        .zip(self.project().device(id))
                        .and_then(|(def, d)| def.param("smoothing").map(|p| self.panel_value(d, def, p) / 100.0))
                        .unwrap_or(0.3);
                    let seq = self.engine.builtin_analysis(id.0, &mut out).unwrap_or(0);
                    let sv = self.devices.panel.analyzers.spectra.entry(id).or_default();
                    if seq != 0 && seq != sv.seq && out.len() >= 2 * BANDS {
                        sv.seq = seq;
                        average_bins(&mut sv.avg, &out[..BANDS], averaging);
                        if out.len() >= 3 * BANDS {
                            average_bins(&mut sv.side, &out[2 * BANDS..3 * BANDS], averaging);
                        } else {
                            sv.side.clear();
                        }
                        let differs = |a: &[f32], b: &[f32]| a.len() != b.len() || a.iter().zip(b).any(|(x, y)| (x - y).abs() > 0.01);
                        let peak = out[BANDS..2 * BANDS].to_vec();
                        let shown = blur_bins(&sv.avg, smoothing);
                        let shown_side = blur_bins(&sv.side, smoothing);
                        dirty |= differs(&shown, &sv.shown) || differs(&shown_side, &sv.shown_side) || differs(&peak, &sv.peak);
                        sv.peak = peak;
                        sv.shown = shown;
                        sv.shown_side = shown_side;
                    }
                }
            }
        }
        if dirty {
            self.ui_dirty = true;
        }
    }

    /// The field of a custom analyser view (None for other views).
    /// Custom body of an analyzer card: its canvas and the dial row spread over the card.
    pub(crate) fn analyzer_body(&self, d: &Device, def: &'static DeviceDef, view: &str, accent: Color) -> CustomBody {
        let Some(m) = metrics(view) else { return CustomBody::default() };
        let width = def.layout.width();
        let w = width - 2.0 * FIELD_X;
        let canvas = match view {
            "pitch-trace" => self.trace_canvas(d, def, w, accent),
            "tuner" => self.tuner_canvas(d, def, w, accent),
            _ => self.spectrum_canvas(d, def, w, accent),
        };
        let ids: Vec<&'static str> = def.layout.clusters.iter().flat_map(|c| c.iter()).filter_map(|c| c.param()).collect();
        let knobs = ids.iter().enumerate().map(|(i, id)| (*id, spread_x(&m, width, i, ids.len()), DIAL_Y)).collect();
        CustomBody { canvas, knobs, cell: m.cell, ..Default::default() }
    }

    fn trace_canvas(&self, d: &Device, def: &'static DeviceDef, w: f32, accent: Color) -> PanelCanvas {
        let (w, h) = (w * Z, (FIELD_BOTTOM - FIELD_Y) * Z);
        let body_y = CAPTION_H * Z;
        let bh = h - body_y;
        let x0 = GUTTER * Z;
        let mid = bh / 2.0;
        let y_of = |ct: f32| mid - ct / TRACE_RANGE_CT * mid;
        let mut layers = Vec::new();
        for (cts, a) in [(&[0.0][..], 0.26), (&[50.0, -50.0][..], 0.10), (&[100.0, -100.0][..], 0.06)] {
            let mut g = String::new();
            for c in cts {
                hline(&mut g, x0, w, y_of(*c).round() + 0.5);
            }
            layers.push(line(g, white(a), 1.0));
        }
        let mixv = def.param("mix").map_or(100.0, |p| self.panel_value(d, def, p)) / 100.0;
        let corr_color = alpha(accent, 0.25 + 0.75 * mixv);
        let mut over = Vec::new();
        if let Some(frames) = self.devices.panel.analyzers.traces.get(&d.id) {
            let now = frames.back().map_or_else(Instant::now, |f| f.t);
            let x_of = |t: Instant| w - now.duration_since(t).as_secs_f32() / TRACE_WINDOW * (w - x0);
            let mut runs: Vec<Vec<(f32, f32, f32)>> = Vec::new();
            let mut last_target = f32::NAN;
            for f in frames.iter().filter(|f| now.duration_since(f.t).as_secs_f32() <= TRACE_WINDOW) {
                if !f.voiced {
                    last_target = f32::NAN;
                    continue;
                }
                if f.target != last_target {
                    runs.push(Vec::new());
                    last_target = f.target;
                }
                let raw = (f.pitch - f.target) * 100.0;
                let fixed = (f.pitch + f.shift - f.target) * 100.0;
                if let Some(r) = runs.last_mut() {
                    r.push((
                        x_of(f.t),
                        y_of(raw.clamp(-TRACE_RANGE_CT * 1.2, TRACE_RANGE_CT * 1.2)),
                        y_of(fixed.clamp(-TRACE_RANGE_CT * 1.2, TRACE_RANGE_CT * 1.2)),
                    ));
                }
            }
            let (mut fill, mut raw_l, mut fix_l) = (String::new(), String::new(), String::new());
            for r in runs.iter().filter(|r| r.len() > 1) {
                let raw: Vec<(f32, f32)> = r.iter().map(|p| (p.0, p.1)).collect();
                let fixed: Vec<(f32, f32)> = r.iter().map(|p| (p.0, p.2)).collect();
                let mut poly = raw.clone();
                poly.extend(fixed.iter().rev());
                fill.push_str(&format!("{} Z ", polyline(&poly)));
                raw_l.push_str(&format!("{} ", polyline(&raw)));
                fix_l.push_str(&format!("{} ", polyline(&fixed)));
            }
            layers.push(area(fill, alpha(accent, 0.09), alpha(accent, 0.09)));
            layers.push(line(raw_l, white(0.55), 1.4 * Z));
            layers.push(line(fix_l, corr_color, 1.6 * Z));
            if let Some(f) = frames.back().filter(|f| f.voiced)
                && let Some(p) = runs.last().and_then(|r| r.last())
            {
                let _ = f;
                over.push(rect(p.0 - 2.0 * Z, p.2 - 2.0 * Z, 4.0 * Z, 4.0 * Z, 2.0 * Z, corr_color));
            }
        }
        canvas(FIELD_X * Z, FIELD_Y * Z, w, h, body_y, "Pitch", String::new(), [Vec::new(), layers, over], Vec::new())
    }

    fn tuner_canvas(&self, d: &Device, def: &'static DeviceDef, w_pt: f32, accent: Color) -> PanelCanvas {
        self.tuner_canvas_at(d, def, w_pt, TUNER_BOTTOM - FIELD_Y, Z, accent)
    }

    /// The tuner picture `w` x `h` units drawn at `z` px per unit (the card's is Z; the
    /// expanded window draws it larger).
    pub(crate) fn tuner_canvas_at(&self, d: &Device, def: &'static DeviceDef, w: f32, h: f32, z: f32, accent: Color) -> PanelCanvas {
        let empty = TunerView::default();
        let tv = self.devices.panel.analyzers.tuners.get(&d.id).unwrap_or(&empty);
        let tuning = def.param("tuning").map_or(0, |p| self.panel_value(d, def, p).round() as usize).min(TUNING_STRINGS.len() - 1);
        let strings = TUNING_STRINGS[tuning];
        let (mut under, mut layers, mut over, mut texts) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
        let voice = tv.voice;
        let dim = 1.0 - 0.55 * tv.hold;
        // String chips.
        let note_top = if strings.is_empty() {
            8.0
        } else {
            let total = strings.len() as f32 * 30.0 - 6.0;
            let x0 = (w - total) / 2.0;
            for (i, &m) in strings.iter().enumerate() {
                let x = x0 + i as f32 * 30.0;
                let current = tv.voiced && tv.string == i as i32;
                under.push(rect(x * z, 8.0 * z, 24.0 * z, 16.0 * z, 8.0 * z, white(0.07)));
                if current {
                    under.push(rect(x * z, 8.0 * z, 24.0 * z, 16.0 * z, 8.0 * z, alpha(accent, voice)));
                }
                let tc = if current { mix(white(0.5), Color::from_rgb_u8(0x17, 0x18, 0x1b), voice) } else { white(0.5) };
                texts.push(text(x * z, 8.0 * z, 24.0 * z, 16.0 * z, &note_name(m as i32), tc, 9.0 * z, 600, 1));
            }
            40.0
        };
        // Tuning bar.
        let bar_h = (0.15 * h).round().clamp(18.0, 28.0);
        let bar_top = h - 32.0 - bar_h;
        let (bar_x, bar_w) = (26.0, w - 52.0);
        let cx = bar_x + bar_w / 2.0;
        under.push(rect(bar_x * z, bar_top * z, bar_w * z, bar_h * z, 6.0 * z, white(0.06)));
        let mut ticks = String::new();
        for c in [-25.0f32, 25.0] {
            vline(
                &mut ticks,
                ((bar_x + (c + 50.0) / 100.0 * bar_w) * z).round() + 0.5,
                (bar_top + 0.3 * bar_h) * z,
                (bar_top + 0.7 * bar_h) * z,
            );
        }
        layers.push(line(ticks, white(0.16), 1.0));
        let tuned = tv.tuned * voice;
        let zone = rounded(
            (cx - 11.0) * z,
            bar_top * z,
            22.0 * z,
            bar_h * z,
            4.0 * z,
            mix(white(0.10), alpha(SUCCESS, 0.4), tuned),
            mix(white(0.22), SUCCESS, tuned),
            1.0,
        );
        under.push(zone);
        let mut needle = String::new();
        vline(&mut needle, cx * z, (bar_top + 5.0) * z, (bar_top + bar_h - 5.0) * z);
        layers.push(line(needle, white(0.4), 2.0 * z));
        // Flat and sharp signs 9 pt outside the bar ends.
        let g = 13.0;
        let gy = bar_top + (bar_h - g) / 2.0;
        layers.push(line(flat_glyph((bar_x - 9.0 - 0.6 * g) * z, gy * z, g * z), white(0.45), 1.6 * z));
        layers.push(line(sharp_glyph((bar_x + bar_w + 9.0) * z, gy * z, g * z), white(0.45), 1.5 * z));
        // Marker.
        if voice > 0.001 {
            let mx = bar_x + (tv.marker.clamp(-50.0, 50.0) + 50.0) / 100.0 * bar_w;
            let col = mix(white(0.92), SUCCESS, tuned);
            over.push(rect(
                (mx - 5.0) * z,
                (bar_top - 4.0) * z,
                10.0 * z,
                (bar_h + 8.0) * z,
                5.0 * z,
                alpha(col, voice * dim * col.alpha() as f32 / 255.0),
            ));
        }
        // Note read-out.
        let space = bar_top - note_top;
        let size = [48.0f32, 36.0, 28.0].into_iter().find(|s| s * 1.15 <= space).unwrap_or(28.0);
        let cy = note_top + space / 2.0;
        if tv.voiced || voice > 0.001 {
            let name = NOTES[tv.note.rem_euclid(12) as usize];
            let letter = &name[..1];
            let sharp = name.len() > 1;
            let octave = (tv.note.div_euclid(12) - 1).to_string();
            let lw = 0.66 * size;
            let gs = 0.36 * size;
            let ow = 7.5 * octave.len() as f32;
            let total = lw + if sharp { gs + 1.0 } else { 0.0 } + 2.0 + ow;
            let x0 = (w - total) / 2.0;
            let col = alpha(mix(white(0.92), SUCCESS, tuned), voice * dim * 0.92_f32.max(tuned));
            texts.push(text(x0 * z, (cy - size * 0.65) * z, lw * z, size * 1.3 * z, letter, col, size * z, 700, 1));
            if sharp {
                layers.push(line(sharp_glyph((x0 + lw + 1.0) * z, (cy - size * 0.36) * z, gs * z), col, (0.09 * size).max(1.5) * z));
            }
            let ox = x0 + lw + if sharp { gs + 1.0 } else { 0.0 } + 2.0;
            texts.push(text(
                ox * z,
                (cy + size * 0.36 - 12.0) * z,
                (ow + 4.0) * z,
                12.0 * z,
                &octave,
                white(0.55 * voice * dim),
                12.0 * z,
                600,
                0,
            ));
        } else {
            over.push(rect((w / 2.0 - 12.0) * z, (cy - 2.5) * z, 24.0 * z, 5.0 * z, 1.0 * z, white(0.25)));
        }
        // Status chevrons: right of centre when flat (play higher), left when sharp.
        if voice > 0.001 && !tv.in_tune && tv.cents.abs() > 0.5 {
            let dir = if tv.cents < 0.0 { 1.0 } else { -1.0 };
            let y = h - 15.0;
            for (k, a) in [(0.0f32, 0.55f32), (1.0, 1.0)] {
                let x = cx + dir * (8.0 + 10.0 * k);
                let tip = x + dir * 6.0;
                layers.push(line(
                    format!("M{:.2} {:.2} L{:.2} {:.2} L{:.2} {:.2}", x * z, (y - 6.0) * z, tip * z, y * z, x * z, (y + 6.0) * z),
                    white(0.85 * a * voice * dim),
                    2.0 * z,
                ));
            }
        }
        canvas(FIELD_X * z, FIELD_Y * z, w * z, h * z, 0.0, "", tv.spoken.clone(), [under, layers, over], texts)
    }

    fn spectrum_canvas(&self, d: &Device, def: &'static DeviceDef, w: f32, _accent: Color) -> PanelCanvas {
        let (w, h) = (w * Z, (FIELD_BOTTOM - FIELD_Y) * Z);
        let body_y = CAPTION_H * Z;
        let bh = h - body_y;
        let x0 = GUTTER * Z;
        let value = |id: &str, dflt: f32| def.param(id).map_or(dflt, |p| self.panel_value(d, def, p));
        let ceiling = value("db_ceiling", 0.0);
        let range = value("db_range", 90.0).max(1.0);
        let slope = value("slope", 4.5);
        let color = PALETTE[(value("color_scheme", 0.0).round() as usize).min(PALETTE.len() - 1)];
        let x_of = |hz: f32| x0 + (hz / 20.0).ln() / 1000f32.ln() * (w - x0);
        let y_of = |db: f32| (ceiling - db) / range * bh;
        let mut layers = Vec::new();
        let (mut strong, mut weak) = (String::new(), String::new());
        vline(&mut strong, x_of(100.0).round() + 0.5, 0.0, bh);
        vline(&mut strong, x_of(1000.0).round() + 0.5, 0.0, bh);
        vline(&mut weak, x_of(10000.0).round() + 0.5, 0.0, bh);
        let step = if range >= 72.0 {
            12.0
        } else if range >= 36.0 {
            6.0
        } else {
            3.0
        };
        let mut db = (ceiling / step).floor() * step;
        while db > ceiling - range {
            let y = y_of(db);
            if y > 2.0 && y < bh - 2.0 {
                hline(if db == 0.0 { &mut strong } else { &mut weak }, x0, w, y.round() + 0.5);
            }
            db -= step;
        }
        layers.push(line(strong, white(0.11), 1.0));
        layers.push(line(weak, white(0.06), 1.0));
        // Slope guide: through the middle at 1 kHz, rising Slope dB per octave.
        let tilt = |hz: f32| bh / 2.0 - slope * (hz / 1000.0).log2() * bh / range;
        layers.push(line(dashed(x_of(20.0), tilt(20.0), w, tilt(20000.0), 3.0 * Z, 5.0 * Z), white(0.22), 1.0));
        if let Some(sv) = self.devices.panel.analyzers.spectra.get(&d.id) {
            let pts = |v: &[f32]| -> Vec<(f32, f32)> {
                v.iter()
                    .enumerate()
                    .map(|(i, db)| (x_of(20.0 * 1000f32.powf((i as f32 + 0.5) / BANDS as f32)), y_of(*db).clamp(-1.0, bh + 1.0)))
                    .collect()
            };
            let audible = |v: &[f32]| v.iter().any(|db| *db > -119.99);
            if audible(&sv.peak) {
                layers.push(line(polyline(&pts(&sv.peak)), alpha(color, 0.35), 1.0));
            }
            if audible(&sv.shown_side) {
                layers.push(line(polyline(&pts(&sv.shown_side)), alpha(color, 0.55), 1.0 * Z));
            }
            if audible(&sv.shown) {
                let p = pts(&sv.shown);
                let curve = polyline(&p);
                let fill = format!("{curve} L{:.2} {bh:.2} L{:.2} {bh:.2} Z", p[p.len() - 1].0, p[0].0);
                layers.push(area(fill, alpha(color, 0.55), alpha(color, 0.04)));
                layers.push(line(curve, alpha(color, 0.95), 1.2 * Z));
            }
        }
        canvas(FIELD_X * Z, FIELD_Y * Z, w, h, body_y, "Spectrum", String::new(), [Vec::new(), layers, Vec::new()], Vec::new())
    }
}

/// Advance the tuner's animations to `now`; true while anything still moves.
fn tuner_animate(tv: &mut TunerView, now: Instant) -> bool {
    let dt = tv.last.map_or(0.0, |t| now.duration_since(t).as_secs_f32()).min(0.25);
    tv.last = Some(now);
    let toward = |v: f32, target: f32, ms: f32| {
        if dt <= 0.0 {
            v
        } else if target > v {
            (v + dt * 1000.0 / ms).min(target)
        } else {
            (v - dt * 1000.0 / ms).max(target)
        }
    };
    let before = (tv.marker, tv.voice, tv.tuned, tv.hold);
    tv.voice = toward(tv.voice, if tv.voiced { 1.0 } else { 0.0 }, 140.0);
    tv.tuned = toward(tv.tuned, if tv.voiced && tv.in_tune { 1.0 } else { 0.0 }, 160.0);
    tv.hold = toward(tv.hold, if tv.holding { 1.0 } else { 0.0 }, 140.0);
    if tv.voiced {
        tv.marker = if before.1 <= 0.0 { tv.cents } else { tv.cents + (tv.marker - tv.cents) * (-dt / 0.11).exp() };
    }
    // Spoken text: "<note>, <direction>", at most one change per 700 ms (the newest wins).
    tv.wanted = if !tv.voiced {
        String::new()
    } else {
        let dir = if tv.in_tune {
            "in tune"
        } else if tv.cents < 0.0 {
            "flat"
        } else {
            "sharp"
        };
        format!("{}, {dir}", note_name(tv.note))
    };
    if tv.wanted != tv.spoken && tv.spoken_at.is_none_or(|t| now.duration_since(t) >= Duration::from_millis(700)) {
        tv.spoken = tv.wanted.clone();
        tv.spoken_at = Some(now);
    }
    let moved = (tv.marker - before.0).abs() > 0.01 || tv.voice != before.1 || tv.tuned != before.2 || tv.hold != before.3;
    moved || tv.voiced
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn knob_rows_spread_like_the_reference() {
        // Pitch Corrector: four 76 pt cells around free space (centres 52.9 .. 322.1).
        let m = metrics("pitch-trace").unwrap();
        let c: Vec<f32> = (0..4)
            .map(|i| spread_x(&m, dstudio_core::builtin::find("pitch-corrector").unwrap().layout.width(), i, 4) + m.cell / 2.0)
            .collect();
        assert!((c[0] - 52.9).abs() < 0.1 && (c[3] - 322.1).abs() < 0.1, "{c:?}");
        // Spectrum: five 66 pt cells from edge to edge (centres 41 .. 356).
        let m = metrics("spectrum").unwrap();
        let c: Vec<f32> =
            (0..5).map(|i| spread_x(&m, dstudio_core::builtin::find("spectrum").unwrap().layout.width(), i, 5) + m.cell / 2.0).collect();
        assert_eq!((c[0], c[4]), (41.0, 356.0));
        assert!((c[2] - 198.5).abs() < 0.01);
    }

    #[test]
    fn spectrum_averaging_is_asymmetric_and_blur_keeps_level() {
        let mut avg = Vec::new();
        average_bins(&mut avg, &[-60.0; 4], 0.3);
        assert_eq!(avg, vec![-60.0; 4], "the first frame is taken as is");
        average_bins(&mut avg, &[0.0, -60.0, -120.0, f32::NAN], 0.0);
        assert!((avg[0] - -12.0).abs() < 1e-4, "rise 0.8 at averaging 0");
        assert!((avg[2] - (-60.0 - 0.22 * 60.0)).abs() < 1e-4, "fall 0.22 at averaging 0");
        assert!((avg[3] - (-60.0 - 0.22 * 60.0)).abs() < 1e-4, "non-finite bins read -120 dB");
        let mut slow = vec![-60.0; 1];
        average_bins(&mut slow, &[0.0], 1.0);
        assert!((slow[0] - (-60.0 + 0.55 * 60.0)).abs() < 1e-4);
        let flat = blur_bins(&[-30.0; 32], 0.5);
        assert!(flat.iter().all(|v| (v + 30.0).abs() < 1e-4));
        let spike: Vec<f32> = (0..32).map(|i| if i == 16 { 0.0 } else { -60.0 }).collect();
        let b = blur_bins(&spike, 0.3);
        assert!(b[16] < 0.0 && b[16] > b[15] && b[15] > -60.0 && (b[0] + 60.0).abs() < 1e-4);
        assert_eq!(blur_bins(&spike, 0.0), spike);
    }

    #[test]
    fn note_names() {
        assert_eq!(note_name(69), "A4");
        assert_eq!(note_name(61), "C#4");
        assert_eq!(note_name(40), "E2");
        assert_eq!(note_name(0), "C-1");
    }

    #[test]
    fn tuner_animation_eases_and_throttles_speech() {
        let t0 = Instant::now();
        let mut tv = TunerView { voiced: true, note: 69, cents: 20.0, ..Default::default() };
        tuner_animate(&mut tv, t0);
        assert_eq!(tv.spoken, "A4, sharp");
        tuner_animate(&mut tv, t0 + Duration::from_millis(70));
        assert!((tv.voice - 0.5).abs() < 1e-3, "140 ms voice fade");
        assert_eq!(tv.marker, 20.0, "the marker starts at the first reading");
        tv.cents = -10.0;
        tuner_animate(&mut tv, t0 + Duration::from_millis(180));
        assert!(tv.marker > -10.0 && tv.marker < 20.0 * (-0.11f32 / 0.11).exp(), "110 ms ease: {}", tv.marker);
        assert_eq!(tv.spoken, "A4, sharp", "a new text waits 700 ms");
        tuner_animate(&mut tv, t0 + Duration::from_millis(720));
        assert_eq!(tv.spoken, "A4, flat");
    }
}
