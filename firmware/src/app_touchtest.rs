//! The touch test page (View::TouchTest; the drawing is ui_touchtest.rs):
//! Settings 3/3 > Touch test, or DBG:TOUCH:TEST ON. Shows what the touch
//! controller reports, before and after the debounce (touch.rs), so the
//! calibration, the jitter and the touch-key strip can be judged by eye.
//!
//! Everything comes from the DBG:TOUCH? log (touch.rs `log_entry`), which
//! `Touch::poll` writes: one `F` entry per frame read (record 0's point
//! before the calibration, the point count, the key report, the stale
//! (status 0xAB) and bad marks) and one `E` entry per debounced state change (the point
//! count and the first point). The page keeps a cursor into the log and
//! draws every new entry after the poll (main.rs), so it needs no copy of
//! the frames and the main loop no extra stack. The log holds record 0
//! only: of a multi-touch frame the page shows the first point and the
//! count.
//!
//! On this page every touch and touch key is shown, not acted on: the
//! gesture tracker's events are swallowed (App::gesture), each touch is
//! captured at its Down (so its release never taps the page under it after
//! the exit). Exit: a short press of the power key (App::tick), or one
//! finger held still for HOLD_EXIT_MS anywhere. A tap on Clear repaints
//! the page (the dots go). The raw dots are the controller's points, the
//! debounced ones went through the touch calibration (touch.rs `screen`).
//!
//! Calibrate (touchcal.rs): a walk over the nine targets, one highlighted
//! at a time, TAPS short still taps on each. A tap's sample is the raw
//! point (the `F` entry) of the frame that made its debounced touch-down,
//! taken when the finger lifts without having moved. A tap more than
//! ACCEPT_PX from the target or a second tap more than SPREAD_PX from the
//! first is refused (the target starts again). After the last one the fit
//! is shown with its residuals; Save applies it at once (`touch::set_cal`)
//! and the settings record keeps it (persist.rs W_TOUCH_CAL, a
//! preference). A fit outside touchcal's limits offers Try again instead.
//! Reset (on the walk's screen) returns to no calibration. Cancel and the
//! power key leave the calibration unchanged; the hold-to-exit is off
//! during the walk and on the result screen.
//!
//! Region C (`.cold`, link-native.x): the page cannot open while the AD2
//! stream or a logger session runs (`tt_conflict`), main.rs calls
//! `tt_poll` only without a session, and a session that a host starts
//! while the page is up closes it (App::tick).

use super::*;
use crate::touch::{self, Point};
use crate::touchcal;
use crate::ui::touchtest as tt;

/// A touch held still this long exits the page, ms.
const HOLD_EXIT_MS: u32 = 2000;
/// From this hold time on the hint says "Keep holding", ms.
const HOLD_HINT_MS: u32 = 700;
/// "Still": within this distance of the touch-down point, px.
const HOLD_SLOP: i32 = 20;
/// The text lines repaint at most this often, ms (a frame comes every
/// ~10..20 ms while touched).
const TEXT_MS: u32 = 100;
/// Calibration: taps per target, a tap's largest distance from its target
/// and between the taps of one target (raw, px, per axis).
const TAPS: u8 = 2;
const ACCEPT_PX: i32 = 60;
const SPREAD_PX: i32 = 30;
const N_SAMP: usize = 9 * TAPS as usize;
/// Page modes: the test page, the calibration walk, the fit's result.
const M_TEST: u8 = 0;
const M_WALK: u8 = 1;
const M_RESULT: u8 = 2;

