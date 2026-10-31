//! Protocol decoder (TODO "Protocol decoders"): UART, Modbus RTU and DALI decoded
//! from the scope's record, drawn as a bus band over the graph (scope_ui
//! `dec`, app_decode.rs) and read with DECode:FRAMes?.
//!
//! Pure: it reads samples through `pack10::Src` (the record's `Codes`
//! view, or a u16 slice in the host tests) and reports characters and
//! frames through callbacks; no state between calls but what the caller
//! keeps (the bit rate found last). Host-tested by tools/decode-test.
//!
//!  * threshold: a manual code, or the middle of the window's min / max
//!    with a hysteresis of 1/6 of the swing; a swing under `MIN_SWING`
//!    codes is no signal;
//!  * bit rate: set, or the shortest edge-to-edge runs (the `KRUN` shortest
//!    of the window, those within 30 % of the shortest averaged), snapped
//!    to a standard rate within 8 %;
//!  * characters: a start edge after a mark level (hysteresis), each bit
//!    sampled at its centre (majority of 3 samples around the middle
//!    level); start, data LSB first, parity, one stop bit (framing error
//!    if it is space). A character that runs past the window's end is not
//!    reported;
//!  * Modbus RTU frames (`Framer`): characters closer than 3.5 character
//!    times (1.75 ms above 19200 baud), CRC-16 (computed as the bytes come:
//!    no frame buffer), request / reply / exception from the function and
//!    the length; a frame that may have begun before the window or go on
//!    after it is partial (no CRC);
//!  * DALI (`dali_step`, feature `dec-dali`): bi-phase at a fixed 1200
//!    bit/s (a half bit Te = 416.67 us, the rate +-10 %), idle high. On the
//!    bus a 1 is low for the first half bit and high for the second (a
//!    rising edge in the bit's centre), a 0 the reverse: Microchip AN1465
//!    (DS01465A, "Manchester Encoding": a 1 is a falling edge as the PIC
//!    sees it behind the opto-coupler, which inverts the bus) and
//!    [Wikipedia](https://en.wikipedia.org/wiki/Digital_Addressable_Lighting_Interface)
//!    ("Technical overview"). A frame: a start bit (a 1), 8 (backward), 16
//!    (forward: address, data) or 24 data bits MSB first, two stop bits
//!    (idle). The runs between edges are one half bit (0.7 .. 1.3 Te) or
//!    two (1.6 .. 2.4 Te; these windows are this decoder's, not IEC
//!    62386-101's receiver limits, which are not checked here); anything
//!    else, or a bit with no transition in its centre, makes the frame an
//!    error. High for more than 2.5 Te ends a frame, and a frame starts only
//!    after that much idle; a first low run under 0.5 Te is a glitch.

// `min_max`, `crc16`, `W::as_str`: the host tests.
#![cfg_attr(target_os = "none", allow(dead_code))]

use crate::pack10::Src;

/// Standard bit rates (auto-baud snaps to these).
pub const STD_BAUD: [u32; 13] = [300, 600, 1200, 2400, 4800, 9600, 14400, 19200, 28800, 38400, 57600, 76800, 115_200];
pub const MAX_BAUD: u32 = 115_200;
pub const MIN_BAUD: u32 = 300;

/// Protocols (Cfg::proto).
pub const P_OFF: u8 = 0;
pub const P_UART: u8 = 1;
pub const P_MODBUS: u8 = 2;
pub const P_DALI: u8 = 3;
/// Modbus RTU is built in (firmware: feature `dec-modbus`; the host tests
/// always); without it only UART.
pub const MODBUS: bool = cfg!(any(feature = "dec-modbus", not(target_os = "none")));
/// `proto` is Modbus RTU (always false without `dec-modbus`).
#[inline(always)]
pub fn is_mb(proto: u8) -> bool {
    MODBUS && proto == P_MODBUS
}

/// DALI is built in (firmware: feature `dec-dali`; the host tests always).
pub const DALI: bool = cfg!(any(feature = "dec-dali", not(target_os = "none")));
/// `proto` is DALI (always false without `dec-dali`).
#[inline(always)]
pub fn is_dali(proto: u8) -> bool {
    DALI && proto == P_DALI
}
/// DALI's fixed bit rate (1200 bit/s, a half bit Te = 416.67 us).
pub const DALI_BAUD: u32 = 1200;

