//! The tempo map (spec 015): tempo changes during a song.
//!
//! A project's tempo is a sorted list of [TempoPoint]s on the transport. Each point holds its tempo
//! until the next point, or ramps to the next point's tempo linearly in BPM over beats. Before the
//! first point the first point's tempo holds; with no points the song has the one tempo
//! `Transport::bpm`. Bars and beats stay fixed and real time stretches around them, so every
//! conversion between ticks, seconds and frames goes through [TempoMap].
//!
//! The map is a list of segments. Segment `i` starts at quarter `q0` (tick / 960) and second `t0`
//! with tempo `a` BPM and runs to the next segment's quarter `q1`, where a ramp reaches `b` (a hold
//! keeps `b = a`; the last segment always holds). With `k = (b - a) / (q1 - q0)` BPM per quarter:
//!
//! - hold: `t(q) = t0 + (q - q0) · 60 / a`, `q(t) = q0 + (t - t0) · a / 60`;
//! - ramp: `t(q) = t0 + (60 / k) · ln(1 + k (q - q0) / a)`,
//!   `q(t) = q0 + (a / k) · (exp(k (t - t0) / 60) - 1)`, tempo `a + k (q - q0)`.
//!
//! The engine evaluates the same formula in frames (`t · rate`) for the metronome and for the tempo
//! and song position devices and plugins see (`se_tempo_segment`, engine/README.md › Tempo). A ramp
//! is integrated exactly, never sampled. Negative positions (a count-in before bar 1) hold the first
//! tempo.
//!
//! With no points every conversion uses exactly the arithmetic of a single tempo, so projects
//! without tempo changes convert (and so sound) exactly as before the map existed.

use crate::model::{PPQ, Ticks};
use serde::{Deserialize, Serialize};

/// Latest tick a tempo point may sit at (far beyond any song; positions stay convertible).
pub const MAX_TEMPO_TICK: Ticks = 1 << 40;
/// Most points a map may have.
pub const MAX_TEMPO_POINTS: usize = 4096;

/// One tempo change.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct TempoPoint {
    /// Where the tempo changes (ticks from bar 1, >= 0).
    pub time: Ticks,
    /// Quarter notes per minute, `MIN_BPM..=MAX_BPM`.
    pub bpm: f64,
    /// Ramp linearly (in BPM over beats) to the next point's tempo instead of holding.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub ramp: bool,
}

/// Checks a point list the way the commands do: ticks in `0..=MAX_TEMPO_TICK` and strictly
/// increasing, tempos finite in `MIN_BPM..=MAX_BPM`, at most [MAX_TEMPO_POINTS] points.
pub fn validate(points: &[TempoPoint]) -> Result<(), String> {
    use crate::command::{MAX_BPM, MIN_BPM};
    if points.len() > MAX_TEMPO_POINTS {
        return Err(format!("at most {MAX_TEMPO_POINTS} tempo points"));
    }
    let mut prev: Option<Ticks> = None;
    for (i, p) in points.iter().enumerate() {
        if p.time < 0 || p.time > MAX_TEMPO_TICK {
            return Err(format!("point {i}: time outside 0..={MAX_TEMPO_TICK}"));
        }
        if prev.is_some_and(|t| p.time <= t) {
            return Err(format!("point {i}: points must be sorted by time, one per tick"));
        }
        if !p.bpm.is_finite() || !(MIN_BPM..=MAX_BPM).contains(&p.bpm) {
            return Err(format!("point {i}: tempo {} outside {MIN_BPM}..={MAX_BPM}", p.bpm));
        }
        prev = Some(p.time);
    }
    Ok(())
}

/// A stretch of the song with a held or linearly ramping tempo.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Segment {
    /// Start, in ticks and in seconds from bar 1.
    pub tick: Ticks,
    pub seconds: f64,
    /// Tempo at the start and at the end (equal for a hold).
    pub start_bpm: f64,
    pub end_bpm: f64,
    /// The next segment's start (`Ticks::MAX` for the last segment, which holds).
    pub end_tick: Ticks,
}

impl Segment {
    /// BPM per quarter note of a ramp; None for a hold.
    fn slope(&self) -> Option<f64> {
        (self.end_bpm != self.start_bpm && self.end_tick != Ticks::MAX)
            .then(|| (self.end_bpm - self.start_bpm) / ((self.end_tick - self.tick) as f64 / PPQ as f64))
    }

    /// Seconds from the segment start to `x` quarters into it.
    fn seconds_in(&self, x: f64) -> f64 {
        let a = self.start_bpm;
        match self.slope() {
            None => x * 60.0 / a,
            Some(k) => 60.0 / k * (k * x / a).ln_1p(),
        }
    }

