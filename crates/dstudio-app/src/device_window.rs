//! Expanded device windows: the large view a built-in effect opens from the
//! arrow in its card header (ui/device_window.slint).
//!
//! A window is the same device as its card, laid out at window size: a top
//! bar (power and name, preset, header pill groups), a large display with the
//! device's own picture, In / Out meters beside it, and raised control cards
//! below. Everything is placed here in window-content points (the window kit's
//! 1x geometry, scaled by the panel zoom Z) and handed to Slint as flat lists
//! of boxes, pills, knobs and texts, so one Slint component draws every
//! device's window.
//!
//! Control cards follow the kit's gutter row: clusters of fixed width share
//! the card's free width as equal gutters (half a gutter at either end), with
//! a hairline rule in the middle of every inner gutter.

use crate::controller::Controller;
use crate::device_panel::{Z, s};
use crate::{PanelCanvas, PanelKnob, PanelText, PanelTick, WinBox, WinMeter, WinPill, WinView};
use dstudio_core::builtin::eq::{EqView, Shape};
use dstudio_core::builtin::{DeviceDef, View};
use dstudio_core::{Command, Device, DeviceId};
use slint::{Color, ModelRc, VecModel};
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Instant;

fn model<T: Clone + 'static>(v: Vec<T>) -> ModelRc<T> {
    ModelRc::from(Rc::new(VecModel::from(v)))
}

/// Window content width (pt).
pub(crate) const WIN_W: f32 = 960.0;
/// Content inset of the display row and the control cards.
const INSET: f32 = 16.0;
/// Width of the meter strip right of the display (and the gap before it).
const METERS_W: f32 = 136.0;
const METERS_GAP: f32 = 12.0;
/// Display plot margins (left ruler, right clearance, meter label block, axis + legend).
pub(crate) const PLOT_L: f32 = 48.0;
const PLOT_R: f32 = 16.0;
pub(crate) const PLOT_T: f32 = 22.0;
pub(crate) const PLOT_B: f32 = 40.0;
/// Meter scale: +12 dB at the top of the column, -50 dB at its foot, 0 dB between.
const METER_TOP_DB: f32 = 12.0;
const METER_MIN_DB: f32 = -50.0;
const METER_TICKS: [f32; 7] = [12.0, 6.0, 0.0, -12.0, -24.0, -36.0, -48.0];

const TEXT: Color = Color::from_rgb_u8(0xe5, 0xe5, 0xe8);

/// Pill kinds (ui/device_window.slint WinPillView).
const PILL_CHOICE: i32 = 0;
const PILL_BAND: i32 = 1;
const PILL_TOGGLE: i32 = 2;
const PILL_PICKER: i32 = 3;
const PILL_ACTION: i32 = 5;

/// Box kinds.
const BOX_CARD: i32 = 0;
const BOX_RULE: i32 = 1;
const BOX_TRACK: i32 = 2;
const BOX_HEADER_RULE: i32 = 3;
const BOX_DISPLAY: i32 = 4;
/// Left edge of the Trance Gate's envelope display and card (its ring panel is left of it).
const TG_DISPLAY_X: f32 = 304.0;

/// Window actions (Studio.win-action, MenuPick::Window).
pub(crate) const ACT_SECTION: i32 = 1;
pub(crate) const ACT_TAME_BAND: i32 = 2;
pub(crate) const ACT_BAND_ON: i32 = 3;
pub(crate) const ACT_LISTEN: i32 = 4;
pub(crate) const ACT_SELECT_BAND: i32 = 5;
/// Character EQ: the selected band's drive step (Off, Low, Medium, High).
pub(crate) const ACT_DRIVE: i32 = 6;
/// Band drive steps, in percent.
const DRIVE_STEPS: [f32; 4] = [0.0, 25.0, 50.0, 75.0];
/// The analyser's view (value: index into Pre, Post, Off).
pub(crate) const ACT_ANALYZER: i32 = 7;
/// The analyser's Hold view latch.
pub(crate) const ACT_HOLD: i32 = 8;
/// Character Mod: play the next take (seed).
pub(crate) const ACT_SEED: i32 = 9;
/// Dynamics windows: the history's length (value: index into 1 s, 4 s, 10 s).
pub(crate) const ACT_SPAN: i32 = 10;
/// Mastering Limiter: the loudness target (value: index into device_window_dynamics::TARGETS).
pub(crate) const ACT_TARGET: i32 = 11;
/// Mastering Limiter: restart the loudness measurement.
pub(crate) const ACT_LOUD_RESET: i32 = 12;
/// Multiband Compressor: the band the rack card edits (value: 0 Low, 1 Mid, 2 High).
pub(crate) const ACT_MB_BAND: i32 = 13;
/// Field devices (Delay, Echo, Reverb): the display's view (value: index into the card's views).
pub(crate) const ACT_VIEW: i32 = 14;
/// Delay: the repeat-EQ band the rack edits (value: 0..6).
pub(crate) const ACT_DELAY_BAND: i32 = 15;
/// Spectrum: clear the window's history.
pub(crate) const ACT_SPEC_RESET: i32 = 16;
const PILL_GHOST: i32 = 6;
/// Character Mod's Mode parameter of each circuit (Ensemble has none).
const CM_MODES: [Option<&str>; 11] = dstudio_core::builtin::character_mod::MODE_PARAMS;
const PILL_GHOST_LATCH: i32 = 7;
/// A momentary latch: on while held (Echo's Throw).
const PILL_MOMENTARY: i32 = 8;
/// panel-menu code of the band picker (a menu of the EQ's bands).
pub(crate) const MENU_BANDS: i32 = -3;
/// Panel-menu code of the analyser view picker (Character EQ).
pub(crate) const MENU_ANALYZER: i32 = -4;
/// Panel-menu code of the loudness target picker (Mastering Limiter).
pub(crate) const MENU_TARGET: i32 = -5;
/// Panel-menu code of the Multiband Compressor's band picker.
pub(crate) const MENU_MB_BAND: i32 = -6;
/// The Multiband Compressor's bands, as its band picker names them.
const MB_BANDS: [&str; 3] = ["Low", "Mid", "High"];
/// Height (units at twice the card's scale) of the Tuner window's picture: it puts the bar
/// across the middle of the display.
const TUNER_WIN_H: f32 = 194.0;
/// Width of the waveshaper and Bit Crusher windows.
const WAVE_W: f32 = 960.0;
/// Panel-menu code of the Delay's repeat-EQ band picker.
pub(crate) const MENU_DELAY_BAND: i32 = -7;

use crate::device_window_dynamics::TARGETS;

/// Character Compressor models (its `model` choices).
const CC_CLEAN: i32 = 0;
const CC_OPTO: i32 = 4;
const CC_RMS_VCA: i32 = 5;

/// Devices with an expanded window.
pub(crate) fn has_window(def: &DeviceDef) -> bool {
    matches!(
        def.id,
        "auto-filter" | "channeleq" | "character-eq" | "transient-shaper" | "multiband-compressor" | "delay" | "reverb" | "echo"
    ) || is_wave_window(def)
        || matches!(def.id, "utility" | "widener" | "tuner" | "spectrum" | "trance-gate" | "pitch-shifter" | "grains" | "pitch-corrector")
        || is_repair_window(def)
        || def.id == "bitcrusher"
        || is_field_window(def)
        || is_dynamics_window(def)
}

/// The restoration graphs drawn at window size (src/panel_graph.rs).
fn is_repair_window(def: &DeviceDef) -> bool {
    matches!(def.id, "deesser" | "dehummer" | "denoiser")
}

/// Windows of the level-history family (src/device_window_dynamics.rs).
fn is_dynamics_window(def: &DeviceDef) -> bool {
    crate::device_window_dynamics::spec(def.id).is_some()
}

/// Windows of the waveshaper family (src/device_window_wave.rs).
fn is_wave_window(def: &DeviceDef) -> bool {
    matches!(def.id, "distortion" | "tapesaturation" | "soft-clipper" | "declipper")
}

/// Windows of the plotted-field family (motion and response pictures).
fn is_field_window(def: &DeviceDef) -> bool {
    matches!(def.id, "tremolo" | "autopanner" | "chorus" | "flanger" | "phaser" | "ring-mod" | "character-mod")
}

/// The frame of a device's window: content width, display box (x, y, w, h), the
/// display's plot insets (left, top, right, bottom) and its meter strip.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Kit {
    pub w: f32,
    pub display: (f32, f32, f32, f32),
    pub plot: (f32, f32, f32, f32),
    /// The narrow strip (114 wide, columns of 54) of the field windows; else the EQs' 136.
    narrow: bool,
    /// Height of the meter strip when it runs past the display (0: the display's).
    meters_h: f32,
}

/// Field windows: a display 152 narrower than the window, a 12 pt gap to the control card.
fn field_kit(w: f32, display_h: f32, left: f32, bottom: f32) -> Kit {
    Kit { w, display: (INSET, 52.0, w - 152.0, display_h), plot: (left, 25.0, 8.0, bottom), narrow: true, meters_h: 0.0 }
}

impl Kit {
    fn with_plot_top(mut self, t: f32) -> Kit {
        self.plot.1 = t;
        self
    }
    fn with_plot_right(mut self, r: f32) -> Kit {
        self.plot.2 = r;
        self
    }
}

pub(crate) fn kit(def: &DeviceDef) -> Kit {
    let eq = |y: f32, h: f32| Kit {
        w: WIN_W,
        display: (INSET, y, WIN_W - 2.0 * INSET - METERS_W - METERS_GAP, h),
        plot: (PLOT_L, PLOT_T, PLOT_R, PLOT_B),
        narrow: false,
        meters_h: 0.0,
    };
    match def.id {
        "character-eq" => eq(52.0, 346.0),
        // Delay: the field fills the display under its caption band.
        "delay" => Kit { plot: (8.0, 25.0, 8.0, 8.0), ..eq(56.0, 450.0) },
        // Echo: the tape strip above a display of the tail.
        "echo" => Kit { plot: (8.0, 25.0, 8.0, 8.0), ..eq(160.0, 354.0) },
        // Reverb: a 1200 pt window; the tail picture fills the display.
        "reverb" => Kit { w: 1200.0, display: (INSET, 56.0, 1020.0, 510.0), plot: (8.0, 8.0, 8.0, 8.0), narrow: false, meters_h: 0.0 },
        // Transient Shaper: the scope fills the display; its legend sits on the caption row.
        "transient-shaper" => field_kit(880.0, 228.0, 8.0, 8.0),
        // Multiband: the band field over the dB ladder, its Hz axis and legend below.
        "multiband-compressor" => field_kit(WIN_W, 336.0, 52.0, 43.0),
        // Pitch Shifter: the semitone field over its axis, the formant strip and legend.
        "pitch-shifter" => field_kit(1200.0, 514.0, 12.0, 8.0),
        // Pitch Corrector: the trace fills the display.
        "pitch-corrector" => field_kit(1200.0, 514.0, 12.0, 8.0).with_plot_top(8.0),
        // Grains: the buffer over its time axis, the pitch axis down its right edge.
        "grains" => field_kit(1200.0, 402.0, 16.0, 24.0).with_plot_right(70.0),
        // Waveshapers: the transfer square and the wave over a dB axis and legend.
        "distortion" | "tapesaturation" | "soft-clipper" => field_kit(WAVE_W, 454.0, 60.0, 44.0),
        // De-Esser, De-Hummer, De-Noiser: the graph over its ladder, the axis and legend below
        // (its plot is crate::panel_graph::WIN_PLOT).
        "deesser" | "dehummer" | "denoiser" => field_kit(1200.0, 510.0, 46.0, 44.0),
        // De-Clipper: the same picture on the wider face.
        "declipper" => field_kit(1200.0, 510.0, 60.0, 44.0),
        // Utility: three cards and no display; the meters run the window's height.
        "utility" => Kit { w: 1200.0, display: (INSET, 52.0, 1042.0, 630.0), plot: (0.0, 0.0, 0.0, 0.0), narrow: true, meters_h: 0.0 },
        // Stereo Widener: the field over the correlation lane.
        // Spectrum: the analyser across the display, one Level meter beside it.
        "spectrum" => Kit { w: 1200.0, display: (INSET, 52.0, 1108.0, 514.0), plot: (36.0, 32.0, 42.0, 34.0), narrow: true, meters_h: 0.0 },
        // Trance Gate: the envelope across the display right of the ring panel; no meters.
        "trance-gate" => {
            Kit { w: 1200.0, display: (TG_DISPLAY_X, 56.0, 880.0, 516.0), plot: (0.0, 0.0, 0.0, 0.0), narrow: true, meters_h: 0.0 }
        }
        // Tuner: the bar across a tall display, one In meter beside it.
        "tuner" => Kit { w: 1200.0, display: (INSET, 52.0, 1108.0, 550.0), plot: (0.0, 0.0, 0.0, 0.0), narrow: true, meters_h: 0.0 },
        // Its meters run down past the card, as the Bit Crusher's.
        "widener" => Kit { w: 1200.0, display: (INSET, 52.0, 1042.0, 514.0), plot: (36.0, 25.0, 12.0, 8.0), narrow: true, meters_h: 630.0 },
        // Bit Crusher: one crushed cycle fills the display.
        "bitcrusher" => Kit { meters_h: 575.0, ..field_kit(WAVE_W, 454.0, 8.0, 8.0) },
        "tremolo" => field_kit(WIN_W, 234.0, 60.0, 44.0),
        "autopanner" => field_kit(WIN_W, 234.0, 40.0, 44.0),
        "chorus" | "flanger" => field_kit(WIN_W, 354.0, 56.0, 44.0),
        "phaser" => field_kit(WIN_W, 374.0, 56.0, 44.0),
        "ring-mod" => field_kit(860.0, 254.0, 40.0, 29.0),
        "character-mod" => field_kit(WIN_W, 222.0, 60.0, 44.0),
        // The level-history family: the plot's ladder in a 52 pt column, the caption band above.
        "compressor" | "limiter" | "gate" => field_kit(WIN_W, 394.0, 52.0, 6.0),
        "glue-compressor" => field_kit(WIN_W, 282.0, 52.0, 6.0),
        "character-compressor" | "mastering-limiter" => field_kit(WIN_W, 342.0, 52.0, 6.0),
        _ => eq(56.0, 394.0),
    }
}

/// Size of the display plot of a device's window (px at the panel zoom).
pub(crate) fn plot_size(def: &DeviceDef) -> Option<(f32, f32)> {
    has_window(def).then(|| {
        let k = kit(def);
        let (_, _, w, h) = k.display;
        ((w - k.plot.0 - k.plot.2) * Z, (h - k.plot.1 - k.plot.3) * Z)
    })
}

/// Window UI state that is not part of the device (not saved).
#[derive(Debug, Default)]
pub(crate) struct WinUi {
    /// Character EQ: section of the lower card (0 Band, 1 Dynamics, 2 Warmth, 3 Valve, 4 Tame).
    section: HashMap<DeviceId, usize>,
    /// Character EQ: tamer band shown in the Tame section.
    tame_band: HashMap<DeviceId, usize>,
    /// In / Out meters of open windows.
    meters: HashMap<DeviceId, Meters>,
    /// Analyser view per window (absent = Post).
    analyzer: HashMap<DeviceId, Analyzer>,
    /// Windows whose analyser frame is held (Hold view).
    held: std::collections::HashSet<DeviceId>,
    /// Analyser frames of open windows.
    spectra: HashMap<DeviceId, Spectrum>,
    clock: Option<Instant>,
    /// Level histories of the open dynamics windows, and the window each shows (s).
    history: HashMap<DeviceId, std::collections::VecDeque<crate::device_window_dynamics::Sample>>,
    span: HashMap<DeviceId, f32>,
    /// Mastering Limiter: loudness target per window, and where its measurement restarts (s).
    target: HashMap<DeviceId, usize>,
    loud_from: HashMap<DeviceId, f64>,
    /// Start of the histories' clock.
    epoch: Option<Instant>,
    /// Stereo Widener: input and output correlation of the open windows.
    pub(crate) corr: HashMap<DeviceId, std::collections::VecDeque<crate::device_window_widener::CorrSample>>,
    /// Spectrum: the frames each open window keeps for its 3D and Split views.
    pub(crate) spec: HashMap<DeviceId, crate::device_window_spectrum::SpecHistory>,
}

/// Which signal a window's analyser shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum Analyzer {
    Pre,
    #[default]
    Post,
    Off,
}

impl Analyzer {
    const ALL: [Analyzer; 3] = [Analyzer::Pre, Analyzer::Post, Analyzer::Off];

    fn label(self) -> &'static str {
        match self {
            Analyzer::Pre => "Pre",
            Analyzer::Post => "Post",
            Analyzer::Off => "Off",
        }
    }

    /// The engine's analyser mode (0 off, 1 input, 2 output).
    fn engine_mode(self) -> u8 {
        match self {
            Analyzer::Pre => 1,
            Analyzer::Post => 2,
            Analyzer::Off => 0,
        }
    }
}

/// The level of an analyser bin with nothing in it (dBFS).
pub(crate) const SPECTRUM_FLOOR_DB: f32 = -90.0;

/// The frame a window's analyser draws: follows the engine's newest frame with an attack
/// and release, falls to the floor at a fixed rate once frames stop, stands still while held.
#[derive(Debug, Clone, Default)]
struct Spectrum {
    shown: Vec<f32>,
    target: Vec<f32>,
    seq: u64,
    /// Seconds since the last new frame.
    since_frame: f32,
}

impl Spectrum {
    const ATTACK: f32 = 0.045;
    const RELEASE: f32 = 0.190;
    const DECAY_DB: f32 = 42.0;
    /// Frames arrive every ~20 ms while audio runs; longer than this without one is silence.
    const STALE: f32 = 0.15;

    /// One poll: `frame` is a new engine frame (if any), `dt` seconds since the last poll.
    /// Returns whether the shown frame moved.
    fn advance(&mut self, frame: Option<&[f32]>, dt: f32, held: bool) -> bool {
        if let Some(f) = frame {
            self.target.clear();
            self.target.extend(f.iter().map(|v| if v.is_finite() { v.max(SPECTRUM_FLOOR_DB) } else { SPECTRUM_FLOOR_DB }));
            if self.shown.len() != self.target.len() {
                self.shown = vec![SPECTRUM_FLOOR_DB; self.target.len()];
            }
            self.since_frame = 0.0;
        } else {
            self.since_frame += dt;
        }
        if held {
            return false;
        }
        let mut moved = false;
        if self.since_frame < Self::STALE {
            let (ka, kr) = (1.0 - (-dt / Self::ATTACK).exp(), 1.0 - (-dt / Self::RELEASE).exp());
            for (s, t) in self.shown.iter_mut().zip(&self.target) {
                let next = *s + (t - *s) * if *t > *s { ka } else { kr };
                moved |= (next - *s).abs() > 0.01;
                *s = next;
            }
        } else {
            let step = Self::DECAY_DB * dt;
            for s in &mut self.shown {
                if *s > SPECTRUM_FLOOR_DB {
                    *s = (*s - step).max(SPECTRUM_FLOOR_DB);
                    moved = true;
                }
            }
        }
        moved
    }

    /// Whether anything is above the floor (the wash, its legend entry and the held badge
    /// show only then).
    fn on_screen(&self) -> bool {
        self.shown.iter().any(|v| *v > SPECTRUM_FLOOR_DB)
    }
}

/// Ballistics of one window's In / Out meters (linear amplitude), per channel.
#[derive(Debug, Clone)]
struct Meters {
    /// [in L, in R, out L, out R]
    level: [f32; 4],
    hold: [f32; 4],
    /// Seconds since each hold last rose.
    held_for: [f32; 4],
    /// Seconds since a channel of the pair last went over full scale (in, out).
    since_clip: [f32; 2],
}

impl Meters {
    const ATTACK: f32 = 0.005;
    const RELEASE: f32 = 0.120;
    const HOLD: f32 = 1.5;
    const DECAY_DB: f32 = 20.0;
    const CLIP_HOLD: f32 = 0.25;

    fn new() -> Self {
        Meters { level: [0.0; 4], hold: [0.0; 4], held_for: [0.0; 4], since_clip: [f32::INFINITY; 2] }
    }

    /// One poll: raw peaks since the last one, `dt` seconds later.
    fn push(&mut self, raw: [f32; 4], dt: f32) {
        let dt = if dt.is_finite() && dt > 0.0 { dt.min(0.1) } else { 0.0 };
        let (att, rel) = (1.0 - (-dt / Self::ATTACK).exp(), 1.0 - (-dt / Self::RELEASE).exp());
        for (i, &raw) in raw.iter().enumerate() {
            let r = if raw.is_finite() { raw.max(0.0) } else { 0.0 };
            let c = if r > self.level[i] { att } else { rel };
            self.level[i] += (r - self.level[i]) * c;
            if self.level[i] < 1e-4 {
                self.level[i] = 0.0;
            }
            if r >= self.hold[i] {
                self.hold[i] = r;
                self.held_for[i] = 0.0;
            } else {
                self.held_for[i] += dt;
                if self.held_for[i] > Self::HOLD && self.hold[i] > 0.0 {
                    let db = 20.0 * self.hold[i].log10() - Self::DECAY_DB * dt;
                    self.hold[i] = if db <= METER_MIN_DB { 0.0 } else { 10f32.powf(db / 20.0) };
                }
            }
        }
        for (k, since) in self.since_clip.iter_mut().enumerate() {
            if raw[2 * k] > 1.0 || raw[2 * k + 1] > 1.0 {
                *since = 0.0;
            } else {
                *since += dt;
            }
        }
    }
}

/// The drive step a band's drive (percent) sits on: 0 Off, 1 Low, 2 Medium, 3 High;
/// `usize::MAX` (no pill lit) for a value between steps, as a preset can hold.
fn drive_step(pct: f32) -> usize {
    DRIVE_STEPS.iter().position(|s| (s - pct).abs() < 0.01).unwrap_or(usize::MAX)
}

/// Position of a level on the meter's -50..0 dB column (0 = foot, 1 = 0 dB).
fn meter_norm(lin: f32) -> f32 {
    if lin <= 0.0 {
        return 0.0;
    }
    ((20.0 * lin.log10() - METER_MIN_DB) / -METER_MIN_DB).clamp(0.0, 1.0)
}

/// Readout under a meter: the held peak of the louder channel.
fn meter_text(lin: f32) -> String {
    if lin <= 10f32.powf(METER_MIN_DB / 20.0) {
        return "\u{2212}\u{221e} dB".into();
    }
    let db = 20.0 * lin.log10();
    let t = format!("{:.1} dB", db.abs());
    if db < -0.05 {
        format!("\u{2212}{t}")
    } else if db > 0.05 {
        format!("+{t}")
    } else {
        "0.0 dB".into()
    }
}

/// Places clusters of the given widths in a card [x, x + w] with equal gutters.
/// Returns each cluster's left edge and the x of every inner rule.
fn gutter(x: f32, w: f32, widths: &[f32]) -> (Vec<f32>, Vec<f32>) {
    let n = widths.len().max(1) as f32;
    let gap = ((w - widths.iter().sum::<f32>()) / n).max(0.0);
    let mut at = x + gap / 2.0;
    let mut xs = Vec::new();
    let mut rules = Vec::new();
    for (i, cw) in widths.iter().enumerate() {
        if i > 0 {
            rules.push(at - gap / 2.0);
        }
        xs.push(at);
        at += cw + gap;
    }
    (xs, rules)
}

/// Width of a pill holding `label` (12 pt medium text plus 12 on either side). The
/// window kit's own labels are measured; anything else is estimated from the
/// card's value face.
fn pill_w(label: &str) -> f32 {
    match label {
        "12 dB/oct" => 76.5,
        "24 dB/oct" => 79.2,
        "6 dB/oct" => 71.8,
        "LP" => 38.3,
        "HP" => 40.4,
        "BP" => 39.3,
        "Notch" => 57.8,
        "DJ" => 39.3,
        "Free" => 48.8,
        "Sync" => 52.3,
        "1" => 28.9,
        "2" | "5" => 31.3,
        "3" | "8" => 31.4,
        "4" => 31.8,
        "6" | "9" => 31.5,
        "7" => 30.8,
        "10" => 36.5,
        "11" => 33.7,
        "12" => 36.1,
        "Band" => 52.9,
        "Dynamics" => 79.7,
        "Warmth" => 68.3,
        "Valve" => 54.9,
        "Tame" => 55.0,
        "Off" | "On" => 41.5,
        "2x" => 37.8,
        "Lo" => 37.8,
        "Hi" => 35.7,
        "4x" => 38.3,
        _ => (crate::text_measure::value_width(label) * 1.2 + 24.0).ceil(),
    }
}

/// A window being built: everything in content points until `finish` scales it.
struct Builder {
    accent: Color,
    /// Window content width.
    w: f32,
    /// Left edge of the control cards (INSET; the Trance Gate's start right of its ring panel).
    left: f32,
    boxes: Vec<WinBox>,
    pills: Vec<WinPill>,
    knobs: Vec<PanelKnob>,
    texts: Vec<PanelText>,
    square: PanelCanvas,
}

fn accent_of(b: &Builder) -> Color {
    b.accent
}

impl Builder {
    fn new(accent: Color, w: f32) -> Self {
        Builder {
            accent,
            w,
            left: INSET,
            boxes: Vec::new(),
            pills: Vec::new(),
            knobs: Vec::new(),
            texts: Vec::new(),
            square: PanelCanvas::default(),
        }
    }

