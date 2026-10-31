//! Replica of the stock DM40 V2.6 oscilloscope page (stock/re/docs/stock-scope-ui.md,
//! port plan ../docs/history/stock-scope-port-plan.md; host reference renderer
//! tools/stock-ui/scope.rs). Everything is drawn straight to the panel (no
//! frame buffer) by composition (stockfont::paint): each widget is one LCD
//! window whose pixels are computed from its layers, with AA off and the
//! stock corner tables, so a repaint of any part is exact and flicker-free.
//!
//! The page is described by a plain `Model` (filled by the app from the scope
//! engine and its own state, or by tools/ui-render from scene values); the
//! widgets whose inputs changed since the last draw are repainted (`update`).
//! The graph keeps, per column, the trace / persistence rows it shows and
//! repaints only the rows that changed (`Graph::flush`): a drag of the trace
//! costs a few rows per column, not the whole 399 x 279 area.
//!
//! Geometry: full page frame (13,20,401,281); the combined view moves the
//! frame to (13,76,401,225) under the meter strip (rows of the 280-row model
//! scaled), everything else as the stock.
#![allow(dead_code)]

use crate::lcd::{self, Color};
use crate::stockfont::{self as sf, in_rbox, on_ring, paint, rect, Font, Ink, Rect};

/// The protocol decoder's bus band and its setup popup (app_decode.rs).
#[path = "ui_decode.rs"]
pub mod dec;
/// The protocol decoder's frame log view (app_log.rs).
#[path = "ui_log.rs"]
pub mod log;


// ---- colours (RGB565, spec sec. 0) ----
pub const C_BAR: Color = 0x4208; // top bar, strip, panel, measurement bar
pub const C_FILL: Color = 0x2124; // graph fill, record-bar inner, dark text on yellow
pub const C_FRAME: Color = 0xad55; // frame border, rings, cursor lines, math off
pub const C_PURPLE: Color = 0xa81f; // status badge, toasts
pub const C_ORANGE: Color = 0xfaa0; // STOP badge, TRIG button, Clear
pub const C_MAGENTA: Color = 0xb833; // timebase badge
pub const C_WHITE: Color = 0xffff;
pub const C_GREEN: Color = 0x07e0; // battery bar while charging
pub const C_DIS: Color = 0x7bef; // label button while its popup is open / pressed
pub const C_POP_BG: Color = 0xd679; // popup page / selected tab
pub const C_POP_TAB: Color = 0x5acb; // unselected tab / header
pub const C_SEL: Color = 0x059e; // list selection, spinner band, slider fill
pub const C_YELLOW: Color = 0xffe0;
/// The extra channels' traces (opendm40, docs/scope-gen-channel.md): the
/// generator (computed) and the DMM's CH2.
pub const C_GEN_TR: Color = 0x07ff;
pub const C_CH2: Color = 0xf81f;
/// The strip value's digits below the accuracy (ui::strip_acc): the DMM
/// page's VALUE_LOW.
pub const C_LOW: Color = crate::ui::VALUE_LOW;
/// Palette 0x0806c730: red orange yellow green blue purple brown pink gold.
pub const PALETTE: [Color; 9] = [0xf800, 0xfd20, 0xffe0, 0x0400, 0x001f, 0x8010, 0xa145, 0xfe19, 0xfea0];
pub const COLOUR_NAMES: [&str; 9] = ["red", "orange", "yellow", "green", "blue", "purple", "brown", "pink", "gold"];
/// Text on a CH1-coloured fill (zoom buttons, CHA, V/div badge, the source
/// badge's selected half), per PALETTE entry: the stock dark text, white
/// where dark reads worse than the stock default math panel (white on
/// orange, contrast 1.97:1): blue 1.83, purple 1.72.
pub const CH1_INK: [Color; 9] = [C_FILL, C_FILL, C_FILL, C_FILL, C_WHITE, C_WHITE, C_FILL, C_FILL, C_FILL];
/// Text on a math-coloured panel button (math on), per PALETTE entry: the
/// stock white, dark where white reads worse than on the default orange:
/// yellow 1.07, pink 1.52, gold 1.41.
pub const MATH_INK: [Color; 9] = [C_WHITE, C_WHITE, C_FILL, C_WHITE, C_WHITE, C_WHITE, C_WHITE, C_FILL, C_FILL];
/// Text in the channel colour on the dark bar / graph (trigger badge,
/// measurement strip, FFT labels), per PALETTE entry: the palette colour,
/// or halfway to white where it reads worse on C_BAR than the stock
/// default math panel (1.97:1): blue 1.20, purple 1.13, brown 1.43 ->
/// 3.22, 3.60, 4.08.
pub const PAL_TEXT: [Color; 9] = [0xf800, 0xfd20, 0xffe0, 0x0400, 0x841f, 0xc418, 0xd492, 0xfe19, 0xfea0];

pub const TB_LABELS: [&str; 30] = [
    "10ns", "20ns", "50ns", "100ns", "200ns", "500ns", "1us", "2us", "5us", "10us", "20us", "50us", "100us", "200us", "500us", "1ms", "2ms",
    "5ms", "10ms", "20ms", "50ms", "100ms", "200ms", "500ms", "1s", "2s", "5s", "10s", "20s", "50s",
];
/// V/div labels 0x0807551c (index = V/div index + 3; 1mV .. 10kV).
pub const VDIV_LABELS: [&str; 22] =
    ["1m", "2m", "5m", "10m", "20m", "50m", "100m", "200m", "500m", "1", "2", "5", "10", "20", "50", "100", "200", "500", "1k", "2k", "5k", "10k"];
/// FFT dB/div labels (stock table 0x080755b0; default index 9 = "10dB").
pub const FFT_DB: [&str; 13] = ["10mdB", "20mdB", "50mdB", "100mdB", "200mdB", "500mdB", "1dB", "2dB", "5dB", "10dB", "20dB", "50dB", "100dB"];
/// Their values (dB per division).
pub const FFT_DB_VAL: [f32; 13] = [0.01, 0.02, 0.05, 0.1, 0.2, 0.5, 1.0, 2.0, 5.0, 10.0, 20.0, 50.0, 100.0];
/// Volts of the VDIV_LABELS entries.
pub const VDIV_VAL: [f32; 22] =
    [1e-3, 2e-3, 5e-3, 0.01, 0.02, 0.05, 0.1, 0.2, 0.5, 1.0, 2.0, 5.0, 10.0, 20.0, 50.0, 100.0, 200.0, 500.0, 1e3, 2e3, 5e3, 1e4];
pub const STATUS: [&str; 5] = ["WAIT", "AUTO", "T'D", "RUN", "STOP"];
pub const ST_WAIT: u8 = 0;
pub const ST_AUTO: u8 = 1;
pub const ST_TD: u8 = 2;
pub const ST_RUN: u8 = 3;
pub const ST_STOP: u8 = 4;

// ---- geometry ----
pub const TOP_BAR: Rect = rect(0, 0, 480, 20);
pub const PANEL: Rect = rect(414, 20, 66, 300);
pub const MEAS_BAR: Rect = rect(0, 301, 414, 19);
/// Combined view: graph frame top (under the 56 px meter strip at y 20).
pub const COMBINED_TOP: i16 = 76;
/// The drag / tap rect of the stock overlay (sec. 10.1).
pub const INNER: Rect = rect(0, 15, 414, 290);

/// Right-panel buttons (sec. 5), 60 x 40 at x 417.
pub const B_RECT: [Rect; 7] =
    [rect(417, 20, 60, 40), rect(417, 61, 60, 40), rect(417, 102, 60, 40), rect(417, 150, 60, 40), rect(417, 191, 60, 40), rect(417, 232, 60, 40), rect(417, 280, 60, 40)];

/// Top-bar badges (sec. 2): run status, timebase, CH1 V/div, trigger.
pub const RUN_BADGE: Rect = rect(13, 1, 62, 17);
pub const TB_BADGE: Rect = rect(79, 1, 62, 17);
pub const VDIV_BADGE: Rect = rect(286, 1, 62, 17);
pub const TRIG_BADGE: Rect = rect(352, 1, 62, 17);

// opendm40 timebase tap step (docs/scope-scale-ux.md sec. 1). The rects are
// absolute; both views share the frame's bottom edge (y 300), so they are
// the same in the combined view.
/// Value-preview tabs (left: the next slower timebase, right: the next
/// faster one), in the bottom graph corners. 52 px wide (the doc's 50 px
/// left 2 px beside the widest labels, "<500ms" is 45 px in the 12 px font).
pub const TAB_L: Rect = rect(23, 280, 52, 17);
pub const TAB_R: Rect = rect(338, 280, 52, 17);
/// "Corners" tap zones (80 x 73 px, about 13.6 x 12.4 mm); the left one
/// starts after the strip markers' press zones (x <= 22). The right one
/// reaches the frame; the right-edge trigger-level marker's zone wins over
/// it where they meet (marker priority), and its tab (`TAB_R`) ends before
/// that marker's column (x 401..412).
pub const ZONE_L: Rect = rect(23, 228, 80, 73);
pub const ZONE_R: Rect = rect(333, 228, 80, 73);
/// `Model::tabs` bits.
pub const TAB_SHOW_L: u8 = 1;
pub const TAB_SHOW_R: u8 = 2;
pub const TAB_PRESS_L: u8 = 4;
pub const TAB_PRESS_R: u8 = 8;
/// Settings "Scope tap step": stock halves (left half slower, right half
/// faster), the corner zones, or no tap step at all (badge list + pinch).
pub const TAP_HALVES: u8 = 0;
pub const TAP_CORNERS: u8 = 1;
pub const TAP_OFF: u8 = 2;
/// The default tap-step mode (the owner's corner zones). Persisted XOR this
/// value, so a record without the field restores it.
pub const TAP_DEFAULT: u8 = TAP_CORNERS;
pub const TAP_NAMES: [&str; 3] = ["Halves (stock)", "Corners", "Off"];

/// Persisted tap-step code (2 bits): the mode XOR TAP_DEFAULT, so a record
/// written before the setting existed (0) restores the default.
pub fn tap_code(mode: u8) -> u32 {
    ((mode ^ TAP_DEFAULT) & 3) as u32
}

/// Tap-step mode of a persisted code (an unknown code: the default).
pub fn tap_decode(code: u32) -> u8 {
    let m = (code & 3) as u8 ^ TAP_DEFAULT;
    if m > TAP_OFF { TAP_DEFAULT } else { m }
}
/// Corners: holding a zone still steps again after REPEAT_FIRST_MS, then
/// every REPEAT_MS.
pub const REPEAT_FIRST_MS: u64 = 500;
pub const REPEAT_MS: u64 = 200;

/// Everything the page shows.
#[derive(Clone, Copy, PartialEq)]
pub struct Model {
    /// ST_WAIT .. ST_STOP.
    pub status: u8,
    /// Timebase index 0..29 (TB_LABELS).
    pub tb: u8,
    /// V/div label index 0..21 (VDIV_LABELS; the probe-scaled V/div).
    pub vdiv: u8,
    /// Probe index 0..12 (0.1X .. 1000X; 3 = 1X).
    pub probe: u8,
    pub ac: bool,
    /// Trigger level row (-10..290 of the 280-row model).
    pub trig_row: i16,
    /// Trigger level in volts (badge text).
    pub trig_v: f32,
    pub rising: bool,
    /// Trigger time column (0..399; outside = off-screen variants).
    pub trig_px: i16,
    /// CH1 position rows (0 = centre).
    pub pos: i16,
    /// Palette index of CH1 (default 2 = yellow).
    pub colour: u8,
    /// Trace brightness / grid intensity, percent.
    pub bright: u8,
    pub grid: u8,
    /// Selection: level item 0 CH1 / 1 trigger level / 3, 4 Y cursors; time
    /// item 0 trigger position / 2, 3 X cursors.
    pub sel_v: u8,
    pub sel_h: u8,
    /// Guide lines of the selection shown (press .. 2 s after the last event).
    pub guide: bool,
    /// X cursor columns (1..399) / Y cursor rows (1..279) when shown.
    pub xcur: Option<[i16; 2]>,
    pub ycur: Option<[i16; 2]>,
    /// Battery level 0..5 (blinking while charging), on a charger (green).
    pub battery: u8,
    pub charging: bool,
    /// A USB host has the meter configured: the small USB symbol between
    /// the trigger badge and the battery (`usb_icon`).
    pub usb: bool,
    /// A Bluetooth host is connected: the Bluetooth rune in the same gap
    /// (`usb_icon`; with USB too, left of it, the battery 3 px right).
    pub bt: bool,
    /// Roll mode (record bar "ROLL").
    pub roll: bool,
    /// Record bar window (start, end) as fractions of the record.
    pub record: (f32, f32),
    /// Generator output frequency, shown in the record bar while running
    /// (<= 0: off).
    pub gen_hz: f32,
    /// CH1 / math label grey (their popup is open).
    pub ch1_open: bool,
    pub math_open: bool,
    /// Right-panel button 1..7 drawn pressed (0 none; our feedback option).
    pub pressed: u8,
    /// Combined view geometry.
    pub combined: bool,
    /// Math mode of the math pill label (stock S+0xd4: 0 "--" .. 5 FFT, 6
    /// ABS; shown even while math is off). opendm40 has no math engine: 0.
    pub math_mode: u8,
    /// opendm40 timebase tabs (docs/scope-scale-ux.md sec. 1): TAB_SHOW_L /
    /// TAB_SHOW_R shown, TAB_PRESS_L / TAB_PRESS_R in pressed style
    /// (`tabs_of`); 0 = none (the stock page).
    pub tabs: u8,
    /// Timebase badge drawn inverted (150 ms after a timebase change).
    pub tb_flash: bool,
    /// Math display on (stock S+0x10b): the math pill in the math colour,
    /// the math trace (`Graph::ma`), the 'M' strip marker; in FFT mode
    /// (math_mode 5) the 'M' top marker and the centre-frequency label.
    pub math_on: bool,
    /// Palette index of the math trace (default 1 = orange).
    pub math_colour: u8,
    /// Math / FFT offset rows: the 'M' strip marker at row 140 + offset
    /// (FFT: the 0 dBV row).
    pub math_off: i16,
    /// Math pill second line ("10dB", "1V"); empty: the stock defaults
    /// ("10dB" in FFT mode, else the CH1 V/div text).
    pub math_scale: Txt,
    /// FFT centre-frequency label text (stock, FFT mode).
    pub fft_hz: Txt,
    /// opendm40 FFT additions (UF-11): axis scale boxes, the peak readout and
    /// marker. None = the stock look only.
    pub fft: Option<FftAdd>,
    /// Runt trigger: the second level row (orange T strip marker, level
    /// item 5), None for the other trigger types.
    pub runt_row: Option<i16>,
    /// opendm40 cursor tab (UF-08): 0 hidden, else CUR_OFF / CUR_X / CUR_Y.
    pub cur_tab: u8,
    /// Source badge (opendm40, the DMM wave view, src/app_wave.rs): SRC_NONE
    /// (the stock page: no badge, builds without the AD2 stream ISR),
    /// SRC_MCX or SRC_DMM (the active half filled).
    pub src: u8,
    /// Source DMM: the unit of the vertical values is A (current rows).
    pub amps: bool,
    /// The V/div value (SI, `fmt_div`: "1.5V", "200uA", "2.5V" with a x5
    /// probe) and, source DMM, the CH1 label ("AC mA"); 0 / "" = the stock
    /// texts from `vdiv` / "CHA".
    pub wvdiv: f32,
    pub ch1_txt: &'static str,
    /// A CSV recording runs (csvlog.rs; the Scope page and the combined
    /// view): the REC mark (`rec_mark`) in the source badge's slot, the
    /// record bar beside it. The Wave view (SRC_DMM) and a recording
    /// exclude each other (App::wave_open, App::rec_conflict).
    pub rec: bool,
    /// The REC mark's blink phase (`rec_blink`): true = its dot hidden. A
    /// change repaints the slot only.
    pub rec_dim: bool,
    /// The recorded function's unit (`rec_unit`) for RecStyle::RecFunc.
    pub rec_unit: &'static str,
    /// Protocol decoder band (ui_decode.rs): 0 none, 1 UART (the byte
    /// row), 2 Modbus (frame and byte rows); `dec_gen` changes with what
    /// it shows (`dec::ann`), 0 = blank (a drag moves the trace).
    pub dec: u8,
    pub dec_gen: u16,
    /// Trigger source (scope::TSRC_*): the trigger badge in the GEN
    /// colour for the generator.
    pub tsrc: u8,
    /// The DMM CH2's label in the graph ("CH2 1.5V", magenta); empty =
    /// none (`CH2_LABEL`, a graph hole).
    pub ch2_txt: Txt,
}

/// `Model::src` values.
pub const SRC_NONE: u8 = 0;
pub const SRC_MCX: u8 = 1;
pub const SRC_DMM: u8 = 2;
/// The source badge or REC (left part of the record-bar slot when shown) and
/// the record bar then (opendm40: the stock bar is 145..282).
pub const SRC_BADGE: Rect = rect(145, 1, 64, 17);
const REC_BAR: Rect = rect(145, 1, 137, 17);
const REC_BAR_SRC: Rect = rect(212, 1, 70, 17);

/// Cursor tab states (`Model::cur_tab`).
pub const CUR_OFF: u8 = 1;
pub const CUR_X: u8 = 2;
pub const CUR_Y: u8 = 3;
/// The cursor tab: bottom row of the graph, between the left Corners tap
/// zone (x <= 102) and the FFT scale box (x >= 168).
pub const CUR_TAB: Rect = rect(108, 280, 54, 17);

/// opendm40 FFT additions (UF-11): the vertical scale ("10dB/div"), the
/// horizontal scale ("50kHz/div"), the automatic peak readout and the peak
/// position (graph column 0..398, 280-row model row) for its marker.
#[derive(Clone, Copy, PartialEq)]
pub struct FftAdd {
    pub ydiv: Txt,
    pub xdiv: Txt,
    pub peak: Txt,
    pub peak_at: Option<(i16, i16)>,
}

impl Model {
    pub const DEFAULT: Model = Model {
        status: ST_TD,
        tb: 13,
        vdiv: 9,
        probe: 3,
        ac: false,
        trig_row: 105,
        trig_v: 0.993,
        rising: true,
        trig_px: 200,
        pos: 0,
        colour: 2,
        bright: 100,
        grid: 50,
        sel_v: 0,
        sel_h: 0,
        guide: false,
        xcur: None,
        ycur: None,
        battery: 4,
        charging: false,
        usb: false,
        bt: false,
        roll: false,
        record: (0.0, 1.0),
        gen_hz: 0.0,
        ch1_open: false,
        math_open: false,
        pressed: 0,
        combined: false,
        math_mode: 0,
        tabs: 0,
        tb_flash: false,
        math_on: false,
        math_colour: 1,
        math_off: 0,
        math_scale: Txt::EMPTY,
        fft_hz: Txt::EMPTY,
        fft: None,
        runt_row: None,
        cur_tab: 0,
        src: SRC_NONE,
        amps: false,
        wvdiv: 0.0,
        ch1_txt: "",
        rec: false,
        rec_dim: false,
        rec_unit: "V",
        dec: 0,
        dec_gen: 0,
        tsrc: 0,
        ch2_txt: Txt::EMPTY,
    };

    pub fn pal(&self) -> Color {
        PALETTE[self.colour as usize % 9]
    }
    /// Trace colour: palette blended into the fill by the brightness.
    pub fn trace_colour(&self) -> Color {
        blend565(self.pal(), C_FILL, (self.bright.min(100) as u32 * 255 / 100) as u8)
    }
    /// Persistence: the same at half alpha.
    pub fn half_colour(&self) -> Color {
        blend565(self.pal(), C_FILL, ((self.bright.min(100) as u32 * 255 / 100) >> 1) as u8)
    }
    /// Math colour: the palette entry (unblended, stock), grey while math
    /// is off (the pill).
    pub fn math_pal(&self) -> Color {
        PALETTE[self.math_colour as usize % 9]
    }
    /// Text colour on `pal()` (CH1_INK).
    pub fn pal_ink(&self) -> Color {
        CH1_INK[self.colour as usize % 9]
    }
    /// Text colour on `math_pal()` (MATH_INK).
    pub fn math_ink(&self) -> Color {
        MATH_INK[self.math_colour as usize % 9]
    }
    /// CH1-coloured text on the dark bar / graph (PAL_TEXT).
    pub fn pal_text(&self) -> Color {
        PAL_TEXT[self.colour as usize % 9]
    }
    /// Math-coloured text on the graph (PAL_TEXT).
    pub fn math_text(&self) -> Color {
        PAL_TEXT[self.math_colour as usize % 9]
    }
    pub fn grid_colour(&self) -> Color {
        blend565(0xffff, C_FILL, (self.grid.min(100) as u32 * 255 / 100) as u8)
    }
    /// Graph frame top / height.
    pub fn top(&self) -> i16 {
        if self.combined { COMBINED_TOP } else { 20 }
    }
    pub fn frame_h(&self) -> i16 {
        301 - self.top()
    }
    /// Coupling / probe glyph of font 0x08054fc0.
    pub fn probe_glyph(&self) -> u32 {
        (if self.ac { 0x2c } else { 0x2d }) + 2 * self.probe as u32
    }
    /// The V/div text (`vtxt`, else the stock label).
    pub fn vdiv_str(&self, out: &mut Buf) {
        if self.wvdiv > 0.0 {
            fmt_div(out, self.wvdiv, self.unit());
        } else {
            vdiv_text(out, self.vdiv);
        }
    }
    /// Unit of the vertical values ("V", source DMM current rows "A").
    pub fn unit(&self) -> &'static str {
        if self.amps { "A" } else { "V" }
    }
}

/// FUN_08019038: `fg` over `bg` with alpha 0..255 (8-bit channel expansion,
/// 16-bit truncation as the stock).
pub fn blend565(fg: Color, bg: Color, a: u8) -> Color {
    if a > 252 {
        return fg;
    }
    if a < 3 {
        return bg;
    }
    let (a, na) = (a as u32, 255 - a as u32);
    let b = ((((fg as u32 & 0x1f) << 3 | 7) * a + ((bg as u32 & 0x1f) << 3 | 7) * na) & 0xffff) >> 11;
    let g = (((((fg as u32 >> 3) & 0xfc) | 3) * a + (((bg as u32 >> 3) & 0xfc) | 3) * na) & 0xffff) >> 10;
    let r = ((((fg as u32 >> 8) & 0xf8) | 7) * a + (((bg as u32 >> 8) & 0xf8) | 7) * na) & 0xf800;
    (r | g << 5 | b) as Color
}

// ------------------------------------------------------------------ text

/// Small text buffer.
pub struct Buf {
    b: [u8; 24],
    n: usize,
}

impl Buf {
    pub const fn new() -> Buf {
        Buf { b: [0; 24], n: 0 }
    }
    pub fn push(&mut self, s: &str) -> &mut Buf {
        let k = s.len().min(24 - self.n);
        self.b[self.n..self.n + k].copy_from_slice(&s.as_bytes()[..k]);
        self.n += k;
        self
    }
    pub fn as_str(&self) -> &str {
        core::str::from_utf8(&self.b[..self.n]).unwrap_or("")
    }
}

/// FUN_08032bfa (V / A / W): NaN " -- "; |v| < 1e-6 "%1.2fu"; else scaled
/// into [1, 1000) over u m - k M with "%1.2f" (< 9.995), "%2.1f" (< 99.95)
/// or "%3.0f".
pub fn fmt_v(out: &mut Buf, v: f32, unit: &str) {
    fmt_v_cut(out, v, unit, 0)
}

