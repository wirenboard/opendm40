//! 10-bit sample codes packed 4 in 5 bytes: a little-endian bit stream,
//! sample i at bits 10 i .. 10 i + 9 (byte 10 i / 8, shift 10 i % 8), so
//! any sample is one unaligned 32-bit load. The scope record is kept in
//! this form (scope.rs `BUF`); `Codes` is a view of it (a window of a
//! ring: index `off + i`, wrapping at `cap`), `Src` the read interface
//! its users take (also implemented for plain u16 slices, for the host
//! tests).
//!
//! A load reads up to 3 bytes past the last sample's byte: the buffer
//! holds those (scope.rs).

/// Read access to a run of 10-bit codes.
pub trait Src {
    fn len(&self) -> usize;
    fn at(&self, i: usize) -> u16;
}

impl Src for [u16] {
    fn len(&self) -> usize {
        <[u16]>::len(self)
    }
    fn at(&self, i: usize) -> u16 {
        self[i]
    }
}

impl Src for &[u16] {
    fn len(&self) -> usize {
        <[u16]>::len(self)
    }
    fn at(&self, i: usize) -> u16 {
        self[i]
    }
}

impl<const N: usize> Src for [u16; N] {

    fn len(&self) -> usize {
        N
    }
    fn at(&self, i: usize) -> u16 {
        self[i]
    }
}

#[cfg(not(target_os = "none"))]
impl Src for std::vec::Vec<u16> {
    fn len(&self) -> usize {
        <[u16]>::len(self)
    }
    fn at(&self, i: usize) -> u16 {
        self[i]
    }
}

/// Code `n` of the coded ring (cring.rs; its `init` sets this): what a
/// view with `cap` 0 reads, `off + i` its sample number.
pub static mut CODED: fn(u32) -> u16 = no_coded;

fn no_coded(_: u32) -> u16 {
    512
}

/// Smallest and largest code of `n .. n + len` of the coded ring
/// (cring::min_max).
pub static mut CODED_MM: fn(u32, usize) -> (u16, u16) = no_coded_mm;

fn no_coded_mm(_: u32, _: usize) -> (u16, u16) {
    (u16::MAX, 0)
}

/// `CODED(n)`, out of line (Codes::at is inlined in every reader).
#[inline(never)]
fn coded(n: u32) -> u16 {
    // SAFETY: CODED is set once at start-up.
    unsafe { (*core::ptr::addr_of!(CODED))(n) }
}

/// Sample `i` of the packed stream at `base`.
#[inline(always)]
pub fn get(base: *const u8, i: usize) -> u16 {
    let b = i * 10;
    // SAFETY: the caller keeps i inside the buffer (+ 3 bytes of slack).
    (unsafe { core::ptr::read_unaligned(base.add(b >> 3) as *const u32) } >> (b & 7)) as u16 & 0x3FF
}

/// Store sample `i` (low 10 bits of `v`) of the packed stream at `base`
/// (host tests; the scope writes cring.rs's blocks: `set_at`).
#[cfg(not(target_os = "none"))]
#[inline(always)]
pub fn set(base: *mut u8, i: usize, v: u16) {
    let b = i * 10;
    set_at(base, b >> 3, (b & 7) as u32, v);
}

/// Store a code (low 10 bits of `v`) at byte `byte`, bit `sh` (< 8).
#[inline(always)]
pub fn set_at(base: *mut u8, byte: usize, sh: u32, v: u16) {
    // SAFETY: the caller keeps byte .. byte + 3 inside the buffer; main
    // loop only (no DMA writes these bytes).
    unsafe {
        let p = base.add(byte) as *mut u32;
        let w = core::ptr::read_unaligned(p);
        core::ptr::write_unaligned(p, (w & !(0x3FF << sh)) | (((v & 0x3FF) as u32) << sh));
    }
}

/// Pack the `n` raw u16 samples at `p` (10-bit codes in the low bits, the
/// rest masked off) in place, from the start: group g (4 samples) is
/// written to bytes 5 g .. 5 g + 4 after its 8 raw bytes are read, and
/// 5 g + 4 < 8 g + 8, so no unread sample is overwritten. `n` a multiple
/// of 4 (a tail is left as it is).
#[inline(never)]
pub fn pack_in_place(p: *mut u8, n: usize) {
    for g in 0..n / 4 {
        // SAFETY: the caller owns p[..2 n]; reads before the writes.
        unsafe {
            let r = p.add(8 * g) as *const u32;
            let a = core::ptr::read_volatile(r);
            let b = core::ptr::read_volatile(r.add(1));
            let lo = (a & 0x3FF) | ((a >> 16) & 0x3FF) << 10 | (b & 0x3FF) << 20 | (b >> 16) << 30;
            let hi = ((b >> 18) & 0xFF) as u8;
            let w = p.add(5 * g);
            core::ptr::write_unaligned(w as *mut u32, lo);
            w.add(4).write(hi);
        }
    }
}

