//! Edits on the selected effects of one track's device chain: duplicate them, put them in a new
//! chain, unpack a chain. Each returns one `Command::Batch` (one undo step), or None when the
//! selection has nothing it applies to.

use crate::builtin::rack;
use crate::modulation::{head_of, is_rack, members, outputs};
use crate::{Command, Device, DeviceId, Project, TrackId};
use std::collections::HashMap;

/// Chain indices of `sel` on track `t`, sorted, with every selected chain's members added.
pub fn expand(devs: &[Device], sel: &[DeviceId]) -> Vec<usize> {
    let mut out: Vec<usize> = Vec::new();
    for (i, d) in devs.iter().enumerate() {
        if sel.contains(&d.id) {
            out.push(i);
            out.extend(members(devs, i));
        }
    }
    out.sort_unstable();
    out.dedup();
    out
}

/// Copies of the selected effects (a selected chain with its members) right after the last
/// one, with fresh ids. A copy's chain and modulation links name the copy of a device that is
/// copied too. Labelled "Duplicate Effects".
pub fn duplicate(p: &Project, track: TrackId, sel: &[DeviceId]) -> Option<Command> {
    let devs = &p.track(track)?.devices;
    let picked = expand(devs, sel);
    let last = *picked.last()?;
    let mut next = p.next_id;
    let map: HashMap<DeviceId, DeviceId> = picked
        .iter()
        .map(|&i| {
            next += 1;
            (devs[i].id, DeviceId(next - 1))
        })
        .collect();
    // After the last one; inside its chain only when every copy outside a copied chain is a
    // member of that chain, else after the chain.
    let host = head_of(devs, last);
    let loose = picked.iter().filter(|&&i| devs[i].chain.is_none_or(|h| !map.contains_key(&h)));
    let inside = host.is_some_and(|h| loose.clone().all(|&i| head_of(devs, i) == Some(h)));
    let at = match host {
        Some(h) if !inside => members(devs, h).end,
        _ => last + 1,
    };
    let commands = picked
        .iter()
        .enumerate()
        .map(|(k, &i)| {
            let mut d = devs[i].clone();
            d.id = map[&d.id];
            d.chain = match d.chain {
                Some(h) if map.contains_key(&h) => Some(map[&h]),
                Some(h) if inside => Some(h),
                _ => None,
            };
            for l in &mut d.links {
                if let Some(n) = map.get(&l.target) {
                    l.target = *n;
                }
            }
            Command::AddDevice { track, index: Some(at + k), device: d }
        })
        .collect();
    Some(Command::Batch { label: "Duplicate Effects".into(), commands })
}

/// A new chain holding the selected effects, in their order, where the first one was. Only
/// effects outside a chain go in; chain heads and LFOs never do. Labelled "Create Chain".
pub fn group(p: &Project, track: TrackId, sel: &[DeviceId]) -> Option<Command> {
    let devs = &p.track(track)?.devices;
    let picked: Vec<usize> = (0..devs.len()).filter(|&i| sel.contains(&devs[i].id)).collect();
    if picked.is_empty() || picked.iter().any(|&i| devs[i].chain.is_some() || outputs(&devs[i]) > 0) {
        return None;
    }
    let mut head = rack::RACK.instantiate();
    head.id = DeviceId(p.next_id);
    let at = picked[0];
    let mut commands = vec![Command::AddDevice { track, index: Some(at), device: head.clone() }];
    for (k, &i) in picked.iter().enumerate() {
        commands.push(Command::MoveDevice { device: devs[i].id, track, index: at + 1 + k });
    }
    for &i in &picked {
        commands.push(Command::SetDeviceChain { device: devs[i].id, chain: Some(head.id) });
    }
    Some(Command::Batch { label: "Create Chain".into(), commands })
}

/// The chain heads the selection names: a selected head, or the head of a selected member.
pub fn selected_heads(devs: &[Device], sel: &[DeviceId]) -> Vec<DeviceId> {
    let mut out = Vec::new();
    for (i, d) in devs.iter().enumerate() {
        if !sel.contains(&d.id) {
            continue;
        }
        let h = if is_rack(d) { Some(i) } else { head_of(devs, i) };
        if let Some(h) = h.map(|h| devs[h].id)
            && !out.contains(&h)
        {
            out.push(h);
        }
    }
    out
}

