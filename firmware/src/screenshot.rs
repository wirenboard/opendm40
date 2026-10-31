//! Screenshot to the SPI-flash disk (docs/user-feedback.md UF-17, stock
//! "PriScr": 3-finger swipe down, gui_v2_6.h "Screenshot"). The same job
//! writes the scope record (UF-13, `start_csv`, content from scope_csv.rs:
//! SCOPE/SCOP%03u.DLG, the compact binary form of the CSV, which the USB
//! volume / MMEM:DATA? serve as SCOP%03u.CSV; no capture phase, length from
//! scope_csv).
//!
//! File: the next free `SHOTS/S%03u.BMP` (fatdir.rs; lowest number used
//! neither in SHOTS/ nor in the root, where earlier builds wrote them, as the
//! stock "s%.3d.bmp" / first free number, spiflash_v2_6.h), byte-compatible
//! with the stock files on this unit's disk (dumps/spi_dumps
//! dm40_msc_full_2026-10-13.bin S000..S002.BMP): 480x320, 16 bpp RGB565,
//! BI_BITFIELDS (masks F800/07E0/001F), 66-byte header (14 file + 40 info +
//! 12 masks), positive height = bottom-up rows of 960 bytes (no padding),
//! pixels little-endian, 72 dpi fields 0: 307266 bytes = 76 clusters of 4 KiB.
//! (Not 24-bit: that would be 460854 bytes. The 17 KB S00x.BMP in
//! private/spi-disk-backup/ are older 4-bpp RLE factory files.)
//!
//! Incremental: `start` only takes the request; `poll` (every main-loop pass,
//! from App::tick) does one bounded slice of the job, so USB (SCPI and the
//! disk), the DMM, the scope and the watchdog keep running during the write
//! (~1.4 s with the clusters prepared, ~5 s without; see below). One shot at
//! a time (`start` refuses while `busy`). Slices:
//!   Start    wait for a host WRITE(10) in progress to finish (msc.rs;
//!            at most 2 s, then "failed"); a shot is refused ("Disk busy")
//!            while a fast-log session or an upload holds the disk;
//!   Bpb      boot sector (1 block read); first waits while a csvlog.rs
//!            commit holds the disk (msc.rs LOG_FLUSHING);
//!   Bpb      also opens SHOTS/ / SCOPE/ (fatdir::ensure: created here only
//!            if missing, synchronously);
//!   Root     one directory block per pass (the root, then SHOTS/ or SCOPE/:
//!            used numbers in either, the first free slot of the directory);
//!   Alloc    one FAT 1 block per pass: first-fit free clusters;
//!   Data     per 4 KiB block: fill (header / pixels read from the panel,
//!            lcd::read_span, ~1.3 ms per block; CRC),
//!            then `BlockDevice::write_step` (spiflash.rs: compare, erase,
//!            16 page programs, read back; <= ~2 ms of CPU per pass, the
//!            erase and the programs run in the chip between passes); a
//!            cluster the idle-time preparation proved blank skips the
//!            compare (and needs no erase); a block whose read back fails is
//!            written once more with a forced erase;
//!   Fat      the chain in every FAT copy (FAT 1 first; read, link, then
//!            `write_block` from the next pass);
//!   Dir      the directory entry (read, check the slot is still free, then
//!            `write_block` from the next pass);
//!   Verify   one data block per pass re-read through the new FAT 1 chain,
//!            CRC-32 compared with the CRC of what was written.
//! Order data -> FAT -> directory as calbackup / fatren: a power loss leaves
//! at worst lost clusters, never an entry on unwritten or free clusters.
//! FAT and root blocks are written synchronously (`write_block`: compare,
//! erase, 16 page programs and read back in one call, <= ~0.45 s, watchdog
//! kicked in spiflash.rs `wait_ready`), never with `write_step`: a FAT or
//! root block must not be left erased or half programmed across a main-loop
//! pass, where a power-off, a reset (DBG:RESET / UPDATE / RECOVER) or a host
//! READ could see it (review 60605d2 H1). Only the data blocks, which no FAT
//! entry references until the Fat phase, are written in slices. Between two
//! `poll` slices the disk is therefore always consistent apart from lost
//! clusters (FAT 1 written, FAT 2 or the entry not yet), and
//! `finish_blocking` (power-off / reset paths) may stop a shot there.
//! Every block write is read back; nothing is written unless the boot sector
//! is the expected FAT16 volume.
//!
//! Frozen image, streamed from the panel: there is no frame buffer and no
//! capture buffer; each data block's pixels are read from the panel's GRAM
//! (RAMRD, lcd::read_span) when the block is filled, straight into the block
//! buffer (diskbuf::blocks()[0]) that is then programmed. So nothing may
//! draw over the panel until the last row is read: `frozen()` is true from
//! `start` until the last data block is filled (main.rs then passes
//! can_draw = false; App::gesture drops input), i.e. for the Start .. Data
//! phases. Frozen time per block ~18.5 ms (fill 1.3, 16 page programs ~14 at
//! tPP 0.7 ms, read back 2.7, compare 2.7 when not proven blank) plus a
//! ~45 ms sector erase when the cluster is not blank: ~1.4 s for 76 prepared
//! blocks, ~5 s unprepared (emulator with W25Q128 timing, emu/shottest.py;
//! the device took ~4-5 s before the preparation). The "idle-time
//! preparation" at the end of this file keeps the next shot's clusters
//! blank. The main loop
//! runs meanwhile (DMM, USB, the watchdog); only drawing and touch input
//! wait. The verify pass compares CRCs, so it does not need the panel again.
//! (Until 2026-10-22 the rows were first run-length coded into a 48 KiB
//! SRAM buffer, CAP, to unfreeze after ~0.1 s on the screens that fit (91 %
//! of the host renders; the AC+DC / power views did not); dropped for the
//! RAM, docs/ram-budget.md. The BMP bytes are the same: both read the same
//! GRAM through the same read_span.)
//!
//! The native build does not export this disk over USB (the PC sees the
//! read-only generated volume of vdisk.rs, rebuilt after a shot: dlog.rs
//! `set_stale`), so no host can write it or hold a stale FAT: shots are
//! never refused for a "mounted" disk. Builds that still export it (no
//! `vdisk`) refuse host writes while busy (NOT READY, msc.rs) and set
//! `MEDIUM_CHANGED` after a write (UNIT ATTENTION, msc.rs).

