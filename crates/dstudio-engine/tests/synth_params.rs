//! End-to-end Synth (synth-v2) parameters: `Command::SetDeviceParam(s)` on
//! an instrument track's built-in Synth → snapshot → engine → offline
//! render.

use dstudio_core::builtin::synth_v2::SYNTH_V2;
use dstudio_core::*;
use dstudio_engine::{Config, Engine};
use std::collections::{BTreeMap, HashMap};

const RATE: f64 = 48_000.0;

fn render(e: &Engine, frames: usize) -> Vec<f32> {
    let mut out = vec![0f32; frames * 2];
    for chunk in out.chunks_mut(2 * 512) {
        e.render_offline(chunk).unwrap();
    }
    out
}

fn left(inter: &[f32]) -> Vec<f32> {
    inter.iter().step_by(2).copied().collect()
}

/// High-frequency content: RMS of the second difference (a 12 dB/oct
/// high-pass) relative to the signal RMS.
fn brightness(x: &[f32]) -> f64 {
    let (mut d, mut a) = (0.0f64, 0.0f64);
    for i in 2..x.len() {
        let d2 = x[i] as f64 - 2.0 * x[i - 1] as f64 + x[i - 2] as f64;
        d += d2 * d2;
        a += x[i] as f64 * x[i] as f64;
    }
    (d / a).sqrt()
}

fn rms(x: &[f32]) -> f64 {
    (x.iter().map(|&v| (v as f64) * (v as f64)).sum::<f64>() / x.len() as f64).sqrt()
}

/// Default project's instrument track with one C4 note from beat 0 to beat 2
/// (frames [0, 48000) at 120 BPM).
fn doc_with_c4() -> (Document, TrackId) {
    let mut doc = Document::new(Project::default());
    let inst = doc.project().tracks.iter().find(|t| t.kind == TrackKind::Instrument).unwrap().id;
    doc.execute(Command::AddMidiClip { track: inst, clip: MidiClip::empty(0, 4 * PPQ) }, Origin::User).unwrap();
    let clip = doc.project().track(inst).unwrap().midi_clips[0].id;
    let c4 = Note { id: 1, pitch: 60, start: 0, length: 2 * PPQ, velocity: 100 };
    doc.execute(Command::SetMidiNotes { clip, notes: vec![c4] }, Origin::User).unwrap();
    (doc, inst)
}

/// Fresh engine, publish the document, play and render the instrument track
/// (left channel; the default project's tracks are centred).
fn render_doc(doc: &Document, frames: usize) -> Vec<f32> {
    let engine = Engine::new(Config { sample_rate: RATE, max_block_size: 1024 }).unwrap();
    let (snap, skipped) = snapshot::build(doc.project(), engine.sample_rate(), &HashMap::new());
    assert!(skipped.is_empty());
    engine.publish(&snap).unwrap();
    engine.play().unwrap();
    left(&render(&engine, frames))
}

/// The track's Synth device.
fn synth(doc: &Document, inst: TrackId) -> DeviceId {
    doc.project().track(inst).unwrap().instrument_plugin.as_ref().unwrap().id
}

#[test]
fn low_cutoff_via_command_darkens_the_sound() {
    let (mut doc, inst) = doc_with_c4();
    let default_out = render_doc(&doc, 36_000);
    let device = synth(&doc, inst);
    doc.execute(Command::SetDeviceParam { device, param: "cutoff".into(), value: 150.0 }, Origin::User).unwrap();
    let dark = render_doc(&doc, 36_000);

    let (b_default, b_dark) = (brightness(&default_out[4800..36_000]), brightness(&dark[4800..36_000]));
    eprintln!("brightness default {b_default:.5}, cutoff 150 Hz {b_dark:.5}");
    assert!(rms(&dark[4800..36_000]) > 0.002, "still audible");
    assert!(b_dark < 0.25 * b_default, "high-frequency energy must drop: {b_dark} vs {b_default}");

    // Undo restores the default sound exactly.
    assert!(doc.undo().unwrap());
    assert_eq!(render_doc(&doc, 36_000), default_out);
}

#[test]
fn every_factory_preset_renders_a_sane_c4() {
    let mut bad = Vec::new();
    for preset in SYNTH_V2.factory {
        let (mut doc, inst) = doc_with_c4();
        let params: BTreeMap<String, f32> = preset.values.iter().map(|(k, v)| (k.to_string(), *v)).collect();
        let device = synth(&doc, inst);
        doc.execute(Command::SetDeviceParams { device, params, preset: Some(preset.name.into()) }, Origin::User).unwrap();
        // Note [0, 1 s) plus 3 s of release and effect tails.
        let out = render_doc(&doc, 4 * 48_000);
        let peak = out.iter().fold(0.0f32, |m, &v| m.max(v.abs()));
        let body = rms(&out[2_400..48_000]);
        let name = preset.name;
        eprintln!("{name:>16}: peak {:.3} ({:.1} dBFS), rms during note {body:.4}", peak, 20.0 * peak.log10());
        if !out.iter().all(|v| v.is_finite()) {
            bad.push(format!("{name}: non-finite output"));
        } else if body <= 1e-3 {
            bad.push(format!("{name}: silent (rms {body})"));
        } else if peak >= 1.0 {
            bad.push(format!("{name}: peak {peak} reaches 0 dBFS"));
        }
    }
    // Every problem at once: with a hundred sounds, one at a time is slow to fix.
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}
