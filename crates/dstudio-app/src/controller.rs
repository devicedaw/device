//! Application controller: owns the `Document`, talks to the engine, and
//! translates UI gestures into `dstudio_core::Command`s.
//!
//! The UI never holds authoritative state. After every change we rebuild the
//! engine snapshot (`snapshot::build` + `engine.publish`) and push fresh view
//! models into the `Studio` Slint global. Transient gesture state (a clip being
//! dragged) lives here only until the gesture ends and becomes one Command.

use crate::assistant_panel::AssistantState;
use crate::assistant_provider::Provider;
use crate::mixer::{self, Ballistics, MASTER_METER_ID};
use crate::peaks::{L0_FRAMES, PeakStore, Pyramid};
use crate::pianoroll::{self, PrState};
use crate::synth::SynthUi;
use crate::timeline::{self, ClipVis, Hit, TrackVis, View, Zone};
use crate::{
    AppWindow, AutoExtra, AutoLabel, ChatEntry, ChoiceItem, ClipLabel, DeviceCard, HighlightRect, MenuItem, MixerStrip, PatternRow,
    PluginRow, RulerLabel, SeqCell, SeqNum, SeqRow, ShapeRow, StripMeter, Studio, SynthDisplay, SynthParam, TrackLevel, TrackRow, TreeRow,
    YLabel,
};
use dstudio_core::{
    Asset, AssetId, Clip, ClipId, ColorIndex, Command, Document, MidiClip, Origin, PPQ, Project, Ticks, Track, TrackId, TrackKind,
    db_to_gain, gain_to_db, persist, snapshot,
};
use dstudio_engine::{AssetInfo, Engine, Meter, MetronomeConfig};
use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant};

// ------------------------------------------------------------------ value mappings

/// Track/header volume slider position (0..1) for a linear gain. 0 dB sits at
/// 0.85 of the travel, +6 dB at 1.
/// Track-header and header volume sliders: (dB, fraction of travel), piecewise linear between
/// the points.
const SLIDER_SCALE: [(f32, f32); 6] = [(-60.0, 0.0), (-48.0, 0.045), (-36.0, 0.11), (-24.0, 0.22), (-12.0, 0.4), (6.0, 1.0)];
pub fn slider_from_gain(g: f32) -> f32 {
    if g <= 0.0 {
        return 0.0;
    }
    let db = gain_to_db(g);
    if db >= 6.0 {
        return 1.0;
    }
    if db <= -60.0 {
        return 0.0;
    }
    let k = SLIDER_SCALE.iter().position(|&(d, _)| db <= d).unwrap_or(SLIDER_SCALE.len() - 1).max(1);
    let ((d0, p0), (d1, p1)) = (SLIDER_SCALE[k - 1], SLIDER_SCALE[k]);
    p0 + (p1 - p0) * (db - d0) / (d1 - d0)
}
pub fn gain_from_slider(p: f32) -> f32 {
    let p = p.clamp(0.0, 1.0);
    if p <= 0.0 {
        return 0.0;
    }
    let k = SLIDER_SCALE.iter().position(|&(_, q)| p <= q).unwrap_or(SLIDER_SCALE.len() - 1).max(1);
    let ((d0, p0), (d1, p1)) = (SLIDER_SCALE[k - 1], SLIDER_SCALE[k]);
    db_to_gain(d0 + (d1 - d0) * (p - p0) / (p1 - p0))
}

/// Track-header meter length: linear in dB from -50 dB (empty) to 0 dB (full).
pub fn header_meter(g: f32) -> f32 {
    if g <= 0.0 { 0.0 } else { ((gain_to_db(g) + 50.0) / 50.0).clamp(0.0, 1.0) }
}

/// Fader/meter taper of the mixer strips and the track inspector: (dB, fraction of travel from
/// the top). Piecewise linear in dB between the breakpoints:
/// +6 dB at the top, 0 dB at 20 %, -12 dB at
/// 60 %, then -24 / -36 / -48 dB at 78 / 89 / 95.5 %, and -60 dB (shown as −∞) at the bottom.
const FADER_SCALE: [(f32, f32); 6] = [(6.0, 0.0), (-12.0, 0.6), (-24.0, 0.78), (-36.0, 0.89), (-48.0, 0.955), (-60.0, 1.0)];
pub fn fader_from_db(db: f32) -> f32 {
    if !db.is_finite() || db <= -60.0 {
        return 1.0;
    }
    if db >= 6.0 {
        return 0.0;
    }
    for w in FADER_SCALE.windows(2) {
        let ((d0, f0), (d1, f1)) = (w[0], w[1]);
        if db <= d0 && db >= d1 {
            return f0 + (f1 - f0) * (d0 - db) / (d0 - d1);
        }
    }
    1.0
}
pub fn db_from_fader(f: f32) -> f32 {
    let f = f.clamp(0.0, 1.0);
    if f >= 0.999 {
        return f32::NEG_INFINITY;
    }
    for w in FADER_SCALE.windows(2) {
        let ((d0, f0), (d1, f1)) = (w[0], w[1]);
        if f >= f0 && f <= f1 {
            return d0 + (d1 - d0) * (f - f0) / (f1 - f0);
        }
    }
    f32::NEG_INFINITY
}

pub fn fmt_db(db: f32) -> String {
    if !db.is_finite() || db < -99.0 { "−∞".into() } else { format!("{db:.2}").replace('-', "−") }
}

/// Track-header pan entry: "C", "<n>L", "<n>R" (n 0..100) or a number -100..100.
pub fn parse_pan_text(s: &str) -> Option<f32> {
    let t = s.trim();
    if t.eq_ignore_ascii_case("c") {
        return Some(0.0);
    }
    let lower = t.to_ascii_lowercase();
    for (suffix, sign) in [("l", -1.0f32), ("r", 1.0)] {
        if let Some(n) = lower.strip_suffix(suffix).map(str::trim_end).filter(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit())) {
            return n.parse::<f32>().ok().map(|v| sign * v.min(100.0) / 100.0);
        }
    }
    t.parse::<f32>().ok().filter(|v| v.is_finite()).map(|v| (v / 100.0).clamp(-1.0, 1.0))
}

/// Channel-strip trim: "0.0 dB", "+3.5 dB", "−6.0 dB".
pub(crate) fn fmt_trim(db: f32) -> String {
    let v = (db * 10.0).round() / 10.0;
    if v == 0.0 {
        "0.0 dB".into()
    } else if v < 0.0 {
        format!("−{:.1} dB", -v)
    } else {
        format!("+{v:.1} dB")
    }
}

/// Channel-strip width as a percentage: "100%".
pub(crate) fn fmt_width(w: f32) -> String {
    format!("{:.0}%", w * 100.0)
}

pub(crate) fn fmt_pan(p: f32) -> String {
    let v = (p * 100.0).round() as i32;
    match v {
        0 => "C".into(),
        v if v < 0 => format!("{}L", -v),
        v => format!("{v}R"),
    }
}

/// The 13 track colours as shown on screen: the palette after the track tint
/// (saturation ×1.64 about Rec. 709 luma, then 10 % toward white) in display-P3 values
///. Rotated so that index 0 and 1 are the
/// blue and purple of older projects; the order is the colour cycle.
const TRACK_PALETTE: [u32; 13] =
    [0x72b5d7, 0x8092eb, 0x9b88f6, 0xc770f7, 0xec5c9f, 0xde7e36, 0x56b5ab, 0xec6e45, 0xe3aa4b, 0xc3cc47, 0xa9de4d, 0x78ea8e, 0x67c9c0];

/// Number of track colours (the Color submenu's entries).
pub fn track_palette_len() -> usize {
    TRACK_PALETTE.len()
}

pub fn track_color(c: ColorIndex) -> [f32; 3] {
    timeline::rgb(TRACK_PALETTE[c.0 as usize % TRACK_PALETTE.len()])
}

/// Colour for a new track: the first palette entry after the last track's colour that no track
/// uses yet, else the least-used one. The first track is pink.
pub fn next_track_color(p: &Project) -> ColorIndex {
    let n = TRACK_PALETTE.len();
    let Some(last) = p.tracks.last() else { return ColorIndex::FIRST };
    let start = last.color.0 as usize % n;
    let used = |c: usize| p.tracks.iter().filter(|t| t.color.0 as usize % n == c).count();
    (1..=n)
        .map(|k| (start + k) % n)
        .find(|&c| used(c) == 0)
        .map_or_else(|| ColorIndex((0..n).min_by_key(|&c| used(c)).unwrap_or(0) as u8), |c| ColorIndex(c as u8))
}

/// Card accent override for renders (`--card-accent RRGGBB`), e.g. white for effect cards
/// shown on the Master track.
pub static CARD_ACCENT: std::sync::OnceLock<[f32; 3]> = std::sync::OnceLock::new();

/// Accent of a device card on a track of colour `c`.
pub fn card_color(c: ColorIndex) -> slint::Color {
    slint_color(CARD_ACCENT.get().copied().unwrap_or_else(|| track_color(c)))
}

/// Record-armed / recording red. Mirrors `Tokens.record-red` (#e5484d).
pub const RECORD_RED: [f32; 3] = [0xe5 as f32 / 255.0, 0x48 as f32 / 255.0, 0x4d as f32 / 255.0];

pub(crate) fn slint_color(c: [f32; 3]) -> slint::Color {
    slint::Color::from_rgb_f32(c[0], c[1], c[2])
}

// ------------------------------------------------------------------ background jobs

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ImportTarget {
    /// Selected track if it is an audio track, else the first audio track, else a new one.
    Selected,
    NewTrack,
    /// A track resolved from `Selected` when the import was requested (an audio
    /// track, or a new one if it has since been removed). Imports finish on worker
    /// threads in any order, so `Selected` must not be resolved at completion: an
    /// earlier import's new track would be selected by then.
    Track(TrackId),
    /// A built-in sampler's sample slot (`next_free`: the first free layer
    /// of the pad that starts at `slot`).
    DeviceSample {
        device: dstudio_core::DeviceId,
        slot: u32,
        next_free: bool,
    },
}

pub enum JobResult {
    /// A file import finished. `doc_epoch` is the project it was started in: a
    /// result from a project that has since been closed is dropped.
    Imported { path: PathBuf, target: ImportTarget, at: Ticks, doc_epoch: u64, result: Result<(AssetInfo, Pyramid), String> },
    /// Media of an opened project reloaded into the engine.
    Reloaded { asset: AssetId, doc_epoch: u64, result: Result<(AssetInfo, Pyramid), String> },
    /// Assistant provider reply (raw `AssistantTurn` JSON) for a request started at `started`.
    Assistant { started: Instant, mode: dstudio_core::assistant::Mode, result: Result<String, String> },
    /// Takes of one finished recording pass, loaded into the engine.
    Recorded {
        doc_epoch: u64,
        takes: Vec<(dstudio_engine::RecordedTake, Result<(AssetInfo, Pyramid), String>)>,
        midi: Vec<dstudio_core::Command>,
        /// The pass's punch-in frame after a count-in.
        punch: Option<i64>,
        /// The loop that was on for the whole pass (its passes become takes).
        rec_loop: Option<(i64, i64)>,
    },
    /// Settings › Audio "Test" tone loaded into the engine.
    TestTone(Result<AssetInfo, String>),
    /// Plugin catalogue scan finished (src/devices.rs).
    PluginScan(Result<dstudio_engine::ScanReport, String>),
    /// Transients of an engine asset (warped clips, src/warpedit.rs).
    Onsets { engine_asset: u64, result: Result<Vec<dstudio_engine::Onset>, String>, tempo: Option<f64> },
    /// A file clicked in Browser › Files, decoded for the preview voice (src/files.rs).
    Auditioned { path: PathBuf, result: Result<AssetInfo, String> },
    /// A Consolidate, Bounce or Export render written and loaded (src/clip_render.rs).
    ClipRendered(Box<crate::clip_render::ClipRendered>),
    /// File › Import Ableton Live Set… read and its samples decoded (src/live_import.rs).
    LiveSet(Box<crate::live_import::LiveSetJob>),
}

/// Blocking decode + peak analysis. Runs on a worker thread.
pub fn load_and_analyse(engine: &Engine, path: &Path) -> Result<(AssetInfo, Pyramid), String> {
    let info = engine.load_file(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let pairs = (info.frames / L0_FRAMES as u64 + 1) as usize;
    let l0 = engine.peaks(info.asset_id, 0, L0_FRAMES, pairs).map_err(|e| e.to_string())?;
    Ok((info, Pyramid::from_level0(l0)))
}

// ------------------------------------------------------------------ gestures

#[derive(Debug, Clone)]
enum Drag {
    Scrub,
    Move {
        clip: ClipId,
        grab_beats: f64,
        orig_track: TrackId,
        orig_start: Ticks,
        track: TrackId,
        start: Ticks,
    },
    TrimLeft {
        clip: ClipId,
        orig: (Ticks, u64, u64),
        cur: (Ticks, u64, u64),
    },
    TrimRight {
        clip: ClipId,
        orig: (Ticks, u64, u64),
        cur: (Ticks, u64, u64),
    },
    /// Right edge of a looping audio clip: its looped length.
    LoopRight {
        clip: ClipId,
        start: Ticks,
        orig: Ticks,
        cur: Ticks,
    },
    /// MIDI clip edges: (start, length) in ticks.
    MidiLeft {
        clip: ClipId,
        orig: (Ticks, Ticks),
        cur: (Ticks, Ticks),
    },
    MidiRight {
        clip: ClipId,
        orig: (Ticks, Ticks),
        cur: (Ticks, Ticks),
    },
}

/// Bottom panel tab ("Clip" / "Devices").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BottomTab {
    Devices,
    Clip,
}

/// Internal clipboard (⌘C / ⌘X / ⌘V): one clip with its track kind.
#[derive(Debug, Clone)]
pub enum ClipboardItem {
    Audio { track: TrackId, clip: Clip },
    Midi { track: TrackId, clip: MidiClip },
}

/// Clip context menu entries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuAction {
    Cut,
    Copy,
    Duplicate,
    Split,
    Consolidate,
    Bounce,
    /// Submenu of a looped recording's takes (Take 1, Take 2, ...).
    Takes,
    Delete,
    Disable,
    Enable,
    ShowInFinder,
    Rename,
    /// Audio clips: rendered to a new track below (the clip editor's Resample).
    Resample,
    /// Audio clips: a new instrument track whose Sampler plays the clip (src/send_to_sampler.rs).
    SendToSampler,
    /// MIDI clips: the clip as a Standard MIDI File (src/midi_export.rs).
    ExportMidi,
    Export,
    Transpose,
    Quantize,
    Humanize,
    /// DStudio extension: plain MIDI clip to a step pattern, and back.
    ToPattern,
    ToMidi,
    Ask,
    Paste,
    /// Empty-lane menu: a new one-bar MIDI clip where the menu was opened.
    InsertMidiClip,
    SectionRename,
    SectionColor,
    SectionInsert,
    SectionDelete,
    SectionClear,
    /// Track header menu (src/track_menu.rs).
    TrackRename,
    TrackDuplicate,
    TrackDuplicateClips,
    TrackColor,
    /// Put the track in a new group (src/groups.rs).
    TrackGroup,
    /// A group track's menu: remove the group, keeping its tracks.
    TrackUngroup,
    TrackDelete,
    /// Record button menu (src/count_in.rs): count-in off, one bar, two bars.
    CountInOff,
    CountIn1,
    CountIn2,
}

