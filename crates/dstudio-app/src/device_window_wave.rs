//! Expanded window display of the waveshapers (Distortion, Tape Saturation, Soft Clipper):
//! the static transfer on a 48 dB square (input level against output level, both peak
//! dBFS of a sine), with the band between it and unity lit, and beside it two cycles of a
//! full-scale sine (dashed) and what the device makes of it. A vertical drag on the wave
//! moves the device's first parameter (Drive, or the Soft Clipper's Input), as on the card.
//! The Bit Crusher's display is one cycle of a sine through its sample-and-hold and
//! quantiser, under a caption of the held rate and the number of levels.

use crate::PanelCanvas;
use crate::controller::Controller;
use crate::device_canvas::{dashed, fill, rgba, stroke, text, white};
use crate::device_panel::Z;
use crate::device_window::Kit;
use crate::{PanelPath, PanelText, PanelZone};
use dstudio_core::Device;
use dstudio_core::builtin::{self, DeviceDef};
use slint::{Color, ModelRc, VecModel};
use std::f32::consts::TAU;
use std::rc::Rc;

fn model<T: Clone + 'static>(v: Vec<T>) -> ModelRc<T> {
    ModelRc::from(Rc::new(VecModel::from(v)))
}

/// The square's range: input and output peak level (dBFS).
const DB_LO: f32 = -36.0;
const DB_HI: f32 = 12.0;
const TICKS: [f32; 4] = [-24.0, -12.0, 0.0, 12.0];
/// Gap between the square and the wave panel (pt).
const GAP: f32 = 40.0;
const FILL: f32 = 0.30;

/// A device's sample law and the open (unclipped) path it is measured against.
pub(crate) struct Law {
    pub curve: Box<dyn Fn(f32) -> f32>,
    pub open: Box<dyn Fn(f32) -> f32>,
    /// The Soft Clipper's caption reads its clipping; the others their gain.
    pub clipper: bool,
}

pub(crate) fn law(def: &DeviceDef, v: &dyn Fn(&str) -> f32) -> Law {
    match def.id {
        "tapesaturation" => {
            let (dr, cp, mx, out, mode) = (v("drive"), v("compression"), v("mix"), v("output_gain"), v("mode").round() as u32);
            Law { curve: Box::new(move |x| builtin::tape_transfer(x, dr, cp, mx, out, mode)), open: Box::new(|x| x), clipper: false }
        }
        // The De-Clipper's picture: the repaired signal held at the threshold, then scaled
        // under the ceiling (the card's model).
        "declipper" => {
            let (t, c, mx) = (v("threshold"), v("ceiling"), v("mix") / 100.0);
            Law { curve: Box::new(move |x| crate::panel_graph::declipper_wave(x, t, c, mx).0), open: Box::new(|x| x), clipper: false }
        }
        "soft-clipper" => {
            let (inp, ceil, soft, shape, gm, mx, out) =
                (v("input"), v("ceiling"), v("softness"), v("shape").round() as u32, v("gain_match") >= 0.5, v("mix"), v("output_gain"));
            Law {
                curve: Box::new(move |x| builtin::clipper_transfer(x, inp, ceil, soft, shape, gm, mx, out)),
                open: Box::new(move |x| builtin::clipper_open(x, inp, gm, mx, out)),
                clipper: true,
            }
        }
        _ => {
            let (dr, bias, gain, mx, out, mode) =
                (v("drive"), v("bias"), v("output"), v("mix"), v("output_gain"), v("mode").round() as u32);
            Law {
                curve: Box::new(move |x| builtin::distortion_transfer(x, dr, bias, gain, mx, out, mode)),
                open: Box::new(|x| x),
                clipper: false,
            }
        }
    }
}

