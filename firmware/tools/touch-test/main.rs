//! Host test of the touch input path (tools/touch-test/run.sh):
//!   1. touch.rs `Debounce` with synthetic controller frames (jittering
//!      touch-down, quick taps, a one-frame blip, release bounce, release
//!      latency, side keys);
//!   2. gesture.rs `Tracker` tap / swipe / long-press / capture decisions;
//!   3. the DBG:TOUCH? log text;
//!   4. hit map vs. drawing: every page is drawn by the real src/ui.rs into
//!      the LCD emulator of tools/ui-render/host.rs (draw-call probes on);
//!      every drawn button box (an `sf::rbox` shape) whose centre hits
//!      something must hit the SAME target on every one of its pixels, two
//!      buttons must not share a target, and every target must be drawn.
//!      Reported per page (not failures): how far each hit area reaches past
//!      its drawn box, and the dead pixels next to buttons (a tap there does
//!      nothing).
//!   5. touchcal.rs: the affine fit (synthetic taps through a known map
//!      give it back; outliers, a bad scale or shift are refused) and
//!      touch.rs `screen()` with a calibration (keys never mapped);
//!   main [-v]     prints "cases N, failures M", exit 1 if M > 0
#![allow(dead_code, unused, static_mut_refs)]

#[path = "../ui-render/host.rs"]
mod host;
pub use host::mmio;

#[path = "../../src/ident.rs"]
mod ident;
#[path = "../../src/bargraph.rs"]
mod bargraph;
#[path = "../../src/clock.rs"]
mod clock;
#[path = "../../src/dialog.rs"]
mod dialog;
#[path = "../../src/sentinel.rs"]
mod sentinel;
// dmm.rs acdcm.rs (AC+DC method C): chip access only.
mod ad2exp {
    pub fn dc_enter(_mode: u8, _r: u8, _r26: u8, _measure: bool) {}
    pub fn dcv_row(_r: u8, _r26: u8, _opt: u8, _op1chop: u8, _ad2: bool) -> [u8; crate::hy3131::CFG_LEN] {
        [0; crate::hy3131::CFG_LEN]
    }
}
#[path = "../../src/fastvos.rs"]
mod fastvos;
#[path = "../../src/dmm.rs"]
mod dmm;
#[path = "../../src/gen.rs"]
mod gen;
#[path = "../../src/gesture.rs"]
mod gesture;
#[path = "../../src/hwtrig.rs"]
mod hwtrig;
#[path = "../../src/hy3131.rs"]
mod hy3131;
#[path = "../../src/hy3131_tables.rs"]
mod hy3131_tables;
#[path = "../../src/lcd.rs"]
mod lcd;
#[path = "../../src/meter.rs"]
mod meter;
#[path = "../../src/scope.rs"]
mod scope;
#[path = "../../src/scope_meas.rs"]
mod scope_meas;
#[path = "../../src/trigsoft.rs"]
mod trigsoft;
#[path = "../../src/scope_ui.rs"]
mod scope_ui;
#[path = "../../src/stockfont.rs"]
mod stockfont;
#[path = "../../src/touch.rs"]
mod touch;
#[path = "../../src/touchcal.rs"]
mod touchcal;
#[path = "../../src/ui.rs"]
mod ui;
// The inrush page's trace geometry (ui_inrush.rs, a ui submodule).
#[path = "../../src/inrush.rs"]
mod inrush;

/// Probe hooks called by the drawing code under cfg(test).
pub mod probe {
    /// ui-render's clip rule (stockfont probe_fit): not checked here.
    pub fn fit(_r: (i32, i32, i32, i32), _ink: (i32, i32, i32, i32), _text: &str) {}
    pub fn open(kind: u8, bg: u16) -> u32 {
        crate::host::open(kind, bg)
    }
    pub fn set(id: u32, bg: u16, g: u8) {
        crate::host::set(id, bg, g)
    }
    pub const TEXT: u8 = 0;
    pub const SHAPE: u8 = 1;
    pub const ICON: u8 = 2;
    pub fn begin(kind: u8, bg: u16) {
        crate::host::begin(kind, bg)
    }
    pub fn sub(g: u8) {
        crate::host::sub(g)
    }
    pub fn end() {
        crate::host::end()
    }
    pub fn group(on: bool) {
        crate::host::group(on)
    }
}

use gesture::{Axis, Gesture as G, Tracker};
use host::{emu, H, W};
use touch::{Debounce, Event, Frame, Point};

struct Ck {
    cases: usize,
    fails: usize,
    verbose: bool,
}

impl Ck {
    fn t(&mut self, name: &str, ok: bool) {
        self.cases += 1;
        if !ok {
            self.fails += 1;
            println!("FAIL {name}");
        } else if self.verbose {
            println!("ok   {name}");
        }
    }
}

// ------------------------------------------------------------ 1. debounce

fn pressed(x: u16, y: u16) -> Frame {
    let mut f = Frame { pressed: true, n: 1, ..Frame::default() };
    f.pts[0] = Point { x, y };
    f
}

fn released() -> Frame {
    Frame::default()
}

fn key(code: u8) -> Frame {
    Frame { pressed: true, key: true, code, ..Frame::default() }
}

/// Run frames (time, Some(frame) or None = a poll without a frame) through a
/// fresh debounce; returns (time, event) for every emitted event.
fn deb(seq: &[(u64, Option<Frame>)]) -> Vec<(u64, Event)> {
    let mut d = Debounce::new();
    let mut out = vec![];
    for (t, f) in seq {
        let e = match f {
            Some(f) => d.frame(f, *t),
            None => d.idle(*t),
        };
        if let Some(e) = e {
            out.push((*t, e));
        }
    }
    out
}

/// Polls without a frame every 10 ms in [a, b).
fn idle(a: u64, b: u64) -> Vec<(u64, Option<Frame>)> {
    (a..b).step_by(10).map(|t| (t, None)).collect()
}

fn debounce_tests(ck: &mut Ck) {
    // A finger landing with 1 px of jitter per report (10 ms period), held
    // 100 ms. The old byte-identical rule never confirmed it.
    let mut s: Vec<(u64, Option<Frame>)> = (0..10).map(|i| (i * 10, Some(pressed(200 + (i % 2) as u16, 150 + i as u16)))).collect();
    s.push((100, Some(released())));
    s.extend(idle(110, 200));
    let ev = deb(&s);
    ck.t("jitter: down at the 2nd frame", ev.first().map(|e| (e.0, e.1.points)) == Some((10, 1)));
    ck.t("jitter: down point = 2nd frame", ev.first().map(|e| e.1.p) == Some(Point { x: 201, y: 151 }));
    let up = ev.iter().find(|e| e.1.points == 0).map(|e| e.0);
    ck.t("jitter: release within 25..35 ms of the release frame", up.is_some_and(|t| (125..=135).contains(&t)));
    // Quick tap: 3 reports (all different), release.
    let mut s: Vec<(u64, Option<Frame>)> = vec![(0, Some(pressed(100, 100))), (12, Some(pressed(102, 99))), (24, Some(pressed(103, 101))), (36, Some(released()))];
    s.extend(idle(46, 120));
    let ev = deb(&s);
    ck.t("quick tap: down + up", ev.len() >= 2 && ev[0].1.points == 1 && ev.last().unwrap().1.points == 0);
    // A one-frame blip (noise): nothing.
    let mut s = vec![(0, Some(pressed(300, 50))), (10, Some(released()))];
    s.extend(idle(20, 100));
    ck.t("one-frame blip: no event", deb(&s).is_empty());
    // Controller that only reports on change: one pressed frame, silence.
    let mut s = vec![(0, Some(pressed(300, 50)))];
    s.extend(idle(10, 60));
    let ev = deb(&s);
    ck.t("single frame + silence: down after 40 ms", ev.len() == 1 && ev[0].0 == 40);
    // Release bounce inside a touch: release frame, pressed again 15 ms later.
    let mut s: Vec<(u64, Option<Frame>)> = (0..5).map(|i| (i * 10, Some(pressed(250, 200)))).collect();
    s.push((50, Some(released())));
    s.push((60, None));
    s.push((65, Some(pressed(251, 200))));
    s.extend((75..150).step_by(10).map(|t| (t, Some(pressed(251, 201)))));
    s.push((150, Some(released())));
    s.extend(idle(160, 250));
    let ev = deb(&s);
    let downs = ev.iter().filter(|e| e.1.points > 0).count();
    let ups = ev.iter().filter(|e| e.1.points == 0).count();
    let first_down = ev.iter().position(|e| e.1.points > 0);
    ck.t("release bounce: one touch (one up)", ups == 1 && first_down == Some(0));
    ck.t("release bounce: moves only after the down", downs >= 1);
    // Two release frames: release at once.
    let s = vec![(0, Some(pressed(10, 10))), (10, Some(pressed(10, 10))), (20, Some(released())), (30, Some(released()))];
    ck.t("two release frames: up at the 2nd", deb(&s).last().map(|e| (e.0, e.1.points)) == Some((30, 0)));
    // Side key: down after two frames, up after the release; a key report
    // during a touch is dropped.
    let mut s = vec![(1000, Some(key(2))), (1010, Some(key(2))), (1020, Some(released()))];
    s.extend(idle(1030, 1100));
    let ev = deb(&s);
    ck.t("side key: down + up", ev.len() == 2 && ev[0].1.keys == 4 && ev[1].1.keys == 0);
    let s = vec![(0, Some(pressed(10, 10))), (10, Some(pressed(10, 10))), (20, Some(key(1))), (30, Some(key(1)))];
    ck.t("key report during a touch: dropped", deb(&s).iter().all(|e| e.1.keys == 0));
    // Points are clamped to the panel.
    let s = vec![(0, Some(pressed(600, 400))), (10, Some(pressed(600, 400)))];
    ck.t("clamp to 479,319", deb(&s).first().map(|e| e.1.p) == Some(Point { x: 479, y: 319 }));
}

// ------------------------------------------------------------- 2. gestures

/// One-finger touch along `path` (a point every 10 ms from t = 1000), then
/// released; `cap` = the app captures on Down. Returns the gestures.
fn touch(path: &[(u16, u16)], cap: bool, hold_ms: u64) -> Vec<G> {
    let mut tr = Tracker::new();
    let mut v = vec![];
    let mut t = 1000;
    let mut push = |tr: &mut Tracker, g: Option<G>, v: &mut Vec<G>| {
        if let Some(g) = g {
            if cap && matches!(g, G::Down(_)) {
                tr.capture();
            }
            v.push(g);
        }
    };
    for (i, &(x, y)) in path.iter().enumerate() {
        let mut e = Event { points: 1, p: Point { x, y }, ..Event::default() };
        e.pts[0] = e.p;
        let g = tr.event(&e, t);
        push(&mut tr, g, &mut v);
        if i == 0 && hold_ms > 0 {
            for k in (10..=hold_ms).step_by(10) {
                let g = tr.tick(t + k);
                push(&mut tr, g, &mut v);
            }
            t += hold_ms;
        }
        t += 10;
    }
    let g = tr.event(&Event::default(), t + 30);
    push(&mut tr, g, &mut v);
    v
}

fn line(a: (u16, u16), b: (u16, u16), n: usize) -> Vec<(u16, u16)> {
    (0..=n).map(|i| ((a.0 as i32 + (b.0 as i32 - a.0 as i32) * i as i32 / n as i32) as u16, (a.1 as i32 + (b.1 as i32 - a.1 as i32) * i as i32 / n as i32) as u16)).collect()
}

