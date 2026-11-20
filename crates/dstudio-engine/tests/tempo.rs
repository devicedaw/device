//! Tempo changes end to end (spec 015): the engine evaluates the tempo map core sends with the
//! same formula, so the metronome clicks on the frames core computes for each beat.

use dstudio_core::*;
use dstudio_engine::{Config, Engine, MetronomeConfig};
use std::collections::HashMap;

const RATE: f64 = 48_000.0;

fn onsets(out: &[f32]) -> Vec<i64> {
    (0..out.len() / 2).filter(|&i| out[2 * i] != 0.0 && (i == 0 || out[2 * i - 2] == 0.0)).map(|i| i as i64).collect()
}

/// Renders `frames` with the metronome on, the project published through core's snapshot.
fn clicks(p: &Project, frames: usize) -> Vec<i64> {
    let e = Engine::new(Config { sample_rate: RATE, max_block_size: 256 }).unwrap();
    let (snap, _) = snapshot::build(p, RATE, &HashMap::new());
    e.publish(&snap).unwrap();
    e.set_metronome(MetronomeConfig { enabled: true, gain: 0.5, frames_per_beat: 24_000.0, beats_per_bar: 4 }).unwrap();
    e.play().unwrap();
    let mut out = vec![0.0f32; frames * 2];
    for chunk in out.chunks_mut(2 * 300) {
        e.render_offline(chunk).unwrap();
    }
    onsets(&out)
}

#[test]
fn the_metronome_clicks_on_the_beats_core_computes() {
    let mut d = Document::new(Project::default());
    // 300 BPM for a bar, a step to 400, a ramp down to 150 over two bars, then 150.
    let points = vec![
        TempoPoint { time: 0, bpm: 300.0, ramp: false },
        TempoPoint { time: 4 * PPQ, bpm: 400.0, ramp: true },
        TempoPoint { time: 12 * PPQ, bpm: 150.0, ramp: false },
    ];
    d.execute(Command::SetTempoMap { points, bpm: None }, Origin::User).unwrap();
    let p = d.project();
    let beats = 16;
    let want: Vec<i64> = (0..=beats).map(|k| p.ticks_to_frames(k * PPQ, RATE)).collect();
    let got = clicks(p, (want[beats as usize] + 10) as usize);
    assert_eq!(got.len(), want.len(), "{got:?}");
    for (g, w) in got.iter().zip(&want) {
        assert!((g - w).abs() <= 1, "click at {g}, core says {w}");
    }

    // 6/8 clicks eighths on the same map.
    d.execute(Command::SetTimeSignature { numerator: 6, denominator: 8 }, Origin::User).unwrap();
    let p = d.project();
    let want: Vec<i64> = (0..=8).map(|k| p.ticks_to_frames(k * PPQ / 2, RATE)).collect();
    let got = clicks(p, (want[8] + 10) as usize);
    assert_eq!(got.len(), want.len());
    for (g, w) in got.iter().zip(&want) {
        assert!((g - w).abs() <= 1, "click at {g}, core says {w}");
    }
}

#[test]
fn without_changes_the_click_grid_is_unchanged() {
    let mut d = Document::new(Project::default());
    d.execute(Command::SetTempo { bpm: 240.0, at: None }, Origin::User).unwrap();
    let got = clicks(d.project(), 60_000);
    assert_eq!(got, vec![0, 24_000, 48_000], "frames_per_beat from se_metronome_set: no map was sent");
}
