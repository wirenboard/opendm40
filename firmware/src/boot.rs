//! Native boot decision (the "shim"), called by the native reset path
//! (src/shim.rs).
//!
//! Runs before .data/.bss exist: no statics are touched until `native::start`
//! has initialised RAM (only `set_info`/`info` below use one, afterwards).
//! Every wait is bounded and feeds the FWDGT. Every cap fails toward a
//! normal boot: a key-gesture pass cap counts as a RELEASE, never as a held
//! key (review N1).
//!
//! Two ways into the vendor FS-loader's update screen:
//! - PA2 entry (`loader_pa2`, DBG:LOADER / DBG:RECOVER, the key hold): PA2
//!   driven low and locked, then a jump to the ROM's reset handler. The ROM
//!   and the loader see PA2 low for their whole poll, as with the TX-GND short.
//!   The marker stays valid: a power cycle (or the loader's power-off) boots
//!   opendm40 again.
//! - Marker zeroing (`enter_loader_update`, DBG:UPDATE, the on-meter
//!   install of a copied image): a one-way door, the loader stays until a good
//!   install. Used only where a staged image must be installed.
//!
//! Decision, in this order (docs/native-boot.md "Shim (reset path)"):
//!   backup-SRAM request word == LOADER_MAGIC  -> PA2 entry (consumed; first
//!     thing, before the clock or the watchdog are touched)
//!   key gesture confirmed (below)             -> request word LOADER_MAGIC,
//!                                                SYSRESETREQ (-> PA2 entry)
//!   key held to the cue, released in time     -> normal   (KeyHoldAborted)
//!   clock init failed (HXTAL and IRC16M), or a GENUINE watchdog reset after
//!     our own normal boot: crash counter += 1 (shown by DBG:BOOT?);
//!       clock failed, counter < CLOCK_RETRIES -> SYSRESETREQ, retry (ClockFail)
//!       clock failed                          -> normal   (ClockFail)
//!       else                                  -> normal   (WdtRetry)
//!   FWDGTRSTF, previous boot entered the loader -> normal (WdtAfterLoader)
//!   FWDGTRSTF otherwise                       -> normal   (WdtStale)
//!   otherwise                                 -> normal
//! "Genuine" = FWDGTRSTF set and neither SWRSTF nor PORRSTF: the shim clears
//! the flags on every native boot, so a hang during a native session shows
//! FWDGT alone, while a loader install adds SW and a power-on adds POR (a
//! stale FWDGT left over from a stock session then does not count). The crash
//! counter is cleared by `mark_healthy` after 60 s of main loop.
//!
//! Key gesture: the backlight comes on at once (instant feedback, so a normal
//! power-on press is released early). Holding the power key HOLD_MS (4 s)
//! after the shim started gives a cue (beep + backlight blink); the key must
//! then STILL be held CONFIRM_MS (0.5 s) to enter the loader. Releasing during
//! that window aborts to a normal boot.

use crate::clock::{self, Src};
use crate::mmio::{modify32, r32, w32};

pub const RCU_RSTSCK: u32 = 0x4002_3874;
/// RCU_RSTSCK (GD32F4xx RCU layout, = STM32 RCC_CSR): write 1 to clear flags.
pub const RSTFC: u32 = 1 << 24;
#[allow(dead_code)]
pub const BORRSTF: u32 = 1 << 25;
#[allow(dead_code)]
pub const EPRSTF: u32 = 1 << 26;
pub const PORRSTF: u32 = 1 << 27;
#[allow(dead_code)]
pub const SWRSTF: u32 = 1 << 28;
pub const FWDGTRSTF: u32 = 1 << 29;

const RCU_AHB1EN: u32 = 0x4002_3830;
const RCU_APB1EN: u32 = 0x4002_3840;
const PMU_CTL: u32 = 0x4000_7000;
const PMU_CS: u32 = 0x4000_7004;
const GPIOA: u32 = 0x4002_0000;
const GPIOD: u32 = 0x4002_0C00;
const GPIOE: u32 = 0x4002_1000;
const SCB_AIRCR: u32 = 0xE000_ED0C;
const DEMCR: u32 = 0xE000_EDFC;
const DWT_CTRL: u32 = 0xE000_1000;
const DWT_CYCCNT: u32 = 0xE000_1004;

