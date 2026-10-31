//! HY3131 DMM front end on PC10 / PC11 / PC12 + CS PD2
//! (stock/re/analysis/hy3131_v2_6.h).
//!
//! SCK = PC10, MOSI = PC12, MISO = PC11, CS = PD2 (active low). MSB first, SCK
//! idle low, CS low for the whole transfer. Command byte = (reg << 1) | rd;
//! registers auto-increment. A read has one extra SCK pulse after the command
//! byte (the datasheet's read period), then MISO is sampled with SCK low before
//! each clock pulse.
//!
//! Reads and writes go over the GD32's hardware SPI2
//! (docs/dmm-input-scope-feasibility.md 2.2 and 8):
//!
//! SPI2 (reads): 0x4000_3C00 on APB1 (50 MHz), PC10 / PC11 / PC12 on AF6
//! (GD32F450xx datasheet Rev1.1 table 5; SPI2EN = RCU_APB1EN bit 15, SPI_CTL0
//! / SPI_STAT per GD32F4xx UM V2.2 21.11), master, mode 0 (HY3131 DS 7.1 /
//! 7.2: CK idle low, DO changes after the falling edge, sampled on the
//! rising one), software NSS, 8-bit frames, PSC /16 = 3.125 MHz. CS stays a
//! GPIO. Budget at 3.125 MHz (160 ns half period): DO path = isolator out
//! (CA-IS3741, ~10-15 ns, INFERRED) + TDODL <= 60 ns + back = ~80-90 ns:
//! ~70 ns margin. Device 2026-10-22 (6d35c53 DIAG image): 2000 x {1, 6, 23
//! byte} reads and 256 values read back clean at /8, /16 and /32.
//! The read period (the 9th clock) cannot come out of a byte-wide SPI: the
//! command byte goes out on SPI2, PC10 is switched to GPIO output for one
//! hand-made SCK pulse (ODR = 0 = the SPI idle level: no edge on either
//! switch) and back to AF, then the data bytes: the exact datasheet frame.
//!
//! Writes: SPI2 too, one frame per register: [cmd, data, dummy], CS high
//! while the dummy shifts (docs/dmm-input-scope-feasibility.md 11.6 / 11.7).
//! The chip takes each byte's LSB late, at the MOSI level that follows it
//! (0d0fd76's byte-by-byte SPI2 write: every 1xxxxxx0 arrived with bit 0 =
//! 1, the GD32 driving the byte's MSB again after it; in a continuous frame
//! the next byte's MSB): the dummy's MSB equals the LSB. ~10 us per register
//! at /16 (bit-banged: ~57 us; that write is gone since 42cbf35's device
//! check).
//!
//! Reads at /8 sample the data phase at CKPH = 1 (`set_data_cpha`, the AD2
//! stream; feasibility 11.3); the DMM reads at /16 mode 0.

use crate::mmio::{modify32, r32, spin, w16, w32};
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU8, Ordering::Relaxed};

const GPIOC: u32 = 0x4002_0800;
const GPIOD: u32 = 0x4002_0C00;
const MODER: u32 = 0x00;
const OTYPER: u32 = 0x04;
const PUPD: u32 = 0x0C;
const BOP: u32 = 0x18;

const SCK: u32 = 10; // PC10
const MISO: u32 = 11; // PC11
const MOSI: u32 = 12; // PC12
const CS: u32 = 2; // PD2

pub const R_AD1: u8 = 0x00;
pub const R_INTF: u8 = 0x1E;
pub const R_INTE: u8 = 0x1F;
pub const R_CFG: u8 = 0x20; // R20..R35: 22 bytes of mode configuration
pub const CFG_LEN: usize = 22;

pub const INTF_AD1: u8 = 1 << 2;
pub const INTF_AD2: u8 = 1 << 1;
pub const INTF_LPF: u8 = 1 << 3;
pub const INTF_RMS: u8 = 1 << 4;

/// R26 bit 7 / R34 bit 7: enable AD2 / AD3.
const ENAD2: u8 = 0x80;
const ENAD3: u8 = 0x80;

#[inline(always)]
fn set(port: u32, pin: u32, high: bool) {
    let bit = if high { 1 << pin } else { 1 << (pin + 16) };
    unsafe { w32(port + BOP, bit) };
}

fn out_pin(port: u32, pin: u32) {
    unsafe {
        modify32(port + MODER, 3 << (2 * pin), 1 << (2 * pin));
        modify32(port + OTYPER, 1 << pin, 0);
        modify32(port + PUPD, 3 << (2 * pin), 0);
    }
}

/// Configure the bus (other pins on these ports are left alone): CS high
/// first and for good, SCK low as a GPIO, then SPI2 configured and enabled
/// (it drives SCK low, its idle level) before the pins are handed to it, so
/// SCK never moves and CS never glitches (the HY3131 ignores CK while CS is
/// high and needs CK low when CS falls, DS 7.2 (6)).
pub fn init_pins() {
    set(GPIOD, CS, true);
    set(GPIOC, SCK, false);
    out_pin(GPIOC, SCK);
    out_pin(GPIOC, MOSI);
    out_pin(GPIOD, CS);
    unsafe { modify32(GPIOC + MODER, 3 << (2 * MISO), 0) }; // input
    spi::init();
}

/// The SPI2 prescaler of the read bus (8 / 16 / 32 = 6.25 / 3.125 / 1.5625
/// MHz).
pub const DIV: u8 = 16;

/// Read divider for the next frames (DBG:HYSPI:SELF? only; the caller
/// restores `DIV`).
pub fn set_div(div: u8) {
    guard();
    spi::set_div(div);
}

/// Write `data` to `reg`, `reg + 1`, ..: over SPI2, one frame per register
/// with an LSB-matched cut dummy (`write_spi` mode 1, at `DIV`; device
/// 42cbf35: 0 bad in 8 x 512 writes at /8, /16 and /32 - the bit-banged
/// write it replaced is gone; see the module notes).
pub fn write(reg: u8, data: &[u8]) {
    guard();
    for (i, &d) in data.iter().enumerate() {
        let r = reg.wrapping_add(i as u8);
        let d = if r == R_CFG {
            R20_ASKED.store(d, Relaxed);
            let d = if beeper_off() { d & !ENPCMPO } else { d };
            R20_SENT.store(d, Relaxed);
            d
        } else {
            d
        };
        let _ = spi::write_cont(r, &[d], true);
    }
}

/// R20 bit 2 (ENPCMPO): the CMP pin buffer. The comparator output on the
/// CMP pin drives the continuity / diode beeper in hardware (CFG_F11 0x56,
/// `diode_beep` 0xb4 / 0xb0; hw/README.md "Buzzer"); the comparator itself
/// (ENCMP, bit 4) keeps running without it.
const ENPCMPO: u8 = 0x04;
/// The beeper is off (key & alert volume 0 %, `App`): `write` clears ENPCMPO.
static BEEP_OFF: AtomicBool = AtomicBool::new(false);
/// R20 as last asked for by the callers and as last sent (masked).
static R20_ASKED: AtomicU8 = AtomicU8::new(0);
static R20_SENT: AtomicU8 = AtomicU8::new(0);

/// Off also in `mute` builds (whatever the volume).
#[inline(always)]
fn beeper_off() -> bool {
    cfg!(feature = "mute") || BEEP_OFF.load(Relaxed)
}

/// Continuity / diode beeper on or off (the volume: 0 % = off; it has no
/// loudness of its own). Takes effect at the next R20 write or `beeper_sync`.
pub fn set_beeper(on: bool) {
    BEEP_OFF.store(!on, Relaxed);
}

/// Rewrite R20 when the beeper was switched while a row with ENPCMPO is
/// active (continuity writes R20 only at mode entry). Main loop, DMM owner.
#[inline(always)]
pub fn beeper_sync() {
    let a = R20_ASKED.load(Relaxed);
    if a & ENPCMPO != 0 && !streaming() && (R20_SENT.load(Relaxed) & ENPCMPO == 0) != beeper_off() {
        write(R_CFG, &[a]);
    }
}

/// Sample the data phase of SPI2 reads with CKPH = 1 (the master samples
/// at the falling edge, a full SCK period after the chip changed DO at the
/// previous one, instead of half a period: the /8 DO-path margin of
/// feasibility 8.2). The command byte stays mode 0 (the chip samples DI on
/// the rising edge). Experiment: DBG:HYSPI:SELF? n,div,1 / DBG:AD2:DIV d,1.
static DATA_CPHA: AtomicBool = AtomicBool::new(false);

