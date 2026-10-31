//! The protocol decoder's frame log (View::Decode, Mode Log of the scope's
//! Decode tab; src/app_log.rs), drawn in the scope page's look (a
//! src/scope_ui.rs submodule: its badges, panel buttons, measurement cells
//! and the Decode tab's page).
//!
//!   0..20     top bar: run state (RUN / STOP / WAIT), protocol, bit rate
//!             and format, the threshold, INV (inverted polarity), battery
//!   20..301   the frame list in the graph frame: ROWS rows of ROW_H px,
//!             time (s since the first frame) | hex + ASCII or the Modbus
//!             fields | OK / FE / PE / CRC
//!   301..320  counters: frames, framing / parity errors, CRC errors, lost
//!   panel     page up / LIVE (follow the newest) / page down; SETUP,
//!             CLEAR, STOP / RUN; SCOPE (back to the scope page, Band)
//! SETUP (or the MF key) opens the Decode tab's page in a popup.
//! Region B: the view is drawn while the stream runs.

use super::*;
use crate::decode as dc;

pub const ROWS: usize = 16;
pub const ROW_H: i16 = 17;
const ROW_Y0: i16 = 22;
/// Row columns: time (right-aligned), content (UART: the hex, then the
/// ASCII at COL_A), status (centred).
const COL_T: (i16, i16) = (17, 74);
const COL_C: (i16, i16) = (107, 260);
const COL_A: (i16, i16) = (290, 78);
const COL_S: (i16, i16) = (373, 36);
/// The list's area inside the frame (taps / drags).
pub const LIST: Rect = rect(14, 21, 399, 279);

const C_OK: Color = 0x07e0;
const C_BAD: Color = 0xf800;
const C_GREY: Color = 0x9cd3;
/// The snippet's grid dots (the scope graph's).
const C_GRID: Color = 0x7bef;

/// One list row (texts formatted by the app).
pub struct Row<'a> {
    pub time: &'a str,
    pub text: &'a str,
    /// UART: the ASCII column ("" for Modbus / a note).
    pub ascii: &'a str,
    pub status: &'a str,
    /// Text colour (white; orange for a Modbus exception).
    pub fg: Color,
}

/// Top-bar values.
pub struct Top<'a> {
    /// 0 WAIT (no level / bit rate yet), 1 RUN, 2 STOP (paused).
    pub state: u8,
    pub proto: &'a str,
    pub rate: &'a str,
    pub thr: &'a str,
    pub inv: bool,
    pub battery: u8,
    pub charging: bool,
}

const PROTO_BADGE: Rect = rect(79, 1, 62, 17);
const RATE_BADGE: Rect = rect(145, 1, 120, 17);
const THR_BADGE: Rect = rect(269, 1, 79, 17);
const INV_BADGE: Rect = rect(352, 1, 62, 17);

/// Page background: bars, panel, the list frame and an empty list.
/// `between`: called between the parts (the decoder keeps up with the
/// stream: a full page is several passes' worth of drawing).
pub fn chrome(between: &mut dyn FnMut()) {
    lcd::fill_rect(0, 0, 480, 20, C_BAR);
    lcd::fill_rect(414, 20, 66, 300, C_BAR);
    lcd::fill_rect(0, 301, 414, 19, C_BAR);
    lcd::fill_rect(0, 20, 13, 281, C_BAR);
    lcd::fill_rect(13, 20, 401, 1, C_FRAME);
    lcd::fill_rect(13, 300, 401, 1, C_FRAME);
    lcd::fill_rect(13, 20, 1, 281, C_FRAME);
    lcd::fill_rect(413, 20, 1, 281, C_FRAME);
    between();
    for k in 0..4u16 {
        lcd::fill_rect(14, 21 + 70 * k, 399, if k == 3 { 69 } else { 70 }, C_FILL);
        between();
    }
}

/// Top-bar items (badges, battery): drawn one at a time (`top_item`), a
/// badge paint is a few ms.
pub const TOP_ITEMS: usize = 6;

/// A key of what top-bar item k shows (redraw when it changes).
pub fn top_key(k: usize, t: &Top) -> u32 {
    let h = |s: &str| s.bytes().fold(0x811c_9dc5u32, |a, b| (a ^ b as u32).wrapping_mul(0x0100_0193));
    match k {
        0 => t.state as u32,
        1 => h(t.proto),
        2 => h(t.rate),
        3 => h(t.thr),
        4 => t.inv as u32,
        _ => t.battery as u32 | (t.charging as u32) << 8,
    }
}

