//! Structured edit commands.
//!
//! Every mutation of the project goes through [`Command::apply`], which validates
//! its arguments, mutates the model and returns the exact inverse command.
//! The same commands are used by the UI, keyboard shortcuts, scripting, tests
//! and the AI assistant (they are serde-serialisable, so an LLM tool call can
//! be deserialised straight into a `Command` and validated before execution).

use crate::automation::{AutomationLane, AutomationTarget};
use crate::device::{Device, DeviceId, DeviceSample, ModLink, PluginKind};
use crate::instrument::SynthParams;
use crate::midi::{self, MidiClip, Note};
use crate::model::*;
use crate::steps::{self, StepPattern};
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// What a `SetComposer` does (its undo label).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ComposerEdit {
    Create,
    Load,
    Original,
    Type,
    Genre,
    Shape,
}

impl ComposerEdit {
    pub fn label(self) -> &'static str {
        match self {
            ComposerEdit::Create => "Create Patterns",
            ComposerEdit::Load => "Load Pattern",
            ComposerEdit::Original => "Load Original",
            ComposerEdit::Type => "Pattern Type Change",
            ComposerEdit::Genre => "Pattern Genre Change",
            ComposerEdit::Shape => "Shape Change",
        }
    }
}

#[derive(Debug, Error, PartialEq)]
pub enum EditError {
    #[error("track {0:?} not found")]
    TrackNotFound(TrackId),
    #[error("clip {0:?} not found")]
    ClipNotFound(ClipId),
    #[error("section {0:?} not found")]
    SectionNotFound(SectionId),
    #[error("asset {0:?} not found")]
    AssetNotFound(AssetId),
    #[error("invalid value for {field}: {reason}")]
    InvalidValue { field: &'static str, reason: String },
    #[error("id {0} already in use")]
    DuplicateId(u64),
}

fn invalid(field: &'static str, reason: impl Into<String>) -> EditError {
    EditError::InvalidValue { field, reason: reason.into() }
}

/// Limits enforced by validation.
pub const MAX_GAIN: f32 = 3.981_071_7; // +12 dB
pub const MIN_BPM: f64 = 20.0;
pub const MAX_BPM: f64 = 999.0;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
#[allow(clippy::large_enum_variant, reason = "built once per edit, never on the audio path; boxing `Track` would touch every caller")]
pub enum Command {
    /// Insert a track. If `track.id` is 0 a fresh id is allocated.
    AddTrack {
        index: Option<usize>,
        track: Track,
    },
    RemoveTrack {
        track: TrackId,
    },
    /// A new name, trimmed, 1..=128 characters.
    RenameTrack {
        track: TrackId,
        name: String,
    },
    /// Move a track to `index` within its group (ordinary tracks first, then the returns).
    MoveTrack {
        track: TrackId,
        index: usize,
    },
    /// A track's colour, an index into the app's track palette.
    SetTrackColor {
        track: TrackId,
        color: ColorIndex,
    },
    /// The undo of `RenameTrack`: puts the previous name back exactly as it was, even one a
    /// rename would refuse (an imported name over 128 characters). Not accepted from outside.
    #[serde(skip_deserializing)]
    RestoreTrackName {
        track: TrackId,
        name: String,
    },
    /// Insert a section at `index` (end when `None`). If `section.id` is 0 a fresh id is allocated.
    InsertSection {
        index: Option<usize>,
        section: Section,
    },
    RemoveSection {
        section: SectionId,
    },
    RenameSection {
        section: SectionId,
        name: String,
    },
    /// Length in ticks, at least one beat. Later sections move with its end.
    SetSectionLength {
        section: SectionId,
        length: Ticks,
    },
    /// Move a section to `index` in the list (sections stay contiguous).
    MoveSection {
        section: SectionId,
        index: usize,
    },
    SetTrackGain {
        track: TrackId,
        gain: f32,
    },
    SetTrackPan {
        track: TrackId,
        pan: f32,
    },
    /// Channel-strip trim in dB ([Track::TRIM_RANGE_DB]).
    SetTrackTrim {
        track: TrackId,
        trim_db: f32,
    },
    /// Stereo width ([Track::WIDTH_RANGE], 1 = unchanged).
    SetTrackWidth {
        track: TrackId,
        width: f32,
    },
    /// Fold the track to mono.
    SetTrackMono {
        track: TrackId,
        mono: bool,
    },
    SetTrackMute {
        track: TrackId,
        mute: bool,
    },
    SetTrackSolo {
        track: TrackId,
        solo: bool,
    },
    SetMasterGain {
        gain: f32,
    },
    SetMasterMute {
        mute: bool,
    },
    /// Register an imported media asset (id 0 → allocate).
    AddAsset {
        asset: Asset,
    },
    RemoveAsset {
        asset: AssetId,
    },
    /// Insert a clip on a track (id 0 → allocate).
    AddClip {
        track: TrackId,
        clip: Clip,
    },
    RemoveClip {
        clip: ClipId,
    },
    /// Move a clip in time and/or to another track.
    MoveClip {
        clip: ClipId,
        track: TrackId,
        start: Ticks,
    },
    /// Set the visible region of a clip (left/right trim).
    TrimClip {
        clip: ClipId,
        start: Ticks,
        source_offset: u64,
        source_length: u64,
    },
    SetClipGain {
        clip: ClipId,
        gain: f32,
    },
    /// Cut an audio or MIDI clip in two at the absolute timeline position at.
    /// The left part keeps the clip's id; the right part gets new_id (0 means
    /// allocate) and the same name. MIDI notes that cross the cut are cut in
    /// two. Refused for warped or reversed audio clips and for pattern clips.
    SplitClip {
        clip: ClipId,
        at: Ticks,
        new_id: ClipId,
    },
    /// Disable or re-enable an audio or MIDI clip (it stays on the timeline but is silent).
    SetClipEnabled {
        clip: ClipId,
        enabled: bool,
    },
    /// Replace a clip's warp settings. `label` names the edit for undo
    /// ("Set clip speed", "Move warp marker", …); successive edits with the
    /// same label on the same clip coalesce (drags).
    SetClipWarp {
        clip: ClipId,
        warp: crate::warp::ClipWarp,
        label: String,
    },
    SetTempo {
        bpm: f64,
    },
    SetTimeSignature {
        numerator: u8,
        denominator: u8,
    },
    SetLoop {
        enabled: bool,
        start: Ticks,
        end: Ticks,
    },
    /// Set or clear the project key.
    SetProjectKey {
        key: Option<ProjectKey>,
    },
    /// Insert a MIDI clip on an instrument track (id 0 → allocate).
    AddMidiClip {
        track: TrackId,
        clip: MidiClip,
    },
    RemoveMidiClip {
        clip: ClipId,
    },
    /// Move a MIDI clip in time and/or to another instrument track.
    MoveMidiClip {
        clip: ClipId,
        track: TrackId,
        start: Ticks,
    },
    /// Change a MIDI clip's start and length (notes keep clip-relative times).
    ResizeMidiClip {
        clip: ClipId,
        start: Ticks,
        length: Ticks,
    },
    /// Replace all notes of a MIDI clip (draw, erase, move, resize, velocity,
    /// quantize, transpose... each editor gesture is one of these).
    SetMidiNotes {
        clip: ClipId,
        notes: Vec<Note>,
    },
    /// Make a MIDI clip a pattern clip, or change its pattern (its notes are
    /// re-baked from the grid; notes on keys that aren't rows stay), or with
    /// `None` make it a plain MIDI clip again (the repeats become notes).
    SetStepPattern {
        clip: ClipId,
        pattern: Option<StepPattern>,
    },
    /// Replace a MIDI clip's notes and pattern as they are (the exact inverse
    /// of the two commands above).
    SetMidiContent {
        clip: ClipId,
        notes: Vec<Note>,
        pattern: Option<StepPattern>,
    },
    /// The clip sidebar's pattern list: its state and the clip content it loads
    /// (`crate::library`). The inverse is the same command with the old values.
    SetComposer {
        clip: ClipId,
        edit: ComposerEdit,
        composer: Option<crate::library::Composer>,
        notes: Vec<Note>,
        pattern: Option<StepPattern>,
    },
    /// Replace an instrument track's synth parameters (knob drags coalesce
    /// per track; loading a preset is one step).
    SetSynthParams {
        track: TrackId,
        params: SynthParams,
    },
    /// A MIDI clip's loop (Loop switch, Position, Length, loop bar) and its length.
    SetMidiLoop {
        clip: ClipId,
        clip_loop: Option<midi::ClipLoop>,
        length: Ticks,
    },
    /// An audio clip's loop (Loop switch, timeline right edge while looping).
    SetClipLoop {
        clip: ClipId,
        clip_loop: Option<AudioLoop>,
    },
    /// An audio clip's fade-in and fade-out (ticks).
    SetClipFades {
        clip: ClipId,
        fades: crate::model::ClipFades,
    },
    /// Which take of a looped recording an audio clip plays (index into `Clip::takes`).
    SetClipTake {
        clip: ClipId,
        take: usize,
    },
    /// The piano roll's scale view of a MIDI clip (root, type, highlight, scale rows).
    SetClipScale {
        clip: ClipId,
        scale: Option<crate::scale::ClipScale>,
    },
    SetTrackArm {
        track: TrackId,
        arm: bool,
    },
    SetTrackMonitor {
        track: TrackId,
        monitor: bool,
    },
    SetTrackInput {
        track: TrackId,
        input: Option<InputSource>,
    },
    /// Insert an effect plugin into a track's chain (id 0 → allocate).
    AddDevice {
        track: TrackId,
        index: Option<usize>,
        device: Device,
    },
    RemoveDevice {
        device: DeviceId,
    },
    /// Reorder an insert, possibly onto another track.
    MoveDevice {
        device: DeviceId,
        track: TrackId,
        index: usize,
    },
    /// Bypass toggle.
    SetDeviceEnabled {
        device: DeviceId,
        enabled: bool,
    },
    /// Set or clear an instrument track's instrument plugin (None = built-in synth).
    SetInstrumentPlugin {
        track: TrackId,
        device: Option<Device>,
    },
    /// Set one parameter of a built-in device (plain value in the
    /// parameter's unit; knob drags coalesce per parameter).
    SetDeviceParam {
        device: DeviceId,
        param: String,
        value: f32,
    },
    /// Set several parameters of a built-in device at once (preset load, one
    /// undo step). `preset` becomes the name shown in the card header.
    SetDeviceParams {
        device: DeviceId,
        params: std::collections::BTreeMap<String, f32>,
        preset: Option<String>,
    },
    /// Load, replace or clear (`sample: None`) the audio file in one sample
    /// slot of a built-in device (sampler, pads); slice edits replace the
    /// sample with its new slices.
    SetDeviceSample {
        device: DeviceId,
        slot: u32,
        sample: Option<DeviceSample>,
    },
    /// Pick (or clear) the sidechain source track of a built-in device.
    SetDeviceSidechain {
        device: DeviceId,
        source: Option<TrackId>,
    },
    /// Add a modulation link to a source device (LFO, chain head) at `index`
    /// (None = the end). Refused past three sources per parameter.
    AddModLink {
        source: DeviceId,
        index: Option<usize>,
        link: ModLink,
    },
    RemoveModLink {
        source: DeviceId,
        index: usize,
    },
    /// Change a link's range (amount drags coalesce per link).
    SetModLink {
        source: DeviceId,
        index: usize,
        min: f32,
        max: f32,
    },
    /// Put an insert into the chain headed by `chain` (it must follow the
    /// head or its members) or take it out (None).
    SetDeviceChain {
        device: DeviceId,
        chain: Option<DeviceId>,
    },
    /// Rename a device's card (None = the plugin name).
    SetDeviceLabel {
        device: DeviceId,
        label: Option<String>,
    },
    /// Rename macro `index` of a chain head ("" = default name).
    SetMacroName {
        device: DeviceId,
        index: usize,
        name: String,
    },
    /// Card fold state; `collapsed` hides a chain head's members.
    SetDeviceView {
        device: DeviceId,
        folded: bool,
        collapsed: bool,
    },
    /// Parameters shown on a third-party plugin's card, in order.
    SetCardParams {
        device: DeviceId,
        params: Vec<u32>,
    },
    /// Swap an insert for another device in the same slot (and chain). The
    /// new device's id 0 allocates one.
    ReplaceDevice {
        device: DeviceId,
        with: Device,
    },
    /// Create, replace (points, enabled) or delete (`lane: None`) the lane for
    /// `target` on a track, or on the master when `track` is None.
    SetAutomationLane {
        track: Option<TrackId>,
        target: AutomationTarget,
        lane: Option<AutomationLane>,
    },
    /// Replace a track's sends into return tracks (spec 002). The app wraps adds and removals in
    /// a labelled `Batch` ("Add Send", "Remove Send"); level drags coalesce per track.
    SetSends {
        track: TrackId,
        sends: Vec<TrackSend>,
    },
    /// Route a track's output to a return track, or to the master (`None`).
    SetTrackOutput {
        track: TrackId,
        output: Option<TrackId>,
    },
    /// Several commands applied atomically (all or nothing).
    Batch {
        label: String,
        commands: Vec<Command>,
    },
}

impl Command {
    /// Human-readable label used for "Undo ⟨label⟩" (the Edit menu
    /// titles the action, e.g. "Undo Shortcut Change").
    pub fn label(&self) -> String {
        match self {
            Command::AddTrack { track, .. } => format!("Add {} Track", kind_name(track.kind)),
            Command::RemoveTrack { .. } => "Delete Track".into(),
            Command::RenameTrack { .. } | Command::RestoreTrackName { .. } => "Rename Track".into(),
            Command::MoveTrack { .. } => "Move Track".into(),
            Command::SetTrackColor { .. } => "Track Color Change".into(),
            Command::InsertSection { .. } => "Add Section".into(),
            Command::RemoveSection { .. } => "Delete Section".into(),
            Command::RenameSection { .. } => "Rename Section".into(),
            Command::SetSectionLength { .. } => "Resize Section".into(),
            Command::MoveSection { .. } => "Move Section".into(),
            Command::SetTrackGain { .. } => "Track Volume Change".into(),
            Command::SetTrackPan { .. } => "Track Pan Change".into(),
            Command::SetTrackTrim { .. } => "Trim Change".into(),
            Command::SetTrackWidth { .. } => "Width Change".into(),
            Command::SetTrackMono { .. } => "Mono Change".into(),
            Command::SetTrackMute { .. } => "Mute Change".into(),
            Command::SetTrackSolo { .. } => "Solo Change".into(),
            Command::SetMasterGain { .. } => "Master Volume Change".into(),
            Command::SetMasterMute { .. } => "Master Mute Change".into(),
            Command::AddAsset { .. } => "Import Audio".into(),
            Command::RemoveAsset { .. } => "Remove Audio".into(),
            Command::AddClip { .. } => "Add Clip".into(),
            Command::RemoveClip { .. } => "Delete Clip".into(),
            Command::MoveClip { .. } => "Move Clip".into(),
            Command::TrimClip { .. } => "Trim Clip".into(),
            Command::SetClipGain { .. } => "Clip Gain Change".into(),
            Command::SplitClip { .. } => "Split Clip".into(),
            Command::SetClipEnabled { enabled, .. } => (if *enabled { "Enable Clip" } else { "Disable Clip" }).into(),
            Command::SetClipWarp { label, .. } => label.clone(),
            Command::SetTempo { .. } => "Tempo Change".into(),
            Command::SetTimeSignature { .. } => "Time Signature Change".into(),
            Command::SetLoop { .. } => "Loop Change".into(),
            Command::SetProjectKey { .. } => "Key Change".into(),
            Command::AddMidiClip { .. } => "Insert MIDI Clip".into(),
            Command::RemoveMidiClip { .. } => "Delete Clip".into(),
            Command::MoveMidiClip { .. } => "Move Clip".into(),
            Command::ResizeMidiClip { .. } => "Resize Clip".into(),
            Command::SetMidiNotes { .. } | Command::SetMidiContent { .. } => "Edit Notes".into(),
            Command::SetComposer { edit, .. } => edit.label().into(),
            Command::SetStepPattern { pattern: Some(_), .. } => "Edit Step Sequencer".into(),
            Command::SetStepPattern { pattern: None, .. } => "Convert to MIDI Clip".into(),
            Command::SetSynthParams { .. } => "Synth Parameter Change".into(),
            Command::SetMidiLoop { .. } | Command::SetClipLoop { .. } => "Clip Loop".into(),
            Command::SetClipScale { .. } => "Clip Scale".into(),
            Command::SetClipFades { .. } => "Clip Fade Change".into(),
            Command::SetClipTake { .. } => "Choose Take".into(),
            Command::SetTrackArm { arm, .. } => (if *arm { "Arm Track" } else { "Disarm Track" }).into(),
            Command::SetTrackMonitor { .. } => "Input Monitoring Change".into(),
            Command::SetTrackInput { .. } => "Input Change".into(),
            Command::AddDevice { device, .. } => format!("Add {}", device.plugin.name),
            Command::RemoveDevice { .. } => "Remove Device".into(),
            Command::MoveDevice { .. } => "Move Device".into(),
            Command::SetDeviceEnabled { enabled, .. } => (if *enabled { "Enable Device" } else { "Bypass Device" }).into(),
            Command::SetDeviceParam { .. } => "Device Parameter Change".into(),
            Command::SetDeviceParams { preset: Some(name), .. } => format!("Load Preset {name}"),
            Command::SetDeviceParams { preset: None, .. } => "Device Parameters Change".into(),
            Command::SetDeviceSidechain { .. } => "Sidechain Change".into(),
            Command::SetDeviceSample { sample: None, .. } => "Remove Sample".into(),
            Command::SetDeviceSample { sample: Some(_), .. } => "Sample Change".into(),
            Command::SetInstrumentPlugin { device, .. } => match device {
                Some(d) => format!("Load {}", d.plugin.name),
                None => "Use Built-in Synth".into(),
            },
            Command::SetAutomationLane { lane, target, .. } => match lane {
                None => format!("Delete {} Automation", target.labels().1),
                Some(_) => format!("Edit {} Automation", target.labels().1),
            },
            Command::AddModLink { .. } => "Add Modulation".into(),
            Command::RemoveModLink { .. } => "Remove Modulation".into(),
            Command::SetModLink { .. } => "Modulation Amount Change".into(),
            Command::SetDeviceChain { chain: Some(_), .. } => "Move Effect into Chain".into(),
            Command::SetDeviceChain { chain: None, .. } => "Remove Effect from Chain".into(),
            Command::SetDeviceLabel { .. } => "Rename Device".into(),
            Command::SetMacroName { .. } => "Rename Macro".into(),
            Command::SetDeviceView { folded, .. } => (if *folded { "Fold Device" } else { "Unfold Device" }).into(),
            Command::SetCardParams { .. } => "Device Controls Change".into(),
            Command::ReplaceDevice { with, .. } => format!("Swap to {}", with.plugin.name),
            Command::SetSends { .. } => "Send Level Change".into(),
            Command::SetTrackOutput { .. } => "Output Change".into(),
            Command::Batch { label, .. } => label.clone(),
        }
    }