pub fn set_data_cpha(on: bool) {
    DATA_CPHA.store(on, Relaxed);
}

/// SPI2 write experiment (DBG:HYSPI:WR?, src/hyspi.rs): `mode` 1 = one
/// frame per register, each [cmd, d, 0x00 cut after its first bits]; 2 = one
/// continuous frame [cmd, d0 .. dn, 0x00 cut]; 3 = byte by byte with gaps and
/// no dummy (0d0fd76's write, the corrupting one). At divider `div` (reads
/// stay at `DIV`). Device b48048e: the chip takes each byte's LSB late, at
/// the MOSI level that follows it (a gap: the GD32 drives the byte's MSB
/// again, 1xxxxxx0 -> bit 0 set; continuous: the next byte's MSB). The cut
/// dummy: the GD32 keeps shifting a dummy byte whose MSB = the last data
/// byte's LSB (0x00 / 0xFF) while CS goes high right after the last data
/// byte's RBNE, so that late sample sees the right level; the incomplete
/// dummy byte is dropped by the chip at CS high (a completed one would write
/// reg + n + 1). Mode 1 (one register per frame) was 0 / 512 bad at /32 with
/// a 0x00 dummy and 1..6 / 512 at /16 (LSB 1 read as 0) - hence the match.
pub fn write_spi(reg: u8, data: &[u8], mode: u8, div: u8) -> bool {
    guard();
    spi::set_div(div);
    let ok = match mode {
        1 => data.iter().enumerate().all(|(i, &d)| spi::write_cont(reg + i as u8, &[d], true)),
        2 => spi::write_cont(reg, data, true),
        _ => spi::write_cont(reg, data, false),
    };
    spi::set_div(DIV);
    ok
}

/// Read `buf.len()` registers from `reg`.
pub fn read(reg: u8, buf: &mut [u8]) {
    guard();
    let _ = spi::read(reg, buf);
}

// ------------------------------------------------- bus hand-off (AD2 stream ISR)
//
// ../docs/history/decisions-2026-10-22.md #3: while an AD2 stream runs on the timer ISR
// (feature `ad2-isr`, src/logger.rs `isr`), the ISR owns the HY3131 bus
// EXCLUSIVELY: the session's chip set-up (set_ad2_stream) is the last
// main-loop bus access before `stream::start` hands it over, `stream::stop`
// takes it back. `read` / `write` / `set_div` (every main-loop path: dmm.rs,
// the power meter, the sentinel, hyspi, ad2exp, the logger's own AD1 /
// main-loop AD2 path) check the hand-off first, and a call while handed off
// stops the stream BEFORE its frame, so a main-loop frame can never
// interleave with the ISR's. That is the enforcement of "the main loop
// touches nothing on the bus while streaming" (decision #3 asked for a debug
// assertion; this is a runtime guard in every build instead, because one
// main-loop path legitimately gets here: a function / range change while
// logging - touch, key, CONF - whose dmm.start the logger's auto-stop used to
// catch only after the fact). `forced_stops` counts it (DBG:ISR?) and the
// logger reports the stopped session as "Settings conflict" (logger.rs
// `poll`, main.rs). The ISR reads through `isr_read` only and never writes a
// register.

static HANDED: AtomicBool = AtomicBool::new(false);
static FORCED: AtomicU32 = AtomicU32::new(0);
static TIMEOUTS: AtomicU32 = AtomicU32::new(0);

/// True while the bus belongs to the stream ISR.
pub fn streaming() -> bool {
    HANDED.load(Relaxed)
}

/// Main-loop bus calls that found the bus handed off (each stopped the
/// stream first): a function change while streaming, else a bug.
pub fn forced_stops() -> u32 {
    FORCED.load(Relaxed)
}

/// SPI2 frames abandoned because a status flag never came (bounded waits,
/// `spi::wait`); 0 on a healthy bus.
pub fn timeouts() -> u32 {
    TIMEOUTS.load(Relaxed)
}

#[inline(always)]
fn guard() {
    if HANDED.load(Relaxed) {
        FORCED.store(FORCED.load(Relaxed).wrapping_add(1), Relaxed);
        #[cfg(feature = "ad2-isr")]
        stream::stop();
    }
}

/// The stream ISR's read (logger.rs `isr`): `read` without the hand-off
/// check. False when the frame timed out (`buf` then holds garbage). Only
/// from the ISR while `streaming()`: the main loop does not touch the bus
/// then, so the frame cannot interleave with another one. Bounded: a frame
/// gives up after ~100 us per byte. The data bytes are pipelined (the next
/// byte is loaded while one shifts: no gap between them, `spi::read_fast`).
#[cfg_attr(not(feature = "ad2-isr"), allow(dead_code))]
pub fn isr_read(reg: u8, buf: &mut [u8]) -> bool {
    spi::read_fast(reg, buf)
}

/// TIMER6 (STM32 name TIM7, 0x4000_1400, APB1, IRQ 55): the AD2 stream tick
/// (decisions #3: a timer, not EXTI11 on PC11, which is also SPI2 MISO).
/// Free in opendm40: TIM1/2/3/5/8 hwtrig + scope, TIM6/12 generator, TIM13/14
/// backlight / buzzer; only the stock firmware's key scan used it.
/// Kernel clock 200 MHz (APB1 x4, TIMPRE, clock.rs); PSC 3 -> 50 MHz count
/// (20 ns), the 16-bit ARR up to 1.31 ms: the ISR sets every interval
/// itself (ARPE = 0, `set_interval`), phase-locked to the chip's conversion
/// grid (src/ad2pll.rs; 1200 .. 38400 S/s, intervals 4 .. 833 us). The
/// AD1 stream (unchopped 160 .. 640 S/s, logger.rs `isr`) runs the same tick with
/// PSC 4 k - 1 (k = 2 / 4 / 8: 40 / 80 / 160 ns counts, a 1.56 / 3.1 / 6.25
/// ms period = 39 062 counts).
#[cfg(feature = "ad2-isr")]
pub mod stream {
    use super::*;

    const RCU_APB1EN: u32 = 0x4002_3840;
    const RCU_TIM7EN: u32 = 1 << 5;
    const TIM7: u32 = 0x4000_1400;
    const CR1: u32 = TIM7;
    const DIER: u32 = TIM7 + 0x0C;
    const SR: u32 = TIM7 + 0x10;
    const EGR: u32 = TIM7 + 0x14;
    const CNT: u32 = TIM7 + 0x24;
    const PSC: u32 = TIM7 + 0x28;
    const ARR: u32 = TIM7 + 0x2C;
    const CR1_CEN: u32 = 1 << 0;
    /// URS: only an overflow sets UIF (not the UG below).
    const CR1_URS: u32 = 1 << 2;
    const UIF: u32 = 1;
    pub const IRQN: u32 = 55;
    const NVIC_ISER: u32 = 0xE000_E100;
    const NVIC_ICER: u32 = 0xE000_E180;
    const NVIC_ICPR: u32 = 0xE000_E280;
    const NVIC_IPR: u32 = 0xE000_E400;
    const SYST_CSR: u32 = 0xE000_E010;
    const SCB_ICSR: u32 = 0xE000_ED04;
    const ICSR_PENDSTCLR: u32 = 1 << 25;
    const ICSR_PENDSVCLR: u32 = 1 << 27;
    const IRQ_BIT: u32 = 1 << (IRQN % 32);
    const IRQ_REG: u32 = 4 * (IRQN / 32);

    /// Read divider of the stream frames: 0 = automatic (below), else 8 /
    /// 16 / 32 with or without the CKPH = 1 data phase (DBG:AD2:DIV d[,cpha]).
    static DIV_REQ: AtomicU32 = AtomicU32::new(0);
    static CPHA_REQ: AtomicBool = AtomicBool::new(false);
    static DIV_USED: AtomicU32 = AtomicU32::new(0);

    pub fn set_div_req(div: u8, cpha: bool) {
        DIV_REQ.store(if matches!(div, 8 | 16 | 32) { div as u32 } else { 0 }, Relaxed);
        CPHA_REQ.store(cpha, Relaxed);
    }

    /// The divider of the running / last stream (0 = none yet).
    pub fn div_used() -> u8 {
        DIV_USED.load(Relaxed) as u8
    }

    /// The running / last stream sampled the data phase at CKPH = 1.
    pub fn cpha_used() -> bool {
        DIV_USED.load(Relaxed) & 0x100 != 0
    }