/// Page state. In SRAM (`.sram_bss`), not in App: TCM .bss shortens the
/// stack 1:1 and its headroom is the tight budget (docs/native-boot.md
/// "Memory budget"); SRAM has tens of KiB free.
struct Tt {
    /// Next DBG:TOUCH? log entry to read (touch::log_count numbering).
    cur: u32,
    /// Frames read since the page opened: all, stale (status 0xAB), bad
    /// (no 0xAB end mark or a failed read).
    frames: u32,
    stale: u32,
    bad: u32,
    /// The current debounced touch: down time (ms, low 32 bits; 0 = none)
    /// and point.
    down_at: u32,
    down_p: Point,
    /// The last raw pressed frame's point and point count.
    raw: Point,
    raw_n: u8,
    /// Most points of one frame since the page opened.
    max_pts: u8,
    /// Points of the last debounced state.
    pts: u8,
    /// FL_* flags.
    fl: u8,
    /// The last touch-down (the page's "Tap" line).
    tap: Point,
    /// Last repaint of the text lines.
    text_at: u32,
    /// Touch keys lit (bit = code) and the raw X of each key's last report
    /// (0 = none yet).
    keys_lit: u8,
    key_x: [u16; 4],
    /// M_TEST, M_WALK, M_RESULT.
    mode: u8,
    /// The walk: the current target (TARGETS index) and its tap (0..TAPS).
    ci: u8,
    ctap: u8,
    /// The raw point of the current touch's touch-down frame.
    raw_down: Point,
    /// The walk's raw samples, TAPS per target in TARGETS order.
    samp: [(i16, i16); N_SAMP],
    /// The fit (M_RESULT); None: the points gave none.
    fit: Option<touchcal::Fit>,
}

/// The touch stayed within HOLD_SLOP of its down point, one finger.
const FL_STILL: u8 = 1;
/// The hint says "Keep holding".
const FL_HINT: u8 = 2;
/// `tap` is valid.
const FL_TAP: u8 = 4;
/// Lines to repaint: the raw line, the counters.
const FL_RAW: u8 = 8;
const FL_CNT: u8 = 16;

/// `TT0` as a region C template (`.rodata.cold_tpl`, link-native.x):
/// its readers (`tt_open`) are region C code; a const copied there was an
/// anonymous Tt image in region B.
#[repr(transparent)]
struct TtTpl(Tt);
// SAFETY: read-only plain data.
unsafe impl Sync for TtTpl {}
#[cfg_attr(target_os = "none", link_section = ".rodata.cold_tpl")]
static TT0_C: TtTpl = TtTpl(TT0);

const TT0: Tt = Tt {
    cur: 0,
    frames: 0,
    stale: 0,
    bad: 0,
    down_at: 0,
    down_p: Point { x: 0, y: 0 },
    raw: Point { x: 0, y: 0 },
    raw_n: 0,
    max_pts: 0,
    pts: 0,
    fl: 0,
    tap: Point { x: 0, y: 0 },
    text_at: 0,
    keys_lit: 0,
    key_x: [0; 4],
    mode: M_TEST,
    ci: 0,
    ctap: 0,
    raw_down: Point { x: 0, y: 0 },
    samp: [(0, 0); N_SAMP],
    fit: None,
};

// Backup SRAM (.bkp_bss, link-native.x; CPU only, zeroed at reset;
// docs/native-boot.md "Extra RAM areas").
#[cfg_attr(target_os = "none", link_section = ".bkp_bss")]
static mut TT: Tt = TT0;

fn st() -> &'static mut Tt {
    // SAFETY: single-threaded (no ISR touches it), no reference is held
    // across calls.
    unsafe { &mut *core::ptr::addr_of_mut!(TT) }
}

