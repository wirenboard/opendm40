// Host test of src/dlog.rs (compact binary DMM logs, docs/compact-log-format.md):
//   1. round trip: rows of several synthetic data sets (noisy DC V, steady,
//      ramp, AC V, Hz, OL bursts, decade sweeps, special values) coded in
//      chunks of varying size; the CSV generated from the DLG file must equal,
//      byte for byte, what the pre-DLG csvlog.rs writer printed for the same
//      push() calls (`old_*` below: its W::num / push, verbatim);
//   2. random access: Gen::read at random offsets / lengths and in 512-byte
//      sectors vs the reference;
//   3. power cut: every prefix of the file decodes to exactly the CSV of the
//      whole chunks it contains; a flipped bit drops its chunk and the rest;
//   4. size: bytes per row per data set (printed).
// Writes the DLG + reference CSV of every set to $1 (if given) for tools/dlog.py.
#![allow(dead_code)]

#[path = "../../src/pack10.rs"]
mod pack10;
#[path = "../../src/dlog.rs"]
mod dlog;
#[path = "../../src/fastenc.rs"]
mod fastenc;
mod fast;

use dlog::*;

// ------------------------------------------------- the pre-DLG writer

struct W<'a> {
    b: &'a mut Vec<u8>,
}
impl W<'_> {
    fn c(&mut self, c: u8) {
        self.b.push(c)
    }
    fn s(&mut self, s: &str) {
        self.b.extend_from_slice(s.as_bytes())
    }
    fn bytes(&mut self, s: &[u8]) {
        self.b.extend_from_slice(s)
    }
    fn u(&mut self, v: u64) {
        self.s(&v.to_string())
    }
    fn ms(&mut self, ms: u64) {
        self.u(ms / 1000);
        self.c(b'.');
        let r = (ms % 1000) as u32;
        self.c(b'0' + (r / 100) as u8);
        self.c(b'0' + (r / 10 % 10) as u8);
        self.c(b'0' + (r % 10) as u8);
    }
    fn num(&mut self, v: f32) {
        if v < 0.0 {
            self.c(b'-');
        }
        let x = if v < 0.0 { -v } else { v };
        if x < 1e-30 {
            self.c(b'0');
            return;
        }
        let mut e = P10_MIN;
        while e < P10_MAX && p10(e + 1) <= x {
            e += 1;
        }
        let f = if e - 5 >= P10_MIN { x / p10(e - 5) } else { x / p10(e) * 1e5 };
        let mut m = (f + 0.5) as u32;
        if m >= 1_000_000 {
            m /= 10;
            e += 1;
        }
        let m = m.clamp(100_000, 999_999);
        let mut dg = [0u8; 6];
        let mut t = m;
        for i in (0..6).rev() {
            dg[i] = b'0' + (t % 10) as u8;
            t /= 10;
        }
        if (-3..6).contains(&e) {
            if e < 0 {
                self.s("0.");
                for _ in 0..(-e - 1) {
                    self.c(b'0');
                }
                self.bytes(&dg);
            } else {
                let ip = (e + 1) as usize;
                self.bytes(&dg[..ip]);
                if ip < 6 {
                    self.c(b'.');
                    self.bytes(&dg[ip..]);
                }
            }
        } else {
            self.c(dg[0]);
            self.c(b'.');
            self.bytes(&dg[1..]);
            self.c(b'E');
            self.c(if e < 0 { b'-' } else { b'+' });
            let a = e.unsigned_abs();
            if a < 10 {
                self.c(b'0');
            }
            self.u(a as u64);
        }
    }
}
const P10_MIN: i32 = -30;
const P10_MAX: i32 = 30;
fn p10(e: i32) -> f32 {
    format!("1e{}", e.clamp(P10_MIN, P10_MAX)).parse().unwrap()
}

/// The old push(): one row (gap = the old j.gap).
fn old_row(out: &mut Vec<u8>, t: u64, value: f32, flags: u8, unit: &[u8], gap: bool) {
    let mut w = W { b: out };
    w.ms(t);
    w.c(b',');
    if flags & 1 == 0 && value.is_finite() {
        w.num(value);
    }
    w.c(b',');
    w.bytes(unit);
    w.c(b',');
    let mut sep = false;
    for (bit, t) in [(1u8, "OL"), (2, "REL"), (4, "HOLD"), (8, "HI"), (16, "LO")] {
        if flags & bit != 0 {
            if sep {
                w.c(b' ');
            }
            w.s(t);
            sep = true;
        }
    }
    if gap {
        if sep {
            w.c(b' ');
        }
        w.s("GAP");
    }
    w.s("\r\n");
}

