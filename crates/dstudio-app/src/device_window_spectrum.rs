//! Spectrum's expanded window display, in its three views. 2D: the analyser curve and its
//! peak hold over a dB ladder (Ceiling down by Range) and a log Hz axis, with the tilt
//! guide. 3D: the frames of the history as a waterfall receding from the front. Split:
//! the 2D picture over a spectrogram of the history (newest row at the top). The window
//! keeps up to History Depth frames while it is open; Hold view stops taking them.

use crate::PanelCanvas;
use crate::analyzer_views::{SPECTRUM_BANDS, spectrum_color};
use crate::controller::Controller;
use crate::device_canvas::{dashed, fill, path, push_tenths, rgba, stroke, text, white};
use crate::device_panel::Z;
use crate::device_window::Kit;
use crate::{PanelPath, PanelText};
use dstudio_core::builtin::DeviceDef;
use dstudio_core::{Device, DeviceId};
use slint::{Color, ModelRc, VecModel};
use std::collections::VecDeque;
use std::fmt::Write;
use std::rc::Rc;

fn model<T: Clone + 'static>(v: Vec<T>) -> ModelRc<T> {
    ModelRc::from(Rc::new(VecModel::from(v)))
}

/// History Depth choices (frames).
pub(crate) const DEPTHS: [usize; 6] = [32, 64, 96, 128, 192, 256];
/// Display modes.
pub(crate) const MODE_2D: i32 = 0;
pub(crate) const MODE_3D: i32 = 1;
pub(crate) const MODE_SPLIT: i32 = 2;
/// Split: height of the 2D part (pt from the plot top) and where the spectrogram starts.
const SPLIT_2D_H: f32 = 242.0;
pub(crate) const SPLIT_GRAM_Y: f32 = 278.0;
/// Most waterfall lines and spectrogram rows drawn, and points / columns per row.
const MAX_LINES: usize = 32;
const MAX_ROWS: usize = 64;
const COLS: usize = 96;
/// Spectrogram ink steps.
const STEPS: usize = 8;
const HZ_TICKS: [(f32, &str); 9] = [
    (50.0, "50"),
    (100.0, "100"),
    (200.0, "200"),
    (500.0, "500"),
    (1000.0, "1k"),
    (2000.0, "2k"),
    (5000.0, "5k"),
    (10000.0, "10k"),
    (20000.0, "20k"),
];

/// The frames a Spectrum window keeps: the last analyser sequence taken and the shown
/// curves, newest last.
#[derive(Debug, Default, Clone)]
pub(crate) struct SpecHistory {
    pub seq: u64,
    pub frames: VecDeque<Vec<f32>>,
}

impl SpecHistory {
    /// Takes a new frame (once per analyser sequence), keeping at most `depth`.
    pub(crate) fn take(&mut self, seq: u64, frame: &[f32], depth: usize) -> bool {
        if seq == 0 || seq == self.seq || frame.is_empty() {
            return false;
        }
        self.seq = seq;
        self.frames.push_back(frame.to_vec());
        while self.frames.len() > depth.max(1) {
            self.frames.pop_front();
        }
        true
    }
}

/// x (0..1) of band `i` on the 20 Hz..20 kHz log axis.
fn band_t(i: usize) -> f32 {
    (i as f32 + 0.5) / SPECTRUM_BANDS as f32
}

/// `n` values sampled evenly from a frame (the loudest band of each span).
fn decimate(frame: &[f32], n: usize) -> Vec<f32> {
    let len = frame.len().max(1);
    (0..n)
        .map(|k| {
            let (a, b) = (k * len / n, ((k + 1) * len / n).max(k * len / n + 1).min(len));
            frame[a.min(len - 1)..b].iter().copied().fold(f32::MIN, f32::max)
        })
        .collect()
}

/// "−12.3 dB", or a dash when nothing is above the floor.
fn peak_text(db: Option<f32>) -> String {
    match db {
        Some(v) if v > -119.0 => {
            if v < 0.0 {
                format!("\u{2212}{:.1} dB", -v)
            } else {
                format!("{v:.1} dB")
            }
        }
        _ => "\u{2013}".into(),
    }
}

impl Controller {
    pub(crate) fn win_spec_history(&self, id: DeviceId) -> Option<&SpecHistory> {
        self.devices.wins.spec.get(&id)
    }

