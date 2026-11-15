//! Timeline geometry: view transform, hit-testing and the per-frame draw list.
//!
//! Coordinates here are *lane-local logical points*: origin at the top-left of
//! the lanes surface (window x 453, y 105.5 at the default window size), i.e. the
//! ruler is at the top, track 0 starts at `RULER_H`.

use crate::gpu::{DrawList, RectInst};
use crate::peaks::{self, Slot, WaveSeg};

pub const RULER_H: f32 = 39.0;
/// Track rows are 88 px at the default zoom (88 × 0.85), including a 0.5 pt rule.
pub const TRACK_PITCH: f32 = 74.8;
pub const TRACK_H: f32 = 74.3;
/// Shortest row: a 20 px floor at the default zoom.
pub const MIN_TRACK_PITCH: f32 = 17.0;

/// Row pitch for a track height scale (`max(88 × scale, 20)` px at the default zoom).
pub fn pitch_for_scale(scale: f32) -> f32 {
    (TRACK_PITCH * scale).max(MIN_TRACK_PITCH)
}
/// Master row: 36 px, its top rule included.
pub const MASTER_H: f32 = 30.6;
/// Master row height while automation lanes are shown (separator at
/// window y 439.5, lanes bottom 504).
pub const MASTER_H_AUTO: f32 = 64.6;
pub const CLIP_HEADER: f32 = 17.0;
pub const EDGE_GRAB: f32 = 5.0;
/// Zoom limits, in logical points per beat.
pub const MIN_PX_PER_BEAT: f64 = 0.5;
pub const MAX_PX_PER_BEAT: f64 = 2000.0;
/// Default zoom: 51.03 pt per 4/4 bar (bar 20 line at x 1422.5, bar 1 at 453).
pub const DEFAULT_PX_PER_BEAT: f64 = 969.5 / 76.0;

/// Design units per 100%-zoom px (drawings are laid out at 0.85); the thresholds below
/// are in 100%-zoom px.
pub const REF: f64 = 0.85;
/// Pixels between two labelled bars before the label step stops doubling.
const LABEL_MIN_PX: f64 = 50.0;
/// Fewest pixels between grid lines inside a bar, and between bar-or-longer grid lines.
const GRID_MIN_PX: f64 = 16.0;
const GRID_BAR_MIN_PX: f64 = 6.0;
/// Fewest pixels a bar-shading group may span.
const BAND_MIN_PX: f64 = 80.0;
/// Shortest grid step, in beats, and the longest, in beats.
const GRID_MIN_BEATS: f64 = 1.0 / 16.0;
const GRID_MAX_BEATS: f64 = 128.0;
/// Ticks closer than this many pixels are not drawn.
const TICK_MIN_PX: f64 = 4.0;
/// A fixed grid size is doubled until it is this wide.
const FIXED_MIN_PX: f64 = 6.0;
const EPS: f64 = 1e-6;

/// The grid sizes of the toolbar's snap menu; `None` in a [RulerSteps] is "Auto".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GridSize {
    Bar,
    HalfBar,
    Quarter,
    Eighth,
    Sixteenth,
    ThirtySecond,
    QuarterTriplet,
    EighthTriplet,
    SixteenthTriplet,
    ThirtySecondTriplet,
}

impl GridSize {
    /// Menu order: straight sizes, then the triplets (a rule between them).
    pub const ALL: [GridSize; 10] = [
        GridSize::Bar,
        GridSize::HalfBar,
        GridSize::Quarter,
        GridSize::Eighth,
        GridSize::Sixteenth,
        GridSize::ThirtySecond,
        GridSize::QuarterTriplet,
        GridSize::EighthTriplet,
        GridSize::SixteenthTriplet,
        GridSize::ThirtySecondTriplet,
    ];
    /// Number of straight entries (after "Auto") before the rule.
    pub const STRAIGHT: usize = 6;

    pub fn label(self) -> &'static str {
        match self {
            GridSize::Bar => "1 Bar",
            GridSize::HalfBar => "1/2",
            GridSize::Quarter => "1/4",
            GridSize::Eighth => "1/8",
            GridSize::Sixteenth => "1/16",
            GridSize::ThirtySecond => "1/32",
            GridSize::QuarterTriplet => "1/4T",
            GridSize::EighthTriplet => "1/8T",
            GridSize::SixteenthTriplet => "1/16T",
            GridSize::ThirtySecondTriplet => "1/32T",
        }
    }

    pub fn is_triplet(self) -> bool {
        self as usize >= GridSize::QuarterTriplet as usize
    }

    /// Length in beats (quarter notes); a "1/2" is two beats whatever the signature.
    pub fn beats(self, beats_per_bar: f64) -> f64 {
        match self {
            GridSize::Bar => beats_per_bar,
            GridSize::HalfBar => 2.0,
            GridSize::Quarter => 1.0,
            GridSize::Eighth => 0.5,
            GridSize::Sixteenth => 0.25,
            GridSize::ThirtySecond => 0.125,
            GridSize::QuarterTriplet => 2.0 / 3.0,
            GridSize::EighthTriplet => 1.0 / 3.0,
            GridSize::SixteenthTriplet => 1.0 / 6.0,
            GridSize::ThirtySecondTriplet => 1.0 / 12.0,
        }
    }
}

/// Which bars get a label and a full-height tick, where lanes get grid lines, and how long a
/// shaded group is, at one zoom. Steps in bars are whole bars; steps in beats are quarter notes.
/// Ruler rules.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RulerSteps {
    /// A bar number every this many bars (a power of two).
    pub label_bars: i64,
    /// A full-height tick every this many bars (half the label step, at least 1).
    pub tick_bars: i64,
    /// Sub-bar and long-bar tick step, in beats (the grid step before it is widened).
    pub tick_beats: f64,
    /// Grid line step in the lanes and clips, in beats.
    pub line_beats: f64,
    /// Length in beats of one shaded group (shading alternates group by group).
    pub band_beats: f64,
    pub beats_per_bar: f64,
    pub beat_len: f64,
    /// Whether the tick step is wide enough (4 px at 100% zoom) to draw short ticks.
    pub ticks_visible: bool,
}

/// The step a clip editor's note edits snap to: a fixed size as `RulerSteps` widens it, or
/// for Auto (and a triplet that does not fit the bar) a step by the zoom: 4 or 2 bars while a bar
/// is under 6 or 12 px, then a bar, a beat, 1/2, 1/4 and 1/8 beat at 5, 12, 30 and 60 px per beat.
/// `px_per_beat` is in 100%-zoom px; beats are quarter notes.
pub fn note_snap_beats(px_per_beat: f64, beats_per_bar: f64, grid: Option<GridSize>) -> f64 {
    if let Some(s) = grid.and_then(|g| RulerSteps::fixed_step(g, beats_per_bar, px_per_beat)) {
        return s;
    }
    let bar_px = px_per_beat * beats_per_bar;
    match () {
        _ if bar_px < 6.0 => 4.0 * beats_per_bar,
        _ if bar_px < 12.0 => 2.0 * beats_per_bar,
        _ if px_per_beat < 5.0 => beats_per_bar,
        _ if px_per_beat < 12.0 => 1.0,
        _ if px_per_beat < 30.0 => 0.5,
        _ if px_per_beat < 60.0 => 0.25,
        _ => 0.125,
    }
}

