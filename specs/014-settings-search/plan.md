# Plan: Settings search

Spec: [spec.md](spec.md)

App only. No core, engine, ABI or file-format change.

- `src/settings_search.rs` (new): the table (`ENTRIES`: page, label, keywords), `matches(query,
  text)`, `row_hit(query, label)`, `page_matches(page, query)`, and `Controller::settings_search`,
  which stores the query and opens the first matching page when the open one doesn't match.
- `src/settings.rs`: `SettingsState::search`; `close_settings` clears it; `push_settings` sends
  the query, each page's sidebar slot (-1 hidden) and whether nothing matched.
- `src/main.rs`: Escape clears a non-empty query before closing Settings; ⌘F with Settings open
  asks the field to take focus; the `settings-row-hit` pure callback calls `row_hit`.
- `ui/settings.slint`: the sidebar field becomes a `TextInput` (sets `search-focused` like the
  Shortcuts field), the sidebar rows take their slot, a "No settings match your search" line, and
  a `SearchHit` highlight behind each searchable row. `ui/studio.slint`: the new properties and
  callbacks.

Principles: no audio-thread or document change; nothing saved.

Risk: the table and the Slint rows drift apart. A test reads `settings.slint` and compares.