    pub(crate) fn spectrum_window_canvas(&self, d: &Device, def: &'static DeviceDef, k: &Kit, _accent: Color) -> PanelCanvas {
        let v = |id: &str, dflt: f32| def.param(id).map_or(dflt, |p| self.panel_value(d, def, p));
        let (_, _, dw, dh) = k.display;
        let (pl, pt, pr, pb) = k.plot;
        let (pw, ph) = ((dw - pl - pr) * Z, (dh - pt - pb) * Z);
        let mode = v("display_mode", 1.0).round() as i32;
        let (ceiling, range, slope) = (v("db_ceiling", 0.0), v("db_range", 90.0).max(1.0), v("slope", 4.5));
        let color = spectrum_color(v("color_scheme", 0.0).round() as usize);
        let dim = white(0.32);
        let mut paths: Vec<PanelPath> = Vec::new();
        let mut texts: Vec<PanelText> = Vec::new();
        let frame = self.spectrum_frame(d.id);
        let history = self.win_spec_history(d.id);
        let depth = DEPTHS[(v("history_depth", 3.0).round().max(0.0) as usize).min(DEPTHS.len() - 1)];

        // Caption: the loudest peak and the transport; the tilt on the right.
        let cy = -(pt - 18.0) * Z;
        let peak = frame.and_then(|(_, _, p)| p.iter().copied().reduce(f32::max));
        texts.push(text(0.0, cy, 60.0 * Z, "Peak", white(0.55), 12.0 * Z, 0));
        let mut x = (crate::text_measure::value_width("Peak") + 12.0) * Z;
        let pk = peak_text(peak);
        texts.push(text(x, cy, 100.0 * Z, pk.clone(), color, 12.0 * Z, 0));
        x += (crate::text_measure::value_width(&pk) + 16.0) * Z;
        if !self.project_playing() {
            texts.push(text(x, cy, 80.0 * Z, "stopped", dim, 10.0 * Z, 0));
        }
        let tilt = format!("Tilt {}{:.1} dB/oct", if slope < 0.0 { "\u{2212}" } else { "+" }, slope.abs());
        texts.push(text(pw, cy, 160.0 * Z, tilt, dim, 10.0 * Z, 2));

        if mode == MODE_3D {
            Self::spectrum_waterfall(history, depth, pw, ph, ceiling, range, color, &mut paths);
        } else {
            let h2 = if mode == MODE_SPLIT { SPLIT_2D_H * Z } else { ph };
            Self::spectrum_2d(frame, pw, h2, ceiling, range, slope, color, &mut paths, &mut texts);
            if mode == MODE_SPLIT {
                let (gx, gy) = (-pl * Z, SPLIT_GRAM_Y * Z);
                let (gw, gh) = (dw * Z, ph + pb * Z - gy);
                paths.push(fill(format!("M{gx:.1} {gy:.1} h{gw:.1} v{gh:.1} h{:.1} Z", -gw), Color::from_rgb_u8(0, 0, 0)));
                Self::spectrogram(history, depth, gx, gy, gw, gh, ceiling, range, color, &mut paths);
            }
        }
        PanelCanvas {
            visible: true,
            w: pw,
            h: ph,
            paths: model(paths),
            texts: model(texts),
            label: "Spectrum".into(),
            ..Default::default()
        }
    }

