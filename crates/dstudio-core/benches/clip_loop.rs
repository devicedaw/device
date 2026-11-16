//! Clip loop cost on the edit path (every edit rebuilds the engine snapshot, which unrolls
//! looping clips): unroll alone, and snapshot build for looping clips against the same
//! notes written out in clips that play once. Prints medians; no thresholds.
//!
//!   cargo bench -p dstudio-core --bench clip_loop

use dstudio_core::midi::{ClipLoop, Note};
use dstudio_core::*;
use std::collections::HashMap;
use std::hint::black_box;
use std::time::Instant;

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

fn bar_notes(bars: i64) -> Vec<Note> {
    (0..bars * 16)
        .map(|i| Note { id: i as u32 + 1, pitch: 48 + (i % 24) as u8, start: i * PPQ / 4, length: PPQ / 5, velocity: 100 })
        .collect()
}

fn project(tracks: usize, bars: i64, looped: bool) -> Document {
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
        let length = bars * 4 * PPQ;
        let mut c = MidiClip::empty(0, length);
        if looped {
            c.notes = bar_notes(1);
            c.clip_loop = Some(ClipLoop::whole(4 * PPQ));
        } else {
            c.notes = bar_notes(bars);
            c.clip_loop = None;
        }
        d.execute(Command::AddMidiClip { track, clip: c }, Origin::User).unwrap();
    }
    d
}

fn main() {
    println!("{:<40} {:>8} {:>12}", "ClipLoop::unroll (16 notes per bar)", "notes", "median us");
    for bars in [4i64, 16, 64] {
        let notes = bar_notes(1);
        let l = ClipLoop::whole(4 * PPQ);
        let n = l.unroll(&notes, bars * 4 * PPQ).len();
        let us = median_us(201, || {
            black_box(black_box(&l).unroll(black_box(&notes), bars * 4 * PPQ));
        });
        println!("{:<40} {n:>8} {us:>12.1}", format!("1-bar loop over {bars} bars"));
    }

    println!("\n{:<44} {:>10} {:>14}", "snapshot::build", "notes", "median us");
    for (tracks, bars) in [(8usize, 16i64), (32, 64)] {
        for looped in [false, true] {
            let d = project(tracks, bars, looped);
            let (snap, _) = snapshot::build(d.project(), 48_000.0, &HashMap::new());
            let n: usize = snap.tracks.iter().map(|t| t.notes.len()).sum();
            let us = median_us(51, || {
                black_box(snapshot::build(black_box(d.project()), 48_000.0, &HashMap::new()));
            });
            let what = if looped { "1-bar loops" } else { "notes written out" };
            println!("{:<44} {n:>10} {us:>14.1}", format!("{tracks} clips x {bars} bars, {what}"));
        }
    }
}
