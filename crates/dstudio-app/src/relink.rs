//! Relink missing media (roadmap S0.4).
//!
//! Opening a project whose audio files moved shows the Missing media dialog: one row per
//! missing file. Locate… asks for a folder and searches it (and the folders inside it) on a
//! worker thread for files with the same name. A candidate counts only when it has the same
//! length as the original, measured at the file's own sample rate (the project stores it that
//! way), so a different take that happens to share a name is not picked. A file in a folder of
//! the same name as before is tried first, and no file is given to two assets. Everything found
//! is relinked in ONE undo step (`Command::RelinkAssets`), and the rows still missing stay in
//! the dialog for another folder. File › Relink Missing Media… opens the dialog again after
//! Skip.
//!
//! Undo, redo and any other change of an asset's path reload that asset
//! ([`Controller::reload_moved_assets`]).

use crate::controller::{Controller, JobResult, load_and_analyse};
use crate::peaks::Pyramid;
use dstudio_core::{AssetId, Command};
use dstudio_engine::{AssetInfo, Engine};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// How many entries a search looks at before it gives up (a whole disk picked by mistake).
pub const MAX_ENTRIES: usize = 200_000;
/// How deep a search goes below the folder picked.
pub const MAX_DEPTH: usize = 8;

#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub asset: AssetId,
    /// The file name the project expects.
    pub name: String,
    /// Where it was, for the row's second line.
    pub was: String,
}

#[derive(Debug, Default)]
pub struct Relink {
    pub open: bool,
    /// The files still missing.
    pub rows: Vec<Row>,
    /// A search is running.
    pub searching: bool,
    /// What the last search did, for the dialog's message.
    pub note: String,
}

/// What a search found: (asset, file, its engine asset and peaks) per relinked file, and how
/// many files it looked at.
pub struct Found {
    pub files: Vec<(AssetId, PathBuf, AssetInfo, Pyramid)>,
    pub looked_at: usize,
}

/// Every file under `folder` (to [`MAX_DEPTH`] folders deep, at most `max_entries` entries)
/// whose name is one of `names`, compared without case: name → files, in a stable order.
pub fn candidates(folder: &Path, names: &[String], max_entries: usize) -> (HashMap<String, Vec<PathBuf>>, usize) {
    let wanted: std::collections::HashSet<String> = names.iter().map(|n| n.to_lowercase()).collect();
    let mut out: HashMap<String, Vec<PathBuf>> = HashMap::new();
    let mut stack = vec![(folder.to_path_buf(), 0usize)];
    let mut seen = 0usize;
    while let Some((dir, depth)) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        let mut entries: Vec<_> = entries.flatten().collect();
        entries.sort_by_key(|e| e.file_name());
        for e in entries {
            seen += 1;
            if seen > max_entries {
                return (out, seen - 1);
            }
            let Ok(kind) = e.file_type() else { continue };
            let name = e.file_name().to_string_lossy().into_owned();
            if kind.is_dir() {
                // Hidden folders (.git, .Trash) and project backups hold nothing to relink to.
                if depth < MAX_DEPTH && !name.starts_with('.') {
                    stack.push((e.path(), depth + 1));
                }
            } else if wanted.contains(&name.to_lowercase()) {
                out.entry(name.to_lowercase()).or_default().push(e.path());
            }
        }
    }
    for v in out.values_mut() {
        v.sort();
    }
    (out, seen)
}

/// A missing file the search looks for.
#[derive(Debug, Clone, PartialEq)]
pub struct Wanted {
    pub asset: AssetId,
    /// File name.
    pub name: String,
    /// Name of the folder it was in (tried first).
    pub folder: String,
    /// Length in frames at the file's own sample rate, as `Asset::frames` stores it.
    pub frames: u64,
}

/// `info` (decoded by an engine running at `engine_rate`) has `frames` frames at its own
/// sample rate. Computed as an import computes `Asset::frames`; one frame either way is allowed
/// for rounding.
pub fn same_length(info: &AssetInfo, engine_rate: f64, frames: u64) -> bool {
    let native = if info.source_sample_rate > 0.0 && engine_rate > 0.0 {
        (info.frames as f64 * info.source_sample_rate / engine_rate).round().max(1.0) as u64
    } else {
        info.frames
    };
    native.abs_diff(frames) <= 1
}

/// The order to try a missing file's candidates in: those in a folder named like the one it was
/// in first, then the rest (each group keeps its stable order).
pub fn by_preference<'a>(paths: &'a [PathBuf], folder: &str) -> Vec<&'a PathBuf> {
    let same = |p: &PathBuf| p.parent().and_then(|d| d.file_name()).is_some_and(|n| n.to_string_lossy().eq_ignore_ascii_case(folder));
    paths.iter().filter(|p| same(p)).chain(paths.iter().filter(|p| !same(p))).collect()
}

