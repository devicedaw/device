//! Ableton Live set writer for Export › Project. Writes the gzipped XML document Live 11
//! and later open (schema 5 / 11.0), with every element Live saves for an audio track, a
//! MIDI track, a return track, the main and pre-hear tracks and the set itself, in Live's order, so the
//! file reads like one Live wrote. Element names and defaults were learned from sets saved
//! by Live (structure only).
//!
//! [validate] parses a written document back and checks it against that structure: the
//! required elements of every track kind and clip, unique ids below NextPointeeId, one clip
//! slot per scene on every track, clip and note times inside their clips, the tempo and time
//! signature envelopes, one send per return track on every track, and that every referenced
//! sample exists.
use std::fmt::Write as _;
use std::path::Path;

/// Live's schema version the document declares (opens in Live 11.3 and later).
const MAJOR_VERSION: &str = "5";
const MINOR_VERSION: &str = "11.0_11300";
const SCENES: usize = 8;
/// Time Live gives the one event of a static automation envelope (before the song start).
const ENVELOPE_START: &str = "-63072000";
const MIDI_CONTROLLERS: usize = 131;

/// The folder that makes a directory a Live project. Live resolves a sample's project-relative
/// path (RelativePathType 3) only when the set sits next to it; without it every sample is
/// offline, even with a correct absolute path. Live 12 opens a project whose folder is empty.
pub const PROJECT_INFO_DIR: &str = "Ableton Project Info";

/// Marks "folder" as a Live project by creating its [PROJECT_INFO_DIR].
pub fn mark_project(folder: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(folder.join(PROJECT_INFO_DIR))
}

pub struct LiveSet {
    pub bpm: f64,
    pub numerator: u32,
    pub denominator: u32,
    /// Arrangement loop in beats (quarter notes).
    pub loop_on: bool,
    pub loop_start: f64,
    pub loop_length: f64,
    pub tracks: Vec<SetTrack>,
    /// Return tracks, written after the ordinary tracks. Every [SetTrack] has one send per return.
    pub returns: Vec<SetReturn>,
}

pub struct SetReturn {
    pub name: String,
    pub color: u32,
    pub on: bool,
    pub volume: f32,
    pub pan: f32,
    /// Live's per-return switch: sends to it tap before the track faders.
    pub pre: bool,
}

/// Live's send range: -70 dB (shown as -inf) ..= 0 dB.
pub const SEND_RANGE: (f64, f64) = (0.0003162277571, 1.0);

pub struct SetTrack {
    pub name: String,
    /// Live colour index (0..=69).
    pub color: u32,
    /// Track activator: false turns the track off.
    pub on: bool,
    /// Linear gain, 1.0 = 0 dB.
    pub volume: f32,
    /// -1 (left) ..= 1 (right).
    pub pan: f32,
    /// Send level to each of the set's returns, in order (linear, clamped to [SEND_RANGE]);
    /// missing entries are off.
    pub sends: Vec<f32>,
    /// Output "Sends Only": the track is heard through its sends alone, not on the master.
    pub sends_only: bool,
    pub content: SetContent,
}

pub enum SetContent {
    Audio(Vec<SetAudioClip>),
    Midi(Vec<SetMidiClip>),
}

/// An audio clip that plays its file from the start at the set tempo (no stretching).
pub struct SetAudioClip {
    pub name: String,
    /// Arrangement position and length in beats.
    pub start: f64,
    pub length: f64,
    /// Path relative to the project folder ("/" separated) and the absolute path.
    pub relative_path: String,
    pub absolute_path: String,
    pub file_size: u64,
    pub frames: u64,
    pub sample_rate: u32,
    /// Warp markers as (seconds into the file, beat); empty plays the file unstretched.
    pub markers: Vec<(f64, f64)>,
}

pub struct SetMidiClip {
    pub name: String,
    pub start: f64,
    pub length: f64,
    pub notes: Vec<SetNote>,
}

#[derive(Clone, Copy)]
pub struct SetNote {
    pub pitch: u8,
    /// Beats from the clip start.
    pub time: f64,
    pub duration: f64,
    pub velocity: u8,
}

/// Live's time signature value: numerator − 1 plus 99 × log2(denominator) (4/4 = 201).
pub fn time_signature_value(numerator: u32, denominator: u32) -> u32 {
    numerator.clamp(1, 99) - 1 + 99 * denominator.max(1).ilog2()
}

struct Xml {
    s: String,
    depth: usize,
    next_id: u64,
}

fn esc(v: &str) -> String {
    let mut o = String::with_capacity(v.len());
    for c in v.chars() {
        match c {
            '&' => o.push_str("&amp;"),
            '<' => o.push_str("&lt;"),
            '>' => o.push_str("&gt;"),
            '"' => o.push_str("&quot;"),
            c if (c as u32) < 0x20 && c != '\t' => o.push(' '),
            c => o.push(c),
        }
    }
    o
}

/// Live writes reals with up to 17 significant digits and integers without a point.
fn num(v: f64) -> String {
    if v.fract() == 0.0 && v.abs() < 1e15 { format!("{}", v as i64) } else { format!("{v}") }
}