/// Frame formats: data bits, parity (0 none, 1 even, 2 odd).
pub const FMT_NAMES: [&str; 4] = ["8N1", "8E1", "8O1", "7E1"];
const FMT: [(u8, u8); 4] = [(8, 0), (8, 1), (8, 2), (7, 1)];

/// Auto threshold: the swing needed, codes (~0.6 V at 2 V/div, 60 mV at
/// 200 mV/div); a manual level's hysteresis.
pub const MIN_SWING: u16 = 24;
const HYST_MANUAL: u16 = 6;
/// Auto-baud: runs shorter than this are noise or bits too short to
/// decode; the shortest KRUN runs kept.
const RUN_MIN: u32 = 3;
const KRUN: usize = 16;
/// Fewest samples per bit decoded (the Wave view's 38.4 kS/s: 4800 baud
/// has 8, 9600 baud 4).
pub const MIN_SPB: u32 = 4;

/// Character / frame flags.
pub const F_FE: u8 = 1;
pub const F_PE: u8 = 2;
pub const F_CRC: u8 = 4;
/// Frame: its start or end may lie outside the window (no CRC check).
pub const F_PART: u8 = 8;
/// Frame kinds.
pub const K_UART: u8 = 0;
pub const K_REQ: u8 = 1;
pub const K_RSP: u8 = 2;
pub const K_EXC: u8 = 3;
/// Modbus frame that is no request / reply (bad CRC, partial, too short).
pub const K_MB: u8 = 4;
/// A DALI frame (`len` bytes: 1 backward, 2 forward, 3 a 24-bit forward
/// frame; F_FE with len 0: bad Manchester or timing).
pub const K_DALI: u8 = 5;

/// Decoder settings (DECode:..., the scope menu's Decode popup).
#[derive(Clone, Copy, PartialEq)]
pub struct Cfg {
    pub proto: u8,
    /// Bit rate, 0 = auto.
    pub baud: u32,
    /// FMT_NAMES index.
    pub fmt: u8,
    /// RS-232 polarity: idle low, a space high.
    pub inverted: bool,
}

impl Cfg {
    pub const DEFAULT: Cfg = Cfg { proto: P_OFF, baud: 0, fmt: 0, inverted: false };
    pub fn bits(&self) -> (u8, u8) {
        FMT[self.fmt as usize % 4]
    }
    /// Bits per character (start + data + parity + stop).
    pub fn char_bits(&self) -> u32 {
        let (b, p) = self.bits();
        b as u32 + 2 + (p != 0) as u32
    }
}

/// The line as one decode sees it: levels (codes), polarity, the bit time
/// (samples, Q8) and the format.
#[derive(Clone, Copy)]
pub struct Line {
    pub lo: u16,
    pub mid: u16,
    pub hi: u16,
    pub inv: bool,
    pub spb: u32,
    pub bits: u8,
    pub parity: u8,
}

impl Line {
    /// Character time, samples.
    pub fn char_samples(&self) -> u32 {
        (self.spb * (self.bits as u32 + 2 + (self.parity != 0) as u32)) >> 8
    }
    #[inline(always)]
    fn level(&self, v: u16, was: bool) -> bool {
        let high = if v > self.hi {
            true
        } else if v < self.lo {
            false
        } else {
            return was;
        };
        high != self.inv
    }
}

/// Levels from the window's min / max (None: no signal), or around a
/// manual code.
pub fn levels(min: u16, max: u16, manual: Option<u16>) -> Option<(u16, u16, u16)> {
    if let Some(c) = manual {
        return Some((c.saturating_sub(HYST_MANUAL), c, c + HYST_MANUAL));
    }
    if max < min + MIN_SWING {
        return None;
    }
    let s = max - min;
    let mid = min + s / 2;
    Some((mid - s / 6, mid, mid + s / 6))
}

/// Smallest and largest code of `s[a..b]` (a plain loop; `Codes` has a
/// faster `min_max`).
pub fn min_max<S: Src + ?Sized>(s: &S, a: usize, b: usize) -> (u16, u16) {
    let (mut lo, mut hi) = (u16::MAX, 0u16);
    for i in a..b {
        let v = s.at(i) & 0x3FF;
        lo = lo.min(v);
        hi = hi.max(v);
    }
    (lo, hi)
}

