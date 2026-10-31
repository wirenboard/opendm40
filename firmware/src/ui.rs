//! Screens. Geometry and colours of the multimeter page measured on read-backs
//! of the stock screen (firmware/ref/); the main reading is red instead of the
//! stock orange. No framebuffer: every element is drawn as complete cells /
//! columns so redraws do not flicker.

use crate::dmm::{Button, Reading};
use crate::gen;
use crate::lcd::{self, Color};
use crate::meter;

/// Power-meter screens (DMM page slots, entry dialog, settings page).
#[path = "ui_power.rs"]
pub mod pw;
/// The Inrush Current page (View::Inrush, app_inrush.rs).
#[path = "ui_inrush.rs"]
pub mod inr;
/// The touch test page (View::TouchTest, app_touchtest.rs).
#[path = "ui_touchtest.rs"]
pub mod touchtest;
/// The specified accuracy of a reading (../docs/dmm-accuracy.md).
#[path = "accuracy.rs"]
pub mod acc;

/// RGB565 of the stock colours (panel reads back RGB666).
pub const BG: Color = 0x0883; // (8, 16, 24)
pub const BAR: Color = 0x1904; // (24, 32, 32) status bar
pub const FG: Color = 0xF79E; // (240, 244, 240) text
pub const DIM: Color = 0x94D3; // (144, 152, 152) grid / frames
pub const BTN: Color = 0x2966; // (40, 44, 48) buttons / boxes
pub const SEL: Color = 0x0419; // (0, 128, 200) selected button
/// DMM readings (main value, extra digit, unit, mode glyph, aux values,
/// meter strip): the stock value colour GUI_C_VALUE 0xFC40 (RGB 248,136,0;
/// stock/re/analysis/gui_v2_6.h:157/169/243, every DMM value label of the
/// stock layout table gui_v2_6.h:330-339 uses it). Owner request: back to
/// the stock colour (an earlier opendm40 build drew the reading red).
pub const VALUE: Color = 0xFC40;
/// Stock value colour in the V functions from 30 V (gui_v2_6.h:157/175,
/// GUI_C_VALUE_HV); the lightning glyph comes on at 100 V.
pub const VALUE_HV: Color = 0xC9A0;
/// A provisional reading (`Reading::prov`: the new function's dashes, a
/// value a DMM gate still holds): the digits and aux values in the stock
/// grid / frame grey instead of VALUE, so they read as "not final".
pub const VALUE_PROV: Color = DIM;
/// The main readout's digits below the reading's uncertainty (the accuracy
/// label, ../docs/dmm-accuracy.md section 5), and the extra digit then.
pub const VALUE_LOW: Color = 0xB4D0; // (176, 152, 128): warm grey (owner 2026-10-26)

/// The continuity / diode beeper indicator (`DmmView::short`): the band's
/// fill, the stock "OK" badge green (`lim_badge`), and the digits' colour,
/// the stock RUN green (`gen_status`).
pub const SHORT_BG: Color = 0x04A9;
pub const SHORT_FG: Color = 0x07E0;
/// The indicator's band over the aux row (y 38..63, as the temperature
/// chip): 4 px under the status bar and above the digits' cells (68).
pub const SHORT_RECT: sf::Rect = rect(8, 38, 464, 26);
/// Indicator style, bit 0: the band with SHORT / BEEP; bit 1: the main
/// digits green. The firmware draws SHORT_STYLE; ui-render also draws the
/// others (`set_short_style`, the PR's comparison).
pub const SHORT_STYLE: u8 = 3;
#[cfg(test)]
static SHORT_STYLE_T: core::sync::atomic::AtomicU8 = core::sync::atomic::AtomicU8::new(SHORT_STYLE);
#[cfg(test)]
pub fn set_short_style(s: u8) {
    SHORT_STYLE_T.store(s, core::sync::atomic::Ordering::Relaxed);
}
#[cfg(test)]
fn short_style() -> u8 {
    SHORT_STYLE_T.load(core::sync::atomic::Ordering::Relaxed)
}
#[cfg(not(test))]
#[inline(always)]
fn short_style() -> u8 {
    SHORT_STYLE
}

pub const BAR_H: u16 = 34;
/// Left edge of the DMM status-bar items (the stock's range label box).
const BAR_X: i16 = 4;
/// Extra-digit cell of the main readout (x, width); see DmmView::reading.
/// 375..402, before the limit badge (403..433, y 72..88) and the unit box
/// (from 404); 28 px = the widest half-size F78 digit ("4", 56 / 2).
const XT_X: u16 = 375;
const XT_W: u16 = 28;
const VALUE_BASE: u16 = 146;
pub const BTN_Y: u16 = 280;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum View {
    Dmm,
    Scope,
    Gen,
    /// Multimeter reading on top, scope below.
    DmmScope,
    Menu,
    Settings,
    /// MF range page of the multimeter.
    Range,
    /// Generator value entry (frequency / Vpp / duty).
    Keypad,
    /// Device / About page (stock gui_v2_6.h section 6 "6.产品信息").
    Device,
    /// DMM AutoHold and Hi/Lo limits of the current function (MF).
    Limits,
    /// Record page (decision 2026-10-22 #1): the CSV recording's interval,
    /// duration, free-time estimate, Start / Stop; Hold & Limits -> Record.
    Record,
    /// Screenshot + log gallery (FR-16, gallery.rs).
    Gallery,
    /// On-device help (UF-29), Settings 2/3 -> Help: HELP pages.
    Help,
    /// Inrush current capture (Current Settings -> Inrush; app_inrush.rs).
    Inrush,
    /// Touch test (Settings 3/3 -> Touch test; app_touchtest.rs).
    TouchTest,
}

impl View {
    /// Views reachable by swiping, in order.
    pub const CYCLE: [View; 4] = [View::Dmm, View::DmmScope, View::Scope, View::Gen];
}

/// Which DMM function button a touch at (x, y) hits (the function row:
/// BTN_N buttons at `btn_x`, see DmmView::draw_static).
pub fn button_hit(x: u16, y: u16) -> Option<usize> {
    if y >= BTN_Y { Some(btn_col(x)) } else { None }
}

fn button_index(b: Button) -> usize {
    Button::ALL.iter().position(|&x| x == b).unwrap_or(0)
}

// ------------------------------------------------------------------ DMM
// Stock layout (gui_v2_6.h section 4), drawn with the stock fonts from flash.

use crate::stockfont::{self as sf, rect};

const STATUS_FG: Color = 0xF7BE;
/// Main-readout cell widths per format (%7.4f .. %7.1f, "   0L  ",
/// %7.0f: capacitance 6000 / 60000 uF, at most 5 digits, so cell 1 is
/// always blank).
const CELLS: [[u8; 7]; 6] = [
    [40, 60, 20, 60, 60, 60, 60],
    [40, 60, 60, 20, 60, 60, 60],
    [40, 60, 60, 60, 20, 60, 60],
    [40, 60, 60, 60, 60, 20, 60],
    [40, 52, 52, 52, 60, 60, 40],
    [40, 20, 60, 60, 60, 60, 60],
];
/// Main-readout cells with the extra digit (formats 0..3 only, never OL):
/// sign 30, point 18 (F78 '-' / '.' are 29 / 17 px), digits 57 (F78 digits
/// are <= 56 px wide): centred, every glyph keeps >= 1 px to its neighbours
/// = 333 px: 40..372, then the half-size digit cell XT_X..XT_X + XT_W
/// (375..402, >= 3 px after the last digit).
const CELLS_X: [[u8; 7]; 4] = [
    [30, 57, 18, 57, 57, 57, 57],
    [30, 57, 57, 18, 57, 57, 57],
    [30, 57, 57, 57, 18, 57, 57],
    [30, 57, 57, 57, 57, 18, 57],
];
/// AC+DC aux rows: aux1 = AC part, aux2 = DC part (AUX_TAG 0x05 AC / 0x04 DC).
const AUX_TAGS: [&str; 2] = ["\u{5}", "\u{4}"];
/// Full view: left edge of each aux row's tag slot and of its value box
/// (value right-aligned in AUX_VX..AUX_VX + 138; aux1's unit box ends at 231).
const AUX_TAG_X: [i16; 2] = [8, 232];
const AUX_VX: [i16; 2] = [38, 260];

/// UF-26: caption of the 4-20 mA loop percentage (DC mA, aux2 "%").
const LOOP_CAP: &str = "4-20mA";

/// Caption of the burden voltage readout (A functions, Settings 3/3
/// "Burden voltage"; doc 3.15): DC A aux1, AC A aux2.
const BURDEN_CAP: &str = "Burden";
/// Power meter, "At load" on (doc 3.16): the mark after the compensated V's
/// unit (aux1) and at the main readout's top left (P / its energy; ui::pw
/// `coupling`).
pub const LOAD_CAP: &str = "@load";

/// Lowest loop percentage shown: 3.6 mA, the NAMUR NE 43 failure limit
/// (below it a loop is broken or absent, not a signal: 0 mA read -25 %).
const LOOP_MIN_PCT: f32 = -2.5;

/// The aux readouts as shown. UF-26: the DC mA loop percentage (aux2 "%",
/// dmm.rs: (|mA| - 4) x 6.25, OL above 125 %) is hidden outside
/// LOOP_MIN_PCT..125 % (a current that is not a 4-20 mA loop signal); the
/// bool says it is that percentage (drawn with the LOOP_CAP caption). The
/// last item: the aux slot holding the burden voltage (`Reading::burden`,
/// drawn with BURDEN_CAP).
fn aux_shown(r: &Reading) -> ([Option<crate::dmm::Val>; 2], bool, Option<usize>) {
    let lp = r.mode == 8 && !r.parts() && r.aux2.is_some_and(|v| v.unit == "%");
    // Capacitance: aux2 is the discharge line (`cap_discharge_label`).
    let a2 = r.aux2.filter(|v| r.mode != 13 && (!lp || (!v.ol && (LOOP_MIN_PCT..=125.0).contains(&v.v))));
    ([r.aux1, a2], lp && a2.is_some(), r.burden().map(|b| b.0))
}

/// Mode glyph above the unit: 0x04 DC, 0x05 AC, 0x07 continuity, 0x08 diode.
fn mode_glyph(mode: u8, acdc: bool) -> &'static str {
    match mode {
        0..=9 if acdc => "\u{6}",
        0 | 2 | 4..=6 => "\u{5}",
        1 | 3 | 7..=9 => "\u{4}",
        11 => "\u{7}",
        12 => "\u{8}",
        _ => "",
    }
}

/// "%7.<dec>f" (right aligned in 7 characters), or the stock "   0L  ".
/// With `ext` (Val::ext, not OL): the value is formatted with dec + 1
/// decimals and split: the 7-character main part plus the last digit, which
/// is drawn small after it (so the main digits are never rounded up past it).
fn fmt7<'a>(buf: &'a mut [u8; 16], v: &crate::dmm::Val, ext: bool) -> (&'a str, usize, Option<u8>) {
    if v.ol || v.v.is_nan() {
        // NaN: no value yet ("--" where "0L" goes: the power meter's
        // pending reading).
        buf[..7].copy_from_slice(if v.ol { b"   0L  " } else { b"   --  " });
        return (core::str::from_utf8(&buf[..7]).unwrap_or(""), 4, None);
    }
    let mut ext = ext && v.ext && v.dec >= 1;
    let mut t = [0u8; 16];
    let mut full = crate::dmm::format(&mut t, v.v, v.dec + ext as u8);
    // The extra digit's layout has a 30 px sign cell (CELLS_X): a value
    // that needs it for a digit ("1562.00" F, the 1 is 33 px) or more
    // than 7 cells goes without the extra digit.
    if ext && (full.len() > 8 || (full.len() == 8 && !full.starts_with('-'))) {
        ext = false;
        full = crate::dmm::format(&mut t, v.v, v.dec);
    }
    let (s, x) = if ext {
        let l = full.len();
        (&full[..l - 1], Some(full.as_bytes()[l - 1]))
    } else {
        (full, None)
    };
    let n = s.len().min(7);
    let pad = 7 - n;
    buf[..pad].fill(b' ');
    buf[pad..7].copy_from_slice(&s.as_bytes()[s.len() - n..]);
    let f = match v.dec {
        4.. => 0,
        3 => 1,
        2 => 2,
        1 => 3,
        _ => 5,
    };
    (core::str::from_utf8(&buf[..7]).unwrap_or(""), f, x)
}

pub struct DmmView {
    /// Main readout as last drawn (text, cell format) for redrawing only the
    /// digits that changed; [7]: the first grey cell as last drawn
    /// (`acc::grey_from`; 7 = the extra digit only, 8 = none).
    last_txt: [u8; 8],
    last_fmt: usize,
    /// Extra (small) digit as last drawn, 0 = none.
    last_ext: u8,
    last_unit: &'static str,
    last_mode: u8,
    last_top: Option<(u8, u8, bool, crate::sentinel::Alert, bool)>,
    last_aux: [bool; 2],
    /// Alternation phase (the app's 1 s `strip_alt`): with HOLD / A-HOLD
    /// and an A-JACK / LIVE? alert the status slot shows the alert while it
    /// is true, the hold state while false (`slot_alert`). Set by the app
    /// before `reading`.
    pub alt: bool,
    /// Temperature sensor name shown instead of the range (set by the app,
    /// `None` outside the temperature function).
    pub label: Option<&'static str>,
    last_label: Option<&'static str>,
    /// Limit badge as last drawn (meter::V_*), 255 = not drawn.
    last_lim: u8,
    /// Temperature sensor chip as last drawn (aux2 slot), None = not drawn.
    last_chip: Option<&'static str>,
    /// Per aux row: the value part (text, AC+DC / loop flags) and the unit
    /// part (unit, AC+DC) as last drawn; None = repaint. A reading that
    /// leaves a part unchanged does not repaint it.
    aux_val: [Option<([u8; 16], u8)>; 2],
    aux_unit: [Option<(&'static str, u8)>; 2],
    /// UF-30: the fast bargraph in the history row's place (Settings 2/2
    /// "DMM row": Bargraph, the default / History). Set by the app before
    /// `draw_static`.
    pub bar_on: bool,
    bar: crate::bargraph::Bar,
    /// Bargraph scale as last labelled: (full scale bits, unit, bipolar),
    /// None = labels to draw; and the overload marker as last drawn.
    bar_scale: Option<(u32, &'static str, bool)>,
    bar_ol: Option<u8>,
    /// ui-review #13: draw the value in the stock >= 30 V red (set by the
    /// app before `reading`); as last drawn.
    pub hv_red: bool,
    /// The main digits' colour as last drawn (VALUE / VALUE_HV / VALUE_PROV).
    last_col: Color,
    /// Power meter shown (set by the app before `draw_static`): the
    /// history row holds the power boxes, never the bargraph.
    pub power: bool,
    pub pwc: pw::PwCache,
    /// The "ON" ohm variant is selected (`draw_static`'s `ohm_on`).
    ohm_on: bool,
    /// Test current label as last drawn: ohm range | variant << 3, 0x10
    /// diode, 255 = none drawn, 254 = to redraw / clear (after `invalidate`, or partly
    /// cleared).
    last_iohm: u8,
    /// The caption next to the main readout's "0L" as drawn: 0 none,
    /// 1 "Overflow", 2 "OPEN" (`ol_word`).
    ol_word: u8,
    /// The accuracy label's context (set by the app before `reading`; None
    /// = no label). The label is in the bargraph's label row, so the
    /// history row (Settings 2/3 "DMM row": History) and the power meter
    /// have none.
    pub acc: Option<Acc>,
    /// Settings 3/3 "Accuracy" (persisted, default ON; the app turns it
    /// into `acc`).
    pub acc_on: bool,
    /// The label as last drawn (`acc_ulps`, the decimals, the unit; None =
    /// empty) and whether ACC_RECT holds it (false: to draw).
    acc_k: Option<(u32, u8, &'static str)>,
    acc_drawn: bool,
    /// The beeper indicator to show (set by the app, `App::beep_ind`): 0
    /// none, 1 SHORT (continuity, a diode-mode short), 2 BEEP (a diode
    /// pattern); and the band as drawn (`short_sync`).
    pub short: u8,
    short_drawn: u8,
}

/// Bargraph geometry in the history row's place (rows 150..172 between the
/// F78 ink, which ends at 146, and the chart frame at 176):
///   segments  12..410 x 150..158 (50 x 8 px pitch, bargraph.rs)
///   labels    F14 row 161..172 under the scale: 0 / half / full scale
///             (bipolar: -FS / 0 / +FS), the full scale with its unit
///   OL marker 418..472 x 147..160 (red "OL" on overload)
pub const BAR_RECT: (u16, u16, u16, u16) = (12, 150, 400, 9);
const BAR_LBL_Y: i16 = 161;
const BAR_LBL_H: i16 = 12;
const BAR_OL: sf::Rect = rect(418, 147, 54, 14);
/// The accuracy label (../docs/dmm-accuracy.md): F14 grey in the label row,
/// right-aligned under the unit, 5 px after the full-scale label (which
/// ends at the bar's end, 410); `pm` draws the "+-" sign before the text.
const ACC_RECT: sf::Rect = rect(415, BAR_LBL_Y, 61, BAR_LBL_H);

/// What the accuracy label needs besides the reading (set by the app; None
/// = no label: Settings 3/3 "Accuracy" OFF, or REL in force).
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Acc {
    /// Dmm::model: 0 DM40A, 1 DM40B, 2 DM40C (the stock digit, acc::err_ulps).
    pub model: u8,
    /// The temperature sensor is the K thermocouple (the only one the
    /// manual specifies).
    pub tc: bool,
}

/// The absolute uncertainty of `r` per the manual (acc.rs) in units of its
/// last digit (`r.main.dec`, rounded up); None = no label (no row in the
/// manual, OL, dashes, a provisional value, the power meter).
#[inline(never)]
#[cfg_attr(target_os = "none", link_section = ".text.a2.acc")]
pub fn acc_ulps(r: &Reading, a: Acc) -> Option<u32> {
    let v = &r.main;
    if r.prov || v.ol || v.v.is_nan() || r.power.is_some() {
        return None;
    }
    // deg F: temperature is the only function with a unit ending in F
    // besides capacitance.
    let f = v.unit.as_bytes().last() == Some(&b'F');
    let sp = acc::spec(r.mode, r.range, r.acdc, a.tc, f)?;
    Some(acc::round_sig(acc::err_ulps(sp, v.v, v.dec, a.model)))
}

/// `ulps` x 10^-`dec` without trailing zeros ("0.002", "0.0015").
fn acc_fmt(b: &mut [u8; 16], mut ulps: u32, mut dec: u8) -> &str {
    while dec > 0 && ulps % 10 == 0 && ulps > 0 {
        ulps /= 10;
        dec -= 1;
    }
    let mut x = ulps as f32;
    for _ in 0..dec {
        x *= 0.1;
    }
    crate::dmm::format(b, x, dec)
}

use crate::scope_ui::{pm, PM_W};

/// The combined view's strip: the value's digits below the uncertainty
/// grey (`StripData::grey`); the accuracy label (`acc_ulps`, ASCII unit,
/// no space, as the sub readings) in the lower sub row when it is free,
/// else in the upper one; none when both hold a sub reading (AC V / A: Hz
/// and duty) or the "0L" caption.
#[inline(never)]
#[cfg_attr(target_os = "none", link_section = ".text.a2.acc")]
pub fn strip_acc(d: &mut crate::scope_ui::StripData, r: &Reading, acc: Option<Acc>) {
    let ulps = acc.and_then(|a| acc_ulps(r, a));
    // The value's digits below the uncertainty (section 5 of the doc).
    d.grey = acc::grey_from(d.value.as_str().as_bytes(), ulps) as u8;
    let Some(u) = ulps else { return };
    let Some(i) = [1, 0].into_iter().find(|&i| d.sub[i].as_str().is_empty()) else { return };
    let mut b = [0u8; 16];
    let mut t = crate::scope_ui::Txt::new(acc_fmt(&mut b, u, r.main.dec));
    ascii_unit(r.main.unit, &mut t);
    d.sub[i] = t;
    d.acc = i as u8 + 1;
}

/// The DMM function row: 7 buttons (the stock six + the power meter, owner
/// 2026-10-22) of BTN_W px at a BTN_PITCH pitch from x 4: 4 px gaps, 4 px to
/// both screen edges (4 + 6 x 68 + 64 = 476).
pub const BTN_N: usize = 7;
pub const BTN_W: i16 = 64;
pub const BTN_PITCH: i16 = 68;

/// The W button's coupling (pw::COUP_*): its glyph DC / AC / AC+DC as the
/// V / A buttons show theirs (`set_power_coup`).
static POWER_COUP: core::sync::atomic::AtomicU8 = core::sync::atomic::AtomicU8::new(0);

/// The W button's coupling for the next button draw (the app: the stored
/// coupling, the one in force in the power meter).
pub fn set_power_coup(c: u8) {
    POWER_COUP.store(c, core::sync::atomic::Ordering::Relaxed);
}

/// Power meter in AC+DC: the parts layout (doc 3.17, the owner's V2): the
/// DC / AC sub-lines under V and I (y 61) and under P, the main readout
/// PARTS_DY lower, the lower row ROW_DY lower, the chart 81 px from y 192,
/// the side column without VAh. Set by the app with the static draw (a
/// coupling change redraws the page); every other page and function keeps
/// the stock geometry (the flag is false there).
static PW_PARTS: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);
pub const PARTS_DY: i16 = 7;
pub const PARTS_ROW_DY: i16 = 17;

pub fn set_pw_parts(on: bool) {
    PW_PARTS.store(on, core::sync::atomic::Ordering::Relaxed);
}

pub fn pw_parts() -> bool {
    PW_PARTS.load(core::sync::atomic::Ordering::Relaxed)
}

/// The main readout's offset (0 outside the parts layout).
pub fn main_dy() -> i16 {
    if pw_parts() { PARTS_DY } else { 0 }
}

/// The stock MODE / BTN_GLYPH glyph of a power-meter coupling: DC, AC or
/// AC+DC, as `mode_glyph` gives V / A theirs.
pub fn pw_glyph(c: u8) -> &'static str {
    match c {
        crate::dmm::pw::COUP_DC => "\u{4}",
        crate::dmm::pw::COUP_AC => "\u{5}",
        _ => "\u{6}",
    }
}

/// AUTO coupling (V / A sub-mode 3) in force, for the next button draw:
/// the V / A button shows the AUTO glyph (`auto_pair`), the setting, not
/// the part AUTO shows (that is the glyph above the unit, "AUTO" beside it).
static CPL_AUTO: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// The AUTO coupling for the next button draw (the app: `Dmm::cpl_auto`).
pub fn set_cpl_auto(on: bool) {
    CPL_AUTO.store(on, core::sync::atomic::Ordering::Relaxed);
}

/// The V / A button in the AUTO coupling: the letter and our "AC/DC" mark
/// (BTN_GLYPH 0x09, stockfont_w.rs from tools/mkwglyph.py: the stock AC and
/// DC button glyphs scaled down, side by side with a slash), distinct from
/// the AC+DC glyph (the two stacked) and the plain DC / AC ones.
fn auto_pair(x: i16, y: i16, h: i16, c: Color, letter: &str) {
    btn_pair(x, y, h, c, (sf::LETTERS, letter, 0), (sf::BTN_GLYPH, "\u{9}", 0), 3);
}

/// Left edge of function button `i`.
pub const fn btn_x(i: usize) -> i16 {
    4 + BTN_PITCH * i as i16
}

/// Function button under x (the 4 px gaps split 2 / 2 between neighbours).
fn btn_col(x: u16) -> usize {
    (x.saturating_sub(2) as usize / BTN_PITCH as usize).min(BTN_N - 1)
}

/// Two glyph runs centred together in a function button (3 px apart at
/// their advance boxes): (font, text, dy) each.
fn btn_pair(x: i16, y: i16, h: i16, c: Color, a: (sf::Font, &str, i16), b: (sf::Font, &str, i16), gap: i16) {
    let (wa, wb) = (sf::text_width(a.0, a.1, 0), sf::text_width(b.0, b.1, 0));
    let x0 = x + (BTN_W - (wa + gap + wb)) / 2;
    sf::glyphs(a.0, rect(x0, y + a.2, wa + 1, h), a.1, STATUS_FG, c, 1, 0);
    sf::glyphs(b.0, rect(x0 + wa + gap, y + b.2, wb + 1, h), b.1, STATUS_FG, c, 1, 0);
}

/// Bottom button `i` (0..6) with its stock glyphs, in the box (x, y, BTN_W,
/// h) (DMM page: BTN_Y; combined view: the function band). The stock 76 px
/// buttons placed their glyphs at fixed offsets; at 64 px every button
/// centres its glyph run (`btn_pair`). `ohm_on`: the stock m+8 "ON" (low-
/// current, in-circuit) ohm variant is selected (dmm.rs `ohm_var`): the ohm
/// button carries the stock small "ON" tag (gui_v2_6.h: glyph 0x20 of font
/// 0x0806c724, 20 x 12) right of the ohm glyph. Button 6 = the power meter:
/// "W" in the LETTERS face (our W of that face, stockfont_w.rs).
fn dmm_button(i: usize, x: i16, y: i16, h: i16, outside: Color, selected: bool, mode: u8, acdc: bool, ohm_on: bool) {
    let c = if selected { SEL } else { BTN };
    sf::rbox(rect(x, y, BTN_W, h), 8, c, outside);
    let ac = matches!(mode, 0 | 2 | 4..=6);
    let sub = if acdc { "\u{6}" } else if ac { "\u{5}" } else { "\u{4}" };
    let auto = CPL_AUTO.load(core::sync::atomic::Ordering::Relaxed);
    match i {
        0 if auto && mode <= 3 => auto_pair(x, y, h, c, "V"),
        1 if auto && (4..=9).contains(&mode) => auto_pair(x, y, h, c, "A"),
        0 => btn_pair(x, y, h, c, (sf::LETTERS, "V", 0), (sf::BTN_GLYPH, if mode <= 3 { sub } else { "\u{4}" }, 0), 3),
        1 => btn_pair(x, y, h, c, (sf::LETTERS, "A", 0), (sf::BTN_GLYPH, if (4..=9).contains(&mode) { sub } else { "\u{4}" }, 0), 3),
        2 => {
            if ohm_on {
                let w = sf::text_width(sf::BTN_GLYPH, "\u{3a9}", 0);
                let x0 = x + (BTN_W - (w + 3 + 20)) / 2;
                sf::glyphs(sf::BTN_GLYPH, rect(x0, y - 1, w + 1, h), "\u{3a9}", STATUS_FG, c, 1, 0);
                sf::glyphs(sf::ON_TAG, rect(x0 + w + 3, y + h / 2 - 3, 21, 12), " ", STATUS_FG, c, 1, 0);
            } else {
                sf::glyphs(sf::BTN_GLYPH, rect(x, y - 1, BTN_W, h), "\u{3a9}", STATUS_FG, c, 8, 0);
            }
        }
        3 => sf::glyphs(sf::BTN_GLYPH, rect(x, y, BTN_W, h), "\u{11}", STATUS_FG, c, 8, 0),
        4 => btn_pair(x, y, h, c, (sf::BTN_GLYPH, "\u{8}", -1), (sf::BTN_GLYPH, "\u{7}", 0), 3),
        5 => btn_pair(x, y, h, c, (sf::BTN_GLYPH, "\u{10}", -2), (sf::BTN_GLYPH, "\u{2103}", 0), 3),
        _ => {
            let g = pw_glyph(POWER_COUP.load(core::sync::atomic::Ordering::Relaxed));
            btn_pair(x, y, h, c, (sf::LETTERS, "W", 0), (sf::BTN_GLYPH, g, 0), 3)
        }
    }
}

/// Function band of the combined view (a tap on the meter strip opens it):
/// the DMM page's six function buttons over the bottom of the graph, from
/// the 1 px line at FUNC_Y (3 px under the panel's T- button) down to the
/// measurement bar.
pub const FUNC_Y: u16 = 258;

pub fn func_band(button: Button, mode: u8, acdc: bool, ohm_on: bool) {
    lcd::fill_rect(0, FUNC_Y, lcd::W, MEAS_Y - FUNC_Y, S_BAR);
    // The scope panel's last two keys (V/div "-" at 232..271, TRIG at
    // 280..319) straddle the band: in the panel column the band runs from
    // the "-" key's top to the screen bottom, so no half key shows while
    // the band is open (band_close repaints the page).
    let py = crate::scope_ui::B_RECT[5].y as u16;
    lcd::fill_rect(BAND_PANEL_X, py, lcd::W - BAND_PANEL_X, lcd::H - py, S_BAR);
    lcd::fill_rect(BAND_PANEL_X, py, lcd::W - BAND_PANEL_X, 1, S_FRAME);
    lcd::fill_rect(0, FUNC_Y, lcd::W, 1, S_FRAME);
    let sel = button_index(button);
    // Power meter: the V / A buttons with their plain DC glyphs.
    let mode = if button == Button::Power { 255 } else { mode };
    for i in 0..BTN_N {
        dmm_button(i, btn_x(i), FUNC_Y as i16 + 4, 36, S_BAR, i == sel, mode, acdc, ohm_on);
    }
}

/// Left edge of the scope page's right panel (scope_ui::PANEL), whose last
/// key ends below the measurement bar.
const BAND_PANEL_X: u16 = 414;

/// Function button (0..6) of the band under a tap, None outside the band.
pub fn func_band_hit(x: u16, y: u16) -> Option<usize> {
    if (FUNC_Y..MEAS_Y).contains(&y) { Some(btn_col(x)) } else { None }
}

// ------------------------------------------------------------ chips

/// Chip fill (temperature sensor pill).
const CHIP: Color = 0x3186;

/// Temperature sensor chip (owner 2026-10-13: the sensor type must be
/// legible at a glance): the full T_LABELS name in TEXT22 on a CHIP pill,
/// on the DMM page in the aux2 slot, in the meter strip as the second sub
/// row. Pill: text + 2 x CHIP_PAD wide.
const CHIP_PAD: i16 = 8;

/// Copy `parts` into `buf` (truncated), returning the text.
fn join<'a>(buf: &'a mut [u8; 24], parts: &[&str]) -> &'a str {
    let mut n = 0;
    for p in parts {
        let k = p.len().min(24 - n);
        buf[n..n + k].copy_from_slice(&p.as_bytes()[..k]);
        n += k;
    }
    core::str::from_utf8(&buf[..n]).unwrap_or("")
}

