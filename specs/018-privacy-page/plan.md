# Plan: Settings › Privacy

Spec: [spec.md](spec.md)

App and feedback service only. No core, engine, ABI or file-format change; one new preferences field.

- `crates/dstudio-app/src/crash.rs` (new): `CrashRecord`; `install(dir)` sets the panic hook (chained
  to the previous one) and, on Unix, the signal handlers (pre-built path and per-signal JSON, then
  `open`/`write`/`close`, restore the previous `sigaction`, `raise`); `take(dir)` reads and removes the
  record; `scrub` removes home paths and the user name; `payload` builds the service JSON;
  `Controller::send_crash_report` (at launch) and `finish_crash_report`; the Privacy switch;
  `DSTUDIO_CRASH_TEST`.
- `src/main.rs`: install the handlers and send a pending report only when the preferences are used
  (not self-test, `--state`, screenshots); the switch callback; review state `review-settings-privacy`.
- `src/prefs.rs`: `crash_reports_off` (absent = on). `src/controller.rs`: `JobResult::CrashReport`.
- `src/settings.rs`: `PAGES` without Account and Usage; push the switch. `src/settings_search.rs`:
  `PRIVACY_PAGE` and the two Privacy entries with help; tests follow the seven pages.
- `ui/settings.slint`: `PrivacyPage`; General without the Sample licensing section (rows below move
  up); the sidebar icons for seven pages. `ui/studio.slint`: the switch property and callback.
- `services/feedback/lib/report.mjs`: kind `crash` (title `[Crash]`, label `crash`), with a test.
- `Cargo.toml` (app): `libc` on Unix, already in the tree.

Principles: nothing touches the audio thread; the signal handler calls only async-signal-safe functions
and allocates nothing. Original text only.

Risks: a handler that swallows crashes (it re-raises after restoring the previous one, checked by hand
with `DSTUDIO_CRASH_TEST=segv`: the process still ends with SIGSEGV); personal data in a panic message
(paths and the user name are scrubbed; the text is otherwise the program's own).
