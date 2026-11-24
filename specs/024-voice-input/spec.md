# Talk to the assistant (voice input)

Status: Implemented · Spec: 024 · Issue: none (roadmap G19)

## Problem

The assistant panel's mic button and the Settings › General › Voice rows (Talk key, Hold talk key
to record, Auto-send, Input, Language) were placeholders. A musician with their hands on a
controller or an instrument had to stop and type to ask the assistant anything. Recorded speech is
private: it must not leave the computer.

## Behaviour

1. Settings › Downloads lists **Speech recognition** (82 MB) with what it is for, its status and a
   **Download** button. While it downloads, the row shows progress and the button reads
   **Cancel**; once it is there, the button reads **Remove**. A failed download says why, under
   the row.
2. Until it is downloaded, clicking the mic opens Settings › Downloads, and the status bar says
   why.
3. With the model downloaded, clicking the mic (or pressing the talk key, Control and Option on
   macOS, Ctrl and Alt on Windows and Linux) starts listening. The mic turns red with a ring that
   follows the input level, and the message box says "Listening…".
4. Clicking again, pressing the talk key again, or 60 seconds, stops listening. With **Hold talk
   key to record** on, Device listens while the talk key is held and stops when it is released.
   The mic dims and the message box says "Recognising speech…" while Device turns the speech
   into text on this computer. The text goes into the message box, after anything already typed.
   With **Auto-send** on, it is sent as a message straight away instead.
5. **Language** is Automatic by default (the model recognises it), or one of a list of common
   languages. **Input** is the audio input Device listens to: Input 1 by default, or any input or
   stereo pair the audio device has, as in a track's In menu.
6. If nothing was heard, the message box is left as it was and the status bar says "Didn't catch
   that". If there is no audio input, it says to choose one in Settings › Audio.
7. Nothing is saved with the project. The Voice rows are saved in preferences.

## Requirements

- R1. Speech recognition runs on this computer with a downloadable open model (whisper.cpp, MIT;
  the multilingual "base" model, 8-bit). No audio or text leaves the computer for recognition.
- R2. The audio thread only copies the chosen input, after Mute input, into a ring allocated
  with the engine, without locks or allocation (principle 1). Conversion to 16 kHz and
  recognition run on a worker thread.
- R3. The model comes over HTTPS from a URL pinned to one revision, is checked against a pinned
  size and SHA-256 and kept next to the preferences; a failed, cancelled or corrupt download
  leaves nothing behind. It can be removed again.
- R4. The mic button, talk key, Hold talk key to record, Auto-send, Input and Language all work,
  and the Voice rows are saved in preferences and found by Settings search.
- R5. Recognising a short request takes a few seconds at most on a laptop CPU (benchmark in the
  pull request).
- R6. Works on macOS, Windows and Linux (principle 5). macOS asks for microphone access the first
  time input is used (the bundle's existing microphone permission string covers it).

## Acceptance checks

- A1 (R1): `voice::tests::transcribes_real_speech` runs the model on a public-domain speech
  recording, taken up to 44.1 kHz and back to 16 kHz first, with English and Automatic, and on
  silence. CI downloads the tiny model (pinned, hash-checked) so it runs on every platform.
- A2 (R2): engine test `voice_capture_reads_the_chosen_input_in_order` (mono, stereo pair, a
  missing channel, Mute input, off) and the voice tap in
  `record_and_monitor_do_not_allocate_on_the_audio_path`.
- A3 (R3): `voice::tests::download_keeps_a_good_file_and_removes_a_bad_one` against a local HTTP
  server: a good file is kept, a wrong hash and a cancel leave nothing.
- A4 (R4): self-test steps: the mic with no model opens Settings › Downloads; the mic and the
  talk key (hold) put the recognised text in the message box; the Voice switches and Language
  menu change the preferences; searching "model" finds the Downloads row. Unit tests cover the
  talk-key chord and Settings search.
- A5 (R5): timing of the base model on a 4.6 s recording in the pull request.
- A6: manual check on a Mac with a real microphone.

## Out of scope

- Talking back (text to speech), wake words, and always-on listening.
- Cloud speech services.
- Choosing another talk key (the reference shows a fixed chord).

## Decisions

1. On-device recognition with a downloadable model (Chris, 2026-11-24), not the operating
   system's dictation and not a cloud service.
2. The 8-bit "base" model: about 82 MB, multilingual, a few seconds for a short request on a
   laptop CPU. "tiny" is used only by the CI test.
3. Input sits in Settings › General › Voice (where the reference puts it), not Settings › Audio.