    /// Main loop, before the chip set-up of a stream at `hz` S/s: the read
    /// divider for its frames, and switch to it (`stop` puts `DIV` and mode
    /// 0 back). Automatic: CPU frames at /8 with the data phase sampled at
    /// CKPH = 1 (device 42cbf35: SELF? 4000 x clean, 35.8 M samples at 38.4
    /// kS/s with 0 sign errors, where plain /8 reads had 193 / 2000 bad
    /// single-byte reads); DMA frames at /16 up to 38.4 kS/s (plain /8: 61 /
    /// 100 sign errors in 1.15 M / 2.36 M samples, device b48048e), and at
    /// 76.8 kS/s the hybrid frame at /8 + CKPH (`hybrid`: the CPU sends the
    /// command byte and switches CKPH, DMA reads the data; a /16 frame does
    /// not fit the 13 us period). Returns the divider; the CKPH choice is
    /// applied too.
    pub fn pick_div(dma: bool, hz: u32) -> u8 {
        let (div, cpha) = choose_div(dma, hz);
        use_div(div, cpha);
        div
    }

    /// The divider / CKPH choice of `pick_div`, without applying it (the
    /// logger's session plan caps the rung with it before any bus access).
    /// DMA frames with /8 + CKPH (automatic above 38.4 kS/s, or forced with
    /// DBG:AD2:DIV 8,1 at any rate) are the hybrid frames.
    pub fn choose_div(dma: bool, hz: u32) -> (u8, bool) {
        let req = DIV_REQ.load(Relaxed) as u8;
        match (req, dma) {
            (0, false) => (8, true),
            (0, true) if hz > 38_400 => (8, true),
            (0, true) => (DIV, false),
            (d, false) => (d, CPHA_REQ.load(Relaxed)),
            (d, true) => (d, d == 8 && CPHA_REQ.load(Relaxed)),
        }
    }

    /// The running / last stream reads with hybrid frames (DMA frames at /8
    /// with the CKPH = 1 data phase).
    pub fn hybrid(dma: bool) -> bool {
        dma && div_used() == 8 && cpha_used()
    }

    /// Main loop, before the chip set-up of a stream: switch the read bus to
    /// `div` / CKPH (`stop` puts `DIV` and mode 0 back). The AD1 stream
    /// uses the main loop's own setting (`DIV`, mode 0: one 3-byte frame
    /// per 1.56 ms or slower, the DMM's reads of every function).
    pub fn use_div(div: u8, cpha: bool) {
        spi::set_div(div);
        super::set_data_cpha(cpha);
        DIV_USED.store(div as u32 | (cpha as u32) << 8, Relaxed);
    }

    /// The fastest AD2 rung (S/s) the stream ISR can read at `div`: with one
    /// CPU frame per sample the frame (~13.5 us at /16) plus a probe frame
    /// must fit the period with the lock's margins (38.4 kS/s at /16 ran the
    /// CPU at 80 % and lost the lock on the device); with DMA frames (5
    /// bytes, 12.8 us at /16) the CPU only starts them; at /8 (the hybrid
    /// frame with CKPH, ~7.3 us from the entry, or a forced plain /8 DMA
    /// frame, 7.2 us) 76.8 kS/s, the chip's fastest AD2 rate.
    pub const fn max_rate_hz(div: u8, dma: bool) -> u32 {
        match (div, dma) {
            (8, false) | (16, true) => 38_400,
            (8, true) => 76_800,
            (16, false) | (32, true) => 19_200,
            _ => 9_600,
        }
    }

    /// DMA frames for the stream: 1 = DMA frames (the default, decision
    /// 2026-10-22 #7), else CPU frames (DBG:AD2:DMA 0|1).
    static DMA_REQ: AtomicU32 = AtomicU32::new(1);

    pub fn set_dma_req(m: u8) {
        DMA_REQ.store(m.min(1) as u32, Relaxed);
    }

    /// Whether the next stream uses DMA frames (the default; DBG:AD2:DMA 0
    /// = CPU frames at /8 + CKPH, the fallback). Device 0471860: 38.4 kS/s
    /// at 17.5 % CPU with DMA frames against 51 % with CPU frames.
    pub fn use_dma() -> bool {
        DMA_REQ.load(Relaxed) == 1
    }

    /// DMA frame primitives (`spi`), for logger.rs `isr`.
    pub fn dma_setup(on: bool) {
        spi::dma_setup(on);
    }
    #[inline(always)]
    pub fn dma_kick(tx: *const u8, rx: *mut u8, n: u32) {
        spi::dma_kick(tx, rx, n);
    }
    #[inline(always)]
    pub fn dma_finish() -> bool {
        spi::dma_finish()
    }
    /// The hybrid data frame (76.8 kS/s): command `cmd` by the CPU (mode 0),
    /// the read-period pulse, CKPH = 1, then `n` bytes by DMA into `rx`
    /// (MOSI 0xFF from `tx`). False: the command byte timed out (CS is up
    /// again, nothing running). The next entry ends it (`dma_finish`, which
    /// also puts CKPH back to 0 for the mode-0 probe frames).
    #[inline(always)]
    pub fn hybrid_kick(cmd: u8, tx: *const u8, rx: *mut u8, n: u32) -> bool {
        spi::hybrid_kick(cmd, tx, rx, n)
    }

    fn barrier() {
        #[cfg(target_os = "none")]
        unsafe {
            core::arch::asm!("dsb", "isb", options(nostack, preserves_flags));
        }
    }

    /// Hand the bus to the ISR; the first timer update (the first entry)
    /// comes `first` counts (20 ns) from now, every later interval is set by
    /// the ISR itself (`set_interval`, logger.rs `isr`). The caller has made
    /// its last bus access (the chip set-up and the edge search) and set up
    /// the ISR's state. The only place PRIMASK is cleared: before that, every
    /// interrupt line and SysTick / PendSV is disabled and its pending bit
    /// cleared (the FS-loader may have left some on), and TIM7 is the only
    /// line enabled, at priority 0: exactly one interrupt in the NVIC.
    /// `psc`: the timer prescaler (3 = 20 ns counts, the AD2 stream; the
    /// AD1 stream counts slower so that a period fits the 16-bit ARR).
    pub fn start(first: u32, psc: u32) {
        let first = first.clamp(100, 65_536);
        unsafe {
            modify32(RCU_APB1EN, 0, RCU_TIM7EN);
            let _ = r32(RCU_APB1EN);
            w32(DIER, 0);
            // ARPE = 0: an ARR write applies to the running interval (the
            // ISR writes it at the start of each one).
            w32(CR1, CR1_URS);
            w32(PSC, psc & 0xFFFF);
            w32(ARR, first - 1);
            w32(EGR, 1); // UG: load PSC now (no UIF: URS)
            w32(CNT, 0);
            w32(SR, 0);
            w32(DIER, 1); // UIE
            w32(SYST_CSR, 0);
            w32(SCB_ICSR, ICSR_PENDSTCLR | ICSR_PENDSVCLR);
            for k in 0..8 {
                w32(NVIC_ICER + 4 * k, !0);
                w32(NVIC_ICPR + 4 * k, !0);
            }
            core::ptr::write_volatile((NVIC_IPR + IRQN) as *mut u8, 0);
            HANDED.store(true, Relaxed);
            barrier();
            w32(NVIC_ISER + IRQ_REG, IRQ_BIT);
            barrier();
            #[cfg(target_os = "none")]
            core::arch::asm!("cpsie i", options(nomem, nostack, preserves_flags));
            w32(CR1, CR1_URS | CR1_CEN);
        }
    }

    /// ISR, at the start of an interval (`cnt` = CNT read by `ack`): make
    /// this interval `len` counts long. Returns the counts it had to be
    /// stretched by when its end had already passed or was too close (the
    /// entry came that late): the caller takes them off the next one.
    #[inline(always)]
    pub fn set_interval(len: u32, cnt: u32) -> u32 {
        // CNT moves on while this runs: a margin of 16 counts (64 cycles).
        let now = unsafe { r32(CNT) }.max(cnt);
        let (arr, over) = if now + 16 < len { (len - 1, 0) } else { (now + 16, now + 17 - len) };
        unsafe { w32(ARR, arr.min(0xFFFF)) };
        over
    }

