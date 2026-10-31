//! Oscilloscope: acquisition (scope_gen_v2_6.h) with the stock timebase,
//! vertical and offset maps (scope_gen_ui_v2_6.h).
//!
//! ADC 3PA1030 (10 bit) on PB0..PB9, clocked by TIM8 CH1 on PC6 at 50 MHz.
//! fs >= 50 MS/s: DMA1 stream 4 ch 6 (TIM1_CH4) GPIOB->IDR -> record;
//! fs <  50 MS/s: stream 3 ch 7 (TIM8_CH2) latches IDR at 50 MHz, stream 4 copies
//! the latch into the record at fs (as the stock firmware).
//! Software trigger: fs > STREAM_FS: one linear record per frame (CAP
//! samples), the crossing searched over every position that leaves a full
//! screen around it; fs <= STREAM_FS: a circular record searched continuously
//! across polls (stream_poll), stopped once the post-trigger part is in.
//! The analog front end is powered by PC4 (the stock raises it on the scope /
//! generator pages and lowers PE2, the DMM side; see rails()). While PC4 is
//! off poll() returns nothing (the scope is paused, the last frame stays).

use crate::hwtrig::{self, HwTrig, Status};
use crate::mmio::{modify32, r32, w32};

const RCU_AHB1EN: u32 = 0x4002_3830;
const RCU_APB1EN: u32 = 0x4002_3840;
const RCU_APB2EN: u32 = 0x4002_3844;
const GPIOB: u32 = 0x4002_0400;
const GPIOC: u32 = 0x4002_0800;
const GPIOD: u32 = 0x4002_0C00;
const GPIOE: u32 = 0x4002_1000;
const GPIOB_IDR: u32 = GPIOB + 0x10;
const TIM1: u32 = 0x4001_0000;
const TIM8: u32 = 0x4001_0400;
const TIM3: u32 = 0x4000_0400;
const DMA1: u32 = 0x4002_6400;
const S3: u32 = DMA1 + 0x10 + 0x18 * 3;
const S4: u32 = DMA1 + 0x10 + 0x18 * 4;
const LIFCR: u32 = DMA1 + 0x08;
const HIFCR: u32 = DMA1 + 0x0C;

/// Settings record (0x080C0000, read only): the scope calibration pairs at
/// record offsets +0x08 (gain) / +0xA8 (offset), `cal_pair`.
///
/// Read through the record's RAM copy (dmm.rs `rec_u32`, `load_record`):
/// the record is in the slow flash bank, and the scope code reads it per
/// frame / measurement, also while an AD2 stream runs (a host's SCOP:MEAS? /
/// WAV:PRE? / DBG:HWTRIG? during a fast-log session: emu/fastlogtest.py
/// --poll, 2026-10-23).
pub fn rec_f32(off: u32) -> f32 {
    f32::from_bits(crate::dmm::rec_u32(off))
}

/// Screen = 400 columns, 10 divisions (graph height 280 rows, 8 x 35).
pub const COLS: usize = 400;
/// Trigger search window (samples), as the stock software refinement
/// around the comparator estimate (hwtrig path only).
const WIN: usize = 800;
/// Highest sample rate searched continuously in software (stream_poll): the
/// ring (CAP samples = 36 ms at 1.25 MS/s) must outlast the main-loop gap
/// between two polls with the pre-trigger part still in it.
const STREAM_FS: u32 = 1_250_000;
/// Longest busy-wait inside a poll for the post-trigger samples (us).
const STREAM_SPIN_US: u64 = 1000;
/// Record buffer (samples) in SRAM0: the raw u16 ring every acquisition
/// mode writes (88 KB, the SRAM budget: docs/native-boot.md "Memory
/// budget"). Debug builds with RAM-hungry features (dbg-touch 1.9 KB,
/// acdc-trace 1.8 KB) give up 1 K samples each to stay above the 8 KiB
/// SRAM floor.
pub const CAP: usize = (44 - cfg!(feature = "dbg-touch") as usize - cfg!(feature = "acdc-trace") as usize) * 1024;
/// The part of BUF lent to the inrush capture (its ring length, as before
/// the record grew).
pub const INRUSH_LEN: usize = 26 * 1024;
/// Linear record length at sample rate `fs`: the whole CAP, but no longer
/// in time than max(26 K samples, 0.1 s), so a slow timebase's pulse / runt
/// frame (one complete linear record each) does not wait longer than
/// before the record grew.
fn lin_len(fs: u32) -> usize {
    (fs as usize / 10).clamp(INRUSH_LEN, CAP)
}

#[link_section = ".dmabuf"]
static mut BUF: [u16; CAP] = [0; CAP];
#[link_section = ".dmabuf"]
static mut LATCH: [u32; 2] = [0; 2];

/// Hardware timer setups (flash 0x08049a54): fs, tim1 psc, tim1 arr, ccr4.
const HW: [(u32, u32, u32, u32); 31] = [
    (50_000_000, 0, 3, 2),
    (25_000_000, 0, 7, 4),
    (12_500_000, 0, 15, 8),
    (10_000_000, 0, 19, 10),
    (6_250_000, 0, 31, 16),
    (5_000_000, 0, 39, 20),
    (2_500_000, 0, 79, 40),
    (1_250_000, 0, 159, 80),
    (1_000_000, 0, 199, 100),
    (625_000, 0, 319, 160),
    (500_000, 0, 399, 200),
    (250_000, 0, 799, 400),
    (125_000, 0, 1599, 800),
    (100_000, 0, 1999, 1000),
    (62_500, 0, 3199, 1600),
    (50_000, 0, 3999, 2000),
    (25_000, 0, 7999, 4000),
    (12_500, 0, 15999, 8000),
    (10_000, 0, 19999, 10000),
    (6_250, 0, 31999, 16000),
    (5_000, 1, 19999, 10000),
    (2_500, 3, 19999, 10000),
    (1_250, 7, 19999, 10000),
    (1_000, 9, 19999, 10000),
    (625, 15, 19999, 10000),
    (500, 19, 19999, 10000),
    (250, 39, 19999, 10000),
    (125, 79, 19999, 10000),
    (100, 99, 19999, 10000),
    (50, 199, 19999, 10000),
    (25, 399, 19999, 10000),
];

/// Timebases (s/div) and the stock Auto entries {fs, num, den, ppp}: each
/// column covers num/den points of the ppp-times upsampled stream.
pub const SDIV: [f32; 30] = [
    10e-9, 20e-9, 50e-9, 100e-9, 200e-9, 500e-9, 1e-6, 2e-6, 5e-6, 10e-6, 20e-6, 50e-6, 100e-6, 200e-6, 500e-6,
    1e-3, 2e-3, 5e-3, 10e-3, 20e-3, 50e-3, 100e-3, 200e-3, 500e-3, 1.0, 2.0, 5.0, 10.0, 20.0, 50.0,
];
const TB: [(u32, u32, u32, u32); 30] = [
    (200_000_000, 1, 1, 20),
    (200_000_000, 1, 1, 10),
    (200_000_000, 1, 1, 4),
    (50_000_000, 1, 1, 8),
    (50_000_000, 1, 1, 4),
    (50_000_000, 5, 4, 2),
    (50_000_000, 5, 4, 1),
    (50_000_000, 5, 2, 1),
    (50_000_000, 25, 4, 1),
    (50_000_000, 25, 2, 1),
    (50_000_000, 25, 1, 1),
    (50_000_000, 125, 2, 1),
    // From 100 us/div: fs halved, 2500 samples/div (stock Normal table), so
    // a screen + trigger window fits the shortest linear record (26 K,
    // `lin_len`).
    (25_000_000, 125, 2, 1),
    (12_500_000, 125, 2, 1),
    (5_000_000, 125, 2, 1),
    (2_500_000, 125, 2, 1),
    (1_250_000, 125, 2, 1),
    (500_000, 125, 2, 1),
    (250_000, 125, 2, 1),
    (125_000, 125, 2, 1),
    (50_000, 125, 2, 1),
    (25_000, 125, 2, 1),
    (12_500, 125, 2, 1),
    (5_000, 125, 2, 1),
    (2_500, 125, 2, 1),
    (1_250, 125, 2, 1),
    (500, 125, 2, 1),
    (250, 125, 2, 1),
    (125, 125, 2, 1),
    (50, 125, 2, 1),
];
/// User timebase range. Stock `SCOPE_TB_USER_MIN`/`MAX` = 3..29 (100 ns/div ..
/// 50 s/div, `scope_gen_ui_v2_6.h:522`/`523`; label list `scope_sdiv[30]`
/// `:494`). opendm40 now matches the full range: 3..29. In YT (non-roll) mode the
/// stock caps the index at 24 (1 s/div, `SCOPE_TB_YT_MAX`, `:524`); 22..29
/// (200 ms .. 50 s/div) run in roll mode when Roll is on.
pub const TB_MIN: usize = 3;
pub const TB_MAX: usize = 29;
/// Highest index usable in YT (non-roll) mode: 1 s/div (stock `SCOPE_TB_YT_MAX`).
pub const TB_YT_MAX: usize = 24;

/// V/div of hardware range r (x1 probe), front-end GPIO row {PC13, PB15, PD12,
/// PD13}, calibration voltage X_r and default gain point.
pub const VDIV: [f32; 10] = [0.01, 0.02, 0.05, 0.1, 0.2, 0.5, 1.0, 2.0, 5.0, 10.0];
const GPIO_ROW: [[u8; 4]; 10] = [
    [1, 1, 1, 0],
    [1, 1, 1, 1],
    [1, 0, 1, 1],
    [1, 0, 0, 1],
    [1, 0, 1, 0],
    [0, 1, 1, 0],
    [0, 1, 1, 1],
    [0, 1, 0, 1],
    [0, 0, 0, 1],
    [0, 0, 1, 0],
];
/// Probe factors (stock `scope_probe[13]`, probe index -3..9): x0.1 .. x1000.
/// Like the stock, the factor relabels only (readouts, V/div, trigger
/// level); the hardware range stays the same.
pub const PROBES: [f32; 13] = [0.1, 0.2, 0.5, 1.0, 2.0, 5.0, 10.0, 20.0, 50.0, 100.0, 200.0, 500.0, 1000.0];
/// Probe button labels (scope settings window, Settings tab).
pub const PROBE_NAMES: [&str; 13] = ["x0.1", "x0.2", "x0.5", "x1", "x2", "x5", "x10", "x20", "x50", "x100", "x200", "x500", "x1000"];
const XR: [f32; 10] = [0.04, 0.04, 0.04, 0.4, 0.4, 0.4, 4.0, 4.0, 4.0, 4.0];
const G_DEFAULT: [f32; 10] = [944.0, 727.0, 598.0, 944.0, 727.0, 598.0, 944.0, 727.0, 598.0, 556.0];

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum TrigMode {
    Auto,
    Normal,
    Single,
}

#[derive(Clone, Copy, PartialEq)]
pub struct Settings {
    pub run: bool,
    pub tb: usize,
    pub range: usize,
    pub ac: bool,
    /// Trigger level and zero position in screen rows (0..280, 140 = centre).
    pub trig_row: i32,
    pub pos: i32,
    pub rising: bool,
    pub mode: TrigMode,
    /// Probe factor (one of PROBES, x0.1 .. x1000): scales readouts and V/div
    /// labels only.
    pub probe: f32,
    /// Trigger position: screen column (0..=400) of the trigger point, 200 =
    /// centre (stock: always the centre). Sets the pre-trigger part of each
    /// record; ignored in roll mode.
    pub hpos: i32,
    /// Trigger type (trigsoft.rs: 0 edge, 1 pulse width, 2 runt). Pulse /
    /// runt are searched in software in a complete linear record (no
    /// comparator, no stream search); `rising` = a positive pulse.
    pub ttype: u8,
    /// Pulse width condition (trigsoft GT / LT / EQ) and width (s).
    pub pcond: u8,
    pub pwidth: f32,
    /// Runt: the second level (row, like trig_row).
    pub runt_row: i32,
    /// Trigger source (TSRC_*): CH1, or the generator's output computed
    /// from its timers (docs/scope-gen-channel.md; edge only, MCX only).
    pub tsrc: u8,
}

/// `Settings::tsrc` values.
pub const TSRC_CH1: u8 = 0;
pub const TSRC_GEN: u8 = 1;
pub const TSRC_NAMES: [&str; 2] = ["CH1", "GEN"];

/// Ticks of the 200 MHz timer clock the generator's output reaches the
/// MCX samples later than its timers say (DAC settling, the output and
/// input stages, the ADC pipeline): taken off the GEN phase of every
/// sample. Default: `DBG:GENTRIG:LAG 188` (37 ticks, 185 ns), measured on
/// the PWM path with the generator looped back to CH1 (PR #64 device
/// check); the DAC path's own lag is not measured yet
/// (docs/scope-gen-channel.md "Device check").
pub const GEN_LAG_DEFAULT: i32 = 37;
static GEN_LAG: core::sync::atomic::AtomicI32 = core::sync::atomic::AtomicI32::new(GEN_LAG_DEFAULT);

pub fn gen_lag() -> i32 {
    GEN_LAG.load(core::sync::atomic::Ordering::Relaxed)
}

pub fn set_gen_lag(ticks: i32) {
    GEN_LAG.store(ticks.clamp(-1_000_000, 1_000_000), core::sync::atomic::Ordering::Relaxed);
}

/// Longest busy-wait (DWT cycles, 1 ms) for the generator's phase before
/// a linear record whose window is shorter than a generator period
/// (`gen_ready`).
const GEN_SPIN: u32 = 200_000;
const DWT_CYCCNT: u32 = 0xE000_1004;

/// Round to the nearest integer (f64, no std; |x| < 2^52).
fn round_f64(x: f64) -> f64 {
    let t = (x + if x < 0.0 { -0.5 } else { 0.5 }) as i64;
    t as f64
}

impl Settings {
    /// Stock defaults: 200 us/div, 1 V/div, DC, rising, Auto, level row 105.
    pub const DEFAULT: Settings =
        Settings { run: true, tb: 13, range: 6, ac: false, trig_row: 105, pos: 0, rising: true, mode: TrigMode::Auto, probe: 1.0, hpos: 200, ttype: 0, pcond: 0, pwidth: 1e-3, runt_row: 70, tsrc: TSRC_CH1 };
}

