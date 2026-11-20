//! Derivation of the immutable session description sent to the audio engine.
//!
//! Pure data (no FFI) so it is testable without the engine. The engine crate
//! converts it into the C ABI structs (`se_session_desc`). All positions are
//! converted from musical time to frames at the engine sample rate here, through
//! the project's tempo map (crate::tempo), so the real-time thread never converts
//! positions. It only gets the map itself ([SessionSnapshot::tempo]) for the
//! metronome and the tempo and song position devices and plugins see.

use crate::device::Device;
use crate::model::{AssetId, Clip, Project};
use crate::tempo::TempoMap;
use crate::warp::{Transient, WarpMode, engine_mode};
use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq)]
pub struct ClipSnapshot {
    pub clip_id: u64,
    pub engine_asset: u64,
    pub timeline_start: i64,
    pub source_offset: i64,
    pub length: i64,
    pub gain: f32,
    /// Warp on: how the clip follows the tempo (`se_clip_warp_desc`).
    pub warp: Option<WarpSnapshot>,
    /// Looping: (loop start, loop end) in content frames from the clip start
    /// (`se_clip_loop_desc`); `length` is then the looped length.
    pub clip_loop: Option<(i64, i64)>,
    /// (fade-in, fade-out) in timeline frames from the clip's start and back from its end
    /// (`se_clip_fade_desc`); (0, 0) plays without fades.
    pub fades: (i64, i64),
}

/// A warped clip in engine frames.
#[derive(Debug, Clone, PartialEq)]
pub struct WarpSnapshot {
    /// `SE_WARP_*`.
    pub mode: u8,
    /// (timeline frame, source frame), strictly increasing, at least two.
    pub points: Vec<(i64, f64)>,
    /// Slices (source frame, percussive) for Drums and Auto, sorted.
    pub slices: Vec<(f64, bool)>,
    /// Semitones for the stretch modes.
    pub transpose: f32,
}

/// A note as absolute engine frames: [start, end) with the clip boundary
/// already applied (notes crossing the clip end are cut; notes starting at
/// or after it are dropped).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NoteSnapshot {
    pub start: i64,
    pub end: i64,
    pub pitch: u8,
    pub velocity: u8,
}

/// An enabled automation lane with points converted to engine frames.
#[derive(Debug, Clone, PartialEq)]
pub struct LaneSnapshot {
    pub target: crate::automation::AutomationTarget,
    /// (frame, plain value, curve), sorted by frame.
    pub points: Vec<(i64, f32, f32)>,
}

fn lanes(map: &TempoMap, lanes: &[crate::automation::AutomationLane], rate: f64) -> Vec<LaneSnapshot> {
    lanes
        .iter()
        .filter(|l| l.enabled && !l.points.is_empty())
        .map(|l| LaneSnapshot {
            target: l.target.clone(),
            points: l.points.iter().map(|q| (map.ticks_to_frames(q.time, rate), q.value, q.curve)).collect(),
        })
        .collect()
}

#[derive(Debug, Clone, PartialEq)]
pub struct TrackSnapshot {
    pub track_id: u64,
    pub gain: f32,
    pub pan: f32,
    /// Channel strip (spec 005): linear trim, width (1 = unchanged) and mono fold.
    pub trim: f32,
    pub width: f32,
    pub mono: bool,
    pub mute: bool,
    pub solo: bool,
    pub clips: Vec<ClipSnapshot>,
    /// True for instrument tracks (render `notes` through the built-in instrument).
    pub instrument: bool,
    /// All playable notes of the track's MIDI clips, sorted by start frame.
    pub notes: Vec<NoteSnapshot>,
    /// Enabled, non-empty automation lanes.
    pub automation: Vec<LaneSnapshot>,
    /// Recording/monitoring state (audio tracks: their input; instrument tracks: MIDI input).
    pub arm: bool,
    pub monitor: bool,
    pub input: Option<crate::model::InputSource>,
    /// Insert effects in processing order.
    pub devices: Vec<crate::device::Device>,
    /// Instrument plugin (replaces the built-in synth when present).
    pub instrument_plugin: Option<crate::device::Device>,
    /// Built-in instrument parameters (instrument tracks only).
    pub synth: Option<crate::instrument::SynthParams>,
    /// A return track: its source is what other tracks send to it (spec 002).
    pub is_return: bool,
    /// Where the track plays into: a return track's id, or the master (None).
    pub output: Option<u64>,
    /// Sends into return tracks.
    pub sends: Vec<SendSnapshot>,
}

