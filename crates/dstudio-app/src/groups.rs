//! Group (folder) tracks: a group plays the tracks routed into it (their `output` is the
//! group) through its own inserts, volume and pan, and its tracks sit right below it in the
//! track list. The engine runs a group as a bus, like a return track. Folding a group hides
//! its tracks' rows (view state, not saved and not undoable).

use crate::controller::Controller;
use dstudio_core::{Command, MAX_RETURNS, Track, TrackId, TrackKind};

/// Moves that put `members` (in this order) right after group `group` in `order`, as (track,
/// final index) pairs for `Command::MoveTrack`, applied one after another.
pub fn member_moves(order: &[TrackId], group: TrackId, members: &[TrackId]) -> Vec<(TrackId, usize)> {
    let mut order = order.to_vec();
    let mut moves = Vec::new();
    for (k, &id) in members.iter().enumerate() {
        let Some(from) = order.iter().position(|&t| t == id) else { continue };
        let t = order.remove(from);
        let Some(g) = order.iter().position(|&t| t == group) else { return moves };
        let to = g + 1 + k;
        order.insert(to, t);
        if from != to {
            moves.push((id, to));
        }
    }
    moves
}

/// Moves (track, final index) that turn `order` into `target` (the same tracks), applied one
/// after another.
pub fn moves_to(order: &[TrackId], target: &[TrackId]) -> Vec<(TrackId, usize)> {
    let mut order = order.to_vec();
    let mut moves = Vec::new();
    for (k, &id) in target.iter().enumerate() {
        if order.get(k) == Some(&id) {
            continue;
        }
        let Some(from) = order.iter().position(|&t| t == id) else { continue };
        let t = order.remove(from);
        order.insert(k, t);
        moves.push((id, k));
    }
    moves
}

/// `order` with group `group` and its `members` (in this order) moved together before the
/// track at `gap` (an index into `order`; past the end = at the end). None when the gap is inside
/// the block itself.
pub fn group_moved(order: &[TrackId], group: TrackId, members: &[TrackId], gap: usize) -> Option<Vec<TrackId>> {
    let before = order.get(gap).copied();
    if before.is_some_and(|b| b == group || members.contains(&b)) {
        return None;
    }
    let mut rest: Vec<TrackId> = order.iter().copied().filter(|t| *t != group && !members.contains(t)).collect();
    let at = before.and_then(|b| rest.iter().position(|&t| t == b)).unwrap_or(rest.len());
    rest.splice(at..at, std::iter::once(group).chain(members.iter().copied()));
    Some(rest)
}

impl Controller {
    /// Can `id` go into a group? Ordinary (audio and instrument) tracks can.
    pub fn groupable(&self, id: TrackId) -> bool {
        self.project().track(id).is_some_and(|t| matches!(t.kind, TrackKind::Audio | TrackKind::Instrument))
    }

    /// A new group track in place of the first of `ids`, holding all of them (in track order).
    /// One undo step, "Group Tracks". Returns the group.
    pub fn group_tracks(&mut self, ids: &[TrackId]) -> Option<TrackId> {
        let p = self.project();
        let mut members: Vec<(usize, TrackId)> =
            ids.iter().filter(|&&id| self.groupable(id)).filter_map(|&id| p.track_index(id).map(|i| (i, id))).collect();
        members.sort();
        members.dedup();
        let &(first, _) = members.first()?;
        if p.tracks.iter().filter(|t| t.is_bus()).count() >= MAX_RETURNS {
            self.set_status(format!("A project has at most {MAX_RETURNS} group and return tracks"), 3);
            return None;
        }
        let id = TrackId(p.next_id);
        let color = p.tracks[first].color;
        let group = Track::new(id, TrackKind::Group, &p.next_group_name(), color);
        let mut order: Vec<TrackId> = p.tracks.iter().map(|t| t.id).collect();
        order.insert(first, id);
        let ids: Vec<TrackId> = members.iter().map(|&(_, t)| t).collect();
        let mut commands = vec![Command::AddTrack { index: Some(first), track: group }];
        commands.extend(member_moves(&order, id, &ids).into_iter().map(|(track, index)| Command::MoveTrack { track, index }));
        commands.extend(ids.iter().map(|&track| Command::SetTrackOutput { track, output: Some(id) }));
        self.doc.seal_coalescing();
        let ok = self.execute(Command::Batch { label: "Group Tracks".into(), commands });
        self.doc.seal_coalescing();
        if ok {
            self.select_track(id);
            self.ui_dirty = true;
            return Some(id);
        }
        None
    }

