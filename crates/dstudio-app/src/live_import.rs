//! Ableton Live set reader for File › Import Ableton Live Set…: [parse] reads the gzipped XML
//! Live saves (Live 11 and 12) and [build]
//! turns it into a DStudio project. Arrangement clips come in; looped clips repeat their loop
//! the way Live plays them. What DStudio can't hold yet is counted in [Report].
use crate::live_set::{child, number, path, value};
use dstudio_core::{
    Asset, AssetId, Clip, ClipId, ClipWarp, Device, DeviceId, MidiClip, Note, PPQ, PluginFormat, PluginKind, PluginRef, Project, Track,
    TrackId, TrackKind, WarpMarker, WarpMode,
};
use std::path::{Path, PathBuf};

pub struct ParsedSet {
    pub bpm: f64,
    pub numerator: u8,
    pub denominator: u8,
    pub loop_on: bool,
    pub loop_start: f64,
    pub loop_length: f64,
    pub tracks: Vec<ParsedTrack>,
    pub group_tracks: usize,
    pub return_tracks: usize,
    pub session_clips: usize,
    pub devices: usize,
}

pub struct ParsedTrack {
    pub name: String,
    pub midi: bool,
    pub on: bool,
    pub volume: f32,
    pub pan: f32,
    pub audio: Vec<ParsedAudioClip>,
    pub midi_clips: Vec<ParsedMidiClip>,
    /// The track's instrument when it is a Drum Rack.
    pub drums: Option<ParsedDrums>,
    /// VST3 and Audio Unit plugins in the track's chain, in order.
    pub plugins: Vec<ParsedPlugin>,
}

/// A plugin in a Live chain, named as DStudio's scanner names it.
pub struct ParsedPlugin {
    pub plugin: PluginRef,
    pub on: bool,
}

/// A Drum Rack: its pads that play a Simpler sample, and what can't come in.
#[derive(Default)]
pub struct ParsedDrums {
    pub pads: Vec<ParsedPad>,
    /// Pads on notes outside DStudio's sixteen (C1 to D#2).
    pub outside: usize,
    /// Pads in a choke group (DStudio's Drum Rack has none).
    pub choked: usize,
    /// Pads that play something other than one Simpler sample.
    pub other: usize,
}

/// A Drum Rack pad that plays a Simpler sample.
pub struct ParsedPad {
    pub note: u8,
    pub on: bool,
    /// Simpler volume plus the pad's mixer volume.
    pub gain_db: f32,
    pub pan: f32,
    pub transpose: f32,
    /// Sample window and fades as fractions of the sample and of the window.
    pub start: f32,
    pub end: f32,
    pub fade_in: f32,
    pub fade_out: f32,
    pub reverse: bool,
    /// Plays only while the note is held (Simpler's Classic mode, or One-Shot with Gate).
    pub gate: bool,
    pub sample: SampleRef,
}

/// Where a clip sits (arrangement beats) and which part of its content plays: Live's clip
/// Loop element, in beats for MIDI and warped audio and in seconds for unwarped audio.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Span {
    pub start: f64,
    pub end: f64,
    pub loop_on: bool,
    pub loop_start: f64,
    pub loop_end: f64,
    pub start_relative: f64,
}

