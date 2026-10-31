//! Screenshot + log gallery (TODO FR-16): browse the S*.BMP screenshots and
//! LOG*.DLG recordings of the SPI-flash disk (SHOTS/, LOGS/ and the root's
//! files of earlier builds; fatdir.rs), view them full screen, delete them. Backend: csvlog.rs (`list_on` for the count and the
//! free space, `delete_on`); the reading side (directory pages, cluster
//! chains, BMP / log decoding) is here.
//!
//! Screens (480 x 320, stock fonts, the menu-page style of ui.rs):
//!   Grid    title bar ("< Back" | "Gallery" | count, free space), 3 x 2 tiles
//!           of 148 x 116 (thumbnail 140 x 93, name + size under it), a nav
//!           row (Prev / "Page i of n" / Next). Newest first: reverse order
//!           of the root, LOGS/, SHOTS/ (screenshots first, then logs, then
//!           the root's; new files are appended to their directory; a deleted
//!           slot that gets reused breaks the order, there is no RTC date).
//!           Swipe left / right = next / previous page, title bar = back.
//!   Viewer  BMP: the image full screen, drawn top-down; tap = overlay bar
//!           ("< Back" | name, index / total | Delete); swipe left / right =
//!           next / previous file. Log: header info, min / avg / max, a
//!           value-vs-time chart; the bar stays.
//!   Dialog  delete confirm (dialog.rs box and buttons, "Cancel" / "Delete"),
//!           or an info box with OK ("Disk in use by the PC", ...).
//!
//! Thumbnails are not stored: each is sampled from the file when its page is
//! shown (nearest neighbour, aspect kept: every 480 / 139 = 3.45th pixel of a
//! screenshot). A 480 x 320 x 16 bpp screenshot is 76 blocks and every block
//! holds a sampled row, so a thumbnail costs 76 block reads (~0.2 s at the
//! 12.5 MHz SPI clock, ~0.8 s if spiflash.rs fell back to /16) and a full page
//! ~1.2 s; the tiles fill in one after the other. The stock firmware's 17 KB
//! factory screenshots (4 bpp BI_RLE4, 16-colour palette) are decoded too (5
//! blocks). Formats: 16 bpp (565 bitfields, 555), 24 / 32 bpp, 4 bpp
//! (plain / RLE4), 1 bpp, bottom-up or top-down, any size up to 4096 x 4096
//! (scaled down to fit, never up); others (8 bpp, RLE8, JPEG / PNG in a BMP,
//! truncated files) show "Unsupported format".
//!
//! Logs are LOG*.DLG (compact binary, dlog.rs; their CSV exists only as the
//! export on LUN 1 / MMEM:DATA?), read chunk by chunk, every row. Scope
//! records (SCOPE/SCOP*.DLG) and fast-logger files (FAST*.DLG) are not
//! shown, and the count (title bar, pages) leaves them out.
//! Log tiles: function and duration, a sparkline of value vs time, the row
//! count. The file is read twice, one chunk per pass: pass A for count /
//! min / max / average / time span, pass B to draw (x = time).
//!
//! Main loop: `poll` does ONE bounded slice per pass: one data block read
//! (a log: one chunk, 1..2 block reads)
//! (+ at most one FAT block when the cached cluster runs end), or one
//! directory page scan (1..4 root blocks), or the catalog (csvlog::list_on:
//! boot block, root, FAT, ~7 blocks once on open / after the disk changed),
//! or a delete (synchronous in csvlog, ~50..400 ms, user-confirmed). The
//! full-screen image is 77 slices of one 4 KiB read (~2.7 ms) and drawing
//! (~0.2 ms) each: not blocking, so USB, the DMM and the watchdog keep
//! running. Drawing happens only from `draw` / `poll` (not from `touch`), so
//! the app's can_draw gate applies.
//!
//! Disk lock (one user of the disk at a time):
//!   * No read while a screenshot (screenshot::busy), a CSV commit
//!     (csvlog::writing) or a host WRITE(10) (msc::HOST_WRITING) is in
//!     progress: the slice waits ("Disk busy" in the nav row / on the
//!     viewer), and after the disk was busy the catalog and the page are
//!     re-read (a new screenshot appears, a recording grows, a host may have
//!     changed anything; tiles whose entry changed are redrawn).
//!   * No host writes this disk in the native build (USB exports the
//!     generated volume, vdisk.rs); csvlog::delete_on refuses while busy or
//!     for the file being recorded.
//!   * diskbuf::blocks() (3 x 4 KiB) is the block buffer: [0] data,
//!     [1] FAT 1. Only used inside one `poll` while the disk is free, never
//!     kept across passes (screenshot.rs, csvlog commits and the SCPI
//!     catalog use it between our passes).
//!
//! RAM: one `Gallery` (~0.7 KiB .bss); no frame or thumbnail buffers.

use core::sync::atomic::Ordering::Relaxed;

use crate::csvlog::{self, GErr, Kind};
use crate::diskbuf;
use crate::dlog::{self, Src as _};
use crate::fatren::{le16, le32, parse_bpb, Vol};
use crate::gesture::Gesture;
use crate::lcd::{self, Color};
use crate::msc::BlockDevice;
use crate::stockfont::{self as sf, rect, Rect};

const BLK: u32 = 4096;
const FPER: u32 = BLK / 2;
const NONE: u32 = u32::MAX;

// ------------------------------------------------------------------ look

/// ui.rs menu-page colours (page_title / list_page / main_menu).
const BAR: Color = 0x2966;
const FG: Color = 0xF7BE;
const M_BG: Color = 0x10A3;
const M_TILE: Color = 0x2146;
const ACCENT: Color = 0x04FC;
const DIM: Color = 0x94D3;
/// Thumbnail / viewer background, CSV panel, chart trace (ui::VALUE).
const IMG_BG: Color = 0x0000;
const PANEL: Color = 0x0883;
const GRID: Color = 0x2966;
const TRACE: Color = 0xFC40;
/// dialog.rs colours.
const D_TITLE: Color = 0x04FC;
const D_BODY: Color = 0x10A3;
const D_FG: Color = 0xF79E;
const D_BTN: Color = 0x2966;
const D_RED: Color = 0xB0A2;
const D_FRAME: Color = 0x94D3;

/// Title bar (ui.rs PAGE_BAR_H) and its "< Back" (x = ui.rs BAR_X + 3).
const BAR_H: i16 = 40;
const BACK_X: i16 = 7;
/// Tiles: 3 x 2 of TILE_W x TILE_H, thumbnail TH_W x TH_H at +4 / +4.
const PER_PAGE: usize = 6;
const TILE_W: i16 = 148;
const TILE_H: i16 = 116;
const TILE_X: [i16; 3] = [10, 166, 322];
const TILE_Y: [i16; 2] = [46, 168];
const TH_W: u16 = 140;
const TH_H: u16 = 93;
/// Nav row: buttons and the page text.
const NAV_Y: i16 = 288;
const NAV_H: i16 = 28;
const PREV_X: i16 = 10;
const NEXT_X: i16 = 360;
const NAV_W: i16 = 110;
/// Viewer bar: name group between "< Back" and Delete; the Delete pill.
const NAME_X0: i16 = 104;
const NAME_X1: i16 = 364;
const DEL: (i16, i16, i16, i16) = (372, 6, 100, 28);
/// Delete / OK hit areas in the bar (generous: the bar is 40 px tall).
const DEL_HIT_X: u16 = 368;
const BACK_HIT_X: u16 = 140;
/// CSV viewer: info lines, chart box, plot area.
const CHART: (i16, i16, i16, i16) = (8, 80, 464, 232);
const PLOT: (i16, i16, i16, i16) = (84, 88, 380, 196);
/// Info dialog OK button (centred under dialog::BOX).
const OK_BTN: (u16, u16, u16, u16) = (176, 180, 128, 48);


// ------------------------------------------------------------------ disk lock

/// Why the disk cannot be read now.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Busy {
    /// screenshot.rs is writing a shot.
    Shot,
    /// csvlog.rs is committing rows.
    Log,
    /// A host WRITE(10) is in progress.
    Host,
    /// An update file is being uploaded (upload.rs).
    Upload,
}

/// The disk lock (csvlog.rs `gallery_free`, the same three conditions).
pub fn busy_reason() -> Option<Busy> {
    if crate::screenshot::busy() {
        Some(Busy::Shot)
    } else if csvlog::writing() {
        Some(Busy::Log)
    } else if crate::msc::HOST_WRITING.load(Relaxed) {
        Some(Busy::Host)
    } else if crate::upload::busy() {
        Some(Busy::Upload)
    } else {
        None
    }
}

fn busy_text(b: Busy) -> &'static str {
    match b {
        Busy::Shot => "Saving a screenshot",
        Busy::Log => "Saving the CSV log",
        Busy::Host => "USB host is writing",
        Busy::Upload => "Firmware upload",
    }
}

/// The device's own SpiFlash handle (as screenshot.rs / csvlog.rs).
static mut DISK: Option<crate::spiflash::SpiFlash> = None;

fn disk() -> &'static mut crate::spiflash::SpiFlash {
    let d = unsafe { &mut *core::ptr::addr_of_mut!(DISK) };
    d.get_or_insert_with(crate::spiflash::SpiFlash::new)
}

/// Block I/O of one poll: blocks()[0] = data block, blocks()[1] = FAT 1 block,
/// tags valid for this poll only.
struct Io<'a> {
    disk: &'a mut dyn BlockDevice,
    v: Vol,
    data: &'a mut [u8; BLK as usize],
    fat: &'a mut [u8; BLK as usize],
    dtag: u32,
    ftag: u32,
}

impl<'a> Io<'a> {
    fn new(disk: &'a mut dyn BlockDevice, v: Vol) -> Self {
        let [data, fat, _] = diskbuf::blocks();
        Io { disk, v, data, fat, dtag: NONE, ftag: NONE }
    }

    fn block(&mut self, lba: u32) -> bool {
        if self.dtag == lba {
            return true;
        }
        self.dtag = NONE;
        if !self.disk.read_block(lba, &mut self.data[..]) {
            return false;
        }
        self.dtag = lba;
        true
    }

    /// FAT 1 entry of cluster `c`.
    fn fat_entry(&mut self, c: u32) -> Option<u32> {
        let fb = c / FPER;
        if fb >= self.v.fat_blocks {
            return None;
        }
        if self.ftag != fb {
            self.ftag = NONE;
            if !self.disk.read_block(self.v.fat_start + fb, &mut self.fat[..]) {
                return None;
            }
            self.ftag = fb;
        }
        Some(le16(&self.fat[..], ((c % FPER) * 2) as usize))
    }

    /// Whether `c`'s FAT entry is in the cached block.
    fn fat_cached(&self, c: u32) -> bool {
        self.ftag == c / FPER
    }
}

// ------------------------------------------------------------------ entries

/// One gallery file (root entry).
#[derive(Clone, Copy, PartialEq, Eq)]
struct Ent {
    name: [u8; 11],
    size: u32,
    start: u16,
    kind: Kind,
}

const ENT0: Ent = Ent { name: [b' '; 11], size: 0, start: 0, kind: Kind::Bmp };

