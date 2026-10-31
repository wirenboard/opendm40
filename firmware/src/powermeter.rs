//! Power meter (W): Vrms, Irms, P, S, Q, PF, f and Wh / VAh at once
//! (docs/power-meter-and-misconnect.md section 3). A child module of dmm.rs
//! (`Dmm::pw`), so it drives the HY3131 with the Dmm's own state.
//!
//! Assignment B of the doc: the current on the stock AC A path (OP1 on the
//! shunt -> AD2 = OP1O - PB3, the f4 / f5_6 row bytes), the voltage through
//! OP2 <- SENSE (x1, DC-coupled) -> AD3 = OP2O - AGND, with the divider
//! switch bytes R2A..R30 of a stock V row (`power_row`). The RMS block has
//! one accumulator, so sum(X*Y) (P), sum(Y^2) (V) and sum(X^2) (I) are
//! sequential: R34 is switched P -> V -> P -> I -> .. and `disc` results are
//! discarded after each switch (device test T3 sets it). The LPF follows
//! SDO23: the DC of I in the P / I phases, of V in the V phase. The HPF
//! takes the DC out of every RMS statistic, so the totals add it back:
//!   P = P_ac + V_dc I_dc,  V = hypot(V_ac, V_dc),  I = hypot(I_ac, I_dc).
//! With the burden compensation on (R_path of the shunt in use) the values
//! are the load's: P_L = P - I^2 R, V_L^2 = V^2 - 2 R P + R^2 I^2 (exact for
//! any waveform: v_L = v - R i), S_L = V_L I, Q = sqrt(S^2 - P^2).
//!
//! The fallback (`alt`, Settings / SCPI POW:METH ALT, for when device test T1
//! shows OP2 on SENSE unusable) alternates the stock AC V and AC A rows as
//! AC+DC does: Vrms, Irms (stock calibration), S, VAh; no P, no PF.
//!
//! Relays: PA state 0 (V / A) in both methods, and the shunt MOSFETs always
//! follow the current range (`self.mode` = 4 + the I unit), also while the
//! fallback's V phase writes the stock AC V row: the stock AC V row would
//! select the 47.5 ohm shunt with the load current flowing.
//!
//! Positive direction: current into the A jack, out of COM (the stock's DC A
//! sign) with V/ohm on the source side of the broken conductor (doc 3.6
//! wiring 1): a consumer reads P > 0, a source P < 0.

use super::*;

/// Main readout of the power page (a tap on it cycles P -> Wh -> VAh): the
/// display only; SCPI READ? / the Reading's SI value stay P (S in the
/// fallback).
pub const MAIN_P: u8 = 0;
pub const MAIN_WH: u8 = 1;
pub const MAIN_VAH: u8 = 2;

/// An energy in the unit that keeps 5 digits: mWh below 1, Wh to 999.99,
/// kWh above (VAh alike). Returns (value, decimals, unit).
pub fn energy_format(wh: f32, va: bool) -> (f32, u8, &'static str) {
    let m = wh.abs();
    // VAh has no milli step and its kilo step only from 100 kVAh ("mVAh" /
    // "kVAh" are wider than the unit box).
    let (k, u) = if m < 1.0 && !va {
        (1e3, "mWh")
    } else if m < 1000.0 || (va && m < 100_000.0) {
        (1.0, if va { "VAh" } else { "Wh" })
    } else {
        (1e-3, if va { "kVAh" } else { "kWh" })
    };
    let x = wh * k;
    let a = x.abs();
    let dec = if a < 10.0 {
        4
    } else if a < 100.0 {
        3
    } else if a < 1000.0 {
        2
    } else {
        1
    };
    (x, dec, u)
}

/// Function code of the power meter (`Dmm::func`, SCPI, persist): after
/// 16 / 17 = V / A AC+DC.
pub const FUNC: u8 = 18;

/// V ranges (60000 counts as the AC V rows; 1000 V clamp): /10, /100,
/// /1000, /10000 taps.
pub const V_LIM: [f32; 4] = [6.0, 60.0, 600.0, 1000.0];
pub const V_NAMES: [&str; 4] = ["6V", "60V", "600V", "1000V"];
/// I ranges by index k (low to high): k = 0 600 uA .. 5 10 A; unit iu =
/// 2 - k / 2 (0 A, 1 mA, 2 uA), OP1 gain ir = k % 2 (0 x10, 1 x1).
pub const I_LIM: [f32; 6] = [600e-6, 6e-3, 0.06, 0.6, 6.0, 10.0];
pub const I_NAMES: [&str; 6] = ["600uA", "6mA", "60mA", "600mA", "6A", "10A"];
/// Shunt per unit (ohm): 10 mohm (PE0), 0.5 ohm (PE1), 47.5 ohm (none).
pub const SHUNT_OHM: [f32; 3] = [0.01, 0.5, 47.5];
const KV: [f32; 4] = [10.0, 100.0, 1000.0, 10_000.0];
/// AD2 / AD3 / LPF code at IG 1.0: 1.25 V / 2^18.
const CODE: f32 = 1.25 / 262_144.0;

/// R34 statistics: ENAD3 + SVXI = sum(X*Y) (P); ENAD3 = sum(X^2) (I);
/// ENAD3 + SVXI + SDO23 = sum(Y^2) (V; the LPF then follows AD3).
pub const R34_XY: u8 = 0x88;
pub const R34_XX: u8 = 0x80;
pub const R34_YY: u8 = 0x8C;
/// The phase cycle of method B.
const CYCLE: [u8; 4] = [R34_XY, R34_YY, R34_XY, R34_XX];
/// R20 for the frequency counter: SCMPI = SENSE (the V tap), ENCMP (the
/// counter restart writes R20 = this, then this | ENCTR).
const R20_SENSE: u8 = 0x10;
/// Settle after a row write (ms): the stock AC A one (AC V 450).
const SETTLE_MS: u64 = 550;
/// Results discarded after an R34 switch (doc 3.4: 1..4; T3 decides).
pub const DISC_DEFAULT: u8 = 2;
/// Fallback: results taken per phase after the settle.
const ALT_N: u8 = 2;
/// Fallback V phase: the stock AC V node is AC-coupled (tau ~0.3 s), its
/// first results after a row write still carry the coupling step (the stock
/// AC+DC V phase: 600 ms, 4 results): settle 600 ms and drop 4 (emulator:
/// 2 left +1.8 % of the coupling step in Vrms at 230 V).
const ALT_V_SETTLE_MS: u64 = 600;
const ALT_V_SKIP: u8 = 4;
/// Continuous-current limit defaults (doc 3.6, 5): above I_CONT a countdown
/// of T_LIM s. The manual gives 10 A max behind one 10 A ceramic fuse and no
/// duty-cycle figure; 6 A is the top regular range (DM40C 6 A / 10 A).
pub const I_CONT_DEFAULT: f32 = 6.0;
pub const T_LIM_DEFAULT: u16 = 30;
/// Integration counts as running (auto power-off held) above this current.
const I_FLOOR: f32 = 50e-6;
/// DBG:POW:RAW? ring.
pub const RING_N: usize = 16;

/// The AC floors (doc 3.16), fractions of the channel's range: an AC part
/// (V_ac, I_ac) below its floor is noise, not a reading. V: the AUTO
/// coupling's "to DC" floor of the V functions (dmm.rs AUTO_FLOOR_V[1], 0.5
/// %; acdc doc 9). I: 0.5 %, not AUTO's 2 % (AUTO_FLOOR_A[1], sized for the
/// uA path's ~1.5 % RMS residue): 2 % blanked a real 0.1 A on the 6 A range
/// (powtest manual); the 10 mohm path's pure-DC residue is ~0.14 % in the
/// emulator; on the uA range a residue above 0.5 % still shows, as in the
/// stock AC A. AC coupling: a part below its floor reads 0 and PF / S / Q
/// are "--" (NaN, SCPI 9.91E37); f also "--" when the channel the counter
/// watches is below. AC+DC: the part is left out of the totals (Q 0 when
/// either is); PF "--" when S is at the noise level.
pub const FLOOR_V: f32 = AUTO_FLOOR_V[1];
pub const FLOOR_I: f32 = 0.005;
/// RMS noise assumed on an AC part above its floor, fractions of the range
/// (doc 3.16; the emulator's pure-DC residues are 0.025 % / 0.14 %, a few x
/// margin, the device's to be confirmed): uncorrelated noise adds to S but
/// not to P, so a resistive load shows Q = hypot(Vn I, V In) of pure noise
/// (first order of S^2 - P^2). Q below that reads 0.
pub const NOISE_V: f32 = 0.001;
pub const NOISE_I: f32 = 0.0025;

/// Coupling setting (`Pw::coup`, POW:COUP, persisted in 2 bits; doc 3.14),
/// the stock V / A trio:
/// * DC = one fixed row (`dc_row`): the current on AD1 (the stock DC A
///   input, 24 bit, factory DC A calibration) and at the same time the
///   voltage on OP2 <- SENSE -> AD3 (the power row's V channel, R34 frozen
///   at YY: its LPF = V_dc), no row writes while measuring. P = V I; PF / S /
///   Q / f not shown.
/// * AC = method B (the RMS / LPF row, P V P I on R34) with the AC parts
///   only: P = P_ac (sum X Y after the HPF), Vrms / Irms of the AC parts, S,
///   PF, Q, f; the DC parts (LPFs, their zeros) are not used.
/// * AC+DC = method B, the total: P = P_ac + V_dc I_dc, Vrms / Irms =
///   hypot(AC, DC).
/// The values are the persisted ones: 0 was "AC" (= AC+DC) and 2 the former
/// AUTO, which loads as AC+DC (`coup_of`); AC took the free value 3.
pub const COUP_ACDC: u8 = 0;
pub const COUP_DC: u8 = 1;
pub const COUP_AC: u8 = 3;

/// A stored / requested coupling value: 2 (the former AUTO) -> AC+DC.
pub fn coup_of(c: u32) -> u8 {
    match c & 3 {
        1 => COUP_DC,
        3 => COUP_AC,
        _ => COUP_ACDC,
    }
}

/// The next coupling of a tap on the active W button, as a tap on the active
/// V / A button steps DC -> AC -> AC+DC (`Dmm::press`).
pub fn next_coupling(c: u8) -> u8 {
    match c {
        COUP_DC => COUP_AC,
        COUP_AC => COUP_ACDC,
        _ => COUP_DC,
    }
}

/// The coupling's name (the settings row, the help line).
pub fn coup_name(c: u8) -> &'static str {
    match c {
        COUP_DC => "DC",
        COUP_AC => "AC",
        _ => "AC+DC",
    }
}
/// DC row: AD1 results dropped after a row write (its first chopped
/// conversion), Y (LPF / RMS) results dropped (the block in progress).
const DC_SKIP: u8 = 1;
/// The DC row's AD1 zero (POW:ZERO) may not differ more than this from the
/// stored one (codes; a current is flowing).
const ZERO_MAX_AD1: i32 = 20_000;

/// The DC row (doc 3.12): the power row of the ranges frozen at YY (OP2 <-
/// SENSE x1 -> AD3 = OP2O - AGND, LPF / RMS of Y; the I row's OP1 on the
/// shunt, AD2 = OP1O - PB3 as in B, so AD2IN = PB3 != AD3IN = AGND, note 8)
/// with AD1 as the stock DC A row of the current range sets it: R22 (OSR
/// 8192 chopped), R23 (ENAD1, buffers), R24 (AD1 = OP1O - PB3), R28 (refs
/// PB6 / AGND), R33 (OP1 chopper 2 kHz, the pre-filter). Every byte is the
/// power row's or the DC A row's (host test `dc_row_bytes`). Written by
/// `configure_dc`: AD1 last, so AD2 / AD3 never switch while AD1 converts
/// (note 7).
pub fn dc_row(vr: u8, k: u8) -> [u8; hy::CFG_LEN] {
    let (iu, ir) = iu_ir(k);
    let a = if iu == 0 { CFG_F7[ir as usize] } else { CFG_F8_9[ir as usize] };
    let mut b = power_row(vr, k, R34_YY);
    for j in [2usize, 3, 4, 8, 19] {
        b[j] = a[j];
    }
    b
}

/// Write the DC row: all three converters off with the row, then AD3, AD2
/// (the beat-fix order of `hy::configure`) and AD1 last (datasheet 13.1
/// note 7: AD2 / AD3 must not be switched while AD1 converts).
fn configure_dc(row: &[u8; hy::CFG_LEN]) {
    let mut c = *row;
    c[3] &= !0x80; // R23 ENAD1
    c[6] &= !0x80; // R26 ENAD2
    c[20] &= !0x80; // R34 ENAD3
    hy::write(0x20, &c);
    hy::write(0x1F, &[0]); // INTE
    hy::write(0x36, &[0]); // SPHACAL (no X*Y here)
    hy::write(0x34, &[row[20]]);
    hy::write(0x26, &[row[6]]);
    hy::write(0x23, &[row[3]]);
}

