//! Card views of built-in devices whose body is a `View::Custom`: today the
//! Multiband Compressor's band field.
//! The field shows the three bands between the crossovers on a
//! log frequency axis (20 Hz..20 kHz) against a 0..−80 dB ladder: each band's
//! live level and gain change, its threshold (dashed) and floor (dotted)
//! lines as drag handles, and the crossover lines as horizontal handles. A
//! band selector in the picker row (also set by clicking a band) picks which
//! band the dial row edits.
//!
//! Geometry is the 100%-zoom card measurements, scaled by [Z].

use crate::controller::Controller;
use crate::device_panel::{VIEW_SELECT, Z};
use crate::{PanelBand, PanelBands, PanelFreq, PanelPick, PanelTick, PanelXover};
use dstudio_core::builtin::{DeviceDef, MULTIBAND_BANDS, Transfer, View, multiband_gain};
use dstudio_core::{Device, DeviceId};
use slint::{ModelRc, SharedString, VecModel};
use std::rc::Rc;
use std::time::Instant;

const MULTIBAND: &str = "multiband";
/// Field box (card relative), dB ladder width and plot range.
const FIELD_X: f32 = 8.0;
const FIELD_Y: f32 = 63.0;
const FIELD_H: f32 = 146.5;
/// Gap between the field and the slider column.
const FIELD_GAP: f32 = 24.0;
const LADDER_W: f32 = 28.0;
const TOP_DB: f32 = 0.0;
const FLOOR_DB: f32 = -80.0;
const F_LO: f32 = 20.0;
const F_HI: f32 = 20000.0;
/// Band meters: levels floor here; rise / fall time constants (s).
const LEVEL_FLOOR: f32 = -100.0;
const RISE: f32 = 0.030;
const FALL: f32 = 0.300;
const BAND_NAMES: [&str; 3] = ["Low", "Mid", "High"];
const MINUS: char = '\u{2212}';

/// Panel state of one custom-view card: the selected band and the smoothed
/// band meters (levels and gain changes, dB).
#[derive(Debug, Clone)]
pub(crate) struct ViewState {
    pub band: usize,
    level: [f32; 3],
    gain: [f32; 3],
    last: Option<Instant>,
}

impl Default for ViewState {
    fn default() -> Self {
        ViewState { band: 0, level: [LEVEL_FLOOR; 3], gain: [0.0; 3], last: None }
    }
}

impl ViewState {
    /// The smoothed band meters: levels and gain changes (dB).
    pub(crate) fn meters(&self) -> ([f32; 3], [f32; 3]) {
        (self.level, self.gain)
    }

    /// Fold in engine readouts (in, out, 3 band levels, 3 band gains): rising
    /// values follow in 30 ms, falling ones in 300 ms. True when the meters moved.
    pub fn poll(&mut self, r: &[f32]) -> bool {
        let now = Instant::now();
        let dt = self.last.map_or(1.0 / 60.0, |t| now.duration_since(t).as_secs_f32()).min(0.25);
        self.last = Some(now);
        let step = |cur: f32, target: f32, growing: bool| {
            let tau = if growing { RISE } else { FALL };
            target + (cur - target) * (-dt / tau).exp()
        };
        let mut moved = false;
        for b in 0..3 {
            let lv = r.get(2 + b).copied().unwrap_or(LEVEL_FLOOR).max(LEVEL_FLOOR);
            let g = r.get(5 + b).copied().unwrap_or(0.0).clamp(-40.0, 36.0);
            let nl = step(self.level[b], lv, lv > self.level[b]);
            let ng = step(self.gain[b], g, g.abs() > self.gain[b].abs());
            moved |= (nl - self.level[b]).abs() > 0.05 || (ng - self.gain[b]).abs() > 0.05;
            self.level[b] = nl;
            self.gain[b] = ng;
        }
        moved
    }
}

fn s(v: impl Into<SharedString>) -> SharedString {
    v.into()
}

fn model<T: Clone + 'static>(v: Vec<T>) -> ModelRc<T> {
    ModelRc::from(Rc::new(VecModel::from(v)))
}

