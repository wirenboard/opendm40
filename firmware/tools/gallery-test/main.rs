//! Host test of src/gallery.rs (tools/gallery-test/run.sh): the real
//! gallery, csvlog, fatren, calbackup, screenshot, stockfont, dialog, lcd
//! code on the ILI9488 emulator of tools/ui-render/host.rs and FAT16 images
//! in the DM40 geometry built with mkfs.fat / mtools from gen.py's files.
//!   main ui <out dir> <data dir> <img dir>   worst-case matrix + layout
//!       rules (as tools/ui-render: >= 3 px between ink boxes, >= 3 px
//!       inside containers, >= 4 px from the screen edges, glyphs of one
//!       text do not touch), named scenes as PPM (2x) for the contact sheet
//!   main decode <img> <data dir>             every BMP of the image full
//!       screen and as a thumbnail against its .ref (nearest neighbour,
//!       the expected mapping computed here); unsupported ones refused;
//!       every CSV's statistics against its .exp
//!   main lock <img> <data dir>               disk lock: no read while busy,
//!       resume after; delete refused while mounted; delete
//! Prints "ok: ..." / "FAIL: ..." lines; exit 1 on any FAIL.
#![allow(dead_code, unused, static_mut_refs)]

#[path = "../ui-render/host.rs"]
mod host;
pub use host::mmio;

