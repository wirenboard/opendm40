// Host test of firmware/src/genplan.rs (run.sh builds and runs this).
// Checks the generator frequency search (register ranges, error bounds,
// exact hits, comparison with the stock algorithm and with a brute-force
// single-period optimum), the per-wave clamp (UF-14), the PWM path, table
// contents (rising ramp, UF-16), and levels/offset (FR-28).

#[path = "../../src/genplan.rs"]
mod genplan;

use genplan::*;
use std::time::Instant;

static mut FAILS: u32 = 0;

macro_rules! check {
    ($c:expr, $($a:tt)*) => {
        if !$c {
            unsafe { FAILS += 1 };
            println!("FAIL line {}: {}", line!(), format!($($a)*));
        }
    };
}

fn ppm(t: &Timing, f: u32) -> f64 {
    (t.hz() - f as f64).abs() / f as f64 * 1e6
}

/// The stock DAC algorithm (FUN_08016530, without its 1 kHz snap):
/// f > 5000: PSC 0, n = 10e6 / f; else k = round(5000 / f), n = round(10e6 / (f k)).
fn stock_hz(f: u32) -> f64 {
    let (k, n) = if f > 5000 {
        (1, 10_000_000 / f)
    } else {
        let k = ((5000 + f / 2) / f).max(1);
        (k, (10_000_000 + f * k / 2) / (f * k))
    };
    10e6 / (k as f64 * n.clamp(2, 2000) as f64)
}

/// Best single-period (K = 1) error over every L in n_min..=2000 and every
/// divisor M >= 20 that PSC = 0 can express (M <= 65536). Hz.
fn brute_k1(f: u32, n_min: u32) -> f64 {
    let mut best = f64::MAX;
    for n in n_min..=2000u32 {
        let m0 = 200_000_000u64 / (f as u64 * n as u64);
        for m in [m0, m0 + 1] {
            if m < 20 || m > 65536 {
                continue;
            }
            let e = (200e6 / (m as f64 * n as f64) - f as f64).abs();
            best = best.min(e);
        }
    }
    best
}

fn check_timing(w: Wave, f: u32, t: &Timing) {
    let tgt = target_hz(w, f);
    match t.path {
        Path::Dac => {
            check!(t.div() >= DIV_MIN as u64, "{w:?} {f}: div {} < 20", t.div());
            check!(t.len as usize <= N_MAX && t.len >= 2, "{w:?} {f}: len {}", t.len);
            check!(t.cycles >= 1, "{w:?} {f}: cycles 0");
            check!(
                t.len as u32 >= t.cycles as u32 * w.n_min(),
                "{w:?} {f}: {} samples/period < n_min",
                t.samples_per_period()
            );
            if w == Wave::Square {
                check!(t.cycles == 1, "{w:?} {f}: square with {} periods per table", t.cycles);
            }
        }
        Path::Pwm => {
            check!(w == Wave::Square && tgt > SQUARE_DAC_MAX, "{w:?} {f}: PWM path");
            check!(t.psc == 0 && t.len == 1 && t.cycles == 1, "{w:?} {f}: PWM fields {t:?}");
            check!(t.div() >= 20 && t.div() <= 4000, "{w:?} {f}: PWM div {}", t.div());
        }
    }
    // Exact ratio = 200 MHz K / (M L) in integers.
    let (n, d) = t.hz_ratio();
    check!(n == 200_000_000u64 * t.cycles as u64, "{w:?} {f}: ratio numerator");
    check!(d == (t.psc as u64 + 1) * (t.arr as u64 + 1) * t.len as u64, "{w:?} {f}: ratio denominator");
}