    /// Stop ticking and take the bus back (main loop only). No ISR can be
    /// half-way through a frame here: on one core the main loop runs only
    /// when the ISR has returned, and the ISR always finishes its frame (CS
    /// high, SPI2 idle). After the NVIC line is off and PRIMASK set again,
    /// no further entry is possible; then the bus is the main loop's.
    pub fn stop() {
        if !HANDED.load(Relaxed) {
            return;
        }
        unsafe {
            w32(CR1, 0);
            w32(DIER, 0);
            w32(NVIC_ICER + IRQ_REG, IRQ_BIT);
            barrier();
            #[cfg(target_os = "none")]
            core::arch::asm!("cpsid i", options(nomem, nostack, preserves_flags));
            w32(SR, 0);
            w32(NVIC_ICPR + IRQ_REG, IRQ_BIT);
            barrier();
        }
        HANDED.store(false, Relaxed);
        spi::dma_setup(false);
        set(GPIOD, CS, true);
        spi::set_div(DIV);
        super::set_data_cpha(false);
    }

    /// ISR entry: clear UIF (rc_w0), return the timer count since the update
    /// event (the entry latency in 20 ns counts).
    #[inline(always)]
    pub fn ack() -> u32 {
        unsafe {
            let c = r32(CNT);
            w32(SR, 0);
            c
        }
    }

    /// ISR exit: a new update already pending = this entry took (or was
    /// delayed) longer than a tick.
    #[inline(always)]
    pub fn overrun() -> bool {
        unsafe { r32(SR) & UIF != 0 }
    }

    /// ISR entered while not streaming (cannot happen: stop() disables the
    /// line first): silence the timer without touching the bus.
    pub fn disarm() {
        unsafe {
            w32(DIER, 0);
            w32(CR1, 0);
            w32(SR, 0);
        }
    }
}

mod spi {
    use super::*;

    const RCU_APB1EN: u32 = 0x4002_3840;
    const RCU_SPI2EN: u32 = 1 << 15;
    const SPI2: u32 = 0x4000_3C00;
    const SPI_CTL0: u32 = SPI2 + 0x00;
    const SPI_STAT: u32 = SPI2 + 0x08;
    const SPI_DATA: u32 = SPI2 + 0x0C;
    const SPI_I2SCTL: u32 = SPI2 + 0x1C;
    const STAT_RBNE: u32 = 1 << 0;
    const STAT_TBE: u32 = 1 << 1;
    const STAT_TRANS: u32 = 1 << 7;
    /// Master (MSTMOD), software NSS high (SWNSSEN | SWNSS), MSB first, 8 bit,
    /// CKPL = CKPH = 0 (mode 0); PSC goes into bits 5:3.
    const CTL0_BASE: u32 = (1 << 9) | (1 << 8) | (1 << 2);
    const CTL0_SPIEN: u32 = 1 << 6;
    const OSPEED: u32 = 0x08;
    const AFRH: u32 = 0x24;
    const PINS2: u32 = (3 << (2 * SCK)) | (3 << (2 * MISO)) | (3 << (2 * MOSI));
    const AF_MASK: u32 = (0xF << (4 * (SCK - 8))) | (0xF << (4 * (MISO - 8))) | (0xF << (4 * (MOSI - 8)));
    const AF6: u32 = (6 << (4 * (SCK - 8))) | (6 << (4 * (MISO - 8))) | (6 << (4 * (MOSI - 8)));
    const MODER_AF: u32 = (2 << (2 * SCK)) | (2 << (2 * MISO)) | (2 << (2 * MOSI));
    /// OSPEED 01 = 25 MHz for SCK / MOSI (2 .. 200 MHz made no difference on
    /// the device).
    const OSPEED_PINS: u32 = (3 << (2 * SCK)) | (3 << (2 * MOSI));
    const OSPEED_SPI: u32 = (1 << (2 * SCK)) | (1 << (2 * MOSI));
    /// CS low -> first SCK edge, last SCK edge -> CS high, CS high time
    /// (spin iterations, 5 cycles = 25 ns each at 200 MHz, measured):
    /// ~200 / ~100 / ~500 ns (0.2 .. 5 us made no difference on the device).
    const CS_SETUP: u32 = 8;
    const CS_HOLD: u32 = 4;
    const CS_IDLE: u32 = 20;
    /// The hand-made read-period pulse: ~100 ns high (TCKHI >= 20 ns), then
    /// ~100 ns low before the SPI's first sampling edge (another half period
    /// later) for DO (TDODL 60 ns + the isolator both ways).
    const PULSE_HI: u32 = 4;
    const PULSE_LO: u32 = 4;

    const fn psc_of(div: u8) -> u32 {
        match div {
            8 => 2,
            32 => 4,
            _ => 3,
        }
    }

    fn config(div: u8) {
        let cfg = CTL0_BASE | (psc_of(div) << 3);
        unsafe {
            w32(SPI_CTL0, CTL0_BASE);
            modify32(SPI_I2SCTL, 1 << 11, 0); // SPI, not I2S
            w32(SPI_CTL0, cfg);
            w32(SPI_CTL0, cfg | CTL0_SPIEN);
            let _ = r32(SPI_DATA); // drop a stale byte
        }
    }

    pub(super) fn init() {
        unsafe {
            modify32(RCU_APB1EN, 0, RCU_SPI2EN);
            let _ = r32(RCU_APB1EN); // clock on before the first register access
            modify32(GPIOC + OSPEED, OSPEED_PINS, OSPEED_SPI);
            modify32(GPIOC + AFRH, AF_MASK, AF6);
        }
        config(DIV);
        to_af();
    }

    /// Between frames only (SPI2 idle, SCK low, CS high).
    pub(super) fn set_div(div: u8) {
        let _ = idle();
        config(div);
    }

    /// Pins back to SPI2 (SCK low on both sides of the switch).
    pub(super) fn to_af() {
        set(GPIOC, SCK, false);
        unsafe { modify32(GPIOC + MODER, PINS2, MODER_AF) };
    }

    /// Wait until `SPI_STAT & mask == want`, at most `WAIT_MAX` polls
    /// (~100 us at APB1 50 MHz, a byte takes 2.56 us): a stuck peripheral
    /// ends the frame instead of hanging the loop / the stream ISR (an ISR
    /// hang is a watchdog reset, counted as a crash by boot.rs).
    #[inline(always)]
    fn wait(mask: u32, want: u32) -> bool {
        let mut n = 0u32;
        while unsafe { r32(SPI_STAT) } & mask != want {
            n += 1;
            if n >= WAIT_MAX {
                TIMEOUTS.store(TIMEOUTS.load(Relaxed).wrapping_add(1), Relaxed);
                return false;
            }
        }
        true
    }
    const WAIT_MAX: u32 = 2000;

    fn idle() -> bool {
        wait(STAT_TRANS, 0)
    }

    #[inline(always)]
    fn xfer(b: u8) -> Option<u8> {
        if !wait(STAT_TBE, STAT_TBE) {
            return None;
        }
        unsafe { w16(SPI_DATA, b as u16) };
        if !wait(STAT_RBNE, STAT_RBNE) {
            return None;
        }
        Some(unsafe { r32(SPI_DATA) } as u8)
    }

    /// Read `buf.len()` registers from `reg`. The command byte has bit 7 = 0,
    /// so the late-LSB effect of SPI2 writes (module notes) cannot touch it.
    /// False when a status wait timed out (the frame is ended, CS high,
    /// `buf` partly stale). The PC10 MODER read-modify-write below may
    /// preempt a main-loop RMW of GPIOC MODER when this runs in the stream
    /// ISR; it puts back exactly the value it read (PC10 AF again), so the
    /// main loop's write-back is unaffected, and nothing in the main loop
    /// changes the mode of PC10..12 while the bus is handed off (`guard`).
    pub(super) fn read(reg: u8, buf: &mut [u8]) -> bool {
        set(GPIOD, CS, false);
        spin(CS_SETUP);
        if xfer((reg << 1) | 1).is_none() || !idle() {
            return end(false);
        }
        // Read period: SPI2 idle (SCK low after the 8th clock), PC10 to GPIO
        // output (ODR 0: stays low), one pulse, back to AF.
        unsafe {
            modify32(GPIOC + MODER, 3 << (2 * SCK), 1 << (2 * SCK));
            set(GPIOC, SCK, true);
            spin(PULSE_HI);
            set(GPIOC, SCK, false);
            spin(PULSE_LO);
            modify32(GPIOC + MODER, 3 << (2 * SCK), 2 << (2 * SCK));
            if DATA_CPHA.load(Relaxed) {
                cpha(true);
            }
        }
        // MOSI 1 during the data phase, as on the bit-banged bus (the R bit).
        for b in buf.iter_mut() {
            match xfer(0xFF) {
                Some(v) => *b = v,
                None => return end(false),
            }
        }
        let ok = idle();
        end(ok)
    }

