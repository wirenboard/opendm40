//! Coded sample ring: the scope record at the slow timebases (fs <=
//! scope::PK_FS) and in the Wave view, in the bytes the packed ring had
//! (scope.rs BUF below the staging ring).
//!
//! Samples are numbered from 0 at `reset` (u32, wrapping). Block b holds
//! samples b * BLK .. (b + 1) * BLK (dlog::SBLK, the DLG scope block). The
//! newest block (the head) is written as packed 10-bit codes (pack10.rs
//! layout from the block's first byte); once full it is coded in place as
//! a DLG scope block (dlog.rs, SEnc's bits: a fixed predictor, adaptive
//! Rice; docs/compact-log-format.md "Scope record") when that is smaller
//! and the coded bytes never overtake the packed codes still to be read;
//! else it stays packed (5120 bytes). Both are known from `plan`, run code
//! by code as the head fills (the order: the one with the least sum of
//! |residual| over the block before), so a full block costs only its
//! coding (`code_block`). The next block starts after it. Blocks are
//! byte runs of a circular buffer of `rb` bytes; when the head needs room
//! the oldest block goes: a coded one whole, a packed one 4 samples at a
//! time. So the ring holds at least rb * 8 / 10 samples (all blocks
//! packed: white noise) and up to NB blocks of a compressible signal.
//!
//! Reads: `at(n)`, packed blocks directly, coded ones through a decoder
//! cursor that goes forward (sequential reads cost one sample each; a read
//! behind it decodes its block again from the start). `view` is a
//! pack10::Codes over the ring (cap 0; pack10::CODED reads it).
//!
//! The 4 bytes after the ring mirror its first 4, so a packed read near
//! its end is one unaligned load. `rb` * 8 is a multiple of 10: while no
//! block was coded since `reset` the ring is the plain packed ring of
//! rb * 8 / 10 samples (`view` gives plain pack10 views, no decoder).

use crate::dlog::{self, Ad};
use crate::pack10::{self, Codes};

/// Samples per block.
pub const BLK: u32 = dlog::SBLK as u32;
/// Blocks at most (the record's bound: NB * BLK samples, dlog::SBLK_MAX).
pub const NB: usize = dlog::SBLK_MAX;
/// Bytes of a packed block.
const PB: u32 = BLK * 10 / 8;
/// Codes read ahead of the coder (`la_get`): the coded bytes may run up to
/// LA codes ahead of the packed ones they replace.
const LA: u32 = 64;
/// Codes the cursor keeps behind it (`c_hist`, across a block start when
/// it read on into the next block): a read up to this far back needs no
/// new decode from a block's start. The sinc interpolation steps back 6
/// codes a column, the protocol decoder's start edge search up to a
/// quarter bit.
const HIST: u32 = 16;

struct Ring {
    base: *mut u8,
    rb: u32,
    /// Coding of full blocks wanted (else every block stays packed); a
    /// block was coded since `reset`.
    compress: bool,
    ever: bool,
    /// Start byte (virtual: monotonic, physical = % rb) of block b at
    /// [b % NB]; bit b % NB of `coded`: block b is coded.
    start: [u32; NB],
    coded: u64,
    /// Oldest block and the samples dropped from its start (packed only);
    /// the head block and its samples; the oldest live byte (virtual).
    ob: u32,
    s0: u32,
    hb: u32,
    hj: u32,
    tail: u32,
    /// Decoder cursor: block, its start, samples decoded, the last one,
    /// physical byte, bit buffer, mode, history, Rice state.
    c_ok: bool,
    c_b: u32,
    c_start: u32,
    c_j: u32,
    c_x: u16,
    c_pos: u32,
    c_acc: u64,
    c_nb: u32,
    c_mode: u32,
    c_h: [i32; 3],
    c_ad: Ad,
    /// The last codes decoded: sample n at [n % HIST], the h_len (<= HIST)
    /// samples before h_end.
    c_hist: [u16; HIST as usize],
    h_end: u32,
    h_len: u32,
    /// The codes read ahead while a block is coded in place.
    la: [u16; LA as usize],
    /// The head block's coding planned code by code (`plan`) in fixed
    /// order `p_o` (the one with the least sum of |residual| over the
    /// block before: `p_sum` per order): its bits so far, Rice state,
    /// still possible (smaller than packed and safe in place); the last
    /// code and its 1st / 2nd differences.
    p_o: u32,
    p_sum: [u32; 4],
    p_bits: u32,
    p_ad: Ad,
    p_ok: bool,
    p_x: i32,
    p_d1: i32,
    p_d2: i32,
}

