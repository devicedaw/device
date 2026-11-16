use dstudio_core::midi::{ClipLoop, Note};
use dstudio_core::*;
use std::collections::HashMap;

fn audio_doc() -> (Document, TrackId, ClipId) {
    let mut d = Document::new(Project::default());
    let track = d.project().tracks[0].id;
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
    let clip = Clip {
        id: ClipId(0),
        asset: a,
        name: "loop".into(),
        start: 4 * PPQ,
        source_offset: 0,
        source_length: 48_000 * 4,
        gain: 1.0,
        warp: Default::default(),
        disabled: false,
        clip_loop: None,
        fades: Default::default(),
        takes: Vec::new(),
    };
    d.execute(Command::AddClip { track, clip }, Origin::User).unwrap();
    let c = d.project().tracks[0].clips[0].id;
    (d, track, c)
}

fn midi_doc() -> (Document, ClipId) {
    let mut d = Document::new(Project::default());
    let inst = d.project().tracks[1].id;
    let mut clip = MidiClip::empty(PPQ, 4 * PPQ);
    clip.notes = vec![
        Note { id: 1, pitch: 60, start: 0, length: PPQ, velocity: 100 },
        Note { id: 2, pitch: 64, start: PPQ, length: 2 * PPQ, velocity: 90 },
        Note { id: 3, pitch: 67, start: 3 * PPQ, length: PPQ, velocity: 80 },
    ];
    d.execute(Command::AddMidiClip { track: inst, clip }, Origin::User).unwrap();
    let c = d.project().tracks[1].midi_clips[0].id;
    (d, c)
}

#[test]
fn split_audio_clip_divides_the_source_and_undoes() {
    let (mut d, _, c) = audio_doc();
    let before = d.project().tracks[0].clips[0].clone();
    d.execute(Command::SplitClip { clip: c, at: 4 * PPQ + 2 * PPQ, new_id: ClipId(0) }, Origin::User).unwrap();
    let clips = &d.project().tracks[0].clips;
    assert_eq!(clips.len(), 2);
    assert_eq!((clips[0].id, clips[0].start, clips[0].source_offset, clips[0].source_length), (c, 4 * PPQ, 0, 48_000));
    assert_eq!((clips[1].start, clips[1].source_offset, clips[1].source_length), (6 * PPQ, 48_000, 48_000 * 3));
    assert_eq!(clips[1].name, "loop");
    assert_eq!(d.undo_label(), Some("Split Clip"));
    d.undo().unwrap();
    assert_eq!(d.project().tracks[0].clips, vec![before]);
    d.redo().unwrap();
    assert_eq!(d.project().tracks[0].clips.len(), 2);
}

#[test]
fn split_audio_clip_refuses_the_edges_and_warped_clips() {
    let (mut d, _, c) = audio_doc();
    for at in [4 * PPQ, 5 * PPQ + 4 * 2 * PPQ, 0] {
        assert!(d.execute(Command::SplitClip { clip: c, at, new_id: ClipId(0) }, Origin::User).is_err(), "at {at}");
    }
    let warp = ClipWarp { reverse: true, ..ClipWarp::default() };
    d.execute(Command::SetClipWarp { clip: c, warp, label: "Reverse".into() }, Origin::User).unwrap();
    assert!(d.execute(Command::SplitClip { clip: c, at: 6 * PPQ, new_id: ClipId(0) }, Origin::User).is_err());
    assert_eq!(d.project().tracks[0].clips.len(), 1);
}

#[test]
fn split_midi_clip_cuts_crossing_notes_and_undoes() {
    let (mut d, c) = midi_doc();
    let before = d.project().tracks[1].midi_clips[0].clone();
    d.execute(Command::SplitClip { clip: c, at: PPQ + 2 * PPQ, new_id: ClipId(0) }, Origin::User).unwrap();
    let clips = &d.project().tracks[1].midi_clips;
    assert_eq!(clips.len(), 2);
    let (l, r) = (&clips[0], &clips[1]);
    assert_eq!((l.id, l.start, l.length), (c, PPQ, 2 * PPQ));
    assert_eq!((r.start, r.length), (3 * PPQ, 2 * PPQ));
    assert_eq!(l.notes.iter().map(|n| (n.pitch, n.start, n.length)).collect::<Vec<_>>(), vec![(60, 0, PPQ), (64, PPQ, PPQ)]);
    assert_eq!(r.notes.iter().map(|n| (n.pitch, n.start, n.length)).collect::<Vec<_>>(), vec![(64, 0, PPQ), (67, PPQ, PPQ)]);
    d.undo().unwrap();
    assert_eq!(d.project().tracks[1].midi_clips, vec![before]);
}

