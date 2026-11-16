//! Offline render of a project on a private engine, for jobs that run on a worker thread (the
//! clip menu's Consolidate, Bounce and Export audio, src/clip_render.rs in the app).
//!
//! The engine of the running app is never touched: the render makes its own [`Engine`] without
//! an audio device, loads the project's audio files into it, publishes the project and plays it
//! through [`Engine::render_offline`] in chunks.
//!
//! **Plugins (VST3, AU, CLAP).** They want their creation, state, latency and teardown on the
//! main thread (a CLAP plugin calls its creating thread its main thread, and
//! [`Engine::publish`] creates), while a render must not hold the main thread. So a render with
//! plugins is split in three:
//!
//! 1. [`Session::new`] and [`Session::prepare_next`] run on the main thread, one plugin per call (the app
//!    calls one per UI tick): each instance is created in the private engine with the current state of the
//!    live instance, so the render sounds like playback.
//! 2. [`Session::render`] runs anywhere (the worker): it publishes with the engine detached
//!    ([`Engine::set_plugin_detached`]: it reuses the prepared instances, creates, restarts and
//!    destroys nothing), renders the plugin delay compensation's latency L frames more than asked
//!    and drops the first L, so the file is aligned to the timeline like the audible playback.
//! 3. Dropping the [`Session`] destroys the instances; do it on the main thread.
//!
//! Plugins run in real-time process mode, like in playback: a plugin that sounds different when
//! it knows it is offline would make the file differ from what the user hears.

use crate::{Config, Engine, EngineError, MetronomeConfig};
use dstudio_core::snapshot;
use dstudio_core::{AssetId, Device, PPQ, PluginFormat, Project, Transient};
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

/// Frames per `render_offline` call; the cancel flag is looked at between calls.
pub const CHUNK: usize = 4096;
/// How long a render waits for the engine's background warp renders (a warped clip is silent
/// until its render is attached).
const WARP_WAIT: Duration = Duration::from_secs(60);

/// What to render: `frames` stereo frames of `project` from timeline frame `start`.
pub struct OfflineRender {
    pub project: Project,
    /// Sample rate of the render and of the frame numbers.
    pub rate: f64,
    /// Detected transients by asset, for warped clips (as in the live session).
    pub transients: HashMap<AssetId, Vec<Transient>>,
    pub start: i64,
    pub frames: usize,
}