/// `fmt_v` with `cut` decimals fewer (a box too narrow for the full text).
fn fmt_v_cut(out: &mut Buf, v: f32, unit: &str, cut: u8) {
    if !v.is_finite() {
        out.push(" -- ");
        return;
    }
    let mut t = [0u8; 16];
    if v.abs() < 1e-6 {
        out.push(crate::dmm::format(&mut t, v * 1e6, 2)).push("u").push(unit);
        return;
    }
    const P: [&str; 5] = ["u", "m", "", "k", "M"];
    let (mut x, mut i) = (v, 2usize);
    for _ in 0..5 {
        if x.abs() >= 1000.0 && i < 4 {
            x /= 1000.0;
            i += 1;
        } else if x.abs() < 1.0 && i > 0 {
            x *= 1000.0;
            i -= 1;
        } else {
            break;
        }
    }
    if !(1.0..1000.0).contains(&x.abs()) {
        out.push(" --");
        return;
    }
    let dec = if x.abs() < 9.995 { 2u8 } else if x.abs() < 99.95 { 1 } else { 0 };
    out.push(crate::dmm::format(&mut t, x, dec.saturating_sub(cut))).push(P[i]).push(unit);
}

/// A per-division value with its prefix and unit ("2V", "200mV", "200uA",
/// "5A", the full-scale steps "1.5V", "2.5A"; the DMM wave view's V/div).
pub fn fmt_div(out: &mut Buf, v: f32, unit: &str) {
    const P: [&str; 5] = ["u", "m", "", "k", "M"];
    let (mut x, mut i) = (v * 1.000_01, 2usize);
    while x >= 1000.0 && i < 4 {
        x /= 1000.0;
        i += 1;
    }
    while x < 1.0 && i > 0 {
        x *= 1000.0;
        i -= 1;
    }
    // One decimal for a fractional step below 10 (the wave view's
    // full-scale steps: "1.5V", "2.5A"); 15, 150, 250 stay whole.
    let frac = x < 10.0 && x - (x as u32) as f32 >= 0.05;
    let mut t = [0u8; 16];
    let v = if frac { (x * 10.0) as u32 as f32 / 10.0 } else { (x as u32) as f32 };
    out.push(crate::dmm::format(&mut t, v, frac as u8)).push(P[i]).push(unit);
}

/// "1V", "200mV": the V/div label and unit.
pub fn vdiv_text(out: &mut Buf, vdiv: u8) {
    out.push(VDIV_LABELS[vdiv.min(21) as usize]).push("V");
}

// ------------------------------------------------------------------ widgets

const K_TEXT: u8 = 0;
const K_SHAPE: u8 = 1;
const K_ICON: u8 = 2;

/// Layer tag for `paint`: layer (1-based) and glyph index.
#[inline]
fn tag(layer: u16, g: u8) -> u16 {
    layer << 8 | g as u16
}

/// A badge: rounded box `r` (radius 5, all corners) in `fill` over `outside`,
/// with up to three inks drawn in their colours on top.
fn badge(r: Rect, fill: Color, outside: Color, inner: Option<(Rect, Color)>, inks: &[(&Ink, Color)]) {
    let layers = [(K_SHAPE, outside), (K_SHAPE, fill), (K_TEXT, fill), (K_TEXT, fill), (K_TEXT, fill)];
    paint(r, &layers[..2 + inks.len()], |x, y| {
        for (k, (ink, c)) in inks.iter().enumerate() {
            if let Some(g) = ink.at(x, y) {
                return (*c, tag(3 + k as u16, g));
            }
        }
        if let Some((ir, ic)) = inner {
            if in_rbox(ir, 5, 0x0f, x, y) {
                return (ic, tag(2, 0));
            }
        }
        if in_rbox(r, 5, 0x0f, x, y) { (fill, tag(1, 0)) } else { (outside, 0) }
    });
}

/// 2.1 run-status badge.
pub fn status_badge(m: &Model) {
    let r = RUN_BADGE;
    let t = Ink::new(sf::SC_C, r, STATUS[m.status.min(4) as usize], 8, 0);
    badge(r, if m.status == ST_STOP { C_ORANGE } else { C_PURPLE }, C_BAR, None, &[(&t, C_WHITE)]);
}

/// 2.2 timebase badge; `tb_flash`: inverted (white box, magenta text), the
/// opendm40 feedback of a timebase change.
pub fn tb_badge(m: &Model) {
    let h = Ink::new(sf::SC_C, rect(83, 1, 12, 17), "H", 1, 0);
    let t = Ink::new(sf::SC_F12, rect(95, 1, 46, 17), TB_LABELS[m.tb.min(29) as usize], 8, 0);
    let (fill, fg) = if m.tb_flash { (C_WHITE, C_MAGENTA) } else { (C_MAGENTA, C_WHITE) };
    badge(TB_BADGE, fill, C_BAR, None, &[(&h, fg), (&t, fg)]);
}

/// 2.3 record-position bar; while running (record = screen) it shows the
/// generator frequency instead of the constant full-record line when the
/// generator is on (opendm40 addition, 12 px grey like "ROLL").
/// The record bar's box (shorter beside the source badge or REC).
pub fn record_bar_rect(m: &Model) -> Rect {
    if m.src == SRC_NONE && !m.rec { REC_BAR } else { REC_BAR_SRC }
}

pub fn record_bar(m: &Model) {
    let r = record_bar_rect(m);
    let inner = rect(r.x + 1, 2, r.w - 2, 15);
    // Bar scale (px of the full record) and its left inset: the stock 129 / 4.
    let (sw, x0s) = (r.w as f32 - 8.0, r.x);
    let running = m.status != ST_STOP;
    let mut b = Buf::new();
    let (txt, line, lb, rb);
    if m.roll {
        txt = Ink::new(sf::SC_F12, inner, "ROLL", 8, 0);
        (line, lb, rb) = (None, Ink::none(), Ink::none());
    } else if running && m.gen_hz > 0.0 {
        b.push("G ");
        fmt_hz(&mut b, m.gen_hz);
        txt = Ink::new(sf::SC_F12, inner, b.as_str(), 8, 0);
        (line, lb, rb) = (None, Ink::none(), Ink::none());
    } else {
        txt = Ink::none();
        let (start, end) = if running { (0.0, 1.0) } else { m.record };
        let x0 = if start < 0.0 { 4 } else { (start * sw + 4.0) as i16 };
        let len = (((end.min(1.0) - start.max(0.0)) * sw) as i16).max(0);
        line = Some((x0s + x0, len));
        // The stock brackets: 4 px boxes that cut the 6 px glyphs (the
        // stock geometry, stock-diff exact; exempt from ui-render's clip
        // rule).
        #[cfg(test)]
        sf::fit_exempt(true);
        lb = if (0.0..1.0).contains(&start) { Ink::new(sf::SC_F12, rect(x0s + x0 - 2, 2, 4, 17), "[", 1, 0) } else { Ink::none() };
        rb = if end <= 1.0 { Ink::new(sf::SC_F12, rect(x0s + (end * sw + 4.0) as i16 - 4, 2, 4, 17), "]", 1, 0) } else { Ink::none() };
        #[cfg(test)]
        sf::fit_exempt(false);
    }
    let layers = [(K_SHAPE, C_BAR), (K_SHAPE, C_FRAME), (K_TEXT, C_FILL), (K_SHAPE, C_FILL), (K_ICON, C_FILL), (K_ICON, C_FILL)];
    paint(r, &layers, |x, y| {
        if let Some(g) = txt.at(x, y) {
            return (C_FRAME, tag(3, g));
        }
        if let Some(g) = lb.at(x, y) {
            return (C_FRAME, tag(5, g));
        }
        if let Some(g) = rb.at(x, y) {
            return (C_FRAME, tag(6, g));
        }
        if let Some((lx, ln)) = line {
            if y == 9 && x >= lx && x < lx + ln {
                return (C_YELLOW, tag(4, 0));
            }
        }
        if in_rbox(inner, 5, 0x0f, x, y) {
            (C_FILL, tag(2, 0))
        } else if in_rbox(r, 5, 0x0f, x, y) {
            (C_FRAME, tag(1, 0))
        } else {
            (C_BAR, 0)
        }
    });
}

/// The REC mark's blink (`now` in ms): true = off; on 1 s, off 1 s (owner
/// 2026-10-30: 0.5 s was too fast). One phase for every page (the scope
/// bar, the DMM and generator status bars).
pub fn rec_blink(now: u64) -> bool {
    now / 1000 & 1 == 1
}

/// Texts of the REC mark (owner feedback on #161, 2026-10-30: say that the
/// recording is the multimeter's): `Rec` "REC", `Dmm` "DMM", `RecDmm`
/// "REC DMM", `RecFunc` "REC" + the recorded function's unit (`rec_unit`),
/// `DmmRec` "DMM REC"; always after the blinking red dot. ui-render draws
/// all of them (`set_rec_style`).
#[derive(Clone, Copy, PartialEq)]
pub enum RecStyle {
    Rec,
    Dmm,
    RecDmm,
    RecFunc,
    DmmRec,
}

#[cfg(not(test))]
fn rec_style() -> RecStyle {
    RecStyle::Dmm
}
#[cfg(test)]
static mut REC_STYLE: RecStyle = RecStyle::Dmm;
#[cfg(test)]
fn rec_style() -> RecStyle {
    unsafe { REC_STYLE }
}
#[cfg(test)]
pub fn set_rec_style(s: RecStyle) {
    unsafe { REC_STYLE = s }
}

/// The unit of a recorded DMM function (meter::func_of codes, csvlog::
/// func_label) for `RecStyle::RecFunc`: V, A, Ω, F, Hz, W, °C.
pub fn rec_unit(func: u8) -> &'static str {
    match func {
        0..=3 | 12 | 16 => "V",
        4..=9 | 17 => "A",
        10 | 11 => "\u{3a9}",
        13 => "F",
        14 => "Hz",
        18 => "W",
        _ => "\u{b0}C",
    }
}

/// The REC dot's diameter: the 12 px font's cap height, smaller when the
/// text needs the room (`rec_layout`).
const REC_DOT: i16 = 9;

/// The REC mark's parts in the slot `r`: the dot's box and the words (one
/// Ink each: a gap narrower than the font's 6 px space). The largest of
/// dot 9 / gap 4 / word gap 4 .. dot 5 / gap 2 / word gap 3 that leaves
/// >= 2 px inside the box's ring; the group centred.
fn rec_layout(r: Rect, unit: &'static str) -> (Rect, [Ink; 2]) {
    let words: [&str; 2] = match rec_style() {
        RecStyle::Rec => ["REC", ""],
        RecStyle::Dmm => ["DMM", ""],
        RecStyle::RecDmm => ["REC", "DMM"],
        RecStyle::RecFunc => ["REC", unit],
        RecStyle::DmmRec => ["DMM", "REC"],
    };
    let w = |t: &str| if t.is_empty() { 0 } else { sf::text_width(sf::SC_F12, t, 0) };
    let (w0, w1) = (w(words[0]), w(words[1]));
    let len = |l: (i16, i16, i16)| l.0 + l.1 + w0 + if w1 > 0 { l.2 + w1 } else { 0 };
    let lay = [(REC_DOT, 4, 4), (7, 3, 4), (7, 2, 3), (5, 2, 3)].into_iter().find(|&l| len(l) <= r.w - 6).unwrap_or((5, 2, 3));
    let (d, g, wg) = lay;
    let x0 = r.x + (r.w - len(lay) + 1) / 2;
    let dot = rect(x0, r.y + 4 + (REC_DOT - d) / 2, d, d);
    let tx = x0 + d + g;
    let t0 = Ink::new(sf::SC_F12, rect(tx, r.y, w0, r.h), words[0], 8, 0);
    let t1 = if w1 > 0 { Ink::new(sf::SC_F12, rect(tx + w0 + wg, r.y, w1, r.h), words[1], 8, 0) } else { Ink::none() };
    (dot, [t0, t1])
}

/// The REC mark in the 17 px slot `r` over `outside` (the top bar), boxed
/// like the neighbouring badges: a red dot and white text (`RecStyle`) on
/// the rows of the badges' 12 px caps (r.y + 4 ..= r.y + 12). `on` = the
/// blink's on phase (the dot shows); `unit` = the recorded function's
/// (`rec_unit`, RecFunc).
pub fn rec_mark(r: Rect, outside: Color, on: bool, unit: &'static str) {
    rec_paint(r, outside, on, false, unit);
}

/// The REC mark's blink step: only the dot's box.
pub fn rec_mark_blink(r: Rect, outside: Color, on: bool, unit: &'static str) {
    rec_paint(r, outside, on, true, unit);
}

/// The REC dot alone in its box `dot` over `bg` (the DMM and generator
/// status bars, ui::status_rec), hidden when not `on`.
pub fn rec_dot(dot: Rect, bg: Color, on: bool) {
    paint(dot, &[(K_ICON, bg)], |x, y| if on && in_rbox(dot, dot.w / 2, 0x0f, x, y) { (crate::ui::REC_CSV, tag(1, 0)) } else { (bg, 0) });
}

fn rec_paint(r: Rect, outside: Color, on: bool, dot_only: bool, unit: &'static str) {
    let red = crate::ui::REC_CSV;
    let (dot, [t0, t1]) = rec_layout(r, unit);
    let win = if dot_only { dot } else { r };
    paint(win, &[(K_SHAPE, outside), (K_SHAPE, C_FILL), (K_ICON, C_FILL), (K_TEXT, C_FILL), (K_TEXT, C_FILL)], |x, y| {
        if let Some(g) = t0.at(x, y) {
            return (C_WHITE, tag(4, g));
        }
        if let Some(g) = t1.at(x, y) {
            return (C_WHITE, tag(5, g));
        }
        if on && in_rbox(dot, dot.w / 2, 0x0f, x, y) {
            return (red, tag(3, 0));
        }
        if in_rbox(r, 5, 0x0f, x, y) {
            if on_ring(r, 5, 0x0f, x, y) {
                (C_FRAME, tag(1, 0))
            } else {
                (C_FILL, tag(2, 0))
            }
        } else {
            (outside, 0)
        }
    });
}

/// Source badge (`Model::src` != SRC_NONE): "MCX | DMM", the active half
/// in the CH1 colour with dark text, the other dark with grey text. While a
/// CSV recording runs (`Model::rec`) the slot shows the REC mark instead
/// (`rec_mark`; the DMM source is refused then).
pub fn src_badge(m: &Model) {
    let r = SRC_BADGE;
    if m.rec {
        rec_mark(r, C_BAR, !m.rec_dim, m.rec_unit);
        return;
    }
    if m.src == SRC_NONE {
        return;
    }
    let half = r.w / 2;
    let a = Ink::new(sf::SC_F12, rect(r.x, 1, half, 17), "MCX", 8, 0);
    let b = Ink::new(sf::SC_F12, rect(r.x + half, 1, r.w - half, 17), "DMM", 8, 0);
    let dmm = m.src == SRC_DMM;
    let on = m.pal();
    let (fa, fb) = if dmm { (C_FILL, on) } else { (on, C_FILL) };
    let (ta, tb) = if dmm { (C_FRAME, m.pal_ink()) } else { (m.pal_ink(), C_FRAME) };
    paint(r, &[(K_SHAPE, C_BAR), (K_SHAPE, fa), (K_SHAPE, fb), (K_TEXT, fa), (K_TEXT, fb)], |x, y| {
        let left = x < r.x + half;
        if let Some(g) = a.at(x, y) {
            return (ta, tag(4, g));
        }
        if let Some(g) = b.at(x, y) {
            return (tb, tag(5, g));
        }
        if in_rbox(r, 5, 0x0f, x, y) {
            if on_ring(r, 5, 0x0f, x, y) || x == r.x + half {
                (C_FRAME, tag(1, 0))
            } else if left {
                (fa, tag(2, 0))
            } else {
                (fb, tag(3, 0))
            }
        } else {
            (C_BAR, 0)
        }
    });
}

/// "1.00kHz" (FUN_08032fa4 style, Hz .. MHz).
pub fn fmt_hz(out: &mut Buf, hz: f32) {
    let (x, p) = if hz >= 1e6 {
        (hz / 1e6, "MHz")
    } else if hz >= 1e3 {
        (hz / 1e3, "kHz")
    } else if hz >= 1.0 {
        (hz, "Hz")
    } else {
        (hz * 1e3, "mHz")
    };
    let dec = if x < 9.995 { 2 } else if x < 99.95 { 1 } else { 0 };
    let mut t = [0u8; 16];
    out.push(crate::dmm::format(&mut t, x * 1.000_001, dec)).push(p);
}

/// Sample rate as `fmt_hz` gives a frequency, "S/s" for "Hz": "50.0MS/s",
/// "12.5MS/s", "76.8kS/s", "500S/s".
pub fn fmt_srate(out: &mut Buf, fs: u32) {
    fmt_hz(out, fs as f32);
    out.n -= 2;
    out.push("S/s");
}

/// 2.4 CH1 V/div badge.
pub fn vdiv_badge(m: &Model) {
    let mut b = Buf::new();
    m.vdiv_str(&mut b);
    let p = Ink::icon(sf::SC_PROBE, rect(286, 1, 18, 17), m.probe_glyph(), 4);
    let t = Ink::new(sf::SC_F12, rect(304, 1, 44, 17), b.as_str(), 8, 0);
    badge(rect(286, 1, 62, 17), m.pal(), C_BAR, None, &[(&p, m.pal_ink()), (&t, m.pal_ink())]);
}

/// 2.5 trigger badge: edge glyph and level.
pub fn trig_badge(m: &Model) {
    let mut b = Buf::new();
    fmt_v(&mut b, m.trig_v, m.unit());
    // The wave view's "-1.23mA" is wider than the stock box: a decimal
    // fewer ("-1.2mA").
    if sf::text_width(sf::SC_F12, b.as_str(), 0) > 49 {
        b = Buf::new();
        fmt_v_cut(&mut b, m.trig_v, m.unit(), 1);
    }
    let e = Ink::icon(sf::SC_EDGE, rect(352, 1, 12, 17), if m.rising { 0x20 } else { 0x21 }, 4);
    let t = Ink::new(sf::SC_F12, rect(364, 1, 50, 17), b.as_str(), 8, 0);
    let c = if m.tsrc == crate::scope::TSRC_GEN { C_GEN_TR } else { m.pal_text() };
    badge(rect(352, 1, 62, 17), C_FRAME, C_BAR, Some((rect(353, 2, 60, 15), C_BAR)), &[(&e, c), (&t, c)]);
}

/// The battery's shift to the right (px) while both the Bluetooth rune and
/// the USB symbol need the gap before it (`usb_icon`).
fn icons_dx(m: &Model) -> i16 {
    if m.bt && m.usb { 3 } else { 0 }
}

/// 2.6 battery (outline + level clipped to (437,3,33,12)); `icons_dx` px
/// further right with both link icons. Painted over both places.
pub fn battery(m: &Model) {
    battery_lv(m.battery, m.charging, icons_dx(m));
}

/// The battery of level 0..5, `dx` px right of its stock place.
pub fn battery_lv(level: u8, charging: bool, dx: i16) {
    let r = rect(435 + dx, 1, 40, 16);
    let o = Ink::icon(sf::SC_BATT, r, 0x22, 0);
    let l = if level > 0 { Ink::icon(sf::SC_BATT, r, 0x22 + level.min(5) as u32, 0) } else { Ink::none() };
    let clip = rect(437 + dx, 3, 33, 12);
    let lc = if charging { C_GREEN } else { C_WHITE };
    paint(rect(435, 1, 43, 16), &[(K_ICON, C_BAR), (K_ICON, C_BAR)], |x, y| {
        if clip.has(x, y) {
            if let Some(g) = l.at(x, y) {
                return (lc, tag(2, g));
            }
        }
        match o.at(x, y) {
            Some(g) => (C_WHITE, tag(1, g)),
            None => (C_BAR, 0),
        }
    });
}

/// The USB host symbol (SC_BATT 0x30, 11 x 18 1 bpp, tools/mkusbglyph.py)
/// centred in the gap between the trigger badge (..413) and the battery
/// (435..): ink x 419..429, y 1..18 (the bar is y 0..19; top on the
/// battery's and the badges' top row). Top-centre alignment: the glyph's
/// yoff -2 puts its top on the rect's.
pub const USB_ICON: Rect = rect(414, 1, 21, 18);

/// The link icons in that gap: the USB symbol, the Bluetooth rune (SC_BATT
/// 0x31, 7 x 13, ink y 3..15), each centred alone; both: the rune at x
/// 416..422, the symbol at 425..435, the battery moved 3 px right
/// (`icons_dx`), 2 px between neighbours.
pub fn usb_icon(m: &Model) {
    let dx = icons_dx(m);
    let r = rect(USB_ICON.x, USB_ICON.y, USB_ICON.w + dx, USB_ICON.h);
    // One Ink (stack: the top bar is on the deepest draw path).
    let u = match (m.usb, m.bt) {
        (true, true) => Ink::new(sf::SC_BATT, rect(416, 1, 20, 18), "\u{31}\u{30}", 0, 2),
        (true, false) => Ink::icon(sf::SC_BATT, r, 0x30, 6),
        (false, true) => Ink::icon(sf::SC_BATT, r, 0x31, 6),
        (false, false) => Ink::none(),
    };
    paint(r, &[(K_ICON, C_BAR)], |x, y| match u.at(x, y) {
        Some(g) => (C_WHITE, tag(1, g)),
        None => (C_BAR, 0),
    });
}

/// Right-panel button k (0..6) as the stock draws it (sec. 5).
pub fn panel_button(m: &Model, k: usize) {
    let r = B_RECT[k];
    // A strip pass (lcd.rs "strip rendering") without its rows: no layout.
    if !lcd::rows_hit(r.y, r.y + r.h - 1) {
        return;
    }
    let pal = m.pal();
    let mc = if m.math_on { m.math_pal() } else { C_FRAME };
    let (ink, math_ink) = (m.pal_ink(), if m.math_on { m.math_ink() } else { C_WHITE });
    let mut b = Buf::new();
    m.vdiv_str(&mut b);
    let pressed = m.pressed as usize == k + 1;
    let (fill, radius, mask, fg) = match k {
        0 => (pal, 8, 0x3, ink),
        1 => (if m.ch1_open { C_DIS } else { pal }, 0, 0, if m.ch1_open { C_FILL } else { ink }),
        2 => (pal, 8, 0xc, ink),
        3 => (mc, 8, 0x3, math_ink),
        4 => (if m.math_open { C_DIS } else { mc }, 0, 0, if m.math_open { C_WHITE } else { math_ink }),
        5 => (mc, 8, 0xc, math_ink),
        _ => (C_ORANGE, 8, 0xf, C_WHITE),
    };
    let fill = if pressed { C_DIS } else { fill };
    // UF-10: the CH1 V/div buttons as magnifiers (top = zoom in = smaller
    // V/div, bottom = zoom out), drawn as shapes instead of "+" / "-".
    let mag = match k {
        0 => Some(true),
        2 => Some(false),
        _ => None,
    };
    let (t1, t2) = match k {
        0 | 2 => (Ink::none(), Ink::none()),
        3 => (Ink::new(sf::SC_F20, r, "+", 8, 0), Ink::none()),
        5 => (Ink::new(sf::SC_F20, r, "-", 8, 0), Ink::none()),
        1 => {
            let (f, t) = if m.ch1_txt.is_empty() { (sf::SC_F20, "CHA") } else { (sf::SC_F16, m.ch1_txt) };
            (Ink::new(f, rect(417, 61, 60, 24), t, 8, 0), Ink::new(sf::SC_F12, rect(417, 85, 60, 16), b.as_str(), 8, 0))
        }
        4 => {
            // FFT: the dB/div label (FFT_DB), else the math V/div; an empty
            // math_scale shows the stock defaults.
            const MATH: [&str; 7] = ["--", "+", "-", "*", "/", "FFT", "ABS"];
            let t2 = if m.math_scale.n > 0 {
                m.math_scale.as_str()
            } else if m.math_mode == 5 {
                FFT_DB[9]
            } else {
                b.as_str()
            };
            (Ink::new(sf::SC_F20, rect(417, 191, 60, 24), MATH[m.math_mode.min(6) as usize], 8, 0), Ink::new(sf::SC_F12, rect(417, 215, 60, 16), t2, 8, 0))
        }
        _ => (Ink::new(sf::SC_F20, r, "TRIG", 8, 0), Ink::none()),
    };
    paint(r, &[(K_SHAPE, C_BAR), (K_TEXT, fill), (K_TEXT, fill), (K_ICON, fill)], |x, y| {
        if let Some(plus) = mag {
            if magnifier(r, plus, x, y) {
                return (fg, tag(4, 0));
            }
        }
        if let Some(g) = t1.at(x, y) {
            return (fg, tag(2, g));
        }
        if let Some(g) = t2.at(x, y) {
            return (fg, tag(3, g));
        }
        if mask == 0 || in_rbox(r, radius, mask, x, y) { (fill, tag(1, 0)) } else { (C_BAR, 0) }
    });
}