fn main() {
    let quick = std::env::args().any(|a| a == "quick");

    // ---- 1. DAC sweep 1 Hz .. 50 kHz: invariants, errors, stock comparison.
    let step = if quick { 7 } else { 1 };
    for &(w, name) in &[(Wave::Sine, "sine/tri/ramp"), (Wave::Square, "square (DAC)")] {
        let (mut max_ppm, mut max_f, mut max_stock, mut multi) = (0f64, 0u32, 0f64, 0u32);
        let mut hist = [0u32; 5]; // <=0.01, <=1, <=10, <=50, >50 ppm
        let (mut worst_us, mut total_us, mut nplans) = (0f64, 0f64, 0u32);
        let mut worse_than_stock = 0u32;
        let mut worse_than_k1 = 0u32;
        let mut max_k1 = 0f64;
        let mut f = 1u32;
        while f <= 50_000 {
            let t0 = Instant::now();
            let t = plan(w, f);
            let us = t0.elapsed().as_secs_f64() * 1e6;
            worst_us = worst_us.max(us);
            total_us += us;
            nplans += 1;
            check!(t.path == Path::Dac, "{w:?} {f}: not DAC");
            check_timing(w, f, &t);
            let e = ppm(&t, f);
            if e > max_ppm {
                max_ppm = e;
                max_f = f;
            }
            hist[if e <= 0.01 { 0 } else if e <= 1.0 { 1 } else if e <= 10.0 { 2 } else if e <= 50.0 { 3 } else { 4 }] += 1;
            if t.cycles > 1 {
                multi += 1;
            }
            let es = (stock_hz(f) - f as f64).abs() / f as f64 * 1e6;
            max_stock = max_stock.max(es);
            if e > es + 1.0 + 1e-6 {
                worse_than_stock += 1;
                println!("  worse than stock: {w:?} {f} Hz: {e:.3} ppm vs {es:.3} ppm, {t:?}");
            }
            // Never worse than the best single-period table over the whole
            // n_min..2000 range by more than 1 ppm (phase A trades up to
            // 1 ppm for more samples per period).
            if f % 97 == 0 || quick || w == Wave::Square {
                let b = brute_k1(f, w.n_min()) / f as f64 * 1e6;
                if b.is_finite() {
                    max_k1 = max_k1.max(b);
                }
                if e > b + 1.0 + 1e-6 {
                    worse_than_k1 += 1;
                    if worse_than_k1 <= 5 {
                        println!("  note: {w:?} {f} Hz: {e:.2} ppm vs brute-force K=1 {b:.2} ppm");
                    }
                }
            }
            f += step;
        }
        println!(
            "{name}: {nplans} freqs, max error {max_ppm:.1} ppm at {max_f} Hz (stock algorithm: {max_stock:.0} ppm, \
             best K=1 table: {max_k1:.0} ppm); \
             <=0.01/<=1/<=10/<=50/>50 ppm: {:?}; multi-period tables {multi}; plan time avg {:.1} us, worst {worst_us:.0} us (host)",
            hist,
            total_us / nplans as f64
        );
        // Sine/triangle/ramp: multi-period tables keep it near the 1/(2 D)
        // limit of D = 4000 at 50 kHz (125 ppm). Square (K = 1 only): bounded
        // by the best single-period table, checked above for every frequency.
        let bound = if w == Wave::Square { 1000.0 } else { 130.0 };
        check!(max_ppm <= bound, "{name}: max error {max_ppm} ppm > {bound}");
        check!(worse_than_stock == 0, "{name}: {worse_than_stock} frequencies worse than the stock algorithm");
        check!(worse_than_k1 == 0, "{name}: {worse_than_k1} frequencies > 1 ppm worse than the brute-force K=1 optimum");
    }

    // ---- 2. Pinned frequencies.
    println!("pinned (wave, set Hz -> actual Hz, M, L, K, samples/period):");
    for &(w, f) in &[
        (Wave::Sine, 1u32),
        (Wave::Sine, 7),
        (Wave::Sine, 1000),
        (Wave::Sine, 12345),
        (Wave::Sine, 32768),
        (Wave::Sine, 44100),
        (Wave::Sine, 50000),
        (Wave::Square, 32768),
        (Wave::Square, 50000),
        (Wave::Square, 50001),
        (Wave::Square, 75000),
        (Wave::Square, 2_000_000),
        (Wave::Square, 7_000_000),
        (Wave::Square, 10_000_000),
    ] {
        let t = plan(w, f);
        println!(
            "  {w:?} {f} -> {:.6} ({:.2} ppm) M={} L={} K={} {:.2}",
            t.hz(),
            ppm(&t, f),
            t.div(),
            t.len,
            t.cycles,
            t.samples_per_period()
        );
    }
    for &f in &[1u32, 2, 5, 10, 100, 1000, 10_000, 20_000, 25_000, 40_000, 50_000] {
        for w in Wave::ALL {
            let t = plan(w, f);
            check!(ppm(&t, f) == 0.0, "{w:?} {f} Hz not exact: {}", t.hz());
            check!(t.cycles == 1, "{w:?} {f} Hz exact but multi-period");
        }
    }
    // Low frequencies: n = 2000 samples (finest waveform) at 1 Hz and 1 kHz.
    check!(plan(Wave::Sine, 1).len == 2000, "1 Hz: len {}", plan(Wave::Sine, 1).len);
    check!(plan(Wave::Sine, 1000).len == 2000, "1 kHz: len {}", plan(Wave::Sine, 1000).len);
    // 32768 Hz (the UF-15 complaint, stock gives 32787 Hz = 580 ppm).
    let t = plan(Wave::Sine, 32768);
    check!(ppm(&t, 32768) < 10.0, "sine 32768: {} ppm", ppm(&t, 32768));
    let t = plan(Wave::Square, 32768);
    check!(ppm(&t, 32768) < 100.0, "square 32768: {} ppm", ppm(&t, 32768));

    // ---- 3. PWM path 50 kHz .. 10 MHz: nearest divisor.
    let mut f = 50_001u32;
    let mut n = 0;
    while f <= 10_000_000 {
        let t = plan(Wave::Square, f);
        check!(t.path == Path::Pwm, "square {f}: path {:?}", t.path);
        check_timing(Wave::Square, f, &t);
        let d = t.div() as f64;
        let e = (200e6 / d - f as f64).abs();
        for alt in [d - 1.0, d + 1.0] {
            if alt >= 20.0 {
                check!(e <= (200e6 / alt - f as f64).abs() + 1e-9, "square {f}: D {d} not nearest");
            }
        }
        n += 1;
        f += if quick { 99_991 } else { 997 };
    }
    println!("PWM: {n} frequencies checked (nearest 200 MHz / D, D 20..4000)");

    // ---- 4. Per-wave clamp (UF-14): never 0 Hz, set value kept.
    for w in Wave::ALL {
        for &f in &[1u32, 49_999, 50_000, 50_001, 60_000, 99_999, 100_000, 2_000_000, 10_000_000, 20_000_000] {
            let s = Settings { wave: w, freq_hz: f, ..Settings::DEFAULT };
            let o = compute(&s);
            check!(o.target_hz > 0 && o.hz() > 0.0, "{w:?} {f}: silent");
            check!(o.target_hz == f.clamp(1, w.max_hz()), "{w:?} {f}: target {}", o.target_hz);
            check!(o.freq_clamped == (f > w.max_hz()), "{w:?} {f}: clamp flag");
            check!(s.normalised().freq_hz == f.min(F_MAX), "{w:?} {f}: set frequency changed");
            check!(limits(w).f_max == w.max_hz(), "{w:?}: limits");
        }
    }
    for w in Wave::ALL {
        let m = w.max_hz();
        let txt = if m >= 1_000_000 { format!("max {} MHz", m / 1_000_000) } else { format!("max {} kHz", m / 1000) };
        check!(w.max_note().contains(&txt), "{w:?}: note '{}' vs {m} Hz", w.max_note());
    }
    // As Gen::apply does it: compute() on the raw settings, store normalised.
    let raw = Settings { wave: Wave::Square, freq_hz: 20_000_000, ..Settings::DEFAULT };
    check!(compute(&raw).freq_clamped && raw.normalised().freq_hz == F_MAX, "square 20 MHz clamp flag via apply path");
    check!(compute(&raw).timing == compute(&raw.normalised()).timing, "raw vs normalised timing");
    check!(Wave::Sine.max_hz() == 50_000 && Wave::Square.max_hz() == 10_000_000, "max_hz");
    let o = compute(&Settings { wave: Wave::Sine, freq_hz: 2_000_000, ..Settings::DEFAULT });
    check!(o.freq_clamped && o.hz() == 50_000.0, "sine 2 MHz -> {} (clamped {})", o.hz(), o.freq_clamped);
    let o = compute(&Settings { wave: Wave::Square, freq_hz: 75_000, ..Settings::DEFAULT });
    check!(!o.freq_clamped && (o.hz() - 75_000.0).abs() < 20.0, "square 75 kHz -> {}", o.hz());

    // ---- 5. sin() accuracy.
    let mut me = 0f64;
    for i in 0..100_000 {
        let x = i as f64 / 100_000.0 * 2.0 * std::f64::consts::PI;
        me = me.max((sin(x as f32) as f64 - x.sin()).abs());
    }
    println!("sin(): max error {me:.2e}");
    check!(me < 5e-6, "sin error {me}");

    // ---- 6. Table contents.
    let mut tab = [0u16; N_MAX];
    let base = Settings { on: true, freq_hz: 1000, amp_v: 3.0, duty: 5000, offset_v: None, wave: Wave::Ramp };
    let t = plan(Wave::Ramp, 1000);
    fill(&mut tab, &base, &t);
    let l = t.len as usize;
    check!(tab[..l].windows(2).all(|p| p[1] >= p[0]), "ramp not rising");
    check!(tab[0] == 0 && tab[l - 1] > 4080, "ramp ends {} {}", tab[0], tab[l - 1]);
    // Multi-period ramp: rising inside each period, K wraps per table.
    let f = (10_000..50_000).find(|&f| plan(Wave::Ramp, f).cycles > 1).expect("no multi-period plan");
    let t = plan(Wave::Ramp, f);
    let s = Settings { freq_hz: f, ..base };
    fill(&mut tab, &s, &t);
    let l = t.len as usize;
    let drops = tab[..l].windows(2).filter(|p| p[1] < p[0]).count() + (tab[0] < tab[l - 1]) as usize;
    check!(drops == t.cycles as usize, "ramp {f} Hz: {drops} drops for K={}", t.cycles);
    println!("multi-period example: ramp {f} Hz -> L={} K={} ({:.2} samples/period, {:.4} Hz)", t.len, t.cycles, t.samples_per_period(), t.hz());
    // Triangle: peak at the middle, ends low.
    let s = Settings { wave: Wave::Triangle, ..base };
    let t = plan(Wave::Triangle, 1000);
    fill(&mut tab, &s, &t);
    let l = t.len as usize;
    let imax = (0..l).max_by_key(|&i| tab[i]).unwrap();
    check!(imax.abs_diff(l / 2) <= 1 && tab[0] == 0, "triangle peak at {imax} of {l}");
    // Sine: starts at mid level and rises; stock levels 50..4095 at 3 Vpp.
    let s = Settings { wave: Wave::Sine, ..base };
    fill(&mut tab, &s, &t);
    let (mn, mx) = (tab[..l].iter().min().unwrap(), tab[..l].iter().max().unwrap());
    check!(*mn == 50 && *mx == 4095, "sine 3 Vpp levels {mn}..{mx}");
    check!(tab[1] > tab[0] && (tab[0] as i32 - 2073).abs() <= 1, "sine start {}", tab[0]);
    // Square: round(L duty) high samples at 25 %.
    let s = Settings { wave: Wave::Square, duty: 2500, ..base };
    let t = plan(Wave::Square, 1000);
    fill(&mut tab, &s, &t);
    let hi = tab[..t.len as usize].iter().filter(|&&v| v == 4095).count();
    check!(hi == t.len as usize / 4, "square 25 %: {hi} of {}", t.len);
    check!(compute(&s).duty == 2500, "square 25 %: duty {}", compute(&s).duty);

    // ---- 7. Levels and offset (FR-28).
    let k = 4095.0 / 3.0;
    let lv = |s: Settings| {
        let o = compute(&s);
        (o.lo_code, o.hi_code)
    };
    let sq = Settings { wave: Wave::Square, freq_hz: 1000, ..base };
    check!(lv(sq) == (0, 4095), "square stock 3 V {:?}", lv(sq));
    check!(lv(Settings { amp_v: 1.0, ..sq }) == (0, 1365), "square stock 1 V {:?}", lv(Settings { amp_v: 1.0, ..sq }));
    // Stock sine lift at 1.5 Vpp: a = 0.5 -> 200a .. 4295a = 100 .. 2147.5.
    let sn = Settings { wave: Wave::Sine, amp_v: 1.5, ..sq };
    check!(lv(sn) == (100, 2148), "sine stock 1.5 V {:?}", lv(sn));
    // Offset 1.5 V, 1 Vpp -> 1.0 .. 2.0 V.
    let s = sq.with_amp(1.0).with_offset(Some(1.5));
    check!(lv(s) == ((1.0 * k + 0.5) as u16, (2.0 * k + 0.5) as u16), "offset 1.5/1 Vpp {:?}", lv(s));
    let o = compute(&s);
    check!((o.offset_v().unwrap() - 1.5).abs() < 0.001 && (o.vpp_v().unwrap() - 1.0).abs() < 0.002, "effective {:?} {:?}", o.offset_v(), o.vpp_v());
    // Sine with an explicit offset: no stock lift.
    let s = Settings { wave: Wave::Sine, ..s };
    check!(lv(s) == (1365, 2730), "sine offset {:?}", lv(s));
    // Offset clamped into [Vpp/2, 3 - Vpp/2].
    check!(sq.with_amp(1.0).with_offset(Some(0.2)).offset_v == Some(0.5), "offset low clamp");
    check!(sq.with_amp(1.0).with_offset(Some(2.9)).offset_v == Some(2.5), "offset high clamp");
    check!(sq.with_amp(3.0).with_offset(Some(2.0)).offset_v == Some(1.5), "offset at 3 Vpp");
    // Vpp set against an offset: the Vpp is clamped.
    check!(sq.with_amp(1.0).with_offset(Some(0.5)).with_amp(3.0).amp_v == 1.0, "Vpp limited by offset");
    check!(sq.with_amp(1.0).with_offset(Some(2.0)).with_amp(2.5).amp_v == 2.0, "Vpp limited by offset 2.0");
    // Full settings with a conflict: Vpp kept, offset moved.
    let s = Settings { amp_v: 2.0, offset_v: Some(2.5), ..sq }.normalised();
    check!(s.amp_v == 2.0 && s.offset_v == Some(2.0), "normalise conflict {:?} {:?}", s.amp_v, s.offset_v);
    // DC source: Vpp 0 at 1.234 V.
    let s = sq.with_amp(0.0).with_offset(Some(1.234));
    let o = compute(&s);
    check!(o.lo_code == o.hi_code && (o.offset_v().unwrap() - 1.234).abs() < 0.001, "DC {:?}", o.offset_v());
    // Back to stock.
    check!(s.with_offset(None).offset_v.is_none(), "offset None");
    // Bad input.
    let s = Settings { amp_v: f32::NAN, offset_v: Some(f32::INFINITY), ..sq }.normalised();
    check!(s.amp_v == 3.0 && s.offset_v.is_none(), "NaN handling");
    check!(Settings { amp_v: 9.0, ..sq }.normalised().amp_v == 3.0 && Settings { amp_v: -1.0, ..sq }.normalised().amp_v == 0.0, "amp clamp");
    // PWM path: levels not adjustable, duty quantised to the counter.
    let o = compute(&Settings { freq_hz: 1_000_000, duty: 2500, ..sq });
    check!(o.fixed_level() && o.vpp_v().is_none() && o.offset_v().is_none(), "PWM levels");
    check!(o.duty == 2500 && pwm_ccr(&o.timing, 2500) == 50, "PWM 1 MHz 25 %: {} ccr {}", o.duty, pwm_ccr(&o.timing, 2500));
    let o = compute(&Settings { freq_hz: 10_000_000, duty: 3333, ..sq });
    check!(o.duty == 3500 && pwm_ccr(&o.timing, 3333) == 7, "PWM 10 MHz 33 %: {}", o.duty);
    check!(pwm_ccr(&o.timing, 10000) == 20 && pwm_ccr(&o.timing, 0) == 0, "PWM 0/100 %");

    // The scope's GEN channel (docs/scope-gen-channel.md): crossings, spans,
    // trigger instants.
    for w in Wave::ALL {
        let sh = Shape { wave: w, lo_v: 0.5, hi_v: 2.5, duty: 0.3 };
        for &(v, rising) in &[(1.0f32, true), (1.0, false), (2.2, true), (2.2, false), (0.6, true)] {
            let p = sh.crossing(v, rising);
            check!(p.is_some_and(|p| (0.0..1.0).contains(&p)), "{w:?} crossing {v} {rising}: {p:?}");
            let p = p.unwrap_or(0.0);
            let (b, a) = (sh.volts(p - 1e-4), sh.volts(p + 1e-4));
            let ok = if rising { b <= v + 1e-3 && a >= v - 1e-3 && a > b } else { b >= v - 1e-3 && a <= v + 1e-3 && a < b };
            check!(ok, "{w:?} crossing {v} {rising} at {p}: {b} -> {a}");
        }
        check!(sh.crossing(0.4, true).is_none() && sh.crossing(2.6, false).is_none() && sh.crossing(0.5, true).is_none(), "{w:?} out of range");
        check!(sh.span(0.37, 1.0) == (0.5, 2.5) && sh.span(0.1, 3.0) == (0.5, 2.5), "{w:?} full span");
        // A span covers every sampled value inside it.
        for k in 0..50 {
            let (a, l) = (k as f32 * 0.0731, (k % 7) as f32 * 0.09);
            let (lo, hi) = sh.span(a, l);
            for j in 0..=20 {
                let v = sh.volts(a + l * j as f32 / 20.0);
                check!(v >= lo - 1e-4 && v <= hi + 1e-4, "{w:?} span {a} {l}: {v} outside {lo}..{hi}");
            }
        }
    }
    // Sync: 1 kHz sine table against a 1 MS/s record (200 ticks / sample).
    let s = Settings { on: true, wave: Wave::Sine, freq_hz: 1000, ..Settings::DEFAULT };
    let o = compute(&s);
    let (q, k) = loop_ticks(&o.timing);
    check!(q as f64 / k as f64 == 200_000.0, "1 kHz loop {q} / {k}");
    let sy = Sync { t0: 12_345.0, s: 200.0, q: q as f64, k: k as f64, shape: Shape::of(&s, &o) };
    check!(sy.period_samples() == 1000.0, "period samples {}", sy.period_samples());
    let ph = sy.shape.crossing(1.5, true).unwrap();
    let mut n0 = 0u64;
    for _ in 0..5 {
        let n = sy.next(n0, ph);
        let d = (sy.phase(n as f64) - ph).rem_euclid(1.0);
        // The first sample at or past the instant: within one sample after it.
        check!(n >= n0 && (d as f64) < 1.0 / sy.period_samples() + 1e-6, "next {n0} -> {n}: phase past {d}");
        check!(n - n0 <= 1000, "next {n0} -> {n}: more than a period");
        n0 = n + 1;
    }
    // PWM path: 1 MHz square, D = 200 ticks.
    let s = Settings { on: true, freq_hz: 1_000_000, ..Settings::DEFAULT };
    let o = compute(&s);
    check!(loop_ticks(&o.timing) == (200, 1) && Shape::of(&s, &o).hi_v == PWM_HIGH_V, "PWM loop / level");

    let fails = unsafe { FAILS };
    if fails == 0 {
        println!("ALL OK");
    } else {
        println!("{fails} FAILED");
        std::process::exit(1);
    }
}
