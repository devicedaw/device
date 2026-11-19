# Tasks: Windows: small audio buffers

Plan: [plan.md](plan.md)

- [x] T1. `wasapi_period.hpp` and `test_wasapi_period.cpp`. Verify: ctest.
- [x] T2. `IAudioClient3` path in `wasapi_device.cpp`. Verify: every output × buffer size on a
  real sound card, 0 xruns; duplex record 10 s at 0, 128 and 256, 0 overruns.
- [x] T3. Settings › Audio note. Verify: `cargo test -p dstudio-app buffer_note`.
- [x] T4. Docs: README, engine/README.md, docs/building.md. Verify: read against the change.
- [x] T5. Evidence for the pull request. Verify: `spec-verify`.