#[test]
fn split_refuses_pattern_clips_and_taken_ids() {
    let (mut d, c) = midi_doc();
    assert!(d.execute(Command::SplitClip { clip: c, at: 2 * PPQ, new_id: c }, Origin::User).is_err());
    assert!(d.execute(Command::SplitClip { clip: ClipId(9999), at: 2 * PPQ, new_id: ClipId(0) }, Origin::User).is_err());
    let mut p = Project::default();
    let inst = p.tracks[1].id;
    let pat = MidiClip::empty_pattern(0, StepRows::default());
    let mut d = Document::new(std::mem::take(&mut p));
    d.execute(Command::AddMidiClip { track: inst, clip: pat }, Origin::User).unwrap();
    let id = d.project().tracks[1].midi_clips[0].id;
    assert!(d.execute(Command::SplitClip { clip: id, at: PPQ, new_id: ClipId(0) }, Origin::User).is_err());
}

#[test]
fn disable_clip_toggles_and_undoes_for_audio_and_midi() {
    let (mut d, _, c) = audio_doc();
    d.execute(Command::SetClipEnabled { clip: c, enabled: false }, Origin::User).unwrap();
    assert!(d.project().tracks[0].clips[0].disabled);
    assert_eq!(d.undo_label(), Some("Disable Clip"));
    d.undo().unwrap();
    assert!(!d.project().tracks[0].clips[0].disabled);
    let (mut d, m) = midi_doc();
    d.execute(Command::SetClipEnabled { clip: m, enabled: false }, Origin::User).unwrap();
    assert!(d.project().midi_clip(m).unwrap().disabled);
    d.execute(Command::SetClipEnabled { clip: m, enabled: true }, Origin::User).unwrap();
    assert_eq!(d.undo_label(), Some("Enable Clip"));
    assert!(d.execute(Command::SetClipEnabled { clip: ClipId(9999), enabled: false }, Origin::User).is_err());
}

#[test]
fn disabled_clips_are_left_out_of_the_session_snapshot_and_persist() {
    let (mut d, _, c) = audio_doc();
    let a = d.project().assets[0].id;
    let engine_assets = HashMap::from([(a, 1u64)]);
    let (s, _) = snapshot::build(d.project(), 48_000.0, &engine_assets);
    assert_eq!(s.tracks[0].clips.len(), 1);
    d.execute(Command::SetClipEnabled { clip: c, enabled: false }, Origin::User).unwrap();
    let (s, _) = snapshot::build(d.project(), 48_000.0, &engine_assets);
    assert!(s.tracks[0].clips.is_empty());
    let json = serde_json::to_string(d.project()).unwrap();
    let back: Project = serde_json::from_str(&json).unwrap();
    assert!(back.tracks[0].clips[0].disabled);

    let (mut d, m) = midi_doc();
    let (s, _) = snapshot::build(d.project(), 48_000.0, &HashMap::new());
    assert_eq!(s.tracks[1].notes.len(), 3);
    d.execute(Command::SetClipEnabled { clip: m, enabled: false }, Origin::User).unwrap();
    let (s, _) = snapshot::build(d.project(), 48_000.0, &HashMap::new());
    assert!(s.tracks[1].notes.is_empty());
}

// ---- Consolidate, Bounce and Export (render_plan) ----

fn audio_clip(id: u64, asset: AssetId, start: Ticks, secs: u64) -> Clip {
    Clip {
        id: ClipId(id),
        asset,
        name: format!("c{id}"),
        start,
        source_offset: 0,
        source_length: 48_000 * secs,
        gain: 1.0,
        warp: Default::default(),
        disabled: false,
        clip_loop: None,
        fades: Default::default(),
        takes: Vec::new(),
    }
}

