//! Arm / monitor / input routing, the record transport and the live recording
//! clip (docs/adr/0002-recording.md).
//!
//! Every project change is a `dstudio_core::Command`: R = `SetTrackArm`,
//! I = `SetTrackMonitor` (audio and instrument tracks), the inspector "In"
//! popup = `SetTrackInput` (audio tracks), and a finished recording pass = ONE
//! `Batch` "Record": `AddAsset` + `AddClip` per audio take (with one armed
//! track that is exactly one take), then one `AddMidiClip` per armed
//! instrument track that received notes (`midi_in.rs`, spec 003). Device I/O
//! goes through the engine's recording API; every engine error is shown as a
//! status message.
//!
//! Where takes are written (`Controller::recording_dir`): `<project dir>/media/`
//! when the project has been saved, else `<system temp dir>/Device Studio/Unsaved
//! recordings/`. Note that Save does not move unsaved recordings into the project
//! (core `persist` has no media consolidation yet), and macOS may purge the temp
//! directory after a few days.

use crate::controller::{Controller, JobResult, load_and_analyse, sync_model};
use crate::peaks::Pyramid;
use crate::timeline::ClipVis;
use crate::{ChoiceItem, Studio};
use dstudio_core::{Asset, AssetId, Clip, ClipId, Command, InputSource, PPQ, TrackId, TrackKind};
use dstudio_engine::{AssetInfo, AudioDeviceInfo, RecordedTake};
use std::path::PathBuf;

/// Where recorded input may come from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum InputPolicy {
    /// Normal app: record from the configured input device.
    #[default]
    Device,
    /// `--self-test`: input is injected with `Engine::render_offline_with_input`;
    /// no real input device is ever opened or required.
    InjectedOnly,
}

#[derive(Debug, Default)]
pub struct RecState {
    pub recording: bool,
    pub policy: InputPolicy,
    /// Overrides `recording_dir()` (self-test scratch directory).
    pub dir_override: Option<PathBuf>,
    /// The "In" popup, while open: its entries (label, source).
    pub input_menu: Option<Vec<(String, Option<InputSource>)>>,
    /// Hint row under "None" when no device inputs can be listed.
    pub input_hint: String,
    /// Live takes drawn on the timeline (rebuilt every frame while recording).
    pub live_vis: Vec<ClipVis>,
    /// Label of the last finished pass (self-test reads it).
    pub last_pass: Option<String>,
    /// The engine is recording audio takes in this pass (MIDI-only passes don't).
    pub audio: bool,
    /// Counting in: the frame the recording starts at, while the playhead is still before it.
    pub count_in: Option<i64>,
    /// The pass's punch-in frame after a count-in: what was recorded before it is dropped.
    pub punch: Option<i64>,
    /// The loop (frames) that was on when the pass started, while it stays unchanged: each
    /// time round it makes one take (src/takes.rs).
    pub rec_loop: Option<(i64, i64)>,
    /// The engine's write-error count when the pass started (it counts across passes).
    pub write_errors_at_start: u64,
    /// Takes of the last stopped pass that the engine cut short (file size limit or a
    /// write error): they keep what was written, and the status line says so.
    pub cut_short: u64,
}

/// Status suffix for takes the engine had to cut short; empty when there are none.
pub fn cut_short_note(n: u64) -> String {
    match n {
        0 => String::new(),
        1 => "; 1 take was cut short (file size limit or a disk error), kept up to that point".into(),
        n => format!("; {n} takes were cut short (file size limit or a disk error), kept up to that point"),
    }
}

/// Rows of the "In" selector: "None", then "Input k (Mono)" per device input
/// channel, then "Input k/k+1 (Stereo)" pairs when the device has ≥ 2 inputs.
pub fn input_choices(dev: Option<&AudioDeviceInfo>) -> Vec<(String, Option<InputSource>)> {
    let mut v = vec![("None".to_string(), None)];
    if let Some(d) = dev {
        let n = d.input_channels.min(u16::MAX as u32) as u16;
        for c in 0..n {
            v.push((input_label(Some(InputSource { channel: c, stereo: false })), Some(InputSource { channel: c, stereo: false })));
        }
        for c in (0..n.saturating_sub(1)).step_by(2) {
            v.push((input_label(Some(InputSource { channel: c, stereo: true })), Some(InputSource { channel: c, stereo: true })));
        }
    }
    v
}

