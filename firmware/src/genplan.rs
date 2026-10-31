//! Generator planning: pure arithmetic, no registers. gen.rs programs the
//! hardware from it, and the host test (tools/gen-test) compiles this file
//! as is, so it uses `core` only.
//!
//! Hardware model (gen.rs, scope_gen_v2_6.h GENERATOR):
//! - DAC path: DAC ch1 (PA4) takes one sample per TIM6 update. The timer
//!   clock is 200 MHz, so the sample rate is 200 MHz / M with
//!   M = (PSC+1)(ARR+1) and both fields 16 bit. DMA plays a circular table of
//!   L samples that holds K whole periods. Output frequency:
//!   `f = 200 MHz * K / (M * L)`. The sample rate is capped at the stock's
//!   10 MS/s (M >= 20), the only rate verified on the device.
//! - PWM path (square above 50 kHz): TIM12 CH1 on PB14, `f = 200 MHz / D`,
//!   with D = ARR+1 and PSC = 0. Logic level: Vpp and offset do not apply.
//!
//! The frequency search (`plan`) picks M, L and K. A table always holds whole
//! periods and never changes while it plays (no CPU refill), so the output
//! cannot glitch. K > 1 ("multi-period table", a static DDS) gives
//! fractional samples per period. That is finer frequency resolution, but a
//! sample lands on a different phase in each of the K periods. Sine,
//! triangle and ramp then show small spurs; on a square the edges would
//! jitter by one sample period, so a square always uses K = 1.
#![allow(dead_code)]

/// Timer kernel clock of TIM6 and TIM12 (APB1 x4, TIMPRE; clock.rs).
pub const TIM_CLK_HZ: u32 = 200_000_000;
/// Smallest TIM6 divisor: 10 MS/s, the stock's DAC update rate.
pub const DIV_MIN: u32 = 20;
/// Highest DAC sample rate, S/s.
pub const FS_MAX: u32 = TIM_CLK_HZ / DIV_MIN;
/// Wave table length (DMA buffer in gen.rs).
pub const N_MAX: usize = 2000;
/// Settable frequency range, Hz.
pub const F_MIN: u32 = 1;
pub const F_MAX: u32 = 10_000_000;
/// A square above this frequency uses the PWM path (stock boundary: the
/// buffered DAC slews too slowly for faster square edges).
pub const SQUARE_DAC_MAX: u32 = 50_000;
/// Highest sine / triangle / ramp frequency (stock limit, see `Wave::max_hz`).
pub const DAC_WAVE_MAX: u32 = 50_000;
/// Smallest PWM divisor (10 MHz).
pub const PWM_DIV_MIN: u32 = 20;
/// DAC full scale: code 4095 = 3.0 V (stock convention, a = Vpp / 3).
pub const V_FS: f32 = 3.0;
pub const CODE_FS: f32 = 4095.0;
/// Amplitude range, Vpp. Stock minimum is 0.5 V; 0 V is allowed here so that
/// an offset with Vpp 0 is a DC source (FR-28).
pub const AMP_MIN: f32 = 0.0;
pub const AMP_MAX: f32 = V_FS;
/// Multi-period tables are tried only when the single-period error is above
/// this (ppm): below it the crystal's own tolerance (tens of ppm) dominates
/// and a clean single-period table is worth more.
const MULTI_PPM: u64 = 10;
/// Phase A pass 2 gives up this much accuracy (ppm) for more samples per
/// period.
const SLACK_PPM: u64 = 1;
/// Multi-period tables use at least FS_MAX / 2 = 5 MS/s, which keeps the phase
/// jitter at or below 200 ns.
const MULTI_DIV_MAX: u32 = 2 * DIV_MIN;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Wave {
    Sine,
    Square,
    Triangle,
    /// Rising sawtooth, matching the preview icon. The stock's sawtooth fell
    /// (`4095 - 4095 i / n`), against its own icon; users saw it falling on
    /// a Rigol (UF-16, user-feedback.md section 7). The stock had no second
    /// ramp direction.
    Ramp,
}

impl Wave {
    pub const ALL: [Wave; 4] = [Wave::Sine, Wave::Square, Wave::Triangle, Wave::Ramp];

