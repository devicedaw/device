//! `--self-test`: drives the real UI input path with synthetic Slint window
//! events (pointer + keyboard → TouchArea/FocusScope → callbacks → Commands)
//! and checks the resulting `Document` / engine state. Does not touch the
//! system cursor or keyboard. Click targets are offsets from the live panel positions
//! (AppWindow.anchors) or from positions the controller reports, in window units.

use crate::assistant_panel::EntryKind;
use crate::controller::{BottomTab, Controller};
use crate::{AppWindow, Studio};
use dstudio_core::assistant::Mode;
use dstudio_core::builtin::synth_v2::SYNTH_V2;
use dstudio_core::{ClipId, PPQ, TrackKind, db_to_gain};
use slint::platform::{Key, PointerEventButton, WindowEvent};
use slint::{ComponentHandle, LogicalPosition, Model, SharedString};
use std::cell::Cell;
use std::cell::RefCell;
use std::rc::Rc;

type Ctl = Rc<RefCell<Controller>>;

thread_local! {
    /// Undo depth before the Project export section added its returns.
    static EXPORT_UNDO_DEPTH: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// This run's scratch folder: every file the self-test writes (exports, saved projects, WAVs,
/// recordings, clip renders, its plugin cache) lives under it, so runs that overlap on one
/// machine never share a path. Created on first use; removed when every check passes.
pub fn run_dir() -> &'static std::path::Path {
    static DIR: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();
    DIR.get_or_init(|| {
        let base = std::env::temp_dir();
        (0..1000)
            .map(|n| base.join(format!("dstudio-selftest-{}-{n}", std::process::id())))
            .find(|d| std::fs::create_dir(d).is_ok())
            .expect("self-test: cannot create a scratch folder in the temp directory")
    })
}

/// Whether an asset's stored path lies in `dir`. Assets keep canonical paths, and on Windows
/// the canonical form (`\\?\C:\Users\runneradmin\...`) differs from the temp folder's
/// (`C:\Users\RUNNER~1\...`), so compare against both spellings of the folder.
fn in_dir(asset_path: &str, dir: &std::path::Path) -> bool {
    let canon = std::fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
    asset_path.contains(&*dir.to_string_lossy()) || asset_path.contains(&*canon.to_string_lossy())
}

/// The plugin scan cache the self-tests share between runs (so a run does not rescan every
/// plugin). A run works on its own copy and puts it back atomically when it ends.
pub fn shared_plugin_cache() -> std::path::PathBuf {
    std::env::temp_dir().join("dstudio-selftest-plugin-cache.json")
}

/// The run's copy of the shared plugin cache.
pub fn run_plugin_cache() -> std::path::PathBuf {
    let mine = run_dir().join("plugin-cache.json");
    if !mine.exists() {
        let _ = std::fs::copy(shared_plugin_cache(), &mine);
    }
    mine
}

/// End of a run: the run's plugin cache replaces the shared one (rename, so a run that starts
/// meanwhile reads a whole file), and the scratch folder goes unless a check failed.
fn finish_run(failed: usize) {
    let dir = run_dir();
    let tmp = std::env::temp_dir().join(format!("dstudio-selftest-plugin-cache.json.{}", std::process::id()));
    if std::fs::copy(dir.join("plugin-cache.json"), &tmp).is_ok() && std::fs::rename(&tmp, shared_plugin_cache()).is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    if failed == 0 {
        if let Err(e) = std::fs::remove_dir_all(dir) {
            println!("self-test: could not remove the scratch folder {}: {e}", dir.display());
        }
    } else {
        println!("self-test: scratch files kept in {}", dir.display());
    }
}

enum Step {
    Do(&'static str, Box<dyn Fn(&AppWindow)>),
    /// Like `Do`, but computes coordinates from the controller first (the closure must
    /// release its borrow before dispatching events).
    DoC(&'static str, Box<dyn Fn(&AppWindow, &Ctl)>),
    Check(&'static str, Box<dyn Fn(&Controller, &AppWindow) -> Result<(), String>>),
}

thread_local! {
    /// Audio loop start before the footer Position drag (audio loop section).
    static AUDIO_LOOP_START: Cell<i64> = const { Cell::new(0) };
    /// Track count and the open clip before the audio clip editor's Resample.
    static RESAMPLE_BEFORE: Cell<(usize, u64)> = const { Cell::new((0, 0)) };
    /// Undo history length before the audio loop bar drags.
    static AUDIO_LOOP_HISTORY: Cell<usize> = const { Cell::new(0) };
}

/// Height of the in-window menu bar (Linux draws the MenuBar inside the window above the content;
/// macOS and Windows use native ones, so 0). Layout anchors are in content coordinates and
/// pointer events in window coordinates, so every pointer event is moved down by this much.
fn menu_bar_h() -> f32 {
    APP.with(|a| {
        let a = a.borrow().as_ref().and_then(|w| w.upgrade()).expect("self-test window");
        let full = a.window().size().to_logical(a.window().scale_factor()).height;
        (full - a.get_content_height()).max(0.0)
    })
}
/// Sends an event to the window, moving pointer positions from content to window coordinates.
fn dispatch(w: &slint::Window, e: WindowEvent) {
    let dy = menu_bar_h();
    let at = |p: LogicalPosition| LogicalPosition::new(p.x, p.y + dy);
    let e = match e {
        WindowEvent::PointerPressed { position, button } => WindowEvent::PointerPressed { position: at(position), button },
        WindowEvent::PointerReleased { position, button } => WindowEvent::PointerReleased { position: at(position), button },
        WindowEvent::PointerMoved { position } => WindowEvent::PointerMoved { position: at(position) },
        WindowEvent::PointerScrolled { position, delta_x, delta_y } => {
            WindowEvent::PointerScrolled { position: at(position), delta_x, delta_y }
        }
        e => e,
    };
    w.dispatch_event(e);
}

fn press(app: &AppWindow, x: f32, y: f32) {
    let w = app.window();
    let p = LogicalPosition::new(x, y);
    dispatch(w, WindowEvent::PointerMoved { position: p });
    dispatch(w, WindowEvent::PointerPressed { position: p, button: PointerEventButton::Left });
}
fn release(app: &AppWindow, x: f32, y: f32) {
    dispatch(app.window(), WindowEvent::PointerReleased { position: LogicalPosition::new(x, y), button: PointerEventButton::Left });
}
fn click(app: &AppWindow, x: f32, y: f32) {
    press(app, x, y);
    release(app, x, y);
}
fn drag(app: &AppWindow, x0: f32, y0: f32, x1: f32, y1: f32) {
    press(app, x0, y0);
    for i in 1..=12 {
        let t = i as f32 / 12.0;
        dispatch(app.window(), WindowEvent::PointerMoved { position: LogicalPosition::new(x0 + (x1 - x0) * t, y0 + (y1 - y0) * t) });
    }
    release(app, x1, y1);
}
fn key(app: &AppWindow, text: impl Into<SharedString> + Clone, primary: bool, shift: bool) {
    let w = app.window();
    if primary {
        dispatch(w, WindowEvent::KeyPressed { text: Key::Control.into() });
    }
    if shift {
        dispatch(w, WindowEvent::KeyPressed { text: Key::Shift.into() });
    }
    dispatch(w, WindowEvent::KeyPressed { text: text.clone().into() });
    dispatch(w, WindowEvent::KeyReleased { text: text.into() });
    if shift {
        dispatch(w, WindowEvent::KeyReleased { text: Key::Shift.into() });
    }
    if primary {
        dispatch(w, WindowEvent::KeyReleased { text: Key::Control.into() });
    }
}

fn type_text(app: &AppWindow, s: &str) {
    let w = app.window();
    for ch in s.chars() {
        let t: SharedString = ch.to_string().into();
        dispatch(w, WindowEvent::KeyPressed { text: t.clone() });
        dispatch(w, WindowEvent::KeyReleased { text: t });
    }
}

fn approx(a: f32, b: f32, tol: f32) -> bool {
    (a - b).abs() <= tol
}

thread_local! {
    /// The window under test, for the layout getters below.
    static APP: RefCell<Option<slint::Weak<AppWindow>>> = const { RefCell::new(None) };
}

/// Where the panels are right now (AppWindow.anchors, window units). Every click target is an
/// offset from one of these or from a position the controller reports, never a fixed window
/// point, so a layout change in one panel does not move the targets in another.
fn geo() -> crate::LayoutAnchors {
    APP.with(|a| a.borrow().as_ref().and_then(|w| w.upgrade()).map(|a| a.get_anchors())).expect("self-test window")
}
/// Size of the content area in window units (the window less any in-window menu bar), the space
/// the layout anchors are measured in.
fn win() -> (f32, f32) {
    APP.with(|a| {
        let a = a.borrow().as_ref().and_then(|w| w.upgrade()).expect("self-test window");
        let s = a.window().size().to_logical(a.window().scale_factor());
        (s.width, a.get_content_height().min(s.height))
    })
}
/// Lanes origin (ruler top-left) and the top of track 0's row.
fn lane_x() -> f32 {
    geo().lanes_x
}
fn lane_y0() -> f32 {
    geo().lanes_y
}
fn track0_y() -> f32 {
    geo().tracks_y
}
/// Top of the bottom panel (Devices or Clip tab, or the mixer).
fn bottom_y() -> f32 {
    geo().bottom_y
}
fn bottom_x() -> f32 {
    geo().bottom_x
}
/// Left edge of the track header column.
fn tracks_x() -> f32 {
    geo().tracks_x
}
/// Left edge of the 1440-unit transport block in the header.
fn transport_x() -> f32 {
    geo().transport_x
}
/// Bottom edge of the bottom panel.
fn bottom_end() -> f32 {
    win().1 - 5.0
}
/// A point over the lanes, and one over the track headers, for wheel events.
fn lanes_point() -> LogicalPosition {
    LogicalPosition::new(lane_x() + 446.35, track0_y() + 155.5)
}
fn headers_point() -> LogicalPosition {
    LogicalPosition::new(tracks_x() + 37.35, track0_y() + 155.5)
}
/// Left edge of the automation parameter picker and of its submenu (ui/app.slint AutoPicker).
fn picker_x(a: &AppWindow) -> (f32, f32) {
    let px = a.global::<Studio>().get_picker_x();
    let max = win().0 - 5.0 - 188.0;
    (px.min(max).max(5.0), (px + 190.0).min(max))
}
const PITCH: f32 = crate::timeline::TRACK_PITCH;

fn clip_start_beats(c: &Controller, track: usize) -> Option<f64> {
    c.project().tracks.get(track)?.clips.first().map(|cl| cl.start as f64 / PPQ as f64)
}

/// Piano-roll texture origin in the window.
fn pr_origin() -> (f32, f32) {
    (bottom_x() + crate::pianoroll::PANEL_ORIGIN.0, bottom_y() + crate::pianoroll::PANEL_ORIGIN.1)
}

/// Window point at the centre of a piano-roll cell (beat is clip-relative).
fn pr_xy(ctl: &Ctl, beat: f64, pitch: i32) -> (f32, f32) {
    let v = ctl.borrow().pr_view();
    (pr_origin().0 + v.x_of(beat), pr_origin().1 + v.y_of(pitch) + crate::pianoroll::ROW_H / 2.0)
}

/// Presses Tab until the footer control `index` (FooterKeys) has the keyboard focus.
fn tab_to(app: &AppWindow, index: i32) {
    for _ in 0..60 {
        if app.global::<Studio>().get_footer_focus() == index {
            return;
        }
        key(app, Key::Tab, false, false);
    }
}

/// What the window shows: a fresh frame drawn through Slint's caches, the way the screen gets it
/// (not our own textures). Take it with the controller unborrowed, so the frame also renders the
/// timeline and piano roll.
struct Shot {
    buf: slint::SharedPixelBuffer<slint::Rgba8Pixel>,
    scale: f32,
    dy: f32,
}

impl Shot {
    fn take(app: &AppWindow) -> Option<Shot> {
        let buf = app.window().take_snapshot().inspect_err(|e| eprintln!("self-test: snapshot failed: {e}")).ok()?;
        Some(Shot { buf, scale: app.window().scale_factor(), dy: menu_bar_h() })
    }

    /// The colour at content point (x, y).
    fn at(&self, x: f32, y: f32) -> Option<[u8; 3]> {
        let (px, py) = ((x * self.scale) as u32, ((y + self.dy) * self.scale) as u32);
        (px < self.buf.width() && py < self.buf.height()).then(|| {
            let p = self.buf.as_slice()[(py * self.buf.width() + px) as usize];
            [p.r, p.g, p.b]
        })
    }
}

/// Largest per-channel difference between two colours.
fn px_diff(a: [u8; 3], b: [u8; 3]) -> u8 {
    (0..3).map(|i| a[i].abs_diff(b[i])).max().unwrap_or(0)
}

fn instrument_clip(c: &Controller) -> Option<&dstudio_core::MidiClip> {
    c.project().tracks.iter().find(|t| t.kind == TrackKind::Instrument)?.midi_clips.first()
}

fn notes_of(c: &Controller) -> Vec<dstudio_core::Note> {
    instrument_clip(c).map(|m| m.notes.clone()).unwrap_or_default()
}

/// MIDI clip + piano roll steps (appended to the main script).
fn midi_steps() -> Vec<Step> {
    use Step::*;
    let hist = Rc::new(Cell::new(0usize));
    let saved: Rc<RefCell<(Vec<dstudio_core::Note>, i64)>> = Rc::new(RefCell::new((Vec::new(), 0)));
    let h = |n: usize| -> Vec<Rc<Cell<usize>>> { (0..n).map(|_| hist.clone()).collect() };
    let hs = h(16);
    let (sv1, sv2) = (saved.clone(), saved.clone());
    let lane_xy = |ctl: &Ctl, beat: f64, track: usize| -> (f32, f32) {
        let c = ctl.borrow();
        (lane_x() + c.view.x_of(beat), track0_y() + track as f32 * PITCH + 40.0 - c.view.scroll_y as f32)
    };
    let record = |hc: Rc<Cell<usize>>| -> Box<dyn Fn(&Controller, &AppWindow) -> Result<(), String>> {
        Box::new(move |c, _| {
            hc.set(c.doc.history_labels().len());
            Ok(())
        })
    };
    // One new undo step since the last record, with this label; re-records.
    let one_step = |hc: Rc<Cell<usize>>, label: &'static str| {
        move |c: &Controller| -> Result<(), String> {
            let n = c.doc.history_labels().len();
            let ok = n == hc.get() + 1 && c.doc.undo_label() == Some(label);
            hc.set(n);
            ok.then_some(()).ok_or(format!("history {} -> {n}, last {:?} (want {label})", hc.get(), c.doc.undo_label()))
        }
    };
    let [h0, h1, h2, h3, h4, h5, h6, h7, h8, h9, h10, h11, h12, h13, h14, h15] = <[Rc<Cell<usize>>; 16]>::try_from(hs).ok().unwrap();
    let hp = Rc::new(Cell::new(0usize));
    let (hp0, hp1, hp2) = (hp.clone(), hp.clone(), hp.clone());
    // Toolbar zoom sliders: (left x, width, y) of the vertical (0) and horizontal (1) track.
    let zoom_track = |ctl: &Ctl, axis: usize| -> (f32, f32, f32) {
        let right = pr_origin().0 + ctl.borrow().pr.size.0 + 0.5;
        let y = bottom_y() + 41.0;
        if axis == 0 { (right - 224.0, 67.5, y) } else { (right - 132.5, 74.0, y) }
    };
    // Reverse, wheel zoom and typing sections: notes and view numbers carried between steps.
    let rvs = Rc::new(Cell::new(0usize));
    let rvk = Rc::new(Cell::new(0usize));
    let (rv1, rv2, rv3) = (rvs.clone(), rvk.clone(), rvs.clone());
    let rv4 = rvk.clone();
    let rvn: Rc<RefCell<Vec<dstudio_core::Note>>> = Rc::new(RefCell::new(Vec::new()));
    let (rvn1, rvn2, rvn3, rvn4, rvn5) = (rvn.clone(), rvn.clone(), rvn.clone(), rvn.clone(), rvn.clone());
    let wz: Rc<RefCell<(f64, f64, f32, f32)>> = Rc::new(RefCell::new((0.0, 0.0, 0.0, 0.0)));
    let (wz1, wz2, wz3, wz4, wz5, wz6, wz7, wz8) =
        (wz.clone(), wz.clone(), wz.clone(), wz.clone(), wz.clone(), wz.clone(), wz.clone(), wz.clone());
    let wzc: Rc<Cell<(f32, f32)>> = Rc::new(Cell::new((0.0, 0.0)));
    let (wz9, wz10) = (wzc.clone(), wzc.clone());
    let (hp3, hp4, hp5) = (hp.clone(), hp.clone(), hp.clone());
    let zoom_mem: Rc<Cell<(f32, i32, f64)>> = Rc::new(Cell::new((0.0, 0, 0.0)));
    let (zm1, zm2, zm3, zm4, zm5, zm6) =
        (zoom_mem.clone(), zoom_mem.clone(), zoom_mem.clone(), zoom_mem.clone(), zoom_mem.clone(), zoom_mem.clone());
    vec![
        DoC(
            "snap: pin 1/16 for the piano-roll sections (their notes snap to the toolbar size)",
            Box::new(|_, ctl| ctl.borrow_mut().snap.grid = Some(crate::timeline::GridSize::Sixteenth)),
        ),
        Do("Enter (playhead to 0)", Box::new(|a| key(a, Key::Return, false, false))),
        DoC(
            "wheel back to the start of the timeline",
            Box::new(|a, _| {
                let w = a.window();
                dispatch(w, WindowEvent::PointerMoved { position: lanes_point() });
                dispatch(w, WindowEvent::PointerScrolled { position: lanes_point(), delta_x: 4000.0, delta_y: 0.0 });
            }),
        ),
        DoC(
            "click the Instrument track header",
            Box::new(|a, ctl| {
                let sy = ctl.borrow().view.scroll_y as f32;
                click(a, tracks_x() + 37.35, track0_y() + PITCH + 10.0 - sy);
            }),
        ),
        Check("record history", record(h0.clone())),
        Do("Cmd+Shift+M (clip.insert_midi)", Box::new(|a| key(a, "M", true, true))),
        Check(
            "1-bar MIDI clip at the playhead, selected and open in the piano roll, one undo step",
            Box::new(move |c, a| {
                let m = instrument_clip(c).ok_or("no MIDI clip")?;
                let s = a.global::<Studio>();
                one_step(h0.clone(), "Insert MIDI Clip")(c)?;
                (m.start == 0
                    && m.length == 4 * PPQ
                    && m.name == "MIDI"
                    && s.get_bottom_tab() == 1
                    && s.get_pr_visible()
                    && c.selected_clip == Some(m.id))
                .then_some(())
                .ok_or(format!("start {} len {} tab {} pr {}", m.start, m.length, s.get_bottom_tab(), s.get_pr_visible()))
            }),
        ),
        DoC(
            "click the MIDI clip in the lane",
            Box::new(move |a, ctl| {
                let (x, y) = lane_xy(ctl, 2.0, 1);
                click(a, x, y);
            }),
        ),
        Check(
            "clip selected, piano roll open",
            Box::new(|c, a| {
                let id = instrument_clip(c).map(|m| m.id);
                (c.selected_clip == id && a.global::<Studio>().get_pr_visible() && c.pr_visible())
                    .then_some(())
                    .ok_or(format!("sel {:?} vs {id:?}", c.selected_clip))
            }),
        ),
        // The default view shows about ten rows below C4; put B3 (59) on the top row so the
        // notes drawn below (D3..B3) are all on screen.
        DoC("scroll the piano roll to B3 at the top", Box::new(|_, ctl| ctl.borrow_mut().pr.scroll = 68.0 * crate::pianoroll::ROW_H)),
        DoC(
            "click empty grid (focus) and press B (pen)",
            Box::new(|a, ctl| {
                let (x, y) = pr_xy(ctl, 3.5, 50);
                click(a, x, y);
                key(a, "b", false, false);
            }),
        ),
        Check(
            "pen tool on (midi.paint_pen), record history",
            Box::new(move |c, a| {
                h1.set(c.doc.history_labels().len());
                (c.pr.pen && c.pr.focused && a.global::<Studio>().get_pr_pen()).then_some(()).ok_or("pen off".into())
            }),
        ),
        DoC(
            "draw 3 notes with the pen (E3 at 1.1, G3 at 1.2, B3 at 1.3)",
            Box::new(|a, ctl| {
                for (beat, pitch) in [(0.1, 52), (1.1, 55), (2.1, 59)] {
                    let (x, y) = pr_xy(ctl, beat, pitch);
                    click(a, x, y);
                }
            }),
        ),
        Check(
            "3 notes: pitches 52/55/59, starts 0/960/1920, length 1/16; 3 undo steps",
            Box::new(move |c, _| {
                let n = notes_of(c);
                let got: Vec<(u8, i64, i64)> = n.iter().map(|n| (n.pitch, n.start, n.length)).collect();
                let steps = c.doc.history_labels().len() - h2.get();
                h2.set(c.doc.history_labels().len());
                (got == vec![(52, 0, 240), (55, 960, 240), (59, 1920, 240)] && steps == 3)
                    .then_some(())
                    .ok_or(format!("{got:?}, {steps} steps"))
            }),
        ),
        Check(
            "drawn notes sound: instrument track meter > 0 in an offline render from the clip start",
            Box::new(|c, _| {
                if c.engine.device_running() {
                    return Ok(()); // offline render is refused while the device owns the audio path
                }
                let p = c.project();
                let t = p.tracks.iter().find(|t| !t.midi_clips.is_empty()).ok_or("no MIDI clip")?;
                let start = p.ticks_to_frames(t.midi_clips[0].start, c.engine.sample_rate());
                let e = &c.engine;
                e.seek(start).map_err(|e| e.to_string())?;
                e.play().map_err(|e| e.to_string())?;
                let mut buf = vec![0f32; 2 * 4_800]; // first 100 ms: inside the first note (E3, 1/16 = 125 ms)
                let r = e.render_offline(&mut buf);
                let mut meters = Vec::new();
                e.peek_meters(&mut meters, 64);
                let _ = e.stop();
                r.map_err(|e| e.to_string())?;
                let m = meters.iter().find(|m| m.track_id == t.id.0).map(|m| m.peak_l.max(m.peak_r)).unwrap_or(0.0);
                (m > 0.01).then_some(()).ok_or(format!("instrument meter {m}"))
            }),
        ),
        DoC(
            "drag the G3 note up 2 semitones",
            Box::new(|a, ctl| {
                let (x, y) = pr_xy(ctl, 1.1, 55);
                drag(a, x, y, x, y - 2.0 * crate::pianoroll::ROW_H);
            }),
        ),
        Check(
            "note moved to A3 (57), start kept, one undo step",
            Box::new(move |c, _| {
                let n = notes_of(c);
                one_step(h3.clone(), "Edit Notes")(c)?;
                n.iter().any(|n| n.pitch == 57 && n.start == 960).then_some(()).ok_or(format!("{n:?}"))
            }),
        ),
        DoC(
            "drag the B3 note's right edge to beat 3",
            Box::new(|a, ctl| {
                let (x0, y) = pr_xy(ctl, 2.25, 59);
                let (x1, _) = pr_xy(ctl, 3.0, 59);
                drag(a, x0 - 1.5, y, x1 - 1.5, y);
            }),
        ),
        Check(
            "B3 note resized to one beat (960 ticks), one undo step",
            Box::new(move |c, _| {
                let n = notes_of(c);
                one_step(h4.clone(), "Edit Notes")(c)?;
                n.iter().any(|n| n.pitch == 59 && n.start == 1920 && n.length == 960).then_some(()).ok_or(format!("{n:?}"))
            }),
        ),
        DoC(
            "snap 1/32, draw an off-grid note at beat 3.375, snap back to 1/16",
            Box::new(|a, ctl| {
                // Notes snap to the toolbar's snap size; the footer Grid is the Quantize grid.
                ctl.borrow_mut().snap.grid = Some(crate::timeline::GridSize::ThirtySecond);
                let (x, y) = pr_xy(ctl, 3.375 + 0.03, 52);
                click(a, x, y);
                ctl.borrow_mut().snap.grid = Some(crate::timeline::GridSize::Sixteenth);
            }),
        ),
        Check(
            "off-grid note drawn at 3240 ticks, footer Grid still 1/16",
            Box::new(move |c, a| {
                let n = notes_of(c);
                h13.set(c.doc.history_labels().len());
                (n.iter().any(|n| n.start == 3240 && n.pitch == 52) && c.pr.grid == 16 && a.global::<Studio>().get_pr_grid_text() == "1/16")
                    .then_some(())
                    .ok_or(format!("grid {} {n:?}", c.pr.grid))
            }),
        ),
        Do("click Quantize (selection = the new note)", Box::new(|a| click(a, pr_origin().0 + 7.0 + 15.0, bottom_y() + 41.0))),
        DoC(
            "Grid menu: pick 1/8T",
            Box::new(|a, _| {
                let (gx, gy) = (pr_origin().0 + 387.0 + 8.0, bottom_end() - 54.5 + 30.0 + 7.0);
                click(a, gx, gy);
                click(a, gx + 22.0, gy - 10.2 - (8.0 * 20.4 + 6.8 + 10.2) + 3.4 + 5.0 * 20.4 + 10.2 + 10.2);
            }),
        ),
        Check(
            "Grid 1/8T (1/12 note), menu closed",
            Box::new(|c, a| {
                let s = a.global::<Studio>();
                (c.pr.grid == 12 && s.get_pr_grid_text() == "1/8T" && s.get_clip_menu() == -1).then_some(()).ok_or(format!(
                    "grid {} {:?} menu {}",
                    c.pr.grid,
                    s.get_pr_grid_text(),
                    s.get_clip_menu()
                ))
            }),
        ),
        DoC(
            "Grid back to 1/16 (menu pick)",
            Box::new(|a, ctl| {
                ctl.borrow_mut().clip_menu_open(3);
                a.global::<Studio>().invoke_clip_menu_pick(2);
            }),
        ),
        Check(
            "Quantize snapped it to the 1/16 grid (3360), one undo step",
            Box::new(move |c, _| {
                let n = notes_of(c);
                one_step(h5.clone(), "Quantize Notes")(c)?;
                (n.iter().all(|n| n.start % 240 == 0) && n.iter().any(|n| n.start == 3360 && n.pitch == 52))
                    .then_some(())
                    .ok_or(format!("{n:?}"))
            }),
        ),
        Do("click Smart Quantize", Box::new(|a| click(a, pr_origin().0 + 55.0 + 30.0, bottom_y() + 41.0))),
        Check(
            "Smart Quantize ran: one undo step, or no change to notes already on the grid",
            Box::new(|c, _| {
                let label = c.doc.undo_label();
                label
                    .is_some_and(|l| l.starts_with("Smart Quantize") || l.starts_with("Quantize Notes"))
                    .then_some(())
                    .ok_or(format!("last {label:?}"))
            }),
        ),
        DoC(
            "undo Smart Quantize if it changed the notes",
            Box::new(|a, ctl| {
                let stepped = ctl.borrow().doc.undo_label().is_some_and(|l| l.starts_with("Smart Quantize"));
                if stepped {
                    key(a, "z", true, false);
                }
            }),
        ),
        DoC(
            "preview: press the G3 key on the piano roll keyboard",
            Box::new(move |a, ctl| {
                hp0.set(ctl.borrow().doc.history_labels().len());
                let (_, y) = pr_xy(ctl, 0.0, 55);
                press(a, pr_origin().0 + 20.0, y);
            }),
        ),
        Check(
            "preview: G3 sounds through the track's instrument (one held preview note), no edit",
            Box::new(move |c, _| {
                let dev = track_of(c, TrackKind::Instrument)
                    .and_then(|t| c.project().tracks[t].instrument_plugin.as_ref())
                    .map(|d| d.id.0)
                    .ok_or("no instrument")?;
                let held = c.engine.preview().notes;
                (c.pr.preview == Some((dev, 55)) && held == 1 && c.doc.history_labels().len() == hp1.get()).then_some(()).ok_or(format!(
                    "preview {:?} held {held} history {} -> {}",
                    c.pr.preview,
                    hp1.get(),
                    c.doc.history_labels().len()
                ))
            }),
        ),
        DoC(
            "preview: slide up to A3 with the button held",
            Box::new(|a, ctl| {
                let (_, y) = pr_xy(ctl, 0.0, 57);
                dispatch(a.window(), WindowEvent::PointerMoved { position: LogicalPosition::new(pr_origin().0 + 20.0, y) });
            }),
        ),
        Check(
            "preview: A3 replaced G3 (still one note held)",
            Box::new(|c, _| {
                let held = c.engine.preview().notes;
                (c.pr.preview.map(|p| p.1) == Some(57) && held == 1).then_some(()).ok_or(format!("preview {:?} held {held}", c.pr.preview))
            }),
        ),
        DoC(
            "preview: release the key",
            Box::new(|a, ctl| {
                let (_, y) = pr_xy(ctl, 0.0, 57);
                release(a, pr_origin().0 + 20.0, y);
            }),
        ),
        Check(
            "preview: released, no note held, the clip unchanged",
            Box::new(move |c, _| {
                let held = c.engine.preview().notes;
                (c.pr.preview.is_none() && held == 0 && c.doc.history_labels().len() == hp2.get())
                    .then_some(())
                    .ok_or(format!("preview {:?} held {held}", c.pr.preview))
            }),
        ),
        DoC(
            "zoom: remember the scroll and the pitch in the middle of the view",
            Box::new(move |_, ctl| {
                let c = ctl.borrow();
                let v = c.pr_view();
                zm1.set((c.pr.scroll, v.pitch_at(crate::pianoroll::GRID_Y + (v.h - crate::pianoroll::GRID_Y) / 2.0), v.start));
            }),
        ),
        Check(
            "zoom: horizontal slider at fit (0), vertical at the default row height, snap readout 1/16",
            Box::new(|c, a| {
                let s = a.global::<Studio>();
                let v = c.pr_view();
                let (lo, hi) = crate::pianoroll::row_h_range(&v);
                let want = (crate::pianoroll::ROW_H - lo) / (hi - lo);
                (s.get_pr_hzoom() == 0.0 && approx(s.get_pr_vzoom(), want, 1e-3) && s.get_snap_text() == "1/16")
                    .then_some(())
                    .ok_or(format!("h {} v {} (want {want}) snap {}", s.get_pr_hzoom(), s.get_pr_vzoom(), s.get_snap_text()))
            }),
        ),
        DoC(
            "zoom: drag the horizontal slider from its left end to the middle",
            Box::new(move |a, ctl| {
                let (x, w, y) = zoom_track(ctl, 1);
                drag(a, x + 1.0, y, x + w / 2.0, y);
            }),
        ),
        Check(
            "zoom: horizontal at 0.5 (points per beat halfway between fit and 1 ref px per tick on a log scale), left edge kept",
            Box::new(move |c, a| {
                let v = c.pr_view();
                let len = instrument_clip(c).map_or(0, |m| m.length) as f64 / dstudio_core::PPQ as f64;
                let fit = crate::pianoroll::PrView::fit(c.pr.size, len, 0.0).ppb;
                let want = (fit * dstudio_core::PPQ as f64 * crate::timeline::REF).sqrt();
                let ok = approx(a.global::<Studio>().get_pr_hzoom(), 0.5, 1e-3)
                    && ((v.ppb - want) / want).abs() < 1e-3
                    && (v.start - zm2.get().2).abs() < 1e-6;
                ok.then_some(()).ok_or(format!("hzoom {} ppb {} want {want} start {}", c.pr.hzoom, v.ppb, v.start))
            }),
        ),
        DoC(
            "zoom: trackpad scroll 100 pt to the right",
            Box::new(|a, _| {
                let w = a.window();
                let p = LogicalPosition::new(pr_origin().0 + 400.0, pr_origin().1 + 60.0);
                dispatch(w, WindowEvent::PointerMoved { position: p });
                dispatch(w, WindowEvent::PointerScrolled { position: p, delta_x: -100.0, delta_y: 0.0 });
            }),
        ),
        Check(
            "zoom: the zoomed piano roll panned 100 pt later in the clip",
            Box::new(move |c, _| {
                let v = c.pr_view();
                let d = (v.start - zm3.get().2) * v.ppb;
                ((d - 100.0).abs() < 0.5).then_some(()).ok_or(format!("moved {d} pt"))
            }),
        ),
        DoC(
            "zoom: click the vertical slider near its right end",
            Box::new(move |a, ctl| {
                let (x, w, y) = zoom_track(ctl, 0);
                click(a, x + w - 1.0, y);
            }),
        ),
        Check(
            "zoom: rows near 32 ref px tall, the middle pitch kept",
            Box::new(move |c, _| {
                let v = c.pr_view();
                let mid = v.pitch_at(crate::pianoroll::GRID_Y + (v.h - crate::pianoroll::GRID_Y) / 2.0);
                let ok = v.row_h > 26.0 && v.row_h <= 32.0 * 0.85 + 1e-3 && mid == zm4.get().1;
                ok.then_some(()).ok_or(format!("row {} mid {mid} want {}", v.row_h, zm4.get().1))
            }),
        ),
        DoC(
            "snap: click the piano roll's snap readout",
            Box::new(|a, ctl| {
                let right = pr_origin().0 + ctl.borrow().pr.size.0 + 0.5;
                click(a, right - 36.0, bottom_y() + 41.0);
            }),
        ),
        Check(
            "snap: off, the readout says Off",
            Box::new(|c, a| {
                let t = a.global::<Studio>().get_snap_text();
                (!c.snap.on && t == "Off").then_some(()).ok_or(format!("on {} text {t}", c.snap.on))
            }),
        ),
        DoC(
            "snap: click it again, then the chevron",
            Box::new(|a, ctl| {
                let right = pr_origin().0 + ctl.borrow().pr.size.0 + 0.5;
                click(a, right - 36.0, bottom_y() + 41.0);
                click(a, right - 17.0, bottom_y() + 41.0);
            }),
        ),
        Check(
            "snap: on again, the shared snap menu open under the chevron",
            Box::new(|c, a| {
                let s = a.global::<Studio>();
                (c.snap.on && s.get_snap_menu_open()).then_some(()).ok_or(format!("on {} open {}", c.snap.on, s.get_snap_menu_open()))
            }),
        ),
        DoC(
            "snap: pick 1/8 in the menu",
            Box::new(|a, ctl| {
                let (x, y, row) = {
                    let c = ctl.borrow();
                    let (x, y) = c.snap.menu.unwrap_or_default();
                    (x, y, c.snap.rows().0[5].clone())
                };
                click(a, x - crate::snap::MENU_W / 2.0, y + 5.1 + row.y + row.h / 2.0);
            }),
        ),
        Check(
            "snap: 1/8 chosen for the piano roll and the timeline alike, menu closed",
            Box::new(|c, a| {
                let s = a.global::<Studio>();
                (c.snap.grid == Some(crate::timeline::GridSize::Eighth) && s.get_snap_text() == "1/8" && !s.get_snap_menu_open())
                    .then_some(())
                    .ok_or(format!("grid {:?} text {} open {}", c.snap.grid, s.get_snap_text(), s.get_snap_menu_open()))
            }),
        ),
        DoC(
            "zoom: back to fit, default rows and 1/16 snap",
            Box::new(move |_, ctl| {
                let mut c = ctl.borrow_mut();
                (c.pr.hzoom, c.pr.vzoom, c.pr.scroll) = (0.0, None, zm5.get().0);
                c.snap.grid = Some(crate::timeline::GridSize::Sixteenth);
                c.ui_dirty = true;
            }),
        ),
        // ---- Reverse (toolbar and R), wheel / pinch zoom, musical typing ----
        DoC(
            "reverse: pen off, click empty grid, select the A3 and B3 notes, remember the notes",
            Box::new(move |a, ctl| {
                ctl.borrow_mut().pr.pen = false;
                let (x, y) = pr_xy(ctl, 2.9, 50);
                click(a, x, y);
                let mut c = ctl.borrow_mut();
                let notes = notes_of(&c);
                c.pr.selected = notes.iter().filter(|n| n.start == 960 || n.start == 1920).map(|n| n.id).collect();
                rv1.set(c.doc.history_labels().len());
                rv2.set(c.doc.history_labels().len());
                *rvn1.borrow_mut() = notes;
            }),
        ),
        Do("R (Reverse)", Box::new(|a| key(a, "r", false, false))),
        Check(
            "R mirrored only the selected notes about their own extent, selection kept, one undo step, nothing recorded",
            Box::new(move |c, _| {
                let before = rvn2.borrow().clone();
                let sel = |n: &dstudio_core::Note| n.start == 960 || n.start == 1920;
                let lo = before.iter().filter(|n| sel(n)).map(|n| n.start).min().unwrap_or(0);
                let hi = before.iter().filter(|n| sel(n)).map(|n| n.start + n.length).max().unwrap_or(0);
                let want: Vec<dstudio_core::Note> = before
                    .iter()
                    .map(|n| if sel(n) { dstudio_core::Note { start: lo + hi - n.start - n.length, ..*n } } else { *n })
                    .collect();
                let mut got = notes_of(c);
                got.sort_by_key(|n| n.id);
                one_step(rv4.clone(), "Reverse Notes")(c)?;
                (got == want && c.pr.selected.len() == 2 && !c.engine.transport().playing)
                    .then_some(())
                    .ok_or(format!("{got:?} want {want:?}"))
            }),
        ),
        Do("Cmd+Z", Box::new(|a| key(a, "z", true, false))),
        Check("undo restored the notes", Box::new(move |c, _| (notes_of(c) == *rvn3.borrow()).then_some(()).ok_or("notes differ".into()))),
        DoC(
            "reverse: deselect, press R again",
            Box::new(|a, ctl| {
                ctl.borrow_mut().pr.selected.clear();
                key(a, "r", false, false);
            }),
        ),
        Check(
            "with nothing selected R mirrored every note about the whole extent",
            Box::new(move |c, _| {
                let before = rvn4.borrow().clone();
                let lo = before.iter().map(|n| n.start).min().unwrap_or(0);
                let hi = before.iter().map(|n| n.start + n.length).max().unwrap_or(0);
                let want: Vec<dstudio_core::Note> =
                    before.iter().map(|n| dstudio_core::Note { start: lo + hi - n.start - n.length, ..*n }).collect();
                let mut got = notes_of(c);
                got.sort_by_key(|n| n.id);
                (got == want && c.doc.undo_label() == Some("Reverse Notes")).then_some(()).ok_or(format!("{got:?} want {want:?}"))
            }),
        ),
        Do("Cmd+Z", Box::new(|a| key(a, "z", true, false))),
        Check(
            "undone: notes restored, history back to where it started",
            Box::new(move |c, _| {
                (notes_of(c) == *rvn5.borrow() && c.doc.history_labels().len() == rv3.get()).then_some(()).ok_or("not restored".into())
            }),
        ),
        DoC(
            "zoom gestures: back to fit, Alt + Shift + wheel over the middle of the grid",
            Box::new(move |a, ctl| {
                let (x, y) = {
                    let mut c = ctl.borrow_mut();
                    (c.pr.hzoom, c.pr.vzoom) = (0.0, None);
                    let v = c.pr_view();
                    *wz1.borrow_mut() = (v.ppb, v.beat_at(v.gx + (v.w - v.gx) / 2.0), v.row_h, v.scroll);
                    (pr_origin().0 + v.gx + (v.w - v.gx) / 2.0, pr_origin().1 + v.h / 2.0)
                };
                let w = a.window();
                dispatch(w, WindowEvent::PointerMoved { position: LogicalPosition::new(x, y) });
                dispatch(w, WindowEvent::KeyPressed { text: Key::Alt.into() });
                dispatch(w, WindowEvent::KeyPressed { text: Key::Shift.into() });
                dispatch(w, WindowEvent::PointerScrolled { position: LogicalPosition::new(x, y), delta_x: 0.0, delta_y: -300.0 });
                dispatch(w, WindowEvent::KeyReleased { text: Key::Shift.into() });
                dispatch(w, WindowEvent::KeyReleased { text: Key::Alt.into() });
            }),
        ),
        Check(
            "Alt+Shift+wheel zoomed time by 1 + 300 * 0.001, the beat under the pointer stayed, rows unchanged",
            Box::new(move |c, _| {
                let (ppb, at, row_h, _) = *wz2.borrow();
                let v = c.pr_view();
                let x = v.gx + (v.w - v.gx) / 2.0;
                let ok = ((v.ppb / ppb) - 1.3).abs() < 1e-3
                    && (v.beat_at(x) - at).abs() < 1e-6
                    && c.pr.hzoom > 0.0
                    && approx(v.row_h, row_h, 1e-4);
                ok.then_some(()).ok_or(format!("ppb {} -> {} (x1.3), beat {at} -> {}, hzoom {}", ppb, v.ppb, v.beat_at(x), c.pr.hzoom))
            }),
        ),
        DoC(
            "zoom gestures: Alt + wheel over the middle row",
            Box::new(move |a, ctl| {
                let (x, y) = {
                    let c = ctl.borrow();
                    let v = c.pr_view();
                    let mid = (v.h - crate::pianoroll::GRID_Y) / 2.0;
                    *wz3.borrow_mut() = (v.ppb, ((v.scroll + mid) / v.row_h) as f64, v.row_h, v.scroll);
                    (pr_origin().0 + v.gx + 200.0, pr_origin().1 + crate::pianoroll::GRID_Y + mid)
                };
                let w = a.window();
                dispatch(w, WindowEvent::PointerMoved { position: LogicalPosition::new(x, y) });
                dispatch(w, WindowEvent::KeyPressed { text: Key::Alt.into() });
                dispatch(w, WindowEvent::PointerScrolled { position: LogicalPosition::new(x, y), delta_x: 0.0, delta_y: -200.0 });
                dispatch(w, WindowEvent::KeyReleased { text: Key::Alt.into() });
            }),
        ),
        Check(
            "Alt+wheel grew the rows by 1 + 200 * 0.001, the pitch row under the pointer stayed, time unchanged",
            Box::new(move |c, _| {
                let (ppb, row_at, row_h, _) = *wz4.borrow();
                let v = c.pr_view();
                let mid = (v.h - crate::pianoroll::GRID_Y) / 2.0;
                let ok = ((v.row_h / row_h) - 1.2).abs() < 1e-3
                    && (((v.scroll + mid) / v.row_h) as f64 - row_at).abs() < 1e-3
                    && ((v.ppb - ppb) / ppb).abs() < 1e-9;
                ok.then_some(()).ok_or(format!(
                    "row_h {row_h} -> {}, row at {row_at} -> {}, ppb {ppb} -> {}",
                    v.row_h,
                    (v.scroll + mid) / v.row_h,
                    v.ppb
                ))
            }),
        ),
        DoC(
            "zoom gestures: Ctrl + wheel (a trackpad pinch) out by far, then in a little",
            Box::new(move |a, ctl| {
                let (x, y) = {
                    let c = ctl.borrow();
                    let v = c.pr_view();
                    *wz5.borrow_mut() = (v.ppb, 0.0, v.row_h, v.scroll);
                    (pr_origin().0 + v.gx + 200.0, pr_origin().1 + v.h / 2.0)
                };
                let w = a.window();
                dispatch(w, WindowEvent::PointerMoved { position: LogicalPosition::new(x, y) });
                dispatch(w, WindowEvent::KeyPressed { text: Key::Control.into() });
                dispatch(w, WindowEvent::PointerScrolled { position: LogicalPosition::new(x, y), delta_x: 0.0, delta_y: -1000.0 });
                dispatch(w, WindowEvent::KeyReleased { text: Key::Control.into() });
            }),
        ),
        Check(
            "one big pinch out is capped at x0.1 and the zoom stops at the fit",
            Box::new(move |c, _| {
                let (ppb, ..) = *wz6.borrow();
                let v = c.pr_view();
                let fit = crate::pianoroll::PrView::fit(c.pr.size, c.pr_clip().map_or(0, |m| m.length) as f64 / PPQ as f64, 0.0).ppb;
                ((v.ppb - (ppb * 0.1).max(fit)).abs() < 1e-6 && v.ppb >= fit - 1e-9)
                    .then_some(())
                    .ok_or(format!("ppb {ppb} -> {} fit {fit}", v.ppb))
            }),
        ),
        DoC(
            "zoom gestures: pinch in by 20 %, then the zoom commands",
            Box::new(move |_, ctl| {
                let mut c = ctl.borrow_mut();
                let v = c.pr_view();
                *wz7.borrow_mut() = (v.ppb, v.beat_at(300.0), v.row_h, v.scroll);
                c.pr_zoom_at(1.2, 1.0, 300.0, 100.0);
            }),
        ),
        Check(
            "pinch zoomed time by 1.2 around x = 300",
            Box::new(move |c, _| {
                let (ppb, at, ..) = *wz8.borrow();
                let v = c.pr_view();
                (((v.ppb / ppb) - 1.2).abs() < 1e-3 && (v.beat_at(300.0) - at).abs() < 1e-6)
                    .then_some(())
                    .ok_or(format!("ppb {ppb} -> {}", v.ppb))
            }),
        ),
        DoC(
            "zoom commands: horizontal out 0.1 from slider 0.7, vertical in 0.1 from 0.3, then 3 x out at 0.0",
            Box::new(move |_, ctl| {
                let mut c = ctl.borrow_mut();
                c.pr_zoom(1, 0.7);
                c.pr_zoom(0, 0.3);
                c.pr_zoom_step(false, 1);
                c.pr_zoom_step(true, 0);
                wz9.set(c.pr_zoom_values());
                c.pr_zoom(1, 0.0);
                for _ in 0..3 {
                    c.pr_zoom_step(false, 1);
                }
            }),
        ),
        Check(
            "each command moved its slider by exactly 0.1, and zooming out at the fit stays at 0 (clamped)",
            Box::new(move |c, _| {
                let (vz, hz) = wz10.get();
                let (_, h0) = c.pr_zoom_values();
                ((vz - 0.4).abs() < 1e-3 && (hz - 0.6).abs() < 1e-3 && h0 == 0.0).then_some(()).ok_or(format!("v {vz} h {hz} then {h0}"))
            }),
        ),
        DoC(
            "zoom gestures: back to fit and the default rows",
            Box::new(move |_, ctl| {
                let mut c = ctl.borrow_mut();
                (c.pr.hzoom, c.pr.vzoom, c.pr.scroll) = (0.0, None, zm6.get().0);
                c.ui_dirty = true;
            }),
        ),
        DoC(
            "typing: click the grid (focus), press K",
            Box::new(move |a, ctl| {
                let (x, y) = pr_xy(ctl, 2.9, 50);
                click(a, x, y);
                hp3.set(ctl.borrow().doc.history_labels().len());
                key(a, "k", false, false);
            }),
        ),
        Check(
            "K turned musical typing on, the header button shows it",
            Box::new(|c, a| (c.typing.active && a.global::<Studio>().get_typing_on()).then_some(()).ok_or("typing off".into())),
        ),
        Do(
            "hold A and D (no release yet), then an auto-repeat of A",
            Box::new(|a| {
                let w = a.window();
                dispatch(w, WindowEvent::KeyPressed { text: "a".into() });
                dispatch(w, WindowEvent::KeyPressed { text: "d".into() });
                dispatch(w, WindowEvent::KeyPressRepeated { text: "a".into() });
            }),
        ),
        Check(
            "two notes sound (C3 = 48 and E3 = 52, one preview each), both keys lit, the repeat did not sound again, no edit",
            Box::new(move |c, _| {
                let held = c.engine.preview().notes;
                let lit = c.typing.lit();
                (held == 2 && lit == (1u128 << 48 | 1u128 << 52) && c.doc.history_labels().len() == hp4.get())
                    .then_some(())
                    .ok_or(format!("held {held} lit {lit:#x}"))
            }),
        ),
        Check(
            "the piano roll scene draws both keys pressed",
            Box::new(|c, _| {
                let notes = c.pr_notes();
                let lit = c.pr_scene(1.0, &notes).map_or(0, |s| s.lit);
                (lit == (1u128 << 48 | 1u128 << 52)).then_some(()).ok_or(format!("lit {lit:#x}"))
            }),
        ),
        Do(
            "release A, press X (octave up) and A again",
            Box::new(|a| {
                let w = a.window();
                dispatch(w, WindowEvent::KeyReleased { text: "a".into() });
                dispatch(w, WindowEvent::KeyPressed { text: "x".into() });
                dispatch(w, WindowEvent::KeyReleased { text: "x".into() });
                dispatch(w, WindowEvent::KeyPressed { text: "a".into() });
            }),
        ),
        Check(
            "C4 (60) and E3 (52) sound: the release let go of C3, X moved the octave up",
            Box::new(|c, _| {
                let held = c.engine.preview().notes;
                (held == 2 && c.typing.lit() == (1u128 << 60 | 1u128 << 52) && c.typing.octave == 4).then_some(()).ok_or(format!(
                    "held {held} lit {:#x} octave {}",
                    c.typing.lit(),
                    c.typing.octave
                ))
            }),
        ),
        Do(
            "release A and D, press C then V",
            Box::new(|a| {
                let w = a.window();
                for k in ["a", "d"] {
                    dispatch(w, WindowEvent::KeyReleased { text: k.into() });
                }
                dispatch(w, WindowEvent::KeyPressed { text: "c".into() });
                dispatch(w, WindowEvent::KeyReleased { text: "c".into() });
            }),
        ),
        Check(
            "nothing sounds, C played softer by 10/127 (default 0.7)",
            Box::new(|c, _| {
                let held = c.engine.preview().notes;
                (held == 0 && c.typing.lit() == 0 && (c.typing.velocity - (0.7 - 10.0 / 127.0)).abs() < 1e-4)
                    .then_some(())
                    .ok_or(format!("held {held} velocity {}", c.typing.velocity))
            }),
        ),
        Do(
            "V (loud), Z (octave down), then K: a note, not the toggle",
            Box::new(|a| {
                let w = a.window();
                for k in ["v", "z"] {
                    dispatch(w, WindowEvent::KeyPressed { text: k.into() });
                    dispatch(w, WindowEvent::KeyReleased { text: k.into() });
                }
                dispatch(w, WindowEvent::KeyPressed { text: "k".into() });
            }),
        ),
        Check(
            "velocity back to 0.7, octave 3, typing still on and K sounds C4 (60) while held",
            Box::new(|c, _| {
                let ok = c.typing.active
                    && c.typing.octave == 3
                    && (c.typing.velocity - 0.7).abs() < 1e-4
                    && c.typing.lit() == 1u128 << 60
                    && c.engine.preview().notes == 1;
                ok.then_some(()).ok_or(format!(
                    "active {} octave {} velocity {} lit {:#x}",
                    c.typing.active,
                    c.typing.octave,
                    c.typing.velocity,
                    c.typing.lit()
                ))
            }),
        ),
        DoC(
            "typing: release K, click the header button (off), press A",
            Box::new(|a, _| {
                let w = a.window();
                dispatch(w, WindowEvent::KeyReleased { text: "k".into() });
                click(a, win().0 - 63.0 + 12.5, 51.5);
                dispatch(w, WindowEvent::KeyPressed { text: "a".into() });
                dispatch(w, WindowEvent::KeyReleased { text: "a".into() });
            }),
        ),
        Check(
            "typing off: the header button is off and A sounds nothing",
            Box::new(move |c, a| {
                let ok = !c.typing.active && !a.global::<Studio>().get_typing_on() && c.engine.preview().notes == 0 && c.typing.lit() == 0;
                (ok && c.doc.history_labels().len() == hp5.get()).then_some(()).ok_or(format!(
                    "active {} held {}",
                    c.typing.active,
                    c.engine.preview().notes
                ))
            }),
        ),
        DoC("pen back on for the sections after", Box::new(|_, ctl| ctl.borrow_mut().pr.pen = true)),
        DoC(
            "click the E3 note at beat 0, press Up",
            Box::new(|a, ctl| {
                let (x, y) = pr_xy(ctl, 0.1, 52);
                click(a, x, y);
                key(a, Key::UpArrow, false, false);
            }),
        ),
        Check(
            "↑ transposed only the selected note (52 -> 53), one undo step",
            Box::new(move |c, _| {
                let n = notes_of(c);
                one_step(h6.clone(), "Transpose Notes")(c)?;
                let first = n.iter().find(|n| n.start == 0).ok_or("no note at 0")?;
                (first.pitch == 53 && n.iter().filter(|n| n.pitch == 52).count() == 1).then_some(()).ok_or(format!("{n:?}"))
            }),
        ),
        DoC(
            "Shift+Up (octave), then B (pen off) and click empty grid (deselect)",
            Box::new(|a, ctl| {
                key(a, Key::UpArrow, false, true);
                key(a, "b", false, false);
                let (x, y) = pr_xy(ctl, 2.9, 50);
                click(a, x, y);
            }),
        ),
        Check(
            "⇧↑ moved it an octave (65), pen off, nothing selected; record notes",
            Box::new(move |c, _| {
                let n = notes_of(c);
                *sv1.borrow_mut() = (n.clone(), instrument_clip(c).map(|m| m.length).unwrap_or(0));
                h14.set(c.doc.history_labels().len());
                (n.iter().any(|n| n.start == 0 && n.pitch == 65) && !c.pr.pen && c.pr.selected.is_empty())
                    .then_some(())
                    .ok_or(format!("{n:?}"))
            }),
        ),
        DoC(
            "Scale: click the root note in the footer, pick Eb",
            Box::new(|a, _| {
                click(a, pr_origin().0 + 720.0 + 3.0, bottom_end() - 54.5 + 30.0 + 7.0);
                a.global::<Studio>().invoke_clip_menu_pick(5);
            }),
        ),
        Check(
            "the clip keeps Eb (scale type from the project key: Major), one undo step, footer reads Eb Major",
            Box::new(|c, a| {
                let s = a.global::<Studio>();
                let scale = instrument_clip(c).and_then(|m| m.scale).ok_or("no clip scale")?;
                let key = scale.key.ok_or("no key")?;
                (key.root == 5
                    && key.kind == 0
                    && !scale.highlight
                    && c.doc.undo_label() == Some("Clip Scale")
                    && s.get_pr_scale_root() == "Eb"
                    && s.get_pr_scale_type() == "Major")
                    .then_some(())
                    .ok_or(format!("{scale:?} {:?} {} {}", c.doc.undo_label(), s.get_pr_scale_root(), s.get_pr_scale_type()))
            }),
        ),
        Do("click the footer Scale switch", Box::new(|a| click(a, pr_origin().0 + 751.0 + 10.0, bottom_end() - 54.5 + 13.0 + 7.0))),
        Check(
            "Scale highlight on, the Scale button shows above the keys, rows unchanged",
            Box::new(|c, a| {
                let on = instrument_clip(c).and_then(|m| m.scale).is_some_and(|s| s.highlight && !s.fold);
                (on && a.global::<Studio>().get_pr_scale_on() && !c.pr_view().rows.folded())
                    .then_some(())
                    .ok_or(format!("{:?}", instrument_clip(c).map(|m| m.scale)))
            }),
        ),
        Do("click Scale above the keys", Box::new(|a| click(a, pr_origin().0 + 6.5 + 19.0, bottom_y() + 27.0 + 27.5 + 30.0 + 8.0))),
        Check(
            "scale rows: only Eb Major pitches plus rows holding notes",
            Box::new(|c, _| {
                let m = instrument_clip(c).ok_or("clip gone")?;
                let v = c.pr_view();
                let key = m.scale.and_then(|s| s.key).ok_or("no key")?;
                let shown: Vec<i32> = (0..v.rows.len())
                    .map(|i| v.pitch_at(crate::pianoroll::GRID_Y + i as f32 * crate::pianoroll::ROW_H - v.scroll + 1.0))
                    .collect();
                let ok = m.scale.is_some_and(|s| s.fold)
                    && v.rows.folded()
                    && shown.iter().all(|&p| key.contains(p) || m.notes.iter().any(|n| n.pitch as i32 == p))
                    && m.notes.iter().all(|n| shown.contains(&(n.pitch as i32)));
                ok.then_some(()).ok_or(format!("{} rows", v.rows.len()))
            }),
        ),
        Do("click the footer Scale switch again", Box::new(|a| click(a, pr_origin().0 + 751.0 + 10.0, bottom_end() - 54.5 + 13.0 + 7.0))),
        Check(
            "Scale off restores chromatic rows and resets the Scale button",
            Box::new(|c, a| {
                let s = instrument_clip(c).and_then(|m| m.scale).unwrap_or_default();
                (!s.highlight && !s.fold && !c.pr_view().rows.folded() && !a.global::<Studio>().get_pr_scale_on())
                    .then_some(())
                    .ok_or(format!("{s:?}"))
            }),
        ),
        Do(
            "undo the four scale edits",
            Box::new(|a| {
                for _ in 0..4 {
                    key(a, "z", true, false);
                }
            }),
        ),
        Check(
            "undo cleared the clip's scale view",
            Box::new(|c, _| instrument_clip(c).is_some_and(|m| m.scale.is_none()).then_some(()).ok_or("scale still set".into())),
        ),
        Do("keyboard: Tab to the footer Loop switch", Box::new(|a| tab_to(a, 0))),
        Check(
            "Tab reached the Loop switch",
            Box::new(|_, a| {
                (a.global::<Studio>().get_footer_focus() == 0)
                    .then_some(())
                    .ok_or(format!("focus {}", a.global::<Studio>().get_footer_focus()))
            }),
        ),
        Do("Space", Box::new(|a| key(a, " ", false, false))),
        Check(
            "Space toggled Loop off (and played nothing)",
            Box::new(|c, _| {
                let l = instrument_clip(c).and_then(|m| m.clip_loop).ok_or("no loop")?;
                (!l.on && !c.engine.transport().playing && c.doc.undo_label() == Some("Clip Loop"))
                    .then_some(())
                    .ok_or(format!("{l:?} playing {}", c.engine.transport().playing))
            }),
        ),
        Do("Return", Box::new(|a| key(a, Key::Return, false, false))),
        Check(
            "Return toggled Loop back on",
            Box::new(|c, _| instrument_clip(c).and_then(|m| m.clip_loop).is_some_and(|l| l.on).then_some(()).ok_or("loop off".into())),
        ),
        Do("keyboard: Tab on to the Root note", Box::new(|a| tab_to(a, 2))),
        Do("Down", Box::new(|a| key(a, Key::DownArrow, false, false))),
        Check(
            "Down picked the next root (C#), one \"Clip Scale\" step",
            Box::new(|c, a| {
                let k = instrument_clip(c).and_then(|m| m.scale).and_then(|s| s.key).ok_or("no key")?;
                (k.root == 1 && a.global::<Studio>().get_pr_scale_root() == "C#" && c.doc.undo_label() == Some("Clip Scale"))
                    .then_some(())
                    .ok_or(format!("{k:?}"))
            }),
        ),
        Do("End", Box::new(|a| key(a, Key::End, false, false))),
        Check(
            "End picked the last root (B)",
            Box::new(|_, a| {
                (a.global::<Studio>().get_pr_scale_root() == "B").then_some(()).ok_or(a.global::<Studio>().get_pr_scale_root().to_string())
            }),
        ),
        Do(
            "Up, then Tab to the Scale type, End and Home",
            Box::new(|a| {
                key(a, Key::UpArrow, false, false);
                tab_to(a, 3);
                key(a, Key::End, false, false);
                key(a, Key::Home, false, false);
            }),
        ),
        Check(
            "Up picked Bb; End then Home came back to the first scale type (Major)",
            Box::new(|c, a| {
                let s = a.global::<Studio>();
                let k = instrument_clip(c).and_then(|m| m.scale).and_then(|s| s.key).ok_or("no key")?;
                (s.get_pr_scale_root() == "Bb" && k.kind == 0 && s.get_pr_scale_type() == "Major").then_some(()).ok_or(format!(
                    "{} {}",
                    s.get_pr_scale_root(),
                    s.get_pr_scale_type()
                ))
            }),
        ),
        Do("Return on the Scale type", Box::new(|a| key(a, Key::Return, false, false))),
        Check("Return opened the Scale type menu", Box::new(|c, _| (c.pr.menu == 9).then_some(()).ok_or(format!("menu {}", c.pr.menu)))),
        Do("Escape", Box::new(|a| key(a, Key::Escape, false, false))),
        Check(
            "Escape closed the menu and gave the keys back to the shortcuts",
            Box::new(|c, a| {
                (c.pr.menu == -1 && a.global::<Studio>().get_footer_focus() == -1).then_some(()).ok_or(format!(
                    "menu {} focus {}",
                    c.pr.menu,
                    a.global::<Studio>().get_footer_focus()
                ))
            }),
        ),
        DoC(
            "undo the keyboard edits (two Loop, five Scale)",
            Box::new(|_, ctl| {
                let mut c = ctl.borrow_mut();
                for _ in 0..7 {
                    c.undo();
                }
            }),
        ),
        Check(
            "keyboard edits undone",
            Box::new(|c, _| {
                let m = instrument_clip(c).ok_or("clip gone")?;
                (m.scale.is_none() && m.clip_loop.is_some_and(|l| l.on)).then_some(()).ok_or(format!("{:?} {:?}", m.scale, m.clip_loop))
            }),
        ),
        Check(
            "a new MIDI clip loops over the whole clip; footer Loop on",
            Box::new(|c, a| {
                let m = instrument_clip(c).ok_or("clip gone")?;
                let l = m.clip_loop.ok_or("no loop")?;
                (l.on && l.start == 0 && l.end == m.length && a.global::<Studio>().get_pr_loop_on())
                    .then_some(())
                    .ok_or(format!("{l:?} len {}", m.length))
            }),
        ),
        DoC(
            "drag the loop bar's end one beat left",
            Box::new(|a, ctl| {
                let end = ctl.borrow().pr_loop().map_or(0, |l| l.end) as f64 / PPQ as f64;
                let (x, _) = pr_xy(ctl, end, 60);
                let (x1, _) = pr_xy(ctl, end - 1.0, 60);
                let y = pr_origin().1 + (crate::pianoroll::BAR_Y.0 + crate::pianoroll::BAR_Y.1) / 2.0;
                drag(a, x - 1.0, y, x1 - 1.0, y);
            }),
        ),
        Check(
            "the loop ends a beat earlier, the clip keeps its length, one \"Clip Loop\" step",
            Box::new(|c, _| {
                let m = instrument_clip(c).ok_or("clip gone")?;
                let l = m.clip_loop.ok_or("no loop")?;
                (l.on && l.start == 0 && l.end == m.length - PPQ && c.doc.undo_label() == Some("Clip Loop")).then_some(()).ok_or(format!(
                    "{l:?} len {} {:?}",
                    m.length,
                    c.doc.undo_label()
                ))
            }),
        ),
        Do("click the footer Loop switch (off)", Box::new(|a| click(a, pr_origin().0 + 202.0 + 10.0, bottom_end() - 54.5 + 13.0 + 7.0))),
        Check(
            "Loop off: the clip plays once, to the old loop end; footer Loop off",
            Box::new(|c, a| {
                let m = instrument_clip(c).ok_or("clip gone")?;
                let l = m.clip_loop.ok_or("no loop")?;
                (!l.on && m.length == l.end && !a.global::<Studio>().get_pr_loop_on())
                    .then_some(())
                    .ok_or(format!("{l:?} len {}", m.length))
            }),
        ),
        Do("click the footer Loop switch (on)", Box::new(|a| click(a, pr_origin().0 + 202.0 + 10.0, bottom_end() - 54.5 + 13.0 + 7.0))),
        Check(
            "Loop on again: the kept range and the length the clip had while looping",
            Box::new(|c, _| {
                let m = instrument_clip(c).ok_or("clip gone")?;
                let l = m.clip_loop.ok_or("no loop")?;
                (l.on && l.end == m.length - PPQ).then_some(()).ok_or(format!("{l:?} len {}", m.length))
            }),
        ),
        Do(
            "undo the three loop edits",
            Box::new(|a| {
                for _ in 0..3 {
                    key(a, "z", true, false);
                }
            }),
        ),
        Check(
            "undo restored the whole-clip loop",
            Box::new(|c, _| {
                let m = instrument_clip(c).ok_or("clip gone")?;
                (m.clip_loop == Some(dstudio_core::midi::ClipLoop::whole(m.length)))
                    .then_some(())
                    .ok_or(format!("{:?} len {}", m.clip_loop, m.length))
            }),
        ),
        Do("click Double", Box::new(|a| click(a, pr_origin().0 + 270.5 + 10.0, bottom_y() + 41.0))),
        Check(
            "Double: 2x notes and 2x clip length in ONE undo step",
            Box::new(move |c, _| {
                let (before, len) = sv2.borrow().clone();
                let m = instrument_clip(c).ok_or("clip gone")?;
                one_step(h7.clone(), "Double")(c)?;
                (m.notes.len() == 2 * before.len() && m.length == 2 * len).then_some(()).ok_or(format!(
                    "{} notes, len {}",
                    m.notes.len(),
                    m.length
                ))
            }),
        ),
        Do("Cmd+Z", Box::new(|a| key(a, "z", true, false))),
        Check(
            "undo restored the notes and the clip length exactly",
            Box::new(move |c, _| {
                let (before, len) = saved.borrow().clone();
                let m = instrument_clip(c).ok_or("clip gone")?;
                h15.set(c.doc.history_labels().len());
                (m.notes == before && m.length == len).then_some(()).ok_or(format!("{} notes len {}", m.notes.len(), m.length))
            }),
        ),
        DoC(
            "select the clip in the lane, Cmd+D",
            Box::new(move |a, ctl| {
                let (x, y) = lane_xy(ctl, 2.0, 1);
                click(a, x, y);
                key(a, "d", true, false);
            }),
        ),
        Check(
            "⌘D duplicated the clip right after itself (same notes), copy selected",
            Box::new(move |c, _| {
                let t = c.project().tracks.iter().find(|t| t.kind == TrackKind::Instrument).ok_or("no inst")?;
                one_step(h8.clone(), "Duplicate Clip")(c)?;
                let ok = t.midi_clips.len() == 2
                    && t.midi_clips[1].start == t.midi_clips[0].end()
                    && t.midi_clips[1].notes == t.midi_clips[0].notes
                    && c.selected_clip == Some(t.midi_clips[1].id);
                ok.then_some(()).ok_or(format!("{} clips", t.midi_clips.len()))
            }),
        ),
        Do("Cmd+Z", Box::new(|a| key(a, "z", true, false))),
        DoC(
            "right-click the clip",
            Box::new(move |a, ctl| {
                let (x, y) = lane_xy(ctl, 2.0, 1);
                let w = a.window();
                let p = LogicalPosition::new(x, y);
                dispatch(w, WindowEvent::PointerMoved { position: p });
                dispatch(w, WindowEvent::PointerPressed { position: p, button: PointerEventButton::Right });
                dispatch(w, WindowEvent::PointerReleased { position: p, button: PointerEventButton::Right });
            }),
        ),
        Check(
            "context menu open with Cut/Copy/Duplicate enabled and Split disabled",
            Box::new(|c, a| {
                let s = a.global::<Studio>();
                let items = s.get_menu_items();
                let get = |i: usize| slint::Model::row_data(&items, i);
                let ok = s.get_menu_open()
                    && get(0).is_some_and(|m| m.label == "Cut" && m.enabled)
                    && get(2).is_some_and(|m| m.label == "Duplicate" && m.enabled)
                    && get(3).is_some_and(|m| m.label == "Split" && !m.enabled)
                    && instrument_clip(c).map(|m| m.id) == c.selected_clip;
                ok.then_some(()).ok_or(format!("open {} items {}", s.get_menu_open(), slint::Model::row_count(&items)))
            }),
        ),
        Do(
            "Escape closes the menu, Delete",
            Box::new(|a| {
                key(a, Key::Escape, false, false);
                key(a, Key::Delete, false, false);
            }),
        ),
        Check(
            "menu closed, clip deleted (RemoveMidiClip), piano roll closed",
            Box::new(move |c, a| {
                let s = a.global::<Studio>();
                (!s.get_menu_open() && instrument_clip(c).is_none() && !s.get_pr_visible()).then_some(()).ok_or(format!(
                    "menu {} clip {:?}",
                    s.get_menu_open(),
                    instrument_clip(c).map(|m| m.id)
                ))
            }),
        ),
        Do("Cmd+Z", Box::new(|a| key(a, "z", true, false))),
        Check(
            "undo restored the clip with its notes",
            Box::new(move |c, _| {
                let _ = (&h9, &h10, &h11, &h12);
                let n = notes_of(c);
                (instrument_clip(c).is_some() && n.len() == 4).then_some(()).ok_or(format!("{} notes", n.len()))
            }),
        ),
        DoC(
            "snap: the beat pin again for the sections after",
            Box::new(|_, ctl| ctl.borrow_mut().snap.grid = Some(crate::timeline::GridSize::Quarter)),
        ),
    ]
}

/// Synth device card origin in the window (bottom panel 306 pt at y 509; card at panel 263.5, 34).
fn synth() -> (f32, f32) {
    (bottom_x() + 263.5, bottom_y() + 34.0)
}

fn synth_xy(cx: f32, cy: f32) -> (f32, f32) {
    (synth().0 + cx, synth().1 + cy)
}

/// Centre of category row `i` in the device's left list.
fn synth_category(i: usize) -> (f32, f32) {
    synth_xy(60.0, 31.85 + i as f32 * 25.3 + 12.65)
}

fn double_click(app: &AppWindow, x: f32, y: f32) {
    click(app, x, y);
    click(app, x, y);
}

/// Centre of row `i` of the open synth popup list (same clamping as ui/app.slint).
fn synth_menu_row(app: &AppWindow, i: usize) -> (f32, f32) {
    let s = app.global::<Studio>();
    let (w, h) = (s.get_synth_menu_w(), s.get_synth_menu_h());
    let x = s.get_synth_menu_x().min(win().0 - 5.0 - w);
    let y = s.get_synth_menu_y().min(win().1 - 5.0 - h).max(5.0);
    (x + 30.0, y + crate::synth::MENU_PAD + i as f32 * crate::synth::MENU_ROW + crate::synth::MENU_ROW / 2.0)
}

/// The Instrument track's Synth: (values, preset name).
fn inst_synth(c: &Controller) -> Option<(Vec<f32>, Option<String>)> {
    let d = c.project().tracks.iter().find(|t| t.kind == TrackKind::Instrument)?.instrument_plugin.as_ref()?;
    Some((SYNTH_V2.values(d), d.preset.clone()))
}

fn sx(id: &str) -> usize {
    crate::synth::ix(id)
}

/// Factory sound n as the Synth's values.
fn factory_values(n: usize) -> Vec<f32> {
    let m = dstudio_core::builtin::DevicePreset::factory(&SYNTH_V2, &SYNTH_V2.factory[n]).load_values(&SYNTH_V2);
    SYNTH_V2.params.iter().map(|p| m.get(p.id).copied().unwrap_or(p.default)).collect()
}

/// Synth device (Devices tab on the Instrument track). Leaves the synth back at its
/// starting parameters (everything undone).
fn synth_steps() -> Vec<Step> {
    let bright_low = Rc::new(Cell::new(-1f32));
    let bright_low2 = bright_low.clone();
    let fold_hist = Rc::new(Cell::new(0usize));
    let fold_hist2 = fold_hist.clone();
    use Step::*;
    #[derive(Default)]
    struct Rec {
        hist: usize,
        start: Option<(Vec<f32>, Option<String>)>,
        before: Vec<f32>,
        path: String,
        routes: usize,
    }
    let rec = Rc::new(RefCell::new(Rec::default()));
    let r = |n: usize| -> Vec<Rc<RefCell<Rec>>> { (0..n).map(|_| rec.clone()).collect() };
    let rs = r(18);
    let [r0, r1, r2, r3, r4, r5, r6, r7, r8, r9, r10, r11, r12, r13, r14, r15, r16, r16b] =
        <[Rc<RefCell<Rec>>; 18]>::try_from(rs).ok().unwrap();
    // One new undo step with this label since the last record; re-records.
    fn one_step(rc: &Rc<RefCell<Rec>>, c: &Controller, label: &str) -> Result<(), String> {
        let n = c.doc.history_labels().len();
        let was = rc.borrow().hist;
        rc.borrow_mut().hist = n;
        (n == was + 1 && c.doc.undo_label().is_some_and(|l| l.starts_with(label)))
            .then_some(())
            .ok_or(format!("history {was} -> {n}, last {:?}", c.doc.undo_label()))
    }
    fn values(c: &Controller) -> Result<Vec<f32>, String> {
        inst_synth(c).map(|v| v.0).ok_or_else(|| "no Synth on the instrument track".to_string())
    }
    fn param_text(a: &AppWindow, i: usize) -> String {
        slint::Model::row_data(&a.global::<Studio>().get_synth_params(), i).map(|p| p.text.to_string()).unwrap_or_default()
    }
    fn others_equal(a: &[f32], b: &[f32], except: &[usize]) -> bool {
        a.iter().zip(b).enumerate().all(|(i, (x, y))| except.contains(&i) || x == y)
    }
    const P: &str = "Device Parameter Change";
    vec![
        DoC(
            "click the Instrument track header",
            Box::new(|a, ctl| {
                let sy = ctl.borrow().view.scroll_y as f32;
                click(a, tracks_x() + 37.35, track0_y() + PITCH + 10.0 - sy);
            }),
        ),
        Check(
            "synth: Instrument track selected -> Devices tab shows the Synth (Oscillators page)",
            Box::new(move |c, a| {
                let s = a.global::<Studio>();
                let mut r = r0.borrow_mut();
                r.hist = c.doc.history_labels().len();
                r.start = inst_synth(c);
                let ok = s.get_synth_visible()
                    && s.get_bottom_tab() == 0
                    && s.get_synth_page() == 0
                    && c.synth_track().is_some()
                    && slint::Model::row_count(&s.get_synth_knobs()) == 5;
                ok.then_some(()).ok_or(format!("visible {} tab {} page {}", s.get_synth_visible(), s.get_bottom_tab(), s.get_synth_page()))
            }),
        ),
        Do(
            "click preset ›",
            Box::new(|a| {
                let (x, y) = synth_xy(crate::devices::SYNTH_W - 45.0, 16.0);
                click(a, x, y);
            }),
        ),
        Check(
            "synth: preset › loaded the first factory sound in one undo step",
            Box::new(move |c, a| {
                one_step(&r1, c, "Load Preset")?;
                let (v, name) = inst_synth(c).ok_or("no instrument")?;
                let want = SYNTH_V2.factory[0].name;
                (v == factory_values(0) && name.as_deref() == Some(want) && a.global::<Studio>().get_synth_preset() == want)
                    .then_some(())
                    .ok_or(format!("preset {name:?}"))
            }),
        ),
        Do(
            "click the preset name, pick the 4th sound in the list",
            Box::new(|a| {
                let (x, y) = synth_xy(crate::devices::SYNTH_W - 74.0, 16.0);
                click(a, x, y);
                let (x, y) = synth_menu_row(a, 4);
                click(a, x, y);
            }),
        ),
        Check(
            "synth: preset list loaded factory sound 4 in one undo step, list closed",
            Box::new(move |c, a| {
                one_step(&r2, c, "Load Preset")?;
                let (v, name) = inst_synth(c).ok_or("no instrument")?;
                (v == factory_values(3) && !a.global::<Studio>().get_synth_menu_open()).then_some(()).ok_or(format!("preset {name:?}"))
            }),
        ),
        Do(
            "Cmd+Z, then the Filter page",
            Box::new(|a| {
                key(a, "z", true, false);
                let (x, y) = synth_category(2);
                click(a, x, y);
            }),
        ),
        Check(
            "synth: undo back to the first sound; Filter page shown",
            Box::new(move |c, a| {
                let mut r = r3.borrow_mut();
                r.hist = c.doc.history_labels().len();
                r.before = values(c)?;
                (r.before == factory_values(0) && a.global::<Studio>().get_synth_page() == 2)
                    .then_some(())
                    .ok_or(format!("page {}", a.global::<Studio>().get_synth_page()))
            }),
        ),
        Do(
            "drag the Cutoff knob down 60 pt",
            Box::new(|a| {
                let (x, y) = synth_xy(152.5, 212.0);
                drag(a, x, y, x, y + 60.0);
            }),
        ),
        Check(
            "synth: cutoff decreased (log taper) with exactly one undo step; readout in Hz",
            Box::new(move |c, a| {
                one_step(&r4, c, P)?;
                let before = r4.borrow().before.clone();
                let v = values(c)?;
                let (i, p) = (sx("cutoff"), &SYNTH_V2.params[sx("cutoff")]);
                let want = p.from_normalized(p.to_normalized(before[i]) - 0.4);
                let text = param_text(a, i);
                (v[i] < before[i] && (v[i] - want).abs() < want * 0.03 && others_equal(&v, &before, &[i]) && text.ends_with("Hz"))
                    .then_some(())
                    .ok_or(format!("cutoff {} -> {} (want ≈{want}), readout {text:?}", before[i], v[i]))
            }),
        ),
        Check(
            "audio: rendered output with the lowered cutoff (measured for the next check)",
            Box::new(move |c, _| {
                if let Some(b) = rendered_brightness(c)? {
                    bright_low.set(b);
                }
                Ok(())
            }),
        ),
        Do(
            "double-click the Cutoff knob",
            Box::new(|a| {
                let (x, y) = synth_xy(152.5, 212.0);
                double_click(a, x, y);
            }),
        ),
        Check(
            "synth: double-click reset cutoff to its default in one undo step",
            Box::new(move |c, _| {
                one_step(&r5, c, P)?;
                let v = values(c)?;
                let i = sx("cutoff");
                (v[i] == SYNTH_V2.params[i].default).then_some(()).ok_or(format!("cutoff {}", v[i]))
            }),
        ),
        Check(
            "audio: panel cutoff reaches the engine (brighter after reset than with the lowered cutoff)",
            Box::new(move |c, _| {
                let Some(reset) = rendered_brightness(c)? else { return Ok(()) };
                let low = bright_low2.get();
                (low >= 0.0 && reset > low * 1.15).then_some(()).ok_or(format!("brightness low {low} vs reset {reset}"))
            }),
        ),
        Do(
            "Oscillators page, open the Osc 2 wave dropdown",
            Box::new(|a| {
                let (x, y) = synth_category(0);
                click(a, x, y);
                let (x, y) = synth_xy(433.0 + 45.0, 42.0);
                click(a, x, y);
            }),
        ),
        Check(
            "synth: wave list open with the six waves; record the Osc 2 picture",
            Box::new(move |c, a| {
                let s = a.global::<Studio>();
                let items: Vec<String> = (0..slint::Model::row_count(&s.get_synth_menu_items()))
                    .filter_map(|i| slint::Model::row_data(&s.get_synth_menu_items(), i))
                    .map(|m| m.label.to_string())
                    .collect();
                r6.borrow_mut().path = slint::Model::row_data(&s.get_synth_displays(), 1).map(|d| d.line.to_string()).unwrap_or_default();
                r6.borrow_mut().hist = c.doc.history_labels().len();
                (s.get_synth_page() == 0 && s.get_synth_menu_open() && items == ["Sine", "Triangle", "Saw", "Square", "Sync", "FM"])
                    .then_some(())
                    .ok_or(format!("page {} open {} {items:?}", s.get_synth_page(), s.get_synth_menu_open()))
            }),
        ),
        Do(
            "pick Square",
            Box::new(|a| {
                let (x, y) = synth_menu_row(a, 3);
                click(a, x, y);
            }),
        ),
        Check(
            "synth: Osc 2 wave = Square (one undo step), pill text and picture updated",
            Box::new(move |c, a| {
                one_step(&r7, c, P)?;
                let s = a.global::<Studio>();
                let v = values(c)?;
                let i = sx("osc2_wave");
                let line = slint::Model::row_data(&s.get_synth_displays(), 1).map(|d| d.line.to_string()).unwrap_or_default();
                let pills = s.get_synth_pills();
                let text = (0..slint::Model::row_count(&pills))
                    .filter_map(|k| slint::Model::row_data(&pills, k))
                    .find(|p| p.action == 0 && p.arg == i as i32)
                    .map(|p| p.value.to_string())
                    .unwrap_or_default();
                let changed = line != r7.borrow().path;
                r7.borrow_mut().path = line;
                (v[i] == 3.0 && text == "Square" && changed && !s.get_synth_menu_open())
                    .then_some(())
                    .ok_or(format!("wave {} text {text} path changed {changed}", v[i]))
            }),
        ),
        Do(
            "Osc 2 wave dropdown -> Saw",
            Box::new(|a| {
                let (x, y) = synth_xy(433.0 + 45.0, 42.0);
                click(a, x, y);
                let (x, y) = synth_menu_row(a, 2);
                click(a, x, y);
            }),
        ),
        Check(
            "synth: Osc 2 wave = Saw; Saw and Square pictures differ",
            Box::new(move |c, a| {
                one_step(&r8, c, P)?;
                let v = values(c)?;
                let line =
                    slint::Model::row_data(&a.global::<Studio>().get_synth_displays(), 1).map(|d| d.line.to_string()).unwrap_or_default();
                (v[sx("osc2_wave")] == 2.0 && line != r8.borrow().path && !line.is_empty())
                    .then_some(())
                    .ok_or(format!("wave {}", v[sx("osc2_wave")]))
            }),
        ),
        Do(
            "drag the Osc 1 Level fader up 10 pt",
            Box::new(|a| {
                let (x, y) = synth_xy(117.5 + 273.3, 61.5 + 45.0);
                drag(a, x, y, x, y - 10.0);
            }),
        ),
        Check(
            "synth: Osc 1 level +10/91 in one undo step, readout in %",
            Box::new(move |c, a| {
                let i = sx("osc1_level");
                let before = factory_values(0)[i];
                one_step(&r9, c, P)?;
                let v = values(c)?;
                let text = param_text(a, i);
                (approx(v[i], before + 10.0 / 91.0, 0.01) && text == format!("{:.0}%", v[i] * 100.0))
                    .then_some(())
                    .ok_or(format!("level {} text {text}", v[i]))
            }),
        ),
        Do(
            "Envelopes page",
            Box::new(|a| {
                let (x, y) = synth_category(3);
                click(a, x, y);
            }),
        ),
        Check(
            "synth: Envelopes page; record params",
            Box::new(move |c, a| {
                r10.borrow_mut().before = values(c)?;
                r10.borrow_mut().hist = c.doc.history_labels().len();
                (a.global::<Studio>().get_synth_page() == 3).then_some(()).ok_or("page".into())
            }),
        ),
        Do(
            "drag the Amp Attack knob up 45 pt",
            Box::new(|a| {
                let (x, y) = synth_xy(117.5 + 27.5, 212.0);
                drag(a, x, y, x, y - 45.0);
            }),
        ),
        Check(
            "synth: amp attack increased (one undo step), Env 2 unchanged, readout in ms/s",
            Box::new(move |c, a| {
                one_step(&r11, c, P)?;
                let before = r11.borrow().before.clone();
                let v = values(c)?;
                let i = sx("amp_attack");
                let text = param_text(a, i);
                (v[i] > before[i] * 2.0 && others_equal(&v, &before, &[i]) && (text.ends_with(" ms") || text.ends_with(" s")))
                    .then_some(())
                    .ok_or(format!("attack {} -> {} {text}", before[i], v[i]))
            }),
        ),
        Do("Cmd+Z", Box::new(|a| key(a, "z", true, false))),
        Check(
            "synth: ⌘Z restored the previous parameters exactly",
            Box::new(move |c, _| {
                let before = r12.borrow().before.clone();
                (values(c)? == before).then_some(()).ok_or("params differ after undo".into())
            }),
        ),
        Do(
            "Modulation page",
            Box::new(|a| {
                let (x, y) = synth_category(5);
                click(a, x, y);
            }),
        ),
        Check(
            "synth: Modulation page lists the routes",
            Box::new(move |c, a| {
                let s = a.global::<Studio>();
                let n = slint::Model::row_count(&s.get_synth_routes());
                let want = crate::synth::routes(&values(c)?).len();
                r13.borrow_mut().routes = n;
                r13.borrow_mut().hist = c.doc.history_labels().len();
                (s.get_synth_page() == 5 && n == want && s.get_synth_add_show()).then_some(()).ok_or(format!("{n} rows, {want} routes"))
            }),
        ),
        Do(
            "+ Add modulation",
            Box::new(|a| {
                let y = a.global::<Studio>().get_synth_add_y();
                let (x, y) = synth_xy(112.0 + 40.0, 35.0 + y + 10.0);
                click(a, x, y);
            }),
        ),
        Check(
            "synth: the source list is open",
            Box::new(|_, a| {
                let s = a.global::<Studio>();
                let n = slint::Model::row_count(&s.get_synth_menu_items());
                (s.get_synth_menu_open() && n == 9).then_some(()).ok_or(format!("open {} rows {n}", s.get_synth_menu_open()))
            }),
        ),
        Do(
            "pick Velocity",
            Box::new(|a| {
                let (x, y) = synth_menu_row(a, 4);
                click(a, x, y);
            }),
        ),
        Check(
            "synth: a Velocity route was added in one undo step",
            Box::new(move |c, a| {
                one_step(&r14, c, "Device Parameters Change")?;
                let v = values(c)?;
                let rs = crate::synth::routes(&v);
                let n = slint::Model::row_count(&a.global::<Studio>().get_synth_routes());
                (rs.len() == r14.borrow().routes + 1 && n == rs.len() && rs.iter().any(|r| r.1 == 5))
                    .then_some(())
                    .ok_or(format!("{} routes", rs.len()))
            }),
        ),
        Do(
            "Effects page",
            Box::new(|a| {
                let (x, y) = synth_category(8);
                click(a, x, y);
            }),
        ),
        Check(
            "synth: Effects page shows the four slot tabs, the mix fader and the dials",
            Box::new(move |_, a| {
                let s = a.global::<Studio>();
                let tabs = (0..slint::Model::row_count(&s.get_synth_pills()))
                    .filter_map(|k| slint::Model::row_data(&s.get_synth_pills(), k))
                    .filter(|p| p.action == 9)
                    .count();
                let _ = &r15;
                (s.get_synth_page() == 8
                    && tabs == 4
                    && slint::Model::row_count(&s.get_synth_faders()) == 1
                    && slint::Model::row_count(&s.get_synth_knobs()) >= 4)
                    .then_some(())
                    .ok_or(format!("{tabs} tabs"))
            }),
        ),
        Do(
            "Cmd+Z x7 (route, level, Saw, Square, reset, cutoff drag, preset ›)",
            Box::new(|a| {
                for _ in 0..7 {
                    key(a, "z", true, false);
                }
            }),
        ),
        Check(
            "synth: every synth edit undone -> starting parameters exactly",
            Box::new(move |c, _| {
                let start = r16.borrow().start.clone().ok_or("no record")?;
                (inst_synth(c).as_ref() == Some(&start)).then_some(()).ok_or(format!("preset {:?}", inst_synth(c).map(|p| p.1)))
            }),
        ),
        DoC(
            "click the Synth header's fold button",
            Box::new(move |a, ctl| {
                fold_hist.set(ctl.borrow().doc.history_labels().len());
                let (x, y) = synth_xy(crate::devices::SYNTH_W - 27.5, 16.0);
                click(a, x, y);
            }),
        ),
        Check(
            "synth: folds to the 30 pt strip in one undo step",
            Box::new(move |c, a| {
                let card = a.global::<Studio>().get_synth_card();
                let n = c.doc.history_labels().len();
                (n == fold_hist2.get() + 1 && card.folded && card.width == crate::device_mod::FOLDED_W).then_some(()).ok_or(format!(
                    "history {} -> {n}, folded {} width {}",
                    fold_hist2.get(),
                    card.folded,
                    card.width
                ))
            }),
        ),
        Do("Cmd+Z", Box::new(|a| key(a, "z", true, false))),
        Check(
            "synth: undo unfolds it to full width",
            Box::new(move |_, a| {
                let card = a.global::<Studio>().get_synth_card();
                (!card.folded && card.width == crate::devices::SYNTH_W)
                    .then_some(())
                    .ok_or(format!("folded {} width {}", card.folded, card.width))
            }),
        ),
        Do(
            "click the Synth header's remove button",
            Box::new(|a| {
                let (x, y) = synth_xy(crate::devices::SYNTH_W - 9.5, 16.0);
                click(a, x, y);
            }),
        ),
        Check(
            "synth: removed, the slot is empty",
            Box::new(|c, a| {
                (c.synth_device().is_none() && !a.global::<Studio>().get_synth_visible())
                    .then_some(())
                    .ok_or("the Synth is still there".to_string())
            }),
        ),
        DoC(
            "choose Synth for the empty slot",
            Box::new(|_, ctl| {
                let mut c = ctl.borrow_mut();
                let t = c.selected_track.expect("selected track");
                c.add_item_to_track(&crate::devices::Item::BuiltinSynth, t, None);
            }),
        ),
        Check(
            "synth: choosing Synth restores it",
            Box::new(|c, a| {
                (c.synth_device().is_some() && a.global::<Studio>().get_synth_visible()).then_some(()).ok_or("no Synth".to_string())
            }),
        ),
        Do(
            "Cmd+Z x2 (add, remove)",
            Box::new(|a| {
                key(a, "z", true, false);
                key(a, "z", true, false);
            }),
        ),
        Check(
            "synth: undo brings back the original Synth",
            Box::new(move |c, _| {
                let start = r16b.borrow().start.clone().ok_or("no record")?;
                (inst_synth(c).as_ref() == Some(&start)).then_some(()).ok_or(format!("preset {:?}", inst_synth(c).map(|p| p.1)))
            }),
        ),
    ]
}

/// Render ~100 ms offline of a test note (C3) on the selected instrument
/// track, using the controller's current session (so the synth parameters are
/// exactly what the Devices panel set), then restore the real session.
/// Returns a brightness measure: RMS of the first difference over RMS of the
/// signal (higher = more high-frequency energy).
fn rendered_brightness(c: &Controller) -> Result<Option<f32>, String> {
    if c.engine.device_running() {
        return Ok(None); // offline render is refused while the device owns the audio path
    }
    let id = c.selected_track.ok_or("no selected track")?;
    let mut snap = c.current_snapshot();
    for t in &mut snap.tracks {
        if t.track_id == id.0 {
            t.notes = vec![dstudio_core::snapshot::NoteSnapshot { start: 0, end: 48_000, pitch: 48, velocity: 100 }];
        } else {
            t.mute = true; // isolate the instrument under test
        }
    }
    let e = &c.engine;
    e.publish(&snap).map_err(|e| e.to_string())?;
    e.seek(0).map_err(|e| e.to_string())?;
    e.play().map_err(|e| e.to_string())?;
    let mut buf = vec![0f32; 2 * 4_800];
    let r = e.render_offline(&mut buf);
    let _ = e.stop();
    e.publish(&c.current_snapshot()).map_err(|e| e.to_string())?;
    r.map_err(|e| e.to_string())?;
    let l: Vec<f32> = buf.iter().step_by(2).copied().collect();
    let rms = |v: &mut dyn Iterator<Item = f32>| {
        let (s, n) = v.fold((0f64, 0usize), |(s, n), x| (s + (x as f64).powi(2), n + 1));
        (s / n.max(1) as f64).sqrt() as f32
    };
    let level = rms(&mut l.iter().copied());
    if level < 1e-4 {
        return Err(format!("render is silent (rms {level})"));
    }
    Ok(Some(rms(&mut l.windows(2).map(|w| w[1] - w[0])) / level))
}

fn steps() -> Vec<Step> {
    use Step::*;
    // Values recorded by one check and compared by a later one.
    let rec_tracks = Rc::new(Cell::new(0usize));
    let rec_gain = Rc::new(Cell::new(0f32));
    let rec_rev = Rc::new(Cell::new(0u64));
    let rec_vol_steps = Rc::new(Cell::new(0usize));
    let rec_mute = Rc::new(Cell::new(false));
    let (press_beat, press_beat2) = {
        let b = Rc::new(Cell::new(0f64));
        (b.clone(), b)
    };
    let (rt1, rt2, rt3, rt4, rt5) = (rec_tracks.clone(), rec_tracks.clone(), rec_tracks.clone(), rec_tracks.clone(), rec_tracks.clone());
    let (rg1, rg2) = (rec_gain.clone(), rec_gain.clone());
    let (rr1, rr2) = (rec_rev.clone(), rec_rev.clone());
    let (rv1, rv2) = (rec_vol_steps.clone(), rec_vol_steps.clone());
    let (rm1, rm2) = (rec_mute.clone(), rec_mute.clone());
    // Assistant panel geometry with the mixer closed (see ui/assistant.slint).
    // Card: 8 px inset, 143 px tall with the AI notice (CSS px x 0.85).
    fn card_x() -> f32 {
        geo().chat_x + 6.8
    }
    fn card_y() -> f32 {
        let g = geo();
        g.chat_y + g.chat_h - 6.8 - 121.55
    }
    fn input_y() -> f32 {
        card_y() + 28.05 + 27.2 + 22.0
    }
    fn mixer_y() -> f32 {
        bottom_y()
    }
    // Mixer strip fader travel (ui/channel_strip.slint): the 200 pt canvas sits 234 pt above the
    // strip bottom; its handle travels 168 pt starting 18 pt down (100%-zoom points * 0.85).
    const MIXER_TRAVEL_TOP: f32 = 390.0 - (234.0 - 18.0) * 0.85;
    const MIXER_TRAVEL: f32 = 168.0 * 0.85;
    let bx = |b: f64| lane_x() + (b * crate::timeline::DEFAULT_PX_PER_BEAT) as f32;
    vec![
        Check(
            "demo imported two clips on two audio tracks",
            Box::new(|c, _| {
                let p = c.project();
                let n: usize = p.tracks.iter().map(|t| t.clips.len()).sum();
                (n == 2 && p.tracks.len() == 3 && p.assets.len() == 2).then_some(()).ok_or(format!("tracks {} clips {n}", p.tracks.len()))
            }),
        ),
        Check(
            "second clip placed at the playhead (beat 10)",
            Box::new(|c, _| {
                let s = clip_start_beats(c, 2).ok_or("no clip on track 3")?;
                ((s - 10.0).abs() < 1e-6).then_some(()).ok_or(format!("start {s}"))
            }),
        ),
        Do("click track 2 header", Box::new(|a| click(a, tracks_x() + 37.35, track0_y() + PITCH + 10.0))),
        Check(
            "track 2 selected, inspector follows",
            Box::new(|c, a| {
                let t = c.project().tracks[1].id;
                let s = a.global::<Studio>();
                (c.selected_track == Some(t) && s.get_sel_number() == 2 && s.get_sel_name() == "Instrument")
                    .then_some(())
                    .ok_or("selection".into())
            }),
        ),
        Do("click M on track 1", Box::new(|a| click(a, tracks_x() + 33.5, track0_y() + 47.0))),
        Check("track 1 muted (SetTrackMute)", Box::new(|c, _| c.project().tracks[0].mute.then_some(()).ok_or("not muted".into()))),
        Check(
            "Edit menu names the edit in sentence case: Undo Mute change enabled",
            Box::new(|_, a| {
                let s = a.global::<Studio>();
                (s.get_undo_text() == "Undo Mute change" && s.get_can_undo()).then_some(()).ok_or(format!(
                    "{:?} {}",
                    s.get_undo_text(),
                    s.get_can_undo()
                ))
            }),
        ),
        Do("Cmd+Z (Edit › Undo shortcut, replayed by the menu)", Box::new(|a| key(a, "z", true, false))),
        Check("undo restored mute=false", Box::new(|c, _| (!c.project().tracks[0].mute).then_some(()).ok_or("still muted".into()))),
        Check(
            "Edit menu: Redo Mute change enabled",
            Box::new(|_, a| {
                let s = a.global::<Studio>();
                (s.get_redo_text() == "Redo Mute change" && s.get_can_redo()).then_some(()).ok_or(format!(
                    "{:?} {}",
                    s.get_redo_text(),
                    s.get_can_redo()
                ))
            }),
        ),
        Do("Cmd+Shift+Z", Box::new(|a| key(a, "Z", true, true))),
        Check("redo muted again", Box::new(|c, _| c.project().tracks[0].mute.then_some(()).ok_or("redo failed".into()))),
        Do("⇧⌘E (file.export_audio)", Box::new(|a| key(a, "E", true, true))),
        Check(
            "export dialog open with a range and a size estimate",
            Box::new(|_c, a| {
                let s = a.global::<Studio>();
                (s.get_export_open() && s.get_export_bars_text().starts_with("Bars 1–") && s.get_export_estimate().contains("MB"))
                    .then_some(())
                    .ok_or(format!("open {} bars {} est {}", s.get_export_open(), s.get_export_bars_text(), s.get_export_estimate()))
            }),
        ),
        DoC(
            "export into a temp folder: click Export",
            Box::new(|a, ctl| {
                {
                    let mut c = ctl.borrow_mut();
                    c.export.dir = run_dir().join("export");
                    c.export.name = "mix".into();
                }
                // Export window centred in the window: 442 x 340.4, Export button 52.2 x 22.1.
                click(a, (win().0 - 442.0) / 2.0 + 442.0 - 17.0 - 26.1, (win().1 - 340.4) / 2.0 + 340.4 - 57.8 + 16.15 + 11.05);
            }),
        ),
        Check(
            "mix.wav written: stereo 24-bit at the dialog's rate, the full length, not silent; dialog closed",
            Box::new(|c, a| {
                let path = c.export.dir.join("mix.wav");
                let (rate, ch, s) = crate::wavgen::read_wav(&path)?;
                let peak = s.iter().fold(0f32, |m, v| m.max(v.abs()));
                let frames = s.len() / 2;
                (rate == c.export_rate()
                    && ch == 2
                    && frames as u64 == c.export_out_frames()
                    && peak > 0.01
                    && !a.global::<Studio>().get_export_open())
                .then_some(())
                .ok_or(format!("rate {rate} ch {ch} frames {frames} vs {} peak {peak}", c.export_out_frames()))
            }),
        ),
        Do("⇧⌘E for an export at the other sample rate", Box::new(|a| key(a, "E", true, true))),
        DoC(
            "click the other rate (44.1 kHz when the engine runs at 48)",
            Box::new(|a, ctl| {
                let to48 = ctl.borrow().export_rate() != 48_000;
                // Rate segments: 71.4 + 197.7 from the window's left, 136.85 from its top, 56.3 wide each.
                let x = 71.4 + 197.7 + 1.7 + if to48 { 56.3 + 28.8 } else { 28.15 };
                click(a, (win().0 - 442.0) / 2.0 + x, (win().1 - 340.4) / 2.0 + 136.85 + 9.35);
            }),
        ),
        Check(
            "the other rate is picked; the size estimate follows it",
            Box::new(|c, a| {
                let s = a.global::<Studio>();
                let other = if c.engine.sample_rate() >= 47_000.0 { 44_100 } else { 48_000 };
                let est = crate::dialogs::estimate_text(c.export_out_frames(), other as f64, if c.export.bits24 { 24 } else { 16 }, 1);
                (c.export_rate() == other && s.get_export_rate48() == (other == 48_000) && s.get_export_estimate() == est.as_str())
                    .then_some(())
                    .ok_or(format!("rate {} est {} vs {est}", c.export_rate(), s.get_export_estimate()))
            }),
        ),
        DoC(
            "export it as mix-other: click Export",
            Box::new(|a, ctl| {
                ctl.borrow_mut().export.name = "mix-other".into();
                click(a, (win().0 - 442.0) / 2.0 + 442.0 - 17.0 - 26.1, (win().1 - 340.4) / 2.0 + 340.4 - 57.8 + 16.15 + 11.05);
            }),
        ),
        Check(
            "mix-other.wav is at the other rate, as long in seconds as mix.wav and as loud",
            Box::new(|c, _| {
                let (rate, ch, s) = crate::wavgen::read_wav(&c.export.dir.join("mix-other.wav"))?;
                let (mix_rate, _, mix) = crate::wavgen::read_wav(&c.export.dir.join("mix.wav"))?;
                let rms = |v: &[f32]| (v.iter().map(|x| x * x).sum::<f32>() / v.len().max(1) as f32).sqrt();
                let secs = |n: usize, r: u32| n as f64 / 2.0 / r as f64;
                let (a, b) = (rms(&s), rms(&mix));
                let ok = rate == c.export_rate()
                    && rate != mix_rate
                    && ch == 2
                    && (s.len() / 2) as u64 == c.export_out_frames()
                    && (secs(s.len(), rate) - secs(mix.len(), mix_rate)).abs() < 1e-3
                    && b > 0.0
                    && (a / b - 1.0).abs() < 0.02;
                ok.then_some(()).ok_or(format!("rate {rate} vs {mix_rate} frames {} rms {a} vs {b}", s.len() / 2))
            }),
        ),
        DoC(
            "back to the default rate for the next exports",
            Box::new(|_, ctl| {
                let _ = std::fs::remove_file(ctl.borrow().export.dir.join("mix-other.wav"));
                ctl.borrow_mut().export.rate = None;
            }),
        ),
        DoC(
            "loop region over bars 2-3",
            Box::new(|_, ctl| {
                let mut c = ctl.borrow_mut();
                let tpb = c.project().ticks_per_bar();
                let enabled = c.project().transport.loop_enabled;
                let _ = c.doc.execute(dstudio_core::Command::SetLoop { enabled, start: tpb, end: 3 * tpb }, dstudio_core::Origin::User);
                c.after_change();
            }),
        ),
        Do("⇧⌘E for a loop-region export", Box::new(|a| key(a, "E", true, true))),
        Do("click the Range picker", Box::new(|a| click(a, (win().0 - 442.0) / 2.0 + 71.4 + 30.0, (win().1 - 340.4) / 2.0 + 105.4 + 10.2))),
        Check(
            "Range menu: Full project (checked) and Loop region",
            Box::new(|c, a| {
                let s = a.global::<Studio>();
                let rows: Vec<_> =
                    (0..s.get_export_range_items().row_count()).filter_map(|i| s.get_export_range_items().row_data(i)).collect();
                let ok = s.get_export_range_menu_open()
                    && rows.len() == 2
                    && rows[0].checked
                    && !rows[1].checked
                    && !rows[1].hint
                    && c.export_range() == crate::dialogs::ExportRange::Project;
                ok.then_some(()).ok_or(format!("open {} rows {}", s.get_export_range_menu_open(), rows.len()))
            }),
        ),
        // The menu sits 3.4 below the picker; rows are 21 tall after 3 of padding.
        Do(
            "pick Loop region",
            Box::new(|a| click(a, (win().0 - 442.0) / 2.0 + 71.4 + 40.0, (win().1 - 340.4) / 2.0 + 105.4 + 20.4 + 3.4 + 3.0 + 21.0 + 10.5)),
        ),
        Check(
            "the range reads Loop region, Bars 2–3, and the estimate covers two bars",
            Box::new(|c, a| {
                let s = a.global::<Studio>();
                let est =
                    crate::dialogs::estimate_text(c.export_out_frames(), c.export_rate() as f64, if c.export.bits24 { 24 } else { 16 }, 1);
                let p = c.project();
                let two_bars = p.ticks_to_seconds(2 * p.ticks_per_bar());
                let secs = c.export_out_frames() as f64 / c.export_rate() as f64;
                let ok = !s.get_export_range_menu_open()
                    && s.get_export_range_label() == "Loop region"
                    && s.get_export_bars_text() == "Bars 2–3"
                    && s.get_export_estimate() == est.as_str()
                    && (secs - two_bars).abs() < 1e-3;
                ok.then_some(()).ok_or(format!(
                    "label {} bars {} est {} secs {secs} vs {two_bars}",
                    s.get_export_range_label(),
                    s.get_export_bars_text(),
                    s.get_export_estimate()
                ))
            }),
        ),
        DoC(
            "export it as loop: click Export",
            Box::new(|a, ctl| {
                ctl.borrow_mut().export.name = "loop".into();
                click(a, (win().0 - 442.0) / 2.0 + 442.0 - 17.0 - 26.1, (win().1 - 340.4) / 2.0 + 340.4 - 57.8 + 16.15 + 11.05);
            }),
        ),
        Check(
            "loop.wav is the two loop bars, with sound where the mix has it",
            Box::new(|c, a| {
                let (rate, ch, s) = crate::wavgen::read_wav(&c.export.dir.join("loop.wav"))?;
                let (_, _, mix) = crate::wavgen::read_wav(&c.export.dir.join("mix.wav"))?;
                let p = c.project();
                let start = (p.ticks_to_seconds(p.ticks_per_bar()) * rate as f64).round() as usize * 2;
                let peak = |v: &[f32]| v.iter().fold(0f32, |m, x| m.max(x.abs()));
                let mix_peak = peak(mix.get(start..(start + s.len()).min(mix.len())).unwrap_or(&[]));
                let ok = rate == c.export_rate()
                    && ch == 2
                    && (s.len() / 2) as u64 == c.export_out_frames()
                    && (mix_peak < 0.01 || peak(&s) > 0.001)
                    && !a.global::<Studio>().get_export_open();
                ok.then_some(()).ok_or(format!(
                    "rate {rate} frames {} vs {} peak {} mix {mix_peak}",
                    s.len() / 2,
                    c.export_out_frames(),
                    peak(&s)
                ))
            }),
        ),
        DoC(
            "back to the full project and the old loop region",
            Box::new(|_, ctl| {
                let mut c = ctl.borrow_mut();
                let _ = std::fs::remove_file(c.export.dir.join("loop.wav"));
                c.export.range = crate::dialogs::ExportRange::Project;
                c.undo();
            }),
        ),
        Do("⇧⌘E again", Box::new(|a| key(a, "E", true, true))),
        Check("export dialog open again", Box::new(|_c, a| a.global::<Studio>().get_export_open().then_some(()).ok_or("not open".into()))),
        Do(
            "click the Stems card",
            Box::new(|a| click(a, (win().0 - 442.0) / 2.0 + 17.0 + 131.47 + 6.8 + 65.0, (win().1 - 340.4) / 2.0 + 44.2 + 22.0)),
        ),
        Check(
            "Stems picked: Tracks row, one file per heard track with clips, Folder label",
            Box::new(|c, a| {
                let s = a.global::<Studio>();
                let n = crate::dialogs::stem_tracks(c.project(), crate::dialogs::StemsPick::AllTracks, None).len();
                (s.get_export_recipe() == 1
                    && n > 0
                    && s.get_export_stems_text() == format!("{n} WAV file{}", if n == 1 { "" } else { "s" })
                    && s.get_export_name_label() == "Folder"
                    && s.get_export_tracks_label() == "All individual tracks")
                    .then_some(())
                    .ok_or(format!(
                        "recipe {} files {n} text {} label {}",
                        s.get_export_recipe(),
                        s.get_export_stems_text(),
                        s.get_export_name_label()
                    ))
            }),
        ),
        Do(
            "click the Tracks picker",
            Box::new(|a| click(a, (win().0 - 442.0) / 2.0 + 71.4 + 30.0, (win().1 - 340.4) / 2.0 + 105.4 + 10.2)),
        ),
        Check(
            "Tracks menu lists both choices, a gap and every track",
            Box::new(|c, a| {
                let s = a.global::<Studio>();
                let rows = s.get_export_tracks_items().row_count();
                (s.get_export_tracks_menu_open() && rows == 3 + c.project().tracks.len())
                    .then_some(())
                    .ok_or(format!("open {} rows {rows}", s.get_export_tracks_menu_open()))
            }),
        ),
        Do("Escape", Box::new(|a| key(a, Key::Escape, false, false))),
        Check(
            "Escape closed the menu, not the dialog",
            Box::new(|_c, a| {
                let s = a.global::<Studio>();
                (s.get_export_open() && !s.get_export_tracks_menu_open()).then_some(()).ok_or("menu or dialog state".into())
            }),
        ),
        DoC(
            "name the stems mix and click Export",
            Box::new(|a, ctl| {
                ctl.borrow_mut().export.name = "mix".into();
                click(a, (win().0 - 442.0) / 2.0 + 442.0 - 17.0 - 26.1, (win().1 - 340.4) / 2.0 + 340.4 - 57.8 + 16.15 + 11.05);
            }),
        ),
        Check(
            "'mix Stems' holds one 'NN Name.wav' per track, full length, summing to the mixdown",
            Box::new(|c, a| {
                let p = c.project();
                let tracks = crate::dialogs::stem_tracks(p, crate::dialogs::StemsPick::AllTracks, None);
                let folder = c.export.dir.join("mix Stems");
                let (_, _, mix) = crate::wavgen::read_wav(&c.export.dir.join("mix.wav"))?;
                let mut sum = vec![0f32; mix.len()];
                for (i, id) in tracks.iter().enumerate() {
                    let name = crate::dialogs::stem_file_name(i, &p.track(*id).map_or(String::new(), |t| t.name.clone()));
                    let (rate, ch, s) = crate::wavgen::read_wav(&folder.join(&name))?;
                    if rate as f64 != c.engine.sample_rate() || ch != 2 || s.len() != mix.len() {
                        return Err(format!("{name}: rate {rate} ch {ch} len {} vs {}", s.len(), mix.len()));
                    }
                    sum.iter_mut().zip(&s).for_each(|(a, b)| *a += b);
                }
                let files = std::fs::read_dir(&folder).map_or(0, |d| d.count());
                let _ = std::fs::remove_dir_all(&c.export.dir);
                let err = sum.iter().zip(&mix).fold(0f32, |m, (a, b)| m.max((a - b).abs()));
                let peak = mix.iter().fold(0f32, |m, v| m.max(v.abs()));
                (files == tracks.len() && err < 1e-3 && peak > 0.01 && !a.global::<Studio>().get_export_open())
                    .then_some(())
                    .ok_or(format!("files {files} of {} max error {err} peak {peak}", tracks.len()))
            }),
        ),
        Do("⇧⌘E for the Project export", Box::new(|a| key(a, "E", true, true))),
        Check(
            "export dialog open for Project",
            Box::new(|_c, a| a.global::<Studio>().get_export_open().then_some(()).ok_or("not open".into())),
        ),
        DoC(
            "two returns for the Project export: Return A (sent to at -6 dB from a playing track, and the Out of another track with clips), Return B (with a device, sent to pre-fader from the playing track)",
            Box::new(|_a, ctl| {
                let mut c = ctl.borrow_mut();
                EXPORT_UNDO_DEPTH.with(|d| d.set(c.doc.undo_depth()));
                // The sends come from a track that plays (a muted track feeds no return); the Out
                // change goes on another track with clips.
                let mut with_clips: Vec<(bool, dstudio_core::TrackId)> =
                    c.project().tracks.iter().filter(|t| !t.clips.is_empty() || !t.midi_clips.is_empty()).map(|t| (t.mute, t.id)).collect();
                with_clips.sort_by_key(|(mute, _)| *mute);
                let with_clips: Vec<dstudio_core::TrackId> = with_clips.into_iter().map(|(_, id)| id).collect();
                let selected = c.selected_track;
                let (Some(ra), Some(rb)) = (c.add_return_track(true), c.add_return_track(true)) else { return };
                let plugin = dstudio_core::PluginRef {
                    format: dstudio_core::PluginFormat::Clap,
                    id: "studio.dstudio.test.gain".into(),
                    path: Some(dstudio_engine::test_plugin_dir().join("DStudioTestGain.clap").to_string_lossy().into_owned()),
                    name: "DStudio Test Gain".into(),
                    vendor: "DStudio".into(),
                    version: "1.0.0".into(),
                    kind: dstudio_core::PluginKind::Effect,
                };
                c.execute(dstudio_core::Command::AddDevice { track: rb, index: None, device: dstudio_core::Device::new(plugin) });
                if let Some(&t0) = with_clips.first() {
                    c.add_send(t0, ra);
                    c.set_send_level(t0, ra, 0.5);
                    c.send_gesture_end();
                    c.add_send(t0, rb);
                    c.toggle_send_pre(t0, rb);
                }
                if let Some(&t1) = with_clips.get(1) {
                    c.set_track_output(t1, Some(ra));
                }
                if let Some(t) = selected {
                    c.select_track(t);
                }
            }),
        ),
        DoC(
            "click the Project card, name it set, click Export",
            Box::new(|a, ctl| {
                click(a, (win().0 - 442.0) / 2.0 + 17.0 + 2.0 * (131.47 + 6.8) + 65.0, (win().1 - 340.4) / 2.0 + 44.2 + 22.0);
                let mut c = ctl.borrow_mut();
                c.export.dir = run_dir().join("project");
                c.export.name = "set".into();
            }),
        ),
        Check(
            "Project picked: recipe 2, Name label",
            Box::new(|_c, a| {
                let s = a.global::<Studio>();
                (s.get_export_recipe() == 2 && s.get_export_name_label() == "Name")
                    .then_some(())
                    .ok_or(format!("recipe {}", s.get_export_recipe()))
            }),
        ),
        Do(
            "click Export",
            Box::new(|a| click(a, (win().0 - 442.0) / 2.0 + 442.0 - 17.0 - 26.1, (win().1 - 340.4) / 2.0 + 340.4 - 57.8 + 16.15 + 11.05)),
        ),
        Check(
            "'set Project' holds set.als that passes the Live structure check, its renders and the notes",
            Box::new(|c, a| {
                let folder = c.export.dir.join("set Project");
                let gz = std::fs::read(folder.join("set.als")).map_err(|e| format!("set.als: {e}"))?;
                let mut xml = String::new();
                std::io::Read::read_to_string(&mut flate2::read::GzDecoder::new(&gz[..]), &mut xml).map_err(|e| e.to_string())?;
                let sum = crate::live_set::validate(&xml, Some(&folder))?;
                let p = c.project();
                let with_clips = p.tracks.iter().filter(|t| !t.clips.is_empty() || !t.midi_clips.is_empty()).count();
                let midi: usize = p.tracks.iter().map(|t| t.midi_clips.len()).sum();
                let notes = std::fs::read_to_string(folder.join("DStudio Export Notes.txt")).unwrap_or_default();
                // Return B has a device and is fed, so it is rendered as one more audio clip.
                let wet = std::fs::read_dir(folder.join("Samples/Processed/DStudio"))
                    .map_or(0, |d| d.filter_map(|e| e.ok()).filter(|e| e.file_name().to_string_lossy().contains("Return B")).count());
                let sends = if with_clips > 1 { 3 } else { 2 };
                let _ = std::fs::remove_dir_all(&c.export.dir);
                (sum.audio_clips == with_clips + 1
                    && sum.midi_clips == midi
                    && sum.return_tracks == 2
                    && sum.sends == sends
                    && wet == 1
                    && !a.global::<Studio>().get_export_open())
                .then_some(())
                .ok_or(format!(
                    "{sum:?} vs {with_clips} rendered, {midi} MIDI clips, {sends} sends; Return B renders {wet}; tracks {:?}",
                    p.tracks.iter().map(|t| (t.name.as_str(), t.mute, t.devices.len(), t.sends.len())).collect::<Vec<_>>()
                ))?;
                ["Return A, Return B", "Returns with devices (Return B)", "Pre-fader sends", if with_clips > 1 { "Sends Only" } else { "" }]
                    .iter()
                    .try_for_each(|w| notes.contains(w).then_some(()).ok_or(format!("the notes do not mention '{w}': {notes}")))
            }),
        ),
        DoC(
            "undo the returns and sends",
            Box::new(|_a, ctl| {
                let mut c = ctl.borrow_mut();
                let depth = EXPORT_UNDO_DEPTH.with(|d| d.get());
                while c.doc.undo_depth() > depth {
                    c.undo();
                }
            }),
        ),
        Check(
            "the project has no return tracks again",
            Box::new(|c, _| (c.project().returns().count() == 0).then_some(()).ok_or("returns left".into())),
        ),
        Do("⌘N (file.new) with unsaved changes", Box::new(|a| key(a, "n", true, false))),
        Check(
            "Save changes? prompt open, project untouched",
            Box::new(|c, a| {
                let n: usize = c.project().tracks.iter().map(|t| t.clips.len()).sum();
                (a.global::<Studio>().get_unsaved_open() && n == 2)
                    .then_some(())
                    .ok_or(format!("prompt {} clips {n}", a.global::<Studio>().get_unsaved_open()))
            }),
        ),
        Do("Escape (Cancel)", Box::new(|a| key(a, Key::Escape, false, false))),
        Check(
            "prompt closed, project kept",
            Box::new(|c, a| {
                let n: usize = c.project().tracks.iter().map(|t| t.clips.len()).sum();
                (!a.global::<Studio>().get_unsaved_open() && !c.unsaved_open && n == 2).then_some(()).ok_or(format!("clips {n}"))
            }),
        ),
        Do(
            "drag track 1 volume slider left",
            Box::new(|a| drag(a, tracks_x() + 115.7 + 5.95 + 0.8 * 56.1, track0_y() + 47.6, tracks_x() + 140.0, track0_y() + 47.6)),
        ),
        Check(
            "track 1 gain lowered, one coalesced undo step",
            Box::new(|c, _| {
                let g = c.project().tracks[0].gain;
                let labels = c.doc.history_labels();
                let n = labels.iter().filter(|l| l.as_str() == "Track Volume Change").count();
                (g < 0.5 && n == 1).then_some(()).ok_or(format!("gain {g}, volume steps {n}"))
            }),
        ),
        Do("drag clip 1 body right by ~2 bars", Box::new(move |a| drag(a, bx(1.0), track0_y() + 40.0, bx(1.0) + 102.3, track0_y() + 40.0))),
        Check(
            "clip moved to beat 8 (snapped)",
            Box::new(|c, _| {
                let s = clip_start_beats(c, 0).ok_or("clip gone")?;
                ((s - 8.0).abs() < 1e-6).then_some(()).ok_or(format!("start {s}"))
            }),
        ),
        Do(
            "drag clip 1 right edge left by 2 beats",
            Box::new(move |a| drag(a, bx(16.0) - 2.0, track0_y() + 40.0, bx(14.0) - 2.0, track0_y() + 40.0)),
        ),
        Check(
            "clip trimmed to 6 beats (TrimClip)",
            Box::new(|c, _| {
                let p = c.project();
                let cl = &p.tracks[0].clips[0];
                let beats = p.clip_length_ticks(cl) as f64 / PPQ as f64;
                ((beats - 6.0).abs() < 0.01).then_some(()).ok_or(format!("len {beats} beats"))
            }),
        ),
        Do(
            "drag clip 1 left edge right by 1 beat",
            Box::new(move |a| drag(a, bx(8.0) + 1.5, track0_y() + 40.0, bx(9.0) + 1.5, track0_y() + 40.0)),
        ),
        Check(
            "left trim: start 9, offset 0.5 s of source, length 5 beats",
            Box::new(|c, _| {
                let p = c.project();
                let cl = &p.tracks[0].clips[0];
                let a = p.asset(cl.asset).unwrap();
                let off_s = cl.source_offset as f64 / a.sample_rate;
                let beats = p.clip_length_ticks(cl) as f64 / PPQ as f64;
                ((cl.start as f64 / PPQ as f64 - 9.0).abs() < 1e-6 && (off_s - 0.5).abs() < 1e-3 && (beats - 5.0).abs() < 0.01)
                    .then_some(())
                    .ok_or(format!("start {} off {off_s}s len {beats}", cl.start))
            }),
        ),
        Do("click ruler at bar 5", Box::new(move |a| click(a, bx(16.0), lane_y0() + 6.5))),
        Check(
            "engine playhead at bar 5 (Bar readout 005, Time 00:08.000)",
            Box::new(|_, a| {
                let s = a.global::<Studio>();
                (s.get_bar_text() == "005" && s.get_time_text() == "00:08.000").then_some(()).ok_or(format!(
                    "{} {}",
                    s.get_bar_text(),
                    s.get_time_text()
                ))
            }),
        ),
        Do("Enter (stop + return)", Box::new(|a| key(a, Key::Return, false, false))),
        Check(
            "playhead back at 0",
            Box::new(|c, _| (c.engine.transport().position == 0).then_some(()).ok_or(format!("pos {}", c.engine.transport().position))),
        ),
        Do("click loop button", Box::new(|a| click(a, transport_x() + 481.0, 52.0))),
        Check(
            "loop enabled in project + UI, undoable (SetLoop)",
            Box::new(|c, a| {
                let on = c.project().transport.loop_enabled;
                let ui = a.global::<Studio>().get_loop_enabled();
                (on && ui && c.doc.undo_label() == Some("Loop Change"))
                    .then_some(())
                    .ok_or(format!("model {on} ui {ui} undo {:?}", c.doc.undo_label()))
            }),
        ),
        Do("click loop button again", Box::new(|a| click(a, transport_x() + 481.0, 52.0))),
        Check("loop disabled", Box::new(|c, _| (!c.project().transport.loop_enabled).then_some(()).ok_or("still on".into()))),
        Do("click metronome button", Box::new(|a| click(a, transport_x() + 992.0, 52.0))),
        Check(
            "metronome on in controller + UI",
            Box::new(|c, a| (c.metronome_on && a.global::<Studio>().get_metronome_on()).then_some(()).ok_or("metronome off".into())),
        ),
        Check(
            "engine clicks land on the 120 BPM beat grid (offline render)",
            Box::new(|c, _| {
                if c.engine.device_running() {
                    return Ok(()); // offline render is refused while the device owns the audio path
                }
                let e = &c.engine;
                e.seek(0).map_err(|e| e.to_string())?;
                e.play().map_err(|e| e.to_string())?;
                let mut buf = vec![0f32; 2 * 48_100];
                let r = e.render_offline(&mut buf);
                let _ = e.stop();
                r.map_err(|e| e.to_string())?;
                let at = |f: usize| buf[2 * f].abs() + buf[2 * f + 1].abs();
                let loud = |a: usize, b: usize| (a..b).any(|f| at(f) > 1e-3);
                // Clips start at beats 9/10, so the first 2 s hold only clicks:
                // beat 0 at frame 0, beat 1 at 24000, beat 2 at 48000; silence between tails.
                (loud(0, 50) && !loud(2_000, 23_990) && loud(24_000, 24_050) && !loud(26_000, 47_990) && loud(48_000, 48_050))
                    .then_some(())
                    .ok_or("click positions wrong".into())
            }),
        ),
        Do("click metronome button again", Box::new(|a| click(a, transport_x() + 992.0, 52.0))),
        Check("metronome off", Box::new(|c, _| (!c.metronome_on).then_some(()).ok_or("still on".into()))),
        DoC(
            "select clip 1 and open the Clip tab",
            Box::new(move |a, ctl| {
                click(a, bx(11.0), track0_y() + 40.0);
                ctl.borrow_mut().set_bottom_tab(BottomTab::Clip);
            }),
        ),
        Check(
            "audio clip editor open: gain readout, no piano roll, clip-length ruler",
            Box::new(|c, a| {
                let s = a.global::<Studio>();
                let clip = c.ac_clip().ok_or("no audio clip open")?;
                let bars = (c.project().clip_length_ticks(clip) as f64 / c.project().ticks_per_bar() as f64).ceil() as usize;
                (s.get_ac_visible()
                    && !s.get_pr_visible()
                    && s.get_ac_gain_text() == "0.0 dB"
                    && s.get_pr_bar_labels().row_count() >= bars
                    && c.pr_view().gx == 0.0)
                    .then_some(())
                    .ok_or(format!(
                        "ac {} pr {} gain {} bars {}",
                        s.get_ac_visible(),
                        s.get_pr_visible(),
                        s.get_ac_gain_text(),
                        s.get_pr_bar_labels().row_count()
                    ))
            }),
        ),
        DoC(
            "audio clip editor: Resample",
            Box::new(|a, ctl| {
                {
                    let mut c = ctl.borrow_mut();
                    RESAMPLE_BEFORE.set((c.project().tracks.len(), c.ac_clip().map_or(0, |k| k.id.0)));
                    if c.clip_render.dir_override.is_none() {
                        c.clip_render.dir_override = Some(run_dir().join("renders"));
                    }
                }
                a.global::<Studio>().invoke_ac_resample();
            }),
        ),
        Check(
            "Resample: the clip rendered to a new '(Bounce)' track below, the source disabled, one undo step",
            Box::new(|c, _| {
                let (tracks, clip) = RESAMPLE_BEFORE.get();
                let p = c.project();
                let (ti, ci) = p.find_clip(ClipId(clip)).ok_or("source clip gone")?;
                let below = p.tracks.get(ti + 1).ok_or("no track below")?;
                let src = &p.tracks[ti].clips[ci];
                let ok = p.tracks.len() == tracks + 1
                    && below.name.ends_with("(Bounce)")
                    && below.clips.len() == 1
                    && below.clips[0].start == src.start
                    && src.disabled
                    && c.doc.undo_label() == Some("Bounce to New Track");
                ok.then_some(()).ok_or(format!(
                    "{} tracks (was {tracks}); below {:?} with {} clips; source disabled {}; undo {:?}; status {:?}",
                    p.tracks.len(),
                    below.name,
                    below.clips.len(),
                    src.disabled,
                    c.doc.undo_label(),
                    c.status_message()
                ))
            }),
        ),
        DoC(
            "Resample: undo, then open the source clip again",
            Box::new(|_, ctl| {
                let mut c = ctl.borrow_mut();
                c.undo();
                c.select_clip(Some(ClipId(RESAMPLE_BEFORE.get().1)));
                c.set_bottom_tab(BottomTab::Clip);
            }),
        ),
        Check(
            "Resample undone in one step: tracks as before, the source plays again",
            Box::new(|c, _| {
                let (tracks, clip) = RESAMPLE_BEFORE.get();
                let enabled = c.project().clip(ClipId(clip)).is_some_and(|k| !k.disabled);
                (c.project().tracks.len() == tracks && enabled && c.ac_clip().map(|k| k.id.0) == Some(clip))
                    .then_some(())
                    .ok_or(format!("{} tracks (was {tracks}); enabled {enabled}", c.project().tracks.len()))
            }),
        ),
        Check(
            "audio clip editor draws the clip waveform across the clip",
            Box::new(|c, _| {
                let sc = c.pr_scene(1.0, &[]).ok_or("no clip editor scene")?;
                let mut dl = crate::gpu::DrawList::default();
                crate::pianoroll::build(&mut dl, &sc);
                let (x0, x1) = (sc.view.x_of(sc.range.0), sc.view.x_of(sc.range.1).min(sc.view.w));
                let span = dl.waves.iter().map(|w| w.rect[2]).sum::<f32>();
                (dl.waves.len() == 1 && (span - (x1 - x0)).abs() <= 1.0).then_some(()).ok_or(format!(
                    "waves {} span {span} want {}",
                    dl.waves.len(),
                    x1 - x0
                ))
            }),
        ),
        Check(
            "clip editor ruler: bar numbers start at 1 and step by the ruler's label step",
            Box::new(|c, a| {
                let labels = a.global::<Studio>().get_pr_bar_labels();
                let step = c.pr_steps().label_bars;
                let nums: Vec<i64> =
                    (0..labels.row_count()).filter_map(|i| labels.row_data(i)).filter_map(|l| l.text.parse().ok()).collect();
                (nums.first() == Some(&1) && nums.windows(2).all(|w| w[1] - w[0] == step))
                    .then_some(())
                    .ok_or(format!("step {step} labels {nums:?}"))
            }),
        ),
        Check(
            "clip editor ruler: the editor's grid draws bar lines, and beat lines while the step is below a bar",
            Box::new(|c, _| {
                let sc = c.pr_scene(1.0, &[]).ok_or("no clip editor scene")?;
                let mut dl = crate::gpu::DrawList::default();
                crate::pianoroll::build(&mut dl, &sc);
                let alphas: Vec<f32> = dl.base.iter().filter(|r| r.rect[3] > 20.0 && r.rect[2] <= 1.01).map(|r| r.color[3]).collect();
                let bar = alphas.iter().filter(|a| (**a - 0.06).abs() < 1e-4).count();
                let beat = alphas.iter().filter(|a| (**a - 0.038).abs() < 1e-4).count();
                (bar > 0 && (beat > 0 || sc.steps.tick_beats >= sc.beats_per_bar))
                    .then_some(())
                    .ok_or(format!("bar lines {bar} beat lines {beat} of {}", alphas.len()))
            }),
        ),
        // ---- warp (SAIN-38): Speed / Warp / Mode menus, Original BPM drag, a warp marker ----
        DoC(
            "warp: Warp menu -> On",
            Box::new(|a, ctl| {
                ctl.borrow_mut().clip_menu_open(1);
                a.global::<Studio>().invoke_clip_menu_pick(0);
            }),
        ),
        Check(
            "warp on at the project tempo: same length, Mode enabled",
            Box::new(|c, a| {
                let s = a.global::<Studio>();
                let clip = c.ac_clip().ok_or("no audio clip open")?;
                let w = &clip.warp;
                let plain = c
                    .project()
                    .seconds_to_ticks(clip.source_length as f64 / c.project().asset(clip.asset).map_or(48_000.0, |x| x.sample_rate));
                let len = c.project().clip_length_ticks(clip);
                (w.enabled
                    && w.original_bpm == c.project().transport.bpm
                    && (len - plain).abs() <= 1
                    && s.get_ac_warped()
                    && s.get_ac_speed_text() == "1x"
                    && c.doc.undo_label() == Some("Set clip warp mode"))
                .then_some(())
                .ok_or(format!("warp {w:?} len {len} vs {plain} speed {}", s.get_ac_speed_text()))
            }),
        ),
        DoC(
            "warp: Speed menu -> 2x",
            Box::new(|a, ctl| {
                ctl.borrow_mut().clip_menu_open(0);
                a.global::<Studio>().invoke_clip_menu_pick(5);
            }),
        ),
        Check(
            "double speed halves the clip",
            Box::new(|c, a| {
                let clip = c.ac_clip().ok_or("no audio clip open")?;
                let plain = c
                    .project()
                    .seconds_to_ticks(clip.source_length as f64 / c.project().asset(clip.asset).map_or(48_000.0, |x| x.sample_rate));
                let len = c.project().clip_length_ticks(clip);
                let s = a.global::<Studio>();
                ((len - plain / 2).abs() <= 1 && s.get_ac_speed_text() == "2x" && c.doc.undo_label() == Some("Set clip speed"))
                    .then_some(())
                    .ok_or(format!("len {len} vs {plain} speed {}", s.get_ac_speed_text()))
            }),
        ),
        DoC(
            "warp: Mode menu -> Drums",
            Box::new(|a, ctl| {
                ctl.borrow_mut().clip_menu_open(2);
                a.global::<Studio>().invoke_clip_menu_pick(1);
            }),
        ),
        Check(
            "mode Drums",
            Box::new(|c, a| {
                let clip = c.ac_clip().ok_or("no audio clip open")?;
                (clip.warp.mode == dstudio_core::WarpMode::Drums && a.global::<Studio>().get_ac_mode_text() == "Drums" && c.pr.menu == -1)
                    .then_some(())
                    .ok_or(format!("mode {:?}", clip.warp.mode))
            }),
        ),
        Do(
            "warp: drag Original BPM right 20 pt",
            Box::new(|a| {
                let s = a.global::<Studio>();
                s.invoke_ac_bpm_drag(0, 0.0, 0.0);
                s.invoke_ac_bpm_drag(1, 10.0, 0.0);
                s.invoke_ac_bpm_drag(1, 20.0, 0.0);
                s.invoke_ac_bpm_drag(2, 20.0, 0.0);
            }),
        ),
        Check(
            "original BPM +20, one undo step",
            Box::new(|c, a| {
                let clip = c.ac_clip().ok_or("no audio clip open")?;
                let want = c.project().transport.bpm + 20.0;
                (clip.warp.original_bpm == want
                    && a.global::<Studio>().get_ac_bpm_text() == format!("{}", want.round())
                    && c.doc.undo_label() == Some("Set original BPM"))
                .then_some(())
                .ok_or(format!("bpm {} label {:?}", clip.warp.original_bpm, c.doc.undo_label()))
            }),
        ),
        DoC(
            "warp: click the handle band at beat 1 (add a warp marker)",
            Box::new(|_, ctl| {
                let x = ctl.borrow().pr_view().x_of(1.0);
                let mut c = ctl.borrow_mut();
                c.pr_pointer(0, x, 30.0, false, true, false);
                c.pr_pointer(2, x, 30.0, false, true, false);
            }),
        ),
        Check(
            "warp marker added (with the clip-start anchor)",
            Box::new(|c, _| {
                let clip = c.ac_clip().ok_or("no audio clip open")?;
                let beats = c.ac_marker_beats();
                (clip.warp.markers.len() == 2
                    && beats.len() == 2
                    && beats[0].abs() < 1e-6
                    && (beats[1] - 1.0).abs() < 0.05
                    && c.doc.undo_label() == Some("Add warp marker"))
                .then_some(())
                .ok_or(format!("markers {:?} beats {beats:?}", clip.warp.markers))
            }),
        ),
        Check(
            "the clip editor waveform bends at the warp marker",
            Box::new(|c, _| {
                let sc = c.pr_scene(1.0, &[]).ok_or("no clip editor scene")?;
                let segs = sc.wave.as_ref().map(|w| w.1.clone()).unwrap_or_default();
                let marker = c.ac_marker_beats().get(1).copied().unwrap_or(f64::NAN);
                let mut dl = crate::gpu::DrawList::default();
                crate::pianoroll::build(&mut dl, &sc);
                let lanes = sc.wave.as_ref().map_or(0, |w| w.0.len());
                (segs.len() == 2
                    && dl.waves.len() == 2 * lanes
                    && (segs[0].beats.1 - marker).abs() < 1e-6
                    && dl.waves[0].rect[0] + dl.waves[0].rect[2] == dl.waves[1].rect[0])
                    .then_some(())
                    .ok_or(format!("segs {segs:?} marker {marker} waves {}", dl.waves.len()))
            }),
        ),
        Check(
            "a mono clip keeps one lane",
            Box::new(|c, _| {
                let clip = c.ac_clip().ok_or("no audio clip open")?;
                let lanes = c.clip_lanes(clip).map_or(0, |l| l.0.len());
                (c.project().asset(clip.asset).is_some_and(|a| a.channels == 1) && lanes == 1).then_some(()).ok_or(format!("lanes {lanes}"))
            }),
        ),
        DoC(
            "open the demo's stereo noise clip in the Clip tab",
            Box::new(|_, ctl| {
                let mut c = ctl.borrow_mut();
                let p = c.project();
                let id =
                    p.tracks.iter().flat_map(|t| &t.clips).find(|cl| p.asset(cl.asset).is_some_and(|a| a.channels == 2)).map(|cl| cl.id);
                c.select_clip(id);
                c.set_bottom_tab(BottomTab::Clip);
            }),
        ),
        Check(
            "a stereo clip draws a lane per channel: left above right, each from its own peaks",
            Box::new(|c, _| {
                let clip = c.ac_clip().ok_or("no audio clip open")?;
                let stereo = c.project().asset(clip.asset).is_some_and(|a| a.channels == 2);
                let (slots, _) = c.clip_lanes(clip).ok_or("no waveform")?;
                let ea = *c.asset_map.get(&clip.asset).ok_or("no engine asset")?;
                let sc = c.pr_scene(1.0, &[]).ok_or("no clip editor scene")?;
                let mut dl = crate::gpu::DrawList::default();
                crate::pianoroll::build(&mut dl, &sc);
                let (top, bottom) = (dl.waves.first().map(|w| w.rect[1]), dl.waves.last().map(|w| w.rect[1]));
                (stereo && slots.len() == 2 && Some(slots) == c.peaks.channel_slots(ea).map(|s| s.to_vec()) && top < bottom)
                    .then_some(())
                    .ok_or(format!("stereo {stereo} lanes {} tops {top:?} {bottom:?}", c.clip_lanes(clip).map_or(0, |l| l.0.len())))
            }),
        ),
        DoC(
            "back to clip 1 in the Clip tab",
            Box::new(|_, ctl| {
                let mut c = ctl.borrow_mut();
                let id = c.project().tracks[0].clips.first().map(|cl| cl.id);
                c.select_clip(id);
                c.set_bottom_tab(BottomTab::Clip);
            }),
        ),
        DoC(
            "warp: Shift-drag the marker half a beat right (slide the audio)",
            Box::new(|_, ctl| {
                let (x0, x1) = {
                    let c = ctl.borrow();
                    let v = c.pr_view();
                    (v.x_of(1.0), v.x_of(1.5))
                };
                let mut c = ctl.borrow_mut();
                c.pr_pointer(0, x0, 30.0, true, false, false);
                c.pr_pointer(1, x1, 30.0, false, false, false);
                c.pr_pointer(2, x1, 30.0, true, false, false);
            }),
        ),
        Check(
            "slid: the marker stays at beat 1 and pins audio half a beat later",
            Box::new(|c, _| {
                let clip = c.ac_clip().ok_or("no audio clip open")?;
                let beats = c.ac_marker_beats();
                let rate = c.project().asset(clip.asset).unwrap().sample_rate;
                let per_beat = rate * 60.0 / clip.warp.original_bpm * clip.warp.speed;
                let src = clip.warp.markers.get(1).map_or(0.0, |m| m.source as f64);
                let want = clip.source_offset as f64 + 1.5 * per_beat;
                ((beats.get(1).copied().unwrap_or(f64::NAN) - 1.0).abs() < 0.05
                    && (src - want).abs() < 0.02 * per_beat
                    && c.doc.undo_label() == Some("Move warp marker"))
                .then_some(())
                .ok_or(format!("beats {beats:?} source {src} want {want} undo {:?}", c.doc.undo_label()))
            }),
        ),
        DoC(
            "warp: three transients, Cmd-click the middle one in the handle band",
            Box::new(|_, ctl| {
                let mut c = ctl.borrow_mut();
                let Some(clip) = c.ac_clip().cloned() else { return };
                let rate = c.project().asset(clip.asset).unwrap().sample_rate;
                let per_beat = rate * 60.0 / clip.warp.original_bpm * clip.warp.speed;
                let at = |b: f64| (clip.source_offset as f64 + b * per_beat) as u64;
                let warp = dstudio_core::ClipWarp { transients_added: vec![at(2.25), at(2.5), at(2.75)], ..clip.warp.clone() };
                c.execute(dstudio_core::Command::SetClipWarp { clip: clip.id, warp, label: "Add transient".into() });
                drop(c);
                let x = ctl.borrow().ac_transient_beats().get(1).map(|&b| ctl.borrow().pr_view().x_of(b)).unwrap_or(0.0);
                let mut c = ctl.borrow_mut();
                c.pr_pointer(0, x, 30.0, false, false, true);
                c.pr_pointer(2, x, 30.0, false, false, true);
            }),
        ),
        Check(
            "Add warp markers at transients: the transient and both neighbours, where they play",
            Box::new(|c, _| {
                let clip = c.ac_clip().ok_or("no audio clip open")?;
                let marks = c.ac_marker_beats();
                let tr = c.ac_transient_beats();
                let pinned = tr.iter().filter(|t| marks.iter().any(|m| (m - *t).abs() < 1e-6)).count();
                (clip.warp.markers.len() == 5 && pinned == 3 && c.doc.undo_label() == Some("Add warp marker at transient"))
                    .then_some(())
                    .ok_or(format!("markers {marks:?} transients {tr:?} undo {:?}", c.doc.undo_label()))
            }),
        ),
        DoC(
            "warp: undo, then press the first transient and drag a quarter beat right in one gesture",
            Box::new(move |_, ctl| {
                ctl.borrow_mut().undo();
                let b = ctl.borrow().ac_transient_beats().first().copied().unwrap_or(0.0);
                press_beat.set(b);
                let (x0, x1) = {
                    let c = ctl.borrow();
                    let v = c.pr_view();
                    (v.x_of(b), v.x_of(b + 0.25))
                };
                let mut c = ctl.borrow_mut();
                c.pr_pointer(0, x0, 30.0, false, false, false);
                c.pr_pointer(1, x1, 30.0, false, false, false);
                c.pr_pointer(2, x1, 30.0, false, false, false);
            }),
        ),
        Check(
            "the press pinned the transient and the drag moved its marker: one undo step",
            Box::new(move |c, _| {
                let clip = c.ac_clip().ok_or("no audio clip open")?;
                let marks = c.ac_marker_beats();
                let tr = c.ac_transient_beats();
                let pinned = tr.first().is_some_and(|t| marks.iter().any(|m| (m - t).abs() < 1e-6));
                let want = press_beat2.get() + 0.25;
                (clip.warp.markers.len() == 3
                    && pinned
                    && (tr[0] - want).abs() < 0.02
                    && c.doc.undo_label() == Some("Add warp marker at transient"))
                .then_some(())
                .ok_or(format!("markers {marks:?} transients {tr:?} undo {:?}", c.doc.undo_label()))
            }),
        ),
        DoC("warp: undo once", Box::new(|_, ctl| ctl.borrow_mut().undo())),
        Check(
            "one undo removes the placed and dragged marker",
            Box::new(|c, _| {
                let clip = c.ac_clip().ok_or("no audio clip open")?;
                (clip.warp.markers.len() == 2).then_some(()).ok_or(format!("markers {:?}", clip.warp.markers))
            }),
        ),
        DoC("warp: undo the transients", Box::new(|_, ctl| ctl.borrow_mut().undo())),
        DoC(
            "warp: Warp menu -> Off",
            Box::new(|a, ctl| {
                ctl.borrow_mut().clip_menu_open(1);
                a.global::<Studio>().invoke_clip_menu_pick(1);
            }),
        ),
        Check(
            "warp off resets the speed and plays 1:1; Mode disabled",
            Box::new(|c, a| {
                let clip = c.ac_clip().ok_or("no audio clip open")?;
                let plain = c
                    .project()
                    .seconds_to_ticks(clip.source_length as f64 / c.project().asset(clip.asset).map_or(48_000.0, |x| x.sample_rate));
                let s = a.global::<Studio>();
                (!clip.warp.enabled
                    && clip.warp.speed == 1.0
                    && c.project().clip_length_ticks(clip) == plain
                    && !s.get_ac_warped()
                    && c.doc.undo_label() == Some("Reset clip speed"))
                .then_some(())
                .ok_or(format!("warp {:?}", clip.warp))
            }),
        ),
        DoC("warp: Mode menu stays closed with warp off", Box::new(|_, ctl| ctl.borrow_mut().clip_menu_open(2))),
        Check("mode menu closed", Box::new(|c, _| (c.pr.menu == -1).then_some(()).ok_or(format!("menu {}", c.pr.menu)))),
        Do(
            "Cmd+Z x7 (back to the unwarped clip)",
            Box::new(|a| {
                for _ in 0..7 {
                    key(a, "z", true, false);
                }
            }),
        ),
        Check(
            "warp undone",
            Box::new(|c, _| {
                let clip = c.ac_clip().ok_or("no audio clip open")?;
                clip.warp.is_default().then_some(()).ok_or(format!("warp {:?}", clip.warp))
            }),
        ),
        DoC("reverse: toolbar Reverse -> On", Box::new(|_, ctl| ctl.borrow_mut().ac_reverse_toggle())),
        Check(
            "reversed: toolbar On, the engine's reversed copy exists and the editor draws it, one undo step",
            Box::new(|c, a| {
                let clip = c.ac_clip().ok_or("no audio clip open")?;
                let rev = c.reversed_assets.get(&clip.asset).copied().ok_or("no reversed copy")?;
                let slot = c.clip_wave(clip).map(|w| w.0.base);
                (clip.warp.reverse
                    && a.global::<Studio>().get_ac_reversed()
                    && slot == c.peaks.slot(rev).map(|s| s.base)
                    && c.doc.undo_label() == Some("Toggle clip reverse"))
                .then_some(())
                .ok_or(format!(
                    "reverse {} ui {} slot {slot:?} undo {:?}",
                    clip.warp.reverse,
                    a.global::<Studio>().get_ac_reversed(),
                    c.doc.undo_label()
                ))
            }),
        ),
        Do(
            "reversed: drag clip 1 left edge right by 1 beat",
            Box::new(move |a| drag(a, bx(9.0) + 1.5, track0_y() + 40.0, bx(10.0) + 1.5, track0_y() + 40.0)),
        ),
        Check(
            "reversed left trim cuts the window's end: start 10, offset still 0.5 s, 4 beats",
            Box::new(|c, _| {
                let p = c.project();
                let cl = &p.tracks[0].clips[0];
                let a = p.asset(cl.asset).unwrap();
                let off_s = cl.source_offset as f64 / a.sample_rate;
                let beats = p.clip_length_ticks(cl) as f64 / PPQ as f64;
                ((cl.start as f64 / PPQ as f64 - 10.0).abs() < 1e-6 && (off_s - 0.5).abs() < 1e-3 && (beats - 4.0).abs() < 0.01)
                    .then_some(())
                    .ok_or(format!("start {} off {off_s}s len {beats}", cl.start))
            }),
        ),
        DoC(
            "reverse: undo the trim and the toggle",
            Box::new(|_, ctl| {
                let mut c = ctl.borrow_mut();
                c.undo();
                c.undo();
            }),
        ),
        Check(
            "reverse undone",
            Box::new(|c, a| {
                let cl = &c.project().tracks[0].clips[0];
                (!cl.warp.reverse && !a.global::<Studio>().get_ac_reversed() && cl.start == 9 * PPQ)
                    .then_some(())
                    .ok_or(format!("reverse {} start {}", cl.warp.reverse, cl.start))
            }),
        ),
        DoC(
            "transpose: drag the Transpose value 50 pt right",
            Box::new(|_, ctl| {
                let mut c = ctl.borrow_mut();
                c.ac_transpose_drag(0, 0.0, 0.0);
                c.ac_transpose_drag(1, 20.0, 0.0);
                c.ac_transpose_drag(1, 50.0, 0.0);
                c.ac_transpose_drag(2, 50.0, 0.0);
            }),
        ),
        Check(
            "transpose +12 with warp off: an octave up, half as long, one undo step",
            Box::new(|c, a| {
                let s = a.global::<Studio>();
                let clip = c.ac_clip().ok_or("no audio clip open")?;
                let plain = dstudio_core::Clip { warp: dstudio_core::ClipWarp::default(), ..clip.clone() };
                let (len, full) = (c.project().clip_length_ticks(clip), c.project().clip_length_ticks(&plain));
                (clip.warp.transpose == 12
                    && s.get_ac_transpose_text() == "+12 st"
                    && s.get_ac_transpose_enabled()
                    && len * 2 == full
                    && c.doc.undo_label() == Some("Adjust clip transpose"))
                .then_some(())
                .ok_or(format!(
                    "transpose {} text {} len {len} of {full} undo {:?}",
                    clip.warp.transpose,
                    s.get_ac_transpose_text(),
                    c.doc.undo_label()
                ))
            }),
        ),
        DoC(
            "transpose: Re-pitch disables it",
            Box::new(|_, ctl| {
                let mut c = ctl.borrow_mut();
                let Some(clip) = c.ac_clip().cloned() else { return };
                let warp = dstudio_core::ClipWarp {
                    enabled: true,
                    mode: dstudio_core::WarpMode::Repitch,
                    original_bpm: 60.0,
                    ..clip.warp.clone()
                };
                c.execute(dstudio_core::Command::SetClipWarp { clip: clip.id, warp, label: "Set clip mode".into() });
            }),
        ),
        Check(
            "Re-pitch: Transpose greyed, shows the tempo ratio (+12 st at 120 over 60 BPM)",
            Box::new(|_, a| {
                let s = a.global::<Studio>();
                (!s.get_ac_transpose_enabled() && s.get_ac_transpose_text() == "+12 st").then_some(()).ok_or(format!(
                    "enabled {} text {}",
                    s.get_ac_transpose_enabled(),
                    s.get_ac_transpose_text()
                ))
            }),
        ),
        DoC(
            "transpose: undo twice",
            Box::new(|_, ctl| {
                let mut c = ctl.borrow_mut();
                c.undo();
                c.undo();
            }),
        ),
        Check(
            "transpose undone",
            Box::new(|c, a| {
                let clip = c.ac_clip().ok_or("no audio clip open")?;
                (clip.warp.is_default() && a.global::<Studio>().get_ac_transpose_text() == "0 st")
                    .then_some(())
                    .ok_or(format!("warp {:?}", clip.warp))
            }),
        ),
        Do(
            "audio loop: click the footer Loop switch",
            Box::new(|a| click(a, pr_origin().0 + 202.0 + 10.0, bottom_end() - 54.5 + 13.0 + 7.0)),
        ),
        Check(
            "Loop on over the whole content: same length, one \"Clip Loop\" step, footer Loop on",
            Box::new(|c, a| {
                let clip = c.ac_clip().ok_or("no audio clip open")?;
                let content = c.project().content_length_ticks(clip);
                let l = clip.clip_loop.ok_or("no loop")?;
                (l.on
                    && l.start == 0
                    && l.end == content
                    && l.length == content
                    && c.project().clip_length_ticks(clip) == content
                    && c.doc.undo_label() == Some("Clip Loop")
                    && a.global::<Studio>().get_pr_loop_on())
                .then_some(())
                .ok_or(format!("{l:?} content {content} {:?}", c.doc.undo_label()))
            }),
        ),
        DoC(
            "audio loop: loop the second quarter of the content",
            Box::new(|_, ctl| {
                let mut c = ctl.borrow_mut();
                let Some(clip) = c.ac_clip().cloned() else { return };
                let content = c.project().content_length_ticks(&clip);
                let l = dstudio_core::model::AudioLoop { on: true, start: content / 4, end: content / 2, length: content };
                c.execute(dstudio_core::Command::SetClipLoop { clip: clip.id, clip_loop: Some(l) });
            }),
        ),
        DoC(
            "audio loop: drag the loop bar's end from half to three quarters of the content",
            Box::new(|a, ctl| {
                AUDIO_LOOP_HISTORY.with(|h| h.set(ctl.borrow().doc.history_labels().len()));
                let (x0, x1) = {
                    let c = ctl.borrow();
                    let Some(clip) = c.ac_clip() else { return };
                    let content = c.project().content_length_ticks(clip) as f64 / PPQ as f64;
                    let v = c.pr_view();
                    (pr_origin().0 + v.x_of(content / 2.0), pr_origin().0 + v.x_of(content * 0.75))
                };
                let y = pr_origin().1 + 20.5;
                drag(a, x0, y, x1, y);
            }),
        ),
        Check(
            "the loop ends near three quarters (on the snap step), its start kept, one \"Clip Loop\" step",
            Box::new(|c, _| {
                let clip = c.ac_clip().ok_or("no audio clip open")?;
                let content = c.project().content_length_ticks(clip);
                let l = clip.clip_loop.ok_or("no loop")?;
                let g = c.pr_snap_ticks(&c.pr_view());
                (l.on
                    && l.start == content / 4
                    && (l.end - content * 3 / 4).abs() <= g
                    && (l.end % g == 0 || l.end == content)
                    && c.doc.undo_label() == Some("Clip Loop"))
                .then_some(())
                .ok_or(format!("{l:?} content {content}"))
            }),
        ),
        DoC(
            "audio loop: drag the middle of the loop bar right by an eighth of the content",
            Box::new(|a, ctl| {
                let (x0, x1) = {
                    let c = ctl.borrow();
                    let Some(l) = c.ac_clip().and_then(|clip| clip.clip_loop) else { return };
                    let content = c.ac_clip().map_or(0, |clip| c.project().content_length_ticks(clip)) as f64 / PPQ as f64;
                    let v = c.pr_view();
                    let mid = (l.start + l.end) as f64 / 2.0 / PPQ as f64;
                    (pr_origin().0 + v.x_of(mid), pr_origin().0 + v.x_of(mid + content / 8.0))
                };
                let y = pr_origin().1 + 20.5;
                drag(a, x0, y, x1, y);
            }),
        ),
        Check(
            "the whole loop moved later on the grid, length kept, inside the content",
            Box::new(|c, _| {
                let clip = c.ac_clip().ok_or("no audio clip open")?;
                let content = c.project().content_length_ticks(clip);
                let l = clip.clip_loop.ok_or("no loop")?;
                let d = l.start - content / 4;
                let g = c.pr_snap_ticks(&c.pr_view());
                (d > 0
                    && (d % g == 0 || l.end == content)
                    && (d - content / 8).abs() <= g
                    && (l.end - l.start - (content * 3 / 4 - content / 4)).abs() <= g
                    && l.end <= content)
                    .then_some(())
                    .ok_or(format!("{l:?} content {content}"))
            }),
        ),
        DoC(
            "audio loop: drag the footer Position value up 16 pt (two beats later)",
            Box::new(|a, ctl| {
                let mem = ctl.borrow().ac_clip().and_then(|clip| clip.clip_loop).map_or(0, |l| l.start);
                AUDIO_LOOP_START.with(|m| m.set(mem));
                let (x, y) = (pr_origin().0 + 216.5 + 8.0, bottom_end() - 54.5 + 30.0 + 7.0);
                drag(a, x, y, x, y - 16.0);
            }),
        ),
        Check(
            "Position moved the loop two beats later (or to the content's end), length kept, one step",
            Box::new(|c, a| {
                let clip = c.ac_clip().ok_or("no audio clip open")?;
                let content = c.project().content_length_ticks(clip);
                let l = clip.clip_loop.ok_or("no loop")?;
                let (before, len) = (AUDIO_LOOP_START.with(|m| m.get()), l.end - l.start);
                let want = (before + 2 * PPQ).min(content - len);
                let tpb = c.project().ticks_per_bar();
                (l.start == want
                    && a.global::<Studio>().get_pr_position_text() == crate::pianoroll::fmt_pos(want, tpb)
                    && c.doc.undo_label() == Some("Clip Loop"))
                .then_some(())
                .ok_or(format!("{l:?} want start {want} content {content}"))
            }),
        ),
        DoC(
            "audio loop: undo the bar and Position drags",
            Box::new(|_, ctl| {
                let mut c = ctl.borrow_mut();
                while c.doc.history_labels().len() > AUDIO_LOOP_HISTORY.with(|h| h.get()) {
                    c.undo();
                }
            }),
        ),
        Check(
            "audio loop back on the second quarter",
            Box::new(|c, _| {
                let clip = c.ac_clip().ok_or("no audio clip open")?;
                let content = c.project().content_length_ticks(clip);
                let l = clip.clip_loop.ok_or("no loop")?;
                (l.start == content / 4 && l.end == content / 2).then_some(()).ok_or(format!("{l:?}"))
            }),
        ),
        DoC(
            "audio loop: drag the clip's right edge out to twice its content",
            Box::new(move |a, ctl| {
                let (x0, x1, y) = {
                    let c = ctl.borrow();
                    let Some(clip) = c.ac_clip() else { return };
                    let (s, content) = (clip.start as f64 / PPQ as f64, c.project().content_length_ticks(clip) as f64 / PPQ as f64);
                    (
                        lane_x() + c.view.x_of(s + content),
                        lane_x() + c.view.x_of(s + 2.0 * content),
                        track0_y() + 40.0 - c.view.scroll_y as f32,
                    )
                };
                drag(a, x0 - 2.0, y, x1 - 2.0, y);
            }),
        ),
        Check(
            "the clip loops out to about twice its content in one step; waveform and footer follow",
            Box::new(|c, a| {
                let clip = c.ac_clip().ok_or("no audio clip open")?;
                let content = c.project().content_length_ticks(clip);
                let len = c.project().clip_length_ticks(clip);
                let i = c.clip_ids.iter().position(|&id| id == clip.id).ok_or("clip not drawn")?;
                let segs = crate::peaks::wave_segments(c.project(), clip, c.rate);
                let last = segs.last().map_or(0.0, |s| s.beats.1);
                let s = a.global::<Studio>();
                let tpb = c.project().ticks_per_bar();
                let ok = (len - 2 * content).abs() <= PPQ / 2
                    && clip.clip_loop.is_some_and(|l| l.on && l.length == len)
                    && c.doc.undo_label() == Some("Clip Loop")
                    && (c.clip_vis[i].len_beats - len as f64 / PPQ as f64).abs() < 1e-6
                    && (last - len as f64 / PPQ as f64).abs() < 1e-6
                    && segs.len() > 2
                    && s.get_pr_position_text() == crate::pianoroll::fmt_pos(content / 4, tpb)
                    && s.get_pr_length_text() == crate::pianoroll::fmt_len(content / 2 - content / 4, tpb);
                ok.then_some(()).ok_or(format!(
                    "len {len} of {content}, {} pieces to {last}, pos {} length {} {:?}",
                    segs.len(),
                    s.get_pr_position_text(),
                    s.get_pr_length_text(),
                    c.doc.undo_label()
                ))
            }),
        ),
        Do("audio loop: click the Loop switch off", Box::new(|a| click(a, pr_origin().0 + 202.0 + 10.0, bottom_end() - 54.5 + 13.0 + 7.0))),
        Check(
            "Loop off: the source window plays once; the range and looped length are kept",
            Box::new(|c, a| {
                let clip = c.ac_clip().ok_or("no audio clip open")?;
                let content = c.project().content_length_ticks(clip);
                let l = clip.clip_loop.ok_or("no loop")?;
                (!l.on
                    && l.length > content
                    && l.start == content / 4
                    && c.project().clip_length_ticks(clip) == content
                    && !a.global::<Studio>().get_pr_loop_on())
                .then_some(())
                .ok_or(format!("{l:?}"))
            }),
        ),
        DoC(
            "audio loop: undo the four loop edits",
            Box::new(|_, ctl| {
                let mut c = ctl.borrow_mut();
                for _ in 0..4 {
                    c.undo();
                }
            }),
        ),
        Check(
            "audio loop undone",
            Box::new(|c, _| {
                let clip = c.ac_clip().ok_or("no audio clip open")?;
                clip.clip_loop.is_none().then_some(()).ok_or(format!("{:?}", clip.clip_loop))
            }),
        ),
        DoC("original BPM: double-click to type", Box::new(|_, ctl| ctl.borrow_mut().ac_edit_start(crate::warpedit::AcEdit::OriginalBpm))),
        Check(
            "Original BPM edit box open with the shown value",
            Box::new(|_, a| {
                let s = a.global::<Studio>();
                (s.get_ac_edit_field() == 0 && s.get_ac_edit_text() == "120").then_some(()).ok_or(format!(
                    "field {} text {}",
                    s.get_ac_edit_field(),
                    s.get_ac_edit_text()
                ))
            }),
        ),
        DoC(
            "original BPM: type 96.5, Enter, then reopen it (shows 97) and click away",
            Box::new(|_, ctl| {
                let mut c = ctl.borrow_mut();
                c.ac_edit_commit("96.5");
                c.ac_edit_start(crate::warpedit::AcEdit::OriginalBpm);
                let shown = c.ac_clip().cloned().map(|clip| c.ac_edit_initial(&clip)).unwrap_or_default();
                c.ac_edit_commit(&shown);
            }),
        ),
        Check(
            "typed BPM turns warp on at 96.5: the clip follows 120 BPM (longer), one undo step",
            Box::new(|c, a| {
                let clip = c.ac_clip().ok_or("no audio clip open")?;
                let p = c.project();
                let secs = clip.source_length as f64 / p.asset(clip.asset).unwrap().sample_rate;
                let beats = p.clip_length_ticks(clip) as f64 / PPQ as f64;
                (clip.warp.enabled
                    && clip.warp.original_bpm == 96.5
                    && (beats - secs * 96.5 / 60.0).abs() < 0.01
                    && a.global::<Studio>().get_ac_edit_field() == -1
                    && c.doc.undo_label() == Some("Set original BPM"))
                .then_some(())
                .ok_or(format!("warp {:?} beats {beats} undo {:?}", clip.warp, c.doc.undo_label()))
            }),
        ),
        DoC(
            "original BPM: clear the field, Enter",
            Box::new(|_, ctl| {
                let mut c = ctl.borrow_mut();
                c.ac_edit_start(crate::warpedit::AcEdit::OriginalBpm);
                c.ac_edit_commit("");
            }),
        ),
        Check(
            "an empty BPM turns warp off (Clear original BPM)",
            Box::new(|c, _| {
                let clip = c.ac_clip().ok_or("no audio clip open")?;
                (!clip.warp.enabled && c.doc.undo_label() == Some("Clear original BPM")).then_some(()).ok_or(format!(
                    "warp {:?} undo {:?}",
                    clip.warp,
                    c.doc.undo_label()
                ))
            }),
        ),
        DoC(
            "transpose: type -7, then nonsense, then Escape, then Enter on an edit opened for another clip",
            Box::new(|_, ctl| {
                let mut c = ctl.borrow_mut();
                c.ac_edit_start(crate::warpedit::AcEdit::Transpose);
                c.ac_edit_commit("-7");
                c.ac_edit_start(crate::warpedit::AcEdit::Transpose);
                c.ac_edit_commit("abc");
                c.ac_edit_start(crate::warpedit::AcEdit::Transpose);
                c.ac_edit_cancel();
                c.ac_edit_start(crate::warpedit::AcEdit::Transpose);
                if let Some(e) = c.pr.ac_edit.as_mut() {
                    e.1 = dstudio_core::ClipId(u64::MAX);
                }
                c.ac_edit_commit("5");
            }),
        ),
        Check(
            "typed transpose -7 st; nonsense, Escape and another clip's edit change nothing",
            Box::new(|c, a| {
                let clip = c.ac_clip().ok_or("no audio clip open")?;
                let s = a.global::<Studio>();
                (clip.warp.transpose == -7
                    && s.get_ac_transpose_text() == "-7 st"
                    && s.get_ac_edit_field() == -1
                    && c.doc.undo_label() == Some("Set clip pitch shift"))
                .then_some(())
                .ok_or(format!(
                    "transpose {} text {} field {} undo {:?}",
                    clip.warp.transpose,
                    s.get_ac_transpose_text(),
                    s.get_ac_edit_field(),
                    c.doc.undo_label()
                ))
            }),
        ),
        DoC(
            "typed values: undo three times",
            Box::new(|_, ctl| {
                let mut c = ctl.borrow_mut();
                for _ in 0..3 {
                    c.undo();
                }
            }),
        ),
        Check(
            "typed values undone",
            Box::new(|c, _| {
                let clip = c.ac_clip().ok_or("no audio clip open")?;
                clip.warp.is_default().then_some(()).ok_or(format!("warp {:?}", clip.warp))
            }),
        ),
        DoC("back to the Devices tab", Box::new(|_, ctl| ctl.borrow_mut().set_bottom_tab(BottomTab::Devices))),
        Do(
            "select clip 1 and press Delete",
            Box::new(move |a| {
                click(a, bx(11.0), track0_y() + 40.0);
                key(a, Key::Delete, false, false)
            }),
        ),
        Check(
            "clip deleted (RemoveClip)",
            Box::new(|c, _| c.project().tracks[0].clips.is_empty().then_some(()).ok_or("clip still there".into())),
        ),
        Do("Cmd+Z", Box::new(|a| key(a, "z", true, false))),
        Check(
            "undo restored the clip",
            Box::new(|c, _| (c.project().tracks[0].clips.len() == 1).then_some(()).ok_or("not restored".into())),
        ),
        Do("click 'Add track'", Box::new(|a| click(a, tracks_x() + 37.35, geo().timeline_y + 45.8))),
        Check(
            "the Add track menu offers Audio, Instrument, Return and Group tracks",
            Box::new(|c, _| {
                let l = c.route_menu_labels();
                (l == ["Audio track", "Instrument track", "Return track", "Group track"]).then_some(()).ok_or(format!("{l:?}"))
            }),
        ),
        DoC(
            "pick 'Audio track'",
            Box::new(|a, ctl| {
                let (x, y) = route_row(ctl, 0);
                click(a, x, y)
            }),
        ),
        Check(
            "4 tracks, new one is Audio and selected",
            Box::new(|c, _| {
                let p = c.project();
                let t = p.tracks.last().unwrap();
                (p.tracks.len() == 4 && t.kind == TrackKind::Audio && c.selected_track == Some(t.id))
                    .then_some(())
                    .ok_or(format!("{} tracks", p.tracks.len()))
            }),
        ),
        Do("Delete (track header selected)", Box::new(|a| key(a, Key::Backspace, false, false))),
        Check(
            "track deleted (RemoveTrack)",
            Box::new(|c, _| (c.project().tracks.len() == 3).then_some(()).ok_or(format!("{} tracks", c.project().tracks.len()))),
        ),
        Do("click track 1 header", Box::new(|a| click(a, tracks_x() + 37.35, track0_y() + 10.0))),
        Do(
            "drag inspector pan knob up 30 pt",
            Box::new(|a| drag(a, bottom_x() + 47.0, bottom_y() + 210.0, bottom_x() + 47.0, bottom_y() + 210.0 - 30.0)),
        ),
        Check(
            "track 1 pan moved right, readout updated",
            Box::new(|c, a| {
                let pan = c.project().tracks[0].pan;
                let txt = a.global::<Studio>().get_sel_pan_text();
                (pan > 0.2 && txt.ends_with('R')).then_some(()).ok_or(format!("pan {pan} text {txt}"))
            }),
        ),
        Do("(audio) unmute track 1 (muted by the redo step)", Box::new(|a| click(a, tracks_x() + 33.5, track0_y() + 47.0))),
        Do("(audio) click ruler at beat 9 (start of track 1 clip)", Box::new(move |a| click(a, bx(9.0), lane_y0() + 6.5))),
        Do(
            "(audio) master -40 dB, Space",
            Box::new(|a| {
                a.global::<Studio>().invoke_master_volume(crate::controller::slider_from_gain(0.01));
                key(a, " ", false, false)
            }),
        ),
        Do("wait", Box::new(|_| {})),
        Do("wait", Box::new(|_| {})),
        Do("wait", Box::new(|_| {})),
        Do("wait", Box::new(|_| {})),
        Do("wait", Box::new(|_| {})),
        Do("wait", Box::new(|_| {})),
        Check(
            "(audio) transport running: Time readout advanced, selected-track meter > 0",
            Box::new(|c, a| {
                if !c.engine.device_running() {
                    return Ok(()); // offline run: Space only shows a status message
                }
                let s = a.global::<Studio>();
                let t = s.get_time_text();
                let playing = c.engine.transport().playing;
                (playing && t.as_str() > "00:04.700" && s.get_sel_meter_l() < 0.999)
                    .then_some(())
                    .ok_or(format!("playing {playing} time {t} meter {}", s.get_sel_meter_l()))
            }),
        ),
        Do("Space (pause)", Box::new(|a| key(a, " ", false, false))),
        Check("(audio) paused", Box::new(|c, _| (!c.engine.transport().playing).then_some(()).ok_or("still playing".into()))),
        Do(
            "wheel: scroll timeline right",
            Box::new(|a| {
                let w = a.window();
                dispatch(w, WindowEvent::PointerMoved { position: lanes_point() });
                dispatch(w, WindowEvent::PointerScrolled { position: lanes_point(), delta_x: -51.0, delta_y: 0.0 });
            }),
        ),
        Check(
            "view scrolled by ~4 beats",
            Box::new(|c, _| ((c.view.start_beats - 4.0).abs() < 0.2).then_some(()).ok_or(format!("start {}", c.view.start_beats))),
        ),
        Do(
            "Cmd + wheel: zoom in around x=900",
            Box::new(|a| {
                let w = a.window();
                dispatch(w, WindowEvent::KeyPressed { text: Key::Control.into() });
                dispatch(w, WindowEvent::PointerScrolled { position: lanes_point(), delta_x: 0.0, delta_y: 50.0 });
                dispatch(w, WindowEvent::KeyReleased { text: Key::Control.into() });
            }),
        ),
        Check(
            "zoom changed px/beat, beat under the cursor kept",
            Box::new(|c, _| {
                let ppb = c.view.px_per_beat;
                let anchor = c.view.beat_at(446.35);
                let expected = 4.0 + 446.35_f64 / crate::timeline::DEFAULT_PX_PER_BEAT;
                ((ppb - crate::timeline::DEFAULT_PX_PER_BEAT).abs() > 1.0 && (anchor - expected).abs() < 0.05)
                    .then_some(())
                    .ok_or(format!("ppb {ppb} anchor {anchor} vs {expected}"))
            }),
        ),
        // ---------------- assistant panel (TEST fixture provider, no network) ----------------
        Do("click the assistant toggle", Box::new(|a| click(a, win().0 - 147.0 + 12.5, 48.0))),
        Check(
            "assistant panel open, timeline narrower, fixture provider available",
            Box::new(|c, a| {
                let w = a.get_lanes_width();
                let s = a.global::<Studio>();
                (c.assistant_open
                    && s.get_assistant_open()
                    && s.get_chat_available()
                    && (w - (win().0 - geo().timeline_x - 5.0 - 257.5 - 191.0)).abs() < 1.0)
                    .then_some(())
                    .ok_or(format!("open {} lanes width {w}", c.assistant_open))
            }),
        ),
        Check(
            "recorded project state before the assistant turn",
            Box::new(move |c, _| {
                rt1.set(c.project().tracks.len());
                rg1.set(c.project().tracks[0].gain);
                Ok(())
            }),
        ),
        Do(
            "click the input and type a prompt (Slint key events)",
            Box::new(|a| {
                click(a, card_x() + 60.0, input_y());
                type_text(a, "add drums, tempo 128, audio down 6 db");
            }),
        ),
        Check(
            "typed text reached the composer",
            Box::new(|c, a| {
                let d = a.global::<Studio>().get_chat_draft();
                (d == "add drums, tempo 128, audio down 6 db" && c.assistant.mode == Mode::Assist)
                    .then_some(())
                    .ok_or(format!("draft {d:?}"))
            }),
        ),
        Do("Enter (send in Assist mode)", Box::new(|a| key(a, Key::Return, false, false))),
        Check(
            "Assist turn applied: track added, 128 BPM, Audio at -6 dB, one undo step",
            Box::new(move |c, _| {
                let p = c.project();
                let ok = p.tracks.len() == rt2.get() + 1
                    && p.tracks.last().is_some_and(|t| t.name == "Drums")
                    && p.transport.bpm == 128.0
                    && approx(p.tracks[0].gain, db_to_gain(-6.0), 1e-4)
                    && c.doc.undo_label() == Some("Assistant Edit");
                ok.then_some(()).ok_or(format!(
                    "tracks {} bpm {} gain {} undo {:?}",
                    p.tracks.len(),
                    p.transport.bpm,
                    p.tracks[0].gain,
                    c.doc.undo_label()
                ))
            }),
        ),
        Check(
            "transcript shows the prompt and the plan summary lines",
            Box::new(|c, a| {
                let e = c.assistant.last().ok_or("empty transcript")?;
                let want = ["Added an audio track, Drums.", "Changed tempo to 128 BPM.", "Set Audio volume to -6.0 dB."];
                let s = a.global::<Studio>();
                let chat = s.get_chat();
                let rows = slint::Model::row_count(&chat);
                let ui_lines =
                    slint::Model::row_data(&chat, rows.saturating_sub(1)).map(|r| slint::Model::row_count(&r.lines)).unwrap_or(0);
                let user_ok =
                    slint::Model::row_data(&chat, rows.saturating_sub(2)).is_some_and(|r| r.kind == 0 && r.text.starts_with("add drums"));
                (e.kind == EntryKind::Reply && e.lines == want && rows == 2 && ui_lines == 3 && user_ok && s.get_chat_draft().is_empty())
                    .then_some(())
                    .ok_or(format!("{:?} rows {rows} ui lines {ui_lines}", e.lines))
            }),
        ),
        Do("Cmd+Z", Box::new(|a| key(a, "z", true, false))),
        Check(
            "one undo reverted the whole assistant turn",
            Box::new(move |c, _| {
                let p = c.project();
                (p.tracks.len() == rt3.get() && p.transport.bpm == 120.0 && approx(p.tracks[0].gain, rg2.get(), 1e-6))
                    .then_some(())
                    .ok_or(format!("tracks {} bpm {} gain {}", p.tracks.len(), p.transport.bpm, p.tracks[0].gain))
            }),
        ),
        Do("click Point", Box::new(|a| click(a, card_x() + 6.8 + 2.125 + 53.125 + 1.7 + 26.5, card_y() + 121.55 - 6.8 - 10.2))),
        Check(
            "Point mode active",
            Box::new(move |c, a| {
                rr1.set(c.doc.revision());
                (c.assistant.mode == Mode::Teach && a.global::<Studio>().get_teach_mode()).then_some(()).ok_or("still Assist".into())
            }),
        ),
        Do(
            "type + Enter (fixture turn with an edit)",
            Box::new(|a| {
                click(a, card_x() + 60.0, input_y());
                type_text(a, "slow it down");
                key(a, Key::Return, false, false);
            }),
        ),
        Check(
            "Point edit rejected: error entry, project unchanged",
            Box::new(move |c, _| {
                let e = c.assistant.last().ok_or("empty transcript")?;
                (e.kind == EntryKind::Error
                    && e.text.contains("Point mode")
                    && c.doc.revision() == rr2.get()
                    && c.project().transport.bpm == 120.0)
                    .then_some(())
                    .ok_or(format!("{:?} {:?} rev {} vs {}", e.kind, e.text, c.doc.revision(), rr2.get()))
            }),
        ),
        Do(
            "type + Enter (fixture turn highlighting transport.bpm)",
            Box::new(|a| {
                click(a, card_x() + 60.0, input_y());
                type_text(a, "where is the tempo");
                key(a, Key::Return, false, false);
            }),
        ),
        Check(
            "highlight active on transport.bpm, overlay rect mapped",
            Box::new(|c, a| {
                let hl = c.assistant.highlights.iter().any(|h| h.target == "transport.bpm");
                let rects = a.global::<Studio>().get_highlights();
                let r = slint::Model::row_data(&rects, 0);
                // Point ring around the tempo box (700, 42, 57 x 20): padded 6 px (CSS), radius 14
                let mapped = r.as_ref().is_some_and(|r| {
                    (r.x - (transport_x() + 694.9)).abs() < 0.01
                        && (r.y - 36.9).abs() < 0.01
                        && (r.r - 11.9).abs() < 0.01
                        && r.title == "Tempo (BPM)"
                });
                let note = c.assistant.last().is_some_and(|e| e.lines.iter().any(|l| l.starts_with("Tempo (BPM)")));
                (hl && mapped && note && slint::Model::row_count(&rects) == 1)
                    .then_some(())
                    .ok_or(format!("hl {hl} rect {r:?} note {note}"))
            }),
        ),
        // ---------------- mixer ----------------
        Do("click the mixer toggle", Box::new(|a| click(a, win().0 - 119.0 + 12.5, 47.0))),
        Check(
            "mixer open: bottom panel 390 pt, timeline lanes shorter",
            Box::new(move |c, a| {
                let h = a.get_lanes_height();
                rt4.set(c.project().tracks.len());
                (c.mixer_open
                    && a.global::<Studio>().get_mixer_open()
                    && (h - (win().1 - 88.4 - crate::highlight::MIXER_BOTTOM_H - 27.5)).abs() < 1.0)
                    .then_some(())
                    .ok_or(format!("open {} lanes h {h}", c.mixer_open))
            }),
        ),
        Do("click the Instrument strip header", Box::new(|a| click(a, bottom_x() + 108.0 * 0.85 + 40.0, mixer_y() + 35.0))),
        Check(
            "strip click selected track 2",
            Box::new(move |c, _| {
                rv1.set(c.doc.history_labels().iter().filter(|l| l.as_str() == "Track Volume Change").count());
                rm1.set(c.project().tracks[0].mute);
                (c.selected_track == Some(c.project().tracks[1].id)).then_some(()).ok_or("not selected".into())
            }),
        ),
        Do(
            "drag the Audio strip fader from -24 dB to -36 dB",
            Box::new(|a| {
                let y = |f: f32| mixer_y() + MIXER_TRAVEL_TOP + f * MIXER_TRAVEL;
                drag(
                    a,
                    bottom_x() + 53.0 * 0.85,
                    y(crate::controller::fader_from_db(-24.0)),
                    bottom_x() + 53.0 * 0.85,
                    y(crate::controller::fader_from_db(-36.0)),
                );
            }),
        ),
        Check(
            "track 1 gain ≈ -36 dB, exactly one new undo step",
            Box::new(move |c, _| {
                let g = c.project().tracks[0].gain;
                let db = dstudio_core::gain_to_db(g);
                let n = c.doc.history_labels().iter().filter(|l| l.as_str() == "Track Volume Change").count();
                ((db + 36.0).abs() < 0.3 && n == rv2.get() + 1)
                    .then_some(())
                    .ok_or(format!("{db} dB, volume steps {n} (was {})", rv2.get()))
            }),
        ),
        Do("click M on the Audio strip", Box::new(|a| click(a, bottom_x() + 25.0 * 0.85, bottom_end() - 17.0 * 0.85))),
        Check(
            "track 1 mute toggled via the mixer (SetTrackMute)",
            Box::new(move |c, _| {
                (c.project().tracks[0].mute != rm2.get() && c.doc.undo_label() == Some("Mute Change")).then_some(()).ok_or(format!(
                    "mute {} undo {:?}",
                    c.project().tracks[0].mute,
                    c.doc.undo_label()
                ))
            }),
        ),
        Do(
            "drag the master strip fader to ~-24 dB",
            Box::new(|a| {
                let y = |f: f32| mixer_y() + MIXER_TRAVEL_TOP + f * MIXER_TRAVEL;
                let x = win().0 - 5.0 - 108.0 * 0.85 + 53.0 * 0.85;
                drag(a, x, y(0.3), x, y(crate::controller::fader_from_db(-24.0)));
            }),
        ),
        Check(
            "master gain ≈ -24 dB (SetMasterGain), mixer master fader follows",
            Box::new(|c, a| {
                let db = dstudio_core::gain_to_db(c.project().master_gain);
                let f = a.global::<Studio>().get_master_fader();
                ((db + 24.0).abs() < 0.3 && (f - 0.78).abs() < 0.01).then_some(()).ok_or(format!("{db} dB, fader {f}"))
            }),
        ),
        Do("click M on the master strip", Box::new(|a| click(a, win().0 - 5.0 - 108.0 * 0.85 + 25.0 * 0.85, bottom_end() - 17.0 * 0.85))),
        Check(
            "master muted (SetMasterMute), fader untouched, header row says so",
            Box::new(|c, a| {
                let s = a.global::<Studio>();
                let db = dstudio_core::gain_to_db(c.project().master_gain);
                (c.project().master_mute
                    && s.get_master_mute()
                    && c.doc.undo_label() == Some("Master Mute Change")
                    && (db + 24.0).abs() < 0.3)
                    .then_some(())
                    .ok_or(format!("mute {} ui {} undo {:?} {db} dB", c.project().master_mute, s.get_master_mute(), c.doc.undo_label()))
            }),
        ),
        Do("Cmd+Z", Box::new(|a| key(a, "z", true, false))),
        Check(
            "undo unmuted the master",
            Box::new(|c, a| {
                (!c.project().master_mute && !a.global::<Studio>().get_master_mute()).then_some(()).ok_or("still muted".into())
            }),
        ),
        Do("click the mixer's collapse button", Box::new(|a| click(a, win().0 - 5.0 - 29.0 + 10.5, mixer_y() + 3.0 + 10.5))),
        Check(
            "mixer shut to its tab row; the button now expands",
            Box::new(|c, a| {
                let s = a.global::<Studio>();
                (c.mixer_open
                    && c.bottom_collapsed()
                    && s.get_bottom_collapsed()
                    && (c.bottom_h() - crate::splitters::BOTTOM_COLLAPSED_H).abs() < 0.01)
                    .then_some(())
                    .ok_or(format!("h {} ui {}", c.bottom_h(), s.get_bottom_collapsed()))
            }),
        ),
        Do("click the expand button", Box::new(|a| click(a, win().0 - 5.0 - 29.0 + 10.5, mixer_y() + 3.0 + 10.5))),
        Check(
            "mixer back at its 390 pt height",
            Box::new(|c, a| {
                ((c.bottom_h() - crate::highlight::MIXER_BOTTOM_H).abs() < 0.5 && !a.global::<Studio>().get_bottom_collapsed())
                    .then_some(())
                    .ok_or(format!("h {}", c.bottom_h()))
            }),
        ),
        Do("click '+ Add track' in the mixer", Box::new(|a| click(a, bottom_x() + 3.0 * 108.0 * 0.85 + 46.0, mixer_y() + 210.0))),
        DoC(
            "pick 'Audio track' in the Add track menu",
            Box::new(|a, ctl| {
                let (x, y) = route_row(ctl, 0);
                click(a, x, y)
            }),
        ),
        Check(
            "mixer Add track added an audio track",
            Box::new(move |c, _| {
                let p = c.project();
                (p.tracks.len() == rt5.get() + 1 && p.tracks.last().unwrap().kind == TrackKind::Audio)
                    .then_some(())
                    .ok_or(format!("{} tracks", p.tracks.len()))
            }),
        ),
        Do("Cmd+Shift+X (view.toggle_mixer)", Box::new(|a| key(a, "X", true, true))),
        Check(
            "mixer closed by the shortcut, bottom panel back",
            Box::new(|c, a| {
                let h = a.get_lanes_height();
                (!c.mixer_open && (h - (win().1 - 88.4 - c.bottom_h() - 27.5)).abs() < 1.0)
                    .then_some(())
                    .ok_or(format!("open {} lanes h {h}", c.mixer_open))
            }),
        ),
    ]
}

/// Window point inside row `i` of the open routing menu (Add track, add send, Out; src/routing.rs).
fn route_row(ctl: &Ctl, i: usize) -> (f32, f32) {
    let c = ctl.borrow();
    let (x, y) = c.routing.menu.as_ref().map_or((0.0, 0.0), |m| (m.x, m.y));
    (x + 30.0, y + 4.0 + i as f32 * 20.0 + 10.0)
}

/// Channel strip trim, width and mono (spec 005): drag, reset, the switch and undo.
fn strip_steps() -> Vec<Step> {
    use Step::*;
    let n0 = Rc::new(std::cell::Cell::new(0usize));
    let n1 = n0.clone();
    let count = |c: &crate::controller::Controller, label: &str| c.doc.history_labels().iter().filter(|l| l.as_str() == label).count();
    // Knob centres in the channel strip's Input tab (ui/app.slint): Trim (47, 141), Width (123, 141),
    // Mono switch (122.5, 210.5).
    let at = |x: f32, y: f32| (bottom_x() + x, bottom_y() + y);
    vec![
        DoC(
            "strip: close the mixer, select track 1, Devices tab, the strip's Input tab",
            Box::new(move |a, ctl| {
                let mut c = ctl.borrow_mut();
                c.mixer_open = false;
                let t = c.project().tracks[0].id;
                c.select_track(t);
                c.set_bottom_tab(BottomTab::Devices);
                a.global::<Studio>().set_strip_tab(0);
                n0.set(count(&c, "Trim Change"));
                // Coalescing window 0: only the held gesture merges the moves into one step.
                c.doc.coalesce_window = std::time::Duration::ZERO;
                c.ui_dirty = true;
            }),
        ),
        Do(
            "strip: drag the Trim knob up 30 pt",
            Box::new(move |a| {
                let (x, y) = at(47.0, 141.0);
                drag(a, x, y, x, y - 30.0)
            }),
        ),
        Check(
            "strip: trim raised (+9.6 dB), readout '+9.6 dB', one 'Trim Change' step",
            Box::new(move |c, a| {
                let t = c.project().tracks[0].trim_db;
                let txt = a.global::<Studio>().get_sel_trim_text();
                let n = count(c, "Trim Change");
                (approx(t, 9.6, 0.25) && txt.starts_with('+') && txt.ends_with(" dB") && n == n1.get() + 1)
                    .then_some(())
                    .ok_or(format!("trim {t} text {txt} steps {n} (was {})", n1.get()))
            }),
        ),
        Do(
            "strip: double-click the Trim knob",
            Box::new(move |a| {
                let (x, y) = at(47.0, 141.0);
                double_click(a, x, y)
            }),
        ),
        Check(
            "strip: trim reset to 0.0 dB",
            Box::new(|c, a| {
                let t = c.project().tracks[0].trim_db;
                let txt = a.global::<Studio>().get_sel_trim_text();
                (t == 0.0 && txt == "0.0 dB").then_some(()).ok_or(format!("trim {t} text {txt}"))
            }),
        ),
        Do(
            "strip: drag the Width knob down 30 pt",
            Box::new(move |a| {
                let (x, y) = at(123.0, 141.0);
                drag(a, x, y, x, y + 30.0)
            }),
        ),
        Check(
            "strip: width 60 %, readout '60%'",
            Box::new(|c, a| {
                let w = c.project().tracks[0].width;
                let txt = a.global::<Studio>().get_sel_width_text();
                (approx(w, 0.6, 0.02) && txt == "60%").then_some(()).ok_or(format!("width {w} text {txt}"))
            }),
        ),
        Do(
            "strip: click the Mono switch",
            Box::new(move |a| {
                let (x, y) = at(122.5, 210.5);
                click(a, x, y)
            }),
        ),
        Check(
            "strip: mono on ('Mono Change'), switch on",
            Box::new(|c, a| {
                let on = c.project().tracks[0].mono;
                (on && a.global::<Studio>().get_sel_mono() && c.doc.undo_label() == Some("Mono Change"))
                    .then_some(())
                    .ok_or(format!("mono {on} undo {:?}", c.doc.undo_label()))
            }),
        ),
        Do("strip: Cmd+Z", Box::new(|a| key(a, "z", true, false))),
        Check(
            "strip: undo turned mono off and kept the width",
            Box::new(|c, _| {
                let t = &c.project().tracks[0];
                (!t.mono && approx(t.width, 0.6, 0.02)).then_some(()).ok_or(format!("mono {} width {}", t.mono, t.width))
            }),
        ),
        DoC(
            "strip: back to 100 % width, coalescing window restored",
            Box::new(|_, ctl| {
                let mut c = ctl.borrow_mut();
                c.doc.coalesce_window = std::time::Duration::from_millis(750);
                let t = c.project().tracks[0].id;
                c.set_track_width(t, 1.0);
                c.strip_gesture_end();
            }),
        ),
        Check(
            "strip: track 1 neutral again",
            Box::new(|c, _| c.project().tracks[0].strip_is_neutral().then_some(()).ok_or("not neutral".into())),
        ),
    ]
}

/// Settings › Plugins custom folders (issue #33): add one holding a copy of the test gain CLAP
/// and one that does not exist, remove them again.
fn plugin_folder_steps() -> Vec<Step> {
    use Step::*;
    let root = std::env::temp_dir().join(format!("dstudio-selftest-plugins-{}", std::process::id()));
    let (kept, gone) = (root.join("Custom Plugins"), root.join("Moved Away"));
    let (k1, k2, k3, g1, g2, r1) = (kept.clone(), kept.clone(), kept.clone(), gone.clone(), gone.clone(), root.clone());
    let custom_rows = |a: &AppWindow| -> Vec<(usize, crate::PluginRow)> {
        let m = a.global::<Studio>().get_scan_dirs();
        (0..m.row_count()).filter_map(|i| m.row_data(i).map(|r| (i, r))).filter(|(_, r)| r.badge == crate::devices::CUSTOM_BADGE).collect()
    };
    let in_kept = move |c: &Controller, dir: &std::path::Path| {
        c.devices.catalog.plugins.iter().any(|p| {
            p.plugin.id == "studio.dstudio.test.gain" && p.plugin.path.as_deref().is_some_and(|q| std::path::Path::new(q).starts_with(dir))
        })
    };
    vec![
        DoC(
            "plugin folders: Settings › Plugins, add a folder holding the test gain CLAP and one that does not exist",
            Box::new(move |_, ctl| {
                let _ = std::fs::create_dir_all(&k1);
                let src = dstudio_engine::test_plugin_dir().join("DStudioTestGain.clap");
                if src.is_file() {
                    let _ = std::fs::copy(&src, k1.join("DStudioTestGain.clap"));
                }
                let mut c = ctl.borrow_mut();
                c.settings.prefs.plugin_folders.clear();
                c.open_settings();
                c.settings_page(crate::settings::PLUGINS_PAGE);
                c.plugins_add_folder(&k1);
                c.plugins_add_folder(&g1);
                c.plugins_add_folder(&k1); // twice: refused
            }),
        ),
        Check(
            "plugin folders: both listed last with the Custom badge, the missing one marked Missing, the scan found the copied plugin there [needs engine plugin scanning]",
            Box::new(move |c, a| {
                let rows = custom_rows(a);
                let warn: Vec<bool> = rows.iter().map(|(_, r)| r.warn).collect();
                let total = a.global::<Studio>().get_scan_dirs().row_count();
                let last = rows.last().map_or(0, |(i, _)| *i + 1);
                if warn != [false, true] || last != total || c.settings.prefs.plugin_folders.len() != 2 {
                    return Err(format!(
                        "custom rows {:?} of {total}, prefs {:?}",
                        rows.iter().map(|(_, r)| r.name.to_string()).collect::<Vec<_>>(),
                        c.settings.prefs.plugin_folders
                    ));
                }
                if c.devices.catalog.fixture.is_some() || !dstudio_engine::test_plugin_dir().join("DStudioTestGain.clap").is_file() {
                    return Ok(());
                }
                in_kept(c, &k2).then_some(()).ok_or(format!("no test gain plugin from {} in the catalogue", k2.display()))
            }),
        ),
        DoC(
            "plugin folders: click the missing folder's remove button",
            Box::new(move |a, ctl| {
                let Some((i, r)) = custom_rows(a).into_iter().last() else { return };
                if i < 8 {
                    let row_w = set_w() - 187.0 - 41.0;
                    click(a, set_x() + 187.0 + 20.5 + row_w - 22.0 + 8.0, set_y() + 91.0 + i as f32 * 30.5 + 12.5);
                } else {
                    ctl.borrow_mut().plugins_remove_folder(r.detail.as_str()); // scrolled out of the 8-row list
                }
            }),
        ),
        Check(
            "plugin folders: the missing folder is gone, the other stays",
            Box::new(move |c, a| {
                let want = vec![k3.to_string_lossy().into_owned()];
                (c.settings.prefs.plugin_folders == want && custom_rows(a).len() == 1).then_some(()).ok_or(format!(
                    "prefs {:?}, gone {}",
                    c.settings.prefs.plugin_folders,
                    g2.display()
                ))
            }),
        ),
        DoC(
            "plugin folders: remove the other one, close Settings",
            Box::new(move |_, ctl| {
                let mut c = ctl.borrow_mut();
                let all = c.settings.prefs.plugin_folders.clone();
                for f in all {
                    c.plugins_remove_folder(&f);
                }
                c.close_settings();
            }),
        ),
        Check(
            "plugin folders: none left, and the copied plugin left the catalogue with its folder",
            Box::new(move |c, _| {
                let _ = std::fs::remove_dir_all(&r1);
                let dir = r1.join("Custom Plugins");
                (c.settings.prefs.plugin_folders.is_empty() && !in_kept(c, &dir))
                    .then_some(())
                    .ok_or(format!("prefs {:?}", c.settings.prefs.plugin_folders))
            }),
        ),
    ]
}

/// Return tracks, sends and Out through the interface (spec 002, A8): Return A from the Sends
/// tab's placeholder, a send level drag (one undo step), the Pre chip, Return B from "+" with a
/// silent knob that a drag turns on, the Out menu (Master only), the Returns pill, the mixer's
/// Sends section and its send options, and removing a return with its routing.
fn returns_steps() -> Vec<Step> {
    use Step::*;
    let u = 0.85;
    let n0 = Rc::new(std::cell::Cell::new(0usize));
    let lv = Rc::new(std::cell::Cell::new(0usize));
    let (n1, n2, n3, lv1) = (n0.clone(), n0.clone(), n0.clone(), lv.clone());
    // Sends tab: knobs two to a row (cells 74 wide at x 10 and 86, 78 tall from 6 below the tab
    // rule at 128 pt); the knob's centre is 33 below the cell top, the Post / Pre chip's 6.
    let sends_y = move || bottom_y() + 128.0 * u;
    let knob =
        move |k: usize| (bottom_x() + if k.is_multiple_of(2) { 47.0 } else { 123.0 }, sends_y() + 6.0 + (k / 2) as f32 * 78.0 + 33.0);
    let chip = move |k: usize| (bottom_x() + if k.is_multiple_of(2) { 47.0 } else { 123.0 }, sends_y() + 6.0 + (k / 2) as f32 * 78.0 + 6.0);
    // Mixer: the letter tag of send row `r` on strip 0, in the expanded Sends section of a
    // project with `n` returns (rows stack upward from the Sends row 302 pt above the strip foot).
    let tag = move |r: usize, n: usize| {
        (bottom_x() + 8.0 * u + 9.0 * u, bottom_end() - 302.0 * u - (n as f32 * 20.0 + 4.0) * u + r as f32 * 20.0 * u + 10.0 * u)
    };
    let one_send = |c: &Controller, want_pre: bool| -> Result<(), String> {
        let p = c.project();
        let a = p.returns().next().map(|t| t.id).ok_or("no Return A")?;
        let s = &p.tracks[0].sends;
        (s.len() == 1 && s[0].target == a && s[0].pre_fader == want_pre).then_some(()).ok_or(format!("sends {s:?}"))
    };
    vec![
        DoC(
            "returns: close the mixer, select track 1, Devices tab, show the Sends tab",
            Box::new(move |a, ctl| {
                {
                    let mut c = ctl.borrow_mut();
                    c.mixer_open = false;
                    c.routing.sends_open = false;
                    c.settings.prefs.hide_returns = false;
                    let t = c.project().tracks[0].id;
                    c.select_track(t);
                    c.set_bottom_tab(BottomTab::Devices);
                    n0.set(c.project().tracks.len());
                    c.ui_dirty = true;
                }
                click(a, bottom_x() + 108.0 * u + 12.0, bottom_y() + 116.0 * u);
            }),
        ),
        Check(
            "returns: Sends tab open, no returns yet (only the Return A placeholder)",
            Box::new(|c, a| {
                let s = a.global::<Studio>();
                (s.get_strip_tab() == 1 && c.project().returns().count() == 0 && s.get_sel_can_send() && s.get_sel_sends().row_count() == 0)
                    .then_some(())
                    .ok_or(format!("tab {} returns {}", s.get_strip_tab(), c.project().returns().count()))
            }),
        ),
        Do("returns: click the Return A placeholder", Box::new(move |a| click(a, knob(0).0, knob(0).1 + 10.0))),
        Check(
            "returns: Return A added after the tracks, track 1 sends to it at 0 dB post-fader (one 'Add Send' step)",
            Box::new(move |c, _| {
                let p = c.project();
                let r = p.tracks.last().ok_or("no tracks")?;
                let s = &p.tracks[0].sends;
                let ok = p.tracks.len() == n1.get() + 1
                    && r.is_return()
                    && r.name == "Return A"
                    && s.len() == 1
                    && s[0].target == r.id
                    && s[0].level == 1.0
                    && !s[0].pre_fader
                    && c.doc.undo_label() == Some("Add Send");
                ok.then_some(()).ok_or(format!(
                    "{} tracks, last {:?} {:?}, sends {s:?}, undo {:?}",
                    p.tracks.len(),
                    r.kind,
                    r.name,
                    c.doc.undo_label()
                ))
            }),
        ),
        DoC(
            "returns: drag the Return A knob down 40 pt (coalescing window 0: only the held gesture merges the moves)",
            Box::new(move |a, ctl| {
                {
                    let mut c = ctl.borrow_mut();
                    lv.set(c.doc.history_labels().iter().filter(|l| l.as_str() == "Send Level Change").count());
                    c.doc.coalesce_window = std::time::Duration::ZERO;
                }
                let (x, y) = knob(0);
                drag(a, x, y, x, y + 40.0);
                ctl.borrow_mut().doc.coalesce_window = std::time::Duration::from_millis(750);
            }),
        ),
        Check(
            "returns: send level below 0 dB, one 'Send Level Change' step for the drag",
            Box::new(move |c, _| {
                let lvl = c.project().tracks[0].sends.first().map_or(1.0, |s| s.level);
                let n = c.doc.history_labels().iter().filter(|l| l.as_str() == "Send Level Change").count();
                (lvl < 0.9 && lvl > 0.0 && n == lv1.get() + 1).then_some(()).ok_or(format!("level {lvl}, steps {n} (was {})", lv1.get()))
            }),
        ),
        Do("returns: click the Post chip above the knob", Box::new(move |a| click(a, chip(0).0, chip(0).1))),
        Check(
            "returns: the send is pre-fader ('Send Pre-Fader')",
            Box::new(move |c, _| {
                one_send(c, true)?;
                (c.doc.undo_label() == Some("Send Pre-Fader")).then_some(()).ok_or(format!("undo {:?}", c.doc.undo_label()))
            }),
        ),
        Do("returns: click '+' beside the Sends tab", Box::new(move |a| click(a, bottom_x() + 154.0 * u, bottom_y() + 116.0 * u))),
        Check(
            "returns: Return B added ('Add Return Track'), track 1 still selected with a silent knob for it",
            Box::new(move |c, a| {
                let p = c.project();
                let names: Vec<&str> = p.returns().map(|t| t.name.as_str()).collect();
                let rows: Vec<_> = a.global::<Studio>().get_sel_sends().iter().collect();
                let ok = names == ["Return A", "Return B"]
                    && c.selected_track == Some(p.tracks[0].id)
                    && rows.len() == 2
                    && rows[0].stored
                    && !rows[1].stored
                    && !rows[1].live
                    && p.tracks[0].sends.len() == 1
                    && c.doc.undo_label() == Some("Add Return Track");
                ok.then_some(()).ok_or(format!(
                    "returns {names:?}, selected {:?}, rows {}, sends {:?}, undo {:?}",
                    c.selected_track,
                    rows.len(),
                    p.tracks[0].sends,
                    c.doc.undo_label()
                ))
            }),
        ),
        Do(
            "returns: drag the silent Return B knob up 60 pt",
            Box::new(move |a| {
                let (x, y) = knob(1);
                drag(a, x, y, x, y - 60.0);
            }),
        ),
        Check(
            "returns: the drag stored a post-fader send to Return B above −∞",
            Box::new(|c, _| {
                let b = c.project().returns().nth(1).map(|t| t.id);
                let s = c.project().tracks[0].sends.iter().find(|s| Some(s.target) == b).copied();
                s.filter(|s| s.level > 0.0 && !s.pre_fader).map(|_| ()).ok_or(format!("{:?}", c.project().tracks[0].sends))
            }),
        ),
        Do("returns: Cmd+Z", Box::new(|a| key(a, "z", true, false))),
        Check("returns: the Return B slot is silent again", Box::new(move |c, _| one_send(c, true))),
        Do("returns: open the Out menu", Box::new(|a| click(a, bottom_x() + 80.0, bottom_y() + 63.0 + 9.0))),
        Check(
            "returns: Out lists only Master (send to a return instead)",
            Box::new(|c, _| {
                let l = c.route_menu_labels();
                (l == ["Master"]).then_some(()).ok_or(format!("{l:?}"))
            }),
        ),
        Do("returns: Escape closes the menu", Box::new(|a| key(a, Key::Escape, false, false))),
        Do("returns: click the Returns pill", Box::new(|a| click(a, tracks_x() + 90.0, geo().timeline_y + 14.0))),
        Check(
            "returns: return rows hidden, the pill unlit, the returns still play",
            Box::new(move |c, a| {
                let s = a.global::<Studio>();
                let rows = s.get_tracks().row_count();
                (!c.returns_shown()
                    && !s.get_returns_shown()
                    && rows == n2.get()
                    && s.get_pinned_h() == 0.0
                    && c.project().returns().count() == 2
                    && c.routing.menu.is_none())
                .then_some(())
                .ok_or(format!("shown {} rows {rows} pinned {} menu {}", c.returns_shown(), s.get_pinned_h(), c.routing.menu.is_some()))
            }),
        ),
        Do("returns: click the Returns pill again", Box::new(|a| click(a, tracks_x() + 90.0, geo().timeline_y + 14.0))),
        Check(
            "returns: the return rows are pinned above the master row",
            Box::new(|c, a| {
                let s = a.global::<Studio>();
                let pinned = s.get_tracks().iter().filter(|t| t.pinned && t.is_return).count();
                (c.returns_shown() && s.get_pinned_h() > 0.0 && pinned == 2).then_some(()).ok_or(format!(
                    "shown {} pinned h {} rows {pinned}",
                    c.returns_shown(),
                    s.get_pinned_h()
                ))
            }),
        ),
        Do("returns: open the mixer (Cmd+Shift+X)", Box::new(|a| key(a, "X", true, true))),
        Check(
            "returns: mixer open, Sends collapsed",
            Box::new(|c, a| {
                (c.mixer_open && a.global::<Studio>().get_mixer_open() && !c.routing.sends_open)
                    .then_some(())
                    .ok_or(format!("mixer {} sends {}", c.mixer_open, c.routing.sends_open))
            }),
        ),
        Do(
            "returns: click 'Sends' on the first strip",
            Box::new(|a| click(a, bottom_x() + 8.0 * 0.85 + 14.0, bottom_end() - 302.0 * 0.85 + 11.0 * 0.85)),
        ),
        Check(
            "returns: every track strip shows one send row per return, the return strips none",
            Box::new(|c, a| {
                let s = a.global::<Studio>();
                let strips: Vec<_> = s.get_strips().iter().collect();
                let ret = strips.iter().filter(|st| st.is_return).count();
                let ordinary = strips.len() - ret;
                let rows = s.get_strip_sends().row_count();
                (c.mixer_open
                    && c.routing.sends_open
                    && s.get_sends_open()
                    && rows == ordinary * 2
                    && ret == 2
                    && s.get_strip_sends_max() == 2)
                    .then_some(())
                    .ok_or(format!("mixer {} open {} rows {rows} returns {ret}", c.mixer_open, c.routing.sends_open))
            }),
        ),
        Do("returns: click the A tag on the first strip", Box::new(move |a| click(a, tag(0, 2).0, tag(0, 2).1))),
        Check(
            "returns: the send options list Post-fader, Pre-fader (checked), Reset to 0 dB and Remove send",
            Box::new(|c, _| {
                let l = c.route_menu_labels();
                let checked: Vec<bool> = c.routing.menu.as_ref().map(|m| m.items.iter().map(|i| i.2).collect()).unwrap_or_default();
                (l == ["Post-fader", "Pre-fader", "Reset to 0 dB", "Remove send"] && checked == [false, true, false, false])
                    .then_some(())
                    .ok_or(format!("{l:?} {checked:?}"))
            }),
        ),
        DoC(
            "returns: pick 'Remove send'",
            Box::new(|a, ctl| {
                let (x, y) = route_row(ctl, 3);
                click(a, x, y)
            }),
        ),
        Check(
            "returns: send removed ('Remove Send'), its slot silent",
            Box::new(|c, a| {
                let row0 = a.global::<Studio>().get_strip_sends().row_data(0);
                (c.project().tracks[0].sends.is_empty()
                    && c.doc.undo_label() == Some("Remove Send")
                    && row0.is_some_and(|r| !r.stored && !r.live))
                .then_some(())
                .ok_or(format!("sends {:?} undo {:?}", c.project().tracks[0].sends, c.doc.undo_label()))
            }),
        ),
        Do("returns: Cmd+Z", Box::new(|a| key(a, "z", true, false))),
        Check("returns: send restored, still pre-fader", Box::new(move |c, _| one_send(c, true))),
        Do("returns: close the mixer", Box::new(|a| key(a, "X", true, true))),
        DoC(
            "returns: route track 1 to Return A (as an older project could), then delete Return A",
            Box::new(|_, ctl| {
                let mut c = ctl.borrow_mut();
                let (t, r) = (c.project().tracks[0].id, c.project().returns().next().map(|t| t.id));
                if let Some(r) = r {
                    c.set_track_output(t, Some(r));
                    c.execute(dstudio_core::Command::RemoveTrack { track: r });
                }
            }),
        ),
        Check(
            "returns: deleting the return removed the send and reset Out, in one step",
            Box::new(move |c, _| {
                let t = &c.project().tracks[0];
                (c.project().returns().count() == 1 && t.sends.is_empty() && t.output.is_none() && c.project().tracks.len() == n3.get() + 1)
                    .then_some(())
                    .ok_or(format!("returns {} sends {:?} out {:?}", c.project().returns().count(), t.sends, t.output))
            }),
        ),
        Do("returns: Cmd+Z", Box::new(|a| key(a, "z", true, false))),
        Check(
            "returns: undo brought back the return, the send and the routing; the Out menu lists the return it plays into",
            Box::new(|c, _| {
                let t = &c.project().tracks[0];
                let r = c.project().returns().next().map(|t| t.id);
                (r.is_some() && t.sends.len() == 1 && t.sends[0].target == r.unwrap() && t.output == r)
                    .then_some(())
                    .ok_or(format!("return {r:?} sends {:?} out {:?}", t.sends, t.output))
            }),
        ),
        Do("returns: open the Out menu", Box::new(|a| click(a, bottom_x() + 80.0, bottom_y() + 63.0 + 9.0))),
        Check(
            "returns: Out lists Master and Return A (checked)",
            Box::new(|c, _| {
                let l = c.route_menu_labels();
                let checked: Vec<bool> = c.routing.menu.as_ref().map(|m| m.items.iter().map(|i| i.2).collect()).unwrap_or_default();
                (l == ["Master", "Return A"] && checked == [false, true]).then_some(()).ok_or(format!("{l:?} {checked:?}"))
            }),
        ),
        DoC(
            "returns: pick Master",
            Box::new(|a, ctl| {
                let (x, y) = route_row(ctl, 0);
                click(a, x, y)
            }),
        ),
        Check(
            "returns: track 1 back on Master ('Output Change')",
            Box::new(|c, a| {
                let txt = a.global::<Studio>().get_sel_out_text();
                (c.project().tracks[0].output.is_none() && txt == "Master" && c.doc.undo_label() == Some("Output Change"))
                    .then_some(())
                    .ok_or(format!("out {:?} text {txt}", c.project().tracks[0].output))
            }),
        ),
        DoC(
            "returns: four more returns from '+' (six in all)",
            Box::new(|_, ctl| {
                let mut c = ctl.borrow_mut();
                let t = c.project().tracks[0].id;
                for _ in 0..4 {
                    c.sends_add_return(t);
                }
                c.ui_dirty = true;
            }),
        ),
        Check(
            "returns: six knobs, more than the Sends tab shows at once",
            Box::new(|c, a| {
                let n = a.global::<Studio>().get_sel_sends().row_count();
                (c.project().returns().count() == 6 && n == 6)
                    .then_some(())
                    .ok_or(format!("returns {} rows {n}", c.project().returns().count()))
            }),
        ),
        Do(
            "returns: scroll the Sends tab down",
            Box::new(move |a| {
                dispatch(
                    a.window(),
                    WindowEvent::PointerScrolled {
                        position: LogicalPosition::new(bottom_x() + 40.0, sends_y() + 60.0),
                        delta_x: 0.0,
                        delta_y: -400.0,
                    },
                );
            }),
        ),
        // Scrolled to the end: the last row of knobs sits at the bottom of the list.
        Do(
            "returns: double-click the last knob",
            Box::new(move |a| double_click(a, bottom_x() + 123.0, bottom_y() + 324.0 * u - 4.0 - 78.0 + 33.0)),
        ),
        Check(
            "returns: track 1 sends to the newest return at 0 dB",
            Box::new(|c, _| {
                let last = c.project().returns().last().map(|t| t.id);
                let s = c.project().tracks[0].sends.iter().find(|x| Some(x.target) == last).copied();
                s.filter(|s| (s.level - 1.0).abs() < 1e-6)
                    .map(|_| ())
                    .ok_or(format!("sends {:?} last {last:?}", c.project().tracks[0].sends))
            }),
        ),
        DoC(
            "returns: rename Return A from its name field",
            Box::new(|a, ctl| {
                let r = ctl.borrow().project().returns().next().map(|t| t.id.0 as i32).unwrap_or(-1);
                let s = a.global::<Studio>();
                s.invoke_track_rename(r, "Plate".into(), s.get_doc_epoch());
            }),
        ),
        Check(
            "returns: the return is renamed ('Rename Track'), the Out menu label follows",
            Box::new(|c, _| {
                let name = c.project().returns().next().map(|t| t.name.clone()).unwrap_or_default();
                (name == "Plate" && c.doc.undo_label() == Some("Rename Track"))
                    .then_some(())
                    .ok_or(format!("name {name} undo {:?}", c.doc.undo_label()))
            }),
        ),
        DoC(
            "returns: show automation with every return pinned",
            Box::new(|_, ctl| {
                let mut c = ctl.borrow_mut();
                if !c.auto.shown {
                    c.auto_toggle_shown();
                }
            }),
        ),
        Check(
            "returns: pinned automation rows stay above the master row",
            Box::new(|c, _| {
                let master_top = c.lanes_size.1 - c.view.master_h;
                let rows = c.auto_rows();
                let bad: Vec<_> = rows.iter().filter(|r| r.pinned && r.y0 + r.h > master_top).map(|r| (r.y0, r.h)).collect();
                let master = rows.iter().filter(|r| r.master).count();
                (bad.is_empty() && master == 1 && c.view.pinned_h > 0.0)
                    .then_some(())
                    .ok_or(format!("overlapping {bad:?} master top {master_top} pinned h {}", c.view.pinned_h))
            }),
        ),
        DoC(
            "returns: clean up (hide automation, delete the returns, back to the Input tab)",
            Box::new(|a, ctl| {
                let mut c = ctl.borrow_mut();
                if c.auto.shown {
                    c.auto_toggle_shown();
                }
                let rs: Vec<_> = c.project().returns().map(|t| t.id).collect();
                for r in rs {
                    c.execute(dstudio_core::Command::RemoveTrack { track: r });
                }
                a.global::<Studio>().set_strip_tab(0);
                c.ui_dirty = true;
            }),
        ),
    ]
}

/// Window centre of a track header's R button (track index `i`, current scroll).
fn header_r(ctl: &Ctl, i: usize) -> (f32, f32) {
    let sy = ctl.borrow().view.scroll_y as f32;
    (tracks_x() + 60.5, track0_y() + i as f32 * PITCH + 47.0 - sy)
}

/// Inspector (bottom panel 306 pt at y 509 with the Devices tab): R, I and the In field.
fn insp_r() -> (f32, f32) {
    (bottom_x() + 86.0 + 10.0, bottom_y() + 275.0 + 9.0)
}
fn insp_i() -> (f32, f32) {
    (bottom_x() + 108.0 + 10.0, bottom_y() + 275.0 + 9.0)
}
fn insp_in() -> (f32, f32) {
    (bottom_x() + 80.0, bottom_y() + 37.0 + 9.5)
}
/// "In" popup (window x 16, y 568): centre of row 0.
fn in_row0() -> (f32, f32) {
    (60.0, bottom_y() + 59.0 + crate::recording::IN_PAD + crate::recording::IN_ROW / 2.0)
}
fn record_btn() -> (f32, f32) {
    (geo().transport_x + 423.0 + 14.0, 38.0 + 14.0)
}
/// Settings panel origin (a 960 x 820 pt window at x 255, y 6.5, in 85 % units) and width.
fn set_x() -> f32 {
    geo().settings_x
}
fn set_y() -> f32 {
    geo().settings_y
}
fn set_w() -> f32 {
    geo().settings_w
}
/// Settings panel sidebar row centres.
fn settings_row(i: usize) -> (f32, f32) {
    (set_x() + 10.0 + 83.0, set_y() + 94.75 + i as f32 * 27.25 + 12.75)
}

/// Injected test input: 1 s of a 440 Hz sine at -6 dBFS, mono.
const INJ_FREQ: f32 = 440.0;
const INJ_AMP: f32 = 0.5;

/// Arm / monitor / input / record transport / settings (appended last). Never opens a
/// real input device: the In popup lists only the configured input device (none in the
/// self-test), and recording uses `Engine::render_offline_with_input` (offline run only).
fn recording_steps() -> Vec<Step> {
    use Step::*;
    let hist = Rc::new(Cell::new(0usize));
    let rec_n = Rc::new(Cell::new(0usize)); // audio track 1 clip count before recording
    let h = || hist.clone();
    // One new undo step with this label since the last record; re-records.
    fn one(hc: &Rc<Cell<usize>>, c: &Controller, label: &str) -> Result<(), String> {
        let n = c.doc.history_labels().len();
        let was = hc.get();
        hc.set(n);
        (n == was + 1 && c.doc.undo_label() == Some(label))
            .then_some(())
            .ok_or(format!("history {was} -> {n}, last {:?} (want {label})", c.doc.undo_label()))
    }
    let (h1, h2, h3, h4, h5, h6, h7, h8, h9, h10, h11) = (h(), h(), h(), h(), h(), h(), h(), h(), h(), h(), h());
    let (h12, h13, h14) = (h(), h(), h());
    let (rn1, rn2, rn3) = (rec_n.clone(), rec_n.clone(), rec_n.clone());
    let pre_hist = Rc::new(Cell::new(0usize)); // history length before the recording pass
    let (ph1, ph2) = (pre_hist.clone(), pre_hist.clone());
    fn status(a: &AppWindow) -> String {
        a.global::<Studio>().get_status_text().to_string()
    }
    fn offline(c: &Controller) -> bool {
        !c.engine.device_running()
    }
    let mut v = vec![
        Do(
            "Enter; Escape (close menus)",
            Box::new(|a| {
                key(a, Key::Return, false, false);
                key(a, Key::Escape, false, false);
            }),
        ),
        DoC(
            "rec: select audio track 1 (header), Devices tab",
            Box::new(|a, ctl| {
                let sy = ctl.borrow().view.scroll_y as f32;
                click(a, tracks_x() + 37.35, track0_y() + 10.0 - sy);
                if ctl.borrow().bottom_tab == crate::controller::BottomTab::Clip {
                    click(a, bottom_x() + 286.0 + 15.0, bottom_y() + 14.0);
                }
            }),
        ),
        Check(
            "rec: nothing armed with an input; record history",
            Box::new(move |c, a| {
                h1.set(c.doc.history_labels().len());
                let t = c.project().tracks.first().ok_or("no tracks")?;
                (c.armed_with_input().is_empty()
                    && c.selected_track == Some(t.id)
                    && t.kind == TrackKind::Audio
                    && a.global::<Studio>().get_bottom_tab() == 0)
                    .then_some(())
                    .ok_or(format!("armed {:?} sel {:?}", c.armed_with_input(), c.selected_track))
            }),
        ),
        Do("rec: click the header record button with nothing armed", Box::new(|a| click(a, record_btn().0, record_btn().1))),
        Check(
            "rec: record refused with a clear status, not recording, project unchanged",
            Box::new(move |c, a| {
                let st = status(a);
                (!c.rec.recording
                    && !a.global::<Studio>().get_recording()
                    && st.starts_with("Nothing to record")
                    && c.doc.history_labels().len() == h2.get())
                .then_some(())
                .ok_or(format!("recording {} status {st:?}", c.rec.recording))
            }),
        ),
        DoC(
            "rec: click R on track 1's header",
            Box::new(|a, ctl| {
                let (x, y) = header_r(ctl, 0);
                click(a, x, y);
            }),
        ),
        Check(
            "rec: track 1 armed (SetTrackArm), one undo step, header + inspector R on",
            Box::new(move |c, a| {
                one(&h3, c, "Arm Track")?;
                let s = a.global::<Studio>();
                let row = slint::Model::row_data(&s.get_tracks(), 0).ok_or("no row")?;
                (c.project().tracks[0].arm && row.arm && s.get_sel_arm()).then_some(()).ok_or("not armed".into())
            }),
        ),
        DoC(
            "rec: click R on track 1's header again",
            Box::new(|a, ctl| {
                let (x, y) = header_r(ctl, 0);
                click(a, x, y);
            }),
        ),
        Check(
            "rec: track 1 disarmed, one undo step",
            Box::new(move |c, _| {
                one(&h4, c, "Disarm Track")?;
                (!c.project().tracks[0].arm).then_some(()).ok_or("still armed".into())
            }),
        ),
        Do("rec: Shift+R (track.arm)", Box::new(|a| key(a, "R", false, true))),
        Check(
            "rec: ⇧R armed the selected track, one undo step",
            Box::new(move |c, _| {
                one(&h5, c, "Arm Track")?;
                c.project().tracks[0].arm.then_some(()).ok_or("not armed".into())
            }),
        ),
        Do("rec: click the inspector R", Box::new(|a| click(a, insp_r().0, insp_r().1))),
        Check(
            "rec: inspector R disarmed, one undo step",
            Box::new(move |c, a| {
                one(&h6, c, "Disarm Track")?;
                (!c.project().tracks[0].arm && !a.global::<Studio>().get_sel_arm()).then_some(()).ok_or("still armed".into())
            }),
        ),
        Do("rec: click the inspector I", Box::new(|a| click(a, insp_i().0, insp_i().1))),
        Check(
            "rec: I turned input monitoring on (SetTrackMonitor), one undo step, I drawn on",
            Box::new(move |c, a| {
                one(&h7, c, "Input Monitoring Change")?;
                (c.project().tracks[0].monitor && a.global::<Studio>().get_sel_monitor()).then_some(()).ok_or("monitor off".into())
            }),
        ),
        Do("rec: Shift+I (track.monitor)", Box::new(|a| key(a, "I", false, true))),
        Check(
            "rec: ⇧I turned monitoring off, one undo step",
            Box::new(move |c, _| {
                one(&h8, c, "Input Monitoring Change")?;
                (!c.project().tracks[0].monitor).then_some(()).ok_or("monitor on".into())
            }),
        ),
        DoC(
            "rec: click R on the Instrument track's header",
            Box::new(|a, ctl| {
                let i = ctl.borrow().project().tracks.iter().position(|t| t.kind == TrackKind::Instrument).unwrap_or(1);
                let (x, y) = header_r(ctl, i);
                click(a, x, y);
            }),
        ),
        Check(
            "rec: instrument R armed it (SetTrackArm, one undo step), it is a MIDI recording track",
            Box::new({
                let h9 = h9.clone();
                move |c, _| {
                    one(&h9, c, "Arm Track")?;
                    let inst = c.project().tracks.iter().find(|t| t.kind == TrackKind::Instrument).ok_or("no instrument track")?;
                    (inst.arm && c.armed_midi() == vec![inst.id]).then_some(()).ok_or("instrument track not armed".into())
                }
            }),
        ),
        DoC(
            "rec: click the Instrument track's R again",
            Box::new(|a, ctl| {
                let i = ctl.borrow().project().tracks.iter().position(|t| t.kind == TrackKind::Instrument).unwrap_or(1);
                let (x, y) = header_r(ctl, i);
                click(a, x, y);
            }),
        ),
        Check(
            "rec: second R disarmed it, one undo step",
            Box::new(move |c, _| {
                one(&h9, c, "Disarm Track")?;
                c.armed_midi().is_empty().then_some(()).ok_or("still armed".into())
            }),
        ),
        DoC(
            "rec: re-select track 1, click the inspector In field",
            Box::new(|a, ctl| {
                let sy = ctl.borrow().view.scroll_y as f32;
                click(a, tracks_x() + 37.35, track0_y() + 10.0 - sy);
                click(a, insp_in().0, insp_in().1);
            }),
        ),
        Check(
            "rec: In popup open, \"None\" first and checked, device inputs listed (or the no-device hint)",
            Box::new(|c, a| {
                let s = a.global::<Studio>();
                let items = s.get_input_menu_items();
                let rows: Vec<_> = (0..slint::Model::row_count(&items)).filter_map(|i| slint::Model::row_data(&items, i)).collect();
                let first = rows.first().ok_or("empty popup")?;
                let want = match c.current_input_device() {
                    Ok(Some(d)) => crate::recording::input_choices(Some(&d)).len(),
                    _ => 1,
                };
                let entries = rows.iter().filter(|r| !r.hint).count();
                let hint_ok = entries > 1 || rows.iter().any(|r| r.hint);
                (s.get_input_menu_open() && first.label == "None" && first.checked && !first.hint && entries == want && hint_ok)
                    .then_some(())
                    .ok_or(format!(
                        "open {} rows {:?}",
                        s.get_input_menu_open(),
                        rows.iter().map(|r| r.label.to_string()).collect::<Vec<_>>()
                    ))
            }),
        ),
        Do("rec: click \"None\" in the popup", Box::new(|a| click(a, in_row0().0, in_row0().1))),
        Check(
            "rec: popup closed; choosing the current entry made no undo step",
            Box::new(move |c, a| {
                (!a.global::<Studio>().get_input_menu_open()
                    && c.doc.history_labels().len() == h10.get()
                    && c.project().tracks[0].input.is_none())
                .then_some(())
                .ok_or("popup open or project changed".into())
            }),
        ),
        DoC(
            "rec: pick a device input in the popup, or (COMMAND PATH: no device inputs are listed in the self-test) SetTrackInput Input 1 (Mono)",
            Box::new(|a, ctl| {
                click(a, insp_in().0, insp_in().1);
                let real = ctl.borrow().rec.input_menu.as_ref().is_some_and(|m| m.len() > 1);
                if real {
                    click(a, in_row0().0, in_row0().1 + crate::recording::IN_ROW);
                } else {
                    key(a, Key::Escape, false, false);
                    let mut c = ctl.borrow_mut();
                    let t = c.project().tracks[0].id;
                    c.set_track_input(t, Some(dstudio_core::InputSource { channel: 0, stereo: false }));
                }
            }),
        ),
        Check(
            "rec: track 1 input = Input 1 (Mono) (SetTrackInput), one undo step, In field shows it",
            Box::new(move |c, a| {
                one(&h11, c, "Input Change")?;
                let s = a.global::<Studio>();
                (c.project().tracks[0].input == Some(dstudio_core::InputSource { channel: 0, stereo: false })
                    && s.get_sel_input_text() == "Input 1 (Mono)"
                    && !s.get_input_menu_open())
                .then_some(())
                .ok_or(format!("input {:?} text {}", c.project().tracks[0].input, s.get_sel_input_text()))
            }),
        ),
        // ---------------- settings ----------------
        Do("settings: Cmd+, (app.settings)", Box::new(|a| key(a, ",", true, false))),
        Check(
            "settings: open on Audio with output/input/mute/sample-rate/buffer controls; 9 sidebar pages",
            Box::new(|c, a| {
                let s = a.global::<Studio>();
                let pages: Vec<String> = (0..slint::Model::row_count(&s.get_settings_pages()))
                    .filter_map(|i| slint::Model::row_data(&s.get_settings_pages(), i))
                    .map(|p| p.to_string())
                    .collect();
                let ok = c.settings.open
                    && s.get_settings_open()
                    && s.get_settings_page() == 1
                    && pages == crate::settings::PAGES
                    && !s.get_set_output_text().is_empty()
                    && s.get_set_input_text() == "None"
                    && s.get_set_rate_text().ends_with("kHz")
                    && !s.get_set_buffer_text().is_empty()
                    && !s.get_set_input_muted();
                ok.then_some(()).ok_or(format!(
                    "open {} page {} out {:?} in {:?} rate {:?}",
                    s.get_settings_open(),
                    s.get_settings_page(),
                    s.get_set_output_text(),
                    s.get_set_input_text(),
                    s.get_set_rate_text()
                ))
            }),
        ),
        Do("settings: click the Buffer size dropdown", Box::new(|a| click(a, set_x() + 187.0 + 300.0, set_y() + 312.0 + 14.0))),
        Check(
            "settings: buffer list 64..2048 with ms readouts",
            Box::new(|_, a| {
                let s = a.global::<Studio>();
                let items = s.get_settings_menu_items();
                let labels: Vec<String> = (0..slint::Model::row_count(&items))
                    .filter_map(|i| slint::Model::row_data(&items, i))
                    .map(|r| r.label.to_string())
                    .collect();
                (s.get_settings_menu_open()
                    && labels.len() == 6
                    && labels[0].starts_with("64 samples (")
                    && labels[5].starts_with("2048 samples (")
                    && labels.iter().all(|l| l.ends_with(" ms)")))
                .then_some(())
                .ok_or(format!("{labels:?}"))
            }),
        ),
        Do(
            "settings: Escape (closes the list), press R (blocked while Settings is open), click General",
            Box::new(|a| {
                key(a, Key::Escape, false, false);
                key(a, "r", false, false);
                let (x, y) = settings_row(0);
                click(a, x, y);
            }),
        ),
        Check(
            "settings: list closed, R did nothing, General is a placeholder page",
            Box::new(move |c, a| {
                let s = a.global::<Studio>();
                (s.get_settings_open()
                    && !s.get_settings_menu_open()
                    && s.get_settings_page() == 0
                    && !c.rec.recording
                    && c.doc.history_labels().len() == h12.get())
                .then_some(())
                .ok_or(format!("open {} menu {} page {}", s.get_settings_open(), s.get_settings_menu_open(), s.get_settings_page()))
            }),
        ),
        Do(
            "settings: click Audio, then Escape",
            Box::new(|a| {
                let (x, y) = settings_row(1);
                click(a, x, y);
                key(a, Key::Escape, false, false);
            }),
        ),
        Check(
            "settings: Escape closed the window",
            Box::new(|c, a| (!c.settings.open && !a.global::<Studio>().get_settings_open()).then_some(()).ok_or("still open".into())),
        ),
        Do("settings: Cmd+, again (Audio page)", Box::new(|a| key(a, ",", true, false))),
        Check(
            "pdc: compensation on by default, latency readout shows the engine report",
            Box::new(|c, a| {
                let s = a.global::<Studio>();
                let txt = s.get_set_latency_text().to_string();
                let max = c.engine.latency_report().tracks.iter().map(|(_, l)| *l).max().unwrap_or(0);
                let ok_txt = if max == 0 { txt == "Plugin latency: none" } else { txt.contains(&format!("({max} samples)")) };
                (c.engine.delay_compensation_enabled() && s.get_set_pdc_on() && ok_txt).then_some(()).ok_or(format!(
                    "enabled {} ui {} text {txt:?} max {max}",
                    c.engine.delay_compensation_enabled(),
                    s.get_set_pdc_on()
                ))
            }),
        ),
        Do(
            "pdc: click the Plugin delay compensation switch",
            Box::new(|a| click(a, set_x() + set_w() - 41.0 + 10.25, set_y() + 355.0 + 5.0)),
        ),
        Check(
            "pdc: switched off in the engine, the UI and the (in-memory) preferences",
            Box::new(|c, a| {
                (!c.engine.delay_compensation_enabled() && !a.global::<Studio>().get_set_pdc_on() && c.settings.prefs.audio.pdc_off)
                    .then_some(())
                    .ok_or(format!(
                        "engine {} ui {} pref_off {}",
                        c.engine.delay_compensation_enabled(),
                        a.global::<Studio>().get_set_pdc_on(),
                        c.settings.prefs.audio.pdc_off
                    ))
            }),
        ),
        Do(
            "pdc: click the switch again, then Escape",
            Box::new(|a| {
                click(a, set_x() + set_w() - 41.0 + 10.25, set_y() + 355.0 + 5.0);
                key(a, Key::Escape, false, false);
            }),
        ),
        Check(
            "pdc: back on; Settings closed",
            Box::new(|c, a| {
                (c.engine.delay_compensation_enabled() && !c.settings.prefs.audio.pdc_off && !a.global::<Studio>().get_settings_open())
                    .then_some(())
                    .ok_or("pdc not restored or settings still open".into())
            }),
        ),
        DoC(
            "pdc (offline): add the engine's test latency plugin (4800 frames) to track 1, play 1 s offline",
            Box::new(|_, ctl| {
                let mut c = ctl.borrow_mut();
                if c.engine.device_running() {
                    return; // offline render is refused while the device owns the audio path
                }
                let track = c.project().tracks[0].id;
                let plugin = dstudio_core::PluginRef {
                    format: dstudio_core::PluginFormat::Clap,
                    id: "studio.dstudio.test.latency".into(),
                    path: Some(dstudio_engine::test_plugin_dir().join("DStudioTestLatency.clap").to_string_lossy().into_owned()),
                    name: "DStudio Test Latency".into(),
                    vendor: "DStudio".into(),
                    version: "1.0.0".into(),
                    kind: dstudio_core::PluginKind::Effect,
                };
                let mut dev = dstudio_core::Device::new(plugin);
                dev.state = Some(dstudio_core::device::b64::encode(&4800u32.to_le_bytes()));
                c.execute(dstudio_core::Command::AddDevice { track, index: None, device: dev });
                let e = &c.engine;
                let _ = e.seek(0);
                let _ = e.play();
                let mut buf = vec![0f32; 2 * 48_000];
                let _ = e.render_offline(&mut buf);
            }),
        ),
        Check(
            "pdc (offline): L = 4800; audible position and Time readout are 100 ms behind the transport",
            Box::new(|c, a| {
                if c.engine.device_running() {
                    return Ok(());
                }
                let l = c.engine.latency_report().total_frames;
                let pos = c.engine.transport().position;
                let aud = c.engine.audible_position();
                let t = a.global::<Studio>().get_time_text().to_string();
                (l == 4800 && pos == 48_000 && aud == 48_000 - 4800 && t == "00:00.900")
                    .then_some(())
                    .ok_or(format!("L {l} pos {pos} audible {aud} time {t:?} load errors {:?}", c.engine.plugin_load_errors()))
            }),
        ),
        DoC(
            "pdc (offline): Enter (stop), Cmd+Z (remove the plugin)",
            Box::new(|a, ctl| {
                if ctl.borrow().engine.device_running() {
                    return; // nothing was added in device mode; an undo here would revert an unrelated edit
                }
                key(a, Key::Return, false, false);
                key(a, "z", true, false);
            }),
        ),
        Check(
            "pdc (offline): plugin removed by undo, no latency left",
            Box::new(|c, _| {
                if c.engine.device_running() {
                    return Ok(());
                }
                let ok = c.project().tracks[0].devices.iter().all(|d| d.plugin.id != "studio.dstudio.test.latency");
                (ok && c.engine.latency_report().total_frames == 0).then_some(()).ok_or(format!(
                    "devices {:?} L {}",
                    c.project().tracks[0].devices.len(),
                    c.engine.latency_report().total_frames
                ))
            }),
        ),
        // ---------------- recording with injected input (offline only) ----------------
        DoC(
            "rec: Shift+R (arm track 1), click the ruler at bar 2",
            Box::new(|a, ctl| {
                key(a, "R", false, true);
                let x = lane_x() + ctl.borrow().view.x_of(4.0);
                click(a, x, lane_y0() + 6.5);
            }),
        ),
        Check(
            "rec: track 1 armed with Input 1, playhead at beat 4; record history",
            Box::new(move |c, _| {
                rn1.set(c.project().tracks[0].clips.len());
                h13.set(c.doc.history_labels().len());
                ph1.set(c.doc.history_labels().len());
                let at = c.playhead_ticks();
                (c.armed_with_input() == vec![c.project().tracks[0].id] && (at - 4 * PPQ).abs() <= 1)
                    .then_some(())
                    .ok_or(format!("armed {:?} playhead {at}", c.armed_with_input()))
            }),
        ),
        Do("rec: press R (transport.record)", Box::new(|a| key(a, "r", false, false))),
        Check(
            "rec: recording started (engine.start_recording), record button red [needs the engine recording API]",
            Box::new(move |c, a| {
                if !offline(c) {
                    return Ok(()); // --self-test-audio: injected recording runs offline only
                }
                let st = c.engine.recording_state();
                (c.rec.recording && a.global::<Studio>().get_recording() && st.active).then_some(()).ok_or(format!(
                    "not recording: {:?} (engine active {})",
                    status(a),
                    st.active
                ))
            }),
        ),
        Check(
            "rec: rendered 1 s of injected 440 Hz input (render_offline_with_input) [needs the engine recording API]",
            Box::new(|c, _| {
                if !offline(c) || !c.rec.recording {
                    return if offline(c) { Err("not recording".into()) } else { Ok(()) };
                }
                let n = 48_000usize;
                let input: Vec<f32> = (0..n).map(|i| INJ_AMP * (std::f32::consts::TAU * INJ_FREQ * i as f32 / 48_000.0).sin()).collect();
                let mut out = vec![0f32; 2 * n];
                c.engine.render_offline_with_input(&input, 1, &mut out).map_err(|e| e.to_string())
            }),
        ),
        DoC("rec: rebuild the live timeline clips", Box::new(|_, ctl| ctl.borrow_mut().rebuild_vis())),
        Check(
            "rec: live clip on track 1 from recording_state(), ~1 s long, live peaks drawn [needs the engine recording API]",
            Box::new(|c, _| {
                if !offline(c) {
                    return Ok(());
                }
                let v = c.rec.live_vis.first().ok_or("no live clip")?;
                let secs = v.len_beats * 60.0 / c.project().transport.bpm;
                let peak = v.live.as_ref().map(|(p, _)| p.iter().fold(0f32, |m, x| m.max(x[1]))).unwrap_or(0.0);
                (c.rec.live_vis.len() == 1 && v.track_index == 0 && (secs - 1.0).abs() < 0.05 && (peak - INJ_AMP).abs() < 0.05)
                    .then_some(())
                    .ok_or(format!("{} live clips, {secs:.3} s, peak {peak}", c.rec.live_vis.len()))
            }),
        ),
        Do("rec: press R (stop; the take loads on a worker)", Box::new(|a| key(a, "r", false, false))),
        Check(
            "rec: one \"Record\" undo step added a clip at beat 4 on track 1 [needs the engine recording API]",
            Box::new(move |c, _| {
                if !offline(c) {
                    return Ok(());
                }
                one(&h14, c, "Record")?;
                let t = &c.project().tracks[0];
                let clip = t.clips.iter().max_by_key(|cl| cl.id.0).ok_or("no clip")?;
                // Offline the engine reports no device latency, so the take starts at the playhead (±1 ms).
                (t.clips.len() == rn2.get() + 1 && (clip.start - 4 * PPQ).abs() <= 2).then_some(()).ok_or(format!(
                    "{} clips (was {}), newest at {} ticks",
                    t.clips.len(),
                    rn2.get(),
                    clip.start
                ))
            }),
        ),
        Check(
            "rec: the take decodes to the injected signal (440 Hz, -6 dBFS, 1 s) [needs the engine recording API]",
            Box::new(|c, _| {
                if !offline(c) {
                    return Ok(());
                }
                let dir = std::fs::canonicalize(c.recording_dir()).unwrap_or_else(|_| c.recording_dir());
                let p = c.project();
                let (clip, asset) = p.tracks[0]
                    .clips
                    .iter()
                    .filter_map(|cl| p.asset(cl.asset).map(|a| (cl, a)))
                    .find(|(_, a)| std::path::Path::new(&a.absolute_path).starts_with(&dir))
                    .ok_or(format!("no recorded take on track 1 (nothing under {})", dir.display()))?;
                let (rate, ch, s) = crate::wavgen::read_wav(std::path::Path::new(&asset.absolute_path))?;
                let mono: Vec<f32> = s.chunks(ch as usize).map(|f| f[0]).skip(clip.source_offset as usize).collect();
                let secs = mono.len() as f32 / rate as f32;
                let rms = (mono.iter().map(|x| x * x).sum::<f32>() / mono.len().max(1) as f32).sqrt();
                let crossings = mono.windows(2).filter(|w| w[0] <= 0.0 && w[1] > 0.0).count() as f32 / secs.max(1e-6);
                let peak = c
                    .engine
                    .peaks(c.asset_map.get(&asset.id).copied().ok_or("asset not loaded")?, 0, 4800, 10)
                    .map_err(|e| e.to_string())?
                    .iter()
                    .fold(0f32, |m, p| m.max(p[1]));
                ((secs - 1.0).abs() < 0.02
                    && (rms - INJ_AMP / 2f32.sqrt()).abs() < 0.02
                    && (crossings - INJ_FREQ).abs() < 3.0
                    && (peak - INJ_AMP).abs() < 0.02)
                    .then_some(())
                    .ok_or(format!("{secs:.3} s, rms {rms:.4}, {crossings:.1} Hz, engine peak {peak:.3}"))
            }),
        ),
        Do(
            "rec: Cmd+Z, Enter",
            Box::new(|a| {
                key(a, "z", true, false);
                key(a, Key::Return, false, false);
            }),
        ),
        Check(
            "rec: ⌘Z removed the recorded clip and its asset [needs the engine recording API]",
            Box::new(move |c, _| {
                if !offline(c) {
                    return Ok(());
                }
                let t = &c.project().tracks[0];
                (t.clips.len() == rn3.get() && c.doc.history_labels().len() == ph2.get() && c.doc.undo_label() != Some("Record"))
                    .then_some(())
                    .ok_or(format!("{} clips, undo {:?}", t.clips.len(), c.doc.undo_label()))
            }),
        ),
    ];
    v.extend(midi_recording_steps());
    v.extend(count_in_steps());
    v.extend(loop_take_steps());
    v
}

/// Takes from a looped recording: record track 1 three and a half times round a 2 s loop with a
/// louder signal each pass, then check the clip covers the loop, plays the last whole pass and
/// lists every pass under Takes, and that picking Take 1 plays the first pass. Offline only.
fn loop_take_steps() -> Vec<Step> {
    use Step::*;
    const LOOP: i64 = 96_000; // beats 4..8 at 120 bpm, 48 kHz
    const AMPS: [f32; 4] = [0.2, 0.4, 0.6, 0.8];
    fn offline(c: &Controller) -> bool {
        !c.engine.device_running()
    }
    fn take_clip(c: &Controller) -> Option<&dstudio_core::Clip> {
        c.project().tracks[0].clips.iter().find(|cl| !cl.takes.is_empty())
    }
    /// RMS of what clip `cl` plays from its file.
    fn played_rms(c: &Controller, cl: &dstudio_core::Clip) -> Result<f32, String> {
        let a = c.project().asset(cl.asset).ok_or("no asset")?;
        let (_, ch, s) = crate::wavgen::read_wav(std::path::Path::new(&a.absolute_path))?;
        let mono: Vec<f32> = s.chunks(ch as usize).map(|f| f[0]).skip(cl.source_offset as usize).take(cl.source_length as usize).collect();
        Ok((mono.iter().map(|x| x * x).sum::<f32>() / mono.len().max(1) as f32).sqrt())
    }
    fn menu_row(a: &AppWindow, label: &str) -> Option<(f32, f32)> {
        let s = a.global::<Studio>();
        let items = s.get_menu_items();
        (0..slint::Model::row_count(&items))
            .filter_map(|i| slint::Model::row_data(&items, i))
            .find(|m| m.label == label && m.enabled)
            .map(|m| (s.get_menu_x() + 40.0, s.get_menu_y() + m.y + crate::clipedit::MENU_ROW / 2.0))
    }
    fn sub_rows(a: &AppWindow) -> Vec<(String, String)> {
        let items = a.global::<Studio>().get_sub_items();
        (0..slint::Model::row_count(&items))
            .filter_map(|i| slint::Model::row_data(&items, i))
            .map(|m| (m.label.to_string(), m.shortcut.to_string()))
            .collect()
    }
    fn sub_row(a: &AppWindow, label: &str) -> Option<(f32, f32)> {
        let s = a.global::<Studio>();
        let items = s.get_sub_items();
        (0..slint::Model::row_count(&items)).filter_map(|i| slint::Model::row_data(&items, i)).find(|m| m.label == label).map(|m| {
            (s.get_menu_x() + crate::clipedit::MENU_W - 4.0 + 40.0, s.get_menu_y() + s.get_sub_y() + m.y + crate::clipedit::MENU_ROW / 2.0)
        })
    }
    let hist = Rc::new(Cell::new(0usize));
    let (h1, h2) = (hist.clone(), hist.clone());
    vec![
        DoC(
            "takes: loop beats 4 to 8, track 1 armed on Input 1, playhead at beat 4",
            Box::new(move |_, ctl| {
                let mut c = ctl.borrow_mut();
                if !offline(&c) {
                    return;
                }
                h1.set(c.doc.history_labels().len());
                let t = c.project().tracks[0].id;
                c.execute(dstudio_core::Command::SetLoop { enabled: true, start: 4 * PPQ, end: 8 * PPQ });
                c.set_track_input(t, Some(dstudio_core::InputSource { channel: 0, stereo: false }));
                if !c.project().tracks[0].arm {
                    c.execute(dstudio_core::Command::SetTrackArm { track: t, arm: true });
                }
                c.seek_beats(4.0);
            }),
        ),
        Do("takes: press R", Box::new(|a| key(a, "r", false, false))),
        DoC(
            "takes: play 3.5 times round the loop, each pass louder, then press R (stop)",
            Box::new(|a, ctl| {
                {
                    let c = ctl.borrow();
                    if !offline(&c) || !c.rec.recording {
                        return;
                    }
                    let n = (LOOP * 7 / 2) as usize;
                    let input: Vec<f32> =
                        (0..n).map(|i| AMPS[i / LOOP as usize] * (std::f32::consts::TAU * INJ_FREQ * i as f32 / 48_000.0).sin()).collect();
                    // In blocks, with a pause after each, so the recorder's disk writer keeps
                    // up with the 2 s ring on slow machines (rendering is faster than real time).
                    const BLOCK: usize = 24_000;
                    let mut out = vec![0f32; 2 * BLOCK];
                    for chunk in input.chunks(BLOCK) {
                        let _ = c.engine.render_offline_with_input(chunk, 1, &mut out[..2 * chunk.len()]);
                        std::thread::sleep(std::time::Duration::from_millis(30));
                    }
                }
                ctl.borrow_mut().midi_tick();
                key(a, "r", false, false);
            }),
        ),
        Check(
            "takes: one Record step; the clip covers the loop, plays the third pass and has three takes",
            Box::new(|c, a| {
                if !offline(c) {
                    return Ok(());
                }
                let cl = take_clip(c).ok_or(format!("no clip with takes; undo {:?}", c.doc.undo_label()))?;
                let len = c.project().clip_length_ticks(cl);
                let rms = played_rms(c, cl)?;
                let status = a.global::<Studio>().get_status_text().to_string();
                (c.doc.undo_label() == Some("Record")
                    && (cl.start - 4 * PPQ).abs() <= 2
                    && (len - 4 * PPQ).abs() <= 2
                    && cl.takes.len() == 3
                    && cl.current_take() == Some(2)
                    && (rms - AMPS[2] / 2f32.sqrt()).abs() < 0.02
                    && status.contains("3 loop takes"))
                .then_some(())
                .ok_or(format!(
                    "start {} len {len} takes {:?} current {:?} rms {rms:.3} status {status:?}",
                    cl.start,
                    cl.takes,
                    cl.current_take()
                ))
            }),
        ),
        DoC(
            "takes: open the clip's menu",
            Box::new(|_, ctl| {
                let mut c = ctl.borrow_mut();
                let Some(id) = take_clip(&c).map(|cl| cl.id) else { return };
                let win = c.devices.win;
                c.open_clip_menu(id, 500.0, 300.0, win);
            }),
        ),
        Do(
            "takes: click Takes",
            Box::new(|a| {
                if let Some((x, y)) = menu_row(a, "Takes") {
                    click(a, x, y);
                }
            }),
        ),
        Check(
            "takes: the submenu lists Take 1 to Take 3 with Take 3 ticked",
            Box::new(|c, a| {
                if !offline(c) {
                    return Ok(());
                }
                let rows = sub_rows(a);
                let want = [("Take 1", ""), ("Take 2", ""), ("Take 3", "✓")];
                (a.global::<Studio>().get_sub_open() && rows.iter().map(|(l, k)| (l.as_str(), k.as_str())).eq(want.iter().copied()))
                    .then_some(())
                    .ok_or(format!("sub open {} rows {rows:?}", a.global::<Studio>().get_sub_open()))
            }),
        ),
        Do(
            "takes: pick Take 1",
            Box::new(|a| {
                if let Some((x, y)) = sub_row(a, "Take 1") {
                    click(a, x, y);
                }
            }),
        ),
        Check(
            "takes: the clip plays the first pass, one Choose Take step, the name shows Take 1",
            Box::new(|c, _| {
                if !offline(c) {
                    return Ok(());
                }
                let cl = take_clip(c).ok_or("no clip with takes")?;
                let rms = played_rms(c, cl)?;
                let i = c.clip_ids.iter().position(|&k| k == cl.id).ok_or("clip not drawn")?;
                let name = &c.clip_vis[i].name;
                (cl.current_take() == Some(0)
                    && c.doc.undo_label() == Some("Choose Take")
                    && (rms - AMPS[0] / 2f32.sqrt()).abs() < 0.02
                    && name.ends_with("Take 1")
                    && !c.menu.open)
                    .then_some(())
                    .ok_or(format!("current {:?} undo {:?} rms {rms:.3} name {name:?}", cl.current_take(), c.doc.undo_label()))
            }),
        ),
        DoC(
            "takes: undo back to before the recording, loop off, disarm",
            Box::new(move |_, ctl| {
                let mut c = ctl.borrow_mut();
                if !offline(&c) {
                    return;
                }
                while c.doc.history_labels().len() > h2.get() && c.doc.undo_label().is_some() {
                    c.undo();
                }
                let _ = c.engine.stop();
            }),
        ),
        Check(
            "takes: the clip, the loop and the arm are undone",
            Box::new(|c, _| {
                if !offline(c) {
                    return Ok(());
                }
                (take_clip(c).is_none() && !c.project().transport.loop_enabled && !c.project().tracks[0].arm).then_some(()).ok_or(format!(
                    "takes clip {} loop {} arm {}",
                    take_clip(c).is_some(),
                    c.project().transport.loop_enabled,
                    c.project().tracks[0].arm
                ))
            }),
        ),
    ]
}

/// MIDI recording (spec 003): arm the Instrument track, play injected notes during a
/// pass, stop: one "Record" step adds a MIDI clip with those notes; undo removes it.
/// Offline only (injected MIDI is rendered with `render_offline`).
fn midi_recording_steps() -> Vec<Step> {
    use Step::*;
    let before = Rc::new(Cell::new(usize::MAX)); // instrument track clip count, history length
    let hist = Rc::new(Cell::new(0usize));
    let (b1, b2, b3, h1, h2) = (before.clone(), before.clone(), before.clone(), hist.clone(), hist.clone());
    fn offline(c: &Controller) -> bool {
        !c.engine.device_running()
    }
    fn inst(c: &Controller) -> Option<&dstudio_core::Track> {
        c.project().tracks.iter().find(|t| t.kind == TrackKind::Instrument)
    }
    fn render(c: &Controller, frames: usize) -> Result<(), String> {
        let mut out = vec![0f32; 2 * frames];
        c.engine.render_offline(&mut out).map_err(|e| e.to_string())
    }
    let peak = Rc::new(Cell::new(-1f32));
    let (pk1, pk2) = (peak.clone(), peak.clone());
    vec![
        DoC(
            "midi play: select the Instrument track (not armed or monitoring) and play a key",
            Box::new(move |_, ctl| {
                let mut c = ctl.borrow_mut();
                if !offline(&c) {
                    return;
                }
                let Some((t, quiet)) = inst(&c).map(|t| (t.id, !t.arm && !t.monitor)) else { return };
                if !quiet {
                    return;
                }
                c.select_track(t);
                c.midi_tick();
                let _ = c.engine.midi_inject(0x90, 60, 110);
                let mut out = vec![0f32; 2 * 8192];
                let ok = c.engine.render_offline(&mut out).is_ok();
                let _ = c.engine.midi_inject(0x80, 60, 0);
                let _ = render(&c, 512);
                if ok {
                    pk1.set(out.iter().fold(0f32, |m, v| m.max(v.abs())));
                }
            }),
        ),
        Check(
            "midi play: a selected track that is not armed or monitoring does not play the keyboard",
            Box::new(move |c, _| {
                if !offline(c) || pk2.get() < 0.0 {
                    return Ok(());
                }
                // Silence, up to tails of earlier steps (a played note peaks far above -60 dBFS).
                (pk2.get() < 1e-3).then_some(()).ok_or(format!("peak {}", pk2.get()))
            }),
        ),
        DoC(
            "midi rec: disarm audio track 1, arm the Instrument track, select it, playhead at beat 4",
            Box::new(|_, ctl| {
                let mut c = ctl.borrow_mut();
                let audio = c.project().tracks[0].id;
                if c.project().tracks[0].arm {
                    c.toggle_arm(audio);
                }
                if let Some(t) = inst(&c).map(|t| t.id) {
                    c.toggle_arm(t);
                    c.select_track(t);
                }
                c.seek_beats(4.0);
                c.midi_tick();
            }),
        ),
        Check(
            "midi rec: only the Instrument track is armed, record button ready",
            Box::new(move |c, a| {
                let t = inst(c).ok_or("no instrument track")?;
                b1.set(t.midi_clips.len());
                h1.set(c.doc.history_labels().len());
                (c.armed_with_input().is_empty() && c.armed_midi() == vec![t.id] && a.global::<Studio>().get_record_ready())
                    .then_some(())
                    .ok_or(format!("armed audio {:?}, midi {:?}", c.armed_with_input(), c.armed_midi()))
            }),
        ),
        Do("midi rec: press R (transport.record)", Box::new(|a| key(a, "r", false, false))),
        Check(
            "midi rec: recording a MIDI-only pass (transport playing, no audio takes)",
            Box::new(|c, a| {
                if !offline(c) {
                    return Ok(());
                }
                (c.rec.recording
                    && !c.rec.audio
                    && c.midi_in.pass.is_some()
                    && c.engine.transport().playing
                    && a.global::<Studio>().get_recording())
                .then_some(())
                .ok_or(format!("recording {} audio {} status {:?}", c.rec.recording, c.rec.audio, a.global::<Studio>().get_status_text()))
            }),
        ),
        DoC(
            "midi rec: play C4 for a beat, then E4 held until stop (injected MIDI, 1 s rendered)",
            Box::new(|_, ctl| {
                let mut c = ctl.borrow_mut();
                if !offline(&c) || !c.rec.recording {
                    return;
                }
                let _ = c.engine.midi_inject(0x90, 60, 100);
                let _ = render(&c, 24_000);
                let _ = c.engine.midi_inject(0x80, 60, 0);
                let _ = c.engine.midi_inject(0x90, 64, 90);
                let _ = render(&c, 24_000);
                c.midi_tick();
                c.rebuild_vis();
            }),
        ),
        Check(
            "midi rec: a red \"Recording\" clip with both notes grows on the Instrument track",
            Box::new(|c, _| {
                if !offline(c) {
                    return Ok(());
                }
                let ti = c.project().tracks.iter().position(|t| t.kind == TrackKind::Instrument).ok_or("no instrument track")?;
                let v = c.rec.live_vis.iter().find(|v| v.track_index == ti).ok_or("no live MIDI clip")?;
                let notes = v.midi.as_ref().map(|m| m.len()).unwrap_or(0);
                (v.name == "Recording" && notes == 2 && (v.start_beats - 4.0).abs() < 1e-6 && (v.len_beats - 2.0).abs() < 0.01)
                    .then_some(())
                    .ok_or(format!("{notes} notes, start {} len {}", v.start_beats, v.len_beats))
            }),
        ),
        DoC(
            "midi rec: try to disarm the Instrument track (R) while recording",
            Box::new(|_, ctl| {
                let mut c = ctl.borrow_mut();
                if !offline(&c) || !c.rec.recording {
                    return;
                }
                if let Some(t) = c.project().tracks.iter().find(|t| t.kind == TrackKind::Instrument).map(|t| t.id) {
                    c.toggle_arm(t);
                }
            }),
        ),
        Check(
            "midi rec: R leaves the Instrument track armed during the pass",
            Box::new(|c, _| {
                if !offline(c) || !c.rec.recording {
                    return Ok(());
                }
                let t = c.project().tracks.iter().find(|t| t.kind == TrackKind::Instrument).ok_or("no instrument track")?;
                t.arm.then_some(()).ok_or("disarmed during the pass".into())
            }),
        ),
        DoC(
            "midi rec: try to undo the Instrument track's arm while recording",
            Box::new(|_, ctl| {
                let mut c = ctl.borrow_mut();
                if offline(&c) && c.rec.recording {
                    c.undo();
                }
            }),
        ),
        Check(
            "midi rec: undo waits, the Instrument track stays armed until the pass stops",
            Box::new(|c, _| {
                if !offline(c) || !c.rec.recording {
                    return Ok(());
                }
                let t = c.project().tracks.iter().find(|t| t.kind == TrackKind::Instrument).ok_or("no instrument track")?;
                (t.arm && c.doc.undo_label().is_some()).then_some(()).ok_or(format!("arm {} undo {:?}", t.arm, c.doc.undo_label()))
            }),
        ),
        Do("midi rec: press R (stop)", Box::new(|a| key(a, "r", false, false))),
        Check(
            "midi rec: one \"Record\" undo step added a 1-bar MIDI clip at bar 2 with C4 and E4",
            Box::new(move |c, _| {
                if !offline(c) {
                    return Ok(());
                }
                let t = inst(c).ok_or("no instrument track")?;
                let n = c.doc.history_labels().len();
                if n != h2.get() + 1 || c.doc.undo_label() != Some("Record") {
                    return Err(format!("history {} -> {n}, last {:?}", h2.get(), c.doc.undo_label()));
                }
                let clip = t.midi_clips.iter().max_by_key(|cl| cl.id.0).ok_or("no clip")?;
                let got: Vec<_> = clip.notes.iter().map(|n| (n.pitch, n.start, n.length, n.velocity)).collect();
                (t.midi_clips.len() == b2.get() + 1
                    && clip.start == 4 * PPQ
                    && clip.length == 4 * PPQ
                    && clip.name == t.name
                    && got == vec![(60, 0, PPQ, 100), (64, PPQ, PPQ, 90)])
                .then_some(())
                .ok_or(format!(
                    "{} clips (was {}), newest at {} len {} notes {got:?}",
                    t.midi_clips.len(),
                    b2.get(),
                    clip.start,
                    clip.length
                ))
            }),
        ),
        Do("midi rec: Cmd+Z", Box::new(|a| key(a, "z", true, false))),
        Check(
            "midi rec: ⌘Z removed the recorded MIDI clip",
            Box::new(move |c, _| {
                if !offline(c) {
                    return Ok(());
                }
                let t = inst(c).ok_or("no instrument track")?;
                (t.midi_clips.len() == b3.get() && c.doc.undo_label() != Some("Record")).then_some(()).ok_or(format!(
                    "{} clips, undo {:?}",
                    t.midi_clips.len(),
                    c.doc.undo_label()
                ))
            }),
        ),
        DoC(
            "midi rec: stop the transport, disarm the Instrument track",
            Box::new(|_, ctl| {
                let mut c = ctl.borrow_mut();
                let _ = c.engine.stop();
                if let Some(t) = inst(&c).filter(|t| t.arm).map(|t| t.id) {
                    c.toggle_arm(t);
                }
            }),
        ),
    ]
}

/// Count-in (src/count_in.rs): the record button's menu, one bar of click before a MIDI pass,
/// and the notes played during the count-in left out of the take.
fn count_in_steps() -> Vec<Step> {
    use Step::*;
    fn offline(c: &Controller) -> bool {
        !c.engine.device_running()
    }
    fn inst(c: &Controller) -> Option<&dstudio_core::Track> {
        c.project().tracks.iter().find(|t| t.kind == TrackKind::Instrument)
    }
    fn render(c: &Controller, frames: usize) {
        let mut out = vec![0f32; 2 * frames];
        let _ = c.engine.render_offline(&mut out);
    }
    fn right_click(a: &AppWindow, x: f32, y: f32) {
        let w = a.window();
        let p = LogicalPosition::new(x, y);
        dispatch(w, WindowEvent::PointerMoved { position: p });
        dispatch(w, WindowEvent::PointerPressed { position: p, button: PointerEventButton::Right });
        dispatch(w, WindowEvent::PointerReleased { position: p, button: PointerEventButton::Right });
    }
    fn menu_row(a: &AppWindow, label: &str) -> Option<(f32, f32)> {
        let s = a.global::<Studio>();
        let items = s.get_menu_items();
        (0..items.row_count())
            .filter_map(|i| items.row_data(i))
            .find(|m| m.label == label)
            .map(|m| (s.get_menu_x() + 40.0, s.get_menu_y() + m.y + crate::clipedit::MENU_ROW / 2.0))
    }
    let clips = Rc::new(Cell::new(0usize));
    let (k1, k2) = (clips.clone(), clips.clone());
    vec![
        Do("count-in: right-click the record button", Box::new(|a| right_click(a, record_btn().0, record_btn().1))),
        Check(
            "count-in: the menu lists no count-in (ticked), 1 bar and 2 bars",
            Box::new(|c, a| {
                let items = a.global::<Studio>().get_menu_items();
                let rows: Vec<(String, String)> = (0..items.row_count())
                    .filter_map(|i| items.row_data(i))
                    .map(|m| (m.label.to_string(), m.shortcut.to_string()))
                    .collect();
                let want = [("No count-in", "✓"), ("Count-in: 1 bar", ""), ("Count-in: 2 bars", "")];
                (c.menu.open && rows.iter().map(|(l, s)| (l.as_str(), s.as_str())).eq(want.iter().copied()))
                    .then_some(())
                    .ok_or(format!("open {} rows {rows:?}", c.menu.open))
            }),
        ),
        Do(
            "count-in: choose 1 bar",
            Box::new(|a| {
                if let Some((x, y)) = menu_row(a, "Count-in: 1 bar") {
                    click(a, x, y);
                }
            }),
        ),
        DoC(
            "count-in: arm the Instrument track, metronome off, playhead at beat 8",
            Box::new(move |_, ctl| {
                let mut c = ctl.borrow_mut();
                if c.metronome_on {
                    c.toggle_metronome();
                }
                if let Some((t, armed, n)) = inst(&c).map(|t| (t.id, t.arm, t.midi_clips.len())) {
                    if !armed {
                        c.toggle_arm(t);
                    }
                    c.select_track(t);
                    k1.set(n);
                }
                let _ = c.engine.stop();
                c.seek_beats(8.0);
                c.midi_tick();
            }),
        ),
        Check(
            "count-in: set to 1 bar and saved in the preferences, menu closed",
            Box::new(|c, _| {
                (c.count_in_bars() == 1 && c.settings.prefs.count_in_bars == 1 && !c.menu.open).then_some(()).ok_or(format!(
                    "bars {} open {}",
                    c.count_in_bars(),
                    c.menu.open
                ))
            }),
        ),
        Do("count-in: press R", Box::new(|a| key(a, "r", false, false))),
        Check(
            "count-in: counting in from beat 4 (a bar early), clicking with the metronome off",
            Box::new(|c, a| {
                if !offline(c) {
                    return Ok(());
                }
                let beat = 24_000; // 120 bpm at 48 kHz
                let pos = c.engine.transport().position;
                (c.counting_in()
                    && c.rec.recording
                    && c.rec.count_in == Some(8 * beat)
                    && (pos - 4 * beat).abs() < 2 * 4096
                    && !c.metronome_on
                    && a.global::<Studio>().get_counting_in())
                .then_some(())
                .ok_or(format!("counting {} punch {:?} pos {pos}", c.counting_in(), c.rec.count_in))
            }),
        ),
        DoC(
            "count-in: play D4 during the count-in, then render past the punch-in point and play G4",
            Box::new(|_, ctl| {
                let mut c = ctl.borrow_mut();
                if !offline(&c) || !c.rec.recording {
                    return;
                }
                let _ = c.engine.midi_inject(0x90, 62, 100);
                render(&c, 24_000);
                let _ = c.engine.midi_inject(0x80, 62, 0);
                c.midi_tick();
                render(&c, 3 * 24_000);
                c.midi_tick();
                let _ = c.engine.midi_inject(0x90, 67, 90);
                render(&c, 24_000);
                let _ = c.engine.midi_inject(0x80, 67, 0);
                render(&c, 1_000);
                c.midi_tick();
            }),
        ),
        Check(
            "count-in: past the punch-in point the click stops and the pass records",
            Box::new(|c, _| {
                if !offline(c) {
                    return Ok(());
                }
                (!c.counting_in() && c.rec.recording).then_some(()).ok_or(format!("counting {}", c.counting_in()))
            }),
        ),
        Do("count-in: press R (stop)", Box::new(|a| key(a, "r", false, false))),
        Check(
            "count-in: the take starts at bar 3 and holds G4 only (D4 was in the count-in)",
            Box::new(move |c, _| {
                if !offline(c) {
                    return Ok(());
                }
                let t = inst(c).ok_or("no instrument track")?;
                let clip = t.midi_clips.iter().max_by_key(|cl| cl.id.0).ok_or("no clip")?;
                let got: Vec<_> = clip.notes.iter().map(|n| (n.pitch, n.start)).collect();
                (t.midi_clips.len() == k2.get() + 1 && clip.start == 8 * PPQ && got.len() == 1 && got[0].0 == 67)
                    .then_some(())
                    .ok_or(format!("{} clips (was {}), newest at {} notes {got:?}", t.midi_clips.len(), k2.get(), clip.start))
            }),
        ),
        DoC(
            "count-in: undo the take, count-in off, stop, disarm",
            Box::new(|_, ctl| {
                let mut c = ctl.borrow_mut();
                if c.doc.undo_label() == Some("Record") {
                    c.undo();
                }
                c.set_count_in_bars(0);
                let _ = c.engine.stop();
                if let Some(t) = inst(&c).filter(|t| t.arm).map(|t| t.id) {
                    c.toggle_arm(t);
                }
            }),
        ),
    ]
}

// ------------------------------------------------------------------ plugin devices

/// Index of the first track of `kind`.
fn track_of(c: &Controller, kind: TrackKind) -> Option<usize> {
    c.project().tracks.iter().position(|t| t.kind == kind)
}

/// Scroll the track list (wheel over the headers) so track `i` is fully visible, then click its header.
fn click_track_header(a: &AppWindow, ctl: &Ctl, i: usize) {
    let (sy, lanes_h) = {
        let c = ctl.borrow();
        (c.view.scroll_y as f32, c.lanes_size.1)
    };
    let y = track0_y() + i as f32 * PITCH - sy;
    let bottom = lane_y0() + lanes_h - 30.5;
    if y < track0_y() || y + 74.0 > bottom {
        let target = i as f32 * PITCH;
        dispatch(a.window(), WindowEvent::PointerMoved { position: headers_point() });
        dispatch(a.window(), WindowEvent::PointerScrolled { position: headers_point(), delta_x: 0.0, delta_y: sy - target });
        if let Ok(mut c) = ctl.try_borrow_mut() {
            c.update_ui(a);
        }
    }
    let sy = ctl.borrow().view.scroll_y as f32;
    click(a, tracks_x() + 37.35, track0_y() + i as f32 * PITCH + 10.0 - sy);
}

/// Window centre of browser tree row `i`.
fn tree_row_xy(a: &AppWindow, i: usize) -> (f32, f32) {
    let sc = a.global::<Studio>().get_tree_scroll();
    (geo().browser_x + 110.0, crate::devices::TREE_Y + crate::devices::TREE_TOP + i as f32 * crate::devices::TREE_ROW + 10.5 - sc)
}

fn tree_labels(a: &AppWindow) -> Vec<(i32, String, String, bool)> {
    let m = a.global::<Studio>().get_tree();
    (0..slint::Model::row_count(&m))
        .filter_map(|i| slint::Model::row_data(&m, i))
        .map(|r| (r.depth, r.label.to_string(), r.count.to_string(), r.leaf))
        .collect()
}

fn tree_row_of(a: &AppWindow, label: &str) -> Option<usize> {
    tree_labels(a).iter().position(|r| r.1 == label && r.3)
}

fn cards(a: &AppWindow) -> Vec<crate::DeviceCard> {
    let m = a.global::<Studio>().get_device_cards();
    (0..slint::Model::row_count(&m)).filter_map(|i| slint::Model::row_data(&m, i)).collect()
}

fn card_named(a: &AppWindow, name: &str) -> Option<crate::DeviceCard> {
    cards(a).into_iter().find(|c| c.name == name)
}

/// The named card after scrolling the chain until the whole card is on screen.
fn card_in_view(a: &AppWindow, ctl: &Ctl, name: &str) -> Option<crate::DeviceCard> {
    let refresh = || {
        if let Ok(mut c) = ctl.try_borrow_mut() {
            c.update_ui(a);
        }
    };
    refresh();
    let cd = card_named(a, name)?;
    let left = crate::devices::CHAIN_AREA_X + 8.0 - cd.x;
    let over = cd.x + cd.width + 5.0 - geo().bottom_w;
    if left > 0.0 {
        ctl.borrow_mut().chain_scroll_by(left);
    } else if over > 0.0 {
        ctl.borrow_mut().chain_scroll_by(-over);
    }
    refresh();
    card_named(a, name)
}

/// Card-relative point -> window point.
fn card_xy(c: &crate::DeviceCard, x: f32, y: f32) -> (f32, f32) {
    (5.0 + c.x + x, bottom_y() + 34.0 + y)
}

/// Centre of row `i` of the device popup list (same clamping as ui/app.slint), scrolled into view.
fn devmenu_row(a: &AppWindow, i: usize) -> (f32, f32) {
    let s = a.global::<Studio>();
    let (w, h) = (s.get_devmenu_w(), s.get_devmenu_h());
    let x = s.get_devmenu_x().min(win().0 - 5.0 - w).max(5.0);
    let y = s.get_devmenu_y().min(win().1 - 5.0 - h).max(5.0);
    let row = s.get_devmenu_items().row_data(i).map_or(0.0, |m| m.y) + 10.25;
    // A list taller than the window scrolls: wheel a row that is out of view to the middle.
    let scroll = (row - h / 2.0).clamp(0.0, (s.get_devmenu_content_h() - h).max(0.0));
    if scroll > 0.0 {
        let p = LogicalPosition::new(x + w / 2.0, y + h / 2.0);
        dispatch(a.window(), WindowEvent::PointerMoved { position: p });
        dispatch(a.window(), WindowEvent::PointerScrolled { position: p, delta_x: 0.0, delta_y: -scroll });
    }
    (x + 40.0, y + row - scroll)
}

fn devmenu_labels(a: &AppWindow) -> Vec<(String, String, bool)> {
    let m = a.global::<Studio>().get_devmenu_items();
    (0..slint::Model::row_count(&m))
        .filter_map(|i| slint::Model::row_data(&m, i))
        .map(|r| (r.label.to_string(), r.shortcut.to_string(), r.enabled))
        .collect()
}

fn track_devices(c: &Controller, kind: TrackKind) -> Vec<(u64, String, bool)> {
    track_of(c, kind)
        .map(|i| c.project().tracks[i].devices.iter().map(|d| (d.id.0, d.plugin.name.clone(), d.enabled)).collect())
        .unwrap_or_default()
}

/// Replace the text of the focused-on-click search field (End, Backspace x n, type).
fn retype(a: &AppWindow, field: (f32, f32), text: &str) {
    click(a, field.0, field.1);
    key(a, Key::End, false, false);
    for _ in 0..24 {
        key(a, Key::Backspace, false, false);
    }
    type_text(a, text);
}

fn browser_search() -> (f32, f32) {
    let g = geo();
    (g.browser_x + 8.5 + 120.0, g.browser_y + 34.0 + 10.0)
}
fn effects_tab() -> (f32, f32) {
    let g = geo();
    (g.browser_x + 86.0, g.browser_y + 67.0)
}

/// Plugin devices (appended last). Checks marked [needs engine plugin …] depend on the
/// engine's plugin hosting; with the engine stub they FAIL (the catalogue is empty and no
/// instance exists), never pass by default.
fn device_steps() -> Vec<Step> {
    use Step::*;
    let hist = Rc::new(Cell::new(0usize));
    let h = || hist.clone();
    fn one(hc: &Rc<Cell<usize>>, c: &Controller, label: &str) -> Result<(), String> {
        let n = c.doc.history_labels().len();
        let was = hc.get();
        hc.set(n);
        (n == was + 1 && c.doc.undo_label() == Some(label))
            .then_some(())
            .ok_or(format!("history {was} -> {n}, last {:?} (want {label})", c.doc.undo_label()))
    }
    let removed = Rc::new(Cell::new(0u64));
    let (rm1, rm2) = (removed.clone(), removed.clone());
    // A lane of the removed AULowpass keeps its colour; its colour is never asked for before
    // the removal, so the name must have been recorded when the device was added.
    let rm3 = removed.clone();
    /// A plugin-parameter target on device `id` named "Cutoff" (colour key "AULowpass/Cutoff").
    fn lowpass_cutoff(id: u64) -> dstudio_core::AutomationTarget {
        dstudio_core::AutomationTarget::Plugin { device: dstudio_core::DeviceId(id), param: 0, name: "Cutoff".into() }
    }
    let scans = Rc::new(Cell::new(0usize));
    let (sc1, sc2) = (scans.clone(), scans.clone());
    // Editor step result: None = skipped (engine reports no editor), Some((opened, closed, error)).
    let editor: Rc<RefCell<Option<(bool, bool, String)>>> = Rc::new(RefCell::new(None));
    let (ed1, ed2) = (editor.clone(), editor.clone());
    let saved: Rc<RefCell<Vec<(u64, String, bool)>>> = Rc::new(RefCell::new(Vec::new()));
    let (sv1, sv2) = (saved.clone(), saved.clone());
    let save_dir = run_dir().join("devices.dsproj");
    let (sd1, sd2, sd3) = (save_dir.clone(), save_dir.clone(), save_dir);
    let (h0, h1, h2, h3, h4, h5, h6, h7, h8, h9, h10, h11, h12) = (h(), h(), h(), h(), h(), h(), h(), h(), h(), h(), h(), h(), h());
    fn scan_state(c: &Controller) -> String {
        format!("{:?}", c.devices.catalog.state)
    }
    vec![
        Do(
            "devices: Enter; Escape (close menus)",
            Box::new(|a| {
                key(a, Key::Return, false, false);
                key(a, Key::Escape, false, false);
            }),
        ),
        DoC(
            "devices: select the first audio track, Devices tab, browser Effects sub-tab",
            Box::new(|a, ctl| {
                let i = track_of(&ctl.borrow(), TrackKind::Audio).unwrap_or(0);
                click_track_header(a, ctl, i);
                if ctl.borrow().bottom_tab == crate::controller::BottomTab::Clip {
                    click(a, bottom_x() + 286.0 + 15.0, bottom_y() + 14.0);
                }
                click(a, effects_tab().0, effects_tab().1);
            }),
        ),
        Check(
            "devices: Effects tab lists Audio Units with plugins (catalogue scanned) [needs engine plugin scanning]",
            Box::new(|c, a| {
                let rows = tree_labels(a);
                let au = rows.iter().find(|r| r.0 == 0 && r.1 == "Audio Units").map(|r| r.2.clone());
                let ok = a.global::<Studio>().get_browser_tab() == 1
                    && au.as_deref().and_then(|n| n.parse::<usize>().ok()).is_some_and(|n| n > 0);
                ok.then_some(()).ok_or(format!(
                    "tab {} Audio Units count {au:?}; scan {}",
                    a.global::<Studio>().get_browser_tab(),
                    scan_state(c)
                ))
            }),
        ),
        Do("devices: type “AULowpass” into the browser search", Box::new(|a| retype(a, browser_search(), "AULowpass"))),
        Check(
            "devices: search shows Audio Units › Apple › AULowpass; record history [needs engine plugin scanning]",
            Box::new(move |c, a| {
                h0.set(c.doc.history_labels().len());
                let rows = tree_labels(a);
                let names: Vec<&str> = rows.iter().map(|r| r.1.as_str()).collect();
                (names.first() == Some(&"Audio Units")
                    && tree_row_of(a, "AULowpass").is_some()
                    && a.global::<Studio>().get_browser_search() == "AULowpass")
                    .then_some(())
                    .ok_or(format!("rows {names:?}, search {:?}; scan {}", a.global::<Studio>().get_browser_search(), scan_state(c)))
            }),
        ),
        Do(
            "devices: double-click AULowpass",
            Box::new(|a| {
                if let Some(i) = tree_row_of(a, "AULowpass") {
                    let (x, y) = tree_row_xy(a, i);
                    double_click(a, x, y);
                }
            }),
        ),
        Check(
            "devices: AULowpass added to the audio track (AddDevice), one undo step, card shown [needs engine plugin scanning]",
            Box::new(move |c, a| {
                one(&h1, c, "Add AULowpass")?;
                let d = track_devices(c, TrackKind::Audio);
                (d.iter().map(|x| x.1.as_str()).collect::<Vec<_>>() == ["AULowpass"] && card_named(a, "AULowpass").is_some())
                    .then_some(())
                    .ok_or(format!("devices {d:?}"))
            }),
        ),
        Check(
            "engine: AULowpass is instantiated (plugin_states lists it, no load error) [needs engine plugin hosting]",
            Box::new(|c, _| {
                let id =
                    track_of(c, TrackKind::Audio).and_then(|i| c.project().tracks[i].devices.first().map(|d| d.id)).ok_or("no device")?;
                let states = c.engine.plugin_states();
                let errors = c.engine.plugin_load_errors();
                (states.iter().any(|(d, s)| *d == id && !s.is_empty()) && !errors.iter().any(|(d, _)| *d == id))
                    .then_some(())
                    .ok_or(format!("plugin_states {} entries (none for {}), load errors {errors:?}", states.len(), id.0))
            }),
        ),
        Do(
            "devices: AULowpass power toggle off",
            Box::new(|a| {
                if let Some(cd) = card_named(a, "AULowpass") {
                    let (x, y) = card_xy(&cd, 19.0, 13.25);
                    click(a, x, y);
                }
            }),
        ),
        Check(
            "devices: AULowpass bypassed (SetDeviceEnabled false), one undo step, toggle drawn off [needs engine plugin scanning]",
            Box::new(move |c, a| {
                one(&h2, c, "Bypass Device")?;
                let d = track_devices(c, TrackKind::Audio);
                (d.first().is_some_and(|x| !x.2) && card_named(a, "AULowpass").is_some_and(|cd| !cd.enabled))
                    .then_some(())
                    .ok_or(format!("devices {d:?}"))
            }),
        ),
        Do(
            "devices: AULowpass power toggle on",
            Box::new(|a| {
                if let Some(cd) = card_named(a, "AULowpass") {
                    let (x, y) = card_xy(&cd, 19.0, 13.25);
                    click(a, x, y);
                }
            }),
        ),
        Check(
            "devices: AULowpass enabled again, one undo step [needs engine plugin scanning]",
            Box::new(move |c, _| {
                one(&h3, c, "Enable Device")?;
                track_devices(c, TrackKind::Audio).first().is_some_and(|x| x.2).then_some(()).ok_or("still bypassed".into())
            }),
        ),
        DoC(
            "devices: clear the search; Add device › Audio Units",
            Box::new(|a, ctl| {
                retype(a, browser_search(), "");
                key(a, Key::Escape, false, false);
                if let Ok(mut c) = ctl.try_borrow_mut() {
                    c.update_ui(a);
                }
                let add_x = add_card_x(a, ctl);
                click(a, bottom_x() + add_x + 60.0, bottom_y() + 34.5 + 56.5 + 22.0 + 11.0);
            }),
        ),
        Check(
            "devices: Audio Units list open with AUDelay [needs engine plugin scanning]",
            Box::new(|c, a| {
                let items = devmenu_labels(a);
                (a.global::<Studio>().get_devmenu_open() && items.iter().any(|r| r.0 == "AUDelay" && r.2)).then_some(()).ok_or(format!(
                    "open {} items {:?}; scan {}",
                    a.global::<Studio>().get_devmenu_open(),
                    items.iter().take(4).collect::<Vec<_>>(),
                    scan_state(c)
                ))
            }),
        ),
        Do(
            "devices: pick AUDelay",
            Box::new(|a| {
                if let Some(i) = devmenu_labels(a).iter().position(|r| r.0 == "AUDelay") {
                    let (x, y) = devmenu_row(a, i);
                    click(a, x, y);
                } else {
                    key(a, Key::Escape, false, false);
                }
            }),
        ),
        Check(
            "devices: chain = AULowpass, AUDelay (AddDevice), one undo step, list closed [needs engine plugin scanning]",
            Box::new(move |c, a| {
                one(&h4, c, "Add AUDelay")?;
                let d = track_devices(c, TrackKind::Audio);
                (d.iter().map(|x| x.1.as_str()).collect::<Vec<_>>() == ["AULowpass", "AUDelay"] && !a.global::<Studio>().get_devmenu_open())
                    .then_some(())
                    .ok_or(format!("devices {d:?}"))
            }),
        ),
        Do(
            "devices: drag the AUDelay card (header) before AULowpass",
            Box::new(|a| {
                if let (Some(lp), Some(dl)) = (card_named(a, "AULowpass"), card_named(a, "AUDelay")) {
                    let (x0, y0) = card_xy(&dl, 140.0, 13.0);
                    let (x1, _) = card_xy(&lp, 30.0, 13.0);
                    drag(a, x0, y0, x1, y0);
                }
            }),
        ),
        Check(
            "devices: reordered to AUDelay, AULowpass in one undo step (MoveDevice) [needs engine plugin scanning]",
            Box::new(move |c, _| {
                one(&h5, c, "Move Device")?;
                let d = track_devices(c, TrackKind::Audio);
                (d.iter().map(|x| x.1.as_str()).collect::<Vec<_>>() == ["AUDelay", "AULowpass"])
                    .then_some(())
                    .ok_or(format!("devices {d:?}"))
            }),
        ),
        DoC(
            "devices: ✕ on the AULowpass card",
            Box::new(move |a, ctl| {
                rm1.set(track_devices(&ctl.borrow(), TrackKind::Audio).iter().find(|x| x.1 == "AULowpass").map(|x| x.0).unwrap_or(0));
                if let Some(cd) = card_named(a, "AULowpass") {
                    let (x, y) = card_xy(&cd, crate::devices::CARD_W - 14.5, 13.5);
                    click(a, x, y);
                }
            }),
        ),
        Check(
            "devices: AULowpass removed (RemoveDevice), one undo step [needs engine plugin scanning]",
            Box::new(move |c, a| {
                one(&h6, c, "Remove Device")?;
                let d = track_devices(c, TrackKind::Audio);
                (d.iter().map(|x| x.1.as_str()).collect::<Vec<_>>() == ["AUDelay"] && card_named(a, "AULowpass").is_none())
                    .then_some(())
                    .ok_or(format!("devices {d:?}"))
            }),
        ),
        Check(
            "automation: a removed plugin's lane keeps its colour (\"AULowpass/Cutoff\", not \"/Cutoff\") [needs engine plugin scanning]",
            Box::new(move |c, _| {
                let now = c.lane_hue(&lowpass_cutoff(rm3.get()));
                let want = crate::automation::Hue::key("AULowpass", "Cutoff");
                (rm3.get() != 0 && now == want).then_some(()).ok_or(format!("device {}: {now:?}, want {want:?}", rm3.get()))
            }),
        ),
        Do("devices: Cmd+Z", Box::new(|a| key(a, "z", true, false))),
        Check(
            "devices: ⌘Z restored AULowpass with the same device id, after AUDelay [needs engine plugin scanning]",
            Box::new(move |c, _| {
                h7.set(c.doc.history_labels().len());
                let d = track_devices(c, TrackKind::Audio);
                (d.iter().map(|x| (x.0, x.1.as_str())).collect::<Vec<_>>().get(1) == Some(&(rm2.get(), "AULowpass")) && rm2.get() != 0)
                    .then_some(())
                    .ok_or(format!("devices {d:?}, removed id {}", rm2.get()))
            }),
        ),
        // Opens a real plugin window for an instant (it can take focus); runs only when the
        // engine reports the plugin has an editor.
        DoC(
            "devices: open and close the AULowpass plugin window (only if the engine reports an editor)",
            Box::new(move |a, ctl| {
                let has_editor = {
                    let c = ctl.borrow();
                    c.devices.catalog.plugins.iter().any(|p| p.plugin.name == "AULowpass" && p.has_editor)
                        && c.devices.catalog.fixture.is_none()
                };
                if !has_editor {
                    *ed1.borrow_mut() = None;
                    return;
                }
                let Some(cd) = card_named(a, "AULowpass") else { return };
                let id = dstudio_core::DeviceId(cd.id as u64);
                let (x, y) = card_xy(&cd, crate::devices::CARD_W - 53.0 * crate::device_panel::Z, 16.0 * crate::device_panel::Z);
                click(a, x, y);
                let (opened, err) = {
                    let c = ctl.borrow();
                    (c.engine.plugin_editor_is_open(id), a.global::<Studio>().get_status_text().to_string())
                };
                // Settings › General › Keep plugin windows on top (on by default): the editor floats.
                let levels = crate::devices::plugin_window_levels();
                let err = if cfg!(target_os = "macos") && opened && !levels.contains(&3) {
                    format!("editor window levels {levels:?}, expected floating (3)")
                } else {
                    err
                };
                if let Ok(mut c) = ctl.try_borrow_mut() {
                    c.update_ui(a);
                }
                if let Some(cd) = card_named(a, "AULowpass") {
                    let (x, y) = card_xy(&cd, crate::devices::CARD_W - 53.0 * crate::device_panel::Z, 16.0 * crate::device_panel::Z);
                    click(a, x, y);
                }
                let closed = !ctl.borrow().engine.plugin_editor_is_open(id);
                *ed1.borrow_mut() = Some((opened, closed, err));
            }),
        ),
        Check(
            "devices: plugin window opened, then closed (plugin_editor_is_open toggled) [needs engine plugin editors]",
            Box::new(move |_, _| match &*ed2.borrow() {
                None => Err("SKIP: the engine reports no editor for AULowpass (no scan, fixture catalogue or has_editor false)".into()),
                Some((true, true, e)) if !e.starts_with("editor window levels") => Ok(()),
                Some((o, cl, e)) => Err(format!("opened {o} closed {cl}; status {e:?}")),
            }),
        ),
        DoC(
            "devices: select the Instrument track; open the Inst selector",
            Box::new(move |a, ctl| {
                let i = track_of(&ctl.borrow(), TrackKind::Instrument).unwrap_or(1);
                click_track_header(a, ctl, i);
                if let Ok(mut c) = ctl.try_borrow_mut() {
                    c.update_ui(a);
                }
                click(a, insp_in().0, insp_in().1);
            }),
        ),
        Check(
            "devices: Inst list shows Synth ✓ and DLSMusicDevice; record history [needs engine plugin scanning]",
            Box::new(move |c, a| {
                h8.set(c.doc.history_labels().len());
                let items = devmenu_labels(a);
                (items.first().is_some_and(|r| r.0 == "Synth" && r.1 == "✓") && items.iter().any(|r| r.0 == "DLSMusicDevice" && r.2))
                    .then_some(())
                    .ok_or(format!("items {:?}; scan {}", items.iter().take(5).collect::<Vec<_>>(), scan_state(c)))
            }),
        ),
        Do(
            "devices: pick DLSMusicDevice",
            Box::new(|a| {
                if let Some(i) = devmenu_labels(a).iter().position(|r| r.0 == "DLSMusicDevice") {
                    let (x, y) = devmenu_row(a, i);
                    click(a, x, y);
                } else {
                    key(a, Key::Escape, false, false);
                }
            }),
        ),
        Check(
            "devices: DLSMusicDevice is the instrument (SetInstrumentPlugin), one undo step; its card replaces the Synth card [needs engine plugin scanning]",
            Box::new(move |c, a| {
                one(&h9, c, "Load DLSMusicDevice")?;
                let i = track_of(c, TrackKind::Instrument).ok_or("no instrument track")?;
                let ip = c.project().tracks[i].instrument_plugin.as_ref().map(|d| d.plugin.name.clone());
                let s = a.global::<Studio>();
                (ip.as_deref() == Some("DLSMusicDevice")
                    && !s.get_synth_visible()
                    && card_named(a, "DLSMusicDevice").is_some_and(|cd| cd.instrument)
                    && s.get_sel_inst_text() == "DLSMusicDevice")
                    .then_some(())
                    .ok_or(format!("instrument {ip:?}, synth visible {}, inst text {}", s.get_synth_visible(), s.get_sel_inst_text()))
            }),
        ),
        Do(
            "devices: Inst selector → Synth",
            Box::new(|a| {
                click(a, insp_in().0, insp_in().1);
                let (x, y) = devmenu_row(a, 0);
                click(a, x, y);
            }),
        ),
        Check(
            "devices: back to the built-in Synth (a new Synth device), one undo step, Synth card shown [needs engine plugin scanning]",
            Box::new(move |c, a| {
                one(&h10, c, "Load Synth")?;
                let i = track_of(c, TrackKind::Instrument).ok_or("no instrument track")?;
                (c.project().tracks[i].instrument_plugin.as_ref().is_some_and(crate::devices::is_builtin_synth)
                    && a.global::<Studio>().get_synth_visible())
                .then_some(())
                .ok_or("still a plugin instrument".into())
            }),
        ),
        DoC(
            "devices: search “AUDelay”, drag it from the browser onto the Instrument track lane",
            Box::new(|a, ctl| {
                retype(a, browser_search(), "AUDelay");
                key(a, Key::Escape, false, false);
                if let Ok(mut c) = ctl.try_borrow_mut() {
                    c.update_ui(a);
                }
                let Some(r) = tree_row_of(a, "AUDelay") else { return };
                let (x0, y0) = tree_row_xy(a, r);
                let (ti, sy) = {
                    let c = ctl.borrow();
                    (track_of(&c, TrackKind::Instrument).unwrap_or(1), c.view.scroll_y as f32)
                };
                drag(a, x0, y0, lane_x() + 300.0, track0_y() + ti as f32 * PITCH + 37.0 - sy);
            }),
        ),
        Check(
            "devices: AUDelay dropped on the Instrument track (AddDevice), one undo step [needs engine plugin scanning]",
            Box::new(move |c, a| {
                one(&h11, c, "Add AUDelay")?;
                let d = track_devices(c, TrackKind::Instrument);
                (d.iter().map(|x| x.1.as_str()).collect::<Vec<_>>() == ["AUDelay"] && !a.global::<Studio>().get_dnd_active())
                    .then_some(())
                    .ok_or(format!("instrument devices {d:?}"))
            }),
        ),
        DoC(
            "devices: search “AULowpass”, drag it into the device chain before AUDelay",
            Box::new(|a, ctl| {
                retype(a, browser_search(), "AULowpass");
                key(a, Key::Escape, false, false);
                if let Ok(mut c) = ctl.try_borrow_mut() {
                    c.update_ui(a);
                }
                let (Some(r), Some(dl)) = (tree_row_of(a, "AULowpass"), card_named(a, "AUDelay")) else { return };
                let (x0, y0) = tree_row_xy(a, r);
                let (x1, y1) = card_xy(&dl, 30.0, 120.0);
                drag(a, x0, y0, x1, y1);
            }),
        ),
        Check(
            "devices: AULowpass dropped into the chain at index 0 (AddDevice), one undo step [needs engine plugin scanning]",
            Box::new(move |c, _| {
                one(&h12, c, "Add AULowpass")?;
                let d = track_devices(c, TrackKind::Instrument);
                (d.iter().map(|x| x.1.as_str()).collect::<Vec<_>>() == ["AULowpass", "AUDelay"])
                    .then_some(())
                    .ok_or(format!("instrument devices {d:?}"))
            }),
        ),
        // ⌘, is the app menu's Settings… shortcut, which replays the key on the next tick.
        DoC(
            "devices: ⌘, (Settings…); record scans",
            Box::new(move |a, ctl| {
                sc1.set(ctl.borrow().devices.catalog.scans);
                key(a, ",", true, false);
            }),
        ),
        Do(
            "settings: click Plugins",
            Box::new(|a| {
                let (x, y) = settings_row(crate::settings::PLUGINS_PAGE);
                click(a, x, y);
            }),
        ),
        Check(
            "settings: Plugins page shows scan directories, scan status and every catalogue entry",
            Box::new(|c, a| {
                let s = a.global::<Studio>();
                let rows = slint::Model::row_count(&s.get_plugin_rows());
                let dirs = slint::Model::row_count(&s.get_scan_dirs());
                (s.get_settings_open()
                    && s.get_settings_page() == crate::settings::PLUGINS_PAGE as i32
                    && dirs >= 3
                    && rows == c.devices.catalog.plugins.len()
                    && !s.get_plugins_status().is_empty())
                .then_some(())
                .ok_or(format!("page {} dirs {dirs} rows {rows} status {:?}", s.get_settings_page(), s.get_plugins_status()))
            }),
        ),
        Do(
            "settings: click “Rescan all plugins”",
            Box::new(|a| {
                let dirs = slint::Model::row_count(&a.global::<Studio>().get_scan_dirs()).min(8) as f32; // list is at most 8 rows tall
                let content_w = set_w() - 187.0;
                let card_x = 20.5 + ((content_w - 41.0) / 2.0 + 4.5);
                let card_y = 91.0 + dirs * 30.5 + 86.0;
                click(a, set_x() + 187.0 + card_x + 10.5 + 30.0, set_y() + card_y + 31.0 + 13.0);
            }),
        ),
        Check(
            "settings: a forced rescan ran (scan_plugins with rescan = true) and finished",
            Box::new(move |c, _| {
                let st = &c.devices.catalog.state;
                (c.devices.catalog.scans == sc2.get() + 1 && c.devices.catalog.last_rescan && *st != crate::devices::ScanState::Scanning)
                    .then_some(())
                    .ok_or(format!(
                        "scans {} -> {}, rescan {}, state {st:?}",
                        sc2.get(),
                        c.devices.catalog.scans,
                        c.devices.catalog.last_rescan
                    ))
            }),
        ),
        Do("settings: Escape", Box::new(|a| key(a, Key::Escape, false, false))),
        DoC(
            "devices: ⌘S into a scratch project folder, then reopen it",
            Box::new(move |a, ctl| {
                {
                    let mut c = ctl.borrow_mut();
                    let mut all = track_devices(&c, TrackKind::Audio);
                    all.extend(track_devices(&c, TrackKind::Instrument));
                    *sv1.borrow_mut() = all;
                    c.project_dir = Some(sd1.clone());
                }
                key(a, "s", true, false);
                ctl.borrow_mut().open(&sd1);
            }),
        ),
        Check(
            "devices: reopened project has the same devices (ids, plugins, order, bypass) [needs engine plugin scanning]",
            Box::new(move |c, _| {
                let mut all = track_devices(c, TrackKind::Audio);
                all.extend(track_devices(c, TrackKind::Instrument));
                let want = sv2.borrow().clone();
                (!want.is_empty() && all == want && c.project_dir.as_deref() == Some(sd2.as_path()))
                    .then_some(())
                    .ok_or(format!("reopened {all:?}, saved {want:?}"))
            }),
        ),
        Check(
            "engine: the saved project carries each device's plugin state (plugin_states before save) [needs engine plugin hosting]",
            Box::new(move |_, _| {
                let l = dstudio_core::persist::load(&sd3).map_err(|e| e.to_string())?;
                let devs: Vec<(String, bool)> = l
                    .project
                    .tracks
                    .iter()
                    .flat_map(|t| t.devices.iter())
                    .map(|d| (d.plugin.name.clone(), d.state.as_ref().is_some_and(|s| !s.is_empty())))
                    .collect();
                let _ = std::fs::remove_dir_all(&sd3);
                (!devs.is_empty() && devs.iter().all(|d| d.1)).then_some(()).ok_or(format!("devices with state: {devs:?}"))
            }),
        ),
    ]
}

// ------------------------------------------------------------------ automation

/// Lanes surface origin in the window (timeline x 262 + lane-x 191, y 78 + 27.5).
fn lanes_origin() -> (f32, f32) {
    (lane_x(), lane_y0())
}

fn auto_row(c: &Controller, track: usize, target: &dstudio_core::AutomationTarget) -> Option<crate::automation::Row> {
    let key = Some(c.project().tracks.get(track)?.id);
    c.auto_rows().into_iter().find(|r| r.key == key && r.target == *target)
}

/// Window point on an automation row at `beat` and lane value `v`.
fn auto_xy(c: &Controller, track: usize, target: &dstudio_core::AutomationTarget, beat: f64, v: f32) -> (f32, f32) {
    let Some(r) = auto_row(c, track, target) else { return (0.0, 0.0) };
    (lanes_origin().0 + c.view.x_of(beat), lanes_origin().1 + r.y_of(crate::automation::norm_of(target, v)))
}

fn lane_points(c: &Controller, track: usize, target: &dstudio_core::AutomationTarget) -> Vec<(i64, f32, f32)> {
    let Some(t) = c.project().tracks.get(track) else { return Vec::new() };
    c.model_lane(Some(t.id), target).map(|l| l.points.iter().map(|p| (p.time, p.value, p.curve)).collect()).unwrap_or_default()
}

/// Track header automation pill (track index `i`): window centre of the label and of the power icon.
fn pill_xy(c: &Controller, i: usize) -> ((f32, f32), (f32, f32)) {
    let y = track0_y() + c.view.track_offset(i) - c.view.scroll_y as f32 + 49.0 + 8.25;
    ((tracks_x() + 42.0 + 70.0, y), (tracks_x() + 42.0 + 8.5, y))
}

fn with_shift(a: &AppWindow, f: impl FnOnce()) {
    dispatch(a.window(), WindowEvent::KeyPressed { text: Key::Shift.into() });
    f();
    dispatch(a.window(), WindowEvent::KeyReleased { text: Key::Shift.into() });
}

/// Advance the transport by `secs`: offline render (no device), else wall-clock time.
fn advance(ctl: &Ctl, secs: f32) {
    let c = ctl.borrow();
    if c.engine.device_running() {
        drop(c);
        std::thread::sleep(std::time::Duration::from_secs_f32(secs));
    } else {
        let mut buf = vec![0f32; 2 * (48_000.0 * secs) as usize];
        let _ = c.engine.render_offline(&mut buf);
    }
}

thread_local! {
    /// Undo-history length recorded by the automation checks.
    static AUTO_HIST: Cell<usize> = const { Cell::new(0) };
}
fn ah_get() -> usize {
    AUTO_HIST.with(|h| h.get())
}
fn ah_set(n: usize) {
    AUTO_HIST.with(|h| h.set(n))
}

fn automation_steps() -> Vec<Step> {
    use Step::*;
    use dstudio_core::AutomationTarget as AT;
    fn one(c: &Controller, label: &str) -> Result<(), String> {
        let n = c.doc.history_labels().len();
        let ok = n == ah_get() + 1 && c.doc.undo_label() == Some(label);
        let was = ah_get();
        ah_set(n);
        ok.then_some(()).ok_or(format!("history {was} -> {n}, last {:?} (want one {label:?})", c.doc.undo_label()))
    }
    let wav = run_dir().join("auto.wav");
    let wav2 = wav.clone();
    vec![
        DoC(
            "automation: fresh project, steady 440 Hz clip on Audio track 1 at 0",
            Box::new(move |a, ctl| {
                key(a, Key::Escape, false, false);
                let n = 3 * 48_000;
                let sine: Vec<f32> = (0..n).map(|i| 0.5 * (std::f32::consts::TAU * 440.0 * i as f32 / 48_000.0).sin()).collect();
                let _ = crate::wavgen::write_wav(&wav, 48_000, 1, &sine);
                let mut c = ctl.borrow_mut();
                let _ = c.engine.stop();
                c.mixer_open = false;
                c.assistant_open = false;
                c.set_bottom_tab(crate::controller::BottomTab::Devices);
                c.open_add_track_menu(0.0, 0.0); // a new project must close it
                let mut p = dstudio_core::Project::default();
                if c.engine.device_running() {
                    p.master_gain = db_to_gain(-40.0); // the write pass plays through the device
                }
                c.replace_project(p);
                c.view.start_beats = 0.0;
                c.view.scroll_y = 0.0;
                let t = c.project().tracks[0].id;
                c.select_track(t);
                c.import_files(vec![wav.clone()], crate::controller::ImportTarget::Selected, Some(0));
            }),
        ),
        Check(
            "automation: clip imported, lanes hidden, timeline at full width, the add-track menu closed with the old project",
            Box::new(|c, a| {
                let ok = c.routing.menu.is_none()
                    && c.project().tracks[0].clips.len() == 1
                    && !c.auto.shown
                    && !a.global::<Studio>().get_auto_shown()
                    && (a.get_lanes_width() - (win().0 - geo().timeline_x - 5.0 - 191.0)).abs() < 1.0;
                ok.then_some(()).ok_or(format!("clips {} shown {}", c.project().tracks[0].clips.len(), c.auto.shown))
            }),
        ),
        Do(
            "automation: click “Show automation lanes” in the timeline header",
            Box::new(|a| click(a, geo().timeline_x + geo().timeline_w - 296.9, geo().timeline_y + 14.0)),
        ),
        Check(
            "automation: lanes shown; each header has the Track Volume pill and the disclosure; master row 64.5 pt; value labels (tracks +0.0, master at its static level)",
            Box::new(move |c, a| {
                ah_set(c.doc.history_labels().len());
                let s = a.global::<Studio>();
                let rows = s.get_tracks();
                let r0 = slint::Model::row_data(&rows, 0).ok_or("no rows")?;
                let labels: Vec<String> = slint::Model::iter(&s.get_auto_labels()).map(|l| l.text.to_string()).collect();
                let ok = c.auto.shown
                    && s.get_auto_shown()
                    && r0.auto_cat == "Track"
                    && r0.auto_param == "Volume"
                    && r0.auto_count == 1
                    && (s.get_master_h() - (crate::timeline::MASTER_H_AUTO - 0.5)).abs() < 1e-3
                    && labels.len() == 3
                    && labels[..2] == ["+0.0", "+0.0"]
                    && labels[2] == crate::automation::volume_label(dstudio_core::gain_to_db(c.project().master_gain));
                ok.then_some(()).ok_or(format!(
                    "shown {} pill {:?} {:?} count {} master {} labels {labels:?}",
                    s.get_auto_shown(),
                    r0.auto_cat,
                    r0.auto_param,
                    r0.auto_count,
                    s.get_master_h()
                ))
            }),
        ),
        DoC(
            "automation: open the track 1 parameter picker, click “Track ›”, then “Pan”",
            Box::new(|a, ctl| {
                let (pill, _) = pill_xy(&ctl.borrow(), 0);
                click(a, pill.0, pill.1);
                let top = pill.1 + 8.25 + 1.5;
                let (px, sub_x) = picker_x(a);
                click(a, px + 50.0, top + 30.5 + 10.5);
                click(a, sub_x + 50.0, top + 30.5 - 4.0 + 4.0 + 21.0 + 10.25);
            }),
        ),
        Check(
            "automation: picker closed, track 1 shows Track › Pan (UI state only, no undo step)",
            Box::new(move |c, a| {
                let s = a.global::<Studio>();
                let r0 = slint::Model::row_data(&s.get_tracks(), 0).ok_or("no rows")?;
                let ok = c.auto_target(Some(c.project().tracks[0].id)) == AT::Pan
                    && !s.get_picker_open()
                    && r0.auto_param == "Pan"
                    && lane_points(c, 0, &AT::Pan).is_empty();
                ok.then_some(()).ok_or(format!(
                    "target {:?} picker {} pill {:?}",
                    c.auto_target(Some(c.project().tracks[0].id)),
                    s.get_picker_open(),
                    r0.auto_param
                ))
            }),
        ),
        Check(
            "automation: record history",
            Box::new(move |c, _| {
                ah_set(c.doc.history_labels().len());
                Ok(())
            }),
        ),
        DoC(
            "automation: click the Pan line at beat 4",
            Box::new(|a, ctl| {
                let (x, y) = auto_xy(&ctl.borrow(), 0, &AT::Pan, 4.0, 0.0);
                click(a, x, y);
            }),
        ),
        Check(
            "automation: first point (beat 4, C) creates the lane, one undo step",
            Box::new({
                move |c, _| {
                    one(c, "Edit Pan Automation")?;
                    let p = lane_points(c, 0, &AT::Pan);
                    (p == vec![(4 * PPQ, 0.0, 0.0)]).then_some(()).ok_or(format!("{p:?}"))
                }
            }),
        ),
        DoC(
            "automation: click the Pan line at beat 8",
            Box::new(|a, ctl| {
                let (x, y) = auto_xy(&ctl.borrow(), 0, &AT::Pan, 8.0, 0.0);
                click(a, x, y);
            }),
        ),
        Check(
            "automation: second point at beat 8, one undo step",
            Box::new({
                move |c, _| {
                    one(c, "Edit Pan Automation")?;
                    let p = lane_points(c, 0, &AT::Pan);
                    (p == vec![(4 * PPQ, 0.0, 0.0), (8 * PPQ, 0.0, 0.0)]).then_some(()).ok_or(format!("{p:?}"))
                }
            }),
        ),
        DoC(
            "automation: drag the beat-8 point to beat 10 and up 20 pt",
            Box::new(|a, ctl| {
                let (x, y) = auto_xy(&ctl.borrow(), 0, &AT::Pan, 8.0, 0.0);
                let x1 = lanes_origin().0 + ctl.borrow().view.x_of(10.0);
                drag(a, x, y, x1 + 3.0, y - 20.0);
            }),
        ),
        Check(
            "automation: point moved to beat 10 (snapped) with a value right of centre, one undo step",
            Box::new({
                move |c, _| {
                    one(c, "Edit Pan Automation")?;
                    let p = lane_points(c, 0, &AT::Pan);
                    (p.len() == 2 && p[1].0 == 10 * PPQ && p[1].1 > 0.4 && p[1].1 < 0.8).then_some(()).ok_or(format!("{p:?}"))
                }
            }),
        ),
        DoC(
            "automation: Shift-drag the segment (beat 7) up 20 pt",
            Box::new(|a, ctl| {
                let (x, y) = {
                    let c = ctl.borrow();
                    let lane = c.model_lane(Some(c.project().tracks[0].id), &AT::Pan).cloned().unwrap();
                    auto_xy(&c, 0, &AT::Pan, 7.0, lane.value_at(7 * PPQ).unwrap())
                };
                with_shift(a, || drag(a, x, y, x, y - 20.0));
            }),
        ),
        Check(
            "automation: segment curve changed (point 1 curve < 0: faster rise), one undo step",
            Box::new({
                move |c, _| {
                    one(c, "Edit Pan Automation")?;
                    let p = lane_points(c, 0, &AT::Pan);
                    (p.len() == 2 && p[0].2 < -0.2 && p[0].0 == 4 * PPQ).then_some(()).ok_or(format!("{p:?}"))
                }
            }),
        ),
        DoC(
            "automation: click the beat-4 point (select) and press Delete",
            Box::new(|a, ctl| {
                let (x, y) = auto_xy(&ctl.borrow(), 0, &AT::Pan, 4.0, 0.0);
                click(a, x, y);
                key(a, "\u{7f}", false, false);
            }),
        ),
        Check(
            "automation: point deleted (one step), track and clip untouched",
            Box::new({
                move |c, _| {
                    one(c, "Edit Pan Automation")?;
                    let p = lane_points(c, 0, &AT::Pan);
                    (p.len() == 1 && p[0].0 == 10 * PPQ && c.project().tracks.len() == 2 && c.project().tracks[0].clips.len() == 1)
                        .then_some(())
                        .ok_or(format!("{p:?}, {} tracks", c.project().tracks.len()))
                }
            }),
        ),
        Do("automation: Cmd+Z", Box::new(|a| key(a, "z", true, false))),
        Check(
            "automation: undo restored the deleted point",
            Box::new(move |c, _| {
                ah_set(c.doc.history_labels().len());
                let p = lane_points(c, 0, &AT::Pan);
                (p.len() == 2 && p[0].0 == 4 * PPQ && p[0].2 < -0.2).then_some(()).ok_or(format!("{p:?}"))
            }),
        ),
        DoC(
            "automation: click the pill's power icon",
            Box::new(|a, ctl| {
                let (_, power) = pill_xy(&ctl.borrow(), 0);
                click(a, power.0, power.1);
            }),
        ),
        Check(
            "automation: lane disabled (enabled = false), points kept, pill state 2, one undo step",
            Box::new(move |c, a| {
                let t = c.project().tracks[0].id;
                let l = c.model_lane(Some(t), &AT::Pan).ok_or("no lane")?;
                let st = slint::Model::row_data(&a.global::<Studio>().get_tracks(), 0).map(|r| r.auto_state).unwrap_or(-1);
                let snap = c.current_snapshot();
                let in_snap = snap.tracks.iter().find(|s| s.track_id == t.0).map(|s| s.automation.len()).unwrap_or(9);
                one(c, "Edit Pan Automation")?;
                (!l.enabled && l.points.len() == 2 && st == 2 && in_snap == 0).then_some(()).ok_or(format!(
                    "enabled {} points {} state {st} snapshot lanes {in_snap}",
                    l.enabled,
                    l.points.len()
                ))
            }),
        ),
        DoC(
            "automation: pick Track › Volume for track 1 via the picker",
            Box::new(|a, ctl| {
                let (pill, _) = pill_xy(&ctl.borrow(), 0);
                click(a, pill.0, pill.1);
                let top = pill.1 + 8.25 + 1.5;
                let (px, sub_x) = picker_x(a);
                click(a, px + 50.0, top + 30.5 + 10.5);
                click(a, sub_x + 50.0, top + 30.5 - 4.0 + 4.0 + 10.25);
            }),
        ),
        Check(
            "automation: track 1 shows Volume",
            Box::new(move |c, _| {
                ah_set(c.doc.history_labels().len());
                (c.auto_target(Some(c.project().tracks[0].id)) == AT::Volume).then_some(()).ok_or("not Volume".into())
            }),
        ),
        DoC(
            "automation: Volume points at beats 4 and 12 (0 dB), then drag the second to the bottom (−∞)",
            Box::new(|a, ctl| {
                let p4 = auto_xy(&ctl.borrow(), 0, &AT::Volume, 4.0, 0.0);
                click(a, p4.0, p4.1);
                let p12 = auto_xy(&ctl.borrow(), 0, &AT::Volume, 12.0, 0.0);
                click(a, p12.0, p12.1);
                let bottom = auto_xy(&ctl.borrow(), 0, &AT::Volume, 12.0, -60.0);
                drag(a, p12.0, p12.1, p12.0, bottom.1 + 6.0);
            }),
        ),
        Check(
            "automation: Volume value_at: 0 dB before beat 4, −30 dB at beat 8 (linear), −60 dB after beat 12; 3 steps",
            Box::new(move |c, _| {
                let t = c.project().tracks[0].id;
                let l = c.model_lane(Some(t), &AT::Volume).ok_or("no lane")?;
                let (a0, a8, a16) = (l.value_at(0).unwrap_or(9.0), l.value_at(8 * PPQ).unwrap_or(9.0), l.value_at(16 * PPQ).unwrap_or(9.0));
                let steps = c.doc.history_labels().len() - ah_get();
                ah_set(c.doc.history_labels().len());
                (a0 == 0.0 && (a8 + 30.0).abs() < 0.1 && a16 == -60.0 && steps == 3 && l.points.len() == 2)
                    .then_some(())
                    .ok_or(format!("{a0} {a8} {a16}, {steps} steps, {:?}", l.points))
            }),
        ),
        Check(
            "automation: header slider and inspector fader follow the automated value at the playhead (display only, gold)",
            Box::new(|c, a| {
                let s = a.global::<Studio>();
                let r0 = slint::Model::row_data(&s.get_tracks(), 0).ok_or("no rows")?;
                let at = c.project().frames_to_ticks(c.audible_frames(), c.rate);
                let want = c.model_lane(Some(c.project().tracks[0].id), &AT::Volume).and_then(|l| l.value_at(at)).ok_or("no value")?;
                let g = if want <= -60.0 { 0.0 } else { db_to_gain(want) };
                let pos = crate::controller::slider_from_gain(g);
                (r0.vol_auto
                    && s.get_sel_vol_auto()
                    && (r0.volume_pos - pos).abs() < 1e-3
                    && (c.project().tracks[0].gain - 1.0).abs() < 1e-6)
                    .then_some(())
                    .ok_or(format!("auto {} pos {} want {pos}", r0.vol_auto, r0.volume_pos))
            }),
        ),
        DoC(
            "automation: move the volume slider while not armed (automation is active)",
            Box::new(move |a, ctl| {
                ah_set(ctl.borrow().doc.history_labels().len());
                let y = track0_y() + 44.0 - 13.0 + 4.0;
                drag(a, tracks_x() + 115.6 + 10.0, y, tracks_x() + 115.6 + 30.0, y);
            }),
        ),
        Check(
            "automation: no edit, status hint “Volume is automated: arm automation to write, or disable the lane”",
            Box::new(move |c, a| {
                let st = a.global::<Studio>().get_status_text().to_string();
                let n = c.doc.history_labels().len();
                (n == ah_get()
                    && (c.project().tracks[0].gain - 1.0).abs() < 1e-6
                    && st.contains("Volume is automated: arm automation to write, or disable the lane"))
                .then_some(())
                .ok_or(format!("gain {} status {st:?}", c.project().tracks[0].gain))
            }),
        ),
        DoC(
            "automation: double-click the beat-4 Volume point",
            Box::new(move |a, ctl| {
                ah_set(ctl.borrow().doc.history_labels().len());
                let p = auto_xy(&ctl.borrow(), 0, &AT::Volume, 4.0, 0.0);
                click(a, p.0, p.1);
                click(a, p.0, p.1);
            }),
        ),
        Check(
            "automation: double-click removed the point (one step); Cmd+Z next restores it",
            Box::new(move |c, _| {
                let p = lane_points(c, 0, &AT::Volume);
                let n = c.doc.history_labels().len();
                (p.len() == 1 && n == ah_get() + 1 && c.doc.undo_label() == Some("Edit Volume Automation"))
                    .then_some(())
                    .ok_or(format!("{p:?}"))
            }),
        ),
        Do("automation: Cmd+Z", Box::new(|a| key(a, "z", true, false))),
        Check(
            "automation: point back",
            Box::new(|c, _| {
                let p = lane_points(c, 0, &AT::Volume);
                (p.len() == 2).then_some(()).ok_or(format!("{p:?}"))
            }),
        ),
        DoC(
            "automation: Volume lane −60 → 0 dB over 1 s on track 1 (one SetAutomationLane)",
            Box::new(|_, ctl| {
                let mut c = ctl.borrow_mut();
                let t = c.project().tracks[0].id;
                let one_s = c.project().seconds_to_ticks(1.0);
                let lane = dstudio_core::AutomationLane {
                    target: AT::Volume,
                    enabled: true,
                    points: vec![
                        dstudio_core::AutoPoint { time: 0, value: -60.0, curve: 0.0 },
                        dstudio_core::AutoPoint { time: one_s, value: 0.0, curve: 0.0 },
                    ],
                };
                c.auto_commit(Some(t), lane);
            }),
        ),
        Check(
            "automation: offline render of track 1 rises from silence to the clip level over 1 s [needs engine automation playback]",
            Box::new(|c, _| {
                if c.engine.device_running() {
                    return Err("SKIP: offline render needs --no-audio".into());
                }
                let e = &c.engine;
                let t1 = c.project().tracks[0].id;
                let mut snap = c.current_snapshot();
                for t in &mut snap.tracks {
                    t.mute = t.track_id != t1.0;
                }
                e.publish(&snap).map_err(|e| e.to_string())?;
                e.seek(0).map_err(|e| e.to_string())?;
                e.play().map_err(|e| e.to_string())?;
                let mut buf = vec![0f32; 2 * 48_000];
                let r = e.render_offline(&mut buf);
                let _ = e.stop();
                let _ = e.seek(0);
                e.publish(&c.current_snapshot()).map_err(|e| e.to_string())?;
                r.map_err(|e| e.to_string())?;
                let rms = |a: usize, b: usize| {
                    ((buf[2 * a..2 * b].iter().map(|x| (*x as f64).powi(2)).sum::<f64>()) / (2 * (b - a)) as f64).sqrt() as f32
                };
                let (early, mid, late) = (rms(0, 4_800), rms(21_600, 26_400), rms(43_200, 48_000));
                (late > 0.05 && early < late * 0.05 && mid > early && late > mid)
                .then_some(())
                .ok_or(format!("rms 0–0.1 s {early:.4}, 0.45–0.55 s {mid:.4}, 0.9–1.0 s {late:.4}: the level does not follow the lane; the engine does not apply automation yet (depends on the engine agent's automation playback)"))
            }),
        ),
        Do("automation: click Automation Arm in the transport", Box::new(|a| click(a, transport_x() + 496.0 + 10.5, 41.0 + 11.0))),
        Check(
            "automation: armed (transport button on)",
            Box::new(move |c, a| {
                ah_set(c.doc.history_labels().len());
                (c.auto.armed && a.global::<Studio>().get_auto_armed()).then_some(()).ok_or("not armed".into())
            }),
        ),
        DoC(
            "automation: play from 2 s; while playing, drag the track 1 volume slider (press, 4 moves 0.15 s apart, release)",
            Box::new(|a, ctl| {
                {
                    let c = ctl.borrow();
                    let f = c.project().ticks_to_frames(c.project().seconds_to_ticks(2.0), c.rate);
                    let _ = c.engine.seek(f);
                    let _ = c.engine.play();
                }
                advance(ctl, 0.1);
                let y = track0_y() + 44.0 - 13.0 + 4.0;
                let x0 = tracks_x() + 115.6 + 3.5;
                press(a, x0 + 50.0, y);
                for k in 1..=4 {
                    advance(ctl, 0.15);
                    dispatch(a.window(), WindowEvent::PointerMoved { position: LogicalPosition::new(x0 + 50.0 - 9.0 * k as f32, y) });
                }
                advance(ctl, 0.1);
                release(a, x0 + 14.0, y);
                let _ = ctl.borrow().engine.stop();
            }),
        ),
        Check(
            "automation: ONE “Write Automation” step; the Volume lane got the written points in the touched range, the curve outside kept",
            Box::new(move |c, _| {
                one(c, "Write Automation")?;
                let p = lane_points(c, 0, &AT::Volume);
                let tps = c.project().seconds_to_ticks(1.0);
                let inside: Vec<_> = p.iter().filter(|q| q.0 >= 2 * tps && q.0 <= 3 * tps).collect();
                let falling = inside.windows(2).any(|w| w[1].1 < w[0].1);
                (p.first().map(|q| (q.0, q.1)) == Some((0, -60.0))
                    && inside.len() >= 2
                    && falling
                    && c.auto.writes.is_empty()
                    && c.auto.pending.is_none())
                .then_some(())
                .ok_or(format!("points {p:?}"))
            }),
        ),
        Do(
            "automation: click Automation Arm again (off), hide lanes",
            Box::new(|a| {
                click(a, transport_x() + 496.0 + 10.5, 41.0 + 11.0);
                click(a, geo().timeline_x + geo().timeline_w - 296.9, geo().timeline_y + 14.0);
            }),
        ),
        Check(
            "automation: disarmed, lanes hidden, master row back to 30 pt",
            Box::new(move |c, a| {
                let _ = &wav2;
                let s = a.global::<Studio>();
                (!c.auto.armed && !c.auto.shown && (s.get_master_h() - (crate::timeline::MASTER_H - 0.5)).abs() < 1e-3)
                    .then_some(())
                    .ok_or(format!("armed {} shown {} master {}", c.auto.armed, c.auto.shown, s.get_master_h()))
            }),
        ),
    ]
}

// ------------------------------------------------------------------ built-in devices

fn builtin_device(c: &Controller) -> Option<&dstudio_core::Device> {
    let t = track_of(c, TrackKind::Audio)?;
    c.project().tracks[t].devices.iter().find(|d| d.plugin.format == dstudio_core::PluginFormat::Builtin)
}

fn builtin_value(c: &Controller, param: &str) -> Option<f32> {
    let d = builtin_device(c)?;
    let def = dstudio_core::builtin::def_of(d)?;
    Some(def.value(d, def.param(param)?))
}

/// Window centre of knob `i` of a built-in card (ui/device_panel.slint KnobCell).
fn panel_knob_xy(cd: &crate::DeviceCard, i: usize) -> Option<(f32, f32)> {
    let k = slint::Model::row_data(&cd.knobs, i)?;
    let z = crate::device_panel::Z;
    Some(card_xy(cd, k.x + 31.0 * z, k.y + 16.0 * z))
}

fn builtin_of<'a>(c: &'a Controller, id: &str) -> Option<&'a dstudio_core::Device> {
    let t = track_of(c, TrackKind::Audio)?;
    c.project().tracks[t].devices.iter().find(|d| dstudio_core::builtin::def_of(d).is_some_and(|def| def.id == id))
}

/// Each other built-in: add it from the Built-in list, check its card, defaults and engine
/// state, drag its first knob (one undo step), then undo back to the empty chain.
fn builtin_each_steps() -> Vec<Step> {
    use Step::*;
    let mut steps: Vec<Step> = vec![Do("builtins: Escape (close menus)", Box::new(|a| key(a, Key::Escape, false, false)))];
    for def in dstudio_core::builtin::all()
        .iter()
        .filter(|d| ["distortion", "tapesaturation", "soft-clipper", "bitcrusher", "widener", "utility"].contains(&d.id))
    {
        let (id, name) = (def.id, def.name);
        let base = Rc::new(Cell::new(0usize));
        let before = Rc::new(Cell::new(0.0f32));
        let (b0, b1, b2, b3) = (base.clone(), base.clone(), base.clone(), base);
        let (v0, v1) = (before.clone(), before);
        steps.push(DoC(
            format!("{name}: Add device › Built-in › {name}").leak(),
            Box::new(move |a, ctl| {
                b0.set(ctl.borrow().doc.history_labels().len());
                if let Ok(mut c) = ctl.try_borrow_mut() {
                    c.update_ui(a);
                }
                let add_x = add_card_x(a, ctl);
                click(a, bottom_x() + add_x + 60.0, bottom_y() + 34.5 + 56.5 + 11.0);
                if let Ok(mut c) = ctl.try_borrow_mut() {
                    c.update_ui(a);
                }
                if let Some(i) = devmenu_labels(a).iter().position(|r| r.0 == name) {
                    let (x, y) = devmenu_row(a, i);
                    click(a, x, y);
                }
            }),
        ));
        steps.push(Check(
            format!("{name}: added at its defaults in one undo step; card drawn; engine live").leak(),
            Box::new(move |c, a| {
                let n = c.doc.history_labels().len();
                let label = format!("Add {name}");
                if n != b1.get() + 1 || c.doc.undo_label() != Some(label.as_str()) {
                    return Err(format!("history {} -> {n}, last {:?}", b1.get(), c.doc.undo_label()));
                }
                let d = builtin_of(c, id).ok_or("no device")?;
                let def = dstudio_core::builtin::def_of(d).ok_or("no def")?;
                if let Some(p) = def.params.iter().find(|p| def.value(d, p) != p.default) {
                    return Err(format!("{} = {} (default {})", p.id, def.value(d, p), p.default));
                }
                let cd = card_named(a, name).ok_or("no card")?;
                if !cd.builtin || slint::Model::row_count(&cd.knobs) == 0 {
                    return Err(format!("builtin {} knobs {}", cd.builtin, slint::Model::row_count(&cd.knobs)));
                }
                let errors = c.engine.plugin_load_errors();
                (!errors.iter().any(|(e, _)| *e == d.id)).then_some(()).ok_or(format!("load errors {errors:?}"))
            }),
        ));
        steps.push(DoC(
            format!("{name}: drag the first knob 40 px").leak(),
            Box::new(move |a, ctl| {
                let Some(cd) = card_named(a, name) else { return };
                let Some(k) = slint::Model::row_data(&cd.knobs, 0) else { return };
                {
                    let c = ctl.borrow();
                    let Some(d) = builtin_of(&c, id) else { return };
                    let def = dstudio_core::builtin::def_of(d).unwrap();
                    v0.set(def.value(d, &def.params[k.param as usize]));
                }
                if let Some((x, y)) = panel_knob_xy(&cd, 0) {
                    drag(a, x, y, x, if k.norm > 0.5 { y + 40.0 } else { y - 40.0 });
                }
            }),
        ));
        steps.push(Check(
            format!("{name}: the drag changed the first knob's parameter in ONE undo step").leak(),
            Box::new(move |c, a| {
                let n = c.doc.history_labels().len();
                if n != b2.get() + 2 || c.doc.undo_label() != Some("Device Parameter Change") {
                    return Err(format!("history {} -> {n}, last {:?}", b2.get() + 1, c.doc.undo_label()));
                }
                let cd = card_named(a, name).ok_or("no card")?;
                let k = slint::Model::row_data(&cd.knobs, 0).ok_or("no knob")?;
                let d = builtin_of(c, id).ok_or("no device")?;
                let def = dstudio_core::builtin::def_of(d).ok_or("no def")?;
                let v = def.value(d, &def.params[k.param as usize]);
                (v != v1.get()).then_some(()).ok_or(format!("value stayed {v}"))
            }),
        ));
        steps.push(Do(
            format!("{name}: ⌘Z twice").leak(),
            Box::new(|a| {
                key(a, "z", true, false);
                key(a, "z", true, false);
            }),
        ));
        steps.push(Check(
            format!("{name}: undo removes the device").leak(),
            Box::new(move |c, _| {
                let n = c.doc.history_labels().len();
                (builtin_of(c, id).is_none() && n == b3.get()).then_some(()).ok_or(format!("history {n}, want {}", b3.get()))
            }),
        ));
    }
    steps
}

/// The built-in Compressor driven through its generated panel (no plugins needed).
fn builtin_device_steps() -> Vec<Step> {
    use Step::*;
    let hist = Rc::new(Cell::new(0usize));
    let h = || hist.clone();
    fn one(hc: &Rc<Cell<usize>>, c: &Controller, label: &str) -> Result<(), String> {
        let n = c.doc.history_labels().len();
        let was = hc.get();
        hc.set(n);
        (n == was + 1 && c.doc.undo_label() == Some(label))
            .then_some(())
            .ok_or(format!("history {was} -> {n}, last {:?} (want {label})", c.doc.undo_label()))
    }
    let (h0, h1, h2, h3, h4, h5, h6, h7, h8) = (h(), h(), h(), h(), h(), h(), h(), h(), h());
    vec![
        Do("builtin: Escape (close menus)", Box::new(|a| key(a, Key::Escape, false, false))),
        DoC(
            "builtin: select the first audio track, Devices tab, Add device › Built-in",
            Box::new(move |a, ctl| {
                let i = track_of(&ctl.borrow(), TrackKind::Audio).unwrap_or(0);
                click_track_header(a, ctl, i);
                if ctl.borrow().bottom_tab == crate::controller::BottomTab::Clip {
                    click(a, bottom_x() + 286.0 + 15.0, bottom_y() + 14.0);
                }
                h0.set(ctl.borrow().doc.history_labels().len());
                if let Ok(mut c) = ctl.try_borrow_mut() {
                    c.update_ui(a);
                }
                let add_x = add_card_x(a, ctl);
                click(a, bottom_x() + add_x + 60.0, bottom_y() + 34.5 + 56.5 + 11.0);
            }),
        ),
        Check(
            "builtin: the Built-in list offers Compressor",
            Box::new(|_, a| {
                let items = devmenu_labels(a);
                (a.global::<Studio>().get_devmenu_open() && items.iter().any(|r| r.0 == "Compressor" && r.2))
                    .then_some(())
                    .ok_or(format!("items {items:?}"))
            }),
        ),
        Do(
            "builtin: pick Compressor",
            Box::new(|a| {
                if let Some(i) = devmenu_labels(a).iter().position(|r| r.0 == "Compressor") {
                    let (x, y) = devmenu_row(a, i);
                    click(a, x, y);
                }
            }),
        ),
        Check(
            "builtin: Compressor added (AddDevice) at its defaults, one undo step; generated card has 7 knobs, 1 slider, a strip",
            Box::new(move |c, a| {
                one(&h1, c, "Add Compressor")?;
                let cd = card_named(a, "Compressor").ok_or("no card")?;
                let n = |m: usize| m;
                let ok = cd.builtin
                    && n(slint::Model::row_count(&cd.knobs)) == 7
                    && slint::Model::row_count(&cd.sliders) == 1
                    && cd.strip.visible
                    && approx(cd.width, 521.0 * crate::device_panel::Z, 0.01)
                    && builtin_value(c, "threshold") == Some(-24.0);
                ok.then_some(()).ok_or(format!("builtin {} knobs {} width {}", cd.builtin, slint::Model::row_count(&cd.knobs), cd.width))
            }),
        ),
        Check(
            "engine: the Compressor is live (readouts available, no load error)",
            Box::new(|c, _| {
                let id = builtin_device(c).ok_or("no device")?.id;
                let mut r = Vec::new();
                c.engine.builtin_readouts(id.0, &mut r).map_err(|e| e.to_string())?;
                let errors = c.engine.plugin_load_errors();
                (r.len() == 3 && !errors.iter().any(|(d, _)| *d == id)).then_some(()).ok_or(format!("readouts {r:?} errors {errors:?}"))
            }),
        ),
        DoC(
            "builtin: drag the Threshold knob up 40 px",
            Box::new(|a, _| {
                if let Some((x, y)) = card_named(a, "Compressor").and_then(|cd| panel_knob_xy(&cd, 0)) {
                    drag(a, x, y, x, y - 40.0);
                }
            }),
        ),
        Check(
            "builtin: Threshold −24 → −9 dB (40 px at 1/160 per px), ONE undo step for the drag",
            Box::new(move |c, a| {
                one(&h2, c, "Device Parameter Change")?;
                let v = builtin_value(c, "threshold").unwrap_or(f32::NAN);
                let shown = card_named(a, "Compressor")
                    .and_then(|cd| slint::Model::row_data(&cd.knobs, 0))
                    .map(|k| k.value.to_string())
                    .unwrap_or_default();
                (approx(v, -9.0, 0.05) && shown == "\u{2212}9.0 dB").then_some(()).ok_or(format!("threshold {v}, knob shows {shown:?}"))
            }),
        ),
        DoC(
            "builtin: double-click the Threshold knob",
            Box::new(|a, _| {
                if let Some((x, y)) = card_named(a, "Compressor").and_then(|cd| panel_knob_xy(&cd, 0)) {
                    double_click(a, x, y);
                }
            }),
        ),
        Check(
            "builtin: double-click reset Threshold to −24 dB, one undo step",
            Box::new(move |c, _| {
                one(&h3, c, "Device Parameter Change")?;
                let v = builtin_value(c, "threshold").unwrap_or(f32::NAN);
                (v == -24.0).then_some(()).ok_or(format!("threshold {v}"))
            }),
        ),
        DoC(
            "builtin: click › (next preset)",
            Box::new(|a, _| {
                if let Some(cd) = card_named(a, "Compressor") {
                    let z = crate::device_panel::Z;
                    // Header: next-preset arrow centre (fold, window, rule and nav from the right edge).
                    let (x, y) = card_xy(&cd, cd.width - 71.25 * z, 16.0 * z);
                    click(a, x, y);
                }
            }),
        ),
        Check(
            "builtin: first factory preset loaded (SetDeviceParams), one undo step, header names it",
            Box::new(move |c, a| {
                let def = dstudio_core::builtin::find("compressor").unwrap();
                let first = def.factory[0];
                one(&h4, c, &format!("Load Preset {}", first.name))?;
                let want = def.factory_values(&first);
                let d = builtin_device(c).ok_or("no device")?;
                let shown = card_named(a, "Compressor").map(|cd| cd.preset.to_string()).unwrap_or_default();
                (def.params.iter().all(|p| def.value(d, p) == want[p.id]) && shown == first.name)
                    .then_some(())
                    .ok_or(format!("preset {:?}, header {shown:?}", d.preset))
            }),
        ),
        DoC(
            "builtin: Sidechain menu, pick the Instrument track",
            Box::new(|a, ctl| {
                let (id, name) = {
                    let c = ctl.borrow();
                    let Some(d) = builtin_device(&c) else { return };
                    let Some(t) = track_of(&c, TrackKind::Instrument) else { return };
                    (d.id.0 as i32, c.project().tracks[t].name.clone())
                };
                a.global::<Studio>().invoke_panel_menu(id, -1, 600.0, 600.0);
                if let Ok(mut c) = ctl.try_borrow_mut() {
                    c.update_ui(a);
                }
                if let Some(i) = devmenu_labels(a).iter().position(|r| r.0 == name) {
                    let (x, y) = devmenu_row(a, i);
                    click(a, x, y);
                }
            }),
        ),
        Check(
            "builtin: sidechain source set and enabled in one undo step; the Gain pill is live",
            Box::new(move |c, a| {
                one(&h5, c, "Sidechain Change")?;
                let d = builtin_device(c).ok_or("no device")?;
                let inst = track_of(c, TrackKind::Instrument).map(|t| c.project().tracks[t].id);
                let pill = card_named(a, "Compressor").and_then(|cd| {
                    (0..slint::Model::row_count(&cd.picks)).filter_map(|i| slint::Model::row_data(&cd.picks, i)).find(|p| p.kind == 3)
                });
                (d.sidechain == inst && builtin_value(c, "sidechainEnable") == Some(1.0) && pill.is_some_and(|p| p.enabled))
                    .then_some(())
                    .ok_or(format!("sidechain {:?} enable {:?}", d.sidechain, builtin_value(c, "sidechainEnable")))
            }),
        ),
        Do("builtin: ⌘Z", Box::new(|a| key(a, "z", true, false))),
        Check(
            "builtin: undo removes the sidechain (source and enable)",
            Box::new(move |c, _| {
                let d = builtin_device(c).ok_or("no device")?;
                h6.set(c.doc.history_labels().len());
                (d.sidechain.is_none() && builtin_value(c, "sidechainEnable") == Some(0.0))
                    .then_some(())
                    .ok_or(format!("sidechain {:?}", d.sidechain))
            }),
        ),
        DoC(
            "builtin: pick the Instrument track as sidechain, then load the second factory preset",
            Box::new(|_, ctl| {
                let mut c = ctl.borrow_mut();
                let (Some(d), Some(t)) = (builtin_device(&c).map(|d| d.id), track_of(&c, TrackKind::Instrument)) else { return };
                let src = c.project().tracks[t].id;
                c.panel_set_sidechain(d, Some(src));
                c.panel_load_preset(d, 1);
            }),
        ),
        Check(
            "builtin: a preset keeps the sidechain source and its enable switch, and is not marked edited",
            Box::new(|c, a| {
                let d = builtin_device(c).ok_or("no device")?;
                let shown = card_named(a, "Compressor").map(|cd| cd.preset.to_string()).unwrap_or_default();
                (d.sidechain.is_some() && builtin_value(c, "sidechainEnable") == Some(1.0) && !shown.is_empty() && !shown.ends_with('*'))
                    .then_some(())
                    .ok_or(format!("sidechain {:?} enable {:?} header {shown:?}", d.sidechain, builtin_value(c, "sidechainEnable")))
            }),
        ),
        DoC(
            "builtin: reset Threshold, then drag it up 40 px as forty 1 px events",
            Box::new(|_, ctl| {
                let mut c = ctl.borrow_mut();
                let Some(d) = builtin_device(&c).map(|d| d.id) else { return };
                c.panel_reset(d, 0);
                c.panel_drag(d, 0, 0, 0.0, 1.0 / 160.0, false);
                for _ in 0..40 {
                    c.panel_drag(d, 0, 1, 1.0, 1.0 / 160.0, false);
                }
                c.panel_drag(d, 0, 2, 0.0, 1.0 / 160.0, false);
            }),
        ),
        Check(
            "builtin: small drag events add up like one large event (−24 → −9 dB)",
            Box::new(|c, _| {
                let v = builtin_value(c, "threshold").unwrap_or(f32::NAN);
                approx(v, -9.0, 0.05).then_some(()).ok_or(format!("threshold {v}"))
            }),
        ),
        DoC(
            "builtin: drag Threshold 20 px, press Escape, release",
            Box::new(move |a, ctl| {
                h7.set(ctl.borrow().doc.history_labels().len());
                let Some(d) = builtin_device(&ctl.borrow()).map(|d| d.id) else { return };
                ctl.borrow_mut().panel_drag(d, 0, 0, 0.0, 1.0 / 160.0, false);
                ctl.borrow_mut().panel_drag(d, 0, 1, 20.0, 1.0 / 160.0, false);
                key(a, Key::Escape, false, false);
                ctl.borrow_mut().panel_drag(d, 0, 2, 0.0, 1.0 / 160.0, false);
            }),
        ),
        Check(
            "builtin: Escape cancels the drag (Threshold stays −9 dB, no undo step)",
            Box::new(move |c, _| {
                let v = builtin_value(c, "threshold").unwrap_or(f32::NAN);
                let n = c.doc.history_labels().len();
                (approx(v, -9.0, 0.05) && n == h8.get()).then_some(()).ok_or(format!("threshold {v}, history {} -> {n}", h8.get()))
            }),
        ),
    ]
}

fn analyzer_device<'a>(c: &'a Controller, id: &str) -> Option<&'a dstudio_core::Device> {
    let t = track_of(c, TrackKind::Audio)?;
    c.project().tracks[t].devices.iter().find(|d| d.plugin.format == dstudio_core::PluginFormat::Builtin && d.plugin.id == id)
}

/// Pitch Corrector, Tuner and Spectrum: added from the Built-in list's
/// definitions, generated cards with their analyzer fields, live engine
/// instances (readouts and analysis frames), latency reported for PDC, and
/// Key Follow (picking a Key while following the project turns Follow off
/// in one undo step).
fn analyzer_device_steps() -> Vec<Step> {
    use Step::*;
    let hist = Rc::new(Cell::new(0usize));
    let (h0, h1) = (hist.clone(), hist.clone());
    vec![
        DoC(
            "analyzers: select the audio track, clear its chain, add Pitch Corrector, Tuner and Spectrum",
            Box::new(move |a, ctl| {
                key(a, Key::Escape, false, false);
                let Some(i) = track_of(&ctl.borrow(), TrackKind::Audio) else { return };
                click_track_header(a, ctl, i);
                let mut c = ctl.borrow_mut();
                let t = c.project().tracks[i].id;
                let ids: Vec<dstudio_core::DeviceId> = c.project().tracks[i].devices.iter().map(|d| d.id).collect();
                for d in ids {
                    c.execute(dstudio_core::Command::RemoveDevice { device: d });
                }
                for id in ["pitch-corrector", "tuner", "spectrum"] {
                    if let Some(def) = dstudio_core::builtin::find(id) {
                        c.add_item_to_track(&crate::devices::Item::Builtin(def), t, None);
                    }
                }
                c.update_ui(a);
            }),
        ),
        Check(
            "analyzers: three cards with their canvases (Pitch trace, tuner, Spectrum) at the reference widths",
            Box::new(|_, a| {
                let z = crate::device_panel::Z;
                let want = [("Pitch Corrector", 375.0, "Pitch"), ("Tuner", 335.0, ""), ("Spectrum", 397.0, "Spectrum")];
                for (name, w, caption) in want {
                    let cd = card_named(a, name).ok_or(format!("no {name} card"))?;
                    if !(cd.builtin && cd.canvas.visible && cd.canvas.caption == caption && approx(cd.width, w * z, 0.01)) {
                        return Err(format!(
                            "{name}: builtin {} field {} caption {:?} width {}",
                            cd.builtin, cd.canvas.visible, cd.canvas.caption, cd.width
                        ));
                    }
                }
                Ok(())
            }),
        ),
        Check(
            "engine: the three are live (readouts, analysis frames, no load error); Pitch Corrector reports 1024 frames latency",
            Box::new(|c, _| {
                let errors = c.engine.plugin_load_errors();
                for id in ["pitch-corrector", "tuner", "spectrum"] {
                    let d = analyzer_device(c, id).ok_or(format!("no {id}"))?;
                    let (mut r, mut f) = (Vec::new(), Vec::new());
                    c.engine.builtin_readouts(d.id.0, &mut r).map_err(|e| format!("{id} readouts: {e}"))?;
                    c.engine.builtin_analysis(d.id.0, &mut f).map_err(|e| format!("{id} analysis: {e}"))?;
                    if errors.iter().any(|(e, _)| *e == d.id) {
                        return Err(format!("{id}: load error {errors:?}"));
                    }
                }
                let lat = analyzer_device(c, "pitch-corrector").map_or(0, |d| c.engine.plugin_latency(d.id));
                (lat == 1024).then_some(()).ok_or(format!("Pitch Corrector latency {lat} (want 1024 at 48 kHz)"))
            }),
        ),
        DoC(
            "analyzers: with no project key, pick Key E on the Pitch Corrector",
            Box::new(|a, ctl| {
                let mut c = ctl.borrow_mut();
                let Some(d) = analyzer_device(&c, "pitch-corrector").map(|d| d.id) else { return };
                let key = dstudio_core::builtin::find("pitch-corrector").and_then(|def| def.index_of("key")).unwrap_or(0);
                c.panel_choice(d, key as i32, 4);
                c.update_ui(a);
            }),
        ),
        Check(
            "analyzers: nothing is followed without a project key: Key E, Follow stays Project",
            Box::new(|c, _| {
                let d = analyzer_device(c, "pitch-corrector").ok_or("no device")?;
                let def = dstudio_core::builtin::def_of(d).ok_or("no def")?;
                let v = |id: &str| def.param(id).map(|p| def.value(d, p));
                (v("key") == Some(4.0) && v("key_follow") == Some(1.0)).then_some(()).ok_or(format!(
                    "key {:?} follow {:?}",
                    v("key"),
                    v("key_follow")
                ))
            }),
        ),
        DoC(
            "analyzers: set the project key to A minor, then pick Key D on the following Pitch Corrector",
            Box::new(move |a, ctl| {
                let mut c = ctl.borrow_mut();
                c.execute(dstudio_core::Command::SetProjectKey { key: Some(dstudio_core::ProjectKey { root: 9, minor: true }) });
                h0.set(c.doc.history_labels().len());
                let Some(d) = analyzer_device(&c, "pitch-corrector").map(|d| d.id) else { return };
                let key = dstudio_core::builtin::find("pitch-corrector").and_then(|def| def.index_of("key")).unwrap_or(0);
                c.panel_choice(d, key as i32, 2);
                c.update_ui(a);
            }),
        ),
        Check(
            "analyzers: Follow → Manual, Key D, Scale Minor (the shown project scale) in ONE undo step",
            Box::new(move |c, _| {
                let d = analyzer_device(c, "pitch-corrector").ok_or("no device")?;
                let def = dstudio_core::builtin::def_of(d).ok_or("no def")?;
                let v = |id: &str| def.param(id).map(|p| def.value(d, p));
                let n = c.doc.history_labels().len();
                (n == h1.get() + 1 && v("key_follow") == Some(0.0) && v("key") == Some(2.0) && v("scale") == Some(2.0))
                    .then_some(())
                    .ok_or(format!("history {} -> {n}, follow {:?} key {:?} scale {:?}", h1.get(), v("key_follow"), v("key"), v("scale")))
            }),
        ),
        DoC(
            "analyzers: undo",
            Box::new(|a, ctl| {
                let mut c = ctl.borrow_mut();
                c.undo();
                c.update_ui(a);
            }),
        ),
        Check(
            "analyzers: undo restores Follow = Project",
            Box::new(|c, _| {
                let d = analyzer_device(c, "pitch-corrector").ok_or("no device")?;
                let def = dstudio_core::builtin::def_of(d).ok_or("no def")?;
                let f = def.param("key_follow").map(|p| def.value(d, p));
                (f == Some(1.0)).then_some(()).ok_or(format!("follow {f:?}"))
            }),
        ),
        DoC(
            "analyzers: remove the three devices and the project key",
            Box::new(|a, ctl| {
                let mut c = ctl.borrow_mut();
                c.execute(dstudio_core::Command::SetProjectKey { key: None });
                for id in ["pitch-corrector", "tuner", "spectrum"] {
                    if let Some(d) = analyzer_device(&c, id).map(|d| d.id) {
                        c.execute(dstudio_core::Command::RemoveDevice { device: d });
                    }
                }
                c.update_ui(a);
            }),
        ),
        Check(
            "analyzers: the audio track's chain is empty again",
            Box::new(|c, _| {
                let t = track_of(c, TrackKind::Audio).ok_or("no audio track")?;
                let n = c.project().tracks[t].devices.len();
                (n == 0 && c.project().transport.key.is_none())
                    .then_some(())
                    .ok_or(format!("{n} devices left, key {:?}", c.project().transport.key))
            }),
        ),
    ]
}

/// Built-in device of definition `id` on the first audio track.
fn builtin_def_of<'a>(c: &'a Controller, id: &str) -> Option<(&'a dstudio_core::Device, &'static dstudio_core::builtin::DeviceDef)> {
    let t = track_of(c, TrackKind::Audio)?;
    c.project().tracks[t].devices.iter().find_map(|d| dstudio_core::builtin::def_of(d).filter(|def| def.id == id).map(|def| (d, def)))
}

fn leak(s: String) -> &'static str {
    Box::leak(s.into_boxed_str())
}

/// Character and Multiband Compressor: added from the Built-in list, live in the engine,
/// and their custom controls (Opto latch, band selector) drive the card.
fn dynamics_device_steps() -> Vec<Step> {
    use Step::*;
    let mut v = Vec::new();
    for (id, name, readouts) in
        [("character-compressor", "Character Compressor", 4usize), ("multiband-compressor", "Multiband Compressor", 8)]
    {
        let hist = Rc::new(Cell::new(0usize));
        let h0 = hist.clone();
        v.push(DoC(
            format!("dynamics: Add device › Built-in › {name}").leak(),
            Box::new(move |a, ctl| {
                h0.set(ctl.borrow().doc.history_labels().len());
                key(a, Key::Escape, false, false);
                let add_x = add_card_x(a, ctl);
                click(a, bottom_x() + add_x + 60.0, bottom_y() + 34.5 + 56.5 + 11.0);
                if let Ok(mut c) = ctl.try_borrow_mut() {
                    c.update_ui(a);
                }
                if let Some(i) = devmenu_labels(a).iter().position(|r| r.0 == name) {
                    let (x, y) = devmenu_row(a, i);
                    click(a, x, y);
                }
            }),
        ));
        v.push(Check(
            format!("dynamics: {name} added in one undo step, card built, engine live ({readouts} readouts)").leak(),
            Box::new(move |c, a| {
                let n = c.doc.history_labels().len();
                if n != hist.get() + 1 || c.doc.undo_label() != Some(&format!("Add {name}")) {
                    return Err(format!("history {} -> {n}, last {:?}", hist.get(), c.doc.undo_label()));
                }
                let d = builtin_of(c, id).ok_or("no device")?;
                let cd = card_named(a, name).ok_or("no card")?;
                let mut r = Vec::new();
                c.engine.builtin_readouts(d.id.0, &mut r).map_err(|e| e.to_string())?;
                let errors = c.engine.plugin_load_errors();
                let ok = cd.builtin
                    && slint::Model::row_count(&cd.knobs) >= 6
                    && (cd.strip.visible || cd.bands.visible)
                    && r.len() == readouts
                    && !errors.iter().any(|(e, _)| *e == d.id);
                ok.then_some(()).ok_or(format!("knobs {} readouts {r:?} errors {errors:?}", slint::Model::row_count(&cd.knobs)))
            }),
        ));
    }
    v.push(DoC(
        "dynamics: Character Compressor model › Opto",
        Box::new(|_, ctl| {
            let mut c = ctl.borrow_mut();
            let Some(d) = builtin_of(&c, "character-compressor").map(|d| d.id) else { return };
            let def = dstudio_core::builtin::find("character-compressor").unwrap();
            let i = def.params.iter().position(|p| p.id == "model").unwrap_or(0) as i32;
            let opto = def.param("model").and_then(|p| p.choices.iter().position(|c| *c == "Opto")).unwrap_or(0) as i32;
            c.panel_choice(d, i, opto);
        }),
    ));
    v.push(Check(
        "dynamics: Opto shows the Comp/Limit latch in the dial row",
        Box::new(|_, a| {
            let cd = card_named(a, "Character Compressor").ok_or("no card")?;
            let latch = (0..slint::Model::row_count(&cd.knobs)).filter_map(|i| slint::Model::row_data(&cd.knobs, i)).find(|k| k.face == 2);
            latch.is_some_and(|k| k.value == "Comp" || k.value == "Limit").then_some(()).ok_or("no latch".into())
        }),
    ));
    v.push(DoC(
        "dynamics: Multiband band selector › Mid",
        Box::new(|a, ctl| {
            let Some(d) = builtin_of(&ctl.borrow(), "multiband-compressor").map(|d| d.id.0 as i32) else { return };
            a.global::<Studio>().invoke_panel_view(d, 1, 1.0);
        }),
    ));
    v.push(Check(
        "dynamics: the Threshold knob follows the Mid band (−20.0 dB)",
        Box::new(|_, a| {
            let cd = card_named(a, "Multiband Compressor").ok_or("no card")?;
            let k = slint::Model::row_data(&cd.knobs, 0).ok_or("no knob")?;
            (k.label == "Threshold" && k.value == "\u{2212}20.0 dB").then_some(()).ok_or(format!("{} {}", k.label, k.value))
        }),
    ));
    // Leave the chain as the later sections expect it (their clicks assume its width).
    v.push(DoC(
        "dynamics: remove both devices again",
        Box::new(|a, ctl| {
            let mut c = ctl.borrow_mut();
            for id in ["character-compressor", "multiband-compressor"] {
                if let Some(d) = builtin_of(&c, id).map(|d| d.id) {
                    c.execute(dstudio_core::Command::RemoveDevice { device: d });
                }
            }
            c.update_ui(a);
        }),
    ));
    v.push(Check(
        "dynamics: both devices are gone",
        Box::new(|c, _| {
            (builtin_of(c, "character-compressor").is_none() && builtin_of(c, "multiband-compressor").is_none())
                .then_some(())
                .ok_or("still in the chain".into())
        }),
    ));
    v
}

/// The dynamics built-ins (limiter, gate, glue, transient shaper, mastering
/// limiter): each added from Add device › Built-in with its generated card and a
/// live engine instance, then the gestures the generic panel gained for them:
/// a style choice that writes its overlays, the strip's Alt+drag of two lines,
/// and latches (one undo step each).
fn limiter_family_steps() -> Vec<Step> {
    use Step::*;
    fn clear_audio_chain(a: &AppWindow, ctl: &Ctl) {
        let Some(t) = track_of(&ctl.borrow(), TrackKind::Audio) else { return };
        click_track_header(a, ctl, t);
        let mut c = ctl.borrow_mut();
        let ids: Vec<dstudio_core::DeviceId> = c.project().tracks[t].devices.iter().map(|d| d.id).collect();
        for d in ids {
            c.execute(dstudio_core::Command::RemoveDevice { device: d });
        }
        c.update_ui(a);
    }
    let mut out = vec![DoC("dynamics: select the audio track and clear its chain", Box::new(clear_audio_chain))];
    for id in ["limiter", "gate", "glue-compressor", "transient-shaper", "mastering-limiter"] {
        let def = dstudio_core::builtin::find(id).expect("registered");
        let name = def.name;
        out.push(DoC(
            leak(format!("{id}: Add device › Built-in, pick {name}")),
            Box::new(move |a, ctl| {
                key(a, Key::Escape, false, false);
                if let Ok(mut c) = ctl.try_borrow_mut() {
                    c.update_ui(a);
                }
                let add_x = add_card_x(a, ctl);
                click(a, bottom_x() + add_x + 60.0, bottom_y() + 34.5 + 56.5 + 11.0);
                if let Some(i) = devmenu_labels(a).iter().position(|r| r.0 == name) {
                    let (x, y) = devmenu_row(a, i);
                    click(a, x, y);
                }
            }),
        ));
        out.push(Check(
            leak(format!(
                "{id}: added at its defaults in one undo step; card has its knobs, sliders, view and width; engine instance live"
            )),
            Box::new(move |c, a| {
                let (d, def) = builtin_def_of(c, id).ok_or("not added")?;
                let cd = card_named(a, name).ok_or("no card")?;
                let knobs: usize = def.layout.clusters.iter().map(|k| k.len()).sum();
                let mut r = Vec::new();
                c.engine.builtin_readouts(d.id.0, &mut r).map_err(|e| e.to_string())?;
                let errors = c.engine.plugin_load_errors();
                let ok = c.doc.undo_label() == Some(&format!("Add {name}"))
                    && def.params.iter().all(|p| def.value(d, p) == p.default)
                    && slint::Model::row_count(&cd.knobs) == knobs
                    && slint::Model::row_count(&cd.sliders) == def.layout.sliders.len()
                    && (cd.strip.visible || slint::Model::row_count(&cd.canvas.paths) > 0)
                    && approx(cd.width, def.layout.width() * crate::device_panel::Z, 0.01)
                    && r.len() == 3
                    && !errors.iter().any(|(e, _)| *e == d.id);
                ok.then_some(()).ok_or(format!(
                    "undo {:?} knobs {} sliders {} strip {} width {} readouts {r:?}",
                    c.doc.undo_label(),
                    slint::Model::row_count(&cd.knobs),
                    slint::Model::row_count(&cd.sliders),
                    cd.strip.visible,
                    cd.width
                ))
            }),
        ));
    }
    let undo_len = Rc::new(Cell::new(0usize));
    let (u0, u1, u2, u3, u4, u5) =
        (undo_len.clone(), undo_len.clone(), undo_len.clone(), undo_len.clone(), undo_len.clone(), undo_len.clone());
    let param = |c: &Controller, dev: &str, p: &str| builtin_def_of(c, dev).and_then(|(d, def)| Some(def.value(d, def.param(p)?)));
    let invoke = |dev: &'static str, f: fn(&Studio, i32, &'static dstudio_core::builtin::DeviceDef)| -> Box<dyn Fn(&AppWindow, &Ctl)> {
        Box::new(move |a, ctl| {
            let Some((id, def)) = builtin_def_of(&ctl.borrow(), dev).map(|(d, def)| (d.id.0 as i32, def)) else { return };
            f(&a.global::<Studio>(), id, def);
        })
    };
    out.push(DoC(
        "mastering-limiter: pick Style › Loud",
        Box::new(move |a, ctl| {
            u0.set(ctl.borrow().doc.history_labels().len());
            invoke("mastering-limiter", |s, id, def| s.invoke_panel_choice(id, def.index_of("style").map_or(-1, |i| i as i32), 1))(a, ctl);
        }),
    ));
    out.push(Check(
        "mastering-limiter: the style and its overlay values (times, links, auto release) change in ONE undo step",
        Box::new(move |c, _| {
            let (d, def) = builtin_def_of(c, "mastering-limiter").ok_or("no device")?;
            let want = def.choice_values("style", 1.0);
            let n = c.doc.history_labels().len();
            (n == u1.get() + 1 && want.len() > 1 && want.iter().all(|(k, v)| def.value(d, def.param(k).unwrap()) == *v))
                .then_some(())
                .ok_or(format!("history {} -> {n}, want {want:?}", u1.get()))
        }),
    ));
    out.push(DoC(
        "mastering-limiter: Alt+drag the strip lines down 20 px (threshold and ceiling together)",
        Box::new(move |a, ctl| {
            u2.set(ctl.borrow().doc.history_labels().len());
            let Some((id, rate)) = builtin_def_of(&ctl.borrow(), "mastering-limiter")
                .and_then(|(d, _)| Some((d.id.0 as i32, card_named(a, "Mastering Limiter")?.strip.thresh_rate)))
            else {
                return;
            };
            let s = a.global::<Studio>();
            let def = dstudio_core::builtin::find("mastering-limiter").unwrap();
            let (t, ce) = (def.index_of("threshold").unwrap() as i32, def.index_of("ceiling").unwrap() as i32);
            s.invoke_panel_drag_pair(id, t, ce, 0, 0.0, rate, false);
            for _ in 0..4 {
                s.invoke_panel_drag_pair(id, t, ce, 1, -5.0, rate, false);
            }
            s.invoke_panel_drag_pair(id, t, ce, 2, 0.0, rate, false);
        }),
    ));
    out.push(Check(
        "mastering-limiter: both lines moved down by the same dB, ONE undo step",
        Box::new(move |c, _| {
            let (t, ce) = (
                param(c, "mastering-limiter", "threshold").unwrap_or(f32::NAN),
                param(c, "mastering-limiter", "ceiling").unwrap_or(f32::NAN),
            );
            let n = c.doc.history_labels().len();
            (n == u3.get() + 1 && t < -1.0 && approx(t, ce + 1.0, 0.051))
                .then_some(())
                .ok_or(format!("history {} -> {n}, threshold {t} ceiling {ce}", u3.get()))
        }),
    ));
    out.push(Check(
        "mastering-limiter: Auto Release is a right-aligned picker latch reading \"Auto Release On\"",
        Box::new(|_, a| {
            let cd = card_named(a, "Mastering Limiter").ok_or("no card")?;
            let picks: Vec<_> = (0..slint::Model::row_count(&cd.picks)).filter_map(|i| slint::Model::row_data(&cd.picks, i)).collect();
            let latch = picks.iter().position(|p| p.kind == 5).ok_or("no latch")?;
            let p = &picks[latch];
            let label = p.label.to_string();
            (picks[..latch].iter().any(|q| q.kind == 4) && p.selected == 1 && label == "Auto Release On")
                .then_some(())
                .ok_or(format!("label {label:?}"))
        }),
    ));
    out.push(DoC(
        "transient-shaper: switch the Clip latch on (panel-choice)",
        Box::new(move |a, ctl| {
            u4.set(ctl.borrow().doc.history_labels().len());
            invoke("transient-shaper", |s, id, def| s.invoke_panel_choice(id, def.index_of("clip").map_or(-1, |i| i as i32), 1))(a, ctl);
        }),
    ));
    out.push(Check(
        "transient-shaper: Clip on in one undo step; its dial cell is a latch reading On",
        Box::new(move |c, a| {
            let n = c.doc.history_labels().len();
            let cd = card_named(a, "Transient Shaper").ok_or("no card")?;
            let k = (0..slint::Model::row_count(&cd.knobs))
                .filter_map(|i| slint::Model::row_data(&cd.knobs, i))
                .find(|k| k.face == 2)
                .ok_or("no latch")?;
            (n == u5.get() + 1
                && c.doc.undo_label() == Some("Device Parameter Change")
                && param(c, "transient-shaper", "clip") == Some(1.0)
                && k.norm > 0.5
                && k.value == "On")
                .then_some(())
                .ok_or(format!("history {} -> {n}, clip {:?}", u5.get(), param(c, "transient-shaper", "clip")))
        }),
    ));
    out.push(DoC("dynamics: remove the section's devices", Box::new(clear_audio_chain)));
    out.push(Check(
        "dynamics: the audio track's chain is empty again",
        Box::new(|c, _| {
            let n = track_of(c, TrackKind::Audio).map_or(usize::MAX, |t| c.project().tracks[t].devices.len());
            (n == 0).then_some(()).ok_or(format!("{n} devices left"))
        }),
    ));
    out
}

// ------------------------------------------------------------------ modulation and containers

fn device_where(c: &Controller, f: impl Fn(&dstudio_core::Device) -> bool) -> Option<dstudio_core::Device> {
    let t = track_of(c, TrackKind::Audio)?;
    c.project().tracks[t].devices.iter().find(|d| f(d)).cloned()
}

fn lfo_device(c: &Controller) -> Option<dstudio_core::Device> {
    device_where(c, dstudio_core::modulation::is_lfo)
}

fn chain_head(c: &Controller) -> Option<dstudio_core::Device> {
    device_where(c, dstudio_core::modulation::is_rack)
}

/// LFO from a control's menu, chain head with a member and a macro, folding,
/// collapsing, the swap picker and the plugin card's "+ Add".
fn modulation_steps() -> Vec<Step> {
    use Step::*;
    let hist = Rc::new(Cell::new(0usize));
    let h = || hist.clone();
    fn one(hc: &Rc<Cell<usize>>, c: &Controller, label: &str) -> Result<(), String> {
        let n = c.doc.history_labels().len();
        let was = hc.get();
        hc.set(n);
        (n == was + 1 && c.doc.undo_label() == Some(label))
            .then_some(())
            .ok_or(format!("history {was} -> {n}, last {:?} (want {label})", c.doc.undo_label()))
    }
    let first_comp = |c: &Controller| device_where(c, |d| d.plugin.id == "compressor" && d.chain.is_none()).map(|d| d.id);
    let (h0, h1, h2, h3, h4, h5, h6, h7, h8, h9, h10, h11) = (h(), h(), h(), h(), h(), h(), h(), h(), h(), h(), h(), h());
    vec![
        DoC(
            "modulation: right-click the Compressor's Threshold knob",
            Box::new(move |a, ctl| {
                key(a, Key::Escape, false, false);
                h0.set(ctl.borrow().doc.history_labels().len());
                let Some(id) = first_comp(&ctl.borrow()) else { return };
                a.global::<Studio>().invoke_mod_menu(id.0 as i32, 0, 600.0, 500.0);
            }),
        ),
        Check(
            "modulation: the control menu offers Automate Threshold and Modulators › New LFO",
            Box::new(|_, a| {
                let items = devmenu_labels(a);
                (items.iter().any(|r| r.0 == "Automate Threshold" && r.2) && items.iter().any(|r| r.0 == "New LFO" && r.2))
                    .then_some(())
                    .ok_or(format!("items {items:?}"))
            }),
        ),
        Do(
            "modulation: pick New LFO",
            Box::new(|a| {
                if let Some(i) = devmenu_labels(a).iter().position(|r| r.0 == "New LFO") {
                    let (x, y) = devmenu_row(a, i);
                    click(a, x, y);
                }
            }),
        ),
        Check(
            "modulation: an LFO at the chain start aims at Threshold (±25%) in one undo step; its card lists the target; the knob shows the reach",
            Box::new(move |c, a| {
                one(&h1, c, "New LFO")?;
                let t = track_of(c, TrackKind::Audio).ok_or("no audio track")?;
                let lfo = c.project().tracks[t].devices.first().filter(|d| dstudio_core::modulation::is_lfo(d)).ok_or("no LFO first")?;
                let l = lfo.links.first().ok_or("no link")?;
                let cd = card_named(a, "LFO").ok_or("no LFO card")?;
                let comp = card_named(a, "Compressor").and_then(|cd| slint::Model::row_data(&cd.knobs, 0)).ok_or("no knob")?;
                let row = slint::Model::row_data(&cd.modv.targets, 0).ok_or("no target row")?;
                (l.param == dstudio_core::ModParam::Builtin("threshold".into())
                    && approx(l.max, 0.25, 1e-6)
                    && approx(l.min, -0.25, 1e-6)
                    && cd.modv.kind == 1
                    && row.amount == "25%"
                    && comp.mod_on
                    && comp.mod_hi > comp.mod_lo)
                    .then_some(())
                    .ok_or(format!(
                        "link {l:?} card kind {} row {:?} knob reach {}..{}",
                        cd.modv.kind, row.amount, comp.mod_lo, comp.mod_hi
                    ))
            }),
        ),
        Check(
            "engine: the LFO is live and reports its readouts (position, depth, cycle)",
            Box::new(|c, _| {
                let id = lfo_device(c).ok_or("no LFO")?.id;
                let mut r = Vec::new();
                c.engine.builtin_readouts(id.0, &mut r).map_err(|e| e.to_string())?;
                (r.len() == 3).then_some(()).ok_or(format!("readouts {r:?}"))
            }),
        ),
        DoC(
            "modulation: drag the target amount up 30 px",
            Box::new(|_, ctl| {
                let Some(id) = lfo_device(&ctl.borrow()).map(|d| d.id) else { return };
                let mut c = ctl.borrow_mut();
                c.mod_amount_drag(id, 0, 0, 0.0, false);
                for _ in 0..30 {
                    c.mod_amount_drag(id, 0, 1, 1.0, false);
                }
                c.mod_amount_drag(id, 0, 2, 0.0, false);
            }),
        ),
        Check(
            "modulation: amount 25% → 45% (±), one undo step",
            Box::new(move |c, _| {
                one(&h2, c, "Modulation Amount Change")?;
                let l = lfo_device(c).and_then(|d| d.links.first().cloned()).ok_or("no link")?;
                (approx(l.max, 0.45, 1e-4) && approx(l.min, -l.max, 1e-6)).then_some(()).ok_or(format!("link {l:?}"))
            }),
        ),
        Do(
            "modulation: fold the LFO (header −)",
            Box::new(|a| {
                if let Some(cd) = card_named(a, "LFO") {
                    let z = crate::device_panel::Z;
                    let (x, y) = card_xy(&cd, cd.width - 33.0 * z, 16.0 * z);
                    click(a, x, y);
                }
            }),
        ),
        Check(
            "modulation: the LFO folds to a 30 pt strip, one undo step",
            Box::new(move |c, a| {
                one(&h3, c, "Fold Device")?;
                let cd = card_named(a, "LFO").ok_or("no card")?;
                (cd.folded && approx(cd.width, crate::device_mod::FOLDED_W, 0.01))
                    .then_some(())
                    .ok_or(format!("folded {} width {}", cd.folded, cd.width))
            }),
        ),
        DoC(
            "modulation: click the folded strip",
            Box::new(|a, ctl| {
                if let Some(cd) = card_in_view(a, ctl, "LFO") {
                    let (x, y) = card_xy(&cd, 15.0, 150.0);
                    click(a, x, y);
                }
            }),
        ),
        Check(
            "modulation: a click unfolds it, one undo step",
            Box::new(move |c, a| {
                one(&h4, c, "Unfold Device")?;
                card_named(a, "LFO").is_some_and(|cd| !cd.folded).then_some(()).ok_or("still folded".into())
            }),
        ),
        DoC(
            "chain: Add device › Chains",
            Box::new(|a, ctl| {
                // The Add device card is scrolled out of view by now: use its row callback.
                a.global::<Studio>().invoke_add_category(4, 600.0, 300.0);
                if let Ok(mut c) = ctl.try_borrow_mut() {
                    c.update_ui(a);
                }
            }),
        ),
        Do(
            "chain: pick Empty chain",
            Box::new(|a| {
                if let Some(i) = devmenu_labels(a).iter().position(|r| r.0 == "Empty chain") {
                    let (x, y) = devmenu_row(a, i);
                    click(a, x, y);
                }
            }),
        ),
        Check(
            "chain: a chain head with 8 macros, one undo step; its empty frame offers Add effect",
            Box::new(move |c, a| {
                one(&h5, c, "Add Chain")?;
                let cd = card_named(a, "Chain").ok_or("no Chain card")?;
                let frames = a.global::<Studio>().get_chain_frames();
                let f = slint::Model::row_data(&frames, 0).ok_or("no frame")?;
                (cd.modv.kind == 2 && slint::Model::row_count(&cd.knobs) == 8 && f.empty).then_some(()).ok_or(format!(
                    "kind {} macros {} empty {}",
                    cd.modv.kind,
                    slint::Model::row_count(&cd.knobs),
                    f.empty
                ))
            }),
        ),
        DoC(
            "chain: Add effect › Compressor",
            Box::new(|a, ctl| {
                let Some(head) = chain_head(&ctl.borrow()).map(|d| d.id) else { return };
                a.global::<Studio>().invoke_chain_add_effect(head.0 as i32, 600.0, 300.0);
                if let Ok(mut c) = ctl.try_borrow_mut() {
                    c.update_ui(a);
                }
                if let Some(i) = devmenu_labels(a).iter().position(|r| r.0 == "Compressor") {
                    let (x, y) = devmenu_row(a, i);
                    click(a, x, y);
                }
            }),
        ),
        Check(
            "chain: the Compressor joins the chain right after the head, one undo step; the frame wraps it",
            Box::new(move |c, a| {
                one(&h6, c, "Add Compressor")?;
                let head = chain_head(c).ok_or("no head")?;
                let member = device_where(c, |d| d.chain == Some(head.id)).ok_or("no member")?;
                let f = slint::Model::row_data(&a.global::<Studio>().get_chain_frames(), 0).ok_or("no frame")?;
                (member.plugin.id == "compressor" && !f.empty)
                    .then_some(())
                    .ok_or(format!("member {:?} empty {}", member.plugin.id, f.empty))
            }),
        ),
        DoC(
            "chain: open Macro 1, press Map, click the member's Threshold knob",
            Box::new(|_, ctl| {
                let (head, member) = {
                    let c = ctl.borrow();
                    let Some(h) = chain_head(&c) else { return };
                    let Some(m) = device_where(&c, |d| d.chain == Some(h.id)) else { return };
                    (h.id, m.id)
                };
                // The member card may be scrolled out of the window: press its knob through the panel callback.
                let mut c = ctl.borrow_mut();
                c.macro_select(head, 0);
                c.mod_map(head, 0);
                c.panel_drag(member, 0, 0, 0.0, 1.0 / 160.0, false);
                c.panel_drag(member, 0, 2, 0.0, 1.0 / 160.0, false);
            }),
        ),
        Check(
            "chain: Macro 1 targets the member's Threshold (one undo step) and is drawn mapped",
            Box::new(move |c, a| {
                one(&h7, c, "Add Modulation")?;
                let head = chain_head(c).ok_or("no head")?;
                let l = head.links.first().ok_or("no link")?;
                let cd = card_named(a, "Chain").ok_or("no card")?;
                let m1 = slint::Model::row_data(&cd.knobs, 0).ok_or("no macro")?;
                (l.slot == 0
                    && l.param == dstudio_core::ModParam::Builtin("threshold".into())
                    && m1.mapped
                    && m1.selected
                    && slint::Model::row_count(&cd.modv.targets) == 1)
                    .then_some(())
                    .ok_or(format!("link {l:?} mapped {} selected {}", m1.mapped, m1.selected))
            }),
        ),
        Do("chain: Escape", Box::new(|a| key(a, Key::Escape, false, false))),
        Check(
            "chain: Escape stops mapping",
            Box::new(|c, _| c.devices.modu.map.is_none().then_some(()).ok_or(format!("map {:?}", c.devices.modu.map))),
        ),
        DoC(
            "chain: drag Macro 1 to the top",
            Box::new(|_, ctl| {
                let Some(head) = chain_head(&ctl.borrow()).map(|d| d.id) else { return };
                let mut c = ctl.borrow_mut();
                c.panel_drag(head, 0, 0, 0.0, 1.0 / 100.0, false);
                c.panel_drag(head, 0, 1, 120.0, 1.0 / 100.0, false);
                c.panel_drag(head, 0, 2, 0.0, 1.0 / 100.0, false);
            }),
        ),
        Check(
            "chain: Macro 1 at 100% moves the member's Threshold to the top of its range (shown on the knob)",
            Box::new(move |c, a| {
                one(&h8, c, "Device Parameter Change")?;
                let head = chain_head(c).ok_or("no head")?;
                let member = device_where(c, |d| d.chain == Some(head.id)).ok_or("no member")?;
                let k = cards(a)
                    .into_iter()
                    .find(|cd| cd.id as u64 == member.id.0)
                    .and_then(|cd| slint::Model::row_data(&cd.knobs, 0))
                    .ok_or("no knob")?;
                (k.mod_on && approx(k.mod_now, 1.0, 1e-4)).then_some(()).ok_or(format!("knob now {}", k.mod_now))
            }),
        ),
        DoC(
            "chain: collapse the chain («)",
            Box::new(|a, ctl| {
                let Some(head) = chain_head(&ctl.borrow()).map(|d| d.id) else { return };
                a.global::<Studio>().invoke_device_collapse(head.0 as i32);
            }),
        ),
        Check(
            "chain: collapsed, the member's card is hidden (one undo step)",
            Box::new(move |c, a| {
                one(&h9, c, "Unfold Device")?;
                let head = chain_head(c).ok_or("no head")?;
                let member = device_where(c, |d| d.chain == Some(head.id)).ok_or("no member")?;
                (head.collapsed && !cards(a).iter().any(|cd| cd.id as u64 == member.id.0)).then_some(()).ok_or("member still shown".into())
            }),
        ),
        DoC(
            "chain: ✕ on the chain head",
            Box::new(|a, ctl| {
                let Some(head) = chain_head(&ctl.borrow()).map(|d| d.id) else { return };
                match cards(a).into_iter().find(|cd| cd.id as u64 == head.0) {
                    // Click the ✕ when the card is on screen; otherwise (a smaller
                    // window, or a longer device row) press it through its callback.
                    Some(cd) if bottom_x() + cd.x + cd.width <= win().0 - 5.0 => {
                        let (x, y) = card_xy(&cd, cd.width - 14.5, 13.5);
                        click(a, x, y);
                    }
                    _ => a.global::<Studio>().invoke_device_remove(head.0 as i32),
                }
            }),
        ),
        Check(
            "chain: removing the head removes its member in one undo step",
            Box::new(move |c, _| {
                one(&h10, c, "Remove Chain")?;
                (chain_head(c).is_none() && device_where(c, |d| d.chain.is_some()).is_none())
                    .then_some(())
                    .ok_or("chain left behind".into())
            }),
        ),
        Do("chain: ⌘Z", Box::new(|a| key(a, "z", true, false))),
        Check(
            "chain: undo brings back the head and its member",
            Box::new(move |c, _| {
                h11.set(c.doc.history_labels().len());
                let head = chain_head(c)
                    .ok_or(format!("no head; history {:?}", c.doc.history_labels().iter().rev().take(3).collect::<Vec<_>>()))?;
                device_where(c, |d| d.chain == Some(head.id)).map(|_| ()).ok_or("no member".into())
            }),
        ),
        DoC(
            "swap: open the Compressor's name menu",
            Box::new(move |a, ctl| {
                let Some(id) = first_comp(&ctl.borrow()) else { return };
                a.global::<Studio>().invoke_device_swap_menu(id.0 as i32, 600.0, 300.0);
            }),
        ),
        Check(
            "swap: the effect picker marks Compressor and offers a new LFO",
            Box::new(|_, a| {
                let items = devmenu_labels(a);
                (items.iter().any(|r| r.0 == "Compressor" && r.1 == "✓") && items.iter().any(|r| r.0 == "LFO" && r.2))
                    .then_some(())
                    .ok_or(format!("items {items:?}"))
            }),
        ),
        DoC(
            "plugin card: add the CLAP test gain, open + Add",
            Box::new(|a, ctl| {
                key(a, Key::Escape, false, false);
                let gain = dstudio_core::PluginRef {
                    format: dstudio_core::PluginFormat::Clap,
                    id: "studio.dstudio.test.gain".into(),
                    path: Some(dstudio_engine::test_plugin_dir().join("DStudioTestGain.clap").to_string_lossy().into_owned()),
                    name: "DStudio Test Gain".into(),
                    vendor: "DStudio".into(),
                    version: "1.0.0".into(),
                    kind: dstudio_core::PluginKind::Effect,
                };
                let id = {
                    let mut c = ctl.borrow_mut();
                    let Some(t) = track_of(&c, TrackKind::Audio).map(|i| c.project().tracks[i].id) else { return };
                    c.add_item_to_track(&crate::devices::Item::Plugin(gain), t, None);
                    c.update_ui(a);
                    c.project().track(t).and_then(|t| t.devices.last()).map(|d| d.id)
                };
                if let Some(id) = id {
                    a.global::<Studio>().invoke_card_add_menu(id.0 as i32, 600.0, 300.0);
                }
            }),
        ),
        Check(
            "plugin card: a new card shows only + Add; the list names the plugin's parameters",
            Box::new(|_, a| {
                let cd = card_named(a, "DStudio Test Gain").ok_or("no card")?;
                let items = devmenu_labels(a);
                (!cd.builtin && slint::Model::row_count(&cd.knobs) == 0 && items.iter().any(|r| r.2))
                    .then_some(())
                    .ok_or(format!("knobs {} items {items:?}", slint::Model::row_count(&cd.knobs)))
            }),
        ),
        Do(
            "plugin card: pick the first parameter",
            Box::new(|a| {
                if let Some(i) = devmenu_labels(a).iter().position(|r| r.2) {
                    let (x, y) = devmenu_row(a, i);
                    click(a, x, y);
                }
            }),
        ),
        Check(
            "plugin card: the control is on the card (SetCardParams, one undo step)",
            Box::new(|c, a| {
                let cd = card_named(a, "DStudio Test Gain").ok_or("no card")?;
                (c.doc.undo_label() == Some("Device Controls Change") && slint::Model::row_count(&cd.knobs) == 1)
                    .then_some(())
                    .ok_or(format!("last {:?} knobs {}", c.doc.undo_label(), slint::Model::row_count(&cd.knobs)))
            }),
        ),
    ]
}

// ------------------------------------------------------------------ custom device views

fn device_of<'a>(c: &'a Controller, id: &str) -> Option<&'a dstudio_core::Device> {
    let t = track_of(c, TrackKind::Audio)?;
    c.project().tracks[t].devices.iter().find(|d| dstudio_core::builtin::def_of(d).is_some_and(|def| def.id == id))
}

fn value_of(c: &Controller, device: &str, param: &str) -> Option<f32> {
    let d = device_of(c, device)?;
    let def = dstudio_core::builtin::def_of(d)?;
    Some(def.value(d, def.param(param)?))
}

/// A pointer event on a custom view at a card-relative point (pt at 100%).
fn field_event(a: &AppWindow, ctl: &Ctl, device: &str, kind: i32, x: f32, y: f32) {
    let Some(id) = device_of(&ctl.borrow(), device).map(|d| d.id.0 as i32) else { return };
    let z = crate::device_panel::Z;
    a.global::<Studio>().invoke_panel_editor(id, kind, x * z, y * z - 27.0, 600.0, 400.0, false, false, 0.0);
}

/// Window click on the Trance Gate's ring hole (no action): gives its field keyboard focus.
fn focus_trance_gate(a: &AppWindow) {
    if let Some(cd) = card_named(a, "Trance Gate") {
        let z = crate::device_panel::Z;
        let (x, y) = card_xy(&cd, 98.0 * z, 157.5 * z + 30.0 * z);
        click(a, x, y);
    }
}

/// Trance Gate ring: card point (pt at 100%) in the middle of step i of 16.
fn ring_xy(i: usize) -> (f32, f32) {
    let a = std::f32::consts::TAU * (i as f32 + 0.5) / 16.0;
    let r = 0.8 * 83.0;
    (98.0 + r * a.sin(), 157.5 - r * a.cos())
}

/// Trance Gate and Shaper: added from the Built-in list, then driven through their custom views.
fn custom_view_steps() -> Vec<Step> {
    use Step::*;
    let hist = Rc::new(Cell::new(0usize));
    let mask0 = Rc::new(Cell::new(0u32));
    fn grew(hc: &Rc<Cell<usize>>, c: &Controller, n: usize) -> Result<(), String> {
        let now = c.doc.history_labels().len();
        let was = hc.get();
        hc.set(now);
        (now == was + n).then_some(()).ok_or(format!("history {was} -> {now} (want +{n}), last {:?}", c.doc.undo_label()))
    }
    fn mask(c: &Controller) -> u32 {
        dstudio_core::builtin::trance_gate::mask(
            value_of(c, "trance-gate", "p1_lo").unwrap_or(0.0),
            value_of(c, "trance-gate", "p1_hi").unwrap_or(0.0),
        )
    }
    fn act(ctl: &Ctl, a: &AppWindow, f: impl FnOnce(&AppWindow, &Controller) -> Vec<(i32, f32, f32)>, device: &str) {
        let events = f(a, &ctl.borrow());
        for (kind, x, y) in events {
            field_event(a, ctl, device, kind, x, y);
        }
        if let Ok(mut c) = ctl.try_borrow_mut() {
            c.update_ui(a);
        }
    }
    let (h0, h1, h2, h3, h4, h5, h6, h7) =
        (hist.clone(), hist.clone(), hist.clone(), hist.clone(), hist.clone(), hist.clone(), hist.clone(), hist.clone());
    let (m0, m1, m2, m3) = (mask0.clone(), mask0.clone(), mask0.clone(), mask0.clone());
    vec![
        Do("views: Escape (close menus)", Box::new(|a| key(a, Key::Escape, false, false))),
        DoC(
            "views: select the first audio track, clear its chain, add Trance Gate and Shaper",
            Box::new(move |a, ctl| {
                let Some(i) = track_of(&ctl.borrow(), TrackKind::Audio) else { return };
                click_track_header(a, ctl, i);
                let mut c = ctl.borrow_mut();
                let ids: Vec<dstudio_core::DeviceId> = c.project().tracks[i].devices.iter().map(|d| d.id).collect();
                for d in ids {
                    c.execute(dstudio_core::Command::RemoveDevice { device: d });
                }
                h0.set(c.doc.history_labels().len());
                let t = c.project().tracks[i].id;
                for id in ["trance-gate", "shaper"] {
                    let def = dstudio_core::builtin::find(id).expect("registered");
                    c.add_item_to_track(&crate::devices::Item::Builtin(def), t, None);
                }
                c.update_ui(a);
            }),
        ),
        Check(
            "views: both added (two undo steps); cards are 528 / 501 pt with an interactive canvas, 6 / 4 knobs on the dial row; engine loaded them",
            Box::new(move |c, a| {
                grew(&h1, c, 2)?;
                let z = crate::device_panel::Z;
                let tg = card_named(a, "Trance Gate").ok_or("no Trance Gate card")?;
                let sh = card_named(a, "Shaper").ok_or("no Shaper card")?;
                let errors = c.engine.plugin_load_errors();
                let ok = tg.canvas.visible
                    && sh.canvas.visible
                    && approx(tg.width, 528.0 * z, 0.01)
                    && approx(sh.width, 501.0 * z, 0.01)
                    && slint::Model::row_count(&tg.knobs) == 6
                    && slint::Model::row_count(&sh.knobs) == 4
                    && slint::Model::row_count(&tg.canvas.paths) > 10
                    && slint::Model::row_count(&sh.canvas.paths) > 10
                    && tg.canvas.interactive
                    && sh.canvas.interactive
                    && [&tg.knobs, &sh.knobs]
                        .iter()
                        .all(|k| slint::Model::iter(*k).all(|k| approx(k.y, crate::device_panel::DIAL_Y * z, 0.01)))
                    && ["trance-gate", "shaper"].iter().all(|id| device_of(c, id).is_some_and(|d| !errors.iter().any(|(e, _)| *e == d.id)));
                ok.then_some(())
                    .ok_or(format!("tg {} {} sh {} {} errors {errors:?}", tg.width, tg.canvas.visible, sh.width, sh.canvas.visible))
            }),
        ),
        DoC(
            "views: click step 1 on the Trance Gate ring",
            Box::new(move |a, ctl| {
                m0.set(mask(&ctl.borrow()));
                act(
                    ctl,
                    a,
                    |_, _| {
                        let (x, y) = ring_xy(0);
                        vec![(0, x, y), (2, x, y)]
                    },
                    "trance-gate",
                );
            }),
        ),
        Check(
            "views: step 1 toggled, one undo step",
            Box::new(move |c, _| {
                grew(&h2, c, 1)?;
                let (was, now) = (m1.get(), mask(c));
                (now == was ^ 1).then_some(()).ok_or(format!("mask {was:#x} -> {now:#x}"))
            }),
        ),
        DoC(
            "views: paint steps 2-4 in one drag",
            Box::new(move |a, ctl| {
                m2.set(mask(&ctl.borrow()));
                act(
                    ctl,
                    a,
                    |_, _| {
                        let ((x1, y1), (x2, y2), (x3, y3)) = (ring_xy(1), ring_xy(2), ring_xy(3));
                        vec![(0, x1, y1), (1, x2, y2), (1, x3, y3), (2, x3, y3)]
                    },
                    "trance-gate",
                );
            }),
        ),
        Check(
            "views: steps 2-4 all take the first step's new state, ONE undo step",
            Box::new(move |c, _| {
                grew(&h3, c, 1)?;
                let (was, now) = (m3.get(), mask(c));
                let want = if was >> 1 & 1 == 1 { was & !0b1110 } else { was | 0b1110 };
                (now == want).then_some(()).ok_or(format!("mask {was:#x} -> {now:#x}, want {want:#x}"))
            }),
        ),
        DoC(
            "views: paint steps 6-7, press Escape before release",
            Box::new(move |a, ctl| {
                focus_trance_gate(a);
                act(
                    ctl,
                    a,
                    |_, _| {
                        let ((x1, y1), (x2, y2)) = (ring_xy(5), ring_xy(6));
                        vec![(0, x1, y1), (1, x2, y2)]
                    },
                    "trance-gate",
                );
                key(a, Key::Escape, false, false);
                act(ctl, a, |_, _| vec![(2, 0.0, 0.0)], "trance-gate");
            }),
        ),
        Check("views: Escape cancels the paint (no undo step)", Box::new(move |c, _| grew(&h4, c, 0))),
        DoC(
            "views: drag the Shaper's middle point down 40 px",
            Box::new(|a, ctl| {
                act(
                    ctl,
                    a,
                    |_, _| {
                        let (x, y) = (48.0 + 0.5 * 435.0, 108.0);
                        let d = 40.0 / crate::device_panel::Z;
                        vec![(0, x, y), (1, x, y + d / 2.0), (1, x, y + d), (2, x, y + d)]
                    },
                    "shaper",
                );
            }),
        ),
        Check(
            "views: the point follows (Volume 100% -> about 51%), one undo step",
            Box::new(move |c, _| {
                grew(&h5, c, 1)?;
                let v = value_of(c, "shaper", "vol_p2_y").unwrap_or(f32::NAN);
                approx(v, 51.0, 1.5).then_some(()).ok_or(format!("vol_p2_y {v}"))
            }),
        ),
        DoC(
            "views: Shaper › Filter tab, switch the lane on, pick the second library shape",
            Box::new(|a, ctl| {
                act(
                    ctl,
                    a,
                    |_, _| vec![(0, 291.5 + 30.0, 50.0), (2, 291.5 + 30.0, 50.0), (0, 291.5, 50.0), (2, 291.5, 50.0), (0, 60.0, 81.0)],
                    "shaper",
                );
                if let Some(i) = devmenu_labels(a).iter().position(|r| r.0 == dstudio_core::builtin::shaper::SHAPES[4][1].name) {
                    let (x, y) = devmenu_row(a, i);
                    click(a, x, y);
                }
            }),
        ),
        Check(
            "views: Filter lane on and shaped (two undo steps); the strip names the shape",
            Box::new(move |c, _| {
                grew(&h6, c, 2)?;
                let d = device_of(c, "shaper").ok_or("no shaper")?;
                let def = dstudio_core::builtin::def_of(d).ok_or("no def")?;
                let pts = dstudio_core::builtin::shaper::lane_points(&def.values(d), 4);
                let name = dstudio_core::builtin::shaper::matching_shape(4, &pts);
                (value_of(c, "shaper", "filter_on") == Some(1.0) && name == Some(dstudio_core::builtin::shaper::SHAPES[4][1].name))
                    .then_some(())
                    .ok_or(format!("filter_on {:?} shape {name:?}", value_of(c, "shaper", "filter_on")))
            }),
        ),
        Do(
            "views: click the Trance Gate's ring hole, ⌘Z",
            Box::new(|a| {
                focus_trance_gate(a);
                key(a, "z", true, false);
            }),
        ),
        Check(
            "views: undo restores the Filter lane's default shape",
            Box::new(move |c, _| {
                h7.set(c.doc.history_labels().len());
                let d = device_of(c, "shaper").ok_or("no shaper")?;
                let def = dstudio_core::builtin::def_of(d).ok_or("no def")?;
                let pts = dstudio_core::builtin::shaper::lane_points(&def.values(d), 4);
                let name = dstudio_core::builtin::shaper::matching_shape(4, &pts);
                (name == Some(dstudio_core::builtin::shaper::SHAPES[4][0].name)).then_some(()).ok_or(format!("shape {name:?}"))
            }),
        ),
        DoC(
            "views: remove Trance Gate and Shaper",
            Box::new(|a, ctl| {
                let mut c = ctl.borrow_mut();
                let ids: Vec<dstudio_core::DeviceId> =
                    ["trance-gate", "shaper"].iter().filter_map(|id| device_of(&c, id).map(|d| d.id)).collect();
                for d in ids {
                    c.execute(dstudio_core::Command::RemoveDevice { device: d });
                }
                c.update_ui(a);
            }),
        ),
        Check(
            "views: the audio track's chain is empty again",
            Box::new(|c, _| {
                let t = track_of(c, TrackKind::Audio).ok_or("no audio track")?;
                let n = c.project().tracks[t].devices.len();
                (n == 0).then_some(()).ok_or(format!("{n} devices left"))
            }),
        ),
    ]
}

/// The restoration devices (De-Clipper, De-Esser, De-Hummer, De-Noiser): each one is added
/// through Add device › Built-in, checked against its definition and the engine, and its field
/// is dragged with real pointer events (one undo step), then undone.
fn restoration_device_steps() -> Vec<Step> {
    use Step::*;
    // (display name, id, readouts, field drag in px, params the drag must move)
    type Moves = &'static [&'static str];
    const DEVICES: [(&str, &str, usize, (f32, f32), Moves); 4] = [
        ("De-Clipper", "declipper", 0, (40.0, 0.0), &["threshold"]),
        ("De-Esser", "deesser", 49, (30.0, 20.0), &["frequency", "reduction"]),
        ("De-Hummer", "dehummer", 48, (-30.0, 20.0), &["frequency", "depth"]),
        ("De-Noiser", "denoiser", 49, (0.0, -20.0), &["threshold"]),
    ];
    fn device<'a>(c: &'a Controller, id: &str) -> Option<&'a dstudio_core::Device> {
        let t = track_of(c, TrackKind::Audio)?;
        c.project().tracks[t].devices.iter().find(|d| d.plugin.format == dstudio_core::PluginFormat::Builtin && d.plugin.id == id)
    }
    fn value(c: &Controller, id: &str, param: &str) -> Option<f32> {
        let d = device(c, id)?;
        let def = dstudio_core::builtin::def_of(d)?;
        Some(def.value(d, def.param(param)?))
    }
    let mut steps = Vec::new();
    for (name, id, readouts, (dx, dy), moves) in DEVICES {
        let hist = Rc::new(Cell::new(0usize));
        let (h0, h1, h2, h3) = (hist.clone(), hist.clone(), hist.clone(), hist.clone());
        steps.push(DoC(
            leak(format!("restoration: clear the audio track's chain, open Add device › Built-in for {name}")),
            Box::new(move |a, ctl| {
                key(a, Key::Escape, false, false);
                let i = track_of(&ctl.borrow(), TrackKind::Audio).unwrap_or(0);
                click_track_header(a, ctl, i);
                {
                    let mut c = ctl.borrow_mut();
                    let ids: Vec<dstudio_core::DeviceId> = c.project().tracks[i].devices.iter().map(|d| d.id).collect();
                    for d in ids {
                        c.execute(dstudio_core::Command::RemoveDevice { device: d });
                    }
                    h0.set(c.doc.history_labels().len());
                    c.update_ui(a);
                }
                let add_x = add_card_x(a, ctl);
                click(a, bottom_x() + add_x + 60.0, bottom_y() + 34.5 + 56.5 + 11.0);
            }),
        ));
        steps.push(Do(
            leak(format!("restoration: pick {name}")),
            Box::new(move |a| {
                if let Some(i) = devmenu_labels(a).iter().position(|r| r.0 == name && r.2) {
                    let (x, y) = devmenu_row(a, i);
                    click(a, x, y);
                }
            }),
        ));
        steps.push(Check(
            leak(format!("restoration: {name} added at its defaults (one undo step); card follows the definition (width, knobs, field)")),
            Box::new(move |c, a| {
                let n = c.doc.history_labels().len();
                let want = format!("Add {name}");
                if n != h1.get() + 1 || c.doc.undo_label() != Some(want.as_str()) {
                    return Err(format!("history {} -> {n}, last {:?}", h1.get(), c.doc.undo_label()));
                }
                let def = dstudio_core::builtin::find(id).ok_or("not registered")?;
                let d = device(c, id).ok_or("not in the chain")?;
                let cd = card_named(a, name).ok_or("no card")?;
                let knobs: usize = def.layout.clusters.iter().map(|c| c.len()).sum();
                let ok = cd.builtin
                    && cd.canvas.visible
                    && slint::Model::row_count(&cd.knobs) == knobs
                    && approx(cd.width, def.layout.width() * crate::device_panel::Z, 0.01)
                    && def.params.iter().all(|p| def.value(d, p) == p.default);
                ok.then_some(()).ok_or(format!(
                    "knobs {} width {} field {}",
                    slint::Model::row_count(&cd.knobs),
                    cd.width,
                    cd.canvas.visible
                ))
            }),
        ));
        steps.push(Check(
            leak(format!("engine: {name} is live with {readouts} readouts (spectrum bands where the field draws one)")),
            Box::new(move |c, _| {
                let d = device(c, id).ok_or("no device")?.id;
                let mut r = Vec::new();
                c.engine.builtin_readouts(d.0, &mut r).map_err(|e| e.to_string())?;
                let errors = c.engine.plugin_load_errors();
                (r.len() == readouts && !errors.iter().any(|(e, _)| *e == d))
                    .then_some(())
                    .ok_or(format!("readouts {} errors {errors:?}", r.len()))
            }),
        ));
        steps.push(DoC(
            leak(format!("restoration: drag {name}'s field ({dx:+}, {dy:+}) px")),
            Box::new(move |a, ctl| {
                h2.set(ctl.borrow().doc.history_labels().len());
                let Some(cd) = card_named(a, name) else { return };
                let f = &cd.canvas;
                // The De-Esser grabs its band handle (the circle path "M{x - r} {y} a..."), the
                // De-Noiser its threshold line; the others drag the plane.
                let handle = || {
                    let p = slint::Model::iter(&f.paths).find(|p| p.commands.contains(" a"))?;
                    let mut n = p.commands.trim_start_matches('M').split_whitespace().map(|t| t.parse::<f32>().unwrap_or(0.0));
                    Some((n.next()? + 5.0 * crate::device_panel::Z, n.next()?))
                };
                let (wx, wy) = match id {
                    "deesser" => handle().unwrap_or((0.0, 0.0)),
                    "denoiser" => (f.w * 0.75, slint::Model::row_data(&f.grips, 0).map_or(0.0, |p| p.y)),
                    _ => (f.w / 2.0, (f.h - f.well_y) / 2.0),
                };
                let (x, y) = card_xy(&cd, f.x + wx, f.y + f.well_y + wy);
                drag(a, x, y, x + dx, y + dy);
            }),
        ));
        steps.push(Check(
            leak(format!("restoration: the {name} field gesture moved {moves:?} in ONE undo step")),
            Box::new(move |c, _| {
                let n = c.doc.history_labels().len();
                if n != h3.get() + 1 || c.doc.undo_label() != Some("Device Parameter Change") {
                    return Err(format!("history {} -> {n}, last {:?}", h3.get(), c.doc.undo_label()));
                }
                let def = dstudio_core::builtin::find(id).ok_or("not registered")?;
                let still: Vec<&str> = moves.iter().copied().filter(|p| value(c, id, p) == def.param(p).map(|p| p.default)).collect();
                still.is_empty().then_some(()).ok_or(format!("unchanged: {still:?}"))
            }),
        ));
        steps.push(Do(leak(format!("restoration: ⌘Z after the {name} gesture")), Box::new(|a| key(a, "z", true, false))));
        steps.push(Check(
            leak(format!("restoration: undo restores every {name} parameter")),
            Box::new(move |c, _| {
                let def = dstudio_core::builtin::find(id).ok_or("not registered")?;
                let d = device(c, id).ok_or("no device")?;
                let off: Vec<&str> = def.params.iter().filter(|p| def.value(d, p) != p.default).map(|p| p.id).collect();
                let vals: Vec<(&str, f32)> = def.params.iter().map(|p| (p.id, def.value(d, p))).collect();
                off.is_empty().then_some(()).ok_or(format!(
                    "not at default: {off:?} values {vals:?} history {:?}",
                    c.doc.history_labels().iter().rev().take(4).collect::<Vec<_>>()
                ))
            }),
        ));
    }
    // The De-Esser's Listen latch in the dial row (the De-Esser is still in the chain from above).
    let hl = Rc::new(Cell::new(0usize));
    let hl2 = hl.clone();
    steps.push(DoC(
        "restoration: click the De-Esser's Listen latch",
        Box::new(move |a, ctl| {
            {
                let mut c = ctl.borrow_mut();
                let Some(t) = track_of(&c, TrackKind::Audio) else { return };
                let ids: Vec<dstudio_core::DeviceId> = c.project().tracks[t].devices.iter().map(|d| d.id).collect();
                for d in ids {
                    c.execute(dstudio_core::Command::RemoveDevice { device: d });
                }
                let tid = c.project().tracks[t].id;
                let def = dstudio_core::builtin::find("deesser").expect("registered");
                c.add_item_to_track(&crate::devices::Item::Builtin(def), tid, None);
                hl.set(c.doc.history_labels().len());
                c.update_ui(a);
            }
            let Some(cd) = card_named(a, "De-Esser") else { return };
            let z = crate::device_panel::Z;
            if let Some(k) =
                (0..slint::Model::row_count(&cd.knobs)).filter_map(|i| slint::Model::row_data(&cd.knobs, i)).find(|k| k.face == 2)
            {
                let (x, y) = card_xy(&cd, k.x + 31.0 * z, (229.0 + 24.0) * z);
                click(a, x, y);
            }
        }),
    ));
    steps.push(Check(
        "restoration: the Listen latch turns Listen on (one undo step) and shows On",
        Box::new(move |c, a| {
            let n = c.doc.history_labels().len();
            let shown = card_named(a, "De-Esser")
                .and_then(|cd| {
                    (0..slint::Model::row_count(&cd.knobs)).filter_map(|i| slint::Model::row_data(&cd.knobs, i)).find(|k| k.face == 2)
                })
                .map(|k| k.value.to_string());
            (n == hl2.get() + 1 && value(c, "deesser", "listen") == Some(1.0) && shown.as_deref() == Some("On"))
                .then_some(())
                .ok_or(format!("history {} -> {n}, listen {:?}, latch {shown:?}", hl2.get(), value(c, "deesser", "listen")))
        }),
    ));
    steps
}

fn index_of(id: &str, param: &str) -> i32 {
    dstudio_core::builtin::find(id).and_then(|def| def.index_of(param)).map_or(-1, |i| i as i32)
}

/// Delay, Echo, Reverb, Pitch Shifter and Grains: added from the Built-in list, then
/// their latches, view switches and custom fields driven through the panel entry points.
fn time_fx_steps() -> Vec<Step> {
    use Step::*;
    const IDS: [&str; 5] = ["delay", "echo", "reverb", "pitch-shifter", "grains"];
    thread_local! {
        /// History length at the last mark (A) and a second mark inside one step (B).
        static A: Cell<usize> = const { Cell::new(0) };
        static B: Cell<usize> = const { Cell::new(0) };
    }
    fn mark(m: &'static std::thread::LocalKey<Cell<usize>>, c: &Controller) {
        m.with(|m| m.set(c.doc.history_labels().len()));
    }
    fn steps(m: &'static std::thread::LocalKey<Cell<usize>>, c: &Controller) -> usize {
        c.doc.history_labels().len() - m.with(|m| m.get())
    }
    fn id_of(c: &Controller, id: &str) -> Option<dstudio_core::DeviceId> {
        device_of(c, id).map(|d| d.id)
    }
    vec![
        Do("time fx: Escape (close menus)", Box::new(|a| key(a, Key::Escape, false, false))),
        DoC(
            "time fx: select the audio track, clear its chain, Add device › Built-in",
            Box::new(|a, ctl| {
                let i = track_of(&ctl.borrow(), TrackKind::Audio).unwrap_or(0);
                click_track_header(a, ctl, i);
                if let Ok(mut c) = ctl.try_borrow_mut() {
                    let ids: Vec<dstudio_core::DeviceId> = c.project().tracks[i].devices.iter().map(|d| d.id).collect();
                    for d in ids {
                        c.execute(dstudio_core::Command::RemoveDevice { device: d });
                    }
                    c.update_ui(a);
                }
                let add_x = add_card_x(a, ctl);
                click(a, bottom_x() + add_x + 60.0, bottom_y() + 34.5 + 56.5 + 11.0);
            }),
        ),
        Check(
            "time fx: the Built-in list offers Delay, Echo, Reverb, Pitch Shifter and Grains",
            Box::new(|_, a| {
                let items = devmenu_labels(a);
                let all = ["Delay", "Echo", "Reverb", "Pitch Shifter", "Grains"].iter().all(|n| items.iter().any(|r| r.0 == *n && r.2));
                all.then_some(()).ok_or(format!("items {items:?}"))
            }),
        ),
        DoC(
            "time fx: pick Delay, add the other four",
            Box::new(|a, ctl| {
                mark(&A, &ctl.borrow());
                if let Some(i) = devmenu_labels(a).iter().position(|r| r.0 == "Delay") {
                    let (x, y) = devmenu_row(a, i);
                    click(a, x, y);
                }
                let mut c = ctl.borrow_mut();
                let Some(t) = track_of(&c, TrackKind::Audio).map(|i| c.project().tracks[i].id) else { return };
                for id in &IDS[1..] {
                    c.add_item_to_track(&crate::devices::Item::Builtin(dstudio_core::builtin::find(id).unwrap()), t, None);
                }
            }),
        ),
        Check(
            "time fx: five cards at their defaults, reference widths, each with its field, one undo step each",
            Box::new(|c, a| {
                let n = steps(&A, c);
                for (id, name, w) in [
                    ("delay", "Delay", 615.0),
                    ("echo", "Echo", 521.0),
                    ("reverb", "Reverb", 543.0),
                    ("pitch-shifter", "Pitch Shifter", 332.0),
                    ("grains", "Grains", 532.0),
                ] {
                    let cd = card_named(a, name).ok_or(format!("no {name} card"))?;
                    if !cd.builtin
                        || !cd.canvas.visible
                        || slint::Model::row_count(&cd.canvas.paths) == 0
                        || !approx(cd.width, w * crate::device_panel::Z, 0.01)
                    {
                        return Err(format!("{name}: builtin {} field {} width {}", cd.builtin, cd.canvas.visible, cd.width));
                    }
                    let d = device_of(c, id).ok_or(format!("no {id}"))?;
                    let def = dstudio_core::builtin::def_of(d).unwrap();
                    if def.params.iter().any(|p| def.value(d, p) != p.default) {
                        return Err(format!("{id} not at defaults"));
                    }
                }
                (n == 5).then_some(()).ok_or(format!("history +{n}"))
            }),
        ),
        DoC(
            "time fx: Delay Link latch off",
            Box::new(|_, ctl| {
                mark(&A, &ctl.borrow());
                let Some(d) = id_of(&ctl.borrow(), "delay") else { return };
                ctl.borrow_mut().panel_latch(d, index_of("delay", "link"), 0);
            }),
        ),
        Check(
            "time fx: Link off, Note L / Note R named, Note R live; one undo step",
            Box::new(|c, a| {
                let cd = card_named(a, "Delay").ok_or("no card")?;
                let knobs: Vec<_> = (0..slint::Model::row_count(&cd.knobs)).filter_map(|i| slint::Model::row_data(&cd.knobs, i)).collect();
                let ok = value_of(c, "delay", "link") == Some(0.0)
                    && knobs.iter().any(|k| k.label == "Note L")
                    && knobs.iter().any(|k| k.label == "Note R" && !k.inert)
                    && steps(&A, c) == 1;
                ok.then_some(()).ok_or(format!(
                    "link {:?} knobs {:?}",
                    value_of(c, "delay", "link"),
                    knobs.iter().map(|k| (k.label.to_string(), k.inert)).collect::<Vec<_>>()
                ))
            }),
        ),
        DoC(
            "time fx: drag the Delay echoes field 30 px down",
            Box::new(|_, ctl| {
                mark(&A, &ctl.borrow());
                let Some(d) = id_of(&ctl.borrow(), "delay") else { return };
                let mut c = ctl.borrow_mut();
                c.panel_field_pointer(d, 0, 60.0, 40.0, false, false);
                for i in 1..=30 {
                    c.panel_field_pointer(d, 1, 60.0, 40.0 + i as f32, false, false);
                }
                c.panel_field_pointer(d, 2, 60.0, 70.0, false, false);
            }),
        ),
        Check(
            "time fx: the echoes drag lowers Feedback, one undo step",
            Box::new(|c, _| {
                let fb = value_of(c, "delay", "feedback").unwrap_or(f32::NAN);
                (fb < 50.0 && fb > 0.0 && steps(&A, c) == 1).then_some(()).ok_or(format!("feedback {fb} history +{}", steps(&A, c)))
            }),
        ),
        DoC(
            "time fx: Delay view EQ",
            Box::new(|_, ctl| {
                let Some(d) = id_of(&ctl.borrow(), "delay") else { return };
                ctl.borrow_mut().card_view_select(d, 1);
            }),
        ),
        Check(
            "time fx: the EQ view is selected and drawn (UI state, no undo step)",
            Box::new(|c, a| {
                let cd = card_named(a, "Delay").ok_or("no card")?;
                let view = (0..slint::Model::row_count(&cd.picks))
                    .filter_map(|i| slint::Model::row_data(&cd.picks, i))
                    .find(|p| p.kind == 7)
                    .map(|p| p.selected);
                (view == Some(1) && slint::Model::row_count(&cd.canvas.grips) == 1 && steps(&A, c) == 1)
                    .then_some(())
                    .ok_or(format!("view {view:?}"))
            }),
        ),
        DoC(
            "time fx: hold the Echo Throw latch",
            Box::new(|_, ctl| {
                mark(&A, &ctl.borrow());
                let Some(d) = id_of(&ctl.borrow(), "echo") else { return };
                ctl.borrow_mut().panel_latch(d, index_of("echo", "throw"), 1);
            }),
        ),
        Check(
            "time fx: Throw shows on while held, live only (model off, no undo step)",
            Box::new(|c, a| {
                let cd = card_named(a, "Echo").ok_or("no card")?;
                let on = (0..slint::Model::row_count(&cd.knobs))
                    .filter_map(|i| slint::Model::row_data(&cd.knobs, i))
                    .any(|k| k.label == "Throw" && k.latch && k.on);
                (on && value_of(c, "echo", "throw") == Some(0.0) && steps(&A, c) == 0).then_some(()).ok_or(format!("on {on}"))
            }),
        ),
        DoC(
            "time fx: release Throw, then drag the first Echo head right",
            Box::new(|_, ctl| {
                let Some(d) = id_of(&ctl.borrow(), "echo") else { return };
                ctl.borrow_mut().panel_latch(d, index_of("echo", "throw"), 2);
                mark(&A, &ctl.borrow());
                // Defaults: heads 2+3 of the tape loop; the first repeat of head 2 sits at 0.2 s of a 3 s span.
                let (w, x) = {
                    let c = ctl.borrow();
                    let def = dstudio_core::builtin::find("echo").unwrap();
                    let (_, _, w, _, _) = crate::panel_fields::field_rect(def, crate::device_panel::view_right(def));
                    let pic = c.echo_picture(device_of(&c, "echo").unwrap(), def);
                    let ms = pic.heads.iter().filter(|h| h.on).map(|h| h.ms).fold(f32::MAX, f32::min);
                    let (left, right) = (40.0 * crate::device_panel::Z, w - 6.0 * crate::device_panel::Z);
                    (w, left + ms / 1000.0 / 3.0 * (right - left))
                };
                let _ = w;
                let mut c = ctl.borrow_mut();
                c.panel_field_pointer(d, 0, x, 80.0, false, false);
                c.panel_field_pointer(d, 1, x + 20.0, 80.0, false, false);
                c.panel_field_pointer(d, 2, x + 20.0, 80.0, false, false);
            }),
        ),
        Check(
            "time fx: Throw released; the head drag authors the heads (Heads, Heads Machine, pinned position) in one undo step",
            Box::new(|c, _| {
                let heads = value_of(c, "echo", "heads").unwrap_or(0.0);
                let machine = value_of(c, "echo", "heads_machine").unwrap_or(0.0);
                let ok = value_of(c, "echo", "throw") == Some(0.0) && heads >= 1.0 && machine == 1.0 && steps(&A, c) == 1;
                ok.then_some(()).ok_or(format!("heads {heads} machine {machine} history +{}", steps(&A, c)))
            }),
        ),
        DoC(
            "time fx: Reverb Mode › Shimmer",
            Box::new(|_, ctl| {
                mark(&A, &ctl.borrow());
                let Some(d) = id_of(&ctl.borrow(), "reverb") else { return };
                ctl.borrow_mut().panel_choice(d, index_of("reverb", "mode2"), 4);
            }),
        ),
        Check(
            "time fx: Shimmer dial row [Decay] [Pre-Delay, Mod Depth] [Shift 1, Shift 2, Shimmer] [Mix]",
            Box::new(|c, a| {
                let cd = card_named(a, "Reverb").ok_or("no card")?;
                let labels: Vec<String> = (0..slint::Model::row_count(&cd.knobs))
                    .filter_map(|i| slint::Model::row_data(&cd.knobs, i))
                    .map(|k| k.label.to_string())
                    .collect();
                let want = ["Decay", "Pre-Delay", "Mod Depth", "Shift 1", "Shift 2", "Shimmer", "Mix"];
                (labels == want && steps(&A, c) == 1).then_some(()).ok_or(format!("{labels:?}"))
            }),
        ),
        DoC(
            "time fx: drag the Reverb pre-delay line 20 px right",
            Box::new(|_, ctl| {
                mark(&A, &ctl.borrow());
                let Some(d) = id_of(&ctl.borrow(), "reverb") else { return };
                // Pre-delay 10 ms on a span of a few seconds: the line sits at the left edge of the plot.
                let x = 26.0 * crate::device_panel::Z + 1.0;
                let mut c = ctl.borrow_mut();
                c.panel_field_pointer(d, 0, x, 60.0, false, false);
                c.panel_field_pointer(d, 1, x + 20.0, 60.0, false, false);
                c.panel_field_pointer(d, 2, x + 20.0, 60.0, false, false);
            }),
        ),
        Check(
            "time fx: Pre-Delay grows, one undo step",
            Box::new(|c, _| {
                let v = value_of(c, "reverb", "predelay").unwrap_or(f32::NAN);
                (v > 10.0 && steps(&A, c) == 1).then_some(()).ok_or(format!("pre-delay {v}"))
            }),
        ),
        DoC(
            "time fx: press the Pitch Shifter field at +7 st",
            Box::new(|_, ctl| {
                mark(&A, &ctl.borrow());
                let Some(d) = id_of(&ctl.borrow(), "pitch-shifter") else { return };
                let def = dstudio_core::builtin::find("pitch-shifter").unwrap();
                let mut c = ctl.borrow_mut();
                let (_, _, w, _, _) = crate::panel_fields::field_rect(def, crate::device_panel::view_right(def));
                let x = (7.0 + 24.5) / 49.0 * w;
                c.panel_field_pointer(d, 0, x - 10.0, 40.0, false, false);
                c.panel_field_pointer(d, 1, x, 40.0, false, false);
                c.panel_field_pointer(d, 2, x, 40.0, false, false);
            }),
        ),
        Check(
            "time fx: Semitones +7, the caption reads Fifth up, one undo step",
            Box::new(|c, a| {
                let cd = card_named(a, "Pitch Shifter").ok_or("no card")?;
                let st = value_of(c, "pitch-shifter", "semitones");
                (st == Some(7.0) && cd.canvas.readout == "Fifth up" && steps(&A, c) == 1)
                    .then_some(())
                    .ok_or(format!("semitones {st:?} caption {}", cd.canvas.readout))
            }),
        ),
        DoC(
            "time fx: Grains Freeze latch, then drag Position 30 px left and press Escape",
            Box::new(|a, ctl| {
                mark(&A, &ctl.borrow());
                let Some(d) = id_of(&ctl.borrow(), "grains") else { return };
                ctl.borrow_mut().panel_latch(d, index_of("grains", "freeze"), 0);
                mark(&B, &ctl.borrow());
                let def = dstudio_core::builtin::find("grains").unwrap();
                let x = {
                    let (_, _, w, _, _) = crate::panel_fields::field_rect(def, crate::device_panel::view_right(def));
                    let right = w - 5.0 * crate::device_panel::Z;
                    // Frozen defaults: reach 0.25 s on a 0.5 s span.
                    right - 0.5 * (right - 4.0 * crate::device_panel::Z)
                };
                ctl.borrow_mut().panel_field_pointer(d, 0, x, 50.0, false, false);
                ctl.borrow_mut().panel_field_pointer(d, 1, x - 30.0, 50.0, false, false);
                // The press focuses the field in the UI; here the pointer is synthetic, so run the Escape action.
                let _ = a;
                crate::run_action(ctl, "view.escape");
                ctl.borrow_mut().panel_field_pointer(d, 2, x - 30.0, 50.0, false, false);
            }),
        ),
        Check(
            "time fx: Freeze on (one undo step); Escape cancels the Position drag (250 ms, no undo step)",
            Box::new(|c, _| {
                let pos = value_of(c, "grains", "position").unwrap_or(f32::NAN);
                let ok = value_of(c, "grains", "freeze") == Some(1.0) && pos == 250.0 && steps(&A, c) == 1 && steps(&B, c) == 0;
                ok.then_some(()).ok_or(format!("freeze {:?} position {pos}", value_of(c, "grains", "freeze")))
            }),
        ),
        DoC(
            "time fx: remove the five devices",
            Box::new(|_, ctl| {
                let mut c = ctl.borrow_mut();
                let ids: Vec<dstudio_core::DeviceId> = IDS.iter().filter_map(|id| device_of(&c, id).map(|d| d.id)).collect();
                for d in ids {
                    c.execute(dstudio_core::Command::RemoveDevice { device: d });
                }
            }),
        ),
        Check(
            "time fx: the audio track's chain is empty again",
            Box::new(|c, _| {
                let n = track_of(c, TrackKind::Audio).map_or(usize::MAX, |t| c.project().tracks[t].devices.len());
                (n == 0).then_some(()).ok_or(format!("{n} devices left"))
            }),
        ),
    ]
}

/// The devices section drives Apple's Audio Units (AULowpass, AUDelay,
/// DLSMusicDevice). Elsewhere its actions are not performed and its checks
/// are reported as skipped (counted like the other guarded skips).
fn platform_device_steps() -> Vec<Step> {
    if cfg!(target_os = "macos") {
        return device_steps();
    }
    device_steps()
        .into_iter()
        .filter_map(|s| match s {
            Step::Check(name, _) => {
                Some(Step::Check(name, Box::new(|_, _| Err("SKIP: Audio Units are macOS-only (this section uses Apple's AUs)".into()))))
            }
            Step::Do(..) | Step::DoC(..) => None,
        })
        .collect()
}

// ------------------------------------------------------------------ built-in samplers

fn inst_device(c: &Controller) -> Option<dstudio_core::Device> {
    let t = track_of(c, TrackKind::Instrument)?;
    c.project().tracks[t].instrument_plugin.clone()
}

fn sampler_card(a: &AppWindow) -> Option<crate::DeviceCard> {
    cards(a).into_iter().find(|c| c.sampler.visible)
}

/// Drum Rack, Pad Sampler and Sampler in the instrument slot: added from the
/// Built-in list, files loaded as drops, pads pressed, slices and a picture handle.
fn sampler_steps() -> Vec<Step> {
    use Step::*;
    let wav = run_dir().join("sampler.wav");
    let _ = crate::wavgen::write_wav(&wav, 48_000, 1, &crate::wavgen::sine_bursts(48_000));
    let hist = Rc::new(Cell::new(0usize));
    fn one(hc: &Rc<Cell<usize>>, c: &Controller, label: &str) -> Result<(), String> {
        let n = c.doc.history_labels().len();
        let was = hc.get();
        hc.set(n);
        (n == was + 1 && c.doc.undo_label() == Some(label))
            .then_some(())
            .ok_or(format!("history {was} -> {n}, last {:?} (want {label})", c.doc.undo_label()))
    }
    let (h0, h1, h2, h3, h4, h5, h6) = (hist.clone(), hist.clone(), hist.clone(), hist.clone(), hist.clone(), hist.clone(), hist.clone());
    let (w1, w2, w3, w4) = (wav.clone(), wav.clone(), wav.clone(), wav);
    let h4b = h4.clone();
    let (hp, hp2) = (hist.clone(), hist.clone());
    vec![
        Do("sampler: Escape (close menus)", Box::new(|a| key(a, Key::Escape, false, false))),
        DoC(
            "sampler: select the instrument track, Devices tab, Add device › Built-in",
            Box::new(move |a, ctl| {
                {
                    let mut c = ctl.borrow_mut();
                    let t = track_of(&c, TrackKind::Instrument).map(|i| c.project().tracks[i].id);
                    if let Some(t) = t {
                        c.select_track(t);
                    }
                    c.set_bottom_tab(crate::controller::BottomTab::Devices);
                    c.devmenu_dismiss();
                    h0.set(c.doc.history_labels().len());
                    // The chain is long here (Synth + plugin inserts): open Add device › Built-in directly.
                    c.add_category(0, 600.0, 520.0);
                    c.update_ui(a);
                }
            }),
        ),
        Check(
            "sampler: the Built-in list offers Sampler, Drum Rack and Pad Sampler",
            Box::new(|c, a| {
                let items = devmenu_labels(a);
                let has = |n: &str| items.iter().any(|r| r.0 == n && r.2);
                (a.global::<Studio>().get_devmenu_open() && has("Sampler") && has("Drum Rack") && has("Pad Sampler"))
                    .then_some(())
                    .ok_or(format!("items {items:?} selected {:?}", c.selected_track))
            }),
        ),
        Do(
            "sampler: pick Drum Rack",
            Box::new(|a| {
                if let Some(i) = devmenu_labels(a).iter().position(|r| r.0 == "Drum Rack") {
                    let (x, y) = devmenu_row(a, i);
                    click(a, x, y);
                }
            }),
        ),
        Check(
            "sampler: Drum Rack is the track's instrument (one undo step); card has 16 pads, 5 knobs, an empty well; engine instance live (48 readouts)",
            Box::new(move |c, a| {
                one(&h1, c, "Load Drum Rack")?;
                let d = inst_device(c).ok_or("no instrument")?;
                let cd = sampler_card(a).ok_or("no sampler card")?;
                let s = &cd.sampler;
                let mut r = Vec::new();
                c.engine.builtin_readouts(d.id.0, &mut r).map_err(|e| e.to_string())?;
                let ok = d.plugin.id == "drum-rack"
                    && s.kind == 2
                    && slint::Model::row_count(&s.pads) == 16
                    && slint::Model::row_count(&s.knobs) == 5
                    && !s.loaded
                    && r.len() == 48;
                ok.then_some(()).ok_or(format!(
                    "id {} kind {} pads {} knobs {} loaded {} readouts {}",
                    d.plugin.id,
                    s.kind,
                    slint::Model::row_count(&s.pads),
                    slint::Model::row_count(&s.knobs),
                    s.loaded,
                    r.len()
                ))
            }),
        ),
        DoC("sampler: drop a WAV on the window (pad 1)", Box::new(move |_, ctl| ctl.borrow_mut().drop_files(vec![w1.clone()]))),
        Check(
            "sampler: the file is pad 1's first sound (asset + sample, one undo step); the pad and well show it",
            Box::new(move |c, a| {
                one(&h2, c, "Load Sample")?;
                let d = inst_device(c).ok_or("no instrument")?;
                let cd = sampler_card(a).ok_or("no card")?;
                let pad = slint::Model::row_data(&cd.sampler.pads, 0).ok_or("no pad")?;
                let ok = d.sample(0).is_some()
                    && pad.filled
                    && cd.sampler.loaded
                    && !cd.sampler.wave.is_empty()
                    && cd.sampler.layer_text == "1/1";
                ok.then_some(()).ok_or(format!(
                    "sample {:?} pad {} well {} layer {}",
                    d.sample(0).map(|s| &s.name),
                    pad.filled,
                    cd.sampler.loaded,
                    cd.sampler.layer_text
                ))
            }),
        ),
        DoC("sampler: drop it again (second sound of pad 1)", Box::new(move |_, ctl| ctl.borrow_mut().drop_files(vec![w2.clone()]))),
        Check(
            "sampler: pad 1 holds two alternating sounds (slots 0 and 1); the panel shows sound 2/2",
            Box::new(move |c, a| {
                one(&h3, c, "Load Sample")?;
                let d = inst_device(c).ok_or("no instrument")?;
                let cd = sampler_card(a).ok_or("no card")?;
                (d.sample(1).is_some() && cd.sampler.layer_text == "2/2").then_some(()).ok_or(format!(
                    "slot1 {} layer {}",
                    d.sample(1).is_some(),
                    cd.sampler.layer_text
                ))
            }),
        ),
        Do(
            "sampler: press the D1 pad",
            Box::new(|a| {
                if let Some(cd) = sampler_card(a)
                    && let Some(p) = slint::Model::row_data(&cd.sampler.pads, 2)
                {
                    let (x, y) = card_xy(&cd, p.x + p.w / 2.0, p.y + p.h / 2.0);
                    click(a, x, y);
                }
            }),
        ),
        Check(
            "sampler: D1 is selected (UI only, no undo step): the sub-card shows Pad 3, Mapped to D1, an empty well, and edits p3_ parameters",
            Box::new(|c, a| {
                let cd = sampler_card(a).ok_or("no card")?;
                let s = &cd.sampler;
                let def = dstudio_core::builtin::find("drum-rack").unwrap();
                let k = slint::Model::row_data(&s.knobs, 0).ok_or("no knob")?;
                let ok = s.layer_name == "Pad 3"
                    && s.mapped == "Mapped to D1"
                    && !s.loaded
                    && def.params[k.param as usize].id == "p3_gain"
                    && c.doc.undo_label() == Some("Load Sample");
                ok.then_some(())
                    .ok_or(format!("{} / {} / loaded {} / knob {}", s.layer_name, s.mapped, s.loaded, def.params[k.param as usize].id))
            }),
        ),
        DoC(
            "sampler: load the Pad Sampler as the instrument and drop the WAV on it",
            Box::new(move |_, ctl| {
                let mut c = ctl.borrow_mut();
                let t = track_of(&c, TrackKind::Instrument).map(|i| c.project().tracks[i].id);
                if let Some(t) = t {
                    c.add_item_to_track(&crate::devices::Item::Builtin(dstudio_core::builtin::find("pad-sampler").unwrap()), t, None);
                }
                hp.set(c.doc.history_labels().len());
                c.drop_files(vec![w4.clone()]);
            }),
        ),
        Check(
            "sampler: the Pad Sampler holds the file as sound 1/1, answers every key, and shows its 5 knobs",
            Box::new(move |c, a| {
                one(&hp2, c, "Load Sample")?;
                let d = inst_device(c).ok_or("no instrument")?;
                let cd = sampler_card(a).ok_or("no card")?;
                let s = &cd.sampler;
                let ok = d.plugin.id == "pad-sampler"
                    && d.sample(0).is_some()
                    && s.kind == 1
                    && s.loaded
                    && s.layer_text == "1/1"
                    && s.mapped == "Mapped to all keys"
                    && slint::Model::row_count(&s.knobs) == 5;
                ok.then_some(())
                    .ok_or(format!("id {} kind {} loaded {} layer {} mapped {}", d.plugin.id, s.kind, s.loaded, s.layer_text, s.mapped))
            }),
        ),
        DoC(
            "sampler: load the Sampler as the instrument and drop the WAV on it",
            Box::new(move |_, ctl| {
                let mut c = ctl.borrow_mut();
                let t = track_of(&c, TrackKind::Instrument).map(|i| c.project().tracks[i].id);
                if let Some(t) = t {
                    c.add_item_to_track(&crate::devices::Item::Builtin(dstudio_core::builtin::find("sampler-steam").unwrap()), t, None);
                }
                h4b.set(c.doc.history_labels().len());
                c.drop_files(vec![w3.clone()]);
            }),
        ),
        Check(
            "sampler: the Sampler plays the file (One Shot); tabs Sample..Mod; start/end read in seconds (0.00 s / 4.00 s)",
            Box::new(move |c, a| {
                one(&h4, c, "Load Sample")?;
                let d = inst_device(c).ok_or("no instrument")?;
                let cd = sampler_card(a).ok_or("no card")?;
                let s = &cd.sampler;
                let v: Vec<String> = slint::Model::iter(&s.knobs).map(|k| k.value.to_string()).collect();
                let ok = d.plugin.id == "sampler-steam"
                    && d.sample(0).is_some()
                    && s.loaded
                    && slint::Model::row_count(&s.tab_pick.choices) == 5
                    && v.first().map(String::as_str) == Some("0.00 s")
                    && v.get(1).map(String::as_str) == Some("4.00 s");
                ok.then_some(()).ok_or(format!("id {} loaded {} knobs {v:?}", d.plugin.id, s.loaded))
            }),
        ),
        DoC(
            "sampler: switch the mode to Slice",
            Box::new(|_, ctl| {
                let mut c = ctl.borrow_mut();
                if let Some(d) = inst_device(&c) {
                    let i = dstudio_core::builtin::find("sampler-steam").unwrap().index_of("mode").unwrap();
                    c.panel_choice(d.id, i as i32, 2);
                }
            }),
        ),
        Check(
            "sampler: Slice mode chops the sample by beat (quarter notes at 120 BPM: 8 slices of 4 s) in the same undo step",
            Box::new(move |c, _| {
                one(&h5, c, "Device Parameter Change")?;
                let d = inst_device(c).ok_or("no instrument")?;
                let n = d.sample(0).map_or(0, |s| s.slices.len());
                (n == 8).then_some(()).ok_or(format!("{n} slices"))
            }),
        ),
        DoC(
            "sampler: Amp tab, drag the attack handle to the right",
            Box::new(|a, ctl| {
                let id = inst_device(&ctl.borrow()).map(|d| d.id);
                let Some(id) = id else { return };
                ctl.borrow_mut().sampler_tab(id, 3);
                if let Ok(mut c) = ctl.try_borrow_mut() {
                    c.update_ui(a);
                }
                let Some(h) = sampler_card(a).and_then(|cd| slint::Model::row_data(&cd.sampler.handles, 0)) else { return };
                let mut c = ctl.borrow_mut();
                c.sampler_handle(id, 0, 0, h.x, h.y);
                c.sampler_handle(id, 0, 1, h.x + 40.0, h.y);
                c.sampler_handle(id, 0, 2, h.x + 40.0, h.y);
            }),
        ),
        Check(
            "sampler: the attack handle drag is one undo step (Amp Envelope) and lengthens the attack",
            Box::new(move |c, _| {
                one(&h6, c, "Amp Envelope")?;
                let d = inst_device(c).ok_or("no instrument")?;
                let def = dstudio_core::builtin::def_of(&d).ok_or("def")?;
                let v = def.value(&d, def.param("ampAttack").unwrap());
                (v > 2.0).then_some(()).ok_or(format!("attack {v} ms"))
            }),
        ),
    ]
}

fn named_builtin<'a>(c: &'a Controller, id: &str) -> Option<&'a dstudio_core::Device> {
    let t = track_of(c, TrackKind::Audio)?;
    c.project().tracks[t].devices.iter().find(|d| dstudio_core::builtin::def_of(d).is_some_and(|def| def.id == id))
}

fn named_value(c: &Controller, id: &str, param: &str) -> Option<f32> {
    let d = named_builtin(c, id)?;
    let def = dstudio_core::builtin::def_of(d)?;
    Some(def.value(d, def.param(param)?))
}

/// The modulation devices: each one added from Add device › Built-in, checked
/// live in the engine with its generated field card, then removed; plus the
/// Tremolo field drag (one undo step for both axes), the Sync swap of the Rate
/// knob, and Character Mod's circuit switch moving untouched artifacts.
fn modulation_device_steps() -> Vec<Step> {
    use Step::*;
    const DEVICES: [(&str, &str, usize); 7] = [
        ("tremolo", "Tremolo", 4),
        ("autopanner", "Auto-Panner", 4),
        ("chorus", "Chorus", 5),
        ("flanger", "Flanger", 5),
        ("phaser", "Phaser", 5),
        ("ring-mod", "Ring Mod", 5),
        ("character-mod", "Character Mod", 5),
    ];
    let hist = Rc::new(Cell::new(0usize));
    fn one(hc: &Rc<Cell<usize>>, c: &Controller, label: &str) -> Result<(), String> {
        let n = c.doc.history_labels().len();
        let was = hc.get();
        hc.set(n);
        (n == was + 1 && c.doc.undo_label() == Some(label))
            .then_some(())
            .ok_or(format!("history {was} -> {n}, last {:?} (want {label})", c.doc.undo_label()))
    }
    let id_of = |ctl: &Ctl, id: &str| named_builtin(&ctl.borrow(), id).map(|d| d.id.0 as i32);
    let refresh = |a: &AppWindow, ctl: &Ctl| {
        if let Ok(mut c) = ctl.try_borrow_mut() {
            c.update_ui(a);
        }
    };
    let mut steps = vec![DoC(
        "modulation: remove the Compressor",
        Box::new(move |a, ctl| {
            if let Some(id) = id_of(ctl, "compressor") {
                a.global::<Studio>().invoke_device_remove(id);
            }
            refresh(a, ctl);
        }),
    )];
    for (id, name, knobs) in DEVICES {
        let (h0, h1) = (hist.clone(), hist.clone());
        steps.push(DoC(
            leak(format!("modulation: Add device › Built-in › {name}")),
            Box::new(move |a, ctl| {
                h0.set(ctl.borrow().doc.history_labels().len());
                refresh(a, ctl);
                let add_x = add_card_x(a, ctl);
                click(a, bottom_x() + add_x + 60.0, bottom_y() + 34.5 + 56.5 + 11.0);
                refresh(a, ctl);
                if let Some(i) = devmenu_labels(a).iter().position(|r| r.0 == name) {
                    let (x, y) = devmenu_row(a, i);
                    click(a, x, y);
                }
            }),
        ));
        steps.push(Check(
            leak(format!("{name}: added at its defaults in one step; field card with {knobs} knobs; live in the engine (LFO readout)")),
            Box::new(move |c, a| {
                one(&h1, c, &format!("Add {name}"))?;
                let d = named_builtin(c, id).ok_or("not added")?;
                let def = dstudio_core::builtin::def_of(d).ok_or("no def")?;
                let cd = card_named(a, name).ok_or("no card")?;
                let mut r = Vec::new();
                c.engine.builtin_readouts(d.id.0, &mut r).map_err(|e| e.to_string())?;
                let errors = c.engine.plugin_load_errors();
                let ok = cd.field.visible
                    && !cd.field.main.is_empty()
                    && slint::Model::row_count(&cd.knobs) == knobs
                    && approx(cd.width, def.layout.width() * crate::device_panel::Z, 0.01)
                    && r.len() == 1
                    && !errors.iter().any(|(e, _)| *e == d.id);
                ok.then_some(()).ok_or(format!(
                    "field {} knobs {} width {} readouts {r:?} errors {errors:?}",
                    cd.field.visible,
                    slint::Model::row_count(&cd.knobs),
                    cd.width
                ))
            }),
        ));
        match id {
            "tremolo" => {
                let (h2, h3) = (hist.clone(), hist.clone());
                steps.push(DoC(
                    "tremolo: drag the field 40 px right and 30 px up",
                    Box::new(|a, _| {
                        if let Some(cd) = card_named(a, "Tremolo") {
                            let f = &cd.field;
                            let (x, y) = card_xy(&cd, f.x + f.plot_x + f.plot_w / 2.0, f.y + f.plot_y + f.plot_h / 2.0);
                            drag(a, x, y, x + 40.0, y - 30.0);
                        }
                    }),
                ));
                steps.push(Check(
                    "tremolo: Rate and Depth both moved, ONE undo step for the drag",
                    Box::new(move |c, _| {
                        one(&h2, c, "Device Parameter Change")?;
                        let (r, d) = (named_value(c, "tremolo", "rate").unwrap_or(0.0), named_value(c, "tremolo", "depth").unwrap_or(0.0));
                        (r > 5.5 && d > 75.0).then_some(()).ok_or(format!("rate {r} depth {d}"))
                    }),
                ));
                steps.push(Do("tremolo: Cmd+Z", Box::new(|a| key(a, "z", true, false))));
                steps.push(Check(
                    "tremolo: one undo restores Rate 5 Hz and Depth 70%",
                    Box::new(move |c, _| {
                        h3.set(c.doc.history_labels().len());
                        let (r, d) = (named_value(c, "tremolo", "rate").unwrap_or(0.0), named_value(c, "tremolo", "depth").unwrap_or(0.0));
                        (r == 5.0 && d == 70.0).then_some(()).ok_or(format!("rate {r} depth {d}"))
                    }),
                ));
                let h4 = hist.clone();
                steps.push(DoC(
                    "tremolo: Timebase › Sync",
                    Box::new(move |a, ctl| {
                        let Some(id) = id_of(ctl, "tremolo") else { return };
                        let p = dstudio_core::builtin::find("tremolo").and_then(|d| d.index_of("sync")).map_or(-1, |i| i as i32);
                        a.global::<Studio>().invoke_panel_choice(id, p, 1);
                        refresh(a, ctl);
                    }),
                ));
                steps.push(Check(
                    "tremolo: Sync on swaps the Rate knob for Rate Sync (1/4)",
                    Box::new(move |c, a| {
                        one(&h4, c, "Device Parameter Change")?;
                        let k = card_named(a, "Tremolo").and_then(|cd| slint::Model::row_data(&cd.knobs, 0)).ok_or("no knob")?;
                        (k.value == "1/4" && k.label == "Rate").then_some(()).ok_or(format!("knob {} {}", k.label, k.value))
                    }),
                ));
            }
            "character-mod" => {
                let h2 = hist.clone();
                steps.push(DoC(
                    "character mod: Character dropdown › Phaser",
                    Box::new(move |a, ctl| {
                        let Some(id) = id_of(ctl, "character-mod") else { return };
                        let p = dstudio_core::builtin::find("character-mod").and_then(|d| d.index_of("model")).map_or(-1, |i| i as i32);
                        a.global::<Studio>().invoke_panel_menu(id, p, 600.0, 600.0);
                        refresh(a, ctl);
                        if let Some(i) = devmenu_labels(a).iter().position(|r| r.0 == "Phaser") {
                            let (x, y) = devmenu_row(a, i);
                            click(a, x, y);
                        }
                    }),
                ));
                steps.push(Check(
                    "character mod: Phaser chosen, untouched Hiss moved to 0 in the same undo step; Mode now lists Phaser modes",
                    Box::new(move |c, a| {
                        one(&h2, c, "Character Change, with Hiss moved to its own defaults")?;
                        let (m, hiss) = (named_value(c, "character-mod", "model"), named_value(c, "character-mod", "art_hiss"));
                        let mode = card_named(a, "Character Mod")
                            .and_then(|cd| {
                                (0..slint::Model::row_count(&cd.picks))
                                    .filter_map(|i| slint::Model::row_data(&cd.picks, i))
                                    .find(|p| p.label == "Mode")
                            })
                            .map(|p| p.value.to_string())
                            .unwrap_or_default();
                        (m == Some(7.0) && hiss == Some(0.0) && mode == "Four Plain")
                            .then_some(())
                            .ok_or(format!("model {m:?} hiss {hiss:?} mode {mode:?}"))
                    }),
                ));
            }
            _ => {}
        }
        let h5 = hist.clone();
        steps.push(DoC(
            leak(format!("modulation: remove {name}")),
            Box::new(move |a, ctl| {
                if let Some(i) = id_of(ctl, id) {
                    a.global::<Studio>().invoke_device_remove(i);
                }
                refresh(a, ctl);
                h5.set(ctl.borrow().doc.history_labels().len());
            }),
        ));
    }
    steps
}

/// Window point of a canvas-local point of a card's canvas view.
fn canvas_xy(cd: &crate::DeviceCard, x: f32, y: f32) -> (f32, f32) {
    card_xy(cd, cd.canvas.x + x, cd.canvas.y + cd.canvas.well_y + y)
}

/// Opens Add device › Built-in on the first audio track and picks \`name\`.
/// The Add device card's x in the bottom panel, after scrolling the chain until the whole card
/// is on screen (a narrow window or a long chain can push it past the right edge).
fn add_card_x(a: &AppWindow, ctl: &Ctl) -> f32 {
    let refresh = || {
        if let Ok(mut c) = ctl.try_borrow_mut() {
            c.update_ui(a);
        }
    };
    refresh();
    let over = a.global::<Studio>().get_add_device_x() + 182.5 + 5.0 - geo().bottom_w;
    if over > 0.0 {
        ctl.borrow_mut().chain_scroll_by(-over);
        refresh();
    }
    a.global::<Studio>().get_add_device_x()
}

fn add_builtin(a: &AppWindow, ctl: &Ctl, name: &str) {
    if let Ok(mut c) = ctl.try_borrow_mut() {
        c.update_ui(a);
    }
    let add_x = add_card_x(a, ctl);
    click(a, bottom_x() + add_x + 60.0, bottom_y() + 34.5 + 56.5 + 11.0);
    if let Ok(mut c) = ctl.try_borrow_mut() {
        c.update_ui(a);
    }
    if let Some(i) = devmenu_labels(a).iter().position(|r| r.0 == name) {
        let (x, y) = devmenu_row(a, i);
        click(a, x, y);
    }
}

/// Picks row \`label\` of the parameter menu of \`param\` (a device panel pill).
fn pick_param_menu(a: &AppWindow, ctl: &Ctl, device: &str, param: &str, label: &str) {
    let ids = {
        let c = ctl.borrow();
        named_builtin(&c, device)
            .and_then(|d| dstudio_core::builtin::def_of(d).and_then(|def| def.index_of(param)).map(|i| (d.id.0 as i32, i as i32)))
    };
    let Some((id, p)) = ids else { return };
    a.global::<Studio>().invoke_panel_menu(id, p, 600.0, 600.0);
    if let Ok(mut c) = ctl.try_borrow_mut() {
        c.update_ui(a);
    }
    if let Some(i) = devmenu_labels(a).iter().position(|r| r.0 == label) {
        let (x, y) = devmenu_row(a, i);
        click(a, x, y);
    }
}

/// Channel EQ, Character EQ and Auto Filter driven through their canvas views.
fn field_device_steps() -> Vec<Step> {
    use Step::*;
    let hist = Rc::new(Cell::new(0usize));
    let h = || hist.clone();
    fn one(hc: &Rc<Cell<usize>>, c: &Controller, label: &str) -> Result<(), String> {
        let n = c.doc.history_labels().len();
        let was = hc.get();
        hc.set(n);
        (n == was + 1 && c.doc.undo_label() == Some(label))
            .then_some(())
            .ok_or(format!("history {was} -> {n}, last {:?} (want {label})", c.doc.undo_label()))
    }
    fn live(c: &Controller, id: &str) -> Result<(), String> {
        let d = named_builtin(c, id).ok_or("no device")?.id;
        let mut r = Vec::new();
        c.engine.builtin_readouts(d.0, &mut r).map_err(|e| e.to_string())?;
        let errors = c.engine.plugin_load_errors();
        (!errors.iter().any(|(x, _)| *x == d)).then_some(()).ok_or(format!("load errors {errors:?}"))
    }
    let (h0, h1, h2, h3, h4, h5, h6, h7, h8, h9, h10) = (h(), h(), h(), h(), h(), h(), h(), h(), h(), h(), h());
    let (h1b, h4b, h8b) = (h1.clone(), h4.clone(), h8.clone());
    let (h5b, h7b, h10b) = (h(), h(), h());
    vec![
        Do("eq: Escape (close menus)", Box::new(|a| key(a, Key::Escape, false, false))),
        DoC(
            "eq: select the first audio track, clear its chain, add Channel EQ from Add device › Built-in",
            Box::new(move |a, ctl| {
                let i = track_of(&ctl.borrow(), TrackKind::Audio).unwrap_or(0);
                click_track_header(a, ctl, i);
                let ids: Vec<dstudio_core::DeviceId> = ctl.borrow().project().tracks[i].devices.iter().map(|d| d.id).collect();
                for id in ids {
                    ctl.borrow_mut().device_remove(id);
                }
                h0.set(ctl.borrow().doc.history_labels().len());
                add_builtin(a, ctl, "Channel EQ");
            }),
        ),
        Check(
            "eq: Channel EQ added in one undo step; card has the EQ field, 8 band tabs, 3 cells, the Output slider, 528 pt wide",
            Box::new(move |c, a| {
                one(&h1b, c, "Add Channel EQ")?;
                live(c, "channeleq")?;
                let cd = card_named(a, "Channel EQ").ok_or("no card")?;
                let ok = cd.canvas.visible
                    && cd.eq.visible
                    && slint::Model::row_count(&cd.eq.tabs) == 8
                    && slint::Model::row_count(&cd.eq.cells) == 3
                    && slint::Model::row_count(&cd.sliders) == 1
                    && approx(cd.width, 528.0 * crate::device_panel::Z, 0.01)
                    && cd.eq.selected == 2;
                ok.then_some(()).ok_or(format!(
                    "field {} eq {} tabs {} width {}",
                    cd.canvas.visible,
                    cd.eq.visible,
                    slint::Model::row_count(&cd.eq.tabs),
                    cd.width
                ))
            }),
        ),
        DoC(
            "eq: drag the band 3 node up 40 px",
            Box::new(|a, _| {
                let Some(cd) = card_named(a, "Channel EQ") else { return };
                // The selected node is drawn as a ring dot (ring > 0) then the node dot.
                let Some(dot) = (0..slint::Model::row_count(&cd.canvas.dots))
                    .filter_map(|i| slint::Model::row_data(&cd.canvas.dots, i))
                    .find(|d| d.ring > 0.0)
                else {
                    return;
                };
                let (x, y) = canvas_xy(&cd, dot.x, dot.y);
                drag(a, x, y, x, y - 40.0);
            }),
        ),
        Check(
            "eq: band 3 gain rises, frequency stays at 100 Hz, ONE undo step",
            Box::new(move |c, _| {
                one(&h2, c, "Device Parameter Change")?;
                let (g, f) = (
                    named_value(c, "channeleq", "band2_gain2").unwrap_or(f32::NAN),
                    named_value(c, "channeleq", "band2_freq").unwrap_or(f32::NAN),
                );
                (g > 3.0 && approx(f, 100.0, 1.0)).then_some(()).ok_or(format!("gain {g} freq {f}"))
            }),
        ),
        DoC(
            "eq: click band tab 5, then the Bell glyph's neighbour (Low Shelf) in the type picker",
            Box::new(|a, ctl| {
                let Some(id) = named_builtin(&ctl.borrow(), "channeleq").map(|d| d.id.0 as i32) else { return };
                a.global::<Studio>().invoke_panel_eq(id, 0, 4);
                if let Ok(mut c) = ctl.try_borrow_mut() {
                    c.update_ui(a);
                }
                let Some(cd) = card_named(a, "Channel EQ") else { return };
                if let Some(g) = (0..slint::Model::row_count(&cd.eq.glyphs))
                    .filter_map(|i| slint::Model::row_data(&cd.eq.glyphs, i))
                    .find(|g| g.name == "Low Shelf")
                {
                    a.global::<Studio>().invoke_panel_eq(id, 2, g.choice);
                }
            }),
        ),
        Check(
            "eq: band 5 is selected and becomes a Low Shelf in one undo step",
            Box::new(move |c, a| {
                one(&h3, c, "Device Parameter Change")?;
                let cd = card_named(a, "Channel EQ").ok_or("no card")?;
                let t = named_value(c, "channeleq", "band4_type").unwrap_or(f32::NAN);
                (cd.eq.selected == 4 && t == 1.0).then_some(()).ok_or(format!("selected {} type {t}", cd.eq.selected))
            }),
        ),
        DoC(
            "eq: double-click the band 3 node (parks it)",
            Box::new(move |a, ctl| {
                h4.set(ctl.borrow().doc.history_labels().len());
                let Some(id) = named_builtin(&ctl.borrow(), "channeleq").map(|d| d.id.0 as i32) else { return };
                a.global::<Studio>().invoke_panel_eq(id, 0, 2);
                if let Ok(mut c) = ctl.try_borrow_mut() {
                    c.update_ui(a);
                }
                let Some(cd) = card_named(a, "Channel EQ") else { return };
                let Some(dot) = (0..slint::Model::row_count(&cd.canvas.dots))
                    .filter_map(|i| slint::Model::row_data(&cd.canvas.dots, i))
                    .find(|d| d.ring > 0.0)
                else {
                    return;
                };
                let (x, y) = canvas_xy(&cd, dot.x, dot.y);
                double_click(a, x, y);
            }),
        ),
        Check(
            "eq: band 3 is off, its gain kept",
            Box::new(move |c, _| {
                let (on, g) = (
                    named_value(c, "channeleq", "band2_enabled").unwrap_or(f32::NAN),
                    named_value(c, "channeleq", "band2_gain2").unwrap_or(f32::NAN),
                );
                let n = c.doc.history_labels().len();
                (on == 0.0 && g > 3.0 && n > h4b.get()).then_some(()).ok_or(format!("on {on} gain {g}"))
            }),
        ),
        DoC(
            "eq: replace it with Character EQ (Add device › Built-in)",
            Box::new(move |a, ctl| {
                let old = named_builtin(&ctl.borrow(), "channeleq").map(|d| d.id);
                if let Some(id) = old {
                    ctl.borrow_mut().device_remove(id);
                }
                h5.set(ctl.borrow().doc.history_labels().len());
                add_builtin(a, ctl, "Character EQ");
            }),
        ),
        Check(
            "eq: Character EQ added; 12 band tabs, a Bank menu, no band enabled",
            Box::new(move |c, a| {
                one(&h5b, c, "Add Character EQ")?;
                live(c, "character-eq")?;
                let cd = card_named(a, "Character EQ").ok_or("no card")?;
                let ok = slint::Model::row_count(&cd.eq.tabs) == 12
                    && cd.eq.bank_param >= 0
                    && !cd.eq.on
                    && approx(cd.width, 575.0 * crate::device_panel::Z, 0.01);
                ok.then_some(()).ok_or(format!("tabs {} bank {} on {}", slint::Model::row_count(&cd.eq.tabs), cd.eq.bank_param, cd.eq.on))
            }),
        ),
        DoC(
            "eq: click empty field space at 2 kHz, +6 dB",
            Box::new(|a, _| {
                let Some(cd) = card_named(a, "Character EQ") else { return };
                let (w, h) = (cd.canvas.w, cd.canvas.h);
                let x = w * (100f32).log10() / 1000f32.log10();
                let y = (24.0 - 6.0) / 48.0 * h;
                let (x, y) = canvas_xy(&cd, x, y);
                click(a, x, y);
            }),
        ),
        Check(
            "eq: band 1 created there (on, about 2 kHz, about +6 dB) in one undo step",
            Box::new(move |c, _| {
                one(&h6, c, "Device Parameter Change")?;
                let g = |p: &str| named_value(c, "character-eq", p).unwrap_or(f32::NAN);
                let (on, f, gain) = (g("band1_enabled"), g("band1_freq"), g("band1_gain"));
                (on == 1.0 && (f / 2000.0 - 1.0).abs() < 0.05 && approx(gain, 6.0, 0.5))
                    .then_some(())
                    .ok_or(format!("on {on} freq {f} gain {gain}"))
            }),
        ),
        DoC(
            "eq: Bank menu › Passive Valve",
            Box::new(move |a, ctl| {
                h7.set(ctl.borrow().doc.history_labels().len());
                pick_param_menu(a, ctl, "character-eq", "bank", "Passive Valve");
            }),
        ),
        Check(
            "eq: the bank and the band's re-snapped frequency change in one undo step",
            Box::new(move |c, _| {
                one(&h7b, c, "Device Parameter Change")?;
                let bank = named_value(c, "character-eq", "bank").unwrap_or(f32::NAN);
                let f = named_value(c, "character-eq", "band1_freq").unwrap_or(f32::NAN);
                let detents = dstudio_core::builtin::eq::bank(1.0).detents;
                (bank == 1.0 && (detents.is_empty() || detents.contains(&f))).then_some(()).ok_or(format!("bank {bank} freq {f}"))
            }),
        ),
        DoC(
            "eq: replace it with Auto Filter (Add device › Built-in)",
            Box::new(move |a, ctl| {
                let old = named_builtin(&ctl.borrow(), "character-eq").map(|d| d.id);
                if let Some(id) = old {
                    ctl.borrow_mut().device_remove(id);
                }
                h8.set(ctl.borrow().doc.history_labels().len());
                add_builtin(a, ctl, "Auto Filter");
            }),
        ),
        Check(
            "eq: Auto Filter added; Response field, 4 picker pills, 7 knobs (Rate shows the free rate)",
            Box::new(move |c, a| {
                one(&h8b, c, "Add Auto Filter")?;
                live(c, "auto-filter")?;
                let cd = card_named(a, "Auto Filter").ok_or("no card")?;
                let knobs: Vec<String> = (0..slint::Model::row_count(&cd.knobs))
                    .filter_map(|i| slint::Model::row_data(&cd.knobs, i))
                    .map(|k| k.label.to_string())
                    .collect();
                let pills = (0..slint::Model::row_count(&cd.picks))
                    .filter_map(|i| slint::Model::row_data(&cd.picks, i))
                    .filter(|p| p.kind != 4)
                    .count();
                let ok = cd.canvas.visible
                    && cd.canvas.caption == "Response"
                    && !cd.eq.visible
                    && pills == 4
                    && knobs.len() == 7
                    && knobs[5] == "Rate";
                ok.then_some(()).ok_or(format!("canvas {} pills {pills} knobs {knobs:?}", cd.canvas.visible))
            }),
        ),
        DoC(
            "eq: drag the filter corner left 120 px and up 30 px",
            Box::new(|a, _| {
                let Some(cd) = card_named(a, "Auto Filter") else { return };
                let Some(dot) =
                    (0..slint::Model::row_count(&cd.canvas.dots)).filter_map(|i| slint::Model::row_data(&cd.canvas.dots, i)).next_back()
                else {
                    return;
                };
                let (x, y) = canvas_xy(&cd, dot.x, dot.y);
                drag(a, x, y, x - 120.0, y - 30.0);
            }),
        ),
        Check(
            "eq: frequency falls and resonance rises, in ONE undo step",
            Box::new(move |c, _| {
                one(&h9, c, "Device Parameter Change")?;
                let (f, r) = (
                    named_value(c, "auto-filter", "frequency").unwrap_or(f32::NAN),
                    named_value(c, "auto-filter", "resonance").unwrap_or(f32::NAN),
                );
                (f < 5000.0 && r > 0.1).then_some(()).ok_or(format!("freq {f} reso {r}"))
            }),
        ),
        DoC(
            "eq: Filter menu › DJ, then Filter menu › HP",
            Box::new(|a, ctl| {
                pick_param_menu(a, ctl, "auto-filter", "filter_type", "DJ");
            }),
        ),
        Check(
            "eq: DJ entry switches the mode on; the Filter pill reads DJ",
            Box::new(move |c, a| {
                let m = named_value(c, "auto-filter", "mode").unwrap_or(f32::NAN);
                let cd = card_named(a, "Auto Filter").ok_or("no card")?;
                let pill = slint::Model::row_data(&cd.picks, 0).map(|p| p.value.to_string()).unwrap_or_default();
                h10.set(c.doc.history_labels().len());
                (m == 1.0 && pill == "DJ").then_some(()).ok_or(format!("mode {m} pill {pill:?}"))
            }),
        ),
        DoC(
            "eq: Filter menu › HP",
            Box::new(|a, ctl| {
                pick_param_menu(a, ctl, "auto-filter", "filter_type", "HP");
            }),
        ),
        Check(
            "eq: HP sets the type and leaves DJ mode in one undo step",
            Box::new(move |c, _| {
                one(&h10b, c, "Device Parameter Change")?;
                let (t, m) = (
                    named_value(c, "auto-filter", "filter_type").unwrap_or(f32::NAN),
                    named_value(c, "auto-filter", "mode").unwrap_or(f32::NAN),
                );
                (t == 1.0 && m == 0.0).then_some(()).ok_or(format!("type {t} mode {m}"))
            }),
        ),
        DoC(
            "eq: remove this section's devices from the audio track",
            Box::new(|a, ctl| {
                key(a, Key::Escape, false, false);
                let i = track_of(&ctl.borrow(), TrackKind::Audio).unwrap_or(0);
                let ids: Vec<dstudio_core::DeviceId> = ctl.borrow().project().tracks[i].devices.iter().map(|d| d.id).collect();
                for id in ids {
                    ctl.borrow_mut().device_remove(id);
                }
                if let Ok(mut c) = ctl.try_borrow_mut() {
                    c.update_ui(a);
                }
            }),
        ),
        Check(
            "eq: the audio track's chain is empty again",
            Box::new(|c, _| {
                let n = track_of(c, TrackKind::Audio).map_or(0, |i| c.project().tracks[i].devices.len());
                (n == 0).then_some(()).ok_or(format!("{n} devices left"))
            }),
        ),
    ]
}

/// Window point at the centre of step-sequencer cell (row, step): editor at window (260, 452).
fn seq_xy(a: &AppWindow, row: u16, step: u16) -> (f32, f32) {
    let s = a.global::<Studio>();
    let (pitch, w, scroll) = (s.get_seq_col_pitch(), s.get_seq_cell_w(), s.get_seq_scroll_y());
    (
        pr_origin().0 + crate::stepseq::COL_X0 + step as f32 * pitch + w / 2.0,
        bottom_y() + 27.0 + crate::stepseq::ROW_Y0 + row as f32 * crate::stepseq::ROW_PITCH + 9.75 - scroll,
    )
}

fn seq_clip(c: &Controller) -> Option<dstudio_core::MidiClip> {
    c.pr_clip().filter(|m| m.pattern.is_some()).cloned()
}

fn seq_cells(c: &Controller) -> Vec<(u8, u16)> {
    seq_clip(c).and_then(|m| m.pattern).map(|p| p.cells.iter().map(|x| (x.key, x.step)).collect()).unwrap_or_default()
}

/// Step sequencer: a Drum Rack track makes pattern clips; cells, lanes, M / S, footer menus,
/// Double, the Piano roll tab and conversion, each one undo step. Restores the track's
/// instrument and removes its clip at the end.
fn step_sequencer_steps() -> Vec<Step> {
    use Step::*;
    let hist = Rc::new(Cell::new(0usize));
    let prev: Rc<RefCell<Option<dstudio_core::Device>>> = Rc::new(RefCell::new(None));
    fn one(hc: &Rc<Cell<usize>>, c: &Controller, label: &str) -> Result<(), String> {
        let n = c.doc.history_labels().len();
        let was = hc.get();
        hc.set(n);
        (n == was + 1 && c.doc.undo_label() == Some(label))
            .then_some(())
            .ok_or(format!("history {was} -> {n}, last {:?} (want {label})", c.doc.undo_label()))
    }
    let h = |_: u8| hist.clone();
    let (h0, h1, h2, h3, h4, h5, h6, h7, h8, h9) = (h(0), h(1), h(2), h(3), h(4), h(5), h(6), h(7), h(8), h(9));
    let (p0, p1) = (prev.clone(), prev);
    vec![
        Do("seq: Escape (close menus)", Box::new(|a| key(a, Key::Escape, false, false))),
        DoC(
            "seq: instrument track gets a Drum Rack; Cmd+Shift+M at bar 60 makes a pattern clip; select it",
            Box::new(move |a, ctl| {
                let mut c = ctl.borrow_mut();
                let Some(t) = track_of(&c, TrackKind::Instrument).map(|i| c.project().tracks[i].id) else { return };
                c.select_track(t);
                *p0.borrow_mut() = c.project().track(t).and_then(|t| t.instrument_plugin.clone());
                c.execute(dstudio_core::Command::SetInstrumentPlugin {
                    track: t,
                    device: Some(dstudio_core::builtin::drum_rack::DRUM_RACK.instantiate()),
                });
                c.seek_beats(236.0);
                h0.set(c.doc.history_labels().len());
                if let Some(id) = c.insert_midi_clip() {
                    c.select_clip(Some(id));
                }
                c.update_ui(a);
            }),
        ),
        Check(
            "seq: one-bar 'Pattern' clip of 16 1/16 steps on pads; Sequencer tab shown with 16 pad rows",
            Box::new(move |c, a| {
                one(&h1, c, "Insert MIDI Clip")?;
                let m = seq_clip(c).ok_or("no pattern clip open")?;
                let p = m.pattern.clone().unwrap_or_default();
                let s = a.global::<Studio>();
                let names: Vec<String> = slint::Model::iter(&s.get_seq_rows()).map(|r| r.name.to_string()).collect();
                let ok = m.name == "Pattern"
                    && m.length == 4 * PPQ
                    && p.steps == 16
                    && p.rows == dstudio_core::steps::StepRows::Pads
                    && s.get_seq_visible()
                    && s.get_seq_tabs()
                    && names.first().map(String::as_str) == Some("Pad 1")
                    && s.get_seq_row_count() == 16;
                ok.then_some(()).ok_or(format!(
                    "{} len {} steps {} rows {names:?} visible {}",
                    m.name,
                    m.length,
                    p.steps,
                    s.get_seq_visible()
                ))
            }),
        ),
        Do(
            "seq: click Pad 1 steps 1 and 5",
            Box::new(|a| {
                for s in [0, 4] {
                    let (x, y) = seq_xy(a, 0, s);
                    click(a, x, y);
                }
            }),
        ),
        Check(
            "seq: two cells, two notes (C1 at 0 and 1 beat), two undo steps 'Edit Step Sequencer'",
            Box::new(move |c, _| {
                let n = c.doc.history_labels().len();
                let ok = seq_cells(c) == vec![(36, 0), (36, 4)] && n == h2.get() + 2 && c.doc.undo_label() == Some("Edit Step Sequencer");
                let notes: Vec<(u8, i64)> = seq_clip(c).map(|m| m.notes.iter().map(|n| (n.pitch, n.start)).collect()).unwrap_or_default();
                h2.set(n);
                (ok && notes == vec![(36, 0), (36, PPQ)]).then_some(()).ok_or(format!("cells {:?} notes {notes:?}", seq_cells(c)))
            }),
        ),
        Do(
            "seq: drag across Pad 2 steps 1..4 (paint)",
            Box::new(|a| {
                let (x0, y) = seq_xy(a, 1, 0);
                let (x1, _) = seq_xy(a, 1, 3);
                drag(a, x0, y, x1, y);
            }),
        ),
        Check(
            "seq: Pad 2 painted on steps 1..4 in one undo step",
            Box::new(move |c, _| {
                one(&h3, c, "Edit Step Sequencer")?;
                let pad2: Vec<u16> = seq_cells(c).into_iter().filter(|x| x.0 == 37).map(|x| x.1).collect();
                (pad2 == vec![0, 1, 2, 3]).then_some(()).ok_or(format!("pad 2 {pad2:?}"))
            }),
        ),
        Do(
            "seq: Velocity tab, drag Pad 1 step 1 down 60 pt",
            Box::new(|a| {
                click(a, pr_origin().0 + 220.0, bottom_y() + 27.0 + 13.5);
                let (x, y) = seq_xy(a, 0, 0);
                drag(a, x, y, x, y + 60.0);
            }),
        ),
        Check(
            "seq: Velocity lane shown; Pad 1 step 1 softer (one undo step), its note follows",
            Box::new(move |c, a| {
                one(&h4, c, "Edit Step Sequencer")?;
                let p = seq_clip(c).and_then(|m| m.pattern).ok_or("no pattern")?;
                let v = p.cell(36, 0).map(|x| x.velocity).unwrap_or(0);
                let nv = seq_clip(c).and_then(|m| m.notes.iter().find(|n| n.start == 0 && n.pitch == 36).map(|n| n.velocity)).unwrap_or(0);
                (a.global::<Studio>().get_seq_lane() == 1 && (1..60).contains(&v) && nv == v)
                    .then_some(())
                    .ok_or(format!("lane {} velocity {v} note {nv}", a.global::<Studio>().get_seq_lane()))
            }),
        ),
        Do(
            "seq: Tie tab, drag Pad 1 step 5 two steps right",
            Box::new(|a| {
                click(a, pr_origin().0 + 404.0, bottom_y() + 27.0 + 13.5);
                let (x, y) = seq_xy(a, 0, 4);
                let (x2, _) = seq_xy(a, 0, 6);
                drag(a, x, y, x2, y);
            }),
        ),
        Check(
            "seq: Pad 1 step 5 ties two steps (note three steps long, less the gate gap)",
            Box::new(move |c, _| {
                one(&h5, c, "Edit Step Sequencer")?;
                let p = seq_clip(c).and_then(|m| m.pattern).ok_or("no pattern")?;
                let len = seq_clip(c).and_then(|m| m.notes.iter().find(|n| n.start == PPQ && n.pitch == 36).map(|n| n.length)).unwrap_or(0);
                (p.cell(36, 4).map(|x| x.tie) == Some(2) && len > 2 * PPQ / 4 && len <= 3 * PPQ / 4)
                    .then_some(())
                    .ok_or(format!("tie {:?} length {len}", p.cell(36, 4).map(|x| x.tie)))
            }),
        ),
        Do(
            "seq: click Pad 2's M",
            Box::new(|a| {
                let s = a.global::<Studio>();
                click(
                    a,
                    pr_origin().0 + 101.0,
                    bottom_y() + 27.0 + crate::stepseq::ROW_Y0 + crate::stepseq::ROW_PITCH + 9.75 - s.get_seq_scroll_y(),
                );
            }),
        ),
        Check(
            "seq: Pad 2 muted (one undo step): its notes stay, playback leaves them out",
            Box::new(move |c, a| {
                one(&h6, c, "Edit Step Sequencer")?;
                let m = seq_clip(c).ok_or("no pattern")?;
                let muted = slint::Model::row_data(&a.global::<Studio>().get_seq_rows(), 1).is_some_and(|r| r.muted && !r.audible);
                let ok = muted && m.notes.iter().any(|n| n.pitch == 37) && m.playing_notes().iter().all(|n| n.pitch != 37);
                ok.then_some(()).ok_or(format!("muted {muted}"))
            }),
        ),
        DoC(
            "seq: footer Length 32, Rate 1/8; Double",
            Box::new(|a, ctl| {
                {
                    let mut c = ctl.borrow_mut();
                    c.clip_menu_open(6);
                    c.clip_menu_pick(4);
                    c.clip_menu_open(7);
                    c.clip_menu_pick(3);
                    c.update_ui(a);
                }
                click(a, pr_origin().0 + 20.0, bottom_y() + 27.0 + 13.5);
            }),
        ),
        Check(
            "seq: 64 steps of 1/8 (three undo steps); the footer reads 64 / 1/8; Double is now disabled",
            Box::new(move |c, a| {
                let n = c.doc.history_labels().len();
                let was = h7.get();
                h7.set(n);
                let p = seq_clip(c).and_then(|m| m.pattern).ok_or("no pattern")?;
                let s = a.global::<Studio>();
                let ok = n == was + 3
                    && p.steps == 64
                    && p.rate == dstudio_core::steps::StepRate::Eighth
                    && s.get_seq_length_text() == "64"
                    && s.get_seq_rate_text() == "1/8"
                    && !s.get_seq_can_double();
                ok.then_some(()).ok_or(format!("history +{} steps {} rate {:?}", n - was, p.steps, p.rate))
            }),
        ),
        Do("seq: Piano roll tab", Box::new(|a| click(a, bottom_x() + 420.0, bottom_y() + 13.5))),
        Check(
            "seq: piano roll shown for the pattern clip",
            Box::new(|c, a| {
                let s = a.global::<Studio>();
                (!s.get_seq_visible() && s.get_seq_tabs() && s.get_pr_visible() && !c.seq_visible())
                    .then_some(())
                    .ok_or("sequencer still shown".into())
            }),
        ),
        DoC(
            "seq: convert the clip to plain MIDI",
            Box::new(|_, ctl| {
                let mut c = ctl.borrow_mut();
                if let Some(id) = c.pr_clip().map(|m| m.id) {
                    c.convert_clip(id);
                }
            }),
        ),
        Check(
            "seq: plain MIDI clip with the pattern's notes (one undo step); no sequencer tabs",
            Box::new(move |c, a| {
                one(&h8, c, "Convert to MIDI Clip")?;
                let m = c.pr_clip().ok_or("clip closed")?;
                (m.pattern.is_none() && !m.notes.is_empty() && !a.global::<Studio>().get_seq_tabs())
                    .then_some(())
                    .ok_or("still a pattern".into())
            }),
        ),
        DoC(
            "seq: stretch the plain clip to 20 beats (notes in the first bar only), convert to a pattern",
            Box::new(|a, ctl| {
                let mut c = ctl.borrow_mut();
                let Some(m) = c.pr_clip().cloned() else { return };
                c.execute(dstudio_core::Command::ResizeMidiClip { clip: m.id, start: m.start, length: 20 * PPQ });
                c.convert_clip(m.id);
                c.ui_dirty = true;
                c.update_ui(a);
            }),
        ),
        Check(
            "seq: the 80-step clip converts with its own step count (no 64-step cap) and pages: 3 pages of 32, page 1 shown, pager on",
            Box::new(|c, a| {
                let m = c.pr_clip().ok_or("clip closed")?;
                let p = m.pattern.as_ref().ok_or("conversion refused")?;
                let s = a.global::<Studio>();
                let ok = m.length == 20 * PPQ
                    && p.steps == 80
                    && !p.cells.is_empty()
                    && s.get_seq_pages() == 3
                    && s.get_seq_page() == 0
                    && s.get_seq_follow()
                    && s.get_seq_play_col() == -1;
                ok.then_some(()).ok_or(format!("steps {} pages {} page {}", p.steps, s.get_seq_pages(), s.get_seq_page()))
            }),
        ),
        DoC(
            "seq: play from beat 17 of the clip (step 69)",
            Box::new(|a, ctl| {
                {
                    let c = ctl.borrow();
                    let start = c.pr_clip().map_or(0, |m| m.start);
                    let f = c.project().ticks_to_frames(start + 17 * PPQ, c.rate);
                    let _ = c.engine.seek(f);
                    let _ = c.engine.play();
                }
                ctl.borrow_mut().update_ui(a);
                ctl.borrow_mut().update_ui(a);
            }),
        ),
        Check(
            "seq: Follow playhead turned to page 3; the ring is on its 5th column",
            Box::new(|c, a| {
                let s = a.global::<Studio>();
                let step = c.seq_play_step();
                // With a device running the playhead keeps moving between the two steps, so it may
                // already be further on: anywhere on page 3, with the ring on the step it shows.
                let last = if c.engine.device_running() { 95 } else { 68 };
                let ok = step.is_some_and(|k| (68..=last).contains(&k) && s.get_seq_play_col() == k as i32 - 64) && s.get_seq_page() == 2;
                ok.then_some(()).ok_or(format!("step {step:?} page {} col {}", s.get_seq_page(), s.get_seq_play_col()))
            }),
        ),
        DoC(
            "seq: stop; click page tab 1, then Follow playhead",
            Box::new(|a, ctl| {
                let _ = ctl.borrow().engine.stop();
                ctl.borrow_mut().update_ui(a);
                let w = ctl.borrow().pr.size.0;
                click(a, pr_origin().0 + w - 23.4 - 2.0 * 36.6, bottom_y() + 27.0 + 13.5);
                click(a, pr_origin().0 + w - 23.4 - 2.0 * 36.6 - 36.8, bottom_y() + 27.0 + 13.5);
            }),
        ),
        Check(
            "seq: page 1 shown, no ring while stopped, Follow playhead off",
            Box::new(|c, a| {
                let s = a.global::<Studio>();
                let ok = s.get_seq_page() == 0 && s.get_seq_play_col() == -1 && !s.get_seq_follow() && c.pr.seq.follow_off;
                ok.then_some(()).ok_or(format!("page {} col {} follow {}", s.get_seq_page(), s.get_seq_play_col(), s.get_seq_follow()))
            }),
        ),
        DoC(
            "seq: undo the conversion and the resize",
            Box::new(|a, ctl| {
                let mut c = ctl.borrow_mut();
                let _ = c.doc.undo();
                let _ = c.doc.undo();
                c.ui_dirty = true;
                c.update_ui(a);
            }),
        ),
        DoC(
            "seq: Cmd+Z, then remove the clip and restore the instrument",
            Box::new(move |a, ctl| {
                let mut c = ctl.borrow_mut();
                let _ = c.doc.undo();
                c.ui_dirty = true;
                c.update_ui(a);
                h9.set(seq_clip(&c).is_some() as usize);
                if let Some(id) = c.pr_clip().map(|m| m.id) {
                    c.execute(dstudio_core::Command::RemoveMidiClip { clip: id });
                }
                if let Some(t) = track_of(&c, TrackKind::Instrument).map(|i| c.project().tracks[i].id) {
                    let d = p1.borrow_mut().take();
                    c.execute(dstudio_core::Command::SetInstrumentPlugin { track: t, device: d });
                }
                c.select_clip(None);
                c.pr.focused = false;
                c.set_bottom_tab(crate::controller::BottomTab::Devices);
                c.seek_beats(0.0);
            }),
        ),
        Check(
            "seq: undo brought the pattern back; clip removed, instrument restored",
            Box::new(move |c, _| {
                let rack = track_of(c, TrackKind::Instrument)
                    .and_then(|i| c.project().tracks[i].instrument_plugin.as_ref())
                    .is_some_and(crate::stepseq::is_drum_rack);
                let left = track_of(c, TrackKind::Instrument)
                    .map_or(0, |i| c.project().tracks[i].midi_clips.iter().filter(|m| m.start >= 236 * PPQ).count());
                (hist.get() == 1 && !rack && left == 0)
                    .then_some(())
                    .ok_or(format!("undo restored {} rack {rack} clips left {left}", hist.get()))
            }),
        ),
    ]
}

/// Track height handle and timeline / bottom-panel splitter (appended last; restores both).
fn splitter_steps() -> Vec<Step> {
    use Step::*;
    fn edge_y(c: &Controller) -> f32 {
        track0_y() + c.view.pitch(0) - 2.5 - c.view.scroll_y as f32
    }
    fn splitter_y(c: &Controller) -> f32 {
        win().1 - 5.0 - c.bottom_h() - 1.0
    }
    let base_h = Rc::new(Cell::new(0f32));
    let (b1, b2, b3, b4) = (base_h.clone(), base_h.clone(), base_h.clone(), base_h.clone());
    let undo_n = Rc::new(Cell::new(0usize));
    let (u1, u2) = (undo_n.clone(), undo_n.clone());
    vec![
        Check(
            "record the undo depth",
            Box::new(move |c, _| {
                u1.set(c.doc.undo_depth());
                Ok(())
            }),
        ),
        DoC(
            "drag track 1's bottom edge down half a row",
            Box::new(|a, ctl| {
                let y = edge_y(&ctl.borrow());
                drag(a, tracks_x() + 37.35, y, tracks_x() + 37.35, y + PITCH / 2.0);
            }),
        ),
        Check(
            "track 1 is 1.5x tall in the model and the view, not an undo step",
            Box::new(move |c, _| {
                let h = c.project().tracks[0].height;
                let ok = approx(h, 1.5, 0.02) && approx(c.view.pitch(0), PITCH * h, 0.5) && c.doc.undo_depth() == u2.get();
                ok.then_some(()).ok_or(format!("height {h} pitch {} undo depth {} vs {}", c.view.pitch(0), c.doc.undo_depth(), u2.get()))
            }),
        ),
        DoC(
            "reset track 1's height",
            Box::new(|_, ctl| {
                let mut c = ctl.borrow_mut();
                let t = c.project().tracks[0].id;
                let _ = c.doc.set_track_height(t, 1.0);
                c.ui_dirty = true;
            }),
        ),
        Check(
            "track 1 back to its default height",
            Box::new(|c, _| {
                let h = c.project().tracks[0].height;
                (h == 1.0 && approx(c.view.pitch(0), PITCH, 0.01)).then_some(()).ok_or(format!("height {h}"))
            }),
        ),
        DoC(
            "drag the bottom-panel splitter up 100 pt",
            Box::new(move |a, ctl| {
                let (y, h) = {
                    let c = ctl.borrow();
                    (splitter_y(&c), c.bottom_h())
                };
                b1.set(h);
                drag(a, lane_x() + 246.35, y, lane_x() + 246.35, y - 100.0);
            }),
        ),
        Check(
            "bottom panel 100 pt taller and saved with the project",
            Box::new(move |c, a| {
                let h = c.bottom_h();
                let saved = c.project().bottom_panel_heights.values().copied().next();
                let ok = approx(h, b2.get() + 100.0, 0.5) && saved == Some(h) && approx(a.global::<Studio>().get_bottom_h(), h, 0.01);
                ok.then_some(()).ok_or(format!("h {h} base {} saved {saved:?}", b2.get()))
            }),
        ),
        Do("click the panel's collapse button", Box::new(|a| click(a, win().0 - 5.0 - 29.0 + 10.5, bottom_y() + 3.0 + 10.5))),
        Check(
            "panel shut to its tab row (saved), the taller height remembered",
            Box::new(|c, a| {
                let saved = c.project().bottom_panel_heights.values().copied().next();
                (c.bottom_collapsed() && a.global::<Studio>().get_bottom_collapsed() && saved == Some(crate::splitters::BOTTOM_COLLAPSED_H))
                    .then_some(())
                    .ok_or(format!("h {} saved {saved:?}", c.bottom_h()))
            }),
        ),
        Do("click it again", Box::new(|a| click(a, win().0 - 5.0 - 29.0 + 10.5, bottom_y() + 3.0 + 10.5))),
        Check(
            "panel reopened at the height it had (100 pt above the default)",
            Box::new(move |c, a| {
                let h = c.bottom_h();
                (approx(h, b3.get() + 100.0, 0.5) && !a.global::<Studio>().get_bottom_collapsed())
                    .then_some(())
                    .ok_or(format!("h {h} base {}", b3.get()))
            }),
        ),
        Do("collapse it with the button again", Box::new(|a| click(a, win().0 - 5.0 - 29.0 + 10.5, bottom_y() + 3.0 + 10.5))),
        DoC(
            "drag the splitter up 200 pt from the tab row",
            Box::new(|a, ctl| {
                let y = splitter_y(&ctl.borrow());
                drag(a, lane_x() + 246.35, y, lane_x() + 246.35, y - 200.0);
            }),
        ),
        Check(
            "the drag opened the panel and dropped the height the button kept",
            Box::new(|c, _| {
                (!c.bottom_collapsed() && c.bottom_restore.is_empty()).then_some(()).ok_or(format!(
                    "h {} restore {:?}",
                    c.bottom_h(),
                    c.bottom_restore
                ))
            }),
        ),
        DoC(
            "drag the splitter to the bottom edge",
            Box::new(|a, ctl| {
                let y = splitter_y(&ctl.borrow());
                drag(a, lane_x() + 246.35, y, lane_x() + 246.35, win().1 - 5.0);
            }),
        ),
        Check(
            "bottom panel collapses to its tab row",
            Box::new(|c, _| {
                approx(c.bottom_h(), crate::splitters::BOTTOM_COLLAPSED_H, 0.01).then_some(()).ok_or(format!("h {}", c.bottom_h()))
            }),
        ),
        Do(
            "click the expand button after a drag shut the panel",
            Box::new(|a| click(a, win().0 - 5.0 - 29.0 + 10.5, bottom_y() + 3.0 + 10.5)),
        ),
        Check(
            "the panel opens at its default height",
            Box::new(move |c, _| {
                let h = c.bottom_h();
                (approx(h, b4.get(), 0.5) && c.project().bottom_panel_heights.is_empty())
                    .then_some(())
                    .ok_or(format!("h {h} default {}", b4.get()))
            }),
        ),
        DoC(
            "restore the default panel height",
            Box::new(|_, ctl| {
                let mut c = ctl.borrow_mut();
                let kinds: Vec<String> = c.project().bottom_panel_heights.keys().cloned().collect();
                for k in kinds {
                    c.doc.set_bottom_panel_height(&k, None);
                }
                c.ui_dirty = true;
            }),
        ),
    ]
}

/// Docked assistant width splitter: drag, narrowing to the header's room, double-click reset
/// (appended last; restores the panel state and the saved width).
fn chat_dock_steps() -> Vec<Step> {
    use crate::splitters::{CHAT_GAP, CHAT_MAX_W, CHAT_MIN_W, chat_drag_max, chat_room};
    use Step::*;
    fn split_x(c: &Controller) -> f32 {
        win().0 - 5.0 - c.chat_w() - CHAT_GAP / 2.0
    }
    fn sliders_hidden() -> bool {
        chat_room(win().0, true) < CHAT_MIN_W
    }
    let was_open = Rc::new(Cell::new(false));
    let (w1, w2) = (was_open.clone(), was_open.clone());
    let undo_n = Rc::new(Cell::new(0usize));
    let (u1, u2) = (undo_n.clone(), undo_n.clone());
    vec![
        DoC(
            "open the assistant at its default width",
            Box::new(move |_, ctl| {
                let mut c = ctl.borrow_mut();
                w1.set(c.assistant_open);
                u1.set(c.doc.undo_depth());
                c.assistant_open = true;
                c.settings.prefs.assistant_width = 0.0;
                c.ui_dirty = true;
            }),
        ),
        Check(
            "default width 297 px, zoom sliders shown when the header has room",
            Box::new(|c, a| {
                let ok = approx(c.chat_w(), CHAT_MIN_W, 0.01)
                    && approx(a.global::<Studio>().get_assistant_pref_w(), CHAT_MIN_W, 0.01)
                    && a.get_header_zoom_hidden() == sliders_hidden();
                ok.then_some(()).ok_or(format!("w {} hidden {}", c.chat_w(), a.get_header_zoom_hidden()))
            }),
        ),
        DoC(
            "drag the splitter 60 pt left",
            Box::new(|a, ctl| {
                let x = split_x(&ctl.borrow());
                drag(a, x, track0_y() + 155.5, x - 60.0, track0_y() + 155.5);
            }),
        ),
        Check(
            "chat 60 pt wider (as far as the header's room allows), saved as a preference, timeline narrower, not an undo step",
            Box::new(move |c, a| {
                let w = c.chat_w();
                let saved = c.settings.prefs.assistant_width;
                let lanes = a.get_lanes_width();
                let ok = approx(w, (CHAT_MIN_W + 60.0).min(chat_drag_max(win().0, true)), 0.6)
                    && approx(saved, w, 0.01)
                    && approx(lanes, win().0 - geo().timeline_x - 5.0 - 5.0 - w - 191.0, 1.0)
                    && c.doc.undo_depth() == u2.get();
                ok.then_some(()).ok_or(format!("w {w} saved {saved} lanes {lanes}"))
            }),
        ),
        DoC(
            "drag the splitter far left: stops at the room that keeps the sliders",
            Box::new(|a, ctl| {
                let x = split_x(&ctl.borrow());
                drag(a, x, track0_y() + 155.5, tracks_x() - 62.65, track0_y() + 155.5);
            }),
        ),
        Check(
            "chat narrowed to the header's room (below 600 px, never below the minimum), sliders shown when they fit",
            Box::new(|c, a| {
                let room = crate::splitters::chat_room(win().0, true);
                let w = c.chat_w();
                let ok = room < CHAT_MAX_W
                    && approx(w, chat_drag_max(win().0, true), 0.01)
                    && a.get_header_zoom_hidden() == sliders_hidden()
                    && approx(a.get_lanes_width(), win().0 - geo().timeline_x - 5.0 - 5.0 - w - 191.0, 1.0);
                ok.then_some(()).ok_or(format!("w {w} room {room} hidden {}", a.get_header_zoom_hidden()))
            }),
        ),
        DoC(
            "drag the splitter right: stops at the minimum",
            Box::new(|a, ctl| {
                let x = split_x(&ctl.borrow());
                drag(a, x, track0_y() + 155.5, win().0 - 40.0, track0_y() + 155.5);
            }),
        ),
        Check("chat at 297 px", Box::new(|c, _| approx(c.chat_w(), CHAT_MIN_W, 0.01).then_some(()).ok_or(format!("w {}", c.chat_w())))),
        DoC(
            "widen, then double-click the splitter",
            Box::new(|a, ctl| {
                let x = split_x(&ctl.borrow());
                drag(a, x, track0_y() + 155.5, x - 40.0, track0_y() + 155.5);
                let x = split_x(&ctl.borrow());
                double_click(a, x, track0_y() + 155.5);
            }),
        ),
        Check(
            "double-click restored the default and cleared the preference",
            Box::new(|c, _| {
                let ok = approx(c.chat_w(), CHAT_MIN_W, 0.01) && c.settings.prefs.assistant_width == 0.0;
                ok.then_some(()).ok_or(format!("w {} pref {}", c.chat_w(), c.settings.prefs.assistant_width))
            }),
        ),
        DoC(
            "restore the panel state",
            Box::new(move |_, ctl| {
                let mut c = ctl.borrow_mut();
                c.assistant_open = w2.get();
                c.ui_dirty = true;
            }),
        ),
    ]
}

/// Window centre of Browser › Files row `i` (rows as pushed to the UI).
fn files_row_xy(a: &AppWindow, i: usize) -> Option<(f32, f32)> {
    let s = a.global::<Studio>();
    let r = s.get_files().row_data(i)?;
    Some((geo().browser_x + 115.0, geo().browser_y + s.get_files_list_y() + 0.5 + r.y - s.get_files_scroll() + r.h / 2.0))
}

fn files_labels(a: &AppWindow) -> Vec<String> {
    let m = a.global::<Studio>().get_files();
    (0..m.row_count()).filter_map(|i| m.row_data(i)).map(|r| r.label.to_string()).collect()
}

fn files_row_of(a: &AppWindow, label: &str) -> Option<usize> {
    let m = a.global::<Studio>().get_files();
    (0..m.row_count()).find(|&i| m.row_data(i).is_some_and(|r| r.kind == 0 && r.label == label))
}

/// Browser sections, Back / Forward, and Files › Folders and Projects on generated files.
fn files_steps() -> Vec<Step> {
    use crate::files::{SECTION_DEVICES, SECTION_FILES, TAB_FOLDERS, TAB_PROJECTS};
    use Step::*;
    let root = run_dir().join("files");
    let kit = root.join("kit");
    let (r1, k1, k2) = (root.clone(), kit.clone(), kit.clone());
    let tracks = Rc::new(Cell::new(0usize));
    let (t1, t2, t3, t4) = (tracks.clone(), tracks.clone(), tracks.clone(), tracks.clone());
    vec![
        DoC(
            "files: generated projects folder and sample folder; Devices section",
            Box::new(move |_, ctl| {
                let _ = std::fs::remove_dir_all(&r1);
                let _ = std::fs::create_dir_all(k1.join("loops"));
                let _ = std::fs::create_dir_all(r1.join("Projects"));
                let _ = crate::wavgen::write_wav(&k1.join("kick.wav"), 48_000, 1, &crate::wavgen::sine_bursts(48_000));
                let mut c = ctl.borrow_mut();
                c.files.projects_dir = Some(r1.join("Projects"));
                c.browser_section(SECTION_DEVICES);
                t1.set(c.project().tracks.len());
            }),
        ),
        Do("files: Escape (close menus)", Box::new(|a| key(a, Key::Escape, false, false))),
        Do("files: click the Files section tab", Box::new(|a| click(a, geo().browser_x + 145.0, geo().browser_y + 13.8))),
        Check(
            "files: Files › Projects is empty, Back is enabled",
            Box::new(|c, a| {
                let s = a.global::<Studio>();
                let ok = c.files.section == SECTION_FILES
                    && c.files.tab == TAB_PROJECTS
                    && s.get_browser_can_back()
                    && !s.get_browser_can_forward()
                    && s.get_files_empty_title() == "No projects yet"
                    && s.get_files_meta() == "";
                ok.then_some(()).ok_or(format!("section {} tab {} title {:?}", c.files.section, c.files.tab, s.get_files_empty_title()))
            }),
        ),
        Do("files: click the Folders sub-tab", Box::new(|a| click(a, geo().browser_x + 71.0, geo().browser_y + 66.5))),
        Check(
            "files: Folders is empty with its hint",
            Box::new(|c, a| {
                let s = a.global::<Studio>();
                (c.files.tab == TAB_FOLDERS && s.get_files_empty_title() == "No folders yet").then_some(()).ok_or(format!(
                    "tab {} title {:?}",
                    c.files.tab,
                    s.get_files_empty_title()
                ))
            }),
        ),
        DoC("files: add the sample folder", Box::new(move |_, ctl| ctl.borrow_mut().files_add_folder(&k2))),
        Check(
            "files: one folder row and the count row",
            Box::new(|_, a| {
                let s = a.global::<Studio>();
                (files_labels(a) == ["kit"] && s.get_files_meta() == "1 folder" && s.get_files_sort() == "Recent")
                    .then_some(())
                    .ok_or(format!("{:?} {:?}", files_labels(a), s.get_files_meta()))
            }),
        ),
        DoC(
            "files: click the sort button",
            Box::new(|a, _| {
                let y = geo().browser_y + a.global::<Studio>().get_files_list_y() - 10.2;
                click(a, geo().browser_x + 242.25 - 12.0, y);
            }),
        ),
        Check(
            "files: the sort menu opens on Recent",
            Box::new(|c, a| {
                let s = a.global::<Studio>();
                (c.files.sort_open && s.get_files_sort_open() && s.get_files_sort_index() == 0).then_some(()).ok_or(format!(
                    "open {} index {}",
                    s.get_files_sort_open(),
                    s.get_files_sort_index()
                ))
            }),
        ),
        DoC(
            "files: pick Name in the sort menu",
            Box::new(|a, _| {
                let y = geo().browser_y + a.global::<Studio>().get_files_list_y() + 3.4 + 3.4 + 18.7 * 1.5;
                click(a, geo().browser_x + 242.25 - 15.0, y);
            }),
        ),
        Check(
            "files: sorted by Name, menu closed",
            Box::new(|c, a| {
                let s = a.global::<Studio>();
                (c.files.sort_name && !s.get_files_sort_open() && s.get_files_sort() == "Name").then_some(()).ok_or(format!(
                    "sort {:?} open {}",
                    s.get_files_sort(),
                    s.get_files_sort_open()
                ))
            }),
        ),
        DoC(
            "files: open the menu and click outside it",
            Box::new(|a, ctl| {
                ctl.borrow_mut().files_sort_open_menu(true);
                ctl.borrow_mut().update_ui(a);
                click(a, lane_x() + 146.35, track0_y() + 255.5);
            }),
        ),
        Check(
            "files: a click outside closes the menu and keeps Name",
            Box::new(|c, _| {
                (!c.files.sort_open && c.files.sort_name)
                    .then_some(())
                    .ok_or(format!("open {} name {}", c.files.sort_open, c.files.sort_name))
            }),
        ),
        DoC("files: back to Recent", Box::new(|_, ctl| ctl.borrow_mut().files_sort_pick(0))),
        DoC(
            "files: click the folder row",
            Box::new(|a, _| {
                let Some((x, y)) = files_row_xy(a, 0) else { return };
                click(a, x, y);
            }),
        ),
        Check(
            "files: the folder opens to its subfolder and WAV",
            Box::new(|_, a| {
                let m = a.global::<Studio>().get_files();
                let wav = (0..m.row_count()).filter_map(|i| m.row_data(i)).find(|r| r.label == "kick.wav");
                let ok = files_labels(a) == ["kit", "loops", "kick.wav"]
                    && wav.is_some_and(|r| r.depth == 1 && r.detail == "wav")
                    && a.global::<Studio>().get_files_guides().row_count() == 1;
                ok.then_some(()).ok_or(format!("{:?}", files_labels(a)))
            }),
        ),
        DoC(
            "files: click kick.wav to preview it",
            Box::new(|a, _| {
                let Some(i) = files_row_of(a, "kick.wav") else { return };
                let Some((x, y)) = files_row_xy(a, i) else { return };
                click(a, x, y);
            }),
        ),
        Check(
            "files: kick.wav loads into the preview bar and plays",
            Box::new(|c, a| {
                let s = a.global::<Studio>();
                let au = &c.files.audition;
                let m = s.get_files();
                let row = (0..m.row_count()).filter_map(|i| m.row_data(i)).find(|r| r.label == "kick.wav");
                let ok = au.path.as_ref().is_some_and(|p| p.ends_with("kick.wav"))
                    && au.asset.is_some()
                    && au.playing
                    && s.get_preview_active()
                    && s.get_preview_playing()
                    && row.as_ref().is_some_and(|r| r.detail == "Playing");
                ok.then_some(()).ok_or(format!(
                    "{:?} asset {:?} playing {} detail {:?}",
                    au.path,
                    au.asset,
                    au.playing,
                    row.map(|r| r.detail)
                ))
            }),
        ),
        DoC(
            "files: pause the preview and solo it",
            Box::new(|_, ctl| {
                let mut c = ctl.borrow_mut();
                c.audition_toggle();
                c.audition_solo();
            }),
        ),
        Check(
            "files: paused and soloed; the row shows its type again",
            Box::new(|c, a| {
                let s = a.global::<Studio>();
                let m = s.get_files();
                let row = (0..m.row_count()).filter_map(|i| m.row_data(i)).find(|r| r.label == "kick.wav");
                let ok = !c.files.audition.playing
                    && !s.get_preview_playing()
                    && s.get_preview_solo()
                    && row.as_ref().is_some_and(|r| r.detail == "wav");
                ok.then_some(()).ok_or(format!(
                    "playing {} solo {} detail {:?}",
                    c.files.audition.playing,
                    s.get_preview_solo(),
                    row.map(|r| r.detail)
                ))
            }),
        ),
        DoC("files: solo off", Box::new(|_, ctl| ctl.borrow_mut().audition_solo())),
        Do("files: Back (chevron)", Box::new(|a| click(a, geo().browser_x + 224.5, geo().browser_y + 13.0))),
        Check(
            "files: Back returns to Projects with Forward enabled",
            Box::new(|c, a| {
                (c.files.section == SECTION_FILES && c.files.tab == TAB_PROJECTS && a.global::<Studio>().get_browser_can_forward())
                    .then_some(())
                    .ok_or(format!("section {} tab {}", c.files.section, c.files.tab))
            }),
        ),
        Do("files: Back again", Box::new(|a| click(a, geo().browser_x + 224.5, geo().browser_y + 13.0))),
        Check(
            "files: Back reaches Devices",
            Box::new(|c, a| {
                (c.files.section == SECTION_DEVICES && a.global::<Studio>().get_browser_section() == SECTION_DEVICES as i32)
                    .then_some(())
                    .ok_or(format!("section {}", c.files.section))
            }),
        ),
        DoC(
            "files: Forward, then the Devices › Effects sub-tab",
            Box::new(|_, ctl| {
                let mut c = ctl.borrow_mut();
                c.browser_forward();
                c.browser_tab(1);
            }),
        ),
        Check(
            "files: a new location drops the Forward entries",
            Box::new(|c, a| {
                let s = a.global::<Studio>();
                (c.files.section == SECTION_DEVICES && c.devices.tab == 1 && !s.get_browser_can_forward() && s.get_browser_can_back())
                    .then_some(())
                    .ok_or(format!("section {} tab {}", c.files.section, c.devices.tab))
            }),
        ),
        DoC(
            "files: Folders again, search \"KICK\"",
            Box::new(|_, ctl| {
                let mut c = ctl.borrow_mut();
                c.browser_tab(0);
                c.files_tab(TAB_FOLDERS);
                c.browser_search("KICK");
            }),
        ),
        Check(
            "files: search keeps the folder and its match only",
            Box::new(|c, a| {
                let shown = a.global::<Studio>().get_browser_search();
                let ok = files_labels(a) == ["kit", "kick.wav"] && shown == "KICK" && c.files.search == "KICK";
                ok.then_some(()).ok_or(format!("{:?} field {shown:?} devices {:?}", files_labels(a), c.devices.search))
            }),
        ),
        DoC(
            "files: clear the search; double-click kick.wav",
            Box::new(|a, ctl| {
                ctl.borrow_mut().browser_search("");
                ctl.borrow_mut().update_ui(a);
                let Some(i) = files_row_of(a, "kick.wav") else { return };
                let Some((x, y)) = files_row_xy(a, i) else { return };
                double_click(a, x, y);
            }),
        ),
        Check(
            "files: the WAV lands on a new audio track",
            Box::new(move |c, _| {
                let p = c.project();
                let last = p.tracks.last();
                let ok = p.tracks.len() == t2.get() + 1
                    && last.is_some_and(|t| t.kind == TrackKind::Audio && t.clips.first().is_some_and(|k| k.name == "kick"));
                ok.then_some(()).ok_or(format!("{} tracks (was {})", p.tracks.len(), t2.get()))
            }),
        ),
        DoC(
            "files: undo; save the project into the projects folder; open Projects",
            Box::new(move |_, ctl| {
                let mut c = ctl.borrow_mut();
                c.undo();
                let dir = c.files.projects_dir.clone().unwrap_or_default().join("Song.dsproj");
                c.save_to(&dir);
                c.files_tab(TAB_PROJECTS);
            }),
        ),
        Check(
            "files: the saved project is listed",
            Box::new(|_, a| {
                (files_labels(a) == ["Song"] && a.global::<Studio>().get_files_meta() == "1 project")
                    .then_some(())
                    .ok_or(format!("{:?}", files_labels(a)))
            }),
        ),
        DoC(
            "files: open the project with its chevron",
            Box::new(|a, _| {
                let Some((_, y)) = files_row_xy(a, 0) else { return };
                click(a, geo().browser_x + 10.2 + 4.0, y);
            }),
        ),
        Check(
            "files: the project lists its tracks and MIDI clips",
            Box::new(|_, a| {
                let l = files_labels(a);
                (l.first().map(String::as_str) == Some("Song")
                    && l.contains(&"Tracks".to_string())
                    && files_row_of(a, "Instrument").is_some())
                .then_some(())
                .ok_or(format!("{l:?}"))
            }),
        ),
        DoC(
            "files: double-click the project's Instrument track",
            Box::new(move |a, ctl| {
                t3.set(ctl.borrow().project().tracks.len());
                let Some(i) = files_row_of(a, "Instrument") else { return };
                let Some((x, y)) = files_row_xy(a, i) else { return };
                double_click(a, x, y);
            }),
        ),
        Check(
            "files: the track is added as a new track (one undo step)",
            Box::new(move |c, _| {
                let p = c.project();
                let ok = p.tracks.len() == t4.get() + 1
                    && p.tracks.last().is_some_and(|t| t.kind == TrackKind::Instrument)
                    && c.doc.undo_label() == Some("Add Track");
                ok.then_some(()).ok_or(format!("{} tracks, undo {:?}", p.tracks.len(), c.doc.undo_label()))
            }),
        ),
        DoC(
            "files: undo; back to Devices; forget the folder",
            Box::new(move |_, ctl| {
                let mut c = ctl.borrow_mut();
                c.undo();
                let _ = c.engine.preview_stop();
                c.files.audition.playing = false;
                c.settings.prefs.browser_folders.clear();
                c.browser_section(SECTION_DEVICES);
                c.project_dir = None;
                let _ = std::fs::remove_dir_all(&root);
            }),
        ),
    ]
}

/// Menu bar items without a shortcut: Check for Updates…, Export Project for Ableton Live…
/// and Open Recent (appended last; it saves and reopens the current project).
fn menu_bar_steps() -> Vec<Step> {
    use Step::*;
    let dir = run_dir().join("recent").join("Recent Song.dsproj");
    let (d1, d2, d3) = (dir.clone(), dir.clone(), dir);
    vec![
        DoC("menu: Options › Check for Updates…", Box::new(|_, ctl| ctl.borrow_mut().menu_action("options.check_updates"))),
        Check(
            "menu: the updates dialog is open and names the version",
            Box::new(|_, a| {
                let s = a.global::<Studio>();
                (s.get_updates_open() && s.get_app_version() == env!("CARGO_PKG_VERSION")).then_some(()).ok_or(format!(
                    "open {} version {:?}",
                    s.get_updates_open(),
                    s.get_app_version()
                ))
            }),
        ),
        Do("menu: Escape closes it", Box::new(|a| key(a, Key::Escape, false, false))),
        Check(
            "menu: the updates dialog is closed",
            Box::new(|c, a| (!c.updates_open && !a.global::<Studio>().get_updates_open()).then_some(()).ok_or("still open".into())),
        ),
        DoC("menu: File › Export Project for Ableton Live…", Box::new(|_, ctl| ctl.borrow_mut().menu_action("file.export_ableton"))),
        Check(
            "menu: the export dialog opens on Project",
            Box::new(|_, a| {
                let s = a.global::<Studio>();
                (s.get_export_open() && s.get_export_recipe() == 2).then_some(()).ok_or(format!(
                    "open {} recipe {}",
                    s.get_export_open(),
                    s.get_export_recipe()
                ))
            }),
        ),
        Do("menu: Escape closes the export dialog", Box::new(|a| key(a, Key::Escape, false, false))),
        DoC(
            "menu: solo track 1 twice (two edits); Edit › Undo History…",
            Box::new(|_, ctl| {
                let mut c = ctl.borrow_mut();
                let t = c.project().tracks[0].id;
                c.toggle_solo(t);
                c.toggle_solo(t);
                c.menu_action("edit.undo_history");
            }),
        ),
        Check(
            "menu: Undo History lists Open Project and every edit, the last one current",
            Box::new(|c, a| {
                let s = a.global::<Studio>();
                let rows: Vec<_> = s.get_undo_history_rows().iter().collect();
                let n = c.doc.history_steps().len();
                let last = rows.last();
                (s.get_undo_history_open()
                    && rows.len() == n + 1
                    && rows[0].label == "Open Project"
                    && last.is_some_and(|r| r.state == 1 && r.label == "Solo change" && r.time.contains(':')))
                .then_some(())
                .ok_or(format!("open {} rows {} steps {n}", s.get_undo_history_open(), rows.len()))
            }),
        ),
        DoC(
            "menu: pick the row two above the current one",
            Box::new(|_, ctl| {
                let depth = ctl.borrow().doc.undo_depth() as i32;
                ctl.borrow_mut().undo_history_travel(depth - 2);
            }),
        ),
        Check(
            "menu: both solo edits are undone and still listed",
            Box::new(|c, a| {
                let s = a.global::<Studio>();
                let rows: Vec<_> = s.get_undo_history_rows().iter().collect();
                let undone = rows.iter().filter(|r| r.state == 2).count();
                (!c.project().tracks[0].solo && undone == 2 && c.doc.can_redo())
                    .then_some(())
                    .ok_or(format!("solo {} undone rows {undone}", c.project().tracks[0].solo))
            }),
        ),
        DoC(
            "menu: Redo button (one step)",
            Box::new(|a, ctl| {
                let cur = a.global::<Studio>().get_undo_history_current();
                ctl.borrow_mut().undo_history_travel(cur + 1);
            }),
        ),
        Check("menu: the first solo edit is back", Box::new(|c, _| c.project().tracks[0].solo.then_some(()).ok_or("not soloed".into()))),
        DoC(
            "menu: undo it; Escape closes Undo History",
            Box::new(|a, ctl| {
                ctl.borrow_mut().undo();
                key(a, Key::Escape, false, false);
            }),
        ),
        Check(
            "menu: Undo History closed, track 1 not soloed",
            Box::new(|c, a| {
                (!c.undo_history_open && !a.global::<Studio>().get_undo_history_open() && !c.project().tracks[0].solo)
                    .then_some(())
                    .ok_or("still open or soloed".into())
            }),
        ),
        DoC(
            "menu: save the project, then forget its folder",
            Box::new(move |_, ctl| {
                let mut c = ctl.borrow_mut();
                c.settings.prefs.recent_projects.clear();
                c.save_to(&d1);
                c.project_dir = None;
            }),
        ),
        Check(
            "menu: File › Open Recent lists it",
            Box::new(|_, a| {
                let names: Vec<String> = a.global::<Studio>().get_recent_projects().iter().map(|s| s.to_string()).collect();
                (names == ["Recent Song"]).then_some(()).ok_or(format!("{names:?}"))
            }),
        ),
        DoC("menu: open it from Open Recent", Box::new(|_, ctl| ctl.borrow_mut().open_recent(0))),
        Check(
            "menu: the project is open from its folder",
            Box::new(move |c, _| (c.project_dir.as_ref() == Some(&d2)).then_some(()).ok_or(format!("{:?}", c.project_dir))),
        ),
        DoC(
            "menu: Open Recent › Clear Menu; remove the folder",
            Box::new(move |_, ctl| {
                let mut c = ctl.borrow_mut();
                c.menu_action("file.clear_recent");
                c.project_dir = None;
                if let Some(parent) = d3.parent() {
                    let _ = std::fs::remove_dir_all(parent);
                }
            }),
        ),
        Check(
            "menu: Open Recent is empty",
            Box::new(|_, a| (a.global::<Studio>().get_recent_projects().row_count() == 0).then_some(()).ok_or("not empty".into())),
        ),
        DoC(
            "menu: open a recent project whose folder is gone",
            Box::new(|_, ctl| {
                let mut c = ctl.borrow_mut();
                c.settings.prefs.recent_projects = vec![run_dir().join("missing.dsproj").to_string_lossy().into_owned()];
                c.open_recent(0);
            }),
        ),
        Check(
            "menu: it leaves Open Recent",
            Box::new(|c, _| {
                c.settings.prefs.recent_projects.is_empty().then_some(()).ok_or(format!("{:?}", c.settings.prefs.recent_projects))
            }),
        ),
    ]
}

/// Settings › General: the controls DStudio backs change the preferences; the others stay off.
fn general_settings_steps() -> Vec<Step> {
    use Step::*;
    fn page_x() -> f32 {
        set_x() + 187.0
    }
    fn content_w() -> f32 {
        set_w() - 187.0
    }
    let switch_xy = |row_y: f32| (page_x() + content_w() - 43.0 + 10.0, set_y() + row_y + 7.0);
    let added = Rc::new(Cell::new(None));
    let (a1, a2) = (added.clone(), added);
    vec![
        DoC(
            "general: open Settings › General",
            Box::new(|_, ctl| {
                let mut c = ctl.borrow_mut();
                c.open_settings();
                c.settings_page(0);
            }),
        ),
        Check(
            "general: defaults (Return to start; third-party windows and on top on)",
            Box::new(|c, a| {
                let s = a.global::<Studio>();
                let on: Vec<bool> = s.get_general_on().iter().collect();
                (s.get_settings_page() == 0
                    && on == [false, true, false, true]
                    && !s.get_general_stop_in_place()
                    && c.settings.prefs.general == crate::prefs::GeneralPrefs::default())
                .then_some(())
                .ok_or(format!("page {} on {on:?}", s.get_settings_page()))
            }),
        ),
        Do(
            "general: click Stop in place",
            Box::new(|a| click(a, page_x() + content_w() - 20.5 - 172.5 + 87.0 + 40.0, set_y() + 143.8 + 8.6)),
        ),
        Do(
            "general: click Auto-show built-in effect windows",
            Box::new(move |a| {
                let (x, y) = switch_xy(331.4);
                click(a, x, y);
            }),
        ),
        Do(
            "general: click the disabled Sample licensing switch",
            Box::new(move |a| {
                let (x, y) = switch_xy(202.9);
                click(a, x, y);
            }),
        ),
        Check(
            "general: Stop in place and built-in windows on; licensing unchanged",
            Box::new(|c, a| {
                let g = &c.settings.prefs.general;
                let s = a.global::<Studio>();
                (g.stop_in_place && g.show_builtin_windows && s.get_general_stop_in_place() && s.get_general_on().row_data(2) == Some(true))
                    .then_some(())
                    .ok_or(format!("{g:?}"))
            }),
        ),
        DoC(
            "general: close Settings; add a built-in Utility to the selected track",
            Box::new(move |_, ctl| {
                let mut c = ctl.borrow_mut();
                c.close_settings();
                let Some(t) = c.selected_track else { return };
                let Some(def) = dstudio_core::builtin::find("utility") else { return };
                c.add_item_to_track(&crate::devices::Item::Builtin(def), t, None);
                a1.set(c.project().track(t).and_then(|t| t.devices.last()).map(|d| d.id));
            }),
        ),
        Check(
            "general: its device window opened",
            Box::new(move |c, _| {
                let id = a2.get().ok_or("no device added")?;
                c.devices.windows.contains(&id).then_some(()).ok_or("no window".into())
            }),
        ),
        DoC(
            "general: remove it; restore the defaults",
            Box::new(|_, ctl| {
                let mut c = ctl.borrow_mut();
                let Some(t) = c.selected_track else { return };
                if let Some(id) = c.project().track(t).and_then(|t| t.devices.last()).map(|d| d.id) {
                    c.device_remove(id);
                }
                c.general_toggle(2);
                c.general_stop_in_place(false);
            }),
        ),
        Check(
            "general: defaults back",
            Box::new(|c, _| {
                (c.settings.prefs.general == crate::prefs::GeneralPrefs::default())
                    .then_some(())
                    .ok_or(format!("{:?}", c.settings.prefs.general))
            }),
        ),
    ]
}

/// Add device card search (CHR-17): picking a search result hands the keyboard back to the
/// shortcuts, so ⌘Z undoes the add instead of editing the search text.
fn add_search_steps() -> Vec<Step> {
    use Step::*;
    let hist = Rc::new(Cell::new(0usize));
    let (h0, h1, h2) = (hist.clone(), hist.clone(), hist);
    let card_xy = |a: &AppWindow, x: f32, y: f32| (bottom_x() + a.global::<Studio>().get_add_device_x() + x, bottom_y() + 34.5 + y);
    let refresh = |a: &AppWindow, ctl: &Ctl| {
        if let Ok(mut c) = ctl.try_borrow_mut() {
            c.update_ui(a);
        }
    };
    vec![
        DoC(
            "add search: select the Audio track (empty chain), Devices tab",
            Box::new(move |a, ctl| {
                let i = track_of(&ctl.borrow(), TrackKind::Audio).unwrap_or(0);
                click_track_header(a, ctl, i);
                ctl.borrow_mut().set_bottom_tab(BottomTab::Devices);
                refresh(a, ctl);
                h0.set(ctl.borrow().doc.history_labels().len());
            }),
        ),
        Do(
            "add search: click the search field and type Utility",
            Box::new(move |a| {
                let (x, y) = card_xy(a, 5.5 + 26.0 + 40.0, 31.5 + 10.0);
                click(a, x, y);
                type_text(a, "Utility");
            }),
        ),
        Check(
            "add search: the field has the focus and lists Utility",
            Box::new(|c, a| {
                let s = a.global::<Studio>();
                let found =
                    (0..s.get_add_results().row_count()).filter_map(|i| s.get_add_results().row_data(i)).any(|m| m.label == "Utility");
                let empty = track_devices(c, TrackKind::Audio).is_empty();
                (s.get_search_focused() && found && empty)
                    .then_some(())
                    .ok_or(format!("focused {} found {found} empty chain {empty}", s.get_search_focused()))
            }),
        ),
        DoC(
            "add search: click the Utility result",
            Box::new(move |a, ctl| {
                refresh(a, ctl);
                let s = a.global::<Studio>();
                let Some(i) =
                    (0..s.get_add_results().row_count()).filter_map(|i| s.get_add_results().row_data(i)).position(|m| m.label == "Utility")
                else {
                    return;
                };
                let (x, y) = card_xy(a, 60.0, 56.5 + i as f32 * 22.0 + 11.0);
                click(a, x, y);
                refresh(a, ctl);
            }),
        ),
        Check(
            "add search: Utility added in one step; the search field gave up the focus",
            Box::new(move |c, a| {
                let n = c.doc.history_labels().len();
                let names: Vec<String> = track_devices(c, TrackKind::Audio).into_iter().map(|d| d.1).collect();
                let focused = a.global::<Studio>().get_search_focused();
                (n == h1.get() + 1 && names == ["Utility"] && !focused)
                    .then_some(())
                    .ok_or(format!("history {} -> {n}, chain {names:?}, focused {focused}", h1.get()))
            }),
        ),
        Do("add search: Cmd+Z", Box::new(|a| key(a, "z", true, false))),
        Check(
            "add search: Cmd+Z undid the add (the search text is untouched)",
            Box::new(move |c, a| {
                let names: Vec<String> = track_devices(c, TrackKind::Audio).into_iter().map(|d| d.1).collect();
                let text = a.global::<Studio>().get_add_search();
                (c.doc.history_labels().len() == h2.get() && names.is_empty() && text == "Utility")
                    .then_some(())
                    .ok_or(format!("chain {names:?}, search {text:?}"))
            }),
        ),
        DoC(
            "add search: clear the search",
            Box::new(move |a, ctl| {
                ctl.borrow_mut().add_search("");
                refresh(a, ctl);
            }),
        ),
    ]
}

/// Section lane under the loop strip (appended last; clears its sections at the end).
fn section_steps() -> Vec<Step> {
    use Step::*;
    use dstudio_core::SectionId;
    // Lane-local (x, y) → window: the lanes start at lane_x(), their ruler at track0_y() - RULER_H.
    fn row_y() -> f32 {
        lane_y0() + 31.8
    }
    fn bar_x(c: &Controller, bar: f64) -> f32 {
        let bpb = c.project().ticks_per_bar() as f64 / PPQ as f64;
        lane_x() + c.view.x_of(bar * bpb)
    }
    fn visible_x(c: &Controller, from: f64, to: f64) -> f32 {
        let lo = bar_x(c, from).max(lane_x());
        let hi = bar_x(c, to).min(lane_x() + c.lanes_size.0 - 10.0);
        (lo + hi) / 2.0
    }
    fn names(c: &Controller) -> Vec<String> {
        c.project().sections.iter().map(|s| s.name.clone()).collect()
    }
    fn bars(c: &Controller) -> Vec<i64> {
        let bar = c.project().ticks_per_bar();
        c.project().sections.iter().map(|s| s.length / bar).collect()
    }
    fn right_click(a: &AppWindow, x: f32, y: f32) {
        let w = a.window();
        let p = LogicalPosition::new(x, y);
        dispatch(w, WindowEvent::PointerMoved { position: p });
        dispatch(w, WindowEvent::PointerPressed { position: p, button: PointerEventButton::Right });
        dispatch(w, WindowEvent::PointerReleased { position: p, button: PointerEventButton::Right });
    }
    fn menu_row(a: &AppWindow, label: &str) -> Option<(f32, f32)> {
        let s = a.global::<Studio>();
        let items = s.get_menu_items();
        (0..slint::Model::row_count(&items))
            .filter_map(|i| slint::Model::row_data(&items, i))
            .find(|m| m.label == label)
            .map(|m| (s.get_menu_x() + 40.0, s.get_menu_y() + m.y + crate::clipedit::MENU_ROW / 2.0))
    }
    let first = Rc::new(Cell::new(0u64));
    let (f1, f2) = (first.clone(), first.clone());
    vec![
        DoC(
            "start the section lane at bar 1, click its +",
            Box::new(|a, ctl| {
                {
                    let mut c = ctl.borrow_mut();
                    c.view.start_beats = 0.0;
                    c.ui_dirty = true;
                }
                let x = lane_x() + ctl.borrow().section_add_x();
                click(a, x, row_y());
            }),
        ),
        Check(
            "+ added \"Section 1\": 8 bars from bar 1, first colour, selected, one undo step",
            Box::new(move |c, _| {
                let p = c.project();
                let s = p.sections.first().ok_or("no section")?;
                f1.set(s.id.0);
                let ok = p.sections.len() == 1
                    && s.name == "Section 1"
                    && s.length == 8 * p.ticks_per_bar()
                    && s.color == crate::sections::PALETTE[6]
                    && c.selected_section == Some(s.id)
                    && c.doc.undo_label() == Some("Add Section");
                ok.then_some(()).ok_or(format!("{:?} {:?}", names(c), c.doc.undo_label()))
            }),
        ),
        DoC(
            "click the + after it",
            Box::new(|a, ctl| {
                let x = lane_x() + ctl.borrow().section_add_x();
                click(a, x, row_y());
            }),
        ),
        Check(
            "\"Section 2\" follows at bar 9 in the next colour",
            Box::new(|c, _| {
                let p = c.project();
                let ok = names(c) == ["Section 1", "Section 2"]
                    && p.section_start(1) == 8 * p.ticks_per_bar()
                    && p.sections[1].color == crate::sections::PALETTE[7];
                ok.then_some(()).ok_or(format!("{:?} {:?}", names(c), bars(c)))
            }),
        ),
        DoC(
            "drag Section 1's right edge from bar 9 to bar 5",
            Box::new(|a, ctl| {
                let (x0, x1) = {
                    let c = ctl.borrow();
                    (bar_x(&c, 8.0) - 2.0, bar_x(&c, 4.0))
                };
                drag(a, x0, row_y(), x1, row_y());
            }),
        ),
        Check(
            "Section 1 is 4 bars and Section 2 moved with its end",
            Box::new(|c, _| {
                let p = c.project();
                (bars(c) == [4, 8] && p.section_start(1) == 4 * p.ticks_per_bar() && c.doc.undo_label() == Some("Resize Section"))
                    .then_some(())
                    .ok_or(format!("{:?}", bars(c)))
            }),
        ),
        DoC(
            "drag Section 2 to bar 1",
            Box::new(|a, ctl| {
                let (x0, x1) = {
                    let c = ctl.borrow();
                    (bar_x(&c, 7.0), bar_x(&c, 0.2))
                };
                drag(a, x0, row_y(), x1, row_y());
            }),
        ),
        Check(
            "Section 2 now comes first",
            Box::new(|c, _| {
                (names(c) == ["Section 2", "Section 1"] && c.doc.undo_label() == Some("Move Section"))
                    .then_some(())
                    .ok_or(format!("{:?}", names(c)))
            }),
        ),
        DoC(
            "double-click Section 1",
            Box::new(|a, ctl| {
                let x = visible_x(&ctl.borrow(), 8.0, 12.0);
                double_click(a, x, row_y());
            }),
        ),
        Check(
            "rename popup open for the section",
            Box::new(move |c, a| {
                let s = a.global::<Studio>();
                (s.get_rename_open() && s.get_rename_title() == "Rename section" && c.rename_section == Some(SectionId(f2.get())))
                    .then_some(())
                    .ok_or(format!("open {} title {}", s.get_rename_open(), s.get_rename_title()))
            }),
        ),
        Do(
            "type Chorus, Enter",
            Box::new(|a| {
                key(a, "a", true, false);
                type_text(a, "Chorus");
                key(a, Key::Return, false, false);
            }),
        ),
        Check(
            "renamed to Chorus (one undo step)",
            Box::new(|c, _| {
                (names(c) == ["Section 2", "Chorus"] && c.doc.undo_label() == Some("Rename Section")).then_some(()).ok_or(format!(
                    "{:?} {:?}",
                    names(c),
                    c.doc.undo_label()
                ))
            }),
        ),
        DoC(
            "right-click Chorus",
            Box::new(|a, ctl| {
                let x = visible_x(&ctl.borrow(), 8.0, 12.0);
                right_click(a, x, row_y());
            }),
        ),
        Check(
            "section menu: Duplicate, Rename, Delete section, Clear all sections",
            Box::new(|_, a| {
                let s = a.global::<Studio>();
                let ok = s.get_menu_open()
                    && ["Duplicate", "Rename…", "Delete section", "Clear all sections"].iter().all(|l| menu_row(a, l).is_some());
                ok.then_some(()).ok_or(format!("open {}", s.get_menu_open()))
            }),
        ),
        Do(
            "choose Delete section",
            Box::new(|a| {
                if let Some((x, y)) = menu_row(a, "Delete section") {
                    click(a, x, y);
                }
            }),
        ),
        Check(
            "Chorus deleted, menu closed",
            Box::new(|c, a| {
                (names(c) == ["Section 2"] && !a.global::<Studio>().get_menu_open()).then_some(()).ok_or(format!("{:?}", names(c)))
            }),
        ),
        Do("Cmd+Z", Box::new(|a| key(a, "z", true, false))),
        DoC(
            "click Section 2, press Delete",
            Box::new(|a, ctl| {
                let x = bar_x(&ctl.borrow(), 2.0);
                click(a, x, row_y());
                key(a, Key::Delete, false, false);
            }),
        ),
        Check(
            "undo restored Chorus; Delete removed the selected section",
            Box::new(|c, _| (names(c) == ["Chorus"]).then_some(()).ok_or(format!("{:?}", names(c)))),
        ),
        DoC(
            "right-click Chorus, Clear all sections",
            Box::new(|a, ctl| {
                let x = bar_x(&ctl.borrow(), 1.0);
                right_click(a, x, row_y());
                if let Some((x, y)) = menu_row(a, "Clear all sections") {
                    click(a, x, y);
                }
            }),
        ),
        Check(
            "no sections left; the + is back at bar 1",
            Box::new(|c, _| {
                let x = c.section_add_x();
                (c.project().sections.is_empty() && approx(x, c.view.x_of(0.0) + 8.5, 0.01))
                    .then_some(())
                    .ok_or(format!("{:?} add x {x}", names(c)))
            }),
        ),
    ]
}

/// Header minimap: bars, the window, drag / jump / fit / wheel (src/minimap.rs). Moves the view
/// and restores it at the end.
fn minimap_steps() -> Vec<Step> {
    use Step::*;
    fn strip_x() -> f32 {
        geo().timeline_x + 200.6
    }
    fn strip_y() -> f32 {
        geo().timeline_y + 5.1 + 8.5
    }
    let before = Rc::new(Cell::new((0.0f64, 0.0f64)));
    let moved_from = Rc::new(Cell::new((0.0f64, 0.0f64)));
    let (b1, b2, b3) = (before.clone(), moved_from.clone(), moved_from.clone());
    let landing = Rc::new(Cell::new(0.0f32));
    let (l1, l2) = (landing.clone(), landing.clone());
    let moved_start = moved_from.clone();
    fn width(a: &AppWindow) -> f32 {
        a.global::<Studio>().get_minimap_width()
    }
    fn scroll(a: &AppWindow, x: f32, dx: f32, dy: f32) {
        dispatch(
            a.window(),
            WindowEvent::PointerScrolled { position: LogicalPosition::new(strip_x() + x, strip_y()), delta_x: dx, delta_y: dy },
        );
    }
    vec![
        DoC(
            "minimap: view at beat 200",
            Box::new(move |_, ctl| {
                crate::pointer::take_log();
                let mut c = ctl.borrow_mut();
                b1.set((c.view.start_beats, c.view.px_per_beat));
                c.view.start_beats = 200.0;
                c.ui_dirty = true;
            }),
        ),
        Check(
            "the strip is laid out and its window sits at the end of the map",
            Box::new(|c, a| {
                let s = a.global::<Studio>();
                let span = c.lanes_size.0 as f64 / c.view.px_per_beat;
                let map = (200.0 + span).max(c.minimap_content_end_for_test());
                let (wx, ww) = (s.get_minimap_win_x(), s.get_minimap_win_w());
                let ok = width(a) > 100.0
                    && approx(wx, (200.0 / map) as f32, 1e-3)
                    && approx(ww, (span / map) as f32, 1e-3)
                    && s.get_minimap_bars().row_count() == c.clip_vis.iter().filter(|v| v.track_index < c.project().tracks.len()).count();
                ok.then_some(()).ok_or(format!("w {} win {wx} {ww} bars {}", width(a), s.get_minimap_bars().row_count()))
            }),
        ),
        DoC(
            "drag the window 40 px left",
            Box::new(move |a, ctl| {
                let (w, s) = (width(a), a.global::<Studio>());
                let x0 = (s.get_minimap_win_x() + s.get_minimap_win_w() / 2.0) * w;
                {
                    let c = ctl.borrow();
                    b2.set((c.view.start_beats, c.view.px_per_beat));
                }
                l1.set(strip_x() + x0);
                drag(a, strip_x() + x0, strip_y(), strip_x() + x0 - 40.0, strip_y());
            }),
        ),
        Check(
            "the drag hid the pointer and put it back on the grabbed spot (the window moved that far on the strip), same row, shown again",
            Box::new(move |c, a| {
                use crate::pointer::Op;
                let ops = crate::pointer::take_log();
                let (start0, _) = moved_start.get();
                let span = c.lanes_size.0 as f64 / c.view.px_per_beat;
                let map = (200.0 + span).max(c.minimap_content_end_for_test());
                let want = l2.get() - ((start0 - c.view.start_beats) / map) as f32 * a.global::<Studio>().get_minimap_width();
                let ok = matches!(ops.as_slice(), [Op::Hide, Op::Warp(x, y), Op::Show] if approx(*x, want, 2.0) && approx(*y, strip_y(), 1.0))
                    && !a.global::<Studio>().get_pointer_hidden();
                ok.then_some(()).ok_or(format!("{ops:?} want x {want}"))
            }),
        ),
        Check(
            "the view scrolled back (at most 40 px of map), at the same zoom",
            Box::new(move |c, a| {
                let (start0, ppb0) = b3.get();
                let w = width(a) as f64;
                let span = c.lanes_size.0 as f64 / c.view.px_per_beat;
                let max_move = 40.0 / w * (200.0 + span).max(16.0) + 0.05;
                let moved = start0 - c.view.start_beats;
                let ok = moved > 1.0 && moved <= max_move && approx((c.view.px_per_beat / ppb0) as f32, 1.0, 0.01) && c.mm.drag.is_none();
                ok.then_some(()).ok_or(format!("moved {moved} max {max_move} ppb {}", c.view.px_per_beat))
            }),
        ),
        DoC("click the strip's left end", Box::new(|a, _| click(a, strip_x() + 3.0, strip_y()))),
        Check(
            "the window jumped to the start of the song",
            Box::new(|c, _| approx(c.view.start_beats as f32, 0.0, 1e-3).then_some(()).ok_or(format!("start {}", c.view.start_beats))),
        ),
        DoC(
            "wheel over the strip scrolls",
            Box::new(|a, ctl| {
                ctl.borrow_mut().view.start_beats = 100.0;
                ctl.borrow_mut().ui_dirty = true;
                scroll(a, 20.0, 0.0, 30.0);
            }),
        ),
        Check(
            "the view moved back",
            Box::new(|c, _| {
                (c.view.start_beats < 99.5 && c.view.start_beats > 0.0).then_some(()).ok_or(format!("start {}", c.view.start_beats))
            }),
        ),
        DoC(
            "Ctrl + wheel zooms",
            Box::new(|a, ctl| {
                ctl.borrow_mut().view.px_per_beat = crate::timeline::DEFAULT_PX_PER_BEAT;
                dispatch(a.window(), WindowEvent::KeyPressed { text: Key::Control.into() });
                scroll(a, 20.0, 0.0, 40.0);
                dispatch(a.window(), WindowEvent::KeyReleased { text: Key::Control.into() });
            }),
        ),
        Check(
            "zoomed in",
            Box::new(|c, _| {
                (c.view.px_per_beat > crate::timeline::DEFAULT_PX_PER_BEAT * 1.05)
                    .then_some(())
                    .ok_or(format!("ppb {}", c.view.px_per_beat))
            }),
        ),
        DoC("double-click fits the arrangement", Box::new(|a, _| double_click(a, strip_x() + 30.0, strip_y()))),
        Check(
            "the view starts at 0 and shows the whole song plus slack",
            Box::new(|c, _| {
                let end = c.view.start_beats + c.lanes_size.0 as f64 / c.view.px_per_beat;
                let want = crate::minimap::fit(c.minimap_content_end_for_test()).end;
                (approx(c.view.start_beats as f32, 0.0, 1e-3) && approx(end as f32, want as f32, 0.05))
                    .then_some(())
                    .ok_or(format!("start {} end {end} want {want}", c.view.start_beats))
            }),
        ),
        DoC(
            "minimap: restore the view",
            Box::new(move |_, ctl| {
                let mut c = ctl.borrow_mut();
                let (s, p) = before.get();
                c.view.start_beats = s;
                c.view.px_per_beat = p;
                c.ui_dirty = true;
            }),
        ),
        Check(
            "view restored",
            Box::new(|c, _| (c.mm.drag.is_none() && c.view.start_beats < 1e6).then_some(()).ok_or("drag left open".to_string())),
        ),
    ]
}

/// Devices minimap: shown while the chain is wider than the panel; jump, drag (pointer hidden, put back), Esc,
/// wheel (src/devminimap.rs, src/pointer.rs). Selects the audio track, adds built-in effects until the chain
/// scrolls and removes them again.
fn devminimap_steps() -> Vec<Step> {
    use crate::devminimap as dm;
    use crate::pointer::Op;
    use Step::*;
    // The strip in the window: the bottom panel (x 5, 1430 wide) less 252.45 from its right edge; mid-height.
    fn strip_x() -> f32 {
        let g = geo();
        g.bottom_x + g.bottom_w - dm::X_FROM_RIGHT
    }
    fn strip_y() -> f32 {
        bottom_y() + dm::Y + 10.2
    }
    const CSS: f32 = crate::minimap::CSS;
    fn clear(a: &AppWindow, ctl: &Ctl) {
        let Some(i) = track_of(&ctl.borrow(), TrackKind::Audio) else { return };
        click_track_header(a, ctl, i);
        let mut c = ctl.borrow_mut();
        let ids: Vec<dstudio_core::DeviceId> = c.project().tracks[i].devices.iter().map(|d| d.id).collect();
        for d in ids {
            c.execute(dstudio_core::Command::RemoveDevice { device: d });
        }
        c.devices.chain_scroll = 0.0;
        c.update_ui(a);
    }
    fn window_mid(a: &AppWindow) -> f32 {
        let s = a.global::<Studio>();
        strip_x() + s.get_devmm_win_x() + s.get_devmm_win_w() / 2.0
    }
    fn wait_chain(c: &Controller) -> Result<&dm::Chain, String> {
        c.devmm_chain_for_test().ok_or("no chain".to_string())
    }
    let before = Rc::new(Cell::new(0.0f32));
    let press_at = Rc::new(Cell::new(0.0f32));
    let (b1, b2, p1, p2) = (before.clone(), before.clone(), press_at.clone(), press_at.clone());
    let first = Rc::new(Cell::new((false, 1.0f32, 0usize)));
    let first2 = first.clone();
    vec![
        DoC(
            "devices minimap: select the audio track, clear its chain, add effects until the chain scrolls",
            Box::new(|a, ctl| {
                key(a, Key::Escape, false, false);
                clear(a, ctl);
                let Some(i) = track_of(&ctl.borrow(), TrackKind::Audio) else { return };
                let mut c = ctl.borrow_mut();
                let t = c.project().tracks[i].id;
                c.set_bottom_tab(BottomTab::Devices);
                for id in
                    ["compressor", "channeleq", "delay", "reverb", "limiter", "gate", "glue-compressor", "phaser"].iter().cycle().take(16)
                {
                    c.update_ui(a);
                    if c.devmm_chain_for_test().is_some_and(|ch| ch.scrollable() && ch.window(0.0, dm::WIDTH).1 <= dm::WIDTH / 2.0) {
                        break;
                    }
                    if let Some(def) = dstudio_core::builtin::find(id) {
                        c.add_item_to_track(&crate::devices::Item::Builtin(def), t, None);
                    }
                }
                c.devices.chain_scroll = 0.0;
                c.update_ui(a);
                crate::pointer::take_log();
            }),
        ),
        Check(
            "devices minimap: shown, one tile per device, 227 reference px wide, the window at the start",
            Box::new(|c, a| {
                let s = a.global::<Studio>();
                let ch = wait_chain(c)?;
                let n = c.project().track(c.selected_track.ok_or("no track")?).map_or(0, |t| t.devices.len());
                let (_, w) = ch.window(0.0, dm::WIDTH);
                let ok = s.get_devmm_visible()
                    && ch.scrollable()
                    && approx(s.get_devmm_width(), dm::WIDTH * CSS, 0.05)
                    && s.get_devmm_tiles().row_count() == n
                    && approx(s.get_devmm_win_x(), 0.0, 0.01)
                    && approx(s.get_devmm_win_w(), w * CSS, 0.05);
                ok.then_some(()).ok_or(format!(
                    "visible {} width {} tiles {} of {n} window {} {}",
                    s.get_devmm_visible(),
                    s.get_devmm_width(),
                    s.get_devmm_tiles().row_count(),
                    s.get_devmm_win_x(),
                    s.get_devmm_win_w()
                ))
            }),
        ),
        Check(
            "devices minimap: every device tile carries its picture (neutral and accent paths) in the track colour",
            Box::new(|c, a| {
                let tiles = a.global::<Studio>().get_devmm_tiles();
                let pics = (0..tiles.row_count())
                    .filter_map(|i| tiles.row_data(i))
                    .filter(|t| !t.frame && !t.art.is_empty() && !t.art_accent.is_empty() && t.art.starts_with('M'))
                    .count();
                let n = c.project().track(c.selected_track.ok_or("no track")?).map_or(0, |t| t.devices.len());
                (pics == n && n > 0).then_some(()).ok_or(format!("{pics} pictures for {n} devices"))
            }),
        ),
        DoC("devices minimap: click the strip's far end (the window jumps there)", Box::new(|a, _| click(a, strip_x() + 190.0, strip_y()))),
        Check(
            "the chain scrolled so the window is centred on the click, and the pointer was not hidden",
            Box::new(|c, _| {
                let ch = wait_chain(c)?;
                let want = ch.scroll_at(190.0 / CSS, dm::WIDTH) * CSS;
                let ok = approx(c.devices.chain_scroll, want, 1.0)
                    && c.devices.chain_scroll > 10.0
                    && crate::pointer::take_log().is_empty()
                    && c.dm.drag.is_none();
                ok.then_some(()).ok_or(format!("scroll {} want {want}", c.devices.chain_scroll))
            }),
        ),
        DoC(
            "devices minimap: back to the start, drag the window 60 px right",
            Box::new(move |a, ctl| {
                {
                    let mut c = ctl.borrow_mut();
                    c.devices.chain_scroll = 0.0;
                    c.update_ui(a);
                }
                let x0 = window_mid(a);
                p1.set(x0);
                drag(a, x0, strip_y(), x0 + 60.0, strip_y());
            }),
        ),
        Check(
            "the chain followed the pointer one to one (in strip px) and the drag is over",
            Box::new(|c, _| {
                let ch = wait_chain(c)?;
                let want = 60.0 / ch.ratio(dm::WIDTH);
                let ok = approx(c.devices.chain_scroll, want, 2.0) && c.dm.drag.is_none();
                ok.then_some(()).ok_or(format!("scroll {} want {want}", c.devices.chain_scroll))
            }),
        ),
        Check(
            "the drag hid the pointer and put it back on the window's spot, same row, shown again",
            Box::new(move |_, a| {
                let ops = crate::pointer::take_log();
                let ok = matches!(ops.as_slice(), [Op::Hide, Op::Warp(x, y), Op::Show] if approx(*x, p2.get() + 60.0, 2.0) && approx(*y, strip_y(), 1.0))
                    && !a.global::<Studio>().get_pointer_hidden();
                ok.then_some(()).ok_or(format!("{ops:?} want x {}", p2.get() + 60.0))
            }),
        ),
        DoC(
            "devices minimap: press the window, drag 50 px, press Esc",
            Box::new(move |a, ctl| {
                let x0 = window_mid(a);
                b1.set(ctl.borrow().devices.chain_scroll);
                let w = a.window();
                press(a, x0, strip_y());
                for i in 1..=6 {
                    dispatch(w, WindowEvent::PointerMoved { position: LogicalPosition::new(x0 + 50.0 * i as f32 / 6.0, strip_y()) });
                }
                if let Ok(mut c) = ctl.try_borrow_mut() {
                    c.update_ui(a);
                }
                key(a, Key::Escape, false, false);
            }),
        ),
        Check(
            "Esc put the chain back, ended the drag and put the pointer on the pressed spot",
            Box::new(move |c, a| {
                let ops = crate::pointer::take_log();
                let ok = approx(c.devices.chain_scroll, b2.get(), 0.01)
                    && c.dm.drag.is_none()
                    && matches!(ops.as_slice(), [Op::Hide, Op::Warp(_, y), Op::Show] if approx(*y, strip_y(), 1.0))
                    && !a.global::<Studio>().get_pointer_hidden();
                ok.then_some(()).ok_or(format!("scroll {} was {} ops {ops:?}", c.devices.chain_scroll, b2.get()))
            }),
        ),
        DoC(
            "devices minimap: the held button keeps moving after Esc, then is released",
            Box::new(|a, _| {
                let w = a.window();
                dispatch(w, WindowEvent::PointerMoved { position: LogicalPosition::new(strip_x() + 150.0, strip_y()) });
                release(a, strip_x() + 150.0, strip_y());
            }),
        ),
        Check(
            "nothing scrolled after Esc",
            Box::new(move |c, _| {
                approx(c.devices.chain_scroll, before.get(), 0.01).then_some(()).ok_or(format!("scroll {}", c.devices.chain_scroll))
            }),
        ),
        DoC(
            "devices minimap: from the start, wheel over the strip",
            Box::new(|a, ctl| {
                {
                    let mut c = ctl.borrow_mut();
                    c.devices.chain_scroll = 0.0;
                    c.update_ui(a);
                }
                dispatch(
                    a.window(),
                    WindowEvent::PointerScrolled {
                        position: LogicalPosition::new(strip_x() + 80.0, strip_y()),
                        delta_x: 0.0,
                        delta_y: -20.0,
                    },
                );
            }),
        ),
        Check(
            "the wheel scrolled the chain forward by the same strip distance",
            Box::new(|c, _| {
                let want = 20.0 / wait_chain(c)?.ratio(dm::WIDTH);
                approx(c.devices.chain_scroll, want, 2.0).then_some(()).ok_or(format!("scroll {} want {want}", c.devices.chain_scroll))
            }),
        ),
        DoC(
            "devices minimap: let a running fade finish, then add one more effect",
            Box::new(move |a, ctl| {
                for _ in 0..40 {
                    if !ctl.borrow().devmm_animating() {
                        break;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(25));
                    ctl.borrow_mut().update_ui(a);
                }
                let mut c = ctl.borrow_mut();
                let Some(t) = c.selected_track else { return };
                if let Some(def) = dstudio_core::builtin::find("delay") {
                    c.add_item_to_track(&crate::devices::Item::Builtin(def), t, None);
                }
                c.update_ui(a);
                let tiles = a.global::<Studio>().get_devmm_tiles();
                let newest = tiles.row_data(tiles.row_count().saturating_sub(1)).map_or(1.0, |t| t.opacity);
                first.set((c.devmm_animating(), newest, tiles.row_count()));
            }),
        ),
        Check(
            "devices minimap: the new tile started transparent and the strip asked for redraws",
            Box::new(move |c, _| {
                let (animating, opacity, rows) = first2.get();
                let n = c.project().track(c.selected_track.ok_or("no track")?).map_or(0, |t| t.devices.len());
                (animating && opacity < 0.05 && rows == n)
                    .then_some(())
                    .ok_or(format!("animating {animating} newest opacity {opacity} tiles {rows} of {n}"))
            }),
        ),
        DoC(
            "devices minimap: wait for the glide to settle",
            Box::new(|a, ctl| {
                for _ in 0..80 {
                    std::thread::sleep(std::time::Duration::from_millis(25));
                    let mut c = ctl.borrow_mut();
                    c.update_ui(a);
                    if !c.devmm_animating() {
                        break;
                    }
                }
            }),
        ),
        Check(
            "devices minimap: at rest every tile is fully opaque and nothing keeps the strip redrawing",
            Box::new(|c, a| {
                let tiles = a.global::<Studio>().get_devmm_tiles();
                let dim = (0..tiles.row_count()).filter_map(|i| tiles.row_data(i)).filter(|t| t.opacity < 0.999).count();
                (!c.devmm_animating() && dim == 0).then_some(()).ok_or(format!("animating {} dim tiles {dim}", c.devmm_animating()))
            }),
        ),
        DoC("devices minimap: remove the effects again", Box::new(clear)),
        Check(
            "the chain is empty again and the strip is hidden",
            Box::new(|c, a| {
                let n = c.selected_track.and_then(|t| c.project().track(t)).map_or(1, |t| t.devices.len());
                (n == 0 && !a.global::<Studio>().get_devmm_visible() && c.devices.chain_scroll == 0.0)
                    .then_some(())
                    .ok_or(format!("devices {n}"))
            }),
        ),
    ]
}

/// Snap readout and menu in the timeline toolbar (src/snap.rs): Auto, a fixed size, Off.
fn snap_steps() -> Vec<Step> {
    use Step::*;
    vec![
        DoC("snap: back to Auto after the sections that pinned a beat", Box::new(|_, ctl| ctl.borrow_mut().snap.grid = None)),
        Check(
            "snap: the readout starts at Auto with snapping on",
            Box::new(|c, a| {
                (a.global::<Studio>().get_snap_text() == "Auto" && c.snap_active())
                    .then_some(())
                    .ok_or(format!("text {}", a.global::<Studio>().get_snap_text()))
            }),
        ),
        DoC("snap: open the menu from the chevron", Box::new(|_, ctl| ctl.borrow_mut().snap_menu_toggle(900.0, 40.0))),
        Check(
            "snap: the menu lists Off, Auto and ten sizes, Auto checked",
            Box::new(|_, a| {
                let s = a.global::<Studio>();
                let rows = s.get_snap_menu_items();
                let checked: Vec<usize> = (0..rows.row_count()).filter(|i| rows.row_data(*i).is_some_and(|r| r.checked)).collect();
                (s.get_snap_menu_open() && rows.row_count() == 12 && checked == [1]).then_some(()).ok_or(format!(
                    "open {} rows {} checked {checked:?}",
                    s.get_snap_menu_open(),
                    rows.row_count()
                ))
            }),
        ),
        DoC("snap: pick 1/16", Box::new(|a, _| a.global::<Studio>().invoke_snap_menu_pick(6))),
        Check(
            "snap: the readout says 1/16, the menu closed, snapping on",
            Box::new(|c, a| {
                let s = a.global::<Studio>();
                (s.get_snap_text() == "1/16" && !s.get_snap_menu_open() && c.snap_active()).then_some(()).ok_or(format!(
                    "text {} open {}",
                    s.get_snap_text(),
                    s.get_snap_menu_open()
                ))
            }),
        ),
        DoC(
            "snap: pick Off",
            Box::new(|a, ctl| {
                ctl.borrow_mut().snap_menu_toggle(900.0, 40.0);
                a.global::<Studio>().invoke_snap_menu_pick(0);
            }),
        ),
        Check(
            "snap: Off reads Off and drags move freely",
            Box::new(|c, a| {
                let t = a.global::<Studio>().get_snap_text();
                (t == "Off" && !c.snap_active()).then_some(()).ok_or(format!("text {t}"))
            }),
        ),
        DoC(
            "snap: toggle the readout back on, then pick Auto",
            Box::new(|a, ctl| {
                ctl.borrow_mut().snap_toggle();
                ctl.borrow_mut().snap_menu_toggle(900.0, 40.0);
                a.global::<Studio>().invoke_snap_menu_pick(1);
            }),
        ),
        Check(
            "snap: Auto again",
            Box::new(|c, a| {
                let t = a.global::<Studio>().get_snap_text();
                (t == "Auto" && c.snap_active()).then_some(()).ok_or(format!("text {t}"))
            }),
        ),
    ]
}

/// The transport bar's project fields: tempo drag (one undo step), typed tempo, tap tempo (one
/// undo step per series), the Sig menu and the key picker. Puts 120 BPM, 4/4 and no key back.
fn transport_fields_steps() -> Vec<Step> {
    use Step::*;
    let bpm = |c: &Controller| c.project().transport.bpm;
    vec![
        Check(
            "transport: starts at 120 BPM, 4/4, no key",
            Box::new(move |c, a| {
                let s = a.global::<Studio>();
                let t = &c.project().transport;
                (bpm(c) == 120.0
                    && (t.time_signature.numerator, t.time_signature.denominator) == (4, 4)
                    && t.key.is_none()
                    && s.get_bpm_text() == "120"
                    && s.get_key_text() == "")
                    .then_some(())
                    .ok_or(format!("bpm {} sig {}/{} key {:?}", bpm(c), t.time_signature.numerator, t.time_signature.denominator, t.key))
            }),
        ),
        Do(
            "transport: drag the tempo 25 points (right and up), pausing past the merge window",
            Box::new(|a| {
                let s = a.global::<Studio>();
                s.invoke_tempo_drag(0, 0.0, 0.0);
                s.invoke_tempo_drag(1, 10.0, 0.0);
                std::thread::sleep(std::time::Duration::from_millis(800));
                s.invoke_tempo_drag(1, 20.0, -5.0);
                s.invoke_tempo_drag(2, 20.0, -5.0);
            }),
        ),
        Check(
            "transport: 145 BPM, readout follows",
            Box::new(move |c, a| {
                let t = a.global::<Studio>().get_bpm_text();
                (bpm(c) == 145.0 && t == "145" && c.doc.undo_label() == Some("Tempo Change"))
                    .then_some(())
                    .ok_or(format!("bpm {} text {t}", bpm(c)))
            }),
        ),
        DoC("transport: one undo takes the whole drag back", Box::new(|_, ctl| ctl.borrow_mut().undo())),
        Check("transport: 120 BPM again", Box::new(move |c, _| (bpm(c) == 120.0).then_some(()).ok_or(format!("bpm {}", bpm(c))))),
        Do("transport: type 97.5", Box::new(|a| a.global::<Studio>().invoke_tempo_text("97.5".into()))),
        Check(
            "transport: 97.5 BPM shown with its decimal",
            Box::new(move |c, a| {
                let t = a.global::<Studio>().get_bpm_text();
                (bpm(c) == 97.5 && t == "97.5").then_some(()).ok_or(format!("bpm {} text {t}", bpm(c)))
            }),
        ),
        DoC(
            "transport: tap four times half a second apart",
            Box::new(|_, ctl| {
                let mut c = ctl.borrow_mut();
                for t in [500.0, 500.5, 501.0, 501.5] {
                    c.tap_tempo_at(t);
                }
            }),
        ),
        Check("transport: taps set 120 BPM", Box::new(move |c, _| (bpm(c) == 120.0).then_some(()).ok_or(format!("bpm {}", bpm(c))))),
        DoC("transport: one undo takes the tap series back", Box::new(|_, ctl| ctl.borrow_mut().undo())),
        Check("transport: 97.5 BPM again", Box::new(move |c, _| (bpm(c) == 97.5).then_some(()).ok_or(format!("bpm {}", bpm(c))))),
        DoC("transport: start a fresh tap series", Box::new(|_, ctl| ctl.borrow_mut().tf.taps.clear())),
        Do(
            "transport: press Shift+T, then hold it (two auto-repeats)",
            Box::new(|a| {
                let w = a.window();
                dispatch(w, WindowEvent::KeyPressed { text: Key::Shift.into() });
                dispatch(w, WindowEvent::KeyPressed { text: "T".into() });
                dispatch(w, WindowEvent::KeyPressRepeated { text: "T".into() });
                dispatch(w, WindowEvent::KeyPressRepeated { text: "T".into() });
                dispatch(w, WindowEvent::KeyReleased { text: "T".into() });
                dispatch(w, WindowEvent::KeyReleased { text: Key::Shift.into() });
            }),
        ),
        Check(
            "transport: the held key counted as one tap, the tempo is unchanged",
            Box::new(move |c, _| {
                (c.tf.taps.len() == 1 && bpm(c) == 97.5).then_some(()).ok_or(format!("taps {} bpm {}", c.tf.taps.len(), bpm(c)))
            }),
        ),
        DoC("transport: end that series", Box::new(|_, ctl| ctl.borrow_mut().tf.taps.clear())),
        DoC(
            "transport: tap at 120 BPM, then twice as fast",
            Box::new(|_, ctl| {
                let mut c = ctl.borrow_mut();
                for t in [600.0, 600.5, 601.0, 601.25] {
                    c.tap_tempo_at(t);
                }
            }),
        ),
        Check(
            "transport: the faster taps set 240 BPM",
            Box::new(move |c, _| (bpm(c) == 240.0).then_some(()).ok_or(format!("bpm {}", bpm(c)))),
        ),
        DoC("transport: undo once", Box::new(|_, ctl| ctl.borrow_mut().undo())),
        Check(
            "transport: the change of pace was its own step: 120 BPM",
            Box::new(move |c, _| (bpm(c) == 120.0).then_some(()).ok_or(format!("bpm {}", bpm(c)))),
        ),
        DoC("transport: undo again", Box::new(|_, ctl| ctl.borrow_mut().undo())),
        Check("transport: 97.5 BPM before the taps", Box::new(move |c, _| (bpm(c) == 97.5).then_some(()).ok_or(format!("bpm {}", bpm(c))))),
        Do(
            "transport: drag the tempo 3 points right from 97.5",
            Box::new(|a| {
                let s = a.global::<Studio>();
                s.invoke_tempo_drag(0, 0.0, 0.0);
                s.invoke_tempo_drag(1, 3.0, 0.0);
                s.invoke_tempo_drag(2, 3.0, 0.0);
            }),
        ),
        Check(
            "transport: the drag keeps the half beat: 100.5 BPM",
            Box::new(move |c, _| (bpm(c) == 100.5).then_some(()).ok_or(format!("bpm {}", bpm(c)))),
        ),
        DoC("transport: undo the drag", Box::new(|_, ctl| ctl.borrow_mut().undo())),
        Check(
            "transport: 97.5 BPM after the drag's undo",
            Box::new(move |c, _| (bpm(c) == 97.5).then_some(()).ok_or(format!("bpm {}", bpm(c)))),
        ),
        Do("transport: open the Sig menu", Box::new(|a| a.global::<Studio>().invoke_sig_menu_toggle())),
        Check(
            "transport: the Sig menu lists twelve signatures, 4/4 checked",
            Box::new(|_, a| {
                let s = a.global::<Studio>();
                let rows = s.get_sig_menu_items();
                let checked: Vec<String> =
                    (0..rows.row_count()).filter_map(|i| rows.row_data(i)).filter(|r| r.checked).map(|r| r.label.to_string()).collect();
                (s.get_sig_menu_open() && rows.row_count() == 12 && checked == ["4/4"]).then_some(()).ok_or(format!(
                    "open {} rows {} checked {checked:?}",
                    s.get_sig_menu_open(),
                    rows.row_count()
                ))
            }),
        ),
        Do("transport: pick 6/8", Box::new(|a| a.global::<Studio>().invoke_sig_menu_pick(8))),
        Check(
            "transport: 6/8, menu closed, bars are three beats",
            Box::new(|c, a| {
                let s = a.global::<Studio>();
                (s.get_sig_text() == "6/8" && !s.get_sig_menu_open() && c.project().ticks_per_bar() == 3 * dstudio_core::PPQ)
                    .then_some(())
                    .ok_or(format!("sig {} open {}", s.get_sig_text(), s.get_sig_menu_open()))
            }),
        ),
        Do(
            "transport: open the key picker, choose Minor, then A",
            Box::new(|a| {
                let s = a.global::<Studio>();
                s.invoke_key_menu_toggle();
                s.invoke_key_mode(true);
                s.invoke_key_pick(9);
            }),
        ),
        Check(
            "transport: A minor, readout Am, picker still open",
            Box::new(|c, a| {
                let s = a.global::<Studio>();
                let k = c.project().transport.key;
                (k == Some(dstudio_core::model::ProjectKey { root: 9, minor: true })
                    && s.get_key_text() == "Am"
                    && s.get_key_menu_open()
                    && s.get_key_root() == 9)
                    .then_some(())
                    .ok_or(format!("key {k:?} text {} open {}", s.get_key_text(), s.get_key_menu_open()))
            }),
        ),
        Do("transport: switch to Major", Box::new(|a| a.global::<Studio>().invoke_key_mode(false))),
        Check(
            "transport: A major",
            Box::new(|c, a| {
                let k = c.project().transport.key;
                (k == Some(dstudio_core::model::ProjectKey { root: 9, minor: false }) && a.global::<Studio>().get_key_text() == "A")
                    .then_some(())
                    .ok_or(format!("key {k:?}"))
            }),
        ),
        Do("transport: No key", Box::new(|a| a.global::<Studio>().invoke_key_clear())),
        Check(
            "transport: no key, dash shown, picker closed",
            Box::new(|c, a| {
                let s = a.global::<Studio>();
                (c.project().transport.key.is_none() && s.get_key_text() == "" && !s.get_key_menu_open())
                    .then_some(())
                    .ok_or(format!("key {:?}", c.project().transport.key))
            }),
        ),
        Do(
            "transport: open the Sig menu, Down, Enter",
            Box::new(|a| {
                a.global::<Studio>().invoke_sig_menu_toggle();
                key(a, Key::DownArrow, false, false);
            }),
        ),
        Check(
            "transport: the highlight moved from 6/8 to 7/8",
            Box::new(|_, a| {
                let s = a.global::<Studio>();
                (s.get_sig_menu_open() && s.get_sig_menu_active() == 9).then_some(()).ok_or(format!(
                    "open {} active {}",
                    s.get_sig_menu_open(),
                    s.get_sig_menu_active()
                ))
            }),
        ),
        Do("transport: Enter picks it", Box::new(|a| key(a, Key::Return, false, false))),
        Check(
            "transport: 7/8 from the keyboard, menu closed",
            Box::new(|_, a| {
                let s = a.global::<Studio>();
                (s.get_sig_text() == "7/8" && !s.get_sig_menu_open()).then_some(()).ok_or(format!(
                    "sig {} open {}",
                    s.get_sig_text(),
                    s.get_sig_menu_open()
                ))
            }),
        ),
        Do(
            "transport: key picker from the keyboard: Right, Right, Down, Enter, M",
            Box::new(|a| {
                a.global::<Studio>().invoke_key_menu_toggle();
                key(a, Key::RightArrow, false, false);
                key(a, Key::RightArrow, false, false);
                key(a, Key::DownArrow, false, false);
                key(a, Key::Return, false, false);
                key(a, "m", false, false);
            }),
        ),
        Check(
            "transport: F# minor, picker open",
            Box::new(|c, a| {
                let s = a.global::<Studio>();
                let k = c.project().transport.key;
                (k == Some(dstudio_core::model::ProjectKey { root: 6, minor: true }) && s.get_key_text() == "F#m" && s.get_key_menu_open())
                    .then_some(())
                    .ok_or(format!("key {k:?} open {}", s.get_key_menu_open()))
            }),
        ),
        Do("transport: Backspace is No key", Box::new(|a| key(a, Key::Backspace, false, false))),
        Check(
            "transport: no key after Backspace, picker closed",
            Box::new(|c, a| {
                (c.project().transport.key.is_none() && !a.global::<Studio>().get_key_menu_open())
                    .then_some(())
                    .ok_or(format!("key {:?}", c.project().transport.key))
            }),
        ),
        Do("transport: type 120slow", Box::new(|a| a.global::<Studio>().invoke_tempo_text("120slow".into()))),
        Check(
            "transport: text after the number is refused, still 97.5",
            Box::new(move |c, _| (bpm(c) == 97.5).then_some(()).ok_or(format!("bpm {}", bpm(c)))),
        ),
        DoC(
            "transport: three taps at 120, type 97.5, one more tap",
            Box::new(|_, ctl| {
                let mut c = ctl.borrow_mut();
                for t in [700.0, 700.5, 701.0] {
                    c.tap_tempo_at(t);
                }
                c.tempo_text("97.5");
                c.tap_tempo_at(701.5);
            }),
        ),
        Check(
            "transport: typing ended the tap series: still 97.5",
            Box::new(move |c, _| (bpm(c) == 97.5).then_some(()).ok_or(format!("bpm {}", bpm(c)))),
        ),
        DoC("transport: type 97.5 again (ends the series)", Box::new(|_, ctl| ctl.borrow_mut().tempo_text("97.5"))),
        // The key press is checked for routing only; the second tap gets a timestamp half a
        // second after it, so the tempo does not depend on how fast the runner is.
        Do("transport: Shift+T", Box::new(|a| key(a, "T", false, true))),
        Check(
            "transport: Shift+T started a tap series, the tempo is unchanged",
            Box::new(move |c, _| {
                (c.tf.taps.len() == 1 && bpm(c) == 97.5).then_some(()).ok_or(format!("taps {} bpm {}", c.tf.taps.len(), bpm(c)))
            }),
        ),
        DoC(
            "transport: a second tap half a second after the key press",
            Box::new(|_, ctl| {
                let mut c = ctl.borrow_mut();
                let t = c.tf.taps.first().copied().unwrap_or(0.0) + 0.5;
                c.tap_tempo_at(t);
            }),
        ),
        Check(
            "transport: the two taps set 120 BPM",
            Box::new(move |c, _| (bpm(c) == 120.0).then_some(()).ok_or(format!("bpm {}", bpm(c)))),
        ),
        DoC(
            "transport: back to 97.5, then Settings open with the Sig menu",
            Box::new(|_, ctl| {
                let mut c = ctl.borrow_mut();
                c.tempo_text("97.5");
                c.sig_menu_toggle();
                c.toggle_settings();
            }),
        ),
        Do(
            "transport: Down and Enter behind Settings",
            Box::new(|a| {
                key(a, Key::DownArrow, false, false);
                key(a, Key::Return, false, false);
            }),
        ),
        Check(
            "transport: the signature did not change behind Settings",
            Box::new(|_, a| {
                let s = a.global::<Studio>();
                (s.get_sig_text() == "7/8").then_some(()).ok_or(format!("sig {}", s.get_sig_text()))
            }),
        ),
        DoC(
            "transport: close Settings and the menu",
            Box::new(|_, ctl| {
                let mut c = ctl.borrow_mut();
                c.toggle_settings();
                c.transport_menus_dismiss();
            }),
        ),
        DoC(
            "transport: back to 120 BPM and 4/4",
            Box::new(|_, ctl| {
                let mut c = ctl.borrow_mut();
                c.execute(dstudio_core::Command::SetTempo { bpm: 120.0 });
                c.execute(dstudio_core::Command::SetTimeSignature { numerator: 4, denominator: 4 });
            }),
        ),
    ]
}
/// Group (folder) tracks: Cmd+G groups the selected track, the header shows the fold chevron and
/// its track's inset, folding hides the track's row, a track joins from the Out menu, the group
/// moves with its tracks, and Ungroup sends them back to the master. All undone at the end.
fn group_steps() -> Vec<Step> {
    use Step::*;
    fn group(c: &Controller) -> Option<&dstudio_core::Track> {
        c.project().tracks.iter().find(|t| t.is_group())
    }
    fn row(a: &AppWindow, i: usize) -> Option<crate::TrackRow> {
        slint::Model::row_data(&a.global::<Studio>().get_tracks(), i)
    }
    fn names(c: &Controller) -> Vec<String> {
        c.project().tracks.iter().map(|t| t.name.clone()).collect()
    }
    let hist = Rc::new(Cell::new(0usize));
    let (h1, h2) = (hist.clone(), hist.clone());
    vec![
        DoC(
            "groups: select track 1, press Cmd+G",
            Box::new(move |a, ctl| {
                {
                    let mut c = ctl.borrow_mut();
                    h1.set(c.doc.history_labels().len());
                    let t = c.project().tracks[0].id;
                    c.select_track(t);
                }
                key(a, "g", true, false);
            }),
        ),
        Check(
            "groups: one Group Tracks step; Group above track 1, which plays into it; the inspector shows a group",
            Box::new(|c, a| {
                let g = group(c).ok_or(format!("no group; tracks {:?}", names(c)))?;
                let t1 = &c.project().tracks[1];
                let (r0, r1) = (row(a, 0).ok_or("no row 0")?, row(a, 1).ok_or("no row 1")?);
                let s = a.global::<Studio>();
                (c.doc.undo_label() == Some("Group Tracks")
                    && c.project().tracks[0].id == g.id
                    && g.name == "Group"
                    && t1.output == Some(g.id)
                    && r0.is_group
                    && !r0.in_group
                    && r1.in_group
                    && c.selected_track == Some(g.id)
                    && s.get_sel_group()
                    && !s.get_sel_return())
                .then_some(())
                .ok_or(format!(
                    "undo {:?} tracks {:?} out {:?} rows ({}, {}) sel-group {}",
                    c.doc.undo_label(),
                    names(c),
                    t1.output,
                    r0.is_group,
                    r1.in_group,
                    s.get_sel_group()
                ))
            }),
        ),
        DoC(
            "groups: fold the group (its chevron)",
            Box::new(|a, ctl| {
                let id = group(&ctl.borrow()).map(|g| g.id.0 as i32).unwrap_or(-1);
                a.global::<Studio>().invoke_group_fold(id);
            }),
        ),
        Check(
            "groups: folded, the group's track has no row height and the group stays selected",
            Box::new(|c, a| {
                let (r0, r1) = (row(a, 0).ok_or("no row 0")?, row(a, 1).ok_or("no row 1")?);
                (r0.folded && r1.h == 0.0 && c.view.pitch(1) == 0.0 && c.selected_track == group(c).map(|g| g.id))
                    .then_some(())
                    .ok_or(format!("folded {} h {} pitch {}", r0.folded, r1.h, c.view.pitch(1)))
            }),
        ),
        DoC(
            "groups: unfold, route the next ungrouped track into the group (Out menu)",
            Box::new(|a, ctl| {
                let id = group(&ctl.borrow()).map(|g| g.id).unwrap_or(dstudio_core::TrackId(0));
                a.global::<Studio>().invoke_group_fold(id.0 as i32);
                let mut c = ctl.borrow_mut();
                if let Some(t) = c.project().tracks.iter().find(|t| !t.is_bus() && t.output.is_none()).map(|t| t.id) {
                    c.route_to_group(t, Some(id));
                }
            }),
        ),
        Check(
            "groups: both tracks play into the group, right below it",
            Box::new(|c, a| {
                let g = group(c).ok_or("no group")?;
                let p = c.project();
                let ok = p.tracks[1].output == Some(g.id)
                    && p.tracks[2].output == Some(g.id)
                    && p.group_members(g.id).count() == 2
                    && c.doc.undo_label() == Some("Output Change")
                    && row(a, 2).is_some_and(|r| r.in_group && r.h > 0.0);
                ok.then_some(()).ok_or(format!("tracks {:?} undo {:?}", names(c), c.doc.undo_label()))
            }),
        ),
        DoC(
            "groups: add an audio track at the end, drag the group header below it",
            Box::new(|_, ctl| {
                let mut c = ctl.borrow_mut();
                let t = dstudio_core::Track::new(dstudio_core::TrackId(0), TrackKind::Audio, "Below", dstudio_core::ColorIndex(2));
                c.execute(dstudio_core::Command::AddTrack { index: None, track: t });
                let end = c.project().ordinary_track_count();
                if let Some(g) = group(&c).map(|g| g.id) {
                    c.move_group(g, end);
                }
            }),
        ),
        Check(
            "groups: the group moved with its two tracks (one Move Track step)",
            Box::new(|c, _| {
                let p = c.project();
                let g = group(c).ok_or("no group")?;
                let n = p.ordinary_track_count();
                (c.doc.undo_label() == Some("Move Track")
                    && n >= 4
                    && p.tracks[n - 3].id == g.id
                    && p.tracks[n - 2].output == Some(g.id)
                    && p.tracks[n - 1].output == Some(g.id)
                    && p.tracks[n - 4].name == "Below")
                    .then_some(())
                    .ok_or(format!("tracks {:?} undo {:?}", names(c), c.doc.undo_label()))
            }),
        ),
        DoC(
            "groups: Ungroup",
            Box::new(|_, ctl| {
                let mut c = ctl.borrow_mut();
                if let Some(g) = group(&c).map(|g| g.id) {
                    c.ungroup(g);
                }
            }),
        ),
        Check(
            "groups: the group is gone and its tracks play into the master",
            Box::new(|c, _| {
                (c.doc.undo_label() == Some("Ungroup")
                    && group(c).is_none()
                    && c.project().tracks.iter().all(|t| t.output.is_none_or(|o| c.project().track(o).is_some_and(|r| r.is_return()))))
                .then_some(())
                .ok_or(format!("tracks {:?} undo {:?}", names(c), c.doc.undo_label()))
            }),
        ),
        DoC(
            "groups: undo back to the start",
            Box::new(move |_, ctl| {
                let mut c = ctl.borrow_mut();
                for _ in h2.get()..c.doc.history_labels().len() {
                    c.undo();
                }
            }),
        ),
        Check(
            "groups: no group left after undo",
            Box::new(move |c, _| {
                (group(c).is_none() && c.doc.history_labels().len() == hist.get()).then_some(()).ok_or(format!(
                    "tracks {:?} history {}",
                    names(c),
                    c.doc.history_labels().len()
                ))
            }),
        ),
    ]
}
/// Zoom to fit: W fits the song, then the selected clip, to the lanes' width; H sizes the
/// track rows to fill the lanes' height. The view and row heights are put back at the end.
fn zoom_fit_steps() -> Vec<Step> {
    use Step::*;
    type Saved = (f64, f64, f64, Vec<(dstudio_core::TrackId, f32)>);
    let saved: Rc<std::cell::RefCell<Option<Saved>>> = Rc::new(std::cell::RefCell::new(None));
    let (s1, s2) = (saved.clone(), saved.clone());
    fn span_ok(c: &Controller, range: (f64, f64)) -> Result<(), String> {
        let w = c.lanes_size.0;
        let (a, b) = (c.view.x_of(range.0), c.view.x_of(range.1));
        // Inside the lanes and filling most of them.
        (a >= -0.5 && b <= w + 0.5 && b - a > 0.9 * w)
            .then_some(())
            .ok_or(format!("range {range:?} at x {a}..{b} of {w}, start {} ppb {}", c.view.start_beats, c.view.px_per_beat))
    }
    vec![
        DoC(
            "zoom fit: zoom far in and scroll to beat 40, then press W",
            Box::new(move |a, ctl| {
                {
                    let mut c = ctl.borrow_mut();
                    let heights = c.project().tracks.iter().map(|t| (t.id, t.height)).collect();
                    *s1.borrow_mut() = Some((c.view.start_beats, c.view.px_per_beat, c.view.scroll_y, heights));
                    c.select_clip(None);
                    c.view.px_per_beat = 400.0;
                    c.view.start_beats = 40.0;
                    c.ui_dirty = true;
                }
                key(a, "w", false, false);
            }),
        ),
        Check(
            "zoom fit: the whole song spans the lanes",
            Box::new(|c, _| {
                let end = crate::minimap::content_end(&c.clip_vis);
                if end <= 0.0 {
                    return Err("no clips in the project".into());
                }
                span_ok(c, (0.0, end))
            }),
        ),
        DoC(
            "zoom fit: select the first clip, press W",
            Box::new(|a, ctl| {
                {
                    let mut c = ctl.borrow_mut();
                    let id = c
                        .project()
                        .tracks
                        .iter()
                        .flat_map(|t| t.clips.iter().map(|cl| cl.id).chain(t.midi_clips.iter().map(|m| m.id)))
                        .next();
                    c.select_clip(id);
                    c.rebuild_vis();
                }
                key(a, "w", false, false);
            }),
        ),
        Check(
            "zoom fit: the selected clip spans the lanes",
            Box::new(|c, _| {
                let sel: Vec<_> = c.clip_vis.iter().filter(|v| v.selected).collect();
                let [v] = sel[..] else { return Err(format!("{} clips selected", sel.len())) };
                span_ok(c, (v.start_beats, v.start_beats + v.len_beats.max(4.0)))
            }),
        ),
        Do("zoom fit: press H", Box::new(|a| key(a, "h", false, false))),
        Check(
            "zoom fit: every shown row has the same height and together they fill the lanes",
            Box::new(|c, _| {
                let p = c.project();
                let n = p.tracks.len().min(c.view.pinned_from);
                let shown: Vec<_> = (0..n).filter(|&i| !c.hidden_by_fold(&p.tracks[i])).collect();
                let h0 = p.tracks[shown[0]].height;
                let same = shown.iter().all(|&i| p.tracks[i].height == h0);
                let (lo, hi) = dstudio_core::Track::HEIGHT_RANGE;
                let bottom = c.view.track_top(n.saturating_sub(1)) + c.view.track_span(n.saturating_sub(1));
                let limit = c.lanes_size.1 - c.view.footer_h();
                let fills = h0 == lo || h0 == hi || (bottom <= limit + 0.5 && bottom > limit - 2.0 * shown.len() as f32);
                (same && fills && c.view.scroll_y == 0.0).then_some(()).ok_or(format!(
                    "heights {:?} bottom {bottom} limit {limit}",
                    shown.iter().map(|&i| p.tracks[i].height).collect::<Vec<_>>()
                ))
            }),
        ),
        DoC(
            "zoom fit: put the view and row heights back",
            Box::new(move |_, ctl| {
                let mut c = ctl.borrow_mut();
                if let Some((start, ppb, scroll, heights)) = s2.borrow_mut().take() {
                    c.view.start_beats = start;
                    c.view.px_per_beat = ppb;
                    c.view.scroll_y = scroll;
                    for (id, h) in heights {
                        let _ = c.doc.set_track_height(id, h);
                    }
                }
                c.select_clip(None);
                c.ui_dirty = true;
            }),
        ),
    ]
}

/// Track rename: double-click the selected track's name in the inspector, type, Enter. An
/// empty name and Escape leave the name alone; undo restores it.
/// Track header menu and reorder drag (src/track_menu.rs).
fn track_menu_steps() -> Vec<Step> {
    use Step::*;
    fn right_click(a: &AppWindow, x: f32, y: f32) {
        let w = a.window();
        let p = LogicalPosition::new(x, y);
        dispatch(w, WindowEvent::PointerMoved { position: p });
        dispatch(w, WindowEvent::PointerPressed { position: p, button: PointerEventButton::Right });
        dispatch(w, WindowEvent::PointerReleased { position: p, button: PointerEventButton::Right });
    }
    fn menu_row(a: &AppWindow, label: &str) -> Option<(f32, f32)> {
        let s = a.global::<Studio>();
        let items = s.get_menu_items();
        (0..slint::Model::row_count(&items))
            .filter_map(|i| slint::Model::row_data(&items, i))
            .find(|m| m.label == label && m.enabled)
            .map(|m| (s.get_menu_x() + 40.0, s.get_menu_y() + m.y + crate::clipedit::MENU_ROW / 2.0))
    }
    fn sub_row(a: &AppWindow, label: &str) -> Option<(f32, f32)> {
        let s = a.global::<Studio>();
        let items = s.get_sub_items();
        (0..slint::Model::row_count(&items)).filter_map(|i| slint::Model::row_data(&items, i)).find(|m| m.label == label).map(|m| {
            (s.get_menu_x() + crate::clipedit::MENU_W - 4.0 + 40.0, s.get_menu_y() + s.get_sub_y() + m.y + crate::clipedit::MENU_ROW / 2.0)
        })
    }
    // Middle of track `i`'s header row, over its number (no control there).
    fn row_xy(c: &Controller, i: usize) -> (f32, f32) {
        (tracks_x() + 15.0, track0_y() + c.view.track_offset(i) + c.view.pitch(i) / 2.0 - c.view.scroll_y as f32)
    }
    fn ids(c: &Controller) -> Vec<u64> {
        c.project().tracks.iter().map(|t| t.id.0).collect()
    }
    let before: Rc<RefCell<Vec<u64>>> = Rc::new(RefCell::new(Vec::new()));
    let (b0, b1, b2, b3, b4) = (before.clone(), before.clone(), before.clone(), before.clone(), before.clone());
    vec![
        DoC(
            "track menu: mixer closed, lanes scrolled to the top, two ordinary tracks",
            Box::new(move |_, ctl| {
                let mut c = ctl.borrow_mut();
                if c.mixer_open {
                    c.toggle_mixer();
                }
                if c.project().ordinary_track_count() < 2 {
                    c.add_audio_track();
                }
                c.view.scroll_y = 0.0;
                c.ui_dirty = true;
                *b0.borrow_mut() = ids(&c);
            }),
        ),
        DoC(
            "track menu: right-click track 1's header",
            Box::new(|a, ctl| {
                let (x, y) = row_xy(&ctl.borrow(), 0);
                right_click(a, x, y);
            }),
        ),
        Check(
            "track menu: Rename, Duplicate track, Duplicate track with clips, Color, Delete track; track 1 selected",
            Box::new(|c, a| {
                let labels = ["Rename…", "Duplicate track", "Duplicate track with clips", "Color", "Delete track"];
                let ok = a.global::<Studio>().get_menu_open()
                    && labels.iter().all(|l| menu_row(a, l).is_some())
                    && c.selected_track == c.project().tracks.first().map(|t| t.id);
                ok.then_some(()).ok_or(format!("open {} selected {:?}", a.global::<Studio>().get_menu_open(), c.selected_track))
            }),
        ),
        Do(
            "track menu: choose Duplicate track with clips",
            Box::new(|a| {
                if let Some((x, y)) = menu_row(a, "Duplicate track with clips") {
                    click(a, x, y);
                }
            }),
        ),
        Check(
            "track menu: the copy sits below track 1 with its name, colour and clips, selected, one undo step",
            Box::new(move |c, _| {
                let p = c.project();
                let n = b1.borrow().len();
                let (src, copy) = (&p.tracks[0], &p.tracks[1]);
                let ok = p.tracks.len() == n + 1
                    && copy.id != src.id
                    && copy.name == src.name
                    && copy.color == src.color
                    && copy.clips.len() == src.clips.len()
                    && copy.midi_clips.len() == src.midi_clips.len()
                    && c.selected_track == Some(copy.id)
                    && c.doc.undo_label() == Some("Duplicate Track with Clips");
                ok.then_some(()).ok_or(format!("{} tracks (was {n}), undo {:?}", p.tracks.len(), c.doc.undo_label()))
            }),
        ),
        Do("track menu: Cmd+Z", Box::new(|a| key(a, "z", true, false))),
        Check(
            "track menu: undo removes the copy",
            Box::new(move |c, _| (ids(c) == *b2.borrow()).then_some(()).ok_or(format!("{:?}", ids(c)))),
        ),
        DoC(
            "track menu: right-click track 1, open Color",
            Box::new(|a, ctl| {
                let (x, y) = row_xy(&ctl.borrow(), 0);
                right_click(a, x, y);
                if let Some((x, y)) = menu_row(a, "Color") {
                    click(a, x, y);
                }
            }),
        ),
        Check(
            "track menu: the Color submenu lists 13 swatches",
            Box::new(|_, a| {
                let s = a.global::<Studio>();
                let items = s.get_sub_items();
                let n = slint::Model::row_count(&items);
                let swatches = (0..n).filter_map(|i| slint::Model::row_data(&items, i)).all(|m| m.swatch != slint::Color::default());
                (s.get_sub_open() && n == 13 && swatches).then_some(()).ok_or(format!("open {} rows {n}", s.get_sub_open()))
            }),
        ),
        Do(
            "track menu: pick Teal",
            Box::new(|a| {
                if let Some((x, y)) = sub_row(a, "Teal") {
                    click(a, x, y);
                }
            }),
        ),
        Check(
            "track menu: track 1 is teal (palette 6), one undo step, menu closed",
            Box::new(|c, a| {
                let col = c.project().tracks[0].color.0;
                (col == 6 && c.doc.undo_label() == Some("Track Color Change") && !a.global::<Studio>().get_menu_open())
                    .then_some(())
                    .ok_or(format!("colour {col} undo {:?}", c.doc.undo_label()))
            }),
        ),
        Do("track menu: Cmd+Z the colour", Box::new(|a| key(a, "z", true, false))),
        DoC(
            "reorder: drag track 1's header below track 2",
            Box::new(|a, ctl| {
                let ((x, y0), (_, y1)) = {
                    let c = ctl.borrow();
                    (row_xy(&c, 0), row_xy(&c, 1))
                };
                drag(a, x, y0, x, y1 + crate::timeline::TRACK_PITCH * 0.3);
            }),
        ),
        Check(
            "reorder: track 1 moved to second place (Move Track), the drop line gone",
            Box::new(move |c, a| {
                let b = b3.borrow();
                let now = ids(c);
                (now[0] == b[1] && now[1] == b[0] && c.doc.undo_label() == Some("Move Track") && !a.global::<Studio>().get_track_drop_on())
                    .then_some(())
                    .ok_or(format!("{now:?} (was {b:?}) undo {:?}", c.doc.undo_label()))
            }),
        ),
        Do("reorder: Cmd+Z", Box::new(|a| key(a, "z", true, false))),
        Check(
            "reorder: undo puts the tracks back in order",
            Box::new(move |c, _| (ids(c) == *b4.borrow()).then_some(()).ok_or(format!("{:?}", ids(c)))),
        ),
    ]
}

fn track_rename_steps() -> Vec<Step> {
    use Step::*;
    let name0 = |c: &Controller| c.project().tracks.first().map(|t| t.name.clone()).unwrap_or_default();
    let name_xy = || (bottom_x() + 60.0, bottom_y() + 14.25);
    let original: Rc<RefCell<String>> = Rc::new(RefCell::new(String::new()));
    let (o0, o1, o2, o3) = (original.clone(), original.clone(), original.clone(), original.clone());
    let e0 = Rc::new(std::cell::Cell::new(0));
    let e1 = e0.clone();
    let rev0 = Rc::new(std::cell::Cell::new(0u64));
    let rev1 = rev0.clone();
    vec![
        DoC(
            "rename: select track 1, Devices tab, mixer closed",
            Box::new(move |_, ctl| {
                let mut c = ctl.borrow_mut();
                *o0.borrow_mut() = c.project().tracks[0].name.clone();
                if c.mixer_open {
                    c.toggle_mixer();
                }
                c.set_bottom_tab(crate::controller::BottomTab::Devices);
                let id = c.project().tracks[0].id;
                c.select_track(id);
            }),
        ),
        Do(
            "rename: double-click the name in the inspector",
            Box::new(move |a| {
                let (x, y) = name_xy();
                double_click(a, x, y);
            }),
        ),
        Do(
            "rename: type a new name and press Enter",
            Box::new(|a| {
                type_text(a, "Lead vox");
                key(a, Key::Return, false, false);
            }),
        ),
        Check(
            "rename: track 1 is Lead vox, one undo step, shown in the inspector",
            Box::new(move |c, a| {
                let n = name0(c);
                (n == "Lead vox" && c.doc.undo_label() == Some("Rename Track") && a.global::<Studio>().get_sel_name() == "Lead vox")
                    .then_some(())
                    .ok_or(format!("name {n:?} undo {:?}", c.doc.undo_label()))
            }),
        ),
        Do(
            "rename: double-click, clear the name, Enter",
            Box::new(move |a| {
                let (x, y) = name_xy();
                double_click(a, x, y);
            }),
        ),
        Do(
            "rename: an empty name",
            Box::new(|a| {
                key(a, Key::Backspace, false, false);
                key(a, Key::Return, false, false);
            }),
        ),
        Check(
            "rename: an empty name keeps Lead vox",
            Box::new(move |c, _| (name0(c) == "Lead vox").then_some(()).ok_or(format!("name {:?}", name0(c)))),
        ),
        Do(
            "rename: double-click, type, Escape",
            Box::new(move |a| {
                let (x, y) = name_xy();
                double_click(a, x, y);
            }),
        ),
        Do(
            "rename: Escape cancels",
            Box::new(|a| {
                type_text(a, "Nope");
                key(a, Key::Escape, false, false);
            }),
        ),
        Check("rename: still Lead vox", Box::new(move |c, _| (name0(c) == "Lead vox").then_some(()).ok_or(format!("name {:?}", name0(c))))),
        Do(
            "rename: rename through the callback with spaces around",
            Box::new(|a| {
                let s = a.global::<Studio>();
                s.invoke_track_rename(s.get_sel_id(), "  Bass  ".into(), s.get_doc_epoch());
            }),
        ),
        Check("rename: trimmed to Bass", Box::new(move |c, _| (name0(c) == "Bass").then_some(()).ok_or(format!("name {:?}", name0(c))))),
        Do(
            "rename: double-click, type Drums, then press in the lanes",
            Box::new(move |a| {
                let (x, y) = name_xy();
                double_click(a, x, y);
                type_text(a, "Drums");
                let p = lanes_point();
                click(a, p.x, p.y);
            }),
        ),
        Do("rename: let the focus move", Box::new(|_| {})),
        Check(
            "rename: the press in the lanes committed Drums",
            Box::new(move |c, _| (name0(c) == "Drums").then_some(()).ok_or(format!("name {:?}", name0(c)))),
        ),
        DoC(
            "rename: select track 1 again",
            Box::new(|_, ctl| {
                let mut c = ctl.borrow_mut();
                c.set_bottom_tab(crate::controller::BottomTab::Devices);
                let id = c.project().tracks[0].id;
                c.select_track(id);
            }),
        ),
        Do(
            "rename: double-click, type Keys, then select track 2",
            Box::new(move |a| {
                let (x, y) = name_xy();
                double_click(a, x, y);
                type_text(a, "Keys");
            }),
        ),
        DoC(
            "rename: select track 2 while the field is open",
            Box::new(|_, ctl| {
                let mut c = ctl.borrow_mut();
                let id = c.project().tracks[1].id;
                c.select_track(id);
            }),
        ),
        Do("rename: let the field see the new selection", Box::new(|_| {})),
        Check(
            "rename: Keys went to track 1, track 2 kept its name",
            Box::new(move |c, _| {
                let t = &c.project().tracks;
                (t[0].name == "Keys" && t[1].name != "Keys").then_some(()).ok_or(format!("names {:?} / {:?}", t[0].name, t[1].name))
            }),
        ),
        DoC(
            "rename: select track 1, Devices tab",
            Box::new(|_, ctl| {
                let mut c = ctl.borrow_mut();
                c.set_bottom_tab(crate::controller::BottomTab::Devices);
                let id = c.project().tracks[0].id;
                c.select_track(id);
            }),
        ),
        Do(
            "rename: double-click, type Pads, then click the Clip tab",
            Box::new(move |a| {
                let (x, y) = name_xy();
                double_click(a, x, y);
                type_text(a, "Pads");
                click(a, bottom_x() + 261.0 + 8.0, bottom_y() + 14.0);
            }),
        ),
        Check(
            "rename: switching tabs committed Pads",
            Box::new(move |c, _| {
                (name0(c) == "Pads" && c.bottom_tab == crate::controller::BottomTab::Clip && c.rename_draft.is_none())
                    .then_some(())
                    .ok_or(format!("name {:?} tab {:?}", name0(c), c.bottom_tab))
            }),
        ),
        DoC(
            "rename: track 1 selected, no clip",
            Box::new(|_, ctl| {
                let mut c = ctl.borrow_mut();
                let id = c.project().tracks[0].id;
                c.select_track(id);
                c.selected_clip = None;
                c.ui_dirty = true;
            }),
        ),
        Do("rename: ⌘R (track.rename)", Box::new(|a| key(a, "r", true, false))),
        Do(
            "rename: type Strings and press Enter",
            Box::new(|a| {
                type_text(a, "Strings");
                key(a, Key::Return, false, false);
            }),
        ),
        Check(
            "rename: the keyboard opened the header field and Strings was committed",
            Box::new(move |c, _| {
                (name0(c) == "Strings" && c.doc.undo_label() == Some("Rename Track")).then_some(()).ok_or(format!(
                    "name {:?} undo {:?}",
                    name0(c),
                    c.doc.undo_label()
                ))
            }),
        ),
        DoC(
            "rename: undo six times",
            Box::new(|_, ctl| {
                for _ in 0..6 {
                    ctl.borrow_mut().undo();
                }
            }),
        ),
        Check(
            "rename: the original name is back",
            Box::new(move |c, _| (name0(c) == *o1.borrow()).then_some(()).ok_or(format!("name {:?} want {:?}", name0(c), o1.borrow()))),
        ),
        Do(
            "rename: a field opened before the project was replaced types and commits",
            Box::new(|a| {
                let s = a.global::<Studio>();
                let stale = s.get_doc_epoch() - 1;
                s.invoke_track_rename_draft(s.get_sel_id(), "Stale".into(), stale);
                s.invoke_track_rename(s.get_sel_id(), "Stale".into(), stale);
            }),
        ),
        Check(
            "rename: the stale field renamed nothing and left no draft",
            Box::new(move |c, _| {
                (name0(c) == *o2.borrow() && c.rename_draft.is_none()).then_some(()).ok_or(format!(
                    "name {:?} draft {:?}",
                    name0(c),
                    c.rename_draft
                ))
            }),
        ),
        DoC(
            "rename: track 1 selected, Devices tab (again)",
            Box::new(move |_, ctl| {
                let mut c = ctl.borrow_mut();
                c.set_bottom_tab(crate::controller::BottomTab::Devices);
                let id = c.project().tracks[0].id;
                c.select_track(id);
                rev0.set(c.doc.revision());
            }),
        ),
        Do(
            "rename: double-click the name and type nothing",
            Box::new(move |a| {
                let (x, y) = name_xy();
                double_click(a, x, y);
            }),
        ),
        Check(
            "rename: the open field holds a draft of the current name",
            Box::new(move |c, _| {
                let want = (c.project().tracks[0].id, name0(c));
                (c.rename_draft.as_ref() == Some(&want)).then_some(()).ok_or(format!("draft {:?}", c.rename_draft))
            }),
        ),
        Do("rename: click the Clip tab with the untouched field open", Box::new(|a| click(a, bottom_x() + 261.0 + 8.0, bottom_y() + 14.0))),
        Do(
            "rename: ⌘R right after",
            Box::new(move |a| {
                e0.set(a.global::<Studio>().get_rename_open_epoch());
                key(a, "r", true, false);
            }),
        ),
        Check(
            "rename: the shortcuts had the keyboard back (⌘R reached the app), no rename step",
            Box::new(move |c, a| {
                let e = a.global::<Studio>().get_rename_open_epoch();
                let rev = c.doc.revision();
                (e == e1.get() + 1 && name0(c) == *o3.borrow() && c.rename_draft.is_some() && rev == rev1.get()).then_some(()).ok_or(
                    format!(
                        "epoch {e} (was {}) name {:?} draft {:?} revision {rev} (was {})",
                        e1.get(),
                        name0(c),
                        c.rename_draft,
                        rev1.get()
                    ),
                )
            }),
        ),
        Do("rename: Escape closes the header field", Box::new(|a| key(a, Key::Escape, false, false))),
        Check(
            "rename: closed without a draft",
            Box::new(|c, _| c.rename_draft.is_none().then_some(()).ok_or(format!("draft {:?}", c.rename_draft))),
        ),
    ]
}

/// The timeline ruler's bar labels at several zooms: the rule is a label every
/// power-of-two bars once the bars are 50 px apart, starting at bar 1.
fn ruler_zoom_steps() -> Vec<Step> {
    use Step::*;
    let saved = Rc::new(Cell::new((0.0f64, 0.0f64)));
    let save = saved.clone();
    let mut v: Vec<Step> = vec![DoC(
        "ruler: remember the view",
        Box::new(move |_, ctl| {
            let c = ctl.borrow();
            save.set((c.view.start_beats, c.view.px_per_beat));
        }),
    )];
    let zooms: [(&'static str, &'static str, f64, i64); 5] = [
        ("ruler: zoom out to 12.5 %", "labels every 8 bars at 12.5 %", 0.125, 8),
        ("ruler: zoom out to 25 %", "labels every 4 bars at 25 %", 0.25, 4),
        ("ruler: default zoom", "a label on every bar at 100 %", 1.0, 1),
        ("ruler: zoom in to 400 %", "a label on every bar at 400 %", 4.0, 1),
        ("ruler: zoom in to 1500 %", "a label on every bar at 1500 %", 15.0, 1),
    ];
    for (name, check, factor, want_bars) in zooms {
        v.push(DoC(
            name,
            Box::new(move |_, ctl| {
                let mut c = ctl.borrow_mut();
                c.view.start_beats = 0.0;
                c.view.px_per_beat = crate::timeline::DEFAULT_PX_PER_BEAT * factor;
                c.ui_dirty = true;
            }),
        ));
        v.push(Check(
            check,
            Box::new(move |c, a| {
                let labels = a.global::<Studio>().get_ruler_labels();
                let rows: Vec<(f32, String)> = (0..slint::Model::row_count(&labels))
                    .filter_map(|i| slint::Model::row_data(&labels, i))
                    .map(|l| (l.x, l.text.to_string()))
                    .collect();
                let gap = want_bars as f64 * 4.0 * c.view.px_per_beat;
                let ok = !rows.is_empty()
                    && rows[0].1 == "1"
                    && rows.windows(2).all(|w| {
                        let (n0, n1): (i64, i64) = (w[0].1.parse().unwrap_or(0), w[1].1.parse().unwrap_or(0));
                        n1 - n0 == want_bars && ((w[1].0 - w[0].0) as f64 - gap).abs() < 0.5 && gap >= 49.0
                    });
                ok.then_some(()).ok_or(format!("labels {rows:?} want every {want_bars} bars"))
            }),
        ));
    }
    let saved_check = saved.clone();
    v.push(DoC(
        "ruler: restore the view",
        Box::new(move |_, ctl| {
            let mut c = ctl.borrow_mut();
            let (s, p) = saved.get();
            c.view.start_beats = s;
            c.view.px_per_beat = p;
            c.ui_dirty = true;
        }),
    ));
    v.push(Check(
        "ruler: view restored",
        Box::new(move |c, _| {
            approx(c.view.px_per_beat as f32, saved_check.get().1 as f32, 0.01).then_some(()).ok_or(format!("ppb {}", c.view.px_per_beat))
        }),
    ));
    v
}

/// Several clips selected at once (src/selection.rs): Shift-click, moving, duplicating and
/// deleting them together, and the selection box.
fn clip_fade_steps() -> Vec<Step> {
    use Step::*;
    use dstudio_core::{ClipFades, ClipId, Command, PPQ, TrackId};
    fn dir() -> std::path::PathBuf {
        run_dir().join("clip-fades")
    }
    /// The clip's fade handle centre (`out`: the fade-out one) in window coordinates, after
    /// scrolling the clip's row into view.
    fn handle(a: &AppWindow, ctl: &Ctl, id: ClipId, out: bool) -> (f32, f32) {
        let mut c = ctl.borrow_mut();
        let i = c.clip_ids.iter().position(|&k| k == id).unwrap_or(0);
        let (_, y, _, h) = c.clip_vis[i].rect(&c.view);
        let over = y + h + 10.0 - c.lanes_size.1;
        if over > 0.0 {
            c.view.scroll_y += f64::from(over);
            c.update_ui(a);
            c.rebuild_vis();
        }
        let i = c.clip_ids.iter().position(|&k| k == id).unwrap_or(0);
        let (hx, hy) = c.clip_vis[i].fade_handle_centres(&c.view)[out as usize];
        (lane_x() + hx, lane_y0() + hy)
    }
    fn fades(c: &Controller, id: u64) -> ClipFades {
        c.project().clip(ClipId(id)).map(|k| k.fades).unwrap_or_default()
    }
    let ids = Rc::new(Cell::new((0u64, 0u64)));
    let (i1, i2, i3, i4, i5, i6, i7, i8, i9) =
        (ids.clone(), ids.clone(), ids.clone(), ids.clone(), ids.clone(), ids.clone(), ids.clone(), ids.clone(), ids.clone());
    vec![
        DoC(
            "fades: import a 2 s sine at beat 4 on a new track and select it (its fade handles show)",
            Box::new(move |_, ctl| {
                let mut c = ctl.borrow_mut();
                if c.mixer_open {
                    c.toggle_mixer();
                }
                let d = dir();
                let _ = std::fs::remove_dir_all(&d);
                std::fs::create_dir_all(&d).unwrap();
                let s: Vec<f32> = (0..96_000).map(|i| 0.3 * (std::f32::consts::TAU * 440.0 * i as f32 / 48_000.0).sin()).collect();
                crate::wavgen::write_wav_pcm(&d.join("f.wav"), 48_000, 1, 24, &s).unwrap();
                c.view.start_beats = 0.0;
                c.import_files(vec![d.join("f.wav")], crate::controller::ImportTarget::NewTrack, Some(4 * PPQ));
                let t = c.selected_track.unwrap();
                let k = c.project().track(t).and_then(|t| t.clips.first()).map(|k| k.id).unwrap_or(ClipId(0));
                i1.set((t.0, k.0));
                c.ui_dirty = true;
            }),
        ),
        DoC(
            "fades: click the clip to select it",
            Box::new(move |a, ctl| {
                let id = ClipId(i9.get().1);
                let (x, y) = handle(a, ctl, id, false);
                let (ox, _) = handle(a, ctl, id, true);
                // Mid-clip, below the handles.
                click(a, (x + ox) / 2.0, y + 14.0);
            }),
        ),
        Check(
            "fades: the clip has no fades and shows both handles at its corners",
            Box::new(move |c, _| {
                let (_, k) = i2.get();
                let i = c.clip_ids.iter().position(|&x| x == ClipId(k)).ok_or("clip not drawn")?;
                let v = &c.clip_vis[i];
                (fades(c, k) == ClipFades::default() && v.fade_handles && v.fades == (0.0, 0.0)).then_some(()).ok_or(format!(
                    "{:?} handles {} {:?}",
                    fades(c, k),
                    v.fade_handles,
                    v.fades
                ))
            }),
        ),
        DoC(
            "fades: drag the fade-in handle one beat to the right",
            Box::new(move |a, ctl| {
                let id = ClipId(i3.get().1);
                let (x, y) = handle(a, ctl, id, false);
                let beat = ctl.borrow().view.px_per_beat as f32;
                drag(a, x, y, x + beat, y);
            }),
        ),
        Check(
            "fades: the clip fades in over about a beat, as one undo step",
            Box::new(move |c, _| {
                let f = fades(c, i4.get().1);
                ((f.fade_in - PPQ).abs() < PPQ / 8 && f.fade_out == 0 && c.doc.undo_label() == Some("Clip Fade Change"))
                    .then_some(())
                    .ok_or(format!("{f:?} undo {:?}", c.doc.undo_label()))
            }),
        ),
        DoC(
            "fades: drag the fade-out handle two beats to the left",
            Box::new(move |a, ctl| {
                let id = ClipId(i5.get().1);
                let (x, y) = handle(a, ctl, id, true);
                let beat = ctl.borrow().view.px_per_beat as f32;
                drag(a, x, y, x - 2.0 * beat, y);
            }),
        ),
        Check(
            "fades: the clip fades out over about two beats and the engine gets both fades",
            Box::new(move |c, _| {
                let (_, k) = i6.get();
                let f = fades(c, k);
                let assets: std::collections::HashMap<_, _> = c.project().assets.iter().map(|a| (a.id, 1u64)).collect();
                let (snap, _) = dstudio_core::snapshot::build(c.project(), 48_000.0, &assets);
                let sf = snap.tracks.iter().flat_map(|t| &t.clips).find(|x| x.clip_id == k).map(|x| x.fades);
                // 120 bpm: a beat is 24 000 frames.
                let near = |a: i64, b: i64| (a - b).abs() < 3_000;
                ((f.fade_out - 2 * PPQ).abs() < PPQ / 8 && sf.is_some_and(|(i, o)| near(i, 24_000) && near(o, 48_000)))
                    .then_some(())
                    .ok_or(format!("{f:?} engine {sf:?}"))
            }),
        ),
        Do("fades: undo", Box::new(|a| key(a, "z", true, false))),
        Check(
            "fades: undo takes back the fade-out and keeps the fade-in",
            Box::new(move |c, _| {
                let f = fades(c, i7.get().1);
                (f.fade_out == 0 && f.fade_in > 0).then_some(()).ok_or(format!("{f:?}"))
            }),
        ),
        DoC(
            "fades: remove the test track",
            Box::new(move |_, ctl| {
                let mut c = ctl.borrow_mut();
                c.select_clip(None);
                c.execute(Command::RemoveTrack { track: TrackId(i8.get().0) });
                c.view.scroll_y = 0.0;
                c.ui_dirty = true;
            }),
        ),
    ]
}

fn clip_selection_steps() -> Vec<Step> {
    use Step::*;
    use dstudio_core::{ClipId, Command, MidiClip, Track, TrackId};
    fn clips(c: &Controller) -> Vec<(u64, i64)> {
        c.project().tracks.first().map(|t| t.midi_clips.iter().map(|k| (k.id.0, k.start)).collect()).unwrap_or_default()
    }
    fn clip_xy(c: &Controller, id: u64) -> (f32, f32) {
        let i = c.clip_ids.iter().position(|&k| k == ClipId(id)).unwrap_or(0);
        let (x, y, w, h) = c.clip_vis[i].rect(&c.view);
        (lane_x() + x + w / 2.0, lane_y0() + y + h * 0.7)
    }
    // A point on track 0's lane at `beat`, low in the row (clear of the clip name).
    fn lane_at(c: &Controller, beat: f64) -> (f32, f32) {
        (lane_x() + c.view.x_of(beat), lane_y0() + c.view.track_top(0) + c.view.pitch(0) * 0.8)
    }
    fn selected(c: &Controller) -> Vec<u64> {
        let mut v: Vec<u64> = c.selected_clips().iter().map(|k| k.0).collect();
        v.sort();
        v
    }
    let ids = Rc::new(Cell::new((0u64, 0u64)));
    let (i1, i2, i3, i4, i5, i6, i7) = (ids.clone(), ids.clone(), ids.clone(), ids.clone(), ids.clone(), ids.clone(), ids.clone());
    const B: i64 = dstudio_core::PPQ;
    vec![
        DoC(
            "selection: an instrument track on top with MIDI clips at beats 0-4 and 8-12",
            Box::new(move |_, ctl| {
                let mut c = ctl.borrow_mut();
                if c.mixer_open {
                    c.toggle_mixer();
                }
                let n = c.project().next_id;
                let (t, a, k) = (n, n + 1, n + 2);
                c.execute(Command::Batch {
                    label: "selection test".into(),
                    commands: vec![
                        Command::AddTrack {
                            index: Some(0),
                            track: Track::new(TrackId(t), dstudio_core::TrackKind::Instrument, "Select", dstudio_core::ColorIndex::FIRST),
                        },
                        Command::AddMidiClip { track: TrackId(t), clip: MidiClip { id: ClipId(a), ..MidiClip::empty(0, 4 * B) } },
                        Command::AddMidiClip { track: TrackId(t), clip: MidiClip { id: ClipId(k), ..MidiClip::empty(8 * B, 4 * B) } },
                    ],
                });
                i1.set((a, k));
                c.view.scroll_y = 0.0;
                c.view.start_beats = 0.0;
                c.select_clip(None);
                c.ui_dirty = true;
            }),
        ),
        DoC(
            "selection: click the first clip, Shift-click the second",
            Box::new(move |a, ctl| {
                let (x0, y0) = clip_xy(&ctl.borrow(), i2.get().0);
                click(a, x0, y0);
                let (x1, y1) = clip_xy(&ctl.borrow(), i2.get().1);
                with_shift(a, || click(a, x1, y1));
            }),
        ),
        Check(
            "selection: both clips selected, the second focused",
            Box::new(move |c, _| {
                let (a, k) = i3.get();
                (selected(c) == [a, k] && c.selected_clip == Some(ClipId(k))).then_some(()).ok_or(format!(
                    "{:?} focus {:?}",
                    selected(c),
                    c.selected_clip
                ))
            }),
        ),
        DoC(
            "selection: drag the first clip four beats right",
            Box::new(move |a, ctl| {
                let ((x, y), dx) = {
                    let c = ctl.borrow();
                    (clip_xy(&c, i4.get().0), 4.0 * c.view.px_per_beat as f32)
                };
                drag(a, x, y, x + dx, y);
            }),
        ),
        Check(
            "selection: both moved four beats in one undo step, still selected",
            Box::new(move |c, _| {
                let (a, k) = i5.get();
                let ok = clips(c) == [(a, 4 * B), (k, 12 * B)] && c.doc.undo_label() == Some("Move 2 Clips") && selected(c) == [a, k];
                ok.then_some(()).ok_or(format!("{:?} undo {:?} sel {:?}", clips(c), c.doc.undo_label(), selected(c)))
            }),
        ),
        Do("selection: Cmd+D", Box::new(|a| key(a, "d", true, false))),
        Check(
            "selection: copies of both after the span (beats 16 and 24), the copies selected",
            Box::new(|c, _| {
                let starts: Vec<i64> = clips(c).iter().map(|k| k.1).collect();
                let sel = c.selected_clips();
                let ok = starts == [4 * B, 12 * B, 16 * B, 24 * B]
                    && c.doc.undo_label() == Some("Duplicate 2 Clips")
                    && sel.len() == 2
                    && sel.iter().all(|s| c.project().midi_clip(*s).is_some_and(|m| m.start >= 16 * B));
                ok.then_some(()).ok_or(format!("{starts:?} undo {:?}", c.doc.undo_label()))
            }),
        ),
        Do("selection: Delete", Box::new(|a| key(a, Key::Delete, false, false))),
        Check(
            "selection: both copies deleted in one undo step",
            Box::new(move |c, _| {
                let (a, k) = i6.get();
                (clips(c) == [(a, 4 * B), (k, 12 * B)] && c.doc.undo_label() == Some("Delete 2 Clips") && c.selected_clips().is_empty())
                    .then_some(())
                    .ok_or(format!("{:?} undo {:?}", clips(c), c.doc.undo_label()))
            }),
        ),
        DoC(
            "selection: drag a box over both clips from empty lane space",
            Box::new(|a, ctl| {
                let ((x0, y0), (x1, _)) = {
                    let c = ctl.borrow();
                    (lane_at(&c, 20.0), lane_at(&c, 2.0))
                };
                drag(a, x0, y0, x1, y0 - 20.0);
            }),
        ),
        Check(
            "selection: the box selected both clips and is gone after release",
            Box::new(move |c, a| {
                let (x, k) = i7.get();
                (selected(c) == [x, k] && !a.global::<Studio>().get_marquee_on()).then_some(()).ok_or(format!("{:?}", selected(c)))
            }),
        ),
        DoC(
            "selection: Shift-click the first clip to take it out",
            Box::new(move |a, ctl| {
                let (x, y) = clip_xy(&ctl.borrow(), ids.get().0);
                with_shift(a, || click(a, x, y));
            }),
        ),
        Check(
            "selection: only the second clip left",
            Box::new(|c, _| (c.selected_clips().len() == 1).then_some(()).ok_or(format!("{:?}", selected(c)))),
        ),
        DoC(
            "selection: remove the test track",
            Box::new(|_, ctl| {
                let mut c = ctl.borrow_mut();
                c.select_clip(None);
                let t = c.project().tracks[0].id;
                c.execute(Command::RemoveTrack { track: t });
            }),
        ),
    ]
}

fn clip_menu_steps() -> Vec<Step> {
    use Step::*;
    use dstudio_core::{Asset, AssetId, Clip, ClipId, Command, MidiClip, Track, TrackId};
    fn right_click(a: &AppWindow, x: f32, y: f32) {
        let w = a.window();
        let p = LogicalPosition::new(x, y);
        dispatch(w, WindowEvent::PointerMoved { position: p });
        dispatch(w, WindowEvent::PointerPressed { position: p, button: PointerEventButton::Right });
        dispatch(w, WindowEvent::PointerReleased { position: p, button: PointerEventButton::Right });
    }
    fn menu_entry(a: &AppWindow, label: &str) -> Option<(f32, f32, bool)> {
        let s = a.global::<Studio>();
        let items = s.get_menu_items();
        (0..slint::Model::row_count(&items))
            .filter_map(|i| slint::Model::row_data(&items, i))
            .find(|m| m.label == label)
            .map(|m| (s.get_menu_x() + 40.0, s.get_menu_y() + m.y + crate::clipedit::MENU_ROW / 2.0, m.enabled))
    }
    fn click_entry(a: &AppWindow, label: &str) {
        if let Some((x, y, _)) = menu_entry(a, label) {
            click(a, x, y);
        }
    }
    fn clip_xy(c: &Controller, id: ClipId) -> (f32, f32) {
        let i = c.clip_ids.iter().position(|&k| k == id).unwrap_or(0);
        let (x, y, w, h) = c.clip_vis[i].rect(&c.view);
        (lane_x() + x + w / 2.0, lane_y0() + y + h * 0.7)
    }
    fn audio_clips(c: &Controller) -> Vec<(u64, i64, u64, bool)> {
        let t = c.project().track(c.selected_track.unwrap_or(TrackId(0))).map(|t| t.clips.clone()).unwrap_or_default();
        t.iter().map(|k| (k.id.0, k.start, k.source_length, k.disabled)).collect()
    }
    let ids = Rc::new(Cell::new((0u64, 0u64, 0u64)));
    let (i1, i2, i3) = (ids.clone(), ids.clone(), ids.clone());
    let (i4, i5) = (ids.clone(), ids.clone());
    vec![
        DoC(
            "clip menu: add its own audio and instrument tracks, an audio clip (beats 4-12) and a MIDI clip (beats 4-8)",
            Box::new(move |_, ctl| {
                let mut c = ctl.borrow_mut();
                let p = c.project();
                let (a_id, i_id, asset) = (p.next_id, p.next_id + 1, p.next_id + 2);
                c.execute(Command::Batch {
                    label: "clip menu test".into(),
                    commands: vec![
                        Command::AddTrack {
                            index: Some(0),
                            track: Track::new(TrackId(a_id), TrackKind::Audio, "Clip menu", dstudio_core::ColorIndex::FIRST),
                        },
                        Command::AddTrack {
                            index: Some(1),
                            track: Track::new(TrackId(i_id), TrackKind::Instrument, "Clip menu MIDI", dstudio_core::ColorIndex::FIRST),
                        },
                        Command::AddAsset {
                            asset: Asset {
                                id: AssetId(asset),
                                relative_path: None,
                                absolute_path: "/nonexistent/clip-menu-test.wav".into(),
                                sample_rate: 48_000.0,
                                channels: 1,
                                frames: 48_000 * 4,
                            },
                        },
                        Command::AddClip {
                            track: TrackId(a_id),
                            clip: Clip {
                                id: ClipId(asset + 1),
                                asset: AssetId(asset),
                                name: "menu".into(),
                                start: 4 * PPQ,
                                source_offset: 0,
                                source_length: 48_000 * 4,
                                gain: 1.0,
                                warp: Default::default(),
                                disabled: false,
                                clip_loop: None,
                                fades: Default::default(),
                                takes: Vec::new(),
                            },
                        },
                        Command::AddMidiClip {
                            track: TrackId(i_id),
                            clip: MidiClip { id: ClipId(asset + 2), start: 4 * PPQ, ..MidiClip::empty(4 * PPQ, 4 * PPQ) },
                        },
                    ],
                });
                i1.set((a_id, asset + 1, asset + 2));
                c.select_track(TrackId(a_id));
                c.select_clip(None);
                c.seek_beats(8.0);
            }),
        ),
        DoC(
            "right-click the audio clip",
            Box::new(move |a, ctl| {
                let (x, y) = clip_xy(&ctl.borrow(), ClipId(i2.get().1));
                right_click(a, x, y);
            }),
        ),
        Check(
            "menu: Split, Disable and Show in Finder are enabled; Consolidate is not",
            Box::new(|_, a| {
                let on = |l: &str| menu_entry(a, l).is_some_and(|e| e.2);
                let ok = a.global::<Studio>().get_menu_open()
                    && on("Split")
                    && on("Disable")
                    && on("Show in Finder")
                    && menu_entry(a, "Consolidate").is_some_and(|e| !e.2);
                ok.then_some(()).ok_or("clip menu entries".into())
            }),
        ),
        Do("choose Split", Box::new(|a| click_entry(a, "Split"))),
        Check(
            "audio clip cut at beat 8 into two 2 s halves; one undo step; right half selected",
            Box::new(move |c, _| {
                let v = audio_clips(c);
                let ok = v.len() == 2
                    && v[0].1 == 4 * PPQ
                    && v[0].2 == 96_000
                    && v[1].1 == 8 * PPQ
                    && v[1].2 == 96_000
                    && c.doc.undo_label() == Some("Split Clip")
                    && c.selected_clip == Some(ClipId(v[1].0));
                ok.then_some(()).ok_or(format!("{v:?} {:?}", c.doc.undo_label()))
            }),
        ),
        Do("Cmd+Z", Box::new(|a| key(a, "z", true, false))),
        Check(
            "undo joined the clip again",
            Box::new(|c, _| {
                let v = audio_clips(c);
                (v.len() == 1 && v[0].2 == 48_000 * 4).then_some(()).ok_or(format!("{v:?}"))
            }),
        ),
        DoC(
            "select the clip, Cmd+E",
            Box::new(move |a, ctl| {
                ctl.borrow_mut().select_clip(Some(ClipId(i3.get().1)));
                key(a, "e", true, false);
            }),
        ),
        Check(
            "Cmd+E split it at the playhead",
            Box::new(|c, _| {
                let v = audio_clips(c);
                (v.len() == 2 && v[1].1 == 8 * PPQ).then_some(()).ok_or(format!("{v:?}"))
            }),
        ),
        Do("press 0", Box::new(|a| key(a, "0", false, false))),
        Check(
            "the right half is disabled and drawn grey",
            Box::new(|c, _| {
                let v = audio_clips(c);
                let i = c.clip_ids.iter().position(|k| Some(k.0) == v.get(1).map(|x| x.0)).ok_or("no vis")?;
                (v[1].3 && !v[0].3 && c.clip_vis[i].muted && c.doc.undo_label() == Some("Disable Clip"))
                    .then_some(())
                    .ok_or(format!("{v:?}"))
            }),
        ),
        DoC(
            "right-click the disabled half",
            Box::new(|a, ctl| {
                let id = ctl.borrow().selected_clip.unwrap_or(ClipId(0));
                let (x, y) = clip_xy(&ctl.borrow(), id);
                right_click(a, x, y);
            }),
        ),
        Check(
            "menu offers Enable instead of Disable",
            Box::new(|_, a| {
                (menu_entry(a, "Enable").is_some() && menu_entry(a, "Disable").is_none()).then_some(()).ok_or("menu labels".into())
            }),
        ),
        Do("choose Enable", Box::new(|a| click_entry(a, "Enable"))),
        Check(
            "both halves enabled again",
            Box::new(|c, _| {
                let v = audio_clips(c);
                (v.len() == 2 && !v[0].3 && !v[1].3).then_some(()).ok_or(format!("{v:?}"))
            }),
        ),
        DoC(
            "move the playhead to beat 6 (beat 8 is outside the MIDI clip); a running device applies it at its next block",
            Box::new(|_, ctl| {
                let mut c = ctl.borrow_mut();
                let t = c.project().tracks[1].id;
                c.select_track(t);
                c.seek_beats(6.0);
            }),
        ),
        DoC(
            "right-click the MIDI clip and choose Split",
            Box::new(move |a, ctl| {
                let mid = ClipId(i4.get().2);
                let (x, y) = clip_xy(&ctl.borrow(), mid);
                right_click(a, x, y);
                click_entry(a, "Split");
            }),
        ),
        Check(
            "MIDI clip split at beat 6 into 2 + 2 beats",
            Box::new(|c, _| {
                let m = &c.project().tracks[1].midi_clips;
                (m.len() == 2 && m[0].length == 2 * PPQ && m[1].start == 6 * PPQ && m[1].length == 2 * PPQ)
                    .then_some(())
                    .ok_or(format!("{} clips", m.len()))
            }),
        ),
        DoC(
            "clip menu: remove the test tracks and clips",
            Box::new(move |_, ctl| {
                let mut c = ctl.borrow_mut();
                let (a_id, _, mid) = i5.get();
                let inst = c.project().tracks[1].id;
                c.execute(Command::Batch {
                    label: "clip menu cleanup".into(),
                    commands: vec![
                        Command::RemoveTrack { track: TrackId(a_id) },
                        Command::RemoveTrack { track: inst },
                        Command::RemoveAsset { asset: AssetId(mid - 2) },
                    ],
                });
                c.select_clip(None);
                c.seek_beats(0.0);
            }),
        ),
        Check(
            "clip menu: project back to its earlier tracks and assets",
            Box::new(|c, _| {
                let p = c.project();
                (!p.tracks.iter().any(|t| t.name.starts_with("Clip menu"))
                    && !p.assets.iter().any(|a| a.absolute_path.contains("clip-menu-test")))
                .then_some(())
                .ok_or("leftovers".into())
            }),
        ),
    ]
}

/// Clip menu Consolidate (⌘J), Bounce › Bounce to new track and Export audio (src/clip_render.rs):
/// real clips imported from generated WAVs, the menu clicked, the rendered files read back.
/// Adds its own tracks and clips and removes them (and its scratch folder) at the end.
fn clip_render_steps() -> Vec<Step> {
    use Step::*;
    use dstudio_core::{AssetId, ClipId, Command, MidiClip, Note, Track, TrackId};
    fn dir() -> std::path::PathBuf {
        run_dir().join("clip-render")
    }
    fn right_click(a: &AppWindow, x: f32, y: f32) {
        let w = a.window();
        let p = LogicalPosition::new(x, y);
        dispatch(w, WindowEvent::PointerMoved { position: p });
        dispatch(w, WindowEvent::PointerPressed { position: p, button: PointerEventButton::Right });
        dispatch(w, WindowEvent::PointerReleased { position: p, button: PointerEventButton::Right });
    }
    fn entry(items: &slint::ModelRc<crate::MenuItem>, label: &str) -> Option<crate::MenuItem> {
        (0..items.row_count()).filter_map(|i| items.row_data(i)).find(|m| m.label == label)
    }
    fn click_entry(a: &AppWindow, label: &str) {
        let s = a.global::<Studio>();
        if let Some(m) = entry(&s.get_menu_items(), label) {
            click(a, s.get_menu_x() + 40.0, s.get_menu_y() + m.y + crate::clipedit::MENU_ROW / 2.0);
        }
    }
    fn click_sub(a: &AppWindow, label: &str) {
        let s = a.global::<Studio>();
        if let Some(m) = entry(&s.get_sub_items(), label) {
            click(a, s.get_menu_x() + 209.0 - 4.0 + 40.0, s.get_menu_y() + s.get_sub_y() + m.y + crate::clipedit::MENU_ROW / 2.0);
        }
    }
    fn clip_xy(c: &Controller, id: ClipId) -> (f32, f32) {
        let i = c.clip_ids.iter().position(|&k| k == id).unwrap_or(0);
        let (x, y, w, h) = c.clip_vis[i].rect(&c.view);
        (lane_x() + x + w / 2.0, lane_y0() + y + h * 0.7)
    }
    /// The clip's point after scrolling the lanes until it is above their bottom edge.
    fn clip_in_view(a: &AppWindow, ctl: &Ctl, id: ClipId) -> (f32, f32) {
        let (x, y) = clip_xy(&ctl.borrow(), id);
        let over = y + 45.0 - (lane_y0() + ctl.borrow().lanes_size.1);
        if over <= 0.0 {
            return (x, y);
        }
        let mut c = ctl.borrow_mut();
        c.view.scroll_y += f64::from(over);
        c.update_ui(a);
        clip_xy(&c, id)
    }
    fn sine(secs: f32, hz: f32, amp: f32) -> Vec<f32> {
        (0..(48_000.0 * secs) as usize).map(|i| amp * (std::f32::consts::TAU * hz * i as f32 / 48_000.0).sin()).collect()
    }
    fn peak(s: &[f32], from: usize, to: usize) -> f32 {
        s[from * 2..(to * 2).min(s.len())].iter().fold(0.0f32, |m, v| m.max(v.abs()))
    }
    fn files(sub: &str) -> Vec<std::path::PathBuf> {
        let mut v: Vec<_> =
            std::fs::read_dir(dir().join("renders").join(sub)).map(|d| d.flatten().map(|e| e.path()).collect()).unwrap_or_default();
        v.sort();
        v
    }
    #[derive(Default)]
    struct St {
        audio_track: u64,
        first: u64,
        second: u64,
        midi_track: u64,
        midi_clip: u64,
        tracks_before: usize,
        assets_before: usize,
        undo_before_export: Option<String>,
        joined_before: usize,
        assets_now: usize,
        undo_now: Option<String>,
        sampler_tracks: usize,
        sampler_clip: u64,
    }
    let st = Rc::new(RefCell::new(St::default()));
    let k: Vec<Rc<RefCell<St>>> = (0..40).map(|_| st.clone()).collect();
    let mut k = k.into_iter();
    let mut next = move || k.next().unwrap();
    let (s1, s2, s3, s4, s5, s6, s7, s8, s9, s10) = (next(), next(), next(), next(), next(), next(), next(), next(), next(), next());
    let (s11, s12, s13, s14, s15, s16, s17, s18) = (next(), next(), next(), next(), next(), next(), next(), next());
    let (s19, s20, s21, s22, s23, s24, s25, s26) = (next(), next(), next(), next(), next(), next(), next(), next());
    let (s27, s28, s29, s30, s31, s32, s33) = (next(), next(), next(), next(), next(), next(), next());
    vec![
        DoC(
            "clip render: scratch folder, two 2 s sine WAVs; import the first at beat 4 on a new track",
            Box::new(move |_, ctl| {
                let mut c = ctl.borrow_mut();
                let d = dir();
                let _ = std::fs::remove_dir_all(&d);
                std::fs::create_dir_all(&d).unwrap();
                c.clip_render.dir_override = Some(d.join("renders"));
                {
                    let mut s = s1.borrow_mut();
                    s.tracks_before = c.project().tracks.len();
                    s.assets_before = c.project().assets.len();
                }
                crate::wavgen::write_wav_pcm(&d.join("a.wav"), 48_000, 1, 24, &sine(2.0, 440.0, 0.3)).unwrap();
                crate::wavgen::write_wav_pcm(&d.join("b.wav"), 48_000, 1, 24, &sine(2.0, 440.0, 0.3)).unwrap();
                c.import_files(vec![d.join("a.wav")], crate::controller::ImportTarget::NewTrack, Some(4 * PPQ));
            }),
        ),
        DoC(
            "clip render: import the second at beat 6 on the same track (overlaps the first by 1 s)",
            Box::new(move |_, ctl| {
                let mut c = ctl.borrow_mut();
                let t = c.selected_track.unwrap();
                s2.borrow_mut().audio_track = t.0;
                c.import_files(vec![dir().join("b.wav")], crate::controller::ImportTarget::Track(t), Some(6 * PPQ));
            }),
        ),
        DoC(
            "clip render: add an instrument track with a 2 s MIDI clip of two notes at beat 4",
            Box::new(move |_, ctl| {
                let mut c = ctl.borrow_mut();
                let mut s = s3.borrow_mut();
                let id = c.project().next_id;
                let mut clip = MidiClip::empty(4 * PPQ, 4 * PPQ);
                clip.id = ClipId(id + 1);
                clip.name = "bounce me".into();
                clip.notes = vec![
                    Note { id: 1, pitch: 60, start: 0, length: 2 * PPQ, velocity: 110 },
                    Note { id: 2, pitch: 67, start: 2 * PPQ, length: PPQ, velocity: 110 },
                ];
                let track = Track::new(TrackId(id), TrackKind::Instrument, "Render synth", dstudio_core::ColorIndex::FIRST);
                let fader = dstudio_core::automation::AutomationLane {
                    target: dstudio_core::automation::AutomationTarget::Volume,
                    enabled: true,
                    points: vec![dstudio_core::automation::AutoPoint { time: 4 * PPQ, value: -6.0, curve: 0.0 }],
                };
                c.execute(Command::Batch {
                    label: "clip render test".into(),
                    commands: vec![
                        Command::AddTrack { index: None, track },
                        Command::AddMidiClip { track: TrackId(id), clip },
                        Command::SetAutomationLane { track: Some(TrackId(id)), target: fader.target.clone(), lane: Some(fader) },
                    ],
                });
                s.midi_track = id;
                s.midi_clip = id + 1;
                let ids: Vec<u64> =
                    c.project().track(TrackId(s.audio_track)).map(|t| t.clips.iter().map(|k| k.id.0).collect()).unwrap_or_default();
                s.first = ids.first().copied().unwrap_or(0);
                s.second = ids.get(1).copied().unwrap_or(0);
                c.select_track(TrackId(s.audio_track));
                c.select_clip(None);
            }),
        ),
        Check(
            "clip render: two overlapping audio clips and one MIDI clip imported",
            Box::new(move |c, _| {
                let s = s4.borrow();
                let t = c.project().track(TrackId(s.audio_track)).ok_or("no audio track")?;
                (t.clips.len() == 2 && s.first != 0 && s.second != 0 && c.project().midi_clip(ClipId(s.midi_clip)).is_some())
                    .then_some(())
                    .ok_or(format!("{} clips", t.clips.len()))
            }),
        ),
        DoC(
            "clip render: right-click the first audio clip",
            Box::new(move |a, ctl| {
                let (x, y) = clip_in_view(a, ctl, ClipId(s5.borrow().first));
                right_click(a, x, y);
            }),
        ),
        Check(
            "clip render: Consolidate, Bounce and Export are enabled, Ask is not; Bounce has a submenu",
            Box::new(|_, a| {
                let s = a.global::<Studio>();
                let items = s.get_menu_items();
                let on = |l: &str| entry(&items, l).is_some_and(|m| m.enabled);
                let ok = on("Consolidate")
                    && on("Bounce")
                    && on("Export audio to Finder")
                    && entry(&items, "Ask").is_some_and(|m| !m.enabled)
                    && entry(&items, "Bounce").is_some_and(|m| m.submenu);
                ok.then_some(()).ok_or("menu entries".into())
            }),
        ),
        Do("clip render: choose Consolidate", Box::new(|a| click_entry(a, "Consolidate"))),
        Check(
            "clip render: Consolidate joined both clips into one 3 s clip at beat 4, named '<first> merged', one undo step",
            Box::new(move |c, _| {
                let s = s6.borrow();
                let t = c.project().track(TrackId(s.audio_track)).ok_or("no track")?;
                let k = t.clips.first().ok_or("no clip")?;
                let ok = t.clips.len() == 1
                    && k.start == 4 * PPQ
                    && k.source_length.abs_diff(144_000) <= 1
                    && k.name.ends_with(" merged")
                    && !k.warp.enabled
                    && c.doc.undo_label() == Some("Consolidate Clips")
                    && c.selected_clip == Some(k.id);
                ok.then_some(()).ok_or(format!(
                    "{} clips; {:?}; undo {:?}",
                    t.clips.len(),
                    t.clips.first().map(|k| (k.start, k.source_length, k.name.clone())),
                    c.doc.undo_label()
                ))
            }),
        ),
        Check(
            "clip render: the rendered file is in the render folder; the overlap is the sum of both clips (unity level), the rest one clip",
            Box::new(|c, _| {
                let f = files("joined");
                let path = f.first().ok_or("no file in joined/")?;
                let (rate, ch, s) = crate::wavgen::read_wav(path)?;
                let (a, b, tail) = (peak(&s, 0, 40_000), peak(&s, 52_000, 92_000), peak(&s, 104_000, 143_000));
                eprintln!("clip-render: consolidate render {:.1} ms (3 s of audio)", c.clip_render.last_ms);
                let ok = rate == 48_000
                    && ch == 2
                    && s.len() / 2 == 144_000
                    && (a - 0.3).abs() < 0.02
                    && (b - 0.6).abs() < 0.03
                    && (tail - 0.3).abs() < 0.02;
                ok.then_some(()).ok_or(format!("rate {rate} ch {ch} frames {} peaks {a} {b} {tail}", s.len() / 2))
            }),
        ),
        Do("clip render: Cmd+Z", Box::new(|a| key(a, "z", true, false))),
        Check(
            "clip render: one undo restored both clips and removed the merged asset",
            Box::new(move |c, _| {
                let s = s7.borrow();
                let t = c.project().track(TrackId(s.audio_track)).ok_or("no track")?;
                let ids: Vec<u64> = t.clips.iter().map(|k| k.id.0).collect();
                (ids == vec![s.first, s.second] && c.project().assets.len() == s.assets_before + 2)
                    .then_some(())
                    .ok_or(format!("{ids:?} assets {}", c.project().assets.len()))
            }),
        ),
        DoC(
            "clip render: select the first clip and press Cmd+J",
            Box::new(move |a, ctl| {
                ctl.borrow_mut().select_clip(Some(ClipId(s8.borrow().first)));
                key(a, "j", true, false);
            }),
        ),
        Check(
            "clip render: Cmd+J consolidated them again",
            Box::new(move |c, _| {
                let t = c.project().track(TrackId(s9.borrow().audio_track)).ok_or("no track")?;
                (t.clips.len() == 1 && c.doc.undo_label() == Some("Consolidate Clips"))
                    .then_some(())
                    .ok_or(format!("{} clips", t.clips.len()))
            }),
        ),
        Do("clip render: Cmd+Z", Box::new(|a| key(a, "z", true, false))),
        DoC(
            "clip render: select the instrument track (the lanes scroll to it)",
            Box::new(move |_, ctl| {
                ctl.borrow_mut().select_track(TrackId(s17.borrow().midi_track));
            }),
        ),
        DoC(
            "clip render: right-click the MIDI clip",
            Box::new(move |a, ctl| {
                let id = ClipId(s10.borrow().midi_clip);
                let (x, y) = clip_in_view(a, ctl, id);
                right_click(a, x, y);
            }),
        ),
        Do("clip render: open Bounce", Box::new(|a| click_entry(a, "Bounce"))),
        Check(
            "clip render: the Bounce submenu lists Bounce to new track (enabled, ⌘ B) and Bounce track in place (greyed)",
            Box::new(|c, a| {
                let s = a.global::<Studio>();
                let sub = s.get_sub_items();
                let _ = c;
                let ok = s.get_sub_open()
                    && sub.row_count() == 2
                    && entry(&sub, "Bounce to new track").is_some_and(|m| m.enabled && m.shortcut == "⌘ B")
                    && entry(&sub, "Bounce track in place").is_some_and(|m| !m.enabled);
                ok.then_some(()).ok_or(format!("open {} rows {}", s.get_sub_open(), sub.row_count()))
            }),
        ),
        Do("clip render: choose Bounce to new track", Box::new(|a| click_sub(a, "Bounce to new track"))),
        Check(
            "clip render: a new audio track 'Render synth (Bounce)' below holds a clip at beat 4 (2-4 s); the MIDI clip is disabled; one undo step",
            Box::new(move |c, _| {
                let s = s11.borrow();
                let p = c.project();
                let i = p.track_index(TrackId(s.midi_track)).ok_or("no instrument track")?;
                let t = p.tracks.get(i + 1).ok_or("no track below")?;
                let k = t.clips.first().ok_or("no clip")?;
                let ok = t.kind == TrackKind::Audio
                    && t.name == "Render synth (Bounce)"
                    && k.start == 4 * PPQ
                    && (96_000..=192_001).contains(&k.source_length)
                    && t.automation.iter().any(|l| l.target == dstudio_core::automation::AutomationTarget::Volume)
                    && p.midi_clip(ClipId(s.midi_clip)).is_some_and(|m| m.disabled)
                    && c.doc.undo_label() == Some("Bounce to New Track")
                    && c.selected_track == Some(t.id);
                ok.then_some(()).ok_or(format!(
                    "{} {:?} {:?}",
                    t.name,
                    t.clips.first().map(|k| (k.start, k.source_length)),
                    c.doc.undo_label()
                ))
            }),
        ),
        Check(
            "clip render: the bounced file has sound (the synth was rendered)",
            Box::new(|c, _| {
                let f = files("bounce");
                let path = f.first().ok_or("no file in bounce/")?;
                let (rate, ch, s) = crate::wavgen::read_wav(path)?;
                let p = peak(&s, 0, s.len() / 2);
                eprintln!("clip-render: bounce render {:.1} ms (2 s clip + tail)", c.clip_render.last_ms);
                (rate == 48_000 && ch == 2 && p > 0.02 && s.len() / 2 >= 96_000)
                    .then_some(())
                    .ok_or(format!("rate {rate} ch {ch} frames {} peak {p}", s.len() / 2))
            }),
        ),
        Do("clip render: Cmd+Z", Box::new(|a| key(a, "z", true, false))),
        Check(
            "clip render: undo removed the bounce track and enabled the MIDI clip again",
            Box::new(move |c, _| {
                let s = s12.borrow();
                let p = c.project();
                (p.tracks.len() == s.tracks_before + 2 && p.midi_clip(ClipId(s.midi_clip)).is_some_and(|m| !m.disabled))
                    .then_some(())
                    .ok_or(format!("{} tracks", p.tracks.len()))
            }),
        ),
        DoC(
            "clip render: select the instrument track again",
            Box::new(move |_, ctl| {
                ctl.borrow_mut().select_track(TrackId(s18.borrow().midi_track));
            }),
        ),
        DoC(
            "clip render: right-click the MIDI clip, with Export pointed at a temp file",
            Box::new(move |a, ctl| {
                let id = ClipId(s13.borrow().midi_clip);
                s13.borrow_mut().undo_before_export = ctl.borrow().doc.undo_label().map(str::to_string);
                ctl.borrow_mut().clip_render.export_path = Some(dir().join("export").join("clip.wav"));
                std::fs::create_dir_all(dir().join("export")).unwrap();
                let (x, y) = clip_in_view(a, ctl, id);
                right_click(a, x, y);
            }),
        ),
        Do("clip render: choose Export audio to Finder", Box::new(|a| click_entry(a, "Export audio to Finder"))),
        Check(
            "clip render: the exported WAV is stereo 48 kHz with sound, 2-4 s long, and the project is untouched",
            Box::new(move |c, _| {
                let (rate, ch, s) = crate::wavgen::read_wav(&dir().join("export").join("clip.wav"))?;
                let p = peak(&s, 0, s.len() / 2);
                eprintln!("clip-render: export render {:.1} ms", c.clip_render.last_ms);
                let same = c.doc.undo_label().map(str::to_string) == s14.borrow().undo_before_export;
                // A stem goes through the master fader: the --self-test-audio run keeps it at -40 dB.
                let floor = 0.02 * c.project().master_gain.min(1.0);
                (rate == 48_000 && ch == 2 && p > floor && (96_000..=192_000).contains(&(s.len() / 2)) && same && !c.clip_render.busy())
                    .then_some(())
                    .ok_or(format!("rate {rate} ch {ch} frames {} peak {p} (floor {floor}) same {same}", s.len() / 2))
            }),
        ),
        DoC(
            "export midi: right-click the MIDI clip, with Export MIDI pointed at a temp file",
            Box::new(move |a, ctl| {
                let id = ClipId(s27.borrow().midi_clip);
                ctl.borrow_mut().midi_export_path = Some(dir().join("export").join("clip.mid"));
                let (x, y) = clip_in_view(a, ctl, id);
                right_click(a, x, y);
            }),
        ),
        Check(
            "export midi: the menu lists Export MIDI between Show in Finder and Export audio",
            Box::new(|c, _| {
                let labels: Vec<&str> = c.menu.items.iter().map(|(a, _)| a.label()).collect();
                let at = |l: &str| labels.iter().position(|x| *x == l);
                let ok = c.menu.open
                    && matches!((at("Show in Finder"), at("Export MIDI"), at("Export audio to Finder")), (Some(a), Some(b), Some(e)) if a + 1 == b && b + 1 == e)
                    && c.menu.items.iter().any(|&(a, en)| a == crate::controller::MenuAction::ExportMidi && en);
                ok.then_some(()).ok_or(format!("{labels:?}"))
            }),
        ),
        Do("export midi: choose Export MIDI", Box::new(|a| click_entry(a, "Export MIDI"))),
        Check(
            "export midi: the file is a format 0 Standard MIDI File with the clip's notes",
            Box::new(move |c, _| {
                let f = std::fs::read(dir().join("export").join("clip.mid")).map_err(|e| e.to_string())?;
                let notes = c.project().midi_clip(ClipId(s28.borrow().midi_clip)).map_or(0, |m| m.playing_notes().len());
                let ons = f.windows(1).filter(|w| w[0] == 0x90).count();
                (f.starts_with(b"MThd") && f[8..10] == [0, 0] && &f[14..18] == b"MTrk" && ons >= notes && !c.menu.open)
                    .then_some(())
                    .ok_or(format!("{} bytes, {ons} note-ons for {notes} notes", f.len()))
            }),
        ),
        DoC(
            "send to sampler: right-click the first audio clip",
            Box::new(move |a, ctl| {
                let mut s = s29.borrow_mut();
                s.sampler_tracks = ctl.borrow().project().tracks.len();
                let id = ClipId(s.first);
                drop(s);
                let (x, y) = clip_in_view(a, ctl, id);
                right_click(a, x, y);
            }),
        ),
        Check(
            "send to sampler: Resample and Send to Sampler follow Rename on an audio clip; no Export MIDI",
            Box::new(move |c, _| {
                s33.borrow_mut().sampler_clip = c.menu.clip.map_or(0, |k| k.0);
                let labels: Vec<&str> = c.menu.items.iter().map(|(a, _)| a.label()).collect();
                let at = |l: &str| labels.iter().position(|x| *x == l);
                let ok = c.menu.open
                    && matches!((at("Rename…"), at("Resample"), at("Send to Sampler")), (Some(r), Some(a), Some(b)) if r + 1 == a && a + 1 == b)
                    && at("Export MIDI").is_none();
                ok.then_some(()).ok_or(format!("{labels:?}"))
            }),
        ),
        Do("send to sampler: choose Send to Sampler", Box::new(|a| click_entry(a, "Send to Sampler"))),
        Check(
            "send to sampler: a new instrument track below, selected, whose Sampler holds the clip's file",
            Box::new(move |c, _| {
                let s = s30.borrow();
                let p = c.project();
                // The clip under the pointer (the menu's clip).
                let (ti, ci) = p.find_clip(ClipId(s.sampler_clip)).ok_or("clip gone")?;
                let clip = &p.tracks[ti].clips[ci];
                let t = p.tracks.get(ti + 1).ok_or("no track below")?;
                let d = t.instrument_plugin.as_ref().ok_or("no instrument")?;
                let ok = p.tracks.len() == s.sampler_tracks + 1
                    && t.kind == TrackKind::Instrument
                    && d.plugin.id == "sampler-steam"
                    && d.samples.first().is_some_and(|x| x.asset == clip.asset)
                    && c.selected_track == Some(t.id)
                    && c.doc.undo_label() == Some("Send to Sampler");
                ok.then_some(()).ok_or(format!(
                    "{} tracks (was {}), plugin {}, sample {:?} clip asset {:?}, selected {:?} new {:?}, undo {:?}",
                    p.tracks.len(),
                    s.sampler_tracks,
                    d.plugin.id,
                    d.samples.first().map(|x| x.asset),
                    clip.asset,
                    c.selected_track,
                    t.id,
                    c.doc.undo_label()
                ))
            }),
        ),
        Do("send to sampler: Cmd+Z", Box::new(|a| key(a, "z", true, false))),
        Check(
            "send to sampler: undo removes the track in one step",
            Box::new(move |c, _| {
                let n = c.project().tracks.len();
                let sampler =
                    c.project().tracks.iter().any(|t| t.instrument_plugin.as_ref().is_some_and(|d| d.plugin.id == "sampler-steam"));
                (n == s31.borrow().sampler_tracks && !sampler && s32.borrow().first != 0).then_some(()).ok_or(format!("{n} tracks"))
            }),
        ),
        DoC(
            "clip render: select the audio track; Consolidate its first clip and Cancel at once",
            Box::new(move |_, ctl| {
                let mut c = ctl.borrow_mut();
                let mut s = s19.borrow_mut();
                c.select_track(TrackId(s.audio_track));
                s.joined_before = files("joined").len();
                s.assets_now = c.project().assets.len();
                s.undo_now = c.doc.undo_label().map(str::to_string);
                c.consolidate_clip(ClipId(s.first));
                c.cancel_clip_render();
            }),
        ),
        Check(
            "clip render: Cancel left nothing behind: both clips, no file, no asset, no undo step",
            Box::new(move |c, _| {
                let s = s20.borrow();
                let ids: Vec<u64> =
                    c.project().track(TrackId(s.audio_track)).map(|t| t.clips.iter().map(|k| k.id.0).collect()).unwrap_or_default();
                let ok = ids == vec![s.first, s.second]
                    && files("joined").len() == s.joined_before
                    && c.project().assets.len() == s.assets_now
                    && c.doc.undo_label().map(str::to_string) == s.undo_now
                    && !c.clip_render.busy()
                    && c.status_message() == Some("Render cancelled");
                ok.then_some(()).ok_or(format!(
                    "{ids:?}; {} files (was {}); {} assets (was {}); undo {:?}; busy {}; {:?}",
                    files("joined").len(),
                    s.joined_before,
                    c.project().assets.len(),
                    s.assets_now,
                    c.doc.undo_label(),
                    c.clip_render.busy(),
                    c.status_message()
                ))
            }),
        ),
        DoC(
            "clip render: Consolidate, then change the second clip while it renders",
            Box::new(move |_, ctl| {
                let mut c = ctl.borrow_mut();
                let s = s21.borrow();
                c.consolidate_clip(ClipId(s.first));
                let busy = c.clip_render.busy();
                c.execute(Command::SetClipGain { clip: ClipId(s.second), gain: 0.5 });
                eprintln!("clip-render: render in flight while the edit ran: {busy}");
            }),
        ),
        Check(
            "clip render: the render was discarded (a source clip changed): two clips, no file, the gain edit on top of the undo stack",
            Box::new(move |c, _| {
                let s = s22.borrow();
                let ids: Vec<u64> =
                    c.project().track(TrackId(s.audio_track)).map(|t| t.clips.iter().map(|k| k.id.0).collect()).unwrap_or_default();
                let ok = ids == vec![s.first, s.second]
                    && files("joined").len() == s.joined_before
                    && c.project().assets.len() == s.assets_now
                    && c.doc.undo_label() != Some("Consolidate Clips")
                    && !c.clip_render.busy()
                    && c.status_message().is_some_and(|m| m.contains("changed while it rendered"));
                ok.then_some(()).ok_or(format!(
                    "{ids:?}; {} files; {} assets; undo {:?}; {:?}",
                    files("joined").len(),
                    c.project().assets.len(),
                    c.doc.undo_label(),
                    c.status_message()
                ))
            }),
        ),
        Do("clip render: Cmd+Z (the gain edit)", Box::new(|a| key(a, "z", true, false))),
        DoC(
            "clip render: Consolidate, then change another track while it renders",
            Box::new(move |_, ctl| {
                let mut c = ctl.borrow_mut();
                let s = s23.borrow();
                c.consolidate_clip(ClipId(s.first));
                c.execute(Command::SetTrackGain { track: TrackId(s.midi_track), gain: 0.9 });
            }),
        ),
        Check(
            "clip render: that render still landed as one step on top of the other edit, the interface was free meanwhile",
            Box::new(move |c, _| {
                let s = s24.borrow();
                let t = c.project().track(TrackId(s.audio_track)).ok_or("no track")?;
                let ok = t.clips.len() == 1
                    && c.doc.undo_label() == Some("Consolidate Clips")
                    && files("joined").len() == s.joined_before + 1
                    && (c.project().track(TrackId(s.midi_track)).map(|m| m.gain) == Some(0.9));
                ok.then_some(()).ok_or(format!("{} clips; undo {:?}; {} files", t.clips.len(), c.doc.undo_label(), files("joined").len()))
            }),
        ),
        Do("clip render: Cmd+Z (the consolidation)", Box::new(|a| key(a, "z", true, false))),
        Do("clip render: Cmd+Z (the gain)", Box::new(|a| key(a, "z", true, false))),
        DoC(
            "clip render: delete the second WAV, then Consolidate",
            Box::new(move |_, ctl| {
                let mut c = ctl.borrow_mut();
                let mut s = s25.borrow_mut();
                s.joined_before = files("joined").len();
                s.assets_now = c.project().assets.len();
                s.undo_now = c.doc.undo_label().map(str::to_string);
                std::fs::remove_file(dir().join("b.wav")).unwrap();
                c.consolidate_clip(ClipId(s.first));
            }),
        ),
        Check(
            "clip render: the render failed with a message; both clips, no file, no asset, no undo step",
            Box::new(move |c, _| {
                let s = s26.borrow();
                let ids: Vec<u64> =
                    c.project().track(TrackId(s.audio_track)).map(|t| t.clips.iter().map(|k| k.id.0).collect()).unwrap_or_default();
                let ok = ids == vec![s.first, s.second]
                    && files("joined").len() == s.joined_before
                    && c.project().assets.len() == s.assets_now
                    && c.doc.undo_label().map(str::to_string) == s.undo_now
                    && !c.clip_render.busy()
                    && c.status_message().is_some_and(|m| m.starts_with("Render failed") && m.contains("b.wav"));
                ok.then_some(()).ok_or(format!(
                    "{ids:?}; {} files (was {}); undo {:?}; {:?}",
                    files("joined").len(),
                    s.joined_before,
                    c.doc.undo_label(),
                    c.status_message()
                ))
            }),
        ),
        DoC(
            "clip render: remove the test tracks and assets and the scratch folder",
            Box::new(move |_, ctl| {
                let mut c = ctl.borrow_mut();
                let s = s15.borrow();
                let mut commands: Vec<Command> =
                    [s.audio_track, s.midi_track].iter().map(|&t| Command::RemoveTrack { track: TrackId(t) }).collect();
                commands.extend(
                    c.project()
                        .assets
                        .iter()
                        .filter(|a| in_dir(&a.absolute_path, &dir()))
                        .map(|a| Command::RemoveAsset { asset: AssetId(a.id.0) }),
                );
                c.execute(Command::Batch { label: "clip render cleanup".into(), commands });
                c.select_clip(None);
                c.clip_render.dir_override = Some(run_dir().join("renders"));
                let _ = std::fs::remove_dir_all(dir());
            }),
        ),
        Check(
            "clip render: project back to its earlier tracks and assets",
            Box::new(move |c, _| {
                let s = s16.borrow();
                let p = c.project();
                (p.tracks.len() == s.tracks_before && p.assets.len() == s.assets_before).then_some(()).ok_or(format!(
                    "{} tracks {} assets",
                    p.tracks.len(),
                    p.assets.len()
                ))
            }),
        ),
    ]
}

/// Clip menu Bounce through a third-party plugin (src/clip_render.rs, CHR-38): the CLAP test gain
/// on a track is edited after the project's state was saved, the clip is bounced, and the file has
/// the live instance's gain (the plugin was prepared on this thread from its current state and
/// rendered on the worker). Measures the longest gap between two turns of the event loop during
/// the render. A plugin that cannot be loaded fails the render before any file is written, and a
/// render cancelled while it prepares leaves nothing. Adds its own track and removes it at the end.
fn clip_render_plugin_steps() -> Vec<Step> {
    use Step::*;
    use dstudio_core::{AssetId, ClipId, Command, Device, PluginFormat, PluginKind, PluginRef, TrackId};
    use std::time::{Duration, Instant};
    fn dir() -> std::path::PathBuf {
        run_dir().join("clip-render-plugin")
    }
    fn files() -> Vec<std::path::PathBuf> {
        let mut v: Vec<_> =
            std::fs::read_dir(dir().join("renders").join("bounce")).map(|d| d.flatten().map(|e| e.path()).collect()).unwrap_or_default();
        v.sort();
        v
    }
    fn plugin(path: &str, name: &str) -> PluginRef {
        PluginRef {
            format: PluginFormat::Clap,
            id: "studio.dstudio.test.gain".into(),
            path: Some(path.into()),
            name: name.into(),
            vendor: "DStudio".into(),
            version: "1.0.0".into(),
            kind: PluginKind::Effect,
        }
    }
    #[derive(Default)]
    struct St {
        track: u64,
        clip: u64,
        tracks_before: usize,
        assets_before: usize,
        tracks_now: usize,
        assets_now: usize,
        undo_now: Option<String>,
        files_now: usize,
        gaps: Rc<RefCell<(Option<Instant>, f64)>>,
        timer: Option<slint::Timer>,
    }
    let st = Rc::new(RefCell::new(St::default()));
    let k: Vec<Rc<RefCell<St>>> = (0..12).map(|_| st.clone()).collect();
    let mut k = k.into_iter();
    let mut next = move || k.next().unwrap();
    let (s1, s2, s3, s4, s5, s6, s7, s8) = (next(), next(), next(), next(), next(), next(), next(), next());
    let (s9, s10, s11) = (next(), next(), next());
    vec![
        DoC(
            "clip render plugin: scratch folder, a 2 s sine imported at beat 4 on a new track",
            Box::new(move |_, ctl| {
                let mut c = ctl.borrow_mut();
                let d = dir();
                let _ = std::fs::remove_dir_all(&d);
                std::fs::create_dir_all(&d).unwrap();
                c.clip_render.dir_override = Some(d.join("renders"));
                {
                    let mut s = s1.borrow_mut();
                    s.tracks_before = c.project().tracks.len();
                    s.assets_before = c.project().assets.len();
                }
                let sine: Vec<f32> = (0..96_000).map(|i| 0.4 * (std::f32::consts::TAU * 440.0 * i as f32 / 48_000.0).sin()).collect();
                crate::wavgen::write_wav_pcm(&d.join("p.wav"), 48_000, 1, 24, &sine).unwrap();
                c.import_files(vec![d.join("p.wav")], crate::controller::ImportTarget::NewTrack, Some(4 * PPQ));
            }),
        ),
        DoC(
            "clip render plugin: add the CLAP test gain (saved state 0.25), then set it to 0.5 in the live instance",
            Box::new(move |_, ctl| {
                let mut c = ctl.borrow_mut();
                let mut s = s2.borrow_mut();
                let t = c.selected_track.unwrap();
                s.track = t.0;
                s.clip = c.project().track(t).and_then(|t| t.clips.first()).map_or(0, |k| k.id.0);
                let mut gain = Device::new(plugin(
                    &dstudio_engine::test_plugin_dir().join("DStudioTestGain.clap").to_string_lossy(),
                    "DStudio Test Gain",
                ));
                gain.state = Some(dstudio_core::device::b64::encode(&0.25f32.to_le_bytes()));
                c.execute(Command::AddDevice { track: t, index: None, device: gain });
                let id = c.project().track(t).and_then(|t| t.devices.first()).map_or(0, |d| d.id.0);
                // The edit reaches the plugin's state when the engine processes a block: one offline
                // block on the live engine (no device runs in the self-test), transport put back.
                let was = c.engine.transport();
                let ok = c.engine.plugin_set_param(id, 0, 0.5).is_ok()
                    && c.engine.seek(0).is_ok()
                    && c.engine.play().is_ok()
                    && c.engine.render_offline(&mut [0.0; 1024]).is_ok();
                let _ = c.engine.pause();
                let _ = c.engine.seek(was.position);
                eprintln!("clip-render plugin: live gain edited: {ok}");
            }),
        ),
        Check(
            "clip render plugin: the track holds the clip and the plugin, no load error",
            Box::new(move |c, _| {
                let s = s3.borrow();
                let t = c.project().track(TrackId(s.track)).ok_or("no track")?;
                (s.clip != 0 && t.devices.len() == 1 && c.engine.plugin_load_errors().is_empty()).then_some(()).ok_or(format!(
                    "{} devices; {:?}",
                    t.devices.len(),
                    c.engine.plugin_load_errors()
                ))
            }),
        ),
        DoC(
            "clip render plugin: Bounce to new track, with a 4 ms timer measuring the event loop meanwhile",
            Box::new(move |_, ctl| {
                let mut c = ctl.borrow_mut();
                let mut s = s4.borrow_mut();
                *s.gaps.borrow_mut() = (Some(Instant::now()), 0.0);
                let gaps = s.gaps.clone();
                let timer = slint::Timer::default();
                timer.start(slint::TimerMode::Repeated, Duration::from_millis(4), move || {
                    let mut g = gaps.borrow_mut();
                    let now = Instant::now();
                    if let Some(last) = g.0 {
                        g.1 = g.1.max((now - last).as_secs_f64() * 1e3);
                    }
                    g.0 = Some(now);
                });
                s.timer = Some(timer);
                c.clip_render.ui_stall_ms = 0.0; // a running maximum: measure this bounce only
                c.bounce_clip(ClipId(s.clip));
                eprintln!("clip-render plugin: render in flight: {}", c.clip_render.busy());
            }),
        ),
        Check(
            "clip render plugin: a new track '(Bounce)' holds the render, one undo step, the event loop was never held long",
            Box::new(move |c, _| {
                let mut s = s5.borrow_mut();
                s.timer = None;
                let gap = s.gaps.borrow().1;
                let p = c.project();
                let i = p.track_index(TrackId(s.track)).ok_or("no track")?;
                let t = p.tracks.get(i + 1).ok_or("no track below")?;
                let ok = t.name.ends_with("(Bounce)")
                    && t.clips.len() == 1
                    && c.doc.undo_label() == Some("Bounce to New Track")
                    && !c.clip_render.busy();
                eprintln!(
                    "clip-render plugin: render {:.1} ms; longest UI-thread piece {:.1} ms; longest gap between event-loop turns {gap:.1} ms (4 ms timer)",
                    c.clip_render.last_ms, c.clip_render.ui_stall_ms
                );
                // The render's own pieces on the UI thread must stay short. The gap between
                // event-loop turns also counts drawing frames, which on a software renderer
                // (SLINT_WGPU_CPU: CI's lavapipe / WARP) alone can take 400-900 ms, so there the gap
                // limit is 3 s: still catches a UI that stops responding, not a slow frame.
                let software = std::env::var_os("SLINT_WGPU_CPU").is_some();
                let gap_limit = if software { 3000.0 } else { 400.0 };
                let stall = c.clip_render.ui_stall_ms;
                (ok && stall < 50.0 && gap < gap_limit).then_some(()).ok_or(format!(
                    "{} {:?} {:?}; UI piece {stall:.1} ms; gap {gap:.1} ms; status {:?}",
                    t.name,
                    t.clips.len(),
                    c.doc.undo_label(),
                    c.status_message()
                ))
            }),
        ),
        Check(
            "clip render plugin: the file has the live gain 0.5 (peak 0.2), not the saved 0.25 (0.1) nor the dry level (0.4)",
            Box::new(|_, _| {
                let f = files();
                let path = f.first().ok_or("no file in bounce/")?;
                let (rate, ch, s) = crate::wavgen::read_wav(path)?;
                let frames = s.len() / 2;
                let p = s.iter().fold(0.0f32, |m, v| m.max(v.abs()));
                eprintln!("clip-render plugin: bounced peak {p:.4} ({frames} frames)");
                (rate == 48_000 && ch == 2 && (96_000..=98_000).contains(&frames) && (p - 0.2).abs() < 0.01)
                    .then_some(())
                    .ok_or(format!("rate {rate} ch {ch} frames {frames} peak {p}"))
            }),
        ),
        Do("clip render plugin: Cmd+Z", Box::new(|a| key(a, "z", true, false))),
        DoC(
            "clip render plugin: add a plugin that cannot be loaded after the first, then Bounce",
            Box::new(move |_, ctl| {
                let mut c = ctl.borrow_mut();
                let mut s = s6.borrow_mut();
                let missing = Device::new(plugin("/nonexistent/Missing Verb.clap", "Missing Verb"));
                c.execute(Command::AddDevice { track: TrackId(s.track), index: None, device: missing });
                s.files_now = files().len();
                s.assets_now = c.project().assets.len();
                s.tracks_now = c.project().tracks.len();
                s.undo_now = c.doc.undo_label().map(str::to_string);
                c.bounce_clip(ClipId(s.clip));
            }),
        ),
        Check(
            "clip render plugin: the render failed by plugin name; no file, no asset, no track, no undo step",
            Box::new(move |c, _| {
                let s = s7.borrow();
                let msg = c.status_message().unwrap_or_default().to_string();
                let ok = msg.starts_with("Render failed: Missing Verb could not be loaded")
                    && files().len() == s.files_now
                    && c.project().assets.len() == s.assets_now
                    && c.project().tracks.len() == s.tracks_now
                    && c.doc.undo_label().map(str::to_string) == s.undo_now
                    && !c.clip_render.busy()
                    && c.pending_jobs == 0;
                ok.then_some(()).ok_or(format!(
                    "{msg:?}; {} files; {} assets; undo {:?}; busy {}",
                    files().len(),
                    c.project().assets.len(),
                    c.doc.undo_label(),
                    c.clip_render.busy()
                ))
            }),
        ),
        DoC(
            "clip render plugin: Bounce and Cancel at once, while the plugins prepare",
            Box::new(move |_, ctl| {
                let mut c = ctl.borrow_mut();
                let s = s8.borrow();
                c.bounce_clip(ClipId(s.clip));
                let busy = c.clip_render.busy();
                let cancelled = c.cancel_clip_render();
                eprintln!("clip-render plugin: busy {busy}, cancel took effect {cancelled}");
            }),
        ),
        Check(
            "clip render plugin: Cancel left nothing behind and the interface is free",
            Box::new(move |c, _| {
                let s = s9.borrow();
                let ok = c.status_message() == Some("Render cancelled")
                    && files().len() == s.files_now
                    && c.project().assets.len() == s.assets_now
                    && c.project().tracks.len() == s.tracks_now
                    && c.doc.undo_label().map(str::to_string) == s.undo_now
                    && !c.clip_render.busy()
                    && c.pending_jobs == 0;
                ok.then_some(()).ok_or(format!("{:?}; busy {}; pending {}", c.status_message(), c.clip_render.busy(), c.pending_jobs))
            }),
        ),
        DoC(
            "clip render plugin: remove the test track, its asset and the scratch folder",
            Box::new(move |_, ctl| {
                let mut c = ctl.borrow_mut();
                let s = s10.borrow();
                let mut commands = vec![Command::RemoveTrack { track: TrackId(s.track) }];
                commands.extend(
                    c.project()
                        .assets
                        .iter()
                        .filter(|a| in_dir(&a.absolute_path, &dir()))
                        .map(|a| Command::RemoveAsset { asset: AssetId(a.id.0) }),
                );
                c.execute(Command::Batch { label: "clip render plugin cleanup".into(), commands });
                c.select_clip(None);
                c.clip_render.dir_override = None;
                let _ = std::fs::remove_dir_all(dir());
            }),
        ),
        Check(
            "clip render plugin: project back to its earlier tracks and assets",
            Box::new(move |c, _| {
                let s = s11.borrow();
                let p = c.project();
                (p.tracks.len() == s.tracks_before && p.assets.len() == s.assets_before).then_some(()).ok_or(format!(
                    "{} tracks {} assets",
                    p.tracks.len(),
                    p.assets.len()
                ))
            }),
        ),
    ]
}

/// Clip sidebar pattern list (DStudio's local library): Create, load a row, Original,
/// Type and Genre, each one undo step. Removes its clip at the end.
fn pattern_library_steps() -> Vec<Step> {
    use Step::*;
    let hist = Rc::new(Cell::new(0usize));
    fn one(hc: &Rc<Cell<usize>>, c: &Controller, label: &str) -> Result<(), String> {
        let n = c.doc.history_labels().len();
        let was = hc.get();
        hc.set(n);
        (n == was + 1 && c.doc.undo_label() == Some(label))
            .then_some(())
            .ok_or(format!("history {was} -> {n}, last {:?} (want {label})", c.doc.undo_label()))
    }
    fn comp(c: &Controller) -> Option<dstudio_core::library::Composer> {
        c.pr_clip().and_then(|m| m.composer.clone())
    }
    let h = |_: u8| hist.clone();
    let (h0, h1, h2, h3, h4, h5) = (h(0), h(1), h(2), h(3), h(4), h(5));
    let (h6, h7) = (h(6), h(7));
    let (h8, h9, h10, h11, hsync) = (h(8), h(9), h(10), h(11), h(12));
    fn shape_rows(a: &AppWindow) -> Vec<crate::ShapeRow> {
        let m = a.global::<Studio>().get_composer_shape_rows();
        (0..slint::Model::row_count(&m)).filter_map(|i| slint::Model::row_data(&m, i)).collect()
    }
    fn shaped(c: &Controller) -> Option<(Vec<dstudio_core::Note>, Vec<dstudio_core::Note>)> {
        let m = c.pr_clip()?;
        Some((m.composer.as_ref()?.base.clone(), m.notes.clone()))
    }
    vec![
        Do("patterns: Escape (close menus)", Box::new(|a| key(a, Key::Escape, false, false))),
        DoC(
            "patterns: Cmd+Shift+M on the instrument track at bar 61; select the clip",
            Box::new(move |a, ctl| {
                let mut c = ctl.borrow_mut();
                let Some(t) = track_of(&c, TrackKind::Instrument).map(|i| c.project().tracks[i].id) else { return };
                c.select_track(t);
                c.seek_beats(240.0);
                if let Some(id) = c.insert_midi_clip() {
                    c.select_clip(Some(id));
                }
                h0.set(c.doc.history_labels().len());
                c.update_ui(a);
            }),
        ),
        Check(
            "patterns: empty list, 'Create chords' pill",
            Box::new(|_, a| {
                let s = a.global::<Studio>();
                (slint::Model::row_count(&s.get_composer_rows()) == 0 && s.get_composer_create_text() == "Create chords")
                    .then_some(())
                    .ok_or(format!("{:?}", s.get_composer_create_text()))
            }),
        ),
        Do("patterns: click Create chords", Box::new(|a| click(a, bottom_x() + 60.0, bottom_end() - 37.5 + 13.5))),
        Check(
            "patterns: six chord patterns, the first plays in the clip (one undo step 'Create Patterns')",
            Box::new(move |c, a| {
                one(&h1, c, "Create Patterns")?;
                let k = comp(c).ok_or("no composer")?;
                let s = a.global::<Studio>();
                let notes = c.pr_clip().map_or(0, |m| m.notes.len());
                let ok = k.playing == Some(0)
                    && slint::Model::row_count(&s.get_composer_rows()) == 6
                    && notes > 0
                    && s.get_composer_create_text() == "Create new chords"
                    && s.get_composer_original();
                ok.then_some(()).ok_or(format!("playing {:?} notes {notes}", k.playing))
            }),
        ),
        Do("patterns: click the third row", Box::new(|a| click(a, bottom_x() + 60.0, bottom_y() + 104.0 + 25.0 + 2.0 * 35.0 + 16.0))),
        Check(
            "patterns: row 3 plays (one undo step 'Load Pattern')",
            Box::new(move |c, _| {
                one(&h2, c, "Load Pattern")?;
                (comp(c).and_then(|k| k.playing) == Some(2)).then_some(()).ok_or("row 3 not playing".into())
            }),
        ),
        Do(
            "patterns: click Original on the clip chip",
            Box::new(|a| click(a, bottom_x() + 10.5 + 234.0 - 8.0 - 20.0, bottom_y() + 38.0 + 15.25)),
        ),
        Check(
            "patterns: the clip's own (empty) notes are back (one undo step 'Load Original')",
            Box::new(move |c, a| {
                one(&h3, c, "Load Original")?;
                let ok = comp(c).is_some_and(|k| k.playing.is_none())
                    && c.pr_clip().is_some_and(|m| m.notes.is_empty())
                    && !a.global::<Studio>().get_composer_original();
                ok.then_some(()).ok_or("original not restored".into())
            }),
        ),
        DoC(
            "patterns: Type menu › Bass",
            Box::new(|a, ctl| {
                let mut c = ctl.borrow_mut();
                c.clip_menu_open(4);
                c.clip_menu_pick(1);
                c.update_ui(a);
            }),
        ),
        Check(
            "patterns: Bass set made and playing (one undo step 'Pattern Type Change')",
            Box::new(move |c, a| {
                one(&h4, c, "Pattern Type Change")?;
                let k = comp(c).ok_or("no composer")?;
                let ok = k.part == dstudio_core::library::PartType::Bass
                    && k.playing == Some(0)
                    && a.global::<Studio>().get_composer_part_text() == "Bass";
                ok.then_some(()).ok_or(format!("{:?} playing {:?}", k.part, k.playing))
            }),
        ),
        DoC(
            "patterns: Genre menu › Drill",
            Box::new(|a, ctl| {
                let mut c = ctl.borrow_mut();
                c.clip_menu_open(5);
                c.clip_menu_pick(3);
                c.update_ui(a);
            }),
        ),
        Check(
            "patterns: Genre reads Drill (one undo step 'Pattern Genre Change')",
            Box::new(move |c, a| {
                one(&h5, c, "Pattern Genre Change")?;
                (a.global::<Studio>().get_composer_genre_text() == "Drill").then_some(()).ok_or("genre not set".into())
            }),
        ),
        DoC(
            "patterns: open the Genre menu",
            Box::new(|a, ctl| {
                let mut c = ctl.borrow_mut();
                c.clip_menu_open(5);
                c.update_ui(a);
            }),
        ),
        Check(
            "patterns: Genre menu lists Any and the 21 genres, Drill lit",
            Box::new(|c, a| {
                let items = slint::Model::row_count(&a.global::<Studio>().get_clip_menu_items());
                let ok = items == 22 && c.composer_menu_active(true) == 3;
                ok.then_some(()).ok_or(format!("{items} items"))
            }),
        ),
        Do(
            "patterns: Escape; click the Shape tab",
            Box::new(|a| {
                key(a, Key::Escape, false, false);
                click(a, bottom_x() + 10.5 + 119.5 + 55.0, bottom_y() + 38.0 + 45.5);
            }),
        ),
        Check(
            "patterns: Shape shows 14 rows, Voices disabled on a bassline",
            Box::new(|_, a| {
                let s = a.global::<Studio>();
                let rows = s.get_composer_shape_rows();
                let voices = slint::Model::row_data(&rows, 1).ok_or("no rows")?;
                let ok = s.get_composer_shape_tab()
                    && slint::Model::row_count(&rows) == 14
                    && !voices.enabled
                    && voices.tip == "This pattern has no extra voices to add";
                ok.then_some(()).ok_or(format!("tab {} rows {}", s.get_composer_shape_tab(), slint::Model::row_count(&rows)))
            }),
        ),
        Do("patterns: Octave ›", Box::new(|a| click(a, bottom_x() + 10.5 + 72.0 + 120.0, bottom_y() + 104.0 + 26.0 + 7.5))),
        Check(
            "patterns: the bassline plays an octave up over its kept base (one undo step 'Shape Change')",
            Box::new(move |c, a| {
                one(&h6, c, "Shape Change")?;
                let (base, notes) = shaped(c).ok_or("no clip")?;
                let ok = !base.is_empty()
                    && base.len() == notes.len()
                    && base.iter().zip(&notes).all(|(b, n)| n.pitch == b.pitch + 12 && n.start == b.start);
                let octave = slint::Model::row_data(&a.global::<Studio>().get_composer_shape_rows(), 0).map(|r| r.value.to_string());
                (ok && octave.as_deref() == Some("+1")).then_some(()).ok_or(format!(
                    "{} base, {} notes, octave {octave:?}",
                    base.len(),
                    notes.len()
                ))
            }),
        ),
        Do(
            "patterns: click Density at a quarter",
            Box::new(|a| click(a, bottom_x() + 10.5 + 72.0 + 0.25 * 118.0, bottom_y() + 104.0 + 26.0 + 2.0 * 17.5 + 7.5)),
        ),
        Check(
            "patterns: Density thins the notes (one undo step 'Shape Change')",
            Box::new(move |c, _| {
                one(&h7, c, "Shape Change")?;
                let (base, notes) = shaped(c).ok_or("no clip")?;
                let d = comp(c).map(|k| k.shape_of(k.shape_source()).density).unwrap_or(1.0);
                ((0.2..0.3).contains(&d) && !notes.is_empty() && notes.len() < base.len()).then_some(()).ok_or(format!(
                    "density {d}, {} of {}",
                    notes.len(),
                    base.len()
                ))
            }),
        ),
        DoC(
            "patterns: undo twice",
            Box::new(|a, ctl| {
                let mut c = ctl.borrow_mut();
                c.undo();
                c.undo();
                c.update_ui(a);
            }),
        ),
        Check(
            "patterns: the bassline plays as made again",
            Box::new(|c, _| {
                let k = comp(c).ok_or("no composer")?;
                let made = k.set_for(Some(dstudio_core::library::PartType::Bass)).ok_or("no set")?;
                let p = c.project();
                let first =
                    dstudio_core::library::patterns_of(made, p.transport.key, c.pr_clip().map_or(0, |m| m.length), p.ticks_per_bar())
                        .remove(0);
                let ok = k.shapes.is_empty() && k.base.is_empty() && c.pr_clip().is_some_and(|m| m.notes == first.notes);
                ok.then_some(()).ok_or("shape not undone".into())
            }),
        ),
        Do("patterns: click the Patterns tab", Box::new(|a| click(a, bottom_x() + 10.5 + 5.0 + 55.0, bottom_y() + 38.0 + 45.5))),
        Check(
            "patterns: the list shows again",
            Box::new(|_, a| {
                let s = a.global::<Studio>();
                (!s.get_composer_shape_tab() && slint::Model::row_count(&s.get_composer_rows()) == 6)
                    .then_some(())
                    .ok_or("still on Shape".into())
            }),
        ),
        DoC(
            "patterns: Type › Chords, Shape tab, toolbar Shape on",
            Box::new(move |a, ctl| {
                let mut c = ctl.borrow_mut();
                hsync.set(c.doc.history_labels().len());
                c.clip_menu_open(4);
                c.clip_menu_pick(0);
                c.composer_tab(true);
                c.shape_draw_toggle();
                c.update_ui(a);
            }),
        ),
        Check(
            "patterns: chords show the chord rows enabled and the toolbar draws curves",
            Box::new(move |c, a| {
                one(&h8, c, "Pattern Type Change")?;
                let rows = shape_rows(a);
                let air = rows.iter().find(|r| r.label == "Air").ok_or("no Air row")?;
                let ok = a.global::<Studio>().get_shape_drawing()
                    && c.shape_drawing()
                    && air.enabled
                    && rows.len() == 14
                    && !a.global::<Studio>().get_shape_has_curves();
                ok.then_some(()).ok_or(format!("{} rows, Air enabled {}", rows.len(), air.enabled))
            }),
        ),
        DoC(
            "patterns: ramp a Velocity curve up across the bar",
            Box::new(|a, ctl| {
                let mut c = ctl.borrow_mut();
                c.shape_stroke(dstudio_core::shape::ShapeKey::Velocity, 2, &[(0.0, 0.2), (2.0, 0.6), (4.0, 1.0)]);
                c.update_ui(a);
            }),
        ),
        Check(
            "patterns: the curve plays (quiet first beat, full last), one undo step 'Shape Change'",
            Box::new(move |c, a| {
                one(&h9, c, "Shape Change")?;
                let k = comp(c).ok_or("no composer")?;
                let curved = k.shape_of(k.shape_source()).curve(dstudio_core::shape::ShapeKey::Velocity).is_some();
                let (base, notes) = shaped(c).ok_or("no clip")?;
                let vel = |v: &[dstudio_core::Note], b: i64| {
                    v.iter().filter(|n| n.start / PPQ == b).map(|n| n.velocity as u32).max().unwrap_or(0)
                };
                let ok = curved
                    && vel(&notes, 0) < vel(&base, 0)
                    && vel(&notes, 3) >= vel(&notes, 0)
                    && a.global::<Studio>().get_shape_has_curves();
                let row = shape_rows(a).into_iter().find(|r| r.label == "Velocity").ok_or("no row")?;
                (ok && row.selected && row.curved).then_some(()).ok_or(format!(
                    "curved {curved}, first {} -> {}",
                    vel(&base, 0),
                    vel(&notes, 0)
                ))
            }),
        ),
        DoC(
            "patterns: toolbar Reset",
            Box::new(|a, ctl| {
                let mut c = ctl.borrow_mut();
                c.shape_reset();
                c.update_ui(a);
            }),
        ),
        Check(
            "patterns: Reset clears the curves (one undo step 'Shape Change')",
            Box::new(move |c, a| {
                one(&h10, c, "Shape Change")?;
                let k = comp(c).ok_or("no composer")?;
                (k.shapes.is_empty() && k.base.is_empty() && !a.global::<Studio>().get_shape_has_curves())
                    .then_some(())
                    .ok_or("curves left".into())
            }),
        ),
        Do(
            "patterns: scroll the Shape rack to its last row; Arp ›",
            Box::new(|a| {
                let x = bottom_x() + 10.5 + 72.0 + 120.0;
                dispatch(
                    a.window(),
                    WindowEvent::PointerScrolled { position: LogicalPosition::new(x, bottom_y() + 200.0), delta_x: 0.0, delta_y: -1000.0 },
                );
                click(a, x, bottom_end() - 8.0 - 17.5 + 7.5);
            }),
        ),
        Check(
            "patterns: Arp 1/8 plays and the Pattern and Octaves rows join (one undo step 'Shape Change')",
            Box::new(move |c, a| {
                one(&h11, c, "Shape Change")?;
                let rows = shape_rows(a);
                let arp = rows.iter().find(|r| r.label == "Arp").map(|r| r.value.to_string());
                let pattern = rows.iter().find(|r| r.label == "Pattern").map(|r| r.value.to_string());
                let ok = rows.len() == 16 && arp.as_deref() == Some("1/8") && pattern.as_deref() == Some("Up & down");
                ok.then_some(()).ok_or(format!("{} rows, arp {arp:?}, pattern {pattern:?}", rows.len()))
            }),
        ),
        DoC(
            "patterns: remove the clip",
            Box::new(|_, ctl| {
                let mut c = ctl.borrow_mut();
                c.composer_shape_tab = None;
                c.shape_ui = Default::default();
                if let Some(id) = c.pr_clip().map(|m| m.id) {
                    c.execute(dstudio_core::Command::RemoveMidiClip { clip: id });
                }
                c.select_clip(None);
                c.set_bottom_tab(crate::controller::BottomTab::Devices);
                c.seek_beats(0.0);
            }),
        ),
        Check(
            "patterns: clip removed",
            Box::new(|c, _| {
                let left = track_of(c, TrackKind::Instrument)
                    .map_or(0, |i| c.project().tracks[i].midi_clips.iter().filter(|m| m.start >= 240 * PPQ).count());
                (left == 0).then_some(()).ok_or(format!("{left} clips left"))
            }),
        ),
    ]
}

/// Timeline shortcuts: ⇧⌘T adds an instrument track, ⌘R reverses the selected clip (MIDI notes
/// mirrored, audio played backwards), ⌘L loops the selected clip or section, ⌘D duplicates the
/// selected section. Real key presses through the keymap.
fn shortcut_steps() -> Vec<Step> {
    use Step::*;
    let tracks = Rc::new(Cell::new(0usize));
    let (t0, t1) = (tracks.clone(), tracks.clone());
    let clip: Rc<Cell<Option<dstudio_core::ClipId>>> = Rc::new(Cell::new(None));
    let (c0, c1, c2) = (clip.clone(), clip.clone(), clip.clone());
    let audio: Rc<Cell<Option<(dstudio_core::ClipId, bool)>>> = Rc::new(Cell::new(None));
    let (a0, a1, a2) = (audio.clone(), audio.clone(), audio.clone());
    let sections = Rc::new(Cell::new(0usize));
    let (s0, s1) = (sections.clone(), sections.clone());
    let unfocus = |c: &mut Controller| {
        c.pr.focused = false;
        c.mixer_open = false;
    };
    vec![
        DoC(
            "shortcuts: count the tracks, focus the timeline",
            Box::new(move |_, ctl| {
                let mut c = ctl.borrow_mut();
                unfocus(&mut c);
                c.select_clip(None);
                c.selected_section = None;
                t0.set(c.project().tracks.len());
            }),
        ),
        Do("shortcuts: Cmd+Shift+T", Box::new(|a| key(a, "T", true, true))),
        Check(
            "shortcuts: a new Instrument track, selected, one undo step",
            Box::new(move |c, _| {
                let p = c.project();
                let t = p.tracks.iter().rev().find(|t| t.kind == TrackKind::Instrument).ok_or("no instrument track")?;
                let ok = p.tracks.len() == t1.get() + 1
                    && t.name == "Instrument"
                    && c.selected_track == Some(t.id)
                    && c.doc.undo_label() == Some("Add Instrument Track");
                ok.then_some(()).ok_or(format!(
                    "tracks {} -> {} name {} selected {:?} last {:?}",
                    t1.get(),
                    p.tracks.len(),
                    t.name,
                    c.selected_track,
                    c.doc.undo_label()
                ))
            }),
        ),
        DoC(
            "shortcuts: a one-bar MIDI clip on it at beat 96 with notes at 0 and 1 beat, selected",
            Box::new(move |_, ctl| {
                let mut c = ctl.borrow_mut();
                c.seek_beats(96.0);
                let id = c.insert_midi_clip();
                if let Some(id) = id {
                    let n = |i: u32, start: i64| dstudio_core::Note { id: i, pitch: 60, start, length: PPQ / 2, velocity: 100 };
                    c.execute(dstudio_core::Command::SetMidiNotes { clip: id, notes: vec![n(1, 0), n(2, PPQ)] });
                }
                c.select_clip(id);
                unfocus(&mut c);
                c0.set(id);
            }),
        ),
        Do("shortcuts: Cmd+R", Box::new(|a| key(a, "r", true, false))),
        Check(
            "shortcuts: the notes are mirrored in the bar, one Reverse Clip step",
            Box::new(move |c, _| {
                let m = c1.get().and_then(|id| c.project().midi_clip(id)).ok_or("no clip")?;
                // Notes are kept in start order: the mirrored second note comes first.
                let got: Vec<(u32, i64)> = m.notes.iter().map(|n| (n.id, n.start)).collect();
                let want = vec![(2, 3 * PPQ - PPQ / 2), (1, 4 * PPQ - PPQ / 2)];
                (got == want && c.doc.undo_label() == Some("Reverse Clip"))
                    .then_some(())
                    .ok_or(format!("notes {got:?} want {want:?} last {:?}", c.doc.undo_label()))
            }),
        ),
        Do("shortcuts: Cmd+L", Box::new(|a| key(a, "l", true, false))),
        Check(
            "shortcuts: the loop is the clip's bar, on",
            Box::new(move |c, _| {
                let m = c2.get().and_then(|id| c.project().midi_clip(id)).ok_or("no clip")?;
                let tr = &c.project().transport;
                (tr.loop_enabled && tr.loop_start == m.start && tr.loop_end == m.end()).then_some(()).ok_or(format!(
                    "loop {} {}..{} clip {}..{}",
                    tr.loop_enabled,
                    tr.loop_start,
                    tr.loop_end,
                    m.start,
                    m.end()
                ))
            }),
        ),
        DoC(
            "shortcuts: select an audio clip",
            Box::new(move |_, ctl| {
                let mut c = ctl.borrow_mut();
                let found = c.project().tracks.iter().flat_map(|t| t.clips.iter()).next().map(|cl| (cl.id, cl.warp.reverse));
                if let Some((id, _)) = found {
                    if let Some(t) = c.clip_track(id) {
                        c.select_track(t);
                    }
                    c.select_clip(Some(id));
                }
                unfocus(&mut c);
                a0.set(found);
            }),
        ),
        Do("shortcuts: Cmd+R on it", Box::new(|a| key(a, "r", true, false))),
        Check(
            "shortcuts: the audio clip plays backwards",
            Box::new(move |c, _| {
                let Some((id, was)) = a1.get() else { return Ok(()) };
                let now = c.project().clip(id).map(|cl| cl.warp.reverse);
                (now == Some(!was) && c.doc.undo_label() == Some("Reverse Clip"))
                    .then_some(())
                    .ok_or(format!("reverse {now:?} (was {was}) last {:?}", c.doc.undo_label()))
            }),
        ),
        DoC(
            "shortcuts: undo the audio reverse",
            Box::new(move |_, ctl| {
                if a2.get().is_some() {
                    ctl.borrow_mut().undo();
                }
            }),
        ),
        DoC(
            "shortcuts: add a section and select it",
            Box::new(move |_, ctl| {
                let mut c = ctl.borrow_mut();
                c.select_clip(None);
                c.insert_section(None);
                unfocus(&mut c);
                s0.set(c.project().sections.len());
            }),
        ),
        Do(
            "shortcuts: Cmd+D, then Cmd+L",
            Box::new(|a| {
                key(a, "d", true, false);
                key(a, "l", true, false);
            }),
        ),
        Check(
            "shortcuts: the section was duplicated and the loop is the copy",
            Box::new(move |c, _| {
                let p = c.project();
                let i = c.selected_section.and_then(|s| p.section_index(s)).ok_or("no section selected")?;
                let (start, end) = (p.section_start(i), p.section_start(i) + p.sections[i].length);
                let tr = &p.transport;
                let ok = p.sections.len() == s1.get() + 1 && tr.loop_enabled && (tr.loop_start, tr.loop_end) == (start, end);
                ok.then_some(()).ok_or(format!(
                    "sections {} -> {} loop {}..{} section {start}..{end}",
                    s1.get(),
                    p.sections.len(),
                    tr.loop_start,
                    tr.loop_end
                ))
            }),
        ),
    ]
}

/// Piano-roll velocity lane: the toolbar toggle opens it under the grid; dragging a stem moves
/// the note's velocity, a sweep across empty lane paints the stems it passes; each gesture is one
/// undo step.
/// Bug: the piano roll showed only the first of several notes added one after another (a
/// double-click each, as by hand) until the panel was switched away and back. The bottom panel's
/// rounded corners make Slint keep it as a cached picture that it redraws only when one of its
/// properties changes; the note grid is our own texture, so a new note changed nothing it watched.
/// These steps look at the screen itself (a snapshot through Slint's caches), not the document.
fn note_redraw_steps() -> Vec<Step> {
    use Step::*;
    let shots: Rc<RefCell<Vec<Shot>>> = Rc::default();
    let (s0, s1, s2, s3, s4) = (shots.clone(), shots.clone(), shots.clone(), shots.clone(), shots);
    let look = |slot: Rc<RefCell<Vec<Shot>>>| -> Box<dyn Fn(&AppWindow, &Ctl)> {
        Box::new(move |a, _| {
            if let Some(shot) = Shot::take(a) {
                slot.borrow_mut().push(shot);
            }
        })
    };
    // Double-click in the middle of beat `beat`'s first 1/16 on pitch `pitch` (pen off).
    let add = |beat: f64, pitch: i32| -> Box<dyn Fn(&AppWindow, &Ctl)> {
        Box::new(move |a, ctl| {
            let (x, y) = pr_xy(ctl, beat + 0.125, pitch);
            double_click(a, x, y);
        })
    };
    // Window points at the centres of the open clip's notes.
    fn note_centres(c: &Controller) -> Vec<(f32, f32)> {
        let v = c.pr_view();
        let notes = c.pr_clip().map(|m| m.notes.clone()).unwrap_or_default();
        notes
            .iter()
            .map(|n| {
                let (x, y, w, h) = v.note_rect(n);
                (pr_origin().0 + x + w / 2.0, pr_origin().1 + y + h / 2.0)
            })
            .collect()
    }
    let colours = |shot: &Shot, pts: &[(f32, f32)]| -> Vec<[u8; 3]> { pts.iter().filter_map(|&(x, y)| shot.at(x, y)).collect() };
    let centres: Rc<RefCell<Vec<(f32, f32)>>> = Rc::default();
    let (c1, c2, c3) = (centres.clone(), centres.clone(), centres);
    vec![
        DoC(
            "note redraw: a new instrument track with an empty MIDI clip open in the piano roll, pen off",
            Box::new(|a, ctl| {
                let mut c = ctl.borrow_mut();
                c.mixer_open = false;
                let color = crate::controller::next_track_color(c.project());
                let track = dstudio_core::Track::new(dstudio_core::TrackId(0), TrackKind::Instrument, "Notes", color);
                c.execute(dstudio_core::Command::AddTrack { index: None, track });
                if let Some(t) = c.project().tracks.last().map(|t| t.id) {
                    c.select_track(t);
                }
                c.seek_beats(96.0);
                c.insert_and_open_midi_clip();
                c.snap.grid = Some(crate::timeline::GridSize::Sixteenth);
                (c.pr.pen, c.pr.vel_lane) = (false, false);
                // As a click does: the interface takes the new state before the next frame.
                c.update_ui(a);
            }),
        ),
        DoC(
            "note redraw: scroll D4 to the middle of the grid (once the editor has its size)",
            Box::new(|a, ctl| {
                let mut c = ctl.borrow_mut();
                let v = c.pr_view();
                c.pr.scroll = (v.y_of(62) + v.scroll - crate::pianoroll::GRID_Y - (v.h - crate::pianoroll::GRID_Y) / 2.0).max(0.0);
                c.update_ui(a);
            }),
        ),
        Check(
            "note redraw: the empty clip is open",
            Box::new(|c, a| {
                (c.pr_clip().is_some_and(|m| m.notes.is_empty()) && a.global::<Studio>().get_pr_visible())
                    .then_some(())
                    .ok_or("no empty clip open".into())
            }),
        ),
        DoC("note redraw: look at the screen", look(s0)),
        DoC("note redraw: double-click C4 at beat 1", add(0.0, 60)),
        DoC("note redraw: double-click D4 at beat 2", add(1.0, 62)),
        DoC("note redraw: double-click E4 at beat 3", add(2.0, 64)),
        Check(
            "note redraw: the clip holds the 3 notes",
            Box::new(move |c, _| {
                *c1.borrow_mut() = note_centres(c);
                let got: Vec<u8> = c.pr_clip().map(|m| m.notes.iter().map(|n| n.pitch).collect()).unwrap_or_default();
                (got == [60, 62, 64]).then_some(()).ok_or(format!("pitches {got:?}"))
            }),
        ),
        DoC("note redraw: look at the screen again", look(s1)),
        Check(
            "note redraw: all 3 notes show on screen, not only the first",
            Box::new(move |_, _| {
                let (shots, pts) = (s2.borrow(), c2.borrow());
                let [before, after] = [0, 1].map(|i| shots.get(i).map(|s| colours(s, &pts)).unwrap_or_default());
                if pts.len() != 3 || before.len() != 3 || after.len() != 3 {
                    return Err(format!("{} notes, {} snapshots", pts.len(), shots.len()));
                }
                let diffs: Vec<u8> = before.iter().zip(&after).map(|(b, a)| px_diff(*b, *a)).collect();
                diffs
                    .iter()
                    .all(|&d| d > 40)
                    .then_some(())
                    .ok_or(format!("change at C4/D4/E4 {diffs:?} (before {before:?}, after {after:?})"))
            }),
        ),
        Do("note redraw: Cmd+Z", Box::new(|a| key(a, "z", true, false))),
        DoC("note redraw: look once more", look(s3)),
        Check(
            "note redraw: undo takes E4 off the screen, C4 and D4 stay",
            Box::new(move |c, _| {
                let shots = s4.borrow();
                let pts = note_centres(c);
                let gone = shots.first().zip(shots.get(1)).zip(shots.get(2));
                let Some(((before, drawn), now)) = gone else { return Err(format!("{} snapshots", shots.len())) };
                // Where E4 was: back to the empty grid. Where C4 and D4 are: still the drawn notes.
                let e4 = c3.borrow().get(2).copied().ok_or("no E4 centre")?;
                let (b, n) = (before.at(e4.0, e4.1).unwrap_or_default(), now.at(e4.0, e4.1).unwrap_or_default());
                let kept: Vec<u8> =
                    pts.iter().map(|&(x, y)| px_diff(drawn.at(x, y).unwrap_or_default(), now.at(x, y).unwrap_or_default())).collect();
                (pts.len() == 2 && px_diff(b, n) <= 12 && kept.iter().all(|&d| d <= 12))
                    .then_some(())
                    .ok_or(format!("E4 spot before {b:?} now {n:?}; C4/D4 change {kept:?}"))
            }),
        ),
    ]
}

fn velocity_lane_steps() -> Vec<Step> {
    use crate::pianoroll::VEL_H;
    use Step::*;
    let hist = Rc::new(Cell::new(0usize));
    let (h1, h2, h3) = (hist.clone(), hist.clone(), hist.clone());
    let clip: Rc<Cell<Option<dstudio_core::ClipId>>> = Rc::new(Cell::new(None));
    let (c0, c1, c2, c3, c4, c5) = (clip.clone(), clip.clone(), clip.clone(), clip.clone(), clip.clone(), clip.clone());
    let vels = |c: &Controller, id: Option<dstudio_core::ClipId>| -> Vec<u8> {
        id.and_then(|id| c.project().midi_clip(id)).map(|m| m.notes.iter().map(|n| n.velocity).collect()).unwrap_or_default()
    };
    // Centre of the toolbar's velocity button (ui/pianoroll.slint: 280 from the panel's right edge).
    fn vel_button(ctl: &Ctl) -> (f32, f32) {
        (pr_origin().0 + ctl.borrow().pr.size.0 + 0.5 - 270.0, bottom_y() + 41.0)
    }
    // Window point on note `beat`'s stem at velocity `vel`.
    fn stem(ctl: &Ctl, beat: f64, vel: u8) -> (f32, f32) {
        let v = ctl.borrow().pr_view();
        (pr_origin().0 + v.x_of(beat), pr_origin().1 + crate::pianoroll::vel_y(&v, vel))
    }
    vec![
        DoC(
            "velocity: a new instrument track with a MIDI clip of four quarter notes at velocity 100, open in the piano roll",
            Box::new(move |_, ctl| {
                let mut c = ctl.borrow_mut();
                c.mixer_open = false;
                // A track of its own: the first instrument track has a Drum Rack by now (step patterns).
                let color = crate::controller::next_track_color(c.project());
                let track = dstudio_core::Track::new(dstudio_core::TrackId(0), TrackKind::Instrument, "Keys", color);
                c.execute(dstudio_core::Command::AddTrack { index: None, track });
                let inst = c.project().tracks.last().map(|t| t.id);
                if let Some(t) = inst {
                    c.select_track(t);
                }
                c.seek_beats(64.0);
                let id = c.insert_midi_clip();
                if let Some(id) = id {
                    let notes = (0..4)
                        .map(|i| dstudio_core::Note {
                            id: i as u32 + 1,
                            pitch: 60 + 2 * i as u8,
                            start: i * PPQ,
                            length: PPQ / 2,
                            velocity: 100,
                        })
                        .collect();
                    c.execute(dstudio_core::Command::SetMidiNotes { clip: id, notes });
                }
                c.select_clip(id);
                c.bottom_tab = BottomTab::Clip;
                c.pr_sync_clip();
                c.pr.selected.clear();
                c.pr.vel_lane = false;
                c.ui_dirty = true;
                c0.set(id);
            }),
        ),
        Check(
            "velocity: the piano roll shows the clip, lane closed",
            Box::new(move |c, a| {
                let open = c.pr_clip().map(|m| m.id) == c1.get() && c1.get().is_some();
                (open && !c.pr.vel_lane && !a.global::<Studio>().get_pr_vel_lane()).then_some(()).ok_or(format!(
                    "pr clip {:?} lane {}",
                    c.pr_clip().map(|m| m.id),
                    c.pr.vel_lane
                ))
            }),
        ),
        DoC(
            "velocity: click the toolbar's velocity button",
            Box::new(|a, ctl| {
                let (x, y) = vel_button(ctl);
                click(a, x, y);
            }),
        ),
        Check(
            "velocity: the lane is open and the grid gave it its height",
            Box::new(move |c, a| {
                h1.set(c.doc.history_labels().len());
                let v = c.pr_view();
                let ok =
                    c.pr.vel_lane && a.global::<Studio>().get_pr_vel_lane() && v.lane == VEL_H && (v.h + v.lane - c.pr.size.1).abs() < 1.0;
                ok.then_some(()).ok_or(format!("lane {} grid h {} texture h {}", c.pr.vel_lane, v.h, c.pr.size.1))
            }),
        ),
        DoC(
            "velocity: drag note 2's stem up by 20",
            Box::new(|a, ctl| {
                let (x, y) = stem(ctl, 1.0, 100);
                let dy = 20.0 / 127.0 * (ctl.borrow().pr_view().lane - 12.0);
                drag(a, x, y, x, y - dy);
            }),
        ),
        Check(
            "velocity: note 2 at 120, the rest at 100, note 2 selected, one Note Velocity step",
            Box::new(move |c, _| {
                let got = vels(c, c2.get());
                let n = c.doc.history_labels().len();
                let (was, last) = (h2.get(), c.doc.undo_label());
                h2.set(n);
                let ok = got == [100, 120, 100, 100]
                    && c.pr.selected.len() == 1
                    && c.pr.selected.contains(&2)
                    && n == was + 1
                    && last == Some("Note Velocity");
                ok.then_some(()).ok_or(format!("vels {got:?} sel {:?} history {was} -> {n} last {last:?}", c.pr.selected))
            }),
        ),
        DoC(
            "velocity: clear the selection, sweep the lane at 40 from beat 1.5 past beat 3",
            Box::new(|a, ctl| {
                ctl.borrow_mut().pr.selected.clear();
                let (x0, y) = stem(ctl, 1.5, 40);
                let (x1, _) = stem(ctl, 3.0, 40);
                drag(a, x0, y, x1 + 2.0, y);
            }),
        ),
        Check(
            "velocity: notes 3 and 4 at 40, notes 1 and 2 kept, one more step",
            Box::new(move |c, _| {
                let got = vels(c, c3.get());
                let n = c.doc.history_labels().len();
                let ok = got == [100, 120, 40, 40] && n == h3.get() + 1 && c.doc.undo_label() == Some("Note Velocity");
                ok.then_some(()).ok_or(format!("vels {got:?} history {n} (was {})", h3.get()))
            }),
        ),
        DoC(
            "velocity: undo twice",
            Box::new(|_, ctl| {
                let mut c = ctl.borrow_mut();
                c.undo();
                c.undo();
            }),
        ),
        Check(
            "velocity: all four back at 100",
            Box::new(move |c, _| {
                let got = vels(c, c4.get());
                (got == [100, 100, 100, 100]).then_some(()).ok_or(format!("vels {got:?}"))
            }),
        ),
        DoC(
            "velocity: click the toolbar button again",
            Box::new(|a, ctl| {
                let (x, y) = vel_button(ctl);
                click(a, x, y);
            }),
        ),
        Check(
            "velocity: the lane is closed and the grid has its height back",
            Box::new(move |c, a| {
                let v = c.pr_view();
                let ok = !c.pr.vel_lane && !a.global::<Studio>().get_pr_vel_lane() && (v.h - c.pr.size.1).abs() < 1.0 && c5.get().is_some();
                ok.then_some(()).ok_or(format!("lane {} grid h {} texture h {}", c.pr.vel_lane, v.h, c.pr.size.1))
            }),
        ),
    ]
}

/// Run the scripted steps, one per timer tick. Prints a summary and quits.
pub fn start(app: &AppWindow, ctl: Ctl) {
    APP.with(|a| *a.borrow_mut() = Some(app.as_weak()));
    let mut all = steps();
    // Auto snap follows the ruler step (two beats at the default zoom); the sections below that
    // drag clips to beats pin the grid to a beat (1/4), and snap_steps puts Auto back.
    all.insert(
        0,
        Step::DoC(
            "snap: pin the grid to a beat for the drag sections",
            Box::new(|_, ctl| ctl.borrow_mut().snap.grid = Some(crate::timeline::GridSize::Quarter)),
        ),
    );
    all.extend(synth_steps());
    all.extend(midi_steps());
    all.extend(recording_steps());
    all.extend(platform_device_steps());
    all.extend(builtin_device_steps());
    all.extend(dynamics_device_steps());
    all.extend(modulation_steps());
    all.extend(builtin_each_steps());
    all.extend(modulation_device_steps());
    all.extend(custom_view_steps());
    all.extend(limiter_family_steps());
    all.extend(restoration_device_steps());
    all.extend(analyzer_device_steps());
    all.extend(sampler_steps());
    all.extend(time_fx_steps());
    all.extend(automation_steps());
    all.extend(field_device_steps());
    all.extend(step_sequencer_steps());
    all.extend(pattern_library_steps());
    all.extend(splitter_steps());
    all.extend(chat_dock_steps());
    all.extend(files_steps());
    all.extend(section_steps());
    all.extend(minimap_steps());
    all.extend(devminimap_steps());
    all.extend(clip_menu_steps());
    all.extend(clip_selection_steps());
    all.extend(clip_fade_steps());
    all.extend(clip_render_steps());
    all.extend(clip_render_plugin_steps());
    all.extend(ruler_zoom_steps());
    all.extend(snap_steps());
    all.extend(transport_fields_steps());
    all.extend(track_rename_steps());
    all.extend(track_menu_steps());
    all.extend(group_steps());
    all.extend(zoom_fit_steps());
    all.extend(menu_bar_steps());
    all.extend(general_settings_steps());
    all.extend(add_search_steps());
    all.extend(live_import_steps());
    all.extend(strip_steps());
    all.extend(returns_steps());
    all.extend(plugin_folder_steps());
    all.extend(shortcut_steps());
    all.extend(velocity_lane_steps());
    all.extend(note_redraw_steps());
    // The checks aim at a 1440×820 pt window. A window opened larger than the screen is shrunk to
    // fit when it is shown (macOS; GitHub's Mac runners have a 1024×768 display), so ask again
    // once it is on screen, and say what we got.
    all.insert(
        0,
        Step::Do(
            "window: ask for 1440×820 pt again now that it is shown",
            Box::new(|a| {
                let s = a.window().size().to_logical(a.window().scale_factor());
                eprintln!("self-test: window {}×{} pt at scale {}", s.width, s.height, a.window().scale_factor());
                if s.width < 1440.0 || s.height < 820.0 {
                    a.window().set_size(slint::LogicalSize::new(s.width.max(1440.0), s.height.max(820.0)));
                }
            }),
        ),
    );
    // The width is what the click targets need (a narrower window squeezes the bottom panels); a
    // shorter window only scrolls the lanes, which the steps handle. GitHub's Mac gives 1440×677.
    // The resize is asynchronous (a window-manager round trip on X11): give it about a second.
    for i in 1..=8 {
        all.insert(i, Step::Do("window: wait for the resize", Box::new(|_| {})));
    }
    all.insert(
        9,
        Step::Check(
            "window: at least 1440 pt wide and 600 pt tall, the room the click targets assume",
            Box::new(|_, a| {
                let s = a.window().size().to_logical(a.window().scale_factor());
                (s.width >= 1440.0 && s.height >= 600.0)
                    .then_some(())
                    .ok_or(format!("window {}×{} pt: the screen is too small for the self-test", s.width, s.height))
            }),
        ),
    );
    let steps = Rc::new(all);
    let idx = Rc::new(std::cell::Cell::new(0usize));
    let fails = Rc::new(std::cell::Cell::new(0usize));
    let skips = Rc::new(std::cell::Cell::new(0usize));
    let timer = Rc::new(slint::Timer::default());
    let aw = app.as_weak();
    let t2 = timer.clone();
    timer.start(slint::TimerMode::Repeated, std::time::Duration::from_millis(120), move || {
        let Some(app) = aw.upgrade() else { return };
        {
            let Ok(mut c) = ctl.try_borrow_mut() else { return };
            if c.pending_jobs > 0 {
                c.poll_jobs();
                return;
            }
        }
        let i = idx.get();
        if i >= steps.len() {
            let n = steps.iter().filter(|s| matches!(s, Step::Check(..))).count() - skips.get();
            let skipped = if skips.get() > 0 { format!(" ({} skipped)", skips.get()) } else { String::new() };
            let fixture = ctl.try_borrow().map(|c| c.devices.catalog.fixture.is_some()).unwrap_or(false);
            let note =
                if fixture { " [plugin catalogue from a TEST fixture: the plugin checks did not exercise the engine scanner]" } else { "" };
            eprintln!(
                "self-test: {} frames waited for the GPU (at most {} in flight)",
                crate::gpu::THROTTLED_FRAMES.load(std::sync::atomic::Ordering::Relaxed),
                crate::gpu::MAX_FRAMES_IN_FLIGHT
            );
            println!("self-test: {} / {n} checks passed{skipped}{note}", n - fails.get());
            finish_run(fails.get());
            t2.stop();
            let _ = slint::quit_event_loop();
            return;
        }
        match &steps[i] {
            Step::DoC(name, f) => {
                eprintln!("self-test: > {name}");
                if let Ok(mut c) = ctl.try_borrow_mut() {
                    c.update_ui(&app);
                }
                f(&app, &ctl);
            }
            Step::Do(name, f) => {
                eprintln!("self-test: > {name}");
                // Push models first, as a rendered frame would: when the window is not
                // presenting frames (occluded/locked screen) the UI would otherwise act
                // on stale view models.
                if let Ok(mut c) = ctl.try_borrow_mut() {
                    c.update_ui(&app);
                }
                f(&app);
            }
            Step::Check(name, f) => {
                // Let the frame update readouts first.
                if let Ok(mut c) = ctl.try_borrow_mut() {
                    c.update_ui(&app);
                }
                let r = f(&ctl.borrow(), &app);
                match r {
                    Ok(()) => println!("PASS {name}"),
                    // A guarded check that did not run (e.g. needs a plugin window): not counted.
                    Err(e) if e.starts_with("SKIP: ") => {
                        skips.set(skips.get() + 1);
                        println!("SKIP {name}: {}", &e[6..]);
                    }
                    Err(e) => {
                        fails.set(fails.get() + 1);
                        println!("FAIL {name}: {e}");
                    }
                }
            }
        }
        idx.set(i + 1);
    });
    // Keep the timer alive for the lifetime of the event loop.
    std::mem::forget(timer);
}

/// File › Import Ableton Live Set…: a set written by our own exporter (a warped audio clip on a
/// generated WAV, a MIDI clip) replaces the project after the unsaved-changes prompt.
fn live_import_steps() -> Vec<Step> {
    use crate::live_set::{LiveSet, SetAudioClip, SetContent, SetMidiClip, SetNote, SetTrack};
    use Step::*;
    let dir = || run_dir().join("live");
    vec![
        DoC(
            "live import: write a set and its sample, then File › Import Ableton Live Set…",
            Box::new(move |_, ctl| {
                let d = dir();
                let _ = std::fs::remove_dir_all(&d);
                std::fs::create_dir_all(d.join("Samples")).unwrap();
                let wav = d.join("Samples/loop.wav");
                let samples: Vec<f32> =
                    (0..48_000 * 4).map(|i| 0.3 * (std::f32::consts::TAU * 220.0 * i as f32 / 48_000.0).sin()).collect();
                crate::wavgen::write_wav(&wav, 48_000, 1, &samples).unwrap();
                let set = LiveSet {
                    bpm: 100.0,
                    numerator: 4,
                    denominator: 4,
                    loop_on: false,
                    loop_start: 0.0,
                    loop_length: 16.0,
                    returns: Vec::new(),
                    tracks: vec![
                        SetTrack {
                            name: "Loop".into(),
                            color: 3,
                            on: true,
                            volume: 0.5,
                            pan: 0.0,
                            sends: Vec::new(),
                            sends_only: false,
                            content: SetContent::Audio(vec![SetAudioClip {
                                name: "loop".into(),
                                start: 4.0,
                                length: 4.0,
                                relative_path: "Samples/loop.wav".into(),
                                absolute_path: wav.display().to_string(),
                                file_size: std::fs::metadata(&wav).map_or(0, |m| m.len()),
                                frames: 192_000,
                                sample_rate: 48_000,
                                markers: vec![(0.0, 0.0), (1.0, 2.0), (3.0, 4.0)],
                            }]),
                        },
                        SetTrack {
                            name: "Keys".into(),
                            color: 9,
                            on: true,
                            volume: 1.0,
                            pan: 0.0,
                            sends: Vec::new(),
                            sends_only: false,
                            content: SetContent::Midi(vec![SetMidiClip {
                                name: "keys".into(),
                                start: 0.0,
                                length: 4.0,
                                notes: vec![SetNote { pitch: 60, time: 0.0, duration: 1.0, velocity: 100 }],
                            }]),
                        },
                    ],
                };
                let kick = d.join("Samples/kick.wav");
                let hit: Vec<f32> = (0..4_800).map(|i| 0.5 * (1.0 - i as f32 / 4_800.0)).collect();
                crate::wavgen::write_wav(&kick, 48_000, 1, &hit).unwrap();
                let rack = format!(
                    r#"<Devices><DrumGroupDevice Id="0"><Branches><DrumBranch Id="0"><BranchInfo><ReceivingNote Value="92" /></BranchInfo><DeviceChain><MidiToAudioDeviceChain><Devices><OriginalSimpler Id="0"><Player><MultiSampleMap><SampleParts><MultiSamplePart Id="0"><SampleRef><FileRef><RelativePathType Value="3" /><RelativePath Value="Samples/kick.wav" /><Path Value="{}" /></FileRef><DefaultDuration Value="4800" /></SampleRef></MultiSamplePart></SampleParts></MultiSampleMap></Player><Globals><PlaybackMode Value="1" /></Globals></OriginalSimpler></Devices></MidiToAudioDeviceChain></DeviceChain></DrumBranch></Branches></DrumGroupDevice></Devices>"#,
                    kick.display()
                );
                let xml = crate::live_set::to_xml(&set);
                let at = xml.find("<MidiTrack").unwrap();
                let at = at + xml[at..].find("<Devices />").unwrap();
                let xml = format!("{}{rack}{}", &xml[..at], &xml[at + "<Devices />".len()..]);
                let fx = r#"<Devices><PluginDevice Id="0"><PluginDesc><Vst3PluginInfo Id="0"><Name Value="Missing FX" /><Uid><Fields.0 Value="1" /><Fields.1 Value="2" /><Fields.2 Value="3" /><Fields.3 Value="4" /></Uid><DeviceType Value="2" /></Vst3PluginInfo></PluginDesc></PluginDevice></Devices>"#;
                let at = xml.find("<AudioTrack").unwrap();
                let at = at + xml[at..].find("<Devices />").unwrap();
                let xml = format!("{}{fx}{}", &xml[..at], &xml[at + "<Devices />".len()..]);
                let als = d.join("Imported Set.als");
                std::fs::write(&als, crate::live_set::gzip(&xml).unwrap()).unwrap();
                ctl.borrow_mut().import_live_set_request(als);
            }),
        ),
        Check(
            "live import: unsaved changes are asked about first",
            Box::new(|c, a| {
                (c.unsaved_open && c.pending_import.is_some() && a.global::<Studio>().get_unsaved_open()).then_some(()).ok_or(format!(
                    "prompt {} pending {}",
                    c.unsaved_open,
                    c.pending_import.is_some()
                ))
            }),
        ),
        Do("live import: Don't Save", Box::new(|a| a.global::<Studio>().invoke_unsaved_dont_save())),
        Check(
            "live import: the set replaced the project (tempo, tracks, warped clip with its markers and sample loaded, MIDI notes, Drum Rack pad with its sample loaded, a plugin that isn't installed as a placeholder)",
            Box::new(move |c, _| {
                let p = c.project();
                let names: Vec<&str> = p.tracks.iter().map(|t| t.name.as_str()).collect();
                let clip = p.tracks.first().and_then(|t| t.clips.first()).ok_or(format!("tracks {names:?}"))?;
                let markers: Vec<(u64, f64)> = clip.warp.markers.iter().map(|m| (m.source, m.beat)).collect();
                let notes = p.tracks.get(1).map_or(0, |t| t.midi_clips.iter().map(|m| m.notes.len()).sum::<usize>());
                let pad = p
                    .tracks
                    .get(1)
                    .and_then(|t| t.instrument_plugin.as_ref())
                    .filter(|d| d.plugin.id == "drum-rack")
                    .and_then(|d| d.samples.first())
                    .map(|s| (s.slot, s.name.clone(), c.asset_map.contains_key(&s.asset)));
                let errors = c.engine.plugin_load_errors();
                let fx: Vec<(&str, bool)> = p.tracks.first().map_or(Vec::new(), |t| {
                    t.devices.iter().map(|d| (d.plugin.name.as_str(), errors.iter().any(|e| e.0 == d.id))).collect()
                });
                let ok = p.name == "Imported Set"
                    && p.transport.bpm == 100.0
                    && names == ["Loop", "Keys"]
                    && p.tracks[1].kind == TrackKind::Instrument
                    && clip.warp.enabled
                    && markers == [(0, 0.0), (48_000, 2.0), (144_000, 4.0)]
                    && clip.start == 4 * PPQ
                    && c.asset_map.contains_key(&clip.asset)
                    && notes == 1
                    && pad == Some((0, "kick".into(), true))
                    && fx == [("Missing FX", true)]
                    && !c.unsaved_open
                    && c.project_dir.is_none();
                let _ = std::fs::remove_dir_all(dir());
                ok.then_some(()).ok_or(format!(
                    "{} {} {names:?} {markers:?} start {} loaded {} notes {notes} pad {pad:?} fx {fx:?}",
                    p.name,
                    p.transport.bpm,
                    clip.start,
                    c.asset_map.contains_key(&clip.asset)
                ))
            }),
        ),
    ]
}