/// Self-zero (POW:ZERO) and V self-calibration (POW:VOLT:CAL) jobs: results
/// dropped after each row write, results averaged per step.
const JOB_SKIP: u8 = 1;
const JOB_N: u8 = 3;
pub const JOB_ZERO: u8 = 1;
pub const JOB_VCAL: u8 = 2;
/// POW:ZERO refuses a step whose sqrt(RMS) (an AC signal) or |LPF - the
/// old zero| (a DC signal; the device's raw zeros are ~150..1100 codes)
/// exceeds these (codes).
const ZERO_MAX_RMS: f32 = 400.0;
const ZERO_MAX_LPF: i32 = 4000;
/// POW:VOLT:CAL level window (fraction of the V range): below, the ratio is
/// resolution-limited; above (AC), the 6 V tap clips (doc 3.11 T1: the
/// op amp's +0.7 V headroom).
const VCAL_MIN: f32 = 0.1;
const VCAL_MAX_AC: f32 = 0.7;
const VCAL_MAX_DC: f32 = 1.0;
/// POW:VOLT:CAL peak guard (fraction of the V range), whatever the kind:
/// |DC| + sqrt(2) x the AC RMS (>= the peak of a sine or a square) must not
/// pass it, or the V channel clipped and the ratio is wrong (device
/// d8e249d: a 0..9.6 V square on 6 V, DC kind by its 4.8 V DC part, was
/// accepted, gain 1.021410).
const VCAL_PEAK_MAX: f32 = 1.0;
/// V cal, AC reference: the stock AC V node is AC-coupled (tau ~0.3 s) and
/// starts from the power row's OP1O level: 2 s (> 6 tau) before its results
/// (the emulator: +0.46 % with the fallback's 0.6 s).
const VCAL_AC_SETTLE_MS: u64 = 2000;
/// Agreement of the two rounds (ratio, and the raw level: steadiness).
const VCAL_SPREAD: f32 = 0.003;
/// Job results (`Pw::job_res`).
pub const JR_NONE: u8 = 0;
pub const JR_RUN: u8 = 1;
pub const JR_OK: u8 = 2;
/// Zero: a signal / current is present.
pub const JR_SIGNAL: u8 = 3;
pub const JR_SMALL: u8 = 4;
pub const JR_BIG: u8 = 5;
pub const JR_UNSTABLE: u8 = 6;
/// V cal: the ratio is outside 0.9..1.1 (wrong range / wiring).
pub const JR_RANGE: u8 = 7;
pub const JR_ABORT: u8 = 8;
/// The fallback method (VA only) has no OP2 channel to calibrate / zero.
pub const JR_METHOD: u8 = 9;

/// The V channel's gain from the factory calibration (no self-cal stored):
/// 60 / 600 / 1000 V use the same divider taps as the stock AC V r1..r3
/// (OP1 x1 -> AD2 there, OP2 x1 -> AD3 here), so their AC V RMS gains
/// (cal 0x38..0x40) apply; 6 V uses the DC V r0 /10 tap (PA7, AD1 there):
/// its DC V gain (0x04) times the AD2-path / AD1-path ratio measured on the
/// 60 V tap by both calibrations (0x38 / 0x08). On the dumped unit:
/// 1.02065 / 1.01614 / 1.00453 / 0.99815 against the device's T1 needs
/// 1.0209 / 1.0170 / 1.0033 (doc 3.11): residuals -0.03 / -0.08 / +0.12 %.
/// A missing or implausible record gives 1.0.
pub fn v_gain_factory(vr: u8) -> f32 {
    let g = match vr.min(3) {
        0 => cal(0x04, 1.0) * cal(0x38, 1.0) / cal(0x08, 1.0),
        r => cal(0x34 + 4 * r as u32, 1.0),
    };
    if (0.9..=1.1).contains(&g) { g } else { 1.0 }
}

/// Comparator thresholds relative to AGND for R21 (registers.md mux tables;
/// REFO - AGND = 1.25 V, VDDA = 3.6 V for the VDSC taps, which only fill
/// the levels the AGNDP / AGNDN taps lack). (code, volts).
const CMP_RH: [(u8, f32); 12] = [
    (0xF, -0.0208),
    (0xE, 0.0),
    (0xD, 0.0208),
    (0xC, 0.0417),
    (0xB, 0.0625),
    (0xA, 0.1042),
    (0x9, 0.2083),
    (0x8, 0.4167),
    (0x1, 0.8),
    (0x0, 1.6),
    (0x5, -0.2),
    (0x6, -0.4),
];
const CMP_RL: [(u8, f32); 12] = [
    (0x8, -0.4167),
    (0x9, -0.2083),
    (0xA, -0.1042),
    (0xB, -0.0625),
    (0xC, -0.0417),
    (0xD, -0.0208),
    (0xE, 0.0),
    (0xF, 0.0208),
    (0x1, -0.8),
    (0x0, -1.6),
    (0x5, 0.2),
    (0x6, 0.4),
];
/// The default window (+-20.8 mV, the stock AC rows' R21).
pub const R21_DEFAULT: u8 = 0xDD;
/// R20 for the counter on the current (PB4: OP1O coupled, as the stock
/// AC A rows' 0xF2): the fallback source when the V tap has no window.
const R20_PB4: u8 = 0xF0;

/// R21 for a DC-coupled comparator input at `c` V (DC) with `a` V rms of AC
/// on it (doc 3.11: the T1 square was 0..96 mV on the 60 V tap: it never
/// crossed the default -20.8 mV, f = 0): the nearest high threshold above
/// c + 6 mV and low threshold below c - 6 mV, both inside the swing (every
/// usual waveform reaches at least its rms around its mean). None = no
/// window (pure DC, too small an AC part).
pub fn cmp_window(c: f32, a: f32) -> Option<u8> {
    // 6 mV: an un-zeroed unit's Y phantom (~5 mV at the tap) keeps a
    // bipolar signal on the default window.
    const M: f32 = 0.006;
    let mut rh: Option<(u8, f32)> = None;
    for &(k, v) in &CMP_RH {
        if v >= c + M && rh.is_none_or(|(_, b)| v < b) {
            rh = Some((k, v));
        }
    }
    let mut rl: Option<(u8, f32)> = None;
    for &(k, v) in &CMP_RL {
        if v <= c - M && rl.is_none_or(|(_, b)| v > b) {
            rl = Some((k, v));
        }
    }
    let ((h, vh), (l, vl)) = (rh?, rl?);
    if vh < c + 0.9 * a && vl > c - 0.9 * a { Some(h << 4 | l) } else { None }
}

/// R36 for a signed SPHACAL step count: > 0 = AD2 (the current) leads
/// (bit 7), < 0 = AD2 lags; |steps| <= 127.
pub fn r36_of(steps: i8) -> u8 {
    let n = (steps as i16).unsigned_abs().min(127) as u8;
    if steps > 0 { 0x80 | n } else { n }
}

/// (unit, op gain index) of current range k.
pub fn iu_ir(k: u8) -> (u8, u8) {
    let k = k.min(5);
    (2 - k / 2, k % 2)
}

/// The power row (R20..R35) of doc 3.1 / 3.10: the stock AC A row of the
/// current range with OP2 <- SENSE x1 -> AD3 = OP2O - AGND, the V range's
/// switch bytes, the RMS / LPF at LPFBW 111 and the comparator on SENSE.
/// `stat`: R34 (R34_XY / _XX / _YY). Every byte is the I row's, the V row's
/// or one of the listed overrides (host test `power_row_bytes`).
pub fn power_row(vr: u8, k: u8, stat: u8) -> [u8; hy::CFG_LEN] {
    let (iu, ir) = iu_ir(k);
    let mut b = if iu == 0 { CFG_F4[ir as usize] } else { CFG_F5_6[ir as usize] };
    // V rows: 6 V is the DC V r0 /10 tap (PA7; the AC V r0 trick, /100 with
    // OP1 x10, is not needed on OP2), the others the AC V r1..r3 taps.
    let v = if vr == 0 { &CFG_F1[0] } else { &CFG_F0[vr.min(3) as usize] };
    b[0] = R20_SENSE | 0x02; // R20: SCMPI SENSE, ENCMP, ENCTR
    b[5] &= !0x02; // R25: OPS2 = 0 (OP2 x1), OPS1 as the I range
    b[9] = 0xF8; // R29: ENRMS ENLPF LPFBW 111, no peak hold
    b[10..17].copy_from_slice(&v[10..17]); // R2A..R30: divider taps, SMODE, ACC
    b[18] = 0x80 | (b[18] & 0x0F); // R32: ENOP2, SOP2P = SENSE; OP1 as the I row
    b[20] = stat; // R34
    b[21] = 0x54; // R35: AD3 = OP2O - AGND, AD3IG 1.0
    b
}

/// Fallback rows: the stock AC V row of the V range, the stock AC A row of
/// the current range.
pub fn alt_row(v_phase: bool, vr: u8, k: u8) -> [u8; hy::CFG_LEN] {
    let (iu, ir) = iu_ir(k);
    if v_phase {
        CFG_F0[vr.min(3) as usize]
    } else if iu == 0 {
        CFG_F4[ir as usize]
    } else {
        CFG_F5_6[ir as usize]
    }
}

/// What a power reading carries (`Reading::power`), SI units; the burden
/// compensation already applied when `comp`.
#[derive(Clone, Copy, PartialEq)]
pub struct PowerVals {
    pub p: f32,
    pub s: f32,
    pub q: f32,
    /// P / S, signed with P (0 when S = 0); NaN in the fallback.
    pub pf: f32,
    pub v: f32,
    pub i: f32,
    pub vdc: f32,
    pub idc: f32,
    pub f_hz: f32,
    /// I x R_path: the meter's own drop inside the V reading.
    pub burden_v: f32,
    pub e_wh: f32,
    pub vah: f32,
    pub secs: u32,
    /// Mean P over the integration time (Wh / h; NaN before any).
    pub p_avg: f32,
    pub vr: u8,
    pub ik: u8,
    pub v_auto: bool,
    pub i_auto: bool,
    /// Fallback (VA only): no P / Q / PF.
    pub alt: bool,
    /// The DC (AD1) method in force: P = V I; PF / S / Q / f not shown.
    pub dc: bool,
    /// Coupling setting (COUP_*).
    pub coup: u8,
    pub comp: bool,
    /// Energy integration running / paused.
    pub run: bool,
    pub v_ol: bool,
    pub i_ol: bool,
    /// Continuous-current limit: 0 below, 1 counting down (`over_left` s),
    /// 2 expired.
    pub over: u8,
    pub over_left: u16,
    /// The SPHACAL in force (steps).
    pub sph: i8,
    /// The main readout (MAIN_*: P, Wh, VAh; a tap on it cycles).
    pub main: u8,
    /// Coupling hint (HINT_*, doc 3.16): fed to the sentinel per reading.
    pub hint: u8,
    /// AC+DC parts (doc 3.17, the sub-lines): [DC, AC] of V, I, P as
    /// measured (V_dc / I_dc the LPFs, V_ac / I_ac the RMS, P_dc = V_dc I_dc,
    /// P_ac the X*Y RMS), the AC floors applied (a part below reads 0, P_ac 0
    /// without both), "At load" applied to V and P as to the totals (v_L = v -
    /// R i per part: V_dc - R I_dc, sqrt(V_ac^2 - 2 R P_ac + R^2 I_ac^2);
    /// P - R I^2 per part), so the parts add up to the totals shown
    /// (P = P_dc + P_ac, V^2 = V_dc^2 + V_ac^2). Zero outside AC+DC.
    pub vp: [f32; 2],
    pub ip: [f32; 2],
    pub pp: [f32; 2],
}

/// The toast of an alert; in the power meter the DC? / AC? hints are the
/// coupling hints (doc 3.16).
pub fn hint_toast(a: Alert, pw_on: bool) -> &'static str {
    match a {
        Alert::Dc if pw_on => HINT_TOASTS[0],
        Alert::Ac if pw_on => HINT_TOASTS[1],
        _ => a.toast(),
    }
}
pub const HINT_TOASTS: [&str; 2] = ["No AC: DC is more precise", "AC present: use AC+DC"];

/// AC+DC with no AC part on V and I (and a DC signal): DC is more precise.
pub const HINT_NO_AC: u8 = 1;
/// DC with an AC part on V: P = V_dc I_dc misses the AC power.
pub const HINT_AC: u8 = 2;

impl PowerVals {
    pub const ZERO: PowerVals = PowerVals {
        p: 0.0,
        s: 0.0,
        q: 0.0,
        pf: 0.0,
        v: 0.0,
        i: 0.0,
        vdc: 0.0,
        idc: 0.0,
        f_hz: 0.0,
        burden_v: 0.0,
        e_wh: 0.0,
        vah: 0.0,
        secs: 0,
        p_avg: f32::NAN,
        vr: 3,
        ik: 5,
        v_auto: true,
        i_auto: true,
        alt: false,
        dc: false,
        coup: COUP_DC,
        comp: false,
        run: true,
        v_ol: false,
        i_ol: false,
        over: 0,
        over_left: 0,
        sph: 0,
        main: MAIN_P,
        hint: 0,
        vp: [0.0; 2],
        ip: [0.0; 2],
        pp: [0.0; 2],
    };
}

/// One RMS / LPF result as read (DBG:POW:RAW?, device test T3).
#[derive(Clone, Copy)]
pub struct Raw {
    /// R34 in force (fallback: 1 = V row, 2 = I row).
    pub stat: u8,
    pub disc: bool,
    pub rms: i64,
    pub lpf: i32,
    /// ms since the previous result.
    pub dt: u16,
}

/// Energy accumulator: whole mJ in an i64 plus the f32 remainder (an f32
/// alone stops adding mW s to kWh after a few hours).
#[derive(Clone, Copy)]
struct Acc {
    mj: i64,
    frac: f32,
}

impl Acc {
    const ZERO: Acc = Acc { mj: 0, frac: 0.0 };
    fn add(&mut self, joules: f32) {
        let x = self.frac + joules * 1000.0;
        let w = x as i64;
        self.mj += w;
        self.frac = x - w as f32;
    }
    fn wh(&self) -> f32 {
        ((self.mj as f64 + self.frac as f64) / 3.6e6) as f32
    }
}