/// Audio track with clips at beats 4-8, 6-10 (overlapping), 10-12 (touching) and 20-24 (apart);
/// at 120 BPM a beat is 0.5 s.
fn overlap_doc() -> (Document, TrackId) {
    let mut d = Document::new(Project::default());
    let track = d.project().tracks[0].id;
    d.execute(
        Command::AddAsset {
            asset: Asset {
                id: AssetId(0),
                relative_path: None,
                absolute_path: "/nonexistent/t.wav".into(),
                sample_rate: 48_000.0,
                channels: 2,
                frames: 48_000 * 10,
            },
        },
        Origin::User,
    )
    .unwrap();
    let a = d.project().assets[0].id;
    for (id, beat, secs) in [(101, 4, 2), (102, 6, 2), (103, 10, 1), (104, 20, 2)] {
        d.execute(Command::AddClip { track, clip: audio_clip(id, a, beat * PPQ, secs) }, Origin::User).unwrap();
    }
    (d, track)
}

#[test]
fn consolidate_joins_overlapping_clips_transitively_and_leaves_the_rest() {
    let (d, track) = overlap_doc();
    let p = d.project();
    let s = render_plan::consolidate_group(p, ClipId(101)).unwrap();
    assert_eq!((s.track, s.kind, s.start, s.end), (track, TrackKind::Audio, 4 * PPQ, 10 * PPQ));
    assert_eq!(s.clips, vec![ClipId(101), ClipId(102)], "touching and distant clips are not joined");
    assert_eq!(render_plan::consolidate_group(p, ClipId(102)).unwrap(), s, "same group from either clip");
    let lone = render_plan::consolidate_group(p, ClipId(104)).unwrap();
    assert_eq!((lone.clips.len(), lone.start, lone.end), (1, 20 * PPQ, 24 * PPQ), "one clip consolidates alone (bakes its edits)");
}

#[test]
fn consolidate_skips_disabled_clips() {
    let (mut d, _) = overlap_doc();
    d.execute(Command::SetClipEnabled { clip: ClipId(102), enabled: false }, Origin::User).unwrap();
    let s = render_plan::consolidate_group(d.project(), ClipId(101)).unwrap();
    assert_eq!((s.clips, s.end), (vec![ClipId(101)], 8 * PPQ));
    assert!(render_plan::consolidate_group(d.project(), ClipId(102)).is_none(), "a disabled clip cannot be consolidated");
    assert!(render_plan::consolidate_group(d.project(), ClipId(999)).is_none());
}

#[test]
fn consolidate_group_extends_across_a_chain_of_overlaps() {
    let (mut d, track) = overlap_doc();
    // A clip from beat 9 to 11 overlaps 102 (6-10) and 103 (10-12 touches only through it).
    let a = d.project().assets[0].id;
    d.execute(Command::AddClip { track, clip: audio_clip(105, a, 9 * PPQ, 1) }, Origin::User).unwrap();
    let s = render_plan::consolidate_group(d.project(), ClipId(101)).unwrap();
    assert_eq!(s.clips, vec![ClipId(101), ClipId(102), ClipId(105), ClipId(103)]);
    assert_eq!((s.start, s.end), (4 * PPQ, 12 * PPQ));
}

#[test]
fn render_project_keeps_one_track_and_the_chosen_clips() {
    let (mut d, track) = overlap_doc();
    d.execute(Command::SetTrackGain { track, gain: 0.5 }, Origin::User).unwrap();
    d.execute(Command::SetTrackPan { track, pan: -0.5 }, Origin::User).unwrap();
    d.execute(Command::SetMasterGain { gain: 0.25 }, Origin::User).unwrap();
    let p = d.project();
    let s = render_plan::consolidate_group(p, ClipId(101)).unwrap();
    let r = render_plan::render_project(p, &s, render_plan::RenderMode::Material);
    let t = r.track(track).unwrap();
    assert_eq!(t.clips.iter().map(|c| c.id).collect::<Vec<_>>(), vec![ClipId(101), ClipId(102)]);
    assert!(
        !t.mute && !t.solo && t.gain == render_plan::CENTRE_UNITY_GAIN && t.pan == 0.0 && r.master_gain == 1.0,
        "unity through the centre pan law, neutral master"
    );
    assert!(r.tracks.iter().filter(|o| o.id != track).all(|o| o.mute && o.clips.is_empty() && o.midi_clips.is_empty()));
    let stem = render_plan::render_project(p, &s, render_plan::RenderMode::Stem);
    assert!(
        stem.track(track).unwrap().gain == 0.5 && stem.track(track).unwrap().pan == -0.5 && stem.master_gain == 0.25,
        "a stem keeps the mix settings"
    );
    assert_eq!(p.track(track).unwrap().clips.len(), 4, "the project is not touched");
}