/// Inspector "In" field text.
pub fn input_label(src: Option<InputSource>) -> String {
    match src {
        None => "Select audio input".into(),
        Some(InputSource { channel, stereo: false }) => format!("Input {} (Mono)", channel as u32 + 1),
        Some(InputSource { channel, stereo: true }) => format!("Input {}/{} (Stereo)", channel as u32 + 1, channel as u32 + 2),
    }
}

/// SCREENSHOT-ONLY fake device listing, used exclusively by `--state
/// reference-input-menu` and `--state reference-settings-audio` so those
/// captures are deterministic. Never configured on the engine.
pub fn screenshot_fake_devices() -> Vec<AudioDeviceInfo> {
    vec![
        AudioDeviceInfo {
            id: "fake:speakers".into(),
            name: "Built-in Speakers".into(),
            input_channels: 0,
            output_channels: 2,
            sample_rates: vec![44_100.0, 48_000.0],
            is_default_input: false,
            is_default_output: true,
        },
        AudioDeviceInfo {
            id: "fake:microphone".into(),
            name: "Built-in Microphone".into(),
            input_channels: 1,
            output_channels: 0,
            sample_rates: vec![44_100.0, 48_000.0],
            is_default_input: true,
            is_default_output: false,
        },
    ]
}

/// Popup geometry (window points): rows 21 pt, 3 pt padding, hint rows 30 pt.
pub const IN_ROW: f32 = 21.0;
pub const IN_HINT_ROW: f32 = 30.0;
pub const IN_PAD: f32 = 3.0;

impl Controller {
    // ---------------------------------------------------------------- devices

    /// Device listing: the SCREENSHOT fake list in `--state` captures, else the engine's.
    pub fn device_list(&self) -> Result<Vec<AudioDeviceInfo>, String> {
        if self.settings.fake_devices {
            return Ok(screenshot_fake_devices());
        }
        self.engine.list_devices().map_err(|e| e.message.clone())
    }

    /// The input device the "In" selector lists channels for: the one chosen in
    /// Settings › Audio (preference), else the one the engine reports open.
    /// No fallback to the system default: an input is only ever opened after the
    /// user picks it deliberately (macOS microphone prompt).
    pub fn current_input_device(&self) -> Result<Option<AudioDeviceInfo>, String> {
        let devs = self.device_list()?;
        if let Some(id) = &self.settings.prefs.audio.input
            && let Some(d) = devs.iter().find(|d| &d.id == id)
        {
            return Ok(Some(d.clone()));
        }
        if !self.settings.fake_devices
            && let Some(name) = self.engine.device_status().input_name
        {
            return Ok(devs.into_iter().find(|d| d.name == name && d.input_channels > 0));
        }
        Ok(None)
    }

    // ---------------------------------------------------------------- arm / monitor / input

    fn audio_track(&self, id: TrackId) -> Option<&dstudio_core::Track> {
        self.project().track(id).filter(|t| t.kind == TrackKind::Audio)
    }

    /// R button: `SetTrackArm` (one undo step). An armed instrument track also plays
    /// the MIDI keyboard and records it.
    /// While a MIDI pass records, instrument tracks keep their arm state: the pass
    /// records on the tracks armed when it started, which are the ones hearing the keyboard.
    pub fn toggle_arm(&mut self, id: TrackId) {
        let Some((kind, arm)) = self.project().track(id).map(|t| (t.kind, t.arm)) else { return };
        if self.midi_in.pass.is_some() && kind == TrackKind::Instrument {
            self.set_status("Stop recording to arm or disarm instrument tracks", 4);
            return;
        }
        self.execute(Command::SetTrackArm { track: id, arm: !arm });
    }

    /// I button: `SetTrackMonitor`. On an instrument track: plays the MIDI keyboard
    /// without arming it.
    pub fn toggle_monitor(&mut self, id: TrackId) {
        if let Some(monitor) = self.project().track(id).map(|t| !t.monitor) {
            self.execute(Command::SetTrackMonitor { track: id, monitor });
        }
    }

    pub fn set_track_input(&mut self, id: TrackId, input: Option<InputSource>) {
        if self.audio_track(id).is_some_and(|t| t.input != input) {
            self.execute(Command::SetTrackInput { track: id, input });
        }
    }