    /// The 2D picture `w` x `h`: grid, ladder, axis, tilt guide, peak hold and curve.
    #[allow(clippy::too_many_arguments)]
    fn spectrum_2d(
        frame: Option<(u64, &[f32], &[f32])>,
        w: f32,
        h: f32,
        ceiling: f32,
        range: f32,
        slope: f32,
        color: Color,
        paths: &mut Vec<PanelPath>,
        texts: &mut Vec<PanelText>,
    ) {
        let dim = white(0.32);
        let x_of = |hz: f32| (hz / 20.0).ln() / 1000f32.ln() * w;
        let y_of = |db: f32| ((ceiling - db) / range).clamp(-0.01, 1.01) * h;
        let (mut grid, mut strong) = (String::new(), String::new());
        for (hz, label) in HZ_TICKS {
            let x = x_of(hz).round() + 0.5;
            let _ = write!(if hz == 1000.0 { &mut strong } else { &mut grid }, "M{x:.1} 0 V{h:.1} ");
            texts.push(text(x, h + 10.0 * Z, 40.0 * Z, label, dim, 10.0 * Z, if hz >= 20000.0 { 2 } else { 1 }));
        }
        texts.push(text(-6.0 * Z, h + 10.0 * Z, 40.0 * Z, "dBFS", dim, 10.0 * Z, 2));
        texts.push(text(w + 30.0 * Z, h + 10.0 * Z, 30.0 * Z, "Hz", dim, 10.0 * Z, 2));
        let step = if range >= 72.0 {
            12.0
        } else if range >= 36.0 {
            6.0
        } else {
            3.0
        };
        let mut db = (ceiling / step).floor() * step;
        while db > ceiling - range + 0.1 {
            let y = y_of(db).round() + 0.5;
            let _ = write!(grid, "M0 {y:.1} H{w:.1} ");
            let label = if db < 0.0 { format!("\u{2212}{:.0}", -db) } else { format!("{db:.0}") };
            texts.push(text(-8.0 * Z, y_of(db), 40.0 * Z, label, dim, 10.0 * Z, 2));
            db -= step;
        }
        let _ = write!(grid, "M0 {h:.1} H{w:.1} M{w:.1} 0 V{h:.1}");
        paths.push(stroke(grid, white(0.05), 1.0));
        paths.push(stroke(strong, white(0.14), 1.0));
        let guide = |hz: f32| h / 2.0 - slope * (hz / 1000.0).log2() * h / range;
        paths.push(stroke(dashed(&[(0.0, guide(20.0)), (w, guide(20000.0))], 3.0 * Z, 4.0 * Z), white(0.22), 1.0));
        let Some((_, shown, peak)) = frame else { return };
        let pts = |v: &[f32]| -> Vec<(f32, f32)> { v.iter().enumerate().map(|(i, db)| (band_t(i) * w, y_of(*db))).collect() };
        if peak.iter().any(|db| *db > -119.99) {
            paths.push(stroke(path(&pts(peak)), rgba(color, 0.35), 1.0));
        }
        if shown.iter().any(|db| *db > -119.99) {
            let p = pts(shown);
            let line = path(&p);
            paths.push(PanelPath { fill2: rgba(color, 0.04), ..fill(format!("{line} L{w:.1} {h:.1} L0 {h:.1} Z"), rgba(color, 0.5)) });
            paths.push(stroke(line, rgba(color, 0.95), 1.25 * Z));
        }
    }

    /// The waterfall: up to MAX_LINES frames from the back (oldest, raised and narrowed)
    /// to the front (newest), each filled with the display so it hides those behind it.
    #[allow(clippy::too_many_arguments)]
    #[allow(clippy::too_many_arguments)]
    fn spectrum_waterfall(
        history: Option<&SpecHistory>,
        depth_frames: usize,
        w: f32,
        h: f32,
        ceiling: f32,
        range: f32,
        color: Color,
        paths: &mut Vec<PanelPath>,
    ) {
        let depth = 0.38 * h;
        let amp = 0.55 * h;
        let inset = 0.1 * w;
        let base = |k: f32| h - k * depth;
        let span = |k: f32| (k * inset, w - k * inset);
        // The floor: its outline and a few depth lines.
        let (bl, br) = span(1.0);
        let mut floor = format!("M0 {h:.1} L{w:.1} {h:.1} L{br:.1} {b:.1} L{bl:.1} {b:.1} Z ", b = base(1.0));
        for i in 1..4 {
            let k = i as f32 / 4.0;
            let (l, r) = span(k);
            let _ = write!(floor, "M{l:.1} {y:.1} H{r:.1} ", y = base(k));
        }
        paths.push(stroke(floor, white(0.08), 1.0));
        let Some(hist) = history.filter(|h| !h.frames.is_empty()) else { return };
        // Lines sit at their age on the floor: the back is the oldest frame the history keeps,
        // so a history still filling up starts at the front.
        let n = hist.frames.len();
        let full = depth_frames.max(n).max(2);
        let stride = full.div_ceil(MAX_LINES);
        let lines = n.div_ceil(stride);
        let bg = Color::from_rgb_u8(0x14, 0x15, 0x17);
        for li in (0..lines).rev() {
            // Oldest first: the largest age is drawn first, at the back.
            let age = li * stride;
            let fi = n - 1 - age;
            let k = age as f32 / (full - 1) as f32;
            let (l, r) = span(k);
            let y0 = base(k);
            let a = amp * (1.0 - 0.35 * k);
            let vals = decimate(&hist.frames[fi], COLS);
            let pts: Vec<(f32, f32)> = vals
                .iter()
                .enumerate()
                .map(|(c, db)| {
                    let t = ((db - (ceiling - range)) / range).clamp(0.0, 1.0);
                    (l + (r - l) * (c as f32 + 0.5) / COLS as f32, y0 - t * a)
                })
                .collect();
            let line = path(&pts);
            paths.push(fill(format!("{line} L{:.1} {y0:.1} L{:.1} {y0:.1} Z", pts[pts.len() - 1].0, pts[0].0), bg));
            paths.push(stroke(line, rgba(color, 0.25 + 0.7 * (1.0 - k)), if k < 0.01 { 1.25 * Z } else { 1.0 }));
        }
    }