    /// Key used to coalesce rapid successive edits (e.g. a fader drag) into a
    /// single undo step. `None` means never coalesce.
    pub fn coalesce_key(&self) -> Option<String> {
        match self {
            Command::SetTrackGain { track, .. } => Some(format!("gain:{}", track.0)),
            Command::SetTrackPan { track, .. } => Some(format!("pan:{}", track.0)),
            Command::SetTrackTrim { track, .. } => Some(format!("trim:{}", track.0)),
            Command::SetTrackWidth { track, .. } => Some(format!("width:{}", track.0)),
            Command::SetSends { track, .. } => Some(format!("sends:{}", track.0)),
            Command::SetMasterGain { .. } => Some("master_gain".into()),
            Command::SetClipGain { clip, .. } => Some(format!("clipgain:{}", clip.0)),
            Command::SetClipWarp { clip, label, .. } => Some(format!("clipwarp:{}:{label}", clip.0)),
            Command::SetTempo { .. } => Some("tempo".into()),
            Command::SetSynthParams { track, params } => Some(format!("synth:{}:{}", track.0, params.preset)),
            Command::SetDeviceParam { device, param, .. } => Some(format!("devparam:{}:{param}", device.0)),
            Command::SetModLink { source, index, .. } => Some(format!("modlink:{}:{index}", source.0)),
            Command::SetComposer { clip, edit: ComposerEdit::Shape, .. } => Some(format!("shape:{}", clip.0)),
            _ => None,
        }
    }

