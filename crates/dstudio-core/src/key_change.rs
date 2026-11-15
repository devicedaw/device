//! Changing the project key while the project holds MIDI clips: the notes can move with it
//! ("Transpose MIDI") or stay where they are ("Don't transpose").

use crate::command::Command;
use crate::model::{Project, ProjectKey};
use crate::scale::{ClipScale, ScaleKey};

/// Semitones from one key root to another, the shorter way round: -5 ..= +6.
pub fn key_shift(from_pc: u8, to_pc: u8) -> i32 {
    let d = (to_pc as i32 - from_pc as i32).rem_euclid(12);
    if d > 6 { d - 12 } else { d }
}

/// How many MIDI clips the project holds (the prompt counts them all).
pub fn midi_clip_count(p: &Project) -> usize {
    p.tracks.iter().map(|t| t.midi_clips.len()).sum()
}

/// The commands that change the project key to `new` and move every MIDI clip's notes from
/// its key (its own, else the project's) to the new root. A clip with its own key takes the
/// new root. Pattern clips (step grids) keep their rows; a note the shift would push past
/// the MIDI range stays put. The key change comes last, so undo restores everything in one step.
pub fn transpose_commands(p: &Project, new: ProjectKey) -> Vec<Command> {
    let old = p.transport.key;
    let new_root = ScaleKey::from_project(Some(new)).root;
    let mut out = Vec::new();
    for c in p.tracks.iter().flat_map(|t| &t.midi_clips) {
        let own = c.scale.and_then(|s| s.key);
        let from = own.map(|k| k.root_pc()).or(old.map(|k| k.root % 12));
        let Some(from) = from else { continue };
        let shift = key_shift(from, new.root % 12);
        if shift != 0 && c.pattern.is_none() && !c.notes.is_empty() {
            let notes = c
                .notes
                .iter()
                .map(|n| {
                    let p = n.pitch as i32 + shift;
                    let mut n = *n;
                    if (0..=127).contains(&p) {
                        n.pitch = p as u8;
                    }
                    n
                })
                .collect();
            out.push(Command::SetMidiNotes { clip: c.id, notes });
        }
        if let (Some(k), Some(s)) = (own, c.scale)
            && k.root != new_root
        {
            out.push(Command::SetClipScale { clip: c.id, scale: Some(ClipScale { key: Some(ScaleKey { root: new_root, ..k }), ..s }) });
        }
    }
    out.push(Command::SetProjectKey { key: Some(new) });
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_shift_takes_the_shorter_way() {
        assert_eq!(key_shift(0, 2), 2);
        assert_eq!(key_shift(0, 9), -3);
        assert_eq!(key_shift(0, 6), 6);
        assert_eq!(key_shift(11, 0), 1);
        assert_eq!(key_shift(4, 4), 0);
    }
}
