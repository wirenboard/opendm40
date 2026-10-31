//! Host test of src/sinc.rs (UF-41): the polynomial sin / cos, the Lanczos-3
//! weights (sum 1, against a direct f64 kernel), sample and DC pass-through,
//! a sine of 2.5..8 samples per period reconstructed from 10-bit codes on a
//! 399-column grid (sinc vs linear error), edges and degenerate records.
//!   tools/sinc-test/run.sh
#[allow(dead_code)]
#[path = "../../src/pack10.rs"]
mod pack10;
#[path = "../../src/sinc.rs"]
mod sinc;

use std::f64::consts::PI;

static mut FAILS: u32 = 0;
static mut CASES: u32 = 0;

fn check(name: &str, ok: bool, msg: String) {
    unsafe {
        CASES += 1;
        if !ok {
            FAILS += 1;
            println!("FAIL {name}: {msg}");
        }
    }
}

fn lanczos(d: f64) -> f64 {
    if d.abs() < 1e-12 {
        1.0
    } else if d.abs() >= 3.0 {
        0.0
    } else {
        3.0 * (PI * d).sin() * (PI * d / 3.0).sin() / (PI * PI * d * d)
    }
}

fn main() {
    // sin / cos.
    let mut e: f64 = 0.0;
    for i in -2000..=2000 {
        let x = i as f64 * 0.005;
        e = e.max((sinc::sin(x as f32) as f64 - x.sin()).abs());
        e = e.max((sinc::cos(x as f32) as f64 - x.cos()).abs());
    }
    check("sin/cos", e < 2e-6, format!("max error {e:e}"));

    // Weights: normalised Lanczos-3 for fr in [0, 1).
    let mut ew: f64 = 0.0;
    for i in 0..200 {
        let fr = i as f64 / 200.0;
        let w = sinc::weights(fr as f32);
        let r: Vec<f64> = (-2..=3).map(|j| lanczos(fr - j as f64)).collect();
        let rs: f64 = r.iter().sum();
        let sum: f32 = w.iter().sum();
        check("weights sum", (sum - 1.0).abs() < 1e-5, format!("fr {fr}: sum {sum}"));
        for k in 0..6 {
            ew = ew.max((w[k] as f64 - r[k] / rs).abs());
        }
    }
    check("weights vs f64 kernel", ew < 1e-5, format!("max error {ew:e}"));
    // fr just below 1 (p - floor(p) can give 1 - ulp): finite, sum 1, the
    // weight moves to the next sample.
    let w = sinc::weights(1.0 - f32::EPSILON);
    check("weights fr = 1 - ulp", w.iter().all(|v| v.is_finite()) && (w.iter().sum::<f32>() - 1.0).abs() < 1e-4 && w[3] > 0.99, format!("{w:?}"));

    // Samples and DC pass through.
    let src: Vec<u16> = (0..40).map(|i| ((i * 37 + 11) % 1024) as u16).collect();
    for (i, &v) in src.iter().enumerate() {
        let y = sinc::at(&src, i as f32);
        check("sample pass-through", (y - v as f32).abs() < 1e-3, format!("i {i}: {y} vs {v}"));
    }
    let dc = vec![512u16; 25];
    for i in 0..240 {
        let y = sinc::at(&dc, i as f32 * 0.1);
        check("DC flat", (y - 512.0).abs() < 1e-2, format!("p {}: {y}", i as f32 * 0.1));
    }

    // Sine reconstruction: n samples of a sine with `spp` samples per
    // period, 10-bit codes, drawn over 399 columns. Ignore 3 samples at each
    // end (edge clamping); the interior error must be well under linear's.
    for &spp in &[2.5f64, 3.0, 4.0, 5.0, 8.0] {
        let n = 60usize;
        let ph = 0.3;
        let code = |t: f64| 512.0 + 400.0 * (2.0 * PI * t / spp + ph).sin();
        let src: Vec<u16> = (0..n).map(|i| code(i as f64).round() as u16).collect();
        let cols = 399;
        let (mut es, mut el) = (0.0f64, 0.0f64);
        for c in 0..cols {
            let p = c as f64 * (n - 1) as f64 / (cols - 1) as f64;
            if p < 3.0 || p > (n - 4) as f64 {
                continue;
            }
            let ideal = code(p);
            let s = sinc::at(&src, p as f32) as f64;
            let k = p as usize;
            let fr = p - k as f64;
            let l = src[k] as f64 + (src[(k + 1).min(n - 1)] as f64 - src[k] as f64) * fr;
            es = es.max((s - ideal).abs());
            el = el.max((l - ideal).abs());
        }
        // Lanczos-3 passband droop grows toward Nyquist (2.5 samples per
        // period = 0.4 fs is its hardest case: ~35 % of the amplitude);
        // from 4 samples per period it is within ~2.5 %. Always at most
        // about half the linear error. (Measured 141 / 36 / 8 / 10 / 3
        // codes vs linear 276 / 195 / 104 / 77 / 30.)
        let lim = if spp < 3.0 { 160.0 } else if spp < 3.5 { 45.0 } else { 12.0 };
        check(&format!("sine {spp} samples/period"), es < lim && es < 0.55 * el, format!("sinc max error {es:.1} codes (limit {lim}), linear {el:.1}"));
        println!("sine {spp:>4} samples/period: max error sinc {es:6.1} codes, linear {el:6.1} (amplitude 400)");
    }

    // Edges, degenerate records.
    let src = [100u16, 900];
    check("2 samples, ends", (sinc::at(&src, 0.0) - 100.0).abs() < 1e-3 && (sinc::at(&src, 1.0) - 900.0).abs() < 1e-3, String::new());
    check("clamp outside", (sinc::at(&src, -5.0) - 100.0).abs() < 1e-3 && (sinc::at(&src, 9.0) - 900.0).abs() < 1e-3, String::new());
    check("1 sample", sinc::at(&[300], 0.7) == 300.0, String::new());
    check("empty", sinc::at(&[], 0.7) == 0.0, String::new());
    let mut out = [0.0f32; 5];
    sinc::resample(&[0, 1000, 0, 1000], 0.0, 0.75, &mut out);
    check("resample = at", (0..5).all(|i| out[i] == sinc::at(&[0, 1000, 0, 1000], i as f32 * 0.75)), format!("{out:?}"));

    // Step: overshoot bounded (Gibbs, Lanczos-3 ~ 10 %).
    let step: Vec<u16> = (0..30).map(|i| if i < 15 { 100 } else { 900 }).collect();
    let (mut lo, mut hi) = (f32::MAX, f32::MIN);
    for i in 0..290 {
        let y = sinc::at(&step, i as f32 * 0.1);
        lo = lo.min(y);
        hi = hi.max(y);
    }
    let os = ((hi - 900.0).max(100.0 - lo)) / 800.0;
    check("step overshoot", os < 0.15, format!("{:.1} %", os * 100.0));
    println!("step overshoot {:.1} %", os * 100.0);

    let (f, c) = unsafe { (FAILS, CASES) };
    println!("cases {c}, failures {f}");
    std::process::exit(if f == 0 { 0 } else { 1 });
}
