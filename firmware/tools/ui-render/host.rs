//! Host stand-ins for the hardware layer under the real opendm40 drawing code
//! (tools/ui-render): an ILI9488 emulator behind `mmio::w16` (window +
//! RAMWR, the only LCD protocol the drawing code uses), no-op register
//! access, and the internal flash mapped as erased (0xFF) at 0x08000000 so
//! code that reads calibration words directly sees "no calibration".
//! The emulator also keeps, per pixel, which probed draw call last put ink
//! there (see `probe`), for the layout check.
#![allow(dead_code, static_mut_refs)]

pub const W: usize = 480;
pub const H: usize = 320;

pub struct Emu {
    pub fb: Vec<u16>,
    /// Owner of each pixel: 0 = background / unprobed, else call id << 8 | glyph.
    pub owner: Vec<u32>,
    win: (usize, usize, usize, usize),
    cur: (usize, usize),
    cmd: u16,
    args: Vec<u16>,
    writing: bool,
    /// Active probed call: (id, bg, glyph index).
    pub call: Option<(u32, u16, u8)>,
    /// Kind of every probed call (index = id - 1).
    pub kinds: Vec<u8>,
    /// Calls grouped as one text (fixed cells of one number): (id, cell).
    pub group: Option<(u32, u8)>,
    pub grouping: bool,
    /// Pixels written (RAMWR data words), for the redraw budget checks.
    pub writes: u64,
    /// Every 16-bit bus write (commands + window arguments + pixels) and
    /// RAMWR commands (= windows opened), for the DMM redraw budget.
    pub bus: u64,
    pub wins: u64,
}

pub static mut EMU: Option<Emu> = None;

pub fn emu() -> &'static mut Emu {
    unsafe {
        if EMU.is_none() {
            EMU = Some(Emu {
                fb: vec![0; W * H],
                owner: vec![0; W * H],
                win: (0, 0, W - 1, H - 1),
                cur: (0, 0),
                cmd: 0,
                args: Vec::new(),
                writing: false,
                call: None,
                kinds: Vec::new(),
                group: None,
                grouping: false,
                writes: 0,
                bus: 0,
                wins: 0,
            });
        }
        EMU.as_mut().unwrap()
    }
}

/// `reset` fills the screen with a pixel hash instead of its colour (an
/// arbitrary previous screen: the deferred-fill check, main.rs).
pub static mut PATTERN: bool = false;

pub fn reset(bg: u16) {
    let e = emu();
    let pat = unsafe { PATTERN };
    e.fb.iter_mut().enumerate().for_each(|(i, p)| *p = if pat { (i as u32).wrapping_mul(2654435761).rotate_right(13) as u16 } else { bg });
    e.owner.iter_mut().for_each(|p| *p = 0);
    e.call = None;
    e.kinds.clear();
}

/// The emulator state a page draw leaves (screen, pixel owners, probed
/// calls, bus position; not the write counters): `snapshot` after drawing
/// a page that many cases share, `restore` per case instead of drawing it
/// again.
pub struct Snap {
    fb: Vec<u16>,
    owner: Vec<u32>,
    kinds: Vec<u8>,
    win: (usize, usize, usize, usize),
    cur: (usize, usize),
    cmd: u16,
    args: Vec<u16>,
    writing: bool,
    call: Option<(u32, u16, u8)>,
    group: Option<(u32, u8)>,
    grouping: bool,
}

pub fn snapshot() -> Snap {
    let e = emu();
    Snap {
        fb: e.fb.clone(),
        owner: e.owner.clone(),
        kinds: e.kinds.clone(),
        win: e.win,
        cur: e.cur,
        cmd: e.cmd,
        args: e.args.clone(),
        writing: e.writing,
        call: e.call,
        group: e.group,
        grouping: e.grouping,
    }
}

pub fn restore(s: &Snap) {
    let e = emu();
    e.fb.copy_from_slice(&s.fb);
    e.owner.copy_from_slice(&s.owner);
    e.kinds.clone_from(&s.kinds);
    e.win = s.win;
    e.cur = s.cur;
    e.cmd = s.cmd;
    e.args.clone_from(&s.args);
    e.writing = s.writing;
    e.call = s.call;
    e.group = s.group;
    e.grouping = s.grouping;
}

/// Same screen, owners and probed calls as `s` (UIR_VERIFY runs).
pub fn same(s: &Snap) -> bool {
    let e = emu();
    e.fb == s.fb && e.owner == s.owner && e.kinds == s.kinds
}

