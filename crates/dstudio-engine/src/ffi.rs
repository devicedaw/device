//! Raw declarations mirroring `engine/include/dstudio_engine.h` (ABI v1).
//! Hand-written to avoid a bindgen/libclang build dependency; layout is
//! checked by the `abi_layout` test.
#![allow(non_camel_case_types, dead_code)]

use std::ffi::c_char;

pub const SE_ABI_VERSION: u32 = 1;
pub const SE_MAX_FRAMES_PER_BEAT: f64 = 1.0e9;
pub const SE_MAX_TIMELINE_FRAME: i64 = 1 << 52;

pub const SE_OK: i32 = 0;
pub const SE_ERR_INVALID_ARG: i32 = -1;
pub const SE_ERR_IO: i32 = -2;
pub const SE_ERR_UNSUPPORTED_FORMAT: i32 = -3;
pub const SE_ERR_DEVICE: i32 = -4;
pub const SE_ERR_NOT_FOUND: i32 = -5;
pub const SE_ERR_STATE: i32 = -6;
pub const SE_ERR_OUT_OF_MEMORY: i32 = -7;

#[repr(C)]
pub struct se_engine {
    _private: [u8; 0],
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct se_config {
    pub abi_version: u32,
    pub sample_rate: f64,
    pub max_block_size: u32,
    pub output_channels: u32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct se_asset_info {
    pub asset_id: u64,
    pub frames: u64,
    pub channels: u32,
    pub source_sample_rate: f64,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct se_clip_desc {
    pub clip_id: u64,
    pub asset_id: u64,
    pub timeline_start: i64,
    pub source_offset: i64,
    pub length: i64,
    pub gain: f32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct se_track_desc {
    pub track_id: u64,
    pub gain: f32,
    pub pan: f32,
    pub mute: u8,
    pub solo: u8,
    pub _pad: [u8; 6],
    pub clips: *const se_clip_desc,
    pub clip_count: u32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct se_session_desc {
    pub tracks: *const se_track_desc,
    pub track_count: u32,
    pub master_gain: f32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct se_transport_info {
    pub position: i64,
    pub playing: u8,
    pub _pad: [u8; 7],
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct se_preview_info {
    pub playing: u8,
    pub notes: u8,
    pub reserved: [u8; 6],
    pub asset_id: u64,
    pub position: i64,
    pub frames: i64,
    pub peak_l: f32,
    pub peak_r: f32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct se_meter {
    pub track_id: u64,
    pub peak_l: f32,
    pub peak_r: f32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct se_stats {
    pub callbacks: u64,
    pub xruns_or_overloads: u64,
    pub last_callback_us: f64,
    pub max_callback_us: f64,
    pub avg_callback_us: f64,
    pub block_deadline_us: f64,
}

/// Appended to ABI v1 (see the "additions" section of dstudio_engine.h).
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct se_metronome_desc {
    pub enabled: u8,
    pub _pad: [u8; 3],
    pub gain: f32,
    pub frames_per_beat: f64,
    pub beats_per_bar: u32,
    pub _pad2: u32,
}

/// MIDI note for [`se_track_desc_v2`] (ABI v1 addition).
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct se_note_desc {
    pub start: i64,
    pub end: i64,
    pub pitch: u8,
    pub velocity: u8,
    pub _pad: [u8; 6],
}

/// v1 track fields plus the built-in instrument flag and notes.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct se_track_desc_v2 {
    pub track_id: u64,
    pub gain: f32,
    pub pan: f32,
    pub mute: u8,
    pub solo: u8,
    pub instrument: u8,
    pub _pad: [u8; 5],
    pub clips: *const se_clip_desc,
    pub clip_count: u32,
    pub note_count: u32,
    pub notes: *const se_note_desc,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct se_session_desc_v2 {
    pub tracks: *const se_track_desc_v2,
    pub track_count: u32,
    pub master_gain: f32,
}

/// Built-in synth parameters (ABI v1 addition), see `se_synth_params` in
/// dstudio_engine.h. Waveforms: 0 saw, 1 square, 2 triangle, 3 sine; filter
/// mode: 0 low-pass, 1 high-pass, 2 band-pass.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct se_synth_params {
    pub osc1_wave: u8,
    pub osc1_octave: i8,
    pub osc1_semitones: i8,
    pub osc2_wave: u8,
    pub osc2_octave: i8,
    pub osc2_semitones: i8,
    pub filter_mode: u8,
    pub lfo_wave: u8,
    pub polyphony: u8,
    pub unison: u8,
    pub _pad: [u8; 6],
    pub osc1_shape: f32,
    pub osc1_fine_cents: f32,
    pub osc1_level: f32,
    pub osc2_shape: f32,
    pub osc2_fine_cents: f32,
    pub osc2_level: f32,
    pub cross_fm: f32,
    pub sub_level: f32,
    pub noise_level: f32,
    pub filter_cutoff_hz: f32,
    pub filter_resonance: f32,
    pub filter_key_track: f32,
    pub filter_env_amount: f32,
    pub amp_attack: f32,
    pub amp_decay: f32,
    pub amp_sustain: f32,
    pub amp_release: f32,
    pub filter_attack: f32,
    pub filter_decay: f32,
    pub filter_sustain: f32,
    pub filter_release: f32,
    pub lfo_rate_hz: f32,
    pub lfo_to_pitch_cents: f32,
    pub lfo_to_cutoff_oct: f32,
    pub lfo_to_amp: f32,
    pub glide: f32,
    pub unison_detune_cents: f32,
    pub output: f32,
}

pub const SE_WAVE_SAW: u8 = 0;
pub const SE_WAVE_SQUARE: u8 = 1;
pub const SE_WAVE_TRIANGLE: u8 = 2;
pub const SE_WAVE_SINE: u8 = 3;
pub const SE_FILTER_LOWPASS: u8 = 0;
pub const SE_FILTER_HIGHPASS: u8 = 1;
pub const SE_FILTER_BANDPASS: u8 = 2;

/// v2 track fields plus a pointer to the track's synth parameters (NULL = defaults).
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct se_track_desc_v3 {
    pub track_id: u64,
    pub gain: f32,
    pub pan: f32,
    pub mute: u8,
    pub solo: u8,
    pub instrument: u8,
    pub _pad: [u8; 5],
    pub clips: *const se_clip_desc,
    pub clip_count: u32,
    pub note_count: u32,
    pub notes: *const se_note_desc,
    pub synth: *const se_synth_params,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct se_session_desc_v3 {
    pub tracks: *const se_track_desc_v3,
    pub track_count: u32,
    pub master_gain: f32,
}

// ---- audio input, monitoring, recording, devices (ABI v1 addition)

pub const SE_INPUT_NONE: u8 = 0;
pub const SE_INPUT_MONO: u8 = 1;
pub const SE_INPUT_STEREO: u8 = 2;

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct se_track_desc_v4 {
    pub track_id: u64,
    pub gain: f32,
    pub pan: f32,
    pub mute: u8,
    pub solo: u8,
    pub instrument: u8,
    pub arm: u8,
    pub monitor: u8,
    pub input_mode: u8,
    pub input_channel: u16,
    pub clips: *const se_clip_desc,
    pub clip_count: u32,
    pub note_count: u32,
    pub notes: *const se_note_desc,
    pub synth: *const se_synth_params,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct se_session_desc_v4 {
    pub tracks: *const se_track_desc_v4,
    pub track_count: u32,
    pub master_gain: f32,
}

pub const SE_PLUGIN_AU: u8 = 0;
pub const SE_PLUGIN_VST3: u8 = 1;
pub const SE_PLUGIN_CLAP: u8 = 2;
pub const SE_PLUGIN_BUILTIN: u8 = 3;
pub const SE_PLUGIN_MESSAGE_MAX: usize = 256;

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct se_device_desc {
    pub device_id: u64,
    pub name: *const std::ffi::c_char,
    pub plugin_id: *const std::ffi::c_char,
    pub path: *const std::ffi::c_char,
    pub state: *const u8,
    pub state_len: usize,
    pub format: u8,
    pub enabled: u8,
    pub _pad: [u8; 6],
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct se_track_desc_v5 {
    pub track_id: u64,
    pub gain: f32,
    pub pan: f32,
    pub mute: u8,
    pub solo: u8,
    pub instrument: u8,
    pub arm: u8,
    pub monitor: u8,
    pub input_mode: u8,
    pub input_channel: u16,
    pub clips: *const se_clip_desc,
    pub clip_count: u32,
    pub note_count: u32,
    pub notes: *const se_note_desc,
    pub synth: *const se_synth_params,
    pub devices: *const se_device_desc,
    pub device_count: u32,
    pub _pad2: u32,
    pub instrument_plugin: *const se_device_desc,
    pub track_name: *const std::ffi::c_char,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct se_session_desc_v5 {
    pub tracks: *const se_track_desc_v5,
    pub track_count: u32,
    pub master_gain: f32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct se_plugin_message {
    pub device_id: u64,
    pub message: [std::ffi::c_char; SE_PLUGIN_MESSAGE_MAX],
}

impl Default for se_plugin_message {
    fn default() -> Self {
        se_plugin_message { device_id: 0, message: [0; SE_PLUGIN_MESSAGE_MAX] }
    }
}

// ---- automation (se_session_publish_v6) and plugin parameters

pub const SE_AUTO_VOLUME: u8 = 0;
pub const SE_AUTO_PAN: u8 = 1;
pub const SE_AUTO_SYNTH: u8 = 2;
pub const SE_AUTO_PLUGIN: u8 = 3;
pub const SE_AUTO_BUILTIN: u8 = 4;
pub const SE_AUTO_MUTE: u8 = 5;
pub const SE_AUTO_DEVICE_ON: u8 = 6;
pub const SE_AUTO_TRIM: u8 = 7;
pub const SE_AUTO_WIDTH: u8 = 8;
pub const SE_AUTO_MONO: u8 = 9;
pub const SE_AUTO_FLAG_STEPPED: u32 = 1;

// ---- built-in devices (state blob, registry)
pub const SE_BUILTIN_STATE_MAGIC: u32 = 0x3149_4253;
pub const SE_BUILTIN_MAX_SIDECHAINS: usize = 8;
pub const SE_MOD_GRID_FRAMES: u32 = 32;
pub const SE_MOD_BUILTIN: u8 = 0;
pub const SE_MOD_PLUGIN: u8 = 1;

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct se_mod_route {
    pub target_device_id: u64,
    pub param_id: u32,
    pub kind: u8,
    pub slot: u8,
    pub absolute: u8,
    pub log: u8,
    pub min: f32,
    pub max: f32,
    pub p_min: f32,
    pub p_max: f32,
    pub p_skew: f32,
    pub p_step: f32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct se_builtin_state {
    pub magic: u32,
    pub value_count: u32,
    pub sidechain_track_id: u64,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct se_builtin_desc {
    pub id: *const c_char,
    pub param_ids: *const *const c_char,
    pub readout_ids: *const *const c_char,
    pub param_count: u32,
    pub readout_count: u32,
    pub param_defaults: *const f32,
    pub sample_slots: u32,
    pub instrument: u8,
    pub _pad: [u8; 3],
}

pub const SE_BUILTIN_STATE_MAGIC2: u32 = 0x3249_4253;
pub const SE_BUILTIN_MAX_SLICES: usize = 128;

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct se_builtin_state_v2 {
    pub magic: u32,
    pub value_count: u32,
    pub sidechain_track_id: u64,
    pub sample_count: u32,
    pub slice_count: u32,
    pub route_count: u32,
    pub _pad: u32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct se_builtin_sample {
    pub asset_id: u64,
    pub slot: u32,
    pub slice_count: u32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct se_builtin_slice {
    pub start: f32,
    pub gain_db: f32,
    pub pan: f32,
    pub pitch: f32,
}
/// Number of built-in synth parameters (`SE_SYNTH_P_*`, `SynthParam::ALL` order).
pub const SE_SYNTH_P_COUNT: u16 = 20;
pub const SE_PARAM_STRING_MAX: usize = 128;

#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct se_auto_point {
    pub frame: i64,
    pub value: f32,
    pub curve: f32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct se_auto_lane_desc {
    pub target: u8,
    pub logarithmic: u8,
    pub synth_param: u16,
    pub param_id: u32,
    pub device_id: u64,
    pub points: *const se_auto_point,
    pub point_count: u32,
    pub flags: u32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct se_track_desc_v6 {
    pub track_id: u64,
    pub gain: f32,
    pub pan: f32,
    pub mute: u8,
    pub solo: u8,
    pub instrument: u8,
    pub arm: u8,
    pub monitor: u8,
    pub input_mode: u8,
    pub input_channel: u16,
    pub clips: *const se_clip_desc,
    pub clip_count: u32,
    pub note_count: u32,
    pub notes: *const se_note_desc,
    pub synth: *const se_synth_params,
    pub devices: *const se_device_desc,
    pub device_count: u32,
    pub _pad2: u32,
    pub instrument_plugin: *const se_device_desc,
    pub track_name: *const std::ffi::c_char,
    pub lanes: *const se_auto_lane_desc,
    pub lane_count: u32,
    pub _pad3: u32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct se_session_desc_v6 {
    pub tracks: *const se_track_desc_v6,
    pub track_count: u32,
    pub master_gain: f32,
    pub master_lanes: *const se_auto_lane_desc,
    pub master_lane_count: u32,
    pub _pad: u32,
}

// ---- audio clip warping (se_session_publish_v7)
pub const SE_WARP_REPITCH: u8 = 0;
pub const SE_WARP_SLICE: u8 = 1;
pub const SE_WARP_STRETCH: u8 = 2;
pub const SE_WARP_STRETCH_FORMANT: u8 = 3;
pub const SE_WARP_AUTO: u8 = 4;

#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct se_warp_point {
    pub frame: i64,
    pub source: f64,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct se_warp_slice {
    pub source: f64,
    pub percussive: u8,
    pub _pad: [u8; 7],
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct se_clip_warp_desc {
    pub clip_id: u64,
    pub mode: u8,
    pub _pad: [u8; 3],
    pub transpose: f32,
    pub points: *const se_warp_point,
    pub point_count: u32,
    pub slice_count: u32,
    pub slices: *const se_warp_slice,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct se_session_desc_v7 {
    pub v6: se_session_desc_v6,
    pub warps: *const se_clip_warp_desc,
    pub warp_count: u32,
    pub _pad: u32,
}

// ---- audio clip looping (se_session_publish_v8)
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct se_clip_loop_desc {
    pub clip_id: u64,
    pub loop_start: i64,
    pub loop_end: i64,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct se_session_desc_v8 {
    pub v7: se_session_desc_v7,
    pub loops: *const se_clip_loop_desc,
    pub loop_count: u32,
    pub _pad: u32,
}

// ---- return tracks, sends and output routing (se_session_publish_v9)
pub const SE_MAX_RETURNS: usize = 16;

#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct se_track_route {
    pub track_id: u64,
    pub output_track_id: u64,
    pub is_return: u8,
    pub _pad: [u8; 7],
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct se_send_desc {
    pub track_id: u64,
    pub return_track_id: u64,
    pub gain: f32,
    pub pre_fader: u8,
    pub _pad: [u8; 3],
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct se_session_desc_v9 {
    pub v8: se_session_desc_v8,
    pub routes: *const se_track_route,
    pub route_count: u32,
    pub send_count: u32,
    pub sends: *const se_send_desc,
}

// ---- channel strip: trim, width and mono (se_session_publish_v10)
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct se_track_strip {
    pub track_id: u64,
    pub trim: f32,
    pub width: f32,
    pub mono: u8,
    pub _pad: [u8; 7],
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct se_session_desc_v10 {
    pub v9: se_session_desc_v9,
    pub strips: *const se_track_strip,
    pub strip_count: u32,
    pub _pad: u32,
}

// ---- audio clip fades (se_session_publish_v11)

#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct se_clip_fade_desc {
    pub clip_id: u64,
    /// Timeline frames from the clip start.
    pub fade_in: i64,
    /// Timeline frames back from the clip end.
    pub fade_out: i64,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct se_session_desc_v11 {
    pub v10: se_session_desc_v10,
    pub fades: *const se_clip_fade_desc,
    pub fade_count: u32,
    pub _pad: u32,
}

// ---- tempo changes (se_session_publish_v12, spec 015)

pub const SE_MAX_TEMPO_SEGMENTS: u32 = 65536;

#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct se_tempo_segment {
    /// Exact (unrounded) timeline frame where the segment starts.
    pub start_frame: f64,
    /// Quarter notes from bar 1 at `start_frame`.
    pub start_quarter: f64,
    pub start_bpm: f64,
    /// Equal to `start_bpm`: hold; else the tempo at the next segment's start.
    pub end_bpm: f64,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct se_session_desc_v12 {
    pub v11: se_session_desc_v11,
    pub tempo: *const se_tempo_segment,
    pub tempo_count: u32,
    pub _pad: u32,
    /// Quarter notes per metronome beat.
    pub click_quarters: f64,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct se_onset {
    pub frame: i64,
    pub strength: f32,
    pub percussive: u8,
    pub _pad: [u8; 3],
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct se_plugin_param_info {
    pub id: u32,
    pub steps: u32,
    pub default_normalized: f32,
    pub automatable: u8,
    pub _pad: [u8; 3],
    pub name: [std::ffi::c_char; SE_PARAM_STRING_MAX],
    pub group: [std::ffi::c_char; SE_PARAM_STRING_MAX],
}

impl Default for se_plugin_param_info {
    fn default() -> Self {
        se_plugin_param_info {
            id: 0,
            steps: 0,
            default_normalized: 0.0,
            automatable: 0,
            _pad: [0; 3],
            name: [0; SE_PARAM_STRING_MAX],
            group: [0; SE_PARAM_STRING_MAX],
        }
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct se_plugin_param_change {
    pub device_id: u64,
    pub frame: i64,
    pub param_id: u32,
    pub normalized: f32,
    pub gesture_begin: u8,
    pub gesture_end: u8,
    pub _pad: [u8; 6],
}

/// Plugin delay compensation: largest delay applied (2^17 frames).
pub const SE_PDC_MAX_DELAY_FRAMES: u32 = 131_072;

#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct se_pdc_report {
    pub total_frames: u32,
    pub uncapped_frames: u32,
    pub track_count: u32,
    pub enabled: u8,
    pub clamped: u8,
    pub _pad: [u8; 2],
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct se_pdc_track {
    pub track_id: u64,
    pub latency_frames: u32,
    pub delay_frames: u32,
}

pub const SE_DEVICE_STRING_MAX: usize = 256;
pub const SE_DEVICE_MAX_RATES: usize = 16;
pub const SE_PATH_MAX: usize = 1024;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct se_device_info {
    pub uid: [c_char; SE_DEVICE_STRING_MAX],
    pub name: [c_char; SE_DEVICE_STRING_MAX],
    pub input_channels: u32,
    pub output_channels: u32,
    pub is_default_input: u8,
    pub is_default_output: u8,
    pub _pad: [u8; 2],
    pub sample_rate_count: u32,
    pub sample_rates: [f64; SE_DEVICE_MAX_RATES],
}

impl Default for se_device_info {
    fn default() -> Self {
        // SAFETY: plain-old-data; all-zero is a valid value.
        unsafe { std::mem::zeroed() }
    }
}

/// One MIDI input port (se_midi_inputs_list).
#[repr(C)]
#[derive(Clone, Copy)]
pub struct se_midi_input_info {
    pub id: [c_char; SE_DEVICE_STRING_MAX],
    pub name: [c_char; SE_DEVICE_STRING_MAX],
    pub events: u64,
    pub open: u8,
    pub _pad: [u8; 7],
}

impl Default for se_midi_input_info {
    fn default() -> Self {
        // SAFETY: plain-old-data; all-zero is a valid value.
        unsafe { std::mem::zeroed() }
    }
}

/// One captured note event (se_midi_capture_read).
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct se_midi_captured {
    pub frame: i64,
    pub status: u8,
    pub data1: u8,
    pub data2: u8,
    pub _pad: [u8; 5],
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct se_midi_stats {
    pub received: u64,
    pub delivered: u64,
    pub dropped: u64,
    pub capture_dropped: u64,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct se_device_config {
    pub output_uid: *const c_char,
    pub input_uid: *const c_char,
    pub sample_rate: f64,
    pub buffer_size: u32,
    pub input_enabled: u8,
    pub _pad: [u8; 3],
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct se_device_status {
    pub output_name: [c_char; SE_DEVICE_STRING_MAX],
    pub input_name: [c_char; SE_DEVICE_STRING_MAX],
    pub input_error: [c_char; SE_DEVICE_STRING_MAX],
    pub running: u8,
    pub input_open: u8,
    pub _pad: [u8; 6],
    pub sample_rate: f64,
    pub buffer_size: u32,
    pub input_channels: u32,
    pub output_channels: u32,
    pub input_latency_frames: u32,
    pub output_latency_frames: u32,
    pub _pad2: u32,
}

impl Default for se_device_status {
    fn default() -> Self {
        // SAFETY: plain-old-data; all-zero is a valid value.
        unsafe { std::mem::zeroed() }
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct se_recorded_take {
    pub track_id: u64,
    pub start_frame: i64,
    pub frames: u64,
    pub channels: u32,
    pub _pad: u32,
    pub sample_rate: f64,
    pub path: [c_char; SE_PATH_MAX],
}

impl Default for se_recorded_take {
    fn default() -> Self {
        // SAFETY: plain-old-data; all-zero is a valid value.
        unsafe { std::mem::zeroed() }
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct se_live_take {
    pub track_id: u64,
    pub start_frame: i64,
    pub frames: u64,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct se_record_stats {
    pub overruns: u64,
    pub dropped_frames: u64,
    pub write_errors: u64,
    pub takes: u64,
}

unsafe extern "C" {
    pub fn se_abi_version() -> u32;
    pub fn se_engine_create(config: *const se_config) -> *mut se_engine;
    pub fn se_engine_destroy(engine: *mut se_engine);
    pub fn se_last_error() -> *const c_char;

    pub fn se_device_start(engine: *mut se_engine) -> i32;
    pub fn se_device_stop(engine: *mut se_engine) -> i32;
    pub fn se_device_is_running(engine: *const se_engine) -> i32;
    pub fn se_engine_sample_rate(engine: *const se_engine) -> f64;

    pub fn se_asset_load_file(engine: *mut se_engine, path: *const c_char, out: *mut se_asset_info) -> i32;
    pub fn se_asset_release(engine: *mut se_engine, asset_id: u64) -> i32;
    pub fn se_asset_get_info(engine: *const se_engine, asset_id: u64, out: *mut se_asset_info) -> i32;
    pub fn se_asset_reverse(engine: *mut se_engine, asset_id: u64, out: *mut se_asset_info) -> i32;
    pub fn se_asset_compute_peaks(
        engine: *const se_engine,
        asset_id: u64,
        start_frame: u64,
        frames_per_peak: u32,
        out_minmax: *mut f32,
        capacity: usize,
        out_count: *mut usize,
    ) -> i32;
    pub fn se_asset_compute_channel_peaks(
        engine: *const se_engine,
        asset_id: u64,
        channel: u32,
        start_frame: u64,
        frames_per_peak: u32,
        out_minmax: *mut f32,
        capacity: usize,
        out_count: *mut usize,
    ) -> i32;

    pub fn se_session_publish(engine: *mut se_engine, session: *const se_session_desc) -> i32;

    pub fn se_transport_play(engine: *mut se_engine) -> i32;
    pub fn se_transport_pause(engine: *mut se_engine) -> i32;
    pub fn se_transport_stop(engine: *mut se_engine) -> i32;
    pub fn se_transport_seek(engine: *mut se_engine, frame: i64) -> i32;
    pub fn se_transport_get(engine: *const se_engine, out: *mut se_transport_info) -> i32;

    pub fn se_meters_read(engine: *mut se_engine, out: *mut se_meter, capacity: usize, out_count: *mut usize) -> i32;
    pub fn se_stats_get(engine: *const se_engine, out: *mut se_stats) -> i32;
    pub fn se_stats_reset(engine: *mut se_engine);

    pub fn se_render_offline(engine: *mut se_engine, out_interleaved: *mut f32, frames: u32) -> i32;
    pub fn se_export_wav(engine: *mut se_engine, path: *const c_char, start_frame: i64, end_frame: i64) -> i32;

    // ---- additions (ABI v1, append-only)
    pub fn se_meters_peek(engine: *mut se_engine, out: *mut se_meter, capacity: usize, out_count: *mut usize) -> i32;
    pub fn se_transport_set_loop(engine: *mut se_engine, enabled: u8, start_frame: i64, end_frame: i64) -> i32;
    pub fn se_metronome_set(engine: *mut se_engine, desc: *const se_metronome_desc) -> i32;
    pub fn se_transport_set_tempo(engine: *mut se_engine, frames_per_quarter: f64) -> i32;
    pub fn se_session_publish_v2(engine: *mut se_engine, session: *const se_session_desc_v2) -> i32;
    pub fn se_session_publish_v3(engine: *mut se_engine, session: *const se_session_desc_v3) -> i32;
    pub fn se_synth_params_default(out: *mut se_synth_params);
    pub fn se_session_publish_v4(engine: *mut se_engine, session: *const se_session_desc_v4) -> i32;
    pub fn se_device_list(out: *mut se_device_info, capacity: usize, out_count: *mut usize) -> i32;
    pub fn se_device_configure(engine: *mut se_engine, config: *const se_device_config) -> i32;
    pub fn se_device_status_get(engine: *mut se_engine, out: *mut se_device_status) -> i32;
    pub fn se_input_set_muted(engine: *mut se_engine, muted: u8) -> i32;
    pub fn se_record_start(engine: *mut se_engine, utf8_dir: *const c_char) -> i32;
    pub fn se_record_stop(engine: *mut se_engine, out_take_count: *mut usize) -> i32;
    pub fn se_record_get_take(engine: *mut se_engine, index: usize, out: *mut se_recorded_take) -> i32;
    pub fn se_record_set_offset(engine: *mut se_engine, frames: i32) -> i32;
    pub fn se_record_get_offset(engine: *mut se_engine, out_frames: *mut i32) -> i32;
    pub fn se_record_state(
        engine: *mut se_engine,
        out_active: *mut u8,
        out: *mut se_live_take,
        capacity: usize,
        out_count: *mut usize,
    ) -> i32;
    pub fn se_record_peaks(
        engine: *mut se_engine,
        track_id: u64,
        frames_per_peak: u32,
        out_minmax: *mut f32,
        capacity: usize,
        out_count: *mut usize,
    ) -> i32;
    pub fn se_record_stats_get(engine: *mut se_engine, out: *mut se_record_stats) -> i32;
    pub fn se_render_offline_with_input(
        engine: *mut se_engine,
        input_interleaved: *const f32,
        input_channels: u32,
        out_interleaved: *mut f32,
        frames: u32,
    ) -> i32;

    // plugins
    pub fn se_session_publish_v5(engine: *mut se_engine, session: *const se_session_desc_v5) -> i32;
    pub fn se_plugin_is_live(engine: *mut se_engine, device_id: u64) -> i32;
    pub fn se_plugin_load_errors(engine: *mut se_engine, out: *mut se_plugin_message, capacity: usize, out_count: *mut usize) -> i32;
    pub fn se_plugin_save_states(engine: *mut se_engine, out_count: *mut usize) -> i32;
    pub fn se_plugin_saved_state(
        engine: *mut se_engine,
        index: usize,
        out_device_id: *mut u64,
        out_data: *mut *const u8,
        out_len: *mut usize,
    ) -> i32;
    pub fn se_plugin_editor_open(engine: *mut se_engine, device_id: u64) -> i32;
    pub fn se_plugin_editor_close(engine: *mut se_engine, device_id: u64) -> i32;
    pub fn se_plugin_editor_is_open(engine: *mut se_engine, device_id: u64) -> i32;
    pub fn se_plugin_latency(engine: *mut se_engine, device_id: u64, out_frames: *mut u32) -> i32;
    pub fn se_plugin_idle(engine: *mut se_engine) -> i32;
    pub fn se_plugin_save_state(engine: *mut se_engine, device_id: u64) -> i32;
    pub fn se_plugin_prepare(engine: *mut se_engine, device: *const se_device_desc, instrument: u8) -> i32;
    pub fn se_plugin_set_detached(engine: *mut se_engine, detached: u8) -> i32;

    // plugin delay compensation
    pub fn se_pdc_set_enabled(engine: *mut se_engine, enabled: u8) -> i32;
    pub fn se_pdc_is_enabled(engine: *const se_engine) -> i32;
    pub fn se_pdc_report_get(
        engine: *mut se_engine,
        out: *mut se_pdc_report,
        tracks: *mut se_pdc_track,
        capacity: usize,
        out_count: *mut usize,
    ) -> i32;
    pub fn se_transport_audible_position(engine: *const se_engine, out_frame: *mut i64) -> i32;

    // automation and plugin parameters
    pub fn se_session_publish_v6(engine: *mut se_engine, session: *const se_session_desc_v6) -> i32;

    // clip warping
    pub fn se_session_publish_v7(engine: *mut se_engine, session: *const se_session_desc_v7) -> i32;
    pub fn se_session_publish_v8(engine: *mut se_engine, session: *const se_session_desc_v8) -> i32;
    pub fn se_session_publish_v9(engine: *mut se_engine, session: *const se_session_desc_v9) -> i32;
    pub fn se_session_publish_v10(engine: *mut se_engine, session: *const se_session_desc_v10) -> i32;
    pub fn se_session_publish_v11(engine: *mut se_engine, session: *const se_session_desc_v11) -> i32;
    pub fn se_session_publish_v12(engine: *mut se_engine, session: *const se_session_desc_v12) -> i32;
    pub fn se_warp_renders_pending(engine: *const se_engine, out_count: *mut usize) -> i32;
    pub fn se_asset_detect_onsets(engine: *mut se_engine, asset_id: u64, out: *mut se_onset, capacity: usize, out_count: *mut usize)
    -> i32;
    pub fn se_automation_eval(lane: *const se_auto_lane_desc, frames: *const i64, count: usize, out_values: *mut f32) -> i32;
    pub fn se_plugin_params(
        engine: *mut se_engine,
        device_id: u64,
        out: *mut se_plugin_param_info,
        capacity: usize,
        out_count: *mut usize,
    ) -> i32;
    pub fn se_plugin_param_value(engine: *mut se_engine, device_id: u64, param_id: u32, out_normalized: *mut f32) -> i32;
    pub fn se_plugin_param_text(
        engine: *mut se_engine,
        device_id: u64,
        param_id: u32,
        normalized: f32,
        out: *mut c_char,
        capacity: usize,
    ) -> i32;
    pub fn se_plugin_take_param_changes(
        engine: *mut se_engine,
        out: *mut se_plugin_param_change,
        capacity: usize,
        out_count: *mut usize,
    ) -> i32;

    // built-in devices
    pub fn se_builtin_registry(out_count: *mut usize) -> *const se_builtin_desc;
    pub fn se_builtin_set_param(engine: *mut se_engine, device_id: u64, index: u32, value: f32) -> i32;
    pub fn se_builtin_readouts(engine: *mut se_engine, device_id: u64, out: *mut f32, capacity: usize, out_count: *mut usize) -> i32;
    pub fn se_builtin_set_metering(engine: *mut se_engine, device_id: u64, on: u8) -> i32;
    pub fn se_builtin_levels(engine: *mut se_engine, device_id: u64, out: *mut f32) -> i32;
    pub fn se_builtin_correlation(engine: *mut se_engine, device_id: u64, out: *mut f32) -> i32;
    pub fn se_device_set_output_metering(engine: *mut se_engine, device_id: u64, on: u8) -> i32;
    pub fn se_device_output_peaks(engine: *mut se_engine, device_ids: *const u64, count: usize, out: *mut f32) -> i32;
    pub fn se_builtin_set_spectrum(engine: *mut se_engine, device_id: u64, mode: u8) -> i32;
    pub fn se_builtin_spectrum(
        engine: *mut se_engine,
        device_id: u64,
        out: *mut f32,
        capacity: usize,
        out_count: *mut usize,
        out_seq: *mut u64,
    ) -> i32;
    pub fn se_builtin_analysis(
        engine: *mut se_engine,
        device_id: u64,
        out: *mut f32,
        capacity: usize,
        out_count: *mut usize,
        out_seq: *mut u64,
    ) -> i32;

    // modulation
    pub fn se_plugin_set_param(engine: *mut se_engine, device_id: u64, param_id: u32, normalized: f32) -> i32;
    pub fn se_mod_base(engine: *mut se_engine, device_id: u64, param_id: u32, out_normalized: *mut f32) -> i32;
    pub fn se_builtin_note(engine: *mut se_engine, device_id: u64, pitch: u8, velocity: u8) -> i32;
    pub fn se_preview_play(engine: *mut se_engine, asset_id: u64, start_frame: i64, gain: f32, rate: f32, solo: u8) -> i32;
    pub fn se_preview_stop(engine: *mut se_engine) -> i32;
    pub fn se_preview_set(engine: *mut se_engine, gain: f32, rate: f32, solo: u8) -> i32;
    pub fn se_preview_get(engine: *mut se_engine, out: *mut se_preview_info) -> i32;
    pub fn se_preview_note(engine: *mut se_engine, device_id: u64, pitch: u8, velocity: u8) -> i32;
    pub fn se_midi_inputs_list(engine: *mut se_engine, out: *mut se_midi_input_info, capacity: usize, out_count: *mut usize) -> i32;
    pub fn se_midi_inputs_open(engine: *mut se_engine, ids: *const *const c_char, count: usize) -> i32;
    pub fn se_midi_inject(engine: *mut se_engine, status: u8, data1: u8, data2: u8) -> i32;
    pub fn se_midi_set_focus(engine: *mut se_engine, track_id: u64) -> i32;
    pub fn se_midi_capture_enable(engine: *mut se_engine, enabled: u8) -> i32;
    pub fn se_midi_capture_read(engine: *mut se_engine, out: *mut se_midi_captured, capacity: usize, out_count: *mut usize) -> i32;
    pub fn se_midi_capture_wrapped(engine: *mut se_engine, out_wrapped: *mut u8) -> i32;
    pub fn se_midi_stats_get(engine: *mut se_engine, out: *mut se_midi_stats) -> i32;
    pub fn se_voice_capture_set(engine: *mut se_engine, channel: i32, stereo: u8) -> i32;
    pub fn se_voice_capture_read(engine: *mut se_engine, out: *mut f32, capacity: usize, out_count: *mut usize, out_peak: *mut f32) -> i32;
}

/// Guards the hand-written declarations against drift from dstudio_engine.h.
/// Expected values were produced by compiling the header with clang (LP64).
#[cfg(all(test, target_pointer_width = "64"))]
mod abi_layout {
    use super::*;
    use std::mem::{align_of, offset_of, size_of};

    #[test]
    fn struct_layouts_match_c_header() {
        assert_eq!((size_of::<se_config>(), align_of::<se_config>()), (24, 8));
        assert_eq!((size_of::<se_asset_info>(), align_of::<se_asset_info>()), (32, 8));
        assert_eq!((size_of::<se_clip_desc>(), align_of::<se_clip_desc>()), (48, 8));
        assert_eq!((size_of::<se_track_desc>(), align_of::<se_track_desc>()), (40, 8));
        assert_eq!((offset_of!(se_track_desc, clips), offset_of!(se_track_desc, clip_count)), (24, 32));
        assert_eq!(size_of::<se_session_desc>(), 16);
        assert_eq!(size_of::<se_transport_info>(), 16);
        assert_eq!(size_of::<se_meter>(), 16);
        assert_eq!((size_of::<se_preview_info>(), align_of::<se_preview_info>()), (40, 8));
        assert_eq!(size_of::<se_stats>(), 48);
        // Verified with clang against dstudio_engine.h: size 24, align 8,
        // offsets enabled 0, gain 4, frames_per_beat 8, beats_per_bar 16, _pad2 20.
        assert_eq!((size_of::<se_metronome_desc>(), align_of::<se_metronome_desc>()), (24, 8));
        assert_eq!(
            (
                offset_of!(se_metronome_desc, enabled),
                offset_of!(se_metronome_desc, gain),
                offset_of!(se_metronome_desc, frames_per_beat),
                offset_of!(se_metronome_desc, beats_per_bar),
                offset_of!(se_metronome_desc, _pad2)
            ),
            (0, 4, 8, 16, 20)
        );
        // MIDI additions, verified with clang (-Wpadded: no implicit padding):
        // se_note_desc 24/8: start 0, end 8, pitch 16, velocity 17, _pad 18.
        assert_eq!((size_of::<se_note_desc>(), align_of::<se_note_desc>()), (24, 8));
        assert_eq!(
            (
                offset_of!(se_note_desc, start),
                offset_of!(se_note_desc, end),
                offset_of!(se_note_desc, pitch),
                offset_of!(se_note_desc, velocity),
                offset_of!(se_note_desc, _pad)
            ),
            (0, 8, 16, 17, 18)
        );
        // se_track_desc_v2 48/8: track_id 0, gain 8, pan 12, mute 16, solo 17,
        // instrument 18, _pad 19, clips 24, clip_count 32, note_count 36, notes 40.
        assert_eq!((size_of::<se_track_desc_v2>(), align_of::<se_track_desc_v2>()), (48, 8));
        assert_eq!(
            (
                offset_of!(se_track_desc_v2, track_id),
                offset_of!(se_track_desc_v2, gain),
                offset_of!(se_track_desc_v2, pan),
                offset_of!(se_track_desc_v2, mute),
                offset_of!(se_track_desc_v2, solo),
                offset_of!(se_track_desc_v2, instrument),
                offset_of!(se_track_desc_v2, _pad),
                offset_of!(se_track_desc_v2, clips),
                offset_of!(se_track_desc_v2, clip_count),
                offset_of!(se_track_desc_v2, note_count),
                offset_of!(se_track_desc_v2, notes)
            ),
            (0, 8, 12, 16, 17, 18, 19, 24, 32, 36, 40)
        );
        // se_session_desc_v2 16/8: tracks 0, track_count 8, master_gain 12.
        assert_eq!((size_of::<se_session_desc_v2>(), align_of::<se_session_desc_v2>()), (16, 8));
        assert_eq!(
            (
                offset_of!(se_session_desc_v2, tracks),
                offset_of!(se_session_desc_v2, track_count),
                offset_of!(se_session_desc_v2, master_gain)
            ),
            (0, 8, 12)
        );
    }

    #[test]
    fn synth_params_layout_matches_c_header() {
        // Verified with clang -Wpadded against dstudio_engine.h (no implicit
        // padding): se_synth_params 128/4, se_track_desc_v3 56/8,
        // se_session_desc_v3 16/8.
        assert_eq!((size_of::<se_synth_params>(), align_of::<se_synth_params>()), (128, 4));
        assert_eq!(
            (
                offset_of!(se_synth_params, osc1_wave),
                offset_of!(se_synth_params, osc1_octave),
                offset_of!(se_synth_params, osc1_semitones),
                offset_of!(se_synth_params, osc2_wave),
                offset_of!(se_synth_params, osc2_octave),
                offset_of!(se_synth_params, osc2_semitones),
                offset_of!(se_synth_params, filter_mode),
                offset_of!(se_synth_params, lfo_wave),
                offset_of!(se_synth_params, polyphony),
                offset_of!(se_synth_params, unison),
                offset_of!(se_synth_params, _pad),
            ),
            (0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10)
        );
        assert_eq!(
            [
                offset_of!(se_synth_params, osc1_shape),
                offset_of!(se_synth_params, osc1_fine_cents),
                offset_of!(se_synth_params, osc1_level),
                offset_of!(se_synth_params, osc2_shape),
                offset_of!(se_synth_params, osc2_fine_cents),
                offset_of!(se_synth_params, osc2_level),
                offset_of!(se_synth_params, cross_fm),
                offset_of!(se_synth_params, sub_level),
                offset_of!(se_synth_params, noise_level),
                offset_of!(se_synth_params, filter_cutoff_hz),
                offset_of!(se_synth_params, filter_resonance),
                offset_of!(se_synth_params, filter_key_track),
                offset_of!(se_synth_params, filter_env_amount),
                offset_of!(se_synth_params, amp_attack),
            ],
            [16, 20, 24, 28, 32, 36, 40, 44, 48, 52, 56, 60, 64, 68]
        );
        assert_eq!(
            (
                offset_of!(se_synth_params, amp_release),
                offset_of!(se_synth_params, filter_attack),
                offset_of!(se_synth_params, filter_release),
                offset_of!(se_synth_params, lfo_rate_hz),
                offset_of!(se_synth_params, lfo_to_pitch_cents),
                offset_of!(se_synth_params, lfo_to_cutoff_oct),
                offset_of!(se_synth_params, lfo_to_amp),
                offset_of!(se_synth_params, glide),
                offset_of!(se_synth_params, unison_detune_cents),
                offset_of!(se_synth_params, output),
            ),
            (80, 84, 96, 100, 104, 108, 112, 116, 120, 124)
        );
        assert_eq!((size_of::<se_track_desc_v3>(), align_of::<se_track_desc_v3>()), (56, 8));
        assert_eq!(
            (
                offset_of!(se_track_desc_v3, track_id),
                offset_of!(se_track_desc_v3, gain),
                offset_of!(se_track_desc_v3, pan),
                offset_of!(se_track_desc_v3, mute),
                offset_of!(se_track_desc_v3, solo),
                offset_of!(se_track_desc_v3, instrument),
                offset_of!(se_track_desc_v3, _pad),
                offset_of!(se_track_desc_v3, clips),
                offset_of!(se_track_desc_v3, clip_count),
                offset_of!(se_track_desc_v3, note_count),
                offset_of!(se_track_desc_v3, notes),
                offset_of!(se_track_desc_v3, synth)
            ),
            (0, 8, 12, 16, 17, 18, 19, 24, 32, 36, 40, 48)
        );
        assert_eq!((size_of::<se_session_desc_v3>(), align_of::<se_session_desc_v3>()), (16, 8));
        assert_eq!(
            (
                offset_of!(se_session_desc_v3, tracks),
                offset_of!(se_session_desc_v3, track_count),
                offset_of!(se_session_desc_v3, master_gain)
            ),
            (0, 8, 12)
        );
    }

    #[test]
    fn recording_layout_matches_c_header() {
        // Verified with clang -Wpadded against dstudio_engine.h (no implicit
        // padding) and by static_asserts in engine/src/c_api.cpp.
        assert_eq!((size_of::<se_track_desc_v4>(), align_of::<se_track_desc_v4>()), (56, 8));
        assert_eq!(
            (
                offset_of!(se_track_desc_v4, track_id),
                offset_of!(se_track_desc_v4, gain),
                offset_of!(se_track_desc_v4, pan),
                offset_of!(se_track_desc_v4, mute),
                offset_of!(se_track_desc_v4, solo),
                offset_of!(se_track_desc_v4, instrument),
                offset_of!(se_track_desc_v4, arm),
                offset_of!(se_track_desc_v4, monitor),
                offset_of!(se_track_desc_v4, input_mode),
                offset_of!(se_track_desc_v4, input_channel),
                offset_of!(se_track_desc_v4, clips),
                offset_of!(se_track_desc_v4, clip_count),
            ),
            (0, 8, 12, 16, 17, 18, 19, 20, 21, 22, 24, 32)
        );
        assert_eq!(
            (offset_of!(se_track_desc_v4, note_count), offset_of!(se_track_desc_v4, notes), offset_of!(se_track_desc_v4, synth)),
            (36, 40, 48)
        );
        assert_eq!((size_of::<se_session_desc_v4>(), align_of::<se_session_desc_v4>()), (16, 8));
        assert_eq!((size_of::<se_device_info>(), align_of::<se_device_info>()), (656, 8));
        assert_eq!(
            (
                offset_of!(se_device_info, name),
                offset_of!(se_device_info, input_channels),
                offset_of!(se_device_info, output_channels),
                offset_of!(se_device_info, is_default_input),
                offset_of!(se_device_info, is_default_output),
                offset_of!(se_device_info, sample_rate_count),
                offset_of!(se_device_info, sample_rates)
            ),
            (256, 512, 516, 520, 521, 524, 528)
        );
        assert_eq!((size_of::<se_midi_input_info>(), align_of::<se_midi_input_info>()), (528, 8));
        assert_eq!(
            (offset_of!(se_midi_input_info, name), offset_of!(se_midi_input_info, events), offset_of!(se_midi_input_info, open)),
            (256, 512, 520)
        );
        assert_eq!((size_of::<se_midi_captured>(), align_of::<se_midi_captured>()), (16, 8));
        assert_eq!(
            (offset_of!(se_midi_captured, status), offset_of!(se_midi_captured, data1), offset_of!(se_midi_captured, data2)),
            (8, 9, 10)
        );
        assert_eq!((size_of::<se_midi_stats>(), align_of::<se_midi_stats>()), (32, 8));
        assert_eq!((size_of::<se_device_config>(), align_of::<se_device_config>()), (32, 8));
        assert_eq!(
            (
                offset_of!(se_device_config, input_uid),
                offset_of!(se_device_config, sample_rate),
                offset_of!(se_device_config, buffer_size),
                offset_of!(se_device_config, input_enabled)
            ),
            (8, 16, 24, 28)
        );
        assert_eq!((size_of::<se_device_status>(), align_of::<se_device_status>()), (808, 8));
        assert_eq!(
            (
                offset_of!(se_device_status, input_name),
                offset_of!(se_device_status, input_error),
                offset_of!(se_device_status, running),
                offset_of!(se_device_status, input_open),
                offset_of!(se_device_status, sample_rate),
                offset_of!(se_device_status, buffer_size),
                offset_of!(se_device_status, input_channels),
                offset_of!(se_device_status, output_channels),
                offset_of!(se_device_status, input_latency_frames),
                offset_of!(se_device_status, output_latency_frames),
                offset_of!(se_device_status, _pad2)
            ),
            (256, 512, 768, 769, 776, 784, 788, 792, 796, 800, 804)
        );
        assert_eq!((size_of::<se_recorded_take>(), align_of::<se_recorded_take>()), (1064, 8));
        assert_eq!(
            (
                offset_of!(se_recorded_take, start_frame),
                offset_of!(se_recorded_take, frames),
                offset_of!(se_recorded_take, channels),
                offset_of!(se_recorded_take, sample_rate),
                offset_of!(se_recorded_take, path)
            ),
            (8, 16, 24, 32, 40)
        );
        assert_eq!((size_of::<se_live_take>(), align_of::<se_live_take>()), (24, 8));
        assert_eq!((size_of::<se_record_stats>(), align_of::<se_record_stats>()), (32, 8));
    }

    #[test]
    fn plugin_layout_matches_c_header() {
        assert_eq!((size_of::<se_device_desc>(), align_of::<se_device_desc>()), (56, 8));
        assert_eq!(
            [
                offset_of!(se_device_desc, device_id),
                offset_of!(se_device_desc, name),
                offset_of!(se_device_desc, plugin_id),
                offset_of!(se_device_desc, path),
                offset_of!(se_device_desc, state),
                offset_of!(se_device_desc, state_len),
                offset_of!(se_device_desc, format),
                offset_of!(se_device_desc, enabled),
                offset_of!(se_device_desc, _pad),
            ],
            [0, 8, 16, 24, 32, 40, 48, 49, 50]
        );
        assert_eq!((size_of::<se_track_desc_v5>(), align_of::<se_track_desc_v5>()), (88, 8));
        assert_eq!(
            [
                offset_of!(se_track_desc_v5, track_id),
                offset_of!(se_track_desc_v5, gain),
                offset_of!(se_track_desc_v5, pan),
                offset_of!(se_track_desc_v5, mute),
                offset_of!(se_track_desc_v5, solo),
                offset_of!(se_track_desc_v5, instrument),
                offset_of!(se_track_desc_v5, arm),
                offset_of!(se_track_desc_v5, monitor),
                offset_of!(se_track_desc_v5, input_mode),
                offset_of!(se_track_desc_v5, input_channel),
                offset_of!(se_track_desc_v5, clips),
                offset_of!(se_track_desc_v5, clip_count),
                offset_of!(se_track_desc_v5, note_count),
                offset_of!(se_track_desc_v5, notes),
                offset_of!(se_track_desc_v5, synth),
                offset_of!(se_track_desc_v5, devices),
                offset_of!(se_track_desc_v5, device_count),
                offset_of!(se_track_desc_v5, _pad2),
                offset_of!(se_track_desc_v5, instrument_plugin),
                offset_of!(se_track_desc_v5, track_name),
            ],
            [0, 8, 12, 16, 17, 18, 19, 20, 21, 22, 24, 32, 36, 40, 48, 56, 64, 68, 72, 80]
        );
        assert_eq!((size_of::<se_session_desc_v5>(), align_of::<se_session_desc_v5>()), (16, 8));
        assert_eq!((size_of::<se_plugin_message>(), align_of::<se_plugin_message>()), (264, 8));
        assert_eq!(offset_of!(se_plugin_message, message), 8);
    }

    #[test]
    fn pdc_layout_matches_c_header() {
        // Verified with clang -Wpadded against dstudio_engine.h (no implicit
        // padding); also static_asserted in c_api.cpp.
        assert_eq!((size_of::<se_pdc_report>(), align_of::<se_pdc_report>()), (16, 4));
        assert_eq!(
            [
                offset_of!(se_pdc_report, total_frames),
                offset_of!(se_pdc_report, uncapped_frames),
                offset_of!(se_pdc_report, track_count),
                offset_of!(se_pdc_report, enabled),
                offset_of!(se_pdc_report, clamped),
                offset_of!(se_pdc_report, _pad),
            ],
            [0, 4, 8, 12, 13, 14]
        );
        assert_eq!((size_of::<se_pdc_track>(), align_of::<se_pdc_track>()), (16, 8));
        assert_eq!(
            [offset_of!(se_pdc_track, track_id), offset_of!(se_pdc_track, latency_frames), offset_of!(se_pdc_track, delay_frames)],
            [0, 8, 12]
        );
    }

    #[test]
    fn automation_layout_matches_c_header() {
        // Also static_asserted in c_api.cpp.
        assert_eq!((size_of::<se_auto_point>(), align_of::<se_auto_point>()), (16, 8));
        assert_eq!([offset_of!(se_auto_point, frame), offset_of!(se_auto_point, value), offset_of!(se_auto_point, curve)], [0, 8, 12]);
        assert_eq!((size_of::<se_auto_lane_desc>(), align_of::<se_auto_lane_desc>()), (32, 8));
        assert_eq!(
            [
                offset_of!(se_auto_lane_desc, target),
                offset_of!(se_auto_lane_desc, logarithmic),
                offset_of!(se_auto_lane_desc, synth_param),
                offset_of!(se_auto_lane_desc, param_id),
                offset_of!(se_auto_lane_desc, device_id),
                offset_of!(se_auto_lane_desc, points),
                offset_of!(se_auto_lane_desc, point_count),
                offset_of!(se_auto_lane_desc, flags),
            ],
            [0, 1, 2, 4, 8, 16, 24, 28]
        );
        assert_eq!((size_of::<se_track_desc_v6>(), align_of::<se_track_desc_v6>()), (104, 8));
        assert_eq!(
            [
                offset_of!(se_track_desc_v6, synth),
                offset_of!(se_track_desc_v6, track_name),
                offset_of!(se_track_desc_v6, lanes),
                offset_of!(se_track_desc_v6, lane_count),
                offset_of!(se_track_desc_v6, _pad3)
            ],
            [48, 80, 88, 96, 100]
        );
        assert_eq!((size_of::<se_session_desc_v6>(), align_of::<se_session_desc_v6>()), (32, 8));
        assert_eq!(
            [
                offset_of!(se_session_desc_v6, master_lanes),
                offset_of!(se_session_desc_v6, master_lane_count),
                offset_of!(se_session_desc_v6, _pad)
            ],
            [16, 24, 28]
        );
        assert_eq!((size_of::<se_warp_point>(), size_of::<se_warp_slice>(), size_of::<se_onset>()), (16, 16, 16));
        assert_eq!((size_of::<se_clip_warp_desc>(), align_of::<se_clip_warp_desc>()), (40, 8));
        assert_eq!(
            [
                offset_of!(se_clip_warp_desc, mode),
                offset_of!(se_clip_warp_desc, transpose),
                offset_of!(se_clip_warp_desc, points),
                offset_of!(se_clip_warp_desc, point_count),
                offset_of!(se_clip_warp_desc, slice_count),
                offset_of!(se_clip_warp_desc, slices),
            ],
            [8, 12, 16, 24, 28, 32]
        );
        assert_eq!((size_of::<se_session_desc_v7>(), align_of::<se_session_desc_v7>()), (48, 8));
        assert_eq!([offset_of!(se_session_desc_v7, warps), offset_of!(se_session_desc_v7, warp_count)], [32, 40]);
        assert_eq!((size_of::<se_clip_loop_desc>(), align_of::<se_clip_loop_desc>()), (24, 8));
        assert_eq!((size_of::<se_session_desc_v8>(), align_of::<se_session_desc_v8>()), (64, 8));
        assert_eq!([offset_of!(se_session_desc_v8, loops), offset_of!(se_session_desc_v8, loop_count)], [48, 56]);
        assert_eq!((size_of::<se_track_route>(), align_of::<se_track_route>(), offset_of!(se_track_route, is_return)), (24, 8, 16));
        assert_eq!((size_of::<se_send_desc>(), offset_of!(se_send_desc, gain), offset_of!(se_send_desc, pre_fader)), (24, 16, 20));
        assert_eq!((size_of::<se_session_desc_v9>(), align_of::<se_session_desc_v9>()), (88, 8));
        assert_eq!(
            [
                offset_of!(se_session_desc_v9, routes),
                offset_of!(se_session_desc_v9, route_count),
                offset_of!(se_session_desc_v9, send_count),
                offset_of!(se_session_desc_v9, sends)
            ],
            [64, 72, 76, 80]
        );
        assert_eq!((size_of::<se_track_strip>(), align_of::<se_track_strip>()), (24, 8));
        assert_eq!([offset_of!(se_track_strip, trim), offset_of!(se_track_strip, width), offset_of!(se_track_strip, mono)], [8, 12, 16]);
        assert_eq!((size_of::<se_session_desc_v10>(), align_of::<se_session_desc_v10>()), (104, 8));
        assert_eq!([offset_of!(se_session_desc_v10, strips), offset_of!(se_session_desc_v10, strip_count)], [88, 96]);
        assert_eq!((size_of::<se_clip_fade_desc>(), align_of::<se_clip_fade_desc>()), (24, 8));
        assert_eq!((size_of::<se_session_desc_v11>(), align_of::<se_session_desc_v11>()), (120, 8));
        assert_eq!([offset_of!(se_session_desc_v11, fades), offset_of!(se_session_desc_v11, fade_count)], [104, 112]);
        assert_eq!((size_of::<se_tempo_segment>(), align_of::<se_tempo_segment>()), (32, 8));
        assert_eq!([offset_of!(se_tempo_segment, start_quarter), offset_of!(se_tempo_segment, end_bpm)], [8, 24]);
        assert_eq!((size_of::<se_session_desc_v12>(), align_of::<se_session_desc_v12>()), (144, 8));
        assert_eq!(
            [
                offset_of!(se_session_desc_v12, tempo),
                offset_of!(se_session_desc_v12, tempo_count),
                offset_of!(se_session_desc_v12, click_quarters)
            ],
            [120, 128, 136]
        );
        assert_eq!([offset_of!(se_onset, strength), offset_of!(se_onset, percussive)], [8, 12]);
        assert_eq!((size_of::<se_plugin_param_info>(), align_of::<se_plugin_param_info>()), (272, 4));
        assert_eq!(
            [
                offset_of!(se_plugin_param_info, steps),
                offset_of!(se_plugin_param_info, default_normalized),
                offset_of!(se_plugin_param_info, automatable),
                offset_of!(se_plugin_param_info, name),
                offset_of!(se_plugin_param_info, group)
            ],
            [4, 8, 12, 16, 144]
        );
        assert_eq!((size_of::<se_plugin_param_change>(), align_of::<se_plugin_param_change>()), (32, 8));
        assert_eq!(
            [
                offset_of!(se_plugin_param_change, frame),
                offset_of!(se_plugin_param_change, param_id),
                offset_of!(se_plugin_param_change, normalized),
                offset_of!(se_plugin_param_change, gesture_begin),
                offset_of!(se_plugin_param_change, gesture_end),
                offset_of!(se_plugin_param_change, _pad),
            ],
            [8, 16, 20, 24, 25, 26]
        );
        assert_eq!((size_of::<se_builtin_state>(), align_of::<se_builtin_state>()), (16, 8));
        assert_eq!([offset_of!(se_builtin_state, value_count), offset_of!(se_builtin_state, sidechain_track_id)], [4, 8]);
        assert_eq!((size_of::<se_builtin_desc>(), align_of::<se_builtin_desc>()), (48, 8));
        assert_eq!([offset_of!(se_builtin_desc, sample_slots), offset_of!(se_builtin_desc, instrument)], [40, 44]);
        assert_eq!((size_of::<se_builtin_state_v2>(), align_of::<se_builtin_state_v2>()), (32, 8));
        assert_eq!(
            [
                offset_of!(se_builtin_state_v2, sample_count),
                offset_of!(se_builtin_state_v2, slice_count),
                offset_of!(se_builtin_state_v2, route_count)
            ],
            [16, 20, 24]
        );
        assert_eq!((size_of::<se_builtin_sample>(), size_of::<se_builtin_slice>()), (16, 16));
        assert_eq!((size_of::<se_mod_route>(), align_of::<se_mod_route>()), (40, 8));
        assert_eq!(
            [
                offset_of!(se_mod_route, param_id),
                offset_of!(se_mod_route, kind),
                offset_of!(se_mod_route, log),
                offset_of!(se_mod_route, min),
                offset_of!(se_mod_route, p_step)
            ],
            [8, 12, 15, 16, 36]
        );
        assert_eq!(
            [
                offset_of!(se_builtin_desc, param_ids),
                offset_of!(se_builtin_desc, readout_ids),
                offset_of!(se_builtin_desc, param_count),
                offset_of!(se_builtin_desc, readout_count),
                offset_of!(se_builtin_desc, param_defaults),
            ],
            [8, 16, 24, 28, 32]
        );
    }

    #[test]
    fn abi_version_matches() {
        assert_eq!(unsafe { se_abi_version() }, SE_ABI_VERSION);
    }
}