/// All zero (.bss): `init` / `reset` set what needs a value. TCM, not
/// SRAM: SRAM is at its 8 KiB floor, the stack has room (~0.5 KB).
static mut R: Ring = Ring {
    base: core::ptr::null_mut(),
    rb: 0,
    compress: false,
    ever: false,
    start: [0; NB],
    coded: 0,
    ob: 0,
    s0: 0,
    hb: 0,
    hj: 0,
    tail: 0,
    c_ok: false,
    c_b: 0,
    c_start: 0,
    c_j: 0,
    c_x: 0,
    c_pos: 0,
    c_acc: 0,
    c_nb: 0,
    c_mode: 0,
    c_h: [0; 3],
    c_ad: Ad { a: 0, n: 0 },
    c_hist: [0; HIST as usize],
    h_end: 0,
    h_len: 0,
    la: [0; LA as usize],
    p_o: 0,
    p_sum: [0; 4],
    p_bits: 0,
    p_ad: Ad { a: 0, n: 0 },
    p_ok: false,
    p_x: 0,
    p_d1: 0,
    p_d2: 0,
};

/// dlog::Ad's k / upd, inlined in the per-code loops (`plan`, the coder,
/// the decoder): the same rule.
#[inline(always)]
fn ak(ad: &Ad) -> u32 {
    if ad.a <= ad.n {
        return 0;
    }
    let k0 = (ad.n.leading_zeros() - (ad.a - 1).leading_zeros()).min(30);
    if k0 < 30 && (ad.n << k0) < ad.a { k0 + 1 } else { k0 }
}
#[inline(always)]
fn aupd(ad: &mut Ad, x: u32) {
    ad.a = ad.a.saturating_add(x.min(1 << 24));
    ad.n += 1;
    if ad.n >= 32 {
        ad.a = (ad.a + 1) >> 1;
        ad.n >>= 1;
    }
}

fn r() -> &'static mut Ring {
    // SAFETY: main loop only, no reference kept across calls.
    unsafe { &mut *core::ptr::addr_of_mut!(R) }
}

fn coded_at(n: u32) -> u16 {
    at(n)
}

/// The buffer: `rb` bytes at `base` and 4 more (the mirror). Once.
pub fn init(base: *mut u8, rb: u32) {
    let s = r();
    s.base = base;
    s.rb = rb;
    // SAFETY: set once before any coded view exists.
    unsafe {
        *core::ptr::addr_of_mut!(pack10::CODED) = coded_at;
        *core::ptr::addr_of_mut!(pack10::CODED_MM) = min_max;
    }
    reset(false);
}

/// Empty the ring; sample 0 is the next one pushed. `compress`: code the
/// full blocks.
pub fn reset(compress: bool) {
    let s = r();
    s.compress = compress;
    s.ever = false;
    s.start[0] = 0;
    s.coded = 0;
    (s.ob, s.s0, s.hb, s.hj, s.tail) = (0, 0, 0, 0, 0);
    s.c_ok = false;
    s.h_len = 0;
    s.plan_reset();
}

/// First live sample, the next one to be pushed, live samples.
pub fn first() -> u32 {
    let s = r();
    s.ob.wrapping_mul(BLK).wrapping_add(s.s0)
}
pub fn end() -> u32 {
    let s = r();
    s.hb.wrapping_mul(BLK).wrapping_add(s.hj)
}
pub fn live() -> u32 {
    end().wrapping_sub(first())
}

/// Block b's physical start byte, its byte count and whether it is coded
/// (host tests).
#[cfg(not(target_os = "none"))]
pub fn block(b: u32) -> (u32, u32, bool) {
    let s = r();
    let st = s.start[b as usize % NB];
    let len = if b == s.hb { (10 * s.hj).div_ceil(8) } else { s.start[(b as usize + 1) % NB].wrapping_sub(st) };
    (s.phys(st), len, s.is_coded(b))
}

