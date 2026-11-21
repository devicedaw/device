//! Smart Fill (spec 021): the timeline header's Smart Fill pill switches a mode in which dragging
//! over empty lane space marks a time range on one track and opens the Smart Fill box under it.
//! Create fills the range with a MIDI clip from Device's pattern library (or asks the assistant,
//! when a part is described); Record records into the range with the typing, count-in and loop
//! switches. The mode, the range and the box are view state; only Create and Record edit.

use crate::controller::Controller;
use crate::timeline::Hit;
use dstudio_core::library::{self, Composer, MadeSet, PartType};
use dstudio_core::range::TimeRange;
use dstudio_core::{ClipId, Command, MidiClip, PPQ, Ticks, TrackId, TrackKind};

/// Type menu rows: Any, then the library's parts.
pub const TYPES: [&str; 5] = ["Any", "Chords", "Bass", "Lead", "Arp"];

#[derive(Debug, Default)]
pub struct SmartFill {
    /// The mode is on (the pill is lit).
    pub on: bool,
    /// A drag marking a range: start beat and track index.
    drag: Option<(f64, usize)>,
    pub fill: Option<FillBox>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FillBox {
    pub track: TrackId,
    pub start: Ticks,
    pub end: Ticks,
    /// 0 Create, 1 Record.
    pub tab: u8,
    pub text: String,
    /// Type menu row (0 Any).
    pub part: usize,
    /// Genre: index into `library::GENRES` (0 Any).
    pub genre: usize,
    /// 0 no menu, 1 Type, 2 Genre.
    pub menu: u8,
    pub query: String,
    pub looping: bool,
}

/// The genres the Genre menu lists for `query` (without Any), as indexes into `library::GENRES`.
pub fn genres_matching(query: &str) -> Vec<usize> {
    let q = query.trim().to_lowercase();
    (1..library::GENRES.len()).filter(|&i| q.is_empty() || library::GENRES[i].to_lowercase().contains(&q)).collect()
}

/// The part a Create from the library makes for Type row `row` (Any makes chords).
pub fn part_of(row: usize) -> PartType {
    PartType::ALL.get(row.wrapping_sub(1)).copied().unwrap_or(PartType::Chords)
}

impl Controller {
    pub fn smart_fill_toggle(&mut self) {
        self.smart.on = !self.smart.on;
        self.smart_close();
        self.ui_dirty = true;
    }

    /// A lane press, move or release (0, 1, 2) while the mode is on: whether Smart Fill took it.
    /// A press on a clip, the ruler or the master lane is left to the timeline.
    pub fn smart_pointer(&mut self, kind: i32, x: f32, y: f32) -> bool {
        if !self.smart.on {
            return false;
        }
        let beat = {
            let b = self.view.beat_at(x).max(0.0);
            if self.snap_active() { self.ruler_steps().snap(b).max(0.0) } else { b }
        };
        match kind {
            0 => {
                let hit = crate::timeline::hit_test(&self.view, self.lanes_size, self.lane_tracks(), &self.clip_vis, x, y);
                if !matches!(hit, Hit::Track(_)) {
                    return false;
                }
                let Some(track) = self.view.track_at(y).filter(|&i| i < self.lane_tracks()) else { return false };
                if self.project().tracks.get(track).is_none_or(|t| !matches!(t.kind, TrackKind::Audio | TrackKind::Instrument)) {
                    return false;
                }
                self.smart.fill = None;
                self.smart.drag = Some((beat, track));
                self.tools.range = None;
                self.select_clip(None);
            }
            1 => {
                let Some((from, track)) = self.smart.drag else { return false };
                self.tools.range = self.smart_range(from, beat, track);
            }
            _ => {
                let Some((from, track)) = self.smart.drag.take() else { return false };
                let range = self.smart_range(from, beat, track);
                self.tools.range = range.clone();
                if let Some(r) = range {
                    let t = &self.project().tracks[track];
                    let tab = if t.kind == TrackKind::Instrument { 0 } else { 1 };
                    self.smart.fill = Some(FillBox {
                        track: t.id,
                        start: r.start,
                        end: r.end,
                        tab,
                        text: String::new(),
                        part: 0,
                        genre: 0,
                        menu: 0,
                        query: String::new(),
                        looping: false,
                    });
                }
            }
        }
        self.ui_dirty = true;
        true
    }

    fn smart_range(&self, a: f64, b: f64, track: usize) -> Option<TimeRange> {
        let ticks = |x: f64| (x * PPQ as f64).round() as Ticks;
        let id = self.project().tracks.get(track)?.id;
        let r = TimeRange { start: ticks(a.min(b)), end: ticks(a.max(b)), tracks: vec![id] };
        (!r.is_empty()).then_some(r)
    }

