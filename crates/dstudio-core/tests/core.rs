use dstudio_core::*;
use std::collections::HashMap;
use std::time::{Duration, Instant};

fn asset(frames: u64, rate: f64) -> Asset {
    Asset { id: AssetId(0), relative_path: None, absolute_path: "/nonexistent/test.wav".into(), sample_rate: rate, channels: 2, frames }
}

fn clip(asset: AssetId, start: Ticks, len: u64) -> Clip {
    Clip {
        id: ClipId(0),
        asset,
        name: "c".into(),
        start,
        source_offset: 0,
        source_length: len,
        gain: 1.0,
        warp: Default::default(),
        disabled: false,
        clip_loop: None,
        fades: Default::default(),
        takes: Vec::new(),
    }
}

fn doc_with_asset() -> (Document, TrackId, AssetId) {
    let mut d = Document::new(Project::default());
    let track = d.project().tracks[0].id;
    d.execute(Command::AddAsset { asset: asset(48_000 * 10, 48_000.0) }, Origin::User).unwrap();
    let a = d.project().assets[0].id;
    (d, track, a)
}

#[test]
fn default_project_matches_reference() {
    let p = Project::default();
    assert_eq!(p.tracks.len(), 2);
    assert_eq!(p.tracks[0].name, "Audio");
    assert_eq!(p.tracks[1].kind, TrackKind::Instrument);
    assert_eq!(p.transport.bpm, 120.0);
    assert_eq!(p.ticks_per_bar(), 4 * PPQ);
}

#[test]
fn add_and_delete_track_undo_redo() {
    let mut d = Document::new(Project::default());
    let t = Track::new(TrackId(0), TrackKind::Audio, "Drums", ColorIndex(2));
    d.execute(Command::AddTrack { index: None, track: t }, Origin::User).unwrap();
    assert_eq!(d.project().tracks.len(), 3);
    let id = d.project().tracks[2].id;
    assert_eq!(d.undo_label(), Some("Add Audio Track"));
    d.undo().unwrap();
    assert_eq!(d.project().tracks.len(), 2);
    d.redo().unwrap();
    assert_eq!(d.project().tracks[2].id, id, "redo must recreate the same id");

    d.execute(Command::RemoveTrack { track: d.project().tracks[0].id }, Origin::User).unwrap();
    assert_eq!(d.project().tracks[0].name, "Instrument");
    d.undo().unwrap();
    assert_eq!(d.project().tracks[0].name, "Audio", "undo restores at the original index");
}

#[test]
fn history_steps_list_undone_edits_after_the_applied_ones() {
    let (mut d, track, a) = doc_with_asset();
    d.execute(Command::AddClip { track, clip: clip(a, 0, 48_000) }, Origin::User).unwrap();
    let c = d.project().tracks[0].clips[0].id;
    d.execute(Command::MoveClip { clip: c, track, start: 4 * PPQ }, Origin::User).unwrap();
    let before: Vec<String> = d.history_steps().iter().map(|(l, _)| l.to_string()).collect();
    d.undo().unwrap();
    let after: Vec<String> = d.history_steps().iter().map(|(l, _)| l.to_string()).collect();
    assert_eq!(after, before, "an undone edit stays listed in place");
    assert_eq!(d.undo_depth(), before.len() - 1);
    let times: Vec<_> = d.history_steps().iter().map(|(_, t)| *t).collect();
    assert!(times.windows(2).all(|w| w[0] <= w[1]));
}

#[test]
fn validation_rejects_and_leaves_state_unchanged() {
    let (mut d, track, a) = doc_with_asset();
    let before = d.project().clone();
    let rev = d.revision();
    assert!(d.execute(Command::SetTrackGain { track, gain: 100.0 }, Origin::User).is_err());
    assert!(d.execute(Command::SetTrackPan { track, pan: f32::NAN }, Origin::User).is_err());
    assert!(d.execute(Command::SetTempo { bpm: 5.0 }, Origin::User).is_err());
    assert!(d.execute(Command::AddClip { track, clip: clip(a, 0, 48_000 * 11) }, Origin::User).is_err(), "longer than asset");
    assert!(d.execute(Command::AddClip { track: TrackId(999), clip: clip(a, 0, 10) }, Origin::User).is_err());
    let inst = d.project().tracks[1].id;
    d.execute(Command::AddClip { track, clip: clip(a, 0, 100) }, Origin::User).unwrap();
    let c = d.project().tracks[0].clips[0].id;
    assert!(d.execute(Command::MoveClip { clip: c, track: inst, start: 0 }, Origin::User).is_err(), "audio clip onto instrument track");
    d.undo().unwrap();
    // Ids are never recycled after undo, so only next_id may differ.
    let mut now = d.project().clone();
    now.next_id = before.next_id;
    assert_eq!(&now, &before);
    assert!(d.revision() > rev);
}

