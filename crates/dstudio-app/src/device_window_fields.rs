//! Expanded windows of the plotted-field family (View::Field: Tremolo, Auto-Panner,
//! Chorus, Flanger, Phaser, Ring Mod): the display picture and the control card.
//!
//! The picture is the card's own curve model (src/device_fields.rs) asked for its
//! window form (`Input::win`): the window's taller ladders, the time or frequency
//! axis under the plot with its units, and a legend that names every series on
//! screen. It is drawn here in plot-local px; the caption sits in the band above
//! the plot and the rulers in the margins around it. Dragging the plot moves the
//! same two parameters as the card's field (across: Rate or Frequency; up: Depth,
//! Feedback or Bias).

use crate::controller::Controller;
use crate::device_canvas::{MUTE_TEXT, dashed, fill, path, rgba, stroke, text, white};
use crate::device_fields::{self, Fill, Mark, Style};
use crate::device_panel::Z;
use crate::device_window::Kit;
use crate::{PanelCanvas, PanelPath, PanelText, PanelZone};
use dstudio_core::Device;
use dstudio_core::builtin::{DeviceDef, Field};
use slint::{Color, ModelRc, VecModel};
use std::rc::Rc;

fn model<T: Clone + 'static>(v: Vec<T>) -> ModelRc<T> {
    ModelRc::from(Rc::new(VecModel::from(v)))
}

/// Inks of the window plots.
const FAINT: f32 = 0.05;
const STRONG: f32 = 0.13;
const INK_LO: f32 = 0.3;
const INK_HI: f32 = 0.92;
const LEGEND_INK: f32 = 0.7;
/// The accent wash under a motion curve, and the band a sweep travels.
const WASH: f32 = 0.28;
const BAND: f32 = 0.16;
/// Series widths: the subject, and a dashed second series.
const W_MAIN: f32 = 1.75;
const W_DASH: f32 = 1.25;
/// Caption band: the baseline 16.73 pt under the display's top edge.
const CAPTION_BASELINE: f32 = 16.73;
const CAPTION_GAP: f32 = 12.0;

/// Window caption of each field device (the card's caption where it has one).
fn caption(def: &DeviceDef, f: &Field) -> &'static str {
    match def.id {
        "autopanner" => "Pan",
        "ring-mod" => "Modulation",
        _ => f.caption,
    }
}

/// Width of 12 pt bold chrome text (the caption's label).
fn label_w(t: &str) -> f32 {
    crate::text_measure::value_width(t) * 1.2
}