impl App {
    /// Why the touch test cannot run now: the AD2 stream or a logger session
    /// (the page is region C code, which must not run then). Region B
    /// (inlined): App::tick checks it every pass while the page is shown.
    #[inline(always)]
    pub(super) fn tt_conflict(&self) -> Option<&'static str> {
        if crate::hy3131::streaming() || self.logger_busy || crate::fastlog::active() {
            Some("Not while logging")
        } else {
            None
        }
    }

    /// Open the page (false: refused, a toast says why).
    #[inline(never)]
    pub(super) fn tt_open(&mut self, clk: &mut Clock) -> bool {
        if let Some(t) = self.tt_conflict() {
            self.toast(clk.now_ms(), t, false);
            return false;
        }
        // SAFETY: plain data, no Drop.
        unsafe { core::ptr::copy_nonoverlapping(&TT0_C.0, st(), 1) };
        st().cur = touch::log_count();
        self.show(View::TouchTest, clk);
        true
    }

    /// Leave the page for its parent, Settings 3/3.
    #[inline(never)]
    pub(super) fn tt_close(&mut self, clk: &mut Clock) {
        self.set_page = 2;
        self.show(View::Settings, clk);
    }

    /// The whole page (draw_static): targets, keys, legend, the text lines
    /// from the state (the dots are gone).
    #[inline(never)]
    pub(super) fn tt_draw(&mut self) {
        let s = st();
        if s.mode == M_WALK {
            tt::cal_page();
            tt::cal_target(s.ci as usize, true);
            tt::cal_step(s.ci as usize, s.ctap, TAPS);
            return;
        }
        if s.mode == M_RESULT {
            tc_result(s);
            return;
        }
        tt::page();
        tc_status();
        for k in 0..4u8 {
            let x = s.key_x[k as usize];
            if x != 0 {
                tt::key(k, s.keys_lit & (1 << k) != 0, Some(x));
            }
        }
        s.fl |= FL_RAW | FL_CNT;
        s.fl &= !FL_HINT;
        tt_text(s, true);
    }

    /// One pass while the page is shown and no session runs (main.rs, after
    /// the touch poll): the new log entries (raw frames, debounced states),
    /// the hold-to-exit timer and Clear. `draw`: the panel may be drawn
    /// (App::tick's `can_draw`).
    #[inline(never)]
    pub fn tt_poll(&mut self, clk: &mut Clock, draw: bool) {
        let now = clk.now_ms() as u32;
        let draw = draw && self.overlay.is_none() && !self.redraw;
        let s = st();
        let n = touch::log_count();
        // DBG:TOUCH 0 cleared the log; or the page fell a whole ring behind.
        if n < s.cur || n - s.cur > touch::LOG_N as u32 {
            s.cur = n.saturating_sub(touch::LOG_N as u32);
        }
        while s.cur < n {
            let e = touch::log_entry(s.cur);
            s.cur += 1;
            let Some((kind, a, b, c, p)) = e else { continue };
            if kind == b'E' {
                // A debounced state: a points, b keys, p the first point.
                self.tt_state(s, clk, a, p, draw);
                continue;
            }
            if kind != b'F' {
                continue;
            }
            s.frames = s.frames.wrapping_add(1);
            s.fl |= FL_CNT;
            if a & 0x80 != 0 {
                if c == 0xAB {
                    s.stale = s.stale.wrapping_add(1);
                } else {
                    s.bad = s.bad.wrapping_add(1);
                }
            } else if b & 0x80 != 0 {
                // A key report: held while the status's low nibble is set
                // (0x83 held, 0x80 released; touch.rs `decode`). Another
                // key that is still lit was let go.
                let k = b & 3;
                let on = c & 0x0F != 0;
                s.key_x[k as usize] = p.x;
                tt_keys_off(s, !(1 << k), draw && s.mode == M_TEST);
                if on {
                    s.keys_lit |= 1 << k;
                } else {
                    s.keys_lit &= !(1 << k);
                }
                // (The key boxes are on the test page only.)
                if draw && s.mode == M_TEST {
                    tt::key(k, on, Some(p.x));
                }
            } else {
                // A screen frame: no key is held (the emulator's touch
                // model ends a key with a plain release frame).
                tt_keys_off(s, 0xFF, draw && s.mode == M_TEST);
                if b > 0 {
                    // A frame at the last frame's point is not drawn again:
                    // it would cover the debounced dot of a resting finger.
                    if draw && s.mode != M_RESULT && p != s.raw && p.x < crate::lcd::W && p.y < crate::lcd::H {
                        tt::dot(p, tt::DOT_RAW, tt::C_RAW);
                    }
                    s.raw = p;
                    s.raw_n = b;
                    s.max_pts = s.max_pts.max(b);
                    s.fl |= FL_RAW;
                }
            }
        }
        // Hold to exit (the test page only).
        let held = if s.mode == M_TEST && s.down_at != 0 && s.fl & FL_STILL != 0 { now.wrapping_sub(s.down_at) } else { 0 };
        if held >= HOLD_EXIT_MS {
            s.down_at = 0;
            self.tt_close(clk);
            return;
        }
        let hint = held >= HOLD_HINT_MS;
        if draw && hint != (s.fl & FL_HINT != 0) {
            s.fl ^= FL_HINT;
            tt::hint(hint);
        }
        if draw && s.mode == M_TEST && now.wrapping_sub(s.text_at) >= TEXT_MS {
            s.text_at = now;
            tt_text(s, false);
        }
    }

    /// A debounced state change (`points`, the first point `p`): the
    /// debounced dot, at a touch-down the cross and the Tap lines; at a
    /// release a short still touch on Clear clears the dots.
    #[inline(never)]
    fn tt_state(&mut self, s: &mut Tt, clk: &mut Clock, points: u8, p: Point, draw: bool) {
        let now = clk.now_ms() as u32;
        let dots = draw && s.mode != M_RESULT;
        if points > 0 {
            if dots {
                tt::dot(p, tt::DOT_DEB, tt::C_DEB);
            }
            if s.pts == 0 {
                // Touch-down: where a tap lands; the raw frame that made
                // it was the last `F` entry.
                s.down_at = now.max(1);
                s.down_p = p;
                s.raw_down = s.raw;
                s.tap = p;
                s.fl |= FL_STILL | FL_TAP;
                if dots {
                    tt::cross(p);
                }
                if draw && s.mode == M_TEST {
                    tt_tap_lines(s);
                }
            } else {
                let (dx, dy) = (p.x as i32 - s.down_p.x as i32, p.y as i32 - s.down_p.y as i32);
                if points > 1 || dx.abs() > HOLD_SLOP || dy.abs() > HOLD_SLOP {
                    s.fl &= !FL_STILL;
                }
            }
        } else if s.pts > 0 {
            let held = now.wrapping_sub(s.down_at);
            let still = s.fl & FL_STILL != 0;
            let (x, y) = (s.down_p.x, s.down_p.y);
            s.down_at = 0;
            s.pts = points;
            match s.mode {
                M_WALK => self.tc_walk_tap(s, clk, still, draw),
                M_RESULT => {
                    if still {
                        self.tc_result_tap(s, clk, x, y);
                    }
                }
                _ if still && held < HOLD_HINT_MS && tt::clear_hit(x, y) => self.redraw = true,
                _ if still && held < HOLD_HINT_MS && tt::calib_hit(x, y) => {
                    s.mode = M_WALK;
                    s.ci = 0;
                    s.ctap = 0;
                    self.redraw = true;
                }
                _ => {}
            }
            return;
        }
        s.pts = points;
    }

    /// A touch of the walk ended (`still`: it did not move): Cancel, Reset,
    /// or a sample for the current target.
    #[inline(never)]
    fn tc_walk_tap(&mut self, s: &mut Tt, clk: &mut Clock, still: bool, draw: bool) {
        if still {
            match tt::btn_hit(s.down_p.x, s.down_p.y) {
                Some(0) => {
                    s.mode = M_TEST;
                    self.redraw = true;
                    return;
                }
                Some(_) => {
                    touch::set_cal(touchcal::IDENTITY);
                    s.mode = M_TEST;
                    self.toast(clk.now_ms(), "Touch calibration reset", true);
                    self.redraw = true;
                    return;
                }
                None => {}
            }
        }
        let (tx, ty) = tt::TARGETS[s.ci as usize];
        let r = (s.raw_down.x as i32, s.raw_down.y as i32);
        let far = |a: (i32, i32), b: (i32, i32), lim: i32| (a.0 - b.0).abs() > lim || (a.1 - b.1).abs() > lim;
        let k = s.ci as usize * TAPS as usize + s.ctap as usize;
        let msg = if !still {
            Some("Moved: tap again")
        } else if far(r, (tx as i32, ty as i32), ACCEPT_PX) {
            Some("Too far from the cross")
        } else if s.ctap > 0 && far(r, (s.samp[k - 1].0 as i32, s.samp[k - 1].1 as i32), SPREAD_PX) {
            // Two taps of one target disagree: both again.
            s.ctap = 0;
            Some("Taps differ: tap again")
        } else {
            s.samp[k] = (r.0 as i16, r.1 as i16);
            s.ctap += 1;
            None
        };
        let next = s.ctap == TAPS;
        if next {
            s.ctap = 0;
            s.ci += 1;
            if s.ci as usize == tt::TARGETS.len() {
                self.tc_fit(s);
                return;
            }
        }
        if !draw {
            self.redraw = true;
            return;
        }
        if next {
            tt::cal_target(s.ci as usize - 1, false);
            tt::cal_target(s.ci as usize, true);
        }
        tt::cal_step(s.ci as usize, s.ctap, TAPS);
        tt::cal_msg(msg.unwrap_or(""), true);
    }

    /// All samples taken: fit, show the result.
    #[inline(never)]
    fn tc_fit(&mut self, s: &mut Tt) {
        let mut tgt = [(0i16, 0i16); N_SAMP];
        for (i, t) in tgt.iter_mut().enumerate() {
            let (x, y) = tt::TARGETS[i / TAPS as usize];
            *t = (x as i16, y as i16);
        }
        s.fit = touchcal::fit(&s.samp, &tgt, &tt::TARGETS);
        s.mode = M_RESULT;
        self.redraw = true;
    }

    /// A still tap on the result screen: Save (or Try again), Cancel.
    #[inline(never)]
    fn tc_result_tap(&mut self, s: &mut Tt, clk: &mut Clock, x: u16, y: u16) {
        match tt::btn_hit(x, y) {
            Some(0) => match s.fit {
                Some(f) if f.bad.is_none() => {
                    touch::set_cal(f.cal);
                    s.mode = M_TEST;
                    self.toast(clk.now_ms(), "Touch calibration saved", true);
                }
                _ => {
                    s.mode = M_WALK;
                    s.ci = 0;
                    s.ctap = 0;
                }
            },
            Some(_) => s.mode = M_TEST,
            None => return,
        }
        self.redraw = true;
    }

    /// On this page every gesture is swallowed; a touch-down is captured
    /// (true), so its release is an `Up`, never a `Tap` (it may end on
    /// Settings after a hold-to-exit).
    #[inline(never)]
    pub(super) fn tt_gesture(&mut self, g: Gesture) -> bool {
        matches!(g, Gesture::Down(_))
    }

    /// DBG:TOUCH:TEST ON / OFF / ?.
    #[inline(never)]
    pub(super) fn tt_scpi(&mut self, clk: &mut Clock, on: Option<bool>, out: &mut crate::scpi::Scpi) -> bool {
        match on {
            None => {
                out.put_bytes(if self.view == View::TouchTest { b"1" } else { b"0" });
                true
            }
            Some(true) if self.view == View::TouchTest => false,
            Some(true) => {
                if self.tt_conflict().is_some() {
                    out.set_err("Settings conflict");
                } else {
                    self.tt_open(clk);
                }
                false
            }
            Some(false) => {
                if self.view == View::TouchTest {
                    self.tt_close(clk);
                }
                false
            }
        }
    }
}

