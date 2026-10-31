//! Battery and power status (dmm_v2_6.h section 9).
//! Battery: ADC0 channel 15 on PC5, Vbat = raw * 3.0 / 4096 * 2, read every 2 s;
//! pct = (Vbat - 3.3) / 0.9; bars 0..5 at 85/70/50/25/0 % with 5 % hysteresis.
//! Charger (TP4056, hw/README.md "Charge status pins"; device 2026-10-29,
//! DBG:CHG:LOG? across a USB power cut): PB10 = 1 while the charger has
//! input power (0 within ms of the cut; stays 1 against the pull-down);
//! PB11 = the charger's STDBY (open drain, PB11 pull-up as the stock: low
//! = charge terminated, high while charging or without input). So: PB10 0 =
//! no charger, PB10 1 + PB11 1 = charging (the stock's condition,
//! FUN_0800b776 / FUN_08023840), PB10 1 + PB11 0 = charged. The charger's
//! CHRG pin is not on an MCU pin found. Not copied: the stock pulses PB10 low
//! for ~10 ms per 2 s on USB power below 80..85 % and reads the battery during
//! the pulse (hw/README.md Unknowns 5); PB10 low stops the charge (device).
//! Bluetooth link: PC14 high = connected.
//! MCU temperature: the GD32 internal sensor on ADC0 channel 16 (TSVREN, bit 23
//! of ADC_SYNCCTL at ADC0 + 0x304), read right after the battery every 2 s,
//! sample time 480 cycles (~19 us at PCLK2/4), against the same 3.0 V
//! reference: T = (V25 - V) / slope + 25. V25 / slope are typical GD32 values
//! (no factory TS calibration on the GD32F4, none found in OTP / system memory
//! by this code): expect several C of offset per chip.

use crate::mmio::{modify32, r32, w32};

const RCU_APB2EN: u32 = 0x4002_3844;
const ADC0: u32 = 0x4001_2000;
const GPIOB: u32 = 0x4002_0400;
const GPIOC: u32 = 0x4002_0800;

/// A USB host has the meter configured (main.rs, every pass; DBG:CHG?).
pub static USB_CONF: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// The last 8 charge pin changes (DBG:CHG:LOG?): ms << 4 | USB << 2 |
/// PB11 << 1 | PB10; `last` starts at 0xFF so the boot state is logged.
pub struct ChgLog {
    pub n: u32,
    last: u8,
    pub ev: [u32; 8],
}
pub static mut CHG_LOG: ChgLog = ChgLog { n: 0, last: 0xFF, ev: [0; 8] };

/// DBG:CHG:PIN n,<mode>: set PB10 / PB11 for a charger experiment.
/// mode 0 input no pull, 1 input pull-up, 2 input pull-down, 3 push-pull
/// low, 4 push-pull high, 5 open drain released, 6 the default (`pins_init`).
pub fn dbg_pin(pin: u32, mode: u32) -> bool {
    if !(pin == 10 || pin == 11) || mode > 6 {
        return false;
    }
    if mode == 6 {
        pins_init();
        return true;
    }
    let (m2, sh) = (3u32 << (2 * pin), 2 * pin);
    unsafe {
        let pupd = match mode { 1 => 1, 2 => 2, _ => 0 };
        modify32(GPIOB + 0x0C, m2, pupd << sh);
        modify32(GPIOB + 0x04, 1 << pin, if mode == 5 { 1 << pin } else { 0 }); // OD
        match mode {
            3 => w32(GPIOB + 0x28, 1 << pin), // BC: low
            4 | 5 => w32(GPIOB + 0x18, 1 << pin), // BOP: high / released
            _ => {}
        }
        modify32(GPIOB, m2, if mode >= 3 { 1 << sh } else { 0 });
    }
    true
}