pub struct ParsedAudioClip {
    pub name: String,
    pub span: Span,
    pub disabled: bool,
    pub warped: bool,
    pub warp_mode: u32,
    /// (seconds into the file, beat), as Live's WarpMarkers.
    pub markers: Vec<(f64, f64)>,
    pub gain: f32,
    pub sample: SampleRef,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct SampleRef {
    /// Relative to the set's folder when Live stored it that way (RelativePathType 3).
    pub relative: Option<String>,
    pub absolute: String,
}

pub struct ParsedMidiClip {
    pub name: String,
    pub span: Span,
    pub disabled: bool,
    /// (pitch, time in clip beats, duration, velocity); notes switched off in Live are left out.
    pub notes: Vec<(u8, f64, f64, u8)>,
}

fn flag(n: roxmltree::Node, p: &str) -> bool {
    value(n, p).is_ok_and(|v| v == "true")
}

fn elements<'a, 'i>(n: roxmltree::Node<'a, 'i>) -> impl Iterator<Item = roxmltree::Node<'a, 'i>> {
    n.children().filter(|c| c.is_element())
}

fn sample_ref(file: roxmltree::Node) -> SampleRef {
    let relative = value(file, "RelativePath")
        .ok()
        .filter(|r| !r.is_empty() && value(file, "RelativePathType").is_ok_and(|t| t != "0"))
        .map(str::to_string);
    SampleRef { relative, absolute: value(file, "Path").unwrap_or("").to_string() }
}

/// A VST3 or Audio Unit plugin device. VST3: the class id's four 32-bit words (Uid/Fields.N)
/// as 32 hex digits; DeviceType 1 is an instrument. Audio Unit: the component's type, subtype
/// and manufacturer codes as four characters each; type aumu is an instrument.
fn plugin(d: roxmltree::Node) -> Option<ParsedPlugin> {
    if !d.has_tag_name("PluginDevice") && !d.has_tag_name("AuPluginDevice") {
        return None;
    }
    let info = path(d, "PluginDesc").and_then(|p| elements(p).next())?;
    let name = value(info, "Name").unwrap_or("").trim().to_string();
    let (format, id, vendor, instrument) = match info.tag_name().name() {
        "Vst3PluginInfo" => {
            let words: Vec<u32> = path(info, "Uid")
                .into_iter()
                .flat_map(elements)
                .filter_map(|f| f.attribute("Value")?.parse::<i64>().ok())
                .map(|w| w as u32)
                .collect();
            if words.len() != 4 {
                return None;
            }
            (
                PluginFormat::Vst3,
                words.iter().map(|w| format!("{w:08X}")).collect::<String>(),
                String::new(),
                num(info, "DeviceType") == Some(1.0),
            )
        }
        "AuPluginInfo" => {
            let code = |p: &str| num(info, p).map(|v| (v as i64 as u32).to_be_bytes().iter().map(|&b| b as char).collect::<String>());
            let (kind, sub, maker) = (code("ComponentType")?, code("ComponentSubType")?, code("ComponentManufacturer")?);
            let instrument = kind == "aumu";
            (
                PluginFormat::AudioUnit,
                format!("{kind} {sub} {maker}"),
                value(info, "Manufacturer").unwrap_or("").trim().to_string(),
                instrument,
            )
        }
        _ => return None,
    };
    let kind = if instrument { PluginKind::Instrument } else { PluginKind::Effect };
    let name = if name.is_empty() { "Plugin".into() } else { name };
    Some(ParsedPlugin {
        plugin: PluginRef { format, id, path: None, name, vendor, version: String::new(), kind },
        on: value(d, "On/Manual").map_or(true, |v| v == "true"),
    })
}

/// A finite number, so a corrupt value (NaN, inf) falls back to its default.
fn num(n: roxmltree::Node, p: &str) -> Option<f64> {
    number(n, p).ok().filter(|v| v.is_finite())
}

/// A Drum Rack's pads. Live stores a pad's note as 128 − note (BranchInfo/ReceivingNote).
fn drums(rack: roxmltree::Node) -> ParsedDrums {
    let mut out = ParsedDrums::default();
    for b in path(rack, "Branches").into_iter().flat_map(elements).filter(|b| b.has_tag_name("DrumBranch")) {
        let note = 128i32.saturating_sub(num(b, "BranchInfo/ReceivingNote").unwrap_or(0.0) as i32);
        if !(dstudio_core::builtin::drum_rack::FIRST_NOTE as i32
            ..dstudio_core::builtin::drum_rack::FIRST_NOTE as i32 + dstudio_core::builtin::drum_rack::PADS as i32)
            .contains(&note)
        {
            out.outside += 1;
            continue;
        }
        let devices: Vec<_> = path(b, "DeviceChain/MidiToAudioDeviceChain/Devices").into_iter().flat_map(elements).collect();
        let (Some(s), 1) = (devices.iter().find(|d| d.has_tag_name("OriginalSimpler")), devices.len()) else {
            out.other += 1;
            continue;
        };
        let parts: Vec<_> = path(*s, "Player/MultiSampleMap/SampleParts").into_iter().flat_map(elements).collect();
        let (Some(part), Some(file)) = (parts.first(), parts.first().and_then(|p| path(*p, "SampleRef/FileRef"))) else {
            out.other += 1;
            continue;
        };
        if parts.len() > 1 || num(*s, "Globals/PlaybackMode").is_some_and(|m| m == 2.0) || out.pads.iter().any(|p| p.note == note as u8) {
            out.other += 1;
            continue;
        }
        if num(b, "BranchInfo/ChokeGroup").unwrap_or(0.0) > 0.0 {
            out.choked += 1;
        }
        let frames = num(*part, "SampleRef/DefaultDuration").unwrap_or(0.0).max(1.0);
        let rate = num(*part, "SampleRef/DefaultSampleRate").unwrap_or(44_100.0).max(1.0);
        let start = (num(*part, "SampleStart").unwrap_or(0.0) / frames).clamp(0.0, 1.0);
        let end = ((num(*part, "SampleEnd").unwrap_or(frames) + 1.0) / frames).clamp(start, 1.0);
        let window_ms = ((end - start) * frames / rate * 1000.0).max(1e-3);
        let fade = |p: &str| (num(*s, p).unwrap_or(0.0) / window_ms * 100.0).clamp(0.0, 100.0) as f32;
        let one_shot = num(*s, "Globals/PlaybackMode").is_some_and(|m| m == 1.0);
        let mixer_gain = num(b, "MixerDevice/Volume/Manual").unwrap_or(1.0).max(1e-6);
        out.pads.push(ParsedPad {
            note: note as u8,
            on: value(b, "MixerDevice/Speaker/Manual").map_or(true, |v| v == "true")
                && value(*s, "On/Manual").map_or(true, |v| v == "true"),
            gain_db: (num(*s, "VolumeAndPan/Volume/Manual").unwrap_or(0.0) + 20.0 * mixer_gain.log10()) as f32,
            pan: (num(*s, "VolumeAndPan/Panorama/Manual").unwrap_or(0.0) + num(b, "MixerDevice/Panorama/Manual").unwrap_or(0.0)) as f32,
            transpose: (num(*s, "Pitch/TransposeKey/Manual").unwrap_or(0.0) + num(*s, "Pitch/TransposeFine/Manual").unwrap_or(0.0) / 100.0)
                .round() as f32,
            start: start as f32,
            end: end as f32,
            fade_in: if one_shot { fade("VolumeAndPan/OneShotEnvelope/FadeInTime/Manual") } else { 0.0 },
            fade_out: if one_shot { fade("VolumeAndPan/OneShotEnvelope/FadeOutTime/Manual") } else { 0.0 },
            reverse: flag(*s, "Player/Reverse/Manual"),
            gate: !one_shot || num(*s, "VolumeAndPan/OneShotEnvelope/SustainMode/Manual").is_some_and(|m| m == 1.0),
            sample: sample_ref(file),
        });
    }
    out
}

fn span(c: roxmltree::Node) -> Result<Span, String> {
    let s = Span {
        start: number(c, "CurrentStart")?,
        end: number(c, "CurrentEnd")?,
        loop_on: flag(c, "Loop/LoopOn"),
        loop_start: number(c, "Loop/LoopStart")?,
        loop_end: number(c, "Loop/LoopEnd")?,
        start_relative: number(c, "Loop/StartRelative").unwrap_or(0.0),
    };
    #[allow(clippy::neg_cmp_op_on_partial_ord, reason = "NaN bounds must be rejected too")]
    if !(s.end > s.start) || !(s.loop_end > s.loop_start || !s.loop_on) {
        return Err(format!("clip {} has an empty span", c.attribute("Id").unwrap_or("?")));
    }
    Ok(s)
}

/// Read a .als file (gzipped or plain XML).
pub fn read_file(file: &Path) -> Result<ParsedSet, String> {
    let bytes = std::fs::read(file).map_err(|e| e.to_string())?;
    let xml = if bytes.starts_with(&[0x1f, 0x8b]) {
        let mut s = String::new();
        std::io::Read::read_to_string(&mut flate2::read::GzDecoder::new(&bytes[..]), &mut s).map_err(|e| format!("not a Live set: {e}"))?;
        s
    } else {
        String::from_utf8(bytes).map_err(|_| "not a Live set".to_string())?
    };
    parse(&xml)
}

pub fn parse(xml: &str) -> Result<ParsedSet, String> {
    let doc = roxmltree::Document::parse(xml).map_err(|e| format!("not a Live set: {e}"))?;
    let root = doc.root_element();
    if !root.has_tag_name("Ableton") {
        return Err("not an Ableton Live set".into());
    }
    let set = child(root, "LiveSet").ok_or("the set has no LiveSet")?;
    let main = child(set, "MainTrack").or_else(|| child(set, "MasterTrack")).ok_or("the set has no main track")?;
    let ts = number(main, "DeviceChain/Mixer/TimeSignature/Manual").unwrap_or(201.0) as u32;
    let mut out = ParsedSet {
        bpm: number(main, "DeviceChain/Mixer/Tempo/Manual")?,
        numerator: (ts % 99 + 1).min(99) as u8,
        denominator: 1u32.checked_shl(ts / 99).unwrap_or(4).min(64) as u8,
        loop_on: flag(set, "Transport/LoopOn"),
        loop_start: number(set, "Transport/LoopStart").unwrap_or(0.0),
        loop_length: number(set, "Transport/LoopLength").unwrap_or(16.0),
        tracks: Vec::new(),
        group_tracks: 0,
        return_tracks: 0,
        session_clips: 0,
        devices: 0,
    };
    for t in elements(path(set, "Tracks").ok_or("the set has no Tracks")?) {
        let midi = match t.tag_name().name() {
            "AudioTrack" => false,
            "MidiTrack" => true,
            "GroupTrack" => {
                out.group_tracks += 1;
                continue;
            }
            "ReturnTrack" => {
                out.return_tracks += 1;
                continue;
            }
            _ => continue,
        };
        out.session_clips += path(t, "DeviceChain/MainSequencer/ClipSlotList")
            .map_or(0, |l| l.descendants().filter(|n| n.has_tag_name("AudioClip") || n.has_tag_name("MidiClip")).count());
        let devices: Vec<_> = path(t, "DeviceChain/DeviceChain/Devices").into_iter().flat_map(elements).collect();
        let rack = devices.iter().find(|d| midi && d.has_tag_name("DrumGroupDevice"));
        let plugins: Vec<ParsedPlugin> = devices.iter().filter_map(|d| plugin(*d)).collect();
        out.devices += devices.len() - rack.is_some() as usize - plugins.len();
        let name = value(t, "Name/EffectiveName").or_else(|_| value(t, "Name/UserName")).unwrap_or("").trim().to_string();
        let mut track = ParsedTrack {
            name: if name.is_empty() { if midi { "MIDI" } else { "Audio" }.into() } else { name },
            midi,
            on: value(t, "DeviceChain/Mixer/Speaker/Manual").map_or(true, |v| v == "true"),
            volume: number(t, "DeviceChain/Mixer/Volume/Manual").unwrap_or(1.0) as f32,
            pan: number(t, "DeviceChain/Mixer/Pan/Manual").unwrap_or(0.0) as f32,
            audio: Vec::new(),
            midi_clips: Vec::new(),
            drums: rack.map(|r| drums(*r)),
            plugins,
        };
        let events = if midi {
            "DeviceChain/MainSequencer/ClipTimeable/ArrangerAutomation/Events"
        } else {
            "DeviceChain/MainSequencer/Sample/ArrangerAutomation/Events"
        };
        for c in path(t, events).into_iter().flat_map(elements) {
            let name = value(c, "Name").unwrap_or("").to_string();
            let disabled = flag(c, "Disabled");
            match (midi, c.tag_name().name()) {
                (false, "AudioClip") => {
                    let file = path(c, "SampleRef/FileRef").ok_or("an audio clip has no file")?;
                    let mut markers: Vec<(f64, f64)> = path(c, "WarpMarkers")
                        .into_iter()
                        .flat_map(elements)
                        .filter_map(|m| Some((m.attribute("SecTime")?.parse().ok()?, m.attribute("BeatTime")?.parse().ok()?)))
                        .collect();
                    markers.sort_by(|a, b| a.0.total_cmp(&b.0));
                    track.audio.push(ParsedAudioClip {
                        name,
                        span: span(c)?,
                        disabled,
                        warped: flag(c, "IsWarped"),
                        warp_mode: number(c, "WarpMode").unwrap_or(0.0) as u32,
                        markers,
                        gain: number(c, "SampleVolume").unwrap_or(1.0) as f32,
                        sample: sample_ref(file),
                    });
                }
                (true, "MidiClip") => {
                    let mut notes = Vec::new();
                    for k in path(c, "Notes/KeyTracks").into_iter().flat_map(elements) {
                        let Ok(key) = number(k, "MidiKey") else { continue };
                        for n in child(k, "Notes").into_iter().flat_map(elements) {
                            let attr = |a: &str| n.attribute(a).and_then(|v| v.parse::<f64>().ok());
                            if n.attribute("IsEnabled") == Some("false") {
                                continue;
                            }
                            if let (Some(time), Some(duration), Some(velocity)) = (attr("Time"), attr("Duration"), attr("Velocity")) {
                                notes.push((key.clamp(0.0, 127.0) as u8, time, duration, velocity.round().clamp(1.0, 127.0) as u8));
                            }
                        }
                    }
                    notes.sort_by(|a, b| a.1.total_cmp(&b.1).then(a.0.cmp(&b.0)));
                    track.midi_clips.push(ParsedMidiClip { name, span: span(c)?, disabled, notes });
                }
                _ => {}
            }
        }
        out.tracks.push(track);
    }
    Ok(out)
}

/// The parts of a clip's content the arrangement plays, as (content from, content to, beats
/// after the clip start). A looped clip plays from its start marker to the loop end, then the
/// loop until the clip ends. `scale` converts content units to beats (1 for beats).
fn pieces(s: &Span, scale: f64) -> Vec<(f64, f64, f64)> {
    let total = s.end - s.start;
    let first = s.loop_start + if s.loop_on { s.start_relative } else { 0.0 };
    if !s.loop_on {
        return vec![(first, first + total / scale, 0.0)];
    }
    let mut out = Vec::new();
    let (mut from, mut at) = (first.min(s.loop_end - 1e-9), 0.0);
    while at < total - 1e-9 && out.len() < 4096 {
        let to = (from + (total - at) / scale).min(s.loop_end);
        out.push((from, to, at));
        at += (to - from) * scale;
        from = s.loop_start;
    }
    out
}

/// A sample found for the import: where it is and what the engine decoded.
pub struct Sample {
    pub path: PathBuf,
    pub sample_rate: f64,
    pub channels: u32,
    pub frames: u64,
}

/// The file a clip's sample ref names: next to the set first, then where Live last saw it.
pub fn find_sample(set_dir: &Path, r: &SampleRef) -> Option<PathBuf> {
    r.relative
        .as_ref()
        .map(|rel| set_dir.join(rel))
        .filter(|p| p.is_file())
        .or_else(|| Some(PathBuf::from(&r.absolute)).filter(|p| !r.absolute.is_empty() && p.is_file()))
}

/// What an import left out, for the status line.
#[derive(Debug, Default, PartialEq)]
pub struct Report {
    pub tracks: usize,
    pub clips: usize,
    pub missing_samples: Vec<String>,
    pub group_tracks: usize,
    pub return_tracks: usize,
    pub session_clips: usize,
    pub devices: usize,
    /// Drum Rack pads outside C1–D#2 or playing something other than one Simpler sample.
    pub drum_pads: usize,
    /// Drum Rack pads in a choke group (they come in without it).
    pub choked_pads: usize,
    /// Plugins brought in (with their default settings: Live's plugin state isn't read).
    pub plugins: usize,
    /// Of those, the ones not installed here (placeholders).
    pub missing_plugins: Vec<String>,
}

impl Report {
    pub fn summary(&self, name: &str) -> String {
        let mut left = Vec::new();
        let plural = |n: usize, one: &str| if n == 1 { format!("1 {one}") } else { format!("{n} {one}s") };
        if !self.missing_samples.is_empty() {
            left.push(format!("{} not found", plural(self.missing_samples.len(), "sample")));
        }
        if self.group_tracks > 0 {
            left.push(plural(self.group_tracks, "group track"));
        }
        if self.return_tracks > 0 {
            left.push(plural(self.return_tracks, "return track"));
        }
        if self.session_clips > 0 {
            left.push(plural(self.session_clips, "Session clip"));
        }
        if self.devices > 0 {
            left.push(plural(self.devices, "device"));
        }
        if self.drum_pads > 0 {
            left.push(plural(self.drum_pads, "Drum Rack pad"));
        }
        if self.choked_pads > 0 {
            left.push(format!("choke groups on {}", plural(self.choked_pads, "pad")));
        }
        if !self.missing_plugins.is_empty() {
            left.push(format!("{} not installed", plural(self.missing_plugins.len(), "plugin")));
        }
        if self.plugins > 0 {
            left.push(format!("the settings of {}", plural(self.plugins, "plugin")));
        }
        let head = format!("Imported {name}: {}, {}", plural(self.tracks, "track"), plural(self.clips, "clip"));
        if left.is_empty() { head } else { format!("{head}. Left out: {}", left.join(", ")) }
    }
}

fn ticks(beats: f64) -> i64 {
    (beats * PPQ as f64).round() as i64
}

/// Live's warp modes as DStudio's: Beats → Drums, Tones → Melody, Texture and Complex → Auto,
/// Re-Pitch → Re-pitch, Complex Pro → Melody.
fn warp_mode(live: u32) -> WarpMode {
    match live {
        0 => WarpMode::Drums,
        1 | 6 => WarpMode::Melody,
        3 => WarpMode::Repitch,
        _ => WarpMode::Auto,
    }
}

/// The DStudio project for a parsed set. `sample` gives the decoded file for a clip's sample
/// ref (None when it is missing). Returns the project and, per asset, the file it loads.
type Found = Option<(AssetId, f64, u64)>;

/// The asset for a sample ref, made the first time a file is used (two refs, relative and
/// absolute, can name one file). None, and listed as missing, when it wasn't found.
fn asset_for(
    p: &mut Project,
    assets: &mut Vec<(SampleRef, Found)>,
    files: &mut Vec<(AssetId, PathBuf)>,
    report: &mut Report,
    sample: &dyn Fn(&SampleRef) -> Option<Sample>,
    r: &SampleRef,
) -> Found {
    if let Some((_, a)) = assets.iter().find(|(x, _)| x == r) {
        return *a;
    }
    let a = sample(r).map(|s| {
        let path = s.path.to_string_lossy().into_owned();
        if let Some(a) = p.assets.iter().find(|a| a.absolute_path == path) {
            return (a.id, a.sample_rate, a.frames);
        }
        let id = AssetId(p.alloc_id());
        p.assets.push(Asset {
            id,
            relative_path: None,
            absolute_path: path,
            sample_rate: s.sample_rate,
            channels: s.channels.max(1),
            frames: s.frames,
        });
        files.push((id, s.path));
        (id, s.sample_rate, s.frames)
    });
    if a.is_none() {
        report.missing_samples.push(r.relative.clone().unwrap_or_else(|| r.absolute.clone()));
    }
    assets.push((r.clone(), a));
    a
}

/// DStudio's Drum Rack for a Live one: pad n plays the Live pad on note 35 + n, its sample in the
/// pad's first slot.
fn drum_rack(
    p: &mut Project,
    assets: &mut Vec<(SampleRef, Found)>,
    files: &mut Vec<(AssetId, PathBuf)>,
    report: &mut Report,
    sample: &dyn Fn(&SampleRef) -> Option<Sample>,
    d: &ParsedDrums,
) -> dstudio_core::Device {
    use dstudio_core::builtin::drum_rack::{DRUM_RACK, FIRST_NOTE};
    let mut dev = DRUM_RACK.instantiate();
    dev.id = dstudio_core::DeviceId(p.alloc_id());
    report.drum_pads += d.outside + d.other;
    report.choked_pads += d.choked;
    for pad in &d.pads {
        let n = (pad.note - FIRST_NOTE) as u32 + 1;
        let Some((asset, _, _)) = asset_for(p, assets, files, report, sample, &pad.sample) else { continue };
        let name = pad
            .sample
            .relative
            .as_deref()
            .unwrap_or(&pad.sample.absolute)
            .rsplit(['/', '\\'])
            .next()
            .unwrap_or("")
            .rsplit_once('.')
            .map_or("Sample", |(stem, _)| stem)
            .to_string();
        dev.samples.push(dstudio_core::DeviceSample {
            slot: (n - 1) * dstudio_core::builtin::pad_sampler::LAYERS,
            asset,
            name,
            slices: Vec::new(),
        });
        let flag = |b: bool| if b { 1.0 } else { 0.0 };
        for (k, v) in [
            ("on", flag(pad.on)),
            ("gain", pad.gain_db.clamp(-60.0, 12.0)),
            ("pan", pad.pan.clamp(-1.0, 1.0)),
            ("transpose", pad.transpose.clamp(-24.0, 24.0)),
            ("fadeIn", pad.fade_in),
            ("fadeOut", pad.fade_out),
            ("start", pad.start),
            ("end", pad.end),
            ("reverse", flag(pad.reverse)),
            ("gate", flag(pad.gate)),
        ] {
            dev.params.insert(format!("p{n}_{k}"), v);
        }
    }
    dev.samples.sort_by_key(|s| s.slot);
    dev
}

/// `installed` gives the scanned plugin for a Live one; a plugin that isn't installed comes in
/// as a placeholder device that the engine can't load (its card says so).
pub fn build(
    set: &ParsedSet,
    name: &str,
    sample: &dyn Fn(&SampleRef) -> Option<Sample>,
    installed: &dyn Fn(&PluginRef) -> Option<PluginRef>,
) -> (Project, Vec<(AssetId, PathBuf)>, Report) {
    let mut p = Project::empty(name);
    p.transport.bpm = set.bpm.clamp(20.0, 999.0);
    p.transport.time_signature.numerator = set.numerator.max(1);
    p.transport.time_signature.denominator = set.denominator.max(1);
    p.transport.loop_enabled = set.loop_on;
    p.transport.loop_start = ticks(set.loop_start.max(0.0));
    p.transport.loop_end = p.transport.loop_start + ticks(set.loop_length.max(1.0));
    let mut report = Report {
        group_tracks: set.group_tracks,
        return_tracks: set.return_tracks,
        session_clips: set.session_clips,
        devices: set.devices,
        ..Default::default()
    };
    let mut assets: Vec<(SampleRef, Option<(AssetId, f64, u64)>)> = Vec::new();
    let mut files = Vec::new();
    for t in &set.tracks {
        let id = TrackId(p.alloc_id());
        let kind = if t.midi { TrackKind::Instrument } else { TrackKind::Audio };
        let mut track = Track::new(id, kind, &t.name, crate::controller::next_track_color(&p));
        track.gain = t.volume.clamp(0.0, dstudio_core::command::MAX_GAIN);
        track.pan = t.pan.clamp(-1.0, 1.0);
        track.mute = !t.on;
        for c in &t.audio {
            let Some((asset, rate, frames)) = asset_for(&mut p, &mut assets, &mut files, &mut report, sample, &c.sample) else { continue };
            let mut warp = ClipWarp::default();
            let scale = if c.warped {
                let mut last = (-1.0f64, f64::NEG_INFINITY);
                for &(sec, beat) in &c.markers {
                    let source = (sec * rate).round();
                    if source > last.0 && beat > last.1 && source >= 0.0 && source <= frames as f64 {
                        warp.markers.push(WarpMarker { source: source as u64, beat });
                        last = (source, beat);
                    }
                }
                if let [a, .., b] = warp.markers.as_slice() {
                    warp.original_bpm = ((b.beat - a.beat) / ((b.source - a.source) as f64 / rate) * 60.0)
                        .clamp(dstudio_core::warp::MIN_ORIGINAL_BPM, dstudio_core::warp::MAX_ORIGINAL_BPM);
                }
                warp.enabled = true;
                warp.mode = warp_mode(c.warp_mode);
                1.0
            } else {
                set.bpm / 60.0
            };
            // A loop from the clip's start becomes the clip's own loop; one that starts later
            // (StartRelative) plays its first pass partly, so it comes in as repeated clips.
            let s = &c.span;
            let clip_loop = (s.loop_on && s.start_relative.abs() < 1e-9 && (s.end - s.start) > (s.loop_end - s.loop_start) * scale + 1e-9)
                .then(|| dstudio_core::AudioLoop {
                    on: true,
                    start: 0,
                    end: ticks((s.loop_end - s.loop_start) * scale),
                    length: ticks(s.end - s.start),
                })
                .filter(|l| l.validate().is_ok());
            let parts = if clip_loop.is_some() { vec![(s.loop_start, s.loop_end, 0.0)] } else { pieces(s, scale) };
            for (from, to, at) in parts {
                let (s0, s1) = if c.warped { (warp.source_at(from, rate), warp.source_at(to, rate)) } else { (from * rate, to * rate) };
                let (s0, s1) = (s0.round().clamp(0.0, frames as f64) as u64, s1.round().clamp(0.0, frames as f64) as u64);
                if s1 <= s0 {
                    continue;
                }
                let id = ClipId(p.alloc_id());
                track.clips.push(Clip {
                    id,
                    asset,
                    name: c.name.clone(),
                    start: ticks(c.span.start + at),
                    source_offset: s0,
                    source_length: s1 - s0,
                    gain: c.gain.clamp(0.0, dstudio_core::command::MAX_GAIN),
                    warp: warp.clone(),
                    disabled: c.disabled,
                    clip_loop,
                    fades: Default::default(),
                    takes: Vec::new(),
                });
                report.clips += 1;
            }
        }
        for c in &t.midi_clips {
            let s = &c.span;
            let length = ticks(s.end - s.start);
            let looped = s.loop_on && s.start_relative.abs() < 1e-9;
            let mut notes = Vec::new();
            let parts = if looped { vec![(s.loop_start, s.loop_end, 0.0)] } else { pieces(s, 1.0) };
            for (from, to, at) in parts {
                for &(pitch, time, duration, velocity) in c.notes.iter().filter(|n| n.1 >= from - 1e-9 && n.1 < to - 1e-9) {
                    let start = ticks(at + time - from);
                    let end = ticks(at + (time + duration).min(to) - from);
                    if start < length && end > start {
                        notes.push(Note { id: notes.len() as u32 + 1, pitch, start, length: end - start, velocity });
                    }
                }
            }
            let clip_loop = looped.then(|| dstudio_core::midi::ClipLoop {
                on: true,
                start: 0,
                end: ticks(s.loop_end - s.loop_start).min(length).max(1),
                span: 0,
            });
            let id = ClipId(p.alloc_id());
            track.midi_clips.push(MidiClip {
                id,
                name: c.name.clone(),
                start: ticks(s.start),
                length,
                notes,
                pattern: None,
                disabled: c.disabled,
                composer: None,
                scale: None,
                clip_loop,
            });
            report.clips += 1;
        }
        if let Some(d) = &t.drums {
            track.instrument_plugin = Some(drum_rack(&mut p, &mut assets, &mut files, &mut report, sample, d));
        }
        for pl in &t.plugins {
            let instrument = pl.plugin.kind == PluginKind::Instrument;
            if instrument && (!t.midi || track.instrument_plugin.is_some()) {
                report.devices += 1;
                continue;
            }
            let mut dev = Device::new(installed(&pl.plugin).unwrap_or_else(|| {
                report.missing_plugins.push(pl.plugin.name.clone());
                pl.plugin.clone()
            }));
            dev.id = DeviceId(p.alloc_id());
            dev.enabled = pl.on;
            report.plugins += 1;
            if instrument {
                track.instrument_plugin = Some(dev);
            } else {
                track.devices.push(dev);
            }
        }
        track.clips.sort_by_key(|c| c.start);
        track.midi_clips.sort_by_key(|c| c.start);
        p.tracks.push(track);
        report.tracks += 1;
    }
    (p, files, report)
}

/// Each sample file the set uses, decoded once, and the file each of the set's refs found.
pub struct Loaded {
    pub files: Vec<(PathBuf, dstudio_engine::AssetInfo, crate::peaks::Pyramid)>,
    pub refs: Vec<(SampleRef, PathBuf)>,
}

/// A read set and the samples found and decoded for it (worker thread). `revision` is the
/// document's when the import started.
pub struct LiveSetJob {
    pub file: PathBuf,
    pub doc_epoch: u64,
    pub revision: u64,
    pub result: Result<(ParsedSet, Loaded), String>,
}

/// An import waiting on the unsaved-changes prompt: a set still to read, or one read while the
/// project was edited.
pub enum PendingImport {
    File(PathBuf),
    Loaded(Box<LiveSetJob>),
}

impl crate::controller::Controller {
    /// File › Import Ableton Live Set…: replaces the open project, asking to save changes first.
    pub fn import_live_set_request(&mut self, file: PathBuf) {
        if self.doc.is_dirty() {
            self.pending_import = Some(PendingImport::File(file));
            self.unsaved_open = true;
            self.ui_dirty = true;
        } else {
            self.import_live_set(file);
        }
    }