// ------------------------------------------------- data

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 16) as u32
    }
    fn uni(&mut self) -> f32 {
        self.next() as f32 / u32::MAX as f32 * 65536.0 % 1.0
    }
    fn gauss(&mut self) -> f32 {
        let mut s = 0.0;
        for _ in 0..12 {
            s += (self.next() % 65536) as f32 / 65536.0;
        }
        s - 6.0
    }
    fn jit(&mut self, j: u32) -> i64 {
        (self.next() % (2 * j + 1)) as i64 - j as i64
    }
}

/// (t ms, value, flags incl. GAP) rows.
type Rows = Vec<(u64, f32, u8)>;

fn set(i: usize, n: usize) -> (&'static str, &'static [u8], Rows) {
    let mut r = Rng(0x9E37_79B9_7F4A_7C15 ^ (i as u64 * 77));
    let mut t = 0i64;
    let mut v = Vec::with_capacity(n);
    let mut per = |r: &mut Rng, p: i64, j: u32| {
        let x = t;
        t += p + r.jit(j);
        x as u64
    };
    match i {
        0 => {
            for _ in 0..n {
                let tt = per(&mut r, 100, 3);
                v.push((tt, 1.234567 + r.gauss() * 30e-6, 0));
            }
            ("noisy DC V 10/s", b"V", v)
        }
        1 => {
            for _ in 0..n {
                let tt = per(&mut r, 400, 3);
                let u = r.uni();
                v.push((tt, 10001.2 + if u < 0.05 { 0.1 } else if u < 0.1 { -0.1 } else { 0.0 }, 0));
            }
            ("steady ohm 2.5/s", b"Ohm", v)
        }
        2 => {
            for k in 0..n {
                let tt = per(&mut r, 400, 3);
                v.push((tt, 4.2 - 1.2 * k as f32 / n as f32 + r.gauss() * 50e-6, 0));
            }
            ("ramp 2.5/s", b"V", v)
        }
        3 => {
            let mut x = 230.0f32;
            for _ in 0..n {
                let tt = per(&mut r, 100, 3);
                x += r.gauss() * 0.02 - (x - 230.0) * 0.001;
                v.push((tt, x + r.gauss() * 0.01, 0));
            }
            ("AC V 230 10/s", b"V", v)
        }
        4 => {
            let mut f = 50.0f32;
            for _ in 0..n {
                let tt = per(&mut r, 333, 5);
                f += r.gauss() * 0.0005 - (f - 50.0) * 0.01;
                v.push((tt, f, 0));
            }
            ("Hz 3/s", b"Hz", v)
        }
        5 => {
            let mut ol = false;
            for _ in 0..n {
                let tt = per(&mut r, 100, 3);
                if r.uni() < 0.01 {
                    ol = !ol;
                }
                v.push((tt, if ol { f32::INFINITY } else { 99.87 + r.gauss() * 0.02 }, if ol { 1 } else { 0 }));
            }
            ("ohm + OL bursts", b"Ohm", v)
        }
        6 => {
            // MEDIUM: 10/s moving, 2.5/s steady; autorange levels incl. a
            // noisy decade boundary at 1.0 and REL / HOLD / HI / LO / GAP.
            let lv = [0.5f32, 0.0123, 12.5, 1.0, 0.000999, 3.3e-7, 250000.0];
            for k in 0..n {
                let seg = (k / 300) % lv.len();
                let moving = k % 300 < 20;
                let tt = per(&mut r, if moving { 100 } else { 400 }, 3);
                let mut f = 0u8;
                if k % 500 > 450 {
                    f |= 2;
                }
                if k % 700 > 690 {
                    f |= 4;
                }
                if k % 900 == 5 {
                    f |= 8;
                }
                if k % 901 == 7 {
                    f |= 16;
                }
                if k % 997 == 13 {
                    f |= F_GAP;
                }
                v.push((tt, lv[seg] + r.gauss() * lv[seg] * 2e-5, f));
            }
            ("MEDIUM + autorange + flags", b"V", v)
        }
        7 => {
            // Every decade and sign, the fatren-test row() pattern.
            const P: [f32; 13] = [1e-9, 1e-8, 1e-7, 1e-6, 1e-5, 1e-4, 1e-3, 1e-2, 1e-1, 1e0, 1e1, 1e2, 1e3];
            for k in 0..n as u32 {
                let tt = per(&mut r, 100, 0);
                let m = ((k * 7919) % 20001) as f32 - 10000.0;
                let mut f = 0u8;
                if k % 97 == 3 {
                    f |= 1;
                }
                if k % 5 == 0 {
                    f |= 2;
                }
                v.push((tt, m * P[(k % 13) as usize], f));
            }
            ("decade sweep", b"A", v)
        }
        _ => {
            // Special values.
            let sp = [
                0.0f32,
                -0.0,
                1e-31,
                -1e-31,
                1e-30,
                -1e-30,
                f32::MIN_POSITIVE,
                1e-45,
                0.0009999995,
                0.001,
                0.00099999,
                999.9995,
                999999.4,
                999999.6,
                1e6,
                -1e6,
                1e30,
                3e38,
                f32::MAX,
                f32::MIN,
                f32::NAN,
                f32::INFINITY,
                f32::NEG_INFINITY,
                1.0,
                0.99999,
                1.00001,
                0.999995,
                -0.999995,
                123456.7,
                -5e-4,
                7e-29,
            ];
            for k in 0..n {
                let tt = per(&mut r, if k % 50 == 0 { 100_000 } else { 7 }, 7);
                v.push((tt, sp[k % sp.len()] * if k % 61 == 0 { 3.3 } else { 1.0 }, if k % 41 == 1 { 1 } else { 0 }));
            }
            ("special values, gaps in time", b"deg", v)
        }
    }
}

