//! Free watchdog timer (FWDGT on GD32F427 = IWDG layout).
//!
//! Once we take over with interrupts off, the stock TIM7 1 kHz ISR that used to
//! reload the watchdog no longer runs, so the main loop must do it. Reload is a
//! write of 0xAAAA to the key register at 0x40003000 (verified in the stock
//! firmware: FUN_08005dfc writes 0xAAAA there).

use core::sync::atomic::{AtomicBool, Ordering};

use crate::mmio::w32;

const FWDGT_CTL: u32 = 0x4000_3000;
const KEY_RELOAD: u32 = 0x0000_AAAA;

#[inline(always)]
pub fn kick() {
    unsafe { w32(FWDGT_CTL, KEY_RELOAD) };
}

static FEEDING: AtomicBool = AtomicBool::new(true);

/// Whether the main loop currently feeds the watchdog (debug builds stop
/// feeding past the keepalive deadline so the device falls back to stock).
pub fn set_feeding(on: bool) {
    FEEDING.store(on, Ordering::Relaxed);
}

/// Kick from inside a long blocking operation, only while the main loop would.
pub fn kick_if_feeding() {
    if FEEDING.load(Ordering::Relaxed) {
        kick();
    }
}

/// End of app.rs power_off() once the rails are dropped. Still powered means
/// USB power, so do not let the FWDGT turn this into a
/// "hang" (a watchdog reset): backlight off, keep feeding, and a new press of the
/// power key (after the off-press is released) restarts opendm40 normally.
#[inline(always)]
pub fn powered_off() -> ! {
    off_wait()
}

fn off_wait() -> ! {
    unsafe { w32(0x4000_1C34, 0) }; // TIM13 CCR1: backlight off
    let mut armed = false;
    let mut n = 0u32;
    loop {
        kick();
        crate::mmio::spin(20_000); // ~0.3 ms
        if crate::boot::pd6_pressed() {
            n = 0;
            if armed {
                crate::boot::sysreset();
            }
        } else {
            n += 1;
            armed |= n > 200; // released for ~60 ms
        }
    }
}

// --- Native boot: the shim starts the FWDGT itself -------------------------
// Option bytes nWDG_HW = 1 (software watchdog), so nothing guarantees it runs
// when the FS-loader jumps to us. Stock configuration: PSC = 6 (/256),
// RLD = 2500 (~16..20 s on the ~32 kHz IRC32K; measured 19 s).

pub mod start {
    use crate::mmio::{r32, w32};

    const PSC: u32 = 0x4000_3004;
    const RLD: u32 = 0x4000_3008;
    const STAT: u32 = 0x4000_300C; // bit 0 PUD, bit 1 RUD: update in progress
    pub const PSC_DIV256: u32 = 6;
    pub const RELOAD: u32 = 2500;

    /// Start (or keep) the watchdog with the stock timeout and reload it. Once
    /// started it cannot be stopped until reset. Bounded waits only.
    #[link_section = ".shim"]
    #[inline(never)]
    pub fn fwdgt() {
        unsafe {
            w32(super::FWDGT_CTL, 0xCCCC); // start (no-op when already running)
            w32(super::FWDGT_CTL, 0x5555); // unlock PSC / RLD
            // A previous update may still be in flight (writes are then lost).
            let mut n = 0u32;
            while r32(STAT) & 3 != 0 && n < 0x10_0000 {
                n += 1;
            }
            w32(PSC, PSC_DIV256);
            w32(RLD, RELOAD);
            w32(super::FWDGT_CTL, super::KEY_RELOAD);
        }
    }
}
