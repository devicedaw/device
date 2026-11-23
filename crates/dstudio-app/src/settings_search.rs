//! Settings › Search settings (spec 014, roadmap G13): typing filters the sidebar to the pages
//! with matching rows and highlights those rows on the open page. Escape clears the text.
//!
//! [ENTRIES] lists every working setting with its page, label and a few keywords. The rows in
//! `ui/settings.slint` that can be highlighted are its `SearchHit` elements; a test keeps the two
//! in step.

use crate::controller::Controller;
use crate::settings::PAGES;

/// A searchable setting: its page (index into [PAGES]), its label as shown, other words someone
/// might type for it, and the one sentence its (i) shows on hover (roadmap G12; empty on the
/// pages without an (i): Shortcuts and Downloads).
pub struct Entry {
    pub page: usize,
    pub label: &'static str,
    pub keywords: &'static [&'static str],
    pub help: &'static str,
}

const fn e(page: usize, label: &'static str, keywords: &'static [&'static str], help: &'static str) -> Entry {
    Entry { page, label, keywords, help }
}

pub const ENTRIES: &[Entry] = &[
    // General
    e(
        0,
        "Always load last session",
        &["startup", "launch", "reopen", "open", "session"],
        "Reopens the project you last had open when Device starts.",
    ),
    e(
        0,
        "Check for updates at launch",
        &["updates", "new version", "release", "startup", "internet"],
        "Asks GitHub once at launch whether a newer version of Device is out, sending only the version you run.",
    ),
    e(
        0,
        "Projects folder",
        &["folder", "save", "location", "directory"],
        "Where Save starts and the folder the browser's Projects tab lists.",
    ),
    e(
        0,
        "Playhead on stop",
        &["transport", "stop", "return to start", "stop in place"],
        "Whether stopping sends the playhead back to where playback began or leaves it where it stopped.",
    ),
    e(
        0,
        "Sections move clips",
        &["markers", "arrangement", "timeline annotations"],
        "Moving, resizing, adding or deleting a section also moves the clips and automation after it.",
    ),
    e(
        0,
        "Auto-show third-party effect windows",
        &["plugin windows", "editor", "open"],
        "Opens a plugin's own window as soon as you add it to a track.",
    ),
    e(
        0,
        "Auto-show built-in effect windows",
        &["plugin windows", "device window", "open"],
        "Opens a built-in effect's large window as soon as you add it to a track.",
    ),
    e(0, "Keep plugin windows on top", &["float", "front", "editor"], "Keeps plugin windows in front of the main window."),
    e(
        0,
        "Let the assistant listen to audio",
        &["assistant", "analysis", "loudness", "key", "tempo", "hear"],
        "Each assistant message includes a short description of the selection's sound: loudness, tone, key and tempo. The audio itself stays on this computer.",
    ),
    e(
        0,
        "Let AI apps control Device",
        &["mcp", "claude", "chatgpt", "ai", "assistant", "cursor"],
        "Lets AI apps such as Claude Desktop read and edit your project on this computer.",
    ),
    // Audio
    e(
        1,
        "Output device",
        &["speakers", "headphones", "interface", "sound card", "test tone", "playback"],
        "The interface or speakers Device plays through; Test plays a short, quiet tone.",
    ),
    e(
        1,
        "Input device",
        &["microphone", "mic", "interface", "sound card", "recording"],
        "The interface or microphone that audio tracks record from.",
    ),
    e(
        1,
        "Mute audio input",
        &["microphone", "mic", "feedback", "recording"],
        "Silences every audio input, for example to stop a laptop microphone feeding back.",
    ),
    e(
        1,
        "Sample rate",
        &["latency", "khz", "hz", "quality"],
        "How many samples per second the engine works at; higher rates use more CPU.",
    ),
    e(
        1,
        "Buffer size",
        &["latency", "samples", "dropouts", "crackles", "glitches"],
        "Smaller buffers respond faster but use more CPU; raise it if playback crackles.",
    ),
    e(
        1,
        "Plugin delay compensation",
        &["latency", "pdc", "alignment"],
        "Delays the other tracks so that tracks with slow plugins stay in time with them.",
    ),
    e(
        1,
        "Recording offset",
        &["latency", "calibration", "late", "early", "takes", "timing"],
        "Moves takes from this input device earlier (positive) or later, for interfaces that report their latency wrongly.",
    ),
    // MIDI
    e(
        2,
        "Default instrument",
        &["synth", "sampler", "drum rack", "new track", "instrument plugin"],
        "The instrument that new instrument tracks start with.",
    ),
    e(
        2,
        "MIDI inputs",
        &["keyboard", "controller", "midi device"],
        "Keyboards and controllers Device listens to; switch one off to ignore it.",
    ),
    // Plugins
    e(
        3,
        "Scan directories",
        &["folder", "add folder", "path", "vst3", "clap", "audio units", "au"],
        "The folders Device searches for plugins; Add folder adds one of your own.",
    ),
    e(
        3,
        "Plugin scanner",
        &["scan", "rescan", "check for new plugins", "blocklist", "failed", "vst3", "clap"],
        "Looks for plugins added since the last scan, or checks every plugin again from scratch.",
    ),
    // Shortcuts
    e(4, "Preset", &["keys", "keyboard", "key commands", "keymap", "other app"], ""),
    e(4, "Reset to defaults", &["keys", "keyboard", "shortcuts", "restore"], ""),
    // Downloads (nothing to download yet: the page's message is its one row)
    e(5, "Downloadable content", &["download", "sound library", "library", "content", "samples", "multi-sample", "install"], ""),
    // Privacy
    e(
        PRIVACY_PAGE,
        "Send crash reports",
        &["crash", "bug", "report", "diagnostics", "error"],
        "After a crash, the next launch sends the app version, your OS and what failed, with nothing personal.",
    ),
    e(
        PRIVACY_PAGE,
        "Usage tracking",
        &["analytics", "telemetry", "statistics", "data collection"],
        "Device collects no usage data at all, so this stays off.",
    ),
];

/// Settings › Privacy (spec 018).
pub const PRIVACY_PAGE: usize = 6;

/// The (i) sentence for the setting labelled `label` ("" when it has none).
pub fn help(label: &str) -> &'static str {
    ENTRIES.iter().find(|e| e.label == label).map_or("", |e| e.help)
}