#[derive(Debug, Default)]
pub struct MenuState {
    pub open: bool,
    pub pos: (f32, f32),
    pub clip: Option<ClipId>,
    pub section: Option<dstudio_core::SectionId>,
    /// Empty-lane menu: the track and timeline position (ticks) it was opened at.
    pub lane: Option<(TrackId, Ticks)>,
    /// Track header menu: the track it was opened on.
    pub track: Option<TrackId>,
    /// The record button's count-in menu is open.
    pub count_in: bool,
    pub items: Vec<(MenuAction, bool)>,
    pub sub_open: bool,
    /// The entry whose submenu `sub_open` shows (Transpose or Bounce).
    pub sub_for: Option<MenuAction>,
}

// ------------------------------------------------------------------ controller

pub struct Controller {
    pub doc: Document,
    pub engine: Arc<Engine>,
    pub rate: f64,
    pub(crate) asset_map: HashMap<AssetId, u64>,
    /// The engine's reversed copies of assets that reversed clips play (src/warpedit.rs).
    pub(crate) reversed_assets: HashMap<AssetId, u64>,
    /// Detected transients per engine asset (asset frames), and those being analysed.
    pub(crate) onsets: HashMap<u64, Vec<dstudio_core::Transient>>,
    pub(crate) onsets_pending: std::collections::HashSet<u64>,
    /// Tempo detected from each analysed engine asset's transients (`warp::estimate_bpm`).
    pub(crate) tempos: HashMap<u64, Option<f64>>,
    pub peaks: PeakStore,
    pub selected_track: Option<TrackId>,
    pub selected_clip: Option<ClipId>,
    /// The other selected clips besides `selected_clip` (src/selection.rs).
    pub extra_clips: Vec<ClipId>,
    /// Selection box being drawn over the lanes (src/selection.rs).
    pub(crate) marquee: Option<crate::selection::Marquee>,
    pub view: View,
    pub lanes_size: (f32, f32),
    drag: Option<Drag>,
    /// Track-height drag preview (src/splitters.rs).
    pub(crate) track_resize: Option<crate::splitters::TrackResize>,
    /// Track header press that may become a reorder drag (src/track_menu.rs).
    pub(crate) track_drag: Option<crate::track_menu::TrackDrag>,
    /// A fade handle being dragged (src/fades.rs).
    pub(crate) fade_drag: Option<crate::fades::FadeDrag>,
    /// The audio clip under the lane pointer, which shows its fade handles.
    pub(crate) hover_clip: Option<ClipId>,
    /// Folded group tracks: their tracks' rows hide (src/groups.rs).
    pub(crate) collapsed: std::collections::HashSet<TrackId>,
    /// Timeline / bottom-panel splitter drag (src/splitters.rs).
    pub(crate) bottom_drag: Option<crate::splitters::BottomDrag>,
    /// Saved height (None = default) each bottom panel kind had before its collapse button shut
    /// it, restored when the button reopens it.
    pub(crate) bottom_restore: std::collections::HashMap<&'static str, Option<f32>>,
    /// Docked-assistant width drag (src/splitters.rs).
    pub(crate) chat_drag: Option<crate::splitters::ChatDrag>,
    status: Option<(String, Instant, Duration)>,
    pub project_dir: Option<PathBuf>,
    /// Export dialog and the unsaved-changes prompt (src/dialogs.rs).
    pub export: crate::dialogs::ExportUi,
    pub clip_render: crate::clip_render::ClipRender,
    /// Test hook: where the next Export MIDI writes, instead of asking with a file dialog.
    pub midi_export_path: Option<std::path::PathBuf>,
    /// Export MIDI waits for the file dialog the main loop runs unborrowed: (clip, suggested name).
    pub midi_dialog_request: Option<(ClipId, String)>,
    pub unsaved_open: bool,
    /// A Live set to import once the unsaved-changes prompt is answered.
    pub(crate) pending_import: Option<crate::live_import::PendingImport>,
    /// Options › Check for Updates… dialog.
    pub updates_open: bool,
    /// Settings › General › Playhead on stop: where the last playback started, whether the
    /// transport is paused there since (Space), and a position the user chose since (a seek, or
    /// 0 after Stop), kept because the engine may apply it only at the next block.
    play_start: Option<i64>,
    paused: bool,
    cue: Option<i64>,
    /// Edit › Undo History… dialog.
    pub undo_history_open: bool,
    pub(crate) undo_history_model: Rc<VecModel<crate::UndoHistoryRow>>,
    pub(crate) jobs_tx: Sender<JobResult>,
    jobs_rx: Receiver<JobResult>,
    pub pending_jobs: usize,
    pub(crate) doc_epoch: u64,
    published_rev: Option<u64>,
    ui_rev: Option<u64>,
    pub ui_dirty: bool,
    meters: Vec<Meter>,
    sel_peak_hold: f32,
    /// Displayed meter level: engine `peek_meters` (non-destructive ~50 ms
    /// window) with a visual release of 24 dB/s on top.
    meter_disp: (f32, f32),
    meter_t: Option<Instant>,
    // models (kept to update in place)
    pub(crate) tracks_model: Rc<VecModel<TrackRow>>,
    ruler_model: Rc<VecModel<RulerLabel>>,
    section_label_model: Rc<VecModel<crate::SectionLabel>>,
    clips_model: Rc<VecModel<ClipLabel>>,
    ruler_key: (i64, i64, i64, i64),
    pub(crate) mm: crate::minimap::State,
    /// Timeline snap toggle, grid size and menu (src/snap.rs).
    pub(crate) snap: crate::snap::State,
    /// Transport bar tempo, signature and key fields (src/transport_fields.rs).
    pub(crate) tf: crate::transport_fields::State,
    /// Devices minimap (src/devminimap.rs) and the pointer hide / warp requests of minimap drags (src/pointer.rs).
    pub(crate) dm: crate::devminimap::State,
    pub(crate) ptr: crate::pointer::Queue,
    pub clip_vis: Vec<ClipVis>,
    /// Group lanes' clip bars (src/groups.rs).
    pub summary_vis: Vec<timeline::SummaryBlock>,
    pub track_vis: Vec<TrackVis>,
    pub(crate) clip_ids: Vec<ClipId>,
    /// Show status messages (disabled for pixel-comparison screenshots).
    pub show_status: bool,
    // ---- clip editing ----
    pub bottom_tab: BottomTab,
    pub pr: PrState,
    pub typing: crate::typing::Typing,
    pub menu: MenuState,
    pub clipboard: Option<ClipboardItem>,
    /// The other clips of a copied group, pasted at the same offset on their own tracks.
    pub(crate) clipboard_rest: Vec<ClipboardItem>,
    /// Rename popup open for this clip.
    pub rename: Option<ClipId>,
    pub rename_section: Option<dstudio_core::SectionId>,
    /// Counts presses on the lanes and the clip editors; the UI moves the keyboard focus back to
    /// the shortcuts on each, so an open name field commits (Studio.press-epoch).
    pub press_epoch: i32,
    /// The last press in the arrangement lanes (time, x, y), for double-clicks (src/clipedit.rs).
    pub(crate) lane_last_down: Option<(std::time::Instant, f32, f32)>,
    /// Where the playhead was before that press moved it (stopped transport only) and the clip
    /// selected before it, put back when the press turns out to start a double-click that made or
    /// renamed something (src/clipedit.rs `lane_press_double`).
    pub(crate) lane_playhead_before: Option<PlayheadState>,
    pub(crate) lane_clip_before: Option<ClipId>,
    /// The press under way completed a double-click: its moves and release do nothing else.
    pub(crate) lane_press_used: bool,
    /// Text typed in an open track name field (Studio.track-rename-draft), committed when the
    /// panel holding the field changes before the field commits itself.
    pub rename_draft: Option<(TrackId, String)>,
    /// Bottom panel state the last push showed (tab, mixer, a track selected).
    rename_panel: Option<(BottomTab, bool, bool)>,
    /// Keyboard rename requests (Studio.rename-open-epoch).
    rename_open_epoch: i32,
    pub selected_section: Option<dstudio_core::SectionId>,
    pub(crate) section_drag: Option<crate::sections::SectionDrag>,
    pub(crate) section_vis: Vec<crate::sections::SectionVis>,
    pub(crate) pr_bar_model: Rc<VecModel<RulerLabel>>,
    pub(crate) pr_key_model: Rc<VecModel<YLabel>>,
    pub(crate) seq_row_model: Rc<VecModel<SeqRow>>,
    pub(crate) seq_cell_model: Rc<VecModel<SeqCell>>,
    pub(crate) seq_num_model: Rc<VecModel<SeqNum>>,
    /// Row tops of the cells the playing step hits (the step sequencer's flash).
    pub(crate) seq_flash_model: Rc<VecModel<f32>>,
    pub(crate) composer_row_model: Rc<VecModel<PatternRow>>,
    /// The rows last pushed to `composer_row_model` (rebuilt only when they change).
    pub(crate) composer_rows_shown: std::cell::RefCell<Vec<crate::composer::RowView>>,
    pub(crate) composer_shape_model: Rc<VecModel<ShapeRow>>,
    pub(crate) composer_shape_shown: std::cell::RefCell<Vec<crate::shape_rack::ShapeRowView>>,
    /// The clip sidebar tab picked (true: Shape); None until one is picked.
    pub(crate) composer_shape_tab: Option<bool>,
    /// Shape tab: the curve the piano roll edits, draw mode, brush.
    pub(crate) shape_ui: crate::shape_rack::ShapeUi,
    pub(crate) menu_model: Rc<VecModel<MenuItem>>,
    pub(crate) sub_model: Rc<VecModel<MenuItem>>,
    /// Metronome on/off (app preference, not saved in the project).
    pub metronome_on: bool,
    // ---- panels (view state owned here, mirrored to Slint) ----
    pub mixer_open: bool,
    pub assistant_open: bool,
    pub assistant: AssistantState,
    pub provider: Arc<Provider>,
    ballistics: Ballistics,
    strips_model: Rc<VecModel<MixerStrip>>,
    strip_meters_model: Rc<VecModel<StripMeter>>,
    track_levels_model: Rc<VecModel<TrackLevel>>,
    chat_model: Rc<VecModel<ChatEntry>>,
    chat_rev: Option<u64>,
    highlights_model: Rc<VecModel<HighlightRect>>,
    // ---- synth device (src/synth.rs) ----
    pub synth: SynthUi,
    pub(crate) synth_params_model: Rc<VecModel<SynthParam>>,
    pub(crate) synth_displays_model: Rc<VecModel<SynthDisplay>>,
    pub(crate) synth_menu_model: Rc<VecModel<MenuItem>>,
    // ---- recording (src/recording.rs) + settings (src/settings.rs) ----
    pub rec: crate::recording::RecState,
    /// Return tracks, sends and Out (src/routing.rs).
    pub routing: crate::routing::RoutingUi,
    pub settings: crate::settings::SettingsState,
    pub(crate) input_menu_model: Rc<VecModel<ChoiceItem>>,
    pub(crate) export_tracks_model: Rc<VecModel<ChoiceItem>>,
    pub(crate) export_range_model: Rc<VecModel<ChoiceItem>>,
    pub(crate) settings_menu_model: Rc<VecModel<ChoiceItem>>,
    // ---- MIDI input (src/midi_in.rs) ----
    pub midi_in: crate::midi_in::MidiInState,
    pub(crate) midi_inputs_model: Rc<VecModel<crate::MidiInputRow>>,
    // ---- plugin devices (src/devices.rs) ----
    pub devices: crate::devices::DevicesUi,
    pub(crate) tree_model: Rc<VecModel<TreeRow>>,
    // ---- browser Files section and Back / Forward (src/files.rs) ----
    pub files: crate::files::FilesUi,
    pub(crate) files_model: Rc<VecModel<crate::FileRow>>,
    pub(crate) files_guides_model: Rc<VecModel<crate::FileGuide>>,
    pub(crate) cards_model: Rc<VecModel<DeviceCard>>,
    pub(crate) window_cards_model: Rc<VecModel<DeviceCard>>,
    pub(crate) add_results_model: Rc<VecModel<MenuItem>>,
    pub(crate) devmenu_model: Rc<VecModel<MenuItem>>,
    pub(crate) scan_dirs_model: Rc<VecModel<PluginRow>>,
    pub(crate) plugin_rows_model: Rc<VecModel<PluginRow>>,
    pub(crate) plugin_failed_model: Rc<VecModel<PluginRow>>,
    // ---- automation (src/automation.rs) ----
    pub auto: crate::automation::AutoUi,
    pub(crate) auto_labels_model: Rc<VecModel<AutoLabel>>,
    pub(crate) auto_extras_model: Rc<VecModel<AutoExtra>>,
    pub(crate) picker_model: Rc<VecModel<MenuItem>>,
    pub(crate) picker_sub_model: Rc<VecModel<MenuItem>>,
}

