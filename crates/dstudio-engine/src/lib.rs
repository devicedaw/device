//! Safe wrapper around the C++ audio engine.
//!
//! The [`Engine`] handle is `Send + Sync`: every C entry point is documented
//! as callable from any non-real-time thread. The real-time audio callback
//! lives entirely inside the C++ library.

pub mod ffi;
pub mod offline;

use dstudio_core::automation::{AutomationTarget, SynthParam};
use dstudio_core::instrument::{FilterMode, SynthParams, Waveform};
use dstudio_core::snapshot::{LaneSnapshot, SessionSnapshot};
use std::ffi::{CStr, CString};
use std::path::{Path, PathBuf};
use thiserror::Error;

#[derive(Debug, Error, Clone, PartialEq)]
#[error("{kind}: {message}")]
pub struct EngineError {
    pub code: i32,
    pub kind: &'static str,
    pub message: String,
}

fn kind_name(code: i32) -> &'static str {
    match code {
        ffi::SE_ERR_INVALID_ARG => "invalid argument",
        ffi::SE_ERR_IO => "i/o error",
        ffi::SE_ERR_UNSUPPORTED_FORMAT => "unsupported format",
        ffi::SE_ERR_DEVICE => "audio device error",
        ffi::SE_ERR_NOT_FOUND => "not found",
        ffi::SE_ERR_STATE => "invalid state",
        ffi::SE_ERR_OUT_OF_MEMORY => "out of memory",
        _ => "engine error",
    }
}

fn check(code: i32) -> Result<(), EngineError> {
    if code == ffi::SE_OK {
        return Ok(());
    }
    // SAFETY: se_last_error returns a thread-local NUL-terminated string or NULL.
    let message = unsafe {
        let p = ffi::se_last_error();
        if p.is_null() { String::new() } else { CStr::from_ptr(p).to_string_lossy().into_owned() }
    };
    Err(EngineError { code, kind: kind_name(code), message })
}

fn cpath(p: &Path) -> Result<CString, EngineError> {
    CString::new(p.to_string_lossy().as_bytes()).map_err(|_| EngineError {
        code: ffi::SE_ERR_INVALID_ARG,
        kind: "invalid argument",
        message: "path contains NUL".into(),
    })
}

/// A transient found by [`Engine::detect_onsets`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Onset {
    pub frame: i64,
    /// 0..1 relative to the strongest onset of the asset.
    pub strength: f32,
    /// The audio up to the next onset decays like a hit.
    pub percussive: bool,
}

#[derive(Debug, Clone, Copy)]
pub struct Config {
    pub sample_rate: f64,
    pub max_block_size: u32,
}

impl Default for Config {
    fn default() -> Self {
        Config { sample_rate: 48_000.0, max_block_size: 2048 }
    }
}

pub type AssetInfo = ffi::se_asset_info;
pub type Stats = ffi::se_stats;

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct TransportState {
    pub position: i64,
    pub playing: bool,
}

/// The browser audition voice, from [`Engine::preview`].
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct PreviewState {
    /// Asset of the sounding voice; `None` when nothing plays.
    pub asset_id: Option<u64>,
    pub position: i64,
    pub frames: i64,
    pub peak_l: f32,
    pub peak_r: f32,
    /// Preview notes held (`preview_note`).
    pub notes: u8,
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Meter {
    pub track_id: u64,
    pub peak_l: f32,
    pub peak_r: f32,
}

/// Largest `frames_per_beat` / `frames_per_quarter` the engine accepts.
pub const MAX_FRAMES_PER_BEAT: f64 = ffi::SE_MAX_FRAMES_PER_BEAT;
/// Largest `|frame|` [`Engine::seek`] accepts.
pub const MAX_TIMELINE_FRAME: i64 = ffi::SE_MAX_TIMELINE_FRAME;

/// Metronome settings for [`Engine::set_metronome`]. Beat `k` clicks at
/// timeline frame `round(k * frames_per_beat)`; every `beats_per_bar`-th
/// beat (from beat 0 at frame 0) is a downbeat. `gain` is linear and applied
/// after master gain. Not included in [`Engine::export_wav`] or the meters.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MetronomeConfig {
    pub enabled: bool,
    pub gain: f32,
    pub frames_per_beat: f64,
    pub beats_per_bar: u32,
}

pub struct Engine {
    raw: *mut ffi::se_engine,
    /// Held from a plugin state save until its bytes are copied out: the
    /// engine-owned buffer `se_plugin_saved_state` points into is replaced by
    /// the next save, and `Engine` is `Sync`.
    saved_state: std::sync::Mutex<()>,
}

// SAFETY: the C ABI is documented thread-safe for non-RT callers; the handle
// is only destroyed in Drop, which requires exclusive ownership.
unsafe impl Send for Engine {}
unsafe impl Sync for Engine {}

impl Engine {
    pub fn new(cfg: Config) -> Result<Self, EngineError> {
        let c = ffi::se_config {
            abi_version: ffi::SE_ABI_VERSION,
            sample_rate: cfg.sample_rate,
            max_block_size: cfg.max_block_size,
            output_channels: 2,
        };
        // SAFETY: valid pointer to a properly initialised config.
        let raw = unsafe { ffi::se_engine_create(&c) };
        if raw.is_null() {
            check(ffi::SE_ERR_STATE)?;
        }
        Ok(Engine { raw, saved_state: std::sync::Mutex::new(()) })
    }

    pub fn abi_version() -> u32 {
        unsafe { ffi::se_abi_version() }
    }

    pub fn start_device(&self) -> Result<(), EngineError> {
        check(unsafe { ffi::se_device_start(self.raw) })
    }

    pub fn stop_device(&self) -> Result<(), EngineError> {
        check(unsafe { ffi::se_device_stop(self.raw) })
    }

    pub fn device_running(&self) -> bool {
        unsafe { ffi::se_device_is_running(self.raw) != 0 }
    }

    pub fn sample_rate(&self) -> f64 {
        unsafe { ffi::se_engine_sample_rate(self.raw) }
    }

    /// Blocking decode + resample. Call from a worker thread.
    pub fn load_file(&self, path: &Path) -> Result<AssetInfo, EngineError> {
        let p = cpath(path)?;
        let mut info = AssetInfo::default();
        check(unsafe { ffi::se_asset_load_file(self.raw, p.as_ptr(), &mut info) })?;
        Ok(info)
    }

    pub fn release_asset(&self, id: u64) -> Result<(), EngineError> {
        check(unsafe { ffi::se_asset_release(self.raw, id) })
    }

    /// A new asset with `id`'s audio back to front (reversed clips). Copies
    /// the audio: a few ms per minute, off the audio thread.
    pub fn reverse_asset(&self, id: u64) -> Result<AssetInfo, EngineError> {
        let mut info = AssetInfo::default();
        check(unsafe { ffi::se_asset_reverse(self.raw, id, &mut info) })?;
        Ok(info)
    }

    pub fn asset_info(&self, id: u64) -> Result<AssetInfo, EngineError> {
        let mut info = AssetInfo::default();
        check(unsafe { ffi::se_asset_get_info(self.raw, id, &mut info) })?;
        Ok(info)
    }

    /// Min/max pairs (mono mix) for waveform display.
    pub fn peaks(&self, id: u64, start_frame: u64, frames_per_peak: u32, max_pairs: usize) -> Result<Vec<[f32; 2]>, EngineError> {
        let mut buf = vec![0f32; max_pairs * 2];
        let mut n = 0usize;
        check(unsafe { ffi::se_asset_compute_peaks(self.raw, id, start_frame, frames_per_peak, buf.as_mut_ptr(), max_pairs, &mut n) })?;
        buf.truncate(n * 2);
        Ok(buf.chunks_exact(2).map(|c| [c[0], c[1]]).collect())
    }

    /// Min/max pairs of one channel (0 left, 1 right) for per-channel lanes.
    pub fn channel_peaks(
        &self,
        id: u64,
        channel: u32,
        start_frame: u64,
        frames_per_peak: u32,
        max_pairs: usize,
    ) -> Result<Vec<[f32; 2]>, EngineError> {
        let mut buf = vec![0f32; max_pairs * 2];
        let mut n = 0usize;
        check(unsafe {
            ffi::se_asset_compute_channel_peaks(self.raw, id, channel, start_frame, frames_per_peak, buf.as_mut_ptr(), max_pairs, &mut n)
        })?;
        buf.truncate(n * 2);
        Ok(buf.chunks_exact(2).map(|c| [c[0], c[1]]).collect())
    }

    /// Transients of a loaded asset (engine-rate frames, sorted), for warp
    /// markers and Auto warping. CPU-bound: tens of ms per minute of audio.
    pub fn detect_onsets(&self, id: u64) -> Result<Vec<Onset>, EngineError> {
        let mut n = 0usize;
        check(unsafe { ffi::se_asset_detect_onsets(self.raw, id, std::ptr::null_mut(), 0, &mut n) })?;
        let mut buf = vec![ffi::se_onset::default(); n];
        check(unsafe { ffi::se_asset_detect_onsets(self.raw, id, buf.as_mut_ptr(), buf.len(), &mut n) })?;
        buf.truncate(n);
        Ok(buf.into_iter().map(|o| Onset { frame: o.frame, strength: o.strength, percussive: o.percussive != 0 }).collect())
    }

    /// Background warp renders still queued or running; [`Engine::plugin_idle`]
    /// attaches the finished ones.
    pub fn warp_renders_pending(&self) -> usize {
        let mut n = 0usize;
        // SAFETY: valid handle and out pointer.
        unsafe { ffi::se_warp_renders_pending(self.raw, &mut n) };
        n
    }