/// The query as matched: trimmed and lower-cased. Empty = no search.
pub fn normalize(query: &str) -> String {
    query.trim().to_lowercase()
}

fn contains(text: &str, q: &str) -> bool {
    text.to_lowercase().contains(q)
}

/// Entry `e` matches the normalised query `q` (non-empty).
fn entry_matches(e: &Entry, q: &str) -> bool {
    contains(e.label, q) || e.keywords.iter().any(|k| contains(k, q))
}

/// The row labelled `label` is highlighted for `query`.
pub fn row_hit(query: &str, label: &str) -> bool {
    let q = normalize(query);
    !q.is_empty() && ENTRIES.iter().any(|e| e.label == label && entry_matches(e, &q))
}

/// Page `page` is listed for `query`: every page for an empty query, else when its name or one of
/// its settings matches.
pub fn page_matches(page: usize, query: &str) -> bool {
    let q = normalize(query);
    q.is_empty() || PAGES.get(page).is_some_and(|name| contains(name, &q)) || ENTRIES.iter().any(|e| e.page == page && entry_matches(e, &q))
}

/// Each page's row in the filtered sidebar (-1 = not listed).
pub fn page_slots(query: &str) -> Vec<i32> {
    let mut next = 0;
    (0..PAGES.len())
        .map(|p| {
            if page_matches(p, query) {
                next += 1;
                next - 1
            } else {
                -1
            }
        })
        .collect()
}

impl Controller {
    /// Typing in the field. Opens the first matching page when the open one doesn't match.
    pub fn settings_search(&mut self, text: &str) {
        self.settings.search = text.to_string();
        if !page_matches(self.settings.page, text)
            && let Some(p) = (0..PAGES.len()).find(|&p| page_matches(p, text))
        {
            self.settings.menu = None;
            self.settings.page = p;
        }
        self.ui_dirty = true;
    }

    /// Escape with text in the field: clear it (true), so Escape doesn't also close Settings.
    pub fn settings_search_clear(&mut self) -> bool {
        if self.settings.search.is_empty() {
            return false;
        }
        self.settings.search.clear();
        self.ui_dirty = true;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PAGE_COMPONENTS: [(&str, usize); 7] = [
        ("GeneralPage", 0),
        ("AudioPage", 1),
        ("MidiPage", 2),
        ("PluginsPage", 3),
        ("ShortcutsPage", 4),
        ("DownloadsPage", 5),
        ("PrivacyPage", PRIVACY_PAGE),
    ];

    /// The source of `component <name>` in settings.slint.
    fn page_body(name: &str) -> &'static str {
        let ui = include_str!("../ui/settings.slint");
        let start = ui.find(&format!("component {name} inherits")).unwrap_or_else(|| panic!("{name}"));
        let end = ui[start + 10..].find("\ncomponent ").map_or(ui.len(), |i| start + 10 + i);
        &ui[start..end]
    }

    /// General's switch rows share one SearchHit and one RowLabel: their labels are the `t` of
    /// the rows with a working switch (k >= 0).
    fn general_switch_rows(body: &str) -> impl Iterator<Item = &str> {
        body.split("{ t: \"").skip(1).filter_map(|row| {
            let fields = &row[..row.find('}').unwrap_or(row.len())];
            let k = fields.find("k: ").map(|i| fields[i + 3..].trim_start().starts_with('-'));
            (k == Some(false)).then(|| &row[..row.find('"').unwrap()])
        })
    }

