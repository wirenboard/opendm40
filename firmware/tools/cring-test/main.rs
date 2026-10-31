// Host test of src/cring.rs (the scope's coded sample ring):
//   1. every live sample reads back (sequentially, backwards, at random)
//      after pushing signals that code well (sine + noise, DC, square),
//      white noise (every block stays packed) and a mix of both, through
//      several turns of the ring;
//   2. the ring keeps at least scope::PCAP = 65 520 samples once that many
//      were pushed, after every push (the worst case: white noise);
//   3. a coded block's bytes are dlog's scope block coding of its samples
//      (dlog::scope_block, the DLG file's format: tools/dlog.py reads it);
//   4. the views (pack10::Codes with cap 0) read the ring;
//   5. bytes per sample and the record length per signal (printed).
#![allow(dead_code)]

#[path = "../../src/pack10.rs"]
mod pack10;
#[path = "../../src/dlog.rs"]
mod dlog;
#[path = "../../src/cring.rs"]
mod cring;

use pack10::Src;

/// scope.rs: RB = ((CAP - STG) * 2 - 4) / 5 * 5 with CAP = 42 KiB, STG = 2048.
const RB: usize = ((42 * 1024 - 2048) * 2 - 4) / 5 * 5;
const PCAP: u32 = 65_520;

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 16) as u32
    }
}

fn sig(kind: &str, i: u64, rng: &mut Rng) -> u16 {
    let t = i as f64;
    let n = (rng.next() % 3) as f64 - 1.0;
    let v = match kind {
        "sine" => 512.0 + 300.0 * (t * 2.0 * std::f64::consts::PI / 25.0).sin() + n,
        "slow" => 512.0 + 400.0 * (t * 2.0 * std::f64::consts::PI / 2500.0).sin() + 2.0 * n,
        "dc" => 300.0 + n,
        "square" => (if (i / 100) % 2 == 0 { 200.0 } else { 800.0 }) + n,
        "noise" => (rng.next() % 1024) as f64,
        // 20 000 noise, 30 000 sine, ...
        "mixed" => {
            if i % 50_000 < 20_000 {
                (rng.next() % 1024) as f64
            } else {
                512.0 + 300.0 * (t * 2.0 * std::f64::consts::PI / 25.0).sin() + n
            }
        }
        _ => unreachable!(),
    };
    v.clamp(0.0, 1023.0) as u16
}