impl RulerSteps {
    /// `beats_per_bar` and `beat_len` are in quarter notes (4/4: 4 and 1; 6/8: 3 and 0.5).
    pub fn new(px_per_beat: f64, beats_per_bar: f64, beat_len: f64, grid: Option<GridSize>) -> Self {
        let px_per_beat = px_per_beat / REF;
        let bar_px = px_per_beat * beats_per_bar;
        let mut label_bars = 1i64;
        while bar_px > 0.0 && bar_px * (label_bars as f64) < LABEL_MIN_PX && label_bars < 1024 {
            label_bars *= 2;
        }
        let tick_bars = (label_bars / 2).max(1);
        let numerator = (beats_per_bar / beat_len).round().max(1.0) as i64;

        // Candidate grid steps, ascending: beat halvings down to 1/16 beat, the beat
        // multiples that divide the bar, then 2, 4, 8 ... bars up to 128 beats.
        let mut cand = [0.0f64; 40];
        let mut n = 0;
        let mut s = beat_len;
        while s / 2.0 >= GRID_MIN_BEATS - EPS {
            s /= 2.0;
        }
        while s < beat_len - EPS {
            cand[n] = s;
            n += 1;
            s *= 2.0;
        }
        cand[n] = beat_len;
        n += 1;
        for k in 2..=numerator {
            if numerator % k == 0 {
                cand[n] = beat_len * k as f64;
                n += 1;
            }
        }
        let mut s = beats_per_bar * 2.0;
        while s <= GRID_MAX_BEATS && n < cand.len() {
            cand[n] = s;
            n += 1;
            s *= 2.0;
        }
        let mut tick_beats = (0..n)
            .map(|i| cand[i])
            .find(|&c| c * px_per_beat >= if c >= beats_per_bar - EPS { GRID_BAR_MIN_PX } else { GRID_MIN_PX })
            .unwrap_or(cand[n - 1]);
        if let Some(g) = grid
            && let Some(fixed) = Self::fixed_step(g, beats_per_bar, px_per_beat)
            && (g.is_triplet() || fixed < tick_beats)
        {
            tick_beats = fixed;
        }
        let line_beats = if tick_beats < beats_per_bar - EPS { tick_beats } else { tick_beats.max(tick_bars as f64 * beats_per_bar) };

        let bar = beats_per_bar;
        let mut bands = [beat_len, bar, numerator as f64 * bar, 4.0 * numerator as f64 * bar];
        bands.sort_by(f64::total_cmp);
        let band_beats = bands.into_iter().find(|&b| b * px_per_beat >= BAND_MIN_PX).unwrap_or(bands[3]);
        RulerSteps {
            label_bars,
            tick_bars,
            tick_beats,
            line_beats,
            band_beats,
            beats_per_bar,
            beat_len,
            ticks_visible: tick_beats * px_per_beat >= TICK_MIN_PX,
        }
    }

    /// A fixed grid size at this zoom: doubled until 6 px wide (or 128 beats); a triplet that
    /// stops fitting the bar when doubled is not used (`None`).
    pub(crate) fn fixed_step(g: GridSize, beats_per_bar: f64, px_per_beat: f64) -> Option<f64> {
        let widen = |mut s: f64| {
            while s * px_per_beat < FIXED_MIN_PX && s < GRID_MAX_BEATS {
                s *= 2.0;
            }
            s
        };
        if !g.is_triplet() {
            return Some(widen(g.beats(beats_per_bar)));
        }
        let unit = {
            let (mut a, mut b) = ((beats_per_bar * 4.0).round() as i64, 8i64);
            while b != 0 {
                (a, b) = (b, a % b);
            }
            a as f64 / 4.0
        };
        let mut s = g.beats(beats_per_bar);
        while !Self::on(unit, s) && s > 0.0625 / 4.0 {
            s /= 2.0;
        }
        let s = widen(s);
        Self::on(unit, s).then_some(s)
    }

    /// The beat a pointer position snaps to on this grid, never before 0.
    pub fn snap(&self, beat: f64) -> f64 {
        ((beat / self.tick_beats).round() * self.tick_beats).max(0.0)
    }

    /// Dragging a clip: the grid position or, for a clip that sits off the grid, the position
    /// that keeps its own offset, whichever is nearer (`orig` is the clip's start before the drag).
    pub fn snap_move(&self, beat: f64, orig: f64) -> f64 {
        let on_grid = (beat / self.tick_beats).round() * self.tick_beats;
        let kept = orig + ((beat - orig) / self.tick_beats).round() * self.tick_beats;
        (if (kept - beat).abs() < (on_grid - beat).abs() { kept } else { on_grid }).max(0.0)
    }

    /// Whether `beat` is a multiple of `unit` (to within a millionth of a beat).
    pub fn on(beat: f64, unit: f64) -> bool {
        (beat - (beat / unit).round() * unit).abs() < EPS
    }
}

#[derive(Debug, Clone)]
pub struct View {
    pub start_beats: f64,
    pub px_per_beat: f64,
    pub scroll_y: f64,
    /// Extra height below each track (expanded automation lanes, src/automation.rs), by track index.
    pub extra: Vec<f32>,
    /// Row pitch of each track (its height scale × [TRACK_PITCH]), by track index; missing
    /// entries use [TRACK_PITCH].
    pub pitches: Vec<f32>,
    /// Height of the master row pinned to the bottom (taller while automation is shown).
    pub master_h: f32,
    /// Tracks from this index on (the return tracks while the Returns pill is lit) are pinned
    /// above the master row instead of scrolling; `pinned_h` is their total height.
    pub pinned_from: usize,
    pub pinned_h: f32,
    /// Lanes height (ruler included), for the pinned rows' positions.
    pub height: f32,
}

impl Default for View {
    fn default() -> Self {
        View {
            start_beats: 0.0,
            px_per_beat: DEFAULT_PX_PER_BEAT,
            scroll_y: 0.0,
            extra: Vec::new(),
            pitches: Vec::new(),
            master_h: MASTER_H,
            pinned_from: usize::MAX,
            pinned_h: 0.0,
            height: 0.0,
        }
    }
}

impl View {
    pub fn x_of(&self, beat: f64) -> f32 {
        ((beat - self.start_beats) * self.px_per_beat) as f32
    }
    pub fn beat_at(&self, x: f32) -> f64 {
        self.start_beats + x as f64 / self.px_per_beat
    }
    /// Row pitch of track `i` (lane plus its 0.5 pt rule), without automation rows.
    pub fn pitch(&self, i: usize) -> f32 {
        self.pitches.get(i).copied().unwrap_or(TRACK_PITCH)
    }
    /// Content offset of track `i` (sum of the rows above it, without the ruler or scroll).
    pub fn track_offset(&self, i: usize) -> f32 {
        (0..i).map(|k| self.track_span(k)).sum()
    }
    /// Total height of track `i` including its expanded automation rows.
    pub fn track_span(&self, i: usize) -> f32 {
        self.pitch(i) + self.extra.get(i).copied().unwrap_or(0.0)
    }
    /// Pinned rows plus the master row: the band at the bottom that does not scroll.
    pub fn footer_h(&self) -> f32 {
        self.master_h + self.pinned_h
    }
    /// Top of the pinned rows (the bottom of the scrolling lanes).
    pub fn pinned_top(&self) -> f32 {
        self.height - self.footer_h()
    }
    pub fn is_pinned(&self, i: usize) -> bool {
        i >= self.pinned_from
    }
    pub fn track_top(&self, i: usize) -> f32 {
        if self.is_pinned(i) {
            return self.pinned_top() + self.track_offset(i) - self.track_offset(self.pinned_from);
        }
        RULER_H + self.track_offset(i) - self.scroll_y as f32
    }
    /// Track whose row (or expanded automation rows) contains `y`.
    pub fn track_at(&self, y: f32) -> Option<usize> {
        let n = self.extra.len().max(self.pitches.len());
        if self.pinned_from < n && y >= self.pinned_top() {
            let mut top = self.pinned_top();
            for i in self.pinned_from..n {
                let h = self.track_span(i);
                if y < top + h {
                    return Some(i);
                }
                top += h;
            }
            return None;
        }
        let mut top = RULER_H - self.scroll_y as f32;
        if y < top {
            return None;
        }
        let pinned = self.pinned_from < n;
        let n = n.min(self.pinned_from);
        for i in 0..n {
            let h = self.track_span(i);
            if y < top + h {
                return Some(i);
            }
            top += h;
        }
        // Empty space under the ordinary tracks: with pinned rows below it, no index past them is
        // an ordinary track, and a synthesized one would land on a pinned row.
        if pinned {
            return None;
        }
        Some(n + ((y - top) / TRACK_PITCH).floor() as usize)
    }
    pub fn zoom_at(&mut self, factor: f64, x: f32) {
        let anchor = self.beat_at(x);
        self.px_per_beat = (self.px_per_beat * factor).clamp(MIN_PX_PER_BEAT, MAX_PX_PER_BEAT);
        self.start_beats = anchor - x as f64 / self.px_per_beat;
        self.clamp();
    }
    pub fn clamp(&mut self) {
        self.start_beats = self.start_beats.max(0.0);
    }
    pub fn clamp_scroll(&mut self, tracks: usize, lanes_h: f32) {
        let content = self.track_offset(tracks.min(self.pinned_from)) + 40.0;
        let visible = lanes_h - RULER_H - self.footer_h();
        self.scroll_y = self.scroll_y.clamp(0.0, (content - visible).max(0.0) as f64);
    }
}