    /// Publish an immutable session snapshot to the audio thread
    /// (`se_session_publish_v12`: the tempo map (spec 015); audio clip fades; channel-strip trim, width and mono (spec 005); return tracks, sends and outputs (spec 002); audio clips, warped ones with their map, looped ones with their loop, plus, on instrument tracks,
    /// MIDI notes rendered by the engine's built-in synth with the track's
    /// [`SynthParams`] (`synth: None` uses the engine defaults, which equal
    /// `SynthParams::default()`) or by the track's instrument plugin; each
    /// track's record arm, input monitoring and input routing; each
    /// track's insert chain (`devices`); and the automation lanes of every
    /// track and of the master (`TrackSnapshot::automation`,
    /// `SessionSnapshot::master_automation`; see docs/adr/0005-automation.md).
    ///
    /// MAIN THREAD: plugin devices seen for the first time are instantiated
    /// during this call (see docs/adr/0003-plugins.md). Saved device state
    /// (base64) is decoded only for devices that need an instance; an
    /// undecodable state is ignored (the plugin starts with its defaults).
    pub fn publish(&self, s: &SessionSnapshot) -> Result<(), EngineError> {
        let clip_descs: Vec<Vec<ffi::se_clip_desc>> = s
            .tracks
            .iter()
            .map(|t| {
                t.clips
                    .iter()
                    .map(|c| ffi::se_clip_desc {
                        clip_id: c.clip_id,
                        asset_id: c.engine_asset,
                        timeline_start: c.timeline_start,
                        source_offset: c.source_offset,
                        length: c.length,
                        gain: c.gain,
                    })
                    .collect()
            })
            .collect();
        let note_descs: Vec<Vec<ffi::se_note_desc>> = s
            .tracks
            .iter()
            .map(|t| {
                if !t.instrument {
                    return Vec::new();
                }
                t.notes
                    .iter()
                    .map(|n| ffi::se_note_desc { start: n.start, end: n.end, pitch: n.pitch, velocity: n.velocity, _pad: [0; 6] })
                    .collect()
            })
            .collect();
        // Parameters only matter on instrument tracks; the engine copies them.
        let synth_descs: Vec<Option<ffi::se_synth_params>> =
            s.tracks.iter().map(|t| if t.instrument { t.synth.as_ref().map(synth_params_to_ffi) } else { None }).collect();
        // Plugin devices: owned strings / decoded states first, then the
        // C descriptors pointing into them (both outlive the call).
        let dev_store: Vec<Vec<DeviceStore>> = s
            .tracks
            .iter()
            .map(|t| {
                t.devices
                    .iter()
                    .map(|d| self.device_store(d, &t.devices, t.instrument_plugin.as_ref(), &s.device_assets))
                    .collect::<Result<_, _>>()
            })
            .collect::<Result<_, _>>()?;
        let inst_store: Vec<Option<DeviceStore>> = s
            .tracks
            .iter()
            .map(|t| {
                if t.instrument {
                    t.instrument_plugin.as_ref().map(|d| self.device_store(d, &[], None, &s.device_assets)).transpose()
                } else {
                    Ok(None)
                }
            })
            .collect::<Result<_, _>>()?;
        let dev_descs: Vec<Vec<ffi::se_device_desc>> = dev_store.iter().map(|v| v.iter().map(DeviceStore::desc).collect()).collect();
        let inst_descs: Vec<Option<ffi::se_device_desc>> = inst_store.iter().map(|d| d.as_ref().map(DeviceStore::desc)).collect();
        // Automation: points first, then the lane descriptors pointing at them.
        let track_points: Vec<Vec<Vec<ffi::se_auto_point>>> =
            s.tracks.iter().map(|t| t.automation.iter().map(lane_points).collect()).collect();
        let lane_descs: Vec<Vec<ffi::se_auto_lane_desc>> = s
            .tracks
            .iter()
            .zip(&track_points)
            .map(|(t, pts)| t.automation.iter().zip(pts).map(|(l, p)| lane_desc(l, p)).collect())
            .collect();
        let master_points: Vec<Vec<ffi::se_auto_point>> = s.master_automation.iter().map(lane_points).collect();
        let master_descs: Vec<ffi::se_auto_lane_desc> =
            s.master_automation.iter().zip(&master_points).map(|(l, p)| lane_desc(l, p)).collect();
        let tracks: Vec<ffi::se_track_desc_v6> = s
            .tracks
            .iter()
            .enumerate()
            .map(|(i, t)| ffi::se_track_desc_v6 {
                track_id: t.track_id,
                gain: t.gain,
                pan: t.pan,
                mute: t.mute as u8,
                solo: t.solo as u8,
                instrument: t.instrument as u8,
                arm: t.arm as u8,
                monitor: t.monitor as u8,
                input_mode: match t.input {
                    None => ffi::SE_INPUT_NONE,
                    Some(i) if i.stereo => ffi::SE_INPUT_STEREO,
                    Some(_) => ffi::SE_INPUT_MONO,
                },
                input_channel: t.input.map_or(0, |i| i.channel),
                clips: clip_descs[i].as_ptr(),
                clip_count: clip_descs[i].len() as u32,
                note_count: note_descs[i].len() as u32,
                notes: note_descs[i].as_ptr(),
                synth: synth_descs[i].as_ref().map_or(std::ptr::null(), |p| p as *const ffi::se_synth_params),
                devices: dev_descs[i].as_ptr(),
                device_count: dev_descs[i].len() as u32,
                _pad2: 0,
                instrument_plugin: inst_descs[i].as_ref().map_or(std::ptr::null(), |d| d as *const ffi::se_device_desc),
                track_name: std::ptr::null(),
                lanes: lane_descs[i].as_ptr(),
                lane_count: lane_descs[i].len() as u32,
                _pad3: 0,
            })
            .collect();
        // Warped clips: points and slices first, then the descriptors.
        let warped: Vec<(u64, &dstudio_core::snapshot::WarpSnapshot)> =
            s.tracks.iter().flat_map(|t| t.clips.iter().filter_map(|c| c.warp.as_ref().map(|w| (c.clip_id, w)))).collect();
        let warp_points: Vec<Vec<ffi::se_warp_point>> =
            warped.iter().map(|(_, w)| w.points.iter().map(|&(frame, source)| ffi::se_warp_point { frame, source }).collect()).collect();
        let warp_slices: Vec<Vec<ffi::se_warp_slice>> = warped
            .iter()
            .map(|(_, w)| {
                w.slices
                    .iter()
                    .map(|&(source, percussive)| ffi::se_warp_slice { source, percussive: percussive as u8, _pad: [0; 7] })
                    .collect()
            })
            .collect();
        let warp_descs: Vec<ffi::se_clip_warp_desc> = warped
            .iter()
            .zip(warp_points.iter().zip(&warp_slices))
            .map(|((id, w), (pts, sl))| ffi::se_clip_warp_desc {
                clip_id: *id,
                mode: w.mode,
                _pad: [0; 3],
                transpose: w.transpose,
                points: pts.as_ptr(),
                point_count: pts.len() as u32,
                slice_count: sl.len() as u32,
                slices: if sl.is_empty() { std::ptr::null() } else { sl.as_ptr() },
            })
            .collect();
        let loop_descs: Vec<ffi::se_clip_loop_desc> = s
            .tracks
            .iter()
            .flat_map(|t| {
                t.clips.iter().filter_map(|c| {
                    c.clip_loop.map(|(loop_start, loop_end)| ffi::se_clip_loop_desc { clip_id: c.clip_id, loop_start, loop_end })
                })
            })
            .collect();
        // Routing: return tracks and outputs, then sends (spec 002).
        let routes: Vec<ffi::se_track_route> = s
            .tracks
            .iter()
            .filter(|t| t.is_return || t.output.is_some())
            .map(|t| ffi::se_track_route {
                track_id: t.track_id,
                output_track_id: t.output.unwrap_or(0),
                is_return: t.is_return as u8,
                _pad: [0; 7],
            })
            .collect();
        let sends: Vec<ffi::se_send_desc> = s
            .tracks
            .iter()
            .flat_map(|t| {
                t.sends.iter().map(move |x| ffi::se_send_desc {
                    track_id: t.track_id,
                    return_track_id: x.target,
                    gain: x.gain,
                    pre_fader: x.pre_fader as u8,
                    _pad: [0; 3],
                })
            })
            .collect();
        let v8 = ffi::se_session_desc_v8 {
            v7: ffi::se_session_desc_v7 {
                v6: ffi::se_session_desc_v6 {
                    tracks: tracks.as_ptr(),
                    track_count: tracks.len() as u32,
                    master_gain: s.master_gain,
                    master_lanes: master_descs.as_ptr(),
                    master_lane_count: master_descs.len() as u32,
                    _pad: 0,
                },
                warps: warp_descs.as_ptr(),
                warp_count: warp_descs.len() as u32,
                _pad: 0,
            },
            loops: loop_descs.as_ptr(),
            loop_count: loop_descs.len() as u32,
            _pad: 0,
        };
        let v9 = ffi::se_session_desc_v9 {
            v8,
            routes: routes.as_ptr(),
            route_count: routes.len() as u32,
            send_count: sends.len() as u32,
            sends: sends.as_ptr(),
        };
        // Channel strip (spec 005): only tracks whose trim, width or mono change the signal.
        let strips: Vec<ffi::se_track_strip> = s
            .tracks
            .iter()
            .filter(|t| t.trim != 1.0 || t.width != 1.0 || t.mono)
            .map(|t| ffi::se_track_strip { track_id: t.track_id, trim: t.trim, width: t.width, mono: t.mono as u8, _pad: [0; 7] })
            .collect();
        let v10 = ffi::se_session_desc_v10 { v9, strips: strips.as_ptr(), strip_count: strips.len() as u32, _pad: 0 };
        // Fades: only clips that have one.
        let fades: Vec<ffi::se_clip_fade_desc> = s
            .tracks
            .iter()
            .flat_map(|t| t.clips.iter())
            .filter(|c| c.fades != (0, 0))
            .map(|c| ffi::se_clip_fade_desc { clip_id: c.clip_id, fade_in: c.fades.0, fade_out: c.fades.1 })
            .collect();
        let v11 = ffi::se_session_desc_v11 { v10, fades: fades.as_ptr(), fade_count: fades.len() as u32, _pad: 0 };
        // Tempo map (spec 015): none when the song has one tempo.
        let tempo: Vec<ffi::se_tempo_segment> = s
            .tempo
            .iter()
            .map(|g| ffi::se_tempo_segment {
                start_frame: g.start_frame,
                start_quarter: g.start_quarter,
                start_bpm: g.start_bpm,
                end_bpm: g.end_bpm,
            })
            .collect();
        let desc = ffi::se_session_desc_v12 {
            v11,
            tempo: tempo.as_ptr(),
            tempo_count: tempo.len() as u32,
            _pad: 0,
            click_quarters: if s.click_quarters.is_finite() && s.click_quarters > 0.0 { s.click_quarters } else { 1.0 },
        };
        // SAFETY: all pointers stay valid for the duration of the call; the engine copies.
        check(unsafe { ffi::se_session_publish_v12(self.raw, &desc) })
    }

    fn device_store(
        &self,
        d: &dstudio_core::Device,
        devices: &[dstudio_core::Device],
        instrument: Option<&dstudio_core::Device>,
        assets: &std::collections::HashMap<dstudio_core::AssetId, u64>,
    ) -> Result<DeviceStore, EngineError> {
        // SAFETY: plain query on a valid handle.
        let state = if let Some(def) = dstudio_core::builtin::def_of(d) {
            let sidechain = d.sidechain.map_or(0, |t| t.0);
            let routes = dstudio_core::modulation::routes(devices, instrument, d);
            let samples: Vec<_> = d.samples.iter().filter_map(|s| Some((*assets.get(&s.asset)?, s.slot, s.slices.as_slice()))).collect();
            if def.samples == 0 && routes.is_empty() {
                Some(builtin_state(&def.values(d), sidechain))
            } else {
                Some(builtin_state_v2(&def.values(d), sidechain, &samples, &routes))
            }
        } else if unsafe { ffi::se_plugin_is_live(self.raw, d.id.0) } != 0 {
            None
        } else {
            d.state.as_deref().and_then(dstudio_core::device::b64::decode)
        };
        store_of(d, state)
    }

    pub fn play(&self) -> Result<(), EngineError> {
        check(unsafe { ffi::se_transport_play(self.raw) })
    }

    pub fn pause(&self) -> Result<(), EngineError> {
        check(unsafe { ffi::se_transport_pause(self.raw) })
    }

    pub fn stop(&self) -> Result<(), EngineError> {
        check(unsafe { ffi::se_transport_stop(self.raw) })
    }

    /// `|frame|` must be at most [`MAX_TIMELINE_FRAME`].
    pub fn seek(&self, frame: i64) -> Result<(), EngineError> {
        check(unsafe { ffi::se_transport_seek(self.raw, frame) })
    }

    pub fn transport(&self) -> TransportState {
        let mut t = ffi::se_transport_info::default();
        unsafe { ffi::se_transport_get(self.raw, &mut t) };
        TransportState { position: t.position, playing: t.playing != 0 }
    }

    pub fn meters(&self, out: &mut Vec<Meter>, capacity: usize) {
        let mut raw = vec![ffi::se_meter::default(); capacity];
        let mut n = 0usize;
        if unsafe { ffi::se_meters_read(self.raw, raw.as_mut_ptr(), capacity, &mut n) } == ffi::SE_OK {
            out.clear();
            out.extend(raw[..n].iter().map(|m| Meter { track_id: m.track_id, peak_l: m.peak_l, peak_r: m.peak_r }));
        }
    }

