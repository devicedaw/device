//! Audio clip warping in the model: tempo following, speed, original BPM,
//! warp markers, transient edits, the command with undo, persistence, and
//! the engine snapshot (map points and slices).

use dstudio_core::snapshot::{self, WarpSnapshot};
use dstudio_core::warp::{ClipWarp, Transient, WarpMode};
use dstudio_core::*;
use std::collections::HashMap;
use std::time::{Duration, Instant};

const RATE: f64 = 48_000.0;

fn doc() -> (Document, ClipId, AssetId) {
    let mut d = Document::new(Project::default());
    let track = d.project().tracks[0].id;
    let asset = Asset {
        id: AssetId(0),
        relative_path: None,
        absolute_path: "/nonexistent/loop.wav".into(),
        sample_rate: 44_100.0,
        channels: 2,
        frames: 44_100 * 8,
    };
    d.execute(Command::AddAsset { asset }, Origin::User).unwrap();
    let a = d.project().assets[0].id;
    let clip = Clip {
        id: ClipId(0),
        asset: a,
        name: "loop".into(),
        start: PPQ * 4,
        source_offset: 0,
        source_length: 44_100 * 4,
        gain: 1.0,
        warp: ClipWarp::default(),
        disabled: false,
        clip_loop: None,
        fades: Default::default(),
        takes: Vec::new(),
    };
    d.execute(Command::AddClip { track, clip }, Origin::User).unwrap();
    let id = d.project().tracks[0].clips[0].id;
    (d, id, a)
}

fn warp(d: &Document, c: ClipId) -> ClipWarp {
    d.project().clip(c).unwrap().warp.clone()
}

fn set(d: &mut Document, c: ClipId, w: ClipWarp, label: &str) {
    d.execute(Command::SetClipWarp { clip: c, warp: w, label: label.into() }, Origin::User).unwrap();
}

fn snap_warp(d: &Document, c: ClipId, transients: &HashMap<AssetId, Vec<Transient>>) -> (i64, i64, Option<WarpSnapshot>) {
    let a = d.project().clip(c).unwrap().asset;
    let (s, skipped) = snapshot::build_with_transients(d.project(), RATE, &HashMap::from([(a, 7)]), transients);
    assert!(skipped.is_empty());
    let cs = s.tracks[0].clips.iter().find(|x| x.clip_id == c.0).unwrap();
    (cs.timeline_start, cs.length, cs.warp.clone())
}

fn slope(points: &[(i64, f64)]) -> f64 {
    let (a, b) = (points[0], points[points.len() - 1]);
    (b.1 - a.1) / (b.0 - a.0) as f64
}

#[test]
fn warp_off_plays_one_to_one() {
    let (d, c, _) = doc();
    let (start, len, w) = snap_warp(&d, c, &HashMap::new());
    assert_eq!((start, len, w), (96_000, 192_000, None));
    assert_eq!(d.project().clip_length_ticks(d.project().clip(c).unwrap()), 8 * PPQ);
}

#[test]
fn warp_on_at_the_project_tempo_keeps_the_clip_and_follows_tempo_changes() {
    let (mut d, c, _) = doc();
    set(&mut d, c, ClipWarp { enabled: true, original_bpm: 120.0, ..ClipWarp::default() }, "Set clip warp mode");
    let (start, len, w) = snap_warp(&d, c, &HashMap::new());
    let w = w.unwrap();
    assert_eq!((start, len), (96_000, 192_000));
    assert_eq!(w.mode, 2, "Auto without transients stretches");
    assert!((slope(&w.points) - 1.0).abs() < 1e-9);
    assert_eq!(w.points[0].0, 96_000);
    assert!(w.points[0].1.abs() < 1e-9);
    // 120 -> 90 BPM: the 8 beats now take 5.33 s and the source plays slower.
    d.execute(Command::SetTempo { bpm: 90.0, at: None }, Origin::User).unwrap();
    let (start, len, w) = snap_warp(&d, c, &HashMap::new());
    assert_eq!(d.project().clip_length_ticks(d.project().clip(c).unwrap()), 8 * PPQ);
    assert_eq!((start, len), (128_000, 256_000));
    assert!((slope(&w.unwrap().points) - 0.75).abs() < 1e-6);
}