/// Peak (dBFS) of f over one cycle of a sine at `level` dBFS: the two crests exactly, and
/// a coarse pass between them for shapers that fold back before the crest.
pub(crate) fn peak_db(f: &dyn Fn(f32) -> f32, level: f32) -> f32 {
    let a = 10f32.powf(level / 20.0);
    let n = 8;
    let crest = f(a).abs().max(f(-a).abs());
    let peak = (0..n).map(|i| f(a * (TAU * (i as f32 + 0.5) / n as f32).sin()).abs()).fold(crest, f32::max);
    20.0 * peak.max(1e-6).log10()
}

/// "−12.0 dB".
fn db_text(db: f32) -> String {
    let a = (db.abs() * 10.0).round() / 10.0;
    if a < 0.05 {
        "0.0 dB".into()
    } else if db < 0.0 {
        format!("\u{2212}{a:.1} dB")
    } else {
        format!("+{a:.1} dB")
    }
}

fn tick_label(db: f32) -> String {
    if db > 0.0 {
        format!("+{db:.0}")
    } else if db < 0.0 {
        format!("\u{2212}{:.0}", -db)
    } else {
        "0".into()
    }
}

impl Controller {
    /// The waveshaper's caption: ("Transfer", its gain at 0 dBFS) or ("Clipping", how far
    /// the clipper holds a full-scale sine under the open path).
    pub(crate) fn wave_caption(&self, d: &Device, def: &'static DeviceDef) -> (&'static str, String) {
        let v = |id: &str| def.param(id).map_or(0.0, |p| self.panel_value(d, def, p));
        let l = law(def, &v);
        if l.clipper {
            ("Clipping", db_text(peak_db(&*l.curve, 0.0) - peak_db(&*l.open, 0.0)))
        } else {
            ("Transfer", db_text(peak_db(&*l.curve, 0.0)))
        }
    }