    /// `read` with the data bytes pipelined: the next 0xFF goes into DATA as
    /// soon as TBE says the previous one moved to the shift register, so the
    /// bytes follow each other without a gap (one RBNE read per byte, well
    /// inside the next byte's 1.28 us at /8: no overrun). The stream ISR's
    /// frames only (3-byte AD2 result, 1-byte INTF).
    pub(super) fn read_fast(reg: u8, buf: &mut [u8]) -> bool {
        set(GPIOD, CS, false);
        spin(CS_SETUP);
        if xfer((reg << 1) | 1).is_none() || !idle() {
            return end(false);
        }
        unsafe {
            modify32(GPIOC + MODER, 3 << (2 * SCK), 1 << (2 * SCK));
            set(GPIOC, SCK, true);
            spin(PULSE_HI);
            set(GPIOC, SCK, false);
            spin(PULSE_LO);
            modify32(GPIOC + MODER, 3 << (2 * SCK), 2 << (2 * SCK));
            if DATA_CPHA.load(Relaxed) {
                cpha(true);
            }
            let n = buf.len();
            if n == 0 {
                return end(true);
            }
            w16(SPI_DATA, 0xFF);
            for i in 0..n {
                if i + 1 < n {
                    if !wait(STAT_TBE, STAT_TBE) {
                        return end(false);
                    }
                    w16(SPI_DATA, 0xFF);
                }
                if !wait(STAT_RBNE, STAT_RBNE) {
                    return end(false);
                }
                buf[i] = r32(SPI_DATA) as u8;
            }
        }
        let ok = idle();
        end(ok)
    }

    /// SPI2 write frame (see `super::write_spi`). `cut`: continuous TX of
    /// cmd + data + a 0x00 dummy, CS high at the last data byte's RBNE (the
    /// dummy's first bit in flight), then the dummy drained. Else byte by
    /// byte: each one sent and received before the next is loaded.
    pub(super) fn write_cont(reg: u8, data: &[u8], cut: bool) -> bool {
        set(GPIOD, CS, false);
        spin(CS_SETUP);
        let ok = unsafe {
            if !cut {
                let mut ok = xfer(reg << 1).is_some();
                for &d in data {
                    ok = ok && xfer(d).is_some();
                }
                ok && idle()
            } else {
                // bytes: cmd, data.., dummy; the RBNE of byte k is read
                // after byte k + 1 went into DATA.
                let n = data.len() + 1;
                let last = data.last().copied().unwrap_or(0);
                let dummy = if last & 1 != 0 { 0xFF } else { 0x00 };
                let byte = |k: usize| if k == 0 { reg << 1 } else if k < n { data[k - 1] } else { dummy };
                w16(SPI_DATA, byte(0) as u16);
                let mut ok = true;
                for k in 1..=n {
                    ok = ok && wait(STAT_TBE, STAT_TBE);
                    if !ok {
                        break;
                    }
                    w16(SPI_DATA, byte(k) as u16);
                    ok = wait(STAT_RBNE, STAT_RBNE);
                    let _ = r32(SPI_DATA);
                    if ok && k == n {
                        // byte n - 1 (the last data byte) is in: CS up now,
                        // the dummy has just started.
                        set(GPIOD, CS, true);
                    }
                }
                ok = ok && wait(STAT_RBNE, STAT_RBNE);
                let _ = r32(SPI_DATA);
                ok && idle()
            }
        };
        end(ok)
    }

    // ---- DMA frames (the AD2 stream ISR, `stream`) ----
    //
    // GD32 DMA0 (STM32 DMA1; register layout as bt.rs / gen.rs), SPI2 =
    // STM32 SPI3 requests: RX on channel (stream) 0, TX on channel 7, both
    // peripheral request 0 (the STM32F4-compatible map; bt.rs uses 2 / 4,
    // gen.rs 5). The frame uses the shift framing (feasibility 8.1): the
    // command byte, then n bytes that also clock the read period, so the
    // data arrive one bit late (decoded by the caller). CS is the GPIO: the
    // ISR drops it, starts both channels and returns; the next entry checks
    // the channel finished and raises CS.
    const DMA0: u32 = 0x4002_6000;
    const DMA_INTC0: u32 = DMA0 + 0x08;
    const DMA_INTC1: u32 = DMA0 + 0x0C;
    const fn dch(ch: u32, off: u32) -> u32 {
        DMA0 + 0x10 + 0x18 * ch + off
    }
    const RXC: u32 = 0;
    const TXC: u32 = 7;
    const SPI_CTL1: u32 = SPI2 + 0x04;
    /// CTL: peripheral request 0, byte / byte, memory increment, P->M (RX)
    /// or M->P (TX, DIR bit 6), high priority (bits 17:16 = 10), enable.
    const DMA_RX_CTL: u32 = (2 << 16) | (1 << 10) | 1;
    const DMA_TX_CTL: u32 = (2 << 16) | (1 << 10) | (1 << 6) | 1;

    /// Session start / end (main loop): the channels' peripheral address and
    /// SPI2's DMA request enables (DMAREN | DMATEN: with a channel disabled a
    /// request does nothing, so the CPU-polled frames are unaffected).
    pub(super) fn dma_setup(on: bool) {
        unsafe {
            w32(dch(RXC, 0), 0);
            w32(dch(TXC, 0), 0);
            w32(DMA_INTC0, 0x3D);
            w32(DMA_INTC1, 0x3D << 22);
            if on {
                modify32(0x4002_3830, 0, 1 << 21); // RCU_AHB1EN DMA0EN
                w32(dch(RXC, 0x08), SPI_DATA);
                w32(dch(TXC, 0x08), SPI_DATA);
                w32(SPI_CTL1, 0x3);
            } else {
                w32(SPI_CTL1, 0);
                let _ = r32(SPI_DATA);
            }
        }
    }

    /// Start a DMA frame: CS low, `n` bytes of `tx` out, the replies to `rx`.
    #[inline(always)]
    pub(super) fn dma_kick(tx: *const u8, rx: *mut u8, n: u32) {
        set(GPIOD, CS, false);
        unsafe {
            dma_arm(tx, rx, n);
            spin(CS_SETUP);
            w32(dch(RXC, 0), DMA_RX_CTL);
            w32(dch(TXC, 0), DMA_TX_CTL);
        }
    }

    /// Both channels set up for `n` bytes (not enabled), RX emptied.
    #[inline(always)]
    unsafe fn dma_arm(tx: *const u8, rx: *mut u8, n: u32) {
        drain_rx();
        w32(DMA_INTC0, 0x3D);
        w32(DMA_INTC1, 0x3D << 22);
        w32(dch(RXC, 0x04), n);
        w32(dch(RXC, 0x0C), rx as u32);
        w32(dch(TXC, 0x04), n);
        w32(dch(TXC, 0x0C), tx as u32);
    }

    /// The hybrid frame (`stream::hybrid_kick`): the CPU part of `read` (CS,
    /// the command byte in mode 0, the read-period pulse, CKPH = 1), then the
    /// data bytes by DMA (the channels armed before CS goes low).
    /// CPU time ~2.5 us at /8 (the command byte's 1.28 us is waited for:
    /// the CKPH switch must come after it and before the first data clock).
    #[inline(always)]
    pub(super) fn hybrid_kick(cmd: u8, tx: *const u8, rx: *mut u8, n: u32) -> bool {
        unsafe {
            dma_arm(tx, rx, n);
            set(GPIOD, CS, false);
            spin(CS_SETUP);
            if !wait(STAT_TBE, STAT_TBE) {
                return end(false);
            }
            w16(SPI_DATA, cmd as u16);
            if !wait(STAT_RBNE, STAT_RBNE) {
                return end(false);
            }
            let _ = r32(SPI_DATA);
            if !idle() {
                return end(false);
            }
            modify32(GPIOC + MODER, 3 << (2 * SCK), 1 << (2 * SCK));
            set(GPIOC, SCK, true);
            spin(PULSE_HI);
            set(GPIOC, SCK, false);
            spin(PULSE_LO);
            modify32(GPIOC + MODER, 3 << (2 * SCK), 2 << (2 * SCK));
            cpha(true);
            w32(dch(RXC, 0), DMA_RX_CTL);
            w32(dch(TXC, 0), DMA_TX_CTL);
        }
        true
    }