/// Packed codes `off .. off + len` of a ring of `cap` samples at `base`
/// (index wraps at `cap`; off < cap, len <= cap); `cap` 0: samples `off ..
/// off + len` of the coded ring (`CODED`).
#[derive(Clone, Copy)]
pub struct Codes {
    base: *const u8,
    off: usize,
    len: usize,
    cap: usize,
}

// SAFETY: a view into the static scope buffer, main loop only.
unsafe impl Sync for Codes {}

impl Codes {
    pub const EMPTY: Codes = Codes { base: core::ptr::null(), off: 0, len: 0, cap: 0 };

    pub const fn new(base: *const u8, off: usize, len: usize, cap: usize) -> Codes {
        Codes { base, off, len, cap }
    }

    #[inline(always)]
    pub fn len(&self) -> usize {
        self.len
    }

    #[inline(always)]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Code `i` (0 .. len).
    #[inline(always)]
    pub fn at(&self, i: usize) -> u16 {
        if self.cap == 0 {
            return coded(self.off.wrapping_add(i) as u32);
        }
        let mut j = self.off + i;
        if j >= self.cap {
            j -= self.cap;
        }
        get(self.base, j)
    }

    /// Codes a .. b of this view (b clamped to len).
    #[inline(never)]
    pub fn sub(&self, a: usize, b: usize) -> Codes {
        let b = b.min(self.len);
        let a = a.min(b);
        let mut o = self.off.wrapping_add(a);
        if self.cap != 0 && o >= self.cap {
            o -= self.cap;
        }
        Codes { base: self.base, off: o, len: b - a, cap: self.cap }
    }

    /// Where `w` (a view of the same ring) starts in this one, if it lies
    /// wholly inside it.
    pub fn offset_of(&self, w: &Codes) -> Option<usize> {
        if w.base != self.base || w.cap != self.cap || self.len == 0 {
            return None;
        }
        let o = if self.cap == 0 { w.off.wrapping_sub(self.off) as u32 as usize } else { (w.off + self.cap - self.off) % self.cap };
        (o + w.len <= self.len).then_some(o)
    }

    /// Smallest and largest code of a .. b ((u16::MAX, 0) when empty).
    #[inline(never)]
    pub fn min_max(&self, a: usize, b: usize) -> (u16, u16) {
        if self.cap == 0 {
            let b = b.min(self.len);
            if a >= b {
                return (u16::MAX, 0);
            }
            // SAFETY: CODED_MM is set once at start-up.
            return unsafe { (*core::ptr::addr_of!(CODED_MM))(self.off.wrapping_add(a) as u32, b - a) };
        }
        let (mut lo, mut hi) = (u16::MAX, 0u16);
        for i in a..b.min(self.len) {
            let v = self.at(i);
            lo = lo.min(v);
            hi = hi.max(v);
        }
        (lo, hi)
    }
}

impl Src for Codes {
    #[inline(always)]
    fn len(&self) -> usize {
        self.len
    }
    #[inline(always)]
    fn at(&self, i: usize) -> u16 {
        Codes::at(self, i)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pack_get_set() {
        let n = 64;
        let mut buf = std::vec![0u16; n + 2];
        for i in 0..n {
            buf[i] = ((i * 37 + 5) % 1024) as u16 | 0xFC00;
        }
        let p = buf.as_mut_ptr() as *mut u8;
        pack_in_place(p, n);
        for i in 0..n {
            assert_eq!(get(p, i), ((i * 37 + 5) % 1024) as u16);
        }
        set(p, 5, 1023);
        set(p, 6, 1);
        assert_eq!((get(p, 4), get(p, 5), get(p, 6), get(p, 7)), (((4 * 37 + 5) % 1024) as u16, 1023, 1, ((7 * 37 + 5) % 1024) as u16));
        let c = Codes::new(p, 60, 8, 64);
        assert_eq!(c.at(3), get(p, 63));
        assert_eq!(c.at(4), get(p, 0));
        assert_eq!(c.sub(2, 6).at(2), get(p, 0));
    }
}
