//! Built-in devices through the Rust wrapper: the engine registry matches
//! the core definitions, a compressor in a published chain compresses,
//! reports readouts, takes live parameter updates and SE_AUTO_BUILTIN
//! automation, and bypasses.

use dstudio_core::automation::AutomationTarget;
use dstudio_core::builtin;
use dstudio_core::snapshot::{ClipSnapshot, LaneSnapshot, NoteSnapshot, SessionSnapshot, TrackSnapshot};
use dstudio_core::{AssetId, DeviceId, DeviceSample, Slice, TrackId};
use dstudio_engine::{Config, Engine, builtin_registry, builtin_state, builtin_state_v2};
use std::path::Path;

fn write_dc_wav(path: &Path, frames: u32, v: i16) {
    let data_len = frames * 2;
    let mut b = Vec::new();
    b.extend_from_slice(b"RIFF");
    b.extend_from_slice(&(36 + data_len).to_le_bytes());
    b.extend_from_slice(b"WAVEfmt ");
    b.extend_from_slice(&16u32.to_le_bytes());
    b.extend_from_slice(&1u16.to_le_bytes());
    b.extend_from_slice(&1u16.to_le_bytes());
    b.extend_from_slice(&48_000u32.to_le_bytes());
    b.extend_from_slice(&96_000u32.to_le_bytes());
    b.extend_from_slice(&2u16.to_le_bytes());
    b.extend_from_slice(&16u16.to_le_bytes());
    b.extend_from_slice(b"data");
    b.extend_from_slice(&data_len.to_le_bytes());
    for _ in 0..frames {
        b.extend_from_slice(&v.to_le_bytes());
    }
    std::fs::write(path, b).unwrap();
}

#[test]
fn engine_registry_matches_core_definitions() {
    let reg = builtin_registry();
    for def in builtin::all() {
        let p = reg.iter().find(|p| p.id == def.id).unwrap_or_else(|| panic!("engine has no processor {:?}", def.id));
        let ids: Vec<&str> = def.params.iter().map(|q| q.id).collect();
        assert_eq!(p.params, ids, "{}: parameter order differs between dstudio-core and the engine", def.id);
        let defaults: Vec<f32> = def.params.iter().map(|q| q.default).collect();
        assert_eq!(p.defaults, defaults, "{}: defaults differ between dstudio-core and the engine", def.id);
    }
    // The panel reads a strip device's first readouts as input, output, gain reduction.
    for def in builtin::all().iter().filter(|d| d.layout.view.is_some_and(|v| v.strip().is_some())) {
        let p = reg.iter().find(|p| p.id == def.id).unwrap();
        assert_eq!(p.readouts[..3.min(p.readouts.len())], ["in_db", "out_db", "gr_db"], "{}: readouts", def.id);
    }
    let ts = reg.iter().find(|p| p.id == "transient-shaper").unwrap();
    assert_eq!(ts.readouts, ["out_peak", "pre_peak", "gain_db"]);
}

#[test]
fn state_blob_layout() {
    let b = builtin_state(&[1.0, -2.5], 7);
    assert_eq!(b.len(), 24);
    assert_eq!(&b[0..4], b"SBI1");
    assert_eq!(u32::from_le_bytes(b[4..8].try_into().unwrap()), 2);
    assert_eq!(u64::from_le_bytes(b[8..16].try_into().unwrap()), 7);
    assert_eq!(f32::from_le_bytes(b[20..24].try_into().unwrap()), -2.5);
}

struct Rig {
    engine: Engine,
    snap: SessionSnapshot,
    _dir: tempfile::TempDir,
}

const DEV: u64 = 41;

impl Rig {
    fn new() -> Rig {
        Rig::with("compressor")
    }

