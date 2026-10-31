//! Replica scope page scenes (src/scope_ui.rs) in the same states as the
//! stock reference scenes of tools/stock-ui/scope.rs, for the pixel diff
//! (tools/ui-render/stock-diff.sh): same names, same values, the same
//! synthetic traces (rows of the 280-row model, the stock column widening).
use crate::host;
use crate::scope_ui::{self as su, Model};

/// A synthetic trace: per column (min row, max row) of the 280-row model
/// (tools/stock-ui/scope.rs Trace).
#[derive(Clone)]
pub struct Trace {
    pub a: Vec<i32>,
    pub b: Vec<i32>,
}

impl Trace {
    pub fn sine(amp: f32, periods: f32, phase: f32) -> Trace {
        let (mut a, mut b) = (vec![], vec![]);
        for i in 0..400 {
            let t = (i as f32 / 400.0 + phase) * periods * core::f32::consts::TAU;
            let t1 = ((i + 1) as f32 / 400.0 + phase) * periods * core::f32::consts::TAU;
            let r0 = 140.0 - amp * 35.0 * t.sin();
            let r1 = 140.0 - amp * 35.0 * t1.sin();
            a.push(r0.min(r1).round() as i32);
            b.push(r0.max(r1).round() as i32);
        }
        Trace { a, b }
    }
    pub fn square(amp: f32, periods: f32, duty: f32) -> Trace {
        let (mut a, mut b) = (vec![], vec![]);
        for i in 0..400 {
            let ph = (i as f32 / 400.0 * periods).fract();
            let r = if ph < duty { 140 - (amp * 35.0) as i32 } else { 140 + (amp * 35.0) as i32 };
            a.push(r);
            b.push(r);
        }
        Trace { a, b }
    }
    pub fn noise(centre: i32, amp: i32, seed: u32) -> Trace {
        let mut s = seed;
        let (mut a, mut b) = (vec![], vec![]);
        for _ in 0..400 {
            s = s.wrapping_mul(1664525).wrapping_add(1013904223);
            let r = centre + ((s >> 16) as i32 % (2 * amp + 1)) - amp;
            a.push(r);
            b.push(r);
        }
        Trace { a, b }
    }
    pub fn shifted(&self, rows: i32) -> Trace {
        Trace { a: self.a.iter().map(|r| r + rows).collect(), b: self.b.iter().map(|r| r + rows).collect() }
    }
    /// Engine widening (FUN_080310f8) + FUN_0802dc28 order and clamp.
    pub fn columns(&self, n: usize) -> Vec<(i32, i32)> {
        let mut out = vec![];
        let (mut pa, mut pb) = (0, 0);
        for i in 0..n.min(400) {
            let (mut a, mut b) = (self.a[i].min(self.b[i]), self.a[i].max(self.b[i]));
            if i > 0 {
                a = a.min(pb);
                b = b.max(pa);
            }
            pa = a;
            pb = b;
            out.push((a.min(b).clamp(0, 278), a.max(b).clamp(0, 278)));
        }
        out
    }
}

pub struct Scene {
    pub name: &'static str,
    pub m: Model,
    pub popup: su::Popup,
    pub trace: Option<Trace>,
    pub persist: Option<Trace>,
    pub cols: usize,
    pub cells: Vec<&'static str>,
}

/// The math trace of a scene (tools/stock-ui st11-fft).
pub fn math_trace(sc: &Scene) -> Option<Trace> {
    if sc.name == "st11-fft" { Some(Trace::noise(230, 3, 3)) } else { None }
}

const VDIV: [f32; 22] = [0.001, 0.002, 0.005, 0.01, 0.02, 0.05, 0.1, 0.2, 0.5, 1.0, 2.0, 5.0, 10.0, 20.0, 50.0, 100.0, 200.0, 500.0, 1000.0, 2000.0, 5000.0, 10000.0];
const PROBE: [f32; 13] = [0.1, 0.2, 0.5, 1.0, 2.0, 5.0, 10.0, 20.0, 50.0, 100.0, 200.0, 500.0, 1000.0];

