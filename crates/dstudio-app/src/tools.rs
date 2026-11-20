//! Timeline edit tools (spec 012): the tool menu in the timeline header (Selector, Marquee tool,
//! Scissors), T to toggle Scissors, the Marquee tool's time range across tracks and what the
//! edit keys do with it, and the Scissors click that splits a clip. The tool and the range are
//! view state: never saved, never undoable. The edits go through `dstudio_core::range`.

use crate::Studio;
use crate::controller::Controller;
use crate::timeline::{self, Hit};
use dstudio_core::range::{self, Piece, TimeRange};
use dstudio_core::{ClipId, Command, PPQ, Ticks};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Tool {
    #[default]
    Selector,
    Marquee,
    Scissors,
}

impl Tool {
    pub const ALL: [Tool; 3] = [Tool::Selector, Tool::Marquee, Tool::Scissors];

    pub fn index(self) -> i32 {
        Tool::ALL.iter().position(|t| *t == self).unwrap_or(0) as i32
    }
}

/// A Marquee drag in progress: where it started (beat, track index) and where it is now.
#[derive(Debug, Clone, Copy)]
struct RangeDrag {
    from: (f64, usize),
    to: (f64, usize),
    moved: bool,
}

#[derive(Default)]
pub struct State {
    pub tool: Tool,
    /// The tool T goes back to from Scissors.
    prev: Tool,
    /// The menu is open, anchored at the button's bottom-left (window units).
    pub menu: Option<(f32, f32)>,
    /// The Marquee tool's selected range.
    pub range: Option<TimeRange>,
    drag: Option<RangeDrag>,
    /// Scissors hover: the lane x of the cut line and the hovered clip's top and height.
    pub cut_line: Option<(f32, f32, f32)>,
    /// What ⌘C / ⌘X of a range copied: the pieces and the range start they are laid out from.
    pub clipboard: Option<(Vec<Piece>, Ticks)>,
}

impl Controller {
    pub fn tool_menu_toggle(&mut self, x: f32, y: f32) {
        self.tools.menu = if self.tools.menu.is_some() { None } else { Some((x, y)) };
        self.ui_dirty = true;
    }

    pub fn tool_menu_dismiss(&mut self) {
        self.tools.menu = None;
        self.ui_dirty = true;
    }

    /// Pick a tool (menu row index or a key). Leaving the Marquee tool drops its range.
    pub fn set_tool(&mut self, tool: Tool) {
        self.tools.menu = None;
        if tool != self.tools.tool {
            self.tools.prev = self.tools.tool;
            self.tools.tool = tool;
        }
        if tool != Tool::Marquee {
            self.range_clear();
        }
        self.tools.cut_line = None;
        self.ui_dirty = true;
    }

    /// T: Scissors, or back to the tool used before them.
    pub fn toggle_scissors(&mut self) {
        let next = match (self.tools.tool, self.tools.prev) {
            (Tool::Scissors, Tool::Scissors) => Tool::Selector,
            (Tool::Scissors, prev) => prev,
            _ => Tool::Scissors,
        };
        self.set_tool(next);
    }

    pub fn range_clear(&mut self) {
        if self.tools.range.take().is_some() || self.tools.drag.take().is_some() {
            self.ui_dirty = true;
        }
    }

    fn snapped_beat(&self, x: f32) -> f64 {
        let beat = self.view.beat_at(x).max(0.0);
        if self.snap_active() { self.ruler_steps().snap(beat).max(0.0) } else { beat }
    }

