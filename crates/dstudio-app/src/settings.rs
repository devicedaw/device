//! Settings window (in-app modal panel with a window's geometry) and its
//! Audio page. Audio and Plugins (src/devices.rs) are real; the other sidebar pages
//! are PLACEHOLDERS.
//!
//! Why a panel and not a second OS window: a second Slint window would not
//! be part of `take_snapshot()`
//! (screenshots), would need its own wgpu/notifier wiring, and the
//! self-test drives one window's event queue. The panel is drawn at a
//! fixed window rect (x 255, y 7, 960 pt wide) with its own traffic-light
//! close button, blocks the main window while open, and closes with Escape,
//! ⌘, or the close button.
//!
//! Every device change calls `Engine::configure_device` and, on success, is
//! persisted as an app preference (`src/prefs.rs`).

use crate::controller::{Controller, JobResult, sync_model};
use crate::prefs::{self, Preferences};
use crate::{ChoiceItem, Studio};
use dstudio_engine::AssetInfo;
use std::path::PathBuf;
use std::time::{Duration, Instant};

pub const PAGES: [&str; 9] = ["General", "Audio", "MIDI", "Plugins", "Shortcuts", "Downloads", "Account", "Usage", "Privacy"];
pub const AUDIO_PAGE: usize = 1;
pub const PLUGINS_PAGE: usize = 3;
pub const BUFFER_SIZES: [u32; 6] = [64, 128, 256, 512, 1024, 2048];

/// Dropdowns on the Audio page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    Output = 0,
    Input = 1,
    SampleRate = 2,
    BufferSize = 3,
    /// Settings › Shortcuts › Preset.
    ShortcutPreset = 4,
    /// Settings › MIDI › Default instrument (src/default_instrument.rs).
    DefaultInstrument = 5,
}

impl Field {
    fn from_i32(i: i32) -> Option<Field> {
        [Field::Output, Field::Input, Field::SampleRate, Field::BufferSize, Field::ShortcutPreset, Field::DefaultInstrument]
            .into_iter()
            .find(|f| *f as i32 == i)
    }
}

