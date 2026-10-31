//! FFT of the scope record (docs/user-feedback.md UF-11): a windowed real
//! FFT in f32 (Cortex-M4F FPU), dBV per bin, peak search with interpolation.
//! Pure code (no hardware), tested on the host by tools/fft-test.
//!
//! Size: `N` = 2048 real points (fewer, a power of two >= 256, only when the
//! source has less), computed as an N/2-point complex radix-2 FFT of the
//! packed real sequence (z[m] = x[2m] + i x[2m+1]) and the usual split into
//! the N/2 + 1 bins 0 .. Nyquist. Twiddles and windows come from one
//! compile-time quarter-wave sine table (no libm on the device).
//!
//! Bounded CPU per main-loop pass (the scope page calls `step` once per
//! pass): `load` (on the pass the frame arrives, while its samples are still
//! in the scope record: decimation, window, packing in bit-reversed order),
//! then `STAGES_PER_STEP` butterfly stages per `step`, then one `step` for
//! the split + power + dB of every bin. The result (`db`, `bins`, `bin_hz`)
//! changes only inside that last step, so it is always one complete
//! spectrum. A `load` while a job runs is refused: the spectrum then updates
//! at the rate the job completes, from the newest frame at each start.
//!
//! Scale: dBV = 20 log10(Vrms) of a sine centred on a bin, i.e. the window's
//! coherent gain is divided out (amplitude-correct for the flat-top window
//! anywhere between bins, up to -1.4 dB scalloping for Hann between bins;
//! `peak` interpolates the vertex of the dB parabola through the top bin and
//! its neighbours, which removes most of that). Bin 0 is the DC volts.

#![allow(dead_code)]

/// Real points of a full-size transform.
pub const N: usize = 2048;
/// Bins 0 ..= N/2 (DC .. Nyquist).
pub const BINS: usize = N / 2 + 1;
/// Smallest transform (real points).
pub const N_MIN: usize = 256;
/// Butterfly stages per `step` (a 1024-point complex stage = 512
/// butterflies).
pub const STAGES_PER_STEP: u8 = 3;
/// Floor of the dB values (an empty bin).
pub const DB_FLOOR: f32 = -200.0;

/// FFT windows: the stock five (Math popup, Other tab; the stock spells
/// "Hanming") and flat-top (opendm40, amplitude-accurate).
pub const WIN_NAMES: [&str; 6] = ["Rectangular", "Blackman", "Hanning", "Hamming", "Triangular", "Flat-top"];
pub const W_RECT: u8 = 0;
pub const W_BLACKMAN: u8 = 1;
pub const W_HANN: u8 = 2;
pub const W_HAMMING: u8 = 3;
pub const W_TRI: u8 = 4;
pub const W_FLATTOP: u8 = 5;
/// Main-lobe half width per window (bins): the bins around DC the peak
/// search leaves out.
const LOBE: [usize; 6] = [1, 3, 2, 2, 2, 5];

const Q: usize = N / 4;

/// sin(2 pi k / N), k = 0..=N/4, at compile time (Taylor series in f64).
/// Region A2 (zero-wait, link-native.x): read per butterfly.
#[cfg_attr(target_os = "none", link_section = ".rodata.a2.QSIN")]
static QSIN: [f32; Q + 1] = qsin();

const fn qsin() -> [f32; Q + 1] {
    let mut t = [0f32; Q + 1];
    let mut k = 0;
    while k <= Q {
        let x = 2.0 * core::f64::consts::PI * k as f64 / N as f64;
        let mut term = x;
        let mut s = x;
        let mut i = 1;
        while i < 14 {
            term = -term * x * x / ((2 * i) as f64 * (2 * i + 1) as f64);
            s += term;
            i += 1;
        }
        t[k] = s as f32;
        k += 1;
    }
    t
}

/// sin(2 pi i / N) for any i.
#[inline]
pub fn sin_i(i: usize) -> f32 {
    let i = i % N;
    match i / Q {
        0 => QSIN[i],
        1 => QSIN[2 * Q - i],
        2 => -QSIN[i - 2 * Q],
        _ => -QSIN[4 * Q - i],
    }
}

/// cos(2 pi i / N) for any i.
#[inline]
pub fn cos_i(i: usize) -> f32 {
    sin_i(i + Q)
}

/// Window `kind` at point `n` of `len` (periodic form, as for spectra).
pub fn window(kind: u8, n: usize, len: usize) -> f32 {
    let st = N / len;
    let c = |m: usize| cos_i(m * n * st);
    match kind {
        W_BLACKMAN => 0.42 - 0.5 * c(1) + 0.08 * c(2),
        W_HANN => 0.5 - 0.5 * c(1),
        W_HAMMING => 0.54 - 0.46 * c(1),
        W_TRI => 1.0 - (2.0 * n as f32 / len as f32 - 1.0).abs(),
        W_FLATTOP => 0.215_578_95 - 0.416_631_58 * c(1) + 0.277_263_16 * c(2) - 0.083_578_95 * c(3) + 0.006_947_37 * c(4),
        _ => 1.0,
    }
}