#[test]
fn clip_move_trim_undo() {
    let (mut d, track, a) = doc_with_asset();
    d.execute(Command::AddClip { track, clip: clip(a, 0, 48_000) }, Origin::User).unwrap();
    let c = d.project().tracks[0].clips[0].id;
    d.execute(Command::MoveClip { clip: c, track, start: 4 * PPQ }, Origin::User).unwrap();
    d.execute(Command::TrimClip { clip: c, start: 5 * PPQ, source_offset: 24_000, source_length: 24_000 }, Origin::User).unwrap();
    let cl = d.project().clip(c).unwrap();
    assert_eq!((cl.start, cl.source_offset, cl.source_length), (5 * PPQ, 24_000, 24_000));
    d.undo().unwrap();
    let cl = d.project().clip(c).unwrap();
    assert_eq!((cl.start, cl.source_offset, cl.source_length), (4 * PPQ, 0, 48_000));
    d.undo().unwrap();
    assert_eq!(d.project().clip(c).unwrap().start, 0);
    d.redo().unwrap();
    d.redo().unwrap();
    assert_eq!(d.project().clip(c).unwrap().source_offset, 24_000);
}

#[test]
fn held_gesture_coalesces_across_long_pauses() {
    let (mut d, track, _) = doc_with_asset();
    let t0 = Instant::now();
    d.execute_at(Command::SetTrackGain { track, gain: 0.8 }, t0).unwrap();
    d.hold_coalescing();
    // The knob is held still for 5 s, then moved again: still the same step.
    d.execute_at(Command::SetTrackGain { track, gain: 0.6 }, t0 + Duration::from_secs(5)).unwrap();
    d.execute_at(Command::SetTrackGain { track, gain: 0.4 }, t0 + Duration::from_secs(10)).unwrap();
    d.seal_coalescing();
    // Released: the next drag is a new step, even right away.
    d.execute_at(Command::SetTrackGain { track, gain: 0.2 }, t0 + Duration::from_secs(10)).unwrap();
    d.undo().unwrap();
    assert_eq!(d.project().track(track).unwrap().gain, 0.4);
    d.undo().unwrap();
    assert_eq!(d.project().track(track).unwrap().gain, 1.0, "one step for the whole held drag");
    // Another entry ends the hold.
    d.execute_at(Command::SetTrackGain { track, gain: 0.7 }, t0).unwrap();
    d.hold_coalescing();
    d.execute_at(Command::SetTrackPan { track, pan: 0.5 }, t0 + Duration::from_secs(1)).unwrap();
    d.execute_at(Command::SetTrackPan { track, pan: -0.5 }, t0 + Duration::from_secs(5)).unwrap();
    d.undo().unwrap();
    assert_eq!(d.project().track(track).unwrap().pan, 0.5);
}

#[test]
fn fader_drag_coalesces_into_one_undo_step() {
    let (mut d, track, _) = doc_with_asset();
    let t0 = Instant::now();
    for i in 1..=20 {
        d.execute_at(Command::SetTrackGain { track, gain: i as f32 * 0.05 }, t0 + Duration::from_millis(i * 16)).unwrap();
    }
    assert_eq!(d.project().track(track).unwrap().gain, 1.0);
    d.undo().unwrap();
    assert_eq!(d.project().track(track).unwrap().gain, 1.0, "back to the value before the drag (default 1.0)");
    d.redo().unwrap();
    assert_eq!(d.project().track(track).unwrap().gain, 1.0);
    // A pause longer than the window starts a new step.
    d.execute_at(Command::SetTrackGain { track, gain: 0.5 }, t0 + Duration::from_secs(5)).unwrap();
    d.execute_at(Command::SetTrackGain { track, gain: 0.25 }, t0 + Duration::from_secs(10)).unwrap();
    d.undo().unwrap();
    assert_eq!(d.project().track(track).unwrap().gain, 0.5);
}

