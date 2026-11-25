# Return tracks and sends

Status: Done · Spec: 002 · Issue: none

## Problem

Every track plays straight into the master. There is no way to share one effect between several
tracks: to put five tracks in the same reverb space you need five reverbs, each set up the same
way, each costing CPU, and changing the room means changing five devices. Parallel processing (a
crushed copy of the drums under the clean ones) needs a duplicated track.

The app already shows where this belongs, but none of it works: the Timeline header's Returns pill
does nothing, the mixer strips' Sends row and its "+" open a placeholder, the channel strip's
Sends tab is empty, and its Out selector always reads Master.

## Behaviour

**Return tracks.** A return track is a third kind of track next to Audio and Instrument. It has no
clips and no input; what it plays is the sum of what other tracks send to it. Like any track it
has a name, a colour, an insert chain (Devices tab), volume, pan, mute, solo, meters and volume and
pan automation. Return tracks are lettered in order (A, B, C ...); a new one is called
"Return A", "Return B" and so on, and can be renamed.

- **Adding one.** Add track (timeline and mixer) opens a menu: Audio track, Instrument track,
  Return track (the shortcut still adds an audio track). The "+" beside Sends (mixer strips and
  the channel strip's Sends tab) adds the next lettered return; every track gets a silent send
  slot for it and the track being edited stays selected. With no return tracks, the Returns
  pill adds Return A, and the Sends tab shows a dim "Return A" placeholder that creates the
  return and a 0 dB send to it in one step.
- **Where they show.** Return tracks sit after the ordinary tracks, always in their own group.
  - Timeline: the Returns pill in the Timeline header shows or hides the return rows (with no
    return tracks yet it adds Return A). They are
    pinned at the bottom of the track list, above the master row, like the master. Their lanes take
    no clips (dropping or recording onto a return does nothing). The pill is lit while the rows
    show. Hiding them does not mute them. The pill state is view state, saved with the window
    preferences, not with the project and not undoable.
  - Mixer: return strips come after the track strips, before the Add track column and the master,
    with a divider before the first one. They have no R/I buttons and no Sends section.
  - Bottom panel: selecting a return track shows its channel strip (In is replaced by a "Return"
    label, Out is fixed to Master) and its Devices chain.
- **Names.** A return track is the shared effect track ("Return A"); a send is how much of one
  track goes into it. The Sends tab and the mixer's Sends rows name each send after its return
  ("Return A", or the letter A), so the two always read as one pair.
- **Removing one.** Removing a return track also removes every send to it and resets every track
  whose Out pointed at it to Master, in the same undo step.

**Sends.** A send copies a track's signal, at its own level, into a return track. A track has at
most one send per return, and every ordinary track has a send slot for every return: a slot with
nothing sent is silent (−∞) and shown dim, and moving its control sends.

- Mixer strip: the Sends row shows "Sends" and a chevron; clicking it expands the Sends section of
  every strip (one row per return: the return's colour tag and letter, a horizontal level slider
  with its dB value), clicking again collapses it. The tag opens the send's options: Post-fader,
  Pre-fader, Reset to 0 dB, Remove send. "+" adds a return track.
- Channel strip, Sends tab: one knob per return, two to a row like the Input tab: a Post / Pre
  chip above a stored send (click to switch), the level knob, its dB value and the return's
  name. "+" beside the tab adds a return track.
- Level: −∞ to +6 dB. Double-click sets 0 dB (also on a silent slot). A drag is one undo step.
- Post-fader (the default): the send taps the track after its volume and pan, so pulling the fader
  down pulls the send down with it. Pre-fader: the send taps after the insert chain, before volume
  and pan.
- A muted track sends nothing. With solo: soloing ordinary tracks keeps the returns playing (you
  hear the soloed track with its reverb, not the other tracks' reverb); soloing a return plays only
  the soloed returns, still fed by every unmuted track's sends (only the soloed ones', when
  ordinary tracks are soloed too).
- A return track has no sends of its own.

**Out.** The channel strip's Out selector lists Master: to share an effect, a track sends to a return
track instead. (A project saved when Out could pick a return keeps that routing, and the selector
then lists that return too, so the track can be moved back to Master. Groups, when they come, will
be listed here.) Return tracks always go to the master.

**Timing.** Delay compensation keeps everything aligned: a track heard directly and through a
return lines up, also when the return has plugins with latency. Meters, the master and exports
include the returns. Exports and the live mix sound the same.

**Undo labels.** Add Return Track, Remove Track, Add Send (the placeholder's return and send),
Remove Send, Send Level Change (a drag coalesces; the first move of a silent slot stores the send),
Send Pre-Fader / Send Post-Fader, Output Change.

**Saving.** Return tracks, sends and outputs are saved with the project. Projects saved before
this load with no returns, no sends and every Out on Master.

## Requirements

- R1. Return tracks are a track kind in the project model, ordered after every ordinary track.
  Adding an ordinary track never places it among the returns. At most 16 return tracks.
- R2. Clips, MIDI clips, recording, input, arm, monitor and instrument plugins are refused on a
  return track (the project is unchanged).
- R3. A track has a list of sends, at most one per return; each has a level (linear gain, 0 to
  +6 dB) and a pre/post-fader flag. Sends must target a return track and not the track itself;
  return tracks have no sends.
- R4. A track's output is Master or a return track; a return's output is Master.
- R5. Removing a return track removes the sends to it and resets the outputs routed to it, and its
  undo restores all of them exactly.
- R6. Every edit above is a command with an exact inverse and the labels in Behaviour; send level
  drags coalesce into one undo step.
- R7. Old projects load unchanged; projects with returns, sends and outputs round-trip through save
  and load.
- R8. The engine renders sends (pre- or post-fader, at their level) and outputs into the return
  tracks, the returns through their own chain, volume, pan and meters, into the master, in the
  live mix and in exports.
- R9. Mute and solo follow Behaviour: muted tracks send nothing. Soloing ordinary tracks keeps
  returns audible, fed only by the soloed tracks. Soloing a return plays only the soloed returns
  (no ordinary track is heard directly unless it is soloed too); they are fed by every unmuted
  ordinary track, or, when ordinary tracks are soloed as well, by the unmuted soloed ones only.
- R10. With delay compensation on, a track's direct signal and its signal through any return
  arrive at the master aligned, including when the return's chain has latency; the latency report
  includes the returns. With it off, nothing is delayed (as today).
- R11. Rendering stays real-time safe (no allocation, locks or I/O on the audio thread) and a
  session without returns renders bit-identically to before.
- R12. The Returns pill shows and hides the return rows; return rows, return strips, the Sends
  sections and the Out selector work as described in Behaviour.
- R13. The cost of a send is measured: a benchmark with sends and returns, against the same
  session without them.

## Acceptance checks

- A1 (R1, R2, R3, R4, R6): core tests: add/remove return with undo/redo and ids; ordinary tracks
  added before returns; the return cap; each refused edit leaves the project unchanged; send and
  output validation; labels; send level coalescing.
- A2 (R5): core test: removing a return with two senders and one routed output, then undo,
  restores the project exactly.
- A3 (R7): core tests: a project without the new fields loads; a project with them round-trips.
- A4 (R8, R9): engine tests (ctest): post- and pre-fader sends at their level, output to a return,
  return volume/pan/mute, mute and solo rules, export equals the live render.
- A5 (R10): engine test: a latent test plugin on a return; an impulse heard directly and through
  the return arrives at the same frame; the latency report includes the return.
- A6 (R11): the existing RT-safety tracker test runs a session with sends and returns; the existing
  PDC golden hashes still match for sessions without returns.
- A7 (R8): Rust end-to-end test through `dstudio-engine`: a send reaches the master through a
  return.
- A8 (R12): self-test steps (Return A from the Sends placeholder, set a send level, Pre, Return B
  from "+" and a drag that turns its silent slot on, Out listing Master, the Returns pill, the
  mixer's send options) and review-state screenshots of the timeline with returns, the mixer with the
  Sends section open, and the channel strip Sends tab, before and after.
- A9 (R13): `bench_render` table "Returns": 32 tracks with 2 sends each into 2 returns, against
  32 tracks without sends; numbers in the pull request.

## Out of scope

- Group tracks (folders that sum their member tracks) and sends from return tracks to other
  returns. The engine routing is written so these can be added later.
- Send level automation, and send level modulation.
- Sidechains reading a return track are allowed but, as today, not delay-compensated.
- Per-send pan, mute or phase.
- Renders of a single track (Bounce to new track, clip renders) stay dry: they do not include what
  the track sends to returns.
- Built-in effect presets for returns (a "Reverb return" template) belong to the device work.

## Open questions

- Q1. How should return rows, the expanded Sends section and the channel strip's Sends tab look?
  Screenshots or a description of the layout you have in mind would help. Until then the UI follows
  the Behaviour above with the app's existing styling.
- Q2. New sends at 0 dB and post-fader. Recommendation: keep.
- Q3. Solo-safe returns (soloing a track keeps the returns playing). Recommendation: keep.
