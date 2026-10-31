//! ILI9488 over FSMC bank 1, as wired on the DM40.
//!
//! Command register 0x6001FFFE (RS=0 via FSMC_A16), data register 0x60020000.
//! 16-bit 8080 bus, panel configured for 16 bpp RGB565 -> one 16-bit data write
//! per pixel. Addresses and the window/pixel protocol come from the stock
//! firmware (FUN_0800a0e4 init, FUN_0800a5ca window, FUN_0800a89a pixel).
//!
//! Milestone 1 reuses the FSMC/LCD setup the stock firmware already performed
//! before handing control over; it does not re-init the panel yet.

use core::ptr::read_volatile;

use crate::mmio::w16;

const LCD_CMD: u32 = 0x6001_FFFE;
const LCD_DATA: u32 = 0x6002_0000;

/// Landscape, as the stock multimeter screen uses it.
pub const W: u16 = 480;
pub const H: u16 = 320;

pub type Color = u16; // RGB565

pub const BLACK: Color = 0x0000;
pub const RED: Color = 0xF800;
pub const MAGENTA: Color = 0xF81F;

#[inline(always)]
pub fn cmd(c: u16) {
    unsafe { w16(LCD_CMD, c) };
}

#[inline(always)]
pub fn data(d: u16) {
    if strip_on() {
        strip_px(d);
    } else {
        unsafe { w16(LCD_DATA, d) };
    }
}

/// `data` for code that never runs in a strip pass (`strips`): the panel
/// directly, without the strip check per pixel.
#[inline(always)]
pub fn data_panel(d: u16) {
    debug_assert!(!strip_on());
    unsafe { w16(LCD_DATA, d) };
}

/// One read cycle on the data register (RS=1). Used for register reads and the
/// GRAM read-back (0x2E) behind screenshots; the stock FSMC read timing applies.
#[inline(always)]
pub fn read_data() -> u16 {
    unsafe { read_volatile(LCD_DATA as *const u16) }
}

/// Memory access control (0x36). The stock firmware draws landscape by rotating
/// in software with MADCTL=0x48 (MV=0, native 320x480). We instead address the
/// panel in landscape directly: MV | MX | MY | BGR = 0xE8 gives a 480x320 grid
/// with (0,0) at the top left as the meter is held (0x28 is upside down; checked
/// against a read-back of the stock screen).
pub const MADCTL_LANDSCAPE: u16 = 0xE8;

pub fn set_madctl(v: u16) {
    cmd(0x36);
    data(v);
}

pub const CMD_RAMWR: u16 = 0x2C;
pub const CMD_RAMRD: u16 = 0x2E;

/// Set the drawing window and leave the controller ready for pixel writes (0x2C).
pub fn set_window(x0: u16, y0: u16, x1: u16, y1: u16) {
    set_window_cmd(x0, y0, x1, y1, CMD_RAMWR);
}

/// Set the window, then issue `mem_cmd` (RAMWR 0x2C or RAMRD 0x2E).
pub fn set_window_cmd(x0: u16, y0: u16, x1: u16, y1: u16, mem_cmd: u16) {
    if strip_on() {
        strip_window(x0, y0, x1, y1);
        return;
    }
    if defer_on() {
        if mem_cmd == CMD_RAMRD {
            defer_flush();
        } else {
            defer_touch(x0, y0, x1, y1);
        }
    }
    window_raw(x0, y0, x1, y1, mem_cmd);
}

fn window_raw(x0: u16, y0: u16, x1: u16, y1: u16, mem_cmd: u16) {
    cmd(0x2A);
    data(x0 >> 8);
    data(x0 & 0xFF);
    data(x1 >> 8);
    data(x1 & 0xFF);
    cmd(0x2B);
    data(y0 >> 8);
    data(y0 & 0xFF);
    data(y1 >> 8);
    data(y1 & 0xFF);
    cmd(mem_cmd);
}

