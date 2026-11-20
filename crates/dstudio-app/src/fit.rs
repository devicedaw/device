//! Zoom to fit (View menu, W and H): W fits the song (or the selected clips) to the width of
//! the lanes, H fits every track row into their height. View state only: not undoable.

use crate::controller::Controller;
use crate::timeline::{ClipVis, DEFAULT_PX_PER_BEAT, MAX_PX_PER_BEAT, MIN_PX_PER_BEAT, RULER_H, TRACK_PITCH};
use dstudio_core::Track;

/// Shortest span W zooms to (one 4/4 bar), so a single short clip doesn't fill the screen.
const MIN_FIT_BEATS: f64 = 4.0;
/// Empty project: W shows the first eight bars.
const EMPTY_FIT_BEATS: f64 = 32.0;
/// Room left on each side of the fitted range, as a fraction of the lanes' width.
const MARGIN: f64 = 0.02;

/// The range W fits (start, end beats): the selected clips if any, else every clip from the
/// song start.
pub fn fit_range(clips: &[ClipVis]) -> (f64, f64) {
    let sel: Vec<&ClipVis> = clips.iter().filter(|c| c.selected).collect();
    if !sel.is_empty() {
        let start = sel.iter().map(|c| c.start_beats).fold(f64::INFINITY, f64::min);
        let end = sel.iter().map(|c| c.start_beats + c.len_beats).fold(0.0, f64::max);
        return (start, end);
    }
    let end = crate::minimap::content_end(clips);
    (0.0, if end > 0.0 { end } else { EMPTY_FIT_BEATS })
}

/// Start beat and pixels per beat that show beats `range` across `lanes_w` px.
pub fn fit_horizontal(range: (f64, f64), lanes_w: f32) -> (f64, f64) {
    let w = lanes_w.max(1.0) as f64;
    let span = (range.1 - range.0).max(MIN_FIT_BEATS);
    let ppb = (w * (1.0 - 2.0 * MARGIN) / span).clamp(MIN_PX_PER_BEAT, MAX_PX_PER_BEAT);
    // Centre the range, never before the song start.
    let start = (range.0 + (range.1 - range.0) / 2.0 - w / ppb / 2.0).max(0.0);
    (start, ppb)
}

/// Height scale for each of `rows` rows (with `extra` px of automation rows between them) to
/// fill `room` px, within [Track::HEIGHT_RANGE].
pub fn fit_vertical(rows: usize, extra: f32, room: f32) -> f32 {
    let (lo, hi) = Track::HEIGHT_RANGE;
    if rows == 0 {
        return 1.0;
    }
    ((room - extra).max(0.0) / rows as f32 / TRACK_PITCH).clamp(lo, hi)
}

/// The timeline header's zoom sliders (log scales). Horizontal: half the default zoom at the left
/// end to the closest zoom at the right, so the default sits near the left as in a new project;
/// vertical: the track height scale from 0.75 to the tallest row. ⌘-scroll and ⌥-scroll reach
/// past the left ends, where the knobs rest at 0.
pub const H_SLIDER: (f64, f64) = (DEFAULT_PX_PER_BEAT / 2.0, MAX_PX_PER_BEAT);
pub const V_SLIDER: (f32, f32) = (0.75, Track::HEIGHT_RANGE.1);

/// Slider position 0..1 of `v` on the log scale `lo..hi`.
pub fn slider_pos(v: f64, (lo, hi): (f64, f64)) -> f32 {
    ((v.max(lo).ln() - lo.ln()) / (hi.ln() - lo.ln())).clamp(0.0, 1.0) as f32
}

/// Value at slider position `z` on the log scale `lo..hi`.
pub fn slider_value(z: f32, (lo, hi): (f64, f64)) -> f64 {
    (lo.ln() + z.clamp(0.0, 1.0) as f64 * (hi.ln() - lo.ln())).exp()
}

impl Controller {
    /// The header sliders' positions: (vertical, horizontal).
    pub fn timeline_zoom_pos(&self) -> (f32, f32) {
        let h = self.project().tracks.iter().take(self.view.pinned_from).find(|t| !self.hidden_by_fold(t)).map_or(1.0, |t| t.height);
        let v = slider_pos(h as f64, (V_SLIDER.0 as f64, V_SLIDER.1 as f64));
        (v, slider_pos(self.view.px_per_beat, H_SLIDER))
    }

    /// A header slider moved to `z`: axis 0 sets every shown track's height, axis 1 the zoom
    /// (around the middle of the lanes).
    pub fn timeline_zoom(&mut self, axis: i32, z: f32) {
        if axis == 0 {
            let h = slider_value(z, (V_SLIDER.0 as f64, V_SLIDER.1 as f64)) as f32;
            self.set_shown_track_heights(h);
        } else {
            let ppb = slider_value(z, H_SLIDER);
            self.view.zoom_at(ppb / self.view.px_per_beat, self.lanes_size.0 / 2.0);
        }
        self.ui_dirty = true;
    }

    /// Double-click on a header slider: back to the default row height or zoom.
    pub fn timeline_zoom_reset(&mut self, axis: i32) {
        if axis == 0 {
            self.set_shown_track_heights(1.0);
        } else {
            self.view.zoom_at(DEFAULT_PX_PER_BEAT / self.view.px_per_beat, self.lanes_size.0 / 2.0);
        }
        self.ui_dirty = true;
    }