    /// Remove group `group`: its tracks stay where they are and play into the master again.
    /// One undo step, "Ungroup".
    pub fn ungroup(&mut self, group: TrackId) -> bool {
        if !self.project().track(group).is_some_and(|t| t.is_group()) {
            return false;
        }
        let first = self.project().group_members(group).next().map(|t| t.id);
        self.collapsed.remove(&group);
        self.doc.seal_coalescing();
        let ok = self.execute(Command::Batch { label: "Ungroup".into(), commands: vec![Command::RemoveTrack { track: group }] });
        self.doc.seal_coalescing();
        if ok {
            if let Some(t) = first {
                self.select_track(t);
            }
            self.ui_dirty = true;
        }
        ok
    }

    /// Route `track` into group `group` and move it below the group's other tracks
    /// ("Output Change"), or out of its group to the master (`group` None, it stays put).
    pub fn route_to_group(&mut self, track: TrackId, group: Option<TrackId>) -> bool {
        let p = self.project();
        let Some(t) = p.track(track) else { return false };
        if t.output == group || !self.groupable(track) {
            return false;
        }
        let mut commands = vec![Command::SetTrackOutput { track, output: group }];
        if let Some(g) = group {
            let mut members: Vec<TrackId> = p.group_members(g).map(|t| t.id).filter(|&m| m != track).collect();
            members.push(track);
            let order: Vec<TrackId> = p.tracks.iter().map(|t| t.id).collect();
            commands.extend(member_moves(&order, g, &members).into_iter().map(|(track, index)| Command::MoveTrack { track, index }));
        }
        self.doc.seal_coalescing();
        let ok = self.execute(Command::Batch { label: "Output Change".into(), commands });
        self.doc.seal_coalescing();
        ok
    }

    /// A track header drag of group `group` dropped at `gap`: the group moves with its tracks
    /// (one undo step, "Move Track").
    pub fn move_group(&mut self, group: TrackId, gap: usize) -> bool {
        let p = self.project();
        let members: Vec<TrackId> = p.group_members(group).map(|t| t.id).collect();
        let order: Vec<TrackId> = p.tracks.iter().map(|t| t.id).collect();
        // Never between another group and its tracks (a folded group's rows have no height, so
        // the gap can land there): after that group's tracks instead.
        let mut gap = gap;
        while gap > 0
            && p.tracks.get(gap).is_some_and(|t| t.output.is_some_and(|g| g != group && p.track(g).is_some_and(|g| g.is_group())))
            && p.tracks[gap - 1].output.or(Some(p.tracks[gap - 1].id)) == p.tracks[gap].output
        {
            gap += 1;
        }
        let Some(target) = group_moved(&order, group, &members, gap) else { return false };
        let commands: Vec<Command> =
            moves_to(&order, &target).into_iter().map(|(track, index)| Command::MoveTrack { track, index }).collect();
        if commands.is_empty() {
            return false;
        }
        self.doc.seal_coalescing();
        let ok = self.execute(Command::Batch { label: "Move Track".into(), commands });
        self.doc.seal_coalescing();
        ok
    }

    /// Bars for the group lanes: every clip of each track of a group, one row per track that
    /// holds clips, in the track's colour while the group is folded and grey while it is open.
    pub fn group_summary(&self) -> Vec<crate::timeline::SummaryBlock> {
        use dstudio_core::PPQ;
        const OPEN: [f32; 3] = [158.0 / 255.0, 160.65 / 255.0, 170.85 / 255.0];
        let p = self.project();
        let mut out = Vec::new();
        for (gi, g) in p.tracks.iter().enumerate().filter(|(_, t)| t.is_group()) {
            let folded = self.collapsed.contains(&g.id);
            let members: Vec<&Track> = p.group_members(g.id).filter(|t| !t.clips.is_empty() || !t.midi_clips.is_empty()).collect();
            let rows = members.len();
            for (row, t) in members.into_iter().enumerate() {
                let rgb = if folded { crate::controller::track_color(t.color) } else { OPEN };
                let clips = t
                    .clips
                    .iter()
                    .map(|c| (c.id, c.start, p.clip_length_ticks(c), c.disabled))
                    .chain(t.midi_clips.iter().map(|c| (c.id, c.start, c.length, c.disabled)));
                for (id, start, len, disabled) in clips {
                    let silent = t.mute || g.mute || disabled;
                    let alpha = match (folded, self.is_clip_selected(id), silent) {
                        (true, true, _) => 1.0,
                        (true, false, true) => 0.3,
                        (true, false, false) => 0.75,
                        (false, true, _) => 0.55,
                        (false, false, true) => 0.14,
                        (false, false, false) => 0.3,
                    };
                    out.push(crate::timeline::SummaryBlock {
                        track_index: gi,
                        start_beats: start as f64 / PPQ as f64,
                        len_beats: len as f64 / PPQ as f64,
                        row,
                        rows,
                        color: [rgb[0], rgb[1], rgb[2], alpha],
                    });
                }
            }
        }
        out
    }

