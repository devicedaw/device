//! Tempo changes during a song (spec 015): the map's commands with undo, persistence and the
//! schema guard, conversions through steps and ramps, and the snapshot the engine plays.

use dstudio_core::automation::{AutoPoint, AutomationLane, AutomationTarget};
use dstudio_core::snapshot;
use dstudio_core::warp::{ClipWarp, WarpMode};
use dstudio_core::*;
use std::collections::HashMap;
use std::time::{Duration, Instant};

const RATE: f64 = 48_000.0;
const BAR: Ticks = 4 * PPQ;

fn pt(beat: i64, bpm: f64, ramp: bool) -> TempoPoint {
    TempoPoint { time: beat * PPQ, bpm, ramp }
}

/// 120 BPM for two bars, a step to 90, a ramp from bar 4 to 150 at bar 6, then 150.
fn song_map() -> Vec<TempoPoint> {
    vec![pt(0, 120.0, false), pt(8, 90.0, false), pt(12, 90.0, true), pt(20, 150.0, false)]
}

fn with_map(points: Vec<TempoPoint>) -> Document {
    let mut d = Document::new(Project::default());
    d.execute(Command::SetTempoMap { points, bpm: None }, Origin::User).unwrap();
    d
}

#[test]
fn map_edits_undo_and_redo_with_their_labels() {
    let mut d = Document::new(Project::default());
    d.execute(Command::SetTempo { bpm: 100.0, at: None }, Origin::User).unwrap();
    let before = d.project().clone();
    d.execute(Command::SetTempoMap { points: song_map(), bpm: None }, Origin::User).unwrap();
    assert_eq!(d.undo_label(), Some("Edit Tempo Automation"));
    assert_eq!(d.project().transport.bpm, 120.0, "the song tempo is the tempo at bar 1");
    assert!(d.project().has_tempo_changes());
    d.undo().unwrap();
    assert_eq!(d.project(), &before, "undo puts the single tempo back");
    d.redo().unwrap();
    assert_eq!(d.project().transport.tempo, song_map());

    // Removing the lane keeps the tempo the song had at bar 1.
    let mut edited = song_map();
    edited[0].bpm = 132.0;
    d.execute(Command::SetTempoMap { points: edited, bpm: None }, Origin::User).unwrap();
    d.execute(Command::SetTempoMap { points: vec![], bpm: None }, Origin::User).unwrap();
    assert_eq!(d.undo_label(), Some("Delete Tempo Automation"));
    assert_eq!((d.project().transport.bpm, d.project().has_tempo_changes()), (132.0, false));
    d.undo().unwrap();
    assert_eq!(d.project().transport.tempo[0].bpm, 132.0);
}

#[test]
fn the_transport_field_edits_the_point_in_effect() {
    let mut d = with_map(song_map());
    // Inside the step at bar 3: the point at beat 8.
    d.execute(Command::SetTempo { bpm: 95.0, at: Some(9 * PPQ) }, Origin::User).unwrap();
    assert_eq!(d.undo_label(), Some("Tempo Change"));
    assert_eq!(d.project().transport.tempo[1].bpm, 95.0);
    assert_eq!(d.project().bpm_at(9 * PPQ), 95.0);
    // Before the first change (and with no position): the first point, which is the song tempo.
    d.execute(Command::SetTempo { bpm: 128.0, at: Some(0) }, Origin::User).unwrap();
    assert_eq!((d.project().transport.tempo[0].bpm, d.project().transport.bpm), (128.0, 128.0));
    d.undo().unwrap();
    d.undo().unwrap();
    assert_eq!(d.project().transport.tempo, song_map());
    // A map that starts after bar 1: the first point holds before it and is the one edited there.
    let mut d = with_map(vec![pt(8, 140.0, false)]);
    d.execute(Command::SetTempo { bpm: 141.0, at: Some(0) }, Origin::User).unwrap();
    assert_eq!((d.project().transport.tempo[0].bpm, d.project().bpm_at(0)), (141.0, 141.0));
}

