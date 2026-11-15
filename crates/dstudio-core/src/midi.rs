//! MIDI clips, notes and the pure note transforms behind the piano-roll
//! toolbar (Quantize · Humanize · Reverse · Legato · Double, plus
//! transpose shortcuts). Transforms never mutate the project; the editor
//! wraps their result in `Command::SetMidiNotes`, so every edit is one
//! validated, invertible undo step.

use crate::model::{ClipId, PPQ, Ticks};
use crate::steps::{StepPattern, StepRows};
use serde::{Deserialize, Serialize};

/// A note inside a MIDI clip. `start` is relative to the clip start.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Note {
    /// Stable within its clip (selection survives edits/undo).
    pub id: u32,
    /// MIDI note number 0..=127 (60 = C4, matching the piano roll's "C4" label).
    pub pitch: u8,
    pub start: Ticks,
    pub length: Ticks,
    /// 1..=127.
    pub velocity: u8,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MidiClip {
    pub id: ClipId,
    pub name: String,
    /// Timeline position (ticks).
    pub start: Ticks,
    /// Clip duration (ticks). Notes beyond it are kept but not played.
    pub length: Ticks,
    pub notes: Vec<Note>,
    /// Set on a pattern clip: the step grid its notes are baked from (the
    /// clip repeats it). See `crate::steps`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pattern: Option<StepPattern>,
    /// Disabled clips stay on the timeline but play no notes.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub disabled: bool,
    /// The sidebar's pattern list (Type, Genre, made sets, kept notes). See `crate::library`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub composer: Option<crate::library::Composer>,
    /// The piano roll's scale view for this clip (root, type, highlight, scale rows).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scale: Option<crate::scale::ClipScale>,
    /// Clip looping (the editor's Loop switch, Position and Length). Clips saved before it
    /// existed load without it and play once.
    #[serde(default, rename = "loop", skip_serializing_if = "Option::is_none")]
    pub clip_loop: Option<ClipLoop>,
}

/// A MIDI clip's loop: the content range that repeats (clip-relative ticks). The clip plays
/// from its start to the loop end, then repeats the loop until the clip ends; repeats are
/// drawn dimmer on the timeline. The range is kept while looping is off.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClipLoop {
    pub on: bool,
    pub start: Ticks,
    pub end: Ticks,
    /// Clip length to return to when looping is turned back on (0 = none).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub span: Ticks,
}

fn is_zero(t: &Ticks) -> bool {
    *t == 0
}

impl ClipLoop {
    /// Looping on over the whole clip, as a new clip starts.
    pub fn whole(length: Ticks) -> Self {
        ClipLoop { on: true, start: 0, end: length, span: 0 }
    }

    pub fn len(&self) -> Ticks {
        self.end - self.start
    }

    pub fn is_empty(&self) -> bool {
        self.end <= self.start
    }

    /// The shortest loop: below the finest grid step (1/48), so every snapped range fits,
    /// and long enough that unrolling stays small.
    pub const MIN_LEN: Ticks = crate::PPQ / 16;

    pub fn validate(&self) -> Result<(), String> {
        if self.start < 0 || self.len() < Self::MIN_LEN || self.span < 0 {
            Err(format!("loop {}..{} is shorter than {} ticks or before the clip", self.start, self.end, Self::MIN_LEN))
        } else {
            Ok(())
        }
    }

    /// Notes as they play over a clip of this length: the first pass to the loop end, then
    /// the loop again and again, each cut at the loop end and at the clip end. The flag
    /// marks repeats.
    pub fn unroll(&self, notes: &[Note], length: Ticks) -> Vec<(Note, bool)> {
        let mut out = Vec::new();
        let first = self.end.min(length);
        for n in notes.iter().filter(|n| n.start >= 0 && n.start < first) {
            out.push((Note { length: n.length.min(first - n.start), ..*n }, false));
        }
        let period = self.len();
        if period < Self::MIN_LEN {
            return out;
        }
        let looped: Vec<&Note> = notes.iter().filter(|n| n.start >= self.start && n.start < self.end).collect();
        let mut at = self.end;
        while at < length && !looped.is_empty() {
            for n in &looped {
                let t = at + (n.start - self.start);
                if t < length {
                    out.push((Note { start: t, length: n.length.min(self.end - n.start).min(length - t), ..**n }, true));
                }
            }
            at += period;
        }
        out
    }
}

