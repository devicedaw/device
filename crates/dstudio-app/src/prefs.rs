//! App preferences (Settings › Audio). Device setup is an app preference, not
//! project state (docs/adr/0002-recording.md), so it lives in a small JSON file
//! in the user's app-config directory, never in a project:
//!
//! - macOS: `~/Library/Application Support/Device Studio/preferences.json`
//! - Linux: `$XDG_CONFIG_HOME/device-studio/preferences.json` (or `~/.config/...`)
//! - Windows: `%APPDATA%\Device Studio\preferences.json`
//!
//! `DSTUDIO_PREFS_PATH` overrides the location. `--self-test` and the
//! `--state`/`--screenshot` runs never read or write this file.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AudioPrefs {
    /// Output device id (Core Audio UID on macOS); None = system default.
    pub output: Option<String>,
    pub output_name: Option<String>,
    /// Input device id; None = no input device. Only ever set by the user
    /// picking an input in Settings › Audio (opening an input can show the
    /// macOS microphone-permission prompt).
    pub input: Option<String>,
    pub input_name: Option<String>,
    /// 0 = device default.
    pub sample_rate: f64,
    /// 0 = device default.
    pub buffer_size: u32,
    pub input_muted: bool,
    /// Plugin delay compensation switched off (stored inverted so files
    /// written before this field default to compensation ON).
    #[serde(default)]
    pub pdc_off: bool,
    /// Settings › Audio › Recording offset (spec 020): input device id -> milliseconds,
    /// positive = takes placed earlier. Devices left at 0 are not stored.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub recording_offsets: std::collections::BTreeMap<String, f64>,
}

/// Recording offset range, milliseconds (spec 020). Values are kept to 0.1 ms.
pub const RECORDING_OFFSET_MAX_MS: f64 = 100.0;

/// `ms` clamped to ±[RECORDING_OFFSET_MAX_MS] and rounded to 0.1 ms; a value that isn't a
/// number is 0.
pub fn clamp_recording_offset(ms: f64) -> f64 {
    if !ms.is_finite() {
        return 0.0;
    }
    let v = (ms.clamp(-RECORDING_OFFSET_MAX_MS, RECORDING_OFFSET_MAX_MS) * 10.0).round() / 10.0;
    if v == 0.0 { 0.0 } else { v } // no -0.0
}

impl AudioPrefs {
    /// True when the preferences ask for anything beyond the system defaults.
    pub fn is_custom(&self) -> bool {
        self.output.is_some() || self.input.is_some() || self.sample_rate > 0.0 || self.buffer_size > 0
    }

    /// The recording offset of the selected input device in ms (0 with no input, or one never
    /// set). A value edited into the file by hand is clamped like a typed one.
    pub fn recording_offset_ms(&self) -> f64 {
        self.input.as_ref().and_then(|id| self.recording_offsets.get(id)).map_or(0.0, |&v| clamp_recording_offset(v))
    }

    /// Store `ms` (clamped and rounded) for the selected input device; 0 removes the entry.
    /// Returns false when no input device is selected.
    pub fn set_recording_offset_ms(&mut self, ms: f64) -> bool {
        let Some(id) = self.input.clone() else { return false };
        let v = clamp_recording_offset(ms);
        if v == 0.0 {
            self.recording_offsets.remove(&id);
        } else {
            self.recording_offsets.insert(id, v);
        }
        true
    }

    /// The offset in engine frames at `rate` (positive = earlier).
    pub fn recording_offset_frames(&self, rate: f64) -> i32 {
        (self.recording_offset_ms() * rate / 1000.0).round() as i32
    }