impl DmmView {
    pub const fn new() -> Self {
        DmmView {
            last_txt: [0, 0, 0, 0, 0, 0, 0, 8],
            last_fmt: 255,
            last_ext: 0,
            last_unit: "",
            last_mode: 255,
            last_top: None,
            last_aux: [false; 2],
            alt: false,
            label: None,
            last_label: None,
            last_lim: 255,
            last_chip: None,
            aux_val: [None; 2],
            aux_unit: [None; 2],
            bar_on: true,
            bar: crate::bargraph::Bar::new(BAR_RECT.0, BAR_RECT.1, BAR_RECT.2, BAR_RECT.3, false),
            bar_scale: None,
            bar_ol: None,
            hv_red: false,
            last_col: VALUE,
            power: false,
            pwc: pw::PwCache::new(),
            ohm_on: false,
            last_iohm: 254,
            ol_word: 0,
            acc: Some(Acc { model: 2, tc: true }),
            acc_on: true,
            acc_k: None,
            acc_drawn: false,
            short: 0,
            short_drawn: 0,
        }
    }

    /// The beeper indicator's band holds the aux row: `reading` leaves it.
    fn band(&self) -> bool {
        self.short_drawn != 0
    }

    /// Draw or clear the beeper indicator's band (SHORT_RECT) after a
    /// change of `short`; a clear hands the aux row back to `reading`
    /// (its next call repaints the aux values and the test current).
    pub fn short_sync(&mut self) {
        let want = if self.power || short_style() & 1 == 0 { 0 } else { self.short };
        if want == self.short_drawn {
            return;
        }
        self.short_drawn = want;
        let b = SHORT_RECT;
        if want != 0 {
            sf::rbox(b, 6, SHORT_BG, BG);
            // The scope's 26 px face (the status bar's F21 lacks B / E).
            sf::glyphs(sf::ASCII26, b, if want == 1 { "SHORT" } else { "BEEP" }, 0xFFFF, SHORT_BG, 8, 2);
        } else {
            lcd::fill_rect(b.x as u16, b.y as u16, b.w as u16, b.h as u16, BG);
        }
        // The band covers / uncovers the aux values and the test current:
        // nothing of them is on the screen now.
        self.aux_val = [None; 2];
        self.aux_unit = [None; 2];
        self.last_aux = [false; 2];
        self.last_iohm = 255;
    }

    /// The accuracy label of `r` (the end of `reading`; `ulps`: `acc_ulps`
    /// of it under `acc`): drawn on a change.
    #[inline(never)]
    #[cfg_attr(target_os = "none", link_section = ".text.a2.acc")]
    fn acc_update(&mut self, r: &Reading, ulps: Option<u32>) {
        if !self.bar_on || self.power {
            return;
        }
        let k = ulps.map(|u| (u, r.main.dec, r.main.unit));
        if self.acc_drawn && self.acc_k == k {
            return;
        }
        self.acc_k = k;
        self.acc_drawn = true;
        self.acc_draw();
    }

    /// Paint ACC_RECT with the label as kept: "+-0.0020V" right-aligned,
    /// grey F14 as the scale labels; the number alone when that is too wide
    /// ("0.0060MHz": the unit is the readout's, right above).
    #[inline(never)]
    #[cfg_attr(target_os = "none", link_section = ".text.a2.acc")]
    fn acc_draw(&self) {
        let b = ACC_RECT;
        lcd::fill_rect(b.x as u16, b.y as u16, b.w as u16, b.h as u16, BG);
        let Some((u, dec, unit)) = self.acc_k else { return };
        let mut nb = [0u8; 16];
        let mut ub = [0u8; 16];
        let num = acc_fmt(&mut nb, u, dec);
        let mut t = crate::scope_ui::Txt::new(num);
        t.push(sf::unit_text(unit, &mut ub));
        let t = if sf::text_width(sf::F14, t.as_str(), 1) <= b.w - PM_W { t } else { crate::scope_ui::Txt::new(num) };
        let w = sf::text_width(sf::F14, t.as_str(), 1);
        let x = b.x + b.w - w;
        sf::label(sf::F14, rect(x, b.y, w, b.h), t.as_str(), DIM, BG, 4, 1);
        pm(x - PM_W, b.y + 1, DIM);
    }

    /// Bargraph scale labels for full scale `fs` in `unit` (empty = none,
    /// temperature).
    fn bar_labels(&self, fs: f32, unit: &str) {
        lcd::fill_rect(8, BAR_LBL_Y as u16, 468, BAR_LBL_H as u16, BG);
        // The row clear took the accuracy label too.
        if self.acc_drawn {
            self.acc_draw();
        }
        if unit.is_empty() {
            return;
        }
        let num = |b: &mut [u8; 16], v: f32| -> usize {
            let mut t = [0u8; 16];
            let s = crate::dmm::format(&mut t, v, 2);
            let s = if s.as_bytes().contains(&b'.') { s.trim_end_matches('0').trim_end_matches('.') } else { s };
            let n = s.len().min(16);
            b[..n].copy_from_slice(&s.as_bytes()[..n]);
            n
        };
        // The unit as the readout and the accuracy label beside it spell
        // it (F14 has the ohm glyph): "600 KΩ", not "600 kOhm".
        let mut ub = [0u8; 16];
        let us = sf::unit_text(unit, &mut ub);
        let (bx, mid, end) = (self.bar.x as i16, self.bar.mid_x() as i16, self.bar.end_x() as i16);
        // Left / centre / right labels.
        let mut l = [0u8; 16];
        let mut c = [0u8; 16];
        let mut r = [0u8; 24];
        let mut v = [0u8; 16];
        let k = num(&mut v, fs);
        let (nl, nc, mut nr);
        if self.bar.bipolar {
            l[0] = b'-';
            l[1..1 + k].copy_from_slice(&v[..k]);
            nl = 1 + k;
            c[0] = b'0';
            nc = 1;
            r[0] = b'+';
            r[1..1 + k].copy_from_slice(&v[..k]);
            nr = 1 + k;
        } else {
            l[0] = b'0';
            nl = 1;
            nc = num(&mut c, fs / 2.0);
            r[..k].copy_from_slice(&v[..k]);
            nr = k;
        }
        sf::label(sf::F14, rect(bx, BAR_LBL_Y, 80, BAR_LBL_H), core::str::from_utf8(&l[..nl]).unwrap_or(""), DIM, BG, 1, 1);
        r[nr] = b' ';
        let k = us.len().min(24 - nr - 1);
        r[nr + 1..nr + 1 + k].copy_from_slice(&us.as_bytes()[..k]);
        nr += 1 + k;
        sf::label(sf::F14, rect(mid - 40, BAR_LBL_Y, 81, BAR_LBL_H), core::str::from_utf8(&c[..nc]).unwrap_or(""), DIM, BG, 8, 1);
        sf::label(sf::F14, rect(end - 120, BAR_LBL_Y, 120, BAR_LBL_H), core::str::from_utf8(&r[..nr]).unwrap_or(""), DIM, BG, 4, 1);
    }

    /// UF-30: feed the bargraph with a raw (un-smoothed, pre-REL) reading;
    /// bipolar in DC V / mV / A (not AC+DC), unipolar elsewhere, empty in
    /// temperature (no meaningful full scale). No-op in history mode.
    pub fn bar_update(&mut self, r: &Reading) {
        if !self.bar_on || self.power {
            return;
        }
        let temp = r.mode == 15;
        let fs = crate::bargraph::mode_full_scale(r.mode, r.range, r.main.dec, r.main.unit);
        let key = if temp { (0, "", false) } else { (fs.to_bits(), r.main.unit, self.bar.bipolar) };
        if self.bar_scale != Some(key) {
            self.bar_scale = Some(key);
            self.bar_labels(fs, key.1);
        }
        let f = if temp { Some(0.0) } else { crate::bargraph::frac(r.main.v, fs, r.main.ol) };
        // The overflow mark: "OL", "OPEN" in continuity / diode.
        let ol = if f.is_some() { 0 } else if meter::open(r) { 2 } else { 1 };
        if self.bar_ol != Some(ol) {
            self.bar_ol = Some(ol);
            sf::label(sf::F14, BAR_OL, ["", "OL", "OPEN"][ol as usize], 0xF800, BG, 1, 1);
        }
        self.bar.update(f);
    }

    /// Static part of the standalone DMM page.
    pub fn draw_static(&mut self, button: Button, mode: u8, acdc: bool, ohm_on: bool) {
        lcd::fill_rect(0, BAR_H, lcd::W, lcd::H - BAR_H, BG);
        lcd::fill_rect(0, 0, lcd::W, BAR_H, BAR);
            if self.bar_on && !self.power {
            self.bar.bipolar = matches!(mode, 1 | 3 | 7..=9) && !acdc;
            self.bar.draw_static(BG);
            self.bar_scale = None;
            self.bar_ol = None;
        } else if !self.power {
            // (The power meter draws its own boxes: ui::pw power_row.)
            for i in 0..5i16 {
                sf::rbox(rect(5 + 95 * i, 153, 88, 19), 3, BAR, BG);
            }
        }
        self.ohm_on = ohm_on;
        let sel = button_index(button);
        let mode = if button == Button::Power { 255 } else { mode };
        // 36 px tall (rows 280..315) and from x 4: 4 px off the screen edges.
        for i in 0..BTN_N {
            dmm_button(i, btn_x(i), BTN_Y as i16, 36, BG, i == sel, mode, acdc, ohm_on);
        }
        self.invalidate();
    }

    pub fn invalidate(&mut self) {
        self.last_label = Some("");
        self.last_fmt = 255;
        self.last_unit = "";
        self.last_mode = 255;
        self.last_top = None;
        self.last_lim = 255;
        self.last_chip = None;
        self.aux_val = [None; 2];
        self.aux_unit = [None; 2];
        self.pwc = pw::PwCache::new();
        self.last_iohm = 254;
        self.acc_drawn = false;
        self.short_drawn = 0;
    }

