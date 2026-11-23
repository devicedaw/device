//! Cost of "Let the assistant listen to audio" (roadmap G20): `listen::analyze` on the longest
//! stretch a turn measures (30 s of stereo at 48 kHz), a chord over a 120 BPM click so the key
//! and tempo searches do real work. Runs on the assistant's worker thread, once per turn.
//! Prints measurements only.
//!
//! usage: cargo bench -p dstudio-core --bench listen

use dstudio_core::listen;
use std::f64::consts::PI;
use std::hint::black_box;
use std::time::Instant;

fn main() {
    let rate = 48_000.0;
    let frames = (listen::MAX_SECONDS * rate) as usize;
    let samples: Vec<f32> = (0..frames)
        .flat_map(|i| {
            let t = i as f64 / rate;
            let chord: f64 = [220.0, 261.63, 329.63].iter().map(|f| 0.1 * (2.0 * PI * f * t).sin()).sum();
            let p = t % 0.5;
            let click = if p < 0.02 { 0.4 * (2.0 * PI * 2000.0 * t).sin() } else { 0.0 };
            let v = (chord + click) as f32;
            [v, v]
        })
        .collect();
    black_box(listen::analyze(&samples, rate));
    let runs = 5;
    let mut ms = Vec::with_capacity(runs);
    for _ in 0..runs {
        let t0 = Instant::now();
        black_box(listen::analyze(black_box(&samples), rate));
        ms.push(t0.elapsed().as_secs_f64() * 1e3);
    }
    ms.sort_by(f64::total_cmp);
    println!(
        "listen::analyze 30 s stereo 48 kHz: median {:.1} ms, min {:.1} ms, max {:.1} ms ({runs} runs)",
        ms[runs / 2],
        ms[0],
        ms[runs - 1]
    );
}