    fn with(device: &str) -> Rig {
        let dir = tempfile::tempdir().unwrap();
        let wav = dir.path().join("dc.wav");
        write_dc_wav(&wav, 96_000, 16384); // 0.5, about -6 dBFS
        let engine = Engine::new(Config { sample_rate: 48_000.0, max_block_size: 256 }).unwrap();
        let info = engine.load_file(&wav).unwrap();
        let mut dev = builtin::find(device).unwrap().instantiate();
        dev.id = DeviceId(DEV);
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
            clips: vec![ClipSnapshot {
                clip_id: 1,
                engine_asset: info.asset_id,
                timeline_start: 0,
                source_offset: 0,
                length: 96_000,
                gain: 1.0,
                warp: None,
                clip_loop: None,
                fades: Default::default(),
            }],
            instrument: false,
            notes: vec![],
            automation: vec![],
            arm: false,
            monitor: false,
            input: None,
            devices: vec![dev],
            instrument_plugin: None,
            synth: None,
        };
        let snap = SessionSnapshot {
            tracks: vec![track],
            master_gain: 1.0,
            master_automation: vec![],
            device_assets: Default::default(),
            tempo: vec![],
            click_quarters: 1.0,
        };
        Rig { engine, snap, _dir: dir }
    }

    /// Publish, render 0.6 s from the start (several release times, so the
    /// previous render's gain reduction has recovered) and return the last
    /// sample's absolute left level.
    fn render(&self) -> f32 {
        self.engine.publish(&self.snap).unwrap();
        self.engine.seek(0).unwrap();
        self.engine.play().unwrap();
        let mut out = vec![0f32; 2 * 28_800];
        for chunk in out.chunks_mut(512) {
            self.engine.render_offline(chunk).unwrap();
        }
        out[out.len() - 2].abs()
    }

    fn readouts(&self) -> Vec<f32> {
        let mut r = Vec::new();
        self.engine.builtin_readouts(DEV, &mut r).unwrap();
        r
    }
}

#[test]
fn limiter_in_a_chain_holds_its_ceiling_and_reports_latency() {
    let mut rig = Rig::with("limiter");
    rig.snap.tracks[0].devices[0].enabled = false;
    let pan_law = rig.render() / 0.5; // the 0.5 DC clip through the track, no device
    rig.snap.tracks[0].devices[0].enabled = true;
    rig.snap.tracks[0].devices[0].params.insert("input_gain".into(), 12.0);
    let out = rig.render() / pan_law;
    let ceiling = 10f32.powf(-0.3 / 20.0);
    assert!(out <= ceiling + 1e-3 && out > ceiling - 0.02, "level {out} ceiling {ceiling}");
    assert!(rig.readouts()[2] < -5.0, "gain reduction {:?}", rig.readouts());
    assert!(rig.engine.plugin_latency(DeviceId(DEV)) > 0);
}

#[test]
fn mastering_limiter_latency_follows_oversampling() {
    let mut rig = Rig::with("mastering-limiter");
    let mut latency = Vec::new();
    for os in 0..4 {
        rig.snap.tracks[0].devices[0].params.insert("oversampling".into(), os as f32);
        rig.render();
        latency.push(rig.engine.plugin_latency(DeviceId(DEV)));
    }
    assert!(latency.windows(2).all(|w| w[1] != w[0]), "latency per oversampling factor {latency:?}");
    rig.snap.tracks[0].devices[0].params.insert("threshold".into(), -12.0);
    let out = rig.render();
    assert!(out <= 10f32.powf(-1.0 / 20.0) + 1e-3, "level {out} over the -1 dB ceiling");
    assert!(rig.readouts()[2] < -1.0, "gain reduction {:?}", rig.readouts());
}

#[test]
fn compressor_in_a_chain_compresses_and_bypasses() {
    let mut rig = Rig::new();
    let compressed = rig.render();
    let r = rig.readouts();
    assert_eq!(r.len(), 3);
    assert!(r[2] < -6.0, "gain reduction {:?}", r);
    assert!(r[1] < r[0], "out below in: {:?}", r);

    rig.snap.tracks[0].devices[0].enabled = false;
    let dry = rig.render();
    assert!(compressed < dry * 0.6, "compressed {compressed} dry {dry}");

    // Threshold at 0 dB: the -6 dBFS signal passes (soft knee 6 dB: little reduction).
    rig.snap.tracks[0].devices[0].enabled = true;
    rig.snap.tracks[0].devices[0].params.insert("threshold".into(), 0.0);
    let open = rig.render();
    assert!(open > dry * 0.9, "open {open} dry {dry}");
}