pub fn top_item(k: usize, t: &Top) {
    match k {
        0 => {
            let r = RUN_BADGE;
            let st = ["WAIT", "RUN", "STOP"][t.state.min(2) as usize];
            let i = Ink::new(sf::SC_C, r, st, 8, 0);
            badge(r, if t.state == 2 { C_ORANGE } else { C_PURPLE }, C_BAR, None, &[(&i, C_WHITE)]);
        }
        1 => {
            let p = Ink::new(sf::SC_F12, PROTO_BADGE, t.proto, 8, 0);
            badge(PROTO_BADGE, C_MAGENTA, C_BAR, None, &[(&p, C_WHITE)]);
        }
        2 => {
            let q = Ink::new(sf::SC_F12, RATE_BADGE, t.rate, 8, 0);
            badge(RATE_BADGE, C_YELLOW, C_BAR, None, &[(&q, C_FILL)]);
        }
        3 => {
            let h = Ink::new(sf::SC_F12, THR_BADGE, t.thr, 8, 0);
            badge(THR_BADGE, C_FRAME, C_BAR, Some((rect(THR_BADGE.x + 1, 2, THR_BADGE.w - 2, 15), C_BAR)), &[(&h, C_YELLOW)]);
        }
        4 => {
            if t.inv {
                let v = Ink::new(sf::SC_F12, INV_BADGE, "INV", 8, 0);
                badge(INV_BADGE, C_ORANGE, C_BAR, None, &[(&v, C_WHITE)]);
            } else {
                lcd::fill_rect(INV_BADGE.x as u16, INV_BADGE.y as u16, INV_BADGE.w as u16, INV_BADGE.h as u16, C_BAR);
            }
        }
        _ => battery_lv(t.battery, t.charging, 0),
    }
}

/// Panel button labels; 0 / 2 are arrows.
const PANEL_TXT: [&str; 7] = ["", "LIVE", "", "SETUP", "CLEAR", "STOP", "SCOPE"];

/// Right-panel button k (0..6): the scope page's groups (yellow: up / LIVE
/// / down; grey: SETUP / CLEAR / STOP-RUN; orange: SCOPE). `live`: the
/// list follows the newest frame (LIVE filled); `paused`: RUN instead of
/// STOP; `pressed`: k drawn pressed.
pub fn panel_button(k: usize, live: bool, paused: bool, pressed: bool) {
    let r = B_RECT[k];
    let (fill, radius, mask, fg) = match k {
        0 => (C_YELLOW, 8, 0x3, C_FILL),
        1 => (if live { C_YELLOW } else { C_FRAME }, 0, 0, C_FILL),
        2 => (C_YELLOW, 8, 0xc, C_FILL),
        3 => (C_FRAME, 8, 0x3, C_WHITE),
        4 => (C_FRAME, 0, 0, C_WHITE),
        5 => (C_FRAME, 8, 0xc, C_WHITE),
        _ => (C_ORANGE, 8, 0xf, C_WHITE),
    };
    let fill = if pressed { C_DIS } else { fill };
    let txt = if k == 5 && paused { "RUN" } else { PANEL_TXT[k] };
    let t = Ink::new(sf::SC_F16, r, txt, 8, 0);
    let arrow = match k {
        0 => Some(true),
        2 => Some(false),
        _ => None,
    };
    paint(r, &[(K_SHAPE, C_BAR), (K_TEXT, fill), (K_ICON, fill)], |x, y| {
        if let Some(up) = arrow {
            if tri(r, up, x, y) {
                return (fg, tag(3, 0));
            }
        }
        if let Some(g) = t.at(x, y) {
            return (fg, tag(2, g));
        }
        if mask == 0 || sf::in_rbox(r, radius, mask, x, y) { (fill, tag(1, 0)) } else { (C_BAR, 0) }
    });
}

/// A filled triangle (16 x 9) centred in `r`, pointing up or down.
fn tri(r: Rect, up: bool, x: i16, y: i16) -> bool {
    let (cx, cy) = (r.x + r.w / 2, r.y + r.h / 2);
    let dy = if up { y - (cy - 4) } else { (cy + 4) - y };
    (0..9).contains(&dy) && (x - cx).abs() <= dy
}

pub fn panel(live: bool, paused: bool, between: &mut dyn FnMut()) {
    for k in 0..7 {
        panel_button(k, live, paused, false);
        between();
    }
}

/// Panel button under (x, y).
pub fn panel_hit(x: u16, y: u16) -> Option<usize> {
    let (x, y) = (x as i16, y as i16);
    if x < PANEL.x {
        return None;
    }
    B_RECT.iter().position(|r| y >= r.y - 1 && y < r.y + r.h + 1)
}

/// The list row under y (taps).
pub fn row_at(y: i16) -> Option<usize> {
    let i = (y - ROW_Y0).div_euclid(ROW_H);
    (y >= ROW_Y0 && (i as usize) < ROWS).then_some(i as usize)
}

