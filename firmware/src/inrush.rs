//! Inrush current capture (TODO "Inrush current", as the Fluke 376 / 323
//! clamps; docs/inrush-current.md): the pure part - the step detector, the
//! pre-trigger ring, the results and the record stream of the saved file.
//! No register access: host-tested by tools/meter-test (`tests` below);
//! the app glue is src/app_inrush.rs, the page src/ui_inrush.rs.
//!
//! Data path: the HY3131 AD2 stream (the logger.rs stream ISR, a wave-view
//! style session, `Logger::start_wave`) in the AC A row of the 10 A range:
//! OP1 x1 across the 10 mohm shunt, DC-coupled, +-125 A at full scale
//! (docs/dmm-input-scope-feasibility.md 1.3), at RATE_HZ. The main loop
//! drains the ring (`logger::wave::drain`) every pass and feeds `push`.
//! Each sample is kept as a 16-bit code `(raw + 4) >> 3` (3.8 mA per code;
//! the AD2 noise is ~32 raw codes = 4 of these, so nothing is lost) in the
//! scope's record buffer (scope::BUF, 26 624 slots, lent while the inrush
//! page is open: `Scope::lend_buf`), used as a ring until the capture is
//! complete, then rotated so the trace runs PRE samples before the trigger
//! to POST samples after it.
//!
//! Trigger rule (`Cap::push`): the mean of |I| over the last SHORT samples
//! (~1 ms) exceeds the largest such mean of the previous ~100 ms (NBLK
//! blocks of BLK samples, the idle "envelope") by the step threshold. With
//! nothing flowing that is an absolute threshold; with a load already
//! running (AC or DC) only a rise by the step triggers: the envelope holds
//! the idle current's own peaks, so a 2 A rms motor already running does
//! not trigger a 1 A step (an AC idle current's 1 ms means reach its peak,
//! 1.41 x rms, and that is in the envelope). |I| makes either polarity
//! trigger (a reversed DC lead). Armed once PRE + SHORT samples of history
//! are in (~0.2 s after the arm, or after a hole in the stream). The trigger
//! point (t = 0) is the first sample of the ~1 ms window that crossed: at
//! most ~1 ms before the step reached the threshold. A rise slower than the
//! step per ~100 ms follows the envelope and never triggers (documented).
//!
//! Results (`Cap::results`, the linear trace): Irms over the first WIN100
//! samples from t = 0 (Fluke's 100 ms window; sqrt of the mean square of
//! the DC-coupled samples, so AC and DC loads alike), the peak (the sample
//! of largest |I| in the post-trigger part, signed), the idle rms (the
//! pre-trigger part), and OL when a sample reached the AD2 rail.

/// AD2 stream rate (a logger ladder rung): 960 samples in 100 ms.
pub const RATE_HZ: u32 = 9600;
/// Pre-trigger part of the trace (200 ms) and the post-trigger part (1.8 s):
/// 2.0 s in all, 100 ms per chart division.
pub const PRE: usize = 1920;
pub const POST: usize = 17_280;
pub const LEN: usize = PRE + POST;
/// RMS window from the trigger (100 ms).
pub const WIN100: usize = 960;
/// Detector window (~1.04 ms) and envelope blocks (10 ms x 10).
pub const SHORT: usize = 10;
pub const BLK: usize = 96;
pub const NBLK: usize = 10;
/// Step thresholds of the page's stepper, A (the SCPI may set any value in
/// THR_MIN..=THR_MAX).
pub const THR_STEPS: [f32; 6] = [0.2, 0.5, 1.0, 2.0, 5.0, 10.0];
pub const THR_DEFAULT: f32 = 1.0;
pub const THR_MIN: f32 = 0.1;
pub const THR_MAX: f32 = 50.0;
/// Measured zero (`Cap::arm` with b > 0): the first ZERO_SKIP samples of
/// the stream are skipped, the next ZERO_N (100 ms) averaged; their mean is
/// the zero when it lies within ZERO_MAX_A of the zero given (the last one
/// measured, else the calibration's: app_inrush.rs) and their rms about it
/// is below ZERO_RMS_A (no load running), else the zero given stays. Device 185c066: the AC A 10 A row's AD2 mean
/// read +0.0717 A with the A jack open (rms about it 1.7 mA) and the cal
/// record has no LPF offset for the AC A rows (0x10C.., all 0): the page
/// read "Armed: 0.069 A now". A DC load below ZERO_MAX_A already flowing
/// at the arm is taken for the zero (docs/inrush-current.md 11).
pub const ZERO_SKIP: u32 = 96;
pub const ZERO_N: u32 = 960;
pub const ZERO_MAX_A: f32 = 0.2;
pub const ZERO_RMS_A: f32 = 0.05;

