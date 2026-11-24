//! The assistant's voice input on the UI thread (spec 024): the mic button and talk key start
//! and stop listening, the take is turned into text on a worker thread, and the text goes into
//! the composer (or is sent, with Auto-send). Settings › Downloads gets the speech model.
//! The audio side is the engine's voice tap; recognition itself is in voice.rs.

use crate::controller::{Controller, JobResult};
use crate::voice::{self, Take};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

/// Where voice input is.
#[derive(Default)]
pub enum Phase {
    #[default]
    Idle,
    /// Capturing from the engine's voice tap.
    Listening { take: Take },
    /// Turning the take into text.
    Working,
}

/// The speech model download (Settings › Downloads).
pub struct ModelDownload {
    pub progress: Arc<AtomicU64>,
    pub cancel: Arc<AtomicBool>,
}

#[derive(Default)]
pub struct Voice {
    pub phase: Phase,
    /// Input level while listening, 0..1 (the loudest sample since the last frame, smoothed).
    pub level: f32,
    /// Text for the composer, handed to the UI on the next update.
    pub draft: Option<String>,
    /// The talk key (both keys of the chord) is held down.
    pub key_down: bool,
    /// No other key was pressed since the chord went down: it is the talk key, not the start
    /// of a shortcut such as Ctrl+Alt+M.
    pub armed: bool,
    /// Listening was started by holding the talk key (Hold talk key to record).
    pub hold_started: bool,
    pub download: Option<ModelDownload>,
    /// The last download failed: shown under the row until the next try.
    pub download_error: Option<String>,
    /// Whether the speech model is on disk (checked once, then after each download or removal).
    pub ready: Option<bool>,
    /// TEST: where the speech model is looked for instead of voice::model_path() (the
    /// self-test points it at a missing file, so it runs the same on every machine).
    pub test_model_path: Option<PathBuf>,
    /// SCREENSHOT-ONLY: keep the listening level as set (no engine input in `--state` renders).
    pub frozen: bool,
    /// TEST: the next take's text instead of running the model (self-test, screenshots).
    pub fixture_text: Option<String>,
}

impl Voice {
    pub fn listening(&self) -> bool {
        matches!(self.phase, Phase::Listening { .. })
    }
    /// 0 idle, 1 listening, 2 working (the mic button's look).
    pub fn ui_state(&self) -> i32 {
        match self.phase {
            Phase::Idle => 0,
            Phase::Listening { .. } => 1,
            Phase::Working => 2,
        }
    }
}

/// The talk key: Control and Option on macOS, Control and Alt elsewhere. `text` is the key
/// that changed (Slint key codes; on macOS Slint reports the Control key as Meta).
pub fn is_talk_chord(text: &str, ctrl: bool, alt: bool, meta: bool) -> bool {
    const CONTROL: &str = "\u{11}";
    const CONTROL_R: &str = "\u{16}";
    const ALT: &str = "\u{12}";
    const ALT_GR: &str = "\u{13}";
    const META: &str = "\u{17}";
    const META_R: &str = "\u{18}";
    let is_alt = text == ALT || text == ALT_GR;
    if cfg!(target_os = "macos") {
        ((text == META || text == META_R) && alt) || (is_alt && meta)
    } else {
        ((text == CONTROL || text == CONTROL_R) && alt) || (is_alt && ctrl)
    }
}

/// A key of the chord was released.
pub fn is_talk_key(text: &str) -> bool {
    let ctrl = if cfg!(target_os = "macos") { ["\u{17}", "\u{18}"] } else { ["\u{11}", "\u{16}"] };
    ctrl.contains(&text) || text == "\u{12}" || text == "\u{13}"
}

impl Controller {
    fn speech_model_path(&self) -> Option<PathBuf> {
        self.voice.test_model_path.clone().or_else(voice::model_path)
    }

    fn speech_model(&self) -> Option<PathBuf> {
        self.speech_model_path().filter(|p| voice::model_ready(p))
    }

    /// The speech model is downloaded (cached; see `Voice::ready`).
    pub fn speech_model_ready(&mut self) -> bool {
        let ready = self.voice.ready.unwrap_or_else(|| self.speech_model().is_some());
        self.voice.ready = Some(ready);
        ready
    }

    /// The mic button: start listening, or stop and recognise.
    pub fn voice_mic(&mut self) {
        match self.voice.phase {
            Phase::Idle => self.voice_start(),
            Phase::Listening { .. } => self.voice_stop(),
            Phase::Working => {}
        }
    }

