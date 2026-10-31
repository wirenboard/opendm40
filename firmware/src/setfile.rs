//! The settings' copy on the SPI disk: `_SETTING.BIN` in the root of the
//! FAT16 volume, next to `_DM40PRV.BIN`. The live copy stays in the backup
//! SRAM (persist.rs); that one is lost when the meter is switched off, this
//! one is what the next power-on starts from.
//!
//! Not the stock's config.bin: a unit taken back to the stock firmware finds
//! its own settings as it left them. The USB volume is generated (vdisk.rs)
//! and lists only the LOGS / SHOTS / SCOPE files and the INFO tables, so no
//! host ever sees this file or has the real FAT mounted while it is written.
//! The vendor FS-loader's USB disk shows the real root, this file included.
//!
//! File: `SLOTS` slots of one 4 KiB block each (one block = one erase sector
//! of the W25Q128, spiflash.rs), created once at boot (all 0xFF) before USB
//! exists and never resized or moved afterwards: a save rewrites one slot's
//! block in place, no FAT or directory block. Slot layout (little-endian
//! words): magic "DSET", persist version, sequence number, payload words n
//! (= persist::words_of(version)), the n payload words (the backup SRAM
//! record's), CRC-32 over all of it; the rest of the block 0xFF.
//!
//! Atomicity: a save writes the slot after the newest one (with 2 slots: the
//! older one) with sequence + 1; a load takes the valid slot with the
//! highest sequence (wrapping compare). A power cut during a save can only
//! tear the slot being written (erase + 16 page programs, then read back):
//! its CRC fails and the other slot, untouched, is the newest valid one.
//!
//! When (`poll`, main.rs every pass, cheap: three backup SRAM reads every
//! 0.5 s): the backup SRAM record (rewritten every 2 s by App::persist)
//! differs from the newest slot and has not changed for `DEBOUNCE_MS`.
//! Flash wear: `poll` saves come out of a budget of `BUDGET` that refills by
//! one every `REFILL_MS` (a setting that changed by itself every few seconds
//! would cost at most one sector erase per 3 minutes after the first 20;
//! a user changing settings never meets the limit). Deferred while the disk is held
//! (a screenshot, a CSV commit, a fast-log session, an update upload, a host
//! WRITE), during an AD1 logger session (main.rs: a 45..400 ms stall drops
//! rows) and while the AD2 stream ISR runs (the writer is region C code).
//! `flush` before every power-off (app.rs power_off: the key, the idle
//! auto-off, the 2-finger "Power off?" confirm) writes at once, gates off.
//! A record equal to the newest slot (version and CRC) is not written.
//!
//! Boot (`boot` from native.rs pre_main, `pick` from App::new): a valid
//! backup SRAM record is used, unless the newest file slot has a higher
//! sequence than persist::seq (the file sequence the backup SRAM copy
//! continues); an invalid backup SRAM record (power-on) loads the newest
//! valid slot; neither: the defaults. The record goes through the same
//! persist::words_of table either way (an older version zero-extended).
//!
//! DBG:BOOT? `SETTINGS,<BKP|FILE|DEFAULTS>,<slot|->,<seq>,SETFILE,<state>,
//! SAVE,<status>,<saves>,<seq>` (native.rs put_extra).
//!
//! Host test: tools/fatren-test/run.sh ("setfile").

#![cfg_attr(not(target_os = "none"), allow(dead_code))]

use crate::calbackup::{chain_of, crc32, write_file, Source};
use crate::diskbuf::blocks;
use crate::msc::BlockDevice;
use crate::persist::{self, Record, WORDS};

pub const NAME: [u8; 11] = *b"_SETTINGBIN";
pub const SLOTS: usize = 2;
const BLK: usize = 4096;
pub const SIZE: u32 = (SLOTS * BLK) as u32;
const MAGIC: u32 = 0x5445_5344; // "DSET"
/// Header words before the payload: magic, version, seq, n.
const HDR: usize = 4;
/// Quiet time after the last change before a save.
pub const DEBOUNCE_MS: u64 = 5_000;
/// Saves `poll` may make in a burst, and the time one more takes to come
/// back (`flush` is not counted).
pub const BUDGET: u8 = 20;
pub const REFILL_MS: u64 = 180_000;
const CHECK_MS: u64 = 500;
const _: () = assert!(4 * (HDR + WORDS + 1) <= BLK);