/// Code of a sample at the AD2 rail (262 143 >> 3 = 32 767, a few codes of
/// margin): OL.
pub const CODE_OL: i32 = 32_700;

/// The 16-bit code of a raw 19-bit AD2 sample (rounded, saturated).
#[inline(always)]
pub fn code_of(raw: i32) -> i16 {
    ((raw + 4) >> 3).clamp(-32768, 32767) as i16
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Phase {
    /// Not capturing (no stream).
    Off,
    /// Stream running, history shorter than PRE + SHORT samples.
    Arming,
    /// Waiting for the step.
    Armed,
    /// Triggered: taking the post-trigger part.
    Post,
    /// Complete: `trig` .. `trig + POST` are in the ring.
    Done,
}

/// The capture state machine (the ring itself is the caller's slice).
#[derive(Clone, Copy)]
pub struct Cap {
    pub phase: Phase,
    /// Zero-current code (16-bit codes, rounded) and the step threshold x
    /// SHORT, in codes.
    z: i32,
    step_sum: u32,
    /// The exact zero code: the one `arm` was given, or the measured one
    /// (`zok`).
    pub zf: f32,
    pub zok: bool,
    /// Measured zero: A per code (its limits), the samples taken so far
    /// (ZERO_SKIP + ZERO_N = done), the sums of (c - z) and its square
    /// (f32: exact for an idle input; a load fails the limits anyway).
    zb: f32,
    zn: u32,
    zs1: f32,
    zs2: f32,
    /// Samples taken since the arm (the ring slot of sample n is n % len).
    pub tot: u64,
    /// First sample of the usable history (the arm, or after a hole).
    hist0: u64,
    /// Sum of |c - z| over the last SHORT samples.
    sum: u32,
    /// The current envelope block's largest `sum`, its sample count; the
    /// last NBLK blocks' maxima, how many are in, the next slot; their max.
    blk_max: u32,
    blk_n: u32,
    env: [u32; NBLK],
    env_n: u8,
    env_i: u8,
    env_max: u32,
    /// Trigger sample index (`tot` units).
    pub trig: u64,
    /// Samples lost after the trigger (ring overflow / ISR misses) and the
    /// trace index of the first loss (the saved file's stamps skip there).
    pub lost: u32,
    pub hole_at: u32,
    /// Live level while armed: sum of squares of (c - z) over the current
    /// 100 ms block, its count, the last complete block's mean square.
    sq: u64,
    sq_n: u32,
    pub live_ms: u32,
    /// Records already handed to the file writer (`records`).
    rd: u32,
}

/// Results of a complete capture (SI from the caller's scale).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Res {
    /// Irms over WIN100 from the trigger, A.
    pub rms100: f32,
    /// The sample of largest |I| after the trigger, A (signed).
    pub peak: f32,
    /// rms of the pre-trigger part, A.
    pub idle: f32,
    /// A sample reached the AD2 rail.
    pub ol: bool,
    /// Samples lost after the trigger.
    pub lost: u32,
}

impl Cap {
    pub const OFF: Cap = Cap {
        phase: Phase::Off,
        z: 0,
        step_sum: 0,
        zf: 0.0,
        zok: false,
        zb: 0.0,
        zn: 0,
        zs1: 0.0,
        zs2: 0.0,
        tot: 0,
        hist0: 0,
        sum: 0,
        blk_max: 0,
        blk_n: 0,
        env: [0; NBLK],
        env_n: 0,
        env_i: 0,
        env_max: 0,
        trig: 0,
        lost: 0,
        hole_at: 0,
        sq: 0,
        sq_n: 0,
        live_ms: 0,
        rd: 0,
    };

