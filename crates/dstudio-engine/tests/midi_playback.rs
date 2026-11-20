//! End-to-end MIDI: document commands (AddMidiClip + SetMidiNotes) →
//! snapshot → `se_session_publish_v2` → built-in synth → offline render.

use dstudio_core::*;
use dstudio_engine::{Config, Engine};
use std::collections::HashMap;

const RATE: f64 = 48_000.0;

fn render(e: &Engine, frames: usize) -> Vec<f32> {
    let mut out = vec![0f32; frames * 2];
    let mut done = 0;
    for chunk in [127usize, 512, 1000, 33].iter().cycle() {
        if done >= frames {
            break;
        }
        let n = (*chunk).min(frames - done);
        e.render_offline(&mut out[done * 2..(done + n) * 2]).unwrap();
        done += n;
    }
    out
}

fn left(inter: &[f32]) -> Vec<f32> {
    inter.iter().step_by(2).copied().collect()
}

fn rms(x: &[f32]) -> f64 {
    (x.iter().map(|&v| (v as f64) * (v as f64)).sum::<f64>() / x.len() as f64).sqrt()
}

/// Frequency from the autocorrelation peak near `periods` periods of
/// `expected` (parabolic refinement). The synth's two detuned oscillators
/// beat, so zero-crossing counts are not reliable.
fn estimate_hz(x: &[f32], expected: f64, periods: f64) -> f64 {
    let n = x.len() / 2;
    let r = |lag: usize| -> f64 { (0..n).map(|i| x[i] as f64 * x[i + lag] as f64).sum() };
    let center = periods * RATE / expected;
    let (lo, hi) = ((center * 0.97) as usize, (center * 1.03) as usize + 1);
    let mut best = lo;
    let mut best_r = r(lo);
    for lag in lo + 1..=hi {
        let v = r(lag);
        if v > best_r {
            best_r = v;
            best = lag;
        }
    }
    let (y0, y2) = (r(best - 1), r(best + 1));
    let delta = 0.5 * (y0 - y2) / (y0 - 2.0 * best_r + y2);
    periods * RATE / (best as f64 + delta)
}

fn publish(engine: &Engine, doc: &Document) -> snapshot::SessionSnapshot {
    let (snap, skipped) = snapshot::build(doc.project(), engine.sample_rate(), &HashMap::new());
    assert!(skipped.is_empty());
    engine.publish(&snap).unwrap();
    snap
}