    fn rect(&mut self, kind: i32, x: f32, y: f32, w: f32, h: f32) {
        self.boxes.push(WinBox { kind, x: x * Z, y: y * Z, w: w * Z, h: h * Z });
    }

    /// Caption centred under a control column (the knob name line).
    fn name(&mut self, x: f32, w: f32, y: f32, text: &str, dim: bool) {
        self.texts.push(PanelText {
            x: (x - 8.0) * Z,
            y: y * Z,
            w: (w + 16.0) * Z,
            h: 16.0 * Z,
            text: s(text),
            color: if dim { Color::from_argb_u8(0x8c, 0xe5, 0xe5, 0xe8) } else { TEXT },
            size: 12.0 * Z,
            weight: 600,
            align: 1,
        });
    }

    #[allow(clippy::too_many_arguments)]
    fn pill(&mut self, kind: i32, param: i32, index: i32, x: f32, y: f32, w: f32, label: &str, active: bool) -> &mut WinPill {
        self.pills.push(WinPill { kind, param, index, x: x * Z, y: y * Z, w: w * Z, label: s(label), active, ..Default::default() });
        self.pills.last_mut().expect("just pushed")
    }

    /// A pill row on its track: `items` are (label, active, index); returns the row width.
    /// gap: space between pills (the header's groups use 8, card rows 2).
    #[allow(clippy::too_many_arguments)]
    fn pill_row(&mut self, kind: i32, param: i32, x: f32, y: f32, items: &[(String, bool, i32)], gap: f32, track: bool) -> f32 {
        let w: f32 = items.iter().map(|(l, _, _)| pill_w(l)).sum::<f32>() + gap * (items.len().saturating_sub(1)) as f32 + 4.0;
        if track {
            self.rect(BOX_TRACK, x, y, w, 28.0);
        }
        let mut at = x + 2.0;
        for (label, active, index) in items {
            let pw = pill_w(label);
            self.pill(kind, param, *index, at, y + 2.0, pw, label, *active);
            at += pw + gap;
        }
        w
    }

    /// Width of a pill row without placing it.
    fn row_w(labels: &[&str], gap: f32) -> f32 {
        labels.iter().map(|l| pill_w(l)).sum::<f32>() + gap * (labels.len().saturating_sub(1)) as f32 + 4.0
    }
}

/// Vertical metrics of a control card row: card height, dial centre, value line and the
/// pill line of a control column (all from the cluster top).
#[derive(Clone, Copy)]
struct Row {
    y: f32,
    h: f32,
    dial: f32,
    value: f32,
    pill: f32,
    /// Picker of a two-line picker field (pill row above it at `pill - 16`).
    picker: f32,
}

impl Row {
    /// The main card: 106 high, dials centred 28 below the cluster top.
    fn main(y: f32) -> Row {
        Row { y, h: 106.0, dial: 28.0, value: 54.0, pill: 30.0, picker: 46.0 }
    }
    /// A tall card (Transient Shaper's, around its 104 pt picture): 132 high.
    fn tall(y: f32) -> Row {
        Row { y, h: 132.0, dial: 54.0, value: 80.0, pill: 56.0, picker: 72.0 }
    }
    /// The rack card (Character EQ's sections): 100 high.
    fn rack(y: f32) -> Row {
        Row { y, h: 100.0, dial: 22.0, value: 48.0, pill: 22.0, picker: 40.0 }
    }
    fn top(&self) -> f32 {
        self.y + 10.0
    }
    fn name_y(&self) -> f32 {
        self.top() + self.value + 16.0
    }
}