use core::sync::atomic::{AtomicBool, Ordering};

use crate::calbackup::{self, crc32_upd};
use crate::diskbuf;
use crate::fatdir;
use crate::fatren::{le16, parse_bpb, sfn_is, Vol};
use crate::lcd;
use crate::msc::{BlockDevice, Step};

/// Set after a screenshot changed the disk; for msc.rs to report UNIT
/// ATTENTION (sense 06/28/00) once on the next command, then clear.
pub static MEDIUM_CHANGED: AtomicBool = AtomicBool::new(false);

const W: u32 = lcd::W as u32;
const H: u32 = lcd::H as u32;
const ROW: u32 = W * 2;
const HDR_LEN: u32 = 66;
const FILE_LEN: u32 = HDR_LEN + ROW * H; // 307266
const BLK: usize = 4096;
/// Data blocks of the file (the last one holds 66 bytes).
const NBLK: u32 = FILE_LEN.div_ceil(BLK as u32);
/// Root slots per block.
const PER: u32 = (BLK / 32) as u32;
/// FAT16 entries per block.
const FPER: u32 = (BLK / 2) as u32;
/// Write / create date 2026-10-13, time 0 (no RTC), as calbackup.
const DATE: u16 = (46 << 9) | (10 << 5) | 13;
const NONE: u32 = u32::MAX;
/// Longest wait for a host WRITE(10) in progress before giving up (cycles,
/// 2 s): a host that died mid-transfer must not freeze the screen for good.
const START_CYC: u32 = crate::clock::HCLK_HZ * 2;

/// The stock header, byte for byte (S000.BMP at 0xD000 in the disk dump).
const HDR: [u8; HDR_LEN as usize] = [
    b'B', b'M', 0x42, 0xB0, 0x04, 0x00, // file size 307266
    0x00, 0x00, 0x00, 0x00, // reserved
    0x42, 0x00, 0x00, 0x00, // pixel data offset 66
    0x28, 0x00, 0x00, 0x00, // BITMAPINFOHEADER (40)
    0xE0, 0x01, 0x00, 0x00, // width 480
    0x40, 0x01, 0x00, 0x00, // height 320 (bottom-up)
    0x01, 0x00, 0x10, 0x00, // 1 plane, 16 bpp
    0x03, 0x00, 0x00, 0x00, // BI_BITFIELDS
    0x00, 0xB0, 0x04, 0x00, // image size 307200
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // ppm x / y
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // colours used / important
    0x00, 0xF8, 0x00, 0x00, 0xE0, 0x07, 0x00, 0x00, 0x1F, 0x00, 0x00, 0x00, // R G B masks
];
const _: () = assert!(FILE_LEN == 307_266 && NBLK == 76);
/// Blocks of the largest file (the screenshot; a scope record, scope_csv.rs,
/// is ~53 KB) and the cluster list size (1 cluster = 1 block on this disk;
/// a bigger cluster only needs fewer entries).
const MAX_NBLK: u32 = {
    let s = crate::scope_csv::MAX_LEN.div_ceil(BLK as u32);
    if s > NBLK { s } else { NBLK }
};
/// File kinds: the screenshot, a scope record CSV (UF-13).
const K_BMP: u8 = 0;
const K_CSV: u8 = 1;
/// Scope record bytes coded per pass (~2500 samples at 0.2 B/sample).
const CSV_PIECE: usize = 512;
const _: () = assert!(HDR_LEN % 2 == 0 && BLK % 2 == 0, "block edges fall between pixels");

/// Result of a screenshot.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Shot {
    /// Written and verified as S%03u.BMP.
    Saved(u16),
    /// No disk / not the expected FAT16 volume.
    NoDisk,
    /// Root directory or disk full, or S000..S999 all taken.
    Full,
    /// A read, write or the verify failed (at worst lost clusters).
    Error,
    /// Screenshot only: the disk is held by a fast-log session or an upload
    /// (unbounded): refused at once, nothing written. The screen is read
    /// from the panel while writing, so it cannot wait frozen that long.
    Busy,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Idle,
    Start,
    Bpb,
    Root,
    Alloc,
    Data,
    Fat,
    Dir,
    Verify,
}

const STEP0: Step = Step { started: false, phase: 0, page: 0, dirty: 0, erase: false, t0: 0 };

struct Job {
    phase: Phase,
    frozen: bool,
    /// A block write was started (the disk may have changed).
    wrote: bool,
    v: Option<Vol>,
    /// Phase cursor: root block, FAT entry, data / verify block.
    i: u32,
    /// Numbers 0..=999 taken by an S%03u.BMP entry (bit n).
    taken: [u32; 32],
    slot: u32,
    n: u16,
    /// Clusters of the new file, in chain order (ascending, first fit).
    cl: [u16; MAX_NBLK as usize],
    /// K_BMP / K_CSV, file length, data blocks.
    kind: u8,
    len: u32,
    nblk: u32,
    ncl: u32,
    got: u32,
    /// blocks()[0] holds the block being written (`lba`), step `st`.
    filled: bool,
    /// Scope record: bytes of the current data block filled so far (it is
    /// filled in CSV_PIECE pieces, one per pass).
    part: u32,
    lba: u32,
    st: Step,
    /// CRC-32 register of the bytes written / re-read.
    crc: u32,
    vcrc: u32,
    /// Fat phase: copy, block.
    fat_k: u32,
    fat_fb: u32,
    /// FAT 1 block cached in blocks()[1].
    cur: u32,
    /// Verify: current cluster, and its link from the previous one checked.
    vc: u32,
    linked: bool,
    /// `start` time (DWT cycles), for the Start timeout and `stats`.
    t0: u32,
    /// `stats`: `step` calls while frozen, data blocks that needed an erase.
    polls: u32,
    erases: u32,
    /// Clusters `cl[..pre]` are known blank (`prep_blank` at the end of
    /// Alloc): their blocks are programmed without the compare.
    pre: u32,
    /// The current data block's read back failed once and it is being
    /// written again with a forced erase (`data`).
    retried: bool,
}

