//! Selecting several clips on the timeline: Shift-click adds or removes a clip, dragging over an
//! empty part of the lanes draws a selection box, and the selected clips move, duplicate, copy,
//! cut, paste, enable / disable and delete together.
//!
//! `Controller::selected_clip` stays the clip the editors and menus act on (the last one
//! clicked); `Controller::extra_clips` holds the others.

use crate::controller::{ClipboardItem, Controller};
use crate::timeline::ClipVis;
use dstudio_core::{ClipId, Command, Ticks, TrackId};

/// Pointer travel (px) before a press on empty lane space becomes a selection box.
pub const BOX_START: f32 = 4.0;

/// A selection box drag in lane coordinates: where it started, where the pointer is, and the
/// selection it adds to (Shift held at the press) or replaces.
#[derive(Debug, Clone)]
pub struct Marquee {
    pub from: (f32, f32),
    pub to: (f32, f32),
    pub active: bool,
    pub base: Vec<ClipId>,
}

impl Marquee {
    pub fn rect(&self) -> (f32, f32, f32, f32) {
        let (x0, x1) = (self.from.0.min(self.to.0), self.from.0.max(self.to.0));
        let (y0, y1) = (self.from.1.min(self.to.1), self.from.1.max(self.to.1));
        (x0, y0, x1 - x0, y1 - y0)
    }
}

/// The time and track offset a group move applies to every selected clip, from the dragged
/// clip's wanted offset: time is held so no clip starts before 0, and the track offset is
/// dropped (time only) unless every clip lands on a track of its own kind.
pub fn group_offset(dt: Ticks, dtrack: isize, clips: &[(usize, Ticks)], kinds: &[dstudio_core::TrackKind]) -> (Ticks, isize) {
    let min_start = clips.iter().map(|&(_, s)| s).min().unwrap_or(0);
    let dt = dt.max(-min_start);
    let fits = clips.iter().all(|&(t, _)| {
        let to = t as isize + dtrack;
        to >= 0 && (to as usize) < kinds.len() && kinds[to as usize] == kinds[t]
    });
    (dt, if fits { dtrack } else { 0 })
}

/// Clips whose drawn rectangle meets the box (x, y, w, h); `top(i)` and `height(i)` give track
/// `i`'s lane in the same coordinates, `x_of` a beat's x.
pub fn clips_in_box(
    vis: &[ClipVis],
    ids: &[ClipId],
    rect: (f32, f32, f32, f32),
    x_of: impl Fn(f64) -> f32,
    top: impl Fn(usize) -> f32,
    height: impl Fn(usize) -> f32,
) -> Vec<ClipId> {
    let (bx, by, bw, bh) = rect;
    vis.iter()
        .zip(ids)
        .filter(|(v, _)| {
            let (x0, x1) = (x_of(v.start_beats), x_of(v.start_beats + v.len_beats));
            let (y0, y1) = (top(v.track_index), top(v.track_index) + height(v.track_index));
            x1 > bx && x0 < bx + bw && y1 > by && y0 < by + bh
        })
        .map(|(_, id)| *id)
        .collect()
}

impl Controller {
    /// Every selected clip, the focused one first.
    pub fn selected_clips(&self) -> Vec<ClipId> {
        self.selected_clip.into_iter().chain(self.extra_clips.iter().copied()).collect()
    }

    pub fn is_clip_selected(&self, id: ClipId) -> bool {
        self.selected_clip == Some(id) || self.extra_clips.contains(&id)
    }

    /// Shift-click: add a clip to the selection (it becomes the focused one), or take it out.
    pub fn toggle_clip_selection(&mut self, id: ClipId) {
        if self.is_clip_selected(id) {
            let mut rest: Vec<ClipId> = self.selected_clips().into_iter().filter(|&c| c != id).collect();
            let first = (!rest.is_empty()).then(|| rest.remove(0));
            self.select_clip(first);
            self.extra_clips = rest;
        } else {
            let mut all = self.selected_clips();
            self.select_clip(Some(id));
            all.retain(|&c| c != id);
            self.extra_clips = all;
        }
        if let Some(t) = self.selected_clip.and_then(|c| self.clip_track(c)) {
            self.select_track(t);
        }
        self.ui_dirty = true;
    }

    /// Select exactly `clips` (the first is focused).
    pub fn set_clip_selection(&mut self, clips: Vec<ClipId>) {
        let mut it = clips.into_iter();
        let first = it.next();
        self.select_clip(first);
        self.extra_clips = it.collect();
        if let Some(t) = first.and_then(|c| self.clip_track(c)) {
            self.select_track(t);
        }
        self.ui_dirty = true;
    }

