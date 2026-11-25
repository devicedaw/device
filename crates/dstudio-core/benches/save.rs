//! What one project write costs (`persist::save`, the same write as Save and autosave):
//! serialise to JSON, write a temp file, fsync, rename. Autosave clones the project on the
//! UI thread and writes the clone on its own thread, so the clone is what a backup costs a
//! frame. Prints medians; no thresholds.
//!
//!   cargo bench -p dstudio-core --bench save

use dstudio_core::steps::{StepPattern, StepRate, StepRows};
use dstudio_core::*;
use std::hint::black_box;
use std::time::Instant;

fn median_ms(runs: usize, mut f: impl FnMut()) -> f64 {
    let mut t: Vec<f64> = (0..runs)
        .map(|_| {
            let s = Instant::now();
            f();
            s.elapsed().as_secs_f64() * 1e3
        })
        .collect();
    t.sort_by(f64::total_cmp);
    t[runs / 2]
}

/// `tracks` instrument tracks, each with `clips` one-bar clips of 16 notes.
fn project(tracks: usize, clips: i64) -> Project {
    let mut d = Document::new(Project::default());
    let inst = d.project().tracks[1].id;
    let mut p = StepPattern { rate: StepRate::Sixteenth, steps: 16, ..StepPattern::default() };
    for s in 0..16 {
        p = p.toggled(36 + (s % 8) as u8, s);
    }
    let notes = p.bake();
    for t in 0..tracks {
        let track = if t == 0 {
            inst
        } else {
            let tr = d.project().track_copy(inst, false).unwrap();
            d.execute(Command::AddTrack { index: None, track: tr }, Origin::User).unwrap();
            d.project().tracks.last().unwrap().id
        };
        for c in 0..clips {
            let mut clip = MidiClip::empty_pattern(0, StepRows::Pads);
            clip.pattern = None;
            clip.notes = notes.clone();
            clip.start = c * 4 * PPQ;
            clip.length = 4 * PPQ;
            d.execute(Command::AddMidiClip { track, clip }, Origin::User).unwrap();
        }
    }
    d.project().clone()
}

fn main() {
    let dir = std::env::temp_dir().join(format!("dstudio-bench-save-{}", std::process::id()));
    println!("{:<34} {:>10} {:>12} {:>12}", "project", "JSON KB", "save ms", "clone ms");
    for (tracks, clips) in [(8usize, 16i64), (32, 64), (64, 128)] {
        let p = project(tracks, clips);
        let target = dir.join("Song.dsproj");
        persist::save(&p, &target).unwrap();
        let kb = std::fs::metadata(target.join(persist::PROJECT_FILE)).map_or(0, |m| m.len() / 1024);
        let ms = median_ms(21, || persist::save(black_box(&p), &target).unwrap());
        let clone = median_ms(21, || drop(black_box(black_box(&p).clone())));
        println!("{:<34} {kb:>10} {ms:>12.2} {clone:>12.2}", format!("{tracks} tracks x {clips} clips x 16 notes"));
    }
    let _ = std::fs::remove_dir_all(&dir);
}
