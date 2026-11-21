//! Clip menu renders (Consolidate, Bounce, Export) end to end: `render_plan::render_project` →
//! `dstudio_engine::offline::render` (a private engine on the calling thread, as src/clip_render.rs
//! runs it on its worker). `bench_four_bar_clip_render` (ignored; `cargo test --release -p
//! dstudio-engine --test clip_render -- --ignored --nocapture`) prints how long a 4-bar clip takes,
//! engine start and file loading included.

use dstudio_core::render_plan::{self, RenderMode};
use dstudio_core::*;
use dstudio_engine::offline::{self, OfflineError, OfflineRender};
use dstudio_engine::{Config, Engine};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Instant;

const RATE: f64 = 48_000.0;

/// 16-bit mono WAV of a sine.
fn write_sine(path: &std::path::Path, secs: f64, hz: f64, amp: f64) {
    let n = (RATE * secs) as usize;
    let mut b = Vec::new();
    b.extend_from_slice(b"RIFF");
    b.extend_from_slice(&(36 + 2 * n as u32).to_le_bytes());
    b.extend_from_slice(b"WAVEfmt ");
    for v in [16u32, 0x0001_0001, RATE as u32, RATE as u32 * 2, 0x0010_0002] {
        b.extend_from_slice(&v.to_le_bytes());
    }
    b.extend_from_slice(b"data");
    b.extend_from_slice(&(2 * n as u32).to_le_bytes());
    for i in 0..n {
        let v = amp * (std::f64::consts::TAU * hz * i as f64 / RATE).sin();
        b.extend_from_slice(&((v * 32767.0) as i16).to_le_bytes());
    }
    std::fs::write(path, b).unwrap();
}

struct Rig {
    doc: Document,
    wav: std::path::PathBuf,
    _dir: tempfile::TempDir,
}

/// Default project (audio track 0, instrument track 1) with a sine asset.
fn rig(secs: f64) -> Rig {
    let dir = tempfile::tempdir().unwrap();
    let wav = dir.path().join("s.wav");
    write_sine(&wav, secs, 440.0, 0.3);
    let mut doc = Document::new(Project::default());
    let asset = Asset {
        id: AssetId(0),
        relative_path: None,
        absolute_path: wav.to_string_lossy().into(),
        sample_rate: RATE,
        channels: 1,
        frames: (RATE * secs) as u64,
    };
    doc.execute(Command::AddAsset { asset }, Origin::User).unwrap();
    Rig { doc, wav, _dir: dir }
}

impl Rig {
    fn job(&self, span: &render_plan::ClipSpan, mode: RenderMode, tail_s: f64) -> OfflineRender {
        let p = self.doc.project();
        let start = p.ticks_to_frames(span.start, RATE);
        let body = (p.ticks_to_frames(span.end, RATE) - start) as usize + (tail_s * RATE) as usize;
        OfflineRender { project: render_plan::render_project(p, span, mode), rate: RATE, transients: HashMap::new(), start, frames: body }
    }

    fn render(&self, span: &render_plan::ClipSpan, mode: RenderMode, tail_s: f64) -> Vec<f32> {
        offline::render(self.job(span, mode, tail_s), &AtomicBool::new(false), &AtomicUsize::new(0)).unwrap()
    }
}

fn peak(x: &[f32]) -> f32 {
    x.iter().fold(0.0, |m, v| m.max(v.abs()))
}

fn audio_clip(asset: AssetId, id: u64, start_beat: i64, secs: f64) -> Clip {
    Clip {
        id: ClipId(id),
        asset,
        name: format!("c{id}"),
        start: start_beat * PPQ,
        source_offset: 0,
        source_length: (RATE * secs) as u64,
        gain: 1.0,
        warp: Default::default(),
        disabled: false,
        clip_loop: None,
        fades: Default::default(),
        takes: Vec::new(),
    }
}

fn midi_clip(start_beat: i64, beats: i64, pitch: u8) -> MidiClip {
    let mut c = MidiClip::empty(start_beat * PPQ, beats * PPQ);
    c.notes = (0..beats * 4)
        .map(|i| Note { id: i as u32 + 1, pitch: pitch + ((i * 5) % 24) as u8, start: i * PPQ / 4, length: PPQ / 2, velocity: 100 })
        .collect();
    c
}