    pub fn device_config(&self) -> dstudio_engine::DeviceConfig {
        dstudio_engine::DeviceConfig {
            output: self.output.clone(),
            input: self.input.clone(),
            sample_rate: self.sample_rate,
            buffer_size: self.buffer_size,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Preferences {
    pub audio: AudioPrefs,
    /// Interface zoom in percent (View › Zoom In / Out / Reset); 0 = default (100).
    #[serde(default)]
    pub interface_zoom: u32,
    /// Browser › Files › Folders: folders the user added, oldest first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub browser_folders: Vec<String>,
    /// Browser preview volume, linear gain 0..2 (None = 1).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preview_volume: Option<f32>,
    /// Docked assistant width in design units (drag its splitter); 0 = default.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub assistant_width: f32,
    /// Device chain "+" menu › Recent: effects added lately, newest first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub recent_devices: Vec<dstudio_core::PluginRef>,
    /// The docked assistant spans the full window height, over the bottom panel's right end
    /// (its header button), instead of the timeline row only.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub assistant_full: bool,
    /// Settings › General › Let AI apps control Device (spec 023): the MCP server is on.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub ai_apps: bool,
    /// AI apps (their MCP client names) the user allowed to change projects.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ai_apps_allowed: Vec<String>,
    /// File › Open Recent: project folders, newest first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub recent_projects: Vec<String>,
    /// Canonical form of each `recent_projects` entry, resolved once when it was added,
    /// so de-duplication never touches the disk. Empty for entries saved before this field.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub recent_resolved: Vec<String>,
    #[serde(default)]
    pub general: GeneralPrefs,
    /// Timeline › Returns pill off: return track rows hidden (view state, not project state).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub hide_returns: bool,
    /// Settings › MIDI.
    #[serde(default, skip_serializing_if = "MidiPrefs::is_default")]
    pub midi: MidiPrefs,
    /// Settings › Plugins › Add folder: plugin folders searched besides the standard ones,
    /// oldest first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub plugin_folders: Vec<String>,
    /// Record button menu: bars of metronome before a recording starts (0 = no count-in).
    #[serde(default, skip_serializing_if = "is_zero_u8")]
    pub count_in_bars: u8,
    /// Settings › Shortcuts: actions whose keys the user changed, action id -> chords in
    /// `Chord::parse` form (an empty list = unmapped). Actions not listed keep their defaults.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub shortcuts: std::collections::BTreeMap<String, Vec<String>>,
    /// Settings › Shortcuts › Preset: the other DAW whose keys the keymap starts from
    /// (`shortcut_presets::PRESETS`; empty = Device Studio's own). `shortcuts` holds the
    /// changes on top of it.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub shortcut_preset: String,
    /// Settings › Privacy › Send crash reports switched off (spec 018); on by default, so
    /// files without it send them.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub crash_reports_off: bool,
    /// Assistant › Getting started tour: its step, and whether the card was closed.
    #[serde(default, skip_serializing_if = "crate::tour::TourPrefs::is_default")]
    pub assistant_tour: crate::tour::TourPrefs,
}

fn is_zero_u8(v: &u8) -> bool {
    *v == 0
}

/// Settings › MIDI: MIDI inputs are on by default, including ones plugged in
/// later, so only the ones the user switched off are stored (by engine port id).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MidiPrefs {
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub disabled_inputs: Vec<String>,
    /// Settings › MIDI › Default instrument (spec 013): what a new empty instrument track
    /// plays. None = the built-in Synth.
    #[serde(skip_serializing_if = "Option::is_none", deserialize_with = "lenient_instrument")]
    pub default_instrument: Option<InstrumentChoice>,
}

impl MidiPrefs {
    pub fn is_default(&self) -> bool {
        self.disabled_inputs.is_empty() && self.default_instrument.is_none()
    }
}

/// A default instrument other than the Synth.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum InstrumentChoice {
    /// A built-in instrument, by its device id (`dstudio_core::builtin::find`).
    Builtin { id: String },
    /// An instrument plugin, matched against the scan by format and id.
    Plugin { plugin: dstudio_core::PluginRef },
}

/// A default instrument entry that doesn't parse loads as the Synth rather than throwing away
/// the whole preferences file.
fn lenient_instrument<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<InstrumentChoice>, D::Error> {
    let v = serde_json::Value::deserialize(d)?;
    Ok(serde_json::from_value(v).ok())
}

/// Settings › General, with its defaults.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GeneralPrefs {
    /// Startup: reopen the most recent project at launch.
    pub load_last_session: bool,
    /// Transport › Playhead on stop: false = Return to start, true = Stop in place.
    pub stop_in_place: bool,
    /// Plugin windows: open a third-party effect's window when it is added.
    pub show_plugin_windows: bool,
    /// Plugin windows: open a built-in effect's device window when it is added.
    pub show_builtin_windows: bool,
    /// Plugin windows: keep plugin windows above the main window.
    pub plugin_windows_on_top: bool,
    /// Projects › Projects folder: where Save starts and what the browser's Projects tab lists.
    /// None is the default folder (~/Documents/Device Studio).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub projects_folder: Option<std::path::PathBuf>,
    /// Timeline annotations: moving, resizing, adding or deleting a marker moves the clips and
    /// automation in and after it.
    pub sections_move_clips: bool,
    /// Startup: look for a newer release once at launch (updates.rs).
    pub check_updates_at_launch: bool,
    /// Assistant › Let the assistant listen to audio: each turn includes a short text
    /// description of the selection's sound (dstudio_core::listen). Off by default, because
    /// that description is sent to the assistant's provider.
    pub assistant_listen: bool,
}