/// Samples `off .. off + len` as a view.
pub fn view(off: u32, len: usize) -> Codes {
    let s = r();
    if !s.ever {
        // Every block packed since `reset`: sample n is at bit 10 n of the
        // ring (rb * 8 is a multiple of 10): the plain packed ring.
        let cap = s.rb * 8 / 10;
        return Codes::new(s.base, (off % cap) as usize, len, cap as usize);
    }
    Codes::new(core::ptr::null(), off as usize, len, 0)
}

impl Ring {
    #[inline(always)]
    fn is_coded(&self, b: u32) -> bool {
        self.coded >> (b as usize % NB) & 1 != 0
    }
    #[inline(always)]
    fn phys(&self, v: u32) -> u32 {
        v % self.rb
    }
    /// Packed code `j` of the block starting at virtual byte `st`.
    #[inline(always)]
    fn pget(&self, st: u32, j: u32) -> u16 {
        let bit = 10 * j;
        let p = self.phys(st.wrapping_add(bit >> 3));
        // SAFETY: p < rb; the mirror covers p + 3 < rb + 4.
        (unsafe { core::ptr::read_unaligned(self.base.add(p as usize) as *const u32) } >> (bit & 7)) as u16 & 0x3FF
    }
    /// Byte at virtual `v` (and its mirror).
    #[inline(always)]
    fn wbyte(&mut self, v: u32, x: u8) {
        let p = self.phys(v) as usize;
        // SAFETY: p < rb, the mirror at rb .. rb + 4.
        unsafe {
            *self.base.add(p) = x;
            if p < 4 {
                *self.base.add(self.rb as usize + p) = x;
            }
        }
    }
    #[inline(always)]
    fn rbyte(&self, v: u32) -> u8 {
        unsafe { *self.base.add(self.phys(v) as usize) }
    }
    /// A new head block: plan it in the order that suited the last one.
    fn plan_reset(&mut self) {
        if self.p_sum != [0; 4] {
            self.p_o = (0..4u32).min_by_key(|&o| self.p_sum[o as usize]).unwrap_or(1);
        }
        self.p_sum = [0; 4];
        self.p_bits = 3;
        self.p_ad = Ad::new(dlog::AS0);
        self.p_ok = true;
    }

    /// Code `x` (the head's code hj): the |residual| of every fixed order
    /// (for the next block's order), and in order p_o its bits (as SEnc
    /// puts them) and whether the coded bytes stay behind the packed codes
    /// the in-place coder still has to read (`code_block`): bytes(bits up
    /// to j) <= the first byte of j + LA.
    #[inline(always)]
    fn plan(&mut self, x: i32) {
        let j = self.hj;
        // Residuals of orders 0..3 (s_pred): x - 512 and the 1st..3rd
        // differences; code j < order uses order j.
        let e1 = x - self.p_x;
        let e2 = e1 - self.p_d1;
        let e3 = e2 - self.p_d2;
        (self.p_x, self.p_d1, self.p_d2) = (x, e1, e2);
        let r = [x - 512, e1, e2, e3];
        let jj = j.min(3) as usize;
        for o in 0..4 {
            self.p_sum[o] += r[o.min(jj)].unsigned_abs();
        }
        if !self.p_ok {
            return;
        }
        let u = dlog::zz(r[(self.p_o as usize).min(jj)]);
        let k = ak(&self.p_ad);
        let q = u >> k;
        let b = self.p_bits + if q < dlog::QMAX { q + 1 + k } else { dlog::QMAX + 32 };
        aupd(&mut self.p_ad, u);
        self.p_bits = b;
        let lim = if j + LA < BLK { 10 * (j + LA) / 8 * 8 + 7 } else { 8 * PB - 1 };
        if b > lim {
            self.p_ok = false;
        }
    }