/// Why auto-baud found no rate.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum NoRate {
    /// Too few edges (an idle line, one character).
    Few,
    /// The bits are shorter than `MIN_SPB` samples (most runs under
    /// RUN_MIN, or the rate found too fast for the sample rate).
    TooFast,
}

/// Auto-baud over `s[a..b]` at `fs`: the bit rate (snapped to a standard
/// one within 8 %).
pub fn auto_baud<S: Src + ?Sized>(s: &S, a: usize, b: usize, lo: u16, hi: u16, fs: u32) -> Result<u32, NoRate> {
    let mut m = Baud::new(s, a, lo, hi);
    m.step(s, b);
    m.result(fs)
}

/// Auto-baud in slices (`step` up to a sample, then `result`): the bit-
/// length runs between edges, the KRUN shortest kept.
#[derive(Clone, Copy)]
pub struct Baud {
    l: Line,
    runs: [u32; KRUN],
    mark: bool,
    edge: Option<usize>,
    nrun: u32,
    nshort: u32,
    /// The next sample.
    pub n: usize,
}

impl Baud {
    pub fn new<S: Src + ?Sized>(s: &S, a: usize, lo: u16, hi: u16) -> Baud {
        let l = Line { lo, mid: 0, hi, inv: false, spb: 0, bits: 8, parity: 0 };
        Baud { l, runs: [u32::MAX; KRUN], mark: s.at(a) & 0x3FF > hi, edge: None, nrun: 0, nshort: 0, n: a }
    }

    /// Enough runs seen: `result` needs no more samples.
    pub fn enough(&self) -> bool {
        self.nrun >= 32
    }

    /// The samples up to `b` (stops early once `enough`).
    pub fn step<S: Src + ?Sized>(&mut self, s: &S, b: usize) {
        let mut i = self.n;
        while i < b && !self.enough() {
            let m = self.l.level(s.at(i) & 0x3FF, self.mark);
            if m != self.mark {
                self.mark = m;
                if let Some(e) = self.edge {
                    let r = (i - e) as u32;
                    if r < RUN_MIN {
                        self.nshort += 1;
                    } else {
                        self.nrun += 1;
                        // Keep the KRUN shortest (insertion into a sorted list).
                        let runs = &mut self.runs;
                        if r < runs[KRUN - 1] {
                            let mut k = KRUN - 1;
                            while k > 0 && runs[k - 1] > r {
                                runs[k] = runs[k - 1];
                                k -= 1;
                            }
                            runs[k] = r;
                        }
                    }
                }
                self.edge = Some(i);
            }
            i += 1;
        }
        self.n = i;
    }

    /// The rate from the runs seen so far.
    pub fn result(&self, fs: u32) -> Result<u32, NoRate> {
        let (nrun, nshort, runs) = (self.nrun, self.nshort, &self.runs);
        if nshort >= 4 && nshort * 2 > nrun {
            return Err(NoRate::TooFast);
        }
        if nrun < 4 {
            return Err(NoRate::Few);
        }
        let min = runs[0];
        let (mut sum, mut cnt) = (0u64, 0u64);
        for &r in runs.iter().filter(|&&r| r != u32::MAX && r * 10 <= min * 13) {
            sum += r as u64;
            cnt += 1;
        }
        if cnt < 2 {
            return Err(NoRate::Few);
        }
        if sum < MIN_SPB as u64 * cnt {
            return Err(NoRate::TooFast);
        }
        let baud = (fs as u64 * cnt / sum.max(1)) as u32;
        if !(MIN_BAUD * 9 / 10..=MAX_BAUD * 11 / 10).contains(&baud) {
            return Err(if baud > MAX_BAUD { NoRate::TooFast } else { NoRate::Few });
        }
        Ok(STD_BAUD.iter().copied().find(|&s| baud.abs_diff(s) * 100 <= s * 8).unwrap_or(baud))
    }
}

/// A decoded character: start edge and end (the stop bit's end), sample
/// indices of the source; the data; F_FE / F_PE.
#[derive(Clone, Copy)]
pub struct Ch {
    pub start: u32,
    pub end: u32,
    pub v: u16,
    pub err: u8,
}