impl MidiClip {
    /// Default: "Insert empty MIDI clip" creates a 1-bar clip named "MIDI".
    pub fn empty(start: Ticks, length: Ticks) -> Self {
        MidiClip {
            id: ClipId(0),
            name: "MIDI".into(),
            start,
            length,
            notes: Vec::new(),
            pattern: None,
            disabled: false,
            composer: None,
            scale: None,
            clip_loop: Some(ClipLoop::whole(length)),
        }
    }

    /// Default for a Drum Rack track: an empty one-bar "Pattern"
    /// clip of 16 steps at 1/16.
    pub fn empty_pattern(start: Ticks, rows: StepRows) -> Self {
        let pattern = StepPattern::new(rows);
        MidiClip {
            id: ClipId(0),
            name: "Pattern".into(),
            start,
            length: pattern.length(),
            notes: Vec::new(),
            pattern: Some(pattern),
            disabled: false,
            composer: None,
            scale: None,
            clip_loop: None,
        }
    }

    /// Notes as they play inside the clip (a pattern repeats without its
    /// muted rows; notes are cut at the clip end).
    pub fn playing_notes(&self) -> Vec<Note> {
        let mut notes = self.tiled_notes();
        if let Some(p) = &self.pattern {
            notes.retain(|n| p.audible(n.pitch));
        }
        notes
    }

    /// Notes over the whole clip: a pattern repeated (every row), cut at the clip end.
    pub fn tiled_notes(&self) -> Vec<Note> {
        match &self.pattern {
            Some(p) => crate::steps::tile(&self.notes, p.length(), self.length),
            None => match self.clip_loop.filter(|l| l.on) {
                Some(l) => l.unroll(&self.notes, self.length).into_iter().map(|(n, _)| n).collect(),
                None => self
                    .notes
                    .iter()
                    .filter(|n| n.start < self.length)
                    .map(|n| Note { length: n.length.min(self.length - n.start), ..*n })
                    .collect(),
            },
        }
    }

    pub fn end(&self) -> Ticks {
        self.start + self.length
    }

    pub fn next_note_id(&self) -> u32 {
        self.notes.iter().map(|n| n.id + 1).max().unwrap_or(1)
    }
}

/// Sort notes canonically (start, pitch, id) for deterministic storage.
pub fn sort_notes(notes: &mut [Note]) {
    notes.sort_by_key(|n| (n.start, n.pitch, n.id));
}

pub fn validate_notes(notes: &[Note]) -> Result<(), String> {
    let mut ids = std::collections::HashSet::new();
    for n in notes {
        if n.pitch > 127 {
            return Err(format!("note {}: pitch {} > 127", n.id, n.pitch));
        }
        if n.velocity == 0 || n.velocity > 127 {
            return Err(format!("note {}: velocity {} outside 1..=127", n.id, n.velocity));
        }
        if n.start < 0 || n.length <= 0 {
            return Err(format!("note {}: start must be >= 0 and length > 0", n.id));
        }
        if !ids.insert(n.id) {
            return Err(format!("duplicate note id {}", n.id));
        }
    }
    Ok(())
}

/// Common grid values in ticks.
pub fn grid_ticks(denominator: u32) -> Ticks {
    PPQ * 4 / denominator.max(1) as Ticks
}

/// Move note starts toward the nearest grid line by `amount` (0..=1, the
/// toolbar's "Amount 100%"). Lengths are kept; start stays >= 0.
pub fn quantize(notes: &[Note], grid: Ticks, amount: f64) -> Vec<Note> {
    let amount = amount.clamp(0.0, 1.0);
    let grid = grid.max(1);
    notes
        .iter()
        .map(|n| {
            let target = ((n.start as f64 / grid as f64).round() as Ticks) * grid;
            let start = n.start + ((target - n.start) as f64 * amount).round() as Ticks;
            Note { start: start.max(0), ..*n }
        })
        .collect()
}

/// Glue notes: the selected notes (`ids`) of each pitch become one note, from the first one's
/// start to the last one's end, keeping the first one's id and velocity. Unselected notes and
/// pitches with a single selected note stay as they are.
pub fn glue(notes: &[Note], ids: &[u32]) -> Vec<Note> {
    let mut sel: Vec<&Note> = notes.iter().filter(|n| ids.contains(&n.id)).collect();
    sel.sort_by_key(|n| (n.start, n.id));
    // Each pitch's glued note, keyed by its first note's id.
    let mut first_of: std::collections::HashMap<u8, u32> = Default::default();
    let mut glued: std::collections::HashMap<u32, Note> = Default::default();
    for n in sel {
        if let Some(g) = first_of.get(&n.pitch).and_then(|id| glued.get_mut(id)) {
            g.length = (g.start + g.length).max(n.start + n.length) - g.start;
        } else {
            first_of.insert(n.pitch, n.id);
            glued.insert(n.id, *n);
        }
    }
    notes.iter().filter_map(|n| if ids.contains(&n.id) { glued.get(&n.id).copied() } else { Some(*n) }).collect()
}