/// PD6 (power key) level when pressed: 1. Confirmed in the stock V2.6 code:
/// FUN_0800b7e0 sets PD6 input with pull-down (gpio_mode_set(GPIOD 0x40020C00,
/// 0, PUPD 2, 0x40)); the TIM7 1 kHz ISR FUN_0800b87c reads IDR & 0x40
/// (FUN_08006000) and raises the key-down event (state 0 -> 1 -> 2, flag bit 0)
/// only while the bit reads 1 (app_decompile.c:6524-6556). Idle measured 0.
pub const PD6_PRESSED_HIGH: bool = true;
/// Hold the power key this long after the shim started -> cue.
pub const HOLD_MS: u32 = 4000;
/// ... and still this long after the cue -> the loader (PA2 entry).
pub const CONFIRM_MS: u32 = 500;
/// A release shorter than this is contact bounce, not a release.
const RELEASE_DEBOUNCE_MS: u32 = 30;
/// A clock failure retries the boot (SYSRESETREQ) while the crash counter is
/// below this; then opendm40 runs on the clock it has.
pub const CLOCK_RETRIES: u32 = 2;
/// Main-loop time after which `mark_healthy` clears the crash counter.
// (60 s of RUNNING main loop since this boot, not 60 s between two resets)
pub const HEALTHY_MS: u64 = 60_000;

/// Backup SRAM (0x40024000, 4 KiB). persist.rs uses the start: record v4 =
/// magic, version, 40 words, CRC = 43 words, 0x40024000..0x400240AB; these
/// boot words are the last four of the 4 KiB, 0x40024FF0..0x40024FFF (no
/// overlap; checked at compile time in persist.rs).
/// Crash counter (`CRASH_TAG | n`).
pub const BKP_CRASH: u32 = 0x4002_4FF0;
const CRASH_TAG: u32 = 0x4352_0000; // "CR"
/// Last loader entry (0x54520012 PA2 entry, 0x54520013 PA2 entry without the
/// GPIO lock).
pub const BKP_TRIG: u32 = 0x4002_4FF4;
pub const BKP_REQ: u32 = 0x4002_4FF8;
pub const BKP_LAST: u32 = 0x4002_4FFC;
/// BKP_REQ value for the PA2 entry into the loader (`loader_pa2`).
pub const LOADER_MAGIC: u32 = 0x4C32_4150; // "PA2L" LE
/// BKP_TRIG after the PA2 entry: GPIOA lock confirmed / not confirmed.
pub const TRIG_PA2: u32 = 0x5452_0012;
pub const TRIG_PA2_NOLOCK: u32 = 0x5452_0013;
const LAST_NORMAL: u32 = 0x4C4D_524E; // "NRML"
/// BKP_LAST after the PA2 entry into the loader.
const LAST_LOADER: u32 = 0x5256_4352; // "RCVR"

/// DBG:BOOT? field 1; the numbers are the reply's (5 was the removed
/// "recovery requested").
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    None = 0,
    WdtAfterLoader = 1,
    WdtStale = 2,
    WdtRetry = 3,
    KeyHoldAborted = 4,
    ClockFail = 6,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ClockState {
    Hxtal,
    Irc16m,
    Failed,
}

#[derive(Clone, Copy)]
pub struct BootInfo {
    /// RCU_RSTSCK as found (before RSTFC), and as read back after clearing.
    pub rstsck: u32,
    pub rstsck_after: u32,
    pub reason: Reason,
    pub clock: ClockState,
    /// Previous boot's mode word (BKP_LAST) as found.
    pub last: u32,
}

#[inline(always)]
pub fn feed() {
    crate::wdt::kick();
}

/// Power key pressed (PD6, GPIOD clock + input mode set by `pd6_input`).
#[inline(always)]
pub fn pd6_pressed() -> bool {
    (unsafe { r32(GPIOD + 0x10) } & (1 << 6) != 0) == PD6_PRESSED_HIGH
}

