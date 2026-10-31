//! Power-meter screens (dmm::pw, docs/power-meter-and-misconnect.md 3.9 /
//! 3.12), a child of ui.rs, in the stock DMM page's slots and visual
//! language:
//!
//!   status bar  AUTO / MANU (both channels), HOLD / alerts as everywhere
//!   aux rows    Vrms (aux1); the energy (aux2: Wh, mWh / kWh; VAh in the
//!               fallback; P while the main readout shows the energy) in the
//!               stock LETTERS rows; long press = reset (confirm)
//!   main        P (fallback: S), or Wh / VAh (a tap on it cycles), in the F78
//!               cells; unit in the stock UNIT face (our W of that face,
//!               stockfont_w.rs), LETTERS for the energy units (UNIT has no h;
//!               our k of that face: "kWh")
//!   glyph       the coupling above the unit: the stock DC / AC / AC+DC mode
//!               glyph (ui.rs `pw_glyph`); a tap on the active W button steps
//!               DC -> AC -> AC+DC; the limit badge left of it (`coupling`)
//!   history row the five 88 x 19 boxes: I, PF, S, Q, f (F14 tag + HIST
//!               value; units the HIST face lacks in F14)
//!   chart       the main readout, as every function
//!   side column burden V, VAh, the energy box (run / pause, time; tap = run /
//!               pause, long press = reset with a confirm), average P,
//!               ranges / the continuous-current countdown
//!
//! Entry: the W function button (the wiring dialog until confirmed once),
//! a long press on the A button. Any other function button leaves. The MF
//! key opens the Power Settings page (`settings_page`).

use super::*;
use crate::dmm::pw::{self, PowerVals};

/// Caches of what the power rows / side column last drew (text per slot).
#[derive(Clone, Copy)]
pub struct PwCache {
    rows: [u32; 5],
    side: [u32; 5],
    coup: u32,
    /// The parts sub-lines (V, I, P) as last drawn.
    sub: [u32; 3],
}

impl PwCache {
    pub const fn new() -> Self {
        PwCache { rows: [0; 5], side: [0; 5], coup: 0, sub: [0; 3] }
    }

    /// Redraw sub-line `i` (0 V, 1 I) with the next reading: the aux row's
    /// value / unit boxes (y 36..65) overlap its slot's top rows.
    pub fn sub_invalidate(&mut self, i: usize) {
        self.sub[i] = 0;
    }

    /// Redraw the limit-badge slot with the next reading.
    pub fn coup_invalidate(&mut self) {
        self.coup = 0;
    }
}

/// The coupling on the page is the stock mode glyph above the unit (DC /
/// AC / AC+DC, ui.rs `reading`); the slot left of it carries the limit
/// verdict (the W limits, meter::V_*) or nothing. The coupling itself is
/// switched by the active W button (DC -> AC -> AC+DC, as V / A) or the
/// Power Settings row.
const BADGE: sf::Rect = rect(403, 72, 31, 17);
/// "At load" mark of the main readout (doc 3.16): top left, left of the F78
/// cells (40..; the power main has at most its sign in the first cell,
/// mid-height), under the aux row (..65), above the HV lightning's box
/// (0, 80, 40, 64; V functions only); cleared by `DmmView::reading` on a
/// mode change. The right column (403..436 between the limit badge and the
/// unit) is 3 px too narrow for it at the F14 letter spacing.
pub const LOAD_MARK: sf::Rect = rect(4, 66, 36, 14);

fn coupling(c: &mut PwCache, pv: &PowerVals, lim: u8) {
    let k = 1 + ((lim as u32) << 2) + ((pv.comp as u32) << 12);
    if c.coup == k {
        return;
    }
    c.coup = k;
    let md = crate::ui::main_dy();
    if lim != crate::meter::V_NONE {
        crate::ui::lim_badge(lim);
    } else {
        sf::label(sf::F14, rect(BADGE.x, BADGE.y + md, BADGE.w, BADGE.h), "", VALUE, BG, 8, 1);
    }
    let m = LOAD_MARK;
    sf::label(sf::F14, rect(m.x, m.y + md, m.w, m.h), if pv.comp { crate::ui::LOAD_CAP } else { "" }, VALUE, BG, 4, 1);
}

/// Small text buffer.
struct T {
    b: [u8; 64],
    n: usize,
}

impl T {
    fn new() -> T {
        T { b: [0; 64], n: 0 }
    }
    fn push(&mut self, s: &str) -> &mut T {
        let k = s.len().min(64 - self.n);
        self.b[self.n..self.n + k].copy_from_slice(&s.as_bytes()[..k]);
        self.n += k;
        self
    }
    fn s(&self) -> &str {
        core::str::from_utf8(&self.b[..self.n]).unwrap_or("")
    }
    /// FNV-1a of the text (cache key; never 0 = "not drawn").
    fn key(&self) -> u32 {
        let mut h = 0x811C_9DC5u32;
        for &b in &self.b[..self.n] {
            h = (h ^ b as u32).wrapping_mul(0x0100_0193);
        }
        h | 1
    }
}

/// `v` with `sig` significant digits (at most 4 decimals, none above
/// 10^sig): "229.98", "0.9987", "12345"; "--" for NaN.
pub fn sig_text(buf: &mut [u8; 16], v: f32, sig: u8) -> &str {
    if !v.is_finite() {
        buf[..2].copy_from_slice(b"--");
        return core::str::from_utf8(&buf[..2]).unwrap_or("");
    }
    let a = v.abs();
    let mut dec = sig as i32 - 1;
    let mut x = 1.0f32;
    while a >= x * 10.0 && dec > 0 {
        x *= 10.0;
        dec -= 1;
    }
    // 999.99 at 4 digits rounds to 1000.0: one decimal less.
    let mut p = 1.0f32;
    for _ in 0..dec.clamp(0, 4) {
        p *= 10.0;
    }
    if dec > 0 && a * p + 0.5 >= x * 10.0 * p {
        dec -= 1;
    }
    let dec = dec.clamp(0, 4) as u8;
    crate::dmm::format(buf, pw::no_neg_zero(v, dec), dec)
}