/// The idle job (`init`; `start` sets the non-zero fields).
const JOB0: Job = Job {
    phase: Phase::Idle,
    frozen: false,
    wrote: false,
    v: None,
    i: 0,
    taken: [0; 32],
    slot: 0,
    n: 0,
    cl: [0; MAX_NBLK as usize],
    kind: K_BMP,
    len: FILE_LEN,
    nblk: NBLK,
    ncl: 0,
    got: 0,
    filled: false,
    part: 0,
    lba: 0,
    st: STEP0,
    crc: 0,
    vcrc: 0,
    fat_k: 0,
    fat_fb: 0,
    cur: 0,
    vc: 0,
    linked: false,
    t0: 0,
    polls: 0,
    erases: 0,
    pre: 0,
    retried: false,
};

/// Not TCM .bss (review 60605d2 B1: 1.7 KB of stack margin, and the target
/// LBAs / cluster list out of the stack-overflow path). In the backup SRAM
/// (.bkp_bss, link-native.x; CPU only, docs/native-boot.md "Extra RAM
/// areas"; zeroed at reset, not JOB0): `init` must run before anything reads
/// it (`busy`, `frozen`, `poll`, msc.rs).
#[cfg_attr(target_os = "none", link_section = ".bkp_bss")]
static mut JOB: Job = JOB0;
/// The last finished shot, not yet shown (`take_result`); the last scope
/// CSV (`take_csv_result`).
static mut RESULT: Option<Shot> = None;
static mut CSV_RESULT: Option<Shot> = None;
/// The last screenshot (`stats`, DBG:FLASH?): frozen ms, start .. result
/// ms, `step` calls while frozen, data blocks that needed an erase.
static mut STATS: [u32; 4] = [0; 4];
/// The device's own SpiFlash handle (created on the first shot).
static mut DISK: Option<crate::spiflash::SpiFlash> = None;

#[inline(always)]
fn job() -> &'static mut Job {
    unsafe { &mut *core::ptr::addr_of_mut!(JOB) }
}

fn cyc() -> u32 {
    unsafe { crate::mmio::r32(0xE000_1004) }
}

/// Reset the job state (JOB is not zeroed at reset, see there). main(),
/// before the main loop and the first `msc.poll`.
pub fn init() {
    *job() = JOB0;
    unsafe { *core::ptr::addr_of_mut!(RESULT) = None };
}

/// Request a screenshot of the screen as it is now (captured from the next
/// `poll` on; the caller draws nothing more while `frozen`). False: one is
/// already in flight (the request is ignored). (A PC can no longer have
/// this disk mounted: USB exports only the generated volume, vdisk.rs.)
pub fn start() -> bool {
    let j = job();
    if j.phase != Phase::Idle {
        return false;
    }
    *j = JOB0;
    j.slot = NONE;
    j.cur = NONE;
    j.crc = !0;
    j.vcrc = !0;
    j.phase = Phase::Start;
    j.frozen = true;
    j.t0 = cyc();
    true
}

/// Request a scope-record file (scope_csv.rs, set up by the caller first):
/// the next free SCOP%03u.DLG, written like a screenshot (no screen
/// capture, nothing frozen). Its result comes from `take_csv_result`, not
/// `take_result`. False: a disk job is in flight.
pub fn start_csv() -> bool {
    let j = job();
    if j.phase != Phase::Idle {
        return false;
    }
    let len = crate::scope_csv::len();
    if len == 0 || len > crate::scope_csv::MAX_LEN {
        return false;
    }
    *j = JOB0;
    j.kind = K_CSV;
    j.len = len;
    j.nblk = len.div_ceil(BLK as u32);
    j.slot = NONE;
    j.cur = NONE;
    j.crc = !0;
    j.vcrc = !0;
    j.phase = Phase::Start;
    j.t0 = cyc();
    true
}

/// The result of the last finished CSV, once.
pub fn take_csv_result() -> Option<Shot> {
    unsafe { (*core::ptr::addr_of_mut!(CSV_RESULT)).take() }
}

/// (prefix, extension) of the job's file name.
fn name_of(kind: u8) -> (&'static [u8], &'static [u8; 3]) {
    if kind == K_CSV { (b"SCOP", b"DLG") } else { (b"S", b"BMP") }
}

/// A screenshot is in flight (host disk writes are refused meanwhile).
#[inline(always)]
pub fn busy() -> bool {
    job().phase != Phase::Idle
}

/// Rows still to be read from the panel: draw nothing.
#[inline(always)]
pub fn frozen() -> bool {
    job().frozen
}

/// The last screenshot's [frozen ms, total ms, slices while frozen, data
/// blocks erased] (DBG:FLASH?; emu/shottest.py).
pub fn stats() -> [u32; 4] {
    unsafe { *core::ptr::addr_of!(STATS) }
}

fn ms_since(t0: u32) -> u32 {
    cyc().wrapping_sub(t0) / (crate::clock::HCLK_HZ / 1000)
}

/// The result of the last finished shot, once.
pub fn take_result() -> Option<Shot> {
    unsafe { (*core::ptr::addr_of_mut!(RESULT)).take() }
}

fn disk() -> &'static mut crate::spiflash::SpiFlash {
    let d = unsafe { &mut *core::ptr::addr_of_mut!(DISK) };
    d.get_or_insert_with(crate::spiflash::SpiFlash::new)
}