/// A dropdown choice: label + the value it sets.
#[derive(Debug, Clone, PartialEq)]
pub enum Choice {
    Device(Option<(String, String)>), // (id, name)
    Rate(f64),
    Buffer(u32),
    Preset(&'static str),
    /// A default instrument (None = the Synth).
    Instrument(Option<crate::prefs::InstrumentChoice>),
    /// A heading row; can't be picked.
    Header,
}

#[derive(Debug)]
pub struct TestTone {
    pub until: Instant,
    pub restore_pos: i64,
}

#[derive(Debug, Default)]
pub struct SettingsState {
    pub open: bool,
    pub page: usize,
    pub prefs: Preferences,
    /// None = never persist (self-test, screenshot states).
    pub prefs_path: Option<PathBuf>,
    /// Self-test and screenshot runs: never open native plugin windows on their own.
    pub headless: bool,
    /// SCREENSHOT-ONLY: use `recording::screenshot_fake_devices()` and never touch the device.
    pub fake_devices: bool,
    pub menu: Option<(Field, Vec<(String, Choice)>)>,
    pub note: String,
    pub tone_asset: Option<AssetInfo>,
    pub tone_loading: bool,
    pub tone: Option<TestTone>,
    /// Settings › Shortcuts and the live keymap (src/shortcuts.rs).
    pub keys: crate::shortcuts::ShortcutsUi,
    /// Search settings (src/settings_search.rs): the text typed; cleared on close.
    pub search: String,
    /// Settings closed since the last view-model push: the search fields it held may have had
    /// the keyboard, and their focus flag must not outlive them (it blocks every shortcut).
    pub release_focus: std::cell::Cell<bool>,
}

pub fn fmt_rate(r: f64) -> String {
    let k = r / 1000.0;
    if (k - k.round()).abs() < 1e-6 { format!("{} kHz", k.round() as i64) } else { format!("{k:.1} kHz") }
}

pub fn fmt_buffer(n: u32, rate: f64) -> String {
    format!("{n} samples ({:.1} ms)", n as f64 * 1000.0 / rate.max(1.0))
}

/// Settings › Audio note when the running device does not use the buffer size
/// picked (Windows shared mode offers only some sizes and rounds to the
/// nearest it supports). `actual` is the device period in device frames.
pub fn buffer_note(requested: u32, running: bool, actual: u32, device_rate: f64) -> Option<String> {
    (requested > 0 && running && actual > 0 && actual != requested)
        .then(|| format!("The device runs at {}, the closest buffer it supports.", fmt_buffer(actual, device_rate)))
}

/// Settings dropdown popup rows: 22 pt, 4 pt padding.
pub const MENU_ROW: f32 = 22.0;
pub const MENU_PAD: f32 = 4.0;
/// Rows a settings dropdown shows before it scrolls (a long plugin list).
pub const MENU_MAX_ROWS: f32 = 14.0;

/// -24 dBFS, 0.5 s, 440 Hz with 10 ms fades (Settings › Audio "Test").
pub fn test_tone_samples(rate: u32) -> Vec<f32> {
    let n = rate as usize / 2;
    let amp = 10f32.powf(-24.0 / 20.0);
    let fade = (rate as f32 * 0.01).max(1.0);
    (0..n)
        .map(|i| {
            let env = (i as f32 / fade).min(1.0).min((n - 1 - i) as f32 / fade);
            amp * env * (std::f32::consts::TAU * 440.0 * i as f32 / rate as f32).sin()
        })
        .collect()
}

const TONE_TRACK_ID: u64 = u64::MAX - 7;

/// Startup: reapply saved audio preferences BEFORE the device starts (never in
/// self-test or screenshot runs). An input device is configured only if the saved
/// preferences name one; nothing here ever picks an input on its own.
pub fn apply_startup_prefs(engine: &dstudio_engine::Engine, p: &Preferences) -> Result<(), String> {
    if p.audio.input_muted {
        engine.set_input_muted(true);
    }
    if p.audio.pdc_off {
        engine.set_delay_compensation(false);
    }
    if p.audio.is_custom() {
        engine.configure_device(&p.audio.device_config()).map_err(|e| format!("saved audio settings not applied: {e}"))?;
    }
    Ok(())
}

impl Controller {
    // ---------------------------------------------------------------- startup

    /// Remember the interface zoom level (saved with the preferences; not project state).
    pub fn set_interface_zoom(&mut self, percent: u32) {
        if self.settings.prefs.interface_zoom != percent {
            self.settings.prefs.interface_zoom = percent;
            self.save_prefs();
        }
    }

    pub(crate) fn save_prefs(&mut self) {
        if let Some(path) = self.settings.prefs_path.clone()
            && let Err(e) = prefs::save(&path, &self.settings.prefs)
        {
            self.set_status(format!("Preferences not saved: {e}"), 6);
        }
    }

    /// The engine's sample rate may change after `configure_device`.
    fn refresh_engine_rate(&mut self) {
        let r = self.engine.sample_rate();
        if r > 0.0 && (r - self.rate).abs() > 1e-6 {
            self.rate = r;
            self.publish();
        }
    }

    // ---------------------------------------------------------------- window

    pub fn toggle_settings(&mut self) {
        if self.settings.open { self.close_settings() } else { self.open_settings() }
    }

    pub fn open_settings(&mut self) {
        self.settings.open = true;
        self.settings.page = AUDIO_PAGE;
        self.settings.menu = None;
        self.rec.input_menu = None;
        self.menu_close();
        self.synth_menu_dismiss();
        self.refresh_settings_note();
        self.ui_dirty = true;
    }

    pub fn close_settings(&mut self) {
        self.settings.open = false;
        self.settings.menu = None;
        self.settings.keys.recording = None;
        self.settings.keys.search.clear();
        self.settings.search.clear();
        self.settings.release_focus.set(true);
        self.ui_dirty = true;
    }