/// Read `n` pixels of GRAM row `y` starting at column `x` and pass them to
/// `out` as RGB565, left to right (screenshots). With COLMOD 16 bpp the
/// ILI9488 still returns RGB666 on a read: one dummy word after RAMRD, then 3
/// bytes per pixel (R, G, B, 6 significant bits each) packed big-endian into
/// the 16-bit words (README, tools/dm40dbg.py decode()), so 2 pixels = 3 words.
/// Leaves the controller in RAMRD; the next drawing call sets its own window.
pub fn read_span(x: u16, y: u16, n: u16, mut out: impl FnMut(Color)) {
    if n == 0 {
        return;
    }
    set_window_cmd(x, y, x + n - 1, y, CMD_RAMRD);
    let _ = read_data(); // dummy
    let mut left = n;
    while left > 0 {
        let w0 = read_data();
        let w1 = read_data();
        out(rgb666_to_565(w0 >> 8, w0 & 0xFF, w1 >> 8));
        left -= 1;
        if left == 0 {
            break;
        }
        let w2 = read_data();
        out(rgb666_to_565(w1 & 0xFF, w2 >> 8, w2 & 0xFF));
        left -= 1;
    }
}

#[inline(always)]
fn rgb666_to_565(r: u16, g: u16, b: u16) -> Color {
    (r & 0xF8) << 8 | (g & 0xFC) << 3 | b >> 3
}

pub fn fill_rect(x: u16, y: u16, w: u16, h: u16, color: Color) {
    if w == 0 || h == 0 {
        return;
    }
    if strip_on() {
        strip_fill(x, y, w, h, color);
        return;
    }
    if defer_on() && defer_fill(x, y, w, h, color) {
        return;
    }
    set_window(x, y, x + w - 1, y + h - 1);
    fill_n(w as u32 * h as u32, color);
}

/// `fill_rect` past any batch (the panic bar: no deferred-fill frames on
/// the panic path's stack).
pub fn fill_rect_raw(x: u16, y: u16, w: u16, h: u16, color: Color) {
    window_raw(x, y, x + w - 1, y + h - 1, CMD_RAMWR);
    fill_n(w as u32 * h as u32, color);
}

/// Panel only (a strip pass fills through `strip_fill`).
fn fill_n(count: u32, color: Color) {
    let mut i = 0u32;
    while i < count {
        unsafe { w16(LCD_DATA, color) };
        i += 1;
    }
}

// ------------------------------------------------------------ deferred fills
//
// A page is drawn as "clear the background, then draw the widgets" with no
// frame buffer, so the panel showed the bare background for a refresh or
// more and then the widgets one by one (emu/lcdtrace.py). While a batch is
// open (`defer_start` .. `defer_flush`), a large fill (`DEFER_MIN` px) is
// not drawn at once: it is remembered as a layer, and its pixels are
// written only when needed: the 8 x 8 px cells under any later drawing
// window get the layers' colours just before that drawing, and the cells
// nothing drew over get them at `defer_flush`. Every pixel still gets the
// same writes in the same order, so the final picture is the same; until a
// widget is drawn the panel keeps showing the old screen there instead of
// the bare background. A fill over cells already drawn on in the batch
// goes there at once; a layer that does not fit (`DEFER_N`) is drawn as
// any other window. Read-back (RAMRD: screenshots) flushes first.

const CELL: u16 = 8; // 60 cell columns: one u64 per cell row
const CH: usize = (H / CELL) as usize; // 40 cell rows
const DEFER_N: usize = 6;
const DEFER_MIN: u32 = 2048;

struct Defer {
    on: bool,
    n: u8,
    /// Bit r: cell row r has waiting cells.
    rows: u64,
    /// Cell rows: bit c = cell (c, row) still waits for the layers.
    pend: [u64; CH],
    /// Cell rows: bit c = drawn on in this batch.
    drawn: [u64; CH],
    /// x, y, w, h, colour, in the order drawn.
    layer: [(u16, u16, u16, u16, Color); DEFER_N],
}

// TCM .bss (CPU only; was .sram_bss until the SRAM audit, docs/native-boot.md).
static mut DEFER: Defer = Defer { on: false, n: 0, rows: 0, pend: [0; CH], drawn: [0; CH], layer: [(0, 0, 0, 0, 0); DEFER_N] };

#[inline(always)]
fn defer() -> &'static mut Defer {
    // Main loop only (no ISR draws on the panel).
    unsafe { &mut *core::ptr::addr_of_mut!(DEFER) }
}

#[inline(always)]
fn defer_on() -> bool {
    defer().on
}

/// Open a batch: large fills from now on wait for `defer_flush` (or for a
/// drawing over them). Nested starts are one batch.
pub fn defer_start() {
    let d = defer();
    if !d.on && !strip_on() {
        d.on = true;
        d.n = 0;
        d.rows = 0;
        d.drawn = [0; CH];
    }
}

/// Close the batch: what the layers still cover is drawn, top to bottom.
pub fn defer_flush() {
    let d = defer();
    if !d.on {
        return;
    }
    if d.rows != 0 {
        cells_run(0, CH - 1, u64::MAX);
    }
    d.on = false;
    d.n = 0;
}