    pub fn name(self) -> &'static str {
        match self {
            Wave::Sine => "Sine",
            Wave::Square => "Square",
            Wave::Triangle => "Triangle",
            Wave::Ramp => "Ramp",
        }
    }

    /// Minimum samples per period the frequency search may use on the DAC
    /// path. Sine / triangle / ramp: 100, their sampling quality floor (a
    /// 50 kHz wave at 5 MS/s or a 100 kHz one at 10 MS/s). Square: 40, as it
    /// has only two levels; a shorter table quantises the duty more coarsely
    /// (1/L, reported in `Out::duty`) but gives the K = 1 search more
    /// divisor pairs near 50 kHz.
    pub const fn n_min(self) -> u32 {
        match self {
            Wave::Square => 40,
            _ => 100,
        }
    }

    /// Highest output frequency for this wave, Hz. A higher set frequency is
    /// clamped to this (UF-14), never silenced. DAC waves keep the stock
    /// 50 kHz: the analog stage after PA4 has not been measured above it.
    /// Raise DAC_WAVE_MAX (up to FS_MAX / n_min = 100 kHz) after the device
    /// test shows a clean, full-amplitude output there.
    pub const fn max_hz(self) -> u32 {
        match self {
            Wave::Square => F_MAX,
            _ => DAC_WAVE_MAX,
        }
    }

    /// Note for the UI when the set frequency is above `max_hz` (UF-14).
    pub fn max_note(self) -> &'static str {
        match self {
            Wave::Square => "max 10 MHz",
            _ => "max 50 kHz",
        }
    }
}

/// Per-wave limits for UI and SCPI range checks.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Limits {
    pub f_min: u32,
    pub f_max: u32,
    /// Above this the output is the fixed-level PWM path (square only;
    /// = f_max for the DAC-only waves).
    pub dac_max_hz: u32,
    pub amp_min: f32,
    pub amp_max: f32,
    /// Output window: every DAC sample lies in 0 ..= v_max volts.
    pub v_max: f32,
}

