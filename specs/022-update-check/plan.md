# Plan: Update check and the "Update available" pill

Spec: [spec.md](spec.md)

App only. No core, engine, ABI or file-format change. Follows the bug report form (`src/feedback.rs`): a `ureq`
request on a worker thread, its result back through `JobResult`, and a view model pushed to Slint.

- `src/updates.rs` (new), from the check half of the draft on `chrislaupama/device-update-from-app`:
  - `Version` (SemVer with pre-release order) and `is_newer`.
  - `parse_release`: GitHub's release JSON to a `Release` (version, page, the first three note items, how many
    more); drafts, pre-release flags and pre-release tags give `Ok(None)`. `notes_summary` reads the changelog
    bullets before GitHub's "## What's Changed" list and turns their Markdown into short plain lines.
  - `fetch_latest`: one GET to `/repos/devicedaw/device/releases/latest` with `User-Agent: Device/<version>`,
    `Accept: application/vnd.github+json` and `X-GitHub-Api-Version`; HTTPS only (plain HTTP to 127.0.0.1 for
    tests); 404 is "no release"; 403 with `x-ratelimit-remaining: 0` or 429 is the rate-limit message with the
    minutes from `retry-after` or `x-ratelimit-reset`; other statuses and connection errors are plain messages.
  - `check`: a launch check answers from `update-check.json` (next to the preferences) when the last successful
    request is under 12 hours old; every successful request is saved there.
  - `Updates` on the controller: the dialog state (Idle, Checking, UpToDate, Available, Failed), whether a check is
    running, the saved-file path, and the self-test hook `check_override` (answer at once, record Download in
    `opened` instead of opening a browser). `updates_check(manual)`, `finish_update_check`, `update_pill_clicked`,
    `updates_download`, `push_updates`.
- `src/controller.rs`: `JobResult::UpdateChecked`, the `updates` field, `options.check_updates` calls
  `updates_check(true)`, `push_updates` in the view-model push.
- `src/main.rs`: the callbacks; at startup a real run sets the saved-file path and checks once; self-test and
  `--state` runs set `check_override` (the running version, or `sample_release()` for the review states) so they
  never touch the network. Review states `review-update-available` and `review-update-dialog`.
- `src/prefs.rs`, `src/settings.rs`, `src/settings_search.rs`: `GeneralPrefs::check_updates_at_launch` (default
  on; `#[serde(default)]` loads old files with it on), General switch 5, its search entry.
- `ui/app.slint`: the pill in the header, left of Report a bug, sized to its text. `ui/dialogs.slint`:
  `UpdatesModal` grows with its text and notes; OK, or Later and Download. `ui/settings.slint`: the switch under
  Startup, the rows below it moved down one row. `ui/studio.slint`: the properties and callbacks.

Rejected: the pill opening the browser directly (the user would not see what changed first); asking GitHub on
every launch (quick restarts would spend the 60 unauthenticated requests an hour that a shared network gets);
shipping the draft's download, verify and self-install (see the spec's Out of scope).

## Principles check

- 1 Real-time safety: nothing on the audio thread; the request runs on its own thread, never the UI thread.
- 2 One mutation path: nothing in the project changes; the setting is a preference like the other General switches.
- 3 Projects keep loading: no project change. Preferences gain a field with a default.
- 5 Cross-platform: the request is plain Rust; opening the page uses `open`, `explorer` or `xdg-open`.
- 6 Original work: the notes are the project's own release notes.
- 7 Evidence: screenshots of the new review states and Settings › General, before and after.
- 8 Docs: README (Download), docs/building.md (releases are what running copies are offered), CHANGELOG, PROGRESS.
- 9 Naming: `DSTUDIO_UPDATE_URL`.

## Tests and evidence

- Unit tests in `src/updates.rs`: versions, notes summary, release parsing, the request against a local server
  (headers sent, 404, rate limits, 500, off-line, plain HTTP refused), the 12-hour reuse.
- Self-test: `menu_bar_steps` (up to date), `updates_steps` (pill, dialog, Download, setting off), and
  `general_settings_steps` (the new switch, rows moved).
- Screenshots: `review-update-available`, `review-update-dialog`, `review-check-updates`,
  `reference-settings-general`.
- Benchmarks: none. Nothing runs per frame; one request at launch on its own thread.
