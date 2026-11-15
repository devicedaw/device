//! Transient Shaper's expanded window: the gain scope at window size (the output's peaks
//! as a mirrored grey silhouette, what Clip shaved as a faint ghost, and the gain the
//! shaper applied as one accent line over the last 4 s), and the shape picture in its
//! control card: a model hit run through the shaper's own detector, with the gain curve
//! the settings ask for and its two handles (Attack amount and time, Sustain amount and
//! time) to drag.

use crate::PanelCanvas;
use crate::controller::Controller;
use crate::device_canvas::{dashed, fill, path, rgba, stroke, text, white};
use crate::device_panel::Z;
use crate::device_window::Kit;
use crate::{PanelPath, PanelText, PanelZone};
use dstudio_core::Device;
use dstudio_core::builtin::DeviceDef;
use slint::{Color, ModelRc, VecModel};
use std::cell::RefCell;
use std::rc::Rc;

fn model<T: Clone + 'static>(v: Vec<T>) -> ModelRc<T> {
    ModelRc::from(Rc::new(VecModel::from(v)))
}

/// The picture's size in the card (reference points), and its inner margin.
pub(crate) const PIC_W: f32 = 332.0;
pub(crate) const PIC_H: f32 = 104.0;
const PIC_M: f32 = 6.0;
/// The model hit: sample rate, length (the longest front plus the longest tail, ms),
/// decay (ms), and where the log time axis turns (ms).
const MODEL_SR: f32 = 8000.0;
const T_MAX: f32 = 2100.0;
const DECAY_MS: f32 = 180.0;
const TAU: f32 = 6.0;
/// Silhouette release per plot point (ms).
const RELEASE_MS: f32 = 3.0;
/// Inks.
const INK_WAVE: f32 = 0.30;
const INK_CLIP: f32 = 0.11;
/// The scope's gain scale (dB at the well's edge).
const GAIN_DB: f32 = 12.0;

fn warp(ms: f32) -> f32 {
    (ms.max(0.0) / TAU).ln_1p() / (T_MAX / TAU).ln_1p()
}

/// One pole at `ms` for the model's rate (as the kernel's `dsp::pole`).
fn pole(ms: f32) -> f32 {
    if ms <= 0.0 { 0.0 } else { (-1000.0 / (ms * MODEL_SR)).exp() }
}

/// The model hit through the shaper, binned to `cols` columns on the log time axis:
/// each column's output peak (released like the scope's bars).
fn run_model(v: &dyn Fn(&str) -> f32, cols: usize) -> Vec<f32> {
    type Key = (usize, [u32; 6]);
    thread_local! {
        static CACHE: RefCell<Option<(Key, Vec<f32>)>> = const { RefCell::new(None) };
    }
    let ids = ["attack", "sustain", "sensitivity", "attack_time", "sustain_time", "mix"];
    let vals: [f32; 6] = ids.map(v);
    let key: Key = (cols, vals.map(f32::to_bits));
    if let Some(hit) = CACHE.with(|c| c.borrow().as_ref().filter(|(k, _)| *k == key).map(|(_, h)| h.clone())) {
        return hit;
    }
    let [attack, sustain, sens, att_ms, sus_ms, mix] = vals;
    let (attack, sustain, sens, mix) = (attack / 100.0, sustain / 100.0, sens / 100.0, mix / 100.0);
    let k = ((sens - 0.5) * 2.4).exp2();
    let floor = 10f32.powf((-50.0 - 40.0 * sens) / 20.0);
    let a_slow = pole(att_ms.clamp(0.5, 1000.0));
    let fall_sus = pole(if sus_ms >= 1999.0 { 10_000.0 } else { sus_ms.clamp(1.0, 10_000.0) });
    let (ka, ks) = (0.5 * k * attack, 0.6 * k * sustain);
    let (rise, fall, smooth) = (pole(0.3), pole(40.0), pole(0.5));
    let (mut env, mut slow, mut tail, mut gdb) = (floor, floor, floor, 0.0f32);
    let mut peak = vec![0.0f32; cols];
    let n = (T_MAX * 0.001 * MODEL_SR) as usize;
    let mut phase = 0.0f32;
    for i in 0..n {
        let ms = i as f32 * 1000.0 / MODEL_SR;
        let hz = 55.0 + 125.0 * (-ms / 22.0).exp();
        phase += std::f32::consts::TAU * hz / MODEL_SR;
        let s = (-ms / DECAY_MS).exp() * phase.sin();
        let x = s.abs().max(floor);
        env = x + if x > env { rise } else { fall } * (env - x);
        slow = if env < slow { env } else { env + a_slow * (slow - env) };
        tail = if env > tail { env } else { env + fall_sus * (tail - env) };
        let att = (6.020_6 * (env / slow).log2()).min(24.0);
        let sus = (6.020_6 * (tail / env).log2()).min(30.0);
        let target = (ka * att + ks * sus).clamp(-30.0, 24.0);
        gdb = target + smooth * (gdb - target);
        let y = s + mix * (s * 10f32.powf(gdb / 20.0) - s);
        let b = ((warp(ms) * cols as f32) as usize).min(cols - 1);
        peak[b] = peak[b].max(y.abs());
    }
    // The scope's silhouette: each column's peak, released over the time it spans.
    let unwarp = |u: f32| TAU * (u * (T_MAX / TAU).ln_1p()).exp_m1();
    let mut e = 0.0f32;
    for (x, p) in peak.iter_mut().enumerate() {
        let span = (unwarp((x + 1) as f32 / cols as f32) - unwarp(x as f32 / cols as f32)).max(1e-3);
        e = p.max(e * (-span / RELEASE_MS).exp());
        *p = e;
    }
    CACHE.with(|c| *c.borrow_mut() = Some((key, peak.clone())));
    peak
}

