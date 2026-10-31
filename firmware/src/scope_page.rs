//! The stock-replica scope page on the App side (../docs/history/stock-scope-port-plan.md
//! S4/S5): the stock interaction model (stock/re/docs/stock-scope-ui.md sec. 10) and
//! the responsive drag path, drawing through src/scope_ui.rs.
//!
//! Interaction (stock mode):
//! * selection on press: 32x32 zones around the markers (glyph rect grown by
//!   10 px), the stock search order (other group after its current index,
//!   then the same group cycling, then the other group from 0), so repeated
//!   taps on overlapping zones cycle through them; the selected marker turns
//!   filled, the guide lines of the selection show from the press until 2 s
//!   after the last event;
//! * drag (opendm40, owner 2026-10-25: "drag only ONE thing, along ONE
//!   axis, until the finger is lifted"; the stock split every move report:
//!   |dx| <= |dy| moved the selected level item, else the selected time
//!   item, so a sideways pan with vertical wobble also moved the trigger
//!   level): one item along one axis per gesture, until the release. The
//!   first motion decides the axis (scope_ui::drag_axis); the item is the
//!   marker of that axis nearest to the press, whatever is selected
//!   (scope_ui::drag_target: vertical, the level marker whose row is
//!   nearest to the press y: CH1, trigger level, runt level, math offset,
//!   Y cursors; horizontal, the time marker whose column is nearest to the
//!   press x: trigger position (a stopped record: its pan), X cursors, the
//!   FFT centre); it becomes the selection. A 4 px tap tolerance before
//!   the first move; the acquisition holds from the first move (no frame
//!   paint between the finger and the marker);
//! * timebase tap step (Settings "Scope tap step", docs/scope-scale-ux.md
//!   sec. 1): Halves (stock: a tap off the zones, left half slower, right
//!   half faster), Corners (default: only the two bottom corner zones step,
//!   a still hold there repeats; elsewhere a tap does nothing) or Off; the
//!   marker zones win over both. Value-preview tabs in the bottom corners
//!   (Corners: always; Halves: while a press is armed, 2 s after a step and
//!   on the first 3 page entries after power-on); the timebase badge flashes
//!   on every timebase change;
//! * top bar (opendm40, the stock one is not tappable): run badge = RUN/STOP,
//!   timebase badge = timebase list popup, V/div badge = CH1 popup, trigger
//!   badge = Trigger tab; the right panel: V/div + / -, CH1 popup, TRIG ->
//!   Trigger tab;
//! * no page swipe (MENU leaves the page) unless Settings "Scope swipes" is
//!   on (swipes that start outside the drag area: top bar, panel);
//! * alternatives (Settings "Scope drag"): Hold 300 ms (moves count after a
//!   still hold; the beep marks the grab). "Stock" and "Axis lock" both lock
//!   the axis now (above).
//! Responsiveness (owner, 2026-10-13: "dragging the trace lags"): while a
//! drag is active the acquisition is not polled (no new frames, no restart
//! per event); each event re-renders the frame already shown from its
//! cached column rows, shifted / scaled (Graph::flush repaints only the rows
//! that change); the settings reach the hardware once, on release.
//! Pinch: horizontal = timebase, vertical = V/div, 1-2-5 steps, a value
//! bubble and a scaled preview of the shown frame; applied on release.
//! AUTO (autoset) follows the stock AUTO-set (scope_gen_ui_v2_6.h sec. 4)
//! with UF-04 (the user's coupling is kept; DC only if AC finds nothing).

use super::*;
use crate::gesture::{Axis, Gesture};
use crate::scope_ui::{self as su, Model};
use crate::stockfont::{self as sf, rect, Rect};
use crate::touch::Point;

/// Guide lines stay this long after the last drag event / release (stock).
const GUIDE_MS: u64 = 2000;
/// UF-09: two taps within this time (and 30 px) are a double tap.
const DOUBLE_MS: u64 = 400;
/// Movement (px from the press) a touch may wobble and still be a tap; the
/// drag starts beyond it (with the accumulated delta, nothing is lost).
const TAP_TOL: i32 = 4;
/// Pressed-button feedback (one stock pressed colour frame), ms.
const PRESS_MS: u64 = 120;
/// Timebase badge flash / stepped tab pressed style after a step, ms.
const FLASH_MS: u64 = 150;
/// Halves: the tabs show on this many page entries after power-on.
const INTRO_ENTRIES: u8 = 3;
/// Drag modes (Settings "Scope drag").
pub const DRAG_STOCK: u8 = 0;
pub const DRAG_AXIS: u8 = 1;
/// Names of the drag modes without hold-to-grab (Settings "Scope drag";
/// Hold is gesture::grab_ms != 0 with the Stock model, shown as "Hold
/// <ms> ms"; review 7389e8a L4: a stored mode 2 showed as "Axis lock").
pub const DRAG_NAMES: [&str; 2] = ["Stock", "Axis lock"];
/// Persistence (Display tab: OFF, MIN, 1S, INF): layer period (ms).
const PERSIST_MS: [u64; 4] = [0, 40, 500, u64::MAX];

/// A touch on the scope page's drag area.
#[derive(Clone, Copy)]
pub(super) struct Touch {
    p0: Point,
    /// Last report seen / last point whose delta was applied.
    last: Point,
    applied: Point,
    far: i32,
    moving: bool,
    /// The press selected a marker zone (no timebase step on release).
    hit: bool,
    t0: u64,
    /// Settings / cursors at the press (undo for a second finger); the
    /// settings are rebased after each Corners hold-repeat step.
    snap: scope::Settings,
    cur: ui::Cursors,
    /// Stopped scope: stored-record anchor at the press (pan base), and the
    /// pan so far (px).
    anchor: Option<usize>,
    pan: i32,
    /// The axis the first motion decided (kept until the release).
    axis: Option<Axis>,
    /// Hold mode: grabbed.
    grabbed: bool,
    /// Combined view: sub-row remainder of vertical moves.
    acc: i32,
    /// What moved (DBG:GEST? code) and its travel.
    code: u8,
    travel: i32,
    /// Tap step of the press (scope_ui::Arm: the side armed when no marker
    /// was hit, the hold repeats done).
    arm: su::Arm,
}

/// A touch on the timebase list (drag scroll / row tap).
#[derive(Clone, Copy)]
pub(super) struct ListTouch {
    p0: Point,
    y: i16,
    far: i32,
    acc: i32,
}

impl Touch {
    /// Still a tap: within the tap tolerance, not dragging.
    fn still(&self) -> bool {
        !self.moving && self.far <= TAP_TOL
    }
}

#[derive(Clone, Copy)]
pub(super) struct Pinch {
    axis: Axis,
    /// Index at the start (tb / range) and the previewed one; always inside
    /// the table of `axis` (scope_ui::pinch_step).
    i0: usize,
    i: usize,
    /// Span ratio (now / start): the live stretch of PINCH_LIVE.
    r: f32,
}

/// Pinch mode (scope_ui::PINCH_*; DBG:PINCH, RAM only).
static PINCH_MODE: core::sync::atomic::AtomicU8 = core::sync::atomic::AtomicU8::new(su::PINCH_STEP);

pub fn pinch_mode() -> u8 {
    PINCH_MODE.load(core::sync::atomic::Ordering::Relaxed)
}

pub fn set_pinch_mode(m: u8) {
    PINCH_MODE.store(m.min(su::PINCH_LIVE), core::sync::atomic::Ordering::Relaxed);
}

/// Column rows (280-row model) of the frame shown, per engine column, and
/// what the frame was captured with: the base of every preview.
struct Cache {
    seg: [(i16, i16); 400],
    /// The GEN trace's and the DMM CH2's rows per engine column
    /// (docs/scope-gen-channel.md).
    gseg: [(i16, i16); 400],
    c2seg: [(i16, i16); 400],
    valid: bool,
    pos: i32,
    hpos: i32,
    range: usize,
    tb: usize,
    /// Persistence layers (model rows per graph column) and the swap time.
    ps_a: [(i16, i16); 399],
    ps_b: [(i16, i16); 399],
    ps_t: u64,
}

// In SRAM (.dmabuf, NOLOAD) on the device (4.8 KB out of TCM); set up by
// `sp_init`.
#[cfg_attr(target_os = "none", link_section = ".dmabuf")]
static mut CACHE: Cache = Cache {
    seg: [su::EMPTY; 400],
    gseg: [su::EMPTY; 400],
    c2seg: [su::EMPTY; 400],
    valid: false,
    pos: 0,
    hpos: 200,
    range: 6,
    tb: 13,
    ps_a: [su::EMPTY; 399],
    ps_b: [su::EMPTY; 399],
    ps_t: 0,
};

fn cache() -> &'static mut Cache {
    unsafe { &mut *core::ptr::addr_of_mut!(CACHE) }
}

/// Set the page buffers up in place (their statics are not loaded on the
/// device): the frame cache here, the graph in scope_ui.
pub(super) fn sp_init() {
    let c = cache();
    for i in 0..400 {
        c.seg[i] = su::EMPTY;
        c.gseg[i] = su::EMPTY;
        c.c2seg[i] = su::EMPTY;
    }
    for i in 0..399 {
        c.ps_a[i] = su::EMPTY;
        c.ps_b[i] = su::EMPTY;
    }
    c.valid = false;
    c.pos = 0;
    c.hpos = 200;
    c.range = 6;
    c.tb = 13;
    c.ps_t = 0;
    su::init();
    crate::scope_ch2::init();
}

/// AUTO-set state (stock mode 4).
#[derive(Clone, Copy)]
pub(super) struct Autoset {
    phase: u8,
    idx: usize,
    round: u8,
    saved: scope::Settings,
    /// Records before this time are not used (range relays / offset PWM
    /// settle: stock 200 ms per candidate).
    settle: u64,
    /// Give up (stock 15 s abort timer).
    abort: u64,
    /// Phase 2 measured once more at the finer result range (refine).
    refined: bool,
    /// Phase 2: finest range allowed (a range that clipped is not taken).
    min_r: u8,
}

/// Math display (stock math popup / pill; UF-11: the FFT is computed by
/// src/fft.rs from the scope record, ABS from the shown columns).
#[derive(Clone, Copy)]
pub(super) struct Math {
    pub on: bool,
    /// 5 FFT, 6 ABS (stock S+0xd4).
    pub mode: u8,
    pub colour: u8,
    /// FFT dB/div (su::FFT_DB index) and the V/div of ABS / FFT in Vrms
    /// (su::VDIV_LABELS index).
    pub db: u8,
    pub vdiv: u8,
    pub vrms: bool,
    /// FFT window (fft::WIN_NAMES) and Hz/div choice (0..3 ascending, 3 =
    /// the widest 1-2-5 step that fits 0 .. Nyquist).
    pub win: u8,
    pub hz: u8,
    /// Offsets (rows): FFT 0 dBV / 0 Vrms row and ABS 0 V row = 140 + off.
    pub fft_off: i32,
    pub abs_off: i32,
    /// FFT centre frequency (Hz; clamped to the span when drawn, 0 = the
    /// span starts at DC).
    pub ctr: f32,
}

impl Math {
    pub const DEFAULT: Math = Math { on: false, mode: 5, colour: 1, db: 9, vdiv: 9, vrms: false, win: crate::fft::W_HANN, hz: 3, fft_off: -105, abs_off: 105, ctr: 0.0 };
    /// Offset of the active mode.
    fn off(&self) -> i32 {
        if self.mode == 5 { self.fft_off } else { self.abs_off }
    }
}

/// The spectrum job and result: 4 KB, TCM .bss (all-zero `Fft::new`). Its
/// 8 KiB work buffer is lent by the disk buffer arena (calbackup.rs
/// `fft_work`).
static mut FFT: crate::fft::Fft = crate::fft::Fft::new();
/// A frame was dropped because a disk job held the work buffer: ask for it
/// again (a stopped scope re-emits its record) once the buffer is back.
static mut FFT_RETRY: bool = false;
const _: () = assert!(crate::diskbuf::FFT_WORK == crate::fft::N);

fn fft() -> &'static mut crate::fft::Fft {
    unsafe { &mut *core::ptr::addr_of_mut!(FFT) }
}

/// The FFT work buffer (diskbuf.rs `fft_work`: the disk buffer arena;
/// `fresh` = a new job's load). None while a disk job holds it, or a disk
/// user wrote it since the load: the job in flight, if any, is dropped (the
/// last result stays) and the frame is asked again.
#[inline(always)]
fn fft_work(fresh: bool) -> Option<&'static mut crate::fft::Work> {
    let w = crate::diskbuf::fft_work(fresh);
    if w.is_none() {
        fft().abort();
        unsafe { FFT_RETRY = true };
    }
    w
}

/// FFT Hz/div choices (ascending) for a spectrum up to `nyq` Hz: the widest
/// 1-2-5 value whose 10 divisions fit 0 .. nyq and the three below it.
fn hz_opts(nyq: f32) -> [f32; 4] {
    let full = crate::fft::floor125(nyq / 10.0);
    let a = crate::fft::down125(full);
    let b = crate::fft::down125(a);
    [crate::fft::down125(b), b, a, full]
}

/// Scope page state.
pub(super) struct Sp {
    pub sel_v: u8,
    pub sel_h: u8,
    /// Group of the last selection (false = level / V, true = time / H).
    last_h: bool,
    pub guide_until: u64,
    pub touch: Option<Touch>,
    pub pinch: Option<Pinch>,
    pub drag_mode: u8,
    pub swipes: bool,
    pub press_fx: bool,
    pressed: u8,
    pressed_until: u64,
    pub colour: u8,
    pub bright: u8,
    pub grid: u8,
    pub persist: u8,
    pub autoset: Option<Autoset>,
    /// Drag in progress with the acquisition held (no polling).
    frozen: bool,
    /// Open stock popup (modal; the page under it is not repainted), the
    /// Measure cell under the finger, a slider being dragged (0 brightness,
    /// 1 grid), the "Sampling" box shown.
    pub popup: su::Popup,
    /// A closed popup to repaint over (`sp_uncover`; bit 0: the CH1 label
    /// was drawn grey, bit 1: the math label, bit 7: pending).
    pub uncover: u8,
    pressed_cell: Option<u8>,
    slider: Option<u8>,
    sampling: bool,
    /// Pinch bubble shown / to be (re)drawn.
    bubble: bool,
    bubble_dirty: bool,
    /// New measurement results not all drawn yet (`sp_cells_step`).
    pub cells_due: bool,
    /// A frame was built and its paint not started yet (scope_poll: the
    /// paint follows in the next pass without a frame).
    pub paint_due: bool,
    /// Combined view: no frame is painted before this time, ms
    /// (App::sp_throttled).
    pub frame_next: u64,
    /// Settings "Scope tap step" (scope_ui::TAP_*).
    pub tap_mode: u8,
    /// Halves: tabs shown until (after a step, on an intro page entry).
    tabs_until: u64,
    /// The side just stepped (pressed style) until `step_until`.
    step_until: u64,
    step_side: bool,
    /// Timebase badge flash: the timebase last seen, flash end.
    flash_tb: u8,
    flash_until: u64,
    /// Halves intro: page entries counted since power-on.
    entries: u8,
    /// Timebase list touch; timebase badge drawn while the list covers the
    /// page (timebase, flash).
    list: Option<ListTouch>,
    tb_drawn: (u8, bool),
    /// The REC mark's blink phase drawn while the page is covered (a popup
    /// leaves the top bar visible); None = not drawn since it was covered.
    rec_drawn: Option<bool>,
    /// UF-08: the press is on the cursor tab; the selection before the
    /// cursor tab turned the cursors on. UF-09: the last plain tap.
    cur_press: bool,
    /// The press picked the item that was already selected (a still tap
    /// then cycles to the next overlapping one, `sp_release`).
    presel: bool,
    cur_saved: Option<([u8; 4], usize)>,
    last_tap: Option<(u64, Point)>,
    /// Math display settings; what the FFT additions show (from `sp_math`).
    pub math: Math,
    fft_view: Option<su::FftAdd>,
    fft_hz: su::Txt,
}

