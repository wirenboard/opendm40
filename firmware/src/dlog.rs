//! Compact binary logs (DLG, docs/compact-log-format.md): the coder behind
//! the DMM recording (csvlog.rs writes LOGnnn.DLG instead of a CSV) and the
//! CSV generator that turns a DLG file back into the exact CSV the old
//! writer produced (virtual LOGnnn.CSV files on the read-only LUN 1,
//! MMEM:DATA?, the gallery).
//!
//! File = file header + chunks. One chunk per csvlog commit; each chunk is
//! self-contained (the coder restarts), CRC-32 protected and carries its first
//! row number, its CSV byte offset and its CSV length, so a power cut loses
//! only the chunk being written and the CSV export can seek without decoding.
//!
//!   file header (24 + hl bytes): "DLG1", kind (1 DMM, 2 scope record, 3 fast
//!       logger stream: section "fast log" below), unit
//!       length, u16 hl, u32 CRC-32 of bytes 12.., unit[12], the CSV header text
//!   chunk (28 + pl bytes): "DC", u16 pl, u32 CRC-32 of bytes 8.., u32 first
//!       row, u32 CSV offset, u32 CSV bytes, u32 t0 ms, u16 rows, u8 flags at
//!       the start, u8 0, pl bytes of bit stream (MSB first)
//!
//! Rows: time = adaptive Rice of the zigzag delta-of-delta (ms); value = the
//! 6-significant-digit decimal the old writer printed, as an integer at the
//! chunk's decimal scale, adaptive Rice of the zigzag delta + 1 (0 = escape:
//! empty value / new flags / new scale / "-0"). Rows with OL code one bit
//! (0 = unchanged, 1 = new flags follow). Exactness: the value is stored as
//! the printed decimal (`dec6`), never as a float, and printed by the same
//! `fmt_row` on both sides.
//!
//! Pure code (no hardware): the coder, `Gen` over any `Src`, `FatFile` over
//! any `Raw` byte reader (FAT16 chain walking) and the root scan `scan`.
//! Device glue (the SPI reader, MMEM:DATA?, the refresh flag of vdisk.rs) at the end.
//! Host tests: tools/dlog-test (round trip vs the old writer, truncation at
//! every byte), tools/fatren-test (csvlog on a FAT image, fsck, dlog.py).
//!
//! Placement: the module is in .cold (region C, link-native.x) except the
//! row coder that runs on every reading of a recording (`Enc::row`, BitW,
//! Ad, fmt_row's CSV length, crc32_upd): `.text.dlog_row`, region B
//! (emulator hooks 2026-10-22: the only dlog code that ran per reading).

#![allow(dead_code)]

pub const MAGIC: [u8; 4] = *b"DLG1";
pub const KIND_DMM: u8 = 1;
pub const KIND_SCOPE: u8 = 2;
/// File header bytes before the header text.
pub const FHDR: usize = 24;
/// Chunk header bytes.
pub const CHDR: usize = 28;
pub const UNIT_MAX: usize = 12;
/// Flag bits (csvlog F_*), GAP = rows were dropped just before this one.
pub const F_OL: u8 = 1;
pub const F_GAP: u8 = 32;
const FLAG_TXT: [(u8, &str); 6] = [(1, "OL"), (2, "REL"), (4, "HOLD"), (8, "HI"), (16, "LO"), (32, "GAP")];
/// Longest CSV row (time, value, unit, every flag).
pub const ROW_MAX: usize = 96;
/// Bytes one `Enc::row` may emit (worst case ~22).
pub const ROW_BYTES: usize = 32;
/// Rice: unary prefix length that switches to a raw 32-bit value.
pub(crate) const QMAX: u32 = 24;

// ------------------------------------------------------------- CRC-32 (zlib)

#[cfg_attr(target_os = "none", link_section = ".text.dlog_row")]
pub fn crc32_upd(mut c: u32, d: &[u8]) -> u32 {
    const T: [u32; 16] = {
        let mut t = [0u32; 16];
        let mut i = 0;
        while i < 16 {
            let mut c = i as u32;
            let mut k = 0;
            while k < 4 {
                c = (c >> 1) ^ (0xEDB8_8320 & (c & 1).wrapping_neg());
                k += 1;
            }
            t[i] = c;
            i += 1;
        }
        t
    };
    for &b in d {
        c ^= b as u32;
        c = (c >> 4) ^ T[(c & 15) as usize];
        c = (c >> 4) ^ T[(c & 15) as usize];
    }
    c
}

// inline(always): the kind-3 read path (`.text.vdisk_rd`, region B) uses them.
#[inline(always)]
fn le16(b: &[u8], o: usize) -> u32 {
    b[o] as u32 | (b[o + 1] as u32) << 8
}
#[inline(always)]
fn le32(b: &[u8], o: usize) -> u32 {
    le16(b, o) | le16(b, o + 2) << 16
}

// ------------------------------------------------------------- values / rows

/// A CSV value as printed: empty, "-0", or 6 significant digits
/// (m = 100000..999999, value = m * 10^(e-5); m = 0 prints "0").
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Val {
    Empty,
    NegZero,
    Num { neg: bool, m: u32, e: i32 },
}

const P10_MIN: i32 = -30;
const P10_MAX: i32 = 30;
const P10: [f32; 61] = [
    1e-30, 1e-29, 1e-28, 1e-27, 1e-26, 1e-25, 1e-24, 1e-23, 1e-22, 1e-21, 1e-20, 1e-19, 1e-18, 1e-17, 1e-16, 1e-15, 1e-14, 1e-13,
    1e-12, 1e-11, 1e-10, 1e-9, 1e-8, 1e-7, 1e-6, 1e-5, 1e-4, 1e-3, 1e-2, 1e-1, 1e0, 1e1, 1e2, 1e3, 1e4, 1e5, 1e6, 1e7, 1e8, 1e9,
    1e10, 1e11, 1e12, 1e13, 1e14, 1e15, 1e16, 1e17, 1e18, 1e19, 1e20, 1e21, 1e22, 1e23, 1e24, 1e25, 1e26, 1e27, 1e28, 1e29, 1e30,
];

fn p10(e: i32) -> f32 {
    P10[(e.clamp(P10_MIN, P10_MAX) - P10_MIN) as usize]
}

/// The value csvlog prints for `v` (the old `W::num`): empty unless finite.
pub fn dec6(v: f32) -> Val {
    if !v.is_finite() {
        return Val::Empty;
    }
    let neg = v < 0.0;
    let x = if neg { -v } else { v };
    if x < 1e-30 {
        return if neg { Val::NegZero } else { Val::Num { neg: false, m: 0, e: 0 } };
    }
    // Decimal exponent: p10(e) <= x < p10(e + 1); one rounding step for
    // the mantissa (m = 100000..999999).
    let mut e = P10_MIN;
    while e < P10_MAX && p10(e + 1) <= x {
        e += 1;
    }
    let f = if e - 5 >= P10_MIN { x / p10(e - 5) } else { x / p10(e) * 1e5 };
    let mut m = (f + 0.5) as u32;
    if m >= 1_000_000 {
        m /= 10;
        e += 1;
    }
    Val::Num { neg, m: m.clamp(100_000, 999_999), e }
}

impl Val {
    /// As f32 (gallery statistics); None when empty.
    pub fn to_f32(self) -> Option<f32> {
        match self {
            Val::Empty => None,
            Val::NegZero => Some(0.0),
            Val::Num { neg, m, e } => {
                let a = if e - 5 >= P10_MIN { m as f32 * p10(e - 5) } else { m as f32 * p10(e) * 1e-5 };
                Some(if neg { -a } else { a })
            }
        }
    }
}

/// A tiny byte writer (silently truncates).
pub struct Txt<'a> {
    pub b: &'a mut [u8],
    pub n: usize,
}

impl Txt<'_> {
    pub fn c(&mut self, c: u8) {
        if self.n < self.b.len() {
            self.b[self.n] = c;
            self.n += 1;
        }
    }
    #[cfg_attr(target_os = "none", link_section = ".text.dlog_row")]
    pub fn bytes(&mut self, s: &[u8]) {
        for &c in s {
            self.c(c);
        }
    }
    pub fn s(&mut self, s: &str) {
        self.bytes(s.as_bytes());
    }
    #[cfg_attr(target_os = "none", link_section = ".text.dlog_row")]
    pub fn u(&mut self, v: u64) {
        let mut d = [0u8; 20];
        let mut k = 0;
        let mut x = v;
        loop {
            d[k] = b'0' + (x % 10) as u8;
            k += 1;
            x /= 10;
            if x == 0 {
                break;
            }
        }
        while k > 0 {
            k -= 1;
            self.c(d[k]);
        }
    }
    /// Milliseconds as seconds with three decimals.
    pub fn ms(&mut self, ms: u64) {
        self.u(ms / 1000);
        self.c(b'.');
        let r = (ms % 1000) as u32;
        self.c(b'0' + (r / 100) as u8);
        self.c(b'0' + (r / 10 % 10) as u8);
        self.c(b'0' + (r % 10) as u8);
    }
    /// A value as csvlog prints it: plain for 1e-3 <= |v| < 1e6, else
    /// d.dddddE+xx.
    pub fn val(&mut self, v: Val) {
        let (neg, m, e) = match v {
            Val::Empty => return,
            Val::NegZero => {
                self.s("-0");
                return;
            }
            Val::Num { neg, m, e } => (neg, m, e),
        };
        if neg {
            self.c(b'-');
        }
        if m == 0 {
            self.c(b'0');
            return;
        }
        let mut dg = [0u8; 6];
        let mut t = m;
        for i in (0..6).rev() {
            dg[i] = b'0' + (t % 10) as u8;
            t /= 10;
        }
        if (-3..6).contains(&e) {
            if e < 0 {
                self.s("0.");
                for _ in 0..(-e - 1) {
                    self.c(b'0');
                }
                self.bytes(&dg);
            } else {
                let ip = (e + 1) as usize;
                self.bytes(&dg[..ip]);
                if ip < 6 {
                    self.c(b'.');
                    self.bytes(&dg[ip..]);
                }
            }
        } else {
            self.c(dg[0]);
            self.c(b'.');
            self.bytes(&dg[1..]);
            self.c(b'E');
            self.c(if e < 0 { b'-' } else { b'+' });
            let a = e.unsigned_abs();
            if a < 10 {
                self.c(b'0');
            }
            self.u(a as u64);
        }
    }
}

/// One CSV row `time_s,value,unit,flags\r\n` (the csvlog format); returns
/// its length. An OL row prints no value.
#[cfg_attr(target_os = "none", link_section = ".text.dlog_row")]
pub fn fmt_row(t_ms: u64, v: Val, unit: &[u8], flags: u8, out: &mut [u8]) -> usize {
    let mut w = Txt { b: out, n: 0 };
    w.ms(t_ms);
    w.c(b',');
    if flags & F_OL == 0 {
        w.val(v);
    }
    w.c(b',');
    w.bytes(unit);
    w.c(b',');
    let mut sep = false;
    for (bit, t) in FLAG_TXT {
        if flags & bit != 0 {
            if sep {
                w.c(b' ');
            }
            w.s(t);
            sep = true;
        }
    }
    w.s("\r\n");
    w.n
}

/// `v` as an integer at decimal scale k (value = n * 10^k), when exact and
/// at most one decade above the scale (|n| < 10^7).
#[cfg_attr(target_os = "none", link_section = ".text.dlog_row")]
fn at_scale(m: u32, e: i32, neg: bool, k: i32) -> Option<i32> {
    if m == 0 {
        return Some(0);
    }
    let n = match e - 5 - k {
        0 => m as i32,
        1 => m as i32 * 10,
        _ => return None,
    };
    Some(if neg { -n } else { n })
}