    /// Validate and apply; returns the inverse command. On error the project is unchanged.
    pub fn apply(self, p: &mut Project) -> Result<Command, EditError> {
        match self {
            Command::AddTrack { index, mut track } => {
                if track.id.0 != 0 && p.track(track.id).is_some() {
                    return Err(EditError::DuplicateId(track.id.0));
                }
                validate_gain("gain", track.gain)?;
                validate_pan(track.pan)?;
                validate_trim(track.trim_db)?;
                validate_width(track.width)?;
                for c in &track.clips {
                    validate_clip(p, c)?;
                }
                // Ordinary tracks come first, then the returns.
                let ordinary = p.ordinary_track_count();
                if track.is_bus() {
                    if !track.clips.is_empty()
                        || !track.midi_clips.is_empty()
                        || track.instrument_plugin.is_some()
                        || track.arm
                        || track.monitor
                        || track.input.is_some()
                    {
                        return Err(invalid("track", "a return or group track has no clips, instrument or input"));
                    }
                    // Returns and groups each take one of the engine's buses.
                    if p.tracks.iter().filter(|t| t.is_bus()).count() >= MAX_RETURNS {
                        return Err(invalid("track", format!("at most {MAX_RETURNS} return and group tracks")));
                    }
                }
                let (lo, hi) = if track.is_return() { (ordinary, p.tracks.len()) } else { (0, ordinary) };
                let idx = index.unwrap_or(hi);
                if idx < lo || idx > hi {
                    return Err(invalid("index", format!("{idx} is outside {lo}..={hi}")));
                }
                validate_routing(p, track.id, track.kind, &track.sends, track.output)?;
                if track.id.0 == 0 {
                    track.id = TrackId(p.alloc_id());
                }
                let ids = track
                    .clips
                    .iter()
                    .map(|c| c.id.0)
                    .chain(track.midi_clips.iter().map(|c| c.id.0))
                    .chain(track.devices.iter().chain(track.instrument_plugin.as_ref()).map(|d| d.id.0));
                p.next_id = ids.chain([track.id.0]).fold(p.next_id, |n, id| n.max(id + 1));
                let id = track.id;
                p.tracks.insert(idx, track);
                // A new instrument track plays the built-in Synth.
                crate::builtin::synth_v2::upgrade_track(p, idx);
                Ok(Command::RemoveTrack { track: id })
            }
            Command::RemoveTrack { track } => {
                let idx = p.track_index(track).ok_or(EditError::TrackNotFound(track))?;
                // Sends to a removed return go with it, outputs routed to it fall back to the master.
                let mut restore = Vec::new();
                for t in p.tracks.iter_mut().filter(|t| t.id != track) {
                    if t.sends.iter().any(|s| s.target == track) {
                        restore.push(Command::SetSends { track: t.id, sends: t.sends.clone() });
                        t.sends.retain(|s| s.target != track);
                    }
                    if t.output == Some(track) {
                        restore.push(Command::SetTrackOutput { track: t.id, output: t.output.take() });
                    }
                }
                let t = p.tracks.remove(idx);
                let add = Command::AddTrack { index: Some(idx), track: t };
                if restore.is_empty() {
                    return Ok(add);
                }
                Ok(Command::Batch { label: "Delete Track".into(), commands: std::iter::once(add).chain(restore).collect() })
            }
            Command::InsertSection { index, mut section } => {
                let name = section.name.trim().to_string();
                if name.is_empty() || name.chars().count() > 128 {
                    return Err(invalid("name", "must be 1..=128 characters"));
                }
                if section.length < PPQ {
                    return Err(invalid("length", "at least one beat"));
                }
                if section.color > 0xff_ffff {
                    return Err(invalid("color", "0xRRGGBB"));
                }
                if section.id.0 == 0 {
                    section.id = SectionId(p.alloc_id());
                } else if p.section_index(section.id).is_some() {
                    return Err(EditError::DuplicateId(section.id.0));
                } else {
                    p.next_id = p.next_id.max(section.id.0 + 1);
                }
                section.name = name;
                let id = section.id;
                let at = index.unwrap_or(p.sections.len()).min(p.sections.len());
                p.sections.insert(at, section);
                Ok(Command::RemoveSection { section: id })
            }
            Command::RemoveSection { section } => {
                let i = p.section_index(section).ok_or(EditError::SectionNotFound(section))?;
                let s = p.sections.remove(i);
                Ok(Command::InsertSection { index: Some(i), section: s })
            }
            Command::RenameSection { section, name } => {
                let name = name.trim().to_string();
                if name.is_empty() || name.chars().count() > 128 {
                    return Err(invalid("name", "must be 1..=128 characters"));
                }
                let i = p.section_index(section).ok_or(EditError::SectionNotFound(section))?;
                let old = std::mem::replace(&mut p.sections[i].name, name);
                Ok(Command::RenameSection { section, name: old })
            }
            Command::SetSectionLength { section, length } => {
                if length < PPQ {
                    return Err(invalid("length", "at least one beat"));
                }
                let i = p.section_index(section).ok_or(EditError::SectionNotFound(section))?;
                let old = std::mem::replace(&mut p.sections[i].length, length);
                Ok(Command::SetSectionLength { section, length: old })
            }
            Command::MoveSection { section, index } => {
                let i = p.section_index(section).ok_or(EditError::SectionNotFound(section))?;
                if index >= p.sections.len() {
                    return Err(invalid("index", format!("{index} past the last section")));
                }
                let s = p.sections.remove(i);
                p.sections.insert(index, s);
                Ok(Command::MoveSection { section, index: i })
            }
            Command::RenameTrack { track, name } => {
                let name = name.trim().to_string();
                if name.is_empty() || name.chars().count() > 128 {
                    return Err(invalid("name", "must be 1..=128 characters"));
                }
                let t = p.track_mut(track).ok_or(EditError::TrackNotFound(track))?;
                let old = std::mem::replace(&mut t.name, name);
                Ok(Command::RestoreTrackName { track, name: old })
            }
            Command::RestoreTrackName { track, name } => {
                let t = p.track_mut(track).ok_or(EditError::TrackNotFound(track))?;
                let old = std::mem::replace(&mut t.name, name);
                Ok(Command::RenameTrack { track, name: old })
            }
            Command::SetTrackGain { track, gain } => {
                validate_gain("gain", gain)?;
                let t = p.track_mut(track).ok_or(EditError::TrackNotFound(track))?;
                let old = std::mem::replace(&mut t.gain, gain);
                Ok(Command::SetTrackGain { track, gain: old })
            }
            Command::SetTrackPan { track, pan } => {
                validate_pan(pan)?;
                let t = p.track_mut(track).ok_or(EditError::TrackNotFound(track))?;
                let old = std::mem::replace(&mut t.pan, pan);
                Ok(Command::SetTrackPan { track, pan: old })
            }
            Command::SetTrackTrim { track, trim_db } => {
                validate_trim(trim_db)?;
                let t = p.track_mut(track).ok_or(EditError::TrackNotFound(track))?;
                let old = std::mem::replace(&mut t.trim_db, trim_db);
                Ok(Command::SetTrackTrim { track, trim_db: old })
            }
            Command::SetTrackWidth { track, width } => {
                validate_width(width)?;
                let t = p.track_mut(track).ok_or(EditError::TrackNotFound(track))?;
                let old = std::mem::replace(&mut t.width, width);
                Ok(Command::SetTrackWidth { track, width: old })
            }
            Command::SetTrackMono { track, mono } => {
                let t = p.track_mut(track).ok_or(EditError::TrackNotFound(track))?;
                let old = std::mem::replace(&mut t.mono, mono);
                Ok(Command::SetTrackMono { track, mono: old })
            }
            Command::MoveTrack { track, index } => {
                let i = p.track_index(track).ok_or(EditError::TrackNotFound(track))?;
                let ordinary = p.ordinary_track_count();
                let (lo, hi) = if p.tracks[i].is_return() { (ordinary, p.tracks.len()) } else { (0, ordinary) };
                if index < lo || index >= hi {
                    return Err(invalid("index", format!("{index} is outside {lo}..{hi}")));
                }
                let t = p.tracks.remove(i);
                p.tracks.insert(index, t);
                Ok(Command::MoveTrack { track, index: i })
            }
            Command::SetTrackColor { track, color } => {
                let t = p.track_mut(track).ok_or(EditError::TrackNotFound(track))?;
                let old = std::mem::replace(&mut t.color, color);
                Ok(Command::SetTrackColor { track, color: old })
            }
            Command::SetTrackMute { track, mute } => {
                let t = p.track_mut(track).ok_or(EditError::TrackNotFound(track))?;
                let old = std::mem::replace(&mut t.mute, mute);
                Ok(Command::SetTrackMute { track, mute: old })
            }
            Command::SetTrackSolo { track, solo } => {
                let t = p.track_mut(track).ok_or(EditError::TrackNotFound(track))?;
                let old = std::mem::replace(&mut t.solo, solo);
                Ok(Command::SetTrackSolo { track, solo: old })
            }
            Command::SetMasterGain { gain } => {
                validate_gain("master_gain", gain)?;
                let old = std::mem::replace(&mut p.master_gain, gain);
                Ok(Command::SetMasterGain { gain: old })
            }
            Command::SetMasterMute { mute } => {
                let old = std::mem::replace(&mut p.master_mute, mute);
                Ok(Command::SetMasterMute { mute: old })
            }
            Command::AddAsset { mut asset } => {
                if asset.id.0 == 0 {
                    asset.id = AssetId(p.alloc_id());
                } else if p.asset(asset.id).is_some() {
                    return Err(EditError::DuplicateId(asset.id.0));
                } else {
                    p.next_id = p.next_id.max(asset.id.0 + 1);
                }
                if asset.sample_rate.is_nan() || asset.sample_rate <= 0.0 || asset.channels == 0 {
                    return Err(invalid("asset", "sample_rate and channels must be positive"));
                }
                let id = asset.id;
                p.assets.push(asset);
                Ok(Command::RemoveAsset { asset: id })
            }
            Command::RemoveAsset { asset } => {
                if p.tracks.iter().flat_map(|t| &t.clips).any(|c| c.asset == asset) {
                    return Err(invalid("asset", "still referenced by clips"));
                }
                if p.tracks
                    .iter()
                    .flat_map(|t| t.devices.iter().chain(t.instrument_plugin.iter()))
                    .flat_map(|d| &d.samples)
                    .any(|s| s.asset == asset)
                {
                    return Err(invalid("asset", "still played by a device"));
                }
                let idx = p.assets.iter().position(|a| a.id == asset).ok_or(EditError::AssetNotFound(asset))?;
                let a = p.assets.remove(idx);
                Ok(Command::AddAsset { asset: a })
            }
            Command::AddClip { track, mut clip } => {
                match p.track(track) {
                    None => return Err(EditError::TrackNotFound(track)),
                    Some(t) if t.is_bus() => return Err(invalid("track", "return and group tracks take no clips")),
                    Some(_) => {}
                }
                if clip.id.0 == 0 {
                    clip.id = ClipId(p.alloc_id());
                } else if p.clip_id_in_use(clip.id) {
                    return Err(EditError::DuplicateId(clip.id.0));
                } else {
                    p.next_id = p.next_id.max(clip.id.0 + 1);
                }
                validate_clip(p, &clip)?;
                let id = clip.id;
                let t = p.track_mut(track).expect("checked");
                t.clips.push(clip);
                t.clips.sort_by_key(|c| (c.start, c.id));
                Ok(Command::RemoveClip { clip: id })
            }
            Command::RemoveClip { clip } => {
                let (ti, ci) = p.find_clip(clip).ok_or(EditError::ClipNotFound(clip))?;
                let track = p.tracks[ti].id;
                let c = p.tracks[ti].clips.remove(ci);
                Ok(Command::AddClip { track, clip: c })
            }
            Command::MoveClip { clip, track, start } => {
                if start < 0 {
                    return Err(invalid("start", "must be >= 0"));
                }
                let (ti, ci) = p.find_clip(clip).ok_or(EditError::ClipNotFound(clip))?;
                let dest = p.track_index(track).ok_or(EditError::TrackNotFound(track))?;
                if p.tracks[dest].kind != p.tracks[ti].kind {
                    return Err(invalid("track", "clip cannot move between audio and instrument tracks"));
                }
                let old_track = p.tracks[ti].id;
                let mut c = p.tracks[ti].clips.remove(ci);
                let old_start = std::mem::replace(&mut c.start, start);
                p.tracks[dest].clips.push(c);
                p.tracks[dest].clips.sort_by_key(|c| (c.start, c.id));
                Ok(Command::MoveClip { clip, track: old_track, start: old_start })
            }
            Command::TrimClip { clip, start, source_offset, source_length } => {
                let (ti, ci) = p.find_clip(clip).ok_or(EditError::ClipNotFound(clip))?;
                let mut probe = p.tracks[ti].clips[ci].clone();
                probe.start = start;
                probe.source_offset = source_offset;
                probe.source_length = source_length;
                validate_clip(p, &probe)?;
                let c = &mut p.tracks[ti].clips[ci];
                let inv = Command::TrimClip { clip, start: c.start, source_offset: c.source_offset, source_length: c.source_length };
                *c = probe;
                p.tracks[ti].clips.sort_by_key(|c| (c.start, c.id));
                Ok(inv)
            }
            Command::SetClipLoop { clip, clip_loop } => {
                if let Some(l) = &clip_loop {
                    l.validate().map_err(|r| invalid("loop", r))?;
                }
                let (ti, ci) = p.find_clip(clip).ok_or(EditError::ClipNotFound(clip))?;
                let old = std::mem::replace(&mut p.tracks[ti].clips[ci].clip_loop, clip_loop);
                Ok(Command::SetClipLoop { clip, clip_loop: old })
            }
            Command::SetClipFades { clip, fades } => {
                fades.validate().map_err(|r| invalid("fades", r))?;
                let (ti, ci) = p.find_clip(clip).ok_or(EditError::ClipNotFound(clip))?;
                let old = std::mem::replace(&mut p.tracks[ti].clips[ci].fades, fades);
                Ok(Command::SetClipFades { clip, fades: old })
            }
            Command::SetClipTake { clip, take } => {
                let (ti, ci) = p.find_clip(clip).ok_or(EditError::ClipNotFound(clip))?;
                let c = &p.tracks[ti].clips[ci];
                let old = c.current_take().ok_or_else(|| invalid("take", format!("clip {} has no takes", clip.0)))?;
                let frames = p.asset(c.asset).map(|a| a.frames).ok_or(EditError::AssetNotFound(c.asset))?;
                let (offset, takes) = c.with_take(take, frames).map_err(|r| invalid("take", r))?;
                let c = &mut p.tracks[ti].clips[ci];
                c.source_offset = offset;
                c.takes = takes;
                Ok(Command::SetClipTake { clip, take: old })
            }
            Command::SetClipGain { clip, gain } => {
                validate_gain("clip_gain", gain)?;
                let (ti, ci) = p.find_clip(clip).ok_or(EditError::ClipNotFound(clip))?;
                let old = std::mem::replace(&mut p.tracks[ti].clips[ci].gain, gain);
                Ok(Command::SetClipGain { clip, gain: old })
            }
            Command::SplitClip { clip, at, new_id } => split_clip(p, clip, at, new_id),
            Command::SetClipEnabled { clip, enabled } => {
                if let Some((ti, ci)) = p.find_clip(clip) {
                    let was = !std::mem::replace(&mut p.tracks[ti].clips[ci].disabled, !enabled);
                    return Ok(Command::SetClipEnabled { clip, enabled: was });
                }
                let (ti, ci) = p.find_midi_clip(clip).ok_or(EditError::ClipNotFound(clip))?;
                let was = !std::mem::replace(&mut p.tracks[ti].midi_clips[ci].disabled, !enabled);
                Ok(Command::SetClipEnabled { clip, enabled: was })
            }
            Command::SetClipWarp { clip, warp, label } => {
                let (ti, ci) = p.find_clip(clip).ok_or(EditError::ClipNotFound(clip))?;
                let mut probe = p.tracks[ti].clips[ci].clone();
                probe.warp = warp;
                validate_clip(p, &probe)?;
                let old = std::mem::replace(&mut p.tracks[ti].clips[ci].warp, probe.warp);
                Ok(Command::SetClipWarp { clip, warp: old, label })
            }
            Command::SetTempo { bpm } => {
                if !(MIN_BPM..=MAX_BPM).contains(&bpm) {
                    return Err(invalid("bpm", format!("must be within {MIN_BPM}..={MAX_BPM}")));
                }
                let old = std::mem::replace(&mut p.transport.bpm, bpm);
                Ok(Command::SetTempo { bpm: old })
            }
            Command::SetTimeSignature { numerator, denominator } => {
                if numerator == 0 || numerator > 32 || ![1, 2, 4, 8, 16, 32].contains(&denominator) {
                    return Err(invalid("time_signature", "numerator 1..=32, denominator power of two ≤ 32"));
                }
                let ts = &mut p.transport.time_signature;
                let old = Command::SetTimeSignature { numerator: ts.numerator, denominator: ts.denominator };
                *ts = TimeSignature { numerator, denominator };
                Ok(old)
            }
            Command::SetProjectKey { key } => {
                if key.is_some_and(|k| k.root > 11) {
                    return Err(invalid("key", "root must be a pitch class 0..=11"));
                }
                let old = std::mem::replace(&mut p.transport.key, key);
                Ok(Command::SetProjectKey { key: old })
            }
            Command::SetLoop { enabled, start, end } => {
                if start < 0 || end <= start {
                    return Err(invalid("loop", "requires 0 <= start < end"));
                }
                let t = &mut p.transport;
                let old = Command::SetLoop { enabled: t.loop_enabled, start: t.loop_start, end: t.loop_end };
                t.loop_enabled = enabled;
                t.loop_start = start;
                t.loop_end = end;
                Ok(old)
            }
            Command::AddMidiClip { track, mut clip } => {
                let ti = p.track_index(track).ok_or(EditError::TrackNotFound(track))?;
                if p.tracks[ti].kind != TrackKind::Instrument {
                    return Err(invalid("track", "MIDI clips belong on instrument tracks"));
                }
                if clip.id.0 == 0 {
                    clip.id = ClipId(p.alloc_id());
                } else if p.clip_id_in_use(clip.id) {
                    return Err(EditError::DuplicateId(clip.id.0));
                } else {
                    p.next_id = p.next_id.max(clip.id.0 + 1);
                }
                validate_midi_clip(&clip)?;
                midi::sort_notes(&mut clip.notes);
                let id = clip.id;
                let t = &mut p.tracks[ti];
                t.midi_clips.push(clip);
                t.midi_clips.sort_by_key(|c| (c.start, c.id));
                Ok(Command::RemoveMidiClip { clip: id })
            }
            Command::RemoveMidiClip { clip } => {
                let (ti, ci) = p.find_midi_clip(clip).ok_or(EditError::ClipNotFound(clip))?;
                let track = p.tracks[ti].id;
                let c = p.tracks[ti].midi_clips.remove(ci);
                Ok(Command::AddMidiClip { track, clip: c })
            }
            Command::MoveMidiClip { clip, track, start } => {
                if start < 0 {
                    return Err(invalid("start", "must be >= 0"));
                }
                let (ti, ci) = p.find_midi_clip(clip).ok_or(EditError::ClipNotFound(clip))?;
                let dest = p.track_index(track).ok_or(EditError::TrackNotFound(track))?;
                if p.tracks[dest].kind != TrackKind::Instrument {
                    return Err(invalid("track", "MIDI clips belong on instrument tracks"));
                }
                let old_track = p.tracks[ti].id;
                let mut c = p.tracks[ti].midi_clips.remove(ci);
                let old_start = std::mem::replace(&mut c.start, start);
                p.tracks[dest].midi_clips.push(c);
                p.tracks[dest].midi_clips.sort_by_key(|c| (c.start, c.id));
                Ok(Command::MoveMidiClip { clip, track: old_track, start: old_start })
            }
            Command::ResizeMidiClip { clip, start, length } => {
                if start < 0 || length <= 0 {
                    return Err(invalid("clip", "requires start >= 0 and length > 0"));
                }
                let (ti, ci) = p.find_midi_clip(clip).ok_or(EditError::ClipNotFound(clip))?;
                let c = &mut p.tracks[ti].midi_clips[ci];
                let inv = Command::ResizeMidiClip { clip, start: c.start, length: c.length };
                c.start = start;
                c.length = length;
                p.tracks[ti].midi_clips.sort_by_key(|c| (c.start, c.id));
                Ok(inv)
            }
            Command::SetMidiNotes { clip, mut notes } => {
                midi::validate_notes(&notes).map_err(|r| invalid("notes", r))?;
                let (ti, ci) = p.find_midi_clip(clip).ok_or(EditError::ClipNotFound(clip))?;
                midi::sort_notes(&mut notes);
                let c = &mut p.tracks[ti].midi_clips[ci];
                let Some(pattern) = c.pattern.take() else {
                    let old = std::mem::replace(&mut c.notes, notes);
                    return Ok(Command::SetMidiNotes { clip, notes: old });
                };
                // A piano-roll edit of a pattern clip: the rows it touched are read back into the grid.
                c.pattern = Some(steps::rederive(&pattern, &c.notes, &notes));
                let old = std::mem::replace(&mut c.notes, notes);
                Ok(Command::SetMidiContent { clip, notes: old, pattern: Some(pattern) })
            }
            Command::SetStepPattern { clip, pattern } => {
                if let Some(pat) = &pattern {
                    pat.validate().map_err(|r| invalid("pattern", r))?;
                }
                let (ti, ci) = p.find_midi_clip(clip).ok_or(EditError::ClipNotFound(clip))?;
                let c = &mut p.tracks[ti].midi_clips[ci];
                if pattern.is_none() && c.pattern.is_none() {
                    return Err(invalid("pattern", "the clip is not a pattern clip"));
                }
                let notes = match &pattern {
                    Some(pat) => steps::bake_into(pat, &c.notes),
                    None => c.tiled_notes(),
                };
                let old_notes = std::mem::replace(&mut c.notes, notes);
                let old_pattern = std::mem::replace(&mut c.pattern, pattern);
                Ok(Command::SetMidiContent { clip, notes: old_notes, pattern: old_pattern })
            }
            Command::SetMidiLoop { clip, clip_loop, length } => {
                if let Some(l) = &clip_loop {
                    l.validate().map_err(|r| invalid("loop", r))?;
                }
                if length <= 0 {
                    return Err(invalid("clip", "requires length > 0"));
                }
                let (ti, ci) = p.find_midi_clip(clip).ok_or(EditError::ClipNotFound(clip))?;
                let c = &mut p.tracks[ti].midi_clips[ci];
                let inv = Command::SetMidiLoop { clip, clip_loop: c.clip_loop, length: c.length };
                c.clip_loop = clip_loop;
                c.length = length;
                Ok(inv)
            }
            Command::SetClipScale { clip, scale } => {
                if let Some(s) = &scale {
                    s.validate().map_err(|r| invalid("scale", r))?;
                }
                let (ti, ci) = p.find_midi_clip(clip).ok_or(EditError::ClipNotFound(clip))?;
                let old = std::mem::replace(&mut p.tracks[ti].midi_clips[ci].scale, scale);
                Ok(Command::SetClipScale { clip, scale: old })
            }
            Command::SetComposer { clip, edit, composer, mut notes, pattern } => {
                midi::validate_notes(&notes).map_err(|r| invalid("notes", r))?;
                if let Some(pat) = &pattern {
                    pat.validate().map_err(|r| invalid("pattern", r))?;
                }
                if let Some(c) = &composer {
                    c.validate().map_err(|r| invalid("composer", r))?;
                }
                let (ti, ci) = p.find_midi_clip(clip).ok_or(EditError::ClipNotFound(clip))?;
                midi::sort_notes(&mut notes);
                let c = &mut p.tracks[ti].midi_clips[ci];
                let old_notes = std::mem::replace(&mut c.notes, notes);
                let old_pattern = std::mem::replace(&mut c.pattern, pattern);
                let old_composer = std::mem::replace(&mut c.composer, composer);
                Ok(Command::SetComposer { clip, edit, composer: old_composer, notes: old_notes, pattern: old_pattern })
            }
            Command::SetMidiContent { clip, mut notes, pattern } => {
                midi::validate_notes(&notes).map_err(|r| invalid("notes", r))?;
                if let Some(pat) = &pattern {
                    pat.validate().map_err(|r| invalid("pattern", r))?;
                }
                let (ti, ci) = p.find_midi_clip(clip).ok_or(EditError::ClipNotFound(clip))?;
                midi::sort_notes(&mut notes);
                let c = &mut p.tracks[ti].midi_clips[ci];
                let old_notes = std::mem::replace(&mut c.notes, notes);
                let old_pattern = std::mem::replace(&mut c.pattern, pattern);
                Ok(Command::SetMidiContent { clip, notes: old_notes, pattern: old_pattern })
            }
            Command::SetSynthParams { track, params } => {
                params.validate().map_err(|r| invalid("synth", r))?;
                let t = p.track_mut(track).ok_or(EditError::TrackNotFound(track))?;
                if t.kind != TrackKind::Instrument {
                    return Err(invalid("track", "only instrument tracks have a synth"));
                }
                let old = std::mem::replace(&mut t.synth, params);
                Ok(Command::SetSynthParams { track, params: old })
            }
            Command::SetTrackArm { track, arm } => {
                let t = p.track_mut(track).ok_or(EditError::TrackNotFound(track))?;
                // Audio tracks record their input; instrument tracks record MIDI; returns record nothing.
                if arm && t.is_bus() {
                    return Err(invalid("track", "return and group tracks cannot be armed"));
                }
                let old = std::mem::replace(&mut t.arm, arm);
                Ok(Command::SetTrackArm { track, arm: old })
            }
            Command::SetTrackMonitor { track, monitor } => {
                let t = p.track_mut(track).ok_or(EditError::TrackNotFound(track))?;
                // Audio tracks hear their input; instrument tracks hear MIDI input; returns have no input.
                if monitor && t.is_bus() {
                    return Err(invalid("track", "return and group tracks have no input to monitor"));
                }
                let old = std::mem::replace(&mut t.monitor, monitor);
                Ok(Command::SetTrackMonitor { track, monitor: old })
            }
            Command::SetTrackInput { track, input } => {
                if let Some(i) = input
                    && i.stereo
                    && i.channel == u16::MAX
                {
                    return Err(invalid("input", "stereo pair exceeds channel range"));
                }
                let t = p.track_mut(track).ok_or(EditError::TrackNotFound(track))?;
                if input.is_some() && t.kind != TrackKind::Audio {
                    return Err(invalid("track", "only audio tracks take an audio input"));
                }
                let old = std::mem::replace(&mut t.input, input);
                Ok(Command::SetTrackInput { track, input: old })
            }
            Command::AddDevice { track, index, mut device } => {
                let ti = p.track_index(track).ok_or(EditError::TrackNotFound(track))?;
                if device.plugin.kind != PluginKind::Effect {
                    return Err(invalid("device", "only effect plugins go in the insert chain"));
                }
                sanitize_builtin(p, &mut device)?;
                if device.id.0 == 0 {
                    device.id = DeviceId(p.alloc_id());
                } else if p.find_device(device.id).is_some() {
                    return Err(EditError::DuplicateId(device.id.0));
                } else {
                    p.next_id = p.next_id.max(device.id.0 + 1);
                }
                let n = p.tracks[ti].devices.len();
                let idx = index.unwrap_or(n);
                if idx > n {
                    return Err(invalid("index", format!("{idx} > {n}")));
                }
                if let Some(h) = device.chain {
                    let devs = &p.tracks[ti].devices;
                    let hi = devs.iter().position(|d| d.id == h && crate::modulation::is_rack(d));
                    let run = hi.map(|hi| crate::modulation::members(devs, hi));
                    if crate::modulation::outputs(&device) > 0 || !run.is_some_and(|r| idx >= r.start && idx <= r.end) {
                        return Err(invalid("chain", "a chain member goes right after its head or another member"));
                    }
                } else {
                    let devs = &p.tracks[ti].devices;
                    if (0..n).any(|hi| crate::modulation::members(devs, hi).contains(&idx)) {
                        return Err(invalid("chain", "a device outside a chain cannot go between its members"));
                    }
                }
                let id = device.id;
                p.tracks[ti].devices.insert(idx, device);
                Ok(Command::RemoveDevice { device: id })
            }
            Command::RemoveDevice { device } => {
                let (ti, slot) = p.find_device(device).ok_or_else(|| invalid("device", format!("{} not found", device.0)))?;
                let track = p.tracks[ti].id;
                match slot {
                    DeviceSlot::Insert(i) => {
                        let d = p.tracks[ti].devices.remove(i);
                        Ok(Command::AddDevice { track, index: Some(i), device: d })
                    }
                    DeviceSlot::Instrument => {
                        let d = p.tracks[ti].instrument_plugin.take();
                        Ok(Command::SetInstrumentPlugin { track, device: d })
                    }
                }
            }
            Command::MoveDevice { device, track, index } => {
                let (ti, slot) = p.find_device(device).ok_or_else(|| invalid("device", format!("{} not found", device.0)))?;
                let DeviceSlot::Insert(from) = slot else {
                    return Err(invalid("device", "instrument plugins can't be moved into a chain"));
                };
                let dest = p.track_index(track).ok_or(EditError::TrackNotFound(track))?;
                let old_track = p.tracks[ti].id;
                let d = p.tracks[ti].devices.remove(from);
                if index > p.tracks[dest].devices.len() {
                    p.tracks[ti].devices.insert(from, d);
                    return Err(invalid("index", "out of range"));
                }
                p.tracks[dest].devices.insert(index, d);
                Ok(Command::MoveDevice { device, track: old_track, index: from })
            }
            Command::SetDeviceEnabled { device, enabled } => {
                let (ti, slot) = p.find_device(device).ok_or_else(|| invalid("device", format!("{} not found", device.0)))?;
                let d = match slot {
                    DeviceSlot::Insert(i) => &mut p.tracks[ti].devices[i],
                    DeviceSlot::Instrument => p.tracks[ti].instrument_plugin.as_mut().expect("found"),
                };
                let old = std::mem::replace(&mut d.enabled, enabled);
                Ok(Command::SetDeviceEnabled { device, enabled: old })
            }
            Command::SetInstrumentPlugin { track, mut device } => {
                let ti = p.track_index(track).ok_or(EditError::TrackNotFound(track))?;
                if p.tracks[ti].kind != TrackKind::Instrument {
                    return Err(invalid("track", "instrument plugins belong on instrument tracks"));
                }
                if let Some(d) = device.as_mut() {
                    if d.plugin.kind != PluginKind::Instrument {
                        return Err(invalid("device", "not an instrument plugin"));
                    }
                    sanitize_builtin(p, d)?;
                    if d.id.0 == 0 {
                        d.id = DeviceId(p.alloc_id());
                    } else if p.find_device(d.id).is_some_and(|(t, s)| !(t == ti && s == DeviceSlot::Instrument)) {
                        return Err(EditError::DuplicateId(d.id.0));
                    } else {
                        p.next_id = p.next_id.max(d.id.0 + 1);
                    }
                }
                let old = std::mem::replace(&mut p.tracks[ti].instrument_plugin, device);
                Ok(Command::SetInstrumentPlugin { track, device: old })
            }
            Command::SetDeviceParam { device, param, value } => {
                let (d, def) = builtin_mut(p, device)?;
                let v = crate::builtin::validate_value(def, &param, value).map_err(|r| invalid("value", r))?;
                let old = d.params.insert(param.clone(), v).unwrap_or_else(|| def.param(&param).map(|q| q.default).unwrap_or(v));
                Ok(Command::SetDeviceParam { device, param, value: old })
            }
            Command::SetDeviceParams { device, params, preset } => {
                let (d, def) = builtin_mut(p, device)?;
                let mut checked = std::collections::BTreeMap::new();
                for (k, v) in &params {
                    checked.insert(k.clone(), crate::builtin::validate_value(def, k, *v).map_err(|r| invalid("value", r))?);
                }
                let mut old = std::collections::BTreeMap::new();
                for (k, v) in checked {
                    let prev = d.params.insert(k.clone(), v).unwrap_or_else(|| def.param(&k).map(|q| q.default).unwrap_or(v));
                    old.insert(k, prev);
                }
                let old_preset = std::mem::replace(&mut d.preset, preset);
                Ok(Command::SetDeviceParams { device, params: old, preset: old_preset })
            }
            Command::SetDeviceSample { device, slot, sample } => {
                if let Some(s) = &sample {
                    s.validate().map_err(|r| invalid("sample", r))?;
                    if s.slot != slot {
                        return Err(invalid("sample", "sample.slot must equal slot"));
                    }
                    if p.asset(s.asset).is_none() {
                        return Err(EditError::AssetNotFound(s.asset));
                    }
                }
                let (d, def) = builtin_mut(p, device)?;
                if slot >= def.samples {
                    return Err(invalid("slot", format!("{} has {} sample slots", def.name, def.samples)));
                }
                let pos = d.samples.iter().position(|s| s.slot == slot);
                let old = match (pos, sample) {
                    (Some(i), Some(s)) => Some(std::mem::replace(&mut d.samples[i], s)),
                    (Some(i), None) => Some(d.samples.remove(i)),
                    (None, Some(s)) => {
                        let at = d.samples.partition_point(|x| x.slot < slot);
                        d.samples.insert(at, s);
                        None
                    }
                    (None, None) => None,
                };
                Ok(Command::SetDeviceSample { device, slot, sample: old })
            }
            Command::SetDeviceSidechain { device, source } => {
                let (ti, _) = p.find_device(device).ok_or_else(|| invalid("device", format!("{} not found", device.0)))?;
                if let Some(s) = source {
                    let si = p.track_index(s).ok_or(EditError::TrackNotFound(s))?;
                    if si == ti {
                        return Err(invalid("source", "a device can't sidechain from its own track"));
                    }
                }
                let (d, def) = builtin_mut(p, device)?;
                if !def.sidechain {
                    return Err(invalid("device", format!("{} has no sidechain input", def.name)));
                }
                let old = std::mem::replace(&mut d.sidechain, source);
                Ok(Command::SetDeviceSidechain { device, source: old })
            }
            Command::AddModLink { source, index, mut link } => {
                let (ti, _) = p.find_device(source).ok_or_else(|| invalid("source", format!("{} not found", source.0)))?;
                let t = &p.tracks[ti];
                let s = t.devices.iter().find(|d| d.id == source).ok_or_else(|| invalid("source", "not in an insert chain"))?;
                if let Some(why) = crate::modulation::link_refusal(t, s, link.slot, link.target, &link.param) {
                    return Err(invalid("link", why));
                }
                (link.min, link.max) =
                    crate::modulation::clamp_range(s, link.min, link.max).ok_or_else(|| invalid("link", "range must be finite"))?;
                let s = insert_mut(p, source)?;
                let i = index.unwrap_or(s.links.len());
                if i > s.links.len() {
                    return Err(invalid("index", "out of range"));
                }
                s.links.insert(i, link);
                Ok(Command::RemoveModLink { source, index: i })
            }
            Command::RemoveModLink { source, index } => {
                let s = insert_mut(p, source)?;
                if index >= s.links.len() {
                    return Err(invalid("index", "out of range"));
                }
                let link = s.links.remove(index);
                Ok(Command::AddModLink { source, index: Some(index), link })
            }
            Command::SetModLink { source, index, min, max } => {
                let s = insert_mut(p, source)?;
                let (min, max) = crate::modulation::clamp_range(s, min, max).ok_or_else(|| invalid("link", "range must be finite"))?;
                let l = s.links.get_mut(index).ok_or_else(|| invalid("index", "out of range"))?;
                let old = (std::mem::replace(&mut l.min, min), std::mem::replace(&mut l.max, max));
                Ok(Command::SetModLink { source, index, min: old.0, max: old.1 })
            }
            Command::SetDeviceChain { device, chain } => {
                let (ti, slot) = p.find_device(device).ok_or_else(|| invalid("device", format!("{} not found", device.0)))?;
                let DeviceSlot::Insert(i) = slot else { return Err(invalid("device", "instruments don't go in a chain")) };
                let devs = &p.tracks[ti].devices;
                if let Some(h) = chain {
                    if crate::modulation::outputs(&devs[i]) > 0 {
                        return Err(invalid("device", "chains don't hold LFOs or other chains"));
                    }
                    let hi =
                        devs.iter().position(|d| d.id == h).ok_or_else(|| invalid("chain", "the chain head must be on the same track"))?;
                    if !crate::modulation::is_rack(&devs[hi]) {
                        return Err(invalid("chain", "not a chain head"));
                    }
                    // The device must sit right after the head or one of its members.
                    let run = crate::modulation::members(devs, hi);
                    if i != run.end && !run.contains(&i) {
                        return Err(invalid("chain", "move the device next to the chain first"));
                    }
                }
                let old = std::mem::replace(&mut p.tracks[ti].devices[i].chain, chain);
                Ok(Command::SetDeviceChain { device, chain: old })
            }
            Command::SetDeviceLabel { device, label } => {
                let label = label.map(|l| l.trim().to_string()).filter(|l| !l.is_empty());
                let d = insert_mut(p, device)?;
                let old = std::mem::replace(&mut d.label, label);
                Ok(Command::SetDeviceLabel { device, label: old })
            }
            Command::SetMacroName { device, index, name } => {
                let d = insert_mut(p, device)?;
                if !crate::modulation::is_rack(d) || index >= crate::builtin::rack::MACROS {
                    return Err(invalid("index", "not a macro of a chain head"));
                }
                if d.macro_names.len() <= index {
                    d.macro_names.resize(index + 1, String::new());
                }
                let old = std::mem::replace(&mut d.macro_names[index], name.trim().to_string());
                while d.macro_names.last().is_some_and(|n| n.is_empty()) {
                    d.macro_names.pop();
                }
                Ok(Command::SetMacroName { device, index, name: old })
            }
            Command::SetDeviceView { device, folded, collapsed } => {
                let d = insert_or_instrument_mut(p, device)?;
                let old = (std::mem::replace(&mut d.folded, folded), std::mem::replace(&mut d.collapsed, collapsed));
                Ok(Command::SetDeviceView { device, folded: old.0, collapsed: old.1 })
            }
            Command::SetCardParams { device, params } => {
                let d = insert_or_instrument_mut(p, device)?;
                if d.plugin.format == crate::device::PluginFormat::Builtin {
                    return Err(invalid("device", "built-in devices have a fixed panel"));
                }
                let mut seen = std::collections::HashSet::new();
                let params: Vec<u32> = params.into_iter().filter(|id| seen.insert(*id)).collect();
                let old = std::mem::replace(&mut d.card_params, params);
                Ok(Command::SetCardParams { device, params: old })
            }
            Command::ReplaceDevice { device, mut with } => {
                let (ti, slot) = p.find_device(device).ok_or_else(|| invalid("device", format!("{} not found", device.0)))?;
                let DeviceSlot::Insert(i) = slot else { return Err(invalid("device", "use SetInstrumentPlugin for instruments")) };
                if with.plugin.kind != PluginKind::Effect {
                    return Err(invalid("with", "only effects go in the insert chain"));
                }
                sanitize_builtin(p, &mut with)?;
                if with.id.0 == 0 {
                    with.id = DeviceId(p.alloc_id());
                } else if with.id != device && p.find_device(with.id).is_some() {
                    return Err(EditError::DuplicateId(with.id.0));
                } else {
                    p.next_id = p.next_id.max(with.id.0 + 1);
                }
                let old = &p.tracks[ti].devices[i];
                if old.chain.is_some() && crate::modulation::outputs(&with) > 0 {
                    return Err(invalid("with", "chains don't hold LFOs or other chains"));
                }
                if crate::modulation::is_rack(old) {
                    return Err(invalid("device", "a chain head can't be swapped"));
                }
                with.chain = old.chain;
                let new_id = with.id;
                let old = std::mem::replace(&mut p.tracks[ti].devices[i], with);
                Ok(Command::ReplaceDevice { device: new_id, with: old })
            }
            Command::SetAutomationLane { track, target, lane } => {
                if let Some(l) = &lane {
                    if l.target != target {
                        return Err(invalid("lane", "lane.target must equal target"));
                    }
                    l.validate().map_err(|r| invalid("automation", r))?;
                }
                let lanes: &mut Vec<AutomationLane> = match track {
                    None => {
                        if target != AutomationTarget::Volume {
                            return Err(invalid("target", "the master only has volume automation"));
                        }
                        &mut p.master_automation
                    }
                    Some(id) => {
                        let ti = p.track_index(id).ok_or(EditError::TrackNotFound(id))?;
                        match &target {
                            AutomationTarget::Synth { .. } if p.tracks[ti].kind != TrackKind::Instrument => {
                                return Err(invalid("target", "synth automation needs an instrument track"));
                            }
                            AutomationTarget::Plugin { device, .. }
                                if !p.tracks[ti].devices.iter().chain(p.tracks[ti].instrument_plugin.iter()).any(|d| d.id == *device) =>
                            {
                                return Err(invalid("target", "plugin automation must target a device on the same track"));
                            }
                            AutomationTarget::DeviceOn { device, .. }
                                if !p.tracks[ti].devices.iter().chain(p.tracks[ti].instrument_plugin.iter()).any(|d| d.id == *device) =>
                            {
                                return Err(invalid("target", "device automation must target a device on the same track"));
                            }
                            AutomationTarget::Builtin { device, device_type, param } => {
                                let d = p.tracks[ti]
                                    .devices
                                    .iter()
                                    .chain(p.tracks[ti].instrument_plugin.iter())
                                    .find(|d| d.id == *device)
                                    .ok_or_else(|| invalid("target", "device automation must target a device on the same track"))?;
                                let def = crate::builtin::def_of(d)
                                    .filter(|def| def.id == device_type)
                                    .ok_or_else(|| invalid("target", "device kind mismatch"))?;
                                if !def.param(param).is_some_and(|q| q.automatable) {
                                    return Err(invalid("target", format!("{} has no automatable parameter {param:?}", def.name)));
                                }
                            }
                            _ => {}
                        }
                        &mut p.tracks[ti].automation
                    }
                };
                let pos = lanes.iter().position(|l| l.target == target);
                let old = match (pos, lane) {
                    (Some(i), Some(l)) => Some(std::mem::replace(&mut lanes[i], l)),
                    (Some(i), None) => Some(lanes.remove(i)),
                    (None, Some(l)) => {
                        lanes.push(l);
                        None
                    }
                    (None, None) => None,
                };
                Ok(Command::SetAutomationLane { track, target, lane: old })
            }
            Command::SetSends { track, sends } => {
                let t = p.track(track).ok_or(EditError::TrackNotFound(track))?;
                validate_routing(p, track, t.kind, &sends, t.output)?;
                let old = std::mem::replace(&mut p.track_mut(track).expect("checked").sends, sends);
                Ok(Command::SetSends { track, sends: old })
            }
            Command::SetTrackOutput { track, output } => {
                let t = p.track(track).ok_or(EditError::TrackNotFound(track))?;
                validate_routing(p, track, t.kind, &[], output)?;
                let old = std::mem::replace(&mut p.track_mut(track).expect("checked").output, output);
                Ok(Command::SetTrackOutput { track, output: old })
            }
            Command::Batch { label, commands } => {
                let mut inverses = Vec::with_capacity(commands.len());
                for cmd in commands {
                    match cmd.apply(p) {
                        Ok(inv) => inverses.push(inv),
                        Err(e) => {
                            // Roll back what was applied so the batch is atomic.
                            for inv in inverses.into_iter().rev() {
                                inv.apply(p).expect("inverse of a successful command must apply");
                            }
                            return Err(e);
                        }
                    }
                }
                inverses.reverse();
                Ok(Command::Batch { label, commands: inverses })
            }
        }
    }
}

/// The built-in device `id` and its definition.
fn builtin_mut(p: &mut Project, id: DeviceId) -> Result<(&mut Device, &'static crate::builtin::DeviceDef), EditError> {
    let (ti, slot) = p.find_device(id).ok_or_else(|| invalid("device", format!("{} not found", id.0)))?;
    let d = match slot {
        DeviceSlot::Insert(i) => &mut p.tracks[ti].devices[i],
        DeviceSlot::Instrument => p.tracks[ti].instrument_plugin.as_mut().expect("found"),
    };
    let def = crate::builtin::def_of(d).ok_or_else(|| invalid("device", "not a built-in device"))?;
    Ok((d, def))
}

/// An insert by id.
fn insert_mut(p: &mut Project, id: DeviceId) -> Result<&mut Device, EditError> {
    match p.find_device(id) {
        Some((ti, DeviceSlot::Insert(i))) => Ok(&mut p.tracks[ti].devices[i]),
        Some((_, DeviceSlot::Instrument)) => Err(invalid("device", "not an insert")),
        None => Err(invalid("device", format!("{} not found", id.0))),
    }
}

/// An insert or an instrument plugin by id.
fn insert_or_instrument_mut(p: &mut Project, id: DeviceId) -> Result<&mut Device, EditError> {
    let (ti, slot) = p.find_device(id).ok_or_else(|| invalid("device", format!("{} not found", id.0)))?;
    Ok(match slot {
        DeviceSlot::Insert(i) => &mut p.tracks[ti].devices[i],
        DeviceSlot::Instrument => p.tracks[ti].instrument_plugin.as_mut().expect("found"),
    })
}

/// Built-in devices entering the model: the definition must exist; stored
/// values are snapped into range, unknown ids dropped and missing ids filled
/// with defaults (so the saved state is always complete).
fn sanitize_builtin(p: &Project, d: &mut Device) -> Result<(), EditError> {
    if d.plugin.format != crate::device::PluginFormat::Builtin {
        if !d.samples.is_empty() {
            return Err(invalid("device", "only built-in devices hold samples"));
        }
        return Ok(());
    }
    let def = crate::builtin::find(&d.plugin.id).ok_or_else(|| invalid("device", format!("unknown built-in device {:?}", d.plugin.id)))?;
    d.samples.sort_by_key(|s| s.slot);
    for (i, s) in d.samples.iter().enumerate() {
        s.validate().map_err(|r| invalid("sample", r))?;
        if s.slot >= def.samples || (i > 0 && d.samples[i - 1].slot == s.slot) {
            return Err(invalid("sample", format!("slot {} is not a free slot of {}", s.slot, def.name)));
        }
        if p.asset(s.asset).is_none() {
            return Err(EditError::AssetNotFound(s.asset));
        }
    }
    let mut values = def.defaults();
    for (k, v) in std::mem::take(&mut d.params) {
        if let Some(q) = def.param(&k) {
            values.insert(k, q.snap(v));
        }
    }
    d.params = values;
    if !def.sidechain {
        d.sidechain = None;
    }
    Ok(())
}

fn kind_name(k: TrackKind) -> &'static str {
    match k {
        TrackKind::Audio => "Audio",
        TrackKind::Instrument => "Instrument",
        TrackKind::Return => "Return",
        TrackKind::Group => "Group",
    }
}

