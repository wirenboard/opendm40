//! Main-loop profiler on the DWT cycle counter (DBG:PROF?, DBG:PROF:RESET).
//!
//! Named sections (`enter` / `exit`, or `lap` for back-to-back sections in
//! main.rs): calls, last, max since the reset, max of the last full second,
//! EMA (alpha 1/16) and total time (CPU share). Plus the main-loop period
//! (max / mean / EMA / histogram), frames drawn per second (`drawn`), and
//! two latencies from a touch event (`touch`: a touch report that produced a
//! gesture, or a DBG:TAP / DBG:DRAG / DBG:KEY): to the first LCD window
//! opened after it (`pixel`) and to the first scope frame drawn after it
//! (`drawn`). Only the oldest unserved event is timed (a burst of drag
//! reports counts once, from its first report); an event not served within
//! LAT_DROP_MS is dropped (no frame on that page; counted as "unserved").
//! touch->pixel needs the optional hook in lcd::set_window_cmd (n = 0
//! without it); it fires on any drawing, related to the touch or not.
//!
//! Hooks outside main.rs are optional: a section that was never entered
//! prints n = 0. opendm40 is single-threaded with no ISRs, so plain statics.
//! Cost: a CYCCNT read and ~20 cycles of bookkeeping per call; one integer
//! division (cycles -> us) per exit.
//!
//! Times are in us (200 cycles each at 200 MHz); a single interval must stay
//! under the 21 s CYCCNT wrap.
//!
//! Built only with the Cargo feature `dbg-prof`; without it the hooks are
//! empty and DBG:PROF? is refused (`cyc` stays: other modules time with it).

// The app-level ids and `drawn` / `pixel` are for the optional hooks in
// app.rs / lcd.rs (DBG:PROF? prints n = 0 for a section never entered).
#![allow(dead_code)]

use crate::mmio::r32;

const DWT_CYCCNT: u32 = 0xE000_1004;
/// The profiler is built (Cargo feature `dbg-prof`, DBG:PROF?). Without it
/// every hook below returns at once, so `P` is left out of the image (1 KB
/// of RAM) and only `cyc` is live.
const ON: bool = cfg!(feature = "dbg-prof");
const CYC_PER_US: u32 = crate::clock::HCLK_HZ / 1_000_000;
/// An unserved touch event older than this is dropped, not timed.
const LAT_DROP_MS: u64 = 3000;

// Main-loop sections (main.rs).
/// dev.poll + MSC + data logger + bt.pump.
pub const USB: usize = 0;
/// One SCPI line handled (USBTMC / BT).
pub const SCPI: usize = 1;
/// csvlog::poll (CSV recording slice).
pub const CSV: usize = 2;
/// App::tick (power, rails, status block, chart, toasts, gallery, screenshot).
pub const TICK: usize = 3;
/// Touch report read (I2C2) + gesture tracker.
pub const TOUCH: usize = 4;
/// App::gesture for a touch gesture (drag handler chain incl. scope_set).
pub const GEST: usize = 5;
/// App::scope_poll (acquisition, render, draw, measurements).
pub const SCOPE: usize = 6;
/// bt.poll + bt.serve.
pub const BT: usize = 7;
/// dmm.poll + App::reading (meter / strip drawing).
pub const DMM: usize = 8;
// App sections (hooks in app.rs; nested inside SCOPE / TICK).
/// Scope::poll (DMA record, trigger search, mask / linearise).
pub const ACQ: usize = 9;
/// ScopeView::frame (per-column min/max + column drawing).
pub const FRAME: usize = 10;
/// scope_meas::compute + cursor values.
pub const MEAS: usize = 11;
/// ui::scope_measurements (bottom bar text).
pub const MTXT: usize = 12;
/// Once-per-second scope status block (scope_status + generator chip).
pub const STAT: usize = 13;
/// Top-bar labels after a settings change (ScopeView::labels).
pub const LABEL: usize = 14;
/// Scope graph paint in a pass without a new frame (App::sp_tick: the
/// budgeted trace flush continuing, badges).
pub const PAINT: usize = 15;
/// Spare id for ad-hoc experiments. The protocol decoder (app_decode.rs)
/// uses it for one pass's slice of a record decode.
pub const X2: usize = 16;
// DMM page sections (hooks in app.rs; nested inside DMM / TICK).
/// DmmView::reading (main digits, +1 digit, aux rows, labels).
pub const DREAD: usize = 17;
/// ui::chart (trend columns, axis labels, side panel, chips).
pub const DCHART: usize = 18;
/// Scope FFT (UF-11, hooks in scope_page.rs): one load (decimation +
/// window, on the frame's pass) or one `step` slice, and the spectrum ->
/// columns render.
pub const FFT: usize = 19;
/// DMM page fast bargraph (UF-30, bargraph.rs update in App::reading).
pub const DBAR: usize = 20;
/// The protocol decoder: one record's whole decode (the sum of its X2
/// slices, `add`).
pub const DJOB: usize = 21;
pub const N: usize = 22;