#[cfg_attr(target_os = "none", link_section = ".text.vdisk_rd")]
fn from_scale(n: i32, k: i32) -> Val {
    if n == 0 {
        return Val::Num { neg: false, m: 0, e: 0 };
    }
    let mut a = n.unsigned_abs();
    let mut d = 1i32;
    let mut t = a;
    while t >= 10 {
        t /= 10;
        d += 1;
    }
    let e = k + d - 1;
    while d > 6 {
        a /= 10;
        d -= 1;
    }
    while d < 6 {
        a = a.saturating_mul(10);
        d += 1;
    }
    Val::Num { neg: n < 0, m: a, e }
}

pub(crate) fn zz(x: i32) -> u32 {
    ((x << 1) ^ (x >> 31)) as u32
}
#[cfg_attr(target_os = "none", link_section = ".text.vdisk_rd")]
pub(crate) fn unzz(u: u32) -> i32 {
    ((u >> 1) as i32) ^ -((u & 1) as i32)
}

/// LOCO-I style adaptive Rice parameter.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Ad {
    pub(crate) a: u32,
    pub(crate) n: u32,
}

impl Ad {
    pub(crate) const fn new(a: u32) -> Ad {
        Ad { a, n: 1 }
    }
    #[cfg_attr(target_os = "none", link_section = ".text.dlog_row")]
    pub(crate) fn k(&self) -> u32 {
        // The least k with n << k >= a (a < 2^29, so k < 30): from the bit
        // lengths, n << k0 >= 2^(len(a - 1) - 1) and n << (k0 - 1) < a.
        if self.a <= self.n {
            return 0;
        }
        let k0 = (self.n.leading_zeros() - (self.a - 1).leading_zeros()).min(30);
        if k0 < 30 && (self.n << k0) < self.a { k0 + 1 } else { k0 }
    }
    #[cfg_attr(target_os = "none", link_section = ".text.dlog_row")]
    pub(crate) fn upd(&mut self, x: u32) {
        self.a = self.a.saturating_add(x.min(1 << 24));
        self.n += 1;
        if self.n >= 32 {
            self.a = (self.a + 1) >> 1;
            self.n >>= 1;
        }
    }
}

const AT0: u32 = 4;
const AV0: u32 = 16;

// ------------------------------------------------------------- encoder

/// Bit writer into a byte slice; whole bytes go out, the rest stays in `acc`.
struct BitW<'a> {
    acc: u64,
    nb: u32,
    out: &'a mut [u8],
    n: usize,
}

impl BitW<'_> {
    #[cfg_attr(target_os = "none", link_section = ".text.dlog_row")]
    fn put(&mut self, v: u32, bits: u32) {
        if bits == 0 {
            return;
        }
        let v = if bits >= 32 { v as u64 } else { (v & ((1u32 << bits) - 1)) as u64 };
        self.acc = (self.acc << bits) | v;
        self.nb += bits;
        while self.nb >= 8 {
            self.nb -= 8;
            if self.n < self.out.len() {
                self.out[self.n] = (self.acc >> self.nb) as u8;
                self.n += 1;
            }
        }
        self.acc &= (1u64 << self.nb) - 1;
    }
    #[cfg_attr(target_os = "none", link_section = ".text.dlog_row")]
    fn ones(&mut self, mut q: u32) {
        while q > 0 {
            let k = q.min(16);
            self.put((1 << k) - 1, k);
            q -= k;
        }
    }
    #[cfg_attr(target_os = "none", link_section = ".text.dlog_row")]
    fn rice(&mut self, x: u32, k: u32) {
        let q = x >> k;
        if q < QMAX {
            self.ones(q);
            self.put(0, 1);
            self.put(x, k);
        } else {
            self.ones(QMAX);
            self.put(x, 32);
        }
    }
}

/// Chunk encoder state (Copy: csvlog snapshots it to make a row all or
/// nothing). Counters over the whole file: `rows`, `csv` (CSV bytes so far,
/// header text included).
#[derive(Clone, Copy, Debug)]
pub struct Enc {
    acc: u64,
    nb: u32,
    pub open: bool,
    // chunk
    row0: u32,
    csv_base: u32,
    csv_len: u32,
    t0: u32,
    pub nrows: u16,
    flags0: u8,
    /// Payload bytes emitted (whole bytes).
    pub pl: u32,
    // coder
    t_prev: u32,
    dt_prev: u32,
    flags: u8,
    has_k: bool,
    k: i32,
    pv: i32,
    at: Ad,
    av: Ad,
    // file
    pub rows: u32,
    pub csv: u32,
}

pub const ENC0: Enc = Enc {
    acc: 0,
    nb: 0,
    open: false,
    row0: 0,
    csv_base: 0,
    csv_len: 0,
    t0: 0,
    nrows: 0,
    flags0: 0,
    pl: 0,
    t_prev: 0,
    dt_prev: 0,
    flags: 0,
    has_k: false,
    k: 0,
    pv: 0,
    at: Ad::new(AT0),
    av: Ad::new(AV0),
    rows: 0,
    csv: 0,
};
/// All zero bits: the placeholder of a zero-initialised (.bss) static that
/// is set to ENC0 before use (csvlog.rs JOB).
pub const ENC_ZERO: Enc = Enc { at: Ad { a: 0, n: 0 }, av: Ad { a: 0, n: 0 }, ..ENC0 };

impl Enc {
    /// A new file whose CSV header text is `hl` bytes.
    pub fn new(hl: usize) -> Enc {
        Enc { csv: hl as u32, ..ENC0 }
    }

    /// Code one row (`t_ms` since the start, the printed value, flags incl.
    /// GAP, `csv_len` = bytes of its CSV text). Whole bytes go to `out`
    /// (returned count, <= ROW_BYTES); a chunk opens with its first row.
    /// The caller writes the CHDR-byte chunk header placeholder before the
    /// first row's bytes (`open` false before the call).
    #[cfg_attr(target_os = "none", link_section = ".text.dlog_row")]
    pub fn row(&mut self, t_ms: u32, v: Val, flags: u8, csv_len: u32, out: &mut [u8]) -> usize {
        if !self.open {
            self.open = true;
            self.row0 = self.rows;
            self.csv_base = self.csv;
            self.csv_len = 0;
            self.t0 = t_ms;
            self.nrows = 0;
            self.flags0 = flags;
            self.pl = 0;
            self.t_prev = t_ms;
            self.dt_prev = 0;
            self.flags = flags;
            self.has_k = false;
            self.k = 0;
            self.pv = 0;
            self.at = Ad::new(AT0);
            self.av = Ad::new(AV0);
            self.acc = 0;
            self.nb = 0;
        }
        let mut w = BitW { acc: self.acc, nb: self.nb, out, n: 0 };
        let dt = t_ms.wrapping_sub(self.t_prev);
        let x = zz(dt.wrapping_sub(self.dt_prev) as i32);
        w.rice(x, self.at.k());
        self.at.upd(x);
        self.t_prev = t_ms;
        self.dt_prev = dt;
        loop {
            if self.flags & F_OL != 0 {
                if flags == self.flags {
                    w.put(0, 1);
                    break;
                }
                w.put(1, 1);
                w.put(flags as u32, 6);
                self.flags = flags;
                continue;
            }
            let kv = self.av.k();
            if flags != self.flags {
                w.rice(0, kv);
                w.put(1, 2);
                w.put(flags as u32, 6);
                self.flags = flags;
                continue;
            }
            match v {
                Val::Empty => {
                    w.rice(0, kv);
                    w.put(0, 2);
                }
                Val::NegZero => {
                    w.rice(0, kv);
                    w.put(3, 2);
                }
                Val::Num { neg, m, e } => {
                    let n = if self.has_k { at_scale(m, e, neg, self.k) } else { None };
                    match n {
                        Some(n) => {
                            let u = zz(n.wrapping_sub(self.pv)) + 1;
                            w.rice(u, kv);
                            self.av.upd(u);
                            self.pv = n;
                        }
                        None => {
                            let k = if m == 0 { 0 } else { e - 5 };
                            let n = at_scale(m, e, neg, k).unwrap_or(0);
                            w.rice(0, kv);
                            w.put(2, 2);
                            w.put(k as u8 as u32, 8);
                            w.put((n < 0) as u32, 1);
                            w.put(n.unsigned_abs(), 24);
                            self.has_k = true;
                            self.k = k;
                            self.pv = n;
                        }
                    }
                }
            }
            break;
        }
        self.acc = w.acc;
        self.nb = w.nb;
        self.pl += w.n as u32;
        self.nrows += 1;
        self.csv_len += csv_len;
        self.rows += 1;
        self.csv += csv_len;
        w.n
    }

    /// Close the chunk: the padded last byte (0 or 1 bytes into `out`).
    /// Then `header` gives its header (with the CRC of the payload).
    pub fn close(&mut self, out: &mut [u8]) -> usize {
        let mut n = 0;
        if self.nb > 0 {
            out[0] = (self.acc << (8 - self.nb)) as u8;
            n = 1;
        }
        self.acc = 0;
        self.nb = 0;
        self.pl += n as u32;
        self.open = false;
        n
    }

    /// Header of the chunk just closed, without the CRC (bytes 4..8 zero):
    /// the caller sets it to !crc32_upd(crc32_upd(!0, &h[8..]), payload).
    pub fn header(&self) -> [u8; CHDR] {
        let mut h = [0u8; CHDR];
        h[0] = b'D';
        h[1] = b'C';
        h[2..4].copy_from_slice(&(self.pl as u16).to_le_bytes());
        h[8..12].copy_from_slice(&self.row0.to_le_bytes());
        h[12..16].copy_from_slice(&self.csv_base.to_le_bytes());
        h[16..20].copy_from_slice(&self.csv_len.to_le_bytes());
        h[20..24].copy_from_slice(&self.t0.to_le_bytes());
        h[24..26].copy_from_slice(&self.nrows.to_le_bytes());
        h[26] = self.flags0;
        h
    }
}

/// The file header (kind, unit, CSV header text) into `out`; returns its
/// length (FHDR + text).
pub fn file_header(kind: u8, unit: &[u8], text: &[u8], out: &mut [u8]) -> usize {
    let ul = unit.len().min(UNIT_MAX);
    let n = FHDR + text.len();
    out[..FHDR].fill(0);
    out[..4].copy_from_slice(&MAGIC);
    out[4] = kind;
    out[5] = ul as u8;
    out[6..8].copy_from_slice(&(text.len() as u16).to_le_bytes());
    out[12..12 + ul].copy_from_slice(&unit[..ul]);
    out[FHDR..n].copy_from_slice(text);
    let c = !crc32_upd(!0, &out[12..n]);
    out[8..12].copy_from_slice(&c.to_le_bytes());
    n
}

// ------------------------------------------------------------- scope record

