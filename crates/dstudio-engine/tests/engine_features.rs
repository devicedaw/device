//! End-to-end tests for the ABI-v1 additions: synchronous offline transport,
//! loop playback, the metronome and non-destructive meter peeking.

use dstudio_core::snapshot::{ClipSnapshot, SessionSnapshot, TrackSnapshot};
use dstudio_engine::{Config, Engine, Meter, MetronomeConfig};
use std::path::Path;

const RATE: u32 = 48_000;

/// 16-bit PCM mono WAV whose sample i is `(i % 30000) as i16`, so every
/// decoded value is exact in f32 and distinct within the first 30000 frames.
fn write_ramp_wav(path: &Path, frames: u32) -> Vec<f32> {
    let data_len = frames * 2;
    let mut b = Vec::with_capacity(44 + data_len as usize);
    b.extend_from_slice(b"RIFF");
    b.extend_from_slice(&(36 + data_len).to_le_bytes());
    b.extend_from_slice(b"WAVEfmt ");
    b.extend_from_slice(&16u32.to_le_bytes());
    b.extend_from_slice(&1u16.to_le_bytes()); // PCM
    b.extend_from_slice(&1u16.to_le_bytes()); // mono
    b.extend_from_slice(&RATE.to_le_bytes());
    b.extend_from_slice(&(RATE * 2).to_le_bytes());
    b.extend_from_slice(&2u16.to_le_bytes());
    b.extend_from_slice(&16u16.to_le_bytes());
    b.extend_from_slice(b"data");
    b.extend_from_slice(&data_len.to_le_bytes());
    let mut expect = Vec::with_capacity(frames as usize);
    for i in 0..frames {
        let v = (i % 30_000) as i16;
        b.extend_from_slice(&v.to_le_bytes());
        expect.push(v as f32 / 32768.0);
    }
    std::fs::write(path, b).unwrap();
    expect
}

fn engine() -> Engine {
    Engine::new(Config { sample_rate: RATE as f64, max_block_size: 256 }).unwrap()
}

/// One hard-left track (L = source exactly, R = 0) with a single clip.
fn publish_ramp(e: &Engine, dir: &Path, frames: u32) -> Vec<f32> {
    publish_ramp_faded(e, dir, frames, (0, 0))
}

/// [publish_ramp] with the clip's (fade-in, fade-out) in frames.
fn publish_ramp_faded(e: &Engine, dir: &Path, frames: u32, fades: (i64, i64)) -> Vec<f32> {
    let path = dir.join("ramp.wav");
    let src = write_ramp_wav(&path, frames);
    let info = e.load_file(&path).unwrap();
    let snap = SessionSnapshot {
        tracks: vec![TrackSnapshot {
            track_id: 1,
            is_return: false,
            output: None,
            sends: Vec::new(),
            gain: 1.0,
            pan: -1.0,
            trim: 1.0,
            width: 1.0,
            mono: false,
            mute: false,
            solo: false,
            instrument: false,
            notes: vec![],
            synth: None,
            devices: vec![],
            instrument_plugin: None,
            automation: vec![],
            arm: false,
            monitor: false,
            input: None,
            clips: vec![ClipSnapshot {
                clip_id: 1,
                engine_asset: info.asset_id,
                timeline_start: 0,
                source_offset: 0,
                length: frames as i64,
                gain: 1.0,
                warp: None,
                clip_loop: None,
                fades,
            }],
        }],
        master_gain: 1.0,
        master_automation: vec![],
        device_assets: Default::default(),
        tempo: vec![],
        click_quarters: 1.0,
    };
    e.publish(&snap).unwrap();
    src
}

/// Render `total` frames in odd-sized chunks; returns interleaved stereo.
fn render(e: &Engine, total: usize) -> Vec<f32> {
    let mut out = vec![0f32; total * 2];
    let sizes = [7usize, 333, 64, 1, 129, 500];
    let mut done = 0;
    let mut k = 0;
    while done < total {
        let n = sizes[k % sizes.len()].min(total - done);
        e.render_offline(&mut out[done * 2..(done + n) * 2]).unwrap();
        done += n;
        k += 1;
    }
    out
}

/// Frames where a click starts (left channel non-zero after silence).
fn onsets(inter: &[f32]) -> Vec<usize> {
    let l: Vec<f32> = inter.iter().step_by(2).copied().collect();
    (0..l.len()).filter(|&i| l[i] != 0.0 && (i == 0 || l[i - 1] == 0.0)).collect()
}