fn main() {
    let mut buf = vec![0u8; RB + 4];
    cring::init(buf.as_mut_ptr(), RB as u32);
    let mut fails = 0;
    let mut t = |ok: bool, what: &str| {
        println!("  {} {}", if ok { "ok  " } else { "FAIL" }, what);
        if !ok {
            fails += 1;
        }
    };
    for kind in ["sine", "slow", "dc", "square", "noise", "mixed"] {
        cring::reset(true);
        let mut rng = Rng(0x1234_5678_9abc);
        let total = 700_000u64;
        let mut refv: Vec<u16> = Vec::with_capacity(total as usize);
        let mut min_live = u32::MAX;
        for i in 0..total {
            let c = sig(kind, i, &mut rng);
            refv.push(c);
            cring::push(c);
            if i + 1 >= PCAP as u64 {
                min_live = min_live.min(cring::live());
            }
            assert_eq!(cring::end() as u64, i + 1);
            // A full read-back now and then (also mid-ring).
            if i == 99_999 || i == 333_333 || i + 1 == total {
                let (f, e) = (cring::first(), cring::end());
                let ok_seq = (f..e).all(|n| cring::at(n) == refv[n as usize]);
                let ok_back = (f..e).rev().step_by(7).all(|n| cring::at(n) == refv[n as usize]);
                let mut ok_rand = true;
                for _ in 0..20_000 {
                    let n = f + rng.next() % (e - f);
                    ok_rand &= cring::at(n) == refv[n as usize];
                }
                let v = cring::view(f, (e - f) as usize);
                let ok_view = (0..v.len()).step_by(13).all(|i| v.at(i) == refv[f as usize + i])
                    && v.sub(1000, 5000).at(17) == refv[f as usize + 1017];
                let ok_mm = {
                    let (lo, hi) = v.min_max(500, 9000);
                    let r = &refv[f as usize + 500..f as usize + 9000];
                    (lo, hi) == (*r.iter().min().unwrap(), *r.iter().max().unwrap())
                };
                // Short steps back (the cursor's history): sinc taps
                // k-2 .. k+3 per column, and up to 17 back from a code
                // read, at and across block starts.
                let mut ok_hist = true;
                let mut k = f + 3;
                while k + 4 < e.min(f + 20_000) {
                    let w: f32 = (0..6).map(|d| cring::at(k + d - 2) as f32).sum();
                    ok_hist &= w == (0..6).map(|d| refv[(k + d - 2) as usize] as f32).sum::<f32>();
                    k += 1 + (k % 3 == 0) as u32;
                }
                let b0 = f / cring::BLK + 2;
                for b in b0..b0 + 4 {
                    for o in [0u32, 1, 5, 15, 16, 17, 18, 100, cring::BLK - 1] {
                        let p = b * cring::BLK + o;
                        if p >= e || p < f + 20 {
                            continue;
                        }
                        ok_hist &= cring::at(p) == refv[p as usize];
                        for d in 1..=18 {
                            ok_hist &= cring::at(p - d) == refv[(p - d) as usize];
                        }
                        ok_hist &= cring::at(p + 1) == refv[p as usize + 1];
                    }
                }
                let ok_mm2 = {
                    // min_max right after a history hit (the cursor stays).
                    let p = f + 3 * cring::BLK + 50;
                    let _ = (cring::at(p), cring::at(p - 3));
                    let (lo, hi) = v.min_max((p - f - 3) as usize, (p - f + 3000) as usize);
                    let r = &refv[p as usize - 3..p as usize + 3000];
                    (lo, hi) == (*r.iter().min().unwrap(), *r.iter().max().unwrap())
                };
                t(ok_seq && ok_back && ok_rand && ok_view && ok_mm && ok_hist && ok_mm2, &format!("{kind}: {} live samples read back at {} pushed (sequential, backwards, random, view, min/max, short steps back)", e - f, i + 1));
            }
        }
        t(min_live >= PCAP, &format!("{kind}: at least {PCAP} live samples after every push (least {min_live})"));
        // Coded blocks = dlog's coding of their samples.
        let (f, e) = (cring::first(), cring::end());
        let (mut ncoded, mut nall, mut same) = (0, 0, true);
        let mut b = f / cring::BLK + 1;
        while (b + 1) * cring::BLK <= e {
            let (st, len, coded) = cring::block(b);
            nall += 1;
            if coded {
                ncoded += 1;
                let codes = &refv[(b * cring::BLK) as usize..((b + 1) * cring::BLK) as usize];
                let mode = (buf[st as usize] >> 5) as u32;
                let mut want = Vec::new();
                dlog::scope_block(codes, mode, &mut |x| want.push(x));
                let got: Vec<u8> = (0..len).map(|k| buf[(st as usize + k as usize) % RB]).collect();
                same &= got == want;
            }
            b += 1;
        }
        t(same, &format!("{kind}: {ncoded} of {nall} whole blocks coded, each = dlog::scope_block of its samples"));
        let bytes = RB as f64;
        println!("       {kind}: record {} samples ({:.3} B/sample over the ring)", e - f, bytes / (e - f) as f64);
        if kind == "noise" {
            t(ncoded == 0, "noise: no block coded");
        }
        // Longer than the packed ring (65 532) by what each codes to.
        let want = match kind {
            "dc" => 200_000,
            "slow" => 150_000,
            "sine" | "square" => 110_000,
            "mixed" => 80_000,
            _ => 0,
        };
        t(e - f >= want, &format!("{kind}: record >= {want} samples ({})", e - f));
    }
    // Packed-only (compress off: the Wave view's fast rates).
    cring::reset(false);
    let mut rng = Rng(7);
    let mut refv = Vec::new();
    for i in 0..300_000u64 {
        let c = sig("sine", i, &mut rng);
        refv.push(c);
        cring::push(c);
    }
    let (f, e) = (cring::first(), cring::end());
    t((f..e).all(|n| cring::at(n) == refv[n as usize]) && e - f >= PCAP && e - f < 66_000, &format!("no coding: {} live samples read back", e - f));
    if fails > 0 {
        println!("FAIL: {fails}");
        std::process::exit(1);
    }
    println!("PASS");
}
