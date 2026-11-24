# Talk to the assistant (voice input)

Status: Draft · Spec: 024 · Issue: none (roadmap G19)

## Problem

The assistant panel's mic button, the Settings › General › Voice rows (Talk key, Hold talk key to
record, Auto-send, Language) and the missing Settings › Audio "Assistant voice input" row are
placeholders. A musician with their hands on a controller or an instrument has to stop and type to
ask the assistant anything. Recorded speech is private: it must not leave the computer unless the
user has chosen a service that does that, and Device has no such service today.

## Behaviour

1. Settings › Downloads lists **Speech recognition** (about 150 MB, English and other languages)
   with a Download button and progress. Until it is downloaded, the mic button's tooltip says
   "Download speech recognition in Settings › Downloads" and clicking it opens that page.
2. With the model downloaded, clicking the mic button (or pressing the talk key, Right Option on
   macOS and Right Alt on Windows and Linux by default) starts listening. The button turns into a
   red disc with a level ring, and the composer shows "Listening…".
3. Clicking again, pressing the talk key again, or after 60 seconds, stops listening. With **Hold
   talk key to record** on, listening lasts while the key is held. Device turns the speech into
   text on this computer and puts it in the composer. With **Auto-send** on, the text is sent as a
   turn straight away; otherwise the user can edit it first.
4. **Language** is Automatic by default, or one of the model's languages.
5. Settings › Audio has an **Assistant voice input** row: "Same as input device" by default, or
   any input the audio device has (input 1, input 2, 1/2).
6. If nothing was heard, the composer is left as it was and the status bar says "Didn't catch
   that". Nothing is saved with the project; the Voice rows are saved in preferences.

## Requirements

- R1. Speech recognition runs on this computer with a downloadable open model (whisper.cpp, MIT,
  multilingual "base" model). No audio or text leaves the computer for recognition.
- R2. The audio thread only copies input samples into a preallocated lock-free ring while
  listening (principle 1); resampling to 16 kHz and recognition run on a worker thread.
- R3. The model is downloaded over HTTPS from a pinned URL, checked against a pinned SHA-256, and
  stored next to the preferences; a failed or corrupt download leaves nothing behind. Downloads
  can be removed again.
- R4. The mic button, talk key, Hold talk key to record, Auto-send, Language and Assistant voice
  input rows all work and are saved in preferences.
- R5. Recognising 10 s of speech takes under 3 s on the CI machines' class of CPU (benchmark).
- R6. Works on macOS, Windows and Linux (principle 5). macOS asks for microphone access the first
  time (the existing input-device permission string covers it).

## Acceptance checks

- A1 (R1): a unit test transcribes a short bundled speech clip (a few seconds, generated with a
  system voice and checked in under tests/fixtures) with the tiny model in CI, and the text
  contains the expected words.
- A2 (R2): an engine test enables voice capture, feeds a test signal through the offline input
  path and reads it back unchanged; the realtime-allocation guard stays clean.
- A3 (R3): download tests against a local HTTP server: good file is kept, wrong hash is removed.
- A4 (R4): self-test steps toggle each Voice row and check prefs; clicking the mic with no model
  opens Settings › Downloads.
- A5 (R5): `cargo bench -p dstudio-app --bench voice` number in the PR.
- A6: manual check on a Mac with a real microphone.

## Out of scope

- Talking back (text to speech), wake words, and always-on listening.
- Cloud speech services. They could be added later as an opt-in choice.

## Open questions

1. Local model (this spec), the operating system's dictation, or leave voice input for later?
