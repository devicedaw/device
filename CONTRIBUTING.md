# Contributing to Device

Thanks for your interest. **Device is not accepting outside contributions yet.** The code is open to read,
build and fork under the [licence](LICENSE), but issues, pull requests and comments are limited to the maintainer for
now while the project is pre-alpha and changing quickly. This page will say when that changes. Security problems can
still be reported privately at any time through [SECURITY.md](SECURITY.md).

The rest of this guide describes how contributions will work once they open.

By taking part you agree to follow the [Code of Conduct](CODE_OF_CONDUCT.md). To report a security problem, follow
[SECURITY.md](SECURITY.md) instead of opening a public issue.

## Before you start

- **Bugs and ideas:** open an issue first using one of the templates. For anything larger than a small fix, wait for a
  reply before writing code, so we can agree on the approach and you don't spend time on something that won't be
  merged.
- **Read the docs:** [docs/architecture.md](docs/architecture.md) explains how the Rust app, the Rust core and the C++
  engine fit together, [docs/adr/](docs/adr/) records the main decisions, and [docs/devices.md](docs/devices.md)
  explains how a built-in device is defined and how to add one.

## Building and testing

[docs/building.md](docs/building.md) has the requirements and packages for macOS, Windows and Linux. In short:

```sh
cargo run --release -p dstudio-app -- --demo      # build the engine and the app, open a demo project
cargo test --workspace                            # core, FFI and end-to-end tests
cd engine && cmake --preset default && cmake --build --preset default && ctest --preset default
cargo run --release -p dstudio-app -- --self-test --no-audio   # drive the real UI through its checks
```

CI runs the tests and the self-test (not the interactive `--demo`) on macOS, Windows and Linux. A pull request runs the
parts its changes reach: documentation-only changes run none, engine-test-only changes skip the Rust tests and the app,
and anything CI can't classify runs everything (the rules are in docs/building.md).
Its runners have no audio device, so the self-test runs with `--no-audio` (Linux also runs it against a headless
PipeWire server) and anything that needs audio hardware has to be tested by hand. The Windows self-test takes about
30 minutes under the software renderer, so pull requests that change the app run it only with the `windows-self-test` label; main and
the nightly run always run it, and a failure fails the job. The engine is built with warnings as errors on
clang, MSVC and GCC, so please build on more than one compiler if you can, or check the CI result.

## Pull requests

- **Keep each pull request to one change** and rebase it on the latest `main`.
- **Tests:** a bug fix comes with a few small tests that would have caught it. New behaviour comes with tests too. We
  prefer a handful of well-aimed tests to a large number of shallow ones.
- **Screenshots:** any visible change includes before and after screenshots in the description. The app can render
  itself: `cargo run --release -p dstudio-app -- --no-audio --state reference-default --screenshot out.png --exit-after-frames 5`.
  Attach the images to the description (drag them into it on GitHub) rather than committing them. The maintainer
  keeps pull request media on the `pr-screenshots` branch instead; see [docs/pr-media.md](docs/pr-media.md).
- **Performance:** anything that could affect performance (especially the audio thread) includes benchmark results
  before and after. Extend the existing benchmarks (`engine/bench`, `crates/*/benches`) if none cover your change. The
  audio thread must never allocate, lock or wait.
- **Changelog:** a change users would notice adds a line under Unreleased in [CHANGELOG.md](CHANGELOG.md).
- **Docs:** keep [README.md](README.md) and `docs/` accurate when your change affects what they describe (see README ›
  Development › Keeping this README up to date).
- **Original work only.** Do not paste code, artwork, icons, fonts, presets or samples from other products, and only add
  third-party libraries whose licences are compatible with ours. New third-party code is listed in
  [engine/third_party_licenses.md](engine/third_party_licenses.md) with its licence.

## Code style

- Rust: `cargo fmt --all` (settings in `rustfmt.toml`) and `cargo clippy --workspace --all-targets -- -D warnings`. CI checks both.
- C++: C++23, follow the style of the surrounding code, no exceptions or allocation on the audio thread.
- Internal names use `dstudio` (crates, namespaces, `DSTUDIO_*` options). Do not use bare `device` as an identifier
  prefix: it already names effects and instruments throughout the code.
- Write comments that explain why, in plain sentences.

## Licence of contributions

Unless you say otherwise, anything you submit for inclusion is licensed under the same licence as the project (see
[LICENSE](LICENSE)).