#[test]
fn render_project_takes_consolidate_and_bounce_before_a_muted_master() {
    let (mut d, track) = overlap_doc();
    d.execute(Command::SetMasterMute { mute: true }, Origin::User).unwrap();
    let p = d.project();
    let s = render_plan::consolidate_group(p, ClipId(101)).unwrap();
    for mode in [render_plan::RenderMode::Material, render_plan::RenderMode::Bounce] {
        let r = render_plan::render_project(p, &s, mode);
        let (snap, _) = snapshot::build(&r, 48_000.0, &HashMap::new());
        assert!(!r.master_mute && snap.master_gain == 1.0, "{mode:?}: a muted master would render (and replace the clips with) silence");
    }
    let stem = render_plan::render_project(p, &s, render_plan::RenderMode::Stem);
    assert!(stem.master_mute, "an export sounds as the stems export does, through the muted master");
    assert!(p.master_mute && p.track(track).is_some(), "the project is not touched");
}

#[test]
fn render_project_keeps_no_clips_samples_or_lanes_on_the_other_tracks() {
    let (mut d, track) = overlap_doc();
    let inst = d.project().tracks[1].id;
    let mut clip = MidiClip::empty(0, 4 * PPQ);
    clip.notes = vec![Note { id: 1, pitch: 60, start: 0, length: PPQ, velocity: 100 }];
    d.execute(Command::AddMidiClip { track: inst, clip }, Origin::User).unwrap();
    let lane =
        AutomationLane { target: AutomationTarget::Mute, enabled: true, points: vec![AutoPoint { time: 0, value: 0.0, curve: 0.0 }] };
    d.execute(Command::SetAutomationLane { track: Some(inst), target: AutomationTarget::Mute, lane: Some(lane) }, Origin::User).unwrap();
    d.execute(Command::SetInstrumentPlugin { track: inst, device: Some(builtin::find("drum-rack").unwrap().instantiate()) }, Origin::User)
        .unwrap();
    let dev = d.project().track(inst).unwrap().instrument_plugin.as_ref().unwrap().id;
    let asset = d.project().assets[0].id;
    let sample = DeviceSample { slot: 0, asset, name: "kick".into(), slices: vec![] };
    d.execute(Command::SetDeviceSample { device: dev, slot: 0, sample: Some(sample) }, Origin::User).unwrap();
    let s = render_plan::consolidate_group(d.project(), ClipId(101)).unwrap();
    for mode in [render_plan::RenderMode::Material, render_plan::RenderMode::Bounce, render_plan::RenderMode::Stem] {
        let r = render_plan::render_project(d.project(), &s, mode);
        let o = r.track(inst).unwrap();
        assert!(o.mute && o.clips.is_empty() && o.midi_clips.is_empty(), "{mode:?}: a muted track's clips are not loaded");
        assert!(o.instrument_plugin.as_ref().is_some_and(|i| i.samples.is_empty()), "{mode:?}: nor its samples");
        assert!(o.automation.is_empty(), "{mode:?}: no mute lane brings the muted track back");
        assert_eq!(r.track(track).unwrap().clips.len(), 2, "{mode:?}: the span's clips stay");
    }
}