const HDR: &[u8] = b"# Alientek DM40A, opendm40 0.0.1, function DC V, range AUTO, started uptime 12.345 s\r\n# time_s = seconds since the start of this recording (no real-time clock); empty value = overflow\r\ntime_s,value,unit,flags\r\n";

/// Reference CSV (old writer), and the CSV offset after each row.
fn reference(rows: &Rows, unit: &[u8]) -> (Vec<u8>, Vec<usize>) {
    let mut out = HDR.to_vec();
    let mut ends = vec![];
    for &(t, v, f) in rows {
        old_row(&mut out, t, v, f & 31, unit, f & F_GAP != 0);
        ends.push(out.len());
    }
    (out, ends)
}

/// DLG file as csvlog writes it: chunks of `chunk(i)` rows. Returns the
/// file and the file offset after each chunk with its row count so far.
fn encode(rows: &Rows, unit: &[u8], chunk: &mut dyn FnMut() -> usize) -> (Vec<u8>, Vec<(usize, usize)>) {
    let mut f = vec![0u8; FHDR + HDR.len()];
    file_header(KIND_DMM, unit, HDR, &mut f);
    let mut e = Enc::new(HDR.len());
    let mut i = 0;
    let mut cuts = vec![];
    while i < rows.len() {
        let n = chunk().max(1).min(rows.len() - i);
        let h0 = f.len();
        f.extend_from_slice(&[0u8; CHDR]);
        for &(t, v, fl) in &rows[i..i + n] {
            let mut txt = [0u8; ROW_MAX];
            let val = dec6(v);
            let len = fmt_row(t, val, unit, fl, &mut txt);
            let mut b = [0u8; ROW_BYTES];
            let k = e.row(t as u32, val, fl, len as u32, &mut b);
            f.extend_from_slice(&b[..k]);
        }
        let mut b = [0u8; 1];
        let k = e.close(&mut b);
        f.extend_from_slice(&b[..k]);
        let mut h = e.header();
        let c = !crc32_upd(crc32_upd(!0, &h[8..]), &f[h0 + CHDR..]);
        h[4..8].copy_from_slice(&c.to_le_bytes());
        f[h0..h0 + CHDR].copy_from_slice(&h);
        assert_eq!(f.len() - h0 - CHDR, e.pl as usize);
        i += n;
        cuts.push((f.len(), i));
    }
    (f, cuts)
}

