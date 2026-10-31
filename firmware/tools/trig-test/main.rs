//! Host test of src/trigsoft.rs (UF-13 software pulse-width / runt
//! triggers) on synthetic 10-bit records. Needs no hardware.
//!   tools/trig-test/run.sh
#[allow(dead_code)]
#[path = "../../src/pack10.rs"]
mod pack10;
#[path = "../../src/trigsoft.rs"]
mod trigsoft;
use trigsoft::*;

static mut BAD: u32 = 0;
static mut N: u32 = 0;
fn t(name: &str, ok: bool) {
    unsafe {
        N += 1;
        if !ok {
            BAD += 1;
            println!("FAIL {name}");
        }
    }
}

/// Pulses (start, width, high code) on a base code with +-1 code noise.
fn rec(n: usize, base: u16, pulses: &[(usize, usize, u16)]) -> Vec<u16> {
    let mut d: Vec<u16> = (0..n).map(|i| base + (i % 3) as u16).collect();
    for &(s, w, h) in pulses {
        for v in &mut d[s..s + w] {
            *v = h;
        }
    }
    d
}

fn main() {
    // Pulses of 10, 50, 100 samples, high (800) on 200.
    let d = rec(1000, 200, &[(100, 10, 800), (300, 50, 800), (600, 100, 800)]);
    t("> 40 -> the 50-sample pulse ends at 350", pulse(&d, 0, 1000, 500, true, GT, 40.0) == Some(350));
    t("> 60 -> the 100-sample pulse", pulse(&d, 0, 1000, 500, true, GT, 60.0) == Some(700));
    t("< 20 -> the 10-sample pulse", pulse(&d, 0, 1000, 500, true, LT, 20.0) == Some(110));
    t("= 50 -> 350", pulse(&d, 0, 1000, 500, true, EQ, 50.0) == Some(350));
    t("= 51 (within 5 %) -> 350", pulse(&d, 0, 1000, 500, true, EQ, 51.0) == Some(350));
    t("= 70 -> none", pulse(&d, 0, 1000, 500, true, EQ, 70.0).is_none());
    t("> 200 -> none", pulse(&d, 0, 1000, 500, true, GT, 200.0).is_none());
    t("lo bound: > 5 from 200 -> 350", pulse(&d, 200, 1000, 500, true, GT, 5.0) == Some(350));
    t("hi bound: trailing edge 700 excluded", pulse(&d, 0, 700, 500, true, GT, 60.0).is_none());
    // Negative pulses: low (100) on a high base (800).
    let n = rec(1000, 800, &[(100, 30, 100), (400, 80, 100)]);
    t("negative > 50 -> 480", pulse(&n, 0, 1000, 500, false, GT, 50.0) == Some(480));
    t("negative < 50 -> 130", pulse(&n, 0, 1000, 500, false, LT, 50.0) == Some(130));
    t("positive search on low pulses: the gaps (>= 270 samples) only", pulse(&n, 0, 1000, 500, true, LT, 100.0).is_none());
    // A pulse already high at the record start is not measured (no leading edge).
    let s = rec(500, 200, &[(0, 40, 800), (200, 40, 800)]);
    t("no leading edge at the start", pulse(&s, 0, 500, 500, true, GT, 30.0) == Some(240));
    // Runt: full pulses to 800, a runt to 500 (levels 300 / 700).
    let r = rec(1200, 200, &[(100, 50, 800), (400, 50, 500), (700, 50, 800)]);
    t("runt + -> 450", runt(&r, 0, 1200, 300, 700, true) == Some(450));
    t("runt +, levels swapped", runt(&r, 0, 1200, 700, 300, true) == Some(450));
    t("runt + none when the runt reaches the upper level", runt(&r, 0, 1200, 300, 480, true).is_none());
    t("runt + lo bound past it", runt(&r, 460, 1200, 300, 700, true).is_none());
    let rn = rec(1200, 900, &[(100, 50, 100), (400, 50, 450)]);
    t("runt - -> 450", runt(&rn, 0, 1200, 300, 700, false) == Some(450));
    t("runt levels too close", runt(&r, 0, 1200, 300, 303, true).is_none());
    // Width steps.
    t("step up 1us -> 2us", (width_step(1e-6, true) - 2e-6).abs() < 1e-12);
    t("step down 1us -> 500ns", (width_step(1e-6, false) - 5e-7).abs() < 1e-13);
    t("step clamps", width_step(10.0, true) == W_MAX && (width_step(20e-9, false) - W_MIN).abs() < 1e-15);
    t("step from off-grid 3us up -> 5us", (width_step(3e-6, true) - 5e-6).abs() < 1e-12);
    // Streaming detector (Stream, the DMM wave view's ISR trigger): the
    // first event at or after lo equals the slice functions'.
    let first = |d: &[u16], c: StreamCfg, lo: usize| -> Option<usize> {
        let mut st = Stream::NEW;
        for (i, &v) in d.iter().enumerate() {
            if st.step(&c, i as u32, v as i32) && i >= lo {
                return Some(i);
            }
        }
        None
    };
    let pc = |cond: u8, w: f32, rising: bool| {
        let (wlo, whi) = pulse_window(cond, w);
        StreamCfg { ttype: PULSE, rising, l: 500, l2: 0, wlo, whi }
    };
    for (cond, w) in [(GT, 40.0f32), (GT, 60.0), (LT, 20.0), (EQ, 50.0), (EQ, 49.0), (GT, 50.0), (LT, 50.0), (LT, 10.5), (EQ, 10.0)] {
        for lo in [0usize, 120, 360, 900] {
            t(&format!("stream pulse {cond} {w} lo {lo}"), first(&d, pc(cond, w, true), lo) == pulse(&d, lo, 1000, 500, true, cond, w));
        }
    }
    t("stream pulse no leading edge", first(&s, pc(GT, 30.0, true), 0) == Some(240));
    let rc = |l: i32, l2: i32, rising: bool| StreamCfg { ttype: RUNT, rising, l, l2, wlo: 0, whi: 0 };
    for lo in [0usize, 200, 460, 800] {
        t(&format!("stream runt + lo {lo}"), first(&r, rc(300, 700, true), lo) == runt(&r, lo, 1200, 300, 700, true));
        t(&format!("stream runt - lo {lo}"), first(&rn, rc(300, 700, false), lo) == runt(&rn, lo, 1200, 300, 700, false));
    }
    t("stream runt reaches upper", first(&r, rc(300, 480, true), 0).is_none());
    t("stream runt too close", first(&r, rc(300, 303, true), 0).is_none());
    // Edge: armed beyond the +-2 band, then across the level; re-arms
    // after each hit (one event per crossing, not one per sample above).
    let sq = rec(1000, 200, &[(100, 50, 800), (300, 50, 800)]);
    let ec = |rising: bool| StreamCfg { ttype: EDGE, rising, l: 500, l2: 0, wlo: 0, whi: 0 };
    let hits = |c: StreamCfg| {
        let mut st = Stream::NEW;
        (0..sq.len()).filter(|&i| st.step(&c, i as u32, sq[i] as i32)).collect::<Vec<_>>()
    };
    t("stream edge rising: one hit per edge", hits(ec(true)) == vec![100, 300]);
    t("stream edge falling", hits(ec(false)) == vec![150, 350]);
    let mut st = Stream::NEW;
    let noisy: Vec<u16> = (0..200).map(|i| if i < 50 { 400 } else { 500 + (i % 2) as u16 * 3 }).collect();
    let n = (0..noisy.len()).filter(|&i| st.step(&ec(true), i as u32, noisy[i] as i32)).count();
    t("stream edge: noise inside the band does not re-trigger", n == 1);
    t("pulse window GT 40 -> 41..", pulse_window(GT, 40.0) == (41, u32::MAX));
    t("pulse window LT 20 -> ..19", pulse_window(LT, 20.0) == (0, 19));
    t("pulse window LT 0.5 -> ..0", pulse_window(LT, 0.5) == (0, 0));
    t("pulse window EQ 50 -> 48..52", pulse_window(EQ, 50.0) == (48, 52));
    t("pulse window EQ 10 -> 9..11", pulse_window(EQ, 10.0) == (9, 11));
    packed();
    let (n, bad) = unsafe { (N, BAD) };
    println!("cases {n}, failures {bad}");
    std::process::exit(if bad == 0 { 0 } else { 1 });
}