    /// Open the inspector "In" popup for the selected audio track.
    pub fn open_input_menu(&mut self) {
        let Some(t) = self.selected_track.and_then(|t| self.audio_track(t)).map(|t| t.id) else { return };
        let _ = t;
        let (dev, hint) = match self.current_input_device() {
            Ok(Some(d)) => (Some(d), String::new()),
            Ok(None) => (None, "No input device. Choose one in Settings › Audio.".to_string()),
            Err(e) => (None, format!("Inputs unavailable: {e}")),
        };
        self.rec.input_menu = Some(input_choices(dev.as_ref()));
        self.rec.input_hint = hint;
        self.ui_dirty = true;
    }

    pub fn input_menu_close(&mut self) {
        self.rec.input_menu = None;
        self.ui_dirty = true;
    }

    pub fn input_menu_pick(&mut self, i: usize) {
        let Some(items) = self.rec.input_menu.take() else { return };
        if let (Some((_, src)), Some(t)) = (items.get(i), self.selected_track) {
            self.set_track_input(t, *src);
        }
        self.ui_dirty = true;
    }

    // ---------------------------------------------------------------- record transport

    /// Armed audio tracks that have an input (the ones a recording captures).
    pub fn armed_with_input(&self) -> Vec<TrackId> {
        self.project().tracks.iter().filter(|t| t.kind == TrackKind::Audio && t.arm && t.input.is_some()).map(|t| t.id).collect()
    }

    /// Takes go to `<project>/media/` once saved, else to a temp "Unsaved recordings" dir.
    pub fn recording_dir(&self) -> PathBuf {
        if let Some(d) = &self.rec.dir_override {
            return d.clone();
        }
        match &self.project_dir {
            Some(p) => p.join("media"),
            None => std::env::temp_dir().join("Device Studio").join("Unsaved recordings"),
        }
    }

    /// Header record button / `R` (transport.record).
    pub fn record_toggle(&mut self) {
        if self.rec.recording { self.stop_recording() } else { self.start_recording() }
    }

    /// Starts a pass: audio takes on armed audio tracks with an input, MIDI on armed
    /// instrument tracks. When audio can't be recorded but MIDI can, records MIDI only.
    pub fn start_recording(&mut self) {
        let audio = !self.armed_with_input().is_empty();
        let midi = !self.armed_midi().is_empty();
        if !audio && !midi {
            self.set_status("Nothing to record: arm a track (R); audio tracks also need an input (In)", 5);
            return;
        }
        match self.rec.policy {
            InputPolicy::Device if !self.engine.device_running() => {
                self.set_status("Can't record: no audio device (offline mode)", 5);
                return;
            }
            InputPolicy::InjectedOnly if self.engine.device_running() => {
                self.set_status("Can't record: injected-input recording needs the offline engine (--no-audio)", 5);
                return;
            }
            _ => {}
        }
        let mut audio_err = None;
        if audio && self.rec.policy == InputPolicy::Device && self.engine.device_status().input_name.is_none() {
            let why = self.engine.input_error().map(|e| format!(" ({e})")).unwrap_or_default();
            audio_err = Some(format!("no audio input device{why}. Choose one in Settings › Audio"));
        }
        // Count-in: back up the playhead and click until the recording point.
        self.rec.punch = self.count_in_begin();
        let mut audio_started = false;
        if audio && audio_err.is_none() {
            let dir = self.recording_dir();
            if let Err(e) = std::fs::create_dir_all(&dir) {
                audio_err = Some(format!("{}: {e}", dir.display()));
            } else {
                match self.engine.start_recording(&dir) {
                    Ok(()) => {
                        audio_started = true;
                        self.rec.write_errors_at_start = self.engine.recording_stats().write_errors;
                        self.autosave_mark_recording(&dir);
                    }
                    Err(e) => audio_err = Some(format!("recording failed: {e}")),
                }
            }
        }
        if let Some(e) = &audio_err
            && !midi
        {
            self.count_in_cancel();
            self.set_status(format!("Can't record: {e}"), 6);
            self.ui_dirty = true;
            return;
        }
        if midi {
            // The engine starts the transport for audio takes; a MIDI-only pass starts it here.
            if !audio_started && let Err(e) = self.engine.play() {
                self.count_in_cancel();
                self.set_status(format!("Can't record: {e}"), 6);
                return;
            }
            self.midi_pass_start();
        }
        self.rec.recording = true;
        self.rec.audio = audio_started;
        self.rec.rec_loop = self.loop_frames();
        match audio_err {
            Some(e) => self.set_status(format!("Recording MIDI only: {e}"), 6),
            None if self.rec.count_in.is_some() => self.set_status("Counting in…", 2),
            None => self.set_status("Recording…", 2),
        }
        self.ui_dirty = true;
    }