#[derive(Debug, Clone, Copy)]
pub struct TrackVis {
    pub selected: bool,
}

#[derive(Debug, Clone)]
pub struct ClipVis {
    pub track_index: usize,
    pub start_beats: f64,
    pub len_beats: f64,
    pub color: [f32; 3],
    pub selected: bool,
    /// Clip on a muted track: drawn grey, without the selection tint.
    pub muted: bool,
    pub name: String,
    /// Waveform: pyramid slot and the clip's pieces (`peaks::wave_segments`).
    pub wave: Option<(Slot, Vec<WaveSeg>)>,
    /// MIDI clip: mini note view as (relative start 0..1, relative length, relative
    /// pitch 0 = top .. 1 = bottom). `None` for audio clips.
    pub midi: Option<Vec<[f32; 4]>>,
    /// Take being recorded: live min/max peaks from `Engine::recording_peaks`
    /// (one pair every `.1` logical points from the clip start), drawn with the
    /// recording tint.
    pub live: Option<(Vec<[f32; 2]>, f32)>,
    /// Audio clip fades (in, out) in beats, clamped to the clip.
    pub fades: (f64, f64),
    /// Draw (and hit-test) the fade handles: the clip is hovered or selected.
    pub fade_handles: bool,
}

/// Half the side of a fade handle's square (its outline is one more).
pub const FADE_HANDLE: f32 = 2.0;
/// How far from a fade handle's centre a press still takes it.
pub const FADE_GRAB: f32 = 6.0;

impl ClipVis {
    pub fn rect(&self, v: &View) -> (f32, f32, f32, f32) {
        let x = v.x_of(self.start_beats);
        let w = (self.len_beats * v.px_per_beat) as f32;
        (x, v.track_top(self.track_index) + 0.5, w.max(1.0), v.pitch(self.track_index) - 1.5)
    }

    /// Fade-in and fade-out widths in px.
    pub fn fade_px(&self, v: &View) -> (f32, f32) {
        ((self.fades.0 * v.px_per_beat) as f32, (self.fades.1 * v.px_per_beat) as f32)
    }

    /// Centres of the fade-in and fade-out handles: just under the title strip at the end of
    /// each fade, kept inside the clip.
    pub fn fade_handle_centres(&self, v: &View) -> [(f32, f32); 2] {
        let (x, y, w, _) = self.rect(v);
        let (fi, fo) = self.fade_px(v);
        let lo = x + FADE_HANDLE + 1.0;
        let hi = (x + w - FADE_HANDLE - 1.0).max(lo);
        let hy = y + CLIP_HEADER + FADE_HANDLE + 2.0;
        [((x + fi).clamp(lo, hi), hy), ((x + w - fo).clamp(lo, hi), hy)]
    }
}

/// The fade handle under (x, y): (clip index, fade-out). Only clips that show their handles
/// take part; the nearer handle wins when both are in reach.
pub fn fade_hit(v: &View, clips: &[ClipVis], x: f32, y: f32) -> Option<(usize, bool)> {
    let mut best: Option<(f32, usize, bool)> = None;
    for (i, c) in clips.iter().enumerate().filter(|(_, c)| c.fade_handles && c.midi.is_none()) {
        for (out, (hx, hy)) in c.fade_handle_centres(v).into_iter().enumerate() {
            let d = (x - hx).abs().max((y - hy).abs());
            if d <= FADE_GRAB && best.is_none_or(|b| d < b.0) {
                best = Some((d, i, out == 1));
            }
        }
    }
    best.map(|(_, i, out)| (i, out))
}

/// One clip of a group's track, drawn as a thin bar in the group's own lane: one row per
/// track of the group that holds clips ([summary_rows]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SummaryBlock {
    /// The group's row.
    pub track_index: usize,
    pub start_beats: f64,
    pub len_beats: f64,
    pub row: usize,
    pub rows: usize,
    /// The track's colour with the alpha for its state (folded or not, selected, silent).
    pub color: [f32; 4],
}

/// Layout of `rows` summary rows in a lane `h` tall: (top of the first row, row pitch, row
/// height). 2 px margins (8 % of short lanes), rows at most 10 px apart and 8 px tall with a
/// gap of a quarter of the pitch (at most 2 px), centred (all in 100 % px, times 0.85).
pub fn summary_rows(h: f32, rows: usize) -> (f32, f32, f32) {
    const U: f32 = 0.85;
    let rows = rows.max(1) as f32;
    let margin = (2.0 * U).min(h * 0.08);
    let avail = (h - 2.0 * margin).max(U);
    let pitch = (10.0 * U).min(avail / rows);
    let gap = (2.0 * U).min(pitch * 0.25);
    let row_h = (pitch - gap).clamp(U, 8.0 * U);
    (margin.max((h - rows * pitch) / 2.0), pitch, row_h)
}

pub struct Scene<'a> {
    pub view: View,
    pub size: (f32, f32), // logical
    pub scale: f32,
    pub tracks: &'a [TrackVis],
    pub clips: &'a [ClipVis],
    /// Group lanes: their tracks' clips in brief ([SummaryBlock]).
    pub summary: &'a [SummaryBlock],
    /// Growing clips of takes in progress (not hit-testable).
    pub live: &'a [ClipVis],
    pub playhead_beats: f64,
    pub loop_beats: (f64, f64),
    pub loop_on: bool,
    pub beats_per_bar: f64,
    /// Tick, label, grid and band steps for the current zoom (see [RulerSteps]).
    pub steps: RulerSteps,
    /// Automation overlay (src/automation.rs) while lanes are shown.
    pub auto: Option<&'a crate::automation::AutoScene>,
    /// Arrangement sections in the ruler's section lane (src/sections.rs).
    pub sections: &'a [crate::sections::SectionVis],
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Zone {
    Body,
    Left,
    Right,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Hit {
    Ruler,
    Master,
    Clip { index: usize, zone: Zone },
    Track(usize),
    Empty,
}

/// Height of the top-right corner of a clip that loops it when dragged (layout px).
pub const LOOP_CORNER: f32 = 8.0;

pub fn hit_test(v: &View, size: (f32, f32), tracks: usize, clips: &[ClipVis], x: f32, y: f32) -> Hit {
    if y < RULER_H {
        return Hit::Ruler;
    }
    if y >= size.1 - v.master_h {
        return Hit::Master;
    }
    // Pinned rows (return tracks) cover the scrolling lanes and take no clips.
    let pinned = y >= size.1 - v.footer_h();
    for (i, c) in clips.iter().enumerate().rev().filter(|_| !pinned) {
        let (cx, cy, cw, ch) = c.rect(v);
        if x >= cx && x < cx + cw && y >= cy && y < cy + ch {
            let edge = EDGE_GRAB.min(cw / 3.0);
            let zone = if x < cx + edge {
                Zone::Left
            } else if x >= cx + cw - edge {
                Zone::Right
            } else {
                Zone::Body
            };
            return Hit::Clip { index: i, zone };
        }
    }
    match v.track_at(y) {
        Some(t) if t < tracks && y < v.track_top(t) + v.pitch(t) - 0.5 => Hit::Track(t),
        _ => Hit::Empty,
    }
}

pub fn rgb(hex: u32) -> [f32; 3] {
    [((hex >> 16) & 0xff) as f32 / 255.0, ((hex >> 8) & 0xff) as f32 / 255.0, (hex & 0xff) as f32 / 255.0]
}

fn rgba(c: [f32; 3], a: f32) -> [f32; 4] {
    [c[0], c[1], c[2], a]
}

fn mix(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t]
}