/// The reference renderer's trigger-level volts (same formula, so the badge
/// texts match).
fn trig_v(m: &Model) -> f32 {
    (140 + m.pos as i32 - m.trig_row as i32) as f32 * 4.0 * VDIV[m.vdiv as usize] * PROBE[m.probe as usize] / 141.0
}

pub fn scenes() -> Vec<Scene> {
    let mut v = vec![];
    let d = Model::DEFAULT;
    let cells = vec!["Freq:1.00kHz", "Vp-p:3.00V", "Cycl:1.00ms", "Vrms:1.06V"];
    let mut add = |name: &'static str, m: Model, trace: Option<Trace>, persist: Option<Trace>, cols: usize, c: Vec<&'static str>| {
        let mut m = m;
        m.trig_v = trig_v(&m);
        v.push(Scene { name, m, popup: su::Popup::None, trace, persist, cols, cells: c });
    };
    let sine = || Some(Trace::sine(1.5, 2.0, 0.0));
    add("st01-default-td", d, sine(), None, 399, cells.clone());
    let mut m = d;
    m.status = su::ST_STOP;
    m.record = (0.1, 0.3);
    add("st02-stop-zoomed", m, sine(), None, 399, cells.clone());
    let mut m = d;
    m.status = su::ST_AUTO;
    add("st03-auto-square", m, Some(Trace::square(2.0, 3.0, 0.3)), None, 399, cells.clone());
    let mut m = d;
    m.status = su::ST_WAIT;
    add("st04-wait-empty", m, None, None, 399, cells.clone());
    let mut m = d;
    m.tb = 23;
    m.status = su::ST_RUN;
    m.roll = true;
    add("st05-roll", m, Some(Trace::noise(150, 6, 7)), None, 180, cells.clone());
    let mut m = d;
    m.xcur = Some([120, 280]);
    m.sel_h = 2;
    add("st06-xcursors", m, sine(), None, 399, vec!["X2-1:800us", "Xf:1.25kHz"]);
    let mut m = d;
    m.ycur = Some([60, 220]);
    m.sel_v = 3;
    add("st07-ycursors", m, sine(), None, 399, vec!["Y2-1:4.54V"]);
    let mut m = d;
    m.sel_v = 1;
    m.trig_row = 60;
    m.pos = 40;
    m.guide = true;
    add("st08-drag-trig-level", m, Some(Trace::sine(1.5, 2.0, 0.0).shifted(40)), None, 399, cells.clone());
    let mut m = d;
    m.guide = true;
    m.trig_px = 120;
    m.pos = -30;
    add("st09-drag-pos-guides", m, Some(Trace::sine(1.5, 2.0, 0.2).shifted(-30)), None, 399, cells.clone());
    let mut m = d;
    m.pos = 200;
    m.trig_row = 295;
    m.trig_px = 5;
    add("st10-offscreen-markers", m, Some(Trace::sine(1.5, 2.0, 0.0).shifted(200)), None, 399, cells.clone());
    // Math on, FFT (tools/stock-ui st11-fft: noise at row 230, orange, the
    // 20dB label, offset 0, "1.25kHz"); the stock look only (no opendm40 FFT
    // additions).
    let mut m = d;
    m.math_on = true;
    m.math_mode = 5;
    m.math_colour = 1;
    m.math_scale = su::Txt::new("20dB");
    m.fft_hz = su::Txt::new("1.25kHz");
    add("st11-fft", m, sine(), None, 399, cells.clone());
    add("st12-persistence", d, sine(), Some(Trace::sine(1.5, 2.0, 0.03)), 399, cells.clone());
    let mut m = d;
    m.ac = true;
    m.probe = 6;
    m.vdiv = 12;
    m.rising = false;
    m.colour = 3;
    m.battery = 1;
    m.charging = true;
    add("st13-ac-x10-green-falling", m, sine(), None, 399, cells.clone());
    // Popups over the default page (st2x MF tabs, st3x CH1, st4x math).
    let mut pops: Vec<(&'static str, Model, su::Popup)> = vec![];
    for (t, n) in ["st20-settings-measure", "st21-settings-display", "st22-settings-trigger", "st23-settings-settings"].iter().enumerate() {
        pops.push((n, d, su::Popup::Mf(t as u8)));
    }
    for (t, n) in ["st30-ch1-coupling", "st31-ch1-probe", "st32-ch1-color"].iter().enumerate() {
        let mut m = d;
        m.ch1_open = true;
        pops.push((n, m, su::Popup::Ch1(t as u8)));
    }
    for (t, n) in ["st40-math-mode", "st41-math-color", "st42-math-hzdiv", "st43-math-other"].iter().enumerate() {
        let mut m = d;
        m.math_open = true;
        // The stock reference opens them with math on, FFT.
        m.math_on = true;
        m.math_mode = 5;
        m.math_scale = su::Txt::new("20dB");
        m.fft_hz = su::Txt::new("1.25kHz");
        pops.push((n, m, su::Popup::Math(t as u8)));
    }
    drop(add);
    for (n, m, p) in pops {
        let mut m = m;
        m.trig_v = trig_v(&m);
        v.push(Scene { name: n, m, popup: p, trace: sine(), persist: None, cols: 399, cells: cells.clone() });
    }
    v
}

/// States of the stock V2.6 captured in the Renode emulator
/// (tools/ui-render/emu-diff.sh): the unit's persisted state (10 ms/div,
/// falling edge at -88.3 mV, trigger column 158, charging) and the cell texts
/// read off the capture; DC 0 V input: a flat trace on row 140 (the
/// emulator's ADC feeder ignores the offset PWM, so the trace does not
/// follow the position of -4 rows).
pub fn emu_scenes() -> Vec<Scene> {
    let mut m = Model::DEFAULT;
    m.status = su::ST_AUTO;
    m.tb = 18;
    m.rising = false;
    m.trig_v = -0.0883;
    m.trig_row = 139;
    m.pos = -4;
    m.trig_px = 158;
    m.battery = 4;
    m.charging = true;
    // The unit has FFT as its math mode (math off): the pill says "FFT".
    m.math_mode = 5;
    let cells = vec!["Freq:167kHz", "Vp-p:0.00uV", "Cycl:6.00us", "Vrms:122mV"];
    let none = vec!["--", "--", "--", "--"];
    let flat = Trace { a: vec![140; 400], b: vec![140; 400] };
    let mut v = vec![];
    // (name, model, popup, battery level (the charging animation), cells)
    // Right after a V/div / timebase / level change the stock graph is
    // empty until the next record (sec. 4.5): no trace in e12..e16.
    let mut add = |name: &'static str, m: Model, popup: su::Popup, batt: u8, c: &Vec<&'static str>| {
        let mut m = m;
        m.battery = batt;
        let blank = matches!(&name[..3], "e12" | "e13" | "e14" | "e15" | "e16" | "e22" | "e23");
        v.push(Scene { name, m, popup, trace: if blank { None } else { Some(flat.clone()) }, persist: None, cols: 399, cells: c.clone() })
    };
    add("e01-page", m, su::Popup::None, 4, &cells);
    let mut c = m;
    c.ch1_open = true;
    add("e02-ch1-coupling", c, su::Popup::Ch1(0), 3, &cells);
    add("e03-ch1-probe", c, su::Popup::Ch1(1), 4, &cells);
    add("e04-ch1-color", c, su::Popup::Ch1(2), 4, &cells);
    add("e05-mf-trigger", m, su::Popup::Mf(2), 4, &cells);
    add("e06-mf-measure", m, su::Popup::Mf(0), 3, &cells);
    add("e07-mf-display", m, su::Popup::Mf(1), 4, &cells);
    add("e08-mf-settings", m, su::Popup::Mf(3), 4, &cells);
    // Run 2 (seq2): V/div and timebase steps, a trigger-level drag, cursors.
    add("e11-page", m, su::Popup::None, 3, &cells);
    let mut s = m;
    s.vdiv = 10;
    s.trig_v = -0.183;
    add("e12-vdiv2v", s, su::Popup::None, 4, &none);
    s.vdiv = 8;
    s.trig_v = -0.043;
    add("e13-vdiv500mv", s, su::Popup::None, 4, &none);
    s.tb = 19;
    s.guide = true;
    add("e14-tb-slower", s, su::Popup::None, 3, &none);
    s.tb = 17;
    add("e15-tb-faster", s, su::Popup::None, 4, &none);
    s.trig_row = 101;
    s.trig_v = 0.496;
    s.sel_v = 1;
    add("e16-trig-drag", s, su::Popup::None, 3, &none);
    s.guide = false;
    let c17 = vec!["Freq:333kHz", "Vp-p:0.00uV", "Cycl:3.00us", "Vrms:59.8mV"];
    add("e17-guide-gone", s, su::Popup::None, 3, &c17);
    s.xcur = Some([10, 20]);
    add("e18-xcursors", s, su::Popup::None, 4, &vec!["X2-1:1.25ms", "--", "--", "--"]);
    s.ycur = Some([10, 20]);
    add("e19-xycursors", s, su::Popup::None, 4, &vec!["X2-1:1.25ms", "Y2-1:-143mV", "--", "--"]);
    // Run 3 (seq3): STOP, AC coupling, rising edge + Normal (WAIT), roll.
    let mut s = m;
    s.status = su::ST_STOP;
    // The stored record is longer than the screen: no "]".
    s.record = (0.0, 1.5);
    add("e20-stop", s, su::Popup::None, 4, &cells);
    let mut s = m;
    s.ac = true;
    add("e21-ac-x10-green", s, su::Popup::None, 3, &cells);
    s.rising = true;
    s.status = su::ST_WAIT;
    add("e22-rising-normal", s, su::Popup::None, 4, &none);
    s.status = su::ST_RUN;
    s.tb = 26;
    s.roll = true;
    s.guide = true;
    add("e23-roll", s, su::Popup::None, 4, &vec!["Freq:333Hz", "Vp-p:0.00uV", "Cycl:3.00ms", "Vrms:122mV"]);
    v
}

/// Popup values of the emulator captures.
pub fn emu_popvals(m: &Model) -> su::PopVals {
    su::PopVals { persist: 1, rising: false, pressed_cell: None, ..popvals(m) }
}

/// Popup values of the reference scenes (tools/stock-ui Scene::default).
pub fn popvals(m: &Model) -> su::PopVals {
    su::PopVals {
        meas: [0, 10, 9, 11],
        meas_n: 4,
        roll: false,
        persist: 0,
        bright: 100,
        grid: 50,
        rising: m.rising,
        mode: 0,
        ac: m.ac,
        probe: m.probe,
        colour: m.colour,
        math_colour: m.math_colour,
        pressed_cell: Some(11),
        math_mode: 5,
        fft_hz: 2,
        hz_opts: [su::Txt::new("250Hz"), su::Txt::new("500Hz"), su::Txt::new("1.25kHz"), su::Txt::new("2.50kHz")],
        fft_win: 0,
        fft_vrms: false,
        ttype: 0,
        pcond: 0,
        pwidth: 1e-3,
        runt_v: 0.0,
        tsrc: 0,
        gen_trace: false,
        ch2_on: false,
        ch2_vdiv: su::Txt::EMPTY,
    }
}

/// Load a scene's traces into the graph (frame rows).
pub fn load(sc: &Scene) {
    let g = su::graph();
    g.clear();
    let h = sc.m.frame_h();
    if let Some(t) = &sc.trace {
        for (i, (a, b)) in t.columns(sc.cols).iter().enumerate().take(399) {
            g.tr[i] = (su::trace_fy(h, *a), su::trace_fy(h, *b));
        }
    }
    if let Some(t) = &sc.persist {
        for (i, (a, b)) in t.columns(399).iter().enumerate().take(399) {
            g.ps[i] = (su::trace_fy(h, *a), su::trace_fy(h, *b));
        }
    }
    if let Some(t) = math_trace(sc) {
        for (i, (a, b)) in t.columns(399).iter().enumerate().take(399) {
            g.ma[i] = (su::trace_fy(h, *a), su::trace_fy(h, *b));
        }
    }
}

/// Draw a scene on a fresh screen.
pub fn draw(sc: &Scene) {
    draw_with(sc, &popvals(&sc.m))
}

pub fn draw_with(sc: &Scene, pv: &su::PopVals) {
    host::reset(0);
    su::invalidate();
    load(sc);
    su::update(&sc.m);
    su::cells(&sc.m, &sc.cells);
    su::draw_popup(sc.popup, pv);
}