#[test]
fn speed_and_original_bpm_scale_the_footprint() {
    let (mut d, c, _) = doc();
    let on = ClipWarp { enabled: true, original_bpm: 120.0, mode: WarpMode::Melody, ..ClipWarp::default() };
    for (speed, beats) in [(2.0, 4), (0.5, 16), (1.5, 5)] {
        set(&mut d, c, ClipWarp { speed, ..on.clone() }, "Set clip speed");
        let clip = d.project().clip(c).unwrap();
        assert_eq!(d.project().clip_length_ticks(clip), (8.0 / speed * PPQ as f64).round() as i64, "speed {speed} ~ {beats} beats");
        let (_, _, w) = snap_warp(&d, c, &HashMap::new());
        assert!((slope(&w.unwrap().points) - speed).abs() < 1e-3);
    }
    // A 60 BPM loop in a 120 BPM project plays twice as fast: 4 beats.
    set(&mut d, c, ClipWarp { original_bpm: 60.0, ..on.clone() }, "Set original BPM");
    assert_eq!(d.project().clip_length_ticks(d.project().clip(c).unwrap()), 4 * PPQ);
    assert_eq!(snap_warp(&d, c, &HashMap::new()).1, 96_000);
}

#[test]
fn source_and_ticks_convert_both_ways_for_trims() {
    let (mut d, c, _) = doc();
    for w in [ClipWarp::default(), ClipWarp { enabled: true, original_bpm: 90.0, speed: 1.5, ..ClipWarp::default() }] {
        set(&mut d, c, w, "Set clip speed");
        let p = d.project();
        let clip = p.clip(c).unwrap();
        assert!(p.clip_source_at(clip, 0.0).abs() < 1e-6);
        let end = p.clip_length_ticks(clip) as f64;
        assert!((p.clip_source_at(clip, end) - clip.source_length as f64).abs() < 50.0, "the clip end plays the last source frame");
        for t in [-960.0, 100.0, 5000.0] {
            assert!((p.clip_ticks_at(clip, p.clip_source_at(clip, t)) - t).abs() < 1e-6);
        }
    }
}

#[test]
fn markers_pin_source_frames_to_beats() {
    let mut w = ClipWarp { enabled: true, original_bpm: 120.0, ..ClipWarp::default() };
    // Source second 1 (beat 2) pinned to beat 3: the first second stretches.
    assert!(w.set_marker(0, 0.0));
    assert!(w.set_marker(44_100, 3.0));
    assert!(!w.set_marker(22_050, 3.5), "crossing a neighbour is refused");
    assert!((w.beat_at(22_050.0, 44_100.0) - 1.5).abs() < 1e-9);
    assert!((w.source_at(1.5, 44_100.0) - 22_050.0).abs() < 1e-6);
    // Past the last marker the last segment continues.
    assert!((w.beat_at(88_200.0, 44_100.0) - 6.0).abs() < 1e-9);
    let (mut d, c, _) = doc();
    set(&mut d, c, w.clone(), "Move warp marker");
    let (start, _, s) = snap_warp(&d, c, &HashMap::new());
    let pts = s.unwrap().points;
    assert_eq!(pts.len(), 3, "clip start, the marker at 1 s, clip end: {pts:?}");
    assert_eq!(pts[1].0 - start, 72_000, "beat 3 at 120 BPM");
    // The last segment (3 beats per source second) continues: 4 s = 12 beats -> 6 s at 120 BPM.
    assert_eq!(d.project().clip_length_ticks(d.project().clip(c).unwrap()), 12 * PPQ);
    assert_eq!(pts[2].0 - start, 288_000);
    // A marker inside the clip appears in the map.
    let mut w2 = w.clone();
    w2.markers.clear();
    assert!(w2.set_marker(44_100, 2.0) && w2.set_marker(88_200, 5.0));
    set(&mut d, c, w2, "Move warp marker");
    let pts = snap_warp(&d, c, &HashMap::new()).2.unwrap().points;
    assert_eq!(pts.len(), 4, "{pts:?}");
    assert!((pts[1].1 - 48_000.0).abs() < 1e-6 && (pts[2].1 - 96_000.0).abs() < 1e-6, "{pts:?}");
    // The first segment (3 beats/s) extends back: the clip start is beat -1.
    assert_eq!((pts[1].0 - start, pts[2].0 - start), (72_000, 144_000));
    assert!(w.remove_marker(1) && !w.remove_marker(5));
}