// --- The slot format (pure; host-tested) ---------------------------------------

fn get(b: &[u8], i: usize) -> u32 {
    u32::from_le_bytes([b[4 * i], b[4 * i + 1], b[4 * i + 2], b[4 * i + 3]])
}

fn put(b: &mut [u8], i: usize, v: u32) {
    b[4 * i..4 * i + 4].copy_from_slice(&v.to_le_bytes());
}

/// Fill a 4 KiB slot block: header, `n` payload words `word(0..n)`, CRC,
/// 0xFF.
pub fn encode(b: &mut [u8], version: u32, seq: u32, n: usize, word: impl Fn(usize) -> u32) {
    b[..BLK].fill(0xFF);
    put(b, 0, MAGIC);
    put(b, 1, version);
    put(b, 2, seq);
    put(b, 3, n as u32);
    for i in 0..n {
        put(b, HDR + i, word(i));
    }
    let c = crc32(&b[..4 * (HDR + n)]);
    put(b, HDR + n, c);
}

/// A valid slot: (version, seq, payload words, payload CRC = persist::crc of
/// the words, as the backup SRAM record stores it). None: no magic, a
/// version this build does not know, a word count that does not match it,
/// a CRC failure (a torn write, never written).
pub fn decode(b: &[u8]) -> Option<(u32, u32, usize, u32)> {
    if get(b, 0) != MAGIC {
        return None;
    }
    let version = get(b, 1);
    let n = persist::words_of(version)?;
    if get(b, 3) as usize != n || crc32(&b[..4 * (HDR + n)]) != get(b, HDR + n) {
        return None;
    }
    Some((version, get(b, 2), n, crc32(&b[4 * HDR..4 * (HDR + n)])))
}

/// `a` is newer than `b` (sequence numbers, wrapping).
pub fn newer(a: u32, b: u32) -> bool {
    (a.wrapping_sub(b) as i32) > 0
}

/// Index of the newest valid slot (`seqs[i]` = slot i's sequence, None =
/// invalid).
pub fn newest(seqs: &[Option<u32>]) -> Option<usize> {
    let mut best: Option<(usize, u32)> = None;
    for (i, s) in seqs.iter().enumerate() {
        if let Some(s) = *s {
            if best.is_none_or(|(_, b)| newer(s, b)) {
                best = Some((i, s));
            }
        }
    }
    best.map(|b| b.0)
}

/// The record in a valid slot block, zero-extended to `WORDS`.
pub fn record(b: &[u8]) -> Option<(Record, u32)> {
    let (version, seq, n, _) = decode(b)?;
    let mut words = [0u32; WORDS];
    for (i, w) in words[..n].iter_mut().enumerate() {
        *w = get(b, HDR + i);
    }
    Some((Record { version, words }, seq))
}

// --- State ---------------------------------------------------------------------

/// The file at boot (DBG:BOOT? SETFILE).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum FileState {
    NotRun = 0,
    /// Found with the expected size and geometry.
    Present,
    /// Made at this boot (absent or the wrong size before).
    Created,
    /// No medium / volume, no room, or a write failed: no file copy.
    NoFile,
    /// Found or made, but its blocks are not `SLOTS` single-block clusters.
    Error,
}

/// Last save (DBG:BOOT? SAVE).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum Save {
    None = 0,
    Ok,
    /// Waiting for the disk (a screenshot, a log, ...).
    Deferred,
    /// The backup SRAM record did not check out: nothing written.
    BadRecord,
    /// The write or its read back failed.
    Error,
    /// No file (FileState NoFile / Error).
    NoFile,
}

/// Where the settings of this boot came from (DBG:BOOT? SETTINGS).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum From {
    NotRun = 0,
    Bkp,
    File,
    Defaults,
}

