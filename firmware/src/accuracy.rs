//! Specified accuracy of a DMM reading: the DM40 user manual V1.5,
//! "Multimeter Technical Parameters" (PDF page 4, printed page 2), as
//! ±(x % of reading + N d). Table and the meaning of "d" on opendm40's
//! 60000-count display: ../docs/dmm-accuracy.md. No dependencies (host
//! test: tools/accuracy-test).
//!
//! `spec` picks the row from the reading's function / range; `err_ulps`
//! gives the absolute uncertainty in units of the reading's last
//! 60000-count digit (`Val::dec`), rounded up.

/// ±(`pct` / 100 % of |reading| + `n` d); `abs`: `n` is in the reading's
/// unit (temperature: +5 deg C / +5 deg F), not in digits.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Spec {
    /// Hundredths of a percent (0.03 % = 3).
    pub pct: u16,
    pub n: u8,
    pub abs: bool,
    /// The function has ranges whose stock full scale is model-dependent
    /// (4 / 5 / 6 x a decade): one d is 10x larger where the stock of a
    /// DM40A / B would show the value on the next range up (`err_ulps`).
    /// 0 = fixed range (continuity, diode, temperature); else the counts
    /// decade of the 60000-count format (10000; capacitance 1000).
    pub decade: u16,
}

/// The manual's rows: (x in 0.01 %, N, decade: 0 fixed range, 1 = 10000
/// counts, 2 = 1000 counts (capacitance)).
const ROWS: [(u16, u8, u8); 13] = [
    (30, 15, 1),  // 0 AC V 600 mV .. 750 V
    (3, 5, 1),    // 1 DC V 600 mV .. 1000 V
    (75, 15, 1),  // 2 AC A 6 A / 10 A
    (50, 15, 1),  // 3 AC mA / uA
    (20, 5, 1),   // 4 DC A 6 A / 10 A
    (15, 5, 1),   // 5 DC mA / uA
    (10, 5, 1),   // 6 ohm 600 ohm .. 6 Mohm (60 Mohm: row 7)
    (20, 10, 1),  // 7 ohm 60 Mohm
    (20, 5, 0),   // 8 continuity
    (100, 5, 0),  // 9 diode
    (250, 30, 2), // 10 capacitance 6 nF .. 60 uF (600 uF .. 60 mF: row 11)
    (350, 30, 2), // 11 capacitance 600 uF .. 60 mF
    (1, 5, 1),    // 12 frequency
];
/// Per HY3131 mode 0..14 (dmm.rs: AC V, DC V, AC mV, DC mV, AC A / mA /
/// uA, DC A / mA / uA, ohm, continuity, diode, capacitance, Hz): the row;
/// +0x40: none on r0 (the 60 mV and 60 ohm ranges, opendm40 only); +0x80: the next row from range
/// SPLIT on (ohm r6 = 60 Mohm, capacitance r5 = 600 uF).
const MODE_ROW: [u8; 15] = [0, 1, 0x40, 0x41, 2, 3, 3, 4, 5, 5, 0xC6, 8, 9, 0x8A, 12];
const SPLIT: [i8; 2] = [6, 5];

/// The manual's row for HY3131 `mode` / `range` (dmm.rs numbering), or
/// None where the manual gives none: AC+DC (`acdc`), the 60 mV ranges
/// (DC / AC mV r0) and 60 ohm (ohm r0), opendm40 only, temperature with any sensor but the
/// K thermocouple (`tc` false: PT100 / PT1000 / NTC are opendm40 only),
/// the power meter (not a mode here) and unknown modes. `f`: the reading
/// is in deg F (temperature only).
pub fn spec(mode: u8, range: i8, acdc: bool, tc: bool, f: bool) -> Option<Spec> {
    if acdc {
        return None;
    }
    if mode == 15 {
        return tc.then_some(Spec { pct: if f { 150 } else { 100 }, n: 5, abs: true, decade: 0 });
    }
    // The experimental 600 Mohm (ohm r7, dmm::OHM600): no spec.
    if mode == 10 && range > 6 {
        return None;
    }
    let m = *MODE_ROW.get(mode as usize)?;
    if m & 0x40 != 0 && range < 1 {
        return None;
    }
    let mut row = (m & 0x3F) as usize;
    if m & 0x80 != 0 && range >= SPLIT[(mode == 13) as usize] {
        row += 1;
    }
    let &(pct, n, d) = ROWS.get(row)?;
    Some(Spec { pct, n, abs: false, decade: [0, 10000, 1000][d as usize] })
}