    pub(crate) fn wave_window_canvas(&self, d: &Device, def: &'static DeviceDef, k: &Kit, accent: Color) -> PanelCanvas {
        let v = |id: &str| def.param(id).map_or(0.0, |p| self.panel_value(d, def, p));
        let l = law(def, &v);
        let (_, _, dw, dh) = k.display;
        let (pl, pt, pr, pb) = k.plot;
        let (pw, ph) = ((dw - pl - pr) * Z, (dh - pt - pb) * Z);
        let side = ph;
        let per_db = side / (DB_HI - DB_LO);
        let x_of = |db: f32| (db.clamp(DB_LO, DB_HI) - DB_LO) * per_db;
        let y_of = |db: f32| (DB_HI - db.clamp(DB_LO, DB_HI)) * per_db;
        let mut paths: Vec<PanelPath> = Vec::new();
        let mut texts: Vec<PanelText> = Vec::new();
        let dim = white(0.3);

        // The square: grid every 6 dB, the 0 dB lines a step brighter, the ladder and axis.
        let mut grid = String::new();
        let mut zero = String::new();
        let mut db = DB_LO + 6.0;
        while db < DB_HI - 0.1 {
            let (x, y) = (x_of(db).round() + 0.5, y_of(db).round() + 0.5);
            let s = if db == 0.0 { &mut zero } else { &mut grid };
            s.push_str(&format!("M{x:.1} 0 V{side:.1} M0 {y:.1} H{side:.1} "));
            db += 6.0;
        }
        paths.push(stroke(grid, white(0.05), 1.0));
        paths.push(stroke(zero, white(0.14), 1.0));
        for db in [DB_HI, 0.0, -12.0, -24.0, DB_LO] {
            texts.push(text(-8.0 * Z, y_of(db), 40.0 * Z, tick_label(db), dim, 11.0 * Z, 2));
        }
        let axis_y = side + 12.0 * Z;
        for db in TICKS.iter().take(3) {
            texts.push(text(x_of(*db), axis_y, 40.0 * Z, tick_label(*db), dim, 10.0 * Z, 1));
        }
        texts.push(text(-8.0 * Z, axis_y, 44.0 * Z, "dBFS", dim, 10.0 * Z, 2));
        texts.push(text(side, axis_y, 60.0 * Z, "dBFS in", dim, 10.0 * Z, 2));

        // Transfer, unity and the band between them (gain, or what the clipper takes off).
        let n = 48;
        let ins: Vec<f32> = (0..=n).map(|i| DB_LO + (DB_HI - DB_LO) * i as f32 / n as f32).collect();
        let out: Vec<(f32, f32)> = ins.iter().map(|&x| (x_of(x), y_of(peak_db(&*l.curve, x)))).collect();
        let open: Vec<(f32, f32)> = ins.iter().map(|&x| (x_of(x), y_of(if l.clipper { peak_db(&*l.open, x) } else { x }))).collect();
        let mut band = crate::device_canvas::path(&out);
        for (x, y) in open.iter().rev() {
            band.push_str(&format!(" L{x:.1} {y:.1}"));
        }
        band.push_str(" Z");
        paths.push(fill(band, rgba(accent, FILL)));
        let unity: Vec<(f32, f32)> = ins.iter().map(|&x| (x_of(x), y_of(x))).collect();
        paths.push(stroke(dashed(&unity, 3.0 * Z, 3.0 * Z), white(0.35), 1.0));
        paths.push(stroke(crate::device_canvas::path(&out), white(0.95), 1.5 * Z));

        // The wave: two cycles of a full-scale sine (dashed) and the device's output, filled
        // to the zero line, between full-scale rules.
        let (wx0, wx1) = (side + GAP * Z, pw);
        let mid = side / 2.0;
        let amp = 0.4 * side;
        let wy = |s: f32| mid - s.clamp(-1.25, 1.25) * amp;
        paths.push(stroke(format!("M{wx0:.1} {a:.1} H{wx1:.1} M{wx0:.1} {b:.1} H{wx1:.1}", a = wy(1.0), b = wy(-1.0)), white(0.08), 1.0));
        paths.push(stroke(format!("M{wx0:.1} {mid:.1} H{wx1:.1}"), white(0.14), 1.0));
        let m = ((wx1 - wx0) / (2.0 * Z)).round().max(64.0) as usize;
        let ph_of = |i: usize| TAU * 2.0 * i as f32 / m as f32;
        let xs = |i: usize| wx0 + (wx1 - wx0) * i as f32 / m as f32;
        let input: Vec<(f32, f32)> = (0..=m).map(|i| (xs(i), wy(ph_of(i).sin()))).collect();
        let shaped: Vec<(f32, f32)> = (0..=m).map(|i| (xs(i), wy((l.curve)(ph_of(i).sin())))).collect();
        paths.push(stroke(dashed(&input, 3.0 * Z, 3.0 * Z), white(0.3), 1.0));
        let mut wave = crate::device_canvas::path(&shaped);
        wave.push_str(&format!(" L{wx1:.1} {mid:.1} L{wx0:.1} {mid:.1} Z"));
        paths.push(fill(wave, rgba(accent, FILL)));
        paths.push(stroke(crate::device_canvas::path(&shaped), accent, 1.5 * Z));
        texts.push(text(wx1, 6.0 * Z, 80.0 * Z, "Wave", dim, 10.0 * Z, 2));

        // Legend under the axis.
        let ly = side + 31.0 * Z;
        let (glyph, gap) = (14.0 * Z, 6.0 * Z);
        let mut x = 0.0;
        let band_name = if l.clipper {
            "Clipping"
        } else if def.id == "declipper" {
            "Reduction"
        } else {
            "Gain"
        };
        for (kind, label) in [(0, "Transfer"), (1, "Unity"), (2, band_name)] {
            match kind {
                0 => paths.push(stroke(format!("M{x:.1} {ly:.1} h{glyph:.1}"), white(0.9), 1.5 * Z)),
                1 => paths.push(stroke(dashed(&[(x, ly), (x + glyph, ly)], 3.0 * Z, 3.0 * Z), white(0.4), 1.5 * Z)),
                _ => paths.push(fill(
                    format!("M{x:.1} {:.1} h{glyph:.1} v{:.1} h{:.1} Z", ly - 4.0 * Z, 8.0 * Z, -glyph),
                    rgba(accent, FILL + 0.05),
                )),
            }
            texts.push(text(x + glyph + gap, ly, 90.0 * Z, label, white(0.7), 9.0 * Z, 0));
            x += glyph + gap + crate::text_measure::value_width(label) * 0.9 * Z + 16.0 * Z;
        }

        // Caption above the plot.
        let (caption, value) = self.wave_caption(d, def);
        let cy = -(pt - 12.0) * Z;
        texts.push(text(0.0, cy, 120.0 * Z, caption, white(0.55), 12.0 * Z, 0));
        let at = (crate::text_measure::value_width(caption) + 12.0) * Z;
        texts.push(text(at, cy, 160.0 * Z, value, white(0.92), 13.0 * Z, 0));

        // A vertical drag on the wave moves the first parameter, as on the card.
        let p0 = &def.params[0];
        let zones = vec![PanelZone {
            x: wx0,
            y: 0.0,
            w: wx1 - wx0,
            h: side,
            param_x: -1,
            param_y: def.index_of(p0.id).map_or(-1, |i| i as i32),
            rate_x: 0.0,
            rate_y: 1.0 / (side - 16.0 * Z).max(8.0),
        }];
        PanelCanvas {
            visible: true,
            w: pw,
            h: ph,
            paths: model(paths),
            texts: model(texts),
            zones: model(zones),
            label: caption.into(),
            ..Default::default()
        }
    }