    /// Quarters from the segment start after `dt` seconds.
    fn quarters_in(&self, dt: f64) -> f64 {
        let a = self.start_bpm;
        match self.slope() {
            None => dt * a / 60.0,
            Some(k) => a / k * (k * dt / 60.0).exp_m1(),
        }
    }

    fn bpm_in(&self, x: f64) -> f64 {
        match self.slope() {
            None => self.start_bpm,
            Some(k) => self.start_bpm + k * x,
        }
    }

    /// Whether the segment ramps.
    pub fn ramps(&self) -> bool {
        self.slope().is_some()
    }
}

/// Conversions between musical and real time for one project (see the module docs).
#[derive(Debug, Clone, PartialEq)]
pub struct TempoMap {
    /// The single tempo when there are no segments.
    bpm: f64,
    /// Empty for a song without tempo changes; else starts at tick 0.
    segments: Vec<Segment>,
}

impl TempoMap {
    /// The map of `points` (sorted, as [validate] accepts), or the single tempo `bpm` when empty.
    pub fn new(bpm: f64, points: &[TempoPoint]) -> Self {
        let mut segments: Vec<Segment> = Vec::new();
        let Some(first) = points.first() else { return TempoMap { bpm, segments } };
        segments.reserve(points.len() + 1);
        if first.time > 0 {
            segments.push(Segment { tick: 0, seconds: 0.0, start_bpm: first.bpm, end_bpm: first.bpm, end_tick: first.time });
        }
        for (i, p) in points.iter().enumerate() {
            let next = points.get(i + 1);
            let seconds = segments.last().map_or(0.0, |s| s.seconds + s.seconds_in((s.end_tick - s.tick) as f64 / PPQ as f64));
            segments.push(Segment {
                tick: p.time,
                seconds,
                start_bpm: p.bpm,
                end_bpm: match next {
                    Some(n) if p.ramp => n.bpm,
                    _ => p.bpm,
                },
                end_tick: next.map_or(Ticks::MAX, |n| n.time),
            });
        }
        TempoMap { bpm: first.bpm, segments }
    }

    /// A single tempo.
    pub fn constant(bpm: f64) -> Self {
        TempoMap { bpm, segments: Vec::new() }
    }

    /// No tempo changes: one tempo for the whole song.
    pub fn is_constant(&self) -> bool {
        self.segments.is_empty()
    }

    /// The segments (empty without tempo changes).
    pub fn segments(&self) -> &[Segment] {
        &self.segments
    }

    /// Tempo at bar 1 (the single tempo without changes).
    pub fn first_bpm(&self) -> f64 {
        self.bpm
    }

    fn segment_at_tick(&self, t: f64) -> &Segment {
        let i = self.segments.partition_point(|s| (s.tick as f64) <= t);
        &self.segments[i.max(1) - 1]
    }

    fn segment_at_seconds(&self, s: f64) -> &Segment {
        let i = self.segments.partition_point(|g| g.seconds <= s);
        &self.segments[i.max(1) - 1]
    }

    /// Seconds from bar 1 at tick `t` (fractional ticks allowed).
    pub fn seconds_at(&self, t: f64) -> f64 {
        if self.segments.is_empty() {
            return t / PPQ as f64 * 60.0 / self.bpm;
        }
        if t < 0.0 {
            return t / PPQ as f64 * 60.0 / self.bpm;
        }
        let g = self.segment_at_tick(t);
        g.seconds + g.seconds_in((t - g.tick as f64) / PPQ as f64)
    }

    /// Tick (fractional) at `s` seconds from bar 1.
    pub fn ticks_at(&self, s: f64) -> f64 {
        if self.segments.is_empty() || s < 0.0 {
            return s * self.bpm / 60.0 * PPQ as f64;
        }
        let g = self.segment_at_seconds(s);
        g.tick as f64 + g.quarters_in(s - g.seconds) * PPQ as f64
    }

    /// Tempo in BPM at tick `t`.
    pub fn bpm_at(&self, t: f64) -> f64 {
        if self.segments.is_empty() || t < 0.0 {
            return self.bpm;
        }
        let g = self.segment_at_tick(t);
        g.bpm_in((t - g.tick as f64) / PPQ as f64)
    }

    pub fn ticks_to_seconds(&self, t: Ticks) -> f64 {
        self.seconds_at(t as f64)
    }

    pub fn seconds_to_ticks(&self, s: f64) -> Ticks {
        self.ticks_at(s).round() as Ticks
    }

    /// Ticks → frames at `rate` (rounded to the nearest frame).
    pub fn ticks_to_frames(&self, t: Ticks, rate: f64) -> i64 {
        (self.ticks_to_seconds(t) * rate).round() as i64
    }

