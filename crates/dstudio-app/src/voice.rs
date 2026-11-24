//! Voice input for the assistant (spec 024, roadmap G19): speech recognition on this
//! computer with a downloadable open model (whisper.cpp through whisper-rs, MIT).
//!
//! The pieces, from the microphone to the composer:
//! - the engine's voice tap copies the chosen input into a ring (`Engine::set_voice_capture`),
//!   and the controller drains it every frame while listening ([`Take`]);
//! - [`to_16k`] converts the take to the model's 16 kHz;
//! - [`transcribe`] runs the model on a worker thread and returns the text.
//!
//! The model is downloaded once from Settings › Downloads ([`download`]): a pinned URL,
//! checked against a pinned SHA-256, kept next to the preferences. Nothing is sent anywhere
//! to recognise speech.

use dstudio_core::model::InputSource;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

/// The model's sample rate.
pub const MODEL_RATE: u32 = 16_000;
/// Listening stops by itself after this long.
pub const MAX_SECONDS: f32 = 60.0;
/// A take whose loudest sample stays below this (about −50 dBFS) is treated as silence.
const SILENT_PEAK: f32 = 0.003;

/// One downloadable model: where it comes from and how to check it.
pub struct ModelSpec {
    /// Shown in Settings › Downloads.
    pub name: &'static str,
    pub detail: &'static str,
    /// File name under the models folder.
    pub file: &'static str,
    /// Pinned to one revision of the model repository, so the hash below never changes.
    pub url: &'static str,
    pub sha256: &'static str,
    pub bytes: u64,
}

/// whisper.cpp "base" (multilingual, 8-bit): about 82 MB, good accuracy for short requests and
/// fast enough on a laptop CPU.
pub const MODEL: ModelSpec = ModelSpec {
    name: "Speech recognition",
    detail: "For talking to the assistant. Works on this computer, in about 100 languages.",
    file: "speech-base-q8_0.bin",
    url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/5359861c739e955e79d9a303bcbc70fb988958b1/ggml-base-q8_0.bin",
    sha256: "c577b9a86e7e048a0b7eada054f4dd79a56bbfa911fbdacf900ac5b567cbb7d9",
    bytes: 81_768_585,
};

/// Settings › General › Voice: the language list (code, name). "auto" lets the model detect it.
pub const LANGUAGES: [(&str, &str); 13] = [
    ("auto", "Automatic"),
    ("en", "English"),
    ("es", "Spanish"),
    ("fr", "French"),
    ("de", "German"),
    ("it", "Italian"),
    ("pt", "Portuguese"),
    ("nl", "Dutch"),
    ("ja", "Japanese"),
    ("ko", "Korean"),
    ("zh", "Chinese"),
    ("hi", "Hindi"),
    ("mi", "Māori"),
];

/// Settings › General › Voice.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct VoicePrefs {
    /// Hold talk key to record: listen while the talk key is held, instead of press to start
    /// and press again to stop.
    pub hold_to_talk: bool,
    /// Auto-send: send the recognised text as a turn straight away.
    pub auto_send: bool,
    /// A code from [`LANGUAGES`].
    pub language: String,
    /// Input: the device input to listen to (None = Input 1).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input: Option<InputSource>,
}

impl Default for VoicePrefs {
    fn default() -> Self {
        VoicePrefs { hold_to_talk: false, auto_send: false, language: "auto".into(), input: None }
    }
}

impl VoicePrefs {
    pub fn is_default(&self) -> bool {
        *self == VoicePrefs::default()
    }
    pub fn source(&self) -> InputSource {
        self.input.unwrap_or(InputSource { channel: 0, stereo: false })
    }
    pub fn language_index(&self) -> usize {
        LANGUAGES.iter().position(|(c, _)| *c == self.language).unwrap_or(0)
    }
}

/// Where the model lives: `DSTUDIO_SPEECH_MODEL` (tests, development), else `models/` next to
/// the preferences.
pub fn model_path() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("DSTUDIO_SPEECH_MODEL") {
        return Some(PathBuf::from(p));
    }
    Some(crate::prefs::support_dir()?.join("models").join(MODEL.file))
}

/// The model is there and complete (its size; the hash was checked when it was downloaded).
/// An override path only has to exist.
pub fn model_ready(path: &Path) -> bool {
    match std::fs::metadata(path) {
        Ok(m) => m.is_file() && (std::env::var_os("DSTUDIO_SPEECH_MODEL").is_some() || m.len() == MODEL.bytes),
        Err(_) => false,
    }
}

