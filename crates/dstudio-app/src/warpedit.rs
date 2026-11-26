//! Audio clip warping in the clip editor: the Speed / Warp / Mode menus, the
//! Original BPM drag, warp markers and transients (the handle band's lower row),
//! and the onset analysis that feeds Drums / Auto slicing.
//!
//! Every edit is one `Command::SetClipWarp` named as an undo step;
//! drags commit live and coalesce into one step per gesture kind.

use crate::controller::{Controller, JobResult, MenuAction};
use crate::peaks::{L0_FRAMES, Pyramid};
use crate::pianoroll::{GRID_Y, PrView};
use dstudio_core::warp::{ClipWarp, SPEED_PRESETS, Transient, WarpMode};
use dstudio_core::{AssetId, Clip, ClipId, Command};
use std::collections::HashMap;
use std::time::{Duration, Instant};

const DOUBLE_CLICK: Duration = Duration::from_millis(350);
/// Warp markers sit in the lower row of the handle band (the upper row holds the clip's start
/// and end flags).
pub const MARKER_ROW: (f32, f32) = (40.5, GRID_Y);
const MARKER_HIT: f32 = 6.0;
/// Reach of a transient line in the lane, and of a transient tick in the marker row.
const TRANSIENT_HIT: f32 = 4.0;
const TRANSIENT_SNAP: f32 = 8.0;
/// Undo label of a marker placed on a transient (and its drag).
const PLACE_LABEL: &str = "Add warp marker at transient";

#[derive(Debug, Clone)]
pub struct AcDrag {
    /// The view when the gesture started (the clip length changes under a marker drag).
    pub view: PrView,
    pub clip: ClipId,
    pub warp: ClipWarp,
    pub kind: AcDragKind,
    pub moved: bool,
}

#[derive(Debug, Clone, Copy)]
pub enum AcDragKind {
    /// `slide` (Shift): the marker keeps its beat and the audio moves under it.
    /// `placed`: the press put it on a transient, so releasing in place keeps it.
    Marker {
        source: u64,
        slide: bool,
        placed: bool,
    },
    /// Press on a transient: the first move pins it (`neighbours`: and the transients either
    /// side) with a warp marker and drags that marker.
    Pin {
        source: u64,
        neighbours: bool,
    },
    /// A transient line in the lane, dragged from `from` to `to` (committed on release).
    Transient {
        from: u64,
        to: u64,
    },
    Bpm {
        start: f64,
    },
    Transpose {
        start: i32,
    },
}

/// Audio clip editor fields that take typed values (double-click).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AcEdit {
    OriginalBpm,
    Transpose,
}

/// Semitones per point of Transpose drag (48 st over 200 pt).
const TRANSPOSE_PER_PT: f32 = 48.0 / 200.0;

/// "0 st", "+3 st", "-1.5 st" (whole semitones bare, others to one decimal).
pub fn format_semitones(st: f64) -> String {
    let st = (st * 10.0).round() / 10.0;
    if st == 0.0 {
        return "0 st".into();
    }
    let sign = if st > 0.0 { "+" } else { "" };
    if st.fract() == 0.0 { format!("{sign}{st:.0} st") } else { format!("{sign}{st:.1} st") }
}

/// "2x", "0.75x", "1.33x" (the speed format).
pub fn format_speed(speed: f64) -> String {
    let s = format!("{:.2}", (speed * 100.0).round() / 100.0);
    format!("{}x", s.trim_end_matches('0').trim_end_matches('.'))
}

/// Conversions between clip-relative beats and source frames of one clip
/// (mirrored when the clip plays reversed).
struct Map<'a> {
    w: &'a ClipWarp,
    rate: f64,
    first: f64,
    /// Reversed clips: their length in beats.
    mirror: Option<f64>,
}

impl<'a> Map<'a> {
    fn new(c: &'a Clip, w: &'a ClipWarp, rate: f64) -> Self {
        let first = w.beat_at(c.source_offset as f64, rate);
        let mirror = w.reverse.then(|| (w.beat_at((c.source_offset + c.source_length) as f64, rate) - first) / w.speed);
        Map { w, rate, first, mirror }
    }
    fn flip(&self, clip_beat: f64) -> f64 {
        self.mirror.map_or(clip_beat, |len| len - clip_beat)
    }
    fn clip_beat(&self, source: f64) -> f64 {
        self.flip((self.w.beat_at(source, self.rate) - self.first) / self.w.speed)
    }
    fn original_beat(&self, clip_beat: f64) -> f64 {
        self.first + self.flip(clip_beat) * self.w.speed
    }
    fn source(&self, clip_beat: f64) -> f64 {
        self.w.source_at(self.original_beat(clip_beat), self.rate)
    }
}

/// The clip's warp with markers pinning the window's source start and end where they play
/// now, where no marker sits there yet: a warped clip always shows those two, and the first
/// marker edit makes them real. Pins on the current mapping leave playback unchanged.
fn anchored(c: &Clip, rate: f64) -> ClipWarp {
    let mut w = c.warp.clone();
    for s in [c.source_offset, c.source_offset + c.source_length] {
        if !w.markers.iter().any(|k| k.source == s) {
            w.set_marker(s, c.warp.beat_at(s as f64, rate));
        }
    }
    w
}