/// History-row box `i` (stock 5 + 95 i, 153, 88 x 19).
/// The power page's lower row: the energy box wider (5 digits + mWh /
/// kWh), then PF / S / Q / f; 3 px gaps, x 5..475 as the stock row.
const ROW_X: [i16; 5] = [5, 124, 205, 298, 391];
const ROW_W: [i16; 5] = [116, 78, 90, 90, 84];

/// The lower row's top: 153, PARTS_ROW_DY lower in the parts layout.
fn row_y() -> i16 {
    153 + if crate::ui::pw_parts() { crate::ui::PARTS_ROW_DY } else { 0 }
}

fn row_rect(i: usize) -> sf::Rect {
    rect(ROW_X[i], row_y(), ROW_W[i], 19)
}

/// Every character of `s` in the HIST face.
fn in_hist(s: &str) -> bool {
    s.chars().all(|c| sf::HIST.glyph(c as u32).is_some())
}

/// One power box: tag (F14, dim) left, value (HIST) and unit (HIST, or F14
/// when the HIST face lacks a letter) right-aligned.
fn power_box(i: usize, tag: &str, val: &str, unit: &str, warn: bool) {
    let r = row_rect(i);
    sf::rbox(r, 3, BAR, BG);
    sf::glyphs(sf::F14, rect(r.x + 4, r.y, 30, r.h), tag, DIM, BAR, 1, 1);
    let fg = if warn { VALUE } else { STATUS_FG };
    let end = r.x + r.w - 4;
    let (uf, us) = if in_hist(unit) { (sf::HIST, 0) } else { (sf::F14, 1) };
    let uw = if unit.is_empty() { 0 } else { sf::text_width(uf, unit, us) + 1 };
    if uw > 0 {
        sf::glyphs(uf, rect(end - uw, r.y, uw, r.h), unit, fg, BAR, 4, us);
    }
    // 3 px between the value's ink and the unit's.
    let ve = if uw > 0 { end - uw - 3 } else { end };
    sf::glyphs(sf::HIST, rect(r.x + 4, r.y, ve - r.x - 4, r.h), val, fg, BAR, 4, 0);
}

/// The parts sub-lines' slots (doc 3.17): under V (right-aligned to the V
/// unit's end, 201) and I (423) at y 61, under P (to 394, under the F78
/// cells) at 155; F14, "DC" / "AC" / "+" dim, the values in the status
/// colour, PART_GAP px between the pieces.
const SUB_V: sf::Rect = rect(4, 61, 224, 14);
const SUB_I: sf::Rect = rect(232, 61, 244, 14);
const SUB_P: sf::Rect = rect(40, 155, 360, 14);
const SUB_END: [i16; 3] = [201, 423, 394];
const PART_GAP: i16 = 5;

/// "<value> <unit>" of a part in the readout's format of the range.
fn part_text<'a>(t: &'a mut T, v: &crate::dmm::Val) -> &'a str {
    let mut b = [0u8; 16];
    let mut u = [0u8; 16];
    t.push(crate::meter::text(&mut b, v)).push(" ").push(sf::unit_text(v.unit, &mut u));
    t.s()
}

/// One sub-line: the slot cleared, then the pieces right-aligned at `end`.
fn pieces_right(slot: sf::Rect, end: i16, items: &[(&str, Color)]) {
    lcd::fill_rect(slot.x as u16, slot.y as u16, slot.w as u16, slot.h as u16, BG);
    let mut x = end;
    // One phrase drawn piece by piece: one text for ui-render's rules.
    #[cfg(test)]
    crate::probe::group(true);
    for (t, fg) in items.iter().rev() {
        let w = sf::text_width(sf::F14, t, 1);
        sf::label(sf::F14, rect(x - w - 1, slot.y, w + 1, slot.h), t, *fg, BG, 4, 1);
        x -= w + 1 + PART_GAP;
    }
    #[cfg(test)]
    crate::probe::group(false);
}

/// The parts sub-lines (per reading, redrawn on a text change): DC / AC of
/// V and I under the top line, DC + AC of P under the main readout (only
/// while it shows P). `pv.vp` / `ip` / `pp` (powermeter.rs).
fn sublines(c: &mut PwCache, pv: &PowerVals) {
    let part = |k: usize| pw::reading_vals(&PowerVals { v: pv.vp[k], i: pv.ip[k], p: pv.pp[k], main: pw::MAIN_P, ..*pv });
    let (pd, vd, id, _) = part(0);
    let (pa, va, ia, _) = part(1);
    let mut ts = [T::new(), T::new(), T::new(), T::new(), T::new(), T::new()];
    let [t0, t1, t2, t3, t4, t5] = &mut ts;
    let lines: [[(&str, Color); 5]; 3] = [
        [("DC", DIM), (part_text(t0, &vd), STATUS_FG), ("AC", DIM), (part_text(t1, &va), STATUS_FG), ("", 0)],
        [("DC", DIM), (part_text(t2, &id), STATUS_FG), ("AC", DIM), (part_text(t3, &ia), STATUS_FG), ("", 0)],
        [("DC", DIM), (part_text(t4, &pd), STATUS_FG), ("+", DIM), ("AC", DIM), (part_text(t5, &pa), STATUS_FG)],
    ];
    for (i, l) in lines.iter().enumerate() {
        let show = i < 2 || pv.main == pw::MAIN_P;
        let mut k = T::new();
        if show {
            for (t, _) in l {
                k.push(t).push("|");
            }
        }
        let key = k.key();
        if c.sub[i] == key {
            continue;
        }
        c.sub[i] = key;
        let slot = [SUB_V, SUB_I, SUB_P][i];
        let n = if i == 2 { 5 } else { 4 };
        pieces_right(slot, SUB_END[i], if show { &l[..n] } else { &[] });
    }
}

