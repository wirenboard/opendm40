//! opendm40 additions on the stock-replica scope page that are not in the
//! stock (src/scope_ui.rs): the FFT axis-scale boxes, the peak readout and
//! marker (UF-11), the trigger-type rows of the Trigger tab (UF-13), the
//! sample rate beside each timebase list entry, the
//! cursor tab (UF-08) and the zoom buttons (UF-10). Every item under the
//! full layout rules (`check_add`) against the replica around it, both
//! views, worst-case texts, and updates equal a fresh draw.
use super::*;

/// The FFT additions with worst-case (widest) texts.
fn fft_add(peak: Option<(i16, i16)>, k: usize) -> scope_ui::FftAdd {
    let t = scope_ui::Txt::new;
    let (y, x, p) = [
        ("10dB/div", "50kHz/div", "1.00kHz -3.0dBV"),
        ("500mdB/div", "200mHz/div", "12.5MHz -99.9dBV"),
        ("200mV/div", "500kHz/div", "12.5MHz 999mV"),
        ("100dB/div", "20MHz/div", "--"),
    ][k % 4];
    scope_ui::FftAdd { ydiv: t(y), xdiv: t(x), peak: t(p), peak_at: peak }
}

fn rb(r: stockfont::Rect) -> Bx {
    Bx::new(r.x as i32, r.y as i32, r.w as i32, r.h as i32)
}

/// Each timebase list entry's sample rate (scope_ui::tb_rate_rect, beside
/// the time; main.rs "Timebase list popup" runs the full layout rules on
/// every centre index): the widest rate fits its box with the stock 3 px
/// margin, MCX and the Wave view; the contact sheet's / emu/sratetest.py's
/// scenes; the record bar while running without a rate.
fn srate(ck: &mut Checker, out: &str) {
    let base = &stock::scenes()[0];
    let mut widest = (0, String::new());
    for wave in [false, true] {
        let lo = if wave { scope::WAVE_TB_MIN } else { scope::TB_MIN };
        for tb in lo..=scope::TB_MAX {
            let mut b = scope_ui::Buf::new();
            scope_ui::fmt_srate(&mut b, scope::tb_rate(tb, wave));
            let w = stockfont::text_width(stockfont::SC_F12, b.as_str(), 1);
            // >= 3 px from the spinner's right edge.
            let lim = scope_ui::TB_SPIN.x + scope_ui::TB_SPIN.w - 3 - scope_ui::tb_rate_rect(0).x;
            ck.cases += 1;
            if w > lim {
                ck.fail(&format!("srate/{}/{tb}", wave as u8), format!("rate \"{}\" {w} px > {lim}", b.as_str()));
            }
            if w > widest.0 {
                widest = (w, b.as_str().to_string());
            }
        }
    }
    let shots: [(&str, bool, u8); 7] = [
        ("s07-srate-tb5", false, 5),
        ("s07-srate-tb13", false, 13),
        ("s07-srate-tb16", false, 16),
        ("s07-srate-tb20", false, 20),
        ("s07-srate-roll", false, 26),
        ("s07-srate-wave", true, 15),
        ("s07-srate-wave-roll", true, 24),
    ];
    for (name, wave, tb) in shots {
        let mut m = base.m;
        m.tb = tb;
        m.src = if wave { scope_ui::SRC_DMM } else { scope_ui::SRC_MCX };
        if wave {
            m.wvdiv = 250.0;
            m.ch1_txt = "AC V";
            m.ac = true;
            scope_ui::set_tb_lo(scope::WAVE_TB_MIN);
        }
        let sc = stock::Scene { name: "srate", m, popup: scope_ui::Popup::Tb(tb), trace: base.trace.clone(), persist: None, cols: 399, cells: base.cells.clone() };
        stock::draw(&sc);
        ck.check_replica(&format!("srate/{name}"));
        save(out, name);
        scope_ui::set_tb_lo(scope::TB_MIN);
    }
    // The record bar while running: no rate (the stock full-record line).
    let mut m = base.m;
    m.src = scope_ui::SRC_MCX;
    let sc = stock::Scene { name: "srate", m, popup: scope_ui::Popup::None, trace: base.trace.clone(), persist: None, cols: 399, cells: base.cells.clone() };
    stock::draw(&sc);
    ck.check_replica("srate/recbar");
    save(out, "s07-srate-recbar");
    let wt = scope_ui::TB_LABELS.iter().map(|t| stockfont::text_width(stockfont::SC_F16, t, 0)).max().unwrap_or(0);
    println!("scope sample rate (timebase list): widest rate \"{}\" {} px, widest time {wt} px", widest.1, widest.0);
}

