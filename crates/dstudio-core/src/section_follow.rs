//! "Sections move clips": the clip and automation edits that keep the arrangement in step with
//! its markers. Given the sections before and after a marker edit (move, resize, insert,
//! delete, paste), [`follow_sections`] returns the commands that move each clip and automation
//! point with the marker it starts in, drop what was in a deleted marker, shift everything after
//! the markers, and fill pasted markers with what was copied ([`capture`]).
//!
//! A clip belongs to the marker its start is in and moves whole (clips are never split). Time
//! after the last marker moves with the end of the original markers, so appending a marker at
//! the end labels the time that is already there instead of pushing it later.

use crate::automation::{AutoPoint, AutomationLane, AutomationTarget};
use crate::{Clip, Command, MidiClip, Project, SectionId, Ticks, TrackId};

/// What lies inside one marker, with times relative to its start: the clips that start in it
/// and the automation points in it, per track (`None` is the master).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SectionContent {
    pub clips: Vec<(TrackId, Clip)>,
    pub midi_clips: Vec<(TrackId, MidiClip)>,
    pub points: Vec<(Option<TrackId>, AutomationTarget, Vec<AutoPoint>)>,
}

impl SectionContent {
    pub fn is_empty(&self) -> bool {
        self.clips.is_empty() && self.midi_clips.is_empty() && self.points.is_empty()
    }
}

/// Each section as (id, start, length).
fn spans(p: &Project) -> Vec<(SectionId, Ticks, Ticks)> {
    let mut start = 0;
    p.sections
        .iter()
        .map(|s| {
            let r = (s.id, start, s.length);
            start += s.length;
            r
        })
        .collect()
}

fn lanes(p: &Project) -> impl Iterator<Item = (Option<TrackId>, &AutomationLane)> {
    p.tracks.iter().flat_map(|t| t.automation.iter().map(move |l| (Some(t.id), l))).chain(p.master_automation.iter().map(|l| (None, l)))
}

/// The contents of marker `id` in `p`, or None when there is no such marker.
pub fn capture(p: &Project, id: SectionId) -> Option<SectionContent> {
    let (_, start, len) = spans(p).into_iter().find(|s| s.0 == id)?;
    let inside = |t: Ticks| t >= start && t < start + len;
    let mut c = SectionContent::default();
    for t in &p.tracks {
        for clip in t.clips.iter().filter(|x| inside(x.start)) {
            c.clips.push((t.id, Clip { start: clip.start - start, ..clip.clone() }));
        }
        for clip in t.midi_clips.iter().filter(|x| inside(x.start)) {
            c.midi_clips.push((t.id, MidiClip { start: clip.start - start, ..clip.clone() }));
        }
    }
    for (track, lane) in lanes(p) {
        let pts: Vec<AutoPoint> = lane.points.iter().filter(|q| inside(q.time)).map(|q| AutoPoint { time: q.time - start, ..*q }).collect();
        if !pts.is_empty() {
            c.points.push((track, lane.target.clone(), pts));
        }
    }
    Some(c)
}

/// Where time `t` goes when the sections change from `before` to `after` (None: it was in a
/// deleted marker).
struct TimeMap {
    before: Vec<(SectionId, Ticks, Ticks)>,
    after: Vec<(SectionId, Ticks, Ticks)>,
    tail_shift: Ticks,
}

impl TimeMap {
    fn new(before: &Project, after: &Project) -> Self {
        let before = spans(before);
        let after = spans(after);
        let old_end = before.last().map_or(0, |s| s.1 + s.2);
        // The end of the original markers after the edit; added markers past them do not count.
        let new_end = after.iter().filter(|a| before.iter().any(|b| b.0 == a.0)).map(|a| a.1 + a.2).max().unwrap_or(0);
        TimeMap { tail_shift: new_end - old_end, before, after }
    }