/// Where a character scan stands (`chars_step`): the next sample, whether
/// a mark level was seen, the scan ended (the window's end or stopped).
#[derive(Clone, Copy)]
pub struct Cursor {
    pub n: usize,
    pub mark: bool,
    pub done: bool,
}

impl Cursor {
    pub const fn at(n: usize) -> Cursor {
        Cursor { n, mark: false, done: false }
    }
}

/// The characters of `s[a..b]`, in order.
pub fn chars(s: &dyn Src, a: usize, b: usize, l: &Line, f: &mut dyn FnMut(Ch)) {
    let mut c = Cursor::at(a);
    chars_step(s, a, b, b, l, &mut c, &mut |ch| {
        f(ch);
        true
    });
}

/// `chars` in slices: from `c.n` until the scan between characters passes
/// `lim` (a character begun before it is finished, up to `b`), then
/// returns with `c` where to go on. `f` returns false to stop the scan
/// (`c.done`). A character that runs past `b` is not reported.
///
/// One copy for every source (`dyn`): the band's record, the log's ring,
/// a snippet; it reads ~3 samples a bit.
#[inline(never)]
pub fn chars_step(s: &dyn Src, a: usize, b: usize, lim: usize, l: &Line, c: &mut Cursor, f: &mut dyn FnMut(Ch) -> bool) {

    if c.done {
        return;
    }
    if b < a + 2 || l.spb < MIN_SPB << 8 {
        c.done = true;
        return;
    }
    let nbits = 1 + l.bits + (l.parity != 0) as u8 + 1;
    let at = |i: usize| s.at(i) & 0x3FF;
    // Between characters every `step`-th sample (a quarter bit: a start bit
    // is never stepped over), then back to the edge's first sample.
    let step = ((l.spb >> 8) as usize / 4).max(1);
    // A start edge counts only after a mark level was seen.
    let mut mark = c.mark;
    let mut n = c.n;
    'idle: while n < b {
        if n >= lim {
            c.n = n;
            c.mark = mark;
            return;
        }
        let v = at(n);
        if !mark {
            mark = l.level(v, false);
            n += step;
            continue;
        }

        if l.level(v, true) {
            n += step;
            continue;
        }
        let lo = (n + 1).saturating_sub(step).max(a);
        if let Some(e) = (lo..n).find(|&i| !l.level(at(i), true)) {
            n = e;
        }
        // The edge lies between n - 1 and n.
        let t0 = ((n as u64) << 8).saturating_sub(128);
        let (mut acc, mut ones, mut err) = (0u16, 0u8, 0u8);
        for k in 0..nbits {
            let ci = ((t0 + ((2 * k as u64 + 1) * l.spb as u64) / 2 + 128) >> 8) as usize;
            if ci + 1 >= b {
                break 'idle; // runs past the window
            }
            let hv = |i: usize| (at(i) >= l.mid) as u8;
            let bit = (hv(ci - 1) + hv(ci) + hv(ci + 1) >= 2) != l.inv;
            if k == 0 {
                if bit {
                    // A glitch, no start bit.
                    n = ci + 1;
                    continue 'idle;
                }
            } else if k <= l.bits {
                acc |= (bit as u16) << (k - 1);
                ones += bit as u8;
            } else if k == l.bits + 1 && l.parity != 0 {
                if ((ones + bit as u8) & 1 == 1) != (l.parity == 2) {
                    err |= F_PE;
                }
            } else {
                if !bit {
                    err |= F_FE;
                }
                mark = bit;
                n = ci + 1;
                let end = ci as u32 + (l.spb >> 9);
                if !f(Ch { start: (t0 >> 8) as u32, end, v: acc, err }) {
                    break 'idle;
                }
                continue 'idle;
            }
        }
    }
    c.n = n;
    c.mark = mark;
    c.done = true;
}

// ---- DALI ----

/// Where a DALI scan stands (`dali_step`): the next sample, the line level
/// (true = high, idle) and its last edge, the frame being collected.
#[derive(Clone, Copy)]
pub struct Dali {
    pub n: usize,
    pub done: bool,
    high: bool,
    edge: usize,
    /// High for more than 2.5 Te: a falling edge starts a frame.
    idle: bool,
    inf: bool,
    start: usize,
    /// Half bits so far (the start bit's two included), the pending bit's
    /// first half.
    halves: u8,
    first: bool,
    acc: u32,
    nb: u8,
    err: bool,
}