pub struct Pw {
    pub on: bool,
    /// Method: false = simultaneous (P, PF), true = fallback (VA only).
    pub alt: bool,
    /// Coupling setting (COUP_*) and the DC method in force (= coup DC).
    pub coup: u8,
    pub dc: bool,
    /// DC row: Y results still to drop.
    skip_y: u8,
    pub vr: u8,
    pub ik: u8,
    pub v_auto: bool,
    pub i_auto: bool,
    /// Index in CYCLE (fallback: 0 V phase, 1 I phase).
    phase: u8,
    /// Results still to drop in this phase / taken in it (fallback).
    skip: u8,
    taken: u8,
    /// Results discarded after an R34 switch (DBG:POW:DISC, persisted).
    pub disc: u8,
    /// R34 in force.
    stat: u8,
    /// Parts measured since the last (re)start: 1 P, 2 V, 4 I.
    seen: u8,
    p_ac: f32,
    v_ac: f32,
    v_dc: f32,
    i_ac: f32,
    i_dc: f32,
    /// DC coupling: the V channel's AC part (the DC row's R34 YY RMS, read
    /// anyway; no register write) for the "AC present" hint only.
    vac_dc: f32,
    /// P of the latest accepted result (unfiltered, for the energy).
    p_last: f32,
    /// Totals of the latest reading: P, S for the next energy interval.
    p_tot: f32,
    s_last: f32,
    /// Energy integration.
    pub run: bool,
    e: Acc,
    va: Acc,
    t_int: u64,
    t_last: Option<u64>,
    /// SPHACAL (steps, signed), burden R_path per unit (ohm), compensation,
    /// continuous-current limit (A, s; t_lim 0 = none).
    pub sph: i8,
    pub r_path: [f32; 3],
    /// "At load" (doc 3.16): off by default, V and P as at the jacks.
    pub comp: bool,
    pub i_cont: f32,
    pub t_lim: u16,
    over_t0: Option<u64>,
    over_beeped: bool,
    /// Phase calibration (POW:PHAS:CAL): 0 off, 1 / 2 averaging at s0 /
    /// s0 + delta; sums of P_ac / (V_ac I_ac).
    cal: u8,
    cal_n: u8,
    cal_acc: f32,
    cal_s0: i8,
    cal_pa: f32,
    /// The last calibration's outcome (CAL_*; POW:PHAS:CAL?).
    pub cal_res: u8,
    /// The calibration moved SPHACAL: rewrite the row (`pw_service`).
    cal_req: bool,
    /// DBG: R34 frozen (0 = cycle), row byte overrides, raw ring.
    pub freeze: u8,
    pub poke: [(u8, u8); 4],
    pub poke_n: u8,
    ring: [Raw; RING_N],
    ring_k: u32,
    t_res: u64,
    /// The row last written and its R36 (DBG:POW:ROW?).
    pub row: [u8; hy::CFG_LEN],
    pub r36: u8,
    /// Range changes / restarts since entry (energy gaps).
    pub restarts: u32,
    /// The main readout (MAIN_*), kept over entries (not persisted).
    pub main: u8,
    /// Parts kept over the next `pw_apply` (2 V, 4 I: the other channel's
    /// range changed, its scale did not).
    pub(super) keep: u8,
    /// V channel gain per range (raw x gain = V), 0 = `v_gain_factory`;
    /// set by POW:VOLT:CAL, persisted.
    pub vgain: [f32; 4],
    /// LPF zeros (codes; POW:ZERO, persisted when `zero_ok`): X per current
    /// range, Y once (after the divider: the same on every V range).
    pub zx: [i16; 6],
    pub zy: i16,
    /// The DC row's AD1 zeros per current range (codes; POW:ZERO).
    pub zd: [i16; 6],
    pub zero_ok: bool,
    /// The wiring dialog was confirmed once (the W button enters directly
    /// afterwards; persisted).
    pub wired: bool,
    /// Running job (JOB_ZERO / JOB_VCAL, 0 none), its step, results taken,
    /// sums; the V cal's reference kind (DC V vs AC V) and its two rounds;
    /// the ranges to restore; the outcome (JR_*) and its V range.
    pub job: u8,
    pub job_step: u8,
    job_n: u8,
    job_a: f32,
    job_b: f32,
    job_dc: bool,
    job_raw: [f32; 2],
    job_ref: [f32; 2],
    job_zx: [i16; 6],
    job_zd: [i16; 6],
    job_save: (u8, u8, bool, bool),
    pub job_res: u8,
    pub job_vr: u8,
    /// The kind of the last job started (its `job_res`).
    pub job_last: u8,
    /// Counter source / comparator window (R20 without ENCTR, R21) and
    /// whether the frequency is meaningful (`pw_cmp_select`); the V range
    /// the window was chosen for.
    pub r20: u8,
    pub r21: u8,
    f_ok: bool,
    cmp_vr: u8,
}

impl Pw {
    pub const fn new() -> Self {
        Pw {
            on: false,
            alt: false,
            coup: COUP_DC,
            dc: false,
            skip_y: 0,
            vr: 3,
            ik: 5,
            v_auto: true,
            i_auto: true,
            phase: 0,
            skip: 0,
            taken: 0,
            disc: DISC_DEFAULT,
            stat: R34_XY,
            seen: 0,
            p_ac: 0.0,
            v_ac: 0.0,
            v_dc: 0.0,
            i_ac: 0.0,
            i_dc: 0.0,
            vac_dc: 0.0,
            p_last: 0.0,
            p_tot: 0.0,
            s_last: 0.0,
            run: true,
            e: Acc::ZERO,
            va: Acc::ZERO,
            t_int: 0,
            t_last: None,
            sph: 0,
            r_path: SHUNT_OHM,
            comp: false,
            i_cont: I_CONT_DEFAULT,
            t_lim: T_LIM_DEFAULT,
            over_t0: None,
            over_beeped: false,
            cal: 0,
            cal_n: 0,
            cal_acc: 0.0,
            cal_s0: 0,
            cal_pa: 0.0,
            cal_res: CAL_NONE,
            cal_req: false,
            freeze: 0,
            poke: [(0, 0); 4],
            poke_n: 0,
            ring: [Raw { stat: 0, disc: false, rms: 0, lpf: 0, dt: 0 }; RING_N],
            ring_k: 0,
            t_res: 0,
            row: [0; hy::CFG_LEN],
            r36: 0,
            restarts: 0,
            main: MAIN_P,
            keep: 0,
            vgain: [0.0; 4],
            zx: [0; 6],
            zy: 0,
            zd: [0; 6],
            zero_ok: false,
            wired: false,
            job: 0,
            job_step: 0,
            job_n: 0,
            job_a: 0.0,
            job_b: 0.0,
            job_dc: false,
            job_raw: [0.0; 2],
            job_ref: [0.0; 2],
            job_zx: [0; 6],
            job_zd: [0; 6],
            job_save: (3, 5, true, true),
            job_res: JR_NONE,
            job_vr: 0,
            job_last: 0,
            r20: R20_SENSE,
            r21: R21_DEFAULT,
            f_ok: true,
            cmp_vr: 255,
        }
    }

    /// The V gain in force for range `vr`.
    pub fn v_gain(&self, vr: u8) -> f32 {
        let g = self.vgain[vr.min(3) as usize];
        if g > 0.0 { g } else { v_gain_factory(vr) }
    }

    /// Zero Wh / VAh / time (POW:ENER:RES, the energy box).
    pub fn energy_reset(&mut self) {
        self.e = Acc::ZERO;
        self.va = Acc::ZERO;
        self.t_int = 0;
        self.t_last = None;
    }

    /// Energy integration running with a current above the floor: the app
    /// holds the idle power-off (and dimming) off.
    pub fn integrating(&self) -> bool {
        self.on && self.run && self.seen & 4 != 0 && hypot(self.i_ac, self.i_dc) > I_FLOOR
    }

    /// The raw ring, oldest first: "n;stat,d,rms,lpf,dt;..." (DBG:POW:RAW?).
    pub fn ring_text(&self, buf: &mut [u8]) -> usize {
        let mut at = 0;
        let mut put = |b: &[u8], at: &mut usize| {
            let k = b.len().min(buf.len() - *at);
            buf[*at..*at + k].copy_from_slice(&b[..k]);
            *at += k;
        };
        let mut t = [0u8; 24];
        put(i64_text(&mut t, self.ring_k as i64), &mut at);
        let n = (self.ring_k as usize).min(RING_N);
        for j in 0..n {
            let e = &self.ring[(self.ring_k as usize + RING_N - n + j) % RING_N];
            put(b";", &mut at);
            let st: &[u8] = match e.stat {
                R34_XY => b"XY",
                R34_XX => b"XX",
                R34_YY => b"YY",
                1 => b"V",
                2 => b"I",
                3 => b"DV",
                4 => b"DI",
                _ => b"?",
            };
            put(st, &mut at);
            put(if e.disc { b",1," } else { b",0," }, &mut at);
            put(i64_text(&mut t, e.rms), &mut at);
            put(b",", &mut at);
            put(i64_text(&mut t, e.lpf as i64), &mut at);
            put(b",", &mut at);
            put(i64_text(&mut t, e.dt as i64), &mut at);
        }
        at
    }
}

/// The power meter's state lives in SRAM (.sram_bss), not in the App on the
/// TCM stack: `Dmm::new` writes it once (`pw_static`).
#[cfg_attr(target_os = "none", link_section = ".sram_bss")]
static mut PW: core::mem::MaybeUninit<Pw> = core::mem::MaybeUninit::uninit();

/// Initialise and hand out the one `Pw` (call once: `Dmm::new`).
pub fn pw_static() -> &'static mut Pw {
    unsafe { (*core::ptr::addr_of_mut!(PW)).write(Pw::new()) }
}

/// Decimal text of `v` (DBG replies).
pub fn i64_text(t: &mut [u8; 24], v: i64) -> &[u8] {
    let mut d = [0u8; 24];
    let mut k = 24;
    let mut u = v.unsigned_abs();
    loop {
        k -= 1;
        d[k] = b'0' + (u % 10) as u8;
        u /= 10;
        if u == 0 {
            break;
        }
    }
    if v < 0 {
        k -= 1;
        d[k] = b'-';
    }
    let n = 24 - k;
    t[..n].copy_from_slice(&d[k..]);
    &t[..n]
}

/// The (P, V, I) totals and load values of doc 3.3 from the parts, with the
/// burden compensation `r` (ohm, 0 = off). Returns (P, S, Q, PF, V, I).
pub fn totals(p_ac: f32, v_ac: f32, v_dc: f32, i_ac: f32, i_dc: f32, r: f32) -> (f32, f32, f32, f32, f32, f32) {
    let p_m = p_ac + v_dc * i_dc;
    let v_m = hypot(v_ac, v_dc);
    let i = hypot(i_ac, i_dc);
    let p = p_m - i * i * r;
    let v2 = v_m * v_m - 2.0 * r * p_m + r * r * i * i;
    let v = sqrt(v2.max(0.0));
    let s = v * i;
    // |P| can exceed S by the noise of the separate statistics: PF clamped.
    // S 0: no PF (NaN, not 0 = "purely reactive"; #109).
    let pf = if s > 0.0 { (p / s).clamp(-1.0, 1.0) } else { f32::NAN };
    let q = sqrt((s * s - p * p).max(0.0));
    (p, s, q, pf, v, i)
}

/// EMA with a relative jump: |x - y| > 5 % of max(|x|, |y|, `floor`) jumps.
fn ema_rel(a: f32, floor: f32, x: f32, y: &mut f32) {
    let m = x.abs().max(y.abs()).max(floor);
    ema(0.05 * m, a, x, y);
}

impl Dmm {
    /// Enter the power meter (SCPI CONF:POW, the wiring dialog's OK): both
    /// channels auto, V from 600 V (mains in range at once; the stock AC V
    /// starts at 6 V), I from 10 A (the 10 mohm shunt: the least burden).
    pub fn pw_enter(&mut self, clk: &mut Clock) {
        self.ovl_clear(clk.now_ms());
        // The A-jack protection latch ends: the meter measures through A
        // on purpose now (its own ranging / OVERLOAD, doc 4.8).
        if self.aprot.take().is_some() {
            self.sent.set_aprot(clk.now_ms(), false);
        }
        self.probe = false;
        self.acdc = false;
        self.cpl_auto = false;
        self.manual = false;
        let pw = &mut *self.pw;
        pw.on = true;
        pw.v_auto = true;
        pw.i_auto = true;
        pw.vr = 2;
        pw.ik = 5;
        pw.run = true;
        pw.energy_reset();
        pw.over_t0 = None;
        pw.cal = 0;
        pw.restarts = 0;
        pw.job = 0;
        pw.cal_req = false;
        pw.keep = 0;
        pw.cmp_vr = 255;
        // Entering starts in DC, as V / A (doc 3.16); a restart in the
        // power meter restores its coupling afterwards (`App::restore`).
        pw.coup = COUP_DC;
        pw.dc = true;
        self.mode = 255;
        self.reset_values();
        self.entry = clk.now_ms();
        self.pw_apply(clk);
    }

    /// Leave the power meter (the next `set` does it: any function change).
    pub(super) fn pw_leave(&mut self) {
        self.pw_job_end(JR_ABORT);
        self.pw.on = false;
        self.pw.freeze = 0;
        self.pw.poke_n = 0;
    }

    /// (Re)write the row for the current ranges / SPHACAL / method, restart
    /// the phase machine, settle. The parts measured so far are dropped (the
    /// scale changed); the energy keeps integrating the last P over the gap.
    pub fn pw_apply(&mut self, clk: &mut Clock) {
        let pw = &mut *self.pw;
        // After a range change of one channel its own statistic comes first
        // (B: XX / YY, DC: the I / V row), so an autorange hunt costs one
        // phase per step (~0.9 s from the settle), not a whole P V P I set.
        pw.phase = match (pw.keep, pw.dc, pw.alt) {
            (2, false, false) => 3,
            (4, false, false) => 1,
            _ => 0,
        };
        // A range change of one channel keeps the other's parts (its scale
        // did not change): the page keeps V while I autoranges (doc 3.11).
        pw.seen &= pw.keep & !1;
        pw.keep = 0;
        pw.taken = 0;
        if pw.cmp_vr != pw.vr {
            // The window was for another tap: back to the default until the
            // next V result (`pw_cmp_select`).
            pw.cmp_vr = pw.vr;
            pw.r20 = R20_SENSE;
            pw.r21 = R21_DEFAULT;
            pw.f_ok = true;
        }
        pw.skip = if pw.dc { DC_SKIP } else if pw.alt { ALT_V_SKIP } else { 0 };
        pw.skip_y = DC_SKIP;
        let settle = SETTLE_MS;
        pw.restarts = pw.restarts.wrapping_add(1);
        self.write_config(clk);
        self.deadline = clk.now_ms() + settle;
        // The gate running now spans the row write and the settle: the
        // first counter access past the settle restarts it, its result
        // comes 350 ms later (`freq_restart_fresh`).
        self.freq.fresh = true;
    }