impl Xml {
    fn indent(&mut self) {
        for _ in 0..self.depth {
            self.s.push('\t');
        }
    }
    fn open(&mut self, tag: &str, attrs: &[(&str, String)]) {
        self.indent();
        let _ = write!(self.s, "<{tag}");
        for (k, v) in attrs {
            let _ = write!(self.s, " {k}=\"{}\"", esc(v));
        }
        self.s.push_str(">\n");
        self.depth += 1;
    }
    fn close(&mut self, tag: &str) {
        self.depth -= 1;
        self.indent();
        let _ = writeln!(self.s, "</{tag}>");
    }
    fn empty(&mut self, tag: &str, attrs: &[(&str, String)]) {
        self.indent();
        let _ = write!(self.s, "<{tag}");
        for (k, v) in attrs {
            let _ = write!(self.s, " {k}=\"{}\"", esc(v));
        }
        self.s.push_str(" />\n");
    }
    fn v(&mut self, tag: &str, value: impl ToString) {
        self.empty(tag, &[("Value", value.to_string())]);
    }
    fn id(&mut self) -> u64 {
        self.next_id += 1;
        self.next_id
    }
    /// An automation or modulation target with a fresh pointee id.
    fn target(&mut self, tag: &str) -> u64 {
        let id = self.id();
        self.open(tag, &[("Id", id.to_string())]);
        self.v("LockEnvelope", 0);
        self.close(tag);
        id
    }
    fn on_off(&mut self, tag: &str, manual: bool) {
        self.open(tag, &[]);
        self.v("LomId", 0);
        self.v("Manual", manual);
        self.target("AutomationTarget");
        self.open("MidiCCOnOffThresholds", &[]);
        self.v("Min", 64);
        self.v("Max", 127);
        self.close("MidiCCOnOffThresholds");
        self.close(tag);
    }
    /// A continuous parameter; returns its automation target id.
    fn float_param(&mut self, tag: &str, manual: f64, range: (f64, f64), modulation: bool) -> u64 {
        self.open(tag, &[]);
        self.v("LomId", 0);
        self.v("Manual", num(manual));
        self.open("MidiControllerRange", &[]);
        self.v("Min", num(range.0));
        self.v("Max", num(range.1));
        self.close("MidiControllerRange");
        let id = self.target("AutomationTarget");
        if modulation {
            self.target("ModulationTarget");
        }
        self.close(tag);
        id
    }
    fn enum_param(&mut self, tag: &str, manual: u32) -> u64 {
        self.open(tag, &[]);
        self.v("LomId", 0);
        self.v("Manual", manual);
        let id = self.target("AutomationTarget");
        self.close(tag);
        id
    }
    fn wrapper(&mut self, tag: &str) {
        self.empty(tag, &[("LomId", "0".into())]);
    }
    /// The elements every device-like part of a track starts with (mixer, sequencers).
    fn device_head(&mut self, show_preset_name: bool) {
        self.v("LomId", 0);
        self.v("LomIdView", 0);
        self.v("IsExpanded", true);
        self.on_off("On", true);
        self.v("ModulationSourceCount", 0);
        self.wrapper("ParametersListWrapper");
        let id = self.id();
        self.empty("Pointee", &[("Id", id.to_string())]);
        self.v("LastSelectedTimeableIndex", 0);
        self.v("LastSelectedClipEnvelopeIndex", 0);
        self.open("LastPresetRef", &[]);
        self.empty("Value", &[]);
        self.close("LastPresetRef");
        self.empty("LockedScripts", &[]);
        self.v("IsFolded", false);
        self.v("ShouldShowPresetName", show_preset_name);
        self.v("UserName", "");
        self.v("Annotation", "");
        self.open("SourceContext", &[]);
        self.empty("Value", &[]);
        self.close("SourceContext");
    }
    fn clip_slots(&mut self, n: usize) {
        if n == 0 {
            self.empty("ClipSlotList", &[]);
            return;
        }
        self.open("ClipSlotList", &[]);
        for i in 0..n {
            self.open("ClipSlot", &[("Id", i.to_string())]);
            self.v("LomId", 0);
            self.open("ClipSlot", &[]);
            self.empty("Value", &[]);
            self.close("ClipSlot");
            self.v("HasStop", true);
            self.v("NeedRefreeze", true);
            self.close("ClipSlot");
        }
        self.close("ClipSlotList");
    }
    fn routing(&mut self, tag: &str, target: &str, upper: &str, lower: &str) {
        self.open(tag, &[]);
        self.v("Target", target);
        self.v("UpperDisplayString", upper);
        self.v("LowerDisplayString", lower);
        self.open("MpeSettings", &[]);
        self.v("ZoneType", 0);
        self.v("FirstNoteChannel", 1);
        self.v("LastNoteChannel", 15);
        self.close("MpeSettings");
        self.close(tag);
    }
    fn grid(&mut self, tag: &str, snap: bool, fixed: bool) {
        self.open(tag, &[]);
        self.v("FixedNumerator", 1);
        self.v("FixedDenominator", 16);
        self.v("GridIntervalPixel", 20);
        self.v("Ntoles", 2);
        self.v("SnapToGrid", snap);
        self.v("Fixed", fixed);
        self.close(tag);
    }
    fn follow_action(&mut self, jump: u32) {
        self.open("FollowAction", &[]);
        self.v("FollowTime", 4);
        self.v("IsLinked", true);
        self.v("LoopIterations", 1);
        self.v("FollowActionA", 4);
        self.v("FollowActionB", 0);
        self.v("FollowChanceA", 100);
        self.v("FollowChanceB", 0);
        self.v("JumpIndexA", jump);
        self.v("JumpIndexB", jump);
        self.v("FollowActionEnabled", false);
        self.close("FollowAction");
    }
    fn transform_view_state(&mut self) {
        self.open("AutomationTransformViewState", &[]);
        self.v("IsTransformPending", false);
        self.empty("TimeAndValueTransforms", &[]);
        self.close("AutomationTransformViewState");
    }
    /// The elements an audio and a MIDI clip share, up to TakeId.
    fn clip_head(&mut self, set: &LiveSet, name: &str, color: u32, start: f64, length: f64) {
        let end = start + length;
        self.v("LomId", 0);
        self.v("LomIdView", 0);
        self.v("CurrentStart", num(start));
        self.v("CurrentEnd", num(end));
        self.open("Loop", &[]);
        self.v("LoopStart", 0);
        self.v("LoopEnd", num(length));
        self.v("StartRelative", 0);
        self.v("LoopOn", false);
        self.v("OutMarker", num(length));
        self.v("HiddenLoopStart", 0);
        self.v("HiddenLoopEnd", num(length));
        self.close("Loop");
        self.v("Name", name);
        self.v("Annotation", "");
        self.v("Color", color);
        self.v("LaunchMode", 0);
        self.v("LaunchQuantisation", 0);
        self.open("TimeSignature", &[]);
        self.open("TimeSignatures", &[]);
        self.open("RemoteableTimeSignature", &[("Id", "0".into())]);
        self.v("Numerator", set.numerator);
        self.v("Denominator", set.denominator);
        self.v("Time", 0);
        self.close("RemoteableTimeSignature");
        self.close("TimeSignatures");
        self.close("TimeSignature");
        self.open("Envelopes", &[]);
        self.empty("Envelopes", &[]);
        self.close("Envelopes");
        self.open("ScrollerTimePreserver", &[]);
        self.v("LeftTime", 0);
        self.v("RightTime", num(length));
        self.close("ScrollerTimePreserver");
        self.open("TimeSelection", &[]);
        self.v("AnchorTime", 0);
        self.v("OtherTime", 0);
        self.close("TimeSelection");
        self.v("Legato", false);
        self.v("Ram", false);
        self.open("GrooveSettings", &[]);
        self.v("GrooveId", -1);
        self.close("GrooveSettings");
        self.v("Disabled", false);
        self.v("VelocityAmount", 0);
        self.follow_action(1);
        self.grid("Grid", true, false);
        self.v("FreezeStart", 0);
        self.v("FreezeEnd", 0);
        self.v("IsWarped", true);
        self.v("TakeId", 1);
    }
    fn audio_clip(&mut self, set: &LiveSet, id: usize, color: u32, c: &SetAudioClip) {
        self.open("AudioClip", &[("Id", id.to_string()), ("Time", num(c.start))]);
        self.clip_head(set, &c.name, color, c.start, c.length);
        self.open("SampleRef", &[]);
        self.open("FileRef", &[]);
        self.v("RelativePathType", 3);
        self.v("RelativePath", &c.relative_path);
        self.v("Path", &c.absolute_path);
        self.v("Type", 2);
        self.v("LivePackName", "");
        self.v("LivePackId", "");
        self.v("OriginalFileSize", c.file_size);
        self.v("OriginalCrc", 0);
        self.close("FileRef");
        self.v("LastModDate", 0);
        self.empty("SourceContext", &[]);
        self.v("SampleUsageHint", 0);
        self.v("DefaultDuration", c.frames);
        self.v("DefaultSampleRate", c.sample_rate);
        self.close("SampleRef");
        self.open("Onsets", &[]);
        self.empty("UserOnsets", &[]);
        self.v("HasUserOnsets", false);
        self.close("Onsets");
        self.v("WarpMode", 0);
        self.v("GranularityTones", 30);
        self.v("GranularityTexture", 65);
        self.v("FluctuationTexture", 25);
        self.v("TransientResolution", 6);
        self.v("TransientLoopMode", 2);
        self.v("TransientEnvelope", 100);
        self.v("ComplexProFormants", 100);
        self.v("ComplexProEnvelope", 128);
        self.v("Sync", true);
        self.v("HiQ", true);
        self.v("Fade", false);
        self.open("Fades", &[]);
        self.v("FadeInLength", 0);
        self.v("FadeOutLength", 0);
        self.v("ClipFadesAreInitialized", true);
        self.v("CrossfadeInState", 0);
        self.v("FadeInCurveSkew", 0);
        self.v("FadeInCurveSlope", 0);
        self.v("FadeOutCurveSkew", 0);
        self.v("FadeOutCurveSlope", 0);
        self.v("IsDefaultFadeIn", true);
        self.v("IsDefaultFadeOut", true);
        self.close("Fades");
        self.v("PitchCoarse", 0);
        self.v("PitchFine", 0);
        self.v("SampleVolume", 1);
        // Without markers, two fix the file's tempo at the set tempo: the file plays unstretched.
        let beat_sec = 60.0 / set.bpm;
        let unstretched = [(0.0, 0.0), (0.03125 * beat_sec, 0.03125)];
        self.open("WarpMarkers", &[]);
        for &(sec, beat) in if c.markers.is_empty() { &unstretched[..] } else { &c.markers[..] } {
            let id = self.id();
            self.empty("WarpMarker", &[("Id", id.to_string()), ("SecTime", num(sec)), ("BeatTime", num(beat))]);
        }
        self.close("WarpMarkers");
        self.empty("SavedWarpMarkersForStretched", &[]);
        self.v("MarkersGenerated", false);
        self.v("IsSongTempoMaster", false);
        self.close("AudioClip");
    }
    fn midi_clip(&mut self, set: &LiveSet, id: usize, color: u32, c: &SetMidiClip) {
        self.open("MidiClip", &[("Id", id.to_string()), ("Time", num(c.start))]);
        self.clip_head(set, &c.name, color, c.start, c.length);
        self.open("Notes", &[]);
        let mut pitches: Vec<u8> = c.notes.iter().map(|n| n.pitch).collect();
        pitches.sort_unstable();
        pitches.dedup();
        if pitches.is_empty() {
            self.empty("KeyTracks", &[]);
        } else {
            self.open("KeyTracks", &[]);
            let mut note_id = 0u32;
            for (k, p) in pitches.iter().enumerate() {
                self.open("KeyTrack", &[("Id", k.to_string())]);
                self.open("Notes", &[]);
                let mut notes: Vec<&SetNote> = c.notes.iter().filter(|n| n.pitch == *p).collect();
                notes.sort_by(|a, b| a.time.total_cmp(&b.time));
                for n in notes {
                    note_id += 1;
                    self.empty(
                        "MidiNoteEvent",
                        &[
                            ("Time", num(n.time)),
                            ("Duration", num(n.duration)),
                            ("Velocity", n.velocity.to_string()),
                            ("VelocityDeviation", "0".into()),
                            ("OffVelocity", "64".into()),
                            ("Probability", "1".into()),
                            ("IsEnabled", "true".into()),
                            ("NoteId", note_id.to_string()),
                        ],
                    );
                }
                self.close("Notes");
                self.v("MidiKey", p);
                self.close("KeyTrack");
            }
            self.close("KeyTracks");
        }
        self.open("PerNoteEventStore", &[]);
        self.empty("EventLists", &[]);
        self.close("PerNoteEventStore");
        self.open("NoteIdGenerator", &[]);
        self.v("NextId", c.notes.len() + 1);
        self.close("NoteIdGenerator");
        self.close("Notes");
        self.v("BankSelectCoarse", -1);
        self.v("BankSelectFine", -1);
        self.v("ProgramChange", -1);
        self.v("NoteEditorFoldInZoom", -1);
        self.v("NoteEditorFoldInScroll", 0);
        self.v("NoteEditorFoldOutZoom", -1);
        self.v("NoteEditorFoldOutScroll", 0);
        self.v("NoteEditorFoldScaleZoom", -1);
        self.v("NoteEditorFoldScaleScroll", 0);
        self.open("ScaleInformation", &[]);
        self.v("RootNote", 0);
        self.v("Name", "Major");
        self.close("ScaleInformation");
        self.v("IsInKey", false);
        self.v("NoteSpellingPreference", 3);
        self.v("PreferFlatRootNote", false);
        self.grid("ExpressionGrid", false, false);
        self.close("MidiClip");
    }
    /// Track start shared by every track kind, through LinkedTrackGroupId.
    fn track_head(&mut self, name: &str, user_name: &str, color: u32, envelopes: &[(u64, &str, String)]) {
        self.v("LomId", 0);
        self.v("LomIdView", 0);
        self.v("IsContentSelectedInDocument", false);
        self.v("PreferredContentViewMode", 0);
        self.open("TrackDelay", &[]);
        self.v("Value", 0);
        self.v("IsValueSampleBased", false);
        self.close("TrackDelay");
        self.open("Name", &[]);
        self.v("EffectiveName", name);
        self.v("UserName", user_name);
        self.v("Annotation", "");
        self.v("MemorizedFirstClipName", "");
        self.close("Name");
        self.v("Color", color);
        self.open("AutomationEnvelopes", &[]);
        if envelopes.is_empty() {
            self.empty("Envelopes", &[]);
        } else {
            self.open("Envelopes", &[]);
            for (i, (pointee, event, value)) in envelopes.iter().enumerate() {
                self.open("AutomationEnvelope", &[("Id", i.to_string())]);
                self.open("EnvelopeTarget", &[]);
                self.v("PointeeId", pointee);
                self.close("EnvelopeTarget");
                self.open("Automation", &[]);
                self.open("Events", &[]);
                self.empty(event, &[("Id", "0".into()), ("Time", ENVELOPE_START.into()), ("Value", value.clone())]);
                self.close("Events");
                self.transform_view_state();
                self.close("Automation");
                self.close("AutomationEnvelope");
            }
            self.close("Envelopes");
        }
        self.close("AutomationEnvelopes");
        self.v("TrackGroupId", -1);
        self.v("TrackUnfolded", true);
        self.wrapper("DevicesListWrapper");
        self.wrapper("ClipSlotsListWrapper");
        self.v("ViewData", "{}");
        self.open("TakeLanes", &[]);
        self.empty("TakeLanes", &[]);
        self.v("AreTakeLanesFolded", true);
        self.close("TakeLanes");
        self.v("LinkedTrackGroupId", -1);
    }
    fn chain_head(&mut self, lanes: usize, audio_in: (&str, &str), audio_out: (&str, &str, &str)) {
        self.open("AutomationLanes", &[]);
        self.open("AutomationLanes", &[]);
        for i in 0..lanes {
            self.open("AutomationLane", &[("Id", i.to_string())]);
            self.v("SelectedDevice", 0);
            self.v("SelectedEnvelope", 0);
            self.v("IsContentSelectedInDocument", false);
            self.v("LaneHeight", 68);
            self.close("AutomationLane");
        }
        self.close("AutomationLanes");
        self.v("AreAdditionalAutomationLanesFolded", false);
        self.close("AutomationLanes");
        self.open("ClipEnvelopeChooserViewState", &[]);
        self.v("SelectedDevice", 0);
        self.v("SelectedEnvelope", 0);
        self.v("PreferModulationVisible", false);
        self.close("ClipEnvelopeChooserViewState");
        self.routing("AudioInputRouting", audio_in.0, "Ext. In", audio_in.1);
        self.routing("MidiInputRouting", "MidiIn/External.All/-1", "Ext: All Ins", "");
        self.routing("AudioOutputRouting", audio_out.0, audio_out.1, audio_out.2);
        self.routing("MidiOutputRouting", "MidiOut/None", "None", "");
    }
    /// Mixer through SendsListWrapper; ids come from the counter. "sends" is None on the main
    /// and pre-hear tracks (no sends), else (level per return, active): one holder per return.
    fn mixer_body(&mut self, on: bool, volume: f32, pan: f32, sends: Option<(&[f32], usize, bool)>) {
        self.device_head(false);
        match sends {
            Some((levels, returns, active)) if returns > 0 => {
                self.open("Sends", &[]);
                for i in 0..returns {
                    self.open("TrackSendHolder", &[("Id", i.to_string())]);
                    let level = levels.get(i).copied().unwrap_or(0.0) as f64;
                    self.float_param("Send", level.clamp(SEND_RANGE.0, SEND_RANGE.1), SEND_RANGE, true);
                    self.v("Active", active);
                    self.close("TrackSendHolder");
                }
                self.close("Sends");
            }
            _ => self.empty("Sends", &[]),
        }
        self.on_off("Speaker", on);
        self.v("SoloSink", false);
        self.v("PanMode", 0);
        self.float_param("Pan", (pan as f64).clamp(-1.0, 1.0), (-1.0, 1.0), true);
        self.float_param("SplitStereoPanL", -1.0, (-1.0, 1.0), true);
        self.float_param("SplitStereoPanR", 1.0, (-1.0, 1.0), true);
        self.float_param("Volume", (volume as f64).clamp(0.0003162277571, 1.99526238), (0.0003162277571, 1.99526238), true);
        self.v("ViewStateSesstionTrackWidth", 93);
        self.enum_param("CrossFadeState", 1);
        self.wrapper("SendsListWrapper");
    }
    fn sample_sequencer_body(&mut self, set: &LiveSet, color: u32, slots: usize, monitoring: u32, clips: &[SetAudioClip]) {
        self.device_head(true);
        self.clip_slots(slots);
        self.v("MonitoringEnum", monitoring);
        self.open("Sample", &[]);
        self.open("ArrangerAutomation", &[]);
        if clips.is_empty() {
            self.empty("Events", &[]);
        } else {
            self.open("Events", &[]);
            for (i, c) in clips.iter().enumerate() {
                self.audio_clip(set, i, color, c);
            }
            self.close("Events");
        }
        self.transform_view_state();
        self.close("ArrangerAutomation");
        self.close("Sample");
        for t in [
            "VolumeModulationTarget",
            "TranspositionModulationTarget",
            "GrainSizeModulationTarget",
            "FluxModulationTarget",
            "SampleOffsetModulationTarget",
        ] {
            self.target(t);
        }
        self.v("PitchViewScrollPosition", -1073741824);
        self.v("SampleOffsetModulationScrollPosition", -1073741824);
        self.open("Recorder", &[]);
        self.v("IsArmed", false);
        self.v("TakeCounter", 1);
        self.close("Recorder");
    }
    fn inner_chain(&mut self) {
        self.open("DeviceChain", &[]);
        self.empty("Devices", &[]);
        self.empty("SignalModulations", &[]);
        self.close("DeviceChain");
    }
    fn track(&mut self, set: &LiveSet, id: u64, t: &SetTrack) {
        let tag = match t.content {
            SetContent::Audio(_) => "AudioTrack",
            SetContent::Midi(_) => "MidiTrack",
        };
        self.open(tag, &[("Id", id.to_string())]);
        self.track_head(&t.name, &t.name, t.color, &[]);
        self.v("SavedPlayingSlot", -1);
        self.v("SavedPlayingOffset", 0);
        self.v("Freeze", false);
        self.v("VelocityDetail", 0);
        self.v("NeedArrangerRefreeze", true);
        self.v("PostProcessFreezeClips", 0);
        self.open("DeviceChain", &[]);
        let out = if t.sends_only { ("AudioOut/None", "Sends Only", "") } else { ("AudioOut/Master", "Master", "") };
        self.chain_head(1, ("AudioIn/External/S0", "1/2"), out);
        self.open("Mixer", &[]);
        self.mixer_body(t.on, t.volume, t.pan, Some((&t.sends, set.returns.len(), true)));
        self.close("Mixer");
        self.open("MainSequencer", &[]);
        match &t.content {
            SetContent::Audio(clips) => self.sample_sequencer_body(set, t.color, SCENES, 1, clips),
            SetContent::Midi(clips) => {
                self.device_head(true);
                self.clip_slots(SCENES);
                self.v("MonitoringEnum", 1);
                self.open("ClipTimeable", &[]);
                self.open("ArrangerAutomation", &[]);
                if clips.is_empty() {
                    self.empty("Events", &[]);
                } else {
                    self.open("Events", &[]);
                    for (i, c) in clips.iter().enumerate() {
                        self.midi_clip(set, i, t.color, c);
                    }
                    self.close("Events");
                }
                self.transform_view_state();
                self.close("ArrangerAutomation");
                self.close("ClipTimeable");
                self.open("Recorder", &[]);
                self.v("IsArmed", false);
                self.v("TakeCounter", 1);
                self.close("Recorder");
                self.open("MidiControllers", &[]);
                for i in 0..MIDI_CONTROLLERS {
                    self.target(&format!("ControllerTargets.{i}"));
                }
                self.close("MidiControllers");
            }
        }
        self.close("MainSequencer");
        self.open("FreezeSequencer", &[]);
        self.sample_sequencer_body(set, t.color, SCENES, 1, &[]);
        self.close("FreezeSequencer");
        self.inner_chain();
        self.close("DeviceChain");
        if matches!(t.content, SetContent::Midi(_)) {
            let rewire = self.id();
            self.v("ReWireSlaveMidiTargetId", rewire);
            self.v("PitchbendRange", 96);
        }
        self.close(tag);
    }
    /// A return track: no clips, its own sends present but inactive (Live's default).
    fn return_track(&mut self, set: &LiveSet, id: u64, r: &SetReturn) {
        self.open("ReturnTrack", &[("Id", id.to_string())]);
        self.track_head(&r.name, &r.name, r.color, &[]);
        self.open("DeviceChain", &[]);
        self.chain_head(1, ("AudioIn/External/S0", "1/2"), ("AudioOut/Master", "Master", ""));
        self.open("Mixer", &[]);
        self.mixer_body(r.on, r.volume, r.pan, Some((&[], set.returns.len(), false)));
        self.close("Mixer");
        self.open("FreezeSequencer", &[]);
        self.open("AudioSequencer", &[("Id", "0".into())]);
        self.sample_sequencer_body(set, r.color, 0, 1, &[]);
        self.close("AudioSequencer");
        self.close("FreezeSequencer");
        self.inner_chain();
        self.close("DeviceChain");
        self.close("ReturnTrack");
    }
    /// The main track (tag MasterTrack in this schema) carries tempo and time signature.
    fn main_track(&mut self, set: &LiveSet) {
        // Ids are assigned in document order, but the envelopes come first and point at the
        // tempo and time signature targets further down: reserve those two ids up front.
        let tempo_id = self.id();
        let ts_id = self.id();
        let ts = time_signature_value(set.numerator, set.denominator);
        self.open("MasterTrack", &[]);
        self.track_head("Master", "", 69, &[(tempo_id, "FloatEvent", num(set.bpm)), (ts_id, "EnumEvent", ts.to_string())]);
        self.open("DeviceChain", &[]);
        self.chain_head(1, ("AudioIn/External/S0", "1/2"), ("AudioOut/External/S0", "Ext. Out", "1/2"));
        self.open("Mixer", &[]);
        self.mixer_body(true, 1.0, 0.0, None);
        self.open("Tempo", &[]);
        self.v("LomId", 0);
        self.v("Manual", num(set.bpm));
        self.open("MidiControllerRange", &[]);
        self.v("Min", 60);
        self.v("Max", 200);
        self.close("MidiControllerRange");
        self.open("AutomationTarget", &[("Id", tempo_id.to_string())]);
        self.v("LockEnvelope", 0);
        self.close("AutomationTarget");
        self.target("ModulationTarget");
        self.close("Tempo");
        self.open("TimeSignature", &[]);
        self.v("LomId", 0);
        self.v("Manual", ts);
        self.open("AutomationTarget", &[("Id", ts_id.to_string())]);
        self.v("LockEnvelope", 0);
        self.close("AutomationTarget");
        self.close("TimeSignature");
        self.float_param("GlobalGrooveAmount", 100.0, (0.0, 131.25), true);
        self.float_param("CrossFade", 0.0, (-1.0, 1.0), true);
        self.v("TempoAutomationViewBottom", 60);
        self.v("TempoAutomationViewTop", 200);
        self.close("Mixer");
        self.open("FreezeSequencer", &[]);
        self.open("AudioSequencer", &[("Id", "0".into())]);
        self.sample_sequencer_body(set, 69, 0, 1, &[]);
        self.close("AudioSequencer");
        self.close("FreezeSequencer");
        self.inner_chain();
        self.close("DeviceChain");
        self.close("MasterTrack");
    }
    fn prehear_track(&mut self) {
        self.open("PreHearTrack", &[]);
        self.track_head("Master", "", 0, &[]);
        self.open("DeviceChain", &[]);
        self.chain_head(1, ("AudioIn/External/S0", "1/2"), ("AudioOut/External/S0", "Ext. Out", "1/2"));
        self.open("Mixer", &[]);
        self.mixer_body(true, 1.0, 0.0, None);
        self.close("Mixer");
        self.inner_chain();
        self.close("DeviceChain");
        self.close("PreHearTrack");
    }
}