/// Sends and output of track `id` (of `kind`; id 0 = a track not added yet): sends only from
/// ordinary tracks, each into a different return track at 0 ..= +6 dB; the output is the master
/// or, for ordinary tracks, a return or group track. Returns and groups play into the master and
/// send nothing (groups don't nest).
fn validate_routing(p: &Project, id: TrackId, kind: TrackKind, sends: &[TrackSend], output: Option<TrackId>) -> Result<(), EditError> {
    let is_return_track = |t: TrackId| t != id && p.track(t).is_some_and(|t| t.is_return());
    let is_bus_track = |t: TrackId| t != id && p.track(t).is_some_and(|t| t.is_bus());
    // Group tracks may send into returns (returns never send, so sends form no loop).
    if kind == TrackKind::Return && !sends.is_empty() {
        return Err(invalid("track", "a return track has no sends"));
    }
    if matches!(kind, TrackKind::Return | TrackKind::Group) && output.is_some() {
        return Err(invalid("track", "a return or group track plays into the master"));
    }
    for (i, s) in sends.iter().enumerate() {
        if !is_return_track(s.target) {
            return Err(invalid("sends", format!("{} is not a return track", s.target.0)));
        }
        if !(0.0..=TrackSend::MAX_LEVEL).contains(&s.level) {
            return Err(invalid("sends", format!("level {} is outside 0..={}", s.level, TrackSend::MAX_LEVEL)));
        }
        if sends[..i].iter().any(|o| o.target == s.target) {
            return Err(invalid("sends", format!("two sends into {}", s.target.0)));
        }
    }
    if let Some(o) = output
        && !is_bus_track(o)
    {
        return Err(invalid("output", format!("{} is not a return or group track", o.0)));
    }
    Ok(())
}