    /// `write_config` in the power meter: quiesce, PA relay state 0, the
    /// shunt of the current range, the row (method B: with SPHACAL before
    /// the enables), no sentinel (the comparator counts the V frequency).
    pub(super) fn pw_write_config(&mut self, clk: &mut Clock) {
        let (iu, ir) = iu_ir(self.pw.ik);
        self.mode = 4 + iu;
        self.range = ir as i8;
        let pw = &mut *self.pw;
        pw.stat = match pw.job {
            JOB_ZERO if pw.job_step < 6 => R34_XX,
            JOB_ZERO if pw.job_step < 12 => R34_YY,
            JOB_ZERO | JOB_VCAL => R34_YY,
            _ if pw.freeze != 0 => pw.freeze,
            _ => CYCLE[pw.phase as usize & 3],
        };
        // V cal, odd steps: the stock reference row of the range (DC V on
        // AD1 or AC V), with the power meter's shunt kept (below).
        let vref = pw.job == JOB_VCAL && pw.job_step & 1 == 1;
        // The DC row: the DC coupling, and the self-zero's AD1 steps (6..11).
        let dcm = (pw.dc && pw.job == 0) || (pw.job == JOB_ZERO && (6..12).contains(&pw.job_step));
        let stock = pw.alt && !dcm || vref;
        let mut row = if vref {
            if pw.job_dc { CFG_F1[pw.vr.min(3) as usize] } else { CFG_F0[pw.vr.min(3) as usize] }
        } else if dcm {
            dc_row(pw.vr, pw.ik)
        } else if pw.alt {
            alt_row(pw.phase & 1 == 0, pw.vr, pw.ik)
        } else {
            power_row(pw.vr, pw.ik, pw.stat)
        };
        if !stock && !dcm {
            // The counter's source and window (`pw_cmp_select`).
            row[0] = pw.r20 | 0x02;
            row[1] = pw.r21;
            for &(i, v) in &pw.poke[..pw.poke_n as usize] {
                if (i as usize) < row.len() {
                    row[i as usize] = v;
                }
            }
            pw.stat = row[20];
        }
        pw.row = row;
        pw.r36 = if stock || dcm { 0 } else { r36_of(pw.sph) };
        self.avg.reset();
        self.t_rd = None;
        self.t_conv = None;
        hy::quiesce();
        if self.pa_now != Some(0) {
            relay_pa(clk, 0);
            self.pa_now = Some(0);
        }
        relay_pe(iu);
        pc7(false);
        if dcm {
            configure_dc(&row);
        } else if stock {
            hy::configure(&row);
        } else {
            hy::configure_ph(&row, self.pw.r36);
        }
        self.r22 = row[2];
        self.r29 = row[9];
        self.chk.copy_from_slice(&row[super::CHK_AT..super::CHK_AT + 8]);
        self.sent_mode = 255;
        self.sent.configured(Kind::Off, clk.now_ms(), true);
        self.diag_on = false;
    }

    /// Restart after the DMM rail came back (`start`).
    pub(super) fn pw_restart(&mut self, clk: &mut Clock) {
        self.pa_now = None;
        self.pw_apply(clk);
    }

    /// Ranges: `v` / `i` None = keep, Some(None) = AUTO, Some(Some(r)) =
    /// that range (V 0..3, I index 0..5).
    pub fn pw_set_ranges(&mut self, clk: &mut Clock, v: Option<Option<u8>>, i: Option<Option<u8>>) {
        if i.is_some() {
            self.ovl_clear(clk.now_ms());
        }
        if self.pw.job != 0 {
            self.pw_job_end(JR_ABORT);
        }
        let pw = &mut *self.pw;
        let (vr, ik) = (pw.vr, pw.ik);
        match v {
            Some(None) => pw.v_auto = true,
            Some(Some(r)) => {
                pw.v_auto = false;
                pw.vr = r.min(3);
            }
            None => {}
        }
        match i {
            Some(None) => pw.i_auto = true,
            Some(Some(r)) => {
                pw.i_auto = false;
                pw.ik = r.min(5);
            }
            None => {}
        }
        self.manual = !(pw.v_auto && pw.i_auto);
        if pw.on && (pw.vr, pw.ik) != (vr, ik) {
            pw.keep = if pw.vr == vr { 2 } else if pw.ik == ik { 4 } else { 0 };
            self.pw_apply(clk);
        }
    }

    /// SPHACAL (POW:PHAS): rewritten with the row (datasheet order).
    pub fn pw_set_phase(&mut self, clk: &mut Clock, steps: i8) {
        self.pw.sph = steps.max(-127);
        if self.pw.on {
            self.pw_apply(clk);
        }
    }

    /// Method B (false) or the fallback (true).
    pub fn pw_set_method(&mut self, clk: &mut Clock, alt: bool) {
        if alt != self.pw.alt {
            self.pw.alt = alt;
            self.pw.freeze = 0;
            if self.pw.on {
                self.pw_apply(clk);
            }
        }
    }

    /// Phase self-calibration on a reactive load (PF ~ 0, any sign): P / S
    /// averaged at the current SPHACAL and 48 steps further, then the
    /// Newton step that nulls it. Result in `pw.cal_res`. Needs method B
    /// (the DC method and the VA-only fallback have no phase).
    pub fn pw_cal_start(&mut self) -> bool {
        let pw = &mut *self.pw;
        if !pw.on || pw.alt || pw.dc {
            return false;
        }
        pw.cal = 1;
        pw.cal_n = 0;
        pw.cal_acc = 0.0;
        pw.cal_s0 = pw.sph;
        pw.cal_res = CAL_RUN;
        true
    }

    /// Coupling (POW:COUP, the settings page, the W button): DC, AC or
    /// AC+DC; the row follows at once (AC <-> AC+DC: the same row, only the
    /// shown values change).
    pub fn pw_set_coupling(&mut self, clk: &mut Clock, coup: u8) {
        if self.pw.job != 0 {
            self.pw_job_end(JR_ABORT);
        }
        let pw = &mut *self.pw;
        pw.coup = coup_of(coup as u32);
        let dc = pw.coup == COUP_DC;
        if pw.on && (dc != pw.dc || pw.job_res == JR_ABORT) {
            pw.dc = dc;
            pw.keep = 0;
            self.pw_apply(clk);
        }
    }

    /// DBG:POW:STAT: freeze R34 (0 = cycle P / V / P / I again).
    pub fn pw_freeze(&mut self, clk: &mut Clock, stat: u8) {
        self.pw.freeze = stat;
        if self.pw.on {
            self.pw_apply(clk);
        }
    }

    /// DBG:POW:POKE: override row byte `i` (0..21 = R20..R35); i = 255
    /// clears all overrides.
    pub fn pw_poke(&mut self, clk: &mut Clock, i: u8, v: u8) -> bool {
        let pw = &mut *self.pw;
        if i == 255 {
            pw.poke_n = 0;
        } else if (i as usize) < hy::CFG_LEN && (pw.poke_n as usize) < pw.poke.len() {
            pw.poke[pw.poke_n as usize] = (i, v);
            pw.poke_n += 1;
        } else {
            return false;
        }
        if pw.on {
            self.pw_apply(clk);
        }
        true
    }

    /// Phase-machine cadence: (discards per switch, P period ms, full set
    /// ms) at the measured 106.7 ms per result (POW:CYC?).
    pub fn pw_cycle(&self) -> (u8, u32, u32) {
        let d = self.pw.disc as u32;
        if self.pw.alt {
            let t = (ALT_V_SETTLE_MS + SETTLE_MS) as u32 + (2 * ALT_N + ALT_V_SKIP) as u32 * 107;
            (0, 0, t)
        } else {
            (self.pw.disc, 2 * (d + 1) * 107, 4 * (d + 1) * 107)
        }
    }

    /// Values for the UI / SCPI (None before V, I and P were measured).
    pub fn pw_vals(&self) -> Option<PowerVals> {
        let pw = &*self.pw;
        let need = if pw.alt && !pw.dc { 6 } else { 7 };
        if !pw.on || pw.seen & need != need {
            return None;
        }
        let (iu, _) = iu_ir(pw.ik);
        let r = if pw.comp { pw.r_path[iu as usize] } else { 0.0 };
        // AC: the DC parts are not used (doc 3.14).
        let ac = pw.coup == COUP_AC && !pw.dc;
        let (vd, id) = if ac { (0.0, 0.0) } else { (pw.v_dc, pw.i_dc) };
        // The AC floors (doc 3.16; method B: the DC row has no AC parts).
        let b = !pw.alt && !pw.dc;
        let (fv, fi) = (FLOOR_V * V_LIM[pw.vr.min(3) as usize], FLOOR_I * I_LIM[pw.ik.min(5) as usize]);
        let (v_ok, i_ok) = (pw.v_ac >= fv, pw.i_ac >= fi);
        let mut hint = 0;
        let (mut vp, mut ip, mut pp) = ([0.0f32; 2], [0.0f32; 2], [0.0f32; 2]);
        if b && pw.coup == COUP_ACDC && !v_ok && !i_ok && (pw.v_dc.abs() >= fv || pw.i_dc.abs() >= fi) {
            hint = HINT_NO_AC;
        } else if pw.dc && pw.seen & 2 != 0 && pw.vac_dc >= fv {
            hint = HINT_AC;
        }
        let (p, mut s, mut q, mut pf, v, i) = if pw.alt && !pw.dc {
            let (v, i) = (pw.v_ac, hypot(pw.i_ac, id));
            (f32::NAN, v * i, f32::NAN, f32::NAN, v, i)
        } else if b {
            let va = if v_ok { pw.v_ac } else { 0.0 };
            let ia = if i_ok { pw.i_ac } else { 0.0 };
            let pa = if v_ok && i_ok { pw.p_ac } else { 0.0 };
            if pw.coup == COUP_ACDC {
                // The parts (doc 3.17), "At load" per part (r 0 = off).
                vp = [vd - r * id, sqrt((va * va - 2.0 * r * pa + r * r * ia * ia).max(0.0))];
                ip = [id, ia];
                pp = [vd * id - r * id * id, pa - r * ia * ia];
            }
            totals(pa, va, vd, ia, id, r)
        } else {
            totals(pw.p_ac, pw.v_ac, vd, pw.i_ac, id, r)
        };
        if b {
            let qn = hypot(NOISE_V * V_LIM[pw.vr.min(3) as usize] * i, v * NOISE_I * I_LIM[pw.ik.min(5) as usize]);
            if ac && !(v_ok && i_ok) {
                // AC: no AC part on V or I: no phase relation.
                (s, q, pf) = (f32::NAN, f32::NAN, f32::NAN);
            } else if !(v_ok && i_ok) || q < qn {
                q = 0.0;
            }
            // S at the noise level (no current, or no voltage): PF has no
            // value (AC+DC; AC has it from the floors above; #109).
            if !(s > qn) {
                pf = f32::NAN;
            }
        }
        // The counter watches the V tap, or the current (PB4) when the tap
        // has no window: "--" when that channel's AC part is below its floor.
        let f_ok = if pw.dc {
            false
        } else if b {
            pw.f_ok && if pw.r20 == R20_PB4 { i_ok } else { v_ok }
        } else {
            pw.alt || pw.f_ok
        };
        let ov = self.over();
        let (over, over_left) = match pw.over_t0 {
            Some(t0) if pw.t_lim > 0 => {
                let el = (self.t_poll.saturating_sub(t0) / 1000) as u16;
                if el >= pw.t_lim { (2, 0) } else { (1, pw.t_lim - el) }
            }
            Some(_) => (0, 0),
            None => (0, 0),
        };
        Some(PowerVals {
            p,
            s,
            q,
            pf,
            v,
            i,
            vdc: if (pw.alt && !pw.dc) || ac { f32::NAN } else { pw.v_dc },
            idc: if ac { f32::NAN } else { pw.i_dc },
            f_hz: if f_ok { self.freq.f_khz * 1000.0 } else { f32::NAN },
            burden_v: i * pw.r_path[iu as usize],
            e_wh: pw.e.wh(),
            vah: pw.va.wh(),
            secs: (pw.t_int / 1000) as u32,
            p_avg: if pw.t_int > 0 && (!pw.alt || pw.dc) { pw.e.wh() * 3.6e6 / pw.t_int as f32 } else { f32::NAN },
            vr: pw.vr,
            ik: pw.ik,
            v_auto: pw.v_auto,
            i_auto: pw.i_auto,
            alt: pw.alt && !pw.dc,
            dc: pw.dc,
            coup: pw.coup,
            comp: pw.comp,
            run: pw.run,
            v_ol: hypot(pw.v_ac, pw.v_dc) > ov * V_LIM[pw.vr as usize],
            i_ol: hypot(pw.i_ac, pw.i_dc) > ov * I_LIM[pw.ik as usize],
            over,
            over_left,
            sph: pw.sph,
            main: pw.main,
            hint,
            vp,
            ip,
            pp,
        })
    }

    /// `pw_vals`, or before the first full set the settings / ranges with
    /// zero values (the settings page, SCPI queries).
    pub fn pw_state(&self) -> PowerVals {
        self.pw_vals().unwrap_or_else(|| {
            let pw = &*self.pw;
            PowerVals {
                vr: pw.vr,
                ik: pw.ik,
                v_auto: pw.v_auto,
                i_auto: pw.i_auto,
                alt: pw.alt && !pw.dc,
                dc: pw.dc,
                coup: pw.coup,
                comp: pw.comp,
                run: pw.run,
                e_wh: pw.e.wh(),
                vah: pw.va.wh(),
                secs: (pw.t_int / 1000) as u32,
                sph: pw.sph,
                main: pw.main,
                ..PowerVals::ZERO
            }
        })
    }

    /// What the DMM page shows after a coupling change until the new
    /// coupling's first reading (device check 92fb8fa: the old layout and
    /// values stayed up to ~4 s): the new coupling's layout and units,
    /// every measured value NaN ("--"); the energy, the time and the
    /// settings as they are.
    /// Also the combined view's strip before the first reading
    /// (`App::pw_strip_wait`, #108): region B.
    #[inline(never)]
    pub fn pw_pending(&self) -> (Reading, PowerVals) {
        let n = f32::NAN;
        let pv = PowerVals {
            p: n,
            s: n,
            q: n,
            pf: n,
            v: n,
            i: n,
            vdc: n,
            idc: n,
            f_hz: n,
            burden_v: n,
            v_ol: false,
            i_ol: false,
            over: 0,
            over_left: 0,
            hint: 0,
            vp: [n; 2],
            ip: [n; 2],
            pp: [n; 2],
            ..self.pw_state()
        };
        (self.pw_reading(&pv), pv)
    }

