//! Mixer panel view-model helpers: per-channel meter ballistics on top of the
//! engine's non-destructive `peek_meters` window, and value formatting.
//! Every mixer edit goes through the same coalesced `Command`s as the track
//! headers (see `Controller::set_track_gain` / `set_master_gain`).

use dstudio_core::{db_to_gain, gain_to_db};
use dstudio_engine::Meter;
use std::collections::HashMap;
use std::time::Instant;

/// Engine meter id of the master bus.
pub const MASTER_METER_ID: u64 = 0;

/// How long a meter stays red after its last peak at or above 0 dBFS.
pub const CLIP_HOLD_S: f32 = 1.5;

#[derive(Default)]
pub struct Ballistics {
    /// Displayed (l, r) linear peaks per meter id, released at 24 dB/s.
    disp: HashMap<u64, (f32, f32)>,
    /// Peak hold since the last reset, per meter id.
    hold: HashMap<u64, f32>,
    /// Seconds since the last peak at or above 0 dBFS, per meter id that has clipped.
    clip_age: HashMap<u64, f32>,
    t: Option<Instant>,
}

impl Ballistics {
    /// Feed one frame of engine readouts. Returns true while any meter is non-silent or red.
    pub fn update(&mut self, meters: &[Meter]) -> bool {
        self.update_at(Instant::now(), meters)
    }

    pub(crate) fn update_at(&mut self, now: Instant, meters: &[Meter]) -> bool {
        let dt = self.t.map(|t| (now - t).as_secs_f32()).unwrap_or(0.0).min(0.25);
        self.t = Some(now);
        let fall = db_to_gain(-24.0 * dt);
        for v in self.disp.values_mut() {
            v.0 *= fall;
            v.1 *= fall;
        }
        for a in self.clip_age.values_mut() {
            *a += dt;
        }
        for m in meters {
            let e = self.disp.entry(m.track_id).or_insert((0.0, 0.0));
            e.0 = e.0.max(m.peak_l);
            e.1 = e.1.max(m.peak_r);
            let h = self.hold.entry(m.track_id).or_insert(0.0);
            *h = h.max(m.peak_l.max(m.peak_r));
            if m.peak_l.max(m.peak_r) >= 1.0 {
                self.clip_age.insert(m.track_id, 0.0);
            }
        }
        self.clip_age.retain(|_, a| *a < CLIP_HOLD_S);
        let mut active = !self.clip_age.is_empty();
        for v in self.disp.values_mut() {
            if v.0 < 1e-5 {
                v.0 = 0.0;
            }
            if v.1 < 1e-5 {
                v.1 = 0.0;
            }
            active |= v.0 > 0.0 || v.1 > 0.0;
        }
        active
    }

    /// (left, right, peak hold), linear.
    pub fn get(&self, id: u64) -> (f32, f32, f32) {
        let (l, r) = self.disp.get(&id).copied().unwrap_or((0.0, 0.0));
        (l, r, self.hold.get(&id).copied().unwrap_or(0.0))
    }

    /// True while the meter reached 0 dBFS within the last `CLIP_HOLD_S`.
    pub fn clipping(&self, id: u64) -> bool {
        self.clip_age.contains_key(&id)
    }

    pub fn reset_hold(&mut self) {
        self.hold.clear();
        self.clip_age.clear();
    }
}

/// Pan readout next to the strip knob: -50.0 (L) .. 50.0 (R), "0.0" at centre.
pub fn fmt_strip_pan(pan: f32) -> String {
    let v = (pan * 50.0 * 10.0).round() / 10.0;
    if v == 0.0 { "0.0".into() } else { format!("{v:.1}").replace('-', "−") }
}

/// Gain readout above a strip fader: one decimal with an explicit sign ("+1.5", "−12.0",
/// "0.0"), and "−∞" at or below -60 dB, where the fader taper bottoms out.
pub fn fmt_gain(db: f32) -> String {
    if !db.is_finite() || db <= -60.0 {
        return "−∞".into();
    }
    let v = (db * 10.0).round() / 10.0;
    if v > 0.0 {
        format!("+{v:.1}")
    } else if v < 0.0 {
        format!("−{:.1}", -v)
    } else {
        "0.0".into()
    }
}

/// Peak-hold readout ("−∞" when silent), one decimal.
pub fn fmt_peak(g: f32) -> String {
    fmt_gain(gain_to_db(g))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ballistics_hold_and_release() {
        let mut b = Ballistics::default();
        assert!(b.update(&[Meter { track_id: 3, peak_l: 0.5, peak_r: 0.25 }]));
        assert_eq!(b.get(3), (0.5, 0.25, 0.5));
        b.reset_hold();
        assert_eq!(b.get(3).2, 0.0);
        assert_eq!(b.get(99), (0.0, 0.0, 0.0));
    }
    #[test]
    fn clip_clears_once_the_signal_stops_clipping() {
        let mut b = Ballistics::default();
        let t0 = Instant::now();
        let at = |s: f32| t0 + std::time::Duration::from_secs_f32(s);
        let over = [Meter { track_id: 3, peak_l: 1.2, peak_r: 0.5 }];
        let under = [Meter { track_id: 3, peak_l: 0.5, peak_r: 0.5 }];
        b.update_at(at(0.0), &over);
        assert!(b.clipping(3));
        let mut s = 0.0;
        while s < 5.0 {
            s += 0.1;
            b.update_at(at(s), &over);
        }
        assert!(b.clipping(3), "a track that keeps clipping stays red");
        b.update_at(at(s + 0.1), &under);
        assert!(b.clipping(3), "held briefly after the last over");
        let end = s + CLIP_HOLD_S + 0.2;
        while s < end {
            s += 0.1;
            assert!(b.update_at(at(s), &under) || !b.clipping(3));
        }
        assert!(!b.clipping(3), "no longer clipping: no longer red");
        assert_eq!(b.get(3).2, 1.2, "the peak readout still latches until clicked");
        b.update_at(at(s + 0.1), &over);
        b.reset_hold();
        assert!(!b.clipping(3), "clicking the peak readout clears the light too");
    }
    #[test]
    fn formatting() {
        assert_eq!(fmt_strip_pan(0.0), "0.0");
        assert_eq!(fmt_strip_pan(-0.5), "−25.0");
        assert_eq!(fmt_peak(0.0), "−∞");
        assert_eq!(fmt_peak(1.0), "0.0");
        assert_eq!(fmt_gain(0.0), "0.0");
        assert_eq!(fmt_gain(1.46), "+1.5");
        assert_eq!(fmt_gain(-12.0), "−12.0");
        assert_eq!(fmt_gain(-60.0), "−∞");
    }
}
