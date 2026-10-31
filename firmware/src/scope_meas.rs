//! Scope measurements (scope_gen_ui_v2_6.h section 4): computed on the
//! samples of the visible window, refreshed at most every 300 ms.
//! Stock item IDs: 0 Freq 1 +Duty 2 -Duty 3 +Wid 4 -Wid 5 +Pul 6 -Pul
//! 9 Cycl 10 Vpp 11 Vrms 12 Vmax 13 Vmin 14 Vtop 15 Vbas 16 Vamp 18 Vcrm 19 Vavg
//! (7/8/17 are the cursor readouts). Invalid values: NaN.

use crate::scope::Scope;

pub const ITEMS: [(u8, &str, Unit); 20] = [
    (0, "Freq", Unit::Hz),
    (9, "Cycl", Unit::S),
    (10, "Vpp", Unit::V),
    (11, "Vrms", Unit::V),
    (12, "Vmax", Unit::V),
    (13, "Vmin", Unit::V),
    (14, "Vtop", Unit::V),
    (15, "Vbas", Unit::V),
    (16, "Vamp", Unit::V),
    (19, "Vavg", Unit::V),
    (18, "Vcrm", Unit::V),
    (1, "+Duty", Unit::Pct),
    (2, "-Duty", Unit::Pct),
    (3, "+Wid", Unit::S),
    (4, "-Wid", Unit::S),
    (5, "+Pul", Unit::N),
    (6, "-Pul", Unit::N),
    (7, "X2-1", Unit::S),
    (8, "Xf", Unit::Hz),
    (17, "Y2-1", Unit::V),
];

/// Default selection (stock 0x0804c6ac): Freq, Vpp, Cycl, Vrms.
pub const DEFAULT_SEL: [u8; 4] = [0, 10, 9, 11];

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Unit {
    V,
    Hz,
    S,
    Pct,
    N,
}