#[test]
fn clip_fades_ramp_the_clip_in_and_out() {
    let dir = tempfile::tempdir().unwrap();
    let e = engine();
    let src = publish_ramp_faded(&e, dir.path(), 10_000, (1_000, 4_000));
    e.play().unwrap();
    let out = render(&e, 10_000);
    let l: Vec<f32> = out.iter().step_by(2).copied().collect();
    for (r, (&got, &s)) in l.iter().zip(&src).enumerate() {
        let g = (r as f32 / 1_000.0).min(1.0) * ((10_000 - r) as f32 / 4_000.0).min(1.0);
        assert!((got - s * g).abs() < 1e-6, "frame {r}: {got} vs {}", s * g);
    }
}

#[test]
fn offline_seek_is_immediately_visible() {
    let e = engine();
    assert!(!e.device_running());
    e.seek(96_000).unwrap();
    assert_eq!(e.transport().position, 96_000, "seek must apply without rendering");
    e.play().unwrap();
    assert!(e.transport().playing);
    e.pause().unwrap();
    assert!(!e.transport().playing);
    e.seek(-480).unwrap();
    assert_eq!(e.transport().position, -480);
    e.stop().unwrap();
    let t = e.transport();
    assert_eq!((t.position, t.playing), (0, false));
}

#[test]
fn loop_wraps_sample_accurately_in_offline_render() {
    let dir = tempfile::tempdir().unwrap();
    let e = engine();
    let src = publish_ramp(&e, dir.path(), 20_000);
    let (start, end) = (4_000i64, 4_777i64);
    assert!(e.set_loop(true, 10, 10).is_err());
    e.set_loop(true, start, end).unwrap();
    e.seek(3_900).unwrap();
    e.play().unwrap();

    let total = 3_000usize;
    let out = render(&e, total);
    let mut p = 3_900i64;
    for i in 0..total {
        assert_eq!(out[2 * i], src[p as usize], "frame {i} (timeline {p})");
        assert_eq!(out[2 * i + 1], 0.0);
        p += 1;
        if p == end {
            p = start;
        }
    }
    let pos = e.transport().position;
    assert_eq!(pos, p);
    assert!((start..end).contains(&pos));

    // Disabling mid-play continues linearly.
    e.set_loop(false, 0, 0).unwrap();
    e.seek(4_770).unwrap();
    let out = render(&e, 20);
    for i in 0..20 {
        assert_eq!(out[2 * i], src[4_770 + i]);
    }
}

#[test]
fn metronome_clicks_at_expected_frames_120_bpm() {
    let e = engine();
    // 120 BPM at 48 kHz = 24000 frames per beat, 4/4.
    let cfg = MetronomeConfig { enabled: true, gain: 0.5, frames_per_beat: 24_000.0, beats_per_bar: 4 };
    assert!(e.set_metronome(MetronomeConfig { frames_per_beat: 0.0, ..cfg }).is_err());
    assert!(e.set_metronome(MetronomeConfig { beats_per_bar: 0, ..cfg }).is_err());
    e.set_metronome(cfg).unwrap();
    e.play().unwrap();
    let out = render(&e, 5 * 24_000 + 100);
    assert_eq!(onsets(&out), vec![0, 24_000, 48_000, 72_000, 96_000, 120_000]);

    // Downbeats (beats 0 and 4) are higher-pitched than the other beats.
    let crossings = |from: usize| (from + 1..from + 700).filter(|&i| (out[2 * i - 2] < 0.0) != (out[2 * i] < 0.0)).count();
    assert!(crossings(0) > crossings(24_000) + 6);
    assert_eq!(crossings(0), crossings(96_000));
    let peak = out.iter().fold(0f32, |m, v| m.max(v.abs()));
    assert!(peak > 0.4 && peak <= 0.5, "peak {peak}");

    // Across a loop wrap: loop [12000, 60000) after playing from 0.
    let e = engine();
    e.set_metronome(cfg).unwrap();
    e.set_loop(true, 12_000, 60_000).unwrap();
    e.play().unwrap();
    let out = render(&e, 110_000);
    // Timeline: 0..60000 (beats 0, 24000, 48000), wrap at output 60000 to
    // 12000 (next beats 24000 -> out 72000, 48000 -> out 96000).
    assert_eq!(onsets(&out), vec![0, 24_000, 48_000, 72_000, 96_000]);

    // Disabled -> silent.
    e.set_metronome(MetronomeConfig { enabled: false, ..cfg }).unwrap();
    let out = render(&e, 50_000);
    assert!(out.iter().all(|&v| v == 0.0));
}

