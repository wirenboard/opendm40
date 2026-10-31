//! The screens of the root README.md (firmware/docs/img/*.png), 1x, with ordinary
//! values and an ordinary status bar (00:12:34, no REC, no host on BT), not
//! the worst cases of the matrix. Synthetic data, drawn by the firmware's
//! own code; each screen also goes through the layout rules.
//!   tools/ui-render/run.sh OUT --readme   (then copy OUT/*.png to docs/img/)
use super::*;

fn status_real() -> ui::Status {
    ui::Status { uptime_s: 12 * 60 + 34, bt: false, usb: false, bars: 4, charging: false, locked: false, rec: false, csv: false, rec_dim: false, page: 0 }
}

fn save1(out: &str, name: &str) {
    host::save_ppm(&format!("{out}/{name}.ppm"), 1);
}

const DMM_REGIONS: [(&str, Bx); 7] = [
    ("screen", Bx { x0: 0, y0: 0, x1: 479, y1: 319 }),
    ("bar", Bx { x0: 0, y0: 0, x1: 479, y1: ui::BAR_H as i32 - 1 }),
    ("top", Bx { x0: 0, y0: 34, x1: 479, y1: 151 }),
    ("history", Bx { x0: 0, y0: 144, x1: 479, y1: 175 }),
    ("axis", Bx { x0: 0, y0: 174, x1: 79, y1: 279 }),
    ("side", Bx { x0: 402, y0: 174, x1: 479, y1: 279 }),
    ("buttons", Bx { x0: 0, y0: 277, x1: 479, y1: 319 }),
];

/// A DMM page: the reading, the history row or the bargraph, the trend
/// chart of the last ~12 min (the value with a small wobble).
fn dmm_page(r: &Reading, b: Button, hist: bool) {
    host::reset(0);
    let mut dv = ui::DmmView::new();
    dv.bar_on = !hist;
    if r.cpl_auto {
        ui::set_cpl_auto(true);
    }
    dv.draw_static(b, r.mode, r.acdc, false);
    ui::set_cpl_auto(false);
    if hist {
        ui::history(&[(r.main.v, r.main.dec, r.main.unit); 5], 5, 4);
    } else {
        dv.bar_update(r);
    }
    dv.reading(r, 0, meter::V_NONE);
    ui::status_right(&status_real());
    let q = 10f32.powi(-(r.main.dec as i32));
    let pts: Vec<f32> = (0..321).map(|i| r.main.v + (((i / 37) % 3) as f32 - 1.0) * q).collect();
    let side = ui::Side { max: Some(r.main.v + q), avg: Some(r.main.v), min: Some(r.main.v - q), sd: Some(0.6 * q), rel: None, secs: Some(12 * 60 + 30), rec: false };
    ui::chart(&mut ui::ChartCache::new(), &pts, r.main.dec, 2, Some(&side), r.main.unit, ui::RateChip { rate: 1, period_ms: dmm::rate_period(dmm::Rate::Medium, r.mode), applies: true });
}

