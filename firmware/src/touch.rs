//! Capacitive touch panel on I2C2 (STM32 "I2C3"; SCL = PA8, SDA = PC9, AF4), reset = PE6,
//! INT = PE5 (falling edge, pull-up). Two controllers are supported, selected at
//! init by probing in the stock order (stock/re/analysis/touch_v2_6.h):
//!   1. Goodix GT1158 at 7-bit 0x14 (8-bit 0x28) — probed first (header §Goodix,
//!      lines 8/110-118/140). Absent on this unit; the probe NAKs and we fall
//!      back to 0x5A, so the 0x5A bus traffic below is byte-identical to before.
//!   2. Hynitron CST2xx-type at 7-bit 0x5A (8-bit 0xB4) — the chip on this unit.
//! The detected controller is stored in `Ctrl`; `poll()` dispatches on it so the
//! coordinate + side-key output to the rest of the firmware is identical either
//! way (gesture.rs / app.rs see the same `Event`).
//!
//! Register addresses are 16-bit big-endian on both chips and always sent as
//! their own write followed by STOP, then a separate read (no repeated start).
//!
//! Hynitron 0x5A (header §INIT/§FRAME, lines 41-68):
//!   init:  write D1 01; write D2 04 + read 4 (byte 2 = chip type, 0xA8);
//!          write D1 09
//!   frame: write D0 00 + read 7; byte 6 must be 0xAB; n = byte5 & 7 points;
//!          extra points at D0 07 / 0C / 11 / 16 (5 bytes each); then write
//!          D0 00 AB to acknowledge.
//!   point: b0 low nibble 6 = pressed; X = b1 << 4 | b3 >> 4,
//!          Y = b2 << 4 | b3 & 0xF (landscape, 480x320).
//!   keys:  b0 bit 7 = key report; b0 = 0x83 while held, 0x80 on release
//!          (measured). Code from X (measured 372/628/884/1140): < 500 -> 0,
//!          < 750 -> 1, < 1000 -> 2, else 3.
//!
//! Goodix GT1158 (header §Goodix, lines 98/110-118; correct-by-construction,
//! not testable on this unit which has the 0x5A chip):
//!   id:    read 0x8140 (4 bytes) == ASCII "1158"
//!   init:  write 0x8040 = 0x02; wait 10 ms; write 0x8040 = 0x00 (normal report)
//!   frame: read 0x814E (1 byte status): bit7 = buffer ready, bit4 = HaveKey,
//!          bits 3..0 = point count. Only act when bit7 set; then clear the
//!          buffer with a write 0x814E = 0x00 (stock order: status, clear, then
//!          point/key reads — header line 112). Point i coords are 4 bytes at
//!          0x8150 + 8*i = X lo, X hi, Y lo, Y hi. The GT1158 reports native
//!          portrait (320x480); the stock swaps into the landscape GUI frame with
//!          NO scale/mirror (header 115-116): GUI x = Goodix Y, GUI y = Goodix X.
//!   keys:  key byte (value 1/2/4/8 -> code 0..3) at 0x814F + 8*n (header 98/113).
//!   Because the GT1158 INT behaviour is unknown, the Goodix path also
//!   force-polls the buffer-ready flag every ~10 ms so a missed INT edge (or a
//!   long loop iteration) cannot leave touch stuck.
//!
//! Transport (both controllers go through `Bus::write_at` / `Bus::read_at`, so
//! the byte sequence on the wire is the same whichever transport carries it):
//!   * Hardware I2C2 (GD32 I2C v1 = STM32F4 style, base 0x4000_5C00, PA8/PC9
//!     AF4 open drain + pull-up, stock FUN_0800cb86), blocking, register-polled
//!     with a DWT timeout on every flag wait (no IRQ, no DMA: opendm40 runs with
//!     PRIMASK set and DMA0 ch2, the stock I2C2 RX channel, now belongs to
//!     bt.rs). 400 kHz when the measured SCL/SDA rise time allows it (the pull-
//!     up strength is unknown; measured at init, see `rise_ns`), else the stock
//!     100 kHz. A NAK is a normal result (the Goodix probe at 0x14 NAKs on this
//!     unit; it now fails in ~30 us instead of the stock's 500 ms timeout, same
//!     bytes). A timeout / bus error / lost arbitration resets the peripheral,
//!     clocks the bus free (9 SCL + STOP on GPIO) and re-inits; FAULT_STREAK
//!     consecutive faults step 400 kHz -> 100 kHz -> bit-bang for good, and
//!     BAD_STREAK consecutive edge-triggered 0x5A frames failing the 0xAB
//!     check at 400 kHz drop to 100 kHz. If no controller answers on the peripheral at init the
//!     probe is repeated at 100 kHz and then on the bit-bang.
//!   * Bit-banged I2C on the same pins (GPIO open drain), T = 4 us half bit:
//!     the fallback, byte-identical to the previous driver.
//! INT = PE5 is latched by EXTI line 5 (falling edge, pending bit polled; the
//! line's NVIC IRQ 23 is kept disabled, opendm40 has no ISRs). `poll()` reads a
//! frame when the edge latched, or when INT is still low 10 ms after the last
//! read (= the old level-polled behaviour as the worst case, so a controller
//! that holds INT low, or an edge lost to a dropped frame, cannot stall touch).
//! `irq_pending()` lets the main loop read a fresh report at once instead of on
//! its 10 ms tick. `diag()` = 40 bytes of bus/IRQ counters for DBG:DIAG?.

use crate::clock::{delay_us, Clock, HCLK_HZ};
use crate::mmio::{modify32, r32, w32};

const GPIOA: u32 = 0x4002_0000;
const GPIOC: u32 = 0x4002_0800;
const GPIOE: u32 = 0x4002_1000;
const SCL: u32 = 8; // PA8
const SDA: u32 = 9; // PC9
const RST: u32 = 6; // PE6
const INT: u32 = 5; // PE5

const T: u32 = 4; // half bit, us (~100 kHz)
const ADDR: u8 = 0x5A; // Hynitron, this unit
const ADDR_GOODIX: u8 = 0x14; // GT1158 (8-bit 0x28), header line 140

// Goodix GT1158 16-bit big-endian registers (header §Goodix, lines 110-118).
const GT_REG_PRODUCT_ID: u16 = 0x8140; // read 4 == "1158"
const GT_REG_COMMAND: u16 = 0x8040; // init: 0x02 then 0x00
const GT_REG_STATUS: u16 = 0x814E; // bit7 ready, bit4 HaveKey, bits3..0 count
const GT_REG_POINT0: u16 = 0x8150; // point i coords (4 bytes) at +8*i
const GT_REG_KEY0: u16 = 0x814F; // key byte at +8*n (point stride 8)
const GT_ID: [u8; 4] = *b"1158";
/// Force a Goodix buffer-ready read at least this often even without an INT
/// edge (the GT1158 INT behaviour is unknown and the loop can stall on a flash
/// write; the 0x5A path does NOT use this).
const GT_POLL_MS: u64 = 10;
/// Re-read while INT is still low this long after the last read (the old
/// level-polled cadence, kept as the fallback behind the EXTI edge).
const LEVEL_MS: u64 = 10;

