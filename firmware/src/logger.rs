//! Data-logger fast-stream mode (FR-38).
//!
//! A main-loop-polled, conversion-paced sampler for the HY3131 AD1 (DC) channel.
//! opendm40 runs with interrupts masked (`cpsid i`): this is a cooperative sampler
//! driven from the main loop, timestamped and paced by the DWT cycle counter
//! (clock.rs enables CYCCNT; HCLK = 200 MHz). The exceptions, with the
//! `ad2-isr` feature (module `isr` below, decisions-2026-10-22 #3): the AD2
//! source, and AD1 unchopped at 160 .. 640 S/s (`ad1_on_isr`, 2026-10-22: the
//! main-loop sampler lost 1.8 % at 640 S/s to long passes on the device):
//! a timer ISR is then the producer and the main loop only drains. FR-38 trades resolution for speed: the HY3131 AD1OSR is
//! dropped to its fast setting (see `hy3131::set_ad1_osr`) and we stream the raw
//! signed 24-bit AD1 counts.
//!
//! Rate ceiling: the HY3131 itself, not the SPI link (hy3131.rs "AD1
//! conversion timing"): period = OSR * 30 / 4.9152 MHz, x2 with the chopper.
//!   LOG:OSR  OSR     chopped (default)   LOG:CHOP OFF
//!   3        2048    25 ms    40/s        12.5 ms    80/s   (default code)
//!   2        1024    12.5 ms  80/s        6.25 ms   160/s
//!   1        512     6.25 ms 160/s        3.125 ms  320/s
//!   0        256     3.125 ms 320/s       1.5625 ms 640/s   (the chip's max)
//!   4..7     4096..32768: 50..400 ms chopped (20..2.5/s)
//! The 38 Hz measured on the device at the default is this 40/s (the old
//! "SPI-limited" diagnosis assumed OSR / SYSCLK, 417 us). Each sample costs
//! ~122 us of bit-banged SPI, so even 640/s keeps the link < 10 % busy; the
//! limit there is the main-loop period, which must stay below the conversion
//! period or samples are lost (`missed`; not measured on the device above
//! 40/s yet). Trade-offs: a lower OSR has fewer effective bits (more noise
//! per sample); LOG:CHOP OFF (AD1CHOP = 00) leaves the converter offset in
//! every sample (the calibration was taken chopped: zero the host's scale
//! with a shorted input at the same settings) and adds low-frequency noise.
//! Both settings apply to the logger only and take effect at LOG:STATe ON;
//! LOG:STATe OFF restores the DMM row (main.rs: `app.dmm.start` rewrites the
//! full configuration, R22 included). SI scaling is intentionally not applied here (the
//! calibration path lives in dmm.rs); `LOG:PREamble?` reports mode/range/OSR so
//! the host can scale, per FR-38's "accuracy sacrificed for speed".
//!
//! Each record carries a monotonic sequence number and a device microsecond
//! timestamp, so loss is always detectable, never silently skipped:
//!   * ring overflow (host draining `LOG:DATA?` too slowly) -> the NEWEST sample
//!     is dropped (single-producer / single-consumer ring: the producer, which
//!     may be the stream ISR, never moves the consumer's tail) and `overwritten`
//!     counts it; the seqs stay consecutive, the host sees a TIMESTAMP gap.
//!     (Before the SPSC ring, 2026-10-22, the oldest record was overwritten and
//!     the host saw a sequence gap; dm40scpi.py's lost_before handles both.)
//!   * a stalled main loop (a slow redraw, a relay pulse, an SPI-flash write)
//!     -> fewer samples captured than the conversion grid; the host sees a
//!     TIMESTAMP gap between consecutive seqs, and `missed` estimates it.
//! The timestamp is taken when AD1F was observed (0..one loop period after the
//! conversion latched); the underlying conversion grid itself is regular.
//! The wire timestamp is the low 32 bits of a device microsecond clock that
//! starts at 0 on `start`: it wraps every 2^32 us (~71.6 min), so the host must
//! unwrap it (add 2^32 whenever it steps backwards). `measured_hz` uses the
//! full 64-bit clock.
//!
//! USB-disk writes would void the interval: a host write to the mass-storage
//! disk erases a 4 KiB SPI-flash sector (45..400 ms) inside the main loop. The
//! MSC refuses writes while `disk_writes_blocked()` (main.rs
//! `set_write_inhibit`; the writable volume, vwrite.rs, gates its own), so a
//! session sees no such gap; host reads stay allowed.
//!
//! Transport: `LOG:DATA?` over USBTMC (priority per FR-38) or Bluetooth SCPI
//! (AD1 sessions only: docs/manual/scpi-reference.md); `drain(buf)` is
//! transport-agnostic.
//!
//! AD2 source (FR-38 (B), LOG:SOURce AD2; docs/dmm-input-scope-feasibility.md
//! sections 1, 2.3, 4): the HY3131's fast 19-bit converter at 1200 ..
//! 76 800 S/s (hy3131::ad2_r26_for: SAD2CLK, OSR2, ENCHOPAD2; the stream ISR's
//! rungs are 19 200 .. 76 800). In the stock AC rows the mux is kept, so the
//! source is DC-coupled in AC mV (OP2O - AGND) and AC A / mA / uA (OP1O -
//! PB3, the shunt drop through OP1) and AC-coupled (~0.5 Hz) in AC V (PB4 fed
//! through a capacitor from OP1O); DC V / DC mV stream from the DC-coupled V
//! row and the AC mV row (ad2exp.rs `dc_enter`, feasibility E7; `mode_ok`). Same
//! pacing as AD1 (INTF.AD2F, a P/2 gate), but since a bit-banged flag check
//! + 3-byte read cost ~122 us and the period is 13 .. 833 us, one main-loop
//! pass may take several samples: `poll` keeps sampling for up to
//! `AD2_BURST_US` while the next result is due within that budget. Without the
//! `ad2-isr` feature every main-loop stall longer than a period drops samples
//! (device d150919, SPI2: 8 % lost at 4800, 11 % at 9600), which the
//! timestamps, `missed`, the span-based `lost()` and the gap histogram
//! (`gaps()`, LOG:GAPs?) report. With it, the timer ISR (`isr`) takes every
//! sample while the main loop redraws; the same counters report what it
//! still misses, DBG:ISR? its own numbers. A read whose bits 23:19
//! are not a copy of bit 18 (a torn read, feasibility 1.4) is dropped and
//! counted (`bad`); it shows up as a gap. Records: the same 12-byte wire
//! format; `raw` = signed AD2 code (+-262143 = +-1.25 V / IG at the AD2 input,
//! IG = 1.0 in every stock row; scaling on the host from LOG:PREamble?).
//!
//! Compact LOG:DATA? (LOG:FORMat PACKed | DLG; ROWS, the 12-byte records,
//! stays the default). The reply is the same `#4nnnn` block; an empty ring
//! is `#40000` in every format. Every reply decodes on its own; all fields
//! little-endian:
//!   reply header, 20 B: 'L' 'P' (PACKed) | 'L' 'D' (DLG), u8 version (1),
//!     u8 source (0 AD1, 1 AD2), u32 rate (mHz: the stream ISR's rate, else
//!     the nominal), f32 a, f32 b (LOG:SCALe?: value = a + b * raw; 0, 0 for
//!     AD1), u32 records left in the ring after this reply;
//!   block header, 16 B: u32 seq of the first sample, u32 its stamp t0 (us,
//!     the wrapping device clock), u16 n, u8 width (3 | 4: PACKed samples;
//!     0: DLG chunks follow), u8 flags (1: the first block of the session,
//!     2: conversions were lost before it), u32 lost_before (conversions
//!     between the last sample of the previous reply and this one:
//!     round(dt * rate) - 1, 0 at the session's start).
//! PACKed: blocks until the reply is full, each with n samples of `width`
//! bytes (signed, two's complement; 3 for every AD1 / AD2 code, 4 for a
//! value outside i24). Sample i of a block is at t0 + i / rate (rounded to
//! us). A sample more than half a period off that grid starts a new block:
//! after a gap (a stall, a late ISR entry, a torn read, the full ring;
//! lost_before > 0, flag 2) or when the stamps drift from the nominal rate
//! by half a period (AD1's real period differs from the nominal by up to
//! ~5 %: a block every ~10 samples there; lost_before 0). The seqs of
//! consecutive blocks continue: a gap is never inside a block.
//! DLG: one block header (n = the rows of the reply, width 0), then
//! kind-3 chunks of the disk fast stream (docs/compact-log-format.md "Fast
//! stream (kind 3)", coded by fastenc.rs `FastEnc` into a 4 KiB FIFO of the
//! reply buffer, so a reply carries at most ~4 KiB of chunks): chunk row0 =
//! the seq of its first row - 1, every stamp and value exact, the first
//! row's lost_before in the block header, the later rows' from their dt.
//! Hosts: tools/dm40scpi.py `log_reply` (the DLG chunks: tools/dlog.py
//! `fast_chunk`).

use crate::fastenc::FastEnc;
use crate::hy3131 as hy;
use crate::mmio::r32;
#[cfg_attr(not(feature = "ad2-isr"), allow(unused_imports))]
use crate::trigsoft::{Stream, StreamCfg};
use core::cell::UnsafeCell;
use core::sync::atomic::{
    AtomicU32,
    Ordering::{Acquire, Relaxed, Release},
};

const DWT_CYCCNT: u32 = 0xE000_1004;
const CYC_PER_US: u32 = crate::clock::HCLK_HZ / 1_000_000; // 200

/// Ring capacity in records (a power of two). 2048 * 8 B = 16 KiB (`Ring`);
/// ~51 s at the default 40/s, ~3.2 s at AD1's fastest 640/s, 427 ms at
/// AD2's 4800/s, 213 ms at 9600/s (the host must drain LOG:DATA? at least
/// ~5x/s there, else `overwritten`). A static in SRAM (`RING`), not on the
/// main stack and not inside `Logger` (the stream ISR writes it).
pub const CAP: usize = 2048;
const _: () = assert!(CAP.is_power_of_two());

/// Default AD1OSR code, applied at startup (main.rs). OSR 2048 chopped (R22 =
/// 0x13, 25 ms, 40/s) is the stock's fastest setting; faster ones are opt-in
/// (LOG:OSR, LOG:CHOP OFF; table above).
pub const DEFAULT_OSR: u8 = hy::AD1OSR_STREAM;

/// AD1 sessions with a conversion period up to this (us) run on the stream
/// ISR (`Logger::ad1_on_isr`), unchopped: OSR 256 / 512 / 1024 (640 / 320
/// / 160 S/s).
pub const AD1_ISR_MAX_US: u32 = 6250;

/// A stream ISR session not locked this long after its start is
/// re-acquired, then ended (`Logger::lock_watch`).
const LOCK_WAIT_MS: u32 = 2000;

/// Main-loop AD1 sampler: blind reads in a row after a swallowed AD1F.
const RESCUE_MAX: u8 = 4;
/// AD1 intervals before `lost` uses their mean instead of the nominal
/// period (`Producer::span_period_ns`).
const P_MEAN_MIN: u32 = 8;

/// AD2 rate when LOG:SRATe was never set, S/s: 4800 with the stream ISR
/// (decisions #3: ~10 % of the CPU at 3.125 MHz), 2400 for the main-loop
/// sampler.
pub const DEFAULT_AD2_HZ: u32 = if cfg!(feature = "ad2-isr") { 4800 } else { 2400 };

/// Longest a single `poll` keeps taking AD2 samples, us (USB and the rest of
/// the loop wait that long at most). Main-loop sampler only.
pub const AD2_BURST_US: u64 = 500;

/// One sample in the ring: 8 bytes. On the wire 12, little-endian (seq,
/// ts_us, raw); the seq is not stored: record number k (from 0) of the
/// session is seq k + 1, and the ring holds consecutive ones (a full ring
/// drops the NEWEST sample, so nothing ever leaves except by `drain`).
#[derive(Clone, Copy)]
struct Rec {
    ts_us: u32,
    raw: i32,
}

const REC_BYTES: usize = 12;

/// LOG:FORMat values (`Logger::set_fmt`).
pub const FMT_ROWS: u8 = 0;
pub const FMT_PACKED: u8 = 1;
pub const FMT_DLG: u8 = 2;
/// The compact replies' version byte.
const FMT_VER: u8 = 1;
/// `#4nnnn`; the compact reply header; a block header.
const BLK_HDR: usize = 6;
const FMT_HDR: usize = 20;
const PK_BLK: usize = 16;
/// DLG: the coder's FIFO in the reply buffer (a power of two).
const DLG_FIFO: usize = 4096;
/// Block flags: the first block after LOG:STATe ON; conversions were lost
/// before this block (`lost_before` > 0).
const FL_START: u8 = 1;
const FL_GAP: u8 = 2;
/// ps per sample = PS_PER_S_MHZ / rate_mhz.
const PS_PER_S_MHZ: u64 = 1_000_000_000_000_000;

/// The scale and rate a compact reply carries (main.rs: LOG:SCALe?'s a, b).
pub struct FmtPar {
    pub rate_mhz: u32,
    pub a: f32,
    pub b: f32,
}

/// Gap histogram buckets (in conversion periods between two kept samples):
/// 2, 3..4, 5..16, 17..128, > 128.
pub const GAP_BUCKETS: usize = 5;

/// The single-producer / single-consumer ring and the producer's counters:
/// the only state shared with the stream ISR (decisions #3, invariant 4).
/// Producer = the stream ISR (AD2 with `ad2-isr`) or the main loop (AD1,
/// main-loop AD2), never both in one session; consumer = the main loop
/// (`drain`). Each atomic has one writer: `head` and every counter the
/// producer, `tail` the consumer; Release on `head` / Acquire on the other
/// side order the record slots. All-zero = empty (`new`, `reset`).
pub struct Ring {
    buf: UnsafeCell<[Rec; CAP]>,
    /// Records pushed this session (slot = head % CAP).
    head: AtomicU32,
    /// Records drained this session.
    tail: AtomicU32,
    captured: AtomicU32,
    /// Captured but dropped: the ring was full (LOG:COUNt? `overwritten`).
    dropped: AtomicU32,
    missed: AtomicU32,
    bad: AtomicU32,
    gaps: [AtomicU32; GAP_BUCKETS],
    max_gap_us: AtomicU32,
    /// First -> last captured sample, us, 64 bit as two words (main reads
    /// hi, lo, hi and retries on a change: the ISR may update between).
    span_lo: AtomicU32,
    span_hi: AtomicU32,
    // Stream ISR only (DBG:ISR?).
    /// Entries (A and B).
    ticks: AtomicU32,
    /// A entries too late for their data frame (skipped, a stamp gap).
    late: AtomicU32,
    /// Probes completed (B + the INTF read of the next A).
    probes: AtomicU32,
    overruns: AtomicU32,
    timeouts: AtomicU32,
    max_isr_cyc: AtomicU32,
    max_lat: AtomicU32,
    // The DMM wave view's trigger (`wave`, the stream ISR's `trig`): the
    // configuration words (main loop; `trg_gen` last, Release), the event
    // ring (ISR: `trg_ev` slots, then `trg_ev_n`, Release) and the first
    // event at or after the configured minimum index (ISR).
    trg_gen: AtomicU32,
    trg_w: [AtomicU32; 6],
    trg_ev: [AtomicU32; TRG_EV],
    trg_ev_n: AtomicU32,
    trg_first_at: AtomicU32,
    trg_first_gen: AtomicU32,
}