impl OfflineRender {
    /// Plugin devices (not built-in ones) the render plays: inserts and, on instrument tracks, the
    /// instrument plugin, each with whether it is the instrument plugin.
    pub fn plugin_devices(&self) -> Vec<(&Device, bool)> {
        let is_plugin = |d: &Device| dstudio_core::builtin::def_of(d).is_none() && d.plugin.format != PluginFormat::Builtin;
        let mut out = Vec::new();
        for t in &self.project.tracks {
            out.extend(t.devices.iter().filter(|d| is_plugin(d)).map(|d| (d, false)));
            if t.kind == dstudio_core::TrackKind::Instrument {
                out.extend(t.instrument_plugin.iter().filter(|d| is_plugin(d)).map(|d| (d, true)));
            }
        }
        out
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OfflineError {
    /// The cancel flag was set; nothing was produced.
    Cancelled,
    Failed(String),
}

impl From<EngineError> for OfflineError {
    fn from(e: EngineError) -> Self {
        OfflineError::Failed(e.to_string())
    }
}

/// A private engine for one render and the plugins still to prepare for it. Drop it on the main
/// thread when it holds plugins (see the module comment).
pub struct Session {
    engine: Engine,
    job: OfflineRender,
    todo: Vec<(Device, bool)>,
    next: usize,
}

impl Session {
    /// Makes the private engine and lists the plugins to prepare. Cheap.
    pub fn new(job: OfflineRender) -> Result<Session, OfflineError> {
        let engine = Engine::new(Config { sample_rate: job.rate, ..Config::default() })?;
        let todo = job.plugin_devices().into_iter().map(|(d, instrument)| (d.clone(), instrument)).collect();
        Ok(Session { engine, job, todo, next: 0 })
    }

    /// Plugins to prepare in all, and how many are done.
    pub fn plugins(&self) -> (usize, usize) {
        (self.next, self.todo.len())
    }

    /// MAIN THREAD. Prepares the next plugin and returns its name, or None when all are ready.
    /// `live` is the running app's engine: the state it reads from the live instance of the
    /// device (what playback sounds like right now) is what the plugin starts from, falling back
    /// to the saved state in the project for a device that is not live. A plugin that cannot be
    /// read or loaded fails the render, so nothing is rendered without it.
    pub fn prepare_next(&mut self, live: Option<&Engine>) -> Result<Option<String>, OfflineError> {
        let Some((device, instrument)) = self.todo.get(self.next) else { return Ok(None) };
        let name = device.plugin.name.clone();
        let from_project = || device.state.as_deref().and_then(dstudio_core::device::b64::decode);
        let state = match live.map(|e| e.plugin_state_of(device.id)).transpose() {
            Ok(Some(Some(bytes))) => Some(bytes),
            Ok(_) => from_project(),
            Err(_) => return Err(OfflineError::Failed(format!("Couldn’t read {name}’s current sound. Try again."))),
        };
        self.engine
            .prepare_plugin(device, *instrument, state)
            .map_err(|e| OfflineError::Failed(format!("{name} could not be loaded: {}", e.message)))?;
        self.next += 1;
        Ok(Some(name))
    }

    /// Renders the job to interleaved stereo f32 on the calling thread (any thread once every
    /// plugin is prepared). `progress` counts the frames done (up to `job.frames`); `cancel` is
    /// looked at before each file load and between chunks.
    pub fn render(&self, cancel: &AtomicBool, progress: &AtomicUsize) -> Result<Vec<f32>, OfflineError> {
        let check = || if cancel.load(Ordering::Relaxed) { Err(OfflineError::Cancelled) } else { Ok(()) };
        check()?;
        if self.next < self.todo.len() {
            return Err(OfflineError::Failed("the plugins are not ready".into()));
        }
        let (engine, job) = (&self.engine, &self.job);
        engine.set_plugin_detached(true);
        let p = &job.project;

        let used: HashSet<AssetId> = p
            .tracks
            .iter()
            .flat_map(|t| {
                t.clips
                    .iter()
                    .filter(|c| !c.disabled)
                    .map(|c| c.asset)
                    .chain(t.devices.iter().chain(t.instrument_plugin.iter()).flat_map(|d| d.samples.iter().map(|s| s.asset)))
            })
            .collect();
        let mut assets = HashMap::new();
        for id in used {
            check()?;
            let a = p.asset(id).ok_or_else(|| OfflineError::Failed(format!("asset {} is not in the project", id.0)))?;
            let info =
                engine.load_file(Path::new(&a.absolute_path)).map_err(|e| OfflineError::Failed(format!("{}: {e}", a.absolute_path)))?;
            assets.insert(id, info.asset_id);
        }
        let mut reversed = HashMap::new();
        for c in p.tracks.iter().flat_map(|t| &t.clips).filter(|c| c.warp.reverse && !c.disabled) {
            if let (Some(&ea), false) = (assets.get(&c.asset), reversed.contains_key(&c.asset)) {
                reversed.insert(c.asset, engine.reverse_asset(ea)?.asset_id);
            }
        }

        let (snap, _) = snapshot::build_full(p, job.rate, &assets, &reversed, &job.transients);
        engine.publish(&snap)?;
        if let Some((_, why)) = engine.plugin_load_errors().first() {
            return Err(OfflineError::Failed(format!("a plugin could not be loaded: {why}")));
        }
        let tr = &p.transport;
        let frames_per_beat = p.ticks_to_seconds(PPQ * 4 / tr.time_signature.denominator as i64) * job.rate;
        engine.set_metronome(MetronomeConfig {
            enabled: false,
            gain: 0.5,
            frames_per_beat,
            beats_per_bar: tr.time_signature.numerator as u32,
        })?;
        let waited = Instant::now();
        while engine.warp_renders_pending() > 0 {
            check()?;
            if waited.elapsed() > WARP_WAIT {
                return Err(OfflineError::Failed("warped audio took too long to prepare".into()));
            }
            engine.plugin_idle();
            std::thread::sleep(Duration::from_millis(2));
        }
        let out = render_aligned(engine, job.start, job.frames, |done| {
            check()?;
            progress.store(done, Ordering::Relaxed);
            Ok(())
        })?;
        progress.store(job.frames, Ordering::Relaxed);
        Ok(out)
    }
}

/// Plays `engine`'s published session once from timeline frame `start` (no loop) and returns
/// `frames` stereo frames of the mix, interleaved, aligned to the timeline. The mix of a chain
/// with plugin latency comes out L frames late (delay compensation aligns the tracks to each
/// other, not to the timeline), so this renders L more and drops the first L. `step` is called
/// before each `render_offline` call with the frames of the result done so far; an error from it
/// stops the render. The transport is left playing.
pub fn render_aligned(
    engine: &Engine,
    start: i64,
    frames: usize,
    mut step: impl FnMut(usize) -> Result<(), OfflineError>,
) -> Result<Vec<f32>, OfflineError> {
    let lead = engine.latency_report().total_frames as usize;
    engine.set_loop(false, 0, 0)?;
    engine.stop()?;
    engine.seek(start)?;
    engine.play()?;
    let mut out = vec![0f32; (frames + lead) * 2];
    for (i, chunk) in out.chunks_mut(CHUNK * 2).enumerate() {
        step((i * CHUNK).saturating_sub(lead).min(frames))?;
        engine.render_offline(chunk)?;
    }
    out.drain(..lead * 2);
    Ok(out)
}

/// Renders `job` start to finish on the calling thread: a plugin job has to be called from
/// the main thread, which it holds for the whole render (the app prepares step by step and
/// renders on a worker instead, see the module comment). Plugin devices start from their state
/// in the project.
pub fn render(job: OfflineRender, cancel: &AtomicBool, progress: &AtomicUsize) -> Result<Vec<f32>, OfflineError> {
    if cancel.load(Ordering::Relaxed) {
        return Err(OfflineError::Cancelled);
    }
    let mut session = Session::new(job)?;
    while session.prepare_next(None)?.is_some() {}
    session.render(cancel, progress)
}