impl Controller {
    pub fn new(engine: Arc<Engine>, project: Project) -> Self {
        let (jobs_tx, jobs_rx) = channel();
        let rate = engine.sample_rate();
        let mut c = Controller {
            doc: Document::new(project),
            engine,
            rate,
            asset_map: HashMap::new(),
            reversed_assets: HashMap::new(),
            onsets: HashMap::new(),
            onsets_pending: std::collections::HashSet::new(),
            tempos: HashMap::new(),
            peaks: PeakStore::default(),
            selected_track: None,
            selected_clip: None,
            extra_clips: Vec::new(),
            marquee: None,
            view: View::default(),
            lanes_size: (983.0, 398.5),
            drag: None,
            track_resize: None,
            track_drag: None,
            fade_drag: None,
            hover_clip: None,
            collapsed: Default::default(),
            bottom_drag: None,
            bottom_restore: Default::default(),
            chat_drag: None,
            status: None,
            project_dir: None,
            export: Default::default(),
            clip_render: Default::default(),
            midi_export_path: None,
            midi_dialog_request: None,
            unsaved_open: false,
            pending_import: None,
            updates_open: false,
            play_start: None,
            paused: false,
            cue: None,
            undo_history_open: false,
            undo_history_model: Rc::new(VecModel::default()),
            jobs_tx,
            jobs_rx,
            pending_jobs: 0,
            doc_epoch: 0,
            published_rev: None,
            ui_rev: None,
            ui_dirty: true,
            meters: Vec::new(),
            sel_peak_hold: 0.0,
            meter_disp: (0.0, 0.0),
            meter_t: None,
            tracks_model: Rc::new(VecModel::default()),
            ruler_model: Rc::new(VecModel::default()),
            mm: crate::minimap::State::new(),
            snap: crate::snap::State::new(),
            tf: crate::transport_fields::State::new(),
            dm: crate::devminimap::State::new(),
            ptr: crate::pointer::Queue::default(),
            section_label_model: Rc::new(VecModel::default()),
            clips_model: Rc::new(VecModel::default()),
            ruler_key: (i64::MIN, 0, 0, 0),
            clip_vis: Vec::new(),
            summary_vis: Vec::new(),
            track_vis: Vec::new(),
            clip_ids: Vec::new(),
            show_status: true,
            bottom_tab: BottomTab::Devices,
            pr: PrState::default(),
            typing: Default::default(),
            menu: MenuState::default(),
            clipboard: None,
            clipboard_rest: Vec::new(),
            rename: None,
            rename_section: None,
            press_epoch: 0,
            lane_last_down: None,
            lane_playhead_before: None,
            lane_clip_before: None,
            lane_press_used: false,
            rename_draft: None,
            rename_panel: None,
            rename_open_epoch: 0,
            selected_section: None,
            section_drag: None,
            section_vis: Vec::new(),
            pr_bar_model: Rc::new(VecModel::default()),
            pr_key_model: Rc::new(VecModel::default()),
            seq_row_model: Rc::new(VecModel::default()),
            seq_cell_model: Rc::new(VecModel::default()),
            seq_num_model: Rc::new(VecModel::default()),
            seq_flash_model: Rc::new(VecModel::default()),
            composer_row_model: Rc::new(VecModel::default()),
            composer_rows_shown: Default::default(),
            composer_shape_model: Rc::new(VecModel::default()),
            composer_shape_shown: Default::default(),
            composer_shape_tab: None,
            shape_ui: Default::default(),
            menu_model: Rc::new(VecModel::default()),
            sub_model: Rc::new(VecModel::default()),
            metronome_on: false,
            mixer_open: false,
            assistant_open: false,
            assistant: AssistantState::default(),
            provider: Arc::new(Provider::Unavailable),
            ballistics: Ballistics::default(),
            strips_model: Rc::new(VecModel::default()),
            strip_meters_model: Rc::new(VecModel::default()),
            track_levels_model: Rc::new(VecModel::default()),
            chat_model: Rc::new(VecModel::default()),
            chat_rev: None,
            highlights_model: Rc::new(VecModel::default()),
            synth: SynthUi::default(),
            synth_params_model: Rc::new(VecModel::default()),
            synth_displays_model: Rc::new(VecModel::default()),
            synth_menu_model: Rc::new(VecModel::default()),
            rec: Default::default(),
            routing: Default::default(),
            settings: Default::default(),
            input_menu_model: Rc::new(VecModel::default()),
            export_tracks_model: Rc::new(VecModel::default()),
            export_range_model: Rc::new(VecModel::default()),
            settings_menu_model: Rc::new(VecModel::default()),
            midi_in: Default::default(),
            midi_inputs_model: Rc::new(VecModel::default()),
            devices: Default::default(),
            tree_model: Rc::new(VecModel::default()),
            files: Default::default(),
            files_model: Rc::new(VecModel::default()),
            files_guides_model: Rc::new(VecModel::default()),
            cards_model: Rc::new(VecModel::default()),
            window_cards_model: Rc::new(VecModel::default()),
            add_results_model: Rc::new(VecModel::default()),
            devmenu_model: Rc::new(VecModel::default()),
            scan_dirs_model: Rc::new(VecModel::default()),
            plugin_rows_model: Rc::new(VecModel::default()),
            plugin_failed_model: Rc::new(VecModel::default()),
            auto: Default::default(),
            auto_labels_model: Rc::new(VecModel::default()),
            auto_extras_model: Rc::new(VecModel::default()),
            picker_model: Rc::new(VecModel::default()),
            picker_sub_model: Rc::new(VecModel::default()),
        };
        c.selected_track = c.doc.project().tracks.first().map(|t| t.id);
        c.publish();
        c
    }

    pub fn bind_models(&self, app: &AppWindow) {
        let s = app.global::<Studio>();
        s.set_tracks(ModelRc::from(self.tracks_model.clone()));
        s.set_ruler_labels(ModelRc::from(self.ruler_model.clone()));
        s.set_minimap_bars(ModelRc::from(self.mm.model.clone()));
        s.set_devmm_tiles(ModelRc::from(self.dm.model.clone()));
        s.set_snap_menu_items(ModelRc::from(self.snap.model.clone()));
        s.set_sig_menu_items(ModelRc::from(self.tf.sig_model.clone()));
        s.set_section_labels(ModelRc::from(self.section_label_model.clone()));
        s.set_clip_labels(ModelRc::from(self.clip_ids_model()));
        s.set_strips(ModelRc::from(self.strips_model.clone()));
        s.set_strip_meters(ModelRc::from(self.strip_meters_model.clone()));
        s.set_track_levels(ModelRc::from(self.track_levels_model.clone()));
        s.set_chat(ModelRc::from(self.chat_model.clone()));
        s.set_highlights(ModelRc::from(self.highlights_model.clone()));
        s.set_pr_bar_labels(ModelRc::from(self.pr_bar_model.clone()));
        s.set_pr_key_labels(ModelRc::from(self.pr_key_model.clone()));
        s.set_seq_rows(ModelRc::from(self.seq_row_model.clone()));
        s.set_seq_cells(ModelRc::from(self.seq_cell_model.clone()));
        s.set_seq_nums(ModelRc::from(self.seq_num_model.clone()));
        s.set_seq_flash_rows(ModelRc::from(self.seq_flash_model.clone()));
        let tips: Vec<SharedString> = std::iter::once("Turn steps on and off")
            .chain(dstudio_core::steps::StepLane::ALL.iter().map(|l| l.spec().tip))
            .map(Into::into)
            .collect();
        s.set_seq_lane_tips(ModelRc::new(VecModel::from(tips)));
        s.set_composer_rows(ModelRc::from(self.composer_row_model.clone()));
        s.set_composer_shape_rows(ModelRc::from(self.composer_shape_model.clone()));
        s.set_menu_items(ModelRc::from(self.menu_model.clone()));
        s.set_sub_items(ModelRc::from(self.sub_model.clone()));
        s.set_synth_params(ModelRc::from(self.synth_params_model.clone()));
        s.set_synth_displays(ModelRc::from(self.synth_displays_model.clone()));
        s.set_synth_menu_items(ModelRc::from(self.synth_menu_model.clone()));
        s.set_input_menu_items(ModelRc::from(self.input_menu_model.clone()));
        s.set_route_menu_items(ModelRc::from(self.routing.menu_model.clone()));
        s.set_strip_sends(ModelRc::from(self.routing.strip_sends_model.clone()));
        s.set_sel_sends(ModelRc::from(self.routing.sel_sends_model.clone()));
        s.set_export_tracks_items(ModelRc::from(self.export_tracks_model.clone()));
        s.set_export_range_items(ModelRc::from(self.export_range_model.clone()));
        s.set_undo_history_rows(ModelRc::from(self.undo_history_model.clone()));
        s.set_settings_menu_items(ModelRc::from(self.settings_menu_model.clone()));
        s.set_midi_inputs(ModelRc::from(self.midi_inputs_model.clone()));
        s.set_tree(ModelRc::from(self.tree_model.clone()));
        s.set_files(ModelRc::from(self.files_model.clone()));
        s.set_files_guides(ModelRc::from(self.files_guides_model.clone()));
        s.set_device_cards(ModelRc::from(self.cards_model.clone()));
        s.set_device_window_cards(ModelRc::from(self.window_cards_model.clone()));
        s.set_add_results(ModelRc::from(self.add_results_model.clone()));
        s.set_devmenu_items(ModelRc::from(self.devmenu_model.clone()));
        s.set_scan_dirs(ModelRc::from(self.scan_dirs_model.clone()));
        s.set_plugin_rows(ModelRc::from(self.plugin_rows_model.clone()));
        s.set_plugin_failed(ModelRc::from(self.plugin_failed_model.clone()));
        s.set_auto_labels(ModelRc::from(self.auto_labels_model.clone()));
        s.set_auto_extras(ModelRc::from(self.auto_extras_model.clone()));
        s.set_picker_items(ModelRc::from(self.picker_model.clone()));
        s.set_picker_sub_items(ModelRc::from(self.picker_sub_model.clone()));
        let pages: Vec<SharedString> = crate::settings::PAGES.iter().map(|p| SharedString::from(*p)).collect();
        s.set_settings_pages(ModelRc::new(VecModel::from(pages)));
    }

    fn clip_ids_model(&self) -> Rc<VecModel<ClipLabel>> {
        self.clips_model.clone()
    }

    pub fn project(&self) -> &Project {
        self.doc.project()
    }

    pub fn status_text(&self) -> Option<&str> {
        self.status.as_ref().map(|s| s.0.as_str())
    }

    /// The message of the status line (self-test).
    pub(crate) fn status_message(&self) -> Option<&str> {
        self.status.as_ref().map(|s| s.0.as_str())
    }

    pub fn set_status(&mut self, msg: impl Into<String>, secs: u64) {
        let msg = msg.into();
        eprintln!("[status] {msg}");
        self.status = Some((msg, Instant::now(), Duration::from_secs(secs)));
        self.ui_dirty = true;
    }

    /// App menu bar items without a shortcut (`Studio.menu-action`). Settings and the
    /// dialogs are modal, so the menu does nothing while one is open.
    pub fn menu_action(&mut self, id: &str) {
        if self.modal_open() {
            return;
        }
        match id {
            "options.audio_midi" => self.open_settings(),
            "edit.undo_history" => {
                self.undo_history_open = true;
                self.ui_dirty = true;
            }
            "options.check_updates" => {
                self.updates_open = true;
                self.ui_dirty = true;
            }
            "file.export_ableton" => {
                self.export_open();
                self.export_pick_recipe(2);
            }
            "file.clear_recent" => {
                self.settings.prefs.recent_projects.clear();
                self.settings.prefs.recent_resolved.clear();
                self.save_prefs();
                self.ui_dirty = true;
            }
            "options.check_plugins" => self.start_plugin_scan(false),
            "options.rescan_plugins" => self.start_plugin_scan(true),
            "file.show_in_finder" => {
                let Some(dir) = &self.project_dir else { return };
                if let Err(e) = std::process::Command::new("open").arg("-R").arg(dir).spawn() {
                    self.set_status(format!("Show in Finder failed: {e}"), 3);
                }
            }
            other => self.set_status(format!("{other}: not implemented yet"), 3),
        }
    }

    pub fn modal_open(&self) -> bool {
        self.settings.open || self.export.open || self.unsaved_open || self.updates_open || self.undo_history_open
    }

    /// File › Open Recent item `i` (newest first). A project that no longer opens leaves the list.
    pub fn open_recent(&mut self, i: usize) {
        if self.modal_open() {
            return;
        }
        let Some(path) = self.settings.prefs.recent_projects.get(i).map(PathBuf::from) else { return };
        if !self.open(&path) {
            self.settings.prefs.recent_projects.remove(i);
            if i < self.settings.prefs.recent_resolved.len() {
                self.settings.prefs.recent_resolved.remove(i);
            }
            self.save_prefs();
        }
    }

    fn remember_recent(&mut self, dir: &Path) {
        let path = dir.to_string_lossy().into_owned();
        let resolved = std::fs::canonicalize(dir).map_or_else(|_| path.clone(), |p| p.to_string_lossy().into_owned());
        let prefs = &mut self.settings.prefs;
        prefs.recent_resolved.resize(prefs.recent_projects.len(), String::new());
        let mut entries: Vec<(String, String)> =
            prefs.recent_projects.drain(..).zip(prefs.recent_resolved.drain(..)).filter(|(p, r)| *p != path && *r != resolved).collect();
        entries.insert(0, (path, resolved));
        entries.truncate(10);
        (prefs.recent_projects, prefs.recent_resolved) = entries.into_iter().unzip();
        self.save_prefs();
    }

    // ---------------------------------------------------------------- core glue

    pub fn execute(&mut self, cmd: Command) -> bool {
        match self.doc.execute(cmd, Origin::User) {
            Ok(()) => {
                self.after_change();
                true
            }
            Err(e) => {
                self.set_status(format!("Edit rejected: {e}"), 4);
                false
            }
        }
    }

    /// Publish a new engine snapshot if the document changed.
    pub fn after_change(&mut self) {
        self.remember_device_names();
        let p = self.doc.project();
        if let Some(t) = self.selected_track
            && p.track(t).is_none()
        {
            self.selected_track = p.tracks.first().map(|t| t.id);
        }
        if let Some(c) = self.selected_clip
            && !p.clip_id_in_use(c)
        {
            self.selected_clip = None;
        }
        let gone: Vec<ClipId> = self.extra_clips.iter().copied().filter(|&c| !p.clip_id_in_use(c)).collect();
        self.extra_clips.retain(|c| !gone.contains(c));
        if self.selected_clip.is_none() && !self.extra_clips.is_empty() {
            self.selected_clip = Some(self.extra_clips.remove(0));
        }
        if self.selected_section.is_some_and(|s| p.section_index(s).is_none()) {
            self.selected_section = None;
        }
        if self.menu.clip.is_some_and(|c| !p.clip_id_in_use(c)) {
            self.menu_close();
        }
        self.pr_sync_clip();
        if self.published_rev != Some(self.doc.revision()) {
            self.publish();
        }
        self.devices_after_change();
        self.ui_dirty = true;
    }

    /// The session the engine should play right now: the document plus any
    /// automation lane being edited.
    pub fn current_snapshot(&self) -> snapshot::SessionSnapshot {
        let (mut snap, _skipped) =
            snapshot::build_full(self.doc.project(), self.rate, &self.asset_map, &self.reversed_assets, &self.transients_by_asset());
        // Automation drag / write preview: the pending lane replaces the model lane.
        if let Some((key, lane)) = &self.auto.pending {
            let p = self.doc.project();
            let ls = snapshot::LaneSnapshot {
                target: lane.target.clone(),
                points: lane.points.iter().map(|q| (p.ticks_to_frames(q.time, self.rate), q.value, q.curve)).collect(),
            };
            let lanes = match key {
                // A muted master stays silent: no volume lane, not even one being drawn.
                None if p.master_mute => None,
                None => Some(&mut snap.master_automation),
                Some(t) => snap.tracks.iter_mut().find(|ts| ts.track_id == t.0).map(|ts| &mut ts.automation),
            };
            if let Some(lanes) = lanes {
                lanes.retain(|l| l.target != lane.target);
                if lane.enabled && !lane.points.is_empty() {
                    lanes.push(ls);
                }
            }
        }
        snap
    }

    pub fn publish(&mut self) {
        self.request_onsets();
        self.ensure_reversed_assets();
        let snap = self.current_snapshot();
        if let Err(e) = self.engine.publish(&snap) {
            self.set_status(format!("Engine rejected the session: {e}"), 6);
        }
        self.published_rev = Some(self.doc.revision());
        self.restore_mod_bases();
        self.sync_loop_and_click();
    }

    /// Push loop region and metronome grid (both derived from the project
    /// transport) to the engine. Cheap; called after every publish.
    pub(crate) fn sync_loop_and_click(&mut self) {
        let p = self.doc.project();
        let tr = &p.transport;
        let (start, end) = (p.ticks_to_frames(tr.loop_start, self.rate), p.ticks_to_frames(tr.loop_end, self.rate));
        let beat_ticks = PPQ * 4 / tr.time_signature.denominator as i64;
        let frames_per_beat = p.ticks_to_seconds(beat_ticks) * self.rate;
        // A count-in clicks whether the metronome is on or not.
        let enabled = self.metronome_on || self.rec.count_in.is_some();
        let cfg = MetronomeConfig { enabled, gain: 0.5, frames_per_beat, beats_per_bar: tr.time_signature.numerator as u32 };
        let r = self.engine.set_loop(tr.loop_enabled, start, end).and_then(|_| self.engine.set_metronome(cfg));
        if let Err(e) = r {
            self.set_status(format!("Engine: {e}"), 4);
        }
    }