const LANE: [u32; 2] = [0x131417, 0x16171a];
const LANE_SEL: [u32; 2] = [0x1a1b1e, 0x1d1e21];
const WHITE: [f32; 3] = [1.0, 1.0, 1.0];
const BLACK: [f32; 3] = [0.0, 0.0, 0.0];

pub struct Px {
    pub s: f32,
}
impl Px {
    /// Logical rect -> physical rect, snapped to whole pixels.
    pub fn r(&self, x: f32, y: f32, w: f32, h: f32) -> [f32; 4] {
        let x0 = (x * self.s).round();
        let y0 = (y * self.s).round();
        let x1 = ((x + w) * self.s).round();
        let y1 = ((y + h) * self.s).round();
        [x0, y0, (x1 - x0).max(1.0), (y1 - y0).max(1.0)]
    }
    pub fn rect(&self, x: f32, y: f32, w: f32, h: f32, color: [f32; 4]) -> RectInst {
        RectInst { rect: self.r(x, y, w, h), color, params: [0.0; 4] }
    }
    pub fn rounded(&self, x: f32, y: f32, w: f32, h: f32, radius: f32, color: [f32; 4]) -> RectInst {
        RectInst { rect: [x * self.s, y * self.s, w * self.s, h * self.s], color, params: [radius * self.s, 0.0, 0.0, 0.0] }
    }
}

/// Grid line strength by weight: bar, beat, sub-beat (.075 / .027 / .011 scaled by the
/// .775 of the lane ink).
fn grid_alpha(s: &RulerSteps, beat: f64) -> f32 {
    if RulerSteps::on(beat, s.beats_per_bar) {
        0.058
    } else if RulerSteps::on(beat, s.beat_len) {
        0.021
    } else {
        0.0085
    }
}

/// Lane background (alternating bands of `band_beats`) and grid for the band y0..y1.
fn bands_and_grid(out: &mut Vec<RectInst>, sc: &Scene, px: &Px, y0: f32, y1: f32, colors: [u32; 2]) {
    let v = &sc.view;
    let w = sc.size.0;
    let band = sc.steps.band_beats;
    let first = (v.start_beats / band).floor() as i64;
    let mut k = first;
    loop {
        let x0 = v.x_of(k as f64 * band);
        if x0 > w {
            break;
        }
        let x1 = v.x_of((k + 1) as f64 * band).min(w);
        out.push(px.rect(x0.max(0.0), y0, x1 - x0.max(0.0), y1 - y0, rgba(rgb(colors[(k.rem_euclid(2)) as usize]), 1.0)));
        k += 1;
    }
    // Grid lines: bar, beat and sub-beat weights at the step picked for this zoom.
    let step = sc.steps.line_beats;
    let first = (v.start_beats / step).floor() as i64;
    let lw = 1.0 / px.s; // one physical pixel (0.5 pt on Retina)
    let mut i = first;
    loop {
        let beat = i as f64 * step;
        let x = v.x_of(beat);
        if x > w {
            break;
        }
        if x >= -1.0 {
            out.push(px.rect(x, y0, lw, y1 - y0, rgba(WHITE, grid_alpha(&sc.steps, beat))));
        }
        i += 1;
    }
}