/// Draw the lit keys in `mask` not lit (their last X kept).
fn tt_keys_off(s: &mut Tt, mask: u8, draw: bool) {
    let off = s.keys_lit & mask;
    if off == 0 {
        return;
    }
    s.keys_lit &= !off;
    if draw {
        for k in 0..4u8 {
            if off & (1 << k) != 0 {
                tt::key(k, false, Some(s.key_x[k as usize]));
            }
        }
    }
}

/// The text lines: the raw line and the counters when changed (`all`: every
/// line, after a full page draw).
fn tt_text(s: &mut Tt, all: bool) {
    if all || s.fl & FL_RAW != 0 {
        let mut t = tt::Txt::new();
        if s.raw_n == 0 {
            t.s("Raw: touch a target");
        } else {
            t.s("Raw ").p(s.raw).s("  ").u(s.raw_n as u32).s(if s.raw_n == 1 { " pt" } else { " pts" });
        }
        tt::line(0, t.as_str());
    }
    if all {
        tt_tap_lines(s);
    }
    if all || s.fl & FL_CNT != 0 {
        let mut t = tt::Txt::new();
        t.s("Frames ").u(s.frames).s("  max ").u(s.max_pts as u32).s(" pts");
        tt::line(3, t.as_str());
        let mut t = tt::Txt::new();
        t.s("Stale ").u(s.stale).s("  Bad ").u(s.bad);
        tt::line(4, t.as_str());
    }
    s.fl &= !(FL_RAW | FL_CNT);
}

