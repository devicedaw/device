//! Compatibility: projects saved by released versions keep opening.
//!
//! `fixtures/compat/<version>/` holds a project saved by that release's own code (see the README
//! there) and `expected.json`, what the generator put into it. Every fixture must load with the
//! current code, keep its content, and survive a save and reload unchanged.

use dstudio_core::*;
use serde_json::Value;
use std::path::{Path, PathBuf};

const VERSIONS: [&str; 4] = ["v0.1.0", "v0.2.0", "v0.3.1", "v0.4.0"];

fn fixture(version: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/compat").join(version)
}

/// Copies a fixture folder into a temp dir (tests must not write into the checkout).
fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for e in std::fs::read_dir(from).unwrap() {
        let e = e.unwrap();
        let target = to.join(e.file_name());
        if e.file_type().unwrap().is_dir() {
            copy_dir(&e.path(), &target);
        } else {
            std::fs::copy(e.path(), target).unwrap();
        }
    }
}

fn expected(version: &str) -> Value {
    serde_json::from_slice(&std::fs::read(fixture(version).join("expected.json")).unwrap()).unwrap()
}

#[test]
fn every_release_has_a_fixture() {
    for v in VERSIONS {
        assert!(fixture(v).join("Compat.dsproj").join(persist::PROJECT_FILE).is_file(), "{v}");
    }
    // A new fixture directory has to be listed here, so it is tested.
    let found = std::fs::read_dir(fixture("")).unwrap().filter_map(|e| e.ok()).filter(|e| e.path().join("expected.json").is_file()).count();
    assert_eq!(found, VERSIONS.len(), "add the new fixture to VERSIONS");
}

#[test]
fn fixtures_load_and_keep_their_content() {
    for v in VERSIONS {
        let exp = expected(v);
        let loaded = persist::load(&fixture(v).join("Compat.dsproj")).unwrap_or_else(|e| panic!("{v}: {e}"));
        assert!(loaded.missing_media.is_empty(), "{v}: media not found: {:?}", loaded.missing_media);
        let p = &loaded.project;

        assert_eq!(p.transport.bpm, exp["bpm"].as_f64().unwrap(), "{v} tempo");
        let ts = (p.transport.time_signature.numerator, p.transport.time_signature.denominator);
        assert_eq!(ts, (3, 4), "{v} time signature");
        assert_eq!(p.transport.tempo.len() as u64, exp["tempo_points"].as_u64().unwrap(), "{v} tempo changes");
        let sections: Vec<&str> = p.sections.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(sections, ["Intro", "Verse"], "{v} sections");
        assert_eq!(p.assets.len(), 1, "{v} assets");

        let tracks = exp["tracks"].as_array().unwrap();
        assert_eq!(p.tracks.len(), tracks.len(), "{v} track count");
        for (t, want) in p.tracks.iter().zip(tracks) {
            let name = want["name"].as_str().unwrap();
            assert_eq!(t.name, name, "{v}");
            assert_eq!(format!("{:?}", t.kind), want["kind"].as_str().unwrap(), "{v} {name} kind");
            let starts: Vec<i64> = t.clips.iter().map(|c| c.start).collect();
            assert_eq!(serde_json::json!(starts), want["clips"], "{v} {name} clip positions");
            let midi: Vec<Value> = t.midi_clips.iter().map(|c| serde_json::json!({"start": c.start, "notes": c.notes.len()})).collect();
            assert_eq!(serde_json::json!(midi), want["midi_clips"], "{v} {name} MIDI clips");
            let effects: Vec<&str> = t.devices.iter().map(|d| d.plugin.id.as_str()).collect();
            assert_eq!(serde_json::json!(effects), want["effects"], "{v} {name} effects");
            assert_eq!(t.automation.len() as u64, want["automation_lanes"].as_u64().unwrap(), "{v} {name} automation");
            assert_eq!(t.sends.len() as u64, want["sends"].as_u64().unwrap(), "{v} {name} sends");
        }

        // Fades, routing, and built-in parameter values by id.
        let drums = p.tracks.iter().find(|t| t.name == "Drums").unwrap();
        let fades = (drums.clips[0].fades.fade_in, drums.clips[0].fades.fade_out);
        assert_eq!([fades.0, fades.1], [exp["fades"][0].as_i64().unwrap(), exp["fades"][1].as_i64().unwrap()], "{v} fades");
        let bus = p.tracks.iter().find(|t| t.name == "Bus").unwrap();
        assert_eq!(drums.output, Some(bus.id), "{v} group routing");
        let space = p.tracks.iter().find(|t| t.name == "Space").unwrap();
        assert_eq!(drums.sends[0].target, space.id, "{v} send target");

        for (track, devices) in exp["params"].as_object().unwrap() {
            let t = p.tracks.iter().find(|t| &t.name == track).unwrap();
            for (device, params) in devices.as_object().unwrap() {
                let d = t
                    .devices
                    .iter()
                    .chain(t.instrument_plugin.as_ref())
                    .find(|d| &d.plugin.id == device)
                    .unwrap_or_else(|| panic!("{v}: {track} lost {device}"));
                for (id, value) in params.as_object().unwrap() {
                    assert_eq!(d.params.get(id).copied(), Some(value.as_f64().unwrap() as f32), "{v}: {track}/{device}/{id}");
                }
            }
        }
    }
}

#[test]
fn fixtures_survive_save_and_reload() {
    for v in VERSIONS {
        let dir = tempfile::tempdir().unwrap();
        copy_dir(&fixture(v), dir.path());
        let proj = dir.path().join("Compat.dsproj");
        let first = persist::load(&proj).unwrap().project;
        persist::save(&first, &proj).unwrap();
        let second = persist::load(&proj).unwrap();
        assert!(second.missing_media.is_empty(), "{v}");
        assert_eq!(first, second.project, "{v}: a save and reload changed the project");

        let env: Value = serde_json::from_slice(&std::fs::read(proj.join(persist::PROJECT_FILE)).unwrap()).unwrap();
        assert!(env["schema_version"].as_u64().unwrap() <= persist::SCHEMA_VERSION as u64, "{v}");
        // Saved again, the file is stable.
        persist::save(&second.project, &proj).unwrap();
        assert_eq!(persist::load(&proj).unwrap().project, second.project, "{v}");
    }
}