/// Row i of the list: a frame, or a grey note (`r.status` empty), or
/// nothing (None: the row cleared).
pub fn row(i: usize, r: Option<&Row>) {
    let y = ROW_Y0 + i as i16 * ROW_H;
    let Some(r) = r else {
        lcd::fill_rect(14, y as u16, 399, ROW_H as u16, C_FILL);
        return;
    };
    lcd::fill_rect(14, y as u16, COL_T.0 as u16 - 14, ROW_H as u16, C_FILL);
    sf::label(sf::SC_F12, rect(COL_T.0, y, COL_T.1, ROW_H), r.time, C_YELLOW, C_FILL, 4, 0);
    let gap = COL_C.0 - COL_T.0 - COL_T.1;
    lcd::fill_rect((COL_T.0 + COL_T.1) as u16, y as u16, gap as u16, ROW_H as u16, C_FILL);
    let note = r.status.is_empty();
    let w = if note { 413 - COL_C.0 } else if r.ascii.is_empty() { COL_C.1 } else { COL_A.0 - COL_C.0 };
    // Spacing 1 (Modbus fields, notes): some glyph pairs of the 12 px
    // font touch at 0.
    let sp = if r.ascii.is_empty() { 1 } else { 0 };
    sf::label(sf::SC_F12, rect(COL_C.0, y, w, ROW_H), r.text, if note { C_GREY } else { r.fg }, C_FILL, 1, sp);
    if !note {
        let mut e = COL_C.0 + w;
        if !r.ascii.is_empty() {
            // Spacing 1: the wide letters (M, W) of the 12 px font touch.
            sf::label(sf::SC_F12, rect(COL_A.0, y, COL_A.1, ROW_H), r.ascii, C_GREY, C_FILL, 1, 1);
            e = COL_A.0 + COL_A.1;
        }
        let g = COL_S.0 - e;
        lcd::fill_rect(e as u16, y as u16, g as u16, ROW_H as u16, C_FILL);
        let c = if r.status == "OK" { C_OK } else { C_BAD };
        sf::label(sf::SC_F12, rect(COL_S.0, y, COL_S.1, ROW_H), r.status, c, C_FILL, 8, 0);
        let e = COL_S.0 + COL_S.1;
        lcd::fill_rect(e as u16, y as u16, (413 - e) as u16, ROW_H as u16, C_FILL);
    }
}

/// The content column's width for a UART row (hex, then the ASCII) or a
/// Modbus row.
pub fn text_w(uart: bool) -> i16 {
    if uart { COL_A.0 - COL_C.0 } else { COL_C.1 }
}

/// Fit `s` (12 px font, `sp` spacing) into a box `w` px wide: the longest
/// prefix that fits, with ">" at its end when cut. Returns its length.
pub fn fit(s: &str, w: i16, sp: i8, out: &mut [u8; 64]) -> usize {
    let w = w - 2;
    let b = s.as_bytes();
    let mut n = b.len().min(out.len());
    out[..n].copy_from_slice(&b[..n]);
    if sf::text_width(sf::SC_F12, txt(&out[..n]), sp) <= w {
        return n;
    }
    while n > 1 {
        n -= 1;
        out[n] = b'>';
        if sf::text_width(sf::SC_F12, txt(&out[..n + 1]), sp) <= w {
            return n + 1;
        }
    }
    n
}

/// `fit` for the ASCII column.
pub fn fit_ascii(s: &str, out: &mut [u8; 64]) -> usize {
    fit(s, COL_A.1, 1, out)
}

pub fn txt(b: &[u8]) -> &str {
    core::str::from_utf8(b).unwrap_or("")
}

/// `v` with at most 5 characters: 12345, 123k, 12M.
fn short(w: &mut dc::W, v: u32) {
    if v < 100_000 {
        w.u(v);
    } else if v < 10_000_000 {
        w.u(v / 1000).c(b'k');
    } else {
        w.u(v / 1_000_000).c(b'M');
    }
}

/// Counter cell i (0..3): Frames, FE, PE (UART) / CRC (Modbus), Lost, of
/// v = frames, framing errors, parity errors, CRC errors, lost, Modbus.
pub fn cell_draw(i: usize, v: &[u32; 6]) {
    let mut b = [0u8; 24];
    let mut w = dc::W { b: &mut b, n: 0 };
    let (label, x) = match i {
        0 => ("Frames:", v[0]),
        1 => ("FE:", v[1]),
        2 if v[5] != 0 => ("CRC:", v[3]),
        2 => ("PE:", v[2]),
        _ => ("Lost:", v[4]),
    };
    w.s(label);
    short(&mut w, x);
    let n = w.n;
    cell(i, txt(&b[..n]));
}

/// What cell i shows (redraw when it changes).
pub fn cell_key(i: usize, v: &[u32; 6]) -> u32 {
    match i {
        0 => v[0],
        1 => v[1],
        2 if v[5] != 0 => v[3] | 1 << 31,
        2 => v[2] & !(1 << 31),
        _ => v[4],
    }
}

