//! Saved and factory device chains (dstudio_core::chains).
//!
//! * Browser › Devices › Chains and Add device › Chains list the user's saved
//!   chains ("My chains") and the factory folders; picking one adds the whole
//!   chain (head, members, macros) to the selected track in one undo step.
//! * The chain card's name menu: Save, Save as…, Chains › (replace the chain
//!   with another), Rename…, Unpack chain, Delete saved chain.
//!
//! Saved chains are JSON files in the `Chains` folder next to the preferences
//! file. Runs without a preferences file (self-test, screenshots) keep saved
//! chains in memory only.

use crate::controller::Controller;
use crate::devices::{DevMenu, Item, MenuPick};
use dstudio_core::chains::{self, ChainPreset};
use dstudio_core::modulation as modu;
use dstudio_core::{Command, DeviceId, TrackId};
use std::path::PathBuf;

/// Folder label of the user's saved chains.
pub const MY_CHAINS: &str = "My chains";

/// A chain the browser or a menu can add: a saved one (`user`) or a factory one.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ChainRef {
    pub user: bool,
    pub name: String,
}

#[derive(Debug)]
pub struct ChainLib {
    pub factory: Vec<(&'static str, ChainPreset)>,
    pub user: Vec<ChainPreset>,
    /// Where saved chains live (None: memory only).
    pub dir: Option<PathBuf>,
}

impl Default for ChainLib {
    fn default() -> Self {
        ChainLib { factory: chains::factory(), user: Vec::new(), dir: None }
    }
}

impl ChainLib {
    /// Use `dir` for saved chains and read the ones already there.
    pub fn open(&mut self, dir: PathBuf) {
        self.user = read_dir(&dir);
        self.dir = Some(dir);
    }

    pub fn get(&self, r: &ChainRef) -> Option<&ChainPreset> {
        if r.user { self.user.iter().find(|p| p.name == r.name) } else { self.factory.iter().map(|(_, p)| p).find(|p| p.name == r.name) }
    }

    pub fn is_user(&self, name: &str) -> bool {
        self.user.iter().any(|p| p.name == name)
    }

    /// Folder names in browser order: "My chains" (when there are any), then the factory folders.
    pub fn folders(&self) -> Vec<&str> {
        let mut v = Vec::new();
        if !self.user.is_empty() {
            v.push(MY_CHAINS);
        }
        v.extend(chains::FOLDERS);
        v
    }

    /// Chains in `folder`, as references.
    pub fn in_folder(&self, folder: &str) -> Vec<ChainRef> {
        if folder == MY_CHAINS {
            return self.user.iter().map(|p| ChainRef { user: true, name: p.name.clone() }).collect();
        }
        self.factory.iter().filter(|(f, _)| *f == folder).map(|(_, p)| ChainRef { user: false, name: p.name.clone() }).collect()
    }

    /// Store (or replace) a saved chain; writes its file when there is a folder.
    pub fn save(&mut self, preset: ChainPreset) -> Result<(), String> {
        if let Some(dir) = &self.dir {
            std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
            let path = dir.join(chains::file_name(&preset.name));
            std::fs::write(&path, preset.to_json()).map_err(|e| format!("{}: {e}", path.display()))?;
        }
        match self.user.iter_mut().find(|p| p.name == preset.name) {
            Some(p) => *p = preset,
            None => {
                self.user.push(preset);
                self.user.sort_by_key(|p| p.name.to_lowercase());
            }
        }
        Ok(())
    }

    pub fn delete(&mut self, name: &str) -> Result<(), String> {
        if let Some(dir) = &self.dir {
            let path = dir.join(chains::file_name(name));
            match std::fs::remove_file(&path) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(format!("{}: {e}", path.display())),
            }
        }
        self.user.retain(|p| p.name != name);
        Ok(())
    }
}

fn read_dir(dir: &std::path::Path) -> Vec<ChainPreset> {
    let mut v: Vec<ChainPreset> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
        .filter_map(|e| std::fs::read_to_string(e.path()).ok())
        .filter_map(|s| ChainPreset::from_json(&s))
        .collect();
    v.sort_by_key(|p| p.name.to_lowercase());
    v.dedup_by(|a, b| a.name == b.name);
    v
}

/// Menu rows of the chains in `folder`; each row runs `pick`.
pub(crate) fn chain_rows(
    lib: &ChainLib,
    folder: &str,
    pick: impl Fn(ChainRef) -> MenuPick,
) -> Vec<(String, String, Option<MenuPick>, bool)> {
    lib.in_folder(folder).into_iter().map(|r| (r.name.clone(), String::new(), Some(pick(r)), false)).collect()
}

impl Controller {
    fn chain_menu_at(&mut self, rows: Vec<(String, String, Option<MenuPick>, bool)>, x: f32, y: f32) {
        self.devices.menu = Some(DevMenu { pos: (x, y), w: 220.0, rows });
        self.ui_dirty = true;
    }