/// Kind 2 (scope record): after the header text, SCOPE_PAR bytes of
/// parameters (f32 a, f32 b: volts = a + b * code; f32 dt, s per sample;
/// u32 t0, the sample of time 0; u32 n samples; u32 CRC-32 of these 20),
/// the block table (per SBLK-sample block: u32 coded bytes, u32 CRC-32 of
/// them; then u32 CRC-32 of the table), then the coded blocks. A block:
/// 3 bits mode (0..3 = FLAC-style fixed predictor order, 4 = raw 10-bit
/// codes), then per sample the adaptive Rice code of the zigzag residual
/// (raw: 10 bits), zero-padded to a byte. Every block restarts the
/// predictor (sample j < order uses order j; order 0 predicts 512) and the
/// Rice state, so a block decodes on its own (random access for the CSV).
pub const SCOPE_PAR: usize = 24;
/// Samples per coded block, blocks per record at most (262 144 samples:
/// the scope's coded ring, cring.rs).
pub const SBLK: usize = 4096;
pub const SBLK_MAX: usize = 64;
/// Raw-mode block bytes (the bound of any block).
pub const SBLK_RAW: usize = (3 + SBLK * 10).div_ceil(8);
const S_RAW: u32 = 4;
pub(crate) const AS0: u32 = 16;

/// Fixed predictor of `order` from the previous codes h[0] = x[-1],
/// h[1] = x[-2], h[2] = x[-3] (`j` = index in the block).
#[cfg_attr(target_os = "none", link_section = ".text.vdisk_rd")]
pub(crate) fn s_pred(order: u32, j: usize, h: &[i32; 3]) -> i32 {
    match order.min(j as u32) {
        0 => 512,
        1 => h[0],
        2 => 2 * h[0] - h[1],
        _ => 3 * h[0] - 3 * h[1] + h[2],
    }
}

/// The fixed order (0..3) with the least sum of |residual| over `codes`.
#[cfg_attr(target_os = "none", link_section = ".text.scope_plan")]
pub fn scope_order<S: crate::pack10::Src + ?Sized>(codes: &S) -> u32 {
    let mut sum = [0u64; 4];
    let mut h = [0i32; 3];
    for j in 0..codes.len() {
        let x = (codes.at(j) & 0x3FF) as i32;
        for (o, s) in sum.iter_mut().enumerate() {
            *s += (x - s_pred(o as u32, j, &h)).unsigned_abs() as u64;
        }
        h = [x, h[0], h[1]];
    }
    (0..4).min_by_key(|&o| sum[o]).unwrap_or(1) as u32
}

/// Resumable encoder of one scope block: bytes one at a time (the writer
/// fills a disk block in pieces; a sequential fill never codes a sample
/// twice). `pos` counts the bytes given out (the caller's file offset).
#[derive(Clone, Copy)]
pub struct SEnc {
    pub pos: u32,
    mode: u32,
    j: usize,
    acc: u64,
    nb: u32,
    h: [i32; 3],
    ad: Ad,
    q: [u8; 16],
    qh: u8,
    qt: u8,
    flushed: bool,
}

impl SEnc {
    pub const fn new(mode: u32, pos: u32) -> SEnc {
        let mut e = SEnc { pos, mode, j: 0, acc: 0, nb: 0, h: [0; 3], ad: Ad::new(AS0), q: [0; 16], qh: 0, qt: 0, flushed: false };
        e.acc = mode as u64;
        e.nb = 3;
        e
    }
    #[cfg_attr(target_os = "none", link_section = ".text.scope_plan")]
    fn put(&mut self, v: u32, bits: u32) {
        if bits == 0 {
            return;
        }
        let v = if bits >= 32 { v as u64 } else { (v & ((1u32 << bits) - 1)) as u64 };
        self.acc = (self.acc << bits) | v;
        self.nb += bits;
        while self.nb >= 8 {
            self.nb -= 8;
            self.q[(self.qt & 15) as usize] = (self.acc >> self.nb) as u8;
            self.qt = self.qt.wrapping_add(1);
        }
        self.acc &= (1u64 << self.nb) - 1;
    }
    /// The next byte of the block coding `codes`; None at its end.
    #[cfg_attr(target_os = "none", link_section = ".text.scope_plan")]
    pub fn next<S: crate::pack10::Src + ?Sized>(&mut self, codes: &S) -> Option<u8> {
        loop {
            if self.qh != self.qt {
                let b = self.q[(self.qh & 15) as usize];
                self.qh = self.qh.wrapping_add(1);
                self.pos += 1;
                return Some(b);
            }
            if self.j < codes.len() {
                let x = (codes.at(self.j) & 0x3FF) as i32;
                if self.mode == S_RAW {
                    self.put(x as u32, 10);
                } else {
                    let u = zz(x - s_pred(self.mode, self.j, &self.h));
                    let k = self.ad.k();
                    let q = u >> k;
                    if q < QMAX {
                        let mut r = q;
                        while r > 0 {
                            let n = r.min(16);
                            self.put((1 << n) - 1, n);
                            r -= n;
                        }
                        self.put(0, 1);
                        self.put(u, k);
                    } else {
                        self.put((1 << 16) - 1, 16);
                        self.put((1 << (QMAX - 16)) - 1, QMAX - 16);
                        self.put(u, 32);
                    }
                    self.ad.upd(u);
                }
                self.h = [x, self.h[0], self.h[1]];
                self.j += 1;
                continue;
            }
            if !self.flushed {
                self.flushed = true;
                if self.nb > 0 {
                    let pad = 8 - self.nb;
                    self.put(0, pad);
                }
                continue;
            }
            return None;
        }
    }
}

/// Code one block (`codes`, at most SBLK) in `mode` (0..3 order, 4 raw)
/// into `sink`; returns the byte count.
///
/// Generic and inline(always), `scope_plan` (with SEnc, `.text.scope_plan`,
/// region B) runs at the Save CSV tap, while the Wave view's stream may run:
/// a dyn sink closure would be a symbol of its own in region C.
#[inline(always)]
pub fn scope_block<S: crate::pack10::Src + ?Sized, F: FnMut(u8)>(codes: &S, mode: u32, sink: &mut F) -> usize {
    let mut e = SEnc::new(mode, 0);
    while let Some(b) = e.next(codes) {
        sink(b);
    }
    e.pos as usize
}

/// Choose and size a block's coding: (mode, bytes, CRC-32 of the bytes).
/// The fixed order with the least |residual|, or raw when that is smaller.
#[cfg_attr(target_os = "none", link_section = ".text.scope_plan")]
pub fn scope_plan<S: crate::pack10::Src + ?Sized>(codes: &S) -> (u32, u32, u32) {
    let o = scope_order(codes);
    let mut crc = !0u32;
    let n = scope_block(codes, o, &mut |b| crc = crc32_upd(crc, &[b]));
    let raw = (3 + codes.len() * 10).div_ceil(8);
    if n > raw {
        let mut crc = !0u32;
        let n = scope_block(codes, S_RAW, &mut |b| crc = crc32_upd(crc, &[b]));
        return (S_RAW, n as u32, !crc);
    }
    (o, n as u32, !crc)
}

/// The block table (`lens[i]`, `crcs[i]` per block, then the table CRC)
/// into `out`; returns its length (8 * blocks + 4).
pub fn scope_table(lens: &[u32], crcs: &[u32], out: &mut [u8]) -> usize {
    let mut k = 0;
    for (l, c) in lens.iter().zip(crcs) {
        out[k..k + 4].copy_from_slice(&l.to_le_bytes());
        out[k + 4..k + 8].copy_from_slice(&c.to_le_bytes());
        k += 8;
    }
    let c = !crc32_upd(!0, &out[..k]);
    out[k..k + 4].copy_from_slice(&c.to_le_bytes());
    k + 4
}

/// Decoder of one scope block.
#[derive(Clone, Copy)]
struct SDec {
    pos: u32,
    bit: u32,
    end: u32,
    mode: u32,
    j: usize,
    h: [i32; 3],
    ad: Ad,
}

impl SDec {
    fn new<S: Src>(s: &mut S, w: &mut Win, start: u32, end: u32) -> Option<SDec> {
        let mut d = SDec { pos: start, bit: 0, end, mode: 0, j: 0, h: [0; 3], ad: Ad::new(AS0) };
        d.mode = d.get(s, w, 3)?;
        (d.mode <= S_RAW).then_some(d)
    }
    #[cfg_attr(target_os = "none", link_section = ".text.vdisk_rd")]
    fn get<S: Src>(&mut self, s: &mut S, w: &mut Win, mut bits: u32) -> Option<u32> {
        let mut v = 0u32;
        while bits > 0 {
            let b = w.byte(s, self.pos, self.end)?;
            let take = (8 - self.bit).min(bits);
            let x = (b >> (8 - self.bit - take)) as u32 & ((1u32 << take) - 1);
            v = if take == 32 { x } else { (v << take) | x };
            self.bit += take;
            bits -= take;
            if self.bit == 8 {
                self.bit = 0;
                self.pos += 1;
            }
        }
        Some(v)
    }
    #[cfg_attr(target_os = "none", link_section = ".text.vdisk_rd")]
    fn next<S: Src>(&mut self, s: &mut S, w: &mut Win) -> Option<u16> {
        let x = if self.mode == S_RAW {
            self.get(s, w, 10)? as i32
        } else {
            let k = self.ad.k();
            let mut q = 0;
            while q < QMAX && self.get(s, w, 1)? == 1 {
                q += 1;
            }
            let u = if q == QMAX { self.get(s, w, 32)? } else { (q << k) | self.get(s, w, k)? };
            self.ad.upd(u);
            s_pred(self.mode, self.j, &self.h) + unzz(u)
        };
        self.h = [x, self.h[0], self.h[1]];
        self.j += 1;
        Some((x & 0x3FF) as u16)
    }
}
/// The generated CSV row: "+d.ddddde+xx,+d.dddde+xx\r\n".
pub const SCOPE_ROW: usize = 26;

/// The scope record file header (header text = the CSV's, column line
/// included) into `out`; returns its length (FHDR + text + SCOPE_PAR).
pub fn scope_header(text: &[u8], a: f32, b: f32, dt: f32, t0: u32, n: u32, out: &mut [u8]) -> usize {
    let k = file_header(KIND_SCOPE, b"V", text, out);
    let p = &mut out[k..k + SCOPE_PAR];
    p[0..4].copy_from_slice(&a.to_le_bytes());
    p[4..8].copy_from_slice(&b.to_le_bytes());
    p[8..12].copy_from_slice(&dt.to_le_bytes());
    p[12..16].copy_from_slice(&t0.to_le_bytes());
    p[16..20].copy_from_slice(&n.to_le_bytes());
    let c = !crc32_upd(!0, &p[..20]);
    p[20..24].copy_from_slice(&c.to_le_bytes());
    k + SCOPE_PAR
}

/// "+d.dd..de+xx" with `dig` decimals into `out` (2 + dig + 4 bytes).
#[cfg_attr(target_os = "none", link_section = ".text.vdisk_rd")]
fn sci(out: &mut [u8], v: f32, dig: usize) {
    let mut v = if v.is_finite() { v } else { 0.0 };
    out[0] = if v < 0.0 { b'-' } else { b'+' };
    v = v.abs();
    let mut e = 0i32;
    if v != 0.0 {
        while v >= 10.0 {
            v *= 0.1;
            e += 1;
        }
        while v < 1.0 {
            v *= 10.0;
            e -= 1;
        }
    }
    let mut scale = 1u32;
    for _ in 0..dig {
        scale *= 10;
    }
    let mut m = (v * scale as f32 + 0.5) as u32;
    if m >= 10 * scale {
        m /= 10;
        e += 1;
    }
    let ip = m / scale;
    let mut fp = m % scale;
    out[1] = b'0' + ip as u8;
    out[2] = b'.';
    for i in (0..dig).rev() {
        out[3 + i] = b'0' + (fp % 10) as u8;
        fp /= 10;
    }
    let k = 3 + dig;
    out[k] = b'e';
    out[k + 1] = if e < 0 { b'-' } else { b'+' };
    let ea = e.unsigned_abs().min(99);
    out[k + 2] = b'0' + (ea / 10) as u8;
    out[k + 3] = b'0' + (ea % 10) as u8;
}