    fn map(&self, t: Ticks) -> Option<Ticks> {
        for &(id, start, len) in &self.before {
            if t >= start && t < start + len {
                return self.after.iter().find(|a| a.0 == id).map(|a| a.1 + (t - start));
            }
        }
        Some((t + self.tail_shift).max(0))
    }

    fn start_of(&self, id: SectionId) -> Option<Ticks> {
        self.after.iter().find(|a| a.0 == id).map(|a| a.1)
    }
}

/// The commands that make the clips and automation of `before` follow its markers into the
/// layout of `after` (the same project with only its sections edited). `pastes` fills new
/// markers (by id in `after`) with captured contents. Returns no commands when nothing moves.
pub fn follow_sections(before: &Project, after: &Project, pastes: &[(SectionId, SectionContent)]) -> Vec<Command> {
    let m = TimeMap::new(before, after);
    let mut out = Vec::new();
    for t in &before.tracks {
        for c in &t.clips {
            match m.map(c.start) {
                None => out.push(Command::RemoveClip { clip: c.id }),
                Some(s) if s != c.start => out.push(Command::MoveClip { clip: c.id, track: t.id, start: s }),
                _ => {}
            }
        }
        for c in &t.midi_clips {
            match m.map(c.start) {
                None => out.push(Command::RemoveMidiClip { clip: c.id }),
                Some(s) if s != c.start => out.push(Command::MoveMidiClip { clip: c.id, track: t.id, start: s }),
                _ => {}
            }
        }
    }

    // Automation: every lane's points mapped, plus pasted points; one lane edit per changed lane.
    let mut edited: Vec<(Option<TrackId>, AutomationLane, bool)> = lanes(before)
        .map(|(track, lane)| {
            let points: Vec<AutoPoint> = lane.points.iter().filter_map(|q| m.map(q.time).map(|time| AutoPoint { time, ..*q })).collect();
            let changed = points != lane.points;
            (track, AutomationLane { points, ..lane.clone() }, changed)
        })
        .collect();

    for (id, content) in pastes {
        let Some(at) = m.start_of(*id) else { continue };
        for (track, clip) in &content.clips {
            if before.track(*track).is_some() {
                let clip = Clip { id: crate::ClipId(0), start: at + clip.start, ..clip.clone() };
                out.push(Command::AddClip { track: *track, clip });
            }
        }
        for (track, clip) in &content.midi_clips {
            if before.track(*track).is_some() {
                let clip = MidiClip { id: crate::ClipId(0), start: at + clip.start, ..clip.clone() };
                out.push(Command::AddMidiClip { track: *track, clip });
            }
        }
        for (track, target, pts) in &content.points {
            if track.is_some_and(|t| before.track(t).is_none()) {
                continue;
            }
            let moved = pts.iter().map(|q| AutoPoint { time: at + q.time, ..*q });
            match edited.iter_mut().find(|(t, l, _)| t == track && l.target == *target) {
                Some((_, lane, changed)) => {
                    lane.points.extend(moved);
                    *changed = true;
                }
                None => edited.push((*track, AutomationLane { target: target.clone(), enabled: true, points: moved.collect() }, true)),
            }
        }
    }
    for (track, mut lane, changed) in edited {
        if changed {
            lane.points.sort_by_key(|q| q.time);
            out.push(Command::SetAutomationLane { track, target: lane.target.clone(), lane: Some(lane) });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{PPQ, Section, TrackKind};

    const BAR: Ticks = 4 * PPQ;

    fn section(id: u64, bars: Ticks) -> Section {
        Section { id: SectionId(id), name: format!("M{id}"), color: 0x50b8b0, length: bars * BAR }
    }

    /// Two 8-bar markers (ids 1, 2) and the default instrument track with a MIDI clip starting
    /// in each marker (ids 20, 21) and one after them (22), plus a volume lane with a point in
    /// each marker.
    fn project() -> Project {
        let mut p = Project::default();
        p.next_id = 100;
        p.sections = vec![section(1, 8), section(2, 8)];
        let t = &mut p.tracks[1];
        assert_eq!(t.kind, TrackKind::Instrument);
        for (id, bar) in [(20, 2), (21, 9), (22, 20)] {
            t.midi_clips.push(MidiClip { id: crate::ClipId(id), ..MidiClip::empty(bar * BAR, BAR) });
        }
        t.automation.push(AutomationLane {
            target: AutomationTarget::Volume,
            enabled: true,
            points: vec![AutoPoint { time: BAR, value: -6.0, curve: 0.0 }, AutoPoint { time: 10 * BAR, value: 0.0, curve: 0.0 }],
        });
        p
    }

    fn apply(p: &mut Project, cmd: Command) {
        cmd.apply(p).expect("valid");
    }

    fn starts(p: &Project) -> Vec<(u64, Ticks)> {
        let mut v: Vec<_> = p.tracks[1].midi_clips.iter().map(|c| (c.id.0, c.start / BAR)).collect();
        v.sort();
        v
    }

    /// Apply a section edit with its follow-up commands, as the app does.
    fn edit(p: &mut Project, cmd: Command, pastes: &[(SectionId, SectionContent)]) {
        let mut after = p.clone();
        apply(&mut after, cmd.clone());
        let follow = follow_sections(p, &after, pastes);
        apply(p, Command::Batch { label: "edit".into(), commands: std::iter::once(cmd).chain(follow).collect() });
    }

    #[test]
    fn moving_a_marker_moves_its_clips_and_points() {
        let mut p = project();
        edit(&mut p, Command::MoveSection { section: SectionId(2), index: 0 }, &[]);
        assert_eq!(starts(&p), vec![(20, 10), (21, 1), (22, 20)]);
        let times: Vec<Ticks> = p.tracks[1].automation[0].points.iter().map(|q| q.time / BAR).collect();
        assert_eq!(times, vec![2, 9]);
    }

    #[test]
    fn resize_and_delete_ripple_what_follows() {
        let mut p = project();
        edit(&mut p, Command::SetSectionLength { section: SectionId(1), length: 4 * BAR }, &[]);
        assert_eq!(starts(&p), vec![(20, 2), (21, 5), (22, 16)]);
        edit(&mut p, Command::RemoveSection { section: SectionId(1) }, &[]);
        assert_eq!(starts(&p), vec![(21, 1), (22, 12)], "the deleted marker's clip goes, later ones move up");
        assert_eq!(p.tracks[1].automation[0].points.len(), 1);
    }

    #[test]
    fn appending_a_marker_labels_time_without_moving_it() {
        let mut p = project();
        edit(&mut p, Command::InsertSection { index: None, section: section(3, 8) }, &[]);
        assert_eq!(starts(&p), vec![(20, 2), (21, 9), (22, 20)]);
        edit(&mut p, Command::InsertSection { index: Some(0), section: section(4, 2) }, &[]);
        assert_eq!(starts(&p), vec![(20, 4), (21, 11), (22, 22)], "inserting in front pushes everything later");
    }

    #[test]
    fn pasting_a_captured_marker_copies_its_clips() {
        let mut p = project();
        let content = capture(&p, SectionId(1)).unwrap();
        assert_eq!(content.midi_clips.len(), 1);
        let copy = Section { id: SectionId(5), ..p.sections[0].clone() };
        edit(&mut p, Command::InsertSection { index: Some(1), section: copy }, &[(SectionId(5), content)]);
        let s = starts(&p);
        assert_eq!(s.len(), 4);
        assert!(s.contains(&(20, 2)) && s.contains(&(21, 17)) && s.contains(&(22, 28)));
        assert!(s.iter().any(|&(id, bar)| id >= 100 && bar == 10), "the copy lands in the pasted marker: {s:?}");
        let times: Vec<Ticks> = p.tracks[1].automation[0].points.iter().map(|q| q.time / BAR).collect();
        assert_eq!(times, vec![1, 9, 18]);
    }
}