#[test]
fn consolidate_render_is_unity_level_and_plays_only_the_group() {
    let mut r = rig(2.0);
    let (a, asset) = (r.doc.project().tracks[0].id, r.doc.project().assets[0].id);
    // Fader and pan must not leak into the material: the track is at -6 dB, panned left.
    r.doc.execute(Command::SetTrackGain { track: a, gain: 0.5 }, Origin::User).unwrap();
    r.doc.execute(Command::SetTrackPan { track: a, pan: -0.7 }, Origin::User).unwrap();
    for (id, beat) in [(101, 2), (102, 12)] {
        r.doc.execute(Command::AddClip { track: a, clip: audio_clip(asset, id, beat, 2.0) }, Origin::User).unwrap();
    }
    let span = render_plan::consolidate_group(r.doc.project(), ClipId(101)).unwrap();
    assert_eq!(span.clips, vec![ClipId(101)], "the clip at beat 12 is apart");
    let out = r.render(&span, RenderMode::Material, 0.0);
    assert_eq!(out.len() / 2, 96_000);
    let (l, rt): (Vec<f32>, Vec<f32>) = (out.iter().step_by(2).copied().collect(), out.iter().skip(1).step_by(2).copied().collect());
    assert!((peak(&l) - 0.3).abs() < 0.01 && (peak(&rt) - 0.3).abs() < 0.01, "source level on both channels: {} {}", peak(&l), peak(&rt));
    let stem = r.render(&span, RenderMode::Stem, 0.0);
    assert!(peak(&stem) < 0.2 && peak(&stem) > 0.01, "a stem keeps fader and pan: {}", peak(&stem));
}

#[test]
fn bounce_of_a_midi_clip_plays_that_clip_only_and_rings_into_its_tail() {
    let mut r = rig(1.0);
    let inst = r.doc.project().tracks[1].id;
    let mk = |start_beat: i64, pitch: u8| {
        let mut c = MidiClip::empty(start_beat * PPQ, PPQ);
        c.notes = vec![Note { id: 1, pitch, start: 0, length: PPQ, velocity: 110 }];
        c
    };
    r.doc.execute(Command::AddMidiClip { track: inst, clip: mk(0, 60) }, Origin::User).unwrap();
    r.doc.execute(Command::AddMidiClip { track: inst, clip: mk(8, 72) }, Origin::User).unwrap();
    let id = r.doc.project().tracks[1].midi_clips[0].id;
    let span = render_plan::clip_span(r.doc.project(), id).unwrap();
    assert!(render_plan::rings_out(r.doc.project(), inst, RenderMode::Bounce));
    let out = r.render(&span, RenderMode::Bounce, 6.0);
    let (body, beat8) = (24_000usize, 8 * 24_000usize);
    assert_eq!(out.len() / 2, body + 6 * 48_000);
    assert!(peak(&out[..body * 2]) > 0.02, "the clip sounds");
    assert!(peak(&out[body * 2..2 * (body + 24_000)]) > 0.0, "the release rings past the clip end");
    assert!(peak(&out[2 * beat8..2 * (beat8 + 24_000)]) < 1e-3, "nothing of the clip at beat 8 leaks in");
    let other = render_plan::clip_span(r.doc.project(), r.doc.project().tracks[1].midi_clips[1].id).unwrap();
    assert!(peak(&r.render(&other, RenderMode::Bounce, 0.0)[..48_000]) > 0.02, "the clip at beat 8 does sound when it is the one rendered");
}