    pub fn import_live_set(&mut self, file: PathBuf) {
        let engine = self.engine.clone();
        let tx = self.jobs_tx.clone();
        let (doc_epoch, revision) = (self.doc_epoch, self.doc.revision());
        self.pending_jobs += 1;
        std::thread::Builder::new()
            .name("live-set".into())
            .spawn(move || {
                let result = read_file(&file).map(|set| {
                    let dir = file.parent().unwrap_or(Path::new(".")).to_path_buf();
                    let mut loaded = Loaded { files: Vec::new(), refs: Vec::new() };
                    let pads = set.tracks.iter().flat_map(|t| t.drums.iter().flat_map(|d| d.pads.iter().map(|p| &p.sample)));
                    for r in set.tracks.iter().flat_map(|t| t.audio.iter().map(|c| &c.sample)).chain(pads) {
                        if loaded.refs.iter().any(|(x, _)| x == r) {
                            continue;
                        }
                        let Some(p) = find_sample(&dir, r) else { continue };
                        if !loaded.files.iter().any(|f| f.0 == p) {
                            let Ok((info, pyr)) = crate::controller::load_and_analyse(&engine, &p) else { continue };
                            loaded.files.push((p.clone(), info, pyr));
                        }
                        loaded.refs.push((r.clone(), p));
                    }
                    (set, loaded)
                });
                let _ = tx.send(crate::controller::JobResult::LiveSet(Box::new(LiveSetJob { file, doc_epoch, revision, result })));
            })
            .expect("spawn live set thread");
        self.set_status("Importing Live set…", 30);
    }

