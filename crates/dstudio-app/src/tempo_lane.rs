//! Tempo changes in the app (spec 015), beside the lane itself (src/automation.rs shows the map as
//! the master row's "Song › Tempo" lane): the marks on the ruler, the point menu that makes a point
//! ramp or hold, and the tempo at the playhead that the transport field and tempo-dependent
//! readouts show.

use crate::controller::{Controller, MenuAction};
use dstudio_core::{AutomationTarget, Command, PPQ, Ticks};

/// The ruler's top band (bar numbers): tempo marks sit in it, after a bar number they share a
/// place with.
pub const MARK_BAND: f32 = 16.5;
/// Width of one character of a ruler label (7.75 px semibold digits), for placing and hitting marks.
const CHAR_W: f32 = 4.4;

/// One tempo point on the ruler (lane-local points).
#[derive(Debug, Clone, PartialEq)]
pub struct TempoMark {
    /// The point's position (the mark).
    pub x: f32,
    /// Where its text starts, and the text: the BPM, with an arrow when it ramps to the next point.
    pub text_x: f32,
    pub text: String,
}

impl TempoMark {
    pub fn right(&self) -> f32 {
        self.text_x + self.text.chars().count() as f32 * CHAR_W
    }
}

/// A mark's text: "128.00", or "↗ 128.00" / "↘ 128.00" for a ramp to a faster / slower point.
pub fn mark_text(bpm: f64, ramp_to: Option<f64>) -> String {
    match ramp_to {
        Some(n) if n > bpm => format!("↗ {bpm:.2}"),
        Some(n) if n < bpm => format!("↘ {bpm:.2}"),
        _ => format!("{bpm:.2}"),
    }
}

impl Controller {
    /// Tempo at the playhead (the song tempo without tempo changes).
    pub fn bpm_now(&self) -> f64 {
        let p = self.project();
        if p.has_tempo_changes() { p.bpm_at(self.playhead_ticks()) } else { p.transport.bpm }
    }

    /// Where a transport-field tempo edit applies: the playhead when the song has tempo changes
    /// (`SetTempo` sets the point in effect there), else the song tempo.
    pub(crate) fn tempo_edit_at(&self) -> Option<Ticks> {
        self.project().has_tempo_changes().then(|| self.playhead_ticks())
    }

    /// The ruler's tempo marks in view: one per point, whether lanes are shown or not.
    pub fn tempo_marks(&self) -> Vec<TempoMark> {
        let p = self.project();
        let pts = &p.transport.tempo;
        if pts.is_empty() {
            return Vec::new();
        }
        let (v, w) = (&self.view, self.lanes_size.0);
        let label_bars = self.ruler_steps().label_bars.max(1);
        let bar_ticks = p.ticks_per_bar().max(1);
        pts.iter()
            .enumerate()
            .filter_map(|(i, q)| {
                let x = v.x_of(q.time as f64 / PPQ as f64);
                if !(-80.0..=w).contains(&x) {
                    return None;
                }
                let ramp_to = pts.get(i + 1).filter(|_| q.ramp).map(|n| n.bpm);
                // On a numbered bar line the bar number comes first.
                let bar = q.time / bar_ticks;
                let numbered = q.time % bar_ticks == 0 && bar % label_bars == 0;
                let shift = if numbered { 4.5 + CHAR_W * (bar + 1).to_string().len() as f32 + 4.0 } else { 3.0 };
                Some(TempoMark { x, text_x: x + shift, text: mark_text(q.bpm, ramp_to) })
            })
            .collect()
    }

    /// A click in the ruler's top band on a tempo mark shows the lanes with "Song › Tempo".
    /// Returns false (the ruler seeks as usual) anywhere else.
    pub fn tempo_mark_click(&mut self, x: f32, y: f32) -> bool {
        if y >= MARK_BAND || !self.tempo_marks().iter().any(|m| x >= m.x - 3.0 && x <= m.right()) {
            return false;
        }
        self.auto_choose(None, AutomationTarget::Tempo);
        true
    }