impl Controller {
    /// The display of a field window: caption, rulers, axis row, wash, series and legend,
    /// with a drag zone over the plot.
    pub(crate) fn field_window_canvas(&self, d: &Device, def: &'static DeviceDef, f: Field, k: &Kit, accent: Color) -> PanelCanvas {
        let (_, _, dw, dh) = k.display;
        let (l, t, r, b) = k.plot;
        let (pw, ph) = ((dw - l - r) * Z, (dh - t - b) * Z);
        let get = |id: &str| def.param(id).map_or(0.0, |p| self.panel_value(d, def, p));
        let input = device_fields::Input {
            get: &get,
            bpm: self.project().transport.bpm as f32,
            n: (pw.round() as usize).max(64),
            cycles: None,
            win: true,
        };
        let Some(plot) = device_fields::plot(f.plot, &input) else { return PanelCanvas::default() };
        let mut paths: Vec<PanelPath> = Vec::new();
        let mut texts: Vec<PanelText> = Vec::new();
        let at = |x: f32, y: f32| (x * pw, y.clamp(0.0, 1.0) * ph);
        let points = |ys: &[f32]| -> Vec<(f32, f32)> {
            ys.iter().enumerate().map(|(i, y)| at(i as f32 / (ys.len() - 1).max(1) as f32, *y)).collect()
        };

        // Caption: the name and the picture's one value, on the band above the plot.
        let base = (CAPTION_BASELINE - t) * Z;
        let label: String = if plot.axis.caption.is_empty() { caption(def, &f).into() } else { plot.axis.caption.clone() };
        let lx = if def.id == "ring-mod" { 0.0 } else { 2.0 * Z };
        texts.push(PanelText { weight: 700, ..text(lx, base - 4.4 * Z, 240.0 * Z, label.clone(), MUTE_TEXT, 12.0 * Z, 0) });
        let vx = lx + (label_w(&label) + CAPTION_GAP) * Z;
        texts.push(text(vx, base - 4.7 * Z, 260.0 * Z, plot.readout.clone(), white(INK_HI), 13.0 * Z, 0));

        // The ladder: a rule per rung (the rest line stronger), numerals in the left margin.
        let response = plot.axis.start == "Hz";
        let rest = plot.rest;
        let (first, last) = (plot.ticks.first().map(|t| t.0), plot.ticks.last().map(|t| t.0));
        for (y, label) in &plot.ticks {
            let py = (y * ph).round() + 0.5;
            let is_rest = rest.is_some_and(|r| (r - y).abs() < 1e-4);
            // A response ladder leaves its end rungs to the frame's own edges.
            let edge = response && (Some(*y) == first || Some(*y) == last);
            if !edge {
                paths.push(stroke(format!("M0 {py:.1} L{pw:.1} {py:.1}"), white(if is_rest { STRONG } else { FAINT }), 1.0));
            }
            if !label.is_empty() {
                let gap = if response { 6.0 } else { 8.0 };
                texts.push(text(-gap * Z, y * ph, 60.0 * Z, label.clone(), white(INK_LO), 11.0 * Z, 2));
            }
        }
        if response {
            for e in [0.5, ph.round() - 0.5] {
                paths.push(stroke(format!("M0 {e:.1} L{pw:.1} {e:.1}"), white(FAINT), 1.0));
            }
        }

        // The axis row: a rule per tick, its numeral under the plot, the units at its ends.
        let ring = k.plot.3 < 40.0;
        let (num_y, num_size) = if ring {
            (ph + 13.0 * Z, 11.0)
        } else if response {
            (ph + 12.0 * Z, 10.0)
        } else {
            (ph + 15.5 * Z, 11.0)
        };
        let unit_w = crate::text_measure::value_width(&plot.axis.end) * Z;
        for (x, label, strong) in &plot.axis.ticks {
            let px = (x * pw).round() + 0.5;
            paths.push(stroke(format!("M{px:.1} 0 L{px:.1} {ph:.1}"), white(if *strong { STRONG } else { FAINT }), 1.0));
            if label.is_empty() {
                continue;
            }
            // Ring Mod drops a numeral that would run into the unit at the row's end.
            let half = crate::text_measure::value_width(label) * 1.1 * Z / 2.0;
            if ring && px + half > pw - unit_w - 10.0 * Z {
                continue;
            }
            texts.push(text(px, num_y, 80.0 * Z, label.clone(), white(INK_LO), num_size * Z, 1));
        }
        let unit_y = num_y - if response { 0.0 } else { 0.5 * Z };
        if !plot.axis.foot.is_empty() {
            texts.push(text(-8.0 * Z, unit_y, 60.0 * Z, plot.axis.foot.clone(), white(INK_LO), 10.0 * Z, 2));
        }
        if !plot.axis.start.is_empty() {
            texts.push(text(0.0, unit_y, 30.0 * Z, plot.axis.start.clone(), white(INK_LO), 10.0 * Z, 0));
        }
        if !plot.axis.end.is_empty() {
            texts.push(text(pw, unit_y, 30.0 * Z, plot.axis.end.clone(), white(INK_LO), 10.0 * Z, 2));
        }

        // The wash between the subject and the rest line, then the dashed series, then the subject.
        let main = plot.traces.first().map(|t| points(&t.ys)).unwrap_or_default();
        if let (Fill::ToRest, Some(r)) = (plot.fill, rest)
            && main.len() > 1
        {
            let ry = r * ph;
            paths.push(fill(
                format!("{} L{:.1} {ry:.1} L{:.1} {ry:.1} Z", path(&main), main[main.len() - 1].0, main[0].0),
                rgba(accent, WASH),
            ));
        }
        if plot.fill == Fill::Band {
            let ys = plot.traces.iter().flat_map(|t| t.ys.iter().copied());
            let (top, bottom) = ys.fold((f32::MAX, f32::MIN), |(a, b), y| (a.min(y), b.max(y)));
            if top <= bottom {
                let y0 = top.clamp(0.0, 1.0) * ph;
                let y1 = (bottom.clamp(0.0, 1.0) * ph).max(y0 + 1.0);
                paths.push(fill(format!("M0 {y0:.1} L{pw:.1} {y0:.1} L{pw:.1} {y1:.1} L0 {y1:.1} Z"), rgba(accent, BAND)));
            }
        }
        for tr in plot.traces.iter().skip(1) {
            if let Style::Dash { on, off, alpha } = tr.style {
                paths.push(stroke(dashed(&points(&tr.ys), on * Z, off * Z), white(alpha), W_DASH * Z));
            }
        }
        paths.push(stroke(path(&main), white(1.0), W_MAIN * Z));

        // Legend: a glyph and a name per series on screen.
        let (glyph, gap, entry_gap) = (16.0 * Z, 6.0 * Z, 14.0 * Z);
        let widths: Vec<f32> = plot.legend.iter().map(|(_, n)| crate::text_measure::value_width(n) * Z).collect();
        let (mut x, y, ink) = if plot.axis.legend_in_caption {
            let total: f32 = widths.iter().map(|w| glyph + gap + w).sum::<f32>() + entry_gap * widths.len().saturating_sub(1) as f32;
            (pw - total, base - 3.5 * Z, MUTE_TEXT)
        } else {
            (0.0, ph + 31.0 * Z, white(LEGEND_INK))
        };
        for ((mark, name), w) in plot.legend.iter().zip(widths) {
            match mark {
                Mark::Band => {
                    paths.push(fill(
                        format!("M{x:.1} {:.1} h{glyph:.1} v{:.1} h{:.1} Z", y - 4.0 * Z, 8.0 * Z, -glyph),
                        rgba(accent, if plot.fill == Fill::Band { BAND } else { WASH }),
                    ));
                }
                Mark::Line(Style::Solid) => paths.push(stroke(format!("M{x:.1} {y:.1} L{:.1} {y:.1}", x + glyph), white(1.0), W_MAIN * Z)),
                &Mark::Line(Style::Dash { on, off, alpha }) => {
                    paths.push(stroke(dashed(&[(x, y), (x + glyph, y)], on * Z, off * Z), white(alpha), W_DASH * Z));
                }
            }
            texts.push(text(x + glyph + gap, y, w + 10.0 * Z, name.clone(), ink, 10.0 * Z, 0));
            x += glyph + gap + w + entry_gap;
        }

        // Drag the plot: across the card field's horizontal parameter, up its vertical one.
        let axis = |ids: &[&'static str]| Self::field_axis(d, def, ids).map_or(-1, |i| i as i32);
        let zone = PanelZone {
            x: 0.0,
            y: 0.0,
            w: pw,
            h: ph,
            param_x: axis(f.x),
            param_y: axis(f.y),
            rate_x: 1.0 / pw.max(1.0),
            rate_y: 1.0 / ph.max(1.0),
        };
        PanelCanvas {
            visible: true,
            w: pw,
            h: ph + b * Z,
            paths: model(paths),
            texts: model(texts),
            zones: model(vec![zone]),
            label: label.as_str().into(),
            value: plot.readout.into(),
            ..Default::default()
        }
    }
}