    /// A new capture: `zf` = the calibration's zero code, `b` = A per code
    /// (> 0: the zero is measured first, ZERO_N; 0: `zf` as it is),
    /// `step_codes` = the step threshold in codes (>= 1). Region C
    /// (link-native.x): the page's and the SCPI's arm.
    #[inline(never)]
    pub fn arm(&mut self, zf: f32, b: f32, step_codes: u32) {
        *self = Cap::OFF;
        self.phase = Phase::Arming;
        self.zf = zf;
        self.z = round_i(zf);
        self.zb = b;
        self.zn = if b > 0.0 { 0 } else { ZERO_SKIP + ZERO_N };
        self.step_sum = step_codes.max(1).saturating_mul(SHORT as u32);
    }

    /// One sample of the zero's measurement (`arm`); at its end the zero
    /// moves to the mean when it passes ZERO_MAX_A / ZERO_RMS_A.
    #[inline(never)]
    fn zero_in(&mut self, c: i16) {
        self.zn += 1;
        if self.zn <= ZERO_SKIP {
            return;
        }
        let d = (c as i32 - self.z) as f32;
        self.zs1 += d;
        self.zs2 += d * d;
        if self.zn == ZERO_SKIP + ZERO_N {
            let m = self.zs1 * (1.0 / ZERO_N as f32);
            let var = self.zs2 * (1.0 / ZERO_N as f32) - m * m;
            let b = self.zb;
            if (m * b).abs() <= ZERO_MAX_A && var * (b * b) <= ZERO_RMS_A * ZERO_RMS_A {
                self.zf = self.z as f32 + m;
                self.z = round_i(self.zf);
                self.zok = true;
            }
        }
    }

    /// A new step threshold (codes) without losing the history.
    pub fn set_step(&mut self, step_codes: u32) {
        self.step_sum = step_codes.max(1).saturating_mul(SHORT as u32);
    }

    /// A gap in the stream of `n` samples (before the samples drained
    /// next): while waiting the history starts again; after the trigger it
    /// is counted (the trace closes the gap; the file's stamps show it).
    pub fn hole(&mut self, n: u32) {
        match self.phase {
            Phase::Arming | Phase::Armed => {
                // The detector afresh; a zero measured stays, one in
                // progress starts again.
                let (z, zf, zok, zb, st, tot, zn) = (self.z, self.zf, self.zok, self.zb, self.step_sum, self.tot, self.zn);
                *self = Cap::OFF;
                self.phase = Phase::Arming;
                (self.z, self.zf, self.zok, self.zb, self.step_sum, self.tot, self.hist0) = (z, zf, zok, zb, st, tot, tot);
                if zn >= ZERO_SKIP + ZERO_N {
                    self.zn = zn;
                }
            }
            Phase::Post => {
                if self.lost == 0 {
                    self.hole_at = (self.tot + PRE as u64 - self.trig) as u32;
                }
                self.lost = self.lost.saturating_add(n);
            }
            _ => {}
        }
    }

    /// One sample (code `c`) into the ring `buf` and the detector. Returns
    /// true when the capture just completed.
    #[inline(always)]
    pub fn push(&mut self, buf: &mut [u16], c: i16) -> bool {
        if matches!(self.phase, Phase::Off | Phase::Done) {
            return false;
        }
        if self.zn < ZERO_SKIP + ZERO_N {
            // (the history starts after the zero)
            self.zero_in(c);
            return false;
        }
        let cap = buf.len() as u64;
        let a = (c as i32 - self.z).unsigned_abs();
        let have = self.tot - self.hist0;
        if have >= SHORT as u64 {
            let o = buf[((self.tot - SHORT as u64) % cap) as usize] as i16;
            self.sum -= (o as i32 - self.z).unsigned_abs();
        }
        buf[(self.tot % cap) as usize] = c as u16;
        self.sum += a;
        self.tot += 1;
        let d = (c as i32 - self.z) as i64;
        self.sq += (d * d) as u64;
        self.sq_n += 1;
        if self.sq_n == WIN100 as u32 {
            self.live_ms = (self.sq / WIN100 as u64).min(u32::MAX as u64) as u32;
            (self.sq, self.sq_n) = (0, 0);
        }
        match self.phase {
            Phase::Arming | Phase::Armed => {
                if have + 1 < SHORT as u64 {
                    return false;
                }
                if self.phase == Phase::Armed && self.sum > self.env_max.saturating_add(self.step_sum) {
                    self.phase = Phase::Post;
                    self.trig = self.tot - SHORT as u64;
                    return false;
                }
                self.blk_max = self.blk_max.max(self.sum);
                self.blk_n += 1;
                if self.blk_n == BLK as u32 {
                    self.env[self.env_i as usize] = self.blk_max;
                    self.env_i = (self.env_i + 1) % NBLK as u8;
                    self.env_n = (self.env_n + 1).min(NBLK as u8);
                    self.env_max = self.env[..self.env_n as usize].iter().copied().max().unwrap_or(0);
                    (self.blk_max, self.blk_n) = (0, 0);
                }
                if self.phase == Phase::Arming && have + 1 >= (PRE + SHORT) as u64 && self.env_n as usize == NBLK {
                    self.phase = Phase::Armed;
                }
                false
            }
            Phase::Post => {
                if self.tot >= self.trig + POST as u64 {
                    self.phase = Phase::Done;
                    return true;
                }
                false
            }
            _ => false,
        }
    }

