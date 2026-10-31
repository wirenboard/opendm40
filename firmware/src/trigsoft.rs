//! Software pulse-width and runt triggers (UF-13), searched in a complete
//! linear record (scope.rs `finish`; the comparator and the continuous
//! stream search only know edges). Pure, host-tested by tools/trig-test.
//!
//! Limits (by construction, stated in the Trigger tab notes and the TODO):
//!  * not a hardware trigger: each record (scope.rs `lin_len`: 26624 samples
//!    or up to 0.1 s, at most scope::CAP) is
//!    searched after it is complete, so an event in the dead time between
//!    two records (search + drawing + restart) is missed; a rare glitch is
//!    caught with probability ~ record time / (record time + dead time);
//!  * only events that lie wholly inside one record and leave a full screen
//!    around the trigger point (the trigger position) count;
//!  * width resolution = one sample (1 / sample rate of the timebase), the
//!    10-bit codes with +-2 codes of hysteresis around each level;
//!  * slow timebases need the whole record per frame (26624 samples: 0.2 s
//!    at 125 kS/s, 10.6 s at 1 s/div; 43008 = 0.9 ms at 50 MS/s);
//!  * roll mode has no trigger (as the edge trigger).

/// Trigger types (Settings::ttype).
#[allow(dead_code)] // the match default; the stream ISR's StreamCfg::OFF
pub const EDGE: u8 = 0;
pub const PULSE: u8 = 1;
pub const RUNT: u8 = 2;
pub const TYPE_NAMES: [&str; 3] = ["Edge", "Pulse", "Runt"];
/// Pulse-width conditions (Settings::pcond).
pub const GT: u8 = 0;
pub const LT: u8 = 1;
#[cfg_attr(not(test), allow(dead_code))] // tools/trig-test (Settings::pcond is set from the UI / SCPI as 0..2)
pub const EQ: u8 = 2;
pub const COND_NAMES: [&str; 3] = [">", "<", "="];
/// Hysteresis around a level, codes (as the edge search).
const HYS: i32 = 2;

/// Pulse-width trigger: the first pulse whose trailing edge lies in
/// [lo, hi) and whose width (samples between the leading and trailing
/// crossings of level `l`) meets `cond` against `w` samples (`EQ`: within
/// +-5 %, at least +-1 sample). `positive`: a high pulse (rising, then
/// falling), else a low one. Returns the trailing-edge index.
pub fn pulse<S: crate::pack10::Src + ?Sized>(d: &S, lo: usize, hi: usize, l: i32, positive: bool, cond: u8, w: f32) -> Option<usize> {
    let hi = hi.min(d.len());
    // state: 0 waiting for the far side (arm), 1 armed, 2 inside a pulse
    // (start = leading edge), the trailing edge needs the near side first.
    let (mut armed, mut start, mut inside) = (false, 0usize, false);
    let mut deep = false;
    let tol = (w * 0.05).max(1.0);
    for i in 0..hi {
        let v = d.at(i) as i32;
        let (pre, past, back) = if positive { (v < l - HYS, v > l, v < l) } else { (v > l + HYS, v < l, v > l) };
        let deep_now = if positive { v > l + HYS } else { v < l - HYS };
        if !inside {
            if pre {
                armed = true;
            } else if armed && past {
                inside = true;
                deep = deep_now;
                start = i;
            }
        } else {
            deep |= deep_now;
            if deep && back {
                let width = (i - start) as f32;
                let ok = match cond {
                    GT => width > w,
                    LT => width < w,
                    _ => (width - w).abs() <= tol,
                };
                if ok && i >= lo {
                    return Some(i);
                }
                inside = false;
                armed = pre;
            }
        }
    }
    None
}

