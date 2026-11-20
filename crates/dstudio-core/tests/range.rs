//! Time-range edits (the timeline's Marquee tool): delete, duplicate and split over audio and
//! MIDI tracks, each one exact undo step; clips that can't be split stay whole.

use dstudio_core::range::{self, Piece, TimeRange};
use dstudio_core::*;

const BAR: Ticks = 4 * PPQ;

/// Default project: an audio clip on track 0 and a MIDI clip on track 1, both bars 1 to 3
/// (0..2 bars) with a note in each bar of the MIDI clip.
fn doc() -> (Document, TrackId, TrackId) {
    let mut d = Document::new(Project::default());
    let (audio, inst) = (d.project().tracks[0].id, d.project().tracks[1].id);
    let asset = Asset {
        id: AssetId(0),
        relative_path: None,
        absolute_path: "/nonexistent/t.wav".into(),
        sample_rate: 48_000.0,
        channels: 2,
        frames: 48_000 * 10,
    };
    d.execute(Command::AddAsset { asset }, Origin::User).unwrap();
    let a = d.project().assets[0].id;
    // 120 bpm at 48 kHz: one bar is 2 s.
    let clip = Clip {
        id: ClipId(0),
        asset: a,
        name: "a".into(),
        start: 0,
        source_offset: 0,
        source_length: 48_000 * 4,
        gain: 1.0,
        warp: Default::default(),
        disabled: false,
        clip_loop: None,
        fades: Default::default(),
        takes: Vec::new(),
    };
    d.execute(Command::AddClip { track: audio, clip }, Origin::User).unwrap();
    let mut m = MidiClip::empty(0, 2 * BAR);
    m.notes = vec![
        Note { id: 1, pitch: 60, start: 0, length: PPQ, velocity: 100 },
        Note { id: 2, pitch: 62, start: BAR, length: PPQ, velocity: 100 },
    ];
    d.execute(Command::AddMidiClip { track: inst, clip: m }, Origin::User).unwrap();
    (d, audio, inst)
}

fn spans(p: &Project) -> Vec<(Ticks, Ticks)> {
    let mut v: Vec<(Ticks, Ticks)> = p
        .tracks
        .iter()
        .flat_map(|t| {
            t.clips.iter().map(|c| (c.start, c.start + p.clip_length_ticks(c))).chain(t.midi_clips.iter().map(|c| (c.start, c.end())))
        })
        .collect();
    v.sort();
    v
}

#[test]
fn delete_cuts_a_hole_through_both_tracks_and_undoes_exactly() {
    let (mut d, audio, inst) = doc();
    let before = d.project().clone();
    let r = TimeRange { start: BAR / 2, end: BAR + BAR / 2, tracks: vec![audio, inst] };
    let cmd = range::delete(d.project(), &r).unwrap();
    d.execute(cmd, Origin::User).unwrap();
    assert_eq!(d.undo_label(), Some("Delete Range"));
    // Each clip keeps its outer halves: 0..½ bar and 1½..2 bars.
    assert_eq!(spans(d.project()), vec![(0, BAR / 2), (0, BAR / 2), (BAR + BAR / 2, 2 * BAR), (BAR + BAR / 2, 2 * BAR)]);
    let after = d.project().clone();
    d.undo().unwrap();
    // Ids are never recycled, so only next_id may differ.
    assert_eq!(d.project().tracks, before.tracks);
    d.redo().unwrap();
    assert_eq!(d.project().tracks, after.tracks);
}

#[test]
fn duplicate_copies_the_inside_after_the_range() {
    let (mut d, _, inst) = doc();
    let r = TimeRange { start: BAR, end: 2 * BAR, tracks: vec![inst] };
    let (cmd, ids) = range::duplicate(d.project(), &r).unwrap();
    d.execute(cmd, Origin::User).unwrap();
    assert_eq!(d.undo_label(), Some("Duplicate Range"));
    let copy = d.project().midi_clip(ids[0]).unwrap();
    assert_eq!((copy.start, copy.length), (2 * BAR, BAR));
    // The second bar's note, now at the copy's start.
    assert_eq!(copy.notes.iter().map(|n| (n.pitch, n.start)).collect::<Vec<_>>(), vec![(62, 0)]);
}

#[test]
fn split_leaves_the_range_as_its_own_clips_and_copy_cuts_to_it() {
    let (mut d, audio, inst) = doc();
    let r = TimeRange { start: BAR / 2, end: BAR, tracks: vec![audio, inst] };
    let pieces = range::copy(d.project(), &r);
    assert_eq!(pieces.len(), 2);
    assert!(pieces.iter().all(|p| p.start() == BAR / 2));
    assert!(matches!(&pieces[1], Piece::Midi { clip, .. } if clip.length == BAR / 2 && clip.notes.is_empty()));
    let n = spans(d.project()).len();
    d.execute(range::split(d.project(), &r).unwrap(), Origin::User).unwrap();
    assert_eq!(d.undo_label(), Some("Split at Range"));
    assert_eq!(spans(d.project()).len(), n + 4);
}

#[test]
fn clips_that_cannot_be_split_stay_whole() {
    let (mut d, audio, _) = doc();
    let id = d.project().tracks[0].clips[0].id;
    let mut warp = d.project().tracks[0].clips[0].warp.clone();
    warp.reverse = true;
    d.execute(Command::SetClipWarp { clip: id, warp, label: "Reverse".into() }, Origin::User).unwrap();
    let before = d.project().clone();
    // Crosses both edges of a reversed clip: nothing splits, so nothing is inside either.
    let r = TimeRange { start: BAR / 2, end: BAR, tracks: vec![audio] };
    assert!(range::delete(&before, &r).is_none());
    assert!(range::split(&before, &r).is_none());
    // An empty range or one with no tracks does nothing.
    assert!(range::delete(&before, &TimeRange { start: BAR, end: BAR, tracks: vec![audio] }).is_none());
    assert!(range::copy(&before, &TimeRange { start: 0, end: BAR, tracks: vec![] }).is_empty());
    assert_eq!(d.project(), &before);
}
