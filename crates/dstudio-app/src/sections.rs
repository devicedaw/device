//! Arrangement sections (shown as markers): the lane under the loop strip. A "+" after the last
//! marker adds eight bars, a marker's body drags to reorder it, its right edge resizes it, and a
//! double-click renames it. Sections tile the timeline from bar 1 (see `dstudio_core::Section`).
//! They only label ranges unless Settings › General › Sections move clips is on; then clips
//! and automation move with them (`execute_section`). Every edit is one undo step.
//!
//! The right-click menu: Cut, Copy, Paste (after this marker), Duplicate, Rename…, Color ›,
//! Insert › (Marker before / after), Delete marker, Clear all markers.

use crate::controller::{Controller, MenuAction};
use dstudio_core::section_follow::{self, SectionContent};
use dstudio_core::{Command, PPQ, Section, SectionId, Ticks};

/// Section lane rows in lane-local points (the ruler's third band, between its rules).
pub const LANE_TOP: f32 = 25.0;
pub const LANE_BOTTOM: f32 = 38.0;
/// Block: 14 px tall (lengths here are 100%-zoom px), 1 px below the lane top, 2 px corner radius.
pub const BLOCK_Y: f32 = 25.85;
pub const BLOCK_H: f32 = 11.9;
pub const RADIUS: f32 = 1.7;
/// Edge grab: 5 px inside the end of a section.
const EDGE: f32 = 4.25;
/// "+" centre: 12 px after the last section's end (8.5 pt in from bar 1 when empty).
pub const ADD_OFFSET: f32 = 10.2;
const ADD_HALF: f32 = 6.8;
const NEW_BARS: i64 = 8;

/// The section palette (vivid enough for a dark label), before display tinting.
pub const PALETTE: [u32; 13] =
    [0xc86040, 0xd0a050, 0xb8c040, 0xa0d050, 0x60d890, 0x50b8b0, 0x68a8c0, 0x7888c8, 0x9080d0, 0xb068d0, 0xd85088, 0xc07030, 0x30a098];
/// Names of the [PALETTE] colours in the Color submenu.
pub const PALETTE_NAMES: [&str; 13] =
    ["Brick", "Sand", "Olive", "Lime", "Mint", "Teal", "Sky", "Blue", "Lavender", "Violet", "Rose", "Copper", "Sea"];
/// Insert submenu rows.
pub const INSERT_ROWS: [&str; 2] = ["Marker before", "Marker after"];

#[derive(Debug, Clone)]
pub struct SectionVis {
    pub start_beats: f64,
    pub len_beats: f64,
    pub color: [f32; 3],
    /// 0 rest, 1 selected, 2 dragging.
    pub state: u8,
    pub name: String,
}

