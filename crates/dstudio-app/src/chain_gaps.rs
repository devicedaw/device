//! The gaps between the cards of the device chain: the output meter of each
//! device (lower part of the gap after its card) and the "+" before every
//! effect, which opens a menu that inserts a device at that slot.
//!
//! The meters read the engine's per-device output peaks
//! (`se_device_output_peaks`), switched on only for the devices of the chain
//! on screen.

use crate::DeviceCard;
use crate::controller::{Controller, header_meter};
use crate::devices::{CARD_GAP, CHAIN_X0, SYNTH_W};
use dstudio_core::modulation as modu;
use dstudio_core::{DeviceId, TrackId, db_to_gain};
use std::collections::{HashMap, HashSet};
use std::time::Instant;

/// Where an inserted device goes: before `t.devices[index]`, inside the chain
/// `chain` when that device is a chain member.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InsertAt {
    pub track: TrackId,
    pub index: usize,
    pub chain: Option<DeviceId>,
}

/// One gap (x: its centre, bottom-panel relative, without scroll).
#[derive(Debug, Clone, PartialEq)]
pub struct Gap {
    pub x: f32,
    /// The device whose output the gap's meter shows.
    pub meter: Option<DeviceId>,
    /// The slot the gap's "+" inserts at.
    pub plus: Option<InsertAt>,
}

/// Chain meters: which devices the engine meters and their displayed levels.
#[derive(Debug, Default)]
pub struct GapMeters {
    on: HashSet<DeviceId>,
    /// Displayed (l, r) linear peaks, released at 24 dB/s.
    disp: HashMap<DeviceId, (f32, f32)>,
    ids: Vec<u64>,
    peaks: Vec<[f32; 2]>,
    t: Option<Instant>,
    /// Frames since metering was last re-asserted (instances can be rebuilt).
    since_assert: u32,
    /// Review states only: fixed (l, r) levels for the meters, in gap order.
    review: Vec<(f32, f32)>,
}

/// Shift of the first card when the chain starts with an effect, to leave
/// room for the "+" in front of it.
pub const FIRST_FX_SHIFT: f32 = 7.0;
/// Recent rows in the insert menu.
pub const RECENT_MAX: usize = 5;

impl Controller {
    /// The gaps of the selected track's chain for laid-out `cards` (see
    /// `chain_layout_unscrolled`): one after every device (its meter, plus the
    /// "+" when an effect follows), and a "+" before a first card that is an effect.
    pub(crate) fn chain_gaps(&self, cards: &[(f32, DeviceCard)]) -> Vec<Gap> {
        let Some(t) = self.selected_track.and_then(|t| self.project().track(t)) else { return Vec::new() };
        let half = CARD_GAP / 2.0;
        struct Slot {
            x: f32,
            w: f32,
            id: DeviceId,
            effect: bool,
            empty_chain: bool,
        }
        let mut slots = Vec::new();
        if let Some(id) = self.synth_device() {
            let folded = t.instrument_plugin.as_ref().is_some_and(|d| d.folded);
            let w = if folded { crate::device_mod::FOLDED_W } else { SYNTH_W };
            slots.push(Slot { x: CHAIN_X0, w, id, effect: false, empty_chain: false });
        }
        for (x, c) in cards {
            let id = DeviceId(c.id as u64);
            let empty_chain = t.devices.iter().position(|d| d.id == id).is_some_and(|i| {
                let d = &t.devices[i];
                modu::is_rack(d) && !d.collapsed && !d.folded && modu::members(&t.devices, i).is_empty()
            });
            slots.push(Slot { x: *x, w: c.width, id, effect: !c.instrument, empty_chain });
        }
        let before = |id: DeviceId| {
            t.devices.iter().position(|d| d.id == id).map(|index| InsertAt { track: t.id, index, chain: t.devices[index].chain })
        };
        let mut gaps = Vec::with_capacity(slots.len() + 1);
        if let Some(first) = slots.first().filter(|s| s.effect) {
            gaps.push(Gap { x: first.x - half, meter: None, plus: before(first.id) });
        }
        for (k, s) in slots.iter().enumerate() {
            let next = slots.get(k + 1).filter(|n| n.effect);
            if s.empty_chain {
                // The empty chain's "Add effect" slot sits between the head and the next card.
                gaps.push(Gap { x: s.x + s.w + half, meter: Some(s.id), plus: None });
                if let Some(n) = next {
                    gaps.push(Gap { x: n.x - half, meter: None, plus: before(n.id) });
                }
            } else {
                gaps.push(Gap { x: s.x + s.w + half, meter: Some(s.id), plus: next.and_then(|n| before(n.id)) });
            }
        }
        gaps
    }

