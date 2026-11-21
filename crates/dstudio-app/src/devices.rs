//! Plugin devices (docs/adr/0003-plugins.md): the scanned plugin catalogue,
//! the browser's Devices tab tree, the bottom-panel device chain (plugin
//! cards + "Add device"), the instrument selector, drag and drop from the
//! browser, plugin windows and Settings › Plugins.
//!
//! Every edit is a `dstudio_core::Command` (`AddDevice`, `RemoveDevice`,
//! `MoveDevice`, `SetDeviceEnabled`, `SetInstrumentPlugin`), one undo step
//! each. The engine owns plugin instances; this module only asks it for
//! load errors, latency and editor windows (main thread).
//!
//! Scanning (`dstudio_engine::scan_plugins`) runs on a worker thread at startup
//! (cache only) and from Settings › Plugins (cache or forced rescan). Its
//! progress goes to the status line; the UI never waits for it.
//!
//! TEST seam: `--plugin-fixture <file.json>` replaces the scan with a fixed
//! report (no engine call). It exists to exercise the UI flows while the
//! engine's scanner is unavailable; it is not a feature.

use crate::controller::{BottomTab, Controller, JobResult, sync_model};
use crate::highlight::Layout;
use crate::{DeviceCard, MenuItem, PluginRow, Studio, TreeRow};
use dstudio_core::builtin::DeviceDef;
use dstudio_core::{Command, Device, DeviceId, ModParam, PluginFormat, PluginKind, PluginRef, TrackId, TrackKind};
use dstudio_engine::{PluginInfo, ScanReport};
use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

// ------------------------------------------------------------------ geometry (window / bottom-panel points)

/// Plugin card width.
pub const CARD_W: f32 = 220.0;
/// Gap between cards: the Synth card ends at 1040 and "Add device" starts at 1056.
pub const CARD_GAP: f32 = 16.0;
/// First card, bottom-panel relative (Synth card at window 268.5).
pub const CHAIN_X0: f32 = 263.5;
pub const SYNTH_W: f32 = 776.5;
/// "Add device" alone (audio track, no devices): at window 270.
pub const ADD_X_ALONE: f32 = 265.0;
pub const ADD_W: f32 = 182.5;
/// Bottom panel x (window) and its device area start (after the inspector).
pub(crate) const PANEL_X: f32 = 5.0;
pub(crate) const CHAIN_AREA_X: f32 = 255.0;
/// Browser tree: area top (window) and row pitch.
pub const TREE_Y: f32 = 78.0 + 81.0;
pub const TREE_TOP: f32 = 4.0;
pub const TREE_ROW: f32 = 22.0;
/// Device popup list rows.
pub const MENU_ROW: f32 = 22.0;
pub const MENU_PAD: f32 = 4.0;
/// Drag threshold (points) before a press becomes a drag.
const DRAG_SLOP: f32 = 4.0;

// ------------------------------------------------------------------ catalogue

/// Something the browser / Add device card can add.
#[derive(Debug, Clone, PartialEq)]
pub enum Item {
    /// Our built-in Synth, plain or with one of its factory sounds (index into
    /// `SYNTH_V2.factory`; the browser's Synth folder).
    BuiltinSynth(Option<u16>),
    /// A built-in device (dstudio_core::builtin), added as an insert.
    Builtin(&'static DeviceDef),
    Plugin(PluginRef),
    /// A saved or factory device chain (src/chain_presets.rs), added as a whole.
    Chain(crate::chain_presets::ChainRef),
}

impl Item {
    pub fn name(&self) -> &str {
        match self {
            Item::BuiltinSynth(_) => "Synth",
            Item::Builtin(d) => d.name,
            Item::Plugin(p) => &p.name,
            Item::Chain(c) => &c.name,
        }
    }
    pub fn kind(&self) -> PluginKind {
        match self {
            Item::BuiltinSynth(_) => PluginKind::Instrument,
            Item::Builtin(d) => d.kind,
            Item::Plugin(p) => p.kind,
            Item::Chain(_) => PluginKind::Effect,
        }
    }
}

/// A new Synth device, with factory sound `preset` loaded when given.
pub fn synth_with(preset: Option<u16>) -> Device {
    let def = &dstudio_core::builtin::synth_v2::SYNTH_V2;
    let mut d = def.instantiate();
    if let Some(f) = preset.and_then(|i| def.factory.get(i as usize)) {
        d.params = def.factory_values(f);
        d.preset = Some(f.name.into());
    }
    d
}

/// The built-in Synth (synth-v2): an instrument device with its own card (src/synth.rs).
pub fn is_builtin_synth(d: &Device) -> bool {
    dstudio_core::builtin::def_of(d).is_some_and(|def| def.id == dstudio_core::builtin::synth_v2::SYNTH_V2.id)
}

pub fn format_badge(f: PluginFormat) -> &'static str {
    match f {
        PluginFormat::AudioUnit => "AU",
        PluginFormat::Vst3 => "VST3",
        PluginFormat::Clap => "CLAP",
        PluginFormat::Builtin => "Built-in",
    }
}

fn format_order(f: PluginFormat) -> u8 {
    match f {
        PluginFormat::AudioUnit => 0,
        PluginFormat::Vst3 => 1,
        PluginFormat::Clap => 2,
        PluginFormat::Builtin => 3,
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub enum ScanState {
    #[default]
    Idle,
    Scanning,
    Done,
    Failed(String),
}

#[derive(Debug, Default)]
pub struct Catalog {
    pub plugins: Vec<PluginInfo>,
    pub failed: Vec<(String, String)>,
    pub from_cache: bool,
    pub state: ScanState,
    /// Latest progress line from the scanner thread.
    pub progress: Arc<Mutex<String>>,
    shown_progress: String,
    pub cache_path: Option<PathBuf>,
    /// TEST seam (`--plugin-fixture`): used instead of `scan_plugins`.
    pub fixture: Option<ScanReport>,
    /// SCREENSHOT-ONLY (`--state` runs): keep only Apple's system Audio Units, which every
    /// Mac has, so committed captures never list the user's installed third-party plugins
    /// (same policy as `recording::screenshot_fake_devices`).
    pub screenshot_apple_only: bool,
    /// Scans started so far and the `rescan` flag of the last one.
    pub scans: usize,
    pub last_rescan: bool,
    /// The custom plugin folders changed while a scan ran: check again when it ends.
    pub check_again: bool,
}

impl Catalog {
    /// Plugins of one kind, sorted by format, vendor, name.
    pub fn of_kind(&self, kind: PluginKind) -> Vec<&PluginInfo> {
        let mut v: Vec<&PluginInfo> = self.plugins.iter().filter(|p| p.plugin.kind == kind).collect();
        v.sort_by(|a, b| {
            (format_order(a.plugin.format), a.plugin.vendor.to_lowercase(), a.plugin.name.to_lowercase()).cmp(&(
                format_order(b.plugin.format),
                b.plugin.vendor.to_lowercase(),
                b.plugin.name.to_lowercase(),
            ))
        });
        v
    }

    pub fn find(&self, p: &PluginRef) -> Option<&PluginInfo> {
        self.plugins.iter().find(|i| i.plugin.format == p.format && i.plugin.id == p.id)
    }
}

/// TEST seam file format for `--plugin-fixture`.
#[derive(Debug, Deserialize)]
struct FixtureFile {
    plugins: Vec<FixturePlugin>,
    #[serde(default)]
    failed: Vec<(String, String)>,
}
#[derive(Debug, Deserialize)]
struct FixturePlugin {
    plugin: PluginRef,
    #[serde(default = "yes")]
    has_editor: bool,
}
fn yes() -> bool {
    true
}

pub fn load_fixture(path: &Path) -> Result<ScanReport, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("--plugin-fixture {}: {e}", path.display()))?;
    let f: FixtureFile = serde_json::from_str(&text).map_err(|e| format!("--plugin-fixture {}: {e}", path.display()))?;
    Ok(ScanReport {
        plugins: f
            .plugins
            .into_iter()
            .map(|p| {
                let outs = 2;
                let ins = if p.plugin.kind == PluginKind::Effect { 2 } else { 0 };
                PluginInfo { plugin: p.plugin, has_editor: p.has_editor, audio_inputs: ins, audio_outputs: outs }
            })
            .collect(),
        failed: f.failed,
        from_cache: false,
    })
}

/// Standard plugin locations shown in Settings › Plugins: the folders the
/// out-of-process scanner searches (engine/scanner/scanner_main.cpp), per
/// platform. macOS: Apple's system Audio Units are found through the Audio
/// Component registry; the Components folders are listed for reference.
pub fn scan_dirs() -> Vec<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let mut v: Vec<PathBuf> = Vec::new();
    if cfg!(target_os = "macos") {
        v.extend(["Components", "VST3", "CLAP"].iter().map(|d| PathBuf::from("/Library/Audio/Plug-Ins").join(d)));
        if let Some(home) = &home {
            v.extend(["Components", "VST3", "CLAP"].iter().map(|d| home.join("Library/Audio/Plug-Ins").join(d)));
        }
    } else if cfg!(windows) {
        for d in ["VST3", "CLAP"] {
            if let Some(common) = std::env::var_os("COMMONPROGRAMFILES") {
                v.push(PathBuf::from(common).join(d));
            }
            if let Some(local) = std::env::var_os("LOCALAPPDATA") {
                v.push(PathBuf::from(local).join("Programs").join("Common").join(d));
            }
        }
    } else {
        if let Some(home) = &home {
            v.push(home.join(".vst3"));
        }
        v.extend(["/usr/lib/vst3", "/usr/local/lib/vst3"].iter().map(PathBuf::from));
        if let Some(home) = &home {
            v.push(home.join(".clap"));
        }
        v.extend(["/usr/lib/clap", "/usr/local/lib/clap"].iter().map(PathBuf::from));
    }
    v
}

/// Whether the engine can host plugin editor windows here: macOS (Cocoa),
/// Windows (Win32) and Linux (X11, which also runs under XWayland). An engine
/// built without libX11 says so when a window is opened. The scanned
/// `has_editor` capability is kept as is.
pub const EDITORS_SUPPORTED: bool = true;

/// Settings › General › Keep plugin windows on top. The engine opens plugin editors as plain
/// NSWindows; the main window and the file panels are subclasses, so only editors float.
#[cfg(target_os = "macos")]
pub fn set_plugin_windows_on_top(on: bool) {
    use objc2::msg_send;
    const NORMAL: isize = 0;
    const FLOATING: isize = 3;
    for w in plugin_windows() {
        let _: () = unsafe { msg_send![w, setLevel: if on { FLOATING } else { NORMAL }] };
    }
}

#[cfg(not(target_os = "macos"))]
pub fn set_plugin_windows_on_top(_: bool) {}

/// Window levels of the open plugin editor windows (self-test).
#[cfg(target_os = "macos")]
pub fn plugin_window_levels() -> Vec<isize> {
    plugin_windows().into_iter().map(|w| unsafe { objc2::msg_send![w, level] }).collect()
}

#[cfg(not(target_os = "macos"))]
pub fn plugin_window_levels() -> Vec<isize> {
    Vec::new()
}

#[cfg(target_os = "macos")]
fn plugin_windows() -> Vec<*mut objc2::runtime::AnyObject> {
    use objc2::runtime::AnyObject;
    use objc2::{class, msg_send};
    unsafe {
        let app: *mut AnyObject = msg_send![class!(NSApplication), sharedApplication];
        let windows: *mut AnyObject = msg_send![app, windows];
        let n: usize = msg_send![windows, count];
        (0..n)
            .map(|i| -> *mut AnyObject { msg_send![windows, objectAtIndex: i] })
            .filter(|w| {
                let plain: bool = msg_send![*w, isMemberOfClass: class!(NSWindow)];
                let visible: bool = msg_send![*w, isVisible];
                plain && visible
            })
            .collect()
    }
}

