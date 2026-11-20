//! Custom device views (layout `View::Custom`): the Trance Gate's step ring and
//! envelope, and the Shaper's lane tabs and curve editor.
//!
//! Both are drawn on the shared custom-body canvas (PanelCanvas, painted by
//! DeviceCanvas in ui/device_panel.slint) with `interactive` set: Rust builds
//! paths and texts in canvas coordinates and the canvas sends every pointer
//! event back to [Controller::panel_editor], where hit testing, gestures and
//! menus live. Lengths are the 100%-zoom card
//! measurements (pt, card relative); [Z] scales them to the 85% UI.
//!
//! Gestures follow the generic panel: while a gesture runs its values go to
//! the engine live (se_builtin_set_param) and the release commits ONE undo
//! step (a batch when several parameters move); Escape or a cancelled pointer
//! restores the values from before the gesture. Single clicks (toggles, menu
//! picks) are single commands.

use crate::controller::Controller;
use crate::device_panel::{DIAL_Y, Z};
use crate::devices::{DevMenu, MenuPick};
use crate::{DeviceCard, PanelCanvas, PanelKnob, PanelPath, PanelText};
use dstudio_core::builtin::DeviceDef;
use dstudio_core::builtin::shaper::{self as sh, Point};
use dstudio_core::builtin::trance_gate as tg;
use dstudio_core::{Command, Device, DeviceId, PPQ};
use slint::{Color, ModelRc, SharedString, VecModel};
use std::collections::HashMap;
use std::fmt::Write;
use std::rc::Rc;

/// The field starts under the card header (px) and runs to the card bottom.
const FIELD_Y: f32 = 27.0;
const CARD_H: f32 = 265.0;
// PanelCanvas.cursor codes (shared with src/panel_graph.rs: 1 move, 2 ew-resize, 3 ns-resize).
const CURSOR_MOVE: i32 = 1;
const CURSOR_EW: i32 = 2;
const CURSOR_NS: i32 = 3;
const CURSOR_POINTER: i32 = 4;
const CURSOR_CROSSHAIR: i32 = 5;
const MINUS: char = '\u{2212}';

/// A pointer event on a field (ui/device_panel.slint FieldView).
#[derive(Debug, Clone, Copy, Default)]
pub struct CanvasEvent {
    pub kind: i32,
    pub x: f32,
    pub y: f32,
    pub wx: f32,
    pub wy: f32,
    pub shift: bool,
    pub alt: bool,
    pub wheel: f32,
}

impl CanvasEvent {
    /// Card-relative coordinates (pt at 100%).
    fn pt(&self) -> (f32, f32) {
        (self.x / Z, (self.y + tg_geo().origin_y) / Z)
    }
}

const DOWN: i32 = 0;
const MOVE: i32 = 1;
const UP: i32 = 2;
const CANCEL: i32 = 3;
const HOVER: i32 = 4;
const RIGHT: i32 = 5;
const DOUBLE: i32 = 6;
const WHEEL: i32 = 7;
const LEAVE: i32 = 8;

/// What the pointer is over.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
enum Hit {
    #[default]
    None,
    // Trance Gate
    Step(usize),
    StepCount,
    StepMore,
    StepFewer,
    Rate,
    Pattern(usize),
    EnvA,
    EnvD,
    EnvR,
    Hole,
    // Shaper
    LaneOn(usize),
    Lane(usize),
    Shape,
    Band,
    Split,
    FilterType,
    Resonance,
    Bits,
    Decimate,
    CurvePoint(usize),
    Mid(usize),
    Plot,
    Trigger,
    Threshold,
    FilterHigh,
    FilterLow,
}

#[derive(Debug, Clone)]
enum Gesture {
    /// Trance Gate ring: every step crossed gets the state of the first one.
    Paint { device: DeviceId, on: bool, last: usize },
    /// A value cell / counter / pill dragged like a knob (Controller::panel_drag);
    /// a release without movement opens `menu` when it has one.
    Param { device: DeviceId, menu: Option<i32> },
    /// Trance Gate envelope handle; the time axis is fixed at press.
    Env { device: DeviceId, handle: Hit, span: f32, gate_ms: f32 },
    /// Shaper point (or bend handle when `mid`).
    Curve { device: DeviceId, lane: usize, k: usize, mid: bool, pts: Vec<Point>, raw: (f32, f32), last: (f32, f32) },
}

/// Custom view state (lives in the panel state).
#[derive(Debug, Default)]
pub struct ViewState {
    hover: Option<(DeviceId, Hit, (f32, f32))>,
    gesture: Option<Gesture>,
    /// Values a running gesture has sent to the engine but not committed.
    live: HashMap<(DeviceId, usize), f32>,
    /// Shaper: selected lane and grid divisions per device (UI state).
    lane: HashMap<DeviceId, usize>,
    grid: HashMap<DeviceId, u32>,
    /// Trance Gate pattern clipboard (mask, steps) and Shaper shape clipboard,
    /// shared by every device of the kind.
    pattern_clip: Option<(u32, u32)>,
    shape_clip: Option<Vec<Point>>,
    /// Window position of the press that may open a menu on release.
    menu_at: (f32, f32),
}

// Menu action codes (MenuPick::View).
const TG_CLEAR: i32 = 10;
const TG_FILL: i32 = 11;
const TG_INVERT: i32 = 12;
const TG_ROT_LEFT: i32 = 13;
const TG_ROT_RIGHT: i32 = 14;
const TG_COPY: i32 = 15;
const TG_PASTE: i32 = 16;
const SH_FLIP: i32 = 1;
const SH_REVERSE: i32 = 2;
const SH_CLEAR: i32 = 3;
const SH_COPY: i32 = 4;
const SH_PASTE: i32 = 5;
const SH_SHAPE: i32 = 100; // + library index
const SH_GRID: i32 = 200; // + divisions
const SH_TRIGGER_SYNC: i32 = 300;
const SH_TRIGGER_SELF: i32 = 301;
const SH_TRIGGER_TRACK: i32 = 1000; // + project track index

// ---------------------------------------------------------------- drawing

fn rgba(r: u8, g: u8, b: u8, a: f32) -> Color {
    Color::from_argb_u8((a.clamp(0.0, 1.0) * 255.0).round() as u8, r, g, b)
}
fn ink(a: f32) -> Color {
    rgba(255, 255, 255, a)
}
fn alpha(c: Color, a: f32) -> Color {
    Color::from_argb_u8((a.clamp(0.0, 1.0) * 255.0).round() as u8, c.red(), c.green(), c.blue())
}
fn none() -> Color {
    Color::from_argb_u8(0, 0, 0, 0)
}
fn vgrad(top: Color, bottom: Color) -> Color {
    let a = 0.5 * (top.alpha() as f32 + bottom.alpha() as f32) / 255.0;
    alpha(top, a)
}

/// Field x / y (px) of a card-relative coordinate (pt at 100%).
fn fx(x: f32) -> f32 {
    x * Z
}
fn fy(y: f32) -> f32 {
    y * Z - tg_geo().origin_y
}

/// SVG path data in field px from card coordinates (pt at 100%).
#[derive(Default)]
struct PathData(String);

impl PathData {
    fn m(mut self, x: f32, y: f32) -> Self {
        let _ = write!(self.0, "M{:.2} {:.2} ", fx(x), fy(y));
        self
    }
    fn l(mut self, x: f32, y: f32) -> Self {
        let _ = write!(self.0, "L{:.2} {:.2} ", fx(x), fy(y));
        self
    }
    /// Arc to (x, y) with radius r (card units); sweep = clockwise on screen.
    fn a(mut self, r: f32, large: bool, sweep: bool, x: f32, y: f32) -> Self {
        let _ = write!(self.0, "A{:.2} {:.2} 0 {} {} {:.2} {:.2} ", r * Z, r * Z, large as u8, sweep as u8, fx(x), fy(y));
        self
    }
    fn z(mut self) -> Self {
        self.0.push_str("Z ");
        self
    }
    fn poly(mut self, pts: &[(f32, f32)]) -> Self {
        for (i, (x, y)) in pts.iter().enumerate() {
            self = if i == 0 { self.m(*x, *y) } else { self.l(*x, *y) };
        }
        self
    }
    fn rrect(self, x: f32, y: f32, w: f32, h: f32, r: f32) -> Self {
        let r = r.min(w / 2.0).min(h / 2.0);
        self.m(x + r, y)
            .l(x + w - r, y)
            .a(r, false, true, x + w, y + r)
            .l(x + w, y + h - r)
            .a(r, false, true, x + w - r, y + h)
            .l(x + r, y + h)
            .a(r, false, true, x, y + h - r)
            .l(x, y + r)
            .a(r, false, true, x + r, y)
            .z()
    }
    fn circle(self, cx: f32, cy: f32, r: f32) -> Self {
        self.m(cx - r, cy).a(r, true, true, cx + r, cy).a(r, true, true, cx - r, cy).z()
    }
    fn empty(&self) -> bool {
        self.0.is_empty()
    }
}

#[derive(Default)]
struct Canvas {
    paths: Vec<PanelPath>,
    texts: Vec<PanelText>,
}

impl Canvas {
    fn path(&mut self, d: PathData, stroke: Color, width: f32, fill: Color) {
        if !d.empty() {
            self.paths.push(PanelPath { commands: d.0.into(), stroke, width: width * Z, fill, fill2: fill });
        }
    }
    fn fill(&mut self, d: PathData, c: Color) {
        self.path(d, none(), 0.0, c);
    }
    fn stroke(&mut self, d: PathData, c: Color, w: f32) {
        self.path(d, c, w, none());
    }
    /// Text in a box (card units); align 0 left, 1 centre, 2 right.
    #[allow(clippy::too_many_arguments)]
    fn text(&mut self, x: f32, y: f32, w: f32, h: f32, text: impl Into<SharedString>, color: Color, size: f32, weight: i32, align: i32) {
        self.texts.push(PanelText { x: fx(x), y: fy(y), w: w * Z, h: h * Z, text: text.into(), color, size: size * Z, weight, align });
    }
    /// A rounded value pill: fill, optional label text, value text, chevron.
    fn pill(&mut self, x: f32, y: f32, w: f32, h: f32, hot: bool) {
        self.fill(
            PathData::default().rrect(x, y, w, h, h / 2.0),
            if hot { rgba(0x33, 0x34, 0x3a, 1.0) } else { rgba(0x2a, 0x2b, 0x30, 1.0) },
        );
    }
    fn chevron(&mut self, cx: f32, cy: f32, down: bool, c: Color) {
        let s = if down { 1.0 } else { -1.0 };
        self.stroke(PathData::default().m(cx - 3.5, cy - 1.75 * s).l(cx, cy + 1.75 * s).l(cx + 3.5, cy - 1.75 * s), c, 1.4);
    }
    fn into_canvas(self, w: f32, cursor: i32) -> PanelCanvas {
        PanelCanvas {
            visible: true,
            x: 0.0,
            y: FIELD_Y,
            w: w * Z,
            h: CARD_H - FIELD_Y,
            paths: ModelRc::from(Rc::new(VecModel::from(self.paths))),
            texts: ModelRc::from(Rc::new(VecModel::from(self.texts))),
            interactive: true,
            cursor,
            ..Default::default()
        }
    }
}

fn in_rect(p: (f32, f32), x: f32, y: f32, w: f32, h: f32) -> bool {
    p.0 >= x && p.0 <= x + w && p.1 >= y && p.1 <= y + h
}

fn knob(def: &DeviceDef, value: f32, index: usize, centre_x: f32) -> PanelKnob {
    let p = &def.params[index];
    PanelKnob {
        param: index as i32,
        x: (centre_x - 31.0) * Z,
        y: DIAL_Y * Z,
        label: p.label.into(),
        value: p.format(value).into(),
        norm: p.to_normalized(value),
        origin: p.origin(),
        ..Default::default()
    }
}

