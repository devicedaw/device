//! Modulation and containers on every device card: the LFO and chain-head
//! bodies, the plugin card for third-party plugins, the generic header
//! actions (fold, swap, collapse, rename) and the per-control menu that
//! aims LFOs and macros at a parameter.
//!
//! Model: a source (LFO or chain head, dstudio_core::modulation) owns its
//! links; the engine applies them (se_mod_route). Every edit here is one
//! command, so one undo step. Live positions come from the LFO readouts
//! (position, depth, cycle) polled with the meters.

use crate::controller::{Controller, card_color};
use crate::device_panel::Z;
use crate::devices::{DevMenu, Item, MenuPick, format_badge};
use crate::{ChainFrame, DeviceCard, ModCard, ModTarget, PanelCanvas, PanelKnob, PanelPath, PanelPick, PanelSlider, PanelText, PanelZone};
use dstudio_core::builtin::{self, DeviceDef};
use dstudio_core::modulation::{self as modu, DEFAULT_AMOUNT};
use dstudio_core::{Command, Device, DeviceId, ModLink, ModParam, PluginFormat, PluginKind, TrackId};
use slint::{ModelRc, SharedString, VecModel};
use std::collections::HashMap;
use std::rc::Rc;

/// `param` of a control menu request that names the device's power switch.
pub const POWER_PARAM: i32 = -2;

/// LFO card (pt at 100%): width, left block, wave picture.
const LFO_W: f32 = 440.0;
const LFO_LEFT_W: f32 = 276.0;
const WAVE_X: f32 = 8.0;
const WAVE_Y: f32 = 63.0;
const WAVE_BOTTOM: f32 = 214.0;
/// Chain head: closed / side-panel-open width, macro cell, row pitch.
pub const CHAIN_W: f32 = 240.0;
pub const CHAIN_OPEN_W: f32 = 448.0;
const MACRO_CELL: f32 = 84.0;
const MACRO_GAP: f32 = 8.0;
const MACRO_ROW: f32 = 65.0;
const MACRO_TOP: f32 = 40.0;
/// Folded card strip width (pt) and the empty chain's "Add effect" target.
pub const FOLDED_W: f32 = 30.0;
pub const EMPTY_CHAIN_W: f32 = 182.5;
/// Plugin card: columns x rows of controls per page.
const CARD_COLS: usize = 3;
const CARD_ROWS: usize = 3;
/// Modulation colours by source order on a track (the macro colours).
const SOURCE_COLORS: [u32; 4] = builtin::rack::MACRO_COLORS;

/// Live state of a link amount drag.
#[derive(Debug, Clone, Copy)]
struct AmountDrag {
    source: DeviceId,
    index: usize,
    start: (f32, f32),
    value: f32,
    active: bool,
    travel: f32,
    /// Movement before the start threshold, applied once the drag starts.
    held: f32,
}

#[derive(Debug, Default)]
pub struct ModUi {
    /// Armed source output for mapping: the next control clicked becomes a target.
    pub map: Option<(DeviceId, u8)>,
    amount: Option<AmountDrag>,
    /// Chain heads with the macro side panel open, and the selected macro.
    pub macro_open: HashMap<DeviceId, u8>,
    /// Latest LFO readouts: (position 0..1, depth 0..1, cycle 0..1).
    pub lfo: HashMap<DeviceId, (f32, f32, f32)>,
    /// Plugin card pages.
    pub pages: HashMap<DeviceId, usize>,
    /// Plugin parameter lists by device (filled on first use, dropped on change).
    pub plugin_params: std::cell::RefCell<HashMap<DeviceId, Vec<dstudio_engine::PluginParamInfo>>>,
    /// Plugin knob drag: (device, param id, start, value, active, travel).
    plugin_drag: Option<(DeviceId, u32, f32, f32, bool, f32, f32)>,
    /// A project was just opened: restore the saved modulation bases after the next publish.
    pub(crate) bases_pending: bool,
}

fn s(v: impl Into<SharedString>) -> SharedString {
    v.into()
}

fn model<T: Clone + 'static>(v: Vec<T>) -> ModelRc<T> {
    ModelRc::from(Rc::new(VecModel::from(v)))
}

fn rgb(c: u32) -> slint::Color {
    slint::Color::from_rgb_u8((c >> 16) as u8, (c >> 8) as u8, c as u8)
}

fn pct(v: f32) -> String {
    format!("{:.0}%", v * 100.0)
}

pub(crate) type MenuRows = Vec<(String, String, Option<MenuPick>, bool)>;

/// LFO picture: two cycles of the shape (phase applied) in a w x h box, scaled by depth.
fn wave_paths(shape: usize, phase: f64, depth: f32, w: f32, h: f32) -> (String, String) {
    use std::fmt::Write;
    let n = 160;
    let mid = h / 2.0;
    let amp = (h / 2.0 - 3.0) * depth.max(0.04);
    let mut line = String::with_capacity(n * 14);
    for i in 0..=n {
        let t = i as f64 / n as f64 * 2.0;
        let y = mid - amp * modu::lfo_wave(shape, t + phase);
        let _ = write!(line, "{}{:.1} {y:.1}", if i == 0 { "M" } else { " L" }, w * i as f32 / n as f32);
    }
    let fill = format!("{line} L{w:.1} {mid:.1} L0 {mid:.1} Z");
    (line, fill)
}

impl Controller {
    fn track_of(&self, id: DeviceId) -> Option<&dstudio_core::Track> {
        self.project().find_device(id).map(|(ti, _)| &self.project().tracks[ti])
    }

    /// Name of a link target ("Gate Threshold").
    pub(crate) fn mod_param_name(&self, target: DeviceId, param: &ModParam) -> String {
        let Some(d) = self.project().device(target) else {
            return "Removed".into();
        };
        let p = match param {
            ModParam::Builtin(id) => builtin::def_of(d).and_then(|def| def.param(id)).map_or_else(|| id.clone(), |p| p.name.to_string()),
            ModParam::Plugin(id) => self.plugin_param_info(d.id, *id).map_or_else(|| format!("Parameter {id}"), |i| i.name),
        };
        format!("{} {p}", d.display_name())
    }

    fn plugin_param_info(&self, device: DeviceId, id: u32) -> Option<dstudio_engine::PluginParamInfo> {
        self.plugin_params_of(device).into_iter().find(|p| p.id == id)
    }

    fn plugin_params_of(&self, device: DeviceId) -> Vec<dstudio_engine::PluginParamInfo> {
        let mut cache = self.devices.modu.plugin_params.borrow_mut();
        let v = cache.entry(device).or_default();
        if v.is_empty() {
            *v = self.engine.plugin_params(device);
        }
        v.clone()
    }

    // ---------------------------------------------------------------- shared card state

    /// Header state and modulation rings for any card (built-in or plugin).
    pub(crate) fn card_common(&self, d: &Device, card: &mut DeviceCard) {
        let Some(t) = self.track_of(d.id) else { return };
        let at = t.devices.iter().position(|x| x.id == d.id);
        card.folded = d.folded;
        card.member = at.and_then(|i| modu::head_of(&t.devices, i)).is_some();
        // Instrument cards swap through the instrument selector.
        card.swappable = at.is_none() || modu::outputs(d) == 0;
        if d.label.is_some() {
            card.name = s(d.display_name());
        }
        if card.folded {
            card.width = FOLDED_W;
        }
        if !card.builtin {
            card.accent = card_color(t.color);
        }
        let n = slint::Model::row_count(&card.knobs);
        if n == 0 || modu::outputs(d) > 0 {
            return;
        }
        let def = builtin::def_of(d);
        let mut knobs: Vec<PanelKnob> = (0..n).filter_map(|i| slint::Model::row_data(&card.knobs, i)).collect();
        for k in knobs.iter_mut() {
            let param = match def {
                Some(def) => match usize::try_from(k.param).ok().and_then(|i| def.params.get(i)) {
                    Some(p) => ModParam::Builtin(p.id.to_string()),
                    None => continue,
                },
                None => ModParam::Plugin(k.param as u32),
            };
            if let Some((lo, hi, now, color)) = self.mod_reach(t, d.id, &param, k.norm) {
                k.mod_on = true;
                k.mod_lo = lo;
                k.mod_hi = hi;
                k.mod_now = now;
                k.mod_color = color;
            }
        }
        card.knobs = model(knobs);
    }

