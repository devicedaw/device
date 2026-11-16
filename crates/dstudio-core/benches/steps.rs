//! Step pattern costs on the edit path (every step edit bakes the pattern
//! and rebuilds the engine snapshot): bake, read back (derive), and snapshot
//! build for a project of pattern clips. Prints medians; no thresholds.
//!
//!   cargo bench -p dstudio-core --bench steps

use dstudio_core::steps::{StepLane, StepPattern, StepRate, StepRows, bake_into};
use dstudio_core::*;
use std::collections::HashMap;
use std::hint::black_box;
use std::time::Instant;

fn pattern(steps: u16, rate: StepRate, every: u16, repeat: i32) -> StepPattern {
    let mut p = StepPattern { rate, steps, ..StepPattern::default() };
    for pad in 0..16u8 {
        for s in (0..steps).filter(|s| (s + pad as u16).is_multiple_of(every)) {
            p = p.toggled(36 + pad, s).with_lane(36 + pad, s, StepLane::Repeat, repeat).with_lane(
                36 + pad,
                s,
                StepLane::Velocity,
                60 + (s as i32 * 7) % 60,
            );
        }
    }
    p
}

fn median_us(runs: usize, mut f: impl FnMut()) -> f64 {
    let mut t: Vec<f64> = (0..runs)
        .map(|_| {
            let s = Instant::now();
            f();
            s.elapsed().as_secs_f64() * 1e6
        })
        .collect();
    t.sort_by(f64::total_cmp);
    t[runs / 2]
}

fn main() {
    println!("{:<34} {:>8} {:>12} {:>12}", "pattern (16 pads)", "notes", "bake us", "derive us");
    for (name, p) in [
        ("16 x 1/16, every 4th step", pattern(16, StepRate::Sixteenth, 4, 1)),
        ("16 x 1/16, every step", pattern(16, StepRate::Sixteenth, 1, 1)),
        ("64 x 1/32, every step, repeat 4", pattern(64, StepRate::ThirtySecond, 1, 4)),
        ("64 x 1/32T, every step, repeat 16", pattern(64, StepRate::ThirtySecondTriplet, 1, 16)),
    ] {
        let notes = p.bake();
        let bake = median_us(201, || {
            black_box(bake_into(black_box(&p), &[]));
        });
        let derive = median_us(201, || {
            black_box(StepPattern::derive(black_box(&notes), p.rate, p.steps, p.swing, StepRows::Pads));
        });
        println!("{name:<34} {:>8} {bake:>12.1} {derive:>12.1}", notes.len());
    }

    println!("\n{:<44} {:>10} {:>14}", "snapshot::build", "notes", "median us");
    for (tracks, bars) in [(8usize, 16i64), (32, 64)] {
        let mut d = Document::new(Project::default());
        let inst = d.project().tracks[1].id;
        for t in 0..tracks {
            let track = if t == 0 {
                inst
            } else {
                // A copy with fresh ids: AddTrack refuses device ids already in use.
                let tr = d.project().track_copy(inst, false).unwrap();
                d.execute(Command::AddTrack { index: None, track: tr }, Origin::User).unwrap();
                d.project().tracks.last().unwrap().id
            };
            let mut c = MidiClip::empty_pattern(0, StepRows::Pads);
            let p = pattern(16, StepRate::Sixteenth, 1 + t as u16 % 4, 1 + t as i32 % 3);
            c.notes = p.bake();
            c.pattern = Some(p);
            c.length = bars * 4 * PPQ;
            d.execute(Command::AddMidiClip { track, clip: c }, Origin::User).unwrap();
        }
        let (snap, _) = snapshot::build(d.project(), 48_000.0, &HashMap::new());
        let n: usize = snap.tracks.iter().map(|t| t.notes.len()).sum();
        let us = median_us(51, || {
            black_box(snapshot::build(black_box(d.project()), 48_000.0, &HashMap::new()));
        });
        println!("{:<44} {n:>10} {us:>14.1}", format!("{tracks} pattern clips x {bars} bars (repeats)"));
    }
}
