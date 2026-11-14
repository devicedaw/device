//! Return tracks, sends and Out routing in the app (spec 002): the Timeline's Returns pill, the
//! Add track / send options / Out menus, send edits (level, pre-fader, remove), and the view models
//! for the mixer Sends sections and the channel strip's Sends tab.
//!
//! A return track is a shared effect track; a send is how much of one track goes into it. Every
//! ordinary track has one send slot per return track: the Sends tab shows a knob for each return
//! and the mixer's Sends section a row for each. A slot with no stored send is silent (−∞), and
//! moving its knob stores it. Returns are added from Add track, the Returns pill (when there are
//! none), and the "+" beside Sends.
//!
//! Every edit is a core command: `SetSends` (wrapped in a labelled `Batch` for Add Send, Remove
//! Send and Send Pre-Fader / Post-Fader; level drags coalesce as "Send Level Change"),
//! `SetTrackOutput` and `AddTrack`. The pill and the expanded Sends sections are view state.

use crate::controller::{Controller, gain_from_slider, next_track_color, slider_from_gain, slint_color, sync_model, track_color};
use crate::{ChoiceItem, SendRow, Studio};
use dstudio_core::{Command, MAX_RETURNS, Track, TrackId, TrackKind, TrackSend, gain_to_db, return_letter};
use slint::VecModel;
use std::rc::Rc;

/// Menu row height and padding (the "In" selector's metrics).
const ROW: f32 = 20.0;
const PAD: f32 = 4.0;

/// What a routing menu row does.
#[derive(Debug, Clone, PartialEq)]
pub enum RouteItem {
    /// Send options (mixer): the send from the menu's track to this return taps after the fader
    /// (false) or before it (true).
    SendPre(TrackId, bool),
    /// Send options: the send to this return back to 0 dB (stored if it was not).
    SendReset(TrackId),
    /// Send options: remove the send to this return (its slot goes silent).
    SendRemove(TrackId),
    /// Route the menu's track to the master (None) or a return.
    Out(Option<TrackId>),
    /// Add a track of this kind.
    AddTrack(TrackKind),
}

#[derive(Debug, Clone)]
pub struct RouteMenu {
    /// The track the menu edits (None for Add track).
    pub track: Option<TrackId>,
    pub items: Vec<(String, RouteItem, bool)>,
    /// Anchor in window coordinates (design units); the popup keeps itself on screen.
    pub x: f32,
    pub y: f32,
}

#[derive(Default)]
pub struct RoutingUi {
    /// Mixer: the Sends section of every strip is expanded.
    pub sends_open: bool,
    pub menu: Option<RouteMenu>,
    pub menu_model: Rc<VecModel<ChoiceItem>>,
    /// Mixer send rows, all strips (each row names its sender).
    pub strip_sends_model: Rc<VecModel<SendRow>>,
    /// Channel strip Sends tab rows for the selected track.
    pub sel_sends_model: Rc<VecModel<SendRow>>,
}

/// Send level readout: "0.0 dB", "−12.5 dB", "−∞ dB".
pub fn fmt_send_db(gain: f32) -> String {
    let db = gain_to_db(gain);
    if !db.is_finite() || db <= -60.0 { "−∞ dB".into() } else { format!("{db:.1} dB").replace('-', "−") }
}

/// Mixer send row readout (no unit): "0.0", "−12.5", "−∞".
fn fmt_send_short(gain: f32) -> String {
    let db = gain_to_db(gain);
    if !db.is_finite() || db <= -60.0 { "−∞".into() } else { format!("{db:.1}").replace('-', "−") }
}

impl Controller {
    /// The Returns pill is lit: return rows show on the timeline.
    pub fn returns_shown(&self) -> bool {
        !self.settings.prefs.hide_returns
    }

    /// Tracks drawn on the timeline: every track, or the ordinary ones while the return rows are
    /// hidden (returns always come last, so the rows keep their project indices).
    pub fn lane_tracks(&self) -> usize {
        let p = self.project();
        if self.returns_shown() { p.tracks.len() } else { p.ordinary_track_count() }
    }