/// Counter cells (as the scope's measurement row).
pub fn cell(i: usize, text: &str) {
    let r = rect(13 + 100 * i as i16, 302, 98, 17);
    let t = Ink::new(sf::SC_F12, r, text, 8, 0);
    paint(r, &[(K_SHAPE, C_BAR), (K_TEXT, C_BAR)], |x, y| {
        if let Some(g) = t.at(x, y) {
            return (C_YELLOW, tag(2, g));
        }
        if sf::on_ring(r, 5, 0x0f, x, y) { (C_FRAME, tag(1, 0)) } else { (C_BAR, 0) }
    });
}

// ---- a frame's waveform (tap a row) ----

/// The snippet's trace area (under the frame's row, above the band).
const SN_TOP: i16 = 44;
const SN_BOT: i16 = 236;

/// The waveform of a frame in the list area: the graph's dotted grid,
/// the samples `s` over the 399 columns (min / max per column, joined),
/// scaled to their own swing. The frame's row (row 0) and the bus band
/// (ui_decode `band`) are drawn by the caller.
pub fn snippet(s: &[u16]) {
    lcd::fill_rect(14, ROW_Y0 as u16 + ROW_H as u16, 399, (300 - ROW_Y0 - ROW_H) as u16, C_FILL);
    for k in 1..8i16 {
        let x = 14 + k * 50;
        let mut y = SN_TOP;
        while y < SN_BOT {
            lcd::fill_rect(x as u16, y as u16, 1, 1, C_GRID);
            y += 5;
        }
    }
    for k in 0..4i16 {
        let y = SN_TOP + k * (SN_BOT - SN_TOP) / 3;
        let mut x = 14;
        while x < 413 {
            lcd::fill_rect(x as u16, y as u16, 1, 1, C_GRID);
            x += 5;
        }
    }
    let n = s.len();
    if n < 2 {
        return;
    }
    let (mn, mx) = s.iter().fold((u16::MAX, 0u16), |(a, b), &v| (a.min(v & 0x3FF), b.max(v & 0x3FF)));
    let span = (mx - mn).max(8) as i32;
    let h = (SN_BOT - SN_TOP - 24) as i32;
    let y_of = |c: u16| (SN_BOT as i32 - 12 - ((c & 0x3FF) as i32 - mn as i32) * h / span) as i16;
    let mut prev: Option<(i16, i16)> = None;
    for x in 0..399usize {
        let a = x * n / 399;
        let b = ((x + 1) * n / 399).max(a + 1).min(n);
        let (lo, hi) = s[a..b].iter().fold((u16::MAX, 0u16), |(l, h), &v| (l.min(v & 0x3FF), h.max(v & 0x3FF)));
        let (mut y0, mut y1) = (y_of(hi), y_of(lo));
        if let Some((p0, p1)) = prev {
            // Joined to the previous column (an edge is a vertical line).
            y0 = y0.min(p1);
            y1 = y1.max(p0);
        }
        prev = Some((y_of(hi), y_of(lo)));
        lcd::fill_rect(14 + x as u16, y0 as u16, 1, (y1 - y0 + 1) as u16, C_YELLOW);
    }
}

// ---- setup popup: the Decode tab's page under a "Decode" header ----

/// The popup box (as the MF popup with the Decode tab).
pub const SU_BOX: Rect = rect(13, 20, 401, 41 + dec::PAGE_H);

pub fn setup(v: &dec::SetupVals) {
    let screen = rect(0, 0, 480, 320);
    fill_c(SU_BOX, screen, C_FRAME);
    let r = rect(14, 21, 399, 40);
    fill_c(rect(14, 21, 399, 38), screen, C_POP_BG);
    fill_c(rect(14, 59, 399, 2), screen, C_POP_TAB);
    let t = Ink::new(sf::SC_F16, r, "Decode", 8, 0);
    paint(isect(t.bx, r), &[(K_TEXT, C_POP_BG)], |x, y| match t.at(x, y) {
        Some(g) => (C_FILL, tag(1, g)),
        None => (C_POP_BG, 0),
    });
    dec::page(v, rect(14, 61, 399, dec::PAGE_H - 1));
}

/// A tap on the setup popup: Some(hit) inside, None below it (closes);
/// elsewhere outside: Some(Inside) (ignored, modal).
pub fn setup_hit(x: i16, y: i16) -> Option<dec::SetupHit> {
    if !SU_BOX.has(x, y) {
        return if y >= SU_BOX.y + SU_BOX.h && x < 414 { None } else { Some(dec::SetupHit::Inside) };
    }
    // The log has no DALI (app_log.rs log_open).
    Some(dec::page_hit(false, x, y))
}