/// The set as Live's XML document (not yet gzipped).
pub fn to_xml(set: &LiveSet) -> String {
    let mut x = Xml { s: String::new(), depth: 0, next_id: 0 };
    x.s.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    x.open(
        "Ableton",
        &[
            ("MajorVersion", MAJOR_VERSION.into()),
            ("MinorVersion", MINOR_VERSION.into()),
            ("SchemaChangeCount", "3".into()),
            ("Creator", "Device".into()),
            ("Revision", String::new()),
        ],
    );
    x.open("LiveSet", &[]);
    // Filled in once every id is known.
    x.v("NextPointeeId", "@NEXT@");
    x.v("OverwriteProtectionNumber", 2819);
    x.v("LomId", 0);
    x.v("LomIdView", 0);
    x.open("Tracks", &[]);
    for (i, t) in set.tracks.iter().enumerate() {
        x.track(set, 10 + i as u64, t);
    }
    for (i, r) in set.returns.iter().enumerate() {
        x.return_track(set, 10 + (set.tracks.len() + i) as u64, r);
    }
    x.close("Tracks");
    x.main_track(set);
    x.prehear_track();
    if set.returns.is_empty() {
        x.empty("SendsPre", &[]);
    } else {
        x.open("SendsPre", &[]);
        for (i, r) in set.returns.iter().enumerate() {
            x.empty("SendPreBool", &[("Id", i.to_string()), ("Value", r.pre.to_string())]);
        }
        x.close("SendsPre");
    }
    x.open("Scenes", &[]);
    for i in 0..SCENES {
        x.open("Scene", &[("Id", i.to_string())]);
        x.follow_action(0);
        x.v("Name", "");
        x.v("Annotation", "");
        x.v("Color", -1);
        x.v("Tempo", num(set.bpm));
        x.v("IsTempoEnabled", false);
        x.v("TimeSignatureId", time_signature_value(set.numerator, set.denominator));
        x.v("IsTimeSignatureEnabled", false);
        x.v("LomId", 0);
        x.wrapper("ClipSlotsListWrapper");
        x.close("Scene");
    }
    x.close("Scenes");
    x.open("Transport", &[]);
    x.v("PhaseNudgeTempo", 10);
    x.v("LoopOn", set.loop_on);
    x.v("LoopStart", num(set.loop_start));
    x.v("LoopLength", num(set.loop_length.max(1.0)));
    x.v("LoopIsSongStart", false);
    x.v("CurrentTime", 0);
    x.v("PunchIn", false);
    x.v("PunchOut", false);
    x.v("MetronomeTickDuration", 0);
    x.v("DrawMode", false);
    x.close("Transport");
    x.open("SongMasterValues", &[]);
    x.empty("SessionScrollerPos", &[("X", "0".into()), ("Y", "0".into())]);
    x.close("SongMasterValues");
    x.empty("SignalModulations", &[]);
    x.v("GlobalQuantisation", 4);
    x.v("AutoQuantisation", 0);
    x.grid("Grid", true, false);
    x.open("ScaleInformation", &[]);
    x.v("RootNote", 0);
    x.v("Name", "Major");
    x.close("ScaleInformation");
    x.v("InKey", false);
    x.v("SmpteFormat", 0);
    x.open("TimeSelection", &[]);
    x.v("AnchorTime", 0);
    x.v("OtherTime", 0);
    x.close("TimeSelection");
    x.open("SequencerNavigator", &[]);
    x.open("BeatTimeHelper", &[]);
    x.v("CurrentZoom", "0.25");
    x.close("BeatTimeHelper");
    x.empty("ScrollerPos", &[("X", "0".into()), ("Y", "0".into())]);
    x.empty("ClientSize", &[("X", "800".into()), ("Y", "500".into())]);
    x.close("SequencerNavigator");
    x.v("IsContentSplitterOpen", true);
    x.v("IsExpressionSplitterOpen", false);
    for (tag, types) in [("ExpressionLanes", 0..4), ("ContentLanes", 4..6)] {
        x.open(tag, &[]);
        for (i, ty) in types.enumerate() {
            x.open("ExpressionLane", &[("Id", i.to_string())]);
            x.v("Type", ty);
            x.v("Size", 41);
            x.v("IsMinimized", ty < 4);
            x.close("ExpressionLane");
        }
        x.close(tag);
    }
    x.v("ViewStateFxSlotCount", 4);
    x.v("ViewStateSessionMixerHeight", 120);
    x.open("Locators", &[]);
    x.empty("Locators", &[]);
    x.close("Locators");
    x.empty("DetailClipKeyMidis", &[]);
    for w in ["TracksListWrapper", "VisibleTracksListWrapper", "ReturnTracksListWrapper", "ScenesListWrapper", "CuePointsListWrapper"] {
        x.wrapper(w);
    }
    x.v("ChooserBar", 0);
    x.v("Annotation", "");
    x.v("SoloOrPflSavedValue", true);
    x.v("SoloInPlace", true);
    x.v("CrossfadeCurve", 2);
    x.v("LatencyCompensation", 2);
    x.v("HighlightedTrackIndex", 0);
    x.open("GroovePool", &[]);
    x.v("LomId", 0);
    x.empty("Grooves", &[]);
    x.close("GroovePool");
    x.v("AutomationMode", false);
    x.v("SnapAutomationToGrid", true);
    x.v("ArrangementOverdub", false);
    x.v("ColorSequenceIndex", 0);
    x.open("AutoColorPickerForPlayerAndGroupTracks", &[]);
    x.v("NextColorIndex", 0);
    x.close("AutoColorPickerForPlayerAndGroupTracks");
    x.open("AutoColorPickerForReturnAndMasterTracks", &[]);
    x.v("NextColorIndex", 0);
    x.close("AutoColorPickerForReturnAndMasterTracks");
    x.v("ViewData", "{}");
    x.v("ResetNonautomatedMidiControllersOnClipStarts", true);
    x.v("MidiFoldIn", false);
    x.v("MidiFoldMode", 0);
    x.v("MultiClipFocusMode", false);
    x.v("MultiClipLoopBarHeight", 0);
    x.v("MidiPrelisten", false);
    x.empty("LinkedTrackGroups", &[]);
    x.v("AccidentalSpellingPreference", 3);
    x.v("PreferFlatRootNote", false);
    x.v("UseWarperLegacyHiQMode", false);
    let unset = (-2147483648i64).to_string();
    x.empty("VideoWindowRect", &[("Top", unset.clone()), ("Left", unset.clone()), ("Bottom", unset.clone()), ("Right", unset)]);
    x.v("ShowVideoWindow", true);
    x.v("TrackHeaderWidth", 93);
    x.v("ViewStateArrangerHasDetail", true);
    x.v("ViewStateSessionHasDetail", true);
    x.v("ViewStateDetailIsSample", false);
    x.open("ViewStates", &[]);
    for (k, v) in [
        ("SessionIO", 1),
        ("SessionSends", 1),
        ("SessionReturns", 1),
        ("SessionMixer", 1),
        ("SessionTrackDelay", 0),
        ("SessionCrossFade", 0),
        ("SessionShowOverView", 0),
        ("ArrangerIO", 1),
        ("ArrangerReturns", 1),
        ("ArrangerMixer", 1),
        ("ArrangerTrackDelay", 0),
        ("ArrangerShowOverView", 1),
    ] {
        x.v(k, v);
    }
    x.close("ViewStates");
    x.close("LiveSet");
    x.close("Ableton");
    let next = x.next_id + 1;
    x.s.replacen("\"@NEXT@\"", &format!("\"{next}\""), 1)
}