/// Two-finger touch: frames of (finger a, finger b) points, a frame every
/// 10 ms, then released. `swap`: report the points in alternating slot
/// order (the controller's order is not stable).
fn touch2(frames: &[((u16, u16), (u16, u16))], swap: bool) -> Vec<G> {
    let mut tr = Tracker::new();
    let mut v = vec![];
    let mut t = 1000;
    for (i, &(a, b)) in frames.iter().enumerate() {
        let (p, q) = if swap && i % 2 == 1 { (b, a) } else { (a, b) };
        let mut e = Event { points: 2, p: Point { x: p.0, y: p.1 }, ..Event::default() };
        e.pts[0] = Point { x: p.0, y: p.1 };
        e.pts[1] = Point { x: q.0, y: q.1 };
        if let Some(g) = tr.event(&e, t) {
            v.push(g);
        }
        t += 10;
    }
    if let Some(g) = tr.event(&Event::default(), t + 30) {
        v.push(g);
    }
    v
}

/// Controller frames through the real Debounce and Tracker, as the main
/// loop: `seq` = (time, frame points; empty = a release frame), a poll without
/// a frame (idle + tick) every 5 ms in between. The app is the scope graph:
/// it captures every Down with raw moves (scope_page.rs). Returns the gestures.
fn frames_to_gestures(seq: &[(u64, Vec<(u16, u16)>)]) -> Vec<G> {
    let mut d = Debounce::new();
    let mut tr = Tracker::new();
    let mut v = vec![];
    let mut feed = |tr: &mut Tracker, g: Option<G>, v: &mut Vec<G>| {
        if let Some(g) = g {
            if matches!(g, G::Down(_)) {
                gesture::raw_next(true);
                tr.capture();
            }
            v.push(g);
        }
    };
    let end = seq.last().map(|s| s.0).unwrap_or(0) + 200;
    let mut k = 0;
    let mut t = seq.first().map(|s| s.0).unwrap_or(0);
    while t <= end {
        let ev = if k < seq.len() && seq[k].0 <= t {
            let pts = &seq[k].1;
            k += 1;
            let mut f = Frame { pressed: !pts.is_empty(), n: pts.len() as u8, ..Frame::default() };
            for (i, &(x, y)) in pts.iter().enumerate() {
                f.pts[i] = Point { x, y };
            }
            d.frame(&f, t)
        } else {
            let e = d.idle(t);
            let g = tr.tick(t);
            feed(&mut tr, g, &mut v);
            t += 5;
            e
        };
        if let Some(e) = ev {
            let g = tr.event(&e, t);
            feed(&mut tr, g, &mut v);
        }
    }
    v
}

/// Owner finding: after a pinch the finger lifted last kept dragging. Once a
/// touch had two fingers nothing but the pinch end may come out of it, in
/// any lift order, also when the controller reports a release between the
/// two lifts (a one-finger touch-down shortly after a multi-finger release
/// is the remaining finger, not a new touch).
fn pinch_lift_tests(ck: &mut Ck) {
    let one_finger = |v: &[G]| v.iter().any(|g| matches!(g, G::Down(_) | G::Tap(_) | G::LongPress(_) | G::Drag(..) | G::Move(..) | G::Grab(_) | G::Up(_) | G::SwipeLeft | G::SwipeRight | G::SwipeUp | G::SwipeDown));
    let ends = |v: &[G]| v.iter().filter(|g| matches!(g, G::PinchEnd(_))).count();
    // Horizontal spread 100 -> 200 px (16 ms reports), then one finger lifts
    // and the other moves 60 px down and 40 px right, then lifts.
    let pinch = |lift_a: bool, gap: Option<u64>, first_alone: bool| {
        let mut seq = vec![];
        let mut t = 1000;
        if first_alone {
            for _ in 0..3 {
                seq.push((t, vec![(150, 160)]));
                t += 16;
            }
        }
        for i in 0..=10u16 {
            seq.push((t, vec![(150 - 5 * i, 160), (250 + 5 * i, 162)]));
            t += 16;
        }
        if let Some(g) = gap {
            seq.push((t, vec![]));
            t += g;
        }
        let rest = if lift_a { (300u16, 162u16) } else { (100u16, 160u16) };
        for i in 0..=12u16 {
            seq.push((t, vec![(rest.0 + 3 * i, rest.1 + 5 * i)]));
            t += 16;
        }
        seq.push((t, vec![]));
        frames_to_gestures(&seq)
    };
    for (name, lift_a, gap, first) in [
        ("pinch, lift left, right finger moves", true, None, false),
        ("pinch, lift right, left finger moves", false, None, false),
        ("pinch, release frame + 40 ms gap, last finger moves", true, Some(40), false),
        ("pinch, release frame + 120 ms gap, last finger moves", false, Some(120), false),
        ("1 finger first, pinch, lift, last finger moves", true, None, true),
    ] {
        let v = pinch(lift_a, gap, first);
        let pinched = v.iter().any(|g| matches!(g, G::Pinch(..)));
        let after_cancel = v.iter().skip_while(|g| !matches!(g, G::Cancel | G::Pinch(..))).cloned().collect::<Vec<_>>();
        ck.t(&format!("{name}: pinch, one PinchEnd(true), nothing one-finger after it"), pinched && ends(&v) == 1 && v.contains(&G::PinchEnd(true)) && !one_finger(&after_cancel));
        if !first {
            ck.t(&format!("{name}: no one-finger gesture at all"), !one_finger(&v));
        }
    }
    // Two-finger tap (both land in one report, lift together or one by one).
    let v = frames_to_gestures(&[(1000, vec![(150, 160), (250, 160)]), (1016, vec![(150, 160), (250, 161)]), (1032, vec![(150, 160), (250, 161)]), (1048, vec![])]);
    ck.t("two-finger tap: nothing", v.is_empty());
    let v = frames_to_gestures(&[(1000, vec![(150, 160), (250, 160)]), (1016, vec![(150, 160), (250, 161)]), (1032, vec![(250, 161)]), (1048, vec![(250, 161)]), (1064, vec![])]);
    ck.t("two-finger tap, lifted one by one: nothing", v.is_empty());
    // A new one-finger touch well after a pinch is a normal touch.
    let mut seq = vec![];
    let mut t = 1000;
    for i in 0..=10u16 {
        seq.push((t, vec![(150 - 5 * i, 160), (250 + 5 * i, 162)]));
        t += 16;
    }
    seq.push((t, vec![]));
    t += 500;
    for i in 0..=5u16 {
        seq.push((t, vec![(200, 100 + 6 * i)]));
        t += 16;
    }
    seq.push((t, vec![]));
    let v = frames_to_gestures(&seq);
    ck.t("pinch, then a new touch 500 ms later: Down, moves, Up", v.iter().filter(|g| matches!(g, G::Down(_))).count() == 1 && v.iter().any(|g| matches!(g, G::Move(..))) && matches!(v.last(), Some(G::Up(_))));
}

fn pinch_tests(ck: &mut Ck) {
    // Horizontal spread 100 -> 200 px, slot order swapping every frame.
    let f: Vec<_> = (0..=10u16).map(|i| ((150 - 5 * i, 160), (250 + 5 * i, 162))).collect();
    let v = touch2(&f, true);
    let last = v.iter().rev().find_map(|g| if let G::Pinch(ax, s0, s) = g { Some((*ax, *s0, *s)) } else { None });
    ck.t("pinch: horizontal, span 100 -> 200 (swapped slots)", last == Some((Axis::H, 100, 200)));
    ck.t("pinch: ends with PinchEnd(true)", v.last() == Some(&G::PinchEnd(true)));
    ck.t("pinch: no power-off swipe", !v.iter().any(|g| matches!(g, G::MultiSwipeDown(_))));
    // Vertical squeeze 160 -> 80.
    let f: Vec<_> = (0..=8u16).map(|i| ((200, 80 + 5 * i), (202, 240 - 5 * i))).collect();
    let v = touch2(&f, false);
    let last = v.iter().rev().find_map(|g| if let G::Pinch(ax, s0, s) = g { Some((*ax, *s0, *s)) } else { None });
    ck.t("pinch: vertical, span 160 -> 80", last == Some((Axis::V, 160, 80)));
    // Two fingers moving down together (power-off swipe): no pinch.
    let f: Vec<_> = (0..=10u16).map(|i| ((150, 60 + 10 * i), (260, 62 + 10 * i))).collect();
    let v = touch2(&f, true);
    ck.t("2-finger swipe down: no pinch", !v.iter().any(|g| matches!(g, G::Pinch(..) | G::PinchEnd(_))));
    ck.t("2-finger swipe down: MultiSwipeDown(2)", v.last() == Some(&G::MultiSwipeDown(2)));
}

/// Pre-flash review 7389e8a B1: a pinch whose end was lost (MF / AUTO /
/// overlay mid-pinch) must not carry its index into the other axis. The
/// review's sequence: CH1 at 10 V/div (range 9), vertical pinch, (MF,
/// release, MF), horizontal pinch closing to ~0.68 -> the index must be a
/// valid timebase, never a range >= 10; and every pinch result stays in its
/// own table.
/// The trigger level keeps its volts across scale changes (owner
/// 2026-10-25: a V/div change kept the row, so the volts changed): V/div
/// up / down, off screen and back, probe x10, a CH1 position move, a level
/// drag, a reload at the saved scale; the runt level too.
fn trig_volts_tests(ck: &mut Ck) {
    let mut sc = scope::Scope::new();
    let d = scope::Settings::DEFAULT; // 1 V/div, pos 0, level row 105
    sc.apply(d, 0);
    let v0 = sc.trig_volts();
    let near = |a: f32, b: f32| (a - b).abs() <= 1e-4 * b.abs().max(1e-3);
    let mut seen = vec![];
    let mut ok = true;
    for r in [5usize, 7, 4, 9, 2, 0, 6] {
        let mut s = sc.s;
        s.range = r;
        sc.apply(s, 0);
        ok &= near(sc.trig_volts(), v0);
        seen.push((r, sc.s.trig_row));
    }
    let row = |r: usize| seen.iter().find(|x| x.0 == r).unwrap().1;
    ck.t(&format!("trigger level: V/div changes keep the volts, the row moves ({seen:?})"), ok && row(5) == 70 && row(7) == 123 && row(9) == 137 && row(6) == 105);
    ck.t("trigger level off screen after a zoom: the marker at the edge, the level kept, back on screen at its row", row(0) == -10 && row(2) == -10 && row(6) == 105);
    let mut s = sc.s;
    s.probe = 10.0;
    sc.apply(s, 0);
    let r10 = sc.s.trig_row;
    ck.t(&format!("trigger level: probe x10 keeps the tip volts (row {r10})"), near(sc.trig_volts(), v0) && r10 == 137);
    s.probe = 1.0;
    sc.apply(s, 0);
    ck.t("trigger level: probe back, row back", sc.s.trig_row == 105 && near(sc.trig_volts(), v0));
    // A CH1 drag moves the level row with the position (scope_page.rs).
    let mut s = sc.s;
    s.pos += 20;
    s.trig_row += 20;
    sc.preview(s);
    ck.t("trigger level: a CH1 position move keeps the volts", sc.s.trig_row == 125 && near(sc.trig_volts(), v0));
    let mut s = sc.s;
    s.range = 4; // 200 mV/div: the level (~1 V) is off screen (top)
    sc.apply(s, 0);
    let mut s = sc.s;
    s.pos -= 20;
    s.trig_row -= 20; // CH1 back while the level is clamped at the edge
    sc.preview(s);
    sc.apply(sc.s, 0);
    let mut s = sc.s;
    s.range = 6;
    sc.apply(s, 0);
    ck.t("trigger level: a CH1 move while the level is off screen keeps it", near(sc.trig_volts(), v0) && sc.s.trig_row == 105);
    // A level drag sets the volts.
    let mut s = sc.s;
    s.trig_row = 70;
    sc.preview(s);
    let v1 = sc.trig_volts();
    ck.t("trigger level: a drag sets new volts", near(v1, 70.0 * 4.0 / 141.0));
    // Reload: the saved row at the saved scale (0.5 V/div, row 70).
    let mut s = sc.s;
    s.range = 5;
    s.trig_row = 70;
    sc.levels_reset();
    sc.apply(s, 0);
    ck.t("trigger level: a loaded row converts at its saved scale", near(sc.trig_volts(), 70.0 * 2.0 / 141.0));
    // Runt: the second level the same way.
    let mut s = sc.s;
    s.range = 6;
    sc.apply(s, 0);
    let rv = sc.runt_volts();
    let mut s = sc.s;
    s.range = 7;
    sc.apply(s, 0);
    ck.t("runt level: V/div change keeps its volts", near(sc.runt_volts(), rv) && sc.s.runt_row != 70);
}