const NAMES: [&str; N] = [
    "usb", "scpi", "csv", "tick", "touch", "gesture", "scope", "bt", "dmm", "s.acq", "s.frame", "s.meas", "s.mtxt",
    "s.status", "s.labels", "s.paint", "x2", "d.read", "d.chart", "s.fft", "d.bar", "dec.job",
];

#[derive(Clone, Copy)]
struct Stat {
    n: u32,
    last: u32,
    max: u32,
    /// Max of the current / the last full 1 s window.
    win: u32,
    win_prev: u32,
    /// EMA x16 (alpha 1/16).
    ema16: u32,
    total: u64,
}

impl Stat {
    const ZERO: Stat = Stat { n: 0, last: 0, max: 0, win: 0, win_prev: 0, ema16: 0, total: 0 };

    fn add(&mut self, us: u32) {
        self.ema16 = if self.n == 0 { us.saturating_mul(16) } else { self.ema16 - (self.ema16 >> 4) + us };
        self.n = self.n.wrapping_add(1);
        self.last = us;
        self.max = self.max.max(us);
        self.win = self.win.max(us);
        self.total += us as u64;
    }

    fn roll(&mut self) {
        self.win_prev = self.win;
        self.win = 0;
    }
}

/// Loop-period histogram bucket upper bounds, us (the last bucket is open).
const HIST: [u32; 7] = [500, 1000, 2000, 5000, 10_000, 20_000, 50_000];

struct Prof {
    t0: [u32; N],
    sec: [Stat; N],
    lap_t: u32,
    loop_t: u32,
    looped: bool,
    period: Stat,
    hist: [u32; HIST.len() + 1],
    frames: u32,
    /// Oldest unserved touch event: (cycles, ms), for the pixel / frame
    /// latencies separately.
    pend_pix: Option<(u32, u64)>,
    pend_frm: Option<(u32, u64)>,
    lat_pix: Stat,
    lat_frm: Stat,
    events: u32,
    dropped: u32,
    reset_ms: u64,
    now_ms: u64,
    win_ms: u64,
}

// A `dbg-prof` build only (see ON). In the backup SRAM (.bkp_bss,
// link-native.x; zeroed at reset, every field's initial value is zero
// bytes; docs/native-boot.md "Extra RAM areas"): TCM has no 1 KB to spare
// next to the stack, SRAM none on the scope chain.
#[cfg_attr(target_os = "none", link_section = ".bkp_bss")]
static mut P: Prof = Prof {
    t0: [0; N],
    sec: [Stat::ZERO; N],
    lap_t: 0,
    loop_t: 0,
    looped: false,
    period: Stat::ZERO,
    hist: [0; HIST.len() + 1],
    frames: 0,
    pend_pix: None,
    pend_frm: None,
    lat_pix: Stat::ZERO,
    lat_frm: Stat::ZERO,
    events: 0,
    dropped: 0,
    reset_ms: 0,
    now_ms: 0,
    win_ms: 0,
};

#[inline(always)]
fn p() -> &'static mut Prof {
    unsafe { &mut *core::ptr::addr_of_mut!(P) }
}

#[inline(always)]
pub fn cyc() -> u32 {
    unsafe { r32(DWT_CYCCNT) }
}

#[inline(always)]
fn us(dc: u32) -> u32 {
    dc / CYC_PER_US
}