#[derive(Debug, Clone, Copy)]
pub enum SectionDrag {
    /// Body drag: reorder. `to` is the preview index.
    Move { id: SectionId, from: usize, to: usize },
    /// Right-edge drag: length preview in ticks.
    Resize { id: SectionId, len: Ticks },
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum SectionHit {
    Add,
    Body(usize),
    Edge(usize),
    Empty,
}

/// Display colour: the palette colour saturated and lifted toward white.
pub fn display_rgb(c: u32) -> [f32; 3] {
    let rgb = crate::timeline::rgb(c);
    let y = 0.2126 * rgb[0] + 0.7152 * rgb[1] + 0.0722 * rgb[2];
    rgb.map(|v| {
        let s = (y + (v - y) * 1.43).clamp(0.0, 1.0);
        s + (1.0 - s) * 0.135
    })
}

/// Selected and dragged blocks lift toward white.
pub fn lift(c: [f32; 3], state: u8) -> [f32; 3] {
    let t = match state {
        1 => 0.08,
        2 => 0.14,
        _ => 0.0,
    };
    c.map(|v| v + (1.0 - v) * t)
}

fn lin(c: f32) -> f32 {
    if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
}
fn unlin(c: f32) -> f32 {
    if c <= 0.003_130_8 { 12.92 * c } else { 1.055 * c.powf(1.0 / 2.4) - 0.055 }
}

/// Label ink: a dark shade of the block's own hue (OKLCH lightness 0.25, chroma at most 0.06).
pub fn label_ink(c: [f32; 3]) -> [f32; 3] {
    let [r, g, b] = c.map(lin);
    let l = (0.412_221_47 * r + 0.536_332_55 * g + 0.051_445_99 * b).cbrt();
    let m = (0.211_903_5 * r + 0.680_699_5 * g + 0.107_396_96 * b).cbrt();
    let s = (0.088_302_46 * r + 0.281_718_84 * g + 0.629_978_7 * b).cbrt();
    let a = 1.977_998_5 * l - 2.428_592_2 * m + 0.450_593_7 * s;
    let bb = 0.025_904_04 * l + 0.782_771_77 * m - 0.808_675_77 * s;
    let chroma = (a * a + bb * bb).sqrt();
    let k = if chroma > 0.0 { (0.45 * chroma).min(0.06) / chroma } else { 0.0 };
    let (lo, a, bb) = (0.25, a * k, bb * k);
    let l = (lo + 0.396_337_78 * a + 0.215_803_76 * bb).powi(3);
    let m = (lo - 0.105_561_35 * a - 0.063_854_17 * bb).powi(3);
    let s = (lo - 0.089_484_18 * a - 1.291_485_5 * bb).powi(3);
    [
        4.076_741_7 * l - 3.307_711_6 * m + 0.230_969_93 * s,
        -1.268_438 * l + 2.609_757_4 * m - 0.341_319_4 * s,
        -0.004_196_086 * l - 0.703_418_6 * m + 1.707_614_7 * s,
    ]
    .map(|v| unlin(v.clamp(0.0, 1.0)))
}

impl Controller {
    fn bar_ticks(&self) -> Ticks {
        self.project().ticks_per_bar()
    }

    /// Sections as displayed: (id, start, length) in timeline order, with any drag preview.
    pub fn section_layout(&self) -> Vec<(SectionId, Ticks, Ticks)> {
        let p = self.project();
        let mut order: Vec<&Section> = p.sections.iter().collect();
        if let Some(SectionDrag::Move { from, to, .. }) = self.section_drag
            && from < order.len()
        {
            let s = order.remove(from);
            order.insert(to.min(order.len()), s);
        }
        let mut start = 0;
        order
            .into_iter()
            .map(|s| {
                let len = match self.section_drag {
                    Some(SectionDrag::Resize { id, len }) if id == s.id => len,
                    _ => s.length,
                };
                let out = (s.id, start, len);
                start += len;
                out
            })
            .collect()
    }

    pub fn build_section_vis(&self) -> Vec<SectionVis> {
        let p = self.project();
        self.section_layout()
            .into_iter()
            .filter_map(|(id, start, len)| {
                let s = p.section_index(id).map(|i| &p.sections[i])?;
                let dragging =
                    matches!(self.section_drag, Some(SectionDrag::Move { id: d, .. } | SectionDrag::Resize { id: d, .. }) if d == id);
                Some(SectionVis {
                    start_beats: start as f64 / PPQ as f64,
                    len_beats: len as f64 / PPQ as f64,
                    color: display_rgb(s.color),
                    state: if dragging {
                        2
                    } else if self.selected_section == Some(id) {
                        1
                    } else {
                        0
                    },
                    name: s.name.clone(),
                })
            })
            .collect()
    }

    /// Centre x of the "+" (lane-local points).
    pub fn section_add_x(&self) -> f32 {
        match self.section_layout().last() {
            Some(&(_, start, len)) => self.view.x_of((start + len) as f64 / PPQ as f64) + ADD_OFFSET,
            None => self.view.x_of(0.0) + 8.5,
        }
    }

    fn section_hit(&self, x: f32) -> SectionHit {
        if (x - self.section_add_x()).abs() <= ADD_HALF {
            return SectionHit::Add;
        }
        for (i, &(_, start, len)) in self.section_layout().iter().enumerate() {
            let x0 = self.view.x_of(start as f64 / PPQ as f64);
            let x1 = self.view.x_of((start + len) as f64 / PPQ as f64);
            if x >= x1 - EDGE && x <= x1 + 1.0 {
                return SectionHit::Edge(i);
            }
            if x >= x0 && x < x1 {
                return SectionHit::Body(i);
            }
        }
        SectionHit::Empty
    }