fn pinch_state_tests(ck: &mut Ck) {
    use scope_ui::{pinch_step, PinchState};
    let (tb, range, tb_min, tb_max) = (13usize, 9usize, 3usize, 24usize);
    let v = pinch_step(None, false, 160, 300, tb, range, tb_min, tb_max);
    ck.t("pinch V from range 9: vertical, range index", !v.horizontal && v.i <= 9);
    // The stale V state is fed to a new horizontal pinch (its end was lost).
    let h = pinch_step(Some(v), true, 200, 136, tb, range, tb_min, tb_max);
    ck.t("stale V + H pinch to 0.68: horizontal, from the timebase", h.horizontal && h.i0 == tb && (tb_min..=tb_max).contains(&h.i));
    // Exhaustive: any stored state x any ratio stays inside its table.
    let mut ok = true;
    for horizontal in [false, true] {
        for prev in [None, Some(PinchState { horizontal: true, i0: 29, i: 29 }), Some(PinchState { horizontal: false, i0: 9, i: 0 })] {
            for s in [1u16, 10, 50, 99, 136, 200, 400, 999] {
                for (t, r) in [(3usize, 0usize), (29, 9), (13, 6)] {
                    let n = pinch_step(prev, horizontal, 200, s, t, r, tb_min, 29);
                    ok &= n.horizontal == horizontal && if horizontal { (tb_min..=29).contains(&n.i) } else { n.i <= scope_ui::RANGE_MAX };
                }
            }
        }
    }
    ck.t("pinch_step: every result inside its own table", ok);
    // Pinch modes (DBG:PINCH): STEP one table step per 1.25x, several in a
    // gesture; LIVE snaps to the nearest step in log scale; both inside the
    // table for any ratio.
    use scope_ui::{pinch_step_mode, PINCH_LIVE, PINCH_STEP};
    let st = |s: u16| pinch_step_mode(PINCH_STEP, None, true, 200, s, 13, 6, 3, 29).i;
    ck.t("pinch step mode: 1.2x nothing, 1.25x one step in, 1.6x two, 0.8x one out", st(240) == 13 && st(250) == 12 && st(320) == 11 && st(160) == 14);
    let sv = |s: u16| pinch_step_mode(PINCH_STEP, None, false, 200, s, 13, 6, 3, 29).i;
    ck.t("pinch step mode, vertical: 1.25x one range down", sv(250) == 5 && sv(200) == 6);
    let lv = |s: u16| pinch_step_mode(PINCH_LIVE, None, true, 200, s, 13, 6, 3, 29).i;
    ck.t("pinch live mode: snaps to the nearest step (1.1x stays, 2x one step, 0.5x one out)", lv(220) == 13 && lv(400) == 12 && lv(100) == 14);
    let mut ok = true;
    for mode in [PINCH_STEP, PINCH_LIVE] {
        for horizontal in [false, true] {
            for s in [1u16, 10, 99, 200, 400, 999] {
                let n = pinch_step_mode(mode, None, horizontal, 200, s, 13, 6, 3, 29);
                ok &= if horizontal { (3..=29).contains(&n.i) } else { n.i <= scope_ui::RANGE_MAX };
            }
        }
    }
    ck.t("pinch modes: every result inside its own table", ok);
}

fn raw_tests(ck: &mut Ck) {
    // Raw-move capture: every moved report is a Move from the previous point,
    // from the first pixel (no slop), and the release is Up.
    let mut tr = Tracker::new();
    let mut v = vec![];
    let pts = [(100u16, 100u16), (101, 100), (101, 100), (103, 98), (120, 98)];
    for (i, &(x, y)) in pts.iter().enumerate() {
        let mut e = Event { points: 1, p: Point { x, y }, ..Event::default() };
        e.pts[0] = e.p;
        if let Some(g) = tr.event(&e, 1000 + 10 * i as u64) {
            if matches!(g, G::Down(_)) {
                gesture::raw_next(true);
                tr.capture();
            }
            v.push(g);
        }
    }
    if let Some(g) = tr.event(&Event::default(), 1100) {
        v.push(g);
    }
    let p = |x, y| Point { x, y };
    ck.t(
        "raw moves: Down, Move x3 (none for a still report), Up",
        v == vec![G::Down(p(100, 100)), G::Move(p(100, 100), p(101, 100)), G::Move(p(101, 100), p(103, 98)), G::Move(p(103, 98), p(120, 98)), G::Up(p(120, 98))],
    );
    // A plain capture after a raw one has no moves.
    let v = touch(&line((100, 100), (100, 160), 6), true, 0);
    ck.t("plain capture: Drag, no Move", v.iter().any(|g| matches!(g, G::Drag(..))) && !v.iter().any(|g| matches!(g, G::Move(..))));
}

/// Side keys from frames through the debounce and the tracker (main.rs
/// order: an event, else a tick, every 10 ms poll); returns the key gestures.
fn key_run(seq: &[(u64, Option<Frame>)]) -> Vec<G> {
    let mut d = Debounce::new();
    let mut tr = Tracker::new();
    let mut out = vec![];
    for (t, f) in seq {
        let e = match f {
            Some(f) => d.frame(f, *t),
            None => d.idle(*t),
        };
        let g = match e {
            Some(e) => tr.event(&e, *t),
            None => tr.tick(*t),
        };
        if let Some(g) = g.filter(|g| matches!(g, G::KeyDown(_) | G::KeyUp(_))) {
            out.push(g);
        }
    }
    out
}

fn key_tests(ck: &mut Ck) {
    use G::{KeyDown as D, KeyUp as U};
    // RUN released, AUTO pressed 20 ms later (inside the 40 ms debounce):
    // the debounce goes from RUN straight to AUTO. Was KeyDown(1) twice and
    // no KeyUp(1) (AUTO acted as RUN, RUN's hold timer then toggled AutoHold).
    let mut s = vec![(1000, Some(key(1)))];
    s.extend(idle(1010, 1150));
    s.push((1150, Some(released())));
    s.push((1160, None));
    s.push((1170, Some(key(2))));
    s.extend(idle(1180, 1300));
    s.push((1300, Some(released())));
    s.extend(idle(1310, 1400));
    ck.t("keys: RUN then AUTO 20 ms after its release", key_run(&s) == [D(1), U(1), D(2), U(2)]);
    // MENU -> MF with no release frame between (a frame lost, or a main-loop
    // stall that read only the newer one): the lower code must not repeat.
    let mut s = vec![(1000, Some(key(0)))];
    s.extend(idle(1010, 1150));
    s.push((1150, Some(key(3))));
    s.extend(idle(1160, 1300));
    s.push((1300, Some(released())));
    s.extend(idle(1310, 1400));
    ck.t("keys: MENU -> MF without a release", key_run(&s) == [D(0), U(0), D(3), U(3)]);
    // The catch-up half comes at the next tick (<= 10 ms in main.rs).
    let mut tr = Tracker::new();
    let ev = |k: u8| Event { keys: k, ..Event::default() };
    let a = tr.event(&ev(2), 0);
    let b = tr.event(&ev(4), 100);
    let c = tr.tick(110);
    let d = tr.tick(120);
    ck.t("keys: up first, down at the next tick", a == Some(D(1)) && b == Some(U(1)) && c == Some(D(2)) && d.is_none());
    // A plain press still gives one down and one up.
    // (Times from 1000: a key report in the first 300 ms counts as after a
    // touch, Debounce::frame.)
    let mut s = vec![(1000, Some(key(2))), (1010, Some(key(2)))];
    s.extend(idle(1020, 1100));
    s.push((1100, Some(released())));
    s.extend(idle(1110, 1200));
    ck.t("keys: plain AUTO press", key_run(&s) == [D(2), U(2)]);
}

fn gesture_tests(ck: &mut Ck) {
    pinch_tests(ck);
    pinch_lift_tests(ck);
    pinch_state_tests(ck);
    raw_tests(ck);
    key_tests(ck);
    let tap = |v: &[G]| v.iter().any(|g| matches!(g, G::Tap(_)));
    ck.t("still tap", tap(&touch(&[(100, 100), (100, 101), (101, 100)], false, 0)));
    ck.t("tap point = down point", touch(&[(100, 100), (110, 104)], false, 0).last() == Some(&G::Tap(Point { x: 100, y: 100 })));
    ck.t("roll 25 px: still a tap (uncaptured)", tap(&touch(&line((100, 100), (100, 125), 5), false, 0)));
    ck.t("moved 45 px: no tap", !tap(&touch(&line((100, 100), (100, 145), 5), false, 0)));
    ck.t("swipe 80 px left", touch(&line((300, 100), (220, 104), 8), false, 0).last() == Some(&G::SwipeLeft));
    ck.t("captured: Up, no tap", { let v = touch(&[(100, 100), (101, 100)], true, 0); !tap(&v) && matches!(v.last(), Some(G::Up(_))) });
    let v = touch(&[(100, 100), (100, 100)], false, 800);
    ck.t("held 800 ms: LongPress", v.iter().any(|g| matches!(g, G::LongPress(_))));
    // TAP_AFTER_LONG on: an uncaptured long press still taps on release.
    ck.t("held 800 ms: tap after LongPress (uncaptured)", tap(&v));
    let v = touch(&[(100, 100), (100, 100)], true, 800);
    ck.t("held 800 ms captured: no tap", !tap(&v));
    ck.t("held 600 ms: tap", tap(&touch(&[(100, 100), (100, 100)], false, 600)));
    let v = touch(&[(100, 100), (100, 100)], false, 0);
    ck.t("one Down, one Tap per touch", v.iter().filter(|g| matches!(g, G::Down(_))).count() == 1 && v.iter().filter(|g| matches!(g, G::Tap(_))).count() == 1);
}

// ------------------------------------------------------------------ 3. log

fn log_tests(ck: &mut Ck) {
    touch::log_clear();
    let _ = touch(&[(100, 100), (100, 101)], false, 0);
    touch::log(b'F', 5, b'e', 1, 0x06, Point { x: 7, y: 8 });
    let mut b = vec![0u8; 6144];
    let n = touch::log_text(&mut b, 1234);
    let s = std::str::from_utf8(&b[..n]).unwrap();
    if ck.verbose {
        print!("{s}");
    }
    ck.t("log: header", s.starts_with("TOUCH,1234,3,3\n"));
    ck.t("log: down / tap lines", s.contains(",G,D,0,100,100\n") && s.contains(",G,T,0,100,100\n"));
    ck.t("log: frame line", s.contains("5,F,e,1,7,8,06\n"));
    for i in 0..1000u32 {
        touch::log(b'E', i as u64, 1, 0, 0, Point { x: 479, y: 319 });
    }
    let n = touch::log_text(&mut b, 4_000_000_000);
    ck.t("log: full ring fits 6 KB", n < 6144 && std::str::from_utf8(&b[..n]).unwrap().lines().count() == touch::LOG_N + 1);
    let n = touch::log_text(&mut b[..100], 0);
    ck.t("log: truncated at a line end", b[n - 1] == b'\n' && n <= 100);
    touch::log_clear();
}