/// Probe hooks called by the drawing code under cfg(test); `begin_key`
/// continues one item over many draw calls (gallery.rs thumbnails, traces).
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
    static mut KEYS: Vec<(u32, u32)> = Vec::new();
    pub fn reset() {
        unsafe { KEYS.clear() }
    }
    pub fn begin(kind: u8, bg: u16) {
        crate::host::begin(kind, bg)
    }
    pub fn begin_key(kind: u8, bg: u16, key: u32) {
        let e = crate::host::emu();
        unsafe {
            if let Some(&(_, id)) = KEYS.iter().find(|(k, _)| *k == key) {
                e.call = Some((id, bg, 0));
                return;
            }
            crate::host::begin(kind, bg);
            KEYS.push((key, e.kinds.len() as u32));
        }
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

#[allow(dead_code)]
pub mod msc {
    use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
    /// Same trait as opendm40 src/msc.rs.
    pub trait BlockDevice {
        fn present(&self) -> bool;
        fn block_size(&self) -> u32;
        fn block_count(&self) -> u32;
        fn read_block(&mut self, lba: u32, buf: &mut [u8]) -> bool;
        fn write_block(&mut self, lba: u32, buf: &[u8]) -> bool;
        fn write_step(&mut self, lba: u32, buf: &dyn crate::wordram::BlockSrc, st: &mut Step) -> Option<bool> {
            let _ = st;
            Some(self.write_block(lba, &src_bytes(buf)))
        }
    }
    /// The bytes of a `write_step` source (src/wordram.rs `BlockSrc`).
    pub fn src_bytes(buf: &dyn crate::wordram::BlockSrc) -> Vec<u8> {
        (0..buf.len() / 4).flat_map(|i| buf.word(i).to_le_bytes()).collect()
    }
    #[derive(Clone, Copy, Default)]
    pub struct Step {
        pub started: bool,
        pub phase: u8,
        pub page: u8,
        pub dirty: u16,
        pub erase: bool,
        pub t0: u32,
    }
    pub static HOST_WRITING: AtomicBool = AtomicBool::new(false);
    pub static LOG_SESSION: AtomicBool = AtomicBool::new(false);
    pub static LOG_FLUSHING: AtomicBool = AtomicBool::new(false);
    pub static FLASH_WRITES: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    /// Test control of `host_mounted`.
    pub static MOUNTED: AtomicBool = AtomicBool::new(false);
    pub fn host_mounted() -> bool {
        MOUNTED.load(Relaxed)
    }
}

mod wdt {
    pub fn kick_if_feeding() {}
}

#[allow(dead_code)]
mod spiflash {
    pub struct SpiFlash;
    impl SpiFlash {
        pub fn new() -> Self {
            SpiFlash
        }
    }
    impl crate::msc::BlockDevice for SpiFlash {
        fn present(&self) -> bool {
            false
        }
        fn block_size(&self) -> u32 {
            4096
        }
        fn block_count(&self) -> u32 {
            0
        }
        fn read_block(&mut self, _: u32, _: &mut [u8]) -> bool {
            false
        }
        fn write_block(&mut self, _: u32, _: &[u8]) -> bool {
            false
        }
    }
}

#[path = "../../src/pack10.rs"]
mod pack10;
#[path = "../../src/ident.rs"]
mod ident;
#[path = "../../src/clock.rs"]
mod clock;
#[path = "../../src/lcd.rs"]
mod lcd;
#[path = "../../src/stockfont.rs"]
mod stockfont;
#[path = "../../src/dialog.rs"]
mod dialog;
#[path = "../../src/touch.rs"]
mod touch;
#[path = "../../src/gesture.rs"]
mod gesture;
#[path = "../../src/fatdir.rs"]
mod fatdir;
#[path = "../../src/upload.rs"]
#[allow(dead_code)]
mod upload;
#[path = "../../src/fatren.rs"]
mod fatren;
#[path = "../../src/calbackup.rs"]
mod calbackup;
#[path = "../../src/diskbuf.rs"]
mod diskbuf;
#[path = "../../src/screenshot.rs"]
mod screenshot;
#[path = "../../src/scope_csv.rs"]
#[allow(dead_code)]
mod scope_csv;
#[path = "../../src/dlog.rs"]
mod dlog;
#[path = "../../src/wordram.rs"]
#[allow(dead_code)]
mod wordram;
#[path = "../../src/csvlog.rs"]
mod csvlog;
#[path = "../../src/fastenc.rs"]
#[allow(dead_code)]
mod fastenc;
#[path = "../../src/fastlog.rs"]
#[allow(dead_code)]
mod fastlog;
#[path = "../../src/gallery.rs"]
mod gallery;

use gesture::Gesture;
use host::{emu, H, W};
use std::sync::atomic::Ordering::Relaxed;

// ------------------------------------------------------------------ disk

struct Img {
    d: Vec<u8>,
    present: bool,
    reads: u32,
    writes: u32,
}

impl Img {
    fn load(p: &str) -> Img {
        Img { d: std::fs::read(p).unwrap(), present: true, reads: 0, writes: 0 }
    }
    fn none() -> Img {
        Img { d: vec![], present: false, reads: 0, writes: 0 }
    }
}

impl msc::BlockDevice for Img {
    fn present(&self) -> bool {
        self.present
    }
    fn block_size(&self) -> u32 {
        4096
    }
    fn block_count(&self) -> u32 {
        (self.d.len() / 4096) as u32
    }
    fn read_block(&mut self, lba: u32, buf: &mut [u8]) -> bool {
        let o = lba as usize * 4096;
        if !self.present || o + 4096 > self.d.len() {
            return false;
        }
        self.reads += 1;
        buf[..4096].copy_from_slice(&self.d[o..o + 4096]);
        true
    }
    fn write_block(&mut self, lba: u32, buf: &[u8]) -> bool {
        let o = lba as usize * 4096;
        if !self.present || o + 4096 > self.d.len() {
            return false;
        }
        self.writes += 1;
        self.d[o..o + 4096].copy_from_slice(&buf[..4096]);
        true
    }
}

/// Gallery files of the image in the gallery's order (independent parse):
/// the root's, then LOGS/, then SHOTS/ (fatdir.rs), each in directory order.
fn dir_order(img: &Img) -> Vec<String> {
    let d = &img.d;
    let le16 = |o: usize| d[o] as usize | (d[o + 1] as usize) << 8;
    let bs = le16(11);
    let spc = d[13] as usize;
    let res = le16(14);
    let nf = d[16] as usize;
    let ne = le16(17);
    let fsz = le16(22);
    let root = (res + nf * fsz) * bs;
    let data = root + ne * 32;
    let cb = spc * bs;
    // A directory's entries: the root region, or a cluster chain.
    let entries = |start: Option<usize>| -> Vec<[u8; 32]> {
        let mut out = vec![];
        let mut offs = vec![];
        match start {
            None => offs.extend((0..ne).map(|i| root + 32 * i)),
            Some(mut c) => {
                while (2..0xFFF0).contains(&c) {
                    offs.extend((0..cb / 32).map(|i| data + (c - 2) * cb + 32 * i));
                    c = le16(res * bs + 2 * c);
                }
            }
        }
        for o in offs {
            let mut e = [0u8; 32];
            e.copy_from_slice(&d[o..o + 32]);
            if e[0] == 0 {
                break;
            }
            out.push(e);
        }
        out
    };
    let name = |e: &[u8; 32]| {
        let base = String::from_utf8_lossy(&e[..8]).trim_end().to_string();
        let ext = String::from_utf8_lossy(&e[8..11]).trim_end().to_string();
        (format!("{base}.{ext}"), ext)
    };
    let rootv = entries(None);
    let sub = |n: &str| rootv.iter().find(|e| e[0] != 0xE5 && e[11] & 0x10 != 0 && name(e).0.trim_end_matches('.') == n).map(|e| le16_of(e, 26));
    let mut v = vec![];
    for dir in [Some(None), sub("LOGS").map(Some), sub("SHOTS").map(Some)].into_iter().flatten() {
        let es = match dir {
            None => rootv.clone(),
            Some(c) => entries(Some(c)),
        };
        for e in es {
            if e[0] == 0xE5 || e[11] & 0x18 != 0 || e[11] & 0x3F == 0x0F {
                continue;
            }
            let (n, ext) = name(&e);
            if (n.starts_with('S') && ext == "BMP") || (n.starts_with("LOG") && ext == "DLG") {
                v.push(n);
            }
        }
    }
    v
}

fn le16_of(e: &[u8; 32], o: usize) -> usize {
    e[o] as usize | (e[o + 1] as usize) << 8
}

fn rank_of(img: &Img, name: &str) -> u32 {
    let v = dir_order(img);
    let i = v.iter().position(|n| n == name).unwrap_or_else(|| panic!("{name} not on the disk"));
    (v.len() - 1 - i) as u32
}

// ------------------------------------------------------------------ driving

fn reset_screen() {
    host::reset(0);
    probe::reset();
}

fn run(g: &mut gallery::Gallery, img: &mut Img) -> usize {
    for i in 0..200_000 {
        if g.idle() {
            return i;
        }
        g.poll_on(img);
    }
    panic!("gallery never idle");
}

fn open(img: &mut Img) -> &'static mut gallery::Gallery {
    reset_screen();
    let g = gallery::get();
    g.open();
    g.draw();
    run(g, img);
    g
}