/// PD6 input WITH pull-down, as the stock key init does
/// (app_decompile.c:6482, gpio_mode_set(GPIOD, input, PUPD 2, 0x40)). After a
/// reset the ROM/loader leave PD6 floating (PUPD 00); without the pull a
/// floating 1 would read as a held key. Then let the pull settle.
fn pd6_input() {
    unsafe {
        modify32(RCU_AHB1EN, 0, 1 << 3); // GPIOD
        modify32(GPIOD + 0x0C, 3 << 12, 2 << 12); // PUPD: pull-down
        modify32(GPIOD, 3 << 12, 0); // input
    }
    // >= 10 us at any clock up to 200 MHz (internal pull ~40 kOhm, pin few pF).
    crate::mmio::spin(4000);
}

/// Backlight full on as plain GPIO (PA6 high): instant feedback that the unit
/// is on, long before the LCD is initialised (board.rs later hands PA6 to
/// TIM13 with the duty at 0 until App::new sets the level).
fn backlight(on: bool) {
    unsafe {
        modify32(RCU_AHB1EN, 0, 1 << 0); // GPIOA
        w32(GPIOA + 0x18, if on { 1 << 6 } else { 1 << (6 + 16) });
        modify32(GPIOA + 0x04, 1 << 6, 0);
        modify32(GPIOA, 3 << 12, 1 << 12);
    }
}

/// Cue of the key-hold gesture: backlight off and a ~0.3 s beep (buzzer PA7
/// toggled as plain GPIO at ~2.7 kHz, the stock drives the same pin with
/// TIM14 PWM at 3.57 kHz), then backlight on again.
fn cue(cyc_per_ms: u32) {
    backlight(false);
    unsafe {
        modify32(GPIOA + 0x04, 1 << 7, 0);
        w32(GPIOA + 0x18, 1 << (7 + 16));
        modify32(GPIOA, 3 << 14, 1 << 14);
    }
    let half = cyc_per_ms * 10 / 54; // ~185 us
    // Pass cap per half period (review N1): with a stopped cycle counter the
    // cue still ends after 1620 x half/2 passes (~1 s). The cue decides nothing.
    let cap = half / 2;
    let mut level = false;
    for _ in 0..1620 {
        // 1620 half periods = ~0.3 s
        let t = cyccnt();
        let mut n = 0u32;
        while cyccnt().wrapping_sub(t) < half && n < cap {
            n += 1;
        }
        level = !level && !cfg!(feature = "mute");
        unsafe { w32(GPIOA + 0x18, if level { 1 << 7 } else { 1 << (7 + 16) }) };
        feed();
    }
    unsafe { w32(GPIOA + 0x18, 1 << (7 + 16)) };
    backlight(true);
}

/// Power hold: PE2 = 1. The ROM already raises PE2 and PC4 first thing on
/// every reset (0x08000574), the stock app keeps PE2 (FUN_0800b410); this
/// re-asserts it. PE2 is the DMM analog supply and, with PC4 (scope supply),
/// what keeps the unit on: the stock power-off drops both. Output level is set
/// before the mode so the pin never drives low.
pub fn latch_on() {
    unsafe {
        modify32(RCU_AHB1EN, 0, 1 << 4); // GPIOE
        w32(GPIOE + 0x18, 1 << 2);
        modify32(GPIOE + 0x04, 1 << 2, 0);
        modify32(GPIOE + 0x08, 0, 3 << 4);
        modify32(GPIOE + 0x0C, 3 << 4, 0);
        modify32(GPIOE, 3 << 4, 1 << 4);
    }
}

/// Backup SRAM access (as persist::init): PMU clock, BKPWEN, BKPSRAM clock,
/// backup regulator on.
/// Region B (not the boot module's .cold): `mark_healthy` calls it once,
/// 60 s after the start, maybe while a stream runs.
#[cfg_attr(target_os = "none", link_section = ".text.bkp_enable")]
pub fn bkp_enable() {
    unsafe {
        modify32(RCU_APB1EN, 0, 1 << 28);
        modify32(PMU_CTL, 0, 1 << 8);
        modify32(RCU_AHB1EN, 0, 1 << 18);
        modify32(PMU_CS, 0, 1 << 9);
    }
}