    /// Modulation reach on a built-in device's parameter (see `mod_reach`).
    pub(crate) fn mod_reach_of(&self, device: DeviceId, param: &str, base: f32) -> Option<(f32, f32, f32, slint::Color)> {
        let t = self.track_of(device)?;
        self.mod_reach(t, device, &ModParam::Builtin(param.to_string()), base)
    }

    /// Reach of every source on a parameter around its set value `base`
    /// (normalized): (low, high, live value, first source colour).
    fn mod_reach(&self, t: &dstudio_core::Track, target: DeviceId, param: &ModParam, base: f32) -> Option<(f32, f32, f32, slint::Color)> {
        let order: Vec<DeviceId> = t.devices.iter().filter(|d| modu::outputs(d) > 0).map(|d| d.id).collect();
        let (mut lo, mut hi, mut now, mut add) = (base, base, base, 0.0f32);
        let mut color = None;
        for (sid, li) in modu::sources_of(t, target, param) {
            let Some(src) = t.devices.iter().find(|d| d.id == sid) else {
                continue;
            };
            let Some(l) = src.links.get(li) else { continue };
            let ci = order.iter().position(|x| *x == sid).unwrap_or(0) + l.slot as usize;
            color.get_or_insert(rgb(SOURCE_COLORS[ci % SOURCE_COLORS.len()]));
            if !src.enabled {
                continue;
            }
            if modu::is_rack(src) {
                lo = l.min.min(l.max).clamp(0.0, 1.0);
                hi = l.min.max(l.max).clamp(0.0, 1.0);
                now = modu::macro_target(l, self.macro_now(src, l.slot));
            } else {
                let def = &builtin::lfo::LFO;
                let depth = def.param("depth").map_or(1.0, |p| self.panel_value_of(src, def, p) / 100.0);
                let pos = self.devices.modu.lfo.get(&sid).map_or(0.5, |r| r.0);
                lo += depth * l.min.min(l.max);
                hi += depth * l.min.max(l.max);
                add += depth * (l.min + (l.max - l.min) * pos);
            }
        }
        let color = color?;
        Some((lo.clamp(0.0, 1.0), hi.clamp(0.0, 1.0), (now + add).clamp(0.0, 1.0), color))
    }

    /// Macro position 0..1, the live drag value included.
    fn macro_now(&self, head: &Device, slot: u8) -> f32 {
        let def = &builtin::rack::RACK;
        def.params.get(slot as usize).map_or(0.0, |p| self.panel_value_of(head, def, p) / 100.0)
    }

    // ---------------------------------------------------------------- LFO and chain bodies

    /// Custom body of an LFO or chain head (the generic card is the base).
    pub(crate) fn mod_body(&self, d: &Device, def: &'static DeviceDef, card: &mut DeviceCard) {
        match def.id {
            modu::LFO_ID => self.lfo_body(d, def, card),
            modu::RACK_ID => self.chain_body(d, def, card),
            _ => {}
        }
    }

    fn targets(&self, src: &Device, slot: Option<u8>) -> Vec<ModTarget> {
        src.links
            .iter()
            .enumerate()
            .filter(|(_, l)| slot.is_none_or(|s| l.slot == s))
            .map(|(i, l)| ModTarget { index: i as i32, name: s(self.mod_param_name(l.target, &l.param)), amount: s(pct(l.max)) })
            .collect()
    }

    fn lfo_body(&self, d: &Device, def: &'static DeviceDef, card: &mut DeviceCard) {
        let idx = |id: &str| def.index_of(id).map_or(-1, |i| i as i32);
        let p = |id: &str| def.param(id).expect("lfo parameter");
        let val = |id: &str| self.panel_value_of(d, def, p(id));
        let sync = val("sync") >= 0.5;
        let picks = vec![
            PanelPick {
                kind: 2,
                param: idx("shape"),
                label: s("Shape"),
                value: s(p("shape").format(val("shape"))),
                enabled: true,
                width: 118.0 * Z,
                ..Default::default()
            },
            PanelPick {
                kind: 1,
                param: idx("sync"),
                label: s(""),
                choices: model(p("sync").choices.iter().map(|c| s(*c)).collect()),
                selected: sync as i32,
                enabled: true,
                ..Default::default()
            },
        ];
        let knob = |id: &str, x: f32| {
            let v = val(id);
            PanelKnob {
                param: idx(id),
                x: x * Z,
                y: crate::device_panel::DIAL_Y * Z,
                label: s(p(id).label),
                value: s(p(id).format(v)),
                norm: p(id).to_normalized(v),
                ..Default::default()
            }
        };
        let knobs = vec![knob(if sync { "rateSync" } else { "rate" }, 8.0), knob("phase", 80.0)];
        let depth = val("depth");
        let slider_x = LFO_LEFT_W - 8.0 - 62.0;
        let sliders = vec![PanelSlider {
            param: idx("depth"),
            x: slider_x * Z,
            label: s(p("depth").label),
            value: s(p("depth").format(depth)),
            norm: p("depth").to_normalized(depth),
            zero: 0.0,
        }];
        let (w, h) = ((slider_x - 10.0 - WAVE_X) * Z, (WAVE_BOTTOM - WAVE_Y) * Z);
        let phase = val("phase") as f64 / 100.0;
        let (wave, wave_fill) = wave_paths(val("shape").round() as usize, phase, depth / 100.0, w, h);
        // The dot: the live cycle position in the first drawn cycle.
        let (cyc, pos) = self.devices.modu.lfo.get(&d.id).map_or((phase as f32, 0.5), |r| (r.2, r.0));
        let dot_x = ((cyc as f64 - phase).rem_euclid(1.0) as f32) * w / 2.0;
        let dot_y = h / 2.0 - (h / 2.0 - 3.0) * (depth / 100.0).max(0.04) * (pos * 2.0 - 1.0);
        // The picture is a canvas: drag sideways for Rate, up or down for Depth (one undo step).
        let accent = card.accent;
        let path = |commands: String, stroke: slint::Color, width: f32, fill: slint::Color| PanelPath {
            commands: s(commands),
            stroke,
            width,
            fill,
            fill2: fill,
        };
        let clear = slint::Color::from_argb_u8(0, 0, 0, 0);
        let r = 3.5 * Z;
        card.canvas = PanelCanvas {
            visible: true,
            well: true,
            x: WAVE_X * Z,
            y: WAVE_Y * Z,
            w,
            h,
            paths: model(vec![
                path(
                    format!("M0 {:.1} L{w:.1} {:.1} M{:.1} 0 L{:.1} {h:.1}", h / 2.0, h / 2.0, w / 2.0, w / 2.0),
                    slint::Color::from_argb_u8(0x14, 255, 255, 255),
                    0.5,
                    clear,
                ),
                path(wave_fill, clear, 0.0, accent.with_alpha(0.18)),
                path(wave, accent, 1.6 * Z, clear),
                path(
                    format!("M{:.1} {dot_y:.1} m-{r} 0 a{r} {r} 0 1 0 {} 0 a{r} {r} 0 1 0 -{} 0", dot_x, 2.0 * r, 2.0 * r),
                    accent,
                    1.0,
                    slint::Color::from_rgb_u8(0xf2, 0xf2, 0xf2),
                ),
            ]),
            texts: model(vec![PanelText {
                x: w - 60.0 * Z,
                y: 4.0 * Z,
                w: 54.0 * Z,
                h: 12.0 * Z,
                text: s("Wave"),
                color: slint::Color::from_argb_u8(0x8c, 255, 255, 255),
                size: 10.0 * Z,
                weight: 500,
                align: 2,
            }]),
            zones: model(vec![PanelZone {
                x: 0.0,
                y: 0.0,
                w,
                h,
                param_x: idx(if sync { "rateSync" } else { "rate" }),
                param_y: idx("depth"),
                rate_x: 1.0 / 200.0,
                rate_y: 1.0 / 200.0,
            }]),
            ..Default::default()
        };
        card.width = LFO_W * Z;
        card.picks = model(picks);
        card.knobs = model(knobs);
        card.sliders = model(sliders);
        card.modv = ModCard {
            kind: 1,
            targets: model(self.targets(d, None)),
            map_armed: self.devices.modu.map.is_some_and(|(id, _)| id == d.id),
            ..Default::default()
        };
    }