    /// The reading of the power meter (`reading_vals`).
    fn pw_reading(&self, pv: &PowerVals) -> Reading {
        let (main, v, i, si) = reading_vals(pv);
        let mut rd = self.reading(main, si);
        rd.aux1 = Some(v);
        rd.aux2 = Some(i);
        rd.power = Some(*pv);
        rd
    }

    /// Poll in the power meter (from `poll`, after the settle deadline).
    pub(super) fn poll_power(&mut self, clk: &mut Clock, now: u64) -> Option<Reading> {
        if self.pw.job != 0 {
            return self.pw_job_poll(clk);
        }
        if self.pw.dc {
            return self.pw_poll_dc(clk, now);
        }
        // Frequency: the comparator on SENSE with a window around the V
        // tap's DC, or on PB4 (the current) when the V tap has none
        // (`pw_cmp_select`; method B); the fallback's AC V row has it on PB4
        // (the coupled V node). The sentinel is off in the power meter
        // (`pw_write_config`): R20 / R21 are the counter's alone.
        if !self.pw.alt {
            if self.pw.f_ok {
                self.freq_secondary(now, self.pw.r20);
            }
        } else if self.pw.phase & 1 == 0 {
            self.freq_secondary(now, 0xF0);
        }
        let need = hy::INTF_RMS | hy::INTF_LPF;
        if hy::read1(hy::R_INTF) & need != need {
            return None;
        }
        let mut b = [0u8; 14];
        hy::read(hy::R_AD1, &mut b);
        let lpf = hy::lpf19(&b[6..9]);
        let rms = hy::rms_signed(&b[9..14]);
        let alt = self.pw.alt;
        let stat = if alt { 1 + (self.pw.phase & 1) } else { self.pw.stat };
        let disc = self.pw.skip > 0;
        {
            let pw = &mut *self.pw;
            let dt = now.saturating_sub(pw.t_res).min(u16::MAX as u64) as u16;
            pw.t_res = now;
            pw.ring[pw.ring_k as usize % RING_N] = Raw { stat, disc, rms, lpf, dt };
            pw.ring_k = pw.ring_k.wrapping_add(1);
            if disc {
                pw.skip -= 1;
                return None;
            }
        }
        let accepted = if alt { self.pw_take_alt(rms, lpf) } else { self.pw_take(stat, rms, lpf) };
        if !accepted {
            return None;
        }
        // Next phase (method B: a new statistic, R34 only).
        let mut ranged = false;
        if alt {
            self.pw.taken += 1;
            if self.pw.taken >= ALT_N {
                ranged = self.pw_autorange(clk, self.pw.phase & 1 == 0);
                if !ranged {
                    self.pw.taken = 0;
                    self.pw.phase ^= 1;
                    let v = self.pw.phase & 1 == 0;
                    let settle = if v { ALT_V_SETTLE_MS } else { SETTLE_MS };
                    self.pw.skip = if v { ALT_V_SKIP } else { 0 };
                    self.write_config(clk);
                    self.deadline = clk.now_ms() + settle;
                }
            }
        } else {
            let st = self.pw.stat;
            if st != R34_XY {
                ranged = self.pw_autorange(clk, st == R34_YY);
                if !ranged {
                    self.pw_cmp_select(now);
                }
            }
            if !ranged && self.pw.freeze == 0 {
                let pw = &mut *self.pw;
                pw.phase = (pw.phase + 1) % CYCLE.len() as u8;
                let next = CYCLE[pw.phase as usize];
                if next != pw.stat {
                    pw.stat = next;
                    pw.skip = pw.disc;
                    hy::write(0x34, &[next]);
                }
            }
        }
        if ranged {
            return None;
        }
        let pv = self.pw_vals()?;
        self.pw_account(now, &pv);
        let pv = self.pw_vals()?;
        Some(self.pw_reading(&pv))
    }

    /// The DC row (`Pw::dc`, `dc_row`): the current from each AD1 result
    /// (the `poll_dca` scaling, factory DC A calibration, minus the row's
    /// AD1 zero of POW:ZERO), the voltage from each LPF / RMS result of Y
    /// (`pw_take`'s V scaling: factory-derived gain, Y zero). Both run at
    /// once, nothing is rewritten while measuring; a reading per current
    /// result once a voltage exists. P = V_dc I_dc (`totals`, the AC parts
    /// 0: burden compensation as in B).
    fn pw_poll_dc(&mut self, clk: &mut Clock, now: u64) -> Option<Reading> {
        let intf = hy::read1(hy::R_INTF);
        if intf & (hy::INTF_RMS | hy::INTF_LPF) == hy::INTF_RMS | hy::INTF_LPF {
            let mut b = [0u8; 14];
            hy::read(hy::R_AD1, &mut b);
            let lpf = hy::lpf19(&b[6..9]);
            let rms = hy::rms_signed(&b[9..14]);
            let pw = &mut *self.pw;
            let dt = now.saturating_sub(pw.t_res).min(u16::MAX as u64) as u16;
            pw.t_res = now;
            pw.ring[pw.ring_k as usize % RING_N] = Raw { stat: 3, disc: pw.skip_y > 0, rms, lpf, dt };
            pw.ring_k = pw.ring_k.wrapping_add(1);
            if pw.skip_y > 0 {
                pw.skip_y -= 1;
            } else {
                let r = pw.vr as usize;
                let s_v = CODE * KV[r] * pw.v_gain(pw.vr);
                let zy = if pw.zero_ok { pw.zy as i32 } else { 0 };
                let d = (lpf - zy) as f32 * s_v;
                // The row's RMS is sum(Y^2) after the HPF: V's AC part, for
                // the "AC present" hint (doc 3.16; P = V_dc I_dc misses
                // mean(v_ac i_ac), which needs v_ac).
                let a = sqrt(rms.max(0) as f32) * s_v;
                if pw.seen & 2 == 0 {
                    pw.v_dc = d;
                    pw.vac_dc = a;
                } else {
                    ema(ACV_WIN[r], 0.5, d, &mut pw.v_dc);
                    ema(ACV_WIN[r], 0.25, a, &mut pw.vac_dc);
                }
                pw.v_ac = 0.0;
                pw.seen |= 2;
                if self.pw_autorange(clk, true) {
                    return None;
                }
            }
        }
        if intf & hy::INTF_AD1 == 0 {
            return None;
        }
        let mut b3 = [0u8; 3];
        hy::read(hy::R_AD1, &mut b3);
        let ad1 = hy::s24(&b3);
        let pw = &mut *self.pw;
        if pw.skip > 0 {
            pw.skip -= 1;
            return None;
        }
        {
            pw.ring[pw.ring_k as usize % RING_N] = Raw { stat: 4, disc: false, rms: 0, lpf: ad1, dt: 0 };
            pw.ring_k = pw.ring_k.wrapping_add(1);
        }
        let (iu, ir) = iu_ir(pw.ik);
        let i = (iu * 2 + ir) as u32;
        let us = [1.0, 1e-3, 1e-6][iu as usize];
        let zd = if pw.zero_ok { pw.zd[pw.ik.min(5) as usize] as i32 } else { 0 };
        let code = (ad1 - zd) as f32;
        // The factory offset (0xAC) only without our own zero of this row.
        let off = if pw.zero_ok { 0.0 } else { cal(0xAC + 4 * i, 0.0) };
        let a = (code * 1.25 / (SHUNT[iu as usize] * MV_DIV[ir as usize] * 0.9) / AD1_DIV * cal(0x94 + 4 * i, 1.0) - off) * us;
        if pw.seen & 4 == 0 {
            pw.i_dc = a;
        } else {
            ema(ACA_WIN[i as usize] * us, 0.5, a, &mut pw.i_dc);
        }
        pw.i_ac = 0.0;
        pw.p_ac = 0.0;
        pw.seen |= 4;
        if pw.seen & 6 == 6 {
            pw.seen |= 1;
        }
        if self.pw_autorange(clk, false) {
            return None;
        }
        let pv = self.pw_vals()?;
        self.pw_account(now, &pv);
        let pv = self.pw_vals()?;
        Some(self.pw_reading(&pv))
    }

    /// Method B: one accepted result of statistic `st`. False = garbage.
    fn pw_take(&mut self, st: u8, rms: i64, lpf: i32) -> bool {
        let pw = &mut *self.pw;
        let (iu, ir) = iu_ir(pw.ik);
        let ci = (iu * 2 + ir) as u32;
        let s_i = CODE / (SHUNT_OHM[iu as usize] * MV_DIV[ir as usize]);
        let s_v = CODE * KV[pw.vr as usize] * pw.v_gain(pw.vr);
        // The DC part runs through the same shunt / OP1 / AD2 as the AC part:
        // with the LPF gain at its default 1.0 (never calibrated: so on the
        // dumped unit, while the AC A RMS gains are 0.954..1.036) the RMS
        // gain applies to it too (INFERRED; device check T4: Idc vs DC A).
        let g_i = cal(0xC4 + 4 * ci, 1.0);
        let g_il = match cal(0xF4 + 4 * ci, 1.0) {
            g if g == 1.0 => g_i,
            g => g,
        };
        // LPF zeros (POW:ZERO): the X one of this current range; the Y one
        // in the V phase below. The RMS statistics are after the HPF.
        let (zx, zy) = if pw.zero_ok { (pw.zx[pw.ik.min(5) as usize] as i32, pw.zy as i32) } else { (0, 0) };
        let i_dc = (lpf - zx) as f32 * s_i * g_il;
        let first = |seen: u8, bit: u8| seen & bit == 0;
        let ia = (ACA_WIN[ci as usize] * [1.0, 1e-3, 1e-6][iu as usize], 0.15);
        match st {
            R34_XY => {
                if rms.unsigned_abs() > 1u64 << 36 {
                    return false;
                }
                let p = rms as f32 * s_i * g_i * s_v;
                pw.p_last = p;
                if first(pw.seen, 1) {
                    pw.p_ac = p;
                } else {
                    ema_rel(0.25, 0.01 * V_LIM[pw.vr as usize] * I_LIM[pw.ik as usize], p, &mut pw.p_ac);
                }
                if first(pw.seen, 4) {
                    pw.i_dc = i_dc;
                } else {
                    ema(ia.0, 0.35, i_dc, &mut pw.i_dc);
                }
                pw.seen |= 1;
                if pw.cal != 0 && pw.seen & 6 == 6 {
                    let s = pw.v_ac * pw.i_ac;
                    if s > 0.0 {
                        pw.cal_acc += p / s;
                        pw.cal_n += 1;
                    }
                }
            }
            R34_XX | R34_YY => {
                let sq = sqrt(rms.max(0) as f32);
                if sq > 262_144.0 {
                    return false;
                }
                if st == R34_XX {
                    let a = sq * s_i * g_i;
                    if first(pw.seen, 4) {
                        pw.i_ac = a;
                        pw.i_dc = i_dc;
                    } else {
                        ema(ia.0, ia.1, a, &mut pw.i_ac);
                        ema(ia.0, 0.35, i_dc, &mut pw.i_dc);
                    }
                    pw.seen |= 4;
                } else {
                    let (a, d) = (sq * s_v, (lpf - zy) as f32 * s_v);
                    let thr = ACV_WIN[pw.vr as usize];
                    if first(pw.seen, 2) {
                        pw.v_ac = a;
                        pw.v_dc = d;
                    } else {
                        ema(thr, 0.25, a, &mut pw.v_ac);
                        ema(thr, 0.25, d, &mut pw.v_dc);
                    }
                    pw.seen |= 2;
                }
            }
            _ => return false,
        }
        true
    }

    /// Fallback: one result of the stock AC V row (V phase) or AC A row,
    /// with the stock scaling and calibration (`poll_ac`).
    fn pw_take_alt(&mut self, rms: i64, lpf: i32) -> bool {
        let pw = &mut *self.pw;
        let sq = sqrt(rms.max(0) as f32);
        if sq > 262_144.0 {
            return false;
        }
        if pw.phase & 1 == 0 {
            let r = pw.vr as u32;
            let s = K_ACV[r as usize] * CODE;
            let a = (sq * s * cal(0x34 + 4 * r, 1.0) - cal(0x44 + 4 * r, 0.0)).max(0.0);
            if pw.seen & 2 == 0 {
                pw.v_ac = a;
            } else {
                ema(ACV_WIN[r as usize], 0.25, a, &mut pw.v_ac);
            }
            pw.v_dc = 0.0;
            pw.seen |= 2;
        } else {
            let (iu, ir) = iu_ir(pw.ik);
            let ci = (iu * 2 + ir) as u32;
            let us = [1.0, 1e-3, 1e-6][iu as usize];
            let s = CODE / (SHUNT_OHM[iu as usize] * MV_DIV[ir as usize]);
            let a = (sq * s * cal(0xC4 + 4 * ci, 1.0) - cal(0xDC + 4 * ci, 0.0) * us).max(0.0);
            let d = lpf as f32 * s * cal(0xF4 + 4 * ci, 1.0) - cal(0x10C + 4 * ci, 0.0) * us;
            let thr = ACA_WIN[ci as usize] * us;
            if pw.seen & 4 == 0 {
                pw.i_ac = a;
                pw.i_dc = d;
            } else {
                ema(thr, 0.15, a, &mut pw.i_ac);
                ema(thr, 0.35, d, &mut pw.i_dc);
            }
            pw.seen |= 4;
        }
        true
    }