/// Top of a main-loop pass: the loop period, the 1 s windows, the latency
/// timeouts; starts the `lap` chain.
pub fn pass(now_ms: u64) {
    if !ON {
        return;
    }
    let p = p();
    let t = cyc();
    if p.looped {
        let d = us(t.wrapping_sub(p.loop_t));
        p.period.add(d);
        let k = HIST.iter().position(|&h| d < h).unwrap_or(HIST.len());
        p.hist[k] = p.hist[k].wrapping_add(1);
    } else {
        p.looped = true;
        p.reset_ms = now_ms;
        p.win_ms = now_ms + 1000;
    }
    p.loop_t = t;
    p.lap_t = t;
    p.now_ms = now_ms;
    if now_ms >= p.win_ms {
        p.win_ms = now_ms + 1000;
        p.period.roll();
        for s in p.sec.iter_mut() {
            s.roll();
        }
    }
    if p.pend_pix.is_some_and(|(_, ms)| now_ms >= ms + LAT_DROP_MS) {
        p.pend_pix = None; // no pixel hook, or nothing drawn
    }
    if p.pend_frm.is_some_and(|(_, ms)| now_ms >= ms + LAT_DROP_MS) {
        p.pend_frm = None;
        p.dropped = p.dropped.wrapping_add(1);
    }
}

/// Section `id` starts now.
#[inline(always)]
pub fn enter(id: usize) {
    if !ON {
        return;
    }
    p().t0[id] = cyc();
}

/// Section `id` (started by `enter`) ends now.
#[inline(always)]
pub fn exit(id: usize) {
    if !ON {
        return;
    }
    let p = p();
    let d = cyc().wrapping_sub(p.t0[id]);
    p.sec[id].add(us(d));
}

/// A time measured elsewhere (cycles) counted as one call of section `id`.
pub fn add(id: usize, cycles: u32) {
    if !ON {
        return;
    }
    p().sec[id].add(us(cycles));
}

/// Back-to-back sections: section `id` ran from the previous `lap` (or the
/// top of the pass) until now.
#[inline(always)]
pub fn lap(id: usize) {
    if !ON {
        return;
    }
    let p = p();
    let t = cyc();
    p.sec[id].add(us(t.wrapping_sub(p.lap_t)));
    p.lap_t = t;
}

/// Restart the `lap` chain without charging the time to any section.
#[inline(always)]
pub fn lap_skip() {
    if !ON {
        return;
    }
    p().lap_t = cyc();
}

/// A touch event (or a synthetic DBG input) was dispatched; `t` = the cycle
/// count when its report was read.
pub fn touch(t: u32) {
    if !ON {
        return;
    }
    let p = p();
    p.events = p.events.wrapping_add(1);
    let ms = p.now_ms;
    if p.pend_pix.is_none() {
        p.pend_pix = Some((t, ms));
    }
    if p.pend_frm.is_none() {
        p.pend_frm = Some((t, ms));
    }
}

/// Something is about to be drawn (an LCD window opened).
#[inline(always)]
pub fn pixel() {
    if !ON {
        return;
    }
    let p = p();
    if let Some((t, _)) = p.pend_pix.take() {
        p.lat_pix.add(us(cyc().wrapping_sub(t)));
    }
}

/// A scope frame was drawn (ScopeView::frame or a drag re-plot).
pub fn drawn() {
    if !ON {
        return;
    }
    let p = p();
    p.frames = p.frames.wrapping_add(1);
    if let Some((t, _)) = p.pend_frm.take() {
        p.lat_frm.add(us(cyc().wrapping_sub(t)));
    }
}

/// DBG:PROF:RESET.
pub fn reset() {
    if !ON {
        return;
    }
    let p = p();
    let now = p.now_ms;
    p.sec = [Stat::ZERO; N];
    p.period = Stat::ZERO;
    p.hist = [0; HIST.len() + 1];
    p.frames = 0;
    p.pend_pix = None;
    p.pend_frm = None;
    p.lat_pix = Stat::ZERO;
    p.lat_frm = Stat::ZERO;
    p.events = 0;
    p.dropped = 0;
    p.looped = false;
    p.now_ms = now;
}

/// Text sink over a byte buffer (truncates at the end).
struct W<'a> {
    b: &'a mut [u8],
    n: usize,
}