    pub fn stop_recording(&mut self) {
        self.rec.recording = false;
        let punch = self.rec.punch;
        self.takes_tick();
        let rec_loop = self.rec.rec_loop.take();
        self.count_in_cancel();
        self.rec.live_vis.clear();
        self.ui_dirty = true;
        let midi = self.midi_pass_finish();
        self.rec.punch = None;
        if !std::mem::take(&mut self.rec.audio) {
            self.finish_midi_only(midi, &[]);
            return;
        }
        let takes = match self.engine.stop_recording() {
            Ok(t) => {
                self.rec.cut_short = self.engine.recording_stats().write_errors.saturating_sub(self.rec.write_errors_at_start);
                t
            }
            Err(e) => {
                self.set_status(format!("Stopping the recording failed: {e}"), 6);
                if !midi.is_empty() {
                    self.finish_midi_only(midi, &[format!("audio takes lost ({e})")]);
                }
                return;
            }
        };
        if takes.is_empty() {
            if midi.is_empty() {
                self.set_status("Recording stopped: no takes", 3);
            } else {
                self.finish_midi_only(midi, &[]);
            }
            return;
        }
        // Decode + analyse on a worker thread, as imports do; the "Record" batch is
        // executed when the engine assets exist (JobResult::Recorded).
        let engine = self.engine.clone();
        let tx = self.jobs_tx.clone();
        let epoch = self.doc_epoch;
        self.pending_jobs += 1;
        std::thread::Builder::new()
            .name("recorded-takes".into())
            .spawn(move || {
                let takes = takes
                    .into_iter()
                    .map(|t| {
                        let r = load_and_analyse(&engine, &t.path);
                        (t, r)
                    })
                    .collect();
                let _ = tx.send(JobResult::Recorded { doc_epoch: epoch, takes, midi, punch, rec_loop });
            })
            .expect("spawn take loader");
        self.set_status("Loading recording…", 30);
    }

    /// A pass without usable audio takes: its MIDI clips are the "Record" step.
    /// `skipped` names audio takes that were lost, so the status still reports them.
    fn finish_midi_only(&mut self, midi: Vec<Command>, skipped: &[String]) {
        let midi = dstudio_core::midi_record::still_valid(self.project(), midi);
        let skipped = if skipped.is_empty() { String::new() } else { format!("; skipped: {}", skipped.join("; ")) };
        if midi.is_empty() {
            let secs = if skipped.is_empty() { 3 } else { 6 };
            self.set_status(format!("Recording stopped: nothing was played{skipped}"), secs);
            return;
        }
        let n = midi.len();
        if self.execute(Command::Batch { label: "Record".into(), commands: midi }) {
            self.rec.last_pass = Some("Record".into());
            let secs = if skipped.is_empty() { 3 } else { 6 };
            self.set_status(format!("Recorded {n} MIDI clip{}{skipped}", if n == 1 { "" } else { "s" }), secs);
        }
    }