impl Sp {
    /// A drag or pinch holds the acquisition (no polling).
    pub fn frozen_or_pinch(&self) -> bool {
        self.frozen || self.pinch.is_some()
    }

    /// The page is covered by a popup / box (no page drawing).
    pub fn covered(&self) -> bool {
        self.popup != su::Popup::None || self.sampling
    }

    pub const fn new() -> Sp {
        Sp {
            sel_v: 0,
            sel_h: 0,
            last_h: false,
            guide_until: 0,
            touch: None,
            pinch: None,
            drag_mode: DRAG_STOCK,
            swipes: false,
            press_fx: true,
            pressed: 0,
            pressed_until: 0,
            colour: 2,
            bright: 100,
            grid: 50,
            persist: 0,
            autoset: None,
            frozen: false,
            popup: su::Popup::None,
            uncover: 0,
            pressed_cell: None,
            slider: None,
            sampling: false,
            bubble: false,
            bubble_dirty: false,
            cells_due: false,
            paint_due: false,
            frame_next: 0,
            tap_mode: su::TAP_DEFAULT,
            tabs_until: 0,
            step_until: 0,
            step_side: false,
            flash_tb: 0,
            flash_until: 0,
            entries: 0,
            list: None,
            tb_drawn: (255, false),
            rec_drawn: None,
            cur_press: false,
            presel: false,
            cur_saved: None,
            last_tap: None,
            math: Math::DEFAULT,
            fft_view: None,
            fft_hz: su::Txt::EMPTY,
        }
    }
}

/// Index of the probe factor in scope::PROBES (3 = x1).
fn probe_idx(p: f32) -> usize {
    scope::PROBES.iter().position(|&x| x == p).unwrap_or(3)
}

/// V/div label index (scope_ui::VDIV_LABELS) of a hardware range and probe.
fn vdiv_idx(range: usize, probe: f32) -> u8 {
    (range + probe_idx(probe)).min(21) as u8
}

/// Rows of the frame (13 + 280-row model) a pinch / pan preview maps: target
/// column j of the graph from the cached columns, with a vertical scale `ky`
/// around the zero row and a time scale `kx` around the trigger column.
fn preview_rows(c: &Cache, j: i32, d_pos: i32, hpos: i32, ky: f32, kx: f32, zr: f32) -> (i16, i16) {
    // Source columns covering target column j.
    let (s0, s1) = if kx == 1.0 {
        let s = j - (hpos - c.hpos);
        (s, s + 1)
    } else {
        let a = c.hpos as f32 + (j - hpos) as f32 / kx;
        let b = c.hpos as f32 + (j + 1 - hpos) as f32 / kx;
        let fl = |v: f32| if v >= 0.0 || v == (v as i32) as f32 { v as i32 } else { v as i32 - 1 };
        let (a, b) = (fl(a), (-fl(-b)).max(fl(a) + 1));
        (a, b)
    };
    let (mut lo, mut hi) = (i32::MAX, i32::MIN);
    for s in s0.max(0)..s1.min(400) {
        let (a, b) = c.seg[s as usize];
        if a > b {
            continue;
        }
        lo = lo.min(a as i32);
        hi = hi.max(b as i32);
    }
    if lo > hi {
        return su::EMPTY;
    }
    let map = |r: i32| -> i32 {
        if ky == 1.0 {
            r + d_pos
        } else {
            let z0 = 140 + c.pos;
            (zr + (r - z0) as f32 * ky) as i32
        }
    };
    let (a, b) = (map(lo), map(hi));
    (a.min(b).clamp(-30000, 30000) as i16, a.max(b).clamp(-30000, 30000) as i16)
}

impl App {
    /// The page model from the engine and the page state.
    pub(super) fn sp_model(&self, now: u64) -> Model {
        let s = &self.scope.s;
        let sp = &self.sp;
        let status = if !s.run {
            su::ST_STOP
        } else if self.scope.rolling() {
            su::ST_RUN
        } else {
            self.scope.status.min(3)
        };
        let record = self.scope.record_frac();
        // Tap-step tabs: the side a still press armed (not once it drags or
        // a pinch took over), else the side just stepped.
        let armed = sp.touch.filter(|_| sp.pinch.is_none()).and_then(|t| t.arm.armed(t.still()));
        let pressed = armed.or(if now < sp.step_until { Some(sp.step_side) } else { None });
        let mut tabs = su::tabs_of(sp.tap_mode, s.tb as u8, self.tb_top(), now < sp.tabs_until, pressed);
        let wave = self.scope.wave_on();
        if wave && s.tb <= scope::WAVE_TB_MIN {
            tabs &= !(su::TAB_SHOW_R | su::TAB_PRESS_R); // no faster wave timebase
        }
        let amps = wave && self.scope.wave_scale_now().amps;
        Model {
            status,
            tb: s.tb as u8,
            vdiv: vdiv_idx(s.range, s.probe),
            probe: probe_idx(s.probe) as u8,
            ac: s.ac,
            trig_row: s.trig_row as i16,
            trig_v: self.scope.trig_volts(),
            rising: s.rising,
            trig_px: s.hpos as i16,
            pos: s.pos as i16,
            colour: sp.colour,
            bright: sp.bright,
            grid: sp.grid,
            sel_v: sp.sel_v,
            sel_h: sp.sel_h,
            guide: now < sp.guide_until || sp.touch.is_some_and(|t| t.hit || t.moving),
            xcur: if self.cur.x_on { Some([self.cur.x[0] as i16, self.cur.x[1] as i16]) } else { None },
            ycur: if self.cur.y_on { Some([self.cur.y[0] as i16, self.cur.y[1] as i16]) } else { None },
            battery: self.power.shown_bars(now / 1000 & 1 == 1),
            charging: self.power.on_charger(),
            usb: self.usb_host,
            bt: self.bt_on && self.power.bt_connected(),
            roll: self.scope.rolling(),
            record,
            // Combined view: the strip's GEN badge shows it instead.
            gen_hz: if self.gen.s.on && self.view == View::Scope { self.gen.actual_hz() } else { 0.0 },
            ch1_open: matches!(sp.popup, su::Popup::Ch1(_)),
            math_open: matches!(sp.popup, su::Popup::Math(_)),
            pressed: if now < sp.pressed_until { sp.pressed } else { 0 },
            combined: self.view == View::DmmScope,
            tabs,
            tb_flash: now < sp.flash_until,
            math_on: sp.math.on,
            math_mode: sp.math.mode,
            math_colour: sp.math.colour,
            math_off: sp.math.off().clamp(-400, 400) as i16,
            math_scale: self.math_scale(),
            fft_hz: sp.fft_hz,
            fft: sp.fft_view,
            runt_row: (s.ttype == crate::trigsoft::RUNT).then_some(s.runt_row as i16),
            cur_tab: self.sp_cur_state(),
            // The source badge on the Scope page of builds with the stream
            // ISR (none in the combined view, none without: the stock page).
            src: if !app_wave::WAVE_BUILD || self.view != View::Scope {
                su::SRC_NONE
            } else if wave {
                su::SRC_DMM
            } else {
                su::SRC_MCX
            },
            amps,
            // The V/div text from the value (fmt_div) also off the Wave
            // view: `vdiv`'s label is wrong with a x2 / x5 ... probe (#133).
            wvdiv: self.scope.vdiv(),
            ch1_txt: if wave { self.wave_func_label() } else { "" },
            // Both scope views (the combined view's strip badge no longer
            // shows REC: one look everywhere).
            rec: crate::csvlog::active(),
            rec_dim: su::rec_blink(now),
            rec_unit: su::rec_unit(self.rec_func),
            tsrc: if wave { scope::TSRC_CH1 } else { s.tsrc },
            ch2_txt: self.ch2_label(true),
        }
    }

    /// Math pill second line: FFT dB/div, else the math V/div.
    fn math_scale(&self) -> su::Txt {
        let m = &self.sp.math;
        if m.mode == 5 && !m.vrms {
            su::Txt::new(su::FFT_DB[m.db.min(12) as usize])
        } else {
            let mut t = su::Txt::new(su::VDIV_LABELS[m.vdiv.min(21) as usize]);
            t.push("V");
            t
        }
    }

    /// Highest timebase index the scope takes now (scope.rs `apply`).
    fn tb_top(&self) -> u8 {
        su::tb_top(self.scope.roll, self.scope.s.mode == scope::TrigMode::Single)
    }

    /// A scope page (Scope / DMM+Scope) is entered: no flash for the
    /// timebase it opens with; Halves: the intro tabs on the first entries.
    pub(super) fn sp_enter(&mut self, now: u64) {
        self.sp.flash_tb = self.scope.s.tb as u8;
        self.sp.flash_until = 0;
        if self.sp.tap_mode == su::TAP_HALVES && self.sp.entries < INTRO_ENTRIES {
            self.sp.entries += 1;
            self.sp.tabs_until = now + GUIDE_MS;
        }
    }

    /// Leaving the scope views: a drag / pinch in flight is ended (its
    /// settings applied, the acquisition no longer held), popups close.
    pub(super) fn sp_leave(&mut self, now: u64) {
        let held = self.sp.frozen || self.sp.pinch.is_some();
        self.sp.frozen = false;
        self.sp.touch = None;
        self.sp.pinch = None;
        self.sp.bubble = false;
        su::graph().set_hole(None);
        self.sp.popup = su::Popup::None;
        self.sp.pressed_cell = None;
        self.sp.slider = None;
        self.sp.list = None;
        if held {
            let s = self.scope.s;
            self.scope.apply(s, now);
        }
        // A running AUTO-set ends with the page (review 7389e8a L2: it went
        // on off-page and applied its result / toast on another view).
        if let Some(a) = self.sp.autoset.take() {
            self.sp.sampling = false;
            self.scope_set(a.saved, now);
        }
    }

    /// A stock-replica scope page is shown (the Scope page, or the combined
    /// view: the same page with the meter strip).
    pub(super) fn sp_on(&self) -> bool {
        matches!(self.view, View::Scope | View::DmmScope)
    }

    /// The page is covered (popup, "Sampling" box, the combined view's
    /// function band): no page drawing.
    pub(super) fn sp_covered(&self) -> bool {
        self.sp.covered() || (self.view == View::DmmScope && self.band_until != 0)
    }

    /// Combined view: the meter strip from the shown reading.
    pub(super) fn sp_strip(&mut self) {
        if self.sp.covered() {
            return;
        }
        let r = match self.shown_reading() {
            Some(r) => r,
            // The power meter before its first reading (#108).
            None if self.dmm.pw.on => self.pw_strip_wait(),
            None => {
                su::meter_strip(&su::StripData::EMPTY);
                return;
            }
        };
        let (hold, lim) = (self.meter.hold_state(), self.meter.verdict());
        let mut d = ui::strip_data(&r, hold, lim, self.meter.rel_active(), self.hv_level, self.strip_alt, self.dmm_view.label);
        ui::strip_acc(&mut d, &r, self.acc_ctx());
        if self.dmm_view.short != 0 && r.alert == crate::sentinel::Alert::None {
            // The beeper indicator (`beep_ind`): the badge and the value green.
            d.badge = if self.dmm_view.short == 1 { "SHORT" } else { "BEEP" };
            d.badge_col = ui::SHORT_BG;
            d.short = true;
        }
        if self.gen.s.on {
            let mut b = su::Buf::new();
            su::fmt_hz(&mut b, self.gen.actual_hz());
            d.gen = su::Txt::new(b.as_str());
        }
        su::meter_strip(&d);
    }

    /// Full page draw (after `redraw`).
    pub(super) fn sp_draw_static(&mut self) {
        self.sp.uncover = 0;
        su::invalidate();
        self.sp_draw_page();
    }

    /// The page from the cached frame and the model (all of it after
    /// `su::invalidate`, the graph after `su::uncover`).
    fn sp_draw_page(&mut self) {
        self.sp_prepare();
        self.sp_draw_widgets();
    }

    /// The page's graph rows from the cached frame (no drawing).
    pub(super) fn sp_prepare(&mut self) {
        // An open popup's box is drawn last: the graph leaves it alone.
        su::graph().set_hole_at(su::H_POPUP, su::popup_box(self.sp.popup));
        self.sp_render();
        self.sp_math();
    }

    /// The page from the graph rows and the model: the same pixels on
    /// every call after `su::invalidate` (App::repaint_strips).
    pub(super) fn sp_draw_widgets(&mut self) {
        self.sp.bubble_dirty = self.sp.bubble;
        self.sp_update(self.now);
        self.sp_cells();
        if self.view == View::DmmScope {
            su::strip_invalidate();
            self.sp_strip();
            if self.band_until != 0 {
                self.pw_btn_glyph();
                ui::func_band(self.dmm.button(), self.dmm.shown_mode(), self.dmm.sub() == 2, self.dmm.ohm_var() == 1);
            }
        }
        if self.sp.popup != su::Popup::None {
            self.sp_popup_draw();
        }
        if self.sp.sampling {
            su::sampling();
        }
    }

    /// Repaint whatever changed (model diff, graph rows).
    pub(super) fn sp_update(&mut self, now: u64) {
        let m = self.sp_model(now);
        su::update(&m);
        if self.sp.bubble && self.sp.bubble_dirty {
            self.sp.bubble_dirty = false;
            self.sp_bubble_draw();
        }
    }

    /// Measurement bar from the selection and the latest results.
    pub(super) fn sp_cells(&mut self) {
        self.sp.cells_due = false;
        self.sp_cells_with(false);
    }

    /// One changed measurement cell per call (scope_poll, a pass without a
    /// new frame): `cells_due` stays set until all four are current.
    pub(super) fn sp_cells_step(&mut self) {
        self.sp.cells_due = self.sp_cells_with(true);
    }

