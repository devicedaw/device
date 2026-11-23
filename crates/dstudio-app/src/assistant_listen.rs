//! Settings › General › Let the assistant listen to audio (parity roadmap G20).
//!
//! With the switch on, every assistant turn first renders what the user is pointing at on a
//! private offline engine (the selected clip, else the selected track, else the mix; over the
//! clip, else the loop region, else the song from its start; at most 30 s), measures it with
//! `dstudio_core::listen`, and adds that one-paragraph description to the request. The audio
//! never leaves the machine. Third-party plugins are left out of the render (they must be
//! created on the main thread); the description says so when that changed the sound.

use crate::controller::Controller;
use dstudio_core::listen;
use dstudio_core::render_plan::{self, ClipSpan, RenderMode};
use dstudio_core::{Project, Ticks, TrackKind, builtin};
use dstudio_engine::offline::{self, OfflineError, OfflineRender};
use std::sync::atomic::{AtomicBool, AtomicUsize};

/// What a turn listens to.
pub struct ListenJob {
    pub render: OfflineRender,
    /// "the selected clip \"Bass\"", for the description.
    pub what: String,
    /// Third-party plugins were taken out of the render.
    pub plugins_left_out: bool,
}

/// Where every enabled clip of the project ends (0 for an empty project).
pub fn content_end(p: &Project) -> Ticks {
    p.tracks
        .iter()
        .flat_map(|t| {
            let audio = t.clips.iter().filter(|c| !c.disabled).map(|c| c.start + p.clip_length_ticks(c));
            let midi = t.midi_clips.iter().filter(|c| !c.disabled).map(|c| c.end());
            audio.chain(midi)
        })
        .max()
        .unwrap_or(0)
}

/// Takes third-party plugins out of `p` (built-in devices stay); true if there were any.
pub fn strip_plugins(p: &mut Project) -> bool {
    let mut any = false;
    for t in &mut p.tracks {
        let before = t.devices.len();
        t.devices.retain(|d| builtin::def_of(d).is_some());
        any |= t.devices.len() != before;
        if t.instrument_plugin.as_ref().is_some_and(|d| builtin::def_of(d).is_none()) {
            t.instrument_plugin = None;
            any = true;
        }
    }
    any
}

impl Controller {
    /// The render for the next turn, or None when there is nothing to hear.
    pub fn listen_job(&self) -> Option<ListenJob> {
        let p = self.project();
        let rate = self.rate;
        let looped = p.transport.loop_enabled && p.transport.loop_end > p.transport.loop_start;
        let song = if looped { (p.transport.loop_start, p.transport.loop_end) } else { (0, content_end(p)) };
        let where_ = if looped { "over the loop region" } else { "from the start of the song" };
        let (mut project, start, end, what) = if let Some(span) = self.selected_clip.and_then(|c| render_plan::clip_span(p, c)) {
            let id = span.clips[0];
            let name = match (p.find_clip(id), p.find_midi_clip(id)) {
                (Some((t, c)), _) => p.tracks[t].clips[c].name.clone(),
                (_, Some((t, c))) => p.tracks[t].midi_clips[c].name.clone(),
                _ => String::new(),
            };
            (render_plan::render_project(p, &span, RenderMode::Stem), span.start, span.end, format!("the selected clip \"{name}\""))
        } else if let Some(t) = self.selected_track.and_then(|id| p.track(id))
            && matches!(t.kind, TrackKind::Audio | TrackKind::Instrument)
        {
            let clips = t.clips.iter().map(|c| c.id).chain(t.midi_clips.iter().map(|c| c.id)).collect();
            let span = ClipSpan { track: t.id, kind: t.kind, clips, start: song.0, end: song.1 };
            (render_plan::render_project(p, &span, RenderMode::Stem), song.0, song.1, format!("the selected track \"{}\" {where_}", t.name))
        } else {
            (p.clone(), song.0, song.1, format!("the whole mix {where_}"))
        };
        if end <= start {
            return None;
        }
        let map = project.tempo_map();
        let (f0, f1) = (map.ticks_to_frames(start, rate), map.ticks_to_frames(end, rate));
        let frames = ((f1 - f0).max(0) as usize).min((listen::MAX_SECONDS * rate) as usize);
        if frames == 0 {
            return None;
        }
        // A clip or track is measured as it leaves its own fader: the master fader and mute
        // would only make it quieter or silent.
        if !what.starts_with("the whole mix") {
            project.master_gain = 1.0;
            project.master_mute = false;
        }
        let plugins_left_out = strip_plugins(&mut project);
        Some(ListenJob {
            render: OfflineRender { project, rate, transients: self.transients_by_asset(), start: f0, frames },
            what,
            plugins_left_out,
        })
    }
}

/// Renders and measures `job` (worker thread): the paragraph the request carries.
pub fn describe(job: ListenJob) -> Result<String, String> {
    let rate = job.render.rate;
    let samples = offline::render(job.render, &AtomicBool::new(false), &AtomicUsize::new(0)).map_err(|e| match e {
        OfflineError::Cancelled => "cancelled".to_string(),
        OfflineError::Failed(e) => e,
    })?;
    let mut text = format!("Device listened to {}. {}", job.what, listen::analyze(&samples, rate).text());
    if job.plugins_left_out {
        text.push_str(" (Third-party plugins were left out of this measurement.)");
    }
    Ok(text)
}
