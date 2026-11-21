//! Preview of an instrument's factory sound from Devices › Instruments: clicking a sound in the
//! Synth's folder plays a short note with that sound in the browser's preview bar, without
//! adding anything to the project.
//!
//! A worker thread renders the note on a private offline engine (`dstudio_engine::offline`; the
//! Synth is built in, so nothing needs the main thread), writes it to a WAV file and loads it
//! like a clicked audio file (`JobResult::Auditioned`). Files are kept in the temp folder, named
//! by a hash of the sound's settings, so a second click plays at once and a sound whose
//! settings change gets a new file.

use crate::controller::{Controller, JobResult};
use dstudio_core::builtin::synth_v2::SYNTH_V2;
use dstudio_core::{ClipId, MidiClip, Note, PPQ, Project, Track, TrackId, TrackKind};
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize};

/// The preview note: C4, held for one beat at 120 BPM, then [TAIL_S] for the release.
const PITCH: u8 = 60;
const VELOCITY: u8 = 100;
const NOTE_S: f64 = 0.5;
const TAIL_S: f64 = 1.5;

/// The preview file of factory sound `index`: the temp folder, a name from its settings.
fn preview_path(index: usize) -> Option<PathBuf> {
    let f = SYNTH_V2.factory.get(index)?;
    let mut h = std::collections::hash_map::DefaultHasher::new();
    f.name.hash(&mut h);
    for (k, v) in SYNTH_V2.factory_values(f) {
        k.hash(&mut h);
        v.to_bits().hash(&mut h);
    }
    (PITCH, VELOCITY, NOTE_S.to_bits(), TAIL_S.to_bits()).hash(&mut h);
    Some(std::env::temp_dir().join("Device Studio").join("Sound previews").join(format!("{:016x}.wav", h.finish())))
}

/// A project that plays factory sound `index` once: one instrument track with the Synth and
/// one note at the start.
fn preview_project(index: usize) -> Project {
    let mut p = Project::default();
    p.tracks.clear();
    let track = TrackId(1);
    let mut t = Track::new(track, TrackKind::Instrument, "Preview", dstudio_core::ColorIndex(0));
    let mut synth = crate::devices::synth_with(Some(index as u16));
    synth.id = dstudio_core::DeviceId(2);
    t.instrument_plugin = Some(synth);
    let mut clip = MidiClip::empty(0, PPQ);
    clip.id = ClipId(3);
    clip.clip_loop = None;
    clip.notes = vec![Note { id: 1, pitch: PITCH, start: 0, length: PPQ, velocity: VELOCITY }];
    t.midi_clips.push(clip);
    p.tracks.push(t);
    p.next_id = 4;
    p
}

impl Controller {
    /// Click on factory sound `index` in the browser: play it in the preview bar.
    pub fn preview_sound(&mut self, index: usize) {
        let Some(path) = preview_path(index) else { return };
        if path.exists() {
            self.audition(path);
            return;
        }
        self.files.audition.loading = Some(path.clone());
        let rate = self.rate;
        let job = dstudio_engine::offline::OfflineRender {
            project: preview_project(index),
            rate,
            transients: Default::default(),
            start: 0,
            frames: ((NOTE_S + TAIL_S) * rate) as usize,
        };
        let engine = self.engine.clone();
        let tx = self.jobs_tx.clone();
        self.pending_jobs += 1;
        std::thread::spawn(move || {
            let result = dstudio_engine::offline::render(job, &AtomicBool::new(false), &AtomicUsize::new(0))
                .map_err(|e| match e {
                    dstudio_engine::offline::OfflineError::Cancelled => "cancelled".to_string(),
                    dstudio_engine::offline::OfflineError::Failed(e) => e,
                })
                .and_then(|buf: Vec<f32>| {
                    // Written beside its name and renamed, so a half-written file is never played.
                    let part = path.with_extension("part");
                    std::fs::create_dir_all(path.parent().expect("a folder")).map_err(|e| e.to_string())?;
                    crate::wavgen::write_wav(&part, rate as u32, 2, &buf).map_err(|e| e.to_string())?;
                    std::fs::rename(&part, &path).map_err(|e| e.to_string())
                })
                .and_then(|()| engine.load_file(&path).map_err(|e| e.to_string()));
            let _ = tx.send(JobResult::Auditioned { path, result });
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_sound_has_its_own_preview_file() {
        let paths: std::collections::HashSet<PathBuf> = (0..SYNTH_V2.factory.len()).filter_map(preview_path).collect();
        assert_eq!(paths.len(), SYNTH_V2.factory.len());
        assert!(preview_path(SYNTH_V2.factory.len()).is_none());
    }

    #[test]
    fn the_preview_project_plays_one_note_on_the_synth_with_the_sound() {
        let p = preview_project(5);
        assert_eq!(p.tracks.len(), 1);
        let t = &p.tracks[0];
        assert_eq!(t.instrument_plugin.as_ref().and_then(|d| d.preset.as_deref()), Some(SYNTH_V2.factory[5].name));
        assert_eq!(t.midi_clips[0].notes.len(), 1);
    }
}
