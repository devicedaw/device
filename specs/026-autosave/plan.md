# Plan: Autosave and crash recovery

Spec: [spec.md](spec.md)

## Approach

All of it lives in the app (`crates/dstudio-app/src/autosave.rs`); core and the engine do not change.

A 5 s UI timer calls `Controller::autosave_tick`. When a backup is due, it syncs plugin states into the model (as Save
does), clones the project and hands the clone to a short-lived thread that runs `persist::save` and writes the note.
One write runs at a time; the next tick collects its result. Cloning costs about 1 ms for a 7 MB project, against 30
ms or more for the write (`benches/save.rs`).

Backups are named by process id, so a backup still on disk at launch from a process that is not running belongs to a
session that crashed. On Unix, `kill(pid, 0)` tells whether the process runs. Recovery renames the backup to this
process's id, opens it, removes it from Open Recent, points the project back at the original folder and marks it
edited (`Document::mark_edited`, added for plugin-window edits in S0.1).

Takes: the engine's take writer puts a fixed 58-byte header first and fills in its sizes at the end of the pass.
`repair_take` checks for exactly that header and rewrites the RIFF, `fact` and `data` sizes from the file's length.

Alternatives: backups beside the project would land in synced folders (and in version control), and an unsaved
project has no folder; a restore dialog adds a choice the unsaved state already gives.

## Principles check

- Real-time safety: nothing runs on the audio thread; the write runs on its own thread.
- One mutation path: recovery opens a project like Open does; nothing edits the model outside commands except the
  existing plugin-state sync.
- Projects keep loading: the backup is an ordinary project written by `persist::save`.
- Cross-platform: process checks are Unix only, with a safe fallback on Windows.

## Tests and evidence

- Tests: the five unit tests in `autosave.rs`.
- Benchmarks: `benches/save.rs` (dstudio-core).
- Screenshots: the only visible change is a status line message.

## Risks

- A write still running at quit: `autosave_finish` joins it before removing the backup.
- A backup removed while a write is running would come back: nothing is removed while a write is under way.
