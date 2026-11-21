//! Bounce Track in Place (roadmap G4): the track span, the track it becomes, and ReplaceTrack,
//! the command that swaps it in as one exact undo step.

use dstudio_core::render_plan::{self, bounced_in_place};
use dstudio_core::*;

fn audio_clip(id: u64, asset: AssetId, start: Ticks) -> Clip {
    Clip {
        id: ClipId(id),
        asset,
        name: "Render".into(),
        start,
        source_offset: 0,
        source_length: 48_000,
        gain: 1.0,
        warp: Default::default(),
        disabled: false,
        clip_loop: None,
        fades: Default::default(),
        takes: Vec::new(),
    }
}

/// The default project's instrument track with MIDI clips at beats 4 and 12 (the second
/// disabled), and an asset to put a render in.
fn doc() -> (Document, TrackId, AssetId) {
    let mut d = Document::new(Project::default());
    let inst = d.project().tracks[1].id;
    for (start, disabled) in [(4 * PPQ, false), (12 * PPQ, true)] {
        let clip = MidiClip { disabled, ..MidiClip::empty(start, 4 * PPQ) };
        d.execute(Command::AddMidiClip { track: inst, clip }, Origin::User).unwrap();
    }
    let asset = Asset {
        id: AssetId(0),
        relative_path: None,
        absolute_path: "/nonexistent/r.wav".into(),
        sample_rate: 48_000.0,
        channels: 2,
        frames: 48_000,
    };
    d.execute(Command::AddAsset { asset }, Origin::User).unwrap();
    let a = d.project().assets[0].id;
    (d, inst, a)
}

#[test]
fn the_span_covers_the_enabled_clips_and_the_track_becomes_audio() {
    let (d, inst, a) = doc();
    let p = d.project();
    let span = render_plan::track_span(p, inst).unwrap();
    assert_eq!((span.start, span.end, span.clips.len()), (4 * PPQ, 8 * PPQ, 1));
    let src = p.track(inst).unwrap();
    let t = bounced_in_place(src, audio_clip(p.next_id, a, span.start));
    assert_eq!(t.kind, TrackKind::Audio);
    assert_eq!((t.id, &t.name, t.color, t.gain, t.pan), (src.id, &src.name, src.color, src.gain, src.pan));
    assert!(t.midi_clips.is_empty() && t.clips.len() == 1);
    assert!(t.devices.iter().chain(t.instrument_plugin.iter()).all(|d| !d.enabled));
    assert!(t.instrument_plugin.is_some(), "the instrument stays, switched off");
}

#[test]
fn replace_track_is_one_exact_undo_step_and_round_trips() {
    let (mut d, inst, a) = doc();
    let before = d.project().tracks.clone();
    let p = d.project();
    let t = bounced_in_place(p.track(inst).unwrap(), audio_clip(p.next_id, a, 4 * PPQ));
    let cmd = Command::Batch { label: "Bounce Track in Place".into(), commands: vec![Command::ReplaceTrack { track: t }] };
    let json = serde_json::to_string(&cmd).unwrap();
    assert_eq!(serde_json::from_str::<Command>(&json).unwrap(), cmd);
    d.execute(cmd, Origin::User).unwrap();
    assert_eq!(d.undo_label(), Some("Bounce Track in Place"));
    let after = d.project().tracks.clone();
    assert_eq!(after[1].kind, TrackKind::Audio);
    assert_eq!(after.len(), before.len());
    d.undo().unwrap();
    assert_eq!(d.project().tracks, before);
    d.redo().unwrap();
    assert_eq!(d.project().tracks, after);
}

#[test]
fn replace_track_refuses_what_a_track_cannot_be() {
    let (mut d, inst, a) = doc();
    let before = d.project().clone();
    let src = before.track(inst).unwrap().clone();
    // An audio track with MIDI clips.
    let bad = Track { kind: TrackKind::Audio, ..src.clone() };
    assert!(d.execute(Command::ReplaceTrack { track: bad }, Origin::User).is_err());
    // A clip id another track uses.
    let other = before.tracks[0].id;
    d.execute(Command::AddClip { track: other, clip: audio_clip(500, a, 0) }, Origin::User).unwrap();
    let taken = bounced_in_place(&src, audio_clip(500, a, 0));
    assert!(d.execute(Command::ReplaceTrack { track: taken }, Origin::User).is_err());
    // A track that is not there.
    let gone = Track { id: TrackId(9_999), ..src };
    assert!(d.execute(Command::ReplaceTrack { track: gone }, Origin::User).is_err());
    assert_eq!(d.project().track(inst), before.track(inst));
}
