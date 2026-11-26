//! Clip context menu renders: Consolidate (⌘J), Bounce › Bounce to new track (⌘B) and Export audio.
//!
//! All three render one track over the range of one clip or one group of clips, in the Stems
//! render's modes (`dstudio_core::render_plan`). The render runs on a worker thread that owns a
//! private offline engine (`dstudio_engine::offline`): the live engine, its device and the UI
//! thread are not involved, so playback and the interface carry on while it runs. The worker
//! renders from a copy of the project taken when the menu entry was chosen, writes the WAV, loads
//! it into the live engine and hands the result to `poll_jobs` as `JobResult::ClipRendered`, which
//! lands it as one undo step, unless the project was replaced, the track or a source clip changed
//! or the render was cancelled; then nothing is kept.
//!
//! A track with plugins (VST3, AU, CLAP) needs one more step first: plugins want their creation,
//! state and teardown on the main thread, so the UI thread prepares their instances in the
//! private engine, one plugin per `poll_jobs` tick (each starting from the live instance's
//! current state, so the file sounds like playback), then the worker renders, and the private
//! engine goes back to the UI thread with the result to be torn down there. The main thread is
//! never held for more than one plugin's creation or teardown at a time. A plugin that cannot be
//! read or loaded fails the render before anything is written.
//!
//! Files go to `<project>/media/joined` (Consolidate) and `<project>/media/bounce` (Bounce) once
//! the project is saved, else to `<temp>/Device Studio/Unsaved renders/`; like unsaved
//! recordings they move into the project when it is saved (src/unsaved_media.rs). A file only appears once its
//! render finished; a cancelled or failed render leaves no file (an Export is written beside its
//! destination as `<name>.part` and renamed, so an existing file is replaced only by a whole one).

use crate::Studio;
use crate::controller::{Controller, JobResult, load_and_analyse};
use crate::dialogs::{TAIL_S, create_free_file, file_safe};
use crate::peaks::Pyramid;
use crate::wavgen;
use dstudio_core::render_plan::{self, ClipSpan, RenderMode};
use dstudio_core::{Asset, AssetId, Clip, ClipId, Command, MidiClip, TrackId, TrackKind};
use dstudio_engine::offline::{self, OfflineError, OfflineRender, Session};
use dstudio_engine::{AssetInfo, Engine};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Instant;

/// Below this a rendered frame counts as silence when the tail after the clip is trimmed (−80 dBFS).
const SILENCE: f32 = 1e-4;

/// What a finished render is for.
pub enum ClipOp {
    /// Replace the span's clips by one audio clip.
    Consolidate { span: ClipSpan },
    /// New audio track below the clip's track holding the render; the source clip is disabled.
    Bounce { source: ClipId, span: ClipSpan },
    /// The whole track replaced by its render: an audio track with one clip, its devices off.
    BounceInPlace { span: ClipSpan },
    /// A WAV the user picked; `reveal` shows it in the file manager afterwards.
    Export { reveal: bool },
}

/// Where a render is written.
enum Output {
    /// A fresh file `<stem>.wav` (or `<stem> 2.wav`, …) in `dir`, loaded into the engine.
    New { dir: PathBuf, stem: String },
    /// `path`, at `bits` per sample.
    Export { path: PathBuf, bits: u16 },
}

/// A render that wrote its file (and, for Consolidate and Bounce, loaded it).
pub struct Rendered {
    pub path: PathBuf,
    pub loaded: Option<(AssetInfo, Pyramid)>,
    /// Milliseconds from the start of the render to the finished file.
    pub render_ms: f64,
}

pub struct ClipRendered {
    pub doc_epoch: u64,
    pub op: ClipOp,
    pub result: Result<Rendered, OfflineError>,
    /// The private engine of a render with plugins, sent back to be dropped on the UI thread.
    pub session: Option<Session>,
}

/// A clip as it was when the render began.
enum Source {
    Audio(Clip),
    Midi(MidiClip),
}

/// What the worker renders.
enum Work {
    Job(OfflineRender),
    /// A private engine with plugins, prepared on the UI thread.
    Session(Session),
}

/// A render whose plugins are still being prepared (UI thread, one per tick).
struct Preparing {
    session: Session,
    op: ClipOp,
    out: Output,
    rate: f64,
    body: usize,
    tail: usize,
    doc_epoch: u64,
}

