//! End-to-end routing through the Rust wrapper (spec 002, A7): a track's send
//! and its Out reach the master through a return track, and the routing goes
//! through `Engine::publish` on ABI v9.
//!
//! Every track is panned hard left (left gain 1, right gain 0), so the left
//! channel carries the signal unscaled.

use dstudio_core::snapshot::{ClipSnapshot, SendSnapshot, SessionSnapshot, TrackSnapshot};
use dstudio_engine::{Config, Engine};
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

fn track(id: u64) -> TrackSnapshot {
    TrackSnapshot {
        track_id: id,
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
        clips: vec![],
    }
}

/// Track 1 plays the ramp; track 9 is a return. `route` adjusts the tracks.
fn publish(e: &Engine, asset: u64, frames: u32, route: impl FnOnce(&mut TrackSnapshot, &mut TrackSnapshot)) {
    let mut t = track(1);
    t.clips = vec![ClipSnapshot {
        clip_id: 1,
        engine_asset: asset,
        timeline_start: 0,
        source_offset: 0,
        length: frames as i64,
        gain: 1.0,
        warp: None,
        clip_loop: None,
        fades: Default::default(),
    }];
    let mut r = track(9);
    r.is_return = true;
    route(&mut t, &mut r);
    let snap = SessionSnapshot {
        tracks: vec![t, r],
        master_gain: 1.0,
        master_automation: vec![],
        device_assets: Default::default(),
        tempo: vec![],
        click_quarters: 1.0,
    };
    e.publish(&snap).unwrap();
}

fn left(e: &Engine, total: usize) -> Vec<f32> {
    e.seek(0).unwrap();
    e.play().unwrap();
    let out = render(e, total);
    e.stop().unwrap();
    out.iter().step_by(2).copied().collect()
}

#[test]
fn send_and_output_reach_the_master_through_a_return() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("ramp.wav");
    let frames = 4_000;
    let src = write_ramp_wav(&path, frames);
    let e = engine();
    let asset = e.load_file(&path).unwrap().asset_id;
    let n = frames as usize;

    // Post-fader send at 0.5 into a return at gain 0.5: direct + 0.25.
    publish(&e, asset, frames, |t, r| {
        t.gain = 0.5;
        t.sends.push(SendSnapshot { target: 9, gain: 0.5, pre_fader: false });
        r.gain = 0.5;
    });
    let l = left(&e, n);
    for i in (0..n).step_by(97) {
        let want = src[i] * 0.5 + src[i] * 0.5 * 0.5 * 0.5;
        assert!((l[i] - want).abs() < 1e-6, "post send frame {i}: {} vs {want}", l[i]);
    }

    // Pre-fader: the send ignores the track's fader.
    publish(&e, asset, frames, |t, _| {
        t.gain = 0.5;
        t.sends.push(SendSnapshot { target: 9, gain: 0.5, pre_fader: true });
    });
    let l = left(&e, n);
    for i in (0..n).step_by(97) {
        let want = src[i] * 0.5 + src[i] * 0.5;
        assert!((l[i] - want).abs() < 1e-6, "pre send frame {i}: {} vs {want}", l[i]);
    }

    // Out to the return: the master hears the track only through it, so
    // muting the return silences the track.
    publish(&e, asset, frames, |t, r| {
        t.output = Some(9);
        r.gain = 0.25;
    });
    let l = left(&e, n);
    for i in (0..n).step_by(97) {
        assert!((l[i] - src[i] * 0.25).abs() < 1e-6, "output frame {i}");
    }
    publish(&e, asset, frames, |t, r| {
        t.output = Some(9);
        r.mute = true;
    });
    assert!(left(&e, n).iter().all(|&v| v == 0.0), "a muted return silences what is routed to it");
}

#[test]
fn routing_to_a_track_that_is_not_a_return_is_refused() {
    let e = engine();
    let mut t = track(1);
    t.sends.push(SendSnapshot { target: 2, gain: 1.0, pre_fader: false });
    let snap = SessionSnapshot {
        tracks: vec![t, track(2)],
        master_gain: 1.0,
        master_automation: vec![],
        device_assets: Default::default(),
        tempo: vec![],
        click_quarters: 1.0,
    };
    assert!(e.publish(&snap).is_err());
}