    pub fn in_section_lane(y: f32) -> bool {
        (LANE_TOP..LANE_BOTTOM).contains(&y)
    }

    /// Next colour: the first unused palette entry after the last section's colour
    /// (from the middle of the palette when there is none), else the least used.
    fn next_section_color(&self) -> u32 {
        let secs = &self.project().sections;
        let n = PALETTE.len();
        let last = secs.iter().rev().find_map(|s| PALETTE.iter().position(|&c| c == s.color)).unwrap_or(n / 2 - 1);
        (1..=n)
            .map(|k| PALETTE[(last + k) % n])
            .find(|c| secs.iter().all(|s| s.color != *c))
            .unwrap_or_else(|| *PALETTE.iter().min_by_key(|&&c| secs.iter().filter(|s| s.color == c).count()).unwrap())
    }

    /// "Marker N" at `index` (end when `None`), eight bars long. Selects it.
    pub fn insert_section(&mut self, index: Option<usize>) -> Option<SectionId> {
        let p = self.project();
        let id = SectionId(p.next_id);
        let section = Section {
            id,
            name: format!("Marker {}", p.sections.len() + 1),
            color: self.next_section_color(),
            length: NEW_BARS * self.bar_ticks(),
        };
        if self.execute_section(Command::InsertSection { index, section }, Vec::new()) {
            self.selected_section = Some(id);
            self.ui_dirty = true;
            return Some(id);
        }
        None
    }

    /// Run a marker edit. With Settings › General › Sections move clips on, the clips and
    /// automation follow the markers in the same undo step (`section_follow`), and `pastes`
    /// fill new markers with copied contents.
    pub fn execute_section(&mut self, cmd: Command, pastes: Vec<(SectionId, SectionContent)>) -> bool {
        if !self.settings.prefs.general.sections_move_clips {
            return self.execute(cmd);
        }
        let before = self.project();
        let mut after = before.clone();
        let label = cmd.label();
        if cmd.clone().apply(&mut after).is_err() {
            return self.execute(cmd);
        }
        let follow = section_follow::follow_sections(before, &after, &pastes);
        if follow.is_empty() {
            return self.execute(cmd);
        }
        self.execute(Command::Batch { label, commands: std::iter::once(cmd).chain(follow).collect() })
    }

    pub fn delete_section(&mut self, id: SectionId) {
        if self.selected_section == Some(id) {
            self.selected_section = None;
        }
        self.execute_section(Command::RemoveSection { section: id }, Vec::new());
    }

    pub fn clear_sections(&mut self) {
        let commands: Vec<Command> = self.project().sections.iter().map(|s| Command::RemoveSection { section: s.id }).collect();
        if !commands.is_empty() {
            self.selected_section = None;
            self.execute(Command::Batch { label: "Clear All Sections".into(), commands });
        }
    }

    /// Copy right after the original (same name and colour). Selects the copy.
    pub fn duplicate_section(&mut self, id: SectionId) {
        let p = self.project();
        let Some(i) = p.section_index(id) else { return };
        let new_id = SectionId(p.next_id);
        let section = Section { id: new_id, ..p.sections[i].clone() };
        let content = section_follow::capture(p, id).unwrap_or_default();
        if self.execute_section(Command::InsertSection { index: Some(i + 1), section }, vec![(new_id, content)]) {
            self.selected_section = Some(new_id);
        }
    }

    pub fn rename_section(&mut self, id: SectionId, name: &str) {
        self.rename_section = None;
        self.ui_dirty = true;
        let name = name.trim();
        if name.is_empty() || self.project().section_index(id).is_some_and(|i| self.project().sections[i].name == name) {
            return;
        }
        self.execute(Command::RenameSection { section: id, name: name.into() });
    }

