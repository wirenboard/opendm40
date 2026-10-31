//! Signal generator (stock/re/analysis/scope_gen_v2_6.h, GENERATOR).
//!
//! DAC path: DAC channel 1 on PA4, triggered by TIM6 TRGO at 200 MHz /
//! ((PSC+1)(ARR+1)) <= 10 MS/s, fed by DMA0 stream 5 (request 7) circularly
//! from a wave table of L samples holding K periods. PWM path (square above
//! 50 kHz): TIM12 CH1 on PB14 (AF9), 200 MHz / (ARR+1), any divisor 20..4000
//! with adjustable duty (the stock had 7 fixed 50 % steps). PB12 selects the
//! output path (1 = PWM, 0 = DAC). Everything runs from DMA / timers, so the
//! generator keeps going while the CPU does other things.
//!
//! The arithmetic (frequency search, per-wave limits, levels/offset, table
//! contents) is in genplan.rs, which the host test tools/gen-test also
//! builds. Differences from the stock (user feedback UF-14/15/16, FR-28):
//! - the set frequency is clamped to the wave's limit (`Wave::max_hz`, with
//!   `Out::freq_clamped` for a UI note), never silenced to 0 Hz, and not
//!   rounded to 1 kHz steps between 1 and 50 kHz;
//! - the timer divisor, table length and periods per table are searched for
//!   the smallest frequency error; `Gen::actual_hz` is the quantised result;
//! - the ramp rises (the stock's fell, against its own preview icon);
//! - an optional DC offset (`Settings::offset_v`) inside the 0..3 V window,
//!   and Vpp down to 0 V (a DC source).

#[path = "genplan.rs"]
mod plan;

#[allow(unused_imports)]
pub use plan::{
    clamp_freq, compute, limits, loop_ticks, wrap, target_hz, Limits, Out, Path, Settings, Shape, Sync, Timing, Wave, AMP_MAX,
    AMP_MIN, F_MAX, F_MIN, N_MAX, SQUARE_DAC_MAX, V_FS,
};

use crate::mmio::{modify32, r32, w32};

const RCU_APB1EN: u32 = 0x4002_3840;
const GPIOA: u32 = 0x4002_0000;
const GPIOB: u32 = 0x4002_0400;
const DAC: u32 = 0x4000_7400;
const DAC_CR: u32 = DAC + 0x00;
const DAC_DHR12R1: u32 = DAC + 0x08;
const DAC_SR: u32 = DAC + 0x34;
const TIM6: u32 = 0x4000_1000;
const TIM12: u32 = 0x4000_1800;
const DMA0: u32 = 0x4002_6000;
const S5: u32 = DMA0 + 0x10 + 0x18 * 5;
const DMA_HIFCR: u32 = DMA0 + 0x0C;

#[link_section = ".dmabuf"]
static mut WAVE: [u16; N_MAX] = [0; N_MAX];

pub struct Gen {
    pub s: Settings,
    /// What the output does for `s` (actual frequency, effective levels and
    /// duty, clamp flag), computed also while the output is off.
    pub out: Out,
    /// Actual output frequency while on, Hz (0 when off).
    pub real_hz: f32,
    /// `out.hz()` as f32, kept by `new` / `apply`: SOUR:FREQ? may come
    /// while an AD2 stream runs (gen::plan, region B since the move out of
    /// region C, need not run for it).
    out_hz: f32,
}

/// The output running now (timing, shape), for the scope's GEN channel
/// (`now`); None while off. Main loop only.
static mut RUNNING: Option<(Timing, Shape)> = None;

/// The running output's timing and shape (None: off).
pub fn running() -> Option<(Timing, Shape)> {
    unsafe { *core::ptr::addr_of!(RUNNING) }
}

/// Count of output (re)starts and stops: a scope record whose generator
/// reading is of another epoch has no GEN phase (the output restarted).
static mut EPOCH: u32 = 0;

pub fn epoch() -> u32 {
    unsafe { *core::ptr::addr_of!(EPOCH) }
}

const TIM6_CNT: u32 = TIM6 + 0x24;
const TIM12_CNT: u32 = TIM12 + 0x24;
const DWT_CYCCNT: u32 = 0xE000_1004;

