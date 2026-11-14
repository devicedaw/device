//! Authoritative project model. Lives on the application thread only; the
//! audio engine receives derived, immutable snapshots (see `snapshot`).

use crate::automation::AutomationLane;
use crate::device::{Device, DeviceId};
use crate::instrument::SynthParams;
use crate::midi::MidiClip;
use crate::warp::ClipWarp;
use serde::{Deserialize, Serialize};

/// Pulses per quarter note for musical positions.
pub const PPQ: i64 = 960;

/// Musical position/duration in ticks (PPQ = 960).
pub type Ticks = i64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TrackId(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ClipId(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AssetId(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SectionId(pub u64);

/// A named, coloured arrangement range. Sections tile the
/// timeline from bar 1 in list order: a section starts where the previous one ends.
/// Labels only: editing sections never moves clips.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Section {
    pub id: SectionId,
    pub name: String,
    /// 0xRRGGBB (the section palette, before display tinting).
    pub color: u32,
    pub length: Ticks,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrackKind {
    Audio,
    Instrument,
    /// A return track: no clips or input; it plays what other tracks send to it
    /// ([Track::sends], [Track::output]). Return tracks come after every other track.
    Return,
    /// A group (folder) track: no clips or input; it plays the tracks whose [Track::output] is
    /// the group, through its own inserts, volume and pan, into the master. Its tracks follow it
    /// in the track list. Groups sit among the ordinary tracks and don't nest.
    Group,
}

/// One send of a track into a return track (spec 002).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct TrackSend {
    /// The return track it feeds.
    pub target: TrackId,
    /// Linear gain, 0 ..= [TrackSend::MAX_LEVEL]. 1.0 = 0 dB.
    pub level: f32,
    /// Tap the track after its insert chain, before volume and pan (else after them).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub pre_fader: bool,
}

impl TrackSend {
    /// +6 dB.
    pub const MAX_LEVEL: f32 = 1.995_262_3;

    /// A new send: 0 dB, post-fader.
    pub fn new(target: TrackId) -> Self {
        TrackSend { target, level: 1.0, pre_fader: false }
    }
}

/// Track colour: an index into the
/// UI palette; the model stores an index so palettes can be re-themed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ColorIndex(pub u8);

impl ColorIndex {
    /// Colour of a new project's first track: pink (palette index 4). Later
    /// tracks follow the cycle from there (orange, then teal, ...).
    pub const FIRST: ColorIndex = ColorIndex(4);
}

/// Which device input feeds a track (the "In" selector: "None",
/// "Input 1 (Mono)", ...). Channels are 0-based device input channels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct InputSource {
    /// First (left or mono) device input channel, 0-based.
    pub channel: u16,
    /// Use `channel` and `channel + 1` as a stereo pair.
    pub stereo: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Track {
    pub id: TrackId,
    pub kind: TrackKind,
    pub name: String,
    pub color: ColorIndex,
    /// Linear gain. 1.0 = 0 dB.
    pub gain: f32,
    /// -1.0 (left) .. +1.0 (right).
    pub pan: f32,
    /// Channel-strip trim in dB ([Track::TRIM_RANGE_DB]), applied after the inserts and before
    /// pre-fader sends and the fader (spec 005).
    #[serde(default, rename = "trim", skip_serializing_if = "is_zero")]
    pub trim_db: f32,
    /// Stereo width ([Track::WIDTH_RANGE]): 0 = mono, 1 = unchanged, 2 = side doubled.
    #[serde(default = "unit_width", skip_serializing_if = "is_unit_width")]
    pub width: f32,
    /// Fold the track to mono (overrides the width).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub mono: bool,
    pub mute: bool,
    pub solo: bool,
    /// Armed for recording ("R").
    #[serde(default)]
    pub arm: bool,
    /// Input monitoring on ("I"): the track's input is heard
    /// through its chain while armed or monitoring.
    #[serde(default)]
    pub monitor: bool,
    /// Input routing; None = "None".
    #[serde(default)]
    pub input: Option<InputSource>,
    pub clips: Vec<Clip>,
    /// MIDI clips (instrument tracks only). Defaulted so schema-v1 files
    /// without the field still load.
    #[serde(default)]
    pub midi_clips: Vec<MidiClip>,
    /// Built-in instrument settings (used by instrument tracks; defaulted for
    /// older files and harmlessly present on audio tracks).
    #[serde(default)]
    pub synth: SynthParams,
    /// Insert effects, processed in order after the track's source.
    #[serde(default)]
    pub devices: Vec<Device>,
    /// Instrument plugin replacing the built-in synth (instrument tracks).
    #[serde(default)]
    pub instrument_plugin: Option<Device>,
    /// Automation lanes (at most one per target).
    #[serde(default)]
    pub automation: Vec<AutomationLane>,
    /// Row height as a multiple of the default 88 px row ([Track::HEIGHT_RANGE]).
    #[serde(default = "unit_height", skip_serializing_if = "is_unit_height")]
    pub height: f32,
    /// Sends into return tracks, at most one per return (none on return tracks).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sends: Vec<TrackSend>,
    /// Where the track's output goes: a return track, or the master (None). Return tracks
    /// always go to the master.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<TrackId>,
}

fn unit_height() -> f32 {
    1.0
}

fn is_zero(v: &f32) -> bool {
    *v == 0.0
}

fn unit_width() -> f32 {
    1.0
}

fn is_unit_width(w: &f32) -> bool {
    *w == 1.0
}

fn is_unit_height(h: &f32) -> bool {
    *h == 1.0
}

/// Where a device sits on its track.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceSlot {
    Insert(usize),
    Instrument,
}

/// An audio clip referencing a media asset.
///
/// Timeline placement is musical (`start`, ticks) so clips follow tempo
/// changes; the played source region is in source frames at the asset's
/// native sample rate. With warp on, the region follows the project tempo
/// (see [`crate::warp`]); with warp off it plays 1:1.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Clip {
    pub id: ClipId,
    pub asset: AssetId,
    pub name: String,
    pub start: Ticks,
    pub source_offset: u64,
    pub source_length: u64,
    pub gain: f32,
    #[serde(default, skip_serializing_if = "ClipWarp::is_default")]
    pub warp: ClipWarp,
    /// Disabled clips stay on the timeline but make no sound (the Disable menu entry).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub disabled: bool,
    /// Clip looping (the editor's Loop switch). Clips saved before it existed load without it.
    #[serde(default, rename = "loop", skip_serializing_if = "Option::is_none")]
    pub clip_loop: Option<AudioLoop>,
    /// Fade-in and fade-out (the handles at the clip's top corners). Clips saved before
    /// fades existed load without them.
    #[serde(default, skip_serializing_if = "ClipFades::is_default")]
    pub fades: ClipFades,
    /// Takes from a looped recording: each loop pass is one take of the same recorded file.
    /// Each entry is that take's start in the asset, in frames, relative to `source_offset`,
    /// so the take that plays is the one at 0 and trims and splits keep the others in step.
    /// Empty for clips that are not a looped take.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub takes: Vec<i64>,
}