    /// Non-destructive meter read: per meter (master first, then tracks in
    /// session order) the peak over the last ~50 ms of rendered audio.
    /// Unlike [`Engine::meters`] this resets nothing, so it can be polled at
    /// any rate.
    pub fn peek_meters(&self, out: &mut Vec<Meter>, capacity: usize) {
        let mut raw = vec![ffi::se_meter::default(); capacity];
        let mut n = 0usize;
        if unsafe { ffi::se_meters_peek(self.raw, raw.as_mut_ptr(), capacity, &mut n) } == ffi::SE_OK {
            out.clear();
            out.extend(raw[..n].iter().map(|m| Meter { track_id: m.track_id, peak_l: m.peak_l, peak_r: m.peak_r }));
        }
    }

    /// Loop playback over `[start, end)` frames. Enabling requires
    /// `start < end`; when disabling, `start`/`end` are ignored.
    pub fn set_loop(&self, enabled: bool, start: i64, end: i64) -> Result<(), EngineError> {
        check(unsafe { ffi::se_transport_set_loop(self.raw, enabled as u8, start, end) })
    }

    /// Song tempo for tempo-synced devices and plugins, in frames per quarter
    /// note (`60 * rate / bpm`), independent of the metronome's click grid
    /// (one time-signature denominator note). 120 BPM until set. Must be
    /// finite and in `1..=MAX_FRAMES_PER_BEAT`.
    pub fn set_tempo(&self, frames_per_quarter: f64) -> Result<(), EngineError> {
        // SAFETY: valid handle.
        check(unsafe { ffi::se_transport_set_tempo(self.raw, frames_per_quarter) })
    }

    /// The click grid only; the tempo devices see is [`Engine::set_tempo`].
    pub fn set_metronome(&self, cfg: MetronomeConfig) -> Result<(), EngineError> {
        let d = ffi::se_metronome_desc {
            enabled: cfg.enabled as u8,
            _pad: [0; 3],
            gain: cfg.gain,
            frames_per_beat: cfg.frames_per_beat,
            beats_per_bar: cfg.beats_per_bar,
            _pad2: 0,
        };
        // SAFETY: valid pointer for the duration of the call; the engine copies.
        check(unsafe { ffi::se_metronome_set(self.raw, &d) })
    }

    pub fn stats(&self) -> Stats {
        let mut s = Stats::default();
        unsafe { ffi::se_stats_get(self.raw, &mut s) };
        s
    }

    pub fn reset_stats(&self) {
        unsafe { ffi::se_stats_reset(self.raw) }
    }

    /// Render interleaved stereo offline (device must be stopped).
    pub fn render_offline(&self, out: &mut [f32]) -> Result<(), EngineError> {
        let frames = (out.len() / 2) as u32;
        check(unsafe { ffi::se_render_offline(self.raw, out.as_mut_ptr(), frames) })
    }

    /// Set one parameter of a live built-in device right away (plain value;
    /// used while dragging, before the edit is committed and republished).
    pub fn builtin_set_param(&self, device_id: u64, index: u32, value: f32) -> Result<(), EngineError> {
        check(unsafe { ffi::se_builtin_set_param(self.raw, device_id, index, value) })
    }

    /// Set a hosted plugin's parameter (normalized) from a plugin card. A
    /// modulated parameter keeps it as its base.
    pub fn plugin_set_param(&self, device_id: u64, param_id: u32, normalized: f32) -> Result<(), EngineError> {
        check(unsafe { ffi::se_plugin_set_param(self.raw, device_id, param_id, normalized) })
    }

    /// Base value of a modulated plugin parameter; `None` when it is not modulated.
    pub fn mod_base(&self, device_id: u64, param_id: u32) -> Option<f32> {
        let mut v = 0f32;
        (unsafe { ffi::se_mod_base(self.raw, device_id, param_id, &mut v) } == ffi::SE_OK).then_some(v)
    }

    /// Latest readouts of a live built-in device (registry order), e.g. the
    /// compressor's input, output and gain reduction in dB.
    pub fn builtin_readouts(&self, device_id: u64, out: &mut Vec<f32>) -> Result<(), EngineError> {
        let mut buf = [0f32; 64];
        let mut n = 0usize;
        check(unsafe { ffi::se_builtin_readouts(self.raw, device_id, buf.as_mut_ptr(), buf.len(), &mut n) })?;
        out.clear();
        out.extend_from_slice(&buf[..n.min(buf.len())]);
        Ok(())
    }

    /// Turns In / Out peak metering of a live built-in device on or off (its expanded window's meters).
    pub fn builtin_set_metering(&self, device_id: u64, on: bool) -> Result<(), EngineError> {
        check(unsafe { ffi::se_builtin_set_metering(self.raw, device_id, u8::from(on)) })
    }

    /// In / Out peaks of a live built-in device since the previous call (linear):
    /// input left, input right, output left, output right. All zero while metering is off.
    pub fn builtin_levels(&self, device_id: u64, out: &mut [f32; 4]) -> Result<(), EngineError> {
        check(unsafe { ffi::se_builtin_levels(self.raw, device_id, out.as_mut_ptr()) })
    }

    /// Turns the chain output meter of a live device (any format) on or off.
    pub fn device_set_output_metering(&self, device_id: u64, on: bool) -> Result<(), EngineError> {
        check(unsafe { ffi::se_device_set_output_metering(self.raw, device_id, u8::from(on)) })
    }

    /// Output peaks (linear, left / right) of `ids` since the previous call, into `out`
    /// (resized to `ids.len()`). Zero for unknown devices and while metering is off.
    pub fn device_output_peaks(&self, ids: &[u64], out: &mut Vec<[f32; 2]>) {
        out.clear();
        out.resize(ids.len(), [0.0; 2]);
        if ids.is_empty() {
            return;
        }
        // SAFETY: `out` holds 2 * ids.len() contiguous floats.
        let rc = unsafe { ffi::se_device_output_peaks(self.raw, ids.as_ptr(), ids.len(), out.as_mut_ptr().cast::<f32>()) };
        if rc != 0 {
            out.iter_mut().for_each(|p| *p = [0.0; 2]);
        }
    }

    /// Stereo correlation of a live built-in device's input and output since the previous
    /// call (-1..1; 2 where that side was silent or metering is off).
    pub fn builtin_correlation(&self, device_id: u64, out: &mut [f32; 2]) -> Result<(), EngineError> {
        check(unsafe { ffi::se_builtin_correlation(self.raw, device_id, out.as_mut_ptr()) })
    }

    /// Number of bins in an analyser frame ([`Engine::builtin_spectrum`]).
    pub const SPECTRUM_BINS: usize = 128;

    /// The expanded window's analyser of a live built-in device: 0 off, 1 its
    /// input (Pre), 2 its output (Post).
    pub fn builtin_set_spectrum(&self, device_id: u64, mode: u8) -> Result<(), EngineError> {
        check(unsafe { ffi::se_builtin_set_spectrum(self.raw, device_id, mode) })
    }

    /// Newest analyser frame: [`Engine::SPECTRUM_BINS`] levels in dBFS, log spaced
    /// from 20 Hz to 20 kHz (floor -90). Returns the frame number (0 = none yet,
    /// unchanged = nothing new); `out` is left empty before the first frame.
    pub fn builtin_spectrum(&self, device_id: u64, out: &mut Vec<f32>) -> Result<u64, EngineError> {
        out.resize(Self::SPECTRUM_BINS, 0.0);
        let (mut n, mut seq) = (0usize, 0u64);
        let r = check(unsafe { ffi::se_builtin_spectrum(self.raw, device_id, out.as_mut_ptr(), out.len(), &mut n, &mut seq) });
        out.truncate(if r.is_ok() { n.min(Self::SPECTRUM_BINS) } else { 0 });
        r.map(|()| seq)
    }

    /// Newest analysis frame of a live built-in device (bulk visualiser data,
    /// e.g. the Spectrum's band levels; layout per device). Returns the
    /// frame's publish number: 0 = nothing published yet, unchanged = the
    /// same frame as last time.
    pub fn builtin_analysis(&self, device_id: u64, out: &mut Vec<f32>) -> Result<u64, EngineError> {
        const MAX: usize = 4096;
        out.resize(MAX, 0.0);
        let (mut n, mut seq) = (0usize, 0u64);
        let r = check(unsafe { ffi::se_builtin_analysis(self.raw, device_id, out.as_mut_ptr(), MAX, &mut n, &mut seq) });
        out.truncate(if r.is_ok() { n.min(MAX) } else { 0 });
        r.map(|()| seq)
    }

    /// Play a note on a live built-in instrument right away (pad audition,
    /// previews); velocity 0 releases it.
    pub fn builtin_note(&self, device_id: u64, pitch: u8, velocity: u8) -> Result<(), EngineError> {
        check(unsafe { ffi::se_builtin_note(self.raw, device_id, pitch, velocity) })
    }

    pub fn export_wav(&self, path: &Path, start: i64, end: i64) -> Result<(), EngineError> {
        let p = cpath(path)?;
        check(unsafe { ffi::se_export_wav(self.raw, p.as_ptr(), start, end) })
    }
}

fn store_of(d: &dstudio_core::Device, state: Option<Vec<u8>>) -> Result<DeviceStore, EngineError> {
    let cs = |s: &str| CString::new(s).map_err(|_| nul_error());
    Ok(DeviceStore {
        id: d.id.0,
        name: cs(&d.plugin.name)?,
        plugin_id: cs(&d.plugin.id)?,
        path: d.plugin.path.as_deref().map(cs).transpose()?,
        state,
        format: match d.plugin.format {
            dstudio_core::PluginFormat::AudioUnit => ffi::SE_PLUGIN_AU,
            dstudio_core::PluginFormat::Vst3 => ffi::SE_PLUGIN_VST3,
            dstudio_core::PluginFormat::Clap => ffi::SE_PLUGIN_CLAP,
            dstudio_core::PluginFormat::Builtin => ffi::SE_PLUGIN_BUILTIN,
        },
        enabled: d.enabled,
    })
}

/// Owned storage behind one `se_device_desc` during a publish.
struct DeviceStore {
    id: u64,
    name: CString,
    plugin_id: CString,
    path: Option<CString>,
    state: Option<Vec<u8>>,
    format: u8,
    enabled: bool,
}

impl DeviceStore {
    fn desc(&self) -> ffi::se_device_desc {
        ffi::se_device_desc {
            device_id: self.id,
            name: self.name.as_ptr(),
            plugin_id: self.plugin_id.as_ptr(),
            path: self.path.as_ref().map_or(std::ptr::null(), |p| p.as_ptr()),
            state: self.state.as_ref().map_or(std::ptr::null(), |s| s.as_ptr()),
            state_len: self.state.as_ref().map_or(0, |s| s.len()),
            format: self.format,
            enabled: self.enabled as u8,
            _pad: [0; 6],
        }
    }
}

/// The engine state of a built-in device: an `se_builtin_state` header, then
/// the plain values in definition order.
pub fn builtin_state(values: &[f32], sidechain_track: u64) -> Vec<u8> {
    let mut out = Vec::with_capacity(16 + 4 * values.len());
    out.extend_from_slice(&ffi::SE_BUILTIN_STATE_MAGIC.to_le_bytes());
    out.extend_from_slice(&(values.len() as u32).to_le_bytes());
    out.extend_from_slice(&sidechain_track.to_le_bytes());
    for v in values {
        out.extend_from_slice(&v.to_le_bytes());
    }
    out
}