/// A finished frame: the visible window of the record, or (roll mode, empty
/// `data`) per-column min/max codes in `cols` (u16::MAX = no data yet).
pub struct Frame<'a> {
    pub data: &'a [u16],
    pub cols: Option<&'a [(u16, u16); COLS]>,
    pub trig: Option<usize>,
    pub fs: u32,
    #[allow(dead_code)] // frame counter, not read by the firmware
    pub seq: u32,
}

fn cal_pair(off: u32, i: usize, dflt: (f32, f32)) -> (f32, f32) {
    let a = off + 8 * i as u32;
    let (p0, p1) = (rec_f32(a), rec_f32(a + 4));
    let ok = |v: f32| v.is_finite() && v > 0.0 && v < 1024.0;
    if ok(p0) && ok(p1) && (p0 - p1).abs() > 10.0 { (p0, p1) } else { dflt }
}

fn pin(port: u32, n: u32, high: bool) {
    unsafe { w32(port + 0x18, if high { 1 << n } else { 1 << (n + 16) }) };
}

/// A rail pin's real level: an output (MODER 01) driven high (OCTL).
fn rail_on(port: u32, n: u32) -> bool {
    unsafe { r32(port) >> (2 * n) & 3 == 1 && r32(port + 0x14) >> n & 1 != 0 }
}

fn out(port: u32, n: u32) {
    unsafe {
        modify32(port + 0x0C, 3 << (2 * n), 0);
        modify32(port + 0x08, 0, 3 << (2 * n));
        modify32(port, 3 << (2 * n), 1 << (2 * n));
    }
}

#[link_section = ".dmabuf"]
static mut ROLL_COLS: [(u16, u16); COLS] = [(0, 0); COLS];

/// Roll mode (stock: from 200 ms/div): continuous circular capture.
pub const TB_ROLL: usize = 22;

pub struct Scope {
    pub s: Settings,
    /// Roll display enabled (used from TB_ROLL up).
    pub roll: bool,
    /// DMM rail (PE2) on; `powered` is the scope rail (PC4).
    dmm_on: bool,
    rolling: bool,
    roll_written: u64,
    roll_last_wr: usize,
    roll_next: u64,
    busy: bool,
    deadline: u64,
    seq: u32,
    relay_until: u64,
    last_pc13: u8,
    powered: bool,
    screen: usize,
    rec: usize,
    // Stop-and-zoom: the last captured record, kept in BUF while stopped so a
    // timebase change re-renders it at the new s/div (stock §3 "Zoom", state 6).
    stored_valid: bool,
    stored_len: usize,
    stored_trig: Option<usize>,
    stored_anchor: usize,
    stored_fs: u32,
    /// Timebase the stored record was captured at (a stopped zoom changes
    /// `s.tb` only): the scope CSV header's.
    stored_tb: u8,
    /// Hardware range the stored codes were captured with (the code -> volts
    /// gain); the current range only sets the display scale while stopped.
    stored_range: usize,
    /// Stored record came from roll mode: newest sample at the right edge.
    stored_roll: bool,
    /// The stored record is the frame on screen; false: a complete linear
    /// record poll() did not show (no trigger in it), kept for a STOP
    /// before the next acquisition starts (`finish`, `freeze`).
    stored_drawn: bool,
    /// Emit one re-render of the stored record on the next poll (set on any
    /// settings change while stopped and by `refresh`); no periodic re-render.
    zoom_pending: bool,
    /// Comparator trigger engine (hwtrig.rs): DAC2 level, TIM2 post count
    /// gating TIM1. `hw_mode`: the acquisition in flight uses it (circular
    /// record over CAP); `hw_t0`: its start time (health check).
    pub hw: HwTrig,
    hw_mode: bool,
    hw_t0: u64,
    /// Trigger level and the runt's second level in volts (wave: SI) at
    /// the probe tip: what a scale change keeps (owner 2026-10-25: a V/div
    /// / probe / position change kept the row, so the level in volts
    /// changed). The rows in `s` follow them (`sync_levels`), clamped to
    /// the screen; the hardware / software trigger uses the volts. NaN:
    /// not known yet, taken from the rows at the next `apply`.
    trig_v: f32,
    runt_v: f32,
    /// Position (rows) the stored record was captured with: the offset PWM
    /// had moved its codes by that much (volts(), see v_centre).
    stored_pos: i32,
    /// Continuous software trigger in flight (fs <= STREAM_FS): circular
    /// record over CAP; absolute sample count in roll_written / roll_last_wr.
    stream: bool,
    /// Next absolute sample to examine, its ring index, the hysteresis state
    /// (find_trigger's `armed`) carried across polls, the crossing found.
    st_scan: u64,
    st_idx: usize,
    st_armed: bool,
    st_found: Option<u64>,
    st_poll: u64,
    /// Run-state chip (stock status strings): 0 WAIT, 1 AUTO (untriggered
    /// frame), 2 T'D (triggered frame), 3 RUN (roll).
    pub status: u8,
    last_frame: u64,
    /// The stored record is being read (scope CSV, scope_csv.rs): `apply`
    /// keeps the scope stopped, so BUF is not overwritten.
    pub hold: bool,
    /// A host reads the stored record (WAV:DATA?, scope_wav.rs, streamed
    /// over USBTMC): kept stopped like `hold`; set by the main loop.
    pub host_read: bool,
    /// DIGitize (SCPI): stop after the next frame (triggered or, in Auto,
    /// the untriggered one), whatever the sweep; no roll meanwhile. Cleared
    /// by any run / stop change (`apply`).
    pub one_shot: bool,
    /// Source DMM (the wave view, `wave_enter`): the record is the HY3131
    /// AD2 stream drained from the logger ring (`WaveSource`), not the MCX
    /// DMA; `wv` its state, `mcx` the MCX settings to restore on exit.
    wave: bool,
    /// BUF is lent to the inrush capture (src/inrush.rs, `lend_buf`): no
    /// acquisition, no stored record, until `return_buf`.
    lent: bool,
    /// Code -> SI of the stored record (wave: `stored_ab`, the scale it
    /// was captured with).
    stored_ab: (f32, f32),
    /// The generator against the acquisition in flight (its sample 0;
    /// None: the generator was off, the reading failed, a comparator
    /// record or the wave view) and the generator epoch it was read in;
    /// `rec_base`: the absolute sample index of BUF[0] for the next
    /// `snapshot_mark` (a linearised ring); `stored_gen`: against the
    /// stored record's BUF[0].
    gsync: Option<(crate::gen::Sync, u32)>,
    rec_base: u64,
    stored_gen: Option<crate::gen::Sync>,
    /// The GEN trace is shown: every MCX record needs the generator's
    /// phase, so the comparator (whose stop time is not known) is off.
    pub gen_trace: bool,
    /// The DMM CH2 is shown (docs/scope-gen-channel.md section 2): every
    /// record needs its time (`rec_time`), the comparator is off.
    pub ch2_on: bool,
    /// The acquisition in flight: DWT count of its sample 0 (None: a
    /// comparator record, the wave view) and DWT cycles per sample; the
    /// stored record's (BUF[0]).
    t_s0: Option<u32>,
    t_step: f32,
    stored_t0: Option<u32>,
}

/// The DMM-input source of the scope page (docs/dmm-input-scope-
/// feasibility.md 5.2 point 3): what the record needs from the AD2 stream.
/// Implemented over src/logger.rs `wave` (the ring the stream ISR fills
/// and the ISR's trigger, trigsoft::Stream on every sample) in app.rs; a
/// host test can feed synthetic samples.
pub trait WaveSource {
    /// Pop every buffered AD2 sample (19-bit signed, oldest first).
    fn drain(&mut self, f: &mut dyn FnMut(i32)) -> u32;
    /// Holes in the stream so far (ring full, conversions the ISR missed):
    /// a change restarts the record after the hole.
    fn dropped(&self) -> u32;
    /// Trigger configuration for the ISR (`min`: the first sample index
    /// `first` reports); its generation.
    fn set_trigger(&mut self, c: &crate::trigsoft::StreamCfg, min: u32) -> u32;
    fn trigger_off(&mut self);
    /// Events (sample indices) since `*seen`, oldest first.
    fn events(&mut self, seen: &mut u32, f: &mut dyn FnMut(u32));
    fn event_count(&self) -> u32;
    /// The first event at or after the minimum of generation `gen`.
    fn first(&self, gen: u32) -> Option<u32>;
}

/// The DMM wave view's timebases: 1 ms/div (index 15) .. 50 s/div, roll
/// from 200 ms/div as the MCX scope (docs/dmm-input-scope-feasibility.md
/// 3.1: the AD2 band is ~0.25 x the rate, nothing faster to show; 1 ms/div
/// needs the chip's top rate, 76.8 kS/s, 11.12).
pub const WAVE_TB_MIN: usize = 15;

/// AD2 rate (a hy3131::ad2_r26_for rung) for a wave timebase: >= 400
/// samples per screen up to 20 ms/div: 76800 S/s at 1 ms/div and 38400 at
/// 2 ms/div (768 samples on the 10 divisions; the hybrid / plain DMA
/// frames, feasibility 11.12 / 11.11), 9600 S/s at 5 ms/div. The chip's
/// sinc^3 decimator band-limits each rung (~0.25 x the rate): the RMS of a
/// square's samples reads 0.5 / 1 / 1.9 / 3.8 % low at 9600 / 4800 / 2400 /
/// 1200 S/s (device d8e249d: Vrms -1.6 % from 50 ms/div;
/// docs/dmm-input-scope-feasibility.md 3.2).
#[inline(always)] // (one more caller, tb_rate, must not outline it)
pub const fn wave_fs_for(tb: usize) -> u32 {
    match tb {
        0..=15 => 76800,
        16 => 38400,
        17 => 9600,
        18 | 19 => 4800,
        20 | 21 => 2400,
        _ => 1200,
    }
}

/// The sample rate (S/s) timebase `tb` gives: MCX, or the Wave view's
/// stream (`wave`); `Scope::sample_rate` (ACQ:SRAT?) at that timebase.
pub fn tb_rate(tb: usize, wave: bool) -> u32 {
    if wave { wave_real_fs(wave_fs_for(tb)) } else { TB[tb].0 }
}

/// The sample rate a wave frame's measurements, FFT and CSV use for the
/// stream rung `fs` (`wave_fs_for`): the chip's real rate (hy3131.rs
/// AD2_OSR32_REAL_MHZ, the datasheet's since device c830e09: its 76 575 made
/// the Wave view's 1 ms/div read a 40 Hz square as 39.88 Hz).
pub const fn wave_real_fs(fs: u32) -> u32 {
    if fs == 76_800 { (crate::hy3131::ad2_real_rate_mhz(0) + 500) / 1000 } else { fs }
}

/// One vertical step of the wave view: V/div (SI), the MF range item
/// (`ui::range_items`, `Dmm::set_range`) it shows, and whether it is that
/// range's full-scale step (+-full on the 8 divisions: V/div = full / 4).
/// Ascending; the V/div + / - buttons, the pinch and AUTO step this list.
pub type WaveStep = (f32, u8, bool);

/// V function: 60 mV (item 5), 600 mV (0), 6 / 60 / 600 / 1000 V (1..4).
/// Between two full-scale steps the 1-2-5 values; each runs on the lowest
/// range whose AD2 window holds its +-4 divisions with >= 20 % to spare
/// (feasibility 3.3: +-125 mV, +-1.25 V, +-12.5 V, +-125 V, +-1250 V), so
/// the range changes only where a step needs it (2 V/div stays on 6 V).
pub const WAVE_STEPS_V: [WaveStep; 19] = [
    (0.015, 5, true),
    (0.02, 5, false),
    (0.05, 0, false),
    (0.1, 0, false),
    (0.15, 0, true),
    (0.2, 0, false),
    (0.5, 1, false),
    (1.0, 1, false),
    (1.5, 1, true),
    (2.0, 1, false),
    (5.0, 2, false),
    (10.0, 2, false),
    (15.0, 2, true),
    (20.0, 2, false),
    (50.0, 3, false),
    (100.0, 3, false),
    (150.0, 3, true),
    (200.0, 3, false),
    (250.0, 4, true),
];

/// A function: 600 / 6000 uA, 60 / 600 mA, 6 / 10 A (items 0..5); windows
/// +-2.6 / +-26 mA, +-250 mA, +-2.5 A, +-12.5 A (10 A: its own full-scale
/// step only). 500 mA/div -> 1 A/div moves from the mA to the A jack, as
/// the range step always did.
pub const WAVE_STEPS_A: [WaveStep; 19] = [
    (150e-6, 0, true),
    (200e-6, 0, false),
    (500e-6, 0, false),
    (1e-3, 1, false),
    (1.5e-3, 1, true),
    (2e-3, 1, false),
    (5e-3, 1, false),
    (10e-3, 2, false),
    (15e-3, 2, true),
    (20e-3, 2, false),
    (50e-3, 2, false),
    (0.1, 3, false),
    (0.15, 3, true),
    (0.2, 3, false),
    (0.5, 3, false),
    (1.0, 4, false),
    (1.5, 4, true),
    (2.0, 4, false),
    (2.5, 5, true),
];

/// The wave view's step list of the V (`amps` false) or A function.
pub fn wave_steps(amps: bool) -> &'static [WaveStep] {
    if amps { &WAVE_STEPS_A } else { &WAVE_STEPS_V }
}

/// Index of the full-scale step of range item `item` (None: no such
/// range in the list).
pub fn wave_full_step(amps: bool, item: u8) -> Option<usize> {
    wave_steps(amps).iter().position(|s| s.1 == item && s.2)
}

/// Index of the step showing `vdiv` (exact: the values come from the list).
pub fn wave_step_of(amps: bool, vdiv: f32) -> Option<usize> {
    wave_steps(amps).iter().position(|s| s.0 == vdiv)
}

/// The pinch in the wave view: the step for span ratio `r` (now / start;
/// fingers apart = zoom in) from step `i0`, taken once its V/div is
/// reached (as PINCH_STEP: a step only once its ratio is reached).
pub fn wave_pinch_step(amps: bool, i0: usize, r: f32) -> usize {
    let st = wave_steps(amps);
    let i0 = i0.min(st.len() - 1);
    let want = st[i0].0 / r.max(1e-3);
    if r >= 1.0 {
        (0..=i0).find(|&i| st[i].0 >= want * 0.999).unwrap_or(i0)
    } else {
        (i0..st.len()).rev().find(|&i| st[i].0 <= want * 1.001).unwrap_or(i0)
    }
}