struct St {
    file: u8,
    from: u8,
    save: u8,
    /// Slot of `from` = File.
    from_slot: u8,
    from_seq: u32,
    /// Each slot's block (valid with FileState Present / Created).
    lba: [u32; SLOTS],
    /// Each slot's sequence, valid slots only.
    seq: [Option<u32>; SLOTS],
    /// (version, payload CRC) of the newest slot; (0, 0) = none.
    saved: (u32, u32),
    /// The backup SRAM record last seen by `poll`, and since when.
    seen: (u32, u32),
    changed: u64,
    next_check: u64,
    /// `poll` saves left (BUDGET at boot), the next one back at `refill`.
    tokens: u8,
    refill: u64,
    saves: u32,
}

// Zero-initialised (.sram_bss: kept out of the TCM stack's memory).
#[cfg_attr(target_os = "none", link_section = ".sram_bss")]
static mut ST: St = St {
    file: 0,
    from: 0,
    save: 0,
    from_slot: 0,
    from_seq: 0,
    lba: [0; SLOTS],
    seq: [None; SLOTS],
    saved: (0, 0),
    seen: (0, 0),
    changed: 0,
    next_check: 0,
    tokens: 0,
    refill: 0,
    saves: 0,
};

#[inline(always)]
fn st() -> &'static mut St {
    unsafe { &mut *core::ptr::addr_of_mut!(ST) }
}

#[inline(always)]
fn file_ok() -> bool {
    matches!(st().file, x if x == FileState::Present as u8 || x == FileState::Created as u8)
}

/// (from, slot, seq, file state, last save, saves, newest slot's seq).
pub fn info() -> (From, u8, u32, FileState, Save, u32, u32) {
    let s = st();
    let from = [From::NotRun, From::Bkp, From::File, From::Defaults][s.from as usize & 3];
    let file = [FileState::NotRun, FileState::Present, FileState::Created, FileState::NoFile, FileState::Error][(s.file as usize).min(4)];
    let save = [Save::None, Save::Ok, Save::Deferred, Save::BadRecord, Save::Error, Save::NoFile][(s.save as usize).min(5)];
    let top = newest(&s.seq).and_then(|k| s.seq[k]).unwrap_or(0);
    (from, s.from_slot, s.from_seq, file, save, s.saves, top)
}

// --- Boot ------------------------------------------------------------------------

/// The new file's contents: all 0xFF (no valid slot).
struct Blank;
impl Source for Blank {
    fn size(&self) -> u32 {
        SIZE
    }
    fn fill(&self, _: u32, buf: &mut [u8]) {
        buf.fill(0xFF);
    }
}

/// The slots' blocks from the file's directory entry and chain.
fn locate(dev: &mut dyn BlockDevice) -> Option<bool> {
    let mut cl = [0u16; SLOTS];
    let (data, spc, size, n) = chain_of(dev, &NAME, &mut cl)?;
    if size != SIZE {
        return None;
    }
    // One erase sector per slot, each its own: one 4 KiB block per cluster
    // (this volume: mkfs -s 1); else no file copy.
    if spc != 1 || n != SLOTS {
        return Some(false);
    }
    for (l, &c) in st().lba.iter_mut().zip(cl.iter()) {
        *l = data + c as u32 - 2;
    }
    Some(true)
}

/// Read every slot's header (sequence, newest record's version / CRC).
fn scan(dev: &mut dyn BlockDevice) {
    let s = st();
    let [b, _, _] = blocks();
    s.saved = (0, 0);
    for k in 0..SLOTS {
        s.seq[k] = None;
        if dev.read_block(s.lba[k], b) {
            if let Some((v, q, _, c)) = decode(&b[..]) {
                s.seq[k] = Some(q);
                if newest(&s.seq) == Some(k) {
                    s.saved = (v, c);
                }
            }
        }
    }
    s.seen = s.saved;
}