    /// Draw a reading on the standalone DMM page. `hold`: Meter::hold_state
    /// (0 RUN, 1 HOLD, 2 AutoHold tracking, 3 AutoHold captured); `lim`:
    /// Meter::verdict.
    pub fn reading(&mut self, r: &Reading, hold: u8, lim: u8) {
        let mut buf = [0u8; 16];
        let (s, f, ext) = fmt7(&mut buf, &r.main, true);
        let mut ub = [0u8; 16];
        let unit = sf::unit_text(r.main.unit, &mut ub);
        // ui-review #13: the stock >= 30 V cue, the value in VALUE_HV (V
        // functions, set by the app from the live reading); a provisional
        // value in VALUE_PROV. A colour change repaints every digit.
        // The beeper indicator (`short`): the digits green.
        let color = if r.prov {
            VALUE_PROV
        } else if self.short != 0 && short_style() & 2 != 0 {
            SHORT_FG
        } else if self.hv_red {
            VALUE_HV
        } else {
            VALUE
        };
        let recolor = self.last_col != color;
        self.last_col = color;
        let vcol = if r.prov { VALUE_PROV } else { VALUE };
        // The digits below the uncertainty (doc dmm-accuracy.md section 5):
        // grey from cell `grey` on (7: the extra digit only, 8: none).
        let ulps = if self.power { None } else { self.acc.and_then(|a| acc_ulps(r, a)) };
        let grey = if ulps.is_some() { acc::grey_from(s.as_bytes(), ulps).min(7) as u8 } else { 8 };
        let last_grey = self.last_txt[7];
        self.last_txt[7] = grey;
        // Only the changed digits are redrawn (cell layout unchanged).
        let same_layout = self.last_fmt == f && (self.last_ext != 0) == ext.is_some() && !recolor;
        let prev_txt = self.last_txt;
        let prev = if same_layout { core::str::from_utf8(&prev_txt[..7]).unwrap_or("") } else { "" };
        let n = s.len().min(7);
        self.last_txt[..n].copy_from_slice(&s.as_bytes()[..n]);
        self.last_fmt = f;
        let last_ext = self.last_ext;
        self.last_ext = ext.unwrap_or(0);
        // Bit 4: AUTO coupling (the mode is the part shown), bits 5..6: the
        // power coupling (1 DC, 2 AC, 3 AC+DC), 7: AC+DC.
        let pk = |c: u8| match c {
            crate::dmm::pw::COUP_DC => 1u8,
            crate::dmm::pw::COUP_AC => 2,
            _ => 3,
        };
        let mkey = r.mode | (r.cpl_auto as u8) << 4 | (r.acdc as u8) << 7 | r.power.map_or(0, |p| pk(p.coup) << 5);
        // Extra digit: the main cells narrow to CELLS_X (40..372) and the
        // digit is the F78 glyph at half size (~38 px vs ~76) in
        // XT_X..XT_X + XT_W (375..402), on the F78 baseline (VALUE_BASE),
        // left of the limit badge / unit box (x >= 403). The full-draw
        // tail fill of the main cells clears the 373..374 gap, the digit
        // paints its whole 80 px high cell, and without the digit the main
        // cells span 40..400 again, so no stale pixels remain. x < 40
        // stays free for the HV warning glyph.
        let (w, cells) = match ext {
            Some(_) => (XT_X - 40, &CELLS_X[f & 3]),
            None => (360, &CELLS[f]),
        };
        // The power meter's parts layout: PARTS_DY lower (0 elsewhere).
        let md = main_dy();
        // The caption after the "0L" (`ol_word`): cleared before the cells
        // (a value's digits reach into its box), drawn after them.
        let ow = rect(OL_WORD.x, OL_WORD.y + md, OL_WORD.w, OL_WORD.h);
        let ok = if !r.main.ol { 0 } else if meter::open(r) { 2 } else { 1 };
        if ok != self.ol_word && self.ol_word != 0 {
            lcd::fill_rect(ow.x as u16, ow.y as u16, ow.w as u16, ow.h as u16, BG);
            self.ol_word = 0;
        }
        sf::cells_diff(sf::F78, rect(40, 68 + md, w as i16, 80), s, prev, cells, color, BG, (VALUE_LOW, grey, last_grey));
        if ok != 0 && (!same_layout || self.ol_word != ok) {
            ol_word(ow, color, ok == 2);
            self.ol_word = ok;
        }
        if let Some(e) = ext {
            if !same_layout || last_ext != e || (grey < 8) != (last_grey < 8) {
                let r = rect(XT_X as i16, 68 + md, XT_W as i16, 80);
                sf::label_half(sf::F78, r, e as char, VALUE_BASE as i16 + md, if grey < 8 { VALUE_LOW } else { color }, BG);
            }
        } else if last_ext != 0 {
            // The full cells end at 400: the rest of the +1 digit cell.
            lcd::fill_rect(400, (68 + md) as u16, XT_X + XT_W - 400, 80, BG);
        }
        if self.last_unit != r.main.unit || self.last_mode != mkey {
            self.last_unit = r.main.unit;
            self.last_mode = mkey;
            // The aux row layout follows the mode (AC+DC tags, loop caption).
            self.aux_val = [None; 2];
            self.aux_unit = [None; 2];
            // From x 404: clear of the +1 digit cell (375..402).
            // Power meter fallback "VA": its V and A touch at the stock -1.
            // The UNIT face has no 'h' (power meter energy: Wh / VAh):
            // LETTERS, 1 px apart when that fits the box (W and h touch).
            let mut uf = if sf::has_all(sf::UNIT, unit) { sf::UNIT } else { sf::LETTERS };
            let mut sp = if r.main.unit.ends_with("VA") {
                1
            } else if uf.0 != sf::UNIT.0 {
                // V / A of the stock overhang by 1: 2 px spacing for "VAh".
                if sf::text_width(uf, unit, 2) <= 72 { 2 } else { 0 }
            } else {
                -1
            };
            // Wider than the 72 px box ("MHz", "mW" in UNIT, "mWh"): the
            // narrower LETTERS, else TEXT22 (device check 92fb8fa: no unit
            // is cut).
            for (f, s) in [(sf::LETTERS, 0), (sf::TEXT22, 1)] {
                if sf::text_width(uf, unit, sp) <= 72 {
                    break;
                }
                if sf::has_all(f, unit) {
                    (uf, sp) = (f, s);
                }
            }
            sf::label(uf, rect(404, 114 + md, 72, 32), unit, VALUE, BG, 4, sp);
            // Stock box (437,76,32,32) align 7, widened 3 px right so the
            // 35 px AC glyph is not clipped (ends at 471, 3 px after the
            // limit badge 403..433).
            // Power meter: the glyph of the coupling (DC / AC / AC+DC), as
            // V / A show theirs; the limit badge left of it (ui::pw::coupling).
            let g = match r.power {
                Some(p) if self.power => pw_glyph(p.coup),
                _ => mode_glyph(r.mode, r.acdc || r.power.is_some()),
            };
            sf::label(sf::MODE, rect(437, 76 + md, 35, 32), g, VALUE, BG, 7, 0);
            // The power meter's "@load" mark slot (ui::pw `coupling` redraws
            // it with the next reading; no other function paints there).
            let m = pw::LOAD_MARK;
            lcd::fill_rect(m.x as u16, (m.y + md) as u16, m.w as u16, m.h as u16, BG);
            self.pwc.coup_invalidate();
            // The AUTO-coupling label shares the limit badge's slot.
            self.last_lim = 255;
            if !r.parts() && !self.band() {
                // Leaving AC+DC / AUTO: the tag slots left of the aux values
                // (the ohm test current label's left part).
                for i in 0..2 {
                    lcd::fill_rect(AUX_TAG_X[i] as u16, 36, (AUX_VX[i] - AUX_TAG_X[i]) as u16, 30, BG);
                }
                if self.last_iohm != 255 {
                    self.last_iohm = 254;
                }
            }
        }
        // Aux readouts (stock: value right-aligned cells at y 40, unit after).
        let (mut aux, lp, bd) = aux_shown(r);
        let band = self.band();
        if band {
            // The beeper indicator's band holds the row (`short_sync`).
            aux = [None; 2];
        }
        // Ohm / continuity / diode: the test current, small, in the empty aux1 slot
        // (`test_current_label`). Cleared before the aux rows (an aux1 that
        // replaces it paints over the cleared box), drawn after them (the
        // aux1 clear below and the unit change above mark it for a redraw).
        let ik = match r.mode {
            _ if band => 255,
            10 if aux[0].is_none() && !self.power => r.range.clamp(0, 7) as u8 | (self.ohm_on as u8) << 3,
            12 if aux[0].is_none() && !self.power => 0x10,
            13 if !self.power => match r.aux2 {
                Some(v) if v.unit == crate::dmm::CAP_CHG_UNIT => 0x21,
                Some(_) => 0x20,
                None => 255,
            },
            11 if aux[0].is_none() && !self.power => 0x11,
            _ => 255,
        };
        if (ik == 255 || (ik & 0xF0 == 0x20) != (self.last_iohm & 0xF0 == 0x20)) && self.last_iohm != 255 {
            let b = if self.last_iohm & 0xF0 == 0x20 { CAP_DIS_RECT } else { OHM_I_RECT };
            lcd::fill_rect(b.x as u16, b.y as u16, b.w as u16, b.h as u16, BG);
            self.last_iohm = 255;
        }
        let temp = r.mode == 15 && self.label.is_some();
        let load = r.power.is_some_and(|p| p.comp) && self.power;
        for (i, a) in aux.iter().enumerate() {
            let bdi = bd == Some(i);
            // Value boxes end 4 px before the unit (separate items: >= 3 px).
            let (vx, ux) = (AUX_VX[i] + 30, if i == 0 { 178 } else { 400 });
            if i == 1 && temp {
                // Sensor chip (CHIP_PAD note), right-aligned in the aux2
                // slot (260..476); redrawn only on a change.
                let l = self.label.unwrap_or("");
                if self.last_chip != self.label || !self.last_aux[1] {
                    let w = sf::text_width(sf::TEXT22, l, 0) + 2 * CHIP_PAD;
                    let c = rect(472 - w, 38, w, 26);
                    lcd::fill_rect(260, 36, (c.x - 260) as u16, 30, BG);
                    lcd::fill_rect(c.x as u16, 36, (476 - c.x) as u16, 2, BG);
                    lcd::fill_rect(c.x as u16, 64, (476 - c.x) as u16, 2, BG);
                    lcd::fill_rect(472, 38, 4, 26, BG);
                    sf::rbox(c, 6, CHIP, BG);
                    sf::glyphs(sf::TEXT22, c, l, FG, CHIP, 8, 0);
                    self.last_chip = self.label;
                    self.aux_val[1] = None;
                    self.aux_unit[1] = None;
                }
                self.last_aux[1] = true;
                continue;
            }
            if i == 1 && self.last_chip.take().is_some() {
                // Leaving temperature: the chip spans columns the value /
                // unit boxes below do not repaint (397..398).
                lcd::fill_rect(260, 36, 216, 30, BG);
                self.aux_val[1] = None;
                self.aux_unit[1] = None;
            }
            match a {
                Some(v) => {
                    let mut t = [0u8; 16];
                    let txt = meter::text(&mut t, v);
                    let mut u = [0u8; 16];
                    let mut key = [0u8; 16];
                    let kn = txt.len().min(16);
                    key[..kn].copy_from_slice(&txt.as_bytes()[..kn]);
                    let vkey = Some((key, (i == 1 && lp) as u8 | (r.parts() as u8) << 1 | (bdi as u8) << 2 | (r.prov as u8) << 3 | (r.main.ol as u8) << 4));
                    if self.aux_val[i] == vkey {
                        // Value part unchanged.
                    } else if bdi {
                        // Burden voltage (doc 3.15): the F14 caption right-
                        // aligned before the value, boxes tiling the slot as
                        // the loop percentage's (aux2: 256..396; aux1:
                        // 8..175, the value's box ending where the plain
                        // one does), so no older text stays behind.
                        let (cx, vx0, vw) = if i == 0 { (8, 68, 107) } else { (256, 316, 81) };
                        sf::label(sf::LETTERS, rect(vx0, 36, vw, 30), txt, vcol, BG, 4, 0);
                        sf::label(sf::F14, rect(cx, 36, 60, 30), BURDEN_CAP, VALUE, BG, 4, 1);
                    } else if i == 1 && lp {
                        // UF-26: "-25.0".."125.0" (<= 78 px: ink from 319)
                        // right-aligned in 316..396, the F14 caption
                        // right-aligned in 256..315 (ink from ~272: clear of
                        // aux1's unit box, which ends at 254; the None
                        // branch below clears it). The two boxes tile
                        // 256..396, so an older text leaves nothing behind.
                        sf::label(sf::LETTERS, rect(316, 36, 81, 30), txt, vcol, BG, 4, 0);
                        sf::label(sf::F14, rect(256, 36, 60, 30), LOOP_CAP, VALUE, BG, 4, 1);
                    } else if r.parts() {
                        // AC+DC / AUTO: the part's tag (AC for aux1, DC for aux2)
                        // right before its value, which stays right-aligned
                        // in its box: tag box from AUX_TAG_X to 4 px left
                        // of the text (both labels repaint their bg).
                        let end = vx - 30 + 137;
                        let te = (end - sf::text_width(sf::LETTERS, txt, 0) - 4).max(AUX_TAG_X[i] + 24);
                        sf::label(sf::AUX_TAG, rect(AUX_TAG_X[i], 36, te - AUX_TAG_X[i], 30), AUX_TAGS[i], VALUE, BG, 4, 0);
                        sf::label(sf::LETTERS, rect(te, 36, end - te, 30), txt, vcol, BG, 4, 0);
                    } else if v.ol && !r.main.ol {
                        // "0L" with "Overflow" (F14) before it, unless the
                        // main readout's caption already says it (the
                        // power meter's V / I are OL only with it): two
                        // boxes tiling the plain one, so a value after it
                        // leaves nothing behind.
                        sf::label(sf::F14, rect(vx - 30, 36, 81, 30), "Overflow", vcol, BG, 4, 1);
                        sf::label(sf::LETTERS, rect(vx + 51, 36, 56, 30), txt, vcol, BG, 4, 0);
                    } else {
                        sf::label(sf::LETTERS, rect(vx - 30, 36, 137, 30), txt, vcol, BG, 4, 0);
                    }
                    if self.aux_val[i] != vkey {
                        // The power meter's parts sub-line under it.
                        self.pwc.sub_invalidate(i);
                    }
                    self.aux_val[i] = vkey;
                    // AC+DC: the aux1 unit box (V..uA, <= 48 px) ends at 231,
                    // the aux2 tag slot starts at 232; else full width (kHz 57 px).
                    // Power meter, "At load" on: "@load" after the V unit
                    // (doc 3.16), in the unit box's own columns.
                    let lm = i == 0 && load;
                    let ukey = Some((v.unit, r.parts() as u8 | (lm as u8) << 1));
                    if self.aux_unit[i] != ukey {
                        self.aux_unit[i] = ukey;
                        self.pwc.sub_invalidate(i);
                        let uw = if i == 0 && r.parts() { 54 } else { 76 };
                        lcd::fill_rect((ux - 1) as u16, 36, 4, 30, BG);
                        // Energy units (Wh, VAh: power meter): the 'h'
                        // ascender is above the caps; 1 px lower, its glyphs
                        // 1 px apart (W and h touch at 0).
                        let hh = v.unit.ends_with('h') as i16;
                        let ut = sf::unit_text(v.unit, &mut u);
                        let sp = if hh == 1 && sf::text_width(sf::LETTERS, ut, 2) <= uw - 3 { 2 } else { 0 };
                        lcd::fill_rect((ux + 3) as u16, 36, (uw - 3) as u16, 1, BG);
                        sf::label(sf::LETTERS, rect(ux + 3, 36 + hh, uw - 3, 30 - hh), ut, VALUE, BG, 1, sp);
                        if lm {
                            let mx = ux + 3 + sf::text_width(sf::LETTERS, ut, sp) + 6;
                            sf::label(sf::F14, rect(mx, 36, ux + uw - mx, 30), LOAD_CAP, VALUE, BG, 1, 1);
                        }
                    }
                    self.last_aux[i] = true;
                }
                None if self.last_aux[i] => {
                    // aux1 from the burden caption's left edge (8; empty
                    // otherwise outside AC+DC); AC+DC / AUTO (a placeholder
                    // without its parts): from the part's tag slot.
                    let x0 = if i == 0 || r.parts() { AUX_TAG_X[i] } else { vx - 30 };
                    lcd::fill_rect(x0 as u16, 36, (ux + 76 - x0) as u16, 30, BG);
                    if i == 0 && self.last_iohm != 255 {
                        self.last_iohm = 254;
                    }
                    self.last_aux[i] = false;
                    self.aux_val[i] = None;
                    self.aux_unit[i] = None;
                }
                None => {}
            }
        }
        if ik & 0xF0 == 0x20 {
            // Every reading: the node voltage moves.
            cap_discharge_label(r.aux2.map_or(0.0, |v| v.v), ik == 0x21, self.last_iohm != ik);
            self.last_iohm = ik;
        } else if ik != 255 && self.last_iohm != ik {
            test_current_label(r.mode, r.range, self.ohm_on);
            self.last_iohm = ik;
        }
        // The phase is part of the key only while it alternates: no
        // repaint every second otherwise.
        // The key holds the auto state itself: AUTO+ and a manual range
        // both have auto_plus set (v_auto stays 1 under a manual range), so
        // the old key (auto && !auto_plus, .., auto_plus) was the same for
        // both and a range released by a reading (REL ended by an AUTO
        // coupling switch) left the manual label on screen (device check
        // 92fb8fa). Manual: the range itself (mode * 8 + range), as the
        // label shows it ("6V" -> "60V" repaints).
        let rk = if r.auto { 255 } else { r.mode.wrapping_mul(8).wrapping_add(r.range as u8) };
        let top = (rk, hold, r.auto_plus, r.alert, self.alt && alert_alternates(hold, r.alert));
        // Temperature: the status-bar slot shows the range state like the
        // other functions (the sensor name is the chip above; the deg F second
        // readout that used to sit here is gone: one global unit, owner).
        if self.last_top != Some(top) || self.last_label != self.label {
            self.last_top = Some(top);
            self.last_label = self.label;
            let s = range_text(r);
            if let (Some(l), false) = (self.label, temp) {
                sf::label(sf::F14, rect(BAR_X, 1, 105, 32), l, STATUS_FG, BAR, 1, 1);
            } else {
                range_status(s);
            }
            hold_label(hold, r.alert, self.alt);
        }
        // Power meter: the badge shares its slot with the AUTO coupling
        // label (ui::pw `coupling` draws whichever applies).
        // AUTO coupling (V / A sub-mode 3): "AUTO" in the badge slot left
        // of the glyph (DC / AC = the part it shows), as the power meter's
        // former AUTO; a limit verdict takes the slot (OK / HI / LO matter
        // more; the glyph still says DC / AC). The status bar's AUTO is
        // the range.
        let pwr = r.power.filter(|_| self.power);
        let lk = if r.cpl_auto && lim == crate::meter::V_NONE { 254 } else { lim };
        if self.last_lim != lk {
            self.last_lim = lk;
            if lk == 254 {
                auto_cpl_label();
            } else if pwr.is_none() {
                lim_badge(lim);
            }
        }
        if let Some(pv) = pwr {
            self.power_row(&pv, lim);
        }
        self.acc_update(r, ulps);
    }

}

/// The "0L" caption's box on the DMM page: the empty cells after the
/// "0L" of the main readout (CELLS[4]: "0L" ends at x 304), bottom-aligned
/// on the digits' baseline; left of the limit badge / unit box (x >= 403).
const OL_WORD: sf::Rect = rect(312, VALUE_BASE as i16 - 22, 86, 22);

/// What "0L" means in `r`, in the readout's colour and F14 (the old
/// "Overload" caption's font): "OPEN" (continuity / diode, open leads:
/// meter::open) or "Overflow" (owner 2026-10-30). Region B (F14: blob in
/// A2, index in B), not .cold: an OL can start while the fast logger's AD2
/// stream runs (V / A functions).
#[inline(never)] // drawn on entering OL only
fn ol_word(r: sf::Rect, c: Color, open: bool) {
    sf::label(sf::F14, r, if open { "OPEN" } else { "Overflow" }, c, BG, 2, 1);
}

/// The test current label's box: the aux1 slot's left part (the burden
/// caption's place in the A functions).
const OHM_I_RECT: sf::Rect = rect(8, 36, 120, 30);
/// The capacitance discharge line: the aux1 slot, to the aux2 tag (231).
const CAP_DIS_RECT: sf::Rect = rect(8, 36, 223, 30);

/// Capacitance, before a ramp range: "Discharging... 0.42 V", or
/// "Discharging... >1.2 V" while the node sits at the AD1 clamp (`clamp`;
/// owner 2026-10-31: the orange "Charged cap: discharge first" read as an
/// order to the user). DIM. `full`: the whole line (the other line was
/// there), else only the value part.
#[inline(never)] // .cold (link-native.x): during a discharge only
pub fn cap_discharge_label(v: f32, clamp: bool, full: bool) {
    const PRE: &str = "Discharging...";
    let b = CAP_DIS_RECT;
    // (The value from a word space after the prefix.)
    let w = sf::text_width(sf::F14, PRE, 1) + 6;
    if full {
        lcd::fill_rect(b.x as u16, b.y as u16, b.w as u16, b.h as u16, BG);
        sf::label(sf::F14, rect(b.x, b.y, w, b.h), PRE, DIM, BG, 1, 1);
    }
    let mut t = [0u8; 16];
    let mut s = crate::scope_ui::Txt::new("");
    if clamp {
        s.push(">1.2");
    } else {
        s.push(crate::dmm::format(&mut t, v, 2));
    }
    s.push(" V");
    sf::label(sf::F14, rect(b.x + w, b.y, b.w - w, b.h), s.as_str(), DIM, BG, 1, 1);
}

/// "I 0.90 mA": the test current of `mode` (dmm::test_current): ohm, the
/// measured short-circuit current of range `range` in the variant `on`
/// (none on 60 M: the box is cleared); continuity / diode, the source current (measured, continuity negative). Two significant digits, DIM F14 (small: a
/// property of the function, not a reading).
#[inline(never)] // .cold (link-native.x): drawn on a function / range / variant change only
fn test_current_label(mode: u8, range: i8, on: bool) {
    let Some(i) = crate::dmm::test_current(mode, range, on as u8) else {
        // No value (60 M): clear the box, a range change from 6 M left the
        // 6 M label (device check 2f56ff9).
        let b = OHM_I_RECT;
        lcd::fill_rect(b.x as u16, b.y as u16, b.w as u16, b.h as u16, BG);
        return;
    };
    // Continuity's is negative (its source drives the other way).
    let (v, u) = if i.abs() >= 0.2e-3 {
        (i * 1e3, " mA")
    } else if i.abs() >= 0.2e-6 {
        (i * 1e6, " uA")
    } else {
        (i * 1e9, " nA")
    };
    let mut b = [0u8; 16];
    // The experimental 600 Mohm says so, in the reading's colour.
    let exp = crate::dmm::is_ohm600(mode, range);
    let mut t = crate::scope_ui::Txt::new(if exp { "EXP  I " } else { "I " });
    t.push(crate::dmm::format(&mut b, v, if v.abs() < 1.0 { 2 } else if v.abs() < 10.0 { 1 } else { 0 }));
    t.push(u);
    sf::label(sf::F14, OHM_I_RECT, t.as_str(), if exp { VALUE } else { DIM }, BG, 1, 1);
}

/// ASCII form of a unit for the 12 px stock font (no ohm / micro / degree
/// glyphs): ohm -> "Ohm", micro -> "u", "deg C" -> "C".
fn ascii_unit(u: &str, out: &mut crate::scope_ui::Txt) {
    for c in u.chars() {
        match c {
            '\u{3a9}' => out.push("Ohm"),
            '\u{b5}' => out.push("u"),
            '\u{b0}' => {}
            c => {
                let mut b = [0u8; 4];
                out.push(c.encode_utf8(&mut b));
            }
        }
    }
}