    /// Per frame: switch the engine's output metering to the devices whose
    /// meters are on screen, read their peaks and update the gap levels.
    /// True while any chain meter shows a level.
    pub(crate) fn chain_meters_tick(&mut self) -> bool {
        let shown = self.bottom_tab == crate::controller::BottomTab::Devices && !self.bottom_collapsed();
        let want: HashSet<DeviceId> = if shown { self.devices.gaps.iter().filter_map(|g| g.meter).collect() } else { HashSet::new() };
        let m = &mut self.devices.gap_meters;
        m.since_assert += 1;
        let reassert = m.since_assert >= 30;
        if reassert {
            m.since_assert = 0;
        }
        for id in m.on.difference(&want) {
            let _ = self.engine.device_set_output_metering(id.0, false);
        }
        for id in &want {
            if reassert || !m.on.contains(id) {
                let _ = self.engine.device_set_output_metering(id.0, true);
            }
        }
        m.on = want;
        m.disp.retain(|id, _| m.on.contains(id));
        let now = Instant::now();
        let dt = m.t.map(|t| (now - t).as_secs_f32()).unwrap_or(0.0).min(0.25);
        m.t = Some(now);
        let fall = db_to_gain(-24.0 * dt);
        m.ids.clear();
        m.ids.extend(m.on.iter().map(|d| d.0));
        self.engine.device_output_peaks(&m.ids, &mut m.peaks);
        if !m.review.is_empty() {
            let metered = self.devices.gaps.iter().filter_map(|g| g.meter);
            for (id, lv) in metered.zip(m.review.iter().cycle()) {
                if let Some(k) = m.ids.iter().position(|x| *x == id.0) {
                    m.peaks[k] = [lv.0, lv.1];
                }
            }
        }
        for (id, p) in m.ids.iter().zip(&m.peaks) {
            let e = m.disp.entry(DeviceId(*id)).or_insert((0.0, 0.0));
            let l = p[0].max(e.0 * fall);
            let r = p[1].max(e.1 * fall);
            *e = (if l < 1e-5 { 0.0 } else { l }, if r < 1e-5 { 0.0 } else { r });
        }
        let mut active = false;
        let model = &self.chain_gaps_model;
        for (i, g) in self.devices.gaps.iter().enumerate() {
            let (l, r) = g.meter.and_then(|id| m.disp.get(&id).copied()).unwrap_or((0.0, 0.0));
            active |= l > 0.0 || r > 0.0;
            let (fl, fr) = (header_meter(l), header_meter(r));
            if let Some(mut row) = slint::Model::row_data(&**model, i)
                && ((row.l - fl).abs() > 1e-4 || (row.r - fr).abs() > 1e-4)
            {
                row.l = fl;
                row.r = fr;
                slint::Model::set_row_data(&**model, i, row);
            }
        }
        active
    }

    /// Review states: hold the chain meters at `levels` (linear, in gap order, repeated).
    pub(crate) fn review_chain_levels(&mut self, levels: Vec<(f32, f32)>) {
        self.devices.gap_meters.review = levels;
    }