// --- The 12-byte loader marker at 0x08008000 ------------------------------
//
// The ROM (0x08000980) and the FS-loader (0x080a3c88) launch the app only if
// this record is valid: word2 type 0x0001 (bytes 10/11 = 01 00), word0 low
// half == ~high half == CRC16 of word1 || word2, and the body CRC matches. The
// loader writes it last after a good install. Zeroing it (what the loader
// itself does first when it finds any dm40*.bin, 0x080a1815) makes both
// reject the app on ANY reset, cold or warm, independent of PA2 (the debug
// header's TX pad, the hardware boot strap: hw/README.md), so the loader
// stays in its update screen with its own working USB disk until a valid
// dm40*.bin is installed (stock/re/docs/fsloader-install-trigger.md, "Entry without a file: zeroing the marker").

pub const MARKER: u32 = 0x0800_8000;
const FMC_KEY: u32 = 0x4002_3C04;
const FMC_STAT: u32 = 0x4002_3C0C;
const FMC_CTL: u32 = 0x4002_3C10;
const FMC_UNLOCK1: u32 = 0x4567_0123;
const FMC_UNLOCK2: u32 = 0xCDEF_89AB;
/// FMC_STAT: BUSY bit 16; error bits as the loader tests them (OPERR 1,
/// WPERR 4, PGMERR 5, PGAERR 6, PGSERR 7, RDDERR 8). The loader clears 0xD3.
const STAT_BUSY: u32 = 1 << 16;
const STAT_ERR: u32 = 0x1F2;
/// Clear every error bit we test (the loader writes 0xD3, which leaves
/// PGMERR bit 5 set if it was).
const STAT_CLEAR: u32 = 0x1F3;
/// FMC_CTL: PG bit 0, PSZ bits 9:8 (0b10 = 32-bit), LK bit 31.
const CTL_PG: u32 = 1;
const CTL_PSZ_MASK: u32 = 0x300;
const CTL_PSZ_32: u32 = 0x200;
const CTL_LK: u32 = 1 << 31;

/// The three marker words.
pub fn marker() -> [u32; 3] {
    unsafe { [r32(MARKER), r32(MARKER + 4), r32(MARKER + 8)] }
}

/// The marker passes the ROM/loader header test (type 1, complement, CRC;
/// fatren::header_valid). The body CRC (also checked by them) is not
/// recomputed here.
pub fn marker_valid() -> bool {
    let mut h = [0u8; 12];
    for (d, w) in h.chunks_mut(4).zip(marker()) {
        d.copy_from_slice(&w.to_le_bytes());
    }
    crate::fatren::header_valid(&h)
}

fn fmc_wait() -> bool {
    let mut n = 0u32;
    while unsafe { r32(FMC_STAT) } & STAT_BUSY != 0 {
        n += 1;
        if n & 0xFF == 0 {
            feed();
        }
        if n >= 0x10_0000 {
            return false; // a word program takes ~16 us; this is > 100 ms
        }
    }
    true
}

/// FMC status of the last `zero_marker` (error bits, 0x8000_0000 = timeout).
static mut FMC_LAST: u32 = 0;

/// Program the 12 bytes at 0x08008000..0x0800800B to 0x00 (no erase: only
/// 1 -> 0 bit changes, as the loader does), relock. Touches no other flash
/// address. The read-back is the result: true iff all three words read 0
/// (FMC error flags are only recorded, see `fmc_last`). Runs after RAM init.
pub fn zero_marker() -> bool {
    feed();
    let mut st = 0u32;
    unsafe {
        if r32(FMC_CTL) & CTL_LK != 0 {
            w32(FMC_KEY, FMC_UNLOCK1);
            w32(FMC_KEY, FMC_UNLOCK2);
        }
        if !fmc_wait() {
            st |= 0x8000_0000;
        } else {
            w32(FMC_STAT, STAT_CLEAR);
            modify32(FMC_CTL, CTL_PSZ_MASK, CTL_PSZ_32);
            modify32(FMC_CTL, 0, CTL_PG);
            for i in 0..3u32 {
                let a = MARKER + 4 * i;
                if r32(a) != 0 {
                    w32(a, 0);
                    core::arch::asm!("dsb");
                    if !fmc_wait() {
                        st |= 0x8000_0000;
                        break;
                    }
                    st |= r32(FMC_STAT) & STAT_ERR;
                }
                feed();
            }
            modify32(FMC_CTL, CTL_PG, 0);
        }
        modify32(FMC_CTL, 0, CTL_LK);
        FMC_LAST = st;
    }
    marker() == [0, 0, 0]
}