fn fmt_num(v: f32) -> String {
    let s = if (v - v.round()).abs() < 0.05 { format!("{:.0}", v.round()) } else { format!("{v:.1}") };
    s.replace('-', &MINUS.to_string())
}

// ---------------------------------------------------------------- trance gate model

/// Envelope settings in ms (sustain 0..1).
#[derive(Debug, Clone, Copy)]
struct Adsr {
    a: f32,
    d: f32,
    s: f32,
    r: f32,
}

/// Linear envelope (the engine's model): attack to 1, decay to sustain, release to 0.
#[derive(Debug, Clone, Copy, Default)]
struct EnvSim {
    level: f32,
    stage: u8, // 0 idle, 1 attack, 2 decay, 3 sustain, 4 release
    rel_rate: f32,
}

impl EnvSim {
    fn gate(&mut self, on: bool, e: &Adsr) {
        if on {
            self.stage = 1;
        } else if self.stage != 0 && self.stage != 4 {
            self.stage = 4;
            self.rel_rate = self.level / e.r.max(0.5);
        }
    }
    /// Advance by `ms` exactly: every stage is linear, so each one is consumed
    /// in one step (cost independent of the time span).
    fn advance(&mut self, mut ms: f32, e: &Adsr) {
        while ms > 0.0 {
            let (rate, end, next) = match self.stage {
                1 => (1.0 / e.a.max(0.5), 1.0, 2),
                2 => (-(1.0 - e.s) / e.d.max(1.0), e.s, 3),
                4 => (-self.rel_rate, 0.0, 0),
                3 => {
                    self.level = e.s;
                    return;
                }
                _ => {
                    self.level = 0.0;
                    return;
                }
            };
            let need = if rate == 0.0 { 0.0 } else { (end - self.level) / rate };
            if need > ms {
                self.level += rate * ms;
                return;
            }
            self.level = end;
            self.stage = next;
            ms -= need.max(0.0);
        }
    }
}

/// Step index at time t (in steps) with swing (0..1 of half a step on odd steps).
fn swung_step(u: f64, swing: f64) -> i64 {
    let k = u.floor();
    let ki = k as i64;
    if ki & 1 == 1 && u - k < swing * 0.5 { ki - 1 } else { ki }
}

/// Start of step i (in steps) with swing.
fn step_start(i: usize, swing: f32) -> f32 {
    i as f32 + if i % 2 == 1 { swing * 0.5 } else { 0.0 }
}

/// Gate level over one pattern cycle (480 samples), simulated three times
/// around so the first cycle's state is warmed up. Exact between step
/// boundaries: O(samples + steps) whatever the tempo and rate.
fn ring_levels(e: &Adsr, steps: u32, mask: u32, swing: f32, step_ms: f32) -> Vec<f32> {
    const N: usize = 480;
    let steps = steps.max(1) as usize;
    let total = steps as f32 * step_ms;
    let gates: Vec<(f32, bool)> = (0..steps).map(|i| (step_start(i, swing) * step_ms, mask >> i & 1 == 1)).collect();
    let mut sim = EnvSim::default();
    let mut out = vec![0.0; N];
    for pass in 0..3 {
        let (mut t, mut next) = (0.0f32, 0usize);
        let run_to = |sim: &mut EnvSim, until: f32, t: &mut f32, next: &mut usize| {
            while *next < gates.len() && gates[*next].0 <= until {
                sim.advance(gates[*next].0 - *t, e);
                *t = gates[*next].0;
                sim.gate(gates[*next].1, e);
                *next += 1;
            }
            sim.advance(until - *t, e);
            *t = until;
        };
        for (j, slot) in out.iter_mut().enumerate() {
            run_to(&mut sim, total * (j + 1) as f32 / N as f32, &mut t, &mut next);
            if pass == 2 {
                *slot = sim.level;
            }
        }
    }
    out
}

/// Time axis of the envelope view: (gate-open ms, span ms).
fn env_span(e: &Adsr, step_ms: f32) -> (f32, f32) {
    let gate = (2.0 * step_ms).max(e.a + e.d + step_ms / 2.0);
    let raw = 1.12 * (gate + e.r);
    let grid = if raw < 1000.0 { 50.0 } else { 100.0 };
    (gate, ((raw / grid).ceil() * grid).max(300.0))
}

// Trance Gate geometry (pt at 100%): the card's, card relative, and the expanded
// window's, relative to the top left of the window's canvas.
const TG_W: f32 = 528.0;
const TG_KNOB_X: f32 = 229.0;

#[derive(Debug, Clone, Copy)]
pub(crate) struct TgGeo {
    /// Ring centre and outer radius; the inner radius is 0.6 of it.
    cx: f32,
    cy: f32,
    r: f32,
    /// Scale of the ring's centre (step counter, chevrons, rate) and of small text.
    s: f32,
    ts: f32,
    /// Pattern track, and its caption (x, y, h).
    pat: (f32, f32, f32, f32),
    pat_label: (f32, f32, f32),
    /// Envelope well and its fill, its caption (x, y), and the plot: x0, y0 (level 1), x1, y1 (level 0).
    env: (f32, f32, f32, f32),
    env_fill: bool,
    env_label: (f32, f32),
    plot: (f32, f32, f32, f32),
    /// Field y (px) of y = 0: the card's canvas starts below its header.
    origin_y: f32,
}

const TG_CARD: TgGeo = TgGeo {
    cx: 98.0,
    cy: 157.5,
    r: 83.0,
    s: 1.0,
    ts: 1.0,
    pat: (8.0, 285.0, 180.0, 18.0),
    pat_label: (8.0, 266.0, 19.0),
    env: (200.0, 85.0, 320.0, 104.0),
    env_fill: true,
    env_label: (200.0, 66.0),
    plot: (210.0, 95.0, 510.0, 173.0),
    origin_y: FIELD_Y,
};

/// The window: the ring and pattern track on the left panel, the envelope across the display.
pub(crate) const TG_WIN: TgGeo = TgGeo {
    cx: 137.0,
    cy: 128.0,
    r: 127.0,
    s: 1.53,
    ts: 1.1,
    pat: (60.0, 278.0, 204.0, 26.0),
    pat_label: (10.0, 278.0, 26.0),
    env: (288.0, 0.0, 880.0, 516.0),
    env_fill: false,
    env_label: (303.0, 3.0),
    plot: (303.0, 25.0, 1152.0, 488.0),
    origin_y: 0.0,
};

#[cfg(test)]
impl TgGeo {
    pub(crate) fn ring_cx(&self) -> f32 {
        self.cx
    }
    pub(crate) fn ring_cy(&self) -> f32 {
        self.cy
    }
    pub(crate) fn ring_r(&self) -> f32 {
        self.r
    }
}

thread_local! {
    static TG_GEO: std::cell::Cell<TgGeo> = const { std::cell::Cell::new(TG_CARD) };
}

/// The Trance Gate geometry being drawn or hit tested.
fn tg_geo() -> TgGeo {
    TG_GEO.with(|g| g.get())
}

/// Draws and hit tests with `g` until dropped.
pub(crate) struct TgScope(TgGeo);

impl TgScope {
    pub(crate) fn new(g: TgGeo) -> Self {
        Self(TG_GEO.with(|c| c.replace(g)))
    }
}

impl Drop for TgScope {
    fn drop(&mut self) {
        TG_GEO.with(|c| c.set(self.0));
    }
}

fn tg_cursor(hit: Hit) -> i32 {
    match hit {
        Hit::Step(_) | Hit::Pattern(_) | Hit::StepMore | Hit::StepFewer | Hit::Rate => CURSOR_POINTER,
        Hit::StepCount => CURSOR_NS,
        Hit::EnvA | Hit::EnvR => CURSOR_EW,
        Hit::EnvD => CURSOR_MOVE,
        _ => 0,
    }
}

fn step_angle(i: f32, steps: u32) -> f32 {
    std::f32::consts::TAU * i / steps.max(1) as f32
}
fn polar(r: f32, a: f32) -> (f32, f32) {
    let g = tg_geo();
    (g.cx + r * a.sin(), g.cy - r * a.cos())
}
fn sector(r0: f32, r1: f32, a0: f32, a1: f32) -> PathData {
    let (o0, o1, i1, i0) = (polar(r1, a0), polar(r1, a1), polar(r0, a1), polar(r0, a0));
    let large = a1 - a0 > std::f32::consts::PI;
    PathData::default().m(o0.0, o0.1).a(r1, large, true, o1.0, o1.1).l(i1.0, i1.1).a(r0, large, false, i0.0, i0.1).z()
}

// ---------------------------------------------------------------- shaper model

// Shaper geometry (pt at 100%, card relative).
const SH_W: f32 = 501.0;
const SH_TRIGGER_SPLIT: f32 = 84.5; // divider from the pill's left
const SH_KNOBS: [f32; 4] = [39.0, 111.0, 371.5, 461.5];

/// Where the Shaper's editor draws: the card's, or its window's (relative to the top left of
/// the window's canvas, which starts at the lane tabs above the display).
#[derive(Debug, Clone, Copy)]
pub(crate) struct ShGeo {
    tabs_y: f32,
    tab_x: [f32; 7],
    /// Strip row: pill top, 19 tall.
    strip_y: f32,
    /// Curve well and its plot: x0, y top (1), x1, y bottom (0).
    well: (f32, f32, f32, f32),
    plot: (f32, f32, f32, f32),
    /// Trigger pill; its caption under it (the card) or to its left (the window).
    trigger: (f32, f32, f32, f32),
    trigger_caption_under: bool,
}

const SH_CARD: ShGeo = ShGeo {
    tabs_y: 50.0,
    tab_x: [16.0, 95.5, 161.0, 232.5, 291.5, 357.0, 428.0],
    strip_y: 71.5,
    well: (8.0, 93.0, 485.0, 120.5),
    plot: (48.0, 108.0, 483.0, 205.0),
    trigger: (171.0, 245.0, 141.0, 19.0),
    trigger_caption_under: true,
};

/// The window: the tabs over the display, the strip and Trigger on the display's caption row,
/// the curve across the rest of the display.
pub(crate) const SH_WIN: ShGeo = ShGeo {
    tabs_y: 20.0,
    tab_x: [8.0, 96.0, 169.0, 248.0, 315.0, 388.0, 467.0],
    strip_y: 58.0,
    well: (0.0, 48.0, 1048.0, 466.0),
    plot: (60.0, 92.0, 1032.0, 500.0),
    trigger: (891.0, 58.0, 141.0, 19.0),
    trigger_caption_under: false,
};

#[cfg(test)]
impl ShGeo {
    /// A point on lane `l`'s name in the tab row.
    pub(crate) fn tab_name(&self, l: usize) -> (f32, f32) {
        (self.tab_x[l] + 20.0, self.tabs_y)
    }
}

thread_local! {
    static SH_GEO: std::cell::Cell<ShGeo> = const { std::cell::Cell::new(SH_CARD) };
}

fn sh_geo() -> ShGeo {
    SH_GEO.with(|g| g.get())
}

/// Draws and hit tests the Shaper with `g` (and the window's origin) until dropped.
pub(crate) struct ShScope(ShGeo, Option<TgScope>);

impl ShScope {
    pub(crate) fn new(g: ShGeo) -> Self {
        let origin = TgScope::new(TgGeo { origin_y: 0.0, ..tg_geo() });
        Self(SH_GEO.with(|c| c.replace(g)), Some(origin))
    }
}

impl Drop for ShScope {
    fn drop(&mut self) {
        SH_GEO.with(|c| c.set(self.0));
        self.1.take();
    }
}