    /// The Returns pill: shows or hides the return rows. In a project without return tracks it
    /// adds Return A instead (and shows the rows).
    pub fn toggle_returns(&mut self) {
        if self.project().returns().next().is_none() {
            if self.add_return_track(true).is_some() && self.settings.prefs.hide_returns {
                self.settings.prefs.hide_returns = false;
                self.save_prefs();
            }
            return;
        }
        self.settings.prefs.hide_returns = !self.settings.prefs.hide_returns;
        self.save_prefs();
        self.ui_dirty = true;
    }

    pub fn toggle_sends_open(&mut self) {
        self.routing.sends_open = !self.routing.sends_open;
        self.ui_dirty = true;
    }

    /// Add a return track ("Add Return Track"): Add track › Return track and the Returns pill
    /// select it (`select`); the "+" beside Sends keeps the selection, so the new return's knob
    /// shows on the track being edited. Returns its id.
    pub fn add_return_track(&mut self, select: bool) -> Option<TrackId> {
        let p = self.project();
        if p.returns().count() >= MAX_RETURNS {
            self.set_status(format!("A project has at most {MAX_RETURNS} return tracks"), 3);
            return None;
        }
        let id = TrackId(p.next_id);
        let track = Track::new(id, TrackKind::Return, &p.next_return_name(), next_track_color(p));
        self.doc.seal_coalescing();
        if self.execute(Command::Batch { label: "Add Return Track".into(), commands: vec![Command::AddTrack { index: None, track }] }) {
            self.doc.seal_coalescing();
            if select {
                self.select_track(id);
            }
            return Some(id);
        }
        None
    }

    fn set_sends(&mut self, label: &str, track: TrackId, sends: Vec<TrackSend>) -> bool {
        self.doc.seal_coalescing();
        let ok = self.execute(Command::Batch { label: label.into(), commands: vec![Command::SetSends { track, sends }] });
        self.doc.seal_coalescing();
        ok
    }

    /// Add a send at 0 dB, post-fader, from `track` to the return `target` ("Add Send").
    pub fn add_send(&mut self, track: TrackId, target: TrackId) -> bool {
        let Some(t) = self.project().track(track) else { return false };
        if t.sends.iter().any(|s| s.target == target) {
            return false;
        }
        let mut sends = t.sends.clone();
        sends.push(TrackSend::new(target));
        self.set_sends("Add Send", track, sends)
    }

    /// Create a return track and a send to it from `track`, in one undo step ("Add Send").
    pub fn new_return_with_send(&mut self, track: TrackId) -> Option<TrackId> {
        let p = self.project();
        let t = p.track(track).filter(|t| !t.is_return())?;
        if p.returns().count() >= MAX_RETURNS {
            self.set_status(format!("A project has at most {MAX_RETURNS} return tracks"), 3);
            return None;
        }
        let id = TrackId(p.next_id);
        let ret = Track::new(id, TrackKind::Return, &p.next_return_name(), next_track_color(p));
        let mut sends = t.sends.clone();
        sends.push(TrackSend::new(id));
        self.doc.seal_coalescing();
        let ok = self.execute(Command::Batch {
            label: "Add Send".into(),
            commands: vec![Command::AddTrack { index: None, track: ret }, Command::SetSends { track, sends }],
        });
        self.doc.seal_coalescing();
        ok.then_some(id)
    }

    /// Remove the send from `track` to `target` ("Remove Send").
    pub fn remove_send(&mut self, track: TrackId, target: TrackId) -> bool {
        let Some(t) = self.project().track(track) else { return false };
        let sends: Vec<TrackSend> = t.sends.iter().filter(|s| s.target != target).cloned().collect();
        if sends.len() == t.sends.len() {
            return false;
        }
        self.set_sends("Remove Send", track, sends)
    }