pub fn limits(w: Wave) -> Limits {
    Limits {
        f_min: F_MIN,
        f_max: w.max_hz(),
        dac_max_hz: if w == Wave::Square { SQUARE_DAC_MAX } else { w.max_hz() },
        amp_min: AMP_MIN,
        amp_max: AMP_MAX,
        v_max: V_FS,
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Settings {
    pub on: bool,
    pub wave: Wave,
    /// The set frequency, Hz (1 .. 10 MHz). It is kept as set across wave
    /// changes; the output runs at `Out::target_hz` (clamped to the wave's
    /// limit), quantised to `Out::hz()`.
    pub freq_hz: u32,
    /// Peak-to-peak amplitude, V (0 .. 3.0).
    pub amp_v: f32,
    /// Square duty, 0..10000 = 0..100.00 %.
    pub duty: u16,
    /// DC offset = centre (mean) level of the waveform, V. None = stock
    /// behaviour: unipolar 0 .. Vpp, centre Vpp/2. Some(o) is kept in
    /// [Vpp/2, 3.0 - Vpp/2] so that the whole waveform stays inside the
    /// unipolar 0..3 V output (FR-28).
    pub offset_v: Option<f32>,
}

impl Settings {
    // Stock power-on defaults (scope_gen_ui_v2_6.h:318-320, 556-559; the ui header
    // corrects the gen header's "wave default 0" to wave 1 = Square at 2 MHz).
    pub const DEFAULT: Settings =
        Settings { on: false, wave: Wave::Square, freq_hz: 2_000_000, amp_v: 3.0, duty: 5000, offset_v: None };

    /// All fields in range. Keeps `freq_hz` as set apart from the global
    /// 1 Hz .. 10 MHz clamp (the per-wave clamp happens at output time).
    /// When Vpp and offset conflict, Vpp is kept and the offset moves.
    pub fn normalised(self) -> Settings {
        let mut s = self;
        s.freq_hz = clamp_freq(s.freq_hz);
        s.amp_v = if s.amp_v.is_finite() { s.amp_v.clamp(AMP_MIN, AMP_MAX) } else { AMP_MAX };
        s.duty = s.duty.min(10000);
        s.offset_v = match s.offset_v {
            Some(o) if o.is_finite() => Some(o.clamp(s.amp_v / 2.0, V_FS - s.amp_v / 2.0)),
            _ => None,
        };
        s
    }

    /// Set Vpp. With an explicit offset, Vpp is limited so that the waveform
    /// fits around that offset: the value being set is the one clamped.
    pub fn with_amp(self, v: f32) -> Settings {
        let mut s = self;
        let mut v = if v.is_finite() { v.clamp(AMP_MIN, AMP_MAX) } else { s.amp_v };
        if let Some(o) = s.offset_v {
            v = v.min(2.0 * o).min(2.0 * (V_FS - o)).max(AMP_MIN);
        }
        s.amp_v = v;
        s.normalised()
    }

    /// Set the offset (centre level). None returns to the stock 0..Vpp. The
    /// offset is clamped to [Vpp/2, 3.0 - Vpp/2].
    pub fn with_offset(self, o: Option<f32>) -> Settings {
        let mut s = self;
        s.offset_v = o;
        s.normalised()
    }
}

/// Normalise a user-set frequency for `Settings::freq_hz`: 1 Hz .. 10 MHz.
pub fn clamp_freq(f: u32) -> u32 {
    f.clamp(F_MIN, F_MAX)
}

/// The frequency the output aims at for `wave` (before quantisation): the
/// set value clamped to the wave's limit. Never 0 (UF-14 removed the stock's
/// silent 0 Hz and its 1 kHz rounding between 1 and 50 kHz).
pub fn target_hz(wave: Wave, f: u32) -> u32 {
    f.clamp(F_MIN, wave.max_hz())
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Path {
    Dac,
    Pwm,
}

/// Timer and table programming for one output frequency.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Timing {
    pub path: Path,
    /// TIM6 (DAC) or TIM12 (PWM) PSC and ARR register values.
    pub psc: u16,
    pub arr: u16,
    /// DAC: table length L (DMA NDTR). PWM: 1.
    pub len: u16,
    /// DAC: whole periods in the table (K). PWM: 1.
    pub cycles: u16,
}

impl Timing {
    /// Timer divisor M = (PSC+1)(ARR+1).
    pub fn div(&self) -> u64 {
        (self.psc as u64 + 1) * (self.arr as u64 + 1)
    }

    /// Exact output frequency as numerator / denominator, Hz.
    pub fn hz_ratio(&self) -> (u64, u64) {
        (TIM_CLK_HZ as u64 * self.cycles as u64, self.div() * self.len as u64)
    }

    /// Actual output frequency, Hz.
    pub fn hz(&self) -> f64 {
        let (n, d) = self.hz_ratio();
        n as f64 / d as f64
    }

    /// DAC sample rate, S/s (PWM: the counter rate).
    pub fn sample_rate(&self) -> f64 {
        TIM_CLK_HZ as f64 / self.div() as f64
    }

    /// Samples per period (L / K; fractional for a multi-period table).
    pub fn samples_per_period(&self) -> f32 {
        self.len as f32 / self.cycles as f32
    }
}

/// Split M into PSC and ARR (both 16 bit). Above 65536 the realised divisor
/// can differ from M; callers evaluate the realised one.
fn split(m: u64) -> (u16, u16) {
    if m <= 65536 {
        (0, (m.max(1) - 1) as u16)
    } else {
        let p = (m + 65535) / 65536;
        let a = ((m + p / 2) / p).min(65536);
        ((p - 1) as u16, (a - 1) as u16)
    }
}

/// A search candidate. Frequency error in Hz = num / den, with
/// num = |TIM_CLK * K - f * M * L| and den = M * L.
#[derive(Clone, Copy)]
struct Cand {
    t: Timing,
    num: u64,
    den: u64,
}

impl Cand {
    fn new(f: u32, psc: u16, arr: u16, len: u32, cycles: u32, path: Path) -> Cand {
        let t = Timing { path, psc, arr, len: len as u16, cycles: cycles as u16 };
        let den = t.div() * len as u64;
        let a = TIM_CLK_HZ as u64 * cycles as u64;
        let b = f as u64 * den;
        Cand { t, num: a.abs_diff(b), den }
    }

    /// Strictly smaller frequency error than `o`.
    fn better(&self, o: &Cand) -> bool {
        const W: u64 = u32::MAX as u64;
        if (self.num | self.den | o.num | o.den) <= W {
            // The usual case: 32 x 32 bit products, no u128 helper call.
            self.num * o.den < o.num * self.den
        } else {
            (self.num as u128) * (o.den as u128) < (o.num as u128) * (self.den as u128)
        }
    }

    /// Relative error <= ppm.
    fn within_ppm(&self, f: u32, ppm: u64) -> bool {
        (self.num as u128) * 1_000_000 <= (ppm as u128) * (f as u128) * (self.den as u128)
    }
}

fn keep(best: &mut Option<Cand>, c: Cand) {
    match best {
        Some(b) if !c.better(b) => {}
        _ => *best = Some(c),
    }
}

/// PWM square: D = 200 MHz / f rounded to the nearer frequency, PSC = 0.
fn plan_pwm(f: u32) -> Timing {
    let d0 = (TIM_CLK_HZ / f).clamp(PWM_DIV_MIN, 65536);
    let mut best: Option<Cand> = None;
    for d in [d0, d0 + 1] {
        if d >= PWM_DIV_MIN && d <= 65536 {
            keep(&mut best, Cand::new(f, 0, (d - 1) as u16, 1, 1, Path::Pwm));
        }
    }
    best.unwrap().t // d0 is always valid
}

/// Best single-period (K = 1) candidate with a table of `n` samples: M =
/// floor or ceil of 200 MHz / (f n), at least DIV_MIN.
fn k1(f: u32, n: u32) -> Option<Cand> {
    let m0 = TIM_CLK_HZ as u64 / (f as u64 * n as u64);
    let mut best = None;
    for m in [m0, m0 + 1] {
        if m >= DIV_MIN as u64 {
            let (psc, arr) = split(m);
            keep(&mut best, Cand::new(f, psc, arr, n, 1, Path::Dac));
        }
    }
    best
}

/// DAC path search for a target `f` (1 ..= FS_MAX / n_min Hz).
///
/// Phase A, one period per table (K = 1), tables of n = n_min ..= n_hi
/// samples, n_hi = min(N_MAX, FS_MAX / f + 1), M = floor or ceil of
/// 200 MHz / (f n). Pass 1 finds the smallest frequency error (an exact hit
/// at the largest n ends the search). Pass 2 takes the largest n whose error
/// is within SLACK_PPM of that minimum: more samples per period for a
/// negligible accuracy cost. At most 2 x 1960 candidates.
///
/// Phase B, multi-period table (K >= 2; not for a square), only when phase A
/// is off by more than MULTI_PPM: M = 20 ..= 40 (5..10 MS/s), every L up to
/// N_MAX with L / K >= n_min, K = round(f M L / 200 MHz). It walks L with a
/// running quotient/remainder, so there is no division in the loop. At most
/// 21 x 2000 candidates (a few ms on the device). B is used only if it at
/// least halves A's error.
pub fn plan_dac(f: u32, n_min: u32, multi: bool) -> Timing {
    let f = f.max(1);
    // FS_MAX / f + 1: at M = 20 that n gives the nearest frequency just below f.
    let n_hi = (N_MAX as u32).min(FS_MAX / f + 1).max(n_min.min(N_MAX as u32)).max(2);
    let n_lo = n_min.clamp(2, n_hi);
    let mut min: Option<Cand> = None;
    for n in (n_lo..=n_hi).rev() {
        if let Some(c) = k1(f, n) {
            if c.num == 0 {
                return c.t;
            }
            keep(&mut min, c);
        }
    }
    // FS_MAX / f + 1 >= n_min keeps M >= DIV_MIN possible; this fallback is
    // for out-of-range input only.
    let Some(min) = min else { return Cand::new(f, 0, (DIV_MIN - 1) as u16, n_lo, 1, Path::Dac).t };
    let mut a = min;
    for n in (n_lo..=n_hi).rev() {
        if let Some(c) = k1(f, n) {
            // err(c) <= err(min) + SLACK_PPM * f, cross-multiplied.
            let lhs = (c.num as u128) * (min.den as u128) * 1_000_000;
            let rhs = ((min.num as u128) * 1_000_000 + SLACK_PPM as u128 * f as u128 * min.den as u128) * c.den as u128;
            if lhs <= rhs {
                a = c;
                break;
            }
        }
    }
    if !multi || a.within_ppm(f, MULTI_PPM) {
        return a.t;
    }
    let tim = TIM_CLK_HZ as u64;
    let mut b: Option<Cand> = None;
    'outer: for m in DIV_MIN..=MULTI_DIV_MAX {
        let step = f as u64 * m as u64; // < tim for f <= FS_MAX / n_min
        let (mut q, mut r) = (0u64, 0u64);
        for l in 1..=N_MAX as u32 {
            r += step;
            while r >= tim {
                r -= tim;
                q += 1;
            }
            let (k, num) = if 2 * r >= tim { (q + 1, tim - r) } else { (q, r) };
            if k < 2 || k * n_min as u64 > l as u64 {
                continue;
            }
            // num = |200 MHz K - f M L| straight from the running remainder.
            let t = Timing { path: Path::Dac, psc: 0, arr: (m - 1) as u16, len: l as u16, cycles: k as u16 };
            let c = Cand { t, num, den: m as u64 * l as u64 };
            keep(&mut b, c);
            if c.num == 0 {
                break 'outer;
            }
        }
    }
    match b {
        Some(b) if (b.num as u128) * 2 * (a.den as u128) < (a.num as u128) * (b.den as u128) => b.t,
        _ => a.t,
    }
}

/// Timing for `wave` at the set frequency `f` (clamped to the wave's limit).
pub fn plan(wave: Wave, f: u32) -> Timing {
    let f = target_hz(wave, f);
    if wave == Wave::Square && f > SQUARE_DAC_MAX {
        plan_pwm(f)
    } else {
        plan_dac(f, wave.n_min(), wave != Wave::Square)
    }
}

/// DAC levels (codes, unrounded) of the waveform's low and high points.
/// offset None = stock: 0 .. Vpp, with the stock's sine lift
/// (FUN_080166a4: a = Vpp/3; sine spans a*c .. a*(c+w) with (c, w) =
/// (50, 4045) if a > 0.5 else (200, 4095); reason unknown, kept for parity).
/// An explicit offset is honoured exactly: centre o, low o - Vpp/2.
pub fn levels(s: &Settings) -> (f32, f32) {
    let s = s.normalised();
    let k = CODE_FS / V_FS;
    let (lo, hi) = match (s.offset_v, s.wave) {
        (None, Wave::Sine) => {
            let a = s.amp_v / V_FS;
            let (c, w) = if a > 0.5 { (50.0, 4045.0) } else { (200.0, 4095.0) };
            (a * c, a * (c + w))
        }
        (None, _) => (0.0, s.amp_v * k),
        (Some(o), _) => ((o - s.amp_v / 2.0) * k, (o + s.amp_v / 2.0) * k),
    };
    (lo.clamp(0.0, CODE_FS), hi.clamp(0.0, CODE_FS))
}

/// What the output actually does for a set of settings: the values the UI
/// and SCPI should show (UF-14, UF-15, FR-28).
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Out {
    pub timing: Timing,
    /// The set frequency clamped to the wave's limit, Hz.
    pub target_hz: u32,
    /// The set frequency is above the wave's limit (show `Wave::max_note`).
    pub freq_clamped: bool,
    /// DAC low / high codes (0..4095). PWM path: 0 / 4095 placeholders, the
    /// level is fixed by the logic output (see `fixed_level`).
    pub lo_code: u16,
    pub hi_code: u16,
    /// Effective square duty, 0..10000 (quantised to the table or PWM
    /// counter). Other waves: the set value, unused.
    pub duty: u16,
}

impl Out {
    /// Actual (quantised) output frequency, Hz.
    pub fn hz(&self) -> f64 {
        self.timing.hz()
    }

    /// PWM path: Vpp and offset are fixed by the logic output and are not
    /// adjustable.
    pub fn fixed_level(&self) -> bool {
        self.timing.path == Path::Pwm
    }

    fn v(code: u16) -> f32 {
        code as f32 * V_FS / CODE_FS
    }

    /// Effective low / high level, V (None on the PWM path).
    pub fn lo_v(&self) -> Option<f32> {
        (!self.fixed_level()).then(|| Self::v(self.lo_code))
    }
    pub fn hi_v(&self) -> Option<f32> {
        (!self.fixed_level()).then(|| Self::v(self.hi_code))
    }
    /// Effective Vpp, V (None on the PWM path).
    pub fn vpp_v(&self) -> Option<f32> {
        (!self.fixed_level()).then(|| Self::v(self.hi_code) - Self::v(self.lo_code))
    }
    /// Effective offset = centre level, V (None on the PWM path).
    pub fn offset_v(&self) -> Option<f32> {
        (!self.fixed_level()).then(|| (Self::v(self.hi_code) + Self::v(self.lo_code)) / 2.0)
    }
    /// Effective duty, percent (inline: SOUR:FUNC:SQU:DCYC? may come while
    /// an AD2 stream runs; gen::plan was region C until it moved to B).
    #[inline(always)]
    pub fn duty_pct(&self) -> f32 {
        self.duty as f32 / 100.0
    }
}

/// Square: number of high samples of a K = 1 table of `len` (rounded).
fn square_high(len: u32, duty: u16) -> u32 {
    (len * duty as u32 + 5000) / 10000
}

/// Everything the output will do for `s` (independent of `s.on`).
pub fn compute(s: &Settings) -> Out {
    let set = s.freq_hz;
    let s = s.normalised();
    let t = plan(s.wave, s.freq_hz);
    let target = target_hz(s.wave, s.freq_hz);
    let (lo, hi) = levels(&s);
    let duty = match (s.wave, t.path) {
        (Wave::Square, Path::Pwm) => {
            let d = t.div() as u32;
            let ccr = (d * s.duty as u32 + 5000) / 10000;
            ((ccr * 10000 + d / 2) / d) as u16
        }
        (Wave::Square, Path::Dac) => {
            let n = t.len as u32;
            ((square_high(n, s.duty) * 10000 + n / 2) / n) as u16
        }
        _ => s.duty,
    };
    let (lo_code, hi_code) = if t.path == Path::Pwm { (0, 4095) } else { ((lo + 0.5) as u16, (hi + 0.5) as u16) };
    Out { timing: t, target_hz: target, freq_clamped: set > s.wave.max_hz(), lo_code, hi_code, duty }
}

/// PWM compare value for TIM12 CCR1 (PWM mode 1: high while CNT < CCR;
/// CCR = ARR+1 is 100 %).
pub fn pwm_ccr(t: &Timing, duty: u16) -> u32 {
    let d = t.div() as u32;
    (d * duty.min(10000) as u32 + 5000) / 10000
}

/// sin(x) for x in [0, 2pi): range reduction + odd polynomial (|err| < 4e-6).
pub fn sin(x: f32) -> f32 {
    const PI: f32 = core::f32::consts::PI;
    let mut x = x;
    while x > PI {
        x -= 2.0 * PI;
    }
    let x = if x > PI / 2.0 {
        PI - x
    } else if x < -PI / 2.0 {
        -PI - x
    } else {
        x
    };
    let x2 = x * x;
    x * (1.0 - x2 / 6.0 * (1.0 - x2 / 20.0 * (1.0 - x2 / 42.0 * (1.0 - x2 / 72.0))))
}

/// Fill the DAC table for `s` with `t` (from `compute(s).timing`, DAC path):
/// `t.len` samples holding `t.cycles` periods. Sample i is at phase
/// ((i K) mod L) / L of its period. Shapes (0..1, scaled to lo..hi): sine
/// 0.5 + 0.5 sin (starts mid-rising), square high for the first
/// round(L duty) samples (K = 1 only), triangle up then down, ramp rising.
pub fn fill(table: &mut [u16], s: &Settings, t: &Timing) {
    let s = s.normalised();
    let (lo, hi) = levels(&s);
    let l = (t.len as usize).min(table.len()) as u32;
    let k = t.cycles as u32;
    let high = square_high(l, s.duty);
    let span = hi - lo;
    for (i, w) in table[..l as usize].iter_mut().enumerate() {
        let i = i as u32;
        let p = (i as u64 * k as u64 % l as u64) as u32; // phase numerator, 0..l-1
        let ph = p as f32 / l as f32;
        let shape = match s.wave {
            Wave::Sine => 0.5 + 0.5 * sin(2.0 * core::f32::consts::PI * ph),
            Wave::Square => {
                if i < high {
                    1.0
                } else {
                    0.0
                }
            }
            Wave::Triangle => {
                if 2 * p < l {
                    2.0 * ph
                } else {
                    2.0 - 2.0 * ph
                }
            }
            Wave::Ramp => ph,
        };
        *w = (lo + span * shape + 0.5).clamp(0.0, CODE_FS) as u16;
    }
}

// ------------------------------------------------- the scope's GEN channel
//
// The scope never digitises the generator for its GEN channel
// (docs/scope-gen-channel.md): the output is a known function of time. The
// DAC table / PWM counter and the scope's sample clock (TIM1) run from the
// same 200 MHz timer clock, so one reading of the generator's position at
// the scope's start (gen.rs `now`) fixes the generator's phase at every
// sample of that record. The trigger is computed (`Sync::next`) and the
// trace drawn from the wave's shape (`Shape::span`).

/// High level of the PWM path at the output (PB14's 3.3 V logic level
/// through the output stage): the GEN trace's high level there. 3.0 V: a
/// 2 MHz square looped back to CH1 read 0..3.0 V (3.07 Vpp) on the device
/// (PR #64 device check).
pub const PWM_HIGH_V: f32 = 3.0;

/// The output as a function of its phase (0..1 = one period, 0 = the first
/// table sample: sine mid-rising, square rising edge, triangle / ramp at
/// the bottom; the PWM counter at 0 = its rising edge).
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Shape {
    pub wave: Wave,
    pub lo_v: f32,
    pub hi_v: f32,
    /// Square: the high fraction of the period (the effective duty).
    pub duty: f32,
}

impl Shape {
    pub fn of(s: &Settings, o: &Out) -> Shape {
        let (lo_v, hi_v) = match (o.lo_v(), o.hi_v()) {
            (Some(l), Some(h)) => (l, h),
            _ => (0.0, PWM_HIGH_V),
        };
        Shape { wave: s.wave, lo_v, hi_v, duty: o.duty.min(10000) as f32 / 10000.0 }
    }

    /// 0..1 at phase `ph` (any real; the fraction is taken).
    pub fn unit(&self, ph: f32) -> f32 {
        let p = ph - floor(ph);
        match self.wave {
            Wave::Sine => 0.5 + 0.5 * sin(2.0 * core::f32::consts::PI * p),
            Wave::Square => {
                if p < self.duty {
                    1.0
                } else {
                    0.0
                }
            }
            Wave::Triangle => {
                if p < 0.5 {
                    2.0 * p
                } else {
                    2.0 - 2.0 * p
                }
            }
            Wave::Ramp => p,
        }
    }

    pub fn volts(&self, ph: f32) -> f32 {
        self.lo_v + (self.hi_v - self.lo_v) * self.unit(ph)
    }

    /// Lowest and highest volts over the phases [ph, ph + len] (`len` in
    /// periods, >= 0): the ends plus the shape's extremes / edges inside.
    pub fn span(&self, ph: f32, len: f32) -> (f32, f32) {
        if !(len < 1.0) {
            return (self.lo_v, self.hi_v);
        }
        let a = ph - floor(ph);
        let b = a + len;
        let (mut lo, mut hi) = (f32::MAX, f32::MIN);
        let mut add = |u: f32| {
            let v = self.lo_v + (self.hi_v - self.lo_v) * u;
            lo = lo.min(v);
            hi = hi.max(v);
        };
        add(self.unit(a));
        add(self.unit(b));
        // Phases inside (a, b] (b < 2) where the shape peaks or jumps: both
        // sides of a jump count.
        let crit: [(f32, f32, f32); 2] = match self.wave {
            Wave::Sine => [(0.25, 1.0, 1.0), (0.75, 0.0, 0.0)],
            Wave::Square => [(0.0, 0.0, 1.0), (self.duty, 1.0, 0.0)],
            Wave::Triangle => [(0.5, 1.0, 1.0), (0.0, 0.0, 0.0)],
            Wave::Ramp => [(0.0, 1.0, 0.0), (0.0, 1.0, 0.0)],
        };
        for (c, u0, u1) in crit {
            for c in [c, c + 1.0] {
                if c > a && c <= b {
                    add(u0);
                    add(u1);
                }
            }
        }
        (lo, hi)
    }

    /// Phase (0..1) where the output crosses `v` volts in the direction
    /// `rising`; None when `v` is not strictly between the low and the high
    /// level (no crossing: the trigger never fires).
    pub fn crossing(&self, v: f32, rising: bool) -> Option<f32> {
        let span = self.hi_v - self.lo_v;
        let u = (v - self.lo_v) / span;
        if !(u > 0.0 && u < 1.0) {
            return None;
        }
        let p = match self.wave {
            Wave::Square => {
                if rising {
                    0.0
                } else {
                    self.duty
                }
            }
            Wave::Triangle => {
                if rising {
                    u / 2.0
                } else {
                    1.0 - u / 2.0
                }
            }
            // The fall is the jump back at the period's end.
            Wave::Ramp => {
                if rising {
                    u
                } else {
                    0.0
                }
            }
            Wave::Sine => {
                // Monotonic halves: rising over -1/4..1/4, falling over
                // 1/4..3/4. Bisection on `unit` (no asin in core).
                let (mut a, mut b) = if rising { (-0.25f32, 0.25f32) } else { (0.25f32, 0.75f32) };
                for _ in 0..24 {
                    let m = 0.5 * (a + b);
                    if (self.unit(m) < u) == rising {
                        a = m;
                    } else {
                        b = m;
                    }
                }
                0.5 * (a + b)
            }
        };
        Some(p - floor(p))
    }
}

/// floor for f32 in `core` (no std): exact for |x| < 2^23.
fn floor(x: f32) -> f32 {
    let t = x as i32 as f32;
    if t > x { t - 1.0 } else { t }
}

/// The generator's position in time, ticks of the 200 MHz timer clock: a
/// table of K periods takes `q` = M L ticks (PWM: q = D, K = 1).
pub fn loop_ticks(t: &Timing) -> (u64, u32) {
    match t.path {
        Path::Pwm => (t.div(), 1),
        Path::Dac => (t.div() * t.len as u64, t.cycles as u32),
    }
}

/// One scope record against the generator: the generator's time (ticks
/// into its table loop, `loop_ticks`) at the record's sample 0 and the
/// ticks per sample. Sample n is at generator time `t0 + n s`; its phase
/// is the fraction of `(t0 + n s) k / q` periods.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Sync {
    pub t0: f64,
    pub s: f64,
    pub q: f64,
    pub k: f64,
    pub shape: Shape,
}