/// S*.BMP / LOG*.CSV files (csvlog.rs kind_of: not a directory, label or
/// long-name entry; free / end entries excluded here).
fn kind_of(e: &[u8]) -> Option<Kind> {
    if e[0] == 0 || e[0] == 0xE5 || e[11] & 0x18 != 0 || e[11] & 0x3F == 0x0F {
        return None;
    }
    let up = |c: u8| c.to_ascii_uppercase();
    let ext = [up(e[8]), up(e[9]), up(e[10])];
    if up(e[0]) == b'S' && &ext == b"BMP" {
        Some(Kind::Bmp)
    } else if up(e[0]) == b'L' && up(e[1]) == b'O' && up(e[2]) == b'G' && &ext == b"DLG" {
        Some(Kind::Csv)
    } else {
        None
    }
}

/// Directory scan (the root's legacy files, then LOGS/, then SHOTS/:
/// fatdir::for_each): gallery files with index lo..hi into `out` (in that
/// order); `name` set: also the index of that file. Returns the number of
/// gallery files, the number stored and the found index.
fn scan(io: &mut Io, lo: u32, hi: u32, out: &mut [Ent], name: Option<&[u8; 11]>) -> Option<(u32, usize, Option<(u32, Ent)>)> {
    let v = io.v;
    let (mut idx, mut n, mut hit) = (0u32, 0usize, None);
    io.dtag = NONE;
    let r = crate::fatdir::for_each(&mut *io.disk, &v, &[&crate::fatdir::LOGS, &crate::fatdir::SHOTS], io.data, &mut |_, _, e| {
        let Some(kind) = kind_of(e) else { return true };
        let mut nm = [0u8; 11];
        nm.copy_from_slice(&e[..11]);
        let ent = Ent { name: nm, size: le32(e, 0x1C), start: le16(e, 0x1A) as u16, kind };
        if idx >= lo && idx < hi && n < out.len() {
            out[n] = ent;
            n += 1;
        }
        if name == Some(&nm) {
            hit = Some((idx, ent));
        }
        idx += 1;
        true
    });
    r.ok()?;
    Some((idx, n, hit))
}

// ------------------------------------------------------------------ chains

const NRUN: usize = 12;

/// A window of a file's cluster chain as runs of consecutive clusters,
/// extended from the FAT on demand (greedily while the FAT block is cached,
/// so a contiguous file needs one FAT read). Going back before the window
/// restarts from the first cluster.
#[derive(Clone, Copy)]
struct Chain {
    start: u16,
    /// File cluster index of runs[0].
    base: u32,
    runs: [(u16, u16); NRUN],
    n: u8,
    /// The chain ends after the last run.
    end: bool,
    /// Clusters walked since the reset (loop guard).
    steps: u32,
}

const CHAIN0: Chain = Chain { start: 0, base: 0, runs: [(0, 0); NRUN], n: 0, end: true, steps: 0 };

impl Chain {
    fn new(start: u16) -> Self {
        let mut c = CHAIN0;
        c.start = start;
        c.reset(u32::MAX);
        c
    }

    fn reset(&mut self, max: u32) {
        self.base = 0;
        self.steps = 0;
        let ok = self.start >= 2 && (self.start as u32) <= max;
        self.n = ok as u8;
        self.end = !ok;
        self.runs[0] = (self.start, 1);
    }

    /// Cluster number of file cluster `k`.
    fn get(&mut self, io: &mut Io, k: u32) -> Option<u32> {
        let max = io.v.clusters + 1;
        if self.n == 0 || k < self.base {
            self.reset(max);
        }
        let mut found = None;
        loop {
            if found.is_none() {
                let mut b = self.base;
                for &(f, l) in &self.runs[..self.n as usize] {
                    if k < b + l as u32 {
                        found = Some(f as u32 + (k - b));
                        break;
                    }
                    b += l as u32;
                }
            }
            if self.end {
                return found;
            }
            let (f, l) = self.runs[self.n as usize - 1];
            let last = f as u32 + l as u32 - 1;
            // Found: keep extending only for free (cached FAT block, no run
            // dropped), so the next blocks need no FAT read.
            if found.is_some() && (!io.fat_cached(last) || self.n as usize == NRUN) {
                return found;
            }
            let nx = io.fat_entry(last)?;
            self.steps += 1;
            if nx < 2 || nx > max || self.steps > io.v.clusters {
                self.end = true;
                continue;
            }
            let i = self.n as usize - 1;
            if nx == last + 1 && l < u16::MAX {
                self.runs[i].1 += 1;
            } else if (self.n as usize) < NRUN {
                self.runs[i + 1] = (nx as u16, 1);
                self.n += 1;
            } else if found.is_none() {
                self.base += self.runs[0].1 as u32;
                self.runs.copy_within(1.., 0);
                self.runs[NRUN - 1] = (nx as u16, 1);
            } else {
                return found;
            }
        }
    }
}

/// LBA of file block `fb`.
fn file_lba(io: &mut Io, ch: &mut Chain, fb: u32) -> Option<u32> {
    let spc = io.v.spc;
    let c = ch.get(io, fb / spc)?;
    Some(io.v.data_start + (c - 2) * spc + fb % spc)
}

/// Read file block `fb` into io.data.
/// A gallery file as a dlog::Src (through the block cache and its chain).
struct GSrc<'a, 'b, 'c> {
    io: &'a mut Io<'c>,
    chain: &'b mut Chain,
    size: u32,
}

impl dlog::Src for GSrc<'_, '_, '_> {
    fn read(&mut self, mut off: u32, buf: &mut [u8]) -> bool {
        if off as u64 + buf.len() as u64 > self.size as u64 {
            return false;
        }
        let mut i = 0;
        while i < buf.len() {
            let fb = off / BLK;
            if !read_fb(self.io, self.chain, fb) {
                return false;
            }
            let o = (off % BLK) as usize;
            let n = (BLK as usize - o).min(buf.len() - i);
            buf[i..i + n].copy_from_slice(&self.io.data[o..o + n]);
            i += n;
            off += n as u32;
        }
        true
    }
}

fn read_fb(io: &mut Io, ch: &mut Chain, fb: u32) -> bool {
    match file_lba(io, ch, fb) {
        Some(lba) => io.block(lba),
        None => false,
    }
}

// ------------------------------------------------------------------ BMP

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Fmt {
    Rgb565,
    Rgb555,
    Rgb24,
    Rgb32,
    Pal4,
    Pal1,
    Rle4,
}

#[derive(Clone, Copy)]
struct Bmp {
    fmt: Fmt,
    w: u32,
    h: u32,
    /// Bottom-up rows (positive height).
    up: bool,
    off: u32,
    /// Bytes per stored row (padded to 4).
    row: u32,
    pal: [Color; 16],
}

const BMP0: Bmp = Bmp { fmt: Fmt::Rgb565, w: 0, h: 0, up: true, off: 0, row: 0, pal: [0; 16] };

const fn rgb(r: u8, g: u8, b: u8) -> Color {
    ((r as u16 & 0xF8) << 8) | ((g as u16 & 0xFC) << 3) | (b as u16 >> 3)
}

impl Bmp {
    /// Header (first block `b`, file `size` bytes). Err((bpp, compression))
    /// for an unsupported or damaged file.
    fn parse(b: &[u8], size: u32) -> Result<Bmp, (u16, u16)> {
        let bad = Err((0, 0));
        if size < 54 || b.len() < 66 || &b[..2] != b"BM" {
            return bad;
        }
        let (off, hs) = (le32(b, 10), le32(b, 14));
        let (w, h) = (le32(b, 18) as i32, le32(b, 22) as i32);
        let (planes, bpp, comp, clr) = (le16(b, 26), le16(b, 28) as u16, le32(b, 30), le32(b, 46));
        let unsup = Err((bpp, comp as u16));
        if !(40..=124).contains(&hs) || planes != 1 || w <= 0 || w > 4096 || h == 0 || h.unsigned_abs() > 4096 {
            return unsup;
        }
        let masks = (le32(b, 54), le32(b, 58), le32(b, 62));
        let fmt = match (bpp, comp) {
            (16, 0) => Fmt::Rgb555,
            (16, 3) if masks == (0xF800, 0x07E0, 0x001F) => Fmt::Rgb565,
            (16, 3) if masks == (0x7C00, 0x03E0, 0x001F) => Fmt::Rgb555,
            (24, 0) => Fmt::Rgb24,
            (32, 0) => Fmt::Rgb32,
            (32, 3) if masks == (0x00FF_0000, 0x0000_FF00, 0x0000_00FF) => Fmt::Rgb32,
            (4, 0) => Fmt::Pal4,
            (4, 2) if h > 0 => Fmt::Rle4,
            (1, 0) => Fmt::Pal1,
            _ => return unsup,
        };
        let (w, up, h) = (w as u32, h > 0, h.unsigned_abs());
        let mut m = Bmp { fmt, w, h, up, off, row: (w * bpp as u32).div_ceil(32) * 4, pal: [0; 16] };
        if matches!(fmt, Fmt::Pal4 | Fmt::Rle4 | Fmt::Pal1) {
            let n = if clr == 0 || clr > 1 << bpp { 1u32 << bpp } else { clr };
            let p0 = 14 + hs;
            if p0 + 4 * n > off || p0 + 4 * n > b.len() as u32 {
                return unsup;
            }
            for i in 0..n as usize {
                let q = (p0 as usize) + 4 * i;
                m.pal[i] = rgb(b[q + 2], b[q + 1], b[q]);
            }
        }
        let need = if fmt == Fmt::Rle4 { off + 2 } else { off + m.row * h };
        if off < 14 + hs || need > size {
            return unsup;
        }
        Ok(m)
    }

    /// Bytes of a pixel and its first byte in the stored row.
    fn pix(&self, x: u32) -> (u32, u32) {
        match self.fmt {
            Fmt::Pal1 => (x / 8, 1),
            Fmt::Pal4 | Fmt::Rle4 => (x / 2, 1),
            Fmt::Rgb565 | Fmt::Rgb555 => (2 * x, 2),
            Fmt::Rgb24 => (3 * x, 3),
            Fmt::Rgb32 => (4 * x, 4),
        }
    }

    fn color(&self, p: &[u8], x: u32) -> Color {
        match self.fmt {
            Fmt::Rgb565 => p[0] as u16 | (p[1] as u16) << 8,
            Fmt::Rgb555 => {
                let c = p[0] as u16 | (p[1] as u16) << 8;
                ((c & 0x7FE0) << 1) | ((c >> 4) & 0x20) | (c & 0x1F)
            }
            Fmt::Rgb24 | Fmt::Rgb32 => rgb(p[2], p[1], p[0]),
            Fmt::Pal4 | Fmt::Rle4 => self.pal[(if x & 1 == 0 { p[0] >> 4 } else { p[0] & 15 }) as usize],
            Fmt::Pal1 => self.pal[((p[0] >> (7 - (x & 7))) & 1) as usize],
        }
    }
}