/// Trigger events the ISR keeps (the newest TRG_EV - 1 are safe to read
/// while it runs, `wave::events`).
const TRG_EV: usize = 8;

// SAFETY: the slots are written only by the producer at `head` and read
// only by the consumer below `head` (Acquire); every other field is atomic.
unsafe impl Sync for Ring {}

const Z: AtomicU32 = AtomicU32::new(0);

impl Ring {
    pub const fn new() -> Self {
        Ring {
            buf: UnsafeCell::new([Rec { ts_us: 0, raw: 0 }; CAP]),
            head: Z,
            tail: Z,
            captured: Z,
            dropped: Z,
            missed: Z,
            bad: Z,
            gaps: [Z; GAP_BUCKETS],
            max_gap_us: Z,
            span_lo: Z,
            span_hi: Z,
            ticks: Z,
            late: Z,
            probes: Z,
            overruns: Z,
            timeouts: Z,
            max_isr_cyc: Z,
            max_lat: Z,
            trg_gen: Z,
            trg_w: [Z; 6],
            trg_ev: [Z; TRG_EV],
            trg_ev_n: Z,
            trg_first_at: Z,
            trg_first_gen: Z,
        }
    }

    /// New session (main loop, no producer running).
    fn reset(&self) {
        for a in [
            &self.head, &self.tail, &self.captured, &self.dropped, &self.missed, &self.bad, &self.max_gap_us,
            &self.span_lo, &self.span_hi, &self.ticks, &self.late, &self.probes, &self.overruns, &self.timeouts,
            &self.max_isr_cyc, &self.max_lat, &self.trg_gen, &self.trg_ev_n, &self.trg_first_at, &self.trg_first_gen,
        ] {
            a.store(0, Relaxed);
        }
        for g in &self.gaps {
            g.store(0, Relaxed);
        }
    }

    fn len(&self) -> u32 {
        self.head.load(Acquire).wrapping_sub(self.tail.load(Relaxed))
    }

    fn span_us(&self) -> u64 {
        loop {
            let h = self.span_hi.load(Relaxed);
            let l = self.span_lo.load(Relaxed);
            if self.span_hi.load(Relaxed) == h {
                return (h as u64) << 32 | l as u64;
            }
        }
    }

    /// Producer: append, or drop the sample when full (newest dropped, so
    /// the consumer's `tail` is never written here). True when stored.
    fn push(&self, ts_us: u32, raw: i32) -> bool {
        let h = self.head.load(Relaxed);
        if h.wrapping_sub(self.tail.load(Acquire)) as usize >= CAP {
            inc(&self.dropped);
            return false;
        }
        // SAFETY: slot h is outside [tail, head): the consumer does not read
        // it until the Release store below publishes it.
        unsafe { (self.buf.get() as *mut Rec).add(h as usize & (CAP - 1)).write_volatile(Rec { ts_us, raw }) };
        self.head.store(h.wrapping_add(1), Release);
        true
    }
}

/// `a += 1` for a counter with a single writer (no RMW needed).
#[inline(always)]
fn inc(a: &AtomicU32) {
    a.store(a.load(Relaxed).wrapping_add(1), Relaxed);
}

/// The ring of the firmware (SRAM .dmabuf, NOLOAD: `Logger::init_in_place`
/// resets it; the slots' contents do not matter).
#[cfg_attr(target_os = "none", link_section = ".dmabuf")]
pub static RING: Ring = Ring::new();

/// The producer side's clock and per-sample statistics: the main loop's in
/// `Logger` (AD1, main-loop AD2), the stream ISR's own copy in `isr::PROD`
/// (set up by the main loop before the hand-off, the ISR's alone after it).
#[derive(Clone, Copy)]
struct Producer {
    period_ns: u32,
    nominal_us: u32,
    // DWT-derived monotonic microsecond clock from the session start (own
    // wrap tracking; CYCCNT wraps every 21.5 s).
    last_cyc: u32,
    acc: u32,
    now_us: u64,
    first_us: u64,
    last_ts_us: u32,
    have_last: bool,
    /// `now_us` when the last conversion was seen (the gate runs from it;
    /// bad reads included).
    last_sample_us: u64,
    /// No flag read within this many us of `last_sample_us`: P/2 (the
    /// main-loop sampler; the stream ISR does not poll the flag).
    gate_us: u32,
    /// Blind AD1 reads after a swallowed flag in a row (`sample`: at most
    /// RESCUE_MAX).
    rescued: u8,
    /// AD1: the last flag-to-flag interval (us, 0 = none yet): the chip's
    /// real period (device 7674c85: 26.3 ms at the 25 ms OSR 2048 chopped
    /// setting), for a rescued sample's stamp and window.
    p_meas: u32,
    /// AD1: sum (us) and count of those intervals: their mean is the
    /// chip's period (the observation delay is on both ends of each, it
    /// cancels in the mean; a single one is off by up to a loop pass),
    /// which `lost` counts the span in.
    p_sum: u64,
    p_n: u32,
    /// AD1: `now_us` of the last conversion seen by its flag (not a rescue)
    /// and the flags seen this session: the period is measured flag to flag
    /// (`sample`), so a rescue never hides the next real interval.
    last_flag_us: u64,
    flags: u32,
    /// The code of the last sample pushed (a rescue that reads it again
    /// read no new conversion: `sample`).
    last_raw: i32,
}

impl Producer {
    const fn new() -> Self {
        Producer {
            period_ns: 0,
            nominal_us: 0,
            last_cyc: 0,
            acc: 0,
            now_us: 0,
            first_us: 0,
            last_ts_us: 0,
            have_last: false,
            last_sample_us: 0,
            gate_us: 0,
            rescued: 0,
            p_meas: 0,
            p_sum: 0,
            p_n: 0,
            last_flag_us: 0,
            flags: 0,
            last_raw: 0,
        }
    }

    /// AD1: one flag-to-flag interval of `d` us (one period; a swallowed
    /// flag in between makes it ~2, outside `sample`'s window): the last
    /// one for the rescue, the mean for `lost`.
    fn note_period(&mut self, d: u32) {
        self.p_meas = d;
        self.p_sum += d as u64;
        self.p_n += 1;
    }

    /// The period `lost` divides the span by (ns): AD1 the measured mean
    /// once there are a few intervals (device c28c91c: 26.3 ms at the
    /// nominal 25 ms OSR 2048 chopped setting, 73 "lost" of a 60 s session
    /// against the nominal, none really), else the nominal.
    fn span_period_ns(&self, ad1: bool) -> u32 {
        if ad1 && self.p_n >= P_MEAN_MIN {
            ((self.p_sum * 1000 + self.p_n as u64 / 2) / self.p_n as u64).min(u32::MAX as u64) as u32
        } else {
            self.period_ns
        }
    }

    /// New session; gate = P/2 (the main-loop sampler).
    fn begin(&mut self, period_ns: u32) {
        *self = Producer::new();
        self.period_ns = period_ns;
        self.nominal_us = (period_ns + 500) / 1000;
        self.gate_us = self.nominal_us / 2;
        self.last_cyc = unsafe { r32(DWT_CYCCNT) };
    }

    /// Advance the microsecond clock to cycle count `c` (u32 maths only:
    /// called at least every few ms, far below the 21 s CYCCNT wrap).
    #[inline(always)]
    fn tick_at(&mut self, c: u32) {
        let d = c.wrapping_sub(self.last_cyc);
        self.last_cyc = c;
        let r = d % CYC_PER_US + self.acc;
        self.now_us += (d / CYC_PER_US + r / CYC_PER_US) as u64;
        self.acc = r % CYC_PER_US;
    }

    fn tick(&mut self) {
        self.tick_at(unsafe { r32(DWT_CYCCNT) });
    }

    /// One captured sample at `now_us` (wire stamp `ts_us`): statistics,
    /// then the ring. True when the ring took it (not full).
    fn push(&mut self, ring: &Ring, ts_us: u32, raw: i32) -> bool {
        self.last_raw = raw;
        inc(&ring.captured);
        // Timestamp-gap estimate of samples the producer was too late for.
        if self.have_last {
            let gap = ts_us.wrapping_sub(self.last_ts_us);
            let nom = self.nominal_us.max(1);
            if gap > nom * 3 / 2 {
                ring.missed.store(ring.missed.load(Relaxed).wrapping_add((gap / nom).saturating_sub(1)), Relaxed);
            }
            // k = round(gap / P) >= 2 needs gap >= 1.5 P: the 64-bit division
            // (__aeabi_uldivmod, ~0.5 us on the device) only for a gap of
            // more than 1.25 nominal periods, not for every sample (76.8 kS/s).
            if gap as u64 * 4 > nom as u64 * 5 {
                let pns = self.period_ns.max(1) as u64;
                let k = (gap as u64 * 1000 + pns / 2) / pns;
                if k >= 2 {
                    inc(&ring.gaps[gap_bucket(k)]);
                }
            }
            if gap > ring.max_gap_us.load(Relaxed) {
                ring.max_gap_us.store(gap, Relaxed);
            }
        } else {
            self.first_us = self.now_us;
            self.have_last = true;
        }
        self.last_ts_us = ts_us;
        let span = self.now_us - self.first_us;
        ring.span_hi.store((span >> 32) as u32, Relaxed);
        ring.span_lo.store(span as u32, Relaxed);
        ring.push(ts_us, raw)
    }
}

pub struct Logger {
    active: bool,
    /// DWT count at the main-loop producer's begin (`begin_cyc`).
    begin_cyc: u32,
    osr: u8,
    /// LOG:CHOP OFF. Stored inverted so that the all-zero `new()` (see
    /// `init_in_place`) is the chopped default.
    nochop: bool,
    /// LOG:SOURce AD2 (false = AD1, the all-zero default).
    ad2: bool,
    /// The running session's producer is the stream ISR.
    isr: bool,
    /// The running / last session's producer was the stream ISR (`lost`).
    isr_sess: bool,
    /// `plan` ran for the next session (`r26` holds its rung).
    planned: bool,
    /// The session's nominal rate, mHz (`nominal_mhz`), set by `plan`.
    nom_mhz: u32,
    /// The stream ISR session's lock watch (`poll`): its rate (mHz), the
    /// DWT count of its (re)start, and whether it was re-acquired once.
    isr_rate: u32,
    isr_t0: u32,
    isr_retried: bool,
    /// LOG:SRATe (AD2), S/s; 0 = DEFAULT_AD2_HZ.
    req_hz: u32,
    /// R26 low bits of the running AD2 session (preamble).
    r26: u8,
    mode: u8,
    range: i8,
    /// Main-loop producer (AD1, main-loop AD2).
    prod: Producer,
    ring: *const Ring,
    /// The running session belongs to the DMM wave view (`start_wave`):
    /// the ring is its, not the host's (no LOG:DATA?, LOG:STATe? 0, the GUI
    /// runs). The host's LOG:SOURce / LOG:SRATe saved for the stop.
    wave: bool,
    saved_ad2: bool,
    saved_hz: u32,
    /// `dc_prepare` ran for the planned session: the next `start` keeps
    /// its DC V self-cal (writes the row only).
    dc_keep: bool,
    /// LOG:FORMat: FMT_ROWS (the all-zero default), FMT_PACKED, FMT_DLG.
    fmt: u8,
    /// The stamp of the last record drained this session, and whether
    /// there is one (`lost_before` of the next reply's first block).
    drained_ts: u32,
    drained: bool,
}

/// Gap bucket of a gap of `k` periods (k >= 2).
fn gap_bucket(k: u64) -> usize {
    match k {
        0..=2 => 0,
        3..=4 => 1,
        5..=16 => 2,
        17..=128 => 3,
        _ => 4,
    }
}

impl Logger {
    /// `new()` in place for an instance in NOLOAD RAM (main.rs LOGGER is in
    /// .dmabuf, not zeroed at reset): `new()` is all zero bits (false / 0 /
    /// null), and all zero bits is a valid Logger; then the ring.
    pub fn init_in_place(this: *mut Logger, ring: &'static Ring) {
        unsafe {
            core::ptr::write_bytes(this, 0, 1);
            (*this).ring = ring;
        }
        ring.reset();
    }

    /// All-zero (see `init_in_place`). Runtime defaults (OSR) are applied in
    /// main.rs via `set_osr(DEFAULT_OSR)`.
    pub const fn new() -> Self {
        Logger {
            active: false,
            begin_cyc: 0,
            osr: 0,
            nochop: false,
            ad2: false,
            isr: false,
            isr_sess: false,
            planned: false,
            nom_mhz: 0,
            isr_rate: 0,
            isr_t0: 0,
            isr_retried: false,
            req_hz: 0,
            r26: 0,
            mode: 0,
            range: 0,
            prod: Producer::new(),
            ring: core::ptr::null(),
            wave: false,
            saved_ad2: false,
            saved_hz: 0,
            dc_keep: false,
            fmt: FMT_ROWS,
            drained_ts: 0,
            drained: false,
        }
    }

    fn ring(&self) -> &Ring {
        unsafe { &*self.ring }
    }

    pub fn active(&self) -> bool {
        self.active
    }

    /// True while USB-disk (MSC) writes must be refused: each one erases a
    /// SPI-flash sector and stalls the main loop for 45..400 ms, i.e. hundreds
    /// of lost conversions. The MSC answers WRITE with NOT READY
    /// (sense 02/04/01) while this holds; reads stay allowed.
    pub fn disk_writes_blocked(&self) -> bool {
        self.active
    }

    /// AD1OSR code (0..7). Takes effect at the next `start`.
    pub fn set_osr(&mut self, code: u8) {
        self.osr = code & 7;
    }

