# The assistant listens to audio

Status: Draft · Spec: 026 · Issue: roadmap G20 (specs/010-parity-roadmap)

## Problem

The assistant sees the project as data: tracks, clips, devices, volumes, tempo. It cannot tell whether a mix is too
quiet for streaming, whether a vocal is harsh, or what key a loop is in, so questions like "why does my kick
disappear?" or "what key is this sample in?" get generic answers. General › Assistant › "Let the assistant listen to
audio" is shown greyed out.

The assistant runs on a remote provider. Anything we add to a turn leaves the machine, so this must be opt-in and the
user must know exactly what is sent.

## Behaviour

- **The switch.** General › Assistant › "Let the assistant listen to audio" becomes a working switch, **off by
  default**, saved as an app preference. Help text under it: "Adds a short text summary of the sound (loudness, tone,
  key, tempo) to each message. Audio itself is never sent."
- **What is analysed.** When the switch is on and a message is sent (Assist or Teach mode):
  - with a clip selected (or set as the context chip by Ask, G17): the clip's track output over the clip's range,
    as the user hears it, effects included;
  - otherwise: the master output over the loop range if the loop is on, else over the whole arrangement, up to the
    first 10 minutes.
- **When.** Analysis runs in the background when the message is sent; the composer shows "Listening to the clip…" or
  "Listening to the mix…" with a Cancel. Results are cached until the project or the selection changes, so follow-up
  messages are instant. If analysis fails or takes over 15 s, the message is sent without it and a status line says so.
- **What the user sees.** A small "Audio summary attached" chip on the sent message; clicking it shows the exact text
  that was added.
- **Silence and short audio.** A silent target attaches `"silent": true` only. Under 2 s, key and tempo are left out.
  Guesses below a confidence threshold are left out rather than sent as noise.
- Nothing is a project edit: no undo step, nothing saved in the project, no file written.

### What exists today and what is new

| Measure | Today | New work |
|---|---|---|
| Sample peak | `se_asset_compute_peaks` (per asset, waveform drawing); live meter peaks | Peak over a rendered buffer (trivial) |
| True peak | `TruePeak4x` inside the limiters (`engine/src/devices/dsp_dynamics.hpp`), not exposed | Offline true peak over a buffer |
| Loudness (LUFS) | None | Integrated and short-term max loudness (ITU-R BS.1770 K-weighting and gating) |
| Spectrum | Live 128-bin analyser of one built-in device's window (`se_builtin_spectrum`), only while that window shows it | Offline average spectrum over a buffer, summarised into six bands and a tilt |
| Tempo | `se_asset_detect_onsets` plus `dstudio_core::warp::estimate_bpm` (warp markers) | Reuse on the rendered buffer; add a confidence |
| Key | None (`pitch_detect.hpp` is monophonic pitch for Tuner and Pitch Corrector) | Chroma over the buffer matched against major and minor key profiles |
| Rendering a clip or the mix | Private offline engine on a worker (`dstudio_engine::offline`, used by Bounce, Consolidate and Stems) | Reuse; render to memory instead of a WAV |

### What is added to the prompt

One extra block in the user message, after "Current selection", only when the switch is on:

```
Audio analysis (JSON):
{"target":"clip 12 \"Vox take 3\" on track 3","seconds":8.4,
 "integrated_lufs":-17.8,"short_term_max_lufs":-14.1,"sample_peak_dbfs":-1.2,"true_peak_dbtp":-0.9,
 "bands_db":{"sub_20_60":-38,"low_60_250":-24,"low_mid_250_1k":-21,"mid_1k_4k":-19,"high_mid_4k_10k":-26,"air_10k_20k":-41},
 "tilt_db_per_octave":-3.6,"key":{"guess":"F minor","confidence":0.71},"tempo":{"bpm":122.0,"confidence":0.64}}
```

About 400 bytes. The system prompt gains one sentence saying what the fields mean and that they are estimates. The
block is not kept in the conversation history; each turn carries the summary for its own target.

## Requirements

- R1. The switch works, is off by default, and is saved as an app preference; old preference files load with it off.
- R2. With the switch off, the request body is byte-identical to today's for the same prompt and project.
- R3. With it on, each turn carries exactly the block above for the target chosen as described, and nothing else
  new: no samples, waveform, spectrum frames or file paths.
- R4. Integrated loudness is within 0.1 LU, and true peak within 0.2 dB, of the reference values for standard
  loudness test signals.
- R5. Key and tempo guesses come with a confidence; low confidence, short or silent targets leave them out.
- R6. Analysis runs off the UI and audio threads, can be cancelled, is cached per target and project revision, and
  never delays sending by more than 15 s.
- R7. The sent message shows the attached summary, and the user can read it in full.

## Acceptance checks

- A1 (R4, R5): core tests on generated signals: a 1 kHz sine at known level gives the expected LUFS and peaks; an
  inter-sample-peak signal gives the true peak; a synthetic chord progression gives its key; a click track gives
  its tempo; silence and a 1 s clip leave key and tempo out.
- A2 (R2, R3): provider unit tests on `build_body`: switch off is identical to a stored body; switch on adds only
  the block, which parses and has only the listed fields.
- A3 (R1, R6, R7): self-test with the fixture provider: switch on, select a clip, send, see the progress line, the
  chip and its text; send again and see no second analysis; cancel leaves the message sent without the block.
- A4 (R1): review-state screenshot of General › Assistant with the switch on, and of the attached-summary chip.
- A5 (R6): bench time to analyse a 3-minute stereo mix, printed in the pull request.

## Out of scope

- Sending audio, or any per-frame data, to the provider.
- Live listening while the transport plays.
- Chord-by-chord or per-section analysis, stereo-width and phase reports (could follow).
- MIDI analysis beyond what rendering it through its instrument gives.

## Open questions

- Q1. Analyse the clip's source audio or the track's output over the clip (effects included)? Recommendation: the
  track output, since that is what the user hears and asks about; it reuses the clip render path.
- Q2. Attach the summary to every turn while the switch is on, or let the model ask for it with a tool call?
  Recommendation: every turn, cached; one request per turn is simpler and the cost is about 400 bytes.
- Q3. Where does the new DSP live: `dstudio-core` (pure Rust over a buffer, no ABI change) or the engine (reuses
  `TruePeak4x` and the window spectrum code, needs new ABI calls)? Recommendation: `dstudio-core`, as it is offline
  work and easier to test; port the small true-peak filter rather than widen the ABI.
- Q4. Key profiles: published research profiles are facts we can use, or derive our own from test material?
  Recommendation: use published profile values as numbers (facts, principle 6), written in our own code.
- Q5. Mix analysis cap (10 minutes, 15 s budget)? Recommendation: keep both; the loop range is the way to point
  the assistant at one section.