/// Image -> screen rectangle mapping (fit inside a box, aspect kept, never
/// enlarged, centred; nearest neighbour: dest dx shows source floor(dx *
/// sw / dw)). Only screen rows y_lo..y_hi are drawn.
#[derive(Clone, Copy)]
struct Sink {
    ox: i16,
    oy: i16,
    dw: u32,
    dh: u32,
    sw: u32,
    sh: u32,
    up: bool,
    y_lo: i16,
    y_hi: i16,
    /// Thumbnail: probe key for the layout check (0 = none).
    key: u32,
    /// Open LCD run: row and next x (the window continues there).
    run_y: i16,
    run_x: i16,
    /// Cached row of file row `ck`: dest row or -1.
    ck: u32,
    cdy: i32,
}

const SINK0: Sink = Sink { ox: 0, oy: 0, dw: 1, dh: 1, sw: 1, sh: 1, up: true, y_lo: 0, y_hi: 0, key: 0, run_y: -1, run_x: 0, ck: NONE, cdy: -1 };

impl Sink {
    fn fit(sw: u32, sh: u32, up: bool, bx: i16, by: i16, bw: u16, bh: u16) -> Sink {
        let (bw32, bh32) = (bw as u32, bh as u32);
        let (mut dw, mut dh) = (sw, sh);
        if dw > bw32 || dh > bh32 {
            if sw * bh32 >= sh * bw32 {
                dw = bw32;
                dh = (sh * bw32 / sw).max(1);
            } else {
                dh = bh32;
                dw = (sw * bh32 / sh).max(1);
            }
        }
        Sink {
            ox: bx + ((bw32 - dw) / 2) as i16,
            oy: by + ((bh32 - dh) / 2) as i16,
            dw,
            dh,
            sw,
            sh,
            up,
            y_lo: by,
            y_hi: by + bh as i16,
            ..SINK0
        }
    }

    /// Dest row of stored row `k` (sampled and inside the band).
    fn row(&mut self, k: u32) -> Option<u32> {
        if k != self.ck {
            self.ck = k;
            self.cdy = -1;
            if k < self.sh {
                let sy = if self.up { self.sh - 1 - k } else { k };
                let dy = (sy * self.dh).div_ceil(self.sh);
                let y = self.oy as i32 + dy as i32;
                if dy < self.dh && dy * self.sh / self.dh == sy && y >= self.y_lo as i32 && y < self.y_hi as i32 {
                    self.cdy = dy as i32;
                }
            }
        }
        (self.cdy >= 0).then_some(self.cdy as u32)
    }

    fn src_x(&self, dx: u32) -> u32 {
        if self.dw == self.sw {
            return dx;
        }
        dx * self.sw / self.dw
    }

    /// Dest column of source column `x`.
    fn col(&self, x: u32) -> Option<u32> {
        if self.dw == self.sw {
            return (x < self.dw).then_some(x);
        }
        let dx = (x * self.dw).div_ceil(self.sw);
        (dx < self.dw && self.src_x(dx) == x).then_some(dx)
    }

    /// Stored rows that hold the band's dest rows: (first, last), or None.
    fn rows(&self) -> Option<(u32, u32)> {
        let dy0 = (self.y_lo as i32 - self.oy as i32).max(0) as u32;
        let dy1 = ((self.y_hi as i32 - self.oy as i32).max(0) as u32).min(self.dh);
        if dy0 >= dy1 {
            return None;
        }
        let (sy0, sy1) = (dy0 * self.sh / self.dh, (dy1 - 1) * self.sh / self.dh);
        Some(if self.up { (self.sh - 1 - sy1, self.sh - 1 - sy0) } else { (sy0, sy1) })
    }

    fn reset_run(&mut self) {
        self.run_y = -1;
    }

    fn put(&mut self, dx: u32, dy: u32, c: Color) {
        let (x, y) = (self.ox + dx as i16, self.oy + dy as i16);
        if y != self.run_y || x != self.run_x {
            lcd::set_window(x as u16, y as u16, (self.ox as u32 + self.dw - 1) as u16, y as u16);
        }
        lcd::data(c);
        self.run_y = y;
        self.run_x = x + 1;
    }
}

/// A pixel that straddles a block boundary (24 / 32 bpp, or 16 bpp at an
/// odd data offset): its bytes collected over the two blocks.
#[derive(Clone, Copy)]
struct Carry {
    p: u32,
    dx: u32,
    dy: u32,
    x: u32,
    buf: [u8; 4],
    have: u8,
}

const CARRY0: Carry = Carry { p: NONE, dx: 0, dy: 0, x: 0, buf: [0; 4], have: 0 };

/// BI_RLE4 decoder state (bytes fed one at a time, across blocks).
#[derive(Clone, Copy)]
struct Rle {
    st: u8,
    n: u8,
    dx: u8,
    left: u16,
    bytes: u16,
    x: u32,
    k: u32,
    done: bool,
}

const RLE0: Rle = Rle { st: 0, n: 0, dx: 0, left: 0, bytes: 0, x: 0, k: 0, done: false };

impl Rle {
    fn feed(&mut self, c: u8, out: &mut impl FnMut(u32, u32, u8)) {
        match self.st {
            0 => {
                if c == 0 {
                    self.st = 2;
                } else {
                    self.n = c;
                    self.st = 1;
                }
            }
            1 => {
                for i in 0..self.n as u32 {
                    out(self.x + i, self.k, if i & 1 == 0 { c >> 4 } else { c & 15 });
                }
                self.x += self.n as u32;
                self.st = 0;
            }
            2 => match c {
                0 => {
                    self.x = 0;
                    self.k += 1;
                    self.st = 0;
                }
                1 => self.done = true,
                2 => self.st = 3,
                n => {
                    self.left = n as u16;
                    self.bytes = ((n as u16).div_ceil(2) + 1) & !1;
                    self.st = 5;
                }
            },
            3 => {
                self.dx = c;
                self.st = 4;
            }
            4 => {
                self.x += self.dx as u32;
                self.k += c as u32;
                self.st = 0;
            }
            _ => {
                for nib in [c >> 4, c & 15] {
                    if self.left > 0 {
                        out(self.x, self.k, nib);
                        self.x += 1;
                        self.left -= 1;
                    }
                }
                self.bytes -= 1;
                if self.bytes == 0 {
                    self.st = 0;
                }
            }
        }
    }
}

// ------------------------------------------------------------------ CSV

/// Log reader state: header fields, pass A statistics, pass B plot state.
#[derive(Clone, Copy)]
struct Csv {
    func: [u8; 16],
    fl: u8,
    range: [u8; 8],
    rl: u8,
    unit: [u8; 8],
    ul: u8,
    /// File offset of the first chunk (after the header text).
    hdr: u32,
    rows: u32,
    vals: u32,
    min: f32,
    max: f32,
    sum: f32,
    comp: f32,
    t0: u32,
    t1: u32,
    // pass B
    pb: bool,
    plot: (i16, i16, i16, i16),
    key: u32,
    col: i32,
    cmin: i32,
    cmax: i32,
    cfirst: i32,
    clast: i32,
    lcol: i32,
    ly: i32,
    /// A fast-logger file (dlog kind 3, FASTnnn.DLG): its parameters.
    fast: Option<dlog::FPar>,
}

const CSV0: Csv = Csv {
    func: [0; 16],
    fl: 0,
    range: [0; 8],
    rl: 0,
    unit: [0; 8],
    ul: 0,
    hdr: 0,
    rows: 0,
    vals: 0,
    min: f32::INFINITY,
    max: f32::NEG_INFINITY,
    sum: 0.0,
    comp: 0.0,
    t0: u32::MAX,
    t1: 0,
    pb: false,
    plot: (0, 0, 1, 1),
    key: 0,
    col: -1,
    cmin: 0,
    cmax: 0,
    cfirst: 0,
    clast: 0,
    lcol: -1,
    ly: 0,
    fast: None,
};

fn find(h: &[u8], n: &[u8]) -> Option<usize> {
    (0..=h.len().saturating_sub(n.len())).find(|&i| h.len() >= n.len() && &h[i..i + n.len()] == n)
}

/// Copy `h` from after `key` up to the next ',' into `out`.
fn field_after(h: &[u8], key: &[u8], out: &mut [u8]) -> u8 {
    let Some(i) = find(h, key) else { return 0 };
    let mut n = 0;
    for &c in &h[i + key.len()..] {
        if c == b',' || n == out.len() {
            break;
        }
        out[n] = if c.is_ascii_graphic() || c == b' ' { c } else { b'?' };
        n += 1;
    }
    n as u8
}

impl Csv {
    /// Function and range from the log's header text (the CSV header
    /// lines: "# ..., function DC V, range AUTO, ...").
    fn header(&mut self, t: &[u8]) {
        let l = t.split(|&c| c == b'\n').next().unwrap_or(&[]);
        self.fl = field_after(l, b"function ", &mut self.func);
        self.rl = field_after(l, b"range ", &mut self.range);
    }

    /// One reading: pass A statistics or pass B plot.
    fn sample(&mut self, t: u32, v: Option<f32>) {
        if self.pb {
            self.plot_row(t, v);
            return;
        }
        self.rows += 1;
        self.t0 = self.t0.min(t);
        self.t1 = self.t1.max(t);
        if let Some(v) = v.filter(|v| v.is_finite()) {
            self.vals += 1;
            self.min = self.min.min(v);
            self.max = self.max.max(v);
            // Kahan sum (f32 only on this FPU).
            let y = v - self.comp;
            let s = self.sum + y;
            self.comp = (s - self.sum) - y;
            self.sum = s;
        }
    }

    fn avg(&self) -> f32 {
        if self.vals == 0 { f32::NAN } else { self.sum / self.vals as f32 }
    }

    fn begin_plot(&mut self, plot: (i16, i16, i16, i16), key: u32) {
        self.pb = true;
        self.plot = plot;
        self.key = key;
        self.col = -1;
        self.lcol = -1;
    }

    fn vy(&self, v: f32) -> i32 {
        let (_, py, _, ph) = self.plot;
        let span = self.max - self.min;
        let f = if span > 0.0 { (v - self.min) / span } else { 0.5 };
        py as i32 + ph as i32 - 1 - (f.clamp(0.0, 1.0) * (ph as f32 - 1.0) + 0.5) as i32
    }

    fn plot_row(&mut self, t: u32, v: Option<f32>) {
        let (_, _, pw, _) = self.plot;
        let span = self.t1.saturating_sub(self.t0);
        let col = if span == 0 {
            pw as i32 / 2
        } else {
            (t.saturating_sub(self.t0).min(span) as u64 * (pw as u64 - 1) / span as u64) as i32
        };
        match v.filter(|v| v.is_finite()) {
            None => {
                self.flush();
                self.lcol = -1;
            }
            Some(v) => {
                let y = self.vy(v);
                if col != self.col {
                    self.flush();
                    self.col = col;
                    self.cmin = y;
                    self.cmax = y;
                    self.cfirst = y;
                } else {
                    self.cmin = self.cmin.min(y);
                    self.cmax = self.cmax.max(y);
                }
                self.clast = y;
            }
        }
    }