#[test]
fn batch_is_atomic_and_redo_reuses_ids() {
    let (mut d, track, a) = doc_with_asset();
    let before = d.project().clone();
    let bad = Command::Batch {
        label: "Bad".into(),
        commands: vec![Command::AddClip { track, clip: clip(a, 0, 10) }, Command::SetTempo { bpm: 0.0 }],
    };
    assert!(d.execute(bad, Origin::Assistant).is_err());
    assert_eq!(d.project(), &before, "failed batch rolls back fully (including next_id)");

    let good = Command::Batch {
        label: "Add Two Clips".into(),
        commands: vec![Command::AddClip { track, clip: clip(a, 0, 10) }, Command::AddClip { track, clip: clip(a, PPQ, 10) }],
    };
    d.execute(good, Origin::Assistant).unwrap();
    let ids: Vec<_> = d.project().tracks[0].clips.iter().map(|c| c.id).collect();
    assert_eq!(d.undo_label(), Some("Add Two Clips"));
    d.undo().unwrap();
    assert!(d.project().tracks[0].clips.is_empty());
    d.redo().unwrap();
    let ids2: Vec<_> = d.project().tracks[0].clips.iter().map(|c| c.id).collect();
    assert_eq!(ids, ids2);
}

#[test]
fn commands_round_trip_as_json_for_ai_and_scripting() {
    let json = r#"{"op":"set_tempo","bpm":128.0}"#;
    let cmd: Command = serde_json::from_str(json).unwrap();
    assert_eq!(cmd, Command::SetTempo { bpm: 128.0 });
    let cmd = Command::SetTrackPan { track: TrackId(3), pan: -0.5 };
    let back: Command = serde_json::from_str(&serde_json::to_string(&cmd).unwrap()).unwrap();
    assert_eq!(cmd, back);
}

#[test]
fn save_and_reopen_project() {
    let dir = tempfile::tempdir().unwrap();
    let media = dir.path().join("media");
    std::fs::create_dir_all(&media).unwrap();
    let wav = media.join("kick.wav");
    std::fs::write(&wav, b"stub").unwrap();

    let mut d = Document::new(Project::default());
    let track = d.project().tracks[0].id;
    let mut a = asset(1000, 44_100.0);
    a.absolute_path = wav.to_string_lossy().into_owned();
    d.execute(Command::AddAsset { asset: a }, Origin::User).unwrap();
    let aid = d.project().assets[0].id;
    d.execute(Command::AddClip { track, clip: clip(aid, 2 * PPQ, 500) }, Origin::User).unwrap();
    d.execute(Command::SetTempo { bpm: 97.5 }, Origin::User).unwrap();

    let proj_dir = dir.path().join("Song.dsproj");
    persist::save(d.project(), &proj_dir).unwrap();
    let loaded = persist::load(&proj_dir).unwrap();
    assert!(loaded.missing_media.is_empty());
    let mut expected = d.project().clone();
    expected.assets[0].relative_path = Some("../media/kick.wav".into());
    // macOS temp dirs live behind the /var -> /private/var symlink; compare canonically.
    let canon = |s: &str| std::fs::canonicalize(s).unwrap();
    assert_eq!(canon(&loaded.project.assets[0].absolute_path), canon(&expected.assets[0].absolute_path));
    let mut loaded_cmp = loaded.project.clone();
    loaded_cmp.assets[0].absolute_path = expected.assets[0].absolute_path.clone();
    assert_eq!(loaded_cmp, expected);

    // Move project + media together: relative path wins.
    let moved = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(moved.path().join("media")).unwrap();
    std::fs::copy(&wav, moved.path().join("media/kick.wav")).unwrap();
    let moved_proj = moved.path().join("Song.dsproj");
    std::fs::create_dir_all(&moved_proj).unwrap();
    std::fs::copy(proj_dir.join("project.json"), moved_proj.join("project.json")).unwrap();
    std::fs::remove_file(&wav).unwrap();
    let l2 = persist::load(&moved_proj).unwrap();
    assert!(l2.missing_media.is_empty());
    assert!(std::path::Path::new(&l2.project.assets[0].absolute_path).starts_with(std::fs::canonicalize(moved.path()).unwrap()));
}