/// Searches `folder` for each missing file: the first file of that name with the same length,
/// never one already given to another asset. Runs on a worker thread.
pub fn search(engine: &Engine, folder: &Path, missing: &[Wanted]) -> Found {
    let names: Vec<String> = missing.iter().map(|w| w.name.clone()).collect();
    let (by_name, looked_at) = candidates(folder, &names, MAX_ENTRIES);
    let rate = engine.sample_rate();
    let mut used = std::collections::HashSet::new();
    let mut files = Vec::new();
    for w in missing {
        let paths = by_name.get(&w.name.to_lowercase()).map(Vec::as_slice).unwrap_or(&[]);
        for path in by_preference(paths, &w.folder) {
            if used.contains(path) {
                continue;
            }
            match load_and_analyse(engine, path) {
                Ok((info, pyr)) if same_length(&info, rate, w.frames) => {
                    used.insert(path.clone());
                    let asset = w.asset;
                    files.push((asset, path.clone(), info, pyr));
                    break;
                }
                Ok((info, _)) => {
                    let _ = engine.release_asset(info.asset_id);
                }
                Err(_) => {}
            }
        }
    }
    Found { files, looked_at }
}

impl Controller {
    /// After Open: the dialog lists the media files that are not where the project says.
    pub fn relink_show(&mut self, missing: &[String]) {
        let rows: Vec<Row> = self
            .project()
            .assets
            .iter()
            .filter(|a| missing.contains(&a.absolute_path))
            .map(|a| Row {
                asset: a.id,
                name: Path::new(&a.absolute_path).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
                was: Path::new(&a.absolute_path).parent().map(|p| p.display().to_string()).unwrap_or_default(),
            })
            .collect();
        if rows.is_empty() {
            return;
        }
        self.relink = Relink { open: true, rows, searching: false, note: String::new() };
        self.ui_dirty = true;
    }

    /// Locate… (the folder picked): searches it on a worker thread.
    pub fn relink_locate(&mut self, folder: PathBuf) {
        if self.relink.searching || self.relink.rows.is_empty() {
            return;
        }
        let missing: Vec<Wanted> = self
            .relink
            .rows
            .iter()
            .filter_map(|r| {
                let a = self.project().asset(r.asset)?;
                let folder = Path::new(&r.was).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                Some(Wanted { asset: r.asset, name: r.name.clone(), folder, frames: a.frames })
            })
            .collect();
        let engine: Arc<Engine> = self.engine.clone();
        let tx = self.jobs_tx.clone();
        let epoch = self.doc_epoch;
        self.relink.searching = true;
        self.relink.note = format!("Searching {}…", folder.display());
        self.pending_jobs += 1;
        std::thread::spawn(move || {
            let found = search(&engine, &folder, &missing);
            let _ = tx.send(JobResult::Relinked { doc_epoch: epoch, folder, found });
        });
        self.ui_dirty = true;
    }

    /// The search finished: relink what it found in one undo step.
    pub(crate) fn finish_relink(&mut self, doc_epoch: u64, folder: &Path, found: Found) {
        self.relink.searching = false;
        if doc_epoch != self.doc_epoch {
            for (_, _, info, _) in found.files {
                let _ = self.engine.release_asset(info.asset_id);
            }
            return;
        }
        let paths: Vec<(AssetId, String)> = found.files.iter().map(|(a, p, _, _)| (*a, p.to_string_lossy().into_owned())).collect();
        if !paths.is_empty() {
            // Noted first, so the change does not send these assets to be loaded again.
            let before: Vec<_> = paths.iter().map(|(a, p)| (*a, self.asset_paths.insert(*a, p.clone()))).collect();
            if !self.execute(Command::RelinkAssets { paths: paths.clone() }) {
                for (a, old) in before {
                    match old {
                        Some(p) => self.asset_paths.insert(a, p),
                        None => self.asset_paths.remove(&a),
                    };
                }
                for (_, _, info, _) in found.files {
                    let _ = self.engine.release_asset(info.asset_id);
                }
                self.relink.note = "Couldn't relink the files found.".into();
                self.ui_dirty = true;
                return;
            }
            for (asset, _, info, pyr) in found.files {
                if let Some(old) = self.asset_map.insert(asset, info.asset_id) {
                    let _ = self.engine.release_asset(old);
                }
                self.peaks.insert(info.asset_id, pyr);
            }
            self.publish();
        }
        let n = paths.len();
        self.relink.rows.retain(|r| !paths.iter().any(|(a, _)| *a == r.asset));
        let left = self.relink.rows.len();
        let where_ = folder.display();
        self.relink.note = match (n, left) {
            (0, _) => format!("Nothing matching in {where_} ({} files looked at).", found.looked_at),
            (_, 0) => String::new(),
            (n, left) => format!("Found {n} in {where_}. Locate the other {left} in another folder, or skip them."),
        };
        if left == 0 {
            self.relink.open = false;
            self.set_status(format!("Relinked {n} media file{}", if n == 1 { "" } else { "s" }), 4);
        }
        self.ui_dirty = true;
    }