// --- RCU / SYSCFG / EXTI / NVIC (GD32F4xx; same layout as STM32F4) ---
const RCU_CFG0: u32 = 0x4002_3808;
const RCU_APB1RST: u32 = 0x4002_3820;
const RCU_APB1EN: u32 = 0x4002_3840;
const RCU_APB2EN: u32 = 0x4002_3844;
const RCU_I2C2: u32 = 1 << 23; // APB1EN / APB1RST bit 23 (stock FUN_0800cb86)
const RCU_SYSCFG: u32 = 1 << 14; // APB2EN bit 14
const SYSCFG_EXTISS1: u32 = 0x4001_380C; // EXTI4..7 source, line 5 = bits 7..4
const EXTI: u32 = 0x4001_3C00;
const EXTI_INTEN: u32 = EXTI + 0x00;
const EXTI_RTEN: u32 = EXTI + 0x08;
const EXTI_FTEN: u32 = EXTI + 0x0C;
const EXTI_PD: u32 = EXTI + 0x14; // write 1 to clear
const NVIC_ICER0: u32 = 0xE000_E180;
const NVIC_ICER2: u32 = 0xE000_E188;
const NVIC_ICPR0: u32 = 0xE000_E280;
const IRQ_EXTI5_9: u32 = 23;
const DWT_CYCCNT: u32 = 0xE000_1004;
const CYC_PER_US: u32 = HCLK_HZ / 1_000_000;

// --- I2C2 registers ---
const I2C2: u32 = 0x4000_5C00;
const I2C_CTL0: u32 = I2C2 + 0x00;
const I2C_CTL1: u32 = I2C2 + 0x04;
const I2C_DATA: u32 = I2C2 + 0x10;
const I2C_STAT0: u32 = I2C2 + 0x14;
const I2C_STAT1: u32 = I2C2 + 0x18;
const I2C_CKCFG: u32 = I2C2 + 0x1C;
const I2C_RT: u32 = I2C2 + 0x20;
// CTL0
const I2CEN: u32 = 1 << 0;
const START: u32 = 1 << 8;
const STOP: u32 = 1 << 9;
const ACKEN: u32 = 1 << 10;
const POAP: u32 = 1 << 11;
// STAT0
const SBSEND: u32 = 1 << 0;
const ADDSEND: u32 = 1 << 1;
const BTC: u32 = 1 << 2;
const RBNE: u32 = 1 << 6;
const TBE: u32 = 1 << 7;
const BERR: u32 = 1 << 8;
const LOSTARB: u32 = 1 << 9;
const AERR: u32 = 1 << 10;
// STAT1
const I2CBSY: u32 = 1 << 1;
// CKCFG
const FAST: u32 = 1 << 15;

/// Every flag wait gives up after this (10 ms): a byte is 90 us at 100 kHz,
/// so this only trips on a stuck bus or a controller stretching SCL forever.
const HW_TIMEOUT_CYC: u32 = 10_000 * CYC_PER_US;
/// SCL and SDA must both rise (released -> reads high) within this for
/// 400 kHz: the low phase is 1.7 us, the I2C fast-mode limit is tr <= 300 ns
/// (30 -> 70 %); 400 ns to the input threshold is that with margin to spare.
const FAST_RISE_NS: u16 = 400;
/// Consecutive hardware faults (timeout, bus error, lost arbitration) before
/// stepping down one bus mode.
const FAULT_STREAK: u8 = 3;
/// Consecutive 0x5A frames failing the 0xAB check at 400 kHz before dropping
/// to 100 kHz (a marginal edge shows up as corrupt data, not as a fault).
const BAD_STREAK: u8 = 5;

/// Which touch controller was detected at init.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Ctrl {
    /// Hynitron CST2xx-type at 0x5A (this unit).
    Hynitron,
    /// Goodix GT1158 at 0x14 (fallback for panels without the 0x5A chip).
    Goodix,
}

/// Transport carrying the touch I2C traffic (DIAG byte 0 = this value).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    BitBang = 0,
    Hw100 = 1,
    Hw400 = 2,
}

/// Bus and IRQ counters, exported through `Touch::diag()` (DBG:DIAG?).
#[derive(Clone, Copy, Default)]
pub struct Stats {
    /// SCL / SDA rise time measured at init (ns from release to reading high).
    pub rise_scl_ns: u16,
    pub rise_sda_ns: u16,
    /// Duration of the last frame read (us) and its pressed point count.
    pub last_us: u16,
    pub last_n: u8,
    /// Longest frame read since boot (us).
    pub max_us: u16,
    /// Mode the controller was found in at init (Mode as u8).
    pub probe_mode: u8,
    /// Reads started by an INT falling edge (EXTI pending bit).
    pub edge_reads: u32,
    /// Reads started by the INT-still-low fallback (no edge latched).
    pub level_reads: u32,
    /// Goodix only: reads started by the GT_POLL_MS timer.
    pub timer_reads: u32,
    /// Frames accepted (0x5A: 0xAB check passed; Goodix: buffer ready).
    pub frames_ok: u32,
    /// 0x5A frames dropped by the 0xAB check (or whose first read failed).
    pub frames_bad: u32,
    /// Transactions NAKed (address or data).
    pub naks: u32,
    /// Hardware faults, each followed by a peripheral reset + 9-clock recovery.
    pub faults: u32,
}

fn od_pin(port: u32, pin: u32) {
    unsafe {
        w32(port + 0x18, 1 << pin); // released (high)
        modify32(port + 0x04, 0, 1 << pin); // open drain
        modify32(port + 0x08, 3 << (2 * pin), 2 << (2 * pin)); // 50 MHz (stock)
        modify32(port + 0x0C, 3 << (2 * pin), 1 << (2 * pin)); // pull-up
        modify32(port + 0x00, 3 << (2 * pin), 1 << (2 * pin)); // output
    }
}

/// SCL / SDA pin function: AF4 = I2C2 (true) or GPIO open-drain output (false,
/// bit-bang and bus recovery). Only MODER changes; AFRH is set once in `new`.
fn pins_af(af: bool) {
    let m = if af { 2 } else { 1 };
    unsafe {
        // Leave the GPIO output released so the switch cannot glitch low.
        w32(GPIOA + 0x18, 1 << SCL);
        w32(GPIOC + 0x18, 1 << SDA);
        modify32(GPIOA + 0x00, 3 << (2 * SCL), m << (2 * SCL));
        modify32(GPIOC + 0x00, 3 << (2 * SDA), m << (2 * SDA));
    }
}

#[inline(always)]
fn cyc() -> u32 {
    unsafe { r32(DWT_CYCCNT) }
}

#[inline(always)]
fn scl(high: bool) {
    unsafe { w32(GPIOA + 0x18, if high { 1 << SCL } else { 1 << (SCL + 16) }) };
}

#[inline(always)]
fn sda(high: bool) {
    unsafe { w32(GPIOC + 0x18, if high { 1 << SDA } else { 1 << (SDA + 16) }) };
}

#[inline(always)]
fn sda_in() -> bool {
    (unsafe { r32(GPIOC + 0x10) } & (1 << SDA)) != 0
}

#[inline(always)]
fn scl_in() -> bool {
    (unsafe { r32(GPIOA + 0x10) } & (1 << SCL)) != 0
}

/// Rise time of an idle bus line (GPIO mode, bus idle): drive it low, release
/// it and count until the input reads high. 20 us cap (= no pull-up).
fn rise_ns(port: u32, pin: u32) -> u16 {
    unsafe {
        w32(port + 0x18, 1 << (pin + 16));
        delay_us(5);
        let t0 = cyc();
        w32(port + 0x18, 1 << pin);
        let mut c;
        loop {
            c = cyc().wrapping_sub(t0);
            if r32(port + 0x10) & (1 << pin) != 0 || c > 20 * CYC_PER_US {
                break;
            }
        }
        (c * 1000 / CYC_PER_US).min(u16::MAX as u32) as u16
    }
}

fn scl_release() {
    scl(true);
    let mut n = 0;
    while !scl_in() && n < 1000 {
        delay_us(1);
        n += 1;
    }
}

fn start() {
    sda(true);
    scl_release();
    delay_us(T);
    sda(false);
    delay_us(T);
    scl(false);
    delay_us(T);
}

fn stop() {
    sda(false);
    delay_us(T);
    scl_release();
    delay_us(T);
    sda(true);
    delay_us(T);
}