    /// Settings › General switches, in page order: 0 Always load last session, 1 Auto-show
    /// third-party effect windows, 2 Auto-show built-in effect windows, 3 Keep plugin windows
    /// on top, 4 Sections move clips. Saved at once.
    pub fn general_toggle(&mut self, i: i32) {
        let g = &mut self.settings.prefs.general;
        let flag = match i {
            0 => &mut g.load_last_session,
            1 => &mut g.show_plugin_windows,
            2 => &mut g.show_builtin_windows,
            3 => &mut g.plugin_windows_on_top,
            4 => &mut g.sections_move_clips,
            _ => return,
        };
        *flag = !*flag;
        if i == 3 {
            crate::devices::set_plugin_windows_on_top(self.settings.prefs.general.plugin_windows_on_top);
        }
        self.save_prefs();
        self.ui_dirty = true;
    }

    /// Settings › General › Projects folder › Choose…: save it, list it in the browser's
    /// Projects tab and start Save there.
    pub fn set_projects_folder(&mut self, dir: std::path::PathBuf) {
        self.settings.prefs.general.projects_folder = Some(dir.clone());
        self.save_prefs();
        self.files.projects_dir = Some(dir);
        self.files_refresh();
        self.ui_dirty = true;
    }

    /// The projects folder for display, with ~ for the home folder.
    pub fn projects_folder_text(&self) -> String {
        let Some(d) = self.files.projects_dir.as_deref() else { return "Not set".into() };
        let home = std::env::var(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).unwrap_or_default();
        let s = d.display().to_string();
        match s.strip_prefix(&home) {
            Some(rest) if !home.is_empty() && (rest.is_empty() || rest.starts_with(std::path::MAIN_SEPARATOR)) => format!("~{rest}"),
            _ => s,
        }
    }

    /// Settings › General › Playhead on stop: Return to start (false) or Stop in place (true).
    pub fn general_stop_in_place(&mut self, on: bool) {
        self.settings.prefs.general.stop_in_place = on;
        self.save_prefs();
        self.ui_dirty = true;
    }

    pub fn settings_page(&mut self, i: usize) {
        self.settings.page = i.min(PAGES.len() - 1);
        self.settings.menu = None;
        self.settings.keys.recording = None;
        self.settings.keys.search.clear();
        if self.settings.page == crate::midi_in::MIDI_PAGE {
            self.midi_scan(true);
        }
        self.ui_dirty = true;
    }

    fn refresh_settings_note(&mut self) {
        let mut note = match self.device_list() {
            Ok(d) if d.is_empty() => "No audio devices found.".into(),
            Ok(_) if self.settings.fake_devices => String::new(),
            Ok(_) => self.engine.input_error().map(|e| format!("Input not open: {e}")).unwrap_or_default(),
            Err(e) => format!("Device list unavailable: {e}"),
        };
        // The engine's processing rate is fixed when it is created (assets are
        // resampled to it on import); a different device rate is converted by
        // the OS for output and is rejected with an input. Say so instead of
        // implying the session rate changed. Changing the session rate needs an
        // engine rebuild + asset reload (not implemented yet).
        let want = self.settings.prefs.audio.sample_rate;
        let engine_rate = self.engine.sample_rate();
        if !self.settings.fake_devices && want > 0.0 && (want - engine_rate).abs() > 0.5 {
            let msg = format!(
                "The session runs at {} kHz. The device at {} kHz is converted for output; recording needs {} kHz.",
                engine_rate / 1000.0,
                want / 1000.0,
                engine_rate / 1000.0
            );
            note = if note.is_empty() { msg } else { format!("{note} {msg}") };
        }
        if !self.settings.fake_devices {
            let st = self.engine.device_status();
            if let Some(msg) = buffer_note(self.settings.prefs.audio.buffer_size, st.running, st.buffer_size, st.sample_rate) {
                note = if note.is_empty() { msg } else { format!("{note} {msg}") };
            }
        }
        self.settings.note = note;
    }

    // ---------------------------------------------------------------- values shown

    fn current_rate(&self) -> f64 {
        if self.settings.prefs.audio.sample_rate > 0.0 {
            return self.settings.prefs.audio.sample_rate;
        }
        let st = if self.settings.fake_devices { 0.0 } else { self.engine.device_status().sample_rate };
        if st > 0.0 { st } else { self.rate }
    }