/// An audio clip's fades: lengths on the timeline, in ticks, from the clip's start (fade-in)
/// and back from its end (fade-out). The gain ramps linearly from silence to the clip's gain
/// over the fade-in and back to silence over the fade-out. Lengths beyond the clip are kept
/// and clamped where they are used ([ClipFades::clamped]), so trimming a clip shorter and
/// back again keeps its fades.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClipFades {
    #[serde(default, rename = "in")]
    pub fade_in: Ticks,
    #[serde(default, rename = "out")]
    pub fade_out: Ticks,
}

impl ClipFades {
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }

    /// The fades of a clip `len` ticks long: each within the clip, the fade-out shortened
    /// so the two don't overlap.
    pub fn clamped(self, len: Ticks) -> Self {
        let len = len.max(0);
        let fade_in = self.fade_in.clamp(0, len);
        Self { fade_in, fade_out: self.fade_out.clamp(0, len - fade_in) }
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.fade_in < 0 || self.fade_out < 0 {
            Err(format!("fades {} / {} must not be negative", self.fade_in, self.fade_out))
        } else {
            Ok(())
        }
    }
}

impl Clip {
    /// The take that plays (the entry at 0), if the clip has takes.
    pub fn current_take(&self) -> Option<usize> {
        self.takes.iter().position(|&d| d == 0)
    }

    /// The takes with take `take` playing: (new source offset, new takes), or why not.
    pub fn with_take(&self, take: usize, asset_frames: u64) -> Result<(u64, Vec<i64>), String> {
        let &d = self.takes.get(take).ok_or_else(|| format!("clip {} has no take {}", self.id.0, take + 1))?;
        let offset = self.source_offset as i64 + d;
        if offset < 0 || offset as u64 + self.source_length > asset_frames {
            return Err(format!("take {} is outside the recording", take + 1));
        }
        Ok((offset as u64, self.takes.iter().map(|&x| x - d).collect()))
    }
}