/// "Tap x,y" and its offset to the nearest target (+ = right of / below it).
fn tt_tap_lines(s: &Tt) {
    let mut t = tt::Txt::new();
    let mut o = tt::Txt::new();
    if s.fl & FL_TAP == 0 {
        t.s("Tap -");
    } else {
        let g = tt::nearest(s.tap);
        t.s("Tap ").p(s.tap);
        o.s("Off ").i(s.tap.x as i32 - g.x as i32).s(",").i(s.tap.y as i32 - g.y as i32).s(" to ").p(g);
    }
    tt::line(1, t.as_str());
    tt::line(2, o.as_str());
}

/// The calibration in use, in the page's F14 lines ("none" for the
/// identity).
fn tc_status() {
    let c = touch::cal();
    tt::cal_status(if c == touchcal::IDENTITY { None } else { Some(touchcal::to_f32(&c)) });
}

/// The result screen: residuals, coefficients, verdict, and per sample the
/// raw dot and the dot the fit moves it to.
fn tc_result(s: &Tt) {
    let Some(f) = s.fit else {
        tt::result_page(0, 0, 0, &touchcal::to_f32(&touchcal::IDENTITY), Some(touchcal::Bad::Points.text()));
        return;
    };
    tt::result_page(f.mean, f.max, f.before, &touchcal::to_f32(&f.cal), f.bad.map(|b| b.text()));
    for &(x, y) in s.samp.iter() {
        let clip = |v: i32, m: u16| v.clamp(0, m as i32 - 1) as u16;
        tt::dot(Point { x: clip(x as i32, crate::lcd::W), y: clip(y as i32, crate::lcd::H) }, tt::DOT_RAW, tt::C_RAW);
        let (cx, cy) = touchcal::apply16(&f.cal, x as i32, y as i32);
        tt::dot(Point { x: clip((cx + 8) >> 4, crate::lcd::W), y: clip((cy + 8) >> 4, crate::lcd::H) }, tt::DOT_DEB, tt::C_DEB);
    }
}