    pub fn osr(&self) -> u8 {
        self.osr
    }
    /// AD1 chopper (half the rate) / AD2 ENCHOPAD2 (same rate); offset cancelled; default
    /// on). Takes effect at the next `start`.
    pub fn set_chop(&mut self, on: bool) {
        self.nochop = !on;
    }
    pub fn chop(&self) -> bool {
        !self.nochop
    }
    /// LOG:SOURce: false = AD1 (DC rows), true = AD2 (AC rows). Takes effect
    /// at the next `start`.
    pub fn set_ad2(&mut self, on: bool) {
        if self.wave { self.saved_ad2 = on } else { self.ad2 = on }
    }
    pub fn ad2(&self) -> bool {
        self.ad2
    }
    /// The DMM modes a source can stream from: AD1 = DC V / mV / A (1, 3,
    /// 7..9); AD2 = the stock AC rows (0 AC V, AC-coupled; 2 AC mV and 4..6
    /// AC A / mA / uA, DC-coupled) and DC V / DC mV (1: the DC-coupled V
    /// row with its self-cal, 3: the AC mV row; feasibility E7,
    /// ad2exp.rs `dc_enter`). AC+DC (16 / 17) alternates rows: neither.
    /// DC V / mV fit both: where the source is implicit (the Record page)
    /// AD1 is taken there (`ad1_first`).
    pub fn mode_ok(ad2: bool, mode: u8) -> bool {
        if ad2 { matches!(mode, 0..=6) } else { matches!(mode, 1 | 3 | 7..=9) }
    }
    /// The implicit source of `mode` (the Record page's fast logger): AD1
    /// where it can (24 bit), else AD2; None where neither streams.
    pub fn ad1_first(mode: u8) -> Option<bool> {
        if Self::mode_ok(false, mode) {
            Some(false)
        } else if Self::mode_ok(true, mode) {
            Some(true)
        } else {
            None
        }
    }
    /// The DC rows' AD2 row and self-cal (ad2exp.rs `dc_enter`) for the
    /// planned session now, not at `start`: the fast logger's file header
    /// needs the scale before the disk preparation (main.rs `fast_start`).
    /// The next `start` keeps the cal (rewrites the row only).
    pub fn dc_prepare(&mut self) {
        if self.planned && self.ad2 && matches!(self.mode, 1 | 3) {
            crate::ad2exp::dc_enter(self.mode, self.range.max(0) as u8, self.r26, true);
            self.dc_keep = true;
        }
    }
    /// LOG:SRATe: requested AD2 rate, S/s (the nearest rung of the ladder is
    /// used, hy3131::ad2_r26_for). Takes effect at the next `start`.
    pub fn set_req_hz(&mut self, hz: u32) {
        if self.wave { self.saved_hz = hz } else { self.req_hz = hz }
    }
    pub fn req_hz(&self) -> u32 {
        if self.req_hz == 0 { DEFAULT_AD2_HZ } else { self.req_hz }
    }
    /// R26 low bits the next / running AD2 session uses.
    pub fn r26(&self) -> u8 {
        if (self.active || self.planned) && self.ad2 { self.r26 } else { hy::ad2_r26_for(self.req_hz(), self.chop()) }
    }
    pub fn mode(&self) -> u8 {
        self.mode
    }
    pub fn range(&self) -> i8 {
        self.range
    }
    pub fn nominal_us(&self) -> u32 {
        self.prod.nominal_us
    }
    /// Nominal (chip) rate of the session in mHz: the chip's conversion
    /// rate from its clock (`hy::ad2_real_rate_mhz` / `hy::ad1_rate_mhz`,
    /// the rate the stream ISR runs at and ACQ:SRATe? reports), not
    /// 1e12 / the period rounded to 1 ns (device 2f56ff9: 76 799 016 for
    /// the 76.8 kS/s rung's 13 021 ns, against ACQ:SRAT? 76 800).
    pub fn nominal_mhz(&self) -> u32 {
        self.nom_mhz
    }
    pub fn buffered(&self) -> u32 {
        self.ring().len()
    }
    pub fn captured(&self) -> u32 {
        self.ring().captured.load(Relaxed)
    }
    /// Captured samples dropped because the ring was full (the host did not
    /// drain LOG:DATA? fast enough). The ring keeps the OLDEST records (the
    /// stream ISR cannot move the consumer's tail), so the host sees the
    /// loss as a timestamp gap, the seqs stay consecutive.
    pub fn overwritten(&self) -> u32 {
        self.ring().dropped.load(Relaxed)
    }
    pub fn missed(&self) -> u32 {
        self.ring().missed.load(Relaxed)
    }
    pub fn bad(&self) -> u32 {
        self.ring().bad.load(Relaxed)
    }
    pub fn gaps(&self) -> [u32; GAP_BUCKETS] {
        let g = &self.ring().gaps;
        [0, 1, 2, 3, 4].map(|i| g[i].load(Relaxed))
    }
    pub fn max_gap_us(&self) -> u32 {
        self.ring().max_gap_us.load(Relaxed)
    }

    /// Conversions the session did not capture, from the span: the chip
    /// delivered round(span / period) + 1 results between the first and the
    /// last captured sample; `captured` of them were taken. Unlike `missed`
    /// (per gap, > 1.5 periods) it also counts the many slightly-late
    /// samples of a rate the loop cannot sustain (9600/s: a 104 us period vs
    /// ~122 us per bit-banged sample). The main-loop sampler counts AD1
    /// against the measured mean period (`span_period_ns`: the chopped
    /// settings run ~5 % slower than nominal), AD2 against the nominal (a
    /// chip clock off by x % shows as x % lost, or as 0); the stream ISR
    /// counts the periods (its A entries).
    pub fn lost(&self) -> u32 {
        let (captured, span) = (self.captured(), self.ring().span_us());
        // The stream ISR counts the conversion periods themselves (one entry
        // A each, phase-locked to the chip): lost = periods - captured,
        // exact, whatever the crystal offset.
        #[cfg(feature = "ad2-isr")]
        if self.isr_sess && isr::lock_stats()[8] != 0 {
            return isr::lock_stats()[8].saturating_sub(captured);
        }
        let p = self.prod.span_period_ns(!self.ad2);
        if captured < 2 || p == 0 {
            return 0;
        }
        let due = (span * 1000 + p as u64 / 2) / p as u64 + 1;
        due.saturating_sub(captured as u64).min(u32::MAX as u64) as u32
    }

    /// Measured mean sample rate in Hz over the current session, from the first
    /// and last sample times (64-bit, no 71.6 min wrap) and the capture count
    /// (the real number, off the device, not the nominal rate). 0 until at
    /// least two samples.
    pub fn measured_hz(&self) -> u32 {
        let (captured, span) = (self.captured(), self.ring().span_us());
        if captured < 2 || span == 0 {
            return 0;
        }
        (((captured as u64 - 1) * 1_000_000) / span) as u32
    }

    /// The next session's settings, without a bus access: mode / range (the
    /// preamble), the AD2 rung (capped by the stream ISR's read divider),
    /// the period (ns, returned; `nominal_mhz` / `r26` report them). `start`
    /// runs it first; the fast logger's session start (main.rs
    /// `fast_start`, region C) runs it alone to write the file header
    /// before `start` hands the bus to the stream ISR (no `.cold` code may
    /// run while it streams: a region C fetch stalls the ISR's entry,
    /// link-native.x). Not during a session.
    pub fn plan(&mut self, mode: u8, range: i8) -> u32 {
        self.mode = mode;
        self.range = range;
        let period_ns = if self.ad2 {
            // The stream ISR's read divider caps the rung.
            #[cfg(feature = "ad2-isr")]
            let req = {
                let dma = hy::stream::use_dma();
                let (div, _) = hy::stream::choose_div(dma, self.req_hz());
                self.req_hz().min(hy::stream::max_rate_hz(div, dma))
            };
            #[cfg(not(feature = "ad2-isr"))]
            let req = self.req_hz();
            self.r26 = hy::ad2_r26_for(req, self.chop());
            self.nom_mhz = hy::ad2_real_rate_mhz(self.r26);
            hy::ad2_real_period_ns(self.r26)
        } else {
            self.r26 = 0;
            self.nom_mhz = hy::ad1_rate_mhz(self.osr, self.chop());
            hy::ad1_period_us(self.osr, self.chop()) * 1000
        };
        self.prod.period_ns = period_ns;
        self.planned = true;
        period_ns
    }

    /// The next AD1 session runs on the stream ISR (feature `ad2-isr`): the
    /// fast unchopped AD1 settings (160 .. 640 S/s, period <= AD1_ISR_MAX_US), where a
    /// main-loop pass (a panel redraw, a page program, USB) can outlast the
    /// period (device e656df3: 1.8 % lost at 640 S/s with the main-loop
    /// sampler). The slower ones keep the main-loop sampler (a period of
    /// 12.5 ms or more; no lock-in time before the first sample).
    /// Unchopped only: the chopped settings run ~5 % slower than their
    /// nominal (device 7674c85: 26.3 ms at OSR 2048 chopped, nominal 25),
    /// beyond the Pll's +-5000 ppm (ad2pll.rs MAX_PPM), which locks to the nominal rate.
    pub fn ad1_on_isr(&self) -> bool {
        cfg!(feature = "ad2-isr") && !self.ad2 && !self.chop() && hy::ad1_period_us(self.osr, false) <= AD1_ISR_MAX_US
    }

    /// Begin a session: configure the HY3131 (AD1: R22 OSR / chopper; AD2:
    /// R26 rung, AD3 / RMS / LPF off), reset all state and, for AD2 with the
    /// `ad2-isr` feature (and the fast AD1 settings, `ad1_on_isr`), hand the
    /// bus to the stream ISR. `mode`/`range` are the current DMM function
    /// (for the preamble); the caller must have selected a mode `mode_ok`
    /// for the source, written its full row (dmm.start) and powered the
    /// analog side.
    pub fn start(&mut self, mode: u8, range: i8) {
        let keep = core::mem::take(&mut self.dc_keep) && (mode, range) == (self.mode, self.range);
        self.stop();
        let period_ns = self.plan(mode, range);
        let on_isr = self.ad2 || self.ad1_on_isr();
        #[cfg(feature = "ad2-isr")]
        if self.ad2 {
            let _ = hy::stream::pick_div(hy::stream::use_dma(), hy::ad2_rate_mhz(self.r26) / 1000);
        } else if on_isr {
            hy::stream::use_div(hy::DIV, false);
        }
        self.ring().reset();
        self.drained = false;
        // The chip set-up is the last main-loop bus access of an ISR session.
        if self.ad2 {
            if matches!(mode, 1 | 3) {
                // DC V / mV: their AD2 row (and the DC V self-cal) first.
                crate::ad2exp::dc_enter(mode, range.max(0) as u8, self.r26, !keep);
            }
            hy::set_ad2_stream(self.r26);
        } else {
            hy::set_ad1_osr(self.osr, self.chop());
        }
        self.prod.begin(period_ns);
        self.begin_cyc = self.prod.last_cyc;
        self.active = true;
        self.isr_sess = false;
        #[cfg(feature = "ad2-isr")]
        if on_isr {
            let rate = if self.ad2 { hy::ad2_real_rate_mhz(self.r26) } else { hy::ad1_rate_mhz(self.osr, self.chop()) };
            isr::start(&self.prod, self.ring, rate, !self.ad2);
            self.isr = true;
            self.isr_sess = true;
            self.isr_rate = rate;
            self.isr_t0 = unsafe { r32(DWT_CYCCNT) };
            self.isr_retried = false;
        }
        #[cfg(not(feature = "ad2-isr"))]
        let _ = on_isr;
    }

    /// End the session (records stay readable). With the stream ISR: the
    /// timer is stopped and the bus is the main loop's again on return; the
    /// caller restores the HY3131 (app.dmm.start).
    pub fn stop(&mut self) {
        #[cfg(feature = "ad2-isr")]
        if self.isr {
            hy::stream::stop();
            isr::stopped();
        }
        self.isr = false;
        self.active = false;
        self.planned = false;
        self.dc_keep = false;
        self.end_wave();
    }

    /// A session for the DMM wave view (scope page, source DMM): AD2 at
    /// `hz` (a ladder rung) in the current AC row, the host's LOG:SOURce /
    /// LOG:SRATe untouched. Same chip set-up and ISR hand-off as `start`.
    pub fn start_wave(&mut self, mode: u8, range: i8, hz: u32) {
        self.stop();
        (self.saved_ad2, self.saved_hz) = (self.ad2, self.req_hz);
        self.ad2 = true;
        self.req_hz = hz;
        self.start(mode, range);
        self.wave = true;
    }

    fn end_wave(&mut self) {
        if self.wave {
            self.wave = false;
            (self.ad2, self.req_hz) = (self.saved_ad2, self.saved_hz);
        }
    }

    /// The running (or last) session is the wave view's.
    pub fn wave(&self) -> bool {
        self.wave
    }

    /// DWT count at the running session's begin (the stamps' zero): the
    /// stream ISR's producer, else the main-loop one.
    pub fn begin_cyc(&self) -> u32 {
        #[cfg(feature = "ad2-isr")]
        if self.isr {
            return isr::begin_cyc();
        }
        self.begin_cyc
    }

    /// A host (LOG:STATe ON) session runs: the GUI is frozen for it.
    pub fn host_active(&self) -> bool {
        self.active && !self.wave
    }

    /// The running session's producer is the stream ISR.
    pub fn on_isr(&self) -> bool {
        self.isr
    }

    /// DBG:ISR? numbers of the last / running ISR session: ticks (entries),
    /// late (A entries that skipped their read), probes, overruns (a tick
    /// already pending at exit), frame timeouts, max cycles in the handler,
    /// max entry latency in timer counts (20 ns).
    pub fn isr_stats(&self) -> [u32; 7] {
        let r = self.ring();
        [&r.ticks, &r.late, &r.probes, &r.overruns, &r.timeouts, &r.max_isr_cyc, &r.max_lat].map(|a| a.load(Relaxed))
    }

    /// Main-loop entry. With the stream ISR: no bus access; returns true
    /// once when the stream was stopped behind the logger's back (a
    /// main-loop bus access, hy3131 `guard`): the session is ended then and
    /// main.rs reports "Settings conflict". Otherwise the main-loop sampler
    /// (below); returns false.
    pub fn poll(&mut self) -> bool {
        if !self.active {
            return false;
        }
        if self.isr {
            if !hy::streaming() {
                self.isr = false;
                self.active = false;
                self.end_wave();
                return true;
            }
            #[cfg(feature = "ad2-isr")]
            if self.lock_watch() {
                // Never locked: the session ends as a bus conflict does.
                hy::stream::stop();
                isr::stopped();
                self.isr = false;
                self.active = false;
                self.end_wave();
                return true;
            }
            return false;
        }
        self.sample();
        false
    }

    /// A gated stream (DMA frames, or AD1 without an edge from the search)
    /// takes no sample until its Pll locks; one that has not locked
    /// LOCK_WAIT_MS after its start is acquired again (the edge search, a
    /// fresh Pll; nothing was captured, so the time base may restart), and
    /// once more not locked ends the session (true). (Emulator 7674c85: an
    /// AD2 19200 DMA stream never locked and the disk session never ended.)
    #[cfg(feature = "ad2-isr")]
    fn lock_watch(&mut self) -> bool {
        if isr::locked() || self.captured() != 0 {
            return false;
        }
        let now = unsafe { r32(DWT_CYCCNT) };
        if now.wrapping_sub(self.isr_t0) < LOCK_WAIT_MS * (crate::clock::HCLK_HZ / 1000) {
            return false;
        }
        if self.isr_retried {
            return true;
        }
        self.isr_retried = true;
        self.isr_t0 = now;
        hy::stream::stop();
        isr::stopped();
        if self.ad2 {
            let _ = hy::stream::pick_div(hy::stream::use_dma(), self.isr_rate / 1000);
        } else {
            hy::stream::use_div(hy::DIV, false);
        }
        isr::start(&self.prod, self.ring, self.isr_rate, !self.ad2);
        false
    }

