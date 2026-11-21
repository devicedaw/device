//! Standard MIDI File export (clip menu › Export MIDI): one MIDI clip as a format 0 file.

use crate::midi::MidiClip;
use crate::model::{PPQ, Ticks, TimeSignature};
use crate::tempo::TempoMap;

/// A format 0 Standard MIDI File of `clip` as it plays (loops and patterns unrolled, notes cut
/// at the clip end), at `bpm` in `sig`, from the clip's start. Notes go on channel 1.
pub fn clip_to_smf(clip: &MidiClip, bpm: f64, sig: TimeSignature) -> Vec<u8> {
    let bpm = if bpm.is_finite() { bpm.clamp(1.0, 999.0) } else { 120.0 };
    clip_to_smf_on(clip, &TempoMap::constant(bpm), sig)
}

/// The tempo events of a clip on `map` (spec 015), as (ticks from the clip start, BPM): the tempo
/// in effect at the clip's start, then one at each change inside it. A ramp is written as a step
/// every sixteenth note, each holding the step's average tempo, so every step ends exactly when it
/// does on the map.
pub fn tempo_events(map: &TempoMap, start: Ticks, length: Ticks) -> Vec<(Ticks, f64)> {
    let end = start + length.max(1);
    if map.is_constant() {
        return vec![(0, map.first_bpm())];
    }
    const STEP: Ticks = PPQ / 4;
    let mut cuts = vec![start];
    for g in map.segments() {
        if g.tick > start && g.tick < end {
            cuts.push(g.tick);
        }
        if g.ramps() {
            let mut t = (g.tick.max(start) / STEP + 1) * STEP;
            while t < g.end_tick.min(end) {
                cuts.push(t);
                t += STEP;
            }
        }
    }
    cuts.sort_unstable();
    cuts.dedup();
    let mut out: Vec<(Ticks, f64)> = Vec::with_capacity(cuts.len());
    for (i, &a) in cuts.iter().enumerate() {
        let b = cuts.get(i + 1).copied().unwrap_or(end);
        let held = map.segments().iter().rev().find(|g| g.tick <= a).is_none_or(|g| !g.ramps());
        let seconds = map.seconds_at(b as f64) - map.seconds_at(a as f64);
        let bpm = if held || seconds <= 0.0 { map.bpm_at(a as f64) } else { (b - a) as f64 / PPQ as f64 * 60.0 / seconds };
        if out.last().is_none_or(|l| (l.1 - bpm).abs() > 1e-9) {
            out.push((a - start, bpm));
        }
    }
    out
}

/// [clip_to_smf] on a song's tempo map: a tempo event at the clip's start and at each change
/// inside it ([tempo_events]).
pub fn clip_to_smf_on(clip: &MidiClip, map: &TempoMap, sig: TimeSignature) -> Vec<u8> {
    let mut events: Vec<(i64, u8, Vec<u8>)> = Vec::new();
    // (tick, order, bytes): at one tick, meta first, then note-offs, then note-ons.
    // Microseconds per quarter note: a 24-bit field (slower than about 3.6 BPM doesn't fit).
    for (t, bpm) in tempo_events(map, clip.start, clip.length) {
        let tempo = ((60_000_000.0 / bpm.clamp(1.0, 999.0)).round() as u32).min(0xFF_FFFF);
        events.push((t, 0, vec![0xFF, 0x51, 3, (tempo >> 16) as u8, (tempo >> 8) as u8, tempo as u8]));
    }
    let denom_pow = (sig.denominator.max(1) as f64).log2().round() as u8;
    events.push((0, 0, vec![0xFF, 0x58, 4, sig.numerator, denom_pow, 24, 8]));
    let name = clip.name.as_bytes();
    if !name.is_empty() {
        let mut meta = vec![0xFF, 0x03];
        meta.extend(vlq(name.len() as u32));
        meta.extend_from_slice(name);
        events.push((0, 0, meta));
    }
    for n in clip.playing_notes() {
        if n.length <= 0 || n.start < 0 {
            continue;
        }
        let pitch = n.pitch.min(127);
        events.push((n.start, 2, vec![0x90, pitch, n.velocity.clamp(1, 127)]));
        events.push((n.start + n.length, 1, vec![0x80, pitch, 0]));
    }
    events.sort_by_key(|e| (e.0, e.1));
    let mut track = Vec::new();
    let mut last = 0i64;
    for (tick, _, bytes) in &events {
        track.extend(vlq((tick - last).max(0) as u32));
        track.extend_from_slice(bytes);
        last = *tick;
    }
    // End of track at the clip end.
    track.extend(vlq((clip.length - last).max(0) as u32));
    track.extend_from_slice(&[0xFF, 0x2F, 0]);

    let mut out = Vec::with_capacity(22 + track.len());
    out.extend_from_slice(b"MThd");
    out.extend_from_slice(&6u32.to_be_bytes());
    out.extend_from_slice(&0u16.to_be_bytes()); // format 0
    out.extend_from_slice(&1u16.to_be_bytes()); // one track
    out.extend_from_slice(&(PPQ as u16).to_be_bytes());
    out.extend_from_slice(b"MTrk");
    out.extend_from_slice(&(track.len() as u32).to_be_bytes());
    out.extend(track);
    out
}