    /// Flip a send between post- and pre-fader ("Send Pre-Fader" / "Send Post-Fader").
    pub fn toggle_send_pre(&mut self, track: TrackId, target: TrackId) -> bool {
        let pre = self.project().track(track).and_then(|t| t.sends.iter().find(|s| s.target == target)).is_some_and(|s| s.pre_fader);
        self.set_send_pre(track, target, !pre)
    }

    /// Tap the send before (`pre`) or after the fader. A silent slot is stored, still silent, so
    /// the choice holds once its level comes up.
    pub fn set_send_pre(&mut self, track: TrackId, target: TrackId, pre: bool) -> bool {
        if !self.can_send(track, target) {
            return false;
        }
        let Some(t) = self.project().track(track) else { return false };
        let mut sends = t.sends.clone();
        if !sends.iter().any(|s| s.target == target) {
            sends.push(TrackSend { target, level: 0.0, pre_fader: false });
        }
        let Some(s) = sends.iter_mut().find(|s| s.target == target) else { return false };
        if s.pre_fader == pre {
            return false;
        }
        s.pre_fader = pre;
        self.set_sends(if pre { "Send Pre-Fader" } else { "Send Post-Fader" }, track, sends)
    }

    /// `track` can send to `target`: an ordinary or group track and an existing return track.
    fn can_send(&self, track: TrackId, target: TrackId) -> bool {
        let p = self.project();
        p.track(track).is_some_and(|t| !t.is_return()) && p.track(target).is_some_and(|r| r.is_return())
    }

    /// Set a send's level (linear gain, clamped to −∞..+6 dB). A silent slot (no send stored yet)
    /// gets a post-fader send at that level. Successive calls in one drag coalesce into one
    /// "Send Level Change" however long the knob is held still; `send_gesture_end` closes the drag.
    pub fn set_send_level(&mut self, track: TrackId, target: TrackId, gain: f32) -> bool {
        if !self.can_send(track, target) {
            return false;
        }
        let Some(t) = self.project().track(track) else { return false };
        let gain = gain.clamp(0.0, TrackSend::MAX_LEVEL);
        let mut sends = t.sends.clone();
        if !sends.iter().any(|s| s.target == target) {
            if gain <= 0.0 {
                return false; // the slot is silent already
            }
            sends.push(TrackSend { target, level: 0.0, pre_fader: false });
        }
        let Some(s) = sends.iter_mut().find(|s| s.target == target) else { return false };
        if (s.level - gain).abs() < 1e-7 {
            return false;
        }
        s.level = gain;
        // One drag is one undo step, pauses while the knob is held included.
        let ok = self.execute(Command::SetSends { track, sends });
        if ok {
            self.doc.hold_coalescing();
        }
        ok
    }

    /// Send level from a slider / knob position (the track volume slider's taper).
    pub fn set_send_pos(&mut self, track: TrackId, target: TrackId, pos: f32) -> bool {
        self.set_send_level(track, target, gain_from_slider(pos))
    }

    pub fn send_gesture_end(&mut self) {
        self.doc.seal_coalescing();
    }

    /// Route `track`'s Out to the master (None) or a return ("Output Change").
    pub fn set_track_output(&mut self, track: TrackId, output: Option<TrackId>) -> bool {
        if self.project().track(track).is_none_or(|t| t.is_bus() || t.output == output) {
            return false;
        }
        self.execute(Command::SetTrackOutput { track, output })
    }

    // ---------------------------------------------------------------- menus

    fn return_label(&self, id: TrackId) -> String {
        self.project().track(id).map(|t| t.name.clone()).unwrap_or_default()
    }

    /// The "+" beside Sends (channel strip Sends tab, mixer strip): a new return track, its slot
    /// silent on every track; `track` stays selected.
    pub fn sends_add_return(&mut self, track: TrackId) -> Option<TrackId> {
        if self.project().track(track).is_none_or(|t| t.is_return()) {
            return None;
        }
        self.add_return_track(false)
    }