/// Free a bus a slave holds (SDA low mid-byte): up to 9 SCL clocks until SDA
/// is released, then a STOP. GPIO mode only.
fn bus_clear() {
    for _ in 0..9 {
        if sda_in() {
            break;
        }
        scl(false);
        delay_us(T);
        scl_release();
        delay_us(T);
    }
    stop();
}

fn tx(b: u8) -> bool {
    for i in (0..8).rev() {
        sda(b & (1 << i) != 0);
        delay_us(T);
        scl_release();
        delay_us(T);
        scl(false);
    }
    sda(true);
    delay_us(T);
    scl_release();
    delay_us(T);
    let ack = !sda_in();
    scl(false);
    delay_us(T);
    ack
}

fn rx(ack: bool) -> u8 {
    let mut b = 0u8;
    sda(true);
    for _ in 0..8 {
        delay_us(T);
        scl_release();
        delay_us(T);
        b = (b << 1) | sda_in() as u8;
        scl(false);
    }
    sda(!ack);
    delay_us(T);
    scl_release();
    delay_us(T);
    scl(false);
    sda(true);
    delay_us(T);
    b
}

fn bb_write_at(addr: u8, data: &[u8]) -> bool {
    start();
    let mut ok = tx(addr << 1);
    for &b in data {
        ok = ok && tx(b);
    }
    stop();
    ok
}

fn bb_read_at(addr: u8, buf: &mut [u8]) -> bool {
    start();
    if !tx((addr << 1) | 1) {
        stop();
        return false;
    }
    let n = buf.len();
    for (i, b) in buf.iter_mut().enumerate() {
        *b = rx(i + 1 < n);
    }
    stop();
    true
}

// ---------------------------------------------------------------------------
// Hardware I2C2, blocking. GD32 I2C v1 receive sequences as the STM32F4 RM
// (method 2, BTC-based, so a late poll can never ACK the last byte).

/// One hardware transaction's outcome.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Hw {
    Ok,
    /// Address or data byte not acknowledged; STOP sent, bus idle.
    Nak,
    /// Timeout, bus error or lost arbitration: the peripheral needs a reset.
    Fault,
}

/// APB1 clock = HCLK (clock.rs: 200 MHz) / APB1PSC (RCU_CFG0 bits 12..10;
/// 0b101 = /4 -> 50 MHz in practice, CFG0 0x940A).
fn apb1_hz() -> u32 {
    let apb = (unsafe { r32(RCU_CFG0) } >> 10) & 7;
    HCLK_HZ >> if apb < 4 { 0 } else { apb - 3 }
}

/// Reset I2C2 and program it for `mode` (Hw100 / Hw400). Pins untouched.
fn hw_init(mode: Mode) {
    let pclk = apb1_hz();
    let mhz = (pclk / 1_000_000).clamp(2, 60);
    unsafe {
        modify32(RCU_APB1EN, 0, RCU_I2C2);
        modify32(RCU_APB1RST, 0, RCU_I2C2);
        modify32(RCU_APB1RST, RCU_I2C2, 0);
        w32(I2C_CTL1, mhz); // I2CCLK; no IRQ / DMA enables
        if mode == Mode::Hw400 {
            // Fast mode, DTCY = 0 (Tlow:Thigh = 2:1): f = pclk / (3 * CLKC),
            // rounded up so it never exceeds 400 kHz (50 MHz -> 42 -> 397 kHz).
            let clkc = ((pclk + 3 * 400_000 - 1) / (3 * 400_000)).max(1);
            w32(I2C_CKCFG, FAST | clkc);
            w32(I2C_RT, mhz * 300 / 1000 + 1);
        } else {
            // Standard mode, as the stock (i2c_clock_config(I2C2, 100000)).
            let clkc = ((pclk + 2 * 100_000 - 1) / (2 * 100_000)).max(4);
            w32(I2C_CKCFG, clkc);
            w32(I2C_RT, mhz + 1);
        }
        w32(I2C_CTL0, I2CEN | ACKEN);
    }
}

/// Wait for any of `mask` in STAT0, or an error. Ok(stat0) when a mask bit
/// rose; Err(Nak) on AERR (cleared here, STOP not yet sent); Err(Fault) on a
/// bus error / lost arbitration / timeout.
fn hw_wait(mask: u32) -> Result<u32, Hw> {
    let t0 = cyc();
    loop {
        let s = unsafe { r32(I2C_STAT0) };
        if s & (BERR | LOSTARB) != 0 {
            return Err(Hw::Fault);
        }
        if s & AERR != 0 {
            unsafe { w32(I2C_STAT0, !AERR & 0xFFFF) }; // rc_w0
            return Err(Hw::Nak);
        }
        if s & mask != 0 {
            return Ok(s);
        }
        if cyc().wrapping_sub(t0) > HW_TIMEOUT_CYC {
            return Err(Hw::Fault);
        }
    }
}

/// Request STOP and wait until the peripheral has sent it.
fn hw_stop() -> Hw {
    unsafe { modify32(I2C_CTL0, 0, STOP) };
    hw_stop_wait()
}

fn hw_stop_wait() -> Hw {
    let t0 = cyc();
    while unsafe { r32(I2C_CTL0) } & STOP != 0 {
        if cyc().wrapping_sub(t0) > HW_TIMEOUT_CYC {
            return Hw::Fault;
        }
    }
    Hw::Ok
}

/// A NAK ends the transaction with STOP, like the bit-bang path.
fn hw_nak() -> Hw {
    match hw_stop() {
        Hw::Ok => Hw::Nak,
        f => f,
    }
}

/// START + address byte. On Ok, ADDSEND is set and NOT yet cleared (the
/// receive sequences must set ACKEN / POAP first).
fn hw_start(addr_byte: u8) -> Hw {
    let t0 = cyc();
    while unsafe { r32(I2C_STAT1) } & I2CBSY != 0 {
        if cyc().wrapping_sub(t0) > HW_TIMEOUT_CYC {
            return Hw::Fault;
        }
    }
    unsafe { modify32(I2C_CTL0, 0, START) };
    if let Err(e) = hw_wait(SBSEND) {
        return e;
    }
    // STAT0 read (in hw_wait) + DATA write clears SBSEND.
    unsafe { w32(I2C_DATA, addr_byte as u32) };
    match hw_wait(ADDSEND) {
        Ok(_) => Hw::Ok,
        Err(Hw::Nak) => hw_nak(),
        Err(e) => e,
    }
}

#[inline(always)]
fn hw_clear_addsend() {
    unsafe {
        r32(I2C_STAT0);
        r32(I2C_STAT1);
    }
}

#[inline(always)]
fn hw_rd() -> u8 {
    unsafe { r32(I2C_DATA) as u8 }
}

/// START, addr|W, data..., STOP.
fn hw_write(addr: u8, data: &[u8]) -> Hw {
    match hw_start(addr << 1) {
        Hw::Ok => {}
        e => return e,
    }
    hw_clear_addsend();
    if data.is_empty() {
        return hw_stop();
    }
    for &b in data {
        match hw_wait(TBE) {
            Ok(_) => unsafe { w32(I2C_DATA, b as u32) },
            Err(Hw::Nak) => return hw_nak(),
            Err(e) => return e,
        }
    }
    match hw_wait(BTC) {
        Ok(_) => hw_stop(),
        Err(Hw::Nak) => hw_nak(),
        Err(e) => e,
    }
}