    /// File › Relink Missing Media…: the dialog again, for the files that are still missing
    /// (after Skip, or files moved while the project was open).
    pub fn relink_reopen(&mut self) {
        if self.relink.open {
            return;
        }
        let missing: Vec<String> =
            self.project().assets.iter().filter(|a| !Path::new(&a.absolute_path).exists()).map(|a| a.absolute_path.clone()).collect();
        if missing.is_empty() {
            self.set_status("No media files are missing", 3);
            self.ui_dirty = true;
            return;
        }
        self.relink_show(&missing);
    }

    /// Skip: leave the rest missing (their clips stay silent).
    pub fn relink_close(&mut self) {
        if self.relink.searching {
            return;
        }
        let left = self.relink.rows.len();
        self.relink = Relink::default();
        if left > 0 {
            self.set_status(format!("{left} media file{} missing", if left == 1 { "" } else { "s" }), 6);
        }
        self.ui_dirty = true;
    }

    /// After any change: an asset whose path changed (Relink Media, or its undo and redo) is
    /// loaded again from where it now points.
    pub(crate) fn reload_moved_assets(&mut self) {
        let moved: Vec<dstudio_core::Asset> =
            self.project().assets.iter().filter(|a| self.asset_paths.get(&a.id).is_some_and(|p| *p != a.absolute_path)).cloned().collect();
        for a in moved {
            if let Some(old) = self.asset_map.remove(&a.id) {
                let _ = self.engine.release_asset(old);
            }
            if Path::new(&a.absolute_path).exists() {
                self.load_asset(&a);
            } else {
                self.asset_paths.insert(a.id, a.absolute_path.clone());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::candidates;

    #[test]
    fn a_search_finds_files_by_name_in_folders_below() {
        let root = std::env::temp_dir().join(format!("dstudio-relink-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("Samples/Drums")).unwrap();
        std::fs::create_dir_all(root.join(".Trash")).unwrap();
        std::fs::write(root.join("Samples/Drums/Kick.WAV"), b"x").unwrap();
        std::fs::write(root.join("Samples/kick.wav"), b"x").unwrap();
        std::fs::write(root.join(".Trash/Snare.wav"), b"x").unwrap();
        std::fs::write(root.join("Bass.wav"), b"x").unwrap();
        let names = ["kick.wav".to_string(), "Snare.wav".to_string()];
        let (found, seen) = candidates(&root, &names, 1000);
        let kicks = &found["kick.wav"];
        assert_eq!(kicks.len(), 2, "any case, any depth: {kicks:?}");
        assert!(!found.contains_key("snare.wav"), "hidden folders are not searched");
        assert!(!found.contains_key("bass.wav"), "only the names asked for");
        assert!(seen >= 5);
        let (_, capped) = candidates(&root, &names, 2);
        assert_eq!(capped, 2, "the search stops at its limit");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn locate_relinks_the_file_of_the_same_length_in_one_undo_step() {
        use crate::controller::Controller;
        use dstudio_core::{Asset, AssetId, Command, Origin, Project};
        use std::sync::Arc;
        let root = std::env::temp_dir().join(format!("dstudio-relink-e2e-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("moved/a-older")).unwrap();
        std::fs::create_dir_all(root.join("moved/b")).unwrap();
        // A file of the same name but another length comes first in the search; it is not picked.
        crate::wavgen::write_wav(&root.join("moved/a-older/Vox.wav"), 48_000, 1, &[0.1; 100]).unwrap();
        let real = root.join("moved/b/Vox.wav");
        crate::wavgen::write_wav(&real, 48_000, 1, &[0.2; 4_800]).unwrap();
        let gone = root.join("was here/Vox.wav").to_string_lossy().into_owned();
        let mut doc = dstudio_core::Document::new(Project::default());
        let asset =
            Asset { id: AssetId(0), relative_path: None, absolute_path: gone.clone(), sample_rate: 48_000.0, channels: 1, frames: 4_800 };
        doc.execute(Command::AddAsset { asset }, Origin::User).unwrap();
        let id = doc.project().assets[0].id;
        let song = root.join("Song.dsproj");
        dstudio_core::persist::save(doc.project(), &song).unwrap();

        let engine = Arc::new(dstudio_engine::Engine::new(dstudio_engine::Config::default()).unwrap());
        let mut c = Controller::new(engine, Project::default());
        assert!(c.open(&song));
        assert!(c.relink.open, "the dialog opens with the missing file");
        assert_eq!(c.relink.rows.len(), 1);
        assert_eq!(c.relink.rows[0].name, "Vox.wav");
        c.relink_locate(root.join("moved"));
        let t = std::time::Instant::now();
        while c.relink.searching && t.elapsed() < std::time::Duration::from_secs(20) {
            c.poll_jobs();
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let now = c.project().asset(id).unwrap().absolute_path.clone();
        assert_eq!(std::path::Path::new(&now), real.as_path(), "the file of the right length");
        assert!(!c.relink.open, "nothing left missing: the dialog closes");
        assert!(c.asset_map.contains_key(&id), "the clip plays from the new file");
        assert_eq!(c.doc.undo_label(), Some("Relink Media"));
        c.undo();
        assert_eq!(c.project().asset(id).unwrap().absolute_path, gone);
        assert!(!c.asset_map.contains_key(&id), "undo puts it back to missing");
        c.redo();
        let t = std::time::Instant::now();
        while !c.asset_map.contains_key(&id) && t.elapsed() < std::time::Duration::from_secs(20) {
            c.poll_jobs();
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(c.asset_map.contains_key(&id), "redo loads the new file again");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Opens a saved project whose `files` (path, sample rate, frames) are all missing.
    fn open_missing(root: &std::path::Path, files: &[(&str, f64, u64)]) -> (crate::controller::Controller, Vec<dstudio_core::AssetId>) {
        use dstudio_core::{Asset, AssetId, Command, Origin, Project};
        let mut doc = dstudio_core::Document::new(Project::default());
        for (path, rate, frames) in files {
            let absolute_path = root.join(path).to_string_lossy().into_owned();
            let asset = Asset { id: AssetId(0), relative_path: None, absolute_path, sample_rate: *rate, channels: 1, frames: *frames };
            doc.execute(Command::AddAsset { asset }, Origin::User).unwrap();
        }
        let ids = doc.project().assets.iter().map(|a| a.id).collect();
        let song = root.join("Song.dsproj");
        dstudio_core::persist::save(doc.project(), &song).unwrap();
        let engine = std::sync::Arc::new(dstudio_engine::Engine::new(dstudio_engine::Config::default()).unwrap());
        let mut c = crate::controller::Controller::new(engine, Project::default());
        assert!(c.open(&song));
        (c, ids)
    }

    #[test]
    fn samples_at_44_1_khz_are_found_and_no_file_is_given_twice() {
        let root = std::env::temp_dir().join(format!("dstudio-relink-44k-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        // Two kits with a snare of the same name and length (a pack's one-shots often are).
        for kit in ["Kit A", "Kit B"] {
            std::fs::create_dir_all(root.join("moved").join(kit)).unwrap();
            crate::wavgen::write_wav(&root.join("moved").join(kit).join("Snare.wav"), 44_100, 1, &[0.3; 4_410]).unwrap();
        }
        let (mut c, ids) = open_missing(&root, &[("was/Kit B/Snare.wav", 44_100.0, 4_410), ("was/Kit A/Snare.wav", 44_100.0, 4_410)]);
        assert_eq!(c.relink.rows.len(), 2);
        c.relink_locate(root.join("moved"));
        let t = std::time::Instant::now();
        while c.relink.searching && t.elapsed() < std::time::Duration::from_secs(20) {
            c.poll_jobs();
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(c.relink.rows.is_empty(), "both found: {}", c.relink.note);
        let at = |i: usize| c.project().asset(ids[i]).unwrap().absolute_path.clone();
        assert_eq!(std::path::Path::new(&at(0)), root.join("moved/Kit B/Snare.wav"), "the snare from the kit it was in");
        assert_eq!(std::path::Path::new(&at(1)), root.join("moved/Kit A/Snare.wav"), "and not the same file twice");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_dialog_opens_again_from_the_file_menu_after_skip() {
        let root = std::env::temp_dir().join(format!("dstudio-relink-again-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let (mut c, _) = open_missing(&root, &[("gone/Bass.wav", 48_000.0, 100)]);
        c.relink_close();
        assert!(!c.relink.open);
        c.menu_action("file.relink_missing");
        assert!(c.relink.open, "File › Relink Missing Media… lists it again");
        assert_eq!(c.relink.rows.len(), 1);
        let _ = std::fs::remove_dir_all(&root);
    }
}