    /// Drop the oldest samples (a coded block whole, 4 of a packed one).
    fn drop_old(&mut self) -> bool {
        if self.ob == self.hb {
            return false;
        }
        if self.is_coded(self.ob) || self.s0 + 4 >= BLK {
            if self.c_b == self.ob {
                self.c_ok = false;
            }
            self.ob = self.ob.wrapping_add(1);
            self.s0 = 0;
            self.tail = self.start[self.ob as usize % NB];
        } else {
            self.s0 += 4;
            self.tail = self.start[self.ob as usize % NB].wrapping_add(self.s0 / 4 * 5);
        }
        true
    }
}

/// Append one 10-bit code.
#[cfg_attr(target_os = "none", link_section = ".text.cring")]
pub fn push(c: u16) {
    let s = r();
    let st = s.start[s.hb as usize % NB];
    let bit = 10 * s.hj;
    // Room for its bytes (3: the read-modify-write below) in the ring.
    let need = st.wrapping_add(bit >> 3).wrapping_add(3);
    while need.wrapping_sub(s.tail) > s.rb {
        if !s.drop_old() {
            break;
        }
    }
    let v = st.wrapping_add(bit >> 3);
    let sh = bit & 7;
    let p = s.phys(v);
    if p >= 4 && p + 4 <= s.rb {
        // pack10::set: away from the ends (no wrap, no mirror).
        pack10::set_at(s.base, p as usize, sh, c);
    } else {
        set_slow(v, sh, c);
    }
    if s.compress {
        s.plan((c & 0x3FF) as i32);
    }
    s.hj += 1;
    if s.hj == BLK {
        full_block();
    }
}

/// The head block is full: code it (when wanted, smaller and safe in
/// place), start the next one after it.
#[cfg_attr(target_os = "none", link_section = ".text.cring")]
fn full_block() {
    let s = r();
    let hb = s.hb;
    let st = s.start[hb as usize % NB];
    let mut len = PB;
    if s.compress {
        if let Some(n) = code_block(st) {
            len = n;
            s.coded |= 1 << (hb as usize % NB);
            s.ever = true;
        }
    }
    let nb = hb.wrapping_add(1);
    // NB blocks with the new head at most.
    while nb.wrapping_sub(s.ob) >= NB as u32 {
        if s.c_b == s.ob {
            s.c_ok = false;
        }
        s.ob = s.ob.wrapping_add(1);
        s.s0 = 0;
        s.tail = s.start[s.ob as usize % NB];
    }
    s.start[nb as usize % NB] = st.wrapping_add(len);
    s.coded &= !(1 << (nb as usize % NB));
    (s.hb, s.hj) = (nb, 0);
    s.plan_reset();
}