/// Results indexed by stock item ID (0..20).
#[derive(Clone, Copy)]
pub struct Results {
    pub v: [f32; 20],
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

pub fn compute(d: &[u16], fs: f32, sc: &Scope) -> Results {
    // Volts are linear in the code: precompute instead of calling the
    // calibrated map per sample (that made a 50k-sample frame take ~100 ms).
    let v_0 = sc.volts(0.0);
    let v_k = sc.volts(1.0) - v_0;
    compute_codes(d, fs, v_0, v_k)
}

/// Sum of (v_0 + v_k c)^2 over `n` samples with code sum `s1` and code
/// square sum `s2`, in f64 (exact integer moments; the f64 expansion keeps
/// the cancellation of a large offset harmless).
fn sum_sq(n: u32, s1: u64, s2: u64, v_0: f32, v_k: f32) -> f64 {
    let (a, k) = (v_0 as f64, v_k as f64);
    n as f64 * a * a + 2.0 * a * k * s1 as f64 + k * k * s2 as f64
}

/// `compute` on codes with volts = v_0 + v_k * code. Per sample only integer
/// work (device check 2026-10-21: 11.4 ms per frame on the device with f32
/// sums and float compares): a histogram (min / max / top / base and the
/// mean / RMS moments come from it), then the edge state machine on integer
/// thresholds, then whole-cycle duty and the integer moments of Vcrm. The
/// sums are exact (integer moments, combined in f64), so the values match
/// the old f32 running sums to their rounding.
pub fn compute_codes(d: &[u16], fs: f32, v_0: f32, v_k: f32) -> Results {
    let mut r = Results { v: [f32::NAN; 20] };
    let n = d.len();
    if n < 4 {
        return r;
    }
    let volts = |c: f32| v_0 + v_k * c;
    let mut hist = [0u16; 1024];
    for &c in d {
        let c = c.min(1023) as usize;
        hist[c] = hist[c].saturating_add(1);
    }
    let (mut lo, mut hi) = (1023u16, 0u16);
    let (mut s1, mut s2) = (0u64, 0u64);
    for (c, &h) in hist.iter().enumerate() {
        if h != 0 {
            lo = lo.min(c as u16);
            hi = c as u16;
            s1 += h as u64 * c as u64;
            s2 += h as u64 * (c * c) as u64;
        }
    }
    let (vmax, vmin) = (volts(hi as f32), volts(lo as f32));
    r.v[12] = vmax;
    r.v[13] = vmin;
    r.v[19] = ((v_0 as f64 * n as f64 + v_k as f64 * s1 as f64) / n as f64) as f32;
    r.v[11] = sqrt((sum_sq(n as u32, s1, s2, v_0, v_k) / n as f64) as f32);
    let clipped = lo == 0 || hi >= 1023;
    r.v[10] = if clipped { f32::NAN } else { vmax - vmin };
    // Top / base: most populated bin in the upper / lower 30 % band, when
    // it is a plateau: the band's best three-bin window holds at least 4x
    // the band's mean for three bins (a wide band), or at least half of the
    // band's samples (a narrow band: a small square, where 4x the mean is
    // more than the whole band). Else (a triangle or ramp fills the band
    // evenly, so its "mode" is any bin of the band) the extreme, Vmax /
    // Vmin (#129).
    let pp_c = (hi - lo) as f32;
    let top_from = (hi as f32 - 0.3 * pp_c) as usize;
    let base_to = (lo as f32 + 0.3 * pp_c) as usize;
    let mode_in = |a: usize, b: usize, ext: usize| -> usize {
        let b = b.min(1023);
        let (mut best, mut sum, mut win) = (a, 0usize, 0usize);
        for i in a..=b {
            sum += hist[i] as usize;
            if hist[i] > hist[best] {
                best = i;
            }
            let w: usize = hist[i.saturating_sub(1).max(a)..=(i + 1).min(b)].iter().map(|&h| h as usize).sum();
            win = win.max(w);
        }
        if win * (b + 1 - a) >= 12 * sum || 2 * win >= sum { best } else { ext }
    };
    let tb = mode_in(top_from, hi as usize, hi as usize);
    let bb = mode_in(lo as usize, base_to, lo as usize);
    let (vtop, vbas) = (volts(tb as f32), volts(bb as f32));
    r.v[14] = vtop;
    r.v[15] = vbas;
    r.v[16] = if bb == 0 || tb == 1023 { f32::NAN } else { vtop - vbas };
    // Edge analysis on codes: 10 / 50 / 90 % of base..top.
    if tb > bb + 4 {
        let span = (tb - bb) as f32;
        let l10 = bb as f32 + 0.1 * span;
        let l50 = bb as f32 + 0.5 * span;
        let l90 = bb as f32 + 0.9 * span;
        // The same tests on integer codes (all three levels are > 0):
        // c >= l <=> c >= ceil(l); c <= l <=> c <= floor(l).
        let ceil = |l: f32| -> u16 {
            let f = l as u16;
            if (f as f32) < l { f + 1 } else { f }
        };
        let (h50, h90, f10) = (ceil(l50), ceil(l90), l10 as u16);
        // State: true = high (passed 90 %), false = low (passed 10 %).
        let mut state: Option<bool> = None;
        let mut rises = [0usize; 2]; // first, last rising 50 % crossing
        let mut nrise = 0u32;
        // ... and falling: a frame of two periods triggered on a rising
        // edge has one whole rise inside (the other two sit on its ends)
        // but two falls (the DMM wave view at 5 ms/div with 40 Hz showed
        // "Freq: --").
        let mut falls = [0usize; 2];
        let mut nfall = 0u32;
        let (mut hp, mut lp) = (0u32, 0u32);
        let mut prev_hi = d[0] >= h50;
        let mut cross50: Option<usize> = None;
        for (i, &c) in d.iter().enumerate() {
            let now_hi = c >= h50;
            if now_hi != prev_hi {
                cross50 = Some(i);
            }
            prev_hi = now_hi;
            match state {
                None => {
                    if c >= h90 {
                        state = Some(true);
                    } else if c <= f10 {
                        state = Some(false);
                    }
                }
                Some(false) if c >= h90 => {
                    state = Some(true);
                    let at = cross50.unwrap_or(i);
                    if nrise == 0 {
                        rises[0] = at;
                    }
                    rises[1] = at;
                    nrise += 1;
                    lp += 1;
                }
                Some(true) if c <= f10 => {
                    state = Some(false);
                    let at = cross50.unwrap_or(i);
                    if nfall == 0 {
                        falls[0] = at;
                    }
                    falls[1] = at;
                    nfall += 1;
                    hp += 1;
                }
                _ => {}
            }
        }
        // Whole cycles between the first and the last rise, else fall.
        let edges = if nrise >= 2 && rises[1] > rises[0] {
            Some((nrise, rises))
        } else if nfall >= 2 && falls[1] > falls[0] {
            Some((nfall, falls))
        } else {
            None
        };
        if let Some((ne, e)) = edges {
            let cycles = (ne - 1) as f32;
            let samples = (e[1] - e[0]) as f32;
            let f = fs * cycles / samples;
            r.v[0] = f;
            r.v[9] = 1.0 / f;
            // Duty / widths over whole cycles.
            let mut hn = 0u32;
            let (mut c1, mut c2) = (0u64, 0u64);
            let w = &d[e[0]..e[1]];
            for &c in w {
                hn += (c >= h50) as u32;
                c1 += c as u64;
                c2 += (c as u32 * c as u32) as u64;
            }
            let cnt = w.len() as u32;
            let ln = cnt - hn;
            let tot = (hn + ln).max(1) as f32;
            r.v[1] = hn as f32 * 100.0 / tot;
            r.v[2] = 100.0 - r.v[1];
            r.v[3] = hn as f32 / cycles / fs;
            r.v[4] = ln as f32 / cycles / fs;
            r.v[18] = sqrt((sum_sq(cnt, c1, c2, v_0, v_k) / cnt.max(1) as f64) as f32);
        }
        r.v[5] = hp as f32;
        r.v[6] = lp as f32;
    }
    r
}

/// "label value unit" in the stock style (<= 15 chars).
#[allow(dead_code)] // tools (ui-render); the app uses format_unit
pub fn format<'a>(buf: &'a mut [u8; 24], id: u8, v: f32) -> &'a str {
    format_unit(buf, id, v, false)
}

