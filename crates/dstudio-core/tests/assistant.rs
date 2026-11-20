use dstudio_core::assistant::*;
use dstudio_core::*;

fn doc() -> Document {
    Document::new(Project::default())
}

#[test]
fn assist_mode_executes_validated_edits_as_one_undo_step() {
    let mut d = doc();
    let audio = d.project().tracks[0].id.0;
    let raw = format!(
        r#"{{"actions":[
        {{"type":"edit","command":{{"op":"add_track","index":null,"track":{{"id":0,"kind":"audio","name":"Drums","color":2,"gain":1.0,"pan":0.0,"mute":false,"solo":false,"clips":[]}}}}}},
        {{"type":"set_track_volume_db","track":{audio},"db":-6.0}},
        {{"type":"edit","command":{{"op":"set_tempo","bpm":128.0}}}},
        {{"type":"explain","text":"Added drums and turned the audio track down."}}
    ]}}"#
    );
    let turn = parse_turn(&raw).unwrap();
    let plan = plan(&d, Mode::Assist, &turn).unwrap();
    assert_eq!(plan.summary, vec!["Added an audio track, Drums.", "Set Audio volume to -6.0 dB.", "Changed tempo to 128 BPM."]);
    assert_eq!(d.project().tracks.len(), 2, "planning must not modify the document");

    commit(&mut d, &plan).unwrap();
    assert_eq!(d.project().tracks.len(), 3);
    assert_eq!(d.project().tracks[2].name, "Drums");
    assert_eq!(d.project().transport.bpm, 128.0);
    assert_eq!(d.undo_label(), Some("Assistant Edit"));
    d.undo().unwrap();
    assert_eq!(d.project().tracks.len(), 2);
    assert_eq!(d.project().transport.bpm, 120.0);
    assert_eq!(d.project().tracks[0].gain, 1.0);
}

#[test]
fn teach_mode_rejects_any_edit_but_allows_highlights() {
    let d = doc();
    let t = d.project().tracks[0].id.0;
    let ok = parse_turn(&format!(
        r#"{{"actions":[{{"type":"highlight","target":"transport.bpm","note":"Double-click to type a tempo"}},
                        {{"type":"highlight","target":"track:{t}:volume","note":"Drag to set level"}},
                        {{"type":"explain","text":"Tempo lives in the transport bar."}}]}}"#
    ))
    .unwrap();
    let p = plan(&d, Mode::Teach, &ok).unwrap();
    assert!(p.batch.is_none());
    assert_eq!(p.highlights.len(), 2);

    let bad = parse_turn(r#"{"actions":[{"type":"explain","text":"x"},{"type":"edit","command":{"op":"set_tempo","bpm":90.0}}]}"#).unwrap();
    assert_eq!(plan(&d, Mode::Teach, &bad), Err(AssistantError::EditInTeachMode { index: 1 }));
}

#[test]
fn invalid_targets_and_edits_are_rejected_before_execution() {
    let d = doc();
    let t = parse_turn(r#"{"actions":[{"type":"highlight","target":"window.self_destruct","note":""}]}"#).unwrap();
    assert!(matches!(plan(&d, Mode::Assist, &t), Err(AssistantError::UnknownTarget(_))));
    let t = parse_turn(r#"{"actions":[{"type":"highlight","target":"track:999:volume","note":""}]}"#).unwrap();
    assert!(matches!(plan(&d, Mode::Assist, &t), Err(AssistantError::UnknownTarget(_))));
    let t = parse_turn(r#"{"actions":[{"type":"edit","command":{"op":"set_tempo","bpm":90.0}},{"type":"edit","command":{"op":"set_tempo","bpm":5000.0}}]}"#).unwrap();
    assert!(matches!(plan(&d, Mode::Assist, &t), Err(AssistantError::InvalidEdit { index: 1, .. })));
    assert!(matches!(parse_turn("not json"), Err(AssistantError::Malformed(_))));
}

#[test]
fn later_edits_may_reference_tracks_created_earlier_in_the_turn() {
    let mut d = doc();
    // The model can't know the new id, so it creates with an explicit id.
    let next = d.project().next_id;
    let raw = format!(
        r#"{{"actions":[
        {{"type":"edit","command":{{"op":"add_track","index":null,"track":{{"id":{next},"kind":"audio","name":"Vox","color":3,"gain":1.0,"pan":0.0,"mute":false,"solo":false,"clips":[]}}}}}},
        {{"type":"edit","command":{{"op":"set_track_pan","track":{next},"pan":-0.25}}}}]}}"#
    );
    let p = plan(&d, Mode::Assist, &parse_turn(&raw).unwrap()).unwrap();
    assert_eq!(p.summary[1], "Panned Vox 25% left.");
    commit(&mut d, &p).unwrap();
    assert_eq!(d.project().track(TrackId(next)).unwrap().pan, -0.25);
}