    /// Pages whose rows have an (i) with help (roadmap G12): General, Audio, MIDI, Plugins, Privacy.
    fn page_has_help(page: usize) -> bool {
        page <= 3 || page == PRIVACY_PAGE
    }

    fn hits(q: &str) -> Vec<&'static str> {
        ENTRIES.iter().filter(|e| row_hit(q, e.label)).map(|e| e.label).collect()
    }

    #[test]
    fn matching_ignores_case_and_spaces_and_uses_keywords() {
        assert_eq!(hits("buf"), ["Buffer size"]);
        assert_eq!(hits("latency"), ["Sample rate", "Buffer size", "Plugin delay compensation", "Recording offset"]);
        assert_eq!(hits("calib"), ["Recording offset"]);
        assert_eq!(hits("synth"), ["Default instrument"]);
        assert_eq!(hits("folder"), ["Projects folder", "Scan directories"]);
        assert!(hits("zzz").is_empty() && hits("").is_empty() && hits("   ").is_empty());
        assert!(page_matches(1, " AUDIO "), "the page name matches");
        assert_eq!(page_slots(""), (0..PAGES.len() as i32).collect::<Vec<_>>());
        assert_eq!(page_slots("instr"), [-1, -1, 0, -1, -1, -1, -1]);
        assert_eq!(page_slots("latency"), [-1, 0, -1, -1, -1, -1, -1]);
        assert_eq!(page_slots("downloads"), [-1, -1, -1, -1, -1, 0, -1], "the page name matches");
        assert_eq!(page_slots("sound library"), [-1, -1, -1, -1, -1, 0, -1]);
        assert_eq!(page_slots("telemetry"), [-1, -1, -1, -1, -1, -1, 0]);
        assert_eq!(PAGES[PRIVACY_PAGE], "Privacy");
        assert!(!PAGES.contains(&"Account") && !PAGES.contains(&"Usage"), "hidden (roadmap Q3)");
        assert!(page_slots("zzz").iter().all(|&s| s < 0));
    }

    /// Every `SearchHit` row in settings.slint is in the table on the right page, and every table
    /// entry has its row: the two can't drift apart.
    #[test]
    fn the_table_matches_the_rows_settings_slint_marks() {
        let mut found: Vec<(usize, String)> = Vec::new();
        for (name, page) in PAGE_COMPONENTS {
            let body = page_body(name);
            for part in body.split("SearchHit {").skip(1) {
                let Some(l) = part.find("label: \"") else { continue };
                let rest = &part[l + 8..];
                found.push((page, rest[..rest.find('"').unwrap()].to_string()));
            }
            found.extend(general_switch_rows(body).map(|l| (page, l.to_string())));
        }
        let mut want: Vec<(usize, String)> = ENTRIES.iter().map(|e| (e.page, e.label.to_string())).collect();
        found.sort();
        want.sort();
        assert_eq!(found, want);
    }

    /// Help on every setting (roadmap G12): every working setting on General, Audio, MIDI,
    /// Plugins and Privacy has one sentence, and every RowLabel there that isn't greyed shows one from the
    /// table (`setting:` names it when the text shown has a count after it).
    #[test]
    fn every_working_row_has_one_sentence_of_help() {
        for e in ENTRIES {
            if page_has_help(e.page) {
                let h = e.help;
                assert!(
                    h.ends_with('.') && h.starts_with(char::is_uppercase) && !h[..h.len() - 1].contains(". ") && h.len() <= 120,
                    "{}: {h:?} is not one short sentence",
                    e.label
                );
            } else {
                assert!(e.help.is_empty(), "{} has help but its page shows no (i)", e.label);
            }
        }
        let mut found: Vec<(usize, String)> = Vec::new();
        for (name, page) in PAGE_COMPONENTS.iter().filter(|(_, p)| page_has_help(*p)) {
            let body = page_body(name);
            for part in body.split("RowLabel {").skip(1) {
                let fields = &part[..part.find('}').unwrap()];
                if fields.contains("dim: true") || fields.contains("text: r.t") {
                    continue;
                }
                let key = if fields.contains("setting: \"") { "setting: \"" } else { "text: \"" };
                let rest = &fields[fields.find(key).unwrap_or_else(|| panic!("{name}: {fields}")) + key.len()..];
                found.push((*page, rest[..rest.find('"').unwrap()].to_string()));
            }
            found.extend(general_switch_rows(body).map(|l| (*page, l.to_string())));
        }
        let mut want: Vec<(usize, String)> =
            ENTRIES.iter().filter(|e| page_has_help(e.page)).map(|e| (e.page, e.label.to_string())).collect();
        found.sort();
        want.sort();
        assert_eq!(found, want);
        assert_eq!(help("Buffer size"), ENTRIES.iter().find(|e| e.label == "Buffer size").unwrap().help);
        assert_eq!(help("No such setting"), "");
        assert!(!include_str!("../ui/settings.slint").contains("License samples"), "hidden (roadmap Q3)");
    }
}