    /// Cell texts from the selection and results; `one`: draw at most one
    /// changed cell and return whether one was drawn.
    fn sp_cells_with(&mut self, one: bool) -> bool {
        let m = self.sp_model(self.now);
        let mut b: [[u8; 24]; 4] = [[0; 24]; 4];
        let mut n = [0usize; 4];
        for i in 0..self.meas_n.min(4) {
            let id = self.meas_sel[i];
            // Stock label "Vp-p:" (ours: "Vpp:").
            let mut t = [0u8; 24];
            let f = crate::scope_meas::format_unit(&mut t, id, self.meas_res.v[id as usize], m.amps);
            let (pre, rest): (&[u8], &[u8]) = match (f.strip_prefix("Vpp"), f.strip_prefix("Ipp")) {
                (Some(r), _) => (b"Vp-p", r.as_bytes()),
                (_, Some(r)) => (b"Ip-p", r.as_bytes()),
                _ => (b"", f.as_bytes()),
            };
            let k = (pre.len() + rest.len()).min(24);
            b[i][..pre.len()].copy_from_slice(pre);
            b[i][pre.len()..k].copy_from_slice(&rest[..k - pre.len()]);
            n[i] = k;
        }
        let txt = |i: usize| core::str::from_utf8(&b[i][..n[i]]).unwrap_or("--");
        let t: [&str; 4] = [txt(0), txt(1), txt(2), txt(3)];
        if one {
            return su::cells_step(&m, &t[..self.meas_n.min(4)]);
        }
        su::cells(&m, &t[..self.meas_n.min(4)]);
        false
    }

    /// A new frame from the engine: column rows (cached for previews),
    /// persistence, the graph.
    pub(super) fn sp_frame(&mut self, f: &scope::Frame, now: u64) {
        let c = cache();
        let s = self.scope.s;
        // Persistence: the previous frame joins the current layer (stock
        // 4.3); layers swap every period; off in roll and STOP.
        let pm = if self.scope.rolling() || !s.run { 0 } else { self.sp.persist as usize };
        if pm == 0 {
            c.ps_a = [su::EMPTY; 399];
            c.ps_b = [su::EMPTY; 399];
        } else {
            if c.valid {
                for i in 0..399 {
                    let (a, b) = c.seg[i];
                    if a <= b {
                        let p = &mut c.ps_a[i];
                        *p = (p.0.min(a), p.1.max(b));
                    }
                }
            }
            if PERSIST_MS[pm] != u64::MAX && now >= c.ps_t {
                c.ps_t = now + PERSIST_MS[pm];
                c.ps_b = c.ps_a;
                c.ps_a = [su::EMPTY; 399];
            }
        }
        // Code -> row is affine: rows of the code min / max per column.
        let r0 = self.scope.row_of_code(0.0);
        let slope = (self.scope.row_of_code(1000.0) - r0) / 1000.0;
        let row = |code: u16| -> i32 { (r0 + slope * code as f32) as i32 };
        let n = f.data.len();
        let mut pseg: Option<(i32, i32)> = None;
        for i in 0..scope::COLS {
            let (lo, hi);
            if let Some(cols) = f.cols {
                let (a, b) = cols[i];
                if a == u16::MAX {
                    pseg = None;
                    c.seg[i] = su::EMPTY;
                    continue;
                }
                let (r1, r2) = (row(a), row(b));
                lo = r1.min(r2);
                hi = r1.max(r2);
            } else if n >= scope::COLS {
                let a = i * n / scope::COLS;
                let b = ((i + 1) * n / scope::COLS).max(a + 1).min(n);
                let (mut cl, mut ch) = (u16::MAX, 0u16);
                for &v in &f.data[a..b] {
                    cl = cl.min(v);
                    ch = ch.max(v);
                }
                let (r1, r2) = (row(cl), row(ch));
                lo = r1.min(r2);
                hi = r1.max(r2);
            } else if n >= 2 {
                // Fewer samples than columns (fast timebases, <= 1 us/div
                // with the 50 MS/s record): band-limited sin(x)/x
                // interpolation (UF-41, src/sinc.rs Lanczos-3) instead of
                // straight lines between the samples.
                let p = i as f32 * (n - 1) as f32 / (scope::COLS - 1) as f32;
                let r = (r0 + slope * crate::sinc::at(f.data, p)) as i32;
                lo = r;
                hi = r;
            } else {
                c.seg[i] = su::EMPTY;
                continue;
            }
            // Stock widening: consecutive bars touch.
            let (mut a, mut b) = (lo, hi);
            if let Some((pa, pb)) = pseg {
                a = a.min(pb);
                b = b.max(pa);
            }
            pseg = Some((lo, hi));
            c.seg[i] = (a.clamp(-30000, 30000) as i16, b.clamp(-30000, 30000) as i16);
        }
        c.valid = true;
        c.pos = s.pos;
        c.hpos = s.hpos;
        c.range = s.range;
        c.tb = s.tb;
        self.sp_gen_frame(f);
        self.ch2_rows(f, &mut c.c2seg);
        self.sp_math_frame(f);
        self.sp_render();
    }

    /// Graph rows from the cache with the current settings (identity for a
    /// fresh frame; the drag / pinch preview otherwise).
    pub(super) fn sp_render(&mut self) {
        let c = cache();
        let g = su::graph();
        let h = if self.view == View::DmmScope { 301 - su::COMBINED_TOP } else { 281 };
        if !c.valid {
            g.clear();
            return;
        }
        let s = &self.scope.s;
        let (ky, kx) = match self.sp.pinch {
            // PINCH_LIVE: the shown frame stretched with the fingers.
            Some(p) if pinch_mode() == su::PINCH_LIVE => {
                let (ky, kx) = (scope::VDIV[c.range] / scope::VDIV[s.range], scope::SDIV[c.tb] / scope::SDIV[s.tb]);
                if p.axis == Axis::V { (ky * p.r, kx) } else { (ky, kx * p.r) }
            }
            // (Indices clamped again: a safety net, pinch_step keeps them in
            // their tables.)
            Some(p) if p.axis == Axis::V && self.scope.wave_on() => (self.scope.vdiv() / self.wave_step_vdiv(p.i), 1.0),
            Some(p) if p.axis == Axis::V => (scope::VDIV[c.range.min(9)] / scope::VDIV[p.i.min(9)], 1.0),
            Some(p) => (1.0, scope::SDIV[c.tb.min(29)] / scope::SDIV[p.i.min(29)]),
            None => (scope::VDIV[c.range] / scope::VDIV[s.range], scope::SDIV[c.tb] / scope::SDIV[s.tb]),
        };
        // A stopped pan moves the record under the trigger column.
        let pan = self.sp.touch.map_or(0, |t| t.pan);
        let hpos = if s.run { s.hpos } else { c.hpos + pan };
        let zr = (140 + s.pos) as f32;
        let d_pos = s.pos - c.pos;
        for j in 0..399 {
            let (a, b) = preview_rows(c, j as i32, d_pos, hpos, ky, kx, zr);
            g.tr[j] = if a > b { su::EMPTY } else { (su::trace_fy(h, a as i32), su::trace_fy(h, b as i32)) };
            // The GEN / CH2 traces only as captured (no preview mapping).
            let still = d_pos == 0 && hpos == c.hpos && ky == 1.0 && kx == 1.0;
            let fy = |(a, b): (i16, i16)| if a > b || !still { su::EMPTY } else { (su::trace_fy(h, a as i32), su::trace_fy(h, b as i32)) };
            g.gn[j] = fy(c.gseg[j]);
            g.c2[j] = fy(c.c2seg[j]);
            let (pa, pb) = (c.ps_a[j], c.ps_b[j]);
            let (lo, hi) = (pa.0.min(pb.0), pa.1.max(pb.1));
            g.ps[j] = if lo > hi || d_pos != 0 || hpos != c.hpos || ky != 1.0 || kx != 1.0 {
                su::EMPTY
            } else {
                (su::trace_fy(h, lo as i32), su::trace_fy(h, hi as i32))
            };
        }
    }

    // ------------------------------------------------------------ CSV

    /// MF Settings "Save CSV" (UF-13): the stored record of the stopped
    /// scope as SCOP%03u.CSV (scope_csv.rs, written by the screenshot job).
    /// Stock rule: refused while sampling ("Please stop sample").
    #[inline(never)] // .cold (link-native.x): a tap (MF Settings Save CSV)
    pub(super) fn sp_save_csv(&mut self, now: u64) {
        let rec = self.scope.record();
        if self.scope.s.run || rec.is_empty() {
            self.toast(now, if self.scope.s.run { "Please stop sample" } else { "No record to save" }, false);
            return;
        }
        if crate::screenshot::busy() || crate::csvlog::writing() {
            self.toast(now, "Disk busy, try again", false);
            return;
        }
        let s = self.scope.s;
        let (fs, t0, trig) = self.scope.record_info();
        let a = self.scope.volts(0.0);
        let b = self.scope.volts(1.0) - a;
        let mut h = crate::scope_csv::Hdr::new();
        let mut t = [0u8; 16];
        h.s("# Alientek ").s(crate::ident::model_name()).s(", opendm40 ").s(crate::ident::VERSION).s(", scope record (no real-time clock)\r\n");
        let wave = self.scope.wave_on();
        let amps = wave && self.scope.wave_scale_now().amps;
        let unit = if amps { "A" } else { "V" };
        let mut v = su::Buf::new();
        // From the value: the label table index (vdiv_idx) is wrong with
        // a x2 / x5 ... probe (#133).
        su::fmt_div(&mut v, self.scope.vdiv(), unit);
        let mut sr = su::Buf::new();
        su::fmt_hz(&mut sr, fs as f32);
        let sr = sr.as_str();
        h.s("# timebase ").s(su::TB_LABELS[self.scope.record_tb().min(29)]).s("/div, ").s(v.as_str()).s("/div, ");
        if wave {
            // Source DMM: the HY3131 AD2 stream of the DMM row (app_wave.rs).
            h.s("source DMM ").s(self.wave_func_label()).s(" (AD2, isolated jacks)");
        } else {
            h.s("probe ").s(scope::PROBE_NAMES[probe_idx(s.probe)]);
        }
        h.s(", coupling ").s(if s.ac { "AC" } else { "DC" }).s(", sample rate ").s(sr.strip_suffix("Hz").unwrap_or(sr)).s("S/s, samples ");
        h.s(crate::dmm::format(&mut t, rec.len() as f32, 0)).s("\r\n");
        let mut tv = su::Buf::new();
        su::fmt_v(&mut tv, self.scope.trig_volts(), unit);
        h.s("# trigger ");
        self.sp_trig_desc(&mut h);
        h.s(" ").s(if s.rising { "rising" } else { "falling" }).s(" ").s(tv.as_str());
        h.s(", mode ").s(match s.mode {
            scope::TrigMode::Auto => "Auto",
            scope::TrigMode::Normal => "Normal",
            scope::TrigMode::Single => "Single",
        });
        h.s(if trig { ", triggered\r\n" } else { ", untriggered\r\n" });
        h.s(if wave {
            if amps {
                "# time_s = seconds from the trigger point (untriggered: from the screen centre); the value column is amperes through the DMM A jack (DMM calibration applied)\r\n"
            } else {
                "# time_s = seconds from the trigger point (untriggered: from the screen centre); the value column is volts at the DMM jacks (DMM calibration applied)\r\n"
            }
        } else {
            "# time_s = seconds from the trigger point (untriggered: from the screen centre); volts at the probe tip\r\n"
        });
        crate::scope_csv::set(rec, a, b, fs as f32, t0, &h, amps);
        if !crate::screenshot::start_csv() {
            self.toast(now, "Save CSV Error", false);
            return;
        }
        self.scope.hold = true;
        self.toast(now, "Saving CSV...", true);
    }

    /// Trigger type text of the CSV header.
    fn sp_trig_desc(&self, h: &mut crate::scope_csv::Hdr) {
        let s = &self.scope.s;
        match s.ttype {
            crate::trigsoft::PULSE => {
                let mut b = su::Buf::new();
                su::fmt_s125(&mut b, s.pwidth);
                h.s("Pulse width ").s(crate::trigsoft::COND_NAMES[s.pcond.min(2) as usize]).s(" ").s(b.as_str()).s(" (software),");
            }
            crate::trigsoft::RUNT => {
                let mut b = su::Buf::new();
                let unit = if self.scope.wave_on() && self.scope.wave_scale_now().amps { "A" } else { "V" };
                su::fmt_v(&mut b, self.scope.runt_volts(), unit);
                h.s("Runt, 2nd level ").s(b.as_str()).s(" (software),");
            }
            _ => {
                h.s("Edge");
            }
        }
    }

    /// Per pass: a finished CSV releases the scope and shows its result.
    pub(super) fn sp_csv_poll(&mut self, now: u64) {
        let Some(r) = crate::screenshot::take_csv_result() else { return };
        self.scope.hold = false;
        let mut t = *b"Saved SCOP000.DLG";
        let (txt, ok) = match r {
            crate::screenshot::Shot::Saved(n) => {
                t[10] = b'0' + (n / 100 % 10) as u8;
                t[11] = b'0' + (n / 10 % 10) as u8;
                t[12] = b'0' + (n % 10) as u8;
                (core::str::from_utf8(&t).unwrap_or("Save CSV OK"), true)
            }
            crate::screenshot::Shot::NoDisk => ("No disk", false),
            crate::screenshot::Shot::Full => ("Disk full", false),
            crate::screenshot::Shot::Error | crate::screenshot::Shot::Busy => ("Save CSV Error", false),
        };
        self.toast(now, txt, ok);
        self.beep(now, if ok { 100 } else { 500 });
    }

    // ------------------------------------------------------------ math / FFT

    /// A new frame with math on: FFT mode starts a spectrum job from the
    /// record around the frame (now, while its samples are still in the
    /// record; `step` finishes it over the next passes), ABS maps the shown
    /// columns.
    /// The GEN trace of frame `f` (`Cache::gseg`): the generator's volts
    /// over each column's samples, from the stored record's generator
    /// phase (scope `gen_sync`), on CH1's scale. Nothing for a roll /
    /// zoomed-out frame (columns, no samples) or a record without the
    /// phase.
    fn sp_gen_frame(&mut self, f: &scope::Frame) {
        let c = cache();
        c.gseg = [su::EMPTY; 400];
        if !self.scope.gen_trace || f.data.len() < 2 {
            return;
        }
        let Some(g) = self.scope.gen_sync() else { return };
        let rec = self.scope.record();
        let off = (f.data.as_ptr() as usize).wrapping_sub(rec.as_ptr() as usize) / 2;
        if off + f.data.len() > rec.len() {
            return;
        }
        let n = f.data.len();
        let row = |v: f32| self.scope.row_of_volts(v).clamp(-30000.0, 30000.0) as i16;
        for i in 0..scope::COLS {
            let (a, b) = if n >= scope::COLS {
                ((i * n / scope::COLS) as f64, ((i + 1) * n / scope::COLS).max(i * n / scope::COLS + 1) as f64)
            } else {
                // Fewer samples than columns: the column's own time span.
                let k = (n - 1) as f64 / (scope::COLS - 1) as f64;
                (i as f64 * k, (i as f64 + 1.0) * k)
            };
            let (lo, hi) = g.span(off as f64 + a, off as f64 + b);
            let (r1, r2) = (row(lo), row(hi));
            c.gseg[i] = (r1.min(r2), r1.max(r2));
        }
    }

