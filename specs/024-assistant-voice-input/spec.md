# Assistant voice input

Status: Draft · Spec: 024 · Issue: roadmap G19 (specs/010-parity-roadmap)

## Problem

The assistant only takes typed prompts. The mic button in the composer says "Voice input (not implemented)", the
General › Voice rows (Talk key, Hold talk key to record, Auto-send, Input, Language) are greyed out, and Settings ›
Audio has no way to pick the microphone the assistant listens to. A producer with both hands on a controller or an
instrument has to stop playing to type a question.

Speech has to become text somewhere. Today the only thing that leaves the machine is a sent prompt (with the project
state) going to the Anthropic Messages API. Voice input must not add a new way for the user's voice or room sound to
leave the machine without them knowing.

## Behaviour

- **Mic button.** Click it to start listening; the button turns red and shows a level ring; click again (or press
  Escape) to stop. While listening, the composer shows "Listening…" and then the words as they are recognised.
- **Talk key.** A key chord (default ⌃⌥, as the placeholder shows) does the same from anywhere in the window. With
  **Hold talk key to record** on (default), listening lasts while the chord is held; with it off, one press starts and
  the next press stops. The chord is set by clicking the badges and pressing new keys, as on the Shortcuts page.
- **Auto-send.** Off (default): the text lands in the composer for the user to edit and send. On: the text is sent
  as soon as listening stops. Empty or unrecognised speech never sends; the composer says "Didn't catch that".
- **Language.** "Automatic" (default) or a fixed language from the model's list.
- **Input.** The channel to listen to on the voice device: "Input 1", "Input 2"… or "Mix of all inputs" (default).
- **Settings › Audio › Assistant voice input.** A device row under Input device. "Same as input device" (default)
  follows the recording input; any other input device can be chosen. "Mute audio input" does not silence voice input.
- **First use.** Speech is recognised on this computer by a model that is not shipped in the installer. Until it is
  downloaded, the mic button's tooltip reads "Download the voice model to talk to the assistant", and clicking it
  opens Settings › Downloads at the "Assistant voice model" row (size shown, Download button, progress).
- **Privacy.** Audio is held in memory only for the length of one utterance, turned into text on this computer, and
  then discarded. It is never written to disk, never added to the project, never sent anywhere. The only thing that
  leaves the machine is the text, and only when it is sent as a prompt, exactly as if it had been typed. The
  General › Voice section says this in one line under the heading.
- Voice input works while the transport plays or records. It never writes to a track; a track armed on the same
  device still records whatever the microphone hears, as it does today.
- Nothing here is a project edit: no undo step, nothing saved in the project. The settings are app preferences.

### Speech-to-text options

| Option | Privacy | Quality | Size | Platforms | Licence |
|---|---|---|---|---|---|
| A. Platform APIs (macOS Speech framework in on-device mode; Windows.Media.SpeechRecognition) | On-device on macOS when on-device mode is forced; on Windows free dictation needs the system's online speech setting, which sends audio to Microsoft (to confirm in the plan) | Good on macOS; modest on Windows | 0 MB (system) | macOS, Windows; nothing built in on Linux | System APIs |
| B. Bundled open model (whisper.cpp with Whisper weights) | Fully on-device everywhere | Good to very good; handles music jargon and many languages | 75 MB (tiny) to 470 MB (small) per model, downloaded on demand | All three, one code path | MIT (code and weights) |
| C. Cloud speech service | Audio leaves the machine on every utterance | Very good | 0 MB | All three | Commercial terms; a second account and key |
| D. Anthropic provider | Not possible today: the Messages API we use does not take audio input | — | — | — | — |

Recommendation: **B**, one engine on every platform, with the model as an optional download (G15). It is the only
option that is private by default on all three platforms and behaves the same everywhere, which keeps tests and
support simple. A is a possible later addition on macOS to skip the download; C is out of scope.

## Requirements

- R1. The mic button and the talk key start and stop listening as described, in both hold and toggle modes; Escape
  cancels and discards.
- R2. Recognised text goes into the composer; with Auto-send on it is sent when listening stops. Empty results never
  send.
- R3. Speech is recognised on this computer. No audio and no text is sent anywhere by voice input itself; audio is
  never written to disk and is discarded after recognition.
- R4. Settings › Audio has an "Assistant voice input" device row, default "Same as input device", which follows a
  change of the input device. A missing or unplugged device falls back to the default with a status line.
- R5. General › Voice rows are enabled and saved as app preferences: Talk key, Hold talk key to record, Auto-send,
  Input, Language. Settings search finds them.
- R6. Without the voice model, the mic button explains the download and opens Settings › Downloads; the model can be
  downloaded, re-downloaded and removed there.
- R7. Capturing voice audio follows the real-time rules: the audio thread only copies frames into a pre-allocated
  lock-free buffer; recognition runs on a worker thread. Playback and recording are unaffected.
- R8. Recognition of a 10 s utterance should finish within 2 s on the slowest supported machine with the default
  model.

## Acceptance checks

- A1 (R1, R2, R5): self-test drives the mic button and the talk key (hold and toggle) with a TEST recogniser that
  returns canned text, as the assistant's fixture provider does, and checks composer text, Auto-send and Escape.
- A2 (R3): a recogniser test feeds a short spoken WAV fixture to the real model (tiny, fetched in CI) and checks the
  words; a test asserts that the voice path opens no network connection and creates no file.
- A3 (R4): settings test: default follows the input device; an unplugged device falls back; review-state screenshot
  of Settings › Audio.
- A4 (R6): review-state screenshots of the mic tooltip without a model and of the Downloads row.
- A5 (R7): engine test that the capture tap allocates nothing (`test_rt_safety.cpp`), and the callback time in
  `dstudio_engine_bench` with the tap on and off.
- A6 (R8): bench of recognition time per model on the CI machines, printed in the pull request.

## Out of scope

- Speaking replies aloud (text to speech).
- Wake words or always-on listening.
- Voice commands that bypass the assistant (for example "play", "stop").
- A cloud speech option.

## Open questions

- Q1. Which engine? Recommendation: whisper.cpp everywhere (option B), platform APIs maybe later on macOS only.
- Q2. Default model size? Recommendation: "base" multilingual (about 140 MB) as the default download, with "small"
  offered for better accuracy; "tiny" only for CI.
- Q3. Does voice input depend on the Downloads page (G15) landing first? Recommendation: yes; build G15 first, then
  this spec, rather than a one-off download dialog.
- Q4. General › Voice › Input: channel on the voice device, or a second copy of the device picker? Recommendation:
  the channel, so the device is chosen in one place (Settings › Audio).
- Q5. Should "Mute audio input" also mute voice input? Recommendation: no; pressing the mic is an explicit request.
- Q6. Where does whisper.cpp build: inside `engine/` (warnings as errors on three compilers) or as a separate Rust
  crate with its own C++ build? Recommendation: a separate crate outside the engine, since it is not audio-thread code;
  list it in `engine/third_party_licenses.md`.