fn sh_px(x: f32) -> f32 {
    let p = sh_geo().plot;
    p.0 + x * (p.2 - p.0)
}
fn sh_py(y: f32) -> f32 {
    let p = sh_geo().plot;
    p.3 - y * (p.3 - p.1)
}
fn sh_x_of(px: f32) -> f32 {
    let p = sh_geo().plot;
    ((px - p.0) / (p.2 - p.0)).clamp(0.0, 1.0)
}
fn sh_y_of(py: f32) -> f32 {
    let p = sh_geo().plot;
    ((p.3 - py) / (p.3 - p.1)).clamp(0.0, 1.0)
}

/// The Crush lane's strip controls from the row start x (pt at 100%):
/// label and (x, width) of the Bits and Decimate pills. Compact once the band
/// split pill takes its room, so the row ends inside the curve well (x 493).
struct CrushRow {
    bits_label: f32,
    bits_pill: (f32, f32),
    dec_label: f32,
    dec_pill: (f32, f32),
    font: f32,
}

fn crush_row(x: f32, compact: bool) -> CrushRow {
    if compact {
        CrushRow { bits_label: x, bits_pill: (x + 24.0, 46.0), dec_label: x + 76.0, dec_pill: (x + 124.0, 42.0), font: 11.0 }
    } else {
        CrushRow { bits_label: x, bits_pill: (x + 27.0, 62.0), dec_label: x + 101.0, dec_pill: (x + 157.0, 50.0), font: 11.5 }
    }
}

/// Lane value text at curve height y.
/// `max_beats`: the Time lane's longest repeat at the project tempo
/// ([sh::TIME_MAX_SECONDS]); a deeper point reads as that limit.
#[allow(clippy::too_many_arguments)]
fn lane_text(lane: usize, y: f32, depth: f32, quarters: f64, max_beats: f64, low: f32, high: f32) -> String {
    match lane {
        0 => format!("{}%", (100.0 * (1.0 - depth * (1.0 - y))).round()),
        1 => {
            let want = (y * depth) as f64 * quarters;
            let b = (want.min(max_beats) * 100.0).round() / 100.0;
            let text = if b == 0.0 {
                "Now".to_string()
            } else if (b - 1.0).abs() < 1e-9 {
                "1 beat".into()
            } else {
                format!("{b} beats")
            };
            if want > max_beats + 1e-9 { format!("{text} (max)") } else { text }
        }
        2 => format!("{}%", (100.0 * (1.0 + depth * (2.0 * y - 1.0))).round()),
        3 => {
            let v = depth * (2.0 * y - 1.0);
            let n = (100.0 * v.abs()).round();
            if n == 0.0 {
                "C".into()
            } else if v < 0.0 {
                format!("{n}L")
            } else {
                format!("{n}R")
            }
        }
        4 => {
            let f = low * (high / low).powf(y);
            if f >= 1000.0 { format!("{:.1} kHz", f / 1000.0) } else { format!("{f:.0} Hz") }
        }
        5 => format!("{}%", (100.0 * depth * y).round()),
        _ => {
            let st = depth * (y - 0.5) * 24.0;
            if st.abs() < 0.05 { "0 st".into() } else { format!("{}{} st", if st > 0.0 { "+" } else { "" }, fmt_num(st)) }
        }
    }
}

/// Short frequency text for the filter range pills ("12k", "150").
fn short_hz(f: f32) -> String {
    if f >= 1000.0 {
        let k = f / 1000.0;
        if k >= 10.0 || (k - k.round()).abs() < 0.05 { format!("{:.0}k", k) } else { format!("{k:.1}k") }
    } else {
        format!("{f:.0}")
    }
}

// ---------------------------------------------------------------- controller

impl Controller {
    /// Value of parameter `i` as shown: a running gesture's live value, else the panel value.
    fn fv(&self, d: &Device, def: &DeviceDef, i: usize) -> f32 {
        if let Some(v) = self.devices.panel.editors.live.get(&(d.id, i)) {
            return *v;
        }
        def.params.get(i).map_or(0.0, |p| self.panel_value(d, def, p))
    }

    fn values(&self, d: &Device, def: &DeviceDef) -> Vec<f32> {
        (0..def.params.len()).map(|i| self.fv(d, def, i)).collect()
    }

    /// Fill the card of a device whose layout has a custom view.
    pub(crate) fn custom_card(&self, name: &str, d: &Device, def: &'static DeviceDef, card: &mut DeviceCard) {
        match name {
            "trance-gate" => self.trance_gate_card(d, def, card),
            "shaper" => self.shaper_card(d, def, card),
            _ => {}
        }
    }

    fn hover_of(&self, id: DeviceId) -> (Hit, (f32, f32)) {
        match self.devices.panel.editors.hover {
            Some((d, h, p)) if d == id => (h, p),
            _ => (Hit::None, (-1.0, -1.0)),
        }
    }

    fn step_ms(&self, rate: usize) -> f32 {
        (60000.0 / self.bpm_now().max(1.0) * tg::RATE_QUARTERS[rate.min(tg::RATES.len() - 1)]) as f32
    }

    fn adsr(&self, d: &Device, def: &DeviceDef) -> Adsr {
        Adsr { a: self.fv(d, def, 2), d: self.fv(d, def, 3), s: self.fv(d, def, 4) / 100.0, r: self.fv(d, def, 5) }
    }

    /// (pattern index, steps, mask) of the active pattern.
    fn tg_pattern(&self, d: &Device, def: &DeviceDef) -> (usize, u32, u32) {
        let p = (self.fv(d, def, 0).round() as usize).min(tg::PATTERN_COUNT - 1);
        let base = 8 + 3 * p;
        let steps = (self.fv(d, def, base).round() as u32).clamp(1, tg::MAX_STEPS);
        (p, steps, tg::mask(self.fv(d, def, base + 1), self.fv(d, def, base + 2)))
    }

    fn trance_gate_card(&self, d: &Device, def: &'static DeviceDef, card: &mut DeviceCard) {
        let (hit, _) = self.hover_of(d.id);
        let mut c = Canvas::default();
        self.tg_draw(d, def, card.accent, &mut c);
        // Knobs: Attack, Decay, Sustain, Release, Swing, Mix (cells 50 wide, gap 4).
        let knobs: Vec<PanelKnob> = (2..8).map(|i| knob(def, self.fv(d, def, i), i, TG_KNOB_X + 54.0 * (i - 2) as f32)).collect();
        card.knobs = ModelRc::from(Rc::new(VecModel::from(knobs)));
        card.canvas = c.into_canvas(TG_W, tg_cursor(hit));
    }

    /// The Trance Gate's expanded window picture (src/device_window.rs places it): the ring
    /// and pattern track on the left panel, the envelope across the display. `w` x `h` pt.
    pub(crate) fn trance_gate_window_canvas(&self, d: &Device, def: &'static DeviceDef, accent: Color, w: f32, h: f32) -> PanelCanvas {
        let _g = TgScope::new(TG_WIN);
        let (hit, _) = self.hover_of(d.id);
        let mut c = Canvas::default();
        self.tg_draw(d, def, accent, &mut c);
        PanelCanvas {
            visible: true,
            w: w * Z,
            h: h * Z,
            paths: ModelRc::from(Rc::new(VecModel::from(c.paths))),
            texts: ModelRc::from(Rc::new(VecModel::from(c.texts))),
            interactive: true,
            cursor: tg_cursor(hit),
            ..Default::default()
        }
    }