/// One send of a track into a return track.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SendSnapshot {
    pub target: u64,
    /// Linear gain.
    pub gain: f32,
    pub pre_fader: bool,
}

/// One segment of the tempo map in engine frames (`se_tempo_segment`; the formula is in
/// crate::tempo and engine/README.md › Tempo).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TempoSegment {
    /// Exact (unrounded) timeline frame where the segment starts.
    pub start_frame: f64,
    /// Quarter notes from bar 1 there.
    pub start_quarter: f64,
    pub start_bpm: f64,
    /// Equal to `start_bpm` for a hold; a ramp reaches it at the next segment's start.
    pub end_bpm: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SessionSnapshot {
    pub tracks: Vec<TrackSnapshot>,
    pub master_gain: f32,
    /// Master volume automation (enabled, non-empty lanes).
    pub master_automation: Vec<LaneSnapshot>,
    /// Engine assets of the audio files built-in devices play
    /// (`Device::samples`); samples whose asset isn't loaded are left out.
    pub device_assets: HashMap<AssetId, u64>,
    /// The tempo map (spec 015); empty when the song has one tempo.
    pub tempo: Vec<TempoSegment>,
    /// Quarter notes per metronome beat (4 / the time signature's denominator); the engine clicks
    /// on the tempo map's beats with it when `tempo` is not empty.
    pub click_quarters: f64,
}

/// The engine's segments of a tempo map at `rate`.
pub fn tempo_segments(map: &TempoMap, rate: f64) -> Vec<TempoSegment> {
    map.segments()
        .iter()
        .map(|g| TempoSegment {
            start_frame: g.seconds * rate,
            start_quarter: g.tick as f64 / crate::PPQ as f64,
            start_bpm: g.start_bpm,
            end_bpm: g.end_bpm,
        })
        .collect()
}

/// Build a snapshot. `engine_assets` maps model assets to loaded engine
/// assets; clips whose asset isn't loaded (missing media, still importing)
/// are skipped and reported in the second return value.
pub fn build(project: &Project, engine_rate: f64, engine_assets: &HashMap<AssetId, u64>) -> (SessionSnapshot, Vec<u64>) {
    build_with_transients(project, engine_rate, engine_assets, &HashMap::new())
}

/// [`build`] with the detected transients of each asset (asset frames), which
/// warped clips in Drums and Auto mode slice at. Without them Drums plays the
/// clip as one slice and Auto stretches.
pub fn build_with_transients(
    project: &Project,
    engine_rate: f64,
    engine_assets: &HashMap<AssetId, u64>,
    transients: &HashMap<AssetId, Vec<Transient>>,
) -> (SessionSnapshot, Vec<u64>) {
    build_full(project, engine_rate, engine_assets, &HashMap::new(), transients)
}

