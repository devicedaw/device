//! Media made before a project's first save lives in the system temp folder
//! (`<temp>/Device Studio/`: recorded takes in "Unsaved recordings", rendered clips in
//! "Unsaved renders", auditioned sounds in "Sound previews"). The system may delete that
//! folder, so Save brings every such file the project uses into `<project>/media/`: takes
//! and renders are moved there, previews (a shared cache) are copied.

use std::path::{Path, PathBuf};

/// The folder unsaved media is written under: `<temp>/Device Studio`.
pub fn temp_root() -> PathBuf {
    std::env::temp_dir().join("Device Studio")
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
        let result = dest.parent().map_or(Ok(()), std::fs::create_dir_all).and_then(|()| {
            if is_move { move_file(p, &dest) } else { std::fs::copy(p, &dest).map(|_| ()) }
        });
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
}