#[test]
fn load_rejects_newer_schema_and_foreign_files() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("project.json"),
        r#"{"format":"device-studio-project","schema_version":99,"saved_with":"x","project":{}}"#,
    )
    .unwrap();
    assert!(matches!(persist::load(dir.path()), Err(persist::PersistError::TooNew { .. })));
    std::fs::write(dir.path().join("project.json"), r#"{"format":"other","schema_version":1,"saved_with":"x","project":{}}"#).unwrap();
    assert!(matches!(persist::load(dir.path()), Err(persist::PersistError::WrongFormat(_))));
}

/// Saves the default project with one MIDI clip, lets `edit` change the file's JSON and loads it.
fn load_edited(edit: impl FnOnce(&mut serde_json::Value)) -> Result<persist::Loaded, persist::PersistError> {
    let dir = tempfile::tempdir().unwrap();
    let mut d = Document::new(Project::default());
    let track = d.project().tracks[1].id;
    let mut c = MidiClip::empty(0, 4 * PPQ);
    c.notes = (1..=4).map(|id| Note { id, pitch: 60, start: id as i64 * 10, length: PPQ, velocity: 100 }).collect();
    d.execute(Command::AddMidiClip { track, clip: c }, Origin::User).unwrap();
    persist::save(d.project(), dir.path()).unwrap();
    let file = dir.path().join(persist::PROJECT_FILE);
    let mut v: serde_json::Value = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
    edit(&mut v["project"]);
    std::fs::write(&file, serde_json::to_vec(&v).unwrap()).unwrap();
    persist::load(dir.path())
}

#[test]
fn load_repairs_values_the_commands_would_refuse() {
    let p = load_edited(|p| {
        p["transport"]["time_signature"]["denominator"] = 0.into();
        p["transport"]["bpm"] = 1e300.into();
        p["transport"]["loop_end"] = (-5).into();
        p["next_id"] = 0.into();
        let notes = &mut p["tracks"][1]["midi_clips"][0]["notes"];
        notes[0]["pitch"] = 200.into();
        notes[1]["velocity"] = 0.into();
        notes[2]["id"] = 4.into();
    })
    .unwrap()
    .project;
    assert_eq!(p.transport.time_signature, TimeSignature { numerator: 4, denominator: 4 });
    assert_eq!(p.ticks_per_bar(), 4 * PPQ);
    assert_eq!(p.transport.bpm, 999.0);
    assert!(p.transport.loop_end > p.transport.loop_start);
    // The note the engine can't play is dropped, the others are fixed, so the session plays.
    let notes = &p.tracks[1].midi_clips[0].notes;
    assert_eq!(notes.len(), 3);
    midi::validate_notes(notes).unwrap();
    assert_eq!(notes[0].velocity, 1);
    // New ids don't collide with any in use.
    let mut d = Document::new(p);
    let used: Vec<u64> = d.project().tracks.iter().map(|t| t.id.0).chain(d.project().tracks[1].midi_clips.iter().map(|c| c.id.0)).collect();
    d.execute(Command::AddTrack { index: None, track: Track::new(TrackId(0), TrackKind::Audio, "New", ColorIndex(1)) }, Origin::User)
        .unwrap();
    assert!(!used.contains(&d.project().tracks.last().unwrap().id.0));

    // Out-of-range transport values in the file: changing them can be undone.
    let p = load_edited(|p| {
        p["transport"]["time_signature"]["numerator"] = 35.into();
        p["transport"]["bpm"] = 10.0.into();
    })
    .unwrap()
    .project;
    let mut d = Document::new(p);
    let (sig, bpm) = (d.project().transport.time_signature, d.project().transport.bpm);
    d.execute(Command::SetTimeSignature { numerator: 3, denominator: 4 }, Origin::User).unwrap();
    d.execute(Command::SetTempo { bpm: 120.0 }, Origin::User).unwrap();
    d.undo().unwrap();
    d.undo().unwrap();
    assert_eq!((d.project().transport.time_signature, d.project().transport.bpm), (sig, bpm));

    // A stale next_id beyond any use is pulled back; ids that leave no room are refused.
    assert_eq!(load_edited(|p| p["next_id"] = u64::MAX.into()).unwrap().project.next_id, 5);
    assert!(matches!(load_edited(|p| p["tracks"][0]["id"] = u64::MAX.into()), Err(persist::PersistError::Damaged(_))));
}