/// START, addr|R, buf.len() bytes (NACK on the last), STOP.
fn hw_read(addr: u8, buf: &mut [u8]) -> Hw {
    let n = buf.len();
    unsafe {
        let c = r32(I2C_CTL0) & !(ACKEN | POAP | START | STOP);
        let ack = if n >= 2 { ACKEN } else { 0 };
        let poap = if n == 2 { POAP } else { 0 };
        w32(I2C_CTL0, c | ack | poap);
    }
    let r = hw_start((addr << 1) | 1);
    if r != Hw::Ok {
        unsafe { modify32(I2C_CTL0, POAP, 0) };
        return r;
    }
    let w = |m: u32| hw_wait(m).map_err(|_| Hw::Fault); // no AERR while receiving
    let res: Result<(), Hw> = (|| {
        match n {
            0 => {
                hw_clear_addsend();
                unsafe { modify32(I2C_CTL0, 0, STOP) };
            }
            1 => {
                // ACK off before ADDSEND clears, STOP right after.
                hw_clear_addsend();
                unsafe { modify32(I2C_CTL0, 0, STOP) };
                w(RBNE)?;
                buf[0] = hw_rd();
            }
            2 => {
                // POAP: the ACK bit applies to the next byte (byte 2).
                hw_clear_addsend();
                unsafe { modify32(I2C_CTL0, ACKEN, 0) };
                w(BTC)?; // byte 1 in DATA, byte 2 in the shift register
                unsafe { modify32(I2C_CTL0, 0, STOP) };
                buf[0] = hw_rd();
                buf[1] = hw_rd();
            }
            _ => {
                hw_clear_addsend();
                for b in buf[..n - 3].iter_mut() {
                    w(RBNE)?;
                    *b = hw_rd();
                }
                w(BTC)?; // n-3 in DATA, n-2 in shift, SCL stretched
                unsafe { modify32(I2C_CTL0, ACKEN, 0) }; // NACK the last byte
                buf[n - 3] = hw_rd();
                w(BTC)?; // n-2 in DATA, n-1 in shift
                unsafe { modify32(I2C_CTL0, 0, STOP) };
                buf[n - 2] = hw_rd();
                w(RBNE)?;
                buf[n - 1] = hw_rd();
            }
        }
        Ok(())
    })();
    let r = match res {
        Ok(()) => hw_stop_wait(),
        Err(e) => e,
    };
    // Only once STOP has gone out: a read-modify-write of CTL0 racing the
    // hardware clearing STOP would re-request it. (A fault resets I2C2.)
    if r == Hw::Ok {
        unsafe { modify32(I2C_CTL0, POAP, 0) };
    }
    r
}

/// The touch I2C transport (hardware I2C2 or bit-bang) plus its counters.
struct Bus {
    mode: Mode,
    /// Consecutive hardware faults / bad 0xAB frames (escalation, see header).
    fault_streak: u8,
    bad_streak: u8,
    /// The current read was started by an INT edge: only such frames count
    /// toward BAD_STREAK (a level-fallback re-read may find no fresh frame).
    edge_read: bool,
    stats: Stats,
}

impl Bus {
    /// Put the pins and the peripheral into `mode` after clocking the bus free.
    fn enter(&mut self, mode: Mode) {
        self.mode = mode;
        self.fault_streak = 0;
        self.bad_streak = 0;
        pins_af(false);
        bus_clear();
        if mode == Mode::BitBang {
            // Peripheral off (and held in reset) so it cannot touch the pins.
            unsafe {
                modify32(RCU_APB1RST, 0, RCU_I2C2);
                modify32(RCU_APB1RST, RCU_I2C2, 0);
                modify32(RCU_APB1EN, RCU_I2C2, 0);
            }
        } else {
            // AF before I2CEN, so the peripheral never sees the pins detached.
            pins_af(true);
            hw_init(mode);
        }
    }

    /// Account one hardware transaction; on a fault reset + recover, and step
    /// down a mode after FAULT_STREAK in a row.
    fn account(&mut self, r: Hw) -> bool {
        match r {
            Hw::Ok => {
                self.fault_streak = 0;
                true
            }
            Hw::Nak => {
                self.fault_streak = 0;
                self.stats.naks = self.stats.naks.wrapping_add(1);
                false
            }
            Hw::Fault => {
                self.stats.faults = self.stats.faults.wrapping_add(1);
                let streak = self.fault_streak + 1;
                if streak >= FAULT_STREAK {
                    let next = if self.mode == Mode::Hw400 { Mode::Hw100 } else { Mode::BitBang };
                    self.enter(next); // resets the streak
                } else {
                    self.enter(self.mode);
                    self.fault_streak = streak;
                }
                false
            }
        }
    }

    /// A 0x5A frame passed (true) or failed (false) the 0xAB check.
    fn frame_check(&mut self, good: bool) {
        if good {
            self.bad_streak = 0;
            self.stats.frames_ok = self.stats.frames_ok.wrapping_add(1);
            return;
        }
        self.stats.frames_bad = self.stats.frames_bad.wrapping_add(1);
        if self.edge_read {
            self.bad_streak = self.bad_streak.saturating_add(1);
        }
        if self.mode == Mode::Hw400 && self.bad_streak >= BAD_STREAK {
            self.enter(Mode::Hw100);
        }
    }

    fn write_at(&mut self, addr: u8, data: &[u8]) -> bool {
        if self.mode == Mode::BitBang {
            let ok = bb_write_at(addr, data);
            if !ok {
                self.stats.naks = self.stats.naks.wrapping_add(1);
            }
            return ok;
        }
        let r = hw_write(addr, data);
        self.account(r)
    }

    fn read_at(&mut self, addr: u8, buf: &mut [u8]) -> bool {
        if self.mode == Mode::BitBang {
            let ok = bb_read_at(addr, buf);
            if !ok {
                self.stats.naks = self.stats.naks.wrapping_add(1);
            }
            return ok;
        }
        let r = hw_read(addr, buf);
        self.account(r)
    }

    fn read_reg_at(&mut self, addr: u8, reg: u16, buf: &mut [u8]) -> bool {
        self.write_at(addr, &reg.to_be_bytes()) && self.read_at(addr, buf)
    }

    // 0x5A (Hynitron) wrappers — unchanged behaviour (addr fixed to ADDR = 0x5A).
    fn write(&mut self, data: &[u8]) -> bool {
        self.write_at(ADDR, data)
    }

    fn read_reg(&mut self, reg: u16, buf: &mut [u8]) -> bool {
        self.read_reg_at(ADDR, reg, buf)
    }

    /// Probe in the stock order (Goodix 0x14 "1158" first, then the 0x5A
    /// chip) and initialise what answers. Bus traffic unchanged from the
    /// bit-bang-only driver.
    fn probe(&mut self, clk: &mut Clock) -> Option<(Ctrl, [u8; 4])> {
        // Probe the Goodix GT1158 first (stock order, header lines 8/110-118).
        // On this unit it NAKs at 0x14, so read_reg_at returns false and we fall
        // through to the 0x5A chip; the 0x5A traffic below is unchanged.
        let mut id = [0u8; 4];
        if self.read_reg_at(ADDR_GOODIX, GT_REG_PRODUCT_ID, &mut id) && id == GT_ID {
            // GT1158 config: command 0x02, settle, then 0x00 = normal reporting
            // (header line 111). Clear any stale buffer so the first INT is fresh.
            // The PE5 pull-up is left enabled (electrically safe); the stock drops
            // it after a Goodix probe (header 34-35) but that is not required here.
            self.write_at(ADDR_GOODIX, &[(GT_REG_COMMAND >> 8) as u8, GT_REG_COMMAND as u8, 0x02]);
            clk.delay_ms(10);
            self.write_at(ADDR_GOODIX, &[(GT_REG_COMMAND >> 8) as u8, GT_REG_COMMAND as u8, 0x00]);
            self.write_at(ADDR_GOODIX, &[(GT_REG_STATUS >> 8) as u8, GT_REG_STATUS as u8, 0x00]);
            return Some((Ctrl::Goodix, id)); // "1158"
        }

        // Hynitron 0x5A (this unit): unchanged init. ACK-only identify.
        let mut chip = [0u8; 4];
        let ok = self.write(&[0xD1, 0x01]) && self.read_reg(0xD204, &mut chip) && self.write(&[0xD1, 0x09]);
        if ok { Some((Ctrl::Hynitron, chip)) } else { None }
    }
}