/// Magnifier icon (UF-10) centred in `r`: a 2 px ring of radius 8 with a
/// "+" (`plus`) or "-" inside and a 3 px handle to the bottom right.
fn magnifier(r: Rect, plus: bool, x: i16, y: i16) -> bool {
    let (cx, cy) = (r.x + r.w / 2 - 3, r.y + r.h / 2 - 3);
    let (dx, dy) = ((x - cx) as i32, (y - cy) as i32);
    let d2 = dx * dx + dy * dy;
    if (49..=90).contains(&d2) {
        return true; // ring: radius 7..9.5
    }
    if dx.abs() <= 3 && dy.abs() <= 1 || (plus && dy.abs() <= 3 && dx.abs() <= 1) {
        return d2 < 49;
    }
    // Handle: along the diagonal from the ring outwards.
    (dx - dy).abs() <= 2 && dx >= 7 && dx <= 14 && dy >= 7 && dy <= 14
}

/// Measurement cell i (0..3): ringed box with `text` in the trace colour.
pub fn meas_cell(m: &Model, i: usize, text: &str) {
    let r = rect(13 + 100 * i as i16, 302, 98, 17);
    let t = Ink::new(sf::SC_F12, r, text, 8, 0);
    let pal = m.pal_text();
    paint(r, &[(K_SHAPE, C_BAR), (K_TEXT, C_BAR)], |x, y| {
        if let Some(g) = t.at(x, y) {
            return (pal, tag(2, g));
        }
        if on_ring(r, 5, 0x0f, x, y) {
            (C_FRAME, tag(1, 0))
        } else {
            (C_BAR, 0)
        }
    });
}

/// Every top-bar element.
pub fn top_bar(m: &Model) {
    status_badge(m);
    tb_badge(m);
    src_badge(m);
    record_bar(m);
    vdiv_badge(m);
    trig_badge(m);
    battery(m);
    usb_icon(m);
}

pub fn panel(m: &Model) {
    for k in 0..7 {
        panel_button(m, k);
    }
}

/// Page background boxes and the frame border (no graph interior). The
/// combined view keeps y 20..`top` for the meter strip (drawn by ui.rs).
pub fn chrome(m: &Model) {
    lcd::fill_rect(0, 0, 480, 20, C_BAR);
    lcd::fill_rect(414, 20, 66, 300, C_BAR);
    lcd::fill_rect(0, 301, 414, 19, C_BAR);
    lcd::fill_rect(0, 20, 13, 281, C_BAR);
    frame(m);
}

/// The graph's 1 px frame.
fn frame(m: &Model) {
    let top = m.top() as u16;
    let h = m.frame_h() as u16;
    lcd::fill_rect(13, top, 401, 1, C_FRAME);
    lcd::fill_rect(13, top + h - 1, 401, 1, C_FRAME);
    lcd::fill_rect(13, top, 1, h, C_FRAME);
    lcd::fill_rect(413, top, 1, h, C_FRAME);
}

// ------------------------------------------------------------------ markers

/// Marker glyph codes of font 0x08065558 (sec. 3): family base + variant
/// (0 down, 1 left, 2 right, 3 up; + 4 filled; 0x31 / 0x35, an "A" pointing
/// left, do not exist).
const FAM_1: u32 = 0x20;
const FAM_2: u32 = 0x28;
const FAM_A: u32 = 0x30;
const FAM_M: u32 = 0x38;
const FAM_T: u32 = 0x40;
const DOWN: u32 = 0;
const LEFT: u32 = 1;
const RIGHT: u32 = 2;
const UP: u32 = 3;
/// Eraser glyphs (by variant): down 'H', left 'I', right 'J', up 'K'.
const ERASER: [u32; 4] = [0x48, 0x49, 0x4a, 0x4b];

/// A 12 x 12 marker: icon and eraser glyph codes at (x, y), colour.
#[derive(Clone, Copy, PartialEq)]
pub struct Mark {
    pub x: i16,
    pub y: i16,
    pub icon: u8,
    pub mask: u8,
    pub col: Color,
}

impl Mark {
    pub const NONE: Mark = Mark { x: i16::MIN, y: 0, icon: 0, mask: 0, col: 0 };
    fn new(x: i16, y: i16, fam: u32, dir: u32, sel: bool, col: Color) -> Mark {
        Mark { x, y, icon: (fam + dir + if sel { 4 } else { 0 }) as u8, mask: ERASER[dir as usize] as u8, col }
    }
    pub fn some(&self) -> bool {
        self.x != i16::MIN
    }
    pub fn rect(&self) -> Rect {
        rect(self.x, self.y, 12, 12)
    }
}

/// Marker glyphs placed for compositing: (glyph, x, y) of the icon and the
/// eraser (both align 8 in the 12 x 12 box).
#[derive(Clone, Copy)]
struct MarkInk {
    icon: Option<(sf::Glyph, i16, i16)>,
    mask: Option<(sf::Glyph, i16, i16)>,
    col: Color,
}

fn place(font: Font, r: Rect, code: u32) -> Option<(sf::Glyph, i16, i16)> {
    let g = font.glyph(code)?;
    let lh = font.line_h() as i16;
    let x = r.x + ((r.w - g.w as i16) >> 1);
    let y = r.y + ((r.h - lh) >> 1) + lh - font.descent() - g.h as i16 - g.yoff as i16;
    Some((g, x, y))
}

#[inline]
fn ink(p: &Option<(sf::Glyph, i16, i16)>, x: i16, y: i16) -> bool {
    match p {
        Some((g, gx, gy)) => {
            let (dx, dy) = (x - gx, y - gy);
            dx >= 0 && dy >= 0 && dx < g.w as i16 && dy < g.h as i16 && g.alpha(dx as u32, dy as u32) >= 8
        }
        None => false,
    }
}

impl MarkInk {
    const NONE: MarkInk = MarkInk { icon: None, mask: None, col: 0 };
    fn of(m: &Mark) -> MarkInk {
        if !m.some() {
            return MarkInk::NONE;
        }
        let r = m.rect();
        MarkInk { icon: place(sf::SC_MARK, r, m.icon as u32), mask: place(sf::SC_MARK, r, m.mask as u32), col: m.col }
    }
    /// Some(colour) where the marker paints: icon colour, or the parent's
    /// background `bg` where only the eraser has ink.
    #[inline]
    fn at(&self, x: i16, y: i16, bg: Color) -> Option<Color> {
        if ink(&self.icon, x, y) {
            Some(self.col)
        } else if ink(&self.mask, x, y) {
            Some(bg)
        } else {
            None
        }
    }
}

/// Level-item slots (CH1, trigger level, math offset, runt level, Y
/// cursors).
pub const NSTRIP: usize = 6;
/// Level-item id of the trigger level.
const TRIG_LEVEL: u8 = 1;
/// x of the trigger-level marker: inside the graph at its right edge (x
/// 401..412, frame at 413).
const RIGHT_X: i16 = 401;

/// Level markers in creation order with their level-item ids (CH1 zero 0,
/// trigger level 1, math offset 2 while math is on, Y cursors 3 / 4) (sec.
/// 3.1). Rows scaled to the frame for the combined view. opendm40: the
/// trigger level sits on the graph's right edge (pointing left, the stock
/// glyphs mirrored), so it never covers the CH1 marker on the left strip;
/// the others are on the left strip as on the stock page.
pub fn strip_items(m: &Model, out: &mut [(u8, Mark); NSTRIP]) -> usize {
    let top = m.top();
    let h = m.frame_h();
    let pal = m.pal();
    let level = |row: i16, fam: u32, sel: bool, col: Color| -> Mark {
        let fy = line_fy(h, row as i32);
        if fy < 0 {
            Mark::new(1, top, fam, UP, sel, col)
        } else if fy > h - 1 {
            Mark::new(1, top + h - 13, fam, DOWN, sel, col)
        } else {
            Mark::new(1, top + fy - 6, fam, RIGHT, sel, col)
        }
    };
    // Right edge: inside the frame (rows 1..h-2), below the top markers'
    // row (y top+1..top+12) where one of them is at the right edge.
    let top_right = {
        let mut g = [Mark::NONE; NGRAPH];
        graph_marks_top(m, &mut g);
        g.iter().any(|mk| mk.some() && mk.x + 12 > RIGHT_X && mk.y < top + 13)
    };
    let y_min = if top_right { top + 13 } else { top + 1 };
    let right = |row: i16, fam: u32, sel: bool, col: Color| -> Mark {
        let fy = line_fy(h, row as i32);
        if fy < 0 {
            Mark::new(RIGHT_X, y_min, fam, UP, sel, col)
        } else if fy > h - 1 {
            Mark::new(RIGHT_X, top + h - 13, fam, DOWN, sel, col)
        } else {
            Mark::new(RIGHT_X, (top + fy - 6).clamp(y_min, top + h - 13), fam, LEFT, sel, col)
        }
    };
    // CH1 / math range is +-140 rows around the centre (the 280-row model).
    let off_row = |off: i16| if off < -140 { -1 } else if off > 140 { 281 } else { 140 + off };
    let mut n = 0;
    out[n] = (0, level(off_row(m.pos), FAM_A, m.sel_v == 0, pal));
    n += 1;
    out[n] = (TRIG_LEVEL, right(if m.trig_row > 280 { 281 } else { m.trig_row }, FAM_T, m.sel_v == 1, pal));
    n += 1;
    if m.math_on {
        out[n] = (2, level(off_row(m.math_off), FAM_M, m.sel_v == 2, m.math_pal()));
        n += 1;
    }
    if let Some(r) = m.runt_row {
        out[n] = (5, level(if r > 280 { 281 } else { r }, FAM_T, m.sel_v == 5, C_ORANGE));
        n += 1;
    }
    if let Some(yc) = m.ycur {
        out[n] = (3, level(yc[0], FAM_1, m.sel_v == 3, C_WHITE));
        n += 1;
        out[n] = (4, level(yc[1], FAM_2, m.sel_v == 4, C_WHITE));
        n += 1;
    }
    for o in out[n..].iter_mut() {
        *o = (255, Mark::NONE);
    }
    n
}

/// Left-strip markers in draw order (creation order, the selected level
/// item last; the trigger level is a graph marker, `graph_marks`).
pub fn strip_marks(m: &Model, out: &mut [Mark; NSTRIP]) -> usize {
    let mut items = [(0u8, Mark::NONE); NSTRIP];
    let n = strip_items(m, &mut items);
    let mut k = 0;
    for &(_, mk) in items[..n].iter().filter(|i| i.0 != m.sel_v && i.0 != TRIG_LEVEL) {
        out[k] = mk;
        k += 1;
    }
    for &(_, mk) in items[..n].iter().filter(|i| i.0 == m.sel_v && i.0 != TRIG_LEVEL) {
        out[k] = mk;
        k += 1;
    }
    for o in out[k..].iter_mut() {
        *o = Mark::NONE;
    }
    k
}

/// Top-of-graph marker for a column (FUN_08028bec), x graph-relative.
fn top_mark(top: i16, col: i16, fam: u32, sel: bool, colour: Color) -> Mark {
    let y = top + 1;
    if col < 12 {
        Mark::new(14 + col.max(0), y, fam, LEFT, sel, colour)
    } else if col + 12 <= 399 {
        Mark::new(14 + col - 6, y, fam, DOWN, sel, colour)
    } else {
        Mark::new(14 + if col > 399 { 387 } else { col - 13 }, y, fam, RIGHT, sel, colour)
    }
}

/// Graph marker slots: T, X cursor 1, X cursor 2, FFT centre, FFT peak,
/// trigger level (right edge).
pub const NGRAPH: usize = 6;
/// Time-item id of each graph marker slot (255: not a time item; the
/// trigger level's zone is a level zone, from `strip_items`).
const GRAPH_IDS: [u8; NGRAPH] = [0, 2, 3, 1, 255, 255];

/// Graph markers in draw order: T, X cursor 1, X cursor 2, the FFT centre
/// 'M' (stock, fixed at column 200, FFT mode), the FFT peak (opendm40: a white
/// filled 'M' pointing down at the peak, its tip on the peak row), the
/// trigger level on the right edge (opendm40, `strip_items`).
pub fn graph_marks(m: &Model, out: &mut [Mark; NGRAPH]) {
    graph_marks_top(m, out);
    let mut items = [(0u8, Mark::NONE); NSTRIP];
    let n = strip_items(m, &mut items);
    if let Some(&(_, mk)) = items[..n].iter().find(|i| i.0 == TRIG_LEVEL) {
        out[5] = mk;
    }
}

/// `graph_marks` without the trigger level.
fn graph_marks_top(m: &Model, out: &mut [Mark; NGRAPH]) {
    let top = m.top();
    *out = [Mark::NONE; NGRAPH];
    out[0] = top_mark(top, m.trig_px, FAM_T, m.sel_h == 0, m.pal());
    if let Some(xc) = m.xcur {
        out[1] = top_mark(top, xc[0], FAM_1, m.sel_h == 2, C_WHITE);
        out[2] = top_mark(top, xc[1], FAM_2, m.sel_h == 3, C_WHITE);
    }
    if m.math_on && m.math_mode == 5 {
        out[3] = top_mark(top, 200, FAM_M, m.sel_h == 1, m.math_pal());
        if let Some((col, row)) = m.fft.and_then(|f| f.peak_at) {
            // Tip (glyph row 11) one row above the peak's frame row; kept
            // below the top marker row.
            let fy = trace_fy(m.frame_h(), row as i32);
            if (0..399).contains(&col) && fy >= 25 {
                out[4] = Mark::new(14 + col - 6, top + fy - 12, FAM_M, DOWN, true, C_WHITE);
            }
        }
    }
}

/// A touch zone of a marker (sec. 10.2; stock: the 12x12 glyph rect grown
/// by 10 px, opendm40: by `GROW`) or of a line; `level`: level group (CH1 0,
/// trigger level 1, Y cursors 3 / 4), else time group (trigger position 0,
/// X cursors 2 / 3); `o`: the object itself (the glyph, the 1 px line),
/// for the distance in `pick_near`.
#[derive(Clone, Copy, PartialEq)]
pub struct Zone {
    pub level: bool,
    pub id: u8,
    pub r: Rect,
    pub o: Rect,
}

impl Zone {
    pub const NONE: Zone = Zone { level: false, id: 0, r: rect(0, 0, 0, 0), o: rect(0, 0, 0, 0) };
    /// A line zone (cursor line), not a marker glyph.
    pub fn line(&self) -> bool {
        self.o.w > 12 || self.o.h > 12
    }
    /// Distance (px, Chebyshev) from (x, y) to the zone's object; None
    /// outside the zone.
    pub fn dist(&self, x: i16, y: i16) -> Option<i16> {
        if !self.r.has(x, y) {
            return None;
        }
        let (x0, y0) = (self.o.x, self.o.y);
        let (x1, y1) = (self.o.x + self.o.w - 1, self.o.y + self.o.h - 1);
        let dx = (x0 - x).max(x - x1).max(0);
        let dy = (y0 - y).max(y - y1).max(0);
        Some(dx.max(dy))
    }
}

/// Marker zones: the glyph grown by this much (owner 2026-10-24: a drag
/// that starts near a marker grabs it; the stock 10 px).
pub const GROW: i16 = 12;
/// Line zones: +-this many px around a cursor line.
pub const LINE_GROW: i16 = 12;
/// `pick_near`: a selected item this much farther than the nearest still
/// wins (the marker last selected is preferred among close ones).
const NEAR_TIE: i16 = 4;

/// Most zones (strip markers + selectable graph markers).
pub const NZONE: usize = NSTRIP + NGRAPH;

/// The marker zones of the page in creation order (level group first).
pub fn zones(m: &Model, out: &mut [Zone; NZONE]) -> usize {
    let grow = |r: Rect| rect(r.x - GROW, r.y - GROW, r.w + 2 * GROW, r.h + 2 * GROW);
    let mut n = 0;
    let mut items = [(0u8, Mark::NONE); NSTRIP];
    let k = strip_items(m, &mut items);
    for &(id, mk) in &items[..k] {
        out[n] = Zone { level: true, id, r: grow(mk.rect()), o: mk.rect() };
        n += 1;
    }
    let mut g = [Mark::NONE; NGRAPH];
    graph_marks(m, &mut g);
    for (k, mk) in g.iter().enumerate() {
        if mk.some() && GRAPH_IDS[k] != 255 {
            out[n] = Zone { level: false, id: GRAPH_IDS[k], r: grow(mk.rect()), o: mk.rect() };
            n += 1;
        }
    }
    n
}

/// opendm40 addition (port plan sec. 5.2): the cursor lines themselves are
/// zones too, +-12 px around each line over the graph (the stock zones are
/// only around the small markers at the graph edge). After `zones`.
pub fn line_zones(m: &Model, out: &mut [Zone; 4]) -> usize {
    let (top, h) = (m.top(), m.frame_h());
    let mut n = 0;
    if let Some(xc) = m.xcur {
        for (k, c) in xc.iter().enumerate() {
            out[n] = vline_zone(false, 2 + k as u8, *c, top, h);
            n += 1;
        }
    }
    if let Some(yc) = m.ycur {
        for (k, r) in yc.iter().enumerate() {
            out[n] = hline_zone(3 + k as u8, top + trace_fy(h, *r as i32));
            n += 1;
        }
    }
    n
}

/// Zone of a vertical line at graph column `c` (x 14 + c), the frame height.
fn vline_zone(level: bool, id: u8, c: i16, top: i16, h: i16) -> Zone {
    Zone { level, id, r: rect(14 + c - LINE_GROW, top, 2 * LINE_GROW + 1, h), o: rect(14 + c, top, 1, h) }
}

/// Zone of a horizontal level line at screen row `y`, the graph width.
fn hline_zone(id: u8, y: i16) -> Zone {
    Zone { level: true, id, r: rect(13, y - LINE_GROW, 401, 2 * LINE_GROW + 1), o: rect(13, y, 401, 1) }
}

/// All press zones: markers (`zones`) and cursor lines (owner 2026-10-25:
/// the guide lines are not zones; a drag on the trace near a shown guide
/// line grabbed its item instead of panning).
pub const NALL: usize = NZONE + 4;

pub fn all_zones(m: &Model, out: &mut [Zone; NALL]) -> usize {
    let mut z = [Zone::NONE; NZONE];
    let mut n = 0;
    let k = zones(m, &mut z);
    for q in z[..k].iter() {
        out[n] = *q;
        n += 1;
    }
    let mut l = [Zone::NONE; 4];
    let k = line_zones(m, &mut l);
    for q in l[..k].iter() {
        out[n] = *q;
        n += 1;
    }
    n
}

/// opendm40 press pick (owner 2026-10-24: "a drag that starts on or near a
/// marker grabs it"): the zone whose object is nearest to (x, y); the
/// selected item (of the last used group first, then of the other group)
/// wins when it is at most NEAR_TIE px farther; equal distances keep the
/// zone order (markers before lines). Repeated taps still cycle through
/// overlapping zones: `pick_zone` on a still release (scope_page.rs).
pub fn pick_near(z: &[Zone], sel_v: u8, sel_h: u8, last_h: bool, x: i16, y: i16) -> Option<usize> {
    let mut best: Option<(usize, i16)> = None;
    for (i, q) in z.iter().enumerate() {
        if let Some(d) = q.dist(x, y) {
            if best.map_or(true, |(_, bd)| d < bd) {
                best = Some((i, d));
            }
        }
    }
    let (bi, bd) = best?;
    for (level, id) in [(!last_h, if last_h { sel_h } else { sel_v }), (last_h, if last_h { sel_v } else { sel_h })] {
        let pref = z.iter().enumerate().filter(|(_, q)| q.level == level && q.id == id).filter_map(|(i, q)| q.dist(x, y).map(|d| (i, d))).min_by_key(|&(_, d)| d);
        if let Some((i, d)) = pref {
            if d <= bd + NEAR_TIE {
                return Some(i);
            }
        }
    }
    Some(bi)
}

/// Drag axis decision: travel (px) that decides it (gesture.rs SLOP).
const LOCK_SLOP: i32 = 15;

/// The axis of a scope drag from its travel since the press (|dx|, |dy|):
/// Some(true) horizontal, Some(false) vertical, None undecided. Past
/// LOCK_SLOP / 2 px along an axis that leads 3:2, or past 2 * LOCK_SLOP
/// along the larger one.
pub fn drag_axis(ax: i32, ay: i32) -> Option<bool> {
    if 2 * ax >= 3 * ay && ax > LOCK_SLOP / 2 {
        Some(true)
    } else if 2 * ay >= 3 * ax && ay > LOCK_SLOP / 2 {
        Some(false)
    } else if ax.max(ay) > 2 * LOCK_SLOP {
        Some(ax >= ay)
    } else {
        None
    }
}

/// What a move report (dx, dy) applies once the drag's axis is decided
/// (`horizontal`): only that axis (owner 2026-10-25: one item, one axis
/// per gesture).
pub fn drag_delta(horizontal: bool, dx: i32, dy: i32) -> (i32, i32) {
    if horizontal { (dx, 0) } else { (0, dy) }
}

/// The item a drag moves (owner 2026-10-25: always the marker nearest to
/// where the finger went down, never the selection): `horizontal`, the
/// time marker whose column is nearest to x0 (trigger position T, the X
/// cursors when shown, the FFT centre in FFT mode), else the level marker
/// whose drawn row is nearest to y0 (`strip_items`: CH1, trigger level,
/// math offset when on, runt level when shown, Y cursors when shown; an
/// off-screen level at its edge marker). No distance limit; a tie goes to
/// the one drawn later (on top). (level group, item id).
#[inline(never)] // once per drag (region B: link-native.x)
pub fn drag_target(m: &Model, horizontal: bool, x0: i16, y0: i16) -> Option<(bool, u8)> {
    let mut best: Option<(u8, i16)> = None;
    let mut take = |id: u8, d: i16| {
        if best.map_or(true, |(_, bd)| d <= bd) {
            best = Some((id, d));
        }
    };
    if horizontal {
        take(0, (14 + m.trig_px - x0).abs());
        if let Some(xc) = m.xcur {
            take(2, (14 + xc[0] - x0).abs());
            take(3, (14 + xc[1] - x0).abs());
        }
        if m.math_on && m.math_mode == 5 {
            take(1, (14 + 200 - x0).abs());
        }
    } else {
        let mut items = [(0u8, Mark::NONE); NSTRIP];
        let n = strip_items(m, &mut items);
        for &(id, mk) in &items[..n] {
            take(id, (mk.y + 6 - y0).abs());
        }
    }
    best.map(|(id, _)| (!horizontal, id))
}