/// DBG:CHG?: PB10 level, PB11 level, GPIOB MODE / PUD / OMODE / OCTL bits
/// of PB10..11 (hex digits: bits 20..23 >> 20 etc.), USB configured, battery
/// ADC (sum of 16 raw conversions; Vbat = sum / 16 * 3 / 4096 * 2).
pub fn dbg_state() -> [u32; 8] {
    let mut sum = 0;
    for _ in 0..16 {
        sum += convert(15);
    }
    unsafe {
        let idr = r32(GPIOB + 0x10);
        [
            idr >> 10 & 1,
            idr >> 11 & 1,
            r32(GPIOB) >> 20 & 0xF,
            r32(GPIOB + 0x0C) >> 20 & 0xF,
            r32(GPIOB + 0x04) >> 10 & 3,
            r32(GPIOB + 0x14) >> 10 & 3,
            USB_CONF.load(core::sync::atomic::Ordering::Relaxed) as u32,
            sum,
        ]
    }
}

/// PB10, PB11 as the stock sets them up (FUN_0800b5f6): PB11 input with
/// pull-up; PB10 open drain, released (no pull), read as an input.
fn pins_init() {
    unsafe {
        modify32(GPIOB + 0x0C, 0xF << 20, 1 << 22);
        modify32(GPIOB + 0x04, 3 << 10, 0);
        modify32(GPIOB, 0xF << 20, 0);
    }
}

/// Typical GD32 temperature sensor constants (unverified for the F427).
const TS_V25: f32 = 1.43;
const TS_SLOPE: f32 = 0.0043;

pub struct Power {
    next: u64,
    pub vbat: f32,
    pub bars: u8,
    /// MCU die temperature, C (NaN until the first read).
    pub mcu_t: f32,
}

/// One software-started conversion of ADC0 channel `ch` (raw 12 bit).
fn convert(ch: u32) -> u32 {
    unsafe {
        w32(ADC0 + 0x34, ch);
        w32(ADC0 + 0x00, 0);
        modify32(ADC0 + 0x08, 0, 1 << 30); // SWSTART
        let mut n = 0;
        while r32(ADC0 + 0x00) & 2 == 0 && n < 10000 {
            n += 1;
        }
        r32(ADC0 + 0x4C) & 0xFFF
    }
}

impl Power {
    pub fn new() -> Self {
        unsafe {
            modify32(RCU_APB2EN, 0, 1 << 8); // ADC0
            modify32(GPIOC, 3 << 10, 3 << 10); // PC5 analog
            modify32(GPIOC, 3 << 28, 0); // PC14 input
            modify32(GPIOC + 0x0C, 3 << 28, 2 << 28); // pull-down
            modify32(ADC0 + 0x304, 3 << 16, 1 << 16); // ADC clock PCLK2 / 4
            modify32(ADC0 + 0x304, 0, 1 << 23); // TSVREN: temperature sensor on
            w32(ADC0 + 0x08, 0); // CR2 off
            w32(ADC0 + 0x0C, 6 << 15 | 7 << 18); // SMPR1: ch15 144, ch16 480 cycles
            w32(ADC0 + 0x2C, 0); // SQR1: 1 conversion
            w32(ADC0 + 0x34, 15); // SQR3: ch15
            w32(ADC0 + 0x08, 1); // ADON
        }
        pins_init();
        Power { next: 0, vbat: 4.2, bars: 5, mcu_t: f32::NAN }
    }