/// Runt trigger between levels `l1` and `l2` (any order): `positive`: a
/// pulse that rises through the lower level and falls back below it
/// without reaching the upper one; else a pulse that falls through the
/// upper level and returns above it without reaching the lower one. The
/// return crossing in [lo, hi) is the trigger point.
pub fn runt<S: crate::pack10::Src + ?Sized>(d: &S, lo: usize, hi: usize, l1: i32, l2: i32, positive: bool) -> Option<usize> {
    let hi = hi.min(d.len());
    let (a, b) = (l1.min(l2), l1.max(l2));
    if b - a <= 2 * HYS {
        return None;
    }
    let (mut armed, mut inside, mut reached, mut deep) = (false, false, false, false);
    for i in 0..hi {
        let v = d.at(i) as i32;
        if positive {
            if !inside {
                if v < a - HYS {
                    armed = true;
                } else if armed && v > a {
                    inside = true;
                    reached = v > b;
                    deep = v > a + HYS;
                }
            } else {
                reached |= v > b;
                deep |= v > a + HYS;
                if deep && v < a {
                    if !reached && i >= lo {
                        return Some(i);
                    }
                    inside = false;
                    armed = v < a - HYS;
                }
            }
        } else if !inside {
            if v > b + HYS {
                armed = true;
            } else if armed && v < b {
                inside = true;
                reached = v < a;
                deep = v < b - HYS;
            }
        } else {
            reached |= v < a;
            deep |= v < b - HYS;
            if deep && v > b {
                if !reached && i >= lo {
                    return Some(i);
                }
                inside = false;
                armed = v > b + HYS;
            }
        }
    }
    None
}

/// Pulse-width steps (s): 1-2-5 values from 20 ns to 10 s.
pub const W_MIN: f32 = 20e-9;
pub const W_MAX: f32 = 10.0;

/// The next 1-2-5 width above (`up`) or below `w`, clamped to W_MIN..W_MAX.
pub fn width_step(w: f32, up: bool) -> f32 {
    let mut dec = 1e-8f32;
    let mut prev = W_MIN;
    while dec < 20.0 {
        for k in [1.0f32, 2.0, 5.0] {
            let x = dec * k;
            if !(W_MIN * 0.99..=W_MAX * 1.01).contains(&x) {
                continue;
            }
            if up && x > w * 1.001 {
                return x;
            }
            if !up && x >= w * 0.999 {
                return prev;
            }
            prev = x;
        }
        dec *= 10.0;
    }
    if up { W_MAX } else { prev }
}

/// Per-sample trigger configuration of the streaming detector (`Stream`):
/// integers only (it runs in the AD2 stream ISR, which uses no float).
/// `l` / `l2`: levels in the same 10-bit codes as the record; `wlo..=whi`:
/// the accepted pulse widths in samples (`pulse_window`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct StreamCfg {
    pub ttype: u8,
    pub rising: bool,
    pub l: i32,
    pub l2: i32,
    pub wlo: u32,
    pub whi: u32,
}

#[cfg_attr(not(feature = "ad2-isr"), allow(dead_code))] // the stream ISR's
impl StreamCfg {
    pub const OFF: StreamCfg = StreamCfg { ttype: EDGE, rising: true, l: i32::MIN, l2: 0, wlo: 0, whi: 0 };
}

/// Accepted widths (samples, inclusive) of `cond` against `w` samples,
/// the integer equivalent of `pulse`'s float rule on whole-sample widths:
/// GT width > w, LT width < w, EQ |width - w| <= max(5 % of w, 1).
pub fn pulse_window(cond: u8, w: f32) -> (u32, u32) {
    let w = if w.is_finite() { w.clamp(0.0, 4.0e9) } else { 0.0 };
    let fl = |x: f32| if x <= 0.0 { 0 } else { x as u32 };
    let ce = |x: f32| {
        let f = fl(x);
        if (f as f32) < x { f + 1 } else { f }
    };
    match cond {
        GT => (fl(w) + 1, u32::MAX),
        LT => {
            let c = ce(w);
            if c == 0 { (1, 0) } else { (0, c - 1) }
        }
        _ => {
            let tol = (w * 0.05).max(1.0);
            (ce(w - tol), fl(w + tol))
        }
    }
}