/// The zone a press at (x, y) picks (`pick_near` over `all_zones`).
pub fn graph_pick(m: &Model, sel_v: u8, sel_h: u8, last_h: bool, x: i16, y: i16) -> Option<Zone> {
    let mut z = [Zone::NONE; NALL];
    let n = all_zones(m, &mut z);
    pick_near(&z[..n], sel_v, sel_h, last_h, x, y).map(|k| z[k])
}

/// A still tap on a marker that was already selected: the next zone under
/// (x, y) in the stock search order (`pick_zone`: repeated taps cycle
/// through overlapping markers), or None.
pub fn graph_cycle(m: &Model, sel_v: u8, sel_h: u8, last_h: bool, x: i16, y: i16) -> Option<(bool, u8)> {
    let mut z = [Zone::NONE; NALL];
    let n = all_zones(m, &mut z);
    pick_zone(&z[..n], sel_v, sel_h, last_h, x, y).map(|k| (z[k].level, z[k].id))
}

/// The stock zone search for a press at (x, y): the group other than the
/// last selected one (`last_h`: time group), zones after its current item;
/// else the same group cycling from its current item + 1 (wrapping, so
/// repeated presses on overlapping zones cycle through them); else the
/// other group from its first zone. Index into `z`.
pub fn pick_zone(z: &[Zone], sel_v: u8, sel_h: u8, last_h: bool, x: i16, y: i16) -> Option<usize> {
    let same_level = !last_h;
    let of = |level: bool| -> ([usize; NALL], usize) {
        let mut l = [0usize; NALL];
        let mut k = 0;
        for (i, q) in z.iter().enumerate() {
            if q.level == level && k < NALL {
                l[k] = i;
                k += 1;
            }
        }
        (l, k)
    };
    let cur = |level: bool, l: &[usize]| {
        let id = if level { sel_v } else { sel_h };
        l.iter().position(|&i| z[i].id == id)
    };
    let hit = |i: usize| z[i].r.has(x, y);
    let (ol, on) = of(!same_level);
    if let Some(c) = cur(!same_level, &ol[..on]) {
        if let Some(&i) = ol[c + 1..on].iter().find(|&&i| hit(i)) {
            return Some(i);
        }
    }
    let (sl, sn) = of(same_level);
    if sn > 0 {
        let c = cur(same_level, &sl[..sn]).unwrap_or(sn - 1);
        for d in 1..=sn {
            let i = sl[(c + d) % sn];
            if hit(i) {
                return Some(i);
            }
        }
    }
    ol[..on].iter().copied().find(|&i| hit(i))
}

/// Repaint strip rows y0..=y1 (x 1..12) with the markers `marks` (draw order).
fn strip_rows(marks: &[Mark], y0: i16, y1: i16) {
    let mut ink: [MarkInk; NSTRIP] = [MarkInk::NONE; NSTRIP];
    for (i, mk) in marks.iter().enumerate().take(NSTRIP) {
        ink[i] = MarkInk::of(mk);
    }
    let n = marks.len().min(NSTRIP);
    let layers = [(K_ICON, C_BAR); NSTRIP];
    paint(rect(1, y0, 12, y1 - y0 + 1), &layers[..n], |x, y| {
        for i in (0..n).rev() {
            if let Some(c) = ink[i].at(x, y, C_BAR) {
                return (c, if c == C_BAR { 0 } else { tag(i as u16 + 1, 0) });
            }
        }
        (C_BAR, 0)
    });
}

// ------------------------------------------------------------------ graph

/// Frame row of a level row (markers, guide lines: stock "20 + row").
pub fn line_fy(h: i16, row: i32) -> i16 {
    if h == 281 { row as i16 } else { (row * (h as i32 - 1) / 280) as i16 }
}

/// Frame row of trace row `r` of the 280-row model (clamped 0..278 as
/// FUN_0802dc28; the trace sits 1 px below its markers on the stock page).
pub fn trace_fy(h: i16, r: i32) -> i16 {
    1 + (r.clamp(0, 278) * (h as i32 - 3) / 278) as i16
}

/// One horizontal line of the graph overlay.
#[derive(Clone, Copy, PartialEq)]
struct HLine {
    fy: i16,
    solid: bool,
    col: Color,
}

/// One vertical line.
#[derive(Clone, Copy, PartialEq)]
struct VLine {
    fx: i16,
    col: Color,
}

const NL: usize = 3;

/// Overlay lines + graph markers + colours: what besides the traces decides
/// the graph pixels.
#[derive(Clone, Copy, PartialEq)]
struct Deco {
    h: [Option<HLine>; NL],
    v: [Option<VLine>; NL],
    marks: [Mark; NGRAPH],
    grid: Color,
    trace: Color,
    half: Color,
    math: Color,
    top: i16,
    fh: i16,
}

impl Deco {
    const NONE: Deco = Deco { h: [None; NL], v: [None; NL], marks: [Mark::NONE; NGRAPH], grid: 0, trace: 0, half: 0, math: 0, top: 0, fh: 0 };

    fn of(m: &Model) -> Deco {
        let h = m.frame_h();
        let mut d = Deco {
            h: [None; NL],
            v: [None; NL],
            marks: [Mark::NONE; NGRAPH],
            grid: m.grid_colour(),
            trace: m.trace_colour(),
            half: m.half_colour(),
            math: m.math_pal(),
            top: m.top(),
            fh: h,
        };
        graph_marks(m, &mut d.marks);
        let ctr = d.trace;
        // 4.4: guide lines of the selected items, then the cursor lines.
        if m.guide {
            match m.sel_h {
                0 if (1..=399).contains(&m.trig_px) => d.v[0] = Some(VLine { fx: m.trig_px, col: ctr }),
                // FFT centre (stock: a line at x 0 that is never positioned;
                // opendm40: the centre column the 'M' marker stands on).
                1 if m.math_on && m.math_mode == 5 => d.v[0] = Some(VLine { fx: 200, col: d.math }),
                _ => {}
            }
            match m.sel_v {
                0 => d.h[0] = Some(HLine { fy: line_fy(h, 140 + m.pos as i32), solid: true, col: ctr }),
                1 => d.h[0] = Some(HLine { fy: line_fy(h, m.trig_row as i32), solid: false, col: ctr }),
                2 if m.math_on => d.h[0] = Some(HLine { fy: line_fy(h, 140 + m.math_off as i32), solid: true, col: d.math }),
                5 => {
                    if let Some(r) = m.runt_row {
                        d.h[0] = Some(HLine { fy: line_fy(h, r as i32), solid: false, col: C_ORANGE });
                    }
                }
                _ => {}
            }
        }
        if let Some(xc) = m.xcur {
            d.v[1] = Some(VLine { fx: xc[0] + 1, col: C_FRAME });
            d.v[2] = Some(VLine { fx: xc[1] + 1, col: C_FRAME });
        }
        if let Some(yc) = m.ycur {
            d.h[1] = Some(HLine { fy: trace_fy(h, yc[0] as i32), solid: false, col: C_FRAME });
            d.h[2] = Some(HLine { fy: trace_fy(h, yc[1] as i32), solid: false, col: C_FRAME });
        }
        d
    }
}

/// No data in a column.
pub const EMPTY: (i16, i16) = (i16::MAX, i16::MIN);

/// The graph: what each column shows and what is on the panel.
pub struct Graph {
    /// Trace / persistence / math trace frame rows per graph column (EMPTY
    /// = none).
    pub tr: [(i16, i16); 399],
    pub ps: [(i16, i16); 399],
    pub ma: [(i16, i16); 399],
    /// The GEN trace (C_GEN) and the CH2 trace (C_CH2), under CH1.
    pub gn: [(i16, i16); 399],
    pub c2: [(i16, i16); 399],
    d_tr: [(i16, i16); 399],
    d_ps: [(i16, i16); 399],
    d_ma: [(i16, i16); 399],
    d_gn: [(i16, i16); 399],
    d_c2: [(i16, i16); 399],
    deco: Deco,
    d_deco: Deco,
    valid: bool,
    /// Screen rects the graph leaves alone (overlays drawn on top, slots
    /// H_*), and those of the last flush.
    holes: [Option<Rect>; NH],
    d_holes: [Option<Rect>; NH],
    /// Pixels written by the last flush (host budget check / DBG).
    pub px: u32,
    /// Trace-only flush resumes at this column (`FLUSH_BUDGET`).
    next: u16,
}

/// Device: a flush that only moves traces (decoration and holes unchanged:
/// the frame-to-frame case) stops after this many DWT cycles (1.5 ms) and
/// continues on the next pass at the column it reached, so a busy trace
/// (a square wave's edges) does not stretch one main-loop pass (device
/// check 2026-10-21). 4 ms was too long for USB: a noise-triggered trace
/// at ~50 fps (every column changes, ~10 ms of paint per frame) kept the
/// loop in 2-10 ms passes back to back, DBG:PING? p50 11.8 ms (device check
/// of f80524f, combined view). Host: no limit, every flush completes.
#[cfg(target_os = "none")]
const FLUSH_BUDGET: u32 = 300_000;
/// The combined view's slice (0.8 ms, `set_flush_short`): its frames are
/// capped (app.rs COMBO_FRAME_MS), so the extra passes cost no frame rate,
/// and the meter's USB / SCPI wait less behind a slice (device check
/// c50425f: DBG:PING? p50 3.3 ms there).
#[cfg(target_os = "none")]
const FLUSH_BUDGET_SHORT: u32 = 160_000;
#[cfg(target_os = "none")]
static FLUSH_SHORT: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// Use the short flush slice (the combined view) or the normal one.
pub fn set_flush_short(on: bool) {
    #[cfg(target_os = "none")]
    FLUSH_SHORT.store(on, core::sync::atomic::Ordering::Relaxed);
    #[cfg(not(target_os = "none"))]
    let _ = on;
}

#[cfg(target_os = "none")]
fn cycles() -> u32 {
    unsafe { core::ptr::read_volatile(0xE000_1004 as *const u32) }
}

// In SRAM (.dmabuf, NOLOAD) on the device: 9.7 KB that TCM .data / the
// stack margin need more (pre-flash review 7389e8a); `init` sets it up.
#[cfg_attr(target_os = "none", link_section = ".dmabuf")]
static mut GRAPH: Graph = Graph {
    tr: [EMPTY; 399],
    ps: [EMPTY; 399],
    ma: [EMPTY; 399],
    gn: [EMPTY; 399],
    c2: [EMPTY; 399],
    d_tr: [EMPTY; 399],
    d_ps: [EMPTY; 399],
    d_ma: [EMPTY; 399],
    d_gn: [EMPTY; 399],
    d_c2: [EMPTY; 399],
    deco: Deco::NONE,
    d_deco: Deco::NONE,
    valid: false,
    holes: [None; NH],
    d_holes: [None; NH],
    px: 0,
    next: 0,
};

/// Graph hole slots (`Graph::set_hole_at`): the pinch bubble, the timebase
/// tabs, the FFT label and boxes, the cursor tab, an open popup's box, the
/// rows `regraph_rows` leaves alone, the protocol decoder's band, the
/// CH2 (DMM) label.
const NH: usize = 12;
pub const H_BUBBLE: usize = 0;
pub const H_TAB_L: usize = 1;
pub const H_TAB_R: usize = 2;
pub const H_FFT_HZ: usize = 3;
pub const H_FFT_Y: usize = 4;
pub const H_FFT_X: usize = 5;
pub const H_FFT_PK: usize = 6;
pub const H_CUR: usize = 7;
pub const H_POPUP: usize = 8;
const H_REST: usize = 9;
/// The protocol decoder's band (ui_decode.rs).
pub const H_DEC: usize = 10;
/// The DMM CH2's label.
pub const H_SPARE: usize = 11;

/// Set the graph state up in place (its static is not loaded on the
/// device). Call once before any other scope_ui function.
pub fn init() {
    let g = unsafe { &mut *core::ptr::addr_of_mut!(GRAPH) };
    for i in 0..399 {
        g.tr[i] = EMPTY;
        g.ps[i] = EMPTY;
        g.ma[i] = EMPTY;
        g.d_tr[i] = EMPTY;
        g.d_ps[i] = EMPTY;
        g.d_ma[i] = EMPTY;
        g.gn[i] = EMPTY;
        g.c2[i] = EMPTY;
        g.d_gn[i] = EMPTY;
        g.d_c2[i] = EMPTY;
    }
    g.deco = Deco::NONE;
    g.d_deco = Deco::NONE;
    g.valid = false;
    g.holes = [None; NH];
    g.d_holes = [None; NH];
    g.px = 0;
    g.next = 0;
    init_shown();
}

/// `init`'s SHOWN / STRIP_SHOWN part. Not inlined: start-up only, in
/// region C (link-native.x) apart from `new_app`.
#[inline(never)]
fn init_shown() {
    unsafe {
        core::ptr::addr_of_mut!(SHOWN).write(Shown { m: None, strip: [Mark::NONE; NSTRIP], cells: [[0; 16]; 4], cells_n: [0; 4] });
        core::ptr::addr_of_mut!(STRIP_SHOWN).write(None);
    }
}

/// The graph state, read only (overlays reading its background).
fn graph_ref() -> &'static Graph {
    unsafe { &*core::ptr::addr_of!(GRAPH) }
}

/// The graph state (single-threaded firmware).
pub fn graph() -> &'static mut Graph {
    unsafe { &mut *core::ptr::addr_of_mut!(GRAPH) }
}

/// Row classes of the grid rule (`grid_on`), one byte per frame row
/// (`ROWS`): centre row, every 7th row, the division rows, the rows
/// near the centre; R_LINE: a horizontal line (`Deco::h`) on the row.
const R_CY: u8 = 1;
const R_7: u8 = 2;
const R_DH: u8 = 4;
const R_NEAR: u8 = 8;
const R_LINE: u8 = 16;
/// Every row (the centre column fx = 200).
const R_ALL: u8 = 0x80;

/// `Graph::flush`'s row classes of the frame being painted: the per-row
/// part of the grid rule, worked out once per flush. Per pixel, its
/// divisions made the most of a full graph repaint's time (device
/// DBG:PROF?, 2026-10-26: the scope page's full repaint 262 ms). In SRAM:
/// the TCM's .bss is taken from the stack.
#[cfg_attr(target_os = "none", link_section = ".sram_bss")]
static mut ROWS: [u8; 320] = [0; 320];

/// Fill `ROWS` for a frame of `h` rows and its horizontal lines.
fn row_classes(h: i16, hl: &[Option<HLine>; NL]) -> &'static [u8; 320] {
    let rows = unsafe { &mut *core::ptr::addr_of_mut!(ROWS) };
    let (cy, dh) = ((h - 1) / 2, (h - 1) / 8);
    for fy in 0..h.clamp(0, 320) {
        rows[fy as usize] = row_class(fy, cy, dh);
    }
    for l in hl.iter().flatten() {
        if let Some(r) = rows.get_mut(l.fy as usize) {
            *r |= R_LINE;
        }
    }
    rows
}

/// The `grid_on` classes of row `fy` (`cy` the centre row, `dh` the
/// division height).
fn row_class(fy: i16, cy: i16, dh: i16) -> u8 {
    let mut c = 0;
    if fy == cy {
        c |= R_CY;
    }
    if fy % 7 == 0 {
        c |= R_7;
    }
    if fy % dh == 0 {
        c |= R_DH;
    }
    if (fy - cy).abs() <= 2 {
        c |= R_NEAR;
    }
    c
}

/// The `grid_on` classes of column `fx` (rows with `ROWS[fy] & mask != 0`
/// are grid; R_ALL: all of them).
/// `col_classes` of fx = 0..=399 (flash): the row loop of a full graph in
/// strips looks them up per pixel.
static COLC: [u8; 400] = {
    let mut t = [0u8; 400];
    let mut fx = 0;
    while fx < 400 {
        t[fx] = col_classes(fx as i16);
        fx += 1;
    }
    t
};

const fn col_classes(fx: i16) -> u8 {
    if fx == 200 {
        return R_ALL;
    }
    let mut c = R_CY;
    if fx % 40 == 0 || (fx - 200).abs() <= 2 {
        c |= R_7;
    }
    if fx % 8 == 0 {
        c |= R_DH | R_NEAR;
    }
    c
}

/// Grid rule of the frame (sec. 4.1) at frame (fx, fy), 1 <= fx <= 399,
/// 1 <= fy <= h - 2; `cy` the centre row, `dh` the division height.
/// That is: the centre column and row; every 7th row in the columns at
/// every 40th and within 2 of the centre column; the division rows and
/// the rows within 2 of the centre in every 8th column.
fn grid_on(fx: i16, fy: i16, cy: i16, dh: i16) -> bool {
    (row_class(fy, cy, dh) | R_ALL) & col_classes(fx) != 0
}

/// Row intervals of one column to repaint.
struct Spans {
    s: [(i16, i16); 12],
    n: usize,
}

impl Spans {
    fn add(&mut self, a: (i16, i16)) {
        if a.0 > a.1 {
            return;
        }
        if self.n == self.s.len() {
            // Out of slots: widen the last one.
            let l = &mut self.s[self.n - 1];
            *l = (l.0.min(a.0), l.1.max(a.1));
            return;
        }
        self.s[self.n] = a;
        self.n += 1;
    }
    /// Sort and merge overlapping / adjacent spans, clamp to lo..=hi.
    fn merged(&mut self, lo: i16, hi: i16) -> &[(i16, i16)] {
        let s = &mut self.s[..self.n];
        s.sort_unstable_by_key(|a| a.0);
        let mut k = 0;
        for i in 0..s.len() {
            let (a, b) = (s[i].0.max(lo), s[i].1.min(hi));
            if a > b {
                continue;
            }
            if k > 0 && a <= s[k - 1].1 + 1 {
                s[k - 1].1 = s[k - 1].1.max(b);
            } else {
                s[k] = (a, b);
                k += 1;
            }
        }
        &self.s[..k]
    }
}

/// Graph row `r` (row classes `rc`) over a run of columns without markers,
/// line or vertical line, into the strip's pixels `sp` (`Graph::flush` in
/// a strip pass): the column loop's inline rule.
#[inline(never)]
fn band_run(sp: &mut [Color], tr: &[(i16, i16)], ps: &[(i16, i16)], ma: &[(i16, i16)], cg: (&[(i16, i16)], &[(i16, i16)]), cols: &[u8], r: i16, rc: u8, d: &Deco) {
    for (p, (((((tr, ps), ma), c2), gn), &cc)) in sp.iter_mut().zip(tr.iter().zip(ps).zip(ma).zip(cg.0).zip(cg.1).zip(cols)) {
        *p = if r >= ma.0 && r <= ma.1 {
            d.math
        } else if r >= tr.0 && r <= tr.1 {
            d.trace
        } else if r >= c2.0 && r <= c2.1 {
            C_CH2
        } else if r >= gn.0 && r <= gn.1 {
            C_GEN_TR
        } else if r >= ps.0 && r <= ps.1 {
            d.half
        } else if (rc | R_ALL) & cc != 0 {
            d.grid
        } else {
            C_FILL
        };
    }
}

impl Graph {
    /// Forget what is on the panel: the next flush repaints the whole graph.
    pub fn invalidate(&mut self) {
        self.valid = false;
    }

    /// Leave `r` (screen) alone from now on (None: paint it again): the
    /// pinch bubble's slot.
    pub fn set_hole(&mut self, r: Option<Rect>) {
        self.holes[H_BUBBLE] = r;
    }

    /// Hole slot `i` (1 / 2: the timebase tabs, set by `update`).
    pub fn set_hole_at(&mut self, i: usize, r: Option<Rect>) {
        self.holes[i] = r;
    }

    /// Clear the trace and persistence (an empty graph).
    pub fn clear(&mut self) {
        self.tr = [EMPTY; 399];
        self.ps = [EMPTY; 399];
        self.ma = [EMPTY; 399];
        self.gn = [EMPTY; 399];
        self.c2 = [EMPTY; 399];
    }