impl DmmView {
    /// The power boxes in the history row's place (redrawn per box when its
    /// text changed). PF, Q are "--" in the fallback (no phase relation).
    pub fn power_row(&mut self, pv: &PowerVals, lim: u8) {
        coupling(&mut self.pwc, pv, lim);
        // AC+DC: the parts sub-lines (doc 3.17; the page layout follows
        // `ui::pw_parts`, set with the static draw).
        if crate::ui::pw_parts() && pv.coup == pw::COUP_ACDC && !pv.dc && !pv.alt {
            sublines(&mut self.pwc, pv);
        }
        let mut b = [0u8; 16];
        // DC method: no phase relation, no AC: PF / S / Q / f blank ("--").
        let pf = if pv.pf.is_finite() && !pv.dc { crate::dmm::format(&mut b, pw::no_neg_zero(pv.pf, 3), 3) } else { "--" };
        let mut pft = T::new();
        pft.push(pf);
        // Below 1 VA: mVA / mvar (the mW ranges).
        let (k, su, qu, sg) = if pv.s < 1.0 { (1e3, "mVA", "mvar", 3) } else { (1.0, "VA", "var", 4) };
        let mut st = T::new();
        st.push(if pv.dc { "--" } else { sig_text(&mut b, pv.s * k, sg) });
        let mut qt = T::new();
        qt.push(if pv.dc { "--" } else { sig_text(&mut b, pv.q * k, sg) });
        let (fv, fu) = if pv.f_hz >= 1000.0 { (pv.f_hz * 1e-3, "kHz") } else { (pv.f_hz, "Hz") };
        let mut ft = T::new();
        ft.push(if pv.f_hz > 0.0 && !pv.dc { sig_text(&mut b, fv, 3) } else { "--" });
        // The energy (owner: V and I on top, the Wh counter below): Wh with
        // mWh / kWh, 5 digits; P instead while the main readout shows the
        // energy. Long press = reset (confirm).
        let ev = pw::energy_val(pv);
        let etag = if pv.main == pw::MAIN_P { "E" } else { "P" };
        // As many of its decimals as the box holds next to the tag and unit.
        let (uf, us) = if in_hist(ev.unit) { (sf::HIST, 0) } else { (sf::F14, 1) };
        let room = ROW_W[0] - 8 - sf::text_width(sf::F14, etag, 1) - 3 - (sf::text_width(uf, ev.unit, us) + 1) - 3 - 3;
        let mut et = T::new();
        if ev.ol {
            et.push("0L");
        } else {
            let mut d = ev.dec;
            while d > 0 && sf::text_width(sf::HIST, crate::dmm::format(&mut b, pw::no_neg_zero(ev.v, d), d), 0) > room {
                d -= 1;
            }
            et.push(crate::dmm::format(&mut b, pw::no_neg_zero(ev.v, d), d));
        }
        // No value ("--": the DC method, or AC coupling below the AC floors,
        // doc 3.16): no unit either.
        let su = if pv.dc || !pv.s.is_finite() { "" } else { su };
        let qu = if pv.dc || !pv.q.is_finite() { "" } else { qu };
        let fu = if pv.dc || !(pv.f_hz > 0.0) { "" } else { fu };
        let items: [(&str, &T, &str); 5] = [(etag, &et, ev.unit), ("PF", &pft, ""), ("S", &st, su), ("Q", &qt, qu), ("f", &ft, fu)];
        for (i, (tag, t, u)) in items.iter().enumerate() {
            let mut k = T::new();
            k.push(t.s()).push(u).push(if pv.comp { "c" } else { "" });
            let key = k.key();
            if self.pwc.rows[i] != key {
                self.pwc.rows[i] = key;
                power_box(i, tag, t.s(), u, false);
            }
        }
    }
}

/// The side column in the power meter (replaces MAX / AVG / REL / MIN /
/// time; same rows, same F14 face, the REL box is the energy box).
/// Side column rows: Vb, VAh (None: not shown), the energy box's top, AVG,
/// the ranges. The parts layout (chart from 192) drops VAh (a tap on the
/// main value still cycles P / Wh / VAh).
struct SideGeom {
    vb: i16,
    vah: Option<i16>,
    box_y: i16,
    avg: i16,
    rng: i16,
}

fn side_geom() -> SideGeom {
    if crate::ui::pw_parts() {
        SideGeom { vb: 192, vah: None, box_y: 208, avg: 249, rng: 263 }
    } else {
        SideGeom { vb: SIDE_Y[0], vah: Some(SIDE_Y[1]), box_y: 205, avg: SIDE_Y[2], rng: SIDE_Y[3] }
    }
}