/// MIDI variable-length quantity.
fn vlq(mut v: u32) -> Vec<u8> {
    let mut bytes = vec![(v & 0x7F) as u8];
    v >>= 7;
    while v > 0 {
        bytes.push((v & 0x7F) as u8 | 0x80);
        v >>= 7;
    }
    bytes.reverse();
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::midi::Note;

    #[test]
    fn variable_length_quantities() {
        assert_eq!(vlq(0), [0x00]);
        assert_eq!(vlq(0x7F), [0x7F]);
        assert_eq!(vlq(0x80), [0x81, 0x00]);
        assert_eq!(vlq(960), [0x87, 0x40]);
        assert_eq!(vlq(0x0FFF_FFFF), [0xFF, 0xFF, 0xFF, 0x7F]);
    }

    #[test]
    fn a_clip_becomes_a_format_0_file() {
        let mut clip = MidiClip::empty(4 * PPQ, 4 * PPQ);
        clip.name = "Bass".into();
        clip.notes = vec![
            Note { id: 1, pitch: 36, start: 0, length: PPQ, velocity: 100 },
            Note { id: 2, pitch: 43, start: PPQ, length: PPQ, velocity: 90 },
            // Past the clip end: not exported.
            Note { id: 3, pitch: 48, start: 5 * PPQ, length: PPQ, velocity: 90 },
        ];
        let f = clip_to_smf(&clip, 120.0, TimeSignature { numerator: 4, denominator: 4 });
        assert_eq!(&f[..4], b"MThd");
        assert_eq!(&f[8..14], &[0, 0, 0, 1, 0x03, 0xC0]); // format 0, one track, 960 PPQ
        assert_eq!(&f[14..18], b"MTrk");
        let len = u32::from_be_bytes(f[18..22].try_into().unwrap()) as usize;
        assert_eq!(f.len(), 22 + len);
        let t = &f[22..];
        // Tempo 500000 µs per quarter, then 4/4.
        assert_eq!(&t[..7], &[0, 0xFF, 0x51, 3, 0x07, 0xA1, 0x20]);
        assert_eq!(&t[7..15], &[0, 0xFF, 0x58, 4, 4, 2, 24, 8]);
        // Name, then note 1 on at 0, off at 960 before note 2 on at 960.
        let rest = &t[15 + 2 + 2 + 4..];
        assert_eq!(&rest[..4], &[0, 0x90, 36, 100]);
        assert_eq!(&rest[4..9], &[0x87, 0x40, 0x80, 36, 0]);
        assert_eq!(&rest[9..13], &[0, 0x90, 43, 90]);
        assert_eq!(&rest[13..18], &[0x87, 0x40, 0x80, 43, 0]);
        // End of track at the clip end (two more beats).
        assert_eq!(&rest[18..], &[0x8F, 0x00, 0xFF, 0x2F, 0]);
        assert_eq!(f.windows(3).filter(|w| w[0] == 0x90 && w[1] == 48).count(), 0);
    }

    #[test]
    fn tempo_changes_inside_a_clip_become_tempo_events() {
        use crate::tempo::TempoPoint;
        let pts = [
            TempoPoint { time: 0, bpm: 120.0, ramp: false },
            TempoPoint { time: 6 * PPQ, bpm: 90.0, ramp: true },
            TempoPoint { time: 8 * PPQ, bpm: 150.0, ramp: false },
        ];
        let map = TempoMap::new(120.0, &pts);
        // A clip from beat 4 to beat 10: 120 at its start, 90 at beat 2, a ramp in sixteenths up to
        // beat 4, then 150.
        let ev = tempo_events(&map, 4 * PPQ, 6 * PPQ);
        assert_eq!(ev[0], (0, 120.0));
        assert_eq!(ev[1].0, 2 * PPQ);
        assert_eq!(ev.len(), 1 + 8 + 1, "{ev:?}");
        assert_eq!(*ev.last().unwrap(), (4 * PPQ, 150.0));
        // Each step lasts exactly as long as on the map.
        for w in ev[1..].windows(2) {
            let (a, b) = (w[0].0 + 4 * PPQ, w[1].0 + 4 * PPQ);
            let secs = (b - a) as f64 / PPQ as f64 * 60.0 / w[0].1;
            assert!((secs - (map.seconds_at(b as f64) - map.seconds_at(a as f64))).abs() < 1e-12);
        }
        assert!(ev[1..9].windows(2).all(|w| w[1].1 > w[0].1), "the ramp rises");
        // The file carries them all.
        let clip = MidiClip::empty(4 * PPQ, 6 * PPQ);
        let f = clip_to_smf_on(&clip, &map, TimeSignature { numerator: 4, denominator: 4 });
        assert_eq!(f.windows(3).filter(|w| w == &[0xFF, 0x51, 3]).count(), ev.len());
        // Without changes: the one tempo, as before.
        assert_eq!(tempo_events(&TempoMap::constant(97.0), 0, PPQ), vec![(0, 97.0)]);
    }

    #[test]
    fn the_tempo_always_fits_its_24_bit_field() {
        let clip = MidiClip::empty(0, PPQ);
        let sig = TimeSignature { numerator: 4, denominator: 4 };
        let tempo = |bpm: f64| clip_to_smf(&clip, bpm, sig)[22 + 4..22 + 7].to_vec();
        assert_eq!(tempo(2.0), [0xFF, 0xFF, 0xFF]);
        assert_eq!(tempo(f64::NAN), tempo(120.0));
    }
}