    /// The main-loop sampler: capture fresh conversions. Cheap when idle (one DWT
    /// read and an early return). The HY3131 AD1F / AD2F flag paces this at
    /// the conversion rate. For half a conversion period after the last
    /// captured sample no new conversion can be due, so INTF is not read
    /// then; after that it is read on every call until the next conversion
    /// is seen. AD1: at most one sample per call. AD2: keeps going while the
    /// next result is due within `AD2_BURST_US` of the call (see the header).
    /// The timestamp is taken when the flag was seen, before the data read.
    ///
    /// The gate runs from the last SAMPLE, not the last INTF check: gated from
    /// the check, a miss pushed the next check another gate period out, so the
    /// observation drifted late and conversions got overwritten unread when the
    /// loop period L was a sizeable fraction of the period P (0.8 P gate,
    /// L = 0.7 P: checks 1.4 P apart). With a P/2 gate from the sample, the
    /// first check after it comes ceil(P/2 / L) * L < P after the sample for a
    /// steady L < P, so the delay between a conversion and its observation
    /// stays under L and no conversion is lost while the loop is faster than
    /// the conversion rate (a gate of 0.8 P would not guarantee that).
    fn sample(&mut self) {
        let ring: &Ring = unsafe { &*self.ring };
        self.prod.tick();
        let end = self.prod.now_us + if self.ad2 { AD2_BURST_US } else { 0 };
        let half = self.prod.gate_us as u64;
        loop {
            let open = if self.prod.have_last { self.prod.last_sample_us + half } else { 0 };
            if self.prod.now_us < open {
                // Gate closed: wait here only if it opens within the burst.
                if open > end {
                    return;
                }
                while self.prod.now_us < open {
                    self.prod.tick();
                }
            }
            let ready = if self.ad2 { hy::ad2_ready() } else { hy::ad1_ready() };
            let p = if self.prod.p_meas != 0 { self.prod.p_meas } else { self.prod.nominal_us } as u64;
            let since = self.prod.now_us.wrapping_sub(self.prod.last_sample_us);
            if !ready && !self.ad2 && self.prod.have_last && self.prod.rescued < RESCUE_MAX && since >= p + p / 4 && since < p + 3 * p / 4 {
                // No AD1F a quarter period past the conversion's due time:
                // an INTF read raced the conversion and cleared its flag
                // unreported (the register clears on read, as for AD2F,
                // feasibility 9.5; emulator IntfRaceAd1). R00..R02 still hold
                // the result for the rest of the period: read it blind,
                // stamped at its due time (one period after the last
                // sample's observation, the measured period); not near the next conversion
                // (a read across its update could tear). At most RESCUE_MAX
                // in a row (a converter that stopped is not read as a
                // stream of copies; a tight polling loop, the emulator's,
                // loses one flag in ~4 at 40 S/s); counted in DBG:ISR? `late`.
                // The same code as the last sample: R00..R02 still hold that
                // conversion, so no flag was lost, the next one is late (the
                // main loop's observation of a flag jitters by up to ~50 ms:
                // device 25b6ec3, OSR 8192 chopped after a 640/s session,
                // flags seen 73-79 / 123-128 ms apart, a copy before every
                // other one); keep waiting. A swallowed flag whose code
                // repeats the last one (~1-2 % at these noise levels) is lost.
                let raw = hy::read_ad1();
                if raw != self.prod.last_raw {
                    self.prod.rescued += 1;
                    self.prod.last_sample_us += p;
                    inc(&ring.late);
                    let _ = self.prod.push(ring, self.prod.last_sample_us as u32, raw);
                }
            } else if ready {
                self.prod.tick(); // the flag-observed moment
                // AD1's period: flag to flag, not from a rescue's stamp, and not
                // from the session's first flag (it can be one raised before the
                // start: device ea73ea5, after a 640/s session, a first interval
                // of 52 ms at OSR 8192 chopped (100.8 ms) became p_meas, and with
                // `rescued != 0` before every later flag it was never measured
                // again: each conversion was rescued once more, a copy 52 ms
                // after it, in 121 of 294 records (osr/ fr_40_15)).
                let nom = self.prod.nominal_us as u64;
                if !self.ad2 {
                    let d = self.prod.now_us.wrapping_sub(self.prod.last_flag_us);
                    if self.prod.flags >= 2 && d > nom / 2 && d < nom * 3 / 2 {
                        self.prod.note_period(d as u32);
                    }
                    self.prod.flags = self.prod.flags.saturating_add(1);
                    self.prod.last_flag_us = self.prod.now_us;
                }
                let after_rescue = core::mem::take(&mut self.prod.rescued) != 0;
                self.prod.last_sample_us = self.prod.now_us;
                let ts = self.prod.now_us as u32;
                if self.ad2 {
                    let (raw, ok) = hy::read_ad2();
                    if ok {
                        let _ = self.prod.push(ring, ts, raw);
                    } else {
                        inc(&ring.bad);
                    }
                } else {
                    // A rescue that read this conversion before its flag
                    // came (the conversion ended between the flag check and
                    // the read): the same code again, not a new sample.
                    let raw = hy::read_ad1();
                    if !(after_rescue && raw == self.prod.last_raw) {
                        let _ = self.prod.push(ring, ts, raw);
                    }
                }
            }
            self.prod.tick();
            if self.prod.now_us >= end {
                return;
            }
        }
    }

    /// Pop oldest-first records into `out` (raw 12-byte LE records, no framing).
    /// Returns the number of bytes written. Transport-agnostic: USBTMC today,
    /// BT later.
    pub fn drain(&mut self, out: &mut [u8]) -> usize {
        let ring = self.ring();
        let t = ring.tail.load(Relaxed);
        let n = (ring.head.load(Acquire).wrapping_sub(t) as usize).min(out.len() / REC_BYTES);
        let base = ring.buf.get() as *const Rec;
        let mut last = 0u32;
        for i in 0..n {
            let k = t.wrapping_add(i as u32);
            // SAFETY: k is below the Acquire-loaded head: published, and the
            // producer does not write it again before `tail` passes it.
            let r = unsafe { base.add(k as usize & (CAP - 1)).read_volatile() };
            let seq = k.wrapping_add(1);
            let o = i * REC_BYTES;
            out[o..o + 4].copy_from_slice(&seq.to_le_bytes());
            out[o + 4..o + 8].copy_from_slice(&r.ts_us.to_le_bytes());
            out[o + 8..o + 12].copy_from_slice(&r.raw.to_le_bytes());
            last = r.ts_us;
        }
        ring.tail.store(t.wrapping_add(n as u32), Release);
        if n > 0 {
            // A compact reply after this one counts its lost_before from
            // here (LOG:FORMat may change between replies).
            self.drained_ts = last;
            self.drained = true;
        }
        n * REC_BYTES
    }

    /// The fast logger's drain (fastlog::Src): (stamp us, raw) records,
    /// oldest first, no seq and no byte packing (the 12-byte form costs
    /// memcpy calls per field at opt-level z: ~15 % of the writer's CPU at
    /// 76.8 kS/s on the device model). Returns the count.
    #[inline(always)]
    pub fn drain_recs(&mut self, out: &mut [(u32, i32)]) -> usize {
        let ring = self.ring();
        let t = ring.tail.load(Relaxed);
        let n = (ring.head.load(Acquire).wrapping_sub(t) as usize).min(out.len());
        let base = ring.buf.get() as *const Rec;
        for (i, o) in out[..n].iter_mut().enumerate() {
            // SAFETY: as `drain`.
            let r = unsafe { base.add(t.wrapping_add(i as u32) as usize & (CAP - 1)).read_volatile() };
            *o = (r.ts_us, r.raw);
        }
        ring.tail.store(t.wrapping_add(n as u32), Release);
        n
    }

    /// Records in the ring.
    #[inline(always)]
    pub fn waiting(&self) -> u32 {
        self.ring().len()
    }

    /// The stream ISR's busy cycles so far (wrapping; 0 without it).
    #[inline(always)]
    pub fn isr_busy_cyc(&self) -> u32 {
        #[cfg(feature = "ad2-isr")]
        return isr::busy_cyc();
        #[cfg(not(feature = "ad2-isr"))]
        return 0;
    }

    /// Fill `out` with a USBTMC definite-length `#4nnnn` block of drained
    /// records and return the total byte count (header + payload). Mirrors the
    /// `Dbg::Lcd` framing in main.rs: `#` + one length-of-length digit (4) + a
    /// 4-digit payload byte count + the payload. Up to 511 records per block
    /// (6132 payload bytes) fit the 6144-byte USBTMC response buffer.
    pub fn encode(&mut self, out: &mut [u8]) -> usize {
        const HDR: usize = 6; // "#4nnnn"
        let maxrec = out.len().saturating_sub(HDR) / REC_BYTES;
        // The wave view's ring is the scope's, not the host's: an empty block.
        let avail = if self.active && self.wave { 0 } else { self.ring().len() as usize };
        let nrec = avail.min(maxrec).min(9999 / REC_BYTES);
        let bytes = nrec * REC_BYTES;
        out[0] = b'#';
        out[1] = b'4';
        for (i, d) in [1000usize, 100, 10, 1].iter().enumerate() {
            out[2 + i] = b'0' + ((bytes / d) % 10) as u8;
        }
        self.drain(&mut out[HDR..HDR + bytes]);
        HDR + bytes
    }

    /// LOG:FORMat (FMT_*).
    pub fn set_fmt(&mut self, f: u8) {
        self.fmt = if f <= FMT_DLG { f } else { FMT_ROWS };
    }
    pub fn fmt(&self) -> u8 {
        self.fmt
    }

    /// The rate the compact formats reconstruct the time with, mHz: the
    /// stream ISR's (the chip's real rate, `hy::ad2_real_rate_mhz`), else
    /// the nominal one.
    pub fn rate_mhz(&self) -> u32 {
        if self.isr_sess && self.isr_rate != 0 { self.isr_rate } else { self.nominal_mhz() }
    }

    /// Record `k` (session index) of the ring; the caller keeps it in
    /// [tail, head).
    #[inline(always)]
    fn rec(&self, k: u32) -> Rec {
        // SAFETY: as `drain`.
        unsafe { (self.ring().buf.get() as *const Rec).add(k as usize & (CAP - 1)).read_volatile() }
    }

    /// The reply of LOG:DATA? in the current LOG:FORMat into `out` (the
    /// transport's reply buffer); the byte count.
    pub fn encode_fmt(&mut self, out: &mut [u8], p: &FmtPar) -> usize {
        match self.fmt {
            FMT_PACKED => self.encode_packed(out, p),
            FMT_DLG => self.encode_dlg(out, p),
            _ => self.encode(out),
        }
    }

    /// Records to send now (0: the wave view's ring, or empty) and the
    /// payload room of a `#4nnnn` block in `out`.
    fn reply_room(&self, out: &[u8]) -> (u32, usize) {
        let avail = if self.active && self.wave { 0 } else { self.ring().len() };
        (avail, out.len().saturating_sub(BLK_HDR).min(9999))
    }

    /// `#4nnnn` for a payload of `n` bytes; the reply's length.
    fn blk_head(out: &mut [u8], n: usize) -> usize {
        out[0] = b'#';
        out[1] = b'4';
        for (i, d) in [1000usize, 100, 10, 1].iter().enumerate() {
            out[2 + i] = b'0' + ((n / d) % 10) as u8;
        }
        BLK_HDR + n
    }

    /// The compact reply header (docs: logger.rs "Compact LOG:DATA?").
    fn fmt_head(&self, o: &mut [u8], magic: &[u8; 2], p: &FmtPar) {
        o[0..2].copy_from_slice(magic);
        o[2] = FMT_VER;
        o[3] = self.ad2 as u8;
        o[4..8].copy_from_slice(&p.rate_mhz.to_le_bytes());
        o[8..12].copy_from_slice(&p.a.to_le_bytes());
        o[12..16].copy_from_slice(&p.b.to_le_bytes());
        o[16..20].fill(0); // records left: at the end
    }

    /// A block header at `o`: first seq, its stamp, count (patched),
    /// sample width, flags and the conversions lost before the first one.
    fn blk_hdr(&self, o: &mut [u8], k: u32, ts: u32, width: u8, pps: u64) {
        let (lost, flags) = if self.drained {
            let d = ts.wrapping_sub(self.drained_ts) as u64 * 1_000_000;
            let periods = (d + pps / 2) / pps.max(1);
            let lost = periods.saturating_sub(1).min(u32::MAX as u64) as u32;
            (lost, if lost > 0 { FL_GAP } else { 0 })
        } else {
            (0, FL_START)
        };
        o[0..4].copy_from_slice(&k.wrapping_add(1).to_le_bytes());
        o[4..8].copy_from_slice(&ts.to_le_bytes());
        o[8..10].fill(0);
        o[10] = width;
        o[11] = flags;
        o[12..16].copy_from_slice(&lost.to_le_bytes());
    }

    /// LOG:FORMat PACKed: blocks of fixed-width samples on the time grid
    /// t0 + i / rate. A sample more than half a period off that grid (a
    /// gap: a stall, a late ISR entry, a torn read, the full ring; or the
    /// clocks' drift adding up) starts a new block with its own seq and
    /// stamp, and `lost_before` counts the conversions in the gap.
    #[inline(never)]
    pub fn encode_packed(&mut self, out: &mut [u8], p: &FmtPar) -> usize {
        let (avail, cap) = self.reply_room(out);
        if avail == 0 || cap < FMT_HDR + PK_BLK + 4 {
            return Self::blk_head(out, 0);
        }
        let pps = PS_PER_S_MHZ / p.rate_mhz.max(1) as u64;
        let half = (pps / 2) as i64;
        let body = &mut out[BLK_HDR..BLK_HDR + cap];
        self.fmt_head(body, b"LP", p);
        let t = self.ring().tail.load(Relaxed);
        let mut o = FMT_HDR;
        let mut bh = 0usize; // the open block's header offset
        let mut n = 0u32; // its samples
        let mut width = 0u8;
        let mut acc = 0i64; // stamp - grid, ps
        let mut prev = 0u32;
        let mut i = 0u32;
        while i < avail {
            let k = t.wrapping_add(i);
            let r = self.rec(k);
            let w = if r.raw == (r.raw << 8) >> 8 { 3u8 } else { 4 };
            let cont = n > 0 && w == width && n < 0xFFFF && {
                acc += r.ts_us.wrapping_sub(prev) as i32 as i64 * 1_000_000 - pps as i64;
                acc.abs() <= half
            };
            if !cont {
                if o + PK_BLK + w as usize > cap {
                    break;
                }
                if n > 0 {
                    body[bh + 8..bh + 10].copy_from_slice(&(n as u16).to_le_bytes());
                }
                self.blk_hdr(&mut body[o..o + PK_BLK], k, r.ts_us, w, pps);
                bh = o;
                o += PK_BLK;
                n = 0;
                width = w;
                acc = 0;
            } else if o + w as usize > cap {
                break;
            }
            body[o..o + w as usize].copy_from_slice(&r.raw.to_le_bytes()[..w as usize]);
            o += w as usize;
            n += 1;
            prev = r.ts_us;
            self.drained_ts = r.ts_us;
            self.drained = true;
            i += 1;
        }
        if n > 0 {
            body[bh + 8..bh + 10].copy_from_slice(&(n as u16).to_le_bytes());
        }
        self.ring().tail.store(t.wrapping_add(i), Release);
        body[16..20].copy_from_slice(&self.ring().len().to_le_bytes());
        Self::blk_head(out, o)
    }

