//! Built-in device parameters are append-only (AGENTS.md): projects, presets and automation name
//! a parameter by its id, so removing or renaming one silently breaks saved projects.
//!
//! `fixtures/builtin_params.lock` lists every parameter id of every built-in device as released.
//! This test fails when a locked device or parameter disappears, changes its name, or loses or
//! reorders a choice. New devices, new parameters and new choices at the end are fine; add them to
//! the lock with
//!
//!     DSTUDIO_BLESS=1 cargo test -p dstudio-core --test param_lock
//!
//! Blessing rewrites the whole file from the current definitions, so review its diff: it must
//! only gain lines.

use dstudio_core::builtin;
use std::path::Path;

const LOCK: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/builtin_params.lock");

/// `device<TAB>param<TAB>name<TAB>choice|choice|...`, one line per parameter, in definition order.
fn current() -> Vec<String> {
    let mut lines = Vec::new();
    for def in builtin::all() {
        for p in def.params {
            lines.push(format!("{}\t{}\t{}\t{}", def.id, p.id, p.name, p.choices.join("|")));
        }
    }
    lines
}

fn fields(line: &str) -> (&str, &str, &str, &str) {
    let mut f = line.splitn(4, '\t');
    (f.next().unwrap_or(""), f.next().unwrap_or(""), f.next().unwrap_or(""), f.next().unwrap_or(""))
}

#[test]
fn builtin_parameters_are_append_only() {
    let now = current();
    if std::env::var_os("DSTUDIO_BLESS").is_some() {
        let header = "# Built-in device parameters as released: device, parameter id, name, choices. Append-only.\n# Regenerate: DSTUDIO_BLESS=1 cargo test -p dstudio-core --test param_lock\n";
        std::fs::write(Path::new(LOCK), format!("{header}{}\n", now.join("\n"))).unwrap();
        return;
    }
    let lock = std::fs::read_to_string(LOCK).expect("tests/fixtures/builtin_params.lock is missing");
    let mut problems = Vec::new();
    for line in lock.lines().filter(|l| !l.is_empty() && !l.starts_with('#')) {
        let (device, param, name, choices) = fields(line);
        let Some(def) = builtin::find(device) else {
            problems.push(format!("device `{device}` is gone"));
            continue;
        };
        let Some(p) = def.param(param) else {
            problems.push(format!("`{device}` lost parameter `{param}`"));
            continue;
        };
        if p.name != name {
            problems.push(format!("`{device}.{param}` was named {name:?}, now {:?}", p.name));
        }
        let locked: Vec<&str> = if choices.is_empty() { Vec::new() } else { choices.split('|').collect() };
        if p.choices.len() < locked.len() || p.choices[..locked.len()] != locked[..] {
            problems.push(format!("`{device}.{param}` changed or lost choices: locked {locked:?}, now {:?}", p.choices));
        }
    }
    assert!(
        problems.is_empty(),
        "built-in parameters are append-only; saved projects name them by id:\n  {}\nIf the change is a deliberate, compatible one, regenerate the lock with:\n  DSTUDIO_BLESS=1 cargo test -p dstudio-core --test param_lock",
        problems.join("\n  ")
    );
}