/// WCAG contrast ratio of two RGB565 colours.
fn contrast(a: u16, b: u16) -> f32 {
    let lum = |c: u16| {
        let ch = |v: u32, max: u32| {
            let v = (v * 255 / max) as f32 / 255.0;
            if v <= 0.03928 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
        };
        0.2126 * ch((c >> 11) as u32 & 31, 31) + 0.7152 * ch((c >> 5) as u32 & 63, 63) + 0.0722 * ch(c as u32 & 31, 31)
    };
    let (x, y) = (lum(a), lum(b));
    (x.max(y) + 0.05) / (x.min(y) + 0.05)
}

/// Panel and badge text on the user's channel colours (CH1_INK, MATH_INK)
/// and channel-coloured text on the dark bar (PAL_TEXT: trigger badge,
/// measurement strip, FFT labels): every palette entry reads at least as
/// well as the stock default math panel (white on orange); the defaults
/// keep the stock text colours; PAL_TEXT is the palette colour wherever
/// that already reads so; the drawn CHA / math label buttons, the V/div
/// badge, the trigger badge and a measurement cell use them.
fn ink_contrast(ck: &mut Checker, out: &str) {
    use scope_ui::{CH1_INK, C_BAR, C_FILL, C_WHITE, MATH_INK, PALETTE, PAL_TEXT};
    let floor = contrast(PALETTE[1], C_WHITE) - 0.01;
    let base = &stock::scenes()[0];
    ck.cases += 1;
    if CH1_INK[base.m.colour as usize] != C_FILL || MATH_INK[base.m.math_colour as usize] != C_WHITE {
        ck.fail("ink/default", "default CH1 / math text colour differs from the stock".into());
    }
    for i in 0..9usize {
        for (what, ink) in [("CH1", CH1_INK[i]), ("math", MATH_INK[i])] {
            ck.cases += 1;
            let c = contrast(PALETTE[i], ink);
            if c < floor {
                ck.fail(&format!("ink/{what}/{i}"), format!("{what} text on {} {c:.2}:1 < {floor:.2}:1", scope_ui::COLOUR_NAMES[i]));
            }
        }
        ck.cases += 1;
        let c = contrast(PAL_TEXT[i], C_BAR);
        let keep = contrast(PALETTE[i], C_BAR) >= floor;
        if c < floor || (keep && PAL_TEXT[i] != PALETTE[i]) {
            ck.fail(&format!("ink/text/{i}"), format!("{} text on the bar {c:.2}:1 (floor {floor:.2}:1), palette kept: {keep}", scope_ui::COLOUR_NAMES[i]));
        }
        let mut m = base.m;
        m.colour = i as u8;
        m.math_colour = i as u8;
        m.math_on = true;
        host::reset(0);
        scope_ui::invalidate();
        scope_ui::update(&m);
        scope_ui::meas_cell(&m, 0, "Freq:1.00kHz");
        if i == 4 {
            save(out, "s08-ink-ch1-blue");
        }
        // CHA button (B_RECT[1]), math label button (B_RECT[4]), V/div badge.
        for (what, r, fill, ink) in [
            ("CHA", scope_ui::B_RECT[1], PALETTE[i], CH1_INK[i]),
            ("math", scope_ui::B_RECT[4], PALETTE[i], MATH_INK[i]),
            ("V/div badge", stockfont::rect(304, 1, 44, 17), PALETTE[i], CH1_INK[i]),
            // Text in the channel colour on the bar: fill = the bar.
            ("trigger badge", stockfont::rect(364, 2, 49, 15), C_BAR, PAL_TEXT[i]),
            ("measurement cell", stockfont::rect(14, 303, 96, 15), C_BAR, PAL_TEXT[i]),
        ] {
            let e = emu();
            let (mut n_fill, mut n_ink, mut other) = (0, 0, 0);
            for y in r.y..r.y + r.h {
                for x in r.x..r.x + r.w {
                    let c = e.fb[y as usize * W + x as usize];
                    if c == fill {
                        n_fill += 1;
                    } else if c == ink {
                        n_ink += 1;
                    } else if c == if fill == C_BAR { if PALETTE[i] != ink { PALETTE[i] } else { 0x0001 } } else if ink == C_FILL { C_WHITE } else { C_FILL } {
                        other += 1;
                    }
                }
            }
            ck.cases += 1;
            if n_fill == 0 || n_ink == 0 || other > 0 {
                ck.fail(&format!("ink/draw/{what}/{i}"), format!("{what} on {}: fill {n_fill} px, ink {n_ink} px, the other text colour {other} px", scope_ui::COLOUR_NAMES[i]));
            }
        }
    }
}