#[test]
fn chain_head_macro_sets_only_members_of_its_chain() {
    let mut rig = Rig::new();
    let compressed = rig.render();
    let mut head = builtin::find(dstudio_core::modulation::RACK_ID).unwrap().instantiate();
    head.id = DeviceId(DEV + 1);
    // Macro 1 pins the member's threshold at the top of its range (0 dB).
    head.links.push(dstudio_core::ModLink {
        slot: 0,
        target: DeviceId(DEV),
        param: dstudio_core::ModParam::Builtin("threshold".into()),
        min: 1.0,
        max: 1.0,
    });
    let t = &mut rig.snap.tracks[0];
    t.devices[0].chain = Some(head.id);
    t.devices.insert(0, head);
    let open = rig.render();
    assert!(open > compressed * 1.5, "macro-opened {open} compressed {compressed}");
    assert!(rig.readouts()[2] > -1.0, "gain reduction {:?}", rig.readouts());
    // A link to a device outside the chain is inert.
    rig.snap.tracks[0].devices[1].chain = None;
    let outside = rig.render();
    assert!((outside - compressed).abs() < compressed * 0.05, "outside {outside} compressed {compressed}");
}

#[test]
fn live_param_update_and_unknown_device() {
    let rig = Rig::new();
    rig.render();
    let before = rig.readouts()[2];
    rig.engine.builtin_set_param(DEV, 0, 0.0).unwrap(); // threshold
    let mut out = vec![0f32; 2 * 12_000];
    for chunk in out.chunks_mut(512) {
        rig.engine.render_offline(chunk).unwrap();
    }
    let after = rig.readouts()[2];
    assert!(after > before + 6.0, "gr before {before} after {after}");
    assert!(rig.engine.builtin_set_param(DEV + 1, 0, 0.0).is_err());
    assert!(rig.engine.builtin_set_param(DEV, 99, 0.0).is_err());
    assert!(rig.engine.builtin_set_param(DEV, 0, f32::NAN).is_err());
}

#[test]
fn automation_lane_drives_a_builtin_param() {
    let mut rig = Rig::new();
    rig.snap.tracks[0].devices[0].params.insert("threshold".into(), 0.0);
    rig.snap.tracks[0].automation.push(LaneSnapshot {
        target: AutomationTarget::Builtin { device: DeviceId(DEV), device_type: "compressor".into(), param: "threshold".into() },
        points: vec![(0, -40.0, 0.0), (96_000, -40.0, 0.0)],
    });
    let automated = rig.render();
    assert!(rig.readouts()[2] < -10.0, "automated threshold -40 dB should compress: {:?}", rig.readouts());
    rig.snap.tracks[0].automation.clear();
    let plain = rig.render();
    assert!(automated < plain * 0.5, "automated {automated} plain {plain}");
}

#[test]
fn sidechain_from_another_track() {
    let mut rig = Rig::new();
    // Track 2: the same DC, with the compressor keyed from track 1 at -60 dB
    // threshold; track 1 muted from the master by a 0 gain fader would also
    // silence its tap (post-fader), so keep it audible and compare levels.
    let mut keyed = rig.snap.tracks[0].clone();
    keyed.track_id = 2;
    keyed.clips[0].clip_id = 2;
    keyed.devices[0].id = DeviceId(DEV + 1);
    keyed.devices[0].sidechain = Some(TrackId(1));
    keyed.devices[0].params.insert("sidechainEnable".into(), 1.0);
    keyed.devices[0].params.insert("sidechain_gain".into(), 24.0); // key hotter than the input
    rig.snap.tracks[0].devices.clear();
    rig.snap.tracks.push(keyed);
    rig.render();
    let mut r = Vec::new();
    rig.engine.builtin_readouts(DEV + 1, &mut r).unwrap();
    let keyed_gr = r[2];

    rig.snap.tracks[1].devices[0].params.insert("sidechainEnable".into(), 0.0);
    rig.render();
    rig.engine.builtin_readouts(DEV + 1, &mut r).unwrap();
    assert!(keyed_gr < r[2] - 6.0, "keyed {keyed_gr} vs self {}", r[2]);
}