    fn chain_body(&self, d: &Device, def: &'static DeviceDef, card: &mut DeviceCard) {
        let open = self.devices.modu.macro_open.get(&d.id).copied();
        let knobs = def
            .params
            .iter()
            .enumerate()
            .map(|(i, p)| {
                let v = self.panel_value_of(d, def, p);
                PanelKnob {
                    param: i as i32,
                    x: (8.0 + (i % 2) as f32 * (MACRO_CELL + MACRO_GAP)) * Z,
                    y: (MACRO_TOP + (i / 2) as f32 * MACRO_ROW) * Z,
                    label: s(modu::macro_name(d, i).to_uppercase()),
                    value: s(p.format(v)),
                    norm: p.to_normalized(v),
                    mapped: d.links.iter().any(|l| l.slot as usize == i),
                    selected: open == Some(i as u8),
                    mod_color: rgb(SOURCE_COLORS[i % SOURCE_COLORS.len()]),
                    ..Default::default()
                }
            })
            .collect();
        let members = self
            .track_of(d.id)
            .and_then(|t| t.devices.iter().position(|x| x.id == d.id).map(|hi| modu::members(&t.devices, hi).len()))
            .unwrap_or(0);
        card.width = if open.is_some() { CHAIN_OPEN_W * Z } else { CHAIN_W * Z };
        card.knobs = model(knobs);
        card.picks = model(Vec::new());
        card.sliders = model(Vec::new());
        card.preset = s("");
        let slot = open.unwrap_or(0);
        card.modv = ModCard {
            kind: 2,
            targets: model(open.map(|o| self.targets(d, Some(o))).unwrap_or_default()),
            map_armed: self.devices.modu.map.is_some_and(|(id, m)| id == d.id && Some(m) == open),
            slot: slot as i32,
            macro_name: s(modu::macro_name(d, slot as usize)),
            macro_open: open.is_some(),
            members: members as i32,
            collapsed: d.collapsed,
        };
    }

    /// Frames around chain heads and their members (bottom-panel x of the laid-out cards).
    pub(crate) fn chain_frames(&self, cards: &[(f32, DeviceCard)]) -> Vec<ChainFrame> {
        let Some(t) = self.selected_track.and_then(|t| self.project().track(t)) else {
            return Vec::new();
        };
        let accent = card_color(t.color);
        let mut out = Vec::new();
        for (hi, h) in t.devices.iter().enumerate().filter(|(_, d)| modu::is_rack(d)) {
            let Some((x0, hc)) = cards.iter().find(|(_, c)| c.id as u64 == h.id.0) else {
                continue;
            };
            let run = modu::members(&t.devices, hi);
            let empty = run.is_empty() && !h.collapsed && !h.folded;
            let last = run.filter_map(|i| cards.iter().find(|(_, c)| c.id as u64 == t.devices[i].id.0)).next_back();
            let end = match last {
                Some((x, c)) => x + c.width,
                None if empty => x0 + hc.width + crate::devices::CARD_GAP + EMPTY_CHAIN_W,
                None => x0 + hc.width,
            };
            out.push(ChainFrame {
                x: x0 - 5.0,
                w: end - x0 + 10.0,
                armed: self.devices.modu.map.is_some_and(|(id, _)| id == h.id),
                color: accent,
                empty_x: x0 + hc.width + crate::devices::CARD_GAP,
                empty,
                head: h.id.0 as i32,
            });
        }
        out
    }

    // ---------------------------------------------------------------- plugin card

    /// Body of a third-party plugin card: the controls the user added, then
    /// every parameter a source or automation moves, three rows per page.
    /// Parameters on a plugin card: the ones the user added, then every one a
    /// source or automation moves.
    fn plugin_card_ids(&self, d: &Device) -> Vec<u32> {
        let Some(t) = self.track_of(d.id) else { return Vec::new() };
        let moved = |id: u32| {
            !modu::sources_of(t, d.id, &ModParam::Plugin(id)).is_empty()
                || t.automation.iter().any(|l| matches!(&l.target, dstudio_core::AutomationTarget::Plugin { device, param, .. } if *device == d.id && *param == id))
        };
        let mut ids: Vec<u32> = d.card_params.clone();
        ids.extend(self.plugin_params_of(d.id).iter().filter(|p| !d.card_params.contains(&p.id) && moved(p.id)).map(|p| p.id));
        ids
    }

    pub(crate) fn plugin_body(&self, d: &Device, card: &mut DeviceCard) {
        if self.track_of(d.id).is_none() {
            return;
        }
        let infos = self.plugin_params_of(d.id);
        let ids = self.plugin_card_ids(d);
        let per = CARD_COLS * CARD_ROWS;
        let pages = ids.len().div_ceil(per).max(1);
        let page = self.devices.modu.pages.get(&d.id).copied().unwrap_or(0).min(pages - 1);
        let knobs: Vec<PanelKnob> = ids
            .iter()
            .skip(page * per)
            .take(per)
            .enumerate()
            .map(|(i, id)| {
                let info = infos.iter().find(|p| p.id == *id);
                let norm = match self.devices.modu.plugin_drag {
                    Some((dev, pid, _, v, _, _, _)) if dev == d.id && pid == *id => v,
                    _ => self.engine.mod_base(d.id.0, *id).or_else(|| self.engine.plugin_param_value(d.id, *id)).unwrap_or(0.0),
                };
                PanelKnob {
                    param: *id as i32,
                    x: (8.0 + (i % CARD_COLS) as f32 * 68.0) * Z,
                    y: (40.0 + (i / CARD_COLS) as f32 * 76.0) * Z,
                    label: s(info.map_or_else(|| format!("Parameter {id}"), |p| p.name.clone())),
                    value: s(self.engine.plugin_param_text(d.id, *id, norm).unwrap_or_else(|| pct(norm))),
                    // The value editor takes a percentage of the range (card_text).
                    edit: s(pct(norm)),
                    norm,
                    // 2 = on/off switch (two states), 3 = menu (more steps).
                    face: match info.map_or(0, |p| p.steps) {
                        0 => 0,
                        1 => 2,
                        _ => 3,
                    },
                    ..Default::default()
                }
            })
            .collect();
        card.knobs = model(knobs);
        card.page = page as i32;
        card.pages = pages as i32;
    }

    // ---------------------------------------------------------------- live positions