/// Error bits / timeout of the last `zero_marker` (0 = clean).
pub fn fmc_last() -> u32 {
    unsafe { FMC_LAST }
}

/// DBG:UPDATE (native): forget the previous-mode word (so a
/// later stale watchdog flag cannot count as our crash, review H3) and reset
/// the crash counter (so a count left from this session cannot survive the
/// stock interlude and count as a crash after the reinstall, review N2), then zero the marker. The caller resets (deferred SYSRESETREQ)
/// into the loader's update screen. Returns the read-back result.
pub fn enter_loader_update() -> bool {
    bkp_enable();
    unsafe {
        w32(BKP_LAST, 0);
        w32(BKP_CRASH, CRASH_TAG);
    }
    zero_marker()
}

/// DBG:LOADER / DBG:RECOVER: ask the next shim pass for the PA2 entry
/// (`loader_pa2`); the caller resets (deferred SYSRESETREQ). The marker is
/// not touched.
pub fn request_loader() {
    bkp_enable();
    unsafe { w32(BKP_REQ, LOADER_MAGIC) };
}

// --- PA2 entry into the vendor loader --------------------------------------
//
// The ROM (0x080005f4) and the FS-loader (0x080a1c0e) each set PA2 to input
// with pull-up (gpio_mode_set: MODER and PUPD rewritten) and poll IDR bit 2
// 1000 times; one high read launches a valid app. Neither resets GPIOA
// (no RCU_AHB1RST write) nor touches GPIOA_LCKR. So PA2 configured as a
// push-pull output at 0 and then LOCKED (GPIOx_LOCK, offset 0x1C: MODER,
// OTYPER, OSPEEDR, PUPDR and AFR of the pin frozen until the next system
// reset; the output data register is not locked, and nothing in the ROM or
// the loader writes PA2's output bit) keeps PA2 low through both polls. The
// loader then shows its update screen; it installs a dm40*.bin found in the
// SPI-flash root as on a TX-GND power-on. A system reset (the loader after an
// install) or a power cycle ends the lock: PA2 reads high again, the marker
// is valid, opendm40 boots.
//
// Called first thing in the shim after a reset, so the state is the one the
// ROM leaves when it launches the app: its clock (PLL 200 MHz), its FWDGT
// (the ROM sets it up again), no interrupt enabled. The jump repeats what the
// ROM's own reset path expects: VTOR 0x08000000, MSP from its vector table,
// PRIMASK clear, CONTROL 0.

const GPIOA_LCKR: u32 = GPIOA + 0x1C;
const LCKR_LKK: u32 = 1 << 16;
const ROM_BASE: u32 = 0x0800_0000;
const SCB_VTOR: u32 = 0xE000_ED08;
const SCB_ICSR: u32 = 0xE000_ED04;
const SYST_CSR: u32 = 0xE000_E010;
const NVIC_ICER: u32 = 0xE000_E180;
const NVIC_ICPR: u32 = 0xE000_E280;