/// Boot (native.rs pre_main, after fatdir; before USB): find the file or
/// make it, read the slots. A file made now gets the backup SRAM record at
/// once when it is valid (a unit updated to this build keeps its settings
/// through the first power-off even before the first timed save).
#[inline(never)]
pub fn boot(dev: &mut dyn BlockDevice) {
    #[cfg(target_os = "none")]
    persist::init();
    let s = st();
    s.seq = [None; SLOTS];
    s.saved = (0, 0);
    (s.next_check, s.tokens, s.refill, s.saves, s.save) = (0, BUDGET, 0, 0, Save::None as u8);
    s.file = match locate(dev) {
        Some(true) => FileState::Present,
        Some(false) => FileState::Error,
        None => {
            let r = write_file(dev, NAME, &Blank);
            match (r.status, locate(dev)) {
                (crate::calbackup::Status::Written, Some(true)) => FileState::Created,
                (_, Some(false)) => FileState::Error,
                _ => FileState::NoFile,
            }
        }
    } as u8;
    if !file_ok() {
        return;
    }
    scan(dev);
    if s.file == FileState::Created as u8 && persist::stored().is_some() {
        write_slot(dev);
    }
}

/// The record to start from (App::new), given the backup SRAM's: see the
/// module doc. Records the source; the backup SRAM copy then continues the
/// chosen sequence (persist::set_seq).
#[inline(never)]
pub fn pick(bkp: Option<Record>) -> Option<Record> {
    let s = st();
    let top = newest(&s.seq).filter(|_| file_ok());
    let file_first = match (bkp.is_some(), top) {
        (_, None) => false,
        (false, Some(_)) => true,
        (true, Some(k)) => persist::seq().is_some_and(|b| newer(s.seq[k].unwrap_or(0), b)),
    };
    if file_first {
        let k = top.unwrap_or(0);
        let mut dev = new_dev();
        let [b, _, _] = blocks();
        if dev.read_block(s.lba[k], b) {
            if let Some((r, q)) = record(&b[..]) {
                s.from = From::File as u8;
                s.from_slot = k as u8;
                s.from_seq = q;
                persist::set_seq(q);
                return Some(r);
            }
        }
    }
    s.from_slot = 0;
    if bkp.is_some() {
        s.from = From::Bkp as u8;
        let q = persist::seq();
        s.from_seq = q.unwrap_or(0);
        // A record without the sequence words (an older build) continues
        // the file's newest.
        if q.is_none() {
            persist::set_seq(top.and_then(|k| s.seq[k]).unwrap_or(0));
        }
    } else {
        s.from = From::Defaults as u8;
        s.from_seq = 0;
        persist::set_seq(top.and_then(|k| s.seq[k]).unwrap_or(0));
    }
    bkp
}

// --- Saves ---------------------------------------------------------------------

#[cfg(target_os = "none")]
fn new_dev() -> crate::spiflash::SpiFlash {
    crate::spiflash::SpiFlash::new()
}

#[cfg(not(target_os = "none"))]
fn new_dev() -> HostDev {
    HostDev
}

/// Host test: forwards to `HOST_DEV`.
#[cfg(not(target_os = "none"))]
struct HostDev;
#[cfg(not(target_os = "none"))]
impl HostDev {
    fn d(&self) -> &'static mut dyn BlockDevice {
        unsafe { (*core::ptr::addr_of_mut!(HOST_DEV)).as_deref_mut().expect("setfile: host device") }
    }
}
#[cfg(not(target_os = "none"))]
impl BlockDevice for HostDev {
    fn present(&self) -> bool {
        self.d().present()
    }
    fn block_size(&self) -> u32 {
        self.d().block_size()
    }
    fn block_count(&self) -> u32 {
        self.d().block_count()
    }
    fn read_block(&mut self, lba: u32, buf: &mut [u8]) -> bool {
        self.d().read_block(lba, buf)
    }
    fn write_block(&mut self, lba: u32, buf: &[u8]) -> bool {
        self.d().write_block(lba, buf)
    }
    fn write_step(&mut self, lba: u32, buf: &dyn crate::wordram::BlockSrc, st: &mut crate::msc::Step) -> Option<bool> {
        self.d().write_step(lba, buf, st)
    }
}