/// Probe hooks (stockfont / ui under cfg(test)): a draw call starts / ends,
/// and the glyph it is drawing (255 = none).
pub fn begin(kind: u8, bg: u16) {
    let e = emu();
    if e.grouping {
        let (id, cell) = match e.group {
            Some((id, c)) => (id, c + 1),
            None => {
                e.kinds.push(kind);
                (e.kinds.len() as u32, 0)
            }
        };
        e.group = Some((id, cell));
        e.call = Some((id, bg, cell));
        return;
    }
    e.kinds.push(kind);
    e.call = Some((e.kinds.len() as u32, bg, 0));
}

pub fn sub(g: u8) {
    let e = emu();
    let cell = e.group.map(|x| x.1);
    if let Some(c) = e.call.as_mut() {
        c.2 = match (e.grouping, cell) {
            (true, Some(k)) => if g == 255 { 255 } else { k },
            _ => g,
        };
    }
}

/// Following calls form one text (one item, a glyph per call) until off.
pub fn group(on: bool) {
    let e = emu();
    e.grouping = on;
    e.group = None;
}

pub fn end() {
    emu().call = None;
}

/// Register one probed element (kind, background) of a composite window
/// (stockfont::paint) and return its call id; `set` then selects, per
/// pixel, which element the next write belongs to (id 0 = none).
pub fn open(kind: u8, bg: u16) -> u32 {
    let e = emu();
    e.kinds.push(kind);
    e.kinds.len() as u32
}

pub fn set(id: u32, bg: u16, g: u8) {
    emu().call = if id == 0 { None } else { Some((id, bg, g)) };
}

fn lcd_w16(addr: u32, v: u16) {
    let e = emu();
    if addr == 0x6001_FFFE || addr == 0x6002_0000 {
        e.bus += 1;
    }
    if addr == 0x6001_FFFE {
        e.wins += (v == 0x2C) as u64;
        e.cmd = v;
        e.args.clear();
        e.writing = v == 0x2C;
        if e.writing {
            e.cur = (e.win.0, e.win.1);
        }
        return;
    }
    if addr != 0x6002_0000 {
        return;
    }
    if e.writing {
        e.writes += 1;
        let (x, y) = e.cur;
        if x < W && y < H {
            e.fb[y * W + x] = v;
            e.owner[y * W + x] = match e.call {
                Some((id, bg, g)) if v != bg => id << 8 | g as u32,
                _ => 0,
            };
        }
        let mut nx = x + 1;
        let mut ny = y;
        if nx > e.win.2 {
            nx = e.win.0;
            ny += 1;
            if ny > e.win.3 {
                ny = e.win.1;
            }
        }
        e.cur = (nx, ny);
        return;
    }
    e.args.push(v);
    if e.args.len() == 4 {
        let a = ((e.args[0] as usize) << 8) | e.args[1] as usize;
        let b = ((e.args[2] as usize) << 8) | e.args[3] as usize;
        match e.cmd {
            0x2A => {
                e.win.0 = a;
                e.win.2 = b;
            }
            0x2B => {
                e.win.1 = a;
                e.win.3 = b;
            }
            _ => {}
        }
    }
}

pub mod mmio {
    pub unsafe fn r32(_: u32) -> u32 {
        0
    }
    pub unsafe fn w32(_: u32, _: u32) {}
    pub unsafe fn w16(a: u32, v: u16) {
        super::lcd_w16(a, v)
    }
    pub unsafe fn modify32(_: u32, _: u32, _: u32) {}
    pub fn spin(_: u32) {}
}

extern "C" {
    fn mmap(addr: *mut u8, len: usize, prot: i32, flags: i32, fd: i32, off: i64) -> *mut u8;
}

/// Map 0x08000000..0x08100000 (internal flash) as erased flash.
pub fn map_flash() {
    unsafe {
        // PROT_READ|PROT_WRITE, MAP_PRIVATE|MAP_ANONYMOUS|MAP_FIXED_NOREPLACE
        let p = mmap(0x0800_0000 as *mut u8, 0x10_0000, 3, 0x02 | 0x20 | 0x10_0000, -1, 0);
        assert_eq!(p as usize, 0x0800_0000, "flash mmap failed");
        core::ptr::write_bytes(p, 0xFF, 0x10_0000);
    }
}

/// RGB888 of an RGB565 pixel.
pub fn rgb(c: u16) -> [u8; 3] {
    let r = (c >> 11 & 31) as u32 * 255 / 31;
    let g = (c >> 5 & 63) as u32 * 255 / 63;
    let b = (c & 31) as u32 * 255 / 31;
    [r as u8, g as u8, b as u8]
}

/// Write the frame buffer as a binary PPM (scaled by `k`, nearest).
pub fn save_ppm(path: &str, k: usize) {
    let e = emu();
    let mut out = format!("P6 {} {} 255\n", W * k, H * k).into_bytes();
    for y in 0..H * k {
        for x in 0..W * k {
            out.extend_from_slice(&rgb(e.fb[(y / k) * W + x / k]));
        }
    }
    std::fs::write(path, out).unwrap();
}
