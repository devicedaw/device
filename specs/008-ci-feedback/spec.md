# Faster CI feedback

Status: Done · Spec: 008 · Issue: none

## Problem

Device contributors wait for cross-platform CI after each push. In one pull request the macOS app
job started 25 minutes into the run, and required CI finished at 45 minutes. The advisory macOS
benchmark job on another pull request also started about 24 minutes into its run. On a successful
main push, the full Windows app self-test took 47 minutes and CI finished at 82 minutes.

- [Pull request CI sample](https://github.com/devicedaw/device/actions/runs/44856571295)
- [Pull request benchmark sample](https://github.com/devicedaw/device/actions/runs/44861682819)
- [Main push CI sample](https://github.com/devicedaw/device/actions/runs/44860588181)

## Behaviour

Routine pull requests keep their cross-platform build and test checks. Changes that affect the
benchmarks keep the required Linux Callgrind comparison. Contributors can request advisory native
timing on all three platforms when investigating a performance-sensitive change.

Main pushes keep the Windows app build and screenshot. The full Windows WARP self-test runs
nightly and can be requested for a pull request or run manually against the release candidate
before publishing.

## Requirements

- R1. Preserve the existing required check names, change detection, coverage and failure
  behaviour for ordinary pull requests.
- R2. Run native timing on eligible main pushes and manual Performance runs. Run it on a pull
  request when advisory timing is explicitly requested. Keep instruction-regression acceptance
  independent of the timing request.
- R3. Run the Windows release build and screenshot on main pushes. Run the full Windows self-test
  on scheduled and manual CI runs and on app-affecting pull requests that opt in. A failed full
  test must fail its job.
- R4. Document how to request the advisory timing and Windows self-test, and how to run the
  full Windows test against a release candidate.
- R5. Compare time to required checks and time to workflow completion separately. Record runner
  start delays independently of job runtime.

## Acceptance checks

- A1 (R1, R2). Verify app-only and benchmark-affecting PRs, opt-in timing, regression acceptance,
  an eligible main push and a manual Performance run against the workflow conditions. Confirm that
  timing remains advisory and the Callgrind gate still reports on every PR.
- A2 (R1, R3). Verify the Windows job conditions for main pushes, scheduled and manual runs,
  and app-affecting PRs with and without the opt-in. Confirm that the existing failure and
  artifact handling applies when the full test runs.
- A3 (R4). Check the contributor and release instructions against the workflow triggers.
- A4 (R5). Record before and after job timings from hosted runs when available. Report pending
  hosted evidence explicitly and avoid treating any single queue delay as a repeatable saving.

## Out of scope

Shortening the app smoke suite and changing build caches are follow-up work. Measure the
remaining slow steps after changing the workflow triggers.

## Open questions

None. The release procedure uses a manual CI run against the candidate.