/// The v2 engine state of a built-in device that plays audio files: the
/// `se_builtin_state_v2` header, the values, then one `se_builtin_sample`
/// per (engine asset, slot, slices), every sample's slices in that order,
/// then the routes of a modulation source.
pub fn builtin_state_v2(
    values: &[f32],
    sidechain_track: u64,
    samples: &[(u64, u32, &[dstudio_core::Slice])],
    routes: &[dstudio_core::modulation::ModRoute],
) -> Vec<u8> {
    // Slices the engine would reject (and with them the whole session):
    // dropped, keeping the rest ascending.
    let valid: Vec<Vec<dstudio_core::Slice>> = samples
        .iter()
        .map(|(_, _, sl)| {
            let mut kept: Vec<dstudio_core::Slice> = Vec::new();
            for s in sl.iter() {
                let ok = [s.start, s.gain, s.pan, s.pitch].iter().all(|f| f.is_finite())
                    && (0.0..1.0).contains(&s.start)
                    && kept.last().is_none_or(|p| s.start > p.start);
                if ok && kept.len() < ffi::SE_BUILTIN_MAX_SLICES {
                    kept.push(*s);
                }
            }
            kept
        })
        .collect();
    let slices: usize = valid.iter().map(Vec::len).sum();
    let mut out = Vec::with_capacity(32 + 4 * values.len() + 16 * (samples.len() + slices) + 40 * routes.len());
    out.extend_from_slice(&ffi::SE_BUILTIN_STATE_MAGIC2.to_le_bytes());
    out.extend_from_slice(&(values.len() as u32).to_le_bytes());
    out.extend_from_slice(&sidechain_track.to_le_bytes());
    out.extend_from_slice(&(samples.len() as u32).to_le_bytes());
    out.extend_from_slice(&(slices as u32).to_le_bytes());
    out.extend_from_slice(&(routes.len() as u32).to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    for v in values {
        out.extend_from_slice(&v.to_le_bytes());
    }
    for ((asset, slot, _), sl) in samples.iter().zip(&valid) {
        out.extend_from_slice(&asset.to_le_bytes());
        out.extend_from_slice(&slot.to_le_bytes());
        out.extend_from_slice(&(sl.len() as u32).to_le_bytes());
    }
    for sl in &valid {
        for s in sl {
            for f in [s.start, s.gain, s.pan, s.pitch] {
                out.extend_from_slice(&f.to_le_bytes());
            }
        }
    }
    for r in routes {
        out.extend_from_slice(&r.target.0.to_le_bytes());
        out.extend_from_slice(&r.param.to_le_bytes());
        out.extend_from_slice(&[if r.plugin { ffi::SE_MOD_PLUGIN } else { ffi::SE_MOD_BUILTIN }, r.slot, r.absolute as u8, r.p_log as u8]);
        for f in [r.min, r.max, r.p_min, r.p_max, r.p_skew, r.p_step] {
            out.extend_from_slice(&f.to_le_bytes());
        }
    }
    out
}

/// One processor in the engine's built-in registry.
#[derive(Debug, Clone, PartialEq)]
pub struct BuiltinProcessor {
    pub id: String,
    pub params: Vec<String>,
    pub readouts: Vec<String>,
    pub defaults: Vec<f32>,
    /// Sample slots the processor plays (0 for effects without files).
    pub sample_slots: u32,
    /// Plays notes (an instrument) rather than processing audio.
    pub instrument: bool,
}

/// Every built-in processor the engine was compiled with.
pub fn builtin_registry() -> Vec<BuiltinProcessor> {
    let strings = |p: *const *const std::ffi::c_char, n: u32| -> Vec<String> {
        // SAFETY: the registry is static data with `n` valid C strings at `p`.
        (0..n as usize).map(|i| unsafe { CStr::from_ptr(*p.add(i)) }.to_string_lossy().into_owned()).collect()
    };
    let mut n = 0usize;
    // SAFETY: returns a pointer to `n` static descriptors.
    let p = unsafe { ffi::se_builtin_registry(&mut n) };
    if p.is_null() {
        return Vec::new();
    }
    // SAFETY: see above.
    unsafe { std::slice::from_raw_parts(p, n) }
        .iter()
        .map(|d| BuiltinProcessor {
            // SAFETY: static C string.
            id: unsafe { CStr::from_ptr(d.id) }.to_string_lossy().into_owned(),
            params: strings(d.param_ids, d.param_count),
            readouts: strings(d.readout_ids, d.readout_count),
            // SAFETY: static array of `param_count` floats.
            defaults: unsafe { std::slice::from_raw_parts(d.param_defaults, d.param_count as usize) }.to_vec(),
            sample_slots: d.sample_slots,
            instrument: d.instrument != 0,
        })
        .collect()
}

fn lane_points(l: &LaneSnapshot) -> Vec<ffi::se_auto_point> {
    l.points.iter().map(|&(frame, value, curve)| ffi::se_auto_point { frame, value, curve }).collect()
}

/// C descriptor of a lane; `points` must outlive its use.
fn lane_desc(l: &LaneSnapshot, points: &[ffi::se_auto_point]) -> ffi::se_auto_lane_desc {
    let (target, synth_param, device_id, param_id) = match &l.target {
        AutomationTarget::Volume => (ffi::SE_AUTO_VOLUME, 0, 0, 0),
        AutomationTarget::Pan => (ffi::SE_AUTO_PAN, 0, 0, 0),
        AutomationTarget::Synth { param } => (ffi::SE_AUTO_SYNTH, synth_param_index(*param), 0, 0),
        AutomationTarget::Plugin { device, param, .. } => (ffi::SE_AUTO_PLUGIN, 0, device.0, *param),
        AutomationTarget::Builtin { device, device_type, param } => {
            let index = dstudio_core::builtin::find(device_type).and_then(|d| d.index_of(param)).map_or(u32::MAX, |i| i as u32);
            (ffi::SE_AUTO_BUILTIN, 0, device.0, index)
        }
        AutomationTarget::Mute => (ffi::SE_AUTO_MUTE, 0, 0, 0),
        AutomationTarget::DeviceOn { device, .. } => (ffi::SE_AUTO_DEVICE_ON, 0, device.0, 0),
        AutomationTarget::Trim => (ffi::SE_AUTO_TRIM, 0, 0, 0),
        AutomationTarget::Width => (ffi::SE_AUTO_WIDTH, 0, 0, 0),
        AutomationTarget::Mono => (ffi::SE_AUTO_MONO, 0, 0, 0),
        // Never in a snapshot (SetAutomationLane refuses it); an unknown target fails the publish.
        AutomationTarget::Tempo => (u8::MAX, 0, 0, 0),
    };
    ffi::se_auto_lane_desc {
        target,
        logarithmic: l.target.logarithmic() as u8,
        synth_param,
        param_id,
        device_id,
        points: points.as_ptr(),
        point_count: points.len() as u32,
        flags: if l.target.stepped() { ffi::SE_AUTO_FLAG_STEPPED } else { 0 },
    }
}

/// `SE_SYNTH_P_*` index: the position in [`SynthParam::ALL`].
pub fn synth_param_index(p: SynthParam) -> u16 {
    SynthParam::ALL.iter().position(|&q| q == p).expect("SynthParam::ALL lists every parameter") as u16
}

/// Evaluates a lane with the engine's own code (`se_automation_eval`) at
/// the given timeline frames, for tests of the evaluation contract. Errors
/// like a publish would (unsorted points, bad curve, out-of-range synth value, no points).
pub fn engine_eval_lane(lane: &LaneSnapshot, frames: &[i64]) -> Result<Vec<f32>, EngineError> {
    let pts = lane_points(lane);
    let d = lane_desc(lane, &pts);
    let mut out = vec![0f32; frames.len()];
    // SAFETY: the descriptor's points, `frames` and `out` are valid for the call.
    check(unsafe { ffi::se_automation_eval(&d, frames.as_ptr(), frames.len(), out.as_mut_ptr()) })?;
    Ok(out)
}

fn wave_to_ffi(w: Waveform) -> u8 {
    match w {
        Waveform::Saw => ffi::SE_WAVE_SAW,
        Waveform::Square => ffi::SE_WAVE_SQUARE,
        Waveform::Triangle => ffi::SE_WAVE_TRIANGLE,
        Waveform::Sine => ffi::SE_WAVE_SINE,
    }
}

/// Maps the model's [`SynthParams`] onto the engine's flat C struct (the
/// preset name is display-only and not sent). Values are passed through
/// unchanged; the engine validates the documented ranges on publish.
pub fn synth_params_to_ffi(p: &SynthParams) -> ffi::se_synth_params {
    let [o1, o2] = &p.osc;
    ffi::se_synth_params {
        osc1_wave: wave_to_ffi(o1.wave),
        osc1_octave: o1.octave,
        osc1_semitones: o1.semitones,
        osc2_wave: wave_to_ffi(o2.wave),
        osc2_octave: o2.octave,
        osc2_semitones: o2.semitones,
        filter_mode: match p.filter.mode {
            FilterMode::LowPass => ffi::SE_FILTER_LOWPASS,
            FilterMode::HighPass => ffi::SE_FILTER_HIGHPASS,
            FilterMode::BandPass => ffi::SE_FILTER_BANDPASS,
        },
        lfo_wave: wave_to_ffi(p.lfo.wave),
        polyphony: p.voice.polyphony,
        unison: p.voice.unison,
        _pad: [0; 6],
        osc1_shape: o1.shape,
        osc1_fine_cents: o1.fine_cents,
        osc1_level: o1.level,
        osc2_shape: o2.shape,
        osc2_fine_cents: o2.fine_cents,
        osc2_level: o2.level,
        cross_fm: p.cross_fm,
        sub_level: p.sub_level,
        noise_level: p.noise_level,
        filter_cutoff_hz: p.filter.cutoff_hz,
        filter_resonance: p.filter.resonance,
        filter_key_track: p.filter.key_track,
        filter_env_amount: p.filter.env_amount,
        amp_attack: p.amp_env.attack,
        amp_decay: p.amp_env.decay,
        amp_sustain: p.amp_env.sustain,
        amp_release: p.amp_env.release,
        filter_attack: p.filter_env.attack,
        filter_decay: p.filter_env.decay,
        filter_sustain: p.filter_env.sustain,
        filter_release: p.filter_env.release,
        lfo_rate_hz: p.lfo.rate_hz,
        lfo_to_pitch_cents: p.lfo.to_pitch_cents,
        lfo_to_cutoff_oct: p.lfo.to_cutoff_oct,
        lfo_to_amp: p.lfo.to_amp,
        glide: p.voice.glide,
        unison_detune_cents: p.voice.unison_detune_cents,
        output: p.output,
    }
}

// ---------------------------------------------------------------- devices & recording
//
// Contract for recording/input (docs/adr/0002-recording.md, engine/README.md
// "Input, monitoring and recording").

/// An audio device as reported by the platform backend.
#[derive(Debug, Clone, PartialEq)]
pub struct AudioDeviceInfo {
    /// Stable identifier (Core Audio device UID on macOS).
    pub id: String,
    pub name: String,
    pub input_channels: u32,
    pub output_channels: u32,
    pub sample_rates: Vec<f64>,
    pub is_default_input: bool,
    pub is_default_output: bool,
}

/// Requested device setup (Settings › Audio). `None`/0 = system default,
/// except `input`:
///
/// `input`: `None` = **no input** (output only; nothing ever opens the
/// microphone), `Some(id)` = that input device, `Some(DEFAULT_INPUT)` (`""`)
/// = the system default input. An input is opened only when one is named
/// here, so a fresh engine or an output-only change never triggers the macOS
/// microphone prompt. `sample_rate` is the device's nominal rate: with an
/// input it must be 0 or the engine rate (the input path does not resample);
/// output-only, 0 leaves the device alone and another rate is converted by
/// the output unit. The engine rate itself never changes.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DeviceConfig {
    pub output: Option<String>,
    pub input: Option<String>,
    pub sample_rate: f64,
    pub buffer_size: u32,
}