/// The edge / pulse / runt rules of `find_trigger` (scope.rs), `pulse` and
/// `runt`, one sample at a time, for a continuous stream (the DMM wave
/// view: the AD2 stream ISR runs it on every sample, src/logger.rs `isr`).
/// `step` returns true when sample `i` completes an event (an edge, a
/// pulse's trailing edge, a runt's return); the state then re-arms as the
/// slice functions do after a rejected candidate, so the next event needs
/// the arming condition again (an edge: the far side of the hysteresis
/// band). A few compares per sample; no loop, no float.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[cfg_attr(not(feature = "ad2-isr"), allow(dead_code))] // the stream ISR's
pub struct Stream {
    armed: bool,
    inside: bool,
    deep: bool,
    reached: bool,
    start: u32,
}

#[cfg_attr(not(feature = "ad2-isr"), allow(dead_code))]
impl Stream {
    pub const NEW: Stream = Stream { armed: false, inside: false, deep: false, reached: false, start: 0 };

    #[inline(always)]
    pub fn step(&mut self, c: &StreamCfg, i: u32, v: i32) -> bool {
        let l = c.l;
        match c.ttype {
            PULSE => {
                let (pre, past, back) = if c.rising { (v < l - HYS, v > l, v < l) } else { (v > l + HYS, v < l, v > l) };
                let deep_now = if c.rising { v > l + HYS } else { v < l - HYS };
                if !self.inside {
                    if pre {
                        self.armed = true;
                    } else if self.armed && past {
                        self.inside = true;
                        self.deep = deep_now;
                        self.start = i;
                    }
                    false
                } else {
                    self.deep |= deep_now;
                    if self.deep && back {
                        let w = i.wrapping_sub(self.start);
                        self.inside = false;
                        self.armed = pre;
                        w >= c.wlo && w <= c.whi
                    } else {
                        false
                    }
                }
            }
            RUNT => {
                let (a, b) = (l.min(c.l2), l.max(c.l2));
                if b - a <= 2 * HYS {
                    return false;
                }
                if c.rising {
                    if !self.inside {
                        if v < a - HYS {
                            self.armed = true;
                        } else if self.armed && v > a {
                            self.inside = true;
                            self.reached = v > b;
                            self.deep = v > a + HYS;
                        }
                        false
                    } else {
                        self.reached |= v > b;
                        self.deep |= v > a + HYS;
                        if self.deep && v < a {
                            self.inside = false;
                            self.armed = v < a - HYS;
                            !self.reached
                        } else {
                            false
                        }
                    }
                } else if !self.inside {
                    if v > b + HYS {
                        self.armed = true;
                    } else if self.armed && v < b {
                        self.inside = true;
                        self.reached = v < a;
                        self.deep = v < b - HYS;
                    }
                    false
                } else {
                    self.reached |= v < a;
                    self.deep |= v < b - HYS;
                    if self.deep && v > b {
                        self.inside = false;
                        self.armed = v > b + HYS;
                        !self.reached
                    } else {
                        false
                    }
                }
            }
            _ => {
                let hit = if c.rising {
                    if v < l - HYS {
                        self.armed = true;
                        false
                    } else {
                        self.armed && v > l
                    }
                } else if v > l + HYS {
                    self.armed = true;
                    false
                } else {
                    self.armed && v < l
                };
                if hit {
                    self.armed = false;
                }
                hit
            }
        }
    }
}

/// AD2 code (19-bit signed, src/hy3131.rs) -> the 10-bit offset-binary code
/// of a scope record (the DMM wave view): the full AD2 window (+-1.25 V /
/// IG at the AD2 input) over 0..1023, 2^18 / 512 = 512 AD2 codes per step.
/// The stream ISR's trigger and the record use the same code.
#[inline(always)]
pub const fn ad2_code10(v: i32) -> i32 {
    let c = (v >> 9) + 512;
    if c < 0 { 0 } else if c > 1023 { 1023 } else { c }
}
