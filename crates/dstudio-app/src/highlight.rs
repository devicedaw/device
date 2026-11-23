//! The single table mapping assistant highlight targets
//! (`dstudio_core::assistant::UI_TARGETS` and `track:<id>:<part>`) to window
//! rectangles in OUR layout (window points). It mirrors the geometry in
//! `ui/app.slint`; keep both in sync. Targets that are not on screen in the
//! current layout (hidden panel, scrolled-out track) return `None`, and the
//! UI falls back to showing only the note text.

use crate::timeline::{MASTER_H, TRACK_H};

/// Lanes start this far into the Timeline panel, right of the track headers (`Tokens.lane-x`).
pub const LANE_X: f32 = 191.0;

/// Inputs that move things around.
#[derive(Debug, Clone, Copy)]
pub struct Layout {
    pub w: f32,
    pub h: f32,
    pub sounds: bool,
    pub mixer: bool,
    pub assistant: bool,
    /// Docked assistant width (src/splitters.rs `Controller::chat_w`).
    pub chat_w: f32,
    /// Bottom panel shows the Clip tab (no inspector).
    pub clip_tab: bool,
    pub scroll_y: f32,
    /// Bottom panel height (src/splitters.rs `Controller::bottom_h`).
    pub bottom: f32,
}

pub type Rect = (f32, f32, f32, f32);

// Panel geometry (app.slint `AppWindow`).
pub const MIXER_BOTTOM_H: f32 = 390.0;
pub const DEFAULT_BOTTOM_H: f32 = 306.0;
pub const ASSISTANT_GAP: f32 = 5.0;

impl Layout {
    pub fn bottom_h(&self) -> f32 {
        self.bottom
    }
    /// Height of the browser/timeline row (y 78 ..).
    pub fn top_h(&self) -> f32 {
        self.h - 88.4 - self.bottom_h()
    }
    pub fn bottom_y(&self) -> f32 {
        self.h - 5.0 - self.bottom_h()
    }
    pub fn timeline_x(&self) -> f32 {
        if self.sounds { 262.65 } else { 5.0 }
    }
    /// Window x where the lanes start (right of the track headers, `Tokens.lane-x`).
    pub fn lanes_x(&self) -> f32 {
        self.timeline_x() + LANE_X
    }
    pub fn timeline_w(&self) -> f32 {
        let right = if self.assistant { self.chat_w + ASSISTANT_GAP } else { 0.0 };
        self.w - self.timeline_x() - 5.0 - right
    }
}