#[test]
fn tempo_synced_devices_follow_the_song_tempo_in_quarters_in_compound_meter() {
    // 6/8 at 90 BPM: a quarter is 32000 frames, so a 1/16 gate step is 8000 (not 4000 from the
    // eighth-note click, nor 6000 from a 120 BPM default).
    let mut r = rig(2.0);
    let (a, asset) = (r.doc.project().tracks[0].id, r.doc.project().assets[0].id);
    r.doc.execute(Command::SetTempo { bpm: 90.0, at: None }, Origin::User).unwrap();
    r.doc.execute(Command::SetTimeSignature { numerator: 6, denominator: 8 }, Origin::User).unwrap();
    let mut gate = dstudio_core::builtin::find("trance-gate").unwrap().instantiate();
    for (k, v) in [("p1_steps", 2.0), ("p1_lo", 1.0), ("release", 1.0)] {
        gate.params.insert(k.into(), v); // pattern "x.", 1 ms release
    }
    r.doc.execute(Command::AddDevice { track: a, index: None, device: gate }, Origin::User).unwrap();
    r.doc.execute(Command::AddClip { track: a, clip: audio_clip(asset, 101, 0, 2.0) }, Origin::User).unwrap();
    let span = render_plan::consolidate_group(r.doc.project(), ClipId(101)).unwrap();
    let out = r.render(&span, RenderMode::Bounce, 0.0);
    let l: Vec<f32> = out.iter().step_by(2).copied().collect();
    let step = 8000;
    assert!(peak(&l[200..step - 200]) > 0.1, "open in step 0");
    assert!(peak(&l[step + 200..2 * step]) < 1e-4, "closed in step 1: {}", peak(&l[step + 200..2 * step]));
    assert!(peak(&l[2 * step + 200..3 * step - 200]) > 0.1, "open again in step 2");
}

#[test]
fn a_finished_render_reports_every_frame_done() {
    let mut r = rig(2.0);
    let (a, asset) = (r.doc.project().tracks[0].id, r.doc.project().assets[0].id);
    r.doc.execute(Command::AddClip { track: a, clip: audio_clip(asset, 101, 0, 2.0) }, Origin::User).unwrap();
    let span = render_plan::clip_span(r.doc.project(), ClipId(101)).unwrap();
    let job = r.job(&span, RenderMode::Material, 0.0);
    let done = AtomicUsize::new(0);
    let frames = job.frames;
    let out = offline::render(job, &AtomicBool::new(false), &done).unwrap();
    assert_eq!((out.len() / 2, done.load(Ordering::Relaxed)), (frames, frames));
}

#[test]
fn a_cancelled_render_returns_cancelled_and_stops_early() {
    let mut r = rig(2.0);
    let inst = r.doc.project().tracks[1].id;
    let reverb = dstudio_core::builtin::find("reverb").unwrap().instantiate();
    r.doc.execute(Command::AddDevice { track: inst, index: None, device: reverb }, Origin::User).unwrap();
    r.doc.execute(Command::AddMidiClip { track: inst, clip: midi_clip(0, 16, 48) }, Origin::User).unwrap();
    let id = r.doc.project().tracks[1].midi_clips[0].id;
    let span = render_plan::clip_span(r.doc.project(), id).unwrap();
    // Not even started: the cancel flag is seen before an engine is made.
    let frames = r.job(&span, RenderMode::Bounce, 2.0).frames;
    let done = AtomicUsize::new(0);
    assert_eq!(offline::render(r.job(&span, RenderMode::Bounce, 2.0), &AtomicBool::new(true), &done), Err(OfflineError::Cancelled));
    assert_eq!(done.load(Ordering::Relaxed), 0);
    // Cancelled once a tenth of the audio is rendered.
    let (cancel, done, finished) = (AtomicBool::new(false), AtomicUsize::new(0), AtomicBool::new(false));
    let result = std::thread::scope(|s| {
        s.spawn(|| {
            while done.load(Ordering::Relaxed) < frames / 10 && !finished.load(Ordering::Relaxed) {
                std::thread::yield_now();
            }
            cancel.store(true, Ordering::Relaxed);
        });
        let r = offline::render(r.job(&span, RenderMode::Bounce, 2.0), &cancel, &done);
        finished.store(true, Ordering::Relaxed);
        r
    });
    assert_eq!(result, Err(OfflineError::Cancelled));
    assert!(done.load(Ordering::Relaxed) < frames, "stopped before the end: {} of {}", done.load(Ordering::Relaxed), frames);
}