#[test]
fn render_project_clears_devices_only_for_material() {
    let mut d = Document::new(Project::default());
    let track = d.project().tracks[0].id;
    let dev = dstudio_core::builtin::find("reverb").expect("reverb exists").instantiate();
    d.execute(Command::AddDevice { track, index: None, device: dev }, Origin::User).unwrap();
    let lane =
        AutomationLane { target: AutomationTarget::Volume, enabled: true, points: vec![AutoPoint { time: 0, value: -6.0, curve: 0.0 }] };
    d.execute(Command::SetAutomationLane { track: Some(track), target: AutomationTarget::Volume, lane: Some(lane) }, Origin::User).unwrap();
    let span = render_plan::ClipSpan { track, kind: TrackKind::Audio, clips: vec![], start: 0, end: PPQ };
    let p = d.project();
    assert!(render_plan::rings_out(p, track, render_plan::RenderMode::Bounce));
    assert!(!render_plan::rings_out(p, track, render_plan::RenderMode::Material));
    assert!(render_plan::render_project(p, &span, render_plan::RenderMode::Material).track(track).unwrap().devices.is_empty());
    let b = render_plan::render_project(p, &span, render_plan::RenderMode::Bounce);
    assert_eq!(b.track(track).unwrap().devices.len(), 1, "a bounce renders through the devices");
    assert!(b.track(track).unwrap().automation.is_empty(), "volume automation moves with the fader, not into the audio");
    let inst = p.tracks[1].id;
    assert!(render_plan::rings_out(p, inst, render_plan::RenderMode::Stem), "an instrument track has a release tail");
}

#[test]
fn merge_midi_moves_notes_to_the_span_start_and_cuts_at_each_clip_end() {
    let mut d = Document::new(Project::default());
    let inst = d.project().tracks[1].id;
    let note = |id, pitch, start, length| Note { id, pitch, start, length, velocity: 100 };
    let mut a = MidiClip::empty(4 * PPQ, 4 * PPQ);
    a.notes = vec![note(1, 60, 0, PPQ), note(2, 64, 3 * PPQ, 3 * PPQ), note(3, 67, 5 * PPQ, PPQ)];
    a.name = "riff".into();
    let mut b = MidiClip::empty(6 * PPQ, 4 * PPQ);
    b.notes = vec![note(1, 72, 0, PPQ)];
    d.execute(Command::AddMidiClip { track: inst, clip: a }, Origin::User).unwrap();
    d.execute(Command::AddMidiClip { track: inst, clip: b }, Origin::User).unwrap();
    let ids: Vec<ClipId> = d.project().tracks[1].midi_clips.iter().map(|c| c.id).collect();
    let span = render_plan::consolidate_group(d.project(), ids[0]).unwrap();
    assert_eq!((span.kind, span.start, span.end, span.clips.len()), (TrackKind::Instrument, 4 * PPQ, 10 * PPQ, 2));
    let m = render_plan::merge_midi(d.project(), &span).unwrap();
    assert_eq!((m.name.as_str(), m.start, m.length), ("riff merged", 4 * PPQ, 6 * PPQ));
    let got: Vec<(u8, Ticks, Ticks)> = m.notes.iter().map(|n| (n.pitch, n.start, n.length)).collect();
    assert_eq!(
        got,
        vec![(60, 0, PPQ), (72, 2 * PPQ, PPQ), (64, 3 * PPQ, PPQ)],
        "the note past the clip end is dropped, the long one is cut at its clip's end"
    );
    assert!(m.notes.iter().enumerate().all(|(i, n)| n.id == i as u32 + 1));
}

#[test]
fn render_project_leaves_plugin_devices_off_the_other_tracks() {
    let (mut d, track) = overlap_doc();
    let clap = PluginRef {
        format: PluginFormat::Clap,
        id: "test.plugin".into(),
        path: None,
        name: "Test".into(),
        vendor: String::new(),
        version: String::new(),
        kind: PluginKind::Effect,
    };
    let inst = d.project().tracks[1].id;
    d.execute(Command::AddDevice { track: inst, index: None, device: Device::new(clap) }, Origin::User).unwrap();
    let span = render_plan::consolidate_group(d.project(), ClipId(101)).unwrap();
    assert_eq!(span.track, track);
    let r = render_plan::render_project(d.project(), &span, render_plan::RenderMode::Bounce);
    assert!(r.track(inst).unwrap().devices.is_empty() && r.track(inst).unwrap().mute, "the muted instrument track keeps no plugin");
    assert_eq!(d.project().track(inst).unwrap().devices.len(), 1, "the project itself is not touched");
}