    /// The ring, pattern track and envelope in the current geometry ([`TgScope`]).
    fn tg_draw(&self, d: &Device, def: &'static DeviceDef, accent: Color, c: &mut Canvas) {
        let g = tg_geo();
        let (s, ri) = (g.s, 0.6 * g.r);
        let (hit, _) = self.hover_of(d.id);
        let (pattern, steps, mask) = self.tg_pattern(d, def);
        let rate = (self.fv(d, def, 1).round() as usize).min(tg::RATES.len() - 1);
        let swing = self.fv(d, def, 6) / 100.0;
        let env = self.adsr(d, def);
        let step_ms = self.step_ms(rate);

        // Ring: one annular sector per step (swing-warped), then the gate-shape preview.
        let gap = 0.6 / g.r;
        for i in 0..steps as usize {
            let a0 = step_angle(step_start(i, swing), steps) + gap;
            let a1 = step_angle(if i + 1 == steps as usize { steps as f32 } else { step_start(i + 1, swing) }, steps) - gap;
            if a1 <= a0 {
                continue;
            }
            let on = mask >> i & 1 == 1;
            let hot = hit == Hit::Step(i);
            let fill = if on { alpha(accent, if hot { 0.16 } else { 0.08 }) } else { ink(if hot { 0.10 } else { 0.045 }) };
            c.fill(sector(ri, g.r, a0, a1), fill);
        }
        let levels = ring_levels(&env, steps, mask, swing, step_ms);
        let n = levels.len();
        for i in (0..steps as usize).filter(|i| mask >> i & 1 == 1) {
            let a0 = step_angle(step_start(i, swing), steps) + gap;
            let a1 = step_angle(if i + 1 == steps as usize { steps as f32 } else { step_start(i + 1, swing) }, steps) - gap;
            if a1 <= a0 {
                continue;
            }
            let j0 = ((a0 / std::f32::consts::TAU) * n as f32).ceil() as usize;
            let j1 = (((a1 / std::f32::consts::TAU) * n as f32).floor() as usize).min(n - 1);
            let mut outer: Vec<(f32, f32)> = Vec::new();
            let r_of = |lv: f32| ri + (g.r - ri) * lv;
            outer.push(polar(r_of(levels[j0.min(n - 1)]), a0));
            for (j, lv) in levels.iter().enumerate().take(j1 + 1).skip(j0) {
                outer.push(polar(r_of(*lv), std::f32::consts::TAU * j as f32 / n as f32));
            }
            outer.push(polar(r_of(levels[j1]), a1));
            if levels[j0.min(n - 1)..=j1].iter().all(|l| *l < 0.004) {
                continue;
            }
            let (i1, i0) = (polar(ri, a1), polar(ri, a0));
            let shape = PathData::default().poly(&outer).l(i1.0, i1.1).a(ri, a1 - a0 > std::f32::consts::PI, false, i0.0, i0.1).z();
            c.fill(shape, alpha(accent, 0.34));
            c.stroke(PathData::default().poly(&outer), accent, 1.5);
        }
        c.stroke(PathData::default().circle(g.cx, g.cy, ri - 4.2), ink(0.05), 1.0);
        // Playhead while the transport plays.
        if self.engine.transport().playing {
            let beats = self.playhead_ticks() as f64 / PPQ as f64;
            let pos = (beats / tg::RATE_QUARTERS[rate]).rem_euclid(steps as f64) as f32;
            if pos > 0.01 {
                let a1 = step_angle(pos, steps);
                let r = ri - 4.2;
                let (sx, sy) = polar(r, 0.0);
                let (ex, ey) = polar(r, a1);
                c.stroke(PathData::default().m(sx, sy).a(r, a1 > std::f32::consts::PI, true, ex, ey), alpha(accent, 0.7), 2.0);
            }
        }
        // Centre: step counter with chevrons, rate pill.
        let count_hot = matches!(hit, Hit::StepCount);
        c.text(
            g.cx - 32.0 * s,
            g.cy - 26.0 * s,
            48.0 * s,
            28.0 * s,
            steps.to_string(),
            ink(if count_hot { 1.0 } else { 0.92 }),
            28.0 * s,
            600,
            1,
        );
        for (k, h) in [(0, Hit::StepMore), (1, Hit::StepFewer)] {
            let enabled = if k == 0 { steps < tg::MAX_STEPS } else { steps > 1 };
            let cy = g.cy + (-18.0 + 14.0 * k as f32) * s;
            c.chevron(
                g.cx + 19.0 * s,
                cy,
                k == 1,
                ink(if !enabled {
                    0.25
                } else if hit == h {
                    1.0
                } else {
                    0.7
                }),
            );
        }
        let rate_hot = hit == Hit::Rate;
        c.text(
            g.cx - 26.0 * s,
            g.cy + 5.0 * s,
            44.0 * s,
            18.0 * s,
            tg::RATES[rate],
            if rate_hot { Color::from_rgb_u8(255, 255, 255) } else { accent.brighter(0.2) },
            13.0 * g.ts,
            600,
            2,
        );
        c.chevron(g.cx + 23.0 * s, g.cy + 14.0 * s, true, rgba(0xa6, 0xa8, 0xad, 1.0));

        // Pattern choice track.
        c.text(g.pat_label.0, g.pat_label.1, 80.0, g.pat_label.2, "Pattern", ink(0.4), 12.0, 600, 0);
        c.fill(PathData::default().rrect(g.pat.0, g.pat.1, g.pat.2, g.pat.3, g.pat.3 / 2.0), rgba(0x29, 0x2a, 0x2f, 1.0));
        let cell = g.pat.2 / tg::PATTERN_COUNT as f32;
        for i in 0..tg::PATTERN_COUNT {
            let x = g.pat.0 + cell * i as f32;
            let sel = i == pattern;
            if sel {
                c.fill(PathData::default().rrect(x + 2.0, g.pat.1 + 2.0, cell - 4.0, g.pat.3 - 4.0, (g.pat.3 - 4.0) / 2.0), accent);
            } else if hit == Hit::Pattern(i) {
                c.fill(PathData::default().rrect(x + 2.0, g.pat.1 + 2.0, cell - 4.0, g.pat.3 - 4.0, (g.pat.3 - 4.0) / 2.0), ink(0.04));
            }
            let col = if sel {
                rgba(0x17, 0x18, 0x1b, 1.0)
            } else if hit == Hit::Pattern(i) {
                ink(0.85)
            } else {
                ink(0.7)
            };
            c.text(x, g.pat.1, cell, g.pat.3, (i + 1).to_string(), col, 11.0 * g.ts, 600, 1);
        }

        // Envelope.
        c.text(g.env_label.0, g.env_label.1, 120.0, 19.0, "Envelope", ink(0.4), 12.0, 600, 0);
        if g.env_fill {
            c.fill(PathData::default().rrect(g.env.0, g.env.1, g.env.2, g.env.3, 6.0), rgba(0, 0, 0, 0.26));
        }
        let (gate_ms, span) = match &self.devices.panel.editors.gesture {
            Some(Gesture::Env { device, span, gate_ms, .. }) if *device == d.id => (*gate_ms, *span),
            _ => env_span(&env, step_ms),
        };
        let (x0, y0, x1, y1) = g.plot;
        let tx = |t: f32| x0 + (t / span).clamp(0.0, 1.0) * (x1 - x0);
        let ly = |l: f32| y1 - l.clamp(0.0, 1.0) * (y1 - y0);
        let mut grid = PathData::default();
        let mut t = step_ms;
        while t < span && step_ms > 1.0 {
            grid = grid.m(tx(t), y0).l(tx(t), y1);
            t += step_ms;
        }
        c.stroke(grid, ink(0.05), 1.0);
        c.stroke(PathData::default().m(x0, y1).l(x1, y1), ink(0.08), 1.0);
        c.fill(PathData::default().m(x0, y0).l(tx(gate_ms), y0).l(tx(gate_ms), y1).l(x0, y1).z(), alpha(accent, 0.07));
        let mut dash = PathData::default();
        let mut y = y0;
        while y < y1 {
            dash = dash.m(tx(gate_ms), y).l(tx(gate_ms), (y + 3.0).min(y1));
            y += 6.0;
        }
        c.stroke(dash, alpha(accent, 0.45), 1.0);
        // Gate level: open until gate_ms, then released.
        let mut sim = EnvSim::default();
        sim.gate(true, &env);
        let samples = 240;
        let mut pts = vec![(x0, ly(0.0))];
        let mut gated = true;
        let mut tt = 0.0f32;
        for k in 1..=samples {
            let target = span * k as f32 / samples as f32;
            if gated && target >= gate_ms {
                sim.advance(gate_ms - tt, &env);
                tt = gate_ms;
                sim.gate(false, &env);
                gated = false;
            }
            sim.advance(target - tt, &env);
            tt = target;
            pts.push((tx(target), ly(sim.level)));
        }
        let mut area = PathData::default().poly(&pts);
        area = area.l(x1, y1).l(x0, y1).z();
        c.fill(area, vgrad(alpha(accent, 0.32), alpha(accent, 0.04)));
        c.stroke(PathData::default().poly(&pts), accent, 1.5);
        let handles = [(Hit::EnvA, env.a, 1.0), (Hit::EnvD, env.a + env.d, env.s), (Hit::EnvR, gate_ms + env.r, 0.0)];
        for (hh, t, l) in handles {
            let hot = hit == hh
                || matches!(&self.devices.panel.editors.gesture, Some(Gesture::Env { handle, device, .. }) if *handle == hh && *device == d.id);
            let r = if hot { 6.0 } else { 4.5 };
            c.path(
                PathData::default().circle(tx(t), ly(l), r),
                if hot { accent } else { ink(0.9) },
                1.5,
                if hot { accent.brighter(0.6) } else { rgba(0x17, 0x18, 0x1b, 1.0) },
            );
        }
        c.text(x0, y1 + 1.0, 80.0, 15.0, "Gate", ink(0.55), 10.5, 600, 0);
        c.text(x1 - 100.0, y1 + 1.0, 100.0, 15.0, format!("{} ms", span.round()), ink(0.55), 10.5, 600, 2);
    }

    fn tg_hit(&self, d: &Device, def: &DeviceDef, p: (f32, f32)) -> Hit {
        let (_, steps, _) = self.tg_pattern(d, def);
        let swing = self.fv(d, def, 6) / 100.0;
        let g = tg_geo();
        let (s, ri) = (g.s, 0.6 * g.r);
        let (dx, dy) = (p.0 - g.cx, p.1 - g.cy);
        let dist = (dx * dx + dy * dy).sqrt();
        if in_rect(p, g.cx + 11.0 * s, g.cy - 25.0 * s, 16.0 * s, 13.0 * s) {
            return Hit::StepMore;
        }
        if in_rect(p, g.cx + 11.0 * s, g.cy - 11.0 * s, 16.0 * s, 13.0 * s) {
            return Hit::StepFewer;
        }
        if in_rect(p, g.cx - 32.0 * s, g.cy - 26.0 * s, 43.0 * s, 28.0 * s) {
            return Hit::StepCount;
        }
        if in_rect(p, g.cx - 26.0 * s, g.cy + 5.0 * s, 54.0 * s, 18.0 * s) {
            return Hit::Rate;
        }
        if (ri - 4.0..=g.r + 6.0).contains(&dist) {
            let a = dx.atan2(-dy).rem_euclid(std::f32::consts::TAU);
            let u = a / std::f32::consts::TAU * steps as f32;
            let k = swung_step(u as f64, swing as f64).rem_euclid(steps as i64) as usize;
            return Hit::Step(k);
        }
        if dist < ri - 4.0 {
            return Hit::Hole;
        }
        if in_rect(p, g.pat.0, g.pat.1, g.pat.2, g.pat.3) {
            return Hit::Pattern((((p.0 - g.pat.0) / (g.pat.2 / 8.0)) as usize).min(7));
        }
        if in_rect(p, g.env.0, g.env.1, g.env.2, g.env.3) {
            let env = self.adsr(d, def);
            let rate = (self.fv(d, def, 1).round() as usize).min(tg::RATES.len() - 1);
            let (gate, span) = env_span(&env, self.step_ms(rate));
            let (x0, y0, x1, y1) = g.plot;
            let tx = |t: f32| x0 + (t / span).clamp(0.0, 1.0) * (x1 - x0);
            let ly = |l: f32| y1 - l * (y1 - y0);
            let mut best = (Hit::None, 12.0f32);
            for (h, t, l) in [(Hit::EnvA, env.a, 1.0), (Hit::EnvD, env.a + env.d, env.s), (Hit::EnvR, gate + env.r, 0.0)] {
                let dd = ((p.0 - tx(t)).powi(2) + (p.1 - ly(l)).powi(2)).sqrt();
                if dd <= best.1 {
                    best = (h, dd);
                }
            }
            return best.0;
        }
        Hit::None
    }

    /// The Time lane's longest repeat in quarter notes at the project tempo.
    fn time_max_beats(&self) -> f64 {
        sh::TIME_MAX_SECONDS * self.bpm_now() / 60.0
    }

    pub(crate) fn shaper_lane(&self, id: DeviceId) -> usize {
        self.devices.panel.editors.lane.get(&id).copied().unwrap_or(0).min(sh::LANE_COUNT - 1)
    }

    fn shaper_card(&self, d: &Device, def: &'static DeviceDef, card: &mut DeviceCard) {
        let mut c = Canvas::default();
        let cursor = self.sh_draw(d, def, card.accent, &mut c);
        // Knobs: the lane's Rate and Depth, then Mix and Output.
        let values = self.values(d, def);
        let base = sh::lane_base(self.shaper_lane(d.id));
        let idx = [base + sh::RATE, base + sh::DEPTH, 2, 3];
        let knobs: Vec<PanelKnob> = idx.iter().zip(SH_KNOBS).map(|(&i, x)| knob(def, values[i], i, x)).collect();
        card.knobs = ModelRc::from(Rc::new(VecModel::from(knobs)));
        card.canvas = c.into_canvas(SH_W, cursor);
    }

    /// The Shaper's expanded window picture (src/device_window.rs places it): the lane tabs
    /// over the display, the strip and Trigger on its caption row, the curve below. `w` x `h` pt.
    pub(crate) fn shaper_editor_window_canvas(&self, d: &Device, def: &'static DeviceDef, accent: Color, w: f32, h: f32) -> PanelCanvas {
        let _g = ShScope::new(SH_WIN);
        let mut c = Canvas::default();
        let cursor = self.sh_draw(d, def, accent, &mut c);
        PanelCanvas {
            visible: true,
            w: w * Z,
            h: h * Z,
            paths: ModelRc::from(Rc::new(VecModel::from(c.paths))),
            texts: ModelRc::from(Rc::new(VecModel::from(c.texts))),
            interactive: true,
            cursor,
            ..Default::default()
        }
    }