/// Window rect of a highlight target. `track_row` resolves `track:<id>:…` ids to the track's
/// content offset and row height.
pub fn target_rect(l: &Layout, target: &str, track_row: impl Fn(u64) -> Option<(f32, f32)>) -> Option<Rect> {
    let w = l.w;
    let (tx, tw) = (l.timeline_x(), l.timeline_w());
    let by = l.bottom_y();
    // The transport block stays centred (app.slint `Header`): x for a 1440-wide window + cx.
    let cx = (w - 1440.0) / 2.0;
    let r = match target {
        // ---- header (x anchored left, toggles anchored right) ----
        "header.sounds_toggle" => (67.0, 39.0, 62.0, 25.0),
        "transport.rewind" => (cx + 368.0, 38.0, 28.0, 28.0),
        "transport.play" => (cx + 396.0, 38.0, 28.0, 28.0),
        "transport.record" => (cx + 423.0, 38.0, 28.0, 28.0),
        "transport.loop" => (cx + 470.0, 41.0, 22.0, 22.0),
        "transport.automation_arm" => (cx + 496.0, 41.0, 21.0, 22.0),
        "transport.bar" => (cx + 524.0, 42.0, 48.0, 20.0),
        "transport.time" => (cx + 592.0, 42.0, 88.0, 20.0),
        "transport.bpm" => (cx + 700.0, 42.0, 57.0, 20.0),
        "transport.tap" => (cx + 762.0, 44.0, 23.0, 15.0),
        "transport.key" => (cx + 809.0, 42.0, 86.0, 20.0),
        "transport.time_signature" => (cx + 913.0, 42.0, 62.0, 20.0),
        "transport.metronome" => (cx + 981.0, 41.0, 22.0, 22.0),
        "header.master_volume" => (cx + 1020.0, 41.0, 100.0, 22.0),
        "header.assistant_toggle" => (w - 147.0, 39.0, 25.0, 25.0),
        "header.mixer_toggle" => (w - 119.0, 39.0, 25.0, 25.0),
        "header.editor_toggle" => (w - 91.0, 39.0, 25.0, 25.0),
        "header.musical_typing" => (w - 63.0, 39.0, 25.0, 25.0),
        "header.settings" => (w - 36.0, 39.0, 25.0, 25.0),
        // ---- browser (only while visible) ----
        "browser.search" if l.sounds => (13.0, 82.0, 38.0, 19.0),
        "browser.library" if l.sounds => (53.0, 82.0, 38.0, 19.0),
        "browser.devices" if l.sounds => (92.0, 82.0, 42.0, 19.0),
        "browser.files" if l.sounds => (136.0, 82.0, 27.0, 19.0),
        // ---- timeline header (timeline-panel relative) ----
        "timeline.returns" => (tx + 60.35, 83.0, 59.5, 18.0),
        "timeline.master_toggle" => (tx + 124.1, 83.0, 59.5, 18.0),
        "timeline.overview" => (tx + 200.6, 83.0, (tw - 595.85).max(0.0), 17.0),
        "timeline.smart_fill" => (tx + tw - 385.05, 83.0, 63.75, 18.0),
        "timeline.automation_toggle" => (tx + tw - 314.6, 83.0, 18.0, 18.0),
        "timeline.draw_mode" => (tx + tw - 292.8, 83.0, 17.0, 18.0),
        "timeline.tool" => (tx + tw - 268.6, 83.0, 25.5, 18.0),
        "timeline.vertical_zoom" | "timeline.horizontal_zoom" if crate::splitters::zoom_hidden(w, l.sounds, l.assistant) => return None,
        "timeline.vertical_zoom" => (tx + tw - 242.0, 83.0, 95.0, 18.0),
        "timeline.horizontal_zoom" => (tx + tw - 149.0, 83.0, 94.0, 18.0),
        "timeline.snap" => (tx + tw - 54.4, 83.0, 44.2, 18.0),
        "timeline.add_track" => (tx, 105.5, 184.0, 39.0),
        // Whole areas (the Getting started tour, src/tour.rs).
        "browser.panel" if l.sounds => (5.0, 78.0, 252.45, l.top_h()),
        "timeline.lanes" => (tx + 191.0, 78.0 + 66.5, tw - 191.0, (l.top_h() - 66.5 - MASTER_H).max(0.0)),
        "timeline.ruler" => (tx + 191.0, 105.5, tw - 191.0, 39.0),
        // ---- inspector + devices (bottom panel; hidden while the mixer is open) ----
        "inspector.input" if !l.mixer && !l.clip_tab => (15.0, by + 37.0, 149.0, 19.0),
        "inspector.output" if !l.mixer && !l.clip_tab => (15.0, by + 63.0, 149.0, 18.0),
        "inspector.trim" if !l.mixer && !l.clip_tab => (20.0, by + 120.0, 54.0, 68.0),
        "inspector.width" if !l.mixer && !l.clip_tab => (96.0, by + 120.0, 54.0, 68.0),
        "inspector.pan" if !l.mixer && !l.clip_tab => (20.0, by + 189.0, 54.0, 68.0),
        "inspector.mono" if !l.mixer && !l.clip_tab => (96.0, by + 189.0, 54.0, 68.0),
        "inspector.fader" if !l.mixer && !l.clip_tab => (188.0, by + 35.0, 63.0, 260.0),
        "devices.add_device" if !l.mixer && !l.clip_tab => (270.0, by + 34.5, 182.5, 264.0),
        t if t.starts_with("track:") => return track_rect(l, t, track_row),
        _ => return None,
    };
    Some(r)
}

/// Point ring around a target rect: (x, y, w, h, corner radius), design units (0.85 of
/// 100%-zoom px). The ring pads small targets (<= 40 px) by 5 px and others by 6,
/// draws near-square targets (<= 56 px, sides within 1.25:1) as a circle, gives other rings
/// the target's radius (8 px when it has none) plus the padding, and keeps 2 px inside the window.
pub fn ring(t: Rect, win_w: f32, win_h: f32) -> (f32, f32, f32, f32, f32) {
    let (x, y, w, h) = t;
    let (big, small) = (w.max(h), w.min(h));
    let pad = if big <= 34.0 { 4.25 } else { 5.1 };
    let (rx, ry, rw, rh, rr) = if small > 0.0 && big <= 47.6 && big / small <= 1.25 {
        let d = big + 2.0 * pad;
        (x + w / 2.0 - d / 2.0, y + h / 2.0 - d / 2.0, d, d, d / 2.0)
    } else {
        let (rw, rh) = (w + 2.0 * pad, h + 2.0 * pad);
        (x - pad, y - pad, rw, rh, (6.8 + pad).min(rw / 2.0).min(rh / 2.0))
    };
    let inset = 1.7;
    let (l, t) = (rx.max(inset), ry.max(inset));
    let (r, b) = ((rx + rw).min(win_w - inset), (ry + rh).min(win_h - inset));
    let (cw, ch) = ((r - l).max(0.0), (b - t).max(0.0));
    (l, t, cw, ch, rr.min(cw / 2.0).min(ch / 2.0))
}

/// Where a ring's note bubble sits: (x, y, above). Below the ring with an 8 px gap, left edges
/// aligned; above it (y is then the bubble's bottom) when less than 120 px remain underneath.
pub fn bubble_anchor(ring: (f32, f32, f32, f32, f32), win_h: f32) -> (f32, f32, bool) {
    let gap = 6.8;
    let below = ring.1 + ring.3 + gap;
    if win_h - below < 102.0 { (ring.0, ring.1 - gap, true) } else { (ring.0, below, false) }
}