impl DeviceConfig {
    /// `input: Some(DEFAULT_INPUT.into())` selects the system default input.
    pub const DEFAULT_INPUT: &'static str = "";
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct DeviceStatus {
    pub running: bool,
    pub output_name: String,
    pub input_name: Option<String>,
    pub sample_rate: f64,
    pub buffer_size: u32,
    pub input_channels: u32,
    pub output_channels: u32,
    /// Input-side latency in engine frames (device, safety offset, stream
    /// and one buffer). Recorded takes are compensated by
    /// `input_latency_frames + output_latency_frames`.
    pub input_latency_frames: u32,
    pub output_latency_frames: u32,
}

/// A finished recording on one track.
#[derive(Debug, Clone, PartialEq)]
pub struct RecordedTake {
    pub track_id: u64,
    /// 32-bit float WAV at the engine sample rate.
    pub path: PathBuf,
    /// Timeline frame of the first sample (latency compensated).
    pub start_frame: i64,
    pub frames: u64,
    pub channels: u32,
    pub sample_rate: f64,
}

/// A take still being recorded.
#[derive(Debug, Clone, PartialEq)]
pub struct LiveTake {
    pub track_id: u64,
    pub start_frame: i64,
    pub frames: u64,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct RecordingState {
    pub active: bool,
    pub takes: Vec<LiveTake>,
}

/// One MIDI input port ([`Engine::midi_inputs`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MidiInputInfo {
    /// Stable id: what [`Engine::open_midi_inputs`] takes and preferences store.
    pub id: String,
    pub name: String,
    pub open: bool,
    /// Messages received since it was opened (an activity light compares two readings).
    pub events: u64,
}

/// A note event heard while MIDI capture was on ([`Engine::read_midi_capture`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CapturedNote {
    /// Timeline frame at the engine rate (latency compensated; may be < 0).
    pub frame: i64,
    /// True for note-on, false for note-off.
    pub on: bool,
    pub pitch: u8,
    pub velocity: u8,
}

/// MIDI input counters ([`Engine::midi_stats`]).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MidiStats {
    pub received: u64,
    pub delivered: u64,
    pub dropped: u64,
    pub capture_dropped: u64,
}

/// Recording health counters ([`Engine::recording_stats`]).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RecordingStats {
    /// Audio blocks dropped because a track's ring was full (writer too slow).
    pub overruns: u64,
    pub dropped_frames: u64,
    /// Takes that could not be written completely (left out of the result).
    pub write_errors: u64,
    pub takes: u64,
}

fn c_chars_to_string(chars: &[std::ffi::c_char]) -> String {
    let bytes: Vec<u8> = chars.iter().take_while(|&&c| c != 0).map(|&c| c as u8).collect();
    String::from_utf8_lossy(&bytes).into_owned()
}

impl Engine {
    /// Enumerate audio devices (reads properties only; never opens a device).
    pub fn list_devices(&self) -> Result<Vec<AudioDeviceInfo>, EngineError> {
        let mut total = 0usize;
        check(unsafe { ffi::se_device_list(std::ptr::null_mut(), 0, &mut total) })?;
        loop {
            let mut raw = vec![ffi::se_device_info::default(); total];
            let mut n = 0usize;
            check(unsafe { ffi::se_device_list(raw.as_mut_ptr(), raw.len(), &mut n) })?;
            if n > raw.len() {
                total = n; // a device appeared in between: retry
                continue;
            }
            return Ok(raw[..n]
                .iter()
                .map(|d| AudioDeviceInfo {
                    id: c_chars_to_string(&d.uid),
                    name: c_chars_to_string(&d.name),
                    input_channels: d.input_channels,
                    output_channels: d.output_channels,
                    sample_rates: d.sample_rates[..(d.sample_rate_count as usize).min(ffi::SE_DEVICE_MAX_RATES)].to_vec(),
                    is_default_input: d.is_default_input != 0,
                    is_default_output: d.is_default_output != 0,
                })
                .collect());
        }
    }

    /// Apply a device configuration. Everything is validated first without
    /// opening anything (unknown id → `SE_ERR_NOT_FOUND`, unsupported
    /// rate/buffer/combination → `SE_ERR_INVALID_ARG`). If the device is
    /// stopped the setup is used by the next [`Engine::start_device`]; if it
    /// runs it is restarted. If the output opens but the input cannot (e.g.
    /// microphone permission denied), output keeps running, `device_status()
    /// .input_name` is `None`, [`Engine::input_error`] says why, and a
    /// running restart returns `SE_ERR_DEVICE` with that message.
    pub fn configure_device(&self, cfg: &DeviceConfig) -> Result<(), EngineError> {
        let out = cfg.output.as_deref().map(CString::new).transpose().map_err(|_| nul_error())?;
        let input_enabled = cfg.input.is_some();
        let inp = cfg.input.as_deref().filter(|s| !s.is_empty()).map(CString::new).transpose().map_err(|_| nul_error())?;
        let c = ffi::se_device_config {
            output_uid: out.as_ref().map_or(std::ptr::null(), |s| s.as_ptr()),
            input_uid: inp.as_ref().map_or(std::ptr::null(), |s| s.as_ptr()),
            sample_rate: cfg.sample_rate,
            buffer_size: cfg.buffer_size,
            input_enabled: input_enabled as u8,
            _pad: [0; 3],
        };
        // SAFETY: the strings outlive the call; the engine copies them.
        check(unsafe { ffi::se_device_configure(self.raw, &c) })
    }

    /// Current device status.
    pub fn device_status(&self) -> DeviceStatus {
        let mut st = ffi::se_device_status::default();
        if unsafe { ffi::se_device_status_get(self.raw, &mut st) } != ffi::SE_OK {
            return DeviceStatus { running: self.device_running(), sample_rate: self.sample_rate(), ..Default::default() };
        }
        DeviceStatus {
            running: st.running != 0,
            output_name: c_chars_to_string(&st.output_name),
            input_name: (st.input_open != 0).then(|| c_chars_to_string(&st.input_name)),
            sample_rate: st.sample_rate,
            buffer_size: st.buffer_size,
            input_channels: st.input_channels,
            output_channels: st.output_channels,
            input_latency_frames: st.input_latency_frames,
            output_latency_frames: st.output_latency_frames,
        }
    }

    /// Why the configured input is not open (permission denied, device
    /// gone, ...), if it is not.
    pub fn input_error(&self) -> Option<String> {
        let mut st = ffi::se_device_status::default();
        unsafe { ffi::se_device_status_get(self.raw, &mut st) };
        Some(c_chars_to_string(&st.input_error)).filter(|s| !s.is_empty())
    }

    /// Audition an asset from `start_frame` (0 = its start), independent of the transport. It
    /// mixes into master after the track sum (so the master meters show it),
    /// never reaches exports, replaces any running preview with a short
    /// crossfade and stops at the asset's end. `gain` 0..2, `rate`
    /// 0.25..4 (1 = original speed and pitch); `solo` silences the
    /// arrangement while the preview sounds.
    pub fn preview_play(&self, asset_id: u64, start_frame: i64, gain: f32, rate: f32, solo: bool) -> Result<(), EngineError> {
        check(unsafe { ffi::se_preview_play(self.raw, asset_id, start_frame, gain, rate, solo as u8) })
    }

    /// Fade the preview out.
    pub fn preview_stop(&self) -> Result<(), EngineError> {
        check(unsafe { ffi::se_preview_stop(self.raw) })
    }

    pub fn preview_set(&self, gain: f32, rate: f32, solo: bool) -> Result<(), EngineError> {
        check(unsafe { ffi::se_preview_set(self.raw, gain, rate, solo as u8) })
    }

    /// Current preview voice; the peaks reset on each call.
    pub fn preview(&self) -> PreviewState {
        let mut p = ffi::se_preview_info::default();
        unsafe { ffi::se_preview_get(self.raw, &mut p) };
        PreviewState {
            asset_id: (p.playing != 0).then_some(p.asset_id),
            position: p.position,
            frames: p.frames,
            peak_l: p.peak_l,
            peak_r: p.peak_r,
            notes: p.notes,
        }
    }

    /// Preview a note through a track's instrument device (velocity 0 = off);
    /// `preview_stop` releases every held one.
    pub fn preview_note(&self, device_id: u64, pitch: u8, velocity: u8) -> Result<(), EngineError> {
        check(unsafe { ffi::se_preview_note(self.raw, device_id, pitch, velocity) })
    }

    /// MIDI input ports present now (rescans; the first call starts the
    /// platform MIDI client). No MIDI system: an empty list.
    pub fn midi_inputs(&self) -> Result<Vec<MidiInputInfo>, EngineError> {
        let mut total = 0usize;
        check(unsafe { ffi::se_midi_inputs_list(self.raw, std::ptr::null_mut(), 0, &mut total) })?;
        loop {
            let mut raw = vec![ffi::se_midi_input_info::default(); total];
            let mut n = 0usize;
            check(unsafe { ffi::se_midi_inputs_list(self.raw, raw.as_mut_ptr(), raw.len(), &mut n) })?;
            if n > raw.len() {
                total = n; // a port appeared in between: retry
                continue;
            }
            return Ok(raw[..n]
                .iter()
                .map(|p| MidiInputInfo {
                    id: c_chars_to_string(&p.id),
                    name: c_chars_to_string(&p.name),
                    open: p.open != 0,
                    events: p.events,
                })
                .collect());
        }
    }

    /// Opens exactly these MIDI input ports (ids that are not present are
    /// ignored) and closes the others; notes they held are released. An
    /// error names the ports that failed; the rest are open. An id with a
    /// NUL byte is an invalid argument (nothing is opened or closed).
    pub fn open_midi_inputs(&self, ids: &[String]) -> Result<(), EngineError> {
        let c = ids.iter().map(|i| CString::new(i.as_str())).collect::<Result<Vec<CString>, _>>().map_err(|_| EngineError {
            code: ffi::SE_ERR_INVALID_ARG,
            kind: "invalid argument",
            message: "MIDI input id contains NUL".into(),
        })?;
        let ptrs: Vec<*const std::os::raw::c_char> = c.iter().map(|s| s.as_ptr()).collect();
        check(unsafe { ffi::se_midi_inputs_open(self.raw, ptrs.as_ptr(), ptrs.len()) })
    }

    /// Delivers a channel message as if a MIDI port had sent it (tests).
    pub fn midi_inject(&self, status: u8, data1: u8, data2: u8) -> Result<(), EngineError> {
        check(unsafe { ffi::se_midi_inject(self.raw, status, data1, data2) })
    }

    /// The instrument track that plays MIDI input while no instrument track
    /// is armed or monitoring (the selected track); `None` = none.
    pub fn set_midi_focus(&self, track_id: Option<u64>) {
        unsafe { ffi::se_midi_set_focus(self.raw, track_id.unwrap_or(0)) };
    }

    /// MIDI capture for recording: enabling empties the capture ring.
    pub fn set_midi_capture(&self, enabled: bool) {
        unsafe { ffi::se_midi_capture_enable(self.raw, enabled as u8) };
    }

