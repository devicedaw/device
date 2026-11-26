//! Media made before a project's first save lives outside any project: recorded takes in
//! "Unsaved recordings" and rendered clips in "Unsaved renders" under `Controller::unsaved_root` (the
//! app's data folder, which survives a restart, so crash recovery finds them), and auditioned
//! sounds in "Sound previews" under [`temp_root`] (a cache). Save brings every such file the
//! project uses into `<project>/media/`: takes and renders are moved there, previews are
//! copied. Versions before 0.5 kept takes and renders under [`temp_root`] too.

use std::path::{Path, PathBuf};

/// The system temp folder's `Device Studio` (sound previews; takes and renders of older versions).
pub fn temp_root() -> PathBuf {
    std::env::temp_dir().join("Device Studio")
}

/// Where a normal session puts takes and renders made before the first save:
/// `<app data>/Unsaved media` (`Controller::unsaved_root`; tests keep [`temp_root`]).
pub fn data_root(support_dir: &Path) -> PathBuf {
    support_dir.join("Unsaved media")
}

/// How long an unsaved take or render nobody saved stays before [`prune`] removes it.
pub const KEEP_UNSAVED: std::time::Duration = std::time::Duration::from_secs(30 * 24 * 3600);

/// At launch: removes audio files under `root` last changed more than `keep` ago (takes and
/// renders of projects that were never saved). The system temp folder used to do this.
pub fn prune(root: &Path, keep: std::time::Duration) -> usize {
    let cutoff = std::time::SystemTime::now().checked_sub(keep);
    let mut removed = 0;
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for e in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            let p = e.path();
            let Ok(kind) = e.file_type() else { continue };
            if kind.is_dir() {
                stack.push(p);
                continue;
            }
            let audio = p.extension().is_some_and(|x| ["wav", "flac", "mp3", "ogg"].iter().any(|a| x.eq_ignore_ascii_case(a)));
            let old = e.metadata().and_then(|m| m.modified()).ok().zip(cutoff).is_some_and(|(t, c)| t < c);
            if audio && old && std::fs::remove_file(&p).is_ok() {
                removed += 1;
            }
        }
    }
    removed
}

/// Where a file at `rest` (its path under the temp root) goes in the project's media folder:
/// takes and renders keep their place under `media/`; anything else goes in `media/` by name.
fn destination(media: &Path, rest: &Path) -> Option<(PathBuf, bool)> {
    let mut parts = rest.components();
    let first = parts.next()?.as_os_str().to_str()?.to_owned();
    match first.as_str() {
        "Unsaved recordings" | "Unsaved renders" => Some((media.join(parts.as_path()), true)),
        _ => Some((media.join(rest.file_name()?), false)),
    }
}

/// `path`, or the same name with " 2", " 3", ... before the extension when it is taken.
fn free_name(path: PathBuf) -> PathBuf {
    if !path.exists() {
        return path;
    }
    let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let ext = path.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
    (2..).map(|n| path.with_file_name(format!("{stem} {n}{ext}"))).find(|p| !p.exists()).expect("a free name")
}

fn move_file(from: &Path, to: &Path) -> std::io::Result<()> {
    if std::fs::rename(from, to).is_ok() {
        return Ok(());
    }
    // Another volume (or a file still open on Windows): copy, then remove what can be removed.
    std::fs::copy(from, to)?;
    let _ = std::fs::remove_file(from);
    Ok(())
}

