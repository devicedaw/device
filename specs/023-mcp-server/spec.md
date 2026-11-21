# Control Device from outside AI apps (MCP server)

Status: Implemented · Spec: 023 · Issue: none

## Problem

The assistant panel only works with an Anthropic API key. Many musicians already pay for an AI
app (Claude Desktop, ChatGPT, or a coding tool such as Claude Code or Cursor) and would rather use
it than create an API account and pay per request. Those apps talk to other programs through the
Model Context Protocol (MCP), and Device has no MCP server, so they cannot see or change a project.

## Behaviour

1. Settings › General › Assistant has a switch, **Let AI apps control Device**, off by default.
   Beside it, **Copy setup** copies a ready-made config snippet for Claude Desktop and similar apps,
   and a line under it says whether the server is on and which app used it last.
2. With the switch on, the user adds Device to their AI app once, using the copied snippet. The
   app starts `device-studio mcp` (the same executable, a small relay mode with no window), which
   connects to the running Device on this computer. The first time an app makes a call, Device
   asks **Allow <app>?** (Allow / Don't Allow). Allow is remembered in preferences; Don't Allow
   lasts until Device quits.
3. The user asks their AI app for something ("add a reverb to the vocal and write a four-bar
   bassline"). The app calls Device's tools. Device applies the change exactly as an assistant
   turn: validated first, nothing applied if any edit is invalid, and one undo step labelled
   **AI App Edit**. The assistant panel's action log is not involved; the status bar briefly says
   which edits were made.
4. If Device isn't running, or the switch is off, the tools answer with a plain error ("Open
   Device and turn on Settings › General › Let AI apps control Device").
5. Nothing is saved with the project. The switch is saved in preferences.

## Requirements

- R1. The server is off until the user turns it on; turning it off closes any connection at once.
- R2. It accepts connections from this computer only (loopback), and only from a client that
  presents the per-install secret written next to the preferences file (0600 on macOS and Linux).
- R3. Tools: `get_project` (the same state the assistant panel sends, plus `next_id_hint`),
  `get_builtin_devices` (the built-in catalogue), `apply_actions` (an `AssistantTurn`'s
  `actions`, validated by `assistant::plan`, committed as one undo step), `undo`, `redo`, and
  `transport` (play, stop). Tool descriptions reuse the assistant's schema text.
- R4. Every edit goes through `Document::execute` (principle 2); an invalid turn changes nothing and
  returns the validation error so the AI app can retry.
- R5. Requests are handled on the UI thread between frames; the audio thread is untouched
  (principle 1). A request waits at most 120 s (long enough to answer the Allow prompt) and then
  fails.
- R6. `device-studio mcp` speaks MCP over stdio (JSON-RPC 2.0, `initialize`, `tools/list`,
  `tools/call`) and relays to the app; it exits when its stdin closes.
- R7. Works on macOS, Windows and Linux (principle 5).
- R8. While Device is recording (audio or MIDI), edits are refused with a clear error instead of
  interleaving with the take.

## Acceptance checks

- A1 (R3, R4): the assistant's core tests cover `plan`/`commit` (one undo step, an invalid turn
  changes nothing).
- A2 (R1, R2, R6): `mcp` unit tests: the relay answers `initialize`/`tools/list` without the app,
  turns an app error into an MCP tool error, and with no app running gives the "turn it on" error;
  the listener refuses a wrong secret, forwards a call with the right one, and writes the
  discovery file 0600 and removes it when stopped.
- A3 (R3, R5): a scripted MCP client against the running app (Linux, under Xvfb) added an
  instrument track with an 8-note bassline, set volume and tempo, added a Reverb, had an invalid
  batch and a malformed action refused with nothing changed, and undid and redid the edit as one
  **AI App Edit** step.
- A4 (R7): CI builds and runs A2 on all three platforms.
- A5: manual check with Claude Desktop on the Mac mini: a real prompt adds a MIDI clip.

## Out of scope

- ChatGPT. It only connects to MCP servers on the internet over HTTPS, not to programs on the
  user's computer, so it would need a hosted relay or a tunnel. That is a separate decision.
- Remote control from another computer, and any account or cloud service.
- Letting the AI app hear audio, read audio files, or change preferences or plugins it hasn't been
  shown.

## Decisions

1. The switch sits in Settings › General, in the Assistant group, so search finds it with the
   assistant's settings.
2. Device asks "Allow <app>?" once per app and remembers Allow in preferences.
3. The undo label is **AI App Edit**, so it reads differently from the panel's **Assistant Edit**.
4. ChatGPT is left out for now (see Out of scope).