/// Code the packed block at `st` in place: its byte count, or None (kept
/// packed: no smaller, or the coded bytes would overtake a packed code not
/// read yet). The coding is dlog's scope block (SEnc's bits: the order
/// with the least sum of |residual|, dlog::scope_order's rule). Its codes
/// are read LA ahead into `la` before the bytes up to code j are written,
/// so bytes(bits up to j) <= the first byte of code j + LA must hold for
/// every j (a dry run checks it, and the size, first).
#[cfg_attr(target_os = "none", link_section = ".text.cring")]
fn code_block(st: u32) -> Option<u32> {
    let s = r();
    // The order planned (`plan`), if it codes the block in place.
    if !s.p_ok {
        return None;
    }
    let o = s.p_o;
    // A block that does not wrap (all but one a turn) is read and written
    // as plain bytes, no modulo per code.
    let p0 = s.phys(st);
    let lin = (p0 + PB + 4 <= s.rb).then(|| unsafe { s.base.add(p0 as usize) });
    // Code: MSB first (SEnc's bits), whole bytes out after each part.
    let mut ad = Ad::new(dlog::AS0);
    let (mut acc, mut nb, mut out) = (o, 3u32, 0u32);
    // n <= 24 bits, with nb < 8 before: fits the u32.
    macro_rules! put {
        ($v:expr, $n:expr) => {{
            let n: u32 = $n;
            acc = (acc << n) | ($v & ((1u32 << n) - 1));
            nb += n;
            while nb >= 8 {
                nb -= 8;
                let b = (acc >> nb) as u8;
                match lin {
                    Some(p) => unsafe { *p.add(out as usize) = b },
                    None => wslow(st.wrapping_add(out), b),
                }
                out += 1;
            }
        }};
    }
    let mut next = 0u32;
    let (mut x1, mut d1, mut d2) = (0i32, 0i32, 0i32);
    for j in 0..BLK {
        while next < (j + LA).min(BLK) {
            s.la[(next % LA) as usize] = s.pget(st, next);
            next += 1;
        }
        let x = s.la[(j % LA) as usize] as i32;
        // The residual of order o (code j < o: order j), as in `plan`.
        let e1 = x - x1;
        let e2 = e1 - d1;
        let e3 = e2 - d2;
        let res = match o.min(j) {
            0 => x - 512,
            1 => e1,
            2 => e2,
            _ => e3,
        };
        (x1, d1, d2) = (x, e1, e2);
        let u = dlog::zz(res);
        let k = ak(&ad);
        let q = u >> k;
        if q < dlog::QMAX {
            // q ones, a zero, k bits (k < 24 here: q < QMAX of a u32).
            put!(((1u32 << q) - 1) << 1, q + 1);
            put!(u, k);
        } else {
            put!((1u32 << dlog::QMAX) - 1, dlog::QMAX);
            put!(u >> 16, 16);
            put!(u, 16);
        }
        aupd(&mut ad, u);
    }
    if nb > 0 {
        put!(0u32, 8 - nb);
    }
    // The mirror of the ring's first bytes (wbyte keeps it; the plain
    // writes do not).
    if lin.is_some() && p0 < 4 {
        for i in p0..4 {
            unsafe { *s.base.add((s.rb + i) as usize) = *s.base.add(i as usize) };
        }
    }
    Some(out)
}

/// A packed code across the ring's end or in its mirrored first bytes.
#[inline(never)]
#[cold]
fn set_slow(v: u32, sh: u32, c: u16) {
    let s = r();
    let w = s.rbyte(v) as u32 | (s.rbyte(v.wrapping_add(1)) as u32) << 8 | (s.rbyte(v.wrapping_add(2)) as u32) << 16;
    let w = (w & !(0x3FF << sh)) | ((c & 0x3FF) as u32) << sh;
    s.wbyte(v, w as u8);
    s.wbyte(v.wrapping_add(1), (w >> 8) as u8);
    s.wbyte(v.wrapping_add(2), (w >> 16) as u8);
}

/// A coded byte of a block that wraps (out of line: the coder's loop
/// stays small).
#[inline(never)]
#[cold]
fn wslow(v: u32, b: u8) {
    r().wbyte(v, b);
}

/// Code `n` (a live sample; another reads as 512).
#[cfg_attr(target_os = "none", link_section = ".text.cring")]
pub fn at(n: u32) -> u16 {
    let s = r();
    // The cursor's next code (sequential reads): one step.
    if s.c_ok && n == s.c_b.wrapping_mul(BLK).wrapping_add(s.c_j) && s.c_j < BLK {
        s.step();
        return s.c_x;
    }
    if n.wrapping_sub(s.ob.wrapping_mul(BLK).wrapping_add(s.s0)) >= s.hb.wrapping_mul(BLK).wrapping_add(s.hj).wrapping_sub(s.ob.wrapping_mul(BLK).wrapping_add(s.s0)) {
        return 512;
    }
    let (b, j) = (n / BLK, n % BLK);
    let st = s.start[b as usize % NB];
    if b == s.hb || !s.is_coded(b) {
        return s.pget(st, j);
    }
    // A code just behind the cursor (sinc taps, an edge search): from the
    // history, the cursor stays where it is (`min_max` steps it on).
    if s.h_end.wrapping_sub(n).wrapping_sub(1) < s.h_len {
        return s.c_hist[(n % HIST) as usize];
    }
    s.dec(b, st, j)
}