/// The settings record's calibration (App::restore): used when it passes
/// the checks, else none.
pub(super) fn tc_restore(c: touchcal::Cal) {
    touch::set_cal(if touchcal::check(&c, &tt::TARGETS).is_ok() { c } else { touchcal::IDENTITY });
}

/// DBG:TOUCH:CAL? (op 0: a,b,c,d,e,f), DBG:TOUCH:CAL RESET (1),
/// DBG:TOUCH:CAL a,b,c,d,e,f (2: the values touchcal::scpi_arg parsed;
/// refused outside the checks). Saved with the settings, as the page's Save.
#[inline(never)]
pub(super) fn tc_scpi(op: u8, out: &mut crate::scpi::Scpi) -> bool {
    match op {
        0 => {
            for (i, v) in touchcal::to_f32(&touch::cal()).iter().enumerate() {
                if i > 0 {
                    out.put_bytes(b",");
                }
                out.put_f32(*v);
            }
            true
        }
        1 => {
            touch::set_cal(touchcal::IDENTITY);
            false
        }
        _ => {
            match touchcal::scpi_take() {
                Some(c) if touchcal::check(&c, &tt::TARGETS).is_ok() => touch::set_cal(c),
                _ => out.set_err("Data out of range"),
            }
            false
        }
    }
}