    /// Right-click on a tempo point (lanes shown, master row on Song › Tempo): its menu, with
    /// "Ramp to Next Point" or "Hold Until Next Point". False when the click is not on one.
    pub fn tempo_point_context(&mut self, x: f32, y: f32, wx: f32, wy: f32, win: (f32, f32)) -> bool {
        let Some(i) = self.auto_point_at(None, &AutomationTarget::Tempo, x, y) else { return false };
        let pts = &self.project().transport.tempo;
        let has_next = i + 1 < pts.len();
        let action = if pts.get(i).is_some_and(|q| q.ramp) { MenuAction::TempoHold } else { MenuAction::TempoRamp };
        self.auto.sel = Some((None, AutomationTarget::Tempo, vec![i]));
        self.open_menu(vec![(action, has_next || action == MenuAction::TempoHold)], wx, wy, win);
        self.menu.tempo_point = Some(i);
        true
    }

    /// Point `i` ramps to the next point (or holds until it): one "Edit Tempo Automation" step.
    pub fn tempo_set_ramp(&mut self, i: usize, ramp: bool) -> bool {
        let mut points = self.project().transport.tempo.clone();
        match points.get_mut(i) {
            Some(q) if q.ramp != ramp => q.ramp = ramp,
            _ => return false,
        }
        self.execute(Command::SetTempoMap { points, bpm: None })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mark_texts_show_the_ramp_direction() {
        assert_eq!(mark_text(128.0, None), "128.00");
        assert_eq!(mark_text(90.0, Some(150.0)), "↗ 90.00");
        assert_eq!(mark_text(150.0, Some(90.0)), "↘ 150.00");
        assert_eq!(mark_text(150.0, Some(150.0)), "150.00");
    }

    #[test]
    fn the_lane_scale_fits_the_points_and_keeps_a_minimum_span() {
        use crate::automation::{TEMPO_MIN_SPAN, tempo_scale_for};
        let (lo, hi) = tempo_scale_for([120.0f32].into_iter());
        assert!((hi - lo - TEMPO_MIN_SPAN).abs() < 1e-3 && lo < 120.0 && hi > 120.0);
        let (lo, hi) = tempo_scale_for([90.0f32, 150.0].into_iter());
        assert!(lo < 90.0 && hi > 150.0 && hi - lo < 90.0, "headroom: {lo}..{hi}");
        let (lo, hi) = tempo_scale_for([20.0f32, 25.0].into_iter());
        assert!(lo >= 20.0 && hi - lo >= TEMPO_MIN_SPAN - 1e-3, "inside the tempo range: {lo}..{hi}");
    }

    #[test]
    fn a_first_point_keeps_the_song_tempo_before_it() {
        use crate::automation::{TEMPO_RAMP, tempo_points};
        use dstudio_core::{AutoPoint, AutomationLane};
        let mut lane = AutomationLane::new(AutomationTarget::Tempo);
        lane.points =
            vec![AutoPoint { time: 8 * PPQ, value: 140.004, curve: TEMPO_RAMP }, AutoPoint { time: 12 * PPQ, value: 90.0, curve: 0.0 }];
        let pts = tempo_points(&lane, 120.0, true);
        assert_eq!(pts.len(), 3);
        assert_eq!((pts[0].time, pts[0].bpm, pts[0].ramp), (0, 120.0, false));
        assert_eq!((pts[1].bpm, pts[1].ramp), (140.0, true));
        // Later edits don't add a point at bar 1 (the first point holds before itself).
        assert_eq!(tempo_points(&lane, 120.0, false).len(), 2);
        // Two points dragged onto one tick: the later one stays.
        lane.points[1].time = 8 * PPQ;
        assert_eq!(tempo_points(&lane, 120.0, false), vec![dstudio_core::TempoPoint { time: 8 * PPQ, bpm: 90.0, ramp: false }]);
    }
}