/// Shift-drag: slide the audio under the marker at `source` to `to`. A
/// reversed clip starts at its window's source end, so that end is pinned
/// first (also when a trim left markers past it) and the slide stays inside
/// the window; otherwise it would move the clip's start and length.
fn slide(c: &Clip, w: &ClipWarp, source: u64, to: u64, rate: f64) -> Option<ClipWarp> {
    let mut w = w.clone();
    let end = c.source_offset + c.source_length;
    let mut to = to;
    if w.reverse {
        if !w.markers.iter().any(|k| k.source == end) {
            w.set_marker(end, w.beat_at(end as f64, rate));
        }
        if source < end {
            to = to.min(end - 1);
        }
    }
    w.slide_marker(source, to).then_some(w)
}

impl Controller {
    fn asset_rate(&self, asset: AssetId) -> f64 {
        self.project().asset(asset).map_or(48_000.0, |a| a.sample_rate)
    }

    /// Make the engine's reversed copy (and its peaks) of every asset a
    /// reversed clip plays. A copy is a few ms per minute of audio.
    pub fn ensure_reversed_assets(&mut self) {
        let wanted: Vec<(AssetId, u64)> = self
            .project()
            .tracks
            .iter()
            .flat_map(|t| &t.clips)
            .filter(|c| c.warp.reverse && !self.reversed_assets.contains_key(&c.asset))
            .filter_map(|c| Some((c.asset, *self.asset_map.get(&c.asset)?)))
            .collect();
        for (asset, ea) in wanted {
            if self.reversed_assets.contains_key(&asset) {
                continue;
            }
            match self.engine.reverse_asset(ea) {
                Ok(info) => {
                    let pairs = (info.frames / L0_FRAMES as u64 + 1) as usize;
                    if let Ok(l0) = self.engine.peaks(info.asset_id, 0, L0_FRAMES, pairs) {
                        self.peaks.insert(info.asset_id, Pyramid::from_level0(l0));
                    }
                    self.reversed_assets.insert(asset, info.asset_id);
                }
                Err(e) => self.set_status(format!("Reverse failed: {e}"), 6),
            }
        }
    }

    /// Reverse toggle in the clip editor toolbar (one undo step).
    pub fn ac_reverse_toggle(&mut self) {
        let Some(c) = self.ac_clip().cloned() else { return };
        self.set_clip_warp(c.id, ClipWarp { reverse: !c.warp.reverse, ..c.warp.clone() }, "Toggle clip reverse");
    }

    /// Detected transients of a model asset (empty until analysed).
    pub fn detected_transients(&self, asset: AssetId) -> &[Transient] {
        self.asset_map.get(&asset).and_then(|ea| self.onsets.get(ea)).map_or(&[], |v| v.as_slice())
    }

    /// Detected transients of every analysed asset, for the snapshot.
    pub fn transients_by_asset(&self) -> HashMap<AssetId, Vec<Transient>> {
        self.asset_map.iter().filter_map(|(a, ea)| self.onsets.get(ea).map(|v| (*a, v.clone()))).collect()
    }

    /// Analyse the assets of warped clips and of the clip open in the editor
    /// (its tempo) that have no onsets yet (worker thread).
    pub fn request_onsets(&mut self) {
        let open = self.ac_clip().map(|c| c.id);
        let wanted: Vec<u64> = self
            .project()
            .tracks
            .iter()
            .flat_map(|t| &t.clips)
            .filter(|c| c.warp.enabled || Some(c.id) == open)
            .filter_map(|c| self.asset_map.get(&c.asset).copied())
            .filter(|ea| !self.onsets.contains_key(ea) && !self.onsets_pending.contains(ea))
            .collect();
        for ea in wanted {
            self.onsets_pending.insert(ea);
            self.pending_jobs += 1;
            let (engine, tx, rate) = (self.engine.clone(), self.jobs_tx.clone(), self.rate);
            let _ = std::thread::Builder::new().name("onsets".into()).spawn(move || {
                let result = engine.detect_onsets(ea).map_err(|e| e.to_string());
                // Tempo from the same onsets (engine frames), here off the UI thread: ms per minute.
                let tempo = result.as_ref().ok().and_then(|v| {
                    let tr: Vec<Transient> = v
                        .iter()
                        .map(|o| Transient { source: o.frame.max(0) as u64, strength: o.strength, percussive: o.percussive })
                        .collect();
                    dstudio_core::warp::estimate_bpm(&tr, rate)
                });
                let _ = tx.send(JobResult::Onsets { engine_asset: ea, result, tempo });
            });
        }
    }

    pub fn finish_onsets(&mut self, engine_asset: u64, result: Result<Vec<dstudio_engine::Onset>, String>, tempo: Option<f64>) {
        self.onsets_pending.remove(&engine_asset);
        let Some(asset) = self.asset_map.iter().find(|(_, ea)| **ea == engine_asset).map(|(a, _)| *a) else { return };
        let k = self.asset_rate(asset) / self.rate;
        let list = match result {
            Ok(v) => v
                .iter()
                .map(|o| Transient { source: (o.frame as f64 * k).round().max(0.0) as u64, strength: o.strength, percussive: o.percussive })
                .collect(),
            Err(e) => {
                self.set_status(format!("Transient detection failed: {e}"), 4);
                Vec::new()
            }
        };
        self.tempos.insert(engine_asset, tempo);
        self.onsets.insert(engine_asset, list);
        self.publish();
        self.ui_dirty = true;
    }

    pub(crate) fn set_clip_warp(&mut self, clip: ClipId, warp: ClipWarp, label: &str) -> bool {
        if self.project().clip(clip).is_none_or(|c| c.warp == warp) {
            return false;
        }
        self.execute(Command::SetClipWarp { clip, warp, label: label.into() })
    }

    /// Tempo detected in a clip's audio, once its transients are analysed.
    pub fn detected_bpm(&self, asset: AssetId) -> Option<f64> {
        self.asset_map.get(&asset).and_then(|ea| self.tempos.get(ea).copied().flatten())
    }