/// Cells of columns x0..=x1 (x0 <= x1 < W).
#[inline(always)]
fn cell_mask(x0: u16, x1: u16) -> u64 {
    let (c0, c1) = (x0 / CELL, x1 / CELL);
    let hi = if c1 >= 63 { u64::MAX } else { (1u64 << (c1 + 1)) - 1 };
    hi & !((1u64 << c0) - 1)
}

/// Rows r0..=r1 (r1 < CH) as summary bits.
#[inline(always)]
fn row_mask(r0: usize, r1: usize) -> u64 {
    ((1u64 << (r1 + 1)) - 1) & !((1u64 << r0) - 1)
}

/// A large fill: a layer (true), or false = draw it now.
#[inline(never)]
fn defer_fill(x: u16, y: u16, w: u16, h: u16, color: Color) -> bool {
    let d = defer();
    if (w as u32) * (h as u32) < DEFER_MIN || d.n as usize >= DEFER_N || x >= W || y >= H {
        return false;
    }
    let (x1, y1) = ((x + w - 1).min(W - 1), (y + h - 1).min(H - 1));
    let m = cell_mask(x, x1);
    let (r0, r1) = ((y / CELL) as usize, (y1 / CELL) as usize);
    // Cells already drawn on in this batch show what is drawn now: the
    // fill goes there at once (only where it covers them).
    let mut r = r0;
    while r <= r1 {
        let live = d.drawn[r] & m;
        let mut e = r;
        while e < r1 && d.drawn[e + 1] & m == live {
            e += 1;
        }
        if live != 0 {
            let ry0 = (r as u16 * CELL).max(y);
            let ry1 = ((e as u16 + 1) * CELL - 1).min(y1);
            for_runs(live, |a, b| {
                let (rx0, rx1) = ((a * CELL).max(x), ((b + 1) * CELL - 1).min(x1));
                window_raw(rx0, ry0, rx1, ry1, CMD_RAMWR);
                fill_n((rx1 - rx0 + 1) as u32 * (ry1 - ry0 + 1) as u32, color);
            });
        }
        let wait = m & !live;
        for q in &mut d.pend[r..=e] {
            *q |= wait;
        }
        if wait != 0 {
            d.rows |= row_mask(r, e);
        }
        r = e + 1;
    }
    d.layer[d.n as usize] = (x, y, x1 - x + 1, y1 - y + 1, color);
    d.n += 1;
    true
}

/// A drawing window: the waiting cells under it get the layers first.
#[inline(never)]
fn defer_touch(x0: u16, y0: u16, x1: u16, y1: u16) {
    if x0 >= W || y0 >= H {
        return;
    }
    let (x1, y1) = (x1.min(W - 1), y1.min(H - 1));
    let (r0, r1, m) = ((y0 / CELL) as usize, (y1 / CELL) as usize, cell_mask(x0, x1));
    let d = defer();
    if d.rows & row_mask(r0, r1) != 0 {
        cells_run(r0, r1, m);
    }
    for q in &mut d.drawn[r0..=r1] {
        *q |= m;
    }
}

/// Draw the layers into the waiting cells of rows r0..=r1 under `m`, rows
/// with the same cells in one window per run.
fn cells_run(r0: usize, r1: usize, m: u64) {
    let d = defer();
    let mut r = r0;
    while r <= r1 {
        let p = d.pend[r] & m;
        if p == 0 {
            r += 1;
            continue;
        }
        let mut e = r;
        while e < r1 && d.pend[e + 1] & m == p {
            e += 1;
        }
        for k in r..=e {
            d.pend[k] &= !p;
            if d.pend[k] == 0 {
                d.rows &= !(1u64 << k);
            }
        }
        let (ry0, ry1) = (r as u16 * CELL, (e as u16 + 1) * CELL - 1);
        let n = d.n as usize;
        for_runs(p, |a, b| {
            let (rx0, rx1) = (a * CELL, (b + 1) * CELL - 1);
            for &(lx, ly, lw, lh, c) in &d.layer[..n] {
                let (ax, ay) = (lx.max(rx0), ly.max(ry0));
                let (bx, by) = ((lx + lw - 1).min(rx1), (ly + lh - 1).min(ry1));
                if ax <= bx && ay <= by {
                    window_raw(ax, ay, bx, by, CMD_RAMWR);
                    fill_n((bx - ax + 1) as u32 * (by - ay + 1) as u32, c);
                }
            }
        });
        r = e + 1;
    }
}

