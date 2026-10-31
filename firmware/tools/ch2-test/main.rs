//! Host test of src/scope_ch2.rs: a frame's column lookups
//! (Store::span_run, rising times, f32) against Store::span (f64, one
//! anchor search per column) on random streams: rates 1200 .. 38 400 S/s,
//! stores short of one anchor and wrapped many times, late reads (a stamp
//! two periods on), a lag, the DWT wrap; frames before the oldest sample,
//! across the store, after the newest one. A run of columns out of order
//! must give span's results too.
//!   tools/ch2-test/run.sh
#![allow(static_mut_refs)]

mod scope {
    #[derive(Clone, Copy, PartialEq, Debug)]
    pub struct WaveScale {
        pub a: f32,
        pub b: f32,
        pub vdiv: f32,
        pub amps: bool,
        pub ac: bool,
    }
}

mod clock {
    pub const HCLK_HZ: u32 = 200_000_000;
}

#[allow(dead_code)]
#[path = "../../src/scope_ch2.rs"]
mod scope_ch2;

use scope::WaveScale;

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u32 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        (self.0 >> 33) as u32
    }
    fn below(&mut self, n: u32) -> u32 {
        self.next() % n.max(1)
    }
}

fn main() {
    let mut r = Rng(0x5eed_c42);
    let (mut cols, mut some, mut edge, mut fails) = (0u64, 0u64, 0u64, 0u64);
    scope_ch2::init();
    for case in 0..600 {
        let fs = [1200u32, 2400, 4800, 9600, 38_400][r.below(5) as usize];
        let c0 = if case % 3 == 0 { u32::MAX - r.below(50_000_000) } else { r.next() };
        scope_ch2::set_lag_us(r.below(400) as i32 - 100);
        let st = scope_ch2::store();
        let sc = WaveScale { a: 0.0, b: 1e-6, vdiv: 1.0, amps: false, ac: false };
        st.reset(c0, fs, sc, 0);
        let n = [r.below(127) + 1, r.below(4096), 4096 + r.below(20_000)][r.below(3) as usize];
        let per_us = 1e6 / fs as f64;
        let mut k = 0u64;
        for _ in 0..n {
            // A late read now and then: that period's sample is missing.
            if r.below(500) == 0 {
                k += 1;
            }
            let ts = (k as f64 * per_us + r.below(3) as f64) as u32;
            st.push(ts, (r.below(1 << 19) as i32) - (1 << 18));
            k += 1;
        }
        let period = clock::HCLK_HZ as f64 / fs as f64;
        let end = c0.wrapping_add((k as f64 * per_us * 200.0) as u32);
        let store_cyc = (n.min(4096) as f64 * period) as u32;
        for frame in 0..8 {
            // Column length: < 1 sample .. ~50 samples.
            let len = (period * [0.3, 1.0, 2.7, 13.0, 50.0][r.below(5) as usize]) as f32;
            let span_cyc = (len as f64 * 400.0) as u32;
            let back = r.below(store_cyc.saturating_add(span_cyc).saturating_add(span_cyc / 2).max(1));
            let start = end.wrapping_sub(back).wrapping_add(r.below(span_cyc / 4 + 1));
            let mut run = st.run();
            let order: Vec<usize> = if frame == 7 { (0..400).map(|i| (i * 37) % 400).collect() } else { (0..400).collect() };
            for i in order {
                let t0 = start.wrapping_add((i as f64 * len as f64) as u32);
                let a = st.range(t0, len);
                let b = st.range_run(&mut run, t0, len);
                cols += 1;
                let ok = match (a, b) {
                    (None, None) => true,
                    (Some(x), Some(y)) => {
                        some += 1;
                        let d = (x.0 as i64 - y.0 as i64).abs().max((x.1 as i64 - y.1 as i64).abs());
                        if d > 0 {
                            edge += 1;
                        }
                        d <= 1
                    }
                    // A rounding edge at the store's ends.
                    (Some(x), None) | (None, Some(x)) => {
                        edge += 1;
                        x.1 - x.0 == 1
                    }
                };
                if !ok {
                    fails += 1;
                    if fails <= 10 {
                        println!("FAIL case {case} frame {frame} col {i}: fs {fs} n {n} len {len} span {a:?} run {b:?}");
                    }
                }
            }
        }
    }
    let rate = edge as f64 / some.max(1) as f64;
    println!("ch2-test: {cols} columns, {some} with samples, {edge} at a rounding edge ({:.4} %), {fails} failed", rate * 100.0);
    if fails > 0 || rate > 0.01 || some < cols / 4 {
        println!("FAIL");
        std::process::exit(1);
    }
    println!("PASS");
}