    /// Bit Crusher: one cycle through the crusher, filled to the zero line, the dry sine
    /// dashed beneath; "Rate 24.0 kHz and 256 levels" above. Right drag lowers Downsample,
    /// up raises Bit Depth, as on the card.
    pub(crate) fn crush_window_canvas(&self, d: &Device, def: &'static DeviceDef, k: &Kit, accent: Color) -> PanelCanvas {
        let v = |id: &str| def.param(id).map_or(0.0, |p| self.panel_value(d, def, p));
        let (_, _, dw, dh) = k.display;
        let (pl, pt, pr, pb) = k.plot;
        let (pw, ph) = ((dw - pl - pr) * Z, (dh - pt - pb) * Z);
        let bits = v("bitdepth");
        let pic = builtin::crush_picture(bits, v("downsample"), v("mix"), v("dither"), v("jitter"));
        let mid = ph / 2.0;
        let amp = (mid - 12.0 * Z) / 1.15;
        let y_of = |s: f32| mid - s.clamp(-1.15, 1.15) * amp;
        let n = pic.len();
        let x_of = |i: usize| pw * i as f32 / n as f32;
        let mut paths: Vec<PanelPath> = Vec::new();
        paths.push(stroke(format!("M0 {a:.1} H{pw:.1} M0 {b:.1} H{pw:.1}", a = y_of(1.0), b = y_of(-1.0)), white(0.06), 1.0));
        paths.push(stroke(format!("M0 {mid:.1} H{pw:.1}"), white(0.14), 1.0));
        let dry: Vec<(f32, f32)> = (0..=n).map(|i| (x_of(i), y_of(pic[i.min(n - 1)].0))).collect();
        let mut steps = Vec::with_capacity(2 * n);
        for (i, (_, c)) in pic.iter().enumerate() {
            steps.push((x_of(i), y_of(*c)));
            steps.push((x_of(i + 1), y_of(*c)));
        }
        let line = crate::device_canvas::path(&steps);
        paths.push(stroke(dashed(&dry, 3.0 * Z, 3.0 * Z), white(0.25), 1.0));
        paths.push(fill(format!("{line} L{pw:.1} {mid:.1} L0 {mid:.1} Z"), rgba(accent, FILL)));
        paths.push(stroke(line, white(0.95), 1.5 * Z));
        let (rate, levels) = crush_caption(bits, v("downsample"));
        let cy = -(pt - 12.0) * Z;
        let at = (crate::text_measure::value_width("Rate") + 12.0) * Z;
        let at2 = at + (crate::text_measure::value_width(&rate) * 1.15 + 16.0) * Z;
        let texts = vec![
            text(0.0, cy, 60.0 * Z, "Rate", white(0.55), 12.0 * Z, 0),
            text(at, cy, 120.0 * Z, rate, white(0.92), 13.0 * Z, 0),
            text(at2, cy, 200.0 * Z, levels, white(0.5), 11.0 * Z, 0),
        ];
        let idx = |id: &str| def.index_of(id).map_or(-1, |i| i as i32);
        let zones = vec![PanelZone {
            x: 0.0,
            y: 0.0,
            w: pw,
            h: ph,
            param_x: idx("downsample"),
            param_y: idx("bitdepth"),
            rate_x: -1.0 / (pw - 16.0 * Z).max(8.0),
            rate_y: 1.0 / (ph - 16.0 * Z).max(8.0),
        }];
        PanelCanvas {
            visible: true,
            w: pw,
            h: ph,
            paths: model(paths),
            texts: model(texts),
            zones: model(zones),
            label: "Rate".into(),
            ..Default::default()
        }
    }
}