    /// Rows of the chain gaps (scrolled), meter levels from the last tick.
    pub(crate) fn chain_gap_rows(&self, scroll: f32) -> Vec<crate::ChainGap> {
        let m = &self.devices.gap_meters;
        self.devices
            .gaps
            .iter()
            .map(|g| {
                let (l, r) = g.meter.and_then(|id| m.disp.get(&id).copied()).unwrap_or((0.0, 0.0));
                crate::ChainGap {
                    x: g.x - scroll,
                    meter: g.meter.is_some(),
                    plus: g.plus.is_some(),
                    l: header_meter(l),
                    r: header_meter(r),
                }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::controller::BottomTab;
    use dstudio_core::{Command, Project, TrackKind, builtin};

    fn controller(kind: TrackKind, effects: &[&str]) -> (Controller, TrackId) {
        let engine = std::sync::Arc::new(dstudio_engine::Engine::new(dstudio_engine::Config::default()).unwrap());
        let mut c = Controller::new(engine, Project::default());
        let t = c.project().tracks.iter().find(|t| t.kind == kind).unwrap().id;
        for id in effects {
            assert!(c.execute(Command::AddDevice { track: t, index: None, device: builtin::find(id).unwrap().instantiate() }));
        }
        c.select_track(t);
        c.set_bottom_tab(BottomTab::Devices);
        refresh(&mut c);
        (c, t)
    }

    fn refresh(c: &mut Controller) {
        let (cards, _, _) = c.chain_layout_unscrolled();
        c.devices.gaps = c.chain_gaps(&cards);
    }

    fn ids(c: &Controller, t: TrackId) -> Vec<DeviceId> {
        c.project().track(t).unwrap().devices.iter().map(|d| d.id).collect()
    }

    fn names(c: &Controller, t: TrackId) -> Vec<String> {
        c.project().track(t).unwrap().devices.iter().map(|d| d.plugin.name.clone()).collect()
    }

    /// Picks the menu row labelled `label`.
    fn pick(c: &mut Controller, label: &str) {
        let rows = &c.devices.menu.as_ref().expect("menu open").rows;
        let i = rows.iter().position(|r| r.0 == label).unwrap_or_else(|| panic!("no row {label}"));
        c.devmenu_pick(i);
    }

    /// Audio track: a "+" in front of the first effect (the card moves right to make room),
    /// then after every device its meter, with a "+" only where another effect follows.
    #[test]
    fn every_device_gets_a_meter_and_every_effect_a_plus_in_front() {
        let (c, t) = controller(TrackKind::Audio, &["delay", "compressor"]);
        let (cards, _, _) = c.chain_layout_unscrolled();
        assert_eq!(cards[0].0, CHAIN_X0 + FIRST_FX_SHIFT);
        let d = ids(&c, t);
        let g = &c.devices.gaps;
        assert_eq!(g.len(), 3);
        assert_eq!((g[0].meter, g[0].plus.map(|p| p.index)), (None, Some(0)));
        assert_eq!((g[1].meter, g[1].plus.map(|p| p.index)), (Some(d[0]), Some(1)));
        assert_eq!((g[2].meter, g[2].plus), (Some(d[1]), None));
        // Centred in the 16 pt gaps between the cards.
        assert_eq!(g[0].x, cards[0].0 - CARD_GAP / 2.0);
        assert_eq!(g[1].x, cards[0].0 + cards[0].1.width + CARD_GAP / 2.0);
    }

    /// Instrument track: the Synth gets a meter and the "+" before the first effect; there is
    /// no "+" in front of the instrument and the cards keep their place.
    #[test]
    fn the_instrument_is_metered_but_nothing_goes_in_front_of_it() {
        let (c, t) = controller(TrackKind::Instrument, &["delay"]);
        let synth = c.synth_device().expect("built-in Synth");
        let (cards, _, _) = c.chain_layout_unscrolled();
        assert_eq!(cards[0].0, CHAIN_X0 + SYNTH_W + CARD_GAP);
        let g = &c.devices.gaps;
        assert_eq!(g.len(), 2);
        assert_eq!((g[0].meter, g[0].plus.map(|p| p.index)), (Some(synth), Some(0)));
        assert_eq!((g[1].meter, g[1].plus), (Some(ids(&c, t)[0]), None));
    }

    /// The "+" between two effects inserts there, and the pick shows up under Recent next time.
    #[test]
    fn plus_inserts_at_its_slot_and_remembers_the_effect() {
        let (mut c, t) = controller(TrackKind::Audio, &["delay", "compressor"]);
        c.chain_insert_menu(1, 0.0, 0.0);
        assert!(!c.devices.menu.as_ref().unwrap().rows.iter().any(|r| r.0 == "Recent"), "nothing added yet");
        pick(&mut c, "Built-in");
        pick(&mut c, "Gate");
        assert_eq!(names(&c, t), ["Delay", "Gate", "Compressor"]);
        // One undo step takes it back.
        c.undo();
        assert_eq!(names(&c, t), ["Delay", "Compressor"]);
        refresh(&mut c);
        c.chain_insert_menu(0, 0.0, 0.0);
        let rows = &c.devices.menu.as_ref().unwrap().rows;
        assert_eq!(rows[0].0, "Recent");
        assert_eq!(rows[1].0, "Gate");
        pick(&mut c, "Gate");
        assert_eq!(names(&c, t), ["Gate", "Delay", "Compressor"]);
    }

    /// A "+" in front of a chain member inserts into that chain.
    #[test]
    fn plus_inside_a_chain_adds_a_member() {
        let (mut c, t) = controller(TrackKind::Audio, &["device-rack"]);
        let head = ids(&c, t)[0];
        c.add_to_chain(head, crate::devices::Item::Builtin(builtin::find("delay").unwrap()));
        refresh(&mut c);
        // Gaps: + before the head, head (meter + "+" before the member), member.
        let at = c.devices.gaps[1].plus.expect("+ before the member");
        assert_eq!((at.index, at.chain), (1, Some(head)));
        c.chain_insert_menu(1, 0.0, 0.0);
        pick(&mut c, "Built-in");
        pick(&mut c, "Gate");
        let devs = &c.project().track(t).unwrap().devices;
        assert_eq!(devs.iter().map(|d| d.plugin.name.as_str()).collect::<Vec<_>>()[1], "Gate");
        assert_eq!(devs[1].chain, Some(head));
        assert_eq!(dstudio_core::modulation::members(devs, 0), 1..3);
    }
}