/// Runs of set bits in `m`: f(first cell, last cell).
#[inline(always)]
fn for_runs(mut m: u64, mut f: impl FnMut(u16, u16)) {
    while m != 0 {
        let a = m.trailing_zeros() as u16;
        let len = (!(m >> a)).trailing_zeros() as u16; // 64 when all ones
        let b = a + len - 1;
        f(a, b);
        m = if b >= 63 { 0 } else { m & !((1u64 << (b + 1)) - 1) };
    }
}

pub fn fill_screen(color: Color) {
    fill_rect(0, 0, W, H, color);
}

// ------------------------------------------------------------ strip rendering
//
// The page is drawn once per strip of full-width rows into a RAM buffer
// (`strips`): the drawing calls are the same, but while a strip is open a
// window and its pixels go to the buffer instead of the panel, and only
// the rows of the strip are kept. A finished strip goes to the panel in
// one window, so every pixel is written once, in its final colour, top to
// bottom; the panel never shows a background without its widgets or a
// half-drawn widget. The drawing code must give the same picture on every
// pass (pages that start from a full-screen fill and keep no state
// between draws) and should skip what lies outside the strip
// (`rows_hit`, `clip_rows`): its cost is paid once per strip it crosses.

struct Strip {
    on: bool,
    /// Rows of the open strip (inclusive).
    y0: u16,
    y1: u16,
    /// The strip's pixels, row-major, `W` per row.
    buf: *mut Color,
    /// The open window (inclusive; x1 < W) and the row being written.
    wx0: u16,
    wx1: u16,
    wy0: u16,
    wy1: u16,
    cy: u16,
    /// Pixels left in row `cy` (0: the next pixel starts the next row) and
    /// where the next one goes (null: row `cy` is outside the strip).
    n: u16,
    p: *mut Color,
}

#[cfg_attr(target_os = "none", link_section = ".sram_bss")]
static mut STRIP: Strip = Strip { on: false, y0: 0, y1: 0, buf: core::ptr::null_mut(), wx0: 0, wx1: 0, wy0: 0, wy1: 0, cy: 0, n: 0, p: core::ptr::null_mut() };

#[inline(always)]
fn strip() -> &'static mut Strip {
    // Main loop only, as DEFER.
    unsafe { &mut *core::ptr::addr_of_mut!(STRIP) }
}

#[inline(always)]
fn strip_on() -> bool {
    strip().on
}

/// The rows drawing can reach now: the open strip, else the whole screen.
#[inline(always)]
pub fn rows() -> (i16, i16) {
    let s = strip();
    if s.on { (s.y0 as i16, s.y1 as i16) } else { (0, H as i16 - 1) }
}

/// A strip pass is open (`strips`).
#[inline(always)]
pub fn in_strip() -> bool {
    strip_on()
}

/// Whether rows y0..=y1 can reach the panel now (false: a strip pass
/// that does not include them; the call can be skipped).
#[inline(always)]
pub fn rows_hit(y0: i16, y1: i16) -> bool {
    let (a, b) = rows();
    y0 <= b && y1 >= a
}

/// Rows y0..=y1 cut to what can reach the panel now (None: nothing).
#[inline(always)]
pub fn clip_rows(y0: i16, y1: i16) -> Option<(i16, i16)> {
    let (a, b) = rows();
    let (a, b) = (y0.max(a), y1.min(b));
    if a <= b { Some((a, b)) } else { None }
}

#[inline(never)]
fn strip_window(x0: u16, y0: u16, x1: u16, y1: u16) {
    let s = strip();
    s.wx0 = x0.min(W - 1);
    s.wx1 = x1.min(W - 1).max(s.wx0);
    s.wy0 = y0;
    s.wy1 = y1.max(y0);
    // The first pixel starts row wy0 (`strip_row` wraps from wy1).
    s.cy = s.wy1;
    s.n = 0;
}

/// One pixel at the controller's write position (next column, next row,
/// back to the window's top after its last pixel), kept when its row is
/// in the strip.
#[inline(never)]
fn strip_px(c: Color) {
    let s = strip();
    if s.n == 0 {
        strip_row();
    }
    let s = strip();
    if !s.p.is_null() {
        unsafe {
            *s.p = c;
            s.p = s.p.add(1);
        }
    }
    s.n -= 1;
}