/// Gzip the document as Live stores it.
pub fn gzip(xml: &str) -> std::io::Result<Vec<u8>> {
    use std::io::Write;
    let mut e = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    e.write_all(xml.as_bytes())?;
    e.finish()
}

/// Elements every track of a kind must have, as paths below the track element.
const TRACK_PATHS: &[&str] = &[
    "Name/EffectiveName",
    "Name/UserName",
    "Color",
    "AutomationEnvelopes/Envelopes",
    "TrackGroupId",
    "DeviceChain/AutomationLanes/AutomationLanes",
    "DeviceChain/AudioInputRouting/Target",
    "DeviceChain/MidiInputRouting/Target",
    "DeviceChain/AudioOutputRouting/Target",
    "DeviceChain/MidiOutputRouting/Target",
    "DeviceChain/Mixer/On/Manual",
    "DeviceChain/Mixer/Speaker/Manual",
    "DeviceChain/Mixer/Pan/Manual",
    "DeviceChain/Mixer/Volume/Manual",
    "DeviceChain/Mixer/CrossFadeState/Manual",
    "DeviceChain/DeviceChain/Devices",
];
const PLAYER_PATHS: &[&str] = &[
    "DeviceChain/FreezeSequencer",
    "SavedPlayingSlot",
    "Freeze",
    "DeviceChain/MainSequencer/ClipSlotList",
    "DeviceChain/MainSequencer/Recorder/IsArmed",
    "DeviceChain/FreezeSequencer/ClipSlotList",
];
const AUDIO_PATHS: &[&str] =
    &["DeviceChain/MainSequencer/Sample/ArrangerAutomation/Events", "DeviceChain/MainSequencer/VolumeModulationTarget"];