    /// Bring the panel up to date with `tr` / `ps` / `ma` and the model's
    /// lines, markers and colours, repainting only the rows that changed.
    pub fn flush(&mut self, m: &Model) {
        self.deco = Deco::of(m);
        let d = self.deco;
        let o = self.d_deco;
        // Nothing changed (most main-loop passes): no per-column work.
        if self.valid && d == o && self.holes == self.d_holes && self.tr == self.d_tr && self.ps == self.d_ps && self.ma == self.d_ma && self.gn == self.d_gn && self.c2 == self.d_c2 {
            self.px = 0;
            return;
        }
        let full = !self.valid || d.grid != o.grid || d.trace != o.trace || d.half != o.half || d.math != o.math || d.top != o.top || d.fh != o.fh;
        let h = d.fh;
        let mut minks = [MarkInk::NONE; NGRAPH];
        for (i, mk) in d.marks.iter().enumerate() {
            minks[i] = MarkInk::of(mk);
        }
        // Rows of changed horizontal lines (guide / cursor lines): repainted
        // across the graph as row runs after the columns, one LCD window per
        // run, instead of one window per row in each of the 399 columns.
        // A trigger-level drag step moves its guide line: report -> marker
        // 15 ms before, 7 ms with this and the trigger badge after the graph
        // (emu/draglat.py, emu --mips 110 = the device model).
        let mut hrows = [0i16; 2 * NL];
        let mut nh = 0;
        if !full {
            for k in 0..NL {
                if d.h[k] != o.h[k] {
                    for l in [d.h[k], o.h[k]].into_iter().flatten() {
                        if l.fy >= 1 && l.fy <= h - 2 && !hrows[..nh].contains(&l.fy) {
                            hrows[nh] = l.fy;
                            nh += 1;
                        }
                    }
                }
            }
        }
        let marks_changed = d.marks != o.marks;
        let vl = |dd: &Deco, fx: i16| dd.v.iter().flatten().find(|l| l.fx == fx).map(|l| l.col);
        let rows = row_classes(h, &d.h);
        // Colour of graph pixel (fx, fy): markers over the column (`mcol`,
        // the topmost last), math, trace, persistence, the vertical line of
        // the column (`vcol`), horizontal lines, grid (`cm`: col_classes).
        type Rows = ((i16, i16), (i16, i16), (i16, i16), (i16, i16), (i16, i16));
        let pix = |fx: i16, fy: i16, (tr, ps, ma, c2, gn): Rows, vcol: Option<Color>, mcol: &[usize], cm: u8| -> Color {
            let (sx, sy) = (13 + fx, d.top + fy);
            for &mi in mcol.iter().rev() {
                if let Some(c) = minks[mi].at(sx, sy, C_FILL) {
                    return c;
                }
            }
            if fy >= ma.0 && fy <= ma.1 {
                return d.math;
            }
            if fy >= tr.0 && fy <= tr.1 {
                return d.trace;
            }
            if fy >= c2.0 && fy <= c2.1 {
                return C_CH2;
            }
            if fy >= gn.0 && fy <= gn.1 {
                return C_GEN_TR;
            }
            if fy >= ps.0 && fy <= ps.1 {
                return d.half;
            }
            if let Some(c) = vcol {
                if (fy - 1) % 2 == 0 && fy <= h - 4 {
                    return c;
                }
            }
            let rc = rows[fy as usize];
            if rc & R_LINE != 0 {
                for l in d.h.iter().flatten() {
                    if l.fy == fy && fx <= 397 && (l.solid || fx % 2 == 1) {
                        return l.col;
                    }
                }
            }
            if (rc | R_ALL) & cm != 0 { d.grid } else { C_FILL }
        };
        self.px = 0;
        // Trace-only: per-column bookkeeping, from the resume column, under
        // the device's time budget (`FLUSH_BUDGET`).
        let trace_only = !full && d == o && self.holes == self.d_holes;
        let start = if trace_only { self.next as usize % 399 } else { 0 };
        #[cfg(target_os = "none")]
        let t0 = cycles();
        #[cfg(target_os = "none")]
        let budget = if FLUSH_SHORT.load(core::sync::atomic::Ordering::Relaxed) { FLUSH_BUDGET_SHORT } else { FLUSH_BUDGET };
        // A full graph in a strip pass (lcd.rs "strip rendering"): row by
        // row, the strip's rows only (the row loop below), instead of 399
        // column windows per strip.
        let band = if full && lcd::in_strip() {
            Some(lcd::clip_rows(d.top + 1, d.top + h - 2).map_or((1, 0), |(a, b)| (a - d.top, b - d.top)))
        } else {
            None
        };
        for j in 0..if band.is_some() { 0 } else { 399usize } {
            let i = (start + j) % 399;
            #[cfg(target_os = "none")]
            if trace_only && self.px > 0 && cycles().wrapping_sub(t0) > budget {
                self.next = i as u16;
                return;
            }
            let fx = i as i16 + 1;
            let sx = 13 + fx;
            let mut sp = Spans { s: [(0, 0); 12], n: 0 };
            if full || vl(&d, fx) != vl(&o, fx) {
                sp.add((1, h - 2));
            } else {
                for (a, b) in [(self.tr[i], self.d_tr[i]), (self.ps[i], self.d_ps[i]), (self.ma[i], self.d_ma[i]), (self.gn[i], self.d_gn[i]), (self.c2[i], self.d_c2[i])] {
                    if a != b {
                        sp.add(a);
                        sp.add(b);
                    }
                }
                if marks_changed {
                    for mk in d.marks.iter().chain(o.marks.iter()) {
                        if mk.some() && sx >= mk.x && sx < mk.x + 12 {
                            sp.add((mk.y - d.top, mk.y - d.top + 11));
                        }
                    }
                }
                // A hole that went away / moved: its rows come back.
                for k in 0..NH {
                    if let Some(o) = self.d_holes[k].filter(|_| self.holes[k] != self.d_holes[k]) {
                        if sx >= o.x && sx < o.x + o.w {
                            sp.add((o.y - d.top, o.y + o.h - 1 - d.top));
                        }
                    }
                }
            }
            if trace_only {
                self.d_tr[i] = self.tr[i];
                self.d_ps[i] = self.ps[i];
                self.d_ma[i] = self.ma[i];
                self.d_gn[i] = self.gn[i];
                self.d_c2[i] = self.c2[i];
            }
            if sp.n == 0 {
                continue;
            }
            let (tr, ps, ma, c2, gn) = (self.tr[i], self.ps[i], self.ma[i], self.c2[i], self.gn[i]);
            let vcol = vl(&d, fx);
            // Markers over this column.
            let mut mcol = [0usize; NGRAPH];
            let mut nm = 0;
            for (k, mk) in d.marks.iter().enumerate() {
                if mk.some() && sx >= mk.x && sx < mk.x + 12 {
                    mcol[nm] = k;
                    nm += 1;
                }
            }
            // Rows of this column under holes are not painted.
            let mut cut = [(0i16, 0i16); 24];
            let mut nc = 0;
            for &(a, b) in sp.merged(1, h - 2) {
                let mut segs = [(a, b); 12];
                let mut ns = 1;
                for r in self.holes.iter().flatten().filter(|r| sx >= r.x && sx < r.x + r.w) {
                    let (ha, hb) = (r.y - d.top, r.y + r.h - 1 - d.top);
                    let mut next = [(0i16, 0i16); 12];
                    let mut nn = 0;
                    for &(sa, sb) in &segs[..ns] {
                        if sb < ha || sa > hb {
                            next[nn] = (sa, sb);
                            nn += 1;
                            continue;
                        }
                        if sa < ha && nn < 12 {
                            next[nn] = (sa, ha - 1);
                            nn += 1;
                        }
                        if sb > hb && nn < 12 {
                            next[nn] = (hb + 1, sb);
                            nn += 1;
                        }
                    }
                    segs = next;
                    ns = nn;
                }
                for &sg in &segs[..ns] {
                    if nc < cut.len() {
                        cut[nc] = sg;
                        nc += 1;
                    }
                }
            }
            let cm = col_classes(fx);
            // `pix` inline for the rows without a marker or a horizontal
            // line: a full graph took 108 ms through the closure.
            let vrow = vcol.map_or(0, |_| h - 4);
            for &(a, b) in &cut[..nc] {
                self.px += (b - a + 1) as u32;
                // Never in a strip pass (a full graph there goes by rows,
                // below; this loop has no columns then).
                lcd::set_window(sx as u16, (d.top + a) as u16, sx as u16, (d.top + b) as u16);
                for fy in a..=b {
                    let rc = rows[fy as usize];
                    lcd::data_panel(if nm > 0 || rc & R_LINE != 0 {
                        pix(fx, fy, (tr, ps, ma, c2, gn), vcol, &mcol[..nm], cm)
                    } else if fy >= ma.0 && fy <= ma.1 {
                        d.math
                    } else if fy >= tr.0 && fy <= tr.1 {
                        d.trace
                    } else if fy >= c2.0 && fy <= c2.1 {
                        C_CH2
                    } else if fy >= gn.0 && fy <= gn.1 {
                        C_GEN_TR
                    } else if fy >= ps.0 && fy <= ps.1 {
                        d.half
                    } else if fy & 1 == 1 && fy <= vrow {
                        vcol.unwrap_or(C_FILL)
                    } else if (rc | R_ALL) & cm != 0 {
                        d.grid
                    } else {
                        C_FILL
                    });
                }
            }
        }
        // The changed line rows (`hrows`), left to right in runs between
        // the holes over that row; per row, only the holes and markers
        // that reach it are checked per pixel.
        let nrows = band.map_or(nh, |(a, b)| (b - a + 1).max(0) as usize);
        for k in 0..nrows {
            let r = band.map_or_else(|| hrows[k], |(a, _)| a + k as i16);
            let sy = d.top + r;
            if !lcd::rows_hit(sy, sy) {
                continue;
            }
            let mut cuts = [(0i16, 0i16); NH];
            let mut ncut = 0;
            for o in self.holes.iter().flatten().filter(|o| sy >= o.y && sy < o.y + o.h) {
                cuts[ncut] = (o.x - 13, o.x + o.w - 14);
                ncut += 1;
            }
            let mut mrow = [0usize; NGRAPH];
            let mut nmr = 0;
            for (k, mk) in d.marks.iter().enumerate() {
                if mk.some() && sy >= mk.y && sy < mk.y + 12 {
                    mrow[nmr] = k;
                    nmr += 1;
                }
            }
            let in_hole = |fx: i16| cuts[..ncut].iter().any(|&(a, b)| fx >= a && fx <= b);
            // Pixels without a marker on a row without a horizontal line
            // (most of a full graph in strips): `pix` inline, as the column
            // loop. The markers' columns: mlo..=mhi.
            let rc = rows[r as usize];
            let line = rc & R_LINE != 0;
            let (mut mlo, mut mhi) = (i16::MAX, i16::MIN);
            for &k in &mrow[..nmr] {
                mlo = mlo.min(d.marks[k].x);
                mhi = mhi.max(d.marks[k].x + 11);
            }
            let vrow = r & 1 == 1 && r <= h - 4 && d.v.iter().any(|l| l.is_some());
            let mut fx = 1i16;
            while fx <= 399 {
                if in_hole(fx) {
                    fx += 1;
                    continue;
                }
                // The run up to the next hole.
                let mut e = 399;
                for &(a, _) in &cuts[..ncut] {
                    if a > fx && a - 1 < e {
                        e = a - 1;
                    }
                }
                self.px += (e - fx + 1) as u32;
                // A strip pass: straight into the strip's row.
                let mut span = lcd::strip_span(sy, 13 + fx, 13 + e);
                let mut out = lcd::Out::panel();
                if let Some(sp) = span.as_deref_mut() {
                    if !line && !vrow && (13 + e < mlo || 13 + fx > mhi) {
                        // No marker, line or vertical line in the run: the
                        // inline rule over the columns, no index checks.
                        band_run(sp, &self.tr[(fx - 1) as usize..e as usize], &self.ps[(fx - 1) as usize..e as usize], &self.ma[(fx - 1) as usize..e as usize], (&self.c2[(fx - 1) as usize..e as usize], &self.gn[(fx - 1) as usize..e as usize]), &COLC[fx as usize..=e as usize], r, rc, &d);
                        fx = e + 1;
                        continue;
                    }
                }
                if span.is_none() {
                    lcd::set_window((13 + fx) as u16, sy as u16, (13 + e) as u16, sy as u16);
                }
                for x in fx..=e {
                    let i = (x - 1) as usize;
                    let sx = 13 + x;
                    let c = if !line && (sx < mlo || sx > mhi || !mrow[..nmr].iter().any(|&k| sx >= d.marks[k].x && sx < d.marks[k].x + 12)) {
                        let (tr, ps, ma, c2, gn) = (self.tr[i], self.ps[i], self.ma[i], self.c2[i], self.gn[i]);
                        if r >= ma.0 && r <= ma.1 {
                            d.math
                        } else if r >= tr.0 && r <= tr.1 {
                            d.trace
                        } else if r >= c2.0 && r <= c2.1 {
                            C_CH2
                        } else if r >= gn.0 && r <= gn.1 {
                            C_GEN_TR
                        } else if r >= ps.0 && r <= ps.1 {
                            d.half
                        } else if let Some(c) = if vrow { vl(&d, x) } else { None } {
                            c
                        } else if (rc | R_ALL) & COLC[x as usize] != 0 {
                            d.grid
                        } else {
                            C_FILL
                        }
                    } else {
                        let mut mcol = [0usize; NGRAPH];
                        let mut nm = 0;
                        for &k in &mrow[..nmr] {
                            if sx >= d.marks[k].x && sx < d.marks[k].x + 12 {
                                mcol[nm] = k;
                                nm += 1;
                            }
                        }
                        pix(x, r, (self.tr[i], self.ps[i], self.ma[i], self.c2[i], self.gn[i]), vl(&d, x), &mcol[..nm], col_classes(x))
                    };
                    match span.as_deref_mut() {
                        Some(sp) => sp[(x - fx) as usize] = c,
                        None => out.px(c),
                    }
                }
                fx = e + 1;
            }
        }
        self.next = 0;
        self.d_tr = self.tr;
        self.d_ps = self.ps;
        self.d_ma = self.ma;
        self.d_gn = self.gn;
        self.d_c2 = self.c2;
        self.d_deco = d;
        self.d_holes = self.holes;
        self.valid = true;
    }

    /// The graph background at screen (x, y) without the traces and
    /// markers: grid, guide / cursor lines (for transparent overlays such
    /// as the stock FFT label). Uses the last flushed decoration.
    pub fn bg_at(&self, x: i16, y: i16) -> Color {
        let d = self.d_deco;
        let (fx, fy) = (x - 13, y - d.top);
        let h = d.fh;
        if fx < 1 || fx > 399 || fy < 1 || fy > h - 2 {
            return C_FILL;
        }
        if let Some(c) = d.v.iter().flatten().find(|l| l.fx == fx).map(|l| l.col) {
            if (fy - 1) % 2 == 0 && fy <= h - 4 {
                return c;
            }
        }
        for l in d.h.iter().flatten() {
            if l.fy == fy && fx <= 397 && (l.solid || fx % 2 == 1) {
                return l.col;
            }
        }
        if grid_on(fx, fy, (h - 1) / 2, (h - 1) / 8) { d.grid } else { C_FILL }
    }
}

// ------------------------------------------------------------------ page

/// What the page last drew (None: nothing / invalid).
struct Shown {
    m: Option<Model>,
    strip: [Mark; NSTRIP],
    cells: [[u8; 16]; 4],
    cells_n: [u8; 4],
}

// On the device in SRAM (.dmabuf, NOLOAD, as GRAPH; region B audit #6:
// no .data load image in region B); `init` sets it up.
#[cfg_attr(target_os = "none", link_section = ".dmabuf")]
static mut SHOWN: Shown = Shown { m: None, strip: [Mark::NONE; NSTRIP], cells: [[0; 16]; 4], cells_n: [0; 4] };

fn shown() -> &'static mut Shown {
    unsafe { &mut *core::ptr::addr_of_mut!(SHOWN) }
}

/// A popup over the graph closed: the next `update` repaints the graph
/// (its overlays too) and the frame, the rest of the page only where the
/// model changed; `ch1` / `math`: their panel labels were drawn grey
/// (panel_button outside `update`), so they are drawn again.
#[inline(never)] // region C (link-native.x)
pub fn uncover(m: &Model, ch1: bool, math: bool) {
    if let Some(o) = shown().m.as_mut() {
        o.ch1_open |= ch1;
        o.math_open |= math;
    }
    graph().invalidate();
    frame(m);
}

/// Rows y0..y1 (screen, exclusive) of the graph and its side frame lines
/// again, the rest of the graph left alone: what a shrinking popup box
/// (`keep`: the new box, drawn) uncovers. Only the graph lies there (the
/// overlays sit at the graph's top rows and its bottom row).
pub fn regraph_rows(m: &Model, keep: Rect, y0: i16, y1: i16) {
    if y1 <= y0 {
        return;
    }
    lcd::fill_rect(13, y0 as u16, 1, (y1 - y0) as u16, C_FRAME);
    lcd::fill_rect(413, y0 as u16, 1, (y1 - y0) as u16, C_FRAME);
    let g = graph();
    g.holes[H_POPUP] = Some(keep);
    g.holes[H_REST] = Some(rect(13, y1, 401, 320 - y1));
    g.invalidate();
    g.flush(m);
    // The rows under the hole still show the graph as it is (nothing
    // renders while the popup covers the page): not repainted later.
    g.holes[H_REST] = None;
    g.d_holes[H_REST] = None;
}

/// Forget everything drawn (the next `update` draws the whole page).
pub fn invalidate() {
    let s = shown();
    s.m = None;
    s.cells_n = [255; 4];
    graph().invalidate();
}

/// Bring the page up to date with `m`: the chrome on a fresh page, then every
/// widget whose inputs changed, the strip markers and the graph.
pub fn update(m: &Model) {
    let s = shown();
    let old = s.m;
    let fresh = old.map_or(true, |o| o.combined != m.combined);
    if fresh {
        chrome(m);
        s.strip = [Mark::NONE; NSTRIP];
        s.cells_n = [255; 4];
        graph().invalidate();
    }
    let o = old.filter(|_| !fresh);
    let d = |a: bool| a || o.is_none();
    let ol = o.unwrap_or(*m);
    if d(ol.status != m.status) {
        status_badge(m);
    }
    if d(ol.tb != m.tb || ol.tb_flash != m.tb_flash) {
        tb_badge(m);
    }
    let pal = ol.colour != m.colour;
    if o.is_some_and(|o| o.src != m.src || o.rec != m.rec) {
        // The record bar moves (beside the badge / REC or not): clear the slot.
        lcd::fill_rect(REC_BAR.x as u16, 0, REC_BAR.w as u16, 20, C_BAR);
    }
    if d(ol.src != m.src || ol.rec != m.rec || ol.rec_unit != m.rec_unit || (m.src != SRC_NONE && pal)) {
        src_badge(m);
    } else if m.rec && ol.rec_dim != m.rec_dim {
        // The REC blink: its dot alone.
        rec_mark_blink(SRC_BADGE, C_BAR, !m.rec_dim, m.rec_unit);
    }
    if d(ol.roll != m.roll || ol.record != m.record || ol.status != m.status || ol.gen_hz != m.gen_hz || ol.src != m.src || ol.rec != m.rec) {
        record_bar(m);
    }
    if d(pal || ol.vdiv != m.vdiv || ol.probe != m.probe || ol.ac != m.ac || ol.wvdiv != m.wvdiv) {
        vdiv_badge(m);
    }
    if d(ol.battery != m.battery || ol.charging != m.charging || icons_dx(&ol) != icons_dx(m)) {
        battery(m);
    }
    if d(ol.usb != m.usb || ol.bt != m.bt) {
        usb_icon(m);
    }
    for k in 0..7 {
        let was = |mm: &Model| mm.pressed as usize == k + 1;
        let dep = match k {
            0 | 2 => pal,
            1 => pal || ol.vdiv != m.vdiv || ol.ch1_open != m.ch1_open || ol.wvdiv != m.wvdiv || ol.ch1_txt != m.ch1_txt,
            3 | 5 => ol.math_on != m.math_on || ol.math_colour != m.math_colour,
            4 => ol.vdiv != m.vdiv || ol.wvdiv != m.wvdiv || ol.math_open != m.math_open || ol.math_mode != m.math_mode || ol.math_on != m.math_on || ol.math_colour != m.math_colour || ol.math_scale != m.math_scale,
            _ => false,
        };
        if d(dep || was(&ol) != was(m)) {
            panel_button(m, k);
        }
    }
    if d(pal) {
        s.cells_n = [255; 4];
    }
    // Strip markers: repaint the rows of every marker that changed.
    let mut nm = [Mark::NONE; NSTRIP];
    let n = strip_marks(m, &mut nm);
    if nm != s.strip {
        let lo = m.top();
        let hi = lo + m.frame_h() - 1;
        let mut sp = Spans { s: [(0, 0); 12], n: 0 };
        for k in 0..NSTRIP {
            if nm[k] != s.strip[k] {
                for mk in [nm[k], s.strip[k]] {
                    if mk.some() {
                        sp.add((mk.y, mk.y + 11));
                    }
                }
            }
        }
        for &(a, b) in sp.merged(lo, hi) {
            strip_rows(&nm[..n], a, b);
        }
        s.strip = nm;
    }
    s.m = Some(*m);
    // The timebase tabs: holes in the graph, painted over it.
    let g = graph();
    let tabs = [(TAB_L, TAB_SHOW_L | TAB_PRESS_L), (TAB_R, TAB_SHOW_R | TAB_PRESS_R)];
    for (k, &(r, bits)) in tabs.iter().enumerate() {
        g.set_hole_at(H_TAB_L + k, (m.tabs & bits & (TAB_SHOW_L | TAB_SHOW_R) != 0).then_some(r));
    }
    // FFT: the stock centre-frequency label and the opendm40 boxes, holes too.
    let fft_on = m.math_on && m.math_mode == 5;
    let add = if fft_on { m.fft } else { None };
    g.set_hole_at(H_FFT_HZ, fft_on.then_some(fft_label_rect(m)));
    g.set_hole_at(H_FFT_Y, add.map(|_| fft_ydiv_rect(m)));
    g.set_hole_at(H_FFT_X, add.map(|_| FFT_XDIV));
    g.set_hole_at(H_FFT_PK, add.map(|_| fft_peak_rect(m)));
    g.set_hole_at(H_CUR, (m.cur_tab != 0).then_some(CUR_TAB));
    g.set_hole_at(H_DEC, (m.dec != 0).then(|| dec::band_rect(m.dec)));
    let ch2 = !m.ch2_txt.as_str().is_empty();
    g.set_hole_at(H_SPARE, ch2.then_some(ch2_label_rect(m)));
    let deco0 = g.d_deco;
    let valid0 = g.valid;
    g.flush(m);
    // The trigger badge after the graph: a level drag step repaints both
    // (the badge ~3.3 ms on the device model, emu --mips 110), the marker
    // under the finger first.
    if d(pal || ol.trig_v != m.trig_v || ol.rising != m.rising || ol.amps != m.amps) {
        trig_badge(m);
    }
    // A repainted graph (a popup closed: `uncover`) leaves its holes as
    // they were: the overlays in them are drawn again.
    if m.cur_tab != 0 && (!valid0 || d(ol.cur_tab != m.cur_tab)) {
        cur_tab(m.cur_tab);
    }
    for (k, &(_, bits)) in tabs.iter().enumerate() {
        if m.tabs & bits & (TAB_SHOW_L | TAB_SHOW_R) != 0 && (!valid0 || d((ol.tabs ^ m.tabs) & bits != 0 || ol.tb != m.tb)) {
            tb_tab(m, k == 0);
        }
    }
    // The decoder band: when what it shows changed, or the graph was
    // repainted under it.
    if m.dec != 0 && (!valid0 || d(ol.dec != m.dec || ol.dec_gen != m.dec_gen)) {
        dec::band(m.dec, m.dec_gen == 0);
    }
    // The label shows the graph background through it: repainted with it.
    let bg_changed = !valid0 || deco0 != g.d_deco;
    let was_fft = ol.math_on && ol.math_mode == 5;
    if fft_on && (bg_changed || d(!was_fft || ol.fft_hz != m.fft_hz || ol.math_colour != m.math_colour)) {
        fft_label(m);
    }
    if ch2 && (bg_changed || d(ol.ch2_txt != m.ch2_txt)) {
        fft_box(ch2_label_rect(m), m.ch2_txt.as_str(), C_CH2);
    }
    if let Some(a) = add {
        let old = if was_fft { ol.fft } else { None };
        let mc = m.math_text();
        let col = d(old.is_none() || ol.math_colour != m.math_colour) || bg_changed;
        if col || old.map_or(true, |o| o.ydiv != a.ydiv) {
            fft_box(fft_ydiv_rect(m), a.ydiv.as_str(), mc);
        }
        if col || old.map_or(true, |o| o.xdiv != a.xdiv) {
            fft_box(FFT_XDIV, a.xdiv.as_str(), mc);
        }
        if col || old.map_or(true, |o| o.peak != a.peak) {
            fft_box(fft_peak_rect(m), a.peak.as_str(), C_WHITE);
        }
    }
}

// ------------------------------------------------------------------ FFT
// Stock: the centre-frequency label (spec sec. 3.2). opendm40 additions
// (UF-11): the vertical scale box (top left), the horizontal scale box
// (bottom, between the timebase tap zones), the peak readout (right, under
// the label, white like the peak marker). All are graph holes painted over it.

/// Stock FFT label: graph (200,12,60,17).
pub fn fft_label_rect(m: &Model) -> Rect {
    rect(214, m.top() + 13, 60, 17)
}
pub fn fft_ydiv_rect(m: &Model) -> Rect {
    rect(18, m.top() + 13, 92, 17)
}
/// Peak readout: right, under the label row (the label leaves too little
/// room beside it for "12.5MHz -99.9dBV").
pub fn fft_peak_rect(m: &Model) -> Rect {
    rect(266, m.top() + 34, 144, 17)
}
/// Horizontal scale box: bottom row (both views share the frame bottom),
/// outside the Corners tap zones (x 23..102 / 333..412) and the cursor tab.
pub const FFT_XDIV: Rect = rect(168, 280, 92, 17);

/// The stock FFT label: no fill (the graph background shows through), 1 px
/// ring r5 with the top-left corner square (mask 0x0e), centred 12 px text,
/// both in the math colour.
pub fn fft_label(m: &Model) {
    let r = fft_label_rect(m);
    let mc = m.math_text();
    let t = Ink::new(sf::SC_F12, r, m.fft_hz.as_str(), 8, 0);
    let g = graph_ref();
    paint(r, &[(K_SHAPE, C_FILL), (K_TEXT, C_FILL)], |x, y| {
        if let Some(gl) = t.at(x, y) {
            return (mc, tag(2, gl));
        }
        if on_ring(r, 5, 0x0e, x, y) { (mc, tag(1, 0)) } else { (g.bg_at(x, y), 0) }
    });
}