    pub fn toggle_loop(&mut self) {
        let tr = self.doc.project().transport.clone();
        let cmd = Command::SetLoop { enabled: !tr.loop_enabled, start: tr.loop_start, end: tr.loop_end };
        match self.doc.execute(cmd, Origin::User) {
            Ok(()) => self.after_change(),
            Err(e) => self.set_status(format!("Loop: {e}"), 4),
        }
    }

    pub fn toggle_metronome(&mut self) {
        self.metronome_on = !self.metronome_on;
        self.sync_loop_and_click();
        self.ui_dirty = true;
    }

    /// Undo and redo wait while a MIDI pass records: they could change which
    /// instrument tracks are armed, and the pass records on the ones armed at its start.
    fn history_locked(&mut self) -> bool {
        if self.midi_in.pass.is_some() {
            self.set_status("Stop recording to undo or redo", 4);
            return true;
        }
        false
    }

    pub fn undo(&mut self) {
        if self.history_locked() {
            return;
        }
        self.synth.drop_gesture();
        self.auto.sel = None;
        match self.doc.undo() {
            Ok(true) => self.after_change(),
            Ok(false) => {}
            Err(e) => self.set_status(format!("Undo failed: {e}"), 4),
        }
    }

    pub fn redo(&mut self) {
        if self.history_locked() {
            return;
        }
        self.synth.drop_gesture();
        self.auto.sel = None;
        match self.doc.redo() {
            Ok(true) => self.after_change(),
            Ok(false) => {}
            Err(e) => self.set_status(format!("Redo failed: {e}"), 4),
        }
    }

    // ---------------------------------------------------------------- transport

    pub fn play_toggle(&mut self) {
        if !self.engine.device_running() {
            self.set_status("No audio device: playback unavailable (offline mode)", 4);
            return;
        }
        let r = if self.engine.transport().playing {
            let r = self.engine.pause();
            if r.is_ok() {
                self.paused = true;
                self.cue = None;
            }
            r
        } else {
            let from = play_from(self.settings.prefs.general.stop_in_place, self.play_start, self.paused, self.cue.take());
            self.paused = false;
            self.play_start = Some(match from {
                Some(start) if self.engine.seek(start).is_ok() => start,
                _ => self.engine.transport().position,
            });
            self.engine.play()
        };
        if let Err(e) = r {
            self.set_status(format!("Transport: {e}"), 4);
        }
        self.ui_dirty = true;
    }

    pub fn stop_return(&mut self) {
        if self.rec.recording {
            self.stop_recording();
        }
        if self.engine.stop().is_ok() {
            self.paused = false;
            self.cue = Some(0);
        }
        self.ui_dirty = true;
    }

    /// Position the user hears: transport position minus plugin delay
    /// compensation and output latency while playing (equal to the transport
    /// position when stopped). Used for the playhead and all readouts.
    pub fn audible_frames(&self) -> i64 {
        self.engine.audible_position().max(0)
    }

    pub fn playhead_ticks(&self) -> Ticks {
        self.project().frames_to_ticks(self.audible_frames(), self.rate)
    }

    /// The stopped transport's position and cue, to put back with [`Controller::restore_playhead`]
    /// (`None` while playing: the playhead moves on by itself).
    pub(crate) fn playhead_state(&self) -> Option<PlayheadState> {
        (!self.engine.transport().playing).then(|| PlayheadState { frame: self.audible_frames(), paused: self.paused, cue: self.cue })
    }

    pub(crate) fn restore_playhead(&mut self, st: PlayheadState) {
        if self.engine.transport().playing || self.engine.seek(st.frame).is_err() {
            return;
        }
        (self.paused, self.cue) = (st.paused, st.cue);
    }

    pub fn seek_beats(&mut self, beats: f64) {
        let ticks = (beats.max(0.0) * PPQ as f64).round() as Ticks;
        let frame = self.project().ticks_to_frames(ticks, self.rate);
        // Applied immediately when no device runs (engine ABI), else at the next block.
        if self.engine.seek(frame).is_ok() {
            self.paused = false;
            self.cue = Some(frame);
        }
    }

    // ---------------------------------------------------------------- track edits

    pub fn select_track(&mut self, id: TrackId) {
        if self.selected_track != Some(id) {
            self.selected_section = None;
            self.selected_track = Some(id);
            if let Some(c) = self.selected_clip
                && self.clip_track(c) != Some(id)
            {
                self.selected_clip = None;
                self.extra_clips.clear();
            }
            self.sel_peak_hold = 0.0;
            self.meter_disp = (0.0, 0.0);
            self.synth.drop_gesture();
            self.synth.menu = None;
            // Instrument tracks open their device (Devices tab) unless a clip is selected.
            if self.selected_clip.is_none() && self.project().track(id).is_some_and(|t| t.kind == TrackKind::Instrument) {
                self.bottom_tab = BottomTab::Devices;
                self.pr_sync_clip();
            }
            self.ui_dirty = true;
        }
    }

    pub fn add_audio_track(&mut self) -> Option<TrackId> {
        // The new track goes after the ordinary tracks, before any return tracks.
        let id = TrackId(self.project().next_id);
        let track = Track::new(id, TrackKind::Audio, "Audio", next_track_color(self.project()));
        if self.execute(Command::AddTrack { index: None, track }) {
            self.select_track(id);
            return Some(id);
        }
        None
    }

    /// ⇧⌘T: a new instrument track at the end of the ordinary tracks, selected.
    pub fn add_instrument_track(&mut self) -> Option<TrackId> {
        let track = Track::new(TrackId(0), TrackKind::Instrument, "Instrument", next_track_color(self.project()));
        if self.execute(Command::AddTrack { index: None, track }) {
            let id = self.project().tracks.iter().rev().find(|t| t.kind == TrackKind::Instrument).map(|t| t.id);
            if let Some(id) = id {
                self.select_track(id);
            }
            return id;
        }
        None
    }

    /// ⌘L with a clip or a section selected: loop exactly that span and turn the loop on.
    pub fn loop_selection(&mut self) {
        let p = self.project();
        let span = if let Some(i) = self.selected_section.and_then(|s| p.section_index(s)) {
            let start = p.section_start(i);
            Some((start, start + p.sections[i].length))
        } else if let Some(id) = self.selected_clip {
            p.midi_clip(id).map(|m| (m.start, m.end())).or_else(|| p.clip(id).map(|c| (c.start, c.start + p.clip_length_ticks(c))))
        } else {
            None
        };
        let Some((start, end)) = span.filter(|(s, e)| e > s) else {
            self.toggle_loop();
            return;
        };
        let tr = &p.transport;
        if tr.loop_enabled && tr.loop_start == start && tr.loop_end == end {
            return;
        }
        self.doc.seal_coalescing();
        self.execute(Command::SetLoop { enabled: true, start, end });
    }

    pub fn delete_selection(&mut self) {
        if let Some(s) = self.selected_section.filter(|s| self.project().section_index(*s).is_some()) {
            self.delete_section(s);
        } else if !self.extra_clips.is_empty() {
            self.delete_clips();
        } else if let Some(c) = self.selected_clip.take() {
            let cmd = if self.project().find_midi_clip(c).is_some() {
                Command::RemoveMidiClip { clip: c }
            } else {
                Command::RemoveClip { clip: c }
            };
            self.execute(cmd);
        } else if let Some(t) = self.selected_track {
            let idx = self.project().track_index(t).unwrap_or(0);
            if self.execute(Command::RemoveTrack { track: t }) {
                let p = self.project();
                self.selected_track = p.tracks.get(idx.min(p.tracks.len().saturating_sub(1))).map(|t| t.id);
                self.ui_dirty = true;
            }
        }
    }

    pub fn toggle_mute(&mut self, id: TrackId) {
        if let Some(t) = self.project().track(id) {
            let mute = !t.mute;
            // An active Mute lane decides (written while armed and playing).
            if self.auto_switch(Some(id), dstudio_core::AutomationTarget::Mute, t.mute) {
                return;
            }
            self.execute(Command::SetTrackMute { track: id, mute });
        }
    }

    /// Rename a track (double-click its name). Surrounding spaces are dropped; an empty or
    /// unchanged name leaves the track as it was (an untouched field keeps a loaded name's own
    /// spaces too).
    pub fn rename_track(&mut self, id: TrackId, name: &str) {
        self.rename_draft = None;
        let raw = name;
        let name = name.trim();
        if name.is_empty() || self.project().track(id).is_none_or(|t| t.name == raw || t.name == name) {
            self.ui_dirty = true;
            return;
        }
        self.execute(Command::RenameTrack { track: id, name: name.to_string() });
    }

    /// Keyboard rename (track.rename): open the selected track's name field in its header row,
    /// scrolled into view.
    pub fn open_rename_field(&mut self) {
        let Some(id) = self.selected_track else { return };
        if self.project().track(id).is_none() {
            return;
        }
        self.reveal_track(id);
        self.rename_open_epoch += 1;
        self.ui_dirty = true;
    }

    /// Scroll the timeline vertically so track `id`'s row (or, for a row taller than the lanes,
    /// its middle) is in view.
    fn reveal_track(&mut self, id: TrackId) {
        let Some(i) = self.project().track_index(id) else { return };
        if self.view.is_pinned(i) {
            return; // A pinned return row is always in view.
        }
        let (top, h) = (self.view.track_offset(i), self.view.pitch(i));
        // The scrolling lanes end at the pinned band (return rows and master).
        let visible = self.lanes_size.1 - timeline::RULER_H - self.view.footer_h();
        let y = self.view.scroll_y as f32;
        if h > visible {
            // Taller than the lanes: the header's controls (the name field with them) sit around
            // the row's middle, so centre that.
            self.view.scroll_y = (top + h / 2.0 - visible / 2.0).max(0.0) as f64;
        } else if top < y {
            self.view.scroll_y = top as f64;
        } else if top + h > y + visible {
            self.view.scroll_y = (top + h - visible).max(0.0) as f64;
        }
        self.view.clamp_scroll(self.project().tracks.len(), self.lanes_size.1);
    }

    /// Commit a name typed in a field that is about to disappear with its panel; true when there was one.
    fn commit_rename_draft(&mut self) -> bool {
        let Some((id, name)) = self.rename_draft.take() else { return false };
        self.rename_track(id, &name);
        true
    }

    pub fn toggle_solo(&mut self, id: TrackId) {
        if let Some(t) = self.project().track(id) {
            let solo = !t.solo;
            self.execute(Command::SetTrackSolo { track: id, solo });
        }
    }

    /// Continuous drags: the core coalesces consecutive SetTrackGain/Pan for one target.
    pub fn set_track_gain(&mut self, id: TrackId, gain: f32) {
        let gain = gain.clamp(0.0, dstudio_core::command::MAX_GAIN);
        if self.auto_intercept(Some(id), dstudio_core::AutomationTarget::Volume, gain_to_db(gain)) {
            return;
        }
        if self.project().track(id).map(|t| (t.gain - gain).abs() > 1e-7).unwrap_or(false) {
            self.execute(Command::SetTrackGain { track: id, gain });
        }
    }

    pub fn set_track_pan(&mut self, id: TrackId, pan: f32) {
        let pan = (pan.clamp(-1.0, 1.0) * 100.0).round() / 100.0;
        if self.auto_intercept(Some(id), dstudio_core::AutomationTarget::Pan, pan) {
            return;
        }
        if self.project().track(id).map(|t| (t.pan - pan).abs() > 1e-7).unwrap_or(false) {
            self.execute(Command::SetTrackPan { track: id, pan });
        }
    }

    /// Channel-strip trim in dB, in 0.1 dB steps. A held knob drag is one undo step
    /// (sealed by [Self::strip_gesture_end]).
    pub fn set_track_trim(&mut self, id: TrackId, db: f32) -> bool {
        let (lo, hi) = dstudio_core::Track::TRIM_RANGE_DB;
        let db = (db.clamp(lo, hi) * 10.0).round() / 10.0;
        if self.auto_intercept(Some(id), dstudio_core::AutomationTarget::Trim, db) {
            return false;
        }
        if self.project().track(id).is_none_or(|t| t.trim_db == db) {
            return false;
        }
        let ok = self.execute(Command::SetTrackTrim { track: id, trim_db: db });
        if ok {
            self.doc.hold_coalescing();
        }
        ok
    }

    /// Channel-strip width (0..2, 1 = unchanged) in 1 % steps; one held drag is one undo step.
    pub fn set_track_width(&mut self, id: TrackId, width: f32) -> bool {
        let (lo, hi) = dstudio_core::Track::WIDTH_RANGE;
        let width = (width.clamp(lo, hi) * 100.0).round() / 100.0;
        if self.auto_intercept(Some(id), dstudio_core::AutomationTarget::Width, width) {
            return false;
        }
        if self.project().track(id).is_none_or(|t| t.width == width) {
            return false;
        }
        let ok = self.execute(Command::SetTrackWidth { track: id, width });
        if ok {
            self.doc.hold_coalescing();
        }
        ok
    }

    pub fn toggle_track_mono(&mut self, id: TrackId) -> bool {
        let Some(old) = self.project().track(id).map(|t| t.mono) else { return false };
        if self.auto_switch(Some(id), dstudio_core::AutomationTarget::Mono, old) {
            return false;
        }
        self.execute(Command::SetTrackMono { track: id, mono: !old })
    }

    /// A trim or width knob was released: the next drag is a new undo step.
    pub fn strip_gesture_end(&mut self) {
        self.auto_write_end(|w| matches!(w.target, dstudio_core::AutomationTarget::Trim | dstudio_core::AutomationTarget::Width));
        self.doc.seal_coalescing();
    }

    pub fn set_master_gain(&mut self, gain: f32) {
        if self.auto_intercept(None, dstudio_core::AutomationTarget::Volume, gain_to_db(gain)) {
            return;
        }
        if (self.project().master_gain - gain).abs() > 1e-7 {
            self.execute(Command::SetMasterGain { gain });
        }
    }

    /// The master strip's M: silences the whole mix (the fader keeps its value). Undoable.
    pub fn toggle_master_mute(&mut self) {
        let mute = !self.project().master_mute;
        self.execute(Command::SetMasterMute { mute });
    }

    // ---------------------------------------------------------------- mixer

    pub fn toggle_mixer(&mut self) {
        self.mixer_open = !self.mixer_open;
        self.ballistics.reset_hold();
        self.ui_dirty = true;
    }

    /// Mixer strip fader (0 = +6 dB at the top .. 1 = −∞): same coalesced SetTrackGain as the headers.
    pub fn set_mixer_fader(&mut self, id: TrackId, f: f32) {
        self.set_track_gain(id, db_to_gain(db_from_fader(f)));
    }

