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
}

impl AudioPrefs {
    /// True when the preferences ask for anything beyond the system defaults.
    pub fn is_custom(&self) -> bool {
        self.output.is_some() || self.input.is_some() || self.sample_rate > 0.0 || self.buffer_size > 0
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
    /// The docked assistant spans the full window height, over the bottom panel's right end
    /// (its header button), instead of the timeline row only.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub assistant_full: bool,
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
    fn the_default_projects_folder_is_not_written() {
        let json = serde_json::to_string(&GeneralPrefs::default()).unwrap();
        assert!(!json.contains("projects_folder"), "{json}");
    }
}