// ------------------------------------------------------- 4. hit vs. drawn

#[derive(Clone, Copy, Debug)]
struct Bx {
    x0: i32,
    y0: i32,
    x1: i32,
    y1: i32,
}

/// Ink boxes of the shapes (sf::rbox) on the emulator screen.
fn shapes() -> Vec<Bx> {
    let e = emu();
    let mut m: std::collections::BTreeMap<u32, Bx> = Default::default();
    for y in 0..H {
        for x in 0..W {
            let o = e.owner[y * W + x];
            if o == 0 || e.kinds[(o >> 8) as usize - 1] != probe::SHAPE {
                continue;
            }
            let b = m.entry(o >> 8).or_insert(Bx { x0: x as i32, y0: y as i32, x1: x as i32, y1: y as i32 });
            b.x0 = b.x0.min(x as i32);
            b.y0 = b.y0.min(y as i32);
            b.x1 = b.x1.max(x as i32);
            b.y1 = b.y1.max(y as i32);
        }
    }
    m.into_values().collect()
}

/// Check the page now on the emulator against its hit function. `only`:
/// shapes to consider (e.g. one screen region); `want`: targets that must
/// be drawn.
fn page(ck: &mut Ck, name: &str, hit: &dyn Fn(u16, u16) -> Option<u32>, only: &dyn Fn(&Bx) -> bool, want: &[u32]) {
    let sh: Vec<Bx> = shapes().into_iter().filter(|b| only(b)).collect();
    let mut seen: Vec<(u32, Bx)> = vec![];
    let mut report = String::new();
    for b in &sh {
        let (cx, cy) = ((b.x0 + b.x1) / 2, (b.y0 + b.y1) / 2);
        let Some(id) = hit(cx as u16, cy as u16) else { continue };
        // Nested shapes (a box inside a button) share the button's target.
        if seen.iter().any(|(i, o)| *i == id && o.x0 <= b.x0 && o.y0 <= b.y0 && o.x1 >= b.x1 && o.y1 >= b.y1) {
            continue;
        }
        let mut bad = 0;
        let mut first = None;
        for y in b.y0..=b.y1 {
            for x in b.x0..=b.x1 {
                let h = hit(x as u16, y as u16);
                if h != Some(id) {
                    bad += 1;
                    first.get_or_insert((x, y, h));
                }
            }
        }
        ck.t(&format!("{name}: target {id} hit on all of its box {b:?} ({bad} px off, first {first:?})"), bad == 0);
        let dup = seen.iter().find(|(i, _)| *i == id).map(|x| x.1);
        ck.t(&format!("{name}: target {id} drawn once ({b:?} and {dup:?})"), dup.is_none());
        // Reach of the hit area past the drawn box (left, top, right, bottom).
        let (mut hx0, mut hy0, mut hx1, mut hy1) = (i32::MAX, i32::MAX, i32::MIN, i32::MIN);
        for y in 0..H as i32 {
            for x in 0..W as i32 {
                if hit(x as u16, y as u16) == Some(id) {
                    hx0 = hx0.min(x);
                    hy0 = hy0.min(y);
                    hx1 = hx1.max(x);
                    hy1 = hy1.max(y);
                }
            }
        }
        report += &format!("    {id:>4} drawn {:>3},{:>3}..{:>3},{:>3}  hit reach L{:+} T{:+} R{:+} B{:+}\n", b.x0, b.y0, b.x1, b.y1, b.x0 - hx0, b.y0 - hy0, hx1 - b.x1, hy1 - b.y1);
        seen.push((id, *b));
    }
    for w in want {
        ck.t(&format!("{name}: target {w} is drawn"), seen.iter().any(|(i, _)| i == w));
    }
    // Dead pixels (a tap there does nothing): `gap` = within 6 px of two
    // buttons (between neighbours), `rim` = within 3 px of one button's
    // outside edge (a near miss).
    let (mut gap, mut rim) = (0, 0);
    for y in 0..H as i32 {
        for x in 0..W as i32 {
            if hit(x as u16, y as u16).is_some() {
                continue;
            }
            let near = |d: i32| seen.iter().filter(|(_, b)| x >= b.x0 - d && x <= b.x1 + d && y >= b.y0 - d && y <= b.y1 + d).count();
            if near(6) >= 2 {
                gap += 1;
            } else if near(3) >= 1 {
                rim += 1;
            }
        }
    }
    println!("  {name}: {} buttons; dead px: {gap} between buttons, {rim} on the 3 px rim", seen.len());
    if ck.verbose {
        print!("{report}");
    }
}

fn all(_: &Bx) -> bool {
    true
}