    /// Add device › Chains (and the chain card's Chains ›): the folders, an empty chain first
    /// when adding. `replace` names the chain head the pick replaces.
    pub(crate) fn chain_folders_menu(&mut self, replace: Option<DeviceId>, x: f32, y: f32) {
        let mut rows = Vec::new();
        if replace.is_none() {
            rows.push((
                "Empty chain".to_string(),
                "Chain".to_string(),
                Some(MenuPick::Add(Item::Builtin(&dstudio_core::builtin::rack::RACK))),
                false,
            ));
        }
        for (i, f) in self.devices.chains.folders().into_iter().enumerate() {
            rows.push((f.to_string(), "›".to_string(), Some(MenuPick::ChainFolder(replace, f.to_string())), i == 0 && replace.is_none()));
        }
        self.chain_menu_at(rows, x, y);
    }

    /// One folder's chains, after a "‹ Chains" row that goes back to the folders.
    pub(crate) fn chain_folder_menu(&mut self, replace: Option<DeviceId>, folder: &str, x: f32, y: f32) {
        let mut rows = vec![("‹ Chains".to_string(), String::new(), Some(MenuPick::ChainFolders(replace)), false)];
        rows.push((folder.to_string(), String::new(), None, true));
        let pick = |r: ChainRef| match replace {
            Some(head) => MenuPick::ChainReplace(head, r),
            None => MenuPick::Add(Item::Chain(r)),
        };
        rows.extend(chain_rows(&self.devices.chains, folder, pick));
        self.chain_menu_at(rows, x, y);
    }

    /// The chain card's name menu.
    pub(crate) fn chain_card_menu(&mut self, head: DeviceId, x: f32, y: f32) {
        let Some(d) = self.project().device(head) else { return };
        let saved = d.preset.as_deref().filter(|n| self.devices.chains.is_user(n)).is_some();
        let on = |p: MenuPick, enabled: bool| enabled.then_some(p);
        let rows = vec![
            ("Save".to_string(), String::new(), on(MenuPick::ChainSave(head), saved), false),
            ("Save as…".to_string(), String::new(), Some(MenuPick::ChainSaveAs(head)), false),
            ("Chains".to_string(), "›".to_string(), Some(MenuPick::ChainFolders(Some(head))), false),
            ("Rename…".to_string(), String::new(), Some(MenuPick::ChainRename(head)), true),
            ("Unpack chain".to_string(), String::new(), Some(MenuPick::ChainUnpack(head)), false),
            ("Delete saved chain".to_string(), String::new(), on(MenuPick::ChainDelete(head), saved), true),
        ];
        self.chain_menu_at(rows, x, y);
    }

    /// Menu pick dispatch for the chain rows (src/devices.rs devmenu_pick).
    pub(crate) fn chain_pick(&mut self, pick: MenuPick, at: (f32, f32)) {
        match pick {
            MenuPick::ChainFolders(replace) => self.chain_folders_menu(replace, at.0, at.1),
            MenuPick::ChainFolder(replace, folder) => self.chain_folder_menu(replace, &folder, at.0, at.1),
            MenuPick::ChainReplace(head, r) => self.chain_replace(head, &r),
            MenuPick::ChainSave(head) => {
                let name = self.project().device(head).and_then(|d| d.preset.clone());
                if let Some(name) = name {
                    self.chain_save(head, &name);
                }
            }
            MenuPick::ChainSaveAs(head) => {
                self.chain_save_as = Some(head);
                self.devices.renaming = Some(head);
                self.ui_dirty = true;
            }
            MenuPick::ChainRename(head) => {
                self.chain_save_as = None;
                self.devices.renaming = Some(head);
                self.ui_dirty = true;
            }
            MenuPick::ChainUnpack(head) => self.chain_unpack(head),
            MenuPick::ChainDelete(head) => self.chain_delete_saved(head),
            _ => {}
        }
    }

    /// Save the chain headed by `head` as `name` (replacing a saved chain of that name). The
    /// card then shows that name.
    pub(crate) fn chain_save(&mut self, head: DeviceId, name: &str) -> bool {
        let name = name.trim();
        if name.is_empty() {
            return false;
        }
        let Some(t) = self.track_of(head) else { return false };
        let Some(preset) = ChainPreset::from_track(t, head, name) else { return false };
        if let Err(e) = self.devices.chains.save(preset) {
            self.set_status(format!("Couldn't save the chain: {e}"), 4);
            return false;
        }
        let Some(d) = self.project().device(head) else { return false };
        if d.preset.as_deref() != Some(name) || d.label.as_deref() != Some(name) {
            let commands = vec![
                Command::SetDeviceLabel { device: head, label: Some(name.to_string()) },
                Command::SetDeviceParams { device: head, params: d.params.clone(), preset: Some(name.to_string()) },
            ];
            self.execute(Command::Batch { label: "Save Chain".into(), commands });
        }
        self.set_status(format!("Saved chain \"{name}\""), 3);
        self.ui_dirty = true;
        true
    }

