//! End-to-end recording: dstudio_core Document (armed audio track with an
//! input) → snapshot → engine with injected input → take on disk → the
//! "Record" Batch (AddAsset + AddClip) → playback of the new clip renders
//! the recorded samples. No audio device is opened.

use dstudio_core::*;
use dstudio_engine::{Config, Engine};
use std::collections::HashMap;
use std::path::Path;

const RATE: f64 = 48_000.0;

/// Injected device input: channel `c` at absolute frame `k`.
fn input_sample(c: u32, k: usize) -> f32 {
    if c == 1 { 0.5 * (k as f32 * 0.013).sin() + 1e-6 * (k % 7) as f32 } else { -0.25 }
}

/// Minimal reader for the engine's 32-bit float WAV: returns (channels, samples).
fn read_float_wav(path: &Path) -> (u16, Vec<f32>) {
    let b = std::fs::read(path).unwrap();
    assert_eq!(&b[0..4], b"RIFF");
    assert_eq!(&b[8..12], b"WAVE");
    let riff_len = u32::from_le_bytes(b[4..8].try_into().unwrap()) as usize;
    assert_eq!(riff_len + 8, b.len(), "RIFF size must match the finalized file");
    let mut pos = 12;
    let mut channels = 0;
    while pos + 8 <= b.len() {
        let id = &b[pos..pos + 4];
        let len = u32::from_le_bytes(b[pos + 4..pos + 8].try_into().unwrap()) as usize;
        let body = &b[pos + 8..pos + 8 + len];
        if id == b"fmt " {
            assert_eq!(u16::from_le_bytes([body[0], body[1]]), 3, "IEEE float");
            channels = u16::from_le_bytes([body[2], body[3]]);
            assert_eq!(u32::from_le_bytes(body[4..8].try_into().unwrap()), RATE as u32);
            assert_eq!(u16::from_le_bytes([body[14], body[15]]), 32);
        } else if id == b"data" {
            return (channels, body.chunks_exact(4).map(|c| f32::from_le_bytes(c.try_into().unwrap())).collect());
        }
        pos += 8 + len + (len & 1);
    }
    panic!("no data chunk");
}