    /// Lane press / move / release (lane-local points). Returns true when the section lane
    /// took the gesture.
    pub fn section_pointer(&mut self, kind: i32, x: f32, y: f32) -> bool {
        match kind {
            0 => {
                if !Self::in_section_lane(y) {
                    return false;
                }
                self.select_clip(None);
                match self.section_hit(x) {
                    SectionHit::Add => {
                        self.insert_section(None);
                    }
                    SectionHit::Body(i) => {
                        let (id, start, _) = self.section_layout()[i];
                        self.selected_section = Some(id);
                        self.seek_beats(start as f64 / PPQ as f64);
                        self.section_drag = Some(SectionDrag::Move { id, from: i, to: i });
                    }
                    SectionHit::Edge(i) => {
                        let (id, _, len) = self.section_layout()[i];
                        self.selected_section = Some(id);
                        self.section_drag = Some(SectionDrag::Resize { id, len });
                    }
                    SectionHit::Empty => self.selected_section = None,
                }
                self.ui_dirty = true;
                true
            }
            1 => {
                let Some(drag) = self.section_drag else { return false };
                let beat = self.view.beat_at(x).max(0.0);
                let bar = self.bar_ticks();
                match drag {
                    SectionDrag::Move { id, from, .. } => {
                        // Drop before the first section that starts at or after the bar under the pointer.
                        let at = ((beat * PPQ as f64) as Ticks / bar) * bar;
                        let p = self.project();
                        let mut start = 0;
                        let mut to = p.sections.len();
                        for (i, s) in p.sections.iter().enumerate() {
                            if start >= at {
                                to = i;
                                break;
                            }
                            start += s.length;
                        }
                        let to = if to > from { to - 1 } else { to }.min(p.sections.len().saturating_sub(1));
                        self.section_drag = Some(SectionDrag::Move { id, from, to });
                    }
                    SectionDrag::Resize { id, .. } => {
                        let p = self.project();
                        let Some(i) = p.section_index(id) else { return true };
                        let start = p.section_start(i);
                        let end = ((beat * PPQ as f64) as Ticks + bar / 2) / bar * bar;
                        self.section_drag = Some(SectionDrag::Resize { id, len: (end - start).max(bar) });
                    }
                }
                self.ui_dirty = true;
                true
            }
            _ => {
                let Some(drag) = self.section_drag.take() else { return false };
                match drag {
                    SectionDrag::Move { id, from, to } if from != to => {
                        self.execute_section(Command::MoveSection { section: id, index: to }, Vec::new());
                    }
                    SectionDrag::Resize { id, len }
                        if self.project().section_index(id).is_some_and(|i| self.project().sections[i].length != len) =>
                    {
                        self.execute_section(Command::SetSectionLength { section: id, length: len }, Vec::new());
                    }
                    _ => {}
                }
                self.ui_dirty = true;
                true
            }
        }
    }

    /// Double-click in the lane: rename a section, or add one after the last.
    pub fn section_double(&mut self, x: f32, y: f32) -> bool {
        if !Self::in_section_lane(y) {
            return false;
        }
        match self.section_hit(x) {
            SectionHit::Body(i) | SectionHit::Edge(i) => {
                self.rename_section = Some(self.section_layout()[i].0);
                self.ui_dirty = true;
            }
            SectionHit::Empty => {
                self.insert_section(None);
            }
            SectionHit::Add => {}
        }
        true
    }

    /// Right-click in the lane: the section menu over a section.
    pub fn section_context(&mut self, x: f32, y: f32, wx: f32, wy: f32, win: (f32, f32)) -> bool {
        if !Self::in_section_lane(y) {
            return false;
        }
        let (SectionHit::Body(i) | SectionHit::Edge(i)) = self.section_hit(x) else {
            self.menu_close();
            return true;
        };
        let id = self.section_layout()[i].0;
        self.selected_section = Some(id);
        use MenuAction::*;
        let items = [Cut, Copy, Paste, Duplicate, SectionRename, SectionColor, SectionInsert, SectionDelete, SectionClear];
        let can_paste = self.section_clipboard.is_some();
        self.open_menu(items.iter().map(|&a| (a, a != Paste || can_paste)).collect(), wx, wy, win);
        self.menu.section = Some(id);
        true
    }

