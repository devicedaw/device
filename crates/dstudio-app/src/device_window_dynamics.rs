//! Expanded windows of the level-history family (Compressor, Limiter, Gate, Glue
//! Compressor): the scrolling display and its transfer square.
//!
//! The display stacks two views on one time axis: a gain-reduction lane (0 to
//! -18 dB, its caption reading the reduction now and its peak over the window)
//! and the level plot under it (input, the reduction band down to the level
//! before makeup, output, and the threshold or ceiling as a dashed line with a
//! grip pill at the oldest edge, which drags it). One px/dB serves both views.
//! The history is the window's own (src/device_window.rs polls in, out and
//! gain reduction while the window is open) and spans 1, 4 or 10 seconds.

use crate::controller::Controller;
use crate::device_canvas::{MUTE_TEXT, fill, rgba, stroke, text, white};
use crate::device_panel::Z;
use crate::device_window::Kit;
use crate::{PanelCanvas, PanelGrip, PanelPath, PanelText, PanelZone};
use dstudio_core::Device;
use dstudio_core::builtin::DeviceDef;
use slint::{Color, ModelRc, SharedString, VecModel};
use std::collections::VecDeque;
use std::rc::Rc;

fn model<T: Clone + 'static>(v: Vec<T>) -> ModelRc<T> {
    ModelRc::from(Rc::new(VecModel::from(v)))
}

/// One history sample: (seconds, input dB, output dB, gain reduction dB <= 0).
pub(crate) type Sample = (f64, f32, f32, f32);

/// The longest window (s); older samples are dropped.
pub(crate) const SPAN_MAX: f64 = 10.5;
/// Window lengths offered in the header (s).
pub(crate) const SPANS: [f32; 3] = [1.0, 4.0, 10.0];

/// Display constants (reference points).
const PAD: f32 = 6.0;
const CAP_H: f32 = 19.0;
const LANE_GAP: f32 = 20.0;
const TIME_H: f32 = 38.0;
const GR_FLOOR: f32 = -18.0;
const LANE_R: f32 = 4.0;
/// A sample at the floor (or within this of it) is silence: no ink.
const FLOOR_EPS: f32 = 0.3;

/// The family's per-device picture.
pub(crate) struct DynSpec {
    /// Level plot range (dB) and its ladder step.
    pub top: f32,
    pub bottom: f32,
    pub step: f32,
    /// The dashed line's parameter and its pill name.
    pub line: &'static str,
    pub line_name: &'static str,
    /// The trace before makeup (Compressor's Gain) drawn and listed.
    pub pre_gain: bool,
    /// Detector entry in the legend (dimmed while nothing feeds it).
    pub detector: bool,
    /// Default window (s).
    pub span: f32,
}

pub(crate) fn spec(id: &str) -> Option<DynSpec> {
    let s = |top, bottom, step, line, line_name, pre_gain, detector, span| DynSpec {
        top,
        bottom,
        step,
        line,
        line_name,
        pre_gain,
        detector,
        span,
    };
    Some(match id {
        "compressor" => s(0.0, -36.0, 6.0, "threshold", "Threshold", true, true, 10.0),
        "limiter" => s(12.0, -30.0, 6.0, "ceiling", "Ceiling", false, false, 4.0),
        "gate" => s(12.0, -60.0, 12.0, "threshold", "Threshold", false, false, 4.0),
        "glue-compressor" => s(0.0, -48.0, 6.0, "threshold", "Threshold", false, true, 4.0),
        "character-compressor" => s(0.0, -60.0, 6.0, "threshold", "Threshold", false, true, 4.0),
        _ => return None,
    })
}

/// The lane and plot geometry of a display h pt high (plot-local, reference points):
/// (lane height, plot top, plot height, px per dB).
pub(crate) fn geometry(sp: &DynSpec, display_h: f32) -> (f32, f32, f32, f32) {
    let avail = display_h - PAD * 2.0 - CAP_H - LANE_GAP - TIME_H;
    let per_db = avail / (-GR_FLOOR + (sp.top - sp.bottom));
    let lane = per_db * -GR_FLOOR;
    (lane, lane + LANE_GAP, per_db * (sp.top - sp.bottom), per_db)
}

/// A gain-reduction amount as the caption reads it: "0.0", "−3.2".
fn gr_text(gr: f32) -> String {
    if gr < -0.05 { format!("\u{2212}{:.1}", -gr) } else { "0.0".into() }
}