    /// LFO readouts of the selected track (each poll tick).
    pub(crate) fn mod_poll(&mut self) {
        let Some(t) = self.selected_track.and_then(|t| self.project().track(t)) else {
            return;
        };
        let ids: Vec<DeviceId> = t.devices.iter().filter(|d| modu::is_lfo(d)).map(|d| d.id).collect();
        self.devices.modu.lfo.retain(|id, _| ids.contains(id));
        let mut out = Vec::new();
        for id in ids {
            if self.engine.builtin_readouts(id.0, &mut out).is_ok() && out.len() >= 3 {
                let r = (out[0], out[1], out[2]);
                if self.devices.modu.lfo.insert(id, r) != Some(r) {
                    self.ui_dirty = true;
                }
            }
        }
    }

    // ---------------------------------------------------------------- header actions

    /// Header spark: select the device's track and open the assistant with it as context.
    pub fn device_ask(&mut self, id: DeviceId) {
        let p = self.project();
        let Some((ti, _)) = p.find_device(id) else { return };
        let track = p.tracks[ti].id;
        self.select_track(track);
        self.select_clip(None);
        self.assistant.context_removed = None;
        self.assistant_open = true;
        self.ui_dirty = true;
    }

    /// Header arrow of a built-in card: show or hide its floating device window.
    pub fn device_window(&mut self, id: DeviceId) {
        if self.project().device(id).is_none() {
            return;
        }
        if self.devices.windows.remove(&id) {
            // Closed: the engine stops measuring its In / Out peaks.
            let _ = self.engine.builtin_set_metering(id.0, false);
        } else {
            self.devices.windows.insert(id);
        }
        self.ui_dirty = true;
    }

    /// Device window dragged: keep its offset (clamped on screen by the window itself).
    pub fn device_window_moved(&mut self, id: DeviceId, dx: f32, dy: f32) {
        self.devices.window_at.insert(id, (dx, dy));
    }

    pub fn device_fold(&mut self, id: DeviceId) {
        let Some(d) = self.project().device(id) else {
            return;
        };
        let (folded, collapsed) = (!d.folded, d.collapsed);
        self.execute(Command::SetDeviceView { device: id, folded, collapsed });
    }

    pub fn device_collapse(&mut self, id: DeviceId) {
        let Some(d) = self.project().device(id) else {
            return;
        };
        let (folded, collapsed) = (d.folded, !d.collapsed);
        self.execute(Command::SetDeviceView { device: id, folded, collapsed });
    }

    pub fn device_rename(&mut self, id: DeviceId, text: &str) {
        let Some(d) = self.project().device(id) else {
            return;
        };
        let label = Some(text.trim().to_string()).filter(|l| !l.is_empty() && *l != d.plugin.name);
        if label != d.label {
            self.execute(Command::SetDeviceLabel { device: id, label });
        }
    }

    /// Built-in effects and effect plugins as menu rows, each turned into an action.
    fn effect_rows(&self, pick: impl Fn(Item) -> MenuPick, checked: impl Fn(&Item) -> bool) -> MenuRows {
        let mark = |it: &Item, right: String| {
            if checked(it) { "✓".to_string() } else { right }
        };
        let mut rows: MenuRows = vec![("Built-in".to_string(), String::new(), None, false)];
        for def in builtin::all().iter().copied().filter(|def| def.kind == PluginKind::Effect && builtin::listed(def)) {
            let it = Item::Builtin(def);
            rows.push((def.name.to_string(), mark(&it, def.category.to_string()), Some(pick(it)), false));
        }
        let plugins: Vec<_> = self.devices.catalog.of_kind(PluginKind::Effect).into_iter().map(|p| p.plugin.clone()).collect();
        if !plugins.is_empty() {
            rows.push(("Plugins".to_string(), String::new(), None, true));
        }
        for p in plugins {
            let right = format_badge(p.format).to_string();
            let it = Item::Plugin(p);
            rows.push((it.name().to_string(), mark(&it, right), Some(pick(it)), false));
        }
        rows
    }

    /// "Choose effect plugin" from a card name: swap the device in place.
    pub fn device_swap_menu(&mut self, id: DeviceId, x: f32, y: f32) {
        let Some(d) = self.project().device(id) else {
            return;
        };
        if modu::outputs(d) > 0 || !self.track_of(id).is_some_and(|t| t.devices.iter().any(|x| x.id == id)) {
            return;
        }
        let cur = d.plugin.clone();
        let mut rows = self.effect_rows(
            |it| MenuPick::Swap(id, it),
            |it| match it {
                Item::Builtin(def) => cur.format == PluginFormat::Builtin && cur.id == def.id,
                Item::Plugin(p) => cur.format == p.format && cur.id == p.id,
                Item::BuiltinSynth => false,
            },
        );
        rows.push(("Modulation".to_string(), String::new(), None, true));
        rows.push(("LFO".to_string(), "New".to_string(), Some(MenuPick::AddLfo(id)), false));
        self.devices.menu = Some(DevMenu { pos: (x, y), w: 220.0, rows });
        self.ui_dirty = true;
    }

    pub(crate) fn device_swap(&mut self, id: DeviceId, item: Item) {
        let mut with = match item {
            Item::Builtin(def) => def.instantiate(),
            Item::Plugin(p) => Device::new(p),
            Item::BuiltinSynth => return,
        };
        let Some(d) = self.project().device(id) else {
            return;
        };
        if with.plugin.format == d.plugin.format && with.plugin.id == d.plugin.id {
            return;
        }
        with.chain = d.chain;
        self.engine.close_plugin_editor(id);
        self.devices.editors.remove(&id);
        self.execute(Command::ReplaceDevice { device: id, with });
    }

    /// A new empty LFO at the start of the track's chain (after the LFOs there).
    pub(crate) fn add_lfo_near(&mut self, near: DeviceId) -> Option<DeviceId> {
        let t = self.track_of(near)?;
        let track = t.id;
        let index = t.devices.iter().take_while(|d| modu::is_lfo(d)).count();
        let mut lfo = builtin::lfo::LFO.instantiate();
        lfo.label = Some(modu::next_name(t, "LFO")).filter(|n| n != "LFO");
        if !self.execute(Command::AddDevice { track, index: Some(index), device: lfo }) {
            return None;
        }
        self.project().track(track).and_then(|t| t.devices.get(index)).map(|d| d.id)
    }

    /// Removing a chain head removes its members with it (one undo step).
    pub(crate) fn remove_with_members(&mut self, id: DeviceId) -> bool {
        let Some(t) = self.track_of(id) else {
            return false;
        };
        let Some(hi) = t.devices.iter().position(|d| d.id == id && modu::is_rack(d)) else {
            return false;
        };
        let run = modu::members(&t.devices, hi);
        if run.is_empty() {
            return false;
        }
        let mut ids: Vec<DeviceId> = run.rev().map(|i| t.devices[i].id).collect();
        ids.push(id);
        for d in &ids {
            self.engine.close_plugin_editor(*d);
            self.devices.editors.remove(d);
        }
        let commands = ids.into_iter().map(|device| Command::RemoveDevice { device }).collect();
        self.execute(Command::Batch { label: "Remove Chain".into(), commands });
        true
    }

