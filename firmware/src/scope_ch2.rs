//! The DMM as CH2 of the MCX scope page (docs/scope-gen-channel.md
//! section 2): the HY3131 AD2 stream (the wave view's, src/logger.rs
//! `wave`) kept with its time, so the columns of an MCX record (its DWT
//! time, `Scope::rec_time` / `roll_time`) pick the CH2 samples taken in
//! the same interval.
//!
//! Storage: the last `CAP` samples (raw >> 3, 16 bit) and an anchor
//! (sample index, DWT) every `EVERY` samples. A sample's DWT is the
//! session's begin (`Logger::begin_cyc`) plus 200 x its microsecond stamp
//! (the stream ISR's read; `LAG_US` earlier for the conversion it holds).
//! Between anchors the index is linear in time (the stream is regular
//! between holes); a hole (ring overflow, missed conversions) restarts
//! the store. The anchors and the record's DWT are compared as wrapping
//! i32 differences: both lie within seconds of each other, far inside
//! the counter's 21 s wrap.
//!
//! Main loop only.

use crate::scope::WaveScale;

/// Samples kept: 107 ms at 38.4 kS/s (`app_ch2::CH2_MAX_FS`, 2 ms/div and
/// faster: a 20 ms screen at most), 1.7 s at
/// 2400 S/s (100 ms/div: 1 s), 3.4 s at 1200 S/s (roll from 200 ms/div:
/// a 2 s screen; slower roll screens show their newest 3.4 s).
pub const CAP: usize = 4096;
/// Samples per anchor and anchors kept (covering CAP).
const EVERY: u64 = 128;
const NANCH: usize = CAP / EVERY as usize + 2;

/// Microseconds the conversion a stamp carries is older than the stamp
/// (half the integration plus the AD2 filter): 0 until measured on the
/// device (a step on both inputs; DBG:CH2:LAG).
static LAG_US: core::sync::atomic::AtomicI32 = core::sync::atomic::AtomicI32::new(0);

pub fn lag_us() -> i32 {
    LAG_US.load(core::sync::atomic::Ordering::Relaxed)
}

pub fn set_lag_us(us: i32) {
    LAG_US.store(us.clamp(-100_000, 100_000), core::sync::atomic::Ordering::Relaxed);
}

pub struct Store {
    codes: [i16; CAP],
    /// Samples pushed since the reset (the next sample's index).
    tot: u64,
    anc: [(u64, u32); NANCH],
    /// Anchors written (the newest is `(nanc - 1) % NANCH`).
    nanc: usize,
    /// DWT of the stream session's begin (the stamps' zero).
    c0: u32,
    /// The stream's sample period, DWT cycles (nominal: the extrapolation
    /// with a single anchor).
    period: f32,
    /// Raw -> SI (the DMM row the stream runs on).
    sc: WaveScale,
    /// The stream's hole count seen (`logger::wave::dropped`).
    dropped: u32,
}

#[cfg_attr(target_os = "none", link_section = ".dmabuf")]
static mut STORE: Store = Store::NEW;

/// The store (set up by `init` before any other use; main loop only).
pub fn store() -> &'static mut Store {
    // SAFETY: main loop only, no reference kept across another call.
    unsafe { &mut *core::ptr::addr_of_mut!(STORE) }
}

/// Set the store up in place (its static is not loaded on the device).
pub fn init() {
    // Field by field: `write(Store::NEW)` put an 8.7 KB image of it in
    // flash. The codes need no init (only indices below `tot` are read).
    let st = store();
    st.tot = 0;
    st.nanc = 0;
    st.c0 = 0;
    st.period = 1.0;
    st.sc = WaveScale { a: 0.0, b: 1.0, vdiv: 1.0, amps: false, ac: false };
    st.dropped = 0;
}

impl Store {
    const NEW: Store = Store {
        codes: [0; CAP],
        tot: 0,
        anc: [(0, 0); NANCH],
        nanc: 0,
        c0: 0,
        period: 1.0,
        sc: WaveScale { a: 0.0, b: 1.0, vdiv: 1.0, amps: false, ac: false },
        dropped: 0,
    };

    /// Empty, for a stream that began at DWT `c0` with `fs` samples/s on
    /// the row of scale `sc`; `dropped` = the stream's hole count now.
    pub fn reset(&mut self, c0: u32, fs: u32, sc: WaveScale, dropped: u32) {
        self.tot = 0;
        self.nanc = 0;
        self.c0 = c0;
        self.period = crate::clock::HCLK_HZ as f32 / fs.max(1) as f32;
        self.sc = sc;
        self.dropped = dropped;
    }

