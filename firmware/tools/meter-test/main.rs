// Host test crate for src/meter.rs, src/dmm.rs and src/logger.rs (FR-05 smoothing / rate,
// UF-21): the modules are compiled for the host with their register-level
// dependencies; the tests only call pure logic (no MMIO is touched).
#![allow(dead_code, unused_imports, clippy::all)]
#[path = "../../src/mmio.rs"]
mod mmio;
#[path = "../../src/hy3131.rs"]
mod hy3131;
#[path = "../../src/hy3131_tables.rs"]
mod hy3131_tables;
#[path = "../../src/clock.rs"]
mod clock;
#[path = "../../src/sentinel.rs"]
mod sentinel;
// The danger alarm's pattern / event rules (pure).
#[path = "../../src/danger.rs"]
mod danger;
// FR-05 FAST: the unchopped AD1 offset (pure).
#[path = "../../src/fastvos.rs"]
mod fastvos;
#[path = "../../src/dmm.rs"]
mod dmm;
#[path = "../../src/meter.rs"]
mod meter;
// The data logger's ring / gap accounting and the AD2 rate ladder (hy3131.rs).
// logger.rs: the DLG reply coder.
#[path = "../../src/fastenc.rs"]
mod fastenc;
#[path = "../../src/logger.rs"]
mod logger;
// logger.rs `start` / `dc_prepare` (the DC rows' AD2 row), dmm.rs acdcm.rs
// (AC+DC method C): chip access only.
mod ad2exp {
    pub fn dc_enter(_mode: u8, _r: u8, _r26: u8, _measure: bool) {}
    pub fn dcv_row(_r: u8, _r26: u8, _opt: u8, _op1chop: u8, _ad2: bool) -> [u8; crate::hy3131::CFG_LEN] {
        [0; crate::hy3131::CFG_LEN]
    }
}
// The wave view's streaming trigger (logger.rs `wave`).
#[path = "../../src/trigsoft.rs"]
mod trigsoft;
// The AD2 stream ISR's phase lock (pure; its model-chip tests).
// The inrush capture's detector / results (src/inrush.rs, pure).
#[path = "../../src/inrush.rs"]
mod inrush;
#[path = "../../src/ad2pll.rs"]
mod ad2pll;
// Scope / wave view measurements (src/scope_meas.rs; `compute` needs the
// scope's volt map, the tests use `compute_codes`).
#[path = "../../src/scope_meas.rs"]
mod scope_meas;
mod scope {
    pub struct Scope;
    impl Scope {
        pub fn volts(&self, c: f32) -> f32 {
            c
        }
    }
}

// Frequency / period from whole cycles (scope_meas::compute_codes). Device
// d8e249d: the DMM wave view at 5 ms/div (480 samples at 9600 S/s) showed
// "Freq: --" for a 40 Hz square: two periods, triggered on a rising edge
// at the centre, leave one whole rise inside the frame.
#[cfg(test)]
mod meas {
    use crate::scope_meas::compute_codes;

    fn square(n: usize, fs: f32, f: f32, ph: f32) -> Vec<u16> {
        (0..n).map(|i| if ((i as f32 / fs * f + ph) % 1.0) < 0.5 { 800 } else { 200 }).collect()
    }

    #[test]
    fn two_periods_rising_edge_at_the_ends() {
        // A rise at sample 0 (no level before it) and one at 480 (past the
        // end), one inside at 240: the falls (120, 360) give the period.
        let d = square(480, 9600.0, 40.0, 0.0);
        let r = compute_codes(&d, 9600.0, 0.0, 0.01);
        assert!((r.v[0] - 40.0).abs() < 0.2, "Freq {}", r.v[0]);
        assert!((r.v[9] - 0.025).abs() < 1e-4, "Cycl {}", r.v[9]);
        assert!((r.v[1] - 50.0).abs() < 1.0, "duty {}", r.v[1]);
    }

    /// Triangle (`ramp` false) or rising ramp of codes lo..hi, `f` Hz.
    fn tri(n: usize, fs: f32, f: f32, lo: f32, hi: f32, ramp: bool) -> Vec<u16> {
        (0..n)
            .map(|i| {
                let p = (i as f32 / fs * f + 0.13) % 1.0;
                let x = if ramp { p } else if p < 0.5 { 2.0 * p } else { 2.0 - 2.0 * p };
                (lo + (hi - lo) * x).round() as u16
            })
            .collect()
    }