    /// LOG:FORMat DLG: the records as kind-3 chunks of the disk fast stream
    /// (fastenc.rs `FastEnc`, docs/compact-log-format.md "Fast stream
    /// (kind 3)"), coded into the reply buffer: every stamp and value
    /// exact. One block header (width 0, the count, the first seq / stamp,
    /// `lost_before`), then the chunks (row0 = the first record's seq - 1).
    #[inline(never)]
    pub fn encode_dlg(&mut self, out: &mut [u8], p: &FmtPar) -> usize {
        let (avail, cap) = self.reply_room(out);
        let room = cap.saturating_sub(FMT_HDR + PK_BLK);
        // The coder's FIFO is a power of two (`wr % len`): 4096 of 6144.
        let fl = if room >= DLG_FIFO { DLG_FIFO } else { 0 };
        if avail == 0 || fl == 0 {
            return Self::blk_head(out, 0);
        }
        let pps = PS_PER_S_MHZ / p.rate_mhz.max(1) as u64;
        let body = &mut out[BLK_HDR..BLK_HDR + cap];
        self.fmt_head(body, b"LD", p);
        let t = self.ring().tail.load(Relaxed);
        let r0 = self.rec(t);
        self.blk_hdr(&mut body[FMT_HDR..FMT_HDR + PK_BLK], t, r0.ts_us, 0, pps);
        let p_us = ((pps + 500_000) / 1_000_000) as u32;
        let fifo = &mut body[FMT_HDR + PK_BLK..FMT_HDR + PK_BLK + fl];
        let mut e = FastEnc::new(&[(2, 1)], 0, fl as u32, fl, p_us);
        e.rows = t; // chunk row0 = seq - 1
        let mut i = 0u32;
        while i < avail && e.room(1) {
            let r = self.rec(t.wrapping_add(i));
            e.add(fifo, r.ts_us, r.raw);
            self.drained_ts = r.ts_us;
            i += 1;
        }
        e.close(fifo, false);
        self.drained = true;
        let h = FMT_HDR;
        body[h + 8..h + 10].copy_from_slice(&(i.min(0xFFFF) as u16).to_le_bytes());
        self.ring().tail.store(t.wrapping_add(i), Release);
        body[16..20].copy_from_slice(&self.ring().len().to_le_bytes());
        Self::blk_head(out, FMT_HDR + PK_BLK + e.wr as usize)
    }
}

/// The stream ISR (feature `ad2-isr`; AD2, and AD1 unchopped at 160 .. 640 S/s with
/// R00..R02 / AD1F in place of R03..R05 / AD2F, CPU frames at /16, the
/// timer at 20 ns << KSH per count; ../docs/history/decisions-2026-10-22.md #3,
/// docs/dmm-input-scope-feasibility.md 10): TIMER6 (STM32 TIM7, IRQ 55, the
/// only interrupt opendm40 ever enables), phase-locked to the HY3131's
/// conversion grid by src/ad2pll.rs. Entry A, once per conversion period,
/// half way between two conversion edges: R03..R05 blind (one 3-byte SPI2
/// frame, the flag is not needed: the register holds the result for a whole
/// period), the sign-copy check, the ring. Every 4th period an entry B where
/// the edge should be: one INTF read (clears AD2F); the next A reads INTF
/// once more after its data frame, and AD2F there says on which side of B the
/// edge came (`Pll::result`). Reading INTF clears AD2F and loses a
/// conversion that completes during the frame (device c50425f: the old 4 x
/// tick lost ~1.4 % in bursts while its read phase sat on the edge): here
/// that race can only blur a probe, never cost a sample.
///
/// Stamps: the DWT time of entry A (decision #2, the observation time), one
/// period apart to within the entry latency. A late A (latency above
/// `Pll::late_max`, where the frame could reach the next edge) skips its read
/// (`late`; the host sees a stamp gap of 2 periods); an interval whose end
/// had passed before the ISR set it is stretched and the next one shortened
/// by as much (`slips`), so the grid keeps its phase.
///
/// Bounded: at most two frames with capped status waits (hy3131
/// `spi::wait`), no loops otherwise, no allocation, no float, no flash / LCD
/// / watchdog, never a register write (HY3131) and never a call into dmm.rs.
/// Shared with the main loop: `RING`, plus the statistics words below (each
/// written by the ISR only, read by DBG:ISR?).
#[cfg(feature = "ad2-isr")]
pub mod isr {
    use super::*;
    use crate::ad2pll::{Pll, COUNT_HZ};

    /// The ISR's producer state and the lock (SRAM .sram_bss, all zero at
    /// reset: off the TCM stack's budget). Written by `start` (main loop)
    /// before the hand-off, by the ISR alone after it; the main loop reads
    /// the Pll's statistics words (DBG:ISR?, u32 loads) only.
    #[cfg_attr(target_os = "none", link_section = ".sram_bss")]
    static mut PROD: Producer = Producer::new();
    /// DWT count at the session's begin: the stamps (`ts_us`) count from
    /// it (`Logger::begin_cyc`; the scope's DMM CH2 aligns by it).
    static mut BEGIN_CYC: u32 = 0;

    pub fn begin_cyc() -> u32 {
        unsafe { *core::ptr::addr_of!(BEGIN_CYC) }
    }
    #[cfg_attr(target_os = "none", link_section = ".sram_bss")]
    static mut PLL: Pll = Pll::idle();
    static mut RINGP: *const Ring = core::ptr::null();
    /// The session's source: AD1 (R00..R02, INTF.AD1F; CPU frames) or AD2
    /// (R03..R05, AD2F), and the timer's count scale: one count = 20 ns <<
    /// KSH (PSC 4 << KSH - 1), so that an AD1 period (1.56 .. 6.25 ms) fits
    /// the 16-bit ARR; the Pll runs in those counts (`Pll::new` at the rate
    /// << KSH), the statistics (`max_lat`, `lock_stats`) in 20 ns / mHz.
    #[cfg_attr(target_os = "none", link_section = ".sram_bss")]
    static mut AD1: bool = false;
    #[cfg_attr(target_os = "none", link_section = ".sram_bss")]
    static mut KSH: u32 = 0;
    /// The last session's frequency word offset (Pll::rel_q24): the next
    /// session starts on it, so it locks at once (AD1 and AD2 share it: both
    /// rates derive from the chip's crystal).
    #[cfg_attr(target_os = "none", link_section = ".sram_bss")]
    static mut REL: i32 = 0;
    /// The same for the 76.8 kS/s hybrid rung alone (`Pll::carry`): its
    /// lock may be off the crystal's by the rung's own offset (ad2pll.rs
    /// MAX_PPM), which must not detune the other rungs' start.
    #[cfg_attr(target_os = "none", link_section = ".sram_bss")]
    static mut REL_HYB: i32 = 0;
    /// Data frame (CS low .. CS high) + ISR entry, timer counts, per divider
    /// (device SELF? 8.6: us_r6 13.4 / 21.8 at /8 / 16, ~1.3 / 2.8 us per byte).
    const fn frame_counts(div: u8) -> u32 {
        match div {
            8 => 400,   // 8 us
            16 => 700,  // 14 us
            _ => 1300,  // 26 us
        }
    }
    /// The DMA frame in flight (hy3131 `stream::dma_*`): started by A, ended
    /// by the next entry. `on`: this session uses DMA frames; `hyb`: hybrid
    /// data frames (76.8 kS/s: the CPU sends the command, DMA the data, at
    /// /8 with the CKPH = 1 data phase; `stream::hybrid_kick`).
    struct Dma {
        on: bool,
        hyb: bool,
        pending: bool,
        /// The frame in flight: DATA (R03..R05, `ts` = A's stamp), CLEAR
        /// (B's INTF read) or PROBE (C's INTF read: the probe result).
        kind: u8,
        /// The RXB half of the DATA frame in flight.
        buf: u8,
        ts: u32,
    }
    const DATA: u8 = 0;
    const CLEAR: u8 = 1;
    const PROBE: u8 = 2;
    #[cfg_attr(target_os = "none", link_section = ".sram_bss")]
    static mut DMA: Dma = Dma { on: false, hyb: false, pending: false, kind: 0, buf: 0, ts: 0 };
    /// The INTF frame: command (R1E, read) + one byte (the read period and
    /// INTF bits 7..1: AD2F = bit 0 of the reply's byte 1); its replies.
    #[cfg_attr(target_os = "none", link_section = ".sram_bss")]
    static mut TXI: [u8; 2] = [0; 2];
    #[cfg_attr(target_os = "none", link_section = ".sram_bss")]
    static mut RXI: [u8; 2] = [0; 2];
    /// The frame's bytes (SRAM: DMA0 cannot reach the TCM): the command
    /// (R03, read) + 4 x 0xFF out (a hybrid frame sends bytes 1..3), the 5
    /// replies in. Two halves: an entry starts the next frame before it
    /// stores the last one (the probe / data frame goes out first, the
    /// sample's ring push and trigger run while it shifts).
    #[cfg_attr(target_os = "none", link_section = ".sram_bss")]
    static mut TXB: [u8; 5] = [0; 5];
    #[cfg_attr(target_os = "none", link_section = ".sram_bss")]
    static mut RXB: [[u8; 5]; 2] = [[0; 5]; 2];
    /// DMA frame (5 bytes + CS / start), timer counts, per divider.
    const fn dma_frame_counts(div: u8) -> u32 {
        match div {
            8 => 360,   // 7.2 us
            16 => 680,  // 13.6 us
            _ => 1320,  // 26.4 us
        }
    }
    /// The hybrid frame's geometry (feasibility 11.12; device estimates from
    /// SELF? us_r1 / us_r6 at /8 + CKPH and the DMA entry cost of 11.11):
    /// A entry to the data frame's last bit (latency 0.25 + finishing the
    /// last frame ~1.0 + CS 0.2 + command 1.3 + pulse / CKPH / DMA start
    /// ~0.6 + 3 x 1.28 us = ~7.3 us); a probe entry to the chip's INTF latch
    /// (~3.2 us; at least 1.64: CS setup + command + one clock); the margin
    /// to an edge. The emulator's ISR runs these in less (~5.8 / 1.9 us):
    /// its A sits ~1.3 us later after the edge, inside the same window.
    const HYB_FRAME: u32 = 370;
    const HYB_DELTA: u32 = 160;
    const HYB_DELTA_MIN: u32 = 82;
    const HYB_MARGIN: u32 = 25;

    /// A probe B whose entry came later than this (counts) is not used.
    const B_LATE: u32 = 100;
    /// The stacked PC of the entry with the largest latency (DBG:ISR?):
    /// where the main loop was when the ISR was held off longest.
    #[cfg_attr(target_os = "none", link_section = ".sram_bss")]
    static LAT_PC: AtomicU32 = AtomicU32::new(0);
    #[cfg_attr(target_os = "none", link_section = ".sram_bss")]
    static SLIPS: AtomicU32 = AtomicU32::new(0);
    /// Cycles spent in the handler (body, from the CNT read to the exit
    /// checks), / 16 (wraps after ~340 s): the CPU share (DBG:ISR?).
    #[cfg_attr(target_os = "none", link_section = ".sram_bss")]
    static BUSY16: AtomicU32 = AtomicU32::new(0);

    /// The ISR's copy of the wave trigger (`trig`): configuration of
    /// generation `gen`, the detector state, the first event at or after
    /// `min` reported.
    struct Trg {
        gen: u32,
        cfg: StreamCfg,
        min: u32,
        st: Stream,
        first: bool,
    }
    static mut TRG: Trg = Trg { gen: 0, cfg: StreamCfg::OFF, min: 0, st: Stream::NEW, first: false };

    /// Per stored sample `idx` (the ring index = the wave record's sample
    /// number) with AD2 code `v`: the wave trigger (trigsoft::Stream on the
    /// record's 10-bit code; generation 0 = off). A few loads and compares;
    /// an event goes to the event ring, the first one at or after the
    /// configured minimum also to `trg_first_*`.
    #[inline(always)]
    unsafe fn trig(ring: &Ring, idx: u32, v: i32) {
        let g = ring.trg_gen.load(Acquire);
        if g == 0 {
            return;
        }
        let t = &mut *core::ptr::addr_of_mut!(TRG);
        if g != t.gen {
            let w = |k: usize| ring.trg_w[k].load(Relaxed);
            t.gen = g;
            t.cfg = StreamCfg { ttype: w(0) as u8, rising: w(0) & 0x100 != 0, l: w(1) as i32, l2: w(2) as i32, wlo: w(3), whi: w(4) };
            t.min = w(5);
            t.st = Stream::NEW;
            t.first = false;
        }
        if t.st.step(&t.cfg, idx, crate::trigsoft::ad2_code10(v)) {
            let n = ring.trg_ev_n.load(Relaxed);
            ring.trg_ev[n as usize % TRG_EV].store(idx, Relaxed);
            ring.trg_ev_n.store(n.wrapping_add(1), Release);
            if !t.first && idx.wrapping_sub(t.min) < 0x8000_0000 {
                t.first = true;
                ring.trg_first_at.store(idx, Relaxed);
                ring.trg_first_gen.store(g, Release);
            }
        }
    }