    /// Master strip fader: SetMasterGain (coalesced).
    pub fn set_master_fader(&mut self, f: f32) {
        self.set_master_gain(db_to_gain(db_from_fader(f)).min(dstudio_core::command::MAX_GAIN));
    }

    pub fn reset_peaks(&mut self) {
        self.ballistics.reset_hold();
        self.sel_peak_hold = 0.0;
    }

    // ---------------------------------------------------------------- import / persistence

    /// Import on worker threads; clips land at `at` (default: the playhead).
    pub fn import_files(&mut self, paths: Vec<PathBuf>, target: ImportTarget, at: Option<Ticks>) {
        let at = at.unwrap_or_else(|| self.playhead_ticks());
        let doc_epoch = self.doc_epoch;
        let target = match target {
            ImportTarget::Selected => self.selected_audio_track().map_or(ImportTarget::NewTrack, ImportTarget::Track),
            t => t,
        };
        for (i, path) in paths.into_iter().enumerate() {
            let engine = self.engine.clone();
            let tx = self.jobs_tx.clone();
            // Additional files go to new tracks so they line up at the playhead.
            let target = if i == 0 || matches!(target, ImportTarget::DeviceSample { .. }) { target } else { ImportTarget::NewTrack };
            self.pending_jobs += 1;
            std::thread::Builder::new()
                .name("import".into())
                .spawn(move || {
                    let result = load_and_analyse(&engine, &path);
                    let _ = tx.send(JobResult::Imported { path, target, at, doc_epoch, result });
                })
                .expect("spawn import thread");
        }
        self.set_status("Importing…", 30);
    }

    /// The selected track if it is an audio track, else the first audio track.
    fn selected_audio_track(&self) -> Option<TrackId> {
        let p = self.project();
        self.selected_track
            .and_then(|t| p.track(t))
            .filter(|t| t.kind == TrackKind::Audio)
            .or_else(|| p.tracks.iter().find(|t| t.kind == TrackKind::Audio))
            .map(|t| t.id)
    }

    /// Drain finished background jobs (called from a UI timer and each frame).
    pub fn poll_jobs(&mut self) {
        self.clip_render_tick();
        while let Ok(job) = self.jobs_rx.try_recv() {
            self.pending_jobs = self.pending_jobs.saturating_sub(1);
            match job {
                JobResult::Imported { doc_epoch, result: Ok((info, _)), .. } if doc_epoch != self.doc_epoch => {
                    // Started in a project that has since been closed: its target
                    // track id could name a track in the new project.
                    let _ = self.engine.release_asset(info.asset_id);
                }
                JobResult::Imported { doc_epoch, .. } if doc_epoch != self.doc_epoch => {}
                JobResult::Imported { path, target, at, result, .. } => match result {
                    Ok((info, pyr)) => self.finish_import(&path, target, at, info, pyr),
                    Err(e) => self.set_status(format!("Import failed: {e}"), 6),
                },
                JobResult::Assistant { started, mode, result } => self.finish_assistant(started, mode, result),
                JobResult::Recorded { doc_epoch, takes, midi, punch, rec_loop } => {
                    self.finish_recorded(doc_epoch, takes, midi, punch, rec_loop)
                }
                JobResult::TestTone(r) => self.finish_test_tone(r),
                JobResult::PluginScan(r) => self.finish_plugin_scan(r),
                JobResult::Onsets { engine_asset, result, tempo } => self.finish_onsets(engine_asset, result, tempo),
                JobResult::Auditioned { path, result } => self.finish_audition(path, result),
                JobResult::ClipRendered(r) => self.finish_clip_render(*r),
                JobResult::LiveSet(job) => self.finish_live_set(*job),
                JobResult::Reloaded { asset, doc_epoch, result } => {
                    if doc_epoch != self.doc_epoch {
                        if let Ok((info, _)) = result {
                            let _ = self.engine.release_asset(info.asset_id);
                        }
                        continue;
                    }
                    match result {
                        Ok((info, pyr)) => {
                            self.peaks.insert(info.asset_id, pyr);
                            self.asset_map.insert(asset, info.asset_id);
                            self.publish();
                            self.ui_dirty = true;
                        }
                        Err(e) => self.set_status(format!("Missing media: {e}"), 6),
                    }
                }
            }
            if self.pending_jobs == 0 && self.status.as_ref().is_some_and(|s| s.0.starts_with("Importing") || s.0.starts_with("Loading")) {
                self.status = None;
                self.ui_dirty = true;
            }
        }
    }