    /// Selected clips that still exist, as (track index, start, kind is MIDI).
    fn selection_places(&self) -> Vec<(ClipId, usize, Ticks, bool)> {
        let p = self.project();
        self.selected_clips()
            .into_iter()
            .filter_map(|id| {
                if let Some((t, c)) = p.find_midi_clip(id) {
                    Some((id, t, p.tracks[t].midi_clips[c].start, true))
                } else {
                    p.find_clip(id).map(|(t, c)| (id, t, p.tracks[t].clips[c].start, false))
                }
            })
            .collect()
    }

    /// Group offset for the dragged clip's place (`track`, `start`) against its own (`orig_track`, `orig_start`).
    pub(crate) fn group_drag_offset(&self, orig_track: TrackId, orig_start: Ticks, track: TrackId, start: Ticks) -> (Ticks, isize) {
        let p = self.project();
        let (Some(from), Some(to)) = (p.track_index(orig_track), p.track_index(track)) else { return (0, 0) };
        let places: Vec<(usize, Ticks)> = self.selection_places().iter().map(|&(_, t, s, _)| (t, s)).collect();
        let kinds: Vec<_> = p.tracks.iter().map(|t| t.kind).collect();
        group_offset(start - orig_start, to as isize - from as isize, &places, &kinds)
    }

    /// Where each selected clip is drawn during a group move (track index, start).
    pub(crate) fn group_previews(&self) -> std::collections::HashMap<ClipId, (usize, Ticks)> {
        let Some((dt, dtrack)) = self.group_move_now() else { return Default::default() };
        self.selection_places().into_iter().map(|(id, t, s, _)| (id, ((t as isize + dtrack) as usize, s + dt))).collect()
    }

    /// Make `id` the focused clip, keeping the rest of the selection when it is already selected
    /// (a press on one of several selected clips drags them all); else select it alone.
    pub(crate) fn focus_clip(&mut self, id: ClipId) {
        if self.is_clip_selected(id) && !self.extra_clips.is_empty() {
            let rest: Vec<ClipId> = self.selected_clips().into_iter().filter(|&c| c != id).collect();
            self.select_clip(Some(id));
            self.extra_clips = rest;
        } else {
            self.select_clip(Some(id));
        }
    }

    /// Release of a group move: every selected clip moves by the same offset, one undo step.
    pub(crate) fn finish_group_move(&mut self, orig_track: TrackId, orig_start: Ticks, track: TrackId, start: Ticks) {
        let (dt, dtrack) = self.group_drag_offset(orig_track, orig_start, track, start);
        if (dt, dtrack) == (0, 0) {
            return;
        }
        let p = self.project();
        let cmds: Vec<Command> = self
            .selection_places()
            .into_iter()
            .map(|(clip, t, s, midi)| {
                let track = p.tracks[(t as isize + dtrack) as usize].id;
                if midi { Command::MoveMidiClip { clip, track, start: s + dt } } else { Command::MoveClip { clip, track, start: s + dt } }
            })
            .collect();
        let n = cmds.len();
        self.execute(Command::Batch { label: format!("Move {n} Clips"), commands: cmds });
    }

    /// Move clips to track above / below: every selected clip moves to the nearest track in that
    /// direction where all of them land on a track of their own kind (audio on audio, MIDI on
    /// instrument), keeping its start. One undo step; nothing happens at the edge.
    pub fn move_clips_to_next_track(&mut self, dir: isize) {
        let places = self.selection_places();
        if places.is_empty() {
            return;
        }
        let p = self.project();
        let kinds: Vec<_> = p.tracks.iter().map(|t| t.kind).collect();
        let starts: Vec<(usize, Ticks)> = places.iter().map(|&(_, t, s, _)| (t, s)).collect();
        let Some(d) = (1..kinds.len() as isize).map(|i| i * dir).find(|&d| group_offset(0, d, &starts, &kinds).1 == d) else {
            return;
        };
        let cmds: Vec<Command> = places
            .iter()
            .map(|&(clip, t, start, midi)| {
                let track = p.tracks[(t as isize + d) as usize].id;
                if midi { Command::MoveMidiClip { clip, track, start } } else { Command::MoveClip { clip, track, start } }
            })
            .collect();
        let focus = p.tracks[(places[0].1 as isize + d) as usize].id;
        let label = if cmds.len() == 1 { "Move Clip".to_string() } else { format!("Move {} Clips", cmds.len()) };
        if self.execute(Command::Batch { label, commands: cmds }) {
            self.select_track(focus);
        }
    }