/// PA2 output low, locked; then the ROM's reset handler. Never returns.
#[link_section = ".shim"]
#[inline(never)]
fn loader_pa2() -> ! {
    unsafe {
        // Consumed: a reset from here on (the loader's install, the FWDGT
        // after its power-off on USB power) boots normally.
        w32(BKP_REQ, 0);
        w32(BKP_LAST, LAST_LOADER);
        modify32(RCU_AHB1EN, 0, 1 << 0); // GPIOA
        w32(GPIOA + 0x18, 1 << (2 + 16)); // PA2 = 0 before the mode
        modify32(GPIOA + 0x04, 1 << 2, 0); // push-pull
        modify32(GPIOA + 0x0C, 3 << 4, 0); // no pull
        modify32(GPIOA, 3 << 4, 1 << 4); // output
        // Lock key sequence (GD32F4xx GPIOx_LOCK = STM32 GPIOx_LCKR).
        let pin = 1 << 2;
        w32(GPIOA_LCKR, LCKR_LKK | pin);
        w32(GPIOA_LCKR, pin);
        w32(GPIOA_LCKR, LCKR_LKK | pin);
        let _ = r32(GPIOA_LCKR);
        let locked = r32(GPIOA_LCKR) & (LCKR_LKK | pin) == LCKR_LKK | pin;
        // Without the lock the ROM's pull-up wins and it launches opendm40
        // again (a plain reboot); DBG:BOOT? shows which.
        w32(BKP_TRIG, if locked { TRIG_PA2 } else { TRIG_PA2_NOLOCK });
        // Nothing may interrupt the ROM before it sets itself up.
        w32(SYST_CSR, 0);
        for i in 0..8 {
            w32(NVIC_ICER + 4 * i, !0);
            w32(NVIC_ICPR + 4 * i, !0);
        }
        w32(SCB_ICSR, 1 << 25); // PENDSTCLR
        w32(SCB_VTOR, ROM_BASE);
        let sp = r32(ROM_BASE);
        let pc = r32(ROM_BASE + 4);
        core::arch::asm!(
            "dsb",
            "isb",
            "msr msp, {sp}",
            "msr control, {z}",
            "isb",
            "cpsie i",
            "bx {pc}",
            sp = in(reg) sp,
            pc = in(reg) pc,
            z = in(reg) 0u32,
            options(noreturn)
        );
    }
}

/// Clear the latched reset flags and reset by SYSRESETREQ (SW reset: the shim
/// then boots normally unless a request word is set).
pub fn sysreset() -> ! {
    unsafe {
        modify32(RCU_RSTSCK, 0, RSTFC);
        core::arch::asm!("dsb");
        w32(SCB_AIRCR, 0x05FA_0004);
    }
    loop {
        feed();
    }
}

#[inline(always)]
fn cyccnt() -> u32 {
    unsafe { r32(DWT_CYCCNT) }
}

/// Wait while PD6 stays pressed for up to `limit_ms` from `t0`. Returns the
/// time of a release (that lasted RELEASE_DEBOUNCE_MS), or None if the key was
/// held throughout.
///
/// Fail-safe cap (review N1): the loop is also bounded by a pass count (each
/// pass is >= 8 cycles: an FWDGT write, a GPIO read, a DWT read, so with a
/// running counter the time limit always comes first). Reaching the cap means
/// the cycle counter is not advancing; it returns `Some(limit_ms)`, i.e. a
/// RELEASE, never "held", so a stopped counter never confirms the gesture.
fn held_for(t0: u32, limit_ms: u32, cyc_per_ms: u32) -> Option<u32> {
    // (4000 + 100) x 25_000 = 1.03e8 at 200 MHz: no u32 overflow.
    let cap = (limit_ms + 100) * (cyc_per_ms / 8);
    let mut released_at: Option<u32> = None;
    let mut n = 0u32;
    loop {
        feed();
        n += 1;
        if n >= cap {
            return Some(limit_ms); // counter stuck: release / abort
        }
        let now = cyccnt();
        if now.wrapping_sub(t0) / cyc_per_ms >= limit_ms {
            return None;
        }
        if pd6_pressed() {
            released_at = None;
        } else {
            match released_at {
                None => released_at = Some(now),
                Some(r) if now.wrapping_sub(r) / cyc_per_ms >= RELEASE_DEBOUNCE_MS => {
                    return Some(r.wrapping_sub(t0) / cyc_per_ms);
                }
                _ => {}
            }
        }
    }
}