fn tap(g: &mut gallery::Gallery, x: u16, y: u16) -> gallery::Out {
    g.touch(Gesture::Tap(touch::Point { x, y }))
}

/// Viewer on `name`, overlay off.
fn view(img: &mut Img, name: &str) -> &'static mut gallery::Gallery {
    let g = open(img);
    let r = rank_of(img, name);
    g.test_view(r);
    run(g, img);
    g
}

// ------------------------------------------------------------------ check (as tools/ui-render)

#[derive(Clone, Copy, Debug, PartialEq)]
struct Bx {
    x0: i32,
    y0: i32,
    x1: i32,
    y1: i32,
}

impl Bx {
    fn new(x: i32, y: i32, w: i32, h: i32) -> Bx {
        Bx { x0: x, y0: y, x1: x + w - 1, y1: y + h - 1 }
    }
    fn add(&mut self, x: i32, y: i32) {
        self.x0 = self.x0.min(x);
        self.y0 = self.y0.min(y);
        self.x1 = self.x1.max(x);
        self.y1 = self.y1.max(y);
    }
    fn contains(&self, b: &Bx) -> bool {
        b.x0 >= self.x0 && b.x1 <= self.x1 && b.y0 >= self.y0 && b.y1 <= self.y1
    }
    fn gap(&self, b: &Bx) -> i32 {
        (b.x0 - self.x1 - 1).max(self.x0 - b.x1 - 1).max(b.y0 - self.y1 - 1).max(self.y0 - b.y1 - 1)
    }
    fn margin(&self, b: &Bx) -> i32 {
        (b.x0 - self.x0).min(self.x1 - b.x1).min(b.y0 - self.y0).min(self.y1 - b.y1)
    }
    fn area(&self) -> i32 {
        (self.x1 - self.x0 + 1) * (self.y1 - self.y0 + 1)
    }
}

const EMPTY: Bx = Bx { x0: i32::MAX, y0: i32::MAX, x1: i32::MIN, y1: i32::MIN };

struct Item {
    id: u32,
    kind: u8,
    b: Bx,
    glyphs: Vec<(u8, Bx)>,
}

fn kind_name(k: u8) -> &'static str {
    ["text", "shape", "icon"][k as usize]
}

fn items() -> Vec<Item> {
    let e = emu();
    let mut m: std::collections::BTreeMap<u32, Item> = Default::default();
    for y in 0..H {
        for x in 0..W {
            let o = e.owner[y * W + x];
            if o == 0 {
                continue;
            }
            let (id, g) = (o >> 8, (o & 255) as u8);
            let it = m.entry(id).or_insert_with(|| Item { id, kind: e.kinds[id as usize - 1], b: EMPTY, glyphs: vec![] });
            it.b.add(x as i32, y as i32);
            match it.glyphs.iter_mut().find(|(k, _)| *k == g) {
                Some((_, b)) => b.add(x as i32, y as i32),
                None => {
                    let mut b = EMPTY;
                    b.add(x as i32, y as i32);
                    it.glyphs.push((g, b));
                }
            }
        }
    }
    m.into_values().collect()
}

struct Checker {
    cases: usize,
    violations: usize,
    seen: std::collections::BTreeMap<String, (usize, String)>,
}

impl Checker {
    fn fail(&mut self, scene: &str, msg: String) {
        self.violations += 1;
        let key: String = msg.split(' ').filter(|w| !w.starts_with('#')).collect::<Vec<_>>().join(" ");
        let e = self.seen.entry(key).or_insert((0, scene.to_string()));
        e.0 += 1;
    }