    /// A lane press, move or release (kind 0, 1, 2) with the Marquee tool or Scissors: whether
    /// the tool took it. The ruler, the master lane and pinned rows keep their own behaviour.
    pub fn tool_pointer(&mut self, kind: i32, x: f32, y: f32) -> bool {
        if self.tools.tool == Tool::Selector {
            return false;
        }
        if kind != 0 && self.tools.drag.is_none() {
            // Scissors (or a Marquee press the lanes took) has nothing to follow.
            return kind == 1 && self.tools.tool == Tool::Scissors;
        }
        let hit = timeline::hit_test(&self.view, self.lanes_size, self.lane_tracks(), &self.clip_vis, x, y);
        if kind == 0 && matches!(hit, Hit::Ruler | Hit::Master) {
            return false;
        }
        match self.tools.tool {
            Tool::Scissors => {
                if kind == 0
                    && let Hit::Clip { index, .. } = hit
                {
                    let at = (self.snapped_beat(x) * PPQ as f64).round() as Ticks;
                    self.scissors_split(self.clip_ids[index], at);
                }
                true
            }
            Tool::Marquee => {
                let tracks = self.lane_tracks();
                let track = self.view.track_at(y).unwrap_or(usize::MAX).min(tracks.saturating_sub(1));
                let beat = self.snapped_beat(x);
                match kind {
                    0 => {
                        if tracks == 0 {
                            return true;
                        }
                        self.select_clip(None);
                        self.selected_section = None;
                        self.pr.focused = false;
                        self.tools.range = None;
                        self.tools.drag = Some(RangeDrag { from: (beat, track), to: (beat, track), moved: false });
                        self.seek_beats(beat);
                    }
                    1 => {
                        if let Some(d) = self.tools.drag.as_mut() {
                            d.moved |= (beat, track) != d.from;
                            d.to = (beat, track);
                            self.tools.range = self.range_of(self.tools.drag.unwrap());
                        }
                    }
                    _ => {
                        if let Some(d) = self.tools.drag.take() {
                            self.tools.range = if d.moved { self.range_of(d) } else { None };
                            if let Some(r) = &self.tools.range {
                                self.seek_beats(r.start as f64 / PPQ as f64);
                            }
                        }
                    }
                }
                self.ui_dirty = true;
                true
            }
            Tool::Selector => false,
        }
    }

    fn range_of(&self, d: RangeDrag) -> Option<TimeRange> {
        let (b0, b1) = (d.from.0.min(d.to.0), d.from.0.max(d.to.0));
        let (t0, t1) = (d.from.1.min(d.to.1), d.from.1.max(d.to.1));
        let p = self.project();
        let tracks = p.tracks.get(t0..=t1.min(p.tracks.len().saturating_sub(1)))?.iter().map(|t| t.id).collect();
        let ticks = |b: f64| (b * PPQ as f64).round() as Ticks;
        let r = TimeRange { start: ticks(b0), end: ticks(b1), tracks };
        (!r.is_empty()).then_some(r)
    }

    /// Scissors click: cut clip `id` at `at`, select the right part. One undo step.
    pub fn scissors_split(&mut self, id: ClipId, at: Ticks) {
        let new_id = ClipId(self.project().next_id);
        if self.execute(Command::SplitClip { clip: id, at, new_id }) {
            self.select_clip(Some(new_id));
        } else {
            self.set_status("This clip can't be split here (warped, reversed and pattern clips can't be split)", 3);
        }
    }

    /// Lane hover with Scissors: the cut line over the clip under the pointer.
    pub fn tool_hover(&mut self, x: f32, y: f32) {
        if self.tools.tool != Tool::Scissors {
            return;
        }
        let hit = timeline::hit_test(&self.view, self.lanes_size, self.lane_tracks(), &self.clip_vis, x, y);
        let line = match hit {
            Hit::Clip { index, .. } => {
                let (_, cy, _, ch) = self.clip_vis[index].rect(&self.view);
                Some((self.view.x_of(self.snapped_beat(x)), cy, ch))
            }
            _ => None,
        };
        if line != self.tools.cut_line {
            self.tools.cut_line = line;
            self.ui_dirty = true;
        }
    }

    /// Delete / Backspace with a range: remove its content.
    pub fn range_delete(&mut self) {
        let Some(r) = self.tools.range.clone() else { return };
        if let Some(cmd) = range::delete(self.project(), &r) {
            self.execute(cmd);
        }
    }

    /// ⌘C with a range: its content to the range clipboard.
    pub fn range_copy(&mut self) -> bool {
        let Some(r) = self.tools.range.clone() else { return false };
        let pieces = range::copy(self.project(), &r);
        if pieces.is_empty() {
            return false;
        }
        self.tools.clipboard = Some((pieces, r.start));
        true
    }