impl Dali {
    /// A scan from sample `a` (the line counts as low until it is high).
    pub const fn at(a: usize) -> Dali {
        Dali { n: a, done: false, high: false, edge: a, idle: false, inf: false, start: a, halves: 0, first: false, acc: 0, nb: 0, err: false }
    }

    /// `k` half bits at the level `h`.
    fn half(&mut self, k: u32, h: bool) {
        for _ in 0..k {
            if self.halves & 1 == 0 {
                self.first = h;
            } else if self.first == h || self.nb >= 32 {
                // No transition in the bit's centre (or too long).
                self.err = true;
            } else if self.halves > 1 {
                // A 1: low, then high. (The start bit, the first pair, is
                // a 1: the frame begins with a falling edge.)
                self.acc = self.acc << 1 | h as u32;
                self.nb += 1;
            }
            self.halves = self.halves.wrapping_add(1);
        }
    }
}

/// DALI frames of `s[..b]` from `d.n` (`Dali::at`), as `chars_step` scans:
/// until the line between frames passes `lim` (a frame begun before it is
/// finished, up to `b`); `f` returns false to stop (`d.done`). A frame that
/// runs past `b` is not reported. `l.spb`: the bit time (samples, Q8);
/// 1200 bit/s, every sample read.
#[inline(never)]
pub fn dali_step(s: &dyn Src, b: usize, lim: usize, l: &Line, d: &mut Dali, f: &mut dyn FnMut(&Fr) -> bool) {
    let te = l.spb / 2;
    if d.done || te < MIN_SPB << 8 {
        d.done = true;
        return;
    }
    // Run lengths (samples, Q8) with a sample of slack: a half bit 0.7 ..
    // 1.3 Te, two 1.6 .. 2.4 Te; high longer than 2.5 Te ends a frame.
    let (h0, h1, d0, d1, long) = (te * 7 / 10 - 256, te * 13 / 10 + 256, te * 16 / 10 - 256, te * 24 / 10 + 256, te * 5 / 2);
    let mut i = d.n;
    while i < b {
        if !d.inf && i >= lim {
            d.n = i;
            return;
        }
        let h = l.level(s.at(i) & 0x3FF, d.high);
        let r = ((i - d.edge) as u32).min(0xFF_FFFF) << 8;
        i += 1;
        if h == d.high {
            if r > long {
                if !d.high {
                    d.err |= d.inf;
                } else if d.inf {
                    // The stop bits: a pending first half is completed by
                    // the line going high.
                    d.inf = false;
                    let odd = d.halves & 1;
                    d.half(odd as u32, true);
                    let n = d.nb;
                    let ok = !d.err && matches!(n, 8 | 16 | 24);
                    let len = if ok { n / 8 } else { 0 };
                    let mut head = [0u8; 8];
                    for k in 0..len {
                        head[k as usize] = (d.acc >> (8 * (len - 1 - k))) as u8;
                    }
                    let end = d.edge as u32 + if ok { (te * (4 + odd as u32)) >> 8 } else { 0 };
                    let fr = Fr { start: d.start as u32, end, len: len as u16, head, flags: if ok { 0 } else { F_FE }, kind: K_DALI };
                    if !f(&fr) {
                        d.done = true;
                        d.n = i;
                        return;
                    }
                }
                d.idle = d.high;
            }
            continue;
        }
        let e = i - 1;
        if d.inf {
            if d.halves == 0 && r < te / 2 {
                // A glitch, no start bit.
                d.inf = false;
                d.idle = true;
            } else if (h0..=h1).contains(&r) {
                d.half(1, d.high);
            } else if (d0..=d1).contains(&r) {
                d.half(2, d.high);
            } else {
                d.err = true;
            }
        } else if !h && d.idle {
            *d = Dali { inf: true, start: e, halves: 0, acc: 0, nb: 0, err: false, idle: false, ..*d };
        }
        d.high = h;
        d.edge = e;
    }
    d.n = i;
    d.done = true;
}

