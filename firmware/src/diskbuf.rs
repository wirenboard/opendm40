//! The disk buffer arena (docs/ram-budget.md "The disk buffer arena"): 3 x
//! 4 KiB block buffers shared by every on-device disk user, lent to the
//! scope FFT and the strip renderer while no disk job holds it. Kept off
//! the TCM stack (the writers run deep in the main loop). NOLOAD SRAM
//! (.dmabuf): undefined at reset, always written before it is read.
//!
//! Disk users take their block buffers here (`blocks`): the boot safety
//! copy before main() (calbackup.rs), then screenshot.rs (shots, scope
//! records), csvlog.rs commits and its catalog / delete / free-space
//! helpers, fastlog.rs Prep / Stop, upload.rs, vwrite.rs, gallery.rs,
//! fatdir.rs, setfile.rs. One disk user at a time (the disk lock: each
//! waits / refuses while another's job is in flight, see those modules); a
//! job keeps blocks here across main-loop passes.
//!
//! Second tenant: the scope FFT's work buffer (`fft_work`, 2048 f32 = the
//! first 8 KiB), needed only while a spectrum job is in flight. Ownership
//! with runtime checks, no aliasing by hope:
//!   * a disk user never waits for the FFT: `blocks` is the disk side's
//!     only accessor and bumps `GEN` on every call;
//!   * the FFT gets the buffer only from `fft_work`, which returns None
//!     while a disk job that keeps data here across passes is in flight
//!     (`disk_job`), and, for a job already loaded (`fresh` false), when
//!     `GEN` moved since the FFT's load (a disk user wrote here in between,
//!     e.g. a gallery read inside one pass): the FFT then drops its job and
//!     asks for the frame again (scope_page.rs);
//!   * both sides take the reference inside one call and keep none across
//!     passes (main loop only; no ISR touches the arena).
//!
//! Third tenant: the strip renderer (`strip_take` / `strip_give`).

#![cfg_attr(not(target_os = "none"), allow(dead_code))]

use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

/// One block buffer: an SPI-flash block / FAT cluster.
pub const BLK: usize = 4096;

#[repr(C, align(4))]
struct Arena([[u8; BLK]; 3]);
#[cfg_attr(target_os = "none", link_section = ".dmabuf")]
static mut ARENA: Arena = Arena([[0; BLK]; 3]);
/// Disk-side uses of the arena (`blocks` calls); FFT_GEN = GEN at the FFT's
/// `fft_work(true)` (its load).
static GEN: AtomicU32 = AtomicU32::new(0);
static FFT_GEN: AtomicU32 = AtomicU32::new(0);
/// f32 entries of the FFT work buffer (= fft::N, checked in scope_page.rs;
/// this file is also built without fft.rs by the host tests).
pub const FFT_WORK: usize = 2048;
const _: () = assert!(FFT_WORK * 4 <= 3 * BLK);

/// The disk side's block buffers. Takes the arena from the FFT (a spectrum
/// job in flight is dropped at its next step).
#[inline(always)]
pub(crate) fn blocks() -> &'static mut [[u8; BLK]; 3] {
    GEN.store(GEN.load(Ordering::Relaxed).wrapping_add(1), Ordering::Relaxed);
    unsafe { &mut (*core::ptr::addr_of_mut!(ARENA)).0 }
}

/// A disk job that keeps data in the arena across main-loop passes is in
/// flight: a screenshot / scope record, a csvlog commit, a fast-log session
/// (Prep .. Stop; conservative, only Prep / Stop use the arena), an upload
/// or a firmware file staged from the USB volume. Single-pass users
/// (gallery reads, csvlog catalog / delete, vwrite commit) are caught by
/// `GEN` instead.
#[cfg(target_os = "none")]
#[inline(always)]
fn disk_job() -> bool {
    crate::screenshot::busy() || crate::csvlog::writing() || crate::fastlog::holds_disk() || crate::upload::busy()
}
#[cfg(not(target_os = "none"))]
fn disk_job() -> bool {
    false
}

/// The FFT work buffer, or None: a disk job holds the arena, or (`fresh`
/// false: a job already loaded) a disk user wrote it since the load.
/// `fresh` true = a new job that fills every entry it uses (fft.rs `load`).
/// Main loop only; the caller keeps no reference across passes.
#[inline(always)]
pub fn fft_work(fresh: bool) -> Option<&'static mut [f32; FFT_WORK]> {
    if disk_job() || STRIP_HELD.load(Ordering::Relaxed) {
        return None;
    }
    let g = GEN.load(Ordering::Relaxed);
    if fresh {
        FFT_GEN.store(g, Ordering::Relaxed);
    } else if FFT_GEN.load(Ordering::Relaxed) != g {
        return None;
    }
    let p = core::ptr::addr_of_mut!(ARENA).cast::<[f32; FFT_WORK]>();
    Some(unsafe { &mut *p })
}

/// Third tenant: the strip renderer's buffer (lcd::strips from
/// App::repaint), inside one call. Taken as a disk user takes it (`GEN`:
/// an FFT job in flight is redone) and refused while a disk job holds the
/// arena; the FFT gets nothing while it is held (a page draw can step the
/// FFT). A single-pass disk user that took the arena meanwhile (no drawing
/// code does; not ruled out by construction) moved `GEN`: `strip_give`
/// returns false and the caller draws the page again directly.
static STRIP_HELD: AtomicBool = AtomicBool::new(false);
static STRIP_GEN: AtomicU32 = AtomicU32::new(0);

/// The arena as `3 * BLK / 2` RGB565 pixels for the strip renderer, or
/// None (a disk job holds it). Give it back with `strip_give`.
#[inline(always)]
pub(crate) fn strip_take() -> Option<&'static mut [u16; 3 * BLK / 2]> {
    if disk_job() {
        return None;
    }
    let _ = blocks();
    STRIP_GEN.store(GEN.load(Ordering::Relaxed), Ordering::Relaxed);
    STRIP_HELD.store(true, Ordering::Relaxed);
    let p = core::ptr::addr_of_mut!(ARENA).cast::<[u16; 3 * BLK / 2]>();
    Some(unsafe { &mut *p })
}

/// End of the strip renderer's use; false: a disk user took the arena in
/// between (the strips sent may be wrong).
#[inline(always)]
pub(crate) fn strip_give() -> bool {
    STRIP_HELD.store(false, Ordering::Relaxed);
    STRIP_GEN.load(Ordering::Relaxed) == GEN.load(Ordering::Relaxed)
}

/// No disk job holds the arena (the FFT may load a new job).
#[inline(always)]
pub fn fft_free() -> bool {
    !disk_job()
}