    // #129: a triangle / ramp fills the top and base bands evenly; their
    // "mode" was any bin there (emulator: triangle +Duty 64.9 %, Vamp 627 mV
    // of 905). Top / base are then Vmax / Vmin: 50 % duty, Vamp = Vpp.
    #[test]
    fn triangle_and_ramp_top_base_are_the_extremes() {
        for (ramp, n, fs, f) in [(false, 25_000, 12.5e6, 2000.0), (true, 25_000, 12.5e6, 1000.0), (false, 400, 4000.0, 30.0), (true, 2048, 1e5, 210.0)] {
            let d = tri(n, fs, f, 300.0, 727.0, ramp);
            let r = compute_codes(&d, fs, 0.0, 0.01);
            let what = if ramp { "ramp" } else { "triangle" };
            assert!((r.v[14] - 7.27).abs() < 0.011 && (r.v[15] - 3.0).abs() < 0.011, "{what} {n}: Vtop {} Vbas {}", r.v[14], r.v[15]);
            assert!((r.v[16] - r.v[10]).abs() < 1e-4, "{what} {n}: Vamp {} Vpp {}", r.v[16], r.v[10]);
            assert!((r.v[1] - 50.0).abs() < 1.0, "{what} {n}: +Duty {}", r.v[1]);
            assert!((r.v[0] - f).abs() < f * 0.01, "{what} {n}: Freq {}", r.v[0]);
        }
    }

    // A plateau keeps the mode: a square with a 3-code overshoot spike per
    // edge and +-2 codes of noise has Vtop / Vbas on its flat parts, not on
    // the spikes; a 10 % duty pulse train too.
    #[test]
    fn square_top_base_stay_on_the_plateau() {
        let noise = |i: usize| ((i * 7919) % 5) as i32 - 2;
        for duty in [0.5f32, 0.1] {
            let d: Vec<u16> = (0..20_000)
                .map(|i| {
                    let p = (i as f32 / 1e6 * 1000.0) % 1.0;
                    let base = if p < duty { 800 } else { 200 };
                    let spike = if p < 0.002 { 40 } else if (p - duty).abs() < 0.002 { -40 } else { 0 };
                    (base + spike + noise(i)) as u16
                })
                .collect();
            let r = compute_codes(&d, 1e6, 0.0, 0.01);
            assert!((r.v[14] - 8.0).abs() <= 0.021 && (r.v[15] - 2.0).abs() <= 0.021, "duty {duty}: Vtop {} Vbas {}", r.v[14], r.v[15]);
            assert!((r.v[1] - duty * 100.0).abs() < 1.0, "duty {duty}: +Duty {}", r.v[1]);
        }
    }

    // Review of #131: small squares (a 30 % band of 9..18 bins, under
    // ~half a division) keep their plateau too: from code 500 to 500 + amp,
    // noise 0..2 codes, a +5 / -5 code spike at the rising / falling edge.
    // A triangle of the same small size still takes the extremes.
    #[test]
    fn small_square_keeps_the_plateau() {
        let noise = |i: usize| ((i * 7919) % 3) as i32;
        for amp in [30i32, 36, 40, 60, 100] {
            for duty in [0.5f32, 0.1] {
                let d: Vec<u16> = (0..20_000)
                    .map(|i| {
                        let p = (i as f32 / 1e6 * 1000.0) % 1.0;
                        let base = if p < duty { 500 + amp } else { 500 };
                        let spike = if p < 0.002 { 5 } else if (p - duty).abs() < 0.002 { -5 } else { 0 };
                        (base + spike + noise(i)) as u16
                    })
                    .collect();
                let r = compute_codes(&d, 1e6, 0.0, 0.01);
                let (top, bas) = ((r.v[14] * 100.0).round() as i32, (r.v[15] * 100.0).round() as i32);
                assert!((500 + amp..=502 + amp).contains(&top) && (500..=502).contains(&bas),
                    "amp {amp} duty {duty}: Vtop code {top} Vbas code {bas} (Vmax {} Vmin {})", r.v[12] * 100.0, r.v[13] * 100.0);
            }
        }
        let d = tri(25_000, 12.5e6, 2000.0, 500.0, 540.0, false);
        let r = compute_codes(&d, 12.5e6, 0.0, 0.01);
        assert!(r.v[14] == r.v[12] && r.v[15] == r.v[13], "triangle 40 codes: Vtop {} Vbas {}", r.v[14], r.v[15]);
    }

    #[test]
    fn rises_preferred_and_one_edge_is_none() {
        let d = square(4800, 9600.0, 40.0, 0.25);
        let r = compute_codes(&d, 9600.0, 0.0, 0.01);
        assert!((r.v[0] - 40.0).abs() < 0.05, "Freq {}", r.v[0]);
        // Less than a period with one rise and one fall: no frequency.
        let d = square(200, 9600.0, 40.0, 0.25);
        assert!(compute_codes(&d, 9600.0, 0.0, 0.01).v[0].is_nan());
    }
}

fn main() {}

// Calibration record CRC (dmm.rs `rec_crc_ok`, `load_record`): a record
// whose CRC fails is treated as erased, as the stock does. Expected CRC from
// an independent implementation (CRC-16/CMS: poly 0x8005, init 0xFFFF, no
// reflection, no final XOR; its check value for "123456789" is 0xAEE7).
#[cfg(test)]
mod rec_crc {
    use crate::dmm::{rec_crc_ok, REC_CRC_END};