/// EXTI line 5 <- PE5, falling edge, pending bit latched (INTEN set so PD
/// latches; the NVIC line stays disabled and PRIMASK is set anyway).
fn exti_init() {
    unsafe {
        w32(NVIC_ICER0, 1 << IRQ_EXTI5_9);
        // I2C2 EV/ER (IRQ 72/73): never vector into opendm40's table.
        w32(NVIC_ICER2, 3 << (72 - 64));
        modify32(RCU_APB2EN, 0, RCU_SYSCFG);
        modify32(SYSCFG_EXTISS1, 0xF << 4, 4 << 4); // line 5 = port E
        modify32(EXTI_RTEN, 1 << INT, 0);
        modify32(EXTI_FTEN, 0, 1 << INT);
        modify32(EXTI_INTEN, 0, 1 << INT);
        w32(EXTI_PD, 1 << INT);
        w32(NVIC_ICPR0, 1 << IRQ_EXTI5_9);
    }
}

#[inline(always)]
fn exti_pending() -> bool {
    unsafe { r32(EXTI_PD) & (1 << INT) != 0 }
}

/// Test-and-clear the INT edge latch (cleared before the read, so an edge
/// during the read latches again).
fn exti_take() -> bool {
    if exti_pending() {
        unsafe { w32(EXTI_PD, 1 << INT) };
        true
    } else {
        false
    }
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub struct Point {
    pub x: u16,
    pub y: u16,
}

/// Most points one frame carries (both controllers report up to 5).
pub const MAX_POINTS: usize = 5;

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub struct Event {
    /// Number of pressed points (0 = released), at most MAX_POINTS.
    pub points: u8,
    /// First pressed point (= pts[0]).
    pub p: Point,
    /// Pressed points in the controller's slot order: pts[..points]; the rest
    /// are zero (Event equality drives the debounce).
    pub pts: [Point; MAX_POINTS],
    /// Side key held: 1 << code (code 0..3 from the key report), 0 = none.
    pub keys: u8,
}

/// One decoded controller frame, identical for both chips.
#[derive(Clone, Copy, Default)]
pub struct Frame {
    pub pressed: bool,
    pub key: bool,
    /// Pressed screen points (pts[..n]).
    pub n: u8,
    pub pts: [Point; MAX_POINTS],
    /// Side-key code 0..3 (valid when `key`).
    pub code: u8,
}

pub struct Touch {
    /// Detected controller; `poll()` dispatches on it.
    ctrl: Ctrl,
    #[cfg_attr(not(feature = "dbg-diag"), allow(dead_code))]
    pub chip: [u8; 4],
    /// Raw first 7 bytes of the last frame (for debugging).
    pub raw: [u8; 7],
    /// Debug ring of the last 8 frames (7 bytes each) and its next index.
    pub ring: [[u8; 7]; 8],
    pub ring_i: u8,
    /// The last Hynitron read found no new report (status 0xAB, `read_hyn`).
    stale: bool,
    /// Frame -> state debounce (pure, see `Debounce`).
    deb: Debounce,
    /// Goodix only: last time the buffer-ready flag was force-polled (see
    /// GT_POLL_MS). Unused on the Hynitron path.
    last_gt_poll: u64,
    /// Last time a frame read was started (LEVEL_MS fallback).
    last_read: u64,
    /// I2C transport (hardware I2C2 or bit-bang) and the DIAG counters.
    bus: Bus,
}

/// A new state (touch-down, a side key, a key release) that is followed by
/// silence is accepted after this, ms.
const DEBOUNCE_MS: u64 = 40;
/// A screen release (last frame had points, this one none) followed by
/// silence is accepted after this, ms. Longer than the controller's report
/// period while touched (stock guess 10..17 ms; DBG:TOUCH? shows it), so one
/// stray release frame inside a touch is still absorbed by the next report.
const RELEASE_MS: u64 = 25;
/// The touch calibration (touchcal.rs; the touch test page's Calibrate,
/// DBG:TOUCH:CAL): Q16 deltas from the identity `[a - 1, b, c, d, e - 1,
/// f]`, x' = a x + b y + c, y' = d x + e y + f. All zero (the zeroed static,
/// no calibration) is the stock's mapping: the controller's coordinates
/// unchanged (touch_v2_6.h COORDINATES). Set only with values that passed
/// touchcal::check (|a - 1|, |b|, |d|, |e - 1| <= 0.15), so the products
/// in `screen` stay far inside i32 for 12-bit coordinates. In SRAM
/// (`.sram_bss`): TCM .bss shortens the stack 1:1.
#[cfg_attr(target_os = "none", link_section = ".sram_bss")]
static mut CAL: [i32; 6] = [0; 6];

/// The calibration in use (touchcal.rs Cal).
pub fn cal() -> [i32; 6] {
    // SAFETY: single-threaded, no ISR touches it.
    unsafe { *core::ptr::addr_of!(CAL) }
}

/// Use calibration `c` (touchcal.rs Cal, checked by the caller) from the
/// next frame on.
pub fn set_cal(c: [i32; 6]) {
    // SAFETY: as `cal`.
    unsafe { *core::ptr::addr_of_mut!(CAL) = c }
}

/// Frame -> state debounce, pure (no I/O; host-tested by tools/touch-test).
///
/// * Moving finger: any change of the points while the screen stays touched
///   is reported at once.
/// * Touch-down: two consecutive pressed frames, whatever their coordinates
///   (the newer point is reported), or one pressed frame followed by
///   DEBOUNCE_MS of silence. An earlier version demanded two byte-identical
///   frames: a finger whose position jitters by 1 px from report to report
///   (rolling onto the glass) was then not accepted until it happened to
///   repeat a point, and a tap whose frames never repeated was lost entirely
///   (its release compared equal to the never-updated idle state).
/// * Release: two release frames, or one followed by RELEASE_MS of silence;
///   a pressed frame in between continues the touch (no split, no second
///   touch-down from a bouncing contact).
/// * Side keys: as touch-down / release, with DEBOUNCE_MS; key reports during
///   a touch or within 300 ms after one are the controller's own long-press
///   gesture and are dropped.
pub struct Debounce {
    last: Event,
    pending: Option<(Event, u64)>,
    /// Last time a screen point was reported (the key filter above).
    last_point: u64,
}

impl Debounce {
    pub const fn new() -> Self {
        Debounce {
            last: Event { points: 0, p: Point { x: 0, y: 0 }, pts: [Point { x: 0, y: 0 }; MAX_POINTS], keys: 0 },
            pending: None,
            last_point: 0,
        }
    }

    /// The current (accepted) state.
    #[allow(dead_code)] // host tests
    pub fn state(&self) -> &Event {
        &self.last
    }

    /// A decoded frame arrived: the new state on a change.
    pub fn frame(&mut self, f: &Frame, now: u64) -> Option<Event> {
        let mut ev = Event::default();
        if f.pressed && !f.key {
            self.last_point = now;
        }
        if f.pressed && f.key && (self.last.points > 0 || now.saturating_sub(self.last_point) < 300) {
            // Long press on the screen reported as a key: ignore.
            return None;
        }
        if f.pressed {
            if f.key {
                ev.keys = 1 << f.code;
            } else {
                ev.points = f.n.clamp(1, MAX_POINTS as u8);
                for i in 0..ev.points as usize {
                    ev.pts[i] = screen(f.pts[i]);
                }
                ev.p = ev.pts[0];
            }
        }
        // Moving finger: accept position updates of the same contact at once.
        if ev.points > 0 && self.last.points > 0 && ev.keys == self.last.keys {
            self.pending = None;
            if ev != self.last {
                self.last = ev;
                return Some(ev);
            }
            return None;
        }
        if ev == self.last {
            self.pending = None;
            return None;
        }
        match self.pending {
            // Second frame of the same kind (pressed / released / same key):
            // accept, with this frame's points.
            Some((q, _)) if (q.points > 0) == (ev.points > 0) && q.keys == ev.keys => {
                self.pending = None;
                self.last = ev;
                Some(ev)
            }
            _ => {
                self.pending = Some((ev, now));
                None
            }
        }
    }

    /// No frame this call: flush a pending state after its silence time.
    pub fn idle(&mut self, now: u64) -> Option<Event> {
        let (ev, t) = self.pending?;
        let release = ev.points == 0 && ev.keys == 0 && self.last.points > 0;
        if now.saturating_sub(t) < if release { RELEASE_MS } else { DEBOUNCE_MS } {
            return None;
        }
        self.pending = None;
        if ev == self.last {
            return None;
        }
        self.last = ev;
        Some(ev)
    }
}

/// A controller point on the 480 x 320 panel: through the calibration
/// (`CAL`, Q16, rounded), clamped. Screen points only: `Debounce::frame`
/// never maps a touch-key report (its X is the key's code position).
fn screen(p: Point) -> Point {
    // SAFETY: as `cal`; a copy, no reference kept.
    let k = unsafe { *core::ptr::addr_of!(CAL) };
    let (x, y) = (p.x as i32, p.y as i32);
    let cx = x + ((k[0] * x + k[1] * y + k[2] + 0x8000) >> 16);
    let cy = y + ((k[3] * x + k[4] * y + k[5] + 0x8000) >> 16);
    Point { x: cx.clamp(0, 479) as u16, y: cy.clamp(0, 319) as u16 }
}

// ---------------------------------------------------------------------------
// DBG:TOUCH? log: one timeline of the frames read, the debounced events and
// the gesture decisions (gesture.rs logs into it), for checking real-finger
// taps on the device. Single-threaded (opendm40 has no ISRs).

/// Entries kept (12 bytes each); ~6 taps with all their frames. Without
/// the feature `dbg-touch` (no DBG:TOUCH?), 16: the touch test page reads
/// the new entries every pass, one touch poll after they were written (a
/// poll logs one frame and its event, plus the gesture decisions).
pub const LOG_N: usize = if cfg!(feature = "dbg-touch") { 160 } else { 16 };

#[derive(Clone, Copy)]
struct LogEntry {
    /// ms since boot (low 32 bits).
    #[cfg_attr(not(feature = "dbg-touch"), allow(dead_code))]
    t: u32,
    /// b'F' frame, b'E' debounced event, b'G' gesture.
    kind: u8,
    /// F: source b'e' edge / b'l' level / b't' timer, | 0x80 = bad frame.
    /// E: points. G: gesture code (see gesture.rs `log_code`).
    a: u8,
    /// F: points in the frame (| 0x80 = key report). E: keys. G: argument.
    b: u8,
    /// F: raw status byte (Hynitron byte 0).
    c: u8,
    x: u16,
    y: u16,
}

struct Log {
    e: [LogEntry; LOG_N],
    /// Entries written since the last clear (the ring holds the last LOG_N).
    n: u32,
}

// TCM .bss (16 entries). The full log of a `dbg-touch` build is in
// .sram_bss: TCM has no 1.9 KB to spare for it next to the stack.
#[cfg_attr(all(target_os = "none", feature = "dbg-touch"), link_section = ".sram_bss")]
static mut LOG: Log = Log { e: [LogEntry { t: 0, kind: 0, a: 0, b: 0, c: 0, x: 0, y: 0 }; LOG_N], n: 0 };

fn log_ref() -> &'static mut Log {
    // SAFETY: single-threaded, no ISR touches the log, and no reference to it
    // is held across calls.
    unsafe { &mut *core::ptr::addr_of_mut!(LOG) }
}

