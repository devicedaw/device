//! Track header menu (right-click a track header): Rename, Duplicate track, Duplicate with clips,
//! Bounce track in place, Show automation, Group track, Delete track, the add-track rows (Audio,
//! Instrument, Sampler, Drum and Return track, inserted below the track) and Color; the Add track
//! menu with the same add-track rows; and dragging a track header up or down to reorder the tracks.

use crate::Studio;
use crate::controller::{Controller, MenuAction, slint_color, track_color, track_palette_len};
use dstudio_core::{ColorIndex, Command, TrackId};

/// Pointer travel (px) before a press on a track header becomes a reorder drag.
const DRAG_START: f32 = 4.0;
/// Window y of the top of the track header list (the timeline panel at 78.2, rows from 66.5; ui/app.slint).
const LIST_TOP: f32 = 78.2 + 66.5;

/// A press on a track header that may become a reorder drag.
#[derive(Debug, Clone, Copy)]
pub struct TrackDrag {
    pub track: TrackId,
    start_y: f32,
    /// Past [DRAG_START]: the drop line shows at `gap`.
    pub active: bool,
    /// Track index the dragged track would be inserted before (its group's end for the last place).
    pub gap: Option<usize>,
}

/// Index a track moves to when dropped before `gap`, from index `from`: the gap counts the
/// track itself, so a gap below it is one less once it is taken out.
pub fn move_target(from: usize, gap: usize) -> usize {
    if gap > from { gap - 1 } else { gap }
}

impl Controller {
    /// The track header menu's rows for track `id`, in order (also the end of the empty-lane menu).
    pub(crate) fn track_menu_items(&self, id: TrackId) -> Vec<(MenuAction, bool)> {
        let Some(t) = self.project().track(id) else { return Vec::new() };
        let (is_return, is_bus, is_group) = (t.is_return(), t.is_bus(), t.is_group());
        let can_copy = !t.is_bus() || self.project().tracks.iter().filter(|t| t.is_bus()).count() < dstudio_core::MAX_RETURNS;
        // Always offered on audio and instrument tracks; it says why when there is nothing to bounce.
        let can_bounce = !is_bus;
        let _ = is_group;
        use MenuAction::*;
        let mut items = vec![(TrackRename, true), (TrackDuplicate, can_copy), (TrackDuplicateClips, true)];
        if !is_bus {
            items.push((TrackBounce, can_bounce));
        }
        items.push((TrackAutomation, true));
        if !is_return {
            items.push((TrackGroup, true));
        }
        if is_group {
            items.push((TrackUngroup, true));
        }
        if !is_bus {
            items.push((TrackCopyEffects, true));
            if !self.effects_clipboard.is_empty() {
                items.push((TrackPasteEffects, true));
            }
        }
        items.push((TrackDelete, true));
        items.extend(Self::add_track_items());
        items
    }

    /// The add-track rows: Audio, Instrument, Sampler, Drum and Return track.
    pub(crate) fn add_track_items() -> [(MenuAction, bool); 5] {
        use MenuAction::*;
        [(AddAudioTrack, true), (AddInstrumentTrack, true), (AddSamplerTrack, true), (AddDrumTrack, true), (AddReturnTrack, true)]
    }

    /// Right press on a track header at window (wx, wy).
    pub fn open_track_menu(&mut self, id: TrackId, wx: f32, wy: f32) {
        if self.project().track(id).is_none() {
            return;
        }
        self.select_track(id);
        let items = self.track_menu_items(id);
        let win = self.devices.win;
        self.open_menu(items, wx, wy, win);
        self.menu.track = Some(id);
    }

    /// An add-track row: the new track goes right below `below` (the clicked track), or at the end
    /// of the ordinary tracks; a return track always joins the returns. The new track is selected.
    pub fn add_track_action(&mut self, action: MenuAction, below: Option<TrackId>) {
        let at = below.and_then(|t| self.project().track_index(t)).map(|i| i + 1).filter(|&i| i <= self.project().ordinary_track_count());
        match action {
            MenuAction::AddAudioTrack => {
                self.add_audio_track_at(at);
            }
            MenuAction::AddInstrumentTrack => {
                self.add_instrument_track_at(at, None);
            }
            MenuAction::AddSamplerTrack => {
                self.add_instrument_track_at(at, dstudio_core::builtin::find("sampler-steam"));
            }
            MenuAction::AddDrumTrack => {
                self.add_instrument_track_at(at, dstudio_core::builtin::find("drum-rack"));
            }
            MenuAction::AddReturnTrack => {
                self.add_return_track(true);
            }
            _ => {}
        }
    }