    /// Original BPM of a clip whose tempo was never set: the detected tempo,
    /// else the project tempo.
    fn default_original_bpm(&self, c: &Clip) -> f64 {
        self.detected_bpm(c.asset).unwrap_or(self.project().bpm_at(c.start))
    }

    /// The open audio clip's warp, switched on: the
    /// first time at the clip's own tempo (`default_original_bpm`).
    fn warp_on(&self, c: &Clip) -> ClipWarp {
        let mut w = c.warp.clone();
        if !w.enabled && w.tempo_is_default() {
            w.original_bpm = self.default_original_bpm(c);
        }
        w.enabled = true;
        w
    }

    /// Speed menu: a preset turns warp on.
    pub fn warp_pick_speed(&mut self, index: usize) {
        let (Some(c), Some(&speed)) = (self.ac_clip().cloned(), SPEED_PRESETS.get(index)) else { return };
        let w = ClipWarp { speed, ..self.warp_on(&c) };
        self.set_clip_warp(c.id, w, "Set clip speed");
    }

    /// Warp menu. Turning warp off with a speed other than 1x also resets the speed.
    pub fn warp_pick_enabled(&mut self, on: bool) {
        let Some(c) = self.ac_clip().cloned() else { return };
        if on {
            let w = self.warp_on(&c);
            self.set_clip_warp(c.id, w, "Set clip warp mode");
            return;
        }
        let reset = (c.warp.speed - 1.0).abs() > 0.001;
        let w = ClipWarp { enabled: false, speed: 1.0, ..c.warp.clone() };
        self.set_clip_warp(c.id, w, if reset { "Reset clip speed" } else { "Set clip warp mode" });
    }

    /// Mode menu (only with warp on).
    pub fn warp_pick_mode(&mut self, index: usize) {
        let (Some(c), Some(&mode)) = (self.ac_clip().cloned(), WarpMode::ALL.get(index)) else { return };
        if !c.warp.enabled {
            return;
        }
        self.set_clip_warp(c.id, ClipWarp { mode, ..c.warp.clone() }, "Set clip mode");
    }

    /// Transpose can be edited except in Re-pitch, where the pitch follows the speed.
    pub fn ac_transpose_editable(c: &Clip) -> bool {
        !(c.warp.enabled && c.warp.mode == WarpMode::Repitch)
    }

    /// Transpose readout: the clip's semitones, or in Re-pitch the shift its tempo ratio makes.
    pub fn ac_transpose_text(&self, c: &Clip) -> String {
        if Self::ac_transpose_editable(c) {
            return format_semitones(c.warp.transpose as f64);
        }
        format_semitones(12.0 * (self.project().bpm_at(c.start) * c.warp.speed / c.warp.original_bpm).log2())
    }

    /// Transpose drag (phase 0 press, 1 move, 2 release): right or up raises,
    /// 48 st per 200 pt, clamped to -24..=24, one undo step.
    pub fn ac_transpose_drag(&mut self, phase: i32, dx: f32, dy: f32) {
        let Some(c) = self.ac_clip().cloned() else { return };
        self.ui_dirty = true;
        match phase {
            0 if Self::ac_transpose_editable(&c) => {
                self.doc.seal_coalescing();
                let start = c.warp.transpose;
                self.pr.ac_drag = Some(AcDrag {
                    view: self.pr_view(),
                    clip: c.id,
                    warp: c.warp.clone(),
                    kind: AcDragKind::Transpose { start },
                    moved: false,
                });
            }
            1 => {
                let Some(d) = self.pr.ac_drag.as_mut() else { return };
                let AcDragKind::Transpose { start } = d.kind else { return };
                let st = (start + ((dx - dy) * TRANSPOSE_PER_PT).round() as i32)
                    .clamp(-dstudio_core::warp::MAX_TRANSPOSE, dstudio_core::warp::MAX_TRANSPOSE);
                if st == d.warp.transpose {
                    return;
                }
                d.moved = true;
                d.warp.transpose = st;
                let (clip, w) = (d.clip, d.warp.clone());
                self.set_clip_warp(clip, w, "Adjust clip transpose");
            }
            _ => {
                if matches!(self.pr.ac_drag, Some(AcDrag { kind: AcDragKind::Transpose { .. }, .. })) {
                    self.pr.ac_drag = None;
                }
            }
        }
    }

    /// Original BPM shown in the toolbar (the project tempo until it is set).
    pub fn ac_original_bpm(&self, c: &Clip) -> f64 {
        if let Some(AcDrag { kind: AcDragKind::Bpm { .. }, warp, .. }) = &self.pr.ac_drag {
            return warp.original_bpm;
        }
        if c.warp.tempo_is_default() { self.default_original_bpm(c) } else { c.warp.original_bpm }
    }

    /// Double-click on Original BPM or Transpose: type a value.
    pub fn ac_edit_start(&mut self, field: AcEdit) {
        let Some(c) = self.ac_clip() else { return };
        if field == AcEdit::Transpose && !Self::ac_transpose_editable(c) {
            return;
        }
        self.pr.ac_edit = Some((field, c.id));
        self.ui_dirty = true;
    }

    /// Text the edit field starts with: the shown value.
    pub fn ac_edit_initial(&self, c: &Clip) -> String {
        match self.pr.ac_edit.map(|e| e.0) {
            Some(AcEdit::OriginalBpm) => format!("{}", self.ac_original_bpm(c).round()),
            Some(AcEdit::Transpose) => c.warp.transpose.to_string(),
            None => String::new(),
        }
    }