pub fn build(dl: &mut DrawList, sc: &Scene) {
    dl.clear_all();
    let px = Px { s: sc.scale };
    let (w, h) = sc.size;
    let v = &sc.view;
    let lanes_bottom = h - v.footer_h();
    dl.clear = [0.0745, 0.0784, 0.0902];
    dl.lanes_clip = ((RULER_H * sc.scale).round() as u32, (lanes_bottom * sc.scale).round() as u32);

    // ---- lanes (scrolling) ----
    bands_and_grid(&mut dl.base, sc, &px, RULER_H, lanes_bottom, LANE);
    for (i, t) in sc.tracks.iter().enumerate().take(v.pinned_from) {
        let top = v.track_top(i);
        let row_h = v.pitch(i) - 0.5;
        if top > lanes_bottom || top + row_h + 0.5 < RULER_H {
            continue;
        }
        if t.selected {
            bands_and_grid(&mut dl.base, sc, &px, top, top + row_h, LANE_SEL);
        }
        let sep = if t.selected { 0x2c2d30 } else { 0x26272a };
        dl.base.push(px.rect(0.0, top + row_h - 0.5, w, 1.0, rgba(rgb(sep), 1.0)));
    }

    // ---- group lanes: a bar per clip of each of the group's tracks ----
    for b in sc.summary {
        if v.is_pinned(b.track_index) {
            continue;
        }
        let top = v.track_top(b.track_index);
        let lane_h = v.pitch(b.track_index) - 0.5;
        let (x0, x1) = (v.x_of(b.start_beats), v.x_of(b.start_beats + b.len_beats));
        if x0 > w || x1 < 0.0 || top > lanes_bottom || top + lane_h < RULER_H {
            continue;
        }
        let (row_top, pitch, row_h) = summary_rows(lane_h, b.rows);
        let width = (x1 - x0 - 0.85).max(0.85);
        dl.base.push(px.rounded(x0, top + row_top + b.row as f32 * pitch, width, row_h, (1.7f32).min(row_h / 2.0), b.color));
    }

    // ---- clips ----
    for c in sc.clips {
        let (x, y, cw, ch) = c.rect(v);
        if x > w || x + cw < 0.0 || y > lanes_bottom || y + ch < RULER_H {
            continue;
        }
        let r = 3.0;
        if let Some(notes) = &c.midi {
            midi_clip(&mut dl.base, sc, &px, c, notes, (x, y, cw, ch));
            continue;
        }
        let (head, body) = if c.muted {
            (rgb(0xa8a8a8), rgb(0x6b6b6b))
        } else if c.selected {
            (c.color, mix(c.color, [0.91; 3], 0.69))
        } else {
            (c.color, c.color)
        };
        let wave = rgba(mix(body, BLACK, 0.92), 0.89);
        // Clip outline = rounded rect in the header colour; body drawn over it.
        dl.base.push(px.rounded(x, y, cw, ch, r, rgba(head, 1.0)));
        dl.base.push(px.rect(x, y + CLIP_HEADER, cw, ch - CLIP_HEADER - r, rgba(body, 1.0)));
        dl.base.push(px.rounded(x, y + ch - 2.0 * r, cw, 2.0 * r, r, rgba(body, 1.0)));
        dl.base.push(px.rect(x, y + CLIP_HEADER, cw, 1.0 / sc.scale, rgba(BLACK, 0.4)));
        let step = sc.steps.line_beats;
        let gw = 0.85f32.max(1.0 / sc.scale);
        let mut k = (c.start_beats.max(v.start_beats) / step).floor() as i64 + 1;
        loop {
            let beat = k as f64 * step;
            let gx = v.x_of(beat);
            if beat >= c.start_beats + c.len_beats || gx > w {
                break;
            }
            if gx >= 0.0 {
                let a = if RulerSteps::on(beat, sc.beats_per_bar) { 0.2 } else { 0.125 };
                dl.base.push(px.rect(gx, y + CLIP_HEADER + 1.0 / sc.scale, gw, ch - CLIP_HEADER - 1.0 / sc.scale, rgba(BLACK, a)));
            }
            k += 1;
        }
        if let Some((slot, segs)) = &c.wave {
            let (wy, wh) = (y + CLIP_HEADER + 2.0, ch - CLIP_HEADER - 4.0);
            peaks::push_wave(
                &mut dl.waves,
                *slot,
                segs,
                |b| v.x_of(c.start_beats + b),
                (x.max(0.0), (x + cw).min(w)),
                (wy, wh),
                sc.scale,
                wave,
            );
        }
        fade_overlay(&mut dl.base, sc, &px, c, body, (x, y, cw, ch));
    }

    // ---- takes being recorded: red-tinted growing clips with a live waveform (audio) or notes (MIDI) ----
    for c in sc.live {
        live_clip(&mut dl.base, sc, &px, c);
    }

    // ---- ruler (over) ----
    let o = &mut dl.over;
    o.push(px.rect(0.0, 0.0, w, RULER_H, rgba(rgb(0x1a1b1e), 1.0)));
    let lw = 1.0 / sc.scale;
    // One 100%-zoom px (0.85 units), at least one physical pixel: 2 px on Retina, 1 px at 1x.
    let mark = 0.85f32.max(lw);
    // Full-height ticks every `tick_bars` bars; between them, ticks standing on the row's
    // bottom at the grid step: 6 px (at 100% zoom) on a beat, 4 off it.
    let st = &sc.steps;
    let bar = st.beats_per_bar;
    let first_bar = ((v.start_beats / bar).floor() as i64).div_euclid(st.tick_bars) * st.tick_bars;
    let mut b = first_bar;
    loop {
        let x = v.x_of(b as f64 * bar);
        if x > w {
            break;
        }
        o.push(px.rect(x - 0.5 * mark, 0.0, mark, 16.5, rgba(rgb(0x4c4d4f), 1.0)));
        b += st.tick_bars;
    }
    if st.ticks_visible {
        let full = st.tick_bars as f64 * bar;
        let mut i = (v.start_beats / st.tick_beats).floor() as i64;
        loop {
            let beat = i as f64 * st.tick_beats;
            let x = v.x_of(beat);
            if x > w {
                break;
            }
            if beat >= 0.0 && x >= 0.0 && !RulerSteps::on(beat, full) {
                let h = if RulerSteps::on(beat, st.beat_len) { 5.1 } else { 3.4 };
                o.push(px.rect(x - 0.5 * mark, 16.5 - h, mark, h, rgba(rgb(0x38393c), 1.0)));
            }
            i += 1;
        }
    }
    for y in [16.5, 25.0, 38.0] {
        o.push(px.rect(0.0, y, w, 1.0, rgba(rgb(0x2c2d30), 1.0)));
    }
    // Loop region bar (model transport.loop_start..loop_end).
    let lx0 = v.x_of(sc.loop_beats.0);
    let lx1 = v.x_of(sc.loop_beats.1);
    if lx1 > 0.0 && lx0 < w {
        let color = if sc.loop_on { rgb(0xefb259) } else { rgb(0x4d4d4f) };
        o.push(px.rect(lx0.max(0.0), 17.6, lx1.min(w) - lx0.max(0.0), 6.8, rgba(color, 1.0)));
    }
    // Sections: rounded blocks inset half a 100%-zoom px at each end; selected and dragged
    // blocks lift and get a white rim (38 % / 1 px, 70 % / 1.5 px).
    use crate::sections::{BLOCK_H, BLOCK_Y, RADIUS, lift};
    for s in sc.sections {
        let x0 = v.x_of(s.start_beats) + 0.425;
        let bw = (s.len_beats * v.px_per_beat) as f32 - 0.85;
        if bw <= 0.0 || x0 > w || x0 + bw < 0.0 {
            continue;
        }
        o.push(px.rounded(x0, BLOCK_Y, bw, BLOCK_H, RADIUS, rgba(lift(s.color, s.state), 1.0)));
        let (a, t) = match s.state {
            1 => (0.38, 0.85),
            2 => (0.7, 1.275),
            _ => continue,
        };
        let rim = rgba(WHITE, a);
        o.push(px.rect(x0 + RADIUS, BLOCK_Y, bw - 2.0 * RADIUS, t, rim));
        o.push(px.rect(x0 + RADIUS, BLOCK_Y + BLOCK_H - t, bw - 2.0 * RADIUS, t, rim));
        o.push(px.rect(x0, BLOCK_Y + RADIUS, t, BLOCK_H - 2.0 * RADIUS, rim));
        o.push(px.rect(x0 + bw - t, BLOCK_Y + RADIUS, t, BLOCK_H - 2.0 * RADIUS, rim));
    }

    // ---- pinned rows (return tracks, over): their own group above the master lane ----
    if v.pinned_h > 0.0 {
        let y0 = h - v.footer_h();
        let mut tmp = Vec::new();
        bands_and_grid(&mut tmp, sc, &px, y0, h - v.master_h, LANE);
        o.extend(tmp);
        for (i, t) in sc.tracks.iter().enumerate().skip(v.pinned_from) {
            let top = v.track_top(i);
            let row_h = v.pitch(i) - 0.5;
            if t.selected {
                let mut tmp = Vec::new();
                bands_and_grid(&mut tmp, sc, &px, top, top + row_h, LANE_SEL);
                o.extend(tmp);
            }
            let sep = if t.selected { 0x2c2d30 } else { 0x26272a };
            o.push(px.rect(0.0, top + row_h - 0.5, w, 1.0, rgba(rgb(sep), 1.0)));
        }
        o.push(px.rect(0.0, y0 - 0.75, w, 1.5, rgba(rgb(0x5a5b60), 1.0)));
    }

    // ---- master lane (over, pinned to the bottom) ----
    o.push(px.rect(0.0, h - v.master_h, w, 1.0, rgba(rgb(0x26272a), 1.0)));
    let mut tmp = Vec::new();
    bands_and_grid(&mut tmp, sc, &px, h - v.master_h + 1.0, h, LANE);
    o.extend(tmp);

    // ---- automation: track rows over the clips (scissored), master row over its lane ----
    if let Some(a) = sc.auto {
        for row in &a.rows {
            let out = if row.master { &mut dl.over } else { &mut dl.top };
            automation_row(out, &px, w, row);
        }
        if let Some([x, y, mw, mh]) = a.marquee {
            dl.top.push(px.rect(x, y, mw, mh, [0.79, 0.66, 0.24, 0.12]));
            for (rx, ry, rw, rh) in [(x, y, mw, 0.5), (x, y + mh - 0.5, mw, 0.5), (x, y, 0.5, mh), (x + mw - 0.5, y, 0.5, mh)] {
                dl.top.push(px.rect(rx, ry, rw, rh, [0.79, 0.66, 0.24, 0.8]));
            }
        }
    }

    // ---- playhead ----
    let phx = v.x_of(sc.playhead_beats);
    if phx >= -1.0 && phx <= w {
        dl.over.push(px.rect(phx - 0.5 * mark, 0.0, mark, h, rgba(rgb(0xb3b3b3), 0.8)));
    }
}