    /// Worker finished loading the takes of one pass: ONE undoable `Batch` "Record"
    /// (audio takes, then the pass's MIDI clips).
    pub(crate) fn finish_recorded(
        &mut self,
        doc_epoch: u64,
        takes: Vec<(RecordedTake, Result<(AssetInfo, Pyramid), String>)>,
        midi: Vec<Command>,
        punch: Option<i64>,
        rec_loop: Option<(i64, i64)>,
    ) {
        if doc_epoch != self.doc_epoch {
            for (_, r) in takes {
                if let Ok((info, _)) = r {
                    let _ = self.engine.release_asset(info.asset_id);
                }
            }
            return;
        }
        let mut next = self.project().next_id;
        let mut alloc = || {
            let id = next;
            next += 1;
            id
        };
        let mut cmds = Vec::new();
        let mut loaded = Vec::new();
        let mut errors = Vec::new();
        let mut passes = 0usize; // the most loop passes one take made
        for (take, res) in takes {
            let (info, pyr) = match res {
                Ok(v) => v,
                Err(e) => {
                    errors.push(e);
                    continue;
                }
            };
            let track = TrackId(take.track_id);
            let Some(name) = self.audio_track(track).map(|t| t.name.clone()) else {
                let _ = self.engine.release_asset(info.asset_id);
                errors.push(format!("track {} is gone", take.track_id));
                continue;
            };
            let rate = if take.sample_rate > 0.0 { take.sample_rate } else { info.source_sample_rate };
            let frames =
                if take.frames > 0 { take.frames } else { (info.frames as f64 * info.source_sample_rate / self.rate).round() as u64 };
            if frames == 0 {
                let _ = self.engine.release_asset(info.asset_id);
                continue;
            }
            // Latency compensation can put the first sample before 0, and a count-in records
            // before the punch-in point: trim those off.
            let first = crate::count_in::first_kept_frame(take.start_frame, punch);
            let skip = ((first - take.start_frame) as f64 * rate / self.rate).round().max(0.0) as u64;
            if skip >= frames {
                let _ = self.engine.release_asset(info.asset_id);
                continue;
            }
            let asset_id = AssetId(alloc());
            let asset = Asset {
                id: asset_id,
                relative_path: None,
                absolute_path: std::fs::canonicalize(&take.path).unwrap_or_else(|_| take.path.clone()).to_string_lossy().into_owned(),
                sample_rate: rate,
                channels: take.channels.max(1),
                frames,
            };
            let clip_at = |id: u64, at: i64, offset: u64, length: u64, takes: Vec<i64>| Clip {
                id: ClipId(id),
                asset: asset_id,
                name: name.clone(),
                start: self.project().frames_to_ticks(at, self.rate),
                source_offset: offset,
                source_length: length,
                gain: 1.0,
                warp: Default::default(),
                disabled: false,
                clip_loop: None,
                fades: Default::default(),
                takes,
            };
            // Timeline frames <-> the take's own frames.
            let to_asset = |tl: i64| (tl as f64 * rate / self.rate).round().max(0.0) as u64;
            let kept = ((frames - skip) as f64 * self.rate / rate).round() as i64;
            let mut clips = Vec::new();
            // Round the loop: one clip over the loop playing the last complete pass, with every
            // pass as a take (and the part before the loop, if any, as a clip of its own).
            if let Some(cut) = rec_loop.and_then(|l| crate::takes::loop_cut(first, kept, l)) {
                if cut.lead > 0 {
                    clips.push(clip_at(alloc(), first, skip, to_asset(cut.lead), Vec::new()));
                }
                let len = to_asset(cut.len);
                let starts: Vec<u64> = cut.passes.iter().map(|&o| skip + to_asset(o)).filter(|&o| o + len <= frames).collect();
                if let Some(&last) = starts.last() {
                    let takes = if starts.len() > 1 { starts.iter().map(|&o| o as i64 - last as i64).collect() } else { Vec::new() };
                    passes = passes.max(starts.len());
                    clips.push(clip_at(alloc(), cut.start, last, len, takes));
                } else {
                    clips.clear(); // no whole pass after rounding: keep the take linear
                }
            }
            if clips.is_empty() {
                clips.push(clip_at(alloc(), first, skip, frames - skip, Vec::new()));
            }
            cmds.push(Command::AddAsset { asset });
            cmds.extend(clips.into_iter().map(|clip| Command::AddClip { track, clip }));
            loaded.push((asset_id, info.asset_id, pyr));
        }
        if cmds.is_empty() {
            if midi.is_empty() {
                let cut = cut_short_note(std::mem::take(&mut self.rec.cut_short));
                self.set_status(
                    format!(
                        "Recording produced no usable takes{}{}",
                        if errors.is_empty() { String::new() } else { format!(": {}", errors.join("; ")) },
                        cut
                    ),
                    6,
                );
            } else {
                self.finish_midi_only(midi, &errors);
            }
            return;
        }
        // MIDI clips last: they take their ids when the batch runs, after the takes' ids.
        // A MIDI clip whose track was deleted while the takes loaded is dropped, so it
        // can't fail the batch and lose the audio takes with it.
        let midi = dstudio_core::midi_record::still_valid(self.project(), midi);
        let midi_clips = midi.len();
        cmds.extend(midi);
        for (asset_id, ea, pyr) in &loaded {
            self.peaks.insert(*ea, pyr.clone());
            self.asset_map.insert(*asset_id, *ea);
        }
        if self.execute(Command::Batch { label: "Record".into(), commands: cmds }) {
            self.rec.last_pass = Some("Record".into());
            let mut extra = if midi_clips > 0 { format!(" and {midi_clips} MIDI clip(s)") } else { String::new() };
            if passes > 1 {
                extra += &format!(", {passes} loop takes (clip menu › Takes)");
            }
            let cut = std::mem::take(&mut self.rec.cut_short);
            extra += &cut_short_note(cut);
            if errors.is_empty() && cut == 0 {
                self.set_status(format!("Recorded {} take(s){extra}", loaded.len()), 3);
            } else if errors.is_empty() {
                self.set_status(format!("Recorded {} take(s){extra}", loaded.len()), 8);
            } else {
                self.set_status(format!("Recorded {} take(s){extra}; skipped: {}", loaded.len(), errors.join("; ")), 6);
            }
        } else {
            for (asset_id, ea, _) in loaded {
                self.asset_map.remove(&asset_id);
                let _ = self.engine.release_asset(ea);
            }
        }
    }