#[test]
fn non_finite_values_never_make_a_project_unloadable() {
    let dir = tempfile::tempdir().unwrap();
    let mut p = Project::default();
    p.transport.bpm = f64::NAN;
    p.tracks[0].gain = f32::INFINITY;
    p.tracks[0].pan = f32::NAN;
    let synth = p.tracks[1].instrument_plugin.as_mut().unwrap();
    let param = synth.params.keys().next().cloned().unwrap_or_else(|| "x".into());
    synth.params.insert(param.clone(), f32::NAN);
    persist::save(&p, dir.path()).unwrap();
    let l = persist::load(dir.path()).unwrap().project;
    assert_eq!(l.transport.bpm, 120.0);
    assert_eq!((l.tracks[0].gain, l.tracks[0].pan), (0.0, 0.0));
    assert_eq!(l.tracks[1].instrument_plugin.as_ref().unwrap().params[&param], 0.0);
    assert_eq!(l.tracks[1].name, p.tracks[1].name);
}

#[test]
fn media_paths_survive_a_project_path_with_dot_dot() {
    let root = tempfile::tempdir().unwrap();
    let r = root.path();
    std::fs::create_dir_all(r.join("songs/sub")).unwrap();
    std::fs::create_dir_all(r.join("songs/media")).unwrap();
    let wav = std::fs::canonicalize(r.join("songs/media")).unwrap().join("k.wav");
    std::fs::write(&wav, b"x").unwrap();
    let mut p = Project::default();
    p.assets.push(Asset { absolute_path: wav.to_string_lossy().into(), ..asset(10, 48_000.0) });
    // Saved through a path with `..` (opened from a sibling folder), then moved with its media.
    persist::save(&p, &r.join("songs/sub/../Song.dsproj")).unwrap();
    std::fs::rename(r.join("songs"), r.join("moved")).unwrap();
    let l = persist::load(&r.join("moved/Song.dsproj")).unwrap();
    assert_eq!(l.project.assets[0].relative_path.as_deref(), Some("../media/k.wav"));
    assert!(l.missing_media.is_empty());
}

#[test]
fn snapshot_converts_musical_time_to_engine_frames() {
    let (mut d, track, a) = doc_with_asset();
    // Asset at 44.1k played by a 48k engine.
    d.execute(Command::AddAsset { asset: asset(44_100, 44_100.0) }, Origin::User).unwrap();
    let a2 = d.project().assets[1].id;
    d.execute(Command::AddClip { track, clip: Clip { source_offset: 22_050, ..clip(a2, 4 * PPQ, 22_050) } }, Origin::User).unwrap();
    d.execute(Command::AddClip { track, clip: clip(a, 0, 100) }, Origin::User).unwrap();
    let mut map = HashMap::new();
    map.insert(a2, 7u64);
    let (snap, skipped) = snapshot::build(d.project(), 48_000.0, &map);
    assert_eq!(skipped.len(), 1, "clip of unloaded asset is skipped");
    let c = &snap.tracks[0].clips[0];
    // Bar 2 at 120 BPM = 2 s = 96000 frames.
    assert_eq!(c.timeline_start, 96_000);
    assert_eq!(c.source_offset, 24_000);
    assert_eq!(c.length, 24_000);
    assert_eq!(c.engine_asset, 7);
}

#[test]
fn db_conversions() {
    assert!((db_to_gain(-6.0) - 0.501).abs() < 1e-3);
    assert!((gain_to_db(0.5) + 6.02).abs() < 1e-2);
    assert_eq!(db_to_gain(-200.0), 0.0);
}

#[test]
fn seal_coalescing_starts_a_new_undo_step() {
    let (mut d, track, _) = doc_with_asset();
    let t0 = Instant::now();
    d.execute_at(Command::SetTrackGain { track, gain: 0.5 }, t0).unwrap();
    d.seal_coalescing();
    d.execute_at(Command::SetTrackGain { track, gain: 0.25 }, t0 + Duration::from_millis(10)).unwrap();
    d.undo().unwrap();
    assert_eq!(d.project().track(track).unwrap().gain, 0.5, "second gesture is its own step");
}