    /// Restart after a hole (the scale and the session stay).
    pub fn restart(&mut self, dropped: u32) {
        self.tot = 0;
        self.nanc = 0;
        self.dropped = dropped;
    }

    pub fn dropped(&self) -> u32 {
        self.dropped
    }

    pub fn scale(&self) -> WaveScale {
        self.sc
    }

    pub fn set_scale(&mut self, sc: WaveScale) {
        self.sc = sc;
    }

    /// One sample: its stamp (us since the session's begin) and raw AD2
    /// code (19-bit signed).
    #[inline(always)]
    pub fn push(&mut self, ts_us: u32, raw: i32) {
        let n = self.tot;
        self.codes[(n % CAP as u64) as usize] = (raw >> 3) as i16;
        if n % EVERY == 0 {
            let t = self.c0.wrapping_add(ts_us.wrapping_mul(200)).wrapping_sub((lag_us() * 200) as u32);
            self.anc[self.nanc % NANCH] = (n, t);
            self.nanc += 1;
        }
        self.tot = n + 1;
    }

    /// SI of a stored code.
    fn si(&self, c: i16) -> f32 {
        let raw = (c as i32) * 8 + 4;
        self.sc.a + self.sc.b * (raw as f32 + 512.0 * 512.0 - 256.0) / 512.0
    }

    /// Sample index (fractional) at DWT time `t`; None before the oldest
    /// anchor still in the store (or no anchor yet).
    fn index_at(&self, t: u32) -> Option<f64> {
        let n = self.nanc.min(NANCH);
        if n == 0 {
            return None;
        }
        let oldest = self.tot.saturating_sub(CAP as u64);
        let newest = (self.nanc - 1) % NANCH;
        // Walk back from the newest anchor to the segment holding `t`.
        let (mut hi_i, mut hi_t) = self.anc[newest];
        let rel = t.wrapping_sub(hi_t) as i32;
        if rel >= 0 {
            // After the newest anchor: its segment's slope (or nominal).
            let slope = if n >= 2 {
                let (pi, pt) = self.anc[(newest + NANCH - 1) % NANCH];
                let dt = hi_t.wrapping_sub(pt) as i32;
                if dt > 0 { (hi_i - pi) as f64 / dt as f64 } else { 1.0 / self.period as f64 }
            } else {
                1.0 / self.period as f64
            };
            return Some(hi_i as f64 + rel as f64 * slope);
        }
        for k in 1..n {
            let (lo_i, lo_t) = self.anc[(newest + NANCH - k) % NANCH];
            if lo_i < oldest {
                return None;
            }
            let r = t.wrapping_sub(lo_t) as i32;
            if r >= 0 {
                let dt = hi_t.wrapping_sub(lo_t) as i32;
                return Some(lo_i as f64 + if dt > 0 { r as f64 * (hi_i - lo_i) as f64 / dt as f64 } else { 0.0 });
            }
            (hi_i, hi_t) = (lo_i, lo_t);
        }
        None
    }

    /// SI min / max of the samples taken in DWT [t0, t0 + len) (len in
    /// cycles, > 0): None when none of them is in the store. A column
    /// shorter than a sample takes the sample nearest its start. The
    /// reference for `span_run` (tools/ch2-test); the frame uses that.
    #[allow(dead_code)]
    pub fn span(&self, t0: u32, len: f32) -> Option<(f32, f32)> {
        self.range(t0, len).map(|(ia, ib)| self.min_max(ia, ib))
    }

    /// The sample indices [ia, ib) of `span`.
    #[allow(dead_code)]
    pub fn range(&self, t0: u32, len: f32) -> Option<(u64, u64)> {
        let a = self.index_at(t0)?;
        let b = a + len as f64 / self.period as f64;
        let oldest = self.tot.saturating_sub(CAP as u64) as f64;
        let (a, b) = (a.max(oldest), b.min(self.tot as f64));
        if !(b > a) {
            return None;
        }
        let ia = a as u64;
        let ib = (b as u64).max(ia + 1).min(self.tot);
        Some((ia, ib))
    }

