//! Time-range edits: what the timeline's Marquee tool does with a range of time across tracks.
//!
//! Every edit here is built from existing commands (`SplitClip`, `RemoveClip`, `AddClip`, ...) as
//! one `Command::Batch`, so undo, redo and scripting work as for any other edit. Clips crossing a
//! range edge are split there first; the batch is worked out on a copy of the project so the ids
//! of the split-off parts are known before it runs, which keeps redo exact. A clip that can't be
//! split at an edge (warped or reversed audio, a pattern clip) is left whole and counts as outside
//! the range, so an edit never half-applies.

use crate::command::Command;
use crate::midi::MidiClip;
use crate::model::{Clip, ClipId, Project, Ticks, TrackId};

/// A stretch of time, `start..end` in ticks, on a set of tracks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimeRange {
    pub start: Ticks,
    pub end: Ticks,
    pub tracks: Vec<TrackId>,
}

impl TimeRange {
    pub fn len(&self) -> Ticks {
        self.end - self.start
    }

    pub fn is_empty(&self) -> bool {
        self.end <= self.start || self.tracks.is_empty()
    }
}

/// A clip (or the part of one) inside a range, as copied to the clipboard.
#[derive(Debug, Clone, PartialEq)]
pub enum Piece {
    Audio { track: TrackId, clip: Clip },
    Midi { track: TrackId, clip: MidiClip },
}

impl Piece {
    pub fn start(&self) -> Ticks {
        match self {
            Piece::Audio { clip, .. } => clip.start,
            Piece::Midi { clip, .. } => clip.start,
        }
    }
}

/// Clips on the range's tracks that cross `at` (start before, end after), in track order.
fn crossing(p: &Project, r: &TimeRange, at: Ticks) -> Vec<ClipId> {
    let mut out = Vec::new();
    for t in p.tracks.iter().filter(|t| r.tracks.contains(&t.id)) {
        out.extend(t.clips.iter().filter(|c| c.start < at && c.start + p.clip_length_ticks(c) > at).map(|c| c.id));
        out.extend(t.midi_clips.iter().filter(|c| c.start < at && c.end() > at).map(|c| c.id));
    }
    out
}

/// Split every clip crossing either edge, on a copy of `p`. Returns the splits that worked (with
/// explicit ids, in order) and the copy after them.
fn split_edges(p: &Project, r: &TimeRange) -> (Vec<Command>, Project) {
    let mut q = p.clone();
    let mut cmds = Vec::new();
    for at in [r.start, r.end] {
        for clip in crossing(&q, r, at) {
            let cmd = Command::SplitClip { clip, at, new_id: ClipId(q.next_id) };
            let mut trial = q.clone();
            if cmd.clone().apply(&mut trial).is_ok() {
                q = trial;
                cmds.push(cmd);
            }
        }
    }
    (cmds, q)
}

/// Pieces lying wholly inside the range in `p` (after the edges were split), in track order.
fn inside(p: &Project, r: &TimeRange) -> Vec<Piece> {
    let mut out = Vec::new();
    for t in p.tracks.iter().filter(|t| r.tracks.contains(&t.id)) {
        for c in t.clips.iter().filter(|c| c.start >= r.start && c.start + p.clip_length_ticks(c) <= r.end) {
            out.push(Piece::Audio { track: t.id, clip: c.clone() });
        }
        for c in t.midi_clips.iter().filter(|c| c.start >= r.start && c.end() <= r.end) {
            out.push(Piece::Midi { track: t.id, clip: c.clone() });
        }
    }
    out
}

fn batch(label: &str, commands: Vec<Command>) -> Option<Command> {
    (!commands.is_empty()).then(|| Command::Batch { label: label.into(), commands })
}

fn remove(piece: &Piece) -> Command {
    match piece {
        Piece::Audio { clip, .. } => Command::RemoveClip { clip: clip.id },
        Piece::Midi { clip, .. } => Command::RemoveMidiClip { clip: clip.id },
    }
}

/// The content of the range as pieces: clips crossing an edge are cut to it.
pub fn copy(p: &Project, r: &TimeRange) -> Vec<Piece> {
    if r.is_empty() {
        return Vec::new();
    }
    inside(&split_edges(p, r).1, r)
}

/// Delete everything inside the range ("Delete Range"); `None` when nothing is there.
pub fn delete(p: &Project, r: &TimeRange) -> Option<Command> {
    delete_labelled(p, r, "Delete Range")
}

/// Delete the range's content under another label (Cut Range).
pub fn delete_labelled(p: &Project, r: &TimeRange, label: &str) -> Option<Command> {
    if r.is_empty() {
        return None;
    }
    let (mut cmds, q) = split_edges(p, r);
    let gone = inside(&q, r);
    if gone.is_empty() {
        return None;
    }
    cmds.extend(gone.iter().map(remove));
    batch(label, cmds)
}

/// Split every clip crossing either edge ("Split at Range"); `None` when none does.
pub fn split(p: &Project, r: &TimeRange) -> Option<Command> {
    if r.is_empty() {
        return None;
    }
    batch("Split at Range", split_edges(p, r).0)
}

/// Add `pieces` shifted by `dt` with fresh ids from `next`, skipping tracks that are gone.
/// Returns the commands and the new clip ids.
pub fn add_pieces(p: &Project, pieces: &[Piece], dt: Ticks, mut next: u64) -> (Vec<Command>, Vec<ClipId>) {
    let (mut cmds, mut ids) = (Vec::new(), Vec::new());
    for piece in pieces {
        let id = ClipId(next);
        let cmd = match piece {
            Piece::Audio { track, clip } if p.track(*track).is_some() => {
                Command::AddClip { track: *track, clip: Clip { id, start: (clip.start + dt).max(0), ..clip.clone() } }
            }
            Piece::Midi { track, clip } if p.track(*track).is_some() => {
                Command::AddMidiClip { track: *track, clip: MidiClip { id, start: (clip.start + dt).max(0), ..clip.clone() } }
            }
            _ => continue,
        };
        cmds.push(cmd);
        ids.push(id);
        next += 1;
    }
    (cmds, ids)
}

/// Copy the range's content to right after it ("Duplicate Range"). Returns the batch and the
/// ids of the copies; `None` when the range is empty.
pub fn duplicate(p: &Project, r: &TimeRange) -> Option<(Command, Vec<ClipId>)> {
    if r.is_empty() {
        return None;
    }
    let (mut cmds, q) = split_edges(p, r);
    let pieces = inside(&q, r);
    if pieces.is_empty() {
        return None;
    }
    let (adds, ids) = add_pieces(&q, &pieces, r.len(), q.next_id);
    cmds.extend(adds);
    batch("Duplicate Range", cmds).map(|c| (c, ids))
}
