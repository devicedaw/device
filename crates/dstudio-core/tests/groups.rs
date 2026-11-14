//! Group (folder) tracks: buses among the ordinary tracks that their tracks play into.

use dstudio_core::*;

fn add(d: &mut Document, kind: TrackKind, name: &str, index: Option<usize>) -> TrackId {
    let t = Track::new(TrackId(0), kind, name, ColorIndex(3));
    d.execute(Command::AddTrack { index, track: t }, Origin::User).unwrap();
    let id = TrackId(d.project().next_id - 1);
    assert_eq!(d.project().track(id).unwrap().name, name);
    id
}

fn add_return(d: &mut Document) -> TrackId {
    let name = d.project().next_return_name();
    add(d, TrackKind::Return, &name, None)
}

#[test]
fn a_group_sits_among_the_ordinary_tracks_and_takes_outputs() {
    let mut d = Document::new(Project::default());
    let g = add(&mut d, TrackKind::Group, "Group", Some(0));
    assert_eq!(d.project().next_group_name(), "Group 2");
    assert!(d.project().track(g).unwrap().is_group() && d.project().track(g).unwrap().is_bus());
    assert_eq!(d.project().tracks[0].id, g);
    let a = d.project().tracks[1].id;
    d.execute(Command::SetTrackOutput { track: a, output: Some(g) }, Origin::User).unwrap();
    assert_eq!(d.project().group_members(g).map(|t| t.id).collect::<Vec<_>>(), [a]);
    // The engine runs the group as a bus.
    let (snap, _) = snapshot::build(d.project(), 48_000.0, &Default::default());
    assert!(snap.tracks.iter().find(|t| t.track_id == g.0).unwrap().is_return);
    assert_eq!(snap.tracks.iter().find(|t| t.track_id == a.0).unwrap().output, Some(g.0));
    d.undo().unwrap();
    assert!(d.project().group_members(g).next().is_none());
}

#[test]
fn a_group_refuses_clips_an_output_and_arming() {
    let mut d = Document::new(Project::default());
    let g = add(&mut d, TrackKind::Group, "Group 1", Some(0));
    let g2 = add(&mut d, TrackKind::Group, "Group 2", Some(0));
    let r = add_return(&mut d);
    let before = d.project().clone();
    assert!(d.execute(Command::SetTrackOutput { track: g, output: Some(g2) }, Origin::User).is_err());
    assert!(d.execute(Command::SetTrackOutput { track: g, output: Some(r) }, Origin::User).is_err());
    assert!(d.execute(Command::SetTrackArm { track: g, arm: true }, Origin::User).is_err());
    // Sends go to returns only, never to a group.
    let a = d.project().tracks.iter().find(|t| t.kind == TrackKind::Audio).unwrap().id;
    assert!(d.execute(Command::SetSends { track: a, sends: vec![TrackSend::new(g)] }, Origin::User).is_err());
    assert_eq!(d.project(), &before);
}

#[test]
fn a_group_sends_into_a_return_but_a_return_does_not_send() {
    let mut d = Document::new(Project::default());
    let g = add(&mut d, TrackKind::Group, "Group 1", Some(0));
    let r = add_return(&mut d);
    let r2 = add_return(&mut d);
    let mut s = TrackSend::new(r);
    s.level = 0.5;
    d.execute(Command::SetSends { track: g, sends: vec![s] }, Origin::User).unwrap();
    assert_eq!(d.project().track(g).unwrap().sends.len(), 1);
    assert!(d.execute(Command::SetSends { track: r, sends: vec![TrackSend::new(r2)] }, Origin::User).is_err());
    // A saved group send survives loading.
    let mut p = d.project().clone();
    p.sanitize_routing();
    assert_eq!(p.track(g).unwrap().sends[0].target, r);
    assert_eq!(p.track(g).unwrap().sends[0].level, 0.5);
    d.undo().unwrap();
    assert!(d.project().track(g).unwrap().sends.is_empty());
}

#[test]
fn groups_and_returns_share_the_bus_limit() {
    let mut d = Document::new(Project::default());
    for _ in 0..MAX_RETURNS - 1 {
        add_return(&mut d);
    }
    add(&mut d, TrackKind::Group, "Group 1", Some(0));
    let before = d.project().clone();
    let t = Track::new(TrackId(0), TrackKind::Group, "Group 2", ColorIndex(1));
    assert!(d.execute(Command::AddTrack { index: Some(0), track: t }, Origin::User).is_err());
    let r = Track::new(TrackId(0), TrackKind::Return, "R", ColorIndex(1));
    assert!(d.execute(Command::AddTrack { index: None, track: r }, Origin::User).is_err());
    assert_eq!(d.project(), &before);
}

#[test]
fn removing_a_group_sends_its_tracks_to_the_master() {
    let mut d = Document::new(Project::default());
    let g = add(&mut d, TrackKind::Group, "Group 1", Some(0));
    let a = d.project().tracks[1].id;
    d.execute(Command::SetTrackOutput { track: a, output: Some(g) }, Origin::User).unwrap();
    d.execute(Command::RemoveTrack { track: g }, Origin::User).unwrap();
    assert_eq!(d.project().track(a).unwrap().output, None);
    d.undo().unwrap();
    assert_eq!(d.project().track(a).unwrap().output, Some(g));
}

#[test]
fn loading_repairs_damaged_groups() {
    let mut p = Project::default();
    let a = p.tracks[0].id;
    let mut g = Track::new(TrackId(p.next_id), TrackKind::Group, "Group 1", ColorIndex(1));
    p.next_id += 1;
    let mut g2 = Track::new(TrackId(p.next_id), TrackKind::Group, "Group 2", ColorIndex(1));
    p.next_id += 1;
    g.output = Some(g2.id); // a group can't play into a group
    g.arm = true;
    g2.midi_clips.push(MidiClip::empty(0, PPQ * 4)); // a group with notes is an instrument track
    let (gid, g2id) = (g.id, g2.id);
    p.tracks.insert(0, g);
    p.tracks.push(g2);
    p.tracks.iter_mut().find(|t| t.id == a).unwrap().output = Some(gid);
    p.sanitize_routing();
    let g = p.track(gid).unwrap();
    assert!(g.is_group() && g.output.is_none() && !g.arm);
    assert_eq!(p.track(g2id).unwrap().kind, TrackKind::Instrument);
    assert_eq!(p.track(a).unwrap().output, Some(gid));
}