/// Extra folders the scanner searches from the environment, with the variable
/// that set them: `CLAP_PATH` (Windows, Linux) and `DSTUDIO_PLUGIN_PATH`.
pub fn configured_scan_dirs() -> Vec<(PathBuf, &'static str)> {
    let vars: &[&'static str] = if cfg!(target_os = "macos") { &["DSTUDIO_PLUGIN_PATH"] } else { &["CLAP_PATH", "DSTUDIO_PLUGIN_PATH"] };
    let mut v = Vec::new();
    for &var in vars {
        if let Some(list) = std::env::var_os(var) {
            v.extend(std::env::split_paths(&list).filter(|p| !p.as_os_str().is_empty()).map(|p| (p, var)));
        }
    }
    v
}

/// Badge of a custom plugin folder row in Settings › Plugins (the rows that can be removed).
pub const CUSTOM_BADGE: &str = "Custom";

/// Adds `dir` to the custom plugin folders. Refused when the scanner already searches it (a
/// standard, environment or custom folder) or when its path cannot be passed to the scanner (a
/// ':' on macOS and Linux, a '"' on Windows).
pub fn add_plugin_folder(folders: &mut Vec<String>, dir: &Path) -> Result<(), String> {
    if std::env::join_paths([dir]).is_err() {
        return Err(format!("A plugin folder's path can't contain '{}'", if cfg!(windows) { '"' } else { ':' }));
    }
    let known = scan_dirs().into_iter().chain(configured_scan_dirs().into_iter().map(|(d, _)| d)).chain(folders.iter().map(PathBuf::from));
    if known.into_iter().any(|d| d == dir) {
        return Err("That folder is already scanned".into());
    }
    folders.push(dir.to_string_lossy().into_owned());
    Ok(())
}

// ------------------------------------------------------------------ UI state

/// One built browser row, parallel to the `TreeRow` model.
#[derive(Debug, Clone)]
pub struct TreeEntry {
    pub key: String,
    pub item: Option<Item>,
    pub folder: bool,
}

#[derive(Debug, Clone)]
pub struct DevMenu {
    pub pos: (f32, f32),
    pub w: f32,
    /// (label, right-hand text, action; None = header / note row, separator above)
    pub rows: Vec<(String, String, Option<MenuPick>, bool)>,
}

/// What a device popup row does.
#[derive(Debug, Clone, PartialEq)]
pub enum MenuPick {
    /// Add to the selected track (Add device submenus, instrument selector).
    Add(Item),
    /// Built-in device panel: sidechain source (None = off).
    Sidechain(DeviceId, Option<TrackId>),
    /// Chains: the folder list (Some = replace that chain head instead of adding).
    ChainFolders(Option<DeviceId>),
    /// Chains: one folder's chains.
    ChainFolder(Option<DeviceId>, String),
    /// Chain card: replace the chain with another.
    ChainReplace(DeviceId, crate::chain_presets::ChainRef),
    ChainSave(DeviceId),
    ChainSaveAs(DeviceId),
    ChainRename(DeviceId),
    ChainUnpack(DeviceId),
    ChainDelete(DeviceId),
    /// Built-in device panel: load factory preset `n`.
    Preset(DeviceId, usize),
    /// Built-in device panel: choice `n` of parameter index `i` (menu pill).
    Choice(DeviceId, i32, i32),
    /// Built-in device interactive view (src/device_editors.rs): action code.
    View(DeviceId, i32),
    /// Built-in device panel: parameter (index) set to a choice.
    Param(DeviceId, usize, f32),
    /// Built-in device panel: the mode entry of a type menu (type index, mode index).
    Mode(DeviceId, usize, usize),
    /// Control menu (src/device_mod.rs): show the parameter's automation lane.
    Automate(DeviceId, ModParam),
    /// Power switch menu: show the device's on/off automation lane.
    AutomateOn(DeviceId),
    /// Channel control menu (volume, pan, mute): show that lane of the track (None = master).
    AutomateTrack(Option<TrackId>, dstudio_core::AutomationTarget),
    /// Control menu: a new LFO aimed at the parameter.
    NewLfo(DeviceId, ModParam),
    /// Aim source output (source, slot) at (device, parameter).
    Link(DeviceId, u8, DeviceId, ModParam),
    /// Remove link `index` of a source.
    Unlink(DeviceId, usize),
    /// Card name menu: swap the device for another effect.
    Swap(DeviceId, Item),
    /// A new empty LFO on the device's track.
    AddLfo(DeviceId),
    /// Empty chain: add an effect into the chain headed by the device.
    AddToChain(DeviceId, Item),
    /// Plugin card: show / hide a parameter.
    CardParam(DeviceId, u32),
    /// Device window action (src/device_window.rs ACT_*): device, action, value.
    Window(DeviceId, i32, i32),
}

#[derive(Debug, Clone)]
pub struct Dnd {
    pub item: Item,
    pub start: (f32, f32),
    pub pos: (f32, f32),
    pub active: bool,
}

#[derive(Debug, Clone, Copy)]
pub struct CardDrag {
    pub id: DeviceId,
    pub start_x: f32,
    pub cur_x: f32,
    pub active: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DropTarget {
    Track(TrackId),
    /// Below the last track: an instrument dropped there gets a new instrument track.
    NewTrack,
    /// Insert position in the selected track's chain (None: instrument slot / end).
    Chain {
        track: TrackId,
        index: Option<usize>,
    },
}

#[derive(Debug)]
pub struct DevicesUi {
    pub catalog: Catalog,
    /// Browser Devices sub-tab: 0 Instruments, 1 Effects, 2 Chains.
    pub tab: usize,
    /// Saved and factory chains (src/chain_presets.rs).
    pub chains: crate::chain_presets::ChainLib,
    /// Chain card whose name field is open (Rename…, Save as…).
    pub renaming: Option<DeviceId>,
    pub expanded: HashSet<String>,
    pub search: String,
    pub selected: Option<String>,
    pub tree_scroll: f32,
    pub entries: Vec<TreeEntry>,
    pub add_search: String,
    pub add_results: Vec<Item>,
    pub menu: Option<DevMenu>,
    pub dnd: Option<Dnd>,
    pub card_drag: Option<CardDrag>,
    pub chain_scroll: f32,
    /// Last `plugin_load_errors()`, editors we opened that are still open, latencies.
    pub load_errors: Vec<(DeviceId, String)>,
    pub editors: HashSet<DeviceId>,
    /// Built-in devices shown in a floating device window.
    pub windows: HashSet<DeviceId>,
    /// Drag offset of each device window, kept while its device exists.
    pub window_at: HashMap<DeviceId, (f32, f32)>,
    /// Expanded device windows: section switches and In / Out meters (src/device_window.rs).
    pub(crate) wins: crate::device_window::WinUi,
    pub latency: HashMap<DeviceId, u32>,
    last_poll: Option<Instant>,
    /// After opening a project: report load errors that show up in this window.
    report_until: Option<Instant>,
    /// Window size and browser visibility, refreshed by `update_ui` (drop hit tests).
    pub win: (f32, f32),
    pub sounds: bool,
    /// Built-in device panels: live gesture and meter history (src/device_panel.rs).
    pub panel: crate::device_panel::PanelState,
    /// Modulation, chains and plugin cards (src/device_mod.rs).
    pub modu: crate::device_mod::ModUi,
    /// (id, folded, kind) of the cards last pushed: a change refocuses the window.
    pub card_layout: Vec<(i32, bool, i32)>,
}

impl Default for DevicesUi {
    fn default() -> Self {
        let expanded =
            ["i/builtin", "i/au", "i/vst3", "i/clap", "e/builtin", "e/au", "e/vst3", "e/clap"].iter().map(|s| s.to_string()).collect();
        DevicesUi {
            catalog: Catalog::default(),
            tab: 0,
            chains: Default::default(),
            renaming: None,
            expanded,
            search: String::new(),
            selected: None,
            tree_scroll: 0.0,
            entries: Vec::new(),
            add_search: String::new(),
            add_results: Vec::new(),
            menu: None,
            dnd: None,
            card_drag: None,
            chain_scroll: 0.0,
            load_errors: Vec::new(),
            editors: HashSet::new(),
            windows: HashSet::new(),
            window_at: HashMap::new(),
            wins: Default::default(),
            latency: HashMap::new(),
            last_poll: None,
            report_until: None,
            win: (1440.0, 820.0),
            sounds: true,
            panel: Default::default(),
            modu: Default::default(),
            card_layout: Vec::new(),
        }
    }
}

fn matches(q: &str, p: &PluginRef) -> bool {
    q.is_empty() || p.name.to_lowercase().contains(q) || p.vendor.to_lowercase().contains(q)
}

fn count_text(n: usize) -> String {
    n.to_string()
}

fn row(depth: i32, label: &str, count: String, icon: i32, expander: i32, leaf: bool) -> TreeRow {
    TreeRow { depth, label: label.into(), count: count.into(), icon, expander, selected: false, leaf }
}

/// Browser tree rows for a tab: (entries, rows). Pure, for tests.
pub fn build_tree(
    cat: &Catalog,
    chains: &crate::chain_presets::ChainLib,
    tab: usize,
    search: &str,
    expanded: &HashSet<String>,
) -> (Vec<TreeEntry>, Vec<TreeRow>) {
    let q = search.trim().to_lowercase();
    let mut entries = Vec::new();
    let mut rows = Vec::new();
    let mut push = |e: TreeEntry, r: TreeRow| {
        entries.push(e);
        rows.push(r);
    };
    if tab == 2 {
        // Saved chains first ("My chains"), then the factory folders. A search opens every
        // folder and keeps the chains whose name (or folder) matches.
        for folder in chains.folders() {
            let fit = |r: &crate::chain_presets::ChainRef| {
                q.is_empty() || r.name.to_lowercase().contains(&q) || folder.to_lowercase().contains(&q)
            };
            let list: Vec<_> = chains.in_folder(folder).into_iter().filter(fit).collect();
            if list.is_empty() {
                continue;
            }
            let key = format!("c/{folder}");
            let o = !q.is_empty() || expanded.contains(&key);
            push(
                TreeEntry { key: key.clone(), item: None, folder: true },
                row(0, folder, count_text(list.len()), 0, if o { 2 } else { 1 }, false),
            );
            if o {
                for r in list {
                    let label = r.name.clone();
                    push(
                        TreeEntry { key: format!("c/{folder}/{}", r.name), item: Some(Item::Chain(r)), folder: false },
                        row(1, &label, String::new(), 2, 0, true),
                    );
                }
            }
        }
        return (entries, rows);
    }
    let kind = if tab == 0 { PluginKind::Instrument } else { PluginKind::Effect };
    let tp = if tab == 0 { "i" } else { "e" };
    let open = |key: &str| !q.is_empty() || expanded.contains(key);
    let leaf_icon = if kind == PluginKind::Instrument { 1 } else { 2 };
    // Built-in: our devices of this kind (dstudio_core::builtin) and, for instruments, the Synth
    // (its own row, Item::BuiltinSynth).
    let builtins: Vec<&'static DeviceDef> = dstudio_core::builtin::all()
        .iter()
        .copied()
        .filter(|d| {
            d.kind == kind
                && dstudio_core::builtin::listed(d)
                && d.id != dstudio_core::builtin::synth_v2::SYNTH_V2.id
                && (q.is_empty() || d.name.to_lowercase().contains(&q) || d.category.to_lowercase().contains(&q))
        })
        .collect();
    if kind == PluginKind::Effect && !builtins.is_empty() {
        let key = format!("{tp}/builtin");
        let o = open(&key);
        push(
            TreeEntry { key: key.clone(), item: None, folder: true },
            row(0, "Built-in", count_text(builtins.len()), 0, if o { 2 } else { 1 }, false),
        );
        if o {
            for d in &builtins {
                push(
                    TreeEntry { key: format!("{tp}/b/{}", d.id), item: Some(Item::Builtin(d)), folder: false },
                    row(1, d.name, String::new(), leaf_icon, 0, true),
                );
            }
        }
    }
    // A search lists the Synth's sounds whose name matches (and the Synth for its own name).
    let sound = |name: &str| q.is_empty() || name.to_lowercase().contains(&q);
    let sounds = dstudio_core::builtin::synth_v2::SYNTH_V2.factory.iter().any(|f| !q.is_empty() && sound(f.name));
    let synth = q.is_empty() || "synth".contains(&q) || "built-in".contains(&q) || sounds;
    if kind == PluginKind::Instrument && (synth || !builtins.is_empty()) {
        let key = format!("{tp}/builtin");
        let o = open(&key);
        push(
            TreeEntry { key: key.clone(), item: None, folder: true },
            row(0, "Built-in", count_text(usize::from(synth) + builtins.len()), 0, if o { 2 } else { 1 }, false),
        );
        if o && synth {
            // The Synth opens into its sound categories, each into its factory sounds; the row
            // itself still adds the plain Synth when dragged.
            let def = &dstudio_core::builtin::synth_v2::SYNTH_V2;
            let skey = format!("{tp}/b/synth");
            let so = if q.is_empty() { expanded.contains(&skey) } else { sounds };
            push(
                TreeEntry { key: skey.clone(), item: Some(Item::BuiltinSynth(None)), folder: true },
                row(1, "Synth", count_text(def.factory.len()), 1, if so { 2 } else { 1 }, true),
            );
            if so {
                for (cat, names) in dstudio_core::builtin::synth_v2::CATEGORIES {
                    let names: Vec<&str> = names.iter().copied().filter(|n| sound(n)).collect();
                    if names.is_empty() {
                        continue;
                    }
                    let ckey = format!("{skey}/{cat}");
                    let co = open(&ckey);
                    push(
                        TreeEntry { key: ckey.clone(), item: None, folder: true },
                        row(2, cat, count_text(names.len()), 0, if co { 2 } else { 1 }, false),
                    );
                    if co {
                        for name in names {
                            let Some(i) = def.factory.iter().position(|f| f.name == name) else { continue };
                            push(
                                TreeEntry { key: format!("{ckey}/{name}"), item: Some(Item::BuiltinSynth(Some(i as u16))), folder: false },
                                row(3, name, String::new(), 3, 0, true),
                            );
                        }
                    }
                }
            }
        }
        if o {
            for d in &builtins {
                push(
                    TreeEntry { key: format!("{tp}/b/{}", d.id), item: Some(Item::Builtin(d)), folder: false },
                    row(1, d.name, String::new(), leaf_icon, 0, true),
                );
            }
        }
    }
    let all = cat.of_kind(kind);
    for (fmt, fkey, label) in
        [(PluginFormat::AudioUnit, "au", "Audio Units"), (PluginFormat::Vst3, "vst3", "VST3"), (PluginFormat::Clap, "clap", "CLAP")]
    {
        let ps: Vec<&&PluginInfo> = all.iter().filter(|p| p.plugin.format == fmt && matches(&q, &p.plugin)).collect();
        if ps.is_empty() && !q.is_empty() {
            continue;
        }
        let key = format!("{tp}/{fkey}");
        let o = open(&key) && !ps.is_empty();
        push(
            TreeEntry { key: key.clone(), item: None, folder: true },
            row(
                0,
                label,
                count_text(ps.len()),
                0,
                if ps.is_empty() {
                    0
                } else if o {
                    2
                } else {
                    1
                },
                false,
            ),
        );
        if !o {
            continue;
        }
        let leaf = |p: &PluginInfo, depth: i32| {
            (
                TreeEntry { key: format!("{tp}/p/{fkey}/{}", p.plugin.id), item: Some(Item::Plugin(p.plugin.clone())), folder: false },
                row(depth, &p.plugin.name, String::new(), leaf_icon, 0, true),
            )
        };
        if fmt == PluginFormat::AudioUnit {
            // Vendor subfolders.
            let mut i = 0;
            while i < ps.len() {
                let vendor = ps[i].plugin.vendor.clone();
                let j = ps[i..].iter().position(|p| p.plugin.vendor != vendor).map(|n| i + n).unwrap_or(ps.len());
                let vkey = format!("{key}/{vendor}");
                let vo = open(&vkey);
                let vlabel = if vendor.is_empty() { "Unknown vendor" } else { vendor.as_str() };
                push(
                    TreeEntry { key: vkey, item: None, folder: true },
                    row(1, vlabel, count_text(j - i), 0, if vo { 2 } else { 1 }, false),
                );
                if vo {
                    for p in &ps[i..j] {
                        let (e, r) = leaf(p, 2);
                        push(e, r);
                    }
                }
                i = j;
            }
        } else {
            for p in &ps {
                let (e, r) = leaf(p, 1);
                push(e, r);
            }
        }
    }
    (entries, rows)
}

fn fmt_latency(frames: u32, rate: f64) -> String {
    if frames == 0 {
        return String::new();
    }
    format!("Latency {frames} samples ({:.1} ms)", frames as f64 * 1000.0 / rate.max(1.0))
}

// ------------------------------------------------------------------ controller glue

impl Controller {
    // ---------------------------------------------------------------- scanning

    /// Start a catalogue scan on a worker thread (`rescan`: ignore the cache).
    pub fn start_plugin_scan(&mut self, rescan: bool) {
        if self.devices.catalog.state == ScanState::Scanning {
            self.set_status("A plugin scan is already running", 3);
            return;
        }
        self.devices.catalog.state = ScanState::Scanning;
        self.devices.catalog.scans += 1;
        self.devices.catalog.last_rescan = rescan;
        *self.devices.catalog.progress.lock().unwrap() = String::new();
        self.devices.catalog.shown_progress.clear();
        self.pending_jobs += 1;
        let tx = self.jobs_tx.clone();
        if let Some(report) = self.devices.catalog.fixture.clone() {
            let _ = tx.send(JobResult::PluginScan(Ok(report)));
        } else if let Some(cache) = self.devices.catalog.cache_path.clone() {
            let progress = self.devices.catalog.progress.clone();
            let folders: Vec<PathBuf> = self.settings.prefs.plugin_folders.iter().map(PathBuf::from).collect();
            std::thread::Builder::new()
                .name("plugin-scan".into())
                .spawn(move || {
                    let r = dstudio_engine::scan_plugins_in(&cache, rescan, &folders, &mut |s: &str| {
                        if let Ok(mut p) = progress.lock() {
                            *p = s.to_string();
                        }
                    });
                    let _ = tx.send(JobResult::PluginScan(r.map_err(|e| e.to_string())));
                })
                .expect("spawn plugin scan thread");
        } else {
            let _ = tx.send(JobResult::PluginScan(Err("no app support directory for the plugin cache".into())));
        }
        self.set_status(if rescan { "Rescanning plugins…" } else { "Scanning plugins…" }, 600);
    }

    /// Settings › Plugins › Add folder (the folder picked in the dialog): keeps it in the
    /// preferences and checks for new plugins, so what it holds shows up.
    pub fn plugins_add_folder(&mut self, dir: &Path) {
        match add_plugin_folder(&mut self.settings.prefs.plugin_folders, dir) {
            Ok(()) => {
                self.save_prefs();
                self.ui_dirty = true;
                self.check_plugins_again();
            }
            Err(e) => self.set_status(&e, 4),
        }
    }

    /// The remove button of a custom folder row: forgets it and checks again, which drops the
    /// plugins only it held.
    pub fn plugins_remove_folder(&mut self, dir: &str) {
        let before = self.settings.prefs.plugin_folders.len();
        self.settings.prefs.plugin_folders.retain(|f| f != dir);
        if self.settings.prefs.plugin_folders.len() != before {
            self.save_prefs();
            self.ui_dirty = true;
            self.check_plugins_again();
        }
    }

    /// "Check for new plugins" after the folder list changed; after the running scan, if any.
    fn check_plugins_again(&mut self) {
        if self.devices.catalog.state == ScanState::Scanning {
            self.devices.catalog.check_again = true;
        } else {
            self.start_plugin_scan(false);
        }
    }

    pub(crate) fn finish_plugin_scan(&mut self, r: Result<ScanReport, String>) {
        match r {
            Ok(mut rep) => {
                if self.devices.catalog.screenshot_apple_only {
                    rep.plugins.retain(|p| p.plugin.format == PluginFormat::AudioUnit && p.plugin.vendor == "Apple");
                    rep.failed.clear();
                }
                let n = rep.plugins.len();
                let nf = rep.failed.len();
                self.devices.catalog.plugins = rep.plugins;
                self.devices.catalog.failed = rep.failed;
                self.devices.catalog.from_cache = rep.from_cache;
                self.devices.catalog.state = ScanState::Done;
                let src = if self.devices.catalog.fixture.is_some() { " (TEST fixture)" } else { "" };
                self.set_status(
                    format!(
                        "Plugins: {n} found{}{src}",
                        if nf > 0 { format!(", {nf} failed (see Settings › Plugins)") } else { String::new() }
                    ),
                    4,
                );
            }
            Err(e) => {
                self.set_status(format!("Plugin scan failed: {e}"), 8);
                self.devices.catalog.state = ScanState::Failed(e);
            }
        }
        self.ui_dirty = true;
        if std::mem::take(&mut self.devices.catalog.check_again) {
            self.start_plugin_scan(false);
        }
    }

    /// Cheap periodic work (UI timer): scan progress, engine load errors,
    /// editor windows closed by the user, latency.
    pub fn device_poll(&mut self) {
        let now = Instant::now();
        self.panel_poll_meters();
        self.panel_poll_views();
        self.panel_poll_fields();
        self.mod_poll();
        self.panel_poll_graphs();
        // Main-thread plugin housekeeping (retired instances, CLAP main-thread callbacks),
        // which also attaches finished background warp renders.
        if !self.devices.editors.is_empty()
            || self
                .project()
                .tracks
                .iter()
                .any(|t| !t.devices.is_empty() || t.instrument_plugin.is_some() || t.clips.iter().any(|c| c.warp.enabled))
        {
            self.engine.plugin_idle();
        }
        if self.devices.last_poll.is_some_and(|t| now - t < Duration::from_millis(250)) {
            return;
        }
        self.devices.last_poll = Some(now);
        if self.devices.catalog.state == ScanState::Scanning {
            let p = self.devices.catalog.progress.lock().map(|p| p.clone()).unwrap_or_default();
            if p != self.devices.catalog.shown_progress {
                self.devices.catalog.shown_progress = p.clone();
                self.set_status(format!("Scanning plugins… {p}"), 600);
            }
        }
        let has_devices = self.project().tracks.iter().any(|t| !t.devices.is_empty() || t.instrument_plugin.is_some());
        if !has_devices && self.devices.editors.is_empty() && self.devices.load_errors.is_empty() {
            return;
        }
        let errors = self.engine.plugin_load_errors();
        if errors != self.devices.load_errors {
            if self.devices.report_until.is_some_and(|t| now < t) && !errors.is_empty() {
                self.report_load_errors(&errors);
            }
            self.devices.load_errors = errors;
            self.ui_dirty = true;
        }
        let closed: Vec<DeviceId> = self.devices.editors.iter().copied().filter(|id| !self.engine.plugin_editor_is_open(*id)).collect();
        if !closed.is_empty() {
            for id in closed {
                self.devices.editors.remove(&id);
            }
            self.ui_dirty = true;
        }
        let ids: Vec<DeviceId> =
            self.project().tracks.iter().flat_map(|t| t.instrument_plugin.iter().chain(t.devices.iter()).map(|d| d.id)).collect();
        for id in ids {
            let l = self.engine.plugin_latency(id);
            if self.devices.latency.get(&id).copied().unwrap_or(0) != l {
                self.devices.latency.insert(id, l);
                self.ui_dirty = true;
            }
        }
    }

    fn report_load_errors(&mut self, errors: &[(DeviceId, String)]) {
        let p = self.project();
        let parts: Vec<String> =
            errors.iter().map(|(id, why)| format!("{} ({why})", p.device(*id).map(|d| d.plugin.name.as_str()).unwrap_or("?"))).collect();
        let msg = format!("{} plugin(s) could not be loaded: {}", errors.len(), parts.join(", "));
        self.set_status(msg, 10);
    }

    /// After a project was opened and published: list plugin load errors.
    pub(crate) fn devices_after_open(&mut self) {
        self.devices.chain_scroll = 0.0;
        self.devices.latency.clear();
        let errors = self.engine.plugin_load_errors();
        if !errors.is_empty() {
            self.report_load_errors(&errors);
        }
        self.devices.load_errors = errors;
        // The engine may instantiate asynchronously: keep reporting for a moment.
        self.devices.report_until = Some(Instant::now() + Duration::from_secs(3));
    }

    /// Close every plugin window (before replacing the project).
    /// Project replaced: close plugin editors and forget device windows and their offsets
    /// (a new project can reuse device ids).
    pub(crate) fn close_all_editors(&mut self) {
        for id in std::mem::take(&mut self.devices.editors) {
            self.engine.close_plugin_editor(id);
        }
        self.devices.windows.clear();
        self.devices.window_at.clear();
    }

    /// After any document change: close windows of devices that no longer exist
    /// (undo of an add, removal by another path) and drop stale gesture state.
    pub(crate) fn devices_after_change(&mut self) {
        self.mod_after_change();
        let gone: Vec<DeviceId> = self.devices.editors.iter().copied().filter(|id| self.project().device(*id).is_none()).collect();
        for id in gone {
            self.engine.close_plugin_editor(id);
            self.devices.editors.remove(&id);
        }
        if self.devices.card_drag.is_some_and(|d| self.project().device(d.id).is_none()) {
            self.devices.card_drag = None;
        }
        self.clamp_chain_scroll();
    }

    // ---------------------------------------------------------------- adding

    /// Add `item` to `track`: an effect becomes an insert (`AddDevice`, at
    /// `index` or the end); an instrument replaces the track's instrument
    /// (`SetInstrumentPlugin`; the built-in Synth sets None). One undo step.
    /// A new instrument track playing `item`, added below the others in one undo step and
    /// named after the instrument.
    pub fn add_item_as_new_track(&mut self, item: &Item) -> bool {
        let device = match item {
            Item::BuiltinSynth(preset) => synth_with(*preset),
            Item::Plugin(p) if p.kind == PluginKind::Instrument => Device::new(p.clone()),
            Item::Builtin(d) if d.kind == PluginKind::Instrument => d.instantiate(),
            _ => return false,
        };
        let p = self.project();
        let id = TrackId(p.next_id);
        let track = dstudio_core::Track::new(id, TrackKind::Instrument, item.name(), crate::controller::next_track_color(p));
        let commands = vec![Command::AddTrack { index: None, track }, Command::SetInstrumentPlugin { track: id, device: Some(device) }];
        if !self.execute(Command::Batch { label: "Add Instrument Track".into(), commands }) {
            return false;
        }
        self.select_track(id);
        self.select_clip(None);
        self.set_bottom_tab(BottomTab::Devices);
        self.reveal_last_change(id, item);
        true
    }

    pub fn add_item_to_track(&mut self, item: &Item, track: TrackId, index: Option<usize>) -> bool {
        let Some(t) = self.project().track(track) else { return false };
        let (kind, has_synth) = (t.kind, t.instrument_plugin.as_ref().is_some_and(is_builtin_synth));
        let ok = match (item.kind(), item) {
            (PluginKind::Effect, Item::Chain(r)) => self.add_chain(r, track, index),
            (PluginKind::Effect, Item::Plugin(p)) => self.execute(Command::AddDevice { track, index, device: Device::new(p.clone()) }),
            (PluginKind::Effect, Item::Builtin(d)) => self.execute(Command::AddDevice { track, index, device: d.instantiate() }),
            (PluginKind::Instrument, _) if kind != TrackKind::Instrument => {
                self.set_status(format!("{} is an instrument: choose an instrument track", item.name()), 4);
                false
            }
            (PluginKind::Instrument, Item::BuiltinSynth(Some(i))) if has_synth => {
                // A factory sound on a track that plays the Synth: load it there.
                let def = &dstudio_core::builtin::synth_v2::SYNTH_V2;
                let device = self.project().track(track).and_then(|t| t.instrument_plugin.as_ref()).map(|d| d.id);
                match (device, def.factory.get(*i as usize)) {
                    (Some(device), Some(f)) => {
                        self.execute(Command::SetDeviceParams { device, params: def.factory_values(f), preset: Some(f.name.into()) })
                    }
                    _ => false,
                }
            }
            (PluginKind::Instrument, Item::BuiltinSynth(preset)) => {
                if has_synth {
                    self.set_status("The built-in Synth is already this track's instrument", 3);
                    false
                } else {
                    self.close_instrument_editor(track);
                    self.execute(Command::SetInstrumentPlugin { track, device: Some(synth_with(*preset)) })
                }
            }
            (PluginKind::Instrument, Item::Plugin(p)) => {
                self.close_instrument_editor(track);
                self.execute(Command::SetInstrumentPlugin { track, device: Some(Device::new(p.clone())) })
            }
            (PluginKind::Instrument, Item::Builtin(d)) => {
                self.close_instrument_editor(track);
                self.execute(Command::SetInstrumentPlugin { track, device: Some(d.instantiate()) })
            }
            (PluginKind::Effect, Item::BuiltinSynth(_)) | (PluginKind::Instrument, Item::Chain(_)) => false,
        };
        if ok {
            self.select_track(track);
            self.select_clip(None);
            self.set_bottom_tab(BottomTab::Devices);
            self.reveal_last_change(track, item);
            self.auto_show_window(item, track, index);
        }
        ok
    }

    /// Settings › General › Plugin windows: open a new effect's window (third-party: its
    /// own editor; built-in: its floating device window) when that switch is on.
    fn auto_show_window(&mut self, item: &Item, track: TrackId, index: Option<usize>) {
        let g = &self.settings.prefs.general;
        let show = match item {
            Item::Plugin(_) => g.show_plugin_windows && EDITORS_SUPPORTED && !self.settings.headless,
            Item::Builtin(_) => g.show_builtin_windows,
            Item::BuiltinSynth(_) | Item::Chain(_) => false,
        };
        if !show || item.kind() != PluginKind::Effect {
            return;
        }
        let Some(t) = self.project().track(track) else { return };
        let Some(id) = index.and_then(|i| t.devices.get(i)).or(t.devices.last()).map(|d| d.id) else { return };
        match item {
            Item::Plugin(_) if !self.engine.plugin_editor_is_open(id) => self.device_edit(id),
            Item::Builtin(_) => {
                self.devices.windows.insert(id);
            }
            _ => {}
        }
    }

    /// Scroll the chain so a newly added card is visible.
    fn reveal_last_change(&mut self, _track: TrackId, item: &Item) {
        if item.kind() != PluginKind::Effect {
            self.devices.chain_scroll = 0.0;
            return;
        }
        let (cards, _, _) = self.chain_layout_unscrolled();
        if let Some((x, c)) = cards.iter().rev().find(|(_, c)| c.name.as_str() == item.name()) {
            let view_w = self.chain_view_w();
            if *x + c.width > CHAIN_AREA_X + view_w + self.devices.chain_scroll {
                self.devices.chain_scroll = *x + c.width + CARD_GAP + ADD_W + 8.0 - CHAIN_AREA_X - view_w;
            }
        }
        self.clamp_chain_scroll();
    }

    fn close_instrument_editor(&mut self, track: TrackId) {
        if let Some(id) = self.project().track(track).and_then(|t| t.instrument_plugin.as_ref()).map(|d| d.id) {
            self.engine.close_plugin_editor(id);
            self.devices.editors.remove(&id);
        }
    }

    // ---------------------------------------------------------------- browser tree

    pub fn browser_tab(&mut self, i: usize) {
        self.browser_visit(crate::files::Loc { section: crate::files::SECTION_DEVICES, tab: i.min(2) });
    }

    /// The search field filters the open section.
    pub fn browser_search(&mut self, text: &str) {
        if self.files.section == crate::files::SECTION_FILES {
            self.files.search = text.to_string();
            self.files.scroll = 0.0;
        } else {
            self.devices.search = text.to_string();
            self.devices.tree_scroll = 0.0;
        }
        self.ui_dirty = true;
    }

    pub fn tree_click(&mut self, i: usize) {
        let Some(e) = self.devices.entries.get(i).cloned() else { return };
        if e.folder && self.devices.search.trim().is_empty() && !self.devices.expanded.remove(&e.key) {
            self.devices.expanded.insert(e.key.clone());
        }
        self.devices.selected = Some(e.key);
        self.ui_dirty = true;
    }

    /// Double-click: add the device to the selected track. An instrument replaces the selected
    /// instrument track's instrument; with no instrument track selected it gets a new track.
    pub fn tree_activate(&mut self, i: usize) {
        // A folder row (the Synth's) opens and closes on its clicks; dragging it adds it.
        let Some(item) = self.devices.entries.get(i).filter(|e| !e.folder).and_then(|e| e.item.clone()) else { return };
        let selected = self.selected_track.filter(|t| self.project().track(*t).is_some());
        if item.kind() == PluginKind::Instrument
            && !selected.and_then(|t| self.project().track(t)).is_some_and(|t| t.kind == TrackKind::Instrument)
        {
            self.add_item_as_new_track(&item);
            return;
        }
        let Some(t) = selected else {
            self.set_status("Select a track first", 3);
            return;
        };
        self.add_item_to_track(&item, t, None);
    }

    pub fn tree_scroll_by(&mut self, dy: f32) {
        let rows = self.devices.entries.len() as f32;
        let view = self.devices.win.1 - 88.0 - self.bottom_h() - 81.0 - 36.0;
        let max = (TREE_TOP + rows * TREE_ROW + 8.0 - view).max(0.0);
        self.devices.tree_scroll = (self.devices.tree_scroll - dy).clamp(0.0, max);
        self.ui_dirty = true;
    }

    pub fn layout(&self) -> Layout {
        let (w, h) = self.devices.win;
        Layout {
            w,
            h,
            sounds: self.devices.sounds,
            mixer: self.mixer_open,
            assistant: self.assistant_open,
            chat_w: self.chat_w_at(w, self.devices.sounds),
            clip_tab: self.bottom_tab == BottomTab::Clip,
            scroll_y: self.view.scroll_y as f32,
            bottom: self.bottom_h(),
        }
    }

    /// Browser row drag: phase 0 press, 1 move, 2 release (window coordinates).
    pub fn tree_drag(&mut self, i: usize, phase: i32, x: f32, y: f32) {
        match phase {
            0 => {
                self.devices.dnd = self.devices.entries.get(i).and_then(|e| e.item.clone()).map(|item| Dnd {
                    item,
                    start: (x, y),
                    pos: (x, y),
                    active: false,
                });
            }
            1 => {
                if let Some(d) = self.devices.dnd.as_mut() {
                    d.pos = (x, y);
                    if !d.active && ((x - d.start.0).abs() > DRAG_SLOP || (y - d.start.1).abs() > DRAG_SLOP) {
                        d.active = true;
                    }
                    if d.active {
                        self.ui_dirty = true;
                    }
                }
            }
            _ => {
                let Some(d) = self.devices.dnd.take() else { return };
                self.ui_dirty = true;
                if !d.active {
                    return;
                }
                match self.drop_target(x, y, &d.item) {
                    Some(DropTarget::Track(t)) => {
                        self.add_item_to_track(&d.item, t, None);
                    }
                    Some(DropTarget::NewTrack) => {
                        self.add_item_as_new_track(&d.item);
                    }
                    Some(DropTarget::Chain { track, index }) => {
                        let index = index.map(|i| self.chain_device_index(i, None));
                        self.add_item_to_track(&d.item, track, index);
                    }
                    None => {}
                }
            }
        }
    }

    /// Where a browser item dropped at window (x, y) goes.
    pub fn drop_target(&self, x: f32, y: f32, item: &Item) -> Option<DropTarget> {
        let l = self.layout();
        let (tx, tw) = (l.timeline_x(), l.timeline_w());
        let (top, bottom) = (78.0 + 66.5, 78.0 + l.top_h() - crate::timeline::MASTER_H);
        if x >= tx && x < tx + tw && y >= top && y < bottom {
            return match self.view.track_at(y - top + crate::timeline::RULER_H).and_then(|i| self.project().tracks.get(i)) {
                Some(t) => Some(DropTarget::Track(t.id)),
                None if item.kind() == PluginKind::Instrument && y >= self.lanes_end_y() => Some(DropTarget::NewTrack),
                None => None,
            };
        }
        if !l.mixer && !l.clip_tab {
            let by = l.bottom_y();
            if x >= PANEL_X + CHAIN_AREA_X && x < l.w - 5.0 && y >= by + 27.0 && y < by + l.bottom_h() {
                let track = self.selected_track?;
                let index = if item.kind() == PluginKind::Effect { Some(self.chain_insert_index(x - PANEL_X, None)) } else { None };
                return Some(DropTarget::Chain { track, index });
            }
        }
        None
    }

    /// Window y where the tracks end (the top of the empty area below them).
    pub(crate) fn lanes_end_y(&self) -> f32 {
        let l = self.layout();
        let n = self.project().tracks.len();
        let end = if n == 0 { 0.0 } else { self.view.track_offset(n - 1) + self.view.pitch(n - 1) };
        78.0 + 66.5 + end - l.scroll_y
    }

    /// The empty area below the tracks, where a drop adds a new track.
    pub(crate) fn new_track_rect(&self) -> Option<(f32, f32, f32, f32)> {
        let l = self.layout();
        let (top, bottom) = (78.0 + 66.5, 78.0 + l.top_h() - crate::timeline::MASTER_H);
        let y0 = self.lanes_end_y().max(top);
        (bottom - y0 > 2.0).then_some((l.timeline_x(), y0, l.timeline_w(), bottom - y0))
    }

    pub(crate) fn track_rect(&self, t: TrackId) -> Option<(f32, f32, f32, f32)> {
        let l = self.layout();
        let i = self.project().track_index(t)?;
        let y = 78.0 + 66.5 + self.view.track_offset(i) - l.scroll_y;
        let (top, bottom) = (78.0 + 66.5, 78.0 + l.top_h() - crate::timeline::MASTER_H);
        let (y0, y1) = (y.max(top), (y + self.view.pitch(i) - 0.5).min(bottom));
        (y1 > y0).then_some((l.timeline_x(), y0, l.timeline_w(), y1 - y0))
    }

    // ---------------------------------------------------------------- chain

    pub(crate) fn chain_view_w(&self) -> f32 {
        (self.devices.win.0 - 2.0 * PANEL_X - CHAIN_AREA_X).max(100.0)
    }

    /// Cards of the selected track WITHOUT scroll or drag offsets:
    /// ((x, card), add_x, synth_visible). x is bottom-panel relative.
    pub(crate) fn chain_layout_unscrolled(&self) -> (Vec<(f32, DeviceCard)>, f32, bool) {
        let synth = self.synth_track().is_some();
        let mut out = Vec::new();
        let Some(t) = self.selected_track.and_then(|t| self.project().track(t)) else { return (out, ADD_X_ALONE, synth) };
        let synth_folded = synth && t.instrument_plugin.as_ref().is_some_and(|d| d.folded);
        let mut x = CHAIN_X0
            + if !synth {
                0.0
            } else if synth_folded {
                crate::device_mod::FOLDED_W + CARD_GAP
            } else {
                SYNTH_W + CARD_GAP
            };
        // The built-in Synth is drawn by its own card (src/synth.rs).
        let inst = if t.kind == TrackKind::Instrument { t.instrument_plugin.as_ref().filter(|d| !is_builtin_synth(d)) } else { None };
        let inserts = t.devices.iter().enumerate().map(|(i, d)| (Some(i), d, false));
        for (i, d, instrument) in inst.into_iter().map(|d| (None, d, true)).chain(inserts) {
            // A collapsed chain hides its members; an empty one leaves room for "Add effect".
            if i.and_then(|i| dstudio_core::modulation::head_of(&t.devices, i))
                .is_some_and(|h| t.devices[h].collapsed || t.devices[h].folded)
            {
                continue;
            }
            let card = self.card_for(d, instrument);
            x += card.width + CARD_GAP;
            out.push((x - card.width - CARD_GAP, card));
            if let Some(i) = i
                && dstudio_core::modulation::is_rack(d)
                && !d.collapsed
                && !d.folded
                && dstudio_core::modulation::members(&t.devices, i).is_empty()
            {
                x += crate::device_mod::EMPTY_CHAIN_W + CARD_GAP;
            }
        }
        let add_x = if out.is_empty() && !synth { ADD_X_ALONE } else { x };
        (out, add_x, synth)
    }

    /// The view model of one device card in the chain: name, format, kind,
    /// load error, latency and editor state (`editor_supported` is false where
    /// the engine cannot open plugin windows).
    fn card_for(&self, d: &Device, instrument: bool) -> DeviceCard {
        let mut card = self.card_body(d, instrument);
        self.card_common(d, &mut card);
        if card.builtin {
            card.editor_open = self.devices.windows.contains(&d.id);
            // Built-ins: only devices with an expanded window show the window button.
            card.has_editor = dstudio_core::builtin::def_of(d).is_some_and(crate::device_window::has_window);
        }
        card.accent_light = crate::device_mod::is_light(card.accent);
        card.accent_text = crate::device_mod::accent_text(card.accent);
        card
    }

    /// Card of a device window: the card, with the expanded view when the device has one.
    fn window_card(&self, d: &Device) -> DeviceCard {
        let mut card = self.card_for(d, false);
        if let Some(def) = dstudio_core::builtin::def_of(d)
            && let Some(win) = self.window_view(d, def, card.accent)
        {
            card.win = win;
        }
        card
    }

    fn card_body(&self, d: &Device, instrument: bool) -> DeviceCard {
        if let Some(def) = dstudio_core::builtin::def_of(d) {
            return self.builtin_card(d, def);
        }
        let info = self.devices.catalog.find(&d.plugin);
        let error = self.devices.load_errors.iter().find(|(id, _)| *id == d.id).map(|(_, e)| e.clone()).unwrap_or_default();
        let mut card = DeviceCard {
            id: d.id.0 as i32,
            x: 0.0,
            name: d.plugin.name.clone().into(),
            vendor: if d.plugin.vendor.is_empty() { "Unknown vendor".into() } else { d.plugin.vendor.clone().into() },
            format: format_badge(d.plugin.format).into(),
            kind: if instrument { "Instrument".into() } else { "Audio effect".into() },
            enabled: self.device_on_now(d),
            instrument,
            error: error.into(),
            latency: fmt_latency(self.devices.latency.get(&d.id).copied().unwrap_or(0), self.rate).into(),
            has_editor: info.map(|i| i.has_editor).unwrap_or(true),
            editor_supported: EDITORS_SUPPORTED,
            editor_open: self.devices.editors.contains(&d.id),
            dragging: false,
            width: CARD_W,
            ..Default::default()
        };
        self.plugin_body(d, &mut card);
        card
    }

    /// Insert index in the selected track's chain for a card centre at bottom-panel x
    /// (`moving`: the card being dragged, left out of the count).
    fn chain_insert_index(&self, centre: f32, moving: Option<DeviceId>) -> usize {
        let (cards, _, _) = self.chain_layout_unscrolled();
        let cx = centre + self.devices.chain_scroll;
        cards.iter().filter(|(_, c)| !c.instrument && Some(DeviceId(c.id as u64)) != moving).filter(|(x, c)| x + c.width / 2.0 < cx).count()
    }

    /// Device position (in the chain without `moving`) of insert index
    /// `visible`: before the card the marker stands at, so the hidden members
    /// of a collapsed or folded chain stay with their head.
    fn chain_device_index(&self, visible: usize, moving: Option<DeviceId>) -> usize {
        let (cards, _, _) = self.chain_layout_unscrolled();
        let Some(t) = self.selected_track.and_then(|t| self.project().track(t)) else { return visible };
        let order: Vec<DeviceId> = t.devices.iter().map(|d| d.id).filter(|id| Some(*id) != moving).collect();
        let anchor = cards
            .iter()
            .map(|(_, c)| c)
            .filter(|c| !c.instrument && Some(DeviceId(c.id as u64)) != moving)
            .nth(visible)
            .map(|c| DeviceId(c.id as u64));
        anchor.and_then(|a| order.iter().position(|x| *x == a)).unwrap_or(order.len())
    }

    /// Marker x (bottom-panel relative, scrolled) for an insert index.
    fn chain_marker_x(&self, index: usize, moving: Option<DeviceId>) -> f32 {
        let (cards, add_x, synth) = self.chain_layout_unscrolled();
        let inserts: Vec<(f32, f32)> =
            cards.iter().filter(|(_, c)| !c.instrument && Some(DeviceId(c.id as u64)) != moving).map(|(x, c)| (*x, c.width)).collect();
        let first = cards.iter().find(|(_, c)| !c.instrument).map(|(x, _)| *x).unwrap_or(add_x);
        let x = match inserts.get(index) {
            Some((x, _)) => x - CARD_GAP / 2.0,
            None => inserts.last().map(|(x, w)| x + w + CARD_GAP / 2.0).unwrap_or(if synth || !cards.is_empty() {
                first - CARD_GAP / 2.0
            } else {
                first - 4.0
            }),
        };
        x - self.devices.chain_scroll
    }

    pub fn chain_scroll_by(&mut self, d: f32) {
        self.devices.chain_scroll -= d;
        self.clamp_chain_scroll();
        self.ui_dirty = true;
    }

    pub(crate) fn clamp_chain_scroll(&mut self) {
        let (_, add_x, _) = self.chain_layout_unscrolled();
        let content = add_x + ADD_W + 8.0 - CHAIN_AREA_X;
        let max = (content - self.chain_view_w()).max(0.0);
        self.devices.chain_scroll = self.devices.chain_scroll.clamp(0.0, max);
    }

    pub fn device_power(&mut self, id: DeviceId) {
        if let Some(d) = self.project().device(id) {
            let enabled = !d.enabled;
            if let Some((t, target)) = self.device_on_target(id)
                && self.auto_switch(Some(t), target, d.enabled)
            {
                return;
            }
            self.execute(Command::SetDeviceEnabled { device: id, enabled });
        }
    }

    /// Whether any plugin window is open.
    pub fn has_open_editors(&self) -> bool {
        !self.devices.editors.is_empty()
    }

    /// Open or close the plugin's own window (main thread).
    pub fn device_edit(&mut self, id: DeviceId) {
        if self.project().device(id).is_none() {
            return;
        }
        if self.engine.plugin_editor_is_open(id) {
            self.engine.close_plugin_editor(id);
            self.devices.editors.remove(&id);
        } else {
            match self.engine.open_plugin_editor(id) {
                Ok(()) => {
                    self.devices.editors.insert(id);
                    set_plugin_windows_on_top(self.settings.prefs.general.plugin_windows_on_top);
                }
                Err(e) => self.set_status(format!("Plugin window: {e}"), 5),
            }
        }
        self.ui_dirty = true;
    }

    /// Remove a device (its window is closed first).
    pub fn device_remove(&mut self, id: DeviceId) {
        if self.project().device(id).is_none() {
            return;
        }
        if self.remove_with_members(id) {
            return;
        }
        self.engine.close_plugin_editor(id);
        self.devices.editors.remove(&id);
        self.devices.windows.remove(&id);
        self.execute(Command::RemoveDevice { device: id });
    }

    /// Instrument card "Use built-in Synth".
    pub fn device_use_synth(&mut self) {
        let Some(t) = self.selected_track else { return };
        if self.project().track(t).is_some_and(|t| !t.instrument_plugin.as_ref().is_some_and(is_builtin_synth)) {
            self.add_item_to_track(&Item::BuiltinSynth(None), t, None);
        }
    }

    /// Card header drag: phase 0 press, 1 move, 2 release; window x.
    pub fn device_drag(&mut self, id: DeviceId, phase: i32, x: f32) {
        let is_insert = self.project().find_device(id).is_some_and(|(_, s)| matches!(s, dstudio_core::model::DeviceSlot::Insert(_)));
        match phase {
            0 => self.devices.card_drag = is_insert.then_some(CardDrag { id, start_x: x, cur_x: x, active: false }),
            1 => {
                if let Some(d) = self.devices.card_drag.as_mut() {
                    d.cur_x = x;
                    if (x - d.start_x).abs() > DRAG_SLOP {
                        d.active = true;
                    }
                    self.ui_dirty = true;
                }
            }
            _ => {
                let Some(d) = self.devices.card_drag.take() else { return };
                self.ui_dirty = true;
                if !d.active {
                    return;
                }
                let Some((ti, dstudio_core::model::DeviceSlot::Insert(from))) = self.project().find_device(d.id) else { return };
                let track = self.project().tracks[ti].id;
                let (cards, _, _) = self.chain_layout_unscrolled();
                let Some((x0, c)) = cards.iter().find(|(_, c)| c.id as u64 == d.id.0) else { return };
                let centre = x0 - self.devices.chain_scroll + (d.cur_x - d.start_x) + c.width / 2.0;
                let index = self.chain_device_index(self.chain_insert_index(centre, Some(d.id)), Some(d.id));
                self.move_in_chain(d.id, track, from, index);
            }
        }
    }

    // ---------------------------------------------------------------- Add device card + menus

    /// Items offered for the selected track: effects always, instruments on
    /// instrument tracks.
    fn offered(&self, fmt: Option<PluginFormat>, q: &str) -> Vec<Item> {
        let inst_track = self.selected_track.and_then(|t| self.project().track(t)).is_some_and(|t| t.kind == TrackKind::Instrument);
        let q = q.trim().to_lowercase();
        let mut v = Vec::new();
        if inst_track && fmt.is_none() && (q.is_empty() || "synth".contains(&q) || "built-in".contains(&q)) {
            v.push(Item::BuiltinSynth(None));
        }
        let kinds: &[PluginKind] = if inst_track { &[PluginKind::Instrument, PluginKind::Effect] } else { &[PluginKind::Effect] };
        if fmt.is_none() {
            for d in dstudio_core::builtin::all()
                .iter()
                .filter(|d| dstudio_core::builtin::listed(d) && d.id != dstudio_core::builtin::synth_v2::SYNTH_V2.id)
            {
                if kinds.contains(&d.kind)
                    && (q.is_empty()
                        || d.name.to_lowercase().contains(&q)
                        || d.category.to_lowercase().contains(&q)
                        || "built-in".contains(&q))
                {
                    v.push(Item::Builtin(d));
                }
            }
        }
        for k in kinds {
            for p in self.devices.catalog.of_kind(*k) {
                if fmt.is_none_or(|f| f == p.plugin.format) && matches(&q, &p.plugin) {
                    v.push(Item::Plugin(p.plugin.clone()));
                }
            }
        }
        v
    }

    /// "Add device" row: 0 Built-in, 1 Audio Units, 2 VST3, 3 CLAP, 4 Chains.
    pub fn add_category(&mut self, row: usize, x: f32, y: f32) {
        if self.selected_track.is_none() {
            self.set_status("Select a track first", 3);
            return;
        }
        let (label, items): (&str, Vec<Item>) = match row {
            0 => (
                "built-in devices",
                self.offered(None, "").into_iter().filter(|i| matches!(i, Item::BuiltinSynth(_) | Item::Builtin(_))).collect(),
            ),
            1 => ("Audio Units", self.offered(Some(PluginFormat::AudioUnit), "")),
            2 => ("VST3 plugins", self.offered(Some(PluginFormat::Vst3), "")),
            3 => ("CLAP plugins", self.offered(Some(PluginFormat::Clap), "")),
            _ => return self.chain_folders_menu(None, x, y),
        };
        let mut rows = Vec::new();
        let mut kind = None;
        let both = items.iter().any(|i| i.kind() == PluginKind::Instrument) && items.iter().any(|i| i.kind() == PluginKind::Effect);
        for it in items {
            if both && kind != Some(it.kind()) {
                kind = Some(it.kind());
                let sep = !rows.is_empty();
                rows.push((
                    if it.kind() == PluginKind::Instrument { "Instruments" } else { "Audio effects" }.to_string(),
                    String::new(),
                    None,
                    sep,
                ));
            }
            let right = match &it {
                Item::BuiltinSynth(_) => "Built-in".to_string(),
                Item::Builtin(d) => d.category.to_string(),
                Item::Plugin(p) => p.vendor.clone(),
                Item::Chain(_) => "Chain".to_string(),
            };
            rows.push((it.name().to_string(), right, Some(MenuPick::Add(it)), false));
        }
        if rows.is_empty() {
            let note = match &self.devices.catalog.state {
                ScanState::Scanning => "Scanning plugins…".to_string(),
                ScanState::Failed(_) => "Plugin scan unavailable".to_string(),
                _ if row == 0 => "No built-in effects yet".to_string(),
                _ => format!("No {label} found"),
            };
            rows.push((note, String::new(), None, false));
        }
        self.devices.menu = Some(DevMenu { pos: (x, y), w: 220.0, rows });
        self.ui_dirty = true;
    }

    /// Instrument selector (inspector "Inst", Synth header ▾): Synth + instrument plugins.
    pub fn open_inst_menu(&mut self, x: f32, y: f32) {
        let Some(t) = self.selected_track.and_then(|t| self.project().track(t)).filter(|t| t.kind == TrackKind::Instrument) else { return };
        let cur = t.instrument_plugin.as_ref().filter(|d| !is_builtin_synth(d)).map(|d| (d.plugin.format, d.plugin.id.clone()));
        let check = |on: bool, s: &str| if on { "✓".to_string() } else { s.to_string() };
        let mut rows = vec![("Synth".to_string(), check(cur.is_none(), "Built-in"), Some(MenuPick::Add(Item::BuiltinSynth(None))), false)];
        for d in dstudio_core::builtin::all()
            .iter()
            .filter(|d| d.kind == PluginKind::Instrument && d.id != dstudio_core::builtin::synth_v2::SYNTH_V2.id)
        {
            let on = cur.as_ref().is_some_and(|(f, id)| *f == PluginFormat::Builtin && id == d.id);
            rows.push((d.name.to_string(), check(on, "Built-in"), Some(MenuPick::Add(Item::Builtin(d))), false));
        }
        let plugins = self.devices.catalog.of_kind(PluginKind::Instrument);
        if !plugins.is_empty() {
            rows.push(("Instrument plugins".to_string(), String::new(), None, true));
        }
        for p in plugins {
            let on = cur.as_ref().is_some_and(|(f, id)| *f == p.plugin.format && *id == p.plugin.id);
            rows.push((
                p.plugin.name.clone(),
                check(on, format_badge(p.plugin.format)),
                Some(MenuPick::Add(Item::Plugin(p.plugin.clone()))),
                false,
            ));
        }
        if self.devices.catalog.state == ScanState::Scanning {
            rows.push(("Scanning plugins…".to_string(), String::new(), None, true));
        }
        self.devices.menu = Some(DevMenu { pos: (x, y), w: 200.0, rows });
        self.ui_dirty = true;
    }

    pub fn devmenu_pick(&mut self, i: usize) {
        let Some(m) = self.devices.menu.take() else { return };
        self.ui_dirty = true;
        let item = match m.rows.get(i).and_then(|r| r.2.clone()) {
            Some(MenuPick::Add(item)) => item,
            Some(MenuPick::Sidechain(device, source)) => return self.panel_set_sidechain(device, source),
            Some(MenuPick::Preset(device, n)) => return self.panel_load_preset(device, n),
            Some(MenuPick::Choice(device, index, choice)) => return self.panel_choice(device, index, choice),
            Some(MenuPick::View(device, code)) => return self.editor_pick(device, code),
            Some(MenuPick::Window(device, action, value)) => return self.win_action(device, action, value),
            Some(MenuPick::Param(device, param, v)) => return self.panel_pick_param(device, param, v),
            Some(MenuPick::Mode(device, _, mode)) => return self.panel_pick_mode(device, mode),
            Some(MenuPick::Automate(device, p)) => return self.mod_automate(device, p),
            Some(MenuPick::AutomateOn(device)) => return self.mod_automate_on(device),
            Some(MenuPick::AutomateTrack(key, target)) => return self.auto_choose(key, target),
            Some(MenuPick::NewLfo(device, p)) => return self.mod_new_lfo(device, p),
            Some(MenuPick::Link(source, slot, device, p)) => {
                self.mod_link(source, slot, device, p);
                return;
            }
            Some(MenuPick::Unlink(source, i)) => return self.mod_remove(source, i),
            Some(MenuPick::Swap(device, item)) => return self.device_swap(device, item),
            Some(MenuPick::AddLfo(device)) => {
                self.add_lfo_near(device);
                return;
            }
            Some(MenuPick::AddToChain(head, item)) => return self.add_to_chain(head, item),
            Some(MenuPick::CardParam(device, id)) => return self.card_toggle_param(device, id),
            Some(
                p @ (MenuPick::ChainFolders(_)
                | MenuPick::ChainFolder(..)
                | MenuPick::ChainReplace(..)
                | MenuPick::ChainSave(_)
                | MenuPick::ChainSaveAs(_)
                | MenuPick::ChainRename(_)
                | MenuPick::ChainUnpack(_)
                | MenuPick::ChainDelete(_)),
            ) => return self.chain_pick(p, m.pos),
            None => return,
        };
        let Some(t) = self.selected_track else { return };
        // Picking the current instrument again is a no-op (no undo step).
        let cur_inst =
            self.project().track(t).and_then(|t| t.instrument_plugin.as_ref()).filter(|d| !is_builtin_synth(d)).map(|d| d.plugin.clone());
        let same = match (&item, &cur_inst) {
            (Item::BuiltinSynth(_), None) => {
                item.kind() == PluginKind::Instrument && self.project().track(t).is_some_and(|t| t.kind == TrackKind::Instrument)
            }
            (Item::Plugin(p), Some(c)) => p.kind == PluginKind::Instrument && p.format == c.format && p.id == c.id,
            (Item::Builtin(d), Some(c)) => d.kind == PluginKind::Instrument && c.format == PluginFormat::Builtin && c.id == d.id,
            _ => false,
        };
        if !same {
            self.add_item_to_track(&item, t, None);
        }
    }

    pub fn devmenu_dismiss(&mut self) {
        self.devices.menu = None;
        self.ui_dirty = true;
    }

    pub fn add_search(&mut self, text: &str) {
        self.devices.add_search = text.to_string();
        self.ui_dirty = true;
    }

    pub fn add_result_pick(&mut self, i: usize) {
        let Some(item) = self.devices.add_results.get(i).cloned() else { return };
        let Some(t) = self.selected_track else { return };
        self.add_item_to_track(&item, t, None);
    }

    // ---------------------------------------------------------------- view models

    /// Pushes the device browser, the drag-and-drop ghost, the device chain
    /// and Settings › Plugins (scan status, the default and configured scan
    /// folders, the found plugins) to the UI.
    pub(crate) fn push_devices(&mut self, s: &Studio) {
        // ---- browser tree ----
        let (entries, mut rows) =
            build_tree(&self.devices.catalog, &self.devices.chains, self.devices.tab, &self.devices.search, &self.devices.expanded);
        for (e, r) in entries.iter().zip(rows.iter_mut()) {
            r.selected = self.devices.selected.as_deref() == Some(e.key.as_str());
        }
        let empty = if !rows.is_empty() {
            String::new()
        } else if !self.devices.search.trim().is_empty() {
            format!("No devices match “{}”", self.devices.search.trim())
        } else {
            String::new()
        };
        self.devices.entries = entries;
        sync_model(&self.tree_model, rows);
        s.set_browser_tab(self.devices.tab as i32);
        if self.files.section == crate::files::SECTION_DEVICES && s.get_browser_search().as_str() != self.devices.search {
            s.set_browser_search(self.devices.search.clone().into());
        }
        s.set_tree_scroll(self.devices.tree_scroll);
        s.set_tree_empty(empty.into());

        // ---- drag and drop ghost + target ----
        match self.devices.dnd.clone().filter(|d| d.active) {
            Some(d) => {
                s.set_dnd_active(true);
                s.set_dnd_clip_visible(false);
                s.set_dnd_x(d.pos.0);
                s.set_dnd_y(d.pos.1);
                let target = self.drop_target(d.pos.0, d.pos.1, &d.item);
                let ok_kind = |t: TrackId| {
                    d.item.kind() == PluginKind::Effect || self.project().track(t).is_some_and(|t| t.kind == TrackKind::Instrument)
                };
                let (ok, rect, marker) = match target {
                    Some(DropTarget::Track(t)) => (ok_kind(t), self.track_rect(t), None),
                    Some(DropTarget::NewTrack) => (true, self.new_track_rect(), None),
                    Some(DropTarget::Chain { track, index }) => {
                        let l = self.layout();
                        let r = (PANEL_X + CHAIN_AREA_X, l.bottom_y() + 27.0, l.w - 2.0 * PANEL_X - CHAIN_AREA_X, l.bottom_h() - 27.0);
                        (ok_kind(track), if index.is_none() { Some(r) } else { None }, index.map(|i| self.chain_marker_x(i, None)))
                    }
                    None => (false, None, None),
                };
                s.set_dnd_ok(ok);
                s.set_dnd_label(d.item.name().into());
                s.set_dnd_target_visible(rect.is_some() && ok);
                if let Some((x, y, w, h)) = rect {
                    s.set_dnd_tx(x);
                    s.set_dnd_ty(y);
                    s.set_dnd_tw(w);
                    s.set_dnd_th(h);
                }
                s.set_chain_marker_visible(marker.is_some() && ok);
                if let Some(x) = marker {
                    s.set_chain_marker_x(x);
                }
            }
            None => {
                s.set_dnd_active(false);
                s.set_dnd_target_visible(false);
                s.set_dnd_clip_visible(false);
                s.set_chain_marker_visible(false);
            }
        }

        // ---- device chain ----
        self.clamp_chain_scroll();
        let scroll = self.devices.chain_scroll;
        let (cards, add_x, _) = self.chain_layout_unscrolled();
        self.devmm_sync_chain(&cards, add_x);
        let frames: Vec<crate::ChainFrame> = self
            .chain_frames(&cards)
            .into_iter()
            .map(|mut f| {
                f.x -= scroll;
                f.empty_x -= scroll;
                f
            })
            .collect();
        s.set_chain_frames(slint::ModelRc::from(std::rc::Rc::new(slint::VecModel::from(frames))));
        let layout: Vec<(i32, bool, i32)> = cards.iter().map(|(_, c)| (c.id, c.folded, c.modv.kind)).collect();
        if layout != self.devices.card_layout {
            self.devices.card_layout = layout;
            s.set_cards_epoch(s.get_cards_epoch().wrapping_add(1));
        }
        let drag = self.devices.card_drag.filter(|d| d.active);
        let cards: Vec<DeviceCard> = cards
            .into_iter()
            .map(|(x, mut c)| {
                c.x = x - scroll;
                if let Some(d) = drag
                    && d.id.0 == c.id as u64
                {
                    c.x += d.cur_x - d.start_x;
                    c.dragging = true;
                }
                c
            })
            .collect();
        // Draw the dragged card last (on top).
        let mut cards = cards;
        cards.sort_by_key(|c| c.dragging);
        for (i, c) in cards.iter_mut().enumerate() {
            if let Some(old) = slint::Model::row_data(&*self.cards_model, i) {
                crate::device_panel::reuse_card_models(&old, c);
            }
        }
        sync_model(&self.cards_model, cards);
        {
            let fields = &self.devices.panel.fields;
            let mut washes = fields.washes.borrow_mut();
            let n = washes.len();
            washes.retain(|(id, window), _| self.project().device(*id).is_some() && (!*window || self.devices.windows.contains(id)));
            if fields.washes_dirty.replace(false) || washes.len() != n {
                let rows: Vec<crate::PanelWash> = washes
                    .iter()
                    .map(|((id, window), w)| crate::PanelWash {
                        device: id.0 as i32,
                        window: *window,
                        img: w.img.clone(),
                        x: w.rect[0],
                        y: w.rect[1],
                        w: w.rect[2],
                        h: w.rect[3],
                    })
                    .collect();
                s.set_field_washes(slint::ModelRc::from(std::rc::Rc::new(slint::VecModel::from(rows))));
            }
        }
        // Device windows: built-in cards opened from their header stay up across track changes
        // (as plugin windows do) and close with their device.
        let gone: Vec<DeviceId> = self.devices.windows.iter().copied().filter(|id| self.project().device(*id).is_none()).collect();
        for id in gone {
            self.devices.windows.remove(&id);
            self.devices.window_at.remove(&id);
        }
        let mut ids: Vec<DeviceId> = self.devices.windows.iter().copied().collect();
        ids.sort_by_key(|id| id.0);
        let mut wins: Vec<DeviceCard> =
            ids.iter().filter_map(|id| self.project().device(*id).cloned()).map(|d| self.window_card(&d)).collect();
        for (i, c) in wins.iter_mut().enumerate() {
            (c.win_dx, c.win_dy) = self.devices.window_at.get(&DeviceId(c.id as u64)).copied().unwrap_or_default();
            if let Some(old) = slint::Model::row_data(&*self.window_cards_model, i) {
                crate::device_panel::reuse_card_models(&old, c);
            }
        }
        sync_model(&self.window_cards_model, wins);
        s.set_chain_scroll(scroll);
        s.set_add_device_x(add_x - scroll);
        if let Some(d) = drag {
            let (cards, _, _) = self.chain_layout_unscrolled();
            if let Some((x0, c)) = cards.iter().find(|(_, c)| c.id as u64 == d.id.0) {
                let centre = x0 - scroll + (d.cur_x - d.start_x) + c.width / 2.0;
                let idx = self.chain_insert_index(centre, Some(d.id));
                s.set_chain_marker_visible(true);
                s.set_chain_marker_x(self.chain_marker_x(idx, Some(d.id)));
            }
        }
        // Inspector instrument name.
        let sel = self.selected_track.and_then(|t| self.project().track(t));
        let inst = sel
            .and_then(|t| t.instrument_plugin.as_ref())
            .filter(|d| !is_builtin_synth(d))
            .map(|d| d.plugin.name.clone())
            .unwrap_or_else(|| "Synth".into());
        s.set_sel_inst_text(inst.into());

        // ---- Add device search ----
        if s.get_add_search().as_str() != self.devices.add_search {
            s.set_add_search(self.devices.add_search.clone().into());
        }
        self.devices.add_results =
            if self.devices.add_search.trim().is_empty() { Vec::new() } else { self.offered(None, &self.devices.add_search.clone()) };
        let mut results: Vec<MenuItem> = self
            .devices
            .add_results
            .iter()
            .take(9)
            .enumerate()
            .map(|(i, it)| MenuItem {
                swatch: Default::default(),
                label: it.name().into(),
                shortcut: match it {
                    Item::BuiltinSynth(_) | Item::Builtin(_) => "Built-in".into(),
                    Item::Plugin(p) => format_badge(p.format).into(),
                    Item::Chain(_) => "Chain".into(),
                },
                enabled: true,
                separator: false,
                submenu: false,
                y: i as f32 * 22.0,
            })
            .collect();
        if results.is_empty() && !self.devices.add_search.trim().is_empty() {
            results.push(MenuItem {
                swatch: Default::default(),
                label: "No matches".into(),
                shortcut: "".into(),
                enabled: false,
                separator: false,
                submenu: false,
                y: 0.0,
            });
        }
        sync_model(&self.add_results_model, results);

        // ---- popup ----
        match &self.devices.menu {
            Some(m) => {
                let items: Vec<MenuItem> = m
                    .rows
                    .iter()
                    .enumerate()
                    .map(|(i, (label, right, item, sep))| MenuItem {
                        swatch: Default::default(),
                        label: label.clone().into(),
                        shortcut: right.clone().into(),
                        enabled: item.is_some(),
                        separator: *sep,
                        submenu: false,
                        y: MENU_PAD + i as f32 * MENU_ROW,
                    })
                    .collect();
                let content = MENU_PAD * 2.0 + items.len() as f32 * MENU_ROW;
                s.set_devmenu_content_h(content);
                s.set_devmenu_h(content.min(self.devices.win.1 - 10.0));
                s.set_devmenu_w(m.w);
                s.set_devmenu_x(m.pos.0);
                s.set_devmenu_y(m.pos.1);
                sync_model(&self.devmenu_model, items);
                s.set_devmenu_open(true);
            }
            None => s.set_devmenu_open(false),
        }
        s.set_renaming_device(self.devices.renaming.map_or(-1, |d| d.0 as i32));

        // ---- Settings › Plugins (only while that page is shown) ----
        if !(self.settings.open && self.settings.page == crate::settings::PLUGINS_PAGE) {
            return;
        }
        let cat = &self.devices.catalog;
        let status = match &cat.state {
            ScanState::Idle => "Not scanned yet".to_string(),
            ScanState::Scanning => {
                let p = cat.progress.lock().map(|p| p.clone()).unwrap_or_default();
                if p.is_empty() { "Scanning…".to_string() } else { format!("Scanning… {p}") }
            }
            ScanState::Done => format!(
                "{} plugins found{}{}",
                cat.plugins.len(),
                if cat.from_cache { " (from the scan cache)" } else { "" },
                if cat.fixture.is_some() { " (TEST fixture)" } else { "" }
            ),
            ScanState::Failed(e) => format!("Scan failed: {e}"),
        };
        s.set_plugins_status(status.into());
        s.set_plugins_scanning(cat.state == ScanState::Scanning);
        let home = std::env::var(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).unwrap_or_default();
        let tilde = |d: &Path| {
            let s = d.display().to_string();
            match s.strip_prefix(&home) {
                Some(rest) if !home.is_empty() => format!("~{rest}"),
                _ => s,
            }
        };
        let dirs: Vec<PluginRow> = scan_dirs()
            .into_iter()
            .map(|d| (d, "Default"))
            .chain(configured_scan_dirs())
            // A configured path may be a folder or a single bundle.
            .map(|(d, badge)| PluginRow {
                name: tilde(&d).into(),
                detail: "".into(),
                badge: badge.into(),
                warn: !d.exists() || (badge == "Default" && !d.is_dir()),
            })
            // Custom folders last; `detail` carries the path the remove button names.
            .chain(self.settings.prefs.plugin_folders.iter().map(|f| {
                let d = Path::new(f);
                PluginRow { name: tilde(d).into(), detail: f.into(), badge: CUSTOM_BADGE.into(), warn: !d.is_dir() }
            }))
            .collect();
        sync_model(&self.scan_dirs_model, dirs);
        let mut found: Vec<&PluginInfo> = cat.plugins.iter().collect();
        found.sort_by_key(|p| (format_order(p.plugin.format), p.plugin.vendor.to_lowercase(), p.plugin.name.to_lowercase()));
        let rows: Vec<PluginRow> = found
            .iter()
            .map(|p| PluginRow {
                name: p.plugin.name.clone().into(),
                detail: format!(
                    "{} · {}{}",
                    if p.plugin.vendor.is_empty() { "Unknown vendor" } else { &p.plugin.vendor },
                    if p.plugin.kind == PluginKind::Instrument { "Instrument" } else { "Audio effect" },
                    if p.plugin.version.is_empty() { String::new() } else { format!(" · {}", p.plugin.version) }
                )
                .into(),
                badge: format_badge(p.plugin.format).into(),
                warn: false,
            })
            .collect();
        sync_model(&self.plugin_rows_model, rows);
        let failed: Vec<PluginRow> = cat
            .failed
            .iter()
            .map(|(what, why)| PluginRow { name: what.clone().into(), detail: why.clone().into(), badge: "".into(), warn: true })
            .collect();
        sync_model(&self.plugin_failed_model, failed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Built-in cards show the window button exactly when the device has an expanded window:
    /// effects (the LFO included) do, the Rack does not.
    #[test]
    fn builtin_cards_offer_the_window_only_when_there_is_one() {
        for (id, want) in [("delay", true), ("lfo", true), ("trance-gate", true), ("device-rack", false)] {
            let engine = std::sync::Arc::new(dstudio_engine::Engine::new(dstudio_engine::Config::default()).unwrap());
            let mut c = Controller::new(engine, dstudio_core::Project::default());
            let def = dstudio_core::builtin::find(id).unwrap();
            let track = c.project().tracks[0].id;
            assert!(c.execute(Command::AddDevice { track, index: None, device: def.instantiate() }), "{id}");
            let d = c.project().tracks[0].devices.last().unwrap().clone();
            let card = c.card_for(&d, false);
            assert!(card.builtin, "{id}");
            assert_eq!(card.has_editor, want, "{id}");
        }
    }

    #[test]
    fn custom_plugin_folders_are_added_once_and_never_shadow_a_standard_one() {
        let mut v = Vec::new();
        let dir = std::env::temp_dir().join("dstudio-custom-plugins");
        add_plugin_folder(&mut v, &dir).unwrap();
        assert!(add_plugin_folder(&mut v, &dir).is_err(), "added twice");
        assert!(add_plugin_folder(&mut v, &scan_dirs()[0]).is_err(), "a standard folder is already scanned");
        let bad = if cfg!(windows) { "a\"b" } else { "a:b" };
        assert!(add_plugin_folder(&mut v, &std::env::temp_dir().join(bad)).is_err(), "the scanner could not split it");
        assert_eq!(v, [dir.to_string_lossy().into_owned()]);
    }

    fn info(name: &str, vendor: &str, fmt: PluginFormat, kind: PluginKind) -> PluginInfo {
        PluginInfo {
            plugin: PluginRef {
                format: fmt,
                id: format!("id-{name}"),
                path: None,
                name: name.into(),
                vendor: vendor.into(),
                version: String::new(),
                kind,
            },
            has_editor: true,
            audio_inputs: 2,
            audio_outputs: 2,
        }
    }

    fn catalog() -> Catalog {
        Catalog {
            plugins: vec![
                info("AULowpass", "Apple", PluginFormat::AudioUnit, PluginKind::Effect),
                info("AUDelay", "Apple", PluginFormat::AudioUnit, PluginKind::Effect),
                info("Crusher", "Acme", PluginFormat::AudioUnit, PluginKind::Effect),
                info("DLSMusicDevice", "Apple", PluginFormat::AudioUnit, PluginKind::Instrument),
                info("Gain", "DStudio Test", PluginFormat::Vst3, PluginKind::Effect),
                info("Sine", "DStudio Test", PluginFormat::Clap, PluginKind::Instrument),
            ],
            state: ScanState::Done,
            ..Default::default()
        }
    }

    fn labels(rows: &[TreeRow]) -> Vec<(i32, String, String)> {
        rows.iter().map(|r| (r.depth, r.label.to_string(), r.count.to_string())).collect()
    }

    #[test]
    fn effects_tree_groups_by_format_and_au_vendor() {
        let c = catalog();
        let mut open: HashSet<String> = ["e/au", "e/vst3", "e/clap"].iter().map(|s| s.to_string()).collect();
        let (_, rows) = build_tree(&c, &Default::default(), 1, "", &open);
        let l = |d: i32, s: &str, n: &str| (d, s.to_string(), n.to_string());
        // Every built-in effect is listed under Built-in (more arrive with each device family).
        let n = dstudio_core::builtin::all().iter().filter(|d| d.kind == PluginKind::Effect && dstudio_core::builtin::listed(d)).count();
        let built_in = n.to_string();
        assert_eq!(
            labels(&rows),
            vec![
                l(0, "Built-in", &built_in),
                l(0, "Audio Units", "3"),
                l(1, "Acme", "1"),
                l(1, "Apple", "2"),
                l(0, "VST3", "1"),
                l(1, "Gain", ""),
                l(0, "CLAP", "0")
            ]
        );
        assert_eq!(rows[6].expander, 0, "empty folder has no expander");
        open.insert("e/au/Apple".into());
        open.insert("e/builtin".into());
        let (entries, rows) = build_tree(&c, &Default::default(), 1, "", &open);
        assert!(matches!(&entries[1].item, Some(Item::Builtin(d)) if d.id == "compressor"));
        // One row per built-in device under the open Built-in folder.
        assert_eq!(rows[4 + n].label, "AUDelay");
        assert_eq!(rows[5 + n].label, "AULowpass");
        assert!(rows[5 + n].leaf && rows[5 + n].icon == 2);
        assert!(matches!(&entries[5 + n].item, Some(Item::Plugin(p)) if p.name == "AULowpass"));
    }

    #[test]
    fn instruments_tree_has_builtin_synth_first() {
        let c = catalog();
        let open: HashSet<String> = ["i/builtin", "i/au", "i/au/Apple"].iter().map(|s| s.to_string()).collect();
        let (entries, rows) = build_tree(&c, &Default::default(), 0, "", &open);
        assert_eq!(rows[0].label, "Built-in");
        assert_eq!(entries[1].item, Some(Item::BuiltinSynth(None)));
        assert_eq!(rows[0].count, "4");
        assert_eq!((rows[1].label.as_str(), rows[1].count.as_str(), rows[1].expander), ("Synth", "100", 1));
        assert_eq!([rows[2].label.as_str(), rows[3].label.as_str(), rows[4].label.as_str()], ["Sampler", "Drum Rack", "Pad Sampler"]);
        assert!(matches!(&entries[3].item, Some(Item::Builtin(d)) if d.id == "drum-rack"));
        assert_eq!(rows[5].label, "Audio Units");
        assert_eq!(rows[7].label, "DLSMusicDevice");
    }

    #[test]
    fn synth_folder_opens_into_categories_and_sounds() {
        let c = catalog();
        let open: HashSet<String> = ["i/builtin", "i/b/synth", "i/b/synth/Bass"].iter().map(|s| s.to_string()).collect();
        let (entries, rows) = build_tree(&c, &Default::default(), 0, "", &open);
        assert_eq!((rows[2].label.as_str(), rows[2].count.as_str(), rows[2].depth), ("Bass", "17", 2));
        assert!(matches!(entries[3].item, Some(Item::BuiltinSynth(Some(_)))) && rows[3].depth == 3);
        assert_eq!(rows[20].label, "Keys", "Bass holds 17 sounds");
        // A search finds sounds by name inside their category.
        let (entries, rows) = build_tree(&c, &Default::default(), 0, "analog kick", &HashSet::new());
        assert_eq!(labels(&rows).iter().map(|r| r.1.as_str()).collect::<Vec<_>>(), ["Built-in", "Synth", "Drums", "Analog Kick"]);
        assert!(matches!(entries[3].item, Some(Item::BuiltinSynth(Some(_)))));
    }

    #[test]
    fn search_filters_and_expands() {
        let c = catalog();
        let (entries, rows) = build_tree(&c, &Default::default(), 1, "lowp", &HashSet::new());
        assert_eq!(labels(&rows).iter().map(|r| r.1.as_str()).collect::<Vec<_>>(), ["Audio Units", "Apple", "AULowpass"]);
        assert_eq!(rows[0].count, "1");
        assert!(entries[2].item.is_some());
        let (_, rows) = build_tree(&c, &Default::default(), 1, "nothing-matches", &HashSet::new());
        assert!(rows.is_empty());
        // Vendor matches too.
        let (_, rows) = build_tree(&c, &Default::default(), 1, "acme", &HashSet::new());
        assert_eq!(rows.last().unwrap().label, "Crusher");
    }

    #[test]
    fn chains_tab_lists_folders_and_opens_them() {
        let lib = crate::chain_presets::ChainLib::default();
        let (entries, rows) = build_tree(&catalog(), &lib, 2, "", &HashSet::new());
        assert_eq!(rows.len(), dstudio_core::chains::FOLDERS.len(), "closed folders only");
        assert!(entries.iter().all(|e| e.folder));
        let open: HashSet<String> = ["c/Drums".to_string()].into();
        let (entries, _) = build_tree(&catalog(), &lib, 2, "", &open);
        assert!(entries.iter().any(|e| matches!(&e.item, Some(Item::Chain(r)) if r.name == "Punchy Kit")));
        // A search opens every folder and keeps only matching chains.
        let (entries, _) = build_tree(&catalog(), &lib, 2, "glue", &HashSet::new());
        let names: Vec<_> = entries.iter().filter_map(|e| e.item.as_ref().map(|i| i.name().to_string())).collect();
        assert_eq!(names, vec!["Bus Glue".to_string()]);
    }

    #[test]
    fn latency_text() {
        assert_eq!(fmt_latency(0, 48_000.0), "");
        assert_eq!(fmt_latency(480, 48_000.0), "Latency 480 samples (10.0 ms)");
    }
}