const MIDI_PATHS: &[&str] =
    &["DeviceChain/MainSequencer/ClipTimeable/ArrangerAutomation/Events", "DeviceChain/MainSequencer/MidiControllers", "PitchbendRange"];
const MAIN_PATHS: &[&str] = &[
    "DeviceChain/Mixer/Tempo/Manual",
    "DeviceChain/Mixer/TimeSignature/Manual",
    "DeviceChain/Mixer/GlobalGrooveAmount",
    "DeviceChain/FreezeSequencer/AudioSequencer/Sample",
];
const CLIP_PATHS: &[&str] = &[
    "CurrentStart",
    "CurrentEnd",
    "Loop/LoopStart",
    "Loop/LoopEnd",
    "Loop/OutMarker",
    "Name",
    "Color",
    "TimeSignature/TimeSignatures/RemoteableTimeSignature/Numerator",
    "Envelopes/Envelopes",
    "FollowAction/FollowActionA",
    "Grid/FixedDenominator",
    "IsWarped",
    "TakeId",
];
const AUDIO_CLIP_PATHS: &[&str] = &[
    "SampleRef/FileRef/RelativePathType",
    "SampleRef/FileRef/RelativePath",
    "SampleRef/FileRef/Path",
    "SampleRef/DefaultDuration",
    "SampleRef/DefaultSampleRate",
    "WarpMode",
    "Fades/FadeInLength",
    "SampleVolume",
    "WarpMarkers",
];
const MIDI_CLIP_PATHS: &[&str] =
    &["Notes/KeyTracks", "Notes/PerNoteEventStore", "Notes/NoteIdGenerator/NextId", "ScaleInformation/RootNote", "ExpressionGrid"];