#[test]
fn record_armed_track_then_play_back_the_take() {
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::new(Config { sample_rate: RATE, max_block_size: 1024 }).unwrap();
    let mut doc = Document::new(Project::default());

    // An armed audio track listening to device input 2 (channel index 1), mono.
    doc.execute(Command::AddTrack { index: None, track: Track::new(TrackId(0), TrackKind::Audio, "Vox", ColorIndex(0)) }, Origin::User)
        .unwrap();
    let track = doc.project().tracks.last().unwrap().id;
    doc.execute(Command::SetTrackInput { track, input: Some(InputSource { channel: 1, stereo: false }) }, Origin::User).unwrap();
    doc.execute(Command::SetTrackArm { track, arm: true }, Origin::User).unwrap();

    let map: HashMap<AssetId, u64> = HashMap::new();
    let (snap, _) = snapshot::build(doc.project(), RATE, &map);
    let ts = snap.tracks.iter().find(|t| t.track_id == track.0).unwrap();
    assert!(ts.arm && !ts.monitor);
    assert_eq!(ts.input, Some(InputSource { channel: 1, stereo: false }));
    engine.publish(&snap).unwrap();

    // Record from bar 2 at 120 BPM (frame 96 000, a whole number of ticks).
    let start_ticks = doc.project().ticks_per_bar();
    let start_frame = doc.project().ticks_to_frames(start_ticks, RATE);
    assert_eq!(start_frame, 96_000);
    engine.seek(start_frame).unwrap();
    engine.start_recording(dir.path()).unwrap();
    assert!(engine.transport().playing);

    let total = 30_001usize;
    let channels = 2u32;
    let input: Vec<f32> = (0..total).flat_map(|k| (0..channels).map(move |c| input_sample(c, k))).collect();
    let mut out = vec![0f32; total * 2];
    let mut done = 0;
    for chunk in [127usize, 1024, 3000, 1, 512].iter().cycle() {
        if done >= total {
            break;
        }
        let n = (*chunk).min(total - done);
        engine.render_offline_with_input(&input[done * 2..(done + n) * 2], channels, &mut out[done * 2..(done + n) * 2]).unwrap();
        done += n;
    }
    // Armed but not monitored: nothing is heard ...
    assert!(out.iter().all(|&v| v == 0.0));
    // ... and the live view shows the growing take.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let st = engine.recording_state();
        assert!(st.active);
        if st.takes.len() == 1 && st.takes[0].frames == total as u64 {
            assert_eq!((st.takes[0].track_id, st.takes[0].start_frame), (track.0, start_frame));
            break;
        }
        assert!(std::time::Instant::now() < deadline, "writer did not catch up: {st:?}");
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    let peaks = engine.recording_peaks(track.0, 1024, 100);
    assert_eq!(peaks.len(), 30); // ceil(30001 / 1024) at 64-frame resolution (1024 = 16 buckets)
    assert!(peaks.iter().all(|p| p[0] <= p[1] && p[1] <= 0.6 && p[0] >= -0.6));

    let takes = engine.stop_recording().unwrap();
    assert!(!engine.recording_state().active);
    assert_eq!(engine.recording_stats().overruns, 0);
    assert!(engine.transport().playing, "stop_recording leaves the transport alone");
    engine.stop().unwrap();
    assert_eq!(takes.len(), 1);
    let take = &takes[0];
    assert_eq!(
        (take.track_id, take.start_frame, take.frames, take.channels, take.sample_rate),
        (track.0, start_frame, total as u64, 1, RATE)
    );
    assert!(take.path.starts_with(dir.path()));

    // The file holds exactly the injected channel, bit for bit.
    let expected: Vec<f32> = (0..total).map(|k| input_sample(1, k)).collect();
    let (ch, samples) = read_float_wav(&take.path);
    assert_eq!(ch, 1);
    assert_eq!(samples, expected);

    // It decodes through the engine's importer to the same samples.
    let info = engine.load_file(&take.path).unwrap();
    assert_eq!((info.frames, info.channels, info.source_sample_rate), (total as u64, 1, RATE));

    // The app's "Record" step: one undoable Batch of AddAsset + AddClip.
    let asset_id = AssetId(9_001);
    doc.execute(
        Command::Batch {
            label: "Record".into(),
            commands: vec![
                Command::AddAsset {
                    asset: Asset {
                        id: asset_id,
                        relative_path: None,
                        absolute_path: take.path.to_string_lossy().into(),
                        sample_rate: take.sample_rate,
                        channels: take.channels,
                        frames: take.frames,
                    },
                },
                Command::AddClip {
                    track,
                    clip: Clip {
                        id: ClipId(9_002),
                        asset: asset_id,
                        name: "Vox Take 1".into(),
                        start: doc.project().frames_to_ticks(take.start_frame, RATE),
                        source_offset: 0,
                        source_length: take.frames,
                        gain: 1.0,
                        warp: Default::default(),
                        disabled: false,
                        clip_loop: None,
                        fades: Default::default(),
                        takes: Vec::new(),
                    },
                },
            ],
        },
        Origin::User,
    )
    .unwrap();
    assert_eq!(doc.undo_label(), Some("Record"));

    // Play the new clip back (disarmed, centre pan): L = R = sample * cos(pi/4).
    doc.execute(Command::SetTrackArm { track, arm: false }, Origin::User).unwrap();
    let map = HashMap::from([(asset_id, info.asset_id)]);
    let (snap, skipped) = snapshot::build(doc.project(), RATE, &map);
    assert!(skipped.is_empty());
    let clip = &snap.tracks.iter().find(|t| t.track_id == track.0).unwrap().clips[0];
    assert_eq!((clip.timeline_start, clip.length), (start_frame, total as i64));
    engine.publish(&snap).unwrap();
    engine.seek(start_frame).unwrap();
    engine.play().unwrap();
    let mut played = vec![0f32; total * 2];
    for c in played.chunks_mut(2 * 777) {
        engine.render_offline(c).unwrap();
    }
    let g = std::f32::consts::FRAC_PI_4.cos();
    let g_engine = ((0.0f64 + 1.0) * std::f64::consts::PI / 4.0).cos() as f32;
    assert!((g - g_engine).abs() < 1e-7);
    let mismatches = (0..total)
        .filter(|&k| {
            played[2 * k] != expected[k] * g_engine || played[2 * k + 1] != expected[k] * (std::f64::consts::PI / 4.0).sin() as f32
        })
        .count();
    assert_eq!(mismatches, 0);

    // Undo removes the clip and the asset again.
    doc.undo().unwrap(); // the disarm
    doc.undo().unwrap(); // the Record batch
    assert!(doc.project().track(track).unwrap().clips.is_empty());
    assert!(doc.project().asset(asset_id).is_none());
}