/// Minimum interval between two triggered wave frames (ms): the drawing,
/// not the trigger, is the limit (25 frames/s).
const WAVE_FRAME_MS: u64 = 40;

/// Scale of a wave record: SI = a + b * code (10-bit code, trigsoft::
/// ad2_code10), the V/div shown (a `wave_steps` value: only the screen,
/// the codes do not depend on it), unit and coupling (dmm::Ad2Scale).
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct WaveScale {
    pub a: f32,
    pub b: f32,
    pub vdiv: f32,
    pub amps: bool,
    pub ac: bool,
}

impl WaveScale {
    /// From the raw-code scale (SI = raw * per_code - offset): a code
    /// stands for the centre of its 512 raw codes.
    pub fn of(per_code: f32, offset: f32, vdiv: f32, amps: bool, ac: bool) -> WaveScale {
        let b = 512.0 * per_code;
        WaveScale { a: (256.0 - 512.0 * 512.0) * per_code - offset, b, vdiv, amps, ac }
    }
}

#[derive(Clone, Copy)]
struct Wave {
    sc: WaveScale,
    /// Rate of the stream the record is taken at; `restart`: the app must
    /// (re)start the stream at `fs` (`take_wave_restart`); `live`: it runs
    /// at `fs` and the record follows it (`wave_live`).
    fs: u32,
    restart: bool,
    live: bool,
    /// Samples drained this stream (= the ring / ISR sample index, u64),
    /// the index stored at BUF[0] (slot of n = (n - slot0) % CAP), the
    /// first index of the current record (after a resync), the ring's drop
    /// count seen.
    tot: u64,
    slot0: u64,
    first: u64,
    dropped: u32,
    /// Discard the backlog at the next arm (after STOP, a drop).
    stale: bool,
    /// Trigger: ISR generation, events seen, the earliest index the next
    /// frame may trigger at, an incomplete candidate (post part not in),
    /// a complete one waiting for the frame interval.
    gen: u32,
    seen: u32,
    next_min: u64,
    pend: Option<u64>,
    ready: Option<u64>,
    last_emit: u64,
    /// The last frame shown: its centre index and trigger (STOP keeps it).
    shown: Option<(u64, Option<u64>)>,
    /// Roll: samples per column, the current column's min / max and count.
    per: u32,
    racc: (u16, u16),
    rn: u32,
    /// Settings of the wave view (kept across entries, `wave_exit`) and
    /// the MCX settings to restore.
    s: Settings,
    mcx: Settings,
}

/// The wave state: a static, not a field of `Scope` in the App on the main
/// stack. On the device in SRAM (.dmabuf, NOLOAD: no 224-byte .data load
/// image in region B; region B audit #6), set up by `Scope::new` before any
/// other use. Main loop only.
#[cfg_attr(target_os = "none", link_section = ".dmabuf")]
static mut WAVE: Wave = Wave::NEW;

fn wv() -> &'static mut Wave {
    // SAFETY: main loop only (the ISR never sees it), no reference kept
    // across a call that takes another.
    unsafe { &mut *core::ptr::addr_of_mut!(WAVE) }
}

impl Wave {
    const NEW: Wave = Wave {
        sc: WaveScale { a: 0.0, b: 1.0, vdiv: 1.0, amps: false, ac: false },
        fs: 4800,
        restart: false,
        live: false,
        tot: 0,
        slot0: 0,
        first: 0,
        dropped: 0,
        stale: true,
        gen: 0,
        seen: 0,
        next_min: 0,
        pend: None,
        ready: None,
        last_emit: 0,
        shown: None,
        per: 1,
        racc: (u16::MAX, 0),
        rn: 0,
        s: Settings { tb: 18, trig_row: 140, ..Settings::DEFAULT },
        mcx: Settings::DEFAULT,
    };
}

impl Scope {
    pub fn new() -> Self {
        unsafe {
            modify32(RCU_AHB1EN, 0, (1 << 22) | 0x1F);
            modify32(RCU_APB2EN, 0, 0b11);
            modify32(RCU_APB1EN, 0, 1 << 1);
            // PB0..PB9 inputs with pull-down.
            modify32(GPIOB, 0xF_FFFF, 0);
            modify32(GPIOB + 0x0C, 0xF_FFFF, 0xA_AAAA);
            // PC6: TIM8_CH1 (AF3). PC8: TIM3_CH3 (AF2) offset PWM.
            modify32(GPIOC + 0x20, 0xF << 24, 3 << 24);
            modify32(GPIOC + 0x08, 3 << 12, 3 << 12);
            modify32(GPIOC, 3 << 12, 2 << 12);
            modify32(GPIOC + 0x24, 0xF, 2);
            modify32(GPIOC, 3 << 16, 2 << 16);
            w32(TIM3 + 0x28, 0);
            w32(TIM3 + 0x2C, 20000);
            w32(TIM3 + 0x1C, (6 << 4) | (1 << 3));
            w32(TIM3 + 0x20, 1 << 8);
            w32(TIM3 + 0x14, 1);
            w32(TIM3, 1 | (1 << 7));
        }
        // Range / coupling GPIOs; PC4 / PE2 keep their current levels.
        for (p, n) in [(GPIOC, 13), (GPIOB, 15), (GPIOD, 12), (GPIOD, 13), (GPIOE, 3)] {
            out(p, n);
        }
        let mut sc = Scope {
            s: Settings::DEFAULT,
            busy: false,
            deadline: 0,
            seq: 0,
            relay_until: 0,
            last_pc13: 2,
            // The levels board::init left (PE2 = 1, PC4 = 0). This used to
            // claim "scope on" while PC4 was low: rails() then never raised
            // PC4 with Both always or a non-DMM start page, and a later
            // switch to the scope dropped PE2 with PC4 still low, both rails
            // off = the power hold gone (the meter switched off, 2026-10-26).
            powered: rail_on(GPIOC, 4),
            screen: 0,
            rec: 0,
            roll: true,
            dmm_on: rail_on(GPIOE, 2),
            rolling: false,
            roll_written: 0,
            roll_last_wr: 0,
            roll_next: 0,
            stored_valid: false,
            stored_len: 0,
            stored_trig: None,
            stored_anchor: 0,
            stored_fs: 0,
            stored_tb: 0,
            stored_range: 0,
            stored_roll: false,
            stored_drawn: false,
            zoom_pending: false,
            hw: HwTrig::new(),
            hw_mode: false,
            hw_t0: 0,
            trig_v: f32::NAN,
            runt_v: f32::NAN,
            stored_pos: 0,
            stream: false,
            st_scan: 0,
            st_idx: 0,
            st_armed: false,
            st_found: None,
            st_poll: 0,
            status: 0,
            last_frame: 0,
            hold: false,
            host_read: false,
            one_shot: false,
            wave: false,
            lent: false,
            stored_ab: (0.0, 1.0),
            gsync: None,
            rec_base: 0,
            stored_gen: None,
            gen_trace: false,
            ch2_on: false,
            t_s0: None,
            t_step: 1.0,
            stored_t0: None,
        };
        // WAVE is not loaded on the device (.dmabuf): set up in place.
        unsafe { core::ptr::addr_of_mut!(WAVE).write(Wave::NEW) };
        sc.hw.init();
        // Comparator trigger on by default (the stock always uses it): the
        // software search of a linear record only covered CAP - screen = 1624
        // samples from 100 us/div up (65 us .. 0.65 ms at 100 us .. 1 ms/div),
        // so a slow signal triggered only now and then (device 2026-10-14:
        // 100 Hz square, 1/5 .. 4/5 frames T'D at 100 us .. 1 ms/div; with
        // the comparator 16/16, 1891 hits, 0 misses, refined crossing -1..+7
        // samples from the estimate). Three misses in a row still hand the
        // scope back to the software trigger until the next range / timebase
        // / coupling / edge / mode change (apply); DBG:HWTRIG:ENABLE 0|1 switches
        // it at run time. hwtrig::DEFAULT_ENABLED stays the module default.
        sc.hw.set_enabled(true);
        sc.apply(Settings::DEFAULT, 0);
        sc
    }

    /// Analog rails: DMM side PE2, scope / generator side PC4 (the stock
    /// raises PC4 on its scope / generator pages and PE2 on the DMM page, never
    /// both). A rail that comes up does so 10 ms before one goes down, as the
    /// stock page switch; at least one rail stays on. Cheap when nothing
    /// changes (called every tick by App::update_rails). Decided on the pins' real levels, not on what was last asked: a rail
    /// can never be believed on while it is off, so one of the two always
    /// stays on (both off drops the power hold).
    pub fn rails(&mut self, dmm: bool, scope: bool, clk: &mut crate::clock::Clock) {
        let dmm = dmm || !scope;
        let (pe2, pc4) = (rail_on(GPIOE, 2), rail_on(GPIOC, 4));
        if dmm == pe2 && scope == pc4 {
            self.dmm_on = dmm;
            self.powered = scope;
            return;
        }
        if scope && !pc4 {
            pin(GPIOC, 4, true);
            out(GPIOC, 4);
            self.relay_until = clk.now_ms() + 200;
        }
        if dmm && !pe2 {
            pin(GPIOE, 2, true);
            out(GPIOE, 2);
        }
        if (!scope && pc4) || (!dmm && pe2) {
            clk.delay_ms(10);
        }
        if !scope && (pc4 || self.powered) {
            self.halt();
            pin(GPIOC, 4, false);
            out(GPIOC, 4);
        }
        if !dmm && pe2 {
            pin(GPIOE, 2, false);
            out(GPIOE, 2);
        }
        self.powered = scope;
        self.dmm_on = dmm;
    }

    /// DMM rail (PE2) on.
    pub fn dmm_rail(&self) -> bool {
        self.dmm_on
    }

    pub fn powered(&self) -> bool {
        self.powered
    }

    fn gain(&self, r: usize) -> (f32, f32) {
        cal_pair(0x08, r, (G_DEFAULT[r], 512.0))
    }

    /// Hardware range of the codes currently being served: the capture range
    /// of the stored record while stopped, else the live range. A V/div change
    /// while stopped then rescales the old codes with the gain they were taken
    /// at (a digital vertical zoom) instead of re-mapping them with the new
    /// range's gain. Right after STOP the two are equal, so the re-render maps
    /// every code to the same row as the live frame did.
    fn code_range(&self) -> usize {
        if !self.s.run && self.stored_valid { self.stored_range } else { self.s.range }
    }

    /// Voltage of ADC code g1 (stock v_c = pos * X / 141, FUN_080301a0): the
    /// offset PWM (apply) puts 0 V at code g1 - pos*..., so code g1 sits on
    /// the centre row whatever the position, and the position is already in
    /// the codes. Uses the capture position / range of the stored record
    /// while stopped (a position drag then moves the stored trace, as the
    /// stock STOP re-render).
    fn v_centre(&self) -> f32 {
        let (r, pos) = if !self.s.run && self.stored_valid { (self.stored_range, self.stored_pos) } else { (self.s.range, self.s.pos) };
        pos as f32 * 4.0 * VDIV[r] * self.s.probe / 141.0
    }

    /// ADC code -> volts (FUN_080301a0: v_c + (code - g1) X_r P / (g0 - g1)).
    /// The probe is the current one: it only relabels (stock: "P scales the
    /// sample map and the displayed V/div only"), so a probe change while
    /// stopped relabels the stored trace without moving it.
    pub fn volts(&self, code: f32) -> f32 {
        if self.wave {
            let (a, b) = if !self.s.run && self.stored_valid { self.stored_ab } else { (wv().sc.a, wv().sc.b) };
            return a + b * code;
        }
        let r = self.code_range();
        let (g0, g1) = self.gain(r);
        self.v_centre() + (code - g1) * XR[r] / (g0 - g1) * self.s.probe
    }

    /// Volts -> screen row (FUN_08030146): 140 + pos - 141 v / (4 V/div).
    pub fn row_of_volts(&self, v: f32) -> f32 {
        140.0 + self.s.pos as f32 - 141.0 * v / (4.0 * self.vdiv())
    }

    /// Volts (wave: SI, V or A) per division at the probe tip / the jacks.
    pub fn vdiv(&self) -> f32 {
        if self.wave { wv().sc.vdiv } else { VDIV[self.s.range] * self.s.probe }
    }

    pub fn row_of_code(&self, code: f32) -> f32 {
        self.row_of_volts(self.volts(code))
    }

    /// TRIG:LEV: the level in volts as given, also off screen (its marker
    /// on the nearest edge row, as a level kept across a zoom). Returns the
    /// marker row for the settings the caller applies next (`apply` keeps
    /// the volts: the row does not move on its own).
    pub fn set_trig_volts(&mut self, v: f32) -> i32 {
        let r = self.row_of_volts(v);
        let row = if r.is_finite() { (if r >= 0.0 { r + 0.5 } else { r - 0.5 }).clamp(-10.0, 290.0) as i32 } else { self.s.trig_row };
        self.trig_v = v;
        self.s.trig_row = row;
        row
    }

    pub fn trig_volts(&self) -> f32 {
        if self.trig_v.is_finite() { self.trig_v } else { self.row_volts(self.s.trig_row) }
    }

    /// The runt's second level in volts (as `trig_volts`).
    pub fn runt_volts(&self) -> f32 {
        if self.runt_v.is_finite() { self.runt_v } else { self.row_volts(self.s.runt_row) }
    }

    /// V/div under settings `s` (the live wave scale in the wave view).
    fn vdiv_of(&self, s: &Settings) -> f32 {
        if self.wave { wv().sc.vdiv } else { VDIV[s.range.min(9)] * s.probe }
    }