#[test]
fn a_missing_audio_file_fails_the_render_and_names_the_file() {
    let mut r = rig(1.0);
    let (a, asset) = (r.doc.project().tracks[0].id, r.doc.project().assets[0].id);
    r.doc.execute(Command::AddClip { track: a, clip: audio_clip(asset, 101, 0, 1.0) }, Origin::User).unwrap();
    let span = render_plan::clip_span(r.doc.project(), ClipId(101)).unwrap();
    let job = r.job(&span, RenderMode::Material, 0.0);
    std::fs::remove_file(&r.wav).unwrap();
    match offline::render(job, &AtomicBool::new(false), &AtomicUsize::new(0)) {
        Err(OfflineError::Failed(msg)) => assert!(msg.contains("s.wav"), "{msg}"),
        other => panic!("expected a failure, got {:?}", other.map(|v| v.len())),
    }
}

fn test_plugin(file: &str, id: &str, name: &str) -> PluginRef {
    PluginRef {
        format: PluginFormat::Clap,
        id: id.into(),
        path: Some(dstudio_engine::test_plugin_dir().join(file).to_string_lossy().into_owned()),
        name: name.into(),
        vendor: "DStudio".into(),
        version: "1.0.0".into(),
        kind: PluginKind::Effect,
    }
}

fn gain_plugin(gain: Option<f32>) -> Device {
    let mut d = Device::new(test_plugin("DStudioTestGain.clap", "studio.dstudio.test.gain", "DStudio Test Gain"));
    d.state = gain.map(|g| device::b64::encode(&g.to_le_bytes()));
    d
}

/// The rig with a 1 s sine clip on the audio track (with `device` on it), and the span of that clip.
fn audio_rig(device: Option<Device>) -> (Rig, render_plan::ClipSpan) {
    let mut r = rig(1.0);
    let (a, asset) = (r.doc.project().tracks[0].id, r.doc.project().assets[0].id);
    r.doc.execute(Command::AddClip { track: a, clip: audio_clip(asset, 101, 0, 1.0) }, Origin::User).unwrap();
    if let Some(device) = device {
        r.doc.execute(Command::AddDevice { track: a, index: None, device }, Origin::User).unwrap();
    }
    let span = render_plan::clip_span(r.doc.project(), ClipId(101)).unwrap();
    (r, span)
}

#[test]
fn a_plugin_chain_is_rendered_from_its_saved_state_and_equals_the_same_render_on_a_worker() {
    let (r, span) = audio_rig(Some(gain_plugin(Some(0.25))));
    // All on this thread (the engine tests have no "main thread" of their own to keep).
    let direct = r.render(&span, RenderMode::Bounce, 0.0);
    assert!((peak(&direct) - 0.3 * 0.25).abs() < 0.005, "the plugin's gain is in the file: {}", peak(&direct));
    // Prepared here (as the app does on the UI thread), rendered on a worker, dropped here.
    let mut session = offline::Session::new(r.job(&span, RenderMode::Bounce, 0.0)).unwrap();
    assert_eq!(session.plugins(), (0, 1));
    assert_eq!(
        session.render(&AtomicBool::new(false), &AtomicUsize::new(0)),
        Err(OfflineError::Failed("the plugins are not ready".into()))
    );
    assert_eq!(session.prepare_next(None).unwrap().as_deref(), Some("DStudio Test Gain"));
    assert_eq!(session.prepare_next(None).unwrap(), None);
    let done = AtomicUsize::new(0);
    let worker = std::thread::scope(|s| s.spawn(|| session.render(&AtomicBool::new(false), &done)).join().unwrap()).unwrap();
    assert_eq!(worker, direct, "a worker render of the prepared plugins is the same file");
    assert_eq!(done.load(Ordering::Relaxed), worker.len() / 2);
    drop(session);
}

