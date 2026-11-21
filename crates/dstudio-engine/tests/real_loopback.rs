//! Manual acoustic loopback check of recording latency compensation.
//!
//! Plays metronome clicks through the default output, records them with the
//! default input on an armed (unmonitored) track, and measures where each
//! click lands in the take relative to the beat grid. With correct
//! compensation the offset is ~0 (plus a few hundred microseconds of
//! speaker→mic travel and transducer delay).
//!
//! `acoustic_loopback_latency_with_pdc` repeats it with plugin delay
//! compensation active: a second track carries DStudioTestLatency (N = 4800
//! frames, 100 ms), so the metronome is delayed by L = 4800 and takes are
//! placed L earlier; the clicks must still land on the beat.
//!
//! Opens the REAL microphone (macOS permission prompt) and makes sound, so it
//! is #[ignore]d and additionally gated on DSTUDIO_TEST_REAL_INPUT=1:
//!   DSTUDIO_TEST_REAL_INPUT=1 cargo test -p dstudio-engine --test real_loopback -- --ignored --nocapture

use dstudio_core::snapshot::{SessionSnapshot, TrackSnapshot};
use dstudio_core::{Device, DeviceId, InputSource, PluginFormat, PluginKind, PluginRef};
use dstudio_engine::{Config, DeviceConfig, Engine, MetronomeConfig};

#[test]
#[ignore]
fn acoustic_loopback_latency() {
    loopback(None);
}

#[test]
#[ignore]
fn acoustic_loopback_latency_with_pdc() {
    loopback(Some(4800));
}

fn loopback(pdc_latency: Option<u32>) {
    if std::env::var("DSTUDIO_TEST_REAL_INPUT").as_deref() != Ok("1") {
        eprintln!("skipped: set DSTUDIO_TEST_REAL_INPUT=1");
        return;
    }
    let engine = Engine::new(Config { sample_rate: 48_000.0, max_block_size: 2048 }).unwrap();
    engine
        .configure_device(&DeviceConfig { output: None, input: Some(DeviceConfig::DEFAULT_INPUT.into()), sample_rate: 0.0, buffer_size: 0 })
        .expect("configure input");
    engine.start_device().expect("start device");
    let st = engine.device_status();
    eprintln!(
        "device: out {:?} in {:?}, latency in {} + out {} frames",
        st.output_name, st.input_name, st.input_latency_frames, st.output_latency_frames
    );
    assert!(st.input_name.is_some(), "input not open: {:?}", engine.input_error());

    let track = TrackSnapshot {
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
        notes: vec![],
        synth: None,
        devices: vec![],
        instrument_plugin: None,
        automation: vec![],
        arm: true,
        monitor: false,
        input: Some(InputSource { channel: 0, stereo: false }),
    };
    let mut tracks = vec![track];
    if let Some(n) = pdc_latency {
        // A silent track whose only job is to make L = n.
        let mut lat = tracks[0].clone();
        lat.track_id = 2;
        lat.arm = false;
        lat.input = None;
        let mut dev = Device::new(PluginRef {
            format: PluginFormat::Clap,
            id: "studio.dstudio.test.latency".into(),
            path: Some(dstudio_engine::test_plugin_dir().join("DStudioTestLatency.clap").to_string_lossy().into_owned()),
            name: "DStudio Test Latency".into(),
            vendor: "DStudio".into(),
            version: "1.0.0".into(),
            kind: PluginKind::Effect,
        });
        dev.id = DeviceId(100);
        dev.state = Some(dstudio_core::device::b64::encode(&n.to_le_bytes()));
        lat.devices = vec![dev];
        tracks.push(lat);
    }
    engine
        .publish(&SessionSnapshot {
            tracks,
            master_gain: 1.0,
            master_automation: vec![],
            device_assets: Default::default(),
            tempo: vec![],
            click_quarters: 1.0,
        })
        .unwrap();
    let report = engine.latency_report();
    eprintln!("delay compensation: L = {} frames", report.total_frames);
    assert_eq!(report.total_frames, pdc_latency.unwrap_or(0));
    let fpb = 24_000.0; // 120 BPM at 48 kHz
    engine.set_metronome(MetronomeConfig { enabled: true, gain: 0.35, frames_per_beat: fpb, beats_per_bar: 4 }).unwrap();
    engine.seek(0).unwrap();

    let dir = std::env::temp_dir().join("dstudio_loopback");
    std::fs::create_dir_all(&dir).unwrap();
    engine.start_recording(&dir).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(4_200));
    let takes = engine.stop_recording().unwrap();
    engine.stop().unwrap();
    engine.set_metronome(MetronomeConfig { enabled: false, gain: 0.0, frames_per_beat: fpb, beats_per_bar: 4 }).unwrap();
    engine.stop_device().unwrap();
    assert_eq!(takes.len(), 1, "takes: {takes:?}");
    let take = &takes[0];
    eprintln!("take: start_frame {} frames {} file {}", take.start_frame, take.frames, take.path.display());

    // Read the float32 WAV (header written by the engine: 'data' chunk search).
    let bytes = std::fs::read(&take.path).unwrap();
    let data = bytes.windows(4).position(|w| w == b"data").expect("data chunk") + 8;
    let x: Vec<f32> = bytes[data..].chunks_exact(4).map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect();

    // Noise floor from the quietest 10 ms window, threshold well above it.
    let win = 480;
    let floor = x.chunks(win).map(|c| c.iter().map(|v| v.abs()).fold(0.0, f32::max)).fold(f32::MAX, f32::min);
    let peak = x.iter().map(|v| v.abs()).fold(0.0, f32::max);
    let thr = (floor * 8.0).max(peak * 0.25);
    eprintln!("floor {floor:.5} peak {peak:.5} threshold {thr:.5}");

    // For each beat whose window lies inside the take, find the first sample above threshold.
    let mut offsets = Vec::new();
    for k in 0.. {
        let beat = (k as f64 * fpb) as i64;
        let idx0 = beat - take.start_frame - 2_400; // search from 50 ms before the beat
        if idx0 < 0 {
            continue;
        }
        let idx1 = idx0 + 9_600; // to 150 ms after
        if idx1 as usize >= x.len() {
            break;
        }
        if let Some(i) = (idx0..idx1).find(|&i| x[i as usize].abs() > thr) {
            let timeline = i + take.start_frame;
            offsets.push(timeline - beat);
        }
    }
    let ms: Vec<String> = offsets.iter().map(|o| format!("{:+.2}", *o as f64 / 48.0)).collect();
    eprintln!("click onset - beat (ms): [{}]", ms.join(", "));
    assert!(offsets.len() >= 4, "too few clicks detected ({}); is the output audible to the mic?", offsets.len());
    let mut sorted = offsets.clone();
    sorted.sort_unstable();
    let median = sorted[sorted.len() / 2] as f64 / 48.0;
    eprintln!("median offset {median:+.2} ms (positive = recording lands late)");
    // Measured on a MacBook Air (2026-11-08): +2.7 ms with a tight spread, or
    // +6.2 ms when the threshold detector catches a later cycle of the click
    // (bimodal ~3.5 ms, independent of the compensation). Before the fix it was
    // -18.5 ms. 8 ms catches buffer-sized errors (10.7 ms at 512 frames).
    assert!(median.abs() < 8.0, "latency compensation off by {median:.2} ms");
}