    /// The spectrogram: one row per frame (newest at the top), its level in STEPS inks.
    #[allow(clippy::too_many_arguments)]
    fn spectrogram(
        history: Option<&SpecHistory>,
        depth_frames: usize,
        x0: f32,
        y0: f32,
        w: f32,
        h: f32,
        ceiling: f32,
        range: f32,
        color: Color,
        paths: &mut Vec<PanelPath>,
    ) {
        let Some(hist) = history.filter(|h| !h.frames.is_empty()) else { return };
        // Rows are sized for the whole history, so one still filling up grows down from the top.
        let n = hist.frames.len();
        let full = depth_frames.max(n).max(1);
        let stride = full.div_ceil(MAX_ROWS);
        let rh = h / full.div_ceil(stride) as f32;
        let rows = n.div_ceil(stride);
        let cw = w / COLS as f32;
        let mut inks: Vec<String> = vec![String::new(); STEPS];
        for ri in 0..rows {
            // Row 0 (top) is the newest.
            let fi = n - 1 - ri * stride;
            let vals = decimate(&hist.frames[fi], COLS);
            let y = y0 + ri as f32 * rh;
            let mut c = 0;
            while c < COLS {
                let step = |db: f32| ((((db - (ceiling - range)) / range).clamp(0.0, 1.0)) * STEPS as f32) as usize;
                let s = step(vals[c]).min(STEPS);
                let mut e = c + 1;
                while e < COLS && step(vals[e]).min(STEPS) == s {
                    e += 1;
                }
                if s > 0 {
                    let (o, rw) = (&mut inks[s - 1], (e - c) as f32 * cw);
                    o.push('M');
                    push_tenths(o, x0 + c as f32 * cw);
                    o.push(' ');
                    push_tenths(o, y);
                    o.push_str(" h");
                    push_tenths(o, rw);
                    o.push_str(" v");
                    push_tenths(o, rh);
                    o.push_str(" h");
                    push_tenths(o, -rw);
                    o.push_str(" Z ");
                }
                c = e;
            }
        }
        for (i, ink) in inks.into_iter().enumerate() {
            if !ink.is_empty() {
                paths.push(fill(ink, rgba(color, (i + 1) as f32 / STEPS as f32)));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn history_takes_each_sequence_once_and_keeps_the_depth() {
        let mut h = SpecHistory::default();
        assert!(h.take(1, &[0.0; 4], 2));
        assert!(!h.take(1, &[1.0; 4], 2));
        assert!(h.take(2, &[2.0; 4], 2));
        assert!(h.take(3, &[3.0; 4], 2));
        assert_eq!(h.frames.len(), 2);
        assert_eq!(h.frames[0][0], 2.0);
        assert!(!h.take(0, &[4.0; 4], 2));
    }

    /// A history still filling up keeps its rows and lines at their age: the spectrogram
    /// grows down from the top and the waterfall starts at the front.
    #[test]
    fn a_short_history_sits_at_its_age() {
        let mut h = SpecHistory::default();
        for i in 1..=3u64 {
            h.take(i, &[-10.0; 256], 64);
        }
        let mut paths = Vec::new();
        Controller::spectrogram(Some(&h), 64, 0.0, 0.0, 96.0, 64.0, 0.0, 90.0, Color::from_rgb_u8(255, 0, 0), &mut paths);
        let ys: Vec<f32> = paths
            .iter()
            .flat_map(|p| p.commands.split('M').skip(1).map(|r| r.split_whitespace().nth(1).unwrap().parse::<f32>().unwrap()))
            .collect();
        assert!(!ys.is_empty() && ys.iter().all(|y| *y < 3.5), "{ys:?}");
        let mut lines = Vec::new();
        Controller::spectrum_waterfall(Some(&h), 32, 400.0, 200.0, 0.0, 90.0, Color::from_rgb_u8(255, 0, 0), &mut lines);
        // Floor, then a fill and a stroke per line; every line sits near the front edge.
        assert_eq!(lines.len(), 1 + 2 * 3);
        let first_x: f32 = lines[1].commands[1..].split_whitespace().next().unwrap().parse().unwrap();
        assert!(first_x < 5.0, "the oldest of three frames is drawn at the front, not the back: {first_x}");
    }

    #[test]
    fn decimate_keeps_the_loudest_band_of_each_span() {
        let f: Vec<f32> = (0..256).map(|i| if i == 37 { 0.0 } else { -60.0 }).collect();
        let d = decimate(&f, 96);
        assert_eq!(d.len(), 96);
        assert_eq!(d.iter().filter(|v| **v == 0.0).count(), 1);
        assert_eq!(peak_text(Some(-12.34)), "\u{2212}12.3 dB");
        assert_eq!(peak_text(None), "\u{2013}");
    }
}