    /// The lane tabs, strip, curve and Trigger in the current geometry ([`ShScope`]); returns
    /// the cursor.
    fn sh_draw(&self, d: &Device, def: &'static DeviceDef, accent: Color, c: &mut Canvas) -> i32 {
        let g = sh_geo();
        let (hit, hp) = self.hover_of(d.id);
        let lane = self.shaper_lane(d.id);
        let values = self.values(d, def);
        let base = sh::lane_base(lane);
        let lane_on = values[base + sh::ON] >= 0.5;
        let depth = values[base + sh::DEPTH] / 100.0;
        let rest_col = ink(0.45);

        // Lane tabs: ring switch + name.
        for (l, (_, name)) in sh::LANES.iter().enumerate() {
            let on = values[sh::lane_base(l) + sh::ON] >= 0.5;
            let x = g.tab_x[l];
            let sel = l == lane;
            let hot = matches!(hit, Hit::LaneOn(h) | Hit::Lane(h) if h == l);
            let ring_col = if on {
                accent
            } else if hot {
                ink(0.7)
            } else {
                rest_col
            };
            c.stroke(PathData::default().circle(x, g.tabs_y, 6.25 * 0.75), ring_col, 1.5);
            if on {
                c.fill(PathData::default().circle(x, g.tabs_y, 3.0 * 0.75), accent);
            }
            let col = if sel {
                ink(1.0)
            } else if hot {
                ink(0.7)
            } else {
                rest_col
            };
            c.text(x + 14.5, g.tabs_y - 10.0, 70.0, 20.0, *name, col, 12.5, 600, 0);
        }

        // Strip row: Shape, Band (+ Split), the lane's own controls.
        let dim = if lane_on { 1.0 } else { 0.55 };
        let pts = sh::lane_points(&values, lane);
        let shape_name = sh::matching_shape(lane, &pts).unwrap_or("Custom");
        let y = g.strip_y;
        c.text(8.0, y, 40.0, 19.0, "Shape", ink(0.4 * dim), 11.5, 500, 0);
        c.pill(48.5, y, 97.0, 19.0, hit == Hit::Shape);
        c.text(56.5, y, 70.0, 19.0, shape_name, ink(0.95 * dim), 11.5, 600, 0);
        c.chevron(135.5, y + 9.5, true, ink(0.65 * dim));
        c.text(162.0, y, 34.0, 19.0, "Band", ink(0.4 * dim), 11.5, 500, 0);
        let band = values[base + sh::BAND].round() as usize;
        let band_w = if band == 0 { 55.0 } else { 55.0 + 56.0 };
        c.pill(196.5, y, band_w, 19.0, matches!(hit, Hit::Band | Hit::Split));
        c.text(204.5, y, 34.0, 19.0, def.params[base + sh::BAND].format(band as f32), ink(0.95 * dim), 11.5, 600, 0);
        c.chevron(241.0, y + 9.5, true, ink(0.65 * dim));
        if band != 0 {
            c.fill(PathData::default().m(251.5, y + 3.0).l(251.5, y + 16.0).l(252.0, y + 16.0).l(252.0, y + 3.0).z(), ink(0.08));
            c.text(
                252.0,
                y,
                55.5,
                19.0,
                def.params[base + sh::SPLIT].format(values[base + sh::SPLIT]),
                ink(if hit == Hit::Split { 0.95 } else { 0.7 } * dim),
                11.0,
                600,
                1,
            );
        }
        let mut x = 196.5 + band_w + 16.0;
        if lane == 4 {
            c.text(x, y, 30.0, 19.0, "Type", ink(0.4 * dim), 11.5, 500, 0);
            x += 32.0;
            c.pill(x, y, 128.0, 19.0, matches!(hit, Hit::FilterType | Hit::Resonance));
            c.text(x + 8.0, y, 64.0, 19.0, def.params[5].format(values[5]), ink(0.95 * dim), 11.5, 600, 0);
            c.chevron(x + 70.0, y + 9.5, true, ink(0.65 * dim));
            c.fill(
                PathData::default().m(x + 79.5, y + 3.0).l(x + 79.5, y + 16.0).l(x + 80.0, y + 16.0).l(x + 80.0, y + 3.0).z(),
                ink(0.08),
            );
            c.text(
                x + 80.0,
                y,
                48.0,
                19.0,
                def.params[6].format(values[6]),
                ink(if hit == Hit::Resonance { 0.95 } else { 0.7 } * dim),
                11.0,
                600,
                1,
            );
        } else if lane == 5 {
            let r = crush_row(x, band != 0);
            c.text(r.bits_label, y, r.bits_pill.0 - r.bits_label, 19.0, "Bits", ink(0.4 * dim), r.font, 500, 0);
            c.pill(r.bits_pill.0, y, r.bits_pill.1, 19.0, hit == Hit::Bits);
            c.text(r.bits_pill.0, y, r.bits_pill.1, 19.0, def.params[9].format(values[9]), ink(0.9 * dim), 11.0, 600, 1);
            c.text(r.dec_label, y, r.dec_pill.0 - r.dec_label, 19.0, "Decimate", ink(0.4 * dim), r.font, 500, 0);
            c.pill(r.dec_pill.0, y, r.dec_pill.1, 19.0, hit == Hit::Decimate);
            c.text(r.dec_pill.0, y, r.dec_pill.1, 19.0, def.params[10].format(values[10]), ink(0.9 * dim), 11.0, 600, 1);
        }

        // Curve well.
        c.fill(PathData::default().rrect(g.well.0, g.well.1, g.well.2, g.well.3, 6.0), rgba(0, 0, 0, 0.26));
        let grid = *self.devices.panel.editors.grid.get(&d.id).unwrap_or(&16);
        let (mut strong, mut weak) = (PathData::default(), PathData::default());
        for k in 0..=grid {
            let gx = sh_px(k as f32 / grid as f32);
            if k % (grid / 4).max(1) == 0 {
                strong = strong.m(gx, g.plot.1).l(gx, g.plot.3);
            } else {
                weak = weak.m(gx, g.plot.1).l(gx, g.plot.3);
            }
        }
        c.stroke(weak, ink(0.05), 1.0);
        c.stroke(strong, ink(0.1), 1.0);
        let quarters = sh::DIVISION_QUARTERS[(values[base + sh::RATE].round() as usize).min(sh::DIVISIONS.len() - 1)];
        let rulers: Vec<(f32, String)> = match lane {
            0 => vec![(1.0, "100".into()), (0.5, fmt_num(100.0 * (1.0 - depth / 2.0))), (0.0, fmt_num(100.0 * (1.0 - depth)))],
            1 => vec![
                (1.0, if depth as f64 * quarters > self.time_max_beats() { "Max".into() } else { "Cycle".into() }),
                (0.0, "Now".into()),
            ],
            2 => vec![(1.0, "Wide".into()), (0.5, "As is".into()), (0.0, "Mono".into())],
            3 => vec![(1.0, "R".into()), (0.5, "C".into()), (0.0, "L".into())],
            5 => vec![(1.0, "Full".into()), (0.0, "Clean".into())],
            6 => vec![(1.0, format!("+{}", fmt_num(12.0 * depth))), (0.5, "0".into()), (0.0, format!("{MINUS}{}", fmt_num(12.0 * depth)))],
            _ => vec![],
        };
        if lane == 6 && depth > 0.0 {
            let spacing = (g.plot.3 - g.plot.1) / (24.0 * depth);
            if spacing >= 5.0 {
                let (mut semis, mut fifths) = (PathData::default(), PathData::default());
                for s in -12i32..=12 {
                    let yy = 0.5 + s as f32 / (24.0 * depth);
                    if !(0.0..=1.0).contains(&yy) || s == 0 {
                        continue;
                    }
                    let py = sh_py(yy);
                    if s.abs() == 7 {
                        fifths = fifths.m(g.plot.0, py).l(g.plot.2, py);
                    } else {
                        semis = semis.m(g.plot.0, py).l(g.plot.2, py);
                    }
                }
                c.stroke(semis, ink(0.05), 1.0);
                c.stroke(fifths, ink(0.1), 1.0);
            }
        }
        let rest = sh::REST[lane];
        for (ry, text) in &rulers {
            let py = sh_py(*ry);
            c.text(g.plot.0 - 46.0, py - 7.0, 38.0, 14.0, text.as_str(), ink(0.5), 10.0, 600, 2);
            if (*ry - rest).abs() > 1e-4 {
                c.stroke(PathData::default().m(g.plot.0, py).l(g.plot.2, py), ink(0.05), 1.0);
            }
        }
        let mut dash = PathData::default();
        let mut xx = g.plot.0;
        while xx < g.plot.2 {
            dash = dash.m(xx, sh_py(rest)).l((xx + 3.0).min(g.plot.2), sh_py(rest));
            xx += 6.0;
        }
        c.stroke(dash, ink(0.22), 1.0);

        let curve_alpha = if lane_on { 1.0 } else { 0.4 };
        let n = 240;
        let line: Vec<(f32, f32)> = (0..=n)
            .map(|i| {
                let x = i as f32 / n as f32;
                (sh_px(x), sh_py(sh::eval(&pts, x)))
            })
            .collect();
        // Vertical jumps (two points at one x) are drawn as steps.
        let mut line = line;
        for w in pts.windows(2) {
            if (w[1].x - w[0].x).abs() < 1e-6 {
                let px = sh_px(w[0].x);
                let pos = line.iter().position(|(x, _)| *x >= px).unwrap_or(line.len());
                line.insert(pos, (px, sh_py(w[1].y)));
                line.insert(pos, (px, sh_py(w[0].y)));
            }
        }
        let area = PathData::default().poly(&line).l(g.plot.2, g.plot.3).l(g.plot.0, g.plot.3).z();
        c.fill(area, vgrad(alpha(accent, 0.3 * curve_alpha), alpha(accent, 0.04 * curve_alpha)));
        c.stroke(PathData::default().poly(&line), alpha(accent, if lane_on { 1.0 } else { 0.45 }), 2.0);
        let over_plot = in_rect(hp, g.well.0, g.well.1, g.well.2, g.well.3);
        let gesture = match &self.devices.panel.editors.gesture {
            Some(Gesture::Curve { device, k, mid, .. }) if *device == d.id => Some((*k, *mid)),
            _ => None,
        };
        if over_plot || gesture.is_some() {
            for (k, w) in pts.windows(2).enumerate() {
                if (w[1].x - w[0].x).abs() < 1e-6 || (w[1].y - w[0].y).abs() < 1e-6 {
                    continue;
                }
                let mx = 0.5 * (w[0].x + w[1].x);
                let hot = hit == Hit::Mid(k) || gesture == Some((k, true));
                let r = if hot { 4.5 } else { 3.5 };
                c.path(
                    PathData::default().circle(sh_px(mx), sh_py(sh::eval(&pts, mx)), r),
                    if hot { accent } else { ink(0.6) },
                    1.2,
                    rgba(0x17, 0x18, 0x1b, 1.0),
                );
            }
        }
        for (k, p) in pts.iter().enumerate() {
            let hot = hit == Hit::CurvePoint(k) || gesture == Some((k, false));
            let r = if hot { 5.5 } else { 4.0 };
            c.path(
                PathData::default().circle(sh_px(p.x), sh_py(p.y), r),
                if hot { accent } else { ink(0.9 * curve_alpha) },
                1.5,
                if hot { Color::from_rgb_u8(255, 255, 255) } else { rgba(0x17, 0x18, 0x1b, 1.0) },
            );
        }
        // Readout bubble: the hot point's value, or the curve under the pointer.
        let bubble = match (hit, gesture) {
            (_, Some((k, false))) | (Hit::CurvePoint(k), None) => pts.get(k).map(|p| (p.x, p.y)),
            (_, Some((k, true))) | (Hit::Mid(k), None) => pts.get(k + 1).map(|b| {
                let mx = 0.5 * (pts[k].x + b.x);
                (mx, sh::eval(&pts, mx))
            }),
            (Hit::Plot, None) => {
                let x = sh_x_of(hp.0);
                Some((x, sh::eval(&pts, x)))
            }
            _ => None,
        };
        if let Some((bx, by)) = bubble {
            let text = lane_text(lane, by, depth, quarters, self.time_max_beats(), values[7], values[8]);
            let w = 10.0 + 6.2 * text.chars().count() as f32;
            let (px, py) = (sh_px(bx), sh_py(by));
            let top = if py - 30.0 < g.well.1 { py + 10.0 } else { py - 28.0 };
            let left = (px - w / 2.0).clamp(g.well.0 + 2.0, g.well.0 + g.well.2 - w - 2.0);
            c.fill(PathData::default().rrect(left, top, w, 17.0, 8.5), rgba(0x2e, 0x2f, 0x35, 0.96));
            c.text(left, top, w, 17.0, text, ink(0.95), 10.5, 600, 1);
        }
        // Filter range pills (High top right, Low bottom right).
        if lane == 4 {
            for (i, (idx, h)) in [(8usize, Hit::FilterHigh), (7usize, Hit::FilterLow)].into_iter().enumerate() {
                let w = 38.0;
                let px = g.well.0 + g.well.2 - w - 6.0;
                let py = if i == 0 { g.well.1 + 6.0 } else { g.well.1 + g.well.3 - 6.0 - 16.0 };
                c.fill(
                    PathData::default().rrect(px, py, w, 16.0, 8.0),
                    if hit == h { rgba(0x29, 0x2a, 0x30, 1.0) } else { rgba(0x22, 0x23, 0x28, 1.0) },
                );
                c.text(px, py, w, 16.0, short_hz(values[idx]), accent.brighter(if hit == h { 0.35 } else { 0.2 }), 10.0, 650, 1);
            }
        }

        // Trigger cell: [Sync / This track / source] | threshold, caption under.
        let (tx, ty, tw, th) = g.trigger;
        let audio = values[0] >= 0.5;
        c.pill(tx, ty, tw, th, matches!(hit, Hit::Trigger | Hit::Threshold));
        let source = if !audio {
            "Sync".to_string()
        } else {
            match d.sidechain.filter(|_| values[4] >= 0.5) {
                Some(t) => self.project().track(t).map_or_else(|| "Removed".to_string(), |t| t.name.clone()),
                None => "This track".to_string(),
            }
        };
        c.text(tx + 8.0, ty, SH_TRIGGER_SPLIT - 24.0, th, source, ink(0.95), 11.5, 600, 0);
        c.chevron(tx + SH_TRIGGER_SPLIT - 11.0, ty + th / 2.0, true, ink(0.65));
        c.fill(
            PathData::default()
                .m(tx + SH_TRIGGER_SPLIT, ty + 3.0)
                .l(tx + SH_TRIGGER_SPLIT, ty + th - 3.0)
                .l(tx + SH_TRIGGER_SPLIT + 0.5, ty + th - 3.0)
                .l(tx + SH_TRIGGER_SPLIT + 0.5, ty + 3.0)
                .z(),
            ink(0.08),
        );
        let thr_alpha = if audio { if hit == Hit::Threshold { 0.95 } else { 0.75 } } else { 0.35 };
        c.text(tx + SH_TRIGGER_SPLIT, ty, tw - SH_TRIGGER_SPLIT, th, def.params[1].format(values[1]), ink(thr_alpha), 11.0, 600, 1);
        if g.trigger_caption_under {
            c.text(tx, 284.5, tw, 14.0, "Trigger", rgba(0xe5, 0xe5, 0xe8, 1.0), 11.0, 600, 1);
        } else {
            c.text(tx - 60.0, ty, 52.0, th, "Trigger", ink(0.4), 11.5, 500, 2);
        }

        match hit {
            Hit::LaneOn(_) | Hit::Lane(_) | Hit::Shape | Hit::Band | Hit::FilterType | Hit::Trigger => CURSOR_POINTER,
            Hit::Split | Hit::Resonance | Hit::Bits | Hit::Decimate | Hit::Threshold | Hit::FilterHigh | Hit::FilterLow => CURSOR_NS,
            Hit::CurvePoint(k) => {
                if k == 0 || k + 1 == pts.len() {
                    CURSOR_NS
                } else {
                    CURSOR_MOVE
                }
            }
            Hit::Mid(_) => CURSOR_NS,
            Hit::Plot if pts.len() < sh::MAX_POINTS => CURSOR_CROSSHAIR,
            _ => 0,
        }
    }