#[test]
fn export_excludes_loop_and_metronome() {
    let dir = tempfile::tempdir().unwrap();
    let e = engine();
    let src = publish_ramp(&e, dir.path(), 10_000);
    e.set_loop(true, 100, 300).unwrap();
    e.set_metronome(MetronomeConfig { enabled: true, gain: 1.0, frames_per_beat: 480.0, beats_per_bar: 4 }).unwrap();
    e.play().unwrap();
    let path = dir.path().join("out.wav");
    e.export_wav(&path, 0, 5_000).unwrap();
    // Re-import the export: it must be the linear clip, no clicks, no wrap.
    let info = e.load_file(&path).unwrap();
    assert_eq!(info.frames, 5_000);
    let snap = SessionSnapshot {
        tracks: vec![TrackSnapshot {
            track_id: 1,
            is_return: false,
            output: None,
            sends: Vec::new(),
            gain: 1.0,
            pan: -1.0,
            trim: 1.0,
            width: 1.0,
            mono: false,
            mute: false,
            solo: false,
            instrument: false,
            notes: vec![],
            synth: None,
            devices: vec![],
            instrument_plugin: None,
            automation: vec![],
            arm: false,
            monitor: false,
            input: None,
            clips: vec![ClipSnapshot {
                clip_id: 1,
                engine_asset: info.asset_id,
                timeline_start: 0,
                source_offset: 0,
                length: 5_000,
                gain: 1.0,
                warp: None,
                clip_loop: None,
                fades: Default::default(),
            }],
        }],
        master_gain: 1.0,
        master_automation: vec![],
        device_assets: Default::default(),
        tempo: vec![],
        click_quarters: 1.0,
    };
    e.publish(&snap).unwrap();
    e.set_metronome(MetronomeConfig { enabled: false, gain: 0.0, frames_per_beat: 1.0, beats_per_bar: 1 }).unwrap();
    e.set_loop(false, 0, 0).unwrap();
    e.seek(0).unwrap();
    let out = render(&e, 5_000);
    // Hard-left on a stereo asset keeps L = exported L, so the exported left
    // channel round-trips; compare it with the original ramp.
    for i in 0..5_000 {
        assert_eq!(out[2 * i], src[i], "frame {i}");
    }
}

#[test]
fn peek_meters_is_non_destructive() {
    let dir = tempfile::tempdir().unwrap();
    let e = engine();
    publish_ramp(&e, dir.path(), 20_000);
    e.seek(10_000).unwrap(); // ramp value 10000/32768 here
    e.play().unwrap();
    let mut buf = vec![0f32; 2 * 64];
    e.render_offline(&mut buf).unwrap();
    e.pause().unwrap();
    let expect = 10_063.0f32 / 32768.0;
    let mut m: Vec<Meter> = Vec::new();
    for _ in 0..10 {
        e.peek_meters(&mut m, 8);
        assert_eq!(m.len(), 2);
        assert_eq!(m[0].track_id, 0);
        assert!((m[0].peak_l - expect).abs() < 1e-6, "{:?}", m[0]);
        assert_eq!(m[1].track_id, 1);
        assert!((m[1].peak_l - expect).abs() < 1e-6);
    }
    // The destructive read still works and does not disturb peeking.
    e.meters(&mut m, 8);
    assert!((m[0].peak_l - expect).abs() < 1e-6);
    e.meters(&mut m, 8);
    assert_eq!(m[0].peak_l, 0.0);
    e.peek_meters(&mut m, 8);
    assert!((m[0].peak_l - expect).abs() < 1e-6);
    // ~50 ms of rendered silence clears the window.
    let mut silence = vec![0f32; 2 * 2_400];
    e.render_offline(&mut silence).unwrap();
    e.peek_meters(&mut m, 8);
    assert_eq!(m[0].peak_l, 0.0);
    assert_eq!(m[1].peak_l, 0.0);
}