    /// × or Escape: the box closes (an open menu first); the mode stays on.
    pub fn smart_close(&mut self) -> bool {
        if let Some(f) = self.smart.fill.as_mut()
            && f.menu != 0
        {
            f.menu = 0;
            self.ui_dirty = true;
            return true;
        }
        let open = self.smart.fill.take().is_some();
        if open {
            self.tools.range = None;
            self.ui_dirty = true;
        }
        open
    }

    fn smart_box(&mut self) -> Option<&mut FillBox> {
        self.ui_dirty = true;
        self.smart.fill.as_mut()
    }

    pub fn smart_tab(&mut self, tab: u8) {
        let audio = self.smart.fill.as_ref().and_then(|f| self.project().track(f.track)).is_some_and(|t| t.kind == TrackKind::Audio);
        if let Some(f) = self.smart_box() {
            f.tab = if audio { 1 } else { tab.min(1) };
            f.menu = 0;
        }
    }

    pub fn smart_text(&mut self, text: &str) {
        if let Some(f) = self.smart_box() {
            f.text = text.to_string();
        }
    }

    /// Type (1) or Genre (2) menu: open it, or close it when it is open.
    pub fn smart_menu(&mut self, which: u8) {
        if let Some(f) = self.smart_box() {
            f.menu = if f.menu == which { 0 } else { which };
            f.query.clear();
        }
    }

    pub fn smart_pick_type(&mut self, row: usize) {
        if let Some(f) = self.smart_box() {
            f.part = row.min(TYPES.len() - 1);
            f.menu = 0;
        }
    }

    /// A Genre menu row (of the rows matching the search).
    pub fn smart_pick_genre(&mut self, row: usize) {
        if let Some(f) = self.smart_box() {
            if let Some(&g) = genres_matching(&f.query).get(row) {
                f.genre = g;
            }
            f.menu = 0;
        }
    }

    pub fn smart_genre_query(&mut self, text: &str) {
        if let Some(f) = self.smart_box() {
            f.query = text.to_string();
        }
    }

    /// Record tab switches: 0 Typing, 1 Count-in, 2 Loop.
    pub fn smart_switch(&mut self, which: u8) {
        match which {
            0 => self.typing_toggle(),
            1 => {
                let bars = if self.count_in_bars() > 0 { 0 } else { 1 };
                self.set_count_in_bars(bars);
            }
            _ => {
                if let Some(f) = self.smart_box() {
                    f.looping = !f.looping;
                }
            }
        }
        self.ui_dirty = true;
    }

    /// Create: a described part goes to the assistant; otherwise the range fills at once with a
    /// MIDI clip from the pattern library.
    pub fn smart_create(&mut self) {
        let Some(f) = self.smart.fill.clone() else { return };
        if self.project().track(f.track).is_none_or(|t| t.kind != TrackKind::Instrument) {
            return;
        }
        if !f.text.trim().is_empty() {
            self.smart_ask(&f);
            return;
        }
        if self.smart_fill_from_library(&f) {
            self.smart_close();
        }
    }

    /// One "Smart Fill" undo step adding a MIDI clip over the range: the first pattern of a new
    /// library set for the Type and Genre, with the set kept so the Patterns tab can swap it.
    pub(crate) fn smart_fill_from_library(&mut self, f: &FillBox) -> bool {
        let p = self.project();
        let part = part_of(f.part);
        let len = f.end - f.start;
        let set = 0;
        let Some(first) = library::create_set(part, f.genre, p.transport.key, set, len, p.ticks_per_bar()).into_iter().next() else {
            return false;
        };
        let id = ClipId(p.next_id);
        let mut clip = MidiClip::empty(f.start, len);
        clip.id = id;
        clip.name = part.label().to_string();
        clip.notes = first.notes;
        clip.pattern = first.pattern;
        clip.composer = Some(Composer {
            part,
            genre: f.genre as u8,
            made: vec![MadeSet { part: Some(part), genre: f.genre as u8, set, last: 0 }],
            playing: Some(0),
            ..Default::default()
        });
        let cmd = Command::Batch { label: "Smart Fill".into(), commands: vec![Command::AddMidiClip { track: f.track, clip }] };
        if self.execute(cmd) {
            self.select_clip(Some(id));
            true
        } else {
            false
        }
    }