/// One bounded slice of the job in flight (main loop, every pass).
/// `inline(always)` (as `busy`, `frozen`, `job`):
/// this module is `.cold` (link-native.x, flash above 0x08080000 is not
/// zero-wait), so what runs on every pass is inlined into the hot caller and
/// only a shot in flight reaches the cold code.
#[inline(always)]
pub fn poll() {
    if !busy() {
        return;
    }
    step(disk());
}

/// Power-off / reset paths (app.rs `power_off`, main.rs DBG:RESET / UPDATE /
/// RECOVER): run the shot in flight to its end, at most `max_ms` (a whole
/// shot is ~5 s; the watchdog is kicked while the main loop feeds it), then
/// `abort_on` whatever is left. Nothing is drawn meanwhile; rows still to be
/// captured are read from the panel as it is.
pub fn finish_blocking(max_ms: u32) {
    if !busy() {
        return;
    }
    let max = max_ms.min(20_000) * (crate::clock::HCLK_HZ / 1000);
    let d = disk();
    let t0 = cyc();
    while busy() && cyc().wrapping_sub(t0) < max {
        step(d);
        crate::wdt::kick_if_feeding();
    }
    abort_on(d);
}

/// Stop a shot in flight between two slices (result Error). Safe at any
/// slice boundary: FAT / root blocks are only ever written whole, so the
/// disk holds at worst lost clusters (data written but not linked, or FAT 1
/// linked and FAT 2 / the entry not yet). Waits (bounded, spiflash.rs) for
/// an erase / page program of a data block the chip may still be running.
pub fn abort_on(disk: &mut dyn BlockDevice) {
    if !busy() {
        return;
    }
    // read_block waits for BUSY to clear first (spiflash.rs).
    let _ = disk.read_block(0, &mut diskbuf::blocks()[2]);
    finish(job(), Shot::Error);
}

/// A whole screenshot on any block device, synchronously (host test:
/// tools/fatren-test; the device uses start / poll).
#[allow(dead_code)]
pub fn take_on(disk: &mut dyn BlockDevice) -> Shot {
    if !start() {
        return Shot::Error;
    }
    loop {
        if let Some(r) = step(disk) {
            take_result();
            return r;
        }
    }
}

/// One slice on any block device (host tests: interleaved with csvlog).
#[allow(dead_code)]
pub fn poll_on(disk: &mut dyn BlockDevice) -> Option<Shot> {
    step(disk)
}

fn step(disk: &mut dyn BlockDevice) -> Option<Shot> {
    let j = job();
    if j.frozen {
        j.polls += 1;
    }
    let r = match j.phase {
        Phase::Idle => return None,
        Phase::Start => {
            if crate::msc::HOST_WRITING.load(Ordering::Relaxed) {
                (cyc().wrapping_sub(j.t0) > START_CYC).then_some(Shot::Error)
            } else if !disk.present() || disk.block_size() as usize != BLK {
                Some(Shot::NoDisk)
            } else {
                j.phase = Phase::Bpb;
                // A shot cannot wait frozen for the end of a fast-log
                // session or an upload (unbounded); a csvlog commit (a few
                // blocks) it waits for at Bpb, frozen.
                (j.kind == K_BMP && (crate::fastlog::holds_disk() || crate::upload::busy())).then_some(Shot::Busy)
            }
        }
        // A csvlog commit holds the disk (and diskbuf::blocks): wait. It
        // is bounded (a few blocks) and never starts while we are busy. An
        // update-file upload (upload.rs) likewise, until it ends / times out.
        // The fast logger (fastlog.rs) holds the disk for its whole session.
        Phase::Bpb if crate::msc::LOG_FLUSHING.load(Ordering::Relaxed) || crate::upload::busy() || crate::fastlog::holds_disk() => None,
        Phase::Bpb => j.bpb(disk),
        Phase::Root => j.root(disk),
        Phase::Alloc => j.alloc(disk),
        Phase::Data => j.data(disk),
        Phase::Fat => j.fat(disk),
        Phase::Dir => j.dir(disk),
        Phase::Verify => j.verify(disk),
    };
    if let Some(r) = r {
        finish(j, r);
    }
    r
}

fn finish(j: &mut Job, r: Shot) {
    j.phase = Phase::Idle;
    if j.frozen {
        j.frozen = false;
        unsafe { (*core::ptr::addr_of_mut!(STATS))[0] = ms_since(j.t0) };
    }
    if j.kind == K_CSV {
        unsafe { *core::ptr::addr_of_mut!(CSV_RESULT) = Some(r) };
    } else {
        unsafe {
            *core::ptr::addr_of_mut!(RESULT) = Some(r);
            let s = &mut *core::ptr::addr_of_mut!(STATS);
            s[1] = ms_since(j.t0);
            s[2] = j.polls;
            s[3] = j.erases;
        }
    }
    if j.wrote {
        MEDIUM_CHANGED.store(true, Ordering::Relaxed);
        // The generated USB volume shows the new file (dlog.rs / vdisk.rs
        // refresh, debounced in main.rs).
        #[cfg(feature = "vdisk")]
        crate::dlog::set_stale();
    }
}

impl Job {
    fn vol(&self) -> Vol {
        self.v.unwrap_or_else(|| unreachable!())
    }

    fn bpb(&mut self, disk: &mut dyn BlockDevice) -> Option<Shot> {
        let buf = &mut diskbuf::blocks()[0];
        if !disk.read_block(0, buf) {
            return Some(Shot::Error);
        }
        let Some(v) = parse_bpb(&buf[..], disk.block_count()) else {
            return Some(Shot::NoDisk);
        };
        // The file's directory (fatdir.rs; created at boot, here only if it
        // went missing: synchronous, ~1 s once). blocks()[1] (FAT cache) is
        // overwritten: `cur` is NONE until Alloc.
        let dir = if self.kind == K_CSV { &fatdir::SCOPE } else { &fatdir::SHOTS };
        let v = match fatdir::ensure(disk, &v, dir, diskbuf::blocks()) {
            Ok(d) => d,
            Err(fatdir::Err::Io) => return Some(Shot::Error),
            Err(_) => return Some(Shot::Full),
        };
        self.v = Some(v);
        self.cur = NONE;
        self.ncl = self.len.div_ceil(v.spc * BLK as u32);
        self.i = 0;
        self.phase = Phase::Root;
        None
    }