#[test]
fn the_plugin_starts_from_the_live_instances_current_state_not_the_saved_one() {
    let (r, span) = audio_rig(Some(gain_plugin(Some(0.25))));
    let device = r.doc.project().tracks[0].devices[0].clone();
    let live = Engine::new(Config { sample_rate: RATE, max_block_size: 512 }).unwrap();
    let info = live.load_file(&r.wav).unwrap();
    let map = HashMap::from([(r.doc.project().assets[0].id, info.asset_id)]);
    live.publish(&snapshot::build(r.doc.project(), RATE, &map).0).unwrap();
    live.plugin_set_param(device.id.0, 0, 0.8).unwrap(); // edited in the plugin since the project's state was saved
    live.seek(0).unwrap();
    live.play().unwrap();
    live.render_offline(&mut [0.0; 1024]).unwrap();
    assert!(live.plugin_state_of(DeviceId(9999)).unwrap().is_none(), "no live instance, no state");
    let mut session = offline::Session::new(r.job(&span, RenderMode::Bounce, 0.0)).unwrap();
    session.prepare_next(Some(&live)).unwrap();
    let out = session.render(&AtomicBool::new(false), &AtomicUsize::new(0)).unwrap();
    assert!((peak(&out) - 0.3 * 0.8).abs() < 0.005, "the live 0.8, not the saved 0.25: {}", peak(&out));
    drop(session);
}

#[test]
fn a_latency_plugin_is_rendered_aligned_to_the_timeline() {
    let n: u32 = 1000;
    let mut latency = Device::new(test_plugin("DStudioTestLatency.clap", "studio.dstudio.test.latency", "DStudio Test Latency"));
    latency.state = Some(device::b64::encode(&n.to_le_bytes()));
    let (with, span) = audio_rig(Some(latency));
    let (without, _) = audio_rig(None);
    let delayed = with.render(&span, RenderMode::Bounce, 0.0);
    let dry = without.render(&span, RenderMode::Bounce, 0.0);
    assert_eq!(delayed.len(), dry.len());
    let worst = delayed.iter().zip(&dry).fold(0.0f32, |m, (a, b)| m.max((a - b).abs()));
    assert!(
        peak(&dry) > 0.2 && worst < 1e-5,
        "the plugin's {n} frames are compensated: the file equals the dry render, worst difference {worst}"
    );
}

#[test]
fn an_export_of_the_live_session_through_a_latent_device_lands_on_the_timeline() {
    // The export dialog renders the app's own engine with `render_aligned`: a clip whose first
    // sample is an impulse must come out at the clip's frame, not the Limiter's latency later.
    let dir = tempfile::tempdir().unwrap();
    let wav = dir.path().join("impulse.wav");
    write_sine(&wav, 0.5, 0.0, 0.0);
    let mut b = std::fs::read(&wav).unwrap();
    b[44..46].copy_from_slice(&16_384i16.to_le_bytes());
    std::fs::write(&wav, b).unwrap();
    let mut doc = Document::new(Project::default());
    let asset = Asset {
        id: AssetId(0),
        relative_path: None,
        absolute_path: wav.to_string_lossy().into(),
        sample_rate: RATE,
        channels: 1,
        frames: (RATE * 0.5) as u64,
    };
    doc.execute(Command::AddAsset { asset }, Origin::User).unwrap();
    let (a, asset) = (doc.project().tracks[0].id, doc.project().assets[0].id);
    doc.execute(Command::AddClip { track: a, clip: audio_clip(asset, 101, 1, 0.5) }, Origin::User).unwrap();
    let limiter = dstudio_core::builtin::find("limiter").unwrap().instantiate();
    doc.execute(Command::AddDevice { track: a, index: None, device: limiter }, Origin::User).unwrap();
    let engine = Engine::new(Config { sample_rate: RATE, max_block_size: 512 }).unwrap();
    let info = engine.load_file(&wav).unwrap();
    engine.publish(&snapshot::build(doc.project(), RATE, &HashMap::from([(asset, info.asset_id)])).0).unwrap();
    let lead = engine.latency_report().total_frames as usize;
    assert!(lead > 0, "the Limiter reports its lookahead");
    let at = doc.project().ticks_to_frames(PPQ, RATE) as usize;
    let loudest = |x: &[f32]| x.iter().step_by(2).enumerate().fold((0, 0.0f32), |m, (i, v)| if v.abs() > m.1 { (i, v.abs()) } else { m }).0;
    let frames = 2 * at;
    let out = offline::render_aligned(&engine, 0, frames, |_| Ok(())).unwrap();
    assert_eq!(out.len(), frames * 2, "the whole range, the end not cut by the latency");
    assert_eq!(loudest(&out), at, "the impulse is at the clip's frame");
    // Rendering exactly `frames` from the start, as the export did, puts it `lead` frames late.
    engine.stop().unwrap();
    engine.seek(0).unwrap();
    engine.play().unwrap();
    let mut naive = vec![0f32; frames * 2];
    for chunk in naive.chunks_mut(offline::CHUNK * 2) {
        engine.render_offline(chunk).unwrap();
    }
    assert_eq!(loudest(&naive), at + lead);
}