#[test]
fn midi_notes_play_through_builtin_synth_and_undo_silences_them() {
    let engine = Engine::new(Config { sample_rate: RATE, max_block_size: 1024 }).unwrap();
    let mut doc = Document::new(Project::default());
    // Default project: one audio and one instrument track, 120 BPM (1 beat = 24000 frames).
    let inst = doc.project().tracks.iter().find(|t| t.kind == TrackKind::Instrument).unwrap().id;
    // The track's Synth as a plain sine voice: one oscillator, no unison,
    // drift, modulation or effects, open filter, short release.
    let synth = doc.project().track(inst).unwrap().instrument_plugin.as_ref().unwrap().id;
    let plain: std::collections::BTreeMap<String, f32> = [
        ("osc1_wave", 0.0),
        ("osc1_shape", 0.0),
        ("osc2_level", 0.0),
        ("sub_level", 0.0),
        ("unison_voices", 0.0),
        ("drift", 0.0),
        ("cutoff", 20_000.0),
        ("resonance", 0.0),
        ("filter_drive", 0.0),
        ("filter_env", 0.0),
        ("amp_release", 0.1),
        ("mod1_on", 0.0),
        ("mod2_on", 0.0),
        ("mod3_on", 0.0),
        ("fx1_on", 0.0),
        ("fx2_on", 0.0),
        ("fx3_on", 0.0),
        ("fx4_on", 0.0),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v))
    .collect();
    doc.execute(Command::SetDeviceParams { device: synth, params: plain, preset: None }, Origin::User).unwrap();
    doc.execute(Command::AddMidiClip { track: inst, clip: MidiClip::empty(0, 4 * PPQ) }, Origin::User).unwrap();
    let clip = doc.project().track(inst).unwrap().midi_clips[0].id;
    // A4 from beat 1 to beat 3: frames [24000, 72000).
    let a4 = Note { id: 1, pitch: 69, start: PPQ, length: 2 * PPQ, velocity: 110 };
    doc.execute(Command::SetMidiNotes { clip, notes: vec![a4] }, Origin::User).unwrap();

    let snap = publish(&engine, &doc);
    let ts = snap.tracks.iter().find(|t| t.track_id == inst.0).unwrap();
    assert!(ts.instrument);
    assert_eq!(ts.notes.len(), 1);
    assert_eq!((ts.notes[0].start, ts.notes[0].end, ts.notes[0].pitch), (24_000, 72_000, 69));

    engine.play().unwrap();
    let out = render(&engine, 96_000);
    let l = left(&out);
    // Centre pan: identical channels.
    assert!(out.chunks_exact(2).all(|f| f[0] == f[1]));
    // Silence before the note, sound from its first frames (a sine starts
    // at zero phase).
    assert!(l[..24_000].iter().all(|&v| v == 0.0), "sound before the note-on");
    assert!(l[24_000..24_004].iter().any(|&v| v != 0.0), "the note must start at its frame");
    let during = rms(&l[30_000..70_000]);
    assert!(during > 0.01, "rms during note {during}");
    // Right pitch (A4 = 440 Hz).
    let hz = estimate_hz(&l[34_000..60_000], 440.0, 10.0);
    assert!((hz - 440.0).abs() <= 0.5, "estimated {hz} Hz");
    // Release tail after the note-off, then silence (100 ms release to -80 dB).
    assert!(rms(&l[72_000..73_000]) > 0.2 * during);
    assert!(l[72_000 + 19_200..].iter().all(|&v| v == 0.0));

    // Undo the note edit: the clip is empty again, playback is silent.
    assert!(doc.undo().unwrap());
    assert!(doc.project().midi_clip(clip).unwrap().notes.is_empty());
    let snap = publish(&engine, &doc);
    assert!(snap.tracks.iter().all(|t| t.notes.is_empty()));
    engine.seek(0).unwrap();
    let out = render(&engine, 96_000);
    assert!(out.iter().all(|&v| v == 0.0), "undone notes must not sound");

    // Redo brings it back, identical to the first render.
    assert!(doc.redo().unwrap());
    publish(&engine, &doc);
    engine.seek(0).unwrap();
    let again = render(&engine, 96_000);
    assert_eq!(again, out_first(&engine, &doc));
}

/// Fresh engine render of the document from frame 0 (reference).
fn out_first(_: &Engine, doc: &Document) -> Vec<f32> {
    let e = Engine::new(Config { sample_rate: RATE, max_block_size: 1024 }).unwrap();
    publish(&e, doc);
    e.play().unwrap();
    render(&e, 96_000)
}

#[test]
fn audio_tracks_ignore_notes_and_v2_keeps_audio_working() {
    // Notes only sound on instrument tracks: a snapshot that (incorrectly)
    // carries notes on an audio track renders silence.
    let engine = Engine::new(Config { sample_rate: RATE, max_block_size: 256 }).unwrap();
    let snap = snapshot::SessionSnapshot {
        tracks: vec![snapshot::TrackSnapshot {
            track_id: 1,
            is_return: false,
            output: None,
            sends: Vec::new(),
            gain: 1.0,
            pan: 0.0,
            trim: 1.0,
            width: 1.0,
            mono: false,
            mute: false,
            solo: false,
            clips: vec![],
            instrument: false,
            notes: vec![snapshot::NoteSnapshot { start: 0, end: 10_000, pitch: 60, velocity: 100 }],
            synth: None,
            devices: vec![],
            instrument_plugin: None,
            automation: vec![],
            arm: false,
            monitor: false,
            input: None,
        }],
        master_gain: 1.0,
        master_automation: vec![],
        device_assets: Default::default(),
        tempo: vec![],
        click_quarters: 1.0,
    };
    engine.publish(&snap).unwrap();
    engine.play().unwrap();
    assert!(render(&engine, 12_000).iter().all(|&v| v == 0.0));
    // Same track flagged as instrument: sound.
    let mut snap = snap;
    snap.tracks[0].instrument = true;
    engine.publish(&snap).unwrap();
    engine.seek(0).unwrap();
    assert!(rms(&render(&engine, 12_000)) > 0.01);
    // Invalid velocity is rejected by the engine.
    snap.tracks[0].notes[0].velocity = 0;
    assert_eq!(engine.publish(&snap).unwrap_err().code, dstudio_engine::ffi::SE_ERR_INVALID_ARG);
}