    /// One directory block (the root, then the file's directory, fatdir.rs
    /// `seq`): S%03u.BMP / SCOP%03u.DLG numbers in use in either, the first
    /// free slot of the directory.
    fn root(&mut self, disk: &mut dyn BlockDevice) -> Option<Shot> {
        let v = self.vol();
        let (lba, sub, base, lim) = fatdir::seq(&v, self.i);
        let buf = &mut diskbuf::blocks()[0];
        if !disk.read_block(lba, buf) {
            return Some(Shot::Error);
        }
        let mut end = false;
        for k in 0..PER {
            let idx = base + k;
            if idx >= lim {
                end = true;
                break;
            }
            let e = &buf[(k * 32) as usize..(k * 32 + 32) as usize];
            if e[0] == 0x00 || e[0] == 0xE5 {
                if sub && self.slot == NONE {
                    self.slot = idx;
                }
                if e[0] == 0x00 {
                    end = true; // end of directory
                    break;
                }
                continue;
            }
            let (pre, ext) = name_of(self.kind);
            let d = pre.len();
            if e[11] & 0x3F == 0x0F || !e[d..d + 3].iter().all(|c| c.is_ascii_digit()) {
                continue;
            }
            let n = (e[d] - b'0') as u16 * 100 + (e[d + 1] - b'0') as u16 * 10 + (e[d + 2] - b'0') as u16;
            if sfn_is(e, &calbackup::numbered_name(pre, n, ext)) {
                self.taken[(n / 32) as usize] |= 1 << (n % 32);
            }
        }
        self.i += 1;
        if end && !sub {
            self.i = v.root_blocks; // the root's end: on to the directory
        } else if end {
            self.i = fatdir::seq_blocks(&v);
        }
        if self.i < fatdir::seq_blocks(&v) {
            return None;
        }
        let Some(n) = (0..1000u16).find(|&n| self.taken[(n / 32) as usize] & (1 << (n % 32)) == 0) else {
            return Some(Shot::Full);
        };
        if self.slot == NONE || self.ncl as usize > self.cl.len() {
            return Some(Shot::Full);
        }
        self.n = n;
        self.i = 2;
        self.cur = NONE;
        self.phase = Phase::Alloc;
        None
    }

    /// FAT 1 entry `c` through the one-block cache in blocks()[1].
    fn fat_get(&mut self, disk: &mut dyn BlockDevice, c: u32) -> Option<u32> {
        let v = self.vol();
        let fat = &mut diskbuf::blocks()[1];
        let fb = c / FPER;
        if fb >= v.fat_blocks {
            return None;
        }
        if fb != self.cur {
            if !disk.read_block(v.fat_start + fb, fat) {
                self.cur = NONE;
                return None;
            }
            self.cur = fb;
        }
        Some(le16(fat, ((c % FPER) * 2) as usize))
    }

    /// Free clusters, first fit, from the entries of one FAT 1 block.
    fn alloc(&mut self, disk: &mut dyn BlockDevice) -> Option<Shot> {
        let v = self.vol();
        let fb = self.i / FPER;
        while self.got < self.ncl && self.i <= v.clusters + 1 && self.i / FPER == fb {
            match self.fat_get(disk, self.i) {
                None => return Some(Shot::Error),
                Some(0) => {
                    self.cl[self.got as usize] = self.i as u16;
                    self.got += 1;
                }
                Some(_) => {}
            }
            self.i += 1;
        }
        if self.got == self.ncl {
            self.i = 0;
            self.filled = false;
            self.pre = prep_blank().min(self.ncl);
            self.phase = Phase::Data;
        } else if self.i > v.clusters + 1 {
            return Some(Shot::Full);
        }
        None
    }

    /// File bytes `off .. off + buf.len()` (whole pixels): the header, then
    /// the pixels read from the panel (file row r = screen row H - 1 - r).
    fn fill(&self, off: u32, buf: &mut [u8]) {
        let mut i = 0usize;
        let mut off = off;
        while i < buf.len() && off < HDR_LEN {
            buf[i] = HDR[off as usize];
            i += 1;
            off += 1;
        }
        while i + 1 < buf.len() {
            let px = (off - HDR_LEN) / 2;
            let (r, x) = (px / W, px % W);
            let n = (W - x).min(((buf.len() - i) / 2) as u32);
            let put = |c: u16| {
                buf[i..i + 2].copy_from_slice(&c.to_le_bytes());
                i += 2;
            };
            lcd::read_span(x as u16, (H - 1 - r) as u16, n as u16, put);
            off += 2 * n;
        }
    }