    /// Reorder after a card drag. A head moves with its members; any other
    /// device joins the chain it lands in (right after a head or member) or
    /// leaves the one it was in.
    pub(crate) fn move_in_chain(&mut self, id: DeviceId, track: TrackId, from: usize, index: usize) {
        let Some(t) = self.project().track(track) else {
            return;
        };
        let mut order: Vec<DeviceId> = t.devices.iter().map(|d| d.id).collect();
        let group: Vec<DeviceId> = if modu::is_rack(&t.devices[from]) {
            std::iter::once(from).chain(modu::members(&t.devices, from)).map(|i| t.devices[i].id).collect()
        } else {
            vec![id]
        };
        // `index` counts the cards without the dragged one: insert before that card.
        let anchor = order.iter().copied().filter(|x| *x != id).nth(index);
        if anchor.is_some_and(|a| group.contains(&a)) {
            return;
        }
        let mut commands = Vec::new();
        for (k, g) in group.iter().enumerate() {
            let cur = order.iter().position(|x| x == g).expect("listed");
            order.remove(cur);
            let at = if k == 0 {
                anchor.and_then(|a| order.iter().position(|x| *x == a)).unwrap_or(order.len())
            } else {
                order.iter().position(|x| *x == group[k - 1]).expect("placed") + 1
            };
            order.insert(at, *g);
            commands.push(Command::MoveDevice { device: *g, track, index: at });
        }
        if group.len() == 1 {
            let d = &t.devices[from];
            let at = order.iter().position(|x| *x == id).expect("placed");
            let prev = at.checked_sub(1).and_then(|p| t.devices.iter().find(|x| x.id == order[p]));
            // A collapsed or folded chain hides its members: a drop next to them stays outside.
            let open = |h: DeviceId| t.devices.iter().any(|x| x.id == h && !x.collapsed && !x.folded);
            let join = prev.and_then(|p| if modu::is_rack(p) { Some(p.id) } else { p.chain }).filter(|h| open(*h));
            let join = join.filter(|_| modu::outputs(d) == 0);
            // Leave before the moves and join after them, so Undo replays a valid order.
            if join != d.chain {
                if d.chain.is_some() {
                    commands.insert(0, Command::SetDeviceChain { device: id, chain: None });
                }
                if join.is_some() {
                    commands.push(Command::SetDeviceChain { device: id, chain: join });
                }
            }
        }
        let unchanged =
            order.iter().zip(&t.devices).all(|(a, b)| *a == b.id) && commands.iter().all(|c| matches!(c, Command::MoveDevice { .. }));
        if !unchanged {
            self.execute(Command::Batch { label: "Move Device".into(), commands });
        }
    }

    // ---------------------------------------------------------------- control menu

    /// The parameter a panel control names on `device` (built-in index or plugin id).
    fn mod_param(&self, device: DeviceId, param: i32) -> Option<ModParam> {
        let d = self.project().device(device)?;
        match builtin::def_of(d) {
            Some(def) => {
                def.params.get(usize::try_from(param).ok()?).filter(|p| p.automatable).map(|p| ModParam::Builtin(p.id.to_string()))
            }
            None => Some(ModParam::Plugin(param as u32)),
        }
    }

    /// Right-click on a control: Automate, Modulators (New LFO, the track's
    /// LFOs, Remove), Macros (inside a chain). On a modulation source (LFO,
    /// chain head) only Automate. `param` POWER_PARAM is the device's power
    /// switch: Automate On.
    pub fn mod_menu(&mut self, device: DeviceId, param: i32, x: f32, y: f32) {
        let Some(d) = self.project().device(device) else {
            return;
        };
        if param == POWER_PARAM {
            if self.track_of(device).is_some() {
                let rows: MenuRows = vec![("Automate On".to_string(), String::new(), Some(MenuPick::AutomateOn(device)), false)];
                self.devices.menu = Some(DevMenu { pos: (x, y), w: 200.0, rows });
                self.ui_dirty = true;
            }
            return;
        }
        let Some(mp) = self.mod_param(device, param) else {
            return;
        };
        let Some(t) = self.track_of(device) else {
            return;
        };
        let pname = match &mp {
            ModParam::Builtin(id) => builtin::def_of(d).and_then(|def| def.param(id)).map_or(String::new(), |p| p.name.to_string()),
            ModParam::Plugin(id) => self.plugin_param_info(device, *id).map_or_else(|| format!("Parameter {id}"), |i| i.name),
        };
        let mut rows: MenuRows = vec![(format!("Automate {pname}"), String::new(), Some(MenuPick::Automate(device, mp.clone())), false)];
        if modu::outputs(d) > 0 {
            // Sources are not modulation targets themselves.
            self.devices.menu = Some(DevMenu { pos: (x, y), w: 200.0, rows });
            self.ui_dirty = true;
            return;
        }
        rows.push(("Modulators".to_string(), String::new(), None, true));
        rows.push(("New LFO".to_string(), String::new(), Some(MenuPick::NewLfo(device, mp.clone())), false));
        let linked = |s: &Device, slot: u8| s.links.iter().position(|l| l.slot == slot && l.target == device && l.param == mp);
        for s in t.devices.iter().filter(|s| modu::is_lfo(s)) {
            rows.push(match linked(s, 0) {
                Some(i) => (format!("Remove {}", s.display_name()), String::new(), Some(MenuPick::Unlink(s.id, i)), false),
                None => (s.display_name().to_string(), String::new(), Some(MenuPick::Link(s.id, 0, device, mp.clone())), false),
            });
        }
        if let Some(h) = t.devices.iter().position(|x| x.id == device).and_then(|i| modu::head_of(&t.devices, i)).map(|h| &t.devices[h]) {
            rows.push(("Macros".to_string(), String::new(), None, true));
            for m in 0..builtin::rack::MACROS {
                let name = modu::macro_name(h, m);
                rows.push(match linked(h, m as u8) {
                    Some(i) => (format!("Remove from {name}"), String::new(), Some(MenuPick::Unlink(h.id, i)), false),
                    None => (name, String::new(), Some(MenuPick::Link(h.id, m as u8, device, mp.clone())), false),
                });
            }
        }
        if let ModParam::Plugin(id) = mp
            && d.card_params.contains(&id)
        {
            rows.push(("Remove from Device".to_string(), String::new(), Some(MenuPick::CardParam(device, id)), true));
        }
        self.devices.menu = Some(DevMenu { pos: (x, y), w: 200.0, rows });
        self.ui_dirty = true;
    }

    /// Add a link from source output `slot` to a parameter (a refusal goes to the status line).
    pub(crate) fn mod_link(&mut self, source: DeviceId, slot: u8, target: DeviceId, param: ModParam) -> bool {
        let Some(src) = self.project().device(source) else {
            return false;
        };
        let Some(t) = self.track_of(source) else {
            return false;
        };
        if let Some(why) = modu::link_refusal(t, src, slot, target, &param) {
            self.set_status(why, 4);
            return false;
        }
        let link = if modu::is_rack(src) {
            // A macro starts at the parameter's current value and reaches the top.
            let base = self.mod_base_norm(target, &param);
            ModLink { slot, target, param, min: base, max: 1.0 }
        } else {
            ModLink { slot, target, param, min: -DEFAULT_AMOUNT, max: DEFAULT_AMOUNT }
        };
        self.panel_commit(Command::AddModLink { source, index: None, link })
    }

    fn mod_base_norm(&self, target: DeviceId, param: &ModParam) -> f32 {
        let Some(d) = self.project().device(target) else {
            return 0.0;
        };
        match param {
            ModParam::Builtin(id) => {
                builtin::def_of(d).and_then(|def| def.param(id).map(|p| p.to_normalized(def.value(d, p)))).unwrap_or(0.0)
            }
            ModParam::Plugin(id) => self.engine.plugin_param_value(target, *id).unwrap_or(0.0),
        }
    }