/// CSV row `i` of a scope record (the scope_csv.rs text format).
pub fn scope_row(a: f32, b: f32, dt: f32, t0: u32, i: usize, code: u16, out: &mut [u8; SCOPE_ROW]) {
    let t = (i as i32 - t0 as i32) as f32 * dt;
    sci(&mut out[0..12], t, 5);
    out[12] = b',';
    sci(&mut out[13..24], a + b * (code & 0x3FF) as f32, 4);
    out[24] = b'\r';
    out[25] = b'\n';
}

// ------------------------------------------------------------- decoder

/// Random-access bytes of one file.
pub trait Src {
    /// File bytes `off .. off + buf.len()`; false = read error.
    fn read(&mut self, off: u32, buf: &mut [u8]) -> bool;
}

impl Src for &[u8] {
    fn read(&mut self, off: u32, buf: &mut [u8]) -> bool {
        let o = off as usize;
        if o + buf.len() > self.len() {
            return false;
        }
        buf.copy_from_slice(&self[o..o + buf.len()]);
        true
    }
}

const WIN: usize = 256;

/// A read window over a `Src` (file bytes `base .. base + n`).
pub struct Win {
    base: u32,
    n: u32,
    b: [u8; WIN],
}

impl Win {
    pub const fn new() -> Win {
        Win { base: 0, n: 0, b: [0; WIN] }
    }
    /// Byte `off` (< `end`, the readable limit).
    #[cfg_attr(target_os = "none", link_section = ".text.vdisk_rd")]
    fn byte<S: Src>(&mut self, s: &mut S, off: u32, end: u32) -> Option<u8> {
        if off < self.base || off >= self.base + self.n {
            if off >= end {
                return None;
            }
            let n = (end - off).min(WIN as u32);
            if !s.read(off, &mut self.b[..n as usize]) {
                self.n = 0;
                return None;
            }
            self.base = off;
            self.n = n;
        }
        Some(self.b[(off - self.base) as usize])
    }
}

/// A chunk header.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Chunk {
    /// File offset of the header.
    pub off: u32,
    pub pl: u32,
    pub row0: u32,
    pub csv_base: u32,
    pub csv_len: u32,
    pub t0: u32,
    pub nrows: u32,
    pub flags0: u8,
}

impl Chunk {
    pub fn end(&self) -> u32 {
        self.off + CHDR as u32 + self.pl
    }
}

/// Row decoder of one chunk.
#[derive(Clone, Copy)]
struct RowDec {
    /// Bit position: byte offset in the file + bit (0 = MSB).
    pos: u32,
    bit: u32,
    end: u32,
    left: u32,
    t_prev: u32,
    dt_prev: u32,
    flags: u8,
    has_k: bool,
    k: i32,
    pv: i32,
    at: Ad,
    av: Ad,
}

impl RowDec {
    fn new(c: &Chunk) -> RowDec {
        RowDec {
            pos: c.off + CHDR as u32,
            bit: 0,
            end: c.end(),
            left: c.nrows,
            t_prev: c.t0,
            dt_prev: 0,
            flags: c.flags0,
            has_k: false,
            k: 0,
            pv: 0,
            at: Ad::new(AT0),
            av: Ad::new(AV0),
        }
    }

    #[cfg_attr(target_os = "none", link_section = ".text.vdisk_rd")]
    fn get<S: Src>(&mut self, s: &mut S, w: &mut Win, mut bits: u32) -> Option<u32> {
        let mut v = 0u32;
        while bits > 0 {
            let b = w.byte(s, self.pos, self.end)?;
            let take = (8 - self.bit).min(bits);
            let x = (b >> (8 - self.bit - take)) as u32 & ((1u32 << take) - 1);
            v = if take == 32 { x } else { (v << take) | x };
            self.bit += take;
            bits -= take;
            if self.bit == 8 {
                self.bit = 0;
                self.pos += 1;
            }
        }
        Some(v)
    }

    #[cfg_attr(target_os = "none", link_section = ".text.vdisk_rd")]
    fn rice<S: Src>(&mut self, s: &mut S, w: &mut Win, k: u32) -> Option<u32> {
        let mut q = 0;
        while q < QMAX && self.get(s, w, 1)? == 1 {
            q += 1;
        }
        if q == QMAX {
            return self.get(s, w, 32);
        }
        Some((q << k) | self.get(s, w, k)?)
    }

    /// The next row: (t ms, value, flags).
    #[cfg_attr(target_os = "none", link_section = ".text.vdisk_rd")]
    fn next<S: Src>(&mut self, s: &mut S, w: &mut Win) -> Option<(u32, Val, u8)> {
        if self.left == 0 {
            return None;
        }
        let x = self.rice(s, w, self.at.k())?;
        self.at.upd(x);
        let dt = self.dt_prev.wrapping_add(unzz(x) as u32);
        let t = self.t_prev.wrapping_add(dt);
        self.t_prev = t;
        self.dt_prev = dt;
        let v;
        loop {
            if self.flags & F_OL != 0 {
                if self.get(s, w, 1)? == 1 {
                    self.flags = self.get(s, w, 6)? as u8;
                    continue;
                }
                v = Val::Empty;
                break;
            }
            let u = self.rice(s, w, self.av.k())?;
            if u != 0 {
                self.av.upd(u);
                self.pv = self.pv.wrapping_add(unzz(u - 1));
                v = from_scale(self.pv, self.k);
                break;
            }
            match self.get(s, w, 2)? {
                0 => {
                    v = Val::Empty;
                    break;
                }
                1 => {
                    self.flags = self.get(s, w, 6)? as u8;
                    continue;
                }
                2 => {
                    self.k = self.get(s, w, 8)? as u8 as i8 as i32;
                    let neg = self.get(s, w, 1)? == 1;
                    let a = self.get(s, w, 24)? as i32;
                    self.pv = if neg { -a } else { a };
                    self.has_k = true;
                    v = from_scale(self.pv, self.k);
                    break;
                }
                _ => {
                    v = Val::NegZero;
                    break;
                }
            }
        }
        self.left -= 1;
        Some((t, v, self.flags))
    }
}

/// Parse the file header: (kind, hl, unit length, unit).
#[cfg_attr(target_os = "none", link_section = ".text.vdisk_rd")]
pub fn parse_file<S: Src>(s: &mut S, size: u32) -> Option<(u8, u32, usize, [u8; UNIT_MAX])> {
    let mut h = [0u8; FHDR];
    if size < FHDR as u32 || !s.read(0, &mut h) || h[..4] != MAGIC {
        return None;
    }
    let hl = le16(&h, 6);
    let ul = (h[5] as usize).min(UNIT_MAX);
    if FHDR as u32 + hl > size {
        return None;
    }
    let mut c = crc32_upd(!0, &h[12..]);
    let mut t = [0u8; 64];
    let mut o = 0u32;
    while o < hl {
        let n = (hl - o).min(64) as usize;
        if !s.read(FHDR as u32 + o, &mut t[..n]) {
            return None;
        }
        c = crc32_upd(c, &t[..n]);
        o += n as u32;
    }
    if !c != le32(&h, 8) {
        return None;
    }
    let mut unit = [0u8; UNIT_MAX];
    unit.copy_from_slice(&h[12..24]);
    Some((h[4], hl, ul, unit))
}

/// The chunk header at `off` if it is whole within `size` (and, with
/// `check`, its CRC matches).
#[cfg_attr(target_os = "none", link_section = ".text.vdisk_rd")]
pub fn chunk_at<S: Src>(s: &mut S, w: &mut Win, off: u32, size: u32, check: bool) -> Option<Chunk> {
    let mut h = [0u8; CHDR];
    if off.checked_add(CHDR as u32)? > size || !s.read(off, &mut h) || h[0] != b'D' || h[1] != b'C' {
        return None;
    }
    let c = Chunk {
        off,
        pl: le16(&h, 2),
        row0: le32(&h, 8),
        csv_base: le32(&h, 12),
        csv_len: le32(&h, 16),
        t0: le32(&h, 20),
        nrows: le16(&h, 24),
        flags0: h[26],
    };
    if c.end() > size {
        return None;
    }
    if check {
        let mut crc = crc32_upd(!0, &h[8..]);
        let mut o = off + CHDR as u32;
        while o < c.end() {
            let b = w.byte(s, o, c.end())?;
            crc = crc32_upd(crc, &[b]);
            o += 1;
        }
        if !crc != le32(&h, 4) {
            return None;
        }
    }
    Some(c)
}

/// Rows of chunk `c` to `f(t_ms, value, flags)` (gallery). False on a
/// decode error.
pub fn rows<S: Src>(s: &mut S, w: &mut Win, c: &Chunk, f: &mut dyn FnMut(u32, Val, u8)) -> bool {
    let mut rd = RowDec::new(c);
    for _ in 0..c.nrows {
        match rd.next(s, w) {
            Some((t, v, fl)) => f(t, v, fl),
            None => return false,
        }
    }
    true
}

/// Summary of a DLG file (`Gen::open`).
#[derive(Clone, Copy, Debug, Default)]
pub struct Info {
    pub kind: u8,
    /// CSV header text length.
    pub hl: u32,
    /// Exact CSV length.
    pub csv: u32,
    pub rows: u32,
    /// Whole valid chunks, and the file bytes they cover (the rest is a
    /// torn tail: a power cut, a full disk).
    pub chunks: u32,
    pub valid: u32,
    /// Time of the last row, ms.
    pub t_last: u32,
}

/// CSV generator over one DLG file (random access, sequential reads O(1)).
pub struct Gen<S: Src> {
    pub s: S,
    size: u32,
    pub info: Info,
    ul: usize,
    unit: [u8; UNIT_MAX],
    win: Win,
    /// Kind 2: a, b, dt, t0; the first block's offset and the blocks'
    /// coded bytes (<= SBLK_RAW: u16); the block decoder (`sd_b` = its
    /// block, `sd_ok`), the last code it gave.
    sc: (f32, f32, f32, u32),
    off0: u32,
    blen: [u16; SBLK_MAX],
    sd: SDec,
    sd_b: u32,
    sd_ok: bool,
    sd_code: u16,
    /// Kind 3: parameters, first chunk's offset, the cursor's chunk and
    /// decoder, the row index held in `row` (u32::MAX: none).
    fp: FPar,
    fstart: u32,
    fc: Option<FChunk>,
    fd: FDec,
    fk: u32,
    /// Kind 3: seek checkpoints (first row, chunk offset) of every
    /// `ck_step`-th chunk, from the open's header walk (`fast_seek`
    /// starts at the nearest one: a host that reads two places of a file in
    /// turn, e.g. an indexer at its head while a copy runs, no longer walks
    /// the chunk headers from the start, ~1300 of them, for each sector).
    ck: [(u32, u32); FCK],
    nck: u8,
    ck_step: u32,
    // cursor
    cur: Option<Chunk>,
    rd: RowDec,
    /// CSV offset of `row`'s first byte, its text.
    rpos: u32,
    row: [u8; ROW_MAX],
    rlen: u32,
}