impl Default for GeneralPrefs {
    fn default() -> Self {
        GeneralPrefs {
            load_last_session: false,
            stop_in_place: false,
            show_plugin_windows: true,
            show_builtin_windows: false,
            plugin_windows_on_top: true,
            projects_folder: None,
            sections_move_clips: false,
            check_updates_at_launch: true,
            assistant_listen: false,
        }
    }
}

fn is_zero(v: &f32) -> bool {
    *v == 0.0
}

/// The app's support directory (see the module docs), shared by the preferences
/// file and the plugin scan cache.
pub fn support_dir() -> Option<PathBuf> {
    support_dir_named("Device Studio", "device-studio")
}

fn support_dir_named(name: &str, linux_name: &str) -> Option<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    Some(if cfg!(target_os = "macos") {
        home?.join("Library/Application Support").join(name)
    } else if cfg!(windows) {
        PathBuf::from(std::env::var_os("APPDATA")?).join(name)
    } else {
        std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).or_else(|| home.map(|h| h.join(".config")))?.join(linux_name)
    })
}

/// Default preferences file location (see the module docs).
pub fn default_path() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("DSTUDIO_PREFS_PATH") {
        return Some(PathBuf::from(p));
    }
    Some(support_dir()?.join("preferences.json"))
}

/// Plugin scan cache (`dstudio_engine::scan_plugins`), next to the preferences:
/// `~/Library/Application Support/Device Studio/plugin-cache.json` on macOS.
/// `DSTUDIO_PLUGIN_CACHE` overrides the location.
pub fn plugin_cache_path() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("DSTUDIO_PLUGIN_CACHE") {
        return Some(PathBuf::from(p));
    }
    Some(support_dir()?.join("plugin-cache.json"))
}