    /// Options of the send from `track` to the return `target` (the return's letter tag in the
    /// mixer's Sends section): Post-fader / Pre-fader (the current one checked), Reset to 0 dB,
    /// and Remove send while one is stored.
    pub fn open_send_options(&mut self, track: TrackId, target: TrackId, x: f32, y: f32) {
        if !self.can_send(track, target) {
            return;
        }
        let send = self.project().track(track).and_then(|t| t.sends.iter().find(|s| s.target == target).copied());
        let pre = send.is_some_and(|s| s.pre_fader);
        let mut items = vec![
            ("Post-fader".to_string(), RouteItem::SendPre(target, false), !pre),
            ("Pre-fader".to_string(), RouteItem::SendPre(target, true), pre),
            ("Reset to 0 dB".to_string(), RouteItem::SendReset(target), false),
        ];
        if send.is_some() {
            items.push(("Remove send".to_string(), RouteItem::SendRemove(target), false));
        }
        self.routing.menu = Some(RouteMenu { track: Some(track), items, x, y });
        self.ui_dirty = true;
    }

    /// Channel strip Out selector: Master. A track's whole output goes to the master; to share
    /// an effect, send to a return track instead. A track that an older project routed into a
    /// return lists that return too (checked), so it can be moved back.
    pub fn open_out_menu(&mut self, x: f32, y: f32) {
        let p = self.project();
        let Some(t) = self.selected_track.and_then(|id| p.track(id)).filter(|t| !t.is_bus()) else { return };
        let mut items = vec![("Master".to_string(), RouteItem::Out(None), t.output.is_none())];
        // Ordinary tracks can play into any group (and join it).
        if !t.is_group() {
            items.extend(
                p.tracks.iter().filter(|g| g.is_group()).map(|g| (g.name.clone(), RouteItem::Out(Some(g.id)), t.output == Some(g.id))),
            );
        }
        if let Some(r) = t.output.and_then(|r| p.track(r)).filter(|r| r.is_return()) {
            items.push((r.name.clone(), RouteItem::Out(Some(r.id)), true));
        }
        self.routing.menu = Some(RouteMenu { track: Some(t.id), items, x, y });
        self.ui_dirty = true;
    }

    /// Add track (timeline and mixer): Audio, Instrument or Return track.
    pub fn open_add_track_menu(&mut self, x: f32, y: f32) {
        let items = vec![
            ("Audio track".to_string(), RouteItem::AddTrack(TrackKind::Audio), false),
            ("Instrument track".to_string(), RouteItem::AddTrack(TrackKind::Instrument), false),
            ("Return track".to_string(), RouteItem::AddTrack(TrackKind::Return), false),
            ("Group track".to_string(), RouteItem::AddTrack(TrackKind::Group), false),
        ];
        self.routing.menu = Some(RouteMenu { track: None, items, x, y });
        self.ui_dirty = true;
    }

    pub fn route_menu_close(&mut self) {
        if self.routing.menu.take().is_some() {
            self.ui_dirty = true;
        }
    }

    pub fn route_menu_pick(&mut self, i: usize) {
        let Some(menu) = self.routing.menu.take() else { return };
        self.ui_dirty = true;
        let Some((_, item, _)) = menu.items.get(i).cloned() else { return };
        match (item, menu.track) {
            (RouteItem::SendPre(target, pre), Some(track)) => {
                self.set_send_pre(track, target, pre);
            }
            (RouteItem::SendReset(target), Some(track)) => {
                self.set_send_level(track, target, 1.0);
                self.send_gesture_end();
            }
            (RouteItem::SendRemove(target), Some(track)) => {
                self.remove_send(track, target);
            }
            (RouteItem::Out(out), Some(track)) => {
                let group = |o: Option<TrackId>| o.and_then(|o| self.project().track(o)).is_some_and(|g| g.is_group());
                let from_group = group(self.project().track(track).and_then(|t| t.output));
                if group(out) || (out.is_none() && from_group) {
                    self.route_to_group(track, out);
                } else {
                    self.set_track_output(track, out);
                }
            }
            (RouteItem::AddTrack(TrackKind::Audio), _) => {
                self.add_audio_track();
            }
            (RouteItem::AddTrack(TrackKind::Instrument), _) => {
                self.add_instrument_track();
            }
            (RouteItem::AddTrack(TrackKind::Return), _) => {
                self.add_return_track(true);
            }
            (RouteItem::AddTrack(TrackKind::Group), _) => {
                self.add_group_track();
            }
            _ => {}
        }
    }