    /// Modulators > New LFO: a new LFO aimed at the control, one undo step.
    pub(crate) fn mod_new_lfo(&mut self, device: DeviceId, param: ModParam) {
        let Some(t) = self.track_of(device) else {
            return;
        };
        let track = t.id;
        let index = t.devices.iter().take_while(|d| modu::is_lfo(d)).count();
        let mut lfo = builtin::lfo::LFO.instantiate();
        lfo.id = DeviceId(self.project().next_id);
        lfo.label = Some(modu::next_name(t, "LFO")).filter(|n| n != "LFO");
        if let Some(why) = {
            let mut probe = t.clone();
            probe.devices.insert(index, lfo.clone());
            modu::link_refusal(&probe, &lfo, 0, device, &param)
        } {
            self.set_status(why, 4);
            return;
        }
        let source = lfo.id;
        let link = ModLink { slot: 0, target: device, param, min: -DEFAULT_AMOUNT, max: DEFAULT_AMOUNT };
        self.panel_commit(Command::Batch {
            label: "New LFO".into(),
            commands: vec![
                Command::AddDevice { track, index: Some(index), device: lfo },
                Command::AddModLink { source, index: None, link },
            ],
        });
    }

    pub(crate) fn mod_automate(&mut self, device: DeviceId, param: ModParam) {
        let Some(track) = self.track_of(device).map(|t| t.id) else {
            return;
        };
        let Some(d) = self.project().device(device) else {
            return;
        };
        let target = match param {
            ModParam::Builtin(id) => dstudio_core::AutomationTarget::Builtin { device, device_type: d.plugin.id.clone(), param: id },
            ModParam::Plugin(id) => {
                let name = self.plugin_param_info(device, id).map_or_else(|| format!("Parameter {id}"), |i| i.name);
                dstudio_core::AutomationTarget::Plugin { device, param: id, name }
            }
        };
        self.auto_choose(Some(track), target);
    }

    /// Power switch menu > Automate On: the device's on/off lane.
    pub(crate) fn mod_automate_on(&mut self, device: DeviceId) {
        let Some((track, target)) = self.device_on_target(device) else {
            return;
        };
        self.auto_choose(Some(track), target);
    }

    /// Right press on a channel control (track header, inspector, mixer strip): `kind` 0
    /// volume, 1 pan, 2 mute, 3 trim, 4 width, 5 mono; `track` < 0 is the master (volume only).
    pub fn strip_menu(&mut self, track: i32, kind: i32, x: f32, y: f32) {
        use dstudio_core::AutomationTarget as T;
        let key = if track < 0 { None } else { Some(TrackId(track as u64)) };
        if let Some(t) = key
            && self.project().track(t).is_none()
        {
            return;
        }
        let (label, target) = match (key, kind) {
            (_, 0) => ("Automate Volume", T::Volume),
            (Some(_), 1) => ("Automate Pan", T::Pan),
            (Some(_), 2) => ("Automate Mute", T::Mute),
            (Some(_), 3) => ("Automate Trim", T::Trim),
            (Some(_), 4) => ("Automate Width", T::Width),
            (Some(_), 5) => ("Automate Mono", T::Mono),
            _ => return,
        };
        let rows: MenuRows = vec![(label.to_string(), String::new(), Some(MenuPick::AutomateTrack(key, target)), false)];
        self.devices.menu = Some(DevMenu { pos: (x, y), w: 200.0, rows });
        self.ui_dirty = true;
    }

    // ---------------------------------------------------------------- source panels

    pub fn mod_map(&mut self, source: DeviceId, slot: i32) {
        let slot = slot.max(0) as u8;
        self.devices.modu.map = if self.devices.modu.map == Some((source, slot)) { None } else { Some((source, slot)) };
        if self.devices.modu.map.is_some() {
            self.set_status("Click a control on this track to add it as a target (Esc stops)", 4);
        }
        self.ui_dirty = true;
    }

    /// Escape: stop mapping. True when it did.
    pub(crate) fn mod_disarm(&mut self) -> bool {
        self.ui_dirty = true;
        self.devices.modu.map.take().is_some()
    }

    /// Map mode: a press on a control adds it to the armed source. True when consumed.
    pub(crate) fn mod_map_click(&mut self, device: DeviceId, param: i32) -> bool {
        let Some((source, slot)) = self.devices.modu.map else {
            return false;
        };
        if device == source {
            return false;
        }
        if let Some(mp) = self.mod_param(device, param) {
            self.mod_link(source, slot, device, mp);
        }
        true
    }

    /// "+": every parameter on the track this source output can move.
    pub fn mod_add_menu(&mut self, source: DeviceId, slot: i32, x: f32, y: f32) {
        let Some(t) = self.track_of(source) else {
            return;
        };
        let Some(src) = t.devices.iter().find(|d| d.id == source) else {
            return;
        };
        let slot = slot.max(0) as u8;
        let mut rows: MenuRows = Vec::new();
        for d in t
            .devices
            .iter()
            .chain(t.instrument_plugin.iter())
            .filter(|d| modu::outputs(d) == 0 && (!modu::is_rack(src) || d.chain == Some(source)))
        {
            let params: Vec<(String, ModParam)> = match builtin::def_of(d) {
                Some(def) => {
                    def.params.iter().filter(|p| p.automatable).map(|p| (p.name.to_string(), ModParam::Builtin(p.id.to_string()))).collect()
                }
                None => {
                    self.plugin_params_of(d.id).into_iter().filter(|p| p.automatable).map(|p| (p.name, ModParam::Plugin(p.id))).collect()
                }
            };
            if params.is_empty() {
                continue;
            }
            let sep = !rows.is_empty();
            rows.push((d.display_name().to_string(), String::new(), None, sep));
            for (name, mp) in params {
                rows.push(match src.links.iter().position(|l| l.slot == slot && l.target == d.id && l.param == mp) {
                    Some(i) => (name, "✓".to_string(), Some(MenuPick::Unlink(source, i)), false),
                    None => (name, String::new(), Some(MenuPick::Link(source, slot, d.id, mp)), false),
                });
            }
        }
        if rows.is_empty() {
            rows.push((
                if modu::is_rack(src) { "Add effects to the chain first" } else { "No parameters on this track" }.to_string(),
                String::new(),
                None,
                false,
            ));
        }
        self.devices.menu = Some(DevMenu { pos: (x, y), w: 220.0, rows });
        self.ui_dirty = true;
    }

    fn link_of(&self, source: DeviceId, index: usize) -> Option<(bool, ModLink)> {
        let d = self.project().device(source)?;
        Some((modu::is_rack(d), d.links.get(index)?.clone()))
    }

    /// New (min, max) for an amount: an LFO spans -a..a, a macro moves its top.
    fn amount_range(rack: bool, l: &ModLink, a: f32) -> (f32, f32) {
        let a = a.clamp(0.0, 1.0);
        if rack { (l.min, a) } else { (-a, a) }
    }

    /// Amount drag: coalesced into one undo step; Escape restores.
    pub fn mod_amount_drag(&mut self, source: DeviceId, index: usize, phase: i32, dy: f32, fine: bool) {
        match phase {
            0 => {
                let Some((_, l)) = self.link_of(source, index) else {
                    return;
                };
                self.doc.seal_coalescing();
                self.devices.modu.amount =
                    Some(AmountDrag { source, index, start: (l.min, l.max), value: l.max, active: false, travel: 0.0, held: 0.0 });
            }
            1 => {
                let Some(g) = self.devices.modu.amount.as_mut().filter(|g| g.source == source && g.index == index) else {
                    return;
                };
                g.travel += dy.abs();
                g.held += dy;
                if !g.active && g.travel < 3.0 {
                    return;
                }
                g.active = true;
                g.value = (g.value + std::mem::take(&mut g.held) / 150.0 * if fine { 0.2 } else { 1.0 }).clamp(0.0, 1.0);
                let value = g.value;
                let Some((rack, l)) = self.link_of(source, index) else {
                    return;
                };
                let (min, max) = Self::amount_range(rack, &l, value);
                self.execute(Command::SetModLink { source, index, min, max });
            }
            2 => {
                self.devices.modu.amount = None;
                self.doc.seal_coalescing();
            }
            _ => {
                let Some(g) = self.devices.modu.amount.take() else {
                    return;
                };
                if g.active {
                    self.execute(Command::SetModLink { source, index, min: g.start.0, max: g.start.1 });
                    self.doc.seal_coalescing();
                }
            }
        }
        self.ui_dirty = true;
    }