/// A DALI frame's text: "FF16:FE 80 BC DAPC" (forward: the address and
/// data bytes, the address byte's target S<n> / G<n> / BC / BCU / SPC
/// (special command) / ?, the selector bit: arc power or a command;
/// `short`: the bytes only), "BF8:FF" (backward), "FF24:A1 B2 C3", "ERR".
pub fn dali_text(w: &mut W, f: &Fr, short: bool) {
    if !DALI {
        return;
    }
    let n = f.len as usize;
    w.s(match n {
        1 => "BF8:",
        2 => "FF16:",
        3 => "FF24:",
        _ => {
            w.s("ERR");
            return;
        }
    });
    for (k, &v) in f.head[..n].iter().enumerate() {
        if k > 0 {
            w.c(b' ');
        }
        w.hex(v);
    }
    if n != 2 || short {
        return;
    }
    let a = f.head[0];
    w.c(b' ');
    match a {
        0..=0x7F => {
            w.c(b'S').u(a as u32 >> 1);
        }
        0x80..=0x9F => {
            w.c(b'G').u(a as u32 >> 1 & 15);
        }
        0xA0..=0xCB => {
            w.s("SPC");
            return;
        }
        0xFC | 0xFD => {
            w.s("BCU");
        }
        0xFE | 0xFF => {
            w.s("BC");
        }
        _ => {
            w.c(b'?');
            return;
        }
    }
    w.s(if a & 1 == 0 { " DAPC" } else { " CMD" });
}

/// Modbus CRC-16 (poly 0xA001, init 0xFFFF), one byte.
#[inline(always)]
pub fn crc16_byte(mut c: u16, b: u8) -> u16 {
    c ^= b as u16;
    for _ in 0..8 {
        c = if c & 1 != 0 { (c >> 1) ^ 0xA001 } else { c >> 1 };
    }
    c
}

pub fn crc16(d: &[u8]) -> u16 {
    d.iter().fold(0xFFFF, |c, &b| crc16_byte(c, b))
}

/// A Modbus frame (or a UART group) as the framer reports it.
#[derive(Clone, Copy)]
pub struct Fr {
    pub start: u32,
    pub end: u32,
    pub len: u16,
    /// The first bytes (`mb_info`, the list's text).
    pub head: [u8; 8],
    pub flags: u8,
    pub kind: u8,
}

/// A frame's Modbus shape from its first bytes and length: (request-shaped,
/// reply-shaped) for the common functions.
fn shape(d: &[u8; 8], n: usize) -> (bool, bool) {
    if n < 4 {
        return (false, false);
    }
    let f = d[1];
    if f & 0x80 != 0 {
        return (false, n == 5);
    }
    match f {
        1..=4 => (n == 8, n >= 6 && n == 5 + d[2] as usize),
        5 | 6 => (n == 8, n == 8),
        15 | 16 => (n >= 10 && n == 9 + d[6] as usize, n == 8),
        _ => (false, false),
    }
}

/// Groups characters into frames: Modbus RTU on the 3.5-character gap, or
/// UART lines (`max` bytes, a gap of 2 characters). Small: the CRC runs
/// two bytes behind the input, no frame buffer.
pub struct Framer {
    modbus: bool,
    gap: u32,
    max: u16,
    /// Window start / end (samples): frames near them are partial.
    lo: u32,
    hi: u32,
    cur: Fr,
    crc: u16,
    last2: [u8; 2],
    /// The previous frame's end (the first frame: the window start).
    prev_end: u32,
    seen_gap: bool,
    /// The request waiting for its reply: (index of the frame, addr, func).
    pend: Option<(u32, u8, u8)>,
    pub nframes: u32,
}

impl Framer {
    /// `cs`: the character time, samples; `fs` the sample rate.
    pub fn new(modbus: bool, cs: u32, fs: u32, baud: u32, lo: u32, hi: u32) -> Framer {
        let modbus = MODBUS && modbus;
        let gap = if !modbus {
            cs * 2
        } else if baud > 19200 {
            (fs as u64 * 1750 / 1_000_000) as u32
        } else {
            cs * 7 / 2
        };
        let cur = Fr { start: 0, end: 0, len: 0, head: [0; 8], flags: 0, kind: K_UART };
        Framer { modbus, gap, max: if modbus { 256 } else { 16 }, lo, hi, cur, crc: 0xFFFF, last2: [0; 2], prev_end: lo, seen_gap: false, pend: None, nframes: 0 }
    }