/// [`build_with_transients`] with the engine's reversed copies of assets
/// (`se_asset_reverse`), which reversed clips play. A reversed clip whose copy
/// is missing is skipped.
pub fn build_full(
    project: &Project,
    engine_rate: f64,
    engine_assets: &HashMap<AssetId, u64>,
    reversed_assets: &HashMap<AssetId, u64>,
    transients: &HashMap<AssetId, Vec<Transient>>,
) -> (SessionSnapshot, Vec<u64>) {
    let mut skipped = Vec::new();
    let map = project.tempo_map();
    // Notes are the bulk of a snapshot: with one tempo, convert them with the same arithmetic as
    // before tempo maps existed (bit for bit, and as cheap), else through the map.
    let bpm = project.transport.bpm;
    let note_frames = |t: crate::model::Ticks| -> i64 {
        if map.is_constant() {
            (t as f64 / crate::PPQ as f64 * 60.0 / bpm * engine_rate).round() as i64
        } else {
            map.ticks_to_frames(t, engine_rate)
        }
    };
    let tracks = project
        .tracks
        .iter()
        .map(|t| TrackSnapshot {
            track_id: t.id.0,
            gain: t.gain,
            pan: t.pan,
            trim: t.trim_gain(),
            width: t.width,
            mono: t.mono,
            mute: t.mute,
            solo: t.solo,
            clips: t
                .clips
                .iter()
                .filter(|c| !c.disabled)
                .filter_map(|c| {
                    let (Some(&ea), Some(asset)) = (engine_assets.get(&c.asset), project.asset(c.asset)) else {
                        skipped.push(c.id.0);
                        return None;
                    };
                    let ratio = engine_rate / asset.sample_rate;
                    let reversed = match (c.warp.reverse, reversed_assets.get(&c.asset)) {
                        (false, _) => None,
                        (true, Some(&r)) => Some(r),
                        (true, None) => {
                            skipped.push(c.id.0);
                            return None;
                        }
                    };
                    let varispeed = !c.warp.enabled && c.warp.transpose != 0;
                    let start = map.ticks_to_frames(c.start, engine_rate);
                    let frames_at = |t| map.ticks_to_frames(c.start + t, engine_rate) - start;
                    let cs = ClipSnapshot {
                        clip_id: c.id.0,
                        engine_asset: ea,
                        timeline_start: start,
                        source_offset: (c.source_offset as f64 * ratio).round() as i64,
                        length: if c.warp.enabled || varispeed {
                            frames_at(project.content_length_ticks_in(&map, c))
                        } else {
                            (c.source_length as f64 * ratio).round() as i64
                        },
                        gain: c.gain,
                        warp: if varispeed {
                            Some(varispeed_snapshot(project, &map, c, ratio, engine_rate))
                        } else {
                            c.warp.enabled.then(|| {
                                warp_snapshot(
                                    project,
                                    &map,
                                    c,
                                    asset.sample_rate,
                                    engine_rate,
                                    transients.get(&c.asset).map_or(&[][..], |v| v),
                                )
                            })
                        },
                        clip_loop: None,
                        fades: Default::default(),
                    };
                    let frames = asset.frames as f64 * ratio;
                    let mut cs = match reversed {
                        Some(r) => mirror(cs, r, frames - (c.source_offset + c.source_length) as f64 * ratio, frames),
                        None => cs,
                    };
                    // With tempo changes a reversed warped clip mirrors in beats, not frames.
                    if reversed.is_some()
                        && c.warp.enabled
                        && !map.is_constant()
                        && let Some(w) = &mut cs.warp
                    {
                        w.points = project.reversed_warp_map_in(&map, c, engine_rate, frames);
                    }
                    if let Some(l) = c.clip_loop.filter(|l| l.on) {
                        let (ls, le) = (frames_at(l.start).max(0), frames_at(l.end).min(cs.length));
                        cs.length = frames_at(l.length);
                        cs.clip_loop = (le > ls).then_some((ls, le));
                    }
                    // Fades follow the clip on the timeline (a reversed clip keeps its fades where they are).
                    // Clips without fades (nearly all of them) skip the length lookup.
                    if !c.fades.is_default() {
                        let len_ticks = project.clip_length_ticks(c);
                        let f = c.fades.clamped(len_ticks);
                        let fade_in = frames_at(f.fade_in).clamp(0, cs.length);
                        let fade_out = (frames_at(len_ticks) - frames_at(len_ticks - f.fade_out)).clamp(0, cs.length - fade_in);
                        cs.fades = (fade_in, fade_out);
                    }
                    Some(cs)
                })
                .collect(),
            instrument: t.kind == crate::model::TrackKind::Instrument,
            synth: (t.kind == crate::model::TrackKind::Instrument).then(|| t.synth.clone()),
            // A chain head's switch bypasses its members too.
            devices: t
                .devices
                .iter()
                .enumerate()
                .map(|(i, d)| Device {
                    enabled: crate::modulation::effectively_enabled(&t.devices, i),
                    ..crate::builtin::engine_device(d, project.transport.key)
                })
                .collect(),
            instrument_plugin: if t.kind == crate::model::TrackKind::Instrument { t.instrument_plugin.clone() } else { None },
            automation: lanes(&map, &t.automation, engine_rate),
            arm: t.arm,
            monitor: t.monitor,
            input: t.input,
            is_return: t.is_bus(),
            output: t.output.map(|o| o.0),
            sends: t.sends.iter().map(|s| SendSnapshot { target: s.target.0, gain: s.level, pre_fader: s.pre_fader }).collect(),
            notes: {
                let mut v: Vec<NoteSnapshot> = t
                    .midi_clips
                    .iter()
                    .filter(|c| !c.disabled)
                    .flat_map(|c| {
                        c.playing_notes().into_iter().map(move |n| (c.start + n.start, c.start + n.start + n.length, n.pitch, n.velocity))
                    })
                    .map(|(s, e, pitch, velocity)| NoteSnapshot { start: note_frames(s), end: note_frames(e), pitch, velocity })
                    .filter(|n| n.end > n.start)
                    .collect();
                v.sort_by_key(|n| (n.start, n.pitch));
                v
            },
        })
        .collect();
    // A muted master is silent: no gain, and no volume lane to bring it back.
    let master_automation = if project.master_mute { Vec::new() } else { lanes(&map, &project.master_automation, engine_rate) };
    let master_gain = if project.master_mute { 0.0 } else { project.master_gain };
    let device_assets = project
        .tracks
        .iter()
        .flat_map(|t| t.devices.iter().chain(t.instrument_plugin.iter()))
        .flat_map(|d| d.samples.iter())
        .filter_map(|s| Some((s.asset, *engine_assets.get(&s.asset)?)))
        .collect();
    let tempo = tempo_segments(&map, engine_rate);
    let click_quarters = 4.0 / project.transport.time_signature.denominator.max(1) as f64;
    (SessionSnapshot { tracks, master_gain, master_automation, device_assets, tempo, click_quarters }, skipped)
}