/// Meter strip contents of the stock-style combined view (scope_ui::
/// meter_strip): the reading as on the old strip (value without the extra
/// digit, OL = "0L"), sub readings (AC+DC parts tagged "AC" / "DC", Hz and
/// duty, the 4-20 mA percentage, the temperature sensor name), one state
/// badge by priority A-JACK / LIVE? alternating with HOLD / A-HOLD (`alt`,
/// `slot_alert`) > HOLD / A-HOLD > A-JACK / V-JACK? / LIVE? (r.alert) >
/// HV (>= 30 V) > REL / limit
/// verdict (alternating with `alt`) > range (`range_text`). A CSV
/// recording shows in the top bar (scope_ui::rec_mark), not here.
pub fn strip_data(r: &Reading, hold: u8, lim: u8, rel: bool, hv: u8, alt: bool, label: Option<&'static str>) -> crate::scope_ui::StripData {
    use crate::scope_ui::{StripData, Txt};
    let mut d = StripData::EMPTY;
    d.func = match crate::meter::func_of(r) {
        0 | 2 => "AC V",
        1 | 3 => "DC V",
        4..=6 => "AC A",
        7..=9 => "DC A",
        10 => "OHM",
        11 => "CONT",
        12 => "DIODE",
        13 => "CAP",
        14 => "FREQ",
        16 | 17 => "AC+DC",
        crate::dmm::pw::FUNC => "POWER",
        _ => "TEMP",
    };
    if r.cpl_auto {
        // AUTO coupling: the part it shows, marked Auto (6 characters fit
        // the 62 px badge; V / A is the unit's, as for "AC+DC").
        d.func = if r.is_ac() { "AutoAC" } else { "AutoDC" };
    }
    if crate::dmm::is_ohm600(r.mode, r.range) {
        // The experimental 600 Mohm says so here too (the DMM page: EXP).
        d.func = "OhmEXP";
    }
    let mut buf = [0u8; 16];
    let (v, _, _) = fmt7(&mut buf, &r.main, false);
    d.value = Txt::new(v.trim_ascii());
    d.dim = r.prov;
    let mut ub = [0u8; 16];
    let unit = sf::unit_text(r.main.unit, &mut ub);
    d.unit = Txt::new(unit);
    d.unit_letters = !unit.chars().all(|c| sf::SC_DIGITS.glyph(c as u32).is_some());
    let (aux, lp, bd) = aux_shown(r);
    let temp = r.mode == 15 && label.is_some();
    for (i, a) in aux.iter().enumerate() {
        let mut t = Txt::EMPTY;
        if temp && i == 1 {
            t.push(label.unwrap_or(""));
        } else if let Some(v) = a {
            if r.parts() {
                t.push(["AC ", "DC "][i]);
            } else if bd == Some(i) {
                t.push("Bd ");
            } else if lp && i == 1 {
                // The strip's sub box is 86 px: "4-20mA 50.0%" did not fit
                // (its "4" was clipped next to the value's unit).
                t.push("4-20 ");
            }
            let mut b = [0u8; 16];
            t.push(meter::text(&mut b, v));
            ascii_unit(v.unit, &mut t);
        }
        d.sub[i] = t;
    }
    // "0L": what it means, in the first free sub row (else the second:
    // the reading matters more than its sub values then).
    if r.main.ol {
        d.sub[if d.sub[0].as_str().is_empty() { 0 } else { 1 }] = Txt::new(if meter::open(r) { "OPEN" } else { "Overflow" });
    }
    let verdict = match lim {
        meter::V_PASS => Some(("PASS", 0x0400)),
        meter::V_HI => Some(("HI", 0xf800)),
        meter::V_LO => Some(("LO", 0xf800)),
        _ => None,
    };
    let alert = r.alert;
    // A-JACK / LIVE? with HOLD / A-HOLD: alternate (decision 2026-10-22 #5).
    let hold = if slot_alert(hold, alert, alt) { 0 } else { hold };
    let (b, c) = match hold {
        1 => ("HOLD", 0xfaa0),
        2 => ("A-HOLD", 0x5acb),
        3 => ("A-HOLD", 0xfaa0),
        // Wrong-connection alert (sentinel.rs): above HV, below HOLD.
        _ if alert != crate::sentinel::Alert::None => (alert.strip_badge(), alert.color()),
        _ if hv >= 1 => ("HV", 0xc9a0),
        _ if rel && (verdict.is_none() || alt) => ("REL", 0xb833),
        _ if verdict.is_some() => verdict.unwrap_or(("", 0)),
        _ if temp => ("", 0x5acb),
        _ => (range_text(r), 0x5acb),
    };
    d.badge = b;
    d.badge_col = c;
    d.hv = hv >= 2;
    d
}

/// Decision 2026-10-22 #5: the safety alerts A-JACK and LIVE? alternate
/// with HOLD / A-HOLD in the status slot (and the combined-view strip
/// badge) at the 1 s `alt` cadence, so neither the alert nor the frozen
/// display is hidden; V-JACK? (a silent hint) stays below HOLD.
pub fn alert_alternates(hold: u8, alert: crate::sentinel::Alert) -> bool {
    use crate::sentinel::Alert;
    hold != 0 && matches!(alert, Alert::AJack | Alert::AProt | Alert::Live | Alert::Overload)
}

/// The alternating slot shows the alert in this phase (`alt` true).
pub fn slot_alert(hold: u8, alert: crate::sentinel::Alert, alt: bool) -> bool {
    alt && alert_alternates(hold, alert)
}

/// HOLD / RUN slot of the DMM status bar. Manual HOLD keeps the stock F21
/// "HOLD"; AutoHold is a different face so the two cannot be confused:
/// "A-HOLD" in TEXT22 (66 px, F21 would need 103), accent blue while it waits
/// for a stable value, inverted (dark on orange) while it shows a captured
/// one. Slot 119..190 (after the range label; uptime from 205).
///
/// With no HOLD / AutoHold the slot carries the wrong-connection alert
/// (sentinel.rs) instead of "RUN", in the same TEXT22 face: "A-JACK" white
/// on red (the HI / LO badge red), "V-JACK?" / "LIVE?" in the stock value
/// amber on the bar; letter-spaced 1 px, so the alert box runs to x 201
/// (3 px before the uptime at 205) and the other states clear 190..201.
/// With HOLD / A-HOLD, A-JACK and LIVE? take the slot every other second
/// (`slot_alert`); each phase repaints the whole 119..201 box.
fn hold_label(hold: u8, alert: crate::sentinel::Alert, alt: bool) {
    use crate::sentinel::Alert;
    let r = rect(119, 1, 71, 32);
    if (hold == 0 && alert != Alert::None) || slot_alert(hold, alert, alt) {
        let (fg, bg) = if alert.red() { (0xFFFF, alert.color()) } else { (alert.color(), BAR) };
        lcd::fill_rect(119, 1, 83, 4, BAR);
        lcd::fill_rect(119, 29, 83, 4, BAR);
        sf::label(sf::TEXT22, rect(119, 5, 83, 24), alert.badge(), fg, bg, 8, 1);
        return;
    }
    lcd::fill_rect(190, 1, 12, 32, BAR);
    match hold {
        2 | 3 => {
            let (fg, bg) = if hold == 3 { (BG, VALUE) } else { (0x04FC, BAR) };
            lcd::fill_rect(119, 1, 71, 4, BAR);
            lcd::fill_rect(119, 29, 71, 4, BAR);
            sf::label(sf::TEXT22, rect(119, 5, 71, 24), "A-HOLD", fg, bg, 8, 0);
        }
        _ => sf::label(sf::F21, r, if hold == 1 { "HOLD" } else { "RUN" }, STATUS_FG, BAR, 8, 0),
    }
}

/// FR-06 verdict badge (meter::V_*): a pill between the digits and the
/// mode glyph, green "OK" (pass), red "HI" / "LO" (fail above / below), none
/// when no limit applies. 31 px is all the room there is (>= 3 px to the
/// last digit and to the mode glyph): "PASS" / "FAIL HI" do not fit at a
/// legible size. The digits keep the value colour.
/// "AUTO" (AUTO coupling) in the limit badge's slot (`DmmView::reading`).
#[inline(never)] // .cold (link-native.x): drawn on a change only
fn auto_cpl_label() {
    sf::label(sf::F14, rect(403, 72, 31, 17), "AUTO", VALUE, BG, 8, 1);
}

pub(crate) fn lim_badge(v: u8) {
    let r = rect(403, 72 + main_dy(), 31, 17);
    let (t, bg) = match v {
        crate::meter::V_PASS => ("OK", 0x04A9),
        crate::meter::V_HI => ("HI", 0xF800),
        crate::meter::V_LO => ("LO", 0xF800),
        _ => {
            lcd::fill_rect(r.x as u16, r.y as u16, r.w as u16, r.h as u16, BG);
            return;
        }
    };
    sf::rbox(r, 4, bg, BG);
    sf::glyphs(sf::F14, rect(r.x, r.y + 1, r.w, r.h), t, 0xFFFF, bg, 8, 1);
}

/// MF range page (stock "Voltage / Current / Resistor Settings"): title bar,
/// range buttons, DC / AC buttons. Items: (label, set_range argument).
/// Labels: the stock DM40C set (60000 counts, flash strings 0x08078704..
/// 0x0807889c hold the 4/5/6 sets per model); opendm40 always uses the DM40C
/// limits (dmm.rs COUNTS_MODEL).
pub fn range_items(b: Button) -> &'static [(&'static str, Option<u8>)] {
    match b {
        // 60 mV: the DC / AC mV r0 row (Dmm::set_range item 5; the stock
        // has the row but no item for it).
        Button::Volt => &[
            ("60mV", Some(5)), ("600mV", Some(0)), ("6V", Some(1)), ("60V", Some(2)),
            ("600V", Some(3)), ("1000V", Some(4)), ("AUTO+", Some(255)), ("AUTO", None),
        ],
        Button::Amp => &[
            ("600uA", Some(0)), ("6000uA", Some(1)), ("60mA", Some(2)), ("600mA", Some(3)),
            ("6A", Some(4)), ("10A", Some(5)), ("AUTO+", Some(255)), ("AUTO", None),
        ],
        // 60 ohm: the stock-table r0 row (OP1 x10, mohm resolution; dmm.rs
        // +1 digit notes), also in AUTO unless the 0.28 mA "ON" variant.
        // 600 Mohm (dmm::OHM600): after AUTO, so the AUTO key's range steps
        // (the items before AUTO) never reach it; its own row on the page.
        Button::Ohm => &[
            ("60\u{3a9}", Some(0)), ("600\u{3a9}", Some(1)), ("6K\u{3a9}", Some(2)), ("60K\u{3a9}", Some(3)),
            ("600K\u{3a9}", Some(4)), ("6M\u{3a9}", Some(5)), ("60M\u{3a9}", Some(6)), ("AUTO", None),
            (OHM600_ITEM, Some(7)),
        ],
        // Capacitance (owner 2026-10-30): dmm.rs ranges r1..r7 (one 60 nF
        // range for the stock's 6 / 60 nF rows: dmm::CAP_R_MIN), the mF
        // ranges in uF as the readout (cap_val).
        Button::Cap => &[
            ("60nF", Some(1)), ("600nF", Some(2)), ("6uF", Some(3)), ("60uF", Some(4)),
            ("600uF", Some(5)), ("6000uF", Some(6)), ("60 000uF", Some(7)), ("AUTO", None),
        ],
        _ => &[("AUTO", None)],
    }
}

/// The MF page item (`range_items`, `Dmm::set_range` code) of HY3131
/// `mode` / `range`: V r + 1, mV 5 (60 mV) / 0 (600 mV), A 600 uA .. 10 A
/// = 0..5, ohm r.
pub fn range_code(mode: u8, range: u8) -> u8 {
    match mode {
        2 | 3 if range == 0 => 5,
        2 | 3 => 0,
        0 | 1 => range + 1,
        4..=9 => (2 - (mode - 4) % 3) * 2 + range,
        _ => range,
    }
}

/// The status bar's label of a manual range, as the stock writes it there
/// ("%d" + unit of the range's full scale, FUN_0803d0xx; checked on stock
/// V2.6 in the emulator): the MF page's item text, "6V", "600mV",
/// "6000uA", "6K\u{3a9}" .. None: a function with no range list
/// (continuity, diode, capacitance, frequency, temperature), where the
/// stock always shows "AUTO".
pub fn range_label(mode: u8, range: i8) -> Option<&'static str> {
    let r = range.max(0) as u8;
    match mode {
        10 if range == crate::dmm::OHM600 => Some("600M\u{3a9}"),
        0..=10 | 13 => {
            let code = Some(range_code(mode, r));
            range_items(crate::dmm::button_of(mode)).iter().find(|it| it.1 == code).map(|it| it.0)
        }
        _ => None,
    }
}

/// Status-bar range text (stock): "AUTO" / "AUTO+" in auto, the range
/// ("6V", "600mV", "60K\u{3a9}" ..) in manual. The power meter (two
/// channels, their ranges in its side column) and a range held in a
/// function with no range list (REL in frequency, SCPI RANG:AUTO OFF) keep
/// "MANU": "600KHz" does not fit the slot.
pub fn range_text(r: &Reading) -> &'static str {
    if r.auto {
        if r.auto_plus { "AUTO+" } else { "AUTO" }
    } else if r.power.is_some() {
        "MANU"
    } else {
        range_label(r.mode, r.range).unwrap_or("MANU")
    }
}

/// The status bar's range text in the stock F21, left in the stock's 105 px
/// box (x 4..109) before the HOLD / alert slot (x 119). The stock
/// letter-spaces it -1; ours closes up only the ranges that need it.
#[inline(never)] // .cold (link-native.x): drawn on a change only
fn range_status(s: &str) {
    let sp = if sf::text_width(sf::F21, s, 0) <= 104 { 0 } else if sf::text_width(sf::F21, s, -1) <= 104 { -1 } else { -2 };
    sf::label(sf::F21, rect(BAR_X, 1, 105, 32), s, STATUS_FG, BAR, 1, sp);
}

fn range_rect(i: usize) -> sf::Rect {
    if i >= 8 {
        // The Ohm page's 600 Mohm item: the note row (141..177, free on
        // that page), two columns wide.
        return rect(8, 141, 228, 36);
    }
    rect(8 + (i % 4) as i16 * 117, 50 + (i / 4) as i16 * 46, 112, 40)
}

/// The 600 Mohm item of the Resistor Settings page (dmm::OHM600), its index
/// in `range_items(Button::Ohm)`, and the line in place of the test-current
/// buttons while it is chosen (its source is fixed: `ohm_vars_shown`).
pub const OHM600_ITEM: &str = "600M\u{3a9} experimental";
const OHM600_IDX: usize = 8;
pub const OHM600_NOTE: &str = "100 nA";

/// The Resistor Settings page shows the 0.90 / 0.28 mA buttons: not with
/// 600 Mohm chosen (`sel`: the page's selected item).
pub fn ohm_vars_shown(sel: Option<usize>) -> bool {
    sel != Some(OHM600_IDX)
}

/// The V / A range page's coupling row: DC, AC, AC+DC, AUTO (= Dmm::sub
/// 0..3).
const CPL_NAMES: [&str; 4] = ["DC", "AC", "AC+DC", "AC/DC"];

/// Coupling button `i` (CPL_NAMES): the range grid's columns, row 182.
fn cpl_rect(i: usize) -> sf::Rect {
    rect(8 + i as i16 * 117, 182, 112, 38)
}

/// `note`: a line under the range buttons (UF-21: the V page says AUTO+ is
/// the recommended auto range, or warns in orange while the mV-first AUTO
/// is in use), true = warning.
#[inline(never)] // .cold (link-native.x): the MF page, drawn on a tap
pub fn range_page(b: Button, sel: Option<usize>, sub: u8, note: Option<(&str, bool)>) {
    let title = match b {
        Button::Volt => "Voltage Settings",
        Button::Amp => "Current Settings",
        Button::Ohm => "Resistor Settings",
        _ => "Capacitance Settings",
    };
    lcd::fill_rect(0, 0, lcd::W, 40, 0x2966);
    lcd::fill_rect(0, 40, lcd::W, lcd::H - 40, BG);
    sf::glyphs(sf::TEXT22, rect(0, 0, 480, 40), title, STATUS_FG, 0x2966, 8, 0);
    for (i, (name, _)) in range_items(b).iter().enumerate() {
        let c = if Some(i) == sel { SEL } else { 0x2966 };
        sf::rbox(range_rect(i), 6, c, BG);
        sf::glyphs(sf::TEXT22, range_rect(i), name, STATUS_FG, c, 8, 0);
    }
    if let Some((t, warn)) = note {
        range_note(t, warn);
    }
    if b == Button::Ohm && !ohm_vars_shown(sel) {
        // 600 Mohm runs its own source: the variant buttons would do nothing,
        // so its test current under its item instead.
        sf::glyphs(sf::TEXT22, rect(8, 182, 228, 38), OHM600_NOTE, DIM, BG, 8, 1);
    } else if b == Button::Ohm {
        // The stock's second ohm mode (m+8, toggled by a second tap on the
        // ohm button; "ON" tag on the button): the p8set rows, a lower test
        // current for in-circuit measurements. `sub` = Dmm::ohm_var here.
        for (i, name) in OHM_VARS.iter().enumerate() {
            let c = if i as u8 == sub { SEL } else { 0x2966 };
            let r = rect([8, 244][i], 182, 228, 38);
            sf::rbox(r, 6, c, BG);
            sf::glyphs(sf::TEXT22, r, name, STATUS_FG, c, 8, 0);
        }
    }
    if matches!(b, Button::Volt | Button::Amp) {
        // Stock: DC (8,182), AC (162,182), AC+DC (319,182), 150x38 each;
        // opendm40 adds the AUTO coupling ("AC/DC": AUTO is the range button
        // above), so four on the range grid's columns (`cpl_rect`).
        for (i, name) in CPL_NAMES.iter().enumerate() {
            let c = if i as u8 == sub { SEL } else { 0x2966 };
            let r = cpl_rect(i);
            sf::rbox(r, 6, c, BG);
            sf::glyphs(sf::TEXT22, r, name, STATUS_FG, c, 8, 0);
        }
    }
    ok_and_limits_buttons();
    if b == Button::Amp && note.is_none() {
        // Fuse check (doc 4.3): the A page has no note line, so the button
        // sits there, the middle column (the bottom middle slot is the wave
        // view's "Wave" in ad2-isr builds).
        let r = FUSE_BTN;
        sf::rbox(r, 6, 0x2966, BG);
        sf::glyphs(sf::TEXT22, r, "Fuse check", STATUS_FG, 0x2966, 8, 0);
    }
}

/// "Inrush" on the Current Settings page (the inrush capture, app_inrush.rs;
/// builds with the AD2 stream ISR only): the note row, right column (the
/// AC+DC column of the stock grid), beside Fuse check.
pub const INRUSH_BTN: sf::Rect = rect(319, 141, 150, 36);

pub fn inrush_button() {
    sf::rbox(INRUSH_BTN, 6, 0x2966, BG);
    sf::glyphs(sf::TEXT22, INRUSH_BTN, "Inrush", STATUS_FG, 0x2966, 8, 0);
}

/// The Inrush button was hit (the gaps to its neighbours split as theirs).
pub fn inrush_hit(x: u16, y: u16) -> bool {
    let (x, y) = (x as i16, y as i16);
    (139..180).contains(&y) && (316..472).contains(&x)
}

/// The fuse-check dialog (dialog::fuse_check) of `stage` (0 instructions, 1 measuring, 2 the
/// verdict `res`); `buf` holds the formatted resistance.
pub fn fuse_screen(stage: u8, res: crate::sentinel::FuseRes, buf: &mut [u8; 32]) {
    use crate::sentinel::FuseRes as F;
    let ohm = match res {
        F::Ok(r) | F::Odd(r) => {
            let mut t = [0u8; 16];
            let v = crate::dmm::format(&mut t, r, if r < 100.0 { 2 } else { 1 });
            let mut n = 0;
            for part in [v, " ohm, expect ~48"] {
                let k = part.len().min(buf.len() - n);
                buf[n..n + k].copy_from_slice(&part.as_bytes()[..k]);
                n += k;
            }
            core::str::from_utf8(&buf[..n]).unwrap_or("")
        }
        _ => "",
    };
    match (stage, res) {
        (0, _) => crate::dialog::fuse_check("Fuse check", crate::dialog::TITLE_BG, "Lead from V/ohm jack to A", "nothing else connected", Some("Cancel"), Some("Start")),
        (1, _) | (_, F::Busy | F::Idle) => crate::dialog::fuse_check("Fuse check", crate::dialog::TITLE_BG, "Measuring ...", "expect ~48 ohm", Some("Cancel"), None),
        (_, F::Ok(_)) => crate::dialog::fuse_check("FUSE OK", crate::dialog::FUSE_OK_BG, ohm, "A input path intact", Some("Again"), Some("OK")),
        (_, F::Open) => crate::dialog::fuse_check("FUSE OPEN", crate::dialog::FUSE_BAD_BG, "OL: no path from V to A", "Replace: 10 A ceramic fuse", Some("Again"), Some("OK")),
        (_, F::Odd(_)) => crate::dialog::fuse_check("Check leads", crate::dialog::FUSE_ODD_BG, ohm, "One lead V/ohm to A only", Some("Again"), Some("OK")),
        (_, F::NoReading) => crate::dialog::fuse_check("Check leads", crate::dialog::FUSE_ODD_BG, "No ohm reading", "One lead V/ohm to A only", Some("Again"), Some("OK")),
    }
}

/// Range page note line (`range_page`): rows 142..177, under the second
/// button row (ends 136), above the DC / AC buttons (182); spacing 1 (TEXT22
/// glyphs touch at spacing 0).
pub fn range_note(t: &str, warn: bool) {
    sf::glyphs(sf::TEXT22, rect(8, 142, 464, 36), t, if warn { VALUE } else { DIM }, BG, 8, 1);
}

/// The bottom row of the MF range / temperature pages (owner 2026-10-30:
/// Record one tap from MF, not under Hold & Limits): four buttons on the
/// range grid's columns, `bottom_rect(0..3)` = Limits (View::Limits),
/// Record (View::Record), Wave (V / A in ad2-isr builds, else empty), OK.
/// "Limits", as on Power Settings: "Hold/Limits" (120 px) is wider than a
/// column (112).
pub fn bottom_rect(col: usize) -> sf::Rect {
    rect(8 + col as i16 * 117, 240, 112, 38)
}