    /// ⌘X with a range: copy, then delete ("Cut Range").
    pub fn range_cut(&mut self) {
        if !self.range_copy() {
            return;
        }
        let Some(r) = self.tools.range.clone() else { return };
        if let Some(cmd) = range::delete_labelled(self.project(), &r, "Cut Range") {
            self.execute(cmd);
        }
    }

    /// ⌘V after a range copy: the pieces at the playhead (snapped to the beat), what was at the
    /// range's start landing there. Whether it pasted.
    pub fn range_paste(&mut self) -> bool {
        let Some((pieces, anchor)) = self.tools.clipboard.clone() else { return false };
        let at = ((self.playhead_ticks() as f64 / PPQ as f64).round() as Ticks * PPQ).max(0);
        let p = self.project();
        let (cmds, ids) = range::add_pieces(p, &pieces, at - anchor, p.next_id);
        if cmds.is_empty() {
            return false;
        }
        let label = if cmds.len() == 1 { "Paste Clip".to_string() } else { format!("Paste {} Clips", cmds.len()) };
        if self.execute(Command::Batch { label, commands: cmds }) {
            self.set_clip_selection(ids);
        }
        true
    }

    /// ⌘D with a range: its content again right after it; the range moves onto the copy.
    pub fn range_duplicate(&mut self) {
        let Some(r) = self.tools.range.clone() else { return };
        let Some((cmd, _)) = range::duplicate(self.project(), &r) else { return };
        if self.execute(cmd) {
            let len = r.len();
            self.tools.range = Some(TimeRange { start: r.start + len, end: r.end + len, tracks: r.tracks });
            self.ui_dirty = true;
        }
    }

    /// ⌘E with a range: split the clips crossing its edges.
    pub fn range_split(&mut self) {
        let Some(r) = self.tools.range.clone() else { return };
        if let Some(cmd) = range::split(self.project(), &r) {
            self.execute(cmd);
        }
    }

    /// ⌘L with a range: loop it.
    pub fn range_loop(&mut self) {
        let Some(r) = self.tools.range.clone() else { return };
        let tr = &self.project().transport;
        if tr.loop_enabled && tr.loop_start == r.start && tr.loop_end == r.end {
            return;
        }
        self.doc.seal_coalescing();
        self.execute(Command::SetLoop { enabled: true, start: r.start, end: r.end });
    }

    /// The range's rectangle in lane coordinates: x, y, w, h.
    fn range_rect(&self) -> Option<(f32, f32, f32, f32)> {
        let r = self.tools.range.as_ref()?;
        let p = self.project();
        let idx: Vec<usize> = r.tracks.iter().filter_map(|t| p.track_index(*t)).collect();
        let (lo, hi) = (*idx.iter().min()?, *idx.iter().max()?);
        let x0 = self.view.x_of(r.start as f64 / PPQ as f64);
        let x1 = self.view.x_of(r.end as f64 / PPQ as f64);
        let y0 = self.view.track_top(lo);
        let y1 = self.view.track_top(hi) + self.view.pitch(hi) - 0.5;
        Some((x0, y0, x1 - x0, y1 - y0))
    }

    pub(crate) fn push_tools(&self, s: &Studio) {
        s.set_tool(self.tools.tool.index());
        match self.tools.menu {
            Some((x, y)) => {
                s.set_tool_menu_x(x);
                s.set_tool_menu_y(y);
                s.set_tool_menu_open(true);
            }
            None => s.set_tool_menu_open(false),
        }
        match self.range_rect() {
            Some((x, y, w, h)) => {
                s.set_range_x(x);
                s.set_range_y(y);
                s.set_range_w(w);
                s.set_range_h(h);
                s.set_range_on(true);
            }
            None => s.set_range_on(false),
        }
        match self.tools.cut_line {
            Some((x, y, h)) => {
                s.set_cut_x(x);
                s.set_cut_y(y);
                s.set_cut_h(h);
                s.set_cut_on(true);
            }
            None => s.set_cut_on(false),
        }
    }
}