    fn finish_import(&mut self, path: &Path, target: ImportTarget, at: Ticks, info: AssetInfo, pyr: Pyramid) {
        let p = self.project();
        let mut next = p.next_id;
        let mut alloc = || {
            let id = next;
            next += 1;
            id
        };
        let asset_id = AssetId(alloc());
        let native_frames = (info.frames as f64 * info.source_sample_rate / self.rate).round().max(1.0) as u64;
        let asset = Asset {
            id: asset_id,
            relative_path: None,
            absolute_path: std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf()).to_string_lossy().into_owned(),
            sample_rate: info.source_sample_rate,
            channels: info.channels,
            frames: native_frames,
        };
        if let ImportTarget::DeviceSample { device, slot, next_free } = target {
            self.peaks.insert(info.asset_id, pyr);
            self.asset_map.insert(asset_id, info.asset_id);
            let name = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "Sample".into());
            if !self.finish_device_sample(asset, name, device, slot, next_free) {
                self.asset_map.remove(&asset_id);
                let _ = self.engine.release_asset(info.asset_id);
            }
            self.ui_dirty = true;
            return;
        }
        let mut cmds = vec![Command::AddAsset { asset }];
        let existing = match target {
            ImportTarget::DeviceSample { .. } => None,
            ImportTarget::NewTrack => None,
            ImportTarget::Selected => self.selected_audio_track(),
            ImportTarget::Track(t) => p.track(t).filter(|t| t.kind == TrackKind::Audio).map(|t| t.id),
        };
        let track_id = existing.unwrap_or_else(|| {
            let id = TrackId(alloc());
            cmds.push(Command::AddTrack { index: None, track: Track::new(id, TrackKind::Audio, "Audio", next_track_color(p)) });
            id
        });
        let name = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "Audio".into());
        let clip_id = ClipId(alloc());
        cmds.push(Command::AddClip {
            track: track_id,
            clip: Clip {
                id: clip_id,
                asset: asset_id,
                name,
                start: at.max(0),
                source_offset: 0,
                source_length: native_frames,
                gain: 1.0,
                warp: Default::default(),
                disabled: false,
                clip_loop: None,
                fades: Default::default(),
                takes: Vec::new(),
            },
        });
        // Engine asset must be known before the snapshot is published.
        self.peaks.insert(info.asset_id, pyr);
        self.asset_map.insert(asset_id, info.asset_id);
        if self.execute(Command::Batch { label: "Import Audio".into(), commands: cmds }) {
            self.selected_track = Some(track_id);
            self.selected_clip = Some(clip_id);
            self.extra_clips.clear();
            self.ui_dirty = true;
        } else {
            self.asset_map.remove(&asset_id);
            let _ = self.engine.release_asset(info.asset_id);
        }
    }

    /// Start over with `project` (no undo history). Used by `--state` setups.
    pub fn replace_project(&mut self, project: Project) {
        self.close_all_editors();
        self.route_menu_close(); // its items name tracks of the old project
        self.onsets.clear();
        self.tempos.clear();
        self.doc_epoch += 1;
        self.auto.reset_project_state();
        self.tf.reset_project_gestures();
        self.rename_draft = None;
        self.bottom_restore.clear();
        self.doc = Document::new(project);
        self.devices.modu.bases_pending = true;
        self.selected_track = self.project().tracks.first().map(|t| t.id);
        self.selected_clip = None;
        self.extra_clips.clear();
        self.published_rev = None;
        self.after_change();
    }

    pub fn save_to(&mut self, dir: &Path) {
        // Plugin instances own their state: pull it into the model first (no undo step).
        let states = self.engine.plugin_states();
        self.doc.sync_device_states(&states);
        let bases = self.mod_bases_now();
        self.doc.sync_mod_bases(&bases);
        if let Some(name) = project_name_of(dir) {
            self.doc.set_project_name(&name);
        }
        match persist::save(self.project(), dir) {
            Ok(()) => {
                self.doc.mark_saved();
                self.project_dir = Some(dir.to_path_buf());
                self.remember_recent(dir);
                self.set_status(format!("Saved {}", dir.display()), 3);
                self.files_refresh();
            }
            Err(e) => self.set_status(format!("Save failed: {e}"), 6),
        }
    }

    /// Open the project in `dir`; false (with a status message) when it can't be read.
    pub fn open(&mut self, dir: &Path) -> bool {
        let loaded = match persist::load(dir) {
            Ok(l) => l,
            Err(e) => {
                self.set_status(format!("Open failed: {e}"), 6);
                return false;
            }
        };
        self.stop_return();
        self.close_all_editors();
        self.route_menu_close(); // its items name tracks of the old project
        for (_, ea) in self.asset_map.drain().chain(self.reversed_assets.drain()) {
            let _ = self.engine.release_asset(ea);
        }
        self.onsets.clear();
        self.tempos.clear();
        self.doc_epoch += 1;
        self.auto.reset_project_state();
        self.tf.reset_project_gestures();
        self.rename_draft = None;
        self.bottom_restore.clear();
        self.doc = Document::new(loaded.project);
        self.devices.modu.bases_pending = true;
        self.project_dir = Some(dir.to_path_buf());
        self.remember_recent(dir);
        self.selected_track = self.project().tracks.first().map(|t| t.id);
        self.selected_clip = None;
        self.extra_clips.clear();
        self.view = View::default();
        self.published_rev = None;
        self.after_change();
        self.devices_after_open();
        let missing: std::collections::HashSet<String> = loaded.missing_media.into_iter().collect();
        for a in self.project().assets.clone() {
            if !missing.contains(&a.absolute_path) {
                self.load_asset(&a);
            }
        }
        if !missing.is_empty() {
            self.set_status(format!("{} media file(s) missing", missing.len()), 6);
        } else if self.pending_jobs > 0 {
            self.set_status("Loading media…", 30);
        }
        true
    }

    /// Load a project asset into the engine on a worker thread (finishes in `poll_jobs`).
    pub(crate) fn load_asset(&mut self, a: &Asset) {
        let engine = self.engine.clone();
        let tx = self.jobs_tx.clone();
        let epoch = self.doc_epoch;
        let (id, path) = (a.id, a.absolute_path.clone());
        self.pending_jobs += 1;
        std::thread::spawn(move || {
            let result = load_and_analyse(&engine, Path::new(&path));
            let _ = tx.send(JobResult::Reloaded { asset: id, doc_epoch: epoch, result });
        });
    }

    // ---------------------------------------------------------------- view

    pub fn scroll(&mut self, dx: f32, dy: f32, x: f32, zoom: bool) {
        if zoom {
            self.view.zoom_at((dy as f64 * 0.01).exp(), x);
        } else {
            self.view.start_beats -= dx as f64 / self.view.px_per_beat;
            self.view.scroll_y -= dy as f64;
            self.view.clamp();
        }
        let n = self.lane_tracks();
        self.view.clamp_scroll(n, self.lanes_size.1);
    }

    // ---------------------------------------------------------------- lane gestures

    fn beats_per_bar(&self) -> f64 {
        self.project().ticks_per_bar() as f64 / PPQ as f64
    }

    fn ruler_steps(&self) -> timeline::RulerSteps {
        let beat_len = 4.0 / self.project().transport.time_signature.denominator as f64;
        timeline::RulerSteps::new(self.view.px_per_beat, self.beats_per_bar(), beat_len, self.snap.grid)
    }

    pub fn hover(&self, x: f32, y: f32) -> i32 {
        if self.auto.shown && y >= timeline::RULER_H {
            return 0;
        }
        if self.fade_drag.is_some() || timeline::fade_hit(&self.view, &self.clip_vis, x, y).is_some() {
            return 1;
        }
        match timeline::hit_test(&self.view, self.lanes_size, self.lane_tracks(), &self.clip_vis, x, y) {
            Hit::Clip { zone: Zone::Left | Zone::Right, .. } => 1,
            Hit::Clip { zone: Zone::Body, .. } => 2,
            _ => 0,
        }
    }

    fn clip_region(&self, id: ClipId) -> Option<(TrackId, Ticks, u64, u64, f64, u64)> {
        let p = self.project();
        let (ti, ci) = p.find_clip(id)?;
        let c = &p.tracks[ti].clips[ci];
        let a = p.asset(c.asset)?;
        Some((p.tracks[ti].id, c.start, c.source_offset, c.source_length, a.sample_rate, a.frames))
    }

    /// Lane pointer with modifiers: automation rows take it while lanes are shown.
    pub fn pointer_mods(&mut self, kind: i32, x: f32, y: f32, shift: bool, alt: bool) {
        if self.drag.is_none() && self.section_pointer(kind, x, y) {
            return;
        }
        if kind == 0 && self.selected_section.take().is_some() {
            self.ui_dirty = true;
        }
        if self.drag.is_none() && self.auto_pointer(kind, x, y, shift, alt) {
            return;
        }
        // Fade handles come first: they sit over the clip's edges and body.
        match kind {
            0 if self.drag.is_none() && !shift && self.fade_press(x, y) => return,
            1 if self.fade_drag.is_some() => return self.fade_move(x),
            2.. if self.fade_drag.is_some() => return self.fade_release(),
            _ => {}
        }
        let beat = self.view.beat_at(x).max(0.0);
        match kind {
            0 => {
                let hit = timeline::hit_test(&self.view, self.lanes_size, self.lane_tracks(), &self.clip_vis, x, y);
                match hit {
                    Hit::Ruler => {
                        self.seek_beats(beat);
                        self.drag = Some(Drag::Scrub);
                    }
                    Hit::Clip { index, zone } => {
                        let id = self.clip_ids[index];
                        self.pr.focused = false;
                        if shift {
                            self.toggle_clip_selection(id);
                            return;
                        }
                        if let Some((ti, ci)) = self.project().find_midi_clip(id) {
                            let (track, c) = (self.project().tracks[ti].id, &self.project().tracks[ti].midi_clips[ci]);
                            let (start, len) = (c.start, c.length);
                            self.focus_clip(id);
                            self.select_track(track);
                            self.drag = Some(match zone {
                                Zone::Body => Drag::Move {
                                    clip: id,
                                    grab_beats: beat - start as f64 / PPQ as f64,
                                    orig_track: track,
                                    orig_start: start,
                                    track,
                                    start,
                                },
                                Zone::Left => Drag::MidiLeft { clip: id, orig: (start, len), cur: (start, len) },
                                Zone::Right => Drag::MidiRight { clip: id, orig: (start, len), cur: (start, len) },
                            });
                            self.ui_dirty = true;
                            return;
                        }
                        let Some((track, start, off, len, _, _)) = self.clip_region(id) else { return };
                        self.focus_clip(id);
                        self.select_track(track);
                        self.drag = Some(match zone {
                            Zone::Body => Drag::Move {
                                clip: id,
                                grab_beats: beat - start as f64 / PPQ as f64,
                                orig_track: track,
                                orig_start: start,
                                track,
                                start,
                            },
                            Zone::Left => Drag::TrimLeft { clip: id, orig: (start, off, len), cur: (start, off, len) },
                            Zone::Right => match self.project().clip(id).and_then(|c| c.clip_loop).filter(|l| l.on) {
                                Some(l) => Drag::LoopRight { clip: id, start, orig: l.length, cur: l.length },
                                None => Drag::TrimRight { clip: id, orig: (start, off, len), cur: (start, off, len) },
                            },
                        });
                    }
                    Hit::Track(i) => {
                        let id = self.project().tracks[i].id;
                        self.select_track(id);
                        if !shift {
                            self.select_clip(None);
                        }
                        self.pr.focused = false;
                        self.seek_beats(beat);
                        self.marquee_press(x, y, shift);
                    }
                    Hit::Empty => {
                        if !shift {
                            self.select_clip(None);
                        }
                        self.pr.focused = false;
                        self.marquee_press(x, y, shift);
                    }
                    Hit::Master => {}
                }
                self.ui_dirty = true;
            }
            1 => {
                if self.marquee.is_some() {
                    self.marquee_move(x, y);
                    return;
                }
                let Some(drag) = self.drag.clone() else { return };
                let (steps, on) = (self.ruler_steps(), self.snap_active());
                let snap = |b: f64| (if on { steps.snap(b) } else { b.max(0.0) } * PPQ as f64).round() as Ticks;
                match drag {
                    Drag::Scrub => self.seek_beats(beat),
                    Drag::Move { clip, grab_beats, orig_track, orig_start, .. } => {
                        let want = beat - grab_beats;
                        let start = (if on { steps.snap_move(want, orig_start as f64 / PPQ as f64) } else { want.max(0.0) } * PPQ as f64)
                            .round() as Ticks;
                        let p = self.project();
                        let kind = p.track(orig_track).map(|t| t.kind);
                        let track = self
                            .view
                            .track_at(y)
                            .and_then(|i| p.tracks.get(i))
                            .filter(|t| Some(t.kind) == kind)
                            .map(|t| t.id)
                            .unwrap_or(orig_track);
                        self.drag = Some(Drag::Move { clip, grab_beats, orig_track, orig_start, track, start });
                    }
                    Drag::TrimLeft { clip, orig, .. } => {
                        let p = self.project();
                        let Some(c) = p.clip(clip) else { return };
                        let (s0, off0, len0) = orig;
                        if c.warp.reverse {
                            // Reversed: the left edge plays the window's end, so it trims (or extends) that.
                            let l0 = p.clip_ticks_at(c, (off0 + len0) as f64);
                            let frames = p.asset(c.asset).map_or(off0 + len0, |a| a.frames);
                            let min_start = s0 + (l0 - p.clip_ticks_at(c, frames as f64)).ceil() as Ticks;
                            let max_start = s0 + l0 as Ticks - PPQ / 4;
                            let start = snap(beat).clamp(min_start.max(0), max_start.max(s0));
                            let end = p.clip_source_at(c, l0 - (start - s0) as f64).round().clamp((off0 + 1) as f64, frames as f64) as u64;
                            self.drag = Some(Drag::TrimLeft { clip, orig, cur: (start, off0, end - off0) });
                            self.ui_dirty = true;
                            return;
                        }
                        let at = |t: Ticks| p.clip_source_at(c, (t - s0) as f64);
                        let mut start = snap(beat);
                        // Source material before the current offset (warped clips follow their map).
                        let min_start = s0 + p.clip_ticks_at(c, 0.0).ceil() as Ticks;
                        let max_start = s0 + p.clip_ticks_at(c, (off0 + len0) as f64) as Ticks - PPQ / 4;
                        start = start.clamp(min_start.max(0), max_start.max(s0));
                        let off = at(start).round().max(0.0) as u64;
                        let len = (len0 as i64 - (off as i64 - off0 as i64)).max(1) as u64;
                        self.drag = Some(Drag::TrimLeft { clip, orig, cur: (start, off, len) });
                    }
                    Drag::MidiLeft { clip, orig, .. } => {
                        let end = orig.0 + orig.1;
                        let start = snap(beat).clamp(0, end - PPQ);
                        self.drag = Some(Drag::MidiLeft { clip, orig, cur: (start, end - start) });
                    }
                    Drag::MidiRight { clip, orig, .. } => {
                        let end = snap(beat).max(orig.0 + PPQ);
                        self.drag = Some(Drag::MidiRight { clip, orig, cur: (orig.0, end - orig.0) });
                    }
                    Drag::LoopRight { clip, start, orig, .. } => {
                        let cur = snap(beat).max(start + PPQ / 4) - start;
                        self.drag = Some(Drag::LoopRight { clip, start, orig, cur });
                    }
                    Drag::TrimRight { clip, orig, .. } => {
                        let Some((_, _, _, _, _, frames)) = self.clip_region(clip) else { return };
                        let (s0, off0, len0) = orig;
                        let p = self.project();
                        let Some(c) = p.clip(clip) else { return };
                        let end = snap(beat).max(s0 + PPQ / 4);
                        if c.warp.reverse {
                            // Reversed: the right edge plays the window's start.
                            let l0 = p.clip_ticks_at(c, (off0 + len0) as f64);
                            let end = end.min((s0 + (l0 - p.clip_ticks_at(c, 0.0)).floor() as Ticks).max(s0 + PPQ / 4));
                            let off = p.clip_source_at(c, l0 - (end - s0) as f64).round().clamp(0.0, (off0 + len0 - 1) as f64) as u64;
                            self.drag = Some(Drag::TrimRight { clip, orig, cur: (s0, off, off0 + len0 - off) });
                            self.ui_dirty = true;
                            return;
                        }
                        let len = (p.clip_source_at(c, (end - s0) as f64) - off0 as f64).round().max(1.0) as u64;
                        let len = len.clamp(1, frames - off0);
                        self.drag = Some(Drag::TrimRight { clip, orig, cur: (s0, off0, len) });
                    }
                }
                self.ui_dirty = true;
            }
            _ => {
                if self.marquee_release() {
                    return;
                }
                match self.drag.take() {
                    Some(Drag::Move { orig_track, orig_start, track, start, .. }) if !self.extra_clips.is_empty() => {
                        if (track, start) != (orig_track, orig_start) {
                            self.finish_group_move(orig_track, orig_start, track, start);
                        } else if !shift && let Some(c) = self.selected_clip {
                            // A click (no drag) on one of several selected clips selects it alone.
                            self.set_clip_selection(vec![c]);
                        }
                    }
                    Some(Drag::Move { clip, orig_track, orig_start, track, start, .. }) if (track, start) != (orig_track, orig_start) => {
                        let midi = self.project().find_midi_clip(clip).is_some();
                        self.execute(if midi {
                            Command::MoveMidiClip { clip, track, start }
                        } else {
                            Command::MoveClip { clip, track, start }
                        });
                        self.select_track(track);
                    }
                    Some(Drag::MidiRight { clip, orig, cur }) if orig != cur => {
                        self.execute(Command::ResizeMidiClip { clip, start: cur.0, length: cur.1 });
                    }
                    Some(Drag::MidiLeft { clip, orig, cur }) if orig != cur => {
                        if let Some(cmd) = self.project().midi_clip(clip).and_then(|c| pianoroll::midi_left_resize(c, cur.0)) {
                            self.execute(cmd);
                        }
                    }
                    Some(Drag::TrimLeft { clip, orig, cur } | Drag::TrimRight { clip, orig, cur }) if orig != cur => {
                        self.execute(Command::TrimClip { clip, start: cur.0, source_offset: cur.1, source_length: cur.2 });
                    }
                    Some(Drag::LoopRight { clip, orig, cur, .. }) if orig != cur => {
                        if let Some(l) = self.project().clip(clip).and_then(|c| c.clip_loop) {
                            self.execute(Command::SetClipLoop {
                                clip,
                                clip_loop: Some(dstudio_core::model::AudioLoop { length: cur, ..l }),
                            });
                        }
                    }
                    _ => {}
                }
                self.ui_dirty = true;
            }
        }
    }

    // ---------------------------------------------------------------- per-frame

    /// Waveform of an audio clip: its pyramid slot (once the peaks exist) and its pieces.
    pub fn clip_wave(&self, clip: &Clip) -> Option<(crate::peaks::Slot, Vec<crate::peaks::WaveSeg>)> {
        let slot = self.clip_engine_asset(clip).and_then(|ea| self.peaks.slot(ea))?;
        Some((slot, crate::peaks::wave_segments(self.project(), clip, self.rate)))
    }

    /// The clip editor's waveform: a lane per channel once `ensure_lane_peaks`
    /// has run for a stereo asset, else the mono mix.
    pub fn clip_lanes(&self, clip: &Clip) -> Option<(Vec<crate::peaks::Slot>, Vec<crate::peaks::WaveSeg>)> {
        let ea = self.clip_engine_asset(clip)?;
        let slots = match self.peaks.channel_slots(ea) {
            Some(lr) => lr.to_vec(),
            None => vec![self.peaks.slot(ea)?],
        };
        Some((slots, crate::peaks::wave_segments(self.project(), clip, self.rate)))
    }

    /// Engine asset a clip plays (the reversed copy when it plays back to front).
    fn clip_engine_asset(&self, clip: &Clip) -> Option<u64> {
        let assets = if clip.warp.reverse { &self.reversed_assets } else { &self.asset_map };
        assets.get(&clip.asset).copied()
    }

    /// Per-channel peaks of the open stereo audio clip, for the editor's two
    /// lanes. Computed once per engine asset (about a ms per channel-minute).
    pub fn ensure_lane_peaks(&mut self) {
        let Some(c) = self.ac_clip() else { return };
        if self.project().asset(c.asset).is_none_or(|a| a.channels != 2) {
            return;
        }
        let Some(ea) = self.clip_engine_asset(c) else { return };
        if self.peaks.channel_slots(ea).is_some() {
            return;
        }
        let Ok(info) = self.engine.asset_info(ea) else { return };
        let pairs = (info.frames / crate::peaks::L0_FRAMES as u64 + 1) as usize;
        let lane = |ch: u32| self.engine.channel_peaks(ea, ch, 0, crate::peaks::L0_FRAMES, pairs).map(crate::peaks::Pyramid::from_level0);
        if let (Ok(l), Ok(r)) = (lane(0), lane(1)) {
            self.peaks.insert_channels(ea, [l, r]);
        }
    }

    /// Rebuild the clip/track visuals (positions include in-progress drag previews).
    pub fn rebuild_vis(&mut self) {
        self.auto_sync_view();
        let p = self.doc.project();
        self.track_vis = p.tracks.iter().map(|t| TrackVis { selected: Some(t.id) == self.selected_track }).collect();
        self.section_vis = self.build_section_vis();
        let group = self.group_previews();
        self.summary_vis = self.group_summary();
        let p = self.doc.project();
        self.clip_vis.clear();
        self.clip_ids.clear();
        for (ti, t) in p.tracks.iter().enumerate() {
            // A folded group's tracks have no row to draw their clips in.
            if t.output.is_some_and(|g| self.collapsed.contains(&g)) {
                continue;
            }
            for c in &t.clips {
                let (mut track_index, mut start, mut off, mut len) = (ti, c.start, c.source_offset, c.source_length);
                let mut clip_loop = c.clip_loop;
                match &self.drag {
                    _ if group.contains_key(&c.id) => (track_index, start) = group[&c.id],
                    Some(Drag::Move { clip, track, start: s, .. }) if *clip == c.id => {
                        track_index = p.track_index(*track).unwrap_or(ti);
                        start = *s;
                    }
                    Some(Drag::TrimLeft { clip, cur, .. } | Drag::TrimRight { clip, cur, .. }) if *clip == c.id => {
                        (start, off, len) = *cur;
                    }
                    Some(Drag::LoopRight { clip, cur, .. }) if *clip == c.id => {
                        clip_loop = clip_loop.map(|l| dstudio_core::model::AudioLoop { length: *cur, ..l });
                    }
                    _ => {}
                }
                let shown = dstudio_core::Clip { source_offset: off, source_length: len, clip_loop, ..c.clone() };
                let len_beats = p.clip_length_ticks(&shown) as f64 / PPQ as f64;
                let wave = self.clip_wave(&shown);
                let fades = self.shown_fades(c.id, c.fades, p.clip_length_ticks(&shown));
                self.clip_vis.push(ClipVis {
                    track_index,
                    start_beats: start as f64 / PPQ as f64,
                    len_beats,
                    color: track_color(t.color),
                    selected: self.is_clip_selected(c.id),
                    muted: t.mute || c.disabled,
                    name: match c.current_take() {
                        Some(i) if c.takes.len() > 1 => format!("{} · Take {}", c.name, i + 1),
                        _ => c.name.clone(),
                    },
                    wave,
                    midi: None,
                    live: None,
                    fades,
                    fade_handles: self.shows_fade_handles(c.id),
                });
                self.clip_ids.push(c.id);
            }
            for c in &t.midi_clips {
                let (mut track_index, mut start, mut len) = (ti, c.start, c.length);
                match &self.drag {
                    _ if group.contains_key(&c.id) => (track_index, start) = group[&c.id],
                    Some(Drag::Move { clip, track, start: s, .. }) if *clip == c.id => {
                        track_index = p.track_index(*track).unwrap_or(ti);
                        start = *s;
                    }
                    Some(Drag::MidiLeft { clip, cur, .. } | Drag::MidiRight { clip, cur, .. }) if *clip == c.id => (start, len) = *cur,
                    _ => {}
                }
                // Notes as displayed: piano-roll gesture preview for the open clip.
                let notes = if self.pr.clip == Some(c.id) { self.pr_notes() } else { c.notes.clone() };
                let shift = start - c.start; // left-resize preview keeps notes in place on the timeline
                // A looping clip repeats its loop to the clip end; repeats are drawn dimmer.
                let looped =
                    if self.pr.clip == Some(c.id) { self.pr_loop_preview() } else { c.clip_loop }.filter(|l| l.on && c.pattern.is_none());
                let notes: Vec<(dstudio_core::Note, bool)> = match looped {
                    Some(l) => l.unroll(&notes, shift + len),
                    None => notes.into_iter().map(|n| (n, false)).collect(),
                };
                self.clip_vis.push(ClipVis {
                    track_index,
                    start_beats: start as f64 / PPQ as f64,
                    len_beats: len as f64 / PPQ as f64,
                    color: track_color(t.color),
                    selected: self.is_clip_selected(c.id),
                    muted: t.mute || c.disabled,
                    name: c.name.clone(),
                    wave: None,
                    midi: Some(mini_notes(&notes, shift, len)),
                    live: None,
                    fades: (0.0, 0.0),
                    fade_handles: false,
                });
                self.clip_ids.push(c.id);
            }
        }
        self.rebuild_live();
    }

    /// The clip move in progress: (original track, original start, track, start).
    pub(crate) fn move_drag(&self) -> Option<(TrackId, Ticks, TrackId, Ticks)> {
        match &self.drag {
            Some(Drag::Move { orig_track, orig_start, track, start, .. }) => Some((*orig_track, *orig_start, *track, *start)),
            _ => None,
        }
    }

    /// Track that holds a clip (audio or MIDI).
    pub fn clip_track(&self, id: ClipId) -> Option<TrackId> {
        let p = self.project();
        p.find_clip(id).or_else(|| p.find_midi_clip(id)).map(|(t, _)| p.tracks[t].id)
    }

    /// Select a clip (or none). Selecting a MIDI clip opens the Clip tab (piano roll).
    pub fn select_clip(&mut self, id: Option<ClipId>) {
        self.selected_clip = id;
        self.extra_clips.clear();
        if id.is_some() {
            self.selected_section = None;
        }
        if id.is_some_and(|c| self.project().find_midi_clip(c).is_some()) {
            self.bottom_tab = BottomTab::Clip;
        }
        self.pr_sync_clip();
        self.ui_dirty = true;
    }

    pub fn set_bottom_tab(&mut self, tab: BottomTab) {
        self.bottom_tab = tab;
        self.pr_sync_clip();
        self.ui_dirty = true;
    }

    pub fn scene<'a>(&'a self, scale: f32) -> timeline::Scene<'a> {
        let p = self.project();
        let tr = &p.transport;
        timeline::Scene {
            view: self.view.clone(),
            size: self.lanes_size,
            scale,
            tracks: &self.track_vis,
            clips: &self.clip_vis,
            summary: &self.summary_vis,
            live: &self.rec.live_vis,
            playhead_beats: self.playhead_ticks() as f64 / PPQ as f64,
            loop_beats: (tr.loop_start as f64 / PPQ as f64, tr.loop_end as f64 / PPQ as f64),
            loop_on: tr.loop_enabled,
            beats_per_bar: self.beats_per_bar(),
            steps: self.ruler_steps(),
            auto: None,
            sections: &self.section_vis,
        }
    }

    /// Push transport readouts, meters and (when dirty) models to Slint.
    /// Returns true while something is animating (playing, meters decaying, jobs).
    pub fn update_ui(&mut self, app: &AppWindow) -> bool {
        let s = app.global::<Studio>();
        {
            let scale = app.window().scale_factor();
            let sz = app.window().size();
            let win = (sz.width as f32 / scale, sz.height as f32 / scale);
            let sounds = s.get_sounds_visible();
            if win != self.devices.win || sounds != self.devices.sounds {
                self.devices.win = win;
                self.devices.sounds = sounds;
                self.ui_dirty = true;
            }
        }
        let tp = self.engine.transport();
        let pos = self.audible_frames();
        let p = self.doc.project();
        // Transport readouts from the engine (audible position, see audible_frames).
        let ticks = p.frames_to_ticks(pos, self.rate).max(0);
        let bar = ticks / p.ticks_per_bar() + 1;
        let secs = pos as f64 / self.rate;
        let ms_total = (secs * 1000.0).floor() as i64;
        let set_if = |cur: SharedString, new: String, f: &dyn Fn(SharedString)| {
            if cur.as_str() != new {
                f(new.into());
            }
        };
        set_if(s.get_bar_text(), format!("{bar:03}"), &|v| s.set_bar_text(v));
        set_if(s.get_time_text(), format!("{:02}:{:02}.{:03}", ms_total / 60_000, (ms_total / 1000) % 60, ms_total % 1000), &|v| {
            s.set_time_text(v)
        });
        if s.get_playing() != tp.playing {
            s.set_playing(tp.playing);
        }
        // App preference (not a document revision), so synced every frame.
        if s.get_metronome_on() != self.metronome_on {
            s.set_metronome_on(self.metronome_on);
        }

        let previewing = self.push_audition(&s);
        self.files.audition.animating = previewing;

        // Meters (master first, then tracks).
        self.engine.peek_meters(&mut self.meters, 256);
        let sel_id = self.selected_track.map(|t| t.0).unwrap_or(u64::MAX);
        let (nl, nr) = self.meters.iter().find(|m| m.track_id == sel_id).map(|m| (m.peak_l, m.peak_r)).unwrap_or((0.0, 0.0));
        let now = Instant::now();
        let dt = self.meter_t.map(|t| (now - t).as_secs_f32()).unwrap_or(0.0).min(0.25);
        self.meter_t = Some(now);
        let fall = db_to_gain(-24.0 * dt);
        let ml = nl.max(self.meter_disp.0 * fall);
        let mr = nr.max(self.meter_disp.1 * fall);
        self.meter_disp = (if ml < 1e-5 { 0.0 } else { ml }, if mr < 1e-5 { 0.0 } else { mr });
        let (ml, mr) = self.meter_disp;
        let fl = fader_from_db(gain_to_db(ml));
        let fr = fader_from_db(gain_to_db(mr));
        if (s.get_sel_meter_l() - fl).abs() > 1e-4 || (s.get_sel_meter_r() - fr).abs() > 1e-4 {
            s.set_sel_meter_l(fl);
            s.set_sel_meter_r(fr);
        }
        // Peak hold readout (since selection change), shown on the "−∞" button.
        self.sel_peak_hold = self.sel_peak_hold.max(ml.max(mr));
        let peak_text = fmt_db(gain_to_db(self.sel_peak_hold));
        if s.get_sel_peak_text().as_str() != peak_text {
            s.set_sel_peak_text(peak_text.into());
        }
        let mut metering = ml > 1e-5 || mr > 1e-5;

        // Track-header meters (always shown) and the mixer meters share one ballistics state.
        metering |= self.ballistics.update(&self.meters);
        let p = self.doc.project();
        let level = |id: u64| {
            let (l, r, hold) = self.ballistics.get(id);
            TrackLevel { l: header_meter(l), r: header_meter(r), clip: hold >= 1.0 }
        };
        let n = self.lane_tracks();
        let levels: Vec<TrackLevel> = p.tracks.iter().take(n).map(|t| level(t.id.0)).collect();
        sync_model(&self.track_levels_model, levels);
        let ml = level(MASTER_METER_ID);
        if s.get_master_level() != ml {
            s.set_master_level(ml);
        }

        // Mixer meters (all strips + master), only while the mixer is visible.
        if self.mixer_open {
            let p = self.doc.project();
            let rows: Vec<StripMeter> = p
                .tracks
                .iter()
                .map(|t| {
                    let (l, r, hold) = self.ballistics.get(t.id.0);
                    StripMeter { l: fader_from_db(gain_to_db(l)), r: fader_from_db(gain_to_db(r)), peak: mixer::fmt_peak(hold).into() }
                })
                .collect();
            sync_model(&self.strip_meters_model, rows);
            let (l, r, hold) = self.ballistics.get(MASTER_METER_ID);
            let (fl, fr) = (fader_from_db(gain_to_db(l)), fader_from_db(gain_to_db(r)));
            if (s.get_master_meter_l() - fl).abs() > 1e-4 || (s.get_master_meter_r() - fr).abs() > 1e-4 {
                s.set_master_meter_l(fl);
                s.set_master_meter_r(fr);
            }
            let pt = mixer::fmt_peak(hold);
            if s.get_master_peak_text().as_str() != pt {
                s.set_master_peak_text(pt.into());
            }
        }

        // Assistant highlights (pulse for HIGHLIGHT_SECS, then expire).
        let highlighting = !self.assistant.highlights.is_empty();
        if highlighting || self.highlights_model.row_count() > 0 {
            let scale = app.window().scale_factor();
            let size = app.window().size();
            let layout = self.highlight_layout(size.width as f32 / scale, size.height as f32 / scale, s.get_sounds_visible());
            let rects = self.highlight_rects(&layout);
            sync_model(&self.highlights_model, rects);
        }

        // Status expiry.
        if let Some((_, t0, d)) = &self.status
            && t0.elapsed() > *d
        {
            self.status = None;
            self.ui_dirty = true;
        }

        self.seq_tick(&s);
        if self.ui_rev != Some(self.doc.revision()) {
            self.ui_dirty = true;
        }
        if self.ui_dirty {
            self.ui_dirty = false;
            self.ui_rev = Some(self.doc.revision());
            self.push_models(app);
        }
        self.auto_sync_view();
        self.view.clamp_scroll(self.lane_tracks(), self.lanes_size.1);
        s.set_scroll_y(self.view.scroll_y as f32);
        self.update_labels();
        self.update_minimap(app);
        self.update_devmm(app);
        self.push_snap(&s);
        self.push_transport_fields(&s);
        s.set_press_epoch(self.press_epoch);
        s.set_doc_epoch(self.doc_epoch as i32);
        s.set_rename_open_id(self.selected_track.map_or(-1, |t| t.0 as i32));
        s.set_rename_open_epoch(self.rename_open_epoch);
        crate::pointer::apply(app, self.ptr.take());
        s.set_section_add_x(self.section_add_x());
        self.update_auto_labels();
        if self.auto_any_active() {
            self.push_auto_feedback(&s);
        }
        // The record button state follows the engine every frame while recording.
        if s.get_recording() != self.rec.recording {
            s.set_recording(self.rec.recording);
        }
        if s.get_counting_in() != self.counting_in() {
            s.set_counting_in(self.counting_in());
        }
        tp.playing
            || previewing
            || metering
            || highlighting
            || self.pending_jobs > 0
            || self.drag.is_some()
            || self.pr_animating()
            || self.rec.recording
            || self.test_tone_active()
            || self.devmm_animating()
    }

    fn push_models(&mut self, app: &AppWindow) {
        // The bottom panel's name field goes away with the panel (tab switch, mixer, selection
        // cleared) without seeing its focus loss: commit what it held first.
        let panel = (self.bottom_tab, self.mixer_open, self.selected_track.is_some());
        if self.rename_panel.is_some_and(|p| p != panel) && self.commit_rename_draft() {
            // The field held the keyboard focus: give it back to the shortcuts.
            self.press_epoch += 1;
        }
        self.rename_panel = Some(panel);
        self.rebuild_vis();
        let s = app.global::<Studio>();
        self.auto_sync_view();
        let rows = self.track_rows();
        let p = self.doc.project();
        if self.tracks_model.row_count() == rows.len() {
            for (i, r) in rows.into_iter().enumerate() {
                if self.tracks_model.row_data(i).as_ref() != Some(&r) {
                    self.tracks_model.set_row_data(i, r);
                }
            }
        } else {
            self.tracks_model.set_vec(rows);
        }
        // The folder's name once saved (a project folder renamed in the Finder shows its new name).
        let name = self.project_dir.as_deref().and_then(project_name_of).unwrap_or_else(|| p.name.clone());
        s.set_project_name(name.into());
        s.set_project_edited(self.doc.is_dirty());
        let sentence = dstudio_core::history::sentence_case;
        s.set_undo_text(self.doc.undo_label().map_or("Undo".into(), |l| format!("Undo {}", sentence(l))).into());
        s.set_redo_text(self.doc.redo_label().map_or("Redo".into(), |l| format!("Redo {}", sentence(l))).into());
        s.set_can_undo(self.doc.can_undo());
        s.set_can_redo(self.doc.can_redo());
        s.set_can_copy(self.selected_clip.is_some());
        s.set_project_on_disk(self.project_dir.is_some());
        s.set_loop_enabled(p.transport.loop_enabled);
        s.set_sig_text(format!("{}/{}", p.transport.time_signature.numerator, p.transport.time_signature.denominator).into());
        s.set_master_pos(slider_from_gain(p.master_gain));
        let sel = self.selected_track.and_then(|t| p.track(t).map(|tr| (p.track_index(t).unwrap(), tr)));
        s.set_has_selection(sel.is_some());
        if let Some((i, t)) = sel {
            s.set_sel_id(t.id.0 as i32);
            s.set_sel_number(i as i32 + 1);
            s.set_sel_name(t.name.clone().into());
            s.set_sel_instrument(t.kind == TrackKind::Instrument);
            s.set_sel_color(slint_color(track_color(t.color)));
            s.set_sel_mute(self.switch_now(Some(t.id), &dstudio_core::AutomationTarget::Mute, t.mute));
            s.set_sel_solo(t.solo);
            s.set_sel_pan(t.pan);
            s.set_sel_pan_text(fmt_pan(t.pan).into());
            let (trim, width, mono) = self.strip_now(t.id);
            s.set_sel_trim(trim);
            s.set_sel_trim_text(fmt_trim(trim).into());
            s.set_sel_width(width);
            s.set_sel_width_text(fmt_width(width).into());
            s.set_sel_mono(mono);
            let db = gain_to_db(t.gain);
            s.set_sel_fader(fader_from_db(db));
            s.set_sel_db_text(fmt_db(db).into());
        }
        // ---- mixer strips ----
        s.set_mixer_open(self.mixer_open);
        let first_return = p.ordinary_track_count();
        // Each strip's range in the strip-sends model (push_routing: ordinary tracks in order,
        // one send slot per return track).
        let slots = p.returns().count() as i32;
        let mut send_first = 0;
        let mut column = 0;
        // The group a strip belongs to (a group strip belongs to itself).
        fn group_of<'a>(p: &'a dstudio_core::Project, t: &'a dstudio_core::Track) -> Option<&'a dstudio_core::Track> {
            if t.is_group() { Some(t) } else { t.output.and_then(|g| p.track(g)).filter(|g| g.is_group()) }
        }
        let strips: Vec<MixerStrip> = p
            .tracks
            .iter()
            .enumerate()
            .map(|(i, t)| {
                let n = if t.is_bus() { 0 } else { slots };
                send_first += n;
                (i, t, send_first - n, n)
            })
            .map(|(i, t, first, n)| {
                let group = group_of(p, t);
                let next_group = p.tracks.get(i + 1).and_then(|n| group_of(p, n)).map(|g| g.id);
                let hidden = self.hidden_by_fold(t);
                let col = column;
                column += i32::from(!hidden);
                (i, t, first, n, group, next_group, hidden, col)
            })
            .map(|(i, t, first, n, group, next_group, hidden, col)| MixerStrip {
                column: col,
                hidden,
                bar: group.is_some(),
                bar_color: group.map(|g| slint_color(track_color(g.color))).unwrap_or_default(),
                bar_end: group.is_some_and(|g| next_group != Some(g.id) || self.collapsed.contains(&g.id)),
                folded: self.collapsed.contains(&t.id),
                group_muted: !t.is_group() && group.is_some_and(|g| g.mute),
                send_first: first,
                send_count: n,
                id: t.id.0 as i32,
                name: t.name.clone().into(),
                instrument: t.kind == TrackKind::Instrument,
                is_return: t.is_return(),
                is_group: t.is_group(),
                group_start: t.is_return() && i == first_return,
                color: slint_color(track_color(t.color)),
                selected: Some(t.id) == self.selected_track,
                mute: self.switch_now(Some(t.id), &dstudio_core::AutomationTarget::Mute, t.mute),
                solo: t.solo,
                pan: t.pan,
                pan_text: mixer::fmt_strip_pan(t.pan).into(),
                fader: fader_from_db(gain_to_db(t.gain)),
                gain_text: mixer::fmt_gain(gain_to_db(t.gain)).into(),
            })
            .collect();
        s.set_strip_columns(strips.iter().filter(|st| !st.hidden).count() as i32);
        s.set_mixer_group_lanes(i32::from(strips.iter().any(|st| st.bar)));
        sync_model(&self.strips_model, strips);
        s.set_master_gain_text(mixer::fmt_gain(gain_to_db(p.master_gain)).into());
        if self.strip_meters_model.row_count() != p.tracks.len() {
            self.strip_meters_model.set_vec(vec![StripMeter { l: 1.0, r: 1.0, peak: "−∞".into() }; p.tracks.len()]);
        }
        s.set_master_fader(fader_from_db(gain_to_db(p.master_gain)));
        s.set_master_mute(p.master_mute);
        // ---- assistant ----
        s.set_assistant_open(self.assistant_open);
        s.set_assistant_pref_w(match self.chat_drag {
            Some(d) => d.w,
            None => self.chat_saved_w(),
        });
        s.set_chat_available(self.provider.available());
        s.set_chat_busy(self.assistant.busy.is_some());
        s.set_chat_notice(self.assistant.notice_visible);
        s.set_teach_mode(self.assistant.mode == dstudio_core::assistant::Mode::Teach);
        if self.chat_rev != Some(self.assistant.rev) {
            self.chat_rev = Some(self.assistant.rev);
            self.chat_model.set_vec(self.assistant.to_model());
            self.assistant.fresh_from = self.assistant.entries.len();
        }
        match self.assistant_context() {
            Some((name, scope, color, instrument)) => {
                s.set_chat_context_visible(true);
                s.set_chat_context_name(name.into());
                s.set_chat_context_scope(scope.into());
                s.set_chat_context_color(slint_color(track_color(color)));
                s.set_chat_context_instrument(instrument);
            }
            None => s.set_chat_context_visible(false),
        }
        self.push_clip_editor(&s);
        self.push_synth(&s);
        self.push_recording(&s);
        self.push_routing(&s);
        self.push_settings(&s);
        self.push_dialogs(&s);
        self.push_clip_render(&s);
        self.push_devices(&s);
        self.push_files(&s);
        self.push_automation(&s);
        self.push_track_drag(&s);
        self.push_marquee(&s);
        s.set_bottom_h(self.bottom_h());
        s.set_bottom_collapsed(self.bottom_collapsed());
        let status = if self.show_status { self.status.as_ref().map(|s| s.0.clone()).unwrap_or_default() } else { String::new() };
        s.set_status_text(status.into());
        self.ruler_key.0 = i64::MIN; // force label refresh
    }

    fn update_labels(&mut self) {
        let v = self.view.clone();
        // Section names: 6 px (at 100% zoom) in, hidden under 10 px of room (sections.rs).
        let w = self.lanes_size.0;
        let section_labels: Vec<crate::SectionLabel> = self
            .section_vis
            .iter()
            .filter_map(|s| {
                let x0 = v.x_of(s.start_beats);
                let x1 = v.x_of(s.start_beats + s.len_beats);
                let lx = (x0 + 5.1).max(3.4);
                let room = x1 - 5.1 - lx;
                (room >= 8.5 && x1 > 0.0 && x0 < w).then(|| {
                    let ink = crate::sections::label_ink(crate::sections::lift(s.color, s.state));
                    crate::SectionLabel {
                        x: lx,
                        w: room,
                        text: s.name.clone().into(),
                        ink: slint::Color::from_rgb_f32(ink[0], ink[1], ink[2]),
                    }
                })
            })
            .collect();
        crate::controller::sync_model(&self.section_label_model, section_labels);
        let key = (
            (v.start_beats * 4096.0) as i64,
            (v.px_per_beat * 4096.0) as i64,
            self.lanes_size.0 as i64,
            (self.beats_per_bar() * 1024.0) as i64,
        );
        if key != self.ruler_key {
            self.ruler_key = key;
            let labels = timeline::ruler_labels(&v, self.lanes_size.0, &self.ruler_steps());
            self.ruler_model.set_vec(labels.into_iter().map(|(x, t)| RulerLabel { x, text: t.into() }).collect::<Vec<_>>());
        }
        let w = self.lanes_size.0;
        let bottom = self.lanes_size.1 - v.footer_h();
        let labels: Vec<ClipLabel> = self
            .clip_vis
            .iter()
            .chain(self.rec.live_vis.iter())
            .filter_map(|c| {
                let (x, y, cw, _) = c.rect(&v);
                if x > w || x + cw < 0.0 || y < timeline::RULER_H - 1.0 || y + timeline::CLIP_HEADER > bottom {
                    return None;
                }
                let x0 = x.max(0.0);
                Some(ClipLabel { x: x0, y, w: (x + cw).min(w) - x0, text: c.name.clone().into(), selected: c.selected })
            })
            .collect();
        let same = self.clips_model.row_count() == labels.len()
            && labels.iter().enumerate().all(|(i, l)| self.clips_model.row_data(i).as_ref() == Some(l));
        if !same {
            self.clips_model.set_vec(labels);
        }
    }
}