    /// Fill (once) and write data block `i`.
    fn data(&mut self, disk: &mut dyn BlockDevice) -> Option<Shot> {
        let v = self.vol();
        if !self.filled {
            let off = self.i * BLK as u32;
            let len = (self.len - off).min(BLK as u32) as usize;
            let buf = &mut diskbuf::blocks()[0];
            if self.kind == K_CSV {
                // The coded record (scope_csv.rs) in pieces: ~1 ms of coding
                // per pass at most, not a whole 4 KiB block (~20000 samples
                // of a well-compressed record) in one. blocks()[0] is ours
                // across the passes (busy() holds the disk).
                let p = self.part as usize;
                let n = (len - p).min(CSV_PIECE);
                crate::scope_csv::fill(off + p as u32, &mut buf[p..p + n]);
                self.part += n as u32;
                if (self.part as usize) < len {
                    return None;
                }
                self.part = 0;
            } else {
                self.fill(off, &mut buf[..len]);
            }
            buf[len..].fill(0xFF);
            self.crc = crc32_upd(self.crc, &buf[..len]);
            let c = self.cl[(self.i / v.spc) as usize] as u32;
            self.lba = v.data_start + (c - 2) * v.spc + self.i % v.spc;
            self.filled = true;
            self.wrote = true;
            // A cluster the preparation found blank (nothing written since):
            // straight to the page programs (write_step phase 1, as after an
            // erase), no compare. Should it not be blank after all, the read
            // back fails and the retry below compares and erases.
            self.st = if self.i / v.spc < self.pre { Step { phase: 1, ..STEP0 } } else { STEP0 };
            if self.i + 1 == self.nblk && self.frozen {
                self.frozen = false; // every row has been read
                unsafe { (*core::ptr::addr_of_mut!(STATS))[0] = ms_since(self.t0) };
            }
        }
        let buf = &diskbuf::blocks()[0];
        match disk.write_step(self.lba, buf, &mut self.st)? {
            // A block that read blank but did not take the program (a
            // sector whose erase a power cut interrupted, `prep_step`):
            // once more, erased first. blocks()[0] still holds the block.
            false if !self.retried => {
                self.retried = true;
                self.st = Step { erase: true, ..STEP0 };
                None
            }
            false => Some(Shot::Error),
            true => {
                self.filled = false;
                self.retried = false;
                self.erases += self.st.erase as u32;
                self.i += 1;
                if self.i == self.nblk {
                    self.fat_k = 0;
                    self.fat_fb = self.cl[0] as u32 / FPER;
                    self.phase = Phase::Fat;
                }
                None
            }
        }
    }

    /// Link the chain in FAT copy `fat_k`, block `fat_fb` (blocks between
    /// the first and the last cluster's that hold one of ours).
    fn fat(&mut self, disk: &mut dyn BlockDevice) -> Option<Shot> {
        let v = self.vol();
        let last = self.cl[self.ncl as usize - 1] as u32 / FPER;
        if !self.filled {
            let all = self.cl;
            let cl = &all[..self.ncl as usize];
            while self.fat_fb <= last && !cl.iter().any(|&c| c as u32 / FPER == self.fat_fb) {
                self.fat_fb += 1;
            }
            self.lba = v.fat_start + self.fat_k * v.fat_blocks + self.fat_fb;
            let buf = &mut diskbuf::blocks()[0];
            if !disk.read_block(self.lba, buf) {
                return Some(Shot::Error);
            }
            for (i, &c) in cl.iter().enumerate() {
                let c = c as u32;
                if c / FPER != self.fat_fb {
                    continue;
                }
                let o = ((c % FPER) * 2) as usize;
                if self.fat_k == 0 && le16(&buf[..], o) != 0 {
                    return Some(Shot::Error); // taken since the scan
                }
                let next = cl.get(i + 1).copied().unwrap_or(0xFFFF);
                buf[o..o + 2].copy_from_slice(&next.to_le_bytes());
            }
            self.filled = true;
            return None; // the write from the next pass (one block read per pass)
        }
        // Whole block in one call (H1: never left erased across a pass).
        let buf = &diskbuf::blocks()[0];
        match disk.write_block(self.lba, &buf[..]) {
            false => Some(Shot::Error),
            true => {
                self.filled = false;
                self.fat_fb += 1;
                if self.fat_fb > last {
                    self.fat_k += 1;
                    self.fat_fb = self.cl[0] as u32 / FPER;
                    if self.fat_k == v.nfats {
                        self.phase = Phase::Dir;
                    }
                }
                None
            }
        }
    }

    /// The root entry, in the slot found free by the scan.
    fn dir(&mut self, disk: &mut dyn BlockDevice) -> Option<Shot> {
        let v = self.vol();
        if !self.filled {
            self.lba = v.dlba(self.slot / PER);
            let buf = &mut diskbuf::blocks()[0];
            if !disk.read_block(self.lba, buf) {
                return Some(Shot::Error);
            }
            let o = ((self.slot % PER) * 32) as usize;
            let e = &mut buf[o..o + 32];
            if e[0] != 0x00 && e[0] != 0xE5 {
                return Some(Shot::Error); // taken since the scan
            }
            e.fill(0);
            let (pre, ext) = name_of(self.kind);
            e[..11].copy_from_slice(&calbackup::numbered_name(pre, self.n, ext));
            e[11] = 0x20; // archive
            e[0x10..0x12].copy_from_slice(&DATE.to_le_bytes()); // created
            e[0x12..0x14].copy_from_slice(&DATE.to_le_bytes()); // accessed
            e[0x18..0x1A].copy_from_slice(&DATE.to_le_bytes()); // written (time 0)
            e[0x1A..0x1C].copy_from_slice(&self.cl[0].to_le_bytes());
            e[0x1C..0x20].copy_from_slice(&self.len.to_le_bytes());
            self.filled = true;
            return None; // the write from the next pass
        }
        // Whole block in one call (H1: never left erased across a pass).
        let buf = &diskbuf::blocks()[0];
        match disk.write_block(self.lba, &buf[..]) {
            false => Some(Shot::Error),
            true => {
                self.filled = false;
                self.i = 0;
                self.vc = self.cl[0] as u32;
                self.cur = NONE; // FAT 1 changed
                self.phase = Phase::Verify;
                None
            }
        }
    }

    /// Re-read block `i` through the new FAT 1 chain; CRC at the end.
    fn verify(&mut self, disk: &mut dyn BlockDevice) -> Option<Shot> {
        let v = self.vol();
        if self.i > 0 && self.i % v.spc == 0 && !self.linked {
            let want = self.cl[(self.i / v.spc) as usize] as u32;
            let miss = self.vc / FPER != self.cur;
            if self.fat_get(disk, self.vc) != Some(want) {
                return Some(Shot::Error);
            }
            self.vc = want;
            self.linked = true;
            if miss {
                return None; // a FAT block was read: the data block next pass
            }
        }
        self.linked = false;
        let off = self.i * BLK as u32;
        let len = (self.len - off).min(BLK as u32) as usize;
        let buf = &mut diskbuf::blocks()[2];
        if !disk.read_block(v.data_start + (self.vc - 2) * v.spc + self.i % v.spc, buf) {
            return Some(Shot::Error);
        }
        self.vcrc = crc32_upd(self.vcrc, &buf[..len]);
        self.i += 1;
        if self.i < self.nblk {
            return None;
        }
        let eoc = self.fat_get(disk, self.vc).is_some_and(|n| n >= 0xFFF8);
        Some(if eoc && self.vcrc == self.crc { Shot::Saved(self.n) } else { Shot::Error })
    }
}