    /// Copies of the copied effects at the end of `track`'s chain, with fresh ids (chain members
    /// keep their head, macro and LFO links their targets among the copies). One undo step.
    pub fn paste_effects(&mut self, track: TrackId) {
        let src = self.effects_clipboard.clone();
        if src.is_empty() || self.project().track(track).is_none_or(|t| t.is_bus()) {
            return;
        }
        let first = self.project().next_id;
        let map: std::collections::HashMap<dstudio_core::DeviceId, dstudio_core::DeviceId> =
            src.iter().enumerate().map(|(i, d)| (d.id, dstudio_core::DeviceId(first + i as u64))).collect();
        let commands = src
            .iter()
            .map(|d| {
                let mut d = d.clone();
                d.id = map[&d.id];
                d.chain = d.chain.and_then(|c| map.get(&c).copied());
                d.links.retain_mut(|l| match map.get(&l.target) {
                    Some(t) => {
                        l.target = *t;
                        true
                    }
                    None => false,
                });
                Command::AddDevice { track, index: None, device: d }
            })
            .collect();
        let label = if src.len() == 1 { "Paste Effect" } else { "Paste Effects" };
        self.execute(Command::Batch { label: label.into(), commands });
    }

    /// A new audio track at index `at` (the end of the ordinary tracks when None), selected.
    pub fn add_audio_track_at(&mut self, at: Option<usize>) -> Option<TrackId> {
        let id = TrackId(self.project().next_id);
        let track =
            dstudio_core::Track::new(id, dstudio_core::TrackKind::Audio, "Audio", crate::controller::next_track_color(self.project()));
        if self.execute(Command::AddTrack { index: at, track }) {
            self.select_track(id);
            return Some(id);
        }
        None
    }

    /// A new instrument track at index `at` playing `instrument` (a built-in: the Sampler or the
    /// Drum Rack), or the default instrument when None; named after its instrument. One undo step.
    pub fn add_instrument_track_at(
        &mut self,
        at: Option<usize>,
        instrument: Option<&'static dstudio_core::builtin::DeviceDef>,
    ) -> Option<TrackId> {
        let id = TrackId(self.project().next_id);
        let (mut commands, fallback) = match instrument {
            Some(def) => {
                let track = dstudio_core::Track::new(
                    id,
                    dstudio_core::TrackKind::Instrument,
                    def.name,
                    crate::controller::next_track_color(self.project()),
                );
                let cmds = vec![
                    Command::AddTrack { index: None, track },
                    Command::SetInstrumentPlugin { track: id, device: Some(def.instantiate()) },
                ];
                (cmds, None)
            }
            None => self.new_instrument_track(id),
        };
        if let Some(Command::AddTrack { index, .. }) = commands.first_mut() {
            *index = at;
        }
        let label = match instrument {
            Some(def) => format!("Add {} Track", def.name),
            None => "Add Instrument Track".into(),
        };
        if !self.execute(Command::Batch { label, commands }) {
            return None;
        }
        if let Some(why) = fallback {
            self.set_status(why, 5);
        }
        self.select_track(id);
        Some(id)
    }

    pub fn track_menu_activate(&mut self, id: TrackId, action: MenuAction) {
        self.menu_close();
        match action {
            MenuAction::TrackRename => {
                self.selected_track = Some(id);
                self.open_rename_field();
            }
            MenuAction::TrackDuplicate => self.duplicate_track(id, false),
            MenuAction::TrackDuplicateClips => self.duplicate_track(id, true),
            MenuAction::TrackGroup => {
                self.group_tracks(&[id]);
            }
            MenuAction::TrackUngroup => {
                self.ungroup(id);
            }
            MenuAction::TrackDelete => {
                self.select_clip(None);
                self.selected_section = None;
                self.selected_track = Some(id);
                self.delete_selection();
            }
            MenuAction::TrackBounce => self.bounce_track_in_place(id),
            MenuAction::TrackCopyEffects => {
                self.effects_clipboard = self.project().track(id).map(|t| t.devices.clone()).unwrap_or_default();
                let n = self.effects_clipboard.len();
                self.set_status(format!("Copied {n} effect{}", if n == 1 { "" } else { "s" }), 3);
            }
            MenuAction::TrackPasteEffects => self.paste_effects(id),
            MenuAction::TrackAutomation => self.auto_toggle_shown(),
            a @ (MenuAction::AddAudioTrack
            | MenuAction::AddInstrumentTrack
            | MenuAction::AddSamplerTrack
            | MenuAction::AddDrumTrack
            | MenuAction::AddReturnTrack) => self.add_track_action(a, Some(id)),
            _ => {}
        }
    }