fn is_multiband(def: &DeviceDef) -> bool {
    def.layout.view == Some(View::Custom(MULTIBAND))
}

/// Dashed horizontal segments from x0 to x1 at y.
fn dashes(x0: f32, x1: f32, y: f32, dash: f32, gap: f32) -> String {
    let mut out = String::new();
    let mut x = x0;
    while x < x1 {
        out.push_str(&format!("M{x:.2} {y:.2} L{:.2} {y:.2} ", (x + dash).min(x1)));
        x += dash + gap;
    }
    out
}

/// Field geometry for a card (scaled px, field relative).
struct Geo {
    w: f32,
    h: f32,
    ladder: f32,
}

impl Geo {
    fn new(def: &DeviceDef) -> Geo {
        let right = def.layout.width() - FIELD_X - 62.0 * def.layout.sliders.len() as f32 - FIELD_GAP;
        Geo { w: (right - FIELD_X) * Z, h: FIELD_H * Z, ladder: LADDER_W * Z }
    }
    fn x_of(&self, hz: f32) -> f32 {
        (hz.clamp(F_LO, F_HI) / F_LO).ln() / (F_HI / F_LO).ln() * self.w
    }
    fn y_of(&self, db: f32) -> f32 {
        ((TOP_DB - db) / (TOP_DB - FLOOR_DB)).clamp(0.0, 1.0) * self.h
    }
}

fn db_label(db: f32) -> String {
    if db == 0.0 { "0".into() } else { format!("{MINUS}{}", db.abs() as i32) }
}

impl Controller {
    fn view_state(&self, id: DeviceId) -> ViewState {
        self.devices.panel.views.get(&id).cloned().unwrap_or_default()
    }

    /// The picker-row control of a custom view (the band selector).
    pub(crate) fn view_picker(&self, d: &Device, def: &DeviceDef) -> Option<PanelPick> {
        if !is_multiband(def) {
            return None;
        }
        thread_local! {
            static NAMES: ModelRc<SharedString> = ModelRc::from(Rc::new(VecModel::from(BAND_NAMES.iter().map(|n| SharedString::from(*n)).collect::<Vec<_>>())));
        }
        Some(PanelPick {
            kind: 1,
            param: VIEW_SELECT,
            label: s("Band"),
            choices: NAMES.with(|n| n.clone()),
            selected: self.view_state(d.id).band as i32,
            enabled: true,
            width: crate::device_panel::segment_width(&BAND_NAMES) * crate::device_panel::Z,
            ..Default::default()
        })
    }