    fn check(&mut self, scene: &str, regions: &[(&str, Bx)]) {
        self.cases += 1;
        let its = items();
        let screen = Bx::new(0, 0, W as i32, H as i32);
        let desc = |i: &Item| format!("{} #{} {:?}", kind_name(i.kind), i.id, i.b);
        let mut parent: Vec<Option<usize>> = vec![None; its.len()];
        for (a, ia) in its.iter().enumerate() {
            let mut best: Option<(usize, i32)> = None;
            for (b, ib) in its.iter().enumerate() {
                if a != b && ib.kind == probe::SHAPE && ib.b.contains(&ia.b) && ib.b.area() > ia.b.area() {
                    if best.map_or(true, |(_, ar)| ib.b.area() < ar) {
                        best = Some((b, ib.b.area()));
                    }
                }
            }
            parent[a] = best.map(|x| x.0);
            if screen.margin(&ia.b) < 4 {
                self.fail(scene, format!("{} only {} px from the screen edge", desc(ia), screen.margin(&ia.b)));
            }
            match parent[a] {
                Some(p) => {
                    let m = its[p].b.margin(&ia.b);
                    if m < 3 {
                        self.fail(scene, format!("{} only {} px inside {}", desc(ia), m, desc(&its[p])));
                    }
                }
                None => {
                    let r = regions.iter().filter(|(_, r)| r.contains(&ia.b)).min_by_key(|(_, r)| r.area());
                    match r {
                        Some((name, r)) => {
                            let m = r.margin(&ia.b);
                            if m < 3 {
                                self.fail(scene, format!("{} only {} px inside region {} {:?}", desc(ia), m, name, r));
                            }
                        }
                        None => self.fail(scene, format!("{} not inside any region", desc(ia))),
                    }
                }
            }
            if ia.kind == probe::TEXT && ia.glyphs.len() > 1 {
                let mut g = ia.glyphs.clone();
                g.sort_by_key(|(k, _)| *k);
                // Widest gap between letters of one word.
                let letter = g.windows(2).filter(|w| w[1].0 == w[0].0 + 1).map(|w| w[1].1.x0 - w[0].1.x1 - 1).max().unwrap_or(1);
                for w in g.windows(2) {
                    let (a, b) = (w[0].1, w[1].1);
                    if a.gap(&b) < 1 {
                        self.fail(scene, format!("{}: glyphs {} / {} touch ({:?} {:?})", desc(ia), w[0].0, w[1].0, a, b));
                    }
                    // An ink-less glyph between them (a space): a word gap
                    // clearly wider than the letter gaps (coordinator: the
                    // 3 px F14 spaces read as "Nodata", "40rows").
                    let need = 4.max(letter + 2);
                    if w[1].0 > w[0].0 + 1 && b.x0 - a.x1 - 1 < need {
                        self.fail(scene, format!("{}: word gap after glyph {} only {} px (letters up to {})", desc(ia), w[0].0, b.x0 - a.x1 - 1, letter));
                    }
                }
            }
        }
        for a in 0..its.len() {
            for b in a + 1..its.len() {
                let (ia, ib) = (&its[a], &its[b]);
                let anc = |mut x: usize, y: usize| {
                    while let Some(p) = parent[x] {
                        if p == y {
                            return true;
                        }
                        x = p;
                    }
                    false
                };
                if anc(a, b) || anc(b, a) {
                    continue;
                }
                let g = ia.b.gap(&ib.b);
                // Two texts side by side on one line read as one: a word gap.
                let same_line = ia.kind == probe::TEXT && ib.kind == probe::TEXT && ia.b.y0 <= ib.b.y1 && ib.b.y0 <= ia.b.y1;
                if same_line && (3..4).contains(&g) {
                    self.fail(scene, format!("{} and {} on one line only {} px apart", desc(ia), desc(ib), g));
                }
                if g < 3 {
                    self.fail(scene, format!("{} and {} {} by {} px", desc(ia), desc(ib), if g < 0 { "overlap" } else { "too close" }, g));
                }
            }
        }
    }
}

fn regions() -> Vec<(&'static str, Bx)> {
    let b = dialog::BOX;
    let r = |t: (u16, u16, u16, u16)| Bx::new(t.0 as i32, t.1 as i32, t.2 as i32, t.3 as i32);
    vec![
        ("screen", Bx::new(0, 0, 480, 320)),
        ("bar", Bx::new(0, 0, 480, 40)),
        ("body", Bx::new(0, 40, 480, 280)),
        ("dialog title", Bx::new(b.0 as i32, b.1 as i32, b.2 as i32, 40)),
        ("dialog body", Bx::new(b.0 as i32, b.1 as i32 + 40, b.2 as i32, 60)),
        ("dialog no", r(dialog::NO)),
        ("dialog yes", r(dialog::YES)),
        ("dialog ok", Bx::new(176, 180, 128, 48)),
    ]
}

fn save(out: &str, name: &str) {
    host::save_ppm(&format!("{out}/{name}.ppm"), 2);
}

// ------------------------------------------------------------------ ui matrix