/// Where the running output is now (docs/scope-gen-channel.md): its time,
/// ticks of the 200 MHz timer clock into its table loop (`loop_ticks`:
/// 0 = the start of table sample 0 / PWM count 0; always below the loop
/// length), and the DWT cycle count (= the same 200 MHz) at that instant.
/// None while off, or when no try gave consistent reads (an interrupt
/// between them, a DMA update in flight).
///
/// DAC: the DMA has moved (L - NDTR) samples into DHR; the DAC output (DOR)
/// holds the one before the last moved (each TIM6 update copies DHR to DOR,
/// then requests the next sample), so it is table sample L - NDTR - 2 since
/// the update TIM6 CNT counts from. PWM: TIM12 CNT is the phase itself.
///
/// The reads are timed by the DWT reads around them (`reads`); nothing
/// else runs between those (a u64 division in that window made every try
/// fail on the device: the DAC path had no GEN phase, PR #64 device check).
/// The first consistent try whose window is under `WIN_MAX` cycles is
/// taken (a later one would put the instant further from the caller's next
/// DWT read: Scope::start allows 400 cycles).
pub fn now() -> Option<(u64, u32)> {
    const TRIES: u32 = 16;
    const WIN_MAX: u32 = 128;
    let (t, _) = running()?;
    let ps = t.psc as u64 + 1;
    let dac = t.path == Path::Dac;
    let mut wmin = u32::MAX;
    for _ in 0..TRIES {
        let (d0, n, c, n2, d1) = reads(dac);
        let w = d1.wrapping_sub(d0);
        wmin = wmin.min(w);
        // DAC: no DMA move between the two NDTR reads, and not right after
        // an update (the DMA may not have moved yet).
        if w >= WIN_MAX || (dac && (n != n2 || (c as u64 * ps) < 8)) {
            continue;
        }
        stat(true, w);
        let g = if dac {
            let l = t.len as u32;
            let idx = (3 * l - (n % (l + 1)) - 2) % l;
            idx as u64 * t.div() + c as u64 * ps
        } else {
            c as u64 * ps
        };
        return Some((g, d0.wrapping_add(w / 2)));
    }
    stat(false, wmin);
    None
}

/// One try's bus reads, DWT first and last and nothing between them but
/// the loads (no arithmetic: the window times the reads): (DWT, NDTR, CNT,
/// NDTR, DWT). PWM: (DWT, 0, TIM12 CNT, 0, DWT).
#[inline(never)]
fn reads(dac: bool) -> (u32, u32, u32, u32, u32) {
    // SAFETY: plain register reads (DWT, DMA0 stream 5, TIM6 / TIM12).
    unsafe {
        if dac {
            let d0 = r32(DWT_CYCCNT);
            let n1 = r32(S5 + 0x04);
            let c = r32(TIM6_CNT);
            let n2 = r32(S5 + 0x04);
            let d1 = r32(DWT_CYCCNT);
            (d0, n1 & 0xFFFF, c & 0xFFFF, n2 & 0xFFFF, d1)
        } else {
            let d0 = r32(DWT_CYCCNT);
            let c = r32(TIM12_CNT);
            let d1 = r32(DWT_CYCCNT);
            (d0, 0, c & 0xFFFF, 0, d1)
        }
    }
}

/// `DBG:GENTRIG:STAT?`: phase readings taken, failed (`now` gave None),
/// the last reading's window and the narrowest window of the last failed
/// call (DWT cycles; u32::MAX = none).
static mut STAT: [u32; 4] = [0, 0, 0, u32::MAX];

fn stat(ok: bool, w: u32) {
    // SAFETY: main loop only.
    unsafe {
        let s = &mut *core::ptr::addr_of_mut!(STAT);
        if ok {
            s[0] = s[0].wrapping_add(1);
            s[2] = w;
        } else {
            s[1] = s[1].wrapping_add(1);
            s[3] = w;
        }
    }
}

/// The counters `stat` keeps (see there).
pub fn stats() -> [u32; 4] {
    unsafe { *core::ptr::addr_of!(STAT) }
}

fn stop_outputs() {
    // SAFETY: main loop only.
    unsafe {
        *core::ptr::addr_of_mut!(RUNNING) = None;
        *core::ptr::addr_of_mut!(EPOCH) = epoch().wrapping_add(1);
    }
    unsafe {
        // Timers off, DAC off, then DMA off: a TIM6 trigger with the DAC's
        // DMA request on and the stream off sets DMA underrun (DAC_SR
        // DMAUDR1), which blocks the DAC's DMA requests until cleared (the
        // output stayed flat after an OFF / ON).
        modify32(TIM6, 1, 0);
        modify32(TIM12, 1, 0);
        w32(DAC_CR, r32(DAC_CR) & !0xFFFF);
        w32(S5, 0);
        while r32(S5) & 1 != 0 {}
        w32(DAC_SR, 1 << 13);
        // PA4 output low, PB14 / PB12 inputs.
        w32(GPIOA + 0x18, 1 << (4 + 16));
        modify32(GPIOA, 3 << 8, 1 << 8);
        modify32(GPIOB, (3 << 28) | (3 << 24), 0);
    }
}

