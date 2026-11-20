//! Settings › MIDI › Default instrument (spec 013): what a new empty instrument track plays.
//! The choice is an app preference (`MidiPrefs::default_instrument`); `None` is the built-in
//! Synth, which leaves the track exactly as it always was (no instrument device).

use crate::controller::Controller;
use crate::prefs::InstrumentChoice;
use dstudio_core::builtin::{self, DeviceDef};
use dstudio_core::{Command, Device, PluginKind, PluginRef, Track, TrackId, TrackKind};

/// The built-in instruments offered after the Synth, in menu order.
pub fn builtin_instruments() -> [&'static DeviceDef; 3] {
    [&builtin::sampler::SAMPLER, &builtin::pad_sampler::PAD_SAMPLER, &builtin::drum_rack::DRUM_RACK]
}

/// What a new instrument track gets: its name and instrument device (None = the Synth), and
/// why the choice fell back to the Synth, if it did.
#[derive(Debug, Clone, PartialEq)]
pub struct Resolved {
    pub name: String,
    pub device: Option<Device>,
    pub fallback: Option<String>,
}

impl Resolved {
    fn synth(fallback: Option<String>) -> Self {
        Resolved { name: "Instrument".into(), device: None, fallback }
    }
}

/// The scanned plugin matching `p` (same format and id), so the device gets the path where the
/// plugin is now rather than where it was when it was picked.
pub fn find_plugin<'a>(plugins: impl IntoIterator<Item = &'a PluginRef>, p: &PluginRef) -> Option<&'a PluginRef> {
    plugins.into_iter().find(|q| q.kind == PluginKind::Instrument && q.format == p.format && q.id == p.id)
}

/// The text the dropdown shows for a choice.
pub fn choice_label(choice: Option<&InstrumentChoice>) -> String {
    match choice {
        None => "Synth".into(),
        Some(InstrumentChoice::Builtin { id }) => builtin::find(id).map_or_else(|| "Synth".into(), |d| d.name.to_string()),
        Some(InstrumentChoice::Plugin { plugin }) => plugin.name.clone(),
    }
}

/// Two choices name the same instrument (a plugin by format and id, wherever it was found).
pub fn same_choice(a: Option<&InstrumentChoice>, b: Option<&InstrumentChoice>) -> bool {
    let synth = |c: Option<&InstrumentChoice>| match c {
        None => true,
        Some(InstrumentChoice::Builtin { id }) => {
            builtin::find(id).is_none_or(|d| d.kind != PluginKind::Instrument || d.id == builtin::synth_v2::SYNTH_V2.id)
        }
        Some(InstrumentChoice::Plugin { .. }) => false,
    };
    match (a, b) {
        _ if synth(a) || synth(b) => synth(a) && synth(b),
        (Some(InstrumentChoice::Builtin { id: x }), Some(InstrumentChoice::Builtin { id: y })) => x == y,
        (Some(InstrumentChoice::Plugin { plugin: x }), Some(InstrumentChoice::Plugin { plugin: y })) => {
            x.format == y.format && x.id == y.id
        }
        _ => false,
    }
}

impl Controller {
    /// The Default instrument dropdown: Synth and the other built-in instruments, then the
    /// scanned instrument plugins by name under a "Plugins" heading. A chosen plugin the scan
    /// didn't find is listed too, marked missing, so the menu still shows the choice.
    pub(crate) fn instrument_choices(&self) -> Vec<(String, crate::settings::Choice)> {
        use crate::settings::Choice;
        let mut v = vec![("Synth".to_string(), Choice::Instrument(None))];
        v.extend(
            builtin_instruments()
                .iter()
                .map(|d| (d.name.to_string(), Choice::Instrument(Some(InstrumentChoice::Builtin { id: d.id.into() })))),
        );
        let mut plugins: Vec<&PluginRef> = self.scanned_plugins().filter(|p| p.kind == PluginKind::Instrument).collect();
        plugins.sort_by_cached_key(|p| (p.name.to_lowercase(), crate::devices::format_badge(p.format)));
        let missing = match &self.settings.prefs.midi.default_instrument {
            Some(InstrumentChoice::Plugin { plugin }) if self.default_instrument_missing() => Some(plugin.clone()),
            _ => None,
        };
        if !plugins.is_empty() || missing.is_some() {
            v.push(("Plugins".to_string(), Choice::Header));
        }
        v.extend(plugins.into_iter().map(|p| {
            let label = format!("{} ({})", p.name, crate::devices::format_badge(p.format));
            (label, Choice::Instrument(Some(InstrumentChoice::Plugin { plugin: p.clone() })))
        }));
        if let Some(p) = missing {
            let label = format!("{} (missing)", p.name);
            v.push((label, Choice::Instrument(Some(InstrumentChoice::Plugin { plugin: p }))));
        }
        v
    }