    pub fn push(&mut self, c: Ch, out: &mut dyn FnMut(&Fr, u32)) {
        if self.cur.len > 0 && (c.start.saturating_sub(self.cur.end) > self.gap || self.cur.len >= self.max) {
            self.end(out, false);
        }
        if self.cur.len == 0 {
            self.seen_gap = c.start.saturating_sub(self.prev_end) > self.gap;
            self.cur = Fr { start: c.start, end: c.end, len: 0, head: [0; 8], flags: 0, kind: K_UART };
            self.crc = 0xFFFF;
        }
        let b = c.v as u8;
        let k = self.cur.len as usize;
        if k < 8 {
            self.cur.head[k] = b;
        }
        if k >= 2 {
            self.crc = crc16_byte(self.crc, self.last2[0]);
        }
        self.last2 = [self.last2[1], b];
        self.cur.len += 1;
        self.cur.end = c.end;
        self.cur.flags |= c.err;
    }

    /// Bytes in the frame being collected (1: the character just pushed
    /// began a frame).
    pub fn open_len(&self) -> u16 {
        self.cur.len
    }

    /// A stream: the line is idle up to sample `now`; a frame whose gap
    /// has passed ends (complete).
    pub fn idle(&mut self, now: u32, out: &mut dyn FnMut(&Fr, u32)) {
        if self.cur.len > 0 && now.saturating_sub(self.cur.end) > self.gap {
            self.end(out, false);
        }
    }

    /// A stream: positions move down by `d` (the caller's sample numbering
    /// was rebased).
    pub fn shift(&mut self, d: u32) {
        self.lo = self.lo.saturating_sub(d);
        self.hi = self.hi.saturating_sub(d);
        self.prev_end = self.prev_end.saturating_sub(d);
        self.cur.start = self.cur.start.saturating_sub(d);
        self.cur.end = self.cur.end.saturating_sub(d);
    }

    /// The window ended: the last frame (partial when it may go on).

    pub fn finish(&mut self, out: &mut dyn FnMut(&Fr, u32)) {
        if self.cur.len > 0 {
            let open = self.hi.saturating_sub(self.cur.end) <= self.gap;
            self.end(out, open);
        }
    }

    fn end(&mut self, out: &mut dyn FnMut(&Fr, u32), open: bool) {
        let mut f = self.cur;
        self.cur.len = 0;
        self.prev_end = f.end;
        if MODBUS && self.modbus {
            let n = f.len as usize;
            let part = open || (!self.seen_gap && f.start.saturating_sub(self.lo) <= self.gap);
            if part {
                f.flags |= F_PART;
                f.kind = K_MB;
            } else if n < 4 || self.crc != (self.last2[0] as u16 | (self.last2[1] as u16) << 8) {
                f.flags |= F_CRC;
                f.kind = K_MB;
            } else {
                let (rq, rs) = shape(&f.head, n);
                let (a, fc) = (f.head[0], f.head[1]);
                let matches = self.pend.is_some_and(|(_, pa, pf)| pa == a && pf == fc & 0x7F);
                f.kind = if fc & 0x80 != 0 && rs {
                    K_EXC
                } else {
                    match (rq, rs) {
                        (true, false) => K_REQ,
                        (false, true) => K_RSP,
                        (true, true) if matches => K_RSP,
                        (true, true) => K_REQ,
                        _ if matches => K_RSP,
                        _ => K_MB,
                    }
                };
            }
            let req = match f.kind {
                K_RSP | K_EXC => self.pend.take().map_or(u32::MAX, |p| p.0),
                _ => u32::MAX,
            };
            if f.kind == K_REQ {
                self.pend = (f.head[0] != 0).then_some((self.nframes, f.head[0], f.head[1]));
            } else if f.kind == K_MB {
                self.pend = None;
            }
            out(&f, req);
        } else {
            if open {
                f.flags |= F_PART;
            }
            out(&f, u32::MAX);
        }
        self.nframes += 1;
    }
}

/// Modbus fields of a frame.
pub struct MbInfo {
    pub addr: u8,
    pub func: u8,
    /// Request / 05 / 06 / 15 / 16 reply: register / coil; else u16::MAX.
    pub reg: u16,
    /// Request: count or value (05 / 06); reply 01..04: the byte count.
    pub n: u16,
    pub exc: u8,
}