/// The crusher's held rate at 48 kHz and its number of levels: ("24.0 kHz", "and 256 levels").
pub(crate) fn crush_caption(bits: f32, downsample: f32) -> (String, String) {
    let khz = 48.0 / downsample.max(1.0);
    let rate = if khz >= 10.0 { format!("{khz:.1} kHz") } else { format!("{:.0} Hz", khz * 1000.0) };
    let b = bits.round().clamp(1.0, 32.0) as u32;
    let levels = if b <= 20 {
        let n = 1u64 << b;
        let mut s = n.to_string();
        let mut i = s.len() as i32 - 3;
        while i > 0 {
            s.insert(i as usize, ',');
            i -= 3;
        }
        s
    } else {
        format!("2^{b}")
    };
    (rate, format!("and {levels} levels"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// With nothing driven, the transfer is unity shifted by the output gain.
    #[test]
    fn transfer_follows_the_output_gain() {
        let def = builtin::find("tapesaturation").unwrap();
        let v = |id: &str| match id {
            "drive" | "compression" | "mix" | "mode" => 0.0,
            "output_gain" => -12.0,
            _ => 0.0,
        };
        let l = law(def, &v);
        for lv in [-24.0, -12.0, 0.0] {
            assert!((peak_db(&*l.curve, lv) - (lv - 12.0)).abs() < 0.2, "{lv}");
        }
    }

    /// The De-Clipper holds the transfer at its threshold above it and passes it below.
    #[test]
    fn declipper_transfer_holds_at_the_threshold() {
        let def = builtin::find("declipper").unwrap();
        let v = |id: &str| match id {
            "threshold" => -6.0,
            "ceiling" => -0.3,
            "mix" => 0.0,
            _ => 0.0,
        };
        let l = law(def, &v);
        assert!((peak_db(&*l.curve, -12.0) + 12.0).abs() < 0.1);
        assert!((peak_db(&*l.curve, 0.0) + 6.0).abs() < 0.1);
        assert!((peak_db(&*l.curve, 12.0) + 6.0).abs() < 0.1);
    }

    /// The soft clipper holds a hot sine under its open path.
    #[test]
    fn clipper_holds_a_hot_sine_under_the_open_path() {
        let def = builtin::find("soft-clipper").unwrap();
        let v = |id: &str| match id {
            "input" => 12.0,
            "ceiling" => -6.0,
            "softness" => 50.0,
            "mix" => 100.0,
            _ => 0.0,
        };
        let l = law(def, &v);
        assert!(peak_db(&*l.curve, 0.0) < peak_db(&*l.open, 0.0) - 6.0);
    }

    #[test]
    fn crush_caption_reads_rate_and_levels() {
        assert_eq!(crush_caption(1.0, 1.0), ("48.0 kHz".to_string(), "and 2 levels".to_string()));
        assert_eq!(crush_caption(16.0, 2.0), ("24.0 kHz".to_string(), "and 65,536 levels".to_string()));
        assert_eq!(crush_caption(8.0, 12.0).0, "4000 Hz");
    }
}