#[test]
fn default_engine_reports_devices_without_opening_input() {
    let engine = Engine::new(Config::default()).unwrap();
    // Enumeration only (never opens a device).
    let devices = engine.list_devices().unwrap();
    for d in &devices {
        assert!(!d.id.is_empty());
        assert!(d.sample_rates.windows(2).all(|w| w[0] < w[1]));
    }
    assert!(devices.iter().filter(|d| d.is_default_output).count() <= 1);
    let st = engine.device_status();
    assert!(!st.running);
    assert_eq!(st.input_name, None);
    assert_eq!(engine.input_error(), None);
    // Unknown ids are rejected by validation, before anything is opened.
    let bad = dstudio_engine::DeviceConfig { output: Some("no-such-device".into()), ..Default::default() };
    assert_eq!(engine.configure_device(&bad).unwrap_err().code, dstudio_engine::ffi::SE_ERR_NOT_FOUND);
    let bad_in = dstudio_engine::DeviceConfig { input: Some("no-such-input".into()), ..Default::default() };
    assert_eq!(engine.configure_device(&bad_in).unwrap_err().code, dstudio_engine::ffi::SE_ERR_NOT_FOUND);
    // Recording needs an armed track with an input.
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(engine.start_recording(dir.path()).unwrap_err().code, dstudio_engine::ffi::SE_ERR_STATE);
    assert!(engine.stop_recording().is_err());
}

/// Spec 020: the manual recording offset reaches the engine through the Rust wrapper and moves
/// the take by exactly that many frames (negative = later).
#[test]
fn recording_offset_moves_the_take() {
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::new(Config { sample_rate: RATE, max_block_size: 1024 }).unwrap();
    let mut doc = Document::new(Project::default());
    doc.execute(Command::AddTrack { index: None, track: Track::new(TrackId(0), TrackKind::Audio, "Vox", ColorIndex(0)) }, Origin::User)
        .unwrap();
    let track = doc.project().tracks.last().unwrap().id;
    doc.execute(Command::SetTrackInput { track, input: Some(InputSource { channel: 0, stereo: false }) }, Origin::User).unwrap();
    doc.execute(Command::SetTrackArm { track, arm: true }, Origin::User).unwrap();
    let (snap, _) = snapshot::build(doc.project(), RATE, &HashMap::new());
    engine.publish(&snap).unwrap();
    assert_eq!(engine.recording_offset(), 0);
    engine.set_recording_offset(-480).unwrap(); // takes 10 ms later
    assert_eq!(engine.recording_offset(), -480);
    assert_eq!(engine.set_recording_offset(48_001).unwrap_err().code, dstudio_engine::ffi::SE_ERR_INVALID_ARG);
    engine.seek(96_000).unwrap();
    engine.start_recording(dir.path()).unwrap();
    let input: Vec<f32> = (0..4800).map(|k| input_sample(0, k)).collect();
    let mut out = vec![0f32; 4800 * 2];
    engine.render_offline_with_input(&input, 1, &mut out).unwrap();
    let takes = engine.stop_recording().unwrap();
    assert_eq!(takes.len(), 1);
    assert_eq!((takes[0].start_frame, takes[0].frames), (96_480, 4800));
}
