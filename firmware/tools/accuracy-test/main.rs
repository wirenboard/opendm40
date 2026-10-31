//! Host test of src/accuracy.rs (docs/dmm-accuracy.md): the row per HY3131
//! mode / range against the manual's table, and the uncertainty in units of
//! the last digit for several readings on DM40A / B / C.
//!   tools/accuracy-test/run.sh
#[allow(dead_code)]
#[path = "../../src/accuracy.rs"]
mod acc;

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

/// (pct in 0.01 %, N) of mode / range, or None.
fn row(mode: u8, range: i8) -> Option<(u16, u8)> {
    acc::spec(mode, range, false, true, false).map(|s| (s.pct, s.n))
}

fn main() {
    // The manual's table (V1.5, PDF page 4), every function / range.
    let rows: &[(&str, u8, &[i8], Option<(u16, u8)>)] = &[
        ("AC V 6 V .. 750 V", 0, &[0, 1, 2, 3], Some((30, 15))),
        ("AC mV 600 mV", 2, &[1], Some((30, 15))),
        ("AC mV 60 mV: no row", 2, &[0], None),
        ("DC V 6 V .. 1000 V", 1, &[0, 1, 2, 3], Some((3, 5))),
        ("DC mV 600 mV", 3, &[1], Some((3, 5))),
        ("DC mV 60 mV: no row", 3, &[0], None),
        ("AC A 6 A / 10 A", 4, &[0, 1], Some((75, 15))),
        ("AC mA", 5, &[0, 1], Some((50, 15))),
        ("AC uA", 6, &[0, 1], Some((50, 15))),
        ("DC A 6 A / 10 A", 7, &[0, 1], Some((20, 5))),
        ("DC mA", 8, &[0, 1], Some((15, 5))),
        ("DC uA", 9, &[0, 1], Some((15, 5))),
        ("ohm 60 ohm: no row", 10, &[0], None),
        ("ohm 600 .. 6 M", 10, &[1, 2, 3, 4, 5], Some((10, 5))),
        ("ohm 60 M", 10, &[6], Some((20, 10))),
        ("continuity", 11, &[0], Some((20, 5))),
        ("diode", 12, &[0], Some((100, 5))),
        ("cap 6 nF .. 60 uF", 13, &[0, 1, 2, 3, 4], Some((250, 30))),
        ("cap 600 uF .. 60 mF", 13, &[5, 6, 7], Some((350, 30))),
        ("Hz", 14, &[0, 3, 6], Some((1, 5))),
        ("unknown mode", 16, &[0], None),
    ];
    for (name, mode, ranges, want) in rows {
        for &r in *ranges {
            let got = row(*mode, r);
            check(name, got == *want, format!("mode {mode} r{r}: {got:?}, want {want:?}"));
        }
    }
    check("AC+DC: no row", acc::spec(1, 0, true, true, false).is_none(), String::new());
    // The experimental 600 Mohm (ohm r7): no row; 60 Mohm (r6) keeps its own.
    check("ohm 600 Mohm: no row", acc::spec(10, 7, false, true, false).is_none(), String::new());
    check("ohm 60 Mohm: row 7", acc::spec(10, 6, false, true, false).map(|s| (s.pct, s.n)) == Some((20, 10)), String::new());
    let t = acc::spec(15, 0, false, true, false).unwrap();
    check("TC K deg C", (t.pct, t.n, t.abs) == (100, 5, true), format!("{t:?}"));
    let t = acc::spec(15, 0, false, true, true).unwrap();
    check("TC K deg F", (t.pct, t.n, t.abs) == (150, 5, true), format!("{t:?}"));
    check("NTC / PT: no row", acc::spec(15, 0, false, false, false).is_none(), String::new());

    // Uncertainty in last-digit units: (name, mode, range, value, dec, model, want).
    let sp = |m: u8, r: i8, f: bool| acc::spec(m, r, false, true, f).unwrap();
    let cases: &[(&str, u8, i8, f32, u8, u8, u32)] = &[
        // 0.03 % x 48176 + 5 = 19.45 -> 20 (0.0020 V)
        ("DC V 4.8176 V, C", 1, 0, 4.8176, 4, 2, 20),
        // B: 48176 < 52000, the 5 V range: same
        ("DC V 4.8176 V, B", 1, 0, 4.8176, 4, 1, 20),
        // A: 48176 > 42000 counts: the stock shows 4.818 V on 40 V, d = 1 mV:
        // 14.45 + 50 = 64.45 -> 65 (0.0065 V)
        ("DC V 4.8176 V, A (x10 d)", 1, 0, 4.8176, 4, 0, 65),
        // A at 41999 counts: still the 4 V range
        ("DC V 4.1999 V, A", 1, 0, 4.1999, 4, 0, 18),
        // 0.03 % of 5.0000 V + 5 d = 20 exactly (no float hair up to 21)
        ("DC V 5.0000 V rounding", 1, 0, 5.0, 4, 2, 20),
        ("DC V -5.6497 V (sign)", 1, 0, -5.6497, 4, 2, 22),
        // above the C's own step-up (6.18 V): the 60 V range's d
        ("DC V 6.6497 V, C (x10 d)", 1, 0, 6.6497, 4, 2, 70),
        // 1000 V range: 1000.0 V, 10000 counts: 3 + 5 = 8 (0.8 V) on every model
        ("DC V 1000.0 V, A", 1, 3, 1000.0, 1, 0, 8),
        // 600 V range on A above 420 V: the stock's 1000 V range, d = 0.1 V:
        // 13.33 + 50 = 63.3 -> 64 (0.64 V)
        ("DC V 444.44 V, A", 1, 2, 444.44, 2, 0, 64),
        ("DC V 444.44 V, C", 1, 2, 444.44, 2, 2, 19),
        ("DC mV 444.44 mV", 3, 1, -444.44, 2, 2, 19),
        // AC V 230.12 V: 0.3 % x 23012 + 15 = 84.04 -> 85 (0.85 V)
        ("AC V 230.12 V", 0, 2, 230.12, 2, 2, 85),
        ("AC A 4.4444 A", 4, 0, 4.4444, 4, 2, 349),
        ("DC mA 12.000 mA", 8, 0, 12.0, 3, 2, 23),
        // A: 12.000 mA < 42000 counts on 60 mA (stock 40 mA): same
        ("DC mA 12.000 mA, A", 8, 0, 12.0, 3, 0, 23),
        // A: 50.000 mA > 42000: the stock's 400 mA range, d = 0.01 mA
        ("DC mA 50.000 mA, A", 8, 0, 50.0, 3, 0, 125),
        ("DC A 10 A range 9.999 A", 7, 1, 9.999, 3, 2, 25),
        ("DC A 6 A range 4.4444 A, A (x10 d)", 7, 0, 4.4444, 4, 0, 139),
        ("ohm 444.44 k", 10, 4, 444.44, 2, 2, 50),
        ("ohm 44.444 M", 10, 6, 44.444, 3, 2, 99),
        ("continuity 444.4", 11, 0, 444.4, 1, 0, 14),
        ("diode 0.6123 V", 12, 0, 0.6123, 4, 0, 67),
        // cap 4.444 nF: 2.5 % x 4444 + 30 = 141.1 -> 142 (0.142 nF)
        ("cap 4.444 nF, C", 13, 0, 4.444, 3, 2, 142),
        // A: 4444 > 4200 (1000-count decade): the stock's 40 nF, d = 0.01 nF
        ("cap 4.444 nF, A (x10 d)", 13, 0, 4.444, 3, 0, 412),
        ("cap 44.444 mF", 13, 7, 44.44, 2, 2, 186),
        ("Hz 50.000 kHz", 14, 3, 50.0, 3, 2, 10),
        ("Hz 9.999 MHz", 14, 6, 9.999, 3, 2, 6),
        // TC K: 1 % of 200.0 + 5 deg C = 7.0 deg C -> 70 tenths
        ("TC -200.0 C", 15, 0, -200.0, 1, 0, 70),
        // 1.5 % of 1000.0 F + 5 F = 20.0 F
        ("TC 1000.0 F", 15, 0, 1000.0, 1, 2, 200),
        ("TC 0.0 C", 15, 0, 0.0, 1, 2, 50),
    ];
    for &(name, mode, r, v, dec, model, want) in cases {
        let f = name.ends_with(" F");
        let got = acc::err_ulps(sp(mode, r, f), v, dec, model);
        check(name, got == want, format!("{got}, want {want}"));
    }

    // Grey digits (doc section 5): the main readout's 7 cells ("%7.<dec>f")
    // from `grey_from` on are grey; (name, mode, range, value, dec, model,
    // orange part, grey part). The extra digit is grey whenever the label
    // applies (ulps >= 1), so it is not listed.
    let grey: &[(&str, u8, i8, f32, u8, u8, &str, &str)] = &[
        // +-0.0020 V: p = 10^-3, the 10^-4 digit grey
        ("DC V 4.8176 V, C", 1, 0, 4.8176, 4, 2, " 4.817", "6"),
        // DM40A: +-0.0065 V, the same split
        ("DC V 4.8176 V, A", 1, 0, 4.8176, 4, 0, " 4.817", "6"),
        // +-0.0006 V: every digit significant (only the extra digit grey)
        ("DC V 1.0000 V", 1, 0, 1.0, 4, 2, " 1.0000", ""),
        ("DC V -0.0123 V", 1, 0, -0.0123, 4, 2, "-0.0123", ""),
        // +-0.0053 V
        ("AC V 1.2345 V", 0, 0, 1.2345, 4, 2, " 1.234", "5"),
        // +-0.0150 V: p = 10^-2, two digits grey
        ("AC V 4.5000 V", 0, 0, 4.5, 4, 2, " 4.50", "00"),
        // +-0.85 V: p = 10^-1
        ("AC V 230.12 V", 0, 2, 230.12, 2, 2, " 230.1", "2"),
        // +-0.0016 V >= the reading: no leading-digit exception
        ("AC V 0.0009 V (leading digit)", 0, 0, 0.0009, 4, 2, " 0.000", "9"),
        ("AC V 0.0000 V", 0, 0, 0.0, 4, 2, " 0.000", "0"),
        ("AC A 4.4444 A", 4, 0, 4.4444, 4, 2, " 4.44", "44"),
        // +-0.0018 kohm
        ("ohm 1.2345 k", 10, 2, 1.2345, 4, 2, " 1.234", "5"),
        ("ohm 444.44 k", 10, 4, 444.44, 2, 2, " 444.4", "4"),
        // +-0.099 M rounds to +-0.1 M: the hundredths go grey too
        ("ohm 44.444 M", 10, 6, 44.444, 3, 2, " 44.4", "44"),
        // +-1.49 uF: p = 10^0, the point goes grey with the digit after it
        ("cap 47.30 uF", 13, 4, 47.3, 2, 2, "  47", ".30"),
        // +-0.155 nF
        ("cap 5.000 nF", 13, 0, 5.0, 3, 2, "  5.0", "00"),
        // +-0.031 nF >= the reading: the 5 goes grey too
        ("cap 0.005 nF (leading digit)", 13, 0, 0.005, 3, 2, "  0.00", "5"),
        ("cap 0.050 nF", 13, 0, 0.05, 3, 2, "  0.05", "0"),
        ("cap 44.44 mF", 13, 7, 44.44, 2, 2, "  44", ".44"),
        // Every capacitance range as dmm.rs cap_val shows it (the 6 / 60 mF
        // ranges in uF): the uncertainty's leading place stays orange.
        // 60 nF (merged), below 6 nF: 3 decimals, +-0.14 nF
        ("cap 4.444 nF on 60 nF", 13, 1, 4.444, 3, 2, "  4.4", "44"),
        // 60 nF: +-1.5 nF
        ("cap 47.30 nF", 13, 1, 47.3, 2, 2, "  47", ".30"),
        // 6 uF: +-0.12 uF
        ("cap 3.499 uF", 13, 3, 3.499, 3, 2, "  3.4", "99"),
        // 600 uF: +-19 uF
        ("cap 444.4 uF", 13, 5, 444.4, 1, 2, "  44", "4.4"),
        // 6 mF in uF: d = 1 uF (the stock's 1.018 mF), +-70 uF
        ("cap 1018 uF", 13, 6, 1018.0, 0, 2, "   101", "8"),
        // 60 mF in uF: d = 10 uF (the stock's 44.44 mF), +-1900 uF
        ("cap 44440 uF", 13, 7, 44440.0, 0, 2, "  44", "440"),
        // +-0.006 MHz: every digit
        ("Hz 9.999 MHz", 14, 6, 9.999, 3, 2, "  9.999", ""),
        ("Hz 50.000 kHz", 14, 3, 50.0, 3, 2, " 50.00", "0"),
        // TC K: +-7.0 C
        ("TC -200.0 C", 15, 0, -200.0, 1, 0, " -200", ".0"),
        ("TC 1000.0 F", 15, 0, 1000.0, 1, 2, " 100", "0.0"),
    ];
    for &(name, mode, r, v, dec, model, a, b) in grey {
        let f = name.ends_with(" F");
        let u = acc::round_sig(acc::err_ulps(sp(mode, r, f), v, dec, model));
        let s = format!("{:>7.*}", dec as usize, v);
        let i = acc::grey_from(s.as_bytes(), Some(u)).min(7);
        let (ga, gb) = s.split_at(i);
        check(name, (ga, gb) == (a, b), format!("{s:?} +-{u}: {ga:?} | {gb:?}, want {a:?} | {b:?}"));
        // The strip: the same value trimmed (no extra digit).
        let t = s.trim_start();
        let j = acc::grey_from(t.as_bytes(), Some(u)).min(t.len());
        check(name, &t[j..] == b, format!("strip {t:?}: grey {:?}, want {b:?}", &t[j..]));
    }
    for (u, want) in [(201, 200), (20, 20), (15, 15), (149, 150), (96, 100), (25, 30), (3, 3), (1999, 2000), (0, 0)] {
        check("round_sig", acc::round_sig(u) == want, format!("{u} -> {}, want {want}", acc::round_sig(u)));
    }
    check("no accuracy: none grey", acc::grey_from(b" 4.8176", None) == 7, String::new());

    let (n, f) = unsafe { (CASES, FAILS) };
    println!("accuracy-test: {n} cases, {f} failed");
    std::process::exit(if f == 0 { 0 } else { 1 });
}