pub fn side(c: &mut PwCache, pv: &PowerVals) {
    let g = side_geom();
    let mut b = [0u8; 16];
    // Burden voltage (amber with the compensation off and the drop above 1 %
    // of the V reading: it is inside P then; not while it shows 0.000, the
    // last digit 1 mV: #107) and VAh; label dim left, value right. The Wh
    // moved to the aux row (large).
    let mut t0 = T::new();
    t0.push(sig_text(&mut b, pv.burden_v, 4));
    let warn = !pv.comp && pv.burden_v >= 0.0005 && pv.burden_v > 0.01 * pv.v;
    let mut b1 = [0u8; 16];
    let mut t1 = T::new();
    t1.push(sig_text(&mut b1, pv.vah, 5));
    for (i, (lbl, t, fg, y)) in [("Vb", &t0, if warn { VALUE } else { STATUS_FG }, Some(g.vb)), ("VAh", &t1, STATUS_FG, g.vah)].iter().enumerate() {
        let Some(y) = *y else { continue };
        let mut kk = T::new();
        kk.push(t.s()).push(if *fg == VALUE { "w" } else { "" });
        let k = kk.key();
        if c.side[i] != k {
            c.side[i] = k;
            sf::label(sf::F14, rect(SIDE_X, y, 24, 14), lbl, DIM, BG, 0, 1);
            sf::label(sf::F14, rect(SIDE_X + 26, y, SIDE_R - SIDE_X - 26, 14), t.s(), *fg, BG, 3, 1);
        }
    }
    // Energy box (405, 205, 71, 38): "E" + RUN / PAUSE, the time below.
    let mut tb = [0u8; 8];
    let mut k2 = T::new();
    k2.push(if pv.run { "RUN" } else { "PAUSE" }).push(hms(&mut tb, pv.secs));
    if c.side[2] != k2.key() {
        c.side[2] = k2.key();
        let y = g.box_y;
        let r = rect(405, y, 71, 38);
        sf::rbox(r, 6, BTN, BG);
        sf::glyphs(sf::F14, rect(409, y + 4, 14, 14), "E", STATUS_FG, BTN, 0, 1);
        sf::glyphs(sf::F14, rect(426, y + 4, 46, 14), if pv.run { "RUN" } else { "PAUSE" }, if pv.run { 0x07E0 } else { VALUE }, BTN, 3, 1);
        sf::glyphs(sf::F14, rect(409, y + 20, 63, 14), hms(&mut tb, pv.secs), STATUS_FG, BTN, 3, 1);
    }
    // Average P over the integration time.
    let mut t3 = T::new();
    let mut b3 = [0u8; 16];
    // Fewer significant digits when wider than the box ("-24.064").
    let mut sg = 5;
    while sg > 2 && sf::text_width(sf::F14, sig_text(&mut b3, pv.p_avg, sg), 1) > SIDE_R - SIDE_X - 26 {
        sg -= 1;
    }
    t3.push(sig_text(&mut b3, pv.p_avg, sg));
    if c.side[3] != t3.key() {
        c.side[3] = t3.key();
        sf::label(sf::F14, rect(SIDE_X, g.avg, 24, 14), "AVG", DIM, BG, 0, 1);
        sf::label(sf::F14, rect(SIDE_X + 26, g.avg, SIDE_R - SIDE_X - 26, 14), t3.s(), STATUS_FG, BG, 3, 1);
    }
    // Ranges, or the continuous-current countdown / alarm.
    let mut t4 = T::new();
    let fg = match pv.over {
        1 => {
            let mut b4 = [0u8; 16];
            t4.push("I>LIM ").push(crate::dmm::format(&mut b4, pv.over_left as f32, 0)).push("s");
            VALUE
        }
        2 => {
            t4.push("I>LIM!");
            0xF800
        }
        _ => {
            t4.push(pw::V_NAMES[pv.vr as usize & 3]).push(" ").push(pw::I_NAMES[pv.ik.min(5) as usize]);
            DIM
        }
    };
    let mut k4 = T::new();
    k4.push(t4.s()).push(if fg == DIM { "d" } else if fg == VALUE { "v" } else { "r" });
    if c.side[4] != k4.key() {
        c.side[4] = k4.key();
        sf::label(sf::F14, rect(SIDE_X, g.rng, SIDE_R - SIDE_X, 14), t4.s(), fg, BG, 3, 1);
    }
}

/// Clear the side column (entering the power meter: the MIN / MAX labels).
pub fn side_clear(c: &mut PwCache) {
    lcd::fill_rect(402, 174, 78, 105, BG);
    c.side = [0; 5];
}

/// The energy box (the lower row's first, `pw::energy_val`): long press =
/// reset.
pub fn energy_hit(x: u16, y: u16) -> bool {
    let y0 = row_y() as u16;
    x < (ROW_X[1] - 2) as u16 && (y0 - 3..y0 + 22).contains(&y)
}

/// The lower row's band (the power page swallows taps there: no MIN / MAX
/// session, no history slot).
pub fn row_hit(y: u16) -> bool {
    let y0 = row_y() as u16;
    (y0..y0 + 19).contains(&y)
}

/// Energy box tap target (the stock REL box; the parts layout: SIDE_P).
pub fn side_box_hit(x: u16, y: u16) -> bool {
    let y0 = side_geom().box_y as u16;
    x >= 402 && (y0..y0 + 38).contains(&y)
}

// ------------------------------------------------------------ entry dialog

/// Wiring dialog box and its buttons (screen coordinates, DBG:TAP).
pub const DLG: (i16, i16, i16, i16) = (10, 8, 460, 304);
pub const DLG_CANCEL: sf::Rect = rect(30, 262, 190, 40);
pub const DLG_OK: sf::Rect = rect(260, 262, 190, 40);
const DLG_TITLE: Color = 0x04FC;
const DLG_BODY: Color = 0x10A3;
const WIRE: Color = 0xFC40;

fn hit(r: sf::Rect, x: u16, y: u16) -> bool {
    let (x, y) = (x as i16, y as i16);
    x >= r.x && x < r.x + r.w && y >= r.y && y < r.y + r.h
}

/// Dialog hit: Some(true) OK, Some(false) Cancel.
pub fn entry_hit(x: u16, y: u16) -> Option<bool> {
    if hit(DLG_OK, x, y) {
        Some(true)
    } else if hit(DLG_CANCEL, x, y) {
        Some(false)
    } else {
        None
    }
}

/// A jack of the meter: a ring with its name (the case legend) under it.
fn jack(cx: i16, cy: i16, name: &str) {
    sf::rbox(rect(cx - 11, cy - 11, 22, 22), 11, DIM, DLG_BODY);
    sf::rbox(rect(cx - 6, cy - 6, 12, 12), 6, DLG_BODY, DIM);
    sf::label(sf::F14, rect(cx - 36, cy + 14, 72, 14), name, STATUS_FG, DLG_BODY, 8, 1);
}

/// Outline of `r`, `t` px thick (lines, not a filled shape: the jacks and
/// the wires sit inside it).
fn outline(r: sf::Rect, t: i16, c: Color) {
    let (x, y, w, h) = (r.x as u16, r.y as u16, r.w as u16, r.h as u16);
    let t = t as u16;
    lcd::fill_rect(x, y, w, t, c);
    lcd::fill_rect(x, y + h - t, w, t, c);
    lcd::fill_rect(x, y + t, t, h - 2 * t, c);
    lcd::fill_rect(x + w - t, y + t, t, h - 2 * t, c);
}

/// Wire segment (2 px).
fn vwire(x: i16, y0: i16, y1: i16) {
    lcd::fill_rect(x as u16, y0 as u16, 2, (y1 - y0) as u16, WIRE);
}

/// Wiring picture geometry, as on the meter (emu/panel_draw.py, the
/// manual's jacks.png): the meter on the left with its jacks stacked on
/// its right side, V/ohm (top), COM, A (bottom), each with its case legend
/// under it; source and load right of it. JACK_X: the jacks' centre x;
/// JV / JC / JA their centre y; INT_X the meter's internal wire (x, left
/// of the legends); SRC_X / LOAD_X the boxes' terminal x; BOX_Y / BOX_H the
/// boxes; WIRE_P the + wire (y).
const JACK_X: i16 = 170;
const JV: i16 = 64;
const JC: i16 = 124;
const JA: i16 = 184;
const INT_X: i16 = 60;
const SRC_X: i16 = 289;
const LOAD_X: i16 = 410;
const BOX_Y: i16 = 60;
const BOX_H: i16 = 53;
const WIRE_P: i16 = 50;
/// V tap: from the V jack right to TAP_X, up to the + wire.
const TAP_X: i16 = 236;
const SHUNT: sf::Rect = rect(INT_X - 5, 140, 12, 28);