/// `format`; `amps`: the vertical items are currents (the DMM wave view's
/// A rows): "Irms: 12.3mA" instead of "Vrms: 12.3mV".
pub fn format_unit<'a>(buf: &'a mut [u8; 24], id: u8, v: f32, amps: bool) -> &'a str {
    let (name, unit) = ITEMS.iter().find(|i| i.0 == id).map(|i| (i.1, i.2)).unwrap_or(("?", Unit::V));
    let amps = amps && unit == Unit::V;
    let mut n = 0;
    let mut put = |b: &mut [u8; 24], s: &[u8]| {
        let k = s.len().min(24 - n);
        b[n..n + k].copy_from_slice(&s[..k]);
        n += k;
    };
    if amps && name.starts_with('V') {
        put(buf, b"I");
        put(buf, &name.as_bytes()[1..]);
    } else {
        put(buf, name.as_bytes());
    }
    put(buf, b":");
    if !v.is_finite() || (v < 0.0 && unit != Unit::V) {
        put(buf, b" --");
        return core::str::from_utf8(&buf[..n]).unwrap_or("");
    }
    // Scale on a value nudged up by 1e-6 so 0.999999e-3 s shows as 1.00 ms.
    let v = v * 1.000_001;
    let (scaled, pre): (f32, &str) = match unit {
        Unit::V => {
            let a = v.abs();
            if a >= 1e3 {
                (v / 1e3, "kV")
            } else if a >= 1.0 || a == 0.0 {
                (v, "V")
            } else if a >= 1e-3 {
                (v * 1e3, "mV")
            } else {
                (v * 1e6, "uV")
            }
        }
        Unit::Hz => {
            if v >= 1e6 {
                (v / 1e6, "MHz")
            } else if v >= 1e3 {
                (v / 1e3, "kHz")
            } else if v >= 1.0 {
                (v, "Hz")
            } else {
                (v * 1e3, "mHz")
            }
        }
        Unit::S => {
            if v >= 1.0 {
                (v, "s")
            } else if v >= 1e-3 {
                (v * 1e3, "ms")
            } else if v >= 1e-6 {
                (v * 1e6, "us")
            } else {
                (v * 1e9, "ns")
            }
        }
        Unit::Pct => (v, "%"),
        Unit::N => (v, ""),
    };
    let a = scaled.abs();
    let dec = if unit == Unit::N {
        0
    } else if a < 9.995 {
        2
    } else if a < 99.95 {
        1
    } else {
        0
    };
    let mut t = [0u8; 16];
    let s = crate::dmm::format(&mut t, scaled, dec);
    put(buf, s.as_bytes());
    if amps {
        // The prefix only, then the unit (pre = "kV" / "V" / "mV" / "uV").
        put(buf, &pre.as_bytes()[..pre.len() - 1]);
        put(buf, b"A");
    } else {
        put(buf, pre.as_bytes());
    }
    core::str::from_utf8(&buf[..n]).unwrap_or("")
}