#[test]
fn a_missing_file_on_another_track_does_not_fail_a_clip_render() {
    let (mut r, span) = audio_rig(None);
    let missing = Asset {
        id: AssetId(0),
        relative_path: None,
        absolute_path: "/nonexistent/elsewhere.wav".into(),
        sample_rate: RATE,
        channels: 1,
        frames: RATE as u64,
    };
    r.doc.execute(Command::AddAsset { asset: missing }, Origin::User).unwrap();
    let other = r.doc.project().assets[1].id;
    let color = r.doc.project().tracks[0].color;
    let track = Track::new(TrackId(0), TrackKind::Audio, "Other", color);
    r.doc.execute(Command::AddTrack { index: Some(1), track }, Origin::User).unwrap();
    let ot = r.doc.project().tracks.iter().find(|t| t.name == "Other").unwrap().id;
    r.doc.execute(Command::AddClip { track: ot, clip: audio_clip(other, 201, 0, 1.0) }, Origin::User).unwrap();
    for mode in [RenderMode::Material, RenderMode::Bounce, RenderMode::Stem] {
        let out = offline::render(r.job(&span, mode, 0.0), &AtomicBool::new(false), &AtomicUsize::new(0));
        assert!(out.as_ref().is_ok_and(|o| peak(o) > 0.2), "{mode:?}: {:?}", out.map(|o| peak(&o)));
    }
}

#[test]
fn a_plugin_that_cannot_be_loaded_fails_the_render_by_name_before_any_audio() {
    let missing = Device::new(PluginRef {
        path: Some("/nonexistent/Missing.clap".into()),
        ..test_plugin("DStudioTestGain.clap", "studio.dstudio.test.gain", "Missing Verb")
    });
    let (r, span) = audio_rig(Some(missing));
    let done = AtomicUsize::new(0);
    match offline::render(r.job(&span, RenderMode::Bounce, 0.0), &AtomicBool::new(false), &done) {
        Err(OfflineError::Failed(msg)) => assert!(msg.starts_with("Missing Verb could not be loaded"), "{msg}"),
        other => panic!("expected a load failure, got {:?}", other.map(|v| v.len())),
    }
    assert_eq!(done.load(Ordering::Relaxed), 0);
}

#[test]
fn an_instrument_plugin_is_rendered_and_a_plugin_on_another_track_is_left_out() {
    let mut r = rig(1.0);
    let (a, inst) = (r.doc.project().tracks[0].id, r.doc.project().tracks[1].id);
    r.doc.execute(Command::AddDevice { track: a, index: None, device: gain_plugin(None) }, Origin::User).unwrap();
    let sine = Device::new(PluginRef {
        kind: PluginKind::Instrument,
        ..test_plugin("DStudioTestSine.clap", "studio.dstudio.test.sine", "DStudio Test Sine")
    });
    r.doc.execute(Command::SetInstrumentPlugin { track: inst, device: Some(sine) }, Origin::User).unwrap();
    r.doc.execute(Command::AddMidiClip { track: inst, clip: midi_clip(0, 1, 60) }, Origin::User).unwrap();
    let id = r.doc.project().tracks[1].midi_clips[0].id;
    let span = render_plan::clip_span(r.doc.project(), id).unwrap();
    let session = offline::Session::new(r.job(&span, RenderMode::Bounce, 0.0)).unwrap();
    assert_eq!(
        session.plugins(),
        (0, 1),
        "only the instrument plugin of the rendered track: the effect on the other track is muted and left out"
    );
    drop(session);
    assert!(peak(&r.render(&span, RenderMode::Bounce, 0.0)) > 0.02, "the plugin instrument sounds");
}