/// The current path of the wiring dialog, in the direction of the current
/// (P > 0: the load current into A): wire runs (x, y, dx, dy, len) 2 px
/// wide, x / y the first pixel, (dx, dy) the step. SOURCE + -> LOAD +;
/// (the load) LOAD - -> A; inside the meter A -> shunt -> COM (the jacks
/// and the shunt are gaps); COM -> SOURCE -. No wire crosses another.
const FLOW_PATH: [(i16, i16, i16, i16, i16); 11] = [
    (SRC_X, BOX_Y - 1, 0, -1, BOX_Y - 1 - WIRE_P - 1),
    (SRC_X, WIRE_P, 1, 0, LOAD_X + 2 - SRC_X),
    (LOAD_X, WIRE_P + 2, 0, 1, BOX_Y - WIRE_P - 2),
    (LOAD_X, BOX_Y + BOX_H, 0, 1, JA - 1 - BOX_Y - BOX_H),
    (LOAD_X + 1, JA - 1, -1, 0, LOAD_X + 2 - (JACK_X + 12)),
    (JACK_X - 12, JA - 1, -1, 0, JACK_X - 12 - INT_X - 1),
    (INT_X, JA, 0, -1, JA - (SHUNT.y + SHUNT.h) + 1),
    (INT_X, SHUNT.y - 1, 0, -1, SHUNT.y - 1 - JC),
    (INT_X, JC - 1, 1, 0, JACK_X - 12 - INT_X + 1),
    (JACK_X + 12, JC - 1, 1, 0, SRC_X + 2 - (JACK_X + 12)),
    (SRC_X, JC - 2, 0, -1, JC - 2 - (BOX_Y + BOX_H) + 1),
];
/// Arrowheads on the path: the distance along FLOW_PATH of each one's base
/// (the tip is ARROW_L - 1 px further, in the current's direction): two on
/// the + wire, down from the load, two on the wire into A, the internal
/// wire left, up (after the shunt) and right into COM, out of COM.
const ARROWS: [i16; 9] = [48, 98, 169, 269, 379, 479, 556, 608, 717];
const ARROW_L: i16 = 10;
/// Marching dashes on the path: on / period (px), colour; the dashes move
/// FLOW_STEP px per frame (`entry_anim`, 10 fps: 30 px/s).
const FLOW_ON: i16 = 6;
const FLOW_P: i16 = 14;
const FLOW_STEP: u32 = 3;
const FLOW: Color = 0xFFFF;

/// Pixel `k` of path run `(x, y, dx, dy, _)`: its top-left corner.
fn run_px(x: i16, y: i16, dx: i16, dy: i16, k: i16) -> (i16, i16) {
    (x + k * dx, y + k * dy)
}

/// The arrowheads (FLOW colour, 2 x 5 px wider than the wire at the base),
/// at the ARROWS distances along FLOW_PATH.
fn arrows() {
    let mut s = 0i16;
    for &(x, y, dx, dy, len) in FLOW_PATH.iter() {
        for &a in ARROWS.iter() {
            if a >= s && a + ARROW_L <= s + len {
                for k in 0..ARROW_L {
                    let (px, py) = run_px(x, y, dx, dy, a - s + k);
                    let hw = (ARROW_L - 1 - k) * 5 / (ARROW_L - 1);
                    if dx != 0 {
                        lcd::fill_rect(px as u16, (py - hw) as u16, 1, (2 + 2 * hw) as u16, FLOW);
                    } else {
                        lcd::fill_rect((px - hw) as u16, py as u16, (2 + 2 * hw) as u16, 1, FLOW);
                    }
                }
            }
        }
        s += len;
    }
}

/// Dash phase `ph` (frames) of the wiring dialog's current path: the wires
/// of FLOW_PATH repainted as runs of WIRE and FLOW (about 560 px x 2),
/// leaving the arrowheads' spans alone.
pub fn entry_anim(ph: u32) {
    let off = ((ph * FLOW_STEP) % FLOW_P as u32) as i16;
    let mut s = 0i16; // distance along the path at a run's first pixel
    for &(x, y, dx, dy, len) in FLOW_PATH.iter() {
        // 0 WIRE, 1 FLOW, 2 an arrowhead (not painted).
        let kind = |k: i16| {
            if ARROWS.iter().any(|&a| (a..a + ARROW_L).contains(&(s + k))) {
                2
            } else {
                ((s + k - off).rem_euclid(FLOW_P) < FLOW_ON) as u8
            }
        };
        let mut i = 0;
        while i < len {
            // Run of equal kind from pixel i.
            let c = kind(i);
            let mut j = i + 1;
            while j < len && kind(j) == c {
                j += 1;
            }
            if c != 2 {
                // Pixels i..j-1 along (dx, dy): a rect from the lower corner.
                let (a, b) = (i.min(j - 1), i.max(j - 1));
                let (x0, x1) = if dx < 0 { (x - b, x - a) } else { (x + a * dx, x + b * dx) };
                let (y0, y1) = if dy < 0 { (y - b, y - a) } else { (y + a * dy, y + b * dy) };
                let (w, h) = if dx != 0 { (x1 - x0 + 1, 2) } else { (2, y1 - y0 + 1) };
                lcd::fill_rect(x0 as u16, y0 as u16, w as u16, h as u16, if c == 1 { FLOW } else { WIRE });
            }
            i = j;
        }
        s += len;
    }
}