/// Bottom-row column under x (the 5 px gaps split between the buttons), or
/// None off the row (y 240..278).
fn bottom_col(x: u16, y: u16) -> Option<usize> {
    let (x, y) = (x as i16, y as i16);
    if !(240..278).contains(&y) {
        return None;
    }
    Some(((x.max(8) - 8 + 2) / 117).min(3) as usize)
}

/// Limits, Record and OK of the bottom row (`bottom_rect`); region B, as
/// `record_button`.
#[inline(never)]
fn ok_and_limits_buttons() {
    for (c, t) in [(0, "Limits"), (3, "OK")] {
        let r = bottom_rect(c);
        sf::rbox(r, 6, 0x2966, BG);
        sf::glyphs(sf::TEXT22, r, t, STATUS_FG, 0x2966, 8, 0);
    }
    record_button(bottom_rect(1), false);
}

/// A CSV recording or a fast-log session runs (`record_button` red, as the
/// Record page's Stop). Set by the app with the page draw (`set_rec_on`).
static REC_ON: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

pub fn set_rec_on(on: bool) {
    REC_ON.store(on, core::sync::atomic::Ordering::Relaxed);
}

/// "Record" (opens View::Record) on the range, temperature and Power
/// Settings pages: red while recording (`set_rec_on`), `pressed` the
/// pressed colour. Region B (not inlined into the .cold page draws).
#[inline(never)]
pub fn record_button(r: sf::Rect, pressed: bool) {
    let c = if pressed {
        K_SEL
    } else if REC_ON.load(core::sync::atomic::Ordering::Relaxed) {
        REC_CSV
    } else {
        0x2966
    };
    sf::rbox(r, 6, c, BG);
    sf::glyphs(sf::TEXT22, r, "Record", STATUS_FG, c, 8, 0);
}

/// "Wave" on the V / A range pages (the DMM wave view, app_wave.rs; builds
/// with the AD2 stream ISR only): the bottom row's third column, between
/// Record and OK.
pub const WAVE_BTN: sf::Rect = rect(242, 240, 112, 38);

/// "Fuse check" on the Current Settings page (`range_page`): rows 141..177
/// between the range buttons (to 136) and DC / AC / AC+DC (from 182).
pub const FUSE_BTN: sf::Rect = rect(162, 141, 150, 36);

pub fn wave_button() {
    sf::rbox(WAVE_BTN, 6, 0x2966, BG);
    sf::glyphs(sf::TEXT22, WAVE_BTN, "Wave", STATUS_FG, 0x2966, 8, 0);
}

/// The Wave button was hit (the gaps to its neighbours split as theirs).
pub fn wave_hit(x: u16, y: u16) -> bool {
    bottom_col(x, y) == Some(2)
}

/// Ohm variant buttons of the Resistor Settings page (Dmm::ohm_var 0 / 1).
/// Labelled with the variant's short-circuit test current on the 600 ohm
/// range (dmm::OHM_ISC r1, measured); "ON" = the button's stock tag.
pub const OHM_VARS: [&str; 2] = ["0.90 mA", "0.28 mA ON"];

/// `range_press` targets besides the range items (0..): RP_ERR + the Err
/// code of `range_hit`, the Wave and Inrush buttons.
pub const RP_ERR: u8 = 100;
pub const RP_WAVE: u8 = 120;
pub const RP_INRUSH: u8 = 121;

/// Pressed feedback on the MF range page (ui-review #11): button `t` (a
/// range item index, RP_*) drawn pressed (`on`: the list rows' pressed
/// colour) or as `range_page` draws it (`sel` / `sub` as there).
#[inline(never)] // .cold (link-native.x): per touch on the MF page
pub fn range_press(b: Button, t: u8, sel: Option<usize>, sub: u8, on: bool) {
    let items = range_items(b);
    let (r, name, selected) = match t {
        RP_WAVE => (WAVE_BTN, "Wave", false),
        RP_INRUSH => (INRUSH_BTN, "Inrush", false),
        i if (i as usize) < items.len() => (range_rect(i as usize), items[i as usize].0, sel == Some(i as usize)),
        RP_ERR.. => match (t - RP_ERR, b) {
            (k @ 0..=1, Button::Ohm) => (rect([8, 244][k as usize], 182, 228, 38), OHM_VARS[k as usize], k == sub),
            (k @ (0 | 1 | 3 | 6), Button::Volt | Button::Amp) => {
                let i = match k {
                    3 => 2,
                    6 => 3,
                    k => k,
                };
                (cpl_rect(i as usize), CPL_NAMES[i as usize], i == sub)
            }
            (2, _) => (bottom_rect(3), "OK", false),
            (4, _) => (bottom_rect(0), "Limits", false),
            (7, _) => return record_button(bottom_rect(1), on),
            (5, Button::Amp) => (FUSE_BTN, "Fuse check", false),
            _ => return,
        },
        _ => return,
    };
    let c = if on { K_SEL } else if selected { SEL } else { 0x2966 };
    sf::rbox(r, 6, c, BG);
    sf::glyphs(sf::TEXT22, r, name, STATUS_FG, c, 8, 0);
}

/// Range page touch: Ok(item index) / Err(0 DC, 1 AC, 3 AC+DC, 2 OK, 4
/// Limits, 7 Record, 5 Fuse check (A only)); ohm: Err(0 / 1) = the OHM_VARS
/// buttons, while `ohm_vars_shown(sel)`.
pub fn range_hit(b: Button, sel: Option<usize>, x: u16, y: u16) -> Option<Result<usize, u8>> {
    let (x, y) = (x as i16, y as i16);
    for i in 0..range_items(b).len() {
        let r = range_rect(i);
        // 2 px (x) / 3 px (y) of slop: the 5 / 6 px gaps split between buttons.
        if x >= r.x - 2 && x < r.x + r.w + 2 && y >= r.y - 3 && y < r.y + r.h + 3 {
            return Some(Ok(i));
        }
    }
    if (179..223).contains(&y) && b == Button::Ohm && ohm_vars_shown(sel) {
        return Some(Err(if x < 240 { 0 } else { 1 }));
    }
    if (179..223).contains(&y) && matches!(b, Button::Volt | Button::Amp) {
        // Coupling row (cpl_rect): DC / AC / AC+DC / AUTO -> Err 0 / 1 / 3 /
        // 6 (Err 2 is OK), the 5 px gaps split between the buttons.
        let i = ((x.max(8) - 8 + 2) / 117).min(3);
        return Some(Err([0, 1, 3, 6][i as usize]));
    }
    match bottom_col(x as u16, y as u16) {
        Some(0) => return Some(Err(4)),
        Some(1) => return Some(Err(7)),
        Some(3) => return Some(Err(2)),
        _ => {}
    }
    if (139..180).contains(&y) && (160..315).contains(&x) && b == Button::Amp {
        return Some(Err(5));
    }
    None
}

/// Temperature sensor names on screen (dmm.rs T_NAMES order).
pub const T_LABELS: [&str; 7] = ["TC K", "PT100", "PT1000", "NTC10k B3950", "NTC10k B3435", "NTC100k B3950", "NTC user"];
/// Sensor buttons of the temperature page (dmm.rs T_NAMES order).
const T_KEYS: [&str; 7] = ["TC K", "PT100", "PT1000", "10k 3950", "10k 3435", "100k 3950", "NTC user"];

/// Temperature page (MF on the temperature function), redesigned 2026-10-21
/// (owner: the "Offset 0" key was unclear, the offset slider unintuitive,
/// "NTC user" unexplained, the buttons "fuzzy" = hit boxes exactly the drawn
/// boxes, so a touch in the 5..6 px gaps did nothing):
///   rows 50 / 96   sensors (range-page grid), slot 7 = the global deg C /
///                  deg F unit (the Settings 2/2 row; a shortcut here)
///   row 144..180   "Offset"  [ - ]  value  [ + ]  (settings-stepper look;
///                  0.1 C steps; a tap on the value opens the signed keypad)
///   row 186..222   NTC user only: R25 and Beta fields (tap = keypad)
///   row 240..278   Limits, Record, OK (`bottom_rect`);  282..316 the
///                  info line.
/// Every hit box reaches 3 px into the gaps around its button.
#[derive(Clone, Copy)]
pub struct TempPage<'a> {
    pub sensor: u8,
    /// Offset of the sensor, C.
    pub offs: f32,
    pub unit_f: bool,
    /// NTC user parameters (ohm, K).
    pub r25: f32,
    pub beta: f32,
    pub info: &'a str,
}

/// Temperature page hit codes besides the sensors 0..6.
pub const TK_UNIT: u8 = 7;
pub const TK_MINUS: u8 = 8;
pub const TK_PLUS: u8 = 9;
pub const TK_OFFS: u8 = 10;
pub const TK_R25: u8 = 11;
pub const TK_BETA: u8 = 12;
pub const TK_RECORD: u8 = 253;
pub const TK_LIMITS: u8 = 254;
pub const TK_OK: u8 = 255;

const T_ROW_Y: i16 = 144;
const T_NTC_Y: i16 = 186;
const T_ROW_H: i16 = 36;

/// Button rectangle of a temperature-page code (None: not on this page).
fn temp_rect(sensor: u8, code: u8) -> Option<sf::Rect> {
    Some(match code {
        0..=7 => range_rect(code as usize),
        TK_MINUS => rect(132, T_ROW_Y, 72, T_ROW_H),
        TK_OFFS => rect(210, T_ROW_Y, 186, T_ROW_H),
        TK_PLUS => rect(402, T_ROW_Y, 70, T_ROW_H),
        TK_R25 if sensor == 6 => rect(8, T_NTC_Y, 228, T_ROW_H),
        TK_BETA if sensor == 6 => rect(244, T_NTC_Y, 228, T_ROW_H),
        TK_LIMITS => bottom_rect(0),
        TK_RECORD => bottom_rect(1),
        TK_OK => bottom_rect(3),
        _ => return None,
    })
}

const T_CODES: [u8; 16] = [0, 1, 2, 3, 4, 5, 6, TK_UNIT, TK_MINUS, TK_OFFS, TK_PLUS, TK_R25, TK_BETA, TK_LIMITS, TK_RECORD, TK_OK];

/// Offset text "+7.3 C" (TEXT22 has no degree sign).
fn temp_offs_text<'a>(b: &'a mut [u8; 24], offs: f32) -> &'a str {
    let mut t = [0u8; 16];
    let v = crate::dmm::format(&mut t, offs, 1);
    join(b, &[if offs > 0.0 { "+" } else { "" }, v, " C"])
}

/// One temperature-page button, normal or pressed (stock pressed colour,
/// ui-review #11: feedback from touch-down).
pub fn temp_key(p: &TempPage, code: u8, pressed: bool) {
    let Some(r) = temp_rect(p.sensor, code) else { return };
    if code == TK_RECORD {
        return record_button(r, pressed);
    }
    let sel = code < 7 && code == p.sensor;
    let c = if pressed { K_SEL } else if sel { SEL } else { 0x2966 };
    let mut b = [0u8; 24];
    let mut t = [0u8; 16];
    let txt: &str = match code {
        0..=6 => T_KEYS[code as usize],
        TK_UNIT => if p.unit_f { "Unit: F" } else { "Unit: C" },
        TK_MINUS => "-",
        TK_PLUS => "+",
        TK_OFFS => temp_offs_text(&mut b, p.offs),
        TK_R25 => {
            let k = p.r25 / 1000.0;
            let v = crate::dmm::format(&mut t, k, if k < 10.0 { 2 } else if k < 100.0 { 1 } else { 0 });
            join(&mut b, &["R25  ", v, " kohm"])
        }
        TK_BETA => {
            let v = crate::dmm::format(&mut t, p.beta, 0);
            join(&mut b, &["Beta  ", v, " K"])
        }
        TK_LIMITS => "Limits",
        _ => "OK",
    };
    if code == TK_OFFS {
        // The value field: darker than the keys, the value in orange.
        let c = if pressed { K_SEL } else { BAR };
        sf::rbox(r, 6, c, BG);
        sf::glyphs(sf::TEXT22, r, txt, VALUE, c, 8, 0);
    } else {
        sf::rbox(r, 6, c, BG);
        sf::glyphs(sf::TEXT22, r, txt, STATUS_FG, c, 8, 0);
    }
}

pub fn temp_page(p: &TempPage) {
    lcd::fill_rect(0, 0, lcd::W, 40, 0x2966);
    lcd::fill_rect(0, 40, lcd::W, lcd::H - 40, BG);
    sf::glyphs(sf::TEXT22, rect(0, 0, 480, 40), "Temperature Settings", STATUS_FG, 0x2966, 8, 0);
    sf::glyphs(sf::TEXT22, rect(8, T_ROW_Y, 118, T_ROW_H), "Offset", STATUS_FG, BG, 1, 0);
    for code in T_CODES {
        temp_key(p, code, false);
    }
    if p.sensor != 6 {
        // The NTC row's place: how to enter the offset exactly.
        let hint = if p.sensor == 0 { "Tap the value to type it (TC: on the CJ)" } else { "Tap the value to type it" };
        sf::glyphs(sf::TEXT22, rect(8, T_NTC_Y, 464, T_ROW_H), hint, DIM, BG, 8, 0);
    }
    sf::glyphs(sf::TEXT22, T_INFO, p.info, STATUS_FG, BG, 1, 0);
}

/// The temperature page's info line (cold junction / sensor current).
const T_INFO: sf::Rect = rect(8, 282, 464, 34);

/// The info line again, while the page is shown (App::temp_info_tick: the
/// cold junction changes with each thermocouple reading and the offset).
pub fn temp_info(info: &str) {
    lcd::fill_rect(T_INFO.x as u16, T_INFO.y as u16, T_INFO.w as u16, T_INFO.h as u16, BG);
    sf::glyphs(sf::TEXT22, T_INFO, info, STATUS_FG, BG, 1, 0);
}

/// Temperature page touch: a T_CODES code (3 px of slop into the gaps:
/// the gaps are >= 6 px, so the halves split between neighbours).
pub fn temp_hit(sensor: u8, x: u16, y: u16) -> Option<u8> {
    let (x, y) = (x as i16, y as i16);
    for s in [0i16, 3] {
        for code in T_CODES {
            if let Some(r) = temp_rect(sensor, code) {
                if x >= r.x - s && x < r.x + r.w + s && y >= r.y - s && y < r.y + r.h + s {
                    return Some(code);
                }
            }
        }
    }
    None
}

/// History boxes (stock (5+95i,153,88,19)): values with units, newest orange.
pub fn history(items: &[(f32, u8, &'static str); 5], n: usize, newest: usize) {
    for i in 0..5 {
        let r = rect(5 + 95 * i as i16, 153, 88, 19);
        sf::rbox(r, 3, BAR, BG);
        if i < n {
            let mut b = [0u8; 32];
            let (v, d, u) = items[i];
            let t = num_unit(&mut b, v, d, u);
            sf::glyphs(sf::HIST, r, t, if i == newest { 0xFC40 } else { STATUS_FG }, BAR, 8, 0);
        }
    }
}

/// High-voltage lightning (stock (0,80,40,64), >= 100 V in V modes).
pub fn hv_warning(on: bool) {
    sf::label(sf::LIGHTNING, rect(0, 80, 40, 64), if on { " " } else { "" }, 0xC9A0, BG, 8, 0);
}

/// What the status block at the right of every instrument bar shows.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Status {
    pub uptime_s: u32,
    pub bt: bool,
    /// A USB host configured the meter (USBTMC + USB disk usable): USB symbol.
    pub usb: bool,
    /// Battery level as shown (`Power::shown_bars`: blinks while charging).
    /// `charging`: on a charger (charging or charged), the battery green.
    pub bars: u8,
    pub charging: bool,
    pub locked: bool,
    /// MIN/MAX recording session on (the DMM statistics, FR-02).
    pub rec: bool,
    /// CSV recording to the disk running (csvlog.rs).
    pub csv: bool,
    /// The REC dot's blink phase (scope_ui::rec_blink): true = hidden.
    pub rec_dim: bool,
    /// Index of the shown view in View::CYCLE (page dots), 255 = none.
    pub page: u8,
}

fn hms(t: &mut [u8; 8], s: u32) -> &str {
    let (h, m, sec) = (s / 3600 % 100, s / 60 % 60, s % 60);
    for (i, v) in [h, m, sec].iter().enumerate() {
        t[i * 3] = b'0' + (v / 10) as u8;
        t[i * 3 + 1] = b'0' + (v % 10) as u8;
        if i < 2 {
            t[i * 3 + 2] = b':';
        }
    }
    core::str::from_utf8(&t[..]).unwrap_or("")
}

fn battery_glyph(b: &mut [u8; 4], bars: u8) -> &str {
    char::from_u32(0x22 + bars.min(5) as u32).unwrap_or('"').encode_utf8(b)
}

/// The USB symbol (SAVE_BT 0x22, tools/mkusbglyph.py) between the BT
/// symbol and the battery while a USB host has the meter configured (not
/// for a charger: no enumeration).
fn usb_glyph(st: &Status) -> &'static str {
    if st.usb { "\u{22}" } else { "" }
}

/// CSV recording mark colour (the REC dot, scope_ui::rec_mark): red. The
/// MIN/MAX session alone shows REC in the value orange (as "REC hh:mm:ss"
/// in the DMM chart column).
pub const REC_CSV: Color = 0xF800;

/// The REC mark of the DMM / generator status bars (CSV recording): a red
/// dot blinking with scope_ui::rec_blink and "REC" in the bar's text
/// colour, as the scope top bar's (scope_ui::rec_mark), between the lock
/// (box 327..350; 8 px left of the stock place to make room for the dot)
/// and the BT icon (390..), "REC" where it was; `y` = the row of the label box
/// (caps 2 rows below, the dot centred on them).
const REC_DOT: sf::Rect = rect(354, 3, 7, 7);
const REC_TXT_X: i16 = 364;

fn rec_dot_at(y: i16) -> sf::Rect {
    rect(REC_DOT.x, y + REC_DOT.y, REC_DOT.w, REC_DOT.h)
}

fn status_rec(st: &Status, bg: Color, y: i16) {
    crate::scope_ui::rec_dot(rec_dot_at(y), bg, st.csv && !st.rec_dim);
    sf::label(sf::F14, rect(REC_TXT_X, y, 20, 15), if st.csv { "REC" } else { "" }, STATUS_FG, bg, 8, 1);
}

/// The REC dot's blink step (App::tick on a `rec_blink` change): only the
/// dot's box of the DMM page's (`gen` false) or the generator bar.
pub fn status_rec_blink(st: &Status, gen: bool) {
    let (bg, y) = if gen { (G_BAR, 11) } else { (BAR, 10) };
    crate::scope_ui::rec_dot(rec_dot_at(y), bg, st.csv && !st.rec_dim);
}

/// Right part of the DMM page status bar (34 px, stock positions): uptime,
/// REC (CSV recording; the MIN/MAX session has its own line in the chart
/// column), lock, BT link, USB host, battery.
pub fn status_right(st: &Status) {
    let mut t = [0u8; 8];
    let mut b = [0u8; 4];
    let bc = if st.charging { 0x04A9 } else { STATUS_FG };
    sf::label(sf::F21, rect(205, 5, 112, 24), hms(&mut t, st.uptime_s), STATUS_FG, BAR, 1, -1);
    status_rec(st, BAR, 10);
    sf::label(sf::LOCK, rect(327, 7, 24, 20), if st.locked { "\u{20}" } else { "\u{21}" }, STATUS_FG, BAR, 8, 0);
    sf::label(sf::SAVE_BT, rect(390, 0, 16, 32), if st.bt { "\u{21}" } else { "" }, STATUS_FG, BAR, 1, 0);
    sf::label(sf::SAVE_BT, rect(412, 0, 16, 32), usb_glyph(st), STATUS_FG, BAR, 1, 0);
    sf::label(sf::BATTERY, rect(435, 9, 40, 16), battery_glyph(&mut b, st.bars), bc, BAR, 8, 0);
}

/// Strip chart (stock frame (80,176,321,97)) with labels, rate button and
/// MAX / REL / MIN. `pts` are display values, oldest first.
pub const CHART_X: u16 = 80;
/// Trend chart timebases: one point per `rate` x 10 ms, 16 points (px) a
/// division, so `rate` x 160 ms/div. The stock has 1 / 2 / 4 (160 / 320 /
/// 640 ms/div, default 320); 8 and 16 (1.28 / 2.56 s/div) are opendm40's.
/// The point ring (App::chart, CHART_W points) is the same at every rate.
pub const TREND_RATES: [u8; 5] = [1, 2, 4, 8, 16];
pub const TREND_DEFAULT: u8 = 2;

/// The timebase after `rate` (a tap on the chart's left column).
pub fn trend_next(rate: u8) -> u8 {
    let i = TREND_RATES.iter().position(|&r| r == rate).map_or(0, |i| (i + 1) % TREND_RATES.len());
    TREND_RATES[i]
}

/// Settings code of a timebase (0 = the default, older records), and back.
pub fn trend_code(rate: u8) -> u32 {
    if rate == TREND_DEFAULT { 0 } else { TREND_RATES.iter().position(|&r| r == rate).map_or(0, |i| i as u32 + 1) }
}

pub fn trend_of_code(c: u32) -> u8 {
    match c {
        1..=5 => TREND_RATES[c as usize - 1],
        _ => TREND_DEFAULT,
    }
}
pub const CHART_W: u16 = 321;
const CHART_Y: u16 = 176;
const CHART_H: u16 = 97;
/// The power meter's parts layout (`pw_parts`): the chart from y 192, 81
/// rows (the lower row moved PARTS_ROW_DY down).
const PARTS_CHART_Y: u16 = 192;
const PARTS_CHART_H: u16 = 81;