    /// ⌘D with several clips: copies of all of them right after the selection's span, keeping
    /// their tracks and spacing. The copies become the selection.
    pub(crate) fn duplicate_clips(&mut self) {
        let p = self.project();
        let mut next = p.next_id;
        let spans: Vec<(Ticks, Ticks)> = self
            .selected_clips()
            .iter()
            .filter_map(|&id| {
                p.midi_clip(id).map(|c| (c.start, c.end())).or_else(|| p.clip(id).map(|c| (c.start, c.start + p.clip_length_ticks(c))))
            })
            .collect();
        let (Some(lo), Some(hi)) = (spans.iter().map(|s| s.0).min(), spans.iter().map(|s| s.1).max()) else { return };
        let dt = hi - lo;
        let mut cmds = Vec::new();
        let mut new_ids = Vec::new();
        for id in self.selected_clips() {
            let new = ClipId(next);
            if let Some((t, c)) = p.find_midi_clip(id) {
                let c = &p.tracks[t].midi_clips[c];
                cmds.push(Command::AddMidiClip {
                    track: p.tracks[t].id,
                    clip: dstudio_core::MidiClip { id: new, start: c.start + dt, ..c.clone() },
                });
            } else if let Some((t, c)) = p.find_clip(id) {
                let c = &p.tracks[t].clips[c];
                cmds.push(Command::AddClip {
                    track: p.tracks[t].id,
                    clip: dstudio_core::Clip { id: new, start: c.start + dt, ..c.clone() },
                });
            } else {
                continue;
            }
            next += 1;
            new_ids.push(new);
        }
        let n = cmds.len();
        if self.execute(Command::Batch { label: format!("Duplicate {n} Clips"), commands: cmds }) {
            self.set_clip_selection(new_ids);
        }
    }

    /// Delete every selected clip in one undo step.
    pub(crate) fn delete_clips(&mut self) {
        let p = self.project();
        let cmds: Vec<Command> = self
            .selected_clips()
            .into_iter()
            .filter_map(|clip| {
                if p.find_midi_clip(clip).is_some() {
                    Some(Command::RemoveMidiClip { clip })
                } else {
                    p.find_clip(clip).map(|_| Command::RemoveClip { clip })
                }
            })
            .collect();
        let n = cmds.len();
        self.set_clip_selection(Vec::new());
        self.execute(Command::Batch { label: format!("Delete {n} Clips"), commands: cmds });
    }

    /// 0 with several clips: if any is enabled, disable them all; else enable them all.
    pub(crate) fn toggle_clips_disabled(&mut self) {
        let p = self.project();
        let states: Vec<(ClipId, bool)> = self
            .selected_clips()
            .into_iter()
            .filter_map(|id| {
                let d = p.find_clip(id).map(|(t, c)| p.tracks[t].clips[c].disabled).or_else(|| p.midi_clip(id).map(|c| c.disabled))?;
                Some((id, d))
            })
            .collect();
        let enable = states.iter().all(|&(_, d)| d);
        let cmds: Vec<Command> = states.iter().map(|&(clip, _)| Command::SetClipEnabled { clip, enabled: enable }).collect();
        let label = if enable { "Enable Clips" } else { "Disable Clips" };
        self.execute(Command::Batch { label: label.into(), commands: cmds });
    }

    /// The selected clips as clipboard items: the focused one first.
    pub(crate) fn copy_items(&self) -> Vec<ClipboardItem> {
        let p = self.project();
        self.selected_clips()
            .into_iter()
            .filter_map(|id| {
                if let Some((t, c)) = p.find_midi_clip(id) {
                    Some(ClipboardItem::Midi { track: p.tracks[t].id, clip: p.tracks[t].midi_clips[c].clone() })
                } else {
                    p.find_clip(id).map(|(t, c)| ClipboardItem::Audio { track: p.tracks[t].id, clip: p.tracks[t].clips[c].clone() })
                }
            })
            .collect()
    }

    /// The other clips of a pasted group: on their own tracks, shifted by `dt` like the first.
    /// Fresh ids from `next`; tracks that are gone are skipped.
    pub(crate) fn paste_rest(&self, dt: Ticks, mut next: u64) -> (Vec<Command>, Vec<ClipId>) {
        let p = self.project();
        let (mut cmds, mut ids) = (Vec::new(), Vec::new());
        for item in &self.clipboard_rest {
            let id = ClipId(next);
            match item {
                ClipboardItem::Audio { track, clip } if p.track(*track).is_some() => {
                    cmds.push(Command::AddClip {
                        track: *track,
                        clip: dstudio_core::Clip { id, start: (clip.start + dt).max(0), ..clip.clone() },
                    });
                }
                ClipboardItem::Midi { track, clip } if p.track(*track).is_some() => {
                    cmds.push(Command::AddMidiClip {
                        track: *track,
                        clip: dstudio_core::MidiClip { id, start: (clip.start + dt).max(0), ..clip.clone() },
                    });
                }
                _ => continue,
            }
            ids.push(id);
            next += 1;
        }
        (cmds, ids)
    }

