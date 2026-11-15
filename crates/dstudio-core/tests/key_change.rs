use dstudio_core::key_change::{midi_clip_count, transpose_commands};
use dstudio_core::midi::Note;
use dstudio_core::scale::{ClipScale, ScaleKey};
use dstudio_core::*;

fn doc() -> (Document, ClipId, ClipId) {
    let mut d = Document::new(Project::default());
    d.execute(Command::SetProjectKey { key: Some(ProjectKey { root: 0, minor: false }) }, Origin::User).unwrap();
    let inst = d.project().tracks[1].id;
    let mut a = MidiClip::empty(0, 4 * PPQ);
    a.notes = vec![
        Note { id: 1, pitch: 60, start: 0, length: PPQ, velocity: 100 },
        Note { id: 2, pitch: 127, start: PPQ, length: PPQ, velocity: 90 },
    ];
    d.execute(Command::AddMidiClip { track: inst, clip: a }, Origin::User).unwrap();
    // A clip in its own key, E.
    let mut b = MidiClip::empty(8 * PPQ, 4 * PPQ);
    b.notes = vec![Note { id: 1, pitch: 64, start: 0, length: PPQ, velocity: 100 }];
    b.scale = Some(ClipScale { key: Some(ScaleKey::from_project(Some(ProjectKey { root: 4, minor: false }))), ..Default::default() });
    d.execute(Command::AddMidiClip { track: inst, clip: b }, Origin::User).unwrap();
    let c = &d.project().tracks[1].midi_clips;
    let (a, b) = (c[0].id, c[1].id);
    (d, a, b)
}

#[test]
fn transpose_moves_each_clip_from_its_key_in_one_step() {
    let (mut d, a, b) = doc();
    assert_eq!(midi_clip_count(d.project()), 2);
    let before = d.project().clone();
    let d_minor = ProjectKey { root: 2, minor: true };
    let commands = transpose_commands(d.project(), d_minor);
    d.execute(Command::Batch { label: "Key Change".into(), commands }, Origin::User).unwrap();
    let p = d.project();
    assert_eq!(p.transport.key, Some(d_minor));
    let (ta, ca) = p.find_midi_clip(a).unwrap();
    // C to D: up 2; the top note can't go past 127 and stays.
    assert_eq!(p.tracks[ta].midi_clips[ca].notes.iter().map(|n| n.pitch).collect::<Vec<_>>(), vec![62, 127]);
    let (tb, cb) = p.find_midi_clip(b).unwrap();
    // Its own key E to D: down 2, and the clip's key root follows.
    let cb = &p.tracks[tb].midi_clips[cb];
    assert_eq!(cb.notes[0].pitch, 62);
    assert_eq!(cb.scale.unwrap().key.unwrap().root_pc(), 2);
    assert_eq!(d.undo_label(), Some("Key Change"));
    d.undo().unwrap();
    assert_eq!(d.project(), &before);
}

#[test]
fn the_shorter_way_round_and_pattern_clips_stay() {
    let (d, a, _) = doc();
    let mut p = d.project().clone();
    let inst = p.tracks[1].id;
    let mut d = Document::new(std::mem::take(&mut p));
    let pat = MidiClip::empty_pattern(16 * PPQ, StepRows::default());
    d.execute(Command::AddMidiClip { track: inst, clip: pat }, Origin::User).unwrap();
    // C to A: down 3, not up 9. The pattern clip has no command of its own.
    let commands = transpose_commands(d.project(), ProjectKey { root: 9, minor: false });
    let moved: Vec<ClipId> = commands
        .iter()
        .filter_map(|c| match c {
            Command::SetMidiNotes { clip, notes } => {
                if *clip == a {
                    assert_eq!(notes[0].pitch, 57);
                }
                Some(*clip)
            }
            _ => None,
        })
        .collect();
    assert_eq!(moved.len(), 2);
    assert!(matches!(commands.last(), Some(Command::SetProjectKey { .. })));
}