fn ui(out: &str, data: &str, imgs: &str) -> bool {
    let mut ck = Checker { cases: 0, violations: 0, seen: Default::default() };
    let reg = regions();
    let load = |n: &str| Img::load(&format!("{imgs}/{n}.img"));
    let mut shots: Vec<&str> = vec![];
    let mut scene = |ck: &mut Checker, name: &str, keep: bool| {
        ck.check(name, &reg);
        if keep {
            save(out, &name.replace('/', "-"));
        }
    };

    // Grids: every disk, every page.
    for (n, keep) in [("e0", true), ("e1", true), ("e6", true), ("e7", true), ("e99", true), ("fmt", false)] {
        let mut img = load(n);
        let g = open(&mut img);
        let pages = (g.test_state().0 as usize).div_ceil(6).max(1);
        for p in 0..pages {
            if p > 0 {
                g.touch(Gesture::SwipeLeft);
                run(g, &mut img);
            }
            let k = keep && (p == 0 || p == pages - 1);
            scene(&mut ck, &format!("grid-{n}-p{}", p + 1), k);
        }
        // Back to the first page with taps on Prev.
        for _ in 1..pages {
            tap(g, 60, 300);
            run(g, &mut img);
        }
        ck.cases += 1;
        if g.test_state().1 != 0 {
            ck.fail("nav", format!("{n}: Prev taps did not return to page 1"));
        }
    }

    // Disk busy before the listing, no disk, busy during thumbnails.
    {
        let mut img = load("e6");
        reset_screen();
        msc::HOST_WRITING.store(true, Relaxed);
        let g = gallery::get();
        g.open();
        g.draw();
        for _ in 0..20 {
            g.poll_on(&mut img);
        }
        scene(&mut ck, "busy-listing", true);
        msc::HOST_WRITING.store(false, Relaxed);
        for _ in 0..30 {
            g.poll_on(&mut img);
        }
        msc::HOST_WRITING.store(true, Relaxed);
        for _ in 0..5 {
            g.poll_on(&mut img);
        }
        scene(&mut ck, "busy-thumbnails", true);
        msc::HOST_WRITING.store(false, Relaxed);
        run(g, &mut img);
        scene(&mut ck, "busy-after", false);
        let mut none = Img::none();
        reset_screen();
        g.open();
        g.draw();
        run(g, &mut none);
        scene(&mut ck, "nodisk", true);
    }

    // Viewer: every file of the format disk, overlay off / on; CSV charts.
    {
        let mut img = load("fmt");
        for name in dir_order(&img) {
            let keep = matches!(name.as_str(), "S003.BMP" | "LOG001.DLG" | "S012.BMP" | "LOG004.DLG" | "LOG002.DLG");
            let g = view(&mut img, &name);
            scene(&mut ck, &format!("view-{name}"), keep && name.ends_with("DLG"));
            if name.ends_with("BMP") {
                tap(g, 240, 200); // overlay on
                run(g, &mut img);
                scene(&mut ck, &format!("view-{name}-bar"), keep);
                tap(g, 240, 200); // off again: the bar rows come back
                run(g, &mut img);
                scene(&mut ck, &format!("view-{name}-nobar"), name == "S003.BMP");
            }
        }
        // Long names in the bar, 7-file index.
        let mut img = load("e7");
        let g = view(&mut img, "SWWWWWWW.BMP");
        tap(g, 240, 200);
        run(g, &mut img);
        scene(&mut ck, "view-long-name-bar", true);
        let g = view(&mut img, "LOG99999.DLG");
        scene(&mut ck, "view-LOG99999", true);
        let mut img = load("e99");
        let g = view(&mut img, "S100.BMP");
        tap(g, 240, 200);
        run(g, &mut img);
        scene(&mut ck, "view-99-bar", false);
        // Busy before the viewer stream.
        let mut img = load("fmt");
        let g = view(&mut img, "LOG001.DLG");
        let r = rank_of(&img, "LOG002.DLG");
        g.test_view(r);
        for _ in 0..6 {
            g.poll_on(&mut img);
        }
        scene(&mut ck, "view-csv-loading", true);
        msc::HOST_WRITING.store(true, Relaxed);
        g.test_view(rank_of(&img, "LOG000.DLG"));
        g.draw();
        for _ in 0..3 {
            g.poll_on(&mut img);
        }
        scene(&mut ck, "view-csv-busy", false);
        msc::HOST_WRITING.store(false, Relaxed);
        run(g, &mut img);
        // Swipe to the next file while the disk is busy: a message, then
        // that file (not the grid).
        let mut img = load("e6");
        let last = dir_order(&img)[5].clone();
        let g = view(&mut img, &last);
        msc::HOST_WRITING.store(true, Relaxed);
        g.touch(Gesture::SwipeLeft);
        for _ in 0..5 {
            g.poll_on(&mut img);
        }
        scene(&mut ck, "view-busy", true);
        msc::HOST_WRITING.store(false, Relaxed);
        run(g, &mut img);
        ck.cases += 1;
        if g.test_mode() != "view" || g.test_state().2 != 1 {
            ck.fail("view-busy", format!("after busy: mode {} rank {}", g.test_mode(), g.test_state().2));
        }
    }

    // Dialogs: delete confirm over BMP / CSV (full repaint), info boxes.
    {
        let mut img = load("e7");
        for name in ["SWWWWWWW.BMP", "LOG99999.DLG"] {
            let g = view(&mut img, &name);
            if name.ends_with("BMP") {
                tap(g, 240, 200);
                run(g, &mut img);
            }
            tap(g, 420, 20); // Delete
            run(g, &mut img);
            reset_screen();
            g.draw();
            scene(&mut ck, &format!("confirm-{name}"), true);
            for (t, m) in [("Disk in use by the PC", "Eject it, then retry"), ("Cannot delete", "Disk busy, try again"), ("Cannot delete", "Stop the recording"), ("Cannot delete", "File not found"), ("Cannot delete", "Delete failed")] {
                g.test_info(t, m);
                reset_screen();
                g.draw();
                scene(&mut ck, &format!("info-{m}"), m.starts_with("Eject") && name.ends_with("BMP"));
            }
            g.back();
            run(g, &mut img);
        }
    }

    for (msg, (n, scene)) in &ck.seen {
        println!("{n:6} x  {msg}   [first: {scene}]");
    }
    println!("cases {}, violations {}", ck.cases, ck.violations);
    ck.violations == 0
}