    /// Autorange the V (`v`) or the I channel after its result (stock
    /// thresholds: up above 1.03 x the limit at 60000 counts, down below
    /// 0.1 x limit / 1.03). True = a range changed (row rewritten).
    fn pw_autorange(&mut self, clk: &mut Clock, v: bool) -> bool {
        let ov = self.over();
        if !v {
            let (iu, ir) = iu_ir(self.pw.ik);
            let over = hypot(self.pw.i_ac, self.pw.i_dc) > ov * I_LIM[self.pw.ik.min(5) as usize];
            if !self.pw.i_auto {
                // OVERLOAD (doc 4.3): a MANUAL range on the uA / mA shunt
                // over range -> 10 A, latched, red alert.
                if sentinel::overload_trips(iu, ir, true, over) {
                    return self.pw_ovl_trip(clk);
                }
            } else if over && iu >= 1 {
                // I AUTO (doc 3.14): over range on the uA / mA shunt is the
                // normal up-range, in one hop to the A shunt (6 A; the shunts
                // without a fuse of their own leave at this first result),
                // no latch / alert; the walk down follows as usual.
                self.pw.ik = 4;
                self.pw.keep = 2;
                self.pw_apply(clk);
                return true;
            }
        }
        // The OVERLOAD latch's shunt floor (ik 0, 1 uA; 2, 3 mA).
        let floor = self.ovl.map_or(0, |o| 2 * (3 - o.barred.min(3) as usize));
        let pw = &mut *self.pw;
        let changed = if v {
            let m = hypot(pw.v_ac, pw.v_dc);
            let r = pw.vr as usize;
            if !pw.v_auto {
                false
            } else if m > ov * V_LIM[r] && r < 3 {
                pw.vr += 1;
                true
            } else if m < 0.1 * V_LIM[r] / ov && r > 0 {
                pw.vr -= 1;
                true
            } else {
                false
            }
        } else {
            let m = hypot(pw.i_ac, pw.i_dc);
            let k = pw.ik as usize;
            // AUTO+ (stock m+7 = 1) never steps into the uA shunt.
            let kmin = (if self.a_auto == 1 { 2 } else { 0 }).max(floor);
            if !pw.i_auto {
                false
            } else if m > ov * I_LIM[k] && k < 5 {
                pw.ik += 1;
                true
            } else if m < 0.1 * I_LIM[k] / ov && k > kmin {
                pw.ik -= 1;
                true
            } else {
                false
            }
        };
        if changed {
            self.pw.keep = if v { 4 } else { 2 };
            self.pw_apply(clk);
        }
        changed
    }

    /// The OVERLOAD rule in the power meter (`Dmm::ovl_trip`): the I
    /// channel to the 10 A range (ik 5), the V parts kept.
    fn pw_ovl_trip(&mut self, clk: &mut Clock) -> bool {
        let (iu, _) = iu_ir(self.pw.ik);
        let back = match self.ovl {
            Some(o) => o.back,
            None => (!self.pw.i_auto).then_some((self.pw.ik, 0)),
        };
        let barred = self.ovl.map_or(iu, |o| o.barred.min(iu));
        let bu = back.map(|(k, _)| iu_ir(k));
        let sticky = self.ovl_sticky(clk.now_ms());
        self.ovl = Some(super::Ovl { barred, back, thr: sentinel::overload_release_thr(barred, bu), low_since: None, sticky });
        self.pw.ik = 5;
        self.pw.keep = 2;
        self.pw_apply(clk);
        self.sent.set_overload(clk.now_ms(), true);
        true
    }

    /// Per accepted reading: energy (the previous P over the time since the
    /// previous reading, so a gap holds the last P), the current limit and
    /// the phase calibration.
    fn pw_account(&mut self, now: u64, pv: &PowerVals) {
        self.sent.pw_hints(now, pv.hint == HINT_NO_AC, pv.hint == HINT_AC);
        let pw = &mut *self.pw;
        if pw.run {
            if let Some(t) = pw.t_last {
                let dt = now.saturating_sub(t);
                let s = dt as f32 * 1e-3;
                if !pw.alt || pw.dc {
                    pw.e.add(pw.p_tot * s);
                }
                pw.va.add(pw.s_last * s);
                pw.t_int += dt;
            }
            pw.t_last = Some(now);
        } else {
            pw.t_last = None;
        }
        // The values for the next interval: this reading's.
        pw.s_last = if pv.s.is_finite() { pv.s } else { 0.0 };
        pw.p_tot = if pv.p.is_finite() { pv.p } else { 0.0 };
        if pv.i > pw.i_cont {
            if pw.over_t0.is_none() {
                pw.over_t0 = Some(now);
                pw.over_beeped = false;
            }
        } else if pv.i < 0.95 * pw.i_cont {
            pw.over_t0 = None;
        }
        if pw.cal != 0 && pw.cal_n >= 8 {
            let m = pw.cal_acc / pw.cal_n as f32;
            pw.cal_n = 0;
            pw.cal_acc = 0.0;
            if pw.cal == 1 && m.abs() > CAL_PF_MAX {
                // Not a reactive load: the null is outside the SPHACAL
                // span (+-18.6 deg even at 1 kHz); SPHACAL unchanged.
                pw.cal = 0;
                pw.cal_res = CAL_NOT_REACTIVE;
            } else if pw.cal == 1 {
                pw.cal_pa = m;
                pw.cal = 2;
                let d: i16 = if pw.cal_s0 > 79 { -48 } else { 48 };
                pw.sph = (pw.cal_s0 as i16 + d) as i8;
                pw.cal_req = true;
            } else {
                let d = pw.sph as f32 - pw.cal_s0 as f32;
                let slope = (m - pw.cal_pa) / d;
                pw.cal = 0;
                let s = if slope.abs() > 1e-7 { pw.cal_s0 as f32 - pw.cal_pa / slope } else { f32::NAN };
                (pw.sph, pw.cal_res) = match cal_outcome(s) {
                    Ok(v) => (v, CAL_OK),
                    Err(e) => (pw.cal_s0, e),
                };
                pw.cal_req = true;
            }
        }
    }

    /// After a V or I result: the counter's source and window. The V tap
    /// (SENSE, DC-coupled) with the R21 window around its DC (`cmp_window`);
    /// else the current on PB4 (AC-coupled, the default window) when its AC
    /// part at OP1O is >= 25 mV rms (INFERRED: the stock AC A rows count
    /// there); else no frequency ("--").
    fn pw_cmp_select(&mut self, now: u64) {
        let pw = &mut *self.pw;
        if pw.seen & 6 != 6 {
            return;
        }
        let k = KV[pw.vr as usize] * pw.v_gain(pw.vr);
        let (iu, ir) = iu_ir(pw.ik);
        let i_op = pw.i_ac * SHUNT_OHM[iu as usize] * MV_DIV[ir as usize];
        let (r20, r21, ok) = match cmp_window(pw.v_dc / k, pw.v_ac / k) {
            Some(w) => (R20_SENSE, w, true),
            None if i_op >= 0.025 => (R20_PB4, R21_DEFAULT, true),
            None => (pw.r20, pw.r21, false),
        };
        pw.f_ok = ok;
        if (r20, r21) != (pw.r20, pw.r21) {
            pw.r20 = r20;
            pw.r21 = r21;
            if r21 != self.pw.row[1] {
                hy::write(0x21, &[r21]);
                self.pw.row[1] = r21;
            }
            self.pw.row[0] = r20 | 0x02;
            // Restart the counter on the new source now; the gate that ran
            // was the old source's: no value until the new one's ("--").
            self.freq.fresh = true;
            self.freq.f_khz = f32::NAN;
            self.freq_secondary(now, r20);
        }
    }

    /// POW:ZERO / the Power Settings "Zero" button: with the leads open
    /// (nothing on V-COM, nothing through A) the LPF zeros of the six
    /// current ranges (XX phase) and of the V channel (YY phase, the current
    /// V range), ~12 s. Refused per step when a signal is present
    /// (`JR_SIGNAL`: the old zeros stay).
    pub fn pw_zero_start(&mut self, clk: &mut Clock) -> bool {
        self.pw_job_start(clk, JOB_ZERO)
    }

    /// POW:VOLT:CAL / the "V cal" button: with a steady AC or DC voltage on
    /// V-COM (10..70 % (AC) / 100 % (DC) of the V range), the power V
    /// channel and the stock's factory-calibrated DC V (AD1) or AC V row of
    /// the same range, alternately, two rounds; the ratio becomes the
    /// range's gain. The kind follows the larger part of the first round.
    pub fn pw_vcal_start(&mut self, clk: &mut Clock) -> bool {
        self.pw_job_start(clk, JOB_VCAL)
    }

    fn pw_job_start(&mut self, clk: &mut Clock, job: u8) -> bool {
        let pw = &mut *self.pw;
        if !pw.on || pw.job != 0 {
            return false;
        }
        if pw.alt {
            pw.job_last = job;
            pw.job_res = JR_METHOD;
            return false;
        }
        pw.job_save = (pw.vr, pw.ik, pw.v_auto, pw.i_auto);
        pw.job = job;
        pw.job_last = job;
        pw.job_step = 0;
        pw.job_res = JR_RUN;
        pw.job_vr = pw.vr;
        pw.job_zx = pw.zx;
        pw.job_zd = pw.zd;
        pw.cal = 0;
        self.pw_job_step(clk);
        true
    }

    /// Write the row of the current job step and settle.
    fn pw_job_step(&mut self, clk: &mut Clock) {
        let pw = &mut *self.pw;
        pw.job_n = 0;
        pw.job_a = 0.0;
        pw.job_b = 0.0;
        if pw.job == JOB_ZERO && pw.job_step < 12 {
            pw.ik = pw.job_step % 6;
        }
        let vref = pw.job == JOB_VCAL && pw.job_step & 1 == 1;
        let (settle, skip) = match (vref, pw.job_dc) {
            (true, false) => (VCAL_AC_SETTLE_MS, ALT_V_SKIP),
            (true, true) => (450, JOB_SKIP),
            _ => (SETTLE_MS, JOB_SKIP),
        };
        pw.skip = skip;
        self.write_config(clk);
        self.deadline = clk.now_ms() + settle;
    }

    /// End the job (`res`): restore the ranges, back to the phase machine.
    fn pw_job_end(&mut self, res: u8) {
        let pw = &mut *self.pw;
        if pw.job == 0 {
            return;
        }
        pw.job = 0;
        pw.job_res = res;
        (pw.vr, pw.ik, pw.v_auto, pw.i_auto) = pw.job_save;
        // `pw_apply` follows from the caller (or the function changes).
        pw.cal_req = pw.on;
    }

    /// A job's result: the zero / gain steps (no reading meanwhile).
    fn pw_job_poll(&mut self, clk: &mut Clock) -> Option<Reading> {
        let pw = &*self.pw;
        let vref = pw.job == JOB_VCAL && pw.job_step & 1 == 1;
        let r = pw.vr.min(3) as usize;
        // (main value, second value, sqrt(RMS) for the zero's signal check)
        let dzero = pw.job == JOB_ZERO && (6..12).contains(&pw.job_step);
        let (x, y, sq) = if dzero {
            // The DC row's AD1 zero of this current range (raw code).
            (self.ad1()? as f32, 0.0, 0.0)
        } else if vref && pw.job_dc {
            let ad1 = self.ad1()?;
            let mut v = ad1 as f32 * K_ACV[r] * (1.25 / 0.9) / AD1_DIV * cal(0x04 + 4 * r as u32, 1.0)
                - cal(0x14 + 4 * r as u32, 0.0);
            if v < 0.0 {
                v *= DCV_NEGK[r];
            }
            (v, 0.0, 0.0)
        } else {
            let need = hy::INTF_RMS | hy::INTF_LPF;
            if hy::read1(hy::R_INTF) & need != need {
                return None;
            }
            let mut b = [0u8; 14];
            hy::read(hy::R_AD1, &mut b);
            let lpf = hy::lpf19(&b[6..9]);
            let rms = hy::rms_signed(&b[9..14]);
            let sq = sqrt(rms.max(0) as f32);
            if vref {
                // The stock AC V row with its calibration (`poll_ac`).
                let v = (sq * K_ACV[r] * CODE * cal(0x34 + 4 * r as u32, 1.0) - cal(0x44 + 4 * r as u32, 0.0)).max(0.0);
                (v, 0.0, sq)
            } else if pw.job == JOB_VCAL {
                // The power V channel, raw (gain 1, zero applied).
                let zy = if pw.zero_ok { pw.zy as i32 } else { 0 };
                (sq * CODE * KV[r], (lpf - zy) as f32 * CODE * KV[r], sq)
            } else {
                (lpf as f32, 0.0, sq)
            }
        };
        let pw = &mut *self.pw;
        if pw.skip > 0 {
            pw.skip -= 1;
            return None;
        }
        if pw.job == JOB_ZERO {
            // Against the previous zero of the step (a stored zero of
            // ~1000 codes is not a signal).
            let st = pw.job_step as usize;
            let old = if st < 6 { pw.job_zx[st] } else if st < 12 { pw.job_zd[st - 6] } else { pw.zy } as i32;
            let old = if pw.zero_ok { old } else { 0 };
            let max = if (6..12).contains(&st) { ZERO_MAX_AD1 } else { ZERO_MAX_LPF };
            if sq > ZERO_MAX_RMS || (x as i32 - old).abs() > max {
                self.pw_job_end(JR_SIGNAL);
                return None;
            }
        }
        pw.job_a += x;
        pw.job_b += y;
        pw.job_n += 1;
        if pw.job_n < JOB_N {
            return None;
        }
        let (a, b) = (pw.job_a / JOB_N as f32, pw.job_b / JOB_N as f32);
        let step = pw.job_step;
        let res = match pw.job {
            JOB_ZERO => {
                let z = (if a < 0.0 { a - 0.5 } else { a + 0.5 }) as i32;
                let z = z.clamp(-32768, 32767) as i16;
                if step < 6 {
                    pw.job_zx[step as usize] = z;
                    None
                } else if step < 12 {
                    pw.job_zd[step as usize - 6] = z;
                    None
                } else {
                    pw.zx = pw.job_zx;
                    pw.zd = pw.job_zd;
                    pw.zy = z;
                    pw.zero_ok = true;
                    Some(JR_OK)
                }
            }
            _ if step & 1 == 0 => {
                // Power V channel: AC (a) or DC (b) part, the kind decided by
                // the first round.
                if step == 0 {
                    pw.job_dc = b.abs() > a;
                }
                let m = if pw.job_dc { b } else { a };
                let lim = V_LIM[r];
                let max = if pw.job_dc { VCAL_MAX_DC } else { VCAL_MAX_AC };
                pw.job_raw[(step / 2) as usize] = m;
                let peak = b.abs() + core::f32::consts::SQRT_2 * a.abs();
                if m.abs() < VCAL_MIN * lim {
                    Some(JR_SMALL)
                } else if m.abs() > max * lim || peak > VCAL_PEAK_MAX * lim {
                    Some(JR_BIG)
                } else {
                    None
                }
            }
            _ => {
                pw.job_ref[(step / 2) as usize] = a;
                if step < 3 {
                    None
                } else {
                    let r0 = pw.job_ref[0] / pw.job_raw[0];
                    let r1 = pw.job_ref[1] / pw.job_raw[1];
                    let g = 0.5 * (r0 + r1);
                    let steady = (pw.job_raw[0] - pw.job_raw[1]).abs() <= VCAL_SPREAD * pw.job_raw[0].abs();
                    if !g.is_finite() || (r0 - r1).abs() > VCAL_SPREAD * g.abs() || !steady {
                        Some(JR_UNSTABLE)
                    } else if !(0.9..=1.1).contains(&g) {
                        Some(JR_RANGE)
                    } else {
                        pw.vgain[r] = g;
                        Some(JR_OK)
                    }
                }
            }
        };
        match res {
            Some(res) => self.pw_job_end(res),
            None => {
                self.pw.job_step += 1;
                self.pw_job_step(clk);
            }
        }
        None
    }