fn hit_tests(ck: &mut Ck) {
    host::map_flash();
    let o = |v: Option<usize>| v.map(|i| i as u32);
    // Main menu.
    host::reset(0);
    ui::main_menu("DMM + Scope");
    page(ck, "menu", &|x, y| o(ui::main_menu_hit(x, y)), &all, &[0, 1, 2, 3, 4, 5]);
    // Settings 1/3, 2/3 and 3/3, Hold & Limits (list pages).
    let vals = ["10 min", "1 hour", "100%", "100%", "OFF", "DMM + Scope", "Auto", "Hold 300 ms", "", "", ""];
    for p in 0..3u8 {
        host::reset(0);
        ui::settings(p, &vals[..[ui::SETTINGS.len(), ui::SETTINGS2.len(), ui::SETTINGS3.len()][p as usize]]);
        let n = [ui::SETTINGS.len(), ui::SETTINGS2.len(), ui::SETTINGS3.len()][p as usize];
        let want: Vec<u32> = (0..n as u32).collect();
        page(ck, &format!("settings {}", p + 1), &|_, y| o(ui::settings_hit(p, y)), &|b| b.y0 >= 40, &want);
    }
    // Backlight / volume steppers (ui-review #12): the "-" key (x 210..263)
    // steps down, the "+" key (410..463) up; the label side is "-".
    ck.t("settings stepper: '-' key and label step down", ui::settings_step(237) == -1 && ui::settings_step(20) == -1);
    ck.t("settings stepper: '+' key steps up", ui::settings_step(437) == 1 && ui::settings_step(463) == 1);
    // Backlight walks the manual's grid both ways and stays on it at the ends (#91).
    let mut up = vec![55u8];
    for _ in 0..6 { up.push(ui::backlight_step(*up.last().unwrap(), 1)); }
    ck.t("backlight +: 55 65 75 85 95 100 100", up == [55, 65, 75, 85, 95, 100, 100]);
    let mut dn = vec![100u8];
    for _ in 0..11 { dn.push(ui::backlight_step(*dn.last().unwrap(), -1)); }
    ck.t("backlight -: 100 95 85 ... 15 10 10", dn == [100, 95, 85, 75, 65, 55, 45, 35, 25, 15, 10, 10]);
    ck.t("backlight: 10 + gives 15, 100 - gives 95", ui::backlight_step(10, 1) == 15 && ui::backlight_step(100, -1) == 95);
    ck.t("backlight: off-grid 20 joins the grid (25 up, 15 down)", ui::backlight_step(20, 1) == 25 && ui::backlight_step(20, -1) == 15);
    host::reset(0);
    ui::list_page("Hold & Limits", &ui::LIMIT_ROWS, &["ON"; 9]);
    page(ck, "limits", &|_, y| o(ui::list_hit(y, ui::LIMIT_ROWS.len())), &|b| b.y0 >= 40, &(0..9).collect::<Vec<u32>>());
    // Record page (decision 2026-10-22 #1): five rows (the stepper keys
    // nest in their rows), its steppers' "-" key at x 150..203.
    host::reset(0);
    ui::record_page(["On change", "1 %", "Until stopped"], "about 62 h left", false);
    page(ck, "record", &|_, y| o(ui::list_hit(y, ui::RECORD_ROWS.len())), &|b| b.y0 >= 40, &[0, 1, 2, 3, 4]);
    ck.t("record stepper: '-' key and label step down", ui::record_step(177) == -1 && ui::record_step(20) == -1);
    ck.t("record stepper: '+' key steps up", ui::record_step(437) == 1 && ui::record_step(463) == 1);
    // Range pages and the temperature page. V / A with the ad2-isr builds'
    // Wave button (200) and the A page's Inrush (201), in App::tap's order
    // (Wave, Inrush, then range_hit).
    for b in [dmm::Button::Volt, dmm::Button::Amp, dmm::Button::Ohm, dmm::Button::Cap] {
        host::reset(0);
        let n = ui::range_items(b).len();
        ui::range_page(b, Some(0), 0, None);
        let va = matches!(b, dmm::Button::Volt | dmm::Button::Amp);
        if va {
            ui::wave_button();
        }
        if b == dmm::Button::Amp {
            ui::inrush_button();
        }
        let hit = move |x, y| {
            if va && ui::wave_hit(x, y) {
                return Some(200);
            }
            if b == dmm::Button::Amp && ui::inrush_hit(x, y) {
                return Some(201);
            }
            ui::range_hit(b, None, x, y).map(|r| match r {
                Ok(i) => i as u32,
                Err(k) => 100 + k as u32,
            })
        };
        let mut want: Vec<u32> = (0..n as u32).collect();
        want.extend([102, 104, 107]); // OK, Limits, Record
        if va {
            want.push(200);
        }
        if b == dmm::Button::Amp {
            want.extend([105, 201]); // Fuse check, Inrush
        }
        if matches!(b, dmm::Button::Volt | dmm::Button::Amp) {
            want.extend([100, 101, 103, 106]); // DC / AC / AC+DC / AUTO (AC/DC)
        }
        if b == dmm::Button::Ohm {
            want.extend([100, 101]); // 0.90 mA / 0.28 mA ON
        }
        page(ck, &format!("range {n} items"), &hit, &|b| b.y0 >= 40, &want);
    }
    // Resistor page with 600 Mohm chosen: no test-current buttons (their
    // line is text), nothing to hit there.
    host::reset(0);
    let n = ui::range_items(dmm::Button::Ohm).len();
    ui::range_page(dmm::Button::Ohm, Some(n - 1), 0, None);
    let hit = move |x, y| {
        ui::range_hit(dmm::Button::Ohm, Some(n - 1), x, y).map(|r| match r {
            Ok(i) => i as u32,
            Err(k) => 100 + k as u32,
        })
    };
    let mut want: Vec<u32> = (0..n as u32).collect();
    want.extend([102, 104, 107]);
    page(ck, "range ohm 600M", &hit, &|b| b.y0 >= 40, &want);
    // Power Settings: every button, the bottom row's five included.
    host::reset(0);
    let pv = dmm::pw::PowerVals { v_auto: false, i_auto: true, ..dmm::pw::PowerVals::ZERO };
    ui::pw::settings_page(&pv);
    let want: Vec<u32> = (0..=4).chain(8..=14).chain(22..=32).filter(|&c| c != 21).collect();
    page(ck, "power settings", &|x, y| ui::pw::settings_hit(x, y).map(|c| c as u32), &|b| b.y0 >= 40, &want);
    ck.t("range ohm 600M: the test-current row is dead", (12..470).step_by(20).all(|x| hit(x, 200).is_none()));
    host::reset(0);
    // Temperature page: every key of both layouts (NTC user adds R25 / Beta).
    for k in [0u8, 6] {
        host::reset(0);
        ui::temp_page(&ui::TempPage { sensor: k, offs: -12.5, unit_f: false, r25: 10_000.0, beta: 3950.0, info: "info" });
        let mut want: Vec<u32> = (0..=10).collect();
        if k == 6 {
            want.extend([11, 12]);
        }
        want.extend([253, 254, 255]);
        page(ck, &format!("temperature {k}"), &move |x, y| ui::temp_hit(k, x, y).map(|v| v as u32), &|b| b.y0 >= 40 && b.y1 < 280, &want);
    }
    // Keypads.
    for kind in [ui::KP_FREQ, ui::KP_NUM, ui::KP_SIGNED] {
        host::reset(0);
        ui::keypad(kind, "Vpp", "1.5", "V", true);
        page(ck, &format!("keypad {kind}"), &|x, y| ui::keypad_hit(kind, x, y).map(|c| c as u32), &|b| b.x0 >= 224, &[b'0' as u32, b'9' as u32, b'=' as u32, b'<' as u32]);
    }
    // DMM page: function row (ui::button_hit) and the app.rs View::Dmm tap
    // order for the rest (copied from App::tap: REL box, MIN/MAX column,
    // history row, rate chip, chart axis column, function row).
    host::reset(0);
    let mut dv = ui::DmmView::new();
    dv.draw_static(dmm::Button::Volt, 1, false, true);
    ui::history(&[(1.0, 3, "V"); 5], 5, 4);
    let side = ui::Side { max: Some(1.0), avg: Some(1.0), min: Some(1.0), sd: Some(1.0), rel: Some(1.0), secs: Some(5), rec: false };
    let pts: Vec<f32> = (0..321).map(|i| i as f32).collect();
    ui::chart(&mut ui::ChartCache::new(), &pts, 3, 1, Some(&side), "V", ui::RateChip { rate: 1, period_ms: 100, applies: true });
    let dmm_hit = |x: u16, y: u16| -> Option<u32> {
        if ui::value_hit(x, y) {
            Some(105) // main value: resolution / rate
        } else if x >= 402 && (205..241).contains(&y) {
            Some(100) // REL
        } else if x >= 402 && (175..279).contains(&y) {
            Some(101) // MIN/MAX column
        } else if (153..172).contains(&y) {
            Some(102) // history row (one target for all five boxes)
        } else if ui::rate_chip_hit(x, y) {
            Some(103)
        } else if x < ui::CHART_X && (176..280).contains(&y) {
            Some(104) // chart rate
        } else {
            o(ui::button_hit(x, y))
        }
    };
    page(ck, "dmm function row", &dmm_hit, &|b| b.y0 >= ui::BTN_Y as i32 - 4, &[0, 1, 2, 3, 4, 5, 6]);
    page(ck, "dmm REL + rate chip", &dmm_hit, &|b| b.y0 >= 175 && b.y1 < 279, &[100, 103]);
    // Power meter, AC+DC parts layout (doc 3.17: lower row 170.., chart
    // 192.., side column without VAh): the App::tap order for the power
    // page (pw_tap: energy side box, value, side column / lower row
    // swallowed; the long press on the energy row = energy_hit; then the
    // rate chip, the chart's left column, the function row).
    for parts in [false, true] {
        host::reset(0);
        ui::set_pw_parts(parts);
        let mut dv = ui::DmmView::new();
        dv.power = true;
        dv.draw_static(dmm::Button::Power, 4, true, false);
        let mut pv = dmm::pw::PowerVals { coup: dmm::pw::COUP_ACDC, v: 12.0, i: 2.0, p: 24.0, s: 24.0, pf: 1.0, vr: 1, ik: 4, ..dmm::pw::PowerVals::ZERO };
        pv.vp = [12.0, 0.0];
        pv.ip = [2.0, 0.0];
        pv.pp = [24.0, 0.0];
        let (main, v, i, si) = dmm::pw::reading_vals(&pv);
        let r = dmm::Reading { mode: 4, range: 0, main, aux1: Some(v), aux2: Some(i), si, auto: true, auto_plus: false, acdc: false, cpl_auto: false, seq: 0, avg_n: 0, alert: sentinel::Alert::None, power: Some(pv), prov: false, open: false };
        dv.reading(&r, 0, 0);
        let mut pc = ui::pw::PwCache::new();
        ui::chart(&mut ui::ChartCache::new(), &pts, 3, 1, None, "W", ui::RateChip { rate: 1, period_ms: 100, applies: true });
        ui::pw::side(&mut pc, &pv);
        let pw_hit = |x: u16, y: u16| -> Option<u32> {
            if ui::pw::side_box_hit(x, y) {
                Some(110) // energy run / pause
            } else if ui::value_hit(x, y) {
                Some(105)
            } else if ui::pw::energy_hit(x, y) {
                Some(107) // energy box (long press: reset)
            } else if (x >= 402 && (175..279).contains(&y)) || ui::pw::row_hit(y) {
                None // swallowed
            } else if ui::rate_chip_hit(x, y) {
                Some(103)
            } else if x < ui::CHART_X && ui::chart_rows().contains(&y) {
                Some(104)
            } else {
                o(ui::button_hit(x, y))
            }
        };
        let y0 = if parts { 165 } else { 148 };
        page(ck, &format!("power page parts {parts}"), &pw_hit, &move |b| b.y0 >= y0 && b.y1 < 279 && (b.x0 < 124 || b.x0 >= 402), &[103, 104, 107, 110]);
        page(ck, &format!("power page parts {parts} function row"), &pw_hit, &|b| b.y0 >= ui::BTN_Y as i32 - 4, &[0, 1, 2, 3, 4, 5, 6]);
    }
    ui::set_pw_parts(false);
    // Combined view: the DMM function band over the replica page.
    host::reset(0);
    let mut cm = scope_ui::Model::DEFAULT;
    cm.combined = true;
    scope_ui::invalidate();
    scope_ui::update(&cm);
    ui::func_band(dmm::Button::Volt, 1, false, true);
    page(ck, "function band", &|x, y| o(ui::func_band_hit(x, y)), &|b| b.y0 >= ui::FUNC_Y as i32, &[0, 1, 2, 3, 4, 5, 6]);
    // Stock-replica scope page: right panel (4 px vertical slop), MF popup
    // tabs, CH1 popup.
    host::reset(0);
    scope_ui::invalidate();
    scope_ui::update(&scope_ui::Model::DEFAULT);
    page(ck, "replica panel", &|x, y| scope_ui::panel_hit(x, y).map(|k| k as u32), &|b| b.x0 >= 414, &[0, 1, 2, 3, 4, 5, 6]);
    let pv = scope_ui::PopVals { meas: [0, 10, 9, 11], meas_n: 4, roll: false, persist: 0, bright: 100, grid: 50, rising: true, mode: 0, ac: false, probe: 3, colour: 2, math_colour: 1, pressed_cell: None, math_mode: 5, fft_hz: 3, hz_opts: [scope_ui::Txt::EMPTY; 4], fft_win: 2, fft_vrms: false, ttype: 0, pcond: 0, pwidth: 1e-3, runt_v: 0.0, tsrc: 0, gen_trace: false, ch2_on: false, ch2_vdiv: scope_ui::Txt::EMPTY };
    for tab in 0..4u8 {
        host::reset(0);
        scope_ui::draw_mf(tab, &pv);
        let hit = move |x: u16, y: u16| match scope_ui::mf_hit(tab, &pv, x as i16, y as i16) {
            scope_ui::MfHit::Tab(t) => Some(1000 + t as u32),
            scope_ui::MfHit::Cell(i) => Some(i as u32),
            scope_ui::MfHit::SelCell(i) => Some(100 + i as u32),
            scope_ui::MfHit::Clear => Some(200),
            scope_ui::MfHit::Default => Some(201),
            scope_ui::MfHit::Roll(on) => Some(300 + on as u32),
            scope_ui::MfHit::Persist(i) => Some(310 + i as u32),
            scope_ui::MfHit::Bright => Some(320),
            scope_ui::MfHit::Grid => Some(321),
            scope_ui::MfHit::Rising(r) => Some(330 + r as u32),
            scope_ui::MfHit::Mode(m) => Some(340 + m as u32),
            scope_ui::MfHit::PriScr => Some(350),
            scope_ui::MfHit::SaveCsv => Some(351),
            _ => None,
        };
        page(ck, &format!("replica MF tab {tab}"), &hit, &|b| b.y0 >= 21, &[]);
    }
    for tab in 0..3u8 {
        host::reset(0);
        scope_ui::draw_ch1(tab, &pv);
        let hit = move |x: u16, y: u16| match scope_ui::side_hit(false, tab, &pv, x as i16, y as i16) {
            scope_ui::SideHit::Tab(t) => Some(1000 + t as u32),
            scope_ui::SideHit::Coupling(c) => Some(c as u32),
            _ => None,
        };
        page(ck, &format!("replica CH1 tab {tab}"), &hit, &|b| b.x0 >= 215 && b.y0 >= 50, &[1000, 1001, 1002]);
    }
    // Generator page: value boxes (ui::gen_box_hit) + the wave box (app.rs
    // View::Gen: x < 243, y 40..82).
    host::reset(0);
    let g = gen::Gen::new();
    ui::gen_static(&g);
    let gen_hit = |x: u16, y: u16| if x < 243 && (40..82).contains(&y) { Some(100) } else { o(ui::gen_box_hit(x, y)) };
    page(ck, "generator", &gen_hit, &|b| b.y0 >= 36, &[0, 1, 2, 3, 100]);
    // RUN/STOP label in the bar (ui-review #9): tappable, the status
    // (uptime from x 186) is not.
    ck.t("generator: RUN/STOP label tappable", ui::gen_run_hit(40, 18) && ui::gen_run_hit(4, 2));
    ck.t("generator: uptime not a RUN/STOP tap", !ui::gen_run_hit(186, 18) && !ui::gen_run_hit(40, 36));
}

// ------------------------------------------------- 5. timebase tap step