pub fn run(ck: &mut Checker, out: &str) {
    srate(ck, out);
    ink_contrast(ck, out);
    let base = &stock::scenes()[0];
    let screen = Bx::new(0, 0, 480, 320);
    let mut n = 0;
    for combined in [false, true] {
        for k in 0..4 {
            for tabs in [0u8, 3] {
                let mut m = base.m;
                m.combined = combined;
                m.math_on = true;
                m.math_mode = 5;
                m.math_colour = (k as u8 * 2) % 9;
                m.math_off = [-105, 0, 140, -140][k];
                m.fft_hz = scope_ui::Txt::new(["250kHz", "1.25kHz", "12.5MHz", "999mHz"][k]);
                m.tabs = tabs;
                m.fft = Some(fft_add(Some(([200, 3, 395, 120][k], [40, 100, 278, 20][k])), k));
                if k == 2 {
                    m.xcur = Some([180, 390]);
                    m.sel_h = 1;
                    m.guide = true;
                }
                let sc = stock::Scene { name: "fft", m, popup: scope_ui::Popup::None, trace: base.trace.clone(), persist: None, cols: 399, cells: base.cells.clone() };
                stock::draw(&sc);
                let name = format!("fft/{}/{k}/t{tabs}", combined as u8);
                ck.check_replica(&name);
                let add = [rb(scope_ui::fft_ydiv_rect(&m)), rb(scope_ui::FFT_XDIV), rb(scope_ui::fft_peak_rect(&m))];
                let graph = [("graph", Bx::new(14, m.top() as i32 + 1, 399, m.frame_h() as i32 - 2))];
                ck.check_add(&name, &graph, &add, screen);
                n += 1;
                if k == 0 && tabs == 3 && !combined {
                    save(out, "s02-fft");
                }
                if k == 1 && combined {
                    save(out, "s02-fft-dmmscope");
                }
            }
        }
    }
    // Updates: FFT on / texts change / peak moves / off, each equal to its
    // fresh draw (the hole rows come back, no stale label pixels).
    let st = |k: usize, on: bool, combined: bool| {
        let mut m = base.m;
        m.combined = combined;
        m.math_on = on;
        m.math_mode = 5;
        m.fft_hz = scope_ui::Txt::new(["250kHz", "1.25kHz", "12.5MHz", "999mHz"][k]);
        m.fft = if k == 3 { None } else { Some(fft_add(Some(([200, 3, 395, 120][k], [40, 100, 278, 20][k])), k)) };
        m.guide = k == 1;
        m.sel_v = if k == 1 { 2 } else { 0 };
        stock::Scene { name: "fft", m, popup: scope_ui::Popup::None, trace: base.trace.clone(), persist: None, cols: 399, cells: base.cells.clone() }
    };
    for combined in [false, true] {
        let states = [st(0, true, combined), st(1, true, combined), st(2, true, combined), st(3, true, combined), st(0, false, combined)];
        for (i, a) in states.iter().enumerate() {
            for (j, b) in states.iter().enumerate() {
                if i == j {
                    continue;
                }
                stock::draw(b);
                let want = emu().fb.clone();
                stock::draw(a);
                stock::load(b);
                scope_ui::update(&b.m);
                scope_ui::cells(&b.m, &b.cells);
                ck.same_as(&format!("fft/update/{}/{i}->{j}", combined as u8), &want);
            }
        }
    }
    // Trigger tab with the trigger-type rows (UF-13): every type x
    // condition x widest width text, both views; the runt marker.
    let tab_area = Bx::new(14, 116, 399, 70);
    let mut nt = 0;
    for combined in [false, true] {
        for ttype in 0..3u8 {
            for (pcond, w, rv) in [(0u8, 500e-9f32, -444.4e-3f32), (1, 200e-3, 9.876e3), (2, 10.0, 0.0)] {
                let mut m = base.m;
                m.combined = combined;
                m.runt_row = (ttype == 2).then_some(60);
                let mut pv = stock::popvals(&m);
                pv.ttype = ttype;
                pv.pcond = pcond;
                pv.pwidth = w;
                pv.runt_v = rv;
                let sc = stock::Scene { name: "trig", m, popup: scope_ui::Popup::Mf(2), trace: base.trace.clone(), persist: None, cols: 399, cells: base.cells.clone() };
                stock::draw_with(&sc, &pv);
                let name = format!("trigtab/{}/{ttype}/{pcond}", combined as u8);
                // The option lists are the stock widget (abutting items,
                // 1 px frame): checked as neighbours of the new labels /
                // buttons / texts, not among their own items.
                let mut add = vec![rb(scope_ui::TT_LABEL), rb(scope_ui::TP_LABEL)];
                match ttype {
                    1 => add.extend([rb(scope_ui::TP_MINUS), rb(scope_ui::TP_VALUE), rb(scope_ui::TP_PLUS)]),
                    2 => add.extend([rb(scope_ui::TP_COND), rb(scope_ui::TR_HINT)]),
                    _ => {}
                }
                ck.check_add(&name, &[("page", tab_area)], &add, Bx::new(13, 20, 401, 167));
                nt += 1;
                if ttype == 1 && pcond == 0 && !combined {
                    save(out, "s03-trig-pulse");
                }
                if ttype == 2 && pcond == 0 && !combined {
                    save(out, "s03-trig-runt");
                }
            }
        }
    }
    // Cursor tab (UF-08), every state, both views, with the timebase tabs,
    // the FFT boxes and the cursors on; zoom buttons (UF-10) with it.
    for combined in [false, true] {
        for st in [scope_ui::CUR_OFF, scope_ui::CUR_X, scope_ui::CUR_Y] {
            let mut m = base.m;
            m.combined = combined;
            m.cur_tab = st;
            m.tabs = 3;
            m.math_on = true;
            m.math_mode = 5;
            m.fft = Some(fft_add(None, 1));
            if st == scope_ui::CUR_X {
                m.xcur = Some([100, 300]);
            }
            if st == scope_ui::CUR_Y {
                m.ycur = Some([70, 278]);
            }
            let sc = stock::Scene { name: "cur", m, popup: scope_ui::Popup::None, trace: base.trace.clone(), persist: None, cols: 399, cells: base.cells.clone() };
            stock::draw(&sc);
            let name = format!("curtab/{}/{st}", combined as u8);
            ck.check_replica(&name);
            let graph = [("graph", Bx::new(14, m.top() as i32 + 1, 399, m.frame_h() as i32 - 2))];
            ck.check_add(&name, &graph, &[rb(scope_ui::CUR_TAB), rb(scope_ui::FFT_XDIV)], screen);
            if st == scope_ui::CUR_X && !combined {
                save(out, "s04-cur-x-zoom");
            }
        }
    }
    // The runt level marker on the strip (selected, guide line).
    let mut m = base.m;
    m.runt_row = Some(60);
    m.sel_v = 5;
    m.guide = true;
    let sc = stock::Scene { name: "runt", m, popup: scope_ui::Popup::None, trace: base.trace.clone(), persist: None, cols: 399, cells: base.cells.clone() };
    stock::draw(&sc);
    ck.check_replica("runt-marker");
    save(out, "s03-runt-marker");
    // The DMM wave view (src/app_wave.rs): the source badge "MCX | DMM"
    // (both states), the shortened record bar beside it (running, stopped
    // with its brackets, roll), the DMM V/div texts (widest: "250V",
    // "200uA", "200mV", "2.5A"), the CH1 label ("AC uA"), the current cells
    // ("Ip-p" / "Irms"), the trigger badge in A; updates between them and
    // to / from the stock page equal fresh draws.
    let mut nw = 0;
    let wave_scene = |src: u8, k: usize, status: u8, roll: bool, tabs: u8| {
        let mut m = base.m;
        m.src = src;
        m.status = status;
        m.roll = roll;
        m.tabs = tabs;
        m.record = if status == scope_ui::ST_STOP { (0.15, 0.85) } else { (0.0, 1.0) };
        let cells: Vec<&'static str> = if src == scope_ui::SRC_DMM {
            let (vd, amps, ch1, tv) = [(250.0f32, false, "AC V", -999.0f32), (200e-6, true, "AC uA", -1.23e-3), (0.15, false, "AC mV", 0.444), (2.5, true, "AC A", 9.99)][k % 4];
            m.wvdiv = vd;
            m.amps = amps;
            m.ch1_txt = ch1;
            m.ac = !amps && ch1 == "AC V";
            m.trig_v = tv;
            if amps {
                vec!["Freq:50.0Hz", "Ip-p:999.9mA", "Cycl:20.0ms", "Irms:-12.3uA"]
            } else {
                vec!["Freq:50.0Hz", "Vp-p:999.9mV", "Cycl:20.0ms", "Vrms:-12.3uV"]
            }
        } else {
            base.cells.clone()
        };
        stock::Scene { name: "wave", m, popup: scope_ui::Popup::None, trace: base.trace.clone(), persist: None, cols: 399, cells }
    };
    let top = Bx::new(0, 0, 480, 20);
    for src in [scope_ui::SRC_MCX, scope_ui::SRC_DMM] {
        for k in 0..4 {
            for (status, roll) in [(scope_ui::ST_TD, false), (scope_ui::ST_STOP, false), (scope_ui::ST_RUN, true)] {
                let sc = wave_scene(src, k, status, roll, 3);
                stock::draw(&sc);
                let name = format!("wave/{src}/{k}/{status}/{}", roll as u8);
                ck.check_replica(&name);
                // The badge (a new item: the full rules, the badge row's
                // vertical inset as the stock badges, y 1..17) against the
                // timebase badge and the shortened record bar (the stock
                // widget, its insides exempt as the replica's).
                ck.check_add(&name, &[("badge row", Bx::new(0, -2, 480, 24))], &[rb(scope_ui::SRC_BADGE)], top);
                nw += 1;
                if src == scope_ui::SRC_DMM && status == scope_ui::ST_TD && k < 2 {
                    save(out, &format!("s05-wave-{}", ["v", "ua"][k]));
                }
                if src == scope_ui::SRC_DMM && status == scope_ui::ST_STOP && k == 2 {
                    save(out, "s05-wave-stopped");
                }
                if src == scope_ui::SRC_DMM && roll && k == 3 {
                    save(out, "s05-wave-roll-a");
                }
                if src == scope_ui::SRC_MCX && status == scope_ui::ST_TD && k == 0 {
                    save(out, "s05-mcx-badge");
                }
            }
        }
    }
    // The wave view's V/div steps (scope::wave_steps): every DMM range at
    // its full-scale step (a sine of 98 % of +-full fills the 8 divisions)
    // and at an intermediate step of the same range (a screen-only zoom:
    // the sine at 98 % of +-full or of +-4 div, the smaller), V and A.
    for amps in [false, true] {
        let st = scope::wave_steps(amps);
        for (i, s) in st.iter().enumerate() {
            if !s.2 {
                continue;
            }
            let full = s.0 * 4.0;
            // The intermediate step: the one below on this range, else the
            // one above (60 mV, 600 uA: the list's first range).
            let mid = [i.wrapping_sub(1), i + 1].into_iter().find(|&j| j < st.len() && st[j].1 == s.1);
            for (tag, j) in [("full", Some(i)), ("step", mid)] {
                let Some(j) = j else { continue };
                let mut m = base.m;
                m.src = scope_ui::SRC_DMM;
                m.status = scope_ui::ST_TD;
                m.tabs = 3;
                m.wvdiv = st[j].0;
                m.amps = amps;
                m.ch1_txt = match (amps, full) {
                    (false, f) if f < 1.0 => "AC mV",
                    (false, _) => "AC V",
                    (true, f) if f < 0.01 => "AC uA",
                    (true, f) if f < 1.0 => "AC mA",
                    _ => "AC A",
                };
                m.ac = m.ch1_txt == "AC V";
                m.trig_row = 140;
                m.trig_v = 0.0;
                let amp = (0.98 * full / st[j].0).min(3.92);
                let mut lbl = scope_ui::Buf::new();
                scope_ui::fmt_div(&mut lbl, full, if amps { "A" } else { "V" });
                let mut vpp = scope_ui::Buf::new();
                scope_ui::fmt_div(&mut vpp, 2.0 * amp * st[j].0, if amps { "A" } else { "V" });
                let pp: &'static str = Box::leak(format!("{}p-p:{}", if amps { "I" } else { "V" }, vpp.as_str()).into_boxed_str());
                let cells = vec!["Freq:50.0Hz", pp, "Cycl:20.0ms", if amps { "Irms:-" } else { "Vrms:-" }];
                let sc = stock::Scene { name: "wave", m, popup: scope_ui::Popup::None, trace: Some(stock::Trace::sine(amp, 5.0, 0.0)), persist: None, cols: 399, cells };
                stock::draw(&sc);
                let name = format!("wave/step/{}/{}", lbl.as_str(), tag);
                ck.check_replica(&name);
                ck.check_add(&name, &[("badge row", Bx::new(0, -2, 480, 24))], &[rb(scope_ui::SRC_BADGE)], top);
                nw += 1;
                save(out, &format!("s06-wave-{}-{}", lbl.as_str(), tag));
            }
        }
    }
    // The texts the wave view changes are no wider than what the stock
    // page already shows in the same box (cells: "Vp-p:" vs "Ip-p:", "mV"
    // vs "mA"; the V/div badge text box 44 px; the CH1 label in 60 px; the
    // trigger badge: the widest MCX level text, e.g. "-1.23kV").
    let wmax = |f: stockfont::Font, ts: &[&str]| ts.iter().map(|t| stockfont::text_width(f, t, 0)).max().unwrap_or(0);
    let pairs: [(&str, stockfont::Font, &[&str], &[&str]); 4] = [
        ("cells", stockfont::SC_F12, &["Ip-p:999.9mA", "Irms:-12.3mA", "Imax:-999.9mA", "Iavg:-12.3uA", "Ivpp:999kA"], &["Vp-p:999.9mV", "Vrms:-12.3mV", "Vmax:-999.9mV", "Vavg:-12.3uV"]),
        ("vdiv", stockfont::SC_F12, &["250V", "200uA", "200mA", "200mV", "150mV", "150uA", "1.5mA", "15mV", "1.5V", "2.5A"], &["10kV", "500mV"]),
        ("trig", stockfont::SC_F12, &["-1.23mA", "-999uA", "-99.9A", "-12.3kV"], &["-1.23kV", "-12.3kV", "-999mV"]),
        ("ch1", stockfont::SC_F16, &["AC V", "AC mV", "AC A", "AC mA", "AC uA"], &[]),
    ];
    for (what, f, wave_t, stock_t) in pairs {
        ck.cases += 1;
        let (ww, sw) = (wmax(f, wave_t), wmax(f, stock_t));
        let lim = match what {
            "vdiv" => 44 - 2,
            "ch1" => 60 - 6,
            _ => sw,
        };
        if ww > lim {
            ck.fail("wave/text", format!("{what}: wave texts {ww} px > {lim} (stock {sw})"));
        }
    }
    let states = [
        stock::Scene { name: "wave", m: base.m, popup: scope_ui::Popup::None, trace: base.trace.clone(), persist: None, cols: 399, cells: base.cells.clone() },
        wave_scene(scope_ui::SRC_MCX, 0, scope_ui::ST_TD, false, 3),
        wave_scene(scope_ui::SRC_DMM, 0, scope_ui::ST_TD, false, 3),
        wave_scene(scope_ui::SRC_DMM, 1, scope_ui::ST_STOP, false, 0),
        wave_scene(scope_ui::SRC_DMM, 3, scope_ui::ST_RUN, true, 3),
    ];
    for (i, a) in states.iter().enumerate() {
        for (j, b) in states.iter().enumerate() {
            if i == j {
                continue;
            }
            stock::draw(b);
            let want = emu().fb.clone();
            stock::draw(a);
            stock::load(b);
            scope_ui::update(&b.m);
            scope_ui::cells(&b.m, &b.cells);
            ck.same_as(&format!("wave/update/{i}->{j}"), &want);
            nw += 1;
        }
    }
    // A CSV recording (Model::rec, issues #101, #105 feedback): the REC mark
    // (scope_ui::rec_mark) in the source badge's slot, the record bar beside
    // it (also on the stock page of builds without the stream ISR,
    // SRC_NONE, and in the combined view): running, stopped with its
    // brackets, roll, the generator frequency; both blink phases; each of
    // the texts (RecStyle; the default Dmm last so the rest of the run
    // draws it; RecFunc with every unit). Updates between them, to / from
    // no recording and the blink step (the dot's box alone) equal fresh draws.
    let rec_scene = |src: u8, status: u8, roll: bool, gen: bool, dim: bool| {
        let mut sc = wave_scene(src, 0, status, roll, 3);
        sc.name = "rec";
        sc.m.rec = true;
        sc.m.rec_dim = dim;
        sc.m.gen_hz = if gen { 1000.0 } else { 0.0 };
        sc
    };
    let styles = [(scope_ui::RecStyle::Rec, "rec"), (scope_ui::RecStyle::RecDmm, "b"), (scope_ui::RecStyle::RecFunc, "c"), (scope_ui::RecStyle::DmmRec, "d"), (scope_ui::RecStyle::Dmm, "a")];
    for (style, sn) in styles {
        scope_ui::set_rec_style(style);
        for dim in [false, true] {
            for src in [scope_ui::SRC_NONE, scope_ui::SRC_MCX] {
                for (status, roll, gen) in [(scope_ui::ST_TD, false, false), (scope_ui::ST_STOP, false, false), (scope_ui::ST_RUN, true, false), (scope_ui::ST_TD, false, true)] {
                    let sc = rec_scene(src, status, roll, gen, dim);
                    stock::draw(&sc);
                    let name = format!("rec/{sn}/{}/{src}/{status}/{}/{}", dim as u8, roll as u8, gen as u8);
                    ck.check_replica(&name);
                    ck.check_add(&name, &[("badge row", Bx::new(0, -2, 480, 24))], &[rb(scope_ui::SRC_BADGE)], top);
                    nw += 1;
                    if src == scope_ui::SRC_MCX {
                        let st = match (status, roll, gen) {
                            (scope_ui::ST_STOP, _, _) => "stop",
                            (_, true, _) => "roll",
                            (_, _, true) => "gen",
                            _ => "run",
                        };
                        save(out, &format!("s05-rec-{sn}-{st}-{}", if dim { "off" } else { "on" }));
                    }
                }
            }
            // The combined view (no source badge; the strip below).
            let mut sc = rec_scene(scope_ui::SRC_NONE, scope_ui::ST_TD, false, false, dim);
            sc.m.combined = true;
            stock::draw(&sc);
            let name = format!("rec/{sn}/{}/combined", dim as u8);
            ck.check_add(&name, &[("badge row", Bx::new(0, -2, 480, 24))], &[rb(scope_ui::SRC_BADGE)], top);
            nw += 1;
            save(out, &format!("s05-rec-{sn}-combined-{}", if dim { "off" } else { "on" }));
        }
        // RecFunc: every recorded function's unit (the widest decides).
        if style == scope_ui::RecStyle::RecFunc {
            for (k, f) in [1u8, 7, 10, 13, 14, 18, 15].into_iter().enumerate() {
                let mut sc = rec_scene(scope_ui::SRC_MCX, scope_ui::ST_STOP, false, false, false);
                sc.m.rec_unit = scope_ui::rec_unit(f);
                stock::draw(&sc);
                let name = format!("rec/{sn}/unit/{f}");
                ck.check_replica(&name);
                ck.check_add(&name, &[("badge row", Bx::new(0, -2, 480, 24))], &[rb(scope_ui::SRC_BADGE)], top);
                nw += 1;
                save(out, &format!("s05-rec-c-unit{k}"));
            }
        }
        let states = [
            stock::Scene { name: "rec", m: base.m, popup: scope_ui::Popup::None, trace: base.trace.clone(), persist: None, cols: 399, cells: base.cells.clone() },
            wave_scene(scope_ui::SRC_MCX, 0, scope_ui::ST_TD, false, 3),
            rec_scene(scope_ui::SRC_MCX, scope_ui::ST_TD, false, false, false),
            rec_scene(scope_ui::SRC_MCX, scope_ui::ST_TD, false, false, true),
            rec_scene(scope_ui::SRC_MCX, scope_ui::ST_STOP, false, false, false),
            rec_scene(scope_ui::SRC_MCX, scope_ui::ST_STOP, false, false, true),
            rec_scene(scope_ui::SRC_NONE, scope_ui::ST_TD, false, true, false),
            rec_scene(scope_ui::SRC_NONE, scope_ui::ST_TD, false, true, true),
            rec_scene(scope_ui::SRC_NONE, scope_ui::ST_RUN, true, false, true),
            {
                let mut s = rec_scene(scope_ui::SRC_MCX, scope_ui::ST_TD, false, false, false);
                s.m.rec_unit = scope_ui::rec_unit(14);
                s
            },
        ];
        for (i, a) in states.iter().enumerate() {
            for (j, b) in states.iter().enumerate() {
                if i == j {
                    continue;
                }
                stock::draw(b);
                let want = emu().fb.clone();
                stock::draw(a);
                stock::load(b);
                scope_ui::update(&b.m);
                scope_ui::cells(&b.m, &b.cells);
                ck.same_as(&format!("rec/{sn}/update/{i}->{j}"), &want);
                nw += 1;
            }
        }
        // The blink step above a popup (App::sp_tick: rec_mark_blink alone).
        for dim in [false, true] {
            let mut b = rec_scene(scope_ui::SRC_MCX, scope_ui::ST_TD, false, false, dim);
            stock::draw(&b);
            let want = emu().fb.clone();
            b.m.rec_dim = !dim;
            stock::draw(&b);
            scope_ui::rec_mark_blink(scope_ui::SRC_BADGE, scope_ui::C_BAR, !dim, b.m.rec_unit);
            ck.same_as(&format!("rec/{sn}/blink/{}", dim as u8), &want);
            nw += 1;
        }
    }
    // Trigger-level drag steps (src/scope_ui.rs Graph::flush repaints the
    // changed guide rows as row runs between the holes; the trigger badge
    // after the graph): each step equals its fresh draw, plain and with the
    // corner tabs, the X / Y cursor lines and the cursor tab, the FFT boxes
    // over the guide rows, both views; rows across the holes and off screen.
    let mut nd = 0;
    for combined in [false, true] {
        for v in 0..4 {
            let step = |row: i16| {
                let mut m = base.m;
                m.combined = combined;
                m.sel_v = 1;
                m.guide = true;
                m.trig_row = row;
                m.trig_v = (140 - row) as f32 * 4.0 / 141.0;
                match v {
                    1 => m.tabs = 3,
                    2 => {
                        m.xcur = Some([60, 300]);
                        m.ycur = Some([40, 200]);
                        m.cur_tab = 1;
                    }
                    3 => {
                        m.math_on = true;
                        m.math_mode = 5;
                        m.fft_hz = scope_ui::Txt::new("250kHz");
                        m.fft = Some(fft_add(Some((200, 100)), 0));
                        m.tabs = 3;
                    }
                    _ => {}
                }
                stock::Scene { name: "drag", m, popup: scope_ui::Popup::None, trace: base.trace.clone(), persist: None, cols: 399, cells: base.cells.clone() }
            };
            let rows = [105i16, 110, 3, 15, 140, 250, 266, 272, 278, -10, 290, 40];
            for w in rows.windows(2) {
                let (a, b) = (step(w[0]), step(w[1]));
                stock::draw(&b);
                let want = emu().fb.clone();
                stock::draw(&a);
                stock::load(&b);
                scope_ui::update(&b.m);
                scope_ui::cells(&b.m, &b.cells);
                ck.same_as(&format!("drag/{}/{v}/{}->{}", combined as u8, w[0], w[1]), &want);
                nd += 1;
            }
        }
    }
    println!("scope additions: {n} FFT scenes, {nt} trigger-tab scenes, {nw} wave-view scenes, {nd} trigger-level drag steps");
    gen_channel(ck, out);
}