    /// A described part: the assistant writes it (Assist mode), told the track, range, Type and Genre.
    fn smart_ask(&mut self, f: &FillBox) {
        let Some(t) = self.project().track(f.track) else { return };
        let tpb = self.project().ticks_per_bar();
        let bar = |ticks: Ticks| ticks as f64 / tpb as f64 + 1.0;
        let kind = if f.part == 0 { "part".to_string() } else { format!("{} part", TYPES[f.part].to_lowercase()) };
        let genre = if f.genre == 0 { String::new() } else { format!(" in a {} style", library::GENRES[f.genre]) };
        let prompt = format!(
            "Smart Fill: write a {kind}{genre} on the track \"{}\" as one new MIDI clip from tick {} to tick {} (bar {:.2} to bar {:.2}), its notes inside that range: {}",
            t.name,
            f.start,
            f.end,
            bar(f.start),
            bar(f.end),
            f.text.trim()
        );
        self.assistant.mode = dstudio_core::assistant::Mode::Assist;
        self.assistant_open = true;
        if self.assistant_send(&prompt) {
            self.smart_close();
        }
        self.ui_dirty = true;
    }

    /// Record: the track armed, the playhead at the range (looped with Loop on), recording on.
    pub fn smart_record(&mut self) {
        let Some(f) = self.smart.fill.clone() else { return };
        if self.project().track(f.track).is_some_and(|t| !t.arm) {
            self.toggle_arm(f.track);
        }
        if f.looping {
            self.execute(Command::SetLoop { enabled: true, start: f.start, end: f.end });
        }
        self.select_track(f.track);
        self.seek_beats(f.start as f64 / PPQ as f64);
        self.smart_close();
        if !self.rec.recording {
            self.start_recording();
        }
    }

    pub(crate) fn push_smart_fill(&self, s: &crate::Studio) {
        use slint::{ModelRc, SharedString, VecModel};
        s.set_smart_on(self.smart.on);
        let Some(f) = &self.smart.fill else {
            s.set_smart_open(false);
            return;
        };
        let Some(t) = self.project().track(f.track) else {
            s.set_smart_open(false);
            return;
        };
        let i = self.project().track_index(f.track).unwrap_or(0);
        let x0 = self.view.x_of(f.start as f64 / PPQ as f64);
        let x1 = self.view.x_of(f.end as f64 / PPQ as f64);
        s.set_smart_open(true);
        s.set_smart_x((x0 + x1) / 2.0);
        s.set_smart_y(self.view.track_top(i) + self.view.pitch(i));
        s.set_smart_track(SharedString::from(t.name.as_str()));
        s.set_smart_audio(t.kind == TrackKind::Audio);
        s.set_smart_tab(f.tab as i32);
        s.set_smart_text(SharedString::from(f.text.as_str()));
        s.set_smart_type(SharedString::from(TYPES[f.part]));
        s.set_smart_genre(SharedString::from(if f.genre == 0 { "Genre" } else { library::GENRES[f.genre] }));
        s.set_smart_menu(f.menu as i32);
        let rows: Vec<crate::ChoiceItem> = match f.menu {
            1 => TYPES.iter().enumerate().map(|(i, l)| (l.to_string(), i == f.part)).collect::<Vec<_>>(),
            2 => genres_matching(&f.query).into_iter().map(|g| (library::GENRES[g].to_string(), g == f.genre)).collect(),
            _ => Vec::new(),
        }
        .into_iter()
        .enumerate()
        .map(|(i, (label, checked))| crate::ChoiceItem {
            label: label.into(),
            checked,
            hint: false,
            y: crate::clipedit::MENU_PAD + i as f32 * crate::clipedit::MENU_ROW,
            h: crate::clipedit::MENU_ROW,
        })
        .collect();
        s.set_smart_menu_h(crate::clipedit::MENU_PAD * 2.0 + rows.len() as f32 * crate::clipedit::MENU_ROW);
        s.set_smart_items(ModelRc::new(VecModel::from(rows)));
        s.set_smart_typing(self.typing.active);
        s.set_smart_count_in(self.count_in_bars() > 0);
        s.set_smart_loop(f.looping);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn any_makes_chords_and_the_other_types_their_part() {
        assert_eq!(part_of(0), PartType::Chords);
        assert_eq!(part_of(2), PartType::Bass);
        assert_eq!(part_of(4), PartType::Arp);
    }

    #[test]
    fn genre_search_filters_and_never_lists_any() {
        assert_eq!(genres_matching("").len(), library::GENRES.len() - 1);
        assert!(genres_matching("").iter().all(|&g| g != 0));
        assert_eq!(genres_matching("house").iter().map(|&g| library::GENRES[g]).collect::<Vec<_>>(), ["House"]);
    }

    #[test]
    fn a_library_fill_stays_inside_its_range_for_every_type() {
        // Two bars at 4/4 for each part: every note starts and ends inside the clip.
        for part in PartType::ALL {
            let len = 8 * PPQ;
            let first = library::create_set(part, 0, None, 0, len, 4 * PPQ).into_iter().next().expect("a pattern");
            assert!(!first.notes.is_empty(), "{part:?} has notes");
            assert!(first.notes.iter().all(|n| n.start >= 0 && n.start + n.length <= len), "{part:?} inside its clip");
        }
    }
}