fn gen_all(f: &[u8]) -> Option<(Info, Vec<u8>)> {
    let mut g = Gen::open(f, f.len() as u32)?;
    let mut out = vec![0u8; g.info.csv as usize];
    // In 512-byte sectors, as LUN 1 reads it.
    for (k, s) in out.chunks_mut(512).enumerate() {
        g.read(k as u32 * 512, s);
    }
    Some((g.info, out))
}

// ------------------------------------------------- scope records

const SN: usize = 26624;

/// Synthetic 10-bit records (as tools/experiments/logcodec-lab/scope.py).
fn scope_records() -> Vec<(&'static str, Vec<u16>)> {
    let mut r = Rng(7);
    let mut g = |s: f32| r.gauss() * s;
    let cl = |x: f32| (x.round().clamp(0.0, 1023.0)) as u16;
    let pi2 = 2.0 * std::f32::consts::PI;
    let n = SN as f32;
    let mut v = vec![];
    v.push(("DC + 1 LSB noise", (0..SN).map(|_| cl(612.0 + g(1.0))).collect()));
    v.push(("sine 13 periods, 0.7 LSB noise", (0..SN).map(|i| cl(512.0 + 400.0 * (pi2 * 13.0 * i as f32 / n).sin() + g(0.7))).collect()));
    v.push(("square 10 periods", (0..SN).map(|i| cl(if (i * 20 / SN) % 2 == 1 { 812.0 } else { 212.0 } + g(0.8))).collect()));
    v.push((
        "mains + harmonics, 2 LSB noise",
        (0..SN)
            .map(|i| {
                let t = i as f32 / n;
                cl(512.0 + 300.0 * (pi2 * 40.0 * t).sin() + 40.0 * (pi2 * 120.0 * t + 1.0).sin() + 15.0 * (pi2 * 200.0 * t).sin() + g(2.0))
            })
            .collect(),
    ));
    v.push(("sine 400 periods (fast)", (0..SN).map(|i| cl(512.0 + 450.0 * (pi2 * 400.0 * i as f32 / n).sin() + g(0.7))).collect()));
    v.push(("noise burst on DC", (0..SN).map(|i| cl(512.0 + if (10000..14000).contains(&i) { g(60.0) } else { g(0.8) })).collect()));
    v.push(("white noise, full scale (raw blocks)", (0..SN).map(|_| (r.next() % 1024) as u16).collect()));
    v.push(("300 samples", (0..300).map(|i| cl(500.0 + 3.0 * (i as f32 * 0.1).sin())).collect()));
    v
}

/// The DLG file of a record, as scope_csv.rs writes it.
fn scope_file(codes: &[u16], text: &[u8]) -> (Vec<u8>, Vec<usize>) {
    let (a, b, dt, t0) = (-5.0f32, 10.0 / 1023.0, 1.0 / 12.5e6, codes.len() as u32 / 2);
    let mut h = vec![0u8; FHDR + text.len() + SCOPE_PAR + 8 * SBLK_MAX + 4];
    let k = scope_header(text, a, b, dt, t0, codes.len() as u32, &mut h);
    let plans: Vec<_> = codes.chunks(SBLK).map(scope_plan).collect();
    let lens: Vec<u32> = plans.iter().map(|p| p.1).collect();
    let crcs: Vec<u32> = plans.iter().map(|p| p.2).collect();
    let tl = scope_table(&lens, &crcs, &mut h[k..]);
    h.truncate(k + tl);
    let mut ends = vec![];
    for (c, p) in codes.chunks(SBLK).zip(&plans) {
        let n0 = h.len();
        scope_block(c, p.0, &mut |x| h.push(x));
        assert_eq!(h.len() - n0, p.1 as usize, "plan size");
        ends.push(h.len());
    }
    (h, ends)
}