    /// A key went down or up while the main window has the keyboard. Returns true when it was
    /// the talk key's chord going down (the key event is used).
    ///
    /// The chord (Control and Option, or Ctrl and Alt) also starts shortcuts such as Ctrl+Alt+M,
    /// so it counts as the talk key only if no other key is pressed while it is held: press
    /// and release it to start listening, again to stop. With Hold talk key to record, listening
    /// starts as soon as the chord is down (so the first word isn't lost), is dropped without
    /// recognition if another key follows, and stops when the chord is released.
    pub fn voice_key(&mut self, text: &str, pressed: bool, ctrl: bool, alt: bool, meta: bool) -> bool {
        let hold = self.settings.prefs.voice.hold_to_talk;
        if pressed {
            if is_talk_chord(text, ctrl, alt, meta) {
                if !self.voice.key_down {
                    self.voice.key_down = true;
                    self.voice.armed = true;
                    if hold && matches!(self.voice.phase, Phase::Idle) {
                        self.voice_start();
                        self.voice.hold_started = self.voice.listening();
                    }
                }
                return true;
            }
            if self.voice.key_down && self.voice.armed && !is_talk_key(text) {
                self.voice.armed = false;
                if std::mem::take(&mut self.voice.hold_started) {
                    self.voice_cancel();
                }
            }
            return false;
        }
        if self.voice.key_down && is_talk_key(text) {
            self.voice.key_down = false;
            let armed = std::mem::take(&mut self.voice.armed);
            if std::mem::take(&mut self.voice.hold_started) {
                if self.voice.listening() {
                    self.voice_stop();
                }
            } else if armed {
                self.voice_mic();
            }
        }
        false
    }

    /// Stops listening and drops the take (no recognition).
    pub fn voice_cancel(&mut self) {
        if self.voice.listening() {
            self.engine.set_voice_capture(None);
            self.voice.phase = Phase::Idle;
            self.voice.level = 0.0;
            self.ui_dirty = true;
        }
    }

    pub fn voice_start(&mut self) {
        if !matches!(self.voice.phase, Phase::Idle) {
            return;
        }
        if self.voice.fixture_text.is_none() && self.speech_model().is_none() {
            self.set_status("Download speech recognition in Settings › Downloads to talk to the assistant", 6);
            self.open_settings();
            self.settings_page(5);
            return;
        }
        if self.rec.recording {
            self.set_status("Stop recording before talking to the assistant", 4);
            return;
        }
        let src = self.settings.prefs.voice.source();
        if !self.engine.set_voice_capture(Some((src.channel, src.stereo))) {
            self.set_status("Device couldn't start listening: not enough memory", 6);
            return;
        }
        let rate = self.engine.sample_rate().round().max(1.0) as u32;
        self.voice.phase = Phase::Listening { take: Take::new(rate) };
        self.voice.level = 0.0;
        self.assistant_open_for_voice();
        self.ui_dirty = true;
    }

    /// Stops listening and starts recognition (or says nothing was heard).
    pub fn voice_stop(&mut self) {
        let Phase::Listening { mut take, .. } = std::mem::take(&mut self.voice.phase) else { return };
        let mut rest = Vec::new();
        let peak = self.engine.read_voice_capture(&mut rest);
        take.push(&rest, peak);
        self.engine.set_voice_capture(None);
        self.voice.level = 0.0;
        self.ui_dirty = true;
        if let Some(text) = self.voice.fixture_text.take() {
            self.finish_voice(Ok(text));
            return;
        }
        if take.samples.is_empty() {
            self.set_status("Device couldn't hear an audio input. Choose one in Settings › Audio.", 6);
            return;
        }
        if take.silent() {
            self.set_status("Didn't catch that", 3);
            return;
        }
        let Some(model) = self.speech_model() else {
            self.set_status("Download speech recognition in Settings › Downloads to talk to the assistant", 6);
            return;
        };
        let language = self.settings.prefs.voice.language.clone();
        let tx = self.jobs_tx.clone();
        self.pending_jobs += 1;
        self.voice.phase = Phase::Working;
        let spawned = std::thread::Builder::new().name("voice-input".into()).spawn(move || {
            voice::quiet_logs();
            let audio = voice::to_16k(&take.samples, take.rate);
            let _ = tx.send(JobResult::Voice(voice::transcribe(&model, &audio, &language)));
        });
        if spawned.is_err() {
            self.pending_jobs -= 1;
            self.voice.phase = Phase::Idle;
            self.set_status("Couldn't start speech recognition", 4);
        }
    }

    /// Every frame while listening: drain the engine's voice tap, update the level, stop at
    /// the time limit.
    pub fn voice_tick(&mut self) {
        if self.voice.frozen {
            return;
        }
        if let Phase::Listening { take, .. } = &mut self.voice.phase {
            let mut got = Vec::new();
            let peak = self.engine.read_voice_capture(&mut got);
            take.push(&got, peak);
            let full = take.full();
            let level = (1.0 + 20.0 * peak.max(1e-5).log10() / 50.0).clamp(0.0, 1.0); // −50..0 dBFS
            self.voice.level = level.max(self.voice.level * 0.85);
            self.ui_dirty = true;
            if full {
                self.voice_stop();
            }
        }
        if self.voice.download.is_some() {
            self.ui_dirty = true;
        }
    }