    /// Main loop, after the session's chip set-up (the last bus access but
    /// the edge search here; the read divider is set): find one conversion
    /// edge by polling INTF (bounded: 3 periods), start the timer so that
    /// the first A comes `P - beta` after it, hand the bus over.
    pub(super) fn start(prod: &Producer, ring: *const Ring, rate_mhz: u32, ad1: bool) {
        let div = hy::stream::div_used(); // Logger::start picked it
        let mut ksh = 0u32;
        while (prod.period_ns / 20) >> ksh > 60_000 {
            ksh += 1;
        }
        let flag = if ad1 { hy::INTF_AD1 } else { hy::INTF_AD2 };
        unsafe {
            *core::ptr::addr_of_mut!(AD1) = ad1;
            *core::ptr::addr_of_mut!(KSH) = ksh;
            (*core::ptr::addr_of_mut!(TRG)).gen = 0;
            let p = &mut *core::ptr::addr_of_mut!(PROD);
            p.begin(prod.period_ns);
            *core::ptr::addr_of_mut!(BEGIN_CYC) = p.last_cyc;
            *core::ptr::addr_of_mut!(RINGP) = ring;
            let pll = &mut *core::ptr::addr_of_mut!(PLL);
            let dma = !ad1 && hy::stream::use_dma();
            let hyb = !ad1 && hy::stream::hybrid(dma);
            *core::ptr::addr_of_mut!(DMA) = Dma { on: dma, hyb, pending: false, kind: DATA, buf: 0, ts: 0 };
            if dma {
                *core::ptr::addr_of_mut!(TXB) = [(hy::R_AD2 << 1) | 1, 0xFF, 0xFF, 0xFF, 0xFF];
                *core::ptr::addr_of_mut!(TXI) = [(hy::R_INTF << 1) | 1, 0xFF];
                let fc = if hyb { HYB_FRAME } else { dma_frame_counts(div) };
                let rel = if hyb { *core::ptr::addr_of!(REL_HYB) } else { *core::ptr::addr_of!(REL) };
                *pll = Pll::new(rate_mhz, rel, fc);
                pll.late_read = true;
                pll.c_off = if hyb { fc } else { fc + 50 };
                pll.np = if hyb { 16 } else { 8 };
                pll.gate = true;
                if hyb {
                    pll.hybrid(HYB_DELTA, HYB_DELTA_MIN, HYB_MARGIN);
                }
                hy::stream::dma_setup(true);
            } else {
                *pll = Pll::new(rate_mhz << ksh, *core::ptr::addr_of!(REL), frame_counts(div) >> ksh);
            }
        }
        LAT_PC.store(0, Relaxed);
        SLIPS.store(0, Relaxed);
        BUSY16.store(0, Relaxed);
        let pll = unsafe { &*core::ptr::addr_of!(PLL) };
        let pc = pll.f >> 16; // period, counts
        // Edge search: AD2F seen by read j and not by read j - 1 = the edge
        // between their sampling points (~ the start + the command byte +
        // the read period; CS setup .. the INTF bit ~ 2.5 us at /16).
        let cyc_per_count = (crate::clock::HCLK_HZ / COUNT_HZ) << ksh; // 4 << KSH
        let mut f = [0u8];
        let _ = hy::isr_read(hy::R_INTF, &mut f);
        let t_begin = unsafe { r32(DWT_CYCCNT) };
        let mut prev = t_begin;
        let mut edge: Option<u32> = None;
        loop {
            let t = unsafe { r32(DWT_CYCCNT) };
            if !hy::isr_read(hy::R_INTF, &mut f) {
                break;
            }
            if f[0] & flag != 0 {
                edge = Some(prev.wrapping_add(t.wrapping_sub(prev) / 2));
                break;
            }
            prev = t;
            // 3 periods (AD2); AD1: 8 (its first result after the R22
            // write may take longer: device 7674c85 found no edge in 3).
            if t.wrapping_sub(t_begin) > if ad1 { 8 } else { 3 } * pc * cyc_per_count {
                break;
            }
        }
        // First A: P - beta after the edge (the next edge then beta after
        // A; hybrid: beta + the probe's latch delay); without an edge
        // anywhere (the loop finds it, ad2pll tests).
        let hyb = unsafe { (*core::ptr::addr_of!(DMA)).hyb };
        let lead = pll.lead(if hyb { HYB_DELTA } else { 0 }) * cyc_per_count;
        let now = unsafe { r32(DWT_CYCCNT) };
        let first = match edge {
            Some(e) => {
                let mut due = e.wrapping_add(lead);
                while due.wrapping_sub(now) as i32 <= (50 * cyc_per_count) as i32 {
                    due = due.wrapping_add(pc * cyc_per_count);
                }
                due.wrapping_sub(now) / cyc_per_count
            }
            None => pc,
        };
        if ad1 && edge.is_none() {
            // No edge seen: blind reads only once locked (a read across the
            // update would tear the 24-bit result; AD1 has no sign copy).
            unsafe { (*core::ptr::addr_of_mut!(PLL)).gate = true };
        }
        hy::stream::start(first.saturating_sub(4), (4 << ksh) - 1);
    }

    /// Main loop, after `hy::stream::stop`: keep the lock for the next one.
    pub(super) fn stopped() {
        unsafe {
            let pll = &*core::ptr::addr_of!(PLL);
            if (*core::ptr::addr_of!(DMA)).hyb {
                if let Some(r) = pll.carry(true) {
                    *core::ptr::addr_of_mut!(REL_HYB) = r;
                }
            } else if let Some(r) = pll.carry(false) {
                *core::ptr::addr_of_mut!(REL) = r;
            }
        }
    }

    /// The lock's numbers for DBG:ISR?: (probes, edges after B, max run,
    /// locked, locked rate mHz, period offset ppm x 10 vs the nominal rung
    /// (+ = the chip's conversions come later than nominal in MCU time),
    /// slips, the PC of the worst entry latency, conversion periods, the
    /// handler's CPU share in 1/1000 of the session's periods - the
    /// exception entry and exit, ~30 cycles each, not included - and whether
    /// the session uses DMA frames).
    /// The Pll is locked (`lock_stats()[3]` without the rest: per pass).
    #[inline(always)]
    pub fn locked() -> bool {
        unsafe { (*core::ptr::addr_of!(PLL)).locked }
    }

    /// Cycles in the handler body so far (BUSY16 x 16, wrapping).
    #[inline(always)]
    pub fn busy_cyc() -> u32 {
        BUSY16.load(Relaxed) << 4
    }

    pub fn lock_stats() -> [u32; 11] {
        let p = unsafe { &*core::ptr::addr_of!(PLL) };
        let ksh = unsafe { *core::ptr::addr_of!(KSH) };
        let ppm10 = ((p.f_mean() as i64 - p.f_nom as i64) * 10_000_000 / (p.f_nom.max(1) as i64)) as i32;
        [
            p.probes,
            p.late_edges,
            p.max_run,
            p.locked as u32,
            p.rate_mhz() >> ksh,
            ppm10 as u32,
            SLIPS.load(Relaxed),
            LAT_PC.load(Relaxed),
            // A DMA data frame still in flight (the last one of a stopped
            // session is never stored) is not a lost conversion.
            p.periods.wrapping_sub(unsafe {
                let d = &*core::ptr::addr_of!(DMA);
                (d.pending && d.kind == DATA) as u32
            }),
            {
                let busy = BUSY16.load(Relaxed) as u64 * 16;
                let total = p.periods as u64 * (p.f_mean() as u64 >> 16) * ((crate::clock::HCLK_HZ / COUNT_HZ) << ksh) as u64;
                if total == 0 { 0 } else { (busy * 1000 / total) as u32 }
            },
            unsafe { (*core::ptr::addr_of!(DMA)).on } as u32,
        ]
    }

    /// IRQ 55 through the shim's `ad2_irq_entry` (src/shim.rs), which passes
    /// the exception frame (MSP at entry: r0 r1 r2 r3 r12 lr pc xpsr).
    #[no_mangle]
    pub unsafe extern "C" fn ad2_stream_irq(frame: *const u32) {
        let lat = hy::stream::ack();
        if !hy::streaming() || RINGP.is_null() {
            hy::stream::disarm();
            return;
        }
        let t0 = r32(DWT_CYCCNT);
        let ad1 = AD1;
        let flag = if ad1 { hy::INTF_AD1 } else { hy::INTF_AD2 };
        let ring = &*RINGP;
        let pll = &mut *core::ptr::addr_of_mut!(PLL);
        let p = &mut *core::ptr::addr_of_mut!(PROD);
        let d = &mut *core::ptr::addr_of_mut!(DMA);
        if pll.next_b || pll.next_c {
            let c = pll.next_c;
            // A late B drops its probe; a late C too when its read could
            // come after the next edge (the hybrid geometry; c_late = MAX
            // otherwise: the C of a /16 session always reads, as before).
            let ok = lat <= B_LATE && (!c || lat <= pll.c_late);
            let iv = if c { pll.on_c() } else { pll.on_b(ok) };
            let over = hy::stream::set_interval(iv, lat);
            if over != 0 {
                pll.overshoot(over);
                inc(&SLIPS);
            }
            // The last frame (A's data: B and C follow an A) ends; the probe
            // frame starts at once, the sample is stored while it shifts.
            let prev = if d.pending { dma_end(d, ring, pll) } else { None };
            if d.on && (if c { lat <= pll.c_late } else { ok }) {
                // The INTF read as a DMA frame (2 bytes: 5.1 us at /16 - a CPU
                // frame, ~8 us, overran the B -> A interval at 38.4 kS/s; at
                // /8 mode 0 in a hybrid session: 2.6 us, a marginal DO bit
                // only blurs the probe).
                d.kind = if c { PROBE } else { CLEAR };
                d.pending = true;
                hy::stream::dma_kick(core::ptr::addr_of!(TXI) as *const u8, core::ptr::addr_of_mut!(RXI) as *mut u8, 2);
            } else if !d.on && (c || ok) {
                let mut f = [0u8];
                if !hy::isr_read(hy::R_INTF, &mut f) {
                    inc(&ring.timeouts);
                } else if c {
                    pll.result(f[0] & flag != 0);
                    inc(&ring.probes);
                }
            }
            if let Some((buf, ts)) = prev {
                dma_store(p, ring, d.hyb, buf, ts);
            }
        } else {
            let (iv, probe) = pll.on_a();
            let over = hy::stream::set_interval(iv, lat);
            if over != 0 {
                pll.overshoot(over);
                inc(&SLIPS);
            }
            p.tick_at(t0);
            // The last frame ends (a probe's result is taken now); its data,
            // if any, is stored after this A's frame started.
            let prev = if d.pending { dma_end(d, ring, pll) } else { None };
            if !pll.take() {
                // Acquiring (DMA frames, or AD1 without an edge from the
                // search: until locked): no sample. A CPU-frame probe still
                // completes here (else a gated CPU stream never locks:
                // device 7674c85, AD1 640 S/s, probes 0 for 700 s).
                if probe && !d.on {
                    let mut f = [0u8];
                    if hy::isr_read(hy::R_INTF, &mut f) {
                        pll.result(f[0] & flag != 0);
                        inc(&ring.probes);
                    } else {
                        inc(&ring.timeouts);
                    }
                }
            } else if lat > pll.late_max() {
                inc(&ring.late);
            } else if d.on {
                d.ts = p.now_us as u32;
                d.kind = DATA;
                d.pending = true;
                d.buf ^= 1;
                let tx = core::ptr::addr_of!(TXB) as *const u8;
                let rx = core::ptr::addr_of_mut!(RXB[d.buf as usize]) as *mut u8;
                if !d.hyb {
                    hy::stream::dma_kick(tx, rx, 5);
                } else if !hy::stream::hybrid_kick(*tx, tx.add(1), rx, 3) {
                    d.pending = false;
                    inc(&ring.timeouts);
                }
            } else {
                let ts = p.now_us as u32;
                let mut b = [0u8; 3];
                if !hy::isr_read(if ad1 { hy::R_AD1 } else { hy::R_AD2 }, &mut b) {
                    inc(&ring.timeouts);
                } else if ad1 {
                    // 24-bit two's complement, no sign copy to check.
                    let _ = p.push(ring, ts, hy::s24(&b));
                } else {
                    store(p, ring, ts, hy::s24(&b));
                }
                if probe {
                    let mut f = [0u8];
                    if hy::isr_read(hy::R_INTF, &mut f) {
                        pll.result(f[0] & flag != 0);
                        inc(&ring.probes);
                    } else {
                        inc(&ring.timeouts);
                    }
                }
            }
            if let Some((buf, ts)) = prev {
                dma_store(p, ring, d.hyb, buf, ts);
            }
        }
        inc(&ring.ticks);
        let dt = r32(DWT_CYCCNT).wrapping_sub(t0);
        BUSY16.store(BUSY16.load(Relaxed).wrapping_add(dt >> 4), Relaxed);
        if dt > ring.max_isr_cyc.load(Relaxed) {
            ring.max_isr_cyc.store(dt, Relaxed);
        }
        let lat20 = lat << KSH; // 20 ns units
        if lat20 > ring.max_lat.load(Relaxed) {
            ring.max_lat.store(lat20, Relaxed);
            LAT_PC.store(*frame.add(6), Relaxed);
        }
        if hy::stream::overrun() {
            inc(&ring.overruns);
        }
    }

    /// A sample into the ring (sign-copy check) and the wave trigger.
    #[inline(always)]
    unsafe fn store(p: &mut Producer, ring: &Ring, ts: u32, v: i32) {
        if hy::ad2_sign_ok(v) {
            let idx = ring.head.load(Relaxed);
            if p.push(ring, ts, v) {
                trig(ring, idx, v);
            }
        } else {
            inc(&ring.bad);
        }
    }

    /// The DMA frame started at the last entry is over (this entry): CS up
    /// (and CKPH back to 0 after a hybrid frame), a probe's result taken.
    /// A data frame's (RXB half, stamp) is returned: the caller stores it
    /// after it started its own frame (`dma_store`).
    #[inline(always)]
    unsafe fn dma_end(d: &mut Dma, ring: &Ring, pll: &mut Pll) -> Option<(u8, u32)> {
        d.pending = false;
        if !hy::stream::dma_finish() {
            inc(&ring.timeouts);
            return None;
        }
        match d.kind {
            DATA => Some((d.buf, d.ts)),
            PROBE => {
                let rx = &*core::ptr::addr_of!(RXI);
                pll.result(rx[1] & 1 != 0); // AD2F (INTF bit 1) one bit late
                inc(&ring.probes);
                None
            }
            _ => None,
        }
    }

    /// A data frame's sample: R03..R05 as received (hybrid: no shift) or
    /// from the shift framing, into the ring with A's stamp.
    #[inline(always)]
    unsafe fn dma_store(p: &mut Producer, ring: &Ring, hyb: bool, buf: u8, ts: u32) {
        let rx = &*core::ptr::addr_of!(RXB[buf as usize]);
        let v = if hyb { hy::s24(&rx[..3]) } else { decode_shift(rx) };
        store(p, ring, ts, v);
    }
}