    /// End the running DMA frame: wait (bounded, ~100 us) until every byte
    /// came back, then CS high. A frame may still run when the next entry
    /// comes (the entry that started it was late, device 00e088f: aborting it
    /// there left its last byte in flight, every later frame read one byte
    /// shifted - bad samples, a runaway lock). On a timeout the channels stop,
    /// the shift ends, and the receive register is emptied. True = complete.
    #[inline(always)]
    pub(super) fn dma_finish() -> bool {
        let mut n = 0u32;
        let mut ok = true;
        while unsafe { r32(dch(RXC, 0x04)) } != 0 {
            n += 1;
            if n >= WAIT_MAX {
                TIMEOUTS.store(TIMEOUTS.load(Relaxed).wrapping_add(1), Relaxed);
                ok = false;
                break;
            }
        }
        unsafe {
            if !ok {
                w32(dch(RXC, 0), 0);
                w32(dch(TXC, 0), 0);
            }
            let _ = idle();
            drain_rx();
            if r32(SPI_CTL0) & 1 != 0 {
                cpha(false); // a hybrid frame: the probe frames are mode 0
            }
        }
        spin(CS_HOLD);
        set(GPIOD, CS, true);
        spin(CS_SETUP); // CS high before any next frame
        ok
    }

    /// Empty the receive register (a stale byte would shift the next DMA
    /// frame by one).
    #[inline(always)]
    unsafe fn drain_rx() {
        let mut k = 0;
        while r32(SPI_STAT) & STAT_RBNE != 0 && k < 4 {
            let _ = r32(SPI_DATA);
            k += 1;
        }
    }

    /// CKPH for the next bytes (SPI2 idle; SCK idle low in both: no edge).
    #[inline(always)]
    fn cpha(on: bool) {
        unsafe {
            let c = r32(SPI_CTL0);
            let c = if on { c | 1 } else { c & !1 };
            w32(SPI_CTL0, c & !CTL0_SPIEN);
            w32(SPI_CTL0, c | CTL0_SPIEN);
        }
    }

    fn end(ok: bool) -> bool {
        if unsafe { r32(SPI_CTL0) } & 1 != 0 {
            cpha(false);
        }
        spin(CS_HOLD);
        set(GPIOD, CS, true);
        spin(CS_IDLE);
        ok
    }
}

pub fn read1(reg: u8) -> u8 {
    let mut b = [0u8];
    read(reg, &mut b);
    b[0]
}

/// Signed 24-bit little-endian value at `b[0..3]`.
pub fn s24(b: &[u8]) -> i32 {
    ((b[0] as u32 | (b[1] as u32) << 8 | (b[2] as u32) << 16) << 8) as i32 >> 8
}

/// Mode change as the stock driver does it: stop AD2/AD3 and R36, then the
/// caller switches relays, then the 22 configuration bytes, then INTE = 0.
pub fn quiesce() {
    write(0x26, &[0]);
    write(0x34, &[0]);
    write(0x36, &[0]);
}

pub fn configure(cfg: &[u8; CFG_LEN]) {
    let (r26, r34) = (cfg[6], cfg[20]);
    if r26 & ENAD2 != 0 && r34 & ENAD3 != 0 {
        // Every AC row (f0, f2, f4, f5_6) runs the RMS converter on
        // sum(AD2 * AD3) / N (R34 SVXI). With the row written as is, ENAD2
        // (R26) is set some 14 bytes before ENAD3 (R34), and when AD3 then
        // starts in a bad phase of the shared AD2/AD3 decimation (period
        // OSR2 / FAD2CLK = 13 us) sqrt(RMS) beats against the signal for as
        // long as the row stays: +-11 % on the 40 Hz PWM at AC V r1, both in
        // plain AC and in AC+DC (stock/re/docs/history/acdc-stock-vs-opendm40.md, "Read-path
        // investigation"). Enabling AD3 first and AD2 last was clean for
        // every skew tried (0..15 us), so the row goes out with both off
        // and they are switched on in that order.
        let mut c = *cfg;
        c[6] = r26 & !ENAD2;
        c[20] = r34 & !ENAD3;
        write(R_CFG, &c);
        write(R_INTE, &[0]);
        write(0x34, &[r34]);
        write(0x26, &[r26]);
    } else {
        write(R_CFG, cfg);
        write(R_INTE, &[0]);
    }
}

/// R36: SPHACAL (HY3131 only), the AD2 / AD3 phase trim of the power
/// measurement: bit 7 = AD2 leads AD3, bits 6..0 = delay in AD2CLK periods
/// (1 / 2.4576 MHz = 0.407 us, up to 51.7 us).
pub const R_SPHACAL: u8 = 0x36;

/// Power-meter row (dmm::pw): as `configure` (AD3 enabled before AD2, the
/// beat fix) with SPHACAL written before both enables, as the datasheet
/// asks ("Set SPHACAL first, then ENAD2 and ENAD3 = 1, otherwise the data
/// would be stabilized later"). `quiesce` zeroed R36 before.
pub fn configure_ph(cfg: &[u8; CFG_LEN], r36: u8) {
    let mut c = *cfg;
    c[6] &= !ENAD2;
    c[20] &= !ENAD3;
    write(R_CFG, &c);
    write(R_INTE, &[0]);
    write(R_SPHACAL, &[r36]);
    write(0x34, &[cfg[20]]);
    write(0x26, &[cfg[6]]);
}

/// RMS<37:0> (R09..R0D, `b[0..5]`) as a signed 38-bit value: sum(X*Y)/N of
/// the power statistic is negative when the energy flows the other way.
/// (The stock and `poll_ac` read 37 bits unsigned: right for the squares.)
pub fn rms_signed(b: &[u8]) -> i64 {
    let raw = b[0] as u64 | (b[1] as u64) << 8 | (b[2] as u64) << 16 | (b[3] as u64) << 24 | ((b[4] & 0x3F) as u64) << 32;
    ((raw << 26) as i64) >> 26
}

/// LPF<18:0> (R06..R08, `b[0..3]`), 19-bit signed.
pub fn lpf19(b: &[u8]) -> i32 {
    (((b[0] as u32) << 13 | (b[1] as u32) << 21 | (b[2] as u32) << 29) as i32) >> 13
}

// ------------------------------------------------------- AD1 conversion timing
//
// R22 = AD1OS[7:5] AD1CHOP[4:3] AD1OSR[2:0] (hy3131_v2_6.h; datasheet 13.1).
// The AD1 modulator runs at AD1CLK = SYSCLK / 30 (fixed divider, chapter 11)
// and delivers one result per OSR1 = 256 << AD1OSR clocks; with AD1CHOP = 1x
// the chip alternates the input polarity per conversion to cancel the offset
// and "the ADC output rate will be one time slower", i.e. halved:
//   period = OSR1 * 30 / SYSCLK * (AD1CHOP = 1x ? 2 : 1), SYSCLK 4.9152 MHz
// Every R22 the stock writes is chopped (AD1CHOP = 10):
//   0x13 OSR 2048   25 ms  40/s   (ohm over range; the data logger default)
//   0x14 OSR 4096   50 ms  20/s   (continuity while the reading moves)
//   0x15 OSR 8192  100 ms  10/s   (every DC row; the stock's moving reading)
//   0x16 OSR 16384 200 ms   5/s   (temperature)
//   0x17 OSR 32768 400 ms 2.5/s   (the stock's steady DC / ohm reading)
// Unchopped, the same codes run twice as fast (OSR 256: 1.5625 ms, 640/s;
// the datasheet's "5 Hz" headline is OSR 32768 unchopped). Source:
// emu/hy3131/RESEARCH.md 3; the emulator models this formula (and agrees
// with the data logger's 38 Hz measured on the device at 0x13); a direct
// device measurement of the DMM rates is still pending (TODO.md).
// Before 2026-10-14 opendm40 assumed OSR / SYSCLK here (16x too short at OSR
// 2048: 417 us) and an "SPI-limited" logger; the bit-banged link needs
// ~122 us per sample (read1(INTF) ~45 us + 3-byte AD1 read ~77 us), far
// below every conversion period, so the chip sets the pace at every code.

/// HY3131 system clock (crystal), Hz.
pub const SYSCLK_HZ: u32 = 4_915_200;
/// AD1CLK = SYSCLK / AD1CLK_DIV (fixed in the chip).
pub const AD1CLK_DIV: u32 = 30;
/// R22 AD1CHOP = 10: chopped, offset cancelled, half the rate. The stock DC
/// base (AD1OS = 000) is 0x10 | AD1OSR.
pub const R22_CHOP: u8 = 0x10;