fn db_rung(db: f32) -> String {
    if db > 0.5 {
        format!("+{db:.0}")
    } else if db < -0.5 {
        format!("\u{2212}{:.0}", -db)
    } else {
        "0".into()
    }
}

/// Polylines through the points whose value clears the floor; a run below it is silence.
fn runs(pts: &[(f32, f32, bool)]) -> String {
    let mut out = String::new();
    let mut open = false;
    for (x, y, audible) in pts {
        if *audible {
            out.push_str(&format!("{}{x:.1} {y:.1} ", if open { "L" } else { "M" }));
            open = true;
        } else {
            open = false;
        }
    }
    out
}

impl Controller {
    /// The display of a level-history window.
    pub(crate) fn dynamics_window_canvas(&self, d: &Device, def: &'static DeviceDef, k: &Kit, accent: Color) -> PanelCanvas {
        let Some(sp) = spec(def.id) else { return PanelCanvas::default() };
        let (_, _, dw, dh) = k.display;
        let (l, _, r, _) = k.plot;
        let pw = (dw - l - r) * Z;
        let (lane_h, plot_t, plot_h, per_db) = geometry(&sp, dh);
        let (lane_h, plot_t, plot_h) = (lane_h * Z, plot_t * Z, plot_h * Z);
        let span = self.win_span(d.id, &sp);
        let held = self.win_held(d.id);
        let hist = self.win_history(d.id);
        let now = self.win_now();
        let mut paths: Vec<PanelPath> = Vec::new();
        let mut texts: Vec<PanelText> = Vec::new();

        let db_y = |db: f32| plot_t + ((sp.top - db) / (sp.top - sp.bottom)).clamp(0.0, 1.0) * plot_h;
        let gr_y = |gr: f32| (gr / GR_FLOOR).clamp(0.0, 1.0) * lane_h;
        let grid = if span <= 1.0 {
            0.25
        } else if span <= 4.0 {
            1.0
        } else {
            2.0
        };
        let at = hist.and_then(|h| h.back().map(|s| s.0)).filter(|_| held).unwrap_or(now);
        let x_of = |t: f64| ((1.0 - (at - t) as f32 / span) * pw).clamp(0.0, pw);
        // Samples inside the window, about one per 2 px.
        let pts: Vec<Sample> = hist
            .map(|h| {
                let from = at - span as f64;
                let mut v: Vec<Sample> = Vec::new();
                let mut last_x = f32::MIN;
                for s in h.iter().filter(|s| s.0 >= from - 0.05) {
                    let x = x_of(s.0);
                    if x - last_x >= 2.0 * Z || v.is_empty() {
                        v.push(*s);
                        last_x = x;
                    } else if let Some(p) = v.last_mut() {
                        // Keep the deepest reduction and the loudest levels of a merged column.
                        *p = (s.0, p.1.max(s.1), p.2.max(s.2), p.3.min(s.3));
                    }
                }
                v
            })
            .unwrap_or_default();
        let gr_now = pts.last().map_or(0.0, |s| s.3);
        let gr_peak = pts.iter().map(|s| s.3).fold(0.0f32, f32::min);

        // Caption: the reduction now, then its peak over the window.
        let base = -(CAP_H - 6.0) * Z;
        let label = "Gain Reduction";
        texts.push(PanelText { weight: 700, ..text(2.0 * Z, base, 120.0 * Z, label, MUTE_TEXT, 12.0 * Z, 0) });
        let vx = 2.0 * Z + (crate::text_measure::value_width(label) * 1.2 + 12.0) * Z;
        let value = format!("{} dB", gr_text(gr_now));
        texts.push(text(vx, base - 0.3 * Z, 80.0 * Z, value.clone(), white(0.92), 13.0 * Z, 0));
        let px = vx + (crate::text_measure::value_width(&value) * 1.3 + 12.0) * Z;
        texts.push(text(px, base, 80.0 * Z, format!("Peak {}", gr_text(gr_peak)), white(0.55), 11.0 * Z, 0));

        // The lane: a light box, time rules, the reduction filled down from its top edge.
        let r = LANE_R * Z;
        let lane_box = format!(
            "M{r:.1} 0 L{:.1} 0 Q{pw:.1} 0 {pw:.1} {r:.1} L{pw:.1} {:.1} Q{pw:.1} {lane_h:.1} {:.1} {lane_h:.1} L{r:.1} {lane_h:.1} Q0 {lane_h:.1} 0 {:.1} L0 {r:.1} Q0 0 {r:.1} 0 Z",
            pw - r,
            lane_h - r,
            pw - r,
            lane_h - r
        );
        paths.push(fill(lane_box.clone(), white(0.035)));
        let mut sec = grid;
        while sec < span - 1e-3 {
            let x = ((1.0 - sec / span) * pw).round() + 0.5;
            paths.push(stroke(format!("M{x:.1} 0 L{x:.1} {lane_h:.1}"), white(0.04), 1.0));
            sec += grid;
        }
        for g in [6.0, 12.0] {
            let y = gr_y(-g).round() + 0.5;
            paths.push(stroke(format!("M0 {y:.1} L{pw:.1} {y:.1}"), white(0.05), 1.0));
        }
        for g in [0.0f32, 6.0, 12.0, 18.0] {
            texts.push(text(-8.0 * Z, gr_y(-g), 40.0 * Z, db_rung(-g), white(0.3), 11.0 * Z, 2));
        }
        if pts.len() > 1 {
            let line: Vec<(f32, f32)> = pts.iter().map(|s| (x_of(s.0), gr_y(s.3))).collect();
            let mut area = format!("M{:.1} 0 ", line[0].0);
            for (x, y) in &line {
                area.push_str(&format!("L{x:.1} {y:.1} "));
            }
            area.push_str(&format!("L{:.1} 0 Z", line[line.len() - 1].0));
            paths.push(fill(area, rgba(accent, 0.55)));
            paths.push(stroke(crate::device_canvas::path(&line), accent, 1.25 * Z));
        }
        paths.push(stroke(lane_box, white(0.06), 1.0));

        // The level plot: time rules, the ladder, then the traces.
        let mut sec = grid;
        while sec < span - 1e-3 {
            let x = ((1.0 - sec / span) * pw).round() + 0.5;
            paths.push(stroke(format!("M{x:.1} {plot_t:.1} L{x:.1} {:.1}", plot_t + plot_h), white(0.05), 1.0));
            sec += grid;
        }
        let mut db = sp.top;
        while db >= sp.bottom - 1e-3 {
            let y = db_y(db).round() + 0.5;
            let strong = db.abs() < 1e-3;
            paths.push(stroke(format!("M0 {y:.1} L{pw:.1} {y:.1}"), white(if strong { 0.13 } else { 0.05 }), 1.0));
            texts.push(text(-8.0 * Z, db_y(db), 40.0 * Z, db_rung(db), white(0.3), 11.0 * Z, 2));
            db -= sp.step;
        }
        texts.push(text(-8.0 * Z, plot_t + plot_h + 9.0 * Z, 40.0 * Z, "dBFS", white(0.3), 9.0 * Z, 2));
        let audible = |db: f32| db > sp.bottom + FLOOR_EPS;
        let trace = |f: &dyn Fn(&Sample) -> f32| -> String {
            runs(&pts.iter().map(|s| (x_of(s.0), db_y(f(s)), audible(f(s)))).collect::<Vec<_>>())
        };
        let pre = |s: &Sample| s.1 + s.3;
        paths.push(stroke(trace(&|s| s.1), white(0.55), 1.0));
        // The band between input and the level before makeup is the reduction.
        if pts.iter().any(|s| s.3 < -0.1 && audible(s.1)) {
            let mut band = String::new();
            for (i, s) in pts.iter().enumerate() {
                band.push_str(&format!("{}{:.1} {:.1} ", if i == 0 { "M" } else { "L" }, x_of(s.0), db_y(s.1)));
            }
            for s in pts.iter().rev() {
                band.push_str(&format!("L{:.1} {:.1} ", x_of(s.0), db_y(pre(s))));
            }
            band.push('Z');
            paths.push(fill(band, rgba(accent, 0.22)));
        }
        paths.push(stroke(trace(&|s| s.1), rgba(accent, 0.85), 1.25 * Z));
        if sp.pre_gain {
            paths.push(stroke(trace(&pre), rgba(accent, 0.75), 1.0));
        }
        let out = trace(&|s| s.2);
        paths.push(stroke(out.clone(), Color::from_argb_u8(140, 0, 0, 0), 3.5 * Z));
        paths.push(stroke(out, white(1.0), 1.75 * Z));

        // Time axis: one label per rule, the window's start at the left, its state at the right.
        let ty = plot_t + plot_h + 11.0 * Z;
        let mut sec = grid;
        while sec < span - 1e-3 {
            let x = (1.0 - sec / span) * pw;
            if pw - x >= 24.0 * Z {
                let label = if sec.fract() > 1e-3 { format!("\u{2212}{sec:.2}s") } else { format!("\u{2212}{sec:.0}s") };
                texts.push(text(x, ty, 60.0 * Z, label, white(0.5), 9.0 * Z, 1));
            }
            sec += grid;
        }
        texts.push(text(0.0, ty, 60.0 * Z, format!("\u{2212}{span:.0}s"), white(0.5), 9.0 * Z, 0));
        let state = if held {
            "held"
        } else if self.project_playing() {
            "now"
        } else {
            "stopped"
        };
        texts.push(text(pw, ty, 60.0 * Z, state, white(0.5), 9.0 * Z, 2));

        // Legend: every series drawn, the detector dimmed while nothing feeds it.
        let ly = plot_t + plot_h + 31.0 * Z;
        let (glyph, gap, entry) = (14.0 * Z, 6.0 * Z, 16.0 * Z);
        let sidechain = d.sidechain.is_some();
        let thresh_ink = Color::from_argb_u8(140, 200, 198, 188);
        let detector_ink = Color::from_argb_u8(115, 200, 198, 188);
        let mut entries: Vec<(&str, u8)> = vec![("Input", 0), ("Output", 1)];
        if sp.pre_gain {
            entries.push(("Pre-Gain", 2));
        }
        entries.push(("Reduction", 3));
        if sp.detector && def.id == "compressor" {
            entries.push(("Detector", 4));
        }
        entries.push((sp.line_name, 5));
        if sp.detector && def.id != "compressor" {
            entries.push(("Detector", 4));
        }
        let mut x = 0.0;
        for (name, kind) in entries {
            let dim = kind == 4 && !sidechain;
            let a = if dim { 0.35 } else { 1.0 };
            let line = format!("M{x:.1} {ly:.1} L{:.1} {ly:.1}", x + glyph);
            match kind {
                0 => paths.push(stroke(line, white(0.55), 1.0)),
                1 => paths.push(stroke(line, white(1.0), 1.75 * Z)),
                2 => paths.push(stroke(line, rgba(accent, 0.75), 1.0)),
                3 => paths
                    .push(fill(format!("M{x:.1} {:.1} h{glyph:.1} v{:.1} h{:.1} Z", ly - 4.0 * Z, 8.0 * Z, -glyph), rgba(accent, 0.32))),
                4 => paths.push(stroke(
                    crate::device_canvas::dashed(&[(x, ly), (x + glyph, ly)], 2.0 * Z, 3.0 * Z),
                    rgba(detector_ink, 0.45 * a),
                    1.0,
                )),
                _ => paths.push(stroke(crate::device_canvas::dashed(&[(x, ly), (x + glyph, ly)], 6.0 * Z, 5.0 * Z), thresh_ink, 1.5 * Z)),
            }
            let w = crate::text_measure::value_width(name) * 0.9 * Z;
            texts.push(text(x + glyph + gap, ly, w + 10.0 * Z, name, white(0.7 * a), 9.0 * Z, 0));
            x += glyph + gap + w + entry;
        }

        // Now: one marker down both views at the right edge.
        let nx = pw - 0.5;
        paths.push(stroke(
            format!("M{:.1} 0 Q{nx:.1} 0 {nx:.1} {r:.1} L{nx:.1} {:.1}", nx - r, plot_t + plot_h),
            rgba(accent, 0.6),
            1.5 * Z,
        ));
        if held {
            paths.push(stroke(
                format!("M0.5 {:.1} h{:.1} v{:.1} h{:.1} Z", plot_t + 0.5, pw - 1.0, plot_h - 1.0, -(pw - 1.0)),
                rgba(accent, 0.8),
                1.0,
            ));
            texts.push(text(pw - 8.0 * Z - 60.0 * Z, plot_t + 12.0 * Z, 60.0 * Z, "Held", accent, 9.0 * Z, 2));
        }

        // The dashed line and its grip pill at the oldest edge; dragging the band moves it.
        let p = def.param(sp.line);
        let v = p.map_or(0.0, |p| self.panel_value(d, def, p));
        let ly = db_y(v.clamp(sp.bottom, sp.top)).round() + 0.5;
        paths.push(stroke(crate::device_canvas::dashed(&[(0.0, ly), (pw, ly)], 6.0 * Z, 5.0 * Z), thresh_ink, 1.5 * Z));
        let value: SharedString = p.map_or_else(String::new, |p| def.text(d, p, v)).into();
        let grips = vec![PanelGrip { x: 6.0 * Z, y: ly, anchor: 0, label: sp.line_name.into(), value, hot: false, plain: false }];
        let index = def.index_of(sp.line).map_or(-1, |i| i as i32);
        let range = p.map_or(1.0, |p| (p.max - p.min).max(1e-3));
        let zone = PanelZone {
            x: 0.0,
            y: ly - 8.0 * Z,
            w: pw,
            h: 16.0 * Z,
            param_x: -1,
            param_y: index,
            rate_x: 0.0,
            rate_y: 1.0 / (per_db * Z * range),
        };
        PanelCanvas {
            visible: true,
            w: pw,
            h: plot_t + plot_h + TIME_H * Z,
            paths: model(paths),
            texts: model(texts),
            grips: model(grips),
            zones: model(vec![zone]),
            label: label.into(),
            value: value_of(&pts).into(),
            ..Default::default()
        }
    }

