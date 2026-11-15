//! Stereo Widener's expanded window display: the stereo field (effective width against
//! frequency, 20 Hz..2 kHz on a 0..400 % ladder, lit between the curve and 100 %), and
//! under it a correlation lane with the input and output correlation over the last
//! seconds (+1 in phase, 0 uncorrelated, −1 cancels in mono). A vertical drag on the
//! field sets Width.

use crate::PanelCanvas;
use crate::controller::Controller;
use crate::device_canvas::{fill, path, rgba, stroke, text, white};
use crate::device_panel::Z;
use crate::device_window::Kit;
use crate::{PanelPath, PanelText, PanelZone};
use dstudio_core::builtin::{self, DeviceDef};
use dstudio_core::{Device, DeviceId};
use slint::{Color, ModelRc, VecModel};
use std::collections::VecDeque;
use std::rc::Rc;

fn model<T: Clone + 'static>(v: Vec<T>) -> ModelRc<T> {
    ModelRc::from(Rc::new(VecModel::from(v)))
}

/// The field's frequency axis (Hz) and width ladder (%).
const F_LO: f32 = 20.0;
const F_HI: f32 = 2000.0;
const PCT_TOP: f32 = 400.0;
const HZ_TICKS: [(f32, &str); 5] = [(50.0, "50"), (100.0, "100"), (200.0, "200"), (500.0, "500"), (1000.0, "1k")];
/// Field height and the lane under it (pt, from the plot top).
const FIELD_H: f32 = 300.0;
const LANE_Y: f32 = 359.0;
const LANE_H: f32 = 120.0;
/// Seconds the correlation lane shows, one grid division each.
pub(crate) const CORR_SPAN: f64 = 6.0;
const FILL: f32 = 0.22;
/// Readings further apart than this (s) are not joined.
const GAP_S: f64 = 0.25;
/// A correlation reading of a silent side.
const SILENT: f32 = 1.5;

/// One correlation reading: (time s, input, output); above 1 where that side was silent.
pub(crate) type CorrSample = (f64, f32, f32);

/// Appends a reading and drops those older than the lane shows.
pub(crate) fn push_corr(h: &mut VecDeque<CorrSample>, s: CorrSample) {
    h.push_back(s);
    while h.front().is_some_and(|f| f.0 < s.0 - CORR_SPAN - 0.5) {
        h.pop_front();
    }
}

/// Polyline commands of one series, broken where it was silent or readings stopped.
fn series(
    h: &VecDeque<CorrSample>,
    now: f64,
    pick: fn(&CorrSample) -> f32,
    x_of: impl Fn(f64) -> f32,
    y_of: impl Fn(f32) -> f32,
) -> String {
    let mut out = String::new();
    let mut open = false;
    let mut last = f64::NEG_INFINITY;
    for s in h.iter().filter(|s| s.0 >= now - CORR_SPAN) {
        let c = pick(s);
        if c > SILENT {
            open = false;
            continue;
        }
        if s.0 - last > GAP_S {
            open = false;
        }
        last = s.0;
        let (x, y) = (x_of(s.0), y_of(c));
        out.push_str(&format!("{}{x:.1} {y:.1} ", if open { "L" } else { "M" }));
        open = true;
    }
    out
}

impl Controller {
    /// The correlation history the Widener window draws.
    pub(crate) fn win_corr(&self, id: DeviceId) -> Option<&VecDeque<CorrSample>> {
        self.devices.wins.corr.get(&id)
    }