#[test]
fn transient_edits_add_remove_and_move_with_a_1ms_tolerance() {
    let det = [Transient { source: 1000, strength: 1.0, percussive: true }, Transient { source: 30_000, strength: 0.5, percussive: false }];
    let rate = 44_100.0;
    let mut w = ClipWarp::default();
    assert!(!w.add_transient(&det, 1030, rate), "within 1 ms of a detected one");
    assert!(w.add_transient(&det, 40_000, rate));
    assert_eq!(
        w.transients(&det, rate).iter().map(|t| (t.source, t.percussive)).collect::<Vec<_>>(),
        [(1000, true), (30_000, false), (40_000, false)]
    );
    assert!(w.remove_transient(&det, 1010, rate));
    assert_eq!((w.transients_added.clone(), w.transients_removed.clone()), (vec![40_000], vec![1000]));
    assert!(w.add_transient(&det, 1000, rate), "re-adding undoes the removal");
    assert!(w.transients_removed.is_empty());
    assert!(w.move_transient(&det, 40_000, 50_000, rate));
    assert_eq!(w.transients_added, vec![50_000]);
    assert!(w.move_transient(&det, 30_000, 20_000, rate));
    assert_eq!((w.transients_added.clone(), w.transients_removed.clone()), (vec![20_000, 50_000], vec![30_000]));
    assert!(!w.move_transient(&det, 7, 9, rate), "nothing at 7");
}

#[test]
fn drums_and_auto_slice_at_the_transients_in_the_clip() {
    let (mut d, c, a) = doc();
    let det = vec![
        Transient { source: 0, strength: 1.0, percussive: true },
        Transient { source: 22_050, strength: 1.0, percussive: true },
        Transient { source: 88_200, strength: 1.0, percussive: false },
        Transient { source: 300_000, strength: 1.0, percussive: true },
    ];
    let tr = HashMap::from([(a, det)]);
    set(&mut d, c, ClipWarp { enabled: true, mode: WarpMode::Drums, ..ClipWarp::default() }, "Set clip mode");
    let w = snap_warp(&d, c, &tr).2.unwrap();
    assert_eq!(w.mode, 1);
    let k = RATE / 44_100.0;
    assert_eq!(w.slices, vec![(0.0, true), (22_050.0 * k, true), (88_200.0 * k, false)]);
    // Trimmed start: the first slice starts at the clip start, flagged like its passage.
    d.execute(Command::TrimClip { clip: c, start: PPQ * 4, source_offset: 44_100, source_length: 44_100 * 3 }, Origin::User).unwrap();
    set(&mut d, c, ClipWarp { enabled: true, mode: WarpMode::Auto, ..ClipWarp::default() }, "Set clip mode");
    let w = snap_warp(&d, c, &tr).2.unwrap();
    assert_eq!(w.mode, 4);
    assert_eq!(w.slices, vec![(44_100.0 * k, true), (88_200.0 * k, false)]);
    assert_eq!(w.points[0].0, 96_000);
    assert!((w.points[0].1 - 48_000.0).abs() < 1e-6);
    for (mode, engine) in [(WarpMode::Melody, 2), (WarpMode::Vocal, 2), (WarpMode::Repitch, 0)] {
        set(&mut d, c, ClipWarp { enabled: true, mode, ..ClipWarp::default() }, "Set clip mode");
        let w = snap_warp(&d, c, &tr).2.unwrap();
        assert_eq!((w.mode, w.slices.len()), (engine, 0));
    }
}