const RETURN_PATHS: &[&str] = &[
    "DeviceChain/Mixer/Volume/Manual",
    "DeviceChain/Mixer/Pan/Manual",
    "DeviceChain/Mixer/Speaker/Manual",
    "DeviceChain/AudioOutputRouting/Target",
    "DeviceChain/DeviceChain/Devices",
];
const SET_PATHS: &[&str] = &[
    "NextPointeeId",
    "OverwriteProtectionNumber",
    "Tracks",
    "MasterTrack",
    "PreHearTrack",
    "SendsPre",
    "Scenes",
    "Transport/LoopOn",
    "Transport/LoopStart",
    "Transport/LoopLength",
    "Grid",
    "Locators/Locators",
    "GroovePool/Grooves",
    "ViewStates",
];

/// What [validate] found in a valid set.
#[derive(Debug, PartialEq)]
pub struct SetSummary {
    pub audio_tracks: usize,
    pub midi_tracks: usize,
    pub audio_clips: usize,
    pub midi_clips: usize,
    pub notes: usize,
    pub return_tracks: usize,
    /// Sends above Live's minimum on ordinary tracks.
    pub sends: usize,
}

pub(crate) fn child<'a, 'i>(n: roxmltree::Node<'a, 'i>, tag: &str) -> Option<roxmltree::Node<'a, 'i>> {
    n.children().find(|c| c.has_tag_name(tag))
}

pub(crate) fn path<'a, 'i>(n: roxmltree::Node<'a, 'i>, p: &str) -> Option<roxmltree::Node<'a, 'i>> {
    p.split('/').try_fold(n, |n, t| child(n, t))
}

pub(crate) fn value<'a>(n: roxmltree::Node<'a, '_>, p: &str) -> Result<&'a str, String> {
    path(n, p).and_then(|e| e.attribute("Value")).ok_or_else(|| format!("{} is missing {p}", n.tag_name().name()))
}

pub(crate) fn number(n: roxmltree::Node, p: &str) -> Result<f64, String> {
    value(n, p)?.parse::<f64>().map_err(|e| format!("{p}: {e}"))
}

fn require(n: roxmltree::Node, paths: &[&str], what: &str) -> Result<(), String> {
    paths.iter().try_for_each(|p| path(n, p).map(|_| ()).ok_or_else(|| format!("{what} is missing {p}")))
}