// ------------------------------------------------- idle-time preparation
//
// The next shot's clusters erased ahead of time (docs/ram-budget.md "The
// screenshot: streamed from the panel"; the owner's option 3). A shot takes
// the first free clusters of FAT 1 (first fit, `alloc`), and `write_step`
// skips the erase of a block that is already blank, so keeping the first
// `ncl` free clusters blank takes the 76 x ~45 ms of sector erases out of the
// frozen time. Nothing is reserved or linked: the prepared clusters are
// ordinary free clusters that happen to read 0xFF. Consequences:
//   * FAT / directory never written: a power cut mid-prepare leaves at worst
//     a half-erased free sector (no lost clusters, nothing for fatren /
//     fsck); the USB volume does not show free clusters (vdisk.rs; the raw
//     DEBUG/SPIFLASH.BIN shows them as they are, 0xFF);
//   * any other writer may take them (a csvlog commit, a fast-log session,
//     a firmware file staged from the PC): it then writes faster, and the
//     preparation tops the set up again afterwards;
//   * purely a speed-up: the shot's `write_step` still compares every block
//     and erases what is not blank (a cluster taken / freed meanwhile, a
//     sector whose erase a power cut interrupted), and a block whose read
//     back fails is written once more with a forced erase (`data`).
// Steps (`prep_poll`, main loop, at most ~1 ms of SPI each): boot sector;
// FAT 1 entries in 64-entry reads up to the next free cluster; that
// cluster's blocks checked blank 1 KiB per step; a dirty block erased (the
// FAT entry re-read in the same step first: still free) and, once the chip
// is idle, checked again. Ready after `ncl` blank free clusters (fewer on a
// nearly full disk: then all the free ones). Any program / erase by another
// writer (msc::FLASH_WRITES moved since our last step) restarts the scan,
// once the disk has been quiet for PREP_QUIET_MS. The caller (main.rs) runs
// it only when nothing else may be disturbed: no disk job in flight (shot,
// csvlog commit, fast-log session, upload / staging: their data sits in
// free clusters until linked), no AD2 stream (this module is region C),
// no logger session, no host USB disk I/O in the last ~0.5 s.

use core::sync::atomic::AtomicU32;

/// No flash write by another writer for this long before (re)starting.
const PREP_QUIET_MS: u64 = 2000;
/// `Off` (no disk, a read failed, an erase timed out) is retried after this.
const PREP_RETRY_MS: u64 = 60_000;
/// FAT entries read per step.
const PREP_FAT_RD: usize = 64;
/// Blank check: bytes read per step (~0.7 ms at 12.5 MHz).
const PREP_CHK: u32 = 1024;
/// Longest wait for one of our erases (cycles, 1 s; tSE max 400 ms).
const PREP_OP_CYC: u32 = crate::clock::HCLK_HZ;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Pp {
    /// Waiting for the quiet time; then the boot sector.
    Start,
    /// FAT 1 from cluster `c` on.
    Scan,
    /// Blank check of cluster `c`, block `b`, from byte `off`.
    Check,
    /// Our erase of that block was started (`Check` again once the chip
    /// is idle).
    Erase,
    /// `found` clusters are blank (all a shot needs, or every free one).
    Ready,
    /// No usable disk / a read failed / an erase timed out: idle until
    /// another writer changes the disk, or PREP_RETRY_MS.
    Off,
}

struct Prep {
    st: Pp,
    /// msc::FLASH_WRITES after our last step / when last seen moving.
    gen: u32,
    t_gen: u64,
    fat_start: u32,
    data_start: u32,
    spc: u32,
    last: u32,
    /// Clusters a screenshot takes.
    ncl: u32,
    c: u32,
    b: u32,
    off: u32,
    /// Blank free clusters found so far (in first-fit order).
    found: u32,
    /// The block in `Check` has been erased by us once already.
    erased: bool,
    t_op: u32,
}

const PREP0: Prep = Prep {
    st: Pp::Start,
    gen: 0,
    t_gen: 0,
    fat_start: 0,
    data_start: 0,
    spc: 1,
    last: 0,
    ncl: 0,
    c: 2,
    b: 0,
    off: 0,
    found: 0,
    erased: false,
    t_op: 0,
};
static mut PREP: Prep = PREP0;
/// Sector erases the preparation started since boot (DBG:FLASH?).
static PREP_ERASES: AtomicU32 = AtomicU32::new(0);

#[inline(always)]
fn prep() -> &'static mut Prep {
    unsafe { &mut *core::ptr::addr_of_mut!(PREP) }
}

/// Free clusters known blank right now: the first `found` of FAT 1 in first-
/// fit order (= the ones a shot's `alloc` takes first) while the preparation
/// is Ready and nothing has written the flash since its last step; else 0.
fn prep_blank() -> u32 {
    let p = prep();
    if p.st == Pp::Ready && p.gen == crate::msc::FLASH_WRITES.load(Ordering::Relaxed) {
        p.found
    } else {
        0
    }
}

/// (state: 0 waiting / 1 working / 2 ready / 3 off, blank clusters found,
/// erases since boot) for DBG:FLASH?.
pub fn prep_stats() -> [u32; 3] {
    let p = prep();
    let st = match p.st {
        Pp::Start => 0,
        Pp::Scan | Pp::Check | Pp::Erase => 1,
        Pp::Ready => 2,
        Pp::Off => 3,
    };
    [st, p.found, PREP_ERASES.load(Ordering::Relaxed)]
}