    /// Labels of the open routing menu (self-test).
    pub fn route_menu_labels(&self) -> Vec<String> {
        self.routing.menu.as_ref().map(|m| m.items.iter().map(|(l, _, _)| l.clone()).collect()).unwrap_or_default()
    }

    // ---------------------------------------------------------------- view models

    /// One send slot per return track, in return order (none on a return track): the stored
    /// send, or a silent one.
    pub(crate) fn send_rows(&self, track: &Track) -> Vec<SendRow> {
        let p = self.project();
        if track.is_return() {
            return Vec::new();
        }
        p.returns()
            .enumerate()
            .map(|(ri, r)| {
                let s = track.sends.iter().find(|s| s.target == r.id);
                let level = s.map_or(0.0, |s| s.level);
                SendRow {
                    track: track.id.0 as i32,
                    row: ri as i32,
                    target: r.id.0 as i32,
                    letter: return_letter(ri).to_string().into(),
                    name: r.name.clone().into(),
                    color: slint_color(track_color(r.color)),
                    pos: slider_from_gain(level),
                    db_text: fmt_send_db(level).into(),
                    short_text: fmt_send_short(level).into(),
                    pre: s.is_some_and(|s| s.pre_fader),
                    stored: s.is_some(),
                    live: fmt_send_short(level) != "−∞",
                }
            })
            .collect()
    }