/// Chart geometry: (frame top, rows, the left axis labels' line tops, the
/// timebase chip's and the rate chip's tops).
struct ChartGeom {
    y: u16,
    h: u16,
    axis: [i16; 3],
    tb: i16,
    rate: i16,
}

fn chart_geom() -> ChartGeom {
    if pw_parts() {
        ChartGeom { y: PARTS_CHART_Y, h: PARTS_CHART_H, axis: [192, 227, 262], tb: 208, rate: 243 }
    } else {
        ChartGeom { y: CHART_Y, h: CHART_H, axis: AXIS_Y, tb: 192, rate: RATE_CHIP_Y }
    }
}

/// The chart's left column (the timebase tap; App): the frame's rows.
pub fn chart_rows() -> core::ops::Range<u16> {
    let g = chart_geom();
    g.y..280
}

/// "value unit" of the chart axis labels and the history boxes; a value
/// that rounds to zero at `dec` is +0 (no "-0.000mW", pw::no_neg_zero).
fn num_unit<'a>(b: &'a mut [u8; 32], v: f32, dec: u8, unit: &str) -> &'a str {
    let mut t = [0u8; 16];
    let s = crate::dmm::format(&mut t, crate::dmm::pw::no_neg_zero(v, dec), dec);
    let mut u = [0u8; 16];
    let u = sf::unit_text(unit, &mut u);
    let n = s.len().min(16);
    b[..n].copy_from_slice(&s.as_bytes()[..n]);
    let k = u.len().min(32 - n);
    b[n..n + k].copy_from_slice(&u.as_bytes()[..k]);
    core::str::from_utf8(&b[..n + k]).unwrap_or("")
}