/// In a strip pass, the buffer's pixels of row y, columns x0..=x1
/// (0 <= x0 <= x1 < W): writing them draws them, no window needed. None:
/// not a strip pass, or row y is not in the strip.
#[inline(always)]
pub fn strip_span(y: i16, x0: i16, x1: i16) -> Option<&'static mut [Color]> {
    let s = strip();
    if !s.on || y < s.y0 as i16 || y > s.y1 as i16 {
        return None;
    }
    let o = (y as usize - s.y0 as usize) * W as usize + x0 as usize;
    Some(unsafe { core::slice::from_raw_parts_mut(s.buf.add(o), (x1 - x0 + 1) as usize) })
}

/// Pixels of one row, left to right (`row_out`): straight into the strip's
/// row in a strip pass, else to the panel's open window.
pub struct Out(*mut Color);

/// `Out` through the strip's write position (`data`).
const CURSOR: *mut Color = 2 as *mut Color;

impl Out {
    /// To the panel's open window (or the strip's write position).
    #[inline(always)]
    pub fn panel() -> Out {
        Out(if strip_on() { CURSOR } else { core::ptr::null_mut() })
    }

    #[inline(always)]
    pub fn px(&mut self, c: Color) {
        if self.0.is_null() {
            unsafe { w16(LCD_DATA, c) };
        } else if self.0 == CURSOR {
            data(c);
        } else {
            unsafe {
                *self.0 = c;
                self.0 = self.0.add(1);
            }
        }
    }

    #[inline(always)]
    pub fn run(&mut self, c: Color, n: u16) {
        if self.0.is_null() || self.0 == CURSOR {
            run(c, n);
        } else {
            fill_px(self.0, n as usize, c);
            self.0 = unsafe { self.0.add(n as usize) };
        }
    }
}

/// The writer of row y from column x (0 <= x, the row's pixels < W): into
/// the strip in a strip pass, else the panel (the caller's window, row by
/// row).
#[inline(always)]
pub fn row_out(y: i16, x: i16) -> Out {
    let s = strip();
    if !s.on {
        Out(core::ptr::null_mut())
    } else if y >= s.y0 as i16 && y <= s.y1 as i16 {
        Out(unsafe { s.buf.add((y as usize - s.y0 as usize) * W as usize + x as usize) })
    } else {
        Out(CURSOR)
    }
}

/// `n` pixels of colour `c` at the write position (as `n` x `data(c)`).
#[inline(always)]
pub fn run(c: Color, n: u16) {
    if strip_on() {
        strip_run(c, n);
    } else {
        for _ in 0..n {
            unsafe { w16(LCD_DATA, c) };
        }
    }
}

#[inline(never)]
fn strip_run(c: Color, mut n: u16) {
    while n > 0 {
        let s = strip();
        if s.n == 0 {
            strip_row();
        }
        let s = strip();
        let k = n.min(s.n);
        if !s.p.is_null() {
            fill_px(s.p, k as usize, c);
            s.p = unsafe { s.p.add(k as usize) };
        }
        s.n -= k;
        n -= k;
    }
}

/// The next row of the window.
#[inline(never)]
fn strip_row() {
    let s = strip();
    s.cy = if s.cy < s.wy1 { s.cy + 1 } else { s.wy0 };
    s.n = s.wx1 - s.wx0 + 1;
    s.p = if s.cy >= s.y0 && s.cy <= s.y1 {
        unsafe { s.buf.add((s.cy - s.y0) as usize * W as usize + s.wx0 as usize) }
    } else {
        core::ptr::null_mut()
    };
}

/// `n` pixels of colour `c` from `p`, two per word store.
#[inline(always)]
fn fill_px(mut p: *mut Color, mut n: usize, c: Color) {
    unsafe {
        if n > 0 && (p as usize) & 2 != 0 {
            *p = c;
            p = p.add(1);
            n -= 1;
        }
        let w = c as u32 | (c as u32) << 16;
        let q = p as *mut u32;
        for i in 0..n / 2 {
            *q.add(i) = w;
        }
        if n & 1 != 0 {
            *p.add(n - 1) = c;
        }
    }
}