/// Mini note view of a MIDI clip: (rel start, rel length, rel pitch 0 top .. 1 bottom).
/// `shift` = ticks the clip start moved (left-resize preview).
pub(crate) fn mini_notes(notes: &[(dstudio_core::Note, bool)], shift: Ticks, len: Ticks) -> Vec<[f32; 4]> {
    if notes.is_empty() || len <= 0 {
        return Vec::new();
    }
    let lo = notes.iter().map(|n| n.0.pitch).min().unwrap_or(60) as f32;
    let hi = notes.iter().map(|n| n.0.pitch).max().unwrap_or(60) as f32;
    let pad = ((12.0 - (hi - lo)) / 2.0).max(0.0);
    let (lo, hi) = (lo - pad, hi + pad);
    notes
        .iter()
        .filter_map(|&(n, repeat)| {
            let s = n.start - shift;
            (s < len && s + n.length > 0).then(|| {
                let s0 = s.max(0);
                [
                    s0 as f32 / len as f32,
                    ((s + n.length).min(len) - s0) as f32 / len as f32,
                    (hi - n.pitch as f32) / (hi - lo).max(1.0),
                    if repeat { 0.5 } else { 1.0 },
                ]
            })
        })
        .collect()
}

/// Update a VecModel in place (row-wise) so Slint only re-renders changed rows.
/// Playhead on stop: with Return to start, playback resumes where the previous one started,
/// unless the playhead was moved since it stopped; Stop in place resumes where it stopped.
/// Where Play starts: a position the user chose since the last pause, else the last playback's
/// start after a pause (Return to start), else wherever the transport is (None).
/// The project's name for its folder `dir` (`Song.dsproj` is "Song").
fn project_name_of(dir: &Path) -> Option<String> {
    dir.file_stem().and_then(|s| s.to_str()).map(str::trim).filter(|s| !s.is_empty()).map(String::from)
}