pub fn mb_info(d: &[u8; 8], kind: u8) -> MbInfo {
    let g = |i: usize| (d[i] as u16) << 8 | d[i + 1] as u16;
    let mut m = MbInfo { addr: d[0], func: d[1], reg: u16::MAX, n: 0, exc: 0 };
    match kind {
        K_EXC => m.exc = d[2],
        K_REQ => {
            m.reg = g(2);
            m.n = g(4);
        }
        K_RSP if (1..=4).contains(&m.func) => m.n = d[2] as u16,
        K_RSP => {
            m.reg = g(2);
            m.n = g(4);
        }
        _ => {}
    }
    m
}

// ---- text ----

/// A small text writer over a byte slice (cut at its end).
pub struct W<'a> {
    pub b: &'a mut [u8],
    pub n: usize,
}

impl W<'_> {
    pub fn s(&mut self, s: &str) -> &mut Self {
        for &c in s.as_bytes() {
            self.c(c);
        }
        self
    }
    pub fn c(&mut self, c: u8) -> &mut Self {
        if self.n < self.b.len() {
            self.b[self.n] = c;
            self.n += 1;
        }
        self
    }
    pub fn hex(&mut self, v: u8) -> &mut Self {
        const H: &[u8; 16] = b"0123456789ABCDEF";
        self.c(H[(v >> 4) as usize]).c(H[(v & 15) as usize])
    }
    pub fn hex16(&mut self, v: u16) -> &mut Self {
        self.hex((v >> 8) as u8).hex(v as u8)
    }
    pub fn u(&mut self, v: u32) -> &mut Self {
        let mut t = [0u8; 10];
        let mut i = t.len();
        let mut x = v;
        loop {
            i -= 1;
            t[i] = b'0' + (x % 10) as u8;
            x /= 10;
            if x == 0 {
                break;
            }
        }
        for k in i..t.len() {
            self.c(t[k]);
        }
        self
    }
    /// Signed seconds from `ns` nanoseconds, 9 decimals: "-0.000123456".
    pub fn secs_ns(&mut self, ns: i64) -> &mut Self {
        if ns < 0 {
            self.c(b'-');
        }
        let a = ns.unsigned_abs();
        self.u((a / 1_000_000_000) as u32).c(b'.');
        let mut f = a % 1_000_000_000;
        let mut d = 100_000_000;
        for _ in 0..9 {
            self.c(b'0' + (f / d) as u8);
            f %= d;
            d /= 10;
        }
        self
    }
    pub fn as_str(&self) -> &str {
        core::str::from_utf8(&self.b[..self.n]).unwrap_or("")
    }
}

/// Status word of a frame: OK, FE, PE, CRC or PART (the worst first).
pub fn status(flags: u8) -> &'static str {
    if flags & F_FE != 0 {
        "FE"
    } else if flags & F_PE != 0 {
        "PE"
    } else if flags & F_CRC != 0 {
        "CRC"
    } else if flags & F_PART != 0 {
        "PART"
    } else {
        "OK"
    }
}

/// Modbus fields as text: "REQ 01 03 @0000 n=10", "RSP 01 03 20B",
/// "EXC 01 83 E02", "RSP 01 06 @0005 =1234"; "?" without fields.
pub fn mb_text(w: &mut W, f: &Fr) {
    if !MODBUS {
        return;
    }
    if !matches!(f.kind, K_REQ | K_RSP | K_EXC) {
        w.s("?");
        return;
    }
    let m = mb_info(&f.head, f.kind);
    w.s(match f.kind {
        K_REQ => "REQ ",
        K_RSP => "RSP ",
        _ => "EXC ",
    });
    w.hex(m.addr).c(b' ').hex(m.func);
    match f.kind {
        K_EXC => {
            w.s(" E").hex(m.exc);
        }
        K_RSP if (1..=4).contains(&m.func) => {
            w.c(b' ').u(m.n as u32).c(b'B');
        }
        _ => {
            w.s(" @").hex16(m.reg);
            if matches!(m.func, 5 | 6) {
                w.s(" =").hex16(m.n);
            } else {
                w.s(" n=").u(m.n as u32);
            }
        }
    }
}