/// The render in flight (one at a time).
pub struct ActiveRender {
    /// Some until the worker starts.
    preparing: Option<Preparing>,
    cancel: Arc<AtomicBool>,
    progress: Arc<AtomicUsize>,
    total: usize,
    /// Frames already shown in the interface.
    shown: usize,
    label: &'static str,
    cancelled: bool,
    track: TrackId,
    sources: Vec<Source>,
    /// Selection when it began: the result is selected only if it is still the same.
    selection: (Option<TrackId>, Option<ClipId>),
}

#[derive(Default)]
pub struct ClipRender {
    pub active: Option<ActiveRender>,
    /// Overrides the render folder (self-test scratch directory).
    pub dir_override: Option<PathBuf>,
    /// Test hook: where the next Export audio writes, instead of asking with a file dialog.
    pub export_path: Option<PathBuf>,
    /// Export audio waits for the file dialog the main loop runs unborrowed: (clip, suggested name).
    pub dialog_request: Option<(ClipId, String)>,
    /// Milliseconds the last finished render took on its worker.
    pub last_ms: f64,
    /// The longest single piece of a render the UI thread did itself: copying the project,
    /// preparing one plugin, tearing the private engine down. Milliseconds, the maximum over
    /// the renders since the app started.
    pub ui_stall_ms: f64,
}

impl ClipRender {
    /// A render is between its start and its landing: the menu entries wait.
    pub fn busy(&self) -> bool {
        self.active.is_some()
    }
}

/// The platform's file manager, as menus name it ("Show in Finder").
pub const FILE_MANAGER: &str = if cfg!(target_os = "macos") {
    "Finder"
} else if cfg!(windows) {
    "Explorer"
} else {
    "Files"
};

/// Open the platform's file manager on `path`.
pub fn reveal(path: &str) -> Result<(), String> {
    let p = Path::new(path);
    let spawned = if cfg!(target_os = "macos") {
        std::process::Command::new("open").arg("-R").arg(p).spawn()
    } else if cfg!(windows) {
        std::process::Command::new("explorer").arg(format!("/select,{path}")).spawn()
    } else {
        std::process::Command::new("xdg-open").arg(p.parent().map(Path::to_path_buf).unwrap_or_default()).spawn()
    };
    spawned.map(|_| ()).map_err(|e| e.to_string())
}

/// Writes `samples` (interleaved stereo) to the output and, for a new file, loads it into
/// `engine`. A failure or a cancel leaves no file behind.
fn write_render(
    engine: &Engine,
    samples: &[f32],
    rate: f64,
    out: &Output,
    cancel: &AtomicBool,
) -> Result<(PathBuf, Option<(AssetInfo, Pyramid)>), OfflineError> {
    let failed = |what: &Path, e: String| OfflineError::Failed(format!("{}: {e}", what.display()));
    let (path, target, bits) = match out {
        Output::New { dir, stem } => {
            std::fs::create_dir_all(dir).map_err(|e| failed(dir, e.to_string()))?;
            let (path, _) = create_free_file(dir, stem, "wav").map_err(|e| failed(dir, e))?;
            (path.clone(), path, 24)
        }
        Output::Export { path, bits } => {
            let mut part = path.clone().into_os_string();
            part.push(".part");
            (path.clone(), PathBuf::from(part), *bits)
        }
    };
    let run = || {
        let file = std::fs::File::create(&target).map_err(|e| failed(&target, e.to_string()))?;
        wavgen::encode_wav_pcm(file, rate.round() as u32, 2, bits, samples).map_err(|e| failed(&target, e.to_string()))?;
        if cancel.load(Ordering::Relaxed) {
            return Err(OfflineError::Cancelled);
        }
        match out {
            Output::Export { .. } => {
                std::fs::rename(&target, &path).map_err(|e| failed(&path, e.to_string()))?;
                Ok(None)
            }
            Output::New { .. } => load_and_analyse(engine, &path).map(Some).map_err(OfflineError::Failed),
        }
    };
    let loaded = run();
    if loaded.is_err() {
        let _ = std::fs::remove_file(&target);
    }
    loaded.map(|l| (path, l))
}