    /// Live rms while armed, in codes (the last complete 100 ms).
    pub fn live_codes(&self) -> f32 {
        sqrtf(self.live_ms as f32)
    }

    /// Rotate the ring so `buf[..LEN]` is the trace (PRE samples before the
    /// trigger first). Done only.
    #[inline(never)]
    pub fn linearize(&mut self, buf: &mut [u16]) {
        let cap = buf.len() as u64;
        let start = self.trig - PRE as u64;
        let r = (start % cap) as usize;
        buf.rotate_left(r);
        self.rd = 0;
    }

    /// Results over the linear trace; `b` = A per code, about the exact
    /// zero code `zf`.
    #[inline(never)]
    pub fn results(&self, buf: &[u16], b: f32) -> Res {
        let zf = self.zf;
        let t = &buf[..LEN];
        let ms = |s: &[u16]| -> f32 {
            // Exact integer sums about the rounded zero, then the fraction.
            let (mut s1, mut s2) = (0i64, 0i64);
            for &c in s {
                let d = (c as i16 as i32 - self.z) as i64;
                s1 += d;
                s2 += d * d;
            }
            let n = s.len().max(1) as f32;
            let f = zf - self.z as f32;
            ((s2 as f32 - 2.0 * f * s1 as f32) / n + f * f).max(0.0)
        };
        // From the zero-current code: with an AD2 offset larger than the
        // inrush, a start at code 0 would report minus the offset.
        let mut pk = self.z;
        let mut ol = false;
        for &c in &t[PRE..] {
            let c = c as i16 as i32;
            ol |= c.abs() >= CODE_OL;
            if (c - self.z).abs() > (pk - self.z).abs() {
                pk = c;
            }
        }
        Res {
            rms100: sqrtf(ms(&t[PRE..PRE + WIN100])) * b,
            peak: (pk as f32 - zf) * b,
            idle: sqrtf(ms(&t[..PRE - SHORT])) * b,
            ol,
            lost: self.lost,
        }
    }

    /// The saved file's records (stamp us, raw: fastlog::Rec) from the
    /// linear trace, from where the last call stopped: stamps from the
    /// nominal rate (t = 0 at the first trace sample; the ISR samples on the
    /// chip's conversion grid), a lost stretch as a stamp gap at `hole_at`.
    /// Returns the records written (0 = all out).
    #[inline(never)]
    pub fn records(&mut self, buf: &[u16], out: &mut [(u32, i32)]) -> usize {
        let n = out.len().min(LEN - self.rd as usize);
        for (i, o) in out[..n].iter_mut().enumerate() {
            let k = self.rd as u64 + i as u64;
            let skip = if self.lost > 0 && k >= self.hole_at as u64 { self.lost as u64 } else { 0 };
            let ts = ((k + skip) * 1_000_000 / RATE_HZ as u64) as u32;
            *o = (ts, buf[k as usize] as i16 as i32);
        }
        self.rd += n as u32;
        n
    }

    /// Records not yet handed out.
    #[allow(dead_code)] // host tests
    pub fn records_left(&self) -> u32 {
        LEN as u32 - self.rd
    }
}

/// What main.rs does with the AD2 stream next (`App::take_inrush_req`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Req {
    /// (Re)start the stream on the current row (AC A, 10 A) at RATE_HZ.
    Start,
    /// Stop it (complete, cancelled or the page closed); then
    /// `App::inrush_stopped`.
    Stop,
}