    /// Keep the levels in volts across the change self.s -> `s` (`apply`,
    /// `preview`): a row that did not move, or moved only with the position
    /// (CH1 drag / centring keep the level's volts), is re-derived from the
    /// volts at the new scale (clamped to the screen rows -10..290, the
    /// level itself stays: off screen its marker sits at the edge); a row
    /// moved on its own (a level drag, AUTO, TRIG:LEV) sets the volts.
    fn sync_levels(&mut self, s: &mut Settings) {
        let o = self.s;
        let vd = self.vdiv_of(s);
        let dp = s.pos - o.pos;
        let volts = |row: i32| (140.0 + s.pos as f32 - row as f32) * 4.0 * vd / 141.0;
        let row = |v: f32| {
            let r = 140.0 + s.pos as f32 - 141.0 * v / (4.0 * vd);
            if r.is_finite() { (if r >= 0.0 { r + 0.5 } else { r - 0.5 }).clamp(-10.0, 290.0) as i32 } else { 140 }
        };
        let follows = |new: i32, old: i32| new == old || new == old + dp;
        if self.trig_v.is_finite() && follows(s.trig_row, o.trig_row) {
            s.trig_row = row(self.trig_v);
        } else {
            self.trig_v = volts(s.trig_row);
        }
        if self.runt_v.is_finite() && follows(s.runt_row, o.runt_row) {
            s.runt_row = row(self.runt_v);
        } else {
            self.runt_v = volts(s.runt_row);
        }
    }

    /// Forget the levels' volts: the next `apply` takes them from its rows
    /// (settings loaded from flash at their saved scale, the wave view's /
    /// MCX's own settings swapped in).
    pub fn levels_reset(&mut self) {
        self.trig_v = f32::NAN;
        self.runt_v = f32::NAN;
    }

    /// The levels' rows again at the current scale (the wave view's DMM
    /// range changed outside `apply`).
    fn levels_follow(&mut self) {
        let mut s = self.s;
        self.sync_levels(&mut s);
        self.s.trig_row = s.trig_row;
        self.s.runt_row = s.runt_row;
    }

    /// Screen row (unrounded, unclamped) of a level in volts.
    fn level_row(&self, v: f32, row: i32) -> f32 {
        let r = self.row_of_volts(v);
        if v.is_finite() && r.is_finite() { r } else { row as f32 }
    }

    /// Trigger level in ADC codes of the live range: relative to the centre
    /// row (code g1), since the offset PWM already shifted the codes by the
    /// position (stock FUN_08020e2a: code = v/k - c with the position in v
    /// and in c). Was g1 + trig_volts * k, which counted the position twice:
    /// after AUTOSET on a 0..9.6 V square (5 V/div, pos 33) the level sat at
    /// 9.36 V for a badge of 4.68 V.
    /// From the level in volts (`trig_v`): a level off screen after a zoom
    /// stays where it is, not at the clamped marker row.
    fn trig_code(&self) -> i32 {
        self.code_of_row(self.level_row(self.trig_v, self.s.trig_row))
    }

    /// ADC code of the runt's second level (from `runt_v`).
    fn runt_code(&self) -> i32 {
        self.code_of_row(self.level_row(self.runt_v, self.s.runt_row))
    }

    /// ADC code of a level row (trig_code; the runt's second level).
    fn code_of_row(&self, row: f32) -> i32 {
        if self.wave {
            // The wave codes are not shifted by the position (no offset
            // PWM): the level row's SI value through the live scale.
            let v = (140.0 + self.s.pos as f32 - row) * 4.0 * self.vdiv() / 141.0;
            let c = (v - wv().sc.a) / wv().sc.b;
            return if c.is_finite() { c.clamp(-100.0, 1124.0) as i32 } else { 512 };
        }
        let r = self.s.range;
        let (g0, g1) = self.gain(r);
        let dv = (140.0 - row) * 4.0 * VDIV[r] / 141.0;
        let c = g1 + dv * (g0 - g1) / XR[r];
        // Off screen (a level kept across a zoom): beyond the ADC span, no
        // crossing (i32 range kept sane).
        if c.is_finite() { c.clamp(-100_000.0, 100_000.0) as i32 } else { g1 as i32 }
    }

    /// Volts of a level row (the runt's second level).
    pub fn row_volts(&self, row: i32) -> f32 {
        (140.0 + self.s.pos as f32 - row as f32) * 4.0 * self.vdiv() / 141.0
    }

    /// Software trigger of a pulse / runt type in a linear record: the
    /// trigger point in [lo, hi), or None (trigsoft.rs).
    fn find_soft(&self, d: &[u16], lo: usize, hi: usize) -> Option<usize> {
        let l = self.trig_code();
        let w = self.s.pwidth * self.sample_rate() as f32;
        match self.s.ttype {
            crate::trigsoft::PULSE => crate::trigsoft::pulse(d, lo, hi, l, self.s.rising, self.s.pcond, w),
            _ => crate::trigsoft::runt(d, lo, hi, l, self.runt_code(), self.s.rising),
        }
    }

    /// Stock auto timeout (ms) of the current timebase (scope_auto_timeout_ms).
    fn auto_timeout(&self) -> u64 {
        match self.s.tb {
            0..=15 => 100,
            16 => 200,
            17 => 500,
            18 => 1000,
            _ => 2000,
        }
    }

    fn set_status(&mut self, st: u8) {
        self.status = st;
    }

    pub fn apply(&mut self, s: Settings, now: u64) {
        let mut s = s;
        if self.held() && !self.s.run {
            s.run = false;
        }
        if s.run != self.s.run {
            self.one_shot = false;
        }
        // A new trigger mode or a RUN press waits for its first frame (WAIT).
        // So does leaving the roll range: the roll chip (RUN) and the roll
        // trace would otherwise stay until the first triggered frame (in
        // Normal possibly never).
        let rolls = |x: &Settings| self.roll && x.tb >= TB_ROLL && x.mode != TrigMode::Single;
        if s.mode != self.s.mode || (s.run && !self.s.run) || (self.status == 3 && !rolls(&s)) {
            self.set_status(0);
        }
        // A user STOP (run -> stop) freezes what is on screen; any other change
        // just aborts the acquisition in flight (it restarts in poll()).
        let redraw = if self.s.run && !s.run {
            self.freeze(now)
        } else {
            self.halt();
            true
        };
        // Roll on: full 3..29 (22.. rolls). Roll off: YT only, capped at 24
        // (1 s/div). Single never rolls (one linear record), so it is capped at
        // the YT maximum too: at 2..50 s/div a full linear record would take
        // 21..532 s with nothing drawn until it completes.
        let top = if self.roll && s.mode != TrigMode::Single { TB_MAX } else { TB_YT_MAX };
        s.tb = s.tb.clamp(if self.wave { WAVE_TB_MIN } else { TB_MIN }, top);
        // Stop-and-zoom: while stopped any change re-renders the stored record
        // at the new settings. The timebase is never clamped to the captured
        // extent: zooming out past it shows the record at the true scale with
        // empty columns around it (zoom_frame), so neither the STOP press nor a
        // later change silently picks a different s/div.
        if !s.run && self.stored_valid && redraw {
            self.zoom_pending = true;
        }
        s.range = s.range.min(9);
        s.trig_row = s.trig_row.clamp(-10, 290);
        s.runt_row = s.runt_row.clamp(-10, 290);
        s.ttype = s.ttype.min(2);
        s.pcond = s.pcond.min(2);
        s.pwidth = if s.pwidth.is_finite() { s.pwidth.clamp(crate::trigsoft::W_MIN, crate::trigsoft::W_MAX) } else { 1e-3 };
        s.pos = s.pos.clamp(-392, 392);
        s.hpos = s.hpos.clamp(0, COLS as i32);
        self.sync_levels(&mut s);
        // A comparator suspended by misses (hwtrig::note_miss: overdrive, a
        // settling front end, a bad map) gets another chance with a new
        // range / timebase / coupling / edge / mode / trigger type, e.g. the
        // result of AUTO, whose sweep through overdriven ranges collected
        // the misses. Level / position drags do not resume it (a broken map
        // would cost up to three miss frames per drag step).
        let o = &self.s;
        if s.range != o.range || s.tb != o.tb || s.ac != o.ac || s.rising != o.rising || s.mode != o.mode || s.ttype != o.ttype {
            self.hw.resume();
        }
        if self.wave {
            // No MCX hardware: the record's scale and coupling are the DMM
            // row's, the probe does not apply; a timebase in another rate
            // band restarts the stream (the app, `take_wave_restart`).
            s.ac = wv().sc.ac;
            s.probe = 1.0;
            self.s = s;
            let fs = wave_fs_for(s.tb);
            if fs != wv().fs {
                wv().fs = fs;
                wv().restart = true;
                wv().live = false;
            }
            self.screen = ((25_000u64 * fs as u64 / TB[s.tb].0.max(1) as u64) as usize).clamp(2, CAP - 2600);
            self.rec = CAP;
            self.deadline = 0;
            return;
        }
        self.s = s;
        let g = GPIO_ROW[s.range];
        if g[0] != self.last_pc13 {
            self.last_pc13 = g[0];
            self.relay_until = now + 200;
        }
        pin(GPIOC, 13, g[0] != 0);
        pin(GPIOB, 15, g[1] != 0);
        pin(GPIOD, 12, g[2] != 0);
        pin(GPIOD, 13, g[3] != 0);
        pin(GPIOE, 3, !s.ac);
        // Offset PWM (FUN_080302d6): ADC code of 0 V at this position.
        let r = s.range;
        let (g0, g1) = self.gain(r);
        let code_t = g1 - s.pos as f32 * (4.0 * VDIV[r] / XR[r]) * (g0 - g1) / 141.0;
        let (o0, o1) = cal_pair(0xA8, r, (730.0, 280.0));
        let ccr = 8800.0 + (code_t - o1) * (11636.0 - 8800.0) / (o0 - o1);
        let ccr = if ccr.is_finite() && ccr > 0.0 { ccr.min(20000.0) as u32 } else { 0 };
        unsafe { w32(TIM3 + 0x3C, ccr) };
        let (_, num, den, ppp) = TB[s.tb];
        self.screen = ((num * COLS as u32 / den / ppp) as usize).max(2);
        // Every linear record is the full CAP: the trigger is searched over all
        // positions that keep a full screen around it (finish), CAP - screen
        // of them instead of the 800-sample refinement window, and Single keeps
        // the full record for the stored-record zoom (stock Single = 65520).
        self.rec = lin_len(TB[s.tb].0);
        self.deadline = 0;
    }

    /// Stop the acquisition hardware (timers + DMA). Leaves BUF and the stored
    /// record state alone.
    fn halt(&mut self) {
        unsafe {
            modify32(TIM1, 1, 0);
            modify32(TIM8, 1, 0);
            w32(S4, 0);
            w32(S3, 0);
            while r32(S4) & 1 != 0 || r32(S3) & 1 != 0 {}
        }
        self.hw.disarm();
        self.hw_mode = false;
        self.stream = false;
        self.busy = false;
        self.rolling = false;
    }

    /// User STOP: keep the frame the user is looking at, and always leave a
    /// stored record for Save CSV / the stopped zoom when samples exist.
    ///  * Roll: the roll display is the newest data in BUF, so linearise it.
    ///  * Nothing in flight: the stored record is the last frame poll() showed,
    ///    or the complete record it did not show (`finish`: no trigger in it;
    ///    the screen stays frozen as drawn, as below). Before this, a STOP
    ///    between such a record and the next start left no record (device,
    ///    100 us/div, a 1 kHz square: the 0.8 ms trigger window misses the
    ///    edge in about 1 record of 5; WAV:PRE? 0 points about 1 STOP in 8).
    ///  * A complete linear record in flight: treated exactly as poll() would
    ///    (trigger search and the Auto/Normal/Single show rule); a frame
    ///    poll() would have shown becomes the stored record and is drawn.
    ///  * Anything else in flight (a partial linear record, a record poll()
    ///    would have discarded, the circular hardware-trigger / stream ring)
    ///    has already overwritten the displayed frame in BUF, so the samples
    ///    captured so far (oldest first) become an untriggered stored record,
    ///    anchored on the newest screen. The screen stays frozen as drawn (as
    ///    the stock STOP does) until a zoom / pan re-renders the record
    ///    (returns false: no re-render for the STOP press itself). Before
    ///    this, STOP mid-acquisition left no record ("No record to save";
    ///    coordinator 2026-10-21): with Auto and no trigger event, nearly
    ///    every STOP lands mid-acquisition.
    fn freeze(&mut self, now: u64) -> bool {
        if self.wave {
            return self.wave_freeze();
        }
        let was_busy = self.busy;
        let was_rolling = self.rolling;
        let was_hw = self.hw_mode;
        let was_stream = self.stream;
        self.halt();
        if !was_busy {
            return self.stored_drawn;
        }
        // Read after the stop: the DMA was still running before it.
        let ndtr = unsafe { r32(S4 + 0x04) } as usize;
        self.stored_valid = false;
        let len = if was_rolling {
            self.snapshot_roll();
            return true;
        } else if was_stream {
            let wr = self.stream_wr();
            self.rec_base = wr - wr.min(CAP as u64);
            if wr > CAP as u64 {
                unsafe { (&mut (&mut *core::ptr::addr_of_mut!(BUF))[..]).rotate_left((wr % CAP as u64) as usize) };
            }
            wr.min(CAP as u64) as usize
        } else if was_hw {
            let wr = CAP - ndtr.min(CAP);
            // HISR TCIF4: the ring has wrapped at least once.
            let wrapped = unsafe { r32(DMA1 + 0x04) } & (1 << 5) != 0;
            self.hw_linearise(wr, wrapped)
        } else if ndtr == 0 {
            // Shown: drawn now; else kept untriggered (`finish`).
            return self.finish(now).is_some();
        } else {
            self.rec - ndtr.min(self.rec)
        };
        self.snapshot_mark(len, len.saturating_sub(self.screen / 2), None, false);
        false
    }