    fn scanned_plugins(&self) -> impl Iterator<Item = &PluginRef> {
        self.devices.catalog.plugins.iter().map(|p| &p.plugin)
    }

    /// The chosen default instrument is a plugin the last scan didn't find.
    pub fn default_instrument_missing(&self) -> bool {
        match &self.settings.prefs.midi.default_instrument {
            Some(InstrumentChoice::Plugin { plugin }) => find_plugin(self.scanned_plugins(), plugin).is_none(),
            _ => false,
        }
    }

    pub fn resolve_default_instrument(&self) -> Resolved {
        match &self.settings.prefs.midi.default_instrument {
            None => Resolved::synth(None),
            Some(InstrumentChoice::Builtin { id }) => match builtin::find(id).filter(|d| d.kind == PluginKind::Instrument) {
                Some(def) if def.id == builtin::synth_v2::SYNTH_V2.id => Resolved::synth(None),
                Some(def) => Resolved { name: def.name.into(), device: Some(def.instantiate()), fallback: None },
                None => Resolved::synth(None),
            },
            Some(InstrumentChoice::Plugin { plugin }) => match find_plugin(self.scanned_plugins(), plugin) {
                Some(p) => Resolved { name: p.name.clone(), device: Some(Device::new(p.clone())), fallback: None },
                None => Resolved::synth(Some(format!(
                    "{} isn't installed (not found by the last plugin scan): the new track plays the Synth",
                    plugin.name
                ))),
            },
        }
    }

    /// The commands that add an empty instrument track `id` at the end of the ordinary tracks,
    /// playing the default instrument, and why it fell back to the Synth, if it did.
    pub(crate) fn new_instrument_track(&self, id: TrackId) -> (Vec<Command>, Option<String>) {
        let r = self.resolve_default_instrument();
        let track = Track::new(id, TrackKind::Instrument, &r.name, crate::controller::next_track_color(self.project()));
        let mut commands = vec![Command::AddTrack { index: None, track }];
        if let Some(device) = r.device {
            commands.push(Command::SetInstrumentPlugin { track: id, device: Some(device) });
        }
        (commands, r.fallback)
    }