    pub fn ac_edit_cancel(&mut self) {
        self.pr.ac_edit = None;
        self.ui_dirty = true;
    }

    /// Enter or focus loss. Original BPM: 20..300, two decimals; it turns
    /// warp on, and an empty (or unreadable) value turns warp off ("Clear
    /// original BPM"). The shown BPM is rounded, so committing it unchanged on a
    /// warped clip keeps the stored value. Transpose: whole semitones, -24..24;
    /// unreadable is ignored.
    pub fn ac_edit_commit(&mut self, text: &str) {
        let unchanged = self.ac_clip().is_some_and(|c| text.trim() == self.ac_edit_initial(c));
        let (Some((field, id)), Some(c)) = (self.pr.ac_edit.take(), self.ac_clip().cloned()) else { return };
        self.ui_dirty = true;
        if c.id != id {
            return;
        }
        let v: f64 = text.trim().parse().unwrap_or(f64::NAN);
        match field {
            AcEdit::OriginalBpm if !v.is_finite() => {
                if c.warp.enabled {
                    self.set_clip_warp(c.id, ClipWarp { enabled: false, speed: 1.0, ..c.warp.clone() }, "Clear original BPM");
                }
            }
            AcEdit::OriginalBpm if unchanged && c.warp.enabled => {}
            AcEdit::OriginalBpm => {
                let bpm = ((v * 100.0).round() / 100.0).clamp(dstudio_core::warp::MIN_ORIGINAL_BPM, dstudio_core::warp::MAX_ORIGINAL_BPM);
                self.set_clip_warp(c.id, ClipWarp { original_bpm: bpm, ..self.warp_on(&c) }, "Set original BPM");
            }
            AcEdit::Transpose if v.is_finite() && Self::ac_transpose_editable(&c) => {
                let st = (v.round() as i32).clamp(-dstudio_core::warp::MAX_TRANSPOSE, dstudio_core::warp::MAX_TRANSPOSE);
                self.set_clip_warp(c.id, ClipWarp { transpose: st, ..c.warp.clone() }, "Set clip pitch shift");
            }
            AcEdit::Transpose => {}
        }
    }

    /// Original BPM drag (phase 0 press, 1 move, 2 release): one BPM per point,
    /// right or up, clamped to 20..300, one undo step.
    pub fn ac_bpm_drag(&mut self, phase: i32, dx: f32, dy: f32) {
        let Some(c) = self.ac_clip().cloned() else { return };
        self.ui_dirty = true;
        match phase {
            0 => {
                self.doc.seal_coalescing();
                let start = self.ac_original_bpm(&c).round();
                self.pr.ac_drag =
                    Some(AcDrag { view: self.pr_view(), clip: c.id, warp: c.warp.clone(), kind: AcDragKind::Bpm { start }, moved: false });
            }
            1 => {
                let Some(d) = self.pr.ac_drag.as_mut() else { return };
                let AcDragKind::Bpm { start } = d.kind else { return };
                let acc = dx - dy;
                if !d.moved && acc.abs() <= 2.0 {
                    return;
                }
                d.moved = true;
                d.warp.original_bpm =
                    (start + acc.round() as f64).clamp(dstudio_core::warp::MIN_ORIGINAL_BPM, dstudio_core::warp::MAX_ORIGINAL_BPM);
                let (clip, w) = (d.clip, d.warp.clone());
                self.set_clip_warp(clip, w, "Set original BPM");
            }
            _ => {
                if matches!(self.pr.ac_drag, Some(AcDrag { kind: AcDragKind::Bpm { .. }, .. })) {
                    self.pr.ac_drag = None;
                }
            }
        }
    }

    /// Clip-relative beat of every source frame in `sources` as the editor shows it: through the
    /// warp map with warp on, 1:1 across the clip's content with warp off (mirrored when reversed).
    fn editor_beats(&self, c: &Clip, sources: impl Iterator<Item = u64>) -> Vec<f64> {
        let rate = self.asset_rate(c.asset);
        if c.warp.enabled {
            let m = Map::new(c, &c.warp, rate);
            return sources.map(|s| m.clip_beat(s as f64)).collect();
        }
        let len = self.project().content_length_ticks(c) as f64 / dstudio_core::PPQ as f64;
        let n = c.source_length.max(1) as f64;
        sources
            .map(|s| {
                let t = (s as f64 - c.source_offset as f64) / n;
                if c.warp.reverse { (1.0 - t) * len } else { t * len }
            })
            .collect()
    }

    /// Warp markers of the open audio clip, as shown: a clip without markers shows the two that
    /// pin its start and its end (the first edit makes them real).
    fn shown_markers(&self, c: &Clip) -> Vec<u64> {
        let rate = self.asset_rate(c.asset);
        anchored(c, rate).markers.iter().map(|k| k.source).collect()
    }

    /// Warp markers of the open audio clip (clip-relative beats). Shown with warp off too
    /// (dimmed, not editable).
    pub fn ac_marker_beats(&self) -> Vec<f64> {
        let Some(c) = self.ac_clip() else { return Vec::new() };
        let shown = self.shown_markers(c);
        self.editor_beats(c, shown.into_iter())
    }

    /// The marker under the pointer, or the one being dragged: its index in `ac_marker_beats`.
    pub fn ac_marker_hover(&self) -> Option<usize> {
        let c = self.ac_clip()?;
        if !c.warp.enabled {
            return None;
        }
        let source = match &self.pr.ac_drag {
            Some(AcDrag { kind: AcDragKind::Marker { source, .. }, .. }) => *source,
            Some(_) => return None,
            None => self.pr.ac_hover?,
        };
        self.shown_markers(c).iter().position(|&s| s == source)
    }