/// "82 MB".
pub fn size_label(bytes: u64) -> String {
    format!("{} MB", (bytes as f64 / 1_000_000.0).round() as u64)
}

/// Downloads `url` to `dest`, checking its size and SHA-256. Writes to `dest.part` and renames
/// it only when the hash matches, so a failed, cancelled or corrupt download leaves nothing at
/// `dest` (and removes the part file). `progress` receives the bytes received so far.
pub fn download(
    url: &str,
    sha256: &str,
    bytes: u64,
    dest: &Path,
    progress: &AtomicU64,
    cancel: &AtomicBool,
) -> Result<(), String> {
    if !url.starts_with("https://") && !url.starts_with("http://127.0.0.1:") {
        return Err("Downloads must use HTTPS.".into());
    }
    let dir = dest.parent().ok_or("The download folder is missing.")?;
    std::fs::create_dir_all(dir).map_err(|e| format!("Couldn't create {}: {e}", dir.display()))?;
    let part = dest.with_extension("part");
    let result = fetch_to(url, sha256, bytes, &part, progress, cancel);
    match result {
        Ok(()) => std::fs::rename(&part, dest).map_err(|e| {
            let _ = std::fs::remove_file(&part);
            format!("Couldn't save the download: {e}")
        }),
        Err(e) => {
            let _ = std::fs::remove_file(&part);
            Err(e)
        }
    }
}

fn fetch_to(url: &str, sha256: &str, bytes: u64, part: &Path, progress: &AtomicU64, cancel: &AtomicBool) -> Result<(), String> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .http_status_as_error(false)
        .timeout_connect(Some(Duration::from_secs(15)))
        .timeout_recv_body(Some(Duration::from_secs(60)))
        .tls_config(ureq::tls::TlsConfig::builder().root_certs(ureq::tls::RootCerts::PlatformVerifier).build())
        .user_agent(format!("Device/{}", env!("CARGO_PKG_VERSION")))
        .build()
        .into();
    let resp = agent.get(url).call().map_err(|_| "Couldn't connect. Check your internet connection and try again.".to_string())?;
    if !resp.status().is_success() {
        return Err(format!("The download failed (HTTP {}). Try again later.", resp.status().as_u16()));
    }
    let mut body = resp.into_body();
    let mut reader = body.with_config().limit(bytes + 1).reader();
    let mut file = std::fs::File::create(part).map_err(|e| format!("Couldn't save the download: {e}"))?;
    let mut hash = Sha256::new();
    let mut got = 0u64;
    let mut buf = vec![0u8; 1 << 16];
    progress.store(0, Ordering::Relaxed);
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err("Download cancelled.".into());
        }
        let n = reader.read(&mut buf).map_err(|_| "The download was interrupted. Try again.".to_string())?;
        if n == 0 {
            break;
        }
        got += n as u64;
        if got > bytes {
            return Err("The download is not the expected file.".into());
        }
        hash.update(&buf[..n]);
        file.write_all(&buf[..n]).map_err(|e| format!("Couldn't save the download: {e}"))?;
        progress.store(got, Ordering::Relaxed);
    }
    file.sync_all().map_err(|e| format!("Couldn't save the download: {e}"))?;
    let hex: String = hash.finalize().iter().map(|b| format!("{b:02x}")).collect();
    if got != bytes || hex != sha256 {
        return Err("The download is damaged or not the expected file. Try again.".into());
    }
    Ok(())
}

/// What was captured while listening, at the engine's rate.
#[derive(Default)]
pub struct Take {
    pub rate: u32,
    pub samples: Vec<f32>,
    pub peak: f32,
}

impl Take {
    pub fn new(rate: u32) -> Take {
        Take { rate, samples: Vec::with_capacity(rate as usize * 10), peak: 0.0 }
    }
    pub fn seconds(&self) -> f32 {
        if self.rate == 0 { 0.0 } else { self.samples.len() as f32 / self.rate as f32 }
    }
    /// Nothing louder than a quiet room was heard.
    pub fn silent(&self) -> bool {
        self.peak < SILENT_PEAK
    }
    /// Keeps at most [`MAX_SECONDS`].
    pub fn push(&mut self, samples: &[f32], peak: f32) {
        let cap = (MAX_SECONDS * self.rate as f32) as usize;
        let room = cap.saturating_sub(self.samples.len());
        let take = samples.len().min(room);
        self.samples.extend_from_slice(&samples[..take]);
        self.peak = self.peak.max(peak);
    }
    pub fn full(&self) -> bool {
        self.seconds() >= MAX_SECONDS
    }
}