    fn set_shown_track_heights(&mut self, h: f32) {
        let (lo, hi) = Track::HEIGHT_RANGE;
        let p = self.project();
        let n = p.tracks.len().min(self.view.pinned_from);
        let rows: Vec<_> = p.tracks[..n].iter().filter(|t| !self.hidden_by_fold(t)).map(|t| t.id).collect();
        for id in rows {
            let _ = self.doc.set_track_height(id, h.clamp(lo, hi));
        }
        let n = self.lane_tracks();
        self.view.clamp_scroll(n, self.lanes_size.1);
    }

    /// W: the song (or the selected clips) across the lanes.
    pub fn zoom_to_fit_width(&mut self) {
        let (start, ppb) = fit_horizontal(fit_range(&self.clip_vis), self.lanes_size.0);
        self.view.start_beats = start;
        self.view.px_per_beat = ppb;
        self.ui_dirty = true;
    }

    /// ⌥ + scroll: every shown track row taller (scrolling up) or shorter, by the same factor.
    pub fn zoom_track_heights(&mut self, dy: f32) {
        let (lo, hi) = dstudio_core::Track::HEIGHT_RANGE;
        let factor = (dy * 0.01).exp();
        let p = self.project();
        let n = p.tracks.len().min(self.view.pinned_from);
        let rows: Vec<_> = p.tracks[..n].iter().filter(|t| !self.hidden_by_fold(t)).map(|t| (t.id, t.height)).collect();
        for (id, h) in rows {
            let _ = self.doc.set_track_height(id, (h * factor).clamp(lo, hi));
        }
        let n = self.lane_tracks();
        self.view.clamp_scroll(n, self.lanes_size.1);
        self.ui_dirty = true;
    }

    /// H: every shown track row (not the pinned return rows, not the rows of folded groups)
    /// sized to fill the lanes' height, scrolled to the top.
    pub fn zoom_to_fit_height(&mut self) {
        let p = self.project();
        let n = p.tracks.len().min(self.view.pinned_from);
        let shown: Vec<_> = p.tracks[..n].iter().enumerate().filter(|(_, t)| !self.hidden_by_fold(t)).map(|(i, t)| (i, t.id)).collect();
        let extra: f32 = shown.iter().map(|&(i, _)| self.view.extra.get(i).copied().unwrap_or(0.0)).sum();
        // Lanes height less the ruler, the master and pinned rows, and the room under the last row.
        let room = self.lanes_size.1 - RULER_H - self.view.footer_h() - 1.0;
        let scale = fit_vertical(shown.len(), extra, room);
        for (_, id) in shown {
            let _ = self.doc.set_track_height(id, scale);
        }
        self.view.scroll_y = 0.0;
        self.ui_dirty = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clip(start: f64, len: f64, selected: bool) -> ClipVis {
        ClipVis {
            track_index: 0,
            start_beats: start,
            len_beats: len,
            color: [1.0; 3],
            selected,
            muted: false,
            name: String::new(),
            wave: None,
            midi: None,
            live: None,
            fades: (0.0, 0.0),
            fade_handles: false,
        }
    }

    #[test]
    fn w_fits_the_song_or_the_selection() {
        let clips = [clip(0.0, 16.0, false), clip(32.0, 32.0, false)];
        assert_eq!(fit_range(&clips), (0.0, 64.0));
        let clips = [clip(0.0, 16.0, false), clip(32.0, 32.0, true)];
        assert_eq!(fit_range(&clips), (32.0, 64.0));
        assert_eq!(fit_range(&[]), (0.0, EMPTY_FIT_BEATS));
    }

    #[test]
    fn the_fitted_range_spans_the_lanes_with_a_margin() {
        let (start, ppb) = fit_horizontal((0.0, 64.0), 1000.0);
        assert_eq!(start, 0.0);
        assert!((ppb * 64.0 - 960.0).abs() < 1e-9, "{ppb}");
        // A selection in the middle is centred.
        let (start, ppb) = fit_horizontal((32.0, 48.0), 1000.0);
        assert!((start + 1000.0 / ppb / 2.0 - 40.0).abs() < 1e-9);
        // A one-beat clip still shows a bar.
        let (_, ppb) = fit_horizontal((8.0, 9.0), 1000.0);
        assert!((ppb * MIN_FIT_BEATS - 960.0).abs() < 1e-9);
    }

    #[test]
    fn h_shares_the_height_within_the_row_limits() {
        assert!((fit_vertical(2, 0.0, 2.0 * TRACK_PITCH * 1.5) - 1.5).abs() < 1e-6);
        assert!((fit_vertical(2, TRACK_PITCH, 3.0 * TRACK_PITCH) - 1.0).abs() < 1e-6);
        assert_eq!(fit_vertical(100, 0.0, 300.0), Track::HEIGHT_RANGE.0);
        assert_eq!(fit_vertical(1, 0.0, 1e6), Track::HEIGHT_RANGE.1);
        assert_eq!(fit_vertical(0, 0.0, 300.0), 1.0);
    }
}
