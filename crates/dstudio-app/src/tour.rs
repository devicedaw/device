//! Assistant › Getting started: a four-step tour for new users (parity roadmap G18).
//!
//! The assistant panel shows a "Getting started" card ("1 of 4", a progress bar, Continue,
//! collapse and close). Continue shows the current step's coach mark: a ring around the area
//! the step is about and a popover with the step's title, one line of help, Back, Next and
//! close. The step is remembered across launches; closing the card hides it for good, and the
//! empty assistant panel offers to start it again.

use crate::HighlightRect;
use crate::highlight::{self, Layout};
use serde::{Deserialize, Serialize};

pub struct Step {
    pub title: &'static str,
    pub body: &'static str,
    /// Highlight target (`highlight::target_rect`).
    pub target: &'static str,
}

pub const STEPS: [Step; 4] = [
    Step { title: "Find a sound", body: "Open Files in the browser and click a sound to hear it.", target: "browser.panel" },
    Step {
        title: "Drag it to the timeline",
        body: "Drop the sound on a track, or below the tracks to make a new one.",
        target: "timeline.lanes",
    },
    Step {
        title: "Add an instrument and draw notes",
        body: "Add an instrument track here, then double-click its lane to draw notes.",
        target: "timeline.add_track",
    },
    Step { title: "Play and export", body: "Press Space to play. Export your song with File › Export Audio.", target: "transport.play" },
];

/// Saved in the preferences file.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TourPrefs {
    /// Current step, 0-based.
    #[serde(default)]
    pub step: u8,
    /// The card was closed (or the tour finished): hidden until restarted.
    #[serde(default)]
    pub closed: bool,
}

impl TourPrefs {
    pub fn is_default(&self) -> bool {
        *self == TourPrefs::default()
    }
}

/// Live tour state (the saved part lives in `TourPrefs`).
#[derive(Debug, Clone, Default)]
pub struct Tour {
    /// The card is folded to its title row.
    pub collapsed: bool,
    /// The current step's coach mark is showing.
    pub coach: bool,
    /// Open the browser at the next interface update (the first step is about it).
    pub show_browser: bool,
}

/// What a tour action changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Change {
    /// Only view state changed.
    View,
    /// The saved state changed (write the preferences).
    Saved,
}

impl Tour {
    pub fn step_index(p: &TourPrefs) -> usize {
        usize::from(p.step).min(STEPS.len() - 1)
    }

    /// Continue on the card: show the current step's coach mark.
    pub fn resume(&mut self) -> Change {
        self.coach = true;
        self.collapsed = false;
        Change::View
    }

    /// Next on the coach mark: the following step, or finish after the last one.
    pub fn next(&mut self, p: &mut TourPrefs) -> Change {
        let i = Self::step_index(p);
        if i + 1 >= STEPS.len() {
            self.finish(p)
        } else {
            p.step = (i + 1) as u8;
            self.coach = true;
            Change::Saved
        }
    }

    /// Back on the coach mark (no-op on the first step).
    pub fn back(&mut self, p: &mut TourPrefs) -> Change {
        let i = Self::step_index(p);
        if i == 0 {
            return Change::View;
        }
        p.step = (i - 1) as u8;
        Change::Saved
    }

    /// Close on the coach mark: hide it, keep the card and its step.
    pub fn hide_coach(&mut self) -> Change {
        self.coach = false;
        Change::View
    }

    /// Close on the card, or Done on the last step: gone until restarted.
    pub fn finish(&mut self, p: &mut TourPrefs) -> Change {
        self.coach = false;
        self.collapsed = false;
        p.closed = true;
        p.step = 0;
        Change::Saved
    }

    /// Start again from the first step, with its coach mark showing.
    pub fn restart(&mut self, p: &mut TourPrefs) -> Change {
        p.closed = false;
        p.step = 0;
        self.collapsed = false;
        self.coach = true;
        Change::Saved
    }

    pub fn toggle_collapsed(&mut self) -> Change {
        self.collapsed = !self.collapsed;
        Change::View
    }
}

/// "1 of 4".
pub fn counter(p: &TourPrefs) -> String {
    format!("{} of {}", Tour::step_index(p) + 1, STEPS.len())
}