    pub fn poll(&mut self, now: u64) -> bool {
        // Charge pin changes for DBG:CHG:LOG? (every pass: one IDR read).
        let p = self.pins();
        unsafe {
            let l = &mut *core::ptr::addr_of_mut!(CHG_LOG);
            if p != l.last {
                l.last = p;
                l.ev[l.n as usize % 8] = (now as u32) << 4 | p as u32 | (USB_CONF.load(core::sync::atomic::Ordering::Relaxed) as u32) << 2;
                l.n = l.n.wrapping_add(1);
            }
        }
        if now < self.next {
            return false;
        }
        self.next = now + 2000;
        self.vbat = convert(15) as f32 * 3.0 / 4096.0 * 2.0;
        let v = convert(16) as f32 * 3.0 / 4096.0;
        unsafe { w32(ADC0 + 0x34, 15) };
        self.mcu_t = (TS_V25 - v) / TS_SLOPE + 25.0;
        let pct = (self.vbat - 3.3) / 0.9 * 100.0;
        let th = [0.0, 25.0, 50.0, 70.0, 85.0];
        let mut b = self.bars;
        while b < 5 && pct > th[b as usize] + 5.0 {
            b += 1;
        }
        while b > 0 && pct < th[b as usize - 1] {
            b -= 1;
        }
        let changed = b != self.bars;
        self.bars = b;
        changed
    }

    /// Debug: sample ADC0 channel `ch` n times back to back (raw 12-bit),
    /// then restore the battery channel. Returns (min, max, mid crossings,
    /// elapsed cycles).
    pub fn sample(&mut self, ch: u32, n: usize) -> (u16, u16, u32, u32) {
        let (mut lo, mut hi) = (u16::MAX, 0u16);
        let mut buf = [0u16; 512];
        let n = n.min(512);
        let t0 = unsafe { r32(0xE000_1004) };
        unsafe {
            w32(ADC0 + 0x10, 0); // SMPR2: shortest sample time on ch0..9
            w32(ADC0 + 0x34, ch);
            for v in buf[..n].iter_mut() {
                w32(ADC0 + 0x00, 0);
                modify32(ADC0 + 0x08, 0, 1 << 30);
                let mut k = 0;
                while r32(ADC0 + 0x00) & 2 == 0 && k < 10000 {
                    k += 1;
                }
                *v = (r32(ADC0 + 0x4C) & 0xFFF) as u16;
                crate::clock::delay_us(40); // ~20 ms window for 512 samples
            }
            w32(ADC0 + 0x34, 15);
        }
        let dt = unsafe { r32(0xE000_1004) }.wrapping_sub(t0);
        for &v in &buf[..n] {
            lo = lo.min(v);
            hi = hi.max(v);
        }
        let mid = (lo as u32 + hi as u32) / 2;
        let hy = ((hi - lo) as u32 / 8).max(4);
        let (mut below, mut cross) = (buf[0] as u32 <= mid, 0u32);
        for &v in &buf[..n] {
            let v = v as u32;
            if below && v > mid + hy {
                below = false;
                cross += 1;
            } else if !below && v + hy < mid {
                below = true;
            }
        }
        self.next = 0;
        (lo, hi, cross, dt)
    }

    /// Charging: input power and STDBY not active (PB10 = PB11 = 1).
    pub fn charging(&self) -> bool {
        self.pins() == 3
    }

    /// On a charger (USB power present, PB10 = 1): charging or charged.
    pub fn on_charger(&self) -> bool {
        self.pins() & 1 != 0
    }

    /// Charge status pins: bit 0 PB10, bit 1 PB11 (DBG:DIAG? byte 61).
    pub fn pins(&self) -> u8 {
        let b = unsafe { r32(GPIOB + 0x10) };
        (b >> 10 & 3) as u8
    }

    /// Battery level as shown: while charging, the top bar blinks (stock:
    /// level and level - 1, 1 -> 0 bars at 0, alternating every 1 s;
    /// FUN_08023840 and its timer 0x08023e88). `phase`: the 1 s alternation.
    pub fn shown_bars(&self, phase: bool) -> u8 {
        let b = self.bars.min(5);
        match (self.charging() && phase, b) {
            (false, _) => b,
            (true, 0) => 1,
            (true, _) => b - 1,
        }
    }

    pub fn bt_connected(&self) -> bool {
        unsafe { r32(GPIOC + 0x10) & (1 << 14) != 0 }
    }
}