/// Unpack the selected chains: their members stay where they are as ordinary effects and the
/// heads (with their macros) go. Labelled "Unpack Chain".
pub fn unpack(p: &Project, track: TrackId, sel: &[DeviceId]) -> Option<Command> {
    let devs = &p.track(track)?.devices;
    let heads = selected_heads(devs, sel);
    if heads.is_empty() {
        return None;
    }
    let mut commands = Vec::new();
    for h in heads {
        let hi = devs.iter().position(|d| d.id == h)?;
        // Members leave from the last one, so each leaves the end of the run.
        commands.extend(members(devs, hi).rev().map(|i| Command::SetDeviceChain { device: devs[i].id, chain: None }));
        commands.push(Command::RemoveDevice { device: h });
    }
    Some(Command::Batch { label: "Unpack Chain".into(), commands })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builtin;
    use crate::history::Document;

    /// A track with effects named by builtin id; "[" opens a chain whose members follow up to "]".
    fn setup(ids: &[&str]) -> (Document, TrackId, Vec<DeviceId>) {
        let mut doc = Document::new(Project::default());
        let track = doc.project().tracks[0].id;
        let mut head = None;
        for id in ids {
            match *id {
                "[" => {
                    let i = doc.project().track(track).unwrap().devices.len();
                    assert!(
                        doc.execute(Command::AddDevice { track, index: None, device: rack::RACK.instantiate() }, crate::Origin::User)
                            .is_ok()
                    );
                    head = Some(doc.project().track(track).unwrap().devices[i].id);
                }
                "]" => head = None,
                _ => {
                    let mut d = builtin::find(id).unwrap().instantiate();
                    d.chain = head;
                    assert!(doc.execute(Command::AddDevice { track, index: None, device: d }, crate::Origin::User).is_ok());
                }
            }
        }
        let ids = doc.project().track(track).unwrap().devices.iter().map(|d| d.id).collect();
        (doc, track, ids)
    }

    fn names(doc: &Document, track: TrackId) -> Vec<String> {
        let devs = &doc.project().track(track).unwrap().devices;
        devs.iter().map(|d| format!("{}{}", if d.chain.is_some() { "  " } else { "" }, d.plugin.id)).collect()
    }

    #[test]
    fn duplicate_copies_after_the_last_selected_and_undoes_in_one_step() {
        let (mut doc, t, ids) = setup(&["channeleq", "compressor", "delay"]);
        let before = doc.project().track(t).unwrap().clone();
        let cmd = duplicate(doc.project(), t, &[ids[0], ids[1]]).unwrap();
        assert!(doc.execute(cmd, crate::Origin::User).is_ok());
        assert_eq!(names(&doc, t), ["channeleq", "compressor", "channeleq", "compressor", "delay"]);
        let devs = &doc.project().track(t).unwrap().devices;
        assert!(devs[2].id != ids[0] && devs[3].id != ids[1], "fresh ids");
        doc.undo().unwrap();
        assert_eq!(doc.project().track(t).unwrap(), &before);
    }

    #[test]
    fn duplicating_a_chain_copies_its_members_into_the_copy() {
        let (mut doc, t, ids) = setup(&["[", "channeleq", "compressor", "]", "delay"]);
        assert!(doc.execute(duplicate(doc.project(), t, &[ids[0]]).unwrap(), crate::Origin::User).is_ok());
        let devs = &doc.project().track(t).unwrap().devices;
        assert_eq!(devs.len(), 7);
        assert_eq!(members(devs, 3), 4..6, "the copy holds copies of the members");
        assert_eq!(devs[6].plugin.id, "delay");
        // A member alone stays in its chain, right after itself.
        let (mut doc, t, ids) = setup(&["[", "channeleq", "compressor", "]"]);
        assert!(doc.execute(duplicate(doc.project(), t, &[ids[1]]).unwrap(), crate::Origin::User).is_ok());
        assert_eq!(names(&doc, t), ["device-rack", "  channeleq", "  channeleq", "  compressor"]);
    }

    #[test]
    fn group_puts_the_selection_in_a_new_chain_where_the_first_was() {
        let (mut doc, t, ids) = setup(&["channeleq", "delay", "compressor"]);
        let before = doc.project().track(t).unwrap().clone();
        assert!(doc.execute(group(doc.project(), t, &[ids[0], ids[2]]).unwrap(), crate::Origin::User).is_ok());
        assert_eq!(names(&doc, t), ["device-rack", "  channeleq", "  compressor", "delay"]);
        doc.undo().unwrap();
        assert_eq!(doc.project().track(t).unwrap(), &before);
        // Chain members and heads don't go in another chain.
        let (doc, t, ids) = setup(&["[", "channeleq", "]", "delay"]);
        assert!(group(doc.project(), t, &[ids[1], ids[2]]).is_none());
        assert!(group(doc.project(), t, &[ids[0]]).is_none());
    }

    #[test]
    fn unpack_frees_the_members_of_the_selected_chain() {
        let (mut doc, t, ids) = setup(&["[", "channeleq", "compressor", "]", "delay"]);
        // Selecting a member unpacks its chain.
        assert!(doc.execute(unpack(doc.project(), t, &[ids[2]]).unwrap(), crate::Origin::User).is_ok());
        assert_eq!(names(&doc, t), ["channeleq", "compressor", "delay"]);
        assert!(unpack(doc.project(), t, &[ids[3]]).is_none(), "no chain selected");
    }
}