    /// Mask BUF[0..len] to 10 bits and mark it as the stored record, captured
    /// with the current settings (fs, range).
    fn snapshot_mark(&mut self, len: usize, anchor: usize, trig: Option<usize>, roll: bool) {
        if len < 2 {
            self.stored_valid = false;
            return;
        }
        let buf = unsafe { &mut (&mut *core::ptr::addr_of_mut!(BUF))[..len] };
        for v in buf.iter_mut() {
            *v &= 0x3FF;
        }
        self.stored_len = len;
        self.stored_fs = self.sample_rate();
        self.stored_tb = self.s.tb as u8;
        self.stored_range = self.s.range;
        self.stored_pos = self.s.pos;
        self.stored_roll = roll;
        self.stored_drawn = true;
        self.stored_anchor = anchor.min(len - 1);
        self.stored_trig = trig;
        self.stored_ab = (wv().sc.a, wv().sc.b);
        self.stored_gen = self.gsync.filter(|g| g.1 == crate::gen::epoch()).map(|(g, _)| {
            let mut g = g;
            g.t0 += self.rec_base as f64 * g.s;
            g.t0 = crate::gen::wrap(g.t0, g.q);
            g
        });
        self.stored_t0 = self.t_s0.map(|t| t.wrapping_add((self.rec_base as f64 * self.t_step as f64) as u64 as u32));
        self.stored_valid = true;
    }

    /// A complete linear record (NDTR = 0, hardware halted): trigger search and
    /// the stock show rule. Returns the start of the visible window and the
    /// trigger when the frame is to be shown, and makes it the stored record
    /// (anchored on the on-screen centre). A record not shown (Normal / Single
    /// without trigger, Auto before its timeout) has replaced the frame shown
    /// before in BUF: it stays the stored record, untriggered and anchored on
    /// its newest screen as a partial one in `freeze`, but not drawn
    /// (`stored_drawn`), so a STOP before the next start keeps it; the next
    /// start drops it.
    fn finish(&mut self, now: u64) -> Option<(usize, Option<usize>)> {
        let n = self.rec;
        // Pre-trigger samples: the trigger point lands on column hpos. Every
        // trigger position with a full screen around it is searched (the
        // stock 800-sample window is the refinement around its comparator
        // estimate, not a search range): start = trig - pre >= 0 and
        // start + screen <= pre + win + screen = n.
        let pre = self.screen * self.s.hpos.clamp(0, COLS as i32) as usize / COLS;
        let win = n - self.screen;
        // Mask once here (snapshot_mark), then search in the masked record.
        self.snapshot_mark(n, 0, None, false);
        let data: &[u16] = unsafe { &(&*core::ptr::addr_of!(BUF))[..n] };
        let trig = if self.gen_src() {
            self.gen_next(pre as u64).map(|t| t as usize).filter(|&t| t <= pre + win)
        } else if self.s.ttype != 0 {
            self.find_soft(data, pre, pre + win)
        } else {
            self.find_trigger(&data[pre..pre + win]).map(|i| i + pre)
        };
        let show = match (trig, self.s.mode) {
            (Some(_), _) => true,
            (None, TrigMode::Auto) => now >= self.deadline,
            (None, _) => false,
        };
        if !show {
            self.stored_anchor = n - self.screen / 2;
            self.stored_drawn = false;
            return None;
        }
        let start = trig.map(|t| t - pre).unwrap_or(win / 2);
        if self.stored_valid {
            self.stored_trig = trig;
            self.stored_anchor = (start + self.screen / 2).min(self.stored_len - 1);
        }
        Some((start, trig))
    }

    /// Roll record: linearise the circular buffer so the stored record runs
    /// oldest -> newest, anchored at the newest sample (the right edge the user
    /// was watching). Matches the stock 65520-sample roll record (§3), capped at
    /// CAP.
    fn snapshot_roll(&mut self) {
        let wr = (CAP - unsafe { r32(S4 + 0x04) } as usize % (CAP + 1)) % CAP;
        let d = (wr + CAP - self.roll_last_wr) % CAP;
        self.roll_written += d as u64;
        self.roll_last_wr = wr;
        let len = if self.roll_written >= CAP as u64 {
            unsafe { (&mut (&mut *core::ptr::addr_of_mut!(BUF))[..]).rotate_left(wr) };
            CAP
        } else {
            wr.min(CAP)
        };
        self.rec_base = self.roll_written - len as u64;
        self.snapshot_mark(len, len.saturating_sub(1), None, true);
    }

    fn start(&mut self) {
        let fs = TB[self.s.tb].0;
        let (_, psc, arr, ccr4) = *HW.iter().find(|h| h.0 == fs).unwrap_or(&HW[0]);
        let buf = core::ptr::addr_of!(BUF) as u32;
        unsafe {
            w32(LIFCR, 0x3D << 22);
            w32(HIFCR, 0x3D);
            w32(TIM8, 0);
            w32(TIM8 + 0x28, 0);
            w32(TIM8 + 0x2C, 3);
            w32(TIM8 + 0x34, 2);
            w32(TIM8 + 0x38, 0);
            w32(TIM8 + 0x18, 6 << 4);
            w32(TIM8 + 0x20, 1);
            w32(TIM8 + 0x44, 1 << 15);
            w32(TIM1, 0);
            w32(TIM1 + 0x28, psc);
            w32(TIM1 + 0x2C, arr);
            w32(TIM1 + 0x40, ccr4);
            w32(TIM1 + 0x1C, 6 << 12);
            w32(TIM1 + 0x20, 1 << 12);
            w32(TIM1 + 0x44, 1 << 15);
            w32(TIM1 + 0x0C, 1 << 12);
            w32(TIM1 + 0x14, 1);
            let src = if fs >= 50_000_000 {
                w32(TIM8 + 0x0C, 0);
                GPIOB_IDR
            } else {
                w32(TIM8 + 0x0C, 1 << 10);
                w32(S3 + 0x08, GPIOB_IDR);
                w32(S3 + 0x0C, core::ptr::addr_of!(LATCH) as u32);
                w32(S3 + 0x04, 2);
                w32(S3, (7 << 25) | (3 << 16) | (1 << 13) | (1 << 11) | (1 << 8) | 1);
                core::ptr::addr_of!(LATCH) as u32
            };
            w32(S4 + 0x08, src);
            w32(S4 + 0x0C, buf);
            // Roll from 200 ms/div (TB_ROLL) when Roll is on, except in Single,
            // which captures a full linear record like the stock Single mode.
            self.rolling = self.roll_ok();
            // BUF is about to be overwritten: the previous stored record is gone
            // until this acquisition completes (or is snapshotted in stop()).
            self.stored_valid = false;
            // Comparator trigger: circular record over CAP, frozen in hardware
            // `post` samples after the edge (hwtrig.rs). Roll never triggers.
            // Pulse / runt (software, trigsoft.rs): one complete linear
            // record per frame.
            // GEN: the trigger is computed (edge only); the GEN trace needs
            // every record's phase, which a comparator stop does not give.
            let gen = self.gen_src() || self.gen_trace;
            let soft = self.s.ttype != 0 && !self.gen_src();
            let hw = self.hw.available() && !self.rolling && !soft && !gen && !self.ch2_on;
            // Software trigger at low rates: circular record searched while
            // it is written (stream_poll).
            let stream = !self.rolling && !hw && !soft && fs <= STREAM_FS;
            let (len, circ) = if self.rolling || hw || stream { (CAP as u32, 1 << 8) } else { (self.rec as u32, 0) };
            self.roll_written = 0;
            self.roll_last_wr = 0;
            self.stream = stream;
            self.st_scan = 0;
            self.st_idx = 0;
            self.st_armed = false;
            self.st_found = None;
            w32(S4 + 0x04, len);
            w32(S4, (6 << 25) | (3 << 16) | (1 << 13) | (1 << 11) | (1 << 10) | circ | 1);
            if hw {
                let (pre, _) = self.hw_split();
                let code = self.trig_code() as f32;
                self.hw.prepare(psc, arr, hwtrig::post_for(self.screen - pre, WIN), code, self.s.rising);
            }
            self.hw_mode = hw;
            self.rec_base = 0;
            w32(TIM8, 1);
            // The generator's time at the TIM1 start: sample n is taken at
            // CC4 of TIM1 period n, (ccr4 + n (arr + 1)) (psc + 1) ticks
            // later (both timers on the 200 MHz clock). An interrupt
            // between the reading and the start: no phase for this record.
            let g = if gen && !self.wave { crate::gen::now() } else { None };
            w32(TIM1, 1);
            let d_en = r32(DWT_CYCCNT);
            self.t_step = ((psc + 1) * (arr + 1)) as f32;
            self.t_s0 = (!hw).then(|| d_en.wrapping_add(ccr4 * (psc + 1)));
            self.gsync = g.and_then(|(gt, d)| {
                let dt = d_en.wrapping_sub(d);
                let (t, shape) = crate::gen::running()?;
                let (q, k) = crate::gen::loop_ticks(&t);
                let ps = psc as f64 + 1.0;
                let t0 = gt as f64 + dt as f64 + ccr4 as f64 * ps - gen_lag() as f64;
                let sy = crate::gen::Sync { t0: crate::gen::wrap(t0, q as f64), s: ps * (arr as f64 + 1.0), q: q as f64, k: k as f64, shape };
                (dt < 400).then_some((sy, crate::gen::epoch()))
            });
        }
        self.busy = true;
    }

    /// DWT count of the stored record's BUF[0] and DWT cycles per sample
    /// (the DMM CH2's alignment); None without a stored record, or for a
    /// comparator record (its stop instant is not known) or the wave view.
    pub fn rec_time(&self) -> Option<(u32, f32)> {
        if self.stored_valid && !self.wave { self.stored_t0.map(|t| (t, self.t_step)) } else { None }
    }

    /// Roll (live): DWT count at the end of the newest column and DWT
    /// cycles per column of the last roll frame.
    pub fn roll_time(&self) -> Option<(u32, f32)> {
        if !self.rolling || self.wave {
            return None;
        }
        let t = self.t_s0?;
        let per = (self.screen / COLS).max(1) as f32;
        Some((t.wrapping_add((self.roll_written as f64 * self.t_step as f64) as u64 as u32), per * self.t_step))
    }

    /// A stream record without the generator's phase (the reading at its
    /// start failed, or the generator was restarted since: a new epoch):
    /// read it now against the record's sample 0 (`t_s0`, the DWT of the
    /// TIM1 start + CC4), so a long Normal wait does not stay blind. `wr`
    /// (samples written) unwraps the 21 s DWT wrap. The samples before a
    /// generator restart get the new phase too (only later ones are
    /// searched; the GEN trace of that record is off by the restart).
    fn gen_resync(&mut self, wr: u64) {
        let Some(s0) = self.t_s0 else { return };
        let Some((t, shape)) = crate::gen::running() else { return };
        let ep = crate::gen::epoch();
        let Some((g, d)) = crate::gen::now() else { return };
        let (q, k) = crate::gen::loop_ticks(&t);
        let s = self.t_step as f64;
        let span = 4_294_967_296.0;
        let e0 = d.wrapping_sub(s0) as f64;
        let e = e0 + span * round_f64((wr as f64 * s - e0) / span);
        let t0 = g as f64 - e - gen_lag() as f64;
        let sy = crate::gen::Sync { t0: crate::gen::wrap(t0, q as f64), s, q: q as f64, k: k as f64, shape };
        self.gsync = Some((sy, ep));
    }

    /// The trigger source is the generator (MCX records only).
    fn gen_src(&self) -> bool {
        self.s.tsrc == TSRC_GEN && !self.wave
    }

    /// The generator against the record in flight, when still valid.
    fn gen_live(&self) -> Option<crate::gen::Sync> {
        self.gsync.filter(|g| g.1 == crate::gen::epoch()).map(|g| g.0)
    }

    /// GEN trigger: the first sample index (absolute, of the acquisition
    /// in flight) >= `n0` at the generator's crossing of the trigger level
    /// in the edge's direction. None: no phase, or the level is outside the
    /// generator's swing.
    fn gen_next(&self, n0: u64) -> Option<u64> {
        let g = self.gen_live()?;
        let ph = g.shape.crossing(self.trig_volts(), self.s.rising)?;
        Some(g.next(n0, ph))
    }

    /// The generator against the stored record (BUF[0] = sample 0), for
    /// the GEN trace; None without one.
    pub fn gen_sync(&self) -> Option<crate::gen::Sync> {
        if self.stored_valid { self.stored_gen } else { None }
    }

    /// GEN trigger, linear record (fs > STREAM_FS): its trigger window
    /// (CAP - screen samples) can be shorter than a generator period, so
    /// the record starts at the generator's phase that puts a crossing in
    /// the window's middle: true = start now (after at most `GEN_SPIN` of
    /// busy-wait), false = not this pass. Auto past its timeout, no
    /// generator, a level outside its swing: start at once (untriggered).
    fn gen_ready(&mut self, now: u64) -> bool {
        let fs = TB[self.s.tb].0;
        if !self.gen_src() || fs <= STREAM_FS || self.roll_ok() {
            return true;
        }
        if self.deadline == 0 {
            self.deadline = now + self.auto_timeout();
        }
        if self.s.mode == TrigMode::Auto && now >= self.deadline {
            return true;
        }
        let Some((t, shape)) = crate::gen::running() else { return true };
        let Some(ph) = shape.crossing(self.trig_volts(), self.s.rising) else { return true };
        let (q, k) = crate::gen::loop_ticks(&t);
        let period = q as f64 / k as f64;
        let (_, psc, arr, ccr4) = *HW.iter().find(|h| h.0 == fs).unwrap_or(&HW[0]);
        let ps = psc as f64 + 1.0;
        let s = ps * (arr as f64 + 1.0);
        let (pre, _) = self.hw_split();
        let win = (self.rec - self.screen) as f64;
        if period <= 0.8 * win * s {
            return true;
        }
        let Some((g, d)) = crate::gen::now() else { return false };
        let x = (g as f64 - gen_lag() as f64) * k as f64 / q as f64;
        let to_next = crate::gen::wrap(ph as f64 - x, 1.0) * period;
        let lead = (pre as f64 + win / 2.0) * s + ccr4 as f64 * ps;
        let wait = crate::gen::wrap(to_next - lead, period);
        if wait > GEN_SPIN as f64 {
            return false;
        }
        let wait = wait as u32;
        while unsafe { r32(DWT_CYCCNT) }.wrapping_sub(d) < wait {}
        true
    }

    /// Screen samples left / right of the trigger point (hpos column).
    fn hw_split(&self) -> (usize, usize) {
        let pre = self.screen * self.s.hpos.clamp(0, COLS as i32) as usize / COLS;
        (pre, self.screen - pre)
    }