    fn sp_math_frame(&mut self, f: &scope::Frame) {
        let m = self.sp.math;
        if !m.on {
            return;
        }
        if m.mode != 5 {
            self.sp_math();
            return;
        }
        if self.scope.rolling() {
            // No linear record in roll: no spectrum.
            if fft().bins > 0 {
                fft().clear();
                self.sp_math();
            }
            return;
        }
        if fft().busy() {
            return;
        }
        let rec = self.scope.record();
        if rec.len() < crate::fft::N_MIN {
            return;
        }
        // The frame's window in the record (zoomed out past it: all of it).
        let (off, n) = if f.data.is_empty() {
            (0, rec.len())
        } else {
            let o = (f.data.as_ptr() as usize).wrapping_sub(rec.as_ptr() as usize) / 2;
            if o + f.data.len() > rec.len() {
                return;
            }
            (o, f.data.len())
        };
        // N points: the window decimated by k (mean of k samples) when it
        // holds N or more, else N record samples centred on it (fast
        // timebases: a finer resolution than the screen span), else the
        // largest power of two the record holds.
        let nn = crate::fft::N;
        let (len, k, start) = if n >= nn {
            let k = n / nn;
            (nn, k, off + (n - nn * k) / 2)
        } else {
            let len = if rec.len() >= nn { nn } else { 1 << (usize::BITS - 1 - rec.len().leading_zeros()) };
            let c = off + n / 2;
            (len, 1, c.saturating_sub(len / 2).min(rec.len() - len))
        };
        let a = self.scope.volts(0.0);
        let b = self.scope.volts(1.0) - a;
        let Some(w) = fft_work(true) else {
            return;
        };
        crate::prof::enter(crate::prof::FFT);
        fft().load(w, &rec[start..], k, len, a, b, f.fs as f32, m.win);
        crate::prof::exit(crate::prof::FFT);
    }

    /// One slice of the spectrum job (every pass on the page): a finished
    /// spectrum is drawn.
    fn sp_math_step(&mut self) {
        if unsafe { FFT_RETRY } && crate::diskbuf::fft_free() {
            unsafe { FFT_RETRY = false };
            self.scope_invalidate();
        }
        if !(self.sp.math.on && self.sp.math.mode == 5 && fft().busy()) {
            return;
        }
        let Some(w) = fft_work(false) else {
            return;
        };
        crate::prof::enter(crate::prof::FFT);
        let done = fft().step(w);
        crate::prof::exit(crate::prof::FFT);
        // (sp_math is its own FFT section, not nested.)
        if done {
            self.sp_math();
        }
    }

    /// FFT Hz/div choices of the current spectrum (zeros without one).
    fn fft_hz_opts(&self) -> [f32; 4] {
        let f = fft();
        if f.bins < 2 {
            return [0.0; 4];
        }
        hz_opts(f.bin_hz * (f.bins - 1) as f32)
    }

    /// The math trace (`Graph::ma`) and the FFT texts from the current
    /// spectrum / shown columns and the math settings (cheap: re-run on
    /// every math change and drag, "the last spectrum is kept while
    /// dragging").
    pub(super) fn sp_math(&mut self) {
        crate::prof::enter(crate::prof::FFT);
        let g = su::graph();
        let h = if self.view == View::DmmScope { 301 - su::COMBINED_TOP } else { 281 };
        let m = self.sp.math;
        self.sp.fft_view = None;
        self.sp.fft_hz = su::Txt::EMPTY;
        g.ma = [su::EMPTY; 399];
        if !m.on {
            crate::prof::exit(crate::prof::FFT);
            return;
        }
        let zr = 140 + m.off();
        let widen = |g: &mut su::Graph, j: usize, r: Option<i32>, prev: &mut Option<i32>| {
            g.ma[j] = match r {
                Some(r) => {
                    let (a, b) = prev.map_or((r, r), |p| (r.min(p), r.max(p)));
                    (su::trace_fy(h, a), su::trace_fy(h, b))
                }
                None => su::EMPTY,
            };
            *prev = r;
        };
        if m.mode != 5 {
            // ABS: |v| of the shown CH1 columns (cached rows -> volts with
            // the settings they were drawn with).
            let c = cache();
            let mv = su::VDIV_VAL[m.vdiv.min(21) as usize];
            let vr = if self.scope.wave_on() { 4.0 * self.scope.vdiv() / 141.0 } else { 4.0 * scope::VDIV[c.range.min(9)] * self.scope.s.probe / 141.0 };
            // (The CH1 columns are already widened: the bars touch.)
            for j in 0..399 {
                let (lo, hi) = c.seg[j];
                if !c.valid || lo > hi {
                    g.ma[j] = su::EMPTY;
                    continue;
                }
                let (va, vb) = ((140 + c.pos - hi as i32) as f32 * vr, (140 + c.pos - lo as i32) as f32 * vr);
                let top = va.abs().max(vb.abs());
                let bot = if va <= 0.0 && vb >= 0.0 { 0.0 } else { va.abs().min(vb.abs()) };
                let ra = (zr as f32 - top / mv * 35.0).clamp(-30000.0, 30000.0) as i32;
                let rb = (zr as f32 - bot / mv * 35.0).clamp(-30000.0, 30000.0) as i32;
                g.ma[j] = (su::trace_fy(h, ra), su::trace_fy(h, rb));
            }
            crate::prof::exit(crate::prof::FFT);
            return;
        }
        let f = fft();
        if f.bins < 2 {
            crate::prof::exit(crate::prof::FFT);
            return;
        }
        let opts = self.fft_hz_opts();
        let (hzd, full) = (opts[m.hz.min(3) as usize], opts[3]);
        if !(hzd > 0.0) {
            crate::prof::exit(crate::prof::FFT);
            return;
        }
        let half = 5.0 * hzd;
        let ctr = m.ctr.clamp(half, half.max(10.0 * full - half));
        self.sp.math.ctr = ctr;
        let f0 = ctr - half;
        let px = hzd / 40.0;
        let ydiv_db = su::FFT_DB_VAL[m.db.min(12) as usize];
        let ydiv_v = su::VDIV_VAL[m.vdiv.min(21) as usize];
        let row = |db: f32| -> i32 {
            let r = if m.vrms { zr as f32 - crate::fft::db_to_v(db) / ydiv_v * 35.0 } else { zr as f32 - db / ydiv_db * 35.0 };
            r.clamp(-30000.0, 30000.0) as i32
        };
        let mut prev = None;
        for j in 0..399 {
            let fa = f0 + j as f32 * px;
            let r = f.col_max(fa, fa + px).map(row);
            widen(g, j, r, &mut prev);
        }
        // Texts: centre (stock label), scales, peak.
        let mut b = su::Buf::new();
        su::fmt_hz(&mut b, ctr);
        self.sp.fft_hz = su::Txt::new(b.as_str());
        let mut y = su::Buf::new();
        if m.vrms {
            y.push(su::VDIV_LABELS[m.vdiv.min(21) as usize]).push("V/div");
        } else {
            y.push(su::FFT_DB[m.db.min(12) as usize]).push("/div");
        }
        let mut x = su::Buf::new();
        su::fmt_hz125(&mut x, hzd);
        x.push("/div");
        let mut pk = su::Buf::new();
        let mut at = None;
        match f.peak(f0, f0 + 10.0 * hzd) {
            Some((pf, pd)) => {
                su::fmt_hz(&mut pk, pf);
                pk.push(" ");
                if m.vrms {
                    su::fmt_v(&mut pk, crate::fft::db_to_v(pd), "V");
                } else {
                    let mut t = [0u8; 16];
                    pk.push(crate::dmm::format(&mut t, pd, if pd.abs() < 99.95 { 1 } else { 0 })).push("dBV");
                }
                at = Some((((pf - f0) / px) as i16, row(pd).clamp(-1000, 1000) as i16));
            }
            None => {
                pk.push("--");
            }
        }
        self.sp.fft_view = Some(su::FftAdd { ydiv: su::Txt::new(y.as_str()), xdiv: su::Txt::new(x.as_str()), peak: su::Txt::new(pk.as_str()), peak_at: at });
        crate::prof::exit(crate::prof::FFT);
    }

    /// A math setting changed: a new spectrum where the computation depends
    /// on it (`fresh`: on / mode / window; a stopped scope re-emits its
    /// stored record), the trace and texts again.
    fn sp_math_changed(&mut self, fresh: bool) {
        if fresh {
            fft().clear();
            self.scope_invalidate();
        }
        self.sp_math();
    }

    /// Settings changed outside a drag: forget the persistence.
    pub(super) fn sp_settings_changed(&mut self) {
        let c = cache();
        c.ps_a = [su::EMPTY; 399];
        c.ps_b = [su::EMPTY; 399];
    }

    /// Per main-loop pass on the scope page (from scope_poll): timers, the
    /// diff repaint.
    pub(super) fn sp_tick(&mut self, now: u64) {
        self.sp_csv_poll(now);
        if self.sp.pressed != 0 && now >= self.sp.pressed_until {
            self.sp.pressed = 0;
        }
        self.sp_math_step();
        if let Some(a) = self.sp.autoset {
            if now >= a.abort {
                self.sp.autoset = None;
                self.scope_set(a.saved, now);
            }
        }
        if self.sp.sampling && self.sp.autoset.is_none() {
            self.sp.sampling = false;
            self.redraw = true;
            return;
        }
        // Every timebase change (tap, list, pinch, AUTO, SCPI) flashes the
        // badge.
        let tb = self.scope.s.tb as u8;
        if tb != self.sp.flash_tb {
            self.sp.flash_tb = tb;
            self.sp.flash_until = now + FLASH_MS;
        }
        // Corners: a still hold in a zone repeats the step.
        if self.sp.popup == su::Popup::None && self.sp.pinch.is_none() {
            if let Some(mut t) = self.sp.touch {
                if let Some(slower) = t.arm.repeat(self.sp.tap_mode, t.still(), now) {
                    self.sp.touch = Some(t);
                    // The repeated step is kept: a later drag + second
                    // finger (Cancel) undoes only what the drag moved
                    // (pre-flash review 621ac3f L9). Still = nothing
                    // dragged yet, so the whole snapshot is rebased.
                    if self.sp_tb_step(slower, now) {
                        if let Some(t) = self.sp.touch.as_mut() {
                            t.snap = self.scope.s;
                        }
                    }
                }
            }
        }
        if self.sp_covered() {
            // Popups leave the top bar visible: the timebase badge follows
            // the list's picks and ends its flash (also after a host's
            // change, #82).
            if self.sp.popup != su::Popup::None && !self.redraw {
                let m = self.sp_model(now);
                if self.sp.tb_drawn != (m.tb, m.tb_flash) {
                    self.sp.tb_drawn = (m.tb, m.tb_flash);
                    su::tb_badge(&m);
                }
            }
            // The REC mark keeps blinking above a popup.
            if crate::csvlog::active() && !self.redraw {
                let dim = su::rec_blink(now);
                if self.sp.rec_drawn != Some(dim) {
                    self.sp.rec_drawn = Some(dim);
                    su::rec_mark_blink(su::SRC_BADGE, su::C_BAR, !dim, su::rec_unit(self.rec_func));
                }
            }
            return;
        }
        self.sp.rec_drawn = None;
        self.sp_update(now);
    }

    // ------------------------------------------------------------ zones

    /// Stock selection on press (zone search, then the cursor lines, then
    /// the tap-step zone: scope_ui::graph_press). Returns (a marker zone was
    /// hit, the tap-step side armed).
    fn sp_select(&mut self, p: Point) -> (bool, Option<bool>) {
        self.sp.cur_press = false;
        self.sp.presel = false;
        let m = self.sp_model(self.now);
        match su::graph_press(&m, self.sp.tap_mode, self.sp.sel_v, self.sp.sel_h, self.sp.last_h, p.x as i16, p.y as i16) {
            su::Press::Marker(level, id) => {
                // Already the selection: a still tap cycles on release.
                self.sp.presel = level == !self.sp.last_h && id == if level { self.sp.sel_v } else { self.sp.sel_h };
                if level {
                    self.sp.sel_v = id;
                    self.sp.last_h = false;
                } else {
                    self.sp.sel_h = id;
                    self.sp.last_h = true;
                }
                (true, None)
            }
            su::Press::Step(slower) => (false, Some(slower)),
            su::Press::Cur => {
                self.sp.cur_press = true;
                (false, None)
            }
            su::Press::None => (false, None),
        }
    }

    // ------------------------------------------------------------ gestures

