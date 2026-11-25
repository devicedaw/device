# Autosave and crash recovery

Status: Done · Spec: 026 · Issue: none

Roadmap item S0.3 in [025](../025-song-ready-roadmap/spec.md).

## Problem

Nothing is written between manual saves. A crash, including a plugin crash, which takes the whole app down, loses
everything since the last Save. A take being recorded at the time is worse off: the engine writes the audio as it
arrives but fills in the WAV header's sizes only when the pass ends, so the file on disk reads as empty.

## Behaviour

- While the project has unsaved changes, Device writes a backup of it every 2 minutes, counted from the first unsaved
  change and then from the last backup. Nothing is written while the project has no unsaved changes, or while audio
  is being recorded (the take joins the project when the pass ends).
- The backup goes away once there is nothing unsaved (the project was saved, or another one was opened) and when
  Device quits normally.
- After a crash, the next launch opens the backup instead of the last session, as unsaved changes to the original
  project. The status line says "Device quit unexpectedly. Recovered your unsaved changes from 4 minutes ago: save to
  keep them." Save writes to the original project folder (or asks where, for a project that was never saved).
  Closing without saving is how to decline: Don't Save throws the backup away.
- If a recording pass was running, the takes it was writing get a header that matches the audio that reached the disk,
  and the status line says where they are, so they can be dragged in.

## Requirements

- R1. A backup is written at most every 2 minutes while the project has unsaved changes, with the same atomic write as
  Save, and without holding up the UI or audio threads for the write itself.
- R2. Backups go to `<preferences folder>/Autosave/<process id>.dsproj`, with `<process id>.json` saying which
  project it belongs to and when it was written. They are never written into the project folder.
- R3. The backup is removed once nothing is unsaved and at a normal quit.
- R4. A launch finds the newest backup from a process that is no longer running, opens it as unsaved changes to the
  original project and takes it over (so a crash of this session does not lose it). Two copies of Device running at
  once never take each other's backups (on macOS and Linux; on Windows a second copy's backup would be offered as
  recovered, which loses nothing).
- R5. A recording pass notes its takes' folder in `<process id>.takes.json` while it runs. A launch repairs the
  header of every take there that a dead process left unfinished, dropping only a partial last frame.
- R6. Self-tests and screenshot runs never autosave or recover.

## Acceptance checks

- A1 (R1, R3): unit test `unsaved_changes_are_backed_up_and_the_backup_goes_once_saved`.
- A2 (R4): unit tests `a_backup_from_a_finished_process_is_recovered_and_taken_over` and
  `a_crashed_sessions_changes_open_unsaved_and_go_when_declined` (restore, then decline).
- A3 (R5): unit tests `a_take_cut_short_gets_a_header_that_matches_its_audio` and
  `the_next_launch_repairs_the_takes_of_a_pass_that_never_ended`.
- A4 (R1): `cargo bench -p dstudio-core --bench save`: what a backup costs the UI thread (the clone) next to the write
  it hands to its own thread.

## Out of scope

- Adding a cut-short take to the project as a clip (which track and where is not known after a crash).
- Keeping more than one backup, or a history of backups.
- An "offer to restore" dialog: the recovered changes open unsaved, so Save and Don't Save already are the choice.

## Open questions

None.