fn track_rect(l: &Layout, t: &str, track_row: impl Fn(u64) -> Option<(f32, f32)>) -> Option<Rect> {
    let mut it = t.split(':');
    let (_, id, part) = (it.next()?, it.next()?.parse::<u64>().ok()?, it.next()?);
    let (offset, row_h) = track_row(id)?;
    let (tx, tw) = (l.timeline_x(), l.timeline_w());
    let y = 78.0 + 66.5 + offset - l.scroll_y;
    // Header chrome is centred in the row (track_header.slint); offsets below are for 88 px rows.
    let c = y + (row_h - TRACK_H) / 2.0;
    // Visible track area: between the top "Add track" row and the master row.
    let (top, bottom) = (78.0 + 66.5, 78.0 + l.top_h() - MASTER_H);
    if y + row_h <= top || y >= bottom {
        return None;
    }
    let r = match part {
        "header" => (tx, y, 190.5, row_h),
        "mute" => (tx + 28.05, c + 41.65, 10.2, 10.2),
        "solo" => (tx + 41.65, c + 41.65, 10.2, 10.2),
        "arm" => (tx + 55.25, c + 41.65, 10.2, 10.2),
        "pan" => (tx + 87.6, c + 37.4, 18.7, 18.7),
        "volume" => (tx + 115.7, c + 41.65, 68.0, 11.9),
        "lane" => (tx + 191.0, y, tw - 191.0, row_h),
        _ => return None,
    };
    Some(r)
}

#[cfg(test)]
mod tests {
    use super::*;
    use dstudio_core::assistant::UI_TARGETS;

    fn default_layout() -> Layout {
        Layout {
            w: 1440.0,
            h: 820.0,
            sounds: true,
            mixer: false,
            assistant: false,
            chat_w: crate::splitters::CHAT_MIN_W,
            clip_tab: false,
            scroll_y: 0.0,
            bottom: DEFAULT_BOTTOM_H,
        }
    }

    #[test]
    fn every_ui_target_maps_in_the_default_layout() {
        let l = default_layout();
        for t in UI_TARGETS {
            let r = target_rect(&l, t, |_| None).unwrap_or_else(|| panic!("{t} unmapped"));
            assert!(r.0 >= 0.0 && r.1 >= 0.0 && r.0 + r.2 <= 1440.0 && r.1 + r.3 <= 820.0, "{t} {r:?}");
        }
    }

    #[test]
    fn layout_dependent_targets() {
        let mut l = default_layout();
        assert_eq!(target_rect(&l, "transport.bpm", |_| None), Some((700.0, 42.0, 57.0, 20.0)));
        l.mixer = true;
        assert_eq!(target_rect(&l, "inspector.fader", |_| None), None);
        l.assistant = true;
        let ruler = target_rect(&l, "timeline.ruler", |_| None).unwrap();
        assert!((ruler.0 + ruler.2 - (1440.0 - 5.0 - crate::splitters::CHAT_MIN_W - ASSISTANT_GAP)).abs() < 1e-3);
        assert_eq!(
            target_rect(&l, "track:7:volume", |id| (id == 7).then_some((0.0, TRACK_H))),
            Some((262.65 + 115.7, 144.5 + 41.65, 68.0, 11.9))
        );
        assert_eq!(target_rect(&l, "track:7:volume", |_| None), None);
        l.scroll_y = 1000.0;
        assert_eq!(target_rect(&l, "track:7:header", |_| Some((0.0, TRACK_H))), None, "scrolled out");
    }

    #[test]
    fn rings_pad_round_and_clamp() {
        let near = |a: (f32, f32, f32, f32, f32), b: (f32, f32, f32, f32, f32)| {
            [a.0 - b.0, a.1 - b.1, a.2 - b.2, a.3 - b.3, a.4 - b.4].iter().all(|d| d.abs() < 1e-3)
        };
        // wide control: padded rect, radius 8 + 6 (CSS) x 0.85
        assert!(near(ring((700.0, 42.0, 57.0, 20.0), 1440.0, 820.0), (694.9, 36.9, 67.2, 30.2, 11.9)));
        // small square button: circle, 5 px (CSS) padding
        assert!(near(ring((100.0, 100.0, 22.0, 22.0), 1440.0, 820.0), (95.75, 95.75, 30.5, 30.5, 15.25)));
        // clamped at the window edge
        assert_eq!(ring((0.0, 0.0, 100.0, 20.0), 1440.0, 820.0).0, 1.7);
        assert!(bubble_anchor((10.0, 780.0, 50.0, 20.0, 5.0), 820.0).2, "near the bottom: above");
        assert_eq!(bubble_anchor((10.0, 36.9, 50.0, 30.2, 5.0), 820.0), (10.0, 36.9 + 30.2 + 6.8, false));
    }
}