// ------------------------------------------------------------------ decode

fn read_ref(p: &str) -> Option<(u32, u32, Vec<u16>)> {
    let d = std::fs::read(p).ok()?;
    let w = u32::from_le_bytes(d[0..4].try_into().unwrap());
    let h = u32::from_le_bytes(d[4..8].try_into().unwrap());
    if w == u32::MAX {
        return Some((w, 0, vec![]));
    }
    let px = d[8..].chunks(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
    Some((w, h, px))
}

/// Expected nearest-neighbour fit of a w x h image into a box.
fn expect(w: u32, h: u32, px: &[u16], bx: i32, by: i32, bw: u32, bh: u32) -> Vec<(i32, i32, u16)> {
    let (mut dw, mut dh) = (w, h);
    if dw > bw || dh > bh {
        if w * bh >= h * bw {
            dw = bw;
            dh = (h * bw / w).max(1);
        } else {
            dh = bh;
            dw = (w * bh / h).max(1);
        }
    }
    let (ox, oy) = (bx + ((bw - dw) / 2) as i32, by + ((bh - dh) / 2) as i32);
    let mut v = vec![];
    for y in 0..bh as i32 {
        for x in 0..bw as i32 {
            let (sx, sy) = (bx + x, by + y);
            let (dx, dy) = (sx - ox, sy - oy);
            let c = if dx >= 0 && dy >= 0 && (dx as u32) < dw && (dy as u32) < dh {
                let (ix, iy) = (dx as u32 * w / dw, dy as u32 * h / dh);
                px[(iy * w + ix) as usize]
            } else {
                0
            };
            v.push((sx, sy, c));
        }
    }
    v
}

fn compare(what: &str, exp: &[(i32, i32, u16)]) -> bool {
    let fb = &emu().fb;
    let bad: Vec<_> = exp.iter().filter(|(x, y, c)| fb[*y as usize * W + *x as usize] != *c).collect();
    if bad.is_empty() {
        println!("ok: {what}: {} pixels match", exp.len());
        true
    } else {
        let (x, y, c) = bad[0];
        println!("FAIL: {what}: {} of {} pixels differ, first ({x},{y}) {:04x} != {:04x}", bad.len(), exp.len(), fb[*y as usize * W + *x as usize], c);
        false
    }
}

fn decode(imgp: &str, data: &str) -> bool {
    let mut ok = true;
    let mut img = Img::load(imgp);
    let names = dir_order(&img);
    let total = names.len() as u32;
    for name in &names {
        let base = name.split('.').next().unwrap();
        if name.ends_with("BMP") {
            let Some((w, h, px)) = read_ref(&format!("{data}/{base}.ref")) else { continue };
            let g = view(&mut img, name);
            let reads = img.reads;
            if w == u32::MAX {
                // Unsupported: a message, no image (the screen is black + text).
                let lit = emu().fb.iter().filter(|&&c| c != 0).count();
                let good = lit > 0 && lit < 20_000;
                println!("{}: {name} unsupported -> message ({lit} lit pixels)", if good { "ok" } else { "FAIL" });
                ok &= good;
                continue;
            }
            ok &= compare(&format!("{name} {w}x{h} full screen"), &expect(w, h, &px, 0, 0, 480, 320));
            // Thumbnail on its grid page.
            let r = rank_of(&img, name);
            let g = open(&mut img);
            for _ in 0..r / 6 {
                g.touch(Gesture::SwipeLeft);
                run(g, &mut img);
            }
            let i = (r % 6) as i32;
            let (tx, ty) = ([10, 166, 322][(i % 3) as usize], [46, 168][(i / 3) as usize]);
            ok &= compare(&format!("{name} thumbnail"), &expect(w, h, &px, tx + 4, ty + 4, 140, 93));
        } else {
            let e = std::fs::read_to_string(format!("{data}/{base}.exp")).unwrap();
            let f: Vec<f64> = e.split_whitespace().map(|s| s.parse().unwrap()).collect();
            let g = view(&mut img, name);
            let (rows, vals, mn, mx, avg, span) = g.csv_stats();
            let close = |a: f64, b: f64, tol: f64| (a - b).abs() <= tol * b.abs().max(1e-12);
            let good = rows as f64 == f[0]
                && vals as f64 == f[1]
                && (vals == 0 || (close(mn as f64, f[2], 1e-5) && close(mx as f64, f[3], 1e-5) && close(avg as f64, f[4], 1e-4)))
                && span as f64 == f[5];
            println!(
                "{}: {name}: rows {rows} vals {vals} min {mn} max {mx} avg {avg} span {span} ms | expected {:?}",
                if good { "ok" } else { "FAIL" },
                f
            );
            ok &= good;
        }
    }
    ok
}

// ------------------------------------------------------------------ lock

fn lock(imgp: &str, data: &str) -> bool {
    let mut ok = true;
    let mut t = |what: &str, good: bool| {
        println!("{}: {what}", if good { "ok" } else { "FAIL" });
        ok &= good;
    };
    let mut img = Img::load(imgp);
    // 1. Busy at open: no read until free.
    reset_screen();
    msc::HOST_WRITING.store(true, Relaxed);
    let g = gallery::get();
    g.open();
    g.draw();
    for _ in 0..50 {
        g.poll_on(&mut img);
    }
    t("host writing: no block read while busy", img.reads == 0);
    msc::HOST_WRITING.store(false, Relaxed);
    run(g, &mut img);
    t("listed once free", g.test_state().0 == dir_order(&img).len() as u32);

    // 2. Busy in the middle of the viewer stream: pause, resume, same image.
    let name = "S000.BMP";
    let (w, h, px) = read_ref(&format!("{data}/S000.ref")).unwrap();
    let g = open(&mut img);
    g.test_view(rank_of(&img, name));
    for _ in 0..20 {
        g.poll_on(&mut img);
    }
    msc::HOST_WRITING.store(true, Relaxed);
    let r0 = img.reads;
    for _ in 0..30 {
        g.poll_on(&mut img);
    }
    t("viewer: no read while the host writes", img.reads == r0);
    msc::HOST_WRITING.store(false, Relaxed);
    run(g, &mut img);
    t("viewer: image complete after the pause", compare("S000 after pause", &expect(w, h, &px, 0, 0, 480, 320)));

    let before = dir_order(&img);
    // 4. Delete: gone, the viewer moves on.
    let g = view(&mut img, name);
    let r = rank_of(&img, name);
    tap(g, 240, 200);
    run(g, &mut img);
    tap(g, 420, 20);
    run(g, &mut img);
    tap(g, 300, 200);
    run(g, &mut img);
    let after = dir_order(&img);
    t("deleted from the directory", !after.contains(&name.to_string()) && after.len() + 1 == before.len());
    let (total, _, rank, _) = g.test_state();
    t("viewer shows the next file", total as usize == after.len() && rank == r.min(total - 1));
    // 5. Taps: a tile opens the viewer, swipes step, the bar returns.
    let g = open(&mut img);
    let n = g.test_state().0;
    tap(g, 10 + 74, 46 + 50); // tile 0 = newest
    run(g, &mut img);
    t("tap on tile 0: viewer on rank 0", g.test_mode() == "view" && g.test_state().2 == 0);
    g.touch(Gesture::SwipeLeft);
    run(g, &mut img);
    t("swipe left: next file", g.test_state().2 == 1.min(n - 1));
    g.touch(Gesture::SwipeRight);
    g.touch(Gesture::SwipeRight);
    run(g, &mut img);
    t("swipe right: previous, stops at the first", g.test_state().2 == 0);
    tap(g, 240, 200); // overlay (or no-op on a CSV: its bar is always up)
    run(g, &mut img);
    tap(g, 60, 20); // "< Back"
    run(g, &mut img);
    t("bar back: grid", g.test_mode() == "grid");
    t("title bar in the grid: leave", tap(g, 60, 20) == gallery::Out::Exit);
    t("MENU key in the grid: leave", g.back() == gallery::Out::Exit);
    // 6. Busy flags of the on-device writers.
    csvlog_busy(&mut t);
    // 7. The recording in progress (issue #86): Delete says to stop it.
    recording(&mut img, &mut t);
    std::fs::write(imgp, &img.d).unwrap();
    ok
}

fn recording(img: &mut Img, t: &mut dyn FnMut(&str, bool)) {
    t("csvlog start", csvlog::start("DC V", "V", "AUTO", 1000));
    let mut now = 1000;
    for _ in 0..50 {
        now += 200;
        csvlog::push(now, 1.0, 0, 6.0);
        csvlog::poll_on(img, now);
    }
    let name = csvlog::recording_name();
    t("the recording has its file", name.is_some());
    let g = open(img);
    let n = g.test_state().0;
    let r = (0..n).find(|&r| {
        g.test_view(r);
        run(g, img);
        Some(g.test_info_cur().1) == name
    });
    t("the recording is listed", r.is_some());
    tap(g, 420, 20); // Delete
    run(g, img);
    tap(g, 300, 200); // Delete in the confirmation
    run(g, img);
    t("Delete of the recording: Stop the recording", g.test_info_cur().0 == Some("Stop the recording"));
    t("the recording is still listed", g.test_state().0 == n);
    csvlog::stop();
    for _ in 0..50 {
        now += 200;
        csvlog::poll_on(img, now);
    }
    t("recording stopped", !csvlog::active());
    t("its file deleted after the stop", name.is_some_and(|nm| csvlog::delete_on(img, &nm).is_ok()));
}

fn csvlog_busy(t: &mut dyn FnMut(&str, bool)) {
    t("busy_reason none when idle", gallery::busy_reason().is_none());
    msc::HOST_WRITING.store(true, Relaxed);
    t("busy_reason host", gallery::busy_reason() == Some(gallery::Busy::Host));
    msc::HOST_WRITING.store(false, Relaxed);
}

/// A csvlog-format CSV re-coded as a compact binary log (dlog.rs), as
/// csvlog.rs would have recorded the same readings (chunks of 600 rows).
fn dlg(inp: &str, out: &str) -> bool {
    let t = std::fs::read(inp).unwrap();
    let txt = String::from_utf8(t).unwrap();
    let hl = txt.find("time_s,value,unit,flags\r\n").unwrap() + 25;
    let (hdr, body) = txt.split_at(hl);
    let mut unit = String::new();
    let mut rows = vec![];
    for l in body.split("\r\n").filter(|l| !l.is_empty()) {
        let f: Vec<&str> = l.split(',').collect();
        let (s, ms) = f[0].split_once('.').unwrap();
        let tm: u64 = s.parse::<u64>().unwrap() * 1000 + ms.parse::<u64>().unwrap();
        let v: f32 = f[1].parse().unwrap_or(f32::NAN);
        unit = f[2].to_string();
        let mut fl = 0u8;
        for (k, n) in ["OL", "REL", "HOLD", "HI", "LO", "GAP"].iter().enumerate() {
            if f[3].split(' ').any(|x| x == *n) {
                fl |= 1 << k;
            }
        }
        rows.push((tm, v, fl));
    }
    let mut d = vec![0u8; dlog::FHDR + hdr.len()];
    dlog::file_header(dlog::KIND_DMM, unit.as_bytes(), hdr.as_bytes(), &mut d);
    let mut e = dlog::Enc::new(hdr.len());
    for ch in rows.chunks(600) {
        let h0 = d.len();
        d.extend_from_slice(&[0u8; dlog::CHDR]);
        for &(tm, v, fl) in ch {
            let val = dlog::dec6(v);
            let mut r = [0u8; dlog::ROW_MAX];
            let n = dlog::fmt_row(tm, val, unit.as_bytes(), fl, &mut r);
            let mut b = [0u8; dlog::ROW_BYTES];
            let k = e.row(tm as u32, val, fl, n as u32, &mut b);
            d.extend_from_slice(&b[..k]);
        }
        let mut b = [0u8; 1];
        let k = e.close(&mut b);
        d.extend_from_slice(&b[..k]);
        let mut h = e.header();
        let c = !dlog::crc32_upd(dlog::crc32_upd(!0, &h[8..]), &d[h0 + dlog::CHDR..]);
        h[4..8].copy_from_slice(&c.to_le_bytes());
        d[h0..h0 + dlog::CHDR].copy_from_slice(&h);
    }
    std::fs::write(out, &d).unwrap();
    println!("{inp} -> {out}: {} rows, {} -> {} bytes", rows.len(), txt.len(), d.len());
    true
}

fn main() {
    host::map_flash();
    println!("size_of Gallery = {} bytes", std::mem::size_of::<gallery::Gallery>());
    let a: Vec<String> = std::env::args().collect();
    let ok = match a[1].as_str() {
        "ui" => ui(&a[2], &a[3], &a[4]),
        "decode" => decode(&a[2], &a[3]),
        "lock" => lock(&a[2], &a[3]),
        "dlg" => dlg(&a[2], &a[3]),
        _ => panic!("ui | decode | lock | dlg"),
    };
    std::process::exit(if ok { 0 } else { 1 });
}
