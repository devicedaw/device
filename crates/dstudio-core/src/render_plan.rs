//! What the clip context menu renders: the clips Consolidate joins, the project copy a
//! per-clip render plays (one track, only the chosen clips), and the MIDI merge.
//!
//! The render itself is the engine's offline render of a [`crate::snapshot`] built from
//! [`render_project`]; nothing here touches the engine.

use crate::builtin;
use crate::midi::{self, MidiClip};
use crate::{ClipId, Project, Ticks, Track, TrackId, TrackKind};

/// Track gain that makes the engine's constant-power pan law transparent at the centre: the law
/// scales both channels by cos(45°), so a render that must come out at the source level (and be
/// played again through the same track settings later) compensates by its inverse.
pub const CENTRE_UNITY_GAIN: f32 = std::f32::consts::SQRT_2;

/// A set of clips on one track and the time range they cover.
#[derive(Debug, Clone, PartialEq)]
pub struct ClipSpan {
    pub track: TrackId,
    pub kind: TrackKind,
    /// The clips, in start order.
    pub clips: Vec<ClipId>,
    pub start: Ticks,
    pub end: Ticks,
}

/// What a per-clip render plays besides the clips themselves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RenderMode {
    /// The clips' own material (consolidate): no devices, fader, pan or automation.
    Material,
    /// Through the track's devices with neutral fader and pan (bounce to a new track,
    /// which takes the fader and pan over itself).
    Bounce,
    /// As the track sounds in a stems export: devices, fader, pan and automation.
    Stem,
}

/// One enabled clip as (track, kind, start, end), or None for a missing, disabled or empty clip.
fn one(p: &Project, id: ClipId) -> Option<(TrackId, TrackKind, Ticks, Ticks)> {
    if let Some((ti, ci)) = p.find_clip(id) {
        let (t, c) = (&p.tracks[ti], &p.tracks[ti].clips[ci]);
        let end = c.start + p.clip_length_ticks(c);
        return (!c.disabled && end > c.start).then_some((t.id, TrackKind::Audio, c.start, end));
    }
    let (ti, ci) = p.find_midi_clip(id)?;
    let (t, c) = (&p.tracks[ti], &p.tracks[ti].midi_clips[ci]);
    (!c.disabled && c.length > 0).then_some((t.id, TrackKind::Instrument, c.start, c.end()))
}

/// The clip alone.
pub fn clip_span(p: &Project, id: ClipId) -> Option<ClipSpan> {
    let (track, kind, start, end) = one(p, id)?;
    Some(ClipSpan { track, kind, clips: vec![id], start, end })
}

/// Consolidate's clips: the clip and every enabled clip of its track that overlaps the range
/// the group covers so far, repeatedly, so nothing left behind overlaps the new clip. Clips
/// that only touch are not joined. Disabled clips stay where they are.
pub fn consolidate_group(p: &Project, id: ClipId) -> Option<ClipSpan> {
    let mut span = clip_span(p, id)?;
    let t = p.track(span.track)?;
    let ids: Vec<ClipId> = match span.kind {
        TrackKind::Audio => t.clips.iter().map(|c| c.id).collect(),
        TrackKind::Instrument => t.midi_clips.iter().map(|c| c.id).collect(),
        TrackKind::Return | TrackKind::Group => Vec::new(),
    };
    let all: Vec<(ClipId, Ticks, Ticks)> = ids.into_iter().filter_map(|c| one(p, c).map(|(_, _, s, e)| (c, s, e))).collect();
    let mut joined = vec![id];
    loop {
        let more: Vec<_> = all.iter().filter(|(c, s, e)| !joined.contains(c) && *s < span.end && *e > span.start).copied().collect();
        if more.is_empty() {
            break;
        }
        for (c, s, e) in more {
            joined.push(c);
            span.start = span.start.min(s);
            span.end = span.end.max(e);
        }
    }
    joined.sort_by_key(|c| all.iter().find(|(k, ..)| k == c).map_or(0, |(_, s, _)| *s));
    span.clips = joined;
    Some(span)
}