/// The stock's step-up point per model in units of the decade: range
/// limit factor x overrange factor (dmm.rs FACTOR / OVER: 4 x 1.05, 5 x
/// 1.04, 6 x 1.03; kept in step with them by hand).
const LIMIT: [f32; 3] = [4.2, 5.2, 6.18];
/// 10^dec for dec 0..4 (the readouts' decimals).
const P10: [f32; 5] = [1.0, 10.0, 100.0, 1000.0, 10000.0];

/// Uncertainty of reading `v` (display units, `dec` decimals of the
/// 60000-count format) under `sp` on `model` (0 A, 1 B, 2 C), in units of
/// 10^-dec, rounded up. One d = 10^-dec, x10 where the value is above the
/// model's stock range (|v| x 10^dec > factor x over x decade: the stock
/// would show it on the next range up, with one decimal less; INFERRED,
/// the manual does not say which range a value of a DM40A / B above
/// 40000 / 50000 counts belongs to).
pub fn err_ulps(sp: Spec, v: f32, dec: u8, model: u8) -> u32 {
    let scale = P10[dec.min(4) as usize];
    let counts = if v < 0.0 { -v } else { v } * scale;
    let d = if sp.abs {
        scale
    } else if sp.decade > 0 && counts > LIMIT[model.min(2) as usize] * sp.decade as f32 {
        10.0
    } else {
        1.0
    };
    let e = counts * sp.pct as f32 * 1e-4 + sp.n as f32 * d;
    // Round up, but not a float hair above a whole number (0.03 % of
    // 5.0000 V + 5 d = 20.000001 counts -> 20).
    let k = e as u32;
    if e - k as f32 > 1e-3 { k + 1 } else { k }
}

/// `ulps` rounded to one significant digit, two when the first is 1
/// (owner 2026-10-26: "±0.2", not "±0.201"; "±0.0015", "±0.03"); half
/// up: 201 -> 200, 25 -> 30, 149 -> 150, 96 -> 100.
#[inline(never)]
#[cfg_attr(target_os = "none", link_section = ".text.a2.acc")]
pub fn round_sig(u: u32) -> u32 {
    let mut p = 1;
    while u / p >= 100 || (u / p >= 10 && u / p / 10 != 1) {
        p *= 10;
    }
    (u + p / 2) / p * p
}

/// The digits of readout `s` (the main readout's 7 cells, or the strip's
/// trimmed value: digits, '-', '.', spaces) that are below the
/// uncertainty `ulps` (`err_ulps`, units of the last digit; None = no
/// accuracy): the index from which the cells are drawn grey, `s.len()`
/// (or more) = none. Doc ../docs/dmm-accuracy.md section 5:
/// - p = the place of the uncertainty's most significant digit; digits
///   below p are grey: the last digits(ulps) - 1 digits (ulps 20 at 4
///   decimals: +-0.0020, p = 10^-3, one grey digit);
/// - the point takes the colour of the digit after it;
/// - no exception for the leading digit (owner 2026-10-26): 0.0004 at
///   +-0.0016 has the 4 grey.
/// The extra (averaged) digit after `s` is below p whenever this is Some
/// (ulps >= 1): the caller greys it.
#[inline(never)]
#[cfg_attr(target_os = "none", link_section = ".text.a2.acc")]
pub fn grey_from(s: &[u8], ulps: Option<u32>) -> usize {
    let n = s.len();
    let Some(mut u) = ulps else { return n };
    let mut split = n;
    let mut i = n;
    while u >= 10 && i > 0 {
        i -= 1;
        if s[i].is_ascii_digit() {
            u /= 10;
            split = i;
        }
    }
    if split > 0 && split < n && s[split - 1] == b'.' {
        split -= 1;
    }
    split
}