#[test]
fn invalid_edits_change_nothing() {
    let mut d = with_map(song_map());
    let before = d.project().clone();
    let bad = [
        vec![pt(4, 120.0, false), pt(0, 100.0, false)],
        vec![pt(0, 120.0, false), pt(0, 100.0, false)],
        vec![TempoPoint { time: -1, bpm: 120.0, ramp: false }],
        vec![pt(0, 19.9, false)],
        vec![pt(0, 1000.0, false)],
        vec![pt(0, f64::NAN, false)],
    ];
    for points in bad {
        assert!(d.execute(Command::SetTempoMap { points, bpm: None }, Origin::User).is_err());
    }
    assert!(d.execute(Command::SetTempoMap { points: vec![], bpm: Some(5.0) }, Origin::User).is_err());
    assert!(d.execute(Command::SetTempo { bpm: 120.0, at: Some(-1) }, Origin::User).is_err());
    assert!(d.execute(Command::SetTempo { bpm: 2000.0, at: Some(0) }, Origin::User).is_err());
    assert_eq!(d.project(), &before);
}

#[test]
fn a_held_tempo_drag_is_one_step_per_point() {
    let mut d = with_map(song_map());
    let t0 = Instant::now();
    d.execute_at(Command::SetTempo { bpm: 91.0, at: Some(8 * PPQ) }, t0).unwrap();
    d.hold_coalescing();
    for i in 1..10 {
        d.execute_at(Command::SetTempo { bpm: 91.0 + i as f64, at: Some(8 * PPQ) }, t0 + Duration::from_secs(i)).unwrap();
    }
    d.seal_coalescing();
    // Another point is its own step even right away.
    d.execute_at(Command::SetTempo { bpm: 99.0, at: Some(0) }, t0 + Duration::from_secs(10)).unwrap();
    d.undo().unwrap();
    assert_eq!((d.project().transport.tempo[0].bpm, d.project().transport.tempo[1].bpm), (120.0, 100.0));
    d.undo().unwrap();
    assert_eq!(d.project().transport.tempo, song_map(), "one step for the whole drag");
}