/// A clip playing the engine's reversed copy (`frames` long, engine rate) of
/// its asset: the source window and the warp map mirrored, so timeline frame
/// start + length - t plays what the forward clip plays at t, back to front.
/// `start` is the window's first frame in the copy.
fn mirror(mut cs: ClipSnapshot, engine_asset: u64, start: f64, frames: f64) -> ClipSnapshot {
    cs.engine_asset = engine_asset;
    cs.source_offset = start.round() as i64;
    let ends = 2 * cs.timeline_start + cs.length;
    if let Some(w) = &mut cs.warp {
        w.points = w.points.iter().rev().map(|&(t, s)| (ends - t, frames - s)).collect();
        // Slice i of the copy holds forward slice i - 1 backwards, so it takes that slice's flag.
        if let Some(&(_, last)) = w.slices.last() {
            let fwd = std::mem::take(&mut w.slices);
            w.slices = std::iter::once((start, last)).chain((1..fwd.len()).rev().map(|i| (frames - fwd[i].0, fwd[i - 1].1))).collect();
        }
    }
    cs
}

fn warp_snapshot(project: &Project, map: &TempoMap, c: &Clip, asset_rate: f64, engine_rate: f64, detected: &[Transient]) -> WarpSnapshot {
    let mut mode = c.warp.mode;
    let mut slices = Vec::new();
    if matches!(mode, WarpMode::Drums | WarpMode::Auto) {
        let ratio = engine_rate / asset_rate;
        let all = c.warp.transients(detected, asset_rate);
        let start = c.source_offset;
        let end = c.source_offset + c.source_length;
        // The clip start is a slice boundary, in the passage it falls in.
        let head = all.iter().rev().find(|t| t.source <= start).is_none_or(|t| t.percussive);
        slices.push((start as f64 * ratio, head));
        slices.extend(all.iter().filter(|t| t.source > start && t.source < end).map(|t| (t.source as f64 * ratio, t.percussive)));
        if mode == WarpMode::Auto && all.is_empty() {
            mode = WarpMode::Melody;
            slices.clear();
        }
    }
    WarpSnapshot {
        mode: engine_mode(mode),
        points: project.warp_map_in(map, c, engine_rate),
        slices,
        transpose: c.warp.stretch_transpose(),
    }
}

/// An unwarped, transposed clip: Re-pitch along a straight map from its start
/// to its end, so the source plays 2^(st/12) times as fast.
fn varispeed_snapshot(project: &Project, map: &TempoMap, c: &Clip, ratio: f64, engine_rate: f64) -> WarpSnapshot {
    let start = map.ticks_to_frames(c.start, engine_rate);
    let end = map.ticks_to_frames(c.start + project.content_length_ticks_in(map, c), engine_rate).max(start + 1);
    let source = c.source_offset as f64 * ratio;
    let points = vec![(start, source), (end, source + c.source_length as f64 * ratio)];
    WarpSnapshot { mode: engine_mode(WarpMode::Repitch), points, slices: Vec::new(), transpose: 0.0 }
}