/// The power-meter entry dialog (a modal overlay, dialog.rs): the wiring of
/// doc 3.6 (V/ohm on the source side, the return conductor broken through
/// A and COM), the current path with arrowheads (through the meter's
/// shunt), the sign convention, the current rating, Cancel / OK.
/// `i_cont` / `t_lim`: the continuous-current limit setting.
pub fn entry_dialog(i_cont: f32, t_lim: u16) {
    let (x, y, w, h) = DLG;
    lcd::fill_rect(x as u16, y as u16, w as u16, 34, DLG_TITLE);
    sf::glyphs(sf::TEXT22, rect(x, y, w, 34), "Power meter: wiring", STATUS_FG, DLG_TITLE, 8, 0);
    lcd::fill_rect(x as u16, (y + 34) as u16, w as u16, (h - 34) as u16, DLG_BODY);
    // Source and load: rounded boxes with + / - terminals (wires at the
    // top / bottom edge centre).
    for (bx, name) in [(SRC_X + 1 - 40, "SOURCE"), (LOAD_X + 1 - 40, "LOAD")] {
        sf::rbox(rect(bx, BOX_Y, 80, BOX_H), 8, BTN, DLG_BODY);
        sf::glyphs(sf::F14, rect(bx, BOX_Y + 1, 80, 16), "+", STATUS_FG, BTN, 8, 1);
        sf::glyphs(sf::F14, rect(bx, BOX_Y + BOX_H / 2 - 8, 80, 16), name, STATUS_FG, BTN, 8, 1);
        sf::glyphs(sf::F14, rect(bx, BOX_Y + BOX_H - 17, 80, 16), "-", STATUS_FG, BTN, 8, 1);
    }
    // The meter: an outline around its jacks and the internal path.
    let m = rect(28, 44, 180, 174);
    outline(m, 1, DIM);
    sf::label(sf::F14, rect(m.x + 6, m.y + 4, 46, 14), "DM40", DIM, DLG_BODY, 0, 1);
    // Jacks top to bottom with the case legends.
    jack(JACK_X, JV, "V\u{3a9}");
    jack(JACK_X, JC, "COM");
    jack(JACK_X, JA, "A mA uA");
    // V taps the + wire on the source side (left of SOURCE +).
    lcd::fill_rect((JACK_X + 12) as u16, (JV - 1) as u16, (TAP_X + 2 - JACK_X - 12) as u16, 2, WIRE);
    vwire(TAP_X, WIRE_P, JV + 1);
    lcd::fill_rect(TAP_X as u16, WIRE_P as u16, (SRC_X - TAP_X) as u16, 2, WIRE);
    // The shunt between A and COM (the internal wire's gap), named by it.
    outline(SHUNT, 2, WIRE);
    sf::label(sf::F14, rect(SHUNT.x + SHUNT.w + 4, SHUNT.y + 7, 44, 14), "shunt", DIM, DLG_BODY, 0, 1);
    // The load current's name by the arrow on the wire down from the load.
    sf::label(sf::F14, rect(LOAD_X + 10, 140, 14, 14), "I", STATUS_FG, DLG_BODY, 8, 1);
    // The current path: marching dashes (moved by `entry_anim` while the
    // dialog is open) and the arrowheads.
    entry_anim(0);
    arrows();
    // Notes: what COM carries, the sign, the rating.
    let mut t = T::new();
    let mut b = [0u8; 16];
    t.push("Max 10 A; above ").push(sig_text(&mut b, i_cont, 2)).push(" A: ");
    let mut b2 = [0u8; 16];
    if t_lim > 0 {
        t.push(crate::dmm::format(&mut b2, t_lim as f32, 0)).push(" s");
    } else {
        t.push("no limit");
    }
    sf::label(sf::F14, rect(x + 16, 222, w - 32, 14), "COM carries the load current; P > 0: I into A", STATUS_FG, DLG_BODY, 8, 1);
    sf::label(sf::F14, rect(x + 16, 240, w - 32, 14), t.s(), VALUE, DLG_BODY, 8, 1);
    for (r, l, c) in [(DLG_CANCEL, "Cancel", BTN), (DLG_OK, "OK", SEL)] {
        sf::rbox(r, 6, c, DLG_BODY);
        sf::glyphs(sf::TEXT22, r, l, STATUS_FG, c, 8, 0);
    }
}

// ------------------------------------------------------------ settings page

/// Power Settings page touch codes (besides range items).
pub const PS_V: u8 = 0; // + 0..3 = V range, 4 = V AUTO
pub const PS_I: u8 = 8; // + 0..5 = I range, 6 = I AUTO
/// "At load": the burden compensation (P / V at the load, the meter's own
/// drop I^2 R_path / I R_path taken off).
pub const PS_BURDEN: u8 = 22;
pub const PS_RESET: u8 = 23;
pub const PS_WIRING: u8 = 24;
pub const PS_OK: u8 = 25;
pub const PS_ZERO: u8 = 26;
/// + 0..2 = the coupling row DC / AC / AC+DC (`PS_COUPS`).
pub const PS_COUP: u8 = 28;
/// The coupling row, left to right, as the Current Settings page's DC / AC /
/// AC+DC row (doc 3.14; the meaning of the selected one in the help line).
pub const PS_COUPS: [u8; 3] = [pw::COUP_DC, pw::COUP_AC, pw::COUP_ACDC];

/// The help line's topic (`settings_status`): the selected coupling (the
/// default, after a coupling tap) or the "At load" switch (after its tap).
pub const HELP_COUP: u8 = 0;
pub const HELP_LOAD: u8 = 1;

/// One line per coupling (the Power Settings help line, the Help page).
pub fn coup_help(c: u8) -> &'static str {
    match c {
        pw::COUP_DC => "DC: P = Vdc x Idc; no f, PF, S, Q",
        pw::COUP_AC => "AC: AC parts only (DC removed); P, PF, S, Q, f",
        _ => "AC+DC: total; P = Pac + Vdc x Idc, rms of both",
    }
}
pub const PS_VCAL: u8 = 27;
/// Hold & Limits (View::Limits: the W limits), "Limits" as on the other
/// settings pages.
pub const PS_LIMITS: u8 = 31;
/// The Record page (View::Record), beside Limits as on the DMM range pages.
pub const PS_RECORD: u8 = 32;
/// The bottom row, left to right: Reset Wh, Wiring, Limits, Record, OK
/// (8..472, 6 px gaps; Reset Wh, 87 px of text, the widest button).
const PS_ROW5: [(u8, &str); 5] =
    [(PS_RESET, "Reset Wh"), (PS_WIRING, "Wiring"), (PS_LIMITS, "Limits"), (PS_RECORD, "Record"), (PS_OK, "OK")];