/// R03..R05 from a shift-framed 5-byte reply (hy3131 `spi::dma_kick`): byte
/// 0 answers the command; bytes 1..4 carry the read-period bit, then R03,
/// R04, R05 (each MSB first), then 7 bits of R06.
pub fn decode_shift(rx: &[u8; 5]) -> i32 {
    let w = u32::from_be_bytes([rx[1], rx[2], rx[3], rx[4]]);
    let b = [(w >> 23) as u8, (w >> 15) as u8, (w >> 7) as u8];
    hy::s24(&b)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(ad2: bool, period_ns: u32) -> std::boxed::Box<Logger> {
        let ring: &'static Ring = std::boxed::Box::leak(std::boxed::Box::new(Ring::new()));
        let mut l = std::boxed::Box::new(Logger::new());
        l.ring = ring;
        l.ad2 = ad2;
        l.prod.period_ns = period_ns;
        l.prod.nominal_us = (period_ns + 500) / 1000;
        l.nom_mhz = (1_000_000_000_000u64 / period_ns as u64) as u32;
        l.active = true;
        l
    }

    fn feed(l: &mut Logger, ts: u64, raw: i32) {
        l.prod.now_us = ts;
        let ring: &Ring = unsafe { &*l.ring };
        l.prod.push(ring, ts as u32, raw);
    }

    fn recs(l: &mut Logger) -> std::vec::Vec<(u32, u32, i32)> {
        let mut b = std::vec![0u8; CAP * REC_BYTES];
        let n = l.drain(&mut b);
        b[..n]
            .chunks(12)
            .map(|c| {
                let w = |i: usize| u32::from_le_bytes([c[i], c[i + 1], c[i + 2], c[i + 3]]);
                (w(0), w(4), w(8) as i32)
            })
            .collect()
    }

    #[test]
    fn full_ring_drops_the_newest_and_seqs_stay_consecutive() {
        let mut l = session(true, 104_167);
        for i in 0..(CAP as u64 + 10) {
            feed(&mut l, 1000 + i * 104, i as i32 - 5);
        }
        assert_eq!(l.overwritten(), 10);
        assert_eq!(l.captured(), CAP as u32 + 10);
        assert_eq!(l.buffered(), CAP as u32);
        let r = recs(&mut l);
        assert_eq!(r.len(), CAP);
        assert_eq!(r[0].0, 1); // the oldest kept; the 10 newest were dropped
        assert_eq!(r[0].2, -5);
        assert!(r.windows(2).all(|w| w[1].0 == w[0].0 + 1));
        assert_eq!(r.last().unwrap().0, CAP as u32);
        assert_eq!(l.buffered(), 0);
        // After the drain the ring takes samples again; the gap shows in
        // the timestamps, the seqs go on.
        feed(&mut l, 1_000_000, -7);
        feed(&mut l, 1_000_104, -8);
        let mut b = [0u8; 12];
        assert_eq!(l.drain(&mut b), 12);
        assert_eq!(u32::from_le_bytes([b[0], b[1], b[2], b[3]]), CAP as u32 + 1);
        assert_eq!(i32::from_le_bytes([b[8], b[9], b[10], b[11]]), -7);
        assert_eq!(recs(&mut l)[0].0, CAP as u32 + 2);
    }

    #[test]
    fn ring_wraps_many_times() {
        let mut l = session(true, 104_167);
        let mut next = 1u32;
        for round in 0..20u64 {
            for i in 0..1500u64 {
                feed(&mut l, (round * 1500 + i) * 104, (round * 1500 + i) as i32);
            }
            let r = recs(&mut l);
            assert_eq!(r.len(), 1500);
            for (k, x) in r.iter().enumerate() {
                assert_eq!(x.0, next + k as u32);
                assert_eq!(x.2 as u32, next + k as u32 - 1);
            }
            next += 1500;
        }
        assert_eq!(l.overwritten(), 0);
    }

    #[test]
    fn lost_and_gaps_from_the_timestamps() {
        // 9600/s: 104.167 us. Samples every 125 us (a loop that cannot keep
        // up): no single gap > 1.5 P, but 1 in 6 conversions is lost.
        let mut l = session(true, 104_167);
        for i in 0..6001u64 {
            feed(&mut l, i * 125, 0);
        }
        assert_eq!(l.missed(), 0);
        let due = (6000 * 125_000 + 52_083) / 104_167 + 1; // 7201
        assert_eq!(l.lost(), due as u32 - 6001);
        assert_eq!(l.measured_hz(), 8000);
        // One 10 ms stall: ~96 periods -> bucket 17..128.
        let mut l = session(true, 104_167);
        feed(&mut l, 0, 0);
        feed(&mut l, 104, 0);
        feed(&mut l, 10_104, 0);
        feed(&mut l, 10_208, 0);
        assert_eq!(l.gaps(), [0, 0, 0, 1, 0]);
        assert_eq!(l.max_gap_us(), 10_000);
        assert_eq!(l.missed(), 95);
        assert_eq!(l.lost(), 98 - 4 + 1); // 10208 us / 104.167 = 98 periods
        // A regular stream loses nothing.
        let mut l = session(false, 25_000_000);
        for i in 0..100u64 {
            feed(&mut l, i * 25_000, 0);
        }
        assert_eq!((l.lost(), l.missed(), l.gaps()), (0, 0, [0; GAP_BUCKETS]));
        // AD1 at a chip period of 26.3 ms against the nominal 25 ms (device
        // c28c91c, OSR 2048 chopped): nothing lost by the measured mean
        // (the nominal would say ~5 of 100), one real hole still counts.
        let mut l = session(false, 25_000_000);
        for i in 0..100u64 {
            feed(&mut l, i * 26_300, 0);
            if i > 0 {
                l.prod.note_period(26_300 + if i % 2 == 0 { 900 } else { 0 } - 450);
            }
        }
        assert_eq!(l.lost(), 0);
        feed(&mut l, 101 * 26_300, 0);
        assert_eq!(l.lost(), 1);
        // Too few intervals: the nominal.
        let mut l = session(false, 25_000_000);
        for i in 0..100u64 {
            feed(&mut l, i * 26_300, 0);
        }
        l.prod.note_period(26_300);
        assert_eq!(l.lost(), (99 * 26_300 + 12_500) / 25_000 + 1 - 100);
        // A span past 2^32 us (71.6 min) keeps the 64-bit maths.
        let mut l = session(false, 25_000_000);
        feed(&mut l, 0, 0);
        feed(&mut l, 5_000_000_000, 0);
        assert_eq!(l.ring().span_us(), 5_000_000_000);
    }

    // ------------------------------------------------ compact LOG:DATA?
    use crate::fastenc;
    // A decoder of both compact replies (as tools/dm40scpi.py's): rows of
    // (seq, t_us reconstructed, raw, lost_before), the records left.

    struct Bits<'a> {
        d: &'a [u8],
        pos: usize,
        end: usize,
    }
    impl Bits<'_> {
        fn get(&mut self, n: u32) -> u32 {
            let mut v = 0u64;
            for _ in 0..n {
                assert!(self.pos < self.end * 8, "bit stream overrun");
                v = v << 1 | ((self.d[self.pos >> 3] >> (7 - (self.pos & 7))) & 1) as u64;
                self.pos += 1;
            }
            v as u32
        }
        fn rice(&mut self, k: u32) -> u32 {
            let mut q = 0;
            while q < 24 && self.get(1) == 1 {
                q += 1;
            }
            if q == 24 { self.get(32) } else { q << k | self.get(k) }
        }
    }
    fn unzz(u: u32) -> i32 {
        (u >> 1) as i32 ^ -((u & 1) as i32)
    }
    fn le32(d: &[u8], o: usize) -> u32 {
        u32::from_le_bytes([d[o], d[o + 1], d[o + 2], d[o + 3]])
    }

    type Row = (u32, u64, i32, u32);

    /// Decode one reply; `rows` gets (seq, t_us (u64, of the u32 stamps;
    /// PACKed: t0 + round(i / rate)), raw, lost_before). Returns the
    /// records-left field and the block count.
    fn decode(reply: &[u8], rows: &mut std::vec::Vec<Row>) -> (u32, usize) {
        assert_eq!(&reply[..2], b"#4");
        let n: usize = core::str::from_utf8(&reply[2..6]).unwrap().parse().unwrap();
        assert_eq!(reply.len(), 6 + n);
        if n == 0 {
            return (0, 0);
        }
        let d = &reply[6..];
        assert_eq!(d[2], FMT_VER);
        let rate = le32(d, 4) as u64;
        let left = le32(d, 16);
        let mut o = FMT_HDR;
        let mut blocks = 0;
        if &d[..2] == b"LP" {
            while o < d.len() {
                let (seq, t0, cnt, w, lost) =
                    (le32(d, o), le32(d, o + 4) as u64, u16::from_le_bytes([d[o + 8], d[o + 9]]) as usize, d[o + 10] as usize, le32(d, o + 12));
                assert!(w == 3 || w == 4);
                assert!(cnt > 0);
                o += PK_BLK;
                for i in 0..cnt {
                    let mut b = [0u8; 4];
                    b[..w].copy_from_slice(&d[o..o + w]);
                    let raw = if w == 3 { (i32::from_le_bytes(b) << 8) >> 8 } else { i32::from_le_bytes(b) };
                    let t = t0 + (i as u64 * 1_000_000_000 + rate * 500 / 1000) / rate; // us, rounded
                    rows.push((seq + i as u32, t, raw, if i == 0 { lost } else { 0 }));
                    o += w;
                }
                blocks += 1;
            }
            return (left, blocks);
        }
        assert_eq!(&d[..2], b"LD");
        let (seq0, cnt, lost0) = (le32(d, o), u16::from_le_bytes([d[o + 8], d[o + 9]]) as usize, le32(d, o + 12));
        assert_eq!(d[o + 10], 0);
        o += PK_BLK;
        let first = rows.len();
        while o < d.len() {
            assert_eq!(&d[o..o + 2], b"DC");
            let pl = u16::from_le_bytes([d[o + 2], d[o + 3]]) as usize;
            let row0 = le32(d, o + 8);
            let mut t = le32(d, o + 12) as u64 | (u16::from_le_bytes([d[o + 16], d[o + 17]]) as u64) << 32;
            let nr = u16::from_le_bytes([d[o + 18], d[o + 19]]) as usize;
            let dt0 = le32(d, o + 20) as u64;
            let mut v = le32(d, o + 24) as i32;
            let crc = !fastenc::crc32_upd(fastenc::crc32_upd(!0, &d[o + CHDR..o + CHDR + pl]), &d[o + 8..o + CHDR]);
            assert_eq!(crc, le32(d, o + 4), "chunk CRC");
            let lost_of = |dt: u64| ((dt * rate + 500_000_000) / 1_000_000_000).saturating_sub(1) as u32;
            let mut dt = ((1_000_000_000 + rate / 2) / rate) as u32;
            let p_us = dt;
            let _ = p_us;
            rows.push((row0 + 1, t, v, if rows.len() == first { lost0 } else { lost_of(dt0) }));
            let mut bits = Bits { d, pos: (o + CHDR) * 8, end: o + CHDR + pl };
            let (mut at, mut av) = (fastenc::Ad::new(4), fastenc::Ad::new(16));
            for j in 1..nr {
                let x = bits.rice(at.k());
                at.upd(x);
                dt = dt.wrapping_add(unzz(x) as u32);
                t += dt as u64;
                let u = bits.rice(av.k());
                av.upd(u);
                v = v.wrapping_add(unzz(u));
                rows.push((row0 + 1 + j as u32, t, v, lost_of(dt as u64)));
            }
            o += CHDR + pl;
            blocks += 1;
        }
        assert_eq!(rows.len() - first, cnt);
        assert_eq!(rows[first].0, seq0);
        (left, blocks)
    }
    const CHDR: usize = fastenc::CHDR;

    /// Drain `l` in format `fmt` with replies of `buf` bytes until empty;
    /// the rows with their stamps unwrapped (u32 -> u64, as a host does).
    fn drain_fmt(l: &mut Logger, fmt: u8, rate_mhz: u32, buf: usize) -> (std::vec::Vec<Row>, usize) {
        l.set_fmt(fmt);
        let par = FmtPar { rate_mhz, a: 0.5, b: 1e-6 };
        let mut out = std::vec![0u8; buf];
        let mut rows = std::vec::Vec::new();
        let mut replies = 0;
        loop {
            let n = l.encode_fmt(&mut out, &par);
            if n == 6 {
                break;
            }
            let mut r = std::vec::Vec::new();
            decode(&out[..n], &mut r);
            replies += 1;
            rows.extend(r);
        }
        (rows, replies)
    }

    /// Unwrap the stamps as a host does: each decoded t is a u32 t0 plus
    /// an offset; a step back of more than 2^31 us is a wrap.
    fn unwrap(rows: &mut [Row]) {
        let (mut base, mut last) = (0u64, 0u64);
        for r in rows.iter_mut() {
            let mut t = r.1 + base;
            if t + (1 << 31) < last {
                base += 1 << 32;
                t += 1 << 32;
            }
            last = t;
            r.1 = t;
        }
    }

    /// The stamps of a regular stream with a few holes; (stamps, values).
    fn stream(n: u64, p_ns: u64, t0: u64, holes: &[(u64, u64)], vals: &dyn Fn(u64) -> i32) -> std::vec::Vec<(u64, i32, u32)> {
        let mut out = std::vec::Vec::new();
        let mut k = 0u64; // conversion index
        let mut i = 0u64;
        while (out.len() as u64) < n {
            if let Some(&(_, len)) = holes.iter().find(|h| h.0 == i) {
                k += len;
                out.push(((t0 * 1000 + k * p_ns) / 1000, vals(i), len as u32));
            } else {
                out.push(((t0 * 1000 + k * p_ns) / 1000, vals(i), 0));
            }
            k += 1;
            i += 1;
        }
        out
    }

    fn check_fmt(fmt: u8, p_ns: u32, s: &[(u64, i32, u32)], exact_t: bool) -> usize {
        let mut l = session(true, p_ns);
        let rate = (1_000_000_000_000u64 / p_ns as u64) as u32;
        let (mut rows, mut replies) = (std::vec::Vec::new(), 0);
        for part in s.chunks(1500) {
            for &(t, v, _) in part {
                feed(&mut l, t, v);
            }
            let (r, n) = drain_fmt(&mut l, fmt, rate, 6144);
            rows.extend(r);
            replies += n;
        }
        assert_eq!(l.overwritten(), 0);
        unwrap(&mut rows);
        assert_eq!(rows.len(), s.len());
        let base = s[0].0 >> 32 << 32;
        for (i, (r, x)) in rows.iter().zip(s).enumerate() {
            assert_eq!(r.0, i as u32 + 1, "seq");
            assert_eq!(r.2, x.1, "value of row {}", i);
            let t = r.1 + base;
            if exact_t {
                assert_eq!(t, x.0, "stamp of row {}", i);
            } else {
                let half = (p_ns as u64 / 2 + 999) / 1000;
                assert!(t.abs_diff(x.0) <= half + 1, "row {} t {} vs {}", i, t, x.0);
            }
            assert_eq!(r.3, x.2, "lost_before of row {}", i);
        }
        replies
    }

    #[test]
    fn fmt_packed_and_dlg_round_trip() {
        let p = 13_021; // 76.8 kS/s
        let full = |i: u64| match i % 7 {
            // AD2 full scale and jumps (the Rice escape), AD1's i24 limits
            0 => 262_143,
            1 => -262_144,
            2 => 8_388_607,
            3 => -8_388_608,
            _ => (i as i32 * 37) % 1000 - 500,
        };
        let s = stream(5000, p, 1000, &[(10, 1), (777, 5), (2500, 300), (4999, 2)], &full);
        for fmt in [FMT_PACKED, FMT_DLG] {
            check_fmt(fmt, p as u32, &s, fmt == FMT_DLG);
        }
        // Regular, low-noise: DLG ~2 B / sample, PACKed 3.
        let s = stream(2040, p, 5, &[], &|i| ((i * 7919) % 41) as i32 - 20);
        let mut l = session(true, p as u32);
        for &(t, v, _) in &s {
            feed(&mut l, t, v);
        }
        let mut out = std::vec![0u8; 6144];
        let par = FmtPar { rate_mhz: 76_800_000, a: 0.0, b: 0.0 };
        l.set_fmt(FMT_PACKED);
        let n = l.encode_fmt(&mut out, &par);
        let mut r = std::vec::Vec::new();
        let (left, blocks) = decode(&out[..n], &mut r);
        // 6138 payload bytes: 2034 samples, 6 left for the next reply.
        assert_eq!((r.len(), left, blocks), (2034, 6, 1));
        assert_eq!(n, 6 + FMT_HDR + PK_BLK + 2034 * 3);
        // DLG: the rest, then the size of a full ring.
        l.set_fmt(FMT_DLG);
        let n = l.encode_fmt(&mut out, &par);
        let mut r = std::vec::Vec::new();
        assert_eq!(decode(&out[..n], &mut r), (0, 1));
        assert_eq!(r.len(), 6);
        for &(t, v, _) in &s {
            feed(&mut l, t + 1_000_000, v);
        }
        let n = l.encode_fmt(&mut out, &par);
        let mut r = std::vec::Vec::new();
        decode(&out[..n], &mut r);
        assert_eq!(r.len(), 2040);
        std::println!("DLG: {} B for 2040 samples ({:.2} B/sample)", n, n as f32 / 2040.0);
        assert!(n < 2040 * 2);
    }

    #[test]
    fn fmt_u32_stamp_wraps() {
        // The stamps cross 2^32 us inside a reply (and a block).
        let p = 26_042; // 38.4 kS/s
        let t0 = (1u64 << 32) - 20_000;
        let s = stream(3000, p, t0, &[(1500, 3)], &|i| i as i32);
        for fmt in [FMT_PACKED, FMT_DLG] {
            check_fmt(fmt, p as u32, &s, fmt == FMT_DLG);
        }
    }

    #[test]
    fn fmt_drift_and_ad1_realign_without_loss() {
        // AD1 at 26.3 ms against the nominal 25 ms (5 %): PACKed starts a
        // block every ~10 samples, lost 0 throughout.
        let p = 25_000_000u32;
        let mut l = session(false, p);
        let mut s = std::vec::Vec::new();
        for i in 0..200u64 {
            s.push((i * 26_300, (i * 3) as i32 - 100, 0u32));
            feed(&mut l, i * 26_300, (i * 3) as i32 - 100);
        }
        l.set_fmt(FMT_PACKED);
        let par = FmtPar { rate_mhz: 40_000, a: 0.0, b: 0.0 };
        let mut out = std::vec![0u8; 6144];
        let n = l.encode_fmt(&mut out, &par);
        let mut r = std::vec::Vec::new();
        let (_, blocks) = decode(&out[..n], &mut r);
        assert_eq!(r.len(), 200);
        assert!(blocks >= 15 && blocks <= 25, "{} blocks", blocks);
        for (x, y) in r.iter().zip(&s) {
            assert_eq!((x.2, x.3), (y.1, 0));
            assert!(x.1.abs_diff(y.0) <= 12_500);
        }
        // 76.8 kS/s with the chip 100 ppm slow: realigned, nothing lost.
        let s: std::vec::Vec<_> = (0..4000u64).map(|i| (i * 13_022_302 / 1_000_000, 1, 0u32)).collect();
        check_fmt(FMT_PACKED, 13_021, &s, false);
    }

    #[test]
    fn fmt_replies_split_and_continue() {
        // A full ring (2048) in small replies (BT-sized or smaller): each
        // decodes on its own, the seqs continue, the reply after a gap
        // carries lost_before; an i32 that does not fit 3 bytes: width 4.
        let p = 104_167u32; // 9600/s
        let mut s = stream(2048, p as u64, 0, &[(1000, 7)], &|i| i as i32 * 100);
        s[1500].1 = 0x0123_4567; // width 4 for this sample
        for fmt in [FMT_PACKED, FMT_DLG] {
            for buf in [6144usize, 4200, 700] {
                let mut l = session(true, p);
                for &(t, v, _) in &s {
                    feed(&mut l, t, v);
                }
                let (rows, replies) = drain_fmt(&mut l, fmt, 9_600_000, buf);
                assert_eq!(l.overwritten(), 0);
                if fmt == FMT_DLG && buf < 4200 {
                    assert_eq!(rows.len(), 0, "DLG needs the 4 KiB FIFO");
                    continue;
                }
                assert_eq!(rows.len(), 2048);
                assert!(buf == 6144 || fmt == FMT_DLG || replies > 1);
                for (i, (r, x)) in rows.iter().zip(&s).enumerate() {
                    assert_eq!((r.0, r.2, r.3), (i as u32 + 1, x.1, x.2), "fmt {} buf {} row {}", fmt, buf, i);
                }
            }
        }
        // Empty ring: an empty block, as ROWS.
        let mut l = session(true, p);
        let mut out = [0u8; 64];
        for fmt in [FMT_PACKED, FMT_DLG] {
            l.set_fmt(fmt);
            assert_eq!(l.encode_fmt(&mut out, &FmtPar { rate_mhz: 1, a: 0.0, b: 0.0 }), 6);
            assert_eq!(&out[..6], b"#40000");
        }
        // The first block of a session is flagged.
        feed(&mut l, 0, 1);
        l.set_fmt(FMT_PACKED);
        let mut out = [0u8; 256];
        l.encode_fmt(&mut out, &FmtPar { rate_mhz: 9_600_000, a: 0.0, b: 0.0 });
        assert_eq!(out[6 + FMT_HDR + 11], FL_START);
    }

    /// With LOGFMT_OUT=<dir> (tools/logfmt-test/run.sh): the replies of
    /// streams with gaps, full-scale codes and a stamp wrap in both
    /// compact formats, and the rows they must decode to, for the host
    /// decoder (tools/dm40scpi.py `log_reply`).
    #[test]
    fn fmt_dump_for_the_host_decoder() {
        let Ok(dir) = std::env::var("LOGFMT_OUT") else { return };
        use std::io::Write;
        let full = |i: u64| match (i % 1001, i % 11) {
            (4, _) => 0x0123_4567, // width 4
            (_, 0) => 262_143,
            (_, 1) => -262_144,
            (_, 2) => 8_388_607,
            (_, 3) => -8_388_608,
            _ => ((i * 7919) % 2001) as i32 - 1000,
        };
        let sets: [(&str, u32, u64, &[(u64, u64)]); 3] = [
            ("a768", 13_021, 1000, &[(10, 1), (777, 5), (2500, 300), (4999, 2)]),
            ("wrap384", 26_042, (1 << 32) - 20_000, &[(1500, 3)]),
            ("ad1", 1_562_500, 0, &[(100, 1)]),
        ];
        for (name, p, t0, holes) in sets {
            let s = stream(6000, p as u64, t0, holes, &full);
            let rate = (1_000_000_000_000u64 / p as u64) as u32;
            for (fmt, fname) in [(FMT_PACKED, "packed"), (FMT_DLG, "dlg")] {
                let mut l = session(true, p);
                let mut f = std::fs::File::create(std::format!("{}/{}-{}.bin", dir, name, fname)).unwrap();
                l.set_fmt(fmt);
                let par = FmtPar { rate_mhz: rate, a: 0.25, b: 2e-6 };
                let mut out = std::vec![0u8; 6144];
                for part in s.chunks(1700) {
                    for &(t, v, _) in part {
                        feed(&mut l, t, v);
                    }
                    loop {
                        let n = l.encode_fmt(&mut out, &par);
                        f.write_all(&(n as u32).to_le_bytes()).unwrap();
                        f.write_all(&out[..n]).unwrap();
                        if n == 6 {
                            break;
                        }
                    }
                }
                assert_eq!(l.overwritten(), 0);
            }
            let mut f = std::fs::File::create(std::format!("{}/{}.txt", dir, name)).unwrap();
            writeln!(f, "{}", p).unwrap();
            for (i, x) in s.iter().enumerate() {
                writeln!(f, "{} {} {} {}", i + 1, x.0 & 0xFFFF_FFFF, x.1, x.2).unwrap();
            }
        }
    }

    #[test]
    fn fmt_switch_between_replies() {
        // ROWS -> PACKed -> ROWS -> DLG in one session: each compact reply's
        // lost_before counts from the last record of the previous reply,
        // whatever its format.
        let p = 104_167u32;
        let mut l = session(true, p);
        let par = FmtPar { rate_mhz: 9_600_000, a: 0.0, b: 0.0 };
        let mut out = std::vec![0u8; 6144];
        let mut t = 0u64;
        let mut fill = |l: &mut Logger, n: u64, hole: u64| {
            t += hole * 104_167 / 1000;
            for _ in 0..n {
                feed(l, t, 5);
                t += 104;
            }
        };
        fill(&mut l, 100, 0);
        l.set_fmt(FMT_ROWS);
        assert_eq!(l.encode_fmt(&mut out, &par), 6 + 1200);
        fill(&mut l, 100, 4);
        l.set_fmt(FMT_PACKED);
        let n = l.encode_fmt(&mut out, &par);
        let mut r = std::vec::Vec::new();
        decode(&out[..n], &mut r);
        assert_eq!((r[0].0, r[0].3, out[6 + FMT_HDR + 11]), (101, 4, FL_GAP));
        fill(&mut l, 100, 0);
        l.set_fmt(FMT_ROWS);
        l.encode_fmt(&mut out, &par);
        fill(&mut l, 100, 7);
        l.set_fmt(FMT_DLG);
        let n = l.encode_fmt(&mut out, &par);
        let mut r = std::vec::Vec::new();
        decode(&out[..n], &mut r);
        assert_eq!((r[0].0, r[0].3), (301, 7));
        assert!(r[1..].iter().all(|x| x.3 == 0));
    }

    #[test]
    fn producer_clock_counts_cycles() {
        let mut p = Producer::new();
        p.last_cyc = u32::MAX - 99;
        p.tick_at(100); // 200 cycles across the wrap
        assert_eq!((p.now_us, p.acc), (1, 0));
        for _ in 0..3 {
            let c = p.last_cyc.wrapping_add(150);
            p.tick_at(c);
        }
        assert_eq!((p.now_us, p.acc), (3, 50));
        let c = p.last_cyc.wrapping_add(4_000_000_000);
        p.tick_at(c);
        assert_eq!(p.now_us, 3 + 20_000_000);
    }

    #[test]
    fn source_gates_the_modes() {
        assert!(Logger::mode_ok(false, 1) && Logger::mode_ok(false, 9) && !Logger::mode_ok(false, 2));
        assert!(Logger::mode_ok(true, 0) && Logger::mode_ok(true, 2) && Logger::mode_ok(true, 6));
        assert!(Logger::mode_ok(true, 1) && Logger::mode_ok(true, 3) && !Logger::mode_ok(true, 7));
        assert!(!Logger::mode_ok(true, 16) && !Logger::mode_ok(true, 17) && !Logger::mode_ok(true, 10));
        assert_eq!(Logger::ad1_first(1), Some(false));
        assert_eq!(Logger::ad1_first(0), Some(true));
        assert_eq!(Logger::ad1_first(10), None);
        let mut l = Logger::new();
        assert_eq!(l.req_hz(), DEFAULT_AD2_HZ);
        l.set_req_hz(9600);
        assert_eq!(hy::ad2_rate_mhz(l.r26()), 9_600_000);
        l.set_chop(false);
        l.set_req_hz(600);
        assert_eq!(hy::ad2_rate_mhz(l.r26()), 1_200_000);
    }

    /// One nominal per rung (device 2f56ff9): LOG:PREamble? / the CSV and
    /// DLG headers (`nominal_mhz`) equal the stream ISR's rate and
    /// ACQ:SRATe? (`scope::wave_real_fs` from `hy::ad2_real_rate_mhz`), not
    /// 1e12 / the rounded period (76 799 016 at 76.8 kS/s).
    #[test]
    fn nominal_is_the_chip_rate() {
        let mut l = Logger::new();
        l.set_ad2(true);
        l.set_chop(false);
        for hz in [76_800u32, 38_400, 9600, 1200] {
            l.set_req_hz(hz);
            let _ = l.plan(1, 0);
            assert_eq!(l.nominal_mhz(), hz * 1000, "AD2 {hz}");
            assert_eq!(l.nominal_mhz(), hy::ad2_real_rate_mhz(l.r26()), "AD2 {hz}");
        }
        l.set_req_hz(76_800);
        let _ = l.plan(1, 0);
        assert_eq!(l.nominal_mhz(), 76_800_000);
        assert_eq!((l.nominal_mhz() + 500) / 1000, 76_800);
        l.set_ad2(false);
        for code in 0..8u8 {
            for chop in [false, true] {
                l.set_osr(code);
                l.set_chop(chop);
                let _ = l.plan(1, 0);
                assert_eq!(l.nominal_mhz(), hy::ad1_rate_mhz(l.osr, chop), "AD1 {code} {chop}");
            }
        }
        l.set_osr(0);
        l.set_chop(false);
        let _ = l.plan(1, 0);
        assert_eq!(l.nominal_mhz(), 640_000);
    }
}