    fn current_buffer(&self) -> u32 {
        if self.settings.prefs.audio.buffer_size > 0 {
            return self.settings.prefs.audio.buffer_size;
        }
        if self.settings.fake_devices { 0 } else { self.engine.device_status().buffer_size }
    }

    fn output_text(&self) -> String {
        let a = &self.settings.prefs.audio;
        if let Some(n) = &a.output_name {
            return n.clone();
        }
        if !self.settings.fake_devices {
            let n = self.engine.device_status().output_name;
            if !n.is_empty() {
                return n;
            }
        }
        "System default".into()
    }

    fn input_text(&self) -> String {
        self.settings.prefs.audio.input_name.clone().unwrap_or_else(|| "None".into())
    }

    // ---------------------------------------------------------------- dropdowns

    fn choices(&self, f: Field) -> Vec<(String, Choice)> {
        let devs = self.device_list().unwrap_or_default();
        match f {
            Field::Output => {
                let mut v = vec![("System default".to_string(), Choice::Device(None))];
                v.extend(
                    devs.iter()
                        .filter(|d| d.output_channels > 0)
                        .map(|d| (d.name.clone(), Choice::Device(Some((d.id.clone(), d.name.clone()))))),
                );
                v
            }
            Field::Input => {
                let mut v = vec![("None".to_string(), Choice::Device(None))];
                v.extend(
                    devs.iter()
                        .filter(|d| d.input_channels > 0)
                        .map(|d| (d.name.clone(), Choice::Device(Some((d.id.clone(), d.name.clone()))))),
                );
                v
            }
            Field::SampleRate => {
                let out = self.settings.prefs.audio.output.as_ref();
                let dev = devs.iter().find(|d| Some(&d.id) == out).or_else(|| devs.iter().find(|d| d.is_default_output));
                let mut rates: Vec<f64> = dev.map(|d| d.sample_rates.clone()).unwrap_or_default();
                if rates.is_empty() {
                    rates.push(self.current_rate());
                }
                rates.sort_by(|a, b| a.partial_cmp(b).unwrap());
                rates.dedup();
                rates.into_iter().map(|r| (fmt_rate(r), Choice::Rate(r))).collect()
            }
            Field::BufferSize => {
                let rate = self.current_rate();
                BUFFER_SIZES.iter().map(|&n| (fmt_buffer(n, rate), Choice::Buffer(n))).collect()
            }
            Field::ShortcutPreset => {
                crate::shortcut_presets::PRESETS.iter().map(|p| (p.name.to_string(), Choice::Preset(p.name))).collect()
            }
            Field::DefaultInstrument => self.instrument_choices(),
        }
    }

    fn is_current(&self, f: Field, c: &Choice) -> bool {
        let a = &self.settings.prefs.audio;
        match (f, c) {
            (Field::Output, Choice::Device(d)) => d.as_ref().map(|d| &d.0) == a.output.as_ref(),
            (Field::Input, Choice::Device(d)) => d.as_ref().map(|d| &d.0) == a.input.as_ref(),
            (Field::SampleRate, Choice::Rate(r)) => (r - self.current_rate()).abs() < 0.5,
            (Field::BufferSize, Choice::Buffer(n)) => *n == self.current_buffer(),
            (Field::ShortcutPreset, Choice::Preset(p)) => *p == crate::shortcut_presets::find(&self.settings.prefs.shortcut_preset).name,
            (Field::DefaultInstrument, Choice::Instrument(c)) => {
                crate::default_instrument::same_choice(c.as_ref(), self.settings.prefs.midi.default_instrument.as_ref())
            }
            _ => false,
        }
    }

    pub fn settings_dropdown(&mut self, field: i32) {
        let Some(f) = Field::from_i32(field) else { return };
        if self.settings.menu.as_ref().is_some_and(|m| m.0 == f) {
            self.settings.menu = None;
        } else {
            self.refresh_settings_note();
            self.settings.menu = Some((f, self.choices(f)));
        }
        self.ui_dirty = true;
    }

