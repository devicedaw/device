//! Sends from one return track into another (spec 016): only into later returns, moves that
//! would break the order are refused, removal and undo, persistence and load repair.

use dstudio_core::*;
use std::time::{Duration, Instant};

fn add_return(d: &mut Document) -> TrackId {
    let name = d.project().next_return_name();
    let t = Track::new(TrackId(0), TrackKind::Return, &name, ColorIndex(9));
    d.execute(Command::AddTrack { index: None, track: t }, Origin::User).unwrap();
    d.project().tracks.last().unwrap().id
}

fn set_sends(d: &mut Document, track: TrackId, sends: Vec<TrackSend>) -> Result<(), EditError> {
    d.execute(Command::Batch { label: "Add Send".into(), commands: vec![Command::SetSends { track, sends }] }, Origin::User)
}

/// Returns A, B and C after the default tracks.
fn three_returns() -> (Document, [TrackId; 3]) {
    let mut d = Document::new(Project::default());
    let ids = [add_return(&mut d), add_return(&mut d), add_return(&mut d)];
    (d, ids)
}

#[test]
fn a_return_sends_only_into_later_returns() {
    let (mut d, [a, b, c]) = three_returns();
    set_sends(&mut d, a, vec![TrackSend::new(b), TrackSend { level: 0.5, pre_fader: true, target: c }]).unwrap();
    set_sends(&mut d, b, vec![TrackSend::new(c)]).unwrap();
    assert!(d.project().send_allowed(a, b) && d.project().send_allowed(b, c));
    assert!(!d.project().send_allowed(c, a) && !d.project().send_allowed(b, b));
    // Into itself, an earlier return, a group, an ordinary track; levels out of range; twice.
    let group = TrackId(d.project().next_id);
    d.execute(Command::AddTrack { index: Some(0), track: Track::new(group, TrackKind::Group, "Group", ColorIndex(1)) }, Origin::User)
        .unwrap();
    let audio = d.project().tracks[1].id;
    let before = d.project().clone();
    for (track, sends) in [
        (c, vec![TrackSend::new(c)]),
        (c, vec![TrackSend::new(a)]),
        (b, vec![TrackSend::new(a)]),
        (b, vec![TrackSend::new(group)]),
        (b, vec![TrackSend::new(audio)]),
        (b, vec![TrackSend { level: 2.5, ..TrackSend::new(c) }]),
        (a, vec![TrackSend::new(c), TrackSend::new(c)]),
    ] {
        assert!(set_sends(&mut d, track, sends.clone()).is_err(), "{sends:?} from {track:?} must be refused");
        assert_eq!(d.project(), &before);
    }
    // A return can't be added with a send that points up the list either.
    let mut r = Track::new(TrackId(0), TrackKind::Return, "R", ColorIndex(9));
    r.sends.push(TrackSend::new(a));
    assert!(d.execute(Command::AddTrack { index: None, track: r.clone() }, Origin::User).is_err());
    // Inserted before A, it may send into A.
    let first = d.project().ordinary_track_count();
    d.execute(Command::AddTrack { index: Some(first), track: r }, Origin::User).unwrap();
}

#[test]
fn return_send_edits_have_labels_and_level_drags_are_one_step() {
    let (mut d, [a, b, _]) = three_returns();
    set_sends(&mut d, a, vec![TrackSend::new(b)]).unwrap();
    assert_eq!(d.undo_label(), Some("Add Send"));
    let t0 = Instant::now();
    for i in 1..=10 {
        let sends = vec![TrackSend { level: 1.0 - i as f32 * 0.05, ..TrackSend::new(b) }];
        d.execute_at(Command::SetSends { track: a, sends }, t0 + Duration::from_millis(i * 16)).unwrap();
    }
    assert_eq!(d.undo_label(), Some("Send Level Change"));
    d.undo().unwrap();
    assert_eq!(d.project().track(a).unwrap().sends, vec![TrackSend::new(b)]);
    d.undo().unwrap();
    assert!(d.project().track(a).unwrap().sends.is_empty());
}