/// Coach mark ring and popover anchor for the current step, or None when its area is not on
/// screen in this layout.
pub fn coach_rect(l: &Layout, p: &TourPrefs) -> Option<HighlightRect> {
    let step = &STEPS[Tour::step_index(p)];
    let t = highlight::target_rect(l, step.target, |_| None)?;
    let r = highlight::ring(t, l.w, l.h);
    let (bx, by, above) = coach_anchor(r, l.w, l.h);
    Some(HighlightRect { x: r.0, y: r.1, w: r.2, h: r.3, r: r.4, bx, by, above, title: step.title.into(), note: step.body.into() })
}

/// Popover width in design units (`CoachMark` in ui/assistant.slint).
pub const POPOVER_W: f32 = 238.0;
/// Popover height (title, one or two lines of help, the button row).
pub const POPOVER_H: f32 = 108.8;

/// Where the popover sits beside a ring: to the right of tall areas that leave room (the
/// browser), else below, else above. Returns (x, y, above); y is the popover's bottom when
/// above, as for Point bubbles.
pub fn coach_anchor(ring: (f32, f32, f32, f32, f32), win_w: f32, win_h: f32) -> (f32, f32, bool) {
    let gap = 6.8;
    let (x, y, w, h, _) = ring;
    let right = x + w + gap;
    if h > win_h * 0.4 && right + POPOVER_W <= win_w - gap {
        return (right, y + 13.6, false);
    }
    let below = y + h + gap;
    if below + POPOVER_H <= win_h - gap {
        (x, below, false)
    } else if h > win_h * 0.4 {
        // Tall area with no room around it (the lanes): inside its top-left corner.
        (x + 13.6, y + 13.6, false)
    } else {
        (x, y - gap, true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn walks_forward_and_back_and_finishes() {
        let (mut t, mut p) = (Tour::default(), TourPrefs::default());
        assert_eq!(counter(&p), "1 of 4");
        assert_eq!(t.back(&mut p), Change::View, "Back on the first step does nothing");
        t.resume();
        assert!(t.coach);
        for n in 2..=4 {
            assert_eq!(t.next(&mut p), Change::Saved);
            assert_eq!(counter(&p), format!("{n} of 4"));
        }
        t.back(&mut p);
        assert_eq!(counter(&p), "3 of 4");
        t.next(&mut p);
        // Next on the last step is Done: the card goes away for good.
        t.next(&mut p);
        assert!(p.closed && !t.coach);
        t.restart(&mut p);
        assert!(!p.closed && t.coach);
        assert_eq!(counter(&p), "1 of 4");
    }

    #[test]
    fn closing_the_coach_keeps_the_step_and_closing_the_card_hides_it() {
        let (mut t, mut p) = (Tour::default(), TourPrefs::default());
        t.resume();
        t.next(&mut p);
        assert_eq!(t.hide_coach(), Change::View);
        assert!(!t.coach && !p.closed);
        assert_eq!(counter(&p), "2 of 4");
        t.finish(&mut p);
        assert!(p.closed);
        // A saved step past the end (older or edited file) clamps to the last step.
        let p = TourPrefs { step: 9, closed: false };
        assert_eq!(counter(&p), "4 of 4");
    }

    #[test]
    fn every_step_has_a_coach_mark_in_the_default_layout() {
        let l = Layout {
            w: 1440.0,
            h: 820.0,
            sounds: true,
            mixer: false,
            assistant: true,
            chat_w: crate::splitters::CHAT_MIN_W,
            clip_tab: false,
            scroll_y: 0.0,
            bottom: highlight::DEFAULT_BOTTOM_H,
        };
        for i in 0..STEPS.len() {
            let p = TourPrefs { step: i as u8, closed: false };
            let r = coach_rect(&l, &p).unwrap_or_else(|| panic!("step {i} has no area"));
            // The popover stays inside the window.
            let top = if r.above { r.by - POPOVER_H } else { r.by };
            assert!(r.bx >= 0.0 && r.bx + POPOVER_W <= l.w + 0.5, "step {i} popover x {}", r.bx);
            assert!(top >= 0.0 && top + POPOVER_H <= l.h + 0.5, "step {i} popover y {top}");
        }
    }
}