fn scope_tests(dir: Option<String>) -> u32 {
    let mut fails = 0;
    let text = b"# test record\r\ntime_s,volts\r\n";
    println!("{:<40} {:>8} {:>8} {:>7}", "scope record", "u16 B/s", "DLG B/s", "vs u16");
    for (si, (name, codes)) in scope_records().into_iter().enumerate() {
        let (f, ends) = scope_file(&codes, text);
        let (a, b, dt, t0) = (-5.0f32, 10.0 / 1023.0, 1.0 / 12.5e6, codes.len() as u32 / 2);
        let mut csv = text.to_vec();
        for (i, &c) in codes.iter().enumerate() {
            let mut r = [0u8; SCOPE_ROW];
            scope_row(a, b, dt, t0, i, c, &mut r);
            csv.extend_from_slice(&r);
        }
        match gen_all(&f) {
            Some((info, out)) if out == csv && info.rows as usize == codes.len() => {}
            _ => {
                println!("FAIL {name}: CSV differs");
                fails += 1;
                continue;
            }
        }
        // Random access.
        let mut g = Gen::open(&f[..], f.len() as u32).unwrap();
        let mut rr = Rng(3 + si as u64);
        for _ in 0..400 {
            let off = rr.next() as usize % (csv.len() + 10);
            let len = 1 + rr.next() as usize % 900;
            let mut b = vec![0xAAu8; len];
            g.read(off as u32, &mut b);
            let want: Vec<u8> = (off..off + len).map(|i| csv.get(i).copied().unwrap_or(0)).collect();
            if b != want {
                println!("FAIL {name}: random read at {off}+{len}");
                fails += 1;
                break;
            }
        }
        // Torn file: exactly the whole blocks before the cut.
        // Header, parameters and table: a cut inside them = not a DLG file.
        let head = FHDR + text.len() + SCOPE_PAR + 8 * ends.len() + 4;
        let mut bad = 0;
        let mut cuts: Vec<usize> = (0..=f.len()).step_by(97).collect();
        for &e in &ends {
            cuts.extend([e - 1, e, e + 1]);
        }
        for l in cuts.into_iter().filter(|&l| l <= f.len()) {
            let whole = ends.iter().filter(|&&e| e <= l).count();
            let rows = (whole * SBLK).min(codes.len());
            let want = &csv[..text.len() + rows * SCOPE_ROW];
            match gen_all(&f[..l]) {
                Some((_, o)) if o == want => {}
                None if l < head => {}
                _ => bad += 1,
            }
        }
        if bad > 0 {
            println!("FAIL {name}: {bad} truncations decode wrong");
            fails += 1;
        }
        let n = codes.len() as f64;
        let data = (f.len() - (FHDR + text.len() + SCOPE_PAR + 8 * ends.len() + 4)) as f64;
        println!("{:<40} {:>8.3} {:>8.3} {:>6.1}x  ok", name, 2.0, data / n, 2.0 * n / data);
        if let Some(d) = &dir {
            std::fs::write(format!("{d}/s{si}.dlg"), &f).unwrap();
            std::fs::write(format!("{d}/s{si}.csv"), &csv).unwrap();
        }
    }
    fails
}