    pub fn finish_voice(&mut self, result: Result<String, String>) {
        self.voice.phase = Phase::Idle;
        self.ui_dirty = true;
        match result {
            Ok(text) if text.is_empty() => self.set_status("Didn't catch that", 3),
            Ok(text) => {
                if self.settings.prefs.voice.auto_send && self.assistant.busy.is_none() {
                    self.assistant_send(&text);
                } else {
                    self.voice.draft = Some(text);
                }
            }
            Err(e) => self.set_status(e, 6),
        }
    }

    /// Talking to the assistant shows its panel.
    fn assistant_open_for_voice(&mut self) {
        if !self.assistant_open {
            self.assistant_open = true;
        }
    }

    // ------------------------------------------------------------ Settings › Downloads

    /// Download / Cancel / Remove on the speech model's row.
    pub fn speech_model_action(&mut self) {
        if let Some(d) = &self.voice.download {
            d.cancel.store(true, Ordering::Relaxed);
            return;
        }
        let Some(path) = self.speech_model_path() else {
            self.voice.download_error = Some("Device can't find a folder for downloads on this computer.".into());
            self.ui_dirty = true;
            return;
        };
        if voice::model_ready(&path) {
            if self.voice.listening() || matches!(self.voice.phase, Phase::Working) {
                return;
            }
            self.voice.ready = None;
            match std::fs::remove_file(&path) {
                Ok(()) => self.set_status("Speech recognition removed", 3),
                Err(e) => self.set_status(format!("Couldn't remove speech recognition: {e}"), 5),
            }
            self.ui_dirty = true;
            return;
        }
        let progress = Arc::new(AtomicU64::new(0));
        let cancel = Arc::new(AtomicBool::new(false));
        self.voice.download = Some(ModelDownload { progress: progress.clone(), cancel: cancel.clone() });
        self.voice.download_error = None;
        let tx = self.jobs_tx.clone();
        let url = std::env::var("DSTUDIO_SPEECH_MODEL_URL").unwrap_or_else(|_| voice::MODEL.url.to_string());
        self.pending_jobs += 1;
        let spawned = std::thread::Builder::new().name("speech-model".into()).spawn(move || {
            let m = &voice::MODEL;
            let r = voice::download(&url, m.sha256, m.bytes, &path, &progress, &cancel);
            let _ = tx.send(JobResult::SpeechModel(r));
        });
        if spawned.is_err() {
            self.pending_jobs -= 1;
            self.voice.download = None;
            self.voice.download_error = Some("Couldn't start the download.".into());
        }
        self.ui_dirty = true;
    }

    pub fn finish_speech_model(&mut self, result: Result<(), String>) {
        let cancelled = self.voice.download.take().is_some_and(|d| d.cancel.load(Ordering::Relaxed));
        self.voice.ready = None;
        match result {
            Ok(()) => self.set_status("Speech recognition is ready: click the mic in the assistant to talk", 5),
            Err(_) if cancelled => {}
            Err(e) => self.voice.download_error = Some(e),
        }
        self.ui_dirty = true;
    }

    /// Settings › Downloads row: (state, button, status line, progress 0..1).
    /// State: 0 not downloaded, 1 downloading, 2 ready.
    pub fn speech_model_row(&self) -> (i32, &'static str, String, f32) {
        let size = voice::size_label(voice::MODEL.bytes);
        if let Some(d) = &self.voice.download {
            let got = d.progress.load(Ordering::Relaxed);
            let f = (got as f64 / voice::MODEL.bytes as f64).clamp(0.0, 1.0) as f32;
            return (1, "Cancel", format!("Downloading… {} of {size}", voice::size_label(got)), f);
        }
        if self.voice.ready.unwrap_or_else(|| self.speech_model().is_some()) {
            return (2, "Remove", format!("Downloaded · {size}"), 1.0);
        }
        let line = match &self.voice.download_error {
            Some(e) => e.clone(),
            None => format!("Not downloaded · {size}"),
        };
        (0, "Download", line, 0.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_talk_chord_is_control_and_option() {
        let (ctrl, alt, meta) = if cfg!(target_os = "macos") { ("\u{17}", "\u{12}", true) } else { ("\u{11}", "\u{12}", false) };
        // Control pressed with Option held, or Option pressed with Control held.
        assert!(is_talk_chord(ctrl, !meta, true, meta));
        assert!(is_talk_chord(alt, !meta, false, meta));
        // Either key alone is not the chord; a letter isn't either.
        assert!(!is_talk_chord(ctrl, !meta, false, meta));
        assert!(!is_talk_chord(alt, false, false, false));
        assert!(!is_talk_chord("a", true, true, true));
        assert!(is_talk_key(ctrl) && is_talk_key(alt) && !is_talk_key("a"));
    }
}
