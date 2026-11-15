//! Multiband Compressor's expanded window display: the three bands on a log frequency
//! axis (20 Hz..20 kHz) against a 0..−80 dBFS ladder. Each band shows the region its
//! thresholds act on, its detector level (a wash under a hairline) and its gain hanging
//! off that level, with its Threshold (dashed) and Up Threshold (dotted) lines and the two
//! crossovers as drag handles with value pills. The selected band (the rack card's Band
//! picker) is lit.

use crate::PanelCanvas;
use crate::controller::Controller;
use crate::device_canvas::{dashed, fill, rgba, stroke, text, white};
use crate::device_panel::Z;
use crate::device_window::Kit;
use crate::{PanelGrip, PanelPath, PanelText, PanelZone};
use dstudio_core::Device;
use dstudio_core::builtin::{DeviceDef, MULTIBAND_BANDS};
use slint::{Color, ModelRc, VecModel};
use std::rc::Rc;

fn model<T: Clone + 'static>(v: Vec<T>) -> ModelRc<T> {
    ModelRc::from(Rc::new(VecModel::from(v)))
}

const F_LO: f32 = 20.0;
const F_HI: f32 = 20000.0;
const DB_TOP: f32 = 0.0;
const DB_BOTTOM: f32 = -80.0;
const DB_RUNG: f32 = 10.0;
const HZ_TICKS: [f32; 8] = [50.0, 100.0, 200.0, 500.0, 1000.0, 2000.0, 5000.0, 10000.0];
/// Inks: the selected band, a threshold's region, the level wash (top, floor), the gain.
const SELECT_FILL: f32 = 0.03;
const ACT_FILL: f32 = 0.035;
const LEVEL_TOP: f32 = 0.09;
const LEVEL_FLOOR: f32 = 0.015;
const GAIN_FILL: f32 = 0.30;
const GAIN_LINE: f32 = 0.85;
const PILL_INSET: f32 = 6.0;

/// A band's level as drawn: "−60.0 dB".
fn db_text(db: f32) -> String {
    if db < -0.05 { format!("\u{2212}{:.1} dB", -db) } else { format!("{:.1} dB", db.max(0.0)) }
}

/// Hertz on the axis: "50", "1k", "10k".
fn hz_label(hz: f32) -> String {
    if hz >= 1000.0 { format!("{}k", hz / 1000.0) } else { format!("{hz}") }
}

impl Controller {
    /// The band currently selected on the Multiband card and window.
    pub(crate) fn multiband_band(&self, id: dstudio_core::DeviceId) -> usize {
        self.devices.panel.views.get(&id).map_or(0, |v| v.band).min(2)
    }

