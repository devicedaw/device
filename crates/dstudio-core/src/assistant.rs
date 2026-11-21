//! AI assistant command layer (provider-agnostic, offline-safe).
//!
//! The language model never touches the project or the engine directly. It
//! returns an [`AssistantTurn`] (JSON matching [`tool_schema`]); [`plan`]
//! validates it against the current mode and a dry run on a copy of the
//! project; [`commit`] executes the result as ONE undo step through the same
//! [`Document`] the UI uses.
//!
//! * **Assist** mode: edits allowed (validated), plus highlights/explanations.
//! * **Teach** mode: no edits at all; only highlights of real UI targets and
//!   explanations. The UI calls this "Point".
//!
//! Network access, model choice and prompting live outside this crate, so
//! the DAW works fully offline.

use crate::command::{Command, EditError};
use crate::history::{Document, Origin, assign_ids};
use crate::model::*;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    Assist,
    Teach,
}

/// One action proposed by the model.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[allow(clippy::large_enum_variant, reason = "a handful per assistant turn; `Edit` holds a `Command`, see its note")]
pub enum AssistantAction {
    /// Any validated edit command (see `Command`).
    Edit { command: Command },
    /// Convenience for models: volume in decibels instead of linear gain.
    SetTrackVolumeDb { track: TrackId, db: f32 },
    /// Add a built-in device by its id (`builtin_devices[].id` in the state) with optional
    /// parameter values (plain values in each parameter's unit). Effects go into the track's
    /// chain at `index` (the end when absent); instruments replace the track's instrument.
    AddBuiltinDevice {
        track: TrackId,
        device: String,
        #[serde(default)]
        index: Option<usize>,
        #[serde(default)]
        params: std::collections::BTreeMap<String, f32>,
    },
    /// Put one of the project's audio files (`assets[].id`) into a sample slot of a built-in
    /// device: slot 0 of the Sampler, or a pad of the Drum Rack (slot = pad index).
    LoadSample { device: crate::device::DeviceId, slot: u32, asset: AssetId },
    /// Point at a UI element (stable target id from [`UI_TARGETS`], or a
    /// per-track target like `track:<id>:volume`).
    Highlight { target: String, note: String },
    /// Plain-language explanation shown in the chat.
    Explain { text: String },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AssistantTurn {
    pub actions: Vec<AssistantAction>,
}

#[derive(Debug, Error, PartialEq)]
pub enum AssistantError {
    #[error("Point mode cannot modify the project (action {index})")]
    EditInTeachMode { index: usize },
    #[error("unknown UI target {0:?}")]
    UnknownTarget(String),
    #[error("action {index} is invalid: {error}")]
    InvalidEdit { index: usize, error: EditError },
    #[error("malformed assistant output: {0}")]
    Malformed(String),
}

/// Validated, ready-to-commit result of a turn.
#[derive(Debug, Clone, PartialEq)]
pub struct Plan {
    /// All edits as one atomic batch (None when the turn has no edits).
    pub batch: Option<Command>,
    pub highlights: Vec<(String, String)>,
    pub explanations: Vec<String>,
    /// Plain sentences describing each edit, as in the action log
    /// ("Added an audio track, Drums.").
    pub summary: Vec<String>,
}

/// Stable ids for highlightable UI elements.
pub const UI_TARGETS: &[&str] = &[
    "header.sounds_toggle",
    "transport.rewind",
    "transport.play",
    "transport.record",
    "transport.loop",
    "transport.automation_arm",
    "transport.bar",
    "transport.time",
    "transport.bpm",
    "transport.tap",
    "transport.key",
    "transport.time_signature",
    "transport.metronome",
    "header.master_volume",
    "header.assistant_toggle",
    "header.mixer_toggle",
    "header.editor_toggle",
    "header.musical_typing",
    "header.settings",
    "browser.search",
    "browser.library",
    "browser.devices",
    "browser.files",
    "timeline.returns",
    "timeline.master_toggle",
    "timeline.overview",
    "timeline.smart_fill",
    "timeline.automation_toggle",
    "timeline.draw_mode",
    "timeline.tool",
    "timeline.vertical_zoom",
    "timeline.horizontal_zoom",
    "timeline.snap",
    "timeline.add_track",
    "timeline.ruler",
    "inspector.input",
    "inspector.output",
    "inspector.trim",
    "inspector.width",
    "inspector.pan",
    "inspector.mono",
    "inspector.fader",
    "devices.add_device",
];

const TRACK_TARGET_PARTS: &[&str] = &["header", "mute", "solo", "arm", "pan", "volume", "lane"];

fn valid_target(p: &Project, t: &str) -> bool {
    if UI_TARGETS.contains(&t) {
        return true;
    }
    // track:<id>:<part>
    let mut it = t.split(':');
    match (it.next(), it.next().and_then(|s| s.parse::<u64>().ok()), it.next(), it.next()) {
        (Some("track"), Some(id), Some(part), None) => p.track(TrackId(id)).is_some() && TRACK_TARGET_PARTS.contains(&part),
        _ => false,
    }
}

/// Parse raw model output (JSON) into a turn.
pub fn parse_turn(json_text: &str) -> Result<AssistantTurn, AssistantError> {
    let mut v: Value = serde_json::from_str(json_text).map_err(|e| AssistantError::Malformed(e.to_string()))?;
    lenient_colors(&mut v);
    number_new_notes(&mut v);
    serde_json::from_value(v).map_err(|e| AssistantError::Malformed(e.to_string()))
}

/// Colours are numbers (a track's palette index, a section's 0xRRGGBB), but models sometimes send
/// a CSS colour ("#3a7bd5") or a name. A `color` that isn't a number becomes 0 rather than failing
/// the whole turn: the colour is cosmetic, and the user can change it in one click.
fn lenient_colors(v: &mut Value) {
    match v {
        Value::Object(map) => {
            for (k, child) in map.iter_mut() {
                if k == "color" && !child.is_number() {
                    *child = json!(0);
                } else {
                    lenient_colors(child);
                }
            }
        }
        Value::Array(items) => items.iter_mut().for_each(lenient_colors),
        _ => {}
    }
}

/// Note ids only need to be unique within a clip, and models often send 0 (or nothing) for every
/// note of a new bassline. In any `notes` list, a note whose id is 0 or missing gets the next id
/// after the largest one in that list, so such a reply is accepted instead of failing as
/// "duplicate note id 0". Non-zero ids are kept as sent.
fn number_new_notes(v: &mut Value) {
    match v {
        Value::Object(map) => {
            for (k, child) in map.iter_mut() {
                if k == "notes"
                    && let Value::Array(notes) = child
                {
                    let id_of = |n: &Value| n.get("id").and_then(Value::as_u64).unwrap_or(0);
                    let mut next = notes.iter().map(id_of).max().unwrap_or(0) + 1;
                    for n in notes.iter_mut() {
                        if let Value::Object(note) = n
                            && note.get("id").and_then(Value::as_u64).unwrap_or(0) == 0
                        {
                            note.insert("id".into(), json!(next));
                            next += 1;
                        }
                    }
                } else {
                    number_new_notes(child);
                }
            }
        }
        Value::Array(items) => items.iter_mut().for_each(number_new_notes),
        _ => {}
    }
}

/// Validate a turn for `mode` against the document without changing it.
pub fn plan(doc: &Document, mode: Mode, turn: &AssistantTurn) -> Result<Plan, AssistantError> {
    let mut highlights = Vec::new();
    let mut explanations = Vec::new();
    let mut edits = Vec::new();
    for (index, action) in turn.actions.iter().enumerate() {
        match action {
            // Checked after the dry run: a turn may point at a track it creates.
            AssistantAction::Highlight { target, note } => highlights.push((target.clone(), note.clone())),
            AssistantAction::Explain { text } => explanations.push(text.clone()),
            AssistantAction::Edit { .. }
            | AssistantAction::SetTrackVolumeDb { .. }
            | AssistantAction::AddBuiltinDevice { .. }
            | AssistantAction::LoadSample { .. }
                if mode == Mode::Teach =>
            {
                return Err(AssistantError::EditInTeachMode { index });
            }
            AssistantAction::AddBuiltinDevice { track, device, index: at, params } => {
                edits
                    .push((index, add_builtin(*track, device, *at, params).map_err(|error| AssistantError::InvalidEdit { index, error })?));
            }
            AssistantAction::LoadSample { device, slot, asset } => {
                let name = doc
                    .project()
                    .asset(*asset)
                    .and_then(|a| std::path::Path::new(&a.absolute_path).file_stem().map(|s| s.to_string_lossy().into_owned()))
                    .unwrap_or_else(|| format!("Sample {}", asset.0));
                let sample = crate::device::DeviceSample { slot: *slot, asset: *asset, name, slices: Vec::new() };
                edits.push((index, Command::SetDeviceSample { device: *device, slot: *slot, sample: Some(sample) }));
            }
            AssistantAction::Edit { command } => edits.push((index, command.clone())),
            AssistantAction::SetTrackVolumeDb { track, db } => {
                edits.push((index, Command::SetTrackGain { track: *track, gain: db_to_gain(*db) }));
            }
        }
    }

    // Dry run on a scratch copy: assigns ids, validates every step in
    // sequence (later edits may depend on earlier ones) and builds summaries.
    let mut scratch = doc.project().clone();
    let mut summary = Vec::new();
    let mut commands = Vec::new();
    for (index, mut cmd) in edits {
        assign_ids(&mut cmd, &mut scratch);
        let before = scratch.clone();
        cmd.clone().apply(&mut scratch).map_err(|error| AssistantError::InvalidEdit { index, error })?;
        summary.push(describe(&cmd, &before, &scratch));
        commands.push(cmd);
    }
    if let Some((target, _)) = highlights.iter().find(|(target, _)| !valid_target(&scratch, target)) {
        return Err(AssistantError::UnknownTarget(target.clone()));
    }
    let batch = (!commands.is_empty()).then(|| Command::Batch { label: "Assistant Edit".into(), commands });
    Ok(Plan { batch, highlights, explanations, summary })
}

/// The edit behind [`AssistantAction::AddBuiltinDevice`].
fn add_builtin(
    track: TrackId,
    id: &str,
    index: Option<usize>,
    params: &std::collections::BTreeMap<String, f32>,
) -> Result<Command, EditError> {
    let def = crate::builtin::find(id)
        .ok_or_else(|| EditError::InvalidValue { field: "device", reason: format!("no built-in device {id:?}") })?;
    let mut device = def.instantiate();
    for (k, v) in params {
        let p = def
            .param(k)
            .ok_or_else(|| EditError::InvalidValue { field: "params", reason: format!("{} has no parameter {k:?}", def.name) })?;
        device.params.insert(k.clone(), p.snap(v.clamp(p.min.min(p.max), p.max.max(p.min))));
    }
    Ok(match def.kind {
        crate::device::PluginKind::Instrument => Command::SetInstrumentPlugin { track, device: Some(device) },
        crate::device::PluginKind::Effect => Command::AddDevice { track, index, device },
    })
}

/// A device as the model sees it: its id, what it is, on or off, and its parameter values.
fn device_json(d: &crate::device::Device) -> Value {
    match crate::builtin::def_of(d) {
        Some(def) => json!({
            "id": d.id.0,
            "builtin": def.id,
            "name": d.label.clone().unwrap_or_else(|| def.name.to_string()),
            "enabled": d.enabled,
            "preset": d.preset,
            "chain": d.chain.map(|c| c.0),
            // Only what differs from the defaults (the catalogue has every parameter and its default).
            "params": def
                .params
                .iter()
                .filter(|p| (def.value(d, p) - p.default).abs() > f32::EPSILON)
                .map(|p| (p.id.to_string(), json!(def.value(d, p))))
                .collect::<serde_json::Map<_, _>>(),
            "samples": d.samples.iter().map(|s| json!({"slot": s.slot, "asset": s.asset.0, "name": s.name})).collect::<Vec<_>>(),
        }),
        None => json!({"id": d.id.0, "plugin": d.plugin.name, "format": d.plugin.format, "enabled": d.enabled}),
    }
}

/// The built-in devices the model may add: id, name, kind, category and each parameter as
/// `[id, min, max, default, unit]`.
pub fn builtin_catalog() -> Value {
    crate::builtin::all()
        .iter()
        .map(|d| {
            json!({
                "id": d.id,
                "name": d.name,
                "kind": d.kind,
                "category": d.category,
                "params": d.params.iter().map(|p| json!([p.id, p.min, p.max, p.default, p.unit])).collect::<Vec<_>>(),
            })
        })
        .collect()
}

/// Execute a validated plan as a single undo step.
pub fn commit(doc: &mut Document, plan: &Plan) -> Result<(), EditError> {
    match &plan.batch {
        Some(batch) => doc.execute(batch.clone(), Origin::Assistant),
        None => Ok(()),
    }
}

/// Compact description of the project for the model's context window.
pub fn describe_state(p: &Project) -> Value {
    json!({
        "tempo_bpm": p.transport.bpm,
        "time_signature": format!("{}/{}", p.transport.time_signature.numerator, p.transport.time_signature.denominator),
        "ppq": PPQ,
        "note_format": "midi_clips[].notes entries are [id, pitch, start_ticks, length_ticks, velocity]",
        "master_volume_db": round1(gain_to_db(p.master_gain)),
        "assets": p.assets.iter().map(|a| json!({
            "id": a.id.0,
            "name": std::path::Path::new(&a.absolute_path).file_stem().map(|s| s.to_string_lossy().into_owned()),
            "seconds": (a.frames as f64 / a.sample_rate * 100.0).round() / 100.0,
            "frames": a.frames,
        })).collect::<Vec<_>>(),
        "tracks": p.tracks.iter().enumerate().map(|(i, t)| json!({
            "id": t.id.0,
            "number": i + 1,
            "name": t.name,
            "kind": t.kind,
            "color": t.color.0,
            "volume_db": round1(gain_to_db(t.gain)),
            "pan": t.pan,
            "mute": t.mute,
            "solo": t.solo,
            "output": t.output.map(|o| o.0),
            "sends": t.sends.iter().map(|s| json!({"target": s.target.0, "level": s.level})).collect::<Vec<_>>(),
            "instrument": t.instrument_plugin.as_ref().map(device_json),
            "devices": t.devices.iter().map(device_json).collect::<Vec<_>>(),
            "automation": t.automation.iter().map(|l| json!({"target": l.target, "enabled": l.enabled, "points": l.points.len()})).collect::<Vec<_>>(),
            "clips": t.clips.iter().map(|c| json!({
                "id": c.id.0,
                "name": c.name,
                "start_bar": 1.0 + c.start as f64 / p.ticks_per_bar() as f64,
                "length_seconds": p.asset(c.asset).map(|a| c.source_length as f64 / a.sample_rate),
            })).collect::<Vec<_>>(),
            "synth": (t.kind == TrackKind::Instrument).then(|| serde_json::to_value(&t.synth).unwrap_or(Value::Null)),
            "midi_clips": t.midi_clips.iter().map(|c| json!({
                "id": c.id.0,
                "name": c.name,
                "start_bar": 1.0 + c.start as f64 / p.ticks_per_bar() as f64,
                "start_ticks": c.start,
                "length_ticks": c.length,
                "notes": c.notes.iter().map(|n| json!([n.id, n.pitch, n.start, n.length, n.velocity])).collect::<Vec<_>>(),
            })).collect::<Vec<_>>(),
        })).collect::<Vec<_>>(),
    })
}

fn round1(v: f32) -> Value {
    if v.is_finite() { json!((v * 10.0).round() / 10.0) } else { json!("-inf") }
}

/// JSON Schema for one assistant turn, given to the model as its tool/output schema.
pub fn tool_schema() -> Value {
    let id = json!({"type": "integer", "minimum": 1});
    json!({
        "type": "object",
        "required": ["actions"],
        "properties": { "actions": { "type": "array", "items": { "oneOf": [
            {"type": "object", "required": ["type", "command"], "properties": {
                "type": {"const": "edit"},
                "command": {"type": "object", "required": ["op"], "description":
                    "Edit command. ops: add_track{index?, track{id:0,kind:audio|instrument,name,color(palette index, an integer 0..12: 0 sky, 1 periwinkle, \
                     2 violet, 3 purple, 4 pink, 5 orange, 6 teal, 7 coral, 8 amber, 9 lime, 10 green, 11 mint, 12 aqua),gain,pan,mute,solo,clips:[]}}, \
                     remove_track{track}, rename_track{track,name}, set_track_gain{track,gain(linear 0..3.98)}, \
                     set_track_pan{track,pan(-1..1)}, set_track_mute{track,mute}, set_track_solo{track,solo}, \
                     set_master_gain{gain}, move_clip{clip,track,start(ticks)}, trim_clip{clip,start,source_offset,source_length}, \
                     set_clip_gain{clip,gain}, remove_clip{clip}, set_tempo{bpm 20..999}, \
                     set_time_signature{numerator,denominator}, set_loop{enabled,start,end}, \
                     add_midi_clip{track(instrument),clip{id:0,name,start,length,notes:[]}}, remove_midi_clip{clip}, \
                     move_midi_clip{clip,track,start}, resize_midi_clip{clip,start,length}, \
                     set_midi_notes{clip,notes:[{id (0 = new note),pitch 0..127 (60=C4),start(ticks, clip-relative),length,velocity 1..127}]} (replaces all notes). \
                     set_synth_params{track(instrument),params(full SynthParams object; start from describe_state's synth and change fields)}. \
                     Devices (ids from tracks[].devices / instrument): set_device_param{device,param,value(plain value in the param's unit)}, \
                     set_device_params{device,params{param:value},preset?}, set_device_enabled{device,enabled}, remove_device{device}, \
                     move_device{device,track,index}. Sends and routing: set_sends{track,sends:[{target(return track id),level 0..1}]} \
                     (replaces all), set_track_output{track,output(return/group track id or null)}. \
                     Automation: set_automation_lane{track(or null for master),target,lane{target,enabled,points:[{time(ticks),value,curve(0)}]} or null}; \
                     targets: \"volume\" (dB -60..6), \"pan\", {\"builtin\":{device,device_type,param}} (plain value). \
                     Audio clips: add_clip{track(audio),clip{id:0,asset(assets[].id),name,start(ticks),source_offset(frames),source_length(frames),gain:1}}. \
                     Positions are ticks (PPQ 960)."}
            }},
            {"type": "object", "required": ["type", "track", "db"], "properties": {
                "type": {"const": "set_track_volume_db"}, "track": id, "db": {"type": "number", "maximum": 12}}},
            {"type": "object", "required": ["type", "track", "device"], "description":
                "Add a built-in device (builtin_devices[].id): effects go into the track's chain at index (end when absent); \
                 instruments (synth, sampler, drum rack...) replace an instrument track's instrument. params: {param_id: value}.",
             "properties": {
                "type": {"const": "add_builtin_device"}, "track": id, "device": {"type": "string"},
                "index": {"type": "integer", "minimum": 0}, "params": {"type": "object", "additionalProperties": {"type": "number"}}}},
            {"type": "object", "required": ["type", "device", "slot", "asset"], "description":
                "Load a project audio file (assets[].id) into a sample slot: the Sampler's slot 0, or a Drum Rack pad (slot = pad).",
             "properties": {"type": {"const": "load_sample"}, "device": id, "slot": {"type": "integer", "minimum": 0}, "asset": id}},
            {"type": "object", "required": ["type", "target", "note"], "properties": {
                "type": {"const": "highlight"},
                "target": {"type": "string", "description": "One of UI_TARGETS or track:<id>:<header|mute|solo|arm|pan|volume|lane>",
                           "examples": UI_TARGETS},
                "note": {"type": "string"}}},
            {"type": "object", "required": ["type", "text"], "properties": {
                "type": {"const": "explain"}, "text": {"type": "string"}}}
        ]}}}
    })
}

fn track_name(p: &Project, id: TrackId) -> String {
    p.track(id).map(|t| t.name.clone()).unwrap_or_else(|| format!("track {}", id.0))
}

/// Plain-language sentence for an applied command.
fn describe(cmd: &Command, before: &Project, after: &Project) -> String {
    match cmd {
        Command::AddTrack { track, .. } | Command::RestoreTrack { track, .. } => format!(
            "Added {} track, {}.",
            match track.kind {
                TrackKind::Audio => "an audio",
                TrackKind::Instrument => "an instrument",
                TrackKind::Return => "a return",
                TrackKind::Group => "a group",
            },
            track.name
        ),
        Command::RemoveTrack { track } => format!("Deleted {}.", track_name(before, *track)),
        Command::RenameTrack { track, name } | Command::RestoreTrackName { track, name } => {
            format!("Renamed {} to {}.", track_name(before, *track), name)
        }
        Command::SetTrackGain { track, gain } => format!("Set {} volume to {:.1} dB.", track_name(after, *track), gain_to_db(*gain)),
        Command::SetTrackPan { track, pan } => {
            let side = if pan.abs() < 0.005 {
                "centre".to_string()
            } else {
                format!("{:.0}% {}", pan.abs() * 100.0, if *pan < 0.0 { "left" } else { "right" })
            };
            format!("Panned {} {}.", track_name(after, *track), side)
        }
        Command::SetTrackTrim { track, trim_db } => format!("Set {} trim to {trim_db:+.1} dB.", track_name(after, *track)),
        Command::SetTrackWidth { track, width } => format!("Set {} width to {:.0}%.", track_name(after, *track), width * 100.0),
        Command::SetTrackMono { track, mono } => {
            format!("Turned mono {} on {}.", if *mono { "on" } else { "off" }, track_name(after, *track))
        }
        Command::MoveTrack { track, index } => format!("Moved {} to position {}.", track_name(after, *track), index + 1),
        Command::SetTrackColor { track, .. } => format!("Changed the colour of {}.", track_name(after, *track)),
        Command::SetTrackMute { track, mute } => format!("{} {}.", if *mute { "Muted" } else { "Unmuted" }, track_name(after, *track)),
        Command::SetTrackSolo { track, solo } => format!("{} {}.", if *solo { "Soloed" } else { "Unsoloed" }, track_name(after, *track)),
        Command::SetMasterGain { gain } => format!("Set master volume to {:.1} dB.", gain_to_db(*gain)),
        Command::SetMasterMute { mute } => format!("{} the master.", if *mute { "Muted" } else { "Unmuted" }),
        Command::SetTempo { bpm, .. } => format!("Changed tempo to {bpm} BPM."),
        Command::SetTimeSignature { numerator, denominator } => format!("Changed time signature to {numerator}/{denominator}."),
        Command::MoveClip { clip, track, start } => format!(
            "Moved {} to bar {:.2} on {}.",
            before.clip(*clip).map(|c| c.name.as_str()).unwrap_or("clip"),
            1.0 + *start as f64 / after.ticks_per_bar() as f64,
            track_name(after, *track)
        ),
        Command::AddDevice { track, device, .. } => format!("Added {} to {}.", device_name(device), track_name(after, *track)),
        Command::SetInstrumentPlugin { track, device: Some(device) } => {
            format!("Set the instrument of {} to {}.", track_name(after, *track), device_name(device))
        }
        Command::SetDeviceParam { device, param, value } => {
            let d = after.device(*device);
            let name = d.map_or_else(|| "a device".to_string(), device_name);
            let label = d.and_then(crate::builtin::def_of).and_then(|def| def.param(param)).map_or(param.as_str(), |p| p.name);
            format!("Set {name} {label} to {value}.")
        }
        Command::SetDeviceSample { sample: Some(s), .. } => format!("Loaded {} into a sample slot.", s.name),
        Command::SetSends { track, .. } => format!("Changed the sends of {}.", track_name(after, *track)),
        Command::SetAutomationLane { track: Some(track), lane: Some(_), .. } => {
            format!("Wrote automation on {}.", track_name(after, *track))
        }
        other => format!("{}.", other.label()),
    }
}

fn device_name(d: &crate::device::Device) -> String {
    d.label.clone().unwrap_or_else(|| crate::builtin::def_of(d).map_or_else(|| d.plugin.name.clone(), |def| def.name.to_string()))
}