    /// Linearise the circular record after a hardware stop: with the ring
    /// wrapped (TCIF4) every sample is valid, rotate so it runs oldest ->
    /// newest; otherwise samples 0..wr are the record. Returns its length.
    fn hw_linearise(&mut self, wr: usize, wrapped: bool) -> usize {
        if wrapped {
            unsafe { (&mut (&mut *core::ptr::addr_of_mut!(BUF))[..]).rotate_left(wr) };
            CAP
        } else {
            wr
        }
    }

    /// Hardware-trigger acquisition in flight: arm after the pre-trigger part,
    /// then wait for TIM2 to gate TIM1 off (Done) or for the Auto timeout.
    fn hw_poll(&mut self, now: u64) -> Option<Frame<'static>> {
        let wr = CAP - (unsafe { r32(S4 + 0x04) } as usize).min(CAP);
        // HISR TCIF4: the ring has wrapped at least once.
        let wrapped = unsafe { r32(DMA1 + 0x04) } & (1 << 5) != 0;
        let st = self.hw.status();
        // Health: nothing written 100 ms after the start means the TIM1 gate
        // is stuck: give up on the comparator for good (a hardware fault, not
        // a signal condition; DBG:HWTRIG:ENABLE 1 retries), the software
        // path restarts.
        if wr == 0 && !wrapped && now >= self.hw_t0 + 100 {
            self.halt();
            self.hw.set_enabled(false);
            return None;
        }
        let (pre, post) = self.hw_split();
        match st {
            Status::Off => None,
            Status::Prepared => {
                if wrapped || wr >= hwtrig::pre_for(pre, WIN) {
                    self.hw.arm();
                }
                None
            }
            Status::Armed | Status::Fired => {
                // Timeout without an edge (Auto: the stock timeout; Normal /
                // Single: the same period, as a comparator health check): stop
                // and look for a crossing in software. One found while the
                // comparator stayed quiet is a miss (no comparator on this
                // board, or the level map is off); three in a row hand the
                // scope back to the software trigger (hwtrig::note_miss).
                if st != Status::Armed || now < self.deadline || !(wrapped || wr >= self.screen) {
                    return None;
                }
                self.halt();
                // Re-read after the stop: the DMA was still running above.
                let wr = CAP - (unsafe { r32(S4 + 0x04) } as usize).min(CAP);
                let wrapped = unsafe { r32(DMA1 + 0x04) } & (1 << 5) != 0;
                let len = self.hw_linearise(wr, wrapped);
                if len < self.screen {
                    self.stored_valid = false;
                    return None;
                }
                self.snapshot_mark(len, len - self.screen / 2, None, false);
                let data: &[u16] = unsafe { &(&*core::ptr::addr_of!(BUF))[..len] };
                let found = if len - post > pre { self.find_trigger(&data[pre..len - post]).map(|i| i + pre) } else { None };
                if found.is_some() {
                    self.hw.note_miss();
                }
                let start = match (found, self.s.mode) {
                    (Some(t), _) => t - pre,
                    // Auto: the newest screen, untriggered (stock TIM5 UIF
                    // "record done without trigger").
                    (None, TrigMode::Auto) => len - self.screen,
                    (None, _) => {
                        self.stored_valid = false;
                        return None;
                    }
                };
                self.stored_trig = found;
                self.stored_anchor = start + pre;
                self.deadline = 0;
                if found.is_some() && self.s.mode == TrigMode::Single {
                    self.s.run = false;
                }
                let data: &'static [u16] = unsafe { &(&*core::ptr::addr_of!(BUF))[start..start + self.screen] };
                self.seq = self.seq.wrapping_add(1);
                Some(Frame { data, cols: None, trig: found, fs: self.sample_rate(), seq: self.seq })
            }
            Status::Done => {
                self.halt();
                let len = self.hw_linearise(wr, wrapped);
                let post_n = hwtrig::post_for(post, WIN) as usize;
                if len < self.screen + WIN || len < post_n + pre {
                    // Cannot happen after `arm` (pre + post samples written),
                    // kept as a guard against a stale TCIF.
                    self.stored_valid = false;
                    return None;
                }
                // Hardware estimate: the edge is post_n samples before the
                // newest sample; the stock software search refines it.
                let th = len - post_n;
                let lo = th.saturating_sub(WIN / 2).max(pre);
                let hi = (th + WIN / 2).min(len - post);
                self.snapshot_mark(len, th, None, false);
                let data: &[u16] = unsafe { &(&*core::ptr::addr_of!(BUF))[..len] };
                // The crossing nearest the estimate: at slow rates the window
                // spans several periods, and the first crossing in it was a
                // period early (device: delta -250 samples at 100 ms/div).
                let found = if lo < hi { self.find_trigger_near(&data[lo..hi], th - lo).map(|i| i + lo) } else { None };
                let t = match found {
                    Some(t) => {
                        self.hw.note_hit(t as i32 - th as i32);
                        t
                    }
                    None => {
                        // Fired, but no crossing near the estimate: the map
                        // is off. Auto shows the frame at the estimate;
                        // Normal / Single wait for the next one.
                        self.hw.note_miss();
                        if self.s.mode != TrigMode::Auto {
                            self.stored_valid = false;
                            return None;
                        }
                        th.clamp(pre, len - post)
                    }
                };
                self.stored_trig = found;
                self.stored_anchor = t;
                self.deadline = 0;
                if found.is_some() && self.s.mode == TrigMode::Single {
                    self.s.run = false;
                }
                let start = t - pre;
                let data: &'static [u16] = unsafe { &(&*core::ptr::addr_of!(BUF))[start..start + self.screen] };
                self.seq = self.seq.wrapping_add(1);
                Some(Frame { data, cols: None, trig: found, fs: self.sample_rate(), seq: self.seq })
            }
        }
    }

    /// Wire-free comparator calibration (hwtrig::calibrate) at range 6 / DC
    /// with the scope's own offset points; the current settings are restored
    /// (acquisition restarts from poll). Needs PC4 on and nothing on the MCX.
    #[inline(never)] // .cold (link-native.x): SCPI calibration
    pub fn hw_calibrate(&mut self, now: u64, clk: &mut crate::clock::Clock) -> Result<hwtrig::CalResult, hwtrig::CalError> {
        if !self.powered {
            return Err(hwtrig::CalError::NoComparator);
        }
        let saved = self.s;
        let mut s = saved;
        s.range = 6;
        s.ac = false;
        self.apply(s, now);
        // Relay settle (PC13 changes when coming from r0..4) + rail.
        clk.delay_ms(250);
        let r = self.hw.calibrate(6, None);
        self.apply(saved, now);
        r
    }

    pub fn sample_rate(&self) -> u32 {
        if self.wave { wave_real_fs(wv().fs) } else { TB[self.s.tb].0 }
    }

    /// Zoom anchor of the stored record (sample index at the screen centre),
    /// None while running or without a stored record. Taken at touch-down so
    /// a pan is relative to it (`pan_from`).
    pub fn anchor(&self) -> Option<usize> {
        if self.s.run || !self.stored_valid || self.stored_len < 2 {
            return None;
        }
        Some(if self.stored_roll {
            // Roll record: newest sample at the right edge.
            (self.stored_len as u64).saturating_sub(self.zoom_screen(self.s.tb) / 2) as usize
        } else {
            self.stored_anchor
        })
    }

    /// Stopped: show the stored record panned by `cols` screen columns from
    /// anchor `a0` (positive = the trace follows the finger to the right, so
    /// earlier samples come into view); re-rendered on the next poll.
    pub fn pan_from(&mut self, a0: usize, cols: i32) {
        if self.anchor().is_none() {
            return;
        }
        let per = self.zoom_screen(self.s.tb) as i64;
        let a = a0 as i64 - cols as i64 * per / COLS as i64;
        self.stored_anchor = a.clamp(0, self.stored_len as i64 - 1) as usize;
        self.stored_roll = false;
        self.zoom_pending = true;
    }

    /// The stored record (10-bit codes, oldest first), empty without one.
    /// Every frame `poll` returns is a window of it (or, zoomed out past
    /// it, the whole of it), valid until the next acquisition starts (the
    /// next `poll` while running).
    pub fn record(&self) -> &'static [u16] {
        if !self.stored_valid || self.stored_len < 2 {
            return &[];
        }
        unsafe { &(&*core::ptr::addr_of!(BUF))[..self.stored_len] }
    }

    /// Stopped: centre the stored record on its trigger point (UF-09
    /// double tap); re-rendered on the next poll.
    pub fn centre_stored(&mut self) {
        if !self.s.run && self.stored_valid {
            if let Some(t) = self.stored_trig {
                self.stored_anchor = t.min(self.stored_len.saturating_sub(1));
                self.stored_roll = false;
                self.zoom_pending = true;
            }
        }
    }

    /// The stored record's sample rate, time-zero index (the trigger point,
    /// else the sample at the screen centre) and whether it triggered.
    pub fn record_info(&self) -> (u32, usize, bool) {
        let t0 = self.stored_trig.unwrap_or(if self.stored_roll { self.stored_len.saturating_sub(1) } else { self.stored_anchor });
        (self.stored_fs, t0, self.stored_trig.is_some())
    }

    /// The timebase (TB index) the stored record was captured at.
    pub fn record_tb(&self) -> usize {
        self.stored_tb as usize
    }

    /// Drag preview: take the position settings (CH1 position, trigger level
    /// row, trigger position) into `s` without touching the hardware or the
    /// acquisition in flight (same clamps as `apply`); the caller applies them
    /// once when the drag ends (../docs/history/scope-lag-fixes.md fix A). Labels and
    /// markers that read `s` follow at once.
    pub fn preview(&mut self, s: Settings) {
        let mut n = self.s;
        n.trig_row = s.trig_row.clamp(-10, 290);
        n.runt_row = s.runt_row.clamp(-10, 290);
        n.pos = s.pos.clamp(-392, 392);
        n.hpos = s.hpos.clamp(0, COLS as i32);
        self.sync_levels(&mut n);
        self.s = n;
    }

    /// The stored record must stay as it is (scope CSV being written, a
    /// host reading it): RUN does not restart the acquisition.
    pub fn held(&self) -> bool {
        self.hold || self.host_read
    }

    /// Roll mode applies to the next acquisition: Roll on, a roll timebase,
    /// and no single record wanted (Single sweep, DIGitize), which captures
    /// a full linear record like the stock Single mode.
    fn roll_ok(&self) -> bool {
        self.roll && self.s.tb >= TB_ROLL && self.s.mode != TrigMode::Single && !self.one_shot
    }

    /// Roll display active (running, timebase in the roll range).
    pub fn rolling(&self) -> bool {
        self.s.run && self.roll_ok()
    }

    /// Record bar (stock 2.3, FUN_0802d1c6) while stopped: the stored
    /// record's extent in screen widths relative to the shown window (0..1 =
    /// the screen: start = -(window start) / window, end = (record length -
    /// window start) / window; "[" drawn when 0 <= start < 1, "]" when end <=
    /// 1); (0, 1) while running or without a record.
    pub fn record_frac(&self) -> (f32, f32) {
        if self.s.run || !self.stored_valid || self.stored_len < 2 {
            return (0.0, 1.0);
        }
        let len = self.stored_len as f32;
        let want = self.zoom_screen(self.s.tb) as f32;
        let w0 = if self.stored_roll {
            len - want
        } else if want <= len {
            (self.stored_anchor as f32 - (want / 2.0) as u32 as f32).clamp(0.0, len - want)
        } else {
            self.stored_anchor as f32 - (want / 2.0) as u32 as f32
        };
        (-w0 / want, (len - w0) / want)
    }

    /// Ask for one re-render of the stored record while stopped (a scope view
    /// was (re)drawn, or its trace cache was invalidated). No-op while running.
    pub fn refresh(&mut self) {
        if !self.s.run && self.stored_valid {
            self.zoom_pending = true;
        }
    }

    /// Poll: start / finish frames. Returns the visible window when ready.
    /// Tracks the run-state chip: a live frame sets T'D / AUTO (RUN in
    /// roll); Normal / Single fall back to WAIT when nothing triggered for a
    /// timeout plus a screen (stock Normal expiry: status WAIT).
    pub fn poll(&mut self, now: u64) -> Option<Frame<'static>> {
        self.poll_src(now, None)
    }

    /// `poll` with the wave source (source DMM: the record comes from
    /// `src`; None there = nothing new).
    pub fn poll_src(&mut self, now: u64, src: Option<&mut dyn WaveSource>) -> Option<Frame<'static>> {
        let live = self.s.run;
        let f = self.poll_frame(now, src);
        // DIGitize: the first record frame ends the run (as Single does on
        // its trigger; the frame is the stored record, the DMA is idle).
        if live && self.one_shot && f.as_ref().is_some_and(|fr| fr.cols.is_none() && !fr.data.is_empty()) {
            self.one_shot = false;
            self.s.run = false;
        }
        if live {
            match &f {
                Some(fr) => {
                    self.last_frame = now;
                    self.set_status(if fr.cols.is_some() { 3 } else if fr.trig.is_some() { 2 } else { 1 });
                }
                None => {
                    if self.status == 2 && self.s.mode != TrigMode::Auto {
                        let screen_ms = self.screen as u64 * 1000 / self.sample_rate().max(1) as u64;
                        if now >= self.last_frame + self.auto_timeout() + screen_ms {
                            self.set_status(0);
                        }
                    }
                }
            }
        }
        f
    }

    fn poll_frame(&mut self, now: u64, src: Option<&mut dyn WaveSource>) -> Option<Frame<'static>> {
        if self.lent {
            return None;
        }
        if !self.s.run {
            // Stopped: re-render the stored record (stock STOP "keeps redrawing
            // the stored record", §4) once per change (zoom_pending: a settings
            // change, or `refresh` from a view redraw). Nothing is recomputed
            // while nothing changes, whatever view is shown.
            if self.stored_valid && self.zoom_pending {
                self.zoom_pending = false;
                return self.zoom_frame();
            }
            return None;
        }
        if self.wave {
            return src.and_then(|s| self.wave_frame(now, s));
        }
        if !self.powered || now < self.relay_until {
            return None;
        }
        if !self.busy {
            if !self.gen_ready(now) {
                return None;
            }
            self.start();
            self.hw_t0 = now;
            self.st_poll = now;
            if self.deadline == 0 {
                // Auto: untriggered frame after the stock timeout.
                self.deadline = now + self.auto_timeout();
            }
            return None;
        }
        if self.rolling {
            return self.roll_poll(now);
        }
        if self.hw_mode {
            return self.hw_poll(now);
        }
        if self.stream {
            return self.stream_poll(now);
        }
        if unsafe { r32(S4 + 0x04) } != 0 {
            return None;
        }
        self.halt();
        // The shown frame becomes the stored record (trigger + on-screen centre
        // remembered so a later zoom stays on this feature).
        let (start, trig) = self.finish(now)?;
        self.deadline = 0;
        if trig.is_some() && self.s.mode == TrigMode::Single {
            self.s.run = false;
        }
        let data: &'static [u16] = unsafe { &(&*core::ptr::addr_of!(BUF))[..self.rec] };
        self.seq = self.seq.wrapping_add(1);
        Some(Frame { data: &data[start..start + self.screen], cols: None, trig, fs: self.sample_rate(), seq: self.seq })
    }

    /// Absolute number of samples written since start() into the circular
    /// record (as roll_poll: the ring position advanced since the last call).
    fn stream_wr(&mut self) -> u64 {
        let wr = (CAP - unsafe { r32(S4 + 0x04) } as usize % (CAP + 1)) % CAP;
        let d = (wr + CAP - self.roll_last_wr) % CAP;
        self.roll_written += d as u64;
        self.roll_last_wr = wr;
        self.roll_written
    }

    /// Continuous software trigger (fs <= STREAM_FS; the stock's comparator
    /// waits for the edge, this scans every sample as it arrives): the
    /// find_trigger rule with its hysteresis state carried across polls, over
    /// the samples whose pre-trigger part is still in the ring; once a
    /// crossing is found, wait (spin at most STREAM_SPIN_US) until the
    /// post-trigger part is written, stop, linearise. Auto: the newest
    /// screen, untriggered, after the timeout.
    fn stream_poll(&mut self, now: u64) -> Option<Frame<'static>> {
        let fs = self.sample_rate() as u64;
        // A gap between two polls longer than the ring: the modulo count in
        // stream_wr cannot see it, and the samples are gone anyway. Restart
        // the search at the write position.
        let lost = now.saturating_sub(self.st_poll) * fs / 1000 + 64 >= CAP as u64;
        self.st_poll = now;
        let wr = self.stream_wr();
        if lost {
            self.st_found = None;
            self.st_armed = false;
            self.st_scan = wr;
            self.st_idx = (wr % CAP as u64) as usize;
        }
        let (pre, post) = self.hw_split();
        if self.st_found.is_none() {
            // Oldest crossing whose pre-trigger part survives until the stop
            // (64 samples of margin for the stop itself).
            let floor = (wr + pre as u64 + 64).saturating_sub(CAP as u64).max(pre as u64);
            if self.st_scan < floor {
                self.st_scan = floor;
                self.st_idx = (floor % CAP as u64) as usize;
                self.st_armed = false;
            }
            if self.gen_src() {
                // Computed, not searched: the next crossing at or after the
                // scan position, once it has been sampled.
                // (The scan below then starts at `wr`: nothing to scan.)
                if self.gen_live().is_none() {
                    self.gen_resync(wr);
                }
                if let Some(t) = self.gen_next(self.st_scan).filter(|&t| t < wr) {
                    self.st_found = Some(t);
                }
            }
            let l = self.trig_code();
            let rising = self.s.rising;
            let buf = unsafe { &*core::ptr::addr_of!(BUF) };
            let (mut i, mut armed, mut n) = (self.st_idx, self.st_armed, if self.gen_src() { wr } else { self.st_scan });
            while n < wr {
                let v = (buf[i] & 0x3FF) as i32;
                let hit = if rising {
                    if v < l - 2 {
                        armed = true;
                        false
                    } else {
                        armed && v > l
                    }
                } else if v > l + 2 {
                    armed = true;
                    false
                } else {
                    armed && v < l
                };
                if hit {
                    self.st_found = Some(n);
                }
                n += 1;
                i += 1;
                if i == CAP {
                    i = 0;
                }
                if hit {
                    break;
                }
            }
            (self.st_idx, self.st_armed, self.st_scan) = (i, armed, n);
        }
        let Some(t) = self.st_found else {
            if self.s.mode == TrigMode::Auto && now >= self.deadline && wr >= self.screen as u64 {
                return self.stream_emit(None);
            }
            return None;
        };
        let need = t + post as u64;
        if wr < need {
            if (need - wr) * 1_000_000 / fs > STREAM_SPIN_US {
                return None;
            }
            // Bounded (a stalled DMA must not hang the loop): ~10 cycles per
            // pass, 200 MHz -> far beyond STREAM_SPIN_US.
            let mut k = 0u32;
            while self.stream_wr() < need {
                k += 1;
                if k > 400_000 {
                    return None;
                }
            }
        }
        self.stream_emit(Some(t))
    }

    /// Stop the stream, linearise the ring (oldest sample first) and emit the
    /// screen around absolute crossing `t` (None: the newest screen).
    fn stream_emit(&mut self, t: Option<u64>) -> Option<Frame<'static>> {
        self.halt();
        let wr = self.stream_wr();
        let len = wr.min(CAP as u64) as usize;
        if len < self.screen {
            self.stored_valid = false;
            return None;
        }
        if wr > CAP as u64 {
            unsafe { (&mut (&mut *core::ptr::addr_of_mut!(BUF))[..]).rotate_left((wr % CAP as u64) as usize) };
        }
        let base = wr - len as u64;
        self.rec_base = base;
        let (pre, post) = self.hw_split();
        let start = match t {
            Some(t) if t >= base + pre as u64 && t + post as u64 <= wr => (t - base) as usize - pre,
            // Pre-trigger part overwritten (the stop came too late): restart.
            Some(_) => {
                self.stored_valid = false;
                return None;
            }
            None => len - self.screen,
        };
        let trig = t.map(|_| start + pre);
        self.snapshot_mark(len, start + self.screen / 2, trig, false);
        self.deadline = 0;
        if trig.is_some() && self.s.mode == TrigMode::Single {
            self.s.run = false;
        }
        let data: &'static [u16] = unsafe { &(&*core::ptr::addr_of!(BUF))[start..start + self.screen] };
        self.seq = self.seq.wrapping_add(1);
        Some(Frame { data, cols: None, trig, fs: self.sample_rate(), seq: self.seq })
    }

    /// Roll: every 100 ms, min/max of 400 columns over the latest screen.
    fn roll_poll(&mut self, now: u64) -> Option<Frame<'static>> {
        if now < self.roll_next {
            return None;
        }
        self.roll_next = now + 100;
        let wr = CAP - unsafe { r32(S4 + 0x04) } as usize % (CAP + 1);
        let wr = wr % CAP;
        let d = (wr + CAP - self.roll_last_wr) % CAP;
        self.roll_written += d as u64;
        self.roll_last_wr = wr;
        let per = (self.screen / COLS).max(1);
        let buf = unsafe { &*core::ptr::addr_of!(BUF) };
        let cols = unsafe { &mut *core::ptr::addr_of_mut!(ROLL_COLS) };
        for i in 0..COLS {
            // Column i ends (COLS - 1 - i) columns before the write position.
            let back = (COLS - i) * per;
            if back as u64 > self.roll_written {
                cols[i] = (u16::MAX, u16::MAX);
                continue;
            }
            let start = (wr + CAP - back) % CAP;
            let (mut lo, mut hi) = (u16::MAX, 0u16);
            for k in 0..per {
                let v = buf[(start + k) % CAP] & 0x3FF;
                lo = lo.min(v);
                hi = hi.max(v);
            }
            cols[i] = (lo, hi);
        }
        self.seq = self.seq.wrapping_add(1);
        Some(Frame { data: &[], cols: Some(unsafe { &*core::ptr::addr_of!(ROLL_COLS) }), trig: None, fs: self.sample_rate(), seq: self.seq })
    }

    /// Samples of the stored record that fill the 400 px screen at timebase
    /// `tb`, using the sample rate the record was captured at. Equivalent to the
    /// stock stored-record re-render (§3): the s/div changes while fs is fixed,
    /// so the window = screen(tb) * stored_fs / fs(tb) (integer, no f32).
    /// Saturates at u32::MAX samples (zooming a 50 MS/s record out to 50 s/div
    /// asks for ~2.5e10); zoom_frame works in u64 with any value.
    fn zoom_screen(&self, tb: usize) -> u64 {
        let (fs_tb, num, den, ppp) = TB[tb];
        let screen = (num * COLS as u32 / den / ppp).max(2) as u64;
        (screen * self.stored_fs as u64 / fs_tb.max(1) as u64).clamp(2, u32::MAX as u64)
    }

    /// Re-render a window of the stored record at the current timebase, anchored
    /// on the feature that was on screen when acquisition stopped (a roll record
    /// on its newest sample at the right edge, as it was displayed).
    fn zoom_frame(&mut self) -> Option<Frame<'static>> {
        if !self.stored_valid || self.stored_len < 2 || self.stored_fs == 0 {
            return None;
        }
        let len = self.stored_len;
        let want = self.zoom_screen(self.s.tb);
        self.seq = self.seq.wrapping_add(1);
        if want <= len as u64 {
            let want = want as usize;
            let start = if self.stored_roll {
                len - want
            } else {
                self.stored_anchor.saturating_sub(want / 2).min(len - want)
            };
            let data: &'static [u16] = unsafe { &(&*core::ptr::addr_of!(BUF))[start..start + want] };
            let trig = self.stored_trig.and_then(|t| t.checked_sub(start).filter(|_| t < start + want));
            return Some(Frame { data, cols: None, trig, fs: self.stored_fs, seq: self.seq });
        }
        // The screen spans more time than was captured: draw the record at its
        // true scale as per-column min/max (the roll-frame format) with empty
        // columns where there is no data, instead of stretching it to 400 px.
        // f32 column edges: exact up to 2^24 samples, and beyond that a column
        // spans >= 40 K samples (> the whole record), so rounding is invisible.
        let step = want as f32 / COLS as f32;
        let w0 = if self.stored_roll { len as f32 - want as f32 } else { self.stored_anchor as f32 - (want / 2) as f32 };
        let buf = unsafe { &*core::ptr::addr_of!(BUF) };
        let cols = unsafe { &mut *core::ptr::addr_of_mut!(ROLL_COLS) };
        for (i, c) in cols.iter_mut().enumerate() {
            let af = w0 + i as f32 * step;
            let bf = af + step;
            *c = (u16::MAX, u16::MAX);
            if bf <= 0.0 || af >= len as f32 {
                continue; // no data in this column
            }
            // A negative left edge saturates to 0 in the cast.
            let a = af as usize;
            let b = (bf as usize).max(a + 1).min(len);
            let (mut lo, mut hi) = (u16::MAX, 0u16);
            for &v in &buf[a..b] {
                lo = lo.min(v);
                hi = hi.max(v);
            }
            *c = (lo, hi);
        }
        Some(Frame { data: &[], cols: Some(unsafe { &*core::ptr::addr_of!(ROLL_COLS) }), trig: None, fs: self.stored_fs, seq: self.seq })
    }

    /// As find_trigger, but the crossing closest to index `near` (the
    /// comparator's estimate) instead of the first one.
    fn find_trigger_near(&self, d: &[u16], near: usize) -> Option<usize> {
        let mut best: Option<usize> = None;
        let mut from = 0;
        while from < d.len() {
            let Some(i) = self.find_trigger(&d[from..]).map(|i| i + from) else { break };
            if best.is_some_and(|b| b.abs_diff(near) <= i.abs_diff(near)) {
                break; // crossings only get farther from here on
            }
            best = Some(i);
            from = i + 1;
        }
        best
    }

    /// Stock software trigger: beyond the hysteresis band (code -+ 2) on the
    /// far side, then the first sample across the level.
    fn find_trigger(&self, d: &[u16]) -> Option<usize> {
        let l = self.trig_code();
        let mut armed = false;
        for (i, &v) in d.iter().enumerate() {
            let v = v as i32;
            if self.s.rising {
                if v < l - 2 {
                    armed = true;
                } else if armed && v > l {
                    return Some(i);
                }
            } else if v > l + 2 {
                armed = true;
            } else if armed && v < l {
                return Some(i);
            }
        }
        None
    }
}