/// Missing file = defaults; a corrupt file is reported and ignored.
pub fn load(path: &Path) -> Result<Option<Preferences>, String> {
    match std::fs::read_to_string(path) {
        Ok(s) => serde_json::from_str(&s).map(Some).map_err(|e| format!("{}: {e}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}

pub fn save(path: &Path, p: &Preferences) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    let tmp = path.with_extension("json.tmp");
    let text = serde_json::to_string_pretty(p).map_err(|e| e.to_string())?;
    std::fs::write(&tmp, text).map_err(|e| format!("{}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("{}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn roundtrip_and_defaults() {
        let dir = std::env::temp_dir().join(format!("dstudio-prefs-test-{}", std::process::id()));
        let path = dir.join("preferences.json");
        assert_eq!(load(&path).unwrap(), None);
        let p = Preferences {
            audio: AudioPrefs { output: Some("out-uid".into()), buffer_size: 256, input_muted: true, ..Default::default() },
            interface_zoom: 110,
            browser_folders: vec!["/tmp/kit".into()],
            preview_volume: Some(0.5),
            assistant_width: 400.0,
            recent_devices: vec![dstudio_core::builtin::find("delay").unwrap().plugin_ref()],
            assistant_full: true,
            ai_apps: true,
            ai_apps_allowed: vec!["Claude Desktop".into()],
            recent_projects: vec!["/tmp/Song.dsproj".into()],
            recent_resolved: vec!["/private/tmp/Song.dsproj".into()],
            general: GeneralPrefs {
                load_last_session: true,
                stop_in_place: true,
                projects_folder: Some("/tmp/My Songs".into()),
                ..GeneralPrefs::default()
            },
            midi: MidiPrefs {
                disabled_inputs: vec!["coremidi:42".into()],
                default_instrument: Some(InstrumentChoice::Builtin { id: "sampler-steam".into() }),
            },
            hide_returns: true,
            plugin_folders: vec!["/opt/plugins".into()],
            count_in_bars: 2,
            shortcuts: [("transport.record".to_string(), vec!["primary+shift+r".to_string()])].into_iter().collect(),
            shortcut_preset: "Logic Pro".into(),
            crash_reports_off: true,
            assistant_tour: crate::tour::TourPrefs { step: 2, closed: false },
        };
        save(&path, &p).unwrap();
        assert_eq!(load(&path).unwrap(), Some(p.clone()));
        assert!(p.audio.is_custom());
        assert!(!AudioPrefs::default().is_custom());
        // Unknown/missing fields are tolerated.
        std::fs::write(&path, r#"{"audio":{"buffer_size":512},"future":1}"#).unwrap();
        let loaded = load(&path).unwrap().unwrap();
        assert_eq!(loaded.audio.buffer_size, 512);
        assert_eq!(loaded.general, GeneralPrefs::default(), "files without General get the defaults");
        assert!(loaded.general.show_plugin_windows && loaded.general.plugin_windows_on_top);
        assert!(!loaded.hide_returns, "files without the Returns pill state show the returns");
        assert!(loaded.midi.disabled_inputs.is_empty(), "MIDI inputs default to on");
        assert!(loaded.plugin_folders.is_empty(), "files without custom plugin folders have none");
        assert!(loaded.shortcuts.is_empty(), "files without shortcut changes keep the defaults");
        assert!(loaded.shortcut_preset.is_empty(), "files without a preset use Device Studio's keys");
        assert_eq!(loaded.count_in_bars, 0, "files without a count-in record straight away");
        assert!(!loaded.crash_reports_off, "files without the Privacy switch send crash reports");
        assert_eq!(loaded.general.projects_folder, None, "files without a projects folder use the default one");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_default_instrument_round_trips_and_bad_entries_load_as_the_synth() {
        let plugin = dstudio_core::PluginRef {
            format: dstudio_core::PluginFormat::Clap,
            id: "com.example.keys".into(),
            path: Some("/tmp/Keys.clap".into()),
            name: "Keys".into(),
            vendor: "Example".into(),
            version: "1.0".into(),
            kind: dstudio_core::PluginKind::Instrument,
        };
        for choice in [None, Some(InstrumentChoice::Builtin { id: "drum-rack".into() }), Some(InstrumentChoice::Plugin { plugin })] {
            let p = Preferences { midi: MidiPrefs { default_instrument: choice.clone(), ..Default::default() }, ..Default::default() };
            let back: Preferences = serde_json::from_str(&serde_json::to_string(&p).unwrap()).unwrap();
            assert_eq!(back.midi.default_instrument, choice);
        }
        assert!(!serde_json::to_string(&Preferences::default()).unwrap().contains("default_instrument"));
        let bad: Preferences =
            serde_json::from_str(r#"{"midi":{"disabled_inputs":["x"],"default_instrument":{"kind":"theremin"}},"count_in_bars":1}"#)
                .unwrap();
        assert_eq!(bad.midi.default_instrument, None, "an entry that doesn't parse is the Synth");
        assert_eq!((bad.midi.disabled_inputs.len(), bad.count_in_bars), (1, 1), "and the rest still loads");
    }

    #[test]
    fn recording_offsets_are_kept_per_input_device() {
        let mut a = AudioPrefs::default();
        assert!(!a.set_recording_offset_ms(4.0), "no input device: nothing to store it under");
        assert_eq!(a.recording_offset_ms(), 0.0);
        a.input = Some("interface-a".into());
        assert!(a.set_recording_offset_ms(4.26));
        assert_eq!(a.recording_offset_ms(), 4.3, "rounded to 0.1 ms");
        assert_eq!(a.recording_offset_frames(48_000.0), 206); // 4.3 ms at 48 kHz, rounded
        a.input = Some("interface-b".into());
        assert_eq!(a.recording_offset_ms(), 0.0, "another device starts at 0");
        a.set_recording_offset_ms(-250.0);
        assert_eq!(a.recording_offset_ms(), -100.0, "clamped to -100 ms");
        assert_eq!(a.recording_offset_frames(44_100.0), -4410);
        a.input = Some("interface-a".into());
        assert_eq!(a.recording_offset_ms(), 4.3, "each device keeps its own");
        // Round trip through the file format; a device set back to 0 is not stored.
        let p = Preferences { audio: a.clone(), ..Default::default() };
        let back: Preferences = serde_json::from_str(&serde_json::to_string(&p).unwrap()).unwrap();
        assert_eq!(back.audio, a);
        a.set_recording_offset_ms(0.0);
        assert_eq!(a.recording_offsets.len(), 1);
        a.input = Some("interface-b".into());
        a.set_recording_offset_ms(0.04);
        assert!(!serde_json::to_string(&a).unwrap().contains("recording_offsets"), "nothing stored when every offset is 0");
        assert_eq!(clamp_recording_offset(f64::NAN), 0.0);
        // Files from before this setting load with no offsets; a hand-edited value is clamped.
        let old: AudioPrefs = serde_json::from_str(r#"{"input":"interface-a"}"#).unwrap();
        assert_eq!(old.recording_offset_ms(), 0.0);
        let wild: AudioPrefs = serde_json::from_str(r#"{"input":"x","recording_offsets":{"x":999}}"#).unwrap();
        assert_eq!(wild.recording_offset_ms(), 100.0);
    }

    #[test]
    fn the_default_projects_folder_is_not_written() {
        let json = serde_json::to_string(&GeneralPrefs::default()).unwrap();
        assert!(!json.contains("projects_folder"), "{json}");
    }
}
