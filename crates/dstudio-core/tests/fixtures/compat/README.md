# Compatibility fixtures

Each folder holds a project saved by a released version of Device, using that release's own
`dstudio-core` (`persist::save` at the release tag), so the files are what users really have:

| Folder | Release tag | Commit | Schema |
|---|---|---|---|
| `v0.1.0` | v0.1.0 | 48472e89 | 1 |
| `v0.2.0` | v0.2.0 | d8fec528 | 1 |
| `v0.3.1` | v0.3.1 | 3e4e2f63 | 2 |
| `v0.4.0` | v0.4.0 | 14a53f50 | 2 |

**Never edit these files by hand, and never regenerate them from newer code.** They exist to prove
that old files keep opening. If a test fails on a fixture, fix the loader (`persist::load`,
`migrate`, `repair`), not the fixture.

Each folder has `Compat.dsproj/` (the project folder as saved, with its 480-frame
`media/click.wav`) and `expected.json`, the content the generator put in: tempo and time signature,
tempo changes (v0.3.1 on), 2 sections, an audio track with a faded clip, a MIDI track with 5 notes
and a Synth instrument, a group, a return with a send, automation lanes (track volume and a
built-in parameter), and Compressor, Delay and Reverb with non-default parameters.
`tests/compat.rs` checks all of it.

## Making fixtures for a new release

1. `git worktree add --detach ../wt-tag-vX.Y.Z vX.Y.Z` and use a separate `CARGO_TARGET_DIR`.
2. Copy `generator/gen_fixture.rs.txt` to `crates/dstudio-core/examples/gen_fixture.rs` in that
   worktree (do not commit it there). It is a reference: fill in `//TEMPOMAP` and `//MIDIFIELDS`
   and adapt whatever the release's API needs (older releases lack `SetTempoMap`, for example).
3. `cargo run -p dstudio-core --example gen_fixture <out-dir>`, copy `<out-dir>` to
   `tests/fixtures/compat/vX.Y.Z/`, add the version to `VERSIONS` in `tests/compat.rs`, and
   add a row above.

v0.3.0 has no fixture: v0.3.1 changed only the assistant module in `dstudio-core`, so its files are identical in format.