    pub(crate) fn widener_window_canvas(&self, d: &Device, def: &'static DeviceDef, k: &Kit, accent: Color) -> PanelCanvas {
        let v = |id: &str| def.param(id).map_or(0.0, |p| self.panel_value(d, def, p));
        let (_, _, dw, dh) = k.display;
        let (pl, pt, pr, pb) = k.plot;
        let (pw, ph) = ((dw - pl - pr) * Z, (dh - pt - pb) * Z);
        let (width, mid_db, side_db, low, mix) = (v("width2"), v("midgain"), v("sidegain"), v("lowcut"), v("mix"));
        let resp = |f: f32| builtin::widener_response(f, width, mid_db, side_db, low, mix, 48_000.0);
        let fh = FIELD_H * Z;
        let x_of = |f: f32| (f.clamp(F_LO, F_HI) / F_LO).ln() / (F_HI / F_LO).ln() * pw;
        let y_of = |pct: f32| fh * (1.0 - (pct / PCT_TOP).clamp(0.0, 1.0));
        let dim = white(0.32);
        let mut paths: Vec<PanelPath> = Vec::new();
        let mut texts: Vec<PanelText> = Vec::new();

        // The field: grid, ladder, axis.
        let mut grid = String::new();
        for (f, label) in HZ_TICKS {
            let x = x_of(f).round() + 0.5;
            grid.push_str(&format!("M{x:.1} 0 V{fh:.1} "));
            texts.push(text(x, fh + 10.0 * Z, 40.0 * Z, label, dim, 10.0 * Z, 1));
        }
        for pct in [100.0, 200.0, 300.0] {
            let y = y_of(pct).round() + 0.5;
            grid.push_str(&format!("M0 {y:.1} H{pw:.1} "));
            texts.push(text(-8.0 * Z, y_of(pct), 36.0 * Z, format!("{pct:.0}"), dim, 10.0 * Z, 2));
        }
        grid.push_str(&format!("M0 {fh:.1} H{pw:.1} M{pw:.1} 0 V{fh:.1}"));
        paths.push(stroke(grid, white(0.05), 1.0));
        texts.push(text(-8.0 * Z, fh + 10.0 * Z, 36.0 * Z, "%", dim, 10.0 * Z, 2));
        texts.push(text(pw, fh + 10.0 * Z, 40.0 * Z, "Hz", dim, 10.0 * Z, 2));

        // The width curve, lit against 100 %.
        let n = ((pw / Z) / 4.0).round().max(48.0) as usize;
        let curve: Vec<(f32, f32)> = (0..=n)
            .map(|i| {
                let f = F_LO * (F_HI / F_LO).powf(i as f32 / n as f32);
                (x_of(f), y_of(resp(f)))
            })
            .collect();
        let line = path(&curve);
        let unity = y_of(100.0);
        paths.push(fill(format!("{line} L{pw:.1} {unity:.1} L0 {unity:.1} Z"), rgba(accent, FILL)));
        paths.push(stroke(line, accent, 1.25 * Z));

        // Caption above the field.
        let cy = -(pt - 12.0) * Z;
        let cx = -(pl - 14.0) * Z;
        texts.push(text(cx, cy, 120.0 * Z, "Stereo Field", white(0.55), 12.0 * Z, 0));
        let at = cx + (crate::text_measure::value_width("Stereo Field") + 14.0) * Z;
        texts.push(text(at, cy, 120.0 * Z, format!("{:.0}%", resp(F_HI)), white(0.92), 13.0 * Z, 0));

        // The correlation lane: caption and legend, grid, the two series.
        let (ly, lh) = (LANE_Y * Z, LANE_H * Z);
        let ky = ly - 13.0 * Z;
        texts.push(text(cx, ky, 120.0 * Z, "Correlation", white(0.55), 12.0 * Z, 0));
        let mut x = cx + (crate::text_measure::value_width("Correlation") + 14.0) * Z;
        let glyph = 14.0 * Z;
        for (label, ink) in [("In", white(0.5)), ("Out", accent)] {
            paths.push(stroke(format!("M{x:.1} {ky:.1} h{glyph:.1}"), ink, 1.25 * Z));
            texts.push(text(x + glyph + 6.0 * Z, ky, 60.0 * Z, label, white(0.7), 10.0 * Z, 0));
            x += glyph + 6.0 * Z + (crate::text_measure::value_width(label) + 16.0) * Z;
        }
        x += 4.0 * Z;
        for label in ["+1 in phase", "0 uncorrelated", "\u{2212}1 cancels in mono"] {
            texts.push(text(x, ky, 160.0 * Z, label, white(0.3), 10.0 * Z, 0));
            x += (crate::text_measure::value_width(label) + 14.0) * Z;
        }
        let mut lane = String::new();
        for i in 0..=4 {
            let y = (ly + lh * i as f32 / 4.0).round() + 0.5;
            lane.push_str(&format!("M0 {y:.1} H{pw:.1} "));
        }
        for i in 1..6 {
            let x = (pw * i as f32 / 6.0).round() + 0.5;
            lane.push_str(&format!("M{x:.1} {ly:.1} V{:.1} ", ly + lh));
        }
        paths.push(stroke(lane, white(0.05), 1.0));
        let mid = (ly + lh / 2.0).round() + 0.5;
        paths.push(stroke(format!("M0 {mid:.1} H{pw:.1}"), white(0.12), 1.0));
        if let Some(h) = self.win_corr(d.id) {
            let now = h.back().map_or(0.0, |s| s.0);
            let tx = |t: f64| pw - ((now - t) / CORR_SPAN) as f32 * pw;
            let cy = |c: f32| ly + lh * (1.0 - c.clamp(-1.0, 1.0)) / 2.0;
            let input = series(h, now, |s| s.1, tx, cy);
            let output = series(h, now, |s| s.2, tx, cy);
            if !input.is_empty() {
                paths.push(stroke(input, white(0.5), 1.25 * Z));
            }
            if !output.is_empty() {
                paths.push(stroke(output, accent, 1.5 * Z));
            }
        }

        // A vertical drag on the field sets Width.
        let zones = vec![PanelZone {
            x: 0.0,
            y: 0.0,
            w: pw,
            h: fh,
            param_x: -1,
            param_y: def.index_of("width2").map_or(-1, |i| i as i32),
            rate_x: 0.0,
            rate_y: 1.0 / fh,
        }];
        PanelCanvas {
            visible: true,
            w: pw,
            h: ph,
            paths: model(paths),
            texts: model(texts),
            zones: model(zones),
            label: "Stereo Field".into(),
            ..Default::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn correlation_history_keeps_the_lane_span_and_breaks_at_silence() {
        let mut h = VecDeque::new();
        for i in 0..800 {
            let t = i as f64 / 60.0;
            push_corr(&mut h, (t, if (300..360).contains(&i) { 2.0 } else { 0.5 }, 1.0));
        }
        assert!(h.front().unwrap().0 >= h.back().unwrap().0 - CORR_SPAN - 0.5);
        let now = h.back().unwrap().0;
        let s = series(&h, now, |s| s.1, |t| t as f32, |c| c);
        assert_eq!(s.matches('M').count(), 1, "no silence in view: one run");
        let mut h2 = VecDeque::new();
        for i in 0..300 {
            push_corr(&mut h2, (i as f64 / 60.0, if (100..160).contains(&i) { 2.0 } else { 0.0 }, 0.0));
        }
        let s2 = series(&h2, h2.back().unwrap().0, |s| s.1, |t| t as f32, |c| c);
        assert_eq!(s2.matches('M').count(), 2, "silence splits the run");
    }
}