#[test]
fn merge_midi_plays_pattern_clips_repeated_and_without_muted_rows() {
    let mut d = Document::new(Project::default());
    let inst = d.project().tracks[1].id;
    let mut clip = MidiClip::empty_pattern(0, StepRows::default());
    let mut pat = clip.pattern.take().unwrap();
    pat.cells = vec![StepCell::new(36, 0), StepCell::new(38, 2)];
    pat.muted = vec![38];
    let period = pat.length();
    clip.notes = pat.bake();
    clip.length = 2 * period;
    clip.pattern = Some(pat);
    assert!(clip.notes.iter().any(|n| n.pitch == 38), "the baked pass keeps the muted row's notes");
    d.execute(Command::AddMidiClip { track: inst, clip }, Origin::User).unwrap();
    let id = d.project().tracks[1].midi_clips[0].id;
    let span = render_plan::consolidate_group(d.project(), id).unwrap();
    let m = render_plan::merge_midi(d.project(), &span).unwrap();
    let got: Vec<(u8, Ticks)> = m.notes.iter().map(|n| (n.pitch, n.start)).collect();
    assert_eq!(got.len(), 2, "{got:?}");
    assert!(got.iter().all(|n| n.0 == 36), "the muted row is left out: {got:?}");
    assert_eq!((got[0].1, got[1].1), (0, period), "the pattern repeats over the clip");
}

#[test]
fn a_bounce_track_sends_and_is_routed_where_its_source_is() {
    let (mut d, track) = overlap_doc();
    let group = Track::new(TrackId(0), TrackKind::Group, "Group", ColorIndex(3));
    d.execute(Command::AddTrack { index: Some(0), track: group }, Origin::User).unwrap();
    let ret = Track::new(TrackId(0), TrackKind::Return, "Return A", ColorIndex(9));
    d.execute(Command::AddTrack { index: None, track: ret }, Origin::User).unwrap();
    let (g, r) = (d.project().tracks[0].id, d.project().tracks.last().unwrap().id);
    d.execute(Command::SetTrackOutput { track, output: Some(g) }, Origin::User).unwrap();
    let sends = vec![TrackSend { level: 0.5, pre_fader: true, ..TrackSend::new(r) }];
    d.execute(Command::SetSends { track, sends: sends.clone() }, Origin::User).unwrap();
    d.execute(Command::SetTrackGain { track, gain: 0.5 }, Origin::User).unwrap();
    let before = d.project().clone();
    let src = d.project().track(track).unwrap();
    let id = TrackId(d.project().next_id);
    let t = render_plan::bounce_track(src, id);
    assert_eq!((t.name.as_str(), t.kind, t.gain), ("Audio (Bounce)", TrackKind::Audio, 0.5));
    assert_eq!((&t.sends, t.output), (&sends, Some(g)), "the bounce keeps the source's sends and group");
    let index = d.project().track_index(track).unwrap() + 1;
    d.execute(Command::AddTrack { index: Some(index), track: t }, Origin::User).unwrap();
    let added = d.project().track(id).unwrap();
    assert_eq!((&added.sends, added.output), (&sends, Some(g)));
    d.undo().unwrap();
    assert!(d.project().tracks == before.tracks, "one undo removes it");
}