#[test]
fn removing_a_return_takes_the_return_sends_into_it_and_undo_restores_them() {
    let (mut d, [a, b, c]) = three_returns();
    set_sends(&mut d, a, vec![TrackSend::new(c), TrackSend { level: 0.25, ..TrackSend::new(b) }]).unwrap();
    set_sends(&mut d, b, vec![TrackSend::new(c)]).unwrap();
    let before = d.project().clone();
    d.execute(Command::RemoveTrack { track: b }, Origin::User).unwrap();
    assert_eq!(d.undo_label(), Some("Delete Track"));
    assert_eq!(d.project().track(a).unwrap().sends, vec![TrackSend::new(c)]);
    d.undo().unwrap();
    assert_eq!(d.project(), &before);
    d.redo().unwrap();
    d.undo().unwrap();
    assert_eq!(d.project(), &before);
    // Removing the sender, then undo, puts it back with its sends.
    d.execute(Command::RemoveTrack { track: a }, Origin::User).unwrap();
    d.undo().unwrap();
    assert_eq!(d.project(), &before);
}

#[test]
fn moves_that_keep_every_send_pointing_down_are_allowed() {
    let (mut d, [a, b, c]) = three_returns();
    set_sends(&mut d, a, vec![TrackSend::new(b)]).unwrap();
    let ia = d.project().track_index(a).unwrap();
    // C to the top of the returns: A still sends down into B.
    d.execute(Command::MoveTrack { track: c, index: ia }, Origin::User).unwrap();
    assert_eq!(d.undo_label(), Some("Move Track"));
    d.undo().unwrap();
    // B above A while A sends into B: refused, nothing changes.
    let before = d.project().clone();
    let err = d.execute(Command::MoveTrack { track: b, index: ia }, Origin::User).unwrap_err();
    assert!(err.to_string().contains("Return A sends into Return B: remove that send to move it here"), "{err}");
    assert_eq!(d.project(), &before);
    assert_eq!(d.undo_label(), Some("Add Send"), "a refused move adds no undo step");
    // A below B: refused the same way.
    assert!(d.execute(Command::MoveTrack { track: a, index: ia + 2 }, Origin::User).is_err());
    assert_eq!(d.project(), &before);
}

#[test]
fn return_sends_round_trip_and_old_projects_load_unchanged() {
    let (mut d, [a, b, c]) = three_returns();
    set_sends(&mut d, a, vec![TrackSend { level: 0.5, pre_fader: true, target: b }, TrackSend::new(c)]).unwrap();
    let dir = tempfile::tempdir().unwrap();
    persist::save(d.project(), dir.path()).unwrap();
    assert_eq!(&persist::load(dir.path()).unwrap().project, d.project());
    // A project without return sends loads as it was saved.
    let (old, _) = three_returns();
    let dir = tempfile::tempdir().unwrap();
    persist::save(old.project(), dir.path()).unwrap();
    assert_eq!(&persist::load(dir.path()).unwrap().project, old.project());
}

#[test]
fn a_hand_edited_loop_or_upward_send_is_dropped_on_load() {
    let (d, [a, b, c]) = three_returns();
    let mut p = d.project().clone();
    let audio = p.tracks[0].id;
    let ia = p.track_index(a).unwrap();
    // A into B and C (kept), B into A (a loop, upward), C into B (upward), B into C twice, B into
    // an ordinary track.
    p.tracks[ia].sends = vec![TrackSend::new(b), TrackSend { level: 9.0, ..TrackSend::new(c) }];
    p.tracks[ia + 1].sends = vec![TrackSend::new(a), TrackSend::new(c), TrackSend::new(c), TrackSend::new(audio)];
    p.tracks[ia + 2].sends = vec![TrackSend::new(b)];
    let dir = tempfile::tempdir().unwrap();
    persist::save(&p, dir.path()).unwrap();
    let loaded = persist::load(dir.path()).unwrap().project;
    let mut want = p.clone();
    want.tracks[ia].sends[1].level = TrackSend::MAX_LEVEL;
    want.tracks[ia + 1].sends = vec![TrackSend::new(c)];
    want.tracks[ia + 2].sends.clear();
    assert_eq!(loaded, want);
    assert_eq!(loaded.return_order_conflict(), None);
}
