//! Host test of src/fft.rs (UF-11 scope FFT): the packed real FFT against a
//! direct DFT, the dBV scale and the interpolated peak for every window on
//! bin-centred and off-bin sines (10-bit codes as the scope delivers them),
//! the decimation path, the sliced stepping, and the helpers (ln, exp2,
//! 1-2-5 steps). Needs no hardware.
//!   tools/fft-test/run.sh
#[allow(dead_code)]
#[path = "../../src/pack10.rs"]
mod pack10;
#[path = "../../src/fft.rs"]
mod fft;

use fft::Fft;

/// Scope-like code map: code = (v - A) / B, 10 bits.
const A: f32 = -5.0;
const B: f32 = 10.0 / 1023.0;

fn codes(f: impl Fn(f64) -> f64, n: usize, fs: f64) -> Vec<u16> {
    (0..n).map(|i| (((f(i as f64 / fs) as f32 - A) / B).round().clamp(0.0, 1023.0)) as u16).collect()
}

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

fn main() {
    let mut f = Box::new(Fft::new());
    let mut work: Box<fft::Work> = Box::new([0.0; fft::N]);

    // 1. Helpers.
    for &x in &[1e-6f32, 0.01, 0.5, 1.0, 1.5, 2.0, 10.0, 12345.0, 3.0e7] {
        let e = (fft::ln(x) - x.ln()).abs();
        check("ln", e < 3e-6 * x.ln().abs().max(1.0), format!("ln({x}) err {e}"));
    }
    for &y in &[-20.0f32, -3.3, -1.0, 0.0, 0.5, 1.0, 7.7, 30.0] {
        let e = (fft::exp2(y) / 2f32.powf(y) - 1.0).abs();
        check("exp2", e < 1e-6, format!("exp2({y}) rel err {e}"));
    }
    for (x, want) in [(52_083.0f32, 50_000.0f32), (100.0, 100.0), (99.0, 50.0), (0.5, 0.5), (0.49, 0.2), (2.5e6, 2e6), (1.0, 1.0)] {
        check("floor125", (fft::floor125(x) - want).abs() < want * 1e-4, format!("floor125({x}) = {}", fft::floor125(x)));
    }
    for (v, want) in [(50_000.0f32, 20_000.0f32), (20.0, 10.0), (10.0, 5.0), (1.0, 0.5), (0.5, 0.2)] {
        check("down125", (fft::down125(v) - want).abs() < want * 1e-4, format!("down125({v}) = {}", fft::down125(v)));
    }
    for i in 0..fft::N {
        let t = 2.0 * std::f64::consts::PI * i as f64 / fft::N as f64;
        let (es, ec) = ((fft::sin_i(i) as f64 - t.sin()).abs(), (fft::cos_i(i) as f64 - t.cos()).abs());
        if es > 1e-7 || ec > 1e-7 {
            check("table", false, format!("i {i}: sin err {es} cos err {ec}"));
            break;
        }
    }

    // 2. Against a direct DFT (rectangular window, a mixed signal), every size.
    for len in [256usize, 512, 1024, 2048] {
        let fs = 1.0e6;
        let src = codes(|t| 0.7 + 1.3 * (2.0 * std::f64::consts::PI * 12_345.0 * t).sin() + 0.4 * (2.0 * std::f64::consts::PI * 210_000.0 * t + 0.3).cos(), len, fs);
        assert!(f.load(&mut work, &src, 1, len, A, B, fs as f32, fft::W_RECT));
        f.finish(&mut work);
        check("bins", f.bins == len / 2 + 1, format!("{} bins", f.bins));
        let v: Vec<f64> = src.iter().map(|&c| (A + B * c as f32) as f64).collect();
        let mut worst = 0.0f64;
        for k in 0..=len / 2 {
            let (mut re, mut im) = (0.0, 0.0);
            for (n, x) in v.iter().enumerate() {
                let a = -2.0 * std::f64::consts::PI * (k * n) as f64 / len as f64;
                re += x * a.cos();
                im += x * a.sin();
            }
            let p = re * re + im * im;
            let s = if k == 0 { 1.0 } else { 2.0 };
            let want = 10.0 * (s * p / (len * len) as f64).log10();
            if want > -60.0 {
                worst = worst.max((f.db[k] as f64 - want).abs());
            }
        }
        check("dft", worst < 0.01, format!("len {len}: worst |dB err| {worst:.4} for bins above -60 dBV"));
    }

    // 3. dBV + peak for every window: 1 V amplitude (-3.01 dBV) sines on and
    // between bins (decimation k = 4, as the scope page at 4x the points).
    // (dB, bins) per window.
    let tol = [(0.15f32, 0.06f64), (0.6, 0.06), (0.5, 0.06), (0.6, 0.06), (0.6, 0.06), (0.1, 0.2)];
    for w in 0..6u8 {
        for &(frac, k) in &[(0.0f64, 1usize), (0.25, 1), (0.5, 1), (0.37, 4)] {
            let fs = 12.5e6f64;
            let n = fft::N;
            let bin = fs / (k * n) as f64;
            let fsig = bin * (100.0 + frac);
            let src = codes(|t| 1.0 * (2.0 * std::f64::consts::PI * fsig * t).sin(), n * k, fs);
            assert!(f.load(&mut work, &src, k, n, A, B, fs as f32, w));
            let mut steps = 0;
            while !f.step(&mut work) {
                steps += 1;
            }
            let (pf, pd) = f.peak(0.0, (fs / k as f64 / 2.0) as f32).expect("peak");
            // Boxcar decimation attenuates by sinc(pi f k / fs).
            let x = std::f64::consts::PI * fsig * k as f64 / fs;
            let box_db = if k > 1 { 20.0 * ((x.sin() / (k as f64 * (x / k as f64).sin())).abs()).log10() } else { 0.0 };
            let want = -3.0103 + box_db;
            let ferr = ((pf as f64 - fsig) / bin).abs();
            check(
                "peak",
                ferr < tol[w as usize].1 && (pd as f64 - want).abs() < tol[w as usize].0 as f64,
                format!("{} frac {frac} k {k}: f err {ferr:.3} bins, {pd:.2} dBV (want {want:.2} +- {:?})", fft::WIN_NAMES[w as usize], tol[w as usize]),
            );
            check("steps", steps == 4, format!("{} steps (10 stages / {} + post)", steps, fft::STAGES_PER_STEP));
            if frac == 0.25 {
                println!("{:>11}: peak {:.1} Hz (sig {:.1}), {:.3} dBV", fft::WIN_NAMES[w as usize], pf, fsig, pd);
            }
        }
    }

    // 4. Busy refuses a load; clear drops the result; col_max / db_at.
    let src = codes(|t| (2.0 * std::f64::consts::PI * 1000.0 * t).sin(), 4096, 1.0e6);
    assert!(f.load(&mut work, &src, 2, 2048, A, B, 1.0e6, fft::W_HANN));
    check("busy", !f.load(&mut work, &src, 1, 2048, A, B, 1.0e6, fft::W_HANN), "load while busy accepted".into());
    f.finish(&mut work);
    let pk = f.peak(0.0, 250_000.0).unwrap();
    check("decim peak", (pk.0 - 1000.0).abs() < 30.0, format!("{pk:?}"));
    let cm = f.col_max(900.0, 1100.0).unwrap();
    check("col_max", (cm - pk.1).abs() < 1.5, format!("col_max {cm} vs peak {}", pk.1));
    check("db_at", f.db_at(1e9).is_none() && f.db_at(0.0).is_some(), "db_at range".into());
    check("lin", (fft::db_to_v(-3.0103) - 0.70711).abs() < 1e-4, format!("{}", fft::db_to_v(-3.0103)));
    f.clear();
    check("clear", f.bins == 0 && f.peak(0.0, 1e9).is_none(), "result kept".into());

    let (n, bad) = unsafe { (CASES, FAILS) };
    println!("cases {n}, failures {bad}");
    std::process::exit(if bad == 0 { 0 } else { 1 });
}