pub fn run(out: &str) -> usize {
    std::fs::create_dir_all(out).unwrap();
    let mut ck = Checker { cases: 0, violations: 0, seen: Default::default() };

    // DMM, DC V, trend chart.
    let dcv = rd(1, val(4.8176, 4, "V"), None, None, false);
    dmm_page(&dcv, Button::Volt, true);
    ck.check("readme/dmm", &DMM_REGIONS);
    save1(out, "dmm");

    // AUTO AC/DC on a DC source: the DC part shown, the AC part small.
    let auto = cpl_auto(rd(1, val(5.0012, 4, "V"), Some(val(0.0021, 4, "V")), Some(val(5.0012, 4, "V")), false));
    dmm_page(&auto, Button::Volt, true);
    ck.check("readme/auto-acdc", &DMM_REGIONS);
    save1(out, "auto-acdc");

    // Burden voltage: 123.45 uA through the 47.5 ohm path = 5.9 mV.
    let bd = rd(9, val(123.45, 2, "\u{b5}A"), Some(val(5.9, 1, "mV")), None, false);
    dmm_page(&bd, Button::Amp, false);
    ck.check("readme/burden", &DMM_REGIONS);
    save1(out, "burden");

    // A-JACK: DC V selected, current in the A jack.
    let mut aj = dcv;
    aj.alert = sentinel::Alert::AJack;
    dmm_page(&aj, Button::Volt, false);
    ck.check("readme/ajack", &DMM_REGIONS);
    save1(out, "ajack");

    // Power meter, AC+DC parts (the matrix's "parts-mixed" values).
    {
        let pv = pw_cases().into_iter().find(|c| c.0 == "parts-mixed").unwrap().1;
        host::reset(0);
        power_page_lim(pv, 0, sentinel::Alert::None, ui::RateChip { rate: 1, period_ms: 430, applies: false }, 0);
        ui::status_right(&status_real());
        ck.check("readme/power", &pw_regions(&pv));
        save1(out, "power");
    }

    // Combined view: AC V in the meter strip over a live 1 kHz sine, the
    // generator running (the strip's GEN text).
    {
        let base = &stock::scenes()[0];
        let mut m = base.m;
        m.combined = true;
        let sc = stock::Scene { name: "readme-combined", m, popup: scope_ui::Popup::None, trace: base.trace.clone(), persist: None, cols: 399, cells: base.cells.clone() };
        host::reset(0);
        scope_ui::invalidate();
        scope_ui::strip_invalidate();
        stock::load(&sc);
        scope_ui::update(&sc.m);
        scope_ui::cells(&sc.m, &sc.cells);
        let acv = rd(0, val(1.0606, 4, "V"), Some(val(1.000, 3, "kHz")), Some(val(50.0, 1, "%")), false);
        let mut d = ui::strip_data(&acv, 0, 0, false, 0, false, None);
        d.gen = scope_ui::Txt::new("1.00kHz");
        scope_ui::meter_strip(&d);
        ck.check_area("readme/combined", &[("screen", Bx::new(0, 0, 480, 320))], true, Some(Bx::new(13, 20, 401, 56)));
        save1(out, "combined");
    }

    // Scope with the FFT on: a 1 kHz sine, its spectrum (5 kHz centre,
    // 1 kHz/div, 20 dB/div, 0 dBV at row 70), the peak readout.
    {
        let base = &stock::scenes()[0];
        let mut m = base.m;
        m.math_on = true;
        m.math_mode = 5;
        m.math_colour = 1;
        m.math_off = -70;
        m.math_scale = scope_ui::Txt::new("20dB");
        m.fft_hz = scope_ui::Txt::new("5.00kHz");
        let peak_row = 70 + 3 * 35 / 20;
        m.fft = Some(scope_ui::FftAdd {
            ydiv: scope_ui::Txt::new("20dB/div"),
            xdiv: scope_ui::Txt::new("1kHz/div"),
            peak: scope_ui::Txt::new("1.00kHz -3.0dBV"),
            peak_at: Some((40, peak_row as i16)),
        });
        let sc = stock::Scene { name: "readme-fft", m, popup: scope_ui::Popup::None, trace: base.trace.clone(), persist: None, cols: 399, cells: base.cells.clone() };
        host::reset(0);
        scope_ui::invalidate();
        stock::load(&sc);
        // Spectrum rows (280-row model): noise floor ~ -100 dBV, the
        // fundamental at column 40, a -60 dBV third harmonic at 120.
        let mut s = 7u32;
        let (mut a, mut b) = (vec![], vec![]);
        for i in 0..400i32 {
            s = s.wrapping_mul(1664525).wrapping_add(1013904223);
            let mut r = 245 + ((s >> 16) % 9) as i32 - 4;
            for (c, top) in [(40, peak_row), (120, 70 + 60 * 35 / 20)] {
                let d = (i - c).abs();
                r = r.min(top + d * d * 6);
            }
            a.push(r);
            b.push(r);
        }
        let h = sc.m.frame_h();
        let g = scope_ui::graph();
        let spec = stock::Trace { a, b };
        for (i, (x, y)) in spec.columns(399).iter().enumerate().take(399) {
            g.ma[i] = (scope_ui::trace_fy(h, *x), scope_ui::trace_fy(h, *y));
        }
        scope_ui::update(&sc.m);
        scope_ui::cells(&sc.m, &sc.cells);
        ck.check_replica("readme/scope");
        save1(out, "scope");
    }

    // Generator: 1 kHz sine, 2 Vpp, running.
    {
        let mut g = gen::Gen::new();
        let mut s = g.s;
        s.on = true;
        s.wave = gen::Wave::Sine;
        s.freq_hz = 1000;
        s.amp_v = 2.0;
        g.apply(s);
        host::reset(0);
        ui::gen_static(&g);
        ui::gen_status(&status_real());
        ck.check_opts("readme/generator", &[("screen", Bx::new(0, 0, 480, 320)), ("bar", Bx::new(0, 0, 480, 36))], false);
        save1(out, "generator");
    }

    for (k, (n, s)) in &ck.seen {
        println!("VIOLATION x{n} (first {s}): {k}");
    }
    println!("readme: {} screens, {} violations", ck.cases, ck.violations);
    ck.violations
}