/// ln(x), x > 0 (exponent + series on the mantissa; |error| < 2e-6).
pub fn ln(x: f32) -> f32 {
    if !(x > 0.0) {
        return f32::NEG_INFINITY;
    }
    let b = x.to_bits();
    let e = ((b >> 23) & 0xFF) as i32 - 127;
    let m = f32::from_bits((b & 0x007F_FFFF) | 0x3F80_0000); // 1..2
    // Centre the mantissa on 1 (0.707..1.414) for a faster series.
    let (m, e) = if m > core::f32::consts::SQRT_2 { (m * 0.5, e + 1) } else { (m, e) };
    let t = (m - 1.0) / (m + 1.0);
    let t2 = t * t;
    e as f32 * core::f32::consts::LN_2 + 2.0 * t * (1.0 + t2 * (1.0 / 3.0 + t2 * (1.0 / 5.0 + t2 * (1.0 / 7.0 + t2 / 9.0))))
}

/// 10 log10(x) (x <= 0: DB_FLOOR).
pub fn db10(x: f32) -> f32 {
    if x > 0.0 { (ln(x) * 4.342_944_8).max(DB_FLOOR) } else { DB_FLOOR }
}

/// 2^y (|relative error| < 3e-7 for |y| < 120).
pub fn exp2(y: f32) -> f32 {
    if y < -126.0 {
        return 0.0;
    }
    if y > 127.0 {
        return f32::INFINITY;
    }
    let fl = if y >= 0.0 { y as i32 } else { y as i32 - ((y as i32) as f32 != y) as i32 };
    let f = y - fl as f32; // 0..1
    // 2^f, f in 0..1: degree-6 Taylor of e^(f ln 2).
    let z = f * core::f32::consts::LN_2;
    let p = 1.0 + z * (1.0 + z * (0.5 + z * (1.0 / 6.0 + z * (1.0 / 24.0 + z * (1.0 / 120.0 + z / 720.0)))));
    p * f32::from_bits(((fl + 127) as u32) << 23)
}