    fn sh_hit(&self, d: &Device, def: &DeviceDef, p: (f32, f32)) -> Hit {
        let g = sh_geo();
        let lane = self.shaper_lane(d.id);
        let values = self.values(d, def);
        let base = sh::lane_base(lane);
        for l in 0..sh::LANE_COUNT {
            let x = g.tab_x[l];
            if in_rect(p, x - 8.0, g.tabs_y - 9.0, 16.0, 18.0) {
                return Hit::LaneOn(l);
            }
            let end = if l + 1 < sh::LANE_COUNT { g.tab_x[l + 1] - 12.0 } else { x + 70.0 };
            if in_rect(p, x + 8.0, g.tabs_y - 10.0, end - x - 8.0, 20.0) {
                return Hit::Lane(l);
            }
        }
        let y = g.strip_y;
        if in_rect(p, 48.5, y, 97.0, 19.0) {
            return Hit::Shape;
        }
        let band = values[base + sh::BAND].round() as usize;
        if in_rect(p, 196.5, y, 55.0, 19.0) {
            return Hit::Band;
        }
        let band_w = if band == 0 { 55.0 } else { 111.0 };
        if band != 0 && in_rect(p, 252.0, y, 55.5, 19.0) {
            return Hit::Split;
        }
        let x = 196.5 + band_w + 16.0;
        if lane == 4 {
            if in_rect(p, x + 32.0, y, 80.0, 19.0) {
                return Hit::FilterType;
            }
            if in_rect(p, x + 112.0, y, 48.0, 19.0) {
                return Hit::Resonance;
            }
        } else if lane == 5 {
            let r = crush_row(x, band != 0);
            if in_rect(p, r.bits_pill.0, y, r.bits_pill.1, 19.0) {
                return Hit::Bits;
            }
            if in_rect(p, r.dec_pill.0, y, r.dec_pill.1, 19.0) {
                return Hit::Decimate;
            }
        }
        let (tx, ty, tw, th) = g.trigger;
        if in_rect(p, tx, ty, SH_TRIGGER_SPLIT, th) {
            return Hit::Trigger;
        }
        if in_rect(p, tx + SH_TRIGGER_SPLIT, ty, tw - SH_TRIGGER_SPLIT, th) {
            return if values[0] >= 0.5 { Hit::Threshold } else { Hit::None };
        }
        if !in_rect(p, g.well.0, g.well.1, g.well.2, g.well.3) {
            return Hit::None;
        }
        if lane == 4 {
            let px = g.well.0 + g.well.2 - 44.0;
            if in_rect(p, px, g.well.1 + 6.0, 38.0, 16.0) {
                return Hit::FilterHigh;
            }
            if in_rect(p, px, g.well.1 + g.well.3 - 22.0, 38.0, 16.0) {
                return Hit::FilterLow;
            }
        }
        let pts = sh::lane_points(&values, lane);
        let near = |x: f32, y: f32| ((p.0 - sh_px(x)).powi(2) + (p.1 - sh_py(y)).powi(2)).sqrt() <= 9.0;
        if let Some(k) = pts.iter().enumerate().rev().find(|(_, q)| near(q.x, q.y)).map(|(k, _)| k) {
            return Hit::CurvePoint(k);
        }
        for (k, w) in pts.windows(2).enumerate() {
            if (w[1].x - w[0].x).abs() < 1e-6 || (w[1].y - w[0].y).abs() < 1e-6 {
                continue;
            }
            let mx = 0.5 * (w[0].x + w[1].x);
            if near(mx, sh::eval(&pts, mx)) {
                return Hit::Mid(k);
            }
        }
        Hit::Plot
    }

    // ---------------------------------------------------------------- live values

    fn live_set(&mut self, device: DeviceId, def: &DeviceDef, i: usize, v: f32) {
        let Some(p) = def.params.get(i) else { return };
        let v = p.snap(v);
        if self.devices.panel.editors.live.get(&(device, i)) == Some(&v) {
            return;
        }
        self.devices.panel.editors.live.insert((device, i), v);
        let _ = self.engine.builtin_set_param(device.0, i as u32, v);
        self.ui_dirty = true;
    }

    /// Commit a gesture's live values as one undo step.
    fn live_commit(&mut self, device: DeviceId, label: &str) {
        let live: Vec<(usize, f32)> =
            self.devices.panel.editors.live.iter().filter(|((d, _), _)| *d == device).map(|((_, i), v)| (*i, *v)).collect();
        self.devices.panel.editors.live.retain(|(d, _), _| *d != device);
        self.ui_dirty = true;
        let Some((d, def)) = self.panel_def(device) else { return };
        let mut changed: Vec<(usize, f32)> = live.into_iter().filter(|(i, v)| (def.value(d, &def.params[*i]) - v).abs() > 0.0).collect();
        changed.sort_by_key(|(i, _)| *i);
        let mut commands: Vec<Command> =
            changed.iter().map(|(i, v)| Command::SetDeviceParam { device, param: def.params[*i].id.to_string(), value: *v }).collect();
        match commands.len() {
            0 => {}
            1 => {
                self.panel_commit(commands.remove(0));
            }
            _ => {
                self.panel_commit(Command::Batch { label: label.to_string(), commands });
            }
        }
    }

    /// Restore the model values of a gesture's live parameters.
    fn live_cancel(&mut self, device: DeviceId) {
        let live: Vec<usize> = self.devices.panel.editors.live.keys().filter(|(d, _)| *d == device).map(|(_, i)| *i).collect();
        self.devices.panel.editors.live.retain(|(d, _), _| *d != device);
        if let Some((d, def)) = self.panel_def(device) {
            let restore: Vec<(usize, f32)> = live.into_iter().map(|i| (i, def.value(d, &def.params[i]))).collect();
            for (i, v) in restore {
                let _ = self.engine.builtin_set_param(device.0, i as u32, v);
            }
        }
        self.ui_dirty = true;
    }

