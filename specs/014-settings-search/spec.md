# Settings search

Status: Done · Spec: 014 · Issue: none

## Problem

The "Search settings" field at the top of the Settings sidebar is a dummy: it draws a magnifier and
a hint, takes no text and filters nothing. Settings now has about 25 controls over five real pages
(General, Audio, MIDI, Plugins, Shortcuts), and someone who wants "buffer size" or "default
instrument" has to guess which page holds it. Shortcuts has its own search for actions and keys, but
nothing finds a setting.

## Behaviour

Roadmap G13 ([010](../010-parity-roadmap/spec.md)): typing filters the sidebar to the pages with
matching rows and highlights the matching rows on the open page; Escape clears.

- Clicking the field puts a text cursor in it. While it is empty, Settings looks and works as today.
- With text typed, the sidebar keeps only the pages that match, in their usual order, packed
  together. A page matches when its name contains the text or one of its settings does.
- A setting matches by its label or a few keywords: "latency" finds Buffer size, Sample rate and
  Plugin delay compensation; "synth" finds Default instrument; "folder" finds Projects folder and
  Scan directories. Matching ignores case and surrounding spaces and needs the text as one piece.
- On the open page, each matching row gets a soft accent highlight behind it. If the open page
  doesn't match, the first matching page opens. Clicking another page in the filtered sidebar opens
  it with its matches highlighted.
- When nothing matches, the sidebar shows "No settings match your search" and the open page stays.
- Escape with text in the field clears it and keeps Settings open; Escape with the field empty
  closes Settings as today. ⌘F while Settings is open puts the cursor in the field.
- Greyed-out settings that Device can't back (sample licensing, the assistant's voice rows) and the
  "Not available yet" pages match by page name only. Shortcuts actions are not searched here: the
  Shortcuts page has its own search.
- The text is cleared when Settings closes. Nothing in a project or the preferences changes, and
  there is no undo step.

## Requirements

- R1. The field takes typed text like the other search fields, and while it has focus typing never
  triggers shortcuts.
- R2. One table lists every working setting on General, Audio, MIDI, Plugins and Shortcuts with its
  page, its label and its keywords.
- R3. A non-empty query shows only matching pages in the sidebar, in order and packed; an empty one
  shows every page as today.
- R4. Matching is case-insensitive, trims surrounding spaces and needs the query as a contiguous part
  of the page name, a label or a keyword.
- R5. Matching rows on the open page are highlighted; if the open page doesn't match and another
  does, the first matching page opens.
- R6. No match shows "No settings match your search" in the sidebar and leaves the page as it is.
- R7. Escape clears a non-empty query without closing Settings; with an empty query it closes
  Settings. ⌘F while Settings is open focuses the field.
- R8. The query is cleared when Settings closes.
- R9. The table stays complete: a test fails when a highlightable row on a page is missing from it,
  or the table names a row the pages don't have.

## Acceptance checks

- A1 (R2, R9): a unit test compares the table with the rows `settings.slint` marks for search.
- A2 (R3, R4): unit tests of the matcher: "buf" finds Buffer size; " AUDIO " matches the Audio page;
  "latency" finds three Audio settings; "zzz" nothing; "" every page.
- A3 (R1, R3, R5, R6, R8): self-test: open Settings on General, type "instr": the sidebar lists only
  MIDI, the MIDI page is open and Default instrument is highlighted; type "zzz": the no-match text;
  close and reopen: the field is empty and every page is listed.
- A4 (R7): self-test: Escape with text clears it and Settings stays open; Escape again closes it;
  ⌘F focuses the field.
- A5 (R3, R5, R6): review-state screenshots `review-settings-search` ("latency" on Audio) and
  `review-settings-search-none`.

## Out of scope

- Searching Shortcuts actions, plugin names or MIDI device names from this field.
- Fuzzy or typo-tolerant matching, ranking, search history, a results list.
- Help text on each setting (roadmap G12) and hiding the Account and Usage pages (Q3); those are
  their own changes.

## Open questions

None. Agreed 2026-11-20: highlight in place as roadmap G13 says, a few keywords per setting, no
Shortcuts actions, ⌘F focuses the field, the query clears on close.