    pub fn settings_menu_dismiss(&mut self) {
        self.settings.menu = None;
        self.ui_dirty = true;
    }

    pub fn settings_menu_pick(&mut self, i: usize) {
        let Some((f, items)) = self.settings.menu.take() else { return };
        let Some((_, c)) = items.get(i).cloned() else { return };
        self.ui_dirty = true;
        if matches!(c, Choice::Header) {
            self.settings.menu = Some((f, items));
            return;
        }
        if self.is_current(f, &c) {
            return;
        }
        if let Choice::Instrument(choice) = c {
            self.settings.prefs.midi.default_instrument = choice;
            self.save_prefs();
            return;
        }
        if let Choice::Preset(name) = c {
            self.shortcuts_set_preset(name);
            return;
        }
        let mut next = self.settings.prefs.clone();
        let a = &mut next.audio;
        match (f, c) {
            (Field::Output, Choice::Device(d)) => (a.output, a.output_name) = d.map(|(i, n)| (Some(i), Some(n))).unwrap_or((None, None)),
            (Field::Input, Choice::Device(d)) => (a.input, a.input_name) = d.map(|(i, n)| (Some(i), Some(n))).unwrap_or((None, None)),
            (Field::SampleRate, Choice::Rate(r)) => a.sample_rate = r,
            (Field::BufferSize, Choice::Buffer(n)) => a.buffer_size = n,
            _ => return,
        }
        self.apply_audio_prefs(next);
    }

    /// `configure_device` with the new setup; persisted only when the engine accepts it.
    fn apply_audio_prefs(&mut self, next: Preferences) {
        if self.settings.fake_devices {
            self.set_status("Screenshot state: device changes are disabled", 3);
            return;
        }
        if self.rec.recording {
            self.set_status("Stop recording before changing the audio device", 4);
            return;
        }
        match self.engine.configure_device(&next.audio.device_config()) {
            Ok(()) => {
                self.settings.prefs = next;
                self.save_prefs();
                self.refresh_engine_rate();
                self.set_status("Audio settings applied", 2);
                self.refresh_settings_note();
            }
            Err(e) => {
                self.set_status(format!("Audio settings not applied: {e}"), 6);
                self.refresh_settings_note();
            }
        }
    }

    pub fn settings_toggle_mute(&mut self) {
        let m = !self.settings.prefs.audio.input_muted;
        if !self.settings.fake_devices {
            self.engine.set_input_muted(m);
        }
        self.settings.prefs.audio.input_muted = m;
        self.save_prefs();
        self.ui_dirty = true;
    }

    pub fn settings_toggle_pdc(&mut self) {
        let off = !self.settings.prefs.audio.pdc_off;
        self.engine.set_delay_compensation(!off);
        self.settings.prefs.audio.pdc_off = off;
        self.save_prefs();
        self.ui_dirty = true;
    }

    /// "Plugin latency: 12.3 ms (590 samples)", or "none" when no plugin adds latency.
    fn latency_text(&self) -> String {
        let r = self.engine.latency_report();
        let max_track = r.tracks.iter().map(|(_, l)| *l).max().unwrap_or(0);
        if max_track == 0 {
            return "Plugin latency: none".into();
        }
        let ms = max_track as f64 * 1000.0 / self.rate;
        let mut t = format!("Plugin latency: {ms:.1} ms ({max_track} samples)");
        if !self.engine.delay_compensation_enabled() {
            t.push_str(", not compensated");
        } else if r.clamped {
            t.push_str(&format!(", compensation capped at {} samples", dstudio_engine::PDC_MAX_DELAY_FRAMES));
        }
        t
    }

    // ---------------------------------------------------------------- test tone

