# Sends from one return track into another

Status: Building · Spec: 016 · Issue: none

## Problem

A return track can't feed another return track. You can't put a delay return into a reverb
return so the echoes sit in the same room as the dry tracks, or send a little of a reverb into
a shimmer or distortion return. Today you have to copy the effect chain onto every return that
needs it, or bounce the return and send the bounce. PROGRESS.md (2026-11-12) lists this as not
yet done.

Today's behaviour, layer by layer:

- **Core.** `validate_routing` refuses any send from a `TrackKind::Return` ("a return track has
  no sends"). `Project::sanitize_routing` clears the sends of every return on load. Group tracks
  can already send into returns (returns never send, so these sends can't loop).
- **Engine.** The engine already supports this routing, because the app publishes a group as a
  return. In `se_session_publish_v9` a return can send into other returns. Sends that form a loop
  are refused (`apply_routing`). Processing order follows the bus edges (`plan_sidechains`), and
  delay compensation carries each bus's arrival time down the chain (`Engine::plan_pdc`). What
  is missing is the solo rule. A bus's sends follow its main output. With a return soloed, a
  bus that is not soloed sends nothing, so soloing a return cuts off what other buses feed it.
  Soloing a return that feeds another also leaves its target silent. Groups have this problem
  today.
- **App.** Return strips in the mixer have no Sends section. The Sends tab of a return
  (`send_rows`) is empty, and `can_send` refuses a return as the sender.
- **Live set export (spec 006).** Each Live return track gets its own sends, written present but
  off. A return with devices is rendered alone (`stem_snapshot`), with the other buses muted.
- **Stems** are dry ordinary tracks only. Returns and groups are never a stem. The mixdown
  includes every return.

## Behaviour

**Which returns are offered.** A return sends only into returns **after** it in the return
order: A can send into B, C and later, B into C and later, and the last return sends nowhere.
Signal always flows down the list, so a loop can't happen and no control is ever disabled for a
reason the user can't see. (The alternative, any other return with loops refused, is in Q1.)

- **Mixer.** When the Sends section is open, return strips show it too. A return strip has a
  row for each later return. The rows for itself and earlier returns stay empty, so rows line up
  across strips. The row tag menu (Post-fader, Pre-fader, Reset to 0 dB, Remove send) works as on
  ordinary tracks. The last return's Sends section is empty, apart from the "+".
- **Channel strip, Sends tab** of a selected return: one knob for each later return, with the
  same layout, Post / Pre chip, dB readout and double-click to 0 dB as an ordinary track. "+"
  adds the next lettered return, which is always later, so a new silent slot shows at once.
- Slots are silent (−∞, dim) until moved, as on ordinary tracks. Level is −∞ to +6 dB.
  Post-fader taps the return after its volume and pan. Pre-fader taps it after its insert chain
  and trim, before volume and pan.
- A return's Out stays fixed to Master.
- **Reordering returns.** If a move would put a send's target before its source, the move is
  refused and the status line names the send ("Return B sends into Return C: remove that send to
  move it here"). Moves that keep every send pointing down are allowed. See Q2.
- **Removing a return** also removes the sends that other returns make into it, in the same
  undo step, as for ordinary tracks.

**Mute and solo.** A muted return plays nothing and sends nothing, into other returns too.

- When only ordinary tracks are soloed, returns stay solo-safe. They are fed by the soloed
  tracks, and the sends between returns keep running. You hear the soloed track's delay going
  into the reverb.
- When a return is soloed, you hear it, and also the returns it feeds, directly or through
  other returns. Those returns play only what reaches them from the soloed return, not what
  ordinary tracks send them directly. Returns that feed a soloed return keep sending into it,
  but you don't hear them on the master. A soloed return therefore sounds the same as it does
  in the full mix. Returns that neither feed a soloed return nor are fed by one are silent.
- Rules for ordinary tracks don't change. Spec 002's rule still applies: when ordinary tracks are
  soloed as well, only their sends feed the returns.

**Timing.** With delay compensation on, a track heard directly, through A, and through A then B
reaches the master aligned, also when A and B both have latent plugins. The latency report
includes the chain. With delay compensation off, nothing is delayed.

**Undo labels.** These are the same as for ordinary sends: Send Level Change (a drag is one
step, and the first move stores a silent slot), Send Pre-Fader / Send Post-Fader, Remove Send,
Add Return Track ("+"), Delete Track. A refused move adds no undo step.

**Saving.** Return sends are saved in the track's existing `sends` list. Old projects load
unchanged, because they have no return sends. A project that would loop, or sends upward, after
a hand edit is repaired on load: the sends that break the rule are dropped.

**Exports.**

- *Mixdown* includes the chained returns and matches the live mix.
- *Stems* stay dry, so nothing changes.
- *Bounce* (clip renders, Bounce to new track) stays dry, as in spec 002.
- *Live set*:
  - A send from one return into another becomes a send from that Live return track to the later
    one, at the same level. Live caps sends at 0 dB, and the notes file names any send that was
    clamped.
  - A rendered return ("<return> (rendered)") includes everything that feeds it, upstream returns
    included. No send is written into a rendered return, so nothing sounds twice.
  - When the sending return is rendered and its target is not, the rendered audio track carries
    the send. The spec 006 rule for pre-fader sends applies: the level is divided by the return's
    volume.

## Requirements

- R1. A return may send into a return that comes after it in the track order, at most one send
  for each target, at 0 to +6 dB, pre- or post-fader. The core refuses any other send from a
  return (into itself, an earlier return, a group or an ordinary track), and the project stays
  unchanged.
- R2. `MoveTrack` refuses a return move that would leave a send pointing at an earlier return.
- R3. Removing a return removes the sends from returns into it, and undo restores them exactly.
- R4. Every edit is a `Command` with an exact inverse and the labels in Behaviour. Return send
  level drags combine into one undo step.
- R5. Projects without return sends load unchanged. Projects with return sends save and load
  without change. On load, return sends that break R1 are dropped, and nothing else changes.
- R6. The engine renders the return-to-return sends at their level and tap point, in the live
  mix and in exports.
- R7. Mute and solo follow Behaviour. This includes these three cases:
  - a soloed return's downstream returns are heard, fed only through the soloed return;
  - upstream buses keep feeding a soloed return without being heard;
  - when only ordinary tracks are soloed, return-to-return sends keep running.
- R8. With delay compensation on, direct, single-return and chained-return paths arrive at the
  master aligned, and the latency report includes the chain. With it off, nothing is delayed.
- R9. Rendering stays real-time safe. A session without sends between buses renders the same,
  bit for bit, as it does today, for every mute and solo state.
- R10. The mixer's Sends section on return strips and the Sends tab of a return work as
  described in Behaviour, with silent slots only for later returns.
- R11. The Live set export follows Behaviour. The structure check accepts active return sends,
  and the notes file lists clamped and rescaled return sends.
- R12. The cost is measured: a benchmark with chained returns against the same session without
  return sends.

## Acceptance checks

- A1 (R1, R3, R4): core tests cover these cases:
  - a send from A into B is accepted;
  - sends into self, into an earlier return, into a group, and into an ordinary track are each
    refused, and the project stays unchanged;
  - removing B with a send from A into it, then undo, restores the project exactly;
  - labels are correct, and level drags combine into one step.
- A2 (R2): core test: a move that keeps the order is allowed. A move that puts B above A while A
  sends into B is refused, and the project stays unchanged.
- A3 (R5): core tests cover an old project, a round trip with return sends, and a hand-edited
  file with a loop or an upward send, which loads with only those sends dropped.
- A4 (R6, R7): engine tests (`test_routing.cpp`) cover these cases:
  - a send from return A into return B reaches the master, both pre- and post-fader;
  - a muted A sends nothing;
  - each solo case in R7, with A→B and with a group feeding a return;
  - the export equals the live render.
- A5 (R8): engine test: latent test plugins on both A and B. An impulse heard directly, through
  A, and through A→B reaches the master on the same frame, and the latency report includes the
  chain.
- A6 (R9): the RT-safety tracker test runs a session with chained returns. The existing PDC
  golden hashes and routing tests still pass without changes.
- A7 (R10): self-test steps and before and after review-state screenshots:
  - set a send from Return A into Return B in the Sends tab, then in the mixer row, including
    the tag menu;
  - the last return has no slots;
  - a refused move shows the status line.
- A8 (R11): unit tests on the written XML: A→B active at its level, a send into a rendered
  return left out, a rendered A carrying its send into an unrendered B, and the notes file.
  The Live set opens in Live with the sends in place (manual check, as spec 006 A4).
- A9 (R12): a `bench_render` "Returns" row with 32 tracks into 2 returns plus a send from A into
  B, against the existing row. The numbers go in the pull request.

## Out of scope

- Feedback loops between returns.
- Return sends to groups, and group-to-group sends.
- Changing a return's Out away from Master.
- Send automation and modulation.
- Return or group stems.
- Changing how the Live export handles groups (spec 006).
- More than 16 buses.

## Open questions

None. Agreed 2026-11-20: a return offers sends to later returns only; a move that would break that
order is refused with a status line; returns below a soloed return are fed only by it; one solo rule
for all buses (returns and groups); `SCHEMA_VERSION` stays 1 and the release notes say older
builds drop return-to-return sends.
