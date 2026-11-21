# Settings › Privacy

Status: Done · Spec: 018 · Issue: none

## Problem

Settings › Privacy says "Not available yet". When Device crashes, nothing reaches the maintainers
unless the person writes a bug report by hand, and most never do; the crash's cause (a panic
message, where it happened) is lost when the process ends. People also have no place that says what
Device collects about them, which is nothing. Settings also lists Account and Usage pages and a
"License samples when you use them" switch that only make sense with an online service Device
doesn't have (roadmap Q3: hide them).

## Behaviour

Roadmap G14 ([010](../010-parity-roadmap/spec.md)), with Q3's answer.

- Settings › Privacy has two rows, each with an (i) whose hover explains it (G12):
  - **Send crash reports**, on by default. After Device crashes, the next launch sends a short
    report through the same service as Report a bug, and the status line says so ("Sent a crash
    report from the last session. Thanks."). With the switch off, nothing is sent and the record of
    the crash is deleted.
  - **Usage tracking**, off and greyed out. A line under it says Device collects no usage data, so
    there is nothing to turn on.
- A crash report holds the app version, the OS name and version, the CPU architecture, and what
  failed: for a panic in Device's Rust code, the panic message, where it happened and the backtrace;
  for a native crash (the engine, a plugin), the signal (macOS and Linux) or nothing (Windows, see
  Decisions). Paths in the user's home folder become `~`, and the user name never appears. No
  project, file name outside the app's own code, audio device, email or status line is sent.
- Settings has seven pages: General, Audio, MIDI, Plugins, Shortcuts, Downloads, Privacy. Account
  and Usage are gone, and General no longer shows the Sample licensing heading and switch.
- The switch is saved in the preferences. Self-test, `--state` and screenshot runs never record a
  crash, never read one and never send anything. `DSTUDIO_FEEDBACK_URL` points the report at
  another service, as for Report a bug.

## Requirements

- R1. Settings › Privacy shows "Send crash reports" (a working switch, on by default) and "Usage
  tracking" (off, disabled) with an explanation that Device collects no usage data.
- R2. Both rows have one sentence of help in the (i) and are in the settings search table.
- R3. "Send crash reports" is saved in the preferences; old preferences files load with it on.
- R4. A panic anywhere in the app writes a crash record (version, OS, architecture, message,
  location, backtrace) to `crash-report.json` next to the preferences before the process ends.
- R5. On macOS and Linux a fatal signal (SIGSEGV, SIGBUS, SIGILL, SIGFPE, SIGABRT) that wasn't
  caused by a recorded panic writes a crash record naming the signal, using only
  async-signal-safe calls, and then lets the crash go on as before.
- R6. At the next launch, a crash record is sent once through the feedback service as kind `crash`
  when the switch is on, and deleted either way; nothing is sent when the switch is off.
- R7. Nothing personal is sent: user home paths and the user name are replaced, and the report has
  no email, project data, audio device or recent activity.
- R8. Runs that don't use the preferences (self-test, `--state`, screenshots) never install the crash
  handlers, read a crash record or send one.
- R9. The feedback service accepts kind `crash` (labels `crash`, `from-app`) with the same limits.
- R10. Settings › Account and Usage and the "License samples when you use them" row are hidden, and
  the settings search table's page numbers follow the new page order.

## Acceptance checks

- A1 (R4, R7): unit test: a panic message and backtrace with home paths become a record with `~`
  and no user name; the record round-trips through the file.
- A2 (R6, R7, R9): unit tests: a record is taken from its file once (a broken one is removed too);
  its payload is kind `crash` with the service's fields, no email and no home path or user name.
  By hand: a launch with `DSTUDIO_FEEDBACK_URL` at a local server posts the record and removes it.
- A3 (R9): `node --test services/feedback/test/*.test.mjs`: `crash` reports validate, get the
  `[Crash]` title and the `crash` label.
- A4 (R3): prefs test: a file without the field loads with crash reports on; the switch round-trips.
- A5 (R1, R2, R10): self-test: Settings lists seven pages; Privacy shows both rows; the crash
  reports switch turns off and on and the preferences follow; Usage tracking can't be switched on;
  the (i) of Send crash reports shows its help.
- A6 (R10): unit test of settings search page indices; General has no licensing row.
- A7 (R5): manual check on macOS: `DSTUDIO_CRASH_TEST=segv` makes the app crash with SIGSEGV (exit
  status 139, so the crash still ends the process) and leaves a record naming the signal.
- A8 (R1): review state `review-settings-privacy`.

## Out of scope

- Collecting usage data of any kind.
- Native crash backtraces (symbolising a native stack safely needs a crash-handling library or an
  out-of-process monitor) and Windows native crash records.
- Asking before each crash report is sent: the switch is the consent, on by default as the roadmap
  says.
- Retrying a crash report that failed to send.

## Decisions

Agreed by the maintainer's standing instruction to proceed on recommendations:

- Native crashes record only the signal, from a handler that writes a pre-built path and fixed text
  with `open`/`write`/`close` and then restores the previous handler so the crash continues to the
  system's crash reporter. A backtrace from inside a signal handler is not async-signal-safe.
- Windows records panics only; an unhandled-exception filter is left for later.
- A panic is recorded even when only a worker thread panics, since that is a bug too.
- The report is sent once and the record deleted whether or not sending worked, so an offline
  machine doesn't send old crashes days later or keep retrying.
- Account, Usage and sample licensing are hidden in this change (Q3).

## Open questions

None.