// ------------------------------------------------------- source DMM (wave)
//
// The wave view (docs/dmm-input-scope-feasibility.md 5.2, TODO (A)): the
// record is the HY3131 AD2 stream of the DMM's AC row, drained every poll
// from the logger ring into BUF as a continuous circular record (10-bit
// codes, trigsoft::ad2_code10), so the pre-trigger history is always there
// and nothing is lost between frames (the ring holds 2048 samples, 213 ms
// at 9600 S/s, against main-loop passes of <= 25 ms). The trigger is
// detected per sample in the stream ISR (trigsoft::Stream, the same edge /
// pulse / runt rules as the MCX software search) and reported as sample
// indices: no dead time. Frames are linear windows of BUF (rotated so the
// oldest sample is at 0), so the renderer, the measurements, the stored
// record (STOP, zoom, CSV) and roll work as for the MCX record.
impl Scope {
    /// Source DMM on (the MCX acquisition stops; its settings are kept for
    /// `wave_exit`); the wave view's own settings come back.
    pub fn wave_enter(&mut self, sc: WaveScale, now: u64) {
        if self.wave {
            self.wave_scale(sc);
            return;
        }
        self.halt();
        self.levels_reset(); // the wave view's own rows, at its scale
        let ws = wv().s;
        *wv() = Wave::NEW;
        wv().s = ws;
        wv().mcx = self.s;
        self.stored_valid = false;
        self.wave = true;
        wv().sc = sc;
        wv().fs = 0; // apply: restart at the timebase's rate
        let mut s = ws;
        s.run = true;
        s.range = wv().mcx.range;
        self.apply(s, now);
    }

    /// Back to the MCX source with its settings (the stored wave record is
    /// dropped: BUF belongs to the MCX acquisition again).
    pub fn wave_exit(&mut self, now: u64) {
        if !self.wave {
            return;
        }
        self.halt();
        wv().s = self.s;
        self.wave = false;
        self.levels_reset(); // the MCX rows, at the MCX scale
        self.stored_valid = false;
        self.hold = false;
        let m = wv().mcx;
        self.apply(m, now);
    }