/// Append to the DBG:TOUCH? log (see `LogEntry` for the fields). Not
/// inlined (five callers, once per touch frame / gesture event).
#[inline(never)]
pub fn log(kind: u8, now: u64, a: u8, b: u8, c: u8, p: Point) {
    let l = log_ref();
    l.e[l.n as usize % LOG_N] = LogEntry { t: now as u32, kind, a, b, c, x: p.x, y: p.y };
    l.n = l.n.wrapping_add(1);
}

#[cfg(feature = "dbg-touch")] // the DBG:TOUCH hook (main.rs)
pub fn log_clear() {
    log_ref().n = 0;
}

#[cfg(feature = "dbg-touch")] // the DBG:TOUCH? hook (main.rs)
/// The log as text, oldest first, into `out`; returns the length. First line
/// `TOUCH,<now ms>,<entries since clear>,<entries shown>`, then one line per
/// entry:
///   `<t>,F,<src e|l|t>,<points>,<x>,<y>,<status hex>`  frame read (src `x`:
///       bad frame, dropped); points `k<code>` = side-key report
///   `<t>,E,<points>,<keys>,<x>,<y>`                    debounced state change
///   `<t>,G,<code>,<arg>,<x>,<y>`                       gesture decision
/// Lines that do not fit are left out (6 KB holds the whole ring).
#[inline(never)] // .cold (link-native.x): not inlined into the hot caller
pub fn log_text(out: &mut [u8], now: u64) -> usize {
    let l = log_ref();
    let shown = (l.n as usize).min(LOG_N);
    let mut w = W { out, n: 0, full: false };
    w.s(b"TOUCH,");
    w.num(now as u32);
    w.s(b",");
    w.num(l.n);
    w.s(b",");
    w.num(shown as u32);
    w.s(b"\n");
    let first = l.n as usize - shown;
    for k in first..first + shown {
        let e = l.e[k % LOG_N];
        let mark = w.n;
        w.num(e.t);
        w.s(b",");
        w.s(&[e.kind, b',']);
        match e.kind {
            b'F' => {
                w.s(&[if e.a & 0x80 != 0 { b'x' } else { e.a }, b',']);
                if e.b & 0x80 != 0 {
                    w.s(b"k");
                }
                w.num((e.b & 0x7F) as u32);
            }
            b'E' => {
                w.num(e.a as u32);
                w.s(b",");
                w.num(e.b as u32);
            }
            _ => {
                w.s(&[e.a, b',']);
                w.num(e.b as u32);
            }
        }
        w.s(b",");
        w.num(e.x as u32);
        w.s(b",");
        w.num(e.y as u32);
        if e.kind == b'F' {
            let h = b"0123456789abcdef";
            w.s(&[b',', h[(e.c >> 4) as usize], h[(e.c & 15) as usize]]);
        }
        w.s(b"\n");
        if w.full {
            w.n = mark;
            break;
        }
    }
    w.n
}

/// Bounded text writer for `log_text` (`full` once something did not fit).
#[cfg(feature = "dbg-touch")]
struct W<'a> {
    out: &'a mut [u8],
    n: usize,
    full: bool,
}

#[cfg(feature = "dbg-touch")]
impl W<'_> {
    fn s(&mut self, b: &[u8]) {
        if self.n + b.len() <= self.out.len() {
            self.out[self.n..self.n + b.len()].copy_from_slice(b);
            self.n += b.len();
        } else {
            self.full = true;
        }
    }
    fn num(&mut self, mut v: u32) {
        let mut t = [0u8; 10];
        let mut i = t.len();
        loop {
            i -= 1;
            t[i] = b'0' + (v % 10) as u8;
            v /= 10;
            if v == 0 {
                break;
            }
        }
        self.s(&t[i..]);
    }
}

/// Entries written to the log since the last clear (the ring holds the last
/// LOG_N of them): the touch test page's cursor (app_touchtest.rs).
pub fn log_count() -> u32 {
    log_ref().n
}

/// Entry `i` (0 = the first since the last clear) as (kind, a, b, c, point),
/// see `LogEntry`; None when not written yet or already overwritten.
pub fn log_entry(i: u32) -> Option<(u8, u8, u8, u8, Point)> {
    let l = log_ref();
    if i >= l.n || l.n - i > LOG_N as u32 {
        return None;
    }
    let e = l.e[i as usize % LOG_N];
    Some((e.kind, e.a, e.b, e.c, Point { x: e.x, y: e.y }))
}