    /// Several parameters as one command (one undo step).
    fn set_many(&mut self, device: DeviceId, label: &str, writes: Vec<(&'static str, f32)>) {
        let Some((d, def)) = self.panel_def(device) else { return };
        let commands: Vec<Command> = writes
            .into_iter()
            .filter(|(id, v)| def.param(id).is_some_and(|p| (def.value(d, p) - p.snap(*v)).abs() > 0.0))
            .map(|(id, value)| Command::SetDeviceParam { device, param: id.to_string(), value })
            .collect();
        match commands.len() {
            0 => {}
            1 => {
                self.panel_commit(commands.into_iter().next().unwrap());
            }
            _ => {
                self.panel_commit(Command::Batch { label: label.to_string(), commands });
            }
        }
    }

    /// Escape: cancel a running field gesture. True when one was running.
    pub(crate) fn editor_cancel(&mut self) -> bool {
        let Some(g) = self.devices.panel.editors.gesture.take() else { return false };
        match g {
            Gesture::Param { .. } => {
                self.panel_cancel_drag();
            }
            Gesture::Paint { device, .. } | Gesture::Env { device, .. } | Gesture::Curve { device, .. } => self.live_cancel(device),
        }
        true
    }

    // ---------------------------------------------------------------- events

    pub fn panel_editor(&mut self, device: DeviceId, mut e: CanvasEvent) {
        // Kind + 16: the event comes from the device's expanded window, drawn in its geometry.
        let win = e.kind >= 16;
        e.kind %= 16;
        let Some((d, def)) = self.panel_def(device) else { return };
        let name = match def.layout.view {
            Some(dstudio_core::builtin::View::Custom(n)) => n,
            _ => return,
        };
        let _geo = (win && name == "trance-gate").then(|| TgScope::new(TG_WIN));
        let _sh = (win && name == "shaper").then(|| ShScope::new(SH_WIN));
        let p = e.pt();
        let hit = match name {
            "trance-gate" => self.tg_hit(d, def, p),
            "shaper" => self.sh_hit(d, def, p),
            _ => Hit::None,
        };
        if e.kind == HOVER || e.kind == LEAVE {
            let new = if e.kind == LEAVE { None } else { Some((device, hit, p)) };
            let old = self.devices.panel.editors.hover;
            if old.map(|o| (o.0, o.1)) != new.map(|n| (n.0, n.1)) || (hit == Hit::Plot && new.is_some()) {
                self.devices.panel.editors.hover = new;
                self.ui_dirty = true;
            }
            return;
        }
        match e.kind {
            MOVE => self.editor_move(def, e, p),
            UP => self.editor_release(device, def),
            CANCEL => {
                self.editor_cancel();
            }
            _ => match name {
                "trance-gate" => self.tg_event(device, def, e, hit),
                _ => self.sh_event(device, def, e, hit),
            },
        }
    }

    fn editor_move(&mut self, def: &'static DeviceDef, e: CanvasEvent, p: (f32, f32)) {
        let Some(g) = self.devices.panel.editors.gesture.clone() else { return };
        match g {
            Gesture::Param { device, .. } => {
                let last = self.devices.panel.editors.hover.map(|h| h.2).unwrap_or(p);
                self.devices.panel.editors.hover = Some((device, self.devices.panel.editors.hover.map_or(Hit::None, |h| h.1), p));
                let index = self.panel_drag_index().unwrap_or(-1);
                self.panel_drag(device, index, 1, (last.1 - p.1) * Z, 1.0 / 160.0, e.shift);
            }
            Gesture::Paint { device, on, last } => {
                let Some((d, _)) = self.panel_def(device) else { return };
                let (Hit::Step(k), (_, steps, _)) = (self.tg_hit(d, def, p), self.tg_pattern(d, def)) else { return };
                if k == last {
                    return;
                }
                // Every step crossed since the last event, the short way round the ring.
                let n = steps as usize;
                let (fwd, back) = ((k + n - last) % n, (last + n - k) % n);
                let crossed: Vec<usize> = if fwd <= back {
                    (1..=fwd).map(|s| (last + s) % n).collect()
                } else {
                    (1..=back).map(|s| (last + n - s) % n).collect()
                };
                for s in crossed {
                    self.tg_paint(device, def, s, on);
                }
                self.devices.panel.editors.gesture = Some(Gesture::Paint { device, on, last: k });
            }
            Gesture::Env { device, handle, span, gate_ms } => {
                let (x0, y0, x1, y1) = tg_geo().plot;
                let t = ((p.0 - x0) / (x1 - x0)).clamp(0.0, 1.0) * span;
                let Some((d, _)) = self.panel_def(device) else { return };
                let attack = self.fv(d, def, 2);
                match handle {
                    Hit::EnvA => self.live_set(device, def, 2, t),
                    Hit::EnvD => {
                        self.live_set(device, def, 3, t - attack);
                        self.live_set(device, def, 4, ((y1 - p.1) / (y1 - y0)).clamp(0.0, 1.0) * 100.0);
                    }
                    _ => self.live_set(device, def, 5, t - gate_ms),
                }
            }
            Gesture::Curve { device, lane, k, mid, mut pts, raw, last } => {
                let factor = if e.shift { 0.25 } else { 1.0 };
                let free = e.shift || e.alt;
                let raw = (raw.0 + (p.0 - last.0) * factor, raw.1 + (p.1 - last.1) * factor);
                let grid = *self.devices.panel.editors.grid.get(&device).unwrap_or(&16) as f32;
                if mid {
                    if let (Some(a), Some(b)) = (pts.get(k).copied(), pts.get(k + 1).copied()) {
                        pts[k].tension = sh::tension_through(a, b, sh_y_of(raw.1));
                    }
                } else {
                    let n = pts.len();
                    let mut x = sh_x_of(raw.0);
                    let mut y = sh_y_of(raw.1);
                    if !free {
                        x = (x * grid).round() / grid;
                        y = if lane == 6 { (y * 24.0).round() / 24.0 } else { (y * 100.0).round() / 100.0 };
                    }
                    if k == 0 || k + 1 == n {
                        x = if k == 0 { 0.0 } else { 1.0 };
                    } else {
                        x = x.clamp(pts[k - 1].x, pts[k + 1].x);
                    }
                    pts[k].x = x;
                    pts[k].y = y;
                }
                self.sh_write_live(device, def, lane, &pts);
                self.devices.panel.editors.gesture = Some(Gesture::Curve { device, lane, k, mid, pts, raw, last: p });
            }
        }
    }

    fn editor_release(&mut self, device: DeviceId, def: &'static DeviceDef) {
        let Some(g) = self.devices.panel.editors.gesture.take() else { return };
        match g {
            Gesture::Param { menu, .. } => {
                let moved = self.panel_drag_active();
                let index = self.panel_drag_index().unwrap_or(-1);
                self.panel_drag(device, index, 2, 0.0, 1.0 / 160.0, false);
                if !moved && let Some(which) = menu {
                    let (x, y) = self.devices.panel.editors.menu_at;
                    self.editor_menu(device, def, which, x, y);
                }
            }
            Gesture::Paint { .. } => self.live_commit(device, "Edit Pattern"),
            Gesture::Env { .. } => self.live_commit(device, "Edit Envelope"),
            Gesture::Curve { .. } => self.live_commit(device, "Edit Shape"),
        }
    }
}

// ---------------------------------------------------------------- clicks, menus

const MENU_PATTERN: i32 = 0;
const MENU_SHAPE: i32 = 1;
const MENU_CURVE: i32 = 2;
const MENU_TRIGGER: i32 = 3;
/// Opens the choice menu of parameter (code - MENU_CHOICE).
const MENU_CHOICE: i32 = 1000;

impl Controller {
    fn begin_param(&mut self, device: DeviceId, index: usize, hit: Hit, p: (f32, f32), e: CanvasEvent, menu: Option<i32>) {
        self.devices.panel.editors.hover = Some((device, hit, p));
        self.devices.panel.editors.menu_at = (e.wx, e.wy);
        self.panel_drag(device, index as i32, 0, 0.0, 1.0 / 160.0, false);
        self.devices.panel.editors.gesture = Some(Gesture::Param { device, menu });
    }

    fn tg_event(&mut self, device: DeviceId, def: &'static DeviceDef, e: CanvasEvent, hit: Hit) {
        let Some((d, _)) = self.panel_def(device) else { return };
        let (pattern, steps, mask) = self.tg_pattern(d, def);
        let steps_idx = 8 + 3 * pattern;
        let p = e.pt();
        match (e.kind, hit) {
            (DOWN, Hit::Step(k)) => {
                let on = mask >> k & 1 == 0;
                self.tg_paint(device, def, k, on);
                self.devices.panel.editors.gesture = Some(Gesture::Paint { device, on, last: k });
            }
            (DOWN, Hit::StepMore) if steps < tg::MAX_STEPS => self.panel_step(device, steps_idx as i32, 1, false),
            (DOWN, Hit::StepFewer) if steps > 1 => self.panel_step(device, steps_idx as i32, -1, false),
            (DOWN, Hit::StepCount) => self.begin_param(device, steps_idx, hit, p, e, None),
            (DOWN, Hit::Rate) => self.begin_param(device, 1, hit, p, e, Some(MENU_CHOICE + 1)),
            (DOWN, Hit::Pattern(i)) => self.panel_choice(device, 0, i as i32),
            (DOWN, Hit::EnvA | Hit::EnvD | Hit::EnvR) => {
                let env = self.adsr(d, def);
                let rate = (self.fv(d, def, 1).round() as usize).min(tg::RATES.len() - 1);
                let (gate_ms, span) = env_span(&env, self.step_ms(rate));
                self.devices.panel.editors.gesture = Some(Gesture::Env { device, handle: hit, span, gate_ms });
                self.ui_dirty = true;
            }
            (RIGHT, Hit::Step(_) | Hit::Hole | Hit::StepCount | Hit::Pattern(_)) => self.editor_menu(device, def, MENU_PATTERN, e.wx, e.wy),
            (DOUBLE, Hit::StepCount) => self.panel_reset(device, steps_idx as i32),
            (DOUBLE, Hit::EnvA) => self.panel_reset(device, 2),
            (DOUBLE, Hit::EnvD) => {
                let writes = vec![("decay", def.params[3].default), ("sustain", def.params[4].default)];
                self.set_many(device, "Reset Decay", writes);
            }
            (DOUBLE, Hit::EnvR) => self.panel_reset(device, 5),
            (WHEEL, Hit::StepCount) => self.panel_step(device, steps_idx as i32, if e.wheel > 0.0 { 1 } else { -1 }, false),
            (WHEEL, Hit::Rate) => self.panel_step(device, 1, if e.wheel > 0.0 { -1 } else { 1 }, false),
            (WHEEL, Hit::Pattern(_)) => self.panel_step(device, 0, if e.wheel > 0.0 { -1 } else { 1 }, false),
            _ => {}
        }
    }

    /// Set step k of the active pattern (live; the release commits).
    fn tg_paint(&mut self, device: DeviceId, def: &'static DeviceDef, k: usize, on: bool) {
        let Some((d, _)) = self.panel_def(device) else { return };
        let (pattern, _, mask) = self.tg_pattern(d, def);
        let mask = if on { mask | 1 << k } else { mask & !(1 << k) };
        let (lo, hi) = tg::halves(mask);
        let base = 8 + 3 * pattern;
        self.live_set(device, def, base + 1, lo);
        self.live_set(device, def, base + 2, hi);
    }

    fn sh_event(&mut self, device: DeviceId, def: &'static DeviceDef, e: CanvasEvent, hit: Hit) {
        let Some((d, _)) = self.panel_def(device) else { return };
        let lane = self.shaper_lane(device);
        let base = sh::lane_base(lane);
        let values = self.values(d, def);
        let pts = sh::lane_points(&values, lane);
        let p = e.pt();
        let step = |w: f32| if w > 0.0 { 1 } else { -1 };
        match (e.kind, hit) {
            (DOWN, Hit::LaneOn(l)) => {
                let i = sh::lane_base(l) + sh::ON;
                let on = values[i] >= 0.5;
                self.devices.panel.editors.lane.insert(device, l);
                self.panel_choice(device, i as i32, if on { 0 } else { 1 });
                self.ui_dirty = true;
            }
            (DOWN, Hit::Lane(l)) => {
                self.devices.panel.editors.lane.insert(device, l);
                self.ui_dirty = true;
            }
            (DOWN, Hit::Shape) => self.editor_menu(device, def, MENU_SHAPE, e.wx, e.wy),
            (DOWN, Hit::Trigger) => self.editor_menu(device, def, MENU_TRIGGER, e.wx, e.wy),
            (DOWN, Hit::Band) => self.begin_param(device, base + sh::BAND, hit, p, e, Some(MENU_CHOICE + (base + sh::BAND) as i32)),
            (DOWN, Hit::FilterType) => self.begin_param(device, 5, hit, p, e, Some(MENU_CHOICE + 5)),
            (DOWN, Hit::Split) => self.begin_param(device, base + sh::SPLIT, hit, p, e, None),
            (DOWN, Hit::Resonance) => self.begin_param(device, 6, hit, p, e, None),
            (DOWN, Hit::Bits) => self.begin_param(device, 9, hit, p, e, None),
            (DOWN, Hit::Decimate) => self.begin_param(device, 10, hit, p, e, None),
            (DOWN, Hit::Threshold) => self.begin_param(device, 1, hit, p, e, None),
            (DOWN, Hit::FilterLow) => self.begin_param(device, 7, hit, p, e, None),
            (DOWN, Hit::FilterHigh) => self.begin_param(device, 8, hit, p, e, None),
            (DOWN, Hit::CurvePoint(k)) => {
                let inner = k > 0 && k + 1 < pts.len();
                if e.alt && inner {
                    let mut next = pts.clone();
                    next.remove(k);
                    self.set_many(device, "Delete Point", sh::lane_writes(lane, &next));
                    return;
                }
                let q = pts[k];
                self.devices.panel.editors.gesture =
                    Some(Gesture::Curve { device, lane, k, mid: false, pts, raw: (sh_px(q.x), sh_py(q.y)), last: p });
                self.ui_dirty = true;
            }
            (DOWN, Hit::Mid(k)) => {
                self.devices.panel.editors.gesture = Some(Gesture::Curve { device, lane, k, mid: true, pts, raw: p, last: p });
                self.ui_dirty = true;
            }
            (DOWN, Hit::Plot) if pts.len() < sh::MAX_POINTS => {
                let grid = *self.devices.panel.editors.grid.get(&device).unwrap_or(&16) as f32;
                let (mut x, mut y) = (sh_x_of(p.0), sh_y_of(p.1));
                if !(e.shift || e.alt) {
                    x = (x * grid).round() / grid;
                    y = if lane == 6 { (y * 24.0).round() / 24.0 } else { (y * 100.0).round() / 100.0 };
                }
                x = x.clamp(1e-3, 1.0 - 1e-3);
                let k = pts.iter().position(|q| q.x > x).unwrap_or(pts.len() - 1).max(1);
                let mut next = pts;
                next.insert(k, Point { x, y, tension: 0.0 });
                self.sh_write_live(device, def, lane, &next);
                self.devices.panel.editors.gesture =
                    Some(Gesture::Curve { device, lane, k, mid: false, pts: next, raw: (sh_px(x), sh_py(y)), last: p });
            }
            (RIGHT | DOUBLE, Hit::CurvePoint(k)) if k > 0 && k + 1 < pts.len() => {
                let mut next = pts;
                next.remove(k);
                self.set_many(device, "Delete Point", sh::lane_writes(lane, &next));
            }
            (DOUBLE, Hit::Mid(k)) => {
                let mut next = pts;
                next[k].tension = 0.0;
                self.set_many(device, "Reset Bend", sh::lane_writes(lane, &next));
            }
            (RIGHT, Hit::Plot | Hit::CurvePoint(_) | Hit::Mid(_)) => self.editor_menu(device, def, MENU_CURVE, e.wx, e.wy),
            (DOUBLE, Hit::Split) => self.panel_reset(device, (base + sh::SPLIT) as i32),
            (DOUBLE, Hit::Resonance) => self.panel_reset(device, 6),
            (DOUBLE, Hit::Bits) => self.panel_reset(device, 9),
            (DOUBLE, Hit::Decimate) => self.panel_reset(device, 10),
            (DOUBLE, Hit::Threshold) => self.panel_reset(device, 1),
            (DOUBLE, Hit::FilterLow) => self.panel_reset(device, 7),
            (DOUBLE, Hit::FilterHigh) => self.panel_reset(device, 8),
            (WHEEL, Hit::Split) => self.panel_step(device, (base + sh::SPLIT) as i32, step(e.wheel), e.shift),
            (WHEEL, Hit::Resonance) => self.panel_step(device, 6, step(e.wheel), e.shift),
            (WHEEL, Hit::Bits) => self.panel_step(device, 9, step(e.wheel), e.shift),
            (WHEEL, Hit::Decimate) => self.panel_step(device, 10, step(e.wheel), e.shift),
            (WHEEL, Hit::Threshold) => self.panel_step(device, 1, step(e.wheel), e.shift),
            (WHEEL, Hit::FilterLow) => self.panel_step(device, 7, step(e.wheel), e.shift),
            (WHEEL, Hit::FilterHigh) => self.panel_step(device, 8, step(e.wheel), e.shift),
            _ => {}
        }
    }