    /// Gesture on the scope page; None = not handled here (the generic
    /// handling in `gesture` runs), Some(captured) otherwise.
    pub(super) fn sp_gesture(&mut self, clk: &mut Clock, g: Gesture) -> Option<bool> {
        let now = clk.now_ms();
        // The end of a pinch always reaches it (before the AUTO / popup
        // early returns).
        if let Gesture::PinchEnd(ok) = g {
            self.sp_pinch_end(ok, now);
            return Some(false);
        }
        // Touch lock while AUTO-set runs (stock), keys too except MENU.
        if self.sp.autoset.is_some() {
            return match g {
                Gesture::KeyDown(0) => None,
                Gesture::KeyDown(1) => {
                    // RUN refused: the stock "Sampling" box until it is done.
                    self.beep(now, 50);
                    if !self.sp.sampling && !self.redraw {
                        self.sp.sampling = true;
                        su::sampling();
                    }
                    Some(false)
                }
                Gesture::KeyDown(_) | Gesture::Down(_) | Gesture::Tap(_) => Some(false),
                _ => None,
            };
        }
        if self.sp.popup != su::Popup::None {
            return self.sp_popup_gesture(clk, g);
        }
        if self.view == View::DmmScope {
            // Combined view: DMM keys; the strip and the function band keep
            // their own handling (app.rs); the strip is not a drag area (a
            // swipe from it changes the view).
            match g {
                Gesture::KeyDown(_) | Gesture::KeyUp(_) => return None,
                Gesture::Down(_) | Gesture::Tap(_) if self.band_until != 0 => return None,
                Gesture::Down(p) if su::strip_hit(p.x, p.y).is_some() => return Some(false),
                Gesture::Tap(p) => {
                    if let Some(badges) = su::strip_hit(p.x, p.y) {
                        self.beep(now, 30);
                        if badges {
                            self.rel_toggle(now);
                        } else {
                            self.band_open(now);
                        }
                        return Some(false);
                    }
                }
                Gesture::LongPress(p) => {
                    if su::strip_hit(p.x, p.y).is_some() {
                        self.beep(now, 60);
                        self.show(View::Dmm, clk);
                        return Some(true);
                    }
                }
                Gesture::SwipeLeft | Gesture::SwipeRight => return None,
                _ => {}
            }
        }
        match g {
            // A top-bar badge (before the graph area: INNER starts at y 15):
            // not captured, its release is the Tap below.
            Gesture::Down(p) if su::top_badge(p.x, p.y).is_some() => Some(false),
            Gesture::Down(p) if app_wave::WAVE_BUILD && self.view == View::Scope && su::src_hit(p.x, p.y) => Some(false),
            Gesture::Down(p) if su::INNER.has(p.x as i16, p.y as i16) => {
                let (hit, side) = self.sp_select(p);
                let s = self.scope.s;
                self.sp.touch = Some(Touch {
                    p0: p,
                    last: p,
                    applied: p,
                    far: 0,
                    moving: false,
                    hit,
                    t0: now,
                    snap: s,
                    cur: self.cur,
                    anchor: self.scope.anchor(),
                    pan: 0,
                    axis: None,
                    grabbed: gesture::grab_ms() == 0,
                    acc: 0,
                    code: 0,
                    travel: 0,
                    arm: su::Arm { side, t0: now, rep: 0 },
                });
                self.sp.guide_until = now + GUIDE_MS;
                gesture::raw_next(true);
                Some(true)
            }
            Gesture::Down(p) => {
                // Right panel: pressed feedback (stock: none; ours, switchable).
                if let Some(k) = su::panel_hit(p.x, p.y) {
                    if self.sp.press_fx {
                        self.sp.pressed = k as u8 + 1;
                        self.sp.pressed_until = now + PRESS_MS;
                    }
                }
                Some(false)
            }
            Gesture::Grab(_) => {
                if let Some(t) = self.sp.touch.as_mut() {
                    t.grabbed = true;
                    t.applied = t.last;
                    self.beep(now, 20);
                }
                Some(true)
            }
            Gesture::Move(_, p) => {
                self.sp_move(p, now);
                Some(true)
            }
            // A raw-move touch never sends Drag; a plain capture would.
            Gesture::Drag(_, p, _) if self.sp.touch.is_some() => {
                self.sp_move(p, now);
                Some(true)
            }
            Gesture::Up(_) => {
                self.sp_release(now);
                Some(false)
            }
            Gesture::Cancel => {
                // A second finger: undo what the first finger moved (a pinch
                // must not start with its accidental nudge), drop the touch.
                if let Some(t) = self.sp.touch.take() {
                    self.cur = t.cur;
                    self.cursor_values();
                    if self.sp.frozen {
                        self.sp.frozen = false;
                        self.scope.preview(t.snap);
                        self.sp_render();
                        self.scope.apply(t.snap, now);
                    }
                }
                Some(false)
            }
            Gesture::LongPress(_) => {
                // opendm40: a still long press on a marker recentres its item:
                // trigger position -> screen centre, trigger level -> the
                // middle of the signal (Vmax + Vmin) / 2, CH1 -> 0 V at the
                // centre row.
                if let Some(t) = self.sp.touch {
                    if t.hit && !t.moving {
                        self.sp.presel = false; // no tap cycling on its release
                        self.sp_recentre(now);
                    }
                }
                Some(true)
            }
            Gesture::Pinch(axis, s0, s) => {
                self.sp_pinch(axis, s0, s);
                Some(false)
            }
            Gesture::PinchEnd(ok) => {
                self.sp_pinch_end(ok, now);
                Some(false)
            }
            Gesture::Tap(p) => {
                if let Some(k) = su::panel_hit(p.x, p.y) {
                    self.beep(now, 30);
                    self.sp_panel(clk, k, now);
                    return Some(false);
                }
                // opendm40 additions (the stock top bar is not tappable): the
                // status badge = RUN/STOP, the timebase badge = timebase
                // list, the V/div badge = CH1 popup (as B2), the trigger
                // badge = Trigger tab; with the stream ISR the source badge
                // (MCX | DMM: the DMM wave view, app_wave.rs).
                if app_wave::WAVE_BUILD && self.view == View::Scope && su::src_hit(p.x, p.y) {
                    self.beep(now, 30);
                    if self.scope.wave_on() {
                        self.wave_badge_close(clk);
                    } else if crate::hy3131::streaming() {
                        // A logger session (or the inrush capture) owns
                        // the HY3131: no Wave view (wave_open, region C,
                        // would also step the DMM range under the stream).
                        self.toast(now, "Wave: data logger running", false);
                    } else {
                        self.wave_open(clk);
                    }
                    return Some(false);
                }
                match su::top_badge(p.x, p.y) {
                    Some(0) => {
                        self.beep(now, 30);
                        self.sp_key(clk, 1);
                    }
                    Some(1) => {
                        self.beep(now, 30);
                        self.sp_popup_open(su::Popup::Tb(self.scope.s.tb as u8));
                    }
                    Some(2) if self.scope.wave_on() => {
                        self.beep(now, 30);
                        self.wave_coupling_note(now);
                    }
                    Some(2) => {
                        self.beep(now, 30);
                        self.sp_popup_open(su::Popup::Ch1(0));
                    }
                    Some(_) => {
                        self.beep(now, 30);
                        self.sp_popup_open(su::Popup::Mf(2));
                    }
                    None => {}
                }
                Some(false)
            }
            Gesture::SwipeLeft | Gesture::SwipeRight if !self.sp.swipes => Some(false),
            Gesture::KeyDown(c) => {
                self.beep(now, 50);
                self.sp_key(clk, c);
                Some(false)
            }
            _ => None,
        }
    }

    /// One move report of the scope touch.
    fn sp_move(&mut self, p: Point, now: u64) {
        let Some(mut t) = self.sp.touch else { return };
        t.last = p;
        let d0 = (p.x as i32 - t.p0.x as i32).abs().max((p.y as i32 - t.p0.y as i32).abs());
        t.far = t.far.max(d0);
        if !t.moving {
            if t.far <= TAP_TOL || !t.grabbed {
                self.sp.touch = Some(t);
                return;
            }
            t.moving = true;
            // Hold the acquisition from the first move: no frame (paint,
            // measurements) between the finger and the marker while the
            // axis is still being decided (owner 2026-10-25: the level drag
            // lagged). The release applies / refreshes as for any drag.
            self.sp.frozen = true;
        }
        let (mut dx, mut dy) = (p.x as i32 - t.applied.x as i32, p.y as i32 - t.applied.y as i32);
        // One axis per gesture, decided by the first motion (every drag
        // mode; the stock per-event split is gone): scope_ui::drag_axis /
        // drag_delta.
        if t.axis.is_none() {
            match su::drag_axis((p.x as i32 - t.p0.x as i32).abs(), (p.y as i32 - t.p0.y as i32).abs()) {
                Some(h) => {
                    t.axis = Some(if h { Axis::H } else { Axis::V });
                    // The item: the nearest marker of that axis to the press
                    // (owner 2026-10-25: drags ignore the selection); it is
                    // the selection from now on (filled, its guide line).
                    let m = self.sp_model(now);
                    if let Some((level, id)) = su::drag_target(&m, h, t.p0.x as i16, t.p0.y as i16) {
                        if level {
                            self.sp.sel_v = id;
                        } else {
                            self.sp.sel_h = id;
                        }
                        self.sp.last_h = !level;
                    }
                }
                None => {
                    self.sp.touch = Some(t);
                    return;
                }
            }
        }
        (dx, dy) = su::drag_delta(t.axis == Some(Axis::H), dx, dy);
        t.applied = p;
        self.sp.touch = Some(t);
        if dx == 0 && dy == 0 {
            return;
        }
        self.sp.guide_until = now + GUIDE_MS;
        if dy != 0 {
            self.sp_move_v(dy, now);
        } else {
            self.sp_move_h(dx, now);
        }
    }

    /// Vertical move of the selected level item by `dy` px.
    fn sp_move_v(&mut self, dy: i32, _now: u64) {
        let Some(mut t) = self.sp.touch else { return };
        // Pixels -> rows (combined view: the graph is shorter than 280 rows).
        let rows = if self.view == View::DmmScope {
            let h = 301 - su::COMBINED_TOP as i32;
            t.acc += dy * 280;
            let r = t.acc / (h - 1);
            t.acc -= r * (h - 1);
            r
        } else {
            dy
        };
        t.travel += dy;
        self.sp.touch = Some(t);
        if rows == 0 {
            return;
        }
        let mut st = self.scope.s;
        match self.sp.sel_v {
            0 => {
                // CH1 position; the trigger row moves with it (same voltage).
                let p = (st.pos + rows).clamp(-392, 392);
                st.trig_row += p - st.pos;
                st.runt_row += p - st.pos;
                st.pos = p;
                self.sp_code(3);
            }
            1 => {
                st.trig_row = (st.trig_row + rows).clamp(-10, 290);
                self.sp_code(1);
            }
            5 if st.ttype == crate::trigsoft::RUNT => {
                st.runt_row = (st.runt_row + rows).clamp(-10, 290);
                self.sp_code(8);
            }
            2 if self.sp.math.on => {
                // Math / FFT offset (the 'M' strip marker).
                let m = &mut self.sp.math;
                if m.mode == 5 {
                    m.fft_off = (m.fft_off + rows).clamp(-392, 392);
                } else {
                    m.abs_off = (m.abs_off + rows).clamp(-392, 392);
                }
                self.sp_math();
                self.sp_code(6);
                return;
            }
            k @ (3 | 4) => {
                let i = (k - 3) as usize;
                self.cur.y[i] = (self.cur.y[i] + rows).clamp(1, 279);
                self.cursor_values();
                self.sp_code(5 + 10 * i as u8);
                return;
            }
            _ => return,
        }
        self.sp_preview(st);
    }

    /// Horizontal move of the selected time item by `dx` px.
    fn sp_move_h(&mut self, dx: i32, _now: u64) {
        let Some(mut t) = self.sp.touch else { return };
        t.travel += dx;
        self.sp.touch = Some(t);
        match self.sp.sel_h {
            0 => {
                if self.scope.rolling() {
                    return; // roll: the trigger items are not draggable
                }
                self.sp_code(2);
                if !self.scope.s.run {
                    // Stopped: pan the stored record (applied on release).
                    if let Some(mut t) = self.sp.touch {
                        if t.anchor.is_some() {
                            t.pan += dx;
                            self.sp.touch = Some(t);
                            self.sp.frozen = true;
                            self.sp_render();
                        }
                    }
                    return;
                }
                let mut st = self.scope.s;
                st.hpos = (st.hpos + dx).clamp(0, scope::COLS as i32);
                self.sp_preview(st);
            }
            1 if self.sp.math.on && self.sp.math.mode == 5 => {
                // FFT centre (stock: centre -= dx, the spectrum follows the
                // finger); the last spectrum is re-drawn, no new one.
                let o = self.fft_hz_opts()[self.sp.math.hz.min(3) as usize];
                self.sp.math.ctr -= dx as f32 * o / 40.0;
                self.sp_math();
                self.sp_code(7);
            }
            k @ (2 | 3) => {
                let i = (k - 2) as usize;
                self.cur.x[i] = (self.cur.x[i] as i32 + dx).clamp(1, 399) as u16;
                self.cursor_values();
                self.sp_code(4 + 10 * i as u8);
            }
            _ => {}
        }
    }

    fn sp_code(&mut self, c: u8) {
        if let Some(t) = self.sp.touch.as_mut() {
            t.code = c;
        }
    }

    /// Drag preview of position settings: no hardware, no acquisition
    /// restart; the shown frame follows from the cache.
    fn sp_preview(&mut self, st: scope::Settings) {
        self.scope.preview(st);
        self.sp.frozen = true;
        self.sp_render();
    }

    /// Release of the scope touch: tap = timebase step; a drag applies its
    /// settings once.
    fn sp_release(&mut self, now: u64) {
        let Some(t) = self.sp.touch.take() else { return };
        self.sp.guide_until = now + GUIDE_MS;
        // DBG:GEST? log.
        if t.moving && t.code != 0 {
            let i = if self.gest_n < GEST_N {
                self.gest_n += 1;
                self.gest_n - 1
            } else {
                self.gest_log.copy_within(1.., 0);
                GEST_N - 1
            };
            let v = matches!(t.code, 1 | 3 | 5 | 6 | 8 | 15) as u8;
            self.gest_log[i] = (t.code, v, t.travel.clamp(-999, 999) as i16, now.saturating_sub(t.t0).min(9999) as u16);
        }
        if self.sp.frozen {
            self.sp.frozen = false;
            let s = self.scope.s;
            if !s.run && t.pan != 0 {
                if let Some(a) = t.anchor {
                    self.scope.pan_from(a, t.pan);
                }
            }
            if s.pos != t.snap.pos || s.trig_row != t.snap.trig_row || s.hpos != t.snap.hpos || s.runt_row != t.snap.runt_row {
                self.scope.apply(s, now);
                self.sp_settings_changed();
            } else {
                self.scope.refresh();
            }
            return;
        }
        // A still tap on the marker that was already selected: the next
        // overlapping one (the stock cycling; the press picked the nearest,
        // preferring the selection).
        if t.hit && t.still() && self.sp.presel {
            self.sp.presel = false;
            let m = self.sp_model(now);
            if let Some((level, id)) = su::graph_cycle(&m, self.sp.sel_v, self.sp.sel_h, self.sp.last_h, t.p0.x as i16, t.p0.y as i16) {
                if level {
                    self.sp.sel_v = id;
                } else {
                    self.sp.sel_h = id;
                }
                self.sp.last_h = !level;
            }
            return;
        }
        // UF-08: a tap on the cursor tab cycles the cursors.
        if self.sp.cur_press {
            self.sp.cur_press = false;
            if t.still() {
                self.beep(now, 30);
                self.sp_cur_cycle();
            }
            return;
        }
        // A tap on an armed tap-step zone steps once (a hold that already
        // repeated does not step again on release).
        if let Some(slower) = t.arm.release(t.still()) {
            self.sp.last_tap = None;
            self.sp_tb_step(slower, now);
            return;
        }
        // UF-09: a double tap where a tap does nothing (no marker, no
        // tap-step zone: the graph middle in Corners mode, anywhere with the
        // tap step Off; never in Halves, where every tap steps): CH1 position
        // to the centre and the trigger at 50 % (a stopped record: centred on
        // its trigger point).
        if t.still() && !t.hit {
            let p = t.p0;
            let dbl = self.sp.last_tap.is_some_and(|(t1, q)| {
                now.saturating_sub(t1) <= DOUBLE_MS && (p.x as i32 - q.x as i32).abs() <= 30 && (p.y as i32 - q.y as i32).abs() <= 30
            });
            if dbl {
                self.sp.last_tap = None;
                self.sp_centre(now);
            } else {
                self.sp.last_tap = Some((now, p));
            }
        } else {
            self.sp.last_tap = None;
        }
    }

    /// UF-09 double tap: CH1 position 0 (the trigger levels keep their
    /// volts), trigger position 50 %; stopped: the stored record centred on
    /// its trigger point.
    fn sp_centre(&mut self, now: u64) {
        let mut st = self.scope.s;
        st.trig_row -= st.pos;
        st.runt_row -= st.pos;
        st.pos = 0;
        st.hpos = 200;
        self.scope.centre_stored();
        self.beep(now, 60);
        self.sp.guide_until = now + GUIDE_MS;
        self.scope_set(st, now);
    }