    /// "Test": a 0.5 s, -24 dBFS sine played through the engine with a temporary
    /// snapshot (only the tone, loop and metronome off) from the current
    /// position; afterwards the transport is paused, put back where it was, and
    /// the real session is republished (`poll_test_tone`).
    pub fn settings_test(&mut self) {
        if self.settings.fake_devices {
            return;
        }
        if !self.engine.device_running() {
            self.set_status("Test: no audio device (offline mode)", 4);
            return;
        }
        if self.engine.transport().playing || self.rec.recording {
            self.set_status("Test: stop playback first", 3);
            return;
        }
        if self.settings.tone.is_some() || self.settings.tone_loading {
            return;
        }
        if let Some(info) = self.settings.tone_asset {
            self.start_test_tone(info);
            return;
        }
        let rate = self.rate.round() as u32;
        let path = std::env::temp_dir().join(format!("dstudio-test-tone-{}-{rate}.wav", std::process::id()));
        if let Err(e) = crate::wavgen::write_wav(&path, rate, 1, &test_tone_samples(rate)) {
            self.set_status(format!("Test: {e}"), 4);
            return;
        }
        let engine = self.engine.clone();
        let tx = self.jobs_tx.clone();
        self.pending_jobs += 1;
        self.settings.tone_loading = true;
        std::thread::spawn(move || {
            let r = engine.load_file(&path).map_err(|e| e.to_string());
            let _ = tx.send(JobResult::TestTone(r));
        });
    }

    pub(crate) fn finish_test_tone(&mut self, r: Result<AssetInfo, String>) {
        self.settings.tone_loading = false;
        match r {
            Ok(info) => {
                self.settings.tone_asset = Some(info);
                if self.settings.open && !self.engine.transport().playing {
                    self.start_test_tone(info);
                }
            }
            Err(e) => self.set_status(format!("Test: {e}"), 4),
        }
    }