/// Main loop, every pass (hot: inlined, region B; only a pass with work to
/// do reaches the cold step). `ok` = the caller's conditions (see above).
#[cfg(target_os = "none")]
#[inline(always)]
pub fn prep_poll(now: u64, ok: bool) {
    if prep_due(now, ok) {
        prep_step(disk());
        prep().gen = crate::msc::FLASH_WRITES.load(Ordering::Relaxed); // our own erase
    }
}

/// `prep_poll`'s bookkeeping: a step is due now.
#[inline(always)]
fn prep_due(now: u64, ok: bool) -> bool {
    let p = prep();
    let g = crate::msc::FLASH_WRITES.load(Ordering::Relaxed);
    if g != p.gen {
        // Another writer: start over once it has been quiet.
        p.gen = g;
        p.t_gen = now;
        p.st = Pp::Start;
    } else if p.st == Pp::Off && now.saturating_sub(p.t_gen) >= PREP_RETRY_MS {
        p.t_gen = now.saturating_sub(PREP_QUIET_MS);
        p.st = Pp::Start;
    }
    !matches!(p.st, Pp::Ready | Pp::Off) && ok && !busy() && now.saturating_sub(p.t_gen) >= PREP_QUIET_MS
}

/// Power-off / reset paths: let an erase of ours finish (<= ~0.4 s) so the
/// power never cuts it (harmless anyway: a free sector).
#[cfg(target_os = "none")]
pub fn prep_finish() {
    if prep().st == Pp::Erase {
        let _ = crate::fastenc::Nor::read(disk(), 0, &mut [0u8; 1]); // waits for BUSY
        prep().st = Pp::Start;
    }
}

/// One preparation step on any disk (host test: tools/fatren-test "prep";
/// the device uses `prep_poll`). The caller keeps `FLASH_WRITES` (host:
/// `prep_on` below does it).
#[inline(never)]
fn prep_step(d: &mut dyn crate::fastlog::Disk) {
    let p = prep();
    let blk = BLK as u32;
    // An erase (ours, or another writer's) still running: never wait for it
    // here (every read below would, spiflash.rs `read_at`).
    if d.nor().busy() {
        if p.st == Pp::Erase && cyc().wrapping_sub(p.t_op) > PREP_OP_CYC {
            p.st = Pp::Off;
        }
        return;
    }
    match p.st {
        Pp::Ready | Pp::Off => {}
        Pp::Start => {
            let mut b = [0u8; 512];
            let n = d.bd().block_count();
            let v = if d.bd().present() && d.bd().block_size() == blk && d.nor().read(0, &mut b) { parse_bpb(&b, n) } else { None };
            let Some(v) = v else {
                p.st = Pp::Off;
                return;
            };
            p.fat_start = v.fat_start;
            p.data_start = v.data_start;
            p.spc = v.spc;
            p.last = v.clusters + 1;
            p.ncl = FILE_LEN.div_ceil(v.spc * blk);
            p.c = 2;
            p.found = 0;
            p.st = Pp::Scan;
        }
        Pp::Scan => {
            if p.found >= p.ncl || p.c > p.last {
                p.st = Pp::Ready;
                return;
            }
            let mut e = [0u8; 2 * PREP_FAT_RD];
            let n = ((p.last + 1 - p.c) as usize).min(PREP_FAT_RD);
            if !d.nor().read(p.fat_start * blk + p.c * 2, &mut e[..2 * n]) {
                p.st = Pp::Off;
                return;
            }
            for k in 0..n {
                if le16(&e, 2 * k) == 0 {
                    p.b = 0;
                    p.off = 0;
                    p.erased = false;
                    p.st = Pp::Check;
                    return;
                }
                p.c += 1;
            }
        }
        Pp::Check => {
            let a = (p.data_start + (p.c - 2) * p.spc + p.b) * blk;
            let mut buf = [0u8; 256];
            let end = p.off + PREP_CHK;
            while p.off < end {
                if !d.nor().read(a + p.off, &mut buf) {
                    p.st = Pp::Off;
                    return;
                }
                if buf.iter().any(|&x| x != 0xFF) {
                    // Still free? (FAT 1 re-read in this step: nothing ran
                    // since, so the erase can only hit a free cluster.)
                    let mut e = [0u8; 2];
                    if !d.nor().read(p.fat_start * blk + p.c * 2, &mut e) {
                        p.st = Pp::Off;
                        return;
                    }
                    if le16(&e, 0) != 0 {
                        p.st = Pp::Start;
                        return;
                    }
                    if p.erased {
                        // Erased and still not blank: skip the cluster (a
                        // shot that takes it erases it again itself).
                        p.c += 1;
                        p.st = Pp::Scan;
                        return;
                    }
                    d.nor().erase(a, false);
                    PREP_ERASES.fetch_add(1, Ordering::Relaxed);
                    p.t_op = cyc();
                    p.st = Pp::Erase;
                    return;
                }
                p.off += buf.len() as u32;
            }
            if p.off < blk {
                return;
            }
            p.off = 0;
            p.erased = false;
            p.b += 1;
            if p.b == p.spc {
                p.found += 1;
                p.c += 1;
                p.st = Pp::Scan;
            }
        }
        Pp::Erase => {
            // Done: check the block again.
            p.off = 0;
            p.erased = true;
            p.st = Pp::Check;
        }
    }
}

/// Host test: back to the boot state.
#[allow(dead_code)]
pub fn prep_reset() {
    *prep() = PREP0;
}

/// Host test: one step, with the hot wrapper's bookkeeping (`now` in ms;
/// `ok` as main.rs passes it). Returns the state code of `prep_stats`.
#[allow(dead_code)]
pub fn prep_on(d: &mut dyn crate::fastlog::Disk, now: u64, ok: bool) -> u32 {
    if prep_due(now, ok) {
        prep_step(d);
        prep().gen = crate::msc::FLASH_WRITES.load(Ordering::Relaxed);
    }
    prep_stats()[0]
}