    /// The transfer square (78 pt): the curve over -48..0 dB in and out, the wedge it cuts
    /// from the unity line, the knee lit, the threshold dotted and the live input as a dot.
    pub(crate) fn dynamics_square(&self, d: &Device, def: &'static DeviceDef, accent: Color) -> PanelCanvas {
        const RANGE: f32 = 48.0;
        let size = 78.0 * Z;
        let (m, s) = (2.0 * Z, 74.0 * Z);
        let in_x = |db: f32| m + (1.0 - db / -RANGE) * s;
        let out_y = |db: f32| m + (db / -RANGE).clamp(0.0, 1.0) * s;
        let num = |id: &str| def.param(id).map_or(0.0, |p| self.panel_value(d, def, p));
        let face = def.layout.faces.first().map(|f| f.1);
        let Some((law, at, knee)) = face.and_then(|f| crate::device_panel::face_law(f, num)).or_else(|| behavior_law(def, d)) else {
            return PanelCanvas::default();
        };
        let mut paths = Vec::new();
        for i in 1..4 {
            let q = m + i as f32 / 4.0 * s;
            paths.push(stroke(format!("M{m:.1} {q:.1} L{:.1} {q:.1} M{q:.1} {m:.1} L{q:.1} {:.1}", m + s, m + s), white(0.05), 1.0));
        }
        let tx = in_x(at.max(-RANGE));
        paths.push(stroke(crate::device_canvas::dashed(&[(tx, m), (tx, m + s)], 1.0 * Z, 3.0 * Z), white(0.2), 1.0));
        let curve: Vec<(f32, f32)> =
            (0..=100).map(|i| -RANGE + i as f32 / 100.0 * RANGE).map(|x| (in_x(x), out_y(law(x).max(-RANGE)))).collect();
        let mut wedge = crate::device_canvas::path(&curve);
        wedge.push_str(&format!(" L{:.1} {m:.1} L{m:.1} {:.1} Z", m + s, m + s));
        paths.push(fill(wedge, rgba(accent, 0.28)));
        paths.push(stroke(format!("M{m:.1} {:.1} L{:.1} {m:.1}", m + s, m + s), rgba(accent, 0.7), 1.0));
        paths.push(stroke(crate::device_canvas::path(&curve), white(0.92), 1.75 * Z));
        if knee.1 - knee.0 > 0.5 {
            let bend: Vec<(f32, f32)> = (0..=24)
                .map(|i| (knee.0 + (knee.1 - knee.0) * i as f32 / 24.0).max(-RANGE))
                .map(|x| (in_x(x), out_y(law(x).max(-RANGE))))
                .collect();
            paths.push(stroke(crate::device_canvas::path(&bend), rgba(accent, 0.95), 2.5 * Z));
        }
        let live_in = self.win_history(d.id).and_then(|h| h.back()).map_or(-RANGE, |s| s.1).clamp(-RANGE, 0.0);
        let (dx, dy) =
            ((in_x(live_in)).clamp(m + 4.0 * Z, m + s - 4.0 * Z), out_y(law(live_in).max(-RANGE)).clamp(m + 4.0 * Z, m + s - 4.0 * Z));
        let rr = 4.0 * Z;
        let dot = format!("M{:.2} {dy:.2} a{rr:.2} {rr:.2} 0 1 0 {:.2} 0 a{rr:.2} {rr:.2} 0 1 0 {:.2} 0 Z", dx - rr, 2.0 * rr, -2.0 * rr);
        paths.push(PanelPath { width: 1.5 * Z, stroke: Color::from_rgb_u8(0x16, 0x18, 0x1a), ..fill(dot, white(1.0)) });
        PanelCanvas { visible: true, w: size, h: size, paths: model(paths), ..Default::default() }
    }
}