    /// What the DMM page shows while no full reading exists yet: the V of
    /// the latest V result (the I channel still autoranging from 10 A) and
    /// the state text. None when a reading exists or nothing is measured.
    pub fn pw_partial(&self) -> Option<(Option<Val>, &'static str)> {
        let pw = &*self.pw;
        if !pw.on {
            return None;
        }
        let txt = match pw.job {
            JOB_ZERO => "Zeroing: leads open",
            JOB_VCAL => "V calibration...",
            _ if self.pw_vals().is_some() => return None,
            _ => "I ranging...",
        };
        let v = (pw.seen & 2 != 0 && pw.job == 0).then(|| Val::new(hypot(pw.v_ac, pw.v_dc), 4 - pw.vr.min(3), "V"));
        Some((v, txt))
    }

    /// Pending SPHACAL rewrite of the calibration (from `poll`).
    pub(super) fn pw_service(&mut self, clk: &mut Clock) {
        if self.pw.cal_req {
            self.pw.cal_req = false;
            self.pw_apply(clk);
        }
    }

    /// The continuous-current limit just expired (once per expiry): the app
    /// toasts and beeps.
    pub fn pw_over_expired(&mut self) -> bool {
        let pw = &mut *self.pw;
        match pw.over_t0 {
            Some(t0) if pw.t_lim > 0 && !pw.over_beeped && self.t_poll.saturating_sub(t0) >= pw.t_lim as u64 * 1000 => {
                pw.over_beeped = true;
                true
            }
            _ => false,
        }
    }
}

// ------------------------------------------------------ phase calibration

/// `Pw::cal_res`: none since boot, running, stored, the two P / S points
/// gave no slope (no current, or a resistive load: d(P/S)/dstep ~ 0 near
/// PF 1), |P / S| above CAL_PF_MAX at the start (not a reactive load), the
/// null outside +-127 steps.
pub const CAL_NONE: u8 = 0;
pub const CAL_RUN: u8 = 1;
pub const CAL_OK: u8 = 2;
pub const CAL_FLAT: u8 = 3;
pub const CAL_NOT_REACTIVE: u8 = 4;
pub const CAL_RANGE: u8 = 5;
pub const CAL_NAMES: [&str; 6] = ["NONE", "RUN", "OK", "FLAT", "NOTREACTIVE", "RANGE"];
/// sin(18.6 deg) = the SPHACAL span at 1 kHz (127 x 0.407 us); at 50 Hz it
/// is +-0.93 deg, so a load must be within that of PF 0 for a valid null.
pub const CAL_PF_MAX: f32 = 0.32;

/// The Newton null `s` (steps, NaN = no slope) -> SPHACAL or the refusal.
pub fn cal_outcome(s: f32) -> Result<i8, u8> {
    if !s.is_finite() {
        Err(CAL_FLAT)
    } else if !(-127.49..=127.49).contains(&s) {
        Err(CAL_RANGE)
    } else {
        Ok((if s < 0.0 { s - 0.5 } else { s + 0.5 }) as i8)
    }
}

/// Why `POW:PHAS:CAL RESistive` is refused (doc 3.13): on a resistive load
/// P / S = cos(phi), and the error to find is phi ~ 0, where d cos / d phi
/// = -sin(phi) ~ 0: one step (0.407 us = 0.0073 deg at 50 Hz) moves P / S by
/// ~1e-8 at the optimum and the whole +-127 span by ~3e-5 around it, below
/// the 107 ms blocks' ripple (non-integer cycles, ~1e-2 per result) and the
/// V / I gain errors (~1e-3), so neither the maximum nor a symmetric
/// two-point / least-squares fit over SPHACAL steps resolves 0.05 deg. The
/// AD2 / AD3 decimators share OSR, so their group delays are equal and
/// cancel: SPHACAL corrects the analog paths (OP1 on the shunt vs OP2 on
/// the divider's ACC network), which only a measurement shows. A reactive
/// load (PF ~ 0) puts the null where the slope is largest (dP/S = dphi).
pub const CAL_RES_REFUSAL: &str = "Settings conflict; RES: PF 1 optimum is flat, use REACtive (C or L load, PF ~0)";

// ------------------------------------------------- Bluetooth (stock app)
//
// The stock phone protocol (docs/bt-protocol.md, bt_meas_v2_6.h) has no
// power function and no unit field: the unit follows from the mode byte's
// group (V group: base mV, A group: base uA, ...) and the format byte's
// x1000 prefix. Least bad mapping (doc 3.13): the power meter reports the
// DC V mode byte (group 0, sub DC; range field = the V channel's: 5 auto,
// 1..4 manual 6 / 60 / 600 / 1000 V) and P (S in the VA-only fallback) as
// the main value with the digits, decimals, sign and prefix of W: the app
// shows "230.02 V" for 230.02 W, "3.7800 mV" for 3.78 mW. The alternative,
// the AC A byte the power meter ran under before, made the app show P in
// uA (prefix 0 = the A group's uA base) and V as uA; reporting Irms (a
// right unit, but no power) loses the one number a phone log of a power
// session is for. Value 2 / 3 stay "no value" as in a stock DC V frame.
// The phone sending this byte back (cmd 06) keeps the power meter
// (`Dmm::apply_mode_byte`); any other byte leaves it as a function button.

/// The BT mode byte of the power meter (DC V, the V channel's range).
pub fn bt_mode_byte(v_auto: bool, vr: u8) -> u8 {
    (if v_auto { 5 } else { vr.min(3) + 1 }) << 3
}

/// The BT main value of a power reading: P (S) as a V-group value (W ->
/// "V", mW -> "mV", kW -> "V" with the decimals moved), at most 65534
/// counts (0xFFFF = no value) with as many of its decimals as fit.
pub fn bt_main(pv: &PowerVals) -> Val {
    let si = if pv.alt { pv.s } else { pv.p };
    let (x, dec, u) = p_format(V_LIM[pv.vr.min(3) as usize] * I_LIM[pv.ik.min(5) as usize], si, true);
    let (mut x, mut dec, unit) = match u {
        "mW" => (x, dec, "mV"),
        "kW" => (x * 1e3, dec.saturating_sub(3), "V"),
        _ => (x, dec, "V"),
    };
    if pv.v_ol || pv.i_ol || !x.is_finite() {
        return Val::ol(dec, unit);
    }
    let mut k = 1.0f32;
    for _ in 0..dec {
        k *= 10.0;
    }
    while dec > 0 && x.abs() * k + 0.5 >= 65535.0 {
        dec -= 1;
        k *= 0.1;
    }
    if x.abs() * k + 0.5 >= 65535.0 {
        x = x.signum() * 65534.0;
    }
    Val::new(no_neg_zero(x, dec), dec, unit)
}

/// The readouts of a power reading: main = P (fallback: S) in the
/// `p_format` of the range pair, V in the AC V format of its range, I in the
/// AC A format (A / mA / uA) of its range, and the main value in SI.
/// `x` as shown with `dec` decimals: a value that rounds to zero is +0, so
/// the readouts never show a negative zero ("-0.0000 mWh" / "-0.000 mW"
/// with no current, device check c50425f).
pub fn no_neg_zero(x: f32, dec: u8) -> f32 {
    let mut s = 1.0f32;
    for _ in 0..dec {
        s *= 10.0;
    }
    if (x * s).abs() < 0.5 { 0.0 } else { x }
}

pub fn reading_vals(pv: &PowerVals) -> (Val, Val, Val, f32) {
    let (iu, ir) = iu_ir(pv.ik);
    let vd = 4 - pv.vr.min(3);
    let v = if pv.v_ol { Val::ol(vd, "V") } else { Val::new(no_neg_zero(pv.v, vd), vd, "V") };
    let (us, unit, idec) = match iu {
        0 => (1.0, "A", 4 - ir),
        1 => (1e3, "mA", 3 - ir),
        _ => (1e6, "\u{b5}A", 2 - ir),
    };
    let i = if pv.i_ol { Val::ol(idec, unit) } else { Val::new(no_neg_zero(pv.i * us, idec), idec, unit) };
    let (main_si, w) = if pv.alt { (pv.s, false) } else { (pv.p, true) };
    let ol = pv.v_ol || pv.i_ol;
    let main = match pv.main {
        // Energy (the fallback has no Wh: VAh): never OL, the integral holds.
        MAIN_WH | MAIN_VAH => {
            let va = pv.main == MAIN_VAH || pv.alt;
            let (x, dec, u) = energy_format(if va { pv.vah } else { pv.e_wh }, va);
            Val::new(no_neg_zero(x, dec), dec, u)
        }
        _ => {
            let (x, dec, u) = p_format(V_LIM[pv.vr.min(3) as usize] * I_LIM[pv.ik.min(5) as usize], main_si, w);
            if ol { Val::ol(dec, u) } else { Val::new(no_neg_zero(x, dec), dec, u) }
        }
    };
    (main, v, i, if ol { 0.0 } else { main_si })
}

/// The energy box of the power page (the first box of the lower row): the
/// energy (Wh; VAh in the fallback), or P when the main readout shows the
/// energy (owner: V and I on top, the Wh counter below).
pub fn energy_val(pv: &PowerVals) -> Val {
    if pv.main == MAIN_P {
        let (x, dec, u) = energy_format(if pv.alt { pv.vah } else { pv.e_wh }, pv.alt);
        Val::new(no_neg_zero(x, dec), dec, u)
    } else {
        reading_vals(&PowerVals { main: MAIN_P, ..*pv }).0
    }
}