impl Controller {
    /// The window's scope: legend on the caption row, the well with the output's
    /// silhouette, the clipped ghost, the rest line and the applied gain.
    pub(crate) fn shaper_window_canvas(&self, d: &Device, def: &'static DeviceDef, k: &Kit, accent: Color) -> PanelCanvas {
        let (_, _, dw, dh) = k.display;
        let (l, t, r, b) = k.plot;
        let (pw, ph) = ((dw - l - r) * Z, (dh - t - b) * Z);
        let val = |id: &str| def.param(id).map_or(0.0, |p| self.panel_value(d, def, p));
        let clip = val("clip") >= 0.5;
        let mut paths: Vec<PanelPath> = Vec::new();
        let mut texts: Vec<PanelText> = Vec::new();

        // Legend: Output (grey block), Applied Gain (accent line), Clipped (ghost block).
        let base = -12.0 * Z;
        let (sw, sh, sg, eg) = (12.0 * Z, 8.0 * Z, 6.0 * Z, 18.0 * Z);
        let mut x = 2.0 * Z;
        for (kind, label) in [(0, "Output"), (1, "Applied Gain"), (2, "Clipped")] {
            let sy = base - sh / 2.0;
            let block = format!("M{x:.1} {sy:.1} h{sw:.1} v{sh:.1} h{:.1} Z", -sw);
            match kind {
                0 => paths.push(fill(block, white(INK_WAVE))),
                1 => paths.push(stroke(format!("M{x:.1} {base:.1} h{sw:.1}"), accent, 2.0 * Z)),
                _ => {
                    paths.push(fill(block.clone(), white(INK_CLIP)));
                    paths.push(stroke(block, white(0.22), 1.0));
                }
            }
            texts.push(PanelText { weight: 600, ..text(x + sw + sg, base, 120.0 * Z, label, white(0.7), 12.0 * Z, 0) });
            x += sw + sg + crate::text_measure::value_width(label) * 1.2 * Z + eg;
        }

        // The well.
        let rr = 4.0 * Z;
        let well = format!(
            "M{rr:.1} 0 H{:.1} Q{pw:.1} 0 {pw:.1} {rr:.1} V{:.1} Q{pw:.1} {ph:.1} {:.1} {ph:.1} H{rr:.1} Q0 {ph:.1} 0 {:.1} V{rr:.1} Q0 0 {rr:.1} 0 Z",
            pw - rr,
            ph - rr,
            pw - rr,
            ph - rr
        );
        paths.push(fill(well.clone(), white(0.025)));
        let mid = ph / 2.0;
        let reach = mid - Z;
        let r_of = |peak: f32| (peak.abs().min(1.41) / 1.41).sqrt() * reach;
        let y_of = |db: f32| mid - db.clamp(-GAIN_DB, GAIN_DB) / GAIN_DB * (reach - 4.0 * Z);
        let bars = (pw / Z).floor().max(1.0);
        let px = self.devices.panel.scopes.get(&d.id).map(|h| h.pixels(bars, 2.0));
        if let Some(px) = px.as_ref().filter(|p| !p.out.is_empty()) {
            let n = px.out.len();
            let x_of = |i: usize| ((i as f32 - 1.0 + px.offset) * Z).clamp(0.0, pw);
            let band = |outer: &dyn Fn(usize) -> f32, inner: &dyn Fn(usize) -> f32| {
                let mut pts: Vec<(f32, f32)> = (0..n).map(|i| (x_of(i), mid - outer(i))).collect();
                pts.extend((0..n).rev().map(|i| (x_of(i), mid - inner(i))));
                format!("{} Z", path(&pts))
            };
            let out_r = |i: usize| r_of(px.out[i]);
            paths.push(fill(band(&out_r, &|i| -out_r(i)), white(INK_WAVE)));
            if clip && (0..n).any(|i| px.pre[i] > px.out[i] * 1.005) {
                let pre_r = |i: usize| if px.pre[i] > px.out[i] * 1.005 { r_of(px.pre[i]) } else { out_r(i) };
                paths.push(fill(format!("{} {}", band(&pre_r, &out_r), band(&|i| -out_r(i), &|i| -pre_r(i))), white(INK_CLIP)));
            }
            let zy = mid.round() + 0.5;
            paths.push(stroke(dashed(&[(0.0, zy), (pw, zy)], 3.0 * Z, 4.0 * Z), white(0.14), 1.0));
            let mut gain: Vec<(f32, f32)> = (0..n).map(|i| (x_of(i), y_of(px.gain[i]))).collect();
            gain.push((pw, y_of(px.gain[n - 1])));
            paths.push(stroke(path(&gain), accent, 2.0 * Z));
        } else {
            let zy = mid.round() + 0.5;
            paths.push(stroke(dashed(&[(0.0, zy), (pw, zy)], 3.0 * Z, 4.0 * Z), white(0.14), 1.0));
            paths.push(stroke(format!("M0 {mid:.1} L{pw:.1} {mid:.1}"), accent, 2.0 * Z));
        }
        paths.push(stroke(well.clone(), white(0.06), 1.0));
        if self.win_held(d.id) {
            paths.push(stroke(well, rgba(accent, 0.8), 1.0));
            texts.push(text(pw - 68.0 * Z, 12.0 * Z, 60.0 * Z, "Held", accent, 9.0 * Z, 2));
        }
        PanelCanvas {
            visible: true,
            w: pw,
            h: ph,
            paths: model(paths),
            texts: model(texts),
            label: "Applied Gain".into(),
            ..Default::default()
        }
    }