fn main() {
    let mut fails = scope_tests(std::env::args().nth(1));
    fails += fast::tests(std::env::args().nth(1));
    let n = 6000;
    println!("{:<30} {:>6} {:>8} {:>8} {:>7}", "set", "rows", "CSV B/r", "DLG B/r", "ratio");
    for si in 0..9 {
        let (name, unit, rows) = set(si, n);
        let (csv, ends) = reference(&rows, unit);
        let mut r = Rng(si as u64 + 5);
        // Chunks like csvlog commits: 1..1200 rows.
        let mut ch = || match r.next() % 4 {
            0 => 1 + (r.next() % 5) as usize,
            1 => 600,
            _ => 1 + (r.next() % 1200) as usize,
        };
        let (dlg, cuts) = encode(&rows, unit, &mut ch);
        // 1. round trip
        let (info, out) = gen_all(&dlg).expect("open");
        if out != csv {
            let p = out.iter().zip(&csv).position(|(a, b)| a != b).unwrap_or(out.len().min(csv.len()));
            let lo = p.saturating_sub(60);
            println!(
                "FAIL {name}: CSV differs at {p} (len {} vs {}):\n got {:?}\n ref {:?}",
                out.len(),
                csv.len(),
                String::from_utf8_lossy(&out[lo..(p + 40).min(out.len())]),
                String::from_utf8_lossy(&csv[lo..(p + 40).min(csv.len())])
            );
            fails += 1;
            continue;
        }
        assert_eq!(info.rows as usize, rows.len());
        assert_eq!(info.t_last as u64, rows.last().unwrap().0);
        // 2. random access
        let mut g = Gen::open(&dlg[..], dlg.len() as u32).unwrap();
        let mut rr = Rng(99 + si as u64);
        for _ in 0..3000 {
            let off = rr.next() as usize % (csv.len() + 10);
            let len = 1 + rr.next() as usize % 700;
            let mut b = vec![0xAAu8; len];
            g.read(off as u32, &mut b);
            let want: Vec<u8> = (off..off + len).map(|i| csv.get(i).copied().unwrap_or(0)).collect();
            if b != want {
                println!("FAIL {name}: random read at {off}+{len}");
                fails += 1;
                break;
            }
        }
        // 3. power cut at every byte: exactly the whole chunks survive.
        let step = if dlg.len() > 20000 { 7 } else { 1 };
        let mut bad = 0;
        let mut l = 0;
        while l <= dlg.len() {
            let whole = cuts.iter().filter(|c| c.0 <= l).map(|c| c.1).last().unwrap_or(0);
            let want: &[u8] = if whole == 0 { HDR } else { &csv[..ends[whole - 1]] };
            match gen_all(&dlg[..l]) {
                Some((_, o)) if o == want => {}
                None if l < FHDR + HDR.len() => {}
                _ => bad += 1,
            }
            l += step;
        }
        // Whole-chunk boundaries exactly (the stepped scan may skip them).
        for &(c, _) in &cuts {
            for l in [c - 1, c, c + 1] {
                if l <= dlg.len() {
                    let whole = cuts.iter().filter(|x| x.0 <= l).map(|x| x.1).last().unwrap_or(0);
                    let want: &[u8] = if whole == 0 { HDR } else { &csv[..ends[whole - 1]] };
                    if gen_all(&dlg[..l]).map(|x| x.1).as_deref() != Some(want) {
                        bad += 1;
                    }
                }
            }
        }
        if bad > 0 {
            println!("FAIL {name}: {bad} truncations decode wrong");
            fails += 1;
        }
        // A flipped bit in a chunk: the chunks before it survive.
        let mut flips = 0;
        for k in 0..40 {
            let ci = (k * 7919) % cuts.len();
            let start = if ci == 0 { FHDR + HDR.len() } else { cuts[ci - 1].0 };
            let at = start + (k * 31) % (cuts[ci].0 - start);
            let mut d = dlg.clone();
            d[at] ^= 1 << (k % 8);
            let want: &[u8] = if ci == 0 { HDR } else { &csv[..ends[cuts[ci - 1].1 - 1]] };
            if gen_all(&d).map(|x| x.1).as_deref() != Some(want) {
                flips += 1;
            }
        }
        if flips > 0 {
            println!("FAIL {name}: {flips} bit flips not contained");
            fails += 1;
        }
        let nr = rows.len() as f64;
        println!(
            "{:<30} {:>6} {:>8.2} {:>8.3} {:>6.1}x  chunks {}  ok",
            name,
            rows.len(),
            (csv.len() - HDR.len()) as f64 / nr,
            (dlg.len() - FHDR - HDR.len()) as f64 / nr,
            (csv.len() as f64) / dlg.len() as f64,
            cuts.len()
        );
        if let Some(dir) = std::env::args().nth(1) {
            std::fs::write(format!("{dir}/t{si}.dlg"), &dlg).unwrap();
            std::fs::write(format!("{dir}/t{si}.csv"), &csv).unwrap();
        }
    }
    // Long run: 10 h at 10/s in one-minute chunks (the csvlog cadence).
    let (_, unit, rows) = set(0, 360_000);
    let (csv, _) = reference(&rows, unit);
    let (dlg, cuts) = encode(&rows, unit, &mut || 600);
    let (_, out) = gen_all(&dlg).unwrap();
    if out != csv {
        println!("FAIL long run");
        fails += 1;
    }
    println!(
        "10 h at 10/s, 1-min chunks: CSV {} B, DLG {} B ({:.3} B/row, {:.1}x, {} chunks)",
        csv.len(),
        dlg.len(),
        dlg.len() as f64 / rows.len() as f64,
        csv.len() as f64 / dlg.len() as f64,
        cuts.len()
    );
    if fails > 0 {
        println!("{fails} FAILED");
        std::process::exit(1);
    }
    println!("dlog-test: all ok");
}