/// The static curve a device's behaviour computes (Character Compressor's models), as a law
/// over all input levels: unity below the computed span, its last slope above it.
#[allow(clippy::type_complexity)]
fn behavior_law(def: &DeviceDef, d: &Device) -> Option<(Box<dyn Fn(f32) -> f32>, f32, (f32, f32))> {
    let t = dstudio_core::builtin::behavior(def).and_then(|b| (b.transfer)(def, d))?;
    let pts = t.points;
    if pts.len() < 2 {
        return None;
    }
    let at = def.param("threshold").map_or(t.bend.0, |p| def.value(d, p));
    let law = move |x: f32| {
        let (first, last) = (pts[0], pts[pts.len() - 1]);
        if x <= first.0 {
            return x + (first.1 - first.0);
        }
        if x >= last.0 {
            let prev = pts[pts.len() - 2];
            let slope = (last.1 - prev.1) / (last.0 - prev.0).max(1e-6);
            return last.1 + slope * (x - last.0);
        }
        let i = pts.partition_point(|p| p.0 < x).clamp(1, pts.len() - 1);
        let (a, b) = (pts[i - 1], pts[i]);
        a.1 + (b.1 - a.1) * (x - a.0) / (b.0 - a.0).max(1e-6)
    };
    Some((Box::new(law), at, t.bend))
}

