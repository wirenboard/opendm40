//! Multimeter engine: a port of the stock measurement behaviour.
//! Sources: stock/re/analysis/hy3131_v2_6.h (bus, config rows, calibration
//! layout) and dmm_v2_6.h (per-mode formulas, filters, settle deadlines,
//! autorange, display formats). Stock field names in comments (m+x, S+x).
//!
//! Modes (HY3131 config tables): 0 ACV, 1 DCV, 2 ACmV, 3 DCmV, 4..6 AC A/mA/uA,
//! 7..9 DC A/mA/uA, 10 ohm, 11 continuity, 12 diode, 13 capacitance,
//! 14 frequency, 15 temperature. AC+DC (stock sub-mode m+5 == 2, offered by
//! the stock only for model byte != 0, here on every model) alternates the
//! HY3131 between the AC and the DC mode of the same quantity (`acdc`).
//!
//! Counts: always the DM40C (60000-count) limits, whatever the model byte says
//! (COUNTS_MODEL). The model byte itself (`model()`) still identifies the
//! instrument (About page, BT name / cmd 00).

use core::ptr::read_volatile;

use crate::clock::Clock;
use crate::fastvos::{self, Fast};
use crate::hy3131 as hy;
use crate::hy3131_tables::*;
use crate::mmio::{modify32, w32};
use crate::sentinel::{self, Alert, Kind, Sentinel};

/// Power meter (function 18): docs/power-meter-and-misconnect.md section 3.
#[path = "powermeter.rs"]
pub mod pw;
#[path = "acdctr.rs"]
pub mod acdctr;
use acdctr::{self as tr, TrEnt};
/// AC+DC methods B / C (DBG:ACDC:METH): only with the feature `acdc-meth`
/// (Cargo.toml); without it method A (the stock alternation) is all there
/// is and the hooks below are empty.
#[cfg(feature = "acdc-meth")]
#[path = "acdcm.rs"]
pub mod am;
#[cfg(not(feature = "acdc-meth"))]
mod am {
    use super::*;
    impl Dmm {
        #[inline(always)]
        pub(super) fn am_on(&self) -> bool {
            false
        }
        #[inline(always)]
        pub(super) fn am_enter(&mut self) {}
        #[inline(always)]
        pub(super) fn am_mode(&mut self) -> bool {
            false
        }
        #[inline(always)]
        pub(super) fn am_row(&mut self, cfg: &[u8; hy::CFG_LEN]) -> [u8; hy::CFG_LEN] {
            *cfg
        }
        #[inline(always)]
        pub(super) fn am_ref(&mut self, _: &mut Clock, _: f32, _: f32, _: u8, _: &'static str, _: f32, _: (u8, i8)) -> Option<Reading> {
            None
        }
        #[inline(always)]
        pub(super) fn poll_am(&mut self, _: &mut Clock, _: u64) -> Option<Reading> {
            None
        }
    }
}
#[cfg(all(target_os = "none", feature = "dbg-src"))]
#[path = "dbgsrc_dmm.rs"]
pub mod src;
/// DBG:SRC is firmware only (the host tests that include dmm.rs get it
/// off) and needs the feature `dbg-src` (not default on
/// test/scope-chain-0.4, Cargo.toml): without it DBG:SRC is an unknown
/// header.
#[cfg(not(all(target_os = "none", feature = "dbg-src")))]
mod src {
    pub fn active() -> bool {
        false
    }
    pub fn row(_: u8, _: usize) -> Option<[u8; super::hy::CFG_LEN]> {
        None
    }
    impl super::Dmm {
        pub(super) fn poll_src(&mut self, _: u64) -> Option<super::Reading> {
            None
        }
    }
}
use pw::Pw;

const GPIOA: u32 = 0x4002_0000;
const GPIOC: u32 = 0x4002_0800;
const GPIOE: u32 = 0x4002_1000;

/// Settings record in internal flash (read only): u16 crc, u16 ~crc,
/// u32 version = 1, data. DMM calibration at record offset 0x168.
const SETTINGS: u32 = 0x080C_0000;
const CAL: u32 = SETTINGS + 0x168;

/// A RAM copy of the record's first REC_LEN bytes (the stock record is 800),
/// made once at start-up (`load_record`, main.rs, before the App): the
/// record is in the slow flash bank (not zero-wait: a load stalls the AD2
/// stream ISR ~21.8 us, device c92b605), and its readers run while a stream
/// may (`ad2_scale` per Wave view frame / LOG:SCALe?, the scope's gain /
/// offset pairs, hwtrig's map: emu/regionc.py data watch, 2026-10-23).
/// opendm40 never writes the record. Before the copy (and in the host tests)
/// the flash is read.
/// TCM .bss (CPU only; was .sram_bss until the SRAM audit, docs/native-boot.md).
const REC_LEN: usize = 0x400;
static REC_COPY: [core::sync::atomic::AtomicU32; REC_LEN / 4] = [const { core::sync::atomic::AtomicU32::new(0) }; REC_LEN / 4];
static REC_LOADED: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// Start-up: copy the record (see REC_COPY). A record whose CRC does not
/// match is copied as erased flash (all 0xFF), as the stock treats it (it
/// falls back to its calibration defaults, model-variants.md "Bad CRC"):
/// the version word is then not 1 (`cal_ok`), every float is a NaN, so
/// every reader (`cal`, the scope's `cal_pair`, hwtrig) takes its default.
#[cfg_attr(not(target_os = "none"), allow(dead_code))]
pub fn load_record() {
    for (i, w) in REC_COPY.iter().enumerate() {
        w.store(unsafe { read_volatile((SETTINGS + 4 * i as u32) as *const u32) }, core::sync::atomic::Ordering::Relaxed);
    }
    let ok = rec_crc_ok(|i| REC_COPY[i].load(core::sync::atomic::Ordering::Relaxed));
    REC_CRC_OK.store(ok, core::sync::atomic::Ordering::Relaxed);
    if !ok {
        for w in REC_COPY.iter() {
            w.store(u32::MAX, core::sync::atomic::Ordering::Relaxed);
        }
    }
    REC_LOADED.store(true, core::sync::atomic::Ordering::Release);
}

/// End of the CRC-covered bytes: the stock record is 800 bytes, the CRC
/// covers bytes 4..0x320 (model-variants.md, the Record table).
pub const REC_CRC_END: usize = 0x320;
static REC_CRC_OK: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(true);

/// Whether the record copied at start-up passed its CRC (true before the copy).
pub fn rec_crc_ok_at_boot() -> bool {
    REC_CRC_OK.load(core::sync::atomic::Ordering::Relaxed)
}

/// `word(i)` = the record's little-endian word i. Word 0 holds the CRC-16
/// (poly 0x8005, init 0xFFFF, MSB first, no final XOR) of bytes
/// 4..REC_CRC_END in its low half and the complement in its high half.
pub fn rec_crc_ok(word: impl Fn(usize) -> u32) -> bool {
    let mut c = 0xFFFFu16;
    for i in 1..REC_CRC_END / 4 {
        for b in word(i).to_le_bytes() {
            c ^= (b as u16) << 8;
            for _ in 0..8 {
                c = if c & 0x8000 != 0 { (c << 1) ^ 0x8005 } else { c << 1 };
            }
        }
    }
    let w0 = word(0);
    w0 & 0xFFFF == c as u32 && w0 >> 16 == (!c) as u32
}

/// The record's word at byte offset `off` (4-aligned), from the copy.
pub fn rec_u32(off: u32) -> u32 {
    match REC_COPY.get(off as usize / 4) {
        Some(w) if REC_LOADED.load(core::sync::atomic::Ordering::Acquire) => w.load(core::sync::atomic::Ordering::Relaxed),
        _ => unsafe { read_volatile((SETTINGS + off) as *const u32) },
    }
}

/// Model index used for the counts-dependent limits: 0 = DM40A (40000 counts),
/// 1 = DM40B (50000), 2 = DM40C (60000). The stock picks FACTOR/OVER by the
/// calibration model byte (dmm_v2_6.h:297-299, hy3131_v2_6.h:147-155); the HY3131
/// setup is identical for all three, so opendm40 always uses the DM40C values.
/// Set to `self.model` in factor()/over() to restore the stock behaviour.
const COUNTS_MODEL: u8 = 2;
const FACTOR: [f32; 3] = [4.0, 5.0, 6.0];
const OVER: [f32; 3] = [1.05, 1.04, 1.03];
const AD1_DIV: f32 = 8_388_606.0;
const K_ACV: [f32; 4] = [10.0, 100.0, 1000.0, 10000.0];
const MV_DIV: [f32; 2] = [10.0, 1.0]; // also the A op gain
const DEC4: [f32; 4] = [1.0, 10.0, 100.0, 1000.0];
const SHUNT: [f32; 3] = [0.01, 0.0005, 4.75e-5]; // ohm / kohm / Mohm
const OHM_RREF: [f32; 7] = [0.1, 1.0, 10.0, 100.0, 1000.0, 10000.0, 63000.0];
const OHM_DEC: [f32; 7] = [0.01, 0.1, 1.0, 10.0, 100.0, 1000.0, 10000.0];
const CAP_DEC: [f32; 8] = [0.001, 0.01, 0.1, 1.0, 10.0, 100.0, 1000.0, 10000.0];
const CAP_WIN: [f32; 8] = [0.0008, 0.0002, 0.0001, 0.0001, 0.0001, 3.0, 30.0, 200.0];
const CAP_K: [f32; 8] = [1020.0, 1003.0, 102.7, 10.68, 1.5, 4.06, 41.0, 400.0];
const CAP_R21: [u8; 3] = [0xDC, 0xCC, 0xBB];
const FREQ_DEC: [f32; 7] = [0.01, 0.1, 1.0, 10.0, 100.0, 1000.0, 10000.0];
const ACV_WIN: [f32; 4] = [0.002, 0.01, 0.1, 1.0];
const ACMV_WIN: [f32; 2] = [0.01, 0.1];
const DCV_NEGK: [f32; 4] = [1.0, 1.00008, 1.00011, 1.0015];
const ACA_WIN: [f32; 6] = [0.001, 0.01, 0.01, 0.1, 0.1, 1.0];
const TC_K_MV: [f32; 16] = [
    -6.942, -4.8, -1.051, 3.044, 7.087, 11.157, 15.346, 19.592, 23.854, 28.077, 32.223, 36.274, 40.224,
    44.067, 47.786, 51.358,
];
const CFG_F13_R7: [u8; hy::CFG_LEN] = [
    0x52, 0xbb, 0x14, 0x83, 0x43, 0x40, 0x00, 0x5d, 0x31, 0x00, 0x00, 0x00, 0x88, 0x08, 0x00, 0x8e, 0x80, 0xc2, 0xcc,
    0xa8, 0x00, 0x54,
];
const CFG_F13_DISCHARGE: [u8; hy::CFG_LEN] = [
    0x52, 0xdd, 0x13, 0x83, 0x43, 0x40, 0x00, 0x5d, 0x31, 0x00, 0x00, 0x00, 0x88, 0x88, 0x00, 0x8e, 0x80, 0xc2, 0xcc,
    0xa8, 0x00, 0x54,
];

// ------------------------------------------------------- +1 digit, mohm
//
// AD1 (hy3131_v2_6.h): signed 24 bit, full scale 1.25 V / 0.9 = 1.3889 V at
// 8388607 codes (0.166 uV/code); ohm is ratiometric, code = R / Rref * 0.9 *
// 2^23. Every DC front end maps its 60000-count full scale to ~0.6 V at AD1, so
// the codes per displayed count are nearly the same everywhere:
//   function  range                        codes/count  per +1 digit  +1 digit
//   DC V      r0..r2  6 / 60 / 600 V           60.4         6.0         yes
//   DC V      r3      1000 V (10000 counts)    60.4         6.0         no (HV divider self-heating)
//   DC mV     r1      600 mV                   60.4         6.0         yes
//   DC mV     r0      60 mV (OP1 x10)          60.4         6.0         no (0.1 uV step: amp noise, EMF)
//   DC A      r0, r1  6 A / 10 A (10 mohm)     60.4         6.0         no (0.1 uV across the shunt)
//   DC mA     r0, r1  60 / 600 mA (0.5 ohm)    30.2         3.0         no
//   DC uA     r0, r1  600 / 6000 uA (47.5 ohm) 28.7         2.9         no
//   ohm       r0      60 ohm (Rref 0.1k)       75.5 (1 mohm) 7.5        no (already mohm)
//   ohm       r1..r3  600 ohm .. 60 kohm       75.5         7.5         yes (r1: 600.000 ohm = mohm)
//   ohm       r4, r5  600 kohm, 6 Mohm         75.5         7.5         no (hum / leakage)
//   ohm       r6      60 Mohm (Rref 63 M)     119.8        12.0         no (hum / leakage)
//   continuity 75.5, diode 60.4, AC (AD2/RMS), cap, Hz, temp: no.
// A single conversion has only 6..7.5 codes per extra-digit step, so the
// extra digit comes from a moving average of the raw codes at the chopped
// R22 of MEDIUM / SLOW (0x16 / 0x17, the FR-05 table): the mean of N codes
// has 1/N code granularity (N = 8: 48 levels per step, N = 16: 120) and the
// noise drops by sqrt(N) (device ea73ea5, 6 V row: one 0x16 conversion 20 uV
// sd, 0x17 17 uV, still falling with averaging over 10..40 s). The window
// slides, so there is one reading per conversion (a window of 8 spans 1.6 s
// at 0x16, of 16 6.4 s at 0x17); it restarts on a step of more than ~2
// counts of the 60000-count format, on any OSR / range / mode change, and is
// not used in AC+DC.
// Ohm r0 is the stock 60 ohm row (f10 row 0: OPS1 = 1, the OP1 x10 gain also
// used by DC mV / A r0; the stock Rref constant 0.1 kohm = the 1 kohm r1
// reference / 10) that the stock never selects (its autorange clamps to r1..r6,
// dmm_v2_6.h section 2). Here it is in the autorange too (owner 2026-10-25,
// `ohm_auto_floor`: not with the "ON" variant) and a manual range, always with
// the default ohm variant (SFUVR = 2, the 0.90 mA one; the "ON" variant gives
// 0.28 mA on r1, OHM_ISC), with PA2 open (OHM_R0_ROW). Its gain is `ohm_row_gain`; the
// single offset 0x144 is the r1..r6 one: use REL.

// ------------------------------------------------- FR-05 conversion rate
//
// `Rate` sets how much converter time goes into one reading of the AD1 (DC)
// functions; the display smoothing on top of the readings is meter.rs
// (`Smooth`). Conversion periods: OSR * 30 / SYSCLK, x2 with the chopper
// (hy3131::ad1_period_us; device ea73ea5: 100.0 / 200.8 / 400.8 ms for
// 0x06 / 0x16 / 0x17, firmware/docs/device-check-2026-10-21.md "AD1 OSR
// characterization"). Each code's sinc^2 window is a whole number of 50 and
// 60 Hz periods (100 ms, 200 ms at 0x17), so every rate nulls both mains
// frequencies; the stock's moving-input OSR 8192 (0x15, a 50 ms window) did
// not null 50 Hz and is gone from the DC rows (dmm-rate-filter-survey.md
// sections 0, 8). DC V / mV / A run at the rate's R22 from the row on (`write_config`:
// written in the row, no R22 write after it, so R22_DROP does not apply).
//   rate    DC V / mV / A AD1 R22          ohm in range   code window (step reset ~2 counts)    readings
//   FAST    0x06 OSR 16384 unchopped,       0x16           none                                  10/s DC (2 dropped per offset
//           offset in firmware (fastvos.rs)                                                       refresh, every 30 s); ohm 5/s
//   MEDIUM  0x16 OSR 16384 chopped          0x17           V r0..r2 / mV r1: 8 (1.6 s),          5/s DC, ohm 2.5/s
//   (dflt)                                                 ohm r0..r3: 16 (6.4 s), others none
//   SLOW    0x17 OSR 32768 chopped          0x17           16 every range (6.4 s; ohm r0..r3:    2.5/s
//                                                          32, 12.8 s)
// Ohm keeps its own OSR steps (`poll_ohm`): 0x13 on OL, OSR_EARLY within
// R22_DROP_WIN_MS of the row write. Ohm MEDIUM / SLOW keep the stock's
// in-range 0x17 (emulator resautotest r30-300-30: at 0x16 the OL jump to
// 60 Mohm stepped down in 4 rows, not 3); FAST is 0x16 without the window
// (an unchopped ohm would need an offset per range of the autorange: not
// done). Before 2026-10-25 FAST was 0x15 and MEDIUM / SLOW ran the stock's
// 0x15 while moving / 0x17 once stable (10/s -> 2.5/s), which left 50 Hz
// unrejected exactly on inputs with AC content.
// The +1 digit (`Val::ext`, ext-capable ranges of the table above) is shown
// only while the window holds at least AVG_FULL conversions at the rate's
// R22: never in FAST (one unchopped conversion has ~1.6x the noise of one at
// 0x16, and its low-frequency noise does not average down), in MEDIUM /
// SLOW from the 8th conversion after a step / range change (1.6 s at MED,
// 3.2 s at SLOW; before opendm40 2026-10-13 the digit was drawn from the
// first conversion on).
// Applies to DC V, DC mV, DC A / mA / uA and ohm, not in AC+DC (the DC phase
// stays at the row's OSR 8192, ACDC_STOCK_OSR; its own phase logic). Not applicable (stock timing in every rate):
// AC (AD2 / RMS converter, its filter is the RMS EMA), capacitance and
// frequency (counter gates of 420 / 350 ms), temperature (thermocouple: three
// phases, ~2.2 s; resistive sensors: fixed OSR 32768 + EMA), continuity
// (instant beep, fast OSR) and diode (stock EMA).

/// Conversion rate of the AD1 functions (see the table above).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Rate {
    Slow = 0,
    Medium = 1,
    Fast = 2,
}

/// SCPI / settings names, indexed by `Rate as usize`.
pub const RATE_NAMES: [&str; 3] = ["SLOW", "MED", "FAST"];

impl Rate {
    pub const ALL: [Rate; 3] = [Rate::Slow, Rate::Medium, Rate::Fast];

    pub fn from_u8(v: u8) -> Option<Rate> {
        Rate::ALL.get(v as usize).copied()
    }
}

/// R22 of SLOW (OSR 32768 chopped, 400 ms) and of MEDIUM (OSR 16384
/// chopped, 200 ms) in DC V / mV / A (FAST: fastvos::R22_FAST); ohm's
/// in-range OSR at MEDIUM and SLOW / at FAST (`steady_osr`).
const OSR_SLOW: u8 = 0x17;
const R22_MED: u8 = 0x16;
/// DMM rail (PE2) up -> the row is written this much later (`set_suspended`).
/// The stock waits 10 ms after raising PE2 before it drops PC4 and sets the
/// DMM up (FUN_0800b384); a wake that left PC4 on wrote the row at once,
/// into a chip still powering up (device check 2026-10-21, Bluetooth 1g).
pub const RAIL_SETTLE_MS: u64 = 50;
/// Stall watchdog (`Dmm::watchdog`): row read-back period with no reading,
/// and the shortest silence that restarts the row anyway.
const WD_CHECK_MS: u64 = 2_000;
const WD_IDLE_MS: u64 = 10_000;
/// Row index of R2A (the first of the 8 read-back bytes R2A..R31).
const CHK_AT: usize = 10;
/// Ohm's in-range OSR within R22_DROP_WIN_MS of the row write (R22_LATE:
/// OSR 8192 chopped, 100 ms), the row's own R22.
const OSR_EARLY: u8 = 0x15;

/// UF-21: text for the UI when the mV-first AUTO is selected for V
/// (`Dmm::mv_auto_warn`).
pub const MV_AUTO_WARN: &str = "AUTO starts on mV: AUTO+ is safer";

/// Code window of an AD1 range (FR-05 table): (window, restart threshold in
/// codes, extra digit possible); None = no averaging (avg_n 0). Thresholds
/// ~2 counts of the 60000-count format: V / mV / A 60.4 codes per count,
/// mA 30.2, uA 28.7, ohm 75.5, ohm r6 119.8 (see the +1 digit table).
fn avg_cfg(rate: Rate, acdc: bool, mode: u8, range: i8) -> Option<(usize, i32, bool)> {
    if acdc || rate == Rate::Fast {
        return None;
    }
    let (medium, thr, ext) = match (mode, range) {
        (1, 0..=2) | (3, 1) => (8, 120, true),
        (1 | 3 | 7, _) => (1, 120, false),
        (8, _) => (1, 60, false),
        (9, _) => (1, 57, false),
        (10, 0) => (16, 150, false),
        (10, 1..=3) => (16, 150, true),
        (10, 6) => (1, 240, false),
        (10, OHM600) => return None,
        (10, _) => (1, 150, false),
        _ => return None,
    };
    let n = match rate {
        Rate::Slow if mode == 10 && range <= 3 => 32,
        Rate::Slow => 16,
        _ => medium,
    };
    (n > 1).then_some((n, thr, ext))
}

/// What `rate` gives in function `mode` / `range`: (code window in
/// conversions, 1 = none; the +1 digit is shown). See the FR-05 and +1
/// digit tables: FAST never averages; MEDIUM / SLOW average 8..32 steady
/// conversions (`rate_period` each) where the table says so.
pub fn rate_window(rate: Rate, acdc: bool, mode: u8, range: i8) -> (usize, bool) {
    avg_cfg(rate, acdc, mode, range).map_or((1, false), |(n, _, ext)| (n, ext))
}

/// AD1 conversion period of `rate` in function `mode`, ms: one reading per
/// conversion (FR-05 table: DC V / mV / A FAST 100, MED 200, SLOW 400; ohm
/// FAST 200, MED and SLOW 400).
pub fn rate_period(rate: Rate, mode: u8) -> u32 {
    let dc = matches!(mode, 1 | 3 | 7..=9);
    match rate {
        Rate::Fast if dc => 100,
        Rate::Fast => 200,
        Rate::Medium if dc => 200,
        _ => 400,
    }
}

/// The extra digit is real: an ext-capable range with at least AVG_FULL
/// slow conversions in the window (FAST never averages, see `avg_cfg`).
fn ext_ok(ext: bool, n: u8) -> bool {
    ext && n >= AVG_FULL
}

/// Moving average of raw AD1 codes (|AD1| in ohm).
struct Avg {
    buf: [i32; AVG_MAX],
    n: usize,
    i: usize,
    sum: i32,
    len: usize,
}

const AVG_MAX: usize = 32;
/// Readings with at least this many slow conversions averaged have the full
/// +1 digit resolution (`Val::ext`). SCPI MEAS? after a function change
/// prefers such a reading but waits for it only SCPI_SETTLE_MS (main.rs),
/// which a full window never meets (~5.3 s after the switch, R22_LATE).
pub const AVG_FULL: u8 = 8;

impl Avg {
    const fn new() -> Self {
        Avg { buf: [0; AVG_MAX], n: 0, i: 0, sum: 0, len: 0 }
    }

    fn reset(&mut self) {
        self.n = 0;
        self.i = 0;
        self.sum = 0;
    }

    /// Add `x` to a window of `len` codes, restarting when `x` is more than
    /// `thr` codes from the mean or the window length changed. Returns the
    /// mean (sum <= 32 * 2^23 fits i32; the fraction is added separately to
    /// keep it exact in f32).
    #[inline(never)]
    fn push(&mut self, x: i32, len: usize, thr: i32) -> f32 {
        let len = len.clamp(1, AVG_MAX);
        if len != self.len || (self.n > 0 && (x - self.sum / self.n as i32).abs() > thr) {
            self.reset();
            self.len = len;
        }
        if self.n == len {
            self.sum -= self.buf[self.i];
        } else {
            self.n += 1;
        }
        self.buf[self.i] = x;
        self.sum += x;
        self.i = (self.i + 1) % len;
        let n = self.n as i32;
        (self.sum / n) as f32 + (self.sum % n) as f32 / n as f32
    }
}

// ------------------------------------------------------------ temperature
//
// Sensors of the temperature function (mode 15). 0 = the stock type-K
// thermocouple path (cfg f15, three phases, cold junction = the HY3131 internal
// sensor). 1..6 are 2-wire resistive sensors read through the ohm path (cfg
// f10_p8clr, the stock default ohm variant m+8 == 0, ratiometric:
// R = |AD1| * Rref[r] / 0.9 / 2^23, gain cal 0x124 + 4r, offset cal 0x144):
//   PT100  fixed r1 (600 ohm, Rref 1 k), PT1000 fixed r2 (6 k, Rref 10 k):
//     Callendar-Van Dusen (IEC 60751), -200..850 C.
//   NTC (Beta model, 1/T = 1/T25 + ln(R/R25)/B), -55..300 C: r2..r6 with
//     hysteresis, starting at the lowest range that holds R25.
// Excitation: the measured short-circuit current of the range (OHM_ISC,
// default variant) scaled as if the drop across Rref stayed at its
// short-circuit value: I = Isc * Rref / (Rref + R), P = I^2 R; see
// `t_excitation`. *Inferred*: the source is not that simple (open-circuit
// 2.17 V, short-circuit drop 0.90 V on r1, where OP3 limits the current).
// Lead resistance is not modelled separately: a 2-wire lead pair adds
// 0.39 C per 0.1 ohm on a PT100 (0.039 C on a PT1000, negligible for NTCs);
// trim it with the per-sensor offset (or REL).

/// Short sensor names (SCPI CONF:TEMP <name>, SENS:TEMP:TRAN?).
pub const T_NAMES: [&str; 7] = ["TCK", "PT100", "PT1000", "NTC10K3950", "NTC10K3435", "NTC100K3950", "NTCUSER"];
// Test sources shown on the DMM page (`test_current`).
/// Ohm r0 (60 ohm) row: the stock f10 row 0 with PA2's DS2 / FS2 open (R2B
/// 0x46 -> 0x40), i.e. the r1 row with OPS1 = 1 (OP1 x10). Device 2026-10-26
/// (MR9270 on V-COM, 10 / 20 / 50 ohm, manual 60 / 600 ohm, alternating): with
/// PA2 closed r0 read 0.971 x r1 (Rref 0.1 k, r1's gain), with it open 1.009.
/// What PA2 is wired to on the board is not known.
const OHM_R0_ROW: [u8; hy::CFG_LEN] = {
    let mut c = CFG_F10_P8CLR[0];
    c[0x0B] &= !0x06;
    c
};

// Ohm r7: the experimental 600 Mohm range (firmware/docs/ohm-600m.md). Manual
// only: the autorange, the AUTO key's short-press range steps and a restored
// setting never reach it (`set_range` item 7, SCPI RES:RANG above 60 Mohm).
// The 6 Mohm row (Rref 10 Mohm) with OP3 on AGNDP9 (~1.05 V, the level the
// V/ohm node leaks to: the leak is then a constant share of the current) and
// AD1 absolute against PB6 - AGND, 200 ms. Two phases on AD1 = OP1O - PB3:
// OP1 on PB0 (the jack, Vx) and OP1 on FB (OP3's side of Rref, Vfb); the
// same OP1 and AD1 path, so their offsets cancel in Vfb - Vx (device
// 2026-10-28: FB via OP1 vs FB on AD1 directly 5.7 uV apart).
// R = Rref * Vx / (Vfb - Vx); a ratio of two conversions on one reference.
pub const OHM600: i8 = 7;
/// Whether HY3131 `mode` / `range` is the experimental 600 Mohm.
pub fn is_ohm600(mode: u8, range: i8) -> bool {
    mode == 10 && range == OHM600
}
/// The ohm autorange's top: 60 Mohm (r7 is manual only).
const OHM_AUTO_TOP: i8 = 6;
/// Open leads (`Ohm600::learn`): the drop across Rref below this (an Rx
/// above ~7 Gohm) with the jack within 10 % of FB.
const OHM600_OPEN_DROP: f32 = 1.5e-3;
/// From a row write to its first reading (2 x (skip + keep) results of
/// 200 ms and the settle), for a host's READ? / MEAS? wait (`deadline`).
const OHM600_FIRST_MS: u64 = 2_500;
const OHM600_ROW: [u8; hy::CFG_LEN] = {
    let mut c = CFG_F10_P8CLR[5];
    c[0x02] = 0x16; // R22: OSR 16384, 200 ms (10 mains periods)
    c[0x08] = 0x31; // R28: AD1 reference PB6 - AGND
    c[0x11] = (c[0x11] & 0xF0) | 3; // R31 SFUVR: AGNDP9
    c
};
/// R32 of the two phases: OP1+ on PB0 (the row's own), on FB.
const OHM600_R32: [u8; 2] = [0x3C, 0x39];
/// Rref of r5 / r7, kohm (nominal; `Ohm600::gain` trims it).
const OHM600_RREF: f32 = 10_000.0;
/// Full scale, kohm.
const OHM600_FS: f32 = 600_000.0;
/// AD1 volts per code (IG 0.9 of the r5 row, REFO 1.25 V).
const OHM600_V: f32 = 1.25 / 0.9 / AD1_DIV;
/// Readings averaged (a sliding mean; it restarts on a step over
/// OHM600_STEP or an OL).
const OHM600_AVG: usize = 4;
const OHM600_STEP: f32 = 0.02;
/// One range over three decades: the digits of the 6000-count range of the
/// value's decade (5.570 / 55.70 / 557.0 Mohm; a decade change needs 3 %
/// past the 6 / 60 Mohm boundary), plus the small extra digit where the
/// mean's own spread allows it (`Ohm600::ext`).
const OHM600_DEC_HYST: f32 = 0.03;
/// Short-circuit current (A) of the r7 source: AGNDP9 through 10 Mohm, jacks
/// shorted (docs/data/short.csv AGNDP9,R10M).
pub const OHM600_ISC: f32 = 1.047e-7;

/// The 600 Mohm phase cycle and its RAM-only tuning (SCPI DBG:OHM6).
#[derive(Clone, Copy)]
pub struct Ohm600 {
    /// Gain on the nominal Rref.
    pub gain: f32,
    /// Subtracted from Vfb - Vx, V.
    pub off: f32,
    /// The V/ohm node's leak as a conductance across the jacks, S, learned
    /// on open leads (`learn`; RAM only, 0 until then). Device 2026-10-28:
    /// 48.5 pA at 1.047 V, ~21.6 Gohm.
    pub leak: f32,
    leak_n: u8,
    /// Open pairs in a row (`learn` skips the first: the node still
    /// settling after the leads came off).
    open_run: u8,
    /// Learning on (DBG:OHM6 with a leak value freezes it, a negative one
    /// turns it back on).
    learn_on: bool,
    /// The decimals were chosen since the row write (`decimals`).
    dec_set: bool,
    /// The extra digit shown (`ext`, with hysteresis).
    ext_on: bool,
    /// Conversions dropped after a phase switch / kept (their median).
    pub skip: u8,
    pub keep: u8,
    ph: u8,
    n: u8,
    buf: [i32; 8],
    a: i32,
    /// The last phase medians (codes), for DBG:OHM6?.
    pub last: (i32, i32),
    hist: [f32; OHM600_AVG],
    hn: u8,
    /// Decimals shown (Mohm), `decimals`.
    dec: u8,
}

impl Ohm600 {
    pub const NEW: Ohm600 = Ohm600 { gain: 1.0, off: 0.0, skip: 2, keep: 3, ph: 0, n: 0, buf: [0; 8], a: 0, last: (0, 0), hist: [0.0; OHM600_AVG], hn: 0, dec: 1, leak: 0.0, leak_n: 0, open_run: 0, learn_on: true, dec_set: false, ext_on: false };

    /// A row write: phase A (the row's R32), no samples, no average.
    fn restart(&mut self) {
        self.ph = 0;
        self.n = 0;
        self.hn = 0;
        self.open_run = 0;
        self.dec_set = false;
        self.ext_on = false;
    }

    /// One AD1 result. Some(R32 to write) when the phase ends; `done` then
    /// holds (Vx, Vfb) codes when both phases are in.
    fn step(&mut self, code: i32, done: &mut Option<(i32, i32)>) -> Option<u8> {
        let keep = self.keep.clamp(1, 8);
        if self.n < self.skip {
            self.n += 1;
            return None;
        }
        self.buf[(self.n - self.skip) as usize] = code;
        self.n += 1;
        if self.n < self.skip + keep {
            return None;
        }
        let m = median_i32(&mut self.buf[..keep as usize]);
        self.n = 0;
        if self.ph == 0 {
            self.a = m;
        } else {
            self.last = (self.a, m);
            *done = Some(self.last);
        }
        self.ph ^= 1;
        Some(OHM600_R32[self.ph as usize])
    }

    /// kohm from the phase codes, None = OL (no drop across Rref, or above
    /// the range).
    fn kohm(&self, a: i32, b: i32, ov: f32) -> Option<f32> {
        let vx = a as f32 * OHM600_V;
        // The current through Rref less the leak's share at this Vx.
        let drop = (b - a) as f32 * OHM600_V - self.off - self.leak * vx * OHM600_RREF * 1e3 * self.gain;
        let k = OHM600_RREF * self.gain * vx / drop;
        (drop > 0.0 && k >= 0.0 && k <= ov * OHM600_FS).then_some(k)
    }

    /// Open leads (the drop under OHM600_OPEN_DROP, the jack near FB): all
    /// the current through Rref is the node's leak; its conductance (as a
    /// resistor across the jacks, the simplest model; whether it is one is
    /// *unknown*) into a mean over the last ~4.
    fn learn(&mut self, a: i32, b: i32) {
        let (vx, vf) = (a as f32 * OHM600_V, b as f32 * OHM600_V);
        let drop = vf - vx - self.off;
        // Not open: a drop too large, or none / negative (the jack driven
        // at or above FB: a charged capacitor, a source).
        if !self.learn_on || drop >= OHM600_OPEN_DROP || drop <= 0.0 || vx < 0.9 * vf {
            self.open_run = 0;
            return;
        }
        self.open_run = self.open_run.saturating_add(1);
        if self.open_run < 2 {
            return;
        }
        let g = drop / (OHM600_RREF * 1e3 * self.gain) / vx;
        self.leak = if self.leak_n == 0 { g } else { self.leak + (g - self.leak) * 0.25 };
        self.leak_n = self.leak_n.saturating_add(1);
    }

    /// The sliding mean with `k` added (restarted on a step).
    fn average(&mut self, k: f32) -> f32 {
        let n = self.hn as usize;
        if n > 0 {
            let mean = self.hist[..n].iter().sum::<f32>() / n as f32;
            if (k - mean).abs() > OHM600_STEP * mean.abs().max(1.0) {
                self.hn = 0;
            }
        }
        let n = self.hn as usize;
        if n == OHM600_AVG {
            self.hist.copy_within(1.., 0);
            self.hist[n - 1] = k;
        } else {
            self.hist[n] = k;
            self.hn += 1;
        }
        let n = self.hn as usize;
        self.hist[..n].iter().sum::<f32>() / n as f32
    }
}

impl Ohm600 {
    /// Decimals for `mohm` (the decade's 6000-count format, with
    /// hysteresis around 6 / 60 Mohm).
    fn decimals(&mut self, mohm: f32) -> u8 {
        let want = if mohm < 6.0 { 3 } else if mohm < 60.0 { 2 } else { 1 };
        if !self.dec_set {
            // The first reading after the row write takes its decade.
            self.dec = want;
            self.dec_set = true;
        } else if want != self.dec {
            // The boundary between the shown and the wanted decade.
            let b = if want.max(self.dec) == 3 { 6.0 } else { 60.0 };
            if (mohm - b).abs() > OHM600_DEC_HYST * b {
                self.dec = want;
            }
        }
        self.dec
    }

    /// The extra digit: a full window whose mean's spread (sd / sqrt n of
    /// the readings in it, kohm) is about one such digit or less (as the
    /// other ranges' extra digits: DC V 6 V shows its 10 uV at a ~7 uV mean
    /// sd); on at 0.7 digit, off above 1.3 (no flicker at the edge).
    fn ext(&mut self, dec: u8) -> bool {
        let n = self.hn as usize;
        if n < OHM600_AVG {
            self.ext_on = false;
            return false;
        }
        let h = &self.hist[..n];
        let m = h.iter().sum::<f32>() / n as f32;
        let var = h.iter().map(|x| (x - m) * (x - m)).sum::<f32>() / (n - 1) as f32;
        // kohm of one extra digit: 1000 / 10^(dec + 1).
        let digit = 1000.0 / [10.0, 100.0, 1000.0, 10000.0][(dec as usize).min(3)];
        let k = if self.ext_on { 1.3 } else { 0.7 };
        self.ext_on = var / n as f32 <= k * k * digit * digit;
        self.ext_on
    }
}

fn median_i32(v: &mut [i32]) -> i32 {
    v.sort_unstable();
    v[v.len() / 2]
}

/// Calibration gain of ohm row `r` from its own slot `cal_r` (0x124 + 4r).
/// r0: the slot is not a calibration (the stock never selects r0,
/// dmm_v2_6.h section 2; 1.005 on the dumped unit, a round value like the
/// other unused slots, inferred), so r1's gain `cal_r1` (the shared 1 kohm
/// reference, PA4 DF) x the record's DC mV x10 / x1 gain ratio `mv10 / mv1`
/// (0x24 / 0x28, the same OP1 x10 stage; 0.9895 on the dumped unit, the x10
/// path reads ~1 % high). That leaves r0 / r1 = 0.9991 at 50 ohm (device
/// 2026-10-26).
fn ohm_row_gain(r: usize, cal_r: f32, cal_r1: f32, mv10: f32, mv1: f32) -> f32 {
    if r == 0 { cal_r1 * mv10 / mv1 } else { cal_r }
}

/// Ohm short-circuit test current (A) per range, [default variant, "ON"
/// variant (m+8, p8set rows)]: the drop across Rref / Rref with V/ohm - COM
/// shorted, measured on one unit with DBG:SRC (tools/srcmap.py; data:
/// docs/data/short.csv, the stock row's own tap: VDS15 default, AGNDP7 ON,
/// AGNDP8 ON on r5). r0 (60 ohm, always the default variant,
/// `Dmm::write_config`) = r1's default value, inferred: its own row has the
/// same tap and Rref switches but was not measured. r6 (60 M, the
/// current-source row): not measured, None.
pub const OHM_ISC: [[Option<f32>; 2]; 7] = [
    [Some(8.983e-4), Some(8.983e-4)],
    [Some(8.983e-4), Some(2.795e-4)],
    [Some(1.969e-4), Some(5.842e-5)],
    [Some(2.026e-5), Some(6.559e-6)],
    [Some(2.172e-6), Some(6.640e-7)],
    [Some(2.153e-7), Some(8.557e-8)],
    [None, None],
];
/// Diode test current (A): OP3 as a current source (SMODE 0110, SCP / SCN)
/// through RLD, not an Rref; 0.756 mA measured on one unit (VDS15 tap, the
/// MR9270S+ mA input into the jacks; device-check "RLD current-source path").
pub const DIODE_I: f32 = 0.756e-3;
/// Continuity test current (A), signed against diode's: -0.260 mA measured
/// on one unit (MR9271S mA input into V/ohm - COM, 2026-10-30; diode read
/// +0.753 mA, ohm 600 +0.868 mA the same way): its source drives the other
/// way.
pub const CONT_I: f32 = -0.260e-3;

/// Ohm test current (A) of range `r` and variant `var` (Dmm::ohm_var):
/// OHM_ISC, the most the range drives through the probes (with R between
/// them it is less). None where it is not measured (r6).
pub fn ohm_test_current(r: i8, var: u8) -> Option<f32> {
    if r == OHM600 {
        return Some(OHM600_ISC);
    }
    OHM_ISC[r.clamp(0, 6) as usize][(var & 1) as usize]
}

/// Test current (A) the DMM page shows for `mode` (10 ohm: per range and
/// variant, `ohm_test_current`; 11 continuity: CONT_I; 12 diode: DIODE_I),
/// None elsewhere.
pub fn test_current(mode: u8, r: i8, var: u8) -> Option<f32> {
    match mode {
        10 => ohm_test_current(r, var),
        11 => Some(CONT_I),
        12 => Some(DIODE_I),
        _ => None,
    }
}

/// Natural log (no libm): exponent * ln2 + atanh series of the mantissa.
fn ln(x: f32) -> f32 {
    let b = x.to_bits();
    let mut e = ((b >> 23) & 0xFF) as i32 - 127;
    let mut m = f32::from_bits((b & 0x007F_FFFF) | 0x3F80_0000);
    if m > 1.414_213_6 {
        m *= 0.5;
        e += 1;
    }
    let s = (m - 1.0) / (m + 1.0);
    let s2 = s * s;
    e as f32 * core::f32::consts::LN_2 + 2.0 * s * (1.0 + s2 * (1.0 / 3.0 + s2 * (0.2 + s2 * (1.0 / 7.0))))
}

/// Platinum RTD, IEC 60751 Callendar-Van Dusen: R/R0 -> C (None outside
/// -200..850 C). T >= 0 closed form (written without the cancellation of the
/// textbook quadratic), T < 0 Newton on the full equation.
fn pt_temp(x: f32) -> Option<f32> {
    const A: f32 = 3.9083e-3;
    const B: f32 = -5.775e-7;
    const C: f32 = -4.183e-12;
    if !(0.18..=3.95).contains(&x) {
        return None;
    }
    let mut t = 2.0 * (x - 1.0) / (A + sqrt(A * A + 4.0 * B * (x - 1.0)));
    if x < 1.0 {
        for _ in 0..4 {
            let f = 1.0 + t * (A + t * B) + C * (t - 100.0) * t * t * t - x;
            let d = A + 2.0 * B * t + C * t * t * (4.0 * t - 300.0);
            t -= f / d;
        }
    }
    if (-200.0..=850.0).contains(&t) { Some(t) } else { None }
}

/// NTC, Beta model: R (ohm) -> C (None outside -55..300 C).
fn ntc_temp(r: f32, r25: f32, beta: f32) -> Option<f32> {
    if r <= 0.0 {
        return None;
    }
    let t = 1.0 / (1.0 / 298.15 + ln(r / r25) / beta) - 273.15;
    if (-55.0..=300.0).contains(&t) { Some(t) } else { None }
}

/// The bottom buttons: the stock six and the power meter (owner
/// 2026-10-22: a full function, the 7th button).
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Button {
    Volt,
    Amp,
    Ohm,
    Cap,
    ContDiode,
    HzTemp,
    Power,
}

impl Button {
    pub const ALL: [Button; 7] = [Button::Volt, Button::Amp, Button::Ohm, Button::Cap, Button::ContDiode, Button::HzTemp, Button::Power];
}

/// A formatted readout: value in display units, digits, unit text, OL.
#[derive(Clone, Copy)]
pub struct Val {
    pub v: f32,
    /// Decimals of the 60000-count format (history, chart, BT, aux use this).
    pub dec: u8,
    pub unit: &'static str,
    pub ol: bool,
    /// `v` resolves one more digit (averaged reading, see `avg_cfg`): the
    /// main readout shows `dec + 1` decimals, the last one in a small font.
    pub ext: bool,
}

impl Val {
    const fn new(v: f32, dec: u8, unit: &'static str) -> Self {
        Val { v, dec, unit, ol: false, ext: false }
    }
    const fn ol(dec: u8, unit: &'static str) -> Self {
        Val { v: 0.0, dec, unit, ol: true, ext: false }
    }
    /// The readout with the extra digit (not for OL).
    const fn with_ext(self, ext: bool) -> Self {
        Val { ext: ext && !self.ol, ..self }
    }
}

#[derive(Clone, Copy)]
pub struct Reading {
    pub mode: u8,
    pub range: i8,
    pub main: Val,
    pub aux1: Option<Val>,
    pub aux2: Option<Val>,
    /// Main value in base SI units (V, A, ohm, F, Hz, C) for SCPI / BT.
    pub si: f32,
    pub auto: bool,
    /// Range label in auto: "AUTO+" instead of "AUTO".
    pub auto_plus: bool,
    /// AC+DC sub-mode: main = sqrt(AC^2 + DC^2), aux1 = AC part, aux2 = DC
    /// part; `mode` is then always the DC mode (1, 3, 7..9).
    pub acdc: bool,
    /// AUTO coupling (sub-mode 3, `Dmm::cpl_auto`): main = the part AUTO
    /// picked (the DC part, signed, or the AC part's RMS), `mode` = the DC
    /// or the AC mode of that part (so `meter::func_of`, the limits slot,
    /// smoothing and the glyph above the unit follow it), aux1 / aux2 = the
    /// AC / DC parts as in AC+DC; `acdc` is false.
    pub cpl_auto: bool,
    pub seq: u32,
    /// Conversions in the moving average behind `main` / `si` (0 = not an
    /// averaged range); `AVG_FULL` or more = full +1-digit resolution.
    pub avg_n: u8,
    /// Wrong-connection alert in force (sentinel.rs).
    pub alert: Alert,
    /// Power meter (`pw`): all its values; main = P (fallback: S), aux1 =
    /// V, aux2 = I, `mode` = the AC A mode of the current range.
    pub power: Option<pw::PowerVals>,
    /// Provisional (../docs/history/switch-latency.md proposals 1 / 6): a value a gate
    /// still holds (the first DC conversions after a range / function
    /// change, AC+DC before both parts, the AC part settling, the AUTO
    /// coupling's first decision), or the new function's placeholder (`main`
    /// NaN: dashes). Drawn dimmed on the DMM page only; never fed to the
    /// meter (statistics, REL, limits, trend, CSV, beeps), autorange, the
    /// sentinel, BT or SCPI (`Dmm::poll` returns it without clearing
    /// `reading_pending` or counting `seq`).
    pub prov: bool,
}

impl Reading {
    /// The burden voltage readout (`Dmm::burden_val`) in this reading: its
    /// aux slot (0 / 1) and value. Recognised by its unit: no other aux
    /// value of a plain A function is in V (aux1 Hz, aux2 % duty / loop).
    pub fn burden(&self) -> Option<(usize, Val)> {
        if !matches!(self.mode, 4..=9) || self.parts() || self.power.is_some() {
            return None;
        }
        for (i, v) in [self.aux1, self.aux2].into_iter().enumerate() {
            if let Some(v) = v.filter(|v| v.unit.as_bytes().last() == Some(&b'V')) {
                return Some((i, v));
            }
        }
        None
    }

    /// aux1 / aux2 are the AC / DC parts (AC+DC or AUTO coupling).
    pub fn parts(&self) -> bool {
        self.acdc || self.cpl_auto
    }
    pub fn is_ac(&self) -> bool {
        matches!(self.mode, 0 | 2 | 4 | 5 | 6)
    }
}

pub fn button_of(mode: u8) -> Button {
    match mode {
        0..=3 => Button::Volt,
        4..=9 => Button::Amp,
        10 => Button::Ohm,
        11 | 12 => Button::ContDiode,
        13 => Button::Cap,
        _ => Button::HzTemp,
    }
}

fn cal_ok() -> bool {
    rec_u32(4) == 1
}

/// Calibration float at struct offset `off`, `dflt` if missing or not sane.
fn cal(off: u32, dflt: f32) -> f32 {
    let v = f32::from_bits(rec_u32(CAL - SETTINGS + off));
    if cal_ok() && v.is_finite() && v.abs() < 1.0e6 { v } else { dflt }
}

/// FUN_080160c6: EMA that jumps (0.99) when the step exceeds `thr`.
fn ema(thr: f32, a: f32, x: f32, y: &mut f32) {
    let d = x - *y;
    let k = if d.abs() < thr { a } else { 0.99 };
    *y += k * d;
}

fn sqrt(x: f32) -> f32 {
    if x <= 0.0 {
        return 0.0;
    }
    let mut y = f32::from_bits((x.to_bits() >> 1) + 0x1FBD_1DF5);
    for _ in 0..3 {
        y = 0.5 * (y + x / y);
    }
    y
}

fn hypot(a: f32, b: f32) -> f32 {
    sqrt(a * a + b * b)
}

/// AC+DC / AUTO: a step down judged on hypot(AC, DC) needs the `other`
/// part (bit 0 AC, bit 1 DC of `Dmm::acdc_known`) measured since the last
/// reset (entry, every unit change): until then it reads 0 and the
/// magnitude is one part alone. TODO "AC+DC / AUTO down-range on one part":
/// a pure AC 1 A on the A shunt read hypot(0, ~0) in the DC phase and could
/// walk down onto the mA / uA shunt (47.5 ohm) with 1 A flowing; 230 V AC
/// into the 6 V / mV range the same way. Up-ranges never wait.
fn acdc_down_ok(known: u8, other: u8) -> bool {
    known & other != 0
}

/// AC+DC part readout: "0L" when |part / full scale| > 1.04 (dmm_v2_6.h:231).
/// The DC mode of a V / A mode (AC+DC reports its readings with it).
fn dc_mode_of(m: u8) -> u8 {
    match m {
        0 | 2 => m + 1,
        4..=6 => m + 3,
        _ => m,
    }
}

/// The AC mode of a DC V / A mode (1 -> 0, 3 -> 2, 7..9 -> 4..6).
fn ac_mode_of(dm: u8) -> u8 {
    if dm <= 3 { dm - 1 } else { dm - 3 }
}

fn part(x: f32, lim: f32, dec: u8, unit: &'static str) -> Val {
    // The value is kept with the OL flag: BT still sends it (bt_meas_v2_6.h:170).
    Val { v: x, dec, unit, ol: x.abs() / lim > 1.04, ext: false }
}

fn u24(b: &[u8]) -> u32 {
    b[0] as u32 | (b[1] as u32) << 8 | (b[2] as u32) << 16
}

/// `Dmm::ac_over_until`: how long after the first AC A result of a row an
/// over-range result is taken for the switch step (~5 RMS blocks of
/// 107 ms). The OVERLOAD rule protects against heat (seconds), so this is
/// no exposure; AUTO's up-range after a switch is this much later too.
const AC_OVER_HOLD_MS: u64 = 500;

/// Plain AC mV: the node's peak estimate |LPF| + sqrt(2) x RMS above this
/// many times the range steps up (as an over-range result), the input
/// clipped. The row is DC-coupled (the LPF is the input's DC) and AD2 spans
/// +-1.25 V there (2.08 x 600 mV; x10 on 60 mV). Device 2f56ff9: the 40 Hz
/// 0 / 9.5 V square on V-COM read 0.606 V on 600 mV for 12 s (AC V AUTO,
/// mV-first), under the 1.03 x 600 mV up threshold: the mV input clamps it
/// to a ~0 .. 1.21 V swing, so its RMS cannot reach the threshold; LPF ~0.6
/// V + 1.414 x 0.606 V = ~1.46 V (2.4 x 600 mV; the LPF value is INFERRED
/// from the DC-coupled row, not read on the device). A sine at 600 mV RMS
/// with no DC is 1.41 x the range.
const MV_CLIP_K: f32 = 2.2;
/// After a clip step (MV_CLIP_K), AC V 6 V takes its reference AC part
/// this long after its row write (the AC-coupled row settles first).
const MV_CLIP_SETTLE_MS: u64 = 1500;

/// `Dmm::precond_ms` default (device d2aff22: 100 ms was enough).
const PRECOND_MS: u16 = 100;

fn relay_pa(clk: &mut Clock, arg: u8) {
    // Latching relay pair: 10 ms pulse. 0: PA10=1 PA9=0, 1: PA10=0 PA9=1.
    let (set, clr) = if arg == 0 { (10, 9) } else { (9, 10) };
    unsafe {
        w32(GPIOA + 0x18, (1 << set) | (1 << (clr + 16)));
        modify32(GPIOA + 0x00, (3 << 18) | (3 << 20), (1 << 18) | (1 << 20));
    }
    clk.delay_ms(10);
    unsafe { w32(GPIOA + 0x18, (1 << (9 + 16)) | (1 << (10 + 16))) };
}

fn relay_pe(arg: u8) {
    // Shunt select: 0: PE0=1 PE1=0, 1: PE0=0 PE1=1, 2: both 0.
    let bop = match arg {
        0 => (1 << 0) | (1 << (1 + 16)),
        1 => (1 << (0 + 16)) | (1 << 1),
        _ => (1 << (0 + 16)) | (1 << (1 + 16)),
    };
    unsafe {
        w32(GPIOE + 0x18, bop);
        modify32(GPIOE + 0x00, 3 | (3 << 2), 1 | (1 << 2));
    }
}

fn pc7(high: bool) {
    unsafe {
        w32(GPIOC + 0x18, if high { 1 << 7 } else { 1 << (7 + 16) });
        modify32(GPIOC + 0x00, 3 << 14, 1 << 14);
    }
}

/// Frequency counter state (FUN_08013450).
struct Freq {
    next: u64,
    src_alt: bool,
    /// A row with the counter on went out (`write_config`): the gate running
    /// now started at the row write, through the relay / input path settle;
    /// the first counter access after the settle only restarts it
    /// (`freq_restart_fresh`). Device 2026-10-24: DC V -> Hz read 0.00 /
    /// 0.99 / 17.04 / 19.94 Hz on the first READ? of a 40 Hz square.
    fresh: bool,
    /// The source's latest result is a clean, non-zero one (`Freq::take`).
    seen: [bool; 2],
    /// Results in a row without counted edges per source (`Freq::take`).
    zc: [u8; 2],
    /// A value was shown since the restart (`Freq::take`).
    shown: bool,
    last: [f32; 2],
    /// The duty (%) of each source's `last`.
    lastd: [f32; 2],
    change: [f32; 2],
    f_khz: f32,
    duty: f32,
    range: u8,
    /// The running gate's start (a restart), ms (low 32 bits of `now`).
    t0: u32,
    /// SYSCLK ticks of each source's `last` gate (`counter_vals` n).
    lastn: [u32; 2],
    /// Ticks of the shown value's gate (0: none / no signal): `freq_ext`.
    n: u32,
    /// The latest clean result of either source, kHz (NaN after a restart,
    /// 0 after no signal): `Freq::take`'s agreement check.
    prev: f32,
    /// Each source's `last` agreed with the clean result before it.
    conf: [bool; 2],
    /// The pick of the latest `take` is provisional (`Reading::prov`).
    prov: bool,
    /// The counter shows no input above its latest result (`f_khz`; NaN:
    /// none yet) up to `hf_t` (ms, low 32 bits): AC mV's V_GATE
    /// (`mv_vside`). False until the first check of a row's gate.
    hf_ok: bool,
    hf_t: u32,
}

/// Two clean gates in a row agree within this (relative): `Freq::take`.
const FREQ_AGREE: f32 = 0.01;

/// The first counter check after a restart (the stock's 350 ms period:
/// above ~6.7 Hz the gate is over by then, so the update rate is the
/// stock's), ms.
const FREQ_CHECK_MS: u64 = 350;
/// Later checks while the gate still runs, ms.
const FREQ_RECHECK_MS: u64 = 20;
/// A gate that counted nothing (CTB = CTC = 0) this long gives up: no edge
/// in 2.2 s is no signal (a 0.5 Hz input has an edge every 2 s), ms.
const FREQ_NOEDGE_MS: u32 = 2200;
/// A gate with counts but no end gives up after this, ms. A gate takes up
/// to two input periods plus 53.3 ms (the wait for the opening edge, then
/// the 0x40000-tick minimum, then the next edge): 4.05 s at 0.5 Hz; this
/// limit measures down to ~0.45 Hz.
const FREQ_GATE_MAX_MS: u32 = 4400;

/// The longest a gate may run (`gate_check`). With counts (`edges`) the
/// gate opened: the input is there, maybe slower than the value shown (a
/// frequency step down), so FREQ_GATE_MAX_MS. Without counts, with a value
/// shown (`f_khz` > 0) 3.5 of its periods + 150 ms (a gate takes up to two
/// periods + 53.3 ms; the margin covers a missed opening edge, unknown on
/// the chip; a signal gone then reads as no signal about as fast as
/// before), at least FREQ_CHECK_MS, at most FREQ_NOEDGE_MS.
pub(crate) fn gate_limit_ms(f_khz: f32, edges: bool) -> u32 {
    if edges {
        return FREQ_GATE_MAX_MS;
    }
    let p = if f_khz > 0.0 { 3.5 / f_khz + 150.0 } else { FREQ_NOEDGE_MS as f32 };
    (p.min(FREQ_NOEDGE_MS as f32) as u32).max(FREQ_CHECK_MS as u32)
}

/// A counter check (`Dmm::freq_gate`).
#[derive(Clone, Copy, PartialEq, Debug)]
#[repr(u8)]
pub(crate) enum Gate {
    /// Still counting: check again later.
    Wait,
    /// A whole gate: the counts are a result.
    Done,
    /// Given up without counts within `gate_limit_ms` (`Freq::take`).
    Empty,
    /// Given up: nothing counted for the whole FREQ_NOEDGE_MS (no signal).
    Quiet,
    /// Given up with counts: the gate opened but did not end in
    /// FREQ_GATE_MAX_MS (an input below the range, or gone mid-gate).
    Late,
}

/// The HY3131 counter is a reciprocal one (DS-HY3131 14): after the
/// 0x40000-tick minimum (CTA overflow) it counts on until CTB holds a whole
/// number of input cycles, then stops (CTF). So a gate is over when CTA is
/// past the overflow and stands still between two reads (`cta`, `cta2`)
/// with cycles counted; its length then follows the input, not a fixed
/// window. Else it waits up to `gate_limit_ms` (`el_ms` since the restart).
pub(crate) fn gate_check(ctc: u32, ctb: u32, cta: u32, cta2: u32, el_ms: u32, f_khz: f32) -> Gate {
    if ctr_valid(ctb, cta) && cta == cta2 {
        return Gate::Done;
    }
    let edges = ctb > 0 || ctc > 0;
    if el_ms < gate_limit_ms(f_khz, edges) {
        return Gate::Wait;
    }
    if edges {
        Gate::Late
    } else if el_ms >= FREQ_NOEDGE_MS {
        Gate::Quiet
    } else {
        Gate::Empty
    }
}

/// The extra (small) digit of a frequency readout `v` (display units, `dec`
/// decimals) from a gate of `n` SYSCLK ticks: the reciprocal count resolves
/// v / n (one tick of the gate), shown when that is at most one count of the
/// extra digit, 10^-(dec+1). E.g. 10 Hz (one 100 ms period, n = 491520):
/// 0.00002 Hz <= 0.0001 Hz, 10.0000 Hz; 50 Hz (three periods, 60 ms):
/// 0.00017 Hz > 0.0001 Hz, 50.000 Hz.
pub(crate) fn freq_ext(v: f32, dec: u8, n: u32) -> bool {
    let mut w = 1.0f32;
    for _ in 0..=dec {
        w *= 0.1;
    }
    n > 0 && v >= 0.0 && v / n as f32 <= w
}

/// `Freq::take`: what this gate's result lets the frequency mode show.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) enum FreqPick {
    /// Nothing yet (no reading: the display keeps "--", READ? waits).
    None,
    /// The source's `last` value.
    Src(usize),
    /// No signal: both sources counted no edges (0.000 Hz).
    NoSignal,
}

impl Freq {
    /// The sources start over (a row write, `poll_freq_mode`).
    fn restart_sources(&mut self) {
        self.seen = [false; 2];
        self.zc = [0; 2];
        self.shown = false;
        self.change = [1e6; 2];
        self.prev = f32::NAN;
        self.prov = false;
    }

    /// One gate's result of source `i` (`f` kHz, duty `d` %, `ok`: a whole
    /// gate with counted edges, `ctr_valid`). Device d2aff22 (2026-10-24):
    /// after every Hz entry source 0 (0xf0) read 0 Hz / 0 % in its first
    /// gates and was shown (0.000 Hz in amber, into READ? and MIN / AVG)
    /// until source 1 won at 2.42 s. So a gate without edges is no result:
    /// the source drops out (`seen` false) and the other one shows; a
    /// glitch (`freq_glitch`) is no result either. 0.000 Hz (no signal)
    /// only once both sources counted nothing NO_SIGNAL_N times in a row
    /// (once each after a value was shown: the signal went away).
    /// `n`: the gate's SYSCLK ticks; `quiet`: a gate that counted nothing
    /// for FREQ_NOEDGE_MS (`Gate::Quiet`) counts as NO_SIGNAL_N empty
    /// gates of both sources (they count the same node; a source with a
    /// clean value still shows it): no signal after one such gate, ~2.9 s
    /// from an entry (four 2.2 s gates would be ~9 s). A gate given up with
    /// counts (`edges`) is no result and no "no signal" either.
    ///
    /// Device 2026-10-26 (MR9270 square 1 Hz -> 0.5 Hz): a gate across the
    /// step read 1.402781 Hz (final) for ~8 s, then 0.000 Hz, 0.500022 Hz
    /// from ~12 s. So once a value was shown, a clean result is final
    /// (`conf`) only within FREQ_AGREE of the clean result before it (either
    /// source); else the pick is provisional (`prov`: grey, not in READ? /
    /// statistics) until the next gate agrees. No signal right after a
    /// frequency is provisional, final from the next empty gate: on the
    /// device the MR9270's output paused across a step (0.000 Hz final for
    /// ~6 s after 1 -> 0.5 Hz with a quiet gate final at once; the pause is
    /// INFERRED from that).
    fn take(&mut self, i: usize, f: f32, d: f32, n: u32, g: Gate) -> FreqPick {
        const NO_SIGNAL_N: u8 = 2;
        let glitch = freq_glitch(f, d);
        let quiet = g == Gate::Quiet;
        if g != Gate::Done || f <= 0.0 {
            self.seen[i] = false;
            if quiet {
                self.zc = [self.zc[0].max(NO_SIGNAL_N), self.zc[1].max(NO_SIGNAL_N)];
            } else if g != Gate::Late {
                self.zc[i] = self.zc[i].saturating_add(1);
            }
            self.change[i] = 1e6;
        } else if glitch {
            // Edges missed: neither a value nor "no signal"; the last clean
            // value stays the source's.
            self.change[i] = 1e6;
            self.zc[i] = 0;
            if self.seen[i] {
                // A source whose latest gate glitched does not show now.
                self.seen[i] = false;
            }
        } else {
            // A source's first clean result has no change to rank it by
            // (1e6: shown only while it is the one source with a result).
            self.change[i] = if self.seen[i] { freq_change(f, d, self.last[i]) } else { 1e6 };
            // Agreeing with the other source's latest result confirms that
            // one too (two gates in a row).
            let p = self.prev;
            let ok = !(p > 0.0) || (f - p).abs() <= FREQ_AGREE * p;
            self.conf[i] = ok;
            if ok && self.last[1 - i] == p {
                self.conf[1 - i] = true;
            }
            self.prev = f;
            self.last[i] = f;
            self.lastd[i] = d;
            self.lastn[i] = n;
            self.seen[i] = true;
            self.zc[i] = 0;
        }
        let pick = match freq_best(self.seen, self.last, self.change) {
            Some(b) => FreqPick::Src(b),
            None => {
                let n = if self.shown { 1 } else { NO_SIGNAL_N };
                if self.zc[0] >= n && self.zc[1] >= n { FreqPick::NoSignal } else { FreqPick::None }
            }
        };
        match pick {
            FreqPick::None => return pick,
            FreqPick::Src(b) => self.prov = !self.conf[b],
            FreqPick::NoSignal => {
                // The next value is final at once (as at an entry).
                self.prov = self.f_khz > 0.0;
                self.prev = 0.0;
            }
        }
        self.shown = true;
        pick
    }
}

/// HY3131 counter registers (CTC R15..17, CTB R18..1A, CTA R1B..1D) ->
/// (frequency kHz, duty %). n = CTA + 0x40000 ticks of the 4.9152 MHz time
/// base (CTA restarts at 0xfc0000), CTB input periods, CTC high ticks.
pub(crate) fn counter_vals(ctc: u32, ctb: u32, cta: u32) -> (f32, f32) {
    let n = cta as f32 + 262_144.0;
    (ctb as f32 / n * 4915.28, ctc as f32 / n * 100.0)
}

/// A counter result that cannot be a clean measurement: input periods
/// counted (f > 0) but the input never seen low during the gate (duty
/// shown as 100.0 %, >= 99.95). (The low side is not rejected: real narrow
/// pulse trains below 0.5 % duty exist.) Device 2026-10-21 (40 Hz square, AC+DC
/// -> Hz): for ~11 s one counter source read 1/3 and 2/3 of the frequency
/// with duty 100 % (edges missed while the input path settles); its results
/// must neither win the source choice nor reach the readout / MIN / AVG.
pub(crate) fn freq_glitch(f_khz: f32, duty: f32) -> bool {
    f_khz > 0.0 && duty >= 99.95
}

/// Frequency mode source choice (FUN_08013450's two counter sources): the
/// one whose result changed least since its previous one. A glitch result
/// (`freq_glitch`) or no signal counts as a change of 1e6.
pub(crate) fn freq_change(f_khz: f32, duty: f32, last: f32) -> f32 {
    if f_khz == 0.0 || freq_glitch(f_khz, duty) { 1e6 } else { (f_khz - last).abs() }
}

/// The source to show (`poll_freq_mode`) among those with a clean result
/// (`seen`): None without one, the one alone, else the smaller change -
/// except that a source at 1/2, 1/3 or 2/3 of the other (+-0.5 %) loses:
/// missed edges (the device's 13.3 / 26.7 Hz of 40 Hz, one or two of the
/// gate's three edges missed). Both sources count the same node, so such a
/// ratio between them is not the signal.
pub(crate) fn freq_best(seen: [bool; 2], last: [f32; 2], change: [f32; 2]) -> Option<usize> {
    match seen {
        [false, false] => return None,
        [true, false] => return Some(0),
        [false, true] => return Some(1),
        _ => {}
    }
    let hi = (last[1] > last[0]) as usize;
    let (lo, hv) = (last[1 - hi], last[hi]);
    if lo > 0.0 && [0.5f32, 1.0 / 3.0, 2.0 / 3.0].iter().any(|&k| (lo - k * hv).abs() <= 0.005 * k * hv) {
        return Some(hi);
    }
    Some((change[1] < change[0]) as usize)
}

/// A counter read is a whole gate with counted edges: CTB > 0, and CTA past
/// its overflow (it counts up from the 0xfc0000 preset; at or above it the
/// 53.3 ms have not run out, the counts are a gate in progress).
pub(crate) fn ctr_valid(ctb: u32, cta: u32) -> bool {
    ctb > 0 && cta < 0xFC_0000
}

/// Restart the HY3131 counter on source `src` (R20 without ENCTR): ENCTR
/// off, the gate preset CTA = 0xfc0000 (0x40000 SYSCLK = 53.3 ms), ENCTR on.
fn ctr_restart(src: u8) {
    hy::write(0x20, &[src & !2]);
    hy::write(0x1B, &[0x00, 0x00, 0xFC]);
    hy::write(0x20, &[src | 2]);
}

/// Frequency auto range for `f` kHz; x = factor * over (6.18 kHz at 60000
/// counts): range r spans up to x * 10^(r-2). Above 10000 x: `prev`.
pub(crate) fn freq_range_for(f: f32, x: f32, prev: u8) -> u8 {
    FREQ_DEC.iter().position(|&d| f < x * d).map_or(prev, |r| r as u8)
}

/// Frequency readout for `f` kHz in range `ri`. The stock notes give
/// printf format indices (fmt k = "%7.(4-k)f": Hz fmt ri+1, kHz fmt ri-2,
/// MHz fmt ri-5; aux one digit less), so the decimals are 4 - fmt, as in the
/// other functions: 60.000 Hz, 600.00 Hz, 6.0000 / 60.000 / 600.00 kHz,
/// 6.0000 / 60.000 MHz. (Using fmt as the decimals showed 0.7 kHz as "1".)
pub(crate) fn freq_val(f: f32, ri: u8, aux: bool) -> Val {
    let ri = ri as i32;
    let a = aux as i32;
    let (v, dec, unit) = if ri < 2 {
        (f * 1000.0, 3 - ri - a, "Hz")
    } else if ri < 5 {
        (f, 6 - ri - a, "kHz")
    } else {
        (f * 1e-3, 9 - ri - a, "MHz")
    };
    Val::new(v, dec.clamp(0, 4) as u8, unit)
}

/// Capacitance ramp state (r5..r7).
struct CapRamp {
    discharging: bool,
    prev_v: f32,
    prev_t: u64,
    med: [f32; 5],
    med_i: usize,
    class: u8,
    buf: [f32; 35],
    n: usize,
    cmp: Option<bool>,
    first: bool,
}

pub struct Dmm {
    model: u8,
    mode: u8,
    /// m+5 == 2: AC+DC. `phase_n` = S+10 (readings left in this phase),
    /// `dc_val` = S+0x20 / m+0xcc (last DC part), `ac_raw` = S+0x1c.
    acdc: bool,
    phase_n: u8,
    dc_val: f32,
    ac_raw: f32,
    /// AC+DC parts measured since the last reset (bit 0 AC, bit 1 DC): no
    /// reading until both are, so the first value (and a SCPI MEAS) is never
    /// |DC| or |AC| alone (the stock shows hypot(0, DC) at first).
    acdc_seen: u8,
    /// AC+DC parts measured since the last `reset_values` (entry, every
    /// unit change), bit 0 AC, bit 1 DC: `acdc_seen` without its range-change
    /// clear (`ac_settle_start`); updated by `parts_known`. Until a part is
    /// known it reads 0 (`reset_values`), so the magnitude is the other
    /// part alone: no step down on it (`acdc_down`).
    acdc_known: u8,
    /// AC+DC / AUTO: the AC part is settling after a range change since
    /// this time (0: not), `acs_prev` = the previous AC result in it (< 0:
    /// none yet), `acs_flat` = results in a row that did not fall. See
    /// ACS_STEP.
    acs_t0: u64,
    acs_prev: f32,
    acs_flat: u8,
    /// `acs_tp` = when `acs_prev` came, `acs_d` = how much it fell from
    /// the one before in the same AC phase (< 0: unknown), `acs_dc` = the
    /// last DC part used (NaN: none on this range; an input step, ACS_DC_STEP).
    acs_tp: u64,
    acs_d: f32,
    acs_dc: f32,
    /// The settle running is for an input step (not a range change): the
    /// AC part held is stale, so the DC phase steps no range down on it.
    acs_step: bool,
    /// DC conversions after a range change (`vet_ad1`, VET_*): results still
    /// to vet, results held so far, the previous conversion on this range
    /// (i32::MIN: none yet).
    vet_left: u8,
    vet_held: u8,
    vet_prev: i32,
    /// Conversions still to drop after an R22 change soon after a row
    /// write (R22_DROP_*; DC V / A `osr`, the ohm in-range OSR), and the
    /// time of the last row write (`set`).
    r22_drop: u8,
    row_t: u64,
    /// Plain DC V / A: the conversion before this one on the row was over
    /// range (`over_ok`).
    over_prev: bool,
    /// Plain AC A: the row write whose results are still vetted (ms, 0 =
    /// vetted; `aca_vet`) and the previous result on the row (NaN: none).
    aca_vet_t: u64,
    aca_prev: f32,
    /// Plain AC V: this row came from a range step (ACV_DC_*).
    acv_vet: bool,
    /// The reading of this poll is provisional by G7 only (`acdc_reading`).
    prov_side: bool,
    /// AC+DC: the first DC result of the run asking for a step down (ms, 0
    /// none; ACDC_DOWN_MS).
    down_t: u64,
    /// A range or function change since the last reading (READ? / FETCh?
    /// answer +9.91E37 meanwhile, `reading_pending`).
    rd_pending: bool,
    /// AUTO coupling (sub-mode 3; `acdc` is set too: the AC+DC alternation
    /// measures both parts): `auto_ac` the part shown (true = AC), `auto_n`
    /// cycles evaluated since the last reset (0: none yet, each reading then
    /// takes the plain rule), `auto_votes` consecutive cycles that wanted the
    /// other part (`auto_eval`), `auto_total_si` |total| of the last reading
    /// in SI (the sentinel / OVERLOAD checks look at the whole signal);
    /// `auto_t0` since when (ms, 0 not) the first decision waits for a
    /// settled AC part, `auto_prev` the AC part at the last evaluation.
    cpl_auto: bool,
    auto_ac: bool,
    auto_n: u8,
    auto_votes: u8,
    auto_t0: u64,
    auto_prev: f32,
    auto_total_si: f32,
    range: i8,
    manual: bool,
    /// m+6: V autorange (bit0 mV->V, bit1 V->mV); m+7: A (1 mA..A, 3 uA..A).
    v_auto: u8,
    a_auto: u8,
    /// m+8: ohm variant (selects f10_p8set when 1).
    ohm_var: u8,
    /// S+0x40: no reading accepted before this time.
    deadline: u64,
    /// Time of the last function change (`set`): ohm steps down one range
    /// at a time until OHM_SINGLE_MS after it (`poll_ohm`).
    entry: u64,
    /// Plain AC mV left on a clip (MV_CLIP_K): 0 = none, < 0 = AC V 6 V
    /// settling (since `row_t`), > 0 = AC V 6 V's AC part then (V, the
    /// reference). While set, AC V 6 V does not step down to mV (the AC V
    /// row is AC-coupled: a DC that clipped mV is not seen there, so the
    /// down rule would bring it back into the clip); cleared once the AC
    /// part moves off the reference (the input changed) or on a function
    /// change (`mv_clip_holds`).
    mv_clip: f32,
    /// Continuity's jack check phase (`cont_jack`: 0 none, 1 settle, 2
    /// window, 3 back) and the row's R25 to restore.
    jwin: u8,
    /// Continuity / diode: the last reading that was not OL (V-COM closed by
    /// something; V_GATE, `CLOSED_HOLD_MS`); 0 none.
    closed_t: u64,
    r25: u8,
    prev_ad1: i32,
    r22: u8,
    ohm_settled: bool,
    /// Ohm vet (OHM_VET_*): the last conversion of this row (kohm, raw;
    /// INFINITY = OL, NaN = none since the row write) and whether two in a
    /// row agreed on it (its conversions are final from then on).
    ohm_prev: f32,
    ohm_ok: bool,
    /// Two conversions of this row agreed (not the OHM_VET_MAX_MS escape):
    /// what the fuse check decides on.
    ohm_agreed: bool,
    /// Time of this row's write (ms; OHM_VET_MAX_MS).
    ohm_t0: u64,
    /// Ohm autorange (`poll_ohm`, OHM_BAND): this row was reached by a step
    /// down from range `ohm_down_r` (-1: not, or an in-range conversion
    /// came since), which read `ohm_down_k` kohm; and range `ohm_blk_r`
    /// (-1: none) is held against a step down while it reads within
    /// OHM_BAND of `ohm_blk_k` kohm. Both end at the next row write (`set`).
    ohm_down_r: i8,
    ohm_down_k: f32,
    ohm_blk_r: i8,
    ohm_blk_k: f32,
    /// Plain AC: the AC filters were reset (entry, range step); the next
    /// used result seeds them (`f_ac`, `f_lpf`) instead of the EMA's 0.99
    /// step from 0, which read 1 % low and took ~1 s to close.
    ac_seed: bool,
    /// Plain AC V / mV after that reset: the results to drop before the seed. 2 =
    /// the first poll past the settle: its INTF read clears the flags of a
    /// block the chip latched during the settle (device 185c066: DC V -> AC
    /// V read 0.707 x the input, half the window before the AC reached the
    /// node); 1 = the next result, whose window began before that read.
    ac_lead: u8,
    /// AC+DC mV: the last DC phase result's magnitude was over the range
    /// (`mv_vside`); plain DC mV: the last result was (its V_GATE).
    mv_dc_drv: bool,
    f_ac: f32,
    f_lpf: f32,
    f_dc: f32,
    f_diode: f32,
    f_cap: f32,
    cap_c: f32,
    cap_cnt_up: u8,
    cap_cnt_dn: u8,
    cap_cnt_small: u8,
    ramp: CapRamp,
    beep_class: u8,
    beep_e: u8,
    beep_next: u64,
    beep_n: u8,
    /// The beeper's condition (`beeping`): continuity, CTSTA ACPO as last
    /// read; diode, the pattern's last on / off (`diode_beep`).
    beep_on: bool,
    t_phase: u8,
    t_code_a: i32,
    t_cj: f32,
    freq: Freq,
    seq: u32,
    pa_now: Option<u8>,
    /// Analog DMM rail (PE2) off: no polling (App::update_rails).
    suspended: bool,
    /// Rail wake: `start` once the rail has settled (0 = none pending).
    wake_at: u64,
    /// Stall watchdog (`watchdog`): last reading / (re)start, last poll,
    /// next check (ms); restarts after a lost row / a long silence.
    wd_t: u64,
    wd_poll: u64,
    wd_chk: u64,
    /// R2A..R31 of the row last written (`write_config`, `pw_write_config`):
    /// the bytes nothing rewrites while a row runs.
    chk: [u8; 8],
    /// SYST:RAIL:STATe? counters: rail wakes, lost-row and silence restarts.
    pub wd_n: [u16; 3],
    /// Temperature sensor (T_NAMES index), per-sensor offsets (0.1 C; the
    /// TC one is applied to the cold junction), user NTC R25 (ohm) / Beta.
    t_sensor: u8,
    t_offs: [i16; 7],
    /// Temperatures (main, cold junction) in deg F instead of deg C.
    t_f: bool,
    ntc_user: (f32, f32),
    /// ms of the last cold-junction measurement (0 = never).
    t_cj_at: u64,
    /// Resistive sensors: filtered C, last R (ohm), range-change votes.
    t_ema: f32,
    t_res: f32,
    t_votes: u8,
    /// +1 digit: moving average of the raw codes (`avg_cfg`); `avg_skip`
    /// drops the first conversion after the OSR was switched to 0x17.
    avg: Avg,
    avg_skip: bool,
    /// Conversions in the mean of the reading being produced (Reading::avg_n).
    avg_n: u8,
    /// AC+DC diagnostic: the last ACDC_DIAG_N AC phases (ring), the number
    /// of AC phases seen, and whether the current AC phase is being logged.
    diag: [AcPhaseDiag; ACDC_DIAG_N],
    diag_k: u32,
    diag_on: bool,
    /// Diagnostic: time of the previous logged result, counter restarts since.
    diag_t: u64,
    diag_ctr: u8,
    /// FR-05 conversion rate (persisted).
    rate: Rate,
    /// FAST in DC V / mV / A: the unchopped AD1's offset (fastvos.rs; in
    /// SRAM, `fast_static`).
    fast: &'static mut Fast,
    /// Time of `poll` (ms), for the interval measurements below.
    t_poll: u64,
    /// Last reading / last AD1 conversion (ms; None: the next interval spans
    /// a config write and is not counted) and their measured mean intervals.
    t_rd: Option<u64>,
    period_ms: f32,
    t_conv: Option<u64>,
    conv_ms: f32,
    /// `avg_n` of the latest reading (`avg_n` itself is per poll).
    rd_avg_n: u8,
    /// Wrong-connection sentinel (sentinel.rs), the mode its history
    /// belongs to, R29 of the row (peak-hold restart), the last ohm /
    /// continuity / diode result was one only a source gives (LIVE?).
    pub sent: Sentinel,
    sent_mode: u8,
    r29: u8,
    /// Last peak-hold harvest in `poll` (frequency, AD2 kinds), ms.
    pk_t: u64,
    /// AC V with the DC? alternation: ENPKH back on at this time (ms; 0 =
    /// none pending): the AD1 conversion in flight at an input switch must
    /// not reach the hold.
    pkh_due: u64,
    /// A row went out (`write_config`) and the sentinel has not been
    /// re-armed since (`sent_rearm`, the first poll past the settle).
    sent_arm: bool,
    /// When the relay last left the ohm network for the V/A path, or the
    /// chip restarted (`start`) (ms; sentinel::OHM_EXIT_*), None before.
    ohm_exit_t: Option<u64>,
    /// Before a DC A family row is entered while `a_blind` (and V-JACK?
    /// is enabled), the temperature row runs for this long (ms; default
    /// PRECOND_MS; DBG:SENTinel:PRECond <ms>, RAM only, 0 = off). Device
    /// 40728b8: a real 24 V on V-COM read CTSTA 0x70 / 0x50 in DC A only
    /// when TEMP came before (DC V 60 V+, the ohm rows and a chip restart
    /// left the A row reading 0). Device d2aff22: 100 / 300 / 1000 ms of
    /// it brought V-JACK? back at 1.9 / 2.1 / 2.8 s after DC V 60 V and
    /// after RES, with no false hit at 5 V or with the source off. Cost:
    /// the relay pulse there and back (2 x 10 ms) and the blocked loop.
    pub precond_ms: u16,
    /// The last row that changes the V-tap network state seen in DC A was
    /// not the temperature row (DC V 60 V+, ohm / continuity / diode / cap
    /// / frequency rows, a chip restart): see `precond_ms`.
    a_blind: bool,
    /// Plain AC A: over-range results are held back until this time (ms;
    /// 0 = set at the first result after the next row write: + AC_OVER_HOLD_MS),
    /// `poll_ac`.
    ac_over_until: u64,
    live: bool,
    /// OVERLOAD latch (doc 4.3 / 4.7, sentinel::overload_trips): None = not
    /// in force.
    ovl: Option<Ovl>,
    /// Time of the last timed OVERLOAD release (None: none yet).
    ovl_rel_t: Option<u64>,
    /// A-jack protection (doc 4.8): the probe (OP2 / AD2 peak on PB2) is
    /// in the row; the latch (the A jack on the 10 mohm shunt).
    probe: bool,
    /// The probe's row was just written: the AD2 peak hold is cleared once
    /// it settled (sentinel::PROBE_SETTLE_MS) before its first window counts.
    probe_settle: bool,
    aprot: Option<AProt>,
    /// Fuse check in progress (`fuse_start`) and the last result.
    fuse_job: Option<FuseJob>,
    fuse_res: sentinel::FuseRes,
    /// Power meter state and settings (powermeter.rs).
    pub pw: &'static mut Pw,
    /// AC+DC method (acdcm.rs; DBG:ACDC:METH, RAM only; feature `acdc-meth`).
    #[cfg(feature = "acdc-meth")]
    pub am: &'static mut am::Am,
    /// AC+DC event trace (acdctr.rs; DBG:ACDC:TRACE, RAM only, off at boot).
    pub tr: &'static mut tr::Trace,
    /// Burden voltage readout in the A functions (Settings 3/3 "Burden
    /// voltage", SCPI DISP:BURD; `burden_val`): I x the shunt's R_path in
    /// aux1 (DC) / aux2 (AC, instead of the duty cycle).
    pub burden_show: bool,
    /// Ohm readings below 0 shown (Settings 3/3 "Negative ohm", SCPI
    /// RES:NEG; `poll_ohm`): the factory offset subtracted without the
    /// stock clamp at 0, so shorted leads read e.g. -0.028 ohm.
    pub ohm_neg: bool,
    /// The experimental 600 Mohm range (r7, `poll_ohm600`).
    pub o6: Ohm600,
}

/// Fuse check (`Dmm::fuse_start`): what to go back to, start time, ohm
/// readings seen.
#[derive(Clone, Copy)]
struct FuseJob {
    func: u8,
    back: Option<(u8, i8)>,
    t0: u64,
    n: u8,
}

/// Fuse check: the final ohm readings taken (the last one decides) and the
/// time limit. Only a final reading of the 600 ohm row whose vet saw two
/// conversions agree counts (OHM_VET_*), never the n-th raw conversion:
/// designed when the ohm entry waited 2.5 s, the third conversion after the
/// fast entry (150 ms, 6bc494a) could still be charging the node (owner on
/// ea73ea5: 18 ohm instead of 48, "Check leads").
const FUSE_READINGS: u8 = 1;
const FUSE_TIMEOUT_MS: u64 = 8000;

/// A-jack protection latch (`aprot_poll`): `ack` = the user chose a
/// function / range / AUTO since the trip; the probe below
/// sentinel::APROT_GONE_A since `low_since`.
#[derive(Clone, Copy)]
struct AProt {
    ack: bool,
    low_since: Option<u64>,
}

/// OVERLOAD latch: the uA / mA shunt overloaded and the meter went to the
/// 10 A shunt. Shunts `barred` and above (sentinel::shunt_of numbering: 1
/// mA, 2 uA) stay out of the autorange until the user picks a range /
/// function / AUTO, or the current stays below `thr` for
/// sentinel::OVL_RELEASE_MS; then `back` (the manual range it left: DMM
/// mode / range, or the power meter's I index) is restored, or (auto) the
/// autorange may use the barred shunt again.
#[derive(Clone, Copy)]
struct Ovl {
    barred: u8,
    back: Option<(u8, i8)>,
    thr: f32,
    low_since: Option<u64>,
    /// No timed release (tripped again within OVL_RETRIP_MS of one).
    sticky: bool,
}

/// Mean interval: first sample taken as is, then an EMA (1/4).
fn ema_ms(acc: &mut f32, last: &mut Option<u64>, now: u64) {
    if let Some(t) = *last {
        let dt = now.saturating_sub(t) as f32;
        *acc = if *acc == 0.0 { dt } else { *acc + 0.25 * (dt - *acc) };
    }
    *last = Some(now);
}

// AC+DC: what reaches the HY3131 and the board, as the stock does it
// (stock/re/docs/history/acdc-stock-vs-opendm40.md sections 7 and 8.1), one switch per difference
// so they can be bisected on the device (false = the opendm40 fab2b96 way).
//
/// DC phase at the config row's OSR 8192 (R22 = 0x15), and no R22 write
/// while it runs. The stock's 0x17 write is gated by a once-flag that stays
/// set in steady AC+DC, so its DC phase stays at 8192; but an AD1 step (a
/// range change, which opendm40 vets) clears it and the next steady result
/// writes 0x17 once. opendm40 did that too until device check ad97fd7
/// (DBG:ACDC:TRACE): after a range change that lands in a DC phase, the next
/// DC phase's first result is read at its settle deadline, up to a
/// conversion late, so that 0x17 write lands in the middle of the running
/// conversion; the next result came after ~155 ms (not 400) at 52 % of the
/// input on 60 V (40 % on 6 V), the 0x15 write that this step caused gave a
/// second result at the same 52 %, and the two agreed, so `vet_ad1` took it
/// as the DC part (5 of 5 traced hits). No R22 write in AC+DC removes both.
/// false: R22 = 0x17 right after every DC-phase row.
const ACDC_STOCK_OSR: bool = true;

/// Misconnect sentinel, AC V / AC mV: restart AD1's peak hold (ENPKH 0 -> 1
/// in R29) after each harvest. The datasheet says ENPKH = 0 only presets
/// PKHMAX / PKHMIN; whether rewriting R29 disturbs the RMS / LPF blocks on
/// the real chip is device test T9 (false: the hold is never restarted, so
/// A-JACK latches until the row is rewritten - a range or function change).
const PKH_RESTART: bool = true;
/// LIVE? thresholds: ohm AD1 code (0.24 % of 2^23), continuity ohm, diode V.
const LIVE_CODE: i32 = 20_000;
const LIVE_CONT_OHM: f32 = 2.0;
/// Continuity / diode V_GATE (owner 2026-10-26: A-JACK for ~1.5 s while the
/// probes were touched together in continuity, A jack empty): a reading
/// under OL within this long = V-COM closed, no A-JACK (an asserted one
/// drops). The PB1 hits come with the contact (INFERRED: the source's
/// current step on the shared COM; not traced). The case A-JACK is for, the
/// leads in A / COM, leaves V-COM open: OL. The hold covers a contact that
/// bounces open (an OL reading while PB1 still hits).
const CLOSED_HOLD_MS: u64 = 1500;
/// ... and an OL reading counts as "open" only from this long after the hit
/// run began: a conversion that ends just after the contact (OSR_SLOW: 400
/// ms long once the reading stood still) still reads OL (emulator senttest
/// cont-bounce). Cost: A-JACK for a current in A 0.6-0.9 s after its onset
/// there (was 0.16-0.36 s; emulator).
const CLOSED_VGATE_LAG_MS: u16 = 450;
const LIVE_DIODE_V: f32 = 0.05;
/// FUN_08012c30 pulses the PA latching relay (10 ms) on every call, i.e. on
/// every phase switch, between the R26/R34/R36 zeroing and the 22-byte row.
/// false: only when the PA state changes (never inside V AC+DC).
const ACDC_RELAY_PULSE: bool = true;
/// FUN_08013734 runs the frequency counter (FUN_08013450, restart every
/// 350 ms: R20 = 0xf0, CTA, R20 = 0xf2) in AC+DC too: on every poll, also
/// during the settle, in the ACV / DCV / DCmV / DCA phases, and after every
/// result in the ACA phase (never in the ACmV phase).
const ACDC_FREQ_RESTART: bool = true;

/// AC+DC diagnostic ring: AC phases kept (`acdc_diag`).
const ACDC_DIAG_N: usize = 4;

// AC+DC (method A) and AUTO coupling after a range or unit change (manual or
// autorange): the AC-coupled AD2 node has to recharge to the new divider
// tap's DC level, and while it does its step reads as AC. Device check
// 540e056 (1 V DC): 6 -> 60 V gave an AC part of 0.35 V (1.06 V from the
// range page) for ~2-4 s, then a tail 18 -> 1 mV over ~10 s, READ? up to
// +6 %; 60 -> 6 V 0.15 V for ~2 s. The emulator's node (tau 0.3 s, charged
// in the AC phases only) shows 5.3 V for the first AC phase. The stock does
// nothing about it (OL + 600 ms, stock/re/docs/history/acdc-stock-vs-opendm40.md 2). opendm40
// discards the AC results until the transient is over (`ac_settle_step`):
// it only adds to the RMS and decays result by result (device: a few % per
// 100 ms result), while a real AC part stays, so the AC part is settled
// once ACS_FLAT_N results in a row moved by less than ACS_STEP each
// (`acs_flat`), or one is below ACS_FLOOR of the range's full scale, or
// ACS_MAX_MS after the change at the latest.
// Meanwhile AC+DC gives no reading (like the entry: no value before both
// parts are measured); AUTO keeps showing its part with the AC part held
// and does not re-decide.
//
// "Flat" (device decaf60: a 1 % step let the 600 -> 60 V tail of a 40 Hz
// square through, +0.55 / +0.24 % for 1.7 s): a result that fell by at
// most ACS_STEP (0.1 %) of itself since the previous one (or rose by at most
// that: `acs_flat`),
// and when the two falls before it in the same AC phase (results
// ACS_NEAR_MS apart at most) shrink as a decay does (ratio q < 1), the
// rest of that decay, d q / (1 - q), at most ACS_REM (0.05 %) of the
// result. The device's tail fell 0.6 % per 100 ms result there (node time
// constant ~0.27 s), so the first flat pair came on the next AC phase:
// 4.7678 V for 4.768 V. ACS_MAX_MS ends it anyway, also in a DC phase
// (`ac_settle_cap`: the last result held is the AC part), so the gap to
// the first reading stays below 6 s.
//
// The same settle after an input step (device decaf60: a 0.5-0.8 V DC step
// made the AC part read up to 3.4 V for ~1 s, AC+DC autorange went 6 -> 60
// V and back after 6 s): a DC part that moved by more than ACS_DC_STEP of
// the range between two DC results, or an AC result above ACS_JUMP x the
// AC part + ACS_JUMP_FLOOR of the range. AC+DC keeps its readings then,
// with the AC part held (the input changed, not the meter), and no range
// moves on the discarded results.
const ACS_STEP: f32 = 0.001;
const ACS_REM: f32 = 5e-4;
const ACS_NEAR_MS: u64 = 300;
const ACS_FLAT_N: u8 = 2;
const ACS_FLOOR: f32 = 5e-4;
const ACS_MAX_MS: u64 = 5000;
const ACS_DC_STEP: f32 = 0.005;
const ACS_JUMP: f32 = 1.5;
const ACS_JUMP_FLOOR: f32 = 0.005;

// AC+DC (method A) / AUTO step down: only in the DC phase, once its results
// asked for it for ACDC_DOWN_MS in a row (`acdc_down`; the AC phase's
// magnitude uses the last DC part, which may be one result of a transient).
// Device check f6c6e05 / ad97fd7: the calibrator's level change dips to
// 2-4 V for ~0.2 s, and one DC result in the dip stepped 60 -> 6 V on a
// 7.0 -> 6.3 V change (then back: 6.3 V is over range on 6 V, no reading
// for ~4 s). A step down now waits for the next DC phase (~1 s later);
// steps up are as before (at once).
const ACDC_DOWN_MS: u64 = 400;

// DC conversions after a range change (DC V / mV / A, the DC phase of
// AC+DC method A; `vet_ad1`). Device decaf60: VOLT:RANG:AUTO ON from 600 V
// steps down twice; the second row (6 V) goes out at the same R22 (OSR
// 8192) as the first, so AD1 is not restarted and its decimation filter
// still holds samples of the 60 V tap: one reading of 0.767-0.790 V for
// 1 V in 5 of 35 tries (one step from 60 V: the row changes R22 from 0x17,
// AD1 restarts, 0 of 10). An R22 change in the middle of a conversion
// (the same range set again, device 540e056: 25-51 %) and the AC+DC DC
// part of 53 % after a range-page change (decaf60) look the same. So the
// first VET_N (AC+DC: VET_N_ACDC, through the second DC phase: the first one
// after a set keeps the AC phase's count, up to 4) conversions on a new range
// count only when each agrees with the one before it within VET_DIV (0.5 %)
// of it or VET_FLOOR codes (~0.1 % of full scale); a conversion that does
// not is held: no reading, no step down (an overload still steps up: a
// mixed-in old tap can only read low on a step down), not in the averaging window,
// not the AC+DC DC part. After VET_HELD_MAX held ones the next counts
// anyway (a moving input). Costs one conversion (100 ms) per range step.
const VET_N: u8 = 2;
const VET_N_ACDC: u8 = 8;
const VET_HELD_MAX: u8 = 4;
const VET_DIV: i32 = 200;
const VET_FLOOR: i32 = 4000;

// Conversions right after a function change read wrong on the device
// (v0.1-pre1 / trig-pinch, 2026-10-24, device-check-2026-10-21.md; the
// mechanism is unknown):
// - the entry's first conversion: DC A -> DC V 6 V at MED / SLOW read
//   0.90-2.13 V for 1 V (5 of 8; the next one, 10 ms later, was right),
//   the open DC mA jack from DC V 6 V read +-44..56 mA or over range, and
//   OVERLOAD then moved CONF:CURR:DC 0.06 to the 10 A range (5 of 10). The
//   switch-latency.md proposal 5 shortcut took that conversion unvetted; it
//   is gone (every entry is vetted), and OVER_TWICE: a held conversion steps
//   up / trips OVERLOAD only as the second over-range one in a row on the
//   row (plain DC; one conversion later);
// - the two conversions after an R22 change made within ~1 s of the row
//   write read ~32-40 % of the input, and agree with each other: DC V
//   entered from AC V / ohm at MED / SLOW (`osr`'s 0x17 at ~0.4 s: 0.376 /
//   0.405 V for 1 V, 10 of 16), RES entered on 390 / 100 ohm / open (the
//   in-range OSR: 125.9 / 30.0 / 57.3 ohm, 134 kohm for 420 kohm, final in
//   amber, 6 of 12; the fuse check on an open input decided FAIL,279-286
//   ohm on such a pair after its 0x13 write, 9 of 10). The same R22 steps later in a function (an input step
//   at MED / SLOW) gave none. R22_DROP: such an R22 change within
//   R22_DROP_WIN_MS of the row write, and a row write that changes the R22
//   the chip ran (RES at SLOW -> DC V read OL, then 0.40 V twice), drop the
//   next R22_DROP_N conversions.
//   The HY3131 datasheet (DS-HY3131-V10 13.1) does not explain the effect:
//   it gives AD1 as a sinc^2 comb ("Comb Filter^2"; AD2 / AD3: ^3) whose
//   chopped modes (AD1CHOP 1x: 0x13 / 0x15 / 0x17) swap the input every OSR
//   period and halve the rate, and says nothing about an OSR change in a
//   running conversion. A sinc^2 restart would give one transitional result,
//   not two at ~32-40 %, so N is not derived from it (it stays 2, the device
//   count).
// - R22_LATE (`r22_early`): the step to the slow OSR (0x17) of MED / SLOW
//   waits until R22_DROP_WIN_MS after the row write, so it no longer drops two
//   400 ms conversions (CONF -> READ? DC V at MED: 1.5-1.8 s on 25b6ec3). Until
//   then the readings come at OSR_EARLY (100 ms, unaveraged); ohm's in-range
//   OSR is OSR_EARLY in the window (two 100 ms conversions dropped), the steady
//   one after it. That a step after the window is clean is INFERRED from the
//   device's input steps later in a function (osrstep.py, v0.1-pre1).
const OVER_TWICE: bool = true;
const R22_DROP_N: u8 = 2;
const R22_DROP_WIN_MS: u64 = 2000;

// Plain AC A after a row write (entry, range or shunt step): the RMS block
// carries the switch step for ~1 s on the device (v0.1-pre1: 10 A -> 60 mA,
// open jack, 6 of 6 first readings 28.7-59.5 mA, in amber and to FETCh?,
// then 1 / 100 per result down to ~1e-6 A). A result is used once it agrees
// with the one before on the row within ACA_VET_REL of it plus ACA_VET_FS of
// the range (a decaying step does not: it falls ~100x per result); until
// then it is provisional (grey). ACA_VET_MAX_MS after the row write a result
// is used anyway (a noisy real signal).
const ACA_VET: bool = true;
const ACA_VET_REL: f32 = 0.05;
const ACA_VET_FS: f32 = 0.002;
const ACA_VET_MAX_MS: u64 = 2000;
// Plain AC V after a range step: OP1's gain changes (6 / 60 V share a tap),
// so its output's DC steps and the coupling into PB4 (tau 0.34 s,
// docs/hy3131-frontend-2026-10-26.md 2) carries the step to AD2, whose RMS
// block sees it: device 0.2.1, the 0 / 9.5 V 50 / 20 Hz square, LPF 1.33,
// 0.99, 0.74, 0.55 ... V over the first results after 60 -> 6 V with RMS =
// sqrt(AC^2 + LPF^2) within ~0.1 V (first finals +3.6..+4.5 %), -25 ... V
// after 6 -> 60 V (21-26 V for 4.84 V). A result is provisional (grey, no
// range move down) while |LPF| > ACV_DC_K x the RMS + ACV_DC_FS x the range
// (its share of the RMS then < 0.15 %), at most ACA_VET_MAX_MS.
const ACV_DC_K: f32 = 0.05;
const ACV_DC_FS: f32 = 0.005;

/// Ohm entry (switch-latency.md proposal 2): the stock's button entry writes
/// no settle deadline; its `now + 2500` at case 10 (app_decompile.c:16135)
/// is S+0x70, the end of the window in which the range steps down one at a
/// time (OHM_SINGLE_MS, `poll_ohm`), so it hunts down from 60 Mohm one range
/// per ~150 ms from the start (emulator: 4.7 kohm at 1.15 s). opendm40 entered
/// with a 2500 ms deadline instead: no conversion for 2.5 s, and the single
/// step window ended with it (3.1 s emulator, 2.7 s device). The entry
/// now waits as a range write does.
const OHM_ENTRY_MS: u64 = 150;

/// V / A entry by a function button or SCPI / BT `select` (switch-latency.md
/// proposal 3): the stock's button entry writes no deadline either; its DMM
/// screen polls at the change and 300 ms later (FUN_0803cdd8), so its first
/// value comes after ~300 ms plus one conversion. The 450 / 650 ms of its
/// S+0x40 table are its in-function row writes (autorange unit changes,
/// AC+DC phases: unchanged here, as is the AC+DC / AUTO entry's first DC
/// phase in `set_sub`; its plain DC / AC entry is a button entry too). The
/// relays switch inside write_config (10 ms pulse).
const ENTRY_MS: u64 = 300;

/// Manual V range change (MF page, SCPI `<f>:RANGe`, `set_range`): the
/// stock's range-write deadline (0xfa); the first conversion is still vetted.
const RANGE_MS: u64 = 250;
const OHM_SINGLE_MS: u64 = 2500;

/// Ohm vet (device 6bc494a new bug 1): after every ohm row write (the
/// entry, each autorange step, a manual range) a conversion is final only
/// when the one before it on the same row agrees with it within
/// OHM_VET_REL plus OHM_VET_CNT counts of the range (60 000 counts; two OL
/// agree); until then it is provisional (grey, `Reading::prov`: no SCPI,
/// BT, statistics or limits). The device showed 8.8 / 7.1 / 12 Mohm on the
/// high ranges and 219 / 126 / 57 / 30 ohm on the right one, in amber and
/// to FETCh? / READ?, before 390 / 100 ohm (input / source settling after
/// a range step). Once a row is vetted its conversions stay final.
const OHM_VET_REL: f32 = 0.001;
const OHM_VET_CNT: f32 = 8.0;
/// ... and a row whose conversions never agree that closely (noise above
/// 0.1 %, e.g. a high range at FAST, a source on the jacks) is not grey
/// forever: a conversion OHM_VET_MAX_MS or more after the row write is
/// final whatever the one before (as VET_HELD_MAX for V / A; a time, not a
/// count, so FAST's 100 ms conversions get as long as MED's 400 ms ones to
/// settle: 1.5 s is 15 / 4 conversions).
const OHM_VET_MAX_MS: u64 = 1500;

/// Ohm ranges that disagree (device 1a11eda, the calibrator's disabled
/// output: 6 Mohm read 0.104 Mohm, 600 kohm OL, and AUTO stepped between
/// them for good). A step down puts the input on a range with ten times
/// the test current (docs/data/short.csv: 0.215 uA on 6 Mohm, 2.17 uA on
/// 600 kohm); an input that is not a resistor (INFERRED here: a path that
/// takes at most ~0.2 uA, e.g. a leakage / bias current of a source's
/// output stage; emulator `isink:`) reads in range at the small current
/// and OL at the larger one. So: an OL on a row reached by a step down,
/// before any in-range conversion of it, that the next conversion confirms
/// (the vet: two OL agree), steps back to the range it came from, and that
/// range then stays (no step down) while it reads within OHM_BAND of the
/// value it stepped down on. A real change of the input (outside the band,
/// OL) and any other row write (function, manual range, AUTO key) end it.
const OHM_BAND: f32 = 2.0;

// AUTO coupling (sub-mode 3, TODO "Auto AC/DC"; stock/re/docs/history/acdc-stock-vs-opendm40.md
// section 11). The AC+DC alternation measures both parts; AUTO shows one of
// them: the AC part's RMS when it dominates the DC part, else the DC part.
//   to AC: AC > AUTO_TO_AC x |DC| and AC > the range's floor (on)
//   to DC: AC < AUTO_TO_DC x |DC| or  AC < the range's floor (off)
// In between nothing changes (hysteresis: a factor 2 band around AC ~ |DC|;
// a half-wave rectified sine, AC / DC = 1.21, stays on whatever it showed).
// The floor is a fraction of the range's full scale (`lim`): V 1 % on /
// 0.5 % off, A 3 % / 2 % (the uA path's RMS floor is ~1.5 %, acdc doc 6b),
// so open leads / no current read DC. Hum on open 10 Mohm V leads is a real
// AC signal and reads as AC above the floor (no LoZ here, feature HW-6).
// Evaluated once per AC+DC cycle (~1.9 s, at the end of the AC phase, both
// parts fresh); a change needs AUTO_VOTES cycles in a row (2: ~2..4 s after
// the signal changed). The first cycle after a reset decides at once (no
// votes): the larger part. It is not taken on a transient, because the
// hysteresis keeps it: the first AC parts after an entry overshoot (A's AC
// phase: +3 % on the 40 Hz square, device ac1e852, which then showed AC
// while C showed DC; +7 % on 33b44bd; up to +52 % in the emulator after the
// up-range; C's ~0.5 % high), and on a tie (that square: AC 4.760, DC
// 4.764 V) the overshoot alone decided. So the first decision waits until
// the AC part has settled on one side: its change since the evaluation
// before, x AUTO_SETTLE_K (an EMA's remaining drift), is less than its
// distance to |DC| (a clear case: the second evaluation, A ~2 s, C ~0.1 s
// later; an AC part below the floor: DC at once). A tie (AC within AUTO_TIE
// of |DC|, where the parts' noise could pick either side: ~0.03 % phase to
// phase in A, C's LPF ripple of a 40 Hz square +-0.3 % in the emulator) and
// one that does not settle within AUTO_TIE_MS are DC. Until then the
// reading shows DC, AC only with no DC part (|DC| below the off floor).
// Either part is fair at a tie; it no longer depends on the transient.
const AUTO_TO_AC: f32 = 1.5;
const AUTO_TO_DC: f32 = 0.75;
const AUTO_FLOOR_V: [f32; 2] = [0.01, 0.005];
const AUTO_FLOOR_A: [f32; 2] = [0.03, 0.02];
const AUTO_VOTES: u8 = 2;
const AUTO_SETTLE_K: f32 = 4.0;
const AUTO_TIE: f32 = 0.01;
const AUTO_TIE_MS: u64 = 4000;

/// The AC settle's "flat" test (ACS_STEP) of result `ac` after `prev` (< 0:
/// none), `d_prev` = the fall before (< 0: unknown), `near` = the two in the
/// same AC phase. A rise is flat only within ACS_STEP too: device check
/// ad97fd7 (DBG:ACDC:TRACE, calibrator 4.0 -> 5.8 V, its level change dips
/// to 2-4 V for ~0.2 s): the AC results of the step rose 0.080 -> 1.166 ->
/// 2.384 V as the RMS window filled, the "rose: flat" rule ended the
/// settle on them, and hypot(2.384, 5.8) moved AC+DC autorange 6 -> 60 V
/// and back 5 s later.
fn acs_flat(prev: f32, d_prev: f32, ac: f32, near: bool) -> bool {
    let d = prev - ac;
    if prev < 0.0 || d.abs() > ACS_STEP * ac {
        return false;
    }
    if near && d > 0.0 && d_prev > 0.0 {
        // still falling, as the result before: what is left of the decay
        let q = d / d_prev;
        return q < 1.0 && d * q <= ACS_REM * ac * (1.0 - q);
    }
    true
}

/// SCPI / persisted function codes of the AUTO coupling (`Dmm::func`), next
/// to 16 / 17 (AC+DC) and pw::FUNC (18).
pub const FUNC_V_AUTO: u8 = 19;
pub const FUNC_A_AUTO: u8 = 20;

/// AC+DC diagnostic: the RMS results of one AC phase (up to 4, the stock V
/// phase length), and when the first one came after the settle deadline.
#[derive(Clone, Copy)]
pub struct AcPhaseDiag {
    /// ms between the settle deadline and the first result.
    pub late_ms: u16,
    /// Results recorded (1..=4).
    pub n: u8,
    /// sqrt(RMS) raw counts (the stock S+0x50).
    pub sq: [u32; 4],
    /// LPF raw (19-bit signed, the stock S+0x4c).
    pub lpf: [i32; 4],
    /// LPF scaled = the DC seen by the AC path (V / mV / A unit of the
    /// phase), the stock S+0x54. Stock, 40 Hz 0..9.63 V PWM at r1: 0.07..0.10
    /// V at the first result; 0.3..0.8 V = the coupling node still settling.
    pub dcp: [f32; 4],
    /// ms since the previous result (first of an AC+DC phase: since the
    /// phase switch, i.e. settle + late_ms).
    pub dt: [u16; 4],
    /// INTF as read before the result, bit 8 set when the frequency counter
    /// was restarted (R20 / CTA writes) since the previous result.
    pub fl: [u16; 4],
}

impl AcPhaseDiag {
    const EMPTY: AcPhaseDiag = AcPhaseDiag { late_ms: 0, n: 0, sq: [0; 4], lpf: [0; 4], dcp: [0.0; 4], dt: [0; 4], fl: [0; 4] };
}

impl Dmm {
    pub fn new() -> Self {
        hy::init_pins();
        // Stock rule (docs/model-variants.md): calibration byte 0, clamped to
        // 2; a record the stock would reject is replaced by its defaults
        // (model 0 = DM40A). opendm40 checks the version word only (cal_ok).
        let model = if cal_ok() { Self::model_raw().min(2) } else { 0 };
        Dmm {
            model,
            mode: 1,
            acdc: false,
            phase_n: 0,
            dc_val: 0.0,
            ac_raw: 0.0,
            acdc_seen: 0,
            acdc_known: 0,
            acs_t0: 0,
            acs_prev: -1.0,
            acs_flat: 0,
            acs_tp: 0,
            acs_d: -1.0,
            acs_dc: f32::NAN,
            acs_step: false,
            vet_left: 0,
            vet_held: 0,
            vet_prev: i32::MIN,
            r22_drop: 0,
            row_t: 0,
            over_prev: false,
            aca_vet_t: 0,
            aca_prev: f32::NAN,
            acv_vet: false,
            prov_side: false,
            down_t: 0,
            rd_pending: false,
            cpl_auto: false,
            auto_ac: false,
            auto_n: 0,
            auto_votes: 0,
            auto_t0: 0,
            auto_prev: 0.0,
            auto_total_si: 0.0,
            range: 0,
            manual: false,
            // UF-21: AUTO+ (V first, up-only) by default; the stock default
            // AUTO (3) steps down to the mV ranges.
            v_auto: 1,
            a_auto: 3,
            ohm_var: 0,
            deadline: 0,
            entry: 0,
            mv_clip: 0.0,
            jwin: 0,
            closed_t: 0,
            r25: 0,
            prev_ad1: 0,
            r22: 0,
            ohm_settled: false,
            ohm_prev: f32::NAN,
            ohm_ok: false,
            ohm_agreed: false,
            ohm_down_r: -1,
            ohm_down_k: 0.0,
            ohm_blk_r: -1,
            ohm_blk_k: 0.0,
            ohm_t0: 0,
            ac_seed: true,
            ac_lead: 2,
            mv_dc_drv: false,
            f_ac: 0.0,
            f_lpf: 0.0,
            f_dc: 0.0,
            f_diode: 0.0,
            f_cap: 0.0,
            cap_c: 0.0,
            cap_cnt_up: 0,
            cap_cnt_dn: 0,
            cap_cnt_small: 0,
            ramp: CapRamp {
                discharging: false,
                prev_v: 0.0,
                prev_t: 0,
                med: [0.0; 5],
                med_i: 0,
                class: 0,
                buf: [0.0; 35],
                n: 0,
                cmp: None,
                first: true,
            },
            beep_class: 4,
            beep_e: 0,
            beep_next: 0,
            beep_n: 0,
            beep_on: false,
            t_phase: 0,
            t_code_a: 0,
            t_cj: 0.0,
            freq: Freq { next: 0, src_alt: false, fresh: false, seen: [false; 2], zc: [0; 2], shown: false, last: [0.0; 2], lastd: [0.0; 2], change: [0.0; 2], f_khz: 0.0, duty: 0.0, range: 0, t0: 0, lastn: [0; 2], n: 0, prev: f32::NAN, conf: [false; 2], prov: false, hf_ok: false, hf_t: 0 },
            seq: 0,
            pa_now: None,
            suspended: false,
            wake_at: 0,
            wd_t: 0,
            wd_poll: 0,
            wd_chk: 0,
            chk: [0; 8],
            wd_n: [0; 3],
            t_sensor: 0,
            t_offs: [0; 7],
            t_f: false,
            ntc_user: (10_000.0, 3950.0),
            t_cj_at: 0,
            t_ema: 0.0,
            t_res: 0.0,
            t_votes: 0,
            avg: Avg::new(),
            avg_skip: false,
            avg_n: 0,
            diag: [AcPhaseDiag::EMPTY; ACDC_DIAG_N],
            diag_k: 0,
            diag_on: false,
            diag_t: 0,
            diag_ctr: 0,
            rate: Rate::Medium,
            fast: fastvos::fast_static(),
            t_poll: 0,
            t_rd: None,
            period_ms: 0.0,
            t_conv: None,
            conv_ms: 0.0,
            rd_avg_n: 0,
            sent: Sentinel::new(),
            sent_mode: 255,
            r29: 0,
            pk_t: 0,
            pkh_due: 0,
            sent_arm: false,
            ohm_exit_t: None,
            precond_ms: PRECOND_MS,
            a_blind: true,
            ac_over_until: 0,
            live: false,
            ovl: None,
            probe: false,
            probe_settle: false,
            aprot: None,
            ovl_rel_t: None,
            fuse_job: None,
            fuse_res: sentinel::FuseRes::Idle,
            pw: pw::pw_static(),
            #[cfg(feature = "acdc-meth")]
            am: am::am_static(),
            tr: tr::tr_static(),
            burden_show: false,
            ohm_neg: false,
            o6: Ohm600::NEW,
        }
    }

    /// 0 DM40A, 1 DM40B, 2 DM40C (identity only: counts are COUNTS_MODEL).
    pub fn model(&self) -> u8 {
        self.model
    }

    /// The calibration model byte as stored (record 0x080C0000 + 0x168).
    pub fn model_raw() -> u8 {
        rec_u32(CAL - SETTINGS) as u8
    }
    /// Function code as seen from outside (= `func()`): the stock mode, or
    /// 16 / 17 in AC+DC. main.rs gates the data logger on this, so LOG:STATe ON
    /// is refused in AC+DC (its HY3131 mode alternates).
    pub fn mode(&self) -> u8 {
        self.func()
    }
    /// The HY3131 mode currently configured (AC+DC: the current phase).
    pub fn hy_mode(&self) -> u8 {
        self.mode
    }
    pub fn range(&self) -> i8 {
        self.range
    }
    /// The frequency counter's range (60 Hz .. 60 MHz, `freq_range_for`).
    pub fn counter_range(&self) -> u8 {
        self.freq.range
    }
    pub fn button(&self) -> Button {
        if self.pw.on { Button::Power } else { button_of(self.mode) }
    }
    pub fn is_auto(&self) -> bool {
        !self.manual
    }
    pub fn v_auto(&self) -> u8 {
        self.v_auto
    }
    pub fn a_auto(&self) -> u8 {
        self.a_auto
    }
    /// Select the ohm variant (stock m+8: 0 normal, 1 "ON" low current,
    /// the p8set rows; r0 always uses the normal row) and re-enter ohm in
    /// the current range, as the stock's second tap on the ohm button.
    pub fn set_ohm_var(&mut self, clk: &mut Clock, v: u8) {
        self.ohm_var = v & 1;
        // 600 Mohm has its own source: the variant waits for the next range
        // (no row rewrite, no restart of its phases).
        if self.mode == 10 && self.range != OHM600 {
            // AUTO on 60 ohm and "ON" chosen: the autorange's floor, 600 ohm.
            let r = self.range.clamp(if self.manual { 0 } else { self.ohm_auto_floor() }, 6);
            self.set(clk, 10, r, 300);
        }
    }

    pub fn ohm_var(&self) -> u8 {
        self.ohm_var
    }

    /// The lowest range of the ohm autorange: r0 (60 ohm) with the default
    /// variant, r1 (600 ohm) with the "ON" one. r0 always drives the default
    /// variant's 0.90 mA (`write_config`), so the autorange stays out of it
    /// when the user chose the 0.28 mA test current; a manual 60 ohm still
    /// works (at 0.90 mA). The stock autorange never goes below r1.
    fn ohm_auto_floor(&self) -> i8 {
        if self.ohm_var == 1 { 1 } else { 0 }
    }
    /// Restore m+6 / m+7 / m+8 (settings): only the values the UI / BT can
    /// set (V, A: 1 = AUTO+, 3 = AUTO; ohm variant 0 / 1); anything else
    /// keeps the default. Takes effect with the next `select`.
    pub fn set_auto_flags(&mut self, v: u8, a: u8, ohm: u8) {
        if matches!(v, 1 | 3) {
            self.v_auto = v;
        }
        if matches!(a, 1 | 3) {
            self.a_auto = a;
        }
        if ohm <= 1 {
            self.ohm_var = ohm;
        }
    }

    /// UF-21: the V autorange is the mV-first AUTO (stock default, `v_auto`
    /// 3) and V is measured in auto range: the UI shows MV_AUTO_WARN (Range
    /// page / status line). AUTO+ (1, the opendm40 default) never warns.
    pub fn mv_auto_warn(&self) -> bool {
        self.button() == Button::Volt && !self.manual && self.v_auto == 3
    }

    // ------------------------------------------------- FR-05 rate / timing

    pub fn rate(&self) -> Rate {
        self.rate
    }

    /// Set the conversion rate (see the FR-05 table). Takes effect with the
    /// next conversion: the code window restarts and `osr` / `poll_ohm`
    /// write the new steady OSR; no settle, no function restart.
    pub fn set_rate(&mut self, r: Rate) {
        if r != self.rate {
            self.rate = r;
            self.avg.reset();
            self.t_rd = None;
            self.period_ms = 0.0;
        }
    }

    /// What `rate` gives in the current function and range: (code window
    /// in conversions, 1 = none; the +1 digit is shown). See the FR-05 and
    /// +1 digit tables (`rate_window`).
    pub fn rate_info(&self, rate: Rate) -> (usize, bool) {
        rate_window(rate, self.acdc, self.mode, self.range)
    }

    /// AD1 conversion period of `rate` in the current function, ms
    /// (`rate_period`).
    pub fn rate_period_ms(&self, rate: Rate) -> u32 {
        rate_period(rate, self.mode)
    }

    /// The rate setting changes something in the current function (DC V /
    /// mV / A, ohm; not AC+DC): the UI can grey the row out otherwise.
    pub fn rate_applies(&self) -> bool {
        // 600 Mohm runs its own fixed 200 ms conversions.
        !self.acdc && matches!(self.mode, 1 | 3 | 7..=10) && !is_ohm600(self.mode, self.range)
    }

    /// Measured mean interval between readings (ms, EMA over the last few;
    /// intervals across a function / range / rate change or a dropped
    /// conversion are not counted). 0 until two readings were seen. Use for "x readings/s" in the UI and SCPI.
    pub fn reading_period_ms(&self) -> u32 {
        self.period_ms as u32
    }

    /// The AD1 R22 of the rate: DC V / mV / A (`dc_r22`), ohm in range.
    fn steady_osr(&self) -> u8 {
        if self.dc_rated() {
            self.dc_r22()
        } else if self.rate == Rate::Fast {
            R22_MED
        } else {
            OSR_SLOW
        }
    }

    /// DC V / mV / A outside AC+DC: the rate's R22 goes out in the row
    /// (`write_config`), so no R22 write follows a row write (R22_DROP).
    fn dc_rated(&self) -> bool {
        !self.acdc && matches!(self.mode, 1 | 3 | 7..=9)
    }

    /// The rate's R22 in DC V / mV / A (the FR-05 table).
    fn dc_r22(&self) -> u8 {
        match self.rate {
            Rate::Fast => fastvos::R22_FAST,
            Rate::Medium => R22_MED,
            Rate::Slow => OSR_SLOW,
        }
    }

    /// DC V / mV / A: a rate changed since the row (`set_rate`) is written
    /// after the next conversion (an OSR change restarts AD1).
    fn dc_rate_osr(&mut self) {
        let want = self.dc_r22();
        if self.r22 != want {
            hy::write(0x22, &[want]);
            self.r22 = want;
            self.avg_skip = true;
            self.r22_dropping();
            if let (fastvos::R22_FAST, Some(k)) = (want, Fast::path(self.mode, self.range)) {
                self.fast.start(k, self.t_poll);
            }
        }
    }

    /// FAST (R22_FAST) in DC V / mV / A: the conversion with the converter
    /// offset and the unchopped gain taken out (fastvos.rs), and the
    /// refresh's AD1CHOP switches; None: a conversion mixed by one. Else
    /// the code as it is.
    fn fast_code(&mut self, ad1: i32) -> Option<i32> {
        if self.r22 != fastvos::R22_FAST || !self.dc_rated() {
            return Some(ad1);
        }
        let (o, w) = self.fast.conv(ad1, self.t_poll, fastvos::THR);
        if let Some(r) = w {
            hy::write(0x22, &[r]);
        }
        match o {
            // Not a reading: the interval across it is not counted either,
            // so the readings/s (`reading_period_ms`, the rate chip) stays
            // the conversion rate (with the gap, one dropped conversion
            // read 125 ms and the chip "8.0/s" .. "8.8/s" for ~1 s after
            // each refresh: device check of 0.2.1, emulator rateprobe).
            fastvos::Out::Drop => {
                self.t_rd = None;
                None
            }
            fastvos::Out::Code(c) => Some((c + if c < 0.0 { -0.5 } else { 0.5 }) as i32),
        }
    }

    /// FAST's offset refreshes this session: (used, rejected, VOS in codes
    /// on the current path, AD1 conversion period in ms: EMA over every
    /// conversion, dropped ones included) (DBG:FAST?).
    pub fn fast_info(&self) -> (u32, u32, f32, f32) {
        (self.fast.used, self.fast.rejected, self.fast.vos(), self.conv_ms)
    }
    fn factor(&self) -> f32 {
        FACTOR[COUNTS_MODEL as usize]
    }
    fn over(&self) -> f32 {
        OVER[COUNTS_MODEL as usize]
    }

    // ---------------------------------------------------------------- setup

    /// FUN_08012c30: quiesce, relays, 22 config bytes, INTE = 0.
    fn write_config(&mut self, clk: &mut Clock) {
        if self.pw.on {
            return self.pw_write_config(clk);
        }
        // AC+DC method B / C (acdcm.rs): the step's row, no sentinel.
        let am = self.am_on();
        if am {
            self.am_mode();
        }
        let m = self.mode;
        let r = self.range.max(0) as usize;
        let cfg: &[u8; hy::CFG_LEN] = match m {
            0 => &CFG_F0[r.min(3)],
            1 => &CFG_F1[r.min(3)],
            2 => &CFG_F2[r.min(1)],
            3 => &CFG_F3[r.min(1)],
            4 => &CFG_F4[r.min(1)],
            5 | 6 => &CFG_F5_6[r.min(1)],
            7 => &CFG_F7[r.min(1)],
            8 | 9 => &CFG_F8_9[r.min(1)],
            // r0 (60 ohm) always with the higher-current default variant.
            10 => {
                if r == OHM600 as usize {
                    self.o6.restart();
                    &OHM600_ROW
                } else if r == 0 { &OHM_R0_ROW } else if self.ohm_var == 1 { &CFG_F10_P8SET[r.min(6)] } else { &CFG_F10_P8CLR[r.min(6)] }
            }
            11 => &CFG_F11[0],
            12 => &CFG_F12[0],
            13 => {
                if r == 7 { &CFG_F13_R7 } else { &CFG_F13[r.min(6)] }
            }
            14 => &CFG_F14[0],
            _ if self.t_sensor != 0 => &CFG_F10_P8CLR[r.clamp(1, 6)],
            _ => &CFG_F15[0],
        };
        // DBG:SRC (dbgsrc_dmm.rs): its row in place of the stock one.
        let src_row = src::row(m, r);
        let cfg = src_row.as_ref().unwrap_or(cfg);
        self.avg.reset();
        self.t_rd = None;
        self.t_conv = None;
        self.ac_over_until = 0;
        let pa = if matches!(m, 0 | 1 | 4..=9) { 0 } else { 1 };
        let pre = self.precond_ms != 0
            && self.a_blind
            && matches!(m, 7..=9)
            && !am
            && self.func() != self.sent_mode
            && self.sent.mask & sentinel::M_VJACK != 0
            && self.fuse_job.is_none();
        if pre {
            self.precondition(clk);
        } else if m == 15 && self.t_sensor == 0 {
            self.a_blind = false;
        } else if pa == 1 || (m == 1 && r > 0) {
            self.a_blind = true;
        }
        hy::quiesce();
        if self.pa_now == Some(1) && pa == 0 && !pre {
            self.ohm_exit_t = Some(clk.now_ms());
        }
        // AC+DC: the stock pulses the relay on every FUN_08012c30 call, so
        // every phase switch has this 10 ms pause (AD1 / AD2 off) between the
        // quiesce and the new row (ACDC_RELAY_PULSE).
        if self.pa_now != Some(pa) || (ACDC_RELAY_PULSE && self.acdc) {
            relay_pa(clk, pa);
            self.pa_now = Some(pa);
        }
        // A-jack protection (doc 4.8): a current function ends the latch
        // (its own shunt switching / OVERLOAD take over); while latched the
        // other functions keep the A jack on the 10 mohm shunt.
        if matches!(m, 4..=9) && self.aprot.take().is_some() {
            self.sent.set_aprot(clk.now_ms(), false);
        }
        relay_pe(match m {
            4 | 7 => 0,
            5 | 8 => 1,
            _ if self.aprot.is_some() => 0,
            _ => 2,
        });
        pc7(m == 14);
        // Misconnect sentinel: the stock row with the idle comparator / AD1
        // pointed at the A-jack shunt (or the V tap in DC A), sentinel.rs.
        let mask = if self.fuse_job.is_some() || src_row.is_some() { 0 } else { self.sent.mask };
        let (row, kind) = if am { (self.am_row(cfg), Kind::Off) } else { sentinel::sentinel_row(m, r as u8, self.acdc, mask, cfg) };
        // The probe (PB2) over the comparator rows after a hit, and while
        // latched in every DC V / mV / ohm / temperature row (OP2 / AD2 are
        // unused there; DC V 60 V+ has no comparator but the probe fits).
        // Ohm / the resistive temperature sensors: the same entry carries the
        // jack check's row instead (sentinel JACK CHECK), never for A-PROT.
        let probe_ok = self.probe_ok();
        let jack = self.jack_mode();
        self.probe = probe_ok && ((self.probe && kind == Kind::CmpA) || (self.aprot.is_some() && !jack));
        let row = if !self.probe {
            row
        } else if jack {
            sentinel::jack_row(&row)
        } else {
            sentinel::probe_row(&row)
        };
        // FR-05: DC V / mV / A at the rate's R22 from the row on. A row at
        // the R22 the chip runs does not restart AD1, and its decimation
        // filter keeps samples of the old divider tap (the VET_* device
        // case: two quick autorange steps at one OSR read 0.77 V for 1 V);
        // with every DC row at one R22 that is every range step, so such a
        // row enables AD1 last: ENAD1 0 -> 1 restarts it (emulator
        // autorangetest from 1000 V: 0.84 V without).
        let mut row = row;
        let dc_rows = self.dc_rated() && !am && src_row.is_none();
        if dc_rows {
            row[2] = self.dc_r22();
        }
        let restart = dc_rows && row[2] == self.r22 && row[3] & 0x80 != 0;
        self.jwin = 0;
        if sentinel::ad1_last(kind, &row) || ((self.probe || am || restart) && row[3] & 0x80 != 0) {
            let mut c = row;
            c[3] &= !0x80;
            hy::configure(&c);
            hy::write(0x23, &[row[3]]);
        } else {
            hy::configure(&row);
        }
        // A row with the counter on (R20 ENCTR): preset the gate. ENCTR
        // 0 -> 1 alone starts it from CTA<23:8> as the last gate left it
        // (~0 after its overflow; 0 after a POR): a ~3.4 s gate. AC+DC is
        // left alone (its phases keep the stock's 350 ms restarts).
        let ctr = row[0] & 0x02 != 0 && !self.acdc;
        if ctr {
            ctr_restart(row[0]);
        }
        self.freq.fresh = ctr;
        self.freq.hf_ok = false;
        self.r22 = row[2];
        if let (true, Some(k)) = (self.r22 == fastvos::R22_FAST && self.dc_rated(), Fast::path(m, self.range)) {
            self.fast.start(k, clk.now_ms());
        }
        self.r29 = row[9];
        self.chk.copy_from_slice(&row[CHK_AT..CHK_AT + 8]);
        if tr::ON && self.tr.on != 0 {
            let h = tr::hash16(&row);
            self.tr_ev(clk.now_ms(), tr::K_ROW, h as i32, self.dc_val, 0);
        }
        // The sentinel's history belongs to the function (AC+DC: both
        // phases, `func`), so a phase switch or a range change keeps it.
        let key = self.func();
        let fresh = self.sent_mode != key;
        self.sent_mode = key;
        self.pk_t = clk.now_ms();
        self.pkh_due = 0;
        self.sent_arm = true;
        self.probe_settle = self.probe;
        self.sent.configured(kind, clk.now_ms(), fresh);
        self.sent.blank(clk.now_ms(), sentinel::BLANK_ROW_MS);
        // Leaving the ohm network into a V-JACK? comparator row (sentinel::OHM_EXIT_*).
        // Every row within OHM_EXIT_MAX_MS of it (DC A, its range steps, the
        // AC+DC / AUTO phases): the windows count from the exit.
        let now = clk.now_ms();
        self.sent.ohm_exit(self.ohm_exit_t.filter(|&t| kind == Kind::CmpV && now < t + sentinel::OHM_EXIT_MAX_MS));
        // Ohm / temperature: A-JACK needs the V side's word (sentinel V_GATE).
        // AC mV: its own result says how far beyond the range the input is
        // (sentinel MV_DRV_K), a block late (MV_VGATE_LAG_MS). AC+DC mV: the
        // AC phase is that row and its sentinel; both phases share the run
        // and the AC phase's verdict (doc 4.13). Plain DC mV: its own result
        // over the range (the readout's OL) = driven, also for A-PROT
        // (sentinel V_HF_KHZ: a 66 kHz signal on V-COM).
        let mv = self.mode == 2 || (self.acdc && self.mode == 3);
        let dcmv = self.mode == 3 && !self.acdc;
        // Continuity / diode: V_GATE on OL (`CLOSED_HOLD_MS`).
        self.sent.vgate = matches!(self.mode, 10 | 11 | 12 | 15) || mv || dcmv;
        self.closed_t = 0;
        self.sent.vlag = if mv {
            sentinel::MV_VGATE_LAG_MS
        } else if dcmv {
            sentinel::DCMV_VGATE_LAG_MS
        } else if matches!(self.mode, 11 | 12) {
            CLOSED_VGATE_LAG_MS
        } else {
            0
        };
        // The jack check (sentinel JACK CHECK): ohm, continuity, the
        // resistive temperature sensors.
        self.sent.jchk = !self.acdc && (matches!(self.mode, 10 | 11) || (self.mode == 15 && self.t_sensor != 0));
        self.sent.bridge = self.acdc;
        // A-JACK evidence a driven V-COM can fake (sentinel `soft`): the
        // ohm-network rows (ohm, continuity, diode, capacitance, the
        // resistive temperature sensors' ohm rows); frequency (its PB1
        // follows the input path; its thresholds sit 1.3-2 x over the V-side
        // levels measured, PK_DC_THR / PK_DC_NEG_THR).
        self.sent.soft = matches!(self.mode, 10..=14) || (self.mode == 15 && self.t_sensor != 0);
        self.sent.pk_dc = self.mode == 14;
        // DC? / AC? hints (doc 4.4): AC V alternates AD1 between PB1 - PB3
        // and the V tap, DC V 6 V alternates the comparator between PB1 and
        // SENSE, one 100 ms window each (`poll`, `poll_ac`).
        self.sent.alt = self.sent.mask & sentinel::M_HINTS != 0
            && sentinel::variant() & sentinel::VAR_HINTS_OFF == 0
            && !self.acdc
            && matches!((m, kind, r), (0, Kind::PeakA, _) | (1, Kind::CmpA, 0));
        if m == 15 && self.t_sensor != 0 {
            // Resistive sensor: the stock in-range ohm OSR (32768) for a quiet
            // reading; the range is fixed, so no fast open-circuit switching.
            hy::write(0x22, &[0x17]);
            self.r22 = 0x17;
        }
        // AC+DC DC phase: at the row's OSR 8192 (see ACDC_STOCK_OSR); every
        // result after the settle deadline is used, none dropped
        // (stock/re/docs/history/acdc-stock-vs-opendm40.md 8.2).
        if !ACDC_STOCK_OSR && self.acdc && matches!(m, 1 | 3 | 7..=9) {
            hy::write(0x22, &[0x17]);
            self.r22 = 0x17;
        }
        // AC+DC diagnostic: log the RMS results of an AC phase (plain AC V:
        // every 4 results as one entry, for comparison).
        self.diag_on = matches!(m, 0 | 2 | 4..=6) && (self.acdc || m == 0);
        if self.diag_on {
            self.diag_k = self.diag_k.wrapping_add(1);
            self.diag[self.diag_k as usize % ACDC_DIAG_N] = AcPhaseDiag::EMPTY;
            self.diag_t = clk.now_ms();
            self.diag_ctr = 0;
        }
    }

    /// AC+DC diagnostic: the last AC phases (oldest first) and the number of
    /// AC phases started so far. The entry for the running phase may hold
    /// fewer than its 4 (V) / 3 (mV, A) results.
    #[cfg_attr(not(feature = "dbg-acdc"), allow(dead_code))]
    pub fn acdc_diag(&self) -> (u32, [AcPhaseDiag; ACDC_DIAG_N]) {
        let mut d = [AcPhaseDiag::EMPTY; ACDC_DIAG_N];
        let n = (self.diag_k as usize).min(ACDC_DIAG_N);
        for (i, e) in d.iter_mut().take(n).enumerate() {
            *e = self.diag[(self.diag_k as usize + 1 + i + ACDC_DIAG_N - n) % ACDC_DIAG_N];
        }
        (self.diag_k, d)
    }

    /// `acdc_diag` as text for a debug query:
    /// `phases;late:sq/lpf/dcp/dt/intf,...;...` (oldest phase first, dcp with
    /// 4 decimals in the phase's unit, dt ms since the previous result, intf
    /// hex with a `*` when the frequency counter was restarted since the
    /// previous result), e.g. `41;2:9926/183/0.0891/802/1a,...`. Plain AC V
    /// logs every 4 results as one entry. Returns the length used; the
    /// oldest phases are left out when `buf` is too small.
    #[inline(never)] // .cold (link-native.x): DBG reply
    #[cfg_attr(not(feature = "dbg-acdc"), allow(dead_code))]
    pub fn acdc_diag_text(&self, buf: &mut [u8]) -> usize {
        fn put(buf: &mut [u8], at: &mut usize, b: &[u8]) -> bool {
            let ok = *at + b.len() <= buf.len();
            if ok {
                buf[*at..*at + b.len()].copy_from_slice(b);
                *at += b.len();
            }
            ok
        }
        fn num(buf: &mut [u8], at: &mut usize, v: f32, dec: u8) -> bool {
            let mut t = [0u8; 16];
            put(buf, at, format(&mut t, v, dec).as_bytes())
        }
        fn hex(buf: &mut [u8], at: &mut usize, v: u16) -> bool {
            const H: &[u8; 16] = b"0123456789abcdef";
            let b = (v & 0xFF) as usize;
            put(buf, at, &[H[b >> 4], H[b & 15]]) && (v & 0x100 == 0 || put(buf, at, b"*"))
        }
        let (k, d) = self.acdc_diag();
        let n = d.iter().filter(|e| e.n > 0).count();
        for skip in 0..=n {
            let mut at = 0;
            let mut ok = num(buf, &mut at, k as f32, 0);
            for e in d.iter().filter(|e| e.n > 0).skip(skip) {
                ok = ok && put(buf, &mut at, b";") && num(buf, &mut at, e.late_ms as f32, 0) && put(buf, &mut at, b":");
                for j in 0..e.n as usize {
                    ok = ok
                        && (j == 0 || put(buf, &mut at, b","))
                        && num(buf, &mut at, e.sq[j] as f32, 0)
                        && put(buf, &mut at, b"/")
                        && num(buf, &mut at, e.lpf[j] as f32, 0)
                        && put(buf, &mut at, b"/")
                        && num(buf, &mut at, e.dcp[j].clamp(-1e9, 1e9), 4)
                        && put(buf, &mut at, b"/")
                        && num(buf, &mut at, e.dt[j] as f32, 0)
                        && put(buf, &mut at, b"/")
                        && hex(buf, &mut at, e.fl[j]);
                }
            }
            if ok {
                return at;
            }
        }
        0
    }

    fn set(&mut self, clk: &mut Clock, mode: u8, range: i8, settle: u64) {
        let changed_mode = mode != self.mode || self.pw.on;
        let old = (self.mode, self.range);
        // The R22 (AD1 OSR) the chip runs now (R22_DROP).
        let r22_was = hy::read1(0x22);
        self.pw_leave();
        self.mode = mode;
        self.range = range;
        self.write_config(clk);
        // A function / range change: hits before it say nothing about the
        // new row (sentinel::Sentinel::new_run; an alert in force stays).
        self.sent.new_run();
        let now = clk.now_ms();
        self.deadline = now + settle;
        self.acv_vet = false;
        if changed_mode {
            self.entry = now;
            self.reset_values();
            if !matches!(mode, 0 | 2) {
                self.mv_clip = 0.0;
            }
        } else if old != (mode, range) && !self.acdc && !self.cpl_auto {
            // A plain range step: the AC filters must not carry the other
            // range's value (device d2aff22: AC A 10 A -> 60 mA read 1 %
            // of the 10 A value first, the EMA's 0.99 step).
            self.f_ac = 0.0;
            self.f_lpf = 0.0;
            self.ac_raw = 0.0;
            self.ac_seed = true;
            self.ac_lead = if mode > 2 { 0 } else { 2 };
            self.acv_vet = mode == 0;
        }
        self.ac_settle_start(now, old);
        self.vet_start(old);
        self.r22_drop = if r22_was != self.r22 { R22_DROP_N } else { 0 };
        self.row_t = now;
        self.over_prev = false;
        self.aca_vet_t = now.max(1);
        self.aca_prev = f32::NAN;
        self.ohm_prev = f32::NAN;
        self.ohm_ok = false;
        self.ohm_agreed = false;
        self.ohm_t0 = now;
        self.ohm_down_r = -1;
        self.ohm_blk_r = -1;
        self.down_t = 0;
        self.tr_ev(now, tr::K_SET, (old.0 as i32) << 8 | old.1 as u8 as i32, settle as f32, 0);
    }

    /// AC+DC parts known since the last reset (`acdc_known`).
    pub(super) fn parts_known(&mut self) -> u8 {
        self.acdc_known |= self.acdc_seen;
        self.acdc_known
    }

    /// Step down on this DC result (`want`)? Plain DC: at once; AC+DC: once
    /// the DC results asked for it for ACDC_DOWN_MS in a row, and only with
    /// the AC part known since the reset (`acdc_down_ok`).
    fn acdc_down(&mut self, now: u64, want: bool) -> bool {
        if !self.acdc {
            return want;
        }
        if !acdc_down_ok(self.parts_known(), 1) || !want {
            self.down_t = 0;
            return false;
        }
        if self.down_t == 0 {
            self.down_t = now.max(1);
        }
        now - self.down_t >= ACDC_DOWN_MS
    }

    /// AC+DC trace event (acdctr.rs) with the vetting / phase state; a DC
    /// conversion (`K_DC`) also reads the row back when the trace asks.
    fn tr_ev(&mut self, now: u64, k: u8, ad1: i32, val: f32, fl: u8) {
        if !self.acdc || !self.tr.live() {
            return;
        }
        let mut e = TrEnt {
            t: now as u32,
            k,
            mode: self.mode,
            range: self.range,
            r22: self.r22,
            r22c: 0,
            r24c: 0,
            r32c: 0,
            vl: self.vet_left,
            vh: self.vet_held,
            pn: self.phase_n,
            seen: self.acdc_seen,
            fl,
            hash: 0,
            ad1,
            val,
        };
        if self.tr.on == 2 && k == tr::K_DC {
            let mut b = [0u8; hy::CFG_LEN];
            hy::read(hy::R_CFG, &mut b);
            e.r22c = b[2];
            e.r24c = b[4];
            e.r32c = b[18];
            e.hash = tr::hash16(&b);
        }
        self.tr.push(e);
    }

    /// A range or function change from `old` (mode, range; AC+DC: either
    /// phase of it): vet the next DC conversions (VET_*), and READ? / FETCh?
    /// have no reading until the first one of the new range.
    fn vet_start(&mut self, old: (u8, i8)) {
        if (dc_mode_of(old.0), old.1) != (dc_mode_of(self.mode), self.range) {
            self.vet_left = if self.acdc { VET_N_ACDC } else { VET_N };
            self.vet_held = 0;
            self.vet_prev = i32::MIN;
            self.rd_pending = true;
        }
    }

    /// Vet one DC conversion (`vet_start`): true = use it.
    fn vet_ad1(&mut self, ad1: i32) -> bool {
        if self.vet_left == 0 || self.am_on() {
            return true;
        }
        let p = self.vet_prev;
        self.vet_prev = ad1;
        if p != i32::MIN && (ad1 - p).abs() <= VET_FLOOR.max(p.abs() / VET_DIV) {
            self.vet_left -= 1;
            return true;
        }
        self.vet_held += 1;
        if self.vet_held > VET_HELD_MAX {
            self.vet_left = 0;
            return true;
        }
        false
    }

    /// A range or function change happened and no reading of it came yet:
    /// READ? / FETCh? answer +9.91E37 ("no reading") rather than the last
    /// reading from before it (main.rs; device decaf60: READ? held a
    /// transient of 3.39 V for 5 s through an AC+DC range step).
    pub fn reading_pending(&self) -> bool {
        self.rd_pending
    }

    /// `reading_pending` after a range step since the function entry (an
    /// autorange step, a manual range), not the entry itself: FETCh? then
    /// answers +9.91E37 at once also before the function's first reading
    /// (main.rs).
    pub fn range_moving(&self) -> bool {
        self.rd_pending && self.row_t > self.entry
    }

    /// `r` was measured on the row the chip runs now (mode, AC+DC: either
    /// phase, and range): FETCh? never answers a reading from another
    /// range or function (device d2aff22: FETC? gave the 10 A value for
    /// 0.9 s after CONF:CURR:AC 0.06). The power meter and frequency (its
    /// readings carry the counter's range, not the row's): always.
    pub fn reading_current(&self, r: &Reading) -> bool {
        self.pw.on || self.mode == 14 || (dc_mode_of(r.mode) == dc_mode_of(self.mode) && r.range == self.range)
    }

    /// AC+DC / AUTO: a range or unit change from `old` (mode, range) of the
    /// same quantity (V or A; not the entry from another function, nor an
    /// AC <-> DC phase switch) starts the AC part's settle (ACS_STEP).
    fn ac_settle_start(&mut self, now: u64, old: (u8, i8)) {
        let q = |m: u8| match m {
            0..=3 => 1,
            4..=9 => 2,
            _ => 0,
        };
        let (om, nm) = (dc_mode_of(old.0), dc_mode_of(self.mode));
        if self.acdc && !self.pw.on && q(om) != 0 && q(om) == q(nm) && (om, old.1) != (nm, self.range) {
            self.acs_begin(now, false);
            self.acs_dc = f32::NAN;
            // AC+DC: no reading until the AC part is back, also in the DC
            // phase in between (the AC part held from the old range belongs
            // to a signal that may have moved: an autorange step).
            if !self.cpl_auto {
                self.acdc_known |= self.acdc_seen;
                self.acdc_seen &= !1;
            }
        }
    }

    /// Start (or restart) the AC part's settle (ACS_STEP).
    fn acs_begin(&mut self, now: u64, step: bool) {
        self.acs_step = step;
        self.acs_t0 = now.max(1);
        self.acs_prev = -1.0;
        self.acs_d = -1.0;
        self.acs_flat = 0;
    }

    /// AC+DC (method A) DC result `x` (vetted), before its autorange: a DC
    /// part that moved by more than ACS_DC_STEP of the range is an input
    /// step (the AC part's settle starts). True while such a settle runs:
    /// no step down then (the magnitude would use the stale AC part; device
    /// decaf60 / emulator: a DC -> AC current step walked uA -> 10 A -> mA).
    fn acs_dc_step(&mut self, now: u64, x: f32, lim: f32) -> bool {
        if !self.acdc || self.am_on() {
            return false;
        }
        if self.acs_t0 == 0 && (x - self.acs_dc).abs() > ACS_DC_STEP * lim {
            self.acs_begin(now, true);
        }
        self.acs_t0 != 0 && self.acs_step
    }

    /// One AC result `ac` of method A's AC phase while the AC part settles
    /// after a range change or an input step (`acs_t0` != 0): true =
    /// discard it. On success the settle ends and this result is used.
    fn ac_settle_step(&mut self, now: u64, ac: f32, lim: f32) -> bool {
        if self.acs_t0 == 0 {
            return false;
        }
        let prev = self.acs_prev;
        if now.saturating_sub(self.acs_t0) >= ACS_MAX_MS && prev >= 0.0 && ac > ACS_JUMP * prev + ACS_JUMP_FLOOR * lim {
            // The time limit ran out on a jump: an input step that came while
            // the settle ran (device c8c2f02: 4.0 -> 5.8 V ~4.6 s after a 60 ->
            // 6 V step; the limit took 0.84 / 2.4 V as the AC part, and 2.4 V
            // stepped 6 -> 60 V). Its own settle starts here.
            self.acs_begin(now, true);
            self.acs_prev = ac;
            self.acs_tp = now;
            return true;
        }
        let near = prev >= 0.0 && now.saturating_sub(self.acs_tp) <= ACS_NEAR_MS;
        let flat = acs_flat(prev, self.acs_d, ac, near);
        self.acs_d = if near { prev - ac } else { -1.0 };
        self.acs_flat = if flat { self.acs_flat.saturating_add(1) } else { 0 };
        self.acs_prev = ac;
        self.acs_tp = now;
        let done = now.saturating_sub(self.acs_t0) >= ACS_MAX_MS || self.acs_flat >= ACS_FLAT_N || ac < ACS_FLOOR * lim;
        if done {
            self.acs_t0 = 0;
        }
        !done
    }

    /// AC+DC DC phase: the settle's time limit ran out with no AC result
    /// since (the DC phase is ~1 s): the last AC result held is the AC part.
    fn ac_settle_cap(&mut self, now: u64) {
        if self.acs_t0 != 0 && now.saturating_sub(self.acs_t0) >= ACS_MAX_MS && self.acs_prev >= 0.0 {
            self.acs_t0 = 0;
            self.ac_raw = self.acs_prev;
            self.f_ac = self.acs_prev;
            self.acdc_seen |= 1;
        }
    }

    fn reset_values(&mut self) {
        self.acdc_seen = 0;
        self.acdc_known = 0;
        self.acs_t0 = 0;
        self.acs_dc = f32::NAN;
        self.auto_n = 0;
        self.auto_votes = 0;
        self.auto_t0 = 0;
        self.auto_ac = false;
        self.dc_val = 0.0;
        self.ac_raw = 0.0;
        self.f_ac = 0.0;
        self.f_lpf = 0.0;
        self.ac_seed = true;
        // (AC+DC: its own settle, `ac_settle_step`; set_sub sets `acdc` first.
        // AC A: its vet. `set` resets again once `mode` is the new one.)
        self.ac_lead = if self.acdc || self.mode > 2 { 0 } else { 2 };
        self.f_dc = 0.0;
        self.f_diode = 0.0;
        self.f_cap = 0.0;
        self.cap_c = 0.0;
        self.ohm_settled = false;
        self.beep_class = 4;
        self.beep_on = false;
        self.t_phase = 0;
        self.t_votes = 0;
        // No value before a counter result past the settle: NaN, drawn "--"
        // (the AC V / A secondary line, the power meter's F).
        self.freq.f_khz = f32::NAN;
        self.freq.duty = f32::NAN;
        self.freq.range = 0;
        self.freq.n = 0;
        // No source history from another function (AC+DC's secondary
        // counter, an earlier frequency session): the first two results
        // of each source decide afresh.
        self.freq.last = [0.0; 2];
        self.freq.restart_sources();
        self.freq.src_alt = false;
        self.ramp.first = true;
    }

    /// AC+DC phase switch: the other HY3131 mode of the same quantity, values
    /// kept (stock FUN_08012c30 without the m+5 change).
    fn switch_phase(&mut self, clk: &mut Clock, mode: u8, range: i8, settle: u64) {
        let old = (self.mode, self.range);
        self.mode = mode;
        self.range = range;
        self.write_config(clk);
        self.deadline = clk.now_ms() + settle;
        self.ac_settle_start(clk.now_ms(), old);
        self.vet_start(old);
        self.tr_ev(clk.now_ms(), tr::K_PHASE, (old.0 as i32) << 8 | old.1 as u8 as i32, settle as f32, 0);
    }

    /// SCPI function code: the stock mode, or 16 = V AC+DC, 17 = A AC+DC,
    /// FUNC_V_AUTO / FUNC_A_AUTO = the AUTO coupling (the setting: it does
    /// not change when AUTO switches between DC and AC, `func_shown`).
    pub fn func(&self) -> u8 {
        if self.pw.on {
            return pw::FUNC;
        }
        match (self.cpl_auto, self.acdc, self.mode) {
            (true, _, 0..=3) => FUNC_V_AUTO,
            (true, _, _) => FUNC_A_AUTO,
            (_, true, 0..=3) => 16,
            (_, true, _) => 17,
            (_, false, m) => m,
        }
    }

    /// The function of what is shown: `func`, but in AUTO the plain DC or AC
    /// mode of the part shown (= `meter::func_of` of its readings; the
    /// limits slot, the Limits page).
    pub fn func_shown(&self) -> u8 {
        if self.cpl_auto && !self.pw.on { self.shown_mode() } else { self.func() }
    }

    /// The HY3131 mode to draw the function's glyphs / bargraph with: the
    /// configured one, in AUTO the DC or AC mode of the part shown (the
    /// alternation's phase does not matter).
    pub fn shown_mode(&self) -> u8 {
        if !self.cpl_auto {
            return self.mode;
        }
        let dm = dc_mode_of(self.mode);
        if self.auto_ac { ac_mode_of(dm) } else { dm }
    }

    /// The new function's frame before its first reading (switch-latency.md
    /// proposal 1): the unit, decimals and glyph the first reading will have
    /// at the present range, the value NaN (drawn as dimmed dashes, never a
    /// false 0.0000). None in the power meter (its own pending page).
    pub fn placeholder(&self) -> Option<Reading> {
        if self.pw.on {
            return None;
        }
        let r = self.range.max(0) as u8;
        let m = if self.acdc { dc_mode_of(self.mode) } else { self.shown_mode() };
        let (dec, unit) = match m {
            0 | 1 => (4 - r.min(3), "V"),
            2 | 3 => (3 - r.min(1), "mV"),
            4 | 7 => (4 - r.min(1), "A"),
            5 | 8 => (3 - r.min(1), "mA"),
            6 | 9 => (2 - r.min(1), "\u{b5}A"),
            10 if r == OHM600 as u8 => (1, "M\u{3a9}"),
            10 => match r {
                0 | 1 => (3 - r, "\u{3a9}"),
                2..=4 => (6 - r, "k\u{3a9}"),
                _ => (9 - r.min(6), "M\u{3a9}"),
            },
            11 => (2, "\u{3a9}"),
            12 => (4, "V"),
            13 => match r {
                0..=2 => (3 - r, "nF"),
                3..=5 => ((6 - r).min(3), "\u{b5}F"),
                _ => ((9 - r.min(7)).min(3), "mF"),
            },
            14 => {
                let v = freq_val(0.0, self.freq.range, false);
                (v.dec, v.unit)
            }
            _ => (1, self.t_disp(0.0).1),
        };
        let mut rd = self.reading(Val::new(f32::NAN, dec, unit), f32::NAN);
        if self.acdc || self.cpl_auto {
            // AC+DC / AUTO: the AC and DC part rows with their tags, dashes.
            rd.aux1 = Some(Val::new(f32::NAN, dec, unit));
            rd.aux2 = rd.aux1;
        }
        rd.mode = m;
        rd.auto = !self.manual;
        rd.auto_plus = (self.button() == Button::Volt && self.v_auto == 1) || (self.button() == Button::Amp && self.a_auto == 1);
        rd.cpl_auto = self.cpl_auto;
        rd.acdc = self.acdc && !self.cpl_auto;
        rd.prov = true;
        Some(rd)
    }

    /// Time of the last function / unit change (`set`), ms (main.rs AUTO_WAIT_MS).
    pub fn entry_ms(&self) -> u64 {
        self.entry
    }

    /// AUTO coupling (sub-mode 3) in force.
    pub fn cpl_auto(&self) -> bool {
        self.cpl_auto && !self.pw.on
    }

    /// V / A sub-mode (stock m+5): 0 DC, 1 AC, 2 AC+DC; opendm40 3 = AUTO.
    pub fn sub(&self) -> u8 {
        if self.cpl_auto {
            3
        } else if self.acdc {
            2
        } else {
            matches!(self.mode, 0 | 2 | 4..=6) as u8
        }
    }

    /// The sub-mode for the Bluetooth mode byte (2 bits, the stock app knows
    /// 0..2): AUTO reports the part it shows (0 DC / 1 AC).
    fn bt_sub(&self) -> u8 {
        if self.cpl_auto { self.auto_ac as u8 } else { self.sub() }
    }

    /// Set the V / A sub-mode (FUN_08013262 / FUN_08013134 cycle step, range
    /// page DC / AC / AC+DC buttons, BT command 06): auto range, V keeps the
    /// range (mV: r1), A keeps range % 2, values cleared (FUN_0803aad6).
    /// AC+DC starts in the DC phase, as the stock. 3 = AUTO coupling: the
    /// AC+DC alternation, showing the part it picks (`auto_eval`).
    pub fn set_sub(&mut self, clk: &mut Clock, sub: u8) {
        self.ovl_clear(clk.now_ms());
        let m = self.mode;
        let (dc, r, t) = match m {
            0..=3 => (m | 1, if m >= 2 { 1 } else { self.range }, 450),
            4..=9 => (if m >= 7 { m } else { m + 3 }, self.range.max(0) % 2, 650),
            _ => return,
        };
        let ac = if dc <= 3 { dc - 1 } else { dc - 3 };
        // Plain DC / AC: a button / SCPI entry (ENTRY_MS); AC+DC / AUTO
        // start with their DC phase's own settle.
        let t = if sub < 2 { ENTRY_MS } else { t };
        self.manual = false;
        self.acdc = sub >= 2;
        self.cpl_auto = sub == 3;
        self.phase_n = 0;
        self.reset_values();
        self.am_enter();
        self.set(clk, if sub == 1 { ac } else { dc }, r, t);
        // A coupling change starts the function afresh (`entry_ms`: the
        // AUTO coupling's SCPI wait counts from it) even on the same mode.
        self.entry = clk.now_ms();
    }

    /// Enter a stock mode directly (SCPI, BT command 06). Auto range.
    /// 16 / 17 = V / A AC+DC (see `func`).
    pub fn select(&mut self, clk: &mut Clock, mode: u8) {
        self.ovl_clear(clk.now_ms());
        if mode == pw::FUNC {
            if !self.pw.on {
                self.pw_enter(clk);
            }
            return;
        }
        if (32..32 + T_NAMES.len() as u8).contains(&mode) {
            // 32 + k: temperature with sensor k (SCPI CONF/MEAS:TEMP <type>).
            self.t_sensor = mode - 32;
            self.mode = 255;
            self.select(clk, 15);
            return;
        }
        if mode == FUNC_V_AUTO || mode == FUNC_A_AUTO {
            self.select(clk, if mode == FUNC_V_AUTO { 1 } else { 8 });
            self.set_sub(clk, 3);
            return;
        }
        if mode >= 16 {
            self.select(clk, if mode == 16 { 1 } else { 8 });
            self.set_sub(clk, 2);
            return;
        }
        self.acdc = false;
        self.cpl_auto = false;
        self.manual = false;
        let (r, settle) = match mode {
            10 => (6, OHM_ENTRY_MS),
            11 => (0, 200),
            12 => (0, 100),
            13 => (0, 200),
            14 => (0, 1000),
            15 => (self.t_range0(), 500),
            _ => (0, ENTRY_MS),
        };
        self.set(clk, mode.min(15), r, settle);
    }

    // ------------------------------------------------------ temperature API

    pub fn t_sensor(&self) -> u8 {
        self.t_sensor
    }

    /// Change the temperature sensor; re-enters mode 15 when it is active.
    pub fn set_t_sensor(&mut self, clk: &mut Clock, k: u8) {
        self.t_sensor = k.min(T_NAMES.len() as u8 - 1);
        if self.mode == 15 {
            self.mode = 255;
            self.select(clk, 15);
        }
    }

    /// Offset of sensor `k` in C (TC: applied to the cold junction).
    pub fn t_offs(&self, k: u8) -> f32 {
        self.t_offs[k as usize % 7] as f32 * 0.1
    }

    pub fn set_t_offs(&mut self, k: u8, c: f32) {
        self.t_offs[k as usize % 7] = (c.clamp(-20.0, 20.0) * 10.0 + if c < 0.0 { -0.5 } else { 0.5 }) as i16;
    }

    /// Persisted form: 7 offsets (0.1 C, i16) packed 2 per word.
    pub fn t_offs_raw(&self) -> [i16; 7] {
        self.t_offs
    }

    pub fn set_t_offs_raw(&mut self, o: [i16; 7]) {
        self.t_offs = o.map(|v| v.clamp(-200, 200));
    }

    pub fn ntc_user(&self) -> (f32, f32) {
        self.ntc_user
    }

    pub fn set_ntc_user(&mut self, r25: f32, beta: f32) {
        if r25.is_finite() && beta.is_finite() {
            self.ntc_user = (r25.clamp(100.0, 10.0e6), beta.clamp(1000.0, 10_000.0));
        }
    }

    /// Global temperature unit (Settings 2/2 "Temp unit", SCPI UNIT:TEMP):
    /// deg F instead of deg C for the main reading and the cold junction.
    /// Offsets stay in C (0.1 C steps, a calibration trim).
    pub fn temp_f(&self) -> bool {
        self.t_f
    }

    pub fn set_temp_f(&mut self, f: bool) {
        self.t_f = f;
    }

    /// A deg C value in the display unit: (value, unit).
    pub fn t_disp(&self, c: f32) -> (f32, &'static str) {
        if self.t_f { (c * 1.8 + 32.0, "\u{b0}F") } else { (c, "\u{b0}C") }
    }

    /// Cold junction (HY3131 internal sensor, C incl. the TC offset) and its
    /// age in ms; None before the first thermocouple-mode measurement.
    pub fn t_cj(&self, now: u64) -> Option<(f32, u64)> {
        if self.t_cj_at == 0 { None } else { Some((self.t_cj, now - self.t_cj_at)) }
    }

    /// R25 / Beta of a resistive sensor, R0 for an RTD (sensor 1..6).
    fn t_params(&self) -> (f32, f32) {
        match self.t_sensor {
            1 => (100.0, 0.0),
            2 => (1000.0, 0.0),
            3 => (10_000.0, 3950.0),
            4 => (10_000.0, 3435.0),
            5 => (100_000.0, 3950.0),
            _ => self.ntc_user,
        }
    }

    /// Entry range of mode 15: TC r0; PT100 r1; PT1000 r2; NTC the lowest of
    /// r2..r6 whose full scale holds R25 (room temperature).
    fn t_range0(&self) -> i8 {
        match self.t_sensor {
            0 => 0,
            1 => 1,
            2 => 2,
            _ => {
                let r25 = self.t_params().0;
                (2..6).find(|&r| r25 < 0.95e3 * self.factor() * OHM_DEC[r]).unwrap_or(6) as i8
            }
        }
    }

    /// Resistive sensor excitation estimate: (range, R ohm, I A, P W) from
    /// the range's measured short-circuit current (OHM_ISC, default variant:
    /// the p8clr rows) as Isc * Rref / (Rref + R); inferred (see the
    /// temperature notes). None for the thermocouple and on r6 (no
    /// measured current).
    pub fn t_excitation(&self) -> Option<(i8, f32, f32, f32)> {
        if self.t_sensor == 0 {
            return None;
        }
        let r = self.range.clamp(1, 6);
        let rref = OHM_RREF[r as usize] * 1e3;
        let i = ohm_test_current(r, 0)? * rref / (rref + self.t_res.max(0.0));
        Some((r, self.t_res, i, i * i * self.t_res.max(0.0)))
    }

    /// DMM analog rail off (true) / back on (false: the HY3131 configuration
    /// is rewritten RAIL_SETTLE_MS later, by `poll`: the chip lost power and
    /// ignores writes until its supply is up). The deadline moves at once,
    /// so a host READ? waits for the first reading after the wake.
    pub fn set_suspended(&mut self, clk: &mut Clock, off: bool) {
        if off != self.suspended {
            self.suspended = off;
            self.wake_at = 0;
            if !off {
                self.wake_at = clk.now_ms() + RAIL_SETTLE_MS;
                self.deadline = self.wake_at + self.wake_settle();
            }
        }
    }

    /// Settle after a rail wake: the AC node (the AC rows, AC+DC, the power
    /// meter) starts over from a powered-down front end; device e656df3, AC V
    /// on a 4.77 V rms square: the first reading +8 %, +0.4 % after 1.1 s.
    /// A host READ? waits for this.
    fn wake_settle(&self) -> u64 {
        if self.pw.on || self.acdc || matches!(self.mode, 0 | 2 | 4..=6) { 2_000 } else { 450 }
    }

    /// The rail came up and the row is not rewritten yet.
    pub fn waking(&self) -> bool {
        self.wake_at != 0
    }

    pub fn suspended(&self) -> bool {
        self.suspended
    }

    /// Wrong-connection alerts enabled (sentinel::M_*; Settings, SCPI
    /// SYST:ALER:ENAB). A change rewrites the row: a disabled sentinel
    /// leaves the stock row untouched.
    pub fn set_alert_mask(&mut self, clk: &mut Clock, mask: u8) {
        let mask = mask & sentinel::M_ALL;
        if mask == self.sent.mask {
            return;
        }
        let rows = (mask ^ self.sent.mask) & sentinel::M_JACK != 0;
        self.sent.set_mask(mask);
        if rows && !self.suspended && self.mode != 255 {
            self.write_config(clk);
            self.deadline = self.deadline.max(clk.now_ms() + 200);
        }
    }

    pub fn alert_mask(&self) -> u8 {
        self.sent.mask
    }

    /// DBG:SENTinel:VARiant: a device-check variant of the sentinel rows
    /// (sentinel::VAR_*), applied by rewriting the current row.
    pub fn set_sent_variant(&mut self, clk: &mut Clock, v: u8) {
        sentinel::set_variant(v);
        if !self.suspended && self.mode != 255 {
            self.write_config(clk);
            self.deadline = self.deadline.max(clk.now_ms() + 200);
        }
    }

    /// Bottom button press (FUN_0803e108..e1a8 -> m+0xf..0x14, consumed by
    /// FUN_08013262, app_decompile.c:14575): enter the function, or cycle its
    /// sub-mode when it is already active. The in-function cycle is also what
    /// the stock does on a PD6 click while the touch lock is on (FUN_08013134).
    ///   V: DC -> AC -> AC+DC -> DC (`set_sub`), the range is kept, in mV it
    ///      becomes r1; from elsewhere DC V r0.
    ///   A: the same cycle in the same unit, range % 2; from elsewhere DC mA
    ///      (mode 8) with the old range % 2.
    ///   ohm: again toggles m+8 (range and manual kept); from elsewhere r6 auto.
    ///   cap: from elsewhere r0 auto; again does nothing.
    ///   cont/diode: diode first, then 11 <-> 12. Hz/temp: 14 first, 14 <-> 15.
    // ---------------------------------------------------------- fuse check

    /// Start the fuse check (doc 4.3 / 4.7; SYST:FUSE:TEST, the Current
    /// Settings page): ohm on the 600 ohm range, manual, the stock row (no
    /// sentinel: the check drives the ohm current through the A-jack shunt
    /// on purpose), PE = 2 as every ohm row. The function before comes back
    /// after FUSE_READINGS readings (the power meter: re-entered, its energy
    /// restarts; a manual DMM range: kept).
    pub fn fuse_start(&mut self, clk: &mut Clock) {
        let now = clk.now_ms();
        let prev = self.fuse_job.take();
        self.ovl_clear(now);
        let job = match prev {
            Some(j) => FuseJob { t0: now, n: 0, ..j },
            None => FuseJob {
                func: self.func(),
                back: (self.manual && !self.acdc && !self.pw.on).then_some((self.mode, self.range)),
                t0: now,
                n: 0,
            },
        };
        self.acdc = false;
        self.cpl_auto = false;
        self.fuse_job = Some(job);
        self.fuse_res = sentinel::FuseRes::Busy;
        self.manual = true;
        self.set(clk, 10, 1, 600);
    }

    /// Abort a running check (the dialog's Cancel / a key): back to the
    /// function before, the result Idle.
    pub fn fuse_cancel(&mut self, clk: &mut Clock) {
        if let Some(j) = self.fuse_job.take() {
            self.fuse_res = sentinel::FuseRes::Idle;
            self.fuse_restore(clk, j);
        }
    }

    pub fn fuse_state(&self) -> sentinel::FuseRes {
        self.fuse_res
    }

    fn fuse_restore(&mut self, clk: &mut Clock, j: FuseJob) {
        self.select(clk, j.func);
        if let Some((m, r)) = j.back {
            self.manual = true;
            self.set(clk, m, r, 450);
        }
    }

    /// Per poll while the check runs: count the final, vetted ohm readings
    /// (FUSE_READINGS), decide on the last one, or give up after
    /// FUSE_TIMEOUT_MS.
    fn fuse_step(&mut self, clk: &mut Clock, now: u64, rd: Option<&Reading>) {
        let Some(mut j) = self.fuse_job else { return };
        let res = match rd {
            Some(r) if self.mode == 10 && !r.prov && self.ohm_agreed => {
                j.n += 1;
                (j.n >= FUSE_READINGS).then(|| sentinel::fuse_verdict(r.si, r.main.ol))
            }
            _ => None,
        };
        let res = res.or_else(|| (now.saturating_sub(j.t0) > FUSE_TIMEOUT_MS).then_some(sentinel::FuseRes::NoReading));
        match res {
            Some(v) => {
                self.fuse_job = None;
                self.fuse_res = v;
                self.fuse_restore(clk, j);
            }
            None => self.fuse_job = Some(j),
        }
    }

    // ------------------------------------------------------------ OVERLOAD

    /// The latch lets the autorange use shunt `unit` (sentinel::shunt_of).
    fn ovl_allows(&self, unit: u8) -> bool {
        self.ovl.map_or(true, |o| unit < o.barred)
    }

    /// A user range / function / AUTO choice ends the latch (the caller
    /// applies the new choice) and drops a running fuse check (no restore:
    /// the user chose something else).
    fn ovl_clear(&mut self, now: u64) {
        if self.ovl.take().is_some() {
            self.sent.set_overload(now, false);
        }
        // The A-jack protection latch: the user acted; it ends once the
        // current is gone too (`aprot_poll`).
        if let Some(a) = &mut self.aprot {
            a.ack = true;
        }
        if self.fuse_job.take().is_some() {
            self.fuse_res = sentinel::FuseRes::Idle;
        }
    }

    /// The OVERLOAD rule (sentinel::overload_trips) after a reading on shunt
    /// `unit` of the current DMM mode, `over` = above 1.03 x the range:
    /// manual range -> the 10 A range at once, latched with the alert; AUTO
    /// / AUTO+ -> a quiet hop to the A shunt (below). True = the row was
    /// rewritten (the caller drops the reading).
    fn ovl_trip(&mut self, clk: &mut Clock, unit: u8, over: bool) -> bool {
        if !sentinel::overload_trips(unit, self.range.max(0) as u8, self.manual, over) {
            return false;
        }
        if !self.manual {
            // AUTO (owner 2026-10-22; the power meter's rule, doc 3.14): the
            // uA / mA shunt's top range over is a normal up-range, in one
            // hop to the A shunt's 6 A range (no stop on the mA shunt after
            // uA: neither has a fuse of its own), no latch, alert or beep;
            // the walk down follows as usual (> 6.18 A: one more step to
            // 10 A). A AUTO coupling enters on DC mA r0 (stock), so AC 1 A
            // there tripped the red OVERLOAD at once (couptest `iac`).
            if matches!(self.mode, 4..=6) && !self.acdc {
                self.set(clk, 4, 0, 650);
            } else {
                // DC, and AC+DC from either phase (its DC phase first).
                self.set(clk, 7, 0, 250);
            }
            return true;
        }
        let back = match self.ovl {
            Some(o) => o.back,
            None => self.manual.then_some((self.mode, self.range)),
        };
        let barred = self.ovl.map_or(unit, |o| o.barred.min(unit));
        let bu = back.and_then(|(m, r)| sentinel::shunt_of(m).map(|u| (u, r.max(0) as u8)));
        let sticky = self.ovl_sticky(clk.now_ms());
        self.ovl = Some(Ovl { barred, back, thr: sentinel::overload_release_thr(barred, bu), low_since: None, sticky });
        // The 10 A range: manual when the user had a manual range (it comes
        // back on release), else auto with the shunt floor (`ovl_allows`).
        self.manual = back.is_some();
        if matches!(self.mode, 4..=6) && !self.acdc {
            self.set(clk, 4, 1, 650);
        } else {
            // DC, and AC+DC from either phase (its DC phase first).
            self.set(clk, 7, 1, 250);
        }
        self.sent.set_overload(clk.now_ms(), true);
        true
    }

    /// A new / renewed latch is sticky: tripped again soon after a timed
    /// release, or an existing latch already was.
    fn ovl_sticky(&self, now: u64) -> bool {
        self.ovl.map_or(false, |o| o.sticky) || self.ovl_rel_t.is_some_and(|t| now.saturating_sub(t) < sentinel::OVL_RETRIP_MS)
    }

    /// Latch release: `i` (A, the reading's magnitude; `ol` = over range)
    /// below the threshold for OVL_RELEASE_MS -> back to the manual range
    /// it left, or (auto) the floor goes.
    fn ovl_release(&mut self, clk: &mut Clock, now: u64, i: f32, ol: bool) {
        let Some(mut o) = self.ovl else { return };
        if o.sticky || ol || !(i < o.thr) {
            o.low_since = None;
            self.ovl = Some(o);
            return;
        }
        let t0 = *o.low_since.get_or_insert(now);
        if now.saturating_sub(t0) < sentinel::OVL_RELEASE_MS {
            self.ovl = Some(o);
            return;
        }
        self.ovl = None;
        self.ovl_rel_t = Some(now);
        self.sent.set_overload(now, false);
        if let Some((m, r)) = o.back {
            if self.pw.on {
                self.pw.ik = m.min(5);
                self.pw.keep = 2;
                self.pw_apply(clk);
            } else {
                self.manual = true;
                self.set(clk, m, r, if matches!(m, 4..=6) { 650 } else { 250 });
            }
        }
    }

    pub fn press(&mut self, clk: &mut Clock, b: Button) {
        self.ovl_clear(clk.now_ms());
        // Power meter: any function button leaves it; A to the DC A mode of
        // the power meter's current range (a load current may still flow:
        // not the mA shunt of the usual A entry).
        let from_pw = self.pw.on.then(|| pw::iu_ir(self.pw.ik));
        let same = self.button() == b && from_pw.is_none();
        let m = self.mode;
        if same && matches!(b, Button::Volt | Button::Amp) {
            // DC -> AC -> AC+DC -> AUTO -> DC (the stock: DC -> AC ->
            // AC+DC, skipping AC+DC for model 0; AUTO is opendm40).
            let next = (self.sub() + 1) % 4;
            self.set_sub(clk, next);
            return;
        }
        if !same {
            self.acdc = false;
            self.cpl_auto = false;
        }
        match b {
            Button::Volt => {
                self.manual = false;
                self.set(clk, 1, 0, ENTRY_MS);
            }
            Button::Amp => {
                self.manual = false;
                match from_pw {
                    Some((iu, ir)) => self.set(clk, 7 + iu, ir as i8, 650),
                    None => {
                        let r = self.range.max(0) % 2;
                        self.set(clk, 8, r, ENTRY_MS);
                    }
                }
            }
            Button::Ohm => {
                // A second tap toggles the test current ("ON"); 600 Mohm has
                // its own source (its range page hides the buttons too).
                if !same {
                    self.manual = false;
                    self.set(clk, 10, 6, OHM_ENTRY_MS);
                } else if !is_ohm600(self.mode, self.range) {
                    self.set_ohm_var(clk, self.ohm_var ^ 1);
                }
            }
            Button::Cap => {
                if !same {
                    self.manual = false;
                    self.set(clk, 13, 0, 200);
                }
            }
            Button::ContDiode => {
                self.manual = false;
                let to = if same && m == 12 { 11 } else { 12 };
                self.set(clk, to, 0, if to == 11 { 200 } else { 100 });
            }
            Button::HzTemp => {
                self.manual = false;
                let to = if same && m == 14 { 15 } else { 14 };
                let r = if to == 15 { self.t_range0() } else { 0 };
                self.set(clk, to, r, if to == 14 { 1000 } else { 500 });
            }
            // Again in the power meter: the next coupling, as a second tap
            // on V / A steps DC / AC / AC+DC (the app shows the wiring dialog
            // first when it was never confirmed).
            Button::Power => {
                if self.pw.on {
                    let c = pw::next_coupling(self.pw.coup);
                    self.pw_set_coupling(clk, c);
                } else {
                    self.pw_enter(clk);
                }
            }
        }
    }

    /// AUTO key (FUN_0803ddf4, app_decompile.c:59745-59777): back to auto
    /// range; only when the range was fixed (manual or REL, m+3 != 0): V in mV
    /// with AUTO (m+6 == 1) -> V r0, A with AUTO (m+7 == 1) -> mA r0.
    pub fn auto_key(&mut self, clk: &mut Clock) {
        self.ovl_clear(clk.now_ms());
        if self.pw.on {
            return self.pw_set_ranges(clk, Some(None), Some(None));
        }
        let was_manual = self.manual;
        self.manual = false;
        if !was_manual {
            return;
        }
        match self.mode {
            2 | 3 if self.v_auto == 1 => self.set(clk, self.mode - 2, 0, 350),
            // A manual 60 ohm with the "ON" variant: AUTO continues from
            // 600 ohm (`ohm_auto_floor`); with the default one from 60 ohm.
            10 if self.range < self.ohm_auto_floor() => self.set(clk, 10, 1, 150),
            // 600 Mohm is not in the autorange: AUTO from its top, 60 Mohm.
            10 if self.range > OHM_AUTO_TOP => self.set(clk, 10, OHM_AUTO_TOP, 150),
            // Stock: m+5 == 1 (AC) -> 5, else (DC, AC+DC) -> 8.
            4..=9 if self.a_auto == 1 => {
                if self.sub() == 1 {
                    self.set(clk, 5, 0, 650)
                } else {
                    self.set(clk, 8, 0, 350)
                }
            }
            _ => {}
        }
    }

    /// Freeze the current range (REL does this) or release it.
    pub fn hold_range(&mut self, manual: bool) {
        if self.pw.on {
            return; // the power meter's ranges: its own page / SCPI
        }
        self.manual = manual;
    }

    /// Manual range from the MF popup: V items 600 mV, 6, 60, 600, 1000 V
    /// (0..4), 5 = 60 mV (the DC / AC mV r0 row, OP1 x10; the stock tables
    /// have it, its UI offers no 60 mV item); A items 600 uA, 6000 uA,
    /// 60 mA, 600 mA, 6 A, 10 A; ohm 1..6 (DM40C labels, COUNTS_MODEL),
    /// ohm 0 = the 60 ohm range (the stock has no item for it). `None` = AUTO, `Some(255)` =
    /// AUTO+ (V/A only).
    pub fn set_range(&mut self, clk: &mut Clock, item: Option<u8>) {
        self.ovl_clear(clk.now_ms());
        match (self.button(), item) {
            (Button::Volt, None) => {
                self.manual = false;
                self.v_auto = 3;
            }
            (Button::Volt, Some(255)) => {
                self.manual = false;
                self.v_auto = 1;
                // AUTO+ never steps to mV: leave it at once for the V
                // autorange's 6 V (device check 2f56ff9: AUTO -> AUTO+ on
                // 60 mV stayed there until the function was entered again).
                // AC+DC: into its DC phase, as poll_ac's mV -> V step.
                if matches!(self.mode, 2 | 3) {
                    let m = if self.acdc { 1 } else { self.mode - 2 };
                    self.set(clk, m, 0, 350);
                }
            }
            (Button::Amp, None) => {
                self.manual = false;
                self.a_auto = 3;
            }
            (Button::Amp, Some(255)) => {
                self.manual = false;
                self.a_auto = 1;
            }
            (Button::Volt, Some(i)) if i <= 5 => {
                let ac = self.sub() == 1;
                self.manual = true;
                let (m, r) = match i {
                    0 => (if ac { 2 } else { 3 }, 1),
                    5 => (if ac { 2 } else { 3 }, 0),
                    _ => (if ac { 0 } else { 1 }, i as i8 - 1),
                };
                if !self.same_row(m, r) {
                    // The stock's range write (S+7 != m+1: 0xfa, switch-
                    // latency.md proposal 4); AC+DC keeps its DC phase's 450.
                    self.set(clk, m, r, if self.acdc { 450 } else { RANGE_MS });
                }
            }
            (Button::Amp, Some(i)) if i <= 5 => {
                let ac = self.sub() == 1;
                let unit = 2 - (i / 2); // 0 A, 1 mA, 2 uA
                let m = if ac { 4 } else { 7 } + unit;
                self.manual = true;
                if !self.same_row(m, (i % 2) as i8) {
                    self.set(clk, m, (i % 2) as i8, 650);
                }
            }
            (Button::Ohm, None) => {
                self.manual = false;
                if self.mode == 10 && self.range < self.ohm_auto_floor() {
                    self.set(clk, 10, 1, 150);
                } else if self.mode == 10 && self.range > OHM_AUTO_TOP {
                    self.set(clk, 10, OHM_AUTO_TOP, 150);
                }
            }
            // 7: the experimental 600 Mohm (`OHM600`), only by this choice.
            (Button::Ohm, Some(i)) if i <= OHM600 as u8 => {
                self.manual = true;
                if !self.same_row(10, i as i8) {
                    self.set(clk, 10, i as i8, 150);
                }
            }
            _ => {}
        }
    }

    /// `set_range`: the chip already runs mode `m` (AC+DC: either phase of
    /// it) on range `r`, so choosing that item again (the highlighted item
    /// tapped, SCPI `<f>:RANG` with the present range, sigrok `range=`, or
    /// AUTO -> manual on the range AUTO is on) only makes the range manual.
    /// Rewriting the row then restarted the conversion behind a 450 ms (then)
    /// settle that reset nothing (`set` with the mode unchanged): 1-2
    /// readings of 25-51 % of the input on DC V (device check 540e056).
    fn same_row(&self, m: u8, r: i8) -> bool {
        let other = match m {
            1 | 3 => m - 1,
            7..=9 => m - 3,
            _ => m,
        };
        !self.pw.on && self.range == r && (self.mode == m || (self.acdc && self.mode == other))
    }

    /// BT mode byte (bt_meas_v2_6.h section 2, forward FUN_0803c842):
    /// bits 0-2 group, 3-5 range field, 6-7 sub-mode.
    pub fn mode_byte(&self) -> u8 {
        if self.pw.on {
            return pw::bt_mode_byte(self.pw.v_auto, self.pw.vr);
        }
        let m = self.mode;
        match m {
            0..=3 => {
                let sub = self.bt_sub();
                let r = if self.manual {
                    if m >= 2 { 0 } else { self.range as u8 + 1 }
                } else if self.v_auto == 1 {
                    6
                } else {
                    5
                };
                sub << 6 | r << 3
            }
            4..=9 => {
                let sub = self.bt_sub();
                let unit = (m - 4) % 3; // 0 A, 1 mA, 2 uA
                let r = if self.manual {
                    [4, 2, 0][unit as usize] + self.range as u8
                } else if self.a_auto == 1 {
                    7
                } else {
                    6
                };
                sub << 6 | r << 3 | 1
            }
            10 => {
                // BT has no 60 ohm or 600 Mohm range: report them as 600
                // ohm / 60 Mohm.
                let r = if self.manual { self.range.clamp(1, 6) as u8 - 1 } else { 6 };
                self.ohm_var << 6 | r << 3 | 2
            }
            11 => 0x44,
            12 => 0x04,
            13 => 0x03,
            14 => 0x05,
            _ => 0x45,
        }
    }

    /// BT command 06 (reverse, FUN_0803c6a8 + FUN_0803c3b4). Returns true if
    /// the function or sub-mode changed (the caller then clears HOLD/REL).
    pub fn apply_mode_byte(&mut self, clk: &mut Clock, b: u8) -> bool {
        // The power meter's own byte (the phone echoing what cmd 09
        // reported) keeps it; any other byte leaves it (`select`).
        if self.pw.on && b == self.mode_byte() {
            return false;
        }
        // AUTO coupling: the byte it reports (the part shown as DC / AC)
        // echoed back keeps AUTO; any other byte sets what it says.
        if self.cpl_auto && b == self.mode_byte() {
            return false;
        }
        // The ohm ranges BT has no code for (60 ohm, 600 Mohm: reported as
        // 600 ohm / 60 Mohm, `mode_byte`): that byte echoed back keeps them.
        if self.mode == 10 && self.manual && matches!(self.range, 0 | OHM600) && b == self.mode_byte() {
            return false;
        }
        self.ovl_clear(clk.now_ms());
        let (g, r, s) = (b & 7, (b >> 3) & 7, b >> 6);
        let before = (self.button(), self.mode_byte() & 0xC0);
        match g {
            0 => {
                if self.button() != Button::Volt {
                    self.select(clk, 1);
                }
                self.bt_acdc(s);
                let ac = s == 1;
                if r <= 4 {
                    self.manual = true;
                    if r == 0 {
                        self.set(clk, if ac { 2 } else { 3 }, 1, 450);
                    } else {
                        self.set(clk, if ac { 0 } else { 1 }, r as i8 - 1, 450);
                    }
                } else {
                    self.manual = false;
                    if r == 5 {
                        self.v_auto = 3;
                    } else if r == 6 {
                        self.v_auto = 1;
                    }
                    let mv = self.mode >= 2;
                    let (m, rr) = if r == 6 && mv { (0, 0) } else { ((mv as u8) * 2, self.range) };
                    let m = m + (!ac) as u8;
                    if m != self.mode {
                        self.set(clk, m, rr, 450);
                    }
                }
            }
            1 => {
                if self.button() != Button::Amp {
                    self.select(clk, 8);
                }
                self.bt_acdc(s);
                let ac = s == 1;
                if r <= 5 {
                    self.manual = true;
                    let m = if ac { 6 } else { 9 } - r / 2;
                    self.set(clk, m, (r % 2) as i8, 650);
                } else {
                    self.manual = false;
                    if r == 6 {
                        self.a_auto = 3;
                    } else {
                        self.a_auto = 1;
                    }
                    let mut unit = (self.mode - 4) % 3;
                    let mut rr = self.range;
                    if r == 7 && unit == 2 {
                        unit = 1;
                        rr = 0;
                    }
                    let m = if ac { 4 } else { 7 } + unit;
                    if m != self.mode || rr != self.range {
                        self.set(clk, m, rr, 650);
                    }
                }
            }
            2 => {
                if self.mode != 10 {
                    self.select(clk, 10);
                }
                self.ohm_var = s & 1;
                if r == 6 {
                    self.manual = false;
                    // AUTO from 600 Mohm starts at the autorange's top.
                    let rr = self.range.min(OHM_AUTO_TOP);
                    self.set(clk, 10, rr, 300);
                } else {
                    self.manual = true;
                    self.set(clk, 10, (r as i8 + 1).min(6), 150);
                }
            }
            3 => self.select(clk, 13),
            4 => self.select(clk, if s == 1 { 11 } else { 12 }),
            5 => self.select(clk, if s == 1 { 15 } else { 14 }),
            _ => {}
        }
        before != (self.button(), self.mode_byte() & 0xC0)
    }

    /// BT command 06 sub-mode 2 = AC+DC (page+9 = s; AC+DC uses the DC HY
    /// mode, which the caller then selects for s != 1).
    fn bt_acdc(&mut self, s: u8) {
        if s != 2 {
            self.diag_on = false;
        }
        // The stock byte has no AUTO: a byte that is not AUTO's own echo
        // (apply_mode_byte) ends it.
        self.cpl_auto = false;
        if (s == 2) != self.acdc {
            self.acdc = s == 2;
            self.phase_n = 0;
            self.reset_values();
        }
    }

    /// Anti-drop: pulse the latching V/A relay again into its current state.
    pub fn reassert_relay(&mut self, clk: &mut Clock) {
        if self.suspended {
            return;
        }
        if let Some(pa) = self.pa_now {
            relay_pa(clk, pa);
        }
    }

    /// `precond_ms` (device experiment): the thermocouple row (relay PA 1,
    /// the rest shunt) for that long, blocking, before the DC A row.
    #[inline(never)] // .cold (link-native.x): opt-in experiment
    fn precondition(&mut self, clk: &mut Clock) {
        hy::quiesce();
        if self.pa_now != Some(1) {
            relay_pa(clk, 1);
            self.pa_now = Some(1);
        }
        relay_pe(2);
        hy::configure(&CFG_F15[0]);
        clk.delay_ms(self.precond_ms.min(2000) as u32);
        self.a_blind = false;
    }

    /// DBG:SENTinel:REG?: R20..R37 read back (48 hex digits), then the
    /// relay PA state (0 V/A, 1 the ohm network, 255 unknown), the mode,
    /// the range and `a_blind`.
    #[inline(never)] // .cold (link-native.x): DBG reply
    #[cfg_attr(not(feature = "dbg-sent"), allow(dead_code))]
    pub fn reg_text(&self, buf: &mut [u8; 72]) -> usize {
        let mut b = [0u8; 24];
        hy::read(0x20, &mut b);
        const H: &[u8; 16] = b"0123456789ABCDEF";
        let mut n = 0;
        for x in b {
            buf[n] = H[(x >> 4) as usize];
            buf[n + 1] = H[(x & 15) as usize];
            n += 2;
        }
        for v in [self.pa_now.unwrap_or(255), self.mode, self.range as u8, self.a_blind as u8] {
            buf[n] = b',';
            n += 1;
            let mut d = [0u8; 3];
            let mut k = 0;
            let mut x = v;
            loop {
                d[k] = b'0' + x % 10;
                k += 1;
                x /= 10;
                if x == 0 {
                    break;
                }
            }
            while k > 0 {
                k -= 1;
                buf[n] = d[k];
                n += 1;
            }
        }
        n
    }

    /// (Re)start measuring in the current mode.
    pub fn start(&mut self, clk: &mut Clock) {
        if self.wake_at != 0 && !self.suspended {
            // The rail came up < RAIL_SETTLE_MS ago and the chip ignores
            // writes until its supply is up: a row written now is lost (a
            // logger session started by a host on the Scope / Gen page,
            // where the DMM rail is off, ran with no samples: fastlogtest
            // --ui-nav). Wait out the rest of the settle (<= 50 ms).
            let now = clk.now_ms();
            if now < self.wake_at {
                clk.delay_ms((self.wake_at - now) as u32);
            }
        }
        self.wake_at = 0;
        self.wd_t = clk.now_ms();
        if self.pw.on {
            return self.pw_restart(clk);
        }
        let (m, r) = (self.mode, self.range);
        self.mode = 255;
        // The chip was another's (Wave view, logger), asleep or lost: the
        // sentinel's history starts over (`write_config` `fresh`).
        self.sent_mode = 255;
        self.a_blind = true;
        // ... and the A rows' SENSE comparator reads 0x70 for seconds after
        // a restart too (device 469f354 E2: A, Scope page, back).
        self.ohm_exit_t = Some(clk.now_ms());
        self.set(clk, m, r, 450);
    }

    // ----------------------------------------------------------------- poll

    /// The rail wake and the stall watchdog, before each poll; true = it
    /// (re)started the row (no reading this poll). With no reading for
    /// WD_CHECK_MS past the deadline it reads R2A..R31 back (every
    /// WD_CHECK_MS): all 0x00 / 0xFF where the row has other bytes = the chip
    /// lost power (a POR) after the row went out -> rewrite it. No reading
    /// for 8 x the reading period (WD_IDLE_MS at least; not in capacitance,
    /// whose ramp may wait on a discharge) -> restart too. A gap in the polls
    /// (the logger owns the chip; it restarts the row) restarts the timer.
    #[inline(always)]
    fn watchdog(&mut self, clk: &mut Clock, now: u64) -> bool {
        if self.wake_at != 0 {
            return self.wd_slow(clk, now, 0);
        }
        if now.saturating_sub(self.wd_poll) > 500 {
            self.wd_t = now;
        }
        self.wd_poll = now;
        let idle = now.saturating_sub(self.wd_t.max(self.deadline()));
        idle >= WD_CHECK_MS && now >= self.wd_chk && self.wd_slow(clk, now, idle)
    }

    /// `watchdog`'s rare part (.cold, link-native.x): the wake (`idle` 0)
    /// and the read-back.
    #[inline(never)]
    fn wd_slow(&mut self, clk: &mut Clock, now: u64, idle: u64) -> bool {
        if self.wake_at != 0 {
            if now < self.wake_at {
                return true;
            }
            self.wd_n[0] = self.wd_n[0].wrapping_add(1);
            self.start(clk);
            self.deadline = self.deadline.max(now + self.wake_settle());
            return true;
        }
        self.wd_chk = now + WD_CHECK_MS;
        let mut b = [0u8; 8];
        hy::read(CHK_AT as u8 + hy::R_CFG, &mut b);
        let lost = b != self.chk && (b == [0; 8] || b == [0xFF; 8]);
        let silent = self.mode != 13 && idle > WD_IDLE_MS.max(8 * self.period_ms as u64);
        if !(lost || silent) {
            return false;
        }
        let k = if lost { 1 } else { 2 };
        self.wd_n[k] = self.wd_n[k].wrapping_add(1);
        self.start(clk);
        true
    }

    /// Poll the converter (every ~10 ms). Returns a reading when one is ready.
    pub fn poll(&mut self, clk: &mut Clock) -> Option<Reading> {
        let now = clk.now_ms();
        if self.suspended || self.watchdog(clk, now) {
            return None;
        }
        hy::beeper_sync(); // a volume change while in continuity
        self.diode_beep(now);
        // AC+DC: the stock runs the frequency counter at the top of the ACV /
        // DCV / DCmV / DCA cases, before its settle check (ACDC_FREQ_RESTART;
        // the ACA phase does it after each result, see poll_ac).
        if ACDC_FREQ_RESTART && self.acdc && matches!(self.mode, 0 | 1 | 3 | 7..=9) {
            let src = self.acdc_src();
            self.freq_secondary(now, src);
        }
        let cmp = matches!(self.sent.kind, Kind::CmpA | Kind::CmpV);
        if self.sent.bridge && self.sent.kind == Kind::PeakV {
            // AC+DC A's AC phase: the DC phase's V-JACK? run asserts on time
            // and holds (settle / blanks: no peak window), sentinel `tick`.
            self.sent.tick(now);
        }
        // A-jack protection (doc 4.8): also through the settles (the
        // temperature phases), so the probe windows keep their 100 ms.
        if !self.pw.on && (self.probe || self.sent.kind == Kind::CmpA) {
            self.aprot_poll(clk, now);
        }
        if self.mode == 11 && self.jwin == 0 && !self.pw.on && !hy::streaming() && !self.sent.blanked(now) {
            // The continuity beeper's comparator: CTSTA bit 6 (ACPO) is
            // the latch whose buffered copy CMPO drives the CMP pin
            // (datasheet, comparator section), also while ENPCMPO is off
            // (volume 0 %, `mute`). Every poll (5 ms), also through the
            // entry settle (the beeper sounds from the row write on; not
            // in the relay pulse's blank): `beeping`.
            self.beep_on = hy::read1(0x14) & 0x40 != 0;
        }
        if now < self.deadline {
            // AC+DC: the comparator watches through the phase settles too
            // (600..900 ms each; A-JACK clears 1 s after the last hit).
            // (not before sentinel::BLANK_ROW_MS: the relay pulse, Sentinel::blank)
            if cmp && self.acdc && !self.suspended {
                self.sent.sample_cmp(now, hy::read1(0x14));
            }
            // AC+DC: the peak hold restarts at the blank's end, not the
            // settle's (`sent_rearm`): it bridges the phase settle as before.
            if self.sent_arm && self.acdc && !self.suspended && !self.sent.blanked(now) {
                self.sent_rearm(now);
            }
            return None;
        }
        if self.sent_arm {
            self.sent_rearm(now);
        }
        self.t_poll = now;
        self.avg_n = 0;
        if cmp {
            // One byte (CTSTA): CMPHO / CMPLO of the comparator on PB1 /
            // the V tap, on every poll so an AC current is caught whatever
            // the poll rhythm (OR over 100 ms windows).
            if self.sent.sample_cmp(now, hy::read1(0x14)) && self.sent.alt {
                // AC? alternation: the next window's node; the first CTSTA
                // after the switch is dropped (a bit of the other node).
                hy::write(0x20, &[if self.sent.alt_v { sentinel::R20_SENSE } else { sentinel::R20_PB1 }]);
                let _ = hy::read1(0x14);
            }
        }
        if self.pkh_due != 0 && now >= self.pkh_due {
            self.pkh_due = 0;
            hy::write(0x29, &[self.r29]);
        }
        let jw = self.sent.jchk
            && self.sent.kind == Kind::Ad2A
            && (self.jwin != 0 || self.sent.jack_due(now))
            && self.cont_jack(now);
        if !jw && self.peak_in_poll() && now.saturating_sub(self.pk_t) >= sentinel::WIN_MS {
            // Frequency / the AD2 kinds: no AC result to bring PKHMIN /
            // PKHMAX along; read them (6 bytes) every window and restart
            // the hold as poll_ac does.
            self.pk_t = now;
            let mut b = [0u8; 6];
            hy::read(0x0E, &mut b);
            self.sent.sample_peak(now, hy::s24(&b[0..3]), hy::s24(&b[3..6]));
            if PKH_RESTART {
                hy::write(0x29, &[self.r29 & !sentinel::R29_ENPKH]);
                hy::write(0x29, &[self.r29]);
            }
        }
        if self.pw.on {
            self.pw_service(clk);
        }
        let r = if self.pw.on { self.poll_power(clk, now) } else { None };
        let r = if self.pw.on { r } else { match self.mode {
            10 | 12 if src::active() => self.poll_src(now),
            0 | 4..=6 if self.am_on() => self.poll_am(clk, now),
            0 | 2 | 4..=6 => self.poll_ac(clk, now),
            1 | 3 => self.poll_dcv(clk),
            7..=9 => self.poll_dca(clk),
            10 if is_ohm600(self.mode, self.range) => self.poll_ohm600(clk),
            10 => self.poll_ohm(clk, now),
            11 => self.poll_cont(),
            12 => self.poll_diode(now),
            13 => self.poll_cap(clk, now),
            14 => self.poll_freq_mode(now),
            _ if self.t_sensor != 0 => self.poll_rtd(clk, now),
            _ => self.poll_temp(now),
        } };
        // G7 (AUTO deciding): a measured value as before, so the sentinel /
        // OVERLOAD release / watchdog below still see it; only its output
        // is provisional.
        let g7 = core::mem::take(&mut self.prov_side);
        if let Some(p) = r.filter(|p| p.prov && !g7) {
            // Provisional (`Reading::prov`): drawn only; no sentinel, no
            // watchdog, no seq, READ? / FETCh? keep waiting (rd_pending).
            if self.fuse_job.is_some() {
                self.fuse_step(clk, now, None);
            }
            return Some(self.prov_out(p, now));
        }
        if let Some(rd) = &r {
            // AUTO coupling: the checks below look at the whole signal (the
            // shown part may be the small one of the two).
            let si_abs = if self.cpl_auto && !self.pw.on { self.auto_total_si } else { rd.si.abs() };
            self.wd_t = now;
            ema_ms(&mut self.period_ms, &mut self.t_rd, now);
            self.rd_avg_n = self.avg_n;
            match self.mode {
                4..=9 if self.sent.kind.family() == 2 && !self.pw.on => {
                    // V-JACK? precondition: |I| < 0.5 % of the range (AC A, AC+DC:
                    // 5 %, above the RMS floor of the uA path: ~8.7 uA = 1.5 %
                    // of 600 uA on the device, acdc doc 6b).
                    let fs = sentinel::current_fs(sentinel::shunt_of(self.mode).unwrap_or(0), self.range.max(0) as u8);
                    let k = if self.mode <= 6 || self.acdc { sentinel::AC_FLOOR_FRAC } else { 0.005 };
                    self.sent.current_small(now, !rd.main.ol && si_abs < k * fs);
                }
                10..=12 if !self.acdc && self.fuse_job.is_none() => {
                    self.v_side_ohm(now, rd.main.ol);
                    self.sent.live_reading(now, self.live)
                }
                _ => {}
            }
            if matches!(self.mode, 4..=9) && !self.pw.on {
                // FUSE? hint: an A function at zero (doc 4.3).
                let fs = sentinel::current_fs(sentinel::shunt_of(self.mode).unwrap_or(0), self.range.max(0) as u8);
                let k = if self.mode <= 6 || self.acdc { sentinel::AC_FLOOR_FRAC } else { sentinel::FUSE_FRAC };
                self.sent.fuse_reading(now, !rd.main.ol && si_abs < k * fs);
            }
            if self.ovl.is_some() {
                let (i, ol) = match &rd.power {
                    Some(pv) => (pv.i, pv.i_ol),
                    None => (si_abs, rd.main.ol),
                };
                self.ovl_release(clk, now, i, ol);
            }
        }
        if self.fuse_job.is_some() {
            self.fuse_step(clk, now, r.as_ref());
        }
        r.map(|mut r| {
            if r.prov {
                return self.prov_out(r, now);
            }
            self.rd_pending = false;
            self.seq = self.seq.wrapping_add(1);
            r.seq = self.seq;
            r.auto = !self.manual;
            r.auto_plus = (self.button() == Button::Volt && self.v_auto == 1)
                || (self.button() == Button::Amp && self.a_auto == 1 && !self.pw.on);
            r.cpl_auto = self.cpl_auto && !self.pw.on;
            r.acdc = self.acdc && !r.cpl_auto;
            r.avg_n = self.avg_n;
            // OVERLOAD: red for its first seconds, then amber "10A".
            r.alert = self.sent.shown(clk.now_ms());
            r
        })
    }

    /// A provisional reading (`Reading::prov`) as `poll` hands it out: the
    /// range / coupling labels and the alert of a settled one.
    fn prov_out(&self, mut r: Reading, now: u64) -> Reading {
        r.auto = !self.manual;
        r.auto_plus = (self.button() == Button::Volt && self.v_auto == 1) || (self.button() == Button::Amp && self.a_auto == 1);
        r.cpl_auto = self.cpl_auto;
        r.acdc = self.acdc && !self.cpl_auto;
        r.alert = self.sent.shown(now);
        r
    }

    /// V_GATE of the ohm-network rows per reading: ohm by the LIVE? sign;
    /// continuity / diode: a reading under OL (`ol` false) within
    /// CLOSED_HOLD_MS = V-COM closed (the probes touch, a part), while
    /// A-JACK's case (the leads in A / COM) leaves V-COM open: OL.
    #[inline(never)]
    fn v_side_ohm(&mut self, now: u64, ol: bool) {
        let drv = if self.mode == 10 {
            // (the jack check's BOTH: sentinel `own`)
            self.sent.vin_t = if ol { 0 } else { now.max(1) };
            self.live
        } else {
            if !ol {
                self.closed_t = now.max(1);
            }
            self.closed_t != 0 && now.saturating_sub(self.closed_t) < CLOSED_HOLD_MS
        };
        self.sent.v_side(now, drv);
    }

    fn reading(&self, main: Val, si: f32) -> Reading {
        Reading {
            mode: self.mode,
            range: self.range,
            main,
            aux1: None,
            aux2: None,
            si,
            auto: true,
            auto_plus: false,
            acdc: false,
            cpl_auto: false,
            seq: 0,
            avg_n: 0,
            alert: Alert::None,
            power: None,
            prov: false,
        }
    }

    fn ad1(&mut self) -> Option<i32> {
        if hy::read1(hy::R_INTF) & hy::INTF_AD1 == 0 {
            return None;
        }
        let mut b = [0u8; 3];
        hy::read(hy::R_AD1, &mut b);
        ema_ms(&mut self.conv_ms, &mut self.t_conv, self.t_poll);
        Some(hy::s24(&b))
    }

    /// AD1 OSR switching of continuity: stable -> 0x17 (32768, written
    /// once), else `fast`.
    fn osr(&mut self, ad1: i32, thr: i32, fast: u8) {
        let was = self.r22;
        if (ad1 - self.prev_ad1).abs() <= thr {
            // Not within R22_DROP_WIN_MS of the row write (R22_LATE): the
            // fast OSR goes on until then, no conversions dropped.
            if self.r22 != OSR_SLOW && !self.r22_early() {
                hy::write(0x22, &[OSR_SLOW]);
                self.r22 = OSR_SLOW;
                self.avg_skip = true;
            }
        } else {
            hy::write(0x22, &[fast]);
            self.r22 = fast;
        }
        if self.r22 != was {
            self.r22_dropping();
        }
        self.prev_ad1 = ad1;
    }

    /// An R22 write of `osr` / `poll_ohm` that changed the OSR: within
    /// R22_DROP_WIN_MS of the row write, drop the next R22_DROP_N
    /// conversions.
    fn r22_dropping(&mut self) {
        if R22_DROP_N != 0 && self.r22_early() {
            self.r22_drop = R22_DROP_N;
        }
    }

    /// Within R22_DROP_WIN_MS of the row write: an R22 change now would
    /// cost R22_DROP_N conversions (R22_LATE).
    fn r22_early(&self) -> bool {
        self.t_poll < self.row_t + R22_DROP_WIN_MS
    }

    /// A conversion to drop (`r22_dropping`): true = drop it.
    fn r22_drop_one(&mut self) -> bool {
        if self.r22_drop == 0 {
            return false;
        }
        self.r22_drop -= 1;
        // (as a FAST refresh's drop, `fast_code`: no interval across it)
        self.t_rd = None;
        true
    }

    /// An over-range DC conversion may step a range up / trip OVERLOAD:
    /// a vetted one, or the second over-range one in a row on the row (a
    /// held one alone can be the bad first conversion after a row write,
    /// OVER_TWICE). AC+DC as before (every conversion).
    fn over_ok(&mut self, over: bool, vet: bool) -> bool {
        let ok = over && (vet || self.over_prev || self.acdc || !OVER_TWICE);
        self.over_prev = over;
        ok
    }

    /// AC V 6 V, every AC part `ac` (V): AC mV clipped on this input
    /// (`mv_clip`), so no step down into it. The reference is the AC part
    /// MV_CLIP_SETTLE_MS after the row write; a later one more than 10 % + 2 mV
    /// off it releases the hold (the input changed: a square switched off,
    /// another signal).
    #[inline(never)]
    fn mv_clip_holds(&mut self, now: u64, ac: f32) -> bool {
        if self.mv_clip < 0.0 {
            if now >= self.row_t + MV_CLIP_SETTLE_MS {
                self.mv_clip = ac.max(1e-6);
            }
        } else if self.mv_clip > 0.0 && (ac - self.mv_clip).abs() > 0.1 * self.mv_clip + 0.002 {
            self.mv_clip = 0.0;
        }
        self.mv_clip != 0.0
    }

    /// Plain AC A after a row write (ACA_VET_*): true = use this result.
    fn aca_vet(&mut self, now: u64, ac: f32, lim: f32) -> bool {
        if self.aca_vet_t == 0 || !ACA_VET {
            return true;
        }
        let p = core::mem::replace(&mut self.aca_prev, ac);
        if (ac - p).abs() <= ACA_VET_REL * ac.max(p) + ACA_VET_FS * lim || now >= self.aca_vet_t + ACA_VET_MAX_MS {
            self.aca_vet_t = 0;
            return true;
        }
        false
    }

    /// Averaged ranges (see the +1 digit and FR-05 tables): (window,
    /// restart threshold in codes = ~2 counts, extra digit possible).
    fn avg_cfg(&self) -> Option<(usize, i32, bool)> {
        avg_cfg(self.rate, self.acdc, self.mode, self.range)
    }

    /// Code for the value formula: the moving average in averaged ranges
    /// (restarted by a moving-OSR conversion), else the raw code. Also
    /// returns whether the extra digit is shown (`ext_ok`).
    #[inline(never)]
    fn avg_code(&mut self, code: i32, slow: bool) -> (f32, bool) {
        match self.avg_cfg() {
            Some((n, thr, ext)) => {
                if !slow {
                    self.avg.reset();
                }
                let mean = self.avg.push(code, n, thr);
                self.avg_n = self.avg.n as u8;
                (mean, ext_ok(ext, self.avg_n))
            }
            None => (code as f32, false),
        }
    }

    fn poll_dcv(&mut self, clk: &mut Clock) -> Option<Reading> {
        let ad1 = self.ad1()?;
        if self.r22_drop_one() {
            return None;
        }
        let ad1 = self.fast_code(ad1)?;
        // This conversion ran at the steady OSR of the rate (and not the first
        // one after the switch, which `osr` flags for the next reading).
        let slow = self.r22 == self.steady_osr() && !self.avg_skip;
        self.avg_skip = false;
        let r = self.range as usize;
        let (f, ov) = (self.factor(), self.over());
        let m = self.mode;
        let before = (m, self.range);
        // AC+DC: the OSR stays as write_config left it (ACDC_STOCK_OSR).
        let acdc = self.acdc;
        if !acdc {
            self.dc_rate_osr();
        }
        // After a range change: a conversion that does not agree with the
        // one before is held (VET_*; AC+DC: dc_out keeps the phase count).
        let vet = self.vet_ad1(ad1);
        let (x, lim, dec, unit, si_k, ext) = if m == 1 {
            let (code, ext) = if vet { self.avg_code(ad1, slow) } else { (ad1 as f32, false) };
            let mut v = code * K_ACV[r] * (1.25 / 0.9) / AD1_DIV * cal(0x04 + 4 * r as u32, 1.0)
                - cal(0x14 + 4 * r as u32, 0.0);
            if v < 0.0 {
                v *= DCV_NEGK[r];
            }
            if r == 0 && v.abs() / 20000.0 > 3e-5 {
                v += 1e-4 - v.abs() / 20000.0;
            }
            (v, (f * DEC4[r]).min(1000.0), 4 - r as u8, "V", 1.0, ext)
        } else {
            let (code, ext) = if vet { self.avg_code(ad1, slow) } else { (ad1 as f32, false) };
            let mv = code * (1.25 / 0.9) / MV_DIV[r] / (AD1_DIV / 1000.0) * cal(0x24 + 4 * r as u32, 1.0)
                - cal(0x2C + 4 * r as u32, 0.0);
            (mv, f * [10.0, 100.0][r], 3 - r as u8, "mV", 1e-3, ext)
        };
        // Autorange magnitude S+0x60: |DC|, AC+DC: hypot(AC raw, DC). A held
        // conversion steps up on an overload (a range too sensitive must not
        // wait), never down.
        let mag = if self.acdc { hypot(self.ac_raw, x) } else { x.abs() };
        let now = clk.now_ms();
        if m == 3 && acdc {
            // (AC+DC mV's V_GATE, `mv_vside`: the magnitude over the range)
            self.mv_dc_drv = mag > ov * lim;
        } else if m == 3 {
            // plain DC mV's V_GATE: over the range (OL) = a source on V-COM;
            // "free" needs two results in range in a row (`mv_dc_drv` = the
            // last one was over): one stray conversion (a row rewrite of the
            // A-PROT probe) read in range among OL ones in the emulator and
            // let A-PROT trip (loudtest ajack-dcmv-hf, 1 of 3 runs)
            let ol = mag > ov * lim;
            self.sent.v_side(now, ol || self.mv_dc_drv);
            self.mv_dc_drv = ol;
        }
        let hold = vet && self.acs_dc_step(now, x, lim);
        self.tr_dc(now, ad1, x, lim, vet, hold);
        let up = self.over_ok(mag > ov * lim, vet);
        if !self.manual {
            if m == 1 {
                if up && r < 3 {
                    self.set(clk, 1, r as i8 + 1, 250);
                } else if self.acdc_down(now, vet && !hold && mag < 0.1 * lim / ov && !self.am_on()) {
                    // (not in an AC+DC method's reference step: it is taken at
                    // the method's range, acdcm.rs)
                    if r > 0 {
                        self.set(clk, 1, r as i8 - 1, 250);
                    } else if self.v_auto & 2 != 0 {
                        self.set(clk, 3, 1, 450);
                    }
                }
            } else if up {
                if r == 0 {
                    self.set(clk, 3, 1, 250);
                } else if self.v_auto & 1 != 0 {
                    self.set(clk, 1, 0, 350);
                }
            } else if r == 1 && self.acdc_down(now, vet && !hold && mag < 0.1 * lim / ov && !self.am_on()) {
                // 600 mV -> 60 mV (opendm40; the stock steps mV only up and
                // leaves 60 mV to a manual choice): the stock's down rule of
                // every other range, below 0.1 x the range / over (58.3 mV;
                // back up above over x 60 mV = 61.8 mV).
                self.set(clk, 3, 0, 250);
            }
        }
        if !vet && !acdc && (self.mode, self.range) != before {
            return None;
        }
        // A held conversion (G2): provisional (dc_out: AC+DC its own way).
        self.dc_out(clk, x, lim, dec, unit, si_k, before, ext, vet).map(|mut rd| {
            rd.prov |= !vet;
            rd
        })
    }

    /// Trace a DC conversion (acdctr.rs); the trigger is an accepted DC
    /// part more than TR_JUMP off the DC part before it.
    fn tr_dc(&mut self, now: u64, ad1: i32, x: f32, lim: f32, vet: bool, hold: bool) {
        if !tr::ON || self.tr.on == 0 {
            return;
        }
        let d = self.dc_val;
        let jump = vet && !self.am_on() && d.abs() > 0.01 * lim && (x - d).abs() > tr::TR_JUMP * d.abs();
        let fl = vet as u8 * tr::F_VET | jump as u8 * tr::F_TRIG | hold as u8 * tr::F_HOLD;
        self.tr_ev(now, tr::K_DC, ad1, x, fl);
    }

    /// End of a DC reading (modes 1, 3, 7..9): plain DC, or the DC phase of
    /// AC+DC (FUN_08013734 cases 1, 3, 7..9): shown = hypot(AC filtered, DC),
    /// aux1/aux2 = AC/DC parts; after S+10 + 1 readings back to the AC mode
    /// (DCV / DCmV 800 ms, DCA 900 ms settle) with S+10 = 3 (ACV: 4 results)
    /// or 2 (ACmV, ACA: 3 results). A range / unit
    /// change in AC+DC gives no value this time (stock: OL, m+0xd8 = 0).
    /// `ext`: the value is averaged and shows the extra digit (not in AC+DC).
    /// `vet` false (AC+DC, `vet_ad1`): the result is held, it only counts
    /// for the phase. A DC part that moved by more than ACS_DC_STEP starts
    /// the AC part's settle (an input step).
    #[allow(clippy::too_many_arguments)]
    fn dc_out(
        &mut self,
        clk: &mut Clock,
        x: f32,
        lim: f32,
        dec: u8,
        unit: &'static str,
        si_k: f32,
        before: (u8, i8),
        ext: bool,
        vet: bool,
    ) -> Option<Reading> {
        let ov = self.over();
        // AC? counts V-tap windows only with a small DC part (sentinel::AC_HINT_DC_MAX).
        self.sent.ac_dc_ok = x.abs() < sentinel::AC_HINT_DC_MAX;
        if !self.acdc {
            let ol = x.abs() / lim > ov;
            let v = if ol { Val::ol(dec, unit) } else { Val::new(x, dec, unit).with_ext(ext) };
            return Some(self.reading(v, x * si_k));
        }
        if self.am_on() {
            return self.am_ref(clk, x, lim, dec, unit, si_k, before);
        }
        if (self.mode, self.range) != before {
            return None;
        }
        let now = clk.now_ms();
        if vet {
            self.acs_dc = x;
            self.dc_val = x;
            self.acdc_seen |= 2;
        }
        self.ac_settle_cap(now);
        let rd = if vet && self.acdc_seen == 3 {
            Some(self.acdc_reading(lim, dec, unit, si_k))
        } else {
            // G2 (a held DC result) / G4 (no AC part yet) / G5 (the AC part
            // settling: its latest result): provisional.
            if vet && self.cpl_auto {
                // (as acdc_reading did for every vetted result: AUTO's
                // part before its first decision, `auto_ac`)
                self.auto_shows_ac(lim);
            }
            let ac = if self.acdc_seen & 1 != 0 {
                self.f_ac
            } else if self.acs_t0 != 0 && self.acs_prev >= 0.0 {
                self.acs_prev
            } else {
                f32::NAN
            };
            Some(self.acdc_prov(ac, x, lim, dec, unit, si_k))
        };
        // A held result leaves no DC part yet (entry, a set from the AC
        // phase): one more DC result before the AC phase (at most
        // VET_HELD_MAX), else that phase would autorange on hypot(AC, 0).
        if self.phase_n == 0 && (vet || self.acdc_seen & 2 != 0) {
            let (to, settle, n) = match self.mode {
                1 => (0, 800, 3),
                3 => (2, 800, 2),
                dm => (dm - 3, 900, 2),
            };
            let r = self.range;
            self.switch_phase(clk, to, r, settle);
            self.phase_n = n;
        } else if self.phase_n > 0 {
            self.phase_n -= 1;
        }
        rd
    }

    fn poll_dca(&mut self, clk: &mut Clock) -> Option<Reading> {
        let ad1 = self.ad1()?;
        if self.r22_drop_one() {
            return None;
        }
        let ad1 = self.fast_code(ad1)?;
        // As in poll_dcv: a conversion at the steady OSR may join the window.
        let slow = self.r22 == self.steady_osr() && !self.avg_skip;
        self.avg_skip = false;
        if !self.acdc {
            self.dc_rate_osr();
        }
        // After a range change (VET_*); the OVERLOAD check below still sees
        // every conversion.
        let vet = self.vet_ad1(ad1);
        // FR-05 SLOW only (MEDIUM: no window in A, as the stock).
        let (code, _) = if vet { self.avg_code(ad1, slow) } else { (ad1 as f32, false) };
        let (m, r) = (self.mode, self.range as usize);
        let before = (m, self.range);
        let u = (m - 7) as usize; // 0 A, 1 mA, 2 uA
        let i = u * 2 + r;
        let raw = code * 1.25 / (SHUNT[u] * MV_DIV[r] * 0.9) / AD1_DIV * cal(0x94 + 4 * i as u32, 1.0)
            - cal(0xAC + 4 * i as u32, 0.0);
        let mut a = raw;
        if m == 7 && r == 1 && vet {
            ema(0.0007, 0.15, a, &mut self.f_dc);
            a = self.f_dc;
        }
        let lim = (self.factor() * DEC4[u + r]).min(if m == 7 { 10.0 } else { f32::MAX });
        let ov = self.over();
        let mag = if self.acdc { hypot(self.ac_raw, raw) } else { a.abs() };
        // OVERLOAD (doc 4.3): the uA / mA shunt over range -> 10 A at once
        // (a held conversion only as the second over-range one, `over_ok`).
        let over = mag > ov * lim;
        let up = self.over_ok(over, vet);
        if self.ovl_trip(clk, u as u8, up) {
            return None;
        }
        let hold = vet && self.acs_dc_step(clk.now_ms(), a, lim);
        self.tr_dc(clk.now_ms(), ad1, raw, lim, vet, hold);
        if !self.manual {
            let lowest = if self.a_auto == 3 { 9 } else { 8 };
            if over {
                if !up {
                    // held over-range conversion: no move either way
                } else if r == 0 {
                    self.set(clk, m, 1, 250);
                } else if m > 7 && self.a_auto & 1 != 0 {
                    self.set(clk, m - 1, 1, 250);
                }
            } else if self.acdc_down(clk.now_ms(), vet && !hold && mag < 0.1 * lim / ov) {
                if r == 1 {
                    self.set(clk, m, 0, 250);
                } else if m < lowest && self.ovl_allows(m + 1 - 7) {
                    self.set(clk, m + 1, 0, 350);
                }
            }
        }
        if !vet && !self.acdc && (self.mode, self.range) != before {
            return None;
        }
        let (unit, dec, si_k) = match u {
            0 => ("A", 4 - r as u8, 1.0),
            1 => ("mA", 3 - r as u8, 1e-3),
            _ => ("\u{b5}A", 2 - r as u8, 1e-6),
        };
        let mut rd = self.dc_out(clk, a, lim, dec, unit, si_k, before, false, vet)?;
        // A held conversion (G2): provisional.
        rd.prov |= !vet;
        if self.burden_show && !self.acdc {
            // Burden voltage, signed (doc 3.15): aux1 is free in DC A.
            rd.aux1 = Some(self.burden_val(u, r, a * si_k, rd.main.ol));
        }
        if m == 8 && !self.acdc {
            // 4-20 mA loop percentage.
            let p = (a.abs() - 4.0) * 6.25;
            rd.aux2 = Some(if p > 125.0 { Val::ol(1, "%") } else { Val::new(p, 1, "%") });
        }
        Some(rd)
    }

    /// AC+DC reading (either phase): hypot(AC filtered, DC part), parts in
    /// aux1 / aux2, reported with the DC mode. AUTO coupling: main = the
    /// part it shows (`auto_shows_ac`), reported with that part's mode.
    fn acdc_reading(&mut self, lim: f32, dec: u8, unit: &'static str, si_k: f32) -> Reading {
        let dm = dc_mode_of(self.mode);
        let t = hypot(self.f_ac, self.dc_val);
        let (v, mode) = if !self.cpl_auto {
            (t, dm)
        } else if self.auto_shows_ac(lim) {
            (self.f_ac, ac_mode_of(dm))
        } else {
            (self.dc_val, dm)
        };
        self.auto_total_si = t * si_k;
        #[cfg(feature = "acdc-meth")]
        {
            self.am.n_rd = self.am.n_rd.wrapping_add(1);
        }
        let ol = v.abs() / lim > self.over();
        let mut rd = self.reading(if ol { Val::ol(dec, unit) } else { Val::new(v, dec, unit) }, v * si_k);
        rd.mode = mode;
        rd.aux1 = Some(part(self.f_ac, lim, dec, unit));
        rd.aux2 = Some(part(self.dc_val, lim, dec, unit));
        // G7: AUTO before its first decision with an AC part above the
        // floor (auto_eval waits for it to settle on one side): provisional.
        let fl = if self.mode <= 3 { AUTO_FLOOR_V } else { AUTO_FLOOR_A };
        rd.prov = self.cpl_auto && self.auto_n == 0 && self.f_ac > fl[0] * lim;
        self.prov_side = rd.prov;
        rd
    }

    /// A provisional AC+DC / AUTO reading (`Reading::prov`) from this AC
    /// result / DC part (`ac` NaN: not measured yet, dashes in its row);
    /// changes no state. AUTO shows the part `auto_ac` names.
    fn acdc_prov(&self, ac: f32, dc: f32, lim: f32, dec: u8, unit: &'static str, si_k: f32) -> Reading {
        let dm = dc_mode_of(self.mode);
        let (v, mode) = if !self.cpl_auto {
            (hypot(if ac.is_nan() { 0.0 } else { ac }, dc), dm)
        } else if self.auto_ac && !ac.is_nan() {
            (ac, ac_mode_of(dm))
        } else {
            (dc, dm)
        };
        let ol = v.abs() / lim > self.over();
        let mut rd = self.reading(if ol { Val::ol(dec, unit) } else { Val::new(v, dec, unit) }, v * si_k);
        rd.mode = mode;
        rd.aux1 = Some(part(ac, lim, dec, unit));
        rd.aux2 = Some(part(dc, lim, dec, unit));
        rd.prov = true;
        rd
    }

    /// AUTO: the part to show now (true = AC) for range full scale `lim`:
    /// the decided one, or before the first evaluation of this session the
    /// DC, AC only above the floor with no DC part (|DC| below the off
    /// floor: no first AC part's overshoot shown). Sets `auto_ac`.
    fn auto_shows_ac(&mut self, lim: f32) -> bool {
        if self.auto_n == 0 {
            let fl = if self.mode <= 3 { AUTO_FLOOR_V } else { AUTO_FLOOR_A };
            self.auto_ac = self.f_ac > fl[0] * lim && self.dc_val.abs() < fl[1] * lim;
        }
        self.auto_ac
    }

    /// AUTO: one evaluation per AC+DC cycle (end of the AC phase, both parts
    /// fresh; constants AUTO_*): the first decides, later ones switch the
    /// part after AUTO_VOTES cycles in a row wanted the other one.
    fn auto_eval(&mut self, lim: f32, now: u64) {
        let fl = if self.mode <= 3 { AUTO_FLOOR_V } else { AUTO_FLOOR_A };
        let (a, d) = (self.f_ac, self.dc_val.abs());
        if self.auto_n == 0 {
            // Wait for the AC part to settle on one side (AUTO_SETTLE_K);
            // DC when it does not within AUTO_TIE_MS.
            if a > fl[0] * lim {
                let settled = self.auto_t0 != 0
                    && AUTO_SETTLE_K * (a - self.auto_prev).abs() < (a - d).abs()
                    && (a - d).abs() > AUTO_TIE * d;
                self.auto_prev = a;
                if self.auto_t0 == 0 {
                    self.auto_t0 = now.max(1);
                }
                let late = now.saturating_sub(self.auto_t0) >= AUTO_TIE_MS;
                if !settled && !late {
                    self.auto_shows_ac(lim);
                    return;
                }
                self.auto_ac = settled && a > d;
            } else {
                self.auto_ac = false;
            }
            self.auto_t0 = 0;
            self.auto_n = 1;
            self.auto_votes = 0;
            return;
        }
        self.auto_n = self.auto_n.saturating_add(1);
        let want = if self.auto_ac { a >= fl[1] * lim && a >= AUTO_TO_DC * d } else { a > fl[0] * lim && a > AUTO_TO_AC * d };
        if want == self.auto_ac {
            self.auto_votes = 0;
        } else {
            self.auto_votes += 1;
            if self.auto_votes >= AUTO_VOTES {
                self.auto_ac = want;
                self.auto_votes = 0;
            }
        }
    }

    fn poll_ac(&mut self, clk: &mut Clock, now: u64) -> Option<Reading> {
        let m = self.mode;
        let need = if m <= 2 { hy::INTF_RMS | hy::INTF_LPF } else { hy::INTF_RMS };
        let intf = hy::read1(hy::R_INTF);
        // Plain AC V / mV after a reset (`ac_lead`): no result latched during
        // the settle, nor the one integrated across its end; the next seeds.
        // (AC A: its own vet, ACA_VET_*: two results in a row must agree.)
        let lead = self.ac_lead;
        if lead == 2 || (lead == 1 && intf & need == need) {
            self.ac_lead = lead - 1;
            return None;
        }
        if intf & need != need {
            return None;
        }
        let mut b = [0u8; 20];
        hy::read(hy::R_AD1, &mut b);
        if matches!(self.sent.kind, Kind::PeakA | Kind::PeakV) {
            // A-JACK on AD1's peak hold (PB1 - PB3; V-JACK? in AC A: SENSE
            // - PB5): PKHMIN / PKHMAX came with this read; restart the hold
            // for the next result.
            self.sent.sample_peak(now, hy::s24(&b[14..17]), hy::s24(&b[17..20]));
            if self.sent.alt {
                self.dc_hint_window(now);
            } else if PKH_RESTART {
                hy::write(0x29, &[self.r29 & !sentinel::R29_ENPKH]);
                hy::write(0x29, &[self.r29]);
            }
        }
        let rms = (b[9] as u64) | (b[10] as u64) << 8 | (b[11] as u64) << 16 | (b[12] as u64) << 24
            | ((b[13] & 0x1F) as u64) << 32;
        let sq = sqrt(rms as f32);
        let lpf = (((b[6] as u32) << 13 | (b[7] as u32) << 21 | (b[8] as u32) << 29) as i32 >> 13) as f32;
        let r = self.range as usize;
        let (f, ov) = (self.factor(), self.over());
        let (s, ci, g_rms, o_rms, g_lpf, o_lpf) = match m {
            0 => (K_ACV[r] * 1.25 / 262_144.0, r, 0x34, 0x44, 0x64, 0x74),
            2 => (1.25 / MV_DIV[r] / 262.144, r, 0x54, 0x5C, 0x84, 0x8C),
            _ => {
                let u = (m - 4) as usize;
                (1.25 / (SHUNT[u] * MV_DIV[r]) / 262_144.0, u * 2 + r, 0xC4, 0xDC, 0xF4, 0x10C)
            }
        };
        let ci = ci as u32;
        let mut ac = sq * s * cal(g_rms + 4 * ci, 1.0) - cal(o_rms + 4 * ci, 0.0);
        let dcp = lpf * s * cal(g_lpf + 4 * ci, 1.0) - cal(o_lpf + 4 * ci, 0.0);
        let over = sq > 262_144.0;
        if m == 2 && self.sent.vgate {
            // (over the converter range: NaN, no verdict but the PB1 one)
            self.mv_vside(now, dcp.abs(), if over { f32::NAN } else { core::f32::consts::SQRT_2 * sq * s });
        }
        if over {
            return None;
        }
        if self.diag_on {
            if !self.acdc && self.diag[self.diag_k as usize % ACDC_DIAG_N].n >= 4 {
                self.diag_k = self.diag_k.wrapping_add(1);
                self.diag[self.diag_k as usize % ACDC_DIAG_N] = AcPhaseDiag::EMPTY;
            }
            let e = &mut self.diag[self.diag_k as usize % ACDC_DIAG_N];
            let j = e.n as usize;
            if j == 0 {
                e.late_ms = now.saturating_sub(self.deadline).min(u16::MAX as u64) as u16;
            }
            if j < 4 {
                let t0 = self.diag_t;
                e.sq[j] = sq as u32;
                e.lpf[j] = lpf as i32;
                e.dcp[j] = dcp;
                e.dt[j] = now.saturating_sub(t0).min(u16::MAX as u64) as u16;
                e.fl[j] = intf as u16 | if self.diag_ctr > 0 { 0x100 } else { 0 };
                e.n += 1;
            }
            self.diag_t = now;
            self.diag_ctr = 0;
        }
        if ac < 0.0 {
            ac = 0.0;
        }
        let (thr, a_rms, a_lpf) = match m {
            0 => (ACV_WIN[r], 0.25, 0.25),
            2 => (ACMV_WIN[r], 0.35, 0.35),
            _ => (ACA_WIN[(m - 4) as usize * 2 + r], 0.15, 0.35),
        };
        let lim = match m {
            0 => (f * DEC4[r]).min(1000.0),
            2 => f * [10.0, 100.0][r],
            _ => (f * DEC4[(m - 4) as usize + r]).min(if m == 4 { 10.0 } else { f32::MAX }),
        };
        // Plain AC A: over-range results within AC_OVER_HOLD_MS of the
        // first result after a row write are held back (no reading, no range
        // move, no OVERLOAD / AUTO hop): device 40728b8, CONF:CURR:AC 10 then
        // 0.06 with the A jack empty read OL on the first 60 mA result(s)
        // (the 10 mohm -> 0.5 ohm shunt, PB2 -> PB1 and x1 -> x10 step
        // through the RMS / LPF blocks) and the OVERLOAD rule fell back to
        // 10 A. A real over range still reads over after the hold and acts
        // then; one that starts later acts at once.
        if m >= 4 && !self.acdc {
            if self.ac_over_until == 0 {
                self.ac_over_until = now + AC_OVER_HOLD_MS;
            }
            if now < self.ac_over_until && sqrt(ac * ac + 0.25 * dcp * dcp) > ov * lim {
                return None;
            }
        }
        // Plain AC A after a row write: held (provisional) until two results
        // in a row agree (ACA_VET_*). Plain AC V after a range step: until
        // the node's DC has settled (ACV_DC_*).
        if self.acv_vet && (dcp.abs() <= ACV_DC_K * ac + ACV_DC_FS * lim || now >= self.aca_vet_t + ACA_VET_MAX_MS) {
            self.acv_vet = false;
        }
        let held = !self.acdc && (self.acv_vet || (m >= 4 && !self.aca_vet(now, ac, lim)));
        // Plain AC after a reset: the first used result seeds the filters.
        let first = !self.acdc && !held && core::mem::take(&mut self.ac_seed);
        if first {
            self.f_lpf = dcp;
        } else if !held {
            ema(thr, a_lpf, dcp, &mut self.f_lpf);
        }
        let (unit, dec, si_k) = match m {
            0 => ("V", 4 - r as u8, 1.0),
            2 => ("mV", 3 - r as u8, 1e-3),
            4 => ("A", 4 - r as u8, 1.0),
            5 => ("mA", 3 - r as u8, 1e-3),
            _ => ("\u{b5}A", 2 - r as u8, 1e-6),
        };
        let acdc = self.acdc;
        // AC+DC after a range change or an input step: the AC part settles
        // first (ACS_STEP); its results are discarded until then (the first
        // good one seeds the EMA), and they move no range. A result far
        // above the AC part (ACS_JUMP) is such a step seen in the AC phase.
        if acdc && self.acs_t0 == 0 && self.acdc_seen & 1 != 0 && ac > ACS_JUMP * self.f_ac + ACS_JUMP_FLOOR * lim {
            self.acs_begin(now, true);
        }
        let was = self.acs_t0 != 0;
        let settling = acdc && self.ac_settle_step(now, ac, lim);
        if tr::ON && self.tr.on != 0 {
            self.tr_ev(now, tr::K_AC, sq as i32, ac, settling as u8 * tr::F_SETTLING);
        }
        let seed = (acdc && was && !settling) || first;
        // AC+DC: every other result after the settle deadline goes through
        // the EMA (thr = the plain-AC window, a 0.25 V / 0.35 mV / 0.15 A),
        // as the stock (stock/re/docs/history/acdc-stock-vs-opendm40.md 8.2).
        if seed {
            self.ac_raw = ac;
            self.f_ac = ac;
        } else if !settling && !held {
            self.ac_raw = ac;
            ema(thr, a_rms, ac, &mut self.f_ac);
        }

        // AC+DC: hypot(AC, last DC part) for every quantity.
        let mag = if acdc {
            hypot(ac, self.dc_val)
        } else if m <= 2 {
            ac
        } else {
            sqrt(ac * ac + 0.25 * dcp * dcp)
        };
        let before = (m, self.range);
        // OVERLOAD (doc 4.3): the uA / mA shunt over range -> 10 A at once.
        if m >= 4 && !settling && self.ovl_trip(clk, m - 4, mag > ov * lim) {
            return None;
        }
        if !self.manual && !settling {
            let up = mag > ov * lim;
            // AC+DC: no step down here, only in the DC phase (ACDC_DOWN_MS).
            let down = mag < 0.1 * lim / ov && !acdc && !held;
            let mv_hold = m == 0 && r == 0 && !acdc && self.mv_clip_holds(now, ac);
            match m {
                // AC+DC: a V / mV range change is applied through the DC
                // phase (600 ms, S+10 = 2); unit changes go to the DC mode.
                0 => {
                    if (up && r < 3) || (down && r > 0) {
                        let to = if up { r as i8 + 1 } else { r as i8 - 1 };
                        if acdc {
                            self.switch_phase(clk, 1, to, 600);
                            self.phase_n = 2;
                        } else {
                            self.set(clk, 0, to, 450);
                        }
                    } else if down && self.v_auto & 2 != 0 && !mv_hold {
                        self.set(clk, if acdc { 3 } else { 2 }, 1, 600);
                    }
                }
                2 => {
                    // The input clipped (MV_CLIP_K): over range whatever
                    // the RMS says.
                    let clip = !acdc && dcp.abs() + core::f32::consts::SQRT_2 * ac > MV_CLIP_K * lim;
                    let up = up || clip;
                    if clip && r == 1 {
                        self.mv_clip = -1.0;
                    }
                    if up && r == 0 {
                        if acdc {
                            self.switch_phase(clk, 3, 1, 600);
                            self.phase_n = 2;
                        } else {
                            self.set(clk, 2, 1, 450);
                        }
                    } else if up && self.v_auto & 1 != 0 {
                        self.set(clk, acdc as u8, 0, 350);
                    } else if down && r == 1 {
                        // 600 mV -> 60 mV, as DC mV (poll_dcv); AC+DC steps
                        // down in its DC phase only (`down` is false here).
                        self.set(clk, 2, 0, 450);
                    }
                }
                _ => {
                    if up && r == 0 {
                        self.set(clk, m, 1, 550);
                    } else if up && m > 4 && self.a_auto & 1 != 0 {
                        self.set(clk, m - 1, 0, 650);
                    } else if down && r == 1 {
                        self.set(clk, m, 0, 550);
                    } else if down && r == 0 && m < if self.a_auto == 3 { 6 } else { 5 } && self.ovl_allows(m + 1 - 4) {
                        self.set(clk, m + 1, 1, 650);
                    }
                }
            }
        }
        // A plain autorange step: `set` reset the AC filters (a range step,
        // or `reset_values` for a unit change), so `f_ac` below would be 0 on
        // the new range; no reading until the new row's first result.
        if !acdc && (self.mode, self.range) != before {
            return None;
        }
        if acdc {
            // AC phase of AC+DC (FUN_08013734 cases 0, 2, 4..6): shown =
            // hypot(AC filtered, DC part) on every result. After S+10 + 1
            // results (ACV 4, ACmV / ACA 3) to the DC mode (ACV 600 ms + R32 =
            // 0x38, ACmV 500 ms, ACA 700 ms) for 3 results (S+10 = 2; the DC
            // part is not averaged in AC+DC, the last one is shown).
            if (self.mode, self.range) != before {
                return None;
            }
            // Settling: AC+DC has no reading (`ac_settle_start`); AUTO
            // shows its part with the AC part held.
            if !settling {
                self.acdc_seen |= 1;
            }
            if self.acdc_seen != 3 || settling {
                // No reading before both parts were measured.
            } else if self.cpl_auto && self.phase_n == 0 {
                self.auto_eval(lim, now);
            }
            let rd = if self.acdc_seen == 3 {
                Some(self.acdc_reading(lim, dec, unit, si_k))
            } else if self.acdc_seen & 2 != 0 {
                // G5: the AC part settling (its latest result), provisional.
                Some(self.acdc_prov(ac, self.dc_val, lim, dec, unit, si_k))
            } else {
                None
            };
            if ACDC_FREQ_RESTART && m >= 4 {
                self.freq_secondary(now, 0xF0);
            }
            if self.phase_n == 0 {
                let r = self.range;
                self.switch_phase(clk, if m <= 2 { m + 1 } else { m + 3 }, r, match m {
                    0 => 600,
                    2 => 500,
                    _ => 700,
                });
                if m == 0 {
                    hy::write(0x32, &[0x38]);
                    self.tr_ev(now, tr::K_R32, 0, 0.0, 0);
                }
                self.phase_n = 2;
            } else {
                self.phase_n -= 1;
            }
            return rd;
        }
        if held {
            if (self.mode, self.range) != before {
                return None;
            }
            let mut rd = self.reading(if ac / lim > ov { Val::ol(dec, unit) } else { Val::new(ac, dec, unit) }, ac * si_k);
            rd.prov = true;
            return Some(rd);
        }
        let ol = self.f_ac / lim > ov;
        let v = self.f_ac;
        let mut rd = self.reading(if ol { Val::ol(dec, unit) } else { Val::new(v, dec, unit) }, v * si_k);
        self.freq_secondary(now, 0xF0);
        let (fv, dv) = self.freq_vals(true);
        rd.aux1 = Some(fv);
        // AC A: the burden voltage (RMS, doc 3.15) instead of the duty cycle.
        rd.aux2 = Some(if m >= 4 && self.burden_show { self.burden_val((m - 4) as usize, r, v * si_k, ol) } else { dv });
        Some(rd)
    }

    /// Burden voltage readout (doc 3.15): `amps` (signed DC / RMS) x the
    /// R_path of shunt `u` (0 A, 1 mA, 2 uA; the power meter's setting,
    /// POW:BURD, default the bare shunt) on range `r`. The unit and decimals
    /// follow the range's burden at the OL limit (stable while the reading
    /// moves, at most 4 digits: the AC slot's box holds 5 characters):
    /// < 0.1 V mV x.xx, < 1 V mV x.x, < 10 V V x.xxx, < 100 V V x.xx.
    fn burden_val(&self, u: usize, r: usize, amps: f32, ol: bool) -> Val {
        let rp = self.pw.r_path[u.min(2)];
        let fs = sentinel::current_fs(u as u8, r as u8) * rp * self.over();
        let (k, dec, unit) = if fs < 0.1 {
            (1000.0, 2, "mV")
        } else if fs < 1.0 {
            (1000.0, 1, "mV")
        } else if fs < 10.0 {
            (1.0, 3, "V")
        } else if fs < 100.0 {
            (1.0, 2, "V")
        } else {
            (1.0, 1, "V")
        };
        if ol { Val::ol(dec, unit) } else { Val::new(amps * rp * k, dec, unit) }
    }

    /// FETC:BURD?: the burden voltage (V) of a reading of a current function
    /// (DC signed, AC / AC+DC the RMS) with the R_path of its shunt; None
    /// outside the A functions, in the power meter or over range. AUTO
    /// coupling: the total RMS (AC+DC) as in AC+DC, not the part shown
    /// (that is what drops across the shunt; a mixed signal's shown part
    /// understated it), over range by the total; `auto_total_si` belongs
    /// to the latest reading, which `r` is (app.rs `last_raw`).
    #[inline(never)] // .cold (link-native.x): SCPI only
    pub fn burden_of(&self, r: &Reading) -> Option<f32> {
        let u = sentinel::shunt_of(r.mode).filter(|_| r.power.is_none() && !r.main.ol)?;
        let i = if r.cpl_auto {
            let t = self.auto_total_si;
            if t > sentinel::current_fs(u, r.range.max(0) as u8) * self.over() {
                return None;
            }
            t
        } else {
            r.si
        };
        Some(i * self.pw.r_path[u as usize])
    }

    fn poll_ohm(&mut self, clk: &mut Clock, now: u64) -> Option<Reading> {
        let ad1 = self.ad1()?;
        if self.r22_drop_one() {
            return None;
        }
        // Autorange r0..r6, r1..r6 with the "ON" variant (`ohm_auto_floor`).
        let r = self.range.clamp(0, 6) as usize;
        let r22 = hy::read1(0x22);
        let steady = self.steady_osr();
        let slow = r22 == steady && !self.avg_skip;
        self.avg_skip = false;
        // LIVE?: a passive Rx never gives a negative code; a source of the
        // other polarity does (0.24 % of full scale: far above the noise).
        self.live = ad1 < -LIVE_CODE;
        let g = ohm_row_gain(r, cal(0x124 + 4 * r as u32, 1.0), cal(0x128, 1.0), cal(0x24, 1.0), cal(0x28, 1.0)) * OHM_RREF[r] / 0.9 / AD1_DIV;
        let k0 = cal(0x144, 0.0);
        let lim = self.factor() * OHM_DEC[r];
        let ov = self.over();
        // Vet this conversion (OHM_VET_*) on its raw value, before the
        // average: a provisional one starts the average over.
        let raw = ad1.unsigned_abs() as i32;
        let kraw = (raw as f32 * g - k0).max(0.0);
        let x = if kraw / lim > ov { f32::INFINITY } else { kraw };
        let p = core::mem::replace(&mut self.ohm_prev, x);
        if (x.is_infinite() && p.is_infinite()) || (x - p).abs() <= OHM_VET_REL * p.abs() + OHM_VET_CNT * lim / 60000.0 {
            self.ohm_ok = true;
            self.ohm_agreed = true;
        } else if now >= self.ohm_t0 + OHM_VET_MAX_MS {
            self.ohm_ok = true;
        }
        let prov = !self.ohm_ok;
        let (code, ext) = if prov {
            self.avg.reset();
            (raw as f32, false)
        } else {
            self.avg_code(raw, slow)
        };
        // Below 0 only by the offset k0 at most (`raw` is unsigned), a few
        // tens of mohm: shown with `ohm_neg`, else clamped as the stock.
        // The range logic below works on the clamped value.
        let kneg = code * g - k0;
        let kohm = kneg.max(0.0);
        let ol = kohm / lim > ov;
        if ol {
            if r22 != 0x13 {
                hy::write(0x22, &[0x13]);
                self.r22_dropping();
            }
        } else if r22 != steady && !(r22 == OSR_EARLY && self.r22_early()) {
            // Within R22_DROP_WIN_MS of the row write the in-range OSR is
            // OSR_EARLY (two dropped conversions of 100 ms, not of 400 ms at
            // MED / SLOW); the steady one after it (R22_LATE).
            let steady = if self.r22_early() { OSR_EARLY } else { steady };
            hy::write(0x22, &[steady]);
            self.deadline = now + 150;
            self.avg_skip = true;
            self.r22_dropping();
        }
        if !self.manual {
            let q = ov * lim / (kohm + 1e-5);
            // OL on a row a step down reached (OHM_BAND): the next
            // conversion confirms it (until then it is grey, as any first
            // one), then back to the range it came from, held there.
            let back = (self.ohm_down_r > r as i8).then_some((self.ohm_down_r, self.ohm_down_k));
            let held = self.ohm_blk_r == r as i8 && kohm * OHM_BAND >= self.ohm_blk_k && kohm <= self.ohm_blk_k * OHM_BAND;
            if q < 1.0 {
                if let Some((to, k)) = back {
                    if self.ohm_ok {
                        self.ohm_settled = false;
                        self.set(clk, 10, to, 150);
                        self.ohm_blk_r = to;
                        self.ohm_blk_k = k;
                        return None;
                    }
                } else if r < 6 {
                    let to = if self.ohm_settled { 6 } else { r as i8 + 1 };
                    self.ohm_settled = false;
                    self.set(clk, 10, to, 150);
                    return None;
                }
            } else if q > 10.0 * ov * ov && r as i8 > self.ohm_auto_floor() && !held {
                // Down below 0.1 * lim / over, as V / A. The stock tests
                // q > 10 (R < over * lim / 10), which is exactly the next lower
                // range's up threshold: no hysteresis band at all. 600 -> 60
                // ohm (not in the stock autorange) by the same rule: down
                // below 58.25 ohm, up above 61.8 ohm (DM40C, over 1.03).
                let step = if now < self.entry + OHM_SINGLE_MS {
                    1
                } else if q > 1e5 {
                    5
                } else if q > 1e4 {
                    4
                } else if q > 1e3 {
                    3
                } else if q > 100.0 {
                    2
                } else {
                    1
                };
                self.ohm_settled = false;
                self.set(clk, 10, (r as i8 - step).max(self.ohm_auto_floor()), 150);
                self.ohm_down_r = r as i8;
                self.ohm_down_k = kohm;
                return None;
            } else {
                // In range (or held, OHM_BAND). The first such conversion
                // is the vet's reference (grey), no longer a 150 ms wait
                // (the vet needs the next conversion anyway).
                self.ohm_settled = true;
                self.ohm_down_r = -1;
            }
        }
        let kohm = if self.ohm_neg { kneg } else { kohm };
        // Stock formats (r < 2: x1000 ohm, fmt r + 1): r0 60.000 ohm.
        let (v, dec, unit) = match r {
            0 | 1 => (kohm * 1000.0, 3 - r as u8, "\u{3a9}"),
            2..=4 => (kohm, 6 - r as u8, "k\u{3a9}"),
            _ => (kohm * 1e-3, 9 - r as u8, "M\u{3a9}"),
        };
        let val = if ol { Val::ol(dec, unit) } else { Val::new(v, dec, unit).with_ext(ext) };
        let mut rd = self.reading(val, kohm * 1e3);
        // Provisional: drawn grey; the sentinel / watchdog still see it
        // (`prov_side`, as AUTO's G7), only the output is provisional.
        rd.prov = prov;
        self.prov_side = prov;
        Some(rd)
    }

    /// Ohm r7, the experimental 600 Mohm (`OHM600`): one phase step per AD1
    /// result, a reading when both phases are in (~2 s at skip 2, keep 3).
    fn poll_ohm600(&mut self, clk: &mut Clock) -> Option<Reading> {
        if !self.manual {
            // Not in the autorange (whatever released the manual range).
            self.set(clk, 10, OHM_AUTO_TOP, 150);
            return None;
        }
        let code = self.ad1()?;
        let mut done = None;
        if let Some(r32) = self.o6.step(code, &mut done) {
            // OP1's input for the next phase, the conversion restarted.
            let r23 = OHM600_ROW[3];
            hy::write(0x23, &[r23 & !0x80]);
            hy::write(0x32, &[r32]);
            hy::write(0x23, &[r23]);
        }
        let (a, b) = done?;
        self.o6.learn(a, b);
        // LIVE?: the jack below COM, or driven above OP3's side of Rref.
        self.live = a < -LIVE_CODE || b - a < -LIVE_CODE;
        // Final from the first reading on: each one is already the median of
        // 3 results per phase after the switch transient (device 2026-10-28:
        // the first 5.568 Mohm, then 5.569 .. 5.571), and a provisional one
        // never reaches SCPI (a mains-noisy input restarting the average
        // would have kept it so).
        let (val, si, prov) = match self.o6.kohm(a, b, self.over()) {
            Some(k) => {
                let v = self.o6.average(k);
                let dec = self.o6.decimals(v * 1e-3);
                let ext = self.o6.ext(dec);
                (Val::new(v * 1e-3, dec, "M\u{3a9}").with_ext(ext), v * 1e3, false)
            }
            None => {
                self.o6.hn = 0;
                (Val::ol(1, "M\u{3a9}"), 2.0 * OHM600_FS * 1e3, false)
            }
        };
        Some(self.ohm600_reading(val, si, prov))
    }

    /// DBG:OHM6 (Some: set the fields that are numbers) / DBG:OHM6?.
    /// Firmware only (the host tests that include dmm.rs have no scpi).
    #[cfg(target_os = "none")]
    #[inline(never)] // .cold (link-native.x): one SCPI command
    #[cfg_attr(not(feature = "dbg-ohm6"), allow(dead_code))]
    pub fn ohm600_dbg(&mut self, set: Option<[f32; 5]>, out: &mut crate::scpi::Scpi) {
        let o = &mut self.o6;
        if let Some(v) = set {
            if v[0] > 0.5 && v[0] < 2.0 {
                o.gain = v[0];
            }
            if v[1].is_finite() {
                o.off = v[1] * 1e-6;
            }
            if v[2] >= 0.0 && v[2] <= 8.0 {
                o.skip = v[2] as u8;
            }
            if v[3] >= 1.0 && v[3] <= 8.0 {
                o.keep = v[3] as u8;
            }
            // A leak value freezes the learning; a negative one restarts it.
            if v[4] < 0.0 {
                o.learn_on = true;
                o.leak = 0.0;
                o.leak_n = 0;
            } else if v[4].is_finite() {
                o.leak = v[4] * 1e-9;
                o.leak_n = 1;
                o.learn_on = false;
            }
            // The phase under way starts over (OP1's input stays), the
            // average too.
            o.n = 0;
            o.hn = 0;
            return;
        }
        let (a, b) = o.last;
        for (i, x) in [o.gain, o.off * 1e6, o.skip as f32, o.keep as f32, o.leak * 1e9, a as f32 * OHM600_V, b as f32 * OHM600_V].into_iter().enumerate() {
            if i > 0 {
                out.put_bytes(b",");
            }
            out.put_f32(x);
        }
    }

    fn ohm600_reading(&mut self, val: Val, si: f32, prov: bool) -> Reading {
        let mut rd = self.reading(val, si);
        rd.prov = prov;
        self.prov_side = prov;
        rd
    }

    fn poll_cont(&mut self) -> Option<Reading> {
        let ad1 = self.ad1()?;
        self.osr(ad1, 0x206, 0x14);
        // Negative results (a source, a diode the wrong way or a charged
        // capacitor on the jacks) clamp to 0 as the stock (dmm_v2_6.h m+0x18:
        // only its raw mode skips the clamp); emulator cross table.
        let ohm = -(ad1 as f32) * cal(0x15C, 1.0) / 0.9 / (AD1_DIV / 1000.0) - cal(0x160, 0.0);
        self.live = ohm < -LIVE_CONT_OHM;
        let ohm = ohm.max(0.0);
        let ol = ohm > 1000.0;
        Some(self.reading(if ol { Val::ol(2, "\u{3a9}") } else { Val::new(ohm, 2, "\u{3a9}") }, ohm))
    }

    fn poll_diode(&mut self, now: u64) -> Option<Reading> {
        let ad1 = self.ad1()?;
        let g = cal(0x164, 1.0);
        let mut v = ad1 as f32 * g * 1.25 * 10.0 / 0.9 / AD1_DIV - cal(0x168, 0.0);
        self.live = v < -LIVE_DIODE_V;
        if v < 0.0 {
            v = 0.0;
        }
        ema(0.0012, 0.05, v, &mut self.f_diode);
        let v = self.f_diode;
        let ol = v / 3.0 > 1.04;
        let class = if v < 0.022 {
            0
        } else if v < 0.45 {
            1
        } else if v < 0.75 {
            2
        } else if v < 1.5 {
            3
        } else {
            4
        };
        if class != self.beep_class {
            self.beep_class = class;
            self.beep_e = 0;
            self.beep_n = 0;
            self.beep_next = now;
        } else {
            self.beep_n = self.beep_n.saturating_add(1);
        }
        let mut rd = self.reading(if ol { Val::ol(4, "V") } else { Val::new(v, 4, "V") }, v);
        // Aux "ohm" readout (stock m+0xdc = V*gain/0.765, shown only below 1.04).
        let k = v * g / 0.765;
        rd.aux2 = Some(if k < 1.04 { Val::new(k * 1000.0, 1, "\u{3a9}") } else { Val::ol(1, "\u{3a9}") });
        Some(rd)
    }

    /// The continuity / diode beeper sounds now (or would, at volume 0 % /
    /// `mute`): 1 = a short (continuity's comparator, diode's continuous
    /// tone below 22 mV), 2 = a diode pattern's on phase, 0 = silent (and
    /// in every other function, while the source (DBG:SRC) or the power
    /// meter has the row).
    pub fn beeping(&self) -> u8 {
        if !self.beep_on || self.pw.on || src::active() || self.suspended {
            return 0;
        }
        match self.mode {
            11 => 1,
            12 if self.beep_class == 0 => 1,
            12 => 2,
            _ => 0,
        }
    }

    /// Diode beep patterns through the HY3131 CMP pin buffer (R20 0xb4 / 0xb0).
    fn diode_beep(&mut self, now: u64) {
        if self.mode != 12 || now < self.beep_next || src::active() {
            return;
        }
        let (on, step, max_e) = match self.beep_class {
            0 => (true, 50, 0),
            1 if self.beep_n <= 6 => (self.beep_e & 1 == 0, 50, 4),
            2 if self.beep_n <= 6 => (self.beep_e & 1 == 0, 100, 2),
            3 if self.beep_n <= 6 => (self.beep_e & 1 != 0, 25, 7),
            _ => (false, 100, 0),
        };
        hy::write(0x20, &[if on { 0xB4 } else { 0xB0 }]);
        self.beep_on = on;
        if self.beep_e < max_e {
            self.beep_e += 1;
        }
        self.beep_next = now + step;
    }

    /// Read the counter and restart it with `src` (the stock's blind read:
    /// capacitance, AC+DC).
    fn read_counters(&mut self, src: u8) -> (f32, f32) {
        let mut b = [0u8; 9];
        hy::read(0x15, &mut b);
        self.diag_ctr = self.diag_ctr.saturating_add(1);
        ctr_restart(src);
        counter_vals(u24(&b[0..3]), u24(&b[3..6]), u24(&b[6..9]))
    }

    /// A counter check (`gate_check`) at most every FREQ_RECHECK_MS from
    /// FREQ_CHECK_MS after the restart: None while the gate runs; at its end
    /// or timeout the counts (f kHz, duty %, its SYSCLK ticks, the check:
    /// `Gate::Done` = a whole gate) and a restart with `src`.
    ///
    /// `hf_t`: a gate still open at a check had no input edges since its
    /// first 53.3 ms (any edge after the CTA overflow ends it), so nothing
    /// above the AC bandwidth up to the check; a gate given up the same up
    /// to its end. A whole gate's result holds from the gate's start only
    /// (it may have ended long before the check).
    #[inline(never)]
    fn freq_gate(&mut self, now: u64, src: u8) -> Option<(f32, f32, u32, Gate)> {
        if now < self.freq.next {
            return None;
        }
        let mut b = [0u8; 9];
        hy::read(0x15, &mut b);
        let (ctc, ctb, cta) = (u24(&b[0..3]), u24(&b[3..6]), u24(&b[6..9]));
        // CTA again: standing still = the counter stopped (a running CTA
        // moves every 0.2 us; this read takes tens of us).
        let mut a = [0u8; 3];
        hy::read(0x1B, &mut a);
        let el = (now as u32).wrapping_sub(self.freq.t0);
        let g = gate_check(ctc, ctb, cta, u24(&a), el, self.freq.f_khz);
        self.freq.hf_ok = true;
        if g == Gate::Wait {
            self.freq.hf_t = now as u32;
            self.freq.next = now + FREQ_RECHECK_MS;
            return None;
        }
        self.freq.hf_t = if g == Gate::Done { self.freq.t0 } else { now as u32 };
        self.diag_ctr = self.diag_ctr.saturating_add(1);
        ctr_restart(src);
        self.freq.t0 = now as u32;
        self.freq.next = now + FREQ_CHECK_MS;
        let (f, d) = counter_vals(ctc, ctb, cta);
        Some((f, d, cta.wrapping_add(0x40000) & 0xFF_FFFF, g))
    }

    /// The first counter access after a row write (`Freq::fresh`): the gate
    /// so far spans the switch and the settle, so it is only restarted
    /// (with `src`); the first result is the gate from now, 350 ms later.
    /// True = it did (no result this time).
    fn freq_restart_fresh(&mut self, now: u64, src: u8) -> bool {
        if !self.freq.fresh {
            return false;
        }
        self.freq.fresh = false;
        ctr_restart(src);
        self.freq.t0 = now as u32;
        self.freq.next = now + FREQ_CHECK_MS;
        true
    }

    fn freq_range(&mut self) {
        self.freq.range = freq_range_for(self.freq.f_khz, self.factor() * self.over(), self.freq.range);
    }

    /// AC V with the DC? alternation, after a harvest: a V-tap window just
    /// ended -> the DC? condition; then AD1 to the next window's input with
    /// the hold off, back on ~3 ms later (`pkh_due`: one 1.6 ms conversion
    /// of mixed inputs passes unheld).
    fn dc_hint_window(&mut self, now: u64) {
        if let Some((mn, mx)) = self.sent.pk_v.take() {
            // The V tap of AC V r0 / r1 is DC V r1's (K 100), r2 / r3 their
            // own; AD1<23:5> at IG 3.6 against 1.25 V.
            let r = self.range.clamp(0, 3) as usize;
            let k = K_ACV[r.max(1)];
            let vdc = (mn as f32 + mx as f32) * 0.5 * 32.0 * 1.25 / (3.6 * AD1_DIV) * k;
            let fs = (self.factor() * DEC4[r]).min(1000.0);
            // (not with a counter frequency above the AC bandwidth: the AC
            // reading then misses the AC part, sentinel V_HF_KHZ)
            let on = vdc.abs() > sentinel::DC_HINT_V.max(0.02 * fs)
                && self.f_ac < sentinel::DC_HINT_AC * vdc.abs()
                && !(self.freq.f_khz > sentinel::V_HF_KHZ);
            self.sent.dc_hint(now, on);
        }
        hy::write(0x29, &[self.r29 & !sentinel::R29_ENPKH]);
        hy::write(0x24, &[if self.sent.alt_v { sentinel::R24_SENSE_PB5 } else { sentinel::R24_PB1_PB3 }]);
        self.pkh_due = now + 3;
    }

    /// The first poll past the settle of a new row: the sentinel counts
    /// samples from sentinel::BLANK_SETTLED_MS on (Sentinel::rearm), and
    /// the peak hold (on since the row went out, so it holds the switch:
    /// relays, the converter's first results, the front end settling) is
    /// cleared. AC+DC: called at sentinel::BLANK_ROW_MS instead (`poll`),
    /// only the peak hold restarts; the comparator is sampled through the
    /// phase settles and the hold spans them, so A-JACK / V-JACK? do not
    /// lapse over a phase (their clear times are 1 s / 0.5 s).
    fn sent_rearm(&mut self, now: u64) {
        self.sent_arm = false;
        if !self.acdc {
            self.sent.rearm(now);
        }
        if matches!(self.sent.kind, Kind::PeakA | Kind::PeakV | Kind::Ad2A) && !self.pw.on {
            self.pkh_due = 0;
            self.pk_t = now;
            hy::write(0x29, &[self.r29 & !sentinel::R29_ENPKH]);
            hy::write(0x29, &[self.r29]);
        }
    }

    /// A-jack protection (doc 4.8, sentinel::aprot_trips), per poll in the
    /// DC V / mV / ohm / temperature functions: the probe goes in with the
    /// first comparator hit window (a row rewrite, AD1 last); every WIN_MS
    /// its AD2 peak gives the A-jack current. Over the limit -> PE0 = 1 (the
    /// 10 mohm shunt) at once and the latch (red A-PROT); latched, it ends
    /// when the user acted and the current stayed below APROT_GONE_A for
    /// APROT_GONE_MS. Without hits and without the latch the probe leaves.
    fn aprot_poll(&mut self, clk: &mut Clock, now: u64) {
        let run = self.sent.run_since();
        if !self.probe {
            if run.is_some() && self.sent.kind == Kind::CmpA && self.probe_ok() {
                self.probe = true;
                self.probe_rewrite(clk, 120);
            }
            return;
        }
        if self.aprot.is_none() && run.is_none() && !matches!(self.sent.alert, Alert::AJack) {
            self.probe = false;
            self.probe_rewrite(clk, 120);
            return;
        }
        // The row was just written: AD2 restarts (its first results, and a
        // hold the rewrite did not clear, are not the A-jack current: device
        // d8e249d latched A-PROT with the A jack open, 15 mA after it, on
        // the first window's peak). Clear the hold once it settled; the
        // first window counts from there.
        if self.probe_settle {
            if now.saturating_sub(self.pk_t) >= sentinel::PROBE_SETTLE_MS {
                self.probe_settle = false;
                self.pk_t = now;
                hy::write(0x29, &[self.r29 & !sentinel::R29_ENPKH]);
                hy::write(0x29, &[self.r29]);
            }
            return;
        }
        if now.saturating_sub(self.pk_t) < sentinel::WIN_MS {
            return;
        }
        self.pk_t = now;
        let mut b = [0u8; 6];
        hy::read(0x0E, &mut b);
        hy::write(0x29, &[self.r29 & !sentinel::R29_ENPKH]);
        hy::write(0x29, &[self.r29]);
        if self.jack_mode() {
            // the jack check's row (ohm / resistive temperature): no A-PROT;
            // the 6 / 60 Mohm rows' line (sentinel JACK_OWN_V)
            let line = if self.mode == 10 && self.range >= 5 { sentinel::JACK_OWN_V } else { f32::NEG_INFINITY };
            self.sent.jack_result(now, true, hy::s24(&b[0..3]), hy::s24(&b[3..6]), line);
            return;
        }
        let amps = sentinel::probe_amps(hy::s24(&b[0..3]), hy::s24(&b[3..6]));
        self.sent.probe_a = amps;
        match &mut self.aprot {
            Some(a) => {
                if amps >= sentinel::APROT_GONE_A {
                    a.low_since = None;
                } else if a.ack && now.saturating_sub(*a.low_since.get_or_insert(now)) >= sentinel::APROT_GONE_MS {
                    self.aprot = None;
                    self.sent.set_aprot(now, false);
                    self.probe = false;
                    self.probe_rewrite(clk, 250);
                }
            }
            // Only while the comparator still sees the current: a real one
            // through the 47.5 ohm chain is far above its window (5 mA =
            // 0.24 V at PB1), so a probe reading without hits is not one.
            // DC mV: not while its own V side says driven or has not said
            // free since the run began (V_GATE: a 66 kHz signal on V-COM put
            // the probe on PB2 over APROT_NOW_A with the A jack empty).
            None if run.is_some() => {
                let hot = run.map_or(0, |t| now.saturating_sub(t));
                if sentinel::aprot_trips(amps, hot) && self.sent.v_free() {
                    relay_pe(0);
                    self.aprot = Some(AProt { ack: false, low_since: None });
                    self.sent.aprot_ms = hot.min(u32::MAX as u64) as u32;
                    self.sent.set_aprot(now, true);
                }
            }
            None => {}
        }
    }

    /// The probe goes in / out (`aprot_poll`): the same function, range and
    /// comparator node (PB1), only OP2 / AD2 change, so the sentinel's
    /// switch blank (sentinel::BLANK_ROW_MS / BLANK_SETTLED_MS) does not
    /// apply: it would add ~0.2 s to A-JACK / A-PROT at every probe entry.
    fn probe_rewrite(&mut self, clk: &mut Clock, settle: u64) {
        self.write_config(clk);
        let now = clk.now_ms();
        self.deadline = now + settle;
        self.sent.unblank(now);
        self.sent_arm = false;
    }

    /// The function can carry the A-jack probe: DC V / mV, not AC+DC (its
    /// phases rewrite the row), not during the fuse check. Not ohm /
    /// temperature (device 22c85ba, A jack open): there the probe follows
    /// the V input, ~0.34 A per volt on V-COM (5 V DC read 1.7 A, 10 V 3.1 A,
    /// the 0..9.6 V square up to 37 A) and latched A-PROT; those rows keep
    /// the comparator's A-JACK only (doc 4.8).
    /// Also the ohm / resistive temperature rows, which carry the jack
    /// check's row in its place (`jack_mode`; never A-PROT there).
    fn probe_ok(&self) -> bool {
        (matches!(self.mode, 1 | 3) || self.jack_mode()) && !self.acdc && self.fuse_job.is_none()
    }

    /// V_GATE (AC mV), per result: `dc` = |LPF|, `ac` = sqrt(2) x RMS (mV).
    /// Driven (a source on V-COM, not the open V jack of an A-jack current):
    /// - the node's peak estimate dc + ac far beyond the range (MV_DRV_K);
    /// - a DC level on the node above MV_DC_DRV_MV (the row is DC-coupled;
    ///   the 0 / 9.5 V square at 66 kHz, clipped by the mV input: 0.51 V,
    ///   under 1.5 x 600 mV, with an RMS of ~30 mV; device 2026-10-26);
    /// - plain AC mV: the frequency counter above sentinel::V_HF_KHZ; a
    ///   "free" verdict is dated where the counter's evidence ends
    ///   (`Freq::hf_t`: a whole gate's start, or the last check that found
    ///   the gate open; none yet in this row: no verdict), so a signal that
    ///   starts within a run is judged by counts taken after it (emulator
    ///   acmv-hf60). Since the reciprocal gate (#43) an open input's gate
    ///   runs 2.2 s; its result alone put A-JACK at ~2.8 s after DC V ->
    ///   AC mV (switchtest real-ajack-pickup);
    /// - AC+DC mV: its DC phase's last result over the range (`mv_dc_drv`;
    ///   device 185c066: after a source went off the open input read OL and
    ///   the AC phase's results "free" while PB1 hit, a false A-JACK).
    ///
    /// A one-sided level on PB1 above sentinel::PK_DC_THR and above the
    /// node's |LPF| (the AD1 hold of this result) = the level sits on PB1,
    /// not on the V input: a current in A, "free" whatever the above say
    /// (device 2026-10-26, 20 mA in A, V open: the readout OL, ~50 kHz on
    /// the open input, no A-JACK; DC mV read +139 mV there). The V-COM
    /// cases measured put less there (66 kHz: ~28 mV against an LPF of
    /// 0.51 V; ~5-15 V DC: -89 mV, the node clamped). An A-JACK on this
    /// verdict is badge only (`soft`). `ac` NaN: a result over the
    /// converter range, which gives only that verdict.
    #[inline(never)] // .cold (link-native.x): per AC mV result
    fn mv_vside(&mut self, now: u64, dc: f32, ac: f32) {
        let pb1 = sentinel::pk_level(self.sent.pk.0, self.sent.pk.1);
        let ovr = pb1 > sentinel::PK_DC_THR && pb1 as f32 > dc * sentinel::PK_CODES_PER_MV;
        // (an A-JACK on this evidence: badge only, no danger alarm: `soft`)
        self.sent.soft = ovr;
        if ovr {
            self.sent.v_side(now, false);
            return;
        }
        if ac.is_nan() {
            return;
        }
        let lim = self.factor() * if self.range > 0 { 100.0 } else { 10.0 };
        let drv = dc + ac > sentinel::MV_DRV_K * lim
            || dc > sentinel::MV_DC_DRV_MV
            || (self.acdc && self.mv_dc_drv)
            || (!self.acdc && self.freq.f_khz > sentinel::V_HF_KHZ);
        if drv || self.acdc {
            self.sent.v_side(now, drv);
        } else if self.freq.hf_ok {
            // (dated where the counter's evidence ends: `Freq::hf_t`)
            let t = now.saturating_sub((now as u32).wrapping_sub(self.freq.hf_t) as u64);
            self.sent.v_side_at(now, t, false);
        }
    }

    /// The probe's row is the jack check's (sentinel `jack_row`): ohm and
    /// the resistive temperature sensors.
    fn jack_mode(&self) -> bool {
        self.mode == 10 || (self.mode == 15 && self.t_sensor != 0)
    }

    /// Continuity's jack check (sentinel JACK CHECK), per poll while the
    /// AD2 sentinel's window is due: AD2 is switched to OP1O - PB3 at IG 0.5
    /// (R25 / R27; AD2 stays on, AD1 untouched), the hold cleared
    /// PROBE_SETTLE_MS later, read one WIN_MS after that, then AD2 goes back
    /// to PB1 and its hold is cleared PROBE_SETTLE_MS later. True = the
    /// window belongs to the check (no PB1 sample).
    #[inline(never)] // .cold (link-native.x): only while PB1 hits (a fault)
    fn cont_jack(&mut self, now: u64) -> bool {
        if self.jwin == 0 {
            self.r25 = hy::read1(0x25);
            hy::write(0x25, &[self.r25 & sentinel::JACK_R25_MASK]);
            hy::write(0x27, &[sentinel::R27_OP1O_PB3]);
            self.jwin = 1;
            self.pk_t = now;
            return true;
        }
        let dt = now.saturating_sub(self.pk_t);
        match self.jwin {
            1 | 3 if dt >= sentinel::PROBE_SETTLE_MS => {
                hy::write(0x29, &[self.r29 & !sentinel::R29_ENPKH]);
                hy::write(0x29, &[self.r29]);
                self.pk_t = now;
                self.jwin = if self.jwin == 1 { 2 } else { 0 };
            }
            2 if dt >= sentinel::WIN_MS => {
                let mut b = [0u8; 6];
                hy::read(0x0E, &mut b);
                hy::write(0x25, &[self.r25]);
                hy::write(0x27, &[sentinel::R27_OP2O_PB3]);
                self.sent.jack_result(now, false, hy::s24(&b[0..3]), hy::s24(&b[3..6]), f32::NEG_INFINITY);
                self.pk_t = now;
                self.jwin = 3;
            }
            _ => {}
        }
        true
    }

    /// The peak-hold sentinel is harvested in `poll` (no AC result carries
    /// it): frequency (AD1 peak) and the AD2 kinds.
    fn peak_in_poll(&self) -> bool {
        self.sent.kind == Kind::Ad2A || (self.sent.kind == Kind::PeakA && self.mode == 14)
    }

    /// AC+DC: R20 for the frequency counter restart of a DC phase - the
    /// comparator stays on the sentinel's node (PB1 / SENSE; the counter
    /// then counts that node, AC+DC shows no frequency), else the stock's
    /// PB4 (0xF0).
    fn acdc_src(&self) -> u8 {
        match self.sent.kind {
            Kind::CmpA => sentinel::R20_PB1,
            Kind::CmpV => sentinel::R20_SENSE,
            _ => 0xF0,
        }
    }

    /// FUN_08013450 as the secondary readout in V/A modes, every 350 ms.
    /// Outside AC+DC it waits for the gate's end as the Hz function
    /// (`freq_gate`): low frequencies read too; a gate given up reads 0 Hz.
    /// AC+DC keeps the stock's blind 350 ms read and restart (its phases
    /// change the counter's source).
    fn freq_secondary(&mut self, now: u64, src: u8) {
        if self.freq_restart_fresh(now, src) || now < self.freq.next {
            return;
        }
        let (f, d) = if self.acdc {
            self.freq.next = now + FREQ_CHECK_MS;
            self.read_counters(src)
        } else {
            match self.freq_gate(now, src) {
                None => return,
                Some((f, d, _, Gate::Done)) => (f, d),
                Some(_) => (0.0, 0.0),
            }
        };
        self.freq.f_khz = f;
        self.freq.duty = d;
        self.freq_range();
    }

    /// Frequency / duty readouts. `aux`: the secondary format of V/A modes.
    fn freq_vals(&self, aux: bool) -> (Val, Val) {
        (freq_val(self.freq.f_khz, self.freq.range, aux), Val::new(self.freq.duty, 1, "%"))
    }

    fn poll_freq_mode(&mut self, now: u64) -> Option<Reading> {
        if self.freq_restart_fresh(now, 0x08) {
            // The first gate is source 1's (0x08): source 0 (0xf0) counted
            // nothing in its first gates after every Hz entry on the device
            // (d2aff22; 40728b8's clean first readings at 1.37 s were source
            // 1's too). The sources start over.
            self.freq.src_alt = true;
            self.freq.restart_sources();
            return None;
        }
        // The counter's gate ran with the source selected last time; at its
        // end (or timeout) restart it with the other one (0xf0 comparator /
        // 0x08 ENCNTI).
        let i = self.freq.src_alt as usize;
        let next_src = if self.freq.src_alt { 0xF0 } else { 0x08 };
        let (f, d, n, g) = self.freq_gate(now, next_src)?;
        self.freq.src_alt = !self.freq.src_alt;
        match self.freq.take(i, f, d, n, g) {
            // Nothing valid yet: no reading (the display keeps "--", READ?
            // and the statistics wait).
            FreqPick::None => return None,
            FreqPick::NoSignal => {
                self.freq.f_khz = 0.0;
                self.freq.duty = 0.0;
                self.freq.n = 0;
            }
            FreqPick::Src(best) => {
                self.freq.f_khz = self.freq.last[best];
                self.freq.duty = self.freq.lastd[best];
                self.freq.n = self.freq.lastn[best];
            }
        }
        if !self.manual {
            self.freq_range();
        }
        let r = self.freq.range as usize;
        let fs = FREQ_DEC[r] * self.factor();
        let ol = self.freq.f_khz / fs > self.over();
        let (fv, dv) = self.freq_vals(false);
        let fv = fv.with_ext(freq_ext(fv.v, fv.dec, self.freq.n));
        let mut rd = self.reading(if ol { Val::ol(fv.dec, fv.unit) } else { fv }, self.freq.f_khz * 1e3);
        rd.range = r as i8;
        rd.aux2 = Some(dv);
        rd.prov = self.freq.prov;
        Some(rd)
    }

    fn poll_temp(&mut self, now: u64) -> Option<Reading> {
        let ad1 = self.ad1()?;
        match self.t_phase {
            0 => {
                self.t_code_a = ad1;
                hy::write(0x33, &[0xAE]);
                self.t_phase = 1;
                self.deadline = now + 700;
                None
            }
            1 => {
                // User offset of the thermocouple sensor on the cold junction:
                // T = Tcj - 26 + T_tc(Vtc), so it shifts T by the same amount
                // and also corrects the CJ readout (community: the internal
                // sensor itself reads 3.6..6 C low on early units).
                self.t_cj = (self.t_code_a - ad1) as f32 / 640_000.0 * 314.15 - 289.15 + cal(0x194, 0.0) + self.t_offs(0);
                self.t_cj_at = now.max(1);
                hy::write(0x33, &[0xAC]);
                self.t_phase = 2;
                self.deadline = now + 800;
                None
            }
            _ => {
                let vtc = ad1 as f32 * cal(0x24, 1.0) * 1.25 / 0.9 * 100.0 / AD1_DIV - cal(0x2C, 0.0);
                let mut i = 1;
                while i < 14 && TC_K_MV[i + 1] < vtc {
                    i += 1;
                }
                let t_tc = (i as f32 - 2.0 + (vtc - TC_K_MV[i]) / (TC_K_MV[i + 1] - TC_K_MV[i])) * 100.0;
                let t = self.t_cj - 26.0 + t_tc;
                self.sent.v_side(now, vtc.abs() > sentinel::T_VDRV_MV);
                hy::write(0x33, &[0xAF]);
                self.t_phase = 0;
                self.deadline = now + 700;
                let ol = !(-40.0..=1000.0).contains(&t);
                // One unit everywhere (owner: no second deg F readout).
                let (td, u) = self.t_disp(t);
                let mut rd = self.reading(if ol { Val::ol(1, u) } else { Val::new(td, 1, u) }, td);
                rd.aux1 = Some(Val::new(self.t_disp(self.t_cj).0, 1, u));
                Some(rd)
            }
        }
    }

    /// Resistive temperature sensor on the ohm path (see the T_NAMES notes).
    fn poll_rtd(&mut self, clk: &mut Clock, now: u64) -> Option<Reading> {
        let ad1 = self.ad1()?;
        let r = self.range.clamp(1, 6) as usize;
        let ohm = ((ad1.unsigned_abs() as f32) * cal(0x124 + 4 * r as u32, 1.0) * OHM_RREF[r] / 0.9 / AD1_DIV
            - cal(0x144, 0.0))
            * 1e3;
        self.t_res = ohm;
        // V_GATE (sentinel.rs): the ohm path's LIVE? sign says V-COM is driven.
        self.sent.v_side(now, ad1 < -LIVE_CODE);
        let lim = self.factor() * OHM_DEC[r] * 1e3;
        let (r0, beta) = self.t_params();
        if self.t_sensor >= 3 {
            // NTC range policy: up at 95 % of full scale (before OL at 103 %),
            // down below 8.5 % (= 85 % of the lower range), two readings in a
            // row; r2..r6. Every candidate range keeps the sensor power far
            // below 0.1 mW, so the choice is about resolution (t_excitation).
            let to = if ohm > 0.95 * lim && r < 6 {
                r + 1
            } else if ohm < 0.085 * lim && r > 2 {
                r - 1
            } else {
                r
            };
            if to != r {
                self.t_votes += 1;
                if self.t_votes >= 2 {
                    self.t_votes = 0;
                    self.range = to as i8;
                    self.write_config(clk);
                    self.deadline = now + 300;
                    return None;
                }
            } else {
                self.t_votes = 0;
            }
        }
        let t = if ohm > self.over() * lim {
            None
        } else if self.t_sensor <= 2 {
            pt_temp(ohm / r0)
        } else {
            ntc_temp(ohm, r0, beta)
        };
        let t = t.map(|t| t + self.t_offs(self.t_sensor));
        if let Some(t) = t {
            if self.t_phase == 0 {
                self.t_phase = 1;
                self.t_ema = t;
            } else {
                ema(0.5, 0.3, t, &mut self.t_ema);
            }
        } else {
            self.t_phase = 0;
        }
        let (tv, c) = self.t_disp(self.t_ema);
        let mut rd = self.reading(if t.is_some() { Val::new(tv, 2, c) } else { Val::ol(2, c) }, tv);
        rd.aux1 = Some(if ohm > self.over() * lim {
            Val::ol(2, "\u{3a9}")
        } else if ohm < 1e3 {
            Val::new(ohm, 2, "\u{3a9}")
        } else if ohm < 1e6 {
            Val::new(ohm * 1e-3, 3, "k\u{3a9}")
        } else {
            Val::new(ohm * 1e-6, 3, "M\u{3a9}")
        });
        Some(rd)
    }

    fn cap_val(&self, c_uf: f32, ol: bool) -> Reading {
        let r = self.range as usize;
        let (v, dec, unit) = match r {
            0..=2 => (c_uf * 1000.0, 3 - r as u8, "nF"),
            3..=5 => (c_uf, 6 - r as u8, "\u{b5}F"),
            _ => (c_uf * 1e-3, 9 - r as u8, "mF"),
        };
        let dec = dec.min(3);
        self.reading(if ol { Val::ol(dec, unit) } else { Val::new(v, dec, unit) }, c_uf * 1e-6)
    }

    fn cap_set_range(&mut self, clk: &mut Clock, r: i8) {
        let now = clk.now_ms();
        self.range = r;
        if r >= 5 {
            // Discharge first, then start the ramp when |V| < 30 mV.
            hy::quiesce();
            hy::configure(&CFG_F13_DISCHARGE);
            self.ramp.discharging = true;
            self.ramp.n = 0;
            self.ramp.med_i = 0;
            self.ramp.class = 0;
            self.ramp.cmp = None;
            self.ramp.first = true;
            self.deadline = now + 255;
        } else {
            self.write_config(clk);
            self.deadline = now + if r < 4 { 120 } else { 320 };
        }
        self.cap_cnt_up = 0;
        self.cap_cnt_dn = 0;
        self.cap_cnt_small = 0;
    }

    fn poll_cap(&mut self, clk: &mut Clock, now: u64) -> Option<Reading> {
        let r = self.range.max(0) as usize;
        let (f_dec, ov) = (self.factor(), self.over());
        let lim = f_dec * CAP_DEC[r];
        if r < 5 {
            let (f, _) = self.read_counters(0x50);
            self.deadline = now + 420;
            ema(CAP_WIN[r], 0.05, f, &mut self.f_cap);
            let fk = self.f_cap.max(1e-9);
            // Negative results are clamped to 0 as in the stock (m+0x18 == 0).
            let c = (cal(0x170 + 4 * r as u32, 1.0) * 0.72135 / (CAP_K[r] * fk) - cal(0x190, 0.0)).clamp(0.0, 1.06 * lim);
            let ol = c / lim > ov;
            if !self.manual {
                if r >= 1 && self.f_cap > 2.0 {
                    self.cap_cnt_small += 1;
                    if r < 4 || self.cap_cnt_small >= 4 {
                        self.cap_set_range(clk, 0);
                        return None;
                    }
                } else {
                    self.cap_cnt_small = 0;
                }
                if c > ov * lim {
                    self.cap_set_range(clk, r as i8 + 1);
                    return Some(self.cap_val(0.0, true));
                }
            }
            if !ol {
                self.cap_c = c;
            }
            return Some(self.cap_val(self.cap_c, ol));
        }

        // r5..r7: constant-current ramp on AD1.
        let ad1 = self.ad1()?;
        let v_mv = ad1 as f32 * 1000.0 * 1.25 / 0.9 / AD1_DIV;
        if self.ramp.discharging {
            if v_mv.abs() < 30.0 {
                self.ramp.discharging = false;
                self.write_config(clk);
                self.ramp.prev_t = now;
                self.ramp.prev_v = v_mv;
            }
            self.deadline = now + 255;
            return None;
        }
        let _ = self.read_counters(0x50);
        let dv = (self.ramp.prev_v - v_mv).abs();
        let dt = now.saturating_sub(self.ramp.prev_t) as f32;
        self.ramp.prev_v = v_mv;
        self.ramp.prev_t = now;
        self.deadline = now + 255;
        if dv == 0.0 {
            return None;
        }
        if self.ramp.med_i == 0 {
            self.ramp.med = [dv; 5];
        } else {
            self.ramp.med[self.ramp.med_i % 5] = dv;
        }
        self.ramp.med_i += 1;
        let mut s = self.ramp.med;
        sort_f32(&mut s);
        let dvm = s[2];
        let class = if dvm < 2.5 {
            1
        } else if dvm < 9.0 {
            2
        } else {
            3
        };
        if class != self.ramp.class {
            self.ramp.class = class;
            self.ramp.med_i = 0;
            // Comparator threshold by the REAL model byte (dmm_v2_6.h:164,326):
            // an HY3131 analog setting, not a counts limit, so not COUNTS_MODEL.
            let v = CAP_R21[class as usize - 1] - if self.model == 2 { 0 } else { 1 };
            hy::write(0x21, &[v]);
            self.deadline = now + 255;
            return Some(self.cap_val(0.0, true));
        }
        let c = (cal(0x170 + 4 * r as u32, 1.0) * CAP_K[r] * dt / dvm).clamp(0.0, 1.06 * lim);
        if self.ramp.n < 35 {
            self.ramp.buf[self.ramp.n] = c;
            self.ramp.n += 1;
        }
        let cmp = hy::read1(0x14) & 0x40 != 0;
        let flip = self.ramp.cmp.map(|p| p != cmp).unwrap_or(false);
        self.ramp.cmp = Some(cmp);
        if flip || self.ramp.n == 35 {
            let n = self.ramp.n;
            let mean = if n == 35 {
                self.ramp.buf[33]
            } else if n > 4 {
                let mut b = self.ramp.buf;
                sort_f32(&mut b[..n]);
                b[2..n - 2].iter().sum::<f32>() / (n - 4) as f32
            } else {
                c
            };
            if self.ramp.first {
                self.cap_c = c;
                self.ramp.first = false;
            } else {
                ema(CAP_WIN[r], 0.25, mean, &mut self.cap_c);
            }
            self.ramp.n = 0;
        }
        let cc = self.cap_c;
        let ol = cc / lim > ov;
        if !self.manual {
            if cc > ov * lim && r < 7 {
                self.cap_cnt_up += 1;
                if self.cap_cnt_up >= 5 {
                    self.cap_set_range(clk, r as i8 + 1);
                }
            } else if cc < 0.1 * lim / ov && r >= 6 {
                self.cap_cnt_dn += 1;
                if self.cap_cnt_dn >= 3 {
                    self.cap_set_range(clk, r as i8 - 1);
                }
            } else {
                self.cap_cnt_up = 0;
                self.cap_cnt_dn = 0;
            }
        }
        Some(self.cap_val(cc, ol))
    }

    /// Time (ms) before which `poll` gives no reading: the settle after a
    /// function / range change (SCPI starts its reply timeout from here).
    pub fn deadline(&self) -> u64 {
        // Power meter: the first full P / V / I set takes one phase cycle
        // after the settle.
        // 600 Mohm: its first reading takes two phases (`OHM600_FIRST_MS`).
        if self.pw.on {
            self.deadline + self.pw_cycle().2 as u64
        } else if is_ohm600(self.mode, self.range) {
            self.deadline + OHM600_FIRST_MS
        } else {
            self.deadline
        }
    }
}

/// Format `v` with `dec` decimals into `buf`; returns the text. A value too
/// long for the 16 bytes (sign, 14 digits and the point; e.g. a limit of
/// 1E20 set over SCPI, or infinity) shows as all nines instead of running
/// past the buffer (a panic). A value that rounds to zero has no sign (no
/// "-0.0000", #113).
pub fn format(buf: &mut [u8; 16], v: f32, dec: u8) -> &str {
    let mut scale = 1.0f32;
    for _ in 0..dec {
        scale *= 10.0;
    }
    let max = 10u64.pow(15 - (dec > 0) as u32) - 1;
    let mut n = ((v.abs() * scale + 0.5) as u64).min(max);
    let neg = v < 0.0 && n != 0;
    let mut tmp = [0u8; 16];
    let mut i = 0;
    loop {
        if i == dec as usize && dec > 0 {
            tmp[i] = b'.';
            i += 1;
        }
        tmp[i] = b'0' + (n % 10) as u8;
        i += 1;
        n /= 10;
        if n == 0 && i > dec as usize + (dec > 0) as usize {
            break;
        }
    }
    let mut o = 0;
    if neg {
        buf[o] = b'-';
        o += 1;
    }
    while i > 0 {
        i -= 1;
        buf[o] = tmp[i];
        o += 1;
    }
    core::str::from_utf8(&buf[..o]).unwrap_or("")
}

/// Scale of one raw AD2 code in a stock AC row, for the DMM wave view
/// (scope page, source DMM; docs/dmm-input-scope-feasibility.md 3.3): the
/// DMM's own LPF formula of `poll_ac` (the DC mean of the AD2 stream) with
/// its calibration gain / offset, so the trace and the DMM agree.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Ad2Scale {
    /// SI (V or A) of raw code c: c * per_code - offset.
    pub per_code: f32,
    pub offset: f32,
    /// 0 V, 1 A.
    pub amps: bool,
    /// AC-coupled (AC V: PB4 through the coupling capacitor); else DC.
    pub ac: bool,
    /// Full scale of the DMM range (DM40C labels), SI: 6 V, 600 mV, 10 A, ...
    pub full: f32,
    /// `Dmm::set_range` item of this range (`ui::range_items`: 60 mV is 5).
    pub item: u8,
}

/// DC V volts of a raw AD1 code read at DC V tap `k` (0..3 = /10 ..
/// /10000) with the DC V calibration of range k: `poll_dcv`'s formula
/// without the averaging and the small negative-side / r0 corrections
/// (device test 6.5, the DC-coupled V row's self-cal).
pub fn dcv_volts(code: f32, k: usize) -> f32 {
    let k = k.min(3);
    code * K_ACV[k] * (1.25 / 0.9) / AD1_DIV * cal(0x04 + 4 * k as u32, 1.0) - cal(0x14 + 4 * k as u32, 0.0)
}

/// AD2 gain of the DC-coupled V row per DC V range (ad2exp.rs `dcv_row`,
/// feasibility E7) relative to the nominal K * 1.25 / 2^18: device test 6.5
/// (f4c792e, the fit of the READ? reference against the AD2 mean, AGND,
/// OP1's chopper held): r0 +0.84 % (the /100 tap with OP1 x10), r1 +1.79 %,
/// r2 +0.53 %, r3 +0.27 % (r2 / r3 up to 24 V only: +-0.2 / 0.5 %). The
/// fallback of the self-cal's gain (an input below ~5 % of AD2's span).
pub const DCV_AD2_GAIN: [f32; 4] = [1.0084, 1.0179, 1.0053, 1.0027];

/// The DC-coupled V row's one-shot self-cal (ad2exp.rs `dc_enter`):
/// range | 0x100 (valid), per_code and offset (f32 bits) of `Ad2Scale`.
static DC_CAL: [core::sync::atomic::AtomicU32; 3] = [const { core::sync::atomic::AtomicU32::new(0) }; 3];

/// Store (`Some((per_code, offset))`) / clear the self-cal of DC V range `r`.
pub fn set_dc_cal(r: u8, cal: Option<(f32, f32)>) {
    use core::sync::atomic::Ordering::Relaxed;
    let (p, o) = cal.unwrap_or((0.0, 0.0));
    DC_CAL[1].store(p.to_bits(), Relaxed);
    DC_CAL[2].store(o.to_bits(), Relaxed);
    DC_CAL[0].store(if cal.is_some() { 0x100 | r as u32 } else { 0 }, Relaxed);
}

/// The self-cal (per_code, offset) of DC V range `r`, if there is one.
pub fn dc_cal(r: i8) -> Option<(f32, f32)> {
    use core::sync::atomic::Ordering::Relaxed;
    if DC_CAL[0].load(Relaxed) != 0x100 | r.clamp(0, 3) as u32 {
        return None;
    }
    Some((f32::from_bits(DC_CAL[1].load(Relaxed)), f32::from_bits(DC_CAL[2].load(Relaxed))))
}

/// `Ad2Scale` of DMM mode `m` at range `r`: the stock AC rows (0 AC V, 2 AC
/// mV, 4..6 AC A / mA / uA), DC V (1: the DC-coupled V row, nominal gain x
/// DCV_AD2_GAIN, no offset: `Dmm::ad2_scale` takes the self-cal's) and DC mV
/// (3: streamed in the AC mV row, DC-coupled, its LPF calibration); None
/// for every other mode.
pub fn ad2_scale_of(m: u8, r: i8) -> Option<Ad2Scale> {
    let ri = r.max(0) as usize;
    let (s, ci, g, o, si, amps, full, item) = match m {
        1 => {
            let r = ri.min(3);
            return Some(Ad2Scale {
                per_code: K_ACV[r] * 1.25 / 262_144.0 * DCV_AD2_GAIN[r],
                offset: 0.0,
                amps: false,
                ac: false,
                full: [6.0, 60.0, 600.0, 1000.0][r],
                item: r as u8 + 1,
            });
        }
        3 => {
            let r = ri.min(1);
            (1.25 / MV_DIV[r] / 262.144, r, 0x84, 0x8C, 1e-3, false, [0.06, 0.6][r], [5, 0][r])
        }
        0 => {
            let r = ri.min(3);
            (K_ACV[r] * 1.25 / 262_144.0, r, 0x64, 0x74, 1.0, false, [6.0, 60.0, 600.0, 1000.0][r], r as u8 + 1)
        }
        2 => {
            let r = ri.min(1);
            (1.25 / MV_DIV[r] / 262.144, r, 0x84, 0x8C, 1e-3, false, [0.06, 0.6][r], [5, 0][r])
        }
        4..=6 => {
            let u = (m - 4) as usize;
            let r = ri.min(1);
            let full = [[6.0, 10.0], [0.06, 0.6], [600e-6, 6000e-6]][u][r];
            (1.25 / (SHUNT[u] * MV_DIV[r]) / 262_144.0, u * 2 + r, 0xF4, 0x10C, [1.0, 1e-3, 1e-6][u], true, full, ((2 - u) * 2 + r) as u8)
        }
        _ => return None,
    };
    let ci = ci as u32;
    Some(Ad2Scale {
        per_code: s * cal(g + 4 * ci, 1.0) * si,
        offset: cal(o + 4 * ci, 0.0) * si,
        amps,
        ac: m == 0,
        full,
        item,
    })
}

impl Dmm {
    /// `ad2_scale_of` the configured mode / range (None: not a stock AC
    /// row, or AC+DC / the power meter, whose rows alternate).
    pub fn ad2_scale(&self) -> Option<Ad2Scale> {
        if self.acdc || self.pw.on {
            return None;
        }
        let mut a = ad2_scale_of(self.mode, self.range)?;
        if self.mode == 1 {
            if let Some((p, o)) = dc_cal(self.range) {
                (a.per_code, a.offset) = (p, o);
            }
        }
        Some(a)
    }
}

/// Ascending sort of a few (5 / <= 35) readings: insertion sort, the same
/// order as `sort_unstable_by(partial_cmp)` for numbers (NaN compares as
/// equal, so its place is not defined, as there). ~0.1 KB instead of the
/// ~3.3 KB core quicksort instantiation (region B).
fn sort_f32(v: &mut [f32]) {
    for i in 1..v.len() {
        let x = v[i];
        let mut j = i;
        while j > 0 && x < v[j - 1] {
            v[j] = v[j - 1];
            j -= 1;
        }
        v[j] = x;
    }
}

// Host tests: tools/meter-test/run.sh (pure logic only; no register access).
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_never_overruns() {
        let mut b = [0u8; 16];
        assert_eq!(format(&mut b, 1.5, 3), "1.500");
        assert_eq!(format(&mut b, -0.25, 2), "-0.25");
        assert_eq!(format(&mut b, 12345.0, 0), "12345");
        // Too long for 16 bytes (a limit of 1E20 set over SCPI): all nines.
        assert_eq!(format(&mut b, 1e20, 3), "99999999999.999");
        assert_eq!(format(&mut b, -1e20, 3), "-99999999999.999");
        assert_eq!(format(&mut b, 9.9e37, 0), "999999999999999");
        assert_eq!(format(&mut b, f32::INFINITY, 1), "9999999999999.9");
        assert_eq!(format(&mut b, f32::NEG_INFINITY, 0), "-999999999999999");
        assert_eq!(format(&mut b, f32::NAN, 2), "0.00");
    }

    #[test]
    fn format_no_negative_zero() {
        let mut b = [0u8; 16];
        // #113: open DC V leads, a few uV below 0 at 4 decimals.
        assert_eq!(format(&mut b, -0.000_04, 4), "0.0000");
        assert_eq!(format(&mut b, -0.000_06, 4), "-0.0001");
        // The extra digit (dec + 1): -10 uV keeps its sign.
        assert_eq!(format(&mut b, -0.000_01, 5), "-0.00001");
        assert_eq!(format(&mut b, -0.4, 0), "0");
        assert_eq!(format(&mut b, -1.25, 1), "-1.3");
        assert_eq!(format(&mut b, 0.0, 3), "0.000");
    }

    #[test]
    fn ohm_r0_gain_from_r1() {
        // The dumped unit's record: r0 slot 1.005 (never calibrated), r1
        // 1.002732, the DC mV x10 / x1 gain ratio 0.9895.
        let (g0, g1, m10, m1) = (1.005f32, 1.002_732f32, 0.9895f32, 1.0f32);
        assert_eq!(ohm_row_gain(1, g1, g1, m10, m1), g1);
        assert_eq!(ohm_row_gain(3, 0.993751, g1, m10, m1), 0.993751);
        assert_eq!(ohm_row_gain(0, g0, g1, m10, m1), ohm_row_gain(0, 1.0, g1, m10, m1));
        // Device 2026-10-26, PA2 open: (r1, r0 as read with r1's gain x
        // 1.0291) at 10 / 20 / 50 ohm. With the record's ratio r0 is within
        // 0.2 % of r1.
        for (r600, r60) in [(9.1141f32, 9.4608f32), (19.0848, 19.8285), (49.4137, 51.3537)] {
            let fixed = r60 / (g1 * 1.0291) * ohm_row_gain(0, g0, g1, m10, m1);
            assert!((fixed / r600 - 1.0).abs() < 0.002, "{fixed} vs {r600}");
        }
        // No record: the plain stock scale.
        assert_eq!(ohm_row_gain(0, 1.0, 1.0, 1.0, 1.0), 1.0);
        // The r0 row: the r1 row with OPS1 (R25 bit 0).
        let mut r1 = CFG_F10_P8CLR[1];
        r1[5] |= 1;
        assert_eq!(OHM_R0_ROW, r1);
    }

    #[test]
    fn ohm600_row_and_cycle() {
        // The r5 row with R22 200 ms, R28 PB6 - AGND and R31 AGNDP9; phase
        // A's R32 (OP1 on PB0) is the row's own.
        let r5 = CFG_F10_P8CLR[5];
        let d: Vec<usize> = (0..hy::CFG_LEN).filter(|&i| OHM600_ROW[i] != r5[i]).collect();
        assert_eq!(d, vec![0x02, 0x08, 0x11]);
        assert_eq!(OHM600_ROW[0x11] & 0x0F, 3);
        assert_eq!(OHM600_ROW[0x12], OHM600_R32[0]);
        let code = |v: f32| (v / OHM600_V) as i32;
        // Device 2026-10-28, 5.5693 Mohm on another meter: Vx 0.374876 V,
        // Vfb 1.047889 V. Per phase 2 results dropped (a switch transient
        // first), the median of 3 (one spike in it).
        let mut o = Ohm600::NEW;
        let mut done = None;
        let mut sw = vec![];
        for v in [0.30, 0.3749, 0.374876, 0.3790, 0.374870] {
            sw.extend(o.step(code(v), &mut done));
        }
        assert_eq!(sw, vec![0x39]);
        assert!(done.is_none());
        for v in [1.04, 1.0478, 1.047889, 1.047880, 1.047895] {
            sw.extend(o.step(code(v), &mut done));
        }
        assert_eq!(sw, vec![0x39, 0x3C]);
        let (a, b) = done.unwrap();
        let k = o.kohm(a, b, 1.03).unwrap();
        assert!((k / 5570.12 - 1.0).abs() < 2e-4, "{k}");
        // 600 Mohm: Vx = Vfb * 600 / 610; above 1.03 x 600 Mohm, no drop
        // and a jack driven above FB are OL.
        let vf = 1.0475f32;
        let k = o.kohm(code(vf * 600.0 / 610.0), code(vf), 1.03).unwrap();
        assert!((k / 600_000.0 - 1.0).abs() < 1e-3, "{k}");
        assert!(o.kohm(code(vf * 700.0 / 710.0), code(vf), 1.03).is_none());
        assert!(o.kohm(code(vf), code(vf), 1.03).is_none());
        assert!(o.kohm(code(1.06), code(vf), 1.03).is_none());
        // The offset is taken off the drop: +17 uV at 600 Mohm is +0.1 %.
        o.off = 17e-6;
        let k2 = o.kohm(code(vf * 600.0 / 610.0), code(vf), 1.03).unwrap();
        assert!((k2 / k - 1.001).abs() < 2e-4, "{k2} {k}");
        // keep 1, skip 0: every result ends a phase.
        let mut o = Ohm600 { skip: 0, keep: 1, ..Ohm600::NEW };
        assert_eq!(o.step(1, &mut done), Some(0x39));
        assert_eq!(o.step(2, &mut done), Some(0x3C));
        assert_eq!(done, Some((1, 2)));
    }

    #[test]
    fn ohm600_average() {
        let mut o = Ohm600::NEW;
        assert_eq!(o.average(100_000.0), 100_000.0);
        assert_eq!(o.average(101_000.0), 100_500.0);
        // A step over 2 %: the mean starts over.
        assert_eq!(o.average(110_000.0), 110_000.0);
        assert_eq!(o.hn, 1);
        for k in [110_100.0, 110_200.0, 110_300.0, 110_400.0] {
            o.average(k);
        }
        // The last OHM600_AVG (4) only.
        assert_eq!(o.hn as usize, OHM600_AVG);
        assert!((o.average(110_500.0) - 110_350.0).abs() < 1.0);
    }

    #[test]
    fn ohm600_digits() {
        // The decade's 6000-count format; 3 % hysteresis at 6 / 60 Mohm once
        // two readings are in.
        let mut o = Ohm600 { hn: 2, ..Ohm600::NEW };
        assert_eq!(o.decimals(5.57), 3);
        assert_eq!(o.decimals(6.1), 3);
        assert_eq!(o.decimals(6.2), 2);
        assert_eq!(o.decimals(5.9), 2);
        assert_eq!(o.decimals(5.8), 3);
        assert_eq!(o.decimals(412.3), 1);
        assert_eq!(o.decimals(59.0), 1);
        assert_eq!(o.decimals(58.0), 2);
        // The first reading of a window takes its decade at once.
        let mut o = Ohm600 { dec: 1, hn: 1, ..Ohm600::NEW };
        assert_eq!(o.decimals(5.9), 3);
        // The extra digit at 3 decimals is 0.0001 Mohm = 0.1 kohm. Device
        // 2026-10-28: 5.5705 Mohm with a reading sd of 0.0002 Mohm, so a
        // mean of 4 at 0.1 kohm: shown.
        let mut o = Ohm600::NEW;
        for k in [5570.5, 5570.7, 5570.4, 5570.6] {
            o.average(k);
        }
        assert!(o.ext(3));
        // Twice that spread: not at 3 decimals, at 2 (1 kohm).
        let mut o = Ohm600::NEW;
        for k in [5570.0, 5570.6, 5569.8, 5570.9] {
            o.average(k);
        }
        assert!(!o.ext(3));
        assert!(o.ext(2));
        // Not before the window is full.
        let mut o = Ohm600::NEW;
        o.average(5570.0);
        assert!(!o.ext(1));
        // Hysteresis: a mean sd of 1.0 digit keeps it on, does not turn it on.
        let mut o = Ohm600::NEW;
        for k in [5570.0, 5570.2, 5569.8, 5570.0] {
            o.average(k);
        }
        // sd 0.163 kohm, of the mean 0.082: 0.82 digit at 3 decimals.
        assert!(!o.ext(3));
        o.ext_on = true;
        assert!(o.ext(3));
    }

    #[test]
    fn ohm600_learn_guards() {
        let code = |v: f32| (v / OHM600_V) as i32;
        let vf = 1.0479f32;
        // The jack above FB (a charged capacitor, a source): never learned.
        let mut o = Ohm600::NEW;
        for _ in 0..4 {
            o.learn(code(1.30), code(vf));
        }
        assert_eq!(o.leak, 0.0);
        // A row write restarts the open run: the first open pair after it
        // is skipped again.
        let (a, b) = (code(vf - 485e-6), code(vf));
        o.learn(a, b);
        o.restart();
        o.learn(a, b);
        assert_eq!(o.leak, 0.0);
        o.learn(a, b);
        assert!(o.leak > 0.0);
        // Frozen (a DBG:OHM6 leak value): open readings leave it.
        o.leak = 1e-11;
        o.learn_on = false;
        o.learn(a, b);
        o.learn(a, b);
        assert_eq!(o.leak, 1e-11);
    }

    #[test]
    fn ohm600_leak() {
        let code = |v: f32| (v / OHM600_V) as i32;
        let vf = 1.0479f32;
        // Device 2026-10-28, open leads: Vfb - Vx = 485 uV (48.5 pA at
        // 1.0474 V). The first open pair is skipped, then it is learned.
        let mut o = Ohm600::NEW;
        let (a, b) = (code(vf - 485e-6), code(vf));
        o.learn(a, b);
        assert_eq!(o.leak, 0.0);
        o.learn(a, b);
        let r_leak = 1.0 / o.leak;
        assert!((r_leak / 21.6e9 - 1.0).abs() < 0.01, "{r_leak}");
        // 600 Mohm in parallel with that leak: the drop carries both
        // currents; the reading is the 600 Mohm.
        let rx = 600e6f32;
        let rp = rx * r_leak / (rx + r_leak);
        let a = code(vf * rp / (rp + 1e7));
        let k = o.kohm(a, code(vf), 1.03).unwrap();
        assert!((k / 600_000.0 - 1.0).abs() < 1e-3, "{k}");
        // Without the leak it reads ~2.7 % low.
        let k0 = Ohm600::NEW.kohm(a, code(vf), 1.03).unwrap();
        assert!((k0 / 600_000.0 - 0.973).abs() < 0.003, "{k0}");
        // A resistor on the jacks (a drop far above the threshold) leaves it
        // and restarts the open run.
        let l = o.leak;
        o.learn(code(0.3749), code(vf));
        assert_eq!(o.leak, l);
        o.learn(a, b);
        assert_eq!(o.leak, l);
    }

    #[test]
    fn ohm600_test_current() {
        assert_eq!(ohm_test_current(OHM600, 0), Some(OHM600_ISC));
        assert_eq!(ohm_test_current(OHM600, 1), Some(OHM600_ISC));
        assert_eq!(ohm_test_current(6, 0), None);
    }

    #[test]
    fn freq_glitch_source_loses() {
        // Clean 40 Hz on one source, 13.334 / 26.668 Hz at 100 % duty on the
        // other (device, AC+DC -> Hz): the clean source wins every time.
        let f = 0.040002;
        assert!(!freq_glitch(f, 50.0));
        assert!(freq_glitch(0.013334, 100.0) && freq_glitch(0.026668, 99.96) && !freq_glitch(0.04, 0.2) && !freq_glitch(0.04, 99.9));
        assert!(!freq_glitch(0.0, 100.0)); // no signal: 0 Hz, not a glitch
        let clean = freq_change(f, 50.0, f);
        for (g, d) in [(0.013334f32, 100.0f32), (0.026668, 100.0)] {
            assert!(freq_change(g, d, g) > clean);
            assert!(freq_change(g, d, 0.0) > clean);
        }
        // No history after a function change: a clean first result (change
        // from 0) still beats a glitch.
        assert!(freq_change(f, 50.0, 0.0) < freq_change(0.013334, 100.0, 0.013334));
    }

    /// `Freq::take`'s gate kinds besides Gate::Done: quiet (no edge for
    /// FREQ_NOEDGE_MS), empty (no edge within the limit), late (counts, no end).
    const Q: Gate = Gate::Quiet;
    const E: Gate = Gate::Empty;
    const L: Gate = Gate::Late;

    fn freq0() -> Freq {
        Freq { next: 0, src_alt: false, fresh: false, seen: [false; 2], zc: [0; 2], shown: false, last: [0.0; 2], lastd: [0.0; 2], change: [1e6; 2], f_khz: f32::NAN, duty: f32::NAN, range: 0, t0: 0, lastn: [0; 2], n: 0, prev: f32::NAN, conf: [false; 2], prov: false, hf_ok: false, hf_t: 0 }
    }

    #[test]
    fn freq_take_zero_is_no_result() {
        let f = 0.0400017f32;
        // Device d2aff22: source 0 reads 0 Hz / 0 % in its first gates,
        // source 1 the signal. Entry order as poll_freq_mode: 1, 0, 1, 0.
        let mut q = freq0();
        assert_eq!(q.take(1, f, 50.0, 1, Gate::Done), FreqPick::Src(1));
        assert_eq!(q.take(0, 0.0, 0.0, 1, E), FreqPick::Src(1));
        assert_eq!(q.take(1, f, 50.0, 1, Gate::Done), FreqPick::Src(1));
        assert_eq!(q.take(0, 0.0, 0.0, 1, Gate::Done), FreqPick::Src(1)); // f 0: no result even when "whole"
        assert_eq!(q.lastd[1], 50.0);
        // Source 0 first with 0, then source 1 with 0 too: nothing (not 0.000 Hz).
        let mut q = freq0();
        assert_eq!(q.take(0, 0.0, 0.0, 1, E), FreqPick::None);
        assert_eq!(q.take(1, 0.0, 0.0, 1, E), FreqPick::None);
        assert_eq!(q.take(0, 0.0, 0.0, 1, E), FreqPick::None);
        // ... no signal: both counted nothing twice -> 0.000 Hz.
        assert_eq!(q.take(1, 0.0, 0.0, 1, E), FreqPick::NoSignal);
        // A signal comes: shown at once.
        assert_eq!(q.take(0, f, 50.0, 1, Gate::Done), FreqPick::Src(0));
        // It goes away: one empty gate each is enough now.
        assert_eq!(q.take(1, 0.0, 0.0, 1, E), FreqPick::Src(0));
        assert_eq!(q.take(0, 0.0, 0.0, 1, E), FreqPick::NoSignal);
        // An incomplete gate (`ok` false) with a value is no result either.
        let mut q = freq0();
        assert_eq!(q.take(1, 0.013, 30.0, 1, E), FreqPick::None);
        // A glitch is no result, and does not count as "no signal".
        assert_eq!(q.take(0, 0.013334, 100.0, 1, Gate::Done), FreqPick::None);
        assert_eq!(q.take(1, 0.0, 0.0, 1, E), FreqPick::None);
        assert_eq!(q.take(0, 0.013334, 100.0, 1, Gate::Done), FreqPick::None);
        assert_eq!(q.take(1, 0.0, 0.0, 1, E), FreqPick::None);
        // Both clean and equal: the tie stays with one of them (source 0).
        let mut q = freq0();
        q.take(1, f, 50.0, 1, Gate::Done);
        assert_eq!(q.take(0, f, 50.0, 1, Gate::Done), FreqPick::Src(0));
    }

    /// A simulated reciprocal gate (DS-HY3131 14, the emulator's model): the
    /// restart at t = 0, the gate opens on the first rising edge at `ph` s,
    /// CTA counts SYSCLK from 0xfc0000 and stops on the first edge after the
    /// overflow. Checks as `Dmm::freq_gate` (350 ms, then every 20 ms) until
    /// a result or a timeout: (time ms, Gate, f kHz).
    fn sim_gate(hz: f32, ph: f32, shown_khz: f32) -> (u32, Gate, f32) {
        const CLK: f64 = 4_915_280.0;
        let (hz, ph) = (hz as f64, ph as f64);
        let periods = if hz > 0.0 { (0.0533333 * hz).ceil().max(1.0) } else { 0.0 };
        let stop = ph + periods / hz.max(1e-9);
        let mut t = FREQ_CHECK_MS as u32;
        loop {
            let ts = t as f64 / 1000.0;
            let (ctb, ctc, cta) = if hz <= 0.0 || ts < ph {
                (0, 0, 0xFC_0000)
            } else {
                let te = ts.min(stop);
                let ticks = ((te - ph) * CLK) as u32;
                let cyc = if ts >= stop { periods } else { ((ts - ph) * hz).floor() };
                (cyc as u32, ticks / 2, (0xFC_0000 + ticks) & 0xFF_FFFF)
            };
            // The second CTA read, 50 us later.
            let cta2 = if hz <= 0.0 || ts < ph || ts >= stop { cta } else { cta + 245 };
            let g = gate_check(ctc, ctb, cta, cta2, t, shown_khz);
            if g != Gate::Wait {
                let (f, _) = counter_vals(ctc, ctb, cta);
                return (t, g, f);
            }
            t += FREQ_RECHECK_MS as u32;
        }
    }

    #[test]
    fn freq_gate_low_frequencies() {
        // 0.5 .. 50 Hz, the opening edge anywhere in the first period: a
        // whole gate, within two periods + 53.3 ms (+ a recheck), at most
        // 350 ms above ~6.7 Hz; f to 10 ppm.
        for hz in [0.5f32, 1.0, 2.0, 5.0, 10.0, 50.0, 1000.0] {
            for k in 0..8 {
                let ph = k as f32 / 8.0 / hz;
                for shown in [f32::NAN, 0.0, hz / 1000.0] {
                    let (t, g, f) = sim_gate(hz, ph, shown);
                    assert_eq!(g, Gate::Done, "{hz} Hz ph {ph} shown {shown}");
                    assert!((f * 1000.0 - hz).abs() <= hz * 1e-5, "{hz} Hz: {f} kHz");
                    let worst = (2000.0 / hz + 53.4).max(FREQ_CHECK_MS as f32) + FREQ_RECHECK_MS as f32;
                    assert!(t as f32 <= worst, "{hz} Hz: {t} ms > {worst}");
                    if hz > 6.8 {
                        assert_eq!(t, FREQ_CHECK_MS as u32, "{hz} Hz");
                    }
                }
            }
        }
        // No signal: given up after FREQ_NOEDGE_MS, quiet (no signal at
        // once); with a 1 kHz value shown, at the first check (as before).
        assert_eq!(sim_gate(0.0, 0.0, f32::NAN).1, Gate::Quiet);
        assert!((FREQ_NOEDGE_MS..FREQ_NOEDGE_MS + FREQ_RECHECK_MS as u32).contains(&sim_gate(0.0, 0.0, f32::NAN).0));
        assert_eq!(sim_gate(0.0, 0.0, 1.0), (FREQ_CHECK_MS as u32, Gate::Empty, 0.0));
        // Below the range (0.2 Hz): no whole gate, given up.
        assert!(matches!(sim_gate(0.2, 1.0, f32::NAN).1, Gate::Empty | Gate::Quiet | Gate::Late));
    }

    #[test]
    fn freq_gate_limits() {
        assert_eq!(gate_limit_ms(f32::NAN, false), FREQ_NOEDGE_MS);
        assert_eq!(gate_limit_ms(f32::NAN, true), FREQ_GATE_MAX_MS);
        assert_eq!(gate_limit_ms(0.0, true), FREQ_GATE_MAX_MS);
        // With counts the gate opened: the input may have slowed down.
        assert_eq!(gate_limit_ms(1.0, true), FREQ_GATE_MAX_MS);
        assert_eq!(gate_limit_ms(1.0, false), FREQ_CHECK_MS as u32); // 1 kHz
        assert!((1899..=1900).contains(&gate_limit_ms(0.002, false))); // 2 Hz: 3.5 x 500 + 150
        assert_eq!(gate_limit_ms(0.0005, true), FREQ_GATE_MAX_MS); // 0.5 Hz: 7.15 s capped
        assert_eq!(gate_limit_ms(0.0005, false), FREQ_NOEDGE_MS);
        // A gate still counting below the limit waits; past the overflow
        // but CTA moving is not over.
        assert_eq!(gate_check(5, 1, 0x00_1000, 0x00_1100, 400, f32::NAN), Gate::Wait);
        assert_eq!(gate_check(5, 1, 0x00_1000, 0x00_1000, 400, f32::NAN), Gate::Done);
        assert_eq!(gate_check(5, 1, 0xFD_0000, 0xFD_0000, 400, f32::NAN), Gate::Wait);
        assert_eq!(gate_check(5, 0, 0x00_1000, 0x00_1100, 4400, f32::NAN), Gate::Late);
    }

    #[test]
    fn freq_take_step() {
        // Device 2026-10-26, 1 Hz -> 0.5 Hz: a gate across the step read
        // 1.402781 Hz (an input period cut short), then the slower gates
        // were given up. Poll order as poll_freq_mode (sources 1, 0, ...);
        // `f_khz` as poll_freq_mode sets it.
        let mut q = freq0();
        let mut show = |q: &mut Freq, i: usize, f: f32, g: Gate| -> (FreqPick, bool) {
            let p = q.take(i, f, 50.0, 1, g);
            match p {
                FreqPick::Src(b) => q.f_khz = q.last[b],
                FreqPick::NoSignal => q.f_khz = 0.0,
                FreqPick::None => {}
            }
            (p, q.prov)
        };
        // Entry: the first value is final at once.
        assert_eq!(show(&mut q, 1, 0.001, Gate::Done), (FreqPick::Src(1), false));
        assert_eq!(show(&mut q, 0, 0.001, Gate::Done), (FreqPick::Src(0), false));
        // The step: 1.4 Hz wins on change (source 0 had no earlier value),
        // grey.
        assert_eq!(show(&mut q, 1, 0.0014028, Gate::Done), (FreqPick::Src(1), true));
        assert_eq!(show(&mut q, 0, 0.0, L), (FreqPick::Src(1), true));
        // A late gate (counts, no end) is not "no signal".
        assert_eq!(show(&mut q, 1, 0.0, L), (FreqPick::None, true));
        // 0.5 Hz: grey until a second gate agrees.
        assert_eq!(show(&mut q, 0, 0.0005, Gate::Done), (FreqPick::Src(0), true));
        assert_eq!(show(&mut q, 1, 0.000500011, Gate::Done).1, false);
        // No edge within 3.5 periods of the shown value: no signal, grey;
        // the next one (quiet, or empty again) is final.
        let mut q = freq0();
        show(&mut q, 1, 0.01, Gate::Done);
        assert_eq!(show(&mut q, 0, 0.0, E), (FreqPick::Src(1), false));
        assert_eq!(show(&mut q, 1, 0.0, E), (FreqPick::NoSignal, true));
        assert_eq!(show(&mut q, 0, 0.0, Q), (FreqPick::NoSignal, false));
        // The signal back: its first value is final.
        assert_eq!(show(&mut q, 1, 0.01, Gate::Done), (FreqPick::Src(1), false));
        // A quiet gate (2.2 s without an edge) right after a value: grey too.
        let mut q = freq0();
        show(&mut q, 1, 0.001, Gate::Done);
        assert_eq!(show(&mut q, 0, 0.0, Q), (FreqPick::Src(1), false));
        assert_eq!(show(&mut q, 1, 0.0, Q), (FreqPick::NoSignal, true));
        assert_eq!(show(&mut q, 0, 0.0, Q), (FreqPick::NoSignal, false));
        // After a grey no signal the next value is final at once.
        let mut q = freq0();
        show(&mut q, 1, 0.001, Gate::Done);
        show(&mut q, 0, 0.0, Q);
        show(&mut q, 1, 0.0, Q);
        assert_eq!(show(&mut q, 1, 0.0005, Gate::Done), (FreqPick::Src(1), false));
    }

    #[test]
    fn freq_extra_digit() {
        // One tick of the gate against one count of the extra digit.
        assert!(freq_ext(10.0, 3, 491_520)); // 10 Hz, 100 ms: 0.00002 Hz
        assert!(!freq_ext(50.0, 3, 294_912)); // 50 Hz, 60 ms: 0.00017 Hz
        assert!(freq_ext(2.0, 3, 2_457_640)); // 2 Hz, 500 ms
        assert!(freq_ext(1.0, 4, 262_400)); // 1 kHz (kHz, 4 decimals): 0.0000038 kHz
        assert!(!freq_ext(5.0, 4, 262_400)); // 5 kHz: 0.000019 kHz
        assert!(!freq_ext(10.0, 3, 0)); // no gate
    }

    #[test]
    fn freq_take_quiet_gate() {
        // One quiet gate (2.2 s without counts) is no signal at once after
        // an entry; a source with a clean value keeps showing it.
        let mut q = freq0();
        assert_eq!(q.take(1, 0.0, 0.0, 0, Q), FreqPick::NoSignal);
        let mut q = freq0();
        assert_eq!(q.take(1, 0.04, 50.0, 300_000, Gate::Done), FreqPick::Src(1));
        assert_eq!(q.take(0, 0.0, 0.0, 0, Q), FreqPick::Src(1));
        let mut q = freq0();
        q.take(1, 0.002, 50.0, 2_457_640, Gate::Done);
        assert_eq!(q.lastn[1], 2_457_640);
    }

    #[test]
    fn ctr_valid_gate() {
        assert!(ctr_valid(3, 0x01_2000)); // whole gate: CTA past the overflow
        assert!(!ctr_valid(0, 0x01_2000)); // no edges
        assert!(!ctr_valid(2, 0xFD_0000)); // the 53.3 ms not run out
    }

    #[test]
    fn freq_best_source() {
        let c = [1e6f32; 2];
        // No clean result yet: nothing; one source alone: that one.
        assert_eq!(freq_best([false, false], [0.0; 2], c), None);
        assert_eq!(freq_best([true, false], [0.04, 0.0], c), Some(0));
        assert_eq!(freq_best([false, true], [0.0, 0.04], c), Some(1));
        // Both: the smaller change (ties: source 0).
        assert_eq!(freq_best([true; 2], [0.04, 0.0401], [1e-5, 1e-4]), Some(0));
        assert_eq!(freq_best([true; 2], [0.04, 0.0401], [1e-4, 1e-5]), Some(1));
        assert_eq!(freq_best([true; 2], [0.04, 0.04], c), Some(0));
        // A source at 1/2, 1/3, 2/3 of the other loses, however steady.
        for lo in [0.02f32, 0.013334, 0.026668] {
            assert_eq!(freq_best([true; 2], [lo, 0.040002], [0.0, 1e-3]), Some(1), "{lo}");
            assert_eq!(freq_best([true; 2], [0.040002, lo], [1e-3, 0.0]), Some(0), "{lo}");
        }
        // Not near a ratio, or no signal on one: by the change.
        assert_eq!(freq_best([true; 2], [0.03, 0.04], [0.0, 1e-3]), Some(0));
        assert_eq!(freq_best([true; 2], [0.0, 0.04], [1e6, 1e-3]), Some(1));
        assert_eq!(freq_best([true; 2], [0.0, 0.0], c), Some(0));
    }

    #[test]
    fn code_window_per_rate() {
        // MEDIUM = the stock-plus-+1-digit windows of before.
        assert_eq!(avg_cfg(Rate::Medium, false, 1, 0), Some((8, 120, true)));
        assert_eq!(avg_cfg(Rate::Medium, false, 3, 1), Some((8, 120, true)));
        assert_eq!(avg_cfg(Rate::Medium, false, 10, 2), Some((16, 150, true)));
        assert_eq!(avg_cfg(Rate::Medium, false, 10, 0), Some((16, 150, false)));
        assert_eq!(avg_cfg(Rate::Medium, false, 1, 3), None);
        assert_eq!(avg_cfg(Rate::Medium, false, 8, 0), None);
        assert_eq!(avg_cfg(Rate::Medium, false, 10, 5), None);
        // SLOW: every AD1 DC range, longer.
        assert_eq!(avg_cfg(Rate::Slow, false, 1, 0), Some((16, 120, true)));
        assert_eq!(avg_cfg(Rate::Slow, false, 1, 3), Some((16, 120, false)));
        assert_eq!(avg_cfg(Rate::Slow, false, 8, 1), Some((16, 60, false)));
        assert_eq!(avg_cfg(Rate::Slow, false, 9, 0), Some((16, 57, false)));
        assert_eq!(avg_cfg(Rate::Slow, false, 10, 3), Some((32, 150, true)));
        assert_eq!(avg_cfg(Rate::Slow, false, 10, 6), Some((16, 240, false)));
        // FAST, AC+DC, other functions: none.
        for m in [1, 3, 7, 8, 9, 10] {
            assert_eq!(avg_cfg(Rate::Fast, false, m, 1), None);
            assert_eq!(avg_cfg(Rate::Medium, true, m, 1), None);
        }
        for m in [0, 2, 4, 11, 12, 13, 14, 15] {
            assert_eq!(avg_cfg(Rate::Slow, false, m, 1), None);
        }
    }

    #[test]
    fn extra_digit_only_with_a_full_window() {
        assert!(!ext_ok(true, 1));
        assert!(!ext_ok(true, AVG_FULL - 1));
        assert!(ext_ok(true, AVG_FULL));
        assert!(ext_ok(true, 32));
        assert!(!ext_ok(false, 32));
    }

    #[test]
    fn code_window_step_reset_and_convergence() {
        let mut a = Avg::new();
        // Noise of +-3 codes around 1000: the mean is exact once full.
        let mut m = 0.0;
        for i in 0..20 {
            m = a.push(if i % 2 == 0 { 1003 } else { 997 }, 8, 120);
        }
        assert_eq!(a.n, 8);
        assert_eq!(m, 1000.0);
        // Sub-code resolution: 1/n steps.
        let mut a = Avg::new();
        for x in [1000, 1001, 1001] {
            m = a.push(x, 8, 120);
        }
        assert!((m - 1000.6667).abs() < 1e-3);
        // A step of more than the threshold restarts the window at once.
        let mut a = Avg::new();
        for _ in 0..8 {
            a.push(1000, 8, 120);
        }
        assert_eq!(a.push(1200, 8, 120), 1200.0);
        assert_eq!(a.n, 1);
        // Within the threshold it is averaged in.
        assert_eq!(a.push(1300, 8, 120), 1250.0);
        // A window length change (rate change) restarts it.
        assert_eq!(a.push(1250, 16, 120), 1250.0);
        assert_eq!(a.n, 1);
        // 32 full-scale codes do not overflow.
        let mut a = Avg::new();
        for _ in 0..40 {
            m = a.push(8_388_607, 32, 120);
        }
        assert_eq!(m, 8_388_607.0);
    }

    #[test]
    fn fr05_conversion_periods() {
        // The FR-05 table's times (datasheet: OSR * 30 / SYSCLK, x2 chopped).
        assert_eq!(hy::ad1_period_us_r22(OSR_EARLY), 100_000);
        assert_eq!(hy::ad1_period_us_r22(crate::fastvos::R22_FAST), 100_000);
        assert_eq!(hy::ad1_period_us_r22(crate::fastvos::R22_FAST_B), 100_000);
        assert_eq!(hy::ad1_period_us_r22(R22_MED), 200_000);
        assert_eq!(hy::ad1_period_us_r22(OSR_SLOW), 400_000);
        // Window spans (one reading per conversion in every rate): MEDIUM
        // V r0 8 (1.6 s), ohm r1 16 (6.4 s at 0x17); SLOW 16 (6.4 s), SLOW
        // ohm r0..r3 32 (12.8 s).
        let span = |r, m, g, r22| avg_cfg(r, false, m, g).unwrap().0 as u32 * hy::ad1_period_us_r22(r22);
        assert_eq!(span(Rate::Medium, 1, 0, R22_MED), 1_600_000);
        assert_eq!(span(Rate::Medium, 10, 1, OSR_SLOW), 6_400_000);
        assert_eq!(span(Rate::Slow, 1, 0, OSR_SLOW), 6_400_000);
        assert_eq!(span(Rate::Slow, 10, 3, OSR_SLOW), 12_800_000);
    }

    #[test]
    fn rate_names_and_codes() {
        for (i, r) in Rate::ALL.iter().enumerate() {
            assert_eq!(Rate::from_u8(i as u8), Some(*r));
            assert_eq!(*r as usize, i);
        }
        assert_eq!(Rate::from_u8(3), None);
        assert_eq!(RATE_NAMES[Rate::Medium as usize], "MED");
    }

    /// The AC settle's flat test on the device's 40 Hz square tail (decaf60,
    /// 600 -> 60 V: 4.821 / 4.790 V shown for 4.768 V; the 1 % step took
    /// them) and on a steady AC part.
    #[test]
    fn acs_flat_tail_and_steady() {
        // the old rule took a 0.64 % fall as flat; now it is not
        assert!(!acs_flat(4.821, -1.0, 4.790, true));
        // the next AC phase: a 0.46 % fall over the DC phase gap
        assert!(!acs_flat(4.790, -1.0, 4.76784, false));
        // steady within the phase: rose, then fell 0.001 %
        assert!(acs_flat(4.76784, 0.022, 4.76786, true));
        assert!(acs_flat(4.76786, -0.00002, 4.76781, true));
        // a slow decay: falls of 0.04 % then 0.035 % (q 0.875): 0.25 % left
        assert!(!acs_flat(4.80, 0.0019, 4.7983, true));
        // a fast one: 0.04 % then 0.008 % (q 0.2): 0.002 % left
        assert!(acs_flat(4.80, 0.0019, 4.79962, true));
        // no previous result
        assert!(!acs_flat(-1.0, -1.0, 4.8, true));
        // the RMS window filling after an input step (device ad97fd7): rising is not flat
        assert!(!acs_flat(1.16636, -1.0, 2.38393, true));
        assert!(!acs_flat(0.07952, -1.0, 1.16636, true));
    }

    #[test]
    fn interval_mean() {
        let (mut acc, mut last) = (0.0, None);
        ema_ms(&mut acc, &mut last, 1000);
        assert_eq!(acc, 0.0);
        ema_ms(&mut acc, &mut last, 1107);
        assert_eq!(acc, 107.0);
        for i in 2..40 {
            ema_ms(&mut acc, &mut last, 1000 + 107 * i);
        }
        assert!((acc - 107.0).abs() < 0.01);
    }

    #[test]
    fn acdc_down_needs_the_other_part() {
        // After a reset nothing is known: no step down on one part (the DC
        // phase's hypot(0, DC) of a pure AC signal).
        assert!(!acdc_down_ok(0, 1));
        assert!(!acdc_down_ok(0, 2));
        // The DC part alone does not let the DC phase step down; the AC
        // part does (bit 0), and vice versa for an AC-phase step (bit 1).
        assert!(!acdc_down_ok(2, 1));
        assert!(acdc_down_ok(1, 1));
        assert!(acdc_down_ok(3, 1));
        assert!(!acdc_down_ok(1, 2));
        assert!(acdc_down_ok(3, 2));
    }
}