    /// The shape picture: the model hit's output, the gain curve these settings ask for,
    /// and the Attack and Sustain handles, each a drag zone for its amount (up) and time
    /// (across).
    pub(crate) fn shaper_picture(&self, d: &Device, def: &'static DeviceDef, accent: Color) -> PanelCanvas {
        let val = |id: &str| def.param(id).map_or(0.0, |p| self.panel_value(d, def, p));
        let (w, h) = (PIC_W * Z, PIC_H * Z);
        let (l, top) = (PIC_M * Z, PIC_M * Z);
        let (iw, ih) = (w - 2.0 * l, h - 2.0 * top);
        let mid = top + ih / 2.0;
        let half = ih / 2.0 - Z;
        let cols = iw.round().max(2.0) as usize;
        let x_at = |ms: f32| l + warp(ms) * iw;
        let y_amp = |a: f32| mid - a.signum() * (a.abs() / 4.0).sqrt().min(1.0) * half;
        let y_amount = |pct: f32| mid - (pct / 100.0).clamp(-1.0, 1.0) * half;
        let mut paths: Vec<PanelPath> = Vec::new();
        let my = mid.round() + 0.5;
        paths.push(stroke(format!("M{l:.1} {my:.1} L{:.1} {my:.1}", l + iw), white(0.08), 1.0));

        let peak = run_model(&val, cols);
        let col_x = |i: usize| l + i as f32 * iw / (cols - 1) as f32;
        let mut band: Vec<(f32, f32)> = peak.iter().enumerate().map(|(i, p)| (col_x(i), y_amp(*p))).collect();
        band.extend(peak.iter().enumerate().rev().map(|(i, p)| (col_x(i), y_amp(-*p))));
        paths.push(fill(format!("{} Z", path(&band)), white(INK_WAVE)));

        // The curve: Attack's amount through the front, handing over to Sustain's, which
        // falls back to nothing over Sustain Time (or holds, at Full).
        let (att, sus) = (val("attack"), val("sustain"));
        let att_ms = val("attack_time");
        let sus_ms = val("sustain_time");
        let full = sus_ms >= 1999.0;
        let hand_off = (att_ms * 0.5).max(1.0);
        let gain_at = |ms: f32| {
            if ms <= att_ms {
                return att;
            }
            let after = ms - att_ms;
            let w = (-after / hand_off).exp();
            let tail = if full { sus } else { sus * (-after / sus_ms).exp() };
            att * w + tail * (1.0 - w)
        };
        let unwarp = |u: f32| TAU * (u * (T_MAX / TAU).ln_1p()).exp_m1();
        let curve: Vec<(f32, f32)> =
            (0..=cols).map(|i| i as f32 / cols as f32).map(|u| (l + u * iw, y_amount(gain_at(unwarp(u))))).collect();
        paths.push(stroke(path(&curve), accent, 2.0 * Z));

        // The handles, and their drag zones.
        let sus_end = if full { T_MAX } else { att_ms + sus_ms };
        let handles = [("attack", "attack_time", x_at(att_ms), y_amount(att)), ("sustain", "sustain_time", x_at(sus_end), y_amount(sus))];
        let mut zones = Vec::new();
        let r = 4.5 * Z;
        for (amount, time, hx, hy) in handles {
            let dot = format!("M{:.2} {hy:.2} a{r:.2} {r:.2} 0 1 0 {:.2} 0 a{r:.2} {r:.2} 0 1 0 {:.2} 0 Z", hx - r, 2.0 * r, -2.0 * r);
            paths.push(PanelPath { width: 2.0 * Z, stroke: accent, ..fill(dot, white(0.92)) });
            let hit = 12.0 * Z;
            zones.push(PanelZone {
                x: hx - hit,
                y: hy - hit,
                w: 2.0 * hit,
                h: 2.0 * hit,
                param_x: def.index_of(time).map_or(-1, |i| i as i32),
                param_y: def.index_of(amount).map_or(-1, |i| i as i32),
                rate_x: 1.0 / iw.max(1.0),
                rate_y: 1.0 / (2.0 * half).max(1.0),
            });
        }
        PanelCanvas { visible: true, w, h, paths: model(paths), zones: model(zones), ..Default::default() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Attack lifts the front of the hit and leaves the tail; Sustain does the opposite.
    #[test]
    fn model_hit_follows_attack_and_sustain() {
        let at = |attack: f32, sustain: f32| {
            move |id: &str| match id {
                "attack" => attack,
                "sustain" => sustain,
                "sensitivity" => 50.0,
                "attack_time" => 30.0,
                "sustain_time" => 2000.0,
                _ => 100.0,
            }
        };
        let flat = run_model(&at(0.0, 0.0), 300);
        let punch = run_model(&at(100.0, 0.0), 300);
        let long = run_model(&at(0.0, 100.0), 300);
        let front = (warp(5.0) * 300.0) as usize;
        let tail = (warp(600.0) * 300.0) as usize;
        assert!(punch[front] > flat[front] * 1.2, "{} {}", punch[front], flat[front]);
        assert!((punch[tail] - flat[tail]).abs() < flat[tail] * 0.3);
        assert!(long[tail] > flat[tail] * 1.2, "{} {}", long[tail], flat[tail]);
    }
}