    /// Copy track `id` (devices, automation, sends and mix settings; its clips too with
    /// `with_clips`) into a new track right below it, and select the copy. One undo step.
    pub fn duplicate_track(&mut self, id: TrackId, with_clips: bool) {
        let p = self.project();
        let (Some(i), Some(copy)) = (p.track_index(id), p.track_copy(id, with_clips)) else { return };
        let new = copy.id;
        let label = if with_clips { "Duplicate Track with Clips" } else { "Duplicate Track" };
        let cmd = Command::Batch { label: label.into(), commands: vec![Command::AddTrack { index: Some(i + 1), track: copy }] };
        if self.execute(cmd) {
            self.select_track(new);
            self.ui_dirty = true;
        }
    }

    /// The colour grid under the track menu: each palette colour in grid order, the track's own
    /// marked (`controller::TRACK_COLOR_GRID`).
    pub fn track_swatches(&self, track: TrackId) -> Vec<crate::MenuSwatch> {
        let cur = self.project().track(track).map(|t| t.color.0 as usize % track_palette_len());
        crate::controller::TRACK_COLOR_GRID
            .iter()
            .map(|&i| crate::MenuSwatch { color: slint_color(track_color(ColorIndex(i))), selected: cur == Some(i as usize) })
            .collect()
    }

    /// A swatch of the grid (position `cell`) picked for `track`: its colour, one undo step.
    pub fn track_swatch_pick(&mut self, track: TrackId, cell: usize) {
        self.menu_close();
        let Some(&i) = crate::controller::TRACK_COLOR_GRID.get(cell) else { return };
        if self.project().track(track).is_some_and(|t| t.color.0 as usize % track_palette_len() != i as usize) {
            self.execute(Command::SetTrackColor { track, color: ColorIndex(i) });
        }
    }

    /// Track header press (window y): phase 0 press, 1 move, 2 release, 3 cancel. A press
    /// that travels [DRAG_START] becomes a drag that shows a drop line between the rows of the
    /// track's group (ordinary tracks, or the returns); release moves the track there.
    pub fn track_drag(&mut self, id: TrackId, phase: i32, y: f32) {
        match phase {
            0 => {
                // Pinned return rows don't scroll with the list; they keep their order.
                let pinned = self.project().track_index(id).is_none_or(|i| self.view.is_pinned(i));
                self.track_drag = (!pinned).then_some(TrackDrag { track: id, start_y: y, active: false, gap: None });
            }
            1 => {
                let Some(mut d) = self.track_drag else { return };
                if !d.active && (y - d.start_y).abs() < DRAG_START {
                    return;
                }
                d.active = true;
                d.gap = self.track_gap(d.track, y);
                self.track_drag = Some(d);
                self.ui_dirty = true;
            }
            2 => {
                let Some(d) = self.track_drag.take() else { return };
                self.ui_dirty |= d.active;
                let (Some(gap), Some(from)) = (d.gap.filter(|_| d.active), self.project().track_index(d.track)) else { return };
                if self.project().track(d.track).is_some_and(|t| t.is_group()) {
                    self.move_group(d.track, gap);
                    return;
                }
                let to = move_target(from, gap);
                if to != from {
                    self.execute(Command::MoveTrack { track: d.track, index: to });
                }
            }
            _ => {
                self.ui_dirty |= self.track_drag.take().is_some_and(|d| d.active);
            }
        }
    }

    /// The gap (insert-before index) under window y for dragging `track`: within its group, the
    /// first row whose middle is below y, else the group's end.
    fn track_gap(&self, track: TrackId, y: f32) -> Option<usize> {
        let p = self.project();
        let i = p.track_index(track)?;
        let ordinary = p.ordinary_track_count();
        let (lo, hi) = if p.tracks[i].is_return() { (ordinary, p.tracks.len()) } else { (0, ordinary) };
        let hi = hi.min(self.view.pinned_from);
        let content_y = y - LIST_TOP + self.view.scroll_y as f32;
        Some((lo..hi).find(|&k| self.view.track_offset(k) + self.view.track_span(k) / 2.0 > content_y).unwrap_or(hi))
    }

    /// Drop line for the header list (content y), while a reorder drag is on.
    pub fn push_track_drag(&self, s: &Studio) {
        let line = self.track_drag.filter(|d| d.active).and_then(|d| d.gap);
        s.set_track_drop_on(line.is_some());
        if let Some(g) = line {
            s.set_track_drop_y(self.view.track_offset(g));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drop_gaps_map_to_move_indices() {
        // Track 1 of 0..4: gaps 0 and 1 leave it at 0 and 1 (1 = where it is), gap 3 puts it at 2.
        assert_eq!(move_target(1, 0), 0);
        assert_eq!(move_target(1, 1), 1);
        assert_eq!(move_target(1, 2), 1);
        assert_eq!(move_target(1, 3), 2);
        assert_eq!(move_target(1, 4), 3);
    }
}