/// A copy of `p` for one render: `span.track` unmuted with only the span's clips, every
/// other track muted and emptied (no clips, samples, lanes or plugin devices: it is silent, so
/// loading or instantiating them would only cost start-up time; the rendered track's own plugins
/// stay, see dstudio_engine::offline), and the track and master settings `mode` asks for.
pub fn render_project(p: &Project, span: &ClipSpan, mode: RenderMode) -> Project {
    let mut r = p.clone();
    for t in &mut r.tracks {
        t.solo = false;
        if t.id != span.track {
            t.mute = true;
            t.devices.retain(|d| builtin::def_of(d).is_some());
            if t.instrument_plugin.as_ref().is_some_and(|d| builtin::def_of(d).is_none()) {
                t.instrument_plugin = None;
            }
            // A muted track plays nothing (not into a sidechain either), so the render needs none
            // of its clips or samples: loading them would only cost time, and a missing file
            // there would fail a render that never plays it. Its lanes go too, so a mute lane
            // can't bring it back.
            t.clips.clear();
            t.midi_clips.clear();
            t.automation.clear();
            for d in t.devices.iter_mut().chain(t.instrument_plugin.iter_mut()) {
                d.samples.clear();
            }
            continue;
        }
        t.mute = false;
        t.arm = false;
        t.monitor = false;
        // Renders of one track are dry: straight to the master, nothing into return tracks.
        t.sends.clear();
        t.output = None;
        t.clips.retain(|c| span.clips.contains(&c.id));
        t.midi_clips.retain(|c| span.clips.contains(&c.id));
        match mode {
            RenderMode::Material => {
                t.devices.clear();
                t.automation.clear();
                t.gain = CENTRE_UNITY_GAIN;
                t.pan = 0.0;
                (t.trim_db, t.width, t.mono) = (0.0, 1.0, false);
            }
            RenderMode::Bounce => {
                t.automation.retain(|l| !l.target.is_channel());
                t.gain = CENTRE_UNITY_GAIN;
                t.pan = 0.0;
                // The track keeps its trim, width and mono, so the bounce is taken before them.
                (t.trim_db, t.width, t.mono) = (0.0, 1.0, false);
            }
            RenderMode::Stem => {}
        }
    }
    // A stem sounds as the stems export does (through a muted master too); the clip renders
    // that become clips again are taken before the master, mute included.
    if mode != RenderMode::Stem {
        r.master_gain = 1.0;
        r.master_mute = false;
        r.master_automation.clear();
    }
    r
}

/// The audio track a Bounce of a clip on `src` lands on (without the clip): the render is taken
/// before the channel strip, so the new track takes over the fader, pan, trim, width, mono and
/// their lanes, and it sends and is routed where `src` is, so it sounds as the source did.
pub fn bounce_track(src: &Track, id: TrackId) -> Track {
    let mut t = Track::new(id, TrackKind::Audio, &format!("{} (Bounce)", src.name.trim()), src.color);
    (t.gain, t.pan, t.trim_db, t.width, t.mono) = (src.gain, src.pan, src.trim_db, src.width, src.mono);
    t.automation = src.automation.iter().filter(|l| l.target.is_channel()).cloned().collect();
    (t.sends, t.output) = (src.sends.clone(), src.output);
    t
}

/// Whether the track's sound outlasts its clips: it has an enabled device or instrument plugin,
/// or it is an instrument track (synth release). A consolidate render has no devices.
pub fn rings_out(p: &Project, track: TrackId, mode: RenderMode) -> bool {
    let Some(t) = p.track(track) else { return false };
    mode != RenderMode::Material
        && (t.kind == TrackKind::Instrument || t.devices.iter().chain(t.instrument_plugin.iter()).any(|d| d.enabled))
}

/// "<first> merged", or "Merged clip" when the first clip has no name.
pub fn merged_name(first: &str) -> String {
    let n = first.trim();
    if n.is_empty() { "Merged clip".into() } else { format!("{n} merged") }
}

/// The consolidated clip of a MIDI span: every clip's notes moved to the span start and cut at
/// the end of their clip (notes past a clip's end are not played). Pattern clips merge as the
/// notes they play (repeated over the clip, without muted rows); the result is a plain MIDI clip.
pub fn merge_midi(p: &Project, span: &ClipSpan) -> Option<MidiClip> {
    let t = p.track(span.track)?;
    let clips: Vec<&MidiClip> = span.clips.iter().filter_map(|id| t.midi_clips.iter().find(|c| c.id == *id)).collect();
    let first = clips.first()?;
    let mut notes = Vec::new();
    for c in &clips {
        for n in c.playing_notes().into_iter().filter(|n| n.start >= 0 && n.start < c.length) {
            let mut m = n;
            m.length = n.length.min(c.length - n.start).max(1);
            m.start = n.start + c.start - span.start;
            notes.push(m);
        }
    }
    midi::sort_notes(&mut notes);
    for (i, n) in notes.iter_mut().enumerate() {
        n.id = i as u32 + 1;
    }
    Some(MidiClip {
        id: ClipId(0),
        name: merged_name(&first.name),
        start: span.start,
        length: span.end - span.start,
        notes,
        pattern: None,
        disabled: false,
        composer: None,
        scale: None,
        clip_loop: None,
    })
}
