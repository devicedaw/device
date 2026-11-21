# CI feedback plan

Keep the existing CI jobs and required check names. In `ci.yml`, restrict the full Windows
WARP step to nightly and manual runs and opted-in PRs. Use manual CI on the release candidate
before publishing. This keeps the long test out of routine main pushes.

In `perf.yml`, retain the benchmark path check and Callgrind gate. Run native timing on eligible
main pushes, manual runs and eligible PRs carrying `perf-timing`. Adding that label requests timing
immediately. Other label events still refresh the independent instruction gate.

Update the CI and release instructions in `docs/building.md`, the README overview and PROGRESS.
Check workflow syntax and event conditions. Use GitHub job start and end timestamps for wall time
and queue comparisons. Application runtime, persistence, ABI and visuals are outside this change.
Cross-platform build coverage stays in the existing matrix.