    // ---------------------------------------------------------------- selection box

    pub(crate) fn marquee_press(&mut self, x: f32, y: f32, shift: bool) {
        let base = if shift { self.selected_clips() } else { Vec::new() };
        self.marquee = Some(Marquee { from: (x, y), to: (x, y), active: false, base });
    }

    /// Pointer move during a possible selection box; true when a box is (now) being drawn.
    pub(crate) fn marquee_move(&mut self, x: f32, y: f32) -> bool {
        let Some(mut m) = self.marquee.take() else { return false };
        m.to = (x, y);
        if !m.active && (x - m.from.0).hypot(y - m.from.1) < BOX_START {
            self.marquee = Some(m);
            return false;
        }
        m.active = true;
        let v = &self.view;
        let hit = clips_in_box(&self.clip_vis, &self.clip_ids, m.rect(), |b| v.x_of(b), |i| v.track_top(i), |i| v.pitch(i));
        let mut sel = m.base.clone();
        sel.extend(hit.into_iter().filter(|c| !m.base.contains(c)));
        self.marquee = Some(m);
        self.set_clip_selection(sel);
        true
    }

    pub(crate) fn marquee_release(&mut self) -> bool {
        let active = self.marquee.take().is_some_and(|m| m.active);
        self.ui_dirty |= active;
        active
    }

    pub(crate) fn push_marquee(&self, s: &crate::Studio) {
        let m = self.marquee.as_ref().filter(|m| m.active);
        s.set_marquee_on(m.is_some());
        if let Some(m) = m {
            let (x, y, w, h) = m.rect();
            s.set_marquee_x(x);
            s.set_marquee_y(y);
            s.set_marquee_w(w);
            s.set_marquee_h(h);
        }
    }

    /// Offset of the group move in progress (dragging one of several selected clips).
    fn group_move_now(&self) -> Option<(Ticks, isize)> {
        if self.extra_clips.is_empty() {
            return None;
        }
        let (orig_track, orig_start, track, start) = self.move_drag()?;
        Some(self.group_drag_offset(orig_track, orig_start, track, start))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dstudio_core::PPQ;
    use dstudio_core::TrackKind::{Audio, Instrument};

    #[test]
    fn group_moves_stay_at_or_after_zero_and_keep_track_kinds() {
        let kinds = [Audio, Audio, Instrument];
        // Two clips on tracks 0 and 1 at beats 4 and 8: dragging 6 beats left stops at -4.
        let clips = [(0, 4 * PPQ), (1, 8 * PPQ)];
        assert_eq!(group_offset(-6 * PPQ, 0, &clips, &kinds), (-4 * PPQ, 0));
        // One track down would put track 1's clip on the instrument track: time only.
        assert_eq!(group_offset(PPQ, 1, &clips, &kinds), (PPQ, 0));
        // One up from tracks 1 and ... track 0 has nowhere to go above: time only.
        assert_eq!(group_offset(0, -1, &clips, &kinds), (0, 0));
        // A clip alone on track 0 can go to track 1 (same kind).
        assert_eq!(group_offset(0, 1, &[(0, 0)], &kinds), (0, 1));
    }

    #[test]
    fn the_box_takes_clips_it_touches() {
        let vis = |t: usize, s: f64, l: f64| ClipVis {
            track_index: t,
            start_beats: s,
            len_beats: l,
            color: [0.0; 3],
            selected: false,
            muted: false,
            name: String::new(),
            wave: None,
            midi: None,
            live: None,
            fades: (0.0, 0.0),
            fade_handles: false,
        };
        let v = [vis(0, 0.0, 4.0), vis(0, 8.0, 4.0), vis(1, 2.0, 2.0)];
        let ids = [ClipId(1), ClipId(2), ClipId(3)];
        // 10 px per beat, tracks 50 px tall.
        let hit = |r| clips_in_box(&v, &ids, r, |b| b as f32 * 10.0, |i| i as f32 * 50.0, |_| 50.0);
        assert_eq!(hit((30.0, 10.0, 60.0, 60.0)), [ClipId(1), ClipId(2), ClipId(3)]);
        assert_eq!(hit((45.0, 10.0, 20.0, 20.0)), Vec::<ClipId>::new());
        assert_eq!(hit((15.0, 60.0, 10.0, 10.0)), [ClipId(3)]);
    }
}