    fn chain_delete_saved(&mut self, head: DeviceId) {
        let Some(name) = self.project().device(head).and_then(|d| d.preset.clone()) else { return };
        match self.devices.chains.delete(&name) {
            Ok(()) => self.set_status(format!("Deleted saved chain \"{name}\""), 3),
            Err(e) => self.set_status(format!("Couldn't delete the chain: {e}"), 4),
        }
        self.ui_dirty = true;
    }

    /// The devices of chain `r`, with ids from the project's next id.
    fn chain_devices(&self, r: &ChainRef) -> Option<Vec<dstudio_core::Device>> {
        let p = self.devices.chains.get(r)?;
        Some(p.instantiate(self.project().next_id))
    }

    /// Add chain `r` to `track` at `index` (end when None), one undo step.
    pub(crate) fn add_chain(&mut self, r: &ChainRef, track: TrackId, index: Option<usize>) -> bool {
        let Some(devs) = self.chain_devices(r) else { return false };
        let Some(t) = self.project().track(track) else { return false };
        // Never inside another chain: past the run when the slot is a member.
        let mut at = index.unwrap_or(t.devices.len()).min(t.devices.len());
        while at < t.devices.len() && at > 0 && modu::head_of(&t.devices, at).is_some() {
            at += 1;
        }
        let commands = devs.into_iter().enumerate().map(|(i, device)| Command::AddDevice { track, index: Some(at + i), device }).collect();
        self.execute(Command::Batch { label: format!("Add Chain \"{}\"", r.name), commands })
    }

    /// Swap the chain headed by `head` for chain `r`, in place, one undo step.
    fn chain_replace(&mut self, head: DeviceId, r: &ChainRef) {
        let Some(t) = self.track_of(head) else { return };
        let track = t.id;
        let Some(hi) = t.devices.iter().position(|d| d.id == head) else { return };
        let run = modu::members(&t.devices, hi);
        let old: Vec<DeviceId> = std::iter::once(hi).chain(run).rev().map(|i| t.devices[i].id).collect();
        let Some(devs) = self.chain_devices(r) else { return };
        for d in &old {
            self.engine.close_plugin_editor(*d);
            self.devices.editors.remove(d);
            self.devices.windows.remove(d);
        }
        let mut commands: Vec<Command> = old.into_iter().map(|device| Command::RemoveDevice { device }).collect();
        commands.extend(devs.into_iter().enumerate().map(|(i, device)| Command::AddDevice { track, index: Some(hi + i), device }));
        self.execute(Command::Batch { label: format!("Load Chain \"{}\"", r.name), commands });
    }

    /// Unpack: the members stay where they are as ordinary effects, the head (and its macros)
    /// goes. One undo step.
    fn chain_unpack(&mut self, head: DeviceId) {
        let Some(t) = self.track_of(head) else { return };
        if let Some(cmd) = dstudio_core::effect_edit::unpack(self.project(), t.id, &[head]) {
            self.execute(cmd);
        }
    }

    /// A rename field closed: "Save as…" saves under the typed name, a rename relabels.
    pub(crate) fn chain_rename_done(&mut self, head: DeviceId, text: &str) {
        self.devices.renaming = None;
        if self.chain_save_as.take() == Some(head) {
            self.chain_save(head, text);
        } else {
            self.device_rename(head, text);
        }
        self.ui_dirty = true;
    }

    pub(crate) fn chain_rename_cancel(&mut self) {
        self.devices.renaming = None;
        self.chain_save_as = None;
        self.ui_dirty = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dstudio_core::PluginKind;

    #[test]
    fn library_saves_replaces_and_deletes_files() {
        let dir = std::env::temp_dir().join(format!("dstudio-chains-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut lib = ChainLib::default();
        lib.open(dir.clone());
        assert!(lib.user.is_empty());
        assert_eq!(lib.folders()[0], "Vocals", "no My chains folder while empty");
        let mut p = lib.factory[0].1.clone();
        p.name = "Mine".into();
        lib.save(p.clone()).unwrap();
        lib.save(p.clone()).unwrap();
        assert_eq!(lib.user.len(), 1, "same name replaces");
        assert_eq!(lib.folders()[0], MY_CHAINS);
        // A fresh library reads the file back.
        let mut again = ChainLib::default();
        again.open(dir.clone());
        assert_eq!(again.in_folder(MY_CHAINS), vec![ChainRef { user: true, name: "Mine".into() }]);
        assert!(again.get(&ChainRef { user: true, name: "Mine".into() }).is_some());
        again.delete("Mine").unwrap();
        assert!(again.user.is_empty());
        let mut third = ChainLib::default();
        third.open(dir.clone());
        assert!(third.user.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn every_factory_folder_has_chains() {
        let lib = ChainLib::default();
        for f in chains::FOLDERS {
            assert!(!lib.in_folder(f).is_empty(), "{f}");
        }
        let r = &lib.in_folder("Drums")[0];
        assert!(lib.get(r).is_some_and(|p| p.devices.len() >= 2));
        assert_eq!(Item::Chain(r.clone()).kind(), PluginKind::Effect);
    }
}