/// opendm40 timebase tap step on the replica scope page (docs/scope-scale-ux.md
/// sec. 3; scope_ui `graph_press`, `top_badge`, `Arm`, `tb_hit`,
/// `tb_scroll`, the functions scope_page.rs runs): corner / halves hits
/// against the drawn tabs, marker priority, badge taps, the hold repeat,
/// the timebase list rows and scroll, the persisted mode.
fn tap_step_tests(ck: &mut Ck) {
    use scope_ui::{self as su, Press};
    // (The flash map is set up by hit_tests.)
    let d = su::Model::DEFAULT;
    let gp = |m: &su::Model, mode: u8, x: i16, y: i16| su::graph_press(m, mode, 0, 0, false, x, y);
    let step = |p: Press| match p {
        Press::Step(true) => Some(1u32),
        Press::Step(false) => Some(2),
        _ => None,
    };
    // Hit maps: every pixel of each drawn tab arms its side (Corners, and
    // Halves where the tab is a preview), both views.
    for combined in [false, true] {
        for mode in [su::TAP_CORNERS, su::TAP_HALVES] {
            let mut m = d;
            m.combined = combined;
            m.tabs = su::tabs_of(su::TAP_CORNERS, m.tb, 24, false, None);
            host::reset(0);
            su::invalidate();
            su::graph().clear();
            su::update(&m);
            let hit = move |x: u16, y: u16| if su::INNER.has(x as i16, y as i16) { step(su::graph_press(&m, mode, 0, 0, false, x as i16, y as i16)) } else { None };
            page(ck, &format!("tabs mode {mode} combined {combined}"), &hit, &|b| b.y0 >= 228 && b.y1 <= 300 && b.x1 < 414, &[1, 2]);
        }
    }
    let inside = |a: stockfont::Rect, b: stockfont::Rect| a.x >= b.x && a.y >= b.y && a.x + a.w <= b.x + b.w && a.y + a.h <= b.y + b.h;
    ck.t("tabs inside their corner zones", inside(su::TAB_L, su::ZONE_L) && inside(su::TAB_R, su::ZONE_R));
    ck.t("corner zones inside the graph area, left after the strip zones (x > 22)", su::ZONE_L.x > 22 && su::ZONE_R.x + su::ZONE_R.w <= 414 && su::ZONE_L.y + su::ZONE_L.h <= 301);
    // Zones per mode.
    let c = su::TAP_CORNERS;
    ck.t("corners: bottom-left slower, bottom-right faster", gp(&d, c, 40, 290) == Press::Step(true) && gp(&d, c, 400, 290) == Press::Step(false));
    ck.t("corners: zone edges (23..102 / 333..412, y 228..300)", gp(&d, c, 23, 228) == Press::Step(true) && gp(&d, c, 102, 300) == Press::Step(true) && gp(&d, c, 103, 290) == Press::None && gp(&d, c, 40, 227) == Press::None && gp(&d, c, 333, 300) == Press::Step(false) && gp(&d, c, 412, 228) == Press::Step(false) && gp(&d, c, 332, 290) == Press::None);
    ck.t("corners: a tap elsewhere in the graph does nothing", gp(&d, c, 200, 290) == Press::None && gp(&d, c, 100, 150) == Press::None && gp(&d, c, 300, 60) == Press::None);
    let h = su::TAP_HALVES;
    ck.t("halves: left half slower, right half faster (stock x <= 207)", gp(&d, h, 100, 150) == Press::Step(true) && gp(&d, h, 207, 60) == Press::Step(true) && gp(&d, h, 208, 60) == Press::Step(false) && gp(&d, h, 400, 290) == Press::Step(false));
    ck.t("off: no tap step anywhere", [(40, 290), (400, 290), (100, 150), (300, 150)].iter().all(|&(x, y)| gp(&d, su::TAP_OFF, x, y) == Press::None));
    let mut cm = d;
    cm.combined = true;
    ck.t("corners in the combined view (same bottom edge)", gp(&cm, c, 40, 290) == Press::Step(true) && gp(&cm, c, 400, 290) == Press::Step(false));
    // Marker priority: the object under the finger wins over the zones.
    for mode in [su::TAP_CORNERS, su::TAP_HALVES] {
        let mut m = d;
        m.ycur = Some([270, 100]);
        ck.t(&format!("mode {mode}: Y1 cursor line across the left corner wins"), gp(&m, mode, 40, 290) == Press::Marker(true, 3));
        let mut m = d;
        m.xcur = Some([30, 385]);
        ck.t(&format!("mode {mode}: X cursor lines in both corners win"), gp(&m, mode, 44, 260) == Press::Marker(false, 2) && gp(&m, mode, 399, 260) == Press::Marker(false, 3));
        let mut m = d;
        m.pos = 300;
        m.trig_row = 300;
        ck.t(&format!("mode {mode}: CH1 / level markers clamped at the bottom keep their zone (x <= 22)"), matches!(gp(&m, mode, 15, 292), Press::Marker(true, _)));
        ck.t(&format!("mode {mode}: trigger level marker zone (right edge)"), gp(&d, mode, 406, 125) == Press::Marker(true, 1) && gp(&d, mode, 6, 125) != Press::Marker(true, 1));
        let mut m = d;
        m.trig_row = 300;
        ck.t(&format!("mode {mode}: trigger level clamped at the bottom wins over the right corner"), gp(&m, mode, 405, 292) == Press::Marker(true, 1));
        ck.t(&format!("mode {mode}: T marker zone"), gp(&d, mode, 214, 30) == Press::Marker(false, 0));
        let mut m = d;
        m.trig_px = 40;
        ck.t(&format!("mode {mode}: still a step next to the cursor-free corner"), gp(&m, mode, 60, 280) == Press::Step(true));
    }
    // opendm40 (owner 2026-10-24): a press near a marker grabs it; the
    // nearest wins, the selected one within NEAR_TIE; guide lines are zones
    // while shown; a still tap on the selection cycles (graph_cycle).
    let off = su::TAP_OFF;
    let gs = |m: &su::Model, sv: u8, sh: u8, lh: bool, x: i16, y: i16| su::graph_press(m, off, sv, sh, lh, x, y);
    // T top marker glyph x 208..219, y 21..32 (trig_px 200).
    ck.t("near grab: 12 px below the T marker glyph grabs it", gp(&d, off, 214, 44) == Press::Marker(false, 0) && gp(&d, off, 214, 46) == Press::None);
    let mut m = d;
    m.xcur = Some([205, 100]); // X1 top marker x 213..224
    ck.t("near grab: the nearer marker wins (X1 2 px off, T 7 px off)", gs(&m, 0, 0, true, 226, 26) == Press::Marker(false, 2));
    ck.t("near grab: the selected marker wins within the tie (T 3 px off, X1 on it)", gs(&m, 0, 0, true, 222, 26) == Press::Marker(false, 0));
    ck.t("near grab: no tie preference for the unselected (X1 selected, 5 px off; T on it)", gs(&m, 0, 2, true, 208, 26) == Press::Marker(false, 0));
    // Owner 2026-10-25: the guide lines are not zones: a drag on the trace
    // next to the shown trigger-level / trigger-position / CH1 line pans.
    let mut m = d;
    m.guide = true;
    m.sel_v = 1;
    m.sel_h = 0;
    ck.t("no guide line zones: 10 px off the shown trigger-level line, mid-graph, is no marker", gs(&m, 1, 0, false, 200, 20 + 105 + 10) == Press::None && gs(&m, 1, 0, false, 200, 20 + 105) == Press::None);
    ck.t("no guide line zones: next to the trigger-position line, CH1 zero line", gs(&m, 1, 0, false, 224, 200) == Press::None && gs(&m, 0, 0, false, 200, 165) == Press::None);
    let mut m = d;
    m.trig_row = 140; // trigger-level marker x 401..412 and CH1 at row 140
    m.ycur = Some([140, 220]); // Y1 line on the same row
    ck.t("near grab: CH1 marker over the Y1 line (equal distance: the marker)", gs(&m, 3, 0, false, 6, 160) == Press::Marker(true, 3) && gs(&m, 4, 0, false, 6, 160) == Press::Marker(true, 0));
    ck.t("tap cycling: CH1 selected, a tap on it -> Y1", su::graph_cycle(&m, 0, 0, false, 6, 160) == Some((true, 3)));
    ck.t("marker glyph zones are not line zones", su::graph_pick(&d, 0, 0, false, 406, 125).is_some_and(|q| !q.line()));
    // One item, one axis per gesture (owner 2026-10-25): a finger path
    // through drag_axis / drag_delta as scope_page.rs sp_move applies it.
    let run = |path: &[(i32, i32)]| -> (i32, i32) {
        let (mut ax, mut sx, mut sy) = (None, 0, 0);
        let mut last = (0, 0);
        for &(x, y) in path {
            if ax.is_none() {
                ax = su::drag_axis(x.abs(), y.abs());
                if ax.is_none() {
                    continue; // undecided: nothing applied, the delta kept
                }
            }
            let (dx, dy) = su::drag_delta(ax.unwrap(), x - last.0, y - last.1);
            sx += dx;
            sy += dy;
            last = (x, y);
        }
        (sx, sy)
    };
    // A sideways pan with vertical wobble (every 3rd report mostly
    // vertical: the stock split moved the selected level item with them).
    let pan: Vec<(i32, i32)> = (1..=30).map(|i| (4 * i, [0, 6, 2][i as usize % 3])).collect();
    ck.t("pan off the markers with vertical wobble: horizontal only (no level move)", run(&pan) == (120, 0));
    let vert: Vec<(i32, i32)> = (1..=20).map(|i| ([0, 4, 2][i as usize % 3], 3 * i)).collect();
    ck.t("vertical drag with sideways jitter: vertical only", run(&vert) == (0, 60));
    let diag: Vec<(i32, i32)> = (1..=20).map(|i| (3 * i, 2 * i)).collect();
    // The item (owner 2026-10-25: never the selection): the nearest
    // marker of the decided axis to the press point (drag_target).
    let dt = |m: &su::Model, h: bool, x: i16, y: i16| su::drag_target(m, h, x, y);
    // Default: CH1 marker centre y 160 (row 140), trigger level y 125
    // (row 105, right edge), T column 200 (x 214).
    let mut m = d;
    m.sel_v = 0; // CH1 selected: ignored
    ck.t("drag target V: nearest level row, not the selection (y 130 -> trigger level, y 150 -> CH1)", dt(&m, false, 200, 130) == Some((true, 1)) && dt(&m, false, 30, 150) == Some((true, 0)));
    ck.t("drag target V: no distance limit (y 290 -> CH1, the nearer)", dt(&m, false, 200, 290) == Some((true, 0)));
    m.trig_row = 140;
    ck.t("drag target V: a tie goes to the one drawn later (trigger level over CH1)", dt(&m, false, 200, 160) == Some((true, 1)));
    let mut m = d;
    m.ycur = Some([60, 250]);
    ck.t("drag target V: the Y cursors when shown (y 82 -> Y1)", dt(&m, false, 200, 82) == Some((true, 3)));
    let mut m = d;
    m.trig_row = 300; // off screen: the edge marker at the bottom
    ck.t("drag target V: an off-screen level by its edge marker", dt(&m, false, 200, 292) == Some((true, 1)));
    let mut m = d;
    m.sel_h = 2;
    m.xcur = Some([100, 300]);
    ck.t("drag target H: nearest column (x 120 -> X1, x 250 -> T, x 330 -> X2), not the selection", dt(&m, true, 120, 200) == Some((false, 2)) && dt(&m, true, 250, 200) == Some((false, 0)) && dt(&m, true, 330, 200) == Some((false, 3)));
    ck.t("drag target H: T alone, anywhere", dt(&d, true, 30, 280) == Some((false, 0)));
    ck.t("a diagonal drag moves one axis only (the larger one)", { let r = run(&diag); r.1 == 0 && r.0 == 60 });
    ck.t("drag_axis: undecided below the slop, 3:2 lead past 7 px", su::drag_axis(7, 0).is_none() && su::drag_axis(8, 5) == Some(true) && su::drag_axis(5, 8) == Some(false) && su::drag_axis(20, 20).is_none() && su::drag_axis(31, 30) == Some(true));
    // Top-bar badges: hit map against the drawn badges, then the edges.
    host::reset(0);
    su::invalidate();
    su::update(&d);
    page(ck, "top badges", &|x, y| su::top_badge(x, y).map(|k| k as u32), &|b| b.y1 < 20, &[0, 1, 2, 3]);
    let tb = |x: u16, y: u16| su::top_badge(x, y);
    ck.t("badges: run / timebase / V/div / trigger", tb(40, 10) == Some(0) && tb(100, 10) == Some(1) && tb(300, 5) == Some(2) && tb(380, 10) == Some(3));
    ck.t("badges: rows 15..19 (inside INNER) are the badge, y 20 the graph", tb(100, 15) == Some(1) && tb(100, 19) == Some(1) && tb(100, 20).is_none() && tb(300, 19) == Some(2));
    ck.t("badges: record bar, gaps and battery are not badges", tb(200, 10).is_none() && tb(77, 10).is_none() && tb(143, 10).is_none() && tb(450, 10).is_none());
    let mut m = d;
    m.trig_px = 90;
    ck.t("badges win over the T zone's top rows (T under the timebase badge)", gp(&m, c, 100, 17) == Press::Marker(false, 0) && tb(100, 17) == Some(1) && gp(&m, c, 100, 25) == Press::Marker(false, 0));
    // Hold repeat (Corners): a press every 5 ms of main loop from t0.
    let hold = |mode: u8, side: Option<bool>, ms: u64, still_until: u64| -> (Vec<u64>, Option<bool>) {
        let mut a = su::Arm { side, t0: 1000, rep: 0 };
        let mut at = vec![];
        let mut t = 1000;
        while t <= 1000 + ms {
            let still = t < 1000u64.saturating_add(still_until);
            if a.repeat(mode, still, t).is_some() {
                at.push(t - 1000);
            }
            t += 5;
        }
        (at, a.release(ms < still_until))
    };
    let (at, rel) = hold(c, Some(true), 100, u64::MAX);
    ck.t("corners: a 100 ms tap steps once, on release", at.is_empty() && rel == Some(true));
    let (at, rel) = hold(c, Some(false), 495, u64::MAX);
    ck.t("corners: no repeat before 500 ms", at.is_empty() && rel == Some(false));
    let (at, rel) = hold(c, Some(false), 1300, u64::MAX);
    ck.t(&format!("corners: hold repeats at 500, 700, 900, 1100, 1300 ms, none on release ({at:?})"), at == vec![500, 700, 900, 1100, 1300] && rel.is_none());
    let (at, rel) = hold(c, Some(true), 1300, 300);
    ck.t("corners: a press that became a drag never repeats nor steps", at.is_empty() && rel.is_none());
    let (at, rel) = hold(c, Some(true), 1000, 800);
    ck.t(&format!("corners: repeats stop when the finger moves ({at:?})"), at == vec![500, 700] && rel.is_none());
    let (at, rel) = hold(h, Some(true), 1500, u64::MAX);
    ck.t("halves: no repeat (one step on release)", at.is_empty() && rel == Some(true));
    let (at, rel) = hold(c, None, 1500, u64::MAX);
    ck.t("a marker press (no side) never steps", at.is_empty() && rel.is_none());
    ck.t("armed side shown pressed while still, not after a repeat", {
        let a = su::Arm { side: Some(true), t0: 0, rep: 0 };
        let b = su::Arm { rep: 1, ..a };
        a.armed(true) == Some(true) && a.armed(false).is_none() && b.armed(true).is_none()
    });
    // Timebase list popup: every drawn row label is the row its touch picks;
    // outside the box closes; scroll by whole rows, clamped at the ends.
    for cur in [scope::TB_MIN as u8, 4, 13, 24, 28, scope::TB_MAX as u8] {
        host::reset(0);
        su::draw_tb(cur);
        let e = emu();
        let mut rows: std::collections::BTreeMap<u32, (i32, i32, i32, i32)> = Default::default();
        let sp = su::TB_SPIN;
        for y in sp.y as usize..(sp.y + sp.h) as usize {
            for x in sp.x as usize..(sp.x + sp.w) as usize {
                let o = e.owner[y * W + x];
                if o == 0 || e.kinds[(o >> 8) as usize - 1] != probe::TEXT {
                    continue;
                }
                let b = rows.entry(o >> 8).or_insert((x as i32, y as i32, x as i32, y as i32));
                *b = (b.0.min(x as i32), b.1.min(y as i32), b.2.max(x as i32), b.3.max(y as i32));
            }
        }
        let lo = (cur as i32 - 2).max(scope::TB_MIN as i32);
        let hi = (cur as i32 + 2).min(scope::TB_MAX as i32);
        // Each row: its time label and, right of it, its sample rate.
        let rate_x = su::tb_rate_rect(0).x as i32;
        let (rates, rows): (Vec<_>, Vec<_>) = rows.into_iter().partition(|(_, b)| b.0 >= rate_x);
        ck.t(&format!("tb list {cur}: {} rows drawn", hi - lo + 1), rows.len() as i32 == hi - lo + 1);
        ck.t(&format!("tb list {cur}: a rate beside each row"), rates.len() == rows.len() && rates.iter().all(|(_, r)| rows.iter().any(|(_, b)| (r.1 - sp.y as i32) / 30 == (b.1 - sp.y as i32) / 30)));
        let mut ok = true;
        for (_, b) in &rows {
            let (cx, cy) = ((b.0 + b.2) / 2, (b.1 + b.3) / 2);
            match su::tb_hit(cur, cx as i16, cy as i16) {
                su::TbHit::Row(i) => {
                    // The row's label width matches the label of its index.
                    let want = su::TB_LABELS[i as usize].len();
                    let glyphs: std::collections::BTreeSet<u8> = (b.1..=b.3).flat_map(|y| (b.0..=b.2).map(move |x| (x, y))).map(|(x, y)| (e.owner[y as usize * W + x as usize] & 255) as u8).filter(|_| true).collect();
                    ok &= glyphs.len() >= want;
                    // The whole 30 px row picks the same item.
                    let ry = sp.y as i32 + ((cy - sp.y as i32) / 30) * 30;
                    ok &= (ry..ry + 30).all(|y| su::tb_hit(cur, cx as i16, y as i16) == su::TbHit::Row(i));
                    ok &= (i as i32 - cur as i32) == (ry - (sp.y as i32 + 60)) / 30;
                }
                _ => ok = false,
            }
        }
        ck.t(&format!("tb list {cur}: each drawn label picks its own row"), ok);
        ck.t(&format!("tb list {cur}: outside the box closes, header / margins do not"), su::tb_hit(cur, 300, 100) == su::TbHit::Close && su::tb_hit(cur, 78, 100) == su::TbHit::Close && su::tb_hit(cur, 100, 215) == su::TbHit::Close && su::tb_hit(cur, 120, 30) == su::TbHit::Inside && su::tb_hit(cur, 81, 100) == su::TbHit::Inside && su::tb_hit(cur, 100, 20) == su::TbHit::Inside);
    }
    ck.t("tb list: rows beyond the ends are no pick", su::tb_hit(3, 140, 60) == su::TbHit::Inside && su::tb_hit(29, 140, 200) == su::TbHit::Inside);
    let sc = |c: u8, a: i32| su::tb_scroll(c, a);
    ck.t("tb scroll: < 30 px no row, 30 px one row (down = faster)", sc(13, 29) == (13, 29) && sc(13, 30) == (12, 0) && sc(13, -65) == (15, -5));
    ck.t("tb scroll: clamped at both ends, remainder dropped", sc(4, 90) == (3, 0) && sc(28, -90) == (29, 0) && sc(3, 45) == (3, 0));
    // DMM wave view: the list starts at WAVE_TB_MIN (5 ms/div), no entry
    // below it to pick or scroll to; back to TB_MIN on exit.
    {
        let w = crate::scope::WAVE_TB_MIN as u8;
        su::set_tb_lo(w as usize);
        let ok = su::tb_hit(w, 140, 60) == su::TbHit::Inside && su::tb_hit(w, 140, 90) == su::TbHit::Inside && su::tb_hit(w, 140, 120) == su::TbHit::Row(w) && su::tb_hit(w, 140, 150) == su::TbHit::Row(w + 1);
        let ok2 = sc(w + 1, 90) == (w, 0) && sc(w, 30) == (w, 0) && sc(w, -30) == (w + 1, 0);
        su::set_tb_lo(crate::scope::TB_MIN);
        ck.t("tb list (wave view): nothing below 5 ms/div to pick", ok);
        ck.t("tb scroll (wave view): clamped at 5 ms/div", ok2);
        ck.t("tb list: TB_MIN again after the wave view", su::tb_hit(3, 140, 120) == su::TbHit::Row(3) && sc(4, 90) == (3, 0));
    }
    // A drag of 3 px reports up 137 px: 4 rows, applied once on release.
    let (mut cur, mut acc) = (13u8, 0);
    let mut shown = vec![];
    for _ in 0..46 {
        acc -= 3;
        let (i, r) = sc(cur, acc);
        if i != cur {
            shown.push(i);
        }
        cur = i;
        acc = r;
    }
    ck.t(&format!("tb scroll: drag 138 px up = 4 rows slower, one redraw per row ({shown:?})"), cur == 17 && shown == vec![14, 15, 16, 17]);
    // List pick (review 621ac3f L8): a roll-only row turns Roll on except in
    // Single, which takes the YT maximum and leaves Roll alone.
    let yt = scope::TB_YT_MAX as u8;
    ck.t("tb pick: YT row as is, Roll untouched", su::tb_pick(13, false, false) == (13, false) && su::tb_pick(13, false, true) == (13, false));
    ck.t("tb pick: roll-only row in Auto/Normal turns Roll on", su::tb_pick(27, false, false) == (27, true) && su::tb_pick(27, true, false) == (27, false));
    ck.t("tb pick: roll-only row in Single = YT max, Roll stays off", su::tb_pick(27, false, true) == (yt, false) && su::tb_pick(29, true, true) == (yt, false));
    // Persisted mode: 0 (older records) = the default; round trip.
    ck.t("tap mode persist: 0 = TAP_DEFAULT (Corners)", su::tap_decode(0) == su::TAP_DEFAULT && su::TAP_DEFAULT == su::TAP_CORNERS);
    ck.t("tap mode persist: round trip, unknown = default", (0..3u8).all(|m| su::tap_decode(su::tap_code(m)) == m && su::tap_code(m) < 4) && (0..4u32).filter(|c| !(0..3u8).any(|m| su::tap_code(m) == *c)).all(|c| su::tap_decode(c) == su::TAP_DEFAULT));
}

