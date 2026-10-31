//! Host test of src/bargraph.rs (UF-30): the bar on the ILI9488 emulator of
//! tools/ui-render/host.rs. Random walks of values (unipolar and bipolar,
//! with overloads and sign changes): after every `update` the screen equals
//! a fresh `draw_static` + `update` of the same value; `frac` scale; the bus
//! words per update. Renders a few states for the contact sheet.
#![allow(dead_code, static_mut_refs)]

#[path = "../ui-render/host.rs"]
mod host;
pub use host::mmio;
#[path = "../../src/lcd.rs"]
mod lcd;
#[path = "../../src/bargraph.rs"]
mod bargraph;

use bargraph::{frac, full_scale, mode_full_scale, Bar};

const BG: u16 = 0x0883;
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

/// ui.rs BAR_RECT (the history row's place) and a wider-pitch variant.
const GEOM: [(u16, u16, u16, u16); 2] = [(12, 150, 400, 9), (4, 150, 450, 12)];

fn main() {
    let out = std::env::args().nth(1).unwrap_or_else(|| ".".into());
    host::map_flash();
    // full_scale: 60000 counts from the decimals; 1000 V / 10 A ranges.
    check("fs 6 V", full_scale(4, "V") == 6.0, String::new());
    check("fs 600 mV", (full_scale(2, "mV") - 600.0).abs() < 1e-3, String::new());
    check("fs 1000 V", full_scale(1, "V") == 1000.0, String::new());
    check("fs 10 A", full_scale(3, "A") == 10.0, String::new());
    check("fs 60 mA", (full_scale(3, "mA") - 60.0).abs() < 1e-4, String::new());
    // Continuity / diode: their OL ceilings, not the format's 600 ohm / 6 V.
    check("fs continuity 1000 ohm", mode_full_scale(11, 0, 2, "\u{3a9}") == 1000.0, String::new());
    check("fs diode 3 V", mode_full_scale(12, 0, 4, "V") == 3.0, String::new());
    check("fs DC V by mode", mode_full_scale(1, 0, 4, "V") == 6.0, String::new());
    // Capacitance: 6000 counts (dmm.rs cap_val): 100.0 nF on 600 nF,
    // 6.000 nF, 60.00 mF.
    check("fs cap 600 nF", (mode_full_scale(13, 2, 1, "nF") - 600.0).abs() < 1e-3, String::new());
    check("fs cap 6 nF", (mode_full_scale(13, 0, 3, "nF") - 6.0).abs() < 1e-5, String::new());
    check("fs cap 6 uF", (mode_full_scale(13, 3, 3, "\u{b5}F") - 6.0).abs() < 1e-5, String::new());
    check("fs cap 6000 uF", (mode_full_scale(13, 6, 0, "\u{b5}F") - 6000.0).abs() < 1e-2, String::new());
    check("fs cap 60000 uF", (mode_full_scale(13, 7, 0, "\u{b5}F") - 60000.0).abs() < 1e-1, String::new());
    // The experimental 600 Mohm: its range at any decimals.
    for d in 1..=3 {
        check("fs 600 Mohm", mode_full_scale(10, 7, d, "M\u{3a9}") == 600.0, String::new());
    }
    check("fs 6 Mohm by mode", (mode_full_scale(10, 5, 4, "M\u{3a9}") - 6.0).abs() < 1e-6, String::new());
    // frac: fraction of the full scale.
    check("frac 4.8176 V", (frac(4.8176, 6.0, false).unwrap() - 4.8176 / 6.0).abs() < 1e-6, String::new());
    check("frac -600 mV", (frac(-600.0, 600.0, false).unwrap() + 1.0).abs() < 1e-6, String::new());
    check("frac 500 V of 1000", (frac(500.0, full_scale(1, "V"), false).unwrap() - 0.5).abs() < 1e-6, String::new());
    check("frac clamps", frac(70000.0, 60000.0, false) == Some(1.0), String::new());
    check("frac OL", frac(1.0, 6.0, true).is_none() && frac(f32::NAN, 6.0, false).is_none(), String::new());
    // Segments: 0 / half / full scale light 0 / 25 / 50 (unipolar); the
    // bipolar scale lights from the centre.
    for (bip, f, want) in [(false, 0.0f32, 0usize), (false, 0.5, 25), (false, 1.0, 50), (true, 1.0, 25), (true, -0.5, 13), (true, 0.02, 1)] {
        host::reset(BG);
        let mut bar = Bar::new(12, 150, 400, 9, bip);
        bar.draw_static(BG);
        bar.update(Some(f));
        let lit = (0..bargraph::SEGS).filter(|&i| host::emu().fb[(158 * host::W) + bar.seg_x(i) as usize] == 0xFC40).count();
        check(&format!("segments bipolar {bip} f {f}"), lit == want, format!("{lit} lit, want {want}"));
    }

    let mut seed = 0x1234_5678u32;
    let mut rnd = move || {
        seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        (seed >> 8) as f32 / (1u32 << 24) as f32
    };
    for &(x, y, w, h) in &GEOM {
        for bipolar in [false, true] {
            host::reset(BG);
            let mut bar = Bar::new(x, y, w, h, bipolar);
            bar.draw_static(BG);
            let mut v: f32 = 0.3;
            let (mut words, mut n) = (0u64, 0u64);
            let mut bad = 0;
            for i in 0..600 {
                // Random walk, jumps, sign changes, overloads.
                v += (rnd() - 0.5) * if i % 50 == 0 { 1.5 } else { 0.05 };
                v = v.clamp(-1.2, 1.2);
                let f = if i % 97 == 13 || v.abs() > 1.1 { None } else { Some(v) };
                host::emu().bus = 0;
                bar.update(f);
                words += host::emu().bus;
                n += 1;
                let got = host::emu().fb.clone();
                host::reset(BG);
                let mut fresh = Bar::new(x, y, w, h, bipolar);
                fresh.draw_static(BG);
                fresh.update(f);
                let diff = (0..got.len()).filter(|&k| got[k] != host::emu().fb[k]).count();
                if diff > 0 && bad == 0 {
                    check(&format!("bar {w}x{h} bipolar {bipolar} step {i}"), false, format!("{diff} stale pixels (f {f:?})"));
                    bad += 1;
                }
                check("incremental == fresh", diff == 0, String::new());
                host::emu().fb.copy_from_slice(&got);
            }
            println!("bar {w}x{h} bipolar {bipolar}: {:.0} bus words / update", words as f64 / n as f64);
        }
    }
    // Renders: proposed history-row geometry, unipolar 0.80, bipolar -0.35, OL.
    for (name, bipolar, f) in [("b01-uni-080", false, Some(0.8)), ("b02-bi-neg035", true, Some(-0.35)), ("b03-ol", false, None)] {
        host::reset(BG);
        let (x, y, w, h) = GEOM[0];
        let _ = (x, y);
        let mut bar = Bar::new(x, y, w, h, bipolar);
        bar.draw_static(BG);
        bar.update(f);
        host::save_ppm(&format!("{out}/{name}.ppm"), 2);
    }
    let (f, c) = unsafe { (FAILS, CASES) };
    println!("cases {c}, failures {f}");
    std::process::exit(if f == 0 { 0 } else { 1 });
}