fn drum_rig(notes: Vec<NoteSnapshot>) -> (Engine, SessionSnapshot, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let wav = dir.path().join("dc.wav");
    write_dc_wav(&wav, 96_000, 16384);
    let engine = Engine::new(Config { sample_rate: 48_000.0, max_block_size: 256 }).unwrap();
    let info = engine.load_file(&wav).unwrap();
    let mut dev = builtin::find("drum-rack").unwrap().instantiate();
    dev.id = DeviceId(DEV);
    dev.samples.push(DeviceSample { slot: 0, asset: AssetId(5), name: "dc".into(), slices: vec![] });
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
        instrument: true,
        notes,
        automation: vec![],
        arm: false,
        monitor: false,
        input: None,
        devices: vec![],
        instrument_plugin: Some(dev),
        synth: None,
    };
    let snap = SessionSnapshot {
        tracks: vec![track],
        master_gain: 1.0,
        master_automation: vec![],
        device_assets: [(AssetId(5), info.asset_id)].into(),
        tempo: vec![],
        click_quarters: 1.0,
    };
    (engine, snap, dir)
}

fn peak(engine: &Engine, frames: usize) -> f32 {
    let mut out = vec![0f32; 2 * frames];
    for chunk in out.chunks_mut(512) {
        engine.render_offline(chunk).unwrap();
    }
    out.iter().fold(0f32, |m, v| m.max(v.abs()))
}

#[test]
fn drum_rack_plays_its_pads_from_notes_and_auditions() {
    let reg = builtin_registry();
    for (id, slots) in [("sampler-steam", 1), ("pad-sampler", 8), ("drum-rack", 128)] {
        let p = reg.iter().find(|p| p.id == id).unwrap();
        assert_eq!((p.sample_slots, p.instrument), (slots, true), "{id}");
        assert_eq!(builtin::find(id).unwrap().samples, slots, "{id}");
    }

    let note = |pitch| vec![NoteSnapshot { start: 0, end: 12_000, pitch, velocity: 127 }];
    let (engine, snap, _dir) = drum_rig(note(36));
    engine.publish(&snap).unwrap();
    engine.seek(0).unwrap();
    engine.play().unwrap();
    assert!(peak(&engine, 9_600) > 0.25, "pad 1 (note 36) plays its sample");
    let mut r = Vec::new();
    engine.builtin_readouts(DEV, &mut r).unwrap();
    assert_eq!(r.len(), 48);

    let (engine, snap, _dir) = drum_rig(note(38));
    engine.publish(&snap).unwrap();
    engine.seek(0).unwrap();
    engine.play().unwrap();
    assert!(peak(&engine, 9_600) < 1e-6, "pad 3 is empty");

    let (engine, snap, _dir) = drum_rig(vec![]);
    engine.publish(&snap).unwrap();
    assert!(peak(&engine, 2_048) < 1e-6);
    engine.builtin_note(DEV, 36, 100).unwrap();
    assert!(peak(&engine, 4_800) > 0.1, "audition plays pad 1");
    assert!(engine.builtin_note(DEV + 1, 36, 100).is_err());
}

#[test]
fn state_v2_blob_layout() {
    let sl = [Slice::at(0.0), Slice { start: 0.5, gain: -3.0, pan: 0.25, pitch: 2.0 }];
    let b = builtin_state_v2(&[1.0], 0, &[(9, 3, &sl[..])], &[]);
    assert_eq!(b.len(), 32 + 4 + 16 + 32);
    assert_eq!(&b[0..4], b"SBI2");
    assert_eq!(u32::from_le_bytes(b[16..20].try_into().unwrap()), 1);
    assert_eq!(u32::from_le_bytes(b[20..24].try_into().unwrap()), 2);
    assert_eq!(u32::from_le_bytes(b[24..28].try_into().unwrap()), 0);
    assert_eq!(u64::from_le_bytes(b[36..44].try_into().unwrap()), 9);
    assert_eq!(u32::from_le_bytes(b[44..48].try_into().unwrap()), 3);
    assert_eq!(f32::from_le_bytes(b[68..72].try_into().unwrap()), 0.5);
    assert_eq!(f32::from_le_bytes(b[72..76].try_into().unwrap()), -3.0);
}
