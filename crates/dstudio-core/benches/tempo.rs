//! Snapshot cost of the tempo map (spec 015): `snapshot::build` for a project of N tracks (half
//! instrument tracks with a 256-note MIDI clip, half audio tracks with a warped and an unwarped
//! clip; every track with a 64-point volume lane), with one tempo and with a 64-point tempo map
//! (a point every bar, alternately holding and ramping between 90 and 150 BPM).
//! Prints measurements only.
//!
//! usage: cargo bench -p dstudio-core --bench tempo

use dstudio_core::automation::{AutoPoint, AutomationLane, AutomationTarget};
use dstudio_core::snapshot;
use dstudio_core::warp::ClipWarp;
use dstudio_core::*;
use std::collections::HashMap;
use std::hint::black_box;
use std::time::Instant;

fn project(tracks: usize) -> (Document, HashMap<AssetId, u64>) {
    let mut d = Document::new(Project::default());
    let asset = Asset {
        id: AssetId(0),
        relative_path: None,
        absolute_path: "/x.wav".into(),
        sample_rate: 44_100.0,
        channels: 2,
        frames: 44_100 * 60,
    };
    d.execute(Command::AddAsset { asset }, Origin::User).unwrap();
    let a = d.project().assets[0].id;
    for t in 0..tracks {
        let kind = if t % 2 == 0 { TrackKind::Instrument } else { TrackKind::Audio };
        d.execute(Command::AddTrack { index: None, track: Track::new(TrackId(0), kind, "T", ColorIndex(1)) }, Origin::User).unwrap();
        let track = d.project().tracks.last().unwrap().id;
        if kind == TrackKind::Instrument {
            d.execute(Command::AddMidiClip { track, clip: MidiClip::empty(0, 64 * 4 * PPQ) }, Origin::User).unwrap();
            let clip = d.project().tracks.last().unwrap().midi_clips[0].id;
            let notes = (0..256)
                .map(|i| Note { id: i + 1, pitch: 60 + (i % 12) as u8, start: i as i64 * PPQ, length: PPQ / 2, velocity: 100 })
                .collect();
            d.execute(Command::SetMidiNotes { clip, notes }, Origin::User).unwrap();
        } else {
            for (start, warp) in
                [(0, ClipWarp { enabled: true, original_bpm: 96.0, ..ClipWarp::default() }), (32 * 4 * PPQ, ClipWarp::default())]
            {
                let clip = Clip {
                    id: ClipId(0),
                    asset: a,
                    name: "c".into(),
                    start,
                    source_offset: 0,
                    source_length: 44_100 * 30,
                    gain: 1.0,
                    warp,
                    disabled: false,
                    clip_loop: None,
                    fades: Default::default(),
                    takes: Vec::new(),
                };
                d.execute(Command::AddClip { track, clip }, Origin::User).unwrap();
            }
        }
        let mut lane = AutomationLane::new(AutomationTarget::Volume);
        lane.points = (0..64).map(|i| AutoPoint { time: i * 4 * PPQ, value: -(i % 12) as f32, curve: 0.0 }).collect();
        d.execute(Command::SetAutomationLane { track: Some(track), target: AutomationTarget::Volume, lane: Some(lane) }, Origin::User)
            .unwrap();
    }
    (d, HashMap::from([(a, 1)]))
}

fn time<F: FnMut()>(iters: u32, mut f: F) -> f64 {
    f();
    let t0 = Instant::now();
    for _ in 0..iters {
        f();
    }
    t0.elapsed().as_secs_f64() * 1e6 / iters as f64
}

fn main() {
    println!("| tracks | one tempo build µs | 64-point map build µs |");
    println!("|---:|---:|---:|");
    for tracks in [8usize, 32] {
        let (mut d, assets) = project(tracks);
        let plain = time(100, || {
            black_box(snapshot::build(black_box(d.project()), 48_000.0, &assets));
        });
        // map-row
        let points =
            (0..64).map(|i| TempoPoint { time: i * 4 * PPQ, bpm: if i % 4 < 2 { 90.0 } else { 150.0 }, ramp: i % 2 == 1 }).collect(); // map-row
        d.execute(Command::SetTempoMap { points, bpm: None }, Origin::User).unwrap(); // map-row
        let mapped = time(100, || {
            // map-row
            black_box(snapshot::build(black_box(d.project()), 48_000.0, &assets)); // map-row
        }); // map-row
        println!("| {tracks} | {plain:.1} | {mapped:.1} |"); // map-row
        let _ = &mut d;
    }
}