const PS_ROW5_X: [i16; 6] = [8, 114, 206, 298, 390, 478];

fn ps_row5(i: usize) -> sf::Rect {
    rect(PS_ROW5_X[i], 240, PS_ROW5_X[i + 1] - PS_ROW5_X[i] - 6, 38)
}

/// Grid: label column 8..40, five 83 px columns from 44 (87 px pitch).
fn ps_cell(row: i16, col: i16) -> sf::Rect {
    rect(44 + 87 * col, 46 + 44 * row, 83, 38)
}

const PS_ROW4: [i16; 3] = [8, 162, 319];

fn ps_button(r: sf::Rect, t: &str, sel: bool) {
    let c = if sel { SEL } else { 0x2966 };
    sf::rbox(r, 6, c, BG);
    sf::glyphs(sf::TEXT22, r, t, STATUS_FG, c, 8, 0);
}

/// The page: V ranges + AUTO, I ranges + AUTO (two rows), "At load" (the
/// burden compensation), Zero and V cal (each a dialog that says what
/// it needs first); the coupling row (DC / AC / AC+DC, the Current Settings
/// page's style); Reset energy / Wiring / Limits / OK; the status and help
/// lines under it (`settings_status`). The VA-only fallback (POW:METH ALT)
/// is SCPI only (status line).
#[inline(never)] // .cold (link-native.x): page draw
pub fn settings_page(pv: &PowerVals) {
    lcd::fill_rect(0, 0, lcd::W, 40, 0x2966);
    lcd::fill_rect(0, 40, lcd::W, lcd::H - 40, BG);
    sf::glyphs(sf::TEXT22, rect(0, 0, 480, 40), "Power Settings", STATUS_FG, 0x2966, 8, 0);
    sf::label(sf::TEXT22, rect(8, 46, 32, 38), "V", STATUS_FG, BG, 8, 0);
    sf::label(sf::TEXT22, rect(8, 90, 32, 38), "A", STATUS_FG, BG, 8, 0);
    for (i, n) in pw::V_NAMES.iter().enumerate() {
        ps_button(ps_cell(0, i as i16), n, !pv.v_auto && pv.vr as usize == i);
    }
    ps_button(ps_cell(0, 4), "AUTO", pv.v_auto);
    for (i, n) in pw::I_NAMES.iter().enumerate() {
        ps_button(ps_cell((1 + i / 5) as i16, (i % 5) as i16), n, !pv.i_auto && pv.ik as usize == i);
    }
    ps_button(ps_cell(2, 1), "AUTO", pv.i_auto);
    // Burden compensation (selected = on), self-zero, V self-calibration
    // (doc 3.12); the status line under the last row (`settings_status`).
    ps_button(ps_cell(2, 2), "At load", pv.comp);
    ps_button(ps_cell(2, 3), "Zero", false);
    ps_button(ps_cell(2, 4), "V cal", false);
    for (i, c) in PS_COUPS.iter().enumerate() {
        ps_button(rect(PS_ROW4[i], 182, 150, 38), pw::coup_name(*c), pv.coup == *c);
    }
    for (i, &(c, t)) in PS_ROW5.iter().enumerate() {
        if c == PS_RECORD {
            super::record_button(ps_row5(i), false);
        } else {
            ps_button(ps_row5(i), t, false);
        }
    }
}

/// Pressed feedback on the Power Settings page (ui-review #11): button `c`
/// (PS_* as `settings_hit`) drawn pressed (`on`: the list rows' pressed
/// colour) or as `settings_page` draws it.
#[inline(never)] // .cold (link-native.x): per touch on the page
pub fn settings_press(pv: &PowerVals, c: u8, on: bool) {
    let (r, t, sel) = match c {
        0..=3 => (ps_cell(0, c as i16), pw::V_NAMES[c as usize], !pv.v_auto && pv.vr == c),
        4 => (ps_cell(0, 4), "AUTO", pv.v_auto),
        PS_I..=13 => {
            let i = c - PS_I;
            (ps_cell((1 + i / 5) as i16, (i % 5) as i16), pw::I_NAMES[i as usize], !pv.i_auto && pv.ik == i)
        }
        14 => (ps_cell(2, 1), "AUTO", pv.i_auto),
        PS_BURDEN => (ps_cell(2, 2), "At load", pv.comp),
        PS_ZERO => (ps_cell(2, 3), "Zero", false),
        PS_VCAL => (ps_cell(2, 4), "V cal", false),
        PS_COUP..=30 => {
            let i = (c - PS_COUP) as usize;
            (rect(PS_ROW4[i], 182, 150, 38), pw::coup_name(PS_COUPS[i]), pv.coup == PS_COUPS[i])
        }
        PS_RECORD => return super::record_button(ps_row5(3), on),
        _ => match PS_ROW5.iter().position(|&(k, _)| k == c) {
            Some(i) => (ps_row5(i), PS_ROW5[i].1, false),
            None => return,
        },
    };
    let bg = if on { super::K_SEL } else if sel { SEL } else { 0x2966 };
    sf::rbox(r, 6, bg, BG);
    sf::glyphs(sf::TEXT22, r, t, STATUS_FG, bg, 8, 0);
}

/// The Zero... / V cal... dialog (dialog.rs fuse-check box, Cancel / Start;
/// lines of <= 30 characters).
pub fn job_dialog(job: u8) {
    let (title, l1, l2) = if job == pw::JOB_ZERO {
        ("Zero the power meter", "Leads open, nothing in A", "Takes about 12 s")
    } else {
        ("V cal of this V range", "Steady V, 10-70 % of range", "Gain vs DC V / AC V, 8 s")
    };
    crate::dialog::fuse_check(title, crate::dialog::TITLE_BG, l1, l2, Some("Cancel"), Some("Start"));
}