/// One automation row: the dimming overlay (track lanes: clips and grid are dimmed under the
/// line), a separator under expanded rows, the curve as merged
/// per-column rects (1 pt line) and the point handles.
fn automation_row(out: &mut Vec<RectInst>, px: &Px, w: f32, row: &crate::automation::RowDraw) {
    use crate::automation::OVERLAY;
    if row.overlay {
        out.push(px.rect(0.0, row.y0, w, row.h, OVERLAY));
    }
    if row.extra {
        out.push(px.rect(0.0, row.y0 + row.h - 0.5, w, 0.5, rgba(rgb(0x26272a), 1.0)));
    }
    let color = row.style.line(row.hue);
    // Curve: consecutive samples become one rect per run of equal y (flat lines are one rect).
    let half = 0.5;
    let mut i = 0;
    let p = &row.poly;
    while i + 1 < p.len() {
        let (x0, y0) = p[i];
        let mut j = i + 1;
        if (p[j].1 - y0).abs() < 1e-3 {
            while j + 1 < p.len() && (p[j + 1].1 - y0).abs() < 1e-3 {
                j += 1;
            }
            out.push(px.rect(x0, y0 - half, p[j].0 - x0, 2.0 * half, color));
        } else {
            let (lo, hi) = (y0.min(p[j].1), y0.max(p[j].1));
            out.push(px.rect(x0, lo - half, (p[j].0 - x0).max(1.0 / px.s), hi - lo + 2.0 * half, color));
        }
        i = j;
    }
    let r = 2.5;
    if let Some((x, y)) = row.start_dot {
        out.push(px.rounded(x - r, y - r, 2.0 * r, 2.0 * r, r, color));
    }
    for &(x, y, selected) in &row.handles {
        if selected {
            out.push(px.rounded(x - r - 1.0, y - r - 1.0, 2.0 * r + 2.0, 2.0 * r + 2.0, r + 1.0, color));
            out.push(px.rounded(x - r + 0.5, y - r + 0.5, 2.0 * r - 1.0, 2.0 * r - 1.0, r - 0.5, [1.0, 1.0, 1.0, 1.0]));
        } else {
            out.push(px.rounded(x - r, y - r, 2.0 * r, 2.0 * r, r, color));
        }
    }
}

/// An audio clip's fades: the body above each fade line darkened, the line itself, and the
/// two square handles while the clip shows them.
fn fade_overlay(out: &mut Vec<RectInst>, sc: &Scene, px: &Px, c: &ClipVis, body: [f32; 3], (x, y, cw, ch): (f32, f32, f32, f32)) {
    let v = &sc.view;
    let w = sc.size.0;
    let (fi, fo) = c.fade_px(v);
    let top = y + CLIP_HEADER + 1.0;
    let bottom = y + ch;
    let shade = rgba(BLACK, 0.32);
    let line = rgba(BLACK, 0.7);
    let lw = 1.0f32.max(1.0 / sc.scale);
    // A fade from x0 to x1 whose line rises (fade-in) or falls (fade-out), in 1 px columns.
    let mut fade = |x0: f32, x1: f32, rising: bool| {
        let span = x1 - x0;
        if span < 1.0 {
            return;
        }
        let line_y = |gx: f32| {
            let t = ((gx - x0) / span).clamp(0.0, 1.0);
            let g = if rising { t } else { 1.0 - t };
            bottom - g * (bottom - top)
        };
        let mut gx = x0.max(0.0);
        let end = x1.min(w);
        while gx < end {
            let nx = (gx + 1.0).min(end);
            let (ya, yb) = (line_y(gx), line_y(nx));
            let (lo, hi) = (ya.min(yb), ya.max(yb));
            if lo > top {
                out.push(px.rect(gx, top, nx - gx, lo - top, shade));
            }
            out.push(px.rect(gx, lo - 0.5 * lw, nx - gx, hi - lo + lw, line));
            gx = nx;
        }
    };
    fade(x, x + fi, true);
    fade(x + cw - fo, x + cw, false);
    if c.fade_handles {
        let o = 1.0;
        for (hx, hy) in c.fade_handle_centres(v) {
            let s = FADE_HANDLE;
            out.push(px.rect(hx - s - o, hy - s - o, 2.0 * (s + o), 2.0 * (s + o), rgba(BLACK, 0.85)));
            out.push(px.rect(hx - s, hy - s, 2.0 * s, 2.0 * s, rgba(mix(body, WHITE, 0.6), 1.0)));
        }
    }
}

/// MIDI clip: track-colour title strip, light lavender body (lifted when selected), grid lines
/// inside the body and a mini note view.
fn midi_clip(out: &mut Vec<RectInst>, sc: &Scene, px: &Px, c: &ClipVis, notes: &[[f32; 4]], (x, y, cw, ch): (f32, f32, f32, f32)) {
    let r = 3.0;
    // On a track that isn't selected the body is drawn in the plain track colour
    // with a darker line under the title and no grid; on the
    // selected track it is the light lavender body with grid lines.
    // A muted track's clips are grey with no selection tint, like audio clips.
    let track_selected = !c.muted && sc.tracks.get(c.track_index).is_some_and(|t| t.selected);
    let head = if c.muted { rgb(0xa8a8a8) } else { c.color };
    let body = if c.muted {
        rgb(0x6b6b6b)
    } else if track_selected {
        mix(c.color, WHITE, 0.67)
    } else {
        c.color
    };
    // Selected clips: the body lifted toward white and a light outline, so several selected
    // clips stand out from the rest (src/selection.rs).
    let selected = c.selected && !c.muted;
    let body = if selected { mix(body, WHITE, 0.45) } else { body };
    // 0.5 pt outline (greyish; light when selected), then title strip and body inset by it.
    let outline = if selected { rgba(WHITE, 0.85) } else { rgba(mix(body, rgb(0x4a4c58), 0.45), 1.0) };
    out.push(px.rounded(x, y, cw, ch, r, outline));
    out.push(px.rounded(x + 0.5, y + 0.5, cw - 1.0, ch - 1.0, r - 0.5, rgba(head, 1.0)));
    let by = y + CLIP_HEADER + 0.5;
    out.push(px.rect(x + 0.5, by, cw - 1.0, ch - CLIP_HEADER - 0.5 - r, rgba(body, 1.0)));
    out.push(px.rounded(x + 0.5, y + ch - 2.0 * r, cw - 1.0, 2.0 * r - 0.5, r - 0.5, rgba(body, 1.0)));
    let v = &sc.view;
    if !track_selected {
        out.push(px.rect(x + 0.5, by - 0.5, cw - 1.0, 0.5, rgba(mix(head, BLACK, 0.4), 1.0)));
    }
    let step = sc.steps.line_beats;
    let mut b = (c.start_beats / step).floor() * step + step;
    while track_selected && b < c.start_beats + c.len_beats - 1e-6 {
        let lx = v.x_of(b);
        if lx > x + 0.5 {
            out.push(px.rect(lx, by, 1.0 / px.s, ch - CLIP_HEADER - 1.0, rgba(mix(body, BLACK, 0.075), 1.0)));
        }
        b += step;
    }
    let nh = ch - CLIP_HEADER - 6.0;
    let bar_h = (nh / 24.0).clamp(1.5, 3.0);
    let ink = mix(head, BLACK, 0.45);
    for n in notes {
        let nx = x + n[0] * cw;
        let nw = (n[1] * cw).max(1.0).min(x + cw - nx);
        if nw <= 0.0 {
            continue;
        }
        out.push(px.rect(nx, by + 2.5 + n[2] * (nh - bar_h), nw, bar_h, rgba(ink, n[3])));
    }
}