/// The GEN channel (docs/scope-gen-channel.md): the computed trace under
/// CH1, the trigger badge in its colour, the Source row of the Trigger
/// tab and the GEN trace row of the Display tab; an update equals a fresh
/// draw.
fn gen_channel(ck: &mut Checker, out: &str) {
    let base = &stock::scenes()[0];
    let shape = gen::Shape { wave: gen::Wave::Sine, lo_v: 0.0, hi_v: 3.0, duty: 0.5 };
    for combined in [false, true] {
        let mut m = base.m;
        m.combined = combined;
        m.tsrc = scope::TSRC_GEN;
        let h = m.frame_h();
        let draw = |m: &scope_ui::Model, popup: scope_ui::Popup, pv: &scope_ui::PopVals, gen_on: bool| {
            let sc = stock::Scene { name: "gen", m: *m, popup: scope_ui::Popup::None, trace: base.trace.clone(), persist: None, cols: 399, cells: base.cells.clone() };
            stock::draw_with(&sc, pv);
            if gen_on {
                let g = scope_ui::graph();
                for i in 0..399 {
                    // 2.5 periods on the screen, 1 V/div, 0 V on the centre row.
                    let (lo, hi) = shape.span(i as f32 * 2.5 / 399.0, 2.5 / 399.0);
                    let r = |v: f32| (140.0 - 141.0 * v / 4.0) as i32;
                    g.gn[i] = (scope_ui::trace_fy(h, r(hi)), scope_ui::trace_fy(h, r(lo)));
                    let c = (200.0 + 60.0 * shape.unit(i as f32 * 1.5 / 399.0 + 0.1)) as i32;
                    g.c2[i] = (scope_ui::trace_fy(h, c), scope_ui::trace_fy(h, c + 1));
                }
                scope_ui::update(m);
            }
            scope_ui::draw_popup(popup, pv);
        };
        let mut pv = stock::popvals(&m);
        pv.tsrc = scope::TSRC_GEN;
        pv.gen_trace = true;
        pv.ch2_on = true;
        pv.ch2_vdiv = scope_ui::Txt::new("250mA");
        // The DMM CH2 too: its label (a graph hole) and a trace.
        m.ch2_txt = scope_ui::Txt::new("CH2 250mA");
        draw(&m, scope_ui::Popup::None, &pv, true);
        ck.check_replica(&format!("gen/{}/trace", combined as u8));
        let want = emu().fb.clone();
        if !combined {
            save(out, "s08-gen-trace");
        }
        // Off -> on as an update.
        draw(&m, scope_ui::Popup::None, &pv, false);
        let g = scope_ui::graph();
        for i in 0..399 {
            let (lo, hi) = shape.span(i as f32 * 2.5 / 399.0, 2.5 / 399.0);
            let r = |v: f32| (140.0 - 141.0 * v / 4.0) as i32;
            g.gn[i] = (scope_ui::trace_fy(h, r(hi)), scope_ui::trace_fy(h, r(lo)));
            let c = (200.0 + 60.0 * shape.unit(i as f32 * 1.5 / 399.0 + 0.1)) as i32;
            g.c2[i] = (scope_ui::trace_fy(h, c), scope_ui::trace_fy(h, c + 1));
        }
        scope_ui::update(&m);
        ck.same_as(&format!("gen/{}/update", combined as u8), &want);
        for (tab, name) in [(1u8, "s08-gen-display"), (2, "s08-gen-trigger")] {
            draw(&m, scope_ui::Popup::Mf(tab), &pv, true);
            let add = if tab == 1 {
                vec![rb(scope_ui::GT_LABEL), rb(scope_ui::C2_LABEL), rb(scope_ui::C2_MINUS), rb(scope_ui::C2_VALUE), rb(scope_ui::C2_PLUS)]
            } else {
                vec![rb(scope_ui::TS_LABEL), rb(scope_ui::TS_NOTE)]
            };
            let area = if tab == 1 { Bx::new(14, 161, 399, 99) } else { Bx::new(14, 186, 399, 50) };
            ck.check_add(&format!("gen/{}/tab{tab}", combined as u8), &[("page", area)], &add, Bx::new(13, 20, 401, if tab == 1 { 241 } else { 216 }));
            if !combined {
                save(out, name);
            }
        }
    }
}