fn validate_gain(field: &'static str, g: f32) -> Result<(), EditError> {
    if !g.is_finite() || !(0.0..=MAX_GAIN).contains(&g) {
        return Err(invalid(field, format!("{g} outside 0..={MAX_GAIN} (linear)")));
    }
    Ok(())
}

fn validate_trim(db: f32) -> Result<(), EditError> {
    let (lo, hi) = Track::TRIM_RANGE_DB;
    if !db.is_finite() || !(lo..=hi).contains(&db) {
        return Err(invalid("trim", format!("{db} outside {lo}..={hi} dB")));
    }
    Ok(())
}

fn validate_width(w: f32) -> Result<(), EditError> {
    let (lo, hi) = Track::WIDTH_RANGE;
    if !w.is_finite() || !(lo..=hi).contains(&w) {
        return Err(invalid("width", format!("{w} outside {lo}..={hi}")));
    }
    Ok(())
}

fn validate_pan(pan: f32) -> Result<(), EditError> {
    if !pan.is_finite() || !(-1.0..=1.0).contains(&pan) {
        return Err(invalid("pan", format!("{pan} outside -1..=1")));
    }
    Ok(())
}

fn split_clip(p: &mut Project, clip: ClipId, at: Ticks, new_id: ClipId) -> Result<Command, EditError> {
    if new_id.0 != 0 && p.clip_id_in_use(new_id) {
        return Err(EditError::DuplicateId(new_id.0));
    }
    if let Some((ti, ci)) = p.find_clip(clip) {
        let c = p.tracks[ti].clips[ci].clone();
        if c.warp.enabled || c.warp.reverse {
            return Err(invalid("clip", "warped or reversed clips cannot be split"));
        }
        if at <= c.start || at >= c.start + p.clip_length_ticks(&c) {
            return Err(invalid("at", "must fall inside the clip"));
        }
        let cut =
            (p.clip_source_at(&c, (at - c.start) as f64).round() as u64).clamp(c.source_offset + 1, c.source_offset + c.source_length - 1);
        let new_id = if new_id.0 == 0 { ClipId(p.alloc_id()) } else { new_id };
        p.next_id = p.next_id.max(new_id.0 + 1);
        // The left part keeps the fade-in, the right part the fade-out (each within its part).
        let fades = c.fades.clamped(p.clip_length_ticks(&c));
        let right_fades = ClipFades { fade_in: 0, fade_out: fades.fade_out };
        let right = Clip {
            id: new_id,
            start: at,
            source_offset: cut,
            source_length: c.source_offset + c.source_length - cut,
            fades: right_fades,
            ..c.clone()
        };
        let t = &mut p.tracks[ti];
        t.clips[ci].source_length = cut - c.source_offset;
        t.clips[ci].fades = ClipFades { fade_in: fades.fade_in, fade_out: 0 };
        t.clips.push(right);
        t.clips.sort_by_key(|c| (c.start, c.id));
        return Ok(Command::Batch {
            label: "Split Clip".into(),
            commands: vec![
                Command::RemoveClip { clip: new_id },
                Command::TrimClip { clip, start: c.start, source_offset: c.source_offset, source_length: c.source_length },
                Command::SetClipFades { clip, fades: c.fades },
            ],
        });
    }
    let (ti, ci) = p.find_midi_clip(clip).ok_or(EditError::ClipNotFound(clip))?;
    let c = p.tracks[ti].midi_clips[ci].clone();
    if c.pattern.is_some() {
        return Err(invalid("clip", "pattern clips cannot be split"));
    }
    if at <= c.start || at >= c.end() {
        return Err(invalid("at", "must fall inside the clip"));
    }
    let rel = at - c.start;
    let mut left_notes = Vec::new();
    let mut right_notes = Vec::new();
    for n in c.notes.iter().filter(|n| n.start < c.length) {
        let end = (n.start + n.length).min(c.length);
        if end <= rel {
            left_notes.push(*n);
        } else if n.start >= rel {
            right_notes.push(Note { start: n.start - rel, length: end - n.start, ..*n });
        } else {
            left_notes.push(Note { length: rel - n.start, ..*n });
            right_notes.push(Note { start: 0, length: end - rel, ..*n });
        }
    }
    let new_id = if new_id.0 == 0 { ClipId(p.alloc_id()) } else { new_id };
    p.next_id = p.next_id.max(new_id.0 + 1);
    let right = MidiClip { id: new_id, start: at, length: c.length - rel, notes: right_notes, ..c.clone() };
    let t = &mut p.tracks[ti];
    t.midi_clips[ci].length = rel;
    t.midi_clips[ci].notes = left_notes;
    t.midi_clips.push(right);
    t.midi_clips.sort_by_key(|c| (c.start, c.id));
    Ok(Command::Batch {
        label: "Split Clip".into(),
        commands: vec![
            Command::RemoveMidiClip { clip: new_id },
            Command::SetMidiContent { clip, notes: c.notes.clone(), pattern: None },
            Command::ResizeMidiClip { clip, start: c.start, length: c.length },
        ],
    })
}

fn validate_midi_clip(c: &MidiClip) -> Result<(), EditError> {
    if c.start < 0 || c.length <= 0 {
        return Err(invalid("clip", "requires start >= 0 and length > 0"));
    }
    midi::validate_notes(&c.notes).map_err(|r| invalid("notes", r))?;
    match &c.pattern {
        Some(p) => p.validate().map_err(|r| invalid("pattern", r)),
        None => Ok(()),
    }
}

fn validate_clip(p: &Project, c: &Clip) -> Result<(), EditError> {
    let a = p.asset(c.asset).ok_or(EditError::AssetNotFound(c.asset))?;
    if c.start < 0 {
        return Err(invalid("start", "must be >= 0"));
    }
    if c.source_length == 0 {
        return Err(invalid("source_length", "must be > 0"));
    }
    if c.source_offset.checked_add(c.source_length).is_none_or(|end| end > a.frames) {
        return Err(invalid(
            "source_region",
            format!("offset {} + length {} exceeds asset length {}", c.source_offset, c.source_length, a.frames),
        ));
    }
    c.warp.validate(a).map_err(|r| invalid("warp", r))?;
    validate_gain("clip_gain", c.gain)
}