/// Volts (rms) of a dBV value.
pub fn db_to_v(db: f32) -> f32 {
    exp2(db * 0.166_096_4)
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Phase {
    Idle,
    /// Next butterfly stage.
    Stages(u8),
    Post,
}

/// Complex work buffer of a job (re, im interleaved, bit-reversed after
/// `load`), owned by the caller and passed to `load` / `step`: it is needed
/// only while a job is in flight, so the device lends it from a buffer that
/// is idle meanwhile (diskbuf.rs `fft_work`, the disk buffer arena).
pub type Work = [f32; N];

/// One spectrum job and the last complete result.
pub struct Fft {
    /// Result: dBV per bin 0 ..= bins - 1.
    pub db: [f32; BINS],
    /// Bins of the result (0 = none yet), Hz per bin, window used.
    pub bins: usize,
    pub bin_hz: f32,
    pub win: u8,
    /// Results produced (wraps).
    pub seq: u32,
    // --- job in flight
    len: usize,
    s1: f32,
    hz: f32,
    jwin: u8,
    phase: Phase,
}

impl Fft {
    pub const fn new() -> Fft {
        // All zero: .bss, not a 4 KB .data image (`bins` 0 = no result).
        Fft { db: [0.0; BINS], bins: 0, bin_hz: 0.0, win: 0, seq: 0, len: 0, s1: 0.0, hz: 0.0, jwin: 0, phase: Phase::Idle }
    }

    /// A job is in flight (`load` refuses).
    pub fn busy(&self) -> bool {
        self.phase != Phase::Idle
    }

    /// Drop the job in flight and the result (no spectrum shown).
    pub fn clear(&mut self) {
        self.phase = Phase::Idle;
        self.bins = 0;
        self.seq = self.seq.wrapping_add(1);
    }

    /// Start a job: `len` points (a power of two, N_MIN..=N), point j = the
    /// mean of `src[j k .. j k + k]` as volts `a + b * code` (the scope's
    /// affine code -> volts map), sample rate `fs` of `src`, window `win`.
    /// False (nothing done): busy, or `src` too short / `len` invalid.
    pub fn load<S: crate::pack10::Src + ?Sized>(&mut self, work: &mut Work, src: &S, k: usize, len: usize, a: f32, b: f32, fs: f32, win: u8) -> bool {
        if self.busy() || !len.is_power_of_two() || !(N_MIN..=N).contains(&len) || k == 0 || src.len() < len * k || !(fs > 0.0) {
            return false;
        }
        let m = len / 2;
        let bits = m.trailing_zeros();
        let inv_k = b / k as f32;
        let mut s1 = 0.0f32;
        let mut p = 0usize;
        for n in 0..len {
            let mut sum = 0u32;
            for i in p..p + k {
                sum += src.at(i) as u32;
            }
            p += k;
            let w = window(win, n, len);
            s1 += w;
            let v = (a + sum as f32 * inv_k) * w;
            // z[n / 2] (re: even n, im: odd n) into its bit-reversed slot.
            let r = ((n / 2) as u32).reverse_bits() >> (32 - bits);
            work[2 * r as usize + (n & 1)] = v;
        }
        self.len = len;
        self.s1 = if s1 > 0.0 { s1 } else { 1.0 };
        self.hz = fs / (k * len) as f32;
        self.jwin = win;
        self.phase = Phase::Stages(0);
        true
    }

    /// One bounded slice of the job in flight. True when a new result is
    /// complete (in `db` / `bins` / `bin_hz`).
    pub fn step(&mut self, w: &mut Work) -> bool {
        match self.phase {
            Phase::Idle => false,
            Phase::Stages(s0) => {
                let m = self.len / 2;
                let total = m.trailing_zeros() as u8;
                let s1 = (s0 + STAGES_PER_STEP).min(total);
                for s in s0..s1 {
                    Self::stage(w, s as u32, m);
                }
                self.phase = if s1 >= total { Phase::Post } else { Phase::Stages(s1) };
                false
            }
            Phase::Post => {
                self.post(w);
                self.phase = Phase::Idle;
                true
            }
        }
    }

    /// Run the whole job now (host tests).
    pub fn finish(&mut self, w: &mut Work) {
        while self.busy() {
            self.step(w);
        }
    }

    /// Drop the job in flight, keep the last result (its work buffer was
    /// taken back).
    pub fn abort(&mut self) {
        self.phase = Phase::Idle;
    }

    /// Radix-2 DIT stage `s` (butterfly span 2^s) of the m-point transform.
    fn stage(b: &mut Work, s: u32, m: usize) {
        let half = 1usize << s;
        let size = half * 2;
        // W_size^j = exp(-2 pi i j / size) = table index j * N / size... of
        // the m-point transform: the table is for N = 2 m_max, so index
        // j * (N / size).
        let step = N / size;
        for j in 0..half {
            let (wr, wi) = (cos_i(j * step), -sin_i(j * step));
            let mut a = j;
            while a < m {
                let c = a + half;
                let (cr, ci) = (b[2 * c], b[2 * c + 1]);
                let tr = wr * cr - wi * ci;
                let ti = wr * ci + wi * cr;
                let (ar, ai) = (b[2 * a], b[2 * a + 1]);
                b[2 * c] = ar - tr;
                b[2 * c + 1] = ai - ti;
                b[2 * a] = ar + tr;
                b[2 * a + 1] = ai + ti;
                a += size;
            }
        }
    }

    /// Split of the packed transform into the real spectrum, power, dB.
    fn post(&mut self, b: &Work) {
        let len = self.len;
        let m = len / 2;
        let st = N / len;
        // Sine of a bin: |X| = A s1 / 2 -> Vrms = sqrt(2) |X| / s1; DC: X / s1.
        let k_ac = db10(2.0 / (self.s1 * self.s1));
        let k_dc = db10(1.0 / (self.s1 * self.s1));
        let (z0r, z0i) = (b[0], b[1]);
        let x0 = z0r + z0i;
        let xm = z0r - z0i;
        self.db[0] = (db10(x0 * x0) + k_dc).max(DB_FLOOR);
        self.db[m] = (db10(xm * xm) + k_ac).max(DB_FLOOR);
        for k in 1..=m / 2 {
            let (ar, ai) = (b[2 * k], b[2 * k + 1]);
            let (br, bi) = (b[2 * (m - k)], b[2 * (m - k) + 1]);
            let x = |ar: f32, ai: f32, br: f32, bi: f32, kk: usize| -> f32 {
                let (fer, fei) = (0.5 * (ar + br), 0.5 * (ai - bi));
                let (for_, foi) = (0.5 * (ai + bi), -0.5 * (ar - br));
                let (c, s) = (cos_i(kk * st), sin_i(kk * st));
                // W = c - i s.
                let xr = fer + c * for_ + s * foi;
                let xi = fei + c * foi - s * for_;
                xr * xr + xi * xi
            };
            let p1 = x(ar, ai, br, bi, k);
            let p2 = x(br, bi, ar, ai, m - k);
            self.db[k] = (db10(p1) + k_ac).max(DB_FLOOR);
            self.db[m - k] = (db10(p2) + k_ac).max(DB_FLOOR);
        }
        self.bins = m + 1;
        self.bin_hz = self.hz;
        self.win = self.jwin;
        self.seq = self.seq.wrapping_add(1);
    }

    /// dB at frequency `f` (linear between bins), None outside the result.
    pub fn db_at(&self, f: f32) -> Option<f32> {
        if self.bins < 2 || !(f >= 0.0) {
            return None;
        }
        let x = f / self.bin_hz;
        if x > (self.bins - 1) as f32 {
            return None;
        }
        let i = (x as usize).min(self.bins - 2);
        let fr = x - i as f32;
        Some(self.db[i] + (self.db[i + 1] - self.db[i]) * fr)
    }

    /// Highest dB of the bins whose centre lies in [f0, f1), else (no bin in
    /// there: zoomed in past the bin spacing) the value at the middle.
    pub fn col_max(&self, f0: f32, f1: f32) -> Option<f32> {
        if self.bins < 2 || !(f1 > 0.0) {
            return None;
        }
        let lo = (f0.max(0.0) / self.bin_hz).ceil_f() as usize;
        let hi_f = f1 / self.bin_hz;
        let mut best: Option<f32> = None;
        let mut i = lo;
        while i < self.bins && (i as f32) < hi_f {
            let v = self.db[i];
            best = Some(best.map_or(v, |b: f32| b.max(v)));
            i += 1;
        }
        best.or_else(|| self.db_at(0.5 * (f0 + f1)))
    }

    /// The highest bin in [f0, f1] outside the DC main lobe: (frequency,
    /// dBV), both interpolated (parabola through the top bin and its
    /// neighbours in dB). None without a result or a bin in the range.
    pub fn peak(&self, f0: f32, f1: f32) -> Option<(f32, f32)> {
        if self.bins < 3 {
            return None;
        }
        let guard = LOBE[(self.win as usize).min(5)];
        let lo = ((f0.max(0.0) / self.bin_hz).ceil_f() as usize).max(guard);
        let hi = ((f1 / self.bin_hz) as usize).min(self.bins - 1);
        if lo > hi {
            return None;
        }
        let mut k = lo;
        for i in lo..=hi {
            if self.db[i] > self.db[k] {
                k = i;
            }
        }
        let y0 = self.db[k];
        if k == 0 || k + 1 >= self.bins {
            return Some((k as f32 * self.bin_hz, y0));
        }
        let (ym, yp) = (self.db[k - 1], self.db[k + 1]);
        if self.win == W_RECT {
            // Rectangular (sinc lobe): the larger neighbour's share of the
            // linear magnitudes gives the offset; the amplitude is the top
            // bin over sinc(offset).
            let (m0, mm, mp) = (db_to_v(y0), db_to_v(ym), db_to_v(yp));
            let d = if mp >= mm { mp / (m0 + mp) } else { -mm / (m0 + mm) };
            let x = d.abs();
            // sin(pi x) = sin(2 pi (x / 2)): table index x * N / 2.
            let sinc = if x > 1e-4 { sin_i((x * (N / 2) as f32 + 0.5) as usize) / (core::f32::consts::PI * x) } else { 1.0 };
            return Some(((k as f32 + d) * self.bin_hz, y0 - 2.0 * db10(sinc)));
        }
        // Other windows: vertex of the parabola through the dB values (a
        // Gaussian fit of the lobe); flat-top keeps the top bin's amplitude
        // (its lobe is flat to ~0.01 dB, the parabola would not be).
        let den = ym - 2.0 * y0 + yp;
        let d = if den < 0.0 { (0.5 * (ym - yp) / den).clamp(-0.5, 0.5) } else { 0.0 };
        let amp = if self.win == W_FLATTOP { y0 } else { y0 - 0.25 * (ym - yp) * d };
        Some(((k as f32 + d) * self.bin_hz, amp))
    }
}

/// `ceil` without libm.
trait CeilF {
    fn ceil_f(self) -> f32;
}

impl CeilF for f32 {
    #[inline]
    fn ceil_f(self) -> f32 {
        let t = self as i32 as f32;
        if t < self { t + 1.0 } else { t }
    }
}

/// The largest 1-2-5 value <= x (x > 0).
pub fn floor125(x: f32) -> f32 {
    if !(x > 0.0) {
        return 0.0;
    }
    // Tolerance for the inexact decades below 1 (0.1 is not representable).
    let x = x * 1.0001;
    let mut d = 1.0f32;
    while d * 10.0 <= x {
        d *= 10.0;
    }
    while d > x {
        d *= 0.1;
    }
    if 5.0 * d <= x {
        5.0 * d
    } else if 2.0 * d <= x {
        2.0 * d
    } else {
        d
    }
}

/// The next lower 1-2-5 value below the 1-2-5 value v (5 -> 2 -> 1 -> 0.5).
pub fn down125(v: f32) -> f32 {
    floor125(v * 0.99)
}