impl Gen {
    pub fn new() -> Self {
        unsafe { modify32(RCU_APB1EN, 0, (1 << 29) | (1 << 4) | (1 << 6)) }; // DAC, TIM6, TIM12
        stop_outputs();
        let out = compute(&Settings::DEFAULT);
        Gen { s: Settings::DEFAULT, out_hz: out.hz() as f32, out, real_hz: 0.0 }
    }

    /// Actual (quantised) output frequency, Hz, also while off (UF-15).
    pub fn actual_hz(&self) -> f32 {
        self.out_hz
    }

    /// Actual output frequency in full precision, Hz.
    pub fn actual_hz_f64(&self) -> f64 {
        self.out.hz()
    }

    /// The set frequency is above the current wave's limit and is clamped
    /// (show `self.s.wave.max_note()`, UF-14).
    pub fn freq_clamped(&self) -> bool {
        self.out.freq_clamped
    }

    /// Apply new settings (FUN_080166a4). `s` is normalised
    /// (`Settings::normalised`: frequency kept as set within 1 Hz .. 10 MHz,
    /// Vpp 0..3 V, offset clamped around Vpp); the output runs at
    /// `out.timing`.
    pub fn apply(&mut self, s: Settings) {
        // compute() sees the raw set frequency, so a value above 10 MHz
        // still flags `freq_clamped`; the stored settings are normalised.
        self.out = compute(&s);
        self.out_hz = self.out.hz() as f32;
        let s = s.normalised();
        self.s = s;
        stop_outputs();
        if !s.on {
            self.real_hz = 0.0;
            return;
        }
        let t = self.out.timing;
        match t.path {
            Path::Pwm => self.pwm(&t),
            Path::Dac => self.dac(&s, &t),
        }
        self.real_hz = t.hz() as f32;
        // SAFETY: main loop only (the scope reads it from the main loop).
        unsafe { *core::ptr::addr_of_mut!(RUNNING) = Some((t, Shape::of(&s, &self.out))) };
    }

    fn pwm(&mut self, t: &Timing) {
        let ccr = plan::pwm_ccr(t, self.s.duty);
        unsafe {
            // PB14 AF9 (TIM12_CH1), PB12 = 1 (PWM path).
            modify32(GPIOB + 0x24, 0xF << 24, 9 << 24);
            modify32(GPIOB + 0x08, 3 << 28, 3 << 28);
            modify32(GPIOB, 3 << 28, 2 << 28);
            w32(GPIOB + 0x18, 1 << 12);
            modify32(GPIOB, 3 << 24, 1 << 24);

            w32(TIM12 + 0x28, t.psc as u32);
            w32(TIM12 + 0x2C, t.arr as u32);
            w32(TIM12 + 0x34, ccr);
            w32(TIM12 + 0x18, (6 << 4) | (1 << 3)); // CH1 PWM1, preload
            w32(TIM12 + 0x20, 1); // CC1E
            w32(TIM12 + 0x14, 1); // UG
            w32(TIM12, 1 | (1 << 7));
        }
    }

    fn dac(&mut self, s: &Settings, t: &Timing) {
        let wave = unsafe { &mut *core::ptr::addr_of_mut!(WAVE) };
        plan::fill(wave, s, t);
        let n = (t.len as usize).min(N_MAX);
        unsafe {
            // PA4 analog, PB12 = 0 (DAC path).
            modify32(GPIOA, 3 << 8, 3 << 8);
            w32(GPIOB + 0x18, 1 << (12 + 16));
            modify32(GPIOB, 3 << 24, 1 << 24);

            // TIM6: TRGO on update, 200 MHz / ((PSC+1)(ARR+1)).
            w32(TIM6 + 0x28, t.psc as u32);
            w32(TIM6 + 0x2C, t.arr as u32);
            w32(TIM6 + 0x04, 0x20);
            w32(TIM6 + 0x14, 1);

            // DMA0 stream 5, request 7: M->P, 16 bit, MINC, circular, high prio.
            w32(DMA_HIFCR, 0x3D << 6);
            w32(S5 + 0x08, DAC_DHR12R1);
            w32(S5 + 0x0C, core::ptr::addr_of!(WAVE) as u32);
            w32(S5 + 0x04, n as u32);
            w32(S5, (7 << 25) | (2 << 16) | (1 << 13) | (1 << 11) | (1 << 10) | (1 << 8) | (1 << 6) | 1);

            // DAC ch1: trigger TIM6 (TSEL 000), DMA, buffer off except square.
            let boff = if s.wave == Wave::Square { 0 } else { 1 << 1 };
            w32(DAC_CR, (r32(DAC_CR) & !0xFFFF) | (1 << 12) | (1 << 2) | boff | 1);
            w32(TIM6, 1);
        }
    }
}