    /// Transients of the open audio clip (clip-relative beats), inside the clip's window.
    pub fn ac_transient_beats(&self) -> Vec<f64> {
        let Some(c) = self.ac_clip() else { return Vec::new() };
        let rate = self.asset_rate(c.asset);
        let end = c.source_offset + c.source_length;
        let (from, to) = match self.pr.ac_drag {
            Some(AcDrag { kind: AcDragKind::Transient { from, to }, .. }) => (Some(from), to),
            _ => (None, 0),
        };
        let tr: Vec<u64> = c
            .warp
            .transients(self.detected_transients(c.asset), rate)
            .iter()
            .map(|t| if Some(t.source) == from { to } else { t.source })
            .filter(|&s| s >= c.source_offset && s < end)
            .collect();
        self.editor_beats(c, tr.into_iter())
    }

    /// A press in the lane on a warped clip: on a transient line, a double-click removes the
    /// transient and a press starts dragging it; elsewhere a double-click adds one. Returns false
    /// when the press is left to move the playhead.
    fn lane_transient_press(&mut self, c: &Clip, view: PrView, x: f32, double: bool) -> bool {
        let rate = self.asset_rate(c.asset);
        let m = Map::new(c, &c.warp, rate);
        let detected = self.detected_transients(c.asset).to_vec();
        let dist = |s: u64| (view.x_of(m.clip_beat(s as f64)) - x).abs();
        let hit = c
            .warp
            .transients(&detected, rate)
            .iter()
            .map(|t| t.source)
            .filter(|&s| dist(s) <= TRANSIENT_HIT)
            .min_by(|&a, &b| dist(a).total_cmp(&dist(b)));
        let mut w = c.warp.clone();
        match (double, hit) {
            (true, Some(s)) => {
                if w.remove_transient(&detected, s, rate) {
                    self.set_clip_warp(c.id, w, "Remove transient");
                }
            }
            (true, None) => {
                let s = m.source(view.beat_at(x)).round().max(0.0) as u64;
                if w.add_transient(&detected, s, rate) {
                    self.set_clip_warp(c.id, w, "Add transient");
                }
            }
            (false, Some(s)) => {
                self.pr.ac_drag = Some(AcDrag { view, clip: c.id, warp: w, kind: AcDragKind::Transient { from: s, to: s }, moved: false });
            }
            (false, None) => return false,
        }
        true
    }