fn int_low() -> bool {
    (unsafe { r32(GPIOE + 0x10) } & (1 << INT)) == 0
}

fn decode(b: &[u8]) -> (bool, bool, Point) {
    let key = b[0] & 0x80 != 0;
    let pressed = if key { b[0] & 0x0F != 0 } else { b[0] & 0x0F == 6 };
    let x = (b[1] as u16) << 4 | (b[3] >> 4) as u16;
    let y = (b[2] as u16) << 4 | (b[3] & 0x0F) as u16;
    (pressed, key, Point { x, y })
}

impl Touch {
    /// Reset and initialise the controller. None if it does not answer.
    #[inline(never)] // .cold (link-native.x): start-up
    pub fn new(clk: &mut Clock) -> Option<Self> {
        od_pin(GPIOA, SCL);
        od_pin(GPIOC, SDA);
        unsafe {
            // INT: input, pull-up. RST: push-pull output, low.
            modify32(GPIOE + 0x00, 3 << (2 * INT), 0);
            modify32(GPIOE + 0x0C, 3 << (2 * INT), 1 << (2 * INT));
            modify32(GPIOE + 0x04, 1 << RST, 0);
            w32(GPIOE + 0x18, 1 << (RST + 16));
            modify32(GPIOE + 0x00, 3 << (2 * RST), 1 << (2 * RST));
        }
        unsafe {
            // SCL / SDA alternate function 4 = I2C2 (used once MODER = AF).
            modify32(GPIOA + 0x24, 0xF << (4 * (SCL - 8)), 4 << (4 * (SCL - 8)));
            modify32(GPIOC + 0x24, 0xF << (4 * (SDA - 8)), 4 << (4 * (SDA - 8)));
        }
        // Free a possibly stuck bus, then measure the pull-up edges while the
        // controller is held in reset (its pins are released then).
        bus_clear();
        let rise_scl_ns = rise_ns(GPIOA, SCL);
        let rise_sda_ns = rise_ns(GPIOC, SDA);

        clk.delay_ms(20);
        unsafe { w32(GPIOE + 0x18, 1 << RST) };
        clk.delay_ms(200);

        // Peripheral first, 400 kHz only when both edges are fast enough; if
        // nothing answers, retry slower and finally on the bit-bang.
        let first = if rise_scl_ns <= FAST_RISE_NS && rise_sda_ns <= FAST_RISE_NS { Mode::Hw400 } else { Mode::Hw100 };
        let mut bus = Bus { mode: Mode::BitBang, fault_streak: 0, bad_streak: 0, edge_read: false, stats: Stats::default() };
        bus.stats.rise_scl_ns = rise_scl_ns;
        bus.stats.rise_sda_ns = rise_sda_ns;
        let order: &[Mode] =
            if first == Mode::Hw400 { &[Mode::Hw400, Mode::Hw100, Mode::BitBang] } else { &[Mode::Hw100, Mode::BitBang] };
        let mut found = None;
        for &mode in order {
            bus.enter(mode);
            bus.stats.probe_mode = mode as u8;
            found = bus.probe(clk);
            if found.is_some() {
                break;
            }
        }
        let (ctrl, chip) = found?;
        exti_init();
        Some(Touch {
            ctrl,
            chip,
            raw: [0; 7],
            ring: [[0; 7]; 8],
            ring_i: 0,
            stale: false,
            deb: Debounce::new(),
            last_gt_poll: 0,
            last_read: 0,
            bus,
        })
    }

    /// An INT edge is latched: a fresh report is waiting, so the main loop can
    /// call `poll` now instead of on its 10 ms tick.
    pub fn irq_pending(&self) -> bool {
        exti_pending()
    }

    /// DBG:DIAG? block (40 bytes, little endian):
    ///  0 bus mode (0 bit-bang, 1 HW 100 kHz, 2 HW 400 kHz), 1 controller
    ///  (0 0x5A, 1 GT1158), 2 SCL rise ns u16, 4 SDA rise ns u16, 6 last frame
    ///  us u16, 8 max frame us u16, 10 last frame points, 11 probe mode,
    ///  12 edge reads, 16 level-fallback reads, 20 Goodix timer reads,
    ///  24 frames ok, 28 frames bad (0xAB), 32 NAKs, 36 HW faults (u32 each).
    #[cfg_attr(not(feature = "dbg-diag"), allow(dead_code))]
    pub fn diag(&self) -> [u8; 40] {
        let s = &self.bus.stats;
        let mut d = [0u8; 40];
        d[0] = self.bus.mode as u8;
        d[1] = (self.ctrl == Ctrl::Goodix) as u8;
        d[2..4].copy_from_slice(&s.rise_scl_ns.to_le_bytes());
        d[4..6].copy_from_slice(&s.rise_sda_ns.to_le_bytes());
        d[6..8].copy_from_slice(&s.last_us.to_le_bytes());
        d[8..10].copy_from_slice(&s.max_us.to_le_bytes());
        d[10] = s.last_n;
        d[11] = s.probe_mode;
        let c = [s.edge_reads, s.level_reads, s.timer_reads, s.frames_ok, s.frames_bad, s.naks, s.faults];
        for (i, v) in c.iter().enumerate() {
            d[12 + 4 * i..16 + 4 * i].copy_from_slice(&v.to_le_bytes());
        }
        d
    }

    /// Poll: reads a frame when due, dispatches to the detected controller, and
    /// returns the new (debounced) state on a change. The decoded `Frame`
    /// (pressed, key, the pressed points, key code) is identical for both chips,
    /// so the debounce tail below and the rest of the firmware never see the
    /// panel. A change of the point count or of any point while the screen
    /// stays touched is reported at once (no debounce), like a moving finger.
    pub fn poll(&mut self, now: u64) -> Option<Event> {
        // A frame is read on an INT falling edge (EXTI line 5 pending bit,
        // cleared before the read), or when INT is still low LEVEL_MS after the
        // last read (the old level-polled cadence, so a controller holding INT
        // low or an edge whose frame was dropped cannot stall touch). Goodix is
        // additionally force-polled on a timer: its INT behaviour is unknown and
        // the main loop can stall (flash writes). The `ctrl == Goodix` guard
        // short-circuits on this 0x5A unit.
        let edge = exti_take();
        let level = !edge && int_low() && now.saturating_sub(self.last_read) >= LEVEL_MS;
        let timer = !edge
            && !level
            && self.ctrl == Ctrl::Goodix
            && now.saturating_sub(self.last_gt_poll) >= GT_POLL_MS;
        let st = &mut self.bus.stats;
        if edge {
            st.edge_reads = st.edge_reads.wrapping_add(1);
        } else if level {
            st.level_reads = st.level_reads.wrapping_add(1);
        } else if timer {
            st.timer_reads = st.timer_reads.wrapping_add(1);
        }

        let frame = if edge || level || timer {
            self.last_read = now;
            self.bus.edge_read = edge;
            let t0 = cyc();
            let f = match self.ctrl {
                // A bad/absent 0x5A frame returns without touching pending, as
                // the original inline path did.
                Ctrl::Hynitron => self.read_hyn(),
                // A "no new data" Goodix read (buffer-ready clear) returns None
                // and falls through to the debounce flush below.
                Ctrl::Goodix => {
                    self.last_gt_poll = now;
                    self.read_goodix()
                }
            };
            let st = &mut self.bus.stats;
            st.last_us = (cyc().wrapping_sub(t0) / CYC_PER_US).min(u16::MAX as u32) as u16;
            st.max_us = st.max_us.max(st.last_us);
            st.last_n = f.as_ref().map_or(0, |f| f.n);
            let src = if edge { b'e' } else if level { b'l' } else { b't' };
            match &f {
                Some(fr) => {
                    // Raw record-0 coordinates (also those of a release or
                    // key report), before the calibration / clamping.
                    let pts = if fr.key { 0x80 | fr.code } else { fr.n };
                    let p = if self.ctrl == Ctrl::Hynitron { decode(&self.raw).2 } else { fr.pts[0] };
                    log(b'F', now, src, pts, self.raw[0], p);
                }
                // 0x5A: bad / absent frame; Goodix: buffer not ready (not logged).
                // A stale buffer (status 0xAB) is logged with that status.
                None if self.ctrl == Ctrl::Hynitron && self.stale => log(b'F', now, src | 0x80, 0, 0xAB, Point::default()),
                None if self.ctrl == Ctrl::Hynitron => log(b'F', now, src | 0x80, 0, 0, Point::default()),
                None => {}
            }
            // A bad frame leaves the debounce as it is; a stale buffer is no
            // new data, so the pending state is flushed after its silence.
            if f.is_none() && self.ctrl == Ctrl::Hynitron && !self.stale {
                return None;
            }
            f
        } else {
            None
        };
        // No new frame this call: flush a pending state after its silence.
        let ev = match frame {
            Some(f) => self.deb.frame(&f, now),
            None => self.deb.idle(now),
        }?;
        log(b'E', now, ev.points, ev.keys, 0, ev.p);
        Some(ev)
    }