#[test]
fn state_description_and_schema_are_well_formed() {
    let d = doc();
    let s = describe_state(d.project());
    assert_eq!(s["tracks"].as_array().unwrap().len(), 2);
    assert_eq!(s["tracks"][0]["volume_db"], 0.0);
    assert_eq!(s["tempo_bpm"], 120.0);
    let schema = tool_schema();
    assert_eq!(schema["properties"]["actions"]["items"]["oneOf"].as_array().unwrap().len(), 6);
    // The model sees each track's chain and the built-ins it may add, with their parameters.
    assert!(s["tracks"][1]["instrument"]["builtin"].is_string());
    let cat = builtin_catalog();
    let cat = cat.as_array().unwrap();
    assert!(cat.iter().any(|d| d["id"] == "compressor" && d["params"].as_array().is_some_and(|p| !p.is_empty())));
}

#[test]
fn the_assistant_adds_a_built_in_effect_and_sets_its_parameters_in_one_step() {
    let mut d = doc();
    let audio = d.project().tracks[0].id.0;
    let raw = format!(
        r#"{{"actions":[
        {{"type":"add_builtin_device","track":{audio},"device":"compressor","params":{{"threshold":-18.0}}}},
        {{"type":"add_builtin_device","track":{audio},"device":"no-such-device"}}
    ]}}"#
    );
    // An unknown device fails the whole turn: nothing half-applies.
    assert!(matches!(plan(&d, Mode::Assist, &parse_turn(&raw).unwrap()), Err(AssistantError::InvalidEdit { index: 1, .. })));
    let raw =
        format!(r#"{{"actions":[{{"type":"add_builtin_device","track":{audio},"device":"compressor","params":{{"threshold":-18.0}}}}]}}"#);
    let p = plan(&d, Mode::Assist, &parse_turn(&raw).unwrap()).unwrap();
    assert!(p.summary[0].starts_with("Added Compressor to Audio"), "{:?}", p.summary);
    commit(&mut d, &p).unwrap();
    let dev = &d.project().tracks[0].devices[0];
    assert_eq!(dev.params.get("threshold"), Some(&-18.0));
    // Its parameter is now in the state, and editable by id.
    let s = describe_state(d.project());
    assert_eq!(s["tracks"][0]["devices"][0]["params"]["threshold"], -18.0);
    let raw = format!(
        r#"{{"actions":[{{"type":"edit","command":{{"op":"set_device_param","device":{},"param":"threshold","value":-12.0}}}}]}}"#,
        dev.id.0
    );
    let p = plan(&d, Mode::Assist, &parse_turn(&raw).unwrap()).unwrap();
    commit(&mut d, &p).unwrap();
    assert_eq!(d.project().tracks[0].devices[0].params.get("threshold"), Some(&-12.0));
    // Point mode may not add devices.
    let raw = format!(r#"{{"actions":[{{"type":"add_builtin_device","track":{audio},"device":"compressor"}}]}}"#);
    assert!(matches!(plan(&d, Mode::Teach, &parse_turn(&raw).unwrap()), Err(AssistantError::EditInTeachMode { index: 0 })));
}

#[test]
fn the_state_lists_only_changed_device_parameters() {
    // The catalogue (every parameter and default) is sent once and cached; each turn's state
    // carries only what a device changed, so it stays small however many devices there are.
    let d = doc();
    let s = describe_state(d.project()).to_string();
    assert!(s.len() < 8_000, "{} bytes", s.len());
    assert!(!s.contains("builtin_devices"));
}
