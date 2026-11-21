//! Selected effects in the device chain: a press on an effect card's header selects it (Shift or
//! Cmd adds it to the selection or takes it out), and the FX slot keys act on the selection:
//! Duplicate selected effects (⌘D), Create chain from selected effects (⌘G), Unpack selected
//! chain (⌘⇧G). Selecting a clip, a section or a range, or pressing Escape, clears it.

use crate::controller::Controller;
use dstudio_core::effect_edit;
use dstudio_core::{DeviceId, TrackId};

impl Controller {
    /// The selected track's effects that are selected, in chain order.
    pub fn selected_effects(&self) -> Vec<DeviceId> {
        let Some(t) = self.selected_track.and_then(|t| self.project().track(t)) else { return Vec::new() };
        t.devices.iter().map(|d| d.id).filter(|id| self.devices.fx_sel.contains(id)).collect()
    }

    /// Whether the FX slot keys apply: effects are selected and no clip, section or range is.
    pub fn fx_slot_selected(&self) -> bool {
        !self.selected_effects().is_empty()
            && self.selected_clips().is_empty()
            && self.selected_section.is_none()
            && self.tools.range.is_none()
    }

    /// Press on card `id`'s header: select it alone, or with `extend` add it to the selection
    /// (or take it out). A press on a card that is already selected keeps the selection.
    pub fn effect_select(&mut self, id: DeviceId, extend: bool) {
        let insert = self.project().find_device(id).is_some_and(|(_, s)| matches!(s, dstudio_core::model::DeviceSlot::Insert(_)));
        if !insert {
            return;
        }
        let mut sel = self.selected_effects();
        if extend {
            if let Some(i) = sel.iter().position(|d| *d == id) {
                sel.remove(i);
            } else {
                sel.push(id);
            }
        } else if !sel.contains(&id) {
            sel = vec![id];
        }
        self.devices.fx_sel = sel;
        self.select_clip(None);
        self.selected_section = None;
        self.range_clear();
        self.ui_dirty = true;
    }

    pub fn effect_select_clear(&mut self) -> bool {
        let had = !self.devices.fx_sel.is_empty();
        self.devices.fx_sel.clear();
        self.ui_dirty |= had;
        had
    }

    fn fx_track(&self) -> Option<TrackId> {
        self.selected_track.filter(|_| self.fx_slot_selected())
    }

    /// ⌘D: copies of the selected effects right after them; the copies end up selected.
    pub fn duplicate_selected_effects(&mut self) {
        let Some(t) = self.fx_track() else { return };
        let Some(cmd) = effect_edit::duplicate(self.project(), t, &self.selected_effects()) else { return };
        let before = self.device_ids(t);
        if self.execute(cmd) {
            self.devices.fx_sel = self.device_ids(t).into_iter().filter(|d| !before.contains(d)).collect();
        }
    }

    /// ⌘G: a new chain holding the selected effects; the chain ends up selected.
    pub fn group_selected_effects(&mut self) {
        let Some(t) = self.fx_track() else { return };
        let Some(cmd) = effect_edit::group(self.project(), t, &self.selected_effects()) else {
            self.set_status("Only effects outside a chain go in a new chain", 3);
            return;
        };
        let head = DeviceId(self.project().next_id);
        if self.execute(cmd) {
            self.devices.fx_sel = vec![head];
        }
    }

    /// ⌘⇧G: unpack the selected chains (or the chains of selected members); their effects end
    /// up selected.
    pub fn unpack_selected_chain(&mut self) {
        let Some(t) = self.fx_track() else { return };
        let Some(cmd) = effect_edit::unpack(self.project(), t, &self.selected_effects()) else { return };
        let members: Vec<DeviceId> = {
            let devs = self.project().track(t).map(|t| t.devices.as_slice()).unwrap_or_default();
            let heads = effect_edit::selected_heads(devs, &self.selected_effects());
            devs.iter().filter(|d| d.chain.is_some_and(|h| heads.contains(&h))).map(|d| d.id).collect()
        };
        if self.execute(cmd) {
            self.devices.fx_sel = members;
        }
    }

    fn device_ids(&self, t: TrackId) -> Vec<DeviceId> {
        self.project().track(t).map(|t| t.devices.iter().map(|d| d.id).collect()).unwrap_or_default()
    }
}
