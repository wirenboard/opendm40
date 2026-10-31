//! The screens of the user manual (docs/manual/img/*.png), 1x, with
//! ordinary values and the defaults of a new meter, not the worst cases of
//! the matrix. Synthetic data, drawn by the firmware's own code; each screen
//! also goes through the layout rules.
//!   tools/ui-render/run.sh OUT --manual   (then manual-img.py)
use super::*;

fn save1(out: &str, name: &str) {
    host::save_ppm(&format!("{out}/{name}.ppm"), 1);
}

/// An ordinary status bar: 00:12:34 since power-on, no REC, no BT host.
fn status_real() -> ui::Status {
    ui::Status { uptime_s: 12 * 60 + 34, bt: false, usb: false, bars: 4, charging: false, locked: false, rec: false, csv: false, rec_dim: false, page: 0 }
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

/// One multimeter page.
#[derive(Clone, Copy)]
struct Dp {
    r: Reading,
    b: Button,
    hist: bool,
    hold: u8,
    lim: u8,
    rel: Option<f32>,
    /// MIN/MAX/AVG session running (REC timer in the side column).
    rec: bool,
    /// CSV recording running (REC in the status bar).
    csv: bool,
    label: Option<&'static str>,
    ohm_on: bool,
    /// The rate applies (DC V / A / ohm), else the chip reads n/a.
    rate: bool,
    /// The beeper indicator (DmmView::short): 1 SHORT, 2 BEEP.
    short: u8,
}

fn dp(r: Reading) -> Dp {
    Dp { r, b: dmm::button_of(r.mode), hist: false, hold: 0, lim: meter::V_NONE, rel: None, rec: false, csv: false, label: None, ohm_on: false, rate: true, short: 0 }
}

/// The multimeter page: the reading, the bargraph or the history row, the
/// trend chart of the last minutes (the value with a small wobble).
fn dmm_page(p: &Dp) {
    let r = &p.r;
    host::reset(0);
    let mut dv = ui::DmmView::new();
    dv.bar_on = !p.hist;
    dv.label = p.label;
    // The accuracy label: none under REL (App::acc_ctx).
    dv.acc = p.rel.is_none().then_some(ui::Acc { model: 2, tc: p.label == Some("TC K") });
    let volts = r.mode <= 3 && !r.main.ol;
    dv.hv_red = volts && r.si.abs() >= 30.0;
    ui::set_cpl_auto(r.cpl_auto);
    dv.draw_static(p.b, r.mode, r.acdc, p.ohm_on);
    ui::set_cpl_auto(false);
    if p.hist {
        let v = r.main.v;
        let q = 10f32.powi(-(r.main.dec as i32));
        let items = [(v - 3.0 * q, r.main.dec, r.main.unit), (v - q, r.main.dec, r.main.unit), (v + 2.0 * q, r.main.dec, r.main.unit), (v, r.main.dec, r.main.unit), (v, r.main.dec, r.main.unit)];
        ui::history(&items, 4, 3);
    } else {
        dv.bar_update(r);
    }
    dv.short = p.short;
    dv.reading(r, p.hold, p.lim);
    dv.short_sync();
    let mut st = status_real();
    st.csv = p.csv;
    ui::status_right(&st);
    if volts && r.si.abs() >= 100.0 {
        ui::hv_warning(true);
    }
    let q = 10f32.powi(-(r.main.dec as i32));
    let v = if r.main.ol { 0.0 } else { r.main.v };
    let pts: Vec<f32> = (0..321).map(|i| v + (((i / 37) % 3) as f32 - 1.0) * q).collect();
    let secs = if p.rec { 5 * 60 + 12 } else { 12 * 60 + 30 };
    let side = ui::Side { max: Some(v + 3.0 * q), avg: Some(v), min: Some(v - 2.0 * q), sd: Some(1.5 * q), rel: p.rel, secs: Some(secs), rec: p.rec };
    let rc = if p.rate { ui::RateChip { rate: 1, period_ms: dmm::rate_period(dmm::Rate::Medium, r.mode), applies: true } } else { ui::RateChip { rate: 1, period_ms: 430, applies: false } };
    ui::chart(&mut ui::ChartCache::new(), &pts, r.main.dec, 2, Some(&side), r.main.unit, rc);
}

/// The base scope scene: a 1 kHz, 3 Vpp sine at 200 us/div, 1 V/div,
/// triggered, the timebase tabs and the CUR tab shown as at rest.
fn scope_base() -> stock::Scene {
    let base = &stock::scenes()[0];
    let mut m = base.m;
    m.tabs = scope_ui::tabs_of(scope_ui::TAP_CORNERS, m.tb, scope_ui::tb_top(true, false), false, None);
    m.cur_tab = scope_ui::CUR_OFF;
    m.src = scope_ui::SRC_MCX;
    let mut sc = stock::Scene { name: "manual-scope", m, popup: scope_ui::Popup::None, trace: None, persist: None, cols: 399, cells: base.cells.clone() };
    sc.trace = Some(trig_sine(&sc.m, 1.5, 2.0));
    sc
}

/// A sine of `amp` divisions and `periods` periods on the screen that
/// crosses the trigger level rising at the trigger position.
fn trig_sine(m: &scope_ui::Model, amp: f32, periods: f32) -> stock::Trace {
    let s = ((140 + m.pos as i32 - m.trig_row as i32) as f32 / (amp * 35.0)).clamp(-1.0, 1.0);
    let th = s.asin() / core::f32::consts::TAU;
    let phase = th / periods - m.trig_px as f32 / 400.0;
    stock::Trace::sine(amp, periods, phase).shifted(-(m.pos as i32))
}

fn popvals(m: &scope_ui::Model) -> scope_ui::PopVals {
    scope_ui::PopVals { pressed_cell: None, fft_win: 2, roll: true, ..stock::popvals(m) }
}

/// Draw a scope scene (with its popup values) and check it.
fn scope_shot(ck: &mut Checker, out: &str, name: &str, sc: &stock::Scene, pv: &scope_ui::PopVals) {
    host::reset(0);
    stock::draw_with(sc, pv);
    ck.check_replica(&format!("manual/{name}"));
}

/// The power meter page with an ordinary status bar and a calm chart.
fn pw_page(pv: dmm::pw::PowerVals) {
    ui::set_pw_parts(pw_parts(&pv));
    let r = pw_rd(pv);
    let mut dv = ui::DmmView::new();
    dv.power = true;
    dv.hv_red = pv.v.abs() >= 30.0;
    ui::set_power_coup(pv.coup);
    host::reset(0);
    dv.draw_static(Button::Power, r.mode, true, false);
    let mut pc = ui::pw::PwCache::new();
    ui::pw::side_clear(&mut pc);
    dv.reading(&r, 0, 0);
    ui::status_right(&status_real());
    let q = 10f32.powi(-(r.main.dec as i32));
    let pts: Vec<f32> = (0..321).map(|i| r.main.v + (((i / 29) % 3) as f32 - 1.0) * q).collect();
    ui::chart(&mut ui::ChartCache::new(), &pts, r.main.dec, 2, None, r.main.unit, ui::RateChip { rate: 1, period_ms: 430, applies: false });
    ui::pw::side(&mut pc, &pv);
    ui::set_pw_parts(false);
}

fn gen_page(s: gen::Settings, keypad: Option<(u8, &str, &str, &str)>) {
    let mut g = gen::Gen::new();
    g.apply(s);
    host::reset(0);
    ui::gen_static(&g);
    ui::gen_status(&status_real());
    if let Some((kind, title, input, unit)) = keypad {
        ui::keypad(kind, title, input, unit, true);
    }
}

pub fn run(out: &str) -> usize {
    std::fs::create_dir_all(out).unwrap();
    let mut ck = Checker { cases: 0, violations: 0, seen: Default::default() };
    let page = vec![("screen", Bx::new(0, 0, 480, 320)), ("bar", Bx::new(0, 0, 480, 40))];
    let screen = vec![("screen", Bx::new(0, 0, 480, 320))];

    // Settings pages with the defaults of a new meter.
    host::reset(0);
    ui::settings(0, &["5 min", "30 min", "55%", "50%", "ON", "Multimeter", "Auto (per view)", "Stock", ""]);
    ck.check_opts("manual/settings1", &page, false);
    save1(out, "settings1");
    host::reset(0);
    ui::settings(1, &["MED  5.0/s", "OFF", "Bargraph", "C (Celsius)", "Off (stock)", "ON", "Corners", "", ""]);
    ck.check_opts("manual/settings2", &page, false);
    save1(out, "settings2");
    host::reset(0);
    ui::settings(2, &["ON", "Loud", ui::usb_disk_name(true, true), "OFF (stock)", "", "", "ON", "OFF (stock)", "OFF"]);
    ck.check_opts("manual/settings3", &page, false);
    save1(out, "settings3");

    // Voltage range page: AUTO+ selected, DC, the AUTO+ note, Wave.
    host::reset(0);
    ui::range_page(Button::Volt, Some(6), 0, Some(("AUTO+ recommended: never steps to mV", false)));
    ui::wave_button();
    ck.check_opts("manual/range-v", &page, false);
    save1(out, "range-v");

    // Current range page: AUTO, DC; Wave, Fuse check and Inrush.
    host::reset(0);
    ui::range_page(Button::Amp, Some(7), 0, None);
    ui::wave_button();
    ui::inrush_button();
    ck.check_opts("manual/range-a", &page, false);
    save1(out, "range-a");

    // Resistance range page with the 0.28 mA ("ON") variant on.
    host::reset(0);
    ui::range_page(Button::Ohm, Some(7), 1, None);
    ck.check_opts("manual/range-ohm", &page, false);
    save1(out, "range-ohm");

    // The experimental 600 Mohm chosen on the range page.
    host::reset(0);
    ui::range_page(Button::Ohm, Some(8), 0, None);
    ck.check_opts("manual/range-ohm-600m", &page, false);
    save1(out, "range-ohm-600m");

    // Hold & Limits of DC V: limits 4.8 .. 5.2 V on.
    host::reset(0);
    ui::list_page("Hold & Limits: DC V", &ui::LIMIT_ROWS, &["OFF", "ON", "5.2000 V", "4.8000 V", "", "", "", "ON", "Every reading"]);
    ck.check_opts("manual/limits", &page, false);
    save1(out, "limits");

    // Record page: on change, until stopped; and the fast recorder.
    host::reset(0);
    ui::record_page(["On change", "1 %", "Until stopped"], "about 240 h left", false);
    ck.check_opts("manual/record", &page, false);
    save1(out, "record");
    host::reset(0);
    ui::record_page(["Fast", "1 %", "1 h"], "4800/s, 25 min left", false);
    ck.check_opts("manual/record-fast", &page, false);
    save1(out, "record-fast");

    // Temperature page: thermocouple K, no offset.
    host::reset(0);
    ui::temp_page(&ui::TempPage { sensor: 0, offs: 0.0, unit_f: false, r25: 10_000.0, beta: 3950.0, info: "Cold junction 24.6 C" });
    ck.check_opts("manual/temp", &page, false);
    save1(out, "temp");

    // Device page (no serial number on it).
    host::reset(0);
    let mut fwb = [0u8; 48];
    let ver = ui::device_software(&mut fwb);
    ui::device_page(&[
        ("Vendor", "Alientek"),
        ("Web", "www.alientek.com"),
        ("Model", "DM40A"),
        ("Bootloader", "V1.1"),
        ("Software", ver),
        ("Hardware", "V4"),
        ("Board temp", "31.5 C (MCU)"),
    ]);
    ck.check_opts("manual/device", &page, false);
    save1(out, "device");

    // Touch test page: taps on the targets (a scale error), MENU held.
    host::reset(0);
    super::touchtest_scene(2, false);
    ck.check_opts("manual/touch-test", &[("screen", Bx::new(0, 0, 480, 320))], false);
    save1(out, "touch-test");

    // Touch calibration: the walk, the result of a good fit.
    for (k, name) in [(0u8, "touch-cal"), (1, "touch-cal-result")] {
        host::reset(0);
        super::touchcal_scene(k, false);
        ck.check_opts(&format!("manual/{name}"), &[("screen", Bx::new(0, 0, 480, 320))], false);
        save1(out, name);
    }

    // On-screen help, page 1 (gestures).
    host::reset(0);
    ui::help_page(0);
    ck.check_opts("manual/help", &page, false);
    save1(out, "help");

    // Fuse check passed.
    {
        host::reset(0);
        let mut b = [0u8; 32];
        ui::fuse_screen(2, sentinel::FuseRes::Ok(48.12), &mut b);
        save1(out, "fuse-check");
    }

    // Power meter: wiring picture (default limit 6 A / 30 s) and Power
    // Settings in its idle state.
    {
        let pcs = pw_cases();
        let rc = ui::RateChip { rate: 1, period_ms: 430, applies: false };
        host::reset(0);
        power_page(pcs[0].1, 0, sentinel::Alert::None, rc);
        ui::status_right(&status_real());
        ui::pw::entry_dialog(dmm::pw::I_CONT_DEFAULT, dmm::pw::T_LIM_DEFAULT);
        save1(out, "power-wiring");
        host::reset(0);
        let mut p = dmm::pw::Pw::new();
        p.vr = 3;
        p.job_vr = 3;
        p.coup = pcs[0].1.coup;
        ui::pw::settings_page(&dmm::pw::PowerVals { v_auto: true, i_auto: true, comp: false, ..pcs[0].1 });
        ui::pw::settings_status(&p, 0, ui::pw::HELP_COUP);
        save1(out, "power-settings");

        // DC: a 12 V supply into a 2 A load, energy counting; the main
        // value stepped to Wh; the current-limit countdown.
        use dmm::pw::PowerVals as P;
        let z = P { coup: dmm::pw::COUP_DC, dc: true, ..P::ZERO };
        let dc = P { p: 24.012, s: 24.012, pf: 1.0, v: 12.006, i: 2.0, vdc: 12.006, idc: 2.0, f_hz: f32::NAN, burden_v: 0.02, e_wh: 1.2506, vah: 1.2506, secs: 187, p_avg: 24.075, vr: 1, ik: 4, vp: [12.006, 0.0], ip: [2.0, 0.0], pp: [24.012, 0.0], ..z };
        for (name, pv) in [
            ("power-dc", dc),
            ("power-wh", P { main: dmm::pw::MAIN_WH, ..dc }),
            ("power-limit", P { p: 81.67, s: 81.67, v: 12.01, i: 6.8, vdc: 12.01, idc: 6.8, burden_v: 0.068, over: 1, over_left: 23, ..dc }),
        ] {
            pw_page(pv);
            ck.check(&format!("manual/{name}"), &pw_regions(&pv));
            save1(out, name);
        }
    }

    // Inrush current page, armed.
    {
        use ui::inr::{Vals, C_ARMED};
        host::reset(0);
        let v = Vals { thr: "+1.0 A", rms: "", peak: "", info: "Armed: 0.123 A now", info_fg: C_ARMED, idle: "", file: "", btn: "Cancel", btn_bg: 0xF800, enabled: true };
        ui::inr::page(&v);
        ui::inr::trace(None, 0.0, 125.0f32 / 262_144.0 * 8.0);
        ck.check_opts("manual/inrush-armed", &page, false);
        save1(out, "inrush-armed");
    }

    // Multimeter functions, ordinary values.
    let c = "\u{b0}C";
    let ohm = "\u{3a9}";
    let dcv = rd(1, val(4.8176, 4, "V"), None, None, false);
    let fns: Vec<(&str, Dp)> = vec![
        // AC V on the mains: red from 30 V, the lightning from 100 V.
        ("dmm-acv", Dp { rate: false, ..dp(rd(0, val(230.12, 2, "V"), Some(val(50.000, 3, "Hz")), Some(val(50.0, 1, "%")), false)) }),
        // AC+DC: 12 V with 0.5 V ripple.
        ("dmm-acdc", Dp { rate: false, ..dp(rd(1, val(12.011, 3, "V"), Some(val(0.5021, 4, "V")), Some(val(12.001, 3, "V")), true)) }),
        // DC mA in a 4-20 mA loop.
        ("dmm-ma", dp(rd(8, val(12.000, 3, "mA"), None, Some(val(50.0, 1, "%")), false))),
        // Resistance, 4.7 k (the 6 kohm range: test current 197 uA).
        ("dmm-ohm", dp(Reading { range: 2, ..rd(10, val(4.7012, 4, "k\u{3a9}"), None, None, false) })),
        // The experimental 600 Mohm range (manual only): 412.3 Mohm.
        ("dmm-ohm-600m", Dp { rate: false, ..dp(Reading { range: dmm::OHM600, auto: false, ..rd(10, val(412.3, 1, "M\u{3a9}"), None, None, false) }) }),
        // Continuity: a short, the beeper indicator on.
        ("dmm-cont", Dp { rate: false, short: 1, ..dp(rd(11, val(0.42, 2, ohm), None, None, false)) }),
        // Diode: a silicon diode.
        ("dmm-diode", Dp { rate: false, ..dp(rd(12, val(0.5873, 4, "V"), None, None, false)) }),
        // Capacitance: 100 nF.
        ("dmm-cap", Dp { rate: false, ..dp(rd(13, val(98.73, 2, "nF"), None, None, false)) }),
        // Frequency: 1 kHz, 50 %, with the small digit (`freq_ext`).
        ("dmm-hz", Dp { rate: false, ..dp(rd(14, Val { ext: true, ..val(1.00000, 4, "kHz") }, None, Some(val(50.0, 1, "%")), false)) }),
        // Temperature: thermocouple K.
        ("dmm-temp", Dp { rate: false, label: Some("TC K"), ..dp(rd(15, val(86.4, 1, c), Some(val(24.6, 1, c)), None, false)) }),
        // The bargraph (default DMM row), DC V.
        ("dmm-bar", dp(dcv)),
        // The accuracy label and the digits below it in grey: DC V 6 V
        // range, the 6 and the extra digit.
        ("dmm-acc", dp(rd(1, Val { ext: true, ..val(4.81763, 4, "V") }, None, None, false))),
        // HOLD.
        ("dmm-hold", Dp { hold: 1, ..dp(dcv) }),
        // AutoHold with a captured value.
        ("dmm-ahold", Dp { hold: 3, ..dp(dcv) }),
        // REL: 4.8176 V as the zero, 4.8203 V now.
        ("dmm-rel", Dp { rel: Some(4.8176), ..dp(rd(1, val(0.0027, 4, "V"), None, None, false)) }),
        // A MIN/MAX/AVG session running.
        ("dmm-minmax", Dp { rec: true, ..dp(dcv) }),
        // Limits 4.8 .. 5.2 V, the reading above.
        ("dmm-limit-hi", Dp { lim: meter::V_HI, ..dp(rd(1, val(5.2871, 4, "V"), None, None, false)) }),
        // The history row.
        ("dmm-history", Dp { hist: true, ..dp(dcv) }),
        // Over range.
        ("dmm-ol", dp(rd(1, ol(4, "V"), None, None, false))),
        // Continuity with open leads.
        ("dmm-open", Dp { rate: false, ..dp(rd(11, ol(2, ohm), None, None, false)) }),
        // A recording running (REC in the status bar).
        ("dmm-rec", Dp { csv: true, ..dp(dcv) }),
    ];
    for (name, p) in &fns {
        dmm_page(p);
        ck.check(&format!("manual/{name}"), &DMM_REGIONS);
        save1(out, name);
    }

    // Alerts: LIVE? in ohms (an external voltage), OVLD on a fixed mA
    // range (the meter moved to the 10 A shunt), with its message.
    {
        let mut r = rd(10, val(-12.43, 2, ohm), None, None, false);
        r.alert = sentinel::Alert::Live;
        dmm_page(&dp(r));
        ck.check("manual/alert-live", &DMM_REGIONS);
        save1(out, "alert-live");
        let mut r = rd(7, val(1.2345, 4, "A"), None, None, false);
        r.auto = false;
        r.alert = sentinel::Alert::Overload;
        dmm_page(&dp(r));
        dialog::toast(sentinel::Alert::Overload.toast(), false);
        ck.check("manual/alert-ovld", &DMM_REGIONS);
        save1(out, "alert-ovld");
    }

    // Messages over the multimeter page: the rate step, the start of a
    // recording, the power-off question, the firmware install question.
    // The rate toast is the firmware's text for a step to MED on this range.
    let mut tb = [0u8; 40];
    let (n, ext) = dmm::rate_window(dmm::Rate::Medium, false, dcv.mode, dcv.range);
    let rate_msg = ui::rate_toast(&mut tb, dmm::Rate::Medium as u8, dmm::rate_period(dmm::Rate::Medium, dcv.mode), n, ext);
    for (name, msg, ok) in [("dmm-rate", rate_msg, true), ("rec-start", "Recording CSV", true)] {
        dmm_page(&Dp { csv: name == "rec-start", ..dp(dcv) });
        dialog::toast(msg, ok);
        ck.check(&format!("manual/{name}"), &DMM_REGIONS);
        save1(out, name);
    }
    for (name, f) in [("power-off", dialog::power_off as fn()), ("confirm-install", dialog::fw_install)] {
        dmm_page(&dp(dcv));
        f();
        let (x, y, w2, h2) = dialog::BOX;
        ck.check_area(&format!("manual/{name}"), &[("screen", Bx::new(0, 0, 480, 320)), ("dialog", Bx::new(x as i32, y as i32, w2 as i32, h2 as i32))], false, Some(Bx::new(x as i32, y as i32, w2 as i32, h2 as i32)));
        save1(out, name);
    }

    // The fast recorder's DATA LOGGING panel (AC V, 4800/s, to the disk).
    {
        dmm_page(&Dp { csv: true, rate: false, ..dp(rd(0, val(230.12, 2, "V"), Some(val(50.000, 3, "Hz")), Some(val(50.0, 1, "%")), false)) });
        dialog::log_panel_reset();
        let info = dialog::LogInfo { hz: 4800, n: 1_152_000, nominal_hz: 4800, ad2: true, lost: 0, disk: Some(dialog::LogDisk { prep: false, pct: 100, file: 3, kb: 6750 }) };
        for i in 0..5 {
            dialog::log_panel(i, info);
        }
        let pb = Bx::new(40, 64, 400, 146);
        ck.check_area("manual/fast-panel", &[("screen", Bx::new(0, 0, 480, 320)), ("panel", pb)], true, Some(pb));
        save1(out, "fast-panel");
        dialog::log_panel_reset();
    }

    // Generator: every waveform, the 50 kHz clamp, a DC output, the keypad.
    {
        let g = |on, wave, freq_hz, amp_v, duty, offset_v| gen::Settings { on, wave, freq_hz, amp_v, duty, offset_v };
        let gen_regions = vec![("screen", Bx::new(0, 0, 480, 320)), ("bar", Bx::new(0, 0, 480, 36))];
        for (name, s, kp) in [
            ("gen-square", g(true, gen::Wave::Square, 1_000, 3.0, 2500, None), None),
            ("gen-triangle", g(true, gen::Wave::Triangle, 100, 2.0, 5000, Some(1.5)), None),
            ("gen-ramp", g(true, gen::Wave::Ramp, 10_000, 3.0, 5000, None), None),
            ("gen-clamped", g(true, gen::Wave::Sine, 75_000, 2.0, 5000, None), None),
            ("gen-keypad", g(false, gen::Wave::Sine, 1_000, 2.0, 5000, None), Some((ui::KP_FREQ, "Frequency", "1", "KHz"))),
            ("gen-keypad-vpp", g(false, gen::Wave::Sine, 1_000, 2.0, 5000, None), Some((ui::KP_NUM, "Vpp", "2.00", "V"))),
        ] {
            gen_page(s, kp);
            if kp.is_none() {
                ck.check_opts(&format!("manual/{name}"), &gen_regions, false);
            } else {
                ck.check_replica(&format!("manual/{name}"));
            }
            save1(out, name);
        }
    }

    // Wave view: mains on the 600 V range (its full-scale step, 150 V/div), triggered at 10 V,
    // at 10 ms/div (5 periods), 50 ms/div and 2 ms/div; a 1.5 A current.
    {
        let mut m = scope_base().m;
        m.src = scope_ui::SRC_DMM;
        m.wvdiv = 150.0;
        m.ch1_txt = "AC V";
        m.ac = true;
        m.trig_v = 10.0;
        m.trig_row = 138;
        for (name, tb, periods) in [("wave", 18u8, 5.0f32), ("wave-50ms", 20, 25.0), ("wave-2ms", 16, 1.0)] {
            let mut m = m;
            m.tb = tb;
            m.tabs = scope_ui::tabs_of(scope_ui::TAP_CORNERS, tb, scope_ui::tb_top(true, false), false, None);
            let cells = vec!["Freq:50.0Hz", "Vp-p:651V", "Cycl:20.0ms", "Vrms:230V"];
            let s2 = stock::Scene { name: "manual-wave", m, popup: scope_ui::Popup::None, trace: Some(trig_sine(&m, 325.0 / 150.0, periods)), persist: None, cols: 399, cells };
            scope_shot(&mut ck, out, name, &s2, &popvals(&m));
            save1(out, name);
        }
        // A current: 1.5 A AC on the 6 A range (its full-scale step, 1.5 A/div), 10 ms/div.
        m.tb = 18;
        m.tabs = scope_ui::tabs_of(scope_ui::TAP_CORNERS, 18, scope_ui::tb_top(true, false), false, None);
        m.amps = true;
        m.ac = false;
        m.wvdiv = 1.5;
        m.ch1_txt = "AC A";
        m.trig_v = 0.1;
        m.trig_row = 140;
        let cells = vec!["Freq:50.0Hz", "Ip-p:4.24A", "Cycl:20.0ms", "Irms:1.50A"];
        let s2 = stock::Scene { name: "manual-wave", m, popup: scope_ui::Popup::None, trace: Some(trig_sine(&m, 2.121 / 1.5, 5.0)), persist: None, cols: 399, cells };
        scope_shot(&mut ck, out, "wave-amps", &s2, &popvals(&m));
        save1(out, "wave-amps");
    }

    // ---- Oscilloscope (CHA), 1 kHz 3 Vpp sine unless stated.
    let sb = scope_base();
    let pv = popvals(&sb.m);
    // The run view.
    scope_shot(&mut ck, out, "scope-run", &sb, &pv);
    save1(out, "scope-run");
    // Every scope state below: (name, model change, popup, popup values
    // change, trace, cells).
    {
        let mut shots: Vec<(&str, stock::Scene, scope_ui::PopVals)> = vec![];
        let with = |f: &dyn Fn(&mut stock::Scene, &mut scope_ui::PopVals)| {
            let mut s = scope_base();
            s.cells = sb.cells.clone();
            let mut p = popvals(&s.m);
            f(&mut s, &mut p);
            (s, p)
        };
        let mut add = |name: &'static str, f: &dyn Fn(&mut stock::Scene, &mut scope_ui::PopVals)| {
            let (s, p) = with(f);
            shots.push((name, s, p));
        };
        // Trigger level being dragged: the T marker on the right edge
        // selected, the level line shown.
        add("scope-trig-level", &|s, _| {
            s.m.sel_v = 1;
            s.m.guide = true;
            s.m.trig_row = 70;
            s.m.trig_v = 1.99;
            s.trace = Some(trig_sine(&s.m, 1.5, 2.0));
        });
        // Trigger position being dragged left.
        add("scope-trig-pos", &|s, _| {
            s.m.sel_h = 0;
            s.m.guide = true;
            s.m.trig_px = 110;
            s.trace = Some(trig_sine(&s.m, 1.5, 2.0));
        });
        // Trigger tab: edge, pulse, runt (with the second level marker).
        add("scope-trig-tab", &|s, _| s.popup = scope_ui::Popup::Mf(2));
        add("scope-trig-pulse", &|s, p| {
            s.popup = scope_ui::Popup::Mf(2);
            p.ttype = 1;
            p.pcond = 0;
            p.pwidth = 400e-6;
        });
        add("scope-trig-runt", &|s, p| {
            s.m.runt_row = Some(60);
            s.m.sel_v = 5;
            s.m.guide = true;
            p.ttype = 2;
        });
        // Timebase: the list, a corner tap (the next slower step), the
        // result.
        add("scope-tb-list", &|s, _| s.popup = scope_ui::Popup::Tb(s.m.tb));
        add("scope-tb-corner", &|s, _| {
            s.m.tb = 14;
            s.m.tb_flash = true;
            s.m.tabs = scope_ui::tabs_of(scope_ui::TAP_CORNERS, 14, scope_ui::tb_top(true, false), false, Some(true));
            s.trace = Some(trig_sine(&s.m, 1.5, 5.0));
        });
        // V/div zoomed in: 500 mV/div.
        add("scope-vdiv", &|s, _| {
            s.m.vdiv = 8;
            s.m.trig_row = 70;
            s.trace = Some(trig_sine(&s.m, 3.0, 2.0));
        });
        // Channel popup: coupling AC.
        add("scope-ch1", &|s, p| {
            s.m.ch1_open = true;
            s.m.ac = true;
            p.ac = true;
            s.popup = scope_ui::Popup::Ch1(0);
        });
        // Cursors X and Y.
        add("scope-cur-x", &|s, _| {
            s.m.cur_tab = scope_ui::CUR_X;
            s.m.xcur = Some([120, 280]);
            s.m.sel_h = 2;
            s.cells = vec!["X2-1:800us", "Xf:1.25kHz", "Cycl:1.00ms", "Vrms:1.06V"];
        });
        add("scope-cur-y", &|s, _| {
            s.m.cur_tab = scope_ui::CUR_Y;
            s.m.ycur = Some([87, 193]);
            s.m.sel_v = 3;
            s.cells = vec!["Y2-1:3.01V", "Vp-p:3.00V", "Cycl:1.00ms", "Vrms:1.06V"];
        });
        // The scope menu: Measure, Display, Settings tabs.
        add("scope-measure", &|s, _| s.popup = scope_ui::Popup::Mf(0));
        add("scope-display", &|s, _| s.popup = scope_ui::Popup::Mf(1));
        add("scope-settings", &|s, _| s.popup = scope_ui::Popup::Mf(3));
        add("scope-decode-tab", &|s, p| {
            s.popup = scope_ui::Popup::Mf(4);
            p.dec.proto = 1;
            p.dec.level = 1.65;
        });

        // Math popup over the FFT: Other (window, unit).
        add("scope-math", &|s, p| {
            s.m.math_open = true;
            s.m.math_on = true;
            s.m.math_mode = 5;
            s.m.math_scale = scope_ui::Txt::new("20dB");
            s.m.fft_hz = scope_ui::Txt::new("5.00kHz");
            p.fft_hz = 2;
            s.popup = scope_ui::Popup::Math(3);
        });
        // Stopped, zoomed into the stored capture (100 us/div).
        add("scope-stop", &|s, _| {
            s.m.status = scope_ui::ST_STOP;
            s.m.record = (0.45, 0.55);
            s.m.tb = 12;
            s.m.tabs = scope_ui::tabs_of(scope_ui::TAP_CORNERS, 12, scope_ui::tb_top(true, false), false, None);
            s.trace = Some(trig_sine(&s.m, 1.5, 1.0));
        });
        // Roll mode: 500 ms/div, a slow signal.
        add("scope-roll", &|s, _| {
            s.m.status = scope_ui::ST_RUN;
            s.m.roll = true;
            s.m.tb = 23;
            s.m.tabs = scope_ui::tabs_of(scope_ui::TAP_CORNERS, 23, scope_ui::tb_top(true, false), false, None);
            s.m.trig_row = 139;
            s.m.trig_v = 0.02;
            s.trace = Some(stock::Trace::sine(1.0, 1.5, 0.1));
            s.cols = 260;
            s.cells = vec!["Freq:300mHz", "Vp-p:2.00V", "Cycl:3.33s", "Vrms:707mV"];
        });
        // After auto-set on a 1 kHz square: 1 V/div, 4 periods.
        add("scope-autoset", &|s, _| {
            s.m.trig_row = 139;
            s.m.trig_v = 0.02;
            s.m.tb = 14;
            s.m.tabs = scope_ui::tabs_of(scope_ui::TAP_CORNERS, 14, scope_ui::tb_top(true, false), false, None);
            let mut t = stock::Trace::square(1.5, 5.0, 0.5);
            // Rising edge at the trigger position (column 186).
            let k = 186 - 14;
            t.a.rotate_right(400 - (k % 80));
            t.b.rotate_right(400 - (k % 80));
            s.trace = Some(t);
            s.cells = vec!["Freq:1.00kHz", "Vp-p:3.00V", "Cycl:1.00ms", "Vrms:1.50V"];
        });
        drop(add);
        for (name, s, p) in &shots {
            scope_shot(&mut ck, out, name, s, p);
            save1(out, name);
        }
    }
    // Auto-set with no signal: the message over a flat trace.
    {
        let mut s = scope_base();
        s.trace = Some(stock::Trace::noise(140, 1, 5));
        s.cells = vec!["Freq:--", "Vp-p:20.0mV", "Cycl:--", "Vrms:4.10mV"];
        host::reset(0);
        stock::draw_with(&s, &pv);
        dialog::toast("AUTO: no signal found", false);
        ck.check_replica("manual/scope-nosignal");
        save1(out, "scope-nosignal");
        // A screenshot saved, over the run view.
        host::reset(0);
        stock::draw_with(&sb, &pv);
        dialog::toast("Saved S012.BMP", true);
        ck.check_replica("manual/screenshot");
        save1(out, "screenshot");
    }

    // The protocol decoder's band: UART, Modbus RTU (stopped, a CRC error).
    decode_ui::manual(&mut ck, out, &save1);

    // Combined view with the function band open (AC V on the mains).
    {
        let mut m = sb.m;
        m.combined = true;
        m.src = scope_ui::SRC_NONE;
        let sc = stock::Scene { name: "manual-combined", m, popup: scope_ui::Popup::None, trace: sb.trace.clone(), persist: None, cols: 399, cells: sb.cells.clone() };
        host::reset(0);
        scope_ui::invalidate();
        scope_ui::strip_invalidate();
        stock::load(&sc);
        scope_ui::update(&sc.m);
        scope_ui::cells(&sc.m, &sc.cells);
        let acv = rd(0, val(230.12, 2, "V"), Some(val(50.000, 3, "Hz")), Some(val(50.0, 1, "%")), false);
        let d = ui::strip_data(&acv, 0, 0, false, 1, false, None);
        scope_ui::meter_strip(&d);
        ui::func_band(Button::Volt, 0, false, false);
        ck.check_area("manual/combined-band", &screen, true, Some(Bx::new(13, 20, 401, 56)));
        save1(out, "combined-band");
    }

    // More multimeter states: the status bar with the lock closed and a
    // Bluetooth host; the extra digit (MED, settled); the auto power-off
    // warning; A-JACK with its message; AUTO on volts (the warning).
    {
        dmm_page(&Dp { csv: true, ..dp(dcv) });
        ui::status_right(&ui::Status { bt: true, csv: true, ..status_real() });
        ck.check("manual/status-all", &DMM_REGIONS);
        save1(out, "status-all");
        // The status-bar picture: the same with a USB host attached.
        dmm_page(&Dp { csv: true, ..dp(dcv) });
        ui::status_right(&ui::Status { bt: true, csv: true, usb: true, ..status_real() });
        ck.check("manual/status-usb", &DMM_REGIONS);
        save1(out, "status-usb");
        dmm_page(&dp(dcv));
        ui::status_right(&ui::Status { locked: true, bt: true, ..status_real() });
        ck.check("manual/status-locked", &DMM_REGIONS);
        save1(out, "status-locked");
        let mut r = dcv;
        r.main = Val { v: 4.81762, dec: 4, unit: "V", ol: false, ext: true };
        dmm_page(&dp(r));
        ck.check("manual/dmm-extra", &DMM_REGIONS);
        save1(out, "dmm-extra");
        dmm_page(&dp(dcv));
        dialog::toast("Auto power-off in 30 s", false);
        ck.check("manual/auto-off", &DMM_REGIONS);
        save1(out, "auto-off");
        let mut r = dcv;
        r.alert = sentinel::Alert::AJack;
        dmm_page(&dp(r));
        dialog::toast(sentinel::Alert::AJack.toast(), false);
        ck.check("manual/danger", &DMM_REGIONS);
        save1(out, "danger");
        host::reset(0);
        ui::range_page(Button::Volt, Some(7), 0, Some((dmm::MV_AUTO_WARN, true)));
        ui::wave_button();
        ck.check_opts("manual/range-v-auto", &page, false);
        save1(out, "range-v-auto");
        // Settings 2 with smoothing LOW; Settings 3 with PC can edit asked
        // for (re-plug pending).
        host::reset(0);
        ui::settings(1, &["MED  5.0/s", "LOW", "Bargraph", "C (Celsius)", "Off (stock)", "ON", "Corners", "", ""]);
        ck.check_opts("manual/settings2-smooth", &page, false);
        save1(out, "settings2-smooth");
        host::reset(0);
        ui::settings(2, &["ON", "Loud", ui::usb_disk_name(true, false), "OFF (stock)", "", "", "ON", "OFF (stock)", "OFF"]);
        ck.check_opts("manual/settings3-edit", &page, false);
        save1(out, "settings3-edit");
    }

    // Every alert badge (A-JACK, LIVE?, OVLD are above).
    {
        let f = "\u{b0}F";
        for (name, r, alert) in [
            ("alert-aprot", rd(1, val(0.0213, 4, "V"), None, None, false), sentinel::Alert::AProt),
            ("alert-vjack", rd(8, val(0.000, 3, "mA"), None, None, false), sentinel::Alert::VJack),
            ("alert-dc", rd(0, val(0.0021, 4, "V"), None, None, false), sentinel::Alert::Dc),
            ("alert-ac", rd(1, val(0.0123, 4, "V"), None, None, false), sentinel::Alert::Ac),
            ("alert-fuse", rd(7, val(0.0000, 4, "A"), None, None, false), sentinel::Alert::Fuse),
        ] {
            let mut r = r;
            r.alert = alert;
            dmm_page(&Dp { rate: r.mode != 0, ..dp(r) });
            ck.check(&format!("manual/{name}"), &DMM_REGIONS);
            save1(out, name);
        }
        // 100.3 C in F (the stock shows 214.5 F).
        dmm_page(&Dp { rate: false, label: Some("TC K"), ..dp(rd(15, val(212.5, 1, f), Some(val(77.0, 1, f)), None, false)) });
        ck.check("manual/dmm-temp-f", &DMM_REGIONS);
        save1(out, "dmm-temp-f");
    }

    // Temperature with a user NTC (R25 4.7 k, Beta 3977): the sensor page,
    // the R25 and Beta keypads over it, the reading.
    {
        let tp = ui::TempPage { sensor: 6, offs: 0.0, unit_f: false, r25: 4_700.0, beta: 3977.0, info: "" };
        host::reset(0);
        ui::temp_page(&tp);
        ck.check_opts("manual/temp-ntc", &page, false);
        save1(out, "temp-ntc");
        for (name, title, input, unit) in [("temp-r25", "R25", "4.7", "k"), ("temp-beta", "Beta", "3977", "K")] {
            host::reset(0);
            ui::temp_page(&tp);
            ui::keypad(ui::KP_NUM, title, input, unit, true);
            ck.check_replica(&format!("manual/{name}"));
            save1(out, name);
        }
        let c = "\u{b0}C";
        dmm_page(&Dp { rate: false, label: Some("NTC user"), ..dp(rd(15, val(24.86, 2, c), Some(val(4.721, 3, "k\u{3a9}")), None, false)) });
        ck.check("manual/dmm-ntc", &DMM_REGIONS);
        save1(out, "dmm-ntc");
    }

    // Power meter: the current range being found; AC on the mains; the
    // energy reset question.
    {
        host::reset(0);
        let mut dv = ui::DmmView::new();
        dv.power = true;
        ui::set_power_coup(dmm::pw::COUP_DC);
        dv.draw_static(Button::Power, 4, true, false);
        ui::pw::ranging(Some(dmm::Val { v: 12.006, dec: 3, unit: "V", ol: false, ext: false }), "I ranging...", 0);
        ui::status_right(&status_real());
        ck.check_opts("manual/power-ranging", &screen, false);
        save1(out, "power-ranging");
        let ac = pw_cases().into_iter().find(|c| c.0 == "ac").unwrap().1;
        let ac = dmm::pw::PowerVals { comp: false, ..ac };
        pw_page(ac);
        ck.check("manual/power-ac", &pw_regions(&ac));
        save1(out, "power-ac");
        pw_page(ac);
        dialog::energy_reset();
        let (x, y, w2, h2) = dialog::BOX;
        let bx = Bx::new(x as i32, y as i32, w2 as i32, h2 as i32);
        ck.check_area("manual/energy-reset", &[("screen", Bx::new(0, 0, 480, 320)), ("dialog", bx)], false, Some(bx));
        save1(out, "energy-reset");
    }

    for (k, (n, s)) in &ck.seen {
        println!("VIOLATION x{n} (first {s}): {k}");
    }
    println!("manual: {} screens, {} violations", ck.cases, ck.violations);
    ck.violations
}