    /// Pops the notes captured since the last read, in order.
    pub fn read_midi_capture(&self) -> Vec<CapturedNote> {
        let mut out = Vec::new();
        let mut buf = [ffi::se_midi_captured::default(); 256];
        loop {
            let mut n = 0usize;
            if unsafe { ffi::se_midi_capture_read(self.raw, buf.as_mut_ptr(), buf.len(), &mut n) } != ffi::SE_OK {
                break;
            }
            out.extend(buf[..n].iter().map(|c| CapturedNote { frame: c.frame, on: c.status == 0x90, pitch: c.data1, velocity: c.data2 }));
            if n < buf.len() {
                break;
            }
        }
        out
    }

    /// Whether the transport wrapped at the loop end during the latest capture pass
    /// (a real wrap, never a seek back); still answers after capture is disabled.
    pub fn midi_capture_wrapped(&self) -> bool {
        let mut w = 0u8;
        let rc = unsafe { ffi::se_midi_capture_wrapped(self.raw, &mut w) };
        rc == ffi::SE_OK && w != 0
    }

    pub fn midi_stats(&self) -> MidiStats {
        let mut s = ffi::se_midi_stats::default();
        unsafe { ffi::se_midi_stats_get(self.raw, &mut s) };
        MidiStats { received: s.received, delivered: s.delivered, dropped: s.dropped, capture_dropped: s.capture_dropped }
    }

    /// Silence all device input (Settings › "Mute audio input"): monitoring,
    /// input meters and recordings get zeros (recordings keep their timing).
    pub fn set_input_muted(&self, muted: bool) {
        unsafe { ffi::se_input_set_muted(self.raw, muted as u8) };
    }

    /// Start capturing every armed track that has an input (as published),
    /// from the current transport position at the next audio block; starts
    /// the transport if stopped. Takes are written to `dir` (created if
    /// missing) by a non-real-time writer thread. Errors with
    /// `SE_ERR_STATE` when already recording, when no armed track has an
    /// input, or when the device runs without an open input.
    pub fn start_recording(&self, dir: &Path) -> Result<(), EngineError> {
        let p = cpath(dir)?;
        check(unsafe { ffi::se_record_start(self.raw, p.as_ptr()) })
    }

    /// Stop capturing (the transport keeps its state) and return the
    /// finalized takes. A seek/stop during recording ends a take and starts
    /// a new one at the new position; looping records linearly (one take).
    pub fn stop_recording(&self) -> Result<Vec<RecordedTake>, EngineError> {
        let mut n = 0usize;
        check(unsafe { ffi::se_record_stop(self.raw, &mut n) })?;
        let mut takes = Vec::with_capacity(n);
        for i in 0..n {
            let mut t = ffi::se_recorded_take::default();
            check(unsafe { ffi::se_record_get_take(self.raw, i, &mut t) })?;
            takes.push(RecordedTake {
                track_id: t.track_id,
                path: PathBuf::from(c_chars_to_string(&t.path)),
                start_frame: t.start_frame,
                frames: t.frames,
                channels: t.channels,
                sample_rate: t.sample_rate,
            });
        }
        Ok(takes)
    }

    /// Live recording state for drawing growing clips (cheap; any thread).
    pub fn recording_state(&self) -> RecordingState {
        let mut active = 0u8;
        let mut n = 0usize;
        if unsafe { ffi::se_record_state(self.raw, &mut active, std::ptr::null_mut(), 0, &mut n) } != ffi::SE_OK {
            return RecordingState::default();
        }
        let mut raw = vec![ffi::se_live_take::default(); n + 8]; // slack for takes started in between
        let mut m = 0usize;
        unsafe { ffi::se_record_state(self.raw, &mut active, raw.as_mut_ptr(), raw.len(), &mut m) };
        raw.truncate(m.min(raw.len()));
        RecordingState {
            active: active != 0,
            takes: raw.iter().map(|t| LiveTake { track_id: t.track_id, start_frame: t.start_frame, frames: t.frames }).collect(),
        }
    }

    /// Min/max peaks of a take in progress (mono mix), for the live
    /// waveform: the newest take of `track_id`, from its first frame, at
    /// 64-frame resolution (`frames_per_peak` is rounded to multiples of 64).
    pub fn recording_peaks(&self, track_id: u64, frames_per_peak: u32, max_pairs: usize) -> Vec<[f32; 2]> {
        let mut buf = vec![0f32; max_pairs * 2];
        let mut n = 0usize;
        if unsafe { ffi::se_record_peaks(self.raw, track_id, frames_per_peak.max(1), buf.as_mut_ptr(), max_pairs, &mut n) } != ffi::SE_OK {
            return Vec::new();
        }
        buf.truncate(n * 2);
        buf.chunks_exact(2).map(|c| [c[0], c[1]]).collect()
    }

    /// Overrun / error counters of the current recording (or of the last one
    /// after [`Engine::stop_recording`]).
    pub fn recording_stats(&self) -> RecordingStats {
        let mut s = ffi::se_record_stats::default();
        unsafe { ffi::se_record_stats_get(self.raw, &mut s) };
        RecordingStats { overruns: s.overruns, dropped_frames: s.dropped_frames, write_errors: s.write_errors, takes: s.takes }
    }

    /// Offline render with injected device input (tests): `input` is
    /// interleaved with `input_channels` channels and the same frame count as `out`/2.
    pub fn render_offline_with_input(&self, input: &[f32], input_channels: u32, out: &mut [f32]) -> Result<(), EngineError> {
        let frames = out.len() / 2;
        if input_channels == 0 || input.len() != frames * input_channels as usize {
            return Err(EngineError {
                code: ffi::SE_ERR_INVALID_ARG,
                kind: "invalid argument",
                message: "input length must be frames * input_channels".into(),
            });
        }
        check(unsafe { ffi::se_render_offline_with_input(self.raw, input.as_ptr(), input_channels, out.as_mut_ptr(), frames as u32) })
    }
}

fn nul_error() -> EngineError {
    EngineError { code: ffi::SE_ERR_INVALID_ARG, kind: "invalid argument", message: "string contains NUL".into() }
}

// ---------------------------------------------------------------- plugins
//
// Plugin hosting (docs/adr/0003-plugins.md, engine/README.md "Plugins").

/// One scanned plugin.
#[derive(Debug, Clone, PartialEq)]
pub struct PluginInfo {
    pub plugin: dstudio_core::PluginRef,
    pub has_editor: bool,
    pub audio_inputs: u32,
    pub audio_outputs: u32,
}

/// Result of a scan. `failed` lists plugins that crashed, timed out or
/// failed validation in the out-of-process scanner (they are blocklisted
/// until a forced rescan), as `(AU id or bundle path, reason)`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ScanReport {
    pub plugins: Vec<PluginInfo>,
    pub failed: Vec<(String, String)>,
    pub from_cache: bool,
}

#[derive(serde::Deserialize)]
struct RawPlugin {
    format: dstudio_core::PluginFormat,
    id: String,
    path: String,
    name: String,
    vendor: String,
    version: String,
    kind: dstudio_core::PluginKind,
    has_editor: bool,
    audio_inputs: u32,
    audio_outputs: u32,
}

#[derive(serde::Deserialize)]
struct RawFailed {
    target: String,
    reason: String,
}

#[derive(serde::Deserialize)]
struct RawReport {
    plugins: Vec<RawPlugin>,
    failed: Vec<RawFailed>,
    from_cache: bool,
}

/// File name of the out-of-process scanner executable.
pub const PLUGIN_SCANNER_NAME: &str = if cfg!(windows) { "dstudio_plugin_scanner.exe" } else { "dstudio_plugin_scanner" };

/// Path of the out-of-process scanner: `$DSTUDIO_PLUGIN_SCANNER` if set, else
/// the scanner next to the running executable (where the release packages
/// put it: the install folder, or `Contents/MacOS` in the app bundle), else
/// the executable built next to the engine library by build.rs (a build run
/// from its target directory).
pub fn plugin_scanner_path() -> PathBuf {
    let exe_dir = std::env::current_exe().ok().and_then(|p| p.parent().map(Path::to_path_buf));
    resolve_plugin_scanner(
        std::env::var_os("DSTUDIO_PLUGIN_SCANNER").map(PathBuf::from),
        exe_dir.as_deref(),
        Path::new(env!("DSTUDIO_PLUGIN_SCANNER_BUILT")),
    )
}

fn resolve_plugin_scanner(env: Option<PathBuf>, exe_dir: Option<&Path>, built: &Path) -> PathBuf {
    if let Some(p) = env.filter(|p| !p.as_os_str().is_empty()) {
        return p;
    }
    if let Some(sibling) = exe_dir.map(|d| d.join(PLUGIN_SCANNER_NAME)).filter(|p| p.is_file()) {
        return sibling;
    }
    built.to_path_buf()
}

/// Directory holding the test plugin bundles built with the engine
/// (DStudioTestGain.clap/.vst3, DStudioTestSine.clap, DStudioTestCrash.clap).
/// For tests and diagnostics.
pub fn test_plugin_dir() -> PathBuf {
    PathBuf::from(env!("DSTUDIO_TEST_PLUGIN_DIR"))
}

/// Scan AU/VST3/CLAP plugins in the standard locations (plus `$CLAP_PATH`
/// on Windows and Linux, and `$DSTUDIO_PLUGIN_PATH`) using the out-of-process
/// scanner, caching results in `cache_path` (JSON). With `rescan == false`
/// unchanged bundles (and blocklisted ones) come from the cache. Blocking:
/// call from a worker thread. `progress` receives a short status per plugin.
pub fn scan_plugins(cache_path: &Path, rescan: bool, progress: &mut dyn FnMut(&str)) -> Result<ScanReport, EngineError> {
    scan_plugins_in(cache_path, rescan, &[], progress)
}