    /// Draw the column collected so far, joined to the previous one.
    fn flush(&mut self) {
        if self.col < 0 {
            return;
        }
        let (mut a, mut b) = (self.cmin, self.cmax);
        if self.lcol >= 0 && self.col > self.lcol {
            // Straight line over the columns in between, then into this one.
            let (c0, y0, n) = (self.lcol, self.ly, self.col - self.lcol);
            let mut yp = y0;
            for x in c0 + 1..self.col {
                let y = y0 + (self.cfirst - y0) * (x - c0) / n;
                self.vline(x, yp.min(y), yp.max(y));
                yp = y;
            }
            a = a.min(yp);
            b = b.max(yp);
        }
        self.vline(self.col, a, b);
        self.lcol = self.col;
        self.ly = self.clast;
        self.col = -1;
    }

    fn vline(&self, x: i32, a: i32, b: i32) {
        let (px, _, _, _) = self.plot;
        probe_begin(self.key, PANEL);
        lcd::fill_rect((px as i32 + x) as u16, a as u16, 1, (b - a + 1) as u16, TRACE);
        probe_end();
    }
}

// ------------------------------------------------------------------ text

/// Small text buffer.
struct Tb {
    b: [u8; 48],
    n: usize,
}

impl Tb {
    const fn new() -> Tb {
        Tb { b: [0; 48], n: 0 }
    }
    #[inline(never)] // one copy: was inlined 9 times next to its own symbol (region C audit #1)
    fn c(&mut self, c: u8) {
        if self.n < self.b.len() {
            self.b[self.n] = if c.is_ascii_graphic() || c == b' ' { c } else { b'?' };
            self.n += 1;
        }
    }
    fn s(&mut self, t: &str) {
        self.bytes(t.as_bytes());
    }
    fn bytes(&mut self, t: &[u8]) {
        for &c in t {
            self.c(c);
        }
    }
    fn u(&mut self, v: u32) {
        let mut d = [0u8; 10];
        let (mut k, mut v) = (d.len(), v);
        loop {
            k -= 1;
            d[k] = b'0' + (v % 10) as u8;
            v /= 10;
            if v == 0 {
                break;
            }
        }
        for i in k..d.len() {
            self.c(d[i]);
        }
    }
    fn u2(&mut self, v: u32) {
        self.c(b'0' + (v / 10 % 10) as u8);
        self.c(b'0' + (v % 10) as u8);
    }
    /// `v` with `dec` decimals.
    fn fixed(&mut self, v: f32, dec: u32) {
        let p = 10u32.pow(dec);
        let m = (v.abs() * p as f32 + 0.5) as u64;
        if v < 0.0 && m != 0 {
            self.c(b'-');
        }
        self.u((m / p as u64) as u32);
        if dec > 0 {
            self.c(b'.');
            let f = (m % p as u64) as u32;
            let mut q = p / 10;
            while q > 0 {
                self.c(b'0' + (f / q % 10) as u8);
                q /= 10;
            }
        }
    }
    fn str(&self) -> &str {
        core::str::from_utf8(&self.b[..self.n]).unwrap_or("")
    }
}

/// "S003    BMP" -> "S003.BMP".
fn name_txt(n: &[u8; 11]) -> Tb {
    let mut t = Tb::new();
    for &c in n[..8].iter().filter(|&&c| c != b' ') {
        t.c(c);
    }
    if n[8] != b' ' {
        t.c(b'.');
        for &c in n[8..].iter().filter(|&&c| c != b' ') {
            t.c(c);
        }
    }
    t
}

/// 307266 -> "300 KB", 12345678 -> "11.8 MB".
fn size_txt(t: &mut Tb, n: u32) {
    if n < 1000 {
        t.u(n);
        t.s(" B");
    } else if n < 1000 * 1024 {
        t.u((n + 512) / 1024);
        t.s(" KB");
    } else {
        let tenths = ((n as u64 * 10 + 524_288) / 1_048_576) as u32;
        t.u(tenths / 10);
        if tenths < 100 {
            t.c(b'.');
            t.u(tenths % 10);
        }
        t.s(" MB");
    }
}

/// ms -> "12.3 s", "4:05", "1:02:03".
fn dur_txt(t: &mut Tb, ms: u32) {
    let s = ms / 1000;
    if s < 60 {
        t.fixed(ms as f32 / 1000.0, 1);
        t.s(" s");
    } else if s < 3600 {
        t.u(s / 60);
        t.c(b':');
        t.u2(s % 60);
    } else {
        t.u(s / 3600);
        t.c(b':');
        t.u2(s / 60 % 60);
        t.c(b':');
        t.u2(s % 60);
    }
}

/// `v` with 5 significant digits; SI prefix for the SI units ("V", "A",
/// "Ohm", "F", "Hz", "W", "s"), then the unit when `unit` is Some.
fn val_txt(t: &mut Tb, v: f32, unit: &[u8], with_unit: bool) {
    if !v.is_finite() {
        t.s("--");
        return;
    }
    let si = matches!(unit, b"V" | b"A" | b"Ohm" | b"F" | b"Hz" | b"W" | b"s");
    let a = v.abs();
    let (k, p) = if !si || a == 0.0 || (1.0..1000.0).contains(&a) {
        (1.0, "")
    } else if a >= 1e9 {
        (1e-9, "G")
    } else if a >= 1e6 {
        (1e-6, "M")
    } else if a >= 1e3 {
        (1e-3, "k")
    } else if a >= 1e-3 {
        (1e3, "m")
    } else if a >= 1e-6 {
        (1e6, "u")
    } else if a >= 1e-9 {
        (1e9, "n")
    } else {
        (1e12, "p")
    };
    let s = v * k;
    let m = s.abs();
    let dec = if m >= 10000.0 { 0 } else if m >= 1000.0 { 1 } else if m >= 100.0 { 2 } else if m >= 10.0 { 3 } else { 4 };
    t.fixed(s, dec);
    if with_unit || !p.is_empty() {
        t.c(b' ');
    }
    t.s(p);
    if with_unit {
        t.bytes(unit);
    }
}

// ------------------------------------------------------------------ probes

/// Layout-check probes (tools/gallery-test, --cfg test): one keyed item per
/// thumbnail / trace across many draw calls.
#[cfg(test)]
fn probe_begin(key: u32, bg: Color) {
    if key != 0 {
        crate::probe::begin_key(crate::probe::ICON, bg, key);
    }
}
#[cfg(test)]
fn probe_end() {
    crate::probe::end();
}
#[cfg(not(test))]
#[inline(always)]
fn probe_begin(_: u32, _: Color) {}
#[cfg(not(test))]
#[inline(always)]
fn probe_end() {}

// ------------------------------------------------------------------ drawing

fn fill(r: (i16, i16, i16, i16), c: Color) {
    lcd::fill_rect(r.0 as u16, r.1 as u16, r.2 as u16, r.3 as u16, c);
}

fn frame(r: (u16, u16, u16, u16), c: Color) {
    lcd::fill_rect(r.0, r.1, r.2, 1, c);
    lcd::fill_rect(r.0, r.1 + r.3 - 1, r.2, 1, c);
    lcd::fill_rect(r.0, r.1, 1, r.3, c);
    lcd::fill_rect(r.0 + r.2 - 1, r.1, 1, r.3, c);
}

fn r4(r: (u16, u16, u16, u16)) -> Rect {
    rect(r.0 as i16, r.1 as i16, r.2 as i16, r.3 as i16)
}

/// ui.rs page_title: bar, "< Back".
fn title_bar() {
    fill((0, 0, 480, BAR_H), BAR);
    sf::glyphs(sf::TEXT22, rect(BACK_X, 0, 110, BAR_H), "< Back", FG, BAR, 1, 0);
}

/// Word space of F14 text, px from the last glyph box of a word to the next
/// word. The stock F14 space glyph is 1 px wide (advance 1), which left
/// "40rows"-like gaps of 2 px; words are therefore placed one by one.
const F14_SPACE: i16 = 5;

/// Width of `t` as `small` draws it.
fn small_width(t: &str) -> i16 {
    let mut w = 0;
    for (i, word) in t.split(' ').filter(|w| !w.is_empty()).enumerate() {
        w += sf::text_width(sf::F14, word, 1) + if i > 0 { F14_SPACE } else { 0 };
    }
    w
}

/// F14 text (spacing 1) in `r` with real word spaces; `align` as stockfont
/// (0..2 left, 3..5 right, 6..8 centred; top / vcenter / bottom). Draws only
/// the glyph boxes (as sf::glyphs).
fn small(r: Rect, t: &str, fg: Color, bg: Color, align: u8) {
    let tw = small_width(t);
    let mut x = r.x
        + match align {
            3..=5 => r.w - tw,
            6..=8 => (r.w - tw) / 2,
            _ => 0,
        };
    let v = match align {
        0 | 3 | 6 => 0,
        2 | 5 | 7 => 2,
        _ => 1,
    };
    for word in t.split(' ').filter(|w| !w.is_empty()) {
        let w = sf::text_width(sf::F14, word, 1);
        sf::glyphs(sf::F14, rect(x, r.y, w + 1, r.h), word, fg, bg, v, 1);
        x += w + F14_SPACE;
    }
}

/// Primary message (TEXT22) and a detail line (F14) centred in the body.
fn message(bg: Color, main: &str, detail: &str) {
    sf::glyphs(sf::TEXT22, rect(0, 136, 480, 30), main, FG, bg, 8, 0);
    if !detail.is_empty() {
        small(rect(0, 172, 480, 16), detail, DIM, bg, 8);
    }
}

fn tile_xy(i: usize) -> (i16, i16) {
    (TILE_X[i % 3], TILE_Y[i / 3])
}

fn inside(r: (u16, u16, u16, u16), x: u16, y: u16) -> bool {
    x >= r.0 && x < r.0 + r.2 && y >= r.1 && y < r.1 + r.3
}

/// The dialog box (dialog.rs layout and colours): title, body line,
/// buttons (Cancel / `yes`, or a single OK).
fn dialog_box(title: &str, body: &str, yes: Option<&str>) {
    let b = crate::dialog::BOX;
    let lab = |r: (u16, u16, u16, u16), s: &str, bg: Color| sf::label(sf::ASCII26, r4(r), s, D_FG, bg, 8, 1);
    lab((b.0, b.1, b.2, 40), title, D_TITLE);
    lab((b.0, b.1 + 40, b.2, 60), body, D_BODY);
    lcd::fill_rect(b.0, b.1 + 100, b.2, b.3 - 100, D_BODY);
    frame(b, D_FRAME);
    match yes {
        Some(y) => {
            lab(crate::dialog::NO, "Cancel", D_BTN);
            frame(crate::dialog::NO, D_FRAME);
            lab(crate::dialog::YES, y, D_RED);
            frame(crate::dialog::YES, D_FRAME);
        }
        None => {
            lab(OK_BTN, "OK", D_BTN);
            frame(OK_BTN, D_FRAME);
        }
    }
}

// ------------------------------------------------------------------ state