/// Comparator trigger policy (device 2026-10-21: after AUTO the comparator
/// stayed off for good): misses suspend it, `resume` (a scope settings
/// change) and `set_enabled(true)` lift that; a disabled engine stays off.
fn hwtrig_tests(ck: &mut Ck) {
    let mut h = hwtrig::HwTrig::new();
    h.set_enabled(true);
    h.note_miss();
    h.note_miss();
    ck.t("hwtrig: two misses keep the comparator", h.enabled() && !h.suspended());
    h.note_miss();
    ck.t("hwtrig: three misses suspend, still enabled", h.enabled() && h.suspended());
    h.resume();
    ck.t("hwtrig: resume lifts the suspension", h.enabled() && !h.suspended());
    h.note_miss();
    h.note_miss();
    ck.t("hwtrig: resume also clears the miss count", !h.suspended());
    h.note_miss();
    h.set_enabled(true);
    ck.t("hwtrig: enable clears a suspension", !h.suspended());
    h.set_enabled(false);
    h.resume();
    ck.t("hwtrig: resume does not enable a disabled engine", !h.enabled());
}

/// The wave view's V/div steps (scope::WAVE_STEPS_V / _A, the pinch on
/// them): against dmm::ad2_scale_of's ranges (nominal, no calibration).
fn wave_step_tests(ck: &mut Ck) {
    // (DMM mode, range) of each MF item: V 600 mV (AC mV r1), 6..1000 V
    // (AC V r0..3), 60 mV (AC mV r0); A 600 / 6000 uA, 60 / 600 mA, 6 / 10 A.
    let row = |amps: bool, item: u8| -> (u8, i8) {
        if amps {
            [(6, 0), (6, 1), (5, 0), (5, 1), (4, 0), (4, 1)][item as usize]
        } else {
            [(2, 1), (0, 0), (0, 1), (0, 2), (0, 3), (2, 0)][item as usize]
        }
    };
    for amps in [false, true] {
        let st = scope::wave_steps(amps);
        let u = if amps { "A" } else { "V" };
        let asc = st.windows(2).all(|w| w[0].0 < w[1].0);
        ck.t(&format!("wave steps {u}: ascending"), asc);
        // Every range of the function has exactly one full-scale step:
        // V/div = full / 4 (+-full on the 8 divisions), its own item.
        let items: &[u8] = if amps { &[0, 1, 2, 3, 4, 5] } else { &[5, 0, 1, 2, 3, 4] };
        for &it in items {
            let (m, r) = row(amps, it);
            let a = dmm::ad2_scale_of(m, r).unwrap();
            ck.t(&format!("wave {u} item {it}: ad2_scale_of item"), a.item == it);
            let f = st.iter().filter(|s| s.1 == it && s.2).count();
            let ok = f == 1 && scope::wave_full_step(amps, it).is_some_and(|i| (st[i].0 * 4.0 - a.full).abs() < a.full * 1e-4);
            ck.t(&format!("wave {u} item {it}: one full-scale step = full / 4 ({})", a.full), ok);
        }
        // Each other step on the lowest range whose AD2 window holds its
        // +-4 div with 20 % to spare.
        for s in st.iter().filter(|s| !s.2) {
            let win = |it: u8| {
                let (m, r) = row(amps, it);
                let a = dmm::ad2_scale_of(m, r).unwrap();
                a.per_code * 262_144.0
            };
            let fits = |it: u8| 4.0 * s.0 <= 0.8 * win(it);
            let lowest = items.iter().copied().find(|&it| fits(it));
            ck.t(&format!("wave {u} step {}: on the lowest range that holds it (item {}, want {:?})", s.0, s.1, lowest), lowest == Some(s.1));
        }
        // The pinch: no move at r 1, one way only, a step once reached.
        let n = st.len();
        let mut ok = true;
        for i0 in 0..n {
            ok &= scope::wave_pinch_step(amps, i0, 1.0) == i0;
            for k in 1..40 {
                let r = 1.0 + k as f32 * 0.1;
                let zi = scope::wave_pinch_step(amps, i0, r);
                let zo = scope::wave_pinch_step(amps, i0, 1.0 / r);
                ok &= zi <= i0 && zo >= i0 && st[zi].0 >= st[i0].0 / r * 0.999 && st[zo].0 <= st[i0].0 * r * 1.001;
                // the next step past it was not yet reached
                ok &= zi == 0 || st[zi - 1].0 < st[i0].0 / r * 0.999;
                ok &= zo + 1 == n || st[zo + 1].0 > st[i0].0 * r * 1.001;
            }
        }
        ck.t(&format!("wave {u}: pinch steps"), ok);
    }
    // The fractional labels.
    for (v, a, want) in [(1.5f32, false, "1.5V"), (0.15, false, "150mV"), (2.5, true, "2.5A"), (1.5e-3, true, "1.5mA"), (250.0, false, "250V"), (0.015, false, "15mV"), (2.0, false, "2V"), (150e-6, true, "150uA")] {
        let mut b = scope_ui::Buf::new();
        scope_ui::fmt_div(&mut b, v, if a { "A" } else { "V" });
        ck.t(&format!("fmt_div {v} = {want} ({})", b.as_str()), b.as_str() == want);
    }
    // #133: the CH1 V/div text is fmt_div(VDIV[range] * probe) (the label
    // table index range + probe index read "2V" for 2.5 V/div at x5): every
    // range x probe reads back as its value; a decade probe gives the
    // stock label.
    let mut bad = vec![];
    for (pi, &p) in scope::PROBES.iter().enumerate() {
        for (r, &d) in scope::VDIV.iter().enumerate() {
            let v = d * p;
            let mut b = scope_ui::Buf::new();
            scope_ui::fmt_div(&mut b, v, "V");
            let s = b.as_str().strip_suffix('V').unwrap_or("?").to_string();
            let (num, k) = match s.chars().last() {
                Some('u') => (&s[..s.len() - 1], 1e-6),
                Some('m') => (&s[..s.len() - 1], 1e-3),
                Some('k') => (&s[..s.len() - 1], 1e3),
                _ => (&s[..], 1.0),
            };
            let back = num.parse::<f32>().map(|x| x * k).unwrap_or(f32::NAN);
            let stock = format!("{}V", scope_ui::VDIV_LABELS[r + pi]);
            if !((back - v).abs() <= v * 1e-3) || (pi % 3 == 0 && b.as_str() != stock) {
                bad.push(format!("x{p} {d}: {}", b.as_str()));
            }
        }
    }
    ck.t(&format!("CH1 V/div text = the value for every range x probe ({bad:?})"), bad.is_empty());
}