/// A stopped transport's playhead (see [`Controller::playhead_state`]).
#[derive(Clone, Copy, Debug)]
pub(crate) struct PlayheadState {
    frame: i64,
    paused: bool,
    cue: Option<i64>,
}

fn play_from(stop_in_place: bool, play_start: Option<i64>, paused: bool, cue: Option<i64>) -> Option<i64> {
    cue.or(play_start.filter(|_| paused && !stop_in_place))
}

pub(crate) fn sync_model<T: Clone + PartialEq + 'static>(m: &VecModel<T>, rows: Vec<T>) {
    if m.row_count() == rows.len() {
        for (i, r) in rows.into_iter().enumerate() {
            if m.row_data(i).as_ref() != Some(&r) {
                m.set_row_data(i, r);
            }
        }
    } else {
        m.set_vec(rows);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn playhead_on_stop() {
        assert_eq!(play_from(false, Some(100), true, None), Some(100), "Return to start");
        assert_eq!(play_from(false, Some(100), false, Some(900)), Some(900), "a seek since the pause wins");
        assert_eq!(play_from(false, Some(100), false, Some(0)), Some(0), "Stop since the pause");
        assert_eq!(play_from(true, Some(100), true, None), None, "Stop in place");
        assert_eq!(play_from(true, Some(100), false, Some(900)), Some(900), "Stop in place still honours a seek");
        assert_eq!(play_from(false, None, false, None), None, "first playback");
    }
    #[test]
    fn slider_mapping_roundtrip() {
        for g in [0.0f32, 0.1, 0.5, 1.0, 1.5, 1.99] {
            let p = slider_from_gain(g);
            assert!((gain_from_slider(p) - g).abs() < 1e-3, "{g} -> {p}");
        }
        assert!((slider_from_gain(1.0) - 0.8).abs() < 1e-6);
    }
    #[test]
    fn pan_text_entry() {
        assert_eq!(parse_pan_text(" c "), Some(0.0));
        assert_eq!(parse_pan_text("30L"), Some(-0.3));
        assert_eq!(parse_pan_text("100 r"), Some(1.0));
        assert_eq!(parse_pan_text("250R"), Some(1.0));
        assert_eq!(parse_pan_text("-45"), Some(-0.45));
        assert_eq!(parse_pan_text("left"), None);
    }
    #[test]
    fn fader_mapping() {
        assert!((fader_from_db(0.0) - 0.2).abs() < 1e-4);
        assert!((fader_from_db(-24.0) - 0.78).abs() < 1e-4);
        assert!((db_from_fader(0.5) + 9.0).abs() < 1e-3);
        assert!((db_from_fader(fader_from_db(-30.0)) + 30.0).abs() < 1e-3);
        assert_eq!(fader_from_db(f32::NEG_INFINITY), 1.0);
        assert!(db_from_fader(1.0).is_infinite());
    }
    /// CHR-24: imports finish on worker threads in any order. A file meant for the selected audio
    /// track must land there even when a later import (on a new track) finishes first and
    /// selects its own track.
    #[test]
    fn import_target_survives_completion_order() {
        let engine = Arc::new(Engine::new(dstudio_engine::Config::default()).unwrap());
        let mut c = Controller::new(engine, Project::default());
        let dir = std::env::temp_dir().join(format!("dstudio-import-order-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (a, b) = (dir.join("a.wav"), dir.join("b.wav"));
        crate::wavgen::write_wav(&a, 48_000, 1, &vec![0.25; 4_800]).unwrap();
        crate::wavgen::write_wav(&b, 48_000, 1, &vec![0.25; 4_800]).unwrap();
        c.import_files(vec![a.clone()], ImportTarget::Selected, Some(0));
        c.import_files(vec![b], ImportTarget::NewTrack, Some(10 * PPQ));
        let mut done: Vec<JobResult> = (0..2).map(|_| c.jobs_rx.recv_timeout(Duration::from_secs(20)).expect("import result")).collect();
        done.sort_by_key(|j| matches!(j, JobResult::Imported { path, .. } if *path == a));
        for j in done {
            c.jobs_tx.send(j).unwrap();
        }
        c.poll_jobs();
        let _ = std::fs::remove_dir_all(&dir);
        let starts: Vec<Vec<Ticks>> = c.project().tracks.iter().map(|t| t.clips.iter().map(|k| k.start).collect()).collect();
        assert_eq!(starts, vec![vec![0], vec![], vec![10 * PPQ]]);
    }
    /// A loaded name with its own surrounding spaces: committing the field untouched keeps it and
    /// adds no undo step; a real edit is trimmed.
    #[test]
    fn an_untouched_rename_keeps_a_loaded_name() {
        let engine = Arc::new(Engine::new(dstudio_engine::Config::default()).unwrap());
        let mut p = Project::default();
        p.tracks[0].name = "  Bass  ".into();
        let mut c = Controller::new(engine, p);
        let (id, rev) = (c.project().tracks[0].id, c.doc.revision());
        c.rename_track(id, "  Bass  ");
        assert_eq!((c.project().tracks[0].name.as_str(), c.doc.revision()), ("  Bass  ", rev));
        c.rename_track(id, "  Sub bass ");
        assert_eq!(c.project().tracks[0].name, "Sub bass");
        assert_ne!(c.doc.revision(), rev);
    }
    /// A splitter drag forgets only its own panel's collapse-button height: the mixer, shut by
    /// the button at a custom height, still reopens there after the Devices panel is dragged.
    #[test]
    fn a_drag_keeps_another_panels_restore_height() {
        let engine = Arc::new(Engine::new(dstudio_engine::Config::default()).unwrap());
        let mut c = Controller::new(engine, Project::default());
        c.bottom_restore.insert("mixer", Some(400.0));
        c.bottom_restore.insert("devices", None);
        c.mixer_open = false;
        c.bottom_tab = BottomTab::Devices;
        c.bottom_resize(0, 500.0);
        c.bottom_resize(1, 400.0);
        c.bottom_resize(2, 400.0);
        assert_eq!(c.bottom_restore.get("mixer"), Some(&Some(400.0)));
        assert!(!c.bottom_restore.contains_key("devices"));
    }
    /// Two quick reverses of one audio clip are two undo steps: one Undo leaves it reversed.
    #[test]
    fn each_reverse_is_its_own_undo_step() {
        let engine = Arc::new(Engine::new(dstudio_engine::Config::default()).unwrap());
        let mut c = Controller::new(engine, Project::default());
        let (asset, clip, track) = (AssetId(9001), ClipId(9002), TrackId(9003));
        c.execute(Command::Batch {
            label: "Fixture".into(),
            commands: vec![
                Command::AddAsset {
                    asset: Asset {
                        id: asset,
                        relative_path: None,
                        absolute_path: "fixture.wav".into(),
                        sample_rate: 48_000.0,
                        channels: 2,
                        frames: 48_000,
                    },
                },
                Command::AddTrack { index: None, track: Track::new(track, TrackKind::Audio, "Audio", next_track_color(c.project())) },
                Command::AddClip {
                    track,
                    clip: Clip {
                        id: clip,
                        asset,
                        name: "Loop".into(),
                        start: 0,
                        source_offset: 0,
                        source_length: 48_000,
                        gain: 1.0,
                        warp: Default::default(),
                        disabled: false,
                        clip_loop: None,
                        fades: Default::default(),
                        takes: Vec::new(),
                    },
                },
            ],
        });
        c.select_clip(Some(clip));
        c.reverse_clip();
        c.reverse_clip();
        assert!(!c.project().clip(clip).unwrap().warp.reverse, "reversed twice");
        c.undo();
        assert!(c.project().clip(clip).unwrap().warp.reverse, "one Undo goes back to after the first reverse");
    }
    /// A master volume lane being drawn must not bring a muted master back.
    #[test]
    fn a_muted_master_ignores_a_lane_being_drawn() {
        let engine = Arc::new(Engine::new(dstudio_engine::Config::default()).unwrap());
        let mut c = Controller::new(engine, Project::default());
        let lane = dstudio_core::AutomationLane {
            target: dstudio_core::AutomationTarget::Volume,
            enabled: true,
            points: vec![dstudio_core::AutoPoint { time: 0, value: 1.0, curve: 0.0 }],
        };
        c.auto.pending = Some((None, lane));
        assert_eq!(c.current_snapshot().master_automation.len(), 1, "unmuted: the drawn lane plays");
        c.execute(Command::SetMasterMute { mute: true });
        let snap = c.current_snapshot();
        assert!(snap.master_automation.is_empty() && snap.master_gain == 0.0);
    }
    #[test]
    fn pan_text() {
        assert_eq!(fmt_pan(0.0), "C");
        assert_eq!(fmt_pan(-0.5), "50L");
        assert_eq!(fmt_pan(1.0), "100R");
    }
}