impl<S: Src> Gen<S> {
    /// Check the header and every chunk (CRC): the exact CSV size. None: not
    /// a DLG file of a known kind.
    #[cfg_attr(target_os = "none", link_section = ".text.vdisk_rd")]
    pub fn open(mut s: S, size: u32) -> Option<Gen<S>> {
        let (kind, hl, ul, unit) = parse_file(&mut s, size)?;
        if kind != KIND_DMM && kind != KIND_SCOPE && kind != KIND_FAST {
            return None;
        }
        let mut g = Gen {
            s,
            size,
            info: Info { kind, hl, csv: hl, valid: FHDR as u32 + hl, ..Info::default() },
            ul,
            unit,
            win: Win::new(),
            sc: (0.0, 0.0, 0.0, 0),
            off0: 0,
            blen: [0; SBLK_MAX],
            sd: SDec { pos: 0, bit: 0, end: 0, mode: 0, j: 0, h: [0; 3], ad: Ad::new(AS0) },
            sd_b: 0,
            sd_ok: false,
            sd_code: 0,
            fp: FPar::default(),
            fstart: 0,
            fc: None,
            fd: FDec::IDLE,
            fk: u32::MAX,
            ck: [(0, 0); FCK],
            nck: 0,
            ck_step: 1,
            cur: None,
            rd: RowDec::new(&Chunk { off: 0, pl: 0, row0: 0, csv_base: 0, csv_len: 0, t0: 0, nrows: 0, flags0: 0 }),
            rpos: 0,
            row: [0; ROW_MAX],
            rlen: 0,
        };
        if kind == KIND_SCOPE {
            let mut p = [0u8; SCOPE_PAR];
            let at = FHDR as u32 + hl;
            if !g.s.read(at, &mut p) || !crc32_upd(!0, &p[..20]) != le32(&p, 20) {
                return None;
            }
            let f = |o: usize| f32::from_le_bytes([p[o], p[o + 1], p[o + 2], p[o + 3]]);
            let n = le32(&p, 16);
            let nb = (n as usize).div_ceil(SBLK);
            if nb > SBLK_MAX {
                return None;
            }
            let t0 = at + SCOPE_PAR as u32;
            let tl = 8 * nb + 4;
            // The table (8 bytes a block, read twice: no copy of it on the
            // stack), then its CRC.
            let mut e8 = [0u8; 8];
            let mut c = !0u32;
            for b in 0..nb {
                if !g.s.read(t0 + 8 * b as u32, &mut e8) {
                    return None;
                }
                c = crc32_upd(c, &e8);
            }
            if !g.s.read(t0 + 8 * nb as u32, &mut e8[..4]) || !c != le32(&e8, 0) {
                return None;
            }
            g.sc = (f(0), f(4), f(8), le32(&p, 12));
            // The whole, CRC-correct blocks (a torn file: the ones before).
            let mut o = t0 + tl as u32;
            let mut rows = 0u32;
            g.off0 = o;
            for b in 0..nb {
                if !g.s.read(t0 + 8 * b as u32, &mut e8) {
                    break;
                }
                let l = le32(&e8, 0);
                let e = o + l;
                if e > size || l > SBLK_RAW as u32 {
                    break;
                }
                let mut c = !0u32;
                let mut x = o;
                while x < e {
                    let Some(v) = g.win.byte(&mut g.s, x, e) else { break };
                    c = crc32_upd(c, &[v]);
                    x += 1;
                }
                if x < e || !c != le32(&e8, 4) {
                    break;
                }
                g.blen[b] = l as u16;
                o = e;
                rows = ((b + 1) * SBLK).min(n as usize) as u32;
            }
            g.info.rows = rows;
            g.info.chunks = rows.div_ceil(SBLK as u32);
            g.info.csv = hl + rows * SCOPE_ROW as u32;
            g.info.valid = o;
            g.size = g.info.valid;
            return Some(g);
        }
        if kind == KIND_FAST {
            let mut p = [0u8; FAST_PAR];
            let at = FHDR as u32 + hl;
            if !g.s.read(at, &mut p) {
                return None;
            }
            g.fp = FPar::parse(&p)?;
            g.fstart = at + FAST_PAR as u32;
            // The chunk headers only (28 bytes each, ~1 per 4 KiB: a 3 MB
            // file in ~0.1 s on the device, where a CRC of every chunk read
            // the whole file, ~2 s per MB at 12.5 MHz, at every refresh of
            // the USB volume and every open of its CSV); then the CRCs of
            // the last FAST_TAIL chunks, where a cut leaves a torn one (the
            // writer has at most ~2 chunks not yet fully programmed: the 8
            // KiB FIFO): the file ends before the first bad one. A chunk
            // corrupted further in decodes as garbage rows (its header
            // still gives their count, so the CSV's size and the later rows
            // stay right); tools/dlog.py checks every CRC.
            let mut off = g.fstart;
            // The last FAST_TAIL + 1 headers (the one before the checked
            // ones: the last row's time when they are all torn).
            const R: usize = FAST_TAIL + 1;
            let mut tail = [None::<FChunk>; R];
            let mut n = 0usize;
            while let Some(c) = fchunk_at(&mut g.s, &mut g.win, off, size, false) {
                if c.row0 != g.info.rows {
                    break;
                }
                g.info.rows += c.nrows;
                g.info.chunks += 1;
                off = c.end();
                tail[n % R] = Some(c);
                if n as u32 % g.ck_step == 0 {
                    if g.nck as usize == FCK {
                        // Full: keep every other one, twice the spacing.
                        for i in 0..FCK / 2 {
                            g.ck[i] = g.ck[2 * i];
                        }
                        g.nck = (FCK / 2) as u8;
                        g.ck_step *= 2;
                    }
                    if n as u32 % g.ck_step == 0 {
                        g.ck[g.nck as usize] = (c.row0, c.off);
                        g.nck += 1;
                    }
                }
                n += 1;
            }
            let k0 = n.saturating_sub(FAST_TAIL);
            let mut last = if k0 > 0 { tail[(k0 - 1) % R] } else { None };
            for k in k0..n {
                let Some(c) = tail[k % R] else { break };
                if fchunk_at(&mut g.s, &mut g.win, c.off, size, true).is_none() {
                    // Torn: the file ends before this chunk.
                    for j in k..n {
                        if let Some(x) = tail[j % R] {
                            g.info.rows -= x.nrows;
                            g.info.chunks -= 1;
                        }
                    }
                    off = c.off;
                    break;
                }
                last = Some(c);
            }
            g.info.valid = off;
            g.info.csv = hl + g.info.rows * g.fp.row_len();
            if let Some(c) = last {
                let fp = g.fp;
                let mut t_last = 0;
                fast_rows(&mut g.s, &mut g.win, &c, &fp, &mut |t, _, _| t_last = t);
                g.info.t_last = (t_last / 1000).min(u32::MAX as u64) as u32;
            }
            g.size = off;
            return Some(g);
        }
        let mut off = g.info.valid;
        while let Some(c) = chunk_at(&mut g.s, &mut g.win, off, size, true) {
            if c.csv_base != g.info.csv || c.row0 != g.info.rows {
                break;
            }
            // The last row's time needs a decode of the last chunk only.
            g.info.csv += c.csv_len;
            g.info.rows += c.nrows;
            g.info.chunks += 1;
            g.info.valid = c.end();
            off = c.end();
            if c.nrows > 0 {
                g.cur = Some(c);
            }
        }
        if let Some(c) = g.cur.take() {
            let mut rd = RowDec::new(&c);
            while let Some((t, _, _)) = rd.next(&mut g.s, &mut g.win) {
                g.info.t_last = t;
            }
        }
        g.size = g.info.valid;
        Some(g)
    }

    pub fn unit(&self) -> &[u8] {
        &self.unit[..self.ul]
    }

    /// The first chunk at or after file offset `off` (None at the end).
    pub fn chunk(&mut self, off: u32) -> Option<Chunk> {
        let off = off.max(FHDR as u32 + self.info.hl);
        chunk_at(&mut self.s, &mut self.win, off, self.size, false)
    }

    /// Rows of chunk `c` to `f(t_ms, value, flags)` (gallery). False on a
    /// decode error.
    pub fn rows(&mut self, c: &Chunk, f: &mut dyn FnMut(u32, Val, u8)) -> bool {
        rows(&mut self.s, &mut self.win, c, f)
    }

    /// The CSV header text bytes `off..` (into `buf`).
#[cfg_attr(target_os = "none", link_section = ".text.vdisk_rd")]
    fn header_text(&mut self, off: u32, buf: &mut [u8]) -> bool {
        self.s.read(FHDR as u32 + off, buf)
    }

    /// Decode the next row of the cursor into `row`; false at the chunk end.
    #[cfg_attr(target_os = "none", link_section = ".text.vdisk_rd")]
    fn advance(&mut self) -> bool {
        match self.rd.next(&mut self.s, &mut self.win) {
            Some((t, v, fl)) => {
                self.rpos += self.rlen;
                let mut r = [0u8; ROW_MAX];
                let unit = self.unit;
                self.rlen = fmt_row(t as u64, v, &unit[..self.ul], fl, &mut r) as u32;
                self.row = r;
                true
            }
            None => false,
        }
    }

    /// Put the cursor at the chunk holding CSV offset `off` (a data row).
    fn seek(&mut self, off: u32) -> bool {
        let mut at = FHDR as u32 + self.info.hl;
        if let Some(c) = self.cur {
            // (the next chunk: as `fast_seek`)
            if off >= c.csv_base + c.csv_len {
                at = c.end();
            } else if off >= c.csv_base {
                at = c.off;
            }
        }
        while let Some(c) = chunk_at(&mut self.s, &mut self.win, at, self.size, false) {
            if off < c.csv_base + c.csv_len {
                self.cur = Some(c);
                self.rd = RowDec::new(&c);
                self.rpos = c.csv_base;
                self.rlen = 0;
                return self.advance();
            }
            at = c.end();
        }
        false
    }

    /// Code `k` of a scope record (the block decoder continues forward,
    /// else restarts at the block's start).
    fn scope_code(&mut self, k: u32) -> Option<u16> {
        let b = k / SBLK as u32;
        let j = (k % SBLK as u32) as usize;
        if !(self.sd_ok && self.sd_b == b && self.sd.j > 0 && self.sd.j - 1 <= j) {
            self.sd_ok = false;
            let o = self.off0 + self.blen[..b as usize].iter().map(|&l| l as u32).sum::<u32>();
            self.sd = SDec::new(&mut self.s, &mut self.win, o, o + self.blen[b as usize] as u32)?;
            self.sd_b = b;
            self.sd_code = self.sd.next(&mut self.s, &mut self.win)?;
            self.sd_ok = true;
        }
        while self.sd.j - 1 < j {
            match self.sd.next(&mut self.s, &mut self.win) {
                Some(c) => self.sd_code = c,
                None => {
                    self.sd_ok = false;
                    return None;
                }
            }
        }
        Some(self.sd_code)
    }