    fn start_test_tone(&mut self, info: AssetInfo) {
        use dstudio_core::snapshot::{ClipSnapshot, SessionSnapshot, TrackSnapshot};
        let pos = self.engine.transport().position;
        let snap = SessionSnapshot {
            tracks: vec![TrackSnapshot {
                track_id: TONE_TRACK_ID,
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
                    clip_id: TONE_TRACK_ID,
                    engine_asset: info.asset_id,
                    timeline_start: pos,
                    source_offset: 0,
                    length: info.frames as i64,
                    gain: 1.0,
                    warp: None,
                    clip_loop: None,
                    fades: Default::default(),
                }],
                instrument: false,
                notes: vec![],
                arm: false,
                monitor: false,
                input: None,
                synth: None,
                devices: vec![],
                instrument_plugin: None,
                automation: vec![],
            }],
            master_gain: 1.0,
            master_automation: vec![],
            device_assets: Default::default(),
            tempo: vec![],
            click_quarters: 1.0,
        };
        let e = &self.engine;
        let off = dstudio_engine::MetronomeConfig { enabled: false, gain: 0.0, frames_per_beat: 24_000.0, beats_per_bar: 4 };
        let r = e.set_loop(false, 0, 0).and_then(|_| e.set_metronome(off)).and_then(|_| e.publish(&snap)).and_then(|_| e.play());
        // Restore even if starting failed (stop + republish happen in poll_test_tone).
        let dur = Duration::from_secs_f64(info.frames as f64 / self.rate.max(1.0)) + Duration::from_millis(80);
        self.settings.tone = Some(TestTone { until: Instant::now() + if r.is_ok() { dur } else { Duration::ZERO }, restore_pos: pos });
        if let Err(err) = r {
            self.set_status(format!("Test: {err}"), 4);
        }
    }

    /// Called every UI tick: ends the test tone and restores the session.
    pub fn poll_test_tone(&mut self) {
        let Some(t) = &self.settings.tone else { return };
        if Instant::now() < t.until {
            return;
        }
        let pos = t.restore_pos;
        self.settings.tone = None;
        let _ = self.engine.pause();
        let _ = self.engine.seek(pos);
        self.publish(); // real session + loop/metronome
    }

    pub fn test_tone_active(&self) -> bool {
        self.settings.tone.is_some()
    }

    // ---------------------------------------------------------------- view model

    pub(crate) fn push_settings(&self, s: &Studio) {
        s.set_settings_open(self.settings.open);
        if !self.settings.open {
            if self.settings.release_focus.take() {
                s.set_search_focused(false);
            }
            return;
        }
        s.set_settings_page(self.settings.page as i32);
        s.set_settings_search(self.settings.search.clone().into());
        let slots = crate::settings_search::page_slots(&self.settings.search);
        s.set_settings_search_none(slots.iter().all(|&i| i < 0));
        s.set_settings_page_slots(slint::ModelRc::new(slint::VecModel::from(slots)));
        s.set_set_output_text(self.output_text().into());
        s.set_set_input_text(self.input_text().into());
        s.set_set_rate_text(fmt_rate(self.current_rate()).into());
        let b = self.current_buffer();
        s.set_set_buffer_text(if b > 0 { fmt_buffer(b, self.current_rate()) } else { "Device default".into() }.into());
        s.set_set_input_muted(self.settings.prefs.audio.input_muted);
        s.set_set_pdc_on(!self.settings.prefs.audio.pdc_off);
        if self.settings.page == crate::midi_in::MIDI_PAGE {
            self.push_midi_settings();
            s.set_midi_instrument_text(
                crate::default_instrument::choice_label(self.settings.prefs.midi.default_instrument.as_ref()).into(),
            );
            s.set_midi_instrument_missing(self.default_instrument_missing());
        }
        if self.settings.page == crate::shortcuts::SHORTCUTS_PAGE {
            self.push_shortcuts(s);
        }
        let g = &self.settings.prefs.general;
        let on = [g.load_last_session, g.show_plugin_windows, g.show_builtin_windows, g.plugin_windows_on_top, g.sections_move_clips];
        let shown = s.get_general_on();
        if (0..on.len()).any(|i| slint::Model::row_data(&shown, i) != Some(on[i])) {
            s.set_general_on(slint::ModelRc::new(slint::VecModel::from(on.to_vec())));
        }
        s.set_general_stop_in_place(g.stop_in_place);
        s.set_general_projects_folder(self.projects_folder_text().into());
        s.set_set_latency_text(self.latency_text().into());
        s.set_settings_note(self.settings.note.clone().into());
        match &self.settings.menu {
            Some((f, items)) => {
                let rows: Vec<ChoiceItem> = items
                    .iter()
                    .enumerate()
                    .map(|(i, (label, c))| ChoiceItem {
                        label: label.clone().into(),
                        checked: self.is_current(*f, c),
                        hint: matches!(c, Choice::Header),
                        y: MENU_PAD + i as f32 * MENU_ROW,
                        h: MENU_ROW,
                    })
                    .collect();
                let full = MENU_PAD * 2.0 + rows.len() as f32 * MENU_ROW;
                let shown = full.min(MENU_PAD * 2.0 + MENU_MAX_ROWS * MENU_ROW);
                s.set_settings_menu_h(shown);
                s.set_settings_menu_content_h(if full > shown { full } else { 0.0 });
                s.set_settings_menu_field(*f as i32);
                sync_model(&self.settings_menu_model, rows);
                s.set_settings_menu_open(true);
            }
            None => s.set_settings_menu_open(false),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn formats() {
        assert_eq!(fmt_rate(48_000.0), "48 kHz");
        assert_eq!(fmt_rate(44_100.0), "44.1 kHz");
        assert_eq!(fmt_buffer(1024, 48_000.0), "1024 samples (21.3 ms)");
    }
    #[test]
    fn buffer_note_only_when_the_device_differs() {
        assert_eq!(
            buffer_note(64, true, 128, 48_000.0).as_deref(),
            Some("The device runs at 128 samples (2.7 ms), the closest buffer it supports.")
        );
        assert_eq!(buffer_note(128, true, 128, 48_000.0), None); // got what was picked
        assert_eq!(buffer_note(0, true, 480, 48_000.0), None); // device default picked
        assert_eq!(buffer_note(64, false, 0, 48_000.0), None); // not running
    }
    #[test]
    fn tone_level() {
        let s = test_tone_samples(48_000);
        assert_eq!(s.len(), 24_000);
        let peak = s.iter().fold(0f32, |m, x| m.max(x.abs()));
        assert!((20.0 * peak.log10() + 24.0).abs() < 0.05, "{peak}");
        assert!(s[0].abs() < 1e-6 && s[s.len() - 1].abs() < 1e-3);
    }
}