/// src/pack10.rs (the scope record's 4-in-5 packing): in-place packing of
/// a raw ring (bits 10..15 masked), the wrapping view, set, and the
/// triggers on a packed ring = on the plain codes.
fn packed() {
    use pack10::{Codes, Src};
    let n = 4000;
    let d = rec(n, 200, &[(1000, 10, 800), (3000, 50, 800)]);
    let mut raw: Vec<u16> = d.iter().enumerate().map(|(i, &v)| v | ((i as u16 * 7) << 10)).collect();
    raw.extend_from_slice(&[0, 0]);
    let p = raw.as_mut_ptr() as *mut u8;
    pack10::pack_in_place(p, n);
    t("pack10: every code back after the in-place pack", (0..n).all(|i| pack10::get(p, i) == d[i]));
    // The ring rotated by 1234: view from slot 2766 wraps to the start.
    let off = n - 1234;
    let mut rot = d[off..].to_vec();
    rot.extend_from_slice(&d[..off]);
    let c = Codes::new(p, off, n, n);
    t("pack10: wrapping view = the rotated codes", (0..n).all(|i| c.at(i) == rot[i]));
    t("pack10: sub view", c.sub(1000, 1300).at(299) == rot[1299] && c.sub(1000, 1300).len() == 300);
    t("pack10: min / max", c.min_max(0, n) == (200, 800));
    t("pack10: pulse on the packed ring = on the codes", pulse(&c, 0, n, 500, true, GT, 40.0) == pulse(&rot, 0, n, 500, true, GT, 40.0) && pulse(&c, 0, n, 500, true, GT, 40.0).is_some());
    t("pack10: runt on the packed ring = on the codes", runt(&c, 0, n, 300, 900, true) == runt(&rot, 0, n, 300, 900, true));
    t("pack10: offset_of", c.offset_of(&c.sub(77, 500)) == Some(77) && c.sub(10, 20).offset_of(&c) .is_none());
    for (i, v) in [(5usize, 1023u16), (6, 0), (7, 512), (3999, 1)] {
        pack10::set(p, i, v);
    }
    t("pack10: set keeps the neighbours", pack10::get(p, 4) == d[4] && pack10::get(p, 5) == 1023 && pack10::get(p, 6) == 0 && pack10::get(p, 7) == 512 && pack10::get(p, 8) == d[8] && pack10::get(p, 3999) == 1 && pack10::get(p, 3998) == d[3998]);
}