impl Sync {
    /// Periods of the generator at sample `n` (any real).
    pub fn periods(&self, n: f64) -> f64 {
        (self.t0 + n * self.s) * self.k / self.q
    }

    /// Samples per generator period.
    pub fn period_samples(&self) -> f64 {
        self.q / (self.k * self.s)
    }

    /// The phase at sample `n` as f32 (0..1).
    pub fn phase(&self, n: f64) -> f32 {
        let p = self.periods(n);
        (p - (p as i64) as f64) as f32
    }

    /// First sample index >= `n0` at or after a crossing at phase `ph`
    /// (the first sample past the instant, as a sampled edge shows it).
    pub fn next(&self, n0: u64, ph: f32) -> u64 {
        let x0 = self.periods(n0 as f64);
        let base = x0 - ph as f64;
        let mut m = base as i64 as f64;
        if m < base {
            m += 1.0;
        }
        let dn = (m - base) * self.period_samples();
        let mut d = dn as u64;
        if (d as f64) < dn {
            d += 1;
        }
        n0 + d
    }

    /// Volts min / max over samples [a, b) (a column of the screen).
    pub fn span(&self, a: f64, b: f64) -> (f32, f32) {
        let len = (b - a).max(0.0) / self.period_samples();
        self.shape.span(self.phase(a), if len >= 1.0 { 1.0 } else { len as f32 })
    }
}

/// `x` modulo `m` (> 0) into [0, m), for f64 in `core` (no rem_euclid):
/// exact enough for |x / m| < 2^52.
pub fn wrap(x: f64, m: f64) -> f64 {
    let n = x / m;
    let mut f = n as i64 as f64;
    if f > n {
        f -= 1.0;
    }
    let r = x - f * m;
    if r >= m { r - m } else if r < 0.0 { r + m } else { r }
}