#[test]
fn track_and_panel_heights_are_view_changes_not_undo_steps() {
    let mut d = Document::new(Project::default());
    let track = d.project().tracks[0].id;
    assert!(d.set_track_height(track, 0.1).is_err());
    assert!(d.set_track_height(track, f32::NAN).is_err());
    assert!(d.set_track_height(TrackId(999), 1.0).is_err());
    let rev = d.revision();
    d.set_track_height(track, 2.0).unwrap();
    assert_eq!(d.project().track(track).unwrap().height, 2.0);
    assert!(d.revision() > rev, "a track height marks the project dirty");
    assert!(!d.can_undo(), "but is not an undo step");

    let rev = d.revision();
    d.set_bottom_panel_height("devices", Some(420.0));
    assert!(d.revision() > rev, "a panel height marks the project dirty");
    assert!(!d.can_undo(), "but is not an undo step");
    d.set_bottom_panel_height("devices", None);
    assert!(d.project().bottom_panel_heights.is_empty());
}

#[test]
fn track_and_panel_heights_persist_and_default_when_absent() {
    let dir = tempfile::tempdir().unwrap();
    let mut d = Document::new(Project::default());
    let track = d.project().tracks[1].id;
    d.set_track_height(track, 0.5).unwrap();
    d.set_bottom_panel_height("mixer", Some(500.0));
    persist::save(d.project(), dir.path()).unwrap();
    let loaded = persist::load(dir.path()).unwrap().project;
    assert_eq!(loaded.track(track).unwrap().height, 0.5);
    assert_eq!(loaded.bottom_panel_heights.get("mixer"), Some(&500.0));

    let json = serde_json::to_string(&Project::default()).unwrap();
    assert!(!json.contains("\"height\"") && !json.contains("bottom_panel_heights"), "defaults are not written");
    let old: Project = serde_json::from_str(&json).unwrap();
    assert!(old.tracks.iter().all(|t| t.height == 1.0));
}

fn section(name: &str, bars: i64) -> Section {
    Section { id: SectionId(0), name: name.into(), color: 0x68a8c0, length: bars * 4 * PPQ }
}

#[test]
fn sections_tile_from_bar_one_and_undo() {
    let mut d = Document::new(Project::default());
    d.execute(Command::InsertSection { index: None, section: section("Section 1", 8) }, Origin::User).unwrap();
    d.execute(Command::InsertSection { index: None, section: section("Section 2", 4) }, Origin::User).unwrap();
    let (a, b) = (d.project().sections[0].id, d.project().sections[1].id);
    assert_eq!(d.project().section_start(1), 8 * 4 * PPQ);

    d.execute(Command::SetSectionLength { section: a, length: 2 * 4 * PPQ }, Origin::User).unwrap();
    assert_eq!(d.project().section_start(1), 2 * 4 * PPQ, "a later section moves with the end");
    d.execute(Command::MoveSection { section: b, index: 0 }, Origin::User).unwrap();
    assert_eq!(d.project().sections[0].id, b);
    d.execute(Command::RenameSection { section: a, name: " Chorus ".into() }, Origin::User).unwrap();
    assert_eq!(d.project().sections[1].name, "Chorus");
    d.execute(Command::RemoveSection { section: b }, Origin::User).unwrap();
    assert_eq!(d.undo_label(), Some("Delete Section"));

    for _ in 0..4 {
        d.undo().unwrap();
    }
    let p = d.project();
    assert_eq!((p.sections[0].id, p.sections[1].id), (a, b));
    assert_eq!((p.sections[0].length, p.sections[0].name.as_str()), (8 * 4 * PPQ, "Section 1"));
    for _ in 0..4 {
        d.redo().unwrap();
    }
    assert_eq!(d.project().sections.len(), 1);
    assert_eq!(d.project().sections[0].id, a, "redo keeps ids");
}

#[test]
fn section_validation_and_clear_all_batch() {
    let mut d = Document::new(Project::default());
    assert!(d.execute(Command::InsertSection { index: None, section: section(" ", 8) }, Origin::User).is_err());
    assert!(
        d.execute(Command::InsertSection { index: None, section: Section { length: PPQ - 1, ..section("A", 1) } }, Origin::User).is_err()
    );
    assert!(d.execute(Command::RemoveSection { section: SectionId(999) }, Origin::User).is_err());
    for n in ["A", "B", "C"] {
        d.execute(Command::InsertSection { index: None, section: section(n, 8) }, Origin::User).unwrap();
    }
    let a = d.project().sections[0].id;
    assert!(d.execute(Command::SetSectionLength { section: a, length: 0 }, Origin::User).is_err());
    assert!(d.execute(Command::MoveSection { section: a, index: 3 }, Origin::User).is_err());
    let commands = d.project().sections.iter().map(|s| Command::RemoveSection { section: s.id }).collect();
    d.execute(Command::Batch { label: "Clear All Sections".into(), commands }, Origin::User).unwrap();
    assert!(d.project().sections.is_empty());
    d.undo().unwrap();
    let names: Vec<_> = d.project().sections.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, ["A", "B", "C"]);
}