impl Controller {
    fn render_dir(&self, kind: &str) -> PathBuf {
        if let Some(d) = &self.clip_render.dir_override {
            return d.join(kind);
        }
        match &self.project_dir {
            Some(p) => p.join("media").join(kind),
            None => self.unsaved_root.join("Unsaved renders").join(kind),
        }
    }

    /// Why clip `id` cannot be rendered, or None. `needs_notes`: an empty MIDI clip has no sound.
    fn render_block(&self, id: ClipId, needs_notes: bool) -> Option<&'static str> {
        let p = self.project();
        if let Some(c) = p.clip(id) {
            return if c.disabled {
                Some("the clip is disabled")
            } else if !self.asset_map.contains_key(&c.asset) {
                Some("the audio file is missing")
            } else {
                None
            };
        }
        match p.midi_clip(id) {
            Some(c) if c.disabled => Some("the clip is disabled"),
            Some(c) if needs_notes && c.notes.is_empty() => Some("the clip has no notes"),
            Some(_) => None,
            None => Some("the clip is gone"),
        }
    }

    /// Whether the menu entry can run on clip `id`.
    pub(crate) fn can_render_clip(&self, id: ClipId, needs_notes: bool) -> bool {
        self.render_block(id, needs_notes).is_none()
    }

    /// Starts rendering `span` alone (see the module comment). The rendered range is the span
    /// plus, when the track's sound rings out, a tail trimmed at silence. A track without plugins
    /// renders on a worker at once; one with plugins first has its instances prepared here, one per
    /// tick (`prepare_render_step`). The interface only blocks for the copy of the project the
    /// render works from.
    fn start_render(&mut self, op: ClipOp, span: &ClipSpan, mode: RenderMode, out: Output, label: &'static str) {
        let t0 = Instant::now();
        let rate = self.engine.sample_rate();
        let p = self.project();
        let start = p.ticks_to_frames(span.start, rate);
        let body = (p.ticks_to_frames(span.end, rate) - start).max(1) as usize;
        let tail = if render_plan::rings_out(p, span.track, mode) { (TAIL_S * rate) as usize } else { 0 };
        let project = render_plan::render_project(p, span, mode);
        let sources: Vec<Source> = span
            .clips
            .iter()
            .filter_map(|&c| p.clip(c).cloned().map(Source::Audio).or_else(|| p.midi_clip(c).cloned().map(Source::Midi)))
            .collect();
        let job = OfflineRender { project, rate, transients: self.transients_by_asset(), start, frames: body + tail };
        let plugins = !job.plugin_devices().is_empty();
        let doc_epoch = self.doc_epoch;
        self.clip_render.active = Some(ActiveRender {
            preparing: None,
            cancel: Arc::new(AtomicBool::new(false)),
            progress: Arc::new(AtomicUsize::new(0)),
            total: body + tail,
            shown: 0,
            label,
            cancelled: false,
            track: span.track,
            sources,
            selection: (self.selected_track, self.selected_clip),
        });
        self.pending_jobs += 1;
        self.ui_dirty = true;
        if !plugins {
            self.spawn_render(Work::Job(job), op, out, rate, body, tail, doc_epoch);
        } else {
            match Session::new(job) {
                Ok(session) => {
                    if let Some(a) = self.clip_render.active.as_mut() {
                        a.preparing = Some(Preparing { session, op, out, rate, body, tail, doc_epoch });
                    }
                }
                Err(e) => self.fail_render(&op, e),
            }
        }
        self.note_ui_stall(t0);
    }

    /// Runs the render on a worker thread; its result (and the private engine of a render with
    /// plugins) comes back through `poll_jobs`.
    fn spawn_render(&mut self, work: Work, op: ClipOp, out: Output, rate: f64, body: usize, tail: usize, doc_epoch: u64) {
        let Some(a) = self.clip_render.active.as_ref() else { return };
        let (c2, p2) = (a.cancel.clone(), a.progress.clone());
        let (engine, tx) = (self.engine.clone(), self.jobs_tx.clone());
        std::thread::Builder::new()
            .name("clip-render".into())
            .spawn(move || {
                let t0 = Instant::now();
                let mut session = None;
                let result = (|| {
                    let mut samples = match work {
                        Work::Job(job) => offline::render(job, &c2, &p2)?,
                        Work::Session(s) => {
                            let rendered = s.render(&c2, &p2);
                            session = Some(s);
                            rendered?
                        }
                    };
                    let last = samples.chunks_exact(2).rposition(|f| f[0].abs() > SILENCE || f[1].abs() > SILENCE);
                    samples.truncate(last.map_or(0, |i| i + 1).clamp(body, body + tail) * 2);
                    let (path, loaded) = write_render(&engine, &samples, rate, &out, &c2)?;
                    Ok(Rendered { path, loaded, render_ms: t0.elapsed().as_secs_f64() * 1e3 })
                })();
                let _ = tx.send(JobResult::ClipRendered(Box::new(ClipRendered { doc_epoch, op, result, session })));
            })
            .expect("spawn clip render thread");
    }

    /// The render ended before its worker started (a plugin could not be read or loaded): say
    /// why and leave nothing behind.
    fn fail_render(&mut self, op: &ClipOp, e: OfflineError) {
        self.clip_render.active = None;
        self.pending_jobs = self.pending_jobs.saturating_sub(1);
        self.ui_dirty = true;
        let what = if matches!(op, ClipOp::Export { .. }) { "Export" } else { "Render" };
        match e {
            OfflineError::Cancelled => self.set_status("Render cancelled", 3),
            OfflineError::Failed(e) => self.set_status(format!("{what} failed: {e}"), 6),
        }
    }

    fn note_ui_stall(&mut self, since: Instant) {
        self.clip_render.ui_stall_ms = self.clip_render.ui_stall_ms.max(since.elapsed().as_secs_f64() * 1e3);
    }

    /// Prepares the next plugin of the render waiting for its instances (one per call, so the
    /// interface is never held for more than one plugin's creation) and starts the worker when
    /// the last one is ready.
    fn prepare_render_step(&mut self) {
        let Some(mut prep) = self.clip_render.active.as_mut().and_then(|a| a.preparing.take()) else { return };
        if prep.doc_epoch != self.doc_epoch {
            self.clip_render.active = None;
            self.pending_jobs = self.pending_jobs.saturating_sub(1);
            self.ui_dirty = true;
            return;
        }
        let t0 = Instant::now();
        match prep.session.prepare_next(Some(&self.engine)) {
            Err(e) => {
                self.fail_render(&prep.op, e);
                self.note_ui_stall(t0);
                return;
            }
            Ok(_) => self.ui_dirty = true,
        }
        let (done, total) = prep.session.plugins();
        if done < total {
            if let Some(a) = self.clip_render.active.as_mut() {
                a.preparing = Some(prep);
            }
        } else {
            let Preparing { session, op, out, rate, body, tail, doc_epoch } = prep;
            self.spawn_render(Work::Session(session), op, out, rate, body, tail, doc_epoch);
        }
        self.note_ui_stall(t0);
    }

    /// The Cancel button and Esc: stop the render in flight; nothing is kept.
    pub fn cancel_clip_render(&mut self) -> bool {
        let Some(a) = self.clip_render.active.as_mut() else { return false };
        self.ui_dirty = true;
        if a.preparing.is_some() {
            let t0 = Instant::now();
            self.clip_render.active = None;
            self.pending_jobs = self.pending_jobs.saturating_sub(1);
            self.set_status("Render cancelled", 3);
            self.note_ui_stall(t0);
            return true;
        }
        a.cancelled = true;
        a.cancel.store(true, Ordering::Relaxed);
        true
    }

    /// Called from `poll_jobs`: prepares plugins, and the interface redraws when the render moved on.
    pub(crate) fn clip_render_tick(&mut self) {
        self.prepare_render_step();
        if let Some(a) = self.clip_render.active.as_mut() {
            let now = a.progress.load(Ordering::Relaxed);
            if now != a.shown {
                a.shown = now;
                self.ui_dirty = true;
            }
        }
    }

    /// The progress line at the bottom right of the window.
    pub(crate) fn push_clip_render(&self, s: &Studio) {
        let Some(a) = &self.clip_render.active else {
            s.set_render_busy(false);
            return;
        };
        let fraction = if a.total == 0 { 0.0 } else { a.shown as f32 / a.total as f32 };
        s.set_render_busy(true);
        s.set_render_progress(fraction);
        s.set_render_label(match &a.preparing {
            _ if a.cancelled => "Cancelling…".into(),
            Some(p) => format!("{}  plugin {}/{}", a.label, p.session.plugins().0 + 1, p.session.plugins().1).into(),
            None => format!("{}  {} %", a.label, (fraction * 100.0).round() as i32).into(),
        });
    }

    /// Consolidate and Bounce write a new file named after the track under `media/<kind>`.
    fn new_output(&self, span: &ClipSpan, kind: &str, suffix: &str) -> Output {
        let track = self.project().track(span.track).map(|t| t.name.clone()).unwrap_or_default();
        Output::New { dir: self.render_dir(kind), stem: format!("{}-{suffix}", file_safe(&track, 40).replace(' ', "-")) }
    }

    // ---------------------------------------------------------------- Consolidate (⌘J)

    /// Joins clip `id` with the clips of its track that overlap it into one clip over their range.
    /// Audio is rendered from the track alone (no devices, fader or pan) and replaces the clips
    /// with one unwarped clip; MIDI clips merge their notes. One undo step.
    pub fn consolidate_clip(&mut self, id: ClipId) {
        if self.clip_render.busy() {
            return self.set_status("Still rendering. One moment.", 3);
        }
        let p = self.project();
        let Some(span) = render_plan::consolidate_group(p, id) else {
            return self.set_status("Consolidate: the clip is disabled or gone", 4);
        };
        if let Some(why) = span.clips.iter().find_map(|&c| self.render_block(c, false)) {
            return self.set_status(format!("Consolidate: {why}"), 4);
        }
        if span.kind == TrackKind::Instrument {
            let Some(mut merged) = render_plan::merge_midi(p, &span) else { return };
            merged.id = ClipId(p.next_id);
            let new_id = merged.id;
            let mut commands: Vec<Command> = span.clips.iter().map(|&clip| Command::RemoveMidiClip { clip }).collect();
            commands.push(Command::AddMidiClip { track: span.track, clip: merged });
            let n = span.clips.len();
            if self.execute(Command::Batch { label: "Consolidate Clips".into(), commands }) {
                self.select_clip(Some(new_id));
                self.set_status(format!("Consolidated {n} clip{}", if n == 1 { "" } else { "s" }), 3);
            }
            return;
        }
        let out = self.new_output(&span, "joined", "merged");
        self.start_render(ClipOp::Consolidate { span: span.clone() }, &span, RenderMode::Material, out, "Consolidating Clips");
    }

    // ---------------------------------------------------------------- Bounce to new track (⌘B)

    /// Renders clip `id` through its track's devices (neutral fader and pan) to a new audio clip
    /// on a new track below, which takes the track's fader and pan; the source clip is disabled.
    /// A MIDI clip becomes audio. One undo step.
    pub fn bounce_clip(&mut self, id: ClipId) {
        if self.clip_render.busy() {
            return self.set_status("Still rendering. One moment.", 3);
        }
        if let Some(why) = self.render_block(id, true) {
            return self.set_status(format!("Bounce: {why}"), 4);
        }
        let Some(span) = render_plan::clip_span(self.project(), id) else { return };
        let out = self.new_output(&span, "bounce", "bounce");
        self.start_render(ClipOp::Bounce { source: id, span: span.clone() }, &span, RenderMode::Bounce, out, "Bouncing to New Track");
    }

    /// Bounce Track in Place: every enabled clip of `track` rendered through its devices (neutral
    /// fader and pan, with the tail of a ringing track) to one audio clip from the first clip's
    /// start. The track becomes an audio track holding that clip, keeping its name, colour, mix,
    /// sends, output and lanes; its effects and instrument stay, switched off. One undo step.
    pub fn bounce_track_in_place(&mut self, track: TrackId) {
        if self.clip_render.busy() {
            return self.set_status("Still rendering. One moment.", 3);
        }
        let Some(span) = render_plan::track_span(self.project(), track) else {
            return self.set_status("Bounce: the track has no clips to bounce", 4);
        };
        if let Some(why) = span.clips.iter().find_map(|&c| self.render_block(c, false)) {
            return self.set_status(format!("Bounce: {why}"), 4);
        }
        let out = self.new_output(&span, "bounce", "bounce");
        self.start_render(ClipOp::BounceInPlace { span: span.clone() }, &span, RenderMode::Bounce, out, "Bouncing Track in Place");
    }

    /// The audio clip editor's Resample button: the open clip rendered in place through its
    /// track's devices, the way Bounce to new track does it. Rendering onto the same track would
    /// run the devices twice, so the printed clip lands on a new track below at the same time
    /// (taking the fader and pan) and the source clip is disabled; its file stays on disk. One
    /// undo step.
    pub fn resample_open_clip(&mut self) {
        match self.ac_clip().map(|c| c.id) {
            Some(id) => self.bounce_clip(id),
            None => self.set_status("Resample: open an audio clip first", 3),
        }
    }

    // ---------------------------------------------------------------- Export audio

    /// The name the save dialog offers: "<track> <clip>.wav".
    pub fn export_clip_name(&self, id: ClipId) -> String {
        let p = self.project();
        let clip = p.clip(id).map(|c| c.name.clone()).or_else(|| p.midi_clip(id).map(|c| c.name.clone())).unwrap_or_default();
        let track = self.clip_track(id).and_then(|t| p.track(t)).map(|t| t.name.clone()).unwrap_or_default();
        let name = file_safe(&format!("{} {}", track.trim(), clip.trim()), 80);
        format!("{}.wav", if name.is_empty() { "Clip" } else { &name })
    }

    /// Export audio to Finder: asks for a file (the main loop runs the dialog with the controller
    /// unborrowed; the self-test sets `export_path` instead), then `export_clip`.
    pub fn export_clip_request(&mut self, id: ClipId) {
        if let Some(why) = self.render_block(id, true) {
            return self.set_status(format!("Export: {why}"), 4);
        }
        match self.clip_render.export_path.take() {
            Some(path) => self.export_clip(id, &path, false),
            None => self.clip_render.dialog_request = Some((id, self.export_clip_name(id))),
        }
    }

    /// Writes clip `id`'s range, as it sounds on its track (devices, fader, pan, with the tail of
    /// a ringing track), to `path` as a stereo WAV at the export dialog's bit depth.
    pub fn export_clip(&mut self, id: ClipId, path: &Path, reveal: bool) {
        if self.clip_render.busy() {
            return self.set_status("Still rendering. One moment.", 3);
        }
        if let Some(why) = self.render_block(id, true) {
            return self.set_status(format!("Export: {why}"), 4);
        }
        let Some(span) = render_plan::clip_span(self.project(), id) else { return };
        let bits = if self.export.bits24 { 24 } else { 16 };
        self.start_render(
            ClipOp::Export { reveal },
            &span,
            RenderMode::Stem,
            Output::Export { path: path.to_path_buf(), bits },
            "Exporting Audio",
        );
    }

    // ---------------------------------------------------------------- landing

    /// Whether the track and every clip the render came from are still as they were.
    fn render_sources_unchanged(&self, a: &ActiveRender) -> Result<(), &'static str> {
        let p = self.project();
        if p.track(a.track).is_none() {
            return Err("the track is gone");
        }
        for s in &a.sources {
            let (found, same) = match s {
                Source::Audio(c) => (p.clip(c.id).is_some(), p.clip(c.id) == Some(c)),
                Source::Midi(c) => (p.midi_clip(c.id).is_some(), p.midi_clip(c.id) == Some(c)),
            };
            if !found {
                return Err("the clip is gone");
            }
            if !same {
                return Err("the clip changed");
            }
        }
        Ok(())
    }

    /// A worker finished: put the render in the project (one Batch) or report the failure.
    pub(crate) fn finish_clip_render(&mut self, r: ClipRendered) {
        let ClipRendered { doc_epoch, op, result, session } = r;
        let t0 = Instant::now();
        drop(session);
        self.note_ui_stall(t0);
        let Some(active) = self.clip_render.active.take() else { return };
        self.ui_dirty = true;
        let is_export = matches!(op, ClipOp::Export { .. });
        let rendered = match result {
            Ok(r) => r,
            Err(OfflineError::Cancelled) => return self.set_status("Render cancelled", 3),
            Err(OfflineError::Failed(e)) => {
                return self.set_status(format!("{} failed: {e}", if is_export { "Export" } else { "Render" }), 6);
            }
        };
        self.clip_render.last_ms = rendered.render_ms;
        let Rendered { path, loaded, .. } = rendered;
        if let (ClipOp::Export { reveal: show }, None) = (&op, &loaded) {
            self.set_status(format!("Exported {}", path.display()), 5);
            if *show {
                let _ = reveal(&path.to_string_lossy());
            }
            return;
        }
        let Some((info, pyr)) = loaded else { return };
        let discard = |this: &mut Self, why: &str| {
            let _ = this.engine.release_asset(info.asset_id);
            let _ = std::fs::remove_file(&path);
            if !why.is_empty() {
                this.set_status(format!("Render discarded: {why}"), 5);
            }
        };
        if active.cancelled {
            discard(self, "");
            return self.set_status("Render cancelled", 3);
        }
        if doc_epoch != self.doc_epoch {
            return discard(self, "");
        }
        if let Err(why) = self.render_sources_unchanged(&active) {
            return discard(self, &format!("{why} while it rendered"));
        }
        let p = self.project();
        let mut next = p.next_id;
        let mut alloc = || {
            next += 1;
            next - 1
        };
        let asset_id = AssetId(alloc());
        let native_frames = (info.frames as f64 * info.source_sample_rate / self.rate).round().max(1.0) as u64;
        let asset = Asset {
            id: asset_id,
            relative_path: None,
            absolute_path: std::fs::canonicalize(&path).unwrap_or_else(|_| path.clone()).to_string_lossy().into_owned(),
            sample_rate: info.source_sample_rate,
            channels: info.channels,
            frames: native_frames,
        };
        let clip_id = ClipId(alloc());
        let new_clip = |name: String, start: dstudio_core::Ticks| Clip {
            id: clip_id,
            asset: asset_id,
            name,
            start: start.max(0),
            source_offset: 0,
            source_length: native_frames,
            gain: 1.0,
            warp: Default::default(),
            disabled: false,
            clip_loop: None,
            fades: Default::default(),
            takes: Vec::new(),
        };
        let mut cmds = vec![Command::AddAsset { asset }];
        let (label, track, status) = match &op {
            ClipOp::Consolidate { span } => {
                cmds.extend(span.clips.iter().map(|&clip| Command::RemoveClip { clip }));
                let first = p.clip(span.clips[0]).map(|c| c.name.clone()).unwrap_or_default();
                cmds.push(Command::AddClip { track: span.track, clip: new_clip(render_plan::merged_name(&first), span.start) });
                let n = span.clips.len();
                ("Consolidate Clips", span.track, format!("Consolidated {n} clip{}", if n == 1 { "" } else { "s" }))
            }
            ClipOp::Bounce { source, span } => {
                let (Some(src), Some(index)) = (p.track(span.track), p.track_index(span.track)) else {
                    return discard(self, "the track is gone while it rendered");
                };
                let name =
                    p.clip(*source).map(|c| c.name.clone()).or_else(|| p.midi_clip(*source).map(|c| c.name.clone())).unwrap_or_default();
                let id = TrackId(alloc());
                let mut t = render_plan::bounce_track(src, id);
                t.clips.push(new_clip(name, span.start));
                cmds.push(Command::AddTrack { index: Some(index + 1), track: t });
                cmds.push(Command::SetClipEnabled { clip: *source, enabled: false });
                ("Bounce to New Track", id, "Bounced to a new track".to_string())
            }
            ClipOp::BounceInPlace { span } => {
                // Clips added or removed while it rendered would be lost or doubled.
                if render_plan::track_span(p, span.track).map(|s| s.clips) != Some(span.clips.clone()) {
                    return discard(self, "the track changed while it rendered");
                }
                let Some(src) = p.track(span.track) else { return discard(self, "the track is gone while it rendered") };
                let t = render_plan::bounced_in_place(src, new_clip(src.name.trim().to_string(), span.start));
                cmds.push(Command::ReplaceTrack { track: t });
                ("Bounce Track in Place", span.track, "Bounced the track in place".to_string())
            }
            ClipOp::Export { .. } => return,
        };
        self.peaks.insert(info.asset_id, pyr);
        self.asset_map.insert(asset_id, info.asset_id);
        let keep_selection = (self.selected_track, self.selected_clip) == active.selection;
        if self.execute(Command::Batch { label: label.into(), commands: cmds }) {
            if keep_selection {
                self.select_track(track);
                self.select_clip(Some(clip_id));
            }
            self.set_status(status, 3);
        } else {
            self.asset_map.remove(&asset_id);
            let _ = self.engine.release_asset(info.asset_id);
            let _ = std::fs::remove_file(&path);
        }
    }
}