/// The accessible value of the display: the reduction now.
fn value_of(pts: &[Sample]) -> String {
    format!("{} dB", gr_text(pts.last().map_or(0.0, |s| s.3)))
}

/// Push a sample, dropping those older than the longest window. Returns whether the
/// picture changed (a silent history that stays silent does not).
pub(crate) fn push(h: &mut VecDeque<Sample>, s: Sample, floor: f32) -> bool {
    let silent = |s: &Sample| s.1 <= floor && s.2 <= floor && s.3 > -0.05;
    while h.front().is_some_and(|f| s.0 - f.0 > SPAN_MAX) {
        h.pop_front();
    }
    let quiet = silent(&s) && h.iter().all(silent);
    h.push_back(s);
    !quiet
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_px_per_db_serves_lane_and_plot() {
        let sp = spec("compressor").unwrap();
        let (lane, plot_t, plot_h, per_db) = geometry(&sp, 394.0);
        assert!((lane - 101.7).abs() < 0.2, "{lane}");
        assert!((plot_h - 203.3).abs() < 0.2, "{plot_h}");
        assert!((plot_t - lane - 20.0).abs() < 1e-3);
        assert!((lane / 18.0 - per_db).abs() < 1e-4 && (plot_h / 36.0 - per_db).abs() < 1e-4);
        assert!(spec("gate").is_some_and(|s| s.bottom == -60.0 && s.step == 12.0));
        assert!(spec("chorus").is_none());
    }

    #[test]
    fn history_keeps_ten_seconds_and_rests_in_silence() {
        let mut h = VecDeque::new();
        assert!(!push(&mut h, (0.0, -120.0, -120.0, 0.0), -36.0), "silence draws nothing new");
        assert!(push(&mut h, (0.1, -12.0, -14.0, -3.0), -36.0));
        for i in 0..200 {
            push(&mut h, (0.2 + i as f64 * 0.1, -12.0, -14.0, -3.0), -36.0);
        }
        assert!(h.back().unwrap().0 - h.front().unwrap().0 <= SPAN_MAX + 1e-9);
        assert_eq!(gr_text(-3.04), "\u{2212}3.0");
        assert_eq!(gr_text(-0.01), "0.0");
        assert_eq!(runs(&[(0.0, 1.0, true), (1.0, 1.0, true), (2.0, 5.0, false), (3.0, 1.0, true)]), "M0.0 1.0 L1.0 1.0 M3.0 1.0 ");
    }
}