#[test]
fn consolidate_and_bounce_batches_are_one_undo_step() {
    let (mut d, track) = overlap_doc();
    let before = d.project().clone();
    let s = render_plan::consolidate_group(d.project(), ClipId(101)).unwrap();
    let p = d.project();
    let asset = Asset {
        id: AssetId(500),
        relative_path: None,
        absolute_path: "/nonexistent/merged.wav".into(),
        sample_rate: 48_000.0,
        channels: 2,
        frames: 48_000 * 3,
    };
    let mut cmds = vec![Command::AddAsset { asset }];
    cmds.extend(s.clips.iter().map(|&clip| Command::RemoveClip { clip }));
    cmds.push(Command::AddClip {
        track,
        clip: Clip {
            id: ClipId(501),
            name: render_plan::merged_name(&p.clip(s.clips[0]).unwrap().name),
            start: s.start,
            ..audio_clip(501, AssetId(500), 0, 3)
        },
    });
    d.execute(Command::Batch { label: "Consolidate Clips".into(), commands: cmds }, Origin::User).unwrap();
    let ids: Vec<u64> = d.project().tracks[0].clips.iter().map(|c| c.id.0).collect();
    assert_eq!(ids, vec![501, 103, 104], "clips stay in start order");
    assert_eq!(d.project().clip(ClipId(501)).unwrap().name, "c101 merged");
    assert_eq!(d.undo_label(), Some("Consolidate Clips"));
    d.undo().unwrap();
    assert!(
        d.project().tracks == before.tracks && d.project().assets == before.assets,
        "one undo restores clips and assets (ids are never recycled, so next_id stays)"
    );

    let mut bounce = Track::new(TrackId(600), TrackKind::Audio, "Audio (Bounce)", d.project().tracks[0].color);
    bounce.clips.push(audio_clip(601, AssetId(500), 4 * PPQ, 3));
    d.execute(
        Command::Batch {
            label: "Bounce to New Track".into(),
            commands: vec![
                Command::AddAsset {
                    asset: Asset {
                        id: AssetId(500),
                        relative_path: None,
                        absolute_path: "/nonexistent/b.wav".into(),
                        sample_rate: 48_000.0,
                        channels: 2,
                        frames: 48_000 * 3,
                    },
                },
                Command::AddTrack { index: Some(1), track: bounce },
                Command::SetClipEnabled { clip: ClipId(101), enabled: false },
            ],
        },
        Origin::User,
    )
    .unwrap();
    assert_eq!(d.project().tracks[1].name, "Audio (Bounce)");
    assert!(d.project().clip(ClipId(101)).unwrap().disabled);
    d.undo().unwrap();
    assert!(
        d.project().tracks == before.tracks && d.project().assets == before.assets,
        "one undo removes the track, the asset and the mute"
    );
}

#[test]
fn split_refuses_a_one_frame_audio_clip() {
    // At 1 Hz one frame lasts two beats, so a cut can fall inside a clip one frame long.
    let mut d = Document::new(Project::default());
    let track = d.project().tracks[0].id;
    let asset = Asset {
        id: AssetId(0),
        relative_path: None,
        absolute_path: "/nonexistent/s.wav".into(),
        sample_rate: 1.0,
        channels: 1,
        frames: 10,
    };
    d.execute(Command::AddAsset { asset }, Origin::User).unwrap();
    let a = d.project().assets[0].id;
    let clip = Clip {
        id: ClipId(0),
        asset: a,
        name: "s".into(),
        start: 0,
        source_offset: 0,
        source_length: 1,
        gain: 1.0,
        warp: Default::default(),
        disabled: false,
        clip_loop: None,
        fades: Default::default(),
        takes: Vec::new(),
    };
    d.execute(Command::AddClip { track, clip }, Origin::User).unwrap();
    let c = d.project().tracks[0].clips[0].id;
    let before = d.project().clone();
    assert!(d.execute(Command::SplitClip { clip: c, at: PPQ, new_id: ClipId(0) }, Origin::User).is_err());
    assert_eq!(d.project(), &before);
}

/// The source frame an audio clip plays at timeline tick `at`, following its loop.
fn frame_at(p: &Project, c: &Clip, at: Ticks) -> Option<i64> {
    let rel = at - c.start;
    if rel < 0 || rel >= p.clip_length_ticks(c) {
        return None;
    }
    let pos = match c.clip_loop.filter(|l| l.on) {
        Some(l) => {
            let le = l.end.min(p.content_length_ticks(c));
            if rel < le { rel } else { l.start + (rel - le) % (le - l.start) }
        }
        None => rel,
    };
    Some(p.clip_source_at(c, pos as f64).round() as i64)
}