/// A take in progress: recording-red header strip, dark red body, live peaks.
fn live_clip(out: &mut Vec<RectInst>, sc: &Scene, px: &Px, c: &ClipVis) {
    let v = &sc.view;
    let w = sc.size.0;
    let lanes_bottom = sc.size.1 - sc.view.master_h;
    let (x, y, cw, ch) = c.rect(v);
    if x > w || x + cw < 0.0 || y > lanes_bottom || y + ch < RULER_H {
        return;
    }
    let r = 3.0;
    let body = mix(rgb(0x131417), c.color, 0.28);
    out.push(px.rounded(x, y, cw, ch, r, rgba(c.color, 1.0)));
    out.push(px.rect(x, y + CLIP_HEADER, cw, ch - CLIP_HEADER - r, rgba(body, 1.0)));
    out.push(px.rounded(x, y + ch - 2.0 * r, cw, 2.0 * r, r, rgba(body, 1.0)));
    // MIDI being recorded: the notes played so far, as on a MIDI clip.
    if let Some(notes) = &c.midi {
        let by = y + CLIP_HEADER + 0.5;
        let nh = ch - CLIP_HEADER - 6.0;
        let bar_h = (nh / 24.0).clamp(1.5, 3.0);
        let ink = mix(c.color, WHITE, 0.35);
        for n in notes {
            let nx = x + n[0] * cw;
            let nw = (n[1] * cw).max(1.0).min(x + cw - nx);
            if nw > 0.0 {
                out.push(px.rect(nx, by + 2.5 + n[2] * (nh - bar_h), nw, bar_h, rgba(ink, n[3])));
            }
        }
    }
    let Some((peaks, step)) = &c.live else { return };
    let wy = y + CLIP_HEADER + 2.0;
    let wh = ch - CLIP_HEADER - 4.0;
    let mid = wy + wh / 2.0;
    let ink = rgba(mix(c.color, WHITE, 0.25), 1.0);
    let lw = (*step).max(1.0 / px.s);
    for (i, p) in peaks.iter().enumerate() {
        let px_x = x + i as f32 * step;
        if px_x > x + cw || px_x > w {
            break;
        }
        if px_x + lw < 0.0 {
            continue;
        }
        let (lo, hi) = (p[0].clamp(-1.0, 1.0), p[1].clamp(-1.0, 1.0));
        let top = mid - hi * wh / 2.0;
        let h = ((hi - lo) * wh / 2.0).max(1.0 / px.s);
        out.push(px.rect(px_x, top, lw, h, ink));
    }
}