/// Host test: the device `pick` / `poll` / `flush` use.
#[cfg(not(target_os = "none"))]
pub static mut HOST_DEV: Option<&'static mut dyn BlockDevice> = None;
/// Host test: the disk is held (`poll` defers).
#[cfg(not(target_os = "none"))]
pub static HOST_BUSY: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// Write the backup SRAM record into the slot after the newest, sequence
/// + 1, read back. Uses diskbuf blocks 0 and 1.
#[inline(never)]
fn write_slot(dev: &mut dyn BlockDevice) -> Save {
    let s = st();
    let Some((version, crc)) = persist::stored() else {
        return Save::BadRecord;
    };
    let n = persist::words_of(version).unwrap_or(0);
    let top = newest(&s.seq);
    let mut seq = top.and_then(|k| s.seq[k]).unwrap_or(0);
    if let Some(b) = persist::seq().filter(|&b| newer(b, seq)) {
        seq = b;
    }
    let seq = seq.wrapping_add(1);
    let k = top.map_or(0, |t| (t + 1) % SLOTS);
    let [b, rb, _] = blocks();
    encode(&mut b[..], version, seq, n, persist::word);
    // The copy must be the record the CRC belongs to (a save torn by a
    // reset cannot be: App::persist writes it in one go; checked anyway).
    if crc32(&b[4 * HDR..4 * (HDR + n)]) != crc {
        return Save::BadRecord;
    }
    let len = 4 * (HDR + n + 1);
    if !dev.write_block(s.lba[k], &b[..]) || !dev.read_block(s.lba[k], rb) || rb[..len] != b[..len] {
        // The slot is torn now: invalid until a later save rewrites it.
        s.seq[k] = None;
        return Save::Error;
    }
    s.seq[k] = Some(seq);
    s.saved = (version, crc);
    s.saves += 1;
    persist::set_seq(seq);
    Save::Ok
}

/// Write now if the backup SRAM record differs from the newest slot.
#[inline(never)]
fn save_now() {
    let s = st();
    if !file_ok() {
        s.save = Save::NoFile as u8;
        return;
    }
    let mut dev = new_dev();
    let r = write_slot(&mut dev);
    s.save = r as u8;
}

/// The disk is held by another writer (the diskbuf arena included).
#[cfg(target_os = "none")]
#[inline(always)]
fn disk_busy() -> bool {
    crate::screenshot::busy()
        || crate::csvlog::writing()
        || crate::fastlog::holds_disk()
        || crate::upload::busy()
        || crate::msc::HOST_WRITING.load(core::sync::atomic::Ordering::Relaxed)
}

#[cfg(not(target_os = "none"))]
fn disk_busy() -> bool {
    HOST_BUSY.load(core::sync::atomic::Ordering::Relaxed)
}

#[cfg(target_os = "none")]
#[inline(always)]
fn streaming() -> bool {
    crate::hy3131::streaming()
}

#[cfg(not(target_os = "none"))]
fn streaming() -> bool {
    false
}

/// Main loop, every pass (`hold`: an AD1 logger session, main.rs). Region
/// B (inlined): the writer itself is region C and runs only once the AD2
/// stream has stopped.
#[inline(always)]
pub fn poll(now: u64, hold: bool) {
    let s = st();
    if now < s.next_check {
        return;
    }
    s.next_check = now + CHECK_MS;
    if s.tokens < BUDGET && now >= s.refill {
        s.tokens += 1;
        s.refill = now + REFILL_MS;
    }
    let Some(cur) = persist::stored() else {
        return;
    };
    if cur == s.saved {
        return;
    }
    if cur != s.seen {
        s.seen = cur;
        s.changed = now;
    }
    if now.saturating_sub(s.changed) < DEBOUNCE_MS || !file_ok() {
        return;
    }
    if hold || s.tokens == 0 || streaming() || disk_busy() {
        s.save = Save::Deferred as u8;
        return;
    }
    if s.tokens == BUDGET {
        s.refill = now + REFILL_MS;
    }
    s.tokens -= 1;
    save_now();
}

/// Before a power-off (app.rs power_off, after finish_disk_writes and the
/// last backup SRAM save): write a changed record at once.
#[inline(never)]
pub fn flush() {
    let s = st();
    if persist::stored().is_some_and(|cur| cur != s.saved) {
        save_now();
    }
}