/// Mono audio at `rate` to the model's 16 kHz (band-limited; see resample.rs).
pub fn to_16k(samples: &[f32], rate: u32) -> Vec<f32> {
    match crate::resample::Resampler::new(rate, MODEL_RATE) {
        Some(r) if rate != MODEL_RATE => r.process(samples, 1).into_owned(),
        _ => samples.to_vec(),
    }
}

/// Recognises speech in `audio` (mono, 16 kHz). `language` is a code from [`LANGUAGES`]
/// ("auto" detects it). Returns the text, trimmed; empty when nothing was said.
pub fn transcribe(model: &Path, audio: &[f32], language: &str) -> Result<String, String> {
    use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};
    let path = model.to_str().ok_or("The speech model's path is not valid text.")?;
    let ctx = WhisperContext::new_with_params(path, WhisperContextParameters::default())
        .map_err(|e| format!("Couldn't load speech recognition ({e}). Download it again in Settings › Downloads."))?;
    let mut state = ctx.create_state().map_err(|e| format!("Couldn't start speech recognition ({e})."))?;
    let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
    params.set_language(Some(if language.is_empty() { "auto" } else { language }));
    params.set_translate(false);
    params.set_no_context(true);
    params.set_single_segment(false);
    params.set_suppress_blank(true);
    params.set_print_special(false);
    params.set_print_progress(false);
    params.set_print_realtime(false);
    params.set_print_timestamps(false);
    let threads = std::thread::available_parallelism().map_or(2, |n| n.get()).clamp(1, 8);
    params.set_n_threads(threads as i32);
    // whisper.cpp needs at least one second; pad short takes with silence.
    let mut padded;
    let audio = if audio.len() < MODEL_RATE as usize + MODEL_RATE as usize / 10 {
        padded = audio.to_vec();
        padded.resize(MODEL_RATE as usize + MODEL_RATE as usize / 10, 0.0);
        &padded[..]
    } else {
        audio
    };
    state.full(params, audio).map_err(|e| format!("Speech recognition failed ({e})."))?;
    let mut text = String::new();
    for seg in state.as_iter() {
        if let Ok(s) = seg.to_str_lossy() {
            text.push_str(&s);
        }
    }
    Ok(clean(&text))
}