    pub fn frames_to_ticks(&self, frames: i64, rate: f64) -> Ticks {
        self.seconds_to_ticks(frames as f64 / rate)
    }

    /// Fractional ticks → frames at `rate`, unrounded.
    pub fn frame_at(&self, t: f64, rate: f64) -> f64 {
        self.seconds_at(t) * rate
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pt(beat: f64, bpm: f64, ramp: bool) -> TempoPoint {
        TempoPoint { time: (beat * PPQ as f64) as Ticks, bpm, ramp }
    }

    #[test]
    fn no_points_is_the_single_tempo() {
        let m = TempoMap::new(120.0, &[]);
        assert!(m.is_constant());
        assert_eq!(m.ticks_to_seconds(PPQ), 0.5);
        assert_eq!(m.seconds_to_ticks(1.0), 2 * PPQ);
        assert_eq!(m.ticks_to_frames(-PPQ, 48_000.0), -24_000);
        assert_eq!(m.bpm_at(1e9), 120.0);
    }

    #[test]
    fn steps_hold_until_the_next_point() {
        // 120 BPM for 4 beats (2 s), then 60 BPM.
        let m = TempoMap::new(120.0, &[pt(0.0, 120.0, false), pt(4.0, 60.0, false)]);
        assert_eq!(m.ticks_to_seconds(4 * PPQ), 2.0);
        assert_eq!(m.ticks_to_seconds(5 * PPQ), 3.0);
        assert_eq!(m.seconds_to_ticks(3.0), 5 * PPQ);
        assert_eq!((m.bpm_at(4.0 * PPQ as f64 - 1.0), m.bpm_at(4.0 * PPQ as f64)), (120.0, 60.0));
    }

    #[test]
    fn the_first_tempo_holds_before_the_first_point() {
        let m = TempoMap::new(90.0, &[pt(8.0, 90.0, false), pt(12.0, 180.0, false)]);
        assert_eq!(m.ticks_to_seconds(8 * PPQ), 8.0 * 60.0 / 90.0);
        assert_eq!(m.ticks_to_seconds(-PPQ), -60.0 / 90.0);
        assert_eq!(m.bpm_at(0.0), 90.0);
    }

    #[test]
    fn a_ramp_matches_numeric_integration() {
        let m = TempoMap::new(80.0, &[pt(0.0, 80.0, false), pt(2.0, 80.0, true), pt(10.0, 160.0, false), pt(12.0, 100.0, false)]);
        let bpm = |q: f64| {
            if q < 2.0 {
                80.0
            } else if q < 10.0 {
                80.0 + (q - 2.0) * 10.0
            } else if q < 12.0 {
                160.0
            } else {
                100.0
            }
        };
        // Midpoint rule, fine steps.
        let (mut t, n) = (0.0, 140_000); // cells end on every tempo point
        let dq = 14.0 / n as f64;
        for i in 0..n {
            let q = (i as f64 + 0.5) * dq;
            t += dq * 60.0 / bpm(q);
            if (i + 1) % 10_000 == 0 {
                let exact = m.seconds_at((i + 1) as f64 * dq * PPQ as f64);
                assert!((exact - t).abs() < 1e-6, "at quarter {}: {exact} vs {t}", (i + 1) as f64 * dq);
            }
        }
        assert!((m.bpm_at(6.0 * PPQ as f64) - 120.0).abs() < 1e-9);
    }

    #[test]
    fn ticks_round_trip_everywhere() {
        let m = TempoMap::new(120.0, &[pt(0.0, 120.0, true), pt(16.0, 60.0, false), pt(20.0, 200.0, true), pt(28.0, 33.0, false)]);
        for t in (-4 * PPQ..40 * PPQ).step_by(37) {
            let f = m.ticks_to_frames(t, 48_000.0);
            let back = m.frames_to_ticks(f, 48_000.0);
            assert!((back - t).abs() <= 1, "{t} -> {f} -> {back}");
            let s = m.seconds_at(t as f64);
            assert!((m.ticks_at(s) - t as f64).abs() < 1e-6);
        }
    }

    #[test]
    fn validation() {
        assert!(validate(&[pt(0.0, 120.0, true), pt(1.0, 60.0, false)]).is_ok());
        assert!(validate(&[pt(1.0, 120.0, false), pt(1.0, 60.0, false)]).is_err(), "one point per tick");
        assert!(validate(&[pt(2.0, 120.0, false), pt(1.0, 60.0, false)]).is_err(), "sorted");
        assert!(validate(&[TempoPoint { time: -1, bpm: 120.0, ramp: false }]).is_err());
        assert!(validate(&[pt(0.0, 19.0, false)]).is_err());
        assert!(validate(&[pt(0.0, f64::NAN, false)]).is_err());
    }
}