/// The key gesture: (held ms, confirmed). Not pressed -> (0, false);
/// released before HOLD_MS -> (ms, false); held to HOLD_MS -> cue, then
/// released within CONFIRM_MS -> (ms, false) = abort; still held -> confirmed.
fn key_gesture(cyc_per_ms: u32) -> (u32, bool) {
    if !pd6_pressed() {
        return (0, false);
    }
    if let Some(ms) = held_for(cyccnt(), HOLD_MS, cyc_per_ms) {
        return (ms, false);
    }
    cue(cyc_per_ms);
    match held_for(cyccnt(), CONFIRM_MS, cyc_per_ms) {
        Some(ms) => (HOLD_MS + ms, false),
        None => (HOLD_MS + CONFIRM_MS, true),
    }
}

/// The whole shim sequence.
#[link_section = ".shim"]
#[inline(never)]
pub fn shim() -> BootInfo {
    // (a) PA2 entry requested before this reset: first, in the state the ROM
    // left (its clock and watchdog), never returns.
    bkp_enable();
    if unsafe { r32(BKP_REQ) } == LOADER_MAGIC {
        loader_pa2();
    }
    // (b) watchdog first: from here on any hang resets into this shim.
    crate::wdt::start::fwdgt();
    // Keep the unit powered before anything slow (the key may be released),
    // and light the backlight at once (feedback for the power-on press).
    latch_on();
    backlight(true);
    // (c) latch and clear the sticky reset flags.
    let rstsck = unsafe { r32(RCU_RSTSCK) };
    unsafe { modify32(RCU_RSTSCK, 0, RSTFC) };
    let rstsck_after = unsafe { r32(RCU_RSTSCK) };
    pd6_input();
    // (d) clock: HXTAL, else the internal RC, else stay on 16 MHz.
    feed();
    let clock = if clock::init_core(Src::Hxtal) {
        ClockState::Hxtal
    } else if clock::init_core(Src::Irc16m) {
        ClockState::Irc16m
    } else {
        ClockState::Failed
    };
    feed();
    let cyc_per_ms = if clock == ClockState::Failed { 16_000 } else { clock::HCLK_HZ / 1000 };
    unsafe {
        modify32(DEMCR, 0, 1 << 24); // TRCENA
        modify32(DWT_CTRL, 0, 1); // CYCCNTENA
    }
    // (e) previous mode, crash counter, key hold.
    let last = unsafe { r32(BKP_LAST) };
    let crash = unsafe { r32(BKP_CRASH) };
    let crash = if crash & 0xFFFF_0000 == CRASH_TAG { crash & 0xFF } else { 0 };
    let (hold_ms, confirmed) = key_gesture(cyc_per_ms);
    if confirmed {
        // The PA2 entry runs on the next pass, from the ROM's state.
        unsafe { w32(BKP_REQ, LOADER_MAGIC) };
        sysreset()
    }

    let wdt = rstsck & FWDGTRSTF != 0;
    // Genuine = our own hang: FWDGT alone (no SW reset, no power-on) after a
    // native normal boot. Stale stock flags or an install never count.
    let genuine = wdt && rstsck & (SWRSTF | PORRSTF) == 0 && last == LAST_NORMAL;
    let failed = clock == ClockState::Failed;
    let reason = if hold_ms >= HOLD_MS {
        Reason::KeyHoldAborted
    } else if failed || genuine {
        // Counted for DBG:BOOT? only: a hang reboots opendm40, every time.
        let n = (crash + 1).min(0xFF);
        unsafe { w32(BKP_CRASH, CRASH_TAG | n) };
        if failed && n < CLOCK_RETRIES {
            // Transient clock failure: retry the whole boot (the ROM sets the
            // clock up again) instead of running opendm40 at 16 MHz.
            unsafe { w32(BKP_LAST, LAST_NORMAL) };
            sysreset()
        }
        if failed { Reason::ClockFail } else { Reason::WdtRetry }
    } else if wdt && last == LAST_LOADER {
        Reason::WdtAfterLoader
    } else if wdt {
        Reason::WdtStale
    } else {
        Reason::None
    };
    unsafe { w32(BKP_LAST, LAST_NORMAL) };
    feed();
    BootInfo { rstsck, rstsck_after, reason, clock, last }
}

