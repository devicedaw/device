//! Command::RelinkAssets (Relink Media, roadmap S0.4).

use dstudio_core::*;

fn asset(path: &str) -> Asset {
    Asset {
        id: AssetId(0),
        relative_path: Some("../x.wav".into()),
        absolute_path: path.into(),
        sample_rate: 48_000.0,
        channels: 2,
        frames: 10,
    }
}

#[test]
fn relinking_is_one_undo_step_that_puts_every_path_back() {
    let mut d = Document::new(Project::default());
    d.execute(Command::AddAsset { asset: asset("/old/a.wav") }, Origin::User).unwrap();
    d.execute(Command::AddAsset { asset: asset("/old/b.wav") }, Origin::User).unwrap();
    let (a, b) = (d.project().assets[0].id, d.project().assets[1].id);
    let cmd = Command::RelinkAssets { paths: vec![(a, "/new/a.wav".into()), (b, "/new/b.wav".into())] };
    d.execute(cmd, Origin::User).unwrap();
    assert_eq!(d.project().asset(a).unwrap().absolute_path, "/new/a.wav");
    assert_eq!(d.project().asset(b).unwrap().absolute_path, "/new/b.wav");
    assert_eq!(d.project().asset(a).unwrap().relative_path, None, "worked out again at the next save");
    assert_eq!(d.undo_label(), Some("Relink Media"));
    d.undo().unwrap();
    assert_eq!(d.project().asset(a).unwrap().absolute_path, "/old/a.wav");
    assert_eq!(d.project().asset(b).unwrap().absolute_path, "/old/b.wav");
    d.redo().unwrap();
    assert_eq!(d.project().asset(b).unwrap().absolute_path, "/new/b.wav");
}

#[test]
fn relinking_an_unknown_asset_or_to_no_path_is_refused_and_changes_nothing() {
    let mut d = Document::new(Project::default());
    d.execute(Command::AddAsset { asset: asset("/old/a.wav") }, Origin::User).unwrap();
    let a = d.project().assets[0].id;
    let before = d.project().clone();
    let unknown = Command::RelinkAssets { paths: vec![(a, "/new/a.wav".into()), (AssetId(9_999), "/x.wav".into())] };
    assert!(d.execute(unknown, Origin::User).is_err());
    assert!(d.execute(Command::RelinkAssets { paths: vec![(a, String::new())] }, Origin::User).is_err());
    assert_eq!(*d.project(), before);
}