#[test]
fn sections_persist_and_old_projects_have_none() {
    let dir = tempfile::tempdir().unwrap();
    let mut d = Document::new(Project::default());
    d.execute(Command::InsertSection { index: None, section: section("Intro", 8) }, Origin::User).unwrap();
    persist::save(d.project(), dir.path()).unwrap();
    assert_eq!(persist::load(dir.path()).unwrap().project.sections, d.project().sections);
    let json = serde_json::to_string(&Project::default()).unwrap();
    assert!(!json.contains("sections"));
    assert!(serde_json::from_str::<Project>(&json).unwrap().sections.is_empty());
}

#[test]
fn arm_monitor_input_commands() {
    let mut d = Document::new(Project::default());
    let (audio, inst) = (d.project().tracks[0].id, d.project().tracks[1].id);
    let input = InputSource { channel: 0, stereo: false };
    d.execute(Command::SetTrackInput { track: audio, input: Some(input) }, Origin::User).unwrap();
    d.execute(Command::SetTrackArm { track: audio, arm: true }, Origin::User).unwrap();
    d.execute(Command::SetTrackMonitor { track: audio, monitor: true }, Origin::User).unwrap();
    // Instrument tracks arm and monitor for MIDI input, but take no audio input.
    d.execute(Command::SetTrackArm { track: inst, arm: true }, Origin::User).unwrap();
    assert_eq!(d.undo_label(), Some("Arm Track"));
    d.execute(Command::SetTrackMonitor { track: inst, monitor: true }, Origin::User).unwrap();
    assert!(d.execute(Command::SetTrackInput { track: inst, input: Some(input) }, Origin::User).is_err());
    let (snap, _) = snapshot::build(d.project(), 48_000.0, &HashMap::new());
    assert!(snap.tracks[0].arm && snap.tracks[0].monitor && snap.tracks[0].input == Some(input));
    assert!(snap.tracks[1].arm && snap.tracks[1].monitor && snap.tracks[1].input.is_none());
    assert_eq!(d.undo_label(), Some("Input Monitoring Change"));
    for _ in 0..5 {
        d.undo().unwrap();
    }
    let t = d.project().track(audio).unwrap();
    assert!(!t.arm && !t.monitor && t.input.is_none());
    let t = d.project().track(inst).unwrap();
    assert!(!t.arm && !t.monitor);
    d.redo().unwrap();
    d.redo().unwrap();
    d.redo().unwrap();
    d.redo().unwrap();
    assert!(d.project().track(inst).unwrap().arm);
}