/// Main-value format of P (or S) for a range pair whose full-scale product
/// is `fs` W: "W" with 4 .. 0 decimals (60000-count style: 3.6000 W, 36.000,
/// 360.00, 3600.0, 10000.0 W), "mW" below 1 W of full scale. Returns
/// (value in the unit, decimals, unit).
pub fn p_format(fs: f32, si: f32, watts: bool) -> (f32, u8, &'static str) {
    // 1000 V x 10 A: kW (the F78 cells have no 6-digit integer format).
    if fs >= 9000.0 {
        return (si * 1e-3, 3, if watts { "kW" } else { "kVA" });
    }
    let (k, unit) = if fs < 1.0 { (1e3, if watts { "mW" } else { "mVA" }) } else { (1.0, if watts { "W" } else { "VA" }) };
    let f = fs * k * 1.05;
    let dec = if f < 10.0 {
        4
    } else if f < 100.0 {
        3
    } else if f < 1000.0 {
        2
    } else if f < 10_000.0 {
        1
    } else {
        1
    };
    (si * k, dec, unit)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every byte of every power row is the stock I row's, the stock V
    /// row's (R2A..R30) or one of the listed overrides (doc 3.10 step 2).
    #[test]
    fn power_row_bytes() {
        for vr in 0..4u8 {
            for k in 0..6u8 {
                for st in [R34_XY, R34_XX, R34_YY] {
                    let row = power_row(vr, k, st);
                    let (iu, ir) = iu_ir(k);
                    let irow = if iu == 0 { CFG_F4[ir as usize] } else { CFG_F5_6[ir as usize] };
                    let vrow = if vr == 0 { CFG_F1[0] } else { CFG_F0[vr as usize] };
                    for (j, (&b, &i)) in row.iter().zip(irow.iter()).enumerate() {
                        let want = match j {
                            0 => 0x12,
                            5 => i & !0x02,
                            9 => 0xF8,
                            10..=16 => vrow[j],
                            18 => 0x80 | (i & 0x0F),
                            20 => st,
                            21 => 0x54,
                            _ => i,
                        };
                        assert_eq!(b, want, "vr {vr} k {k} st {st:#x} R{:02X}", 0x20 + j);
                    }
                    // The decoded intent: OP1 on the shunt (PB2 for A, PB1
                    // for mA / uA), OP2 on SENSE, AD2 = OP1O - PB3, AD3 =
                    // OP2O - AGND (different negative inputs, datasheet
                    // note 8), OP2 x1, OP1 x10 in the low range, AD1 off.
                    assert_eq!(row[18] & 0x07, if iu == 0 { 6 } else { 5 });
                    assert_eq!(row[18] & 0xF8, 0x88);
                    assert_eq!(row[7] >> 4, 0x2);
                    assert_eq!(row[21] >> 4, 0x5);
                    assert_eq!(row[5] & 0x03, if ir == 0 { 1 } else { 0 });
                    assert_eq!(row[3] & 0x80, 0);
                }
            }
        }
        // Examples of the doc / the emulator agent's table.
        assert_eq!(power_row(1, 5, R34_XY), [0x12, 0xdd, 0x15, 0x03, 0x52, 0x40, 0x80, 0x2d, 0x31, 0xf8, 0x20, 0x00, 0x00, 0x09, 0x28, 0xa0, 0x80, 0xc7, 0x8e, 0xac, 0x88, 0x54]);
        assert_eq!(power_row(0, 2, R34_XY), [0x12, 0xdd, 0x14, 0x03, 0x35, 0x41, 0x80, 0x2d, 0x31, 0xf8, 0x06, 0x00, 0x00, 0x90, 0x28, 0xa0, 0x80, 0xc7, 0x8d, 0xac, 0x88, 0x54]);
        assert_eq!(power_row(2, 1, R34_YY)[20], 0x8C);
    }

    /// The DC row: the power row (YY) with the stock DC A row's AD1 bytes
    /// (doc 3.12): AD1 = OP1O - PB3 enabled, OP2 <- SENSE -> AD3, AD2IN PB3
    /// != AD3IN AGND (note 8).
    #[test]
    fn dc_row_bytes() {
        for vr in 0..4u8 {
            for k in 0..6u8 {
                let row = dc_row(vr, k);
                let p = power_row(vr, k, R34_YY);
                let (iu, ir) = iu_ir(k);
                let a = if iu == 0 { CFG_F7[ir as usize] } else { CFG_F8_9[ir as usize] };
                for j in 0..row.len() {
                    let want = if [2, 3, 4, 8, 19].contains(&j) { a[j] } else { p[j] };
                    assert_eq!(row[j], want, "vr {vr} k {k} R{:02X}", 0x20 + j);
                }
                assert_eq!(row[3] & 0x80, 0x80); // ENAD1
                assert_eq!(row[4] >> 4, 3); // SAD1FP = OP1O
                assert_eq!(row[4] & 7, 5); // SAD1FN = PB3
                assert_eq!(row[20], R34_YY);
                assert_eq!(row[21], 0x54);
                assert_eq!((row[7] >> 4) & 3, 2); // AD2IN = PB3
                assert_eq!(row[18] & 0xF0, 0x80); // OP2 on SENSE
                assert_eq!(row[5] & 3, if ir == 0 { 1 } else { 0 }); // OPS1 x10 / x1
            }
        }
        // For the emulator's host test (emu/hy3131/tests Program.cs).
        assert_eq!(dc_row(1, 5), [0x12, 0xdd, 0x15, 0x83, 0x35, 0x40, 0x80, 0x2d, 0x31, 0xf8, 0x20, 0x00, 0x00, 0x09, 0x28, 0xa0, 0x80, 0xc7, 0x8e, 0xac, 0x8c, 0x54]);
    }

    #[test]
    fn ranges_and_r36() {
        assert_eq!([iu_ir(0), iu_ir(1), iu_ir(2), iu_ir(3), iu_ir(4), iu_ir(5)], [(2, 0), (2, 1), (1, 0), (1, 1), (0, 0), (0, 1)]);
        assert_eq!([r36_of(0), r36_of(5), r36_of(-5), r36_of(127), r36_of(-128)], [0x00, 0x85, 0x05, 0xFF, 0x7F]);
        // Fallback rows are the stock ones.
        assert_eq!(alt_row(true, 2, 5), CFG_F0[2]);
        assert_eq!(alt_row(false, 2, 5), CFG_F4[1]);
        assert_eq!(alt_row(false, 2, 0), CFG_F5_6[0]);
    }

    fn close(a: f32, b: f32, tol: f32) -> bool {
        (a - b).abs() <= tol * b.abs().max(1e-9)
    }

    #[test]
    fn totals_and_burden() {
        // 230 V / 1 A resistive, no burden: P = S = 230, PF 1, Q 0.
        let (p, s, q, pf, v, i) = totals(230.0, 230.0, 0.0, 1.0, 0.0, 0.0);
        assert!(close(p, 230.0, 1e-6) && close(s, 230.0, 1e-6) && q < 1e-3 && close(pf, 1.0, 1e-6) && close(v, 230.0, 1e-6) && close(i, 1.0, 1e-6));
        // PF 0.5.
        let (p, s, _, pf, _, _) = totals(115.0, 230.0, 0.0, 1.0, 0.0, 0.0);
        assert!(close(p, 115.0, 1e-6) && close(s, 230.0, 1e-6) && close(pf, 0.5, 1e-6));
        // DC: P = Vdc Idc, V and I the DC parts.
        let (p, s, q, pf, v, i) = totals(0.0, 0.0, 12.0, 0.0, 2.0, 0.0);
        assert!(close(p, 24.0, 1e-6) && close(s, 24.0, 1e-6) && q < 1e-3 && close(pf, 1.0, 1e-6) && close(v, 12.0, 1e-6) && close(i, 2.0, 1e-6));
        // Reversed current: P < 0, PF -1.
        let (p, _, _, pf, _, _) = totals(0.0, 0.0, 12.0, 0.0, -2.0, 0.0);
        assert!(close(p, -24.0, 1e-6) && close(pf, -1.0, 1e-6));
        // No current (#109): S 0, Q 0, PF none (not 0 = "purely reactive").
        let (p, s, q, pf, v, _) = totals(0.0, 0.0, 5.0, 0.0, 0.0, 0.0);
        assert!(p == 0.0 && s == 0.0 && q == 0.0 && pf.is_nan() && close(v, 5.0, 1e-6));
        // Burden: 5 V source, 0.4 A, R_path 0.6 ohm (DC, wiring 1): the
        // load sees 5 - 0.24 = 4.76 V and takes 1.904 W; the meter 0.096 W.
        let (p, s, _, pf, v, i) = totals(0.0, 0.0, 5.0, 0.0, 0.4, 0.6);
        assert!(close(p, 1.904, 1e-5) && close(v, 4.76, 1e-5) && close(s, 1.904, 1e-5) && close(pf, 1.0, 1e-5) && close(i, 0.4, 1e-6));
        // Reversed (a source on the load side), burden still subtracted.
        let (p, _, _, _, v, _) = totals(0.0, 0.0, 5.0, 0.0, -0.4, 0.6);
        assert!(close(p, -2.096, 1e-5) && close(v, 5.24, 1e-5));
        // AC: v_L = v - R i holds for any phase: 230 V, 1 A at PF 0.5, R 1:
        // P_L = 115 - 1, V_L^2 = 230^2 - 2 * 115 + 1.
        let (p, s, _, _, v, _) = totals(115.0, 230.0, 0.0, 1.0, 0.0, 1.0);
        assert!(close(p, 114.0, 1e-5) && close(v, (230.0f32 * 230.0 - 230.0 + 1.0).sqrt(), 1e-5) && close(s, v, 1e-6));
    }

    #[test]
    fn energy_accumulator_keeps_small_steps() {
        // 1 mW for 10 h in 100 ms steps: 0.01 Wh (an f32 sum of joules in
        // the mJ range keeps them; check a large base too).
        let mut a = Acc::ZERO;
        a.add(3.6e6); // 1 kWh already
        for _ in 0..360_000 {
            a.add(1e-3 * 0.1);
        }
        assert!((a.wh() - 1000.01).abs() < 1e-4, "{}", a.wh());
        let mut b = Acc::ZERO;
        for _ in 0..1000 {
            b.add(-0.25);
        }
        assert!((b.wh() + 250.0 / 3600.0).abs() < 1e-6);
    }

    #[test]
    fn p_formats() {
        // (full-scale product) -> decimals and unit.
        let f = |fs: f32| {
            let (_, d, u) = p_format(fs, 0.0, true);
            (d, u)
        };
        assert_eq!(f(6.0 * 600e-6), (4, "mW")); // 3.6 mW
        assert_eq!(f(6.0 * 0.06), (2, "mW")); // 360 mW
        assert_eq!(f(6.0 * 6.0), (3, "W"));
        assert_eq!(f(60.0 * 6.0), (2, "W"));
        assert_eq!(f(600.0 * 6.0), (1, "W"));
        assert_eq!(f(1000.0 * 10.0), (3, "kW"));
        assert_eq!(p_format(0.36, 0.1234, true).0, 123.4);
        assert_eq!(p_format(36.0, 1.0, false).2, "VA");
    }

    #[test]
    fn comparator_windows() {
        // A bipolar signal: the stock window (+-20.8 mV).
        assert_eq!(cmp_window(0.0, 0.1), Some(R21_DEFAULT));
        assert_eq!(cmp_window(0.001, 0.03), Some(R21_DEFAULT));
        // The T1 square on the 60 V tap: 0..96 mV (DC 48 mV, AC 48 mV rms):
        // RH 62.5 mV (0xB), RL 20.8 mV (0xF, the highest RL tap below
        // +0.2 V), both inside 0..96 mV.
        assert_eq!(cmp_window(0.048, 0.048), Some(0xBF));
        // An un-zeroed Y phantom (5 mV) keeps the default window.
        assert_eq!(cmp_window(0.005, 0.1), Some(R21_DEFAULT));
        // Negative DC with ripple: -0.3 V +- 0.15 V.
        let w = cmp_window(-0.3, 0.15).unwrap();
        let rh = CMP_RH.iter().find(|e| e.0 == w >> 4).unwrap().1;
        let rl = CMP_RL.iter().find(|e| e.0 == w & 15).unwrap().1;
        assert!(rh > -0.3 && rh < -0.3 + 0.135 && rl < -0.3 && rl > -0.3 - 0.135, "{rh} {rl}");
        // Pure DC / too small an AC part: no window.
        assert_eq!(cmp_window(0.048, 0.0), None);
        assert_eq!(cmp_window(0.0, 0.01), None);
        assert_eq!(cmp_window(0.5, 0.05), None);
    }

    #[test]
    fn energy_formats() {
        let (x, d, u) = energy_format(0.1234, false);
        assert!((x - 123.4).abs() < 1e-3 && d == 2 && u == "mWh");
        assert_eq!(energy_format(0.5, true), (0.5, 4, "VAh"));
        assert_eq!(energy_format(1.23456, false).1, 4);
        assert_eq!(energy_format(123.45, true).2, "VAh");
        assert_eq!(energy_format(-2500.0, false), (-2.5, 4, "kWh"));
        assert_eq!(energy_format(0.0, false), (0.0, 4, "mWh"));
    }

    /// No negative zero on the readouts (device check c50425f).
    #[test]
    fn no_negative_zero() {
        let mut b = [0u8; 16];
        let (x, d, _) = energy_format(-1.66e-9, false);
        assert_eq!(crate::dmm::format(&mut b, no_neg_zero(x, d), d), "0.0000");
        assert_eq!(crate::dmm::format(&mut b, no_neg_zero(-0.00004, 4), 4), "0.0000");
        assert_eq!(crate::dmm::format(&mut b, no_neg_zero(-0.00006, 4), 4), "-0.0001");
        let pv = PowerVals { e_wh: -1.27e-10, p: -1e-7, ..PowerVals::ZERO };
        let (m, _, _, _) = reading_vals(&pv);
        assert!(!m.v.is_sign_negative());
        assert!(!energy_val(&pv).v.is_sign_negative());
    }

    #[test]
    fn ring_text_format() {
        let mut pw = Pw::new();
        pw.ring[0] = Raw { stat: R34_XY, disc: true, rms: -12345678901, lpf: -7, dt: 107 };
        pw.ring_k = 1;
        let mut b = [0u8; 128];
        let n = pw.ring_text(&mut b);
        assert_eq!(core::str::from_utf8(&b[..n]).unwrap(), "1;XY,1,-12345678901,-7,107");
    }

    /// BT (stock phone protocol): the DC V byte and P in the V group's
    /// digits, 16-bit counts.
    #[test]
    fn bt_mapping() {
        assert_eq!(bt_mode_byte(true, 2), 0x28);
        assert_eq!(bt_mode_byte(false, 0), 0x08);
        assert_eq!(bt_mode_byte(false, 3), 0x20);
        // 600 V x 10 A: 6000 W full scale, 1 decimal.
        let pv = PowerVals { p: 230.02, vr: 2, ik: 5, ..PowerVals::ZERO };
        let v = bt_main(&pv);
        assert_eq!((v.unit, v.dec, v.ol), ("V", 1, false));
        assert!((v.v - 230.02).abs() < 1e-4);
        // Negative (a source): the sign survives.
        assert!(bt_main(&PowerVals { p: -24.0, ..pv }).v < 0.0);
        // 6 V x 600 uA: mW, 4 decimals.
        let m = bt_main(&PowerVals { p: 3.2e-3, vr: 0, ik: 0, ..PowerVals::ZERO });
        assert_eq!((m.unit, m.dec), ("mV", 4));
        // 1000 V x 10 A: kW -> W with no decimals (10600 counts).
        let k = bt_main(&PowerVals { p: 10600.0, vr: 3, ik: 5, ..PowerVals::ZERO });
        assert_eq!((k.unit, k.dec), ("V", 0));
        // 6 V x 10 A over range on both channels (63.65 W at 3 decimals =
        // 63650 counts fits; 70 W drops to 2 decimals).
        assert_eq!(bt_main(&PowerVals { p: 63.65, vr: 0, ik: 5, ..PowerVals::ZERO }).dec, 3);
        assert_eq!(bt_main(&PowerVals { p: 70.0, vr: 0, ik: 5, ..PowerVals::ZERO }).dec, 2);
        // Fallback: S.
        assert!((bt_main(&PowerVals { s: 100.0, p: f32::NAN, alt: true, vr: 2, ik: 5, ..PowerVals::ZERO }).v - 100.0).abs() < 1e-3);
        assert!(bt_main(&PowerVals { v_ol: true, ..pv }).ol);
    }

    /// Phase-cal outcome: the Newton null within +-127 steps or a reason.
    #[test]
    fn cal_outcomes() {
        assert_eq!(cal_outcome(f32::NAN), Err(CAL_FLAT));
        assert_eq!(cal_outcome(130.0), Err(CAL_RANGE));
        assert_eq!(cal_outcome(-200.0), Err(CAL_RANGE));
        assert_eq!(cal_outcome(126.6), Ok(127));
        assert_eq!(cal_outcome(-3.4), Ok(-3));
        assert_eq!(cal_outcome(0.2), Ok(0));
        assert_eq!(CAL_NAMES[CAL_NOT_REACTIVE as usize], "NOTREACTIVE");
        // 127 steps x 0.407 us at 1 kHz = 18.6 deg: CAL_PF_MAX = its sine.
        let deg: f32 = 127.0 * 0.407e-6 * 1000.0 * 360.0;
        assert!((sin_deg(deg) - CAL_PF_MAX).abs() < 0.005);
    }

    fn sin_deg(d: f32) -> f32 {
        // Taylor to x^7 (no libm in the host test crate either way).
        let x = d * core::f32::consts::PI / 180.0;
        x - x * x * x / 6.0 + x.powi(5) / 120.0 - x.powi(7) / 5040.0
    }
}