    /// UF-08 cursor tab: off -> X -> Y -> off. The cursors are the stock
    /// cursor measurements (X2-1 + Xf, Y2-1) in the measurement row; the
    /// selection from before the first tap comes back with "off".
    fn sp_cur_cycle(&mut self) {
        const XS: [u8; 2] = [7, 8];
        const YS: [u8; 1] = [17];
        let cur = |id: &u8| XS.contains(id) || YS.contains(id);
        let state = self.sp_cur_state();
        let base = match self.sp.cur_saved {
            Some(b) if state != su::CUR_OFF => b,
            _ => {
                let b = (self.meas_sel, self.meas_n);
                self.sp.cur_saved = Some(b);
                b
            }
        };
        let rest = base.0[..base.1].iter().copied().filter(|id| !cur(id));
        let mut sel = [0u8; 4];
        let mut n = 0;
        let lead: &[u8] = match state {
            su::CUR_OFF => &XS,
            su::CUR_X => &YS,
            _ => &[],
        };
        if lead.is_empty() {
            // Back to the saved selection.
            sel = base.0;
            n = base.1;
            self.sp.cur_saved = None;
        } else {
            for id in lead.iter().copied().chain(rest) {
                if n < 4 {
                    sel[n] = id;
                    n += 1;
                }
            }
        }
        self.meas_sel = sel;
        self.meas_n = n;
        self.cursors_from_selection();
        self.cursor_values();
        self.sp_cells();
    }

    /// Cursor tab state from the measurement selection.
    fn sp_cur_state(&self) -> u8 {
        let sel = &self.meas_sel[..self.meas_n];
        let x = sel.contains(&7) || sel.contains(&8);
        let y = sel.contains(&17);
        match (x, y) {
            (true, false) => su::CUR_X,
            (false, true) => su::CUR_Y,
            (false, false) => su::CUR_OFF,
            // Both (chosen in the Measure tab): the next tap turns them off.
            _ => su::CUR_Y,
        }
    }

    fn sp_recentre(&mut self, now: u64) {
        let mut st = self.scope.s;
        if self.sp.last_h {
            if self.sp.sel_h == 1 {
                // FFT centre: the span starts at DC again.
                self.sp.math.ctr = 0.0;
                self.sp_math();
                self.beep(now, 60);
                self.sp.touch = None;
                return;
            }
            if self.sp.sel_h != 0 {
                return;
            }
            st.hpos = 200;
        } else {
            match self.sp.sel_v {
                2 => {
                    let m = &mut self.sp.math;
                    if m.mode == 5 {
                        m.fft_off = Math::DEFAULT.fft_off;
                    } else {
                        m.abs_off = Math::DEFAULT.abs_off;
                    }
                    self.sp_math();
                    self.beep(now, 60);
                    self.sp.touch = None;
                    return;
                }
                0 => {
                    st.trig_row -= st.pos;
                    st.runt_row -= st.pos;
                    st.pos = 0;
                }
                1 => {
                    let (hi, lo) = (self.meas_res.v[12], self.meas_res.v[13]);
                    if !(hi.is_finite() && lo.is_finite()) {
                        return;
                    }
                    st.trig_row = (self.scope.row_of_volts((hi + lo) / 2.0) as i32).clamp(-10, 290);
                }
                _ => return,
            }
        }
        self.beep(now, 60);
        self.sp.touch = None;
        self.sp.guide_until = now + GUIDE_MS;
        self.scope_set(st, now);
    }

    /// Tap-step of the timebase (true = slower): applied with a 30 ms beep
    /// and the stepped tab's pressed style; Halves: the tabs stay 2 s (the
    /// guide-line time). Nothing at the limit (its tab is hidden).
    fn sp_tb_step(&mut self, slower: bool, now: u64) -> bool {
        let mut st = self.scope.s;
        let top = self.tb_top() as usize;
        let lo = if self.scope.wave_on() { scope::WAVE_TB_MIN } else { scope::TB_MIN };
        let tb = if slower { (st.tb + 1).min(top) } else { st.tb.saturating_sub(1).max(lo) };
        if tb == st.tb {
            return false;
        }
        st.tb = tb;
        self.beep(now, 30);
        self.scope_set(st, now);
        self.sp.step_until = now + FLASH_MS;
        self.sp.step_side = slower;
        if self.sp.tap_mode == su::TAP_HALVES {
            self.sp.tabs_until = now + GUIDE_MS;
        }
        true
    }

    /// End a held drag / pinch whose end may never arrive (a popup, AUTO,
    /// an overlay or a key took over): a pinch is dropped (nothing applied),
    /// a drag applies what it moved so far; the acquisition runs again
    /// (pre-flash review 7389e8a B1: a stale pinch kept it held and carried
    /// its index into the other axis).
    pub(super) fn sp_end_hold(&mut self, now: u64) {
        if self.sp.pinch.is_some() {
            self.sp_pinch_end(false, now);
        }
        let held = core::mem::replace(&mut self.sp.frozen, false);
        if let Some(t) = self.sp.touch.take() {
            if held {
                let s = self.scope.s;
                if !s.run && t.pan != 0 {
                    if let Some(a) = t.anchor {
                        self.scope.pan_from(a, t.pan);
                    }
                }
                self.scope.apply(s, now);
                self.sp_settings_changed();
            }
        } else if held {
            let s = self.scope.s;
            self.scope.apply(s, now);
        }
    }

    // ------------------------------------------------------------ pinch

    fn sp_pinch(&mut self, axis: Axis, s0: u16, s: u16) {
        if self.sp.touch.is_some() {
            self.sp.touch = None;
        }
        let st = self.scope.s;
        let h = axis == Axis::H;
        let prev = self.sp.pinch.map(|p| su::PinchState { horizontal: p.axis == Axis::H, i0: p.i0, i: p.i });
        let tb_max = if self.scope.roll { scope::TB_MAX } else { scope::TB_YT_MAX };
        let tb_min = if self.scope.wave_on() { scope::WAVE_TB_MIN } else { scope::TB_MIN };
        let mode = pinch_mode();
        let r = s.max(1) as f32 / s0.max(1) as f32;
        let n = if axis == Axis::V && self.scope.wave_on() {
            // The wave view: its V/div step list (app_wave.rs), any mode.
            let i0 = prev.filter(|p| !p.horizontal).map_or(self.wave_step_now(), |p| p.i0);
            su::PinchState { horizontal: false, i0, i: self.wave_pinch_step(i0, r) }
        } else {
            su::pinch_step_mode(mode, prev, h, s0, s, st.tb, st.range, tb_min, tb_max)
        };
        let p = Pinch { axis, i0: n.i0, i: n.i, r };
        let live_moved = mode == su::PINCH_LIVE && self.sp.pinch.map_or(true, |q| q.r != r);
        let changed = prev.map_or(true, |q| q != n) || live_moved;
        if changed || !self.sp.bubble {
            self.sp.pinch = Some(p);
            self.sp_render();
            if !self.redraw && self.overlay.is_none() {
                self.sp.bubble = true;
                self.sp.bubble_dirty = true;
                su::graph().set_hole(Some(BUBBLE));
                self.sp_update(self.now);
            }
        }
    }

    fn sp_pinch_end(&mut self, ok: bool, now: u64) {
        let Some(p) = self.sp.pinch.take() else { return };
        self.sp.bubble = false;
        su::graph().set_hole(None);
        let mut st = self.scope.s;
        if ok && p.i != p.i0 && p.axis == Axis::V && self.scope.wave_on() {
            self.wave_step_to(p.i, now);
            self.sp_render();
        } else if ok && p.i != p.i0 {
            if p.axis == Axis::H {
                st.tb = p.i;
            } else {
                st.range = p.i;
            }
            self.scope_set(st, now);
        } else {
            self.sp_render();
        }
    }

    /// The pinch value bubble (opendm40 addition, stock purple toast style).
    fn sp_bubble_draw(&self) {
        let Some(p) = self.sp.pinch else { return };
        let mut b = su::Buf::new();
        if p.axis == Axis::H {
            b.push(su::TB_LABELS[p.i.min(29)]).push("/div");
        } else if self.scope.wave_on() {
            su::fmt_div(&mut b, self.wave_step_vdiv(p.i), if self.scope.wave_scale_now().amps { "A" } else { "V" });
            b.push("/div");
        } else {
            su::fmt_div(&mut b, scope::VDIV[p.i.min(9)] * self.scope.s.probe, "V");
            b.push("/div");
        }
        let t = sf::Ink::new(sf::SC_F16, BUBBLE, b.as_str(), 8, 0);
        let bg = su::C_FILL;
        sf::paint(BUBBLE, &[(1, bg), (0, su::C_PURPLE)], |x, y| {
            if let Some(g) = t.at(x, y) {
                return (su::C_WHITE, 2 << 8 | g as u16);
            }
            if sf::in_rbox(BUBBLE, 8, 0x0f, x, y) { (su::C_PURPLE, 1 << 8) } else { (bg, 0) }
        });
    }

    // ------------------------------------------------------------ panel, keys

    fn sp_panel(&mut self, _clk: &mut Clock, k: usize, now: u64) {
        let mut st = self.scope.s;
        if self.scope.wave_on() && k <= 2 {
            // The wave view: V/div = its step list; CH1 = the coupling note.
            match k {
                1 => self.wave_coupling_note(now),
                _ => self.wave_range_step(k == 0, now),
            }
            return;
        }
        match k {
            // UF-10: B1 = zoom in (smaller V/div), B3 = zoom out.
            0 => st.range = st.range.saturating_sub(1),
            2 => st.range = (st.range + 1).min(9),
            1 => {
                self.sp_popup_open(su::Popup::Ch1(0));
                return;
            }
            4 => {
                // Stock: the math label turns math on and opens the popup
                // (a second tap on it, popup open, turns math off).
                if !self.sp.math.on {
                    self.sp.math.on = true;
                    self.sp_math_changed(true);
                }
                self.sp_popup_open(su::Popup::Math(0));
                return;
            }
            6 => {
                self.sp_popup_open(su::Popup::Mf(2));
                return;
            }
            3 | 5 => {
                // Math scale +1 / -1: FFT dB/div, else the math V/div.
                let m = &mut self.sp.math;
                let up = k == 3;
                let step = |v: u8, max: u8| if up { (v + 1).min(max) } else { v.saturating_sub(1) };
                if m.mode == 5 && !m.vrms {
                    m.db = step(m.db, 12);
                } else {
                    m.vdiv = step(m.vdiv, 21);
                }
                self.sp_math();
                return;
            }
            _ => return,
        }
        self.scope_set(st, now);
    }

    fn sp_key(&mut self, clk: &mut Clock, code: u8) {
        let now = clk.now_ms();
        if self.scope.hold && (code == 1 || code == 2) {
            self.toast(now, "Saving CSV, wait", false);
            return;
        }
        match code {
            0 => self.show(View::Menu, clk),
            1 => {
                let mut s = self.scope.s;
                s.run = !s.run;
                self.scope_set(s, now);
            }
            2 => self.autoset_start(now),
            _ => self.sp_popup_open(su::Popup::Mf(0)),
        }
    }

    // ------------------------------------------------------------ popups

    fn sp_popvals(&self) -> su::PopVals {
        let s = &self.scope.s;
        su::PopVals {
            meas: self.meas_sel,
            meas_n: self.meas_n as u8,
            roll: self.roll,
            persist: self.sp.persist,
            bright: self.sp.bright,
            grid: self.sp.grid,
            rising: s.rising,
            mode: match s.mode {
                scope::TrigMode::Auto => 0,
                scope::TrigMode::Normal => 1,
                scope::TrigMode::Single => 2,
            },
            ac: s.ac,
            probe: probe_idx(s.probe) as u8,
            colour: self.sp.colour,
            math_colour: self.sp.math.colour,
            pressed_cell: self.sp.pressed_cell,
            math_mode: self.sp.math.mode,
            fft_hz: self.sp.math.hz,
            hz_opts: self.fft_hz_opts().map(|f| {
                let mut b = su::Buf::new();
                if f > 0.0 {
                    su::fmt_hz125(&mut b, f);
                } else {
                    b.push("--");
                }
                su::Txt::new(b.as_str())
            }),
            fft_win: self.sp.math.win,
            fft_vrms: self.sp.math.vrms,
            ttype: s.ttype,
            pcond: s.pcond,
            pwidth: s.pwidth,
            runt_v: self.scope.runt_volts(),
            tsrc: s.tsrc,
            gen_trace: self.scope.gen_trace,
            ch2_on: self.ch2.on,
            ch2_vdiv: self.ch2_label(false),
        }
    }

    fn sp_popup_draw(&mut self) {
        // The box's fills wait for its widgets (lcd.rs "deferred fills").
        crate::lcd::defer_start();
        let v = self.sp_popvals();
        su::draw_popup(self.sp.popup, &v);
    }

    /// Open (or switch) the popup; the CH1 / math label turns grey.
    fn sp_popup_open(&mut self, p: su::Popup) {
        self.sp_end_hold(self.now);
        self.sp.popup = p;
        self.sp.list = None;
        self.sp.tb_drawn = (255, false);
        self.sp.rec_drawn = None;
        if !self.redraw {
            // The CH1 / math pill label shows the open popup (0x7bef).
            let m = self.sp_model(self.now);
            su::panel_button(&m, 1);
            su::panel_button(&m, 4);
            self.sp_popup_draw();
        }
    }

    /// Every popup lies inside the graph frame: the scope page repaints
    /// only that (`sp_uncover`, the next tick); the combined view, whose
    /// popups also cover the meter strip, repaints the whole page.
    #[inline(never)] // region C (link-native.x)
    fn sp_popup_close(&mut self) {
        let grey = match self.sp.popup {
            su::Popup::Ch1(_) => 1,
            su::Popup::Math(_) => 2,
            _ => 0,
        };
        self.sp.popup = su::Popup::None;
        self.sp.pressed_cell = None;
        self.sp.slider = None;
        self.sp.list = None;
        if self.view == View::Scope {
            self.sp.uncover |= 0x80 | grey;
        } else {
            self.redraw = true;
        }
    }

    /// The page after a popup (`sp_popup_close`): the graph and its frame
    /// repainted, the rest by the page's model diff (a popup only stops
    /// the page drawing). The whole page took 262 ms on the device
    /// (DBG:PROF?, 2026-10-26), and the tap's beep lasted as long.
    #[inline(never)] // region C (link-native.x): `tick` checks the flag
    pub(super) fn sp_uncover(&mut self) {
        let u = core::mem::take(&mut self.sp.uncover);
        if !self.sp_on() || self.sp.covered() {
            return;
        }
        // As `repaint`: a stopped record re-rendered, a screenshot (MF
        // menu "Screenshot" closes it first) after SHOT_SETTLE_MS.
        self.scope_invalidate();
        self.repaint_at = self.now;
        crate::lcd::defer_start();
        let m = self.sp_model(self.now);
        su::uncover(&m, u & 1 != 0, u & 2 != 0);
        self.sp_draw_page();
        self.status_next = 0;
    }