    /// Pill, Sends sections, the channel strip routing fields and the routing menu → Slint.
    pub(crate) fn push_routing(&self, s: &Studio) {
        let p = self.project();
        // The pill is lit while return rows actually show (not in a project without returns).
        s.set_returns_shown(self.returns_shown() && p.returns().next().is_some());
        s.set_sends_open(self.routing.sends_open);
        let rows: Vec<SendRow> = p.tracks.iter().filter(|t| !t.is_return()).flat_map(|t| self.send_rows(t)).collect();
        s.set_strip_sends_max(p.returns().count() as i32);
        sync_model(&self.routing.strip_sends_model, rows);
        let sel = self.selected_track.and_then(|id| p.track(id));
        s.set_sel_return(sel.is_some_and(|t| t.is_return()));
        s.set_sel_group(sel.is_some_and(|t| t.is_group()));
        let letter = sel.and_then(|t| p.return_index(t.id)).map(|i| return_letter(i).to_string()).unwrap_or_default();
        s.set_sel_letter(letter.into());
        let out = match sel.and_then(|t| t.output) {
            Some(r) => self.return_label(r),
            None => "Master".into(),
        };
        s.set_sel_out_text(out.into());
        sync_model(&self.routing.sel_sends_model, sel.map(|t| self.send_rows(t)).unwrap_or_default());
        s.set_sel_can_send(sel.is_some_and(|t| !t.is_return()));
        s.set_sel_can_route(sel.is_some_and(|t| !t.is_bus()));
        s.set_can_add_return(p.returns().count() < MAX_RETURNS);
        match &self.routing.menu {
            Some(m) => {
                let mut y = PAD;
                let rows: Vec<ChoiceItem> = m
                    .items
                    .iter()
                    .map(|(label, _, checked)| {
                        let r = ChoiceItem { label: label.clone().into(), checked: *checked, hint: false, y, h: ROW };
                        y += ROW;
                        r
                    })
                    .collect();
                let w = m.items.iter().map(|(l, _, _)| crate::text_measure::value_width(l)).fold(0.0f32, f32::max) + 44.0;
                s.set_route_menu_x(m.x);
                s.set_route_menu_y(m.y);
                s.set_route_menu_w(w.max(120.0));
                s.set_route_menu_h(y + PAD);
                sync_model(&self.routing.menu_model, rows);
                s.set_route_menu_open(true);
            }
            None => s.set_route_menu_open(false),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dstudio_core::Project;
    use dstudio_engine::Engine;
    use std::sync::Arc;

    fn controller() -> Controller {
        let engine = Arc::new(Engine::new(dstudio_engine::Config::default()).unwrap());
        Controller::new(engine, Project::default())
    }

    /// Every ordinary track has one send slot per return, silent until something is sent; a
    /// return track has none.
    #[test]
    fn every_return_is_a_send_slot_on_every_track() {
        let mut c = controller();
        let t = c.project().tracks[0].id;
        let a = c.new_return_with_send(t).unwrap();
        let b = c.sends_add_return(t).unwrap();
        assert_eq!(c.selected_track, Some(t), "'+' keeps the track selected");
        assert_eq!(c.doc.undo_label(), Some("Add Return Track"));
        let rows = c.send_rows(&c.project().tracks[0]);
        let slots: Vec<_> = rows.iter().map(|r| (r.target, r.name.to_string(), r.stored, r.live)).collect();
        assert_eq!(slots, [(a.0 as i32, "Return A".into(), true, true), (b.0 as i32, "Return B".into(), false, false)]);
        let other = c.project().tracks[1].clone();
        assert_eq!(c.send_rows(&other).len(), 2, "a slot per return on every ordinary track");
        let ret = c.project().track(a).unwrap().clone();
        assert!(c.send_rows(&ret).is_empty());
    }

    /// Moving a silent slot stores a post-fader send; undo makes it silent again. A silent slot
    /// stays unstored when set to −∞.
    #[test]
    fn moving_a_silent_send_turns_it_on() {
        let mut c = controller();
        let t = c.project().tracks[0].id;
        let r = c.sends_add_return(t).unwrap();
        assert!(!c.set_send_level(t, r, 0.0), "−∞ on a silent slot is no edit");
        assert!(c.set_send_level(t, r, 0.5));
        c.send_gesture_end();
        assert_eq!(c.project().tracks[0].sends, [TrackSend { target: r, level: 0.5, pre_fader: false }]);
        assert_eq!(c.doc.undo_label(), Some("Send Level Change"));
        c.undo();
        assert!(c.project().tracks[0].sends.is_empty());
        assert!(!c.set_send_level(r, r, 1.0), "a return track has no sends");
    }

    /// The Returns pill adds Return A when there is none; afterwards it shows and hides the rows.
    /// Out lists only Master unless an older project routed the track into a return.
    #[test]
    fn returns_pill_and_out_menu() {
        let mut c = controller();
        c.settings.prefs.hide_returns = true;
        c.toggle_returns();
        assert_eq!(c.project().returns().map(|r| r.name.as_str()).collect::<Vec<_>>(), ["Return A"]);
        assert!(c.returns_shown());
        c.toggle_returns();
        assert!(!c.returns_shown());
        assert_eq!(c.project().returns().count(), 1, "hiding adds nothing");
        let t = c.project().tracks[0].id;
        c.select_track(t);
        c.open_out_menu(0.0, 0.0);
        assert_eq!(c.route_menu_labels(), ["Master"]);
        let r = c.project().returns().next().unwrap().id;
        assert!(c.set_track_output(t, Some(r)));
        c.open_out_menu(0.0, 0.0);
        assert_eq!(c.route_menu_labels(), ["Master", "Return A"]);
    }

    #[test]
    fn send_readouts() {
        assert_eq!(fmt_send_db(1.0), "0.0 dB");
        assert_eq!(fmt_send_db(0.0), "−∞ dB");
        assert_eq!(fmt_send_short(0.5), "−6.0");
        assert_eq!(fmt_send_short(TrackSend::MAX_LEVEL), "6.0");
    }
}