/// An FFT box (opendm40): ring r5 and text in `col` on the graph fill (letter
/// spacing 1: "B/", "z/" touch in this font otherwise).
fn fft_box(r: Rect, text: &str, col: Color) {
    let t = Ink::new(sf::SC_F12, r, text, 8, 1);
    let g = graph_ref();
    paint(r, &[(K_SHAPE, C_FILL), (K_TEXT, C_FILL)], |x, y| {
        if let Some(gl) = t.at(x, y) {
            return (col, tag(2, gl));
        }
        if !in_rbox(r, 5, 0x0f, x, y) {
            (g.bg_at(x, y), 0)
        } else if on_ring(r, 5, 0x0f, x, y) {
            (col, tag(1, 0))
        } else {
            (C_FILL, tag(1, 0))
        }
    });
}

/// "50kHz/div", "200mHz/div": a 1-2-5 frequency without decimals.
pub fn fmt_hz125(out: &mut Buf, hz: f32) {
    let (x, p) = if hz >= 0.999e6 {
        (hz / 1e6, "MHz")
    } else if hz >= 999.0 {
        (hz / 1e3, "kHz")
    } else if hz >= 0.999 {
        (hz, "Hz")
    } else {
        (hz * 1e3, "mHz")
    };
    let mut t = [0u8; 16];
    out.push(crate::dmm::format(&mut t, x, if x < 0.999 { 1 } else { 0 })).push(p);
}

/// Measurement bar texts (<= 4, "--" for unused cells); only changed cells
/// are repainted.
pub fn cells(m: &Model, texts: &[&str]) {
    while cells_step(m, texts) {}
}

/// At most one changed cell of `cells` (~3 ms on the device per cell): true
/// if it drew one (more may follow), false when all four show `texts`. The
/// scope page spreads a measurement update over passes with it (device
/// check 2026-10-21: four cells in one pass took 12.3 ms).
pub fn cells_step(m: &Model, texts: &[&str]) -> bool {
    let s = shown();
    for i in 0..4 {
        let t = texts.get(i).copied().unwrap_or("--");
        let b = t.as_bytes();
        let n = b.len().min(16);
        if s.cells_n[i] as usize == n && s.cells[i][..n] == b[..n] {
            continue;
        }
        meas_cell(m, i, t);
        s.cells[i][..n].copy_from_slice(&b[..n]);
        s.cells_n[i] = n as u8;
        return true;
    }
    false
}

// ------------------------------------------------------------------ tap step
// opendm40 timebase tap step (docs/scope-scale-ux.md sec. 1): value-preview
// tabs, tap zones, the press decision, the hold repeat. Pure functions of
// the model and the touch so the host tests (tools/touch-test, ui-render)
// run exactly what the page runs.

/// A timebase preview tab: the measurement-cell style (r5 box in the bar
/// colour, 1 px ring) in the timebase badge's magenta; pressed / just
/// stepped: filled magenta. Text: the value the next tap on that side sets
/// ("<500us" / "100us>"). The pixels outside the rounded corners are the
/// graph fill (the graph leaves the tab rect alone: `Graph` holes).
pub fn tb_tab(m: &Model, left: bool) {
    let (r, press) = if left { (TAB_L, TAB_PRESS_L) } else { (TAB_R, TAB_PRESS_R) };
    let tb = m.tb.min(29) as usize;
    let mut b = Buf::new();
    if left {
        b.push("<").push(TB_LABELS[(tb + 1).min(29)]);
    } else {
        b.push(TB_LABELS[tb.saturating_sub(1)]).push(">");
    }
    let t = Ink::new(sf::SC_F12, r, b.as_str(), 8, 0);
    let pressed = m.tabs & press != 0;
    let fill = if pressed { C_MAGENTA } else { C_BAR };
    paint(r, &[(K_SHAPE, C_FILL), (K_TEXT, fill)], |x, y| {
        if let Some(g) = t.at(x, y) {
            return (C_WHITE, tag(2, g));
        }
        if !in_rbox(r, 5, 0x0f, x, y) {
            (C_FILL, 0)
        } else if on_ring(r, 5, 0x0f, x, y) {
            (C_MAGENTA, tag(1, 0))
        } else {
            (fill, tag(1, 0))
        }
    });
}

/// The cursor tab (UF-08), the timebase-tab style in the cursor colours:
/// grey ring, white "CUR" at rest; filled grey with "CUR X" / "CUR Y" while
/// those cursors are on.
pub fn cur_tab(state: u8) {
    let r = CUR_TAB;
    let txt = match state {
        CUR_X => "CUR X",
        CUR_Y => "CUR Y",
        _ => "CUR",
    };
    let on = state == CUR_X || state == CUR_Y;
    let t = Ink::new(sf::SC_F12, r, txt, 8, 1);
    let (fill, fg) = if on { (C_FRAME, C_FILL) } else { (C_BAR, C_WHITE) };
    paint(r, &[(K_SHAPE, C_FILL), (K_TEXT, fill)], |x, y| {
        if let Some(g) = t.at(x, y) {
            return (fg, tag(2, g));
        }
        if !in_rbox(r, 5, 0x0f, x, y) {
            (C_FILL, 0)
        } else if on_ring(r, 5, 0x0f, x, y) {
            (C_FRAME, tag(1, 0))
        } else {
            (fill, tag(1, 0))
        }
    });
}

/// Highest timebase index the scope accepts (scope.rs `apply`): roll on and
/// not Single = TB_MAX, else the YT maximum.
pub fn tb_top(roll: bool, single: bool) -> u8 {
    (if roll && !single { crate::scope::TB_MAX } else { crate::scope::TB_YT_MAX }) as u8
}

/// Timebase list pick of row `i` with the Roll setting `roll` and trigger
/// mode Single `single`: (the timebase to apply, turn the saved Roll setting
/// on). A roll-only row (beyond 1 s/div, orange) turns Roll on, except in
/// Single: it never rolls, so the scope takes the YT maximum and the saved
/// Roll setting is left alone (pre-flash review 621ac3f L8).
pub fn tb_pick(i: u8, roll: bool, single: bool) -> (u8, bool) {
    let roll_on = i as usize > crate::scope::TB_YT_MAX && !roll && !single;
    (i.min(tb_top(roll || roll_on, single)), roll_on)
}

/// `Model::tabs` for a tap-step `mode`: Corners always shows both tabs,
/// Halves while `transient` (2 s after a step, the first page entries) or
/// a press is armed, Off never. A tab whose step is at its limit is hidden
/// (slower at `tb_max`, faster at TB_MIN). `pressed`: the side armed by the
/// finger or just stepped (true = left / slower).
pub fn tabs_of(mode: u8, tb: u8, tb_max: u8, transient: bool, pressed: Option<bool>) -> u8 {
    let show = match mode {
        TAP_CORNERS => true,
        TAP_HALVES => transient || pressed.is_some(),
        _ => false,
    };
    if !show {
        return 0;
    }
    let mut b = 0;
    if tb < tb_max {
        b |= TAB_SHOW_L;
        if pressed == Some(true) {
            b |= TAB_PRESS_L;
        }
    }
    if tb as usize > crate::scope::TB_MIN {
        b |= TAB_SHOW_R;
        if pressed == Some(false) {
            b |= TAB_PRESS_R;
        }
    }
    b
}

/// The tap-step side of a press at (x, y) inside INNER that hit no marker:
/// Some(true) = slower, Some(false) = faster, None = no step there.
pub fn tap_zone(mode: u8, x: i16, y: i16) -> Option<bool> {
    match mode {
        TAP_HALVES if INNER.has(x, y) => Some(x <= 207),
        TAP_CORNERS if ZONE_L.has(x, y) => Some(true),
        TAP_CORNERS if ZONE_R.has(x, y) => Some(false),
        _ => None,
    }
}

/// What a press in the graph area does.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Press {
    /// A marker / cursor-line zone: select (level group, item id).
    Marker(bool, u8),
    /// No marker: the release steps the timebase (true = slower).
    Step(bool),
    /// The cursor tab (UF-08): the release cycles the cursors.
    Cur,
    /// Nothing (a drag still moves the selected item).
    None,
}

/// The page's press decision at (x, y) inside INNER: the object under the
/// finger wins: the nearest marker / cursor line
/// (`pick_near`), and only then the tap-step zone of `mode`.
pub fn graph_press(m: &Model, mode: u8, sel_v: u8, sel_h: u8, last_h: bool, x: i16, y: i16) -> Press {
    if let Some(q) = graph_pick(m, sel_v, sel_h, last_h, x, y) {
        return Press::Marker(q.level, q.id);
    }
    if m.cur_tab != 0 && rect(CUR_TAB.x - 3, CUR_TAB.y - 6, CUR_TAB.w + 6, CUR_TAB.h + 6).has(x, y) {
        return Press::Cur;
    }
    match tap_zone(mode, x, y) {
        Some(s) => Press::Step(s),
        None => Press::None,
    }
}

/// Tappable top-bar badge under (x, y) (y < 20, the badge's x span): 0 run
/// status (RUN/STOP), 1 timebase (list popup), 2 V/div (CH1 popup), 3
/// trigger (Trigger tab). Tested before the graph area (INNER starts at y
/// 15): a badge wins over the top 5 rows of a marker zone there.
pub fn top_badge(x: u16, y: u16) -> Option<u8> {
    if y >= 20 {
        return None;
    }
    let x = x as i16;
    // (The source badge: `src_hit`, the app knows whether it is shown.)
    [RUN_BADGE, TB_BADGE, VDIV_BADGE, TRIG_BADGE].iter().position(|r| x >= r.x && x < r.x + r.w).map(|k| k as u8)
}

/// The source badge (when shown) was hit.
pub fn src_hit(x: u16, y: u16) -> bool {
    let r = SRC_BADGE;
    y < 20 && (x as i16) >= r.x && (x as i16) < r.x + r.w
}

/// Corners hold repeat: the `done`-th repeat (0-based) of a press at `t0`
/// is due at t0 + REPEAT_FIRST_MS + done * REPEAT_MS.
pub fn repeat_due(t0: u64, done: u32, now: u64) -> bool {
    now >= t0 + REPEAT_FIRST_MS + done as u64 * REPEAT_MS
}

/// The tap step of one press (scope_page Touch): the side `graph_press`
/// armed (None: a marker or no zone), the press time, the hold repeats done.
/// `still`: the touch has not left the tap tolerance and is not a drag.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Arm {
    pub side: Option<bool>,
    pub t0: u64,
    pub rep: u8,
}

impl Arm {
    pub const NONE: Arm = Arm { side: None, t0: 0, rep: 0 };

    /// Per main-loop pass while the finger is down: the side to step now
    /// (Corners only: a still hold, first after REPEAT_FIRST_MS, then every
    /// REPEAT_MS).
    pub fn repeat(&mut self, mode: u8, still: bool, now: u64) -> Option<bool> {
        let s = self.side?;
        if mode != TAP_CORNERS || !still || !repeat_due(self.t0, self.rep as u32, now) {
            return None;
        }
        self.rep = self.rep.saturating_add(1);
        Some(s)
    }

    /// The step of the release: a still tap that has not repeated.
    pub fn release(&self, still: bool) -> Option<bool> {
        if still && self.rep == 0 { self.side } else { None }
    }

    /// The side shown pressed while the finger is down (a still press that
    /// has not repeated: it previews what the release does).
    pub fn armed(&self, still: bool) -> Option<bool> {
        self.release(still)
    }
}

// ------------------------------------------------------------------ popups
// Stock popups (sec. 7): MF settings popup (Measure / Display / Trigger /
// Settings), CH1 popup (Coupling / Probe / Color), Math popup (inert: no
// math engine). Drawn by composition like the page; AA off.

pub const MEAS_NAMES: [&str; 20] =
    ["Freq", "+Duty", "-Duty", "+Wid", "-Wid", "+Pul", "-Pul", "X2-1", "Xf", "Cycl", "Vp-p", "Vrms", "Vmax", "Vmin", "Vtop", "Vbas", "Vamp", "Y2-1", "Vcrm", "Vavg"];
pub const PROBE_NAMES: [&str; 13] = ["0.1X", "0.2X", "0.5X", "1X", "2X", "5X", "10X", "20X", "50X", "100X", "200X", "500X", "1000X"];
/// FFT windows (stock five, "Hanming" spelled Hamming, + flat-top; the same
/// order as fft::WIN_NAMES).
pub const WINDOWS: [&str; 6] = ["Rectangular", "Blackman", "Hanning", "Hamming", "Triangular", "Flat-top"];
/// The MF popup's tabs (stock four, opendm40 Decode).
pub const MF_TABS: [&str; 5] = ["Measure", "Display", "Trigger", "Settings", "Decode"];
/// Tab pitch (stock four tabs: 101 px; five: 80).
const MF_TAB_W: i16 = 80;

/// Values the popups show.
#[derive(Clone, Copy)]
pub struct PopVals {
    pub meas: [u8; 4],
    pub meas_n: u8,
    pub roll: bool,
    /// 0 OFF 1 MIN 2 1S 3 INF.
    pub persist: u8,
    pub bright: u8,
    pub grid: u8,
    pub rising: bool,
    /// 0 Auto 1 Normal 2 Single.
    pub mode: u8,
    pub ac: bool,
    pub probe: u8,
    pub colour: u8,
    pub math_colour: u8,
    /// Measure grid cell under the finger (0x7bef while pressed).
    pub pressed_cell: Option<u8>,
    /// Math popup: mode (5 FFT, 6 ABS), Hz/div choice 0..3 and its four
    /// labels (ascending), FFT window, FFT unit Vrms.
    pub math_mode: u8,
    pub fft_hz: u8,
    pub hz_opts: [Txt; 4],
    pub fft_win: u8,
    pub fft_vrms: bool,
    /// Trigger type, pulse condition and width (s), runt second level (V).
    pub ttype: u8,
    pub pcond: u8,
    pub pwidth: f32,
    pub runt_v: f32,
    /// The decoder setup popup's values.
    pub dec: dec::SetupVals,
    /// Trigger source (scope::TSRC_*) and the GEN trace shown (opendm40,
    /// docs/scope-gen-channel.md).
    pub tsrc: u8,
    pub gen_trace: bool,
    /// The DMM CH2 shown and its label ("CH2 1.5V"; empty before its
    /// first start).
    pub ch2_on: bool,
    pub ch2_vdiv: Txt,
}

/// Which popup is open (tab).
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Popup {
    None,
    Mf(u8),
    Ch1(u8),
    Math(u8),
    /// opendm40 timebase list (tap on the timebase badge): the timebase index
    /// in the list's centre band.
    Tb(u8),
}

/// Intersection of two rects (w / h 0 when disjoint).
fn isect(a: Rect, b: Rect) -> Rect {
    let x0 = a.x.max(b.x);
    let y0 = a.y.max(b.y);
    let x1 = (a.x + a.w).min(b.x + b.w);
    let y1 = (a.y + a.h).min(b.y + b.h);
    rect(x0, y0, (x1 - x0).max(0), (y1 - y0).max(0))
}

fn fill_c(r: Rect, clip: Rect, c: Color) {
    let r = isect(r, clip);
    if r.w > 0 && r.h > 0 {
        lcd::fill_rect(r.x as u16, r.y as u16, r.w as u16, r.h as u16, c);
    }
}

/// A plain fill `r` (clipped) with a text on top.
fn fill_text(r: Rect, clip: Rect, bg: Color, font: Font, tr: Rect, text: &str, fg: Color, align: u8, sp: i8) {
    fill_text_on(r, clip, bg, bg, font, tr, text, fg, align, sp)
}

/// `fill_text` of a button-like fill on a `parent` colour (the fill is a
/// probed shape for the host hit-map checks when it differs from it).
fn fill_text_on(r: Rect, clip: Rect, bg: Color, parent: Color, font: Font, tr: Rect, text: &str, fg: Color, align: u8, sp: i8) {
    let t = Ink::new(font, tr, text, align, sp);
    paint(isect(r, clip), &[(K_TEXT, bg), (K_SHAPE, parent)], |x, y| match t.at(x, y) {
        Some(g) => (fg, tag(1, g)),
        None => (bg, if bg != parent { tag(2, 0) } else { 0 }),
    });
}

/// A rounded box (on `outside`) with a centred text.
fn rbox_text(r: Rect, clip: Rect, radius: i16, fill: Color, outside: Color, font: Font, text: &str, fg: Color) {
    let t = Ink::new(font, r, text, 8, 0);
    paint(isect(r, clip), &[(K_SHAPE, outside), (K_TEXT, fill)], |x, y| {
        if let Some(g) = t.at(x, y) {
            return (fg, tag(2, g));
        }
        if in_rbox(r, radius, 0x0f, x, y) { (fill, tag(1, 0)) } else { (outside, 0) }
    });
}

/// Option list FUN_08026cc8: frame fill, items 1 px inset at x = 1 + i *
/// item_w, the last item fills the rest; selected 0x059e / white text.
fn option_list(r: Rect, clip: Rect, items: &[&str], item_w: i16, sel: usize) {
    fill_c(r, clip, C_BAR);
    for (i, t) in items.iter().enumerate() {
        let x = 1 + i as i16 * item_w;
        let w = if i + 1 == items.len() { r.w - x - 1 } else { item_w };
        let ir = rect(r.x + x, r.y + 1, w, r.h - 2);
        let (bg, fg) = if i == sel { (C_SEL, C_WHITE) } else { (C_POP_BG, C_BAR) };
        fill_text_on(ir, clip, bg, C_BAR, sf::SC_F16, ir, t, fg, 8, 0);
    }
}

/// Item of an option list under x (with the items' x layout), if inside.
fn option_hit(r: Rect, n: usize, item_w: i16, x: i16, y: i16) -> Option<u8> {
    if !rect(r.x - 2, r.y - 3, r.w + 4, r.h + 6).has(x, y) {
        return None;
    }
    Some(((x - r.x - 1).max(0) / item_w).min(n as i16 - 1) as u8)
}

/// Spinner FUN_080274b2: 5 rows of 30 px, the current item in the band;
/// `colours`: names drawn in their palette colour (text box 56 x 13).
fn spinner(r: Rect, clip: Rect, items: &[&str], cur: usize, colours: bool) {
    let rows = r.h / 30;
    let band = rect(r.x, r.y + (r.h - 30) / 2, r.w, 30);
    let mut ink = [Ink::none(); 5];
    let mut col = [0u16; 5];
    for k in 0..rows.min(5) {
        let idx = cur as i16 - rows / 2 + k;
        if idx < 0 || idx >= items.len() as i16 {
            continue;
        }
        let y = r.y + k * 30;
        let in_band = y == band.y;
        col[k as usize] = if colours { PALETTE[idx as usize % 9] } else if in_band { C_WHITE } else { C_POP_BG };
        let tr = if colours { rect(r.x + (r.w - 56) / 2, y + (30 - 13) / 2, 56, 13) } else { rect(r.x, y, r.w, 30) };
        ink[k as usize] = Ink::new(sf::SC_F16, tr, items[idx as usize], 8, 0);
    }
    paint(isect(r, clip), &[(K_SHAPE, C_POP_BG), (K_TEXT, C_BAR)], |x, y| {
        for k in 0..5 {
            if let Some(g) = ink[k].at(x, y) {
                return (col[k], tag(2, g));
            }
        }
        if band.has(x, y) {
            (C_SEL, tag(1, 0))
        } else if in_rbox(r, 8, 0x0f, x, y) {
            (C_BAR, tag(1, 0))
        } else {
            (C_POP_BG, 0)
        }
    });
}

/// Spinner row under (x, y): the item index it shows.
fn spinner_hit(r: Rect, n: usize, cur: usize, x: i16, y: i16) -> Option<u8> {
    if !r.has(x, y) {
        return None;
    }
    let idx = cur as i16 - (r.h / 30) / 2 + (y - r.y) / 30;
    if idx < 0 || idx >= n as i16 { None } else { Some(idx as u8) }
}

/// Slider FUN_0802639a: track r5 0x2124 border, filled 0x059e left of the
/// knob / 0xd679 right, knob 10 x 24 with a white 8 x 22 inner; pos =
/// int(130 * value).
fn slider(r: Rect, clip: Rect, pos: i16) {
    let track = rect(r.x, r.y + (r.h - 10) / 2, r.w, 10);
    let inner = track.inset(1);
    let split = r.x + pos + 5;
    let knob = rect(r.x + pos, r.y + (r.h - 24) / 2, 10, 24);
    let ki = knob.inset(1);
    paint(isect(r, clip), &[(K_SHAPE, C_POP_BG)], |x, y| {
        let c = if ki.has(x, y) {
            C_WHITE
        } else if knob.has(x, y) {
            C_FILL
        } else if in_rbox(inner, 5, 0x0f, x, y) {
            if x < split { C_SEL } else { C_POP_BG }
        } else if in_rbox(track, 5, 0x0f, x, y) {
            C_FILL
        } else {
            return (C_POP_BG, 0);
        };
        (c, tag(1, 0))
    });
}

/// Slider value (0..1) of a touch at x.
pub fn slider_value(x: i16) -> f32 {
    ((x - 214 - 5) as f32 / 130.0).clamp(0.0, 1.0)
}

/// MF popup page height per tab (box h = 40 + page).
/// (Trigger: stock 55; opendm40 125 with the trigger-type rows, UF-13;
/// Decode: ui_decode.rs.)
const MF_PAGE_H: [i16; 5] = [150, 200, 175, 55, dec::PAGE_H];

/// The box a popup paints whole: a graph hole while it is open (a full
/// repaint under an open popup does not draw the graph there first).
/// None: no hole (the timebase list).
pub fn popup_box(p: Popup) -> Option<Rect> {
    match p {
        Popup::Mf(t) => Some(rect(13, 20, 401, 40 + MF_PAGE_H[t.min(4) as usize])),
        Popup::Ch1(_) | Popup::Math(_) => Some(rect(214, 20, 200, 281)),
        _ => None,
    }
}