impl W<'_> {
    fn s(&mut self, s: &str) {
        for &c in s.as_bytes() {
            if self.n < self.b.len() {
                self.b[self.n] = c;
                self.n += 1;
            }
        }
    }

    /// Decimal, right-aligned in `width` (0 = no padding).
    fn u(&mut self, v: u64, width: usize) {
        let mut t = [0u8; 20];
        let mut i = t.len();
        let mut v = v;
        loop {
            i -= 1;
            t[i] = b'0' + (v % 10) as u8;
            v /= 10;
            if v == 0 {
                break;
            }
        }
        for _ in (t.len() - i)..width {
            self.s(" ");
        }
        self.s(core::str::from_utf8(&t[i..]).unwrap_or(""));
    }

    /// v / 10^dec with `dec` decimals, right-aligned in `width`.
    fn fix(&mut self, v: u64, dec: u32, width: usize) {
        let d = 10u64.pow(dec);
        let int_w = width.saturating_sub(dec as usize + 1);
        self.u(v / d, int_w);
        self.s(".");
        let mut f = v % d;
        let mut k = d / 10;
        while k > 0 {
            self.u(f / k, 0);
            f %= k;
            k /= 10;
        }
    }

    fn left(&mut self, s: &str, width: usize) {
        self.s(s);
        for _ in s.len()..width {
            self.s(" ");
        }
    }

    fn lat(&mut self, name: &str, l: &Stat) {
        self.s(name);
        self.s(" ms: n ");
        self.u(l.n as u64, 0);
        if l.n > 0 {
            self.s(" last ");
            self.fix(l.last as u64 / 100, 1, 0);
            self.s(" ema ");
            self.fix((l.ema16 / 16) as u64 / 100, 1, 0);
            self.s(" avg ");
            self.fix(l.total / l.n as u64 / 100, 1, 0);
            self.s(" max ");
            self.fix(l.max as u64 / 100, 1, 0);
        }
        self.s("\n");
    }
}

/// DBG:PROF? text table into `buf` (ends with '\n'); returns its length.
#[inline(never)] // region B (link-native.x): asked during fast sessions; not inlined into the hot caller
#[cfg(feature = "dbg-prof")]
pub fn report(buf: &mut [u8]) -> usize {
    let p = p();
    let mut w = W { b: buf, n: 0 };
    let el_ms = p.now_ms.saturating_sub(p.reset_ms).max(1);
    let pn = p.period.n as u64;
    w.s("PROF ");
    w.fix(el_ms / 10, 2, 0);
    w.s(" s; passes ");
    w.u(pn, 0);
    w.s(" (");
    w.u(pn * 1000 / el_ms, 0);
    w.s("/s); frames ");
    w.u(p.frames as u64, 0);
    w.s(" (");
    w.fix(p.frames as u64 * 10_000 / el_ms, 1, 0);
    w.s(" fps); touch events ");
    w.u(p.events as u64, 0);
    w.s(", unserved ");
    w.u(p.dropped as u64, 0);
    w.s("\nloop us: avg ");
    w.u(if pn > 0 { p.period.total / pn } else { 0 }, 0);
    w.s(" ema ");
    w.u((p.period.ema16 / 16) as u64, 0);
    w.s(" max ");
    w.u(p.period.max as u64, 0);
    w.s(" max1s ");
    w.u(p.period.win_prev.max(p.period.win) as u64, 0);
    w.s("\nloop hist ms:");
    for (k, &c) in p.hist.iter().enumerate() {
        match HIST.get(k) {
            Some(&h) => {
                w.s(" <");
                if h < 1000 {
                    w.fix(h as u64 / 100, 1, 0);
                } else {
                    w.u(h as u64 / 1000, 0);
                }
            }
            None => {
                w.s(" >=");
                w.u(HIST[HIST.len() - 1] as u64 / 1000, 0);
            }
        }
        w.s(":");
        w.u(c as u64, 0);
    }
    w.s("\n");
    w.lat("touch->pixel", &p.lat_pix);
    w.lat("touch->frame", &p.lat_frm);
    w.s("section       calls  ema_us  avg_us  max_us max1s_us  last_us  tot_ms  cpu%\n");
    for (i, s) in p.sec.iter().enumerate() {
        w.left(NAMES[i], 10);
        w.u(s.n as u64, 8);
        w.u((s.ema16 / 16) as u64, 8);
        w.u(if s.n > 0 { s.total / s.n as u64 } else { 0 }, 8);
        w.u(s.max as u64, 8);
        w.u(s.win_prev.max(s.win) as u64, 9);
        w.u(s.last as u64, 9);
        w.u(s.total / 1000, 8);
        // Permille of the elapsed time, shown as a percentage.
        w.fix(s.total / el_ms, 1, 6);
        w.s("\n");
    }
    w.s("(s.* nested in scope/tick, d.* in dmm/tick; max1s = max of the current + last 1 s window)\n");
    w.n
}