    pub fn section_menu_activate(&mut self, id: SectionId, action: MenuAction) {
        if matches!(action, MenuAction::SectionColor | MenuAction::SectionInsert) {
            self.menu.sub_open = !(self.menu.sub_open && self.menu.sub_for == Some(action));
            self.menu.sub_for = Some(action);
            self.menu.section = Some(id);
            self.ui_dirty = true;
            return;
        }
        self.menu_close();
        match action {
            MenuAction::Cut => self.cut_section(id),
            MenuAction::Copy => self.copy_section(id),
            MenuAction::Paste => self.paste_section(Some(id)),
            MenuAction::Duplicate => self.duplicate_section(id),
            MenuAction::SectionRename => {
                self.rename_section = Some(id);
                self.ui_dirty = true;
            }
            MenuAction::SectionDelete => self.delete_section(id),
            MenuAction::SectionClear => self.clear_sections(),
            _ => {}
        }
    }

    /// Submenu pick on a marker: a colour, or a marker before / after it.
    pub fn section_submenu_activate(&mut self, id: SectionId, index: usize) {
        let sub = self.menu.sub_for;
        self.menu_close();
        let Some(i) = self.project().section_index(id) else { return };
        match sub {
            Some(MenuAction::SectionColor) => {
                let Some(&color) = PALETTE.get(index) else { return };
                if self.project().sections[i].color != color {
                    self.execute(Command::SetSectionColor { section: id, color });
                }
            }
            Some(MenuAction::SectionInsert) if index < INSERT_ROWS.len() => {
                self.insert_section(Some(i + index));
            }
            _ => {}
        }
    }

    /// Submenu rows for a marker's Color (a swatch per palette colour, the marker's own ticked)
    /// and Insert entries.
    pub fn section_submenu_rows(&self) -> (Vec<crate::MenuItem>, f32) {
        let p = self.project();
        let cur = self.menu.section.and_then(|id| p.section_index(id)).map(|i| p.sections[i].color);
        if self.menu.sub_for == Some(MenuAction::SectionColor) {
            let (mut rows, h) = crate::clipedit::layout_items(
                PALETTE_NAMES
                    .iter()
                    .zip(PALETTE)
                    .map(|(n, c)| (n.to_string(), if cur == Some(c) { "✓".into() } else { String::new() }, true, false, false)),
            );
            for (r, c) in rows.iter_mut().zip(PALETTE) {
                let rgb = display_rgb(c);
                r.swatch = slint::Color::from_rgb_f32(rgb[0], rgb[1], rgb[2]);
            }
            return (rows, h);
        }
        crate::clipedit::layout_items(INSERT_ROWS.iter().map(|l| (l.to_string(), String::new(), true, false, false)))
    }

    /// ⌘X / ⌘C / ⌘V act on the selected marker when no clip is selected.
    pub fn marker_shortcut_target(&self) -> Option<SectionId> {
        if !self.selected_clips().is_empty() {
            return None;
        }
        self.selected_section.filter(|s| self.project().section_index(*s).is_some())
    }

    /// Copy a marker (name, colour and length, and what is in it) for Paste.
    pub fn copy_section(&mut self, id: SectionId) {
        let p = self.project();
        if let (Some(i), Some(content)) = (p.section_index(id), section_follow::capture(p, id)) {
            self.section_clipboard = Some((p.sections[i].clone(), content));
        }
    }

    pub fn cut_section(&mut self, id: SectionId) {
        self.copy_section(id);
        if self.section_clipboard.as_ref().is_some_and(|(s, _)| s.id == id) {
            self.delete_section(id);
        }
    }

    /// Paste the copied marker after `after` (the end when None). Selects the copy.
    pub fn paste_section(&mut self, after: Option<SectionId>) {
        let Some((copy, content)) = self.section_clipboard.clone() else { return };
        let p = self.project();
        let index = after.and_then(|a| p.section_index(a)).map(|i| i + 1);
        let id = SectionId(p.next_id);
        if self.execute_section(Command::InsertSection { index, section: Section { id, ..copy } }, vec![(id, content)]) {
            self.selected_section = Some(id);
            self.ui_dirty = true;
        }
    }
}

#[cfg(test)]
mod menu_tests {
    use super::*;

    #[test]
    fn palette_names_match_the_palette() {
        assert_eq!(PALETTE_NAMES.len(), PALETTE.len());
        let mut seen = std::collections::HashSet::new();
        assert!(PALETTE_NAMES.iter().all(|n| seen.insert(*n)));
    }
}