/// MF settings popup (FUN_08037574), tab 0..4.
pub fn draw_mf(tab: u8, v: &PopVals) {
    let tab = tab.min(4);
    let page_h = MF_PAGE_H[tab as usize];
    let hh = 40 + page_h;
    let screen = rect(0, 0, 480, 320);
    fill_c(rect(13, 20, 401, hh), screen, C_FRAME);
    let tclip = rect(14, 21, 399, hh - 1);
    // The stock tab font when every label fits the narrower tabs.
    let font = if MF_TABS.iter().all(|t| sf::text_width(sf::SC_F22, t, 0) + 6 <= MF_TAB_W - 1) { sf::SC_F22 } else { sf::SC_F16 };
    for i in 0..5i16 {
        let x = 14 + MF_TAB_W * i;
        let r = rect(x, 21, MF_TAB_W - 1, 40);
        let sel = i == tab as i16;
        let ul = rect(x, 59, MF_TAB_W - 1, 2);
        let t = Ink::new(font, r, MF_TABS[i as usize], 8, 0);

        let (bg, fg) = if sel { (C_POP_BG, C_FILL) } else { (C_POP_TAB, C_WHITE) };
        paint(isect(r, tclip), &[(K_TEXT, bg), (K_SHAPE, C_FRAME)], |x, y| {
            if let Some(g) = t.at(x, y) {
                return (fg, tag(1, g));
            }
            (if sel && ul.has(x, y) { C_POP_TAB } else { bg }, tag(2, 0))
        });
    }
    let pc = rect(14, 61, 399, page_h - 1);
    match tab {
        0 => {
            fill_c(rect(15, 61, 398, 110), pc, C_POP_BG);
            for i in 0..20i16 {
                let r = rect(16 + 57 * (i % 7), 66 + 35 * (i / 7), 55, 30);
                let c = if v.pressed_cell == Some(i as u8) { C_DIS } else { C_BAR };
                rbox_text(r, pc, 8, c, C_POP_BG, sf::SC_F16, MEAS_NAMES[i as usize], C_WHITE);
            }
            fill_c(rect(15, 171, 398, 40), pc, C_POP_BG);
            for k in 0..v.meas_n.min(4) as usize {
                let r = rect(16 + 57 * k as i16, 176, 55, 30);
                rbox_text(r, pc, 8, C_BAR, C_POP_BG, sf::SC_F16, MEAS_NAMES[v.meas[k] as usize % 20], C_YELLOW);
            }
            rbox_text(rect(281, 176, 60, 30), pc, 8, C_ORANGE, C_POP_BG, sf::SC_F16, "Clear", C_WHITE);
            rbox_text(rect(348, 176, 60, 30), pc, 8, C_PURPLE, C_POP_BG, sf::SC_F16, "Default", C_WHITE);
        }
        1 => {
            fill_c(rect(14, 61, 399, 100), pc, C_POP_BG);
            for (r, t) in [(rect(24, 61, 189, 20), "Roll:"), (rect(24, 105, 189, 20), "Persist:"), (rect(214, 61, 189, 20), "Brightness:"), (rect(214, 105, 189, 20), "Grid intensity:")] {
                fill_text(r, pc, C_POP_BG, sf::SC_F16, r, t, C_FILL, 1, 0);
            }
            option_list(rect(24, 81, 88, 24), pc, &["On", "Off"], 44, if v.roll { 0 } else { 1 });
            option_list(rect(24, 125, 176, 24), pc, &["OFF", "MIN", "1S", "INF"], 44, v.persist as usize);
            mf_display_values(v);
            // opendm40: the computed generator trace.
            fill_c(rect(14, 161, 399, 50), pc, C_POP_BG);
            fill_text(GT_LABEL, pc, C_POP_BG, sf::SC_F16, GT_LABEL, "GEN trace:", C_FILL, 1, 0);
            option_list(GT_LIST, pc, &["On", "Off"], 44, if v.gen_trace { 0 } else { 1 });
            // opendm40: the DMM as CH2 and its V/div.
            fill_c(rect(14, 211, 399, 49), pc, C_POP_BG);
            fill_text(C2_LABEL, pc, C_POP_BG, sf::SC_F16, C2_LABEL, "CH2 (DMM):", C_FILL, 1, 0);
            option_list(C2_LIST, pc, &["On", "Off"], 44, if v.ch2_on { 0 } else { 1 });
            if v.ch2_on {
                rbox_text(C2_MINUS, pc, 8, C_BAR, C_POP_BG, sf::SC_F16, "-", C_WHITE);
                rbox_text(C2_PLUS, pc, 8, C_BAR, C_POP_BG, sf::SC_F16, "+", C_WHITE);
                fill_text(C2_VALUE, pc, C_POP_BG, sf::SC_F16, C2_VALUE, v.ch2_vdiv.as_str(), C_FILL, 8, 0);
            }
        }
        2 => {
            fill_c(rect(14, 61, 399, 55), pc, C_POP_BG);
            fill_text(rect(24, 61, 189, 20), pc, C_POP_BG, sf::SC_F16, rect(24, 61, 189, 20), "Edge:", C_FILL, 1, 0);
            fill_text(rect(214, 61, 189, 20), pc, C_POP_BG, sf::SC_F16, rect(214, 61, 189, 20), "Mode:", C_FILL, 1, 0);
            option_list(rect(24, 81, 120, 24), pc, &["Rising", "Falling"], 60, if v.rising { 0 } else { 1 });
            option_list(rect(214, 81, 180, 24), pc, &["Auto", "Normal", "Single"], 60, v.mode as usize);
            // opendm40 (UF-13): trigger type and its parameters.
            fill_c(rect(14, 116, 399, 70), pc, C_POP_BG);
            fill_text(TT_LABEL, pc, C_POP_BG, sf::SC_F16, TT_LABEL, "Type:", C_FILL, 1, 0);
            option_list(TT_LIST, pc, &crate::trigsoft::TYPE_NAMES, 60, v.ttype.min(2) as usize);
            match v.ttype {
                crate::trigsoft::PULSE => {
                    fill_text(TP_LABEL, pc, C_POP_BG, sf::SC_F16, TP_LABEL, "Width:", C_FILL, 1, 0);
                    option_list(TP_COND, pc, &crate::trigsoft::COND_NAMES, 30, v.pcond.min(2) as usize);
                    rbox_text(TP_MINUS, pc, 8, C_BAR, C_POP_BG, sf::SC_F16, "-", C_WHITE);
                    rbox_text(TP_PLUS, pc, 8, C_BAR, C_POP_BG, sf::SC_F16, "+", C_WHITE);
                    let mut b = Buf::new();
                    fmt_s125(&mut b, v.pwidth);
                    fill_text(TP_VALUE, pc, C_POP_BG, sf::SC_F16, TP_VALUE, b.as_str(), C_FILL, 8, 0);
                }
                crate::trigsoft::RUNT => {
                    fill_text(TP_LABEL, pc, C_POP_BG, sf::SC_F16, TP_LABEL, "2nd level:", C_FILL, 1, 0);
                    let mut b = Buf::new();
                    fmt_v(&mut b, v.runt_v, "V");
                    fill_text(TP_COND, pc, C_POP_BG, sf::SC_F16, TP_COND, b.as_str(), C_ORANGE, 1, 0);
                    fill_text(TR_HINT, pc, C_POP_BG, sf::SC_F16, TR_HINT, "drag orange T", C_FILL, 1, 0);
                }
                _ => {}
            }
            // opendm40: the trigger source (the generator computed, edge only).
            fill_c(rect(14, 186, 399, 50), pc, C_POP_BG);
            fill_text(TS_LABEL, pc, C_POP_BG, sf::SC_F16, TS_LABEL, "Source:", C_FILL, 1, 0);
            option_list(TS_LIST, pc, &crate::scope::TSRC_NAMES, 60, v.tsrc.min(1) as usize);
            if v.tsrc == crate::scope::TSRC_GEN {
                fill_text(TS_NOTE, pc, C_POP_BG, sf::SC_F16, TS_NOTE, "edge, computed", C_FILL, 1, 0);
            }
        }
        3 => {
            fill_c(rect(14, 61, 399, 55), pc, C_POP_BG);
            rbox_text(rect(24, 78, 80, 30), pc, 8, C_BAR, C_POP_BG, sf::SC_F16, "PriScr", C_WHITE);
            rbox_text(rect(114, 78, 80, 30), pc, 8, C_BAR, C_POP_BG, sf::SC_F16, "Save CSV", C_WHITE);
        }
        // opendm40: the protocol decoder (ui_decode.rs).
        _ => dec::page(&v.dec, pc),
    }
}

/// Display tab: the two sliders and their values (repainted while dragged).
// Trigger tab rows (opendm40, UF-13).
pub const TT_LABEL: Rect = rect(24, 116, 180, 20);
pub const TT_LIST: Rect = rect(24, 136, 180, 24);
pub const TP_LABEL: Rect = rect(214, 116, 189, 20);
pub const TP_COND: Rect = rect(214, 136, 92, 24);
pub const TP_MINUS: Rect = rect(214, 164, 36, 18);
pub const TP_VALUE: Rect = rect(254, 164, 104, 18);
pub const TP_PLUS: Rect = rect(362, 164, 36, 18);
pub const TR_HINT: Rect = rect(214, 164, 189, 18);
// Trigger source row, Display tab GEN trace row (opendm40).
pub const TS_LABEL: Rect = rect(24, 188, 180, 20);
pub const TS_LIST: Rect = rect(24, 208, 120, 24);
pub const TS_NOTE: Rect = rect(214, 208, 189, 24);
pub const GT_LABEL: Rect = rect(24, 163, 180, 20);
pub const GT_LIST: Rect = rect(24, 183, 88, 24);
pub const C2_LABEL: Rect = rect(24, 213, 180, 20);
pub const C2_LIST: Rect = rect(24, 233, 88, 24);
pub const C2_MINUS: Rect = rect(214, 235, 36, 20);
pub const C2_VALUE: Rect = rect(254, 235, 104, 20);
pub const C2_PLUS: Rect = rect(362, 235, 36, 20);
/// The DMM CH2 label in the graph (a hole, below the FFT's V/div box).
pub fn ch2_label_rect(m: &Model) -> Rect {
    rect(18, m.top() + 34, 92, 17)
}

/// "500ns", "20us", "1ms", "2s": a 1-2-5 time (integer in its unit).
pub fn fmt_s125(out: &mut Buf, s: f32) {
    let (x, u) = if s >= 0.999 {
        (s, "s")
    } else if s >= 0.999e-3 {
        (s * 1e3, "ms")
    } else if s >= 0.999e-6 {
        (s * 1e6, "us")
    } else {
        (s * 1e9, "ns")
    };
    let mut t = [0u8; 16];
    out.push(crate::dmm::format(&mut t, x + 0.001, 0)).push(u);
}

pub fn mf_display_values(v: &PopVals) {
    let pc = rect(14, 61, 399, MF_PAGE_H[1] - 1);
    for (y, val) in [(81i16, v.bright), (125, v.grid)] {
        slider(rect(214, y, 140, 24), pc, (130 * val.min(100) as i16) / 100);
        let mut b = Buf::new();
        let mut t = [0u8; 16];
        b.push(crate::dmm::format(&mut t, val as f32, 0)).push("%");
        let r = rect(364, y, 49, 24);
        fill_text(r, pc, C_POP_BG, sf::SC_F16, r, b.as_str(), C_FILL, 1, 0);
    }
}

/// What a touch on the MF popup hits.
#[derive(Clone, Copy, PartialEq)]
pub enum MfHit {
    Tab(u8),
    Cell(u8),
    SelCell(u8),
    Clear,
    Default,
    Roll(bool),
    Persist(u8),
    Bright,
    Grid,
    Rising(bool),
    Mode(u8),
    /// Trigger type, pulse condition, width step (true = up).
    TType(u8),
    PCond(u8),
    Width(bool),
    /// Trigger source, GEN trace on (opendm40).
    TSrc(u8),
    GenTrace(bool),
    /// The DMM CH2 on, its V/div step (true = up).
    Ch2(bool),
    Ch2Step(bool),
    PriScr,
    SaveCsv,
    /// The Decode tab's page.
    Dec(dec::SetupHit),
    /// Inside the box, nothing there.
    Inside,
    /// Below the box: closes (stock: x >= 10).
    Close,
    /// Elsewhere outside (ignored, modal).
    Outside,
}

pub fn mf_hit(tab: u8, v: &PopVals, x: i16, y: i16) -> MfHit {
    let tab = tab.min(4);
    let hh = 40 + MF_PAGE_H[tab as usize];
    if !rect(13, 20, 401, hh).has(x, y) {
        return if y >= 20 + hh && x >= 10 { MfHit::Close } else { MfHit::Outside };
    }
    if y < 61 {
        return MfHit::Tab(((x - 14) / MF_TAB_W).clamp(0, 4) as u8);
    }
    let near = |r: Rect| rect(r.x - 1, r.y - 2, r.w + 2, r.h + 4).has(x, y);
    match tab {
        0 => {
            // Each grid cell owns the gaps around it (pitch 57 x 35).
            if y < 171 {
                let (c, r) = (((x - 15) / 57).clamp(0, 6), ((y - 64) / 35).clamp(0, 2));
                let i = r * 7 + c;
                return if i < 20 { MfHit::Cell(i as u8) } else { MfHit::Inside };
            }
            let k = ((x - 15) / 57).max(0);
            if x < 278 {
                if k < v.meas_n.min(4) as i16 { MfHit::SelCell(k as u8) } else { MfHit::Inside }
            } else if x < 345 {
                MfHit::Clear
            } else {
                MfHit::Default
            }
        }
        1 => {
            if let Some(i) = option_hit(rect(24, 81, 88, 24), 2, 44, x, y) {
                MfHit::Roll(i == 0)
            } else if let Some(i) = option_hit(rect(24, 125, 176, 24), 4, 44, x, y) {
                MfHit::Persist(i)
            } else if rect(204, 75, 210, 36).has(x, y) {
                MfHit::Bright
            } else if rect(204, 119, 210, 36).has(x, y) {
                MfHit::Grid
            } else if let Some(i) = option_hit(GT_LIST, 2, 44, x, y) {
                MfHit::GenTrace(i == 0)
            } else if let Some(i) = option_hit(C2_LIST, 2, 44, x, y) {
                MfHit::Ch2(i == 0)
            } else if v.ch2_on && near(C2_MINUS) {
                MfHit::Ch2Step(false)
            } else if v.ch2_on && near(C2_PLUS) {
                MfHit::Ch2Step(true)
            } else {
                MfHit::Inside
            }
        }
        2 => {
            if let Some(i) = option_hit(rect(24, 81, 120, 24), 2, 60, x, y) {
                MfHit::Rising(i == 0)
            } else if let Some(i) = option_hit(rect(214, 81, 180, 24), 3, 60, x, y) {
                MfHit::Mode(i)
            } else if let Some(i) = option_hit(TT_LIST, 3, 60, x, y) {
                MfHit::TType(i)
            } else if let Some(i) = option_hit(TS_LIST, 2, 60, x, y) {
                MfHit::TSrc(i)
            } else if v.ttype == crate::trigsoft::PULSE {
                if let Some(i) = option_hit(TP_COND, 3, 30, x, y) {
                    MfHit::PCond(i)
                } else if near(TP_MINUS) {
                    MfHit::Width(false)
                } else if near(TP_PLUS) {
                    MfHit::Width(true)
                } else {
                    MfHit::Inside
                }
            } else {
                MfHit::Inside
            }
        }
        3 => {
            if near(rect(24, 78, 80, 30)) {
                MfHit::PriScr
            } else if near(rect(114, 78, 80, 30)) {
                MfHit::SaveCsv
            } else {
                MfHit::Inside
            }
        }
        _ => MfHit::Dec(dec::page_hit(x, y)),
    }
}

/// CH1 / Math popup frame: box, header, vertical tabs, page area.
fn side_popup(title: &str, tabs: &[&str], tab: u8) {
    let screen = rect(0, 0, 480, 320);
    fill_c(rect(214, 20, 200, 281), screen, C_FRAME);
    let h = rect(215, 21, 198, 29);
    fill_text(h, screen, C_POP_TAB, sf::SC_F16, h, title, C_YELLOW, 8, 0);
    fill_c(rect(215, 50, 198, 250), screen, C_FRAME);
    for (i, t) in tabs.iter().enumerate() {
        let y = 50 + 41 * i as i16;
        let r = rect(215, y, 78, 40);
        let sel = i == tab as usize;
        let ink = Ink::new(sf::SC_F16, r, t, 8, 1);
        let (bg, fg) = if sel { (C_POP_BG, C_BAR) } else { (C_BAR, C_WHITE) };
        let ind = rect(291, y, 2, 40);
        paint(r, &[(K_TEXT, bg), (K_SHAPE, C_FRAME)], |x, y| {
            if let Some(g) = ink.at(x, y) {
                return (fg, tag(1, g));
            }
            (if sel && ind.has(x, y) { C_BAR } else { bg }, tag(2, 0))
        });
    }
    fill_c(rect(293, 50, 120, 250), screen, C_POP_BG);
}

pub fn draw_ch1(tab: u8, v: &PopVals) {
    let tab = tab.min(2);
    side_popup("CHA", &["Coupling", "Probe", "Color"], tab);
    let pc = rect(293, 50, 120, 250);
    match tab {
        0 => option_list(rect(308, 71, 90, 24), pc, &["DC", "AC"], 44, v.ac as usize),
        1 => {
            let r = rect(293, 50, 120, 20);
            fill_text(r, pc, C_POP_BG, sf::SC_F16, r, "Scale", C_BAR, 8, 0);
            // Stock also shows "Type Vol / Cur" here; it changes nothing
            // (opendm40 reads the Wave view's current from the DMM), so it
            // is left out.
            spinner(rect(298, 70, 110, 150), pc, &PROBE_NAMES, v.probe as usize, false);
        }
        _ => spinner(rect(298, 70, 110, 150), pc, &COLOUR_NAMES, v.colour as usize, true),
    }
}

/// Math popup (stock layout): Mode FFT / ABS, Color, Hz/Div (four values
/// from the current FFT span), Other (window, unit).
pub fn draw_math(tab: u8, v: &PopVals) {
    let tab = tab.min(3);
    side_popup("MATH", &["Mode", "Color", "Hz/Div", "Other"], tab);
    let pc = rect(293, 50, 120, 250);
    match tab {
        // Stock: FFT selected unless the mode is ABS (also while math is off).
        0 => option_list(rect(308, 71, 90, 24), pc, &["FFT", "ABS"], 44, (v.math_mode == 6) as usize),
        1 => spinner(rect(298, 70, 110, 150), pc, &COLOUR_NAMES, v.math_colour as usize, true),
        2 => {
            let o = [v.hz_opts[0].as_str(), v.hz_opts[1].as_str(), v.hz_opts[2].as_str(), v.hz_opts[3].as_str()];
            spinner(rect(298, 70, 110, 150), pc, &o, v.fft_hz.min(3) as usize, false)
        }
        _ => {
            let r = rect(293, 50, 120, 20);
            fill_text(r, pc, C_POP_BG, sf::SC_F16, r, "Window", C_BAR, 8, 0);
            spinner(rect(298, 70, 110, 150), pc, &WINDOWS, v.fft_win.min(5) as usize, false);
            let r = rect(293, 224, 120, 24);
            fill_text(r, pc, C_POP_BG, sf::SC_F16, r, "FFT Unit", C_BAR, 8, 0);
            option_list(rect(308, 250, 90, 24), pc, &["dB", "Vrms"], 44, v.fft_vrms as usize);
        }
    }
}

/// What a touch on the CH1 / Math popup hits.
#[derive(Clone, Copy, PartialEq)]
pub enum SideHit {
    Tab(u8),
    /// CH1: coupling 0 DC 1 AC.
    Coupling(u8),
    Probe(u8),
    Colour(u8),
    /// Math popup: mode 5 FFT / 6 ABS, colour, Hz/div choice, window, unit
    /// (1 = Vrms).
    MathMode(u8),
    MathColour(u8),
    HzDiv(u8),
    Window(u8),
    Unit(u8),
    Inside,
    /// Outside the box: closes.
    Close,
}

pub fn side_hit(math: bool, tab: u8, v: &PopVals, x: i16, y: i16) -> SideHit {
    if !rect(214, 20, 200, 281).has(x, y) {
        return SideHit::Close;
    }
    let ntabs = if math { 4 } else { 3 };
    if x < 293 && y >= 50 {
        let i = (y - 50) / 41;
        return if i < ntabs { SideHit::Tab(i as u8) } else { SideHit::Inside };
    }
    if math {
        return match tab {
            0 => option_hit(rect(308, 71, 90, 24), 2, 44, x, y).map_or(SideHit::Inside, |i| SideHit::MathMode(5 + i)),
            1 => spinner_hit(rect(298, 70, 110, 150), 9, v.math_colour as usize, x, y).map_or(SideHit::Inside, SideHit::MathColour),
            2 => spinner_hit(rect(298, 70, 110, 150), 4, v.fft_hz.min(3) as usize, x, y).map_or(SideHit::Inside, SideHit::HzDiv),
            _ => {
                if let Some(i) = spinner_hit(rect(298, 70, 110, 150), WINDOWS.len(), v.fft_win.min(5) as usize, x, y) {
                    SideHit::Window(i)
                } else {
                    option_hit(rect(308, 250, 90, 24), 2, 44, x, y).map_or(SideHit::Inside, SideHit::Unit)
                }
            }
        };
    }
    match tab {
        0 => option_hit(rect(308, 71, 90, 24), 2, 44, x, y).map_or(SideHit::Inside, SideHit::Coupling),
        1 => spinner_hit(rect(298, 70, 110, 150), 13, v.probe as usize, x, y).map_or(SideHit::Inside, SideHit::Probe),
        _ => spinner_hit(rect(298, 70, 110, 150), 9, v.colour as usize, x, y).map_or(SideHit::Inside, SideHit::Colour),
    }
}

/// Draw the open popup.
pub fn draw_popup(p: Popup, v: &PopVals) {
    match p {
        Popup::None => {}
        Popup::Mf(t) => draw_mf(t, v),
        Popup::Ch1(t) => draw_ch1(t, v),
        Popup::Math(t) => draw_math(t, v),
        Popup::Tb(i) => draw_tb(i),

    }
}

// opendm40 timebase list popup (docs/scope-scale-ux.md sec. 1): the stock
// popup box and header, the stock spinner widget over every timebase. The
// box's top row is y 21: y 20 is the frame line in the same colour (the
// stock popups start at y 20), and the box keeps 3 px from the badges.
pub const TB_BOX: Rect = rect(79, 21, 132, 191);
const TB_HEAD: Rect = rect(80, 22, 130, 29);
pub const TB_SPIN: Rect = rect(85, 55, 120, 150);
/// Timebases beyond the YT maximum (roll only) in the list.
const C_ROLL: Color = 0xfd20;
/// Drag travel per list row, px.
pub const TB_ROW_PX: i32 = 30;

/// Fastest timebase the list offers (`set_tb_lo`): TB_MIN, or WAVE_TB_MIN
/// while the DMM wave view runs (its 1 ms/div floor: the faster entries
/// only clamped back to it).
static TB_LO: core::sync::atomic::AtomicU8 = core::sync::atomic::AtomicU8::new(crate::scope::TB_MIN as u8);

/// The list's first entry (`TB_LO`); app_wave.rs sets it on the wave
/// view's entry / exit.
pub fn set_tb_lo(lo: usize) {
    TB_LO.store(lo.clamp(crate::scope::TB_MIN, crate::scope::TB_MAX) as u8, core::sync::atomic::Ordering::Relaxed);
}

fn tb_lo() -> usize {
    TB_LO.load(core::sync::atomic::Ordering::Relaxed) as usize
}