    /// ⇧⌘T, Create › Instrument Track and the add-track entries: a new instrument track at the
    /// end of the ordinary tracks playing the default instrument, selected. One undo step.
    pub fn add_instrument_track(&mut self) -> Option<TrackId> {
        let id = TrackId(self.project().next_id);
        let (commands, fallback) = self.new_instrument_track(id);
        if !self.execute(Command::Batch { label: "Add Instrument Track".into(), commands }) {
            return None;
        }
        if let Some(why) = fallback {
            self.set_status(why, 5);
        }
        self.select_track(id);
        Some(id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dstudio_core::{PluginFormat, Project};

    fn controller() -> Controller {
        let engine = std::sync::Arc::new(dstudio_engine::Engine::new(dstudio_engine::Config::default()).unwrap());
        Controller::new(engine, Project::default())
    }

    fn keys(path: &str) -> PluginRef {
        PluginRef {
            format: PluginFormat::Clap,
            id: "com.example.keys".into(),
            path: Some(path.into()),
            name: "Keys".into(),
            vendor: "Example".into(),
            version: "1.0".into(),
            kind: PluginKind::Instrument,
        }
    }

    fn plays_synth(t: &Track) -> bool {
        t.instrument_plugin.as_ref().is_some_and(crate::devices::is_builtin_synth)
    }

    fn last_track(c: &Controller) -> &Track {
        c.project().tracks.iter().rev().find(|t| t.kind == TrackKind::Instrument).unwrap()
    }

    #[test]
    fn the_synth_default_makes_the_same_track_as_before() {
        let mut c = controller();
        let n = c.project().tracks.len();
        let id = c.add_instrument_track().unwrap();
        let t = c.project().track(id).unwrap().clone();
        assert_eq!((t.name.as_str(), plays_synth(&t)), ("Instrument", true));
        assert_eq!(c.selected_track, Some(id));
        // The same track a bare AddTrack makes, as before this setting existed.
        let mut p = c.project().clone();
        p.tracks.retain(|x| x.id != id);
        let mut d = dstudio_core::Document::new(p);
        let bare = Track::new(id, TrackKind::Instrument, "Instrument", t.color);
        d.execute(Command::AddTrack { index: None, track: bare }, dstudio_core::Origin::User).unwrap();
        // Device ids differ only because they come from different id counters.
        let json = |t: &Track| {
            let mut t = t.clone();
            t.instrument_plugin.iter_mut().for_each(|d| d.id = dstudio_core::DeviceId(0));
            serde_json::to_value(&t).unwrap()
        };
        assert_eq!(json(&t), json(d.project().track(id).unwrap()));
        c.undo();
        assert_eq!(c.project().tracks.len(), n);
    }

    #[test]
    fn a_builtin_default_names_the_track_and_undoes_in_one_step() {
        let mut c = controller();
        let n = c.project().tracks.len();
        c.settings.prefs.midi.default_instrument = Some(InstrumentChoice::Builtin { id: "sampler-steam".into() });
        let id = c.add_instrument_track().unwrap();
        let t = c.project().track(id).unwrap();
        assert_eq!(t.name, "Sampler");
        assert_eq!(t.instrument_plugin.as_ref().map(|d| d.plugin.id.as_str()), Some("sampler-steam"));
        assert_eq!(c.doc.undo_label(), Some("Add Instrument Track"));
        c.undo();
        assert_eq!(c.project().tracks.len(), n, "one undo removes the track and its instrument");
        // An id that isn't a built-in instrument plays the Synth.
        c.settings.prefs.midi.default_instrument = Some(InstrumentChoice::Builtin { id: "reverb".into() });
        c.add_instrument_track().unwrap();
        assert!(plays_synth(last_track(&c)));
    }

    #[test]
    fn a_plugin_default_uses_the_scanned_plugin_and_a_missing_one_falls_back() {
        let mut c = controller();
        let choice = Some(InstrumentChoice::Plugin { plugin: keys("/old/Keys.clap") });
        c.settings.prefs.midi.default_instrument = choice.clone();
        assert!(c.default_instrument_missing());
        c.add_instrument_track().unwrap();
        let t = last_track(&c);
        assert_eq!((t.name.as_str(), plays_synth(t)), ("Instrument", true));
        assert!(c.status_text().is_some_and(|s| s.contains("Keys isn't installed")), "{:?}", c.status_text());
        assert_eq!(c.settings.prefs.midi.default_instrument, choice, "the choice is kept");

        c.devices.catalog.plugins.push(dstudio_engine::PluginInfo {
            plugin: keys("/new/Keys.clap"),
            has_editor: false,
            audio_inputs: 0,
            audio_outputs: 2,
        });
        assert!(!c.default_instrument_missing());
        c.add_instrument_track().unwrap();
        let t = last_track(&c);
        assert_eq!(t.name, "Keys");
        let path = t.instrument_plugin.as_ref().and_then(|d| d.plugin.path.as_deref());
        assert_eq!(path, Some("/new/Keys.clap"), "the device uses where the plugin is now");
    }

    #[test]
    fn labels() {
        assert_eq!(choice_label(None), "Synth");
        assert_eq!(choice_label(Some(&InstrumentChoice::Builtin { id: "drum-rack".into() })), "Drum Rack");
        assert_eq!(choice_label(Some(&InstrumentChoice::Plugin { plugin: keys("/x") })), "Keys");
    }
}
