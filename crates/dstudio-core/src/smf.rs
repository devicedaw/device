//! Standard MIDI File export (clip menu › Export MIDI): one MIDI clip as a format 0 file.

use crate::midi::MidiClip;
use crate::model::{PPQ, TimeSignature};

/// A format 0 Standard MIDI File of `clip` as it plays (loops and patterns unrolled, notes cut
/// at the clip end), at `bpm` in `sig`, from the clip's start. Notes go on channel 1.
pub fn clip_to_smf(clip: &MidiClip, bpm: f64, sig: TimeSignature) -> Vec<u8> {
    let mut events: Vec<(i64, u8, Vec<u8>)> = Vec::new();
    // (tick, order, bytes): at one tick, meta first, then note-offs, then note-ons.
    let tempo = (60_000_000.0 / bpm.clamp(1.0, 999.0)).round() as u32;
    events.push((0, 0, vec![0xFF, 0x51, 3, (tempo >> 16) as u8, (tempo >> 8) as u8, tempo as u8]));
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
}