/// AD1 conversion period in microseconds for an AD1OSR code (0..7, OSR =
/// 256 << code) with or without the chopper: 1563 us (OSR 256, unchopped)
/// .. 400000 us (OSR 32768, chopped); rounded to the nearest us.
pub const fn ad1_period_us(osr_code: u8, chop: bool) -> u32 {
    let clocks = (256u64 << (osr_code & 7)) * AD1CLK_DIV as u64 * if chop { 2 } else { 1 };
    ((clocks * 1_000_000 + SYSCLK_HZ as u64 / 2) / SYSCLK_HZ as u64) as u32
}

/// AD1 conversion rate for an AD1OSR code and chopper, mHz (exact: 640 000
/// at OSR 256 unchopped; `ad1_period_us` rounds the period to 1 us).
pub const fn ad1_rate_mhz(osr_code: u8, chop: bool) -> u32 {
    let clocks = (256u64 << (osr_code & 7)) * AD1CLK_DIV as u64 * if chop { 2 } else { 1 };
    ((SYSCLK_HZ as u64 * 1000 + clocks / 2) / clocks) as u32
}

/// `ad1_period_us` of a raw R22 value (AD1OSR = bits 2..0, AD1CHOP bit 4).
pub const fn ad1_period_us_r22(r22: u8) -> u32 {
    ad1_period_us(r22 & 7, r22 & R22_CHOP != 0)
}

/// AD1OSR run-time code of the data logger's default (R22 = 0x13, OSR 2048,
/// chopped: 25 ms, 40/s, the stock's fastest setting).
pub const AD1OSR_STREAM: u8 = 3;

/// Write R22 with AD1OS = 000 (no DC offset) and the given AD1OSR code
/// (clamped to 0..7) and chopper. Lower code = faster, fewer effective bits;
/// `chop` false (AD1CHOP = 00: VX + VOS) doubles the rate but leaves the
/// converter offset in every result (the calibration was taken chopped) and
/// raises the low-frequency noise. A legal setting: Hycon's own ENOB tool
/// runs AD1 unchopped in its AC rows (R22 = 0x04 / 0x07).
pub fn set_ad1_osr(code: u8, chop: bool) {
    let chop = if chop { R22_CHOP } else { 0 };
    write(0x22, &[chop | (code & 7)]);
}

/// True when AD1 has a fresh conversion (INTF AD1F). Reading the AD1 data
/// registers afterwards clears the flag, so this paces the logger at the
/// conversion rate without an external timer.
pub fn ad1_ready() -> bool {
    read1(R_INTF) & INTF_AD1 != 0
}

/// Read the current signed 24-bit AD1 conversion (R00..R02). Clears AD1F.
pub fn read_ad1() -> i32 {
    let mut b = [0u8; 3];
    read(R_AD1, &mut b);
    s24(&b)
}

// ------------------------------------------------------- AD2 streaming (FR-38 B)
//
// R26 = ENAD2 . 0 . ENCHOPAD2 . AD2RG . SAD2CLK . AD2OSR[2:0] (datasheet 13.2,
// emu/hy3131/ref/registers.md). AD2CLK = SYSCLK / 2 (SAD2CLK = 0) or / 4;
// one result per OSR2 = 32 << AD2OSR clocks (codes 5..7 all 1024). ENCHOPAD2
// does NOT halve the output rate on the device, whatever the datasheet says
// (device check of f80524f: R26 0x25 and 0x05 both 422 us per flag-synced
// sample, 0x24 and 0x04 both 232 us; the logger at "2400" = 0x24 took a
// result on every P/2 gate). Every stock AC row writes R26 = 0x80:
// /2, OSR 32, unchopped = 76.8 kS/s (13 us). The data logger's AD2 source
// keeps the row (mux, gain, op amps) and only rewrites R26 to a slower rung
// (docs/dmm-input-scope-feasibility.md 1.1); results: 19-bit signed in
// R03..R05 (bits 23:19 a copy of the sign), flag INTF.AD2F, set by AD3 too.
// No FIFO: an unread result is overwritten by the next one.

/// First AD2 data register (R03..R05, LSB first).
pub const R_AD2: u8 = 0x03;
/// R26 bits.
pub const R26_CHOP: u8 = 0x20;
pub const R26_CLK4: u8 = 0x08;

/// AD2 output period in nanoseconds for an R26 value (ENAD2 and ENCHOPAD2
/// ignored): 13021 ns (0x80, the stock AC rows) .. 833_333 ns (/4, OSR 1024).
pub const fn ad2_period_ns(r26: u8) -> u32 {
    let osr = 32u64 << if r26 & 7 > 5 { 5 } else { r26 & 7 };
    let clocks = osr * if r26 & R26_CLK4 != 0 { 4 } else { 2 };
    ((clocks * 1_000_000_000 + SYSCLK_HZ as u64 / 2) / SYSCLK_HZ as u64) as u32
}

/// AD2 output rate in mHz for an R26 value (9600 S/s = 9_600_000).
#[allow(dead_code)] // tests, docs
pub const fn ad2_rate_mhz(r26: u8) -> u32 {
    let osr = 32u64 << if r26 & 7 > 5 { 5 } else { r26 & 7 };
    let clocks = osr * if r26 & R26_CLK4 != 0 { 4 } else { 2 };
    ((SYSCLK_HZ as u64 * 1000 + clocks / 2) / clocks) as u32
}

/// The 76.8 kS/s rung's conversion rate, mHz of the MCU clock (R26 low bits
/// 0: /2, OSR 32, with or without ENCHOPAD2): the datasheet's FAD2CLK / OSR2
/// (`ad2_rate_mhz(0)`). c830e09 had 76 574.6 here, the rate the 20a91d0 /
/// 825080c / decaf60 builds streamed at; device c830e09 (device-check
/// 2026-10-21 "c830e09: the 76.8 kS/s PLL retest") showed that rate came
/// from the old loop, not the chip: with the fixed loop (ad2pll.rs) the
/// stream locks at rate_mhz 76 800 0xx from a cold start (-2935 ppm
/// against 76 574.6), gap-free disk data has 1919.95 samples per 40 Hz
/// calibrator period (decaf60: 1914.27) and the Wave view's record 960 per
/// half period, so 76 574.6 made every 76800 time axis 0.29 % long.
pub const AD2_OSR32_REAL_MHZ: u32 = ad2_rate_mhz(0);

/// The AD2 conversion rate the chip really runs at, mHz: `ad2_rate_mhz`
/// (the 76.8 kS/s rung through `AD2_OSR32_REAL_MHZ`, now the same). For the stream's
/// Pll nominal and the session's nominal period (LOG:PREamble?
/// nominal_mhz, the CSV / DLG headers, the gap accounting); the rung keeps
/// its name (`ad2_rate_mhz`: LOG:SRATe, the Wave view's bands).
pub const fn ad2_real_rate_mhz(r26: u8) -> u32 {
    if r26 & 0x0F == 0 { AD2_OSR32_REAL_MHZ } else { ad2_rate_mhz(r26) }
}

/// `ad2_real_rate_mhz` as a period, ns (13 021 at the 76.8 kS/s rung).
pub const fn ad2_real_period_ns(r26: u8) -> u32 {
    if r26 & 0x0F == 0 {
        ((1_000_000_000_000u64 + AD2_OSR32_REAL_MHZ as u64 / 2) / AD2_OSR32_REAL_MHZ as u64) as u32
    } else {
        ad2_period_ns(r26)
    }
}

/// The data logger's AD2 rate ladder: R26 low bits for a requested rate in
/// S/s. Prefers the /2 clock (more modulator samples per result) and the
/// largest OSR that gives the rate: 1200 .. 76800 S/s (19200 .. 76800 for
/// the phase-locked stream ISR, feasibility 11; 76.8 kS/s = OSR 32 at /2,
/// the stock AC rows' own rate, only with the hybrid DMA frames, 11.12; the
/// stream's read divider caps the rung, `stream::max_rate_hz`), the same rungs with
/// `chop` (ENCHOPAD2, offset cancelled, the logger default) and without
/// (ENCHOPAD2 does not slow AD2 down, see above; /4, OSR 1024 = 1200 S/s is
/// the slowest the chip has). A request between two rungs takes the nearest
/// one in log scale (rounding down on a tie); the achieved rung is what
/// LOG:PREamble? reports. Above ~6.5 kS/s the bit-banged bus was the limit
/// (device f80524f: 6.2-6.6 kS/s captured at 9600); LOG:COUNt? lost shows
/// what the bus in use misses.
pub const fn ad2_r26_for(req_hz: u32, chop: bool) -> u8 {
    // (rate S/s, R26 low bits without ENCHOPAD2)
    const T: [(u32, u8); 7] =
        [(1200, R26_CLK4 | 5), (2400, 5), (4800, 4), (9600, 3), (19200, 2), (38400, 1), (76800, 0)];
    let c = if chop { R26_CHOP } else { 0 };
    let t = &T;
    let mut i = 0;
    while i + 1 < t.len() {
        // geometric midpoint of a 2x step: rate * sqrt(2); req^2 vs 2 rate^2
        let r = t[i].0 as u64;
        if (req_hz as u64) * (req_hz as u64) <= 2 * r * r {
            return t[i].1 | c;
        }
        i += 1;
    }
    t[t.len() - 1].1 | c
}

