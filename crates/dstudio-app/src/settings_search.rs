//! Settings › Search settings (spec 014, roadmap G13): typing filters the sidebar to the pages
//! with matching rows and highlights those rows on the open page. Escape clears the text.
//!
//! [ENTRIES] lists every working setting with its page, label and a few keywords. The rows in
//! `ui/settings.slint` that can be highlighted are its `SearchHit` elements; a test keeps the two
//! in step.

use crate::controller::Controller;
use crate::settings::PAGES;

/// A searchable setting: its page (index into [PAGES]), its label as shown, and other words
/// someone might type for it.
pub struct Entry {
    pub page: usize,
    pub label: &'static str,
    pub keywords: &'static [&'static str],
}

const fn e(page: usize, label: &'static str, keywords: &'static [&'static str]) -> Entry {
    Entry { page, label, keywords }
}

pub const ENTRIES: &[Entry] = &[
    // General
    e(0, "Always load last session", &["startup", "launch", "reopen", "open", "session"]),
    e(0, "Projects folder", &["folder", "save", "location", "directory"]),
    e(0, "Playhead on stop", &["transport", "stop", "return to start", "stop in place"]),
    e(0, "Sections move clips", &["markers", "arrangement", "timeline annotations"]),
    e(0, "Auto-show third-party effect windows", &["plugin windows", "editor", "open"]),
    e(0, "Auto-show built-in effect windows", &["plugin windows", "device window", "open"]),
    e(0, "Keep plugin windows on top", &["float", "front", "editor"]),
    // Audio
    e(1, "Output device", &["speakers", "headphones", "interface", "sound card", "test tone", "playback"]),
    e(1, "Input device", &["microphone", "mic", "interface", "sound card", "recording"]),
    e(1, "Mute audio input", &["microphone", "mic", "feedback", "recording"]),
    e(1, "Sample rate", &["latency", "khz", "hz", "quality"]),
    e(1, "Buffer size", &["latency", "samples", "dropouts", "crackles", "glitches"]),
    e(1, "Plugin delay compensation", &["latency", "pdc", "alignment"]),
    // MIDI
    e(2, "Default instrument", &["synth", "sampler", "drum rack", "new track", "instrument plugin"]),
    e(2, "MIDI inputs", &["keyboard", "controller", "midi device"]),
    // Plugins
    e(3, "Scan directories", &["folder", "add folder", "path", "vst3", "clap", "audio units", "au"]),
    e(3, "Plugin scanner", &["scan", "rescan", "check for new plugins", "blocklist", "failed", "vst3", "clap"]),
    // Shortcuts
    e(4, "Preset", &["keys", "keyboard", "key commands", "keymap", "other app"]),
    e(4, "Reset to defaults", &["keys", "keyboard", "shortcuts", "restore"]),
];

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

    fn hits(q: &str) -> Vec<&'static str> {
        ENTRIES.iter().filter(|e| row_hit(q, e.label)).map(|e| e.label).collect()
    }

    #[test]
    fn matching_ignores_case_and_spaces_and_uses_keywords() {
        assert_eq!(hits("buf"), ["Buffer size"]);
        assert_eq!(hits("latency"), ["Sample rate", "Buffer size", "Plugin delay compensation"]);
        assert_eq!(hits("synth"), ["Default instrument"]);
        assert_eq!(hits("folder"), ["Projects folder", "Scan directories"]);
        assert!(hits("zzz").is_empty() && hits("").is_empty() && hits("   ").is_empty());
        assert!(page_matches(1, " AUDIO "), "the page name matches");
        assert_eq!(page_slots(""), (0..PAGES.len() as i32).collect::<Vec<_>>());
        assert_eq!(page_slots("instr"), [-1, -1, 0, -1, -1, -1, -1, -1, -1]);
        assert_eq!(page_slots("latency"), [-1, 0, -1, -1, -1, -1, -1, -1, -1]);
        assert_eq!(page_slots("privacy").iter().filter(|&&s| s >= 0).count(), 1, "unavailable pages match by name");
        assert!(page_slots("zzz").iter().all(|&s| s < 0));
    }

    /// Every `SearchHit` row in settings.slint is in the table on the right page, and every table
    /// entry has its row: the two can't drift apart.
    #[test]
    fn the_table_matches_the_rows_settings_slint_marks() {
        let ui = include_str!("../ui/settings.slint");
        let pages = [("GeneralPage", 0), ("AudioPage", 1), ("MidiPage", 2), ("PluginsPage", 3), ("ShortcutsPage", 4)];
        let mut found: Vec<(usize, String)> = Vec::new();
        for (name, page) in pages {
            let start = ui.find(&format!("component {name} inherits")).unwrap_or_else(|| panic!("{name}"));
            let end = ui[start + 10..].find("\ncomponent ").map_or(ui.len(), |i| start + 10 + i);
            let body = &ui[start..end];
            for part in body.split("SearchHit {").skip(1) {
                let Some(l) = part.find("label: \"") else { continue };
                let rest = &part[l + 8..];
                found.push((page, rest[..rest.find('"').unwrap()].to_string()));
            }
            // General's switch rows share one SearchHit: their labels are the `t` of the rows
            // with a working switch (k >= 0).
            for row in body.split("{ t: \"").skip(1) {
                let label = &row[..row.find('"').unwrap()];
                let fields = &row[..row.find('}').unwrap_or(row.len())];
                let k = fields.find("k: ").map(|i| fields[i + 3..].trim_start().starts_with('-'));
                if k == Some(false) {
                    found.push((page, label.to_string()));
                }
            }
        }
        let mut want: Vec<(usize, String)> = ENTRIES.iter().map(|e| (e.page, e.label.to_string())).collect();
        found.sort();
        want.sort();
        assert_eq!(found, want);
    }
}