// ------------------------------------------------------- 5. calibration

/// The touch test page's nine targets (ui_touchtest.rs TARGETS).
const TGT: [(u16, u16); 9] = ui::touchtest::TARGETS;

/// Raw taps (`per` per target) of a panel whose controller reports
/// raw = inverse of (a..f) at each target, plus `noise` (deterministic,
/// +-noise px), as (raw, target) lists.
fn taps(k: [f64; 6], per: usize, noise: i32) -> (Vec<(i16, i16)>, Vec<(i16, i16)>) {
    // Invert x' = a x + b y + c, y' = d x + e y + f.
    let det = k[0] * k[4] - k[1] * k[3];
    let (mut raw, mut tgt) = (vec![], vec![]);
    let mut seed = 12345u32;
    for &(tx, ty) in TGT.iter() {
        for _ in 0..per {
            let (u, v) = (tx as f64 - k[2], ty as f64 - k[5]);
            let x = (k[4] * u - k[1] * v) / det;
            let y = (-k[3] * u + k[0] * v) / det;
            let mut r = || {
                seed = seed.wrapping_mul(1103515245).wrapping_add(12345);
                if noise == 0 { 0 } else { (seed >> 16) as i32 % (2 * noise + 1) - noise }
            };
            let (nx, ny) = (r(), r());
            raw.push(((x.round() as i32 + nx) as i16, (y.round() as i32 + ny) as i16));
            tgt.push((tx as i16, ty as i16));
        }
    }
    (raw, tgt)
}

fn cal_tests(ck: &mut Ck) {
    use touchcal::{fit, Bad};
    // This unit's fit (DBG:TOUCH? taps on the nine targets): given back.
    let unit = [0.9961, 0.0101, -5.65, 0.0061, 0.9523, 0.70];
    let (raw, tgt) = taps(unit, 2, 0);
    let f = fit(&raw, &tgt, &TGT);
    let k = f.map(|f| touchcal::to_f32(&f.cal)).unwrap_or([0.0; 6]);
    let near = |a: f32, b: f64, tol: f64| ((a as f64) - b).abs() <= tol;
    ck.t("fit: the unit's map given back (scale / cross within 0.003)", [0, 1, 3, 4].iter().all(|&i| near(k[i], unit[i], 0.003)));
    ck.t("fit: offsets within 0.6 px", near(k[2], unit[2], 0.6) && near(k[5], unit[5], 0.6));
    ck.t("fit: accepted, residual <= 1 px", f.is_some_and(|f| f.bad.is_none() && f.max <= 10));
    ck.t("fit: raw error before ~ 8 px mean", f.is_some_and(|f| (50..=120).contains(&f.before)));
    // The same with +-6 px finger scatter: accepted, residual under the limit.
    let (raw, tgt) = taps(unit, 2, 6);
    let f = fit(&raw, &tgt, &TGT);
    ck.t("fit with +-6 px scatter: accepted, max <= 12 px", f.is_some_and(|f| f.bad.is_none() && f.max <= 120));
    let k = f.map(|f| touchcal::to_f32(&f.cal)).unwrap_or([0.0; 6]);
    ck.t("fit with scatter: scale within 0.02", near(k[0], unit[0], 0.02) && near(k[4], unit[4], 0.02));
    // Identity: a panel without error gives ~ the identity.
    let (raw, tgt) = taps([1.0, 0.0, 0.0, 0.0, 1.0, 0.0], 1, 0);
    ck.t("fit: identity in, identity out", fit(&raw, &tgt, &TGT).is_some_and(|f| f.cal.iter().all(|v| v.abs() <= 1) && f.mean == 0));
    // One wild tap (40 px off): the residual limit refuses the fit.
    let (mut raw, tgt) = taps(unit, 2, 0);
    raw[8].0 += 40;
    raw[8].1 -= 35;
    ck.t("fit: an outlier tap is refused (residual)", fit(&raw, &tgt, &TGT).is_some_and(|f| f.bad == Some(Bad::Resid)));
    // A scale beyond 1.15 / a shift beyond 40 px: refused.
    let (raw, tgt) = taps([1.2, 0.0, -24.0, 0.0, 1.0, 0.0], 1, 0);
    ck.t("fit: scale 1.2 refused", fit(&raw, &tgt, &TGT).is_some_and(|f| f.bad == Some(Bad::Scale)));
    let (raw, tgt) = taps([1.0, 0.0, 45.0, 0.0, 1.0, 0.0], 1, 0);
    ck.t("fit: shift 45 px refused", fit(&raw, &tgt, &TGT).is_some_and(|f| f.bad == Some(Bad::Shift)));
    // Degenerate: all taps on one line, too few.
    let raw: Vec<(i16, i16)> = (0..9).map(|i| (20 + 50 * i, 160)).collect();
    let tgt = raw.clone();
    ck.t("fit: collinear points give none", fit(&raw, &tgt, &TGT).is_none());
    ck.t("fit: two points give none", fit(&raw[..2], &tgt[..2], &TGT).is_none());
    // Coefficients <-> Cal.
    let c = touchcal::from_f32(&[0.9961, 0.0101, -5.65, 0.0061, 0.9523, 0.70]).unwrap();
    let k = touchcal::to_f32(&c);
    ck.t("from_f32 / to_f32 round trip", near(k[0], 0.9961, 1e-4) && near(k[2], -5.65, 1e-4) && near(k[4], 0.9523, 1e-4));
    ck.t("from_f32: NaN refused", touchcal::from_f32(&[f32::NAN, 0.0, 0.0, 0.0, 1.0, 0.0]).is_none());
    ck.t("check: the unit's map passes", touchcal::check(&c, &TGT).is_ok());

    // screen() with a calibration, through the debounce.
    touch::set_cal(c);
    let down = |x: u16, y: u16| deb(&[(0, Some(pressed(x, y))), (10, Some(pressed(x, y)))]).first().map(|e| e.1.p);
    // Raw (25, 20) -> 0.9961*25 + 0.0101*20 - 5.65 = 19.45 -> 19; y 0.0061*25
    // + 0.9523*20 + 0.7 = 19.9 -> 20.
    ck.t("screen: (25,20) -> (19,20)", down(25, 20) == Some(Point { x: 19, y: 20 }));
    // Raw (466, 313) -> 461.6 -> 462, 301.6 -> 302 (the bottom right).
    ck.t("screen: (466,313) -> (462,302)", down(466, 313) == Some(Point { x: 462, y: 302 }));
    // Raw (2, 1) -> -3.6 -> 0 (clamped), 1.7 -> 2.
    ck.t("screen: clamped at the panel edge", down(2, 1) == Some(Point { x: 0, y: 2 }));
    // A touch key's report is not mapped (keys carry no screen point).
    let mut s = vec![(1000, Some(key(1))), (1010, Some(key(1)))];
    s.extend(idle(1020, 1060));
    ck.t("screen: key reports unaffected", deb(&s).first().is_some_and(|e| e.1.keys == 2 && e.1.points == 0));
    touch::set_cal(touchcal::IDENTITY);
    ck.t("screen: identity again = the raw point", down(25, 20) == Some(Point { x: 25, y: 20 }));
}

fn main() {
    let verbose = std::env::args().any(|a| a == "-v");
    let mut ck = Ck { cases: 0, fails: 0, verbose };
    debounce_tests(&mut ck);
    gesture_tests(&mut ck);
    log_tests(&mut ck);
    hit_tests(&mut ck);
    trig_volts_tests(&mut ck); // after hit_tests: flash mapped (calibration reads)
    tap_step_tests(&mut ck);
    hwtrig_tests(&mut ck);
    wave_step_tests(&mut ck);
    cal_tests(&mut ck);
    println!("cases {}, failures {}", ck.cases, ck.fails);
    std::process::exit(if ck.fails == 0 { 0 } else { 1 });
}