    /// Kind 3: decode row `k` into `row` (forward in the cursor's chunk
    /// when it is ahead, else from the chunk holding it).
#[cfg_attr(target_os = "none", link_section = ".text.vdisk_rd")]
    fn fast_seek(&mut self, k: u32) -> bool {
        let fwd = self.fc.is_some_and(|c| self.fk != u32::MAX && k > self.fk && k < c.row0 + c.nrows);
        if !fwd {
            FR[2].fetch_add(1, core::sync::atomic::Ordering::Relaxed);
            // From the next chunk when `k` is past the cursor's one: its
            // header is behind the reader, and a step back in the file
            // walks the FAT chain from the start again (FatFile::read; once
            // per chunk = quadratic in the file's size).
            let (mut r0, mut at) = match self.fc {
                Some(c) if k >= c.row0 + c.nrows => (c.row0 + c.nrows, c.end()),
                Some(c) if k >= c.row0 => (c.row0, c.off),
                _ => (0, self.fstart),
            };
            // A checkpoint nearer to `k` (chunks past the torn tail are
            // not in the table: the walk stops at the last whole one).
            for &(cr, co) in &self.ck[..(self.nck as usize).min(FCK)] {
                if cr <= k && cr > r0 {
                    (r0, at) = (cr, co);
                }
            }
            loop {
                let Some(c) = fchunk_at(&mut self.s, &mut self.win, at, self.size, false) else {
                    self.fc = None;
                    self.fk = u32::MAX;
                    return false;
                };
                if k < c.row0 + c.nrows {
                    self.fc = Some(c);
                    self.fd = FDec::new(&c, self.fp.p_us());
                    self.fk = c.row0.wrapping_sub(1);
                    break;
                }
                at = c.end();
            }
        }
        while self.fk != k {
            let Some((t, v, dt)) = self.fd.next(&mut self.s, &mut self.win) else {
                self.fc = None;
                self.fk = u32::MAX;
                return false;
            };
            self.fk = self.fk.wrapping_add(1);
            FR[4].fetch_add(1, core::sync::atomic::Ordering::Relaxed);
            if self.fk == k {
                let fp = self.fp;
                let mut r = [0u8; ROW_MAX];
                fast_row(t, v, fast_lost(dt, fp.rate_mhz), &fp, &mut r);
                self.row = r;
            }
        }
        true
    }

    /// The CSV offset the cursor is at (the start of the row it holds; 0
    /// with none): the next sequential read continues from here cheaply.
    #[cfg_attr(target_os = "none", link_section = ".text.vdisk_rd")]
    pub fn pos(&self) -> u32 {
        if self.info.kind == KIND_FAST {
            if self.fk == u32::MAX { 0 } else { self.info.hl + self.fk * self.fp.row_len() }
        } else {
            self.rpos
        }
    }

    /// Kind 3: parameters (gallery).
    pub fn fast_par(&self) -> FPar {
        self.fp
    }

    /// CSV bytes `off .. off + buf.len()` (zeros past the end).
#[cfg_attr(target_os = "none", link_section = ".text.vdisk_rd")]
    pub fn read(&mut self, mut off: u32, buf: &mut [u8]) {
        let mut i = 0;
        buf.fill(0);
        while i < buf.len() && off < self.info.csv {
            if off < self.info.hl {
                let n = ((self.info.hl - off) as usize).min(buf.len() - i);
                if !self.header_text(off, &mut buf[i..i + n]) {
                    return;
                }
                i += n;
                off += n as u32;
                continue;
            }
            if self.info.kind == KIND_FAST {
                let wd = self.fp.row_len();
                let k = (off - self.info.hl) / wd;
                if self.fk != k && !self.fast_seek(k) {
                    return;
                }
                let o = ((off - self.info.hl) % wd) as usize;
                let n = (wd as usize - o).min(buf.len() - i);
                buf[i..i + n].copy_from_slice(&self.row[o..o + n]);
                i += n;
                off += n as u32;
                continue;
            }
            if self.info.kind == KIND_SCOPE {
                let k = (off - self.info.hl) / SCOPE_ROW as u32;
                let Some(code) = self.scope_code(k) else { return };
                let mut r = [0u8; SCOPE_ROW];
                let (a, b, dt, t0) = self.sc;
                scope_row(a, b, dt, t0, k as usize, code, &mut r);
                let o = ((off - self.info.hl) % SCOPE_ROW as u32) as usize;
                let n = (SCOPE_ROW - o).min(buf.len() - i);
                buf[i..i + n].copy_from_slice(&r[o..o + n]);
                i += n;
                off += n as u32;
                continue;
            }
            let inrow = self.cur.is_some() && off >= self.rpos && off < self.rpos + self.rlen;
            if !inrow {
                let next = self.cur.is_some_and(|c| off >= self.rpos + self.rlen && off < c.csv_base + c.csv_len);
                let ok = if next {
                    let mut ok = true;
                    while ok && off >= self.rpos + self.rlen {
                        ok = self.advance();
                    }
                    ok
                } else {
                    self.seek(off) && {
                        let mut ok = true;
                        while ok && off >= self.rpos + self.rlen {
                            ok = self.advance();
                        }
                        ok
                    }
                };
                if !ok {
                    self.cur = None;
                    return;
                }
            }
            let o = (off - self.rpos) as usize;
            let n = (self.rlen as usize - o).min(buf.len() - i);
            buf[i..i + n].copy_from_slice(&self.row[o..o + n]);
            i += n;
            off += n as u32;
        }
    }
}

// ------------------------------------------------------------- fast log (kind 3)

/// Kind 3 (the fast logger's stream, fastlog.rs; docs/compact-log-format.md
/// "Fast stream"): after the header text FAST_PAR bytes of parameters
/// (f32 a, f32 b: value = a + b * raw; u32 nominal rate in mHz; u8 flags
/// (FP_SCALED: the value column exists), u8 source (1 AD1, 2 AD2), u8 DMM
/// mode, i8 range, 12 bytes 0, u32 CRC-32 of the 28 bytes before it), then
/// chunks:
///   0 "DC", 2 u16 payload bytes pl, 4 u32 CRC-32 of the payload FOLLOWED by
///   bytes 8..28 (the writer patches the header in after the payload went
///   out), 8 u32 first row, 12 u32 t0 low / 16 u16 t0 high (us of the first
///   row, 48 bit), 18 u16 rows, 20 u32 dt0 (us from the previous row, 0 for
///   the file's first), 24 i32 v0 (raw value of the first row), 28 payload.
/// Rows 1.. of a chunk: Rice(zz(dt - dt_prev)) then Rice(zz(raw - raw_prev)),
/// adaptive as kind 1 (time A0 4, value A0 16), dt_prev starting at the
/// nominal period (`FPar::p_us`).
/// CSV (fixed width, so its size is hl + rows * `row_len` and a seek is a
/// division): `t_s,raw,value_<unit>,lost_before` (AD2, scaled) or
/// `t_s,raw,lost_before` (AD1, raw counts): t_s = the device us clock /
/// 1e6, 6 decimals (dm40scpi.py `log`'s column), lost_before = conversions
/// missing before the row: round(dt / nominal period) - 1 (a stall, the ring
/// full, the disk too slow: all show as a stamp gap).
pub const KIND_FAST: u8 = 3;
/// Kind 3: seek checkpoints per generator (`Gen::ck`).
const FCK: usize = 16;
/// Kind 3: the chunks at the end whose CRC `Gen::open` checks.
const FAST_TAIL: usize = 4;
pub const FAST_PAR: usize = 32;
/// FPar::flags: the value column (a, b valid).
pub const FP_SCALED: u8 = 1;
/// Row widths: t_s 14, raw 8, value 12, lost_before 7, CR LF.
pub const FROW_SCALED: u32 = 14 + 1 + 8 + 1 + 12 + 1 + 7 + 2;
pub const FROW_RAW: u32 = 14 + 1 + 8 + 1 + 7 + 2;
const FLOST_MAX: u32 = 9_999_999;
const AF_T0: u32 = 4;
const AF_V0: u32 = 16;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FPar {
    pub a: f32,
    pub b: f32,
    pub rate_mhz: u32,
    pub flags: u8,
    pub src: u8,
    pub mode: u8,
    pub range: i8,
}

impl FPar {
    pub fn row_len(&self) -> u32 {
        if self.flags & FP_SCALED != 0 { FROW_SCALED } else { FROW_RAW }
    }
    /// Nominal period, us (rounded): the time coder's first dt_prev.
    pub fn p_us(&self) -> u32 {
        let r = self.rate_mhz.max(1) as u64;
        ((1_000_000_000 + r / 2) / r) as u32
    }
    pub fn bytes(&self) -> [u8; FAST_PAR] {
        let mut p = [0u8; FAST_PAR];
        p[0..4].copy_from_slice(&self.a.to_le_bytes());
        p[4..8].copy_from_slice(&self.b.to_le_bytes());
        p[8..12].copy_from_slice(&self.rate_mhz.to_le_bytes());
        p[12] = self.flags;
        p[13] = self.src;
        p[14] = self.mode;
        p[15] = self.range as u8;
        let c = !crc32_upd(!0, &p[..28]);
        p[28..32].copy_from_slice(&c.to_le_bytes());
        p
    }
    // Region B: Gen::open of a FAST CSV calls it while a host reads the
    // volume, also during the Wave view's stream (device check 33b44bd:
    // max_lat 21.4 us at 38400 S/s with lat_pc in this fn, region C). No
    // closure: it would get a symbol of its own in C.
    #[cfg_attr(target_os = "none", link_section = ".text.vdisk_rd")]
    pub fn parse(p: &[u8]) -> Option<FPar> {
        if p.len() < FAST_PAR || !crc32_upd(!0, &p[..28]) != le32(p, 28) {
            return None;
        }
        let fp = FPar { a: f32::from_bits(le32(p, 0)), b: f32::from_bits(le32(p, 4)), rate_mhz: le32(p, 8), flags: p[12], src: p[13], mode: p[14], range: p[15] as i8 };
        (fp.rate_mhz != 0).then_some(fp)
    }
}

/// Conversions missing before a row `dt_us` after the previous one.
#[cfg_attr(target_os = "none", link_section = ".text.vdisk_rd")]
pub fn fast_lost(dt_us: u32, rate_mhz: u32) -> u32 {
    let n = (dt_us as u64 * rate_mhz as u64 + 500_000_000) / 1_000_000_000;
    (n.saturating_sub(1)).min(FLOST_MAX as u64) as u32
}

/// `v` right-aligned in `out` (spaces before), "-" for negatives.
#[cfg_attr(target_os = "none", link_section = ".text.vdisk_rd")]
fn rjust(out: &mut [u8], v: i64) {
    let mut x = v.unsigned_abs();
    let mut i = out.len();
    loop {
        i -= 1;
        out[i] = b'0' + (x % 10) as u8;
        x /= 10;
        if x == 0 || i == 0 {
            break;
        }
    }
    if v < 0 && i > 0 {
        i -= 1;
        out[i] = b'-';
    }
    out[..i].fill(b' ');
}

/// One kind-3 CSV row into `out` (`row_len` bytes, returned).
#[cfg_attr(target_os = "none", link_section = ".text.vdisk_rd")]
pub fn fast_row(t_us: u64, raw: i32, lost: u32, p: &FPar, out: &mut [u8]) -> usize {
    // t_s: 7 integer digits (2^48 us would need 9: capped, never reached)
    let s = (t_us / 1_000_000).min(9_999_999);
    rjust(&mut out[0..7], s as i64);
    out[7] = b'.';
    let mut f = (t_us % 1_000_000) as u32;
    for i in (8..14).rev() {
        out[i] = b'0' + (f % 10) as u8;
        f /= 10;
    }
    out[14] = b',';
    rjust(&mut out[15..23], raw as i64);
    out[23] = b',';
    let mut k = 24;
    if p.flags & FP_SCALED != 0 {
        sci(&mut out[24..36], p.a + p.b * raw as f32, 5);
        out[36] = b',';
        k = 37;
    }
    rjust(&mut out[k..k + 7], lost.min(FLOST_MAX) as i64);
    out[k + 7] = b'\r';
    out[k + 8] = b'\n';
    k + 9
}

/// A kind-3 chunk header.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FChunk {
    pub off: u32,
    pub pl: u32,
    pub row0: u32,
    pub t0: u64,
    pub nrows: u32,
    pub dt0: u32,
    pub v0: i32,
}