#[test]
fn set_clip_warp_undoes_validates_and_coalesces_drags() {
    let (mut d, c, _) = doc();
    let t0 = Instant::now();
    let on = ClipWarp { enabled: true, ..ClipWarp::default() };
    d.execute_at(Command::SetClipWarp { clip: c, warp: on.clone(), label: "Set clip warp mode".into() }, t0).unwrap();
    assert_eq!(d.undo_label(), Some("Set clip warp mode"));
    // An original-BPM drag is one undo step; the next kind of edit is another.
    for (i, bpm) in [121.0, 122.0, 123.0].into_iter().enumerate() {
        d.execute_at(
            Command::SetClipWarp { clip: c, warp: ClipWarp { original_bpm: bpm, ..on.clone() }, label: "Set original BPM".into() },
            t0 + Duration::from_millis(16 * (i as u64 + 1)),
        )
        .unwrap();
    }
    d.execute_at(
        Command::SetClipWarp { clip: c, warp: ClipWarp { original_bpm: 123.0, speed: 2.0, ..on.clone() }, label: "Set clip speed".into() },
        t0 + Duration::from_millis(80),
    )
    .unwrap();
    d.undo().unwrap();
    assert_eq!(warp(&d, c).original_bpm, 123.0);
    d.undo().unwrap();
    assert_eq!(warp(&d, c), on);
    d.undo().unwrap();
    assert_eq!(warp(&d, c), ClipWarp::default());
    d.redo().unwrap();
    assert!(warp(&d, c).enabled);
    for bad in [
        ClipWarp { original_bpm: 10.0, ..on.clone() },
        ClipWarp { original_bpm: f64::NAN, ..on.clone() },
        ClipWarp { speed: 0.0, ..on.clone() },
        ClipWarp { markers: vec![WarpMarker { source: 10, beat: 1.0 }, WarpMarker { source: 20, beat: 0.5 }], ..on.clone() },
        ClipWarp { transients_added: vec![44_100 * 9], ..on.clone() },
        ClipWarp { transients_removed: vec![5, 5], ..on.clone() },
    ] {
        assert!(d.execute(Command::SetClipWarp { clip: c, warp: bad, label: "x".into() }, Origin::User).is_err());
    }
    assert!(d.execute(Command::SetClipWarp { clip: ClipId(999), warp: on, label: "x".into() }, Origin::User).is_err());
}