    /// The parameter a dial-row slot edits: the multiband card's knobs are
    /// defined with the Low band and follow the selected band.
    pub(crate) fn view_bind(&self, d: &Device, def: &DeviceDef, slot: &'static str) -> &'static str {
        if !is_multiband(def) {
            return slot;
        }
        let band = self.view_state(d.id).band.min(2);
        MULTIBAND_BANDS[0].iter().position(|id| *id == slot).map_or(slot, |k| MULTIBAND_BANDS[band][k])
    }

    /// Transfer face of the selected band's law (Ratio knob of the multiband card).
    pub(crate) fn view_transfer(&self, live: &Device, def: &DeviceDef) -> Option<Transfer> {
        if !is_multiband(def) {
            return None;
        }
        let v = |id: &str| def.param(id).map_or(0.0, |p| def.value(live, p));
        let ids = MULTIBAND_BANDS[self.view_state(live.id).band.min(2)];
        let (thr, ratio, floor, lift) = (v(ids[0]), v(ids[1]), v(ids[6]), v(ids[7]));
        let (down, up, depth) = (v("downward"), v("upward"), v("depth"));
        let (lo, hi) = (FLOOR_DB, TOP_DB);
        let n = 64;
        let points = (0..=n)
            .map(|i| {
                let x = lo + (hi - lo) * i as f32 / n as f32;
                (x, x + multiband_gain(x, thr, ratio, floor, lift, down, up, depth))
            })
            .collect();
        let m = 0.25 + 3.0 / 26.0 * (hi - lo);
        Some(Transfer { lo, hi, points, bend: (thr - m, thr + m) })
    }

    /// The multiband field (an empty, invisible field for other devices).
    pub(crate) fn view_field(&self, d: &Device, def: &DeviceDef, live: &Device) -> PanelBands {
        if !is_multiband(def) {
            return PanelBands::default();
        }
        let g = Geo::new(def);
        let st = self.view_state(d.id);
        let v = |id: &str| def.param(id).map_or(0.0, |p| def.value(live, p));
        let idx = |id: &str| def.index_of(id).map_or(-1, |i| i as i32);
        let (f1, f2) = (v("low_crossover"), v("high_crossover"));
        let edges = [g.ladder, g.x_of(f1).max(g.ladder), g.x_of(f2).max(g.ladder), g.w];
        let (down, up, depth) = (v("downward"), v("upward"), v("depth"));
        let bands: Vec<PanelBand> = (0..3)
            .map(|b| {
                let ids = MULTIBAND_BANDS[b];
                let (x0, x1) = (edges[b], edges[b + 1].max(edges[b]));
                let (thr, floor) = (v(ids[0]), v(ids[6]));
                let (ty, fy) = (g.y_of(thr), g.y_of(floor));
                // Handle strips: 12 px around each line, split midway when the lines are closer
                // (Floor can sit above Threshold, so the split follows their actual order).
                let half = 6.0 * Z;
                let mid = 0.5 * (ty + fy);
                let (t0, t1, f0, f1h) = if (fy - ty).abs() >= 2.0 * half {
                    (ty - half, ty + half, fy - half, fy + half)
                } else if fy >= ty {
                    (ty - half, mid, mid, fy + half)
                } else {
                    (mid, ty + half, fy - half, mid)
                };
                let (t0, t1, f0, f1h) = (t0.max(0.0), t1.min(g.h), f0.max(0.0), f1h.min(g.h));
                let level = st.level[b];
                let gain = st.gain[b];
                let ly = g.y_of(level);
                let gy = g.y_of((level + gain).clamp(FLOOR_DB - 40.0, 36.0));
                let inset = 6.0 * Z;
                let hx = x0 + inset;
                let hw = (x1 - x0 - 2.0 * inset).max(2.0);
                let p_thr = def.param(ids[0]);
                let p_floor = def.param(ids[6]);
                PanelBand {
                    x: x0,
                    w: x1 - x0,
                    selected: b == st.band,
                    level_y: ly,
                    show_level: level > FLOOR_DB,
                    gain_y: ly.min(gy),
                    gain_h: (ly - gy).abs(),
                    show_gain: level > FLOOR_DB && gain.abs() > 0.25,
                    thr_y: ty,
                    floor_y: fy,
                    thr_param: idx(ids[0]),
                    floor_param: idx(ids[6]),
                    thr_active: v(ids[1]) * down * depth > 0.0 && v(ids[1]) > 1.0,
                    floor_active: v(ids[7]) > 1.0 && up * depth > 0.0,
                    thr_dash: s(dashes(x0, x1, ty, 5.0 * Z, 4.0 * Z)),
                    floor_dash: s(dashes(x0, x1, fy, 1.5 * Z, 3.0 * Z)),
                    thr_text: s(p_thr.map(|p| p.format(thr)).unwrap_or_default()),
                    floor_text: s(p_floor.map(|p| p.format(floor)).unwrap_or_default()),
                    handle_x: hx,
                    handle_w: hw,
                    thr_hit_y: t0,
                    thr_hit_h: (t1 - t0).max(2.0),
                    floor_hit_y: f0,
                    floor_hit_h: (f1h - f0).max(2.0),
                }
            })
            .collect();
        let xovers: Vec<PanelXover> = [("low_crossover", f1), ("high_crossover", f2)]
            .iter()
            .map(|(id, f)| {
                let p = def.param(id);
                // Norm per px: the axis is log 20 Hz..20 kHz, the parameter log over its own range.
                let rate = p.map_or(0.0, |p| (F_HI / F_LO).ln() / g.w / (p.max / p.min).ln());
                PanelXover {
                    x: edges[if *id == "low_crossover" { 1 } else { 2 }],
                    param: idx(id),
                    text: s(p.map(|p| p.format(*f)).unwrap_or_default()),
                    rate,
                }
            })
            .collect();
        let ticks: Vec<PanelTick> =
            [-20.0f32, -40.0, -60.0].iter().map(|db| PanelTick { y: g.y_of(*db), text: s(db_label(*db)), rule: true }).collect();
        let mut freqs = vec![PanelFreq { x: 0.0, text: s("Hz"), grid: false }];
        for (hz, label) in [(100.0, "100"), (1000.0, "1k"), (10000.0, "10k")] {
            freqs.push(PanelFreq { x: g.x_of(hz), text: s(label), grid: true });
        }
        let span = TOP_DB - FLOOR_DB;
        PanelBands {
            visible: true,
            x: FIELD_X * Z,
            y: FIELD_Y * Z,
            w: g.w,
            h: g.h,
            ladder: g.ladder,
            ticks: model(ticks),
            freqs: model(freqs),
            bands: model(bands),
            xovers: model(xovers),
            thr_rate: span / g.h / 60.0,
            floor_rate: span / g.h / 80.0,
        }
    }

    /// Custom view actions: 0 = select the band under field x (px), 1 = select band `value`,
    /// 2 = show card view `value` (picker view switch / sampler tabs).
    pub fn panel_view(&mut self, device: DeviceId, action: i32, value: f32) {
        if action == 2 {
            return self.card_view_select(device, value as i32);
        }
        let Some(d) = self.project().device(device) else { return };
        let Some(def) = dstudio_core::builtin::def_of(d) else { return };
        if !is_multiband(def) {
            return;
        }
        let band = match action {
            0 => {
                let g = Geo::new(def);
                let v = |id: &str| def.param(id).map_or(0.0, |p| def.value(d, p));
                if value < g.ladder {
                    return;
                }
                if value < g.x_of(v("low_crossover")) {
                    0
                } else if value < g.x_of(v("high_crossover")) {
                    1
                } else {
                    2
                }
            }
            _ => (value.round().max(0.0) as usize).min(2),
        };
        let st = self.devices.panel.views.entry(device).or_default();
        if st.band != band {
            st.band = band;
            self.ui_dirty = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn field_axes_follow_the_reference_capture() {
        let def = dstudio_core::builtin::find("multiband-compressor").unwrap();
        let g = Geo::new(def);
        // Field spans card x 8..449 (crossover lines at 122 and 316 pt).
        assert!((g.w / Z - 441.0).abs() < 0.01);
        assert!((FIELD_X + g.x_of(120.0) / Z - 122.0).abs() < 1.5);
        assert!((FIELD_X + g.x_of(2500.0) / Z - 316.0).abs() < 1.5);
        // dB rungs: -20 at card y 100, -60 at 173.
        assert!((FIELD_Y + g.y_of(-20.0) / Z - 99.6).abs() < 0.5);
        assert!((FIELD_Y + g.y_of(-60.0) / Z - 172.9).abs() < 0.5);
    }

    #[test]
    fn meters_rise_fast_and_fall_slowly() {
        let mut st = ViewState { last: Some(Instant::now() - std::time::Duration::from_millis(30)), ..Default::default() };
        assert!(st.poll(&[0.0, 0.0, -10.0, -100.0, -100.0, -6.0, 0.0, 0.0]));
        assert!(st.level[0] > -50.0, "one time constant up: {}", st.level[0]);
        assert!(st.gain[0] < -3.0);
        let peak = st.level[0];
        st.last = Some(Instant::now() - std::time::Duration::from_millis(30));
        st.poll(&[0.0, 0.0, -100.0, -100.0, -100.0, 0.0, 0.0, 0.0]);
        assert!(st.level[0] > peak - 6.0, "falls slowly: {peak} -> {}", st.level[0]);
    }
}