    fn min_max(&self, ia: u64, ib: u64) -> (f32, f32) {
        let (mut lo, mut hi) = (i16::MAX, i16::MIN);
        for n in ia..ib {
            let c = self.codes[(n % CAP as u64) as usize];
            lo = lo.min(c);
            hi = hi.max(c);
        }
        (self.si(lo), self.si(hi))
    }

    /// The anchor with age `k` (0 = the newest).
    fn anc_k(&self, k: usize) -> (u64, u32) {
        self.anc[((self.nanc - 1) % NANCH + NANCH - k) % NANCH]
    }

    /// A run of `span_run` lookups at rising start times (a frame's
    /// columns, left to right).
    pub fn run(&self) -> Run {
        let n = self.nanc.min(NANCH);
        let oldest = self.tot.saturating_sub(CAP as u64);
        // The oldest anchor `index_at` may use (older ones were overwritten
        // in the codes ring).
        let mut kmax = n.saturating_sub(1);
        while kmax > 0 && self.anc_k(kmax).0 < oldest {
            kmax -= 1;
        }
        Run { k: kmax, kmax, inv_period: 1.0 / self.period }
    }

    /// `span` for the next column of a run: the anchor search goes on from
    /// the last column's segment, the maths in f32 relative to the
    /// segment's anchor (`span`'s soft-float f64 per column was ~11 ms of a
    /// 1 ms/div frame in the emulator, ~70 % of it). Results as `span`
    /// (the sample range may differ by one at a rounding edge).
    pub fn span_run(&self, r: &mut Run, t0: u32, len: f32) -> Option<(f32, f32)> {
        self.range_run(r, t0, len).map(|(ia, ib)| self.min_max(ia, ib))
    }

    /// The sample indices [ia, ib) of `span_run`.
    pub fn range_run(&self, r: &mut Run, t0: u32, len: f32) -> Option<(u64, u64)> {
        if self.nanc == 0 {
            return None;
        }
        let after = |t: u32, at: u32| t.wrapping_sub(at) as i32 >= 0;
        let mut k = r.k.min(r.kmax);
        while k > 0 && after(t0, self.anc_k(k - 1).1) {
            k -= 1;
        }
        let (base, frac) = if k == 0 && after(t0, self.anc_k(0).1) {
            // After the newest anchor: its segment's slope (or nominal).
            let (hi_i, hi_t) = self.anc_k(0);
            let slope = if self.nanc.min(NANCH) >= 2 {
                let (pi, pt) = self.anc_k(1);
                let dt = hi_t.wrapping_sub(pt) as i32;
                if dt > 0 { (hi_i - pi) as u32 as f32 / dt as f32 } else { r.inv_period }
            } else {
                r.inv_period
            };
            (hi_i, t0.wrapping_sub(hi_t) as i32 as f32 * slope)
        } else if k > 0 && after(t0, self.anc_k(k).1) {
            let (lo_i, lo_t) = self.anc_k(k);
            let (hi_i, hi_t) = self.anc_k(k - 1);
            let dt = hi_t.wrapping_sub(lo_t) as i32;
            let rel = t0.wrapping_sub(lo_t) as i32;
            (lo_i, if dt > 0 { rel as f32 * (hi_i - lo_i) as u32 as f32 / dt as f32 } else { 0.0 })
        } else if k == r.kmax {
            // Before the oldest usable anchor.
            r.k = k;
            return None;
        } else {
            // Not rising (never in a frame): the full search.
            r.k = r.kmax;
            return self.range(t0, len);
        };
        r.k = k;
        // 32-bit conversions (the u64 / i64 ones are library calls): base
        // is within the store, oldest .. tot.
        let lo = -(base.saturating_sub(self.tot.saturating_sub(CAP as u64)) as u32 as f32);
        let hi = (self.tot - base) as u32 as f32;
        let fa = frac.max(lo);
        let fb = (frac + len * r.inv_period).min(hi);
        if !(fb > fa) {
            return None;
        }
        // fa >= 0: the slopes are positive, `lo` <= 0 (base >= oldest).
        let ia = base + fa as u32 as u64;
        let ib = (base + fb as u32 as u64).max(ia + 1).min(self.tot);
        Some((ia, ib))
    }
}

/// The state of a `Store::span_run` sequence.
pub struct Run {
    /// The age of the anchor the last column's segment starts at.
    k: usize,
    kmax: usize,
    inv_period: f32,
}