    // ---------------------------------------------------------------- live clip

    /// Growing clips of the takes in progress (from `recording_state()` and
    /// `recording_peaks()`), plus follow-playhead while recording.
    pub fn rebuild_live(&mut self) {
        self.rec.live_vis.clear();
        if !self.rec.recording {
            return;
        }
        let st = self.engine.recording_state();
        let p = self.doc.project();
        // Frames per beat at the playhead (for the peak resolution; positions use the tempo map).
        let fpb = self.rate * 60.0 / self.bpm_now();
        // About one peak pair per logical point at the current zoom; the engine works in
        // multiples of 64 frames, so round to that and scale the pair width to match.
        let frames_per_pt = fpb / self.view.px_per_beat;
        let fpp = ((frames_per_pt / 64.0).round().max(1.0) * 64.0) as u32;
        for (k, t) in st.takes.iter().enumerate() {
            let Some(ti) = p.track_index(TrackId(t.track_id)) else { continue };
            let start_beats = p.frames_to_ticks(t.start_frame, self.rate) as f64 / PPQ as f64;
            let len_beats = if p.has_tempo_changes() {
                (p.frames_to_ticks(t.start_frame + t.frames as i64, self.rate) as f64 / PPQ as f64) - start_beats
            } else {
                t.frames as f64 / fpb
            };
            let max_pairs = ((t.frames as f64 / fpp as f64).ceil() as usize + 1).min(8192);
            // recording_peaks() serves the newest take of a track (a seek starts a new take).
            let newest = !st.takes[k + 1..].iter().any(|n| n.track_id == t.track_id);
            let peaks = if newest { self.engine.recording_peaks(t.track_id, fpp, max_pairs) } else { Vec::new() };
            self.rec.live_vis.push(ClipVis {
                track_index: ti,
                start_beats,
                len_beats: len_beats.max(0.0),
                color: crate::controller::RECORD_RED,
                selected: false,
                muted: false,
                name: "Recording".into(),
                wave: None,
                midi: None,
                live: Some((peaks, (fpp as f64 / frames_per_pt) as f32)),
                fades: (0.0, 0.0),
                fade_handles: false,
            });
        }
        self.rebuild_live_midi();
        // Follow the playhead: page forward when it nears the right edge.
        let ph = self.playhead_ticks() as f64 / PPQ as f64;
        let x = self.view.x_of(ph);
        let w = self.lanes_size.0;
        if x > w * 0.92 || x < 0.0 {
            self.view.start_beats = (ph - (w as f64 * 0.1) / self.view.px_per_beat).max(0.0);
        }
    }