/// The saved file's progress.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Save {
    None,
    /// Waiting for the fast logger's session (main.rs `fast_req`).
    Pending,
    /// The session runs (fastlog Prep / Drain), reading the trace.
    Running,
    /// Saved as LOGS/FASTnnn.DLG.
    Saved(u16),
    /// Not saved (why: the fast logger's end state code, fastlog::State).
    Failed(u8),
}

/// Why the last capture stopped short (the state line).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Stop {
    None,
    /// Cancelled (the button, RUN).
    Cancel,
    /// The stream died: a DMM change (host CONF, ...), a host data logger.
    Lost(&'static str),
}

/// The inrush session (the page's state; a static: TCM .bss, not on the
/// main stack as an App field would be).
pub struct Session {
    /// The page is open: BUF is lent (`Scope::lend_buf`).
    pub on: bool,
    /// The AD2 stream runs for the capture (main.rs started it).
    pub streaming: bool,
    /// The page closed while streaming / saving: finish, then restore.
    pub closing: bool,
    pub cap: Cap,
    /// Step threshold, A.
    pub thr: f32,
    /// A per 16-bit code, the calibration's zero code (exact; the capture's
    /// own zero: `cap.zf`), the last zero measured (NaN: none since boot).
    pub b: f32,
    pub zf: f32,
    pub zm: f32,
    pub res: Option<Res>,
    /// logger::wave::dropped() last seen.
    pub dropped: u32,
    pub req: Option<Req>,
    pub save: Save,
    pub stop: Stop,
    /// The DMM function before the page (Dmm::func), the A function's
    /// sub-mode (255: not A), autorange a_auto / manual item: restored on
    /// close while the DMM is still in the page's AC A 10 A state.
    pub prev_func: u8,
    pub prev_sub: u8,
    pub prev_auto: Option<u8>,
    pub prev_item: Option<u8>,
    /// The stream's nominal rate, mHz (the file header).
    pub rate_mhz: u32,
    /// Live redraw: what the state line / buttons last showed, next time.
    pub shown: u32,
    pub next_live: u64,
    /// The trace on the page is the current capture.
    pub traced: bool,
}

impl Session {
    pub const NEW: Session = Session {
        on: false,
        streaming: false,
        closing: false,
        cap: Cap::OFF,
        thr: THR_DEFAULT,
        b: 125.0 / 262_144.0 * 8.0,
        zf: 0.0,
        zm: f32::NAN,
        res: None,
        dropped: 0,
        req: None,
        save: Save::None,
        stop: Stop::None,
        prev_func: 8,
        prev_sub: 0,
        prev_auto: None,
        prev_item: None,
        rate_mhz: RATE_HZ * 1000,
        shown: u32::MAX,
        next_live: 0,
        traced: false,
    };

    /// The saved trace is being read by the fast logger (the ring / page
    /// must wait).
    pub fn saving(&self) -> bool {
        matches!(self.save, Save::Pending | Save::Running)
    }
}

static mut SES: Session = Session::NEW;

/// The session (main loop only; no reference kept across a call that
/// takes another).
#[inline(always)]
pub fn ses() -> &'static mut Session {
    // SAFETY: main loop only, the ISR never sees it.
    unsafe { &mut *core::ptr::addr_of_mut!(SES) }
}

/// sqrt without libm (Newton from a bit-trick guess, as dmm.rs).
pub fn sqrtf(x: f32) -> f32 {
    if x <= 0.0 {
        return 0.0;
    }
    let mut y = f32::from_bits((x.to_bits() >> 1) + 0x1FBD_1DF5);
    for _ in 0..4 {
        y = 0.5 * (y + x / y);
    }
    y
}

/// Nearest integer (no libm).
pub fn round_i(x: f32) -> i32 {
    if x >= 0.0 { (x + 0.5) as i32 } else { (x - 0.5) as i32 }
}

/// The step threshold in codes for `amps` at `b` A per code.
pub fn step_codes(amps: f32, b: f32) -> u32 {
    let k = amps / b.max(1e-9) + 0.5;
    if k < 1.0 { 1 } else { k as u32 }
}