/// [`scan_plugins`] that also searches `extra_dirs` (the user's custom plugin
/// folders, every format), ahead of `$DSTUDIO_PLUGIN_PATH`. A folder that does
/// not exist is skipped.
pub fn scan_plugins_in(
    cache_path: &Path,
    rescan: bool,
    extra_dirs: &[PathBuf],
    progress: &mut dyn FnMut(&str),
) -> Result<ScanReport, EngineError> {
    use std::io::{BufRead, Read};
    let exe = plugin_scanner_path();
    if !exe.is_file() {
        return Err(EngineError {
            code: ffi::SE_ERR_NOT_FOUND,
            kind: "not found",
            message: format!("plugin scanner not found at {} (set DSTUDIO_PLUGIN_SCANNER)", exe.display()),
        });
    }
    let io_err = |e: std::io::Error| EngineError { code: ffi::SE_ERR_IO, kind: "i/o error", message: format!("plugin scanner: {e}") };
    let mut cmd = std::process::Command::new(&exe);
    cmd.arg("--scan").arg("--cache").arg(cache_path);
    if rescan {
        cmd.arg("--rescan");
    }
    // The scanner reads its extra folders from $DSTUDIO_PLUGIN_PATH: the custom ones go first. A
    // folder whose path holds the list separator cannot be passed (the app refuses to add one).
    let extra: Vec<&PathBuf> = extra_dirs.iter().filter(|d| std::env::join_paths([d]).is_ok()).collect();
    if !extra.is_empty() {
        let inherited = std::env::var_os("DSTUDIO_PLUGIN_PATH");
        let all = extra.into_iter().cloned().chain(inherited.iter().flat_map(std::env::split_paths));
        if let Ok(joined) = std::env::join_paths(all) {
            cmd.env("DSTUDIO_PLUGIN_PATH", joined);
        }
    }
    #[cfg(windows)]
    {
        // The scanner is a console program: no console window from a GUI app.
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let mut child = cmd
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(io_err)?;
    let mut stdout = child.stdout.take().expect("piped stdout");
    let reader = std::thread::spawn(move || {
        let mut s = String::new();
        stdout.read_to_string(&mut s).map(|_| s)
    });
    let mut errors = String::new();
    for line in std::io::BufReader::new(child.stderr.take().expect("piped stderr")).lines() {
        let line = line.map_err(io_err)?;
        match line.strip_prefix("progress: ") {
            Some(p) => progress(p),
            None => {
                errors.push_str(&line);
                errors.push('\n');
            }
        }
    }
    let status = child.wait().map_err(io_err)?;
    let out = reader.join().map_err(|_| io_err(std::io::Error::other("reader thread panicked")))?.map_err(io_err)?;
    if !status.success() {
        return Err(EngineError {
            code: ffi::SE_ERR_IO,
            kind: "i/o error",
            message: format!("plugin scanner failed ({status}): {}", errors.trim()),
        });
    }
    let raw: RawReport = serde_json::from_str(&out).map_err(|e| EngineError {
        code: ffi::SE_ERR_IO,
        kind: "i/o error",
        message: format!("plugin scanner output: {e}"),
    })?;
    Ok(ScanReport {
        plugins: raw
            .plugins
            .into_iter()
            .map(|p| PluginInfo {
                plugin: dstudio_core::PluginRef {
                    format: p.format,
                    id: p.id,
                    path: (!p.path.is_empty()).then_some(p.path),
                    name: p.name,
                    vendor: p.vendor,
                    version: p.version,
                    kind: p.kind,
                },
                has_editor: p.has_editor,
                audio_inputs: p.audio_inputs,
                audio_outputs: p.audio_outputs,
            })
            .collect(),
        failed: raw.failed.into_iter().map(|f| (f.target, f.reason)).collect(),
        from_cache: raw.from_cache,
    })
}

impl Engine {
    /// Current state blobs (base64) of all live plugin instances, for
    /// `Document::sync_device_states` before saving. MAIN THREAD (VST3 and
    /// CLAP save state on the main thread; safe while audio runs).
    /// Instances whose save fails are left out.
    pub fn plugin_states(&self) -> Vec<(dstudio_core::DeviceId, String)> {
        let _saved = self.saved_state.lock().unwrap_or_else(|e| e.into_inner());
        let mut n = 0usize;
        // SAFETY: valid handle and out pointer.
        if unsafe { ffi::se_plugin_save_states(self.raw, &mut n) } != ffi::SE_OK {
            return Vec::new();
        }
        let mut out = Vec::with_capacity(n);
        for i in 0..n {
            let (mut id, mut data, mut len) = (0u64, std::ptr::null::<u8>(), 0usize);
            // SAFETY: out pointers are valid; the data stays valid until the next save call, which
            // `saved_state` holds off until we have copied it.
            if unsafe { ffi::se_plugin_saved_state(self.raw, i, &mut id, &mut data, &mut len) } == ffi::SE_OK {
                let bytes = if len == 0 { &[][..] } else { unsafe { std::slice::from_raw_parts(data, len) } };
                out.push((dstudio_core::DeviceId(id), dstudio_core::device::b64::encode(bytes)));
            }
        }
        out
    }

    /// The current state of one live plugin instance (the bytes a save would
    /// store, before base64), or `None` when the device has no live instance.
    /// An instance that fails to save is an error. MAIN THREAD, like
    /// [`Engine::plugin_states`], but one instance per call.
    pub fn plugin_state_of(&self, device: dstudio_core::DeviceId) -> Result<Option<Vec<u8>>, EngineError> {
        let _saved = self.saved_state.lock().unwrap_or_else(|e| e.into_inner());
        // SAFETY: valid handle.
        match unsafe { ffi::se_plugin_save_state(self.raw, device.0) } {
            ffi::SE_ERR_NOT_FOUND => return Ok(None),
            rc => check(rc)?,
        }
        let (mut id, mut data, mut len) = (0u64, std::ptr::null::<u8>(), 0usize);
        // SAFETY: out pointers are valid; the data stays valid until the next save call, which
        // `saved_state` holds off until we have copied it.
        check(unsafe { ffi::se_plugin_saved_state(self.raw, 0, &mut id, &mut data, &mut len) })?;
        Ok(Some(if len == 0 { Vec::new() } else { unsafe { std::slice::from_raw_parts(data, len) }.to_vec() }))
    }

    /// Instantiates a plugin device now with `state` (its saved state, or the
    /// live instance's current one), so that a later [`Engine::publish`] of a
    /// session with this device from another thread reuses the instance
    /// instead of creating it. `instrument`: the device is a track's
    /// instrument plugin. MAIN THREAD. A plugin that fails to load is an
    /// error with the plugin's reason. See [`Engine::set_plugin_detached`].
    pub fn prepare_plugin(&self, device: &dstudio_core::Device, instrument: bool, state: Option<Vec<u8>>) -> Result<(), EngineError> {
        let store = store_of(device, state)?;
        let desc = store.desc();
        // SAFETY: valid handle; `desc` points into `store`, which outlives the call; the engine copies.
        check(unsafe { ffi::se_plugin_prepare(self.raw, &desc, instrument as u8) })
    }

    /// Detaches publish and housekeeping from the main thread (engine/README.md,
    /// "Plugins in offline renders"): after this the engine never instantiates,
    /// restarts or destroys plugin instances on whichever thread publishes or
    /// renders it; a device that was not prepared fails to load. The instances
    /// are torn down when the engine is dropped, which then has to happen on
    /// the main thread.
    pub fn set_plugin_detached(&self, detached: bool) {
        // SAFETY: valid handle.
        unsafe { ffi::se_plugin_set_detached(self.raw, detached as u8) };
    }

    /// Devices whose plugin could not be instantiated (missing, failed to
    /// load, wrong format) with a reason. They are passed through silently.
    pub fn plugin_load_errors(&self) -> Vec<(dstudio_core::DeviceId, String)> {
        loop {
            let mut total = 0usize;
            // SAFETY: capacity 0 only queries the count.
            if unsafe { ffi::se_plugin_load_errors(self.raw, std::ptr::null_mut(), 0, &mut total) } != ffi::SE_OK {
                return Vec::new();
            }
            let mut raw = vec![ffi::se_plugin_message::default(); total];
            let mut n = 0usize;
            // SAFETY: `raw` has `total` elements.
            if unsafe { ffi::se_plugin_load_errors(self.raw, raw.as_mut_ptr(), raw.len(), &mut n) } != ffi::SE_OK {
                return Vec::new();
            }
            if n > raw.len() {
                continue; // changed in between
            }
            return raw[..n].iter().map(|m| (dstudio_core::DeviceId(m.device_id), c_chars_to_string(&m.message))).collect();
        }
    }

    /// Open the plugin's native editor in its own window (or bring it to
    /// the front). MAIN THREAD ONLY (`SE_ERR_STATE` otherwise).
    pub fn open_plugin_editor(&self, device: dstudio_core::DeviceId) -> Result<(), EngineError> {
        // SAFETY: valid handle.
        check(unsafe { ffi::se_plugin_editor_open(self.raw, device.0) })
    }

    /// Close the editor window if open. MAIN THREAD ONLY.
    pub fn close_plugin_editor(&self, device: dstudio_core::DeviceId) {
        // SAFETY: valid handle.
        unsafe { ffi::se_plugin_editor_close(self.raw, device.0) };
    }

    /// Whether the device's editor window is open (false after the user
    /// closed it with the window's close button).
    pub fn plugin_editor_is_open(&self, device: dstudio_core::DeviceId) -> bool {
        // SAFETY: valid handle.
        unsafe { ffi::se_plugin_editor_is_open(self.raw, device.0) != 0 }
    }

    /// Reported processing latency of a device, in frames (0 if unknown).
    pub fn plugin_latency(&self, device: dstudio_core::DeviceId) -> u32 {
        let mut frames = 0u32;
        // SAFETY: valid handle and out pointer.
        unsafe { ffi::se_plugin_latency(self.raw, device.0, &mut frames) };
        frames
    }

    /// Main-thread housekeeping (optional; also done by `publish` and the
    /// other plugin calls): destroys retired instances the audio thread can
    /// no longer reach and runs pending CLAP main-thread callbacks. On Linux
    /// it also runs the plugins' fd and timer callbacks (their windows' event
    /// loop), so call it at the rate a plugin window should repaint.
    pub fn plugin_idle(&self) {
        // SAFETY: valid handle.
        unsafe { ffi::se_plugin_idle(self.raw) };
    }
}

/// Largest delay plugin delay compensation applies, in frames (2^17,
/// ~2.7 s at 48 kHz). Larger latencies are clamped and reported.
pub const PDC_MAX_DELAY_FRAMES: u32 = ffi::SE_PDC_MAX_DELAY_FRAMES;

/// Plugin delay compensation state of the published session
/// ([`Engine::latency_report`]).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LatencyReport {
    /// Applied compensation L in frames: the whole mix (and the metronome)
    /// is this much late relative to the transport. 0 when disabled.
    pub total_frames: u32,
    /// `(track_id, lat_i)` in session order: the summed reported latency of
    /// the track's devices (bypassed ones included) and instrument plugin.
    /// Reported even when compensation is disabled.
    pub tracks: Vec<(u64, u32)>,
    /// Some latency exceeded [`PDC_MAX_DELAY_FRAMES`] and was clamped (those
    /// tracks are not fully aligned). Only set while enabled.
    pub clamped: bool,
}

impl Engine {
    /// Global switch for plugin delay compensation (on by default). Off, the
    /// engine renders exactly as without compensation. Any thread; re-plans
    /// the published session immediately (from the latencies cached on the
    /// main thread).
    pub fn set_delay_compensation(&self, enabled: bool) {
        // SAFETY: valid handle.
        unsafe { ffi::se_pdc_set_enabled(self.raw, enabled as u8) };
    }

    pub fn delay_compensation_enabled(&self) -> bool {
        // SAFETY: valid handle.
        unsafe { ffi::se_pdc_is_enabled(self.raw) != 0 }
    }

    /// Latencies and the applied compensation of the most recently published
    /// session. Latency changes reported by plugins are picked up by
    /// [`Engine::plugin_idle`] (call it regularly on the main thread) and by
    /// `publish`.
    pub fn latency_report(&self) -> LatencyReport {
        loop {
            let mut r = ffi::se_pdc_report::default();
            let mut total = 0usize;
            // SAFETY: capacity 0 only queries the count.
            if unsafe { ffi::se_pdc_report_get(self.raw, &mut r, std::ptr::null_mut(), 0, &mut total) } != ffi::SE_OK {
                return LatencyReport::default();
            }
            let mut tracks = vec![ffi::se_pdc_track::default(); total];
            let mut n = 0usize;
            // SAFETY: `tracks` has `total` elements.
            if unsafe { ffi::se_pdc_report_get(self.raw, &mut r, tracks.as_mut_ptr(), tracks.len(), &mut n) } != ffi::SE_OK {
                return LatencyReport::default();
            }
            if n > tracks.len() {
                continue; // republished in between
            }
            tracks.truncate(n);
            return LatencyReport {
                total_frames: r.total_frames,
                tracks: tracks.iter().map(|t| (t.track_id, t.latency_frames)).collect(),
                clamped: r.clamped != 0,
            };
        }
    }

    /// The timeline frame being heard: while playing, the transport position
    /// minus the applied compensation L minus the output device latency
    /// (`DeviceStatus::output_latency_frames`; 0 offline or with the device
    /// stopped). While paused/stopped, the transport position. Block
    /// granularity, like [`Engine::transport`].
    pub fn audible_position(&self) -> i64 {
        let mut f = 0i64;
        // SAFETY: valid handle and out pointer.
        unsafe { ffi::se_transport_audible_position(self.raw, &mut f) };
        f
    }
}