/// Parse a written set back and check it against Live's structure (see the module docs).
/// "project_dir" resolves the samples' relative paths and must be marked as a Live project
/// ([mark_project]); None skips the file checks.
pub fn validate(xml: &str, project_dir: Option<&Path>) -> Result<SetSummary, String> {
    let doc = roxmltree::Document::parse(xml).map_err(|e| format!("not well-formed XML: {e}"))?;
    let root = doc.root_element();
    if !root.has_tag_name("Ableton") || root.attribute("MajorVersion") != Some(MAJOR_VERSION) || root.attribute("MinorVersion").is_none() {
        return Err("root is not an Ableton schema 5 document".into());
    }
    let set = child(root, "LiveSet").ok_or("no LiveSet")?;
    require(set, SET_PATHS, "LiveSet")?;
    if let Some(dir) = project_dir
        && !dir.join(PROJECT_INFO_DIR).is_dir()
    {
        return Err(format!("the folder has no \"{PROJECT_INFO_DIR}\", so Live would show every sample offline"));
    }
    let scenes = path(set, "Scenes").map_or(0, |s| s.children().filter(|c| c.has_tag_name("Scene")).count());
    if scenes == 0 {
        return Err("no scenes".into());
    }
    let mut sum = SetSummary { audio_tracks: 0, midi_tracks: 0, audio_clips: 0, midi_clips: 0, notes: 0, return_tracks: 0, sends: 0 };
    let mut track_ids = std::collections::HashSet::new();
    let tracks = path(set, "Tracks").ok_or("no Tracks")?;
    let returns = tracks.children().filter(|c| c.has_tag_name("ReturnTrack")).count();
    let pre = path(set, "SendsPre").map_or(0, |s| s.children().filter(|c| c.has_tag_name("SendPreBool")).count());
    if pre != returns {
        return Err(format!("{pre} SendPreBool entries for {returns} return tracks"));
    }
    for t in tracks.children().filter(|c| c.is_element()) {
        let kind = t.tag_name().name();
        let what = format!("{kind} {}", t.attribute("Id").unwrap_or("?"));
        if !track_ids.insert(t.attribute("Id").ok_or(format!("{what} has no Id"))?.to_string()) {
            return Err(format!("{what}: duplicate track id"));
        }
        require(t, TRACK_PATHS, &what)?;
        // One send per return on every track, in Live's range.
        let holders: Vec<_> =
            path(t, "DeviceChain/Mixer/Sends").map_or(Vec::new(), |s| s.children().filter(|c| c.has_tag_name("TrackSendHolder")).collect());
        if holders.len() != returns {
            return Err(format!("{what}: {} sends for {returns} return tracks", holders.len()));
        }
        for (i, h) in holders.iter().enumerate() {
            let level = number(*h, "Send/Manual")?;
            let active = value(*h, "Active")?;
            if h.attribute("Id") != Some(i.to_string().as_str())
                || !(SEND_RANGE.0..=SEND_RANGE.1).contains(&level)
                || path(*h, "Send/AutomationTarget").is_none()
            {
                return Err(format!("{what}: bad send {i} (level {level})"));
            }
            if kind != "ReturnTrack" && active == "true" && level > SEND_RANGE.0 {
                sum.sends += 1;
            }
        }
        if kind == "ReturnTrack" {
            sum.return_tracks += 1;
            require(t, RETURN_PATHS, &what)?;
            if path(t, "DeviceChain/MainSequencer").is_some() {
                return Err(format!("{what}: a return track with clips"));
            }
            continue;
        }
        if sum.return_tracks > 0 {
            return Err(format!("{what} comes after a return track"));
        }
        require(t, PLAYER_PATHS, &what)?;
        let (events, clip_tag, extra) = match kind {
            "AudioTrack" => {
                sum.audio_tracks += 1;
                require(t, AUDIO_PATHS, &what)?;
                ("DeviceChain/MainSequencer/Sample/ArrangerAutomation/Events", "AudioClip", AUDIO_CLIP_PATHS)
            }
            "MidiTrack" => {
                sum.midi_tracks += 1;
                require(t, MIDI_PATHS, &what)?;
                ("DeviceChain/MainSequencer/ClipTimeable/ArrangerAutomation/Events", "MidiClip", MIDI_CLIP_PATHS)
            }
            other => return Err(format!("unexpected track kind {other}")),
        };
        for seq in ["MainSequencer", "FreezeSequencer"] {
            let slots = path(t, &format!("DeviceChain/{seq}/ClipSlotList"))
                .map_or(0, |l| l.children().filter(|c| c.has_tag_name("ClipSlot")).count());
            if slots != scenes {
                return Err(format!("{what}: {seq} has {slots} clip slots for {scenes} scenes"));
            }
        }
        let mut last_end = f64::NEG_INFINITY;
        for c in path(t, events).ok_or("events")?.children().filter(|c| c.is_element()) {
            if !c.has_tag_name(clip_tag) {
                return Err(format!("{what}: {} in its arrangement", c.tag_name().name()));
            }
            let cw = format!("{what} {clip_tag} {}", c.attribute("Id").unwrap_or("?"));
            require(c, CLIP_PATHS, &cw)?;
            require(c, extra, &cw)?;
            let (start, end) = (number(c, "CurrentStart")?, number(c, "CurrentEnd")?);
            let time: f64 = c.attribute("Time").ok_or(format!("{cw} has no Time"))?.parse().map_err(|e| format!("{cw} Time: {e}"))?;
            if end <= start || (time - start).abs() > 1e-9 || start < last_end - 1e-9 {
                return Err(format!("{cw}: bad span {start}..{end} at {time} (previous clip ends at {last_end})"));
            }
            last_end = end;
            let (loop_start, loop_end) = (number(c, "Loop/LoopStart")?, number(c, "Loop/LoopEnd")?);
            if (loop_end - loop_start - (end - start)).abs() > 1e-6 {
                return Err(format!("{cw}: loop {loop_start}..{loop_end} does not match the clip length"));
            }
            if clip_tag == "AudioClip" {
                sum.audio_clips += 1;
                if value(c, "SampleRef/FileRef/RelativePathType")? != "3" {
                    return Err(format!("{cw}: sample path is not project-relative"));
                }
                let markers = path(c, "WarpMarkers").map_or(0, |m| m.children().filter(|x| x.has_tag_name("WarpMarker")).count());
                if markers < 2 {
                    return Err(format!("{cw}: {markers} warp markers"));
                }
                if number(c, "SampleRef/DefaultDuration")? <= 0.0 || number(c, "SampleRef/DefaultSampleRate")? <= 0.0 {
                    return Err(format!("{cw}: empty sample"));
                }
                if let Some(dir) = project_dir {
                    let rel = value(c, "SampleRef/FileRef/RelativePath")?;
                    if !dir.join(rel).is_file() {
                        return Err(format!("{cw}: sample {rel} is missing"));
                    }
                }
            } else {
                sum.midi_clips += 1;
                let mut ids = std::collections::HashSet::new();
                let mut max_id = 0u64;
                for k in path(c, "Notes/KeyTracks").ok_or("keytracks")?.children().filter(|k| k.has_tag_name("KeyTrack")) {
                    let key = number(k, "MidiKey")?;
                    if !(0.0..=127.0).contains(&key) {
                        return Err(format!("{cw}: key {key}"));
                    }
                    for n in child(k, "Notes")
                        .ok_or(format!("{cw}: KeyTrack without Notes"))?
                        .children()
                        .filter(|n| n.has_tag_name("MidiNoteEvent"))
                    {
                        let attr = |a: &str| {
                            n.attribute(a)
                                .ok_or(format!("{cw}: note without {a}"))
                                .and_then(|v| v.parse::<f64>().map_err(|e| format!("{cw} {a}: {e}")))
                        };
                        let (t0, d, vel, id) = (attr("Time")?, attr("Duration")?, attr("Velocity")?, attr("NoteId")?);
                        for a in ["VelocityDeviation", "OffVelocity", "Probability", "IsEnabled"] {
                            n.attribute(a).ok_or(format!("{cw}: note without {a}"))?;
                        }
                        if t0 < 0.0 || d <= 0.0 || t0 >= end - start + 1e-9 || !(1.0..=127.0).contains(&vel) || !ids.insert(id as u64) {
                            return Err(format!("{cw}: bad note at {t0} length {d} velocity {vel} id {id}"));
                        }
                        max_id = max_id.max(id as u64);
                        sum.notes += 1;
                    }
                }
                if number(c, "Notes/NoteIdGenerator/NextId")? as u64 <= max_id {
                    return Err(format!("{cw}: NextId is not above the note ids"));
                }
            }
        }
    }
    let main = path(set, "MasterTrack").ok_or("no MasterTrack")?;
    require(main, TRACK_PATHS, "MasterTrack")?;
    require(main, MAIN_PATHS, "MasterTrack")?;
    require(path(set, "PreHearTrack").ok_or("no PreHearTrack")?, TRACK_PATHS, "PreHearTrack")?;
    let tempo = number(main, "DeviceChain/Mixer/Tempo/Manual")?;
    if !(20.0..=999.0).contains(&tempo) {
        return Err(format!("tempo {tempo} is outside Live's range"));
    }
    // Pointee ids: unique, and below NextPointeeId.
    let mut ids = std::collections::HashSet::new();
    let mut max = 0u64;
    for n in set.descendants().filter(|n| n.is_element()) {
        let tag = n.tag_name().name();
        if tag == "AutomationTarget"
            || tag == "ModulationTarget"
            || tag == "Pointee"
            || tag.ends_with("ModulationTarget")
            || tag.starts_with("ControllerTargets.")
        {
            let id: u64 = n.attribute("Id").ok_or(format!("{tag} without Id"))?.parse().map_err(|e| format!("{tag} Id: {e}"))?;
            if !ids.insert(id) {
                return Err(format!("pointee id {id} is used twice"));
            }
            max = max.max(id);
        }
    }
    if number(set, "NextPointeeId")? as u64 <= max {
        return Err("NextPointeeId is not above every pointee id".into());
    }
    // The tempo and time signature envelopes point at the main track's targets.
    let target_id = |p: &str| path(main, p).and_then(|n| n.attribute("Id")).map(str::to_string);
    let (tempo_target, ts_target) =
        (target_id("DeviceChain/Mixer/Tempo/AutomationTarget"), target_id("DeviceChain/Mixer/TimeSignature/AutomationTarget"));
    let pointees: Vec<String> = path(main, "AutomationEnvelopes/Envelopes")
        .map(|e| {
            e.children()
                .filter_map(|a| path(a, "EnvelopeTarget/PointeeId").and_then(|p| p.attribute("Value")).map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    if tempo_target.is_none_or(|t| !pointees.contains(&t)) || ts_target.is_none_or(|t| !pointees.contains(&t)) {
        return Err("the main track's tempo and time signature envelopes are missing".into());
    }
    Ok(sum)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_set() -> LiveSet {
        LiveSet {
            bpm: 120.0,
            numerator: 4,
            denominator: 4,
            loop_on: true,
            loop_start: 0.0,
            loop_length: 16.0,
            tracks: vec![
                SetTrack {
                    name: "Bass & <Drums>".into(),
                    color: 3,
                    on: true,
                    volume: 1.0,
                    pan: 0.0,
                    sends: vec![0.5, 3.0],
                    sends_only: false,
                    content: SetContent::Audio(vec![SetAudioClip {
                        name: "01 Bass".into(),
                        start: 0.0,
                        length: 28.0,
                        relative_path: "Samples/Processed/DStudio/01 Bass.wav".into(),
                        absolute_path: "/tmp/x/Samples/Processed/DStudio/01 Bass.wav".into(),
                        file_size: 1000,
                        frames: 672_000,
                        sample_rate: 48_000,
                        markers: Vec::new(),
                    }]),
                },
                SetTrack {
                    name: "Keys (MIDI)".into(),
                    color: 9,
                    on: false,
                    volume: 0.5,
                    pan: -0.25,
                    sends: vec![0.0, 0.25],
                    sends_only: true,
                    content: SetContent::Midi(vec![SetMidiClip {
                        name: "MIDI".into(),
                        start: 4.0,
                        length: 4.0,
                        notes: vec![
                            SetNote { pitch: 60, time: 0.0, duration: 1.0, velocity: 100 },
                            SetNote { pitch: 64, time: 1.0, duration: 0.5, velocity: 90 },
                            SetNote { pitch: 60, time: 2.0, duration: 1.0, velocity: 80 },
                        ],
                    }]),
                },
                SetTrack {
                    name: "Empty".into(),
                    color: 0,
                    on: true,
                    volume: 1.0,
                    pan: 0.0,
                    sends: Vec::new(),
                    sends_only: false,
                    content: SetContent::Midi(vec![]),
                },
            ],
            returns: vec![
                SetReturn { name: "Return A".into(), color: 5, on: true, volume: 0.8, pan: 0.1, pre: false },
                SetReturn { name: "Return B".into(), color: 6, on: false, volume: 1.0, pan: 0.0, pre: true },
            ],
        }
    }

    #[test]
    fn time_signature_values_match_live() {
        assert_eq!(time_signature_value(4, 4), 201);
        assert_eq!(time_signature_value(3, 4), 200);
        assert_eq!(time_signature_value(6, 8), 302);
        assert_eq!(time_signature_value(7, 16), 402);
    }

    #[test]
    fn written_set_passes_the_structural_check() {
        let xml = to_xml(&sample_set());
        let sum = validate(&xml, None).unwrap();
        assert_eq!(
            sum,
            SetSummary { audio_tracks: 1, midi_tracks: 2, audio_clips: 1, midi_clips: 1, notes: 3, return_tracks: 2, sends: 3 }
        );
        assert!(xml.contains("<EffectiveName Value=\"Bass &amp; &lt;Drums&gt;\" />"));
        let gz = gzip(&xml).unwrap();
        assert_eq!(&gz[..2], &[0x1f, 0x8b]);
    }

    #[test]
    fn returns_and_sends_are_written_the_way_live_saves_them() {
        let xml = to_xml(&sample_set());
        let doc = roxmltree::Document::parse(&xml).unwrap();
        let set = child(doc.root_element(), "LiveSet").unwrap();
        let tracks: Vec<_> = child(set, "Tracks").unwrap().children().filter(|c| c.is_element()).collect();
        // Returns come last, after every ordinary track, with ids of their own.
        assert_eq!(
            tracks.iter().map(|t| t.tag_name().name()).collect::<Vec<_>>(),
            ["AudioTrack", "MidiTrack", "MidiTrack", "ReturnTrack", "ReturnTrack"]
        );
        assert_eq!(value(tracks[3], "Name/EffectiveName").unwrap(), "Return A");
        assert_eq!(number(tracks[3], "DeviceChain/Mixer/Volume/Manual").unwrap() as f32, 0.8);
        assert_eq!(value(tracks[4], "DeviceChain/Mixer/Speaker/Manual").unwrap(), "false");
        let sends = |t: roxmltree::Node| -> Vec<(f64, String)> {
            path(t, "DeviceChain/Mixer/Sends")
                .unwrap()
                .children()
                .filter(|c| c.is_element())
                .map(|h| (number(h, "Send/Manual").unwrap(), value(h, "Active").unwrap().to_string()))
                .collect()
        };
        // 0.5 as is, +9.5 dB clamped to Live's 0 dB top; an unset send sits at Live's minimum.
        assert_eq!(sends(tracks[0]), [(0.5, "true".into()), (1.0, "true".into())]);
        assert_eq!(sends(tracks[1]), [(SEND_RANGE.0, "true".into()), (0.25, "true".into())]);
        assert_eq!(sends(tracks[2]), [(SEND_RANGE.0, "true".into()), (SEND_RANGE.0, "true".into())]);
        // A return's own sends exist but are inactive.
        assert_eq!(sends(tracks[3]), [(SEND_RANGE.0, "false".into()), (SEND_RANGE.0, "false".into())]);
        // "Sends Only" output, and the per-return pre/post switches.
        assert_eq!(value(tracks[0], "DeviceChain/AudioOutputRouting/Target").unwrap(), "AudioOut/Master");
        assert_eq!(value(tracks[1], "DeviceChain/AudioOutputRouting/Target").unwrap(), "AudioOut/None");
        assert_eq!(value(tracks[1], "DeviceChain/AudioOutputRouting/UpperDisplayString").unwrap(), "Sends Only");
        let pre: Vec<_> =
            child(set, "SendsPre").unwrap().children().filter(|c| c.is_element()).map(|c| c.attribute("Value").unwrap()).collect();
        assert_eq!(pre, ["false", "true"]);
        // The main and pre-hear tracks have no sends.
        assert!(path(set, "MasterTrack/DeviceChain/Mixer/Sends").unwrap().children().all(|c| !c.is_element()));
    }

    #[test]
    fn a_set_without_returns_has_no_sends() {
        let mut s = sample_set();
        s.returns.clear();
        let xml = to_xml(&s);
        let sum = validate(&xml, None).unwrap();
        assert_eq!((sum.return_tracks, sum.sends), (0, 0));
        assert!(xml.contains("<SendsPre />") && !xml.contains("TrackSendHolder"));
    }

    #[test]
    fn the_check_catches_broken_sets() {
        let xml = to_xml(&sample_set());
        let broken = [
            xml.replacen("<PitchbendRange", "<PitchBendRange", 1),
            xml.replacen("<CurrentEnd Value=\"28\"", "<CurrentEnd Value=\"0\"", 1),
            xml.replacen("NoteId=\"2\"", "NoteId=\"1\"", 1),
            xml.replacen("<Scene Id=\"7\">", "<Scene Id=\"7\" />\n<Scene Id=\"8\">", 1),
            // A send too many, a missing pre/post switch, a send above 0 dB.
            xml.replacen("<TrackSendHolder Id=\"1\">", "<TrackSendHolder Id=\"2\">", 1),
            xml.replacen("<SendPreBool Id=\"1\" Value=\"true\" />", "", 1),
            xml.replacen("<Manual Value=\"0.5\" />", "<Manual Value=\"1.5\" />", 1),
        ];
        for (i, b) in broken.iter().enumerate() {
            assert_ne!(b, &xml, "case {i} changed nothing");
            assert!(validate(b, None).is_err(), "case {i} passed");
        }
        assert!(validate(&xml, Some(Path::new("/nonexistent"))).is_err(), "missing samples are caught");
    }

    #[test]
    fn samples_resolve_only_inside_a_marked_project() {
        // Live 12 shows project-relative samples offline unless the set's folder holds
        // "Ableton Project Info" (found by opening an export in Live).
        let dir = std::env::temp_dir().join(format!("dstudio-live-project-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut s = sample_set();
        let SetContent::Audio(clips) = &mut s.tracks[0].content else { unreachable!() };
        let wav = dir.join(&clips[0].relative_path);
        std::fs::create_dir_all(wav.parent().unwrap()).unwrap();
        std::fs::write(&wav, [0u8; 44]).unwrap();
        let xml = to_xml(&s);
        let err = validate(&xml, Some(&dir)).unwrap_err();
        assert!(err.contains(PROJECT_INFO_DIR), "{err}");
        mark_project(&dir).unwrap();
        assert!(validate(&xml, Some(&dir)).is_ok());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