/// Next / previous threshold of the stepper from `cur` (`dir` -1 / +1):
/// the nearest ladder step below / above it. `cur` stays when there is
/// none: a value set over SCPI below 0.2 A or above 10 A (a "-" raised
/// 0.1 A to 0.2 A, a "+" lowered 30 A to 10 A).
pub fn thr_step(cur: f32, dir: i8) -> f32 {
    let next = if dir < 0 {
        THR_STEPS.iter().rev().find(|&&t| t < cur * 0.999)
    } else {
        THR_STEPS.iter().find(|&&t| t > cur * 1.001)
    };
    next.copied().unwrap_or(cur)
}

#[cfg(test)]
mod tests {
    use super::*;

    const B: f32 = 125.0 / 262_144.0 * 8.0; // A per code, 10 A range x1

    fn feed(cap: &mut Cap, buf: &mut [u16], f: &mut dyn FnMut(usize) -> f32, n: usize, z: f32) -> Option<usize> {
        for k in 0..n {
            let c = code_of(((f(k) / B + z) * 8.0).round() as i32);
            if cap.push(buf, c) {
                return Some(k);
            }
        }
        None
    }

    fn noise(k: usize) -> f32 {
        // Deterministic +-8 mA "noise".
        (((k * 7919) % 17) as f32 - 8.0) * 0.001
    }

    fn run(f: &mut dyn FnMut(usize) -> f32, thr: f32, z: f32) -> (Cap, Vec<u16>, Option<usize>) {
        let mut buf = vec![0u16; 26_624];
        let mut cap = Cap::OFF;
        cap.arm(z, 0.0, step_codes(thr, B));
        let done = feed(&mut cap, &mut buf, f, 60_000, z);
        (cap, buf, done)
    }

    #[test]
    fn dc_step_triggers_and_measures() {
        // 0.05 A idle, 8 A from sample 5000.
        let (mut cap, mut buf, done) = run(&mut |k| if k >= 5000 { 8.0 } else { 0.05 } + noise(k), 1.0, 3.0);
        assert!(done.is_some());
        assert_eq!(cap.phase, Phase::Done);
        // The window's first sample: at most SHORT - 1 before the step.
        assert!(cap.trig <= 5000 && cap.trig + SHORT as u64 > 5000, "trig {}", cap.trig);
        cap.linearize(&mut buf);
        let r = cap.results(&buf, B);
        assert!((r.rms100 - 8.0).abs() < 0.08, "{:?}", r);
        assert!((r.peak - 8.0).abs() < 0.03, "{:?}", r);
        assert!((r.idle - 0.05).abs() < 0.01, "{:?}", r);
        assert!(!r.ol && r.lost == 0);
        assert_eq!(done.unwrap() as u64 + 1, cap.trig + POST as u64);
    }

    #[test]
    fn decaying_inrush_rms_window() {
        // 12 A for 20 ms, 6 A for 80 ms, then 3 A: Irms(100 ms) = sqrt(57.6).
        let s = 4000usize;
        let f = &mut |k: usize| {
            if k < s {
                0.0
            } else if k < s + 192 {
                12.0
            } else if k < s + 960 {
                6.0
            } else {
                3.0
            }
        };
        let (mut cap, mut buf, done) = run(f, 1.0, 0.0);
        assert!(done.is_some());
        cap.linearize(&mut buf);
        // t = 0 may sit up to SHORT - 1 samples before the step: those are 0.
        let pre0 = (s as u64 - cap.trig) as usize;
        let want = ((144.0 * 192.0 + 36.0 * (768.0 - pre0 as f32)) / 960.0f32).sqrt();
        let r = cap.results(&buf, B);
        assert!((r.rms100 - want).abs() < 0.02, "{:?} want {}", r, want);
        assert!((r.peak - 12.0).abs() < 0.01);
    }

    #[test]
    fn running_ac_load_does_not_trigger_but_a_step_on_it_does() {
        // 2 A rms 50 Hz running: no trigger at a 1 A step.
        let sine = |k: usize, a: f32| a * 1.414_213_5 * (2.0 * core::f32::consts::PI * 50.0 * k as f32 / RATE_HZ as f32).sin();
        let (cap, _, done) = run(&mut |k| sine(k, 2.0) + noise(k), 1.0, 0.0);
        assert!(done.is_none() && cap.phase == Phase::Armed, "{:?}", cap.phase);
        // The same load going to 5 A rms at sample 9000 triggers.
        let (mut cap, mut buf, done) = run(&mut |k| sine(k, if k >= 9000 { 5.0 } else { 2.0 }), 1.0, 0.0);
        assert!(done.is_some());
        assert!(cap.trig >= 9000 - SHORT as u64 && cap.trig < 9000 + 100, "trig {}", cap.trig);
        cap.linearize(&mut buf);
        let r = cap.results(&buf, B);
        // The window starts within ~1 ms of the step: rms close to 5 A.
        assert!((r.rms100 - 5.0).abs() < 0.15, "{:?}", r);
        assert!((r.idle - 2.0).abs() < 0.05, "{:?}", r);
        assert!((r.peak.abs() - 7.07).abs() < 0.05, "{:?}", r);
    }

