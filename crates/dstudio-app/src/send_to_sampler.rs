//! Clip menu › Send to Sampler: an audio clip's sound on a new instrument track, played by
//! a Sampler that holds the clip's file, its start and end set to the part the clip plays.
//! The file isn't copied or rendered; one undo step.

use crate::controller::{BottomTab, Controller, next_track_color};
use dstudio_core::builtin::sampler::SAMPLER;
use dstudio_core::{ClipId, Command, DeviceSample, Track, TrackId, TrackKind};

/// Sampler start and end (fractions of the file) for a clip playing `length` frames from
/// `offset` in a file of `frames` frames.
pub fn sample_range(offset: u64, length: u64, frames: u64) -> (f32, f32) {
    if frames == 0 {
        return (0.0, 1.0);
    }
    let f = frames as f64;
    let start = (offset as f64 / f).clamp(0.0, 1.0);
    let end = ((offset + length) as f64 / f).clamp(start, 1.0);
    (start as f32, end as f32)
}

impl Controller {
    /// Send to Sampler: a new instrument track below the clip's track, its instrument a
    /// Sampler loaded with the clip's audio. Selects the track and shows its devices.
    pub fn send_to_sampler(&mut self, id: ClipId) -> Option<TrackId> {
        let p = self.project();
        let (ti, ci) = p.find_clip(id)?;
        let clip = &p.tracks[ti].clips[ci];
        let Some(asset) = p.asset(clip.asset) else {
            self.set_status("Send to Sampler: the clip's audio is missing", 3);
            return None;
        };
        let (start, end) = sample_range(clip.source_offset, clip.source_length, asset.frames);
        let name = if clip.name.trim().is_empty() { "Sampler".to_string() } else { clip.name.clone() };
        let mut device = SAMPLER.instantiate();
        device.params.insert("start".into(), start);
        device.params.insert("end".into(), end);
        device.samples.push(DeviceSample { slot: 0, asset: asset.id, name: name.clone(), slices: Vec::new() });
        let track_id = TrackId(p.next_id);
        let track = Track::new(track_id, TrackKind::Instrument, &name, next_track_color(p));
        // Below the clip's track, inside its group if it has one.
        let index = Some((ti + 1).min(p.ordinary_track_count()));
        let output = p.tracks[ti].output.filter(|g| p.track(*g).is_some_and(|g| g.is_group()));
        let commands = vec![
            Command::AddTrack { index, track: Track { output, ..track } },
            Command::SetInstrumentPlugin { track: track_id, device: Some(device) },
        ];
        if !self.execute(Command::Batch { label: "Send to Sampler".into(), commands }) {
            self.set_status("Send to Sampler failed", 3);
            return None;
        }
        self.select_track(track_id);
        self.select_clip(None);
        self.set_bottom_tab(BottomTab::Devices);
        Some(track_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_sample_range_is_the_part_the_clip_plays() {
        assert_eq!(sample_range(0, 1000, 1000), (0.0, 1.0));
        assert_eq!(sample_range(250, 500, 1000), (0.25, 0.75));
        // A clip running past the file end stops at the end.
        assert_eq!(sample_range(500, 1000, 1000), (0.5, 1.0));
        assert_eq!(sample_range(0, 10, 0), (0.0, 1.0));
    }

    #[test]
    fn send_to_sampler_adds_a_sampler_track_below() {
        use dstudio_core::{Asset, AssetId, Clip, PPQ, Project};
        let engine = std::sync::Arc::new(dstudio_engine::Engine::new(dstudio_engine::Config::default()).unwrap());
        let mut c = Controller::new(engine, Project::default());
        let asset = Asset {
            id: AssetId(0),
            relative_path: None,
            absolute_path: "/nonexistent/t.wav".into(),
            sample_rate: 48_000.0,
            channels: 2,
            frames: 48_000 * 10,
        };
        assert!(c.execute(Command::AddAsset { asset }));
        let a = c.project().assets[0].id;
        let track = c.project().tracks[0].id;
        let clip = Clip {
            id: ClipId(0),
            asset: a,
            name: "Loop".into(),
            start: 4 * PPQ,
            source_offset: 48_000,
            source_length: 48_000 * 4,
            gain: 1.0,
            warp: Default::default(),
            disabled: false,
            clip_loop: None,
            fades: Default::default(),
            takes: Vec::new(),
        };
        assert!(c.execute(Command::AddClip { track, clip }));
        let id = c.project().tracks[0].clips[0].id;
        let n = c.project().tracks.len();
        let new = c.send_to_sampler(id).expect("sent");
        let p = c.project();
        assert_eq!(p.tracks.len(), n + 1);
        let t = &p.tracks[1];
        assert_eq!((t.id, t.kind, t.name.as_str()), (new, TrackKind::Instrument, "Loop"));
        let d = t.instrument_plugin.as_ref().expect("instrument");
        assert_eq!(d.plugin.id, "sampler-steam");
        assert_eq!(d.samples.first().map(|s| s.asset), Some(a));
        assert_eq!((d.params.get("start").copied(), d.params.get("end").copied()), (Some(0.1), Some(0.5)));
        assert_eq!(c.selected_track, Some(new));
        assert_eq!(c.doc.undo_label(), Some("Send to Sampler"));
        c.undo();
        assert_eq!(c.project().tracks.len(), n);
    }
}