impl Ring {
    /// At least 57 bits in the decoder's buffer (a code is at most 56).
    #[inline(always)]
    fn fill(&mut self) {
        while self.c_nb <= 56 {
            // SAFETY: c_pos < rb.
            self.c_acc = self.c_acc << 8 | unsafe { *self.base.add(self.c_pos as usize) } as u64;
            self.c_pos += 1;
            if self.c_pos == self.rb {
                self.c_pos = 0;
            }
            self.c_nb += 8;
        }
    }
    /// The next `n` (<= 32) bits.
    #[inline(always)]
    fn take(&mut self, n: u32) -> u32 {
        self.c_nb -= n;
        ((self.c_acc >> self.c_nb) & ((1u64 << n) - 1)) as u32
    }

    /// Code j of coded block b (start st) through the cursor.
    #[cfg_attr(target_os = "none", link_section = ".text.cring")]
    fn dec(&mut self, b: u32, st: u32, j: u32) -> u16 {
        if !(self.c_ok && self.c_b == b && self.c_start == st && self.c_j >= 1 && self.c_j - 1 <= j) {
            self.c_ok = true;
            (self.c_b, self.c_start, self.c_j) = (b, st, 0);
            self.c_pos = self.phys(st);
            (self.c_acc, self.c_nb) = (0, 0);
            self.fill();
            self.c_mode = self.take(3);
            self.c_h = [0; 3];
            self.c_ad = Ad::new(dlog::AS0);
        }
        while self.c_j <= j {
            self.step();
        }
        self.c_x
    }

    /// Decode the cursor's next code (into c_x). One copy (`at`, `dec`,
    /// `min_max` call it).
    #[inline(never)]
    #[cfg_attr(target_os = "none", link_section = ".text.cring")]
    fn step(&mut self) {
        {
            self.fill();
            let x = if self.c_mode >= 4 {
                self.take(10) as i32
            } else {
                // Rice: q ones (QMAX: then 32 bits), a zero, k bits.
                let k = ak(&self.c_ad);
                let q = (!(self.c_acc << (64 - self.c_nb))).leading_zeros().min(dlog::QMAX);
                self.c_nb -= q;
                let u = if q == dlog::QMAX {
                    self.take(32)
                } else {
                    self.c_nb -= 1;
                    (q << k) | self.take(k)
                };
                aupd(&mut self.c_ad, u);
                dlog::s_pred(self.c_mode, self.c_j as usize, &self.c_h) + dlog::unzz(u)
            };
            self.c_h = [x, self.c_h[0], self.c_h[1]];
            self.c_x = (x & 0x3FF) as u16;
            let n = self.c_b.wrapping_mul(BLK).wrapping_add(self.c_j);
            if n != self.h_end {
                self.h_len = 0;
            }
            self.c_hist[(n % HIST) as usize] = self.c_x;
            self.h_end = n.wrapping_add(1);
            self.h_len = (self.h_len + 1).min(HIST);
            self.c_j += 1;
        }
    }
}

/// Smallest and largest code of samples `off .. off + len` (live ones),
/// block by block in one loop each ((u16::MAX, 0) when empty): the
/// render's columns and the zoomed-out redraw of a coded record.
#[cfg_attr(target_os = "none", link_section = ".text.cring")]
pub fn min_max(off: u32, len: usize) -> (u16, u16) {
    let s = r();
    let (mut lo, mut hi) = (u16::MAX, 0u16);
    let f = s.ob.wrapping_mul(BLK).wrapping_add(s.s0);
    let live = s.hb.wrapping_mul(BLK).wrapping_add(s.hj).wrapping_sub(f);
    // Clip to the live samples.
    let a = off.wrapping_sub(f);
    if a >= live {
        return (lo, hi);
    }
    let mut n = off;
    let mut left = (len as u32).min(live - a);
    while left > 0 {
        let (b, j) = (n / BLK, n % BLK);
        let k = (BLK - j).min(left);
        let st = s.start[b as usize % NB];
        if b == s.hb || !s.is_coded(b) {
            for i in j..j + k {
                let v = s.pget(st, i);
                lo = lo.min(v);
                hi = hi.max(v);
            }
        } else {
            let v = s.dec(b, st, j);
            (lo, hi) = (lo.min(v), hi.max(v));
            for _ in 1..k {
                s.step();
                (lo, hi) = (lo.min(s.c_x), hi.max(s.c_x));
            }
        }
        n = n.wrapping_add(k);
        left -= k;
    }
    (lo, hi)
}