/// Tidies the model's text: trims it, joins its segments with single spaces and drops the
/// bracketed non-speech tags it emits for silence or noise ("[BLANK_AUDIO]", "(music)").
pub fn clean(text: &str) -> String {
    let mut out = String::new();
    let mut depth = 0i32;
    for ch in text.chars() {
        match ch {
            '[' | '(' => depth += 1,
            ']' | ')' if depth > 0 => depth -= 1,
            _ if depth == 0 => out.push(ch),
            _ => {}
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Silences whisper.cpp's own logging (it writes to stderr otherwise).
pub fn quiet_logs() {
    whisper_rs::install_logging_hooks();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_drops_non_speech_tags_and_joins_segments() {
        assert_eq!(clean(" Add a reverb\n to the bass track. "), "Add a reverb to the bass track.");
        assert_eq!(clean("[BLANK_AUDIO]"), "");
        assert_eq!(clean(" (music) Play it  [noise] again"), "Play it again");
    }

    #[test]
    fn a_take_stops_at_the_limit_and_knows_silence() {
        let mut t = Take::new(1000);
        assert!(t.silent());
        t.push(&vec![0.001; 30_000], 0.001);
        assert!(t.silent() && !t.full());
        t.push(&vec![0.2; 40_000], 0.2);
        assert_eq!(t.samples.len(), 60_000);
        assert!(t.full() && !t.silent());
    }

    #[test]
    fn resampling_to_16k_keeps_length_and_a_low_tone() {
        let rate = 44_100;
        let tone: Vec<f32> = (0..rate).map(|i| (i as f32 * 440.0 * std::f32::consts::TAU / rate as f32).sin() * 0.5).collect();
        let out = to_16k(&tone, rate);
        assert!((out.len() as i64 - 16_000).abs() <= 2, "{}", out.len());
        let rms = (out[2000..14000].iter().map(|v| v * v).sum::<f32>() / 12000.0).sqrt();
        assert!((rms - 0.5 / 2f32.sqrt()).abs() < 0.01, "{rms}");
        assert_eq!(to_16k(&[0.1, 0.2], 16_000), vec![0.1, 0.2]);
    }

    #[test]
    fn voice_prefs_default_and_language() {
        let p = VoicePrefs::default();
        assert!(p.is_default());
        assert_eq!(p.language_index(), 0);
        assert_eq!(p.source(), InputSource { channel: 0, stereo: false });
        let q = VoicePrefs { language: "fr".into(), ..p };
        assert_eq!(LANGUAGES[q.language_index()].1, "French");
        assert_eq!(size_label(MODEL.bytes), "82 MB");
    }

    /// A one-shot HTTP server on 127.0.0.1 that answers every request with `body`.
    fn serve(body: Vec<u8>) -> String {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            if let Ok((mut s, _)) = listener.accept() {
                let mut req = [0u8; 2048];
                let _ = s.read(&mut req);
                let head = format!("HTTP/1.1 200 OK\r\ncontent-length: {}\r\nconnection: close\r\n\r\n", body.len());
                let _ = s.write_all(head.as_bytes());
                let _ = s.write_all(&body);
            }
        });
        format!("http://{addr}/model.bin")
    }

    fn sha(b: &[u8]) -> String {
        Sha256::digest(b).iter().map(|x| format!("{x:02x}")).collect()
    }

    #[test]
    fn download_keeps_a_good_file_and_removes_a_bad_one() {
        let dir = std::env::temp_dir().join(format!("dstudio-voice-dl-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let dest = dir.join("m.bin");
        let body: Vec<u8> = (0..200_000u32).map(|i| (i % 251) as u8).collect();
        let (progress, cancel) = (AtomicU64::new(0), AtomicBool::new(false));

        let url = serve(body.clone());
        download(&url, &sha(&body), body.len() as u64, &dest, &progress, &cancel).unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), body);
        assert_eq!(progress.load(Ordering::Relaxed), body.len() as u64);
        std::fs::remove_file(&dest).unwrap();

        let url = serve(body.clone());
        let err = download(&url, &sha(b"other"), body.len() as u64, &dest, &progress, &cancel).unwrap_err();
        assert!(err.contains("damaged"), "{err}");
        assert!(!dest.exists() && !dest.with_extension("part").exists());

        let url = serve(body.clone());
        cancel.store(true, Ordering::Relaxed);
        assert!(download(&url, &sha(&body), body.len() as u64, &dest, &progress, &cancel).is_err());
        assert!(!dest.exists() && !dest.with_extension("part").exists());

        assert!(download("ftp://example.com/x", "", 1, &dest, &progress, &cancel).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The real model on real speech: the first 4.6 s of the whisper.cpp project's sample, a
    /// public-domain 1961 speech recording (fixtures/speech/fellow-americans.wav, 16 kHz). It is
    /// taken up to 44.1 kHz first, so the conversion back to 16 kHz is part of the test. Needs a
    /// model: DSTUDIO_SPEECH_MODEL names a whisper.cpp model file; CI downloads the tiny one
    /// (.github/workflows/ci.yml) so this runs there on every platform.
    #[test]
    fn transcribes_real_speech() {
        let Some(model) = std::env::var_os("DSTUDIO_SPEECH_MODEL").map(PathBuf::from) else {
            eprintln!("DSTUDIO_SPEECH_MODEL not set: speech recognition test not run");
            return;
        };
        quiet_logs();
        let (rate, samples) = read_wav_mono(&fixture("fellow-americans.wav"));
        assert_eq!(rate, MODEL_RATE);
        let at_44k = crate::resample::Resampler::new(rate, 44_100).unwrap().process(&samples, 1).into_owned();
        let text = transcribe(&model, &to_16k(&at_44k, 44_100), "en").unwrap().to_lowercase();
        assert!(text.contains("fellow americans"), "{text}");
        // The language guess works too, and silence gives no text.
        let text = transcribe(&model, &samples, "auto").unwrap().to_lowercase();
        assert!(text.contains("americans"), "{text}");
        let text = transcribe(&model, &vec![0.0; 32_000], "auto").unwrap();
        assert!(text.is_empty(), "{text:?}");
    }

    fn fixture(name: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/speech").join(name)
    }

    /// 16-bit PCM mono WAV (the fixture's format).
    fn read_wav_mono(path: &Path) -> (u32, Vec<f32>) {
        let b = std::fs::read(path).unwrap();
        let rate = u32::from_le_bytes(b[24..28].try_into().unwrap());
        let mut i = 12;
        while &b[i..i + 4] != b"data" {
            i += 8 + u32::from_le_bytes(b[i + 4..i + 8].try_into().unwrap()) as usize;
        }
        let n = u32::from_le_bytes(b[i + 4..i + 8].try_into().unwrap()) as usize;
        let data = &b[i + 8..i + 8 + n];
        (rate, data.chunks_exact(2).map(|c| i16::from_le_bytes([c[0], c[1]]) as f32 / 32768.0).collect())
    }
}