impl FChunk {
    pub fn end(&self) -> u32 {
        self.off + CHDR as u32 + self.pl
    }
}

/// The kind-3 chunk header at `off` if whole within `size` (with `check`:
/// its CRC over the payload, then header bytes 8..28, matches).
#[cfg_attr(target_os = "none", link_section = ".text.vdisk_rd")]
pub fn fchunk_at<S: Src>(s: &mut S, w: &mut Win, off: u32, size: u32, check: bool) -> Option<FChunk> {
    FR[3].fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    let mut h = [0u8; CHDR];
    if off.checked_add(CHDR as u32)? > size || !s.read(off, &mut h) || h[0] != b'D' || h[1] != b'C' {
        return None;
    }
    let c = FChunk {
        off,
        pl: le16(&h, 2),
        row0: le32(&h, 8),
        t0: le32(&h, 12) as u64 | (le16(&h, 16) as u64) << 32,
        nrows: le16(&h, 18),
        dt0: le32(&h, 20),
        v0: le32(&h, 24) as i32,
    };
    if c.nrows == 0 || c.end() > size {
        return None;
    }
    if check {
        let mut crc = !0u32;
        let mut o = off + CHDR as u32;
        while o < c.end() {
            let b = w.byte(s, o, c.end())?;
            crc = crc32_upd(crc, &[b]);
            o += 1;
        }
        crc = crc32_upd(crc, &h[8..]);
        if !crc != le32(&h, 4) {
            return None;
        }
    }
    Some(c)
}

/// Row decoder of one kind-3 chunk.
#[derive(Clone, Copy)]
struct FDec {
    pos: u32,
    bit: u32,
    end: u32,
    left: u32,
    first: bool,
    t: u64,
    dt: u32,
    dt0: u32,
    v: i32,
    at: Ad,
    av: Ad,
}

impl FDec {
    const IDLE: FDec = FDec { pos: 0, bit: 0, end: 0, left: 0, first: false, t: 0, dt: 0, dt0: 0, v: 0, at: Ad::new(AF_T0), av: Ad::new(AF_V0) };

    #[cfg_attr(target_os = "none", link_section = ".text.vdisk_rd")]
    fn new(c: &FChunk, p_us: u32) -> FDec {
        FDec { pos: c.off + CHDR as u32, end: c.end(), left: c.nrows, first: true, t: c.t0, dt: p_us, dt0: c.dt0, v: c.v0, ..FDec::IDLE }
    }

    #[cfg_attr(target_os = "none", link_section = ".text.vdisk_rd")]
    fn get<S: Src>(&mut self, s: &mut S, w: &mut Win, mut bits: u32) -> Option<u32> {
        let mut v = 0u32;
        while bits > 0 {
            let b = w.byte(s, self.pos, self.end)?;
            let take = (8 - self.bit).min(bits);
            let x = (b >> (8 - self.bit - take)) as u32 & ((1u32 << take) - 1);
            v = if take == 32 { x } else { (v << take) | x };
            self.bit += take;
            bits -= take;
            if self.bit == 8 {
                self.bit = 0;
                self.pos += 1;
            }
        }
        Some(v)
    }

    #[cfg_attr(target_os = "none", link_section = ".text.vdisk_rd")]
    fn rice<S: Src>(&mut self, s: &mut S, w: &mut Win, k: u32) -> Option<u32> {
        let mut q = 0;
        while q < QMAX && self.get(s, w, 1)? == 1 {
            q += 1;
        }
        if q == QMAX {
            return self.get(s, w, 32);
        }
        Some((q << k) | self.get(s, w, k)?)
    }

    /// The next row: (t us, raw, us since the previous row).
    #[cfg_attr(target_os = "none", link_section = ".text.vdisk_rd")]
    fn next<S: Src>(&mut self, s: &mut S, w: &mut Win) -> Option<(u64, i32, u32)> {
        if self.left == 0 {
            return None;
        }
        self.left -= 1;
        if self.first {
            self.first = false;
            return Some((self.t, self.v, self.dt0));
        }
        let x = self.rice(s, w, self.at.k())?;
        self.at.upd(x);
        self.dt = self.dt.wrapping_add(unzz(x) as u32);
        self.t += self.dt as u64;
        let u = self.rice(s, w, self.av.k())?;
        self.av.upd(u);
        self.v = self.v.wrapping_add(unzz(u));
        Some((self.t, self.v, self.dt))
    }
}

/// Rows of kind-3 chunk `c` to `f(t_us, raw, dt_us)` (gallery). False on a
/// decode error.
pub fn fast_rows<S: Src>(s: &mut S, w: &mut Win, c: &FChunk, p: &FPar, f: &mut dyn FnMut(u64, i32, u32)) -> bool {
    let mut d = FDec::new(c, p.p_us());
    for _ in 0..c.nrows {
        match d.next(s, w) {
            Some((t, v, dt)) => f(t, v, dt),
            None => return false,
        }
    }
    true
}

// ------------------------------------------------------------- FAT16 files

/// Raw bytes of the disk (SPI flash: byte address = block * 4096 + offset).
pub trait Raw {
    fn read_at(&mut self, addr: u32, buf: &mut [u8]) -> bool;
}

/// FAT16 volume geometry in bytes.
#[derive(Clone, Copy, Debug)]
pub struct Geo {
    pub fat: u32,
    pub root: u32,
    pub root_entries: u32,
    pub data: u32,
    pub cb: u32,
    pub clusters: u32,
}

#[cfg_attr(target_os = "none", link_section = ".text.vdisk_rd")]
pub fn geometry(raw: &mut dyn Raw) -> Option<Geo> {
    let mut b = [0u8; 64];
    if !raw.read_at(0, &mut b) {
        return None;
    }
    let bps = le16(&b, 0x0B);
    let spc = b[0x0D] as u32;
    let rsvd = le16(&b, 0x0E);
    let nfats = b[0x10] as u32;
    let re = le16(&b, 0x11);
    let tot = if le16(&b, 0x13) != 0 { le16(&b, 0x13) } else { le32(&b, 0x20) };
    let fatsz = le16(&b, 0x16);
    if !(b[0] == 0xEB || b[0] == 0xE9) || bps < 512 || spc == 0 || nfats == 0 || re == 0 || fatsz == 0 {
        return None;
    }
    let root = (rsvd + nfats * fatsz) * bps;
    let data = root + re * 32;
    let clusters = (tot * bps).saturating_sub(data) / (spc * bps);
    Some(Geo { fat: rsvd * bps, root, root_entries: re, data, cb: spc * bps, clusters })
}

/// One root file as a `Src` (follows its FAT 1 chain; caches the position).
pub struct FatFile<R: Raw> {
    pub raw: R,
    g: Geo,
    start: u32,
    pub size: u32,
    ci: u32,
    cl: u32,
}

impl<R: Raw> FatFile<R> {
    pub fn new(raw: R, g: Geo, start: u32, size: u32) -> Self {
        FatFile { raw, g, start, size, ci: 0, cl: start }
    }
    #[cfg_attr(target_os = "none", link_section = ".text.vdisk_rd")]
    fn next(&mut self, c: u32) -> Option<u32> {
        let mut b = [0u8; 2];
        if c < 2 || c > self.g.clusters + 1 || !self.raw.read_at(self.g.fat + c * 2, &mut b) {
            return None;
        }
        let n = le16(&b, 0);
        (2..0xFFF0).contains(&n).then_some(n)
    }
}

impl<R: Raw> Src for FatFile<R> {
    #[cfg_attr(target_os = "none", link_section = ".text.vdisk_rd")]
    fn read(&mut self, mut off: u32, buf: &mut [u8]) -> bool {
        if off as u64 + buf.len() as u64 > self.size as u64 {
            return false;
        }
        let mut i = 0;
        while i < buf.len() {
            let ci = off / self.g.cb;
            if ci < self.ci {
                self.ci = 0;
                self.cl = self.start;
            }
            while self.ci < ci {
                match self.next(self.cl) {
                    Some(n) => {
                        self.cl = n;
                        self.ci += 1;
                    }
                    None => return false,
                }
            }
            if self.cl < 2 || self.cl > self.g.clusters + 1 {
                return false;
            }
            let o = off % self.g.cb;
            let n = ((self.g.cb - o) as usize).min(buf.len() - i);
            if !self.raw.read_at(self.g.data + (self.cl - 2) * self.g.cb + o, &mut buf[i..i + n]) {
                return false;
            }
            i += n;
            off += n as u32;
        }
        true
    }
}

/// A root entry: 8.3 name, start cluster, size.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ent {
    pub name: [u8; 11],
    pub start: u32,
    pub size: u32,
}

/// `<prefix>NNN.DLG` (prefix LOG, SCOP or FAST): Some(prefix length).
/// SCOP and FAST are both 4 long: `dlg_prefix` tells them apart.
pub fn dlg_name(n: &[u8]) -> Option<usize> {
    dlg_prefix(n).map(|p| p.len())
}

/// The prefix of a `<prefix>NNN.DLG` name: b"LOG" (DMM recordings,
/// LOGS/), b"SCOP" (scope records, SCOPE/), b"FAST" (the fast logger,
/// LOGS/); None for any other name.
#[cfg_attr(target_os = "none", link_section = ".text.vdisk_rd")]
pub fn dlg_prefix(n: &[u8]) -> Option<&'static [u8]> {
    let up = |c: u8| c.to_ascii_uppercase();
    if up(n[8]) != b'D' || up(n[9]) != b'L' || up(n[10]) != b'G' {
        return None;
    }
    let p: &'static [u8] = if n[..3].eq_ignore_ascii_case(b"LOG") {
        b"LOG"
    } else if n[..4].eq_ignore_ascii_case(b"SCOP") {
        b"SCOP"
    } else if n[..4].eq_ignore_ascii_case(b"FAST") {
        b"FAST"
    } else {
        return None;
    };
    let k = p.len();
    (n[k..k + 3].iter().all(|c| c.is_ascii_digit()) && n[k + 3..8].iter().all(|&c| c == b' ')).then_some(p)
}

/// Directory of the DMM logs / scope records on the SPI disk (fatdir.rs;
/// the root may still hold files of earlier builds).
pub const DIR_LOGS: [u8; 11] = *b"LOGS       ";
pub const DIR_SCOPE: [u8; 11] = *b"SCOPE      ";
pub const DIR_SHOTS: [u8; 11] = *b"SHOTS      ";

/// Every 32-byte entry of a directory, in order, to `f` (false = stop),
/// until the end marker: the root (`start` None) or the subdirectory whose
/// chain starts at cluster `start` (followed in FAT 1, at most 64 clusters).
/// False: a read failed.
///
/// Generic and inline(always) (region B audit #3): `f` is then a static call,
/// inlined, and no closure of a caller gets its own symbol. A closure's
/// section carries this module's path, so the .cold rule would put it in
/// region C even when its caller (`find`, `dir_start`: a host opening a CSV
/// of the volume, maybe while a stream runs) is in region B.
#[inline(always)]
pub fn each_entry<F: FnMut(&[u8; 32]) -> bool>(raw: &mut dyn Raw, g: &Geo, start: Option<u32>, f: &mut F) -> bool {
    let mut e = [0u8; 32];
    let (mut base, mut n, mut cl) = match start {
        None => (g.root, g.root_entries, 0),
        Some(c) if c >= 2 && c <= g.clusters + 1 => (g.data + (c - 2) * g.cb, g.cb / 32, c),
        Some(_) => return true,
    };
    let mut hops = 0;
    loop {
        for i in 0..n {
            if !raw.read_at(base + i * 32, &mut e) {
                return false;
            }
            if e[0] == 0 || !f(&e) {
                return true;
            }
        }
        if cl == 0 {
            return true;
        }
        let mut b = [0u8; 2];
        if !raw.read_at(g.fat + cl * 2, &mut b) {
            return false;
        }
        let next = le16(&b, 0);
        hops += 1;
        if !(2..=g.clusters + 1).contains(&next) || hops >= 64 {
            return true;
        }
        cl = next;
        base = g.data + (cl - 2) * g.cb;
        n = g.cb / 32;
    }
}