    /// Hynitron 0x5A frame read — byte-identical bus traffic to the original
    /// inline path (D0 00 read 7, 0xAB check, extra points D0 07/0C/11/16, then
    /// the D0 00 AB ack). Multi-touch (touch_v2_6.h FRAME READ / DRIVER OUTPUT):
    /// rx[5] & 7 = n records; record 0 is in the 7-byte frame, records 1..n-1
    /// (5 bytes each, same layout) follow. A record is a pressed point when its
    /// status low nibble is 6; released records are dropped and the pressed ones
    /// packed in slot order (as the stock queue entry: "n, minus 1 for each
    /// released record"). `code` is the side-key code derived from the X of a
    /// key report (record 0 bit 7). None (bad/absent frame) means "no frame"
    /// and is handled without touching pending, as before.
    fn read_hyn(&mut self) -> Option<Frame> {
        let mut b = [0u8; 7];
        self.stale = false;
        let good = self.bus.read_reg(0xD000, &mut b) && b[6] == 0xAB;
        self.bus.frame_check(good);
        if !good {
            return None;
        }
        // Status 0xAB: the buffer as our clear (D0 00 AB) left it, no new
        // report. A read on the INT level while a finger is down returns it
        // between the real frames (device 2026-10-25, DBG:TOUCH?: 0x06 frames
        // interleaved with 0xAB ones every 10 ms); decoded, it is a key-0
        // (MENU) press that breaks the touch debounce.
        self.stale = b[0] == 0xAB;
        if self.stale {
            return None;
        }
        self.raw = b;
        self.ring[(self.ring_i & 7) as usize] = b;
        self.ring_i = self.ring_i.wrapping_add(1);
        let n = b[5] & 7;
        let (pressed0, key, p) = decode(&b);
        let mut f = Frame { pressed: pressed0, key, n: 0, pts: [Point::default(); MAX_POINTS], code: 0 };
        if pressed0 && !key {
            f.pts[0] = p;
            f.n = 1;
        }
        // Extra points: always read, so the controller sees the complete
        // transaction, as the stock driver does.
        for i in 1..n.min(MAX_POINTS as u8) {
            let mut e = [0u8; 5];
            if self.bus.read_reg(0xD000 + 7 + 5 * (i as u16 - 1), &mut e) && !key {
                let (pr, k, q) = decode(&e);
                if pr && !k && (f.n as usize) < MAX_POINTS {
                    f.pts[f.n as usize] = q;
                    f.n += 1;
                }
            }
        }
        self.bus.write(&[0xD0, 0x00, 0xAB]);
        if !key {
            f.pressed = f.n > 0;
        }
        f.code = match p.x {
            0..=499 => 0,
            500..=749 => 1,
            750..=999 => 2,
            _ => 3,
        };
        Some(f)
    }

    /// Goodix GT1158 frame read (header §Goodix, lines 98/110-118). Correct-by-
    /// construction from the stock reverse-engineering; it cannot be exercised on
    /// this 0x5A unit. Normalises Goodix coordinates and keys into the same tuple
    /// the shared debounce path (and therefore gesture.rs / app.rs) expect, so no
    /// other file changes. None = no new data (buffer-ready bit clear); the buffer
    /// is NOT acked in that case (header 112 clears only a ready buffer) and the
    /// force-poll timer re-checks, so there is no hang.
    fn read_goodix(&mut self) -> Option<Frame> {
        let mut st = [0u8; 1];
        if !self.bus.read_reg_at(ADDR_GOODIX, GT_REG_STATUS, &mut st) {
            return None;
        }
        let status = st[0];
        if status & 0x80 == 0 {
            return None; // buffer not ready: nothing new, nothing to clear
        }
        self.bus.stats.frames_ok = self.bus.stats.frames_ok.wrapping_add(1);
        let n = status & 0x0F; // bits 3..0 = point count
        let have_key = status & 0x10 != 0; // bit 4 = HaveKey

        // Stock order (header line 112): read status, clear the buffer, then read
        // the point/key records.
        self.bus.write_at(ADDR_GOODIX, &[(GT_REG_STATUS >> 8) as u8, GT_REG_STATUS as u8, 0x00]);

        let mut pressed = false;
        let mut pts = [Point::default(); MAX_POINTS];
        let mut np = 0u8;
        // Every reported point i: 4 bytes at 0x8150 + 8*i = X lo, X hi, Y lo,
        // Y hi (header line 113; the stock reads them all, table 0x0804976c).
        for i in 0..n.min(MAX_POINTS as u8) {
            let mut pt = [0u8; 4];
            if self.bus.read_reg_at(ADDR_GOODIX, GT_REG_POINT0 + 8 * i as u16, &mut pt) {
                let gx = (pt[1] as u16) << 8 | pt[0] as u16; // Goodix native X
                let gy = (pt[3] as u16) << 8 | pt[2] as u16; // Goodix native Y
                // Native portrait (320x480) -> landscape GUI, no scale/mirror
                // (header 115-116): GUI x = Goodix Y, GUI y = Goodix X.
                pts[np as usize] = Point { x: gy, y: gx };
                np += 1;
                pressed = true;
            }
        }
        let p = pts[0];

        let mut key = false;
        let mut code = 0u8;
        if have_key {
            // Key byte at 0x814F + 8*n (header 98/113): one-hot 1/2/4/8 -> code
            // 0..3. Mask to the 4 valid bits so a spurious value cannot shift the
            // event bit past bit 3.
            let mut kb = [0u8; 1];
            if self.bus.read_reg_at(ADDR_GOODIX, GT_REG_KEY0 + 8 * n as u16, &mut kb) {
                let kv = kb[0] & 0x0F;
                if kv != 0 {
                    code = kv.trailing_zeros() as u8;
                    key = true;
                    pressed = true;
                }
            }
        }

        // Debug mirror for DBG:DIAG: [status, GUIx lo, GUIx hi, GUIy lo, GUIy hi,
        // key|0x80, 0] — the Goodix format has no 7-byte frame of its own.
        let rawf = [
            status,
            (p.x & 0xFF) as u8,
            (p.x >> 8) as u8,
            (p.y & 0xFF) as u8,
            (p.y >> 8) as u8,
            if key { code | 0x80 } else { 0 },
            0,
        ];
        self.raw = rawf;
        self.ring[(self.ring_i & 7) as usize] = rawf;
        self.ring_i = self.ring_i.wrapping_add(1);

        if key {
            np = 0; // a key report carries no screen point
        }
        Some(Frame { pressed, key, n: np, pts, code })
    }
}