    /// Write a lane's points as live values (the release commits them).
    fn sh_write_live(&mut self, device: DeviceId, def: &'static DeviceDef, lane: usize, pts: &[Point]) {
        for (id, v) in sh::lane_writes(lane, pts) {
            if let Some(i) = def.index_of(id) {
                self.live_set(device, def, i, v);
            }
        }
    }

    fn editor_menu(&mut self, device: DeviceId, def: &'static DeviceDef, which: i32, x: f32, y: f32) {
        let Some((d, _)) = self.panel_def(device) else { return };
        let check = |on: bool| if on { "\u{2713}".to_string() } else { String::new() };
        let pick = |code: i32| Some(MenuPick::View(device, code));
        let mut rows: Vec<(String, String, Option<MenuPick>, bool)> = Vec::new();
        let mut w = 180.0;
        match which {
            MENU_PATTERN => {
                let clip = self.devices.panel.editors.pattern_clip.is_some();
                for (label, code) in [
                    ("Clear", TG_CLEAR),
                    ("Fill", TG_FILL),
                    ("Invert", TG_INVERT),
                    ("Rotate Left", TG_ROT_LEFT),
                    ("Rotate Right", TG_ROT_RIGHT),
                ] {
                    rows.push((label.into(), String::new(), pick(code), false));
                }
                rows.push(("Copy Pattern".into(), String::new(), pick(TG_COPY), true));
                rows.push(("Paste Pattern".into(), String::new(), if clip { pick(TG_PASTE) } else { None }, false));
            }
            MENU_SHAPE => {
                let lane = self.shaper_lane(device);
                let pts = sh::lane_points(&self.values(d, def), lane);
                let cur = sh::matching_shape(lane, &pts);
                for (i, s) in sh::SHAPES[lane].iter().enumerate() {
                    rows.push((s.name.into(), check(cur == Some(s.name)), pick(SH_SHAPE + i as i32), false));
                }
            }
            MENU_CURVE => {
                let clip = self.devices.panel.editors.shape_clip.is_some();
                for (label, code) in [("Flip", SH_FLIP), ("Reverse", SH_REVERSE), ("Clear", SH_CLEAR)] {
                    rows.push((label.into(), String::new(), pick(code), false));
                }
                rows.push(("Copy Shape".into(), String::new(), pick(SH_COPY), true));
                rows.push(("Paste Shape".into(), String::new(), if clip { pick(SH_PASTE) } else { None }, false));
                let grid = *self.devices.panel.editors.grid.get(&device).unwrap_or(&16);
                for (i, g) in [4u32, 8, 16, 32].into_iter().enumerate() {
                    rows.push((format!("Grid {g}"), check(grid == g), pick(SH_GRID + g as i32), i == 0));
                }
            }
            MENU_TRIGGER => {
                let audio = self.fv(d, def, 0) >= 0.5;
                let side = d.sidechain.filter(|_| self.fv(d, def, 4) >= 0.5);
                rows.push(("Sync".into(), check(!audio), pick(SH_TRIGGER_SYNC), false));
                rows.push(("This track".into(), check(audio && side.is_none()), pick(SH_TRIGGER_SELF), false));
                let own = self.project().find_device(device).map(|(ti, _)| self.project().tracks[ti].id);
                let mut first = true;
                for (ti, t) in self
                    .project()
                    .tracks
                    .iter()
                    .enumerate()
                    .filter(|(_, t)| Some(t.id) != own && (d.sidechain == Some(t.id) || !self.sidechain_feeds_back(device, t.id)))
                {
                    rows.push((
                        t.name.clone(),
                        check(audio && side == Some(t.id)),
                        pick(SH_TRIGGER_TRACK + ti as i32),
                        std::mem::take(&mut first),
                    ));
                }
                w = 200.0;
            }
            _ => {
                let Some(index) = usize::try_from(which - MENU_CHOICE).ok() else { return };
                let Some(p) = def.params.get(index) else { return };
                let cur = self.fv(d, def, index).round() as usize;
                for (i, c) in p.choices.iter().enumerate() {
                    rows.push(((*c).into(), check(i == cur), Some(MenuPick::Choice(device, index as i32, i as i32)), false));
                }
                w = 120.0;
            }
        }
        // The separator flag of the first row is meaningless; keep only later ones.
        if let Some(r) = rows.first_mut() {
            r.3 = false;
        }
        self.devices.menu = Some(DevMenu { pos: (x, y), w, rows });
        self.ui_dirty = true;
    }

    /// A custom-view menu row was picked.
    pub(crate) fn editor_pick(&mut self, device: DeviceId, code: i32) {
        let Some((d, def)) = self.panel_def(device) else { return };
        match def.id {
            "trance-gate" => {
                let (pattern, steps, mask) = self.tg_pattern(d, def);
                let full = if steps >= 32 { u32::MAX } else { (1u32 << steps) - 1 };
                let rot = |m: u32, left: bool| {
                    let m = m & full;
                    if steps <= 1 {
                        return m;
                    }
                    if left { (m >> 1 | m << (steps - 1)) & full } else { (m << 1 | m >> (steps - 1)) & full }
                };
                let (next, n) = match code {
                    TG_CLEAR => (0, steps),
                    TG_FILL => (full, steps),
                    TG_INVERT => (!mask & full, steps),
                    TG_ROT_LEFT => (rot(mask, true), steps),
                    TG_ROT_RIGHT => (rot(mask, false), steps),
                    TG_COPY => {
                        self.devices.panel.editors.pattern_clip = Some((mask & full, steps));
                        return;
                    }
                    TG_PASTE => match self.devices.panel.editors.pattern_clip {
                        Some(c) => c,
                        None => return,
                    },
                    _ => return,
                };
                let (s_id, lo_id, hi_id) = tg::pattern_ids(pattern);
                let (lo, hi) = tg::halves(next);
                self.set_many(device, "Edit Pattern", vec![(s_id, n as f32), (lo_id, lo), (hi_id, hi)]);
            }
            "shaper" => {
                let lane = self.shaper_lane(device);
                let pts = sh::lane_points(&self.values(d, def), lane);
                let next: Vec<Point> = match code {
                    // A segment's bend is signed by its direction (sh::segment), so a
                    // vertical mirror negates it and a time reversal keeps it.
                    SH_FLIP => pts.iter().map(|p| Point { y: 1.0 - p.y, tension: -p.tension, ..*p }).collect(),
                    SH_REVERSE => {
                        let n = pts.len();
                        (0..n)
                            .map(|k| {
                                let p = pts[n - 1 - k];
                                // A segment's bend belongs to its left point.
                                let tension = if k + 1 < n { pts[n - 2 - k].tension } else { 0.0 };
                                Point { x: 1.0 - p.x, y: p.y, tension }
                            })
                            .collect()
                    }
                    SH_CLEAR => {
                        let r = sh::REST[lane];
                        vec![Point { x: 0.0, y: r, tension: 0.0 }, Point { x: 1.0, y: r, tension: 0.0 }]
                    }
                    SH_COPY => {
                        self.devices.panel.editors.shape_clip = Some(pts);
                        return;
                    }
                    SH_PASTE => match self.devices.panel.editors.shape_clip.clone() {
                        Some(c) => c,
                        None => return,
                    },
                    c if (SH_GRID..SH_GRID + 100).contains(&c) => {
                        self.devices.panel.editors.grid.insert(device, (c - SH_GRID) as u32);
                        self.ui_dirty = true;
                        return;
                    }
                    SH_TRIGGER_SYNC => return self.set_many(device, "Trigger Change", vec![("trigger", 0.0)]),
                    SH_TRIGGER_SELF => {
                        let mut commands = vec![];
                        if d.sidechain.is_some() {
                            commands.push(Command::SetDeviceSidechain { device, source: None });
                        }
                        commands.push(Command::SetDeviceParam { device, param: "trigger".into(), value: 1.0 });
                        commands.push(Command::SetDeviceParam { device, param: "sidechain".into(), value: 0.0 });
                        self.panel_commit(Command::Batch { label: "Trigger Change".into(), commands });
                        return;
                    }
                    c if c >= SH_TRIGGER_TRACK => {
                        let Some(t) = self.project().tracks.get((c - SH_TRIGGER_TRACK) as usize).map(|t| t.id) else { return };
                        let commands = vec![
                            Command::SetDeviceSidechain { device, source: Some(t) },
                            Command::SetDeviceParam { device, param: "trigger".into(), value: 1.0 },
                            Command::SetDeviceParam { device, param: "sidechain".into(), value: 1.0 },
                        ];
                        self.panel_commit(Command::Batch { label: "Trigger Change".into(), commands });
                        return;
                    }
                    c if c >= SH_SHAPE => match sh::SHAPES[lane].get((c - SH_SHAPE) as usize) {
                        Some(s) => sh::shape_points(s),
                        None => return,
                    },
                    _ => return,
                };
                self.set_many(device, "Edit Shape", sh::lane_writes(lane, &next));
            }
            _ => {}
        }
    }
}