    /// The shown marker nearest `x`, within `MARKER_HIT`.
    fn marker_at(&self, c: &Clip, view: &PrView, x: f32) -> Option<u64> {
        let shown = self.shown_markers(c);
        let beats = self.editor_beats(c, shown.iter().copied());
        shown
            .iter()
            .zip(beats)
            .map(|(&s, b)| (s, (view.x_of(b) - x).abs()))
            .filter(|&(_, d)| d <= MARKER_HIT)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(s, _)| s)
    }

    /// Remove the marker at `source` (a clip without markers first gets its two anchors).
    fn remove_marker_at(&mut self, c: &Clip, source: u64) {
        let mut w = anchored(c, self.asset_rate(c.asset));
        if let Some(i) = w.markers.iter().position(|k| k.source == source)
            && w.remove_marker(i)
        {
            self.set_clip_warp(c.id, w, "Remove warp marker");
        }
    }

    /// The transient nearest `x` within `TRANSIENT_HIT` (warp map positions).
    fn transient_at(&self, c: &Clip, view: &PrView, x: f32) -> Option<u64> {
        let rate = self.asset_rate(c.asset);
        let m = Map::new(c, &c.warp, rate);
        let dist = |s: u64| (view.x_of(m.clip_beat(s as f64)) - x).abs();
        let end = c.source_offset + c.source_length;
        c.warp
            .transients(self.detected_transients(c.asset), rate)
            .iter()
            .map(|t| t.source)
            .filter(|&s| s >= c.source_offset && s < end && dist(s) <= TRANSIENT_HIT)
            .min_by(|&a, &b| dist(a).total_cmp(&dist(b)))
    }

    /// The ghost marker over a hovered transient tick (clip-relative beat).
    pub fn ac_ghost_beat(&self) -> Option<f64> {
        let c = self.ac_clip()?;
        let s = self.pr.ac_ghost.filter(|_| c.warp.enabled && self.pr.ac_drag.is_none())?;
        self.editor_beats(c, std::iter::once(s)).first().copied()
    }

    /// Right-click in the audio clip editor at (x, y), window point (wx, wy). On a warp marker
    /// it removes the marker. Elsewhere in the marker row or the lane of a warped clip it opens
    /// the transient menu: Insert Transient at the pointer, Delete Transient on one.
    pub fn ac_context(&mut self, x: f32, y: f32, wx: f32, wy: f32, win: (f32, f32)) {
        let Some(c) = self.ac_clip().cloned() else { return };
        self.menu_close();
        self.doc.seal_coalescing();
        self.ui_dirty = true;
        if !c.warp.enabled || y < MARKER_ROW.0 {
            return;
        }
        let view = self.pr_view();
        if y < MARKER_ROW.1
            && let Some(s) = self.marker_at(&c, &view, x)
        {
            self.pr.ac_hover = None;
            self.remove_marker_at(&c, s);
            return;
        }
        let rate = self.asset_rate(c.asset);
        let at = Map::new(&c, &c.warp, rate).source(view.beat_at(x)).round().max(0.0) as u64;
        let hit = self.transient_at(&c, &view, x);
        let inside = at >= c.source_offset && at < c.source_offset + c.source_length;
        self.open_menu(
            vec![(MenuAction::InsertTransient, hit.is_none() && inside), (MenuAction::DeleteTransient, hit.is_some())],
            wx,
            wy,
            win,
        );
        self.menu.ac_transient = Some((c.id, hit.unwrap_or(at)));
    }

    /// The transient menu's entries: insert one at `source`, or delete the one there.
    pub fn ac_transient_menu_activate(&mut self, clip: ClipId, source: u64, action: MenuAction) {
        self.menu_close();
        let Some(c) = self.ac_clip().filter(|c| c.id == clip).cloned() else { return };
        let rate = self.asset_rate(c.asset);
        let detected = self.detected_transients(c.asset).to_vec();
        let mut w = c.warp.clone();
        let label = match action {
            MenuAction::InsertTransient if w.add_transient(&detected, source, rate) => "Add transient",
            MenuAction::DeleteTransient if w.remove_transient(&detected, source, rate) => "Remove transient",
            _ => return,
        };
        self.set_clip_warp(c.id, w, label);
    }

    /// Pointer in the audio clip editor. `kind`: 0 press, 1 move (pressed), 2 release, 3 hover,
    /// 4 the pointer left. Right-clicks go to `ac_context`.
    ///
    /// The handle band's lower row holds the warp markers (warp on): double-click adds one (on a
    /// transient when one is under the pointer), a drag moves one between its neighbours, a
    /// double-click or a right-click removes one. A press on a transient (a ghost marker shows on
    /// hover) and a drag places a marker
    /// on the transient and drags it (one undo step); a click alone changes nothing. In the lane
    /// (warp on) a transient line drags to move the transient, and a double-click adds or
    /// removes one. Ruler, loop bar (outside the loop), the upper handle row and the rest of
    /// the lane move the playhead.
    /// Modifiers: `shift` slides the audio under a dragged marker, `alt` ignores transients,
    /// `command` (Cmd or Ctrl) on a transient also pins its two neighbours.
    pub fn ac_pointer(&mut self, kind: i32, x: f32, y: f32, shift: bool, alt: bool, command: bool) {
        let Some(c) = self.ac_clip().cloned() else { return };
        let rate = self.asset_rate(c.asset);
        let in_row = (MARKER_ROW.0..MARKER_ROW.1).contains(&y);
        match kind {
            3 | 4 => {
                if self.pr.ac_drag.is_some() {
                    return;
                }
                let view = self.pr_view();
                let hover = if kind == 3 && in_row && c.warp.enabled { self.marker_at(&c, &view, x) } else { None };
                // Over a transient tick (no marker there): a ghost marker shows what a drag would place.
                let ghost = if kind == 3 && in_row && c.warp.enabled && hover.is_none() { self.transient_at(&c, &view, x) } else { None };
                if (hover, ghost) != (self.pr.ac_hover, self.pr.ac_ghost) {
                    self.pr.ac_hover = hover;
                    self.pr.ac_ghost = ghost;
                    self.ui_dirty = true;
                }
            }
            0 => {
                self.ui_dirty = true;
                self.menu_close();
                // Each press is its own undo step; only its drag coalesces.
                self.doc.seal_coalescing();
                let now = Instant::now();
                let double =
                    self.pr.ac_last_down.is_some_and(|(t, lx, ly)| now - t < DOUBLE_CLICK && (lx - x).abs() < 4.0 && (ly - y).abs() < 4.0);
                self.pr.ac_last_down = Some((now, x, y));
                self.pr.ac_ghost = None;
                let view = self.pr_view();
                if self.pr_loop_press(&view, x, y, (view.beat_at(x) * dstudio_core::PPQ as f64).round() as dstudio_core::Ticks) {
                    return;
                }
                if y >= GRID_Y && c.warp.enabled && self.lane_transient_press(&c, view, x, double) {
                    return;
                }
                if !in_row {
                    self.seek_beats((c.start as f64 / dstudio_core::PPQ as f64) + view.beat_at(x).max(0.0));
                    return;
                }
                if !c.warp.enabled {
                    return;
                }
                if let Some(s) = self.marker_at(&c, &view, x) {
                    if double {
                        self.pr.ac_hover = None;
                        self.remove_marker_at(&c, s);
                    } else {
                        let warp = anchored(&c, rate);
                        self.pr.ac_drag = Some(AcDrag {
                            view,
                            clip: c.id,
                            warp,
                            kind: AcDragKind::Marker { source: s, slide: shift, placed: false },
                            moved: false,
                        });
                    }
                    return;
                }
                let m = Map::new(&c, &c.warp, rate);
                let tr: Vec<u64> = c.warp.transients(self.detected_transients(c.asset), rate).iter().map(|t| t.source).collect();
                let dist = |s: u64| (view.x_of(m.clip_beat(s as f64)) - x).abs();
                let hit = if alt {
                    None
                } else {
                    (0..tr.len()).filter(|&i| dist(tr[i]) <= TRANSIENT_SNAP).min_by(|&a, &b| dist(tr[a]).total_cmp(&dist(tr[b])))
                };
                let window = c.source_offset..=c.source_offset + c.source_length;
                match (hit, double) {
                    (Some(i), false) => {
                        self.pr.ac_drag = Some(AcDrag {
                            view,
                            clip: c.id,
                            warp: c.warp.clone(),
                            kind: AcDragKind::Pin { source: tr[i], neighbours: command },
                            moved: false,
                        });
                    }
                    (Some(i), true) => {
                        let s = tr[i];
                        let mut w = anchored(&c, rate);
                        if window.contains(&s) && w.set_marker(s, c.warp.beat_at(s as f64, rate)) {
                            self.set_clip_warp(c.id, w, PLACE_LABEL);
                        }
                    }
                    (None, true) => {
                        let s = m.source(view.beat_at(x)).round().max(0.0) as u64;
                        let mut w = anchored(&c, rate);
                        if window.contains(&s) && w.set_marker(s, c.warp.beat_at(s as f64, rate)) {
                            self.set_clip_warp(c.id, w, "Add warp marker");
                        }
                    }
                    (None, false) => {}
                }
            }
            1 => {
                if matches!(self.pr.drag, Some(crate::pianoroll::PrDrag::Loop { .. })) {
                    self.ui_dirty = true;
                    let raw = (self.pr_view().beat_at(x) * dstudio_core::PPQ as f64).round() as dstudio_core::Ticks;
                    self.pr_loop_move(raw);
                    return;
                }
                let Some(d) = self.pr.ac_drag.as_mut() else { return };
                self.ui_dirty = true;
                // A drag from a transient: pin it where it plays now (Cmd: with its neighbours)
                // and drag that marker on.
                if let AcDragKind::Pin { source, neighbours } = d.kind {
                    let tr: Vec<u64> = c.warp.transients(self.detected_transients(c.asset), rate).iter().map(|t| t.source).collect();
                    let Some(i) = tr.iter().position(|&t| t == source) else { return };
                    let mut w = anchored(&c, rate);
                    let inside = |s: u64| s > c.source_offset && s < c.source_offset + c.source_length;
                    let picks = if neighbours { &tr[i.saturating_sub(1)..(i + 2).min(tr.len())] } else { &tr[i..=i] };
                    for &p in picks.iter().filter(|&&p| inside(p)) {
                        w.set_marker(p, c.warp.beat_at(p as f64, rate));
                    }
                    let Some(d) = self.pr.ac_drag.as_mut() else { return };
                    if !w.markers.iter().any(|k| k.source == source) {
                        self.pr.ac_drag = None;
                        return;
                    }
                    d.warp = w;
                    d.kind = AcDragKind::Marker { source, slide: false, placed: true };
                }
                let Some(d) = self.pr.ac_drag.as_mut() else { return };
                let m = Map::new(&c, &d.warp, rate);
                let beat = d.view.beat_at(x);
                match d.kind {
                    AcDragKind::Marker { source, slide: true, .. } => {
                        // The audio moves at the local rate of the map, the marker stays put.
                        let b0 = m.clip_beat(source as f64);
                        let slope = (m.source(b0) - m.source(b0 - 1e-3)) / 1e-3;
                        let lo = d
                            .warp
                            .markers
                            .iter()
                            .rev()
                            .find(|k| k.source < source)
                            .map_or(c.source_offset as f64, |k| k.source as f64 + 1.0);
                        let hi = d
                            .warp
                            .markers
                            .iter()
                            .find(|k| k.source > source)
                            .map_or((c.source_offset + c.source_length) as f64, |k| k.source as f64 - 1.0);
                        let to = (source as f64 + (beat - b0) * slope).clamp(lo, hi.max(lo)).round() as u64;
                        let Some(w) = slide(&c, &d.warp, source, to, rate) else { return };
                        d.moved = true;
                        let clip = d.clip;
                        self.set_clip_warp(clip, w, "Move warp marker");
                    }
                    AcDragKind::Marker { source, slide: false, placed } => {
                        let mut w = d.warp.clone();
                        let Some(beat) = clamp_between_neighbours(&w, source, m.original_beat(beat)) else { return };
                        if !w.set_marker(source, beat) {
                            return;
                        }
                        d.moved = true;
                        let clip = d.clip;
                        self.set_clip_warp(clip, w, if placed { PLACE_LABEL } else { "Move warp marker" });
                    }
                    AcDragKind::Transient { from, .. } => {
                        let to =
                            m.source(beat).round().clamp(c.source_offset as f64, (c.source_offset + c.source_length - 1) as f64) as u64;
                        d.moved = d.moved || to != from;
                        d.kind = AcDragKind::Transient { from, to };
                    }
                    AcDragKind::Pin { .. } | AcDragKind::Bpm { .. } | AcDragKind::Transpose { .. } => {}
                }
            }
            _ => {
                self.ui_dirty = true;
                if let Some(crate::pianoroll::PrDrag::Loop { orig, cur, .. }) = self.pr.drag.take() {
                    if let Some(l) = c.clip_loop.filter(|_| orig != cur) {
                        self.execute(Command::SetClipLoop {
                            clip: c.id,
                            clip_loop: Some(dstudio_core::model::AudioLoop { start: cur.start, end: cur.end, ..l }),
                        });
                    }
                    return;
                }
                // A click without a drag changes nothing: markers are removed by double- or right-click.
                match self.pr.ac_drag.take() {
                    Some(AcDrag { kind: AcDragKind::Marker { source, .. }, .. }) => self.pr.ac_hover = Some(source),
                    Some(AcDrag { kind: AcDragKind::Transient { from, to }, moved: true, clip, warp, .. }) => {
                        let detected = self.detected_transients(c.asset).to_vec();
                        let mut w = warp;
                        if w.move_transient(&detected, from, to, rate) {
                            self.set_clip_warp(clip, w, "Move transient");
                        }
                    }
                    _ => {}
                }
            }
        }
    }
}