/// Shift pitch by `semitones`. Fails (returns None) if any note would leave 0..=127.
pub fn transpose(notes: &[Note], semitones: i32) -> Option<Vec<Note>> {
    notes
        .iter()
        .map(|n| {
            let p = n.pitch as i32 + semitones;
            (0..=127).contains(&p).then_some(Note { pitch: p as u8, ..*n })
        })
        .collect()
}

/// Deterministic humanize: offsets start by up to ±`timing` ticks and velocity
/// by up to ±`velocity` using a seeded xorshift, so results are reproducible
/// (tests, undo/redo, AI replays). `amount` scales both (the toolbar's "Amount 50%").
pub fn humanize(notes: &[Note], amount: f64, seed: u64) -> Vec<Note> {
    let mut s = seed | 1;
    let mut rnd = move || {
        s ^= s << 13;
        s ^= s >> 7;
        s ^= s << 17;
        (s >> 11) as f64 / (1u64 << 53) as f64 * 2.0 - 1.0 // -1..1
    };
    let a = amount.clamp(0.0, 1.0);
    let timing = (PPQ as f64 / 16.0) * a; // up to a 1/64 note
    let vel = 20.0 * a;
    notes
        .iter()
        .map(|n| {
            let dt = (rnd() * timing).round() as Ticks;
            let dv = (rnd() * vel).round() as i32;
            Note { start: (n.start + dt).max(0), velocity: (n.velocity as i32 + dv).clamp(1, 127) as u8, ..*n }
        })
        .collect()
}

/// Mirror notes in time about the middle of their own extent (earliest start to latest end):
/// lengths and pitches stay, the first note's start swaps with the last
/// note's end, and the extent itself does not move.
pub fn reverse(notes: &[Note]) -> Vec<Note> {
    let Some(lo) = notes.iter().map(|n| n.start).min() else { return Vec::new() };
    let hi = notes.iter().map(|n| n.start + n.length).max().unwrap_or(lo);
    notes.iter().map(|n| Note { start: lo + hi - (n.start + n.length), ..*n }).collect()
}

/// Extend each note to the start of the next later note start (any pitch);
/// the last notes extend to the clip end.
pub fn legato(notes: &[Note], clip_length: Ticks) -> Vec<Note> {
    let mut starts: Vec<Ticks> = notes.iter().map(|n| n.start).collect();
    starts.sort_unstable();
    starts.dedup();
    notes
        .iter()
        .map(|n| {
            let next = starts.iter().copied().find(|&s| s > n.start).unwrap_or(clip_length.max(n.start + 1));
            Note { length: (next - n.start).max(1), ..*n }
        })
        .collect()
}

/// Duplicate the clip's content after itself. Returns (notes, new clip length).
pub fn double(notes: &[Note], clip_length: Ticks) -> (Vec<Note>, Ticks) {
    let first_id = notes.iter().map(|n| n.id + 1).max().unwrap_or(1);
    let mut out = notes.to_vec();
    for (id, n) in (first_id..).zip(notes.iter().filter(|n| n.start < clip_length)) {
        out.push(Note { id, start: n.start + clip_length, ..*n });
    }
    (out, clip_length * 2)
}