/// The DMM wave view's side of the ring (main loop; consumer + trigger
/// configuration). The ring is `RING`; the session is a `Logger::start_wave`
/// one, so the stream ISR is the producer.
pub mod wave {
    use super::*;

    /// Pop every buffered sample (oldest first) with its stamp (us since
    /// the session's begin, `Logger::begin_cyc`) into `f`; the count.
    pub fn drain_ts(f: &mut dyn FnMut(u32, i32)) -> u32 {
        let ring = &RING;
        let t = ring.tail.load(Relaxed);
        let n = ring.head.load(Acquire).wrapping_sub(t);
        let base = ring.buf.get() as *const Rec;
        for i in 0..n {
            // SAFETY: below the Acquire-loaded head (Logger::drain).
            let r = unsafe { base.add(t.wrapping_add(i) as usize & (CAP - 1)).read_volatile() };
            f(r.ts_us, r.raw);
        }
        ring.tail.store(t.wrapping_add(n), Release);
        n
    }

    /// Pop every buffered sample (oldest first) into `f`; the count.
    pub fn drain(f: &mut dyn FnMut(i32)) -> u32 {
        let ring = &RING;
        let t = ring.tail.load(Relaxed);
        let n = ring.head.load(Acquire).wrapping_sub(t);
        let base = ring.buf.get() as *const Rec;
        for i in 0..n {
            // SAFETY: below the Acquire-loaded head (Logger::drain).
            let r = unsafe { base.add(t.wrapping_add(i) as usize & (CAP - 1)).read_volatile() };
            f(r.raw);
        }
        ring.tail.store(t.wrapping_add(n), Release);
        n
    }

    /// Holes in the stream so far: samples dropped because the ring was
    /// full, plus conversions the ISR missed (`missed`, from the stamps: a
    /// gap > 1.5 periods; the device check of c50425f found ~11.5 ms bursts
    /// every ~3950 conversions). The wave record restarts after a hole
    /// instead of splicing across it.
    pub fn dropped() -> u32 {
        RING.dropped.load(Relaxed).wrapping_add(RING.missed.load(Relaxed))
    }

    /// New trigger configuration (`min`: the first sample index whose
    /// event `first` reports); returns its generation (never 0).
    pub fn set_trigger(c: &StreamCfg, min: u32) -> u32 {
        let r = &RING;
        let w = [c.ttype as u32 | (c.rising as u32) << 8, c.l as u32, c.l2 as u32, c.wlo, c.whi, min];
        for (a, v) in r.trg_w.iter().zip(w) {
            a.store(v, Relaxed);
        }
        let g = r.trg_gen.load(Relaxed).wrapping_add(1).max(1);
        r.trg_gen.store(g, Release);
        g
    }

    /// No trigger search in the ISR.
    pub fn trigger_off() {
        RING.trg_gen.store(0, Release);
    }

    /// Events since `*seen` (sample indices, oldest first; at most the
    /// TRG_EV - 1 newest: older ones are gone); `*seen` advances.
    pub fn events(seen: &mut u32, f: &mut dyn FnMut(u32)) {
        let r = &RING;
        let n = r.trg_ev_n.load(Acquire);
        let from = if n.wrapping_sub(*seen) > TRG_EV as u32 - 1 { n.wrapping_sub(TRG_EV as u32 - 1) } else { *seen };
        let mut k = from;
        while k != n {
            f(r.trg_ev[k as usize % TRG_EV].load(Relaxed));
            k = k.wrapping_add(1);
        }
        *seen = n;
    }

    /// Event count so far (for `events`).
    pub fn event_count() -> u32 {
        RING.trg_ev_n.load(Acquire)
    }

    /// The first event at or after the minimum of generation `gen`.
    pub fn first(gen: u32) -> Option<u32> {
        let r = &RING;
        (gen != 0 && r.trg_first_gen.load(Acquire) == gen).then(|| r.trg_first_at.load(Relaxed))
    }
}