    /// Bytes 4..0x320 = (i * 7 + 3) & 0xFF; their CRC is 0x6FE8.
    fn record() -> Vec<u32> {
        let mut b: Vec<u8> = (0..REC_CRC_END).map(|i| (i * 7 + 3) as u8).collect();
        b[0..4].copy_from_slice(&(0x6FE8u32 | (!0x6FE8u32 & 0xFFFF) << 16).to_le_bytes());
        b.chunks(4).map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect()
    }

    #[test]
    fn good_record_passes() {
        let r = record();
        assert!(rec_crc_ok(|i| r[i]));
    }

    #[test]
    fn one_flipped_bit_fails() {
        let mut r = record();
        r[0x168 / 4] ^= 1 << 3;
        assert!(!rec_crc_ok(|i| r[i]));
    }

    #[test]
    fn wrong_complement_fails() {
        let mut r = record();
        r[0] ^= 1 << 16;
        assert!(!rec_crc_ok(|i| r[i]));
    }

    #[test]
    fn erased_flash_fails() {
        assert!(!rec_crc_ok(|_| u32::MAX));
    }
}

// DMM frequency mode (mode 14) end to end from synthetic HY3131 counter
// values: counters -> kHz -> auto range -> readout text (main and the AC V/A
// aux strip), plus the SI value SCPI / BT report. Owner report: 700 Hz and
// 999 Hz showed "1 kHz", 1.5 kHz "2 kHz".
#[cfg(test)]
mod freq {
    use crate::dmm::{counter_vals, freq_range_for, freq_val};

    /// 60000 counts: factor 6 * over 1.03.
    const X: f32 = 6.0 * 1.03;

    /// Counter registers for an input of `hz` with 50 % duty: the gate of
    /// 0x40000 time base ticks (4.9152 MHz) ends on the next input edge.
    fn counters(hz: f64) -> (u32, u32, u32) {
        let tb = 4_915_280.0;
        let ctb = (262_144.0 * hz / tb).ceil().max(1.0);
        let n = (ctb * tb / hz).round();
        ((n / 2.0) as u32, ctb as u32, (n - 262_144.0) as u32)
    }

    fn show(hz: f64) -> (String, String, String, String, f32, f32) {
        let (ctc, ctb, cta) = counters(hz);
        let (f, duty) = counter_vals(ctc, ctb, cta);
        let r = freq_range_for(f, X, 0);
        let mut b = [0u8; 16];
        let m = freq_val(f, r, false);
        let a = freq_val(f, r, true);
        let mt = crate::meter::text(&mut b, &m).to_string();
        let at = crate::meter::text(&mut b, &a).to_string();
        (mt, m.unit.to_string(), at, a.unit.to_string(), f * 1e3, duty)
    }

    #[test]
    fn readout_text() {
        let cases: [(f64, &str, &str, &str); 11] = [
            (50.0, "50.000", "Hz", "50.00"),
            (600.0, "600.00", "Hz", "600.0"),
            (700.0, "0.7000", "kHz", "0.700"),
            (999.0, "0.9990", "kHz", "0.999"),
            (1_000.0, "1.0000", "kHz", "1.000"),
            (1_500.0, "1.5000", "kHz", "1.500"),
            (32_768.0, "32.768", "kHz", "32.77"),
            (40_000.0, "40.000", "kHz", "40.00"),
            (100_000.0, "100.00", "kHz", "100.0"),
            (1_000_000.0, "1.0000", "MHz", "1.000"),
            (10_000_000.0, "10.000", "MHz", "10.00"),
        ];
        for (hz, main, unit, aux) in cases {
            let (mt, mu, at, au, si, duty) = show(hz);
            assert_eq!((mt.as_str(), mu.as_str()), (main, unit), "{hz} Hz main");
            assert_eq!((at.as_str(), au.as_str()), (aux, unit), "{hz} Hz aux");
            assert!(((si as f64) - hz).abs() <= hz * 2e-5, "{hz} Hz: SI {si}");
            assert!((duty - 50.0).abs() < 0.01, "{hz} Hz: duty {duty}");
        }
    }

    #[test]
    fn ranges() {
        // Range r spans up to X * 10^(r-2) kHz; above all ranges: unchanged.
        let r = |f: f32| freq_range_for(f, X, 9);
        assert_eq!([r(0.0), r(0.0617), r(0.0619), r(0.617), r(0.619), r(6.17), r(6.19)], [0, 0, 1, 1, 2, 2, 3]);
        assert_eq!([r(61.7), r(61.9), r(617.0), r(619.0), r(6170.0), r(6190.0), r(61_900.0)], [3, 4, 4, 5, 5, 6, 9]);
    }
}