/// Start cluster of root subdirectory `name` (None: there is none).
#[cfg_attr(target_os = "none", link_section = ".text.vdisk_rd")]
pub fn dir_start(raw: &mut dyn Raw, g: &Geo, name: &[u8; 11]) -> Option<u32> {
    let mut hit = None;
    each_entry(raw, g, None, &mut |e| {
        if e[0] != 0xE5 && e[11] & 0x3F != 0x0F && e[11] & 0x10 != 0 && e[..11].eq_ignore_ascii_case(name) {
            hit = Some(le16(e, 0x1A));
            return false;
        }
        true
    });
    hit
}

/// A live plain-file 8.3 entry (not deleted, long-name, label, directory).
#[cfg_attr(target_os = "none", link_section = ".text.vdisk_rd")]
fn file_entry(e: &[u8; 32]) -> Option<Ent> {
    if e[0] == 0xE5 || e[11] & 0x18 != 0 || e[11] & 0x3F == 0x0F {
        return None;
    }
    let mut name = [0u8; 11];
    name.copy_from_slice(&e[..11]);
    Some(Ent { name, start: le16(e, 0x1A), size: le32(e, 0x1C) })
}

/// Files `LOGnnn.DLG` / `FASTnnn.DLG` / `SCOPnnn.DLG` to `f`: the root's (earlier builds),
/// then LOGS/, then SCOPE/, each in directory order.
pub fn scan(raw: &mut dyn Raw, g: &Geo, f: &mut dyn FnMut(&Ent)) -> bool {
    let dirs = [None, dir_start(raw, g, &DIR_LOGS), dir_start(raw, g, &DIR_SCOPE)];
    for (i, d) in dirs.iter().enumerate() {
        if i > 0 && d.is_none() {
            continue;
        }
        let ok = each_entry(raw, g, *d, &mut |e| {
            if let Some(x) = file_entry(e).filter(|x| dlg_name(&x.name).is_some()) {
                f(&x);
            }
            true
        });
        if !ok {
            return false;
        }
    }
    true
}

/// "LOG001  DLG" -> "LOG001  CSV".
pub fn csv_name(n: &[u8; 11]) -> [u8; 11] {
    let mut c = *n;
    c[8..].copy_from_slice(b"CSV");
    c
}

// ------------------------------------------------------------- device glue

/// A `Raw` through a plain function (the SPI flash on the device, an image
/// in the host tests).
#[derive(Clone, Copy)]
pub struct FnRaw(pub fn(u32, &mut [u8]) -> bool);

impl Raw for FnRaw {
#[cfg_attr(target_os = "none", link_section = ".text.vdisk_rd")]
    fn read_at(&mut self, addr: u32, buf: &mut [u8]) -> bool {
        (self.0)(addr, buf)
    }
}

pub type DiskGen = Gen<FatFile<FnRaw>>;

#[cfg(target_os = "none")]
static mut FLASH: Option<crate::spiflash::SpiFlash> = None;

/// flash_read calls, bytes; kind-3 seeks that did not go on from the
/// cursor, chunk headers read, rows decoded (DBG:VDISK:RD?).
pub static FR: [core::sync::atomic::AtomicU32; 5] = [const { core::sync::atomic::AtomicU32::new(0) }; 5];

/// The SPI flash, raw (waits for a running erase / program first).
#[cfg(target_os = "none")]
#[cfg_attr(target_os = "none", link_section = ".text.vdisk_rd")]
pub fn flash_read(addr: u32, buf: &mut [u8]) -> bool {
    FR[0].fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    FR[1].fetch_add(buf.len() as u32, core::sync::atomic::Ordering::Relaxed);
    let f = unsafe { &mut *core::ptr::addr_of_mut!(FLASH) };
    f.get_or_insert_with(crate::spiflash::SpiFlash::new).read_at(addr, buf)
}

#[cfg(not(target_os = "none"))]
fn flash_read(_: u32, _: &mut [u8]) -> bool {
    false
}

static mut DISK: FnRaw = FnRaw(flash_read);

/// A cached generator in zero-initialised RAM (.bss): an `Option<Gen>`
/// static would be ~0.45 KiB of .data (its None is not all zero bits).
pub struct Slot<K: Copy + PartialEq> {
    ok: bool,
    key: core::mem::MaybeUninit<K>,
    gen: core::mem::MaybeUninit<DiskGen>,
}

impl<K: Copy + PartialEq> Slot<K> {
    pub const fn new() -> Self {
        Slot { ok: false, key: core::mem::MaybeUninit::uninit(), gen: core::mem::MaybeUninit::uninit() }
    }
    /// The cached generator's key and CSV cursor (`Gen::pos`), if any.
    #[cfg_attr(target_os = "none", link_section = ".text.vdisk_rd")]
    pub fn peek(&self) -> Option<(K, u32)> {
        self.ok.then(|| unsafe { (self.key.assume_init(), self.gen.assume_init_ref().pos()) })
    }
    pub fn clear(&mut self) {
        self.ok = false;
    }
    /// The generator for `key`, opened with `open` when it is not the cached one.
#[cfg_attr(target_os = "none", link_section = ".text.vdisk_rd")]
    pub fn get(&mut self, key: K, open: impl FnOnce() -> Option<DiskGen>) -> Option<&mut DiskGen> {
        if !(self.ok && unsafe { self.key.assume_init() } == key) {
            self.ok = false;
            self.gen.write(open()?);
            self.key.write(key);
            self.ok = true;
        }
        Some(unsafe { self.gen.assume_init_mut() })
    }
}

/// Host tests: read the "SPI disk" through `f`.
pub fn set_disk(f: fn(u32, &mut [u8]) -> bool) {
    unsafe { *core::ptr::addr_of_mut!(DISK) = FnRaw(f) };
}

/// The SPI disk's raw reader (the flash on the device, `set_disk`'s in tests).
pub fn disk() -> FnRaw {
    unsafe { *core::ptr::addr_of!(DISK) }
}

/// Open file `name` (8.3, a .DLG; in its directory or the root) of the
/// SPI disk.
#[cfg_attr(target_os = "none", link_section = ".text.vdisk_rd")]
pub fn open(name: &[u8; 11]) -> Option<DiskGen> {
    let mut d = disk();
    let g = geometry(&mut d)?;
    let e = find(&mut d, &g, name)?;
    Gen::open(FatFile::new(d, g, e.start, e.size), e.size)
}

/// The directory a file of ours lives in (fatdir.rs `dir_of`).
#[cfg_attr(target_os = "none", link_section = ".text.vdisk_rd")]
pub fn dir_of(n: &[u8; 11]) -> Option<[u8; 11]> {
    let up = |c: u8| c.to_ascii_uppercase();
    match (dlg_prefix(n), [up(n[8]), up(n[9]), up(n[10])]) {
        (Some(b"SCOP"), _) => Some(DIR_SCOPE),
        (Some(_), _) => Some(DIR_LOGS),
        (None, x) if &x == b"BMP" && up(n[0]) == b'S' => Some(DIR_SHOTS),
        _ => None,
    }
}

/// Entry `name` (8.3, case-insensitive; any file, not a directory): in its
/// directory (`dir_of`), else in the root.
#[cfg_attr(target_os = "none", link_section = ".text.vdisk_rd")]
pub fn find(raw: &mut dyn Raw, g: &Geo, name: &[u8; 11]) -> Option<Ent> {
    // A fn, not a closure (its own symbol would be region C: each_entry).
    #[cfg_attr(target_os = "none", link_section = ".text.vdisk_rd")]
    fn look(raw: &mut dyn Raw, g: &Geo, name: &[u8; 11], start: Option<u32>) -> Option<Ent> {
        let mut hit = None;
        each_entry(raw, g, start, &mut |e| {
            if let Some(x) = file_entry(e).filter(|x| x.name.eq_ignore_ascii_case(name)) {
                hit = Some(x);
                return false;
            }
            true
        });
        hit
    }
    if let Some(d) = dir_of(name).and_then(|d| dir_start(raw, g, &d)) {
        if let Some(x) = look(raw, g, name, Some(d)) {
            return Some(x);
        }
    }
    look(raw, g, name, None)
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DataErr {
    NoDisk,
    NotFound,
    Read,
}

/// The SCPI generator, kept between MMEM:DATA? calls (a host reads a file
/// in 6000-byte pieces: sequential, O(1) each) while the entry is unchanged.
static mut SCPI_GEN: Slot<Ent> = Slot::new();

/// MMEM:DATA?: bytes `off..` of root file `name` into `out` (count
/// returned, 0 at the end). "LOGnnn.CSV" with a LOGnnn.DLG next to it =
/// the CSV generated from the DLG; else the file's raw bytes.
#[inline(never)] // .cold (link-native.x): not inlined into the hot caller
#[cfg_attr(target_os = "none", link_section = ".text.vdisk_rd")]
pub fn data(name: &[u8; 11], off: u32, out: &mut [u8]) -> Result<usize, DataErr> {
    let mut d = disk();
    let g = geometry(&mut d).ok_or(DataErr::NoDisk)?;
    if name[8..].eq_ignore_ascii_case(b"CSV") {
        let mut dn = *name;
        dn[8..].copy_from_slice(b"DLG");
        if let Some(e) = find(&mut d, &g, &dn) {
            let cache = unsafe { &mut *core::ptr::addr_of_mut!(SCPI_GEN) };
            let gen = cache.get(e, || Gen::open(FatFile::new(d, g, e.start, e.size), e.size)).ok_or(DataErr::Read)?;
            let n = (gen.info.csv.saturating_sub(off) as usize).min(out.len());
            gen.read(off, &mut out[..n]);
            return Ok(n);
        }
    }
    let e = find(&mut d, &g, name).ok_or(DataErr::NotFound)?;
    let n = (e.size.saturating_sub(off) as usize).min(out.len());
    let mut f = FatFile::new(d, g, e.start, e.size);
    if n > 0 && !f.read(off, &mut out[..n]) {
        return Err(DataErr::Read);
    }
    Ok(n)
}

#[cfg(feature = "vdisk")]
pub use vfs::*;

/// When the generated USB volume (vdisk.rs) must be rebuilt: a recording
/// ended, a screenshot / scope record was written, a file was deleted.
#[cfg(feature = "vdisk")]
mod vfs {
    use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

    static STALE: AtomicBool = AtomicBool::new(true);
    static GEN: AtomicU32 = AtomicU32::new(0);

    /// The SPI files changed: vdisk::refresh is due (main.rs runs it once
    /// the disk has been quiet for a second: `gen` stops moving).
    pub fn set_stale() {
        STALE.store(true, Ordering::Relaxed);
        GEN.fetch_add(1, Ordering::Relaxed);
    }

    pub fn stale() -> bool {
        STALE.load(Ordering::Relaxed)
    }

    pub fn clear_stale() {
        STALE.store(false, Ordering::Relaxed);
    }

    /// Bumped by every `set_stale` (debounce).
    pub fn gen() -> u32 {
        GEN.load(Ordering::Relaxed)
    }
}