/// What the chart last drew besides the trace, to skip unchanged labels.
/// The side rows are keyed by their value rounded to the shown digits, so a
/// running AVG repaints only when a shown digit changes.
pub struct ChartCache {
    scale: Option<(u32, u32, u8, &'static str)>,
    side: Option<(u8, &'static str)>,
    rows: [i64; 5],
    time: Option<(u32, bool)>,
    rate: u8,
    rchip: Option<(u8, u32, bool)>,
    /// Trace rows (first, last) each column shows, NO_SPAN = none; only
    /// valid with `drawn` (else every column is drawn in full). A new point
    /// rewrites just the columns whose span changed, and in them only the
    /// rows of the old and the new span (one window per column).
    cols: [(u8, u8); CHART_W as usize],
    drawn: bool,
}

/// Column without trace (first > last).
const NO_SPAN: (u8, u8) = (u8::MAX, 0);

/// DMM reading-rate chip under the chart's timebase chip (owner request):
/// the FR-05 rate setting (`rate` = dmm::Rate as u8) and the measured
/// readings/s (`period_ms` = Dmm::reading_period_ms, 0 = not measured yet).
/// `applies` = Dmm::rate_applies: otherwise grey "n/a" + the measured rate.
/// Tap (`rate_chip_hit`) = next rate.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct RateChip {
    pub rate: u8,
    pub period_ms: u32,
    pub applies: bool,
}

impl RateChip {
    /// What the chip shows (rate, rounded readings/s, applies): the cache
    /// key, so the EMA period's millisecond jitter does not repaint it.
    fn shown(&self) -> (u8, u32, bool) {
        let p = self.period_ms;
        let r10 = if p == 0 { 0 } else { (10_000 + p / 2) / p };
        (self.rate, if r10 >= 100 { (r10 + 5) / 10 * 10 } else { r10 }, self.applies)
    }
}

/// Rate chip rows (between the middle and the bottom axis labels, x 4..76
/// as the timebase chip).
const RATE_CHIP_Y: i16 = 236;
const RATE_CHIP_H: i16 = 17;

/// A tap on the DMM page's rate chip (3 px of slop above and below).
pub fn rate_chip_hit(x: u16, y: u16) -> bool {
    let y = y as i16;
    let r = chart_geom().rate;
    x < CHART_X && y >= r - 3 && y < r + RATE_CHIP_H + 3
}

/// A tap on the DMM page's main value (the F78 cells 40..400 x 68..148,
/// the +1 digit cell, 4 px around): resolution / rate step (App). The
/// power meter's parts layout: down to its P sub-line (..168).
pub fn value_hit(x: u16, y: u16) -> bool {
    let y1 = if pw_parts() { 168 } else { 151 };
    (36..402).contains(&x) && (64..y1).contains(&y)
}

/// Readings per second from a reading period: "9.4/s", "37/s", "--/s"
/// (period 0 = not measured yet).
pub fn rps_text(buf: &mut [u8; 16], period_ms: u32) -> &str {
    if period_ms == 0 {
        return "--/s";
    }
    let r10 = (10_000 + period_ms / 2) / period_ms;
    let mut t = [0u8; 16];
    let s = if r10 >= 100 {
        crate::dmm::format(&mut t, ((r10 + 5) / 10) as f32, 0)
    } else {
        crate::dmm::format(&mut t, r10 as f32 / 10.0, 1)
    };
    let n = s.len().min(13);
    buf[..n].copy_from_slice(&s.as_bytes()[..n]);
    buf[n..n + 2].copy_from_slice(b"/s");
    core::str::from_utf8(&buf[..n + 2]).unwrap_or("")
}

/// The toast of a tap on the DMM value (App::resolution_cycle): the rate
/// `rate` (dmm::Rate as u8) with its readings/s as the rate chip writes them
/// (`rps_text` of the conversion period `period_ms`, dmm::rate_period), and
/// what the code window of `n` conversions gives (dmm::rate_window):
/// "FAST 10/s, 60000 counts"; with the extra digit (`ext`) the time until
/// it shows (dmm::AVG_FULL conversions in the window), "MED 5.0/s, +1 digit
/// in 1.6 s"; else the window, "SLOW 2.5/s, 6.4 s average".
#[inline(never)] // .cold (link-native.x): a tap on the DMM value
pub fn rate_toast(buf: &mut [u8; 40], rate: u8, period_ms: u32, n: usize, ext: bool) -> &str {
    let mut k = 0;
    let mut put = |t: &str| {
        let m = t.len().min(40 - k);
        buf[k..k + m].copy_from_slice(&t.as_bytes()[..m]);
        k += m;
    };
    put(crate::dmm::RATE_NAMES.get(rate as usize).copied().unwrap_or(""));
    put(" ");
    let mut t = [0u8; 16];
    put(rps_text(&mut t, period_ms));
    if n > 1 {
        let w = if ext { n.min(crate::dmm::AVG_FULL as usize) } else { n };
        let s = crate::dmm::format(&mut t, (w as u32 * period_ms) as f32 / 1000.0, 1);
        if ext {
            put(", +1 digit in ");
            put(s);
            put(" s");
        } else {
            put(", ");
            put(s);
            put(" s average");
        }
    } else {
        put(", 60000 counts");
    }
    core::str::from_utf8(&buf[..k]).unwrap_or("")
}

/// Right column of the DMM chart (FR-02): MAX / AVG above the REL box, MIN,
/// SD (standard deviation) and the time since the statistics started below
/// it (display units).
/// `rec`: a recording session is on (no idle shutdown): "REC" in orange.
#[derive(Clone, Copy)]
pub struct Side {
    pub max: Option<f32>,
    pub avg: Option<f32>,
    pub min: Option<f32>,
    pub sd: Option<f32>,
    pub rel: Option<f32>,
    pub secs: Option<u32>,
    pub rec: bool,
}

/// Right column: text x range and the line tops of MAX, AVG, MIN, time
/// (F14 caps 2 px below: rows 178, 190, 247, 261); REL box (405,205,71,38).
/// The power meter's side column (ui_power.rs) keeps these rows.
const SIDE_X: i16 = 405;
const SIDE_R: i16 = 476;
const SIDE_Y: [i16; 4] = [176, 188, 245, 259];
/// The DMM column's line tops: MAX, AVG, MIN, SD, time (caps 2 px below,
/// 9 rows of ink, 3 px apart); the REL box 2 px shorter (405,205,71,36) to
/// make room for SD, the time row 3 px above the function buttons' region.
const STAT_Y: [i16; 5] = [176, 188, 242, 254, 266];
/// Line tops of the chart's left axis labels (caps on rows 178, 221, 264).
const AXIS_Y: [i16; 3] = [176, 219, 262];

impl ChartCache {
    pub const fn new() -> Self {
        ChartCache {
            scale: None,
            side: None,
            rows: [i64::MIN; 5],
            time: None,
            rate: 0,
            rchip: None,
            cols: [NO_SPAN; CHART_W as usize],
            drawn: false,
        }
    }
    pub fn invalidate(&mut self) {
        *self = ChartCache::new();
    }
}

/// The rate chip (`chart` draws it with every chart point; App::
/// resolution_cycle at once, before its toast: the page under a toast is
/// not drawn until the toast goes, and the chip is outside the toast).
/// Repainted only when what it shows changes (`RateChip::shown`).
pub fn rate_chip(c: &mut ChartCache, rc: RateChip) {
    if c.rchip == Some(rc.shown()) {
        return;
    }
    c.rchip = Some(rc.shown());
    // "MED 9.4/s" (<= 62 px of F14: "SLOW 2.3/s"); grey "n/a 2.5/s"
    // where the rate setting changes nothing (AC, Hz, cap, temperature..).
    let mut rb = [0u8; 16];
    let mut b = [0u8; 24];
    let name = if rc.applies { crate::dmm::RATE_NAMES[rc.rate as usize % 3] } else { "n/a" };
    let txt = join(&mut b, &[name, " ", rps_text(&mut rb, rc.period_ms)]);
    let r = rect(4, chart_geom().rate, 73, RATE_CHIP_H);
    sf::rbox(r, 4, BTN, BG);
    sf::glyphs(sf::F14, rect(r.x, r.y + 1, r.w, r.h), txt, if rc.applies { STATUS_FG } else { DIM }, BTN, 8, 1);
}

/// `side` None: the right column is not touched (the power meter draws its
/// own, `pw::side`).
pub fn chart(c: &mut ChartCache, pts: &[f32], dec: u8, rate: u8, side: Option<&Side>, unit: &'static str, rc: RateChip) {
    let mut step = 1.0f32;
    for _ in 0..dec {
        step /= 10.0;
    }
    let (mut lo, mut hi) = (f32::MAX, f32::MIN);
    for &p in pts {
        lo = lo.min(p);
        hi = hi.max(p);
    }
    if pts.is_empty() {
        lo = 0.0;
        hi = 0.0;
    }
    lo -= 50.0 * step;
    hi += 50.0 * step;
    let span = (hi - lo).max(1e-12);
    let mut b = [0u8; 32];
    let key = (hi.to_bits(), lo.to_bits(), dec, unit);
    let g = chart_geom();
    let (cy, ch) = (g.y, g.h);
    if c.scale != Some(key) {
        c.scale = Some(key);
        // Axis labels right-aligned to x 76 (3 px left of the frame), caps on
        // the frame's top row, its middle and its bottom row; 1 px spacing.
        for (y, v) in [(g.axis[0], hi), (g.axis[1], (hi + lo) / 2.0), (g.axis[2], lo)] {
            // Fewer decimals when the label is wider than its box (power:
            // "-100.049kWh" is 75 px).
            let mut d = dec;
            while d > 0 && sf::text_width(sf::F14, num_unit(&mut b, v, d, unit), 1) > 73 {
                d -= 1;
            }
            sf::label(sf::F14, rect(4, y, 73, 14), num_unit(&mut b, v, d, unit), STATUS_FG, BG, 4, 1);
        }
    }
    // Column templates: plain / dotted column / vertical grid line / frame.
    let mut tpl = [[0u16; CHART_H as usize]; 4];
    for y in 0..ch as usize {
        let edge = y == 0 || y == ch as usize - 1;
        let hline = y % 16 == 0;
        tpl[0][y] = if edge { DIM } else { BG };
        tpl[1][y] = if edge || hline { DIM } else { BG };
        tpl[2][y] = if edge || hline || y % 4 == 0 { DIM } else { BG };
        tpl[3][y] = DIM;
    }
    let row = |v: f32| -> u16 {
        (((hi - v) / span) * (ch - 3) as f32 + 1.0).clamp(1.0, (ch - 2) as f32) as u16
    };
    let n = pts.len();
    let first = CHART_W as usize - n.min(CHART_W as usize);
    let mut prev: Option<u16> = None;
    let full = !c.drawn;
    c.drawn = true;
    for x in 0..CHART_W {
        // Trace rows of this column (rows 1..CHART_H - 2: fit a u8).
        let span = if (x as usize) >= first {
            let y = row(pts[x as usize - first]);
            let s = match prev {
                Some(p) if p < y => (p, y),
                Some(p) => (y, p),
                None => (y, y),
            };
            prev = Some(y);
            (s.0 as u8, s.1 as u8)
        } else {
            NO_SPAN
        };
        let old = core::mem::replace(&mut c.cols[x as usize], span);
        // Rows to rewrite: the whole column on a full draw, else the union
        // of the old and the new trace span (the rest is unchanged).
        let (y0, y1) = if full {
            (0, ch - 1)
        } else if old == span {
            continue;
        } else if old == NO_SPAN {
            (span.0 as u16, span.1 as u16)
        } else if span == NO_SPAN {
            (old.0 as u16, old.1 as u16)
        } else {
            (old.0.min(span.0) as u16, old.1.max(span.1) as u16)
        };
        let k = if x == 0 || x == CHART_W - 1 { 3 } else if x % 16 == 0 { 2 } else if x % 4 == 0 { 1 } else { 0 };
        let t = &tpl[k];
        // (Never in a strip pass: the DMM page is drawn directly.)
        lcd::set_window(CHART_X + x, cy + y0, CHART_X + x, cy + y1);
        // Template above the trace, trace, template below (NO_SPAN: the
        // first loop covers the column).
        let (a, b2) = (span.0 as u16, span.1 as u16);
        let a_end = a.min(y1 + 1);
        for y in y0..a_end {
            lcd::data_panel(t[y as usize]);
        }
        for _ in a.max(y0)..=b2.min(y1) {
            lcd::data_panel(0xFC40);
        }
        for y in (b2 + 1).max(y0).max(a_end)..=y1 {
            lcd::data_panel(t[y as usize]);
        }
    }
    if c.rate != rate {
        c.rate = rate;
        let mut t = [0u8; 16];
        // "640ms/div", from 1280 ms "1.28s/div" (the chip holds 9 glyphs).
        let ms = rate as u32 * 160;
        let (s, u): (&str, &[u8]) =
            if ms < 1000 { (crate::dmm::format(&mut t, ms as f32, 0), b"ms/div") } else { (crate::dmm::format(&mut t, ms as f32 / 1000.0, 2), b"s/div") };
        let mut l = [0u8; 16];
        let k = s.len().min(10);
        l[..k].copy_from_slice(&s.as_bytes()[..k]);
        l[k..k + u.len()].copy_from_slice(u);
        // Timebase chip (tap: next rate), between the top and middle labels.
        let r = rect(4, g.tb, 73, 17);
        sf::rbox(r, 4, BTN, BG);
        sf::glyphs(sf::F14, rect(r.x, r.y + 1, r.w, r.h), core::str::from_utf8(&l[..k + u.len()]).unwrap_or(""), STATUS_FG, BTN, 8, 1);
    }
    rate_chip(c, rc);
    let Some(side) = side else { return };
    // Right column (x 405..475, the REL box rows 205..240 the tap target):
    // MAX and AVG above the REL box, MIN, SD and the session time below; one F14 row each, label left (dim), value right-aligned
    // without the unit, which is shown once, in the REL box header
    // ("REL" .. "mV"). The stock version crammed "4.8170V" into 54 px with
    // touching glyphs.
    if c.side != Some((dec, unit)) {
        c.side = Some((dec, unit));
        c.rows = [i64::MIN; 5];
        c.time = None;
        lcd::fill_rect(402, 174, 78, 31, BG);
        lcd::fill_rect(402, 241, 78, 38, BG);
        for (i, t) in ["MAX", "AVG", "MIN", "SD"].iter().enumerate() {
            sf::label(sf::F14, rect(SIDE_X, STAT_Y[i], 22, 14), t, DIM, BG, 0, 1);
        }
    }
    let q = |v: Option<f32>| -> i64 {
        match v {
            Some(x) if x.is_finite() => {
                let mut s = 1.0f32;
                for _ in 0..dec {
                    s *= 10.0;
                }
                let k = x * s;
                (if k < 0.0 { k - 0.5 } else { k + 0.5 }) as i64
            }
            _ => i64::MIN,
        }
    };
    let mut t = [0u8; 16];
    for (i, v) in [side.max, side.avg, side.min, side.sd].iter().enumerate() {
        let k = q(*v);
        if c.rows[i] != k {
            c.rows[i] = k;
            // One decimal less when the value is wider than its box
            // ("-444.44" is 50 px of 46: its minus was cut).
            let bw = SIDE_R - SIDE_X - 25;
            let mut d = dec;
            while d > 0 && v.is_some_and(|v| sf::text_width(sf::F14, crate::dmm::format(&mut t, v, d), 1) > bw) {
                d -= 1;
            }
            let s = v.map(|v| crate::dmm::format(&mut t, v, d)).unwrap_or("");
            sf::label(sf::F14, rect(SIDE_X + 25, STAT_Y[i], bw, 14), s, STATUS_FG, BG, 3, 1);
        }
    }
    let k = q(side.rel.or(Some(0.0)));
    if c.rows[4] != k {
        c.rows[4] = k;
        let r = rect(405, 205, 71, 36);
        sf::rbox(r, 6, BTN, BG);
        sf::glyphs(sf::F14, rect(409, 209, 30, 14), "REL", STATUS_FG, BTN, 0, 1);
        let mut u = [0u8; 16];
        sf::glyphs(sf::F14, rect(430, 209, 42, 14), sf::unit_text(unit, &mut u), DIM, BTN, 3, 1);
        let rv = crate::dmm::format(&mut t, side.rel.unwrap_or(0.0), dec);
        sf::glyphs(sf::F14, rect(409, 225, 63, 14), rv, STATUS_FG, BTN, 3, 1);
    }
    let tk = side.secs.map(|t| (t, side.rec));
    if c.time != tk {
        c.time = tk;
        // "REC 01:02:03" (recording session, orange) / "T 01:02:03" since
        // the statistics started.
        let mut tb = [0u8; 8];
        let mut b = [0u8; 24];
        let txt = match side.secs {
            Some(sec) => join(&mut b, &[if side.rec { "REC " } else { "T " }, hms(&mut tb, sec)]),
            None => "",
        };
        // "REC 99:59:59" is 75 px of 71: the time alone (orange = REC).
        let mut b2 = [0u8; 24];
        let txt = if sf::text_width(sf::F14, txt, 1) > SIDE_R - SIDE_X {
            let k = txt.len() - txt.trim_start_matches(|c: char| c != ' ').len();
            let t = &txt[(k + 1).min(txt.len())..];
            b2[..t.len()].copy_from_slice(t.as_bytes());
            core::str::from_utf8(&b2[..t.len()]).unwrap_or("")
        } else {
            txt
        };
        let fg = if side.rec { 0xFC40 } else { DIM };
        sf::label(sf::F14, rect(SIDE_X, STAT_Y[4], SIDE_R - SIDE_X, 14), txt, fg, BG, 3, 1);
    }
}

// ---------------------------------------------------------------- scope
// The scope page and the combined view's graph are the stock replica
// (src/scope_ui.rs, src/scope_page.rs); what is left here: the colours the
// combined view's function band uses and the cursor state.

const S_BAR: Color = 0x4208;
const S_FRAME: Color = 0xAD55;
/// Measurement bar rows (the function band ends there).
const MEAS_Y: u16 = 301;

/// Scope cursors: two vertical (columns 1..399) and two horizontal (rows of
/// the 280-row model), as the stock X2-1 / Xf / Y2-1 items.
#[derive(Clone, Copy)]
pub struct Cursors {
    pub x_on: bool,
    pub y_on: bool,
    pub x: [u16; 2],
    pub y: [i32; 2],
}

// ------------------------------------------------------------ generator
// Stock generator page (scope_gen_ui_v2_6.h section 6).

const G_BG: Color = 0x10A3;
const G_BAR: Color = 0x2146;
const G_BOX: Color = 0x3A08;
const G_TRACE: Color = 0x04A9;

const G_DIM: Color = 0x7BEF;
/// Generator value boxes (right column x 246..475, 66 px each): Frequency,
/// Vpp, Offset (FR-28), Duty. A tap opens the box's keypad (`gen_box_hit`).
/// Each box: the name (TEXT22) and a note (F14, right) on the top row, the
/// value (LETTERS, right-aligned to 411) and its unit (from 421) below.
pub const GEN_BOX_Y: [i16; 4] = [40, 110, 180, 250];
const GEN_BOX_H: i16 = 66;
const GEN_NAMES: [&str; 4] = ["Frequency", "Vpp", "Offset", "Duty"];

/// The wave drop-down box (4, 40, 239, 42) under a tap.
pub fn gen_wave_hit(x: u16, y: u16) -> bool {
    x < 243 && (40..82).contains(&y)
}

/// Generator box (0..3) under a tap.
pub fn gen_box_hit(x: u16, y: u16) -> Option<usize> {
    let y = y as i16;
    if x < 246 {
        return None;
    }
    GEN_BOX_Y.iter().position(|&b| y >= b && y < b + GEN_BOX_H)
}

/// Value / unit row of generator box `k` (the frequency keypad types into
/// box 0's). `letters`: the value in LETTERS (digits), else TEXT22 ("auto").
/// `bg`: the box's colour (G_BOX; K_SEL pressed).
fn gen_box_value(k: usize, v: &str, letters: bool, unit: &str, fg: Color, bg: Color) {
    gen_box_value_bg(k, v, letters, unit, fg, bg, bg);
}

/// `gen_box_value` with the value field's background `vbg` (the keypad's
/// selected prefill: K_SEL).
fn gen_box_value_bg(k: usize, v: &str, letters: bool, unit: &str, fg: Color, vbg: Color, bg: Color) {
    let y = GEN_BOX_Y[k];
    sf::label(if letters { sf::LETTERS } else { sf::TEXT22 }, rect(262, y + 33, 150, 30), v, fg, vbg, 4, 0);
    sf::label(sf::TEXT22, rect(421, y + 36, 50, 26), unit, fg, bg, 1, 0);
}

/// Note at the right of box `k`'s name row ("" clears it).
fn gen_box_note(k: usize, note: &str, fg: Color, bg: Color) {
    sf::label(sf::F14, rect(360, GEN_BOX_Y[k] + 3, 112, 26), note, fg, bg, 4, 1);
}

/// Box `k` (0..3; 4 = the wave drop-down) normal or pressed (K_SEL, the
/// keypads' pressed colour; ui-review #11: feedback from touch-down).
pub fn gen_press(g: &gen::Gen, k: usize, pressed: bool) {
    let bg = if pressed { K_SEL } else { G_BOX };
    if k >= 4 {
        gen_wave_box(g.s.wave, bg);
        return;
    }
    gen_box_frame(k, bg);
    gen_box(g, k, bg);
}

/// Box `k`'s fill and name.
fn gen_box_frame(k: usize, bg: Color) {
    let y = GEN_BOX_Y[k];
    sf::rbox(rect(246, y, 230, GEN_BOX_H), 3, bg, G_BG);
    sf::glyphs(sf::TEXT22, rect(257, y + 2, 100, 26), GEN_NAMES[k], STATUS_FG, bg, 1, 0);
}

/// The wave drop-down: name and the stock arrow.
fn gen_wave_box(w: gen::Wave, bg: Color) {
    sf::rbox(rect(4, 40, 239, 42), 3, bg, G_BG);
    sf::glyphs(sf::TEXT22, rect(14, 40, 180, 42), w.name(), STATUS_FG, bg, 1, 0);
    sf::glyphs(sf::Font(0x0805_9428), rect(197, 40, 42, 42), " ", STATUS_FG, bg, 8, 0);
}

pub fn gen_static(g: &gen::Gen) {
    lcd::fill_rect(0, 0, lcd::W, 36, G_BAR);
    lcd::fill_rect(0, 36, lcd::W, lcd::H - 36, G_BG);
    for k in 0..4 {
        gen_box_frame(k, G_BOX);
    }
    gen_values(g);
}

/// RUN/STOP label of the generator bar (x < GEN_RUN_W): a tap
/// toggles the output like the RUN key (ui-review #9).
const GEN_RUN_W: u16 = BAR_X as u16 + 80;

pub fn gen_run_hit(x: u16, y: u16) -> bool {
    x < GEN_RUN_W && y < 36
}

/// Generator bar status (ui-review #9; stock: uptime (186,0,120,36), battery
/// (430,8,40,18)): uptime, REC, lock, BT link, USB, battery, at the DMM bar's x
/// positions (`status_right`), centred in the 36 px bar.
pub fn gen_status(st: &Status) {
    let mut t = [0u8; 8];
    let mut b = [0u8; 4];
    let bc = if st.charging { 0x04A9 } else { STATUS_FG };
    sf::label(sf::F21, rect(186, 6, 120, 24), hms(&mut t, st.uptime_s), STATUS_FG, G_BAR, 1, -1);
    status_rec(st, G_BAR, 11);
    sf::label(sf::LOCK, rect(327, 8, 24, 20), if st.locked { "\u{20}" } else { "\u{21}" }, STATUS_FG, G_BAR, 8, 0);
    sf::label(sf::SAVE_BT, rect(390, 2, 16, 32), if st.bt { "\u{21}" } else { "" }, STATUS_FG, G_BAR, 1, 0);
    sf::label(sf::SAVE_BT, rect(412, 2, 16, 32), usb_glyph(st), STATUS_FG, G_BAR, 1, 0);
    sf::label(sf::BATTERY, rect(435, 10, 40, 16), battery_glyph(&mut b, st.bars), bc, G_BAR, 8, 0);
}

/// `gen_preview` in strips: the wave the table holds (its `sel` + 1, the
/// square's duty above bit 8; 0 = none) and each column's trace rows (from
/// the preview's top). All zero
/// at boot: `.sram_bss` is zeroed by the startup code, whatever the
/// initialiser says (#153: a 255 "none" read back 0 = Sine with an empty
/// table, so the boot-time Sine drew no trace).
#[cfg_attr(target_os = "none", link_section = ".sram_bss")]
pub(crate) static mut GEN_SP: (u32, [(u8, u8); 239]) = (0, [(0, 0); 239]);

// Region B (not inlined into the region C callers): drawn per strip.
#[inline(never)]
fn gen_preview(w: gen::Wave, duty: u16) {
    // Two periods over 238 points between h/4 and 3h/4, as the stock preview.
    let (x0, y0, ww, hh) = (4u16, 85u16, 239u16, 231u16);
    let sel = match w {
        gen::Wave::Sine => 0,
        gen::Wave::Ramp => 1,
        gen::Wave::Triangle => 2,
        gen::Wave::Square => 3,
    };
    // The square is high for the effective duty of each period (#81).
    let duty = if sel == 3 { duty.min(10000) } else { 0 };
    let d = duty as f32 / 10000.0;
    let key = (sel as u32 + 1) | (duty as u32) << 8;
    let yv = |i: u16| -> u16 {
        // The fraction (x in 0..2: exact, as `% 1.0`; f32 `%` pulls libm's
        // fmodf, ~0.7 KB, into region B).
        let x = i as f32 * 2.0 / 238.0;
        let ph = x - (x as u32) as f32;
        let v = match sel {
            0 => 0.5 + 0.5 * libsin(ph * 2.0 * core::f32::consts::PI),
            1 => ph,
            2 => if ph < 0.5 { ph * 2.0 } else { 2.0 - ph * 2.0 },
            _ => if ph < d { 1.0 } else { 0.0 },
        };
        y0 + hh * 3 / 4 - (v * (hh / 2) as f32) as u16
    };
    // A strip pass (lcd.rs "strip rendering"): the strip's rows.
    let Some((ya, yb)) = lcd::clip_rows(y0 as i16, (y0 + hh - 1) as i16) else {
        return;
    };
    let (ya, yb) = (ya as u16, yb as u16);
    if lcd::in_strip() {
        // Row by row into the strip from the columns' trace rows,
        // worked out once per wave (not once per strip).
        let sp = unsafe { &mut *core::ptr::addr_of_mut!(GEN_SP) };
        if sp.0 != key {
            let mut prev = yv(0);
            for i in 0..ww {
                let y = yv(i.min(237));
                let (a, b) = if y < prev { (y, prev) } else { (prev, y) };
                prev = y;
                sp.1[i as usize] = ((a - y0) as u8, (b - y0) as u8);
            }
            sp.0 = key;
        }
        for y in ya..=yb {
            let yy = y - y0;
            let Some(row) = lcd::strip_span(y as i16, x0 as i16, (x0 + ww - 1) as i16) else {
                continue;
            };
            let (edge, grid) = (yy == 0 || yy == hh - 1, yy % 46 == 0 && yy > 0);
            for (i, (p, &(a, b))) in row.iter_mut().zip(sp.1.iter()).enumerate() {
                let i = i as u16;
                *p = if yy >= a as u16 && yy <= b as u16 {
                    G_TRACE
                } else if edge || i == 0 || i == ww - 1 {
                    0x73CF
                } else if grid || (i % 48 == 0 && i > 0) {
                    G_BOX
                } else {
                    G_BG
                };
            }
        }
        return;
    }
    let mut prev = yv(0);
    for i in 0..ww {
        let y = yv(i.min(237));
        let (a, b) = if y < prev { (y, prev) } else { (prev, y) };
        prev = y;
        lcd::set_window(x0 + i, ya, x0 + i, yb);
        for yy in ya - y0..=yb - y0 {
            let edge = i == 0 || i == ww - 1 || yy == 0 || yy == hh - 1;
            let grid = (i % 48 == 0 && i > 0) || (yy % 46 == 0 && yy > 0);
            let c = if y0 + yy >= a && y0 + yy <= b {
                G_TRACE
            } else if edge {
                0x73CF
            } else if grid {
                G_BOX
            } else {
                G_BG
            };
            lcd::data(c);
        }
    }
}

fn libsin(x: f32) -> f32 {
    const PI: f32 = core::f32::consts::PI;
    let mut x = x;
    while x > PI {
        x -= 2.0 * PI;
    }
    let x = if x > PI / 2.0 { PI - x } else if x < -PI / 2.0 { -PI - x } else { x };
    let x2 = x * x;
    x * (1.0 - x2 / 6.0 * (1.0 - x2 / 20.0 * (1.0 - x2 / 42.0)))
}

pub fn gen_values(g: &gen::Gen) {
    let s = &g.s;
    // Top bar: RUN/STOP (output state).
    sf::label(sf::F21, rect(BAR_X, 0, 80, 36), if s.on { "RUN" } else { "STOP" }, if s.on { 0x07E0 } else { STATUS_FG }, G_BAR, 1, 0);
    gen_wave_box(s.wave, G_BOX);
    gen_preview(s.wave, g.out.duty);
    for k in 0..4 {
        gen_box(g, k, G_BOX);
    }
}

/// Value, unit and note of generator box `k` on `bg`.
fn gen_box(g: &gen::Gen, k: usize, bg: Color) {
    let s = &g.s;
    let out = &g.out;
    let mut t = [0u8; 16];
    // Vpp / Offset: the set values; "auto" = the stock 0..Vpp (centre
    // Vpp/2). On the PWM path (square above 50 kHz) both are fixed by the
    // logic output: grey, note "logic level".
    let fixed = out.fixed_level();
    let lv = if fixed { G_DIM } else { STATUS_FG };
    match k {
        0 => {
            // Frequency: the actual (quantised) output in full digits,
            // "32768.08", 3 decimals below 1 kHz (UF-15). Above the wave's
            // limit (UF-14): the set value grey and the limit ("max 50 kHz")
            // as the note.
            let clamped = g.freq_clamped();
            let f = if clamped { s.freq_hz as f32 } else { g.actual_hz_f64() as f32 };
            let dec = if f < 1e3 { 3 } else if f < 1e5 { 2 } else if f < 1e6 { 1 } else { 0 };
            gen_box_value(0, crate::dmm::format(&mut t, f, dec), true, "Hz", if clamped { G_DIM } else { STATUS_FG }, bg);
            let note = s.wave.max_note();
            gen_box_note(0, if clamped { note } else { "" }, VALUE, bg);
        }
        1 | 2 => {
            match (k, s.offset_v) {
                (1, _) => gen_box_value(1, crate::dmm::format(&mut t, s.amp_v, 2), true, "V", lv, bg),
                (_, Some(o)) => gen_box_value(2, crate::dmm::format(&mut t, o, 2), true, "V", lv, bg),
                (_, None) => gen_box_value(2, "auto", false, "", lv, bg),
            }
            gen_box_note(k, if fixed { "logic level" } else { "" }, G_DIM, bg);
        }
        _ => {
            // Duty: square only (DAC table or PWM counter, any frequency);
            // the effective, quantised value.
            let sq = s.wave == gen::Wave::Square;
            let d = if sq { out.duty_pct() } else { s.duty as f32 / 100.0 };
            gen_box_value(3, crate::dmm::format(&mut t, d, 1), true, "%", if sq { STATUS_FG } else { G_DIM }, bg);
        }
    }
}

// --------------------------------------------------------------- keypads
// Stock generator keypads: key bg 0x6b4d, panel 0xe73c, keys 60x40.

const K_KEY: Color = 0x6B4D;
const K_PANEL: Color = 0xE73C;
const K_FONT: sf::Font = sf::Font(0x0805_C5DC);

type Key = (sf::Rect, &'static str, u8);

/// Keypad kinds: the generator frequency field, the generator Vpp / duty
/// popup, and the same popup with a "+/-" key (DMM limits).
pub const KP_FREQ: u8 = 0;
pub const KP_NUM: u8 = 1;
pub const KP_SIGNED: u8 = 2;

/// Keys: (rect, label, code). Codes: digits, '.', 'H'/'K'/'M', '<' backspace,
/// '-' sign toggle, '=' OK. Constant tables (cheaper in the RAM image than
/// building them).
pub fn keypad_keys(kind: u8) -> &'static [Key; 15] {
    const fn r(x: i16, y: i16, w: i16, h: i16) -> sf::Rect {
        rect(x, y, w, h)
    }
    const FREQ: [Key; 15] = [
            (r(228, 146, 60, 40), "1", b'1'), (r(292, 146, 60, 40), "2", b'2'), (r(356, 146, 60, 40), "3", b'3'),
            (r(420, 146, 56, 40), "H", b'H'), (r(228, 190, 60, 40), "4", b'4'), (r(292, 190, 60, 40), "5", b'5'),
            (r(356, 190, 60, 40), "6", b'6'), (r(420, 190, 56, 40), "K", b'K'), (r(228, 234, 60, 40), "7", b'7'),
            (r(292, 234, 60, 40), "8", b'8'), (r(356, 234, 60, 40), "9", b'9'), (r(420, 234, 56, 40), "M", b'M'),
            (r(228, 277, 60, 40), "0", b'0'), (r(291, 277, 123, 40), "<", b'<'), (r(417, 277, 59, 40), "OK", b'='),
        ];
    const NUM: [Key; 15] = [
            (r(228, 146, 60, 40), "1", b'1'), (r(292, 146, 60, 40), "2", b'2'), (r(356, 146, 60, 40), "3", b'3'),
            (r(228, 190, 60, 40), "4", b'4'), (r(292, 190, 60, 40), "5", b'5'), (r(356, 190, 60, 40), "6", b'6'),
            (r(228, 234, 60, 40), "7", b'7'), (r(292, 234, 60, 40), "8", b'8'), (r(356, 234, 60, 40), "9", b'9'),
            (r(228, 277, 60, 40), "0", b'0'), (r(291, 277, 123, 40), ".", b'.'), (r(417, 146, 59, 84), "<", b'<'),
            (r(417, 234, 59, 83), "OK", b'='), (r(0, 0, 0, 0), "", 0), (r(0, 0, 0, 0), "", 0),
        ];
    // NUM with the wide '.' split into '.' and "+/-".
    const SIGNED: [Key; 15] = [
            (r(228, 146, 60, 40), "1", b'1'), (r(292, 146, 60, 40), "2", b'2'), (r(356, 146, 60, 40), "3", b'3'),
            (r(228, 190, 60, 40), "4", b'4'), (r(292, 190, 60, 40), "5", b'5'), (r(356, 190, 60, 40), "6", b'6'),
            (r(228, 234, 60, 40), "7", b'7'), (r(292, 234, 60, 40), "8", b'8'), (r(356, 234, 60, 40), "9", b'9'),
            (r(228, 277, 60, 40), "0", b'0'), (r(292, 277, 60, 40), ".", b'.'), (r(417, 146, 59, 84), "<", b'<'),
            (r(417, 234, 59, 83), "OK", b'='), (r(356, 277, 58, 40), "+/-", b'-'), (r(0, 0, 0, 0), "", 0),
        ];
    match kind {
        KP_FREQ => &FREQ,
        KP_NUM => &NUM,
        _ => &SIGNED,
    }
}

/// `sel`: `input` is the current value, prefilled and selected (the next
/// digit replaces it): drawn on the stock pressed-key colour.
pub fn keypad(kind: u8, title: &str, input: &str, unit: &str, sel: bool) {
    let freq = kind == KP_FREQ;
    if freq {
        lcd::fill_rect(224, 142, 256, 178, K_PANEL);
    } else {
        lcd::fill_rect(224, 100, 256, 220, K_PANEL);
        sf::label(sf::TEXT22, rect(228, 108, 100, 26), title, 0x0000, K_PANEL, 1, 0);
    }
    keypad_input(freq, input, unit, sel);
    for &(_, _, c) in keypad_keys(kind) {
        if c != 0 {
            keypad_key(kind, c, false);
        }
    }
}

/// Keypad key `code` normal or pressed (stock pressed colour 0x04FC;
/// ui-review #11: feedback at touch-down).
pub fn keypad_key(kind: u8, code: u8, pressed: bool) {
    if let Some(&(r, l, c)) = keypad_keys(kind).iter().find(|k| k.2 == code && code != 0) {
        let bg = if pressed { K_SEL } else { K_KEY };
        sf::rbox(r, 4, bg, K_PANEL);
        sf::glyphs(if l.len() > 1 || c == b'<' { sf::TEXT22 } else { K_FONT }, r, l, STATUS_FG, bg, 8, 0);
    }
}

/// Selected keypad text (the prefilled current value): the stock pressed-key
/// colour as the field background.
const K_SEL: Color = 0x04FC;

/// The typed text: in the page's frequency field, or the popup's value box.
/// `sel`: the prefilled value, selected (see `keypad`).
pub fn keypad_input(freq: bool, input: &str, unit: &str, sel: bool) {
    if freq {
        if sel {
            gen_box_value_bg(0, input, true, unit, STATUS_FG, K_SEL, G_BOX);
        } else {
            gen_box_value(0, input, true, unit, K_SEL, G_BOX);
        }
        gen_box_note(0, "", G_DIM, G_BOX);
    } else {
        // 300..439: room for "-12.345" (112 px); titles end before 300.
        let (fg, bg) = if sel { (STATUS_FG, K_SEL) } else { (0x0000, K_PANEL) };
        sf::label(sf::LETTERS, rect(300, 104, 139, 34), input, fg, bg, 4, 0);
        sf::label(sf::TEXT22, rect(446, 110, 30, 22), unit, 0x0000, K_PANEL, 1, 0);
    }
}

pub fn keypad_hit(kind: u8, x: u16, y: u16) -> Option<u8> {
    let (x, y) = (x as i16, y as i16);
    // A key's own box first, then 2 px of slop around it (the 1..5 px gaps
    // between keys go to a neighbour instead of nowhere).
    for s in [0, 2] {
        for &(r, _, c) in keypad_keys(kind) {
            if c != 0 && x >= r.x - s && x < r.x + r.w + s && y >= r.y - s && y < r.y + r.h + s {
                return Some(c);
            }
        }
    }
    None
}

// ----------------------------------------------------------------- menu
// Stock main menu (gui_v2_6.h section 2) without its logo: four tiles
// 190x106 with the stock icons; plus a fifth row for the combined views of this firmware.

const M_BG: Color = 0x10A3;
const M_TILE: Color = 0x2146;
const M_ACCENT: Color = 0x04FC;
const ICONS: sf::Font = sf::Font(0x0805_592C);

pub const TILES: [(i16, i16, &str, u32); 4] = [
    (46, 52, "Multimeter", 0x20),
    (244, 52, "Oscilloscope", 0x21),
    (46, 166, "Signal", 0x22),
    (244, 166, "Settings", 0x23),
];

/// Pressed menu tile / row: the stock tile style's second colour
/// (gui_v2_6.h, style 0x0806c754: bg 0x2146, pressed 0x4419; its role is
/// the RE note's guess).
const M_PRESSED: Color = 0x4419;

pub fn main_menu(combo: &str) {
    lcd::fill_rect(0, 0, lcd::W, lcd::H, M_BG);
    for i in 0..6 {
        menu_item(i, combo, false);
    }
}

/// Menu item `i` (as `main_menu_hit`: tiles 0..3, the combined-view row 4,
/// the gallery row 5), normal or pressed (ui-review #11: from touch-down).
pub fn menu_item(i: usize, combo: &str, pressed: bool) {
    let bg = if pressed { M_PRESSED } else { M_TILE };
    if let Some(&(x, y, name, icon)) = TILES.get(i) {
        sf::rbox(rect(x, y, 190, 106), 8, bg, M_BG);
        let g = char::from_u32(icon).unwrap_or(' ');
        let mut b = [0u8; 4];
        sf::glyphs(ICONS, rect(x + 79, y + 25, 30, 30), g.encode_utf8(&mut b), STATUS_FG, bg, 8, 0);
        sf::glyphs(sf::TEXT22, rect(x, y + 58, 190, 27), name, STATUS_FG, bg, 8, 0);
    } else if i == 5 {
        // Gallery row above the tiles (FR-16).
        sf::rbox(rect(46, 10, 388, 34), 8, bg, M_BG);
        sf::glyphs(sf::TEXT22, rect(46, 10, 388, 34), "Gallery: screenshots and logs", STATUS_FG, bg, 8, 0);
    } else {
        sf::rbox(rect(46, 280, 388, 34), 8, bg, M_BG);
        let mut t = [0u8; 48];
        let pre = b"Combined view: ";
        t[..pre.len()].copy_from_slice(pre);
        let k = combo.len().min(48 - pre.len());
        t[pre.len()..pre.len() + k].copy_from_slice(&combo.as_bytes()[..k]);
        sf::glyphs(sf::TEXT22, rect(46, 280, 388, 34), core::str::from_utf8(&t[..pre.len() + k]).unwrap_or(""), STATUS_FG, bg, 8, 0);
    }
}

/// Which tile (0..3) or the combined-view row (4) a touch hits.
pub fn main_menu_hit(x: u16, y: u16) -> Option<usize> {
    let (x, y) = (x as i16, y as i16);
    for (i, (tx, ty, _, _)) in TILES.iter().enumerate() {
        // 4 px of slop: the 8 px gaps between tiles split between them.
        if x >= *tx - 4 && x < tx + 194 && y >= *ty - 4 && y < ty + 110 {
            return Some(i);
        }
    }
    if (42..438).contains(&x) && (276..318).contains(&y) {
        return Some(4);
    }
    if (42..438).contains(&x) && (6..48).contains(&y) {
        return Some(5);
    }
    None
}

/// System settings (subset of the stock sections that this firmware has),
/// three pages of at most SET_ROWS rows. Page 1: the last row "More" opens
/// page 2 (SETTINGS2): the DMM rate / smoothing (FR-05), the DMM page's
/// bargraph / history row (UF-30), Help; its "More" opens page 3
/// (SETTINGS3): the wrong-connection alerts, "Device" (the stock §6
/// Device/About page) and "Touch test". "Back" is the tappable title bar (stock settings have
/// a title-bar back, not a list row), handled by `title_back_hit` (page 3 ->
/// page 2 -> page 1 -> menu), so the lists hold only settings.
pub const SETTINGS: [&str; 9] = [
    "Dim screen", "Shutdown", "Backlight", "Key/alert volume", "Bluetooth", "Start page", "Analog power",
    "Scope drag", "More",
];
pub const SETTINGS2: [&str; 9] = ["DMM rate", "Smoothing", "DMM row", "Temp unit", "Scope swipe", "Scope press", "Scope tap step", "Help", "More"];
/// Page 3: the wrong-connection alerts (sentinel.rs; ON = all, A-JACK only,
/// OFF), the danger alarm (danger.rs, doc
/// 4.10: Loud / Volume / Off, persisted), the USB disk mode (decisions-2026-10-22 #6: vwrite.rs, volatile,
/// applies at the next USB plug), the burden voltage readout of the A
/// functions (doc 3.15; persisted, default OFF = the stock aux row), the
/// Device / About page, the touch test page (app_touchtest.rs) and the
/// accuracy label (../docs/dmm-accuracy.md; persisted, default ON) and
/// ohm readings below 0 (Dmm::ohm_neg; persisted, default OFF); appended,
/// so the rows above keep their places.
/// Row 9, "Mode Remember" (the stock's name and place, Other Settings;
/// default OFF as the stock): ON restarts the multimeter after a power-off
/// as it was (app.rs `restore`).
pub const SETTINGS3: [&str; 9] = [
    "Jack alerts", "Danger alarm", "USB disk", "Burden voltage", "Device", "Touch test", "Accuracy", "Negative ohm", "Mode Remember",
];

/// Settings 3/3 "USB disk" value: the mode asked for, "(re-plug)" while the
/// one in force differs (a host reads the WP bit when it mounts).
pub fn usb_disk_name(ro_now: bool, ro_want: bool) -> &'static str {
    match (ro_want, ro_now == ro_want) {
        (true, true) => "Files live",
        (false, true) => "PC can edit",
        (true, false) => "Files live (re-plug)",
        (false, false) => "PC can edit (re-plug)",
    }
}