#[test]
fn warp_settings_persist_and_old_projects_load_unwarped() {
    let (mut d, c, _) = doc();
    let mut w = ClipWarp { enabled: true, mode: WarpMode::Vocal, original_bpm: 96.5, speed: 0.75, ..ClipWarp::default() };
    assert!(w.set_marker(4410, 1.0));
    w.transients_added = vec![100];
    w.transients_removed = vec![200];
    set(&mut d, c, w.clone(), "Set clip speed");
    let dir = tempfile::tempdir().unwrap();
    persist::save(d.project(), dir.path()).unwrap();
    let loaded = persist::load(dir.path()).unwrap().project;
    assert_eq!(loaded.clip(c), d.project().clip(c));
    assert_eq!(loaded.clip(c).unwrap().warp, w);
    // An unwarped clip stores nothing, and a clip without the key loads unwarped.
    let json = serde_json::to_value(Clip { warp: ClipWarp::default(), ..loaded.clip(c).unwrap().clone() }).unwrap();
    assert!(json.get("warp").is_none());
    let back: Clip = serde_json::from_value(json).unwrap();
    assert_eq!(back.warp, ClipWarp::default());
    let partial: ClipWarp = serde_json::from_str(r#"{"enabled":true}"#).unwrap();
    assert_eq!((partial.speed, partial.original_bpm, partial.mode), (1.0, 120.0, WarpMode::Auto));
}

fn snap_full(
    d: &Document,
    c: ClipId,
    reversed: &HashMap<AssetId, u64>,
    transients: &HashMap<AssetId, Vec<Transient>>,
) -> (snapshot::SessionSnapshot, Vec<u64>) {
    let a = d.project().clip(c).unwrap().asset;
    snapshot::build_full(d.project(), RATE, &HashMap::from([(a, 7)]), reversed, transients)
}

#[test]
fn reversed_clip_plays_the_mirrored_window_of_the_reversed_copy() {
    let (mut d, c, a) = doc();
    // Window 1 s .. 3 s of the 8 s asset.
    d.execute(Command::TrimClip { clip: c, start: PPQ * 4, source_offset: 44_100, source_length: 44_100 * 2 }, Origin::User).unwrap();
    set(&mut d, c, ClipWarp { reverse: true, ..ClipWarp::default() }, "Toggle clip reverse");
    let (_, skipped) = snap_full(&d, c, &HashMap::new(), &HashMap::new());
    assert_eq!(skipped, vec![c.0], "no reversed copy yet: skipped, not played forwards");
    let (s, skipped) = snap_full(&d, c, &HashMap::from([(a, 8)]), &HashMap::new());
    assert!(skipped.is_empty());
    let cs = &s.tracks[0].clips[0];
    assert_eq!(
        (cs.engine_asset, cs.timeline_start, cs.source_offset, cs.length, cs.warp.is_none()),
        (8, 96_000, 5 * 48_000, 2 * 48_000, true)
    );
    d.undo().unwrap();
    assert!(!warp(&d, c).reverse);
}

#[test]
fn reversed_warped_clip_mirrors_its_map_and_slices() {
    let (mut d, c, a) = doc();
    let mut w = ClipWarp { enabled: true, mode: WarpMode::Drums, original_bpm: 120.0, reverse: true, ..ClipWarp::default() };
    assert!(w.set_marker(0, 0.0));
    assert!(w.set_marker(44_100, 3.0));
    set(&mut d, c, w.clone(), "Set clip speed");
    let detected = HashMap::from([(
        a,
        vec![Transient { source: 22_050, strength: 1.0, percussive: false }, Transient { source: 88_200, strength: 1.0, percussive: true }],
    )]);
    set(&mut d, c, ClipWarp { reverse: false, ..w.clone() }, "Toggle clip reverse");
    let (_, _, fwd) = snap_warp(&d, c, &detected);
    let fwd = fwd.unwrap();
    set(&mut d, c, w, "Toggle clip reverse");
    let (s, _) = snap_full(&d, c, &HashMap::from([(a, 8)]), &detected);
    let cs = &s.tracks[0].clips[0];
    let rev = cs.warp.clone().unwrap();
    let frames = 8.0 * 48_000.0;
    let (t0, t1) = (cs.timeline_start, cs.timeline_start + cs.length);
    assert_eq!(rev.points.len(), fwd.points.len());
    for (r, f) in rev.points.iter().zip(fwd.points.iter().rev()) {
        assert_eq!(r.0, t0 + t1 - f.0);
        assert!((r.1 - (frames - f.1)).abs() < 1e-9);
    }
    assert!(rev.points.windows(2).all(|p| p[1].0 > p[0].0 && p[1].1 > p[0].1), "{:?}", rev.points);
    assert_eq!(rev.points[0].0, t0);
    // Forward slices: clip start (in a non-percussive passage), 22050, 88200 (asset frames).
    // Mirrored: the window start in the copy takes the last slice's flag, then each boundary the flag before it.
    let r = 48_000.0 / 44_100.0;
    assert_eq!(fwd.slices, vec![(0.0, true), (22_050.0 * r, false), (88_200.0 * r, true)]);
    let want = [(frames - 176_400.0 * r, true), (frames - 88_200.0 * r, false), (frames - 22_050.0 * r, true)];
    assert_eq!(rev.slices.len(), 3);
    for (g, w) in rev.slices.iter().zip(want) {
        assert!((g.0 - w.0).abs() < 1e-6 && g.1 == w.1, "{:?} vs {want:?}", rev.slices);
    }
}

#[test]
fn reverse_persists() {
    let (mut d, c, _) = doc();
    set(&mut d, c, ClipWarp { reverse: true, ..ClipWarp::default() }, "Toggle clip reverse");
    let dir = tempfile::tempdir().unwrap();
    persist::save(d.project(), dir.path()).unwrap();
    assert!(persist::load(dir.path()).unwrap().project.clip(c).unwrap().warp.reverse);
    assert!(ClipWarp { reverse: true, ..ClipWarp::default() }.tempo_is_default());
}

#[test]
fn transpose_with_warp_off_varispeeds_the_clip() {
    let (mut d, c, _) = doc();
    set(&mut d, c, ClipWarp { transpose: 12, ..ClipWarp::default() }, "Adjust clip transpose");
    let clip = d.project().clip(c).unwrap().clone();
    assert!(!clip.warp.enabled);
    assert_eq!(d.project().clip_length_ticks(&clip), 4 * PPQ, "an octave up plays twice as fast");
    assert!((d.project().clip_source_at(&clip, (2 * PPQ) as f64) - 44_100.0 * 2.0).abs() < 1e-6);
    assert!((d.project().clip_ticks_at(&clip, 44_100.0 * 2.0) - (2 * PPQ) as f64).abs() < 1e-6);
    let (start, len, w) = snap_warp(&d, c, &HashMap::new());
    let w = w.expect("a varispeed clip goes to the engine as Re-pitch");
    assert_eq!((start, len, w.mode, w.transpose), (96_000, 96_000, 0, 0.0));
    assert_eq!((w.points.len(), w.points[0], w.points[1].0), (2, (96_000, 0.0), 192_000));
    assert!((w.points[1].1 - 4.0 * RATE).abs() < 1e-6, "{:?}", w.points);
    assert!((slope(&w.points) - 2.0).abs() < 1e-9);
    d.undo().unwrap();
    assert_eq!(snap_warp(&d, c, &HashMap::new()), (96_000, 192_000, None));
}

#[test]
fn transpose_with_warp_on_keeps_the_timing_except_in_repitch() {
    let (mut d, c, _) = doc();
    let on = ClipWarp { enabled: true, original_bpm: 120.0, transpose: -5, ..ClipWarp::default() };
    for (mode, engine_transpose) in [(WarpMode::Melody, -5.0), (WarpMode::Vocal, -5.0), (WarpMode::Drums, -5.0), (WarpMode::Repitch, 0.0)] {
        set(&mut d, c, ClipWarp { mode, ..on.clone() }, "Set clip mode");
        let (_, len, w) = snap_warp(&d, c, &HashMap::new());
        assert_eq!(len, 192_000, "{mode:?}: the length follows the tempo, not the pitch");
        assert_eq!(w.unwrap().transpose, engine_transpose, "{mode:?}");
    }
}

#[test]
fn transpose_is_validated_and_persisted() {
    let (mut d, c, _) = doc();
    let bad = ClipWarp { transpose: 25, ..ClipWarp::default() };
    assert!(d.execute(Command::SetClipWarp { clip: c, warp: bad, label: "Adjust clip transpose".into() }, Origin::User).is_err());
    set(&mut d, c, ClipWarp { transpose: -24, ..ClipWarp::default() }, "Adjust clip transpose");
    let dir = tempfile::tempdir().unwrap();
    persist::save(d.project(), dir.path()).unwrap();
    assert_eq!(persist::load(dir.path()).unwrap().project.clip(c).unwrap().warp.transpose, -24);
    assert!(!ClipWarp { transpose: 3, ..ClipWarp::default() }.is_default());
    assert!(ClipWarp { transpose: 3, ..ClipWarp::default() }.tempo_is_default());
}

fn pulse(bpm: f64, bars: usize, sub: usize, jitter_ms: f64) -> Vec<Transient> {
    let rate = 44_100.0;
    let step = 60.0 / bpm / sub as f64;
    let mut seed = 7u64;
    (0..bars * 4 * sub)
        .map(|i| {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            let j = ((seed >> 33) as f64 / (1u64 << 31) as f64 - 0.5) * 2.0 * jitter_ms / 1000.0;
            let t = (0.25 + i as f64 * step + j).max(0.0);
            Transient { source: (t * rate) as u64, strength: if i % sub == 0 { 1.0 } else { 0.4 }, percussive: true }
        })
        .collect()
}

#[test]
fn tempo_detection_finds_the_beat() {
    use dstudio_core::warp::estimate_bpm;
    assert_eq!(estimate_bpm(&pulse(123.0, 16, 2, 3.0), 44_100.0), Some(123.0), "eighths with jitter");
    assert_eq!(estimate_bpm(&pulse(140.0, 16, 1, 0.0), 44_100.0), Some(140.0), "quarters");
    assert_eq!(estimate_bpm(&pulse(128.0, 8, 4, 2.0), 44_100.0), Some(128.0), "sixteenths");
    let b = estimate_bpm(&pulse(97.5, 16, 2, 1.0), 44_100.0).unwrap();
    assert!((b - 97.5).abs() <= 0.1, "{b}");
    assert_eq!(estimate_bpm(&pulse(75.0, 16, 1, 0.0), 44_100.0), Some(150.0), "folded into 80..160");
}

#[test]
fn tempo_detection_declines_without_a_pulse() {
    use dstudio_core::warp::estimate_bpm;
    assert_eq!(estimate_bpm(&pulse(120.0, 1, 1, 0.0), 44_100.0), None, "four hits are too few");
    let mut seed = 3u64;
    let mut t = 0.0;
    let scattered: Vec<Transient> = (0..120)
        .map(|_| {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            t += 0.05 + (seed >> 33) as f64 / (1u64 << 31) as f64 * 0.6;
            Transient { source: (t * 44_100.0) as u64, strength: 1.0, percussive: true }
        })
        .collect();
    assert_eq!(estimate_bpm(&scattered, 44_100.0), None);
}

#[test]
fn sliding_a_marker_keeps_its_beat_and_stays_between_neighbours() {
    let mut w = ClipWarp { enabled: true, ..ClipWarp::default() };
    for (s, b) in [(0, 0.0), (44_100, 2.0), (88_200, 4.0)] {
        assert!(w.set_marker(s, b));
    }
    assert!(w.slide_marker(44_100, 50_000));
    assert_eq!(w.markers[1].source, 50_000);
    assert_eq!(w.markers[1].beat, 2.0);
    assert!(!w.slide_marker(50_000, 88_200), "would reach the next marker");
    assert!(!w.slide_marker(50_000, 0), "would reach the previous marker");
    assert!(!w.slide_marker(12_345, 20_000), "no marker there");
    assert_eq!(w.markers[1].source, 50_000);
}

#[test]
fn looped_clip_snapshot_carries_its_loop_in_content_frames() {
    let (mut d, c, a) = doc();
    // 4 s of source at 120 BPM: 8 beats of content. Loop beats 2..4 over a 16-beat clip.
    let l = AudioLoop { on: true, start: 2 * PPQ, end: 4 * PPQ, length: 16 * PPQ };
    d.execute(Command::SetClipLoop { clip: c, clip_loop: Some(l) }, Origin::User).unwrap();
    assert_eq!(d.project().clip_length_ticks(d.project().clip(c).unwrap()), 16 * PPQ);
    assert_eq!(d.undo_label(), Some("Clip Loop"));
    let (s, _) = snapshot::build(d.project(), RATE, &HashMap::from([(a, 7)]));
    let cs = &s.tracks[0].clips[0];
    assert_eq!((cs.length, cs.clip_loop), (8 * 48_000, Some((48_000, 2 * 48_000))));
    // Warped, the loop stays in content frames; the map keeps the content's slope.
    let mut w = warp(&d, c);
    w.enabled = true;
    set(&mut d, c, w, "warp");
    let (s, _) = snapshot::build_with_transients(d.project(), RATE, &HashMap::from([(a, 7)]), &HashMap::new());
    let cs = &s.tracks[0].clips[0];
    let pts = &cs.warp.as_ref().unwrap().points;
    assert_eq!((cs.length, cs.clip_loop), (8 * 48_000, Some((48_000, 2 * 48_000))));
    assert_eq!(pts.last().unwrap().0 - pts[0].0, 4 * 48_000);
    // Off: plays the content once; undo removes the loop.
    d.execute(Command::SetClipLoop { clip: c, clip_loop: Some(AudioLoop { on: false, ..l }) }, Origin::User).unwrap();
    let (s, _) = snapshot::build(d.project(), RATE, &HashMap::from([(a, 7)]));
    assert_eq!((s.tracks[0].clips[0].length, s.tracks[0].clips[0].clip_loop), (4 * 48_000, None));
    assert!(d.execute(Command::SetClipLoop { clip: c, clip_loop: Some(AudioLoop { end: 0, ..l }) }, Origin::User).is_err());
}