/// An audio clip's loop, like a MIDI clip's: the range of the clip's content that repeats,
/// in ticks from the clip start, where content is what the clip plays without the loop. The
/// clip plays from its start to the loop end, then repeats the range until it ends. The range
/// (and the looped length) is kept while looping is off.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct AudioLoop {
    pub on: bool,
    pub start: Ticks,
    pub end: Ticks,
    /// The clip's length on the timeline while it loops.
    pub length: Ticks,
}

impl AudioLoop {
    pub fn validate(&self) -> Result<(), String> {
        if self.start < 0 || self.end - self.start < crate::midi::ClipLoop::MIN_LEN || self.length <= 0 {
            Err(format!(
                "loop {}..{} (length {}) is shorter than {} ticks or before the clip",
                self.start,
                self.end,
                self.length,
                crate::midi::ClipLoop::MIN_LEN
            ))
        } else {
            Ok(())
        }
    }
}

/// External media reference. Media is never copied into the project unless
/// the user chooses "collect"; paths are stored both relative to the project
/// directory (portable) and absolute (fallback).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Asset {
    pub id: AssetId,
    pub relative_path: Option<String>,
    pub absolute_path: String,
    pub sample_rate: f64,
    pub channels: u32,
    pub frames: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct TimeSignature {
    pub numerator: u8,
    pub denominator: u8,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Transport {
    pub bpm: f64,
    pub time_signature: TimeSignature,
    pub loop_enabled: bool,
    pub loop_start: Ticks,
    pub loop_end: Ticks,
    /// Project key (None = not set, shown as "—"). Devices whose Key
    /// Follow is on use it (builtin::engine_device).
    #[serde(default)]
    pub key: Option<ProjectKey>,
}

/// A project key: tonic pitch class and mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectKey {
    /// Pitch class of the tonic, 0 = C .. 11 = B.
    pub root: u8,
    pub minor: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Project {
    pub name: String,
    pub transport: Transport,
    pub master_gain: f32,
    /// The master strip's M: the whole mix is silent (the fader keeps its value).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub master_mute: bool,
    pub tracks: Vec<Track>,
    pub assets: Vec<Asset>,
    /// Monotonic id allocator shared by tracks, clips and assets.
    pub next_id: u64,
    /// Master bus automation (volume only).
    #[serde(default)]
    pub master_automation: Vec<AutomationLane>,
    /// Bottom panel height (window points) per panel kind ("devices", "clip", "mixer") after the
    /// user dragged the splitter; absent kinds use the default height. Not an undo step.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub bottom_panel_heights: std::collections::BTreeMap<String, f32>,
    /// Arrangement sections in timeline order (see [Section]).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sections: Vec<Section>,
}

impl Default for Project {
    /// The default project: one Audio and one Instrument
    /// track, 120 BPM, 4/4, loop region bars 1–9.
    fn default() -> Self {
        let mut p = Project {
            name: "Untitled".into(),
            transport: Transport {
                bpm: 120.0,
                time_signature: TimeSignature { numerator: 4, denominator: 4 },
                loop_enabled: false,
                loop_start: 0,
                loop_end: 8 * 4 * PPQ,
                key: None,
            },
            master_gain: 1.0,
            master_mute: false,
            tracks: Vec::new(),
            assets: Vec::new(),
            next_id: 1,
            master_automation: Vec::new(),
            bottom_panel_heights: Default::default(),
            sections: Vec::new(),
        };
        let a = p.alloc_id();
        p.tracks.push(Track::new(TrackId(a), TrackKind::Audio, "Audio", ColorIndex::FIRST));
        let b = p.alloc_id();
        p.tracks.push(Track::new(TrackId(b), TrackKind::Instrument, "Instrument", ColorIndex(ColorIndex::FIRST.0 + 1)));
        crate::builtin::synth_v2::upgrade_project(&mut p);
        p
    }
}

impl Track {
    pub fn new(id: TrackId, kind: TrackKind, name: &str, color: ColorIndex) -> Self {
        Track {
            id,
            kind,
            name: name.into(),
            color,
            gain: 1.0,
            pan: 0.0,
            trim_db: 0.0,
            width: 1.0,
            mono: false,
            mute: false,
            solo: false,
            arm: false,
            monitor: false,
            input: None,
            clips: Vec::new(),
            midi_clips: Vec::new(),
            synth: SynthParams::default(),
            devices: Vec::new(),
            instrument_plugin: None,
            automation: Vec::new(),
            height: 1.0,
            sends: Vec::new(),
            output: None,
        }
    }

    /// Row height scale limits (the per-track height scale).
    pub const HEIGHT_RANGE: (f32, f32) = (0.25, 6.0);

    /// Channel-strip trim limits in dB.
    pub const TRIM_RANGE_DB: (f32, f32) = (-24.0, 24.0);

    /// Stereo width limits (1 = unchanged).
    pub const WIDTH_RANGE: (f32, f32) = (0.0, 2.0);

    /// Linear trim gain.
    pub fn trim_gain(&self) -> f32 {
        10f32.powf(self.trim_db / 20.0)
    }

    /// True when trim, width and mono leave the signal unchanged.
    pub fn strip_is_neutral(&self) -> bool {
        self.trim_db == 0.0 && self.width == 1.0 && !self.mono
    }

    pub fn is_return(&self) -> bool {
        self.kind == TrackKind::Return
    }

    pub fn is_group(&self) -> bool {
        self.kind == TrackKind::Group
    }

    /// Return and group tracks: a bus of what other tracks play into it, with no source of its own.
    pub fn is_bus(&self) -> bool {
        matches!(self.kind, TrackKind::Return | TrackKind::Group)
    }
}

/// Most return tracks a project can have (the engine's SE_MAX_RETURNS).
pub const MAX_RETURNS: usize = 16;

/// The letter of the return track at `index` among the returns: A, B, ... P.
pub fn return_letter(index: usize) -> char {
    (b'A' + (index % 26) as u8) as char
}

impl Project {
    pub fn empty(name: &str) -> Self {
        let mut p = Project { name: name.into(), ..Default::default() };
        p.tracks.clear();
        p
    }

    /// A copy of track `id` for Duplicate track: the same name, colour, mix settings, devices,
    /// automation and sends, with fresh ids for the track, its devices and (with `with_clips`)
    /// its clips, so the copy can be added next to the original with `Command::AddTrack`. The
    /// copy is not armed or soloed, and links, chains and automation lanes that name one of the
    /// original's devices name the copy's device instead. None when there is no such track.
    pub fn track_copy(&self, id: TrackId, with_clips: bool) -> Option<Track> {
        use crate::automation::AutomationTarget;
        let src = self.track(id)?;
        let mut next = self.next_id;
        let mut fresh = || {
            let id = next;
            next += 1;
            id
        };
        let mut t = src.clone();
        t.id = TrackId(fresh());
        t.arm = false;
        t.solo = false;
        if with_clips {
            for c in &mut t.clips {
                c.id = ClipId(fresh());
            }
            for c in &mut t.midi_clips {
                c.id = ClipId(fresh());
            }
        } else {
            t.clips.clear();
            t.midi_clips.clear();
        }
        let mut map = std::collections::HashMap::new();
        for d in t.devices.iter_mut().chain(t.instrument_plugin.as_mut()) {
            let new = DeviceId(fresh());
            map.insert(d.id, new);
            d.id = new;
        }
        let remap = |d: &mut DeviceId| {
            if let Some(n) = map.get(d) {
                *d = *n;
            }
        };
        for d in t.devices.iter_mut().chain(t.instrument_plugin.as_mut()) {
            if let Some(c) = d.chain.as_mut() {
                remap(c);
            }
            for l in &mut d.links {
                remap(&mut l.target);
            }
        }
        for lane in &mut t.automation {
            match &mut lane.target {
                AutomationTarget::Plugin { device, .. }
                | AutomationTarget::Builtin { device, .. }
                | AutomationTarget::DeviceOn { device, .. } => remap(device),
                _ => {}
            }
        }
        Some(t)
    }

    pub fn alloc_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    pub fn track(&self, id: TrackId) -> Option<&Track> {
        self.tracks.iter().find(|t| t.id == id)
    }

    pub fn track_mut(&mut self, id: TrackId) -> Option<&mut Track> {
        self.tracks.iter_mut().find(|t| t.id == id)
    }

    pub fn section_index(&self, id: SectionId) -> Option<usize> {
        self.sections.iter().position(|s| s.id == id)
    }

    /// Start of section `i` (the sum of the lengths before it).
    pub fn section_start(&self, i: usize) -> Ticks {
        self.sections[..i.min(self.sections.len())].iter().map(|s| s.length).sum()
    }

    /// Number of ordinary (audio and instrument) tracks; the return tracks follow them.
    pub fn ordinary_track_count(&self) -> usize {
        self.tracks.iter().take_while(|t| !t.is_return()).count()
    }

    /// Tracks of group `group`, in order (the tracks that play into it).
    pub fn group_members(&self, group: TrackId) -> impl Iterator<Item = &Track> {
        self.tracks.iter().filter(move |t| t.output == Some(group))
    }

    /// Name for a new group track: "Group", then "Group 2", "Group 3", ..., the first not in use.
    pub fn next_group_name(&self) -> String {
        (1..)
            .map(|n| if n == 1 { "Group".to_string() } else { format!("Group {n}") })
            .find(|n| !self.tracks.iter().any(|t| &t.name == n))
            .unwrap_or_default()
    }

    /// The return tracks, in order.
    pub fn returns(&self) -> impl Iterator<Item = &Track> {
        self.tracks.iter().filter(|t| t.is_return())
    }

    /// Position of a return track among the returns (its letter, [return_letter]).
    pub fn return_index(&self, id: TrackId) -> Option<usize> {
        self.returns().position(|t| t.id == id)
    }

    /// Repairs routing a hand-edited or damaged file could hold: return tracks move after the
    /// ordinary tracks (keeping their order) and lose any sends, output, input, arm or monitor.
    /// A return holding clips or an instrument, or beyond the first [MAX_RETURNS], loads as an
    /// ordinary track instead, so nothing in the file is lost. Sends and outputs that don't name a
    /// return track and duplicate sends are dropped, levels are clamped. Valid projects are unchanged.
    pub fn sanitize_routing(&mut self) {
        let (mut ordinary, mut returns): (Vec<Track>, Vec<Track>) =
            std::mem::take(&mut self.tracks).into_iter().partition(|t| !t.is_return());
        // A return can't hold clips or an instrument, and there are at most MAX_RETURNS. Rather than
        // drop content from a damaged file, such returns load as ordinary tracks at the end of the list.
        let mut kept = Vec::new();
        for mut r in returns.drain(..) {
            let has_content = !r.clips.is_empty() || !r.midi_clips.is_empty() || r.instrument_plugin.is_some();
            if has_content || kept.len() == MAX_RETURNS {
                r.kind = if r.midi_clips.is_empty() && r.instrument_plugin.is_none() { TrackKind::Audio } else { TrackKind::Instrument };
                ordinary.push(r);
            } else {
                r.input = None;
                r.arm = false;
                r.monitor = false;
                kept.push(r);
            }
        }
        let returns = kept;
        let return_ids: Vec<TrackId> = returns.iter().map(|t| t.id).collect();
        // Groups: kept while they hold no content and there is room for their bus next to the
        // returns; otherwise they load as ordinary tracks. A group keeps its sends to returns.
        let mut buses = returns.len();
        for t in &mut ordinary {
            if t.is_group() {
                let has_content = !t.clips.is_empty() || !t.midi_clips.is_empty() || t.instrument_plugin.is_some();
                if has_content || buses == MAX_RETURNS {
                    t.kind =
                        if t.midi_clips.is_empty() && t.instrument_plugin.is_none() { TrackKind::Audio } else { TrackKind::Instrument };
                } else {
                    buses += 1;
                    t.input = None;
                    t.arm = false;
                    t.monitor = false;
                    t.output = None;
                }
            }
        }
        let group_ids: Vec<TrackId> = ordinary.iter().filter(|t| t.is_group()).map(|t| t.id).collect();
        for t in &mut ordinary {
            let mut seen = Vec::new();
            t.sends.retain(|s| {
                let ok = return_ids.contains(&s.target) && !seen.contains(&s.target);
                seen.push(s.target);
                ok
            });
            for s in &mut t.sends {
                s.level = if s.level.is_finite() { s.level.clamp(0.0, TrackSend::MAX_LEVEL) } else { 0.0 };
            }
            if t.output.is_some_and(|o| !return_ids.contains(&o) && (t.is_group() || !group_ids.contains(&o))) {
                t.output = None;
            }
        }
        self.tracks = ordinary;
        for mut r in returns {
            r.sends.clear();
            r.output = None;
            self.tracks.push(r);
        }
    }

    /// Name for a new return track: "Return A", "Return B", ..., the first letter no return is named after.
    pub fn next_return_name(&self) -> String {
        (0..26)
            .map(|i| format!("Return {}", return_letter(i)))
            .find(|n| !self.returns().any(|t| &t.name == n))
            .unwrap_or_else(|| "Return".into())
    }

    pub fn track_index(&self, id: TrackId) -> Option<usize> {
        self.tracks.iter().position(|t| t.id == id)
    }

    pub fn asset(&self, id: AssetId) -> Option<&Asset> {
        self.assets.iter().find(|a| a.id == id)
    }

    /// Locate a clip: (track index, clip index).
    pub fn find_clip(&self, id: ClipId) -> Option<(usize, usize)> {
        self.tracks.iter().enumerate().find_map(|(ti, t)| t.clips.iter().position(|c| c.id == id).map(|ci| (ti, ci)))
    }

    /// Locate a MIDI clip: (track index, clip index).
    pub fn find_midi_clip(&self, id: ClipId) -> Option<(usize, usize)> {
        self.tracks.iter().enumerate().find_map(|(ti, t)| t.midi_clips.iter().position(|c| c.id == id).map(|ci| (ti, ci)))
    }

    pub fn midi_clip(&self, id: ClipId) -> Option<&MidiClip> {
        self.find_midi_clip(id).map(|(t, c)| &self.tracks[t].midi_clips[c])
    }

    /// Locate a device: (track index, slot).
    pub fn find_device(&self, id: DeviceId) -> Option<(usize, DeviceSlot)> {
        self.tracks.iter().enumerate().find_map(|(ti, t)| {
            if t.instrument_plugin.as_ref().is_some_and(|d| d.id == id) {
                return Some((ti, DeviceSlot::Instrument));
            }
            t.devices.iter().position(|d| d.id == id).map(|i| (ti, DeviceSlot::Insert(i)))
        })
    }

    pub fn device(&self, id: DeviceId) -> Option<&Device> {
        self.find_device(id).map(|(ti, slot)| match slot {
            DeviceSlot::Insert(i) => &self.tracks[ti].devices[i],
            DeviceSlot::Instrument => self.tracks[ti].instrument_plugin.as_ref().expect("found"),
        })
    }

    /// True if any clip (audio or MIDI) uses this id.
    pub fn clip_id_in_use(&self, id: ClipId) -> bool {
        self.find_clip(id).is_some() || self.find_midi_clip(id).is_some()
    }

    pub fn clip(&self, id: ClipId) -> Option<&Clip> {
        self.find_clip(id).map(|(t, c)| &self.tracks[t].clips[c])
    }

    pub fn ticks_per_bar(&self) -> Ticks {
        let ts = self.transport.time_signature;
        PPQ * 4 * ts.numerator as i64 / ts.denominator as i64
    }

    /// Ticks → seconds at the (currently constant) project tempo.
    pub fn ticks_to_seconds(&self, t: Ticks) -> f64 {
        t as f64 / PPQ as f64 * 60.0 / self.transport.bpm
    }

    pub fn seconds_to_ticks(&self, s: f64) -> Ticks {
        (s * self.transport.bpm / 60.0 * PPQ as f64).round() as Ticks
    }

    /// Ticks → frames at `rate` (rounded to nearest frame).
    pub fn ticks_to_frames(&self, t: Ticks, rate: f64) -> i64 {
        (self.ticks_to_seconds(t) * rate).round() as i64
    }

    pub fn frames_to_ticks(&self, frames: i64, rate: f64) -> Ticks {
        self.seconds_to_ticks(frames as f64 / rate)
    }

    /// Duration of a clip in ticks at the current tempo: warped clips cover
    /// their beats, the others keep their real-time duration.
    pub fn clip_length_ticks(&self, clip: &Clip) -> Ticks {
        match clip.clip_loop {
            Some(l) if l.on => l.length,
            _ => self.content_length_ticks(clip),
        }
    }

    /// What the clip plays without its loop: the source window at the clip's speed.
    pub fn content_length_ticks(&self, clip: &Clip) -> Ticks {
        if clip.warp.enabled {
            return crate::warp::beats_to_ticks(self.warped_clip_beats(clip));
        }
        let rate = self.asset(clip.asset).map(|a| a.sample_rate).unwrap_or(48_000.0);
        self.seconds_to_ticks(clip.source_length as f64 / rate / clip.warp.varispeed())
    }
}

/// Linear gain ↔ decibels helpers used by UI and AI commands.
pub fn db_to_gain(db: f32) -> f32 {
    if db <= -120.0 { 0.0 } else { 10f32.powf(db / 20.0) }
}

pub fn gain_to_db(g: f32) -> f32 {
    if g <= 1e-6 { f32::NEG_INFINITY } else { 20.0 * g.log10() }
}