/// `beat` kept strictly between the beats of the markers either side of `source` in `w`, so a
/// dragged marker stops at its neighbours. `None` when there is no room left between them.
fn clamp_between_neighbours(w: &ClipWarp, source: u64, beat: f64) -> Option<f64> {
    const GAP: f64 = 1e-3;
    let lo = w.markers.iter().rev().find(|k| k.source < source).map_or(f64::NEG_INFINITY, |k| k.beat + GAP);
    let hi = w.markers.iter().find(|k| k.source > source).map_or(f64::INFINITY, |k| k.beat - GAP);
    (lo <= hi && beat.is_finite()).then(|| beat.clamp(lo, hi))
}

#[cfg(test)]
mod tests {
    use super::format_speed;

    #[test]
    fn speeds_format_like_create() {
        assert_eq!(format_speed(2.0), "2x");
        assert_eq!(format_speed(0.5), "0.5x");
        assert_eq!(format_speed(0.75), "0.75x");
        assert_eq!(format_speed(1.333), "1.33x");
        assert_eq!(format_speed(1.0), "1x");
    }

    #[test]
    fn semitones_format_like_create() {
        use super::format_semitones;
        assert_eq!(format_semitones(0.0), "0 st");
        assert_eq!(format_semitones(-0.04), "0 st");
        assert_eq!(format_semitones(3.0), "+3 st");
        assert_eq!(format_semitones(-12.0), "-12 st");
        assert_eq!(format_semitones(4.98), "+5 st");
        assert_eq!(format_semitones(-1.53), "-1.5 st");
    }