#[test]
#[ignore]
fn bench_four_bar_clip_render() {
    let mut r = rig(8.0);
    let (a, inst, asset) = (r.doc.project().tracks[0].id, r.doc.project().tracks[1].id, r.doc.project().assets[0].id);
    let reverb = dstudio_core::builtin::find("reverb").unwrap().instantiate();
    let delay = dstudio_core::builtin::find("delay").map(|d| d.instantiate());
    r.doc.execute(Command::AddDevice { track: a, index: None, device: reverb.clone() }, Origin::User).unwrap();
    r.doc.execute(Command::AddDevice { track: inst, index: None, device: reverb }, Origin::User).unwrap();
    if let Some(d) = delay {
        r.doc.execute(Command::AddDevice { track: inst, index: None, device: d }, Origin::User).unwrap();
    }
    r.doc.execute(Command::AddClip { track: a, clip: audio_clip(asset, 101, 0, 8.0) }, Origin::User).unwrap();
    r.doc.execute(Command::AddMidiClip { track: inst, clip: midi_clip(0, 16, 48) }, Origin::User).unwrap();
    let midi = r.doc.project().tracks[1].midi_clips[0].id;
    let cases = [
        ("audio 4 bars, Material (consolidate)", ClipId(101), RenderMode::Material, 0.0),
        ("audio 4 bars + reverb, Bounce", ClipId(101), RenderMode::Bounce, 2.0),
        ("MIDI 4 bars, synth + reverb + delay, Bounce", midi, RenderMode::Bounce, 2.0),
        ("MIDI 4 bars, Stem (export)", midi, RenderMode::Stem, 2.0),
    ];
    for (name, id, mode, tail) in cases {
        let span = render_plan::clip_span(r.doc.project(), id).unwrap();
        let mut ms: Vec<f64> = (0..9)
            .map(|_| {
                let t = Instant::now();
                let out = r.render(&span, mode, tail);
                assert!(peak(&out) > 0.0);
                t.elapsed().as_secs_f64() * 1e3
            })
            .collect();
        ms.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let audio = 8.0 + tail;
        println!(
            "BENCH {name}: median {:.1} ms, min {:.1} ms for {audio:.0} s of audio ({:.0}x real time)",
            ms[4],
            ms[0],
            audio * 1e3 / ms[4]
        );
    }
    // The same clip with a CLAP plugin added to the chain: the main thread's share (prepare + teardown) next to the worker's.
    r.doc.execute(Command::AddDevice { track: a, index: None, device: gain_plugin(Some(0.5)) }, Origin::User).unwrap();
    let span = render_plan::clip_span(r.doc.project(), ClipId(101)).unwrap();
    let mut rows: Vec<(f64, f64, f64)> = (0..9)
        .map(|_| {
            let mut session = offline::Session::new(r.job(&span, RenderMode::Bounce, 2.0)).unwrap();
            let t = Instant::now();
            while session.prepare_next(None).unwrap().is_some() {}
            let prepare = t.elapsed().as_secs_f64() * 1e3;
            let t = Instant::now();
            let out = session.render(&AtomicBool::new(false), &AtomicUsize::new(0)).unwrap();
            let worker = t.elapsed().as_secs_f64() * 1e3;
            assert!(peak(&out) > 0.0);
            let t = Instant::now();
            drop(session);
            (prepare, worker, t.elapsed().as_secs_f64() * 1e3)
        })
        .collect();
    rows.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
    println!(
        "BENCH audio 4 bars + reverb + CLAP gain, Bounce, plugin path: main thread prepare {:.2} ms, worker {:.1} ms, main thread teardown {:.2} ms (medians of 9)",
        rows[4].0, rows[4].1, rows[4].2
    );
}
