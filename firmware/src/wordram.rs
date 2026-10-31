//! RAM that takes only whole 32-bit words: the USBHS FIFO RAM at 0x40060000
//! (link-native.x `.usbhs_bss`; docs/native-boot.md "Extra RAM areas").
//!
//! On the device that window ignores the transfer size of a store: a byte
//! store writes the whole word with the byte in all four lanes (`strb 0x31`
//! leaves `31 31 31 31`), so a byte-written buffer there is garbage. Reads
//! of any size and 32-bit stores are fine.
//!
//! The only way to place a buffer there is `usbhs_static!`, which declares a
//! `WordRam`; its words are reached only through `read` / `write`, one
//! volatile 32-bit access each (no slice, no `Deref`, no `fill` that the
//! compiler could turn into a byte memset). build.sh fails when
//! `.usbhs_bss` is named anywhere else in src/.

use core::cell::UnsafeCell;

/// `N` words of word-only RAM. Little-endian byte `b` of the buffer is bits
/// `8 * (b % 4)..` of word `b / 4`.
#[repr(C, align(4))]
pub struct WordRam<const N: usize>(UnsafeCell<[u32; N]>);

// Main loop only (no ISR touches a WordRam; build.sh checks the AD2 stream
// ISR holds no address in the area).
unsafe impl<const N: usize> Sync for WordRam<N> {}

impl<const N: usize> WordRam<N> {
    pub const fn new() -> Self {
        WordRam(UnsafeCell::new([0; N]))
    }

    #[inline(always)]
    pub fn read(&self, i: usize) -> u32 {
        assert!(i < N);
        unsafe { (self.0.get() as *const u32).add(i).read_volatile() }
    }

    #[inline(always)]
    pub fn write(&self, i: usize, v: u32) {
        assert!(i < N);
        unsafe { (self.0.get() as *mut u32).add(i).write_volatile(v) }
    }

    /// Every word = `v`, one word store each.
    pub fn fill_words(&self, v: u32) {
        for i in 0..N {
            self.write(i, v);
        }
    }

    /// Bytes `off..off + n` from `next()`, in order. A word only partly
    /// covered is read, merged in a register and written back whole.
    pub fn put_bytes(&self, off: usize, n: usize, mut next: impl FnMut() -> u8) {
        let end = off + n;
        assert!(end <= 4 * N);
        let mut b = off;
        while b < end {
            let w = b / 4;
            let lo = b % 4;
            let hi = (end - 4 * w).min(4);
            let mut v = if lo == 0 && hi == 4 { 0 } else { self.read(w) };
            for k in lo..hi {
                let s = 8 * k as u32;
                v = (v & !(0xFF << s)) | (next() as u32) << s;
            }
            self.write(w, v);
            b = 4 * w + hi;
        }
    }
}

/// The data of one block write (msc.rs `BlockDevice::write_step`): a byte
/// slice or a `WordRam`, read as little-endian words.
pub trait BlockSrc {
    /// Bytes `4 * i..4 * i + 4` as a little-endian word.
    fn word(&self, i: usize) -> u32;
    /// Length in bytes.
    fn len(&self) -> usize;
    /// The bytes as a slice, where they may be read so (not a `WordRam`).
    fn bytes(&self) -> Option<&[u8]>;
}

impl<const N: usize> BlockSrc for [u8; N] {
    #[inline]
    fn word(&self, i: usize) -> u32 {
        u32::from_le_bytes([self[4 * i], self[4 * i + 1], self[4 * i + 2], self[4 * i + 3]])
    }
    fn len(&self) -> usize {
        N
    }
    fn bytes(&self) -> Option<&[u8]> {
        Some(self)
    }
}

impl<const N: usize> BlockSrc for WordRam<N> {
    #[inline]
    fn word(&self, i: usize) -> u32 {
        self.read(i)
    }
    fn len(&self) -> usize {
        4 * N
    }
    fn bytes(&self) -> Option<&[u8]> {
        None
    }
}

/// `$vis static $name: WordRam<$n>` in the USBHS FIFO RAM (on the device;
/// plain `.bss` in a host build). The only place that names the section.
macro_rules! usbhs_static {
    ($(#[$m:meta])* $vis:vis static $name:ident: WordRam<$n:tt>;) => {
        $(#[$m])*
        #[cfg_attr(target_os = "none", link_section = ".usbhs_bss")]
        $vis static $name: crate::wordram::WordRam<$n> = crate::wordram::WordRam::new();
    };
}
pub(crate) use usbhs_static;