    #[test]
    fn negative_step_and_ol() {
        let (mut cap, mut buf, done) = run(&mut |k| if k >= 3000 { -130.0 } else { 0.0 }, 0.5, 0.0);
        assert!(done.is_some());
        cap.linearize(&mut buf);
        let r = cap.results(&buf, B);
        assert!(r.ol && r.peak < -124.0, "{:?}", r);
    }

    #[test]
    fn small_step_below_threshold_is_ignored() {
        let (cap, _, done) = run(&mut |k| if k >= 3000 { 0.6 } else { 0.0 } + noise(k), 1.0, 0.0);
        assert!(done.is_none() && cap.phase == Phase::Armed);
    }

    #[test]
    fn no_trigger_before_the_history() {
        // A current flowing from the start: the history fills with it.
        let (cap, _, done) = run(&mut |_| 6.0, 1.0, 0.0);
        assert!(done.is_none() && cap.phase == Phase::Armed);
    }

    #[test]
    fn hole_while_armed_restarts_history_and_records_skip_a_hole_after() {
        let mut buf = vec![0u16; 26_624];
        let mut cap = Cap::OFF;
        cap.arm(0.0, 0.0, step_codes(1.0, B));
        let f = |k: usize| if k >= 6000 { 4.0f32 } else { 0.0 };
        for k in 0..3000 {
            cap.push(&mut buf, code_of(((f(k) / B) * 8.0) as i32));
        }
        assert_eq!(cap.phase, Phase::Armed);
        cap.hole(5);
        assert_eq!(cap.phase, Phase::Arming);
        let mut k = 3000;
        while cap.phase != Phase::Post {
            cap.push(&mut buf, code_of(((f(k) / B) * 8.0) as i32));
            k += 1;
        }
        cap.hole(7);
        while !cap.push(&mut buf, code_of(((f(k) / B) * 8.0) as i32)) {
            k += 1;
        }
        assert_eq!(cap.lost, 7);
        cap.linearize(&mut buf);
        let mut out = vec![(0u32, 0i32); LEN];
        let n = cap.records(&buf, &mut out);
        assert_eq!(n, LEN);
        assert_eq!(cap.records_left(), 0);
        let ts = |i: usize| out[i].0;
        let h = cap.hole_at as usize;
        assert!(h > PRE);
        assert_eq!(ts(h) - ts(h - 1), ((h as u64 + 7) * 1_000_000 / 9600 - (h as u64 - 1) * 1_000_000 / 9600) as u32);
        assert_eq!(ts(1) - ts(0), 104);
    }

    #[test]
    fn small_step_on_a_large_offset() {
        // Zero code 200 (an AD2 offset of ~0.76 A at x1), a 0.3 A step at a
        // 0.2 A threshold: the peak is the step, not minus the offset.
        let (mut cap, mut buf, done) = run(&mut |k| if k >= 4000 { 0.3 } else { 0.0 }, 0.2, 200.0);
        assert!(done.is_some());
        cap.linearize(&mut buf);
        let r = cap.results(&buf, B);
        assert!((r.peak - 0.3).abs() < 0.01, "{:?}", r);
        assert!((r.rms100 - 0.3).abs() < 0.01, "{:?}", r);
        assert!(r.idle.abs() < 0.005, "{:?}", r);
    }

    /// A capture with the measured zero: the cal zero `zc`, the true zero
    /// code `z`, the current `f`.
    fn run_mz(f: &mut dyn FnMut(usize) -> f32, zc: f32, z: f32) -> (Cap, Vec<u16>, Option<usize>) {
        let mut buf = vec![0u16; 26_624];
        let mut cap = Cap::OFF;
        cap.arm(zc, B, step_codes(1.0, B));
        let done = feed(&mut cap, &mut buf, f, 60_000, z);
        (cap, buf, done)
    }