/// MIDI note name with the octave convention 60 = C4.
pub fn note_name(pitch: u8) -> String {
    const NAMES: [&str; 12] = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];
    format!("{}{}", NAMES[(pitch % 12) as usize], pitch as i32 / 12 - 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn n(id: u32, pitch: u8, start: Ticks, length: Ticks) -> Note {
        Note { id, pitch, start, length, velocity: 100 }
    }

    #[test]
    fn loop_unrolls_first_pass_then_repeats() {
        let notes = [n(1, 60, 0, 240), n(2, 62, 480, 960), n(3, 64, 1440, 480)];
        let l = ClipLoop { on: true, start: 480, end: 1920, span: 0 };
        let out: Vec<(Ticks, Ticks, u8, bool)> = l.unroll(&notes, 4000).iter().map(|(n, r)| (n.start, n.length, n.pitch, *r)).collect();
        assert_eq!(
            out,
            vec![
                (0, 240, 60, false),
                (480, 960, 62, false),
                (1440, 480, 64, false),
                (1920, 960, 62, true),
                (2880, 480, 64, true),
                (3360, 640, 62, true)
            ]
        );
        assert_eq!(l.unroll(&notes, 1000).len(), 2);
        assert!(ClipLoop { on: true, start: 960, end: 960, span: 0 }.validate().is_err());
    }

    #[test]
    fn quantize_full_and_partial() {
        let g = grid_ticks(16); // 240
        let notes = [n(1, 60, 250, 100), n(2, 62, 470, 100)];
        let q = quantize(&notes, g, 1.0);
        assert_eq!((q[0].start, q[1].start), (240, 480));
        let h = quantize(&notes, g, 0.5);
        assert_eq!((h[0].start, h[1].start), (245, 475));
    }

    #[test]
    fn transpose_bounds() {
        assert_eq!(transpose(&[n(1, 60, 0, 10)], 12).unwrap()[0].pitch, 72);
        assert!(transpose(&[n(1, 120, 0, 10)], 12).is_none());
    }

    #[test]
    fn humanize_is_deterministic_and_bounded() {
        let notes: Vec<Note> = (0..32).map(|i| n(i, 60, i as Ticks * 240, 100)).collect();
        let a = humanize(&notes, 0.5, 42);
        assert_eq!(a, humanize(&notes, 0.5, 42));
        assert!(a.iter().zip(&notes).all(|(h, o)| (h.start - o.start).abs() <= 30 && (h.velocity as i32 - 100).abs() <= 10));
        assert_ne!(a, notes);
    }

    #[test]
    fn reverse_legato_double() {
        let notes = [n(1, 60, 0, 240), n(2, 64, 960, 240)];
        let l = legato(&notes, 3840);
        assert_eq!((l[0].length, l[1].length), (960, 2880));
        let (d, len) = double(&notes, 3840);
        assert_eq!(len, 7680);
        assert_eq!(d.len(), 4);
        assert_eq!(d[2].start, 3840);
        assert!(validate_notes(&d).is_ok(), "doubled notes get fresh ids");
    }

    #[test]
    fn reverse_mirrors_about_the_notes_own_extent() {
        // Extent 960..2880: the early short note lands at the end, the long one at the start.
        let notes = [n(1, 60, 960, 240), n(2, 64, 1920, 960)];
        let r = reverse(&notes);
        assert_eq!((r[0].start, r[1].start), (2640, 960));
        assert!(r.iter().zip(&notes).all(|(a, b)| a.id == b.id && a.pitch == b.pitch && a.length == b.length && a.velocity == b.velocity));
        let extent = |v: &[Note]| (v.iter().map(|n| n.start).min().unwrap(), v.iter().map(|n| n.start + n.length).max().unwrap());
        assert_eq!(extent(&r), extent(&notes));
        assert_eq!(reverse(&r), notes, "reversing twice restores the notes");
    }

    #[test]
    fn reverse_edge_cases() {
        assert!(reverse(&[]).is_empty());
        let one = [n(1, 60, 500, 100)];
        assert_eq!(reverse(&one), one, "a single note is its own mirror");
        let chord = [n(1, 60, 0, 480), n(2, 64, 0, 480), n(3, 67, 480, 480)];
        let r = reverse(&chord);
        assert_eq!((r[0].start, r[1].start, r[2].start), (480, 480, 0));
        assert!(validate_notes(&r).is_ok());
    }

    #[test]
    fn validation_and_names() {
        assert!(validate_notes(&[n(1, 60, 0, 10), n(1, 61, 0, 10)]).is_err());
        assert!(validate_notes(&[Note { velocity: 0, ..n(1, 60, 0, 10) }]).is_err());
        assert_eq!(note_name(60), "C4");
        assert_eq!(note_name(61), "C#4");
        assert_eq!(note_name(21), "A0");
    }

    #[test]
    fn glue_joins_selected_notes_per_pitch() {
        let n = |id, pitch, start, length| Note { id, pitch, start, length, velocity: 90 + id as u8 };
        let notes = vec![n(1, 60, 0, 100), n(2, 60, 200, 100), n(3, 64, 0, 50), n(4, 60, 400, 50), n(5, 67, 0, 10)];
        // 1, 2 and 4 are C4 (4 not selected), 3 and 5 alone on their pitches.
        let out = glue(&notes, &[2, 1, 3]);
        assert_eq!(out, vec![n(1, 60, 0, 300), n(3, 64, 0, 50), n(4, 60, 400, 50), n(5, 67, 0, 10)]);
        assert_eq!(glue(&notes, &[]), notes);
    }
}