/// Logger AD2 stream: AD3 off (the row starts it before AD2, so its grid is
/// skewed and AD2F would fire twice per period), RMS / LPF off (R29 = 0),
/// then R26 = ENAD2 | `r26`. ENAD2 is never cleared (that would zero
/// R03..R05). The stock AC rows leave AD1 off; the misconnect sentinel
/// (sentinel.rs) turns it on in AC V / mV for its peak hold, so AD1 is
/// switched off FIRST: datasheet 13.1 note 7 (no AD2/AD3 switching while AD1
/// converts) then does not apply, and AD2 runs alone. The DMM restores the
/// row, sentinel included, afterwards (dmm.start).
pub fn set_ad2_stream(r26: u8) {
    let r23 = read1(0x23);
    if r23 & 0x80 != 0 {
        write(0x23, &[r23 & 0x7F]);
    }
    write(0x34, &[0]);
    write(0x29, &[0]);
    write(0x26, &[0x80 | (r26 & 0x3F)]);
    write(R_INTF, &[0]); // drop the flags of the old setting
}

/// True when AD2 has a result not seen yet (INTF AD2F). Reading INTF clears
/// ALL flags (AD1F, RMSF, LPFF too): only while the logger owns the chip.
pub fn ad2_ready() -> bool {
    read1(R_INTF) & INTF_AD2 != 0
}

/// The AD2 result (R03..R05) as signed 19 bit, plus whether bits 23:19 are a
/// clean copy of the sign (bit 18): a torn 3-byte read can break that.
pub fn read_ad2() -> (i32, bool) {
    let mut b = [0u8; 3];
    read(R_AD2, &mut b);
    let v = s24(&b);
    (v, ad2_sign_ok(v))
}

/// Bits 23:19 of a sign-extended 24-bit value equal bit 18.
pub const fn ad2_sign_ok(v: i32) -> bool {
    v >= -0x40000 && v <= 0x3FFFF
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ad2_ladder_matches_the_datasheet() {
        // The stock AC rows: /2, OSR 32, unchopped: 76.8 kS/s.
        assert_eq!(ad2_rate_mhz(0x80), 76_800_000);
        assert_eq!(ad2_period_ns(0x80), 13_021);
        for hz in [1200u32, 2400, 4800, 9600, 19200, 38400, 76800] {
            let r = ad2_r26_for(hz, true);
            assert_eq!(ad2_rate_mhz(r), hz * 1000, "chop {hz}");
            assert!(r & R26_CHOP != 0);
            let r = ad2_r26_for(hz, false);
            assert_eq!(ad2_rate_mhz(r), hz * 1000, "plain {hz}");
            assert!(r & R26_CHOP == 0);
        }
        // The device (f80524f check): ENCHOPAD2 does not halve the rate.
        assert_eq!(ad2_period_ns(0x25), ad2_period_ns(0x05));
        assert_eq!(ad2_period_ns(0x24), 208_333);
        // The f80524f labels: 0x2D was "600" (measured 1125/s captured,
        // 886 us spacing <= the real 833 us + the gate), 0x24 was "2400".
        assert_eq!(ad2_rate_mhz(0x2D), 1_200_000);
        // Out-of-ladder requests: nearest rung, clamped at the ends.
        assert_eq!(ad2_rate_mhz(ad2_r26_for(1, true)), 1_200_000);
        assert_eq!(ad2_rate_mhz(ad2_r26_for(600, true)), 1_200_000);
        assert_eq!(ad2_rate_mhz(ad2_r26_for(600, false)), 1_200_000);
        assert_eq!(ad2_rate_mhz(ad2_r26_for(50_000, true)), 38_400_000);
        assert_eq!(ad2_rate_mhz(ad2_r26_for(60_000, true)), 76_800_000);
        assert_eq!(ad2_rate_mhz(ad2_r26_for(1_000_000, false)), 76_800_000);
        assert_eq!(ad2_r26_for(76_800, false), 0); // the stock AC rows' AD2 setting (0x80 with ENAD2)
        assert_eq!(ad2_rate_mhz(ad2_r26_for(12_000, true)), 9_600_000);
        assert_eq!(ad2_rate_mhz(ad2_r26_for(3000, true)), 2_400_000);
        assert_eq!(ad2_rate_mhz(ad2_r26_for(3500, true)), 4_800_000);
        // 9600 S/s: 104.167 us; 1200 S/s: 833.3 us.
        assert_eq!(ad2_period_ns(ad2_r26_for(9600, true)), 104_167);
        assert_eq!(ad2_period_ns(ad2_r26_for(1200, true)), 833_333);
        // AD2OSR codes 5..7 are all 1024.
        assert_eq!(ad2_period_ns(7), ad2_period_ns(5));
        // The real rates are the datasheet's at every rung (device c830e09);
        // R26_CHOP and AD2RG do not matter.
        assert_eq!(ad2_real_rate_mhz(0x00), ad2_rate_mhz(0));
        assert_eq!(ad2_real_rate_mhz(R26_CHOP | 0x10), ad2_rate_mhz(0));
        assert_eq!(ad2_real_period_ns(R26_CHOP), 13_021);
        for r in [0x01u8, 0x21, 0x02, 0x03, 0x04, 0x05, 0x0D, 0x08] {
            assert_eq!(ad2_real_rate_mhz(r), ad2_rate_mhz(r), "r26 {r:#x}");
            assert_eq!(ad2_real_period_ns(r), ad2_period_ns(r), "r26 {r:#x}");
        }
        assert!(ad2_sign_ok(0x3FFFF) && ad2_sign_ok(-0x40000) && !ad2_sign_ok(0x40000) && !ad2_sign_ok(0x7F0000));
    }

    #[test]
    fn ad1_periods_match_the_datasheet() {
        // Chopped, as every stock R22 (emu/hy3131/RESEARCH.md 3).
        assert_eq!(ad1_period_us_r22(0x13), 25_000);
        assert_eq!(ad1_period_us_r22(0x14), 50_000);
        assert_eq!(ad1_period_us_r22(0x15), 100_000);
        assert_eq!(ad1_period_us_r22(0x16), 200_000);
        assert_eq!(ad1_period_us_r22(0x17), 400_000);
        // Unchopped: half; the fastest setting, OSR 256: 1.5625 ms = 640/s.
        assert_eq!(ad1_period_us(0, false), 1_563);
        assert_eq!(ad1_period_us(0, true), 3_125);
        assert_eq!(ad1_period_us(3, false), 12_500);
        assert_eq!(ad1_period_us(7, false), 200_000);
        assert_eq!(ad1_period_us_r22(0x07), 200_000);
        assert_eq!(ad1_period_us_r22(0x18 | 3), 25_000); // AD1CHOP = 11 chops too
        assert_eq!(ad1_period_us(9, true), ad1_period_us(1, true)); // code masked to 0..7
    }

    #[test]
    fn rms_and_lpf_sign() {
        assert_eq!(rms_signed(&[0x10, 0, 0, 0, 0]), 16);
        // Bit 37 set: negative; bits 39..38 (sign copies or not) ignored.
        assert_eq!(rms_signed(&[0xFF, 0xFF, 0xFF, 0xFF, 0x3F]), -1);
        assert_eq!(rms_signed(&[0xFF, 0xFF, 0xFF, 0xFF, 0xFF]), -1);
        assert_eq!(rms_signed(&[0, 0, 0, 0, 0x20]), -(1i64 << 37));
        assert_eq!(rms_signed(&[0xFF, 0xFF, 0xFF, 0xFF, 0x1F]), (1i64 << 37) - 1);
        assert_eq!(lpf19(&[0xFF, 0xFF, 0x07]), -1);
        assert_eq!(lpf19(&[0x00, 0x00, 0x04]), -262_144);
        assert_eq!(lpf19(&[0xFF, 0xFF, 0x03]), 262_143);
    }
}