/// Brings the files among `paths` that lie under `root` into `project/media/`. Returns each
/// file that now lives in the project (old path, new path) and a message for each that could
/// not be brought in (it stays where it is and still plays).
pub fn adopt(root: &Path, paths: &[String], project: &Path) -> (Vec<(String, String)>, Vec<String>) {
    let roots: Vec<PathBuf> = [Some(root.to_path_buf()), std::fs::canonicalize(root).ok()].into_iter().flatten().collect();
    let media = project.join("media");
    let (mut moved, mut failed) = (Vec::new(), Vec::new());
    for path in paths {
        let p = Path::new(path);
        let Some(rest) = roots.iter().find_map(|r| p.strip_prefix(r).ok()) else { continue };
        let Some((dest, is_move)) = destination(&media, rest) else { continue };
        if !p.exists() {
            continue;
        }
        let dest = free_name(dest);
        let result = dest
            .parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .and_then(|()| if is_move { move_file(p, &dest) } else { std::fs::copy(p, &dest).map(|_| ()) });
        match result {
            Ok(()) => {
                let dest = std::fs::canonicalize(&dest).unwrap_or(dest);
                moved.push((path.clone(), dest.to_string_lossy().into_owned()));
            }
            Err(e) => failed.push(format!("{}: {e}", p.display())),
        }
    }
    (moved, failed)
}

#[cfg(test)]
mod tests {
    use super::adopt;
    use std::path::Path;

    fn write(p: &Path) {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, b"RIFF").unwrap();
    }

    #[test]
    fn save_brings_takes_renders_and_previews_into_the_project() {
        let base = std::env::temp_dir().join(format!("dstudio-unsaved-media-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let root = base.join("Device Studio");
        let take = root.join("Unsaved recordings").join("Audio 1 take 1.wav");
        let render = root.join("Unsaved renders").join("bounce").join("Synth.wav");
        let preview = root.join("Sound previews").join("0123.wav");
        let elsewhere = base.join("loops").join("drums.wav");
        for p in [&take, &render, &preview, &elsewhere] {
            write(p);
        }
        let project = base.join("Song.dsproj");
        // A take of the same name is already in the project: the new one gets " 2".
        write(&project.join("media").join("Audio 1 take 1.wav"));
        let paths: Vec<String> = [&take, &render, &preview, &elsewhere].iter().map(|p| p.to_string_lossy().into_owned()).collect();

        let (moved, failed) = adopt(&root, &paths, &project);

        assert!(failed.is_empty(), "{failed:?}");
        let media = std::fs::canonicalize(project.join("media")).unwrap();
        let to = |from: &Path| moved.iter().find(|(f, _)| Path::new(f) == from).map(|(_, t)| t.clone());
        assert_eq!(to(&take), Some(media.join("Audio 1 take 1 2.wav").to_string_lossy().into_owned()));
        assert_eq!(to(&render), Some(media.join("bounce").join("Synth.wav").to_string_lossy().into_owned()));
        assert_eq!(to(&preview), Some(media.join("0123.wav").to_string_lossy().into_owned()));
        assert_eq!(to(&elsewhere), None, "files outside the temp folder stay where they are");
        assert!(!take.exists() && !render.exists(), "takes and renders are moved");
        assert!(preview.exists(), "previews are a shared cache: copied");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn unsaved_takes_nobody_saved_for_a_month_are_removed() {
        let root = std::env::temp_dir().join(format!("dstudio-prune-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let takes = root.join("Unsaved recordings/2026-11-26 10-00-00");
        std::fs::create_dir_all(&takes).unwrap();
        let old = takes.join("Audio 1.wav");
        let new = takes.join("Audio 2.wav");
        let note = takes.join("notes.txt");
        for f in [&old, &new, &note] {
            std::fs::write(f, b"x").unwrap();
        }
        let long_ago = std::time::SystemTime::now() - std::time::Duration::from_secs(40 * 24 * 3600);
        for f in [&old, &note] {
            std::fs::File::options().write(true).open(f).unwrap().set_modified(long_ago).unwrap();
        }
        assert_eq!(super::prune(&root, super::KEEP_UNSAVED), 1);
        assert!(!old.exists(), "a take from 40 days ago goes");
        assert!(new.exists(), "today's stays");
        assert!(note.exists(), "only audio files are removed");
        let _ = std::fs::remove_dir_all(&root);
    }
}