#[test]
fn commands_round_trip_as_json() {
    for cmd in [
        Command::SetTempoMap { points: song_map(), bpm: None },
        Command::SetTempoMap { points: vec![], bpm: Some(97.0) },
        Command::SetTempo { bpm: 128.0, at: Some(PPQ) },
    ] {
        let json = serde_json::to_string(&cmd).unwrap();
        assert_eq!(serde_json::from_str::<Command>(&json).unwrap(), cmd, "{json}");
    }
    // The assistant's set_tempo has no position.
    let c: Command = serde_json::from_str(r#"{"op":"set_tempo","bpm":128}"#).unwrap();
    assert_eq!(c, Command::SetTempo { bpm: 128.0, at: None });
}

fn saved(p: &Project) -> (tempfile::TempDir, serde_json::Value) {
    let dir = tempfile::tempdir().unwrap();
    persist::save(p, dir.path()).unwrap();
    let v = serde_json::from_slice(&std::fs::read(dir.path().join(persist::PROJECT_FILE)).unwrap()).unwrap();
    (dir, v)
}

#[test]
fn a_map_is_saved_as_schema_2_and_a_song_without_one_as_schema_1() {
    let d = with_map(song_map());
    let (dir, v) = saved(d.project());
    assert_eq!(v["schema_version"], 2);
    assert_eq!(v["project"]["transport"]["tempo"][2]["ramp"], true);
    assert_eq!(persist::load(dir.path()).unwrap().project, *d.project());

    // Without changes the file is exactly what a 0.2.x build writes and reads.
    let p = Project::default();
    let (dir, v) = saved(&p);
    assert_eq!(v["schema_version"], 1);
    assert!(v["project"]["transport"].get("tempo").is_none());
    assert_eq!(persist::load(dir.path()).unwrap().project, p);
}

#[test]
fn a_020_project_loads_as_a_single_tempo() {
    let dir = tempfile::tempdir().unwrap();
    let mut p = serde_json::to_value(Project::default()).unwrap();
    p["transport"]["bpm"] = 87.5.into();
    p["transport"].as_object_mut().unwrap().remove("tempo");
    let env = serde_json::json!({"format": persist::FORMAT, "schema_version": 1, "saved_with": "dstudio-core 0.2.0", "project": p});
    std::fs::write(dir.path().join(persist::PROJECT_FILE), serde_json::to_vec(&env).unwrap()).unwrap();
    let l = persist::load(dir.path()).unwrap().project;
    assert_eq!((l.transport.bpm, l.has_tempo_changes()), (87.5, false));
    assert_eq!(l.ticks_to_frames(BAR, RATE), (4.0 * 60.0 / 87.5 * RATE).round() as i64);
}

#[test]
fn the_schema_version_guards_a_project_with_changes() {
    // A build that reads schema 1 (0.2.x) refuses what this one writes for a song with changes;
    // the same check makes this build refuse a newer schema.
    let (dir, mut v) = saved(with_map(song_map()).project());
    assert!(v["schema_version"].as_u64().unwrap() > 1);
    v["schema_version"] = (persist::SCHEMA_VERSION + 1).into();
    std::fs::write(dir.path().join(persist::PROJECT_FILE), serde_json::to_vec(&v).unwrap()).unwrap();
    assert!(matches!(persist::load(dir.path()), Err(persist::PersistError::TooNew { .. })));
}

#[test]
fn a_damaged_map_is_repaired_on_load() {
    let (dir, mut v) = saved(with_map(song_map()).project());
    v["project"]["transport"]["tempo"] = serde_json::json!([
        {"time": 3840, "bpm": 2000.0}, {"time": 0, "bpm": 100.0}, {"time": 0, "bpm": 50.0}, {"time": -5, "bpm": 90.0}
    ]);
    v["project"]["transport"]["bpm"] = 61.0.into();
    std::fs::write(dir.path().join(persist::PROJECT_FILE), serde_json::to_vec(&v).unwrap()).unwrap();
    let t = persist::load(dir.path()).unwrap().project.transport;
    assert_eq!(t.tempo, vec![TempoPoint { time: 0, bpm: 100.0, ramp: false }, TempoPoint { time: 3840, bpm: 999.0, ramp: false }]);
    assert_eq!(t.bpm, 100.0);
}

#[test]
fn ticks_and_frames_round_trip_over_steps_and_ramps() {
    let d = with_map(song_map());
    let p = d.project();
    // The step: bars 1-2 at 120 (4 s), then 90.
    assert_eq!(p.ticks_to_frames(8 * PPQ, RATE), 4 * 48_000);
    assert_eq!(p.ticks_to_frames(9 * PPQ, RATE), (4.0 * RATE + 60.0 / 90.0 * RATE).round() as i64);
    // The ramp, 90 -> 150 over 8 beats, against numeric integration.
    let ramp_start = p.ticks_to_seconds(12 * PPQ);
    let mut t = ramp_start;
    let n = 80_000;
    for i in 0..n {
        let q = (i as f64 + 0.5) / n as f64 * 8.0;
        t += 8.0 / n as f64 * 60.0 / (90.0 + 7.5 * q);
    }
    assert!((p.ticks_to_seconds(20 * PPQ) - t).abs() < 1e-7);
    assert!((p.bpm_at(16 * PPQ) - 120.0).abs() < 1e-9);
    for tick in (-BAR..12 * BAR).step_by(97) {
        let back = p.frames_to_ticks(p.ticks_to_frames(tick, RATE), RATE);
        assert!((back - tick).abs() <= 1, "{tick} -> {back}");
    }
}

fn asset(d: &mut Document) -> AssetId {
    let asset = Asset {
        id: AssetId(0),
        relative_path: None,
        absolute_path: "/nonexistent/loop.wav".into(),
        sample_rate: 44_100.0,
        channels: 2,
        frames: 44_100 * 20,
    };
    d.execute(Command::AddAsset { asset }, Origin::User).unwrap();
    d.project().assets[0].id
}

fn add_clip(d: &mut Document, a: AssetId, start: Ticks, warp: ClipWarp) -> ClipId {
    let track = d.project().tracks[0].id;
    let clip = Clip {
        id: ClipId(0),
        asset: a,
        name: "loop".into(),
        start,
        source_offset: 44_100,
        source_length: 44_100 * 16,
        gain: 1.0,
        warp,
        disabled: false,
        clip_loop: None,
        fades: Default::default(),
        takes: Vec::new(),
    };
    d.execute(Command::AddClip { track, clip }, Origin::User).unwrap();
    d.project().tracks[0].clips.last().unwrap().id
}

fn snap(d: &Document) -> snapshot::SessionSnapshot {
    let a = d.project().assets.first().map(|a| a.id);
    let assets: HashMap<AssetId, u64> = a.into_iter().map(|a| (a, 7)).collect();
    let rev: HashMap<AssetId, u64> = a.into_iter().map(|a| (a, 8)).collect();
    let (s, skipped) = snapshot::build_full(d.project(), RATE, &assets, &rev, &HashMap::new());
    assert!(skipped.is_empty());
    s
}

/// Timeline frame at which a piecewise-linear warp map plays engine source frame `s`.
fn frame_of_source(points: &[(i64, f64)], s: f64) -> f64 {
    let i = points.partition_point(|p| p.1 <= s).clamp(1, points.len() - 1);
    let (a, b) = (points[i - 1], points[i]);
    a.0 as f64 + (s - a.1) / (b.1 - a.1) * (b.0 - a.0) as f64
}

#[test]
fn warped_beats_land_on_the_map_in_every_mode_and_reversed() {
    let ms = RATE / 1000.0;
    for mode in WarpMode::ALL {
        for reverse in [false, true] {
            let mut d = with_map(song_map());
            let a = asset(&mut d);
            // 16 s of source at 100 BPM: 26.67 beats from bar 2, across the step and the ramp.
            let warp = ClipWarp { enabled: true, mode, original_bpm: 100.0, reverse, ..ClipWarp::default() };
            let c = add_clip(&mut d, a, BAR, warp);
            let p = d.project();
            let clip = p.clip(c).unwrap().clone();
            let s = snap(&d);
            let cs = &s.tracks[0].clips[0];
            let points = &cs.warp.as_ref().unwrap().points;
            let ratio = RATE / 44_100.0;
            let first = clip.warp.beat_at(clip.source_offset as f64, 44_100.0);
            let beats = p.warped_clip_beats(&clip);
            assert_eq!(cs.timeline_start, p.ticks_to_frames(BAR, RATE));
            // Every sixteenth of the clip (beats and the points between them).
            let mut x = 0.25;
            while x < beats - 0.25 {
                let tick = BAR as f64 + x * PPQ as f64;
                let want = p.tempo_map().frame_at(tick, RATE);
                let source = if reverse {
                    44_100.0 * 20.0 * ratio - clip.warp.source_at(first + (beats - x), 44_100.0) * ratio
                } else {
                    clip.warp.source_at(first + x, 44_100.0) * ratio
                };
                let got = frame_of_source(points, source);
                assert!((got - want).abs() < ms, "{mode:?} reverse {reverse}: beat {x}: {got} vs {want}");
                x += 0.25;
            }
        }
    }
}

#[test]
fn unwarped_clips_play_at_their_own_speed() {
    let mut plain = Document::new(Project::default());
    let a = asset(&mut plain);
    let c = add_clip(&mut plain, a, 6 * BAR, ClipWarp::default());
    let mut d = with_map(song_map());
    asset(&mut d);
    add_clip(&mut d, a, 6 * BAR, ClipWarp::default());
    let (p, q) = (&snap(&plain).tracks[0].clips[0], &snap(&d).tracks[0].clips[0]);
    assert_eq!((p.length, p.source_offset), (q.length, q.source_offset), "same frames at any tempo");
    assert_eq!(q.timeline_start, d.project().ticks_to_frames(6 * BAR, RATE), "starts on its tick");
    // Its length in ticks is what the map gives its 16 s from its start.
    let pr = d.project();
    let len = pr.clip_length_ticks(pr.clip(c).unwrap());
    assert_eq!(len, pr.seconds_to_ticks(pr.ticks_to_seconds(6 * BAR) + 16.0) - 6 * BAR);
    // Source position halfway through the clip is half its real time.
    let half = pr.clip_ticks_at(pr.clip(c).unwrap(), 44_100.0 + 8.0 * 44_100.0);
    assert!((pr.clip_source_at(pr.clip(c).unwrap(), half) - 9.0 * 44_100.0).abs() < 1e-3);
}

#[test]
fn notes_automation_loop_and_fades_land_on_the_map() {
    let mut d = with_map(song_map());
    let inst = d.project().tracks[1].id;
    d.execute(Command::AddMidiClip { track: inst, clip: MidiClip::empty(2 * BAR, 4 * BAR) }, Origin::User).unwrap();
    let mc = d.project().tracks[1].midi_clips[0].id;
    let notes = vec![
        Note { id: 1, pitch: 60, start: 0, length: PPQ, velocity: 100 },
        Note { id: 2, pitch: 62, start: 3 * BAR, length: PPQ, velocity: 100 },
    ];
    d.execute(Command::SetMidiNotes { clip: mc, notes }, Origin::User).unwrap();
    let mut lane = AutomationLane::new(AutomationTarget::Volume);
    lane.points = vec![AutoPoint { time: 4 * BAR + PPQ, value: -6.0, curve: 0.0 }];
    d.execute(Command::SetAutomationLane { track: Some(inst), target: AutomationTarget::Volume, lane: Some(lane) }, Origin::User).unwrap();
    let a = asset(&mut d);
    let c = add_clip(&mut d, a, 3 * BAR, ClipWarp { enabled: true, original_bpm: 100.0, ..ClipWarp::default() });
    d.execute(Command::SetClipFades { clip: c, fades: ClipFades { fade_in: PPQ, fade_out: 0 } }, Origin::User).unwrap();
    let p = d.project();
    let f = |t: Ticks| p.ticks_to_frames(t, RATE);
    let s = snap(&d);
    let n = &s.tracks[1].notes;
    assert_eq!((n[0].start, n[0].end), (f(2 * BAR), f(2 * BAR + PPQ)));
    assert_eq!((n[1].start, n[1].end), (f(5 * BAR), f(5 * BAR + PPQ)), "a note inside the ramp");
    assert_eq!(s.tracks[1].automation[0].points[0].0, f(4 * BAR + PPQ));
    assert_eq!(s.tracks[0].clips[0].fades.0, f(3 * BAR + PPQ) - f(3 * BAR));
    // The loop region converts the same way (the app sends these frames to the engine).
    assert_eq!(f(3 * BAR), (4.0 * RATE + 4.0 * 60.0 / 90.0 * RATE).round() as i64);
    // The engine's segments: the formula's inputs.
    assert_eq!(s.tempo.len(), 4);
    assert_eq!((s.tempo[2].start_quarter, s.tempo[2].start_bpm, s.tempo[2].end_bpm), (12.0, 90.0, 150.0));
    assert_eq!(s.tempo[3].start_frame, p.ticks_to_seconds(20 * PPQ) * RATE);
    assert_eq!(s.click_quarters, 1.0);
}

#[test]
fn a_song_without_changes_converts_exactly_as_before() {
    let mut d = Document::new(Project::default());
    d.execute(Command::SetTempo { bpm: 97.3, at: None }, Origin::User).unwrap();
    let a = asset(&mut d);
    add_clip(&mut d, a, 3 * BAR + 17, ClipWarp { enabled: true, original_bpm: 100.0, ..ClipWarp::default() });
    add_clip(&mut d, a, 7 * BAR + 5, ClipWarp { transpose: 3, ..ClipWarp::default() });
    let s = snap(&d);
    assert!(s.tempo.is_empty());
    // The single-tempo arithmetic, written out.
    let old = |t: Ticks| (t as f64 / PPQ as f64 * 60.0 / 97.3 * RATE).round() as i64;
    assert_eq!(s.tracks[0].clips[0].timeline_start, old(3 * BAR + 17));
    let w = s.tracks[0].clips[0].warp.as_ref().unwrap();
    let beats = d.project().warped_clip_beats(d.project().clip(ClipId(s.tracks[0].clips[0].clip_id)).unwrap());
    assert_eq!(w.points.last().unwrap().0, old(3 * BAR + 17) + (beats * 60.0 / 97.3 * RATE).round() as i64);
    let v = &s.tracks[0].clips[1];
    let len_ticks = (16.0 / 2f64.powf(0.25) * 97.3 / 60.0 * PPQ as f64).round() as Ticks;
    assert_eq!(v.length, old(7 * BAR + 5 + len_ticks) - old(7 * BAR + 5));
}