    pub fn mod_amount_text(&mut self, source: DeviceId, index: usize, text: &str) {
        let Some((rack, l)) = self.link_of(source, index) else {
            return;
        };
        let Ok(v) = text.trim().trim_end_matches('%').trim().parse::<f32>() else {
            self.set_status(format!("Amount: can't read “{}”", text.trim()), 3);
            return;
        };
        let (min, max) = Self::amount_range(rack, &l, v / 100.0);
        self.panel_commit(Command::SetModLink { source, index, min, max });
    }

    pub fn mod_amount_step(&mut self, source: DeviceId, index: usize, delta: i32, fine: bool) {
        let Some((rack, l)) = self.link_of(source, index) else {
            return;
        };
        let (min, max) = Self::amount_range(rack, &l, l.max + delta as f32 * if fine { 0.002 } else { 0.01 });
        self.panel_commit(Command::SetModLink { source, index, min, max });
    }

    pub fn mod_remove(&mut self, source: DeviceId, index: usize) {
        if self.link_of(source, index).is_some() {
            self.panel_commit(Command::RemoveModLink { source, index });
        }
    }

    /// Chain head: a click on a macro opens its side panel; the same macro again closes it.
    pub fn macro_select(&mut self, head: DeviceId, i: i32) {
        let i = i.clamp(0, builtin::rack::MACROS as i32 - 1) as u8;
        let armed = self.devices.modu.map.is_some_and(|(s, _)| s == head);
        if self.devices.modu.macro_open.get(&head) == Some(&i) {
            self.devices.modu.macro_open.remove(&head);
            if armed {
                self.devices.modu.map = None;
            }
        } else {
            self.devices.modu.macro_open.insert(head, i);
            if armed {
                self.devices.modu.map = Some((head, i));
            }
        }
        self.ui_dirty = true;
    }

    pub fn macro_rename(&mut self, head: DeviceId, i: i32, name: &str) {
        let Some(d) = self.project().device(head) else {
            return;
        };
        let index = i.max(0) as usize;
        let cur = d.macro_names.get(index).cloned().unwrap_or_default();
        let name = if name.trim() == format!("Macro {}", index + 1) { String::new() } else { name.trim().to_string() };
        if name != cur {
            self.execute(Command::SetMacroName { device: head, index, name });
        }
    }

    /// Empty chain "Add effect": the effect lists, added into the chain.
    pub fn chain_add_effect(&mut self, head: DeviceId, x: f32, y: f32) {
        let rows = self.effect_rows(|it| MenuPick::AddToChain(head, it), |_| false);
        self.devices.menu = Some(DevMenu { pos: (x, y), w: 220.0, rows });
        self.ui_dirty = true;
    }

    pub(crate) fn add_to_chain(&mut self, head: DeviceId, item: Item) {
        let Some(t) = self.track_of(head) else { return };
        let track = t.id;
        let Some(hi) = t.devices.iter().position(|d| d.id == head) else {
            return;
        };
        let index = modu::members(&t.devices, hi).end.max(hi + 1);
        let mut device = match item {
            Item::Builtin(def) => def.instantiate(),
            Item::Plugin(p) => Device::new(p),
            Item::BuiltinSynth => return,
        };
        device.chain = Some(head);
        self.execute(Command::AddDevice { track, index: Some(index), device });
    }

    // ---------------------------------------------------------------- plugin card gestures

    /// "+ Add": every parameter the plugin reports; a row toggles it on the card.
    pub fn card_add_menu(&mut self, device: DeviceId, x: f32, y: f32) {
        let Some(d) = self.project().device(device) else {
            return;
        };
        let shown = d.card_params.clone();
        let mut rows: MenuRows = Vec::new();
        let mut group: Option<String> = None;
        for p in self.plugin_params_of(device) {
            if !p.group.is_empty() && group.as_deref() != Some(p.group.as_str()) {
                group = Some(p.group.clone());
                let sep = !rows.is_empty();
                rows.push((p.group.clone(), String::new(), None, sep));
            }
            rows.push((
                p.name.clone(),
                if shown.contains(&p.id) { "✓".into() } else { String::new() },
                Some(MenuPick::CardParam(device, p.id)),
                false,
            ));
        }
        if rows.is_empty() {
            rows.push(("The plugin reports no parameters".into(), String::new(), None, false));
        }
        self.devices.menu = Some(DevMenu { pos: (x, y), w: 220.0, rows });
        self.ui_dirty = true;
    }

    pub(crate) fn card_toggle_param(&mut self, device: DeviceId, id: u32) {
        let Some(d) = self.project().device(device) else {
            return;
        };
        let mut params = d.card_params.clone();
        match params.iter().position(|p| *p == id) {
            Some(i) => {
                params.remove(i);
            }
            None => params.push(id),
        }
        self.execute(Command::SetCardParams { device, params });
    }

    pub fn card_page(&mut self, device: DeviceId, dir: i32) {
        let pages = self.project().device(device).map_or(0, |d| self.plugin_card_ids(d).len()).div_ceil(CARD_COLS * CARD_ROWS).max(1);
        let p = self.devices.modu.pages.entry(device).or_insert(0);
        *p = ((*p as i32 + dir).max(0) as usize).min(pages - 1);
        self.ui_dirty = true;
    }

    fn plugin_now(&self, device: DeviceId, id: u32) -> f32 {
        self.engine.mod_base(device.0, id).or_else(|| self.engine.plugin_param_value(device, id)).unwrap_or(0.0)
    }

    /// Plugin knob drag (panel-drag on a plugin card): values go straight to
    /// the plugin, whose state is saved with the project.
    pub(crate) fn card_drag(&mut self, device: DeviceId, id: u32, phase: i32, dy: f32, rate: f32, fine: bool) {
        match phase {
            0 => {
                let v = self.plugin_now(device, id);
                self.devices.modu.plugin_drag = Some((device, id, v, v, false, 0.0, v));
            }
            1 => {
                let steps = self.plugin_param_info(device, id).map_or(0, |p| p.steps);
                let Some(g) = self.devices.modu.plugin_drag.as_mut().filter(|g| g.0 == device && g.1 == id) else {
                    return;
                };
                g.5 += dy.abs();
                if !g.4 && g.5 < 3.0 {
                    return;
                }
                g.4 = true;
                g.6 = (g.6 + dy * rate * if fine { 0.2 } else { 1.0 }).clamp(0.0, 1.0);
                let mut v = g.6;
                if steps > 0 {
                    v = (v * steps as f32).round() / steps as f32;
                }
                if v != g.3 {
                    g.3 = v;
                    let _ = self.engine.plugin_set_param(device.0, id, v);
                }
            }
            2 => {
                let was_click = self.devices.modu.plugin_drag.take().is_some_and(|g| !g.4);
                if was_click {
                    self.card_click(device, id);
                }
            }
            _ => {
                if let Some(g) = self.devices.modu.plugin_drag.take() {
                    let _ = self.engine.plugin_set_param(device.0, id, g.2);
                }
            }
        }
        self.ui_dirty = true;
    }

    /// Plugin control reset (Alt-click, double-click): the plugin's default.
    pub(crate) fn card_reset(&mut self, device: DeviceId, id: u32) {
        if let Some(p) = self.plugin_param_info(device, id) {
            let _ = self.engine.plugin_set_param(device.0, id, p.default_normalized);
            self.ui_dirty = true;
        }
    }