    /// Every gesture while a popup is open (modal).
    fn sp_popup_gesture(&mut self, clk: &mut Clock, g: Gesture) -> Option<bool> {
        let now = clk.now_ms();
        let v = self.sp_popvals();
        match g {
            Gesture::KeyDown(0) => {
                self.beep(now, 50);
                self.sp_popup_close();
                self.show(View::Menu, clk);
            }
            Gesture::KeyDown(3) => {
                self.beep(now, 50);
                self.sp_popup_close();
            }
            Gesture::KeyDown(_) => self.beep(now, 50), // RUN / AUTO swallowed
            Gesture::Down(p) => {
                let (x, y) = (p.x as i16, p.y as i16);
                if matches!(self.sp.popup, su::Popup::Tb(_)) && su::TB_SPIN.has(x, y) {
                    // The timebase list: drag scrolls, a still tap picks.
                    self.sp.list = Some(ListTouch { p0: p, y: p.y as i16, far: 0, acc: 0 });
                    gesture::raw_next(true);
                    return Some(true);
                }
                if let su::Popup::Mf(t) = self.sp.popup {
                    match su::mf_hit(t, &v, x, y) {
                        su::MfHit::Cell(i) => {
                            self.sp.pressed_cell = Some(i);
                            self.sp_popup_draw();
                        }
                        h @ (su::MfHit::Bright | su::MfHit::Grid) => {
                            self.sp.slider = Some((h == su::MfHit::Grid) as u8);
                            self.sp_slider(x);
                            gesture::raw_next(true);
                            return Some(true);
                        }
                        _ => {}
                    }
                }
            }
            Gesture::Move(_, p) | Gesture::Drag(_, p, _) if self.sp.slider.is_some() => {
                self.sp_slider(p.x as i16);
                return Some(true);
            }
            Gesture::Move(_, p) | Gesture::Drag(_, p, _) if self.sp.list.is_some() => {
                self.sp_list_move(p);
                return Some(true);
            }
            Gesture::Up(_) => {
                self.sp.slider = None;
                self.sp_list_release(now);
            }
            Gesture::Cancel if self.sp.list.is_some() => {
                // A second finger: the list goes back to the timebase in use.
                self.sp.list = None;
                let tb = self.scope.s.tb as u8;
                self.sp.popup = su::Popup::Tb(tb);
                su::draw_tb(tb);
            }
            Gesture::Tap(p) => {
                self.beep(now, 30);
                let (x, y) = (p.x as i16, p.y as i16);
                let had_press = self.sp.pressed_cell.take().is_some();
                match self.sp.popup {
                    // A toast from the tap (CH2 refused, "Generator is
                    // off", ...) waits for the popup's repaint (`toast_hold`).
                    su::Popup::Mf(t) => {
                        self.toast_hold = true;
                        self.sp_mf_tap(clk, t, su::mf_hit(t, &v, x, y), now);
                        self.sp_toast_release(now);
                    }
                    su::Popup::Ch1(t) => {
                        self.toast_hold = true;
                        self.sp_side_tap(false, t, su::side_hit(false, t, &v, x, y), now);
                        self.sp_toast_release(now);
                    }
                    su::Popup::Math(_) if su::panel_hit(p.x, p.y) == Some(4) => {
                        // Stock: the math label with its popup open = math off.
                        self.sp.math.on = false;
                        self.sp_math_changed(false);
                        self.sp_popup_close();
                    }
                    su::Popup::Math(t) => {
                        self.toast_hold = true;
                        self.sp_side_tap(true, t, su::side_hit(true, t, &v, x, y), now);
                        self.sp_toast_release(now);
                    }
                    su::Popup::Tb(i) => match su::tb_hit(i, x, y) {
                        su::TbHit::Close => self.sp_popup_close(),
                        su::TbHit::Row(r) => self.sp_tb_pick(r, now),
                        su::TbHit::Inside => {}
                    },
                    su::Popup::None => {}
                }
                if had_press && self.sp.popup != su::Popup::None && !self.redraw {
                    self.sp_popup_draw();
                }
            }
            Gesture::MultiSwipeDown(_) => return None, // screenshot / power-off
            _ => {
                // A touch that ended as no tap releases a pressed cell.
                if self.sp.pressed_cell.take().is_some() && !self.redraw {
                    self.sp_popup_draw();
                }
            }
        }
        Some(false)
    }

    /// One move of a timebase-list touch: whole rows per TB_ROW_PX of
    /// vertical travel once it is a drag; nothing is applied until release.
    fn sp_list_move(&mut self, p: Point) {
        let (Some(mut l), su::Popup::Tb(cur)) = (self.sp.list, self.sp.popup) else { return };
        let dy = p.y as i32 - l.y as i32;
        l.y = p.y as i16;
        l.far = l.far.max((p.x as i32 - l.p0.x as i32).abs().max((p.y as i32 - l.p0.y as i32).abs()));
        l.acc += dy;
        if l.far > TAP_TOL {
            let (i, rem) = su::tb_scroll(cur, l.acc);
            l.acc = rem;
            if i != cur {
                self.sp.popup = su::Popup::Tb(i);
                su::draw_tb(i);
            }
        }
        self.sp.list = Some(l);
    }

    /// Release of a timebase-list touch: a still tap picks its row, a drag
    /// applies the row it settled on (one scope_set per pick).
    fn sp_list_release(&mut self, now: u64) {
        let (Some(l), su::Popup::Tb(cur)) = (self.sp.list.take(), self.sp.popup) else { return };
        let pick = if l.far <= TAP_TOL {
            match su::tb_hit(cur, l.p0.x as i16, l.p0.y as i16) {
                su::TbHit::Row(r) => r,
                _ => return,
            }
        } else {
            cur
        };
        self.beep(now, 30);
        self.sp_tb_pick(pick, now);
    }

    /// Timebase picked in the list (`su::tb_pick`). A roll-only entry
    /// (beyond 1 s/div, orange) turns Roll on, except in Single (it stays at
    /// the YT maximum, Roll untouched); the list then shows what the scope
    /// took.
    fn sp_tb_pick(&mut self, i: u8, now: u64) {
        let mut st = self.scope.s;
        let (i, roll_on) = su::tb_pick(i, self.scope.roll, st.mode == scope::TrigMode::Single);
        if roll_on {
            self.roll = true;
            self.scope.roll = true;
        }
        if i as usize != st.tb || roll_on {
            st.tb = i as usize;
            self.scope_set(st, now);
        }
        let tb = self.scope.s.tb as u8;
        self.sp.popup = su::Popup::Tb(tb);
        su::draw_tb(tb);
    }

    fn sp_slider(&mut self, x: i16) {
        let val = (su::slider_value(x) * 100.0 + 0.5) as u8;
        match self.sp.slider {
            Some(0) => self.sp.bright = val,
            Some(_) => self.sp.grid = val,
            None => return,
        }
        let v = self.sp_popvals();
        su::mf_display_values(&v);
    }

    /// End of a popup tap (`toast_hold`): its toast now, over the popup
    /// drawn again.
    fn sp_toast_release(&mut self, now: u64) {
        self.toast_hold = false;
        if self.toast_later_n != 0 && !self.overlay.is_some_and(|o| o.modal()) {
            self.toast_later_show(now);
        }
    }

    fn sp_mf_tap(&mut self, clk: &mut Clock, tab: u8, h: su::MfHit, now: u64) {
        let mut st = self.scope.s;
        match h {
            su::MfHit::Close => {
                self.sp_popup_close();
                return;
            }
            su::MfHit::Outside | su::MfHit::Inside | su::MfHit::Bright | su::MfHit::Grid => return,
            su::MfHit::Tab(t) => {
                if t != tab {
                    let old = su::popup_box(self.sp.popup);
                    self.sp.popup = su::Popup::Mf(t);
                    let new = su::popup_box(self.sp.popup);
                    if let (View::Scope, Some(o), Some(n)) = (self.view, old, new) {
                        // The new box, then the graph rows the old one
                        // covered and the new one does not.
                        self.sp_popup_draw();
                        let m = self.sp_model(now);
                        su::regraph_rows(&m, n, n.y + n.h, o.y + o.h);
                    } else {
                        // The combined view (its popups cover the meter
                        // strip): the whole page, the graph skipping the
                        // new box (sp_draw_page).
                        self.redraw = true;
                    }
                }
                return;
            }
            su::MfHit::Cell(i) => {
                // Refuse a duplicate or a 5th item silently (stock).
                if self.meas_n < 4 && !self.meas_sel[..self.meas_n].contains(&i) {
                    self.meas_sel[self.meas_n] = i;
                    self.meas_n += 1;
                    self.cursors_from_selection();
                }
            }
            su::MfHit::SelCell(k) => {
                let k = k as usize;
                if k < self.meas_n {
                    self.meas_sel.copy_within(k + 1..self.meas_n, k);
                    self.meas_n -= 1;
                    self.cursors_from_selection();
                }
            }
            su::MfHit::Clear => {
                self.meas_n = 0;
                self.cursors_from_selection();
            }
            su::MfHit::Default => {
                self.meas_sel = crate::scope_meas::DEFAULT_SEL;
                self.meas_n = 4;
                self.cursors_from_selection();
            }
            su::MfHit::Roll(on) => {
                self.roll = on;
                self.scope.roll = on;
                // Stock: roll on with a slower index enters roll at 22.
                self.scope_set(st, now);
            }
            su::MfHit::Persist(i) => {
                self.sp.persist = i.min(3);
                self.sp_settings_changed();
            }
            su::MfHit::Rising(r) => {
                st.rising = r;
                self.scope_set(st, now);
            }
            su::MfHit::TType(t) => {
                st.ttype = t.min(2);
                if t == crate::trigsoft::RUNT && (st.runt_row - st.trig_row).abs() < 10 {
                    // A second level 2 div above the first (below at the top).
                    st.runt_row = if st.trig_row >= 80 { st.trig_row - 70 } else { st.trig_row + 70 };
                }
                self.scope_set(st, now);
                // The page height is the same, the rows below change.
            }
            su::MfHit::TSrc(t) => {
                st.tsrc = t.min(1);
                self.scope_set(st, now);
                if st.tsrc == scope::TSRC_GEN && !self.gen.s.on {
                    self.toast(now, "Generator is off", false);
                }
            }
            su::MfHit::Ch2(on) => {
                self.ch2_set(clk, on);
            }
            su::MfHit::Ch2Step(up) => {
                self.ch2_step(up, now);
            }
            su::MfHit::GenTrace(on) => {
                self.scope.gen_trace = on;
                // The next record is taken without the comparator (or
                // with it again): restart the acquisition.
                self.scope_set(st, now);
                self.scope_invalidate();
            }
            su::MfHit::PCond(c) => {
                st.pcond = c.min(2);
                self.scope_set(st, now);
            }
            su::MfHit::Width(up) => {
                st.pwidth = crate::trigsoft::width_step(st.pwidth, up);
                self.scope_set(st, now);
            }
            su::MfHit::Mode(m) => {
                st.mode = [scope::TrigMode::Auto, scope::TrigMode::Normal, scope::TrigMode::Single][m.min(2) as usize];
                st.run = true;
                self.scope_set(st, now);
            }
            su::MfHit::PriScr => {
                // Close first: the screenshot shows the page (opendm40).
                self.sp_popup_close();
                self.shot_request(now);
                return;
            }
            su::MfHit::SaveCsv => {
                self.sp_popup_close();
                self.sp_save_csv(now);
                return;
            }
        }
        let _ = clk;
        self.sp_popup_draw();
    }

    fn sp_side_tap(&mut self, math: bool, tab: u8, h: su::SideHit, now: u64) {
        let mut st = self.scope.s;
        match h {
            su::SideHit::Close => {
                self.sp_popup_close();
                return;
            }
            su::SideHit::Inside => return,
            su::SideHit::Tab(t) => {
                if t == tab {
                    return;
                }
                self.sp.popup = if math { su::Popup::Math(t) } else { su::Popup::Ch1(t) };
            }
            su::SideHit::Coupling(c) => {
                st.ac = c == 1;
                self.scope_set(st, now);
            }
            su::SideHit::Probe(i) => {
                st.probe = scope::PROBES[(i as usize).min(12)];
                self.scope_set(st, now);
            }
            su::SideHit::Colour(i) => self.sp.colour = i.min(8),
            su::SideHit::MathMode(md) => {
                if md != self.sp.math.mode {
                    self.sp.math.mode = md;
                    self.sp_math_changed(true);
                }
            }
            su::SideHit::MathColour(i) => self.sp.math.colour = i.min(8),
            su::SideHit::HzDiv(i) => {
                self.sp.math.hz = i.min(3);
                self.sp_math();
            }
            su::SideHit::Window(i) => {
                if i != self.sp.math.win {
                    self.sp.math.win = i.min(5);
                    self.sp_math_changed(true);
                }
            }
            su::SideHit::Unit(i) => {
                self.sp.math.vrms = i == 1;
                self.sp_math();
            }
        }
        self.sp_popup_draw();
    }

    // ------------------------------------------------------------ AUTO-set

    /// AUTO-set (stock mode 4, scope_gen_ui_v2_6.h sec. 4), phase 1: 5 us/div,
    /// position 0, V/div 10 V, 1 V, 100 mV, 10 mV until a record with a swing
    /// (30 % / 70 % levels more than 19 codes apart) is seen; each candidate
    /// waits 200 ms (relays, offset PWM) before its record counts. UF-04: the
    /// user's coupling is kept (the stock forces DC); only when an AC search
    /// finds nothing, it is repeated in DC.
    ///
    /// opendm40 fixes (device 2026-10-14, 100 Hz 0..9.6 V square ended at 5 us/div,
    /// 2 V/div, flat top): a 5 us/div record of a slow signal holds one edge
    /// or none, so phase 1 found a swing without a period (n < 2 crossings)
    /// and phase 2 took the timebase from Cycl of a one-edge / flat record
    /// (the noise-floor ~9 us); or a flat record clipped at a finer range let
    /// the descent go on until noise passed at 10 mV/div.
    ///  * Phase 1: a record flat at the ADC rail (clipped, no swing) means the
    ///    signal is beyond this range: the previous (coarser) candidate is
    ///    taken without descending further.
    ///  * Phase 3 (new): range known but no period: the period is measured at
    ///    AS_PERIOD_TB (500 us/div, then 20 ms/div); none at all (DC, < 5 Hz):
    ///    20 ms/div is kept.
    ///  * Phase 2: a clipped record moves to a coarser range and measures
    ///    again (at most AS_P2_MAX records); the timebase comes from the
    ///    30 / 70 % crossing period (>= 2 crossings), Cycl only when it agrees
    ///    with it within 2x; V/div = the finest that holds the swing in 6.4 div
    ///    with the centre at most 4 div off 0 V (the stock demands the whole
    ///    signal within +-4 div before centring, 5 V/div for 0..9.6 V); a range
    ///    that clipped in phase 2 is never the result. A result finer than the
    ///    measuring range is measured once more at that range (the centre
    ///    from a 10 V/div record is off by up to ~1 V).
    /// Each record's decision is logged (DBG:GEST? after "|AUTO", `autoset_log`).
    pub(super) fn autoset_start(&mut self, now: u64) {
        if self.scope.wave_on() {
            self.sp_end_hold(now);
            self.wave_autoset(now);
            return;
        }
        self.sp_end_hold(now);
        let saved = self.scope.s;
        let mut st = saved;
        st.tb = 8;
        st.range = AS_CAND[0];
        st.pos = 0;
        st.mode = scope::TrigMode::Auto;
        st.run = true;
        as_log_clear();
        self.sp.autoset = Some(Autoset { phase: 1, idx: 0, round: 0, saved, settle: now + 200, abort: now + 15_000, refined: false, min_r: 0 });
        self.scope.apply(st, now);
    }