    pub(crate) fn release_live_set(&mut self, job: LiveSetJob) {
        if let Ok((_, loaded)) = job.result {
            for (_, info, _) in loaded.files {
                let _ = self.engine.release_asset(info.asset_id);
            }
        }
    }

    /// A finished read: replace the project, unless it was edited meanwhile, which asks first.
    pub(crate) fn finish_live_set(&mut self, job: LiveSetJob) {
        if let Err(e) = &job.result {
            return self.set_status(format!("Import failed: {e}"), 6);
        }
        if job.doc_epoch != self.doc_epoch {
            return self.release_live_set(job);
        }
        // The prompt already asks on behalf of something the user chose since: that wins.
        if self.unsaved_open {
            self.release_live_set(job);
            return self.set_status("Live set import cancelled", 4);
        }
        if self.doc.revision() != job.revision && self.doc.is_dirty() {
            self.pending_import = Some(PendingImport::Loaded(Box::new(job)));
            self.unsaved_open = true;
            self.ui_dirty = true;
            return;
        }
        self.apply_live_set(job);
    }

    pub(crate) fn apply_live_set(&mut self, job: LiveSetJob) {
        let Ok((set, loaded)) = job.result else { return };
        if job.doc_epoch != self.doc_epoch {
            for (_, info, _) in loaded.files {
                let _ = self.engine.release_asset(info.asset_id);
            }
            return;
        }
        let name = job.file.file_stem().map_or_else(|| "Live Set".into(), |s| s.to_string_lossy().into_owned());
        let rate = self.rate;
        let (project, files, report) = build(
            &set,
            &name,
            &|r| {
                let path = &loaded.refs.iter().find(|l| l.0 == *r)?.1;
                loaded.files.iter().find(|f| f.0 == *path).map(|(p, info, _)| Sample {
                    path: p.clone(),
                    sample_rate: info.source_sample_rate,
                    channels: info.channels,
                    frames: (info.frames as f64 * info.source_sample_rate / rate).round().max(1.0) as u64,
                })
            },
            &|r| self.devices.catalog.find(r).map(|i| i.plugin.clone()),
        );
        self.dialogs_close();
        self.stop_return();
        for (_, ea) in self.asset_map.drain().chain(self.reversed_assets.drain()) {
            let _ = self.engine.release_asset(ea);
        }
        self.project_dir = None;
        self.view = Default::default();
        self.replace_project(project);
        for (p, info, pyr) in loaded.files {
            match files.iter().find(|f| f.1 == p) {
                Some(&(asset, _)) => {
                    self.peaks.insert(info.asset_id, pyr);
                    self.asset_map.insert(asset, info.asset_id);
                }
                None => {
                    let _ = self.engine.release_asset(info.asset_id);
                }
            }
        }
        self.publish();
        self.set_status(report.summary(&name), 8);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::live_set::{LiveSet, SetAudioClip, SetContent, SetMidiClip, SetNote, SetTrack, to_xml};

    fn set() -> LiveSet {
        LiveSet {
            bpm: 100.0,
            tempo: Vec::new(),
            numerator: 3,
            denominator: 4,
            loop_on: true,
            loop_start: 8.0,
            loop_length: 16.0,
            returns: Vec::new(),
            tracks: vec![
                SetTrack {
                    name: "Break".into(),
                    color: 3,
                    on: true,
                    volume: 0.5,
                    pan: 0.25,
                    sends: Vec::new(),
                    sends_only: false,
                    content: SetContent::Audio(vec![SetAudioClip {
                        name: "break".into(),
                        start: 4.0,
                        length: 4.0,
                        relative_path: "Samples/break.wav".into(),
                        absolute_path: "/nowhere/Samples/break.wav".into(),
                        file_size: 1,
                        frames: 192_000,
                        sample_rate: 48_000,
                        markers: vec![(0.0, 0.0), (1.0, 2.0), (3.0, 4.0)],
                    }]),
                    devices: Vec::new(),
                },
                SetTrack {
                    name: "Keys".into(),
                    color: 9,
                    on: false,
                    volume: 1.0,
                    pan: 0.0,
                    sends: Vec::new(),
                    sends_only: false,
                    content: SetContent::Midi(vec![SetMidiClip {
                        name: "chords".into(),
                        start: 2.0,
                        length: 4.0,
                        notes: vec![
                            SetNote { pitch: 60, time: 0.0, duration: 1.0, velocity: 100 },
                            SetNote { pitch: 64, time: 3.5, duration: 1.0, velocity: 90 },
                        ],
                    }]),
                    devices: Vec::new(),
                },
            ],
        }
    }

    fn sample(r: &SampleRef) -> Option<Sample> {
        (r.relative.as_deref() == Some("Samples/break.wav")).then(|| Sample {
            path: "/s/break.wav".into(),
            sample_rate: 48_000.0,
            channels: 2,
            frames: 192_000,
        })
    }

    fn import(xml: &str) -> (Project, Report) {
        let (p, files, report) = build(&parse(xml).unwrap(), "Set", &sample, &|_| None);
        assert_eq!(files.len(), p.assets.len());
        (p, report)
    }

    #[test]
    fn a_set_from_our_exporter_comes_back() {
        let (p, report) = import(&to_xml(&set()));
        let t = &p.transport;
        assert_eq!((t.bpm, t.time_signature.numerator, t.time_signature.denominator), (100.0, 3, 4));
        assert_eq!((t.loop_enabled, t.loop_start, t.loop_end), (true, 8 * PPQ, 24 * PPQ));
        let [audio, keys] = &p.tracks[..] else { panic!("{} tracks", p.tracks.len()) };
        assert_eq!((audio.kind, audio.name.as_str(), audio.gain, audio.pan, audio.mute), (TrackKind::Audio, "Break", 0.5, 0.25, false));
        assert_eq!((keys.kind, keys.name.as_str(), keys.mute), (TrackKind::Instrument, "Keys", true));
        let c = &audio.clips[0];
        assert!(c.warp.enabled);
        assert_eq!(c.warp.markers.iter().map(|m| (m.source, m.beat)).collect::<Vec<_>>(), [(0, 0.0), (48_000, 2.0), (144_000, 4.0)]);
        assert_eq!((c.start, c.source_offset, c.source_length), (4 * PPQ, 0, 144_000));
        assert_eq!(p.assets[0].absolute_path, "/s/break.wav");
        let m = &keys.midi_clips[0];
        assert_eq!((m.start, m.length, m.clip_loop), (2 * PPQ, 4 * PPQ, None));
        assert_eq!(
            m.notes.iter().map(|n| (n.pitch, n.start, n.length, n.velocity)).collect::<Vec<_>>(),
            [(60, 0, PPQ, 100), (64, 7 * PPQ / 2, PPQ / 2, 90)]
        );
        assert_eq!(report.summary("Set"), "Imported Set: 2 tracks, 2 clips");
    }

    #[test]
    fn non_finite_numbers_fall_back() {
        let xml = to_xml(&set());
        let volume = r#"<Volume>"#;
        let at = xml.find(volume).unwrap();
        let manual = at + xml[at..].find(r#"<Manual Value="0.5" />"#).unwrap();
        let xml = format!("{}<Manual Value=\"NaN\" />{}", &xml[..manual], &xml[manual + r#"<Manual Value="0.5" />"#.len()..]);
        let (p, _) = import(&xml);
        assert_eq!(p.tracks[0].gain, 1.0);
    }

    #[test]
    fn an_unwarped_clip_plays_its_seconds() {
        let xml = to_xml(&set()).replace(r#"<IsWarped Value="true" />"#, r#"<IsWarped Value="false" />"#);
        let (p, _) = import(&xml);
        let c = &p.tracks[0].clips[0];
        assert!(!c.warp.enabled);
        assert_eq!((c.source_offset, c.source_length), (0, 115_200), "4 beats at 100 BPM are 2.4 s");
    }

    #[test]
    fn looped_clips_repeat_their_loop() {
        let s = Span { start: 0.0, end: 10.0, loop_on: true, loop_start: 1.0, loop_end: 4.0, start_relative: 1.0 };
        assert_eq!(pieces(&s, 1.0), [(2.0, 4.0, 0.0), (1.0, 4.0, 2.0), (1.0, 4.0, 5.0), (1.0, 3.0, 8.0)]);
        let xml = to_xml(&set())
            .replace(r#"<LoopOn Value="false" />"#, r#"<LoopOn Value="true" />"#)
            .replace(r#"<CurrentEnd Value="8" />"#, r#"<CurrentEnd Value="14" />"#);
        let (p, _) = import(&xml);
        let clips: Vec<(i64, u64, Option<(bool, i64, i64, i64)>)> =
            p.tracks[0].clips.iter().map(|c| (c.start, c.source_length, c.clip_loop.map(|l| (l.on, l.start, l.end, l.length)))).collect();
        assert_eq!(clips, [(4 * PPQ, 144_000, Some((true, 0, 4 * PPQ, 10 * PPQ)))], "a 10-beat clip of a 4-beat loop");
        let m = &p.tracks[1].midi_clips[0];
        assert_eq!(m.clip_loop.map(|l| (l.on, l.start, l.end)), Some((true, 0, 4 * PPQ)));
    }

    #[test]
    fn what_is_left_out_is_counted() {
        let mut s = set();
        if let SetContent::Audio(clips) = &mut s.tracks[0].content {
            clips[0].relative_path = "Samples/gone.wav".into();
        }
        let xml = to_xml(&s).replacen("<Tracks>", r#"<Tracks><GroupTrack Id="900" /><ReturnTrack Id="901" />"#, 1);
        let (p, report) = import(&xml);
        assert!(p.tracks[0].clips.is_empty() && p.assets.is_empty());
        assert_eq!((report.group_tracks, report.return_tracks, report.missing_samples.len()), (1, 1, 1));
        assert_eq!(report.summary("Set"), "Imported Set: 2 tracks, 1 clip. Left out: 1 sample not found, 1 group track, 1 return track");
    }

    #[test]
    fn two_refs_to_one_file_share_its_asset() {
        let mut s = set();
        if let SetContent::Audio(clips) = &mut s.tracks[0].content {
            let c = &clips[0];
            let other = SetAudioClip {
                name: "again".into(),
                start: 12.0,
                length: c.length,
                relative_path: "Elsewhere/break.wav".into(),
                absolute_path: c.absolute_path.clone(),
                file_size: 1,
                frames: c.frames,
                sample_rate: c.sample_rate,
                markers: Vec::new(),
            };
            clips.push(other);
        }
        let any = |_: &SampleRef| Some(Sample { path: "/s/break.wav".into(), sample_rate: 48_000.0, channels: 2, frames: 192_000 });
        let (p, files, _) = build(&parse(&to_xml(&s)).unwrap(), "Set", &any, &|_| None);
        let clips = &p.tracks[0].clips;
        assert_eq!((p.assets.len(), files.len(), clips.len()), (1, 1, 2));
        assert_eq!(clips[0].asset, clips[1].asset);
    }

    fn pad(note: u8, choke: u8, mode: u8, file: &str, devices: usize) -> String {
        let simpler = format!(
            r#"<OriginalSimpler Id="0"><On><Manual Value="true" /></On><Player><MultiSampleMap><SampleParts><MultiSamplePart Id="0"><SampleStart Value="0" /><SampleEnd Value="23999" /><SampleRef><FileRef><RelativePathType Value="3" /><RelativePath Value="{file}" /><Path Value="/nowhere/{file}" /></FileRef><DefaultDuration Value="48000" /><DefaultSampleRate Value="48000" /></SampleRef></MultiSamplePart></SampleParts></MultiSampleMap><Reverse><Manual Value="true" /></Reverse></Player><Pitch><TransposeKey><Manual Value="3" /></TransposeKey><TransposeFine><Manual Value="0" /></TransposeFine></Pitch><VolumeAndPan><Volume><Manual Value="-6" /></Volume><Panorama><Manual Value="0.2" /></Panorama><OneShotEnvelope><FadeInTime><Manual Value="10" /></FadeInTime><SustainMode><Manual Value="0" /></SustainMode><FadeOutTime><Manual Value="20" /></FadeOutTime></OneShotEnvelope></VolumeAndPan><Globals><PlaybackMode Value="{mode}" /></Globals></OriginalSimpler>"#
        );
        format!(
            r#"<DrumBranch Id="0"><BranchInfo><ReceivingNote Value="{}" /><ChokeGroup Value="{choke}" /></BranchInfo><DeviceChain><MidiToAudioDeviceChain><Devices>{}</Devices></MidiToAudioDeviceChain></DeviceChain><MixerDevice><Speaker><Manual Value="true" /></Speaker><Volume><Manual Value="0.5" /></Volume><Panorama><Manual Value="0.1" /></Panorama></MixerDevice></DrumBranch>"#,
            128 - note as u32,
            simpler.repeat(devices)
        )
    }

    #[test]
    fn a_drum_rack_becomes_ours() {
        let branches = [
            pad(36, 0, 1, "Samples/break.wav", 1),
            pad(37, 1, 0, "Samples/gone.wav", 1),
            pad(60, 0, 1, "Samples/break.wav", 1),
            pad(38, 0, 1, "Samples/break.wav", 2),
        ]
        .concat();
        let rack = format!(r#"<Devices><DrumGroupDevice Id="0"><Branches>{branches}</Branches></DrumGroupDevice></Devices>"#);
        let xml = to_xml(&set());
        let at = xml.find("<MidiTrack").unwrap();
        let at = at + xml[at..].find("<Devices />").unwrap();
        let (p, report) = import(&format!("{}{rack}{}", &xml[..at], &xml[at + "<Devices />".len()..]));
        let dev = p.tracks[1].instrument_plugin.as_ref().expect("a Drum Rack");
        assert_eq!(dev.samples.iter().map(|s| (s.slot, s.asset, s.name.as_str())).collect::<Vec<_>>(), [(0, p.assets[0].id, "break")]);
        let param = |k: &str| dev.params[k];
        assert!((param("p1_gain") + 12.02).abs() < 0.01, "Simpler -6 dB and a pad mixer at half gain");
        assert!((param("p1_pan") - 0.3).abs() < 1e-6);
        assert_eq!(
            [param("p1_on"), param("p1_transpose"), param("p1_start"), param("p1_end"), param("p1_reverse"), param("p1_gate")],
            [1.0, 3.0, 0.0, 0.5, 1.0, 0.0]
        );
        assert!((param("p1_fadeIn") - 2.0).abs() < 1e-3 && (param("p1_fadeOut") - 4.0).abs() < 1e-3, "10 and 20 ms of a 500 ms window");
        assert_eq!((report.drum_pads, report.choked_pads, report.missing_samples.len(), report.devices), (2, 1, 1, 0));
        let summary = report.summary("Set");
        assert!(summary.contains("2 Drum Rack pads") && summary.contains("choke groups on 1 pad"), "{summary}");
    }

    #[test]
    fn a_corrupt_pad_number_falls_back() {
        let rack = format!(
            r#"<DrumGroupDevice Id="0"><Branches>{}</Branches></DrumGroupDevice>"#,
            pad(36, 0, 1, "Samples/break.wav", 1).replace(r#"<SampleStart Value="0" />"#, r#"<SampleStart Value="NaN" />"#)
        );
        let doc = roxmltree::Document::parse(&rack).unwrap();
        let d = drums(doc.root_element());
        assert_eq!((d.pads[0].start, d.pads[0].end), (0.0, 0.5));
    }

    /// `xml` with `devices` in the first track of `tag`.
    fn with_devices(xml: &str, tag: &str, devices: &str) -> String {
        let at = xml.find(tag).unwrap();
        let at = at + xml[at..].find("<Devices />").unwrap();
        format!("{}<Devices>{devices}</Devices>{}", &xml[..at], &xml[at + "<Devices />".len()..])
    }

    fn vst3(words: [i64; 4], name: &str, device_type: u8, on: bool) -> String {
        let fields: String = words.iter().enumerate().map(|(i, w)| format!(r#"<Fields.{i} Value="{w}" />"#)).collect();
        format!(
            r#"<PluginDevice Id="0"><On><Manual Value="{on}" /></On><PluginDesc><Vst3PluginInfo Id="0"><Name Value="{name}" /><Uid>{fields}</Uid><DeviceType Value="{device_type}" /></Vst3PluginInfo></PluginDesc></PluginDevice>"#
        )
    }

    #[test]
    fn plugins_come_in_as_devices_or_placeholders() {
        let au = r#"<AuPluginDevice Id="0"><On><Manual Value="true" /></On><PluginDesc><AuPluginInfo Id="0"><Name Value="Echo" /><Manufacturer Value="Us" /><ComponentType Value="1635083896" /><ComponentSubType Value="1684368505" /><ComponentManufacturer Value="1970496032" /></AuPluginInfo></PluginDesc></AuPluginDevice>"#;
        let xml = to_xml(&set());
        let xml = with_devices(
            &xml,
            "<AudioTrack",
            &format!(r#"{au}{}<Eq8 Id="0" />{}"#, vst3([1, 2, 3, -1], "Squash", 2, false), vst3([5, 6, 7, 8], "Synth", 1, true)),
        );
        let xml = with_devices(&xml, "<MidiTrack", &vst3([0x5653_5400, 0, 0, 1], "Keys Synth", 1, true));
        let installed = |r: &PluginRef| {
            (r.id == "aufx dely usb ").then(|| PluginRef {
                path: Some("/Library/Echo.component".into()),
                name: "Echo Installed".into(),
                ..r.clone()
            })
        };
        let (p, _, report) = build(&parse(&xml).unwrap(), "Set", &sample, &installed);
        let fx: Vec<(PluginFormat, &str, &str, bool)> =
            p.tracks[0].devices.iter().map(|d| (d.plugin.format, d.plugin.id.as_str(), d.plugin.name.as_str(), d.enabled)).collect();
        assert_eq!(
            fx,
            [
                (PluginFormat::AudioUnit, "aufx dely usb ", "Echo Installed", true),
                (PluginFormat::Vst3, "000000010000000200000003FFFFFFFF", "Squash", false)
            ]
        );
        assert!(p.tracks[0].instrument_plugin.is_none(), "an instrument on an audio track is left out");
        let keys = p.tracks[1].instrument_plugin.as_ref().expect("the MIDI track's instrument");
        assert_eq!(
            (keys.plugin.kind, keys.plugin.id.as_str(), keys.plugin.path.as_deref()),
            (PluginKind::Instrument, "56535400000000000000000000000001", None)
        );
        assert_eq!(
            (report.plugins, report.missing_plugins.clone(), report.devices),
            (3, vec!["Squash".to_string(), "Keys Synth".to_string()], 2)
        );
        assert!(
            report.summary("Set").ends_with("2 devices, 2 plugins not installed, the settings of 3 plugins"),
            "{}",
            report.summary("Set")
        );
    }

    #[test]
    fn reads_gzipped_sets_and_refuses_other_files() {
        let dir = std::env::temp_dir().join(format!("dstudio-als-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let als = dir.join("Set.als");
        std::fs::write(&als, crate::live_set::gzip(&to_xml(&set())).unwrap()).unwrap();
        assert_eq!(read_file(&als).unwrap().tracks.len(), 2);
        std::fs::write(&als, "<Other />").unwrap();
        assert!(read_file(&als).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