    #[test]
    fn measured_zero_takes_the_ad2_offset() {
        // Device 185c066: +0.0717 A of AD2 offset (18.8 codes), the cal's 0;
        // nothing in A, then 8 A at sample 5000.
        let z = 0.0717 / B;
        let (mut cap, mut buf, done) = run_mz(&mut |k| if k >= 5000 { 8.0 } else { 0.0 } + noise(k), 0.0, z);
        assert!((cap.zf - z).abs() < 0.2 && cap.zok, "zf {} want {}", cap.zf, z);
        assert!(done.is_some());
        cap.linearize(&mut buf);
        let r = cap.results(&buf, B);
        assert!(r.idle < 0.01, "{:?}", r);
        assert!((r.peak - 8.0).abs() < 0.02, "{:?}", r);
        // The same offset as a constant current at the true zero 0.
        let (cap2, _, _) = run_mz(&mut |k| 0.0717 + noise(k), 0.0, 0.0);
        assert!((cap2.zf * B - 0.0717).abs() < 0.002, "{}", cap2.zf * B);
    }

    #[test]
    fn measured_zero_live_level() {
        // The live rms while armed is about the measured zero.
        let z = 0.0717 / B;
        let mut buf = vec![0u16; 26_624];
        let mut cap = Cap::OFF;
        cap.arm(0.0, B, step_codes(1.0, B));
        feed(&mut cap, &mut buf, &mut |k| noise(k), 6000, z);
        assert_eq!(cap.phase, Phase::Armed);
        assert!(cap.live_codes() * B < 0.01, "live {} A", cap.live_codes() * B);
    }

    #[test]
    fn measured_zero_keeps_the_cal_with_a_load() {
        // A 2 A rms load running at the arm (rms over ZERO_RMS_A) and a 0.5 A
        // DC one (beyond ZERO_MAX_A): the cal's zero stays.
        let sine = |k: usize| 2.0 * 1.414_213_5 * (2.0 * core::f32::consts::PI * 45.0 * k as f32 / RATE_HZ as f32).sin();
        let (cap, _, _) = run_mz(&mut |k| sine(k), 3.0, 3.0);
        assert!(cap.zf == 3.0 && !cap.zok);
        let (cap, _, _) = run_mz(&mut |_| 0.5, 3.0, 3.0);
        assert!(cap.zf == 3.0 && !cap.zok);
    }

    #[test]
    fn hole_while_measuring_the_zero_starts_it_again() {
        let z = 0.0717 / B;
        let mut buf = vec![0u16; 26_624];
        let mut cap = Cap::OFF;
        cap.arm(0.0, B, step_codes(1.0, B));
        feed(&mut cap, &mut buf, &mut |_| 0.0, 500, z);
        cap.hole(3);
        assert_eq!(cap.zf, 0.0);
        feed(&mut cap, &mut buf, &mut |_| 0.0, (ZERO_SKIP + ZERO_N) as usize, z);
        // (no noise: the mean is the rounded code, 19 for 18.8)
        assert!((cap.zf - z).abs() <= 0.5, "zf {} want {}", cap.zf, z);
        // after the zero: a hole keeps it
        let zf = cap.zf;
        cap.hole(3);
        assert_eq!(cap.zf, zf);
    }

    #[test]
    fn stepper_ladder() {
        assert_eq!(thr_step(1.0, 1), 2.0);
        assert_eq!(thr_step(1.0, -1), 0.5);
        assert_eq!(thr_step(0.2, -1), 0.2);
        assert_eq!(thr_step(10.0, 1), 10.0);
        assert_eq!(thr_step(0.7, 1), 1.0);
        assert_eq!(thr_step(0.7, -1), 0.5);
        // Off the ladder (INR:THR): never the other way.
        assert_eq!(thr_step(0.1, -1), 0.1);
        assert_eq!(thr_step(0.1, 1), 0.2);
        assert_eq!(thr_step(30.0, 1), 30.0);
        assert_eq!(thr_step(30.0, -1), 10.0);
        assert_eq!(thr_step(12.0, -1), 10.0);
        assert_eq!(step_codes(1.0, B), 262);
    }
}
