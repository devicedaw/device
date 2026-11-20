# Update check and the "Update available" pill

Status: Building · Spec: 022 · Issue: none · Roadmap: [010 › G16](../010-parity-roadmap/spec.md)

## Problem

Device ships as GitHub releases, but a running copy never learns that a newer one exists. Options › Check for
Updates… only says "There's no update server yet, so new versions arrive as new builds", so people keep running old
versions with bugs that are already fixed, and the only way to find out is to visit the releases page.

## Behaviour

- **At launch**, once, Device asks GitHub for the latest published release in the background. Nothing is shown while
  it runs, and nothing at all when the computer is off-line, GitHub is unreachable or GitHub is limiting requests.
- When the latest release is newer than the running version, a blue **Update available** pill appears in the title
  bar, left of Report a bug, with the new version next to the words ("Update available  v0.3.0"). It stays until
  Device quits.
- Clicking the pill opens the Check for Updates dialog showing the update: "Device 0.3.0 is available. You're running
  v0.2.0.", up to three lines from the release notes (the first items of the changelog section, shortened), "and N
  more changes" when there are more, and two buttons: **Later** closes the dialog, **Download** opens the release's
  page in the web browser (it lists the packages for every system) and closes the dialog.
- **Options › Check for Updates…** opens the same dialog and checks again, with a "Checking for updates…" line while it
  runs. It then shows the update as above, "You're running Device v0.2.0, the latest version.", or why the check
  failed in plain words: no connection, GitHub limiting requests (with roughly how long to wait), or a server error.
  When an update is already known, the dialog shows it at once without asking GitHub again.
- **Settings › General › Startup › Check for updates at launch** (on by default) turns the launch check off. The menu
  item always works.
- The launch check asks GitHub at most once every 12 hours: in between, Device reuses the answer it saved last time
  (so the pill still appears), and asks again at the next launch after that. A failed check saves nothing.
- Only full releases are offered. Pre-releases (`-alpha.N`, `-beta.N`, `-rc.N`) and drafts never are, and versions
  compare as Semantic Versioning does (`0.10.0` is newer than `0.9.0`; `0.3.0` is newer than `0.3.0-rc.1`).
- Nothing about the user or the computer is sent: the request carries `User-Agent: Device/<version>` and GitHub's
  API headers, no cookies, identifiers or settings.
- Nothing in the project changes; there is no undo step. The setting is saved in the preferences.

## Requirements

- R1. At launch, when the setting is on, Device checks the latest GitHub release on a worker thread; the UI never
  waits on the network.
- R2. A newer full release shows the pill left of Report a bug with its version; an equal, older, pre-release or draft
  release shows nothing.
- R3. Versions compare by Semantic Versioning, including pre-release order; an unreadable tag is never newer.
- R4. Launch-check failures (off-line, timeouts, rate limiting, server errors, an unexpected answer) show nothing.
- R5. Options › Check for Updates… opens the dialog, shows "Checking for updates…" while the check runs, then the
  update, "the latest version", or a plain-language reason for a failure (including how long to wait when GitHub is
  rate limiting).
- R6. The pill and the menu open the same dialog; it shows the new version, the running version and a short summary
  of the release notes; Download opens the release page in the default browser; Later closes it.
- R7. A launch check within 12 hours of the last successful one reuses that answer instead of asking GitHub.
- R8. Settings › General has "Check for updates at launch", on by default, saved in the preferences; old preference
  files load with it on.
- R9. The request sends only a generic `Device/<version>` User-Agent and GitHub's Accept and API-version headers,
  only over HTTPS.
- R10. Self-tests and `--state` review runs never use the network or open a browser.

## Acceptance checks

- A1 (R3): unit test of the version comparison: patch, minor, major, `v` prefix, pre-release order, a release beating
  its release candidates, an unreadable tag.
- A2 (R2, R4, R5, R9): unit tests against a local HTTP server: a newer release is read with its notes summary; no
  release (404) is "nothing to offer"; a refused connection gives the off-line message; a 403 with
  `x-ratelimit-remaining: 0` gives the rate-limit message with minutes; the server sees `User-Agent: Device/<version>`
  and no other identifying header; plain HTTP to another host is refused.
- A3 (R2): unit test: a release flagged as a pre-release, or tagged with a pre-release version, is not offered.
- A4 (R6): unit test of the release-notes summary on the shape `changelog-notes.sh` produces plus GitHub's generated
  "What's Changed" list.
- A5 (R7): unit test: a second launch check within 12 hours is answered from the saved file without a request; a menu
  check always asks.
- A6 (R1, R2, R5, R6, R8, R10): self-test: Options › Check for Updates… says the running version is the latest;
  a launch check that finds a newer version shows the pill; clicking it opens the dialog with the version and notes;
  Download "opens" the page (recorded, not opened) and closes the dialog; with the setting off, a launch check shows
  nothing.
- A7 (R2, R6, R8): review-state screenshots: `review-update-available` (the pill), `review-update-dialog` (the dialog
  with an update), `review-check-updates` (up to date) and `reference-settings-general` (the new switch).

## Out of scope

- **In-app install** (downloading the package, checking it against `SHA256SUMS.txt` and the macOS signature, staging
  it and restarting into the new version). A draft exists (branch `chrislaupama/device-update-from-app`), but none of
  it has run against a real release, and its Windows (silent Inno Setup upgrade) and Linux (`install.sh` into the same
  prefix) paths have never run at all. It is a follow-up that needs an end-to-end test against a real release on each
  platform before it ships. Until then, Download opens the release page.
- Release channels (offering pre-releases to people who opt in), skipping a version, and reminders.
- Checking for updates to plugins or downloadable content.

## Open questions

None. Decided while building (the maintainer asked to proceed on recommendations):

- The pill opens the dialog rather than the browser, so the user sees what changed before leaving the app; Download
  in the dialog opens the page. One more click, no surprise browser window.
- Settings › General has room for one more switch under Startup, so the launch check can be turned off there.
- The 12-hour reuse of the launch answer keeps launches well inside GitHub's 60 unauthenticated requests per hour
  and avoids a request on every quick restart; the menu always asks GitHub.
- The self-install half of the draft is left out (see Out of scope).