#[inline(never)]
fn strip_fill(x: u16, y: u16, w: u16, h: u16, c: Color) {
    let s = strip();
    let (ya, yb) = (y.max(s.y0), (y as u32 + h as u32 - 1).min(s.y1 as u32) as u16);
    let (xa, xb) = (x, (x as u32 + w as u32).min(W as u32) as u16);
    if ya > yb || xa >= xb {
        return;
    }
    if xa == 0 && xb == W {
        // Whole rows: one run.
        fill_px(unsafe { s.buf.add((ya - s.y0) as usize * W as usize) }, (yb - ya + 1) as usize * W as usize, c);
        return;
    }
    for r in ya..=yb {
        fill_px(unsafe { s.buf.add((r - s.y0) as usize * W as usize + xa as usize) }, (xb - xa) as usize, c);
    }
}

/// Draw the screen with `draw` once per strip of `buf.len() / W` rows,
/// each strip sent to the panel as one window when done. `draw` must
/// cover every pixel (the buffer keeps the previous strip). Leaves the
/// controller in RAMWR. `buf` must be 4-byte aligned.
#[inline(never)]
pub fn strips(buf: &mut [Color], mut draw: impl FnMut()) {
    let n = (buf.len() / W as usize).min(H as usize) as u16;
    if n == 0 {
        draw();
        return;
    }
    defer_flush();
    let mut y = 0u16;
    while y < H {
        let y1 = (y + n - 1).min(H - 1);
        let px = (y1 - y + 1) as usize * W as usize;
        *strip() = Strip { on: true, y0: y, y1, buf: buf.as_mut_ptr(), wx0: 0, wx1: W - 1, wy0: y, wy1: y1, cy: y1, n: 0, p: core::ptr::null_mut() };
        draw();
        strip().on = false;
        window_raw(0, y, W - 1, y1, CMD_RAMWR);
        push(&buf[..px]);
        y = y1 + 1;
    }
}

/// Pixels to the panel, from RAM (the window is open).
#[inline(never)]
fn push(px: &[Color]) {
    // Eight pixels per step; 16-bit bus writes.
    let mut it = px.chunks_exact(8);
    for c in &mut it {
        let c: &[Color; 8] = c.try_into().unwrap_or(&[0; 8]);
        unsafe {
            w16(LCD_DATA, c[0]);
            w16(LCD_DATA, c[1]);
            w16(LCD_DATA, c[2]);
            w16(LCD_DATA, c[3]);
            w16(LCD_DATA, c[4]);
            w16(LCD_DATA, c[5]);
            w16(LCD_DATA, c[6]);
            w16(LCD_DATA, c[7]);
        }
    }
    for &c in it.remainder() {
        unsafe { w16(LCD_DATA, c) };
    }
}

// ------------------------------------------------------------ TE probe
//
// Whether the ILI9488's tearing-effect output (TE) reaches the MCU is
// unknown (../../hw/README.md "LCD"). DBG:TE? watches the four pins no
// firmware is known to use (PA3, PB13, PC1, PC15) while TE is off, on in
// mode 0 (V-blank) and on in mode 1 (V- and H-blank), then switches it
// off. It only reads the GPIOs (their reset input state is kept).

/// Candidate pins: (GPIO IDR address, bit).
#[cfg(target_os = "none")]
const TE_PINS: [(u32, u32); 4] = [(0x4002_0010, 3), (0x4002_0410, 13), (0x4002_0810, 1), (0x4002_0810, 15)];
/// Sampling time per phase (ms).
#[cfg(target_os = "none")]
const TE_MS: u32 = 100;

/// Per phase (TE off, mode 0, mode 1): the samples, then per candidate
/// pin the level changes and the samples high.
#[cfg(target_os = "none")]
#[inline(never)]
pub fn te_probe() -> [(u32, [(u32, u32); 4]); 3] {
    use core::ptr::read_volatile;
    let cyc = || unsafe { read_volatile(0xE000_1004 as *const u32) };
    let mut out = [(0u32, [(0u32, 0u32); 4]); 3];
    for (ph, (n, o)) in out.iter_mut().enumerate() {
        if ph == 0 {
            cmd(0x34); // TEOFF
        } else {
            cmd(0x35); // TEON, mode 0 / 1
            data(ph as u16 - 1);
        }
        let t0 = cyc();
        let mut last = [0u32; 4];
        while cyc().wrapping_sub(t0) < TE_MS * (crate::clock::HCLK_HZ / 1000) {
            for (i, &(idr, bit)) in TE_PINS.iter().enumerate() {
                let v = unsafe { read_volatile(idr as *const u32) } >> bit & 1;
                if *n > 0 && v != last[i] {
                    o[i].0 += 1;
                }
                o[i].1 += v;
                last[i] = v;
            }
            *n += 1;
        }
    }
    cmd(0x34);
    out
}