// ---------------------------------------------------------------- automation
//
// Contract for automation (docs/adr/0005-automation.md). Lanes travel in the
// session snapshot (`TrackSnapshot::automation`, `SessionSnapshot::master_automation`).
// Lanes are evaluated by the engine exactly like `AutomationLane::value_at`.

/// A plugin parameter that can be automated.
#[derive(Debug, Clone, PartialEq)]
pub struct PluginParamInfo {
    /// Format parameter id (VST3 ParamID, CLAP clap_id, AU AudioUnitParameterID).
    pub id: u32,
    pub name: String,
    /// Group/module name for menus (may be empty).
    pub group: String,
    /// Default as a normalized 0..1 value.
    pub default_normalized: f32,
    /// Number of discrete steps (0 = continuous).
    pub steps: u32,
    pub automatable: bool,
}

/// A parameter change made inside a plugin's own editor (for "write"
/// automation), normalized 0..1, with gesture boundaries where the format
/// reports them.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PluginParamChange {
    pub device: dstudio_core::DeviceId,
    pub param: u32,
    pub normalized: f32,
    /// Engine transport frame when the change was observed.
    pub frame: i64,
    pub gesture_begin: bool,
    pub gesture_end: bool,
}

impl Engine {
    /// Parameters of a live plugin instance (empty if not loaded), in the
    /// plugin's order, hidden ones left out. MAIN THREAD (the publishing
    /// thread; VST3/CLAP parameter queries are main-thread APIs).
    pub fn plugin_params(&self, device: dstudio_core::DeviceId) -> Vec<PluginParamInfo> {
        loop {
            let mut total = 0usize;
            // SAFETY: capacity 0 only queries the count.
            if unsafe { ffi::se_plugin_params(self.raw, device.0, std::ptr::null_mut(), 0, &mut total) } != ffi::SE_OK {
                return Vec::new();
            }
            let mut raw = vec![ffi::se_plugin_param_info::default(); total];
            let mut n = 0usize;
            // SAFETY: `raw` has `total` elements.
            if unsafe { ffi::se_plugin_params(self.raw, device.0, raw.as_mut_ptr(), raw.len(), &mut n) } != ffi::SE_OK {
                return Vec::new();
            }
            if n > raw.len() {
                continue; // changed in between
            }
            return raw[..n]
                .iter()
                .map(|p| PluginParamInfo {
                    id: p.id,
                    name: c_chars_to_string(&p.name),
                    group: c_chars_to_string(&p.group),
                    default_normalized: p.default_normalized,
                    steps: p.steps,
                    automatable: p.automatable != 0,
                })
                .collect();
        }
    }

    /// Current normalized value of a plugin parameter. MAIN THREAD.
    pub fn plugin_param_value(&self, device: dstudio_core::DeviceId, param: u32) -> Option<f32> {
        let mut v = 0f32;
        // SAFETY: valid handle and out pointer.
        (unsafe { ffi::se_plugin_param_value(self.raw, device.0, param, &mut v) } == ffi::SE_OK).then_some(v)
    }

    /// Display text for a normalized value ("-6.0 dB", "1.2 kHz"). MAIN THREAD.
    pub fn plugin_param_text(&self, device: dstudio_core::DeviceId, param: u32, normalized: f32) -> Option<String> {
        let mut buf = [0 as std::ffi::c_char; 256];
        // SAFETY: `buf` is valid for its length; the engine NUL-terminates.
        (unsafe { ffi::se_plugin_param_text(self.raw, device.0, param, normalized, buf.as_mut_ptr(), buf.len()) } == ffi::SE_OK)
            .then(|| c_chars_to_string(&buf))
    }

    /// Drain parameter changes made in plugin editors since the last call
    /// (main thread; call from the UI poll). A gesture arrives as a record
    /// with `gesture_begin` (value at that moment), value changes, and a
    /// record with `gesture_end`; formats without gestures send values only.
    /// `frame` is the transport position when the change was observed.
    pub fn take_plugin_param_changes(&self) -> Vec<PluginParamChange> {
        let mut out = Vec::new();
        let mut buf = [ffi::se_plugin_param_change::default(); 256];
        loop {
            let mut n = 0usize;
            // SAFETY: `buf` is valid for its length.
            if unsafe { ffi::se_plugin_take_param_changes(self.raw, buf.as_mut_ptr(), buf.len(), &mut n) } != ffi::SE_OK {
                return out;
            }
            out.extend(buf[..n].iter().map(|c| PluginParamChange {
                device: dstudio_core::DeviceId(c.device_id),
                param: c.param_id,
                normalized: c.normalized,
                frame: c.frame,
                gesture_begin: c.gesture_begin != 0,
                gesture_end: c.gesture_end != 0,
            }));
            if n < buf.len() {
                return out;
            }
        }
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        // SAFETY: we own the handle exclusively here.
        unsafe { ffi::se_engine_destroy(self.raw) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dstudio_core::instrument::{Adsr, Filter, Lfo, Osc, Voice, factory_presets};

    #[test]
    fn plugin_scanner_resolution_order() {
        let dir = tempfile::tempdir().unwrap();
        let built = Path::new("/build/out/dstudio_plugin_scanner");
        // No scanner next to the executable: the build-time path.
        assert_eq!(resolve_plugin_scanner(None, Some(dir.path()), built), built);
        assert_eq!(resolve_plugin_scanner(None, None, built), built);
        // A packaged scanner next to the executable wins over the build path.
        let sibling = dir.path().join(PLUGIN_SCANNER_NAME);
        std::fs::write(&sibling, b"").unwrap();
        assert_eq!(resolve_plugin_scanner(None, Some(dir.path()), built), sibling);
        // $DSTUDIO_PLUGIN_SCANNER wins over both; an empty value is ignored.
        let custom = PathBuf::from("/custom/scanner");
        assert_eq!(resolve_plugin_scanner(Some(custom.clone()), Some(dir.path()), built), custom);
        assert_eq!(resolve_plugin_scanner(Some(PathBuf::new()), Some(dir.path()), built), sibling);
    }

    #[test]
    fn default_synth_params_map_to_engine_defaults() {
        let mut engine_default = ffi::se_synth_params::default();
        // SAFETY: valid out pointer.
        unsafe { ffi::se_synth_params_default(&mut engine_default) };
        assert_eq!(synth_params_to_ffi(&SynthParams::default()), engine_default);
    }

    #[test]
    fn synth_params_mapping_covers_every_field() {
        // Distinct, non-default values everywhere so a swapped field shows.
        let p = SynthParams {
            osc: [
                Osc { wave: Waveform::Triangle, shape: 0.11, octave: -2, semitones: 7, fine_cents: -12.5, level: 0.21 },
                Osc { wave: Waveform::Sine, shape: 0.12, octave: 3, semitones: -11, fine_cents: 33.0, level: 0.22 },
            ],
            cross_fm: 0.31,
            sub_level: 0.32,
            noise_level: 0.33,
            filter: Filter { mode: FilterMode::BandPass, cutoff_hz: 1234.0, resonance: 0.41, key_track: 0.42, env_amount: -0.43 },
            amp_env: Adsr { attack: 0.051, decay: 0.052, sustain: 0.53, release: 0.054 },
            filter_env: Adsr { attack: 0.061, decay: 0.062, sustain: 0.63, release: 0.064 },
            lfo: Lfo { wave: Waveform::Square, rate_hz: 7.1, to_pitch_cents: 72.0, to_cutoff_oct: 0.73, to_amp: 0.74 },
            voice: Voice { polyphony: 3, glide: 0.81, unison: 4, unison_detune_cents: 8.2 },
            output: 1.9,
            preset: "Mapping".into(),
        };
        assert!(p.validate().is_ok());
        let c = synth_params_to_ffi(&p);
        assert_eq!((c.osc1_wave, c.osc1_octave, c.osc1_semitones), (ffi::SE_WAVE_TRIANGLE, -2, 7));
        assert_eq!((c.osc2_wave, c.osc2_octave, c.osc2_semitones), (ffi::SE_WAVE_SINE, 3, -11));
        assert_eq!((c.osc1_shape, c.osc1_fine_cents, c.osc1_level), (0.11, -12.5, 0.21));
        assert_eq!((c.osc2_shape, c.osc2_fine_cents, c.osc2_level), (0.12, 33.0, 0.22));
        assert_eq!((c.cross_fm, c.sub_level, c.noise_level), (0.31, 0.32, 0.33));
        assert_eq!(c.filter_mode, ffi::SE_FILTER_BANDPASS);
        assert_eq!((c.filter_cutoff_hz, c.filter_resonance, c.filter_key_track, c.filter_env_amount), (1234.0, 0.41, 0.42, -0.43));
        assert_eq!((c.amp_attack, c.amp_decay, c.amp_sustain, c.amp_release), (0.051, 0.052, 0.53, 0.054));
        assert_eq!((c.filter_attack, c.filter_decay, c.filter_sustain, c.filter_release), (0.061, 0.062, 0.63, 0.064));
        assert_eq!(c.lfo_wave, ffi::SE_WAVE_SQUARE);
        assert_eq!((c.lfo_rate_hz, c.lfo_to_pitch_cents, c.lfo_to_cutoff_oct, c.lfo_to_amp), (7.1, 72.0, 0.73, 0.74));
        assert_eq!((c.polyphony, c.glide, c.unison, c.unison_detune_cents), (3, 0.81, 4, 8.2));
        assert_eq!(c.output, 1.9);
        assert_eq!(c._pad, [0; 6]);
        for (w, code) in [(Waveform::Saw, 0), (Waveform::Square, 1), (Waveform::Triangle, 2), (Waveform::Sine, 3)] {
            assert_eq!(wave_to_ffi(w), code);
        }
    }

    #[test]
    fn every_factory_preset_is_accepted_by_the_engine() {
        let e = Engine::new(Config::default()).unwrap();
        for p in factory_presets() {
            let snap = SessionSnapshot {
                tracks: vec![dstudio_core::snapshot::TrackSnapshot {
                    track_id: 1,
                    is_return: false,
                    output: None,
                    sends: Vec::new(),
                    gain: 1.0,
                    pan: 0.0,
                    trim: 1.0,
                    width: 1.0,
                    mono: false,
                    mute: false,
                    solo: false,
                    clips: vec![],
                    instrument: true,
                    notes: vec![],
                    arm: false,
                    monitor: false,
                    input: None,
                    synth: Some(p.clone()),
                    devices: vec![],
                    instrument_plugin: None,
                    automation: vec![],
                }],
                master_gain: 1.0,
                master_automation: vec![],
                device_assets: Default::default(),
                tempo: vec![],
                click_quarters: 1.0,
            };
            e.publish(&snap).unwrap_or_else(|err| panic!("{}: {err}", p.preset));
        }
        // Out-of-range values are rejected by the engine.
        let mut bad = SynthParams::default();
        bad.voice.unison = 9;
        let snap = SessionSnapshot {
            tracks: vec![dstudio_core::snapshot::TrackSnapshot {
                track_id: 1,
                is_return: false,
                output: None,
                sends: Vec::new(),
                gain: 1.0,
                pan: 0.0,
                trim: 1.0,
                width: 1.0,
                mono: false,
                mute: false,
                solo: false,
                clips: vec![],
                instrument: true,
                notes: vec![],
                arm: false,
                monitor: false,
                input: None,
                synth: Some(bad),
                devices: vec![],
                instrument_plugin: None,
                automation: vec![],
            }],
            master_gain: 1.0,
            master_automation: vec![],
            device_assets: Default::default(),
            tempo: vec![],
            click_quarters: 1.0,
        };
        assert_eq!(e.publish(&snap).unwrap_err().code, ffi::SE_ERR_INVALID_ARG);
    }
}