    /// The growing "Recording" clip of each armed instrument track, with the notes so far.
    fn rebuild_live_midi(&mut self) {
        let Some(pass) = &self.midi_in.pass else { return };
        let now = self.audible_frames();
        let bounds = self.midi_pass_bounds(pass, now);
        let notes = dstudio_core::midi_record::pair_notes(&pass.events, &bounds);
        let p = self.doc.project();
        let bar = p.ticks_per_bar().max(1);
        let ticks = |f: i64| p.frames_to_ticks(f.max(0), self.rate);
        let (mut from, mut to) = (ticks(bounds.start), ticks(now).max(ticks(bounds.start)));
        if let Some((ls, le)) = bounds.wrapped_loop {
            from = from.min(ticks(ls));
            to = to.max(ticks(le));
        }
        from = from.div_euclid(bar) * bar;
        let len = (to - from).max(1);
        let rel: Vec<(dstudio_core::Note, bool)> = notes
            .iter()
            .map(|n| {
                (
                    dstudio_core::Note {
                        id: 0,
                        pitch: n.pitch,
                        start: ticks(n.start) - from,
                        length: (ticks(n.end) - ticks(n.start)).max(1),
                        velocity: n.velocity,
                    },
                    false,
                )
            })
            .collect();
        let mini = crate::controller::mini_notes(&rel, 0, len);
        for &t in &pass.tracks {
            let Some(ti) = p.track_index(t) else { continue };
            self.rec.live_vis.push(ClipVis {
                track_index: ti,
                start_beats: from as f64 / PPQ as f64,
                len_beats: len as f64 / PPQ as f64,
                color: crate::controller::RECORD_RED,
                selected: false,
                muted: false,
                name: "Recording".into(),
                wave: None,
                midi: Some(mini.clone()),
                live: None,
                fades: (0.0, 0.0),
                fade_handles: false,
            });
        }
    }

    pub(crate) fn push_recording(&self, s: &Studio) {
        s.set_recording(self.rec.recording);
        s.set_record_ready(!self.armed_with_input().is_empty() || !self.armed_midi().is_empty());
        let sel = self.selected_track.and_then(|t| self.project().track(t));
        s.set_sel_arm(sel.is_some_and(|t| t.arm));
        s.set_sel_monitor(sel.is_some_and(|t| t.monitor));
        s.set_sel_input_text(input_label(sel.and_then(|t| t.input)).into());
        match &self.rec.input_menu {
            Some(items) => {
                let cur = sel.and_then(|t| t.input);
                let mut y = IN_PAD;
                let mut rows = Vec::new();
                for (i, (label, src)) in items.iter().enumerate() {
                    rows.push(ChoiceItem { label: label.clone().into(), checked: *src == cur, hint: false, y, h: IN_ROW });
                    y += IN_ROW;
                    if i == 0 && !self.rec.input_hint.is_empty() {
                        rows.push(ChoiceItem { label: self.rec.input_hint.clone().into(), checked: false, hint: true, y, h: IN_HINT_ROW });
                        y += IN_HINT_ROW;
                    }
                }
                s.set_input_menu_h(y + IN_PAD);
                s.set_input_menu_w(if self.rec.input_hint.is_empty() { 136.0 } else { 200.0 });
                sync_model(&self.input_menu_model, rows);
                s.set_input_menu_open(true);
            }
            None => s.set_input_menu_open(false),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn dev(ch: u32) -> AudioDeviceInfo {
        AudioDeviceInfo {
            id: "x".into(),
            name: "X".into(),
            input_channels: ch,
            output_channels: 0,
            sample_rates: vec![],
            is_default_input: false,
            is_default_output: false,
        }
    }
    #[test]
    fn cut_short_takes_are_named_in_the_status() {
        assert_eq!(cut_short_note(0), "");
        assert!(cut_short_note(1).contains("1 take was cut short"));
        assert!(cut_short_note(3).contains("3 takes were cut short"));
    }
    #[test]
    fn choices() {
        let labels = |d: Option<&AudioDeviceInfo>| input_choices(d).into_iter().map(|(l, _)| l).collect::<Vec<_>>();
        assert_eq!(labels(None), ["None"]);
        assert_eq!(labels(Some(&dev(1))), ["None", "Input 1 (Mono)"]);
        assert_eq!(labels(Some(&dev(2))), ["None", "Input 1 (Mono)", "Input 2 (Mono)", "Input 1/2 (Stereo)"]);
        assert_eq!(labels(Some(&dev(5))).len(), 1 + 5 + 2);
        let c = input_choices(Some(&dev(4)));
        assert_eq!(c.last().unwrap().1, Some(InputSource { channel: 2, stereo: true }));
        assert_eq!(input_label(None), "Select audio input");
    }
}