/// Bar-number labels for the ruler (Slint text over the texture).
pub fn ruler_labels(v: &View, w: f32, steps: &RulerSteps) -> Vec<(f32, String)> {
    let (beats_per_bar, step) = (steps.beats_per_bar, steps.label_bars);
    let mut out = Vec::new();
    let mut b = ((v.start_beats / beats_per_bar).floor() as i64).div_euclid(step) * step;
    loop {
        let x = v.x_of(b as f64 * beats_per_bar);
        if x > w {
            break;
        }
        if x > -30.0 && b >= 0 {
            out.push((x, format!("{}", b + 1)));
        }
        b += step;
    }
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn note_snap_auto_by_zoom_and_fixed_sizes() {
        let b = 4.0;
        assert_eq!(note_snap_beats(1.0, b, None), 16.0);
        assert_eq!(note_snap_beats(2.0, b, None), 8.0);
        assert_eq!(note_snap_beats(4.0, b, None), 4.0);
        assert_eq!(note_snap_beats(11.9, b, None), 1.0);
        assert_eq!(note_snap_beats(29.0, b, None), 0.5);
        assert_eq!(note_snap_beats(59.0, b, None), 0.25);
        assert_eq!(note_snap_beats(155.0, b, None), 0.125);
        assert_eq!(note_snap_beats(155.0, b, Some(GridSize::Eighth)), 0.5);
        assert_eq!(note_snap_beats(155.0, b, Some(GridSize::Bar)), 4.0);
        assert_eq!(note_snap_beats(20.0, b, Some(GridSize::ThirtySecond)), 0.5);
    }

    use super::*;

    fn steps(ppb: f64) -> RulerSteps {
        RulerSteps::new(ppb, 4.0, 1.0, None)
    }

    fn fixed(ppb: f64, g: GridSize) -> RulerSteps {
        RulerSteps::new(ppb, 4.0, 1.0, Some(g))
    }

    #[test]
    fn ruler_steps_at_several_zooms() {
        // (px per beat, label bars, tick bars, tick beats, line beats, band beats)
        let rows = [
            (MIN_PX_PER_BEAT, 32, 16, 16.0, 64.0, 64.0),
            (DEFAULT_PX_PER_BEAT / 8.0, 8, 4, 4.0, 16.0, 64.0),
            (DEFAULT_PX_PER_BEAT / 4.0, 4, 2, 4.0, 8.0, 64.0),
            (DEFAULT_PX_PER_BEAT / 2.0, 2, 1, 4.0, 4.0, 16.0),
            (DEFAULT_PX_PER_BEAT, 1, 1, 2.0, 2.0, 16.0),
            (DEFAULT_PX_PER_BEAT * 2.0, 1, 1, 1.0, 1.0, 4.0),
            (DEFAULT_PX_PER_BEAT * 4.0, 1, 1, 0.5, 0.5, 4.0),
            (200.0, 1, 1, 0.125, 0.125, 1.0),
            (MAX_PX_PER_BEAT, 1, 1, 0.0625, 0.0625, 1.0),
        ];
        for (ppb, label, tick, tick_beats, line, band) in rows {
            let s = steps(ppb);
            assert_eq!(
                (s.label_bars, s.tick_bars, s.tick_beats, s.line_beats, s.band_beats),
                (label, tick, tick_beats, line, band),
                "ppb {ppb}"
            );
        }
    }

    #[test]
    fn ruler_steps_keep_their_minimum_spacing() {
        let mut ppb = MIN_PX_PER_BEAT;
        while ppb <= MAX_PX_PER_BEAT {
            let s = steps(ppb);
            let ref_px = ppb / REF;
            assert!(s.label_bars as f64 * 4.0 * ref_px >= LABEL_MIN_PX - 1e-9 || s.label_bars == 1024, "labels at {ppb}");
            assert!(s.line_beats * ref_px >= GRID_BAR_MIN_PX, "grid at {ppb}");
            assert!(s.line_beats >= s.tick_beats && s.tick_bars * 2 >= s.label_bars, "ticks at {ppb}");
            assert!(s.band_beats * ref_px >= BAND_MIN_PX || s.band_beats == 64.0, "band at {ppb}");
            ppb *= 1.07;
        }
    }

    #[test]
    fn ruler_steps_follow_the_time_signature() {
        let s = RulerSteps::new(DEFAULT_PX_PER_BEAT, 3.0, 0.5, None);
        assert_eq!((s.label_bars, s.tick_bars, s.tick_beats, s.line_beats), (2, 1, 1.5, 1.5));
        let s = RulerSteps::new(DEFAULT_PX_PER_BEAT, 3.0, 1.0, None);
        assert_eq!((s.label_bars, s.tick_beats, s.line_beats), (2, 3.0, 3.0));
    }

    #[test]
    fn summary_rows_fit_the_lane() {
        // A full 74.8 pt lane with two tracks: 8.5 apart, 6.8 tall, centred.
        let (top, pitch, h) = summary_rows(74.3, 2);
        assert!((pitch - 8.5).abs() < 1e-4 && (h - 6.8).abs() < 1e-4, "{pitch} {h}");
        assert!((top - (74.3 - 17.0) / 2.0).abs() < 1e-4);
        // Many tracks in a short lane: rows shrink to share it, never under one px.
        let (top, pitch, h) = summary_rows(20.0, 12);
        assert!(top >= 1.6 - 1e-4 && top + 12.0 * pitch <= 20.0 - 1.6 + 1e-3, "{top} {pitch}");
        assert!(h >= 0.85);
    }

    #[test]
    fn ruler_ticks_in_the_draw_list_follow_the_steps() {
        let w = 969.5f32;
        let count = |ppb: f64| -> (usize, usize, usize) {
            let view = View { px_per_beat: ppb, ..View::default() };
            let sc = Scene {
                steps: steps(ppb),
                view,
                size: (w, 300.0),
                scale: 2.0,
                tracks: &[],
                clips: &[],
                summary: &[],
                live: &[],
                playhead_beats: 0.0,
                loop_beats: (0.0, 0.0),
                loop_on: false,
                beats_per_bar: 4.0,
                auto: None,
                sections: &[],
            };
            let mut dl = DrawList::default();
            build(&mut dl, &sc);
            let by = |grey: u32, h: f32| dl.over.iter().filter(|r| (r.color[0] * 255.0).round() as u32 == grey && r.rect[3] == h).count();
            (by(0x4c, 33.0), by(0x38, 10.0), by(0x38, 7.0))
        };
        let expect = |ppb: f64| -> (usize, usize, usize) {
            let s = steps(ppb);
            let reach = w as f64 / ppb;
            let full = s.tick_bars as f64 * 4.0;
            let fulls = (reach / full).floor() as usize + 1;
            if !s.ticks_visible {
                return (fulls, 0, 0);
            }
            let (mut beat, mut sub) = (0, 0);
            let mut i = 0;
            while i as f64 * s.tick_beats <= reach {
                let b = i as f64 * s.tick_beats;
                i += 1;
                if RulerSteps::on(b, full) {
                    continue;
                }
                if RulerSteps::on(b, 1.0) {
                    beat += 1;
                } else {
                    sub += 1;
                }
            }
            (fulls, beat, sub)
        };
        for ppb in [
            DEFAULT_PX_PER_BEAT / 8.0,
            DEFAULT_PX_PER_BEAT / 2.0,
            DEFAULT_PX_PER_BEAT,
            DEFAULT_PX_PER_BEAT * 4.0,
            DEFAULT_PX_PER_BEAT * 16.0,
        ] {
            assert_eq!(count(ppb), expect(ppb), "ppb {ppb}");
        }
    }

    #[test]
    fn thresholds_are_in_reference_pixels() {
        // The default is 15 px per beat (100%-zoom px); one beat is 16 px wide from 16 / 15 of that zoom.
        let ppb = |create_px: f64| create_px * REF;
        assert_eq!(steps(ppb(15.9)).tick_beats, 2.0);
        assert_eq!(steps(ppb(16.1)).tick_beats, 1.0);
        assert_eq!(steps(ppb(12.4)).label_bars, 2, "a 4/4 bar of 49.6 px");
        assert_eq!(steps(ppb(12.6)).label_bars, 1, "a bar of 50.4 px");
    }

    #[test]
    fn a_fixed_grid_overrides_auto_when_finer() {
        let z = DEFAULT_PX_PER_BEAT;
        assert_eq!(steps(z).tick_beats, 2.0);
        assert_eq!(fixed(z, GridSize::Quarter).tick_beats, 1.0);
        assert_eq!(fixed(z, GridSize::Eighth).tick_beats, 0.5);
        assert_eq!(fixed(z, GridSize::HalfBar).tick_beats, 2.0);
        assert_eq!(fixed(z, GridSize::Bar).tick_beats, 2.0, "coarser than auto: auto stays");
        assert_eq!(fixed(z, GridSize::Quarter).line_beats, 1.0);
        assert_eq!(fixed(z * 8.0, GridSize::Sixteenth).tick_beats, 0.25);
    }

    #[test]
    fn a_fixed_grid_too_dense_is_doubled_to_six_px() {
        let z = DEFAULT_PX_PER_BEAT;
        assert_eq!(fixed(z, GridSize::ThirtySecond).tick_beats, 0.5);
        assert_eq!(fixed(z, GridSize::Sixteenth).tick_beats, 0.5);
        assert_eq!(fixed(z / 8.0, GridSize::Bar).tick_beats, 4.0);
    }

    #[test]
    fn triplets_override_and_stay_on_the_bar() {
        let z = DEFAULT_PX_PER_BEAT * 8.0;
        let third = fixed(z, GridSize::QuarterTriplet);
        assert!((third.tick_beats - 2.0 / 3.0).abs() < 1e-9);
        assert!((fixed(z, GridSize::SixteenthTriplet).tick_beats - 1.0 / 6.0).abs() < 1e-9);
        assert!((fixed(DEFAULT_PX_PER_BEAT, GridSize::EighthTriplet).tick_beats - 2.0 / 3.0).abs() < 1e-9, "doubled once to 6 px");
        assert!(
            (RulerSteps::new(z, 3.0, 1.0, Some(GridSize::QuarterTriplet)).tick_beats - 1.0 / 3.0).abs() < 1e-9,
            "3/4 is not a whole number of 2/3 beats: halved to 1/3"
        );
    }

    #[test]
    fn snap_rounds_to_the_step_and_moves_keep_an_off_grid_offset() {
        let s = fixed(DEFAULT_PX_PER_BEAT, GridSize::Quarter);
        assert_eq!(s.snap(3.4), 3.0);
        assert_eq!(s.snap(3.6), 4.0);
        assert_eq!(s.snap(-2.0), 0.0);
        assert_eq!(s.snap_move(5.2, 4.0), 5.0);
        assert!((s.snap_move(5.4, 4.25) - 5.25).abs() < 1e-9, "a clip that started 0.25 off the grid keeps that offset");
        assert_eq!(s.snap_move(5.9, 4.25), 6.0, "nearer to the grid than to the kept offset");
    }

    #[test]
    fn ruler_labels_thin_out_with_zoom() {
        let label_nums = |ppb: f64| -> Vec<String> {
            let v = View { px_per_beat: ppb, ..View::default() };
            ruler_labels(&v, 1000.0, &steps(ppb)).into_iter().map(|(_, t)| t).collect()
        };
        assert_eq!(&label_nums(DEFAULT_PX_PER_BEAT)[..4], ["1", "2", "3", "4"]);
        assert_eq!(&label_nums(DEFAULT_PX_PER_BEAT / 4.0)[..4], ["1", "5", "9", "13"]);
        assert_eq!(&label_nums(DEFAULT_PX_PER_BEAT * 8.0)[..3], ["1", "2", "3"]);
    }

    #[test]
    fn empty_space_above_pinned_rows_hits_no_track() {
        // One ordinary track and one pinned return: the space between them is no track.
        let v = View { pitches: vec![TRACK_PITCH; 2], pinned_from: 1, pinned_h: TRACK_PITCH, height: 600.0, ..View::default() };
        assert_eq!(v.track_at(RULER_H + 1.0), Some(0));
        assert_eq!(v.track_at(RULER_H + 2.0 * TRACK_PITCH + 10.0), None);
        assert_eq!(v.track_at(v.pinned_top() - 1.0), None);
        assert_eq!(v.track_at(v.pinned_top() + 1.0), Some(1));
        assert_eq!(hit_test(&v, (900.0, 600.0), 2, &[], 10.0, 120.0), Hit::Empty);
    }

    #[test]
    fn view_roundtrip_and_hits() {
        let v = View::default();
        assert!((v.x_of(4.0) - 51.03).abs() < 0.01);
        assert!((v.beat_at(v.x_of(4.0)) - 4.0).abs() < 1e-4);
        assert_eq!(v.track_at(RULER_H + 1.0), Some(0));
        assert_eq!(v.track_at(RULER_H + TRACK_PITCH + 1.0), Some(1));
        let clips = vec![ClipVis {
            track_index: 0,
            start_beats: 4.0,
            len_beats: 4.0,
            color: [1.0; 3],
            selected: false,
            muted: false,
            name: String::new(),
            wave: None,
            midi: None,
            live: None,
            fades: (0.0, 0.0),
            fade_handles: false,
        }];
        let size = (900.0, 400.0);
        assert_eq!(hit_test(&v, size, 2, &clips, 10.0, 5.0), Hit::Ruler);
        assert_eq!(hit_test(&v, size, 2, &clips, 70.0, 60.0), Hit::Clip { index: 0, zone: Zone::Body });
        assert_eq!(hit_test(&v, size, 2, &clips, 52.0, 60.0), Hit::Clip { index: 0, zone: Zone::Left });
        assert_eq!(hit_test(&v, size, 2, &clips, 101.0, 60.0), Hit::Clip { index: 0, zone: Zone::Right });
        assert_eq!(hit_test(&v, size, 2, &clips, 10.0, 60.0), Hit::Track(0));
        assert_eq!(hit_test(&v, size, 2, &clips, 10.0, RULER_H + 2.0 * TRACK_PITCH + 5.0), Hit::Empty);
        assert_eq!(hit_test(&v, size, 2, &clips, 10.0, 395.0), Hit::Master);
    }
}