/// Settings page hit (PS_* codes).
pub fn settings_hit(x: u16, y: u16) -> Option<u8> {
    for row in 0..3i16 {
        for col in 0..5i16 {
            let r = ps_cell(row, col);
            if hit(rect(r.x - 2, r.y - 3, r.w + 4, r.h + 6), x, y) {
                return match (row, col) {
                    (0, c) => Some(PS_V + c as u8),
                    (1, c) => Some(PS_I + c as u8),
                    (2, 0) => Some(PS_I + 5),
                    (2, 1) => Some(PS_I + 6),
                    (2, 2) => Some(PS_BURDEN),
                    (2, 3) => Some(PS_ZERO),
                    (2, 4) => Some(PS_VCAL),
                    _ => None,
                };
            }
        }
    }
    for (i, &bx) in PS_ROW4.iter().enumerate() {
        if hit(rect(bx, 182, 150, 38), x, y) {
            return Some(PS_COUP + i as u8);
        }
    }
    for (i, (c, _)) in PS_ROW5.iter().enumerate() {
        let r = ps_row5(i);
        // Touch boxes up to the 6 px gaps' middle.
        if hit(rect(r.x - 3, r.y, r.w + 6, r.h), x, y) {
            return Some(*c);
        }
    }
    None
}

/// Status and help lines of the Power Settings page (under the last button
/// row, 282..318): the running job, the last result, else the gain and zero
/// in force; under it the meaning of the selected coupling or of "At load"
/// (`help`: HELP_*). Redrawn when the text differs from `key` (its FNV);
/// returns the new key.
pub fn settings_status(p: &pw::Pw, key: u32, help: u8) -> u32 {
    let mut t = T::new();
    let mut b = [0u8; 16];
    let vr = if p.job_last == pw::JOB_VCAL && p.job == 0 { p.job_vr } else { p.vr } as usize;
    let rn = pw::V_NAMES[vr & 3];
    let res = if p.job != 0 { pw::JR_RUN } else { p.job_res };
    let warn = match (p.job_last, res) {
        (pw::JOB_ZERO, pw::JR_RUN) => {
            t.push("Zero: leads open (12 s)");
            false
        }
        (pw::JOB_VCAL, pw::JR_RUN) => {
            t.push("V cal ").push(rn).push(": hold V steady (8 s)");
            false
        }
        (_, pw::JR_METHOD) => {
            t.push("Needs the P + PF method");
            true
        }
        (pw::JOB_ZERO, pw::JR_OK) => {
            t.push("Zero stored");
            false
        }
        (pw::JOB_ZERO, pw::JR_SIGNAL) => {
            t.push("Zero: signal present");
            true
        }
        (pw::JOB_VCAL, pw::JR_OK) => {
            t.push("V cal ").push(rn).push(": gain ").push(crate::dmm::format(&mut b, p.v_gain(vr as u8), 5)).push(" stored");
            false
        }
        (pw::JOB_VCAL, pw::JR_SMALL) => {
            t.push("V cal: < 10 % of ").push(rn);
            true
        }
        (pw::JOB_VCAL, pw::JR_BIG) => {
            t.push("V cal: too high for ").push(rn);
            true
        }
        (pw::JOB_VCAL, pw::JR_UNSTABLE) => {
            t.push("V cal: V not steady");
            true
        }
        (pw::JOB_VCAL, pw::JR_RANGE) => {
            t.push("V cal: off > 10 %");
            true
        }
        (_, pw::JR_ABORT) => {
            t.push("Aborted");
            true
        }
        _ => {
            // "V gain 6 V: 1.0207 factory, zero: not set"
            t.push("V gain ")
                .push(rn.trim_end_matches('V'))
                .push(" V: ")
                .push(crate::dmm::format(&mut b, p.v_gain(vr as u8), 4))
                .push(if p.vgain[vr] > 0.0 { " V cal" } else { " factory" })
                .push(match (p.alt, p.zero_ok) {
                    // The VA-only fallback (SCPI): no room for the zero.
                    (true, _) => ", VA only",
                    (false, true) => ", zero: set",
                    (false, false) => ", zero: not set",
                });
            false
        }
    };
    let hl = match help {
        // When to turn it on (doc 3.16): V leads at the source, the meter in
        // series, so the load gets V - I R_path; ON marks V / P "@load".
        HELP_LOAD if p.comp => "ON: the load sees V - I*R",
        HELP_LOAD => "OFF: V, P at the jacks",
        _ => coup_help(p.coup),
    };
    let mut k = T::new();
    k.push(t.s()).push(if warn { "w" } else { "" }).push(hl);
    let nk = k.key();
    if nk != key {
        sf::label(sf::F14, rect(8, 284, 464, 14), t.s(), if warn { VALUE } else { STATUS_FG }, BG, 1, 1);
        sf::label(sf::F14, rect(8, 302, 464, 14), hl, DIM, BG, 1, 1);
    }
    nk
}

/// The DMM page before the first full power reading (`Dmm::pw_partial`):
/// the V reading in the aux1 slot and the state text in the main area.
/// Returns the new key (drawn when it differs from `key`).
pub fn ranging(v: Option<crate::dmm::Val>, txt: &str, key: u32) -> u32 {
    let mut b = [0u8; 16];
    let vt = match v {
        Some(v) => crate::meter::text(&mut b, &v),
        None => "",
    };
    let mut k = T::new();
    k.push(vt).push("|").push(txt);
    let nk = k.key();
    if nk != key {
        // aux1: value right-aligned in 38..175, unit from 181 (DmmView::reading).
        sf::label(sf::LETTERS, rect(38, 36, 137, 30), vt, VALUE, BG, 4, 0);
        sf::label(sf::LETTERS, rect(181, 36, 73, 30), if vt.is_empty() { "" } else { "V" }, VALUE, BG, 1, 0);
        sf::label(sf::TEXT22, RANGING_TXT, txt, DIM, BG, 8, 0);
    }
    nk
}

/// The state text's box in the main readout area.
const RANGING_TXT: sf::Rect = rect(40, 92, 360, 32);

/// Before the first full reading is drawn over the `ranging` texts.
pub fn ranging_clear() {
    lcd::fill_rect(RANGING_TXT.x as u16, RANGING_TXT.y as u16, RANGING_TXT.w as u16, RANGING_TXT.h as u16, BG);
    lcd::fill_rect(38, 36, 216, 30, BG);
}