    /// A click on a switch toggles it; on a stepped menu it moves to the next step.
    pub(crate) fn card_click(&mut self, device: DeviceId, id: u32) {
        let steps = self.plugin_param_info(device, id).map_or(0, |p| p.steps);
        if steps == 0 {
            return;
        }
        let k = ((self.plugin_now(device, id) * steps as f32).round() as u32 + 1) % (steps + 1);
        let _ = self.engine.plugin_set_param(device.0, id, k as f32 / steps as f32);
    }

    /// Arrow keys on a plugin control: one step, or 1% of the range (0.2% fine).
    pub(crate) fn card_step(&mut self, device: DeviceId, id: u32, delta: i32, fine: bool) {
        let steps = self.plugin_param_info(device, id).map_or(0, |p| p.steps);
        let unit = if steps > 0 {
            1.0 / steps as f32
        } else if fine {
            0.002
        } else {
            0.01
        };
        let v = (self.plugin_now(device, id) + delta as f32 * unit).clamp(0.0, 1.0);
        let _ = self.engine.plugin_set_param(device.0, id, if steps > 0 { (v * steps as f32).round() / steps as f32 } else { v });
        self.ui_dirty = true;
    }

    /// Typed plugin value: a percentage of the range.
    pub(crate) fn card_text(&mut self, device: DeviceId, id: u32, text: &str) {
        match text.trim().trim_end_matches('%').trim().parse::<f32>() {
            Ok(v) if v.is_finite() => {
                let _ = self.engine.plugin_set_param(device.0, id, (v / 100.0).clamp(0.0, 1.0));
                self.ui_dirty = true;
            }
            _ => self.set_status(format!("Can't read “{}” (type a percentage)", text.trim()), 3),
        }
    }

    /// (plugin device, [(parameter, base)]) of every modulated plugin parameter, for saving.
    pub(crate) fn mod_bases_now(&self) -> Vec<(DeviceId, Vec<(u32, f32)>)> {
        let mut out: Vec<(DeviceId, Vec<(u32, f32)>)> = Vec::new();
        for t in &self.project().tracks {
            for l in t.devices.iter().flat_map(|s| s.links.iter()) {
                let ModParam::Plugin(p) = l.param else { continue };
                let Some(b) = self.engine.mod_base(l.target.0, p) else { continue };
                match out.iter_mut().find(|(d, _)| *d == l.target) {
                    Some((_, v)) if !v.iter().any(|(q, _)| *q == p) => v.push((p, b)),
                    Some(_) => {}
                    None => out.push((l.target, vec![(p, b)])),
                }
            }
        }
        out
    }

    /// After a project opens: the saved plugin state holds the moving value,
    /// so put back each modulated parameter's saved base (once per open).
    pub(crate) fn restore_mod_bases(&mut self) {
        if !std::mem::take(&mut self.devices.modu.bases_pending) {
            return;
        }
        let bases: Vec<(DeviceId, u32, f32)> = self
            .project()
            .tracks
            .iter()
            .flat_map(|t| t.devices.iter().chain(t.instrument_plugin.iter()))
            .flat_map(|d| d.mod_bases.iter().map(move |(p, b)| (d.id, *p, *b)))
            .collect();
        for (d, p, b) in bases {
            let _ = self.engine.plugin_set_param(d.0, p, b);
        }
    }

    /// After a model change: drop state of devices that went away.
    pub(crate) fn mod_after_change(&mut self) {
        let live: Vec<DeviceId> =
            self.project().tracks.iter().flat_map(|t| t.devices.iter().chain(t.instrument_plugin.iter())).map(|d| d.id).collect();
        self.devices.modu.plugin_params.borrow_mut().retain(|id, v| live.contains(id) && !v.is_empty());
        if self.devices.modu.map.is_some_and(|(s, _)| !live.contains(&s)) {
            self.devices.modu.map = None;
        }
        self.devices.modu.macro_open.retain(|id, _| live.contains(id));
        self.devices.modu.pages.retain(|id, _| live.contains(id));
    }
}

/// The accent needs dark ink on it: its relative luminance is above 0.3, where
/// white text on it falls under 3:1 contrast (switch thumb, selected segments).
pub fn is_light(c: slint::Color) -> bool {
    let lin = |v: u8| {
        let v = v as f32 / 255.0;
        if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
    };
    0.2126 * lin(c.red()) + 0.7152 * lin(c.green()) + 0.0722 * lin(c.blue()) > 0.3
}

/// Value text colour on the dark card: the accent's hue as an OKLCH tint at
/// lightness 0.87 with chroma capped at 0.11. On the card surface that tint
/// already clears 4.5:1 contrast, so no lightness search is needed.
pub fn accent_text(c: slint::Color) -> slint::Color {
    let lin = |v: u8| {
        let v = v as f32 / 255.0;
        if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
    };
    let (r, g, b) = (lin(c.red()), lin(c.green()), lin(c.blue()));
    let l = (0.412_221_46 * r + 0.536_332_55 * g + 0.051_445_995 * b).cbrt();
    let m = (0.211_903_5 * r + 0.680_699_5 * g + 0.107_396_96 * b).cbrt();
    let s = (0.088_302_46 * r + 0.281_718_85 * g + 0.629_978_7 * b).cbrt();
    let a = 1.977_998_5 * l - 2.428_592_2 * m + 0.450_593_7 * s;
    let bb = 0.025_904_037 * l + 0.782_771_77 * m - 0.808_675_77 * s;
    let chroma = a.hypot(bb).min(0.11);
    let hue = bb.atan2(a);
    let (a, bb, ll) = (chroma * hue.cos(), chroma * hue.sin(), 0.87f32);
    let l = (ll + 0.396_337_78 * a + 0.215_803_76 * bb).powi(3);
    let m = (ll - 0.105_561_346 * a - 0.063_854_17 * bb).powi(3);
    let s = (ll - 0.089_484_18 * a - 1.291_485_5 * bb).powi(3);
    let gamma = |v: f32| {
        let v = v.clamp(0.0, 1.0);
        let v = if v <= 0.0031308 { v * 12.92 } else { 1.055 * v.powf(1.0 / 2.4) - 0.055 };
        (v * 255.0).round() as u8
    };
    slint::Color::from_rgb_u8(
        gamma(4.076_741_7 * l - 3.307_711_6 * m + 0.230_969_94 * s),
        gamma(-1.268_438 * l + 2.609_757_4 * m - 0.341_319_38 * s),
        gamma(-0.0041960863 * l - 0.703_418_6 * m + 1.707_614_7 * s),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wave_paths_span_the_box() {
        let (line, fill) = wave_paths(0, 0.0, 1.0, 100.0, 50.0);
        assert!(line.starts_with("M0.0 25.0"), "{line}");
        assert!(fill.ends_with('Z'));
        // Square at depth 0 still draws a visible line around the middle.
        let (flat, _) = wave_paths(4, 0.0, 0.0, 100.0, 50.0);
        assert!(flat.starts_with("M0.0 24.1"), "{flat}");
    }

    /// Model cost of an LFO picture per refresh
    /// (`cargo test -p dstudio-app --release bench_lfo_wave -- --ignored --nocapture`).
    #[test]
    #[ignore]
    fn bench_lfo_wave() {
        let n = 5000;
        let t0 = std::time::Instant::now();
        for i in 0..n {
            std::hint::black_box(wave_paths(i % 7, 0.25, 0.8, 160.0, 128.0));
        }
        println!("lfo picture: {:.2} us per refresh", t0.elapsed().as_secs_f64() * 1e6 / n as f64);
    }
}