#[test]
fn import_track_gives_fresh_ids_and_keeps_references() {
    let mut src = Project::default();
    let t = src.tracks[0].id;
    let a = AssetId(500);
    src.assets.push(Asset {
        id: a,
        relative_path: Some("media/x.wav".into()),
        absolute_path: "/media/x.wav".into(),
        sample_rate: 48_000.0,
        channels: 2,
        frames: 48_000,
    });
    let mut c = clip(a, 0, 48_000);
    c.id = ClipId(501);
    src.tracks[0].clips.push(c);
    let lfo = Device {
        id: DeviceId(502),
        links: vec![ModLink { slot: 0, target: DeviceId(503), param: ModParam::Builtin("gain".into()), min: 0.0, max: 1.0 }],
        ..Device::new(PluginRef {
            format: PluginFormat::Builtin,
            id: "lfo".into(),
            name: "LFO".into(),
            path: None,
            vendor: String::new(),
            version: String::new(),
            kind: PluginKind::Effect,
        })
    };
    let gain = Device {
        id: DeviceId(503),
        chain: Some(DeviceId(502)),
        sidechain: Some(TrackId(9)),
        ..Device::new(PluginRef {
            format: PluginFormat::Builtin,
            id: "gain".into(),
            name: "Gain".into(),
            path: None,
            vendor: String::new(),
            version: String::new(),
            kind: PluginKind::Effect,
        })
    };
    src.tracks[0].devices = vec![lfo, gain];
    src.tracks[0].automation.push(AutomationLane::new(AutomationTarget::Builtin {
        device: DeviceId(503),
        device_type: "gain".into(),
        param: "gain".into(),
    }));
    src.tracks[0].solo = true;

    let mut d = Document::new(Project::default());
    let mut next = d.project().next_id;
    let (track, assets) = dstudio_core::import::import_track(&src, t, &mut next).unwrap();
    assert!(dstudio_core::import::import_track(&src, TrackId(12345), &mut next.clone()).is_none());
    assert_eq!(assets.len(), 1);
    assert_eq!(assets[0].relative_path, None);
    assert_eq!(assets[0].absolute_path, "/media/x.wav");
    assert_eq!(track.clips[0].asset, assets[0].id);
    let (lfo, gain) = (&track.devices[0], &track.devices[1]);
    assert_eq!(lfo.links[0].target, gain.id);
    assert_eq!(gain.chain, Some(lfo.id));
    assert_eq!(gain.sidechain, None);
    assert!(matches!(&track.automation[0].target, AutomationTarget::Builtin { device, .. } if *device == gain.id));
    assert!(!track.solo);
    let used = [track.id.0, track.clips[0].id.0, lfo.id.0, gain.id.0, assets[0].id.0];
    assert!(used.iter().all(|&id| id >= d.project().next_id && id < next), "{used:?}");
    let id = track.id;
    let mut commands: Vec<Command> = assets.into_iter().map(|asset| Command::AddAsset { asset }).collect();
    commands.push(Command::AddTrack { index: None, track });
    d.execute(Command::Batch { label: "Add Track".into(), commands }, Origin::User).unwrap();
    assert_eq!(d.project().tracks.last().map(|t| t.id), Some(id));
    assert!(d.project().next_id >= next, "next_id {} < {next}", d.project().next_id);
}

#[test]
fn rejected_add_track_leaves_next_id_alone() {
    let mut p = Project::default();
    let before = p.next_id;
    let mut track = Track::new(TrackId(before + 50), TrackKind::Instrument, "X", p.tracks[0].color);
    track.midi_clips.push(MidiClip { id: ClipId(before + 100), ..MidiClip::empty(0, PPQ) });
    assert!(Command::AddTrack { index: Some(99), track: track.clone() }.apply(&mut p).is_err());
    assert_eq!(p.next_id, before);
    Command::AddTrack { index: None, track }.apply(&mut p).unwrap();
    // Past the MIDI clip; the built-in Synth an instrument track gets can take ids after it.
    assert!(p.next_id > before + 100, "next_id {}", p.next_id);
}

#[test]
fn a_failed_undo_keeps_its_history_entry() {
    let mut p = Project::default();
    p.tracks[0].gain = -1.0;
    let track = p.tracks[0].id;
    let mut d = Document::new(p);
    d.execute(Command::SetTrackGain { track, gain: 1.0 }, Origin::User).unwrap();
    assert!(d.undo().is_err(), "an out-of-range old gain cannot be restored");
    assert_eq!((d.undo_depth(), d.undo_label(), d.can_redo()), (1, Some("Track Volume Change"), false));
    assert_eq!(d.project().tracks[0].gain, 1.0);
}

#[test]
fn undoing_a_rename_restores_the_old_name_exactly() {
    let mut p = Project::default();
    let long = "x".repeat(200);
    p.tracks[0].name = long.clone();
    let track = p.tracks[0].id;
    let mut d = Document::new(p);
    d.execute(Command::RenameTrack { track, name: "  Lead  ".into() }, Origin::User).unwrap();
    assert_eq!(d.project().tracks[0].name, "Lead");
    d.undo().unwrap();
    assert_eq!(d.project().tracks[0].name, long, "an imported name over the limit comes back as it was");
    d.redo().unwrap();
    assert_eq!(d.project().tracks[0].name, "Lead");
    assert!(d.execute(Command::RenameTrack { track, name: "   ".into() }, Origin::User).is_err());
    let json = r#"{"op":"restore_track_name","track":1,"name":""}"#;
    assert!(serde_json::from_str::<Command>(json).is_err(), "a restore is never taken from outside");
}