/// What `touch` / `back` asks of the app.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Out {
    Stay,
    /// Leave the gallery (back to where it was opened from).
    Exit,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Mode {
    Grid,
    View,
    Dialog,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Dlg {
    Delete,
    /// Confirmed, the delete runs in the next poll.
    Deleting,
    /// Title, body.
    Info(&'static str, &'static str),
}

/// Disk / catalog state.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum St {
    Listing,
    Ok,
    NoDisk,
    Error,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum JK {
    None,
    /// BMP / CSV thumbnail of tile i.
    Tile(u8),
    /// Full-screen BMP (band = the rows to draw).
    View,
    /// CSV viewer chart.
    Chart,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Ph {
    Head,
    Pix,
    A,
    B,
}

struct Job {
    kind: JK,
    ph: Ph,
    ent: Ent,
    chain: Chain,
    bmp: Bmp,
    sink: Sink,
    carry: [Carry; 2],
    rle: Rle,
    /// Pix: next block, last block, descending; A / B: sample index, count.
    i: u32,
    end: u32,
    desc: bool,
    csv: Csv,
    /// csv holds pass-A results for `ent` (a redraw starts at pass B).
    stats: bool,
}

const JOB0: Job = Job {
    kind: JK::None,
    ph: Ph::Head,
    ent: ENT0,
    chain: CHAIN0,
    bmp: BMP0,
    sink: SINK0,
    carry: [CARRY0; 2],
    rle: RLE0,
    i: 0,
    end: 0,
    desc: false,
    csv: CSV0,
    stats: false,
};

// need flags (disk work for `poll`)
const N_LIST: u8 = 1;
const N_PAGE: u8 = 2;
const N_CUR: u8 = 4;
const N_FIND: u8 = 8;
const N_DEL: u8 = 16;
// paint flags (drawing for `poll` / `draw`)
const P_ALL: u8 = 1;
const P_BAR: u8 = 2;
const P_INFO: u8 = 4;
const P_NAV: u8 = 8;
const P_DLG: u8 = 16;
const P_MSG: u8 = 32;

pub struct Gallery {
    mode: Mode,
    dlg: Dlg,
    st: St,
    busy: Option<Busy>,
    v: Option<Vol>,
    need: u8,
    paint: u8,
    /// Tiles whose static part is to be painted / whose thumbnail is to be made.
    tpaint: u8,
    todo: u8,
    total: u32,
    free: u32,
    page: u32,
    ents: [Ent; PER_PAGE],
    n: usize,
    /// Viewer: rank (0 = newest) and entry; overlay bar shown.
    rank: u32,
    cur: Ent,
    overlay: bool,
    /// Viewer: the image is complete (a band restore is enough).
    full: bool,
    /// Viewer: a message instead of the image (unsupported / error).
    vmsg: Option<(&'static str, u16, u16)>,
    /// Viewer: rows to restore after the running job.
    band: Option<(i16, i16)>,
    job: Job,
}

/// Zero-initialised (.bss), set to `Gallery::new()` on the first `get`: as a
/// const initialiser it was 672 B of the .data load image in region C
/// (sentinels such as NONE are not zero; region C audit #1).
static mut G: core::mem::MaybeUninit<Gallery> = core::mem::MaybeUninit::uninit();
static mut G_INIT: bool = false;

/// The gallery state (one, static: no room on the main stack).
#[inline(never)] // .cold (link-native.x): the callers are the gallery's own paths
pub fn get() -> &'static mut Gallery {
    unsafe {
        let g = &mut *core::ptr::addr_of_mut!(G);
        if !G_INIT {
            G_INIT = true;
            g.write(Gallery::new());
        }
        g.assume_init_mut()
    }
}

impl Gallery {
    pub const fn new() -> Self {
        Gallery {
            mode: Mode::Grid,
            dlg: Dlg::Delete,
            st: St::Listing,
            busy: None,
            v: None,
            need: 0,
            paint: 0,
            tpaint: 0,
            todo: 0,
            total: 0,
            free: 0,
            page: 0,
            ents: [ENT0; PER_PAGE],
            n: 0,
            rank: 0,
            cur: ENT0,
            overlay: false,
            full: false,
            vmsg: None,
            band: None,
            job: JOB0,
        }
    }

    /// Enter the gallery: first page, catalog re-read. The app then calls
    /// `draw` (its full redraw) and `poll` every pass.
    #[inline(never)] // .cold (link-native.x): not inlined into the hot caller
    pub fn open(&mut self) {
        *self = Gallery::new();
        self.need = N_LIST | N_PAGE;
    }

    /// Full repaint of the current screen (the app's redraw); images and
    /// thumbnails follow from `poll`.
    pub fn draw(&mut self) {
        self.paint |= P_ALL;
        self.do_paint();
    }

    /// Nothing left to do (tests: poll until idle).
    #[allow(dead_code)]
    pub fn idle(&self) -> bool {
        self.need == 0 && self.paint == 0 && self.tpaint == 0 && self.todo == 0 && self.job.kind == JK::None && self.band.is_none()
    }

    /// MENU key: close the dialog / the viewer, or leave.
    #[inline(never)] // .cold (link-native.x): not inlined into the hot caller
    pub fn back(&mut self) -> Out {
        match self.mode {
            Mode::Dialog if self.dlg != Dlg::Deleting => self.close_dialog(),
            Mode::Dialog => {}
            Mode::View => self.to_grid(),
            Mode::Grid => return Out::Exit,
        }
        Out::Stay
    }

    /// A gesture (taps and swipes; the rest is ignored).
    #[inline(never)] // .cold (link-native.x): not inlined into the hot caller
    pub fn touch(&mut self, g: Gesture) -> Out {
        match (self.mode, g) {
            (Mode::Dialog, Gesture::Tap(p)) => match self.dlg {
                Dlg::Delete => match crate::dialog::power_off_hit(p.x, p.y) {
                    Some(true) => {
                        self.dlg = Dlg::Deleting;
                        self.need |= N_DEL;
                    }
                    Some(false) => self.close_dialog(),
                    None => {}
                },
                Dlg::Info(..) if inside(OK_BTN, p.x, p.y) => self.close_dialog(),
                _ => {}
            },
            (Mode::Grid, Gesture::Tap(p)) => {
                if (p.y as i16) < BAR_H {
                    return Out::Exit;
                }
                if self.st != St::Ok || self.total == 0 {
                    return Out::Stay;
                }
                if (p.y as i16) >= NAV_Y - 4 {
                    if (p.x as i16) < PREV_X + NAV_W + 10 {
                        self.turn(-1);
                    } else if (p.x as i16) >= NEXT_X - 10 {
                        self.turn(1);
                    }
                    return Out::Stay;
                }
                for i in 0..self.n {
                    let (x, y) = tile_xy(i);
                    if (x..x + TILE_W).contains(&(p.x as i16)) && (y..y + TILE_H).contains(&(p.y as i16)) {
                        self.view(self.page * PER_PAGE as u32 + i as u32, self.ents[i], false);
                        break;
                    }
                }
            }
            (Mode::Grid, Gesture::SwipeLeft) => self.turn(1),
            (Mode::Grid, Gesture::SwipeRight) => self.turn(-1),
            (Mode::View, Gesture::Tap(p)) => {
                let bar = self.overlay || self.cur.kind == Kind::Csv;
                if bar && (p.y as i16) < BAR_H {
                    if p.x < BACK_HIT_X {
                        self.to_grid();
                    } else if p.x >= DEL_HIT_X {
                        self.mode = Mode::Dialog;
                        self.dlg = Dlg::Delete;
                        self.stop_view();
                        self.paint |= P_DLG;
                    } else {
                        self.toggle_overlay();
                    }
                } else {
                    self.toggle_overlay();
                }
            }
            (Mode::View, Gesture::SwipeLeft) if self.rank + 1 < self.total => self.step_file(1),
            (Mode::View, Gesture::SwipeRight) if self.rank > 0 => self.step_file(-1),
            _ => {}
        }
        Out::Stay
    }

    /// One bounded slice (main loop, every pass while the gallery is shown
    /// and drawing is allowed).
    #[inline(never)] // .cold (link-native.x): only while the gallery is shown
    pub fn poll(&mut self) {
        self.poll_on(disk());
    }

    /// `poll` on any block device (host tests).
    pub fn poll_on(&mut self, disk: &mut dyn BlockDevice) {
        if crate::screenshot::frozen() {
            return;
        }
        self.do_paint();
        let b = busy_reason();
        if b != self.busy {
            if b.is_some() && self.busy.is_none() {
                // The disk will have changed when it comes back.
                // (A pending file load re-reads its entry anyway.)
                self.need |= N_LIST
                    | match self.mode {
                        Mode::Grid => N_PAGE,
                        _ if self.need & N_CUR != 0 => 0,
                        _ => N_FIND,
                    };
            }
            self.busy = b;
            self.on_busy_change();
            self.do_paint();
        }
        if b.is_some() {
            return;
        }
        if self.need & N_DEL != 0 {
            self.need &= !N_DEL;
            self.delete(disk);
        } else if self.need & N_LIST != 0 {
            self.need &= !N_LIST;
            self.catalog(disk);
        } else if let Some(v) = self.v {
            let mut io = Io::new(disk, v);
            if self.need & N_PAGE != 0 {
                self.need &= !N_PAGE;
                self.page_scan(&mut io);
            } else if self.need & N_FIND != 0 {
                self.need &= !N_FIND;
                self.refind(&mut io);
            } else if self.need & N_CUR != 0 {
                self.need &= !N_CUR;
                self.load_cur(&mut io);
            } else {
                self.work(&mut io);
            }
        } else {
            self.need = 0;
        }
        self.do_paint();
    }

    // ----------------------------------------------------------- navigation

    fn pages(&self) -> u32 {
        self.total.div_ceil(PER_PAGE as u32).max(1)
    }

    fn turn(&mut self, d: i32) {
        let p = self.page as i32 + d;
        if p < 0 || p as u32 >= self.pages() {
            return;
        }
        self.page = p as u32;
        self.job = JOB0;
        self.todo = 0;
        self.need |= N_PAGE;
    }

    fn view(&mut self, rank: u32, e: Ent, overlay: bool) {
        self.mode = Mode::View;
        self.rank = rank;
        self.cur = e;
        self.overlay = overlay || e.kind == Kind::Csv;
        self.vmsg = None;
        self.band = None;
        self.full = false;
        self.todo = 0;
        self.tpaint = 0;
        self.job = JOB0;
        self.paint |= P_ALL;
    }

    fn step_file(&mut self, d: i32) {
        self.rank = (self.rank as i32 + d) as u32;
        self.stop_view();
        self.vmsg = None;
        self.need |= N_CUR;
    }

    fn to_grid(&mut self) {
        self.mode = Mode::Grid;
        self.page = (self.rank / PER_PAGE as u32).min(self.pages() - 1);
        self.job = JOB0;
        self.band = None;
        self.n = 0;
        self.paint |= P_ALL;
        self.need |= N_PAGE;
    }

    fn stop_view(&mut self) {
        if matches!(self.job.kind, JK::View | JK::Chart) {
            let keep = self.job.stats;
            self.job.kind = JK::None;
            self.job.stats = keep;
        }
        self.band = None;
    }

    fn close_dialog(&mut self) {
        self.mode = Mode::View;
        if self.cur.kind == Kind::Bmp && self.full && self.vmsg.is_none() {
            let b = crate::dialog::BOX;
            self.band = Some((b.1 as i16, (b.1 + b.3) as i16));
        } else {
            self.paint |= P_ALL;
        }
    }

    fn toggle_overlay(&mut self) {
        if self.cur.kind == Kind::Csv {
            return;
        }
        self.overlay = !self.overlay;
        if self.vmsg.is_some() {
            self.paint |= P_ALL;
        } else if self.overlay {
            if self.job.kind == JK::View {
                self.job.sink.y_lo = self.job.sink.y_lo.max(BAR_H);
            }
            self.paint |= P_BAR;
        } else {
            if self.job.kind == JK::View {
                self.job.sink.y_lo = 0;
            }
            self.band = Some((0, BAR_H));
        }
    }

    fn on_busy_change(&mut self) {
        match self.mode {
            Mode::Grid if self.st == St::Ok && self.total > 0 => self.paint |= P_NAV,
            Mode::Grid => self.paint |= P_ALL,
            Mode::View if self.need & N_CUR != 0 => self.paint |= P_MSG,
            Mode::View if self.job.kind == JK::View && self.job.ph == Ph::Head => self.paint |= P_MSG,
            Mode::View if self.job.kind == JK::Chart && self.job.ph == Ph::Head && !self.job.stats => self.paint |= P_MSG,
            _ => {}
        }
    }

    // ----------------------------------------------------------- disk work

    fn catalog(&mut self, disk: &mut dyn BlockDevice) {
        crate::wdt::kick_if_feeding();
        let r = csvlog::list_on(disk, &mut []);
        let st = match r {
            Ok(c) => {
                let buf = &mut diskbuf::blocks()[0];
                self.v = if disk.read_block(0, &mut buf[..]) { parse_bpb(&buf[..], disk.block_count()) } else { None };
                // The count of what the gallery shows (S*.BMP, LOG*.DLG: the
                // directories `scan` reads): csvlog's catalog also counts
                // the SCOPE/ and FAST*.DLG files, which have no tile here.
                let mut total = 0u32;
                let ok = match self.v {
                    Some(v) => crate::fatdir::for_each(disk, &v, &[&crate::fatdir::LOGS, &crate::fatdir::SHOTS], buf, &mut |_, _, e| {
                        total += kind_of(e).is_some() as u32;
                        true
                    })
                    .is_ok(),
                    None => false,
                };
                if total != self.total || c.free_bytes != self.free {
                    self.paint |= P_INFO | P_NAV;
                }
                self.total = total;
                self.free = c.free_bytes;
                if ok { St::Ok } else { St::Error }
            }
            Err(GErr::Busy) => {
                self.need |= N_LIST;
                return;
            }
            Err(GErr::NoDisk) => St::NoDisk,
            Err(_) => St::Error,
        };
        if st != self.st {
            self.st = st;
            if self.mode == Mode::Grid {
                self.paint |= P_ALL;
            }
        }
        if st != St::Ok {
            self.need = 0;
        }
    }

    /// Directory index range of page `p` (reverse order: rank r is index
    /// total - 1 - r).
    fn page_range(total: u32, p: u32) -> (u32, u32) {
        let hi = total.saturating_sub(p * PER_PAGE as u32);
        (hi.saturating_sub(PER_PAGE as u32), hi)
    }

    fn page_scan(&mut self, io: &mut Io) {
        let mut tmp = [ENT0; PER_PAGE];
        let mut got = None;
        for _ in 0..2 {
            self.page = self.page.min(self.pages() - 1);
            let (lo, hi) = Self::page_range(self.total, self.page);
            match scan(io, lo, hi, &mut tmp, None) {
                None => {
                    self.st = St::Error;
                    self.paint |= P_ALL;
                    return;
                }
                Some((t, n, _)) if t == self.total => {
                    got = Some(n);
                    break;
                }
                Some((t, _, _)) => {
                    self.total = t;
                    self.paint |= P_INFO | P_NAV;
                }
            }
        }
        let Some(n) = got else {
            self.need |= N_PAGE;
            return;
        };
        let mut ents = [ENT0; PER_PAGE];
        for i in 0..n {
            ents[i] = tmp[n - 1 - i];
        }
        if self.mode != Mode::Grid {
            return;
        }
        for i in 0..PER_PAGE {
            let was = i < self.n;
            let is = i < n;
            if was != is || (is && ents[i] != self.ents[i]) {
                self.tpaint |= 1 << i;
                if self.job.kind == JK::Tile(i as u8) {
                    self.job = JOB0;
                }
                self.todo &= !(1 << i);
            }
        }
        self.ents = ents;
        self.n = n;
        self.paint |= P_NAV;
    }

    /// After the disk was busy: the viewed file, by name.
    fn refind(&mut self, io: &mut Io) {
        if self.mode == Mode::Grid || self.need & N_CUR != 0 {
            return;
        }
        match scan(io, 0, 0, &mut [], Some(&self.cur.name)) {
            Some((t, _, Some((idx, e)))) => {
                let rank = t - 1 - idx;
                if t != self.total || rank != self.rank {
                    self.total = t;
                    self.rank = rank;
                    if self.overlay || self.cur.kind == Kind::Csv {
                        self.paint |= P_BAR;
                    }
                }
                if e.start != self.cur.start || (e.size != self.cur.size && e.kind == Kind::Bmp) {
                    // Rewritten under us (a host): show it again.
                    self.cur = e;
                    self.stop_view();
                    self.job.stats = false;
                    self.paint |= P_ALL;
                }
            }
            Some((t, _, None)) => {
                self.total = t;
                self.to_grid();
            }
            None => {}
        }
    }

    /// Viewer: the file of `rank`.
    fn load_cur(&mut self, io: &mut Io) {
        let mut e = [ENT0; 1];
        let d = self.total.saturating_sub(1 + self.rank);
        match scan(io, d, d + 1, &mut e, None) {
            Some((t, 1, _)) if t == self.total => {
                let (r, o) = (self.rank, self.overlay && self.cur.kind == Kind::Bmp);
                self.view(r, e[0], o);
            }
            Some((t, _, _)) => {
                self.total = t;
                if t == 0 {
                    self.to_grid();
                } else {
                    self.rank = self.rank.min(t - 1);
                    self.need |= N_CUR;
                }
            }
            None => {
                self.vmsg = Some(("Read error", 0, 0));
                self.paint |= P_ALL;
            }
        }
    }

    fn delete(&mut self, disk: &mut dyn BlockDevice) {
        if self.mode != Mode::Dialog || self.dlg != Dlg::Deleting {
            return;
        }
        crate::wdt::kick_if_feeding();
        let r = csvlog::delete_on(disk, &self.cur.name);
        crate::wdt::kick_if_feeding();
        let r = match r {
            Ok(()) => Ok(()),
            // The file being recorded (csvlog::delete_on): retrying cannot help.
            Err(GErr::Busy) if csvlog::recording_name() == Some(self.cur.name) => Err(("Cannot delete", "Stop the recording")),
            Err(GErr::Busy) => Err(("Cannot delete", "Disk busy, try again")),
            Err(GErr::NotFound) => Err(("Cannot delete", "File not found")),
            Err(_) => Err(("Cannot delete", "Delete failed")),
        };
        match r {
            Ok(()) => {
                self.total = self.total.saturating_sub(1);
                self.job.stats = false;
                self.mode = Mode::View;
                self.need |= N_LIST;
                if self.total == 0 {
                    self.to_grid();
                } else {
                    self.rank = self.rank.min(self.total - 1);
                    self.vmsg = None;
                    self.full = false;
                    self.need |= N_CUR;
                    // Blank now; the next file follows.
                    fill((0, 0, 480, 320), IMG_BG);
                }
            }
            Err((t, m)) => {
                self.dlg = Dlg::Info(t, m);
                self.paint |= P_DLG;
                if m == "File not found" {
                    self.need |= N_LIST | N_FIND;
                }
            }
        }
    }

    /// Thumbnails, the viewer image or chart: one block.
    fn work(&mut self, io: &mut Io) {
        if self.job.kind == JK::None {
            match self.mode {
                Mode::Grid => {
                    if self.todo == 0 {
                        return;
                    }
                    let i = self.todo.trailing_zeros() as usize;
                    self.start_job(JK::Tile(i as u8), self.ents[i]);
                }
                Mode::View if self.vmsg.is_none() => {
                    if let Some((a, b)) = self.band.take() {
                        if self.cur.kind == Kind::Bmp {
                            self.start_job(JK::View, self.cur);
                            self.job.sink.y_lo = a;
                            self.job.sink.y_hi = b;
                        } else {
                            self.paint |= P_ALL;
                            self.do_paint();
                        }
                    } else {
                        return;
                    }
                }
                _ => return,
            }
        }
        self.step(io);
    }

    fn start_job(&mut self, kind: JK, e: Ent) {
        let (stats, csv) = (self.job.stats && self.job.ent == e, self.job.csv);
        self.job = JOB0;
        self.job.kind = kind;
        self.job.ent = e;
        self.job.chain = Chain::new(e.start);
        if stats {
            self.job.csv = csv;
            self.job.stats = true;
        }
        if kind == JK::View {
            self.job.sink.y_lo = if self.overlay { BAR_H } else { 0 };
            self.job.sink.y_hi = 320;
        }
    }

    fn finish(&mut self) {
        match self.job.kind {
            JK::Tile(i) => self.todo &= !(1 << i),
            JK::View => self.full = self.job.sink.y_lo <= BAR_H && self.job.sink.y_hi >= 320 || self.full,
            _ => {}
        }
        let keep = self.job.stats;
        self.job.kind = JK::None;
        self.job.stats = keep;
    }

    /// The current job failed: a message in its place.
    fn fail(&mut self, m: &'static str, bpp: u16, comp: u16) {
        match self.job.kind {
            JK::Tile(i) => {
                let (x, y) = tile_xy(i as usize);
                let p = rect(x + 4, y + 4, TH_W as i16, TH_H as i16);
                sf::rbox(p, 4, PANEL, M_TILE);
                let (a, b) = if m == "Unsupported format" { ("Unsupported", "format") } else { (m, "") };
                small(rect(p.x, p.y + 30, p.w, 14), a, FG, PANEL, 8);
                if !b.is_empty() {
                    small(rect(p.x, p.y + 50, p.w, 14), b, FG, PANEL, 8);
                }
            }
            JK::View | JK::Chart => {
                self.vmsg = Some((m, bpp, comp));
                self.paint |= P_ALL;
            }
            JK::None => {}
        }
        self.finish();
    }

    fn step(&mut self, io: &mut Io) {
        let e = self.job.ent;
        let size = e.size;
        match (e.kind, self.job.ph) {
            (Kind::Bmp, Ph::Head) => {
                if size == 0 || !read_fb(io, &mut self.job.chain, 0) {
                    return self.fail(if size == 0 { "Empty file" } else { "Read error" }, 0, 0);
                }
                let b = match Bmp::parse(&io.data[..], size) {
                    Ok(b) => b,
                    Err((bpp, comp)) => return self.fail("Unsupported format", bpp, comp),
                };
                let j = &mut self.job;
                j.bmp = b;
                match j.kind {
                    JK::Tile(i) => {
                        let (x, y) = tile_xy(i as usize);
                        j.sink = Sink::fit(b.w, b.h, b.up, x + 4, y + 4, TH_W, TH_H);
                        j.sink.key = ((y as u32) << 16) | x as u32;
                        probe_begin(j.sink.key, M_TILE);
                        fill((x + 4, y + 4, TH_W as i16, TH_H as i16), IMG_BG);
                        probe_end();
                    }
                    _ => {
                        let (lo, hi) = (j.sink.y_lo, j.sink.y_hi);
                        j.sink = Sink::fit(b.w, b.h, b.up, 0, 0, 480, 320);
                        j.sink.y_lo = lo;
                        j.sink.y_hi = hi;
                        // Rows of the band outside a smaller image.
                        let s = j.sink;
                        if s.dw < 480 || s.dh < 320 {
                            fill((0, lo, 480, hi - lo), IMG_BG);
                        }
                    }
                }
                let Some((k0, k1)) = j.sink.rows() else {
                    return self.finish();
                };
                if b.fmt == Fmt::Rle4 {
                    (j.i, j.end, j.desc) = (b.off / BLK, (size - 1) / BLK, false);
                } else {
                    let o0 = b.off + k0 * b.row;
                    let o1 = (b.off + (k1 + 1) * b.row).min(size) - 1;
                    // Screen top first: stored rows bottom-up = blocks descending.
                    j.desc = j.kind == JK::View && b.up;
                    (j.i, j.end) = if j.desc { (o1 / BLK, o0 / BLK) } else { (o0 / BLK, o1 / BLK) };
                }
                j.ph = Ph::Pix;
            }
            (Kind::Bmp, _) => {
                let fb = self.job.i;
                if !read_fb(io, &mut self.job.chain, fb) {
                    return self.fail("Read error", 0, 0);
                }
                let len = (size - fb * BLK).min(BLK) as usize;
                self.job.pix_block(&io.data[..len], fb);
                let j = &mut self.job;
                let done = j.rle.done || fb == j.end;
                if done {
                    self.finish();
                } else if j.desc {
                    j.i -= 1;
                } else {
                    j.i += 1;
                }
            }
            (Kind::Csv, ph) => self.step_dlg(io, ph),
        }
    }

    /// A compact binary log (LOGnnn.DLG, dlog.rs): the header text (function,
    /// range) and unit at Head, then one chunk per pass (pass A statistics,
    /// pass B plot; every row, exact counts). `j.i` = file offset of the next
    /// chunk; `csv.hdr` = the first chunk's. A torn tail ends the pass.
    fn step_dlg(&mut self, io: &mut Io, ph: Ph) {
        let e = self.job.ent;
        let j = &mut self.job;
        let mut src = GSrc { io, chain: &mut j.chain, size: e.size };
        if ph == Ph::Head {
            if j.stats {
                return self.begin_b();
            }
            j.csv = CSV0;
            let Some((kind, hl, ul, unit)) = dlog::parse_file(&mut src, e.size) else {
                return self.fail("Unsupported format", 0, 0);
            };
            // The header text's first line: function, range.
            let mut t = [0u8; 160];
            let n = (hl as usize).min(t.len());
            if !src.read(dlog::FHDR as u32, &mut t[..n]) {
                return self.fail("Read error", 0, 0);
            }
            j.csv.header(&t[..n]);
            let k = ul.min(j.csv.unit.len());
            j.csv.unit[..k].copy_from_slice(&unit[..k]);
            j.csv.ul = k as u8;
            j.csv.hdr = dlog::FHDR as u32 + hl;
            if kind == dlog::KIND_FAST {
                // Its parameters follow the header text; values in SI (AD2)
                // or raw counts (AD1), rows at device time.
                let mut p = [0u8; dlog::FAST_PAR];
                let Some(fp) = src.read(j.csv.hdr, &mut p).then(|| dlog::FPar::parse(&p)).flatten() else {
                    return self.fail("Unsupported format", 0, 0);
                };
                j.csv.fast = Some(fp);
                j.csv.hdr += dlog::FAST_PAR as u32;
            }
            j.i = j.csv.hdr;
            j.ph = Ph::A;
            return;
        }
        if j.i == 0 {
            j.i = j.csv.hdr; // pass B starts at the first chunk
        }
        let mut w = dlog::Win::new();
        if let Some(fp) = j.csv.fast {
            let c = dlog::fchunk_at(&mut src, &mut w, j.i, e.size, true);
            let ok = c.is_some_and(|c| {
                let csv = &mut j.csv;
                dlog::fast_rows(&mut src, &mut w, &c, &fp, &mut |t, raw, _| {
                    let v = if fp.flags & dlog::FP_SCALED != 0 { fp.a + fp.b * raw as f32 } else { raw as f32 };
                    csv.sample((t / 1000).min(u32::MAX as u64) as u32, Some(v))
                })
            });
            match c {
                Some(c) if ok => j.i = c.end(),
                _ => {
                    if ph == Ph::A {
                        self.end_a();
                    } else {
                        self.job.csv.flush();
                        self.finish();
                    }
                }
            }
            return;
        }
        let c = dlog::chunk_at(&mut src, &mut w, j.i, e.size, true);
        let ok = c.is_some_and(|c| {
            let csv = &mut j.csv;
            dlog::rows(&mut src, &mut w, &c, &mut |t, v, fl| {
                csv.sample(t, if fl & dlog::F_OL != 0 { None } else { v.to_f32() })
            })
        });
        match c {
            Some(c) if ok => j.i = c.end(),
            _ => {
                if ph == Ph::A {
                    self.end_a();
                } else {
                    self.job.csv.flush();
                    self.finish();
                }
            }
        }
    }

    fn end_a(&mut self) {
        self.job.stats = true;
        self.begin_b();
    }

    /// Pass B: the panel / page texts, then the trace.
    fn begin_b(&mut self) {
        let j = &mut self.job;
        let c = j.csv;
        // The recording in progress: only its committed part is read.
        let none = if csvlog::recording_name() == Some(j.ent.name) { "Recording" } else { "No data" };
        let mut func = Tb::new();
        func.bytes(if c.fl > 0 { &c.func[..c.fl as usize] } else { b"Log" });
        let mut rows = Tb::new();
        let n = c.rows;
        rows.u(n);
        rows.s(if n == 1 { " row" } else { " rows" });
        let mut dur = Tb::new();
        if c.rows > 0 {
            dur_txt(&mut dur, c.t1.saturating_sub(c.t0));
        }
        let unit = &c.unit[..c.ul as usize];
        match j.kind {
            JK::Tile(i) => {
                let (x, y) = tile_xy(i as usize);
                let (px, py) = (x + 4, y + 4);
                sf::rbox(rect(px, py, TH_W as i16, TH_H as i16), 4, PANEL, M_TILE);
                small(rect(px + 4, py + 3, 132, 14), func.str(), FG, PANEL, 1);
                small(rect(px + 4, py + 3, 132, 14), dur.str(), FG, PANEL, 4);
                if c.vals == 0 {
                    small(rect(px, py + 38, TH_W as i16, 14), none, DIM, PANEL, 8);
                    small(rect(px + 4, py + 70, 132, 14), rows.str(), DIM, PANEL, 1);
                    return self.finish();
                }
                small(rect(px + 4, py + 70, 132, 14), rows.str(), DIM, PANEL, 1);
                let key = (((py + 20) as u32) << 16) | (px + 4) as u32;
                j.csv.begin_plot((px + 4, py + 20, 132, 46), key);
            }
            _ => {
                title_bar_view(self.rank, self.total, &self.cur);
                fill((0, BAR_H, 480, 320 - BAR_H), M_BG);
                let mut t = Tb::new();
                t.bytes(func.str().as_bytes());
                if c.rl > 0 {
                    t.s(", range ");
                    t.bytes(&c.range[..c.rl as usize]);
                }
                small(rect(8, 44, 464, 16), t.str(), FG, M_BG, 1);
                let mut r = Tb::new();
                r.bytes(rows.str().as_bytes());
                if c.rows > 0 {
                    r.s(", ");
                    r.bytes(dur.str().as_bytes());
                }
                small(rect(8, 44, 464, 16), r.str(), FG, M_BG, 4);
                for (k, (lab, v)) in [("Min ", c.min), ("Avg ", c.avg()), ("Max ", c.max)].iter().enumerate() {
                    let mut s = Tb::new();
                    s.s(lab);
                    val_txt(&mut s, *v, unit, true);
                    small(rect(8, 62, 464, 16), s.str(), ACCENT, M_BG, [1, 6, 3][k]);
                }
                sf::rbox(rect(CHART.0, CHART.1, CHART.2, CHART.3), 6, PANEL, M_BG);
                let (px, py, pw, ph) = PLOT;
                if c.vals == 0 {
                    sf::glyphs(sf::TEXT22, rect(px, py, pw, ph), none, DIM, PANEL, 8, 0);
                    return self.finish();
                }
                for k in 0..3 {
                    let gy = py + (ph - 1) * k / 2;
                    fill((px, gy, pw, 1), GRID);
                    let v = c.max - (c.max - c.min) * k as f32 / 2.0;
                    let mut s = Tb::new();
                    val_txt(&mut s, v, unit, false);
                    small(rect(14, gy - 7, 64, 14), s.str(), DIM, PANEL, 4);
                }
                small(rect(px, py + ph + 6, 120, 14), "0 s", DIM, PANEL, 0);
                small(rect(px + pw - 120, py + ph + 6, 120, 14), dur.str(), DIM, PANEL, 3);
                j.csv.begin_plot(PLOT, ((py as u32) << 16) | px as u32);
            }
        }
        let j = &mut self.job;
        j.i = 0;
        j.ph = Ph::B;
    }

    // ----------------------------------------------------------- painting

    fn do_paint(&mut self) {
        if self.paint & P_ALL != 0 {
            self.paint = 0;
            self.paint_all();
            return;
        }
        if self.paint & P_INFO != 0 && self.mode == Mode::Grid {
            self.grid_info();
        }
        if self.paint & P_NAV != 0 && self.mode == Mode::Grid && self.st == St::Ok {
            if self.total == 0 {
                self.paint_all();
                self.paint = 0;
                return;
            }
            self.nav();
        }
        if self.paint & P_BAR != 0 && (self.mode == Mode::View) && (self.overlay || self.cur.kind == Kind::Csv) {
            title_bar_view(self.rank, self.total, &self.cur);
        }
        if self.paint & P_MSG != 0 && self.mode == Mode::View {
            self.view_msg();
        }
        if self.paint & P_DLG != 0 && self.mode == Mode::Dialog {
            self.paint_dialog();
        }
        self.paint = 0;
        if self.mode == Mode::Grid && self.st == St::Ok {
            for i in 0..PER_PAGE {
                if self.tpaint & (1 << i) != 0 {
                    self.tile(i);
                }
            }
        }
        self.tpaint = 0;
    }

    fn paint_all(&mut self) {
        self.tpaint = 0;
        match self.mode {
            Mode::Grid => {
                title_bar();
                sf::glyphs(sf::TEXT22, rect(0, 0, 480, BAR_H), "Gallery", FG, BAR, 8, 0);
                fill((0, BAR_H, 480, 320 - BAR_H), M_BG);
                self.job = JOB0;
                self.todo = 0;
                match self.st {
                    St::Listing => {
                        let (m, d) = match self.busy {
                            Some(b) => ("Disk busy", busy_text(b)),
                            None => ("Reading the disk", ""),
                        };
                        message(M_BG, m, d);
                    }
                    St::NoDisk => message(M_BG, "No disk", "The SPI flash disk is missing or not FAT16"),
                    St::Error => message(M_BG, "Disk read error", "Leave and open the gallery again to retry"),
                    St::Ok => {
                        self.grid_info();
                        if self.total == 0 {
                            message(M_BG, "No screenshots or logs", "Swipe down with 3 fingers to save a screenshot");
                        } else {
                            for i in 0..self.n {
                                self.tile(i);
                            }
                            self.nav();
                        }
                    }
                }
            }
            Mode::View | Mode::Dialog => {
                if self.cur.kind == Kind::Csv {
                    title_bar_view(self.rank, self.total, &self.cur);
                    fill((0, BAR_H, 480, 320 - BAR_H), M_BG);
                    if self.mode == Mode::View && self.busy.is_none() && self.vmsg.is_none() {
                        // Pass A of a long log takes up to ~1 s.
                        message(M_BG, "Reading the log", "");
                    }
                } else {
                    fill((0, 0, 480, 320), IMG_BG);
                    if self.overlay {
                        title_bar_view(self.rank, self.total, &self.cur);
                    }
                }
                self.full = false;
                if self.mode == Mode::Dialog {
                    self.stop_view();
                    self.paint_dialog();
                } else if self.vmsg.is_some() || self.busy.is_some() {
                    self.view_msg();
                    if self.vmsg.is_none() {
                        self.restart_view();
                    }
                } else {
                    self.restart_view();
                }
            }
        }
    }

    fn restart_view(&mut self) {
        self.band = None;
        let k = if self.cur.kind == Kind::Csv { JK::Chart } else { JK::View };
        self.start_job(k, self.cur);
    }

    fn view_msg(&mut self) {
        let (bg, y0) = if self.cur.kind == Kind::Csv { (M_BG, BAR_H) } else { (IMG_BG, if self.overlay { BAR_H } else { 0 }) };
        if self.need & N_CUR != 0 {
            // Switching files: the old one goes.
            fill((0, y0, 480, 320 - y0), bg);
        } else {
            fill((0, y0.max(100), 480, 100), bg);
        }
        match (self.vmsg, self.busy) {
            (Some((m, bpp, comp)), _) => {
                let mut t = Tb::new();
                if bpp != 0 {
                    t.s("BMP with ");
                    t.u(bpp as u32);
                    t.s(" bits per pixel, compression ");
                    t.u(comp as u32);
                }
                message(bg, m, t.str());
            }
            (None, Some(b)) => message(bg, "Disk busy", busy_text(b)),
            _ => {}
        }
    }

    fn grid_info(&self) {
        fill((340, 2, 136, 36), BAR);
        if self.st != St::Ok {
            return;
        }
        let mut a = Tb::new();
        a.u(self.total);
        a.s(if self.total == 1 { " file" } else { " files" });
        let mut b = Tb::new();
        size_txt(&mut b, self.free);
        b.s(" free");
        small(rect(352, 5, 120, 15), a.str(), FG, BAR, 4);
        small(rect(352, 20, 120, 15), b.str(), DIM, BAR, 4);
    }

    fn nav(&self) {
        fill((0, NAV_Y - 2, 480, NAV_H + 4), M_BG);
        let pages = self.pages();
        for (x, txt, on) in [(PREV_X, "< Prev", self.page > 0), (NEXT_X, "Next >", self.page + 1 < pages)] {
            sf::rbox(rect(x, NAV_Y, NAV_W, NAV_H), 6, M_TILE, M_BG);
            sf::glyphs(sf::TEXT22, rect(x, NAV_Y, NAV_W, NAV_H), txt, if on { FG } else { DIM }, M_TILE, 8, 0);
        }
        let mut t = Tb::new();
        match self.busy {
            Some(_) => t.s("Disk busy"),
            None => {
                t.s("Page ");
                t.u(self.page + 1);
                t.s(" of ");
                t.u(pages);
            }
        }
        small(rect(PREV_X + NAV_W + 6, NAV_Y, NEXT_X - PREV_X - NAV_W - 12, NAV_H), t.str(), FG, M_BG, 8);
    }

    /// Static part of tile `i` (frame, placeholder, name and size); its
    /// thumbnail is queued.
    fn tile(&mut self, i: usize) {
        let (x, y) = tile_xy(i);
        if i >= self.n {
            fill((x, y, TILE_W, TILE_H), M_BG);
            return;
        }
        let e = self.ents[i];
        sf::rbox(rect(x, y, TILE_W, TILE_H), 6, M_TILE, M_BG);
        probe_begin(((y as u32) << 16) | x as u32, M_TILE);
        fill((x + 4, y + 4, TH_W as i16, TH_H as i16), if e.kind == Kind::Csv { PANEL } else { IMG_BG });
        probe_end();
        let name = name_txt(&e.name);
        let mut sz = Tb::new();
        size_txt(&mut sz, e.size);
        let (wn, ws) = (small_width(name.str()), small_width(sz.str()));
        let r = rect(x + 5, y + 99, 138, 14);
        small(r, name.str(), FG, M_TILE, 1);
        if wn + 6 + ws <= r.w {
            small(r, sz.str(), DIM, M_TILE, 4);
        }
        self.todo |= 1 << i;
        if self.job.kind == JK::Tile(i as u8) {
            self.job = JOB0;
        }
    }

    fn paint_dialog(&mut self) {
        match self.dlg {
            Dlg::Delete | Dlg::Deleting => {
                let t = if self.cur.kind == Kind::Csv { "Delete this log?" } else { "Delete screenshot?" };
                dialog_box(t, name_txt(&self.cur.name).str(), Some("Delete"))
            }
            Dlg::Info(t, m) => dialog_box(t, m, None),
        }
    }

    // ----------------------------------------------------------- tests

    /// (rows, values, min, max, avg, span ms) of the
    /// last log job.
    #[cfg(test)]
    pub fn csv_stats(&self) -> (u32, u32, f32, f32, f32, u32) {
        let c = &self.job.csv;
        (c.rows, c.vals, c.min, c.max, c.avg(), c.t1.saturating_sub(c.t0))
    }

    /// Open the viewer on rank `r` (tests).
    #[cfg(test)]
    pub fn test_view(&mut self, r: u32) {
        self.mode = Mode::View;
        self.rank = r;
        self.need |= N_CUR;
    }

    /// Show an info box (tests: every message's layout).
    #[cfg(test)]
    pub fn test_info(&mut self, t: &'static str, m: &'static str) {
        self.mode = Mode::Dialog;
        self.dlg = Dlg::Info(t, m);
    }

    /// "grid" / "view" / "dialog" (tests).
    #[cfg(test)]
    pub fn test_mode(&self) -> &'static str {
        match self.mode {
            Mode::Grid => "grid",
            Mode::View => "view",
            Mode::Dialog => "dialog",
        }
    }

    /// The info box's message and the current file's 8.3 name (tests).
    #[cfg(test)]
    pub fn test_info_cur(&self) -> (Option<&'static str>, [u8; 11]) {
        (if let Dlg::Info(_, m) = self.dlg { Some(m) } else { None }, self.cur.name)
    }

    #[cfg(test)]
    pub fn test_state(&self) -> (u32, u32, u32, usize) {
        (self.total, self.page, self.rank, self.n)
    }
}

/// Viewer bar: back, name and index, Delete.
fn title_bar_view(rank: u32, total: u32, e: &Ent) {
    title_bar();
    let name = name_txt(&e.name);
    let mut idx = Tb::new();
    idx.u(rank + 1);
    idx.s(" / ");
    idx.u(total);
    let avail = NAME_X1 - NAME_X0;
    let mut font = sf::ASCII26;
    let mut wn = sf::text_width(font, name.str(), 1);
    let wi = small_width(idx.str());
    if wn > avail {
        font = sf::F14;
        wn = sf::text_width(font, name.str(), 1);
    }
    let show_idx = wn + 10 + wi <= avail;
    let gw = if show_idx { wn + 10 + wi } else { wn };
    let gx = NAME_X0 + (avail - gw) / 2;
    sf::glyphs(font, rect(gx, 0, wn + 2, BAR_H), name.str(), FG, BAR, 1, 1);
    if show_idx {
        small(rect(gx + wn + 10, 0, wi + 2, BAR_H), idx.str(), DIM, BAR, 1);
    }
    sf::rbox(rect(DEL.0, DEL.1, DEL.2, DEL.3), 6, D_RED, BAR);
    sf::glyphs(sf::TEXT22, rect(DEL.0, DEL.1, DEL.2, DEL.3), "Delete", FG, D_RED, 8, 0);
}

impl Job {
    /// Draw the sampled pixels stored in file block `fb` (`d`).
    fn pix_block(&mut self, d: &[u8], fb: u32) {
        let b = self.bmp;
        let s = &mut self.sink;
        s.reset_run();
        probe_begin(s.key, M_TILE);
        let b0 = fb * BLK;
        let b1 = b0 + d.len() as u32;
        if b.fmt == Fmt::Rle4 {
            let rle = &mut self.rle;
            let mut out = |x: u32, k: u32, i: u8| {
                if x < b.w {
                    if let Some(dy) = s.row(k) {
                        if let Some(dx) = s.col(x) {
                            s.put(dx, dy, b.pal[i as usize]);
                        }
                    }
                }
            };
            for q in b0.max(b.off)..b1 {
                if rle.done {
                    break;
                }
                rle.feed(d[(q - b0) as usize], &mut out);
            }
            probe_end();
            return;
        }
        if b1 <= b.off {
            probe_end();
            return;
        }
        let lo = b0.max(b.off) - b.off;
        let hi = b1 - b.off;
        let k0 = lo / b.row;
        let k1 = ((hi - 1) / b.row).min(b.h - 1);
        for k in k0..=k1 {
            let Some(dy) = s.row(k) else { continue };
            let rs = b.off + k * b.row;
            for dx in 0..s.dw {
                let x = s.src_x(dx);
                let (o, nb) = b.pix(x);
                let p = rs + o;
                if p + nb <= b0 || p >= b1 {
                    continue;
                }
                if p >= b0 && p + nb <= b1 {
                    let q = (p - b0) as usize;
                    s.put(dx, dy, b.color(&d[q..q + nb as usize], x));
                    continue;
                }
                // Straddles a block edge: collect, draw when complete.
                let slot = match self.carry.iter().position(|c| c.p == p) {
                    Some(i) => i,
                    None => {
                        let i = self.carry.iter().position(|c| c.p == NONE).unwrap_or(0);
                        self.carry[i] = Carry { p, dx, dy, x, buf: [0; 4], have: 0 };
                        i
                    }
                };
                let c = &mut self.carry[slot];
                for j in 0..nb {
                    let q = p + j;
                    if q >= b0 && q < b1 {
                        c.buf[j as usize] = d[(q - b0) as usize];
                        c.have |= 1 << j;
                    }
                }
                if c.have == (1u8 << nb) - 1 {
                    let (cdx, cdy, cx, buf) = (c.dx, c.dy, c.x, c.buf);
                    c.p = NONE;
                    s.reset_run();
                    s.put(cdx, cdy, b.color(&buf[..nb as usize], cx));
                    s.reset_run();
                }
            }
        }
        probe_end();
    }
}
