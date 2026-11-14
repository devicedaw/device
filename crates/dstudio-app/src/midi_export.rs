//! Clip menu › Export MIDI: a MIDI clip as a Standard MIDI File (format 0, at the project's
//! tempo and time signature), saved where the user picks and shown in the file manager.

use crate::controller::Controller;
use dstudio_core::ClipId;
use std::path::{Path, PathBuf};

/// The file name the save dialog suggests: "<track> <clip>.mid".
pub fn midi_file_name(track: &str, clip: &str) -> String {
    let name = crate::dialogs::file_safe(&format!("{} {}", track.trim(), clip.trim()), 80);
    format!("{}.mid", if name.is_empty() { "Clip" } else { &name })
}

/// `path` with a .mid extension (kept if it already has .mid or .midi).
pub fn with_mid_extension(path: PathBuf) -> PathBuf {
    let ok = path.extension().is_some_and(|e| e.eq_ignore_ascii_case("mid") || e.eq_ignore_ascii_case("midi"));
    if ok {
        return path;
    }
    let mut name = path.into_os_string();
    name.push(".mid");
    name.into()
}

impl Controller {
    /// Export MIDI: asks for a file (the main loop runs the dialog with the controller
    /// unborrowed; the self-test sets `midi_export_path` instead), then `export_midi`.
    pub fn export_midi_request(&mut self, id: ClipId) {
        let p = self.project();
        let Some(clip) = p.midi_clip(id) else { return };
        let track = self.clip_track(id).and_then(|t| p.track(t)).map(|t| t.name.clone()).unwrap_or_default();
        let name = midi_file_name(&track, &clip.name);
        match self.midi_export_path.take() {
            Some(path) => self.export_midi(id, &path, false),
            None => self.midi_dialog_request = Some((id, name)),
        }
    }

    /// Writes MIDI clip `id` to `path`; `reveal` shows the file in the file manager afterwards.
    pub fn export_midi(&mut self, id: ClipId, path: &Path, reveal: bool) {
        let p = self.project();
        let Some(clip) = p.midi_clip(id) else { return };
        let bytes = dstudio_core::smf::clip_to_smf(clip, p.transport.bpm, p.transport.time_signature);
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        match std::fs::write(path, bytes) {
            Ok(()) => {
                self.set_status("Exported MIDI", 3);
                if reveal {
                    let _ = crate::clip_render::reveal(&path.to_string_lossy());
                }
            }
            Err(e) => self.set_status(format!("Export MIDI failed: {e}"), 4),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_suggested_name_and_extension() {
        assert_eq!(midi_file_name("Bass", "Part 1"), "Bass Part 1.mid");
        assert_eq!(midi_file_name("", ""), "Clip.mid");
        assert_eq!(with_mid_extension("/a/b".into()), PathBuf::from("/a/b.mid"));
        assert_eq!(with_mid_extension("/a/b.MID".into()), PathBuf::from("/a/b.MID"));
        assert_eq!(with_mid_extension("/a/b.midi".into()), PathBuf::from("/a/b.midi"));
    }
}