    fn reversed_clip(markers: &[(u64, f64)]) -> dstudio_core::Clip {
        // 4 s at 120 BPM: 8 beats, played back to front.
        let mut warp = dstudio_core::ClipWarp { enabled: true, reverse: true, original_bpm: 120.0, ..Default::default() };
        for &(s, b) in markers {
            assert!(warp.set_marker(s, b));
        }
        dstudio_core::Clip {
            id: dstudio_core::ClipId(1),
            asset: dstudio_core::AssetId(1),
            name: String::new(),
            start: 0,
            source_offset: 0,
            source_length: 4 * 48_000,
            gain: 1.0,
            warp,
            disabled: false,
            clip_loop: None,
            fades: Default::default(),
            takes: Vec::new(),
        }
    }

    #[test]
    fn a_clip_without_markers_is_anchored_at_its_window_start_and_end() {
        let c = reversed_clip(&[]);
        let mut w = super::anchored(&c, 48_000.0);
        let pins: Vec<(u64, f64)> = w.markers.iter().map(|k| (k.source, k.beat)).collect();
        assert_eq!(pins, vec![(0, 0.0), (4 * 48_000, 8.0)]);
        assert!(w.set_marker(24_000, c.warp.beat_at(24_000.0, 48_000.0)), "an interior transient can be pinned");
        // Pinning where the audio plays now changes nothing.
        for s in [0.0, 24_000.0, 96_000.0, 191_999.0] {
            assert!((w.beat_at(s, 48_000.0) - c.warp.beat_at(s, 48_000.0)).abs() < 1e-9);
        }
    }

    #[test]
    fn a_dragged_marker_stops_at_its_neighbours() {
        let c = reversed_clip(&[(0, 0.0), (96_000, 4.0), (192_000, 8.0)]);
        let clamp = |b: f64| super::clamp_between_neighbours(&c.warp, 96_000, b).unwrap();
        assert_eq!(clamp(5.0), 5.0);
        assert!(clamp(9.0) < 8.0 && clamp(9.0) > 7.99, "stops just before the next marker");
        assert!(clamp(-3.0) > 0.0 && clamp(-3.0) < 0.01, "stops just after the previous marker");
        let first = super::clamp_between_neighbours(&c.warp, 0, -2.0).unwrap();
        assert_eq!(first, -2.0, "the first marker is free on its outer side");
    }

    #[test]
    fn sliding_a_reversed_clips_last_marker_keeps_the_marker_and_the_clip_length() {
        let rate = 48_000.0;
        let c = reversed_clip(&[(0, 0.0), (96_000, 4.0)]);
        let m = super::Map::new(&c, &c.warp, rate);
        let (len, at) = (m.mirror.unwrap(), m.clip_beat(96_000.0));
        let w = super::slide(&c, &c.warp, 96_000, 120_000, rate).unwrap();
        let m2 = super::Map::new(&c, &w, rate);
        assert!((m2.mirror.unwrap() - len).abs() < 1e-9, "clip length {} vs {len}", m2.mirror.unwrap());
        assert!((m2.clip_beat(120_000.0) - at).abs() < 1e-9, "the marker stays at clip beat {at}");
        assert_eq!(w.markers.last().map(|k| k.source), Some(4 * 48_000), "the window end is pinned");
    }

    #[test]
    fn sliding_inside_a_trimmed_reversed_clip_keeps_the_clip_length() {
        // A trim left the last marker (240000) past the window end (192000).
        let rate = 48_000.0;
        let c = reversed_clip(&[(0, 0.0), (96_000, 4.0), (240_000, 10.0)]);
        let m = super::Map::new(&c, &c.warp, rate);
        let (len, at) = (m.mirror.unwrap(), m.clip_beat(96_000.0));
        let w = super::slide(&c, &c.warp, 96_000, 300_000, rate).unwrap();
        let m2 = super::Map::new(&c, &w, rate);
        assert!((m2.mirror.unwrap() - len).abs() < 1e-9, "clip length {} vs {len}", m2.mirror.unwrap());
        let moved = w.markers.iter().find(|k| k.beat == 4.0).unwrap().source;
        assert_eq!(moved, 4 * 48_000 - 1, "the slide stops inside the window");
        assert!((m2.clip_beat(moved as f64) - at).abs() < 1e-9);
    }
}