#[test]
fn split_looped_audio_clip_keeps_what_plays() {
    // 4 beats of audio looping beats 1..3, 16 beats on the timeline: the cuts fall before the
    // loop, in its first pass, in a later repeat and on a repeat boundary.
    for cut in [PPQ / 2, 2 * PPQ, 9 * PPQ + PPQ / 3, 7 * PPQ] {
        let (mut d, _, c) = audio_doc();
        let l = AudioLoop { on: true, start: PPQ, end: 3 * PPQ, length: 16 * PPQ };
        d.execute(Command::SetClipLoop { clip: c, clip_loop: Some(l) }, Origin::User).unwrap();
        let fades = ClipFades { fade_in: PPQ / 4, fade_out: PPQ / 4 };
        d.execute(Command::SetClipFades { clip: c, fades }, Origin::User).unwrap();
        let before = d.project().clone();
        let orig = before.tracks[0].clips[0].clone();
        let at = orig.start + cut;
        d.execute(Command::SplitClip { clip: c, at, new_id: ClipId(0) }, Origin::User).unwrap();
        let p = d.project();
        let parts = &p.tracks[0].clips;
        assert!(parts.len() >= 2, "cut {cut}");
        assert_eq!(parts[0].id, c);
        assert_eq!(parts[0].start + p.clip_length_ticks(&parts[0]), at, "cut {cut}: left part ends at the cut");
        assert_eq!(parts[1].start, at);
        for w in parts.windows(2) {
            assert_eq!(w[0].start + p.clip_length_ticks(&w[0]), w[1].start, "cut {cut}: parts follow each other");
        }
        let last = parts.last().unwrap();
        assert_eq!(last.start + p.clip_length_ticks(last), orig.start + 16 * PPQ, "cut {cut}: ends where the clip ended");
        assert_eq!((parts[0].fades.fade_in, last.fades.fade_out), (PPQ / 4, PPQ / 4));
        for t in (orig.start..orig.start + 16 * PPQ).step_by(PPQ as usize / 8) {
            let want = frame_at(&before, &orig, t).unwrap();
            let got = parts.iter().find_map(|cl| frame_at(p, cl, t)).unwrap();
            assert!((want - got).abs() <= 1, "cut {cut}: at tick {t} plays frame {got}, was {want}");
        }
        let n = parts.len();
        d.undo().unwrap();
        assert_eq!(d.project().tracks, before.tracks, "cut {cut}");
        d.redo().unwrap();
        assert_eq!(d.project().tracks[0].clips.len(), n);
    }
}

#[test]
fn split_looped_midi_clip_keeps_its_repeats() {
    let mut d = Document::new(Project::default());
    let inst = d.project().tracks[1].id;
    let mut clip = MidiClip::empty(PPQ, 4 * PPQ);
    clip.notes = vec![
        Note { id: 1, pitch: 60, start: 0, length: PPQ / 2, velocity: 100 },
        Note { id: 2, pitch: 62, start: PPQ / 2, length: PPQ, velocity: 90 },
    ];
    d.execute(Command::AddMidiClip { track: inst, clip }, Origin::User).unwrap();
    let c = d.project().tracks[1].midi_clips[0].id;
    // Loop the first beat over four beats: the notes repeat on every beat.
    let l = ClipLoop { on: true, start: 0, end: PPQ, span: 0 };
    d.execute(Command::SetMidiLoop { clip: c, clip_loop: Some(l), length: 4 * PPQ }, Origin::User).unwrap();
    let before = d.project().clone();
    let playing = |p: &Project| {
        let mut v: Vec<_> = p.tracks[1]
            .midi_clips
            .iter()
            .flat_map(|cl| cl.tiled_notes().into_iter().map(|n| (cl.start + n.start, n.length, n.pitch)))
            .collect();
        v.sort();
        v
    };
    d.execute(Command::SplitClip { clip: c, at: PPQ + 2 * PPQ + PPQ / 4, new_id: ClipId(0) }, Origin::User).unwrap();
    assert_eq!(d.project().tracks[1].midi_clips.len(), 2);
    let mut want = playing(&before);
    // The note that crosses the cut plays as two notes.
    let crossing = want.iter().position(|&(s, _, pitch)| s == 3 * PPQ && pitch == 60).unwrap();
    let (s, len, pitch) = want.remove(crossing);
    let cut = PPQ + 2 * PPQ + PPQ / 4;
    want.extend([(s, cut - s, pitch), (cut, s + len - cut, pitch)]);
    want.sort();
    assert_eq!(playing(d.project()), want);
    for cl in &d.project().tracks[1].midi_clips {
        midi::validate_notes(&cl.notes).unwrap();
    }
    d.undo().unwrap();
    assert_eq!(d.project().tracks, before.tracks);
}