// --- After RAM init: boot info for DBG:BOOT? --------------------------------

static mut INFO: Option<BootInfo> = None;
static mut BOARD: Option<crate::board::Before> = None;

pub fn set_info(i: BootInfo) {
    unsafe { INFO = Some(i) };
}

pub fn set_board(b: crate::board::Before) {
    unsafe { BOARD = Some(b) };
}

#[allow(dead_code)]
pub fn info() -> Option<BootInfo> {
    unsafe { INFO }
}

static mut HEALTHY: bool = false;

/// Call from the main loop every iteration with the uptime in ms: after
/// HEALTHY_MS of running it clears the crash counter once, so the counter
/// shows the hangs in a row (DBG:BOOT?).
// Call site: main.rs main loop (`now` = ms since main started, after pre_main).
pub fn mark_healthy(now_ms: u64) {
    if now_ms >= HEALTHY_MS && !unsafe { HEALTHY } {
        unsafe { HEALTHY = true };
        bkp_enable();
        unsafe { w32(BKP_CRASH, CRASH_TAG) };
    }
}

/// DBG:BOOT? reply: `B2,` then the words below in hex (no prefix, no
/// leading zeros), then `,` and fatren's renamed 8.3 name (11 characters,
/// blanks if none). One formatter for every field: names, enum orders and
/// meanings in docs/manual/scpi-reference.md ("DBG:BOOT?") and
/// tools/dm40dbg.py `BOOT_FIELDS` (`dm40dbg.py boot` decodes it).
///   0 mode (always 0; 1 was the removed recovery)  1 reason (`Reason`)  2 clock
///   (`ClockState` order)  3 rstsck  4 rstsck_after  5 last  6 last_trigger
///   (BKP_TRIG)  7..9 loader marker words  10 marker valid (1 / 0)
///   11 crash word  12 healthy (1 / 0)  13 fmc_last  14..17 FSMC BCR1 /
///   BTR1 / BWTR1 and the panel ID as found  18 fatren status
///   (`fatren::Status` order)  19 found  20 old_backup  21 old_deleted
///   22 freed_clusters  23 others  24 rcvr_deleted  25 staged
///   26.. native::boot_words.
#[inline(never)] // .cold (link-native.x): not inlined into the hot caller
pub fn put_info(s: &mut crate::scpi::Scpi) {
    s.len = 0;
    let Some(i) = (unsafe { INFO }) else {
        s.put_bytes(b"NONE");
        return;
    };
    let b = unsafe { BOARD }.unwrap_or_default();
    let r = crate::fatren::result();
    let m = marker();
    let head = [
        0, // mode: always normal
        i.reason as u32,
        i.clock as u32,
        i.rstsck,
        i.rstsck_after,
        i.last,
        // Last loader entry: 0x54520012 PA2 entry, 0x54520013 PA2 entry,
        // lock not confirmed (0 / other = none recorded).
        unsafe { r32(BKP_TRIG) },
        m[0],
        m[1],
        m[2],
        marker_valid() as u32,
        unsafe { r32(BKP_CRASH) },
        unsafe { HEALTHY } as u32,
        fmc_last(),
        b.bcr1,
        b.btr1,
        b.bwtr1,
        b.lcd_id,
        r.status as u32,
        r.found as u32,
        r.old_backup as u32,
        r.old_deleted as u32,
        r.freed_clusters,
        r.others as u32,
        r.fake_deleted as u32,
        r.staged as u32,
    ];
    s.put_bytes(b"B2");
    for v in head.iter().chain(crate::native::boot_words().iter()) {
        s.put_bytes(b",");
        let mut t = [0u8; 8];
        let mut n = t.len();
        let mut v = *v;
        loop {
            n -= 1;
            t[n] = b"0123456789ABCDEF"[(v & 15) as usize];
            v >>= 4;
            if v == 0 {
                break;
            }
        }
        s.put_bytes(&t[n..]);
    }
    s.put_bytes(b",");
    s.put_bytes(&r.renamed);
}