    /// One record during AUTO-set.
    pub(super) fn autoset_step(&mut self, f: &scope::Frame, now: u64) {
        let Some(mut a) = self.sp.autoset else { return };
        if now < a.settle || f.data.len() < 16 {
            return;
        }
        let mut st = self.scope.s;
        let (mut lo, mut hi) = (1023u16, 0u16);
        for &v in f.data {
            lo = lo.min(v);
            hi = hi.max(v);
        }
        let pp = (hi - lo) as f32;
        let (l30, l70) = (lo as f32 + 0.3 * pp, lo as f32 + 0.7 * pp);
        let swing = l70 - l30 > 19.0;
        let clipped = lo == 0 || hi > 1022;
        // Period from rising crossings of the 30 % / 70 % levels.
        let (mut armed, mut first, mut last, mut n) = (false, 0usize, 0usize, 0u32);
        if swing {
            for (i, &v) in f.data.iter().enumerate() {
                let v = v as f32;
                if v < l30 {
                    armed = true;
                } else if armed && v > l70 {
                    armed = false;
                    if n == 0 {
                        first = i;
                    }
                    last = i;
                    n += 1;
                }
            }
        }
        let period = if n >= 2 { (last - first) as f32 / (n - 1) as f32 / f.fs as f32 } else { 0.0 };
        as_log(a.phase, st.tb, st.range, lo, hi, n, period);
        // Trigger level for the next record at the middle of this one's swing
        // (its Auto timeout is 2 s at 20 ms/div: an untriggered record there
        // costs that much), else the centre row.
        let vmid = self.scope.volts((lo as f32 + hi as f32) / 2.0);
        let mid_row = |st: &scope::Settings| {
            let r = 140.0 + st.pos as f32 - 141.0 * vmid / (4.0 * scope::VDIV[st.range] * st.probe);
            if swing && r.is_finite() { (r as i32).clamp(10, 270) } else { 140 }
        };
        // Next step: apply `st` and wait for its record.
        let next = |me: &mut Self, a: Autoset, st: scope::Settings| {
            let mut a = a;
            a.settle = now + 200;
            me.sp.autoset = Some(a);
            me.scope.apply(st, now);
        };
        if a.phase == 1 {
            let beyond = clipped && !swing;
            if swing || beyond {
                if beyond {
                    // Flat at the rail: the signal is beyond this range.
                    st.range = if a.idx > 0 { AS_CAND[a.idx - 1] } else { 9 };
                } else if clipped {
                    st.range = (st.range + 3).min(9);
                }
                st.trig_row = mid_row(&st);
                a.idx = 0;
                if period > 0.0 {
                    st.tb = tb_for(period);
                    a.phase = 2;
                } else {
                    a.phase = 3;
                    st.tb = AS_PERIOD_TB[0];
                }
            } else if a.idx + 1 < AS_CAND.len() {
                a.idx += 1;
                st.range = AS_CAND[a.idx];
            } else if a.round == 0 || a.round == 2 {
                // Nothing at 5 us/div: the same search at 20 ms/div.
                a.round += 1;
                a.idx = 0;
                st.tb = 19;
                st.range = AS_CAND[0];
            } else if a.round == 1 && st.ac {
                // Nothing in AC: the same search in DC (UF-04).
                a.round = 2;
                a.idx = 0;
                st.ac = false;
                st.tb = 8;
                st.range = AS_CAND[0];
            } else {
                self.sp.autoset = None;
                self.scope_set(a.saved, now);
                self.toast(now, "AUTO: no signal found", false);
                return;
            }
            next(self, a, st);
            return;
        }
        if a.phase == 3 {
            // Range known, period not: coarser timebases until >= 2 cycles.
            if clipped && st.range < 9 {
                st.range = (st.range + 3).min(9);
            }
            st.trig_row = mid_row(&st);
            if period > 0.0 {
                st.tb = tb_for(period);
                a.phase = 2;
                a.idx = 0;
            } else if a.idx + 1 < AS_PERIOD_TB.len() {
                a.idx += 1;
                st.tb = AS_PERIOD_TB[a.idx];
            } else {
                // No period at all (DC or very slow): keep 20 ms/div.
                a.phase = 2;
                a.idx = 0;
            }
            next(self, a, st);
            return;
        }
        // Phase 2: a clipped record cannot be measured: coarser range, again.
        if clipped && st.range < 9 && (a.idx as u8) < AS_P2_MAX {
            a.idx += 1;
            a.min_r = a.min_r.max(st.range as u8 + 1);
            st.range = (st.range + 3).min(9);
            st.trig_row = mid_row(&st);
            next(self, a, st);
            return;
        }
        // V/div so that the swing fits 6.4 div, centred; trigger at the
        // middle; timebase from the measured cycle.
        let (vmax, vmin) = (self.scope.volts(hi as f32), self.scope.volts(lo as f32));
        let probe = st.probe;
        let mid = (vmax + vmin) / 2.0;
        let mut r = 9;
        for (i, &d) in scope::VDIV.iter().enumerate().skip(a.min_r as usize) {
            let d = d * probe;
            // The centre at most 4 div off 0 V (the stock's +-4 div window,
            // but for the centre only, so a 0..9.6 V signal gets 2 V/div).
            if (vmax - vmin) <= 6.4 * d && mid.abs() <= 4.0 * d {
                r = i;
                break;
            }
        }
        let coarser = r < st.range;
        st.range = r;
        st.pos = ((141.0 * mid / (4.0 * scope::VDIV[r] * probe)) as i32).clamp(-392, 392);
        st.trig_row = 140;
        let res = crate::scope_meas::compute(f.data, f.fs as f32, &self.scope);
        let cyc = res.v[9];
        if period > 0.0 {
            let ok = cyc.is_finite() && cyc > 0.0 && cyc < 2.0 * period && cyc > 0.5 * period;
            st.tb = tb_for(if ok { cyc } else { period });
        }
        // Measured at a coarser range than the result (10 V/div: ~11 codes
        // per volt): the centre and the swing are off by up to ~1 V (device:
        // 0..9.6 V centred 0.5 div high, trigger at 3.7 V). Measure once
        // more at the result range, centred by this estimate.
        if coarser && !a.refined && (a.idx as u8) < AS_P2_MAX {
            a.refined = true;
            a.idx += 1;
            as_log(5, st.tb, st.range, lo, hi, n, period);
            next(self, a, st);
            return;
        }
        as_log(4, st.tb, st.range, lo, hi, n, if cyc.is_finite() { cyc } else { -1.0 });
        st.mode = scope::TrigMode::Auto;
        self.sp.autoset = None;
        self.scope_set(st, now);
        self.redraw = true;
        if a.saved.ac && !st.ac {
            self.toast(now, crate::dialog::AUTO_DC, true);
        }
    }

    // ------------------------------------------------------------ persist

    /// Page settings -> persist words (p[1] bits 17..29, p[11] bits 1..6,
    /// 12..15). p[1] bits 28..29: tap-step mode XOR TAP_DEFAULT (older
    /// records: 0 = the default).
    pub(super) fn sp_persist(&self, p1: &mut u32, p11: &mut u32) {
        let s = &self.sp;
        *p1 |= ((s.colour as u32 ^ 2) & 0xF) << 17 | ((100 - s.bright.min(100)) as u32 & 0x7F) << 21;
        *p1 |= su::tap_code(s.tap_mode) << 28;
        *p11 |= (s.drag_mode as u32 & 3) << 1 | (s.swipes as u32) << 3 | (s.persist as u32 & 3) << 4 | (!s.press_fx as u32) << 6;
        *p11 |= ((s.grid.min(100) as u32 / 10) ^ 5) << 12;
    }

    /// Extension words (persist::W_SCOPE_EXT..): [0] math flags / indices,
    /// [1] math offsets, [2] FFT centre (f32 bits); 0 = defaults
    /// everywhere (fields stored XOR their default).
    pub(super) fn sp_persist_ext(&self, p: &mut [u32]) {
        let m = &self.sp.math;
        let d = Math::DEFAULT;
        p[0] = m.on as u32
            | ((m.mode == 6) as u32) << 1
            | ((m.colour ^ d.colour) as u32 & 0xF) << 2
            | ((m.db ^ d.db) as u32 & 0xF) << 6
            | ((m.vdiv ^ d.vdiv) as u32 & 0x1F) << 10
            | (m.vrms as u32) << 15
            | ((m.win ^ d.win) as u32 & 7) << 16
            | ((m.hz ^ d.hz) as u32 & 3) << 19;
        p[1] = ((m.fft_off - d.fft_off) as i16 as u16 as u32) | ((m.abs_off - d.abs_off) as i16 as u16 as u32) << 16;
        p[2] = m.ctr.to_bits();
        // [3] trigger type / pulse condition / runt row - default; [4] the
        // pulse width (f32 bits, 0 = default 1 ms).
        // The MCX settings (the wave view's trigger is not persisted).
        let s = &self.scope.mcx_settings();
        let dr = scope::Settings::DEFAULT.runt_row;
        // Bits 4 / 5: the trigger source GEN, the GEN trace shown.
        p[3] = (s.ttype as u32 & 3)
            | (s.pcond as u32 & 3) << 2
            | (s.tsrc as u32 & 1) << 4
            | (self.scope.gen_trace as u32) << 5
            | (self.ch2.on as u32) << 6
            | ((s.runt_row - dr) as i16 as u16 as u32) << 16;
        p[4] = if s.pwidth == scope::Settings::DEFAULT.pwidth { 0 } else { s.pwidth.to_bits() };
    }

    pub(super) fn sp_restore_ext(&mut self, p: &[u32]) {
        let d = Math::DEFAULT;
        let w = p[0];
        let ctr = f32::from_bits(p[2]);
        self.sp.math = Math {
            on: w & 1 != 0,
            mode: if w >> 1 & 1 != 0 { 6 } else { 5 },
            colour: ((w >> 2 & 0xF) as u8 ^ d.colour).min(8),
            db: ((w >> 6 & 0xF) as u8 ^ d.db).min(12),
            vdiv: ((w >> 10 & 0x1F) as u8 ^ d.vdiv).min(21),
            vrms: w >> 15 & 1 != 0,
            win: ((w >> 16 & 7) as u8 ^ d.win).min(5),
            hz: (w >> 19 & 3) as u8 ^ d.hz,
            fft_off: (d.fft_off + (p[1] as u16 as i16) as i32).clamp(-392, 392),
            abs_off: (d.abs_off + ((p[1] >> 16) as u16 as i16) as i32).clamp(-392, 392),
            ctr: if ctr.is_finite() && ctr >= 0.0 { ctr } else { 0.0 },
        };
        let mut st = self.scope.s;
        let dd = scope::Settings::DEFAULT;
        st.ttype = (p[3] & 3).min(2) as u8;
        st.pcond = (p[3] >> 2 & 3).min(2) as u8;
        st.tsrc = (p[3] >> 4 & 1) as u8;
        self.scope.gen_trace = p[3] >> 5 & 1 != 0;
        self.ch2.on = p[3] >> 6 & 1 != 0;
        self.scope.ch2_on = self.ch2.on;
        st.runt_row = dd.runt_row + (p[3] >> 16) as u16 as i16 as i32;
        st.pwidth = if p[4] == 0 { dd.pwidth } else { f32::from_bits(p[4]) };
        self.scope.apply(st, 0);
    }

    pub(super) fn sp_restore(&mut self, p1: u32, p11: u32) {
        let s = &mut self.sp;
        s.colour = ((p1 >> 17 & 0xF) ^ 2).min(8) as u8;
        s.bright = 100 - (p1 >> 21 & 0x7F).min(100) as u8;
        s.drag_mode = (p11 >> 1 & 3).min(1) as u8;
        s.swipes = p11 >> 3 & 1 != 0;
        s.persist = (p11 >> 4 & 3) as u8;
        s.press_fx = p11 >> 6 & 1 == 0;
        s.grid = (((p11 >> 12 & 0xF) ^ 5).min(10) * 10) as u8;
        s.tap_mode = su::tap_decode(p1 >> 28);
    }
}

/// AUTO-set V/div candidates (hardware ranges 10 V, 1 V, 100 mV, 10 mV).
const AS_CAND: [usize; 4] = [9, 6, 3, 0];
/// AUTO-set phase 3: timebases that look for the period when the range is
/// known but a 5 us/div record held fewer than two cycles (500 us/div: 50 us
/// .. 4.5 ms periods; 20 ms/div: up to 180 ms, sampled at 125 kS/s).
const AS_PERIOD_TB: [usize; 2] = [14, 19];
/// AUTO-set phase 2: at most this many re-measurements at a coarser range
/// after a clipped record.
const AS_P2_MAX: u8 = 3;

/// AUTO-set decision log (DBG:GEST? after "|AUTO"): per record phase (1
/// amplitude, 3 period, 2 final; 5 = re-measure at the result range, 4 =
/// the result), timebase, range, min /
/// max code, rising crossings, period (phase 4: Cycl) in us. SRAM, not the
/// TCM: the stack's headroom.
#[cfg_attr(target_os = "none", link_section = ".sram_bss")]
static mut AS_LOG: [(u8, u8, u8, u16, u16, u16, f32); 16] = [(0, 0, 0, 0, 0, 0, 0.0); 16];
static mut AS_LOG_N: usize = 0;

fn as_log_clear() {
    unsafe { AS_LOG_N = 0 };
}

fn as_log(phase: u8, tb: usize, range: usize, lo: u16, hi: u16, n: u32, period: f32) {
    unsafe {
        let i = AS_LOG_N;
        if i < 16 {
            (*core::ptr::addr_of_mut!(AS_LOG))[i] = (phase, tb as u8, range as u8, lo, hi, n.min(65535) as u16, period);
            AS_LOG_N = i + 1;
        }
    }
}

/// The AUTO-set log as text: `;A<phase>,<tb>,<range>,<lo>,<hi>,<n>,<us>`.
pub(super) fn autoset_log(out: &mut crate::scpi::Scpi) {
    let n = unsafe { AS_LOG_N };
    for i in 0..n {
        let (ph, tb, r, lo, hi, k, per) = unsafe { (*core::ptr::addr_of!(AS_LOG))[i] };
        out.put_bytes(b";A");
        for (j, v) in [ph as u32, tb as u32, r as u32, lo as u32, hi as u32, k as u32].iter().enumerate() {
            if j > 0 {
                out.put_bytes(b",");
            }
            crate::put_u32(out, *v);
        }
        out.put_bytes(b",");
        if per < 0.0 {
            out.put_bytes(b"-1");
        } else {
            crate::put_u32(out, (per * 1e6) as u32);
        }
    }
}

/// First timebase whose s/div exceeds a quarter period (FUN_08033388).
fn tb_for(period: f32) -> usize {
    scope::SDIV.iter().position(|&d| period / 4.0 < d).unwrap_or(scope::TB_MAX).clamp(scope::TB_MIN, scope::TB_YT_MAX)
}

const _: () = assert!(scope::CAP <= crate::scope_csv::REC_MAX, "scope CSV sized for the record");

/// The pinch value bubble: centred in the graph.
const BUBBLE: Rect = rect(153, 140, 120, 40);