/// The timebase list popup with `cur` (timebase index) in the band. Items
/// `tb_lo()`..=TB_MAX; roll-only entries (beyond TB_YT_MAX) in orange.
pub fn draw_tb(cur: u8) {
    let (lo, hi) = (tb_lo(), crate::scope::TB_MAX);
    let cur = (cur as usize).clamp(lo, hi);
    let screen = rect(0, 0, 480, 320);
    paint(TB_BOX, &[(K_SHAPE, C_BAR)], |x, y| if TB_HEAD.has(x, y) { (C_POP_TAB, tag(1, 0)) } else { (C_FRAME, tag(1, 0)) });
    let t = Ink::new(sf::SC_F16, TB_HEAD, "Time/div", 8, 0);
    paint(TB_HEAD, &[(K_TEXT, C_POP_TAB)], |x, y| match t.at(x, y) {
        Some(g) => (C_YELLOW, tag(1, g)),
        None => (C_POP_TAB, 0),
    });
    // The stock spinner (FUN_080274b2), one probe layer per row.
    let r = TB_SPIN;
    let band = rect(r.x, r.y + (r.h - 30) / 2, r.w, 30);
    let mut ink = [Ink::none(); 5];
    let mut col = [0u16; 5];
    for k in 0..5i16 {
        let idx = cur as i16 - 2 + k;
        if idx < lo as i16 || idx > hi as i16 {
            continue;
        }
        let y = r.y + k * 30;
        col[k as usize] = if idx as usize > crate::scope::TB_YT_MAX {
            C_ROLL
        } else if y == band.y {
            C_WHITE
        } else {
            C_POP_BG
        };
        ink[k as usize] = Ink::new(sf::SC_F16, rect(r.x, y, TB_TIME_W, 30), TB_LABELS[idx as usize], 4, 0);
    }
    let layers = [(K_SHAPE, C_FRAME), (K_TEXT, C_BAR), (K_TEXT, C_BAR), (K_TEXT, C_SEL), (K_TEXT, C_BAR), (K_TEXT, C_BAR)];
    paint(isect(r, screen), &layers, |x, y| {
        for k in 0..5 {
            if let Some(g) = ink[k].at(x, y) {
                return (col[k], tag(2 + k as u16, g));
            }
        }
        if band.has(x, y) {
            (C_SEL, tag(1, 0))
        } else if in_rbox(r, 8, 0x0f, x, y) {
            (C_BAR, tag(1, 0))
        } else {
            (C_FRAME, 0)
        }
    });
    tb_rates(cur, lo, hi);
}

/// Timebase list row: the time's column (right-aligned) and the rate's
/// box (vertically inside the row, clear of the spinner's corners).
pub const TB_TIME_W: i16 = 44;
pub const fn tb_rate_rect(y: i16) -> Rect {
    rect(TB_SPIN.x + TB_TIME_W + 6, y + 7, TB_SPIN.w - TB_TIME_W - 9, 16)
}

/// opendm40: each list entry's sample rate (the list's `cur` centre,
/// entries `lo..=hi`), small and dim beside its time: what ACQ:SRAT? gives
/// at that timebase. One text at a time (a second Ink array in `draw_tb`
/// would make the list the deepest stack path). Region A2 (zero-wait,
/// link-native.x): region B is near its 16 KiB warning, and the list draws
/// while the Wave view streams, so not .cold.
#[inline(never)]
#[cfg_attr(target_os = "none", link_section = ".text.a2.srate")]
fn tb_rates(cur: usize, lo: usize, hi: usize) {
    for k in 0..5 {
        let Some(idx) = (cur + k).checked_sub(2) else { continue };
        if idx < lo || idx > hi {
            continue;
        }
        let mut b = Buf::new();
        fmt_srate(&mut b, crate::scope::tb_rate(idx, lo != crate::scope::TB_MIN));
        let (bg, fg) = if k == 2 { (C_SEL, C_POP_BG) } else { (C_BAR, C_DIS) };
        let r = tb_rate_rect(TB_SPIN.y + k as i16 * 30);
        fill_text_on(r, r, bg, bg, sf::SC_F12, r, b.as_str(), fg, 1, 1);
    }
}

/// What a touch on the timebase list hits.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum TbHit {
    /// A list row: its timebase index.
    Row(u8),
    Inside,
    /// Outside the box: closes.
    Close,
}

pub fn tb_hit(cur: u8, x: i16, y: i16) -> TbHit {
    if !TB_BOX.has(x, y) && !(y == 20 && x >= TB_BOX.x && x < TB_BOX.x + TB_BOX.w) {
        return TbHit::Close;
    }
    let (lo, hi) = (tb_lo() as i16, crate::scope::TB_MAX as i16);
    let cur = (cur as i16).clamp(lo, hi);
    if !TB_SPIN.has(x, y) {
        return TbHit::Inside;
    }
    let idx = cur - 2 + (y - TB_SPIN.y) / 30;
    if idx < lo || idx > hi { TbHit::Inside } else { TbHit::Row(idx as u8) }
}

/// Drag scroll of the list: `acc` px of vertical travel not yet turned into
/// rows (down = earlier, faster entries come into the band). Returns the
/// new centre index and the remainder (0 at either end of the list).
pub fn tb_scroll(cur: u8, acc: i32) -> (u8, i32) {
    let (lo, hi) = (tb_lo() as i32, crate::scope::TB_MAX as i32);
    let rows = acc / TB_ROW_PX;
    let want = cur as i32 - rows;
    let i = want.clamp(lo, hi);
    let rem = if i != want { 0 } else { acc - rows * TB_ROW_PX };
    (i as u8, rem)
}

/// The "Sampling" box (RUN refused during AUTO-set).
pub fn sampling() {
    rbox_text(rect(190, 150, 100, 20), rect(0, 0, 480, 320), 8, C_PURPLE, C_FILL, sf::SC_F16, "Sampling", C_WHITE);
}

// ------------------------------------------------------------------ meter strip
// Combined DMM + scope view (port plan S7, mockup tools/stock-ui mk01/mk02):
// the stock page with a 56 px meter strip at y 20..75 in the stock language
// (r5 badges, the 12 px font, the 20 px digit font of the stock generator
// page), the graph below at (13,76,401,225). Unlike the stock page, the
// strip passes the owner's layout rules (>= 3 px between ink boxes and
// inside containers; checked by tools/ui-render).

/// Short text (strip fields).
#[derive(Clone, Copy, PartialEq)]
pub struct Txt {
    b: [u8; 20],
    n: u8,
}

impl Txt {
    pub const EMPTY: Txt = Txt { b: [0; 20], n: 0 };
    pub fn new(s: &str) -> Txt {
        let mut t = Txt::EMPTY;
        t.push(s);
        t
    }
    pub fn push(&mut self, s: &str) {
        for c in s.chars() {
            let mut e = [0u8; 4];
            let e = c.encode_utf8(&mut e).as_bytes();
            if self.n as usize + e.len() > 20 {
                return;
            }
            self.b[self.n as usize..self.n as usize + e.len()].copy_from_slice(e);
            self.n += e.len() as u8;
        }
    }
    pub fn as_str(&self) -> &str {
        core::str::from_utf8(&self.b[..self.n as usize]).unwrap_or("")
    }
}

/// What the strip shows.
#[derive(Clone, Copy, PartialEq)]
pub struct StripData {
    /// Function badge (purple).
    pub func: &'static str,
    /// Reading (20 px digits) and unit (digit font, or LETTERS for deg C / F).
    pub value: Txt,
    pub unit: Txt,
    pub unit_letters: bool,
    /// Sub readings, 12 px ASCII, right-aligned.
    pub sub: [Txt; 2],
    /// State badge (HOLD / A-HOLD / HV / REL / PASS / FAIL / range).
    pub badge: &'static str,
    pub badge_col: Color,
    /// >= 100 V: the strip ring turns red.
    pub hv: bool,
    /// Generator frequency text (empty: generator off).
    pub gen: Txt,
    /// A provisional reading (`Reading::prov`): the value in C_DIS grey.
    pub dim: bool,
    /// The beeper indicator (App::beep_ind): the value in ui::SHORT_FG.
    pub short: bool,
    /// Sub row 1 + this holds the accuracy (ui::strip_acc: grey, after a
    /// "+-" sign); 0 = none.
    pub acc: u8,
    /// The value's glyphs from this index on are below the accuracy
    /// (ui::strip_acc, ../docs/dmm-accuracy.md section 5): C_LOW grey.
    pub grey: u8,
}

impl StripData {
    pub const EMPTY: StripData = StripData {
        func: "",
        value: Txt::EMPTY,
        unit: Txt::EMPTY,
        unit_letters: false,
        sub: [Txt::EMPTY; 2],
        badge: "",
        badge_col: C_POP_TAB,
        hv: false,
        gen: Txt::EMPTY,
        dim: false,
        short: false,
        acc: 0,
        grey: u8::MAX,
    };
}

pub const STRIP: Rect = rect(13, 21, 401, 54);
const ST_INNER: Rect = rect(14, 22, 399, 52);
const ST_FUNC: Rect = rect(19, 25, 62, 17);
const ST_BADGE: Rect = rect(19, 46, 62, 17);
/// Reading: right-aligned, ends 4 px before the unit (the widest values,
/// "-444.44" / "1562.00", are 105 / 107 px of ink in SC_DIGITS); from 3 px
/// after the badge column.
const ST_VALUE: Rect = rect(84, 24, 108, 30);
/// Unit: from x 196, as wide as its text (`st_unit`): SC_DIGITS up to
/// ST_UNIT_W px ("mW", "MHz", "VAh" 54..59), else SC_F20, the panel
/// buttons' 20 px face (device check 92fb8fa: a 40 px box cut "mW" to "mV";
/// "mWh" is 76 px in SC_DIGITS, 32 in SC_F20).
const ST_UNIT_X: i16 = 196;
const ST_UNIT_W: i16 = 59;
const ST_UNIT_Y: i16 = 24;
const ST_UNIT_H: i16 = 30;
/// Sub rows: left-aligned from 4 px after the unit's box, to 4 px before
/// the GEN column while it shows, else to 4 px inside the strip (`st_sub`).
pub const ST_SUB_Y: [i16; 2] = [25, 43];
const ST_SUB_H: i16 = 14;
const ST_SUB_END: i16 = 409;
/// GEN column: 64 px, the widest frequency ("10.00MHz", "999.9kHz").
const ST_GEN: Rect = rect(344, 25, 64, 17);
const ST_GEN_T: Rect = rect(344, 46, 64, 17);
const C_HV: Color = 0xc9a0;
const C_GEN: Color = 0x0400;

// On the device in SRAM (.dmabuf, NOLOAD, as GRAPH); `init` sets it up.
#[cfg_attr(target_os = "none", link_section = ".dmabuf")]
static mut STRIP_SHOWN: Option<StripData> = None;

/// Forget the drawn strip (the next `meter_strip` draws all of it).
pub fn strip_invalidate() {
    unsafe { *core::ptr::addr_of_mut!(STRIP_SHOWN) = None };
}

/// Sub row `i`'s box from x `x0` (`st_unit`'s end) with / without the GEN
/// column (see ST_SUB_Y). With GEN it ends 15 px before the column: 1.5
/// SC_F12 word spaces, so a sub row and the GEN frequency do not read as
/// one line.
pub fn st_sub(i: usize, gen: bool, x0: i16) -> Rect {
    rect(x0, ST_SUB_Y[i & 1], if gen { ST_GEN.x - 15 - x0 } else { ST_SUB_END - x0 }, ST_SUB_H)
}

/// The unit's font, spacing and box: SC_DIGITS (LETTERS for a unit it
/// lacks, deg C / F, uA) when it fits ST_UNIT_W, else SC_F20, else SC_F16,
/// on the digits' baseline; the box is the text's width (>= 1).
pub fn st_unit(unit: &str, letters: bool) -> (Font, Rect, u8) {
    let big = if letters { sf::LETTERS } else { sf::SC_DIGITS };
    let mut f = big;
    for c in [big, sf::SC_F20, sf::SC_F16] {
        f = c;
        if sf::has_all(c, unit) && sf::text_width(c, unit, 1) <= ST_UNIT_W {
            break;
        }
    }
    let w = sf::text_width(f, unit, 1).clamp(1, ST_UNIT_W);
    // Smaller fonts: bottom-aligned 4 px above the box end, near the
    // digits' baseline.
    let (h, align) = if f.0 == big.0 { (ST_UNIT_H, 1) } else { (ST_UNIT_H - 4, 2) };
    (f, rect(ST_UNIT_X, ST_UNIT_Y, w, h), align)
}

/// x of the sub rows for a unit box `u` (4 px after it).
pub fn st_sub_x(u: Rect) -> i16 {
    u.x + u.w + 4
}

#[inline(always)]
fn st_text(r: Rect, font: Font, text: &str, fg: Color, align: u8, sp: i8) {
    st_text_grey(r, font, text, fg, align, sp, u8::MAX);
}

/// `st_text` with the glyphs from index `grey` on in C_LOW (the value's
/// digits below the accuracy).
#[inline(never)]
fn st_text_grey(r: Rect, font: Font, text: &str, fg: Color, align: u8, sp: i8, grey: u8) {
    let t = Ink::new(font, r, text, align, sp);
    paint(r, &[(K_TEXT, C_FILL)], |x, y| match t.at(x, y) {
        Some(g) => (if g >= grey { C_LOW } else { fg }, tag(1, g)),
        None => (C_FILL, 0),
    });
}

/// A badge (r5) on the strip's dark box; empty text = no badge.
fn st_badge(r: Rect, text: &str, fill: Color) {
    if text.is_empty() {
        lcd::fill_rect(r.x as u16, r.y as u16, r.w as u16, r.h as u16, C_FILL);
        return;
    }
    let t = Ink::new(sf::SC_F12, r, text, 8, 1);
    badge(r, fill, C_FILL, None, &[(&t, C_WHITE)]);
}

/// Draw the strip, repainting only the fields that changed.
pub fn meter_strip(d: &StripData) {
    let shown = unsafe { &mut *core::ptr::addr_of_mut!(STRIP_SHOWN) };
    let o = match *shown {
        Some(o) if o.hv == d.hv => Some(o),
        _ => {
            // Ring (0xad55, red at >= 100 V) and the dark box, one shape.
            let ring = if d.hv { C_HV } else { C_FRAME };
            lcd::fill_rect(13, 20, 401, 1, C_BAR);
            lcd::fill_rect(13, 75, 401, 1, C_BAR);
            paint(STRIP, &[(K_SHAPE, C_BAR)], |x, y| {
                if in_rbox(ST_INNER, 5, 0x0f, x, y) {
                    (C_FILL, tag(1, 0))
                } else if in_rbox(STRIP, 5, 0x0f, x, y) {
                    (ring, tag(1, 0))
                } else {
                    (C_BAR, 0)
                }
            });
            None
        }
    };
    let changed = |same: bool| o.is_none() || !same;
    let ol = o.unwrap_or(*d);
    if changed(ol.func == d.func) {
        st_badge(ST_FUNC, d.func, C_PURPLE);
    }
    if changed(ol.badge == d.badge && ol.badge_col == d.badge_col) {
        st_badge(ST_BADGE, d.badge, d.badge_col);
    }
    if changed(ol.value == d.value && ol.dim == d.dim && ol.grey == d.grey && ol.short == d.short) {
        let c = if d.dim { C_DIS } else if d.short { crate::ui::SHORT_FG } else { C_YELLOW };
        st_text_grey(ST_VALUE, sf::SC_DIGITS, d.value.as_str(), c, 4, 1, d.grey);
    }
    // The unit's box follows its text and the sub rows follow the unit: on
    // a change of their x (or of the GEN column) clear from the unit to
    // the strip's inner edge first, then the unit, GEN and the subs.
    let (uf, ur, ua) = st_unit(d.unit.as_str(), d.unit_letters);
    let sx = st_sub_x(ur);
    let gen_on = d.gen.n > 0;
    let moved = changed((ol.gen.n > 0) == gen_on && ol.unit == d.unit && ol.unit_letters == d.unit_letters);
    if moved {
        if o.is_some() {
            lcd::fill_rect(ST_UNIT_X as u16, ST_UNIT_Y as u16, (ST_SUB_END + 1 - ST_UNIT_X) as u16, (ST_GEN_T.y + ST_GEN_T.h - ST_UNIT_Y) as u16, C_FILL);
        }
        st_text(ur, uf, d.unit.as_str(), C_YELLOW, ua, 1);
    }
    if changed(ol.gen == d.gen) || moved {
        st_badge(ST_GEN, if gen_on { "GEN" } else { "" }, C_GEN);
        st_text(ST_GEN_T, sf::SC_F12, d.gen.as_str(), C_GREEN, 8, 1);
    }
    for i in 0..2 {
        if changed(ol.sub[i] == d.sub[i] && ol.dim == d.dim && ol.acc == d.acc) || moved {
            let mut r = st_sub(i, gen_on, sx);
            // The accuracy: grey, the text after the "+-" (`pm`); none
            // when it does not fit even at spacing 0 (the GEN column).
            let acc = d.acc == i as u8 + 1;
            let mut t = d.sub[i].as_str();
            if acc {
                lcd::fill_rect(r.x as u16, r.y as u16, PM_W as u16, r.h as u16, C_FILL);
                r = rect(r.x + PM_W, r.y, r.w - PM_W, r.h);
                if sf::text_width(sf::SC_F12, t, 0) > r.w {
                    t = "";
                } else {
                    pm(r.x - PM_W, r.y + 3, C_DIS);
                }
            }
            // Narrow (GEN shown): the text without its spaces if it is too
            // wide, then at the stock spacing 0 (as the scope's cells).
            let mut b = [0u8; 24];
            let wide = sf::text_width(sf::SC_F12, t, 1) > r.w;
            let t = if wide {
                let mut n = 0;
                for &c in t.as_bytes().iter().filter(|&&c| c != b' ') {
                    if n < b.len() {
                        b[n] = c;
                        n += 1;
                    }
                }
                core::str::from_utf8(&b[..n]).unwrap_or(t)
            } else {
                t
            };
            let sp = if wide && sf::text_width(sf::SC_F12, t, 1) > r.w { 0 } else { 1 };
            st_text(r, sf::SC_F12, t, if d.dim || acc { C_DIS } else { C_WHITE }, 1, sp);
        }
    }
    *shown = Some(*d);
}

/// Width of the "+-" sign (`pm`) and its gap to the text.
pub const PM_W: i16 = 6;

/// "+-" (no stock font has U+00B1): a 5 x 5 plus over a 5 px bar, 8 px
/// tall from `y` (the F14 / SC_F12 digits' height); the DMM page's and the
/// strip's accuracy label.
#[inline(never)]
#[cfg_attr(target_os = "none", link_section = ".text.a2.acc")]
pub fn pm(x: i16, y: i16, c: Color) {
    lcd::fill_rect((x + 2) as u16, y as u16, 1, 5, c);
    lcd::fill_rect(x as u16, (y + 2) as u16, 5, 1, c);
    lcd::fill_rect(x as u16, (y + 7) as u16, 5, 1, c);
}

/// Right-panel button under a touch: the stock rects, x from the panel edge
/// and 4 px of slop up / down (the 1 / 8 px gaps between the pills go to the
/// nearer button instead of nowhere).
pub fn panel_hit(x: u16, y: u16) -> Option<usize> {
    if x < 414 {
        return None;
    }
    let y = y as i16;
    let mut best: Option<(i16, usize)> = None;
    for (k, r) in B_RECT.iter().enumerate() {
        let d = if y < r.y { r.y - y } else if y >= r.y + r.h { y - (r.y + r.h - 1) } else { 0 };
        if d <= 4 && best.map_or(true, |b| d < b.0) {
            best = Some((d, k));
        }
    }
    best.map(|b| b.1)
}

/// Strip touch areas: the badge column (REL) and the rest (function band).
pub fn strip_hit(x: u16, y: u16) -> Option<bool> {
    if (20..76).contains(&y) && x < 414 { Some(x < 86) } else { None }
}

// ------------------------------------------------------------------ pinch

/// Pinch preview state (scope_page.rs): horizontal = timebase index, else
/// hardware V/div range index; `i0` at the start, `i` previewed.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct PinchState {
    pub horizontal: bool,
    pub i0: usize,
    pub i: usize,
}

/// Highest V/div range index (scope::VDIV has 10 entries).
pub const RANGE_MAX: usize = 9;

/// One pinch report: the stored state (None at the start of a pinch) and the
/// finger separation `s0` -> `s` along the pair's axis. 1-2-5: one index
/// step per x2.15 of the separation; spreading zooms in (smaller s/div or
/// V/div). A stored state of the other direction is dropped (a pinch whose
/// end was lost must never carry its index into the other axis' table:
/// pre-flash review 7389e8a B1). The result is always inside the table of
/// its own axis: tb `tb_min..=tb_max`, range `0..=RANGE_MAX`.
/// Pinch modes (DBG:PINCH, owner 2026-10-24 "pinch needs a big stretch
/// before anything changes"): PINCH_STOCK the first opendm40 rule (one
/// table step per span ratio e^0.767 = 2.15, rounded: the first step at
/// 1.47x); PINCH_STEP (default) one step per 1.25x of span, truncated (the
/// first step at 1.25x, the next at 1.56x, ...: each step re-anchors);
/// PINCH_LIVE the trace stretches continuously with the fingers (a
/// screen-only scale of the shown frame), the release snaps to the nearest
/// table step (`pinch_snap`).
pub const PINCH_STOCK: u8 = 0;
pub const PINCH_STEP: u8 = 1;
pub const PINCH_LIVE: u8 = 2;
/// ln(1.25): the PINCH_STEP ratio per step.
const LN_STEP: f32 = 0.223_143_55;

pub fn pinch_step(prev: Option<PinchState>, horizontal: bool, s0: u16, s: u16, tb: usize, range: usize, tb_min: usize, tb_max: usize) -> PinchState {
    pinch_step_mode(PINCH_STOCK, prev, horizontal, s0, s, tb, range, tb_min, tb_max)
}

/// `pinch_step` in pinch mode `mode` (PINCH_LIVE: `i` is the step the
/// release would snap to, see `pinch_snap`).
pub fn pinch_step_mode(mode: u8, prev: Option<PinchState>, horizontal: bool, s0: u16, s: u16, tb: usize, range: usize, tb_min: usize, tb_max: usize) -> PinchState {
    let cur = if horizontal { tb } else { range };
    let p = match prev {
        Some(p) if p.horizontal == horizontal => p,
        _ => PinchState { horizontal, i0: cur, i: cur },
    };
    let r = s.max(1) as f32 / s0.max(1) as f32;
    let (lo, hi) = if horizontal { (tb_min, tb_max) } else { (0, RANGE_MAX) };
    if mode == PINCH_LIVE {
        return PinchState { i: pinch_snap(horizontal, p.i0, r, lo, hi), ..p };
    }
    let k = if mode == PINCH_STEP {
        let x = ln_f(r) / LN_STEP;
        x as i32 // toward zero: a step only once its ratio is reached
    } else {
        round_i(ln_f(r) / 0.767)
    };
    let i = (p.i0 as i32 - k).clamp(lo as i32, hi as i32) as usize;
    PinchState { i, ..p }
}

/// The table step nearest (in log scale) to the start step `i0` zoomed by
/// span ratio `r` (fingers apart = zoom in = smaller per-div value), in
/// lo..=hi. `div` gives the per-div value of a table index.
pub fn pinch_snap(horizontal: bool, i0: usize, r: f32, lo: usize, hi: usize) -> usize {
    let div = |i: usize| -> f32 { if horizontal { crate::scope::SDIV[i] } else { crate::scope::VDIV[i] } };
    let want = ln_f(div(i0)) - ln_f(r.max(1e-3));
    let mut best = i0.clamp(lo, hi);
    let mut bd = f32::MAX;
    for i in lo..=hi {
        let d = (ln_f(div(i)) - want).abs();
        if d < bd {
            bd = d;
            best = i;
        }
    }
    best
}

/// ln(x) for x > 0 (no libm): exponent + a short series on the mantissa.
pub fn ln_f(x: f32) -> f32 {
    if !(x > 0.0) {
        return 0.0;
    }
    let b = x.to_bits();
    let e = ((b >> 23) & 0xFF) as i32 - 127;
    let m = f32::from_bits((b & 0x007F_FFFF) | 0x3F80_0000); // 1..2
    let t = (m - 1.0) / (m + 1.0);
    let t2 = t * t;
    e as f32 * core::f32::consts::LN_2 + 2.0 * t * (1.0 + t2 / 3.0 + t2 * t2 / 5.0 + t2 * t2 * t2 / 7.0)
}

fn round_i(x: f32) -> i32 {
    if x >= 0.0 { (x + 0.5) as i32 } else { (x - 0.5) as i32 }
}