    pub(crate) fn multiband_window_canvas(&self, d: &Device, def: &'static DeviceDef, k: &Kit, accent: Color) -> PanelCanvas {
        let (_, _, dw, dh) = k.display;
        let (l, t, r, b) = k.plot;
        let (pw, ph) = ((dw - l - r) * Z, (dh - t - b) * Z);
        let val = |id: &str| def.param(id).map_or(0.0, |p| self.panel_value(d, def, p));
        let idx = |id: &str| def.index_of(id).map_or(-1, |i| i as i32);
        let x_of = |hz: f32| (hz.clamp(F_LO, F_HI) / F_LO).ln() / (F_HI / F_LO).ln() * pw;
        let y_of = |db: f32| ((DB_TOP - db) / (DB_TOP - DB_BOTTOM)).clamp(0.0, 1.0) * ph;
        let (f1, f2) = (val("low_crossover"), val("high_crossover"));
        let edges = [0.0, x_of(f1), x_of(f2).max(x_of(f1)), pw];
        let sel = self.multiband_band(d.id);
        let (levels, gains) = self.devices.panel.views.get(&d.id).map_or(([-100.0; 3], [0.0; 3]), |v| v.meters());
        let (down, up, depth) = (val("downward"), val("upward"), val("depth"));
        let handle = Color::from_argb_u8(140, 200, 198, 188);
        let mut paths: Vec<PanelPath> = Vec::new();
        let mut texts: Vec<PanelText> = Vec::new();
        let rect = |x0: f32, y0: f32, x1: f32, y1: f32| format!("M{x0:.1} {y0:.1} H{x1:.1} V{y1:.1} H{x0:.1} Z");

        paths.push(fill(rect(edges[sel], 0.0, edges[sel + 1], ph), white(SELECT_FILL)));
        for hz in HZ_TICKS {
            let x = x_of(hz).round() + 0.5;
            paths.push(stroke(format!("M{x:.1} 0 V{ph:.1}"), white(0.04), 1.0));
        }
        let mut db = DB_TOP;
        while db >= DB_BOTTOM - 1e-3 {
            let y = y_of(db).round() + 0.5;
            if db != DB_TOP && db != DB_BOTTOM {
                paths.push(stroke(format!("M0 {y:.1} H{pw:.1}"), white(0.05), 1.0));
            }
            let label = if db == 0.0 { "0".to_string() } else { format!("\u{2212}{}", -db as i32) };
            texts.push(text(-8.0 * Z, y_of(db), 40.0 * Z, label, white(0.3), 11.0 * Z, 2));
            db -= DB_RUNG;
        }

        // The regions each band acts on, then its level and gain.
        for band in 0..3 {
            let ids = MULTIBAND_BANDS[band];
            let (x0, x1) = (edges[band], edges[band + 1]);
            if x1 - x0 < 1.0 {
                continue;
            }
            let down_acts = depth > 0.0 && down > 0.0 && val(ids[1]) > 1.0;
            let up_acts = depth > 0.0 && up > 0.0 && val(ids[7]) > 1.0;
            if down_acts {
                paths.push(fill(rect(x0, 0.0, x1, y_of(val(ids[0]))), white(ACT_FILL)));
            }
            if up_acts {
                paths.push(fill(rect(x0, y_of(val(ids[6])), x1, ph), white(ACT_FILL)));
            }
            let level = levels[band];
            let ly = y_of(level);
            paths.push(PanelPath { fill2: white(LEVEL_FLOOR), ..fill(rect(x0, ly, x1, ph), white(LEVEL_TOP)) });
            let gy = y_of(level + gains[band]);
            if (gy - ly).abs() > 0.5 {
                paths.push(fill(rect(x0, ly.min(gy), x1, ly.max(gy)), rgba(accent, GAIN_FILL)));
                paths.push(stroke(format!("M{x0:.1} {gy:.1} H{x1:.1}"), rgba(accent, GAIN_LINE), 1.25 * Z));
            }
            let ink = if level > DB_BOTTOM + 0.5 { 0.55 } else { 0.12 };
            paths.push(stroke(format!("M{x0:.1} {ly:.1} H{x1:.1}"), white(ink), 1.5 * Z));
        }

        // The frequency axis, the ruler's unit, the transport word and the legend.
        let axis_y = ph + 12.0 * Z;
        for hz in HZ_TICKS {
            texts.push(text(x_of(hz), axis_y, 40.0 * Z, hz_label(hz), white(0.3), 9.0 * Z, 1));
        }
        texts.push(text(0.0, axis_y, 30.0 * Z, "Hz", white(0.3), 9.0 * Z, 0));
        texts.push(text(-8.0 * Z, axis_y, 40.0 * Z, "dBFS", white(0.3), 9.0 * Z, 2));
        if !self.project_playing() {
            texts.push(text(pw, axis_y, 60.0 * Z, "stopped", white(0.55), 9.0 * Z, 2));
        }
        let ly = ph + 31.0 * Z;
        let (glyph, gap, entry) = (14.0 * Z, 6.0 * Z, 16.0 * Z);
        let mut x = 0.0;
        for (kind, label) in [(0, "Level"), (1, "Gain"), (2, "Threshold"), (3, "Up Threshold")] {
            match kind {
                0 => paths.push(stroke(format!("M{x:.1} {ly:.1} h{glyph:.1}"), white(0.55), 1.5 * Z)),
                1 => paths.push(fill(rect(x, ly - 4.0 * Z, x + glyph, ly + 4.0 * Z), rgba(accent, GAIN_FILL + 0.02))),
                2 => paths.push(stroke(dashed(&[(x, ly), (x + glyph, ly)], 6.0 * Z, 5.0 * Z), handle, 1.5 * Z)),
                _ => paths.push(stroke(dashed(&[(x, ly), (x + glyph, ly)], 2.0 * Z, 3.0 * Z), handle, 1.5 * Z)),
            }
            texts.push(text(x + glyph + gap, ly, 90.0 * Z, label, white(0.7), 9.0 * Z, 0));
            x += glyph + gap + crate::text_measure::value_width(label) * 0.9 * Z + entry;
        }

        // Handles: each band's two thresholds across its own slice, then the crossovers.
        let mut grips = Vec::new();
        let mut zones = Vec::new();
        let pill_w = |v: &str| (16.0 + crate::text_measure::value_width(v) * 1.1) * Z;
        let per_db = ph / (DB_TOP - DB_BOTTOM);
        for band in 0..3 {
            let ids = MULTIBAND_BANDS[band];
            let (x0, x1) = (edges[band], edges[band + 1]);
            if x1 - x0 < 1.0 {
                continue;
            }
            for (id, up_line) in [(ids[0], false), (ids[6], true)] {
                let v = val(id);
                let y = y_of(v).round() + 0.5;
                let pattern = if up_line { (2.0, 3.0) } else { (6.0, 5.0) };
                paths.push(stroke(dashed(&[(x0, y), (x1, y)], pattern.0 * Z, pattern.1 * Z), handle, 1.5 * Z));
                let value = db_text(v);
                if x1 - x0 >= pill_w(&value) + 2.0 * PILL_INSET * Z {
                    let (gx, anchor) = if up_line { (x1 - PILL_INSET * Z, 2) } else { (x0 + PILL_INSET * Z, 0) };
                    grips.push(PanelGrip { x: gx, y, anchor, label: "".into(), value: value.into(), hot: band == sel, plain: false });
                }
                let range = def.param(id).map_or(1.0, |p| (p.max - p.min).max(1e-3));
                zones.push(PanelZone {
                    x: x0,
                    y: y - 6.0 * Z,
                    w: x1 - x0,
                    h: 12.0 * Z,
                    param_x: -1,
                    param_y: idx(id),
                    rate_x: 0.0,
                    rate_y: 1.0 / (per_db * range),
                });
            }
        }
        for (id, hz, edge) in [("low_crossover", f1, edges[1]), ("high_crossover", f2, edges[2])] {
            let x = edge.round() + 0.5;
            paths.push(stroke(format!("M{x:.1} 0 V{ph:.1}"), handle, 1.5 * Z));
            let value = def.param(id).map_or_else(String::new, |p| p.format(hz));
            let w = pill_w(&value);
            let gx = (x - w / 2.0).clamp(2.0 * Z, pw - w - 2.0 * Z);
            grips.push(PanelGrip { x: gx, y: ph - 14.0 * Z, anchor: 0, label: "".into(), value: value.into(), hot: false, plain: false });
            // Norm per px: the axis is log 20 Hz..20 kHz, the parameter log over its own range.
            let rate = def.param(id).map_or(0.0, |p| (F_HI / F_LO).ln() / pw / (p.max / p.min).ln());
            zones.push(PanelZone { x: x - 6.0 * Z, y: 0.0, w: 12.0 * Z, h: ph, param_x: idx(id), param_y: -1, rate_x: rate, rate_y: 0.0 });
        }
        PanelCanvas {
            visible: true,
            w: pw,
            h: ph,
            paths: model(paths),
            texts: model(texts),
            grips: model(grips),
            zones: model(zones),
            label: "Bands".into(),
            ..Default::default()
        }
    }
}