/// Pressed feedback on the list pages (ui-review #11): `row`, `part` (0 the
/// row, -1 / +1 its stepper's "-" / "+" key), `on` (pressed / released).
#[derive(Clone, Copy, PartialEq)]
pub struct RowPress {
    pub row: u8,
    pub part: i8,
    pub on: bool,
}

/// While set, the list pages (`list_page`, `settings`, `record_page`)
/// repaint only that row or key (app.rs: the page's own draw call with its
/// texts, no full repaint). 0 = off; else on << 17 | 1 << 16 | (part + 1)
/// << 8 | row.
static ROW_ONLY: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

pub fn set_row_only(p: Option<RowPress>) {
    let v = p.map_or(0, |p| 1 << 16 | (p.on as u32) << 17 | (((p.part + 1) as u8 as u32) << 8) | p.row as u32);
    ROW_ONLY.store(v, core::sync::atomic::Ordering::Relaxed);
}

fn row_only() -> Option<RowPress> {
    let v = ROW_ONLY.load(core::sync::atomic::Ordering::Relaxed);
    (v != 0).then(|| RowPress { row: v as u8, part: ((v >> 8) & 0xFF) as i8 - 1, on: v & (1 << 17) != 0 })
}

/// Pressed stepper key / Start button (already the accent colour): the
/// stock popups' pressed colour (stock/re/stock-ui-notes/popups.md).
const KEY_PRESSED: Color = 0x7BEF;

/// Settings/menu-page title bar: 40 px, bg 0x2966 (stock gui_v2_6.h:101).
pub const PAGE_BAR_H: i16 = 40;
/// Settings list geometry (vendor rows are ~37 px, gui_v2_6.h:109): 9 rows
/// at pitch 30 fit the 320 px panel (44 + 8 * 30 + 27 = 311).
const SET_Y0: i16 = 44;
const SET_PITCH: i16 = 30;
const SET_ROW_H: i16 = 27;
const SET_ROWS: usize = 9;
/// Row `i` outer box. Shared by `settings()` and `settings_hit()` so the render
/// and the touch map cannot drift.
fn set_row(i: usize) -> sf::Rect {
    rect(8, SET_Y0 + i as i16 * SET_PITCH, 464, SET_ROW_H)
}

/// Draw the shared menu-page title bar with a left "< Back" affordance; the
/// whole bar is the back target (`title_back_hit`).
fn page_title(title: &str) {
    lcd::fill_rect(0, 0, lcd::W, PAGE_BAR_H as u16, 0x2966);
    // Centred on the screen; a title that would come closer than 28 px
    // (two word spaces) to "< Back" starts that far after it instead ("Help
    // 5/7: Record + alerts" ran into it: device check 92fb8fa).
    let w = sf::text_width(sf::TEXT22, title, 0);
    let back = BAR_X + 3 + sf::text_width(sf::TEXT22, "< Back", 0) + 28;
    if (480 - w) / 2 >= back {
        sf::glyphs(sf::TEXT22, rect(0, 0, 480, PAGE_BAR_H), title, STATUS_FG, 0x2966, 8, 0);
    } else {
        sf::glyphs(sf::TEXT22, rect(back, 0, 476 - back, PAGE_BAR_H), title, STATUS_FG, 0x2966, 1, 0);
    }
    sf::glyphs(sf::TEXT22, rect(BAR_X + 3, 0, 110, PAGE_BAR_H), "< Back", STATUS_FG, 0x2966, 1, 0);
}

/// A tap on the title bar is "go back".
pub fn title_back_hit(y: u16) -> bool {
    (y as i16) < PAGE_BAR_H
}

/// Settings page 1 rows with -/+ steppers (Backlight, Key/alert volume;
/// ui-review #12): no more wrap-around +10 % per tap.
pub const SET_STEP_ROWS: [usize; 2] = [2, 3];
/// Stepper keys of a settings row (x of "-", "+"), value between them.
const STEP_MINUS_X: i16 = 210;
const STEP_PLUS_X: i16 = 410;
const STEP_KEY_W: i16 = 54;

/// Settings page `page` (0 / 1 / 2) with its row values.
pub fn settings(page: u8, vals: &[&str]) {
    if page == 0 {
        let mut v = [""; 9];
        for (i, s) in vals.iter().enumerate().take(9) {
            v[i] = if SET_STEP_ROWS.contains(&i) { "" } else { s };
        }
        list_page("Settings", &SETTINGS, &v[..vals.len().min(9)]);
        for i in SET_STEP_ROWS {
            if let Some(t) = vals.get(i) {
                set_stepper(i, t, STEP_MINUS_X);
            }
        }
    } else if page == 1 {
        list_page("Settings 2/3", &SETTINGS2, vals);
    } else {
        list_page("Settings 3/3", &SETTINGS3, vals);
    }
}

/// A settings-style page: title bar with "< Back", up to SET_ROWS rows of
/// label + value (SET_* geometry; `settings_hit` / `list_hit` map touches).
pub fn list_page(title: &str, names: &[&str], vals: &[&str]) {
    // One value per row (the 26d64d4 row mis-mapping); host builds only
    // (ui-render), compiled away on the device.
    if cfg!(not(target_os = "none")) {
        assert_eq!(vals.len(), names.len(), "list_page {title}: values vs rows");
    }
    let only = row_only();
    if only.is_none() {
        page_title(title);
        lcd::fill_rect(0, PAGE_BAR_H as u16, lcd::W, lcd::H - PAGE_BAR_H as u16, M_BG);
    }
    for (i, name) in names.iter().enumerate().take(SET_ROWS) {
        // Pressed: the keypads' pressed colour, the value white on it.
        let (bg, vfg) = match only {
            None => (M_TILE, M_ACCENT),
            Some(p) if p.row as usize == i && p.part == 0 => if p.on { (K_SEL, STATUS_FG) } else { (M_TILE, M_ACCENT) },
            Some(_) => continue,
        };
        let r = set_row(i);
        sf::rbox(r, 6, bg, M_BG);
        // Label then value kept close together (value left-aligned just past a
        // 190 px label column) instead of flung to the far right edge.
        sf::glyphs(sf::TEXT22, rect(r.x + 12, r.y, 190, r.h), name, STATUS_FG, bg, 1, 0);
        sf::glyphs(sf::TEXT22, rect(r.x + 202, r.y, 250, r.h), vals[i], vfg, bg, 1, 0);
    }
}

/// "-" key at `minus_x`, value, "+" key (STEP_PLUS_X) in settings row `i`.
/// In `set_row_only` mode only the pressed / released key.
fn set_stepper(i: usize, val: &str, minus_x: i16) {
    let only = row_only();
    if only.is_some_and(|p| p.row as usize != i) {
        return;
    }
    let r = set_row(i);
    for (d, x, t) in [(-1, minus_x, "-"), (1, STEP_PLUS_X, "+")] {
        let bg = match only {
            None => M_ACCENT,
            Some(p) if p.part == d => if p.on { KEY_PRESSED } else { M_ACCENT },
            Some(_) => continue,
        };
        let k = rect(x, r.y + 3, STEP_KEY_W, r.h - 6);
        sf::rbox(k, 4, bg, M_TILE);
        sf::glyphs(sf::TEXT22, k, t, STATUS_FG, bg, 8, 0);
    }
    if only.is_some() {
        return;
    }
    let vx = minus_x + STEP_KEY_W + 6;
    sf::glyphs(sf::TEXT22, rect(vx, r.y, step_value_w(minus_x), r.h), val, M_ACCENT, M_TILE, 8, 0);
}

/// Width of a stepper's value box between the "-" key at `minus_x` and "+".
pub const fn step_value_w(minus_x: i16) -> i16 {
    STEP_PLUS_X - 6 - (minus_x + STEP_KEY_W + 6)
}

/// The Record page's "-" keys sit further left (its labels are short):
/// "Every reading" / "Until stopped" (143 px) need more than Settings 1's
/// 134 px value box.
pub const REC_STEP_MINUS_X: i16 = 150;

/// A tap at `x` on a stepper row: -1 (left of the value's centre: the
/// label and the "-" key) or +1.
pub fn settings_step(x: u16) -> i8 {
    step_dir(x, STEP_MINUS_X)
}

/// Settings' Backlight stepper: the next value on the 10, 15, 25 ... 95, 100
/// grid above (`dir` > 0) or below the current one, so 10 + goes to 15 and
/// 100 - to 95 (the manual's list); an off-grid value (an old settings file)
/// joins the grid on the first step.
pub fn backlight_step(cur: u8, dir: i8) -> u8 {
    const GRID: [u8; 11] = [10, 15, 25, 35, 45, 55, 65, 75, 85, 95, 100];
    if dir > 0 {
        GRID.iter().copied().find(|&g| g > cur).unwrap_or(100)
    } else {
        GRID.iter().rev().copied().find(|&g| g < cur).unwrap_or(10)
    }
}

/// `settings_step` on the Record page.
pub fn record_step(x: u16) -> i8 {
    step_dir(x, REC_STEP_MINUS_X)
}

fn step_dir(x: u16, minus_x: i16) -> i8 {
    let mid = (minus_x + STEP_KEY_W + STEP_PLUS_X) / 2;
    if (x as i16) < mid { -1 } else { 1 }
}

pub fn settings_hit(page: u8, y: u16) -> Option<usize> {
    list_hit(y, [SETTINGS.len(), SETTINGS2.len(), SETTINGS3.len()][page.min(2) as usize])
}

/// Row of a `list_page` with `n` rows under a touch at `y`.
pub fn list_hit(y: u16, n: usize) -> Option<usize> {
    // Each row owns the gap rows around it (half above, half below): a
    // fingertip that lands between two rows still picks the nearer one.
    let y = y as i16 - (SET_Y0 - (SET_PITCH - SET_ROW_H) / 2);
    if y < 0 {
        return None;
    }
    let i = (y / SET_PITCH) as usize;
    if i < n.min(SET_ROWS) { Some(i) } else { None }
}

/// Rows of the DMM Hold & Limits page (View::Limits). The ninth, "Record",
/// opens the Record page (View::Record; 9 rows at the settings pitch fit:
/// 44 + 8 x 30 + 27 = 311): the way there for continuity, diode and
/// frequency, which have no range page and so no Record button.
pub const LIMIT_ROWS: [&str; 9] = [
    "Auto Hold", "Limits", "Upper limit", "Lower limit", "Upper = reading", "Lower = reading", "Clear limits",
    "Fail beep", "Record",
];

/// Record page rows (decision 2026-10-22 #1): interval, On change
/// threshold and duration with -/+ steppers (as Settings 1), the free-time
/// estimate, Start / Stop.
pub const RECORD_ROWS: [&str; 5] = ["Interval", "Change", "Duration", "Free", ""];
/// Rows with a stepper on the Record page.
pub const REC_STEP_ROWS: [usize; 3] = [0, 1, 2];
/// Value texts of csvlog::INTERVALS_S / DURATIONS_S; the last interval,
/// "Fast", is the fast logger (fastlog.rs: raw AD1 / AD2 samples to
/// LOGS/FASTnnn.DLG at the LOG:OSR / LOG:SRATe rate; App::rec_fast).
/// "On change" is csvlog::ON_CHANGE.
pub const REC_INTERVALS: [&str; 6] = ["Every reading", "1 s", "10 s", "1 min", "On change", "Fast"];
/// Index of "Fast" in REC_INTERVALS.
pub const REC_FAST: usize = 5;
/// Texts of csvlog::CHANGES (the On change threshold, % of full scale).
pub const REC_CHANGES: [&str; 3] = ["0.1 %", "1 %", "5 %"];
pub const REC_DURATIONS: [&str; 5] = ["Until stopped", "10 min", "1 h", "8 h", "24 h"];
/// Record page row of the Start / Stop button.
pub const REC_START_ROW: usize = 4;

/// The Record page: title bar, interval / duration steppers, the estimate
/// (`free`: "about N h left" at the interval, or why there is none), Start / Stop
/// (a filled row: accent Start, red Stop while recording). `vals` =
/// interval, threshold, duration texts.
pub fn record_page(vals: [&str; 3], free: &str, recording: bool) {
    let names = RECORD_ROWS;
    list_page("Record", &names[..4], &["", "", "", free]);
    for (i, v) in REC_STEP_ROWS.iter().zip(vals) {
        set_stepper(*i, v, REC_STEP_MINUS_X);
    }
    let only = row_only();
    if only.is_some_and(|p| p.row as usize != REC_START_ROW) {
        return;
    }
    let r = set_row(REC_START_ROW);
    let (t, bg) = if recording { ("Stop recording", 0xF800) } else { ("Start recording", M_ACCENT) };
    let bg = if only.is_some_and(|p| p.on) { KEY_PRESSED } else { bg };
    sf::rbox(r, 6, bg, M_BG);
    sf::glyphs(sf::TEXT22, r, t, STATUS_FG, bg, 8, 0);
}

pub fn view_name(v: View) -> &'static str {
    match v {
        View::Dmm => "Multimeter",
        View::Scope => "Oscilloscope",
        View::Gen => "Signal",
        View::DmmScope => "DMM + Scope",
        View::Menu => "Main menu",
        View::Settings => "Settings",
        View::Range => "Range",
        View::Keypad => "Keypad",
        View::Device => "Device",
        View::Limits => "Hold & Limits",
        View::Record => "Record",
        View::Gallery => "Gallery",
        View::Help => "Help",
        View::Inrush => "Inrush",
        View::TouchTest => "Touch test",
    }
}

/// On-device help (UF-29: owners have no manual; gestures, resets and limits
/// were undiscoverable). Pages of (topic, what it does) rows in the settings
/// list style; a tap below the title bar turns the page, the title bar or
/// MENU goes back. The essentials only (region B audit #9: the old seven
/// pages were 1.4 KB of region B text); the manual has the rest.
pub const HELP: [(&str, &[(&str, &str)]); 2] = [
    (
        "Gestures",
        &[
            ("Swipe left / right", "Change view"),
            ("Swipe down (DMM)", "Record on / off"),
            ("3 fingers down", "Screenshot"),
            ("2 fingers down", "Power off"),
            ("Scope: drag", "Move the trace"),
        ],
    ),
    (
        "Keys",
        &[
            ("MENU", "Menu / back"),
            ("RUN", "Hold; run / stop"),
            ("AUTO", "Autorange"),
            ("MF", "Range / menu"),
            ("Power key twice", "Touch lock"),
        ],
    ),
];

/// Help page `page` (0..HELP.len()): title "Help n/N: topic", the rows as
/// the settings list (wider topic column: HELP_COL), the last row says
/// what a tap does.
/// The topic's box ends HELP_GAP before the value column: with the glyphs'
/// blank edge column >= 1.5 TEXT22 word spaces of ink between them
/// (closer, "Power key held 1.5 s Power off" read as one line; device
/// check 92fb8fa; ui-render's line rule).
pub const HELP_COL: i16 = 228;
pub const HELP_GAP: i16 = 19;
#[inline(never)] // .cold (link-native.x): not inlined into the hot caller
pub fn help_page(page: usize) {
    let (topic, rows) = &HELP[page % HELP.len()];
    let mut t = [0u8; 40];
    let mut n = 0;
    let mut nb = [0u8; 4];
    let mut put = |p: &str| {
        let k = p.len().min(40 - n);
        t[n..n + k].copy_from_slice(&p.as_bytes()[..k]);
        n += k;
    };
    let digit = |b: &mut [u8; 4], d: usize| -> usize {
        b[0] = b'0' + d as u8;
        1
    };
    put("Help ");
    let k = digit(&mut nb, page % HELP.len() + 1);
    put(core::str::from_utf8(&nb[..k]).unwrap_or(""));
    put("/");
    let k = digit(&mut nb, HELP.len());
    put(core::str::from_utf8(&nb[..k]).unwrap_or(""));
    put(": ");
    put(topic);
    page_title(core::str::from_utf8(&t[..n]).unwrap_or("Help"));
    lcd::fill_rect(0, PAGE_BAR_H as u16, lcd::W, lcd::H - PAGE_BAR_H as u16, M_BG);
    let next = HELP[(page + 1) % HELP.len()].0;
    let last = ("Tap: next page", next);
    for (i, (name, val)) in rows.iter().chain(core::iter::once(&last)).enumerate().take(SET_ROWS) {
        let r = set_row(i);
        sf::rbox(r, 6, M_TILE, M_BG);
        let fg = if i + 1 == SET_ROWS { M_ACCENT } else { STATUS_FG };
        sf::glyphs(sf::TEXT22, rect(r.x + 12, r.y, HELP_COL - HELP_GAP, r.h), name, fg, M_TILE, 1, 0);
        sf::glyphs(sf::TEXT22, rect(r.x + 12 + HELP_COL, r.y, r.w - 15 - HELP_COL, r.h), val, M_ACCENT, M_TILE, 1, 0);
    }
}

/// opendm40 firmware version (ident.rs, the same as *IDN?). This is the
/// replacement firmware's own version, NOT the stock "Software V2.6".
pub const FW_VERSION: &str = crate::ident::VERSION;

/// Width of the Device page's value column (`device_page`).
const DEVICE_VALUE_W: i16 = 260;

/// The Device page's "Software" value: "opendm40 <version>", or the version
/// alone when that is wider than the value column (a CI pull request build,
/// e.g. 0.3.2-pr148.1a2b3c4).
pub fn device_software(buf: &mut [u8; 48]) -> &str {
    let pre = b"opendm40 ";
    let v = FW_VERSION.as_bytes();
    let n = (pre.len() + v.len()).min(buf.len());
    buf[..pre.len()].copy_from_slice(pre);
    buf[pre.len()..n].copy_from_slice(&v[..n - pre.len()]);
    match core::str::from_utf8(&buf[..n]) {
        Ok(s) if sf::text_width(sf::TEXT22, s, 0) <= DEVICE_VALUE_W - 8 => s,
        _ => FW_VERSION,
    }
}

/// Device / About page (stock gui_v2_6.h section 6: Vendor / Web / Model /
/// versions, panel 0x08074a10 as left-aligned "label:value" lines). Rendered
/// in the opendm40 settings style (title bar + rows) so it matches the other
/// menu pages. `lines` are (label, value) pairs built by the caller.
#[inline(never)] // .cold (link-native.x): page draw
pub fn device_page(lines: &[(&str, &str)]) {
    page_title("Device");
    lcd::fill_rect(0, PAGE_BAR_H as u16, lcd::W, lcd::H - PAGE_BAR_H as u16, M_BG);
    for (i, (label, value)) in lines.iter().enumerate() {
        let y = 48 + i as i16 * 36;
        sf::rbox(rect(8, y, 464, 33), 6, M_TILE, M_BG);
        sf::glyphs(sf::TEXT22, rect(20, y, 180, 33), label, STATUS_FG, M_TILE, 1, 0);
        sf::glyphs(sf::TEXT22, rect(200, y, DEVICE_VALUE_W, 33), value, M_ACCENT, M_TILE, 1, 0);
    }
}