    /// Fold or unfold group `group` (its tracks' rows hide).
    pub fn toggle_group_folded(&mut self, group: TrackId) {
        if !self.collapsed.remove(&group) && self.project().track(group).is_some_and(|t| t.is_group()) {
            self.collapsed.insert(group);
            // A hidden track can't stay selected.
            if self.selected_track.and_then(|id| self.project().track(id)).is_some_and(|t| t.output == Some(group)) {
                self.select_track(group);
            }
        }
        self.ui_dirty = true;
    }

    /// Whether track `t`'s row is hidden by a folded group.
    pub fn hidden_by_fold(&self, t: &Track) -> bool {
        t.output.is_some_and(|g| self.collapsed.contains(&g))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(v: &[u64]) -> Vec<TrackId> {
        v.iter().map(|&i| TrackId(i)).collect()
    }

    /// Applies the moves to `order` as MoveTrack would.
    fn apply(order: &[TrackId], moves: &[(TrackId, usize)]) -> Vec<TrackId> {
        let mut o = order.to_vec();
        for &(id, to) in moves {
            let from = o.iter().position(|&t| t == id).unwrap();
            let t = o.remove(from);
            o.insert(to, t);
        }
        o
    }

    #[test]
    fn members_follow_the_group_in_track_order() {
        // Group 9 inserted at index 1, grouping tracks 2 and 4 (3 stays out).
        let order = ids(&[1, 9, 2, 3, 4, 5]);
        let moves = member_moves(&order, TrackId(9), &ids(&[2, 4]));
        assert_eq!(apply(&order, &moves), ids(&[1, 9, 2, 4, 3, 5]));
        assert_eq!(moves.len(), 1, "track 2 is already in place");
    }

    #[test]
    fn a_track_from_above_the_group_moves_down_to_it() {
        // Track 1 joins group 9 (tracks 3 and 4): it ends up after them.
        let order = ids(&[1, 2, 9, 3, 4, 5]);
        let moves = member_moves(&order, TrackId(9), &ids(&[3, 4, 1]));
        assert_eq!(apply(&order, &moves), ids(&[2, 9, 3, 4, 1, 5]));
    }

    #[test]
    fn a_group_moves_with_its_tracks() {
        let order = ids(&[1, 9, 2, 3, 4, 5]);
        // Down below track 4 (gap 5 = before track 5).
        let target = group_moved(&order, TrackId(9), &ids(&[2, 3]), 5).unwrap();
        assert_eq!(target, ids(&[1, 4, 9, 2, 3, 5]));
        assert_eq!(apply(&order, &moves_to(&order, &target)), target);
        // Up to the top, and to the end.
        assert_eq!(group_moved(&order, TrackId(9), &ids(&[2, 3]), 0).unwrap(), ids(&[9, 2, 3, 1, 4, 5]));
        assert_eq!(group_moved(&order, TrackId(9), &ids(&[2, 3]), 6).unwrap(), ids(&[1, 4, 5, 9, 2, 3]));
        // Dropped inside its own block: nothing.
        assert_eq!(group_moved(&order, TrackId(9), &ids(&[2, 3]), 2), None);
    }

    #[test]
    fn a_track_from_below_joins_at_the_end() {
        let order = ids(&[9, 3, 4, 5, 6]);
        let moves = member_moves(&order, TrackId(9), &ids(&[3, 4, 6]));
        assert_eq!(apply(&order, &moves), ids(&[9, 3, 4, 6, 5]));
    }
}