/// One placed item of a cluster.
enum Item {
    /// Knob: parameter, name, size (1 sm, 2 md), cell width, inert.
    Knob(&'static str, &'static str, i32, f32, bool),
    /// On / off toggle column (56 wide): parameter, name, inert.
    Toggle(&'static str, &'static str, bool),
    /// On / off toggle column of a given width: parameter, name, width.
    ToggleW(&'static str, &'static str, f32),
    /// One lit pill that runs a window action: action, label, name, width.
    Action(i32, String, &'static str, f32),
    /// Picker column: parameter (or a menu code), label, name, width, inert.
    Picker(i32, String, &'static str, f32, bool),
    /// Pill row column: parameter, labels with their values, name, width.
    Pills(&'static str, Vec<(&'static str, f32)>, &'static str, f32),
    /// Pill row of window actions: action, labels, selected index, name, width, inert.
    ActionPills(i32, Vec<&'static str>, usize, &'static str, f32, bool),
    /// One unlit pill that runs a window action: action, label, name, width.
    Button(i32, &'static str, &'static str, f32),
    /// A latch over a menu picker (Delay's band column): latch parameter, its label and
    /// state, the menu code, the picker's label, name, width.
    LatchPicker(i32, String, bool, i32, String, &'static str, f32),
    /// Momentary latch column (on while held): parameter, label, name, width.
    Momentary(&'static str, &'static str, &'static str, f32),
    /// Two choice pickers stacked in one column: (parameter, label) of each, name, width, inert.
    Pickers2([(i32, String); 2], &'static str, f32, bool),
    /// A latch pill with a fixed label (lit while on): parameter, label, name, width, inert.
    Latch(&'static str, &'static str, &'static str, f32, bool),
    /// Fixed blank (keeps a column's width).
    Blank(f32),
}

impl Item {
    fn width(&self) -> f32 {
        match self {
            Item::Knob(_, _, _, w, _) => *w,
            Item::Toggle(..) => 56.0,
            Item::ToggleW(.., w) => *w,
            Item::Action(.., w) => *w,
            Item::Button(.., w) => *w,
            Item::Picker(_, _, _, w, _) => *w,
            Item::Pills(.., w) => *w,
            Item::ActionPills(_, _, _, _, w, _) => *w,
            Item::LatchPicker(.., w) => *w,
            Item::Pickers2(_, _, w, _) => *w,
            Item::Momentary(.., w) => *w,
            Item::Latch(.., w, _) => *w,
            Item::Blank(w) => *w,
        }
    }
}

impl Controller {
    /// The expanded window of a built-in device (None when the device has none).
    pub(crate) fn window_view(&self, d: &Device, def: &'static DeviceDef, accent: Color) -> Option<WinView> {
        if !has_window(def) {
            return None;
        }
        let k = kit(def);
        let mut b = Builder::new(accent, k.w);
        let (dx, dy, dw, dh) = k.display;
        if def.id != "utility" {
            b.rect(BOX_DISPLAY, dx, dy, dw, dh);
        }
        let mut display = match def.layout.view {
            Some(View::Eq(v)) => self.eq_window_field(d, def, v, accent),
            Some(View::Custom("auto-filter")) => self.auto_filter_window_field(d, def, accent),
            Some(View::Field(f)) => self.field_window_canvas(d, def, f, &k, accent),
            _ if is_dynamics_window(def) => self.dynamics_window_canvas(d, def, &k, accent),
            _ if def.id == "transient-shaper" => self.shaper_window_canvas(d, def, &k, accent),
            _ if def.id == "multiband-compressor" => self.multiband_window_canvas(d, def, &k, accent),
            _ if is_wave_window(def) => self.wave_window_canvas(d, def, &k, accent),
            _ if def.id == "bitcrusher" => self.crush_window_canvas(d, def, &k, accent),
            _ if def.id == "widener" => self.widener_window_canvas(d, def, &k, accent),
            _ if def.id == "spectrum" => self.spectrum_window_canvas(d, def, &k, accent),
            _ if def.id == "pitch-corrector" => {
                let (w, h) = plot_size(def).unwrap_or_default();
                self.trace_window_canvas(d, def, w, h, accent)
            }
            _ if is_repair_window(def) => self.graph_window(d, def, accent),
            // The ring panel and the envelope display share one canvas from the panel's corner.
            _ if def.id == "trance-gate" => self.trance_gate_window_canvas(d, def, accent, dx + dw - INSET, dh),
            // The card's tuner at twice its scale, its bar across the middle of the display.
            _ if def.id == "tuner" => {
                PanelCanvas { well: false, ..self.tuner_canvas_at(d, def, k.display.2 / 2.0, TUNER_WIN_H, 2.0 * Z, accent) }
            }
            Some(View::Custom(_)) if crate::panel_fields::field_name(def).is_some() => self.field_window_display(d, def, &k),
            _ => PanelCanvas::default(),
        };
        display.x = (dx + k.plot.0) * Z;
        display.y = (dy + k.plot.1) * Z;
        if def.id == "trance-gate" {
            display.x = INSET * Z;
        }
        let height = match def.id {
            "auto-filter" => self.auto_filter_window(d, def, &mut b),
            "channeleq" => self.channel_eq_window(d, def, &mut b),
            "character-eq" => self.character_eq_window(d, def, &mut b),
            _ if is_dynamics_window(def) => self.dynamics_window(d, def, &mut b, dy + dh + 12.0),
            "transient-shaper" => self.shaper_window(d, def, &mut b, dy + dh + 12.0),
            "multiband-compressor" => self.multiband_window(d, def, &mut b, dy + dh + 12.0),
            "delay" => self.delay_window(d, def, &mut b, dy + dh + 12.0),
            "reverb" => self.reverb_window(d, def, &mut b, dy + dh + 12.0),
            "echo" => self.echo_window(d, def, &mut b, dy + dh + 12.0),
            "pitch-shifter" => self.pitch_shifter_window(d, def, &mut b, dy + dh + 12.0),
            "pitch-corrector" => self.pitch_corrector_window(d, def, &mut b, dy + dh + 12.0),
            "grains" => self.grains_window(d, def, &mut b, dy + dh + 12.0),
            _ if is_wave_window(def) || def.id == "bitcrusher" => self.wave_window(d, def, &mut b, dy + dh + 12.0),
            "utility" => self.utility_window(d, def, &mut b),
            "widener" => self.widener_window(d, def, &mut b, dy + dh + 12.0),
            "tuner" => self.tuner_window(d, def, &mut b, dy + dh + 14.0),
            "spectrum" => self.spectrum_window(d, def, &mut b, dy + dh + 12.0),
            "trance-gate" => self.trance_gate_window(d, def, &mut b, dy + dh + 12.0),
            _ if is_repair_window(def) => self.repair_window(d, def, &mut b, dy + dh + 12.0),
            _ => self.field_window(d, def, &mut b, dy + dh + 12.0),
        };
        let (mut meters, mut ticks, mut ticks_x) = self.window_meters(d.id, &k);
        if def.id == "trance-gate" {
            meters.clear();
            ticks.clear();
        }
        // The Tuner meters its input only; the Spectrum its level.
        if matches!(def.id, "tuner" | "spectrum") {
            meters.truncate(1);
            ticks_x = meters[0].x / Z + 38.0;
            if def.id == "spectrum" {
                meters[0].label = s("Level");
            }
        }
        Some(WinView {
            visible: true,
            w: k.w * Z,
            h: height * Z,
            title: s(d.display_name()),
            display,
            square: b.square,
            boxes: model(b.boxes),
            pills: model(b.pills),
            knobs: model(b.knobs),
            texts: model(b.texts),
            meters: model(meters),
            ticks: model(ticks),
            ticks_x: ticks_x * Z,
        })
    }

    /// In / Out meter columns in the strip right of the display, and the x of the ruler's
    /// numerals (content points). The EQs' strip is 136 wide with each bar under its label;
    /// the field windows' is 114, two 54 pt columns with the bar at the left of each.
    fn window_meters(&self, id: DeviceId, k: &Kit) -> (Vec<WinMeter>, Vec<PanelTick>, f32) {
        let m = self.devices.wins.meters.get(&id).cloned().unwrap_or_else(Meters::new);
        let (dx, y, dw, dh) = k.display;
        let h = if k.meters_h > 0.0 { k.meters_h } else { dh };
        let (top, bar_h) = if k.narrow { (y + 26.2, h - 48.2) } else { (y + 22.0, h - 44.0) };
        let x = dx + dw + if k.narrow { 6.0 } else { METERS_GAP };
        // (bar x, label centre) of each column.
        let cols = if k.narrow { [(x + 8.0, x + 27.0), (x + 68.0, x + 87.0)] } else { [(x + 19.0, x + 27.0), (x + 85.0, x + 93.0)] };
        let ticks_x = if k.narrow { x + 93.0 } else { x + 126.0 };
        let span = METER_TOP_DB - METER_MIN_DB;
        let zero_h = bar_h * METER_TOP_DB / span;
        let meters = [("In", 0usize, cols[0]), ("Out", 1, cols[1])]
            .into_iter()
            .map(|(label, k, (mx, lx))| WinMeter {
                label_x: lx * Z,
                x: mx * Z,
                y: top * Z,
                w: 16.0 * Z,
                h: bar_h * Z,
                zero_h: zero_h * Z,
                l: meter_norm(m.level[2 * k]),
                r: meter_norm(m.level[2 * k + 1]),
                hold_l: meter_norm(m.hold[2 * k]),
                hold_r: meter_norm(m.hold[2 * k + 1]),
                clip: m.since_clip[k] < Meters::CLIP_HOLD,
                label: s(label),
                value: s(meter_text(m.hold[2 * k].max(m.hold[2 * k + 1]))),
            })
            .collect();
        let ticks = METER_TICKS
            .iter()
            .map(|db| PanelTick {
                y: bar_h * (METER_TOP_DB - db) / span * Z,
                text: s(if *db > 0.0 {
                    format!("+{db}")
                } else if *db < 0.0 {
                    format!("\u{2212}{}", -db)
                } else {
                    "0".into()
                }),
                rule: true,
            })
            .collect();
        (meters, ticks, ticks_x)
    }

    /// Engine peaks -> meter ballistics of every open window (each poll tick).
    /// The analyser frame a window draws (dBFS, log spaced 20 Hz .. 20 kHz) and whether it is
    /// held; None while the analyser is off or has nothing above its floor.
    pub(crate) fn window_spectrum(&self, id: DeviceId) -> Option<(&[f32], bool)> {
        if self.devices.wins.analyzer.get(&id).copied().unwrap_or_default() == Analyzer::Off {
            return None;
        }
        let sp = self.devices.wins.spectra.get(&id)?;
        sp.on_screen().then(|| (sp.shown.as_slice(), self.devices.wins.held.contains(&id)))
    }

    pub(crate) fn poll_window_meters(&mut self) {
        self.poll_window_meters_at(Instant::now());
    }

    /// One meter and analyser poll at `now` (reviews step a clock of their own).
    pub(crate) fn poll_window_meters_at(&mut self, now: Instant) {
        let dt = self.devices.wins.clock.map_or(1.0 / 60.0, |t| now.duration_since(t).as_secs_f32());
        self.devices.wins.clock = Some(now);
        let open: Vec<DeviceId> = self
            .devices
            .windows
            .iter()
            .copied()
            .filter(|id| self.project().device(*id).and_then(dstudio_core::builtin::def_of).is_some_and(has_window))
            .collect();
        self.devices.wins.meters.retain(|id, _| open.contains(id));
        self.devices.wins.history.retain(|id, _| open.contains(id));
        self.devices.wins.corr.retain(|id, _| open.contains(id));
        self.devices.wins.spec.retain(|id, _| open.contains(id));
        for &id in &open {
            let Some(d) = self.project().device(id) else { continue };
            let Some(def) = dstudio_core::builtin::def_of(d).filter(|def| def.id == "spectrum") else { continue };
            if self.devices.wins.held.contains(&id) {
                continue;
            }
            let depth = def.param("history_depth").map_or(3, |p| self.panel_value(d, def, p).round() as usize);
            let depth = crate::device_window_spectrum::DEPTHS[depth.min(crate::device_window_spectrum::DEPTHS.len() - 1)];
            let Some((seq, shown, _)) = self.spectrum_frame(id) else { continue };
            let shown = shown.to_vec();
            if self.devices.wins.spec.entry(id).or_default().take(seq, &shown, depth) {
                self.ui_dirty = true;
            }
        }
        let epoch = *self.devices.wins.epoch.get_or_insert(now);
        let t = now.duration_since(epoch).as_secs_f64();
        let mut out = Vec::new();
        for &id in &open {
            let Some(sp) = self
                .project()
                .device(id)
                .and_then(dstudio_core::builtin::def_of)
                .and_then(|def| crate::device_window_dynamics::spec(def.id))
            else {
                continue;
            };
            if self.devices.wins.held.contains(&id) {
                continue;
            }
            let (i, o, g) = match self.engine.builtin_readouts(id.0, &mut out) {
                Ok(()) if out.len() >= 3 => (out[0], out[1], out[2].min(0.0)),
                _ => (-120.0, -120.0, 0.0),
            };
            let h = self.devices.wins.history.entry(id).or_default();
            if crate::device_window_dynamics::push(h, (t, i, o, g), sp.bottom) {
                self.ui_dirty = true;
            }
        }
        self.devices.wins.spectra.retain(|id, _| open.contains(id));
        let mut frame = Vec::new();
        for &id in &open {
            let view = self.devices.wins.analyzer.get(&id).copied().unwrap_or_default();
            // Set every tick: a rebuilt instance starts with its analyser off.
            let _ = self.engine.builtin_set_spectrum(id.0, view.engine_mode());
            let held = self.devices.wins.held.contains(&id);
            let sp = self.devices.wins.spectra.entry(id).or_default();
            let fresh = match self.engine.builtin_spectrum(id.0, &mut frame) {
                Ok(seq) if seq != 0 && seq != sp.seq && !frame.is_empty() && view != Analyzer::Off => {
                    sp.seq = seq;
                    Some(frame.as_slice())
                }
                _ => None,
            };
            if sp.advance(fresh, dt, held) {
                self.ui_dirty = true;
            }
        }
        for id in open {
            // Set every tick: a rebuilt instance starts with metering off.
            let _ = self.engine.builtin_set_metering(id.0, true);
            let mut raw = [0.0f32; 4];
            if self.engine.builtin_levels(id.0, &mut raw).is_err() {
                raw = [0.0; 4];
            }
            if self.project().device(id).and_then(dstudio_core::builtin::def_of).is_some_and(|def| def.id == "widener")
                && !self.devices.wins.held.contains(&id)
            {
                let mut c = [2.0f32; 2];
                if self.engine.builtin_correlation(id.0, &mut c).is_err() {
                    c = [2.0; 2];
                }
                // Both sides empty: no block ran since the last poll, or silence; the lane
                // breaks across the gap.
                if c[0] <= 1.0 || c[1] <= 1.0 {
                    crate::device_window_widener::push_corr(self.devices.wins.corr.entry(id).or_default(), (t, c[0], c[1]));
                    self.ui_dirty = true;
                }
            }
            let m = self.devices.wins.meters.entry(id).or_insert_with(Meters::new);
            let before = (m.level, m.hold, m.since_clip[0] < Meters::CLIP_HOLD, m.since_clip[1] < Meters::CLIP_HOLD);
            m.push(raw, dt);
            let after = (m.level, m.hold, m.since_clip[0] < Meters::CLIP_HOLD, m.since_clip[1] < Meters::CLIP_HOLD);
            if before != after {
                self.ui_dirty = true;
            }
        }
    }

    // ------------------------------------------------------------ controls

    /// A window knob: cell [x, x + w], dial centred on the row's dial line.
    #[allow(clippy::too_many_arguments)]
    fn win_knob(
        &self,
        d: &Device,
        def: &'static DeviceDef,
        b: &mut Builder,
        row: Row,
        id: &str,
        name: &str,
        size: i32,
        x: f32,
        w: f32,
        inert: bool,
    ) {
        let Some(p) = def.param(id) else { return };
        let Some(index) = def.index_of(id) else { return };
        let v = self.panel_value(d, def, p);
        let n = p.to_normalized(v);
        let live = self.live_device_of(d, def);
        let mut value = def.text(live.as_ref(), p, v);
        // Auto Filter's DJ position reads as the shape and corner it resolves to.
        if p.id == "dj_position" {
            value = self.af_corner_text_of(d, def).1;
        }
        // Overshoot only acts on corners: on any other shape it reads as having none.
        if inert && name == "Overshoot" {
            value = "None".into();
        }
        let mut k = PanelKnob {
            param: index as i32,
            x: x * Z,
            y: (row.top() + row.dial) * Z,
            label: s(name),
            value: s(value),
            norm: if p.reversed { 1.0 - n } else { n },
            origin: p.origin(),
            cell: w * Z,
            ticks: s(crate::device_panel::detent_ticks(p, def.detents(live.as_ref(), p))),
            inert: inert || def.is_inert(d, id),
            size,
            label_y: (row.top() + row.value) * Z,
            ..Default::default()
        };
        if let Some((lo, hi, now, color)) = self.mod_reach_of(d.id, p.id, k.norm) {
            k.mod_on = true;
            k.mod_lo = lo;
            k.mod_hi = hi;
            k.mod_now = now;
            k.mod_color = color;
        }
        b.knobs.push(k);
    }

    /// A row of clusters in a card: places every item, the card and its rules.
    fn win_card(&self, d: &Device, def: &'static DeviceDef, b: &mut Builder, row: Row, clusters: Vec<(Vec<Item>, f32)>) {
        b.rect(BOX_CARD, b.left, row.y, b.w - b.left - INSET, row.h);
        let widths: Vec<f32> = clusters
            .iter()
            .map(|(items, gap)| items.iter().map(Item::width).sum::<f32>() + gap * (items.len().saturating_sub(1)) as f32)
            .collect();
        let (xs, rules) = gutter(b.left, b.w - b.left - INSET, &widths);
        let rule_h = row.h - 24.0;
        for rx in rules {
            b.rect(BOX_RULE, rx, row.y + 12.0, 1.0, rule_h);
        }
        let idx = |id: &str| def.index_of(id).map_or(-1, |i| i as i32);
        let val = |id: &str| def.param(id).map_or(0.0, |p| self.panel_value(d, def, p));
        let top = row.top();
        for ((items, gap), cx) in clusters.into_iter().zip(xs) {
            let mut x = cx;
            for it in items {
                let w = it.width();
                match it {
                    Item::Knob(id, name, size, w, inert) => self.win_knob(d, def, b, row, id, name, size, x, w, inert),
                    Item::Toggle(id, name, inert) => {
                        let on = val(id) >= 0.5;
                        let label = if on { "On" } else { "Off" };
                        let pw = pill_w(label);
                        b.rect(BOX_TRACK, x + (56.0 - pw - 4.0) / 2.0, top + row.pill - 2.0, pw + 4.0, 28.0);
                        let p = b.pill(PILL_TOGGLE, idx(id), 0, x + (56.0 - pw) / 2.0, top + row.pill, pw, label, on);
                        p.inert = inert;
                        b.name(x, 56.0, row.name_y(), name, inert);
                    }
                    Item::ToggleW(id, name, w) => {
                        let on = val(id) >= 0.5;
                        let label = if on { "On" } else { "Off" };
                        let pw = pill_w(label);
                        b.rect(BOX_TRACK, x + (w - pw - 4.0) / 2.0, top + row.pill - 2.0, pw + 4.0, 28.0);
                        b.pill(PILL_TOGGLE, idx(id), 0, x + (w - pw) / 2.0, top + row.pill, pw, label, on);
                        b.name(x, w, row.name_y(), name, false);
                    }
                    Item::Action(action, label, name, w) => {
                        let pw = pill_w(&label);
                        b.pill(PILL_ACTION, action, 1, x + (w - pw) / 2.0, top + row.pill, pw, &label, true);
                        b.name(x, w, row.name_y(), name, false);
                    }
                    Item::Button(action, label, name, w) => {
                        let pw = pill_w(label);
                        b.rect(BOX_TRACK, x + (w - pw - 4.0) / 2.0, top + row.pill - 2.0, pw + 4.0, 28.0);
                        b.pill(PILL_ACTION, action, 1, x + (w - pw) / 2.0, top + row.pill, pw, label, false);
                        b.name(x, w, row.name_y(), name, false);
                    }
                    Item::Picker(param, label, name, w, inert) => {
                        let p = b.pill(PILL_PICKER, param, 0, x + w / 2.0, top + row.pill, 0.0, &label, false);
                        p.inert = inert;
                        b.name(x, w, row.name_y(), name, inert);
                    }
                    Item::Pills(id, labels, name, w) => {
                        let cur = val(id);
                        let items: Vec<(String, bool, i32)> =
                            labels.iter().enumerate().map(|(i, (l, v))| (l.to_string(), (cur - v).abs() < 0.5, i as i32)).collect();
                        let rw = Builder::row_w(&labels.iter().map(|(l, _)| *l).collect::<Vec<_>>(), 2.0);
                        b.pill_row(PILL_CHOICE, idx(id), x + (w - rw) / 2.0, top + row.pill - 2.0, &items, 2.0, true);
                        b.name(x, w, row.name_y(), name, false);
                    }
                    Item::ActionPills(action, labels, sel, name, w, inert) => {
                        let items: Vec<(String, bool, i32)> =
                            labels.iter().enumerate().map(|(i, l)| (l.to_string(), i == sel, i as i32)).collect();
                        let rw = Builder::row_w(&labels, 2.0);
                        let from = b.pills.len();
                        b.pill_row(PILL_ACTION, action, x + (w - rw) / 2.0, top + row.pill - 2.0, &items, 2.0, true);
                        for p in &mut b.pills[from..] {
                            p.inert = inert;
                        }
                        b.name(x, w, row.name_y(), name, inert);
                    }
                    Item::LatchPicker(param, latch, on, menu, label, name, w) => {
                        let pw = pill_w(&latch);
                        b.rect(BOX_TRACK, x + (w - pw - 4.0) / 2.0, top + 12.0, pw + 4.0, 28.0);
                        b.pill(PILL_TOGGLE, param, 0, x + (w - pw) / 2.0, top + 14.0, pw, &latch, on);
                        b.pill(PILL_PICKER, menu, 0, x + w / 2.0, top + 46.0, 0.0, &label, false);
                        b.name(x, w, row.name_y(), name, false);
                    }
                    Item::Momentary(id, label, name, w) => {
                        let held = def.index_of(id).is_some_and(|i| self.devices.panel.held == Some((d.id, i)));
                        let pw = pill_w(label);
                        b.rect(BOX_TRACK, x + (w - pw - 4.0) / 2.0, top + row.pill - 2.0, pw + 4.0, 28.0);
                        b.pill(PILL_MOMENTARY, idx(id), 0, x + (w - pw) / 2.0, top + row.pill, pw, label, held);
                        b.name(x, w, row.name_y(), name, false);
                    }
                    Item::Pickers2(pickers, name, w, inert) => {
                        for ((param, label), dy) in pickers.into_iter().zip([6.0, 36.0]) {
                            let p = b.pill(PILL_PICKER, param, 0, x + w / 2.0, top + dy, 0.0, &label, false);
                            p.inert = inert;
                        }
                        b.name(x, w, row.name_y(), name, inert);
                    }
                    Item::Latch(id, label, name, w, inert) => {
                        let pw = pill_w(label) + 12.0;
                        b.rect(BOX_TRACK, x + (w - pw - 4.0) / 2.0, top + row.pill - 2.0, pw + 4.0, 28.0);
                        let p = b.pill(PILL_TOGGLE, idx(id), 0, x + (w - pw) / 2.0, top + row.pill, pw, label, val(id) >= 0.5);
                        p.inert = inert;
                        b.name(x, w, row.name_y(), name, inert);
                    }
                    Item::Blank(_) => {}
                }
                x += w + gap;
            }
        }
    }

    /// Header pill groups, right-aligned in the top bar with a rule between groups.
    /// Each group: (kind, param, items (label, active, index), gap, on a track).
    #[allow(clippy::type_complexity)]
    fn win_header(&self, b: &mut Builder, groups: Vec<(i32, i32, Vec<(String, bool, i32)>, f32, bool)>) {
        let widths: Vec<f32> = groups
            .iter()
            .map(|(_, _, items, gap, _)| Builder::row_w(&items.iter().map(|(l, _, _)| l.as_str()).collect::<Vec<_>>(), *gap))
            .collect();
        let total: f32 = widths.iter().sum::<f32>() + 34.0 * (groups.len().saturating_sub(1)) as f32;
        let mut x = b.w - INSET - total;
        for (i, ((kind, param, items, gap, track), w)) in groups.into_iter().zip(widths).enumerate() {
            if i > 0 {
                b.rect(BOX_HEADER_RULE, x - 17.0, 11.5, 1.0, 20.0);
            }
            b.pill_row(kind, param, x, 7.5, &items, gap, track);
            x += w + 34.0;
        }
    }

    // ------------------------------------------------------------ Auto Filter

    fn auto_filter_window(&self, d: &Device, def: &'static DeviceDef, b: &mut Builder) -> f32 {
        let idx = |id: &str| def.index_of(id).map_or(-1, |i| i as i32);
        let val = |id: &str| def.param(id).map_or(0.0, |p| self.panel_value(d, def, p));
        let dj = val("mode") >= 0.5;
        let kind = val("filter_type").round() as i32;
        let slope = val("slope").round() as i32;
        let types: Vec<(String, bool, i32)> = ["LP", "HP", "BP", "Notch", "DJ"]
            .iter()
            .enumerate()
            .map(|(i, l)| (l.to_string(), if i == 4 { dj } else { !dj && kind == i as i32 }, i as i32))
            .collect();
        self.win_header(
            b,
            vec![
                (PILL_CHOICE, idx("slope"), vec![("12 dB/oct".into(), slope == 0, 0), ("24 dB/oct".into(), slope == 1, 1)], 8.0, true),
                (PILL_CHOICE, idx("filter_type"), types, 8.0, true),
            ],
        );
        let row = Row::main(462.0);
        let sync = val("lfo_sync") >= 0.5;
        let shape = def.param("lfo_shape").map(|p| p.format(val("lfo_shape"))).unwrap_or_default();
        let corner = if dj { "dj_position" } else { "frequency" };
        let rate = if sync { "lfo_rate_sync" } else { "lfo_rate_free" };
        self.win_card(
            d,
            def,
            b,
            row,
            vec![
                (
                    vec![
                        Item::Knob(corner, "Freq", 2, 60.0, false),
                        Item::Knob("resonance", "Reso", 2, 52.0, false),
                        Item::Knob("drive", "Drive", 1, 54.0, false),
                    ],
                    14.0,
                ),
                (
                    vec![
                        Item::Knob("env_amount", "Env", 2, 62.0, dj),
                        Item::Knob("env_attack", "Attack", 1, 62.0, dj),
                        Item::Knob("env_release", "Release", 1, 58.0, dj),
                    ],
                    14.0,
                ),
                (
                    vec![
                        Item::Knob("lfo_amount", "LFO", 2, 52.0, dj),
                        Item::Knob(rate, "Rate", 2, 64.0, dj),
                        Item::Blank(108.0),
                        Item::Knob("lfo_stereo", "Stereo", 1, 54.0, dj),
                    ],
                    14.0,
                ),
                (vec![Item::Knob("mix", "Mix", 1, 52.0, false)], 14.0),
            ],
        );
        // LFO mode field: Free / Sync over the shape picker (in the Blank column).
        let (xs, _) = gutter(INSET, WIN_W - 2.0 * INSET, &[194.0, 210.0, 320.0, 52.0]);
        let fx = xs[2] + 52.0 + 14.0 + 64.0 + 14.0;
        let top = row.top();
        let rw = Builder::row_w(&["Free", "Sync"], 2.0);
        let start = b.pills.len();
        b.pill_row(
            PILL_CHOICE,
            idx("lfo_sync"),
            fx + (108.0 - rw) / 2.0,
            top + 14.0,
            &[("Free".into(), !sync, 0), ("Sync".into(), sync, 1)],
            2.0,
            true,
        );
        b.pill(PILL_PICKER, idx("lfo_shape"), 0, fx + 54.0, top + row.picker, 0.0, &shape, false);
        b.name(fx, 108.0, row.name_y(), "Shape", dj);
        if dj {
            for p in &mut b.pills[start..] {
                p.inert = true;
            }
        }
        584.0
    }

    // ------------------------------------------------------------ Field family

    /// Control card of a field window (one row at y): the LFO's Timebase, Rate and Depth
    /// (Ring Mod: its modulator), the device's own selector, then its secondary knobs.
    /// Returns the window's content height.
    fn field_window(&self, d: &Device, def: &'static DeviceDef, b: &mut Builder, y: f32) -> f32 {
        if def.id == "character-mod" {
            return self.character_mod_window(d, def, b, y);
        }
        let val = |id: &str| def.param(id).map_or(0.0, |p| self.panel_value(d, def, p));
        let rate = if val("sync") >= 0.5 { "rate_sync" } else { "rate" };
        let lfo = || {
            vec![
                Item::Pills("sync", vec![("Free", 0.0), ("Sync", 1.0)], "Timebase", 108.0),
                Item::Knob(rate, "Rate", 2, 70.0, false),
                Item::Knob("depth", "Depth", 2, 70.0, false),
            ]
        };
        let tail = || {
            vec![
                Item::Knob("feedback", "Feedback", 1, 66.0, false),
                Item::Knob("width", "Width", 1, 52.0, false),
                Item::Knob("mix", "Mix", 1, 52.0, false),
                Item::Knob("output_gain", "Output", 1, 52.0, false),
            ]
        };
        let clusters = match def.id {
            "tremolo" => vec![
                lfo(),
                vec![Item::Pills("shape", vec![("Sine", 0.0), ("Triangle", 1.0), ("Square", 2.0), ("Saw Down", 3.0)], "Shape", 278.0)],
                vec![Item::Knob("stereo_phase", "Stereo Phase", 1, 84.0, false), Item::Knob("mix", "Mix", 1, 52.0, false)],
            ],
            "autopanner" => vec![
                lfo(),
                vec![Item::Pills("shape", vec![("Sine", 0.0), ("Triangle", 1.0), ("Square", 2.0), ("Random", 3.0)], "Shape", 250.0)],
                vec![Item::Knob("phase", "Phase", 1, 52.0, false), Item::Knob("mix", "Mix", 1, 52.0, false)],
            ],
            "chorus" => {
                vec![lfo(), vec![Item::Pills("voices", vec![("1", 0.0), ("2", 1.0), ("3", 2.0), ("4", 3.0)], "Voices", 186.0)], tail()]
            }
            "flanger" => {
                vec![lfo(), vec![Item::Pills("mode", vec![("Classic", 0.0), ("Jet", 1.0), ("Thru-Zero", 2.0)], "Mode", 238.0)], tail()]
            }
            "phaser" => {
                vec![lfo(), vec![Item::Pills("stages", vec![("2", 0.0), ("4", 1.0), ("6", 2.0), ("12", 3.0)], "Stages", 186.0)], tail()]
            }
            _ => {
                let idx = def.index_of("modulator").map_or(-1, |i| i as i32);
                let shape = def.param("modulator").map(|p| p.format(val("modulator"))).unwrap_or_default();
                vec![
                    vec![
                        Item::Picker(idx, shape, "Modulator", 118.0, false),
                        Item::Knob("frequency", "Frequency", 2, 70.0, false),
                        Item::Knob("spread", "Spread", 1, 60.0, false),
                    ],
                    vec![Item::Knob("bias", "Bias", 1, 60.0, false), Item::Knob("rectify", "Rectify", 1, 60.0, false)],
                    vec![Item::Knob("mix", "Mix", 1, 60.0, false), Item::Knob("output", "Output", 1, 60.0, false)],
                ]
            }
        };
        let row = Row::main(y);
        self.win_card(d, def, b, row, clusters.into_iter().map(|c| (c, 14.0)).collect());
        y + row.h + 16.0
    }

    /// Character Mod's two cards: the circuit and its motion, then its rack (Input, the
    /// circuit's artifacts, the mid / side depths, Seed and Mono, Output). A control the
    /// circuit lacks leaves the row, and the gutters close up around it.
    fn character_mod_window(&self, d: &Device, def: &'static DeviceDef, b: &mut Builder, y: f32) -> f32 {
        let val = |id: &str| def.param(id).map_or(0.0, |p| self.panel_value(d, def, p));
        let idx = |id: &str| def.index_of(id).map_or(-1, |i| i as i32);
        let label = |id: &str| def.param(id).map(|p| p.format(val(id))).unwrap_or_default();
        let circuit = (val("model").round().max(0.0) as usize).min(CM_MODES.len() - 1);
        let mut character = vec![Item::Picker(idx("model"), label("model"), "Character", 112.0, false)];
        if let Some(mode) = CM_MODES[circuit] {
            character.push(Item::Picker(idx(mode), label(mode), "Mode", 104.0, false));
        }
        let rate = if val("sync") >= 0.5 { "note" } else { "rate" };
        let main = Row::main(y);
        self.win_card(
            d,
            def,
            b,
            main,
            vec![
                (character, 10.0),
                (
                    vec![
                        Item::Pills("sync", vec![("Free", 0.0), ("Sync", 1.0)], "Timebase", 108.0),
                        Item::Knob(rate, "Rate", 2, 70.0, false),
                        Item::Knob("depth", "Depth", 2, 70.0, false),
                        Item::Knob("mix", "Mix", 1, 56.0, false),
                    ],
                    10.0,
                ),
                (
                    vec![
                        Item::Knob("width", "Width", 1, 56.0, false),
                        Item::Knob("drift", "Drift", 1, 56.0, false),
                        Item::Knob("voice", "Voice", 1, 56.0, false),
                    ],
                    10.0,
                ),
            ],
        );
        let shown = |id: &str| def.is_shown(d, id);
        let artifacts: Vec<Item> = [("art_hiss", "Hiss"), ("art_drive", "Drive"), ("art_darken", "Darken"), ("art_wow", "Wow")]
            .into_iter()
            .filter(|(id, _)| shown(id))
            .map(|(id, name)| Item::Knob(id, name, 1, 56.0, false))
            .collect();
        let mut clusters = vec![(vec![Item::Pills("input", vec![("Off", 0.0), ("Lo", 1.0), ("Hi", 2.0)], "Input", 120.0)], 10.0)];
        if !artifacts.is_empty() {
            clusters.push((artifacts, 10.0));
        }
        clusters.push((vec![Item::Knob("depth_m", "Depth-M", 1, 60.0, false), Item::Knob("depth_s", "Depth-S", 1, 60.0, false)], 10.0));
        let take = format!("Take {}", val("seed").round() as i32);
        clusters.push((vec![Item::Action(ACT_SEED, take, "Seed", 96.0), Item::ToggleW("mono_audition", "Mono", 64.0)], 10.0));
        clusters.push((vec![Item::Knob("trim", "Output", 1, 56.0, false)], 10.0));
        let rack = Row::rack(y + main.h + 12.0);
        self.win_card(d, def, b, rack, clusters);
        rack.y + rack.h + 16.0
    }

    // ------------------------------------------------------------ Dynamics family

    pub(crate) fn win_span(&self, id: DeviceId, sp: &crate::device_window_dynamics::DynSpec) -> f32 {
        self.devices.wins.span.get(&id).copied().unwrap_or(sp.span)
    }

    pub(crate) fn win_target(&self, id: DeviceId) -> usize {
        self.devices.wins.target.get(&id).copied().unwrap_or(0).min(TARGETS.len() - 1)
    }

    /// Where the loudness measurement starts (history clock, s).
    pub(crate) fn win_loud_from(&self, id: DeviceId) -> f64 {
        self.devices.wins.loud_from.get(&id).copied().unwrap_or(f64::MIN)
    }

    /// Loudness target menu rows (panel-menu code MENU_TARGET).
    pub(crate) fn win_target_rows(&self, device: DeviceId) -> crate::device_mod::MenuRows {
        let cur = self.win_target(device);
        TARGETS
            .iter()
            .enumerate()
            .map(|(i, (name, lufs))| {
                (
                    format!("{name}  \u{2212}{:.0} LUFS", -lufs),
                    if i == cur { "\u{2713}".to_string() } else { String::new() },
                    Some(crate::devices::MenuPick::Window(device, ACT_TARGET, i as i32)),
                    false,
                )
            })
            .collect()
    }

    pub(crate) fn win_held(&self, id: DeviceId) -> bool {
        self.devices.wins.held.contains(&id)
    }

    pub(crate) fn win_history(&self, id: DeviceId) -> Option<&std::collections::VecDeque<crate::device_window_dynamics::Sample>> {
        self.devices.wins.history.get(&id)
    }

    /// The histories' clock now (s).
    pub(crate) fn win_now(&self) -> f64 {
        self.devices.wins.epoch.map_or(0.0, |e| Instant::now().duration_since(e).as_secs_f64())
    }

    pub(crate) fn project_playing(&self) -> bool {
        self.engine.transport().playing
    }

    /// The 1s / 4s / 10s pills, right-aligned at `right`; returns their left edge.
    fn win_span_pills(&self, b: &mut Builder, span: f32, right: f32) -> f32 {
        let widths = [33.2, 36.1, 40.8];
        let left = right - widths.iter().sum::<f32>() - 2.0 * 8.0;
        let mut x = left;
        for (i, (s, w)) in crate::device_window_dynamics::SPANS.iter().zip(widths).enumerate() {
            b.pill(PILL_GHOST, ACT_SPAN, i as i32, x, 10.5, w, &format!("{s:.0}s"), (span - s).abs() < 1e-3);
            x += w + 8.0;
        }
        left
    }

    /// Header and control cards of a level-history window: the window lengths (and the
    /// Compressor's Hold view and style), then the device's controls with its transfer square.
    fn dynamics_window(&self, d: &Device, def: &'static DeviceDef, b: &mut Builder, y: f32) -> f32 {
        let Some(sp) = crate::device_window_dynamics::spec(def.id) else { return y };
        let val = |id: &str| def.param(id).map_or(0.0, |p| self.panel_value(d, def, p));
        let idx = |id: &str| def.index_of(id).map_or(-1, |i| i as i32);
        // Header, from the right: the style pills, Hold view, then the window lengths.
        let span = self.win_span(d.id, &sp);
        let mut x = b.w - INSET;
        // The circuit pills: Compressor's style (then Hold view), Character Compressor's model.
        let circuit = match def.id {
            "compressor" => Some("style"),
            "character-compressor" => Some("model"),
            "mastering-limiter" => Some("style"),
            _ => None,
        };
        // Character Compressor keeps its window lengths at the right edge.
        let spans_first = matches!(def.id, "character-compressor" | "mastering-limiter");
        if spans_first {
            x = self.win_span_pills(b, span, x);
            b.rect(BOX_HEADER_RULE, x - 17.0, 11.5, 1.0, 20.0);
            x -= 34.0;
        }
        if let Some(id) = circuit {
            let cur = val(id).round() as i32;
            let labels: Vec<&str> = def.param(id).map_or_else(Vec::new, |p| p.choices.to_vec());
            let items: Vec<(String, bool, i32)> =
                labels.iter().enumerate().map(|(i, l)| (l.to_string(), cur == i as i32, i as i32)).collect();
            let w = Builder::row_w(&labels, 8.0);
            x -= w;
            b.pill_row(PILL_CHOICE, idx(id), x, 7.5, &items, 8.0, true);
            b.rect(BOX_HEADER_RULE, x - 17.0, 11.5, 1.0, 20.0);
            x -= 34.0;
            if def.id == "compressor" {
                x -= 77.5;
                b.pill(PILL_GHOST_LATCH, ACT_HOLD, 0, x, 10.5, 77.5, "Hold view", self.win_held(d.id));
                b.rect(BOX_HEADER_RULE, x - 17.0, 11.5, 1.0, 20.0);
                x -= 34.0;
            }
        }
        if !spans_first {
            self.win_span_pills(b, span, x);
        }

        let sidechain = || {
            let source = match d.sidechain {
                None => "None".to_string(),
                Some(t) => self.project().track(t).map_or_else(|| "Removed".to_string(), |t| t.name.clone()),
            };
            Item::Picker(-1, source, "Sidechain", 112.0, false)
        };
        let square = Item::Blank(86.0);
        let row = Row::main(y);
        let clusters: Vec<(Vec<Item>, f32)> = match def.id {
            "compressor" => vec![
                (vec![Item::Knob("threshold", "Threshold", 2, 70.0, false), Item::Knob("ratio", "Ratio", 2, 70.0, false)], 10.0),
                (
                    vec![
                        Item::Knob("attack", "Attack", 1, 52.0, false),
                        Item::Knob("release", "Release", 1, 52.0, false),
                        Item::Knob("knee", "Knee", 1, 52.0, false),
                        Item::Knob("makeup_gain", "Gain", 1, 52.0, false),
                        Item::Knob("mix", "Mix", 1, 52.0, false),
                    ],
                    10.0,
                ),
                (vec![sidechain(), Item::Knob("sidechain_gain", "Sidechain Gain", 1, 82.0, d.sidechain.is_none())], 10.0),
                (vec![square], 10.0),
                (vec![Item::Knob("output_gain", "Output", 1, 52.0, false)], 10.0),
            ],
            "limiter" => vec![
                (vec![Item::Knob("input_gain", "Input", 1, 64.0, false)], 14.0),
                (vec![Item::Knob("ceiling", "Ceiling", 2, 70.0, false), Item::Knob("release", "Release", 1, 64.0, false)], 14.0),
                (vec![Item::Knob("lookahead", "Lookahead", 1, 64.0, false), Item::Toggle("true_peak", "True Peak", false)], 14.0),
                (vec![square], 14.0),
                (vec![Item::Knob("mix", "Mix", 1, 64.0, false), Item::Knob("output_gain", "Output", 1, 64.0, false)], 14.0),
            ],
            "gate" => vec![
                (vec![Item::Knob("threshold", "Threshold", 2, 70.0, false), Item::Knob("ratio", "Ratio", 1, 64.0, false)], 14.0),
                (
                    vec![
                        Item::Knob("attack", "Attack", 1, 64.0, false),
                        Item::Knob("hold", "Hold", 1, 64.0, false),
                        Item::Knob("release", "Release", 1, 64.0, false),
                    ],
                    14.0,
                ),
                (vec![square], 14.0),
                (
                    vec![
                        Item::Knob("lookahead", "Lookahead", 1, 64.0, false),
                        Item::Knob("sidechain_hpf", "Sidechain HPF", 1, 82.0, false),
                    ],
                    14.0,
                ),
                (vec![Item::Knob("mix", "Mix", 1, 64.0, false)], 14.0),
            ],
            "mastering-limiter" => {
                let target = TARGETS[self.win_target(d.id)].0.to_string();
                vec![
                    (vec![Item::Knob("threshold", "Threshold", 2, 64.0, false), Item::Knob("ceiling", "Ceiling", 2, 64.0, false)], 10.0),
                    (vec![Item::Knob("release", "Release", 1, 64.0, false), Item::ToggleW("auto_release", "Auto", 56.0)], 10.0),
                    (vec![square], 10.0),
                    (vec![Item::ToggleW("unity_monitor", "Unity", 64.0), Item::ToggleW("delta_monitor", "Delta", 64.0)], 10.0),
                    (
                        vec![
                            Item::Picker(MENU_TARGET, target, "Target", 104.0, false),
                            Item::Button(ACT_LOUD_RESET, "Reset", "Loudness", 64.0),
                        ],
                        10.0,
                    ),
                ]
            }
            "character-compressor" => {
                let model = val("model").round() as i32;
                let first = if model == CC_OPTO {
                    let ratio = vec![("Comp", 3.0), ("Limit", 8.0)];
                    (vec![Item::Knob("threshold", "Threshold", 2, 64.0, false), Item::Pills("ratio", ratio, "Ratio", 112.0)], 10.0)
                } else {
                    (vec![Item::Knob("threshold", "Threshold", 2, 64.0, false), Item::Knob("ratio", "Ratio", 2, 64.0, false)], 10.0)
                };
                let os = vec![("2x", 0.0), ("4x", 1.0)];
                let drive =
                    (vec![Item::Knob("drive", "Drive", 1, 56.0, false), Item::Pills("oversampling", os, "Oversampling", 112.0)], 10.0);
                let out = (
                    vec![
                        Item::Knob("mix", "Mix", 1, 56.0, false),
                        Item::Knob("makeup", "Makeup", 1, 56.0, false),
                        Item::ToggleW("auto_gain", "Auto Gain", 64.0),
                        Item::ToggleW("delta_monitor", "Delta", 64.0),
                    ],
                    10.0,
                );
                let times = |knee: bool| {
                    let mut v = Vec::new();
                    if knee {
                        v.push(Item::Knob("knee", "Knee", 1, 56.0, false));
                    }
                    v.push(Item::Knob("attack", "Attack", 1, 56.0, false));
                    v.push(Item::Knob("release", "Release", 1, 56.0, false));
                    (v, 10.0)
                };
                match model {
                    CC_CLEAN => vec![first, times(true), (vec![square], 10.0), out],
                    CC_OPTO | CC_RMS_VCA => vec![first, (vec![square], 10.0), drive, out],
                    _ => vec![first, times(false), (vec![square], 10.0), drive, out],
                }
            }
            _ => {
                // The unit is in the column's name: the pills carry the numbers alone.
                let pills = |id: &'static str| -> Vec<(&'static str, f32)> {
                    def.param(id).map_or_else(Vec::new, |p| {
                        p.choices.iter().enumerate().map(|(i, c)| (c.split(' ').next().unwrap_or(c), i as f32)).collect()
                    })
                };
                vec![
                    (vec![Item::Knob("threshold", "Threshold", 2, 64.0, false), Item::Pills("ratio", pills("ratio"), "Ratio", 125.0)], 9.0),
                    (vec![square], 9.0),
                    (
                        vec![
                            Item::Pills("attack", pills("attack"), "Attack (ms)", 248.0),
                            Item::Pills("release", pills("release"), "Release (s)", 207.0),
                        ],
                        9.0,
                    ),
                ]
            }
        };
        // The square's column: find it to place the transfer canvas.
        let widths: Vec<f32> = clusters
            .iter()
            .map(|(items, gap)| items.iter().map(Item::width).sum::<f32>() + gap * (items.len().saturating_sub(1)) as f32)
            .collect();
        let sq_at = clusters.iter().position(|(items, _)| items.len() == 1 && matches!(items[0], Item::Blank(w) if w == 86.0));
        self.win_card(d, def, b, row, clusters);
        if let Some(i) = sq_at {
            let (xs, _) = gutter(INSET, b.w - 2.0 * INSET, &widths);
            let mut sq = self.dynamics_square(d, def, accent_of(b));
            sq.x = (xs[i] + 4.0) * Z;
            sq.y = (row.top() + 4.0) * Z;
            sq.well = true;
            b.square = sq;
        }
        if matches!(def.id, "character-compressor" | "mastering-limiter") {
            let rack = Row::rack(row.y + row.h + 12.0);
            let clusters = if def.id == "mastering-limiter" {
                let choice = |id: &str| def.param(id).and_then(|p| p.choices.get(val(id).round() as usize)).map_or("", |c| *c).to_string();
                let os = def
                    .param("oversampling")
                    .map_or_else(Vec::new, |p| p.choices.iter().enumerate().map(|(i, c)| (*c, i as f32)).collect());
                vec![
                    (vec![Item::Knob("lookahead", "Lookahead", 1, 64.0, false), Item::Knob("attack", "Attack", 1, 56.0, false)], 10.0),
                    (
                        vec![
                            Item::Knob("transient_link", "Transient Link", 2, 80.0, false),
                            Item::Knob("release_link", "Release Link", 2, 80.0, false),
                            Item::ToggleW("ms_mode", "Mid-Side", 56.0),
                        ],
                        10.0,
                    ),
                    (vec![Item::Pills("oversampling", os, "Oversampling", 156.0), Item::ToggleW("true_peak", "True Peak", 56.0)], 10.0),
                    (
                        vec![
                            Item::Picker(idx("dither_bits"), choice("dither_bits"), "Dither", 88.0, false),
                            Item::Picker(idx("dither_shape"), choice("dither_shape"), "Shape", 88.0, false),
                        ],
                        10.0,
                    ),
                ]
            } else {
                self.character_compressor_rack(d, def)
            };
            self.win_card(d, def, b, rack, clusters);
            return rack.y + rack.h + 16.0;
        }
        if def.id != "glue-compressor" {
            return row.y + row.h + 16.0;
        }
        let pills = |id: &'static str| -> Vec<(&'static str, f32)> {
            def.param(id).map_or_else(Vec::new, |p| p.choices.iter().enumerate().map(|(i, c)| (*c, i as f32)).collect())
        };
        let rack = Row::rack(row.y + row.h + 12.0);
        self.win_card(
            d,
            def,
            b,
            rack,
            vec![
                (vec![sidechain(), Item::Pills("sc_hpf", pills("sc_hpf"), "SC HPF (Hz)", 207.0)], 9.0),
                (vec![Item::Knob("range", "Range", 1, 52.0, false), Item::Knob("makeup", "Makeup", 1, 52.0, false)], 9.0),
                (vec![Item::Knob("color", "Color", 1, 52.0, false), Item::Knob("mix", "Mix", 1, 52.0, false)], 9.0),
                (vec![Item::Knob("output_gain", "Output", 1, 52.0, false)], 9.0),
            ],
        );
        rack.y + rack.h + 16.0
    }

    /// Waveshapers and the Bit Crusher: one card under the display. Distortion: Drive and
    /// Bias, the Mode pills, Tone, then Gain, Mix and Output. Tape Saturation: Drive and
    /// Compression, Mode, Warmth and Noise, Mix and Output. Soft Clipper: Input and Ceiling,
    /// Shape and Softness, Gain Match, Mix and Output. Bit Crusher (its card under the
    /// display only, the meters running beside it): Bit Depth, Dither, Downsample and
    /// Jitter, Mix.
    fn wave_window(&self, d: &Device, def: &'static DeviceDef, b: &mut Builder, y: f32) -> f32 {
        let pills = |id: &str| -> Vec<(&'static str, f32)> {
            def.param(id).map_or_else(Vec::new, |p| p.choices.iter().enumerate().map(|(i, c)| (*c, i as f32)).collect())
        };
        let knob = |id, name, size, w| Item::Knob(id, name, size, w, false);
        let row = Row::main(y);
        let clusters = match def.id {
            "distortion" => vec![
                (vec![knob("drive", "Drive", 2, 70.0), knob("bias", "Bias", 1, 52.0)], 8.0),
                (vec![Item::Pills("mode", pills("mode"), "Mode", 268.0)], 8.0),
                (vec![knob("tone", "Tone", 1, 52.0)], 8.0),
                (vec![knob("output", "Gain", 1, 62.0), knob("mix", "Mix", 1, 52.0), knob("output_gain", "Output", 1, 62.0)], 8.0),
            ],
            "tapesaturation" => vec![
                (vec![knob("drive", "Drive", 2, 70.0), knob("compression", "Compression", 1, 76.0)], 8.0),
                (vec![Item::Pills("mode", pills("mode"), "Mode", 224.0)], 8.0),
                (vec![knob("warmth", "Warmth", 1, 76.0), knob("noise", "Noise", 1, 76.0)], 8.0),
                (vec![knob("mix", "Mix", 1, 76.0), knob("output_gain", "Output", 1, 76.0)], 8.0),
            ],
            "soft-clipper" => vec![
                (vec![knob("input", "Input", 2, 64.0), knob("ceiling", "Ceiling", 2, 64.0)], 12.0),
                (vec![Item::Pills("shape", pills("shape"), "Shape", 184.0), knob("softness", "Softness", 1, 62.0)], 24.0),
                (
                    vec![
                        Item::ToggleW("gain_match", "Gain Match", 72.0),
                        knob("mix", "Mix", 1, 62.0),
                        knob("output_gain", "Output", 1, 62.0),
                    ],
                    14.0,
                ),
            ],
            "declipper" => vec![
                (vec![knob("threshold", "Threshold", 2, 76.0), knob("ceiling", "Ceiling", 2, 76.0)], 8.0),
                (vec![Item::Pills("oversampling", pills("oversampling"), "Oversampling", 130.0)], 8.0),
                (vec![knob("mix", "Mix", 1, 62.0)], 8.0),
            ],
            _ => vec![
                (vec![knob("bitdepth", "Bit Depth", 2, 70.0)], 8.0),
                (
                    vec![knob("dither", "Dither", 1, 76.0), knob("downsample", "Downsample", 1, 76.0), knob("jitter", "Jitter", 1, 76.0)],
                    14.0,
                ),
                (vec![knob("mix", "Mix", 1, 76.0)], 8.0),
            ],
        };
        // The Bit Crusher's card ends under the display, its meters running down beside it.
        let full = b.w;
        if def.id == "bitcrusher" {
            b.w = kit(def).display.2 + 2.0 * INSET;
        }
        self.win_card(d, def, b, row, clusters);
        b.w = full;
        row.y + row.h + 16.0
    }

    /// Utility: three cards in the face's left part, each its controls centred: Channel
    /// Mode and the two phase switches; Width, Mono, Bass Mono with its frequency and
    /// Listen; Gain, Balance, Mute and DC Filter.
    fn utility_window(&self, d: &Device, def: &'static DeviceDef, b: &mut Builder) -> f32 {
        let idx = |id: &str| def.index_of(id).map_or(-1, |i| i as i32);
        let val = |id: &str| def.param(id).map_or(0.0, |p| self.panel_value(d, def, p));
        let label = |id: &str| def.param(id).map_or_else(String::new, |p| p.format(self.panel_value(d, def, p)));
        let knob = |id, name, size, w, inert| Item::Knob(id, name, size, w, inert);
        let full = b.w;
        b.w = kit(def).display.2 + 2.0 * INSET;
        let bass = val("bass_mono") >= 0.5;
        let rows = [
            (
                Row { y: 60.0, h: 96.0, dial: 28.0, value: 45.0, pill: 11.0, picker: 11.0 },
                vec![
                    Item::Picker(idx("channel_mode"), label("channel_mode"), "Channel Mode", 116.0, false),
                    Item::ToggleW("phase_l", "Phase L", 100.0),
                    Item::ToggleW("phase_r", "Phase R", 56.0),
                ],
            ),
            (
                Row { y: 168.0, h: 112.0, dial: 29.0, value: 54.0, pill: 27.0, picker: 27.0 },
                vec![
                    knob("width", "Width", 1, 78.0, false),
                    Item::ToggleW("mono", "Mono", 78.0),
                    Item::ToggleW("bass_mono", "Bass Mono", 106.0),
                    knob("bass_mono_freq", "Frequency", 1, 52.0, !bass),
                    Item::Latch("bass_mono_audition", "Listen", "Audition", 120.0, !bass),
                ],
            ),
            (
                Row { y: 292.0, h: 118.0, dial: 32.0, value: 59.0, pill: 35.0, picker: 35.0 },
                vec![
                    knob("gain", "Gain", 2, 81.0, false),
                    knob("balance", "Balance", 1, 87.0, false),
                    Item::ToggleW("mute", "Mute", 92.0),
                    Item::ToggleW("dc_filter", "DC Filter", 64.0),
                ],
            ),
        ];
        for (row, items) in rows {
            self.win_card(d, def, b, row, vec![(items, 0.0)]);
        }
        b.w = full;
        700.0
    }

    /// A picker in the header after its name, right-aligned at `right`; returns the left.
    #[allow(clippy::too_many_arguments)]
    fn header_picker(&self, d: &Device, def: &'static DeviceDef, b: &mut Builder, id: &str, name: &str, right: f32, inert: bool) -> f32 {
        let label = def.param(id).map_or_else(String::new, |p| p.format(self.panel_value(d, def, p)));
        let pw = pill_w(&label) + 14.0;
        let p = b.pill(PILL_PICKER, def.index_of(id).map_or(-1, |i| i as i32), 0, right - pw / 2.0, 9.5, 0.0, &label, false);
        p.inert = inert;
        let lw = crate::text_measure::value_width(name) * 1.15 + 6.0;
        let ink = if inert { 0x59 } else { 0xb3 };
        b.texts.push(PanelText {
            x: (right - pw - 8.0 - lw) * Z,
            y: 13.5 * Z,
            w: lw * Z,
            h: 16.0 * Z,
            text: s(name),
            color: Color::from_argb_u8(ink, 0xe5, 0xe5, 0xe8),
            size: 12.0 * Z,
            weight: 600,
            align: 2,
        });
        right - pw - 8.0 - lw
    }

    /// Spectrum: header (View pills, Reset and Hold view, History and Color), the analyser
    /// in the display (the Split view's spectrogram in a well of its own below the 2D
    /// part), and one card: FFT, Window, Channel | Slope, Ceiling, Range | Smoothing,
    /// Averaging, Peak Hold, Decay.
    fn spectrum_window(&self, d: &Device, def: &'static DeviceDef, b: &mut Builder, y: f32) -> f32 {
        let val = |id: &str| def.param(id).map_or(0.0, |p| self.panel_value(d, def, p));
        let idx = |id: &str| def.index_of(id).map_or(-1, |i| i as i32);
        let label = |id: &str| def.param(id).map_or_else(String::new, |p| p.format(self.panel_value(d, def, p)));
        let mode = val("display_mode").round() as i32;
        let mut x = self.header_choice(d, def, b, "display_mode", b.w - INSET);
        x = Self::header_rule(b, x);
        let rw = pill_w("Reset") + 4.0;
        b.pill(PILL_GHOST, ACT_SPEC_RESET, 0, x - rw, 10.5, rw, "Reset", false);
        x -= rw + 10.0 + 77.5;
        b.pill(PILL_GHOST_LATCH, ACT_HOLD, 0, x, 10.5, 77.5, "Hold view", self.win_held(d.id));
        x = Self::header_rule(b, x);
        x = self.header_picker(d, def, b, "history_depth", "History", x, mode == crate::device_window_spectrum::MODE_2D);
        self.header_picker(d, def, b, "color_scheme", "Color", x - 24.0, false);
        // Split: the 2D part in the display, the spectrogram in its own well below.
        if mode == crate::device_window_spectrum::MODE_SPLIT {
            let k = kit(def);
            if let Some(bx) = b.boxes.iter_mut().find(|bx| bx.kind == BOX_DISPLAY) {
                bx.h = (k.plot.1 + crate::device_window_spectrum::SPLIT_GRAM_Y - 16.0) * Z;
            }
        }
        let row = Row::main(y);
        self.win_card(
            d,
            def,
            b,
            row,
            vec![
                (
                    vec![
                        Item::Picker(idx("fft_size"), label("fft_size"), "FFT", 104.0, false),
                        Item::Picker(idx("window"), label("window"), "Window", 104.0, false),
                        Item::Picker(idx("channel_mode"), label("channel_mode"), "Channel", 104.0, false),
                    ],
                    0.0,
                ),
                (
                    vec![
                        Item::Knob("slope", "Slope", 2, 70.0, false),
                        Item::Knob("db_ceiling", "Ceiling", 2, 70.0, false),
                        Item::Knob("db_range", "Range", 2, 70.0, false),
                    ],
                    4.0,
                ),
                (
                    vec![
                        Item::Knob("smoothing", "Smoothing", 1, 76.0, false),
                        Item::Knob("averaging", "Averaging", 1, 76.0, false),
                        Item::Picker(idx("peak_hold"), label("peak_hold"), "Peak Hold", 90.0, false),
                        Item::Knob("decay", "Decay", 1, 70.0, false),
                    ],
                    4.0,
                ),
            ],
        );
        row.y + row.h + 16.0
    }

    /// Tuner: a short card: Tuning and Reference, then Mute Output.
    fn tuner_window(&self, d: &Device, def: &'static DeviceDef, b: &mut Builder, y: f32) -> f32 {
        let idx = |id: &str| def.index_of(id).map_or(-1, |i| i as i32);
        let label = |id: &str| def.param(id).map_or_else(String::new, |p| p.format(self.panel_value(d, def, p)));
        let row = Row { y, h: 68.0, dial: 20.0, value: 17.0, pill: 4.0, picker: 4.0 };
        self.win_card(
            d,
            def,
            b,
            row,
            vec![
                (
                    vec![
                        Item::Picker(idx("tuning"), label("tuning"), "Tuning", 124.0, false),
                        Item::Picker(idx("reference"), label("reference"), "Reference", 124.0, false),
                    ],
                    0.0,
                ),
                (vec![Item::ToggleW("mute_output", "Mute Output", 96.0)], 0.0),
            ],
        );
        row.y + row.h + 16.0
    }

    /// De-Esser, De-Hummer and De-Noiser: one card with every control (the De-Esser's Listen
    /// switch in a cluster of its own).
    fn repair_window(&self, d: &Device, def: &'static DeviceDef, b: &mut Builder, y: f32) -> f32 {
        let knob = |id, name, size, w| Item::Knob(id, name, size, w, false);
        let row = Row::main(y);
        let clusters = match def.id {
            "deesser" => vec![
                (
                    vec![
                        knob("frequency", "Frequency", 2, 86.0),
                        knob("bandwidth", "Bandwidth", 2, 86.0),
                        knob("threshold", "Threshold", 1, 76.0),
                        knob("reduction", "Reduction", 1, 76.0),
                        knob("mix", "Mix", 1, 76.0),
                    ],
                    0.0,
                ),
                (vec![Item::Pills("listen", vec![("Off", 0.0), ("On", 1.0)], "Listen", 96.0)], 0.0),
            ],
            "dehummer" => vec![(
                vec![
                    knob("frequency", "Frequency", 2, 86.0),
                    knob("num_harmonics", "Harmonics", 2, 86.0),
                    knob("notch_width", "Q", 1, 66.0),
                    knob("depth", "Depth", 1, 66.0),
                    knob("mix", "Mix", 1, 66.0),
                ],
                0.0,
            )],
            _ => vec![(
                vec![
                    knob("threshold", "Threshold", 2, 86.0),
                    knob("reduction", "Reduction", 2, 86.0),
                    knob("frequency_smoothing", "Freq Smooth", 1, 92.0),
                    knob("attack", "Attack", 1, 92.0),
                    knob("release", "Release", 1, 92.0),
                    knob("mix", "Mix", 1, 92.0),
                ],
                0.0,
            )],
        };
        self.win_card(d, def, b, row, clusters);
        row.y + row.h + 16.0
    }

    /// Trance Gate: the ring panel down the left; under the envelope, one card with the
    /// envelope's Attack, Decay, Sustain and Release, Swing, and Mix.
    fn trance_gate_window(&self, d: &Device, def: &'static DeviceDef, b: &mut Builder, y: f32) -> f32 {
        let knob = |id, name| Item::Knob(id, name, 1, 62.0, false);
        let row = Row { y, h: 100.0, dial: 26.0, value: 52.0, pill: 28.0, picker: 44.0 };
        b.rect(BOX_CARD, INSET, 56.0, TG_DISPLAY_X - INSET - 14.0, row.y + row.h - 56.0);
        b.left = TG_DISPLAY_X;
        self.win_card(
            d,
            def,
            b,
            row,
            vec![
                (vec![knob("attack", "Attack"), knob("decay", "Decay"), knob("sustain", "Sustain"), knob("release", "Release")], 0.0),
                (vec![knob("swing", "Swing")], 0.0),
                (vec![knob("mix", "Mix")], 0.0),
            ],
        );
        b.left = INSET;
        row.y + row.h + 16.0
    }

    /// Stereo Widener: one card under the display: Width and Low Cut, Mid and Side Gain, Mix.
    fn widener_window(&self, d: &Device, def: &'static DeviceDef, b: &mut Builder, y: f32) -> f32 {
        let knob = |id, name, size, w| Item::Knob(id, name, size, w, false);
        let full = b.w;
        b.w = kit(def).display.2 + 2.0 * INSET;
        let row = Row::main(y);
        self.win_card(
            d,
            def,
            b,
            row,
            vec![
                (vec![knob("width2", "Width", 2, 76.0), knob("lowcut", "Low Cut", 2, 76.0)], 10.0),
                (vec![knob("midgain", "Mid Gain", 1, 76.0), knob("sidegain", "Side Gain", 1, 76.0)], 2.0),
                (vec![knob("mix", "Mix", 2, 76.0)], 10.0),
            ],
        );
        b.w = full;
        row.y + row.h + 16.0
    }

    /// Transient Shaper: Hold view in the header; one card with Attack and Sustain, Sense,
    /// the shape picture (its handles set the two times), then Mix, Output and Clip.
    fn shaper_window(&self, d: &Device, def: &'static DeviceDef, b: &mut Builder, y: f32) -> f32 {
        let x = b.w - INSET - 77.5;
        b.pill(PILL_GHOST_LATCH, ACT_HOLD, 0, x, 10.5, 77.5, "Hold view", self.win_held(d.id));
        let row = Row::tall(y);
        let pic = crate::device_window_shaper::PIC_W;
        let clusters = vec![
            (vec![Item::Knob("attack", "Attack", 2, 70.0, false), Item::Knob("sustain", "Sustain", 2, 70.0, false)], 10.0),
            (vec![Item::Knob("sensitivity", "Sense", 1, 52.0, false)], 10.0),
            (vec![Item::Blank(pic)], 10.0),
            (
                vec![
                    Item::Knob("mix", "Mix", 1, 56.0, false),
                    Item::Knob("output_gain", "Output", 1, 56.0, false),
                    Item::ToggleW("clip", "Clip", 56.0),
                ],
                10.0,
            ),
        ];
        let widths: Vec<f32> = clusters
            .iter()
            .map(|(items, gap)| items.iter().map(Item::width).sum::<f32>() + gap * (items.len().saturating_sub(1)) as f32)
            .collect();
        self.win_card(d, def, b, row, clusters);
        let (xs, _) = gutter(INSET, b.w - 2.0 * INSET, &widths);
        let mut sq = self.shaper_picture(d, def, accent_of(b));
        sq.x = xs[2] * Z;
        sq.y = (row.top() + 4.0) * Z;
        sq.well = true;
        b.square = sq;
        row.y + row.h + 16.0
    }

    /// Multiband Compressor: the global card (Input, the crossovers, Depth and Time, the
    /// upward and downward amounts, Mix and Output), then the rack card of the selected band
    /// (picker, Band Input, its thresholds and ratios, its times and makeup, its transfer).
    fn multiband_window(&self, d: &Device, def: &'static DeviceDef, b: &mut Builder, y: f32) -> f32 {
        let row = Row::main(y);
        self.win_card(
            d,
            def,
            b,
            row,
            vec![
                (vec![Item::Knob("input_gain", "Input", 1, 52.0, false)], 10.0),
                (
                    vec![
                        Item::Knob("low_crossover", "Low Crossover", 1, 84.0, false),
                        Item::Knob("high_crossover", "High Crossover", 1, 84.0, false),
                    ],
                    10.0,
                ),
                (vec![Item::Knob("depth", "Depth", 2, 64.0, false), Item::Knob("time", "Time", 2, 64.0, false)], 10.0),
                (vec![Item::Knob("upward", "Upward", 2, 64.0, false), Item::Knob("downward", "Downward", 2, 64.0, false)], 10.0),
                (vec![Item::Knob("mix", "Mix", 1, 52.0, false), Item::Knob("output_gain", "Output", 1, 52.0, false)], 10.0),
            ],
        );
        let band = self.multiband_band(d.id);
        let ids = dstudio_core::builtin::MULTIBAND_BANDS[band];
        let rack = Row::main(row.y + row.h + 12.0);
        let clusters = vec![
            (
                vec![
                    Item::Picker(MENU_MB_BAND, MB_BANDS[band].to_string(), "Band", 72.0, false),
                    Item::Knob(ids[5], "Band Input", 2, 64.0, false),
                ],
                10.0,
            ),
            (vec![Item::Knob(ids[0], "Threshold", 2, 64.0, false), Item::Knob(ids[1], "Ratio", 2, 64.0, false)], 10.0),
            (vec![Item::Knob(ids[6], "Up Threshold", 2, 80.0, false), Item::Knob(ids[7], "Up Ratio", 2, 60.0, false)], 10.0),
            (
                vec![
                    Item::Knob(ids[2], "Attack", 1, 56.0, false),
                    Item::Knob(ids[3], "Release", 1, 56.0, false),
                    Item::Knob(ids[4], "Makeup", 1, 56.0, false),
                ],
                6.0,
            ),
            (vec![Item::Blank(86.0)], 10.0),
        ];
        let widths: Vec<f32> = clusters
            .iter()
            .map(|(items, gap)| items.iter().map(Item::width).sum::<f32>() + gap * (items.len().saturating_sub(1)) as f32)
            .collect();
        self.win_card(d, def, b, rack, clusters);
        let (xs, _) = gutter(INSET, b.w - 2.0 * INSET, &widths);
        let mut sq = self.dynamics_square(d, def, accent_of(b));
        sq.x = (xs[4] + 4.0) * Z;
        sq.y = (rack.top() + 4.0) * Z;
        sq.well = true;
        b.square = sq;
        rack.y + rack.h + 16.0
    }

    /// The display of a field device (Delay, Echo, Reverb): its card's field drawn at the
    /// plot's size, with the view's caption in the band above it. Pointer events over it come
    /// back through the card's field handlers with the window's geometry.
    fn field_window_display(&self, d: &Device, def: &'static DeviceDef, k: &Kit) -> PanelCanvas {
        let (_, _, dw, dh) = k.display;
        let (l, t, r, bt) = k.plot;
        let (w, h) = ((dw - l - r) * Z, (dh - t - bt) * Z);
        let mut c = self.field_canvas(d, def, w, h, true);
        c.well = false;
        let (caption, value, right) = self.field_window_caption(d, def);
        use slint::Model;
        let mut texts: Vec<PanelText> = c.texts.iter().collect();
        let y = -(t - 12.0) * Z;
        texts.push(crate::device_canvas::text(
            0.0,
            y,
            160.0 * Z,
            caption.clone(),
            Color::from_argb_u8(0x8c, 0xe5, 0xe5, 0xe8),
            12.0 * Z,
            0,
        ));
        let at = (crate::text_measure::value_width(&caption) * 1.0 + 12.0) * Z;
        texts.push(crate::device_canvas::text(at, y, 240.0 * Z, value, TEXT, 13.0 * Z, 0));
        if !right.is_empty() {
            texts.push(crate::device_canvas::text(
                w - 240.0 * Z,
                y,
                240.0 * Z,
                right,
                Color::from_argb_u8(0xb3, 0xe5, 0xe5, 0xe8),
                11.0 * Z,
                2,
            ));
        }
        c.texts = model(texts);
        c.label = s(caption);
        c
    }

    /// Caption of a field window's display: (caption, its value, a right-aligned note).
    fn field_window_caption(&self, d: &Device, def: &'static DeviceDef) -> (String, String, String) {
        let fmt = |id: &str| def.param(id).map_or_else(String::new, |p| p.format(self.panel_value(d, def, p)));
        let val = |id: &str| def.param(id).map_or(0.0, |p| self.panel_value(d, def, p));
        match def.id {
            "echo" => ("Tail".into(), String::new(), String::new()),
            "grains" => ("Buffer".into(), String::new(), String::new()),
            "pitch-shifter" => {
                ("Interval".into(), dstudio_core::builtin::pitch_shifter::interval_text(val("semitones"), val("cents")), String::new())
            }
            "delay" => {
                let synced = val("sync") >= 0.5;
                let (l, r) = if synced { (fmt("timeDivision"), fmt("timeDivisionR")) } else { (fmt("timeMs"), fmt("timeMsR")) };
                let r = if val("link") >= 0.5 { l.clone() } else { r };
                if self.card_view(d.id) == 1 {
                    let band = dstudio_core::builtin::delay::BANDS[self.delay_band(d.id)].0;
                    ("Feedback Filter".into(), band.into(), format!("L {l}   R {r}"))
                } else if l == r {
                    ("Echoes".into(), l, String::new())
                } else {
                    ("Echoes".into(), format!("L {l}   R {r}"), String::new())
                }
            }
            _ => (String::new(), String::new(), String::new()),
        }
    }

    /// A latch toggle on its own track in the header, right-aligned at `right`; returns its left.
    fn header_latch(&self, d: &Device, def: &'static DeviceDef, b: &mut Builder, id: &str, right: f32) -> f32 {
        let Some(p) = def.param(id) else { return right };
        let on = self.panel_value(d, def, p) >= 0.5;
        let (off_text, on_text) = p.latch.unwrap_or(("Off", "On"));
        let label = if on { on_text } else { off_text };
        let pw = pill_w(label);
        let x = right - pw - 4.0;
        b.rect(BOX_TRACK, x, 7.5, pw + 4.0, 28.0);
        b.pill(PILL_TOGGLE, def.index_of(id).map_or(-1, |i| i as i32), 0, x + 2.0, 9.5, pw, label, on);
        x
    }

    /// A choice parameter's pills on a track in the header, right-aligned; returns the left.
    fn header_choice(&self, d: &Device, def: &'static DeviceDef, b: &mut Builder, id: &str, right: f32) -> f32 {
        let Some(p) = def.param(id) else { return right };
        let cur = self.panel_value(d, def, p).round() as i32;
        let items: Vec<(String, bool, i32)> =
            p.choices.iter().enumerate().map(|(i, l)| (l.to_string(), cur == i as i32, i as i32)).collect();
        let w = Builder::row_w(p.choices, 8.0);
        b.pill_row(PILL_CHOICE, def.index_of(id).map_or(-1, |i| i as i32), right - w, 7.5, &items, 8.0, true);
        right - w
    }

    /// The card's views (Echoes / EQ, Tail / EQ) as header pills after a "View" label; returns the left.
    fn header_views(&self, d: &Device, def: &'static DeviceDef, b: &mut Builder, right: f32) -> f32 {
        let views = def.layout.views;
        let cur = self.card_view(d.id);
        let items: Vec<(String, bool, i32)> = views.iter().enumerate().map(|(i, l)| (l.to_string(), cur == i, i as i32)).collect();
        let w = Builder::row_w(views, 2.0);
        let x = right - w;
        b.pill_row(PILL_ACTION, ACT_VIEW, x, 7.5, &items, 2.0, true);
        let lw = crate::text_measure::value_width("View") + 2.0;
        b.texts.push(PanelText {
            x: (x - 8.0 - lw) * Z,
            y: 13.5 * Z,
            w: lw * Z,
            h: 16.0 * Z,
            text: s("View"),
            color: Color::from_argb_u8(0xb3, 0xe5, 0xe5, 0xe8),
            size: 12.0 * Z,
            weight: 600,
            align: 2,
        });
        x - 8.0 - lw
    }

    /// A rule between header groups left of `x`; returns where the next group ends.
    fn header_rule(b: &mut Builder, x: f32) -> f32 {
        b.rect(BOX_HEADER_RULE, x - 17.0, 11.5, 1.0, 20.0);
        x - 34.0
    }

    /// Delay: header (Link, Sync, Hold view, View, Dry, Mode), the echo train or repeat EQ
    /// in the display, and one card: the times, Feedback and Grit, the wobble and width, the
    /// selected repeat-EQ band, Mix.
    fn delay_window(&self, d: &Device, def: &'static DeviceDef, b: &mut Builder, y: f32) -> f32 {
        let val = |id: &str| def.param(id).map_or(0.0, |p| self.panel_value(d, def, p));
        let idx = |id: &str| def.index_of(id).map_or(-1, |i| i as i32);
        let mut x = self.header_choice(d, def, b, "mode", b.w - INSET);
        x = Self::header_rule(b, x);
        x = self.header_latch(d, def, b, "dry", x);
        x = Self::header_rule(b, x);
        x = self.header_views(d, def, b, x);
        x = Self::header_rule(b, x) - 77.5;
        b.pill(PILL_GHOST_LATCH, ACT_HOLD, 0, x, 10.5, 77.5, "Hold view", self.win_held(d.id));
        x = Self::header_rule(b, x);
        x = self.header_latch(d, def, b, "sync", x);
        self.header_latch(d, def, b, "link", x - 8.0);

        let synced = val("sync") >= 0.5;
        let linked = val("link") >= 0.5;
        let (tl, tr, names) =
            if synced { ("timeDivision", "timeDivisionR", ["Note L", "Note R"]) } else { ("timeMs", "timeMsR", ["Time L", "Time R"]) };
        let band = self.delay_band(d.id);
        let ids = crate::panel_fields::DELAY_BAND_IDS[band];
        let on = val(ids[3]) >= 0.5;
        let latch = if on { "Band On" } else { "Band Off" };
        let band_name = dstudio_core::builtin::delay::BANDS[band].0.to_string();
        let row = Row::main(y);
        self.win_card(
            d,
            def,
            b,
            row,
            vec![
                (vec![Item::Knob(tl, names[0], 1, 62.0, false), Item::Knob(tr, names[1], 1, 62.0, linked)], 10.0),
                (vec![Item::Knob("feedback", "Feedback", 2, 72.0, false), Item::Knob("grit", "Grit", 1, 62.0, false)], 10.0),
                (
                    vec![
                        Item::Knob("modSpeed", "Rate", 1, 62.0, false),
                        Item::Knob("modDepth", "Depth", 1, 62.0, false),
                        Item::Knob("stereoWidth", "Width", 1, 62.0, false),
                    ],
                    10.0,
                ),
                (
                    vec![
                        Item::LatchPicker(idx(ids[3]), latch.into(), on, MENU_DELAY_BAND, band_name, "Band", 96.0),
                        Item::Knob(ids[0], "Freq", 2, 72.0, false),
                        Item::Knob(ids[1], "Gain", 2, 72.0, false),
                        Item::Knob(ids[2], "Q", 1, 62.0, false),
                    ],
                    10.0,
                ),
                (vec![Item::Knob("mix", "Mix", 2, 72.0, false)], 10.0),
            ],
        );
        row.y + row.h + 16.0
    }

    /// Reverb: header (View, Dry, Mode), the tail picture (or the EQ wash) in the display,
    /// and one card: Decay, Damping, Size; the Filter range; Pre-Delay, Width; Diffusion and
    /// the modulation; the shimmer's shifts and amount (lit in Shimmer); Mix.
    fn reverb_window(&self, d: &Device, def: &'static DeviceDef, b: &mut Builder, y: f32) -> f32 {
        let val = |id: &str| def.param(id).map_or(0.0, |p| self.panel_value(d, def, p));
        let idx = |id: &str| def.index_of(id).map_or(-1, |i| i as i32);
        let fmt = |id: &str| def.param(id).map_or_else(String::new, |p| p.format(val(id)));
        let mut x = self.header_choice(d, def, b, "mode2", b.w - INSET);
        x = Self::header_rule(b, x);
        x = self.header_latch(d, def, b, "dry", x);
        x = Self::header_rule(b, x);
        self.header_views(d, def, b, x);

        let shimmer = val("mode2").round() as u32 != dstudio_core::builtin::reverb::SHIMMER;
        let row = Row::main(y);
        const RANGE_W: f32 = 150.0;
        let clusters = vec![
            (
                vec![
                    Item::Knob("decay", "Decay", 2, 72.0, false),
                    Item::Knob("damping", "Damping", 2, 72.0, false),
                    Item::Knob("size", "Size", 1, 62.0, false),
                ],
                14.0,
            ),
            (vec![Item::Blank(RANGE_W)], 14.0),
            (vec![Item::Knob("predelay", "Pre-Delay", 1, 62.0, false), Item::Knob("width", "Width", 1, 62.0, false)], 14.0),
            (
                vec![
                    Item::Knob("diffusion", "Diffusion", 1, 62.0, false),
                    Item::Knob("moddepth", "Mod", 1, 62.0, false),
                    Item::Knob("modrate", "Rate", 1, 62.0, false),
                ],
                14.0,
            ),
            (
                vec![
                    Item::Pickers2([(idx("shift1"), fmt("shift1")), (idx("shift2"), fmt("shift2"))], "Shift", 88.0, shimmer),
                    Item::Knob("shimmer", "Shimmer", 1, 62.0, shimmer),
                ],
                14.0,
            ),
            (vec![Item::Knob("mix", "Mix", 2, 72.0, false)], 14.0),
        ];
        let widths: Vec<f32> = clusters
            .iter()
            .map(|(items, gap)| items.iter().map(Item::width).sum::<f32>() + gap * (items.len().saturating_sub(1)) as f32)
            .collect();
        self.win_card(d, def, b, row, clusters);
        let (xs, _) = gutter(INSET, b.w - 2.0 * INSET, &widths);
        let top = row.top();
        b.name(xs[1], RANGE_W, row.name_y(), "Filter", false);
        b.texts.push(PanelText {
            x: (xs[1] - 8.0) * Z,
            y: (top + row.value) * Z,
            w: (RANGE_W + 16.0) * Z,
            h: 16.0 * Z,
            text: s(format!("{} \u{2013} {}", fmt("lowcut2"), fmt("highcut2"))),
            color: accent_of(b),
            size: 12.0 * Z,
            weight: 600,
            align: 1,
        });
        let mut range = self.range_canvas(d, def, ("lowcut2", "highcut2"), RANGE_W, accent_of(b));
        range.x = xs[1] * Z;
        range.y = (top + 12.0) * Z;
        b.square = range;
        row.y + row.h + 16.0
    }

    /// Pitch Shifter: Semitones and Cents, the Formant switch (Shift, or Keep), Mix.
    fn pitch_shifter_window(&self, d: &Device, def: &'static DeviceDef, b: &mut Builder, y: f32) -> f32 {
        let keep = def.param("formant_preserve").is_some_and(|p| self.panel_value(d, def, p) >= 0.5);
        let row = Row::main(y);
        self.win_card(
            d,
            def,
            b,
            row,
            vec![
                (vec![Item::Knob("semitones", "Semitones", 2, 72.0, false), Item::Knob("cents", "Cents", 2, 72.0, false)], 14.0),
                (vec![Item::Latch("formant_preserve", if keep { "Keep" } else { "Shift" }, "Formant", 120.0, false)], 14.0),
                (vec![Item::Knob("mix", "Mix", 2, 72.0, false)], 14.0),
            ],
        );
        row.y + row.h + 16.0
    }

    /// Pitch Corrector: Follow, Key and Scale (the Key picker inert while the project's key is
    /// followed), then Speed, Retune Speed and Humanize, then Mix.
    fn pitch_corrector_window(&self, d: &Device, def: &'static DeviceDef, b: &mut Builder, y: f32) -> f32 {
        let val = |id: &str| def.param(id).map_or(0.0, |p| self.panel_value(d, def, p));
        let idx = |id: &str| def.index_of(id).map_or(-1, |i| i as i32);
        let fmt = |id: &str| def.param(id).map_or_else(String::new, |p| p.format(val(id)));
        let row = Row::main(y);
        self.win_card(
            d,
            def,
            b,
            row,
            vec![
                (
                    vec![
                        Item::Pills("key_follow", vec![("Manual", 0.0), ("Project", 1.0)], "Follow", 136.0),
                        Item::Picker(idx("key"), fmt("key"), "Key", 56.0, val("key_follow") >= 0.5),
                        Item::Pills("scale", vec![("Chromatic", 0.0), ("Major", 1.0), ("Minor", 2.0)], "Scale", 210.0),
                    ],
                    16.0,
                ),
                (
                    vec![
                        Item::Knob("speed", "Speed", 2, 72.0, false),
                        Item::Knob("retune_speed_ms", "Retune Speed", 2, 96.0, false),
                        Item::Knob("humanize", "Humanize", 1, 72.0, false),
                    ],
                    14.0,
                ),
                (vec![Item::Knob("mix", "Mix", 2, 72.0, false)], 14.0),
            ],
        );
        row.y + row.h + 16.0
    }

    /// Grains: Cloud / Rhythm and Freeze in the header; the grain card (where, how long and
    /// how often, pitch) over the sound card (texture, Character, Mix, Output). Rhythm swaps
    /// Position, Size and Density for its note, Gate, Rate and Chance.
    fn grains_window(&self, d: &Device, def: &'static DeviceDef, b: &mut Builder, y: f32) -> f32 {
        let val = |id: &str| def.param(id).map_or(0.0, |p| self.panel_value(d, def, p));
        let idx = |id: &str| def.index_of(id).map_or(-1, |i| i as i32);
        let fmt = |id: &str| def.param(id).map_or_else(String::new, |p| p.format(val(id)));
        let x = self.header_latch(d, def, b, "freeze", b.w - INSET);
        let x = Self::header_rule(b, x);
        self.header_choice(d, def, b, "mode", x);

        let rhythm = val("mode").round() as u32 == dstudio_core::builtin::grains::RHYTHM;
        let (where_, timing) = if rhythm {
            (
                vec![
                    Item::Picker(idx("position_note"), fmt("position_note"), "Position", 72.0, false),
                    Item::Knob("spray", "Spray", 1, 62.0, false),
                    Item::Knob("drift", "Drift", 1, 62.0, false),
                ],
                vec![
                    Item::Picker(idx("rate"), fmt("rate"), "Rate", 72.0, false),
                    Item::Knob("gate", "Gate", 1, 62.0, false),
                    Item::Knob("chance", "Chance", 1, 62.0, false),
                    Item::Knob("shape", "Shape", 1, 62.0, false),
                ],
            )
        } else {
            (
                vec![
                    Item::Knob("position", "Position", 1, 62.0, false),
                    Item::Knob("spray", "Spray", 1, 62.0, false),
                    Item::Knob("drift", "Drift", 1, 62.0, false),
                ],
                vec![
                    Item::Knob("size", "Size", 1, 62.0, false),
                    Item::Knob("density", "Density", 1, 62.0, false),
                    Item::Knob("shape", "Shape", 1, 62.0, false),
                ],
            )
        };
        let row = Row::main(y);
        self.win_card(
            d,
            def,
            b,
            row,
            vec![
                (where_, 8.0),
                (timing, 8.0),
                (
                    vec![
                        Item::Knob("pitch", "Pitch", 1, 62.0, false),
                        Item::Knob("spread", "Spread", 1, 62.0, rhythm),
                        Item::Picker(idx("scale"), fmt("scale"), "Scale", 84.0, false),
                        Item::Knob("reverse", "Reverse", 1, 62.0, false),
                    ],
                    8.0,
                ),
            ],
        );
        let row2 = Row { h: 100.0, ..Row::main(row.y + row.h + 12.0) };
        self.win_card(
            d,
            def,
            b,
            row2,
            vec![
                (
                    vec![
                        Item::Knob("width", "Width", 1, 56.0, false),
                        Item::Knob("feedback", "Feedback", 1, 56.0, false),
                        Item::Knob("blur", "Blur", 1, 56.0, false),
                        Item::Knob("tone", "Tone", 1, 56.0, false),
                        Item::Knob("space", "Space", 1, 56.0, false),
                    ],
                    8.0,
                ),
                (vec![Item::Picker(idx("character"), fmt("character"), "Character", 96.0, false)], 8.0),
                (vec![Item::Knob("mix", "Mix", 1, 56.0, false), Item::Toggle("mix_lock", "Mix Lock", false)], 10.0),
                (vec![Item::Knob("output", "Output", 1, 62.0, false)], 8.0),
            ],
        );
        row2.y + row2.h + 16.0
    }

    /// A two-handle range on a line (Reverb's Filter): the span between the handles lit, each
    /// handle dragged along the line in its parameter's control range.
    fn range_canvas(&self, d: &Device, def: &'static DeviceDef, ids: (&str, &str), w: f32, accent: Color) -> PanelCanvas {
        use crate::device_canvas::{fill, stroke, white};
        let (pw, ph) = (w * Z, 28.0 * Z);
        let (x0, x1) = (6.0 * Z, pw - 6.0 * Z);
        let cy = ph / 2.0;
        let norm = |id: &str| def.param(id).map_or(0.0, |p| p.to_normalized(self.panel_value(d, def, p)));
        let (a, bx) = (x0 + norm(ids.0) * (x1 - x0), x0 + norm(ids.1) * (x1 - x0));
        let circle = |cx: f32| {
            let r = 4.5 * Z;
            format!("M{:.1} {cy:.1} a{r:.1} {r:.1} 0 1 0 {:.1} 0 a{r:.1} {r:.1} 0 1 0 {:.1} 0 Z", cx - r, 2.0 * r, -2.0 * r)
        };
        let mut paths = vec![
            stroke(format!("M{x0:.1} {cy:.1} H{x1:.1}"), white(0.15), 1.5 * Z),
            stroke(format!("M{a:.1} {cy:.1} H{bx:.1}"), accent, 1.5 * Z),
        ];
        for cx in [a, bx] {
            paths.push(fill(circle(cx), Color::from_rgb_u8(0x1c, 0x1c, 0x20)));
            paths.push(stroke(circle(cx), accent, 1.25 * Z));
        }
        let rate = 1.0 / (x1 - x0);
        let zones = [(ids.0, a), (ids.1, bx)]
            .iter()
            .map(|(id, cx)| crate::PanelZone {
                x: cx - 7.0 * Z,
                y: 0.0,
                w: 14.0 * Z,
                h: ph,
                param_x: def.index_of(id).map_or(-1, |i| i as i32),
                param_y: -1,
                rate_x: rate,
                rate_y: 0.0,
            })
            .collect();
        PanelCanvas { visible: true, w: pw, h: ph, paths: model(paths), zones: model(zones), label: "Filter".into(), ..Default::default() }
    }

    /// Echo: header (the machine's Mode menu, the machine pills), the tape strip with its
    /// heads, the tail in the display, the main card (spacing, the transport, width, mix,
    /// Throw) and a rack that shows the tail filters and ducking, or the machine's character.
    fn echo_window(&self, d: &Device, def: &'static DeviceDef, b: &mut Builder, y: f32) -> f32 {
        let val = |id: &str| def.param(id).map_or(0.0, |p| self.panel_value(d, def, p));
        let idx = |id: &str| def.index_of(id).map_or(-1, |i| i as i32);
        let fmt = |id: &str| def.param(id).map_or_else(String::new, |p| p.format(val(id)));
        let machine = (val("machine").round() as usize).min(2);
        let mut x = self.header_choice(d, def, b, "machine", b.w - INSET);
        x = Self::header_rule(b, x);
        let mode = ["mode_loop", "combo_drum", "mode_cartridge"][machine];
        let label = fmt(mode);
        let pw = pill_w(&label) + 14.0;
        b.pill(PILL_PICKER, idx(mode), 0, x - pw / 2.0, 9.5, 0.0, &label, false);
        let lw = crate::text_measure::value_width("Mode") * 1.15 + 6.0;
        b.texts.push(PanelText {
            x: (x - pw - 8.0 - lw) * Z,
            y: 13.5 * Z,
            w: lw * Z,
            h: 16.0 * Z,
            text: s("Mode"),
            color: Color::from_argb_u8(0xb3, 0xe5, 0xe5, 0xe8),
            size: 12.0 * Z,
            weight: 600,
            align: 2,
        });

        // The tape strip: the machine's name, the Character latch and the heads on the tape.
        let character = self.devices.wins.section.get(&d.id).copied().unwrap_or(0) == 1;
        b.rect(BOX_DISPLAY, INSET, 52.0, b.w - 2.0 * INSET, 96.0);
        b.pill(PILL_GHOST_LATCH, ACT_SECTION, if character { 0 } else { 1 }, b.w - INSET - 86.0, 58.0, 77.8, "Character", character);
        let mut strip = self.echo_tape_canvas(d, def, b.w - 2.0 * INSET, accent_of(b));
        strip.x = INSET * Z;
        strip.y = 52.0 * Z;
        b.square = strip;

        let heads = ["heads_machine", "heads"];
        let row = Row::main(y);
        self.win_card(
            d,
            def,
            b,
            row,
            vec![
                (
                    vec![
                        Item::Picker(idx("spacing"), fmt("spacing"), "Spacing", 88.0, false),
                        Item::Knob("spacing_morph", "Morph", 1, 60.0, false),
                        Item::Picker(idx(heads[1]), fmt(heads[1]), "Heads", 88.0, false),
                    ],
                    10.0,
                ),
                (
                    vec![
                        Item::Knob("motor", "Motor", 2, 64.0, false),
                        Item::Knob("intensity", "Intensity", 2, 64.0, false),
                        Item::Knob("generations", "Generations", 2, 72.0, false),
                        Item::Knob("swell", "Smear", 1, 60.0, false),
                    ],
                    10.0,
                ),
                (vec![Item::Knob("width", "Width", 1, 60.0, false), Item::Knob("mix", "Mix", 2, 64.0, false)], 10.0),
                (vec![Item::Momentary("throw", "Tap", "Throw", 64.0)], 10.0),
            ],
        );
        let rack = Row::rack(row.y + row.h + 12.0);
        let clusters = if character {
            vec![
                (vec![Item::Knob("input_drive", "Input", 1, 60.0, false), Item::Knob("rec_level", "Rec Level", 1, 60.0, false)], 10.0),
                (vec![Item::Knob("tone_bass", "Bass", 1, 60.0, false), Item::Knob("tone_treble", "Treble", 1, 60.0, false)], 10.0),
                (
                    vec![
                        Item::Knob("art_hiss", "Hiss", 1, 60.0, false),
                        Item::Knob("art_wf", "Wow & Flutter", 1, 84.0, false),
                        Item::Knob("art_splice", "Splice", 1, 60.0, false),
                        Item::Knob("art_bias", "Bias", 1, 60.0, false),
                        Item::Knob("art_scrape", "Scrape", 1, 60.0, false),
                    ],
                    10.0,
                ),
                (vec![Item::ToggleW("spring_on", "Spring", 56.0), Item::Knob("spring", "Amount", 1, 60.0, false)], 10.0),
            ]
        } else {
            let tail = val("tail_on") < 0.5;
            let duck = val("duck_on") < 0.5;
            vec![
                (
                    vec![
                        Item::ToggleW("tail_on", "Tail", 56.0),
                        Item::Knob("tail_hp_l", "HP Left", 1, 60.0, tail),
                        Item::Knob("tail_lp_l", "LP Left", 1, 60.0, tail),
                        Item::Knob("tail_hp_r", "HP Right", 1, 60.0, tail),
                        Item::Knob("tail_lp_r", "LP Right", 1, 60.0, tail),
                    ],
                    6.0,
                ),
                (
                    vec![
                        Item::ToggleW("duck_on", "Duck", 56.0),
                        Item::Knob("duck_amount", "Amount", 1, 60.0, duck),
                        Item::Knob("duck_thresh", "Threshold", 2, 64.0, duck),
                        Item::Knob("duck_attack", "Attack", 1, 60.0, duck),
                        Item::Knob("duck_release", "Release", 1, 60.0, duck),
                        Item::Picker(idx("duck_key"), fmt("duck_key"), "Key", 76.0, duck),
                        Item::ToggleW("duck_fb_only", "FB Only", 56.0),
                    ],
                    6.0,
                ),
            ]
        };
        self.win_card(d, def, b, rack, clusters);
        rack.y + rack.h + 16.0
    }

    /// Echo's tape strip: the machine's name over the tape, the erase and record heads at its
    /// start and each playback head at its distance (lit while it plays).
    fn echo_tape_canvas(&self, d: &Device, def: &'static DeviceDef, w: f32, accent: Color) -> PanelCanvas {
        use crate::device_canvas::{fill, stroke, text, white};
        let val = |id: &str| def.param(id).map_or(0.0, |p| self.panel_value(d, def, p));
        let machine = (val("machine").round() as usize).min(2);
        let spec = dstudio_core::builtin::echo::MACHINE_SPECS[machine];
        let pic = self.echo_picture(d, def);
        let (pw, ph) = (w * Z, 96.0 * Z);
        let mut paths = Vec::new();
        let mut texts = vec![text(16.0 * Z, 13.0 * Z, 200.0 * Z, dstudio_core::builtin::echo::MACHINES[machine], white(0.75), 12.0 * Z, 0)];
        // The tape: a long capsule.
        let (x0, x1, ty, th) = (20.0 * Z, pw - 16.0 * Z, 56.0 * Z, 28.0 * Z);
        let r = th / 2.0;
        paths.push(stroke(
            format!(
                "M{:.1} {ty:.1} H{:.1} A{r:.1} {r:.1} 0 0 1 {:.1} {:.1} H{:.1} A{r:.1} {r:.1} 0 0 1 {:.1} {ty:.1} Z",
                x0 + r,
                x1 - r,
                x1 - r,
                ty + th,
                x0 + r,
                x0 + r
            ),
            white(0.45),
            1.0,
        ));
        let rec_x = 68.0 * Z;
        let far = spec.heads_mm.iter().chain(pic.heads.iter().map(|h| &h.mm)).fold(1.0f32, |m, v| m.max(*v));
        let x_of = |mm: f32| rec_x + mm / (far * 1.25) * (x1 - 40.0 * Z - rec_x);
        let head = |x: f32, lit: bool, color: Color, paths: &mut Vec<_>| {
            let rect = format!("M{:.1} {:.1} h{:.1} v{:.1} h{:.1} Z", x - 3.0 * Z, ty - 10.0 * Z, 6.0 * Z, 20.0 * Z, -6.0 * Z);
            if lit {
                paths.push(fill(rect, color));
            } else {
                paths.push(stroke(rect, white(0.5), 1.0));
            }
        };
        let erase_x = rec_x - 30.0 * Z;
        head(erase_x, false, white(0.9), &mut paths);
        head(rec_x, true, white(0.9), &mut paths);
        texts.push(text(erase_x - 20.0 * Z, ty - 18.0 * Z, 40.0 * Z, "Erase", white(0.4), 9.0 * Z, 1));
        texts.push(text(rec_x - 20.0 * Z, ty - 18.0 * Z, 40.0 * Z, "Rec", white(0.4), 9.0 * Z, 1));
        for (i, h) in pic.heads.iter().enumerate() {
            let x = x_of(h.mm);
            head(x, h.on, accent, &mut paths);
            texts.push(text(
                x - 20.0 * Z,
                ty - 18.0 * Z,
                40.0 * Z,
                format!("{}", i + 1),
                if h.on { accent } else { white(0.4) },
                9.0 * Z,
                1,
            ));
        }
        PanelCanvas { visible: true, w: pw, h: ph, paths: model(paths), texts: model(texts), label: "Tape".into(), ..Default::default() }
    }

    /// Delay band picker menu rows (panel-menu code MENU_DELAY_BAND).
    pub(crate) fn win_delay_band_rows(&self, device: DeviceId) -> crate::device_mod::MenuRows {
        let cur = self.delay_band(device);
        dstudio_core::builtin::delay::BANDS
            .iter()
            .enumerate()
            .map(|(i, (name, ..))| {
                let pick = Some(crate::devices::MenuPick::Window(device, ACT_DELAY_BAND, i as i32));
                (name.to_string(), if i == cur { "\u{2713}".to_string() } else { String::new() }, pick, false)
            })
            .collect()
    }

    /// Band picker menu rows (panel-menu code MENU_MB_BAND).
    pub(crate) fn win_mb_band_rows(&self, device: DeviceId) -> crate::device_mod::MenuRows {
        let cur = self.multiband_band(device);
        MB_BANDS
            .iter()
            .enumerate()
            .map(|(i, name)| {
                let pick = Some(crate::devices::MenuPick::Window(device, ACT_MB_BAND, i as i32));
                (name.to_string(), if i == cur { "\u{2713}".to_string() } else { String::new() }, pick, false)
            })
            .collect()
    }

    /// Character Compressor's rack card: the Section pills, then the section's controls
    /// (Detector: release behaviour, lookahead and stereo; Sidechain: the detector's source
    /// and filters; Level: the slow rider and its weighting bands).
    fn character_compressor_rack(&self, d: &Device, def: &'static DeviceDef) -> Vec<(Vec<Item>, f32)> {
        let val = |id: &str| def.param(id).map_or(0.0, |p| self.panel_value(d, def, p));
        let section = self.devices.wins.section.get(&d.id).copied().unwrap_or(0).min(2);
        let pills = |id: &'static str| -> Vec<(&'static str, f32)> {
            def.param(id).map_or_else(Vec::new, |p| p.choices.iter().enumerate().map(|(i, c)| (*c, i as f32)).collect())
        };
        let head = (vec![Item::ActionPills(ACT_SECTION, vec!["Detector", "Sidechain", "Level"], section, "Section", 220.0, false)], 10.0);
        match section {
            1 => vec![
                head,
                (vec![Item::Pills("sc_source", pills("sc_source"), "Source", 156.0)], 10.0),
                (vec![Item::ToggleW("sc_hpf_on", "HPF", 64.0), Item::Knob("sc_hpf_freq", "HPF Freq", 1, 64.0, false)], 10.0),
                (
                    vec![
                        Item::Knob("sc_eq_freq", "Bell Freq", 1, 64.0, false),
                        Item::Knob("sc_eq_gain", "Bell Gain", 1, 64.0, false),
                        Item::Knob("sc_eq_q", "Bell Q", 1, 64.0, false),
                    ],
                    6.0,
                ),
                (vec![Item::ToggleW("sc_listen", "Listen", 64.0)], 10.0),
            ],
            2 => {
                const BANDS: [[&str; 4]; 4] = [
                    ["level1_enabled", "level1_freq", "level1_q", "level1_weight"],
                    ["level2_enabled", "level2_freq", "level2_q", "level2_weight"],
                    ["level3_enabled", "level3_freq", "level3_q", "level3_weight"],
                    ["level4_enabled", "level4_freq", "level4_q", "level4_weight"],
                ];
                let band = self.devices.wins.tame_band.get(&d.id).copied().unwrap_or(0).min(3);
                let [on, freq, q, weight] = BANDS[band];
                vec![
                    head,
                    (
                        vec![
                            Item::ToggleW("level_enabled", "Level", 64.0),
                            Item::Knob("level_amount", "Amount", 1, 58.0, false),
                            Item::Knob("level_response", "Response", 1, 58.0, false),
                            Item::Knob("level_tilt", "Tilt", 1, 58.0, false),
                        ],
                        10.0,
                    ),
                    (
                        vec![
                            Item::ActionPills(ACT_TAME_BAND, vec!["1", "2", "3", "4"], band, "Band", 136.0, false),
                            Item::ToggleW(on, "Band On", 64.0),
                            Item::Knob(freq, "Freq", 1, 56.0, false),
                            Item::Knob(q, "Q", 1, 56.0, false),
                            Item::Knob(weight, "Weight", 1, 56.0, false),
                        ],
                        10.0,
                    ),
                ]
            }
            _ => {
                let model = val("model").round() as i32;
                let release = if matches!(model, CC_OPTO | CC_RMS_VCA) {
                    vec![Item::Knob("range", "Range", 1, 56.0, false)]
                } else {
                    vec![
                        Item::ToggleW("adapt", "Adapt", 64.0),
                        Item::Knob("hold", "Hold", 1, 56.0, false),
                        Item::Knob("range", "Range", 1, 56.0, false),
                    ]
                };
                vec![
                    head,
                    (release, 10.0),
                    (vec![Item::Knob("lookahead", "Lookahead", 1, 64.0, false)], 10.0),
                    (
                        vec![
                            Item::Knob("stereo_link", "Stereo Link", 1, 64.0, false),
                            Item::Pills("ms_mode", pills("ms_mode"), "M/S Mode", 156.0),
                        ],
                        10.0,
                    ),
                ]
            }
        }
    }

    // ------------------------------------------------------------ EQs

    /// Band pills of an EQ header: the selected band lit, bands out of the chain dimmed.
    fn eq_band_pills(&self, d: &Device, def: &DeviceDef, v: &EqView) -> Vec<(String, bool, i32)> {
        let val = |id: &str| def.param(id).map_or(0.0, |p| self.panel_value(d, def, p));
        let sel = self.eq_selected(d, v);
        (0..v.bands.len())
            .map(|i| ((i + 1).to_string(), i == sel, i as i32))
            .collect::<Vec<_>>()
            .into_iter()
            .map(|(l, a, i)| {
                let _ = v.band_on(i as usize, &val);
                (l, a, i)
            })
            .collect()
    }

    /// Marks the band pills of bands out of the chain (drawn dimmed).
    fn dim_parked(&self, d: &Device, def: &DeviceDef, v: &EqView, b: &mut Builder, from: usize) {
        let val = |id: &str| def.param(id).map_or(0.0, |p| self.panel_value(d, def, p));
        for p in b.pills[from..].iter_mut().filter(|p| p.kind == PILL_BAND) {
            p.dim = !v.band_on(p.index as usize, &val);
        }
    }

    /// The band field of an EQ window: Band On / Listen latches over the band picker.
    fn eq_band_field(&self, d: &Device, def: &DeviceDef, v: &EqView, b: &mut Builder, row: Row, x: f32, w: f32) {
        let val = |id: &str| def.param(id).map_or(0.0, |p| self.panel_value(d, def, p));
        let sel = self.eq_selected(d, v);
        let on = v.band_on(sel, &val);
        let listen = def.param("listen").is_some_and(|_| val("listen").round() as usize == sel + 1);
        let top = row.top();
        let half = (w - 2.0) / 2.0;
        // Each latch sits on its own track, as wide as its wider state plus padding.
        for (i, (action, label, active, pw)) in
            [(ACT_BAND_ON, if on { "Band On" } else { "Band Off" }, on, 73.4), (ACT_LISTEN, "Listen", listen, 75.1)].into_iter().enumerate()
        {
            let cx = x + half * i as f32 + (half - pw - 4.0) / 2.0 + if i == 1 { 2.0 } else { 0.0 };
            b.rect(BOX_TRACK, cx, top + 14.0, pw + 4.0, 28.0);
            b.pill(PILL_ACTION, action, 0, cx + 2.0, top + 16.0, pw, label, active);
        }
        b.pill(PILL_PICKER, MENU_BANDS, 0, x + w / 2.0, top + row.picker, 0.0, &(sel + 1).to_string(), false);
        b.name(x, w, row.name_y(), "Band", false);
    }

    /// Type / Slope pickers and the band knobs of an EQ's selected band.
    fn eq_band_items(&self, d: &Device, def: &DeviceDef, v: &EqView, md: f32, sm: f32) -> (Vec<Item>, Vec<Item>) {
        let val = |id: &str| def.param(id).map_or(0.0, |p| self.panel_value(d, def, p));
        let sel = self.eq_selected(d, v);
        let band = &v.bands[sel];
        let shape = v.shape(val(band.kind));
        let gain_off = !shape.has_gain();
        // Dual Low: the gain is the low boost and the Q knob's place holds the cut.
        let knobs = match (shape, band.low_atten) {
            (Shape::DualLow, Some(cut)) => {
                vec![
                    Item::Knob(band.freq, "Freq", 2, md, false),
                    Item::Knob(band.gain, "Boost", 2, md, false),
                    Item::Knob(cut, "Cut", 1, sm, false),
                ]
            }
            _ => vec![
                Item::Knob(band.freq, "Freq", 2, md, false),
                Item::Knob(band.gain, "Gain", 2, md, gain_off),
                Item::Knob(band.q, "Q", 1, sm, false),
            ],
        };
        let idx = |id: &str| def.index_of(id).map_or(-1, |i| i as i32);
        let slope = v.slope_param(sel, shape).filter(|_| matches!(shape, Shape::HighPass | Shape::LowPass));
        let slope_text = slope
            .and_then(|sp| def.param(sp).map(|p| format!("{} dB/oct", p.format(val(sp)).trim_end_matches(" dB/oct"))))
            .unwrap_or_else(|| "None".into());
        let w = if v.bank.is_some() { 98.0 } else { 96.0 };
        let pickers = vec![
            Item::Picker(idx(band.kind), shape.name().to_string(), "Type", w, false),
            Item::Picker(slope.map_or(-1, idx), slope_text, "Slope", w, slope.is_none()),
        ];
        (knobs, pickers)
    }

    fn channel_eq_window(&self, d: &Device, def: &'static DeviceDef, b: &mut Builder) -> f32 {
        let Some(View::Eq(v)) = def.layout.view else { return 584.0 };
        let from = b.pills.len();
        self.win_header(b, vec![(PILL_BAND, -1, self.eq_band_pills(d, def, v), 8.0, true)]);
        self.dim_parked(d, def, v, b, from);
        // Left of the band strip: the analyser's view and its Hold view latch.
        let strip_x = b.boxes.last().map_or(WIN_W, |r| r.x / Z);
        b.rect(BOX_HEADER_RULE, strip_x - 17.0, 11.5, 1.0, 20.0);
        let view = self.devices.wins.analyzer.get(&d.id).copied().unwrap_or_default();
        let hold_x = strip_x - 33.0 - 77.5;
        b.pill(PILL_GHOST_LATCH, ACT_HOLD, 0, hold_x, 10.5, 77.5, "Hold view", self.devices.wins.held.contains(&d.id));
        let widths = [41.0, 48.0, 38.0];
        let mut x = hold_x - 33.0 - (widths.iter().sum::<f32>() + 2.0 * 8.0);
        for (i, (a, w)) in Analyzer::ALL.iter().zip(widths).enumerate() {
            b.pill(PILL_GHOST, ACT_ANALYZER, i as i32, x, 10.5, w, a.label(), *a == view);
            x += w + 8.0;
        }
        let row = Row::main(462.0);
        let (knobs, pickers) = self.eq_band_items(d, def, v, 72.0, 62.0);
        let mut first = vec![Item::Blank(172.0)];
        first.extend(knobs);
        // The band's own shape leads the Type / Slope cluster.
        let mut second = vec![Item::Blank(86.0)];
        second.extend(pickers);
        self.win_card(d, def, b, row, vec![(first, 14.0), (second, 14.0), (vec![Item::Knob(v.output, "Output", 1, 62.0, false)], 14.0)]);
        let (xs, _) = gutter(INSET, WIN_W - 2.0 * INSET, &[172.0 + 72.0 * 2.0 + 62.0 + 42.0, 86.0 + 96.0 * 2.0 + 28.0, 62.0]);
        self.eq_band_field(d, def, v, b, row, xs[0], 172.0);
        let mut sq = self.eq_shape_card(d, def, v, accent_of(b), 78.0 * Z);
        sq.x = (xs[1] + 4.0) * Z;
        sq.y = (row.top() + 4.0) * Z;
        sq.well = true;
        b.square = sq;
        584.0
    }

    fn character_eq_window(&self, d: &Device, def: &'static DeviceDef, b: &mut Builder) -> f32 {
        let Some(View::Eq(v)) = def.layout.view else { return 644.0 };
        let idx = |id: &str| def.index_of(id).map_or(-1, |i| i as i32);
        let val = |id: &str| def.param(id).map_or(0.0, |p| self.panel_value(d, def, p));
        // Header: the bank picker, then the band strip.
        let bank = v.bank.and_then(|bp| def.param(bp).map(|p| p.format(val(bp)))).unwrap_or_default();
        let from = b.pills.len();
        self.win_header(b, vec![(PILL_BAND, -1, self.eq_band_pills(d, def, v), 8.0, true)]);
        self.dim_parked(d, def, v, b, from);
        let strip_x = b.boxes.last().map_or(WIN_W, |r| r.x / Z);
        b.rect(BOX_HEADER_RULE, strip_x - 17.0, 11.5, 1.0, 20.0);
        let p = b.pill(PILL_PICKER, v.bank.map_or(-1, idx), 0, strip_x - 34.0, 9.5, 0.0, &bank, false);
        p.name = s("Bank");
        p.w = -1.0; // right-aligned at x

        // Main card: band field + band knobs | type, slope | output, auto gain, analyser view.
        let view = self.devices.wins.analyzer.get(&d.id).copied().unwrap_or_default();
        let row = Row::main(410.0);
        let (knobs, pickers) = self.eq_band_items(d, def, v, 72.0, 60.0);
        let mut first = vec![Item::Blank(168.0)];
        first.extend(knobs);
        let auto = v.auto_gain.unwrap_or("auto_gain");
        self.win_card(
            d,
            def,
            b,
            row,
            vec![
                (first, 10.0),
                (pickers, 10.0),
                (
                    vec![
                        Item::Knob(v.output, "Output", 1, 60.0, false),
                        Item::Toggle(auto, "Auto Gain", false),
                        Item::Picker(MENU_ANALYZER, view.label().to_string(), "Analyzer", 60.0, false),
                    ],
                    10.0,
                ),
            ],
        );
        let (xs, _) = gutter(INSET, WIN_W - 2.0 * INSET, &[168.0 + 72.0 * 2.0 + 60.0 + 30.0, 98.0 * 2.0 + 10.0, 60.0 + 56.0 + 60.0 + 20.0]);
        self.eq_band_field(d, def, v, b, row, xs[0], 168.0);

        // Rack card: the section switch and the selected section's controls.
        let rack = Row::rack(528.0);
        let section = self.devices.wins.section.get(&d.id).copied().unwrap_or(0);
        let sel = self.eq_selected(d, v);
        let band = &v.bands[sel];
        let sections =
            Item::ActionPills(ACT_SECTION, vec!["Band", "Dynamics", "Warmth", "Valve", "Tame"], section, "Section", 324.0, false);
        let n = sel + 1;
        // Parameter ids are static strings of the definition: find the selected band's.
        let pid = |suffix: &str| -> &'static str {
            let want = format!("band{n}_{suffix}");
            def.params.iter().find(|p| p.id == want).map_or("", |p| p.id)
        };
        let clusters: Vec<(Vec<Item>, f32)> = match section {
            1 => {
                let dyn_on = band.dyn_on.is_some_and(|p| val(p) >= 0.5);
                let auto_on = val(pid("dyn_auto")) >= 0.5;
                vec![
                    (vec![sections], 10.0),
                    (
                        vec![
                            Item::Toggle(band.dyn_on.unwrap_or(""), "Dynamic", false),
                            Item::Knob(band.dyn_range.unwrap_or(""), "Range", 1, 60.0, !dyn_on),
                        ],
                        10.0,
                    ),
                    (
                        vec![
                            Item::Toggle(pid("dyn_auto"), "Auto", !dyn_on),
                            Item::Knob(pid("dyn_thresh"), "Threshold", 1, 60.0, !dyn_on || auto_on),
                            Item::Knob(pid("dyn_attack"), "Attack", 1, 60.0, !dyn_on),
                            Item::Knob(pid("dyn_release"), "Release", 1, 60.0, !dyn_on),
                        ],
                        10.0,
                    ),
                ]
            }
            2 => vec![
                (vec![sections], 10.0),
                (
                    vec![
                        Item::Knob("warmth_amount", "Warmth", 1, 60.0, false),
                        Item::Pills("warmth_slope", vec![("6 dB/oct", 0.0), ("12 dB/oct", 1.0)], "Slope", 156.0),
                        Item::Knob("warmth_tail", "Tail", 1, 60.0, false),
                    ],
                    10.0,
                ),
            ],
            3 => vec![
                (vec![sections], 10.0),
                (
                    vec![
                        Item::Toggle("valve_enabled", "Valve", false),
                        Item::Knob("valve_trim", "Trim", 1, 60.0, false),
                        Item::Knob("valve_mix", "Mix", 1, 60.0, false),
                    ],
                    10.0,
                ),
                (vec![Item::Pills("oversampling", vec![("2x", 0.0), ("4x", 1.0)], "Oversampling", 84.0)], 10.0),
            ],
            4 => {
                let t = self.devices.wins.tame_band.get(&d.id).copied().unwrap_or(0).min(3);
                let tid = |suffix: &str| -> &'static str {
                    let want = format!("tame{}_{suffix}", t + 1);
                    def.params.iter().find(|p| p.id == want).map_or("", |p| p.id)
                };
                let t_on = val(tid("enabled")) >= 0.5;
                vec![
                    (vec![sections], 10.0),
                    (vec![Item::Toggle("tame_enabled", "Tame", false), Item::Knob("tame_amount", "Amount", 1, 60.0, false)], 10.0),
                    (
                        vec![
                            Item::ActionPills(ACT_TAME_BAND, vec!["1", "2", "3", "4"], t, "Weighting", 136.0, false),
                            Item::Toggle(tid("enabled"), "On", false),
                            Item::Knob(tid("freq"), "Freq", 1, 60.0, !t_on),
                            Item::Knob(tid("q"), "Width", 1, 60.0, !t_on),
                            Item::Knob(tid("weight"), "Weight", 1, 60.0, !t_on),
                        ],
                        10.0,
                    ),
                ]
            }
            _ => {
                let shape = v.shape(val(band.kind));
                let bell = shape == Shape::Bell;
                // Overshoot shapes the corner of shelves and passes only.
                let corner = matches!(shape, Shape::LowShelf | Shape::HighShelf | Shape::HighPass | Shape::LowPass);
                vec![
                    (vec![sections], 10.0),
                    (
                        vec![
                            Item::Knob(band.overshoot.unwrap_or(""), "Overshoot", 1, 60.0, !corner),
                            Item::Toggle(band.prop_q.unwrap_or(""), "Prop Q", !bell),
                        ],
                        10.0,
                    ),
                    // Band drive saturates bells and shelves only.
                    (
                        vec![Item::ActionPills(
                            ACT_DRIVE,
                            vec!["Off", "Low", "Medium", "High"],
                            drive_step(val(pid("drive"))),
                            "Drive",
                            220.0,
                            !matches!(shape, Shape::Bell | Shape::LowShelf | Shape::HighShelf),
                        )],
                        10.0,
                    ),
                ]
            }
        };
        self.win_card(d, def, b, rack, clusters);
        644.0
    }

    // ------------------------------------------------------------ actions

    /// Choice pill of a window: a type pill that shares a mode switch picks through it
    /// (Auto Filter: LP..Notch leave DJ mode, DJ enters it); the rest set the choice.
    pub fn win_choice(&mut self, device: DeviceId, index: i32, choice: i32) {
        let Some((_, def)) = self.panel_def(device) else { return };
        let Some(p) = usize::try_from(index).ok().and_then(|i| def.params.get(i)) else { return };
        let mode = def.layout.picker.iter().find_map(|c| match c {
            dstudio_core::builtin::Control::ModeDropdown { param: q, mode } if *q == p.id => def.index_of(mode),
            _ => None,
        });
        match mode {
            Some(m) if choice as usize >= p.choices.len() => self.panel_pick_mode(device, m),
            Some(_) => self.panel_pick_param(device, index as usize, choice as f32),
            None => self.panel_choice(device, index, choice),
        }
    }

    /// Window-only actions (sections, band latches, band menu picks).
    pub fn win_action(&mut self, device: DeviceId, action: i32, value: i32) {
        let Some((d, def)) = self.panel_def(device) else { return };
        let eqv = match def.layout.view {
            Some(View::Eq(v)) => Some(v),
            _ => None,
        };
        match (action, eqv) {
            (ACT_SECTION, _) => {
                self.devices.wins.section.insert(device, value.clamp(0, 4) as usize);
            }
            (ACT_TAME_BAND, _) => {
                self.devices.wins.tame_band.insert(device, value.clamp(0, 3) as usize);
            }
            (ACT_DRIVE, Some(v)) => {
                let sel = self.eq_selected(d, v);
                let want = format!("band{}_drive", sel + 1);
                let Some(p) = def.params.iter().find(|p| p.id == want) else { return };
                let value = DRIVE_STEPS[value.clamp(0, 3) as usize];
                self.panel_commit(Command::SetDeviceParam { device, param: p.id.into(), value });
            }
            (ACT_ANALYZER, _) => {
                let view = Analyzer::ALL[value.clamp(0, 2) as usize];
                self.devices.wins.analyzer.insert(device, view);
                let _ = self.engine.builtin_set_spectrum(device.0, view.engine_mode());
            }
            (ACT_SEED, _) => {
                let Some(p) = def.param("seed") else { return };
                let span = (p.max - p.min + 1.0).round() as i32;
                let cur = def.value(d, p).round() as i32 - p.min as i32;
                let step = if value < 0 { -1 } else { 1 };
                let next = p.min + (cur + step).rem_euclid(span) as f32;
                self.panel_commit(Command::SetDeviceParam { device, param: p.id.into(), value: next });
            }
            (ACT_TARGET, _) => {
                self.devices.wins.target.insert(device, value.clamp(0, TARGETS.len() as i32 - 1) as usize);
            }
            (ACT_VIEW, _) => self.card_view_select(device, value),
            (ACT_DELAY_BAND, _) => self.set_delay_band(device, value.clamp(0, 5) as usize),
            (ACT_MB_BAND, _) => {
                self.devices.panel.views.entry(device).or_default().band = value.clamp(0, 2) as usize;
            }
            (ACT_LOUD_RESET, _) => {
                let now = self.win_now();
                self.devices.wins.loud_from.insert(device, now);
            }
            (ACT_SPAN, _) => {
                let span = crate::device_window_dynamics::SPANS[value.clamp(0, 2) as usize];
                self.devices.wins.span.insert(device, span);
            }
            (ACT_SPEC_RESET, _) => {
                self.devices.wins.spec.remove(&device);
            }
            (ACT_HOLD, _) => {
                if !self.devices.wins.held.remove(&device) {
                    self.devices.wins.held.insert(device);
                }
            }
            (ACT_BAND_ON, Some(_)) => self.panel_eq(device, 1, 0),
            (ACT_SELECT_BAND, Some(_)) => self.panel_eq(device, 0, value),
            (ACT_LISTEN, Some(v)) => {
                let sel = self.eq_selected(d, v);
                let Some(p) = def.param("listen") else { return };
                let cur = def.value(d, p).round() as usize;
                let next = if cur == sel + 1 { 0.0 } else { (sel + 1) as f32 };
                self.panel_commit(Command::SetDeviceParam { device, param: p.id.into(), value: next });
            }
            _ => return,
        }
        self.ui_dirty = true;
    }

    /// Analyser view menu rows (panel-menu code MENU_ANALYZER): Post, Pre, Off.
    pub(crate) fn win_analyzer_rows(&self, device: DeviceId) -> crate::device_mod::MenuRows {
        let view = self.devices.wins.analyzer.get(&device).copied().unwrap_or_default();
        [Analyzer::Post, Analyzer::Pre, Analyzer::Off]
            .into_iter()
            .map(|a| {
                let i = Analyzer::ALL.iter().position(|x| *x == a).unwrap_or(1) as i32;
                (
                    a.label().to_string(),
                    if a == view { "\u{2713}".to_string() } else { String::new() },
                    Some(crate::devices::MenuPick::Window(device, ACT_ANALYZER, i)),
                    false,
                )
            })
            .collect()
    }

    /// Band picker menu rows (panel-menu code MENU_BANDS).
    pub(crate) fn win_band_rows(&self, device: DeviceId) -> Option<crate::device_mod::MenuRows> {
        let (d, def) = self.panel_def(device)?;
        let Some(View::Eq(v)) = def.layout.view else { return None };
        let sel = self.eq_selected(d, v);
        Some(
            (0..v.bands.len())
                .map(|i| {
                    (
                        format!("Band {}", i + 1),
                        if i == sel { "\u{2713}".to_string() } else { String::new() },
                        Some(crate::devices::MenuPick::Window(device, ACT_SELECT_BAND, i as i32)),
                        false,
                    )
                })
                .collect(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gutter_row_matches_the_window_kit() {
        // Auto Filter's control card: four clusters, 38 pt gutters, half at either end.
        let (xs, rules) = gutter(16.0, 928.0, &[194.0, 210.0, 320.0, 52.0]);
        assert!((xs[0] - 35.0).abs() < 0.5 && (xs[1] - 267.0).abs() < 0.5, "{xs:?}");
        assert!((rules[0] - 248.0).abs() < 1.0, "{rules:?}");
        // Character EQ's Tame section: 22 pt gutters.
        let (xs, _) = gutter(16.0, 928.0, &[324.0, 126.0, 412.0]);
        assert!((xs[0] - 27.0).abs() < 0.5 && (xs[2] - 521.0).abs() < 0.5, "{xs:?}");
    }

    #[test]
    fn meter_ballistics_rise_hold_and_fall() {
        let mut m = Meters::new();
        m.push([1.0, 0.5, 0.0, 0.0], 0.05);
        assert!(m.level[0] > 0.9, "fast attack: {}", m.level[0]);
        assert_eq!(m.hold[0], 1.0);
        // Silence: the bar falls, the hold stays for 1.5 s, then decays.
        for _ in 0..10 {
            m.push([0.0; 4], 0.05);
        }
        assert!(m.level[0] < 0.05, "release: {}", m.level[0]);
        assert_eq!(m.hold[0], 1.0);
        for _ in 0..40 {
            m.push([0.0; 4], 0.05);
        }
        assert!(m.hold[0] < 1.0);
        // Overs light the clip strip briefly.
        m.push([0.0, 0.0, 1.2, 0.0], 0.01);
        assert!(m.since_clip[1] < Meters::CLIP_HOLD);
        assert!(m.since_clip[0] > Meters::CLIP_HOLD);
    }

    #[test]
    fn meter_readouts() {
        assert_eq!(meter_text(0.0), "\u{2212}\u{221e} dB");
        assert_eq!(meter_text(1.0), "0.0 dB");
        assert_eq!(meter_text(0.5), "\u{2212}6.0 dB");
        assert!((meter_norm(1.0) - 1.0).abs() < 1e-6);
        assert_eq!(meter_norm(0.0), 0.0);
    }

    fn controller_with(id: &str) -> (Controller, DeviceId, &'static DeviceDef) {
        let engine = std::sync::Arc::new(dstudio_engine::Engine::new(dstudio_engine::Config::default()).unwrap());
        let mut c = Controller::new(engine, dstudio_core::Project::default());
        let def = dstudio_core::builtin::find(id).unwrap();
        let track = c.project().tracks[0].id;
        assert!(c.execute(Command::AddDevice { track, index: None, device: def.instantiate() }), "{id}");
        let did = c.project().tracks[0].devices.last().unwrap().id;
        (c, did, def)
    }

    fn view(c: &Controller, id: DeviceId, def: &'static DeviceDef) -> WinView {
        let d = c.project().device(id).unwrap().clone();
        c.window_view(&d, def, Color::from_rgb_u8(201, 214, 74)).unwrap()
    }

    fn knob_names(v: &WinView) -> Vec<String> {
        use slint::Model;
        v.knobs.iter().map(|k| k.label.to_string()).collect()
    }

    /// The three windowed devices lay every control inside the window, one control per
    /// parameter, with the display and both meters.
    #[test]
    fn windows_place_every_control_inside_the_face() {
        use slint::Model;
        for id in ["auto-filter", "channeleq", "character-eq"] {
            let (c, did, def) = controller_with(id);
            assert!(has_window(def), "{id}");
            let v = view(&c, did, def);
            assert!(v.visible && v.w > 0.0 && v.h > 0.0, "{id}");
            assert_eq!(v.meters.row_count(), 2, "{id}: In and Out");
            let mut params = std::collections::HashSet::new();
            for k in v.knobs.iter() {
                assert!(k.x >= 0.0 && k.x <= v.w, "{id}: {} at {}", k.label, k.x);
                assert!(k.y > 0.0 && k.y < v.h, "{id}: {} at {}", k.label, k.y);
                assert!(params.insert(k.param), "{id}: {} shown twice", k.label);
            }
            assert!(v.knobs.row_count() >= 4, "{id}");
        }
        let (c, did, def) = controller_with("auto-filter");
        let names = knob_names(&view(&c, did, def));
        assert_eq!(names, ["Freq", "Reso", "Drive", "Env", "Attack", "Release", "LFO", "Rate", "Stereo", "Mix"]);
    }

    /// The modulation family: a plotted display with a drag zone, both meters, and one
    /// control per parameter on a single card row.
    #[test]
    fn field_windows_show_the_plot_and_every_control() {
        use slint::Model;
        for id in ["tremolo", "autopanner", "chorus", "flanger", "phaser", "ring-mod", "character-mod"] {
            let (c, did, def) = controller_with(id);
            assert!(has_window(def), "{id}");
            let v = view(&c, did, def);
            assert!(v.visible && v.w > 0.0 && v.h > 0.0, "{id}");
            assert_eq!(v.meters.row_count(), 2, "{id}: In and Out");
            let mut params = std::collections::HashSet::new();
            for k in v.knobs.iter() {
                assert!(k.x >= 0.0 && k.x <= v.w && k.y > 0.0 && k.y < v.h, "{id}: {} outside", k.label);
                assert!(params.insert(k.param), "{id}: {} shown twice", k.label);
            }
        }
        let (c, did, def) = controller_with("flanger");
        assert_eq!(knob_names(&view(&c, did, def)), ["Rate", "Depth", "Feedback", "Width", "Mix", "Output"]);
        let (c, did, def) = controller_with("ring-mod");
        assert_eq!(knob_names(&view(&c, did, def)), ["Frequency", "Spread", "Bias", "Rectify", "Mix", "Output"]);
    }

    /// Pitch Shifter: the interval caption, the note readouts against A4, the axis, the
    /// formant strip and legend over a card of every control; the Formant switch says Shift
    /// or Keep.
    #[test]
    fn pitch_shifter_window_shows_the_interval_and_every_control() {
        use slint::Model;
        let (mut c, did, def) = controller_with("pitch-shifter");
        assert!(has_window(def));
        let v = view(&c, did, def);
        assert!((v.w / Z - 1200.0).abs() < 0.5 && (v.h / Z - 700.0).abs() < 0.5, "{} x {}", v.w / Z, v.h / Z);
        assert_eq!(knob_names(&v), ["Semitones", "Cents", "Mix"]);
        let texts: Vec<String> = v.display.texts.iter().map(|t| t.text.to_string()).collect();
        for t in ["Interval", "Unison", "A4", "+24 st", "Formant", "Follows the shift", "Interval"] {
            assert!(texts.iter().any(|x| x == t), "missing {t}: {texts:?}");
        }
        assert!(v.pills.iter().any(|p| p.label == "Shift" && !p.active));
        c.execute(Command::SetDeviceParam { device: did, param: "semitones".into(), value: 7.0 });
        c.execute(Command::SetDeviceParam { device: did, param: "formant_preserve".into(), value: 1.0 });
        let v = view(&c, did, def);
        let texts: Vec<String> = v.display.texts.iter().map(|t| t.text.to_string()).collect();
        for t in ["Fifth up", "E5", "Out  659.3 Hz", "Held in place"] {
            assert!(texts.iter().any(|x| x == t), "missing {t}: {texts:?}");
        }
        assert!(v.pills.iter().any(|p| p.label == "Keep" && p.active));
    }

    /// Pitch Corrector: the cent rules labelled in the gutter over a card of every control.
    #[test]
    fn pitch_corrector_window_shows_the_trace_rules_and_every_control() {
        use slint::Model;
        let (c, did, def) = controller_with("pitch-corrector");
        assert!(has_window(def));
        let v = view(&c, did, def);
        assert!((v.w / Z - 1200.0).abs() < 0.5 && (v.h / Z - 700.0).abs() < 0.5, "{} x {}", v.w / Z, v.h / Z);
        assert_eq!(knob_names(&v), ["Speed", "Retune Speed", "Humanize", "Mix"]);
        let texts: Vec<String> = v.display.texts.iter().map(|t| t.text.to_string()).collect();
        for t in ["+100", "+50", "0", "\u{2212}50", "\u{2212}100", "ct"] {
            assert!(texts.iter().any(|x| x == t), "missing {t}: {texts:?}");
        }
        for l in ["Manual", "Project", "Chromatic", "Major", "Minor"] {
            assert!(v.pills.iter().any(|p| p.label == l), "pill {l}");
        }
        assert!(v.display.visible && v.display.paths.row_count() >= 3);
    }

    /// Grains: the buffer with both readouts and its two axes; Cloud and Rhythm swap the
    /// grain card's timing controls; Freeze and the mode sit in the header.
    #[test]
    fn grains_window_shows_the_buffer_and_both_cards() {
        use slint::Model;
        let (mut c, did, def) = controller_with("grains");
        assert!(has_window(def));
        let v = view(&c, did, def);
        assert!((v.w / Z - 1200.0).abs() < 0.5 && (v.h / Z - 700.0).abs() < 0.5, "{} x {}", v.w / Z, v.h / Z);
        assert_eq!(
            knob_names(&v),
            [
                "Position", "Spray", "Drift", "Size", "Density", "Shape", "Pitch", "Spread", "Reverse", "Width", "Feedback", "Blur",
                "Tone", "Space", "Mix", "Output"
            ]
        );
        let grips: Vec<String> = v.display.grips.iter().map(|g| g.label.to_string()).collect();
        assert_eq!(grips, ["Position", "Spray"]);
        let texts: Vec<String> = v.display.texts.iter().map(|t| t.text.to_string()).collect();
        for t in ["Buffer", "+24", "\u{2212}24", "s", "st"] {
            assert!(texts.iter().any(|x| x == t), "missing {t}: {texts:?}");
        }
        for l in ["Cloud", "Rhythm", "Freeze"] {
            assert!(v.pills.iter().any(|p| p.label == l), "header {l}");
        }
        c.execute(Command::SetDeviceParam { device: did, param: "mode".into(), value: 1.0 });
        let v = view(&c, did, def);
        assert_eq!(
            knob_names(&v),
            [
                "Spray", "Drift", "Gate", "Chance", "Shape", "Pitch", "Spread", "Reverse", "Width", "Feedback", "Blur", "Tone", "Space",
                "Mix", "Output"
            ]
        );
        assert!(v.knobs.iter().any(|k| k.label == "Spread" && k.inert));
        for k in v.knobs.iter() {
            assert!(k.y > 0.0 && k.y < v.h, "{} outside", k.label);
        }
    }

    /// Cost of rebuilding a field window against its card, per device
    /// (`cargo test -p dstudio-app --release bench_field_windows -- --ignored --nocapture`).
    #[test]
    #[ignore]
    fn bench_field_windows() {
        let n = 500;
        for id in ["tremolo", "autopanner", "chorus", "flanger", "phaser", "ring-mod", "character-mod"] {
            let (c, did, def) = controller_with(id);
            let d = c.project().device(did).unwrap().clone();
            let t0 = std::time::Instant::now();
            for _ in 0..n {
                std::hint::black_box(c.builtin_card(&d, def));
            }
            let card = t0.elapsed().as_secs_f64() * 1e6 / n as f64;
            let t0 = std::time::Instant::now();
            for _ in 0..n {
                std::hint::black_box(view(&c, did, def));
            }
            let win = t0.elapsed().as_secs_f64() * 1e6 / n as f64;
            println!("{id:<11} card {card:7.1} us   window {win:7.1} us");
        }
    }

    /// The dynamics windows: the level display with its threshold grip, a transfer square,
    /// every control once inside the face, and span pills that pick the history window.
    #[test]
    fn dynamics_windows_show_levels_and_every_control() {
        use slint::Model;
        for id in ["compressor", "limiter", "gate", "glue-compressor", "character-compressor"] {
            let (mut c, did, def) = controller_with(id);
            assert!(has_window(def), "{id}");
            let v = view(&c, did, def);
            assert!(v.display.visible && v.display.zones.row_count() == 1, "{id}: threshold drag");
            assert!(v.square.visible && v.square.paths.row_count() > 0, "{id}: transfer square");
            let mut params = std::collections::HashSet::new();
            for k in v.knobs.iter() {
                assert!(k.x >= 0.0 && k.x <= v.w && k.y > 0.0 && k.y < v.h, "{id}: {} outside", k.label);
                assert!(params.insert(k.param), "{id}: {} shown twice", k.label);
            }
            let lit = |c: &Controller| {
                view(c, did, def).pills.iter().filter(|p| p.param == ACT_SPAN && p.active).map(|p| p.label.to_string()).collect::<Vec<_>>()
            };
            c.win_action(did, ACT_SPAN, 0);
            assert_eq!(lit(&c), ["1s"], "{id}");
            c.win_action(did, ACT_SPAN, 2);
            assert_eq!(lit(&c), ["10s"], "{id}");
        }
    }

    /// Character Compressor: each model lays out its own main card (Opto's Comp / Limit
    /// pills, Drive and Oversampling for the coloured models), and the Section pills switch
    /// the rack card; every control sits once inside the face.
    #[test]
    fn character_compressor_window_follows_model_and_section() {
        use slint::Model;
        let (mut c, did, def) = controller_with("character-compressor");
        let model = def.index_of("model").unwrap() as i32;
        let check = |c: &Controller, what: &str| {
            let v = view(c, did, def);
            assert!((v.h / Z - 640.0).abs() < 0.5, "{what}: {}", v.h / Z);
            let mut params = std::collections::HashSet::new();
            for k in v.knobs.iter() {
                assert!(k.x >= 0.0 && k.x + 40.0 * Z <= v.w && k.y > 0.0 && k.y < v.h, "{what}: {} outside", k.label);
                assert!(params.insert(k.param), "{what}: {} shown twice", k.label);
            }
            for p in v.pills.iter() {
                assert!(p.x >= 0.0 && p.x + p.w <= v.w, "{what}: pill {} outside", p.label);
            }
            knob_names(&v)
        };
        assert_eq!(
            check(&c, "Clean"),
            ["Threshold", "Ratio", "Knee", "Attack", "Release", "Mix", "Makeup", "Hold", "Range", "Lookahead", "Stereo Link"]
        );
        c.win_choice(did, model, 1);
        assert!(check(&c, "Vari-Mu").contains(&"Drive".to_string()));
        c.win_choice(did, model, 4);
        let opto = check(&c, "Opto");
        assert!(!opto.contains(&"Ratio".to_string()) && !opto.contains(&"Hold".to_string()), "{opto:?}");
        assert!(view(&c, did, def).pills.iter().any(|p| p.label == "Limit"));
        c.win_action(did, ACT_SECTION, 1);
        assert!(check(&c, "Sidechain").contains(&"Bell Q".to_string()));
        c.win_action(did, ACT_SECTION, 2);
        let level = check(&c, "Level");
        assert!(level.contains(&"Weight".to_string()) && level.contains(&"Tilt".to_string()), "{level:?}");
        c.win_action(did, ACT_TAME_BAND, 3);
        check(&c, "Level band 4");
    }

    /// Mastering Limiter: the target picker moves the loudness lane's ladder, the caption
    /// reads the short-term level of the output, and Reset starts the measurement again.
    #[test]
    fn mastering_limiter_measures_loudness_against_the_target() {
        use slint::Model;
        let (mut c, did, def) = controller_with("mastering-limiter");
        let texts = |c: &Controller| view(c, did, def).display.texts.iter().map(|t| t.text.to_string()).collect::<Vec<_>>();
        assert!(texts(&c).contains(&"\u{2212}14".to_string()), "{:?}", texts(&c));
        c.win_action(did, ACT_TARGET, 1);
        assert!(texts(&c).contains(&"\u{2212}23".to_string()));
        assert!(view(&c, did, def).pills.iter().any(|p| p.param == MENU_TARGET && p.label == "Broadcast"));
        // Four seconds at -14 dB out read as -14.0 short-term.
        let h: std::collections::VecDeque<_> = (0..240).map(|i| (i as f64 / 60.0, -10.0, -14.0, -2.0)).collect();
        c.devices.wins.history.insert(did, h);
        c.devices.wins.epoch = Some(Instant::now() - std::time::Duration::from_secs(4));
        assert!(texts(&c).iter().any(|t| t == "\u{2212}14.0 LUFS"), "{:?}", texts(&c));
        c.win_action(did, ACT_LOUD_RESET, 0);
        assert!(!texts(&c).iter().any(|t| t.ends_with("LUFS")));
        let ceiling = view(&c, did, def).display.grips.iter().next().unwrap();
        assert_eq!((ceiling.label.as_str(), ceiling.value.as_str()), ("Ceiling", "\u{2212}1.0 dBTP"));
    }

    /// Transient Shaper: the scope fills the display, one card holds every control but the
    /// two times, which are the shape picture's handles (drag zones).
    #[test]
    fn transient_shaper_window_sets_times_from_the_picture() {
        use slint::Model;
        let (mut c, did, def) = controller_with("transient-shaper");
        assert!(has_window(def));
        let v = view(&c, did, def);
        assert!((v.h / Z - 440.0).abs() < 0.5 && (v.w / Z - 880.0).abs() < 0.5, "{} x {}", v.w / Z, v.h / Z);
        assert_eq!(knob_names(&v), ["Attack", "Sustain", "Sense", "Mix", "Output"]);
        assert!(v.display.visible && v.display.paths.row_count() > 3);
        let zones: Vec<_> = v.square.zones.iter().collect();
        let idx = |id: &str| def.index_of(id).unwrap() as i32;
        assert_eq!(zones.len(), 2);
        assert_eq!((zones[0].param_x, zones[0].param_y), (idx("attack_time"), idx("attack")));
        assert_eq!((zones[1].param_x, zones[1].param_y), (idx("sustain_time"), idx("sustain")));
        for z in &zones {
            assert!(z.x + z.w / 2.0 >= 0.0 && z.x + z.w / 2.0 <= v.square.w && z.y + z.h / 2.0 <= v.square.h);
        }
        // Raising Attack lifts its handle.
        let y0 = zones[0].y;
        c.execute(Command::SetDeviceParam { device: did, param: "attack".into(), value: 80.0 });
        assert!(view(&c, did, def).square.zones.iter().next().unwrap().y < y0);
        c.win_action(did, ACT_HOLD, 0);
        assert!(view(&c, did, def).pills.iter().any(|p| p.param == ACT_HOLD && p.active));
    }

    /// Multiband Compressor: the band field's handles edit the selected band's thresholds
    /// and the crossovers; the band picker switches the rack card to another band.
    #[test]
    fn multiband_window_edits_the_selected_band() {
        use slint::Model;
        let (mut c, did, def) = controller_with("multiband-compressor");
        assert!(has_window(def));
        let v = view(&c, did, def);
        assert!((v.h / Z - 640.0).abs() < 0.5 && (v.w / Z - WIN_W).abs() < 0.5, "{} x {}", v.w / Z, v.h / Z);
        assert_eq!(
            knob_names(&v),
            [
                "Input",
                "Low Crossover",
                "High Crossover",
                "Depth",
                "Time",
                "Upward",
                "Downward",
                "Mix",
                "Output",
                "Band Input",
                "Threshold",
                "Ratio",
                "Up Threshold",
                "Up Ratio",
                "Attack",
                "Release",
                "Makeup"
            ]
        );
        let idx = |id: &str| def.index_of(id).unwrap() as i32;
        let zones: Vec<_> = v.display.zones.iter().collect();
        // Two threshold lines per band, then the two crossovers.
        assert_eq!(zones.len(), 8);
        assert_eq!(zones[0].param_y, idx("low_threshold"));
        assert_eq!(zones[5].param_y, idx("high_up_threshold"));
        assert_eq!((zones[6].param_x, zones[7].param_x), (idx("low_crossover"), idx("high_crossover")));
        assert!(v.square.visible && v.square.paths.row_count() > 0);
        assert!(v.knobs.iter().any(|k| k.param == idx("low_threshold")));
        // Picking Mid moves the rack to the mid band's parameters and lights its pills.
        c.win_action(did, ACT_MB_BAND, 1);
        let v = view(&c, did, def);
        assert!(v.pills.iter().any(|p| p.param == MENU_MB_BAND && p.label == "Mid"));
        assert!(v.knobs.iter().any(|k| k.param == idx("mid_up_ratio")) && !v.knobs.iter().any(|k| k.param == idx("low_threshold")));
        assert_eq!(v.display.grips.iter().filter(|g| g.hot).count(), 2);
        assert_eq!(c.win_mb_band_rows(did)[1].1, "\u{2713}");
        // A narrow low band (crossover at its floor) drops its handles.
        c.execute(Command::SetDeviceParam { device: did, param: "low_crossover".into(), value: 20.0 });
        assert_eq!(view(&c, did, def).display.zones.iter().count(), 6);
    }

    /// Delay: the field fills the display with a caption above it, the header carries the
    /// latches, views and mode, and the rack follows the selected repeat-EQ band.
    #[test]
    fn delay_window_shows_the_field_and_the_selected_band() {
        use slint::Model;
        let (mut c, did, def) = controller_with("delay");
        assert!(has_window(def));
        let v = view(&c, did, def);
        assert!((v.w / Z - WIN_W).abs() < 0.5 && (v.h / Z - 640.0).abs() < 0.5, "{} x {}", v.w / Z, v.h / Z);
        assert!(v.display.visible && v.display.pointer && v.display.paths.row_count() > 3);
        assert!(v.display.texts.iter().any(|t| t.text == "Echoes"));
        let idx = |id: &str| def.index_of(id).unwrap() as i32;
        for id in ["link", "sync", "dry", "mode"] {
            assert!(v.pills.iter().any(|p| p.param == idx(id)), "{id} in the header");
        }
        assert!(v.knobs.iter().any(|k| k.param == idx("eq2Freq")));
        // Picking the High Shelf band moves the rack and names it on the picker.
        c.win_action(did, ACT_DELAY_BAND, 4);
        let v = view(&c, did, def);
        assert!(v.knobs.iter().any(|k| k.param == idx("eq4Gain")) && !v.knobs.iter().any(|k| k.param == idx("eq2Gain")));
        assert!(v.pills.iter().any(|p| p.param == MENU_DELAY_BAND && p.label == "High Shelf"));
        assert_eq!(c.win_delay_band_rows(did)[4].1, "\u{2713}");
        // The EQ view retitles the display.
        c.win_action(did, ACT_VIEW, 1);
        let v = view(&c, did, def);
        assert!(v.display.texts.iter().any(|t| t.text == "Feedback Filter"));
        assert!(v.pills.iter().any(|p| p.param == ACT_VIEW && p.index == 1 && p.active));
        // Pointer events from the window hit-test the window's plot, not the card's well.
        let card = c.field_size(def);
        c.devices.panel.canvas.window = true;
        assert_eq!(c.field_size(def), plot_size(def).unwrap());
        assert!(c.field_size(def).0 > card.0);
    }

    /// Reverb: a 1200 pt window; the Filter range drags Low Cut and High Cut, and the shimmer
    /// controls light only in Shimmer mode.
    #[test]
    fn reverb_window_drags_the_filter_range() {
        use slint::Model;
        let (mut c, did, def) = controller_with("reverb");
        let v = view(&c, did, def);
        assert!((v.w / Z - 1200.0).abs() < 0.5 && (v.h / Z - 700.0).abs() < 0.5, "{} x {}", v.w / Z, v.h / Z);
        assert_eq!(knob_names(&v), ["Decay", "Damping", "Size", "Pre-Delay", "Width", "Diffusion", "Mod", "Rate", "Shimmer", "Mix"]);
        let idx = |id: &str| def.index_of(id).unwrap() as i32;
        let zones: Vec<_> = v.square.zones.iter().collect();
        assert_eq!((zones[0].param_x, zones[1].param_x), (idx("lowcut2"), idx("highcut2")));
        assert!(zones[0].x < zones[1].x && !v.square.well);
        assert!(v.pills.iter().filter(|p| p.param == idx("shift1") || p.param == idx("shift2")).all(|p| p.inert));
        c.execute(Command::SetDeviceParam { device: did, param: "mode2".into(), value: dstudio_core::builtin::reverb::SHIMMER as f32 });
        let v = view(&c, did, def);
        assert!(v.pills.iter().filter(|p| p.param == idx("shift1")).all(|p| !p.inert));
        // Raising Low Cut moves its handle right.
        let x0 = zones[0].x;
        c.execute(Command::SetDeviceParam { device: did, param: "lowcut2".into(), value: 400.0 });
        assert!(view(&c, did, def).square.zones.iter().next().unwrap().x > x0);
    }

    /// Echo: the tape strip marks every playback head, Throw is a momentary pill, and the
    /// Character latch swaps the rack between the tail filters and ducking and the character.
    #[test]
    fn echo_window_shows_the_tape_and_swaps_the_rack() {
        use slint::Model;
        let (mut c, did, def) = controller_with("echo");
        assert!(has_window(def));
        let v = view(&c, did, def);
        assert!((v.w / Z - WIN_W).abs() < 0.5 && (v.h / Z - 760.0).abs() < 0.5, "{} x {}", v.w / Z, v.h / Z);
        let d = c.project().device(did).unwrap().clone();
        let heads = c.echo_picture(&d, def).heads.len();
        assert!(v.square.texts.iter().filter(|t| t.text.parse::<usize>().is_ok()).count() == heads && heads > 0);
        let idx = |id: &str| def.index_of(id).unwrap() as i32;
        assert!(v.pills.iter().any(|p| p.kind == PILL_MOMENTARY && p.param == idx("throw")));
        assert!(v.knobs.iter().any(|k| k.param == idx("tail_hp_l")) && !v.knobs.iter().any(|k| k.param == idx("art_hiss")));
        c.win_action(did, ACT_SECTION, 1);
        let v = view(&c, did, def);
        assert!(v.knobs.iter().any(|k| k.param == idx("art_hiss")) && !v.knobs.iter().any(|k| k.param == idx("tail_hp_l")));
        assert!(v.pills.iter().any(|p| p.param == ACT_SECTION && p.active && p.index == 0));
        // Throw is on only while held.
        c.panel_latch(did, idx("throw"), 1);
        assert!(view(&c, did, def).pills.iter().any(|p| p.kind == PILL_MOMENTARY && p.active));
        c.panel_latch(did, idx("throw"), 2);
        assert!(!view(&c, did, def).pills.iter().any(|p| p.kind == PILL_MOMENTARY && p.active));
    }

    /// The waveshaper windows read their transfer and every card control; the caption
    /// follows the output gain.
    #[test]
    fn wave_windows_show_the_transfer_and_every_control() {
        use slint::Model;
        for (id, knobs, pills) in [
            ("distortion", &["drive", "bias", "tone", "output", "mix", "output_gain"][..], "mode"),
            ("tapesaturation", &["drive", "compression", "warmth", "noise", "mix", "output_gain"][..], "mode"),
            ("soft-clipper", &["input", "ceiling", "softness", "mix", "output_gain"][..], "shape"),
        ] {
            let (mut c, did, def) = controller_with(id);
            assert!(has_window(def), "{id}");
            let v = view(&c, did, def);
            assert!((v.w / Z - WAVE_W).abs() < 0.5 && (v.h / Z - 640.0).abs() < 0.5, "{id} {} x {}", v.w / Z, v.h / Z);
            let idx = |p: &str| def.index_of(p).unwrap() as i32;
            for k in knobs {
                assert!(v.knobs.iter().any(|n| n.param == idx(k)), "{id} {k}");
            }
            let n = def.param(pills).unwrap().choices.len();
            assert_eq!(v.pills.iter().filter(|p| p.param == idx(pills)).count(), n, "{id}");
            assert!(v.display.texts.iter().any(|t| t.text == "Wave"), "{id}");
            let db = |c: &Controller| {
                let d = c.project().device(did).unwrap().clone();
                let (caption, value) = c.wave_caption(&d, def);
                (caption, value.trim_end_matches(" dB").replace('\u{2212}', "-").parse::<f32>().unwrap())
            };
            let (caption, before) = db(&c);
            assert_eq!(caption, if id == "soft-clipper" { "Clipping" } else { "Transfer" }, "{id}");
            c.execute(Command::SetDeviceParam { device: did, param: "output_gain".into(), value: -12.0 });
            let after = db(&c).1;
            if id != "soft-clipper" {
                assert!((after - before + 12.0).abs() < 0.2, "{id} {before} -> {after}");
            }
        }
    }

    /// The Bit Crusher's window: the crushed cycle under its rate caption, the card under
    /// the display only, every control on it.
    #[test]
    fn bitcrusher_window_reads_rate_and_levels() {
        use slint::Model;
        let (mut c, did, def) = controller_with("bitcrusher");
        assert!(has_window(def));
        c.execute(Command::SetDeviceParam { device: did, param: "downsample".into(), value: 2.0 });
        c.execute(Command::SetDeviceParam { device: did, param: "bitdepth".into(), value: 8.0 });
        let v = view(&c, did, def);
        assert!(v.display.texts.iter().any(|t| t.text == "24.0 kHz"));
        assert!(v.display.texts.iter().any(|t| t.text == "and 256 levels"));
        let idx = |p: &str| def.index_of(p).unwrap() as i32;
        for k in ["bitdepth", "dither", "downsample", "jitter", "mix"] {
            assert!(v.knobs.iter().any(|n| n.param == idx(k)), "{k}");
        }
        let card = v.boxes.iter().find(|b| b.kind == BOX_CARD).unwrap();
        let (dx, _, dw, _) = kit(def).display;
        assert!(((card.x + card.w) / Z - (dx + dw)).abs() < 0.5, "{}", (card.x + card.w) / Z);
    }

    /// Utility: no display, three cards with every control, the meters down the face.
    #[test]
    fn utility_window_shows_three_cards_and_every_control() {
        use slint::Model;
        let (mut c, did, def) = controller_with("utility");
        assert!(has_window(def));
        let v = view(&c, did, def);
        assert!((v.w / Z - 1200.0).abs() < 0.5 && (v.h / Z - 700.0).abs() < 0.5, "{} x {}", v.w / Z, v.h / Z);
        assert!(!v.boxes.iter().any(|b| b.kind == BOX_DISPLAY));
        assert_eq!(v.boxes.iter().filter(|b| b.kind == BOX_CARD).count(), 3);
        let idx = |p: &str| def.index_of(p).unwrap() as i32;
        for k in ["width", "bass_mono_freq", "gain", "balance"] {
            assert!(v.knobs.iter().any(|n| n.param == idx(k)), "{k}");
        }
        for t in ["channel_mode", "phase_l", "phase_r", "mono", "bass_mono", "bass_mono_audition", "mute", "dc_filter"] {
            assert!(v.pills.iter().any(|p| p.param == idx(t)), "{t}");
        }
        // The bass-mono controls are inert until Bass Mono is on.
        assert!(v.pills.iter().any(|p| p.param == idx("bass_mono_audition") && p.inert));
        c.execute(Command::SetDeviceParam { device: did, param: "bass_mono".into(), value: 1.0 });
        let v = view(&c, did, def);
        assert!(v.pills.iter().any(|p| p.param == idx("bass_mono_audition") && !p.inert && p.label == "Listen"));
        assert!(v.meters.iter().all(|m| m.h / Z > 500.0));
    }

    /// Stereo Widener: the field caption reads the width at the top of the axis, and the
    /// correlation lane draws its two series once there is history.
    #[test]
    fn widener_window_draws_the_field_and_the_correlation_lane() {
        use slint::Model;
        let (mut c, did, def) = controller_with("widener");
        assert!(has_window(def));
        let v = view(&c, did, def);
        assert!((v.w / Z - 1200.0).abs() < 0.5 && (v.h / Z - 700.0).abs() < 0.5, "{} x {}", v.w / Z, v.h / Z);
        assert!(v.display.texts.iter().any(|t| t.text == "Stereo Field"));
        assert!(v.display.texts.iter().any(|t| t.text == "Correlation"));
        let before = v.display.paths.row_count();
        let h = c.devices.wins.corr.entry(did).or_default();
        for i in 0..360 {
            crate::device_window_widener::push_corr(h, (i as f64 / 60.0, 0.8, 0.2));
        }
        let after = view(&c, did, def).display.paths.row_count();
        assert_eq!(after, before + 2);
        let idx = |p: &str| def.index_of(p).unwrap() as i32;
        for k in ["width2", "lowcut", "midgain", "sidegain", "mix"] {
            assert!(v.knobs.iter().any(|n| n.param == idx(k)), "{k}");
        }
        assert!(v.display.zones.iter().any(|z| z.param_y == idx("width2")));
    }

    /// Tuner: the bar across the display, one In meter, a short card with every control.
    #[test]
    fn tuner_window_shows_the_bar_and_one_meter() {
        use slint::Model;
        let (c, did, def) = controller_with("tuner");
        assert!(has_window(def));
        let v = view(&c, did, def);
        assert!((v.w / Z - 1200.0).abs() < 0.5 && (v.h / Z - 700.0).abs() < 0.5, "{} x {}", v.w / Z, v.h / Z);
        assert_eq!(v.meters.row_count(), 1);
        assert_eq!(v.meters.row_data(0).unwrap().label, "In");
        assert!(v.display.visible && v.display.paths.row_count() > 0);
        let idx = |p: &str| def.index_of(p).unwrap() as i32;
        for p in ["tuning", "reference", "mute_output"] {
            assert!(v.pills.iter().any(|q| q.param == idx(p)), "{p}");
        }
    }

    /// De-Esser, De-Hummer, De-Noiser: the graph at window size with its caption, ladder and
    /// legend, one card with every control, and a drag in the window moves the De-Noiser's
    /// threshold line where the window draws it.
    #[test]
    fn repair_windows_draw_the_graph_and_every_control() {
        use slint::Model;
        for (id, caption, legend) in [("deesser", "Band", "Removing"), ("dehummer", "Hum", "Notches"), ("denoiser", "Noise gate", "Gate")] {
            let (c, did, def) = controller_with(id);
            assert!(has_window(def), "{id}");
            let v = view(&c, did, def);
            assert!((v.w / Z - 1200.0).abs() < 0.5 && (v.h / Z - 696.0).abs() < 0.5, "{id} {} x {}", v.w / Z, v.h / Z);
            for p in def.params {
                let i = def.index_of(p.id).unwrap() as i32;
                assert!(v.knobs.iter().any(|k| k.param == i) || v.pills.iter().any(|q| q.param == i), "{id} {}", p.id);
            }
            for t in [caption, legend, "0 dB", "Hz"] {
                assert!(v.display.texts.iter().any(|x| x.text == t), "{id} {t}");
            }
            assert!(v.display.pointer && !v.display.well, "{id}");
        }
        let (mut c, did, def) = controller_with("denoiser");
        let t0 = c.panel_value(c.project().device(did).unwrap(), def, def.param("threshold").unwrap());
        let (w, h) = crate::panel_graph::WIN_PLOT;
        let y = (-t0 / 80.0) * h * Z;
        c.panel_graph(did, 16, 0.5 * w * Z, y, false, false);
        c.panel_graph(did, 17, 0.5 * w * Z, y - 40.0, false, false);
        c.panel_graph(did, 18, 0.5 * w * Z, y - 40.0, false, false);
        let t1 = c.panel_value(c.project().device(did).unwrap(), def, def.param("threshold").unwrap());
        assert!(t1 > t0 + 3.0, "{t0} -> {t1}");
    }

    /// De-Clipper: the transfer held at the threshold beside the wave, captioned with the
    /// transfer at 0 dBFS, and one card with every control.
    #[test]
    fn declipper_window_shows_the_transfer_and_every_control() {
        use slint::Model;
        let (mut c, did, def) = controller_with("declipper");
        assert!(has_window(def));
        let v = view(&c, did, def);
        assert!((v.w / Z - 1200.0).abs() < 0.5 && (v.h / Z - 696.0).abs() < 0.5, "{} x {}", v.w / Z, v.h / Z);
        let idx = |p: &str| def.index_of(p).unwrap() as i32;
        for k in ["threshold", "ceiling", "mix"] {
            assert!(v.knobs.iter().any(|n| n.param == idx(k)), "{k}");
        }
        assert_eq!(v.pills.iter().filter(|p| p.param == idx("oversampling")).count(), 3);
        assert_eq!(v.meters.row_count(), 2);
        for t in ["Wave", "Reduction"] {
            assert!(v.display.texts.iter().any(|x| x.text == t), "{t}");
        }
        c.execute(Command::SetDeviceParam { device: did, param: "threshold".into(), value: -6.0 });
        c.execute(Command::SetDeviceParam { device: did, param: "mix".into(), value: 0.0 });
        let v = view(&c, did, def);
        assert!(v.display.texts.iter().any(|x| x.text.contains("6.0 dB")), "caption reads the threshold");
    }

    /// Trance Gate: the ring panel and envelope share one interactive canvas, no meters, and
    /// a press on the window's ring toggles the step under it (window geometry, not the card's).
    #[test]
    fn trance_gate_window_toggles_steps_on_its_own_ring() {
        use slint::Model;
        let (mut c, did, def) = controller_with("trance-gate");
        assert!(has_window(def));
        let v = view(&c, did, def);
        assert!((v.w / Z - 1200.0).abs() < 0.5 && (v.h / Z - 700.0).abs() < 0.5, "{} x {}", v.w / Z, v.h / Z);
        assert_eq!(v.meters.row_count(), 0);
        assert!(v.display.interactive && v.display.paths.row_count() > 10);
        assert_eq!(knob_names(&v), ["Attack", "Decay", "Sustain", "Release", "Swing", "Mix"]);
        assert!(v.display.texts.iter().any(|t| t.text == "Envelope"));
        let bits = |c: &Controller| {
            let d = c.project().device(did).unwrap();
            c.panel_value(d, def, def.param("p1_lo").unwrap()).round() as u32
        };
        let before = bits(&c);
        // The middle of step 1 on the window's ring (16 steps), in canvas px.
        let g = crate::device_editors::TG_WIN;
        let (r, a) = (0.8 * g.ring_r(), std::f32::consts::TAU * 0.5 / 16.0);
        let (x, y) = (g.ring_cx() + r * a.sin(), g.ring_cy() - r * a.cos());
        let ev =
            |kind| crate::device_editors::CanvasEvent { kind, x: x * Z, y: y * Z, wx: 0.0, wy: 0.0, shift: false, alt: false, wheel: 0.0 };
        c.panel_editor(did, ev(16));
        c.panel_editor(did, ev(18));
        assert_eq!(bits(&c) ^ before, 1, "step 1 toggled");
        // The same point on the card misses the ring (its ring is smaller and higher).
        c.panel_editor(did, ev(0));
        c.panel_editor(did, ev(2));
        assert_eq!(bits(&c) ^ before, 1, "the card's geometry did not take the window's point");
    }

    /// Spectrum: three views from one history; Reset clears it, Hold view stops taking
    /// frames, the History picker is inert in 2D.
    #[test]
    fn spectrum_window_draws_each_view_from_its_history() {
        use slint::Model;
        let (mut c, did, def) = controller_with("spectrum");
        assert!(has_window(def));
        let idx = |p: &str| def.index_of(p).unwrap() as i32;
        let v = view(&c, did, def);
        assert!((v.w / Z - 1200.0).abs() < 0.5 && (v.h / Z - 700.0).abs() < 0.5, "{} x {}", v.w / Z, v.h / Z);
        assert_eq!(v.meters.row_count(), 1);
        assert_eq!(v.meters.row_data(0).unwrap().label, "Level");
        for p in ["fft_size", "window", "channel_mode", "peak_hold", "color_scheme", "history_depth"] {
            assert!(v.pills.iter().any(|q| q.param == idx(p)), "{p}");
        }
        assert_eq!(v.pills.iter().filter(|q| q.param == idx("display_mode")).count(), 3);
        let empty_3d = v.display.paths.row_count();
        let h = c.devices.wins.spec.entry(did).or_default();
        for i in 1..=200u64 {
            let f: Vec<f32> = (0..crate::analyzer_views::SPECTRUM_BANDS).map(|b| -30.0 - (b as f32 * 0.2) - (i % 7) as f32).collect();
            h.take(i, &f, 128);
        }
        assert_eq!(h.frames.len(), 128);
        let full_3d = view(&c, did, def).display.paths.row_count();
        assert!(full_3d >= empty_3d + 2 * 32, "{empty_3d} -> {full_3d}");
        c.execute(Command::SetDeviceParam { device: did, param: "display_mode".into(), value: 2.0 });
        let split = view(&c, did, def);
        let well = split.boxes.iter().find(|b| b.kind == BOX_DISPLAY).unwrap();
        assert!(well.h / Z < 300.0, "{}", well.h / Z);
        assert!(split.display.paths.row_count() > 4);
        c.execute(Command::SetDeviceParam { device: did, param: "display_mode".into(), value: 0.0 });
        let flat = view(&c, did, def);
        assert!(flat.pills.iter().any(|q| q.param == idx("history_depth") && q.inert));
        assert!(flat.display.texts.iter().any(|t| t.text.starts_with("Tilt")));
        c.win_action(did, ACT_SPEC_RESET, 0);
        assert!(c.win_spec_history(did).is_none());
    }

    /// A full ten-second history draws as one line per series, however many samples it holds.
    #[test]
    fn dynamics_history_draws_once_per_series() {
        use slint::Model;
        let (mut c, did, def) = controller_with("compressor");
        let empty = view(&c, did, def).display.paths.row_count();
        let h: std::collections::VecDeque<_> = (0..600).map(|i| (i as f64 / 60.0, -12.0, -18.0, -6.0)).collect();
        c.devices.wins.history.insert(did, h);
        c.devices.wins.epoch = Some(Instant::now() - std::time::Duration::from_secs(10));
        let full = view(&c, did, def).display.paths.row_count();
        assert!(full > empty && full < empty + 12, "{empty} -> {full}");
    }

    /// Cost of rebuilding a dynamics window with a full history, against its card
    /// (`cargo test -p dstudio-app --release bench_dynamics_windows -- --ignored --nocapture`).
    #[test]
    #[ignore]
    fn bench_dynamics_windows() {
        let n = 500;
        for id in [
            "compressor",
            "limiter",
            "gate",
            "glue-compressor",
            "character-compressor",
            "mastering-limiter",
            "transient-shaper",
            "multiband-compressor",
            "delay",
            "reverb",
            "echo",
            "distortion",
            "tapesaturation",
            "soft-clipper",
            "bitcrusher",
            "utility",
            "widener",
            "tuner",
            "spectrum",
            "trance-gate",
            "declipper",
            "deesser",
            "dehummer",
            "denoiser",
            "pitch-shifter",
            "grains",
            "pitch-corrector",
        ] {
            let (mut c, did, def) = controller_with(id);
            let h: std::collections::VecDeque<_> =
                (0..630).map(|i| (i as f64 / 60.0, -12.0 + (i as f32 * 0.1).sin() * 6.0, -18.0, -6.0)).collect();
            c.devices.wins.history.insert(did, h);
            c.devices.wins.epoch = Some(Instant::now() - std::time::Duration::from_millis(10_500));
            if id == "spectrum" {
                let h = c.devices.wins.spec.entry(did).or_default();
                for i in 1..=128u64 {
                    let f: Vec<f32> =
                        (0..crate::analyzer_views::SPECTRUM_BANDS).map(|b| -20.0 - b as f32 * 0.25 - (i % 9) as f32).collect();
                    h.take(i, &f, 128);
                }
            }
            let d = c.project().device(did).unwrap().clone();
            let t0 = std::time::Instant::now();
            for _ in 0..n {
                std::hint::black_box(c.builtin_card(&d, def));
            }
            let card = t0.elapsed().as_secs_f64() * 1e6 / n as f64;
            let t0 = std::time::Instant::now();
            for _ in 0..n {
                std::hint::black_box(view(&c, did, def));
            }
            let win = t0.elapsed().as_secs_f64() * 1e6 / n as f64;
            println!("{id:<15} card {card:7.1} us   window {win:7.1} us");
        }
    }

    /// Character Mod: two cards; the rack drops the artifacts a circuit lacks, and the
    /// Seed pill plays the next take, wrapping at the end.
    #[test]
    fn character_mod_window_rows_follow_the_circuit() {
        use slint::Model;
        let (mut c, did, def) = controller_with("character-mod");
        let v = view(&c, did, def);
        assert!((v.h / Z - 520.0).abs() < 0.5, "{}", v.h / Z);
        assert_eq!(
            knob_names(&v),
            ["Rate", "Depth", "Mix", "Width", "Drift", "Voice", "Hiss", "Drive", "Darken", "Depth-M", "Depth-S", "Output"]
        );
        let take = |c: &Controller| view(c, did, def).pills.iter().find(|p| p.param == ACT_SEED).map(|p| p.label.to_string());
        assert_eq!(take(&c).as_deref(), Some("Take 1"));
        c.win_action(did, ACT_SEED, 1);
        assert_eq!(take(&c).as_deref(), Some("Take 2"));
        c.win_action(did, ACT_SEED, -1);
        c.win_action(did, ACT_SEED, -1);
        assert_eq!(take(&c).as_deref(), Some("Take 9999"));
        // Panner has no artifacts but Wow-free circuits keep the rest; Ensemble has no Mode.
        let model = def.index_of("model").unwrap() as i32;
        c.win_choice(did, model, 10);
        let names = knob_names(&view(&c, did, def));
        assert!(!names.iter().any(|n| n == "Hiss" || n == "Drive" || n == "Darken"), "{names:?}");
        c.win_choice(did, model, 3);
        let modes = view(&c, did, def).texts.iter().filter(|t| t.text == "Mode").count();
        assert_eq!(modes, 0, "Ensemble has no Mode picker");
    }

    /// Character EQ's lower card shows one section at a time.
    #[test]
    fn character_eq_sections_switch_the_lower_card() {
        let (mut c, did, def) = controller_with("character-eq");
        let mut seen = Vec::new();
        for section in 0..5 {
            c.win_action(did, ACT_SECTION, section);
            let names = knob_names(&view(&c, did, def));
            assert!(!seen.contains(&names), "section {section} repeats another: {names:?}");
            seen.push(names);
        }
        assert!(seen[1].iter().any(|n| n == "Threshold"), "{:?}", seen[1]);
        assert!(seen[4].iter().any(|n| n == "Amount"), "{:?}", seen[4]);
        // A device without a window keeps its card.
        let (c, did, def) = controller_with("device-rack");
        assert!(!has_window(def));
        let d = c.project().device(did).unwrap().clone();
        assert!(c.window_view(&d, def, Color::from_rgb_u8(0, 0, 0)).is_none());
    }

    #[test]
    fn analyzer_frame_follows_holds_and_drains() {
        let mut sp = Spectrum::default();
        let frame = vec![-20.0f32; 128];
        for _ in 0..20 {
            sp.advance(Some(&frame), 0.03, false);
        }
        assert!((sp.shown[0] + 20.0).abs() < 1.0, "attack reaches the frame: {}", sp.shown[0]);
        assert!(sp.on_screen());
        // Held: a new, quieter frame does not move it.
        let quiet = vec![-60.0f32; 128];
        assert!(!sp.advance(Some(&quiet), 0.03, true));
        assert!((sp.shown[0] + 20.0).abs() < 1.0);
        // Released, it follows; with no frames it drains to the floor at 42 dB/s.
        sp.advance(Some(&quiet), 0.03, false);
        assert!(sp.shown[0] < -20.0);
        for _ in 0..100 {
            sp.advance(None, 0.03, false);
        }
        assert!(!sp.on_screen(), "drained: {}", sp.shown[0]);
    }

    /// Drive pills write the selected band's drive steps; the analyser's view and Hold view
    /// are window state, offered on Channel EQ's top bar and Character EQ's picker.
    #[test]
    fn character_eq_drive_and_analyzer_controls() {
        use slint::Model;
        let (mut c, did, def) = controller_with("character-eq");
        c.win_action(did, ACT_DRIVE, 2);
        assert_eq!(c.project().device(did).map(|d| def.value(d, def.param("band1_drive").unwrap())), Some(50.0));
        let v = view(&c, did, def);
        let lit: Vec<String> = v.pills.iter().filter(|p| p.param == ACT_DRIVE && p.active).map(|p| p.label.to_string()).collect();
        assert_eq!(lit, ["Medium"]);
        assert_eq!(drive_step(20.0), usize::MAX, "between steps: no pill lit");
        // Analyzer picker: Post by default, then Off.
        assert!(v.pills.iter().any(|p| p.param == MENU_ANALYZER && p.label == "Post"));
        let rows = c.win_analyzer_rows(did);
        assert_eq!(rows.iter().map(|r| r.0.as_str()).collect::<Vec<_>>(), ["Post", "Pre", "Off"]);
        c.win_action(did, ACT_ANALYZER, 2);
        assert!(view(&c, did, def).pills.iter().any(|p| p.param == MENU_ANALYZER && p.label == "Off"));
        assert!(c.window_spectrum(did).is_none());

        let (mut c, did, def) = controller_with("channeleq");
        let ghosts = |c: &Controller| -> Vec<(String, bool)> {
            view(c, did, def).pills.iter().filter(|p| p.kind >= PILL_GHOST).map(|p| (p.label.to_string(), p.active)).collect()
        };
        assert_eq!(ghosts(&c), [("Hold view".into(), false), ("Pre".into(), false), ("Post".into(), true), ("Off".into(), false)]);
        c.win_action(did, ACT_ANALYZER, 0);
        c.win_action(did, ACT_HOLD, 0);
        assert_eq!(ghosts(&c), [("Hold view".into(), true), ("Pre".into(), true), ("Post".into(), false), ("Off".into(), false)]);
    }
}