    pub fn wave_on(&self) -> bool {
        self.wave
    }

    /// Lend BUF to the inrush capture (src/app_inrush.rs): the MCX
    /// acquisition stops (its DMA writes BUF) and stays idle, the stored
    /// record is dropped, until `return_buf`. Not with the wave view on.
    pub fn lend_buf(&mut self) {
        self.halt();
        self.stored_valid = false;
        self.lent = true;
    }

    /// BUF while lent (the inrush ring / trace), else None.
    #[inline(always)]
    pub fn lent_buf(&self) -> Option<&'static mut [u16]> {
        // SAFETY: main loop only; while lent nothing else in scope.rs
        // touches BUF (poll_frame returns at once, no DMA runs).
        self.lent.then(|| unsafe { &mut (&mut *core::ptr::addr_of_mut!(BUF))[..INRUSH_LEN] })
    }

    /// BUF back to the scope: the next poll starts an acquisition.
    pub fn return_buf(&mut self) {
        if self.lent {
            self.lent = false;
            self.busy = false;
        }
    }

    /// The settings to persist: the MCX ones while the wave view runs.
    pub fn mcx_settings(&self) -> Settings {
        if self.wave { wv().mcx } else { self.s }
    }

    pub fn wave_scale_now(&self) -> WaveScale {
        wv().sc
    }

    /// New DMM range / function scale: the record restarts (the app
    /// restarts the stream on the new row, then `wave_live`).
    pub fn wave_scale(&mut self, sc: WaveScale) {
        wv().sc = sc;
        self.levels_follow(); // the levels keep their SI value
        self.s.ac = sc.ac;
        wv().live = false;
        self.busy = false;
        // Stopped: the stored record stays (its own scale, `stored_ab`),
        // drawn at the new V/div.
        if self.s.run {
            self.stored_valid = false;
        }
    }

    /// A screen-only V/div step (`wave_steps`, the same DMM row): the codes
    /// keep their scale; the acquisition in flight restarts (`apply`: the
    /// trigger levels keep their SI value, their rows and codes follow), a
    /// stopped record is drawn again at the new V/div.
    pub fn wave_vdiv_set(&mut self, vdiv: f32, now: u64) {
        wv().sc.vdiv = vdiv;
        let s = self.s;
        self.apply(s, now);
        self.refresh();
    }

    /// The stream has to be (re)started at this rate (once).
    pub fn take_wave_restart(&mut self) -> Option<u32> {
        if self.wave && wv().restart {
            wv().restart = false;
            return Some(wv().fs);
        }
        None
    }

    /// Rate the wave view wants (0 = none).
    pub fn wave_fs(&self) -> u32 {
        if self.wave { wv().fs } else { 0 }
    }

    /// The stream was (re)started at `wave_fs` with a fresh ring (index 0).
    pub fn wave_live(&mut self) {
        let w = wv();
        // A new ring: every index (record, trigger) starts again at 0.
        (w.tot, w.slot0, w.first, w.dropped, w.seen, w.next_min) = (0, 0, 0, 0, 0, 0);
        (w.pend, w.ready, w.gen) = (None, None, 0);
        w.stale = false;
        w.live = true;
        w.shown = None;
        self.busy = false;
        if self.s.run {
            self.stored_valid = false;
        }
    }

    /// The stream is not running (stopped behind the view's back).
    pub fn wave_dead(&mut self) {
        wv().live = false;
        self.busy = false;
    }

    /// Arm a wave acquisition: the record resynchronised when stale (the
    /// backlog thrown away), roll set up, the ISR trigger configured.
    fn wave_start(&mut self, now: u64, src: &mut dyn WaveSource) {
        self.gsync = None; // the DMM's clock: no generator phase

        if wv().stale {
            let n = src.drain(&mut |_| {});
            let w = wv();
            w.tot += n as u64;
            w.slot0 = w.tot;
            w.first = w.tot;
            w.dropped = src.dropped();
            w.stale = false;
            w.shown = None;
            self.stored_valid = false;
        }
        self.rolling = self.roll_ok();
        let (pre, _) = self.hw_split();
        let w = wv();
        w.pend = None;
        w.ready = None;
        w.next_min = w.next_min.max(w.first + pre as u64);
        if self.rolling {
            // Samples per column from the timebase, not `screen`: that is
            // clamped to the record (CAP), so from 5 s/div (1200 S/s) every
            // slower timebase rolled at the 2 s/div speed (device d8e249d:
            // ~20 px/s at 2, 20 and 50 s/div). Roll keeps no record.
            let full = 25_000u64 * w.fs as u64 / TB[self.s.tb].0.max(1) as u64;
            w.per = (full / COLS as u64).clamp(1, u32::MAX as u64) as u32;
            w.racc = (u16::MAX, 0);
            w.rn = 0;
            unsafe { (*core::ptr::addr_of_mut!(ROLL_COLS)).fill((u16::MAX, u16::MAX)) };
            self.roll_next = now;
            src.trigger_off();
            w.gen = 0;
        } else {
            let st = self.s;
            let (wlo, whi) = crate::trigsoft::pulse_window(st.pcond, st.pwidth * w.fs as f32);
            let c = crate::trigsoft::StreamCfg {
                ttype: st.ttype,
                rising: st.rising,
                l: self.trig_code(),
                l2: self.runt_code(),
                wlo,
                whi,
            };
            // Single: the first event after the pre-trigger history; the
            // others pick from the event list (`wave_frame`).
            let min = if st.mode == TrigMode::Single { wv().first + pre as u64 } else { 0 };
            wv().gen = src.set_trigger(&c, min as u32);
            wv().seen = src.event_count();
        }
        self.deadline = now + self.auto_timeout();
        self.busy = true;
    }

    /// The wave view between the scope pass's slow parts (app.rs
    /// `sp_paint_pass`: before the paint and before a measurement cell):
    /// the ring into BUF so it never holds more than one part's time. The
    /// ring holds 2048 samples, 26.7 ms at 76 800 S/s, and a pass with a
    /// status change (status badge ~6 ms + record bar ~7.6 ms) plus a cell
    /// (~10 ms) after the acquisition (~6 ms) took ~30 ms (emulator --mips
    /// 110, a 10 Hz square at 1 ms/div): the ring overflowed, `dropped` grew
    /// (device c785807 M3: ~50/s) and every overflow restarted the record.
    pub fn wave_keep(&mut self, src: &mut dyn WaveSource) {
        if self.wave && wv().live && self.busy {
            self.wave_drain(src);
        }
    }

    /// Ring -> BUF (and the roll columns).
    fn wave_drain(&mut self, src: &mut dyn WaveSource) {
        let buf = unsafe { &mut *core::ptr::addr_of_mut!(BUF) };
        let cols = unsafe { &mut *core::ptr::addr_of_mut!(ROLL_COLS) };
        let w = wv();
        let (slot0, mut tot) = (w.slot0, w.tot);
        let roll = self.rolling;
        let (per, mut acc, mut rn) = (w.per, w.racc, w.rn);
        src.drain(&mut |raw| {
            let c = crate::trigsoft::ad2_code10(raw) as u16;
            buf[(tot.wrapping_sub(slot0) % CAP as u64) as usize] = c;
            tot += 1;
            if roll {
                acc = (acc.0.min(c), acc.1.max(c));
                rn += 1;
                if rn >= per {
                    cols.copy_within(1.., 0);
                    cols[COLS - 1] = acc;
                    acc = (u16::MAX, 0);
                    rn = 0;
                }
            }
        });
        (w.tot, w.racc, w.rn) = (tot, acc, rn);
    }

    /// Oldest index in BUF.
    fn wave_oldest(&self) -> u64 {
        wv().first.max(wv().tot.saturating_sub(CAP as u64))
    }

    /// Rotate BUF so the record runs oldest -> newest from 0; its length.
    fn wave_linear(&mut self) -> usize {
        let oldest = self.wave_oldest();
        let r = (oldest.wrapping_sub(wv().slot0) % CAP as u64) as usize;
        if r != 0 {
            unsafe { (&mut (&mut *core::ptr::addr_of_mut!(BUF))[..]).rotate_left(r) };
        }
        wv().slot0 = oldest;
        (wv().tot - oldest) as usize
    }

    /// Show the screen around trigger index `t` (None: the newest screen,
    /// untriggered) as the stored record's window.
    fn wave_emit(&mut self, now: u64, t: Option<u64>) -> Option<Frame<'static>> {
        let len = self.wave_linear();
        let base = wv().slot0;
        let (pre, _) = self.hw_split();
        if len < self.screen {
            return None;
        }
        let start = match t {
            Some(t) => (t - pre as u64 - base) as usize,
            None => len - self.screen,
        };
        let trig = t.map(|_| start + pre);
        self.snapshot_mark(len, start + self.screen / 2, trig, false);
        wv().shown = Some((base + (start + self.screen / 2) as u64, t));
        wv().last_emit = now;
        self.deadline = now + self.auto_timeout();
        if let Some(t) = t {
            wv().next_min = t + 1;
            if self.s.mode == TrigMode::Single {
                self.s.run = false;
                self.busy = false;
                wv().stale = true;
            }
        }
        let data: &'static [u16] = unsafe { &(&*core::ptr::addr_of!(BUF))[start..start + self.screen] };
        self.seq = self.seq.wrapping_add(1);
        Some(Frame { data, cols: None, trig, fs: wave_real_fs(wv().fs), seq: self.seq })
    }

    fn wave_frame(&mut self, now: u64, src: &mut dyn WaveSource) -> Option<Frame<'static>> {
        if !wv().live {
            // Waiting for the (re)start: whatever the old stream left.
            src.drain(&mut |_| {});
            return None;
        }
        if !self.busy {
            self.wave_start(now, src);
        }
        self.wave_drain(src);
        let holes = src.dropped();
        if holes != wv().dropped {
            // A hole: the ring overflowed (a main-loop stall > 213 ms at
            // 9600 S/s) or the ISR missed conversions. No triggered frame
            // spans it: the record starts again at the newest sample (and
            // the trigger re-arms); roll just goes on (a short gap in the
            // trace, as the timestamps say).
            wv().dropped = holes;
            if !self.rolling {
                let w = wv();
                w.first = w.tot;
                (w.pend, w.ready, w.shown) = (None, None, None);
                self.busy = false;
                return None;
            }
        }
        if self.rolling {
            if now < self.roll_next {
                return None;
            }
            self.roll_next = now + 100;
            self.seq = self.seq.wrapping_add(1);
            return Some(Frame { data: &[], cols: Some(unsafe { &*core::ptr::addr_of!(ROLL_COLS) }), trig: None, fs: wave_real_fs(wv().fs), seq: self.seq });
        }
        let (pre, post) = self.hw_split();
        let (tot, oldest) = (wv().tot, self.wave_oldest());
        let tail = tot as u32;
        // ISR index (u32, the ring's) -> record index; an event of a sample
        // pushed after the drain is ahead of `tot`.
        let abs = |i: u32| (tot as i64 - tail.wrapping_sub(i) as i32 as i64).max(0) as u64;
        let ok = |t: u64, nm: u64| t >= nm && t >= oldest + pre as u64;
        if self.s.mode == TrigMode::Single {
            if let Some(t) = src.first(wv().gen).map(abs) {
                if t + post as u64 <= tot {
                    if ok(t, 0) {
                        return self.wave_emit(now, Some(t));
                    }
                    // Its pre-trigger part is gone (cannot happen: the
                    // minimum is first + pre): arm again.
                    self.busy = false;
                }
            }
            return None;
        }
        // Normal / Auto: the newest event whose post-trigger part is in;
        // else wait for the earliest incomplete one.
        let w = wv();
        let nm = w.next_min;
        if let Some(p) = w.pend {
            if p + post as u64 <= tot {
                w.ready = Some(p);
                w.pend = None;
            }
        }
        let (mut ready, mut pend) = (w.ready, w.pend);
        let mut seen = w.seen;
        src.events(&mut seen, &mut |i| {
            let t = abs(i);
            if !ok(t, nm) {
                return;
            }
            if t + post as u64 <= tot {
                ready = Some(t);
                pend = None;
            } else if pend.is_none() {
                pend = Some(t);
            }
        });
        let w = wv();
        (w.ready, w.pend, w.seen) = (ready, pend, seen);
        if let Some(t) = ready {
            if now >= w.last_emit + WAVE_FRAME_MS && ok(t, 0) {
                w.ready = None;
                return self.wave_emit(now, Some(t));
            }
            return None;
        }
        if self.s.mode == TrigMode::Auto && now >= self.deadline && tot - oldest >= self.screen as u64 {
            return self.wave_emit(now, None);
        }
        None
    }

    /// STOP in the wave view: the record linearised and kept, anchored on
    /// the frame on screen (roll: the newest sample at the right edge).
    fn wave_freeze(&mut self) -> bool {
        let (was_busy, was_rolling) = (self.busy, self.rolling);
        self.busy = false;
        self.rolling = false;
        wv().stale = true;
        if !was_busy || !wv().live {
            return true;
        }
        let len = self.wave_linear();
        let base = wv().slot0;
        if was_rolling {
            self.snapshot_mark(len, len.saturating_sub(1), None, true);
            return true;
        }
        match wv().shown {
            Some((c, t)) if c >= base && c < base + len as u64 => {
                let trig = t.filter(|&t| t >= base && t < base + len as u64).map(|t| (t - base) as usize);
                self.snapshot_mark(len, (c - base) as usize, trig, false);
            }
            _ => self.snapshot_mark(len, len.saturating_sub(self.screen / 2), None, false),
        }
        true
    }
}

/// Debug: latch word, record DMA count left, first sample, GPIOB IDR.
#[cfg_attr(not(feature = "dbg-diag"), allow(dead_code))]
pub fn debug() -> [u32; 4] {
    unsafe {
        [
            core::ptr::read_volatile(core::ptr::addr_of!(LATCH[0])),
            r32(S4 + 0x04),
            core::ptr::read_volatile(core::ptr::addr_of!(BUF[0])) as u32,
            r32(GPIOB_IDR),
        ]
    }
}
