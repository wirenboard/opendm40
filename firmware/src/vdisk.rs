//! The USB disk: a FAT16 volume synthesized on the fly
//! (DAPLink-style virtual FAT), the only mass-storage LUN of the native
//! build. Nothing of it is stored: the boot sector, both FATs and every
//! directory are generated from tables, file data is produced when the host
//! READs the sector. The SPI flash (our FAT16 internal storage, fatdir.rs)
//! is no longer exported, so no host can write it or hold a stale copy of
//! its FAT, and the device never has to wait for an eject.
//!
//! Volume "DM40", 512-byte sectors, 64 sectors (32 KiB) per cluster, 2 FATs
//! of 254 sectors, 512 root entries, 65000 clusters (FAT16 by the cluster
//! count), 4160541 sectors = 1.98 GiB: room for SPIFLASH.BIN, the CSV
//! files (a DLG log expands ~10-20x into CSV, a scope record more; a full
//! 16 MiB SPI disk of logs is a few hundred MiB of CSV) and a margin for
//! the append-only allocation below. Layout (fixed clusters first):
//!   README.TXT     what is where, how to update the firmware (cluster 7);
//!   INFO/          (cluster 2) CALIB / BOOTDATA / UID / OPTBYTES / OTP /
//!                  ROMBOOT / LOADER .BIN (raw bytes of the MCU regions,
//!                  calbackup::FILES), BACKUP.TXT (addresses, sizes, CRC-32s,
//!                  restore notes), FIRMWARE.BIN (the running firmware as its
//!                  vendor update file, calbackup::FwFile; only with a valid
//!                  loader record); from cluster 8, each contiguous;
//!   DEBUG/         (cluster 3) SPIFLASH.BIN: the raw 16 MiB of the SPI
//!                  flash (`fsck.fat -n` / mtools on it show the internal
//!                  FAT16), read on every access;
//!   LOGS/          (cluster 4) LOGnnn.CSV: the CSV generated from each
//!                  LOGnnn.DLG (dlog.rs `Gen`, the exact text the old CSV
//!                  writer produced);
//!   SCOPE/         (cluster 5) SCOPnnn.CSV from SCOPnnn.DLG;
//!   SHOTS/         (cluster 6) Snnn.BMP: the screenshots, streamed from the
//!                  SPI file (its FAT chain);
//! the LOGS / SCOPE / SHOTS files come from the SPI disk's LOGS/, SCOPE/,
//! SHOTS/ and from its root (earlier builds), numbered names only, up to
//! MAX_VF of them. A file being recorded (csvlog) is left out until it
//! ends. Every subdirectory is one cluster (1024 slots: "." / ".." and up to
//! 1022 files).
//!
//! Changes (`refresh`, from the main loop when dlog.rs `stale()` and the
//! disk has been quiet for a second, see main.rs): the SPI disk is scanned
//! (directories only; a DLG's CSV size is computed once, when it is new)
//! and the table updated:
//!   * an existing file keeps its directory slot and its clusters for as
//!     long as it exists (a host's cached directory / FAT / inode stays
//!     right for it);
//!   * a new file gets the next slot of its directory and clusters from the
//!     high-water mark (append-only: a freed slot shows as deleted, 0xE5,
//!     and its clusters stay unused);
//!   * a deleted / changed file disappears (`forget` does it at once for a
//!     delete on the device, so a new file of the same name never inherits
//!     its place);
//!   * when the slots of a directory or the clusters run out, the table is
//!     compacted (every file renumbered from the start), but only while no
//!     host holds the medium locked (msc.rs `host_mounted`: Linux sends
//!     PREVENT on mount); else the new files wait for the unmount. After a
//!     USB bus reset (re-plug) the next refresh compacts.
//! Any change raises a one-shot media change: the next command gets UNIT
//! ATTENTION 06/28/00 at once, also while the volume is mounted and locked
//! (msc.rs; the 2026-10-21 experiment: Linux then re-reads and shows the new
//! files, a file manager after a refresh; holding it back while locked hid
//! them until the unmount).
//!
//! Host writes (vwrite.rs): read-only by default ("Files live": the WP bit,
//! the UNIT ATTENTION above at once). Settings 3/3 "USB disk" = "PC can
//! edit" (DBG:VDISK:RO 0) makes it writable from the next bus reset (a
//! re-plug; volatile, read-only again at boot). Writable, a write is only
//! accepted as a delete of a LOGS / SCOPE /
//! SHOTS file, a firmware file DM40*.BIN in the root, the hosts' own
//! housekeeping (trash, indexer: kept for the session) or metadata noise
//! (dirty flags, dates); everything else fails with DATA PROTECT. While a
//! host holds the volume (`vwrite::frozen`) `refresh` waits: the layout
//! changes only by the host's own writes (tomb flags `T_*` in the table,
//! vwrite.rs records) until the session ends.
//!
//! RAM: the file table (MAX_VF x 16 bytes) and the built-in table in SRAM
//! (.sram_bss / .dmabuf), the CSV / BMP read cursors in .bss.
//!
//! Host test: tools/fatren-test/run.sh section 19 (the whole volume dumped
//! through `read_block`, fsck.fat -n, mtools, contents vs the sources, the
//! allocation rules across refreshes).

use crate::calbackup::{crc32_upd, FwFile, Source, FILES};
use crate::dlog::{self, FatFile, FnRaw, Gen, Geo, Raw, Src as _};
use crate::fastlog::{MIR_BEHIND, MIR_FREE};
use crate::msc::BlockDevice;

pub const BS: usize = 512;
/// Sectors per cluster, and bytes per cluster.
pub(crate) const SPC: u32 = 64;
pub(crate) const CB: u32 = SPC * BS as u32;
const RSVD: u32 = 1;
const NFATS: u32 = 2;
const ROOT_ENTRIES: u32 = 512;
const ROOT_SECS: u32 = ROOT_ENTRIES * 32 / BS as u32;
pub const CLUSTERS: u32 = 65_000;
pub(crate) const FAT_SECS: u32 = ((CLUSTERS + 2) * 2).div_ceil(BS as u32);
pub(crate) const FAT_START: u32 = RSVD;
pub(crate) const ROOT_START: u32 = RSVD + NFATS * FAT_SECS;
pub(crate) const DATA_START: u32 = ROOT_START + ROOT_SECS;
pub const TOTAL: u32 = DATA_START + CLUSTERS * SPC;
const _: () = assert!(CLUSTERS >= 4085 + 16 && CLUSTERS < 65525 - 16, "FAT16 by the cluster count, with a margin");
pub const LABEL: [u8; 11] = *b"DM40       ";
/// Directory dates 2026-10-13 (no RTC), as calbackup / screenshot.
const DATE: u16 = (46 << 9) | (10 << 5) | 13;
const TXT: [u8; 11] = *b"BACKUP  TXT";
const FW: [u8; 11] = *b"FIRMWAREBIN";
/// Built-ins of INFO/: FILES + BACKUP.TXT + FIRMWARE.BIN.
pub const NF: usize = FILES.len() + 2;
/// Upper bound of BACKUP.TXT (asserted against the worst case below).
const TXT_MAX: u32 = 4096;
const UID_ADDR: u32 = 0x1FFF_7A10;
const VERSION: &str = crate::ident::VERSION;

/// Fixed clusters: the five subdirectories (one cluster each), README.TXT,
/// then the INFO/ files from C_FILES.
const C_INFO: u32 = 2;
const C_DEBUG: u32 = 3;
/// LOGS, SCOPE, SHOTS: C_VDIR + d (d = the file kind, `VDIRS`).
const C_VDIR: u32 = 4;
const C_README: u32 = 7;
const C_FILES: u32 = 8;
/// DEBUG/SPIFLASH.BIN: the W25Q128, 16 MiB.
pub const SPI_BYTES: u32 = 16 << 20;
/// Root entries after the label: README.TXT, INFO, DEBUG, LOGS, SCOPE, SHOTS.
pub(crate) const ROOT_N: usize = 6;
/// Generated directories, file kinds: (name, file prefix, file extension).
/// Kind 3 (FASTnnn.CSV, the fast logger) lives in LOGS (`dir`).
const VDIRS: [([u8; 11], &[u8], [u8; 3]); 4] = [
    (*b"LOGS       ", b"LOG", *b"CSV"),
    (*b"SCOPE      ", b"SCOP", *b"CSV"),
    (*b"SHOTS      ", b"S", *b"BMP"),
    (*b"LOGS       ", b"FAST", *b"CSV"),
];

/// The directory (0 LOGS, 1 SCOPE, 2 SHOTS) of file kind `k`.
#[cfg_attr(target_os = "none", link_section = ".text.vdisk_rd")]
fn dir(k: usize) -> usize {
    if k == 3 { 0 } else { k }
}

/// The directory (as `dir_kind`) of table id `id`.
#[cfg_attr(target_os = "none", link_section = ".text.vdisk_rd")]
pub(crate) fn id_dir(id: u16) -> usize {
    dir(kind(id))
}
/// Slots of a one-cluster directory.
const SLOTS: u32 = CB / 32;
/// Files of LOGS / SCOPE / SHOTS at most (16 bytes of SRAM each).
pub const MAX_VF: usize = 400;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Mem,
    Txt,
    Fw,
}

#[derive(Clone, Copy)]
struct VFile {
    name: [u8; 11],
    kind: Kind,
    addr: u32,
    size: u32,
    /// First cluster (0 = empty file).
    start: u32,
    crc: u32,
}

struct State {
    files: [VFile; NF],
    n: usize,
    fw: Option<FwFile<'static>>,
    mem: fn(u32, &mut [u8]),
    uid: [u8; 12],
    /// DEBUG/SPIFLASH.BIN's first cluster, and the first dynamic cluster.
    spi_start: u32,
    dyn0: u32,
    /// LBA ranges [lo, hi) of the INFO/ sectors served from the slow flash
    /// bank (`SLOW_LO`..): refused while the AD2 stream runs (`read_busy`).
    slow: [(u32, u32); NF],
}

/// Internal flash from here to the end of its 1 MiB is NOT zero-wait
/// (link-native.x region C, the FS-loader, its UID data, the calibration
/// record): one CPU load from it stalls the bus ~21.8 us, and the AD2
/// stream ISR's entry with it, whatever code does the load (device c92b605:
/// INFO/CALIB.BIN, BOOTDATA.BIN, LOADER.BIN and the FIRMWARE.BIN tail copied
/// by a host during a 38400 disk session: 10 k samples lost, at 76800 289 k).
/// The volume therefore never reads it while the stream ISR owns the bus
/// (hy3131::streaming: fast-log session, Wave view, inrush page, the AD1
/// stream): those sectors are served from the mirror on the SPI disk
/// (`mirror_boot`, `MIR_HI`), byte-equal to the flash as checked at this
/// boot. Without a valid mirror (no SPI disk, full, a write error) a
/// READ(10) touching them gets NOT READY, BECOMING READY (02/04/01, msc.rs)
/// and the host retries until the stream ends (Linux, sd / scsi_lib:
/// delayed retries up to (retries + 1) x timeout = 6 x 30 s after the
/// request was made, then an I/O error). The emulator's data watch
/// (emu/regionc.py, peripherals/FlashWatch.cs) checks that nothing reads
/// the slow bank while streaming.
pub(crate) const SLOW_LO: u32 = 0x0808_0000;
const SLOW_HI: u32 = 0x0810_0000;

/// The mirror: internal flash `SLOW_LO .. MIR_HI` (the FIRMWARE.BIN tail,
/// the FS-loader sector with BOOTDATA, the calibration record's 4 KiB:
/// every INFO/ byte from the slow bank, asserted below), 260 KiB, raw, as
/// root file `MIR_NAME` of the SPI disk.
pub(crate) const MIR_HI: u32 = 0x080C_1000;
const MIR_NAME: [u8; 11] = *b"_INFOMIRBIN";
/// Clusters of the mirror on a disk of 4 KiB clusters (the DM40's; larger
/// clusters need fewer).
const MIR_NCL: usize = ((MIR_HI - SLOW_LO) / 4096) as usize;
/// Every slow-bank byte of an INFO/ file is inside the mirror.
const _: () = {
    let mut i = 0;
    while i < FILES.len() {
        let (_, a, n) = FILES[i];
        assert!(a + n <= SLOW_LO || a >= SLOW_HI || (a >= SLOW_LO && a + n <= MIR_HI), "an INFO/ file's slow-bank bytes are not mirrored");
        i += 1;
    }
    assert!(crate::calbackup::FW_BASE + crate::calbackup::FW_MAX_BODY as u32 <= MIR_HI);
};

/// Where the mirror is (`mirror_boot`): SPI byte address of cluster 2,
/// bytes per cluster, its clusters; the boot's writer result. SRAM, zeroed.
struct Mir {
    data: u32,
    cb: u32,
    cl: [u16; MIR_NCL],
    status: u8,
    written: bool,
    ms: u32,
}
// Backup SRAM (.bkp_bss, link-native.x; CPU only, zeroed at reset;
// docs/native-boot.md "Extra RAM areas").
#[cfg_attr(target_os = "none", link_section = ".bkp_bss")]
static mut MIR: Mir = Mir { data: 0, cb: 0, cl: [0; MIR_NCL], status: 0, written: false, ms: 0 };
/// The mirror equals the flash (checked or written at this boot) and is
/// served; cleared for good by a failed SPI read.
static MIR_OK: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// One generated file of LOGS / SCOPE / SHOTS. `id` = flags | kind << 10 |
/// number (kind + number = the name, `ID`; flags `T_*`: a host delete in
/// progress, vwrite.rs), `slot` its directory slot, `vstart` its first cluster here,
/// `src` / `ssize` the SPI file (first cluster, bytes), `vsize` its size
/// here (the CSV's for a DLG).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct VF {
    pub id: u16,
    pub slot: u16,
    pub vstart: u16,
    pub src: u16,
    pub ssize: u32,
    pub vsize: u32,
}

const VF0: VF = VF { id: 0, slot: 0, vstart: 0, src: 0, ssize: 0, vsize: 0 };
/// `VF::id`: kind + number (the name).
pub(crate) const ID: u16 = 0x0FFF;
/// The host marked its directory entry deleted (0xE5): the slot shows the
/// entry with 0xE5 (vwrite.rs).
pub(crate) const T_DIR: u16 = 1 << 12;
/// The host freed its chain in the FAT: shown free, data reads zeros.
pub(crate) const T_FAT: u16 = 1 << 13;
/// The SPI file is gone (deleted while the layout is frozen, by the host's
/// delete or on the meter): data reads zeros, the entry stays until the
/// session ends.
pub(crate) const T_SRC: u16 = 1 << 14;
/// Its entry (start cluster + size) was written into a host-created
/// directory (a move to the trash): deleted at the session's end.
pub(crate) const T_MOVED: u16 = 1 << 15;
/// Every flag.
pub(crate) const T_ALL: u16 = T_DIR | T_FAT | T_SRC | T_MOVED;

#[cfg_attr(target_os = "none", link_section = ".text.vdisk_rd")]
fn kind(id: u16) -> usize {
    ((id >> 10) & 3) as usize
}

/// The dynamic table, ascending `vstart` (allocation is append-only, a
/// compaction keeps the order), so the FAT and data lookups bisect it.
struct Tab {
    e: [VF; MAX_VF],
    n: usize,
    /// Next free cluster, next free slot per directory.
    hw: u32,
    slot_hw: [u16; 3],
    /// Compact at the next refresh (bus reset), or when a file did not fit.
    want_compact: bool,
    /// Files left out (no slot / cluster / table room) at the last refresh.
    left_out: u16,
}

/// Backup SRAM (.bkp_bss, link-native.x; CPU only, docs/native-boot.md
/// "Extra RAM areas"): written by `init` before READY is set; TCM is kept
/// for the stack.
#[cfg_attr(target_os = "none", link_section = ".bkp_bss")]
static mut STATE: core::mem::MaybeUninit<State> = core::mem::MaybeUninit::uninit();
#[cfg_attr(target_os = "none", link_section = ".dmabuf")]
static mut TAB: core::mem::MaybeUninit<Tab> = core::mem::MaybeUninit::uninit();
static READY: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);
/// One-shot media change, taken by msc.rs.
static CHANGED: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

#[cfg_attr(target_os = "none", link_section = ".text.vdisk_rd")]
fn state() -> Option<&'static State> {
    if READY.load(core::sync::atomic::Ordering::Acquire) {
        Some(unsafe { (*core::ptr::addr_of!(STATE)).assume_init_ref() })
    } else {
        None
    }
}

#[cfg_attr(target_os = "none", link_section = ".text.vdisk_rd")]
fn tab() -> &'static mut Tab {
    unsafe { (*core::ptr::addr_of_mut!(TAB)).assume_init_mut() }
}

/// Target memory reader: byte loads (the OTP / option-byte areas included).
#[cfg_attr(target_os = "none", link_section = ".text.vdisk_rd")]
pub fn mem_read(addr: u32, buf: &mut [u8]) {
    for (i, b) in buf.iter_mut().enumerate() {
        *b = unsafe { core::ptr::read_volatile((addr as usize + i) as *const u8) };
    }
}

fn mem_crc(mem: fn(u32, &mut [u8]), addr: u32, len: u32) -> u32 {
    let mut t = [0u8; 256];
    let mut c = !0u32;
    let mut off = 0u32;
    while off < len {
        let n = (len - off).min(t.len() as u32) as usize;
        mem(addr + off, &mut t[..n]);
        c = crc32_upd(c, &t[..n]);
        off += n as u32;
    }
    !c
}

/// Build the built-in tables and the CRC-32s (boot, before USB; ~0.1 s).
/// `mem` reads the source regions, `flash` = the bytes from 0x08004000 (at
/// most calbackup::FW_MAX_BODY) for FIRMWARE.BIN. The LOGS / SCOPE / SHOTS
/// files come with the first `refresh`. Never inlined: its buffers are gone
/// before main().
#[inline(never)]
pub fn init(mem: fn(u32, &mut [u8]), flash: &'static [u8]) {
    let none = VFile { name: [b' '; 11], kind: Kind::Mem, addr: 0, size: 0, start: 0, crc: 0 };
    let mut files = [none; NF];
    let mut n = 0;
    for &(name, addr, len) in FILES.iter() {
        files[n] = VFile { name, kind: Kind::Mem, addr, size: len, start: 0, crc: mem_crc(mem, addr, len) };
        crate::wdt::kick_if_feeding();
        n += 1;
    }
    let txt = n;
    files[n] = VFile { name: TXT, kind: Kind::Txt, ..none };
    n += 1;
    let fw = FwFile::from_flash(flash);
    if let Some(f) = fw {
        files[n] = VFile { name: FW, kind: Kind::Fw, addr: crate::calbackup::FW_BASE, size: f.size(), start: 0, crc: f.crc() };
        n += 1;
    }
    let mut uid = [0u8; 12];
    mem(UID_ADDR, &mut uid);
    let mut st = State { files, n, fw, mem, uid, spi_start: 0, dyn0: 0, slow: [(0, 0); NF] };
    // The text's length does not depend on the CRC values (fixed width).
    st.files[txt].size = describe(&st, 0, &mut []) as u32;
    let mut c = C_FILES;
    for f in st.files[..n].iter_mut() {
        let k = f.size.div_ceil(CB);
        f.start = if k == 0 { 0 } else { c };
        c += k;
    }
    st.spi_start = c;
    st.dyn0 = c + SPI_BYTES / CB;
    // The slow sectors: from the one holding the first byte at SLOW_LO (the
    // update file is encoded per 128-byte block, inside one 512-byte
    // sector) to the file's end.
    for (f, sl) in st.files.iter().take(n).zip(st.slow.iter_mut()) {
        let off = match f.kind {
            Kind::Mem if f.addr < SLOW_HI && f.addr + f.size > SLOW_LO => SLOW_LO.saturating_sub(f.addr),
            Kind::Fw if f.size > 12 + (SLOW_LO - f.addr) => 12 + (SLOW_LO - f.addr),
            _ => continue,
        };
        if f.start != 0 {
            let lba0 = DATA_START + (f.start - 2) * SPC;
            *sl = (lba0 + off / BS as u32, lba0 + f.size.div_ceil(BS as u32));
        }
    }
    unsafe {
        (*core::ptr::addr_of_mut!(STATE)).write(st);
        (*core::ptr::addr_of_mut!(TAB)).write(Tab { e: [VF0; MAX_VF], n: 0, hw: 0, slot_hw: [2; 3], want_compact: false, left_out: 0 });
    }
    tab().hw = st_dyn0();
    READY.store(true, core::sync::atomic::Ordering::Release);
}

fn st_dyn0() -> u32 {
    unsafe { (*core::ptr::addr_of!(STATE)).assume_init_ref().dyn0 }
}

// --- the dynamic files -----------------------------------------------------------

/// (kind, number) of an SPI file of ours with a numbered name (LOGnnn.DLG,
/// SCOPnnn.DLG, Snnn.BMP, FASTnnn.DLG), else None.
fn ours(n: &[u8; 11]) -> Option<(usize, u16)> {
    let k = if dlog::dlg_prefix(n) == Some(b"LOG") {
        0
    } else if dlog::dlg_prefix(n) == Some(b"SCOP") {
        1
    } else if dlog::dlg_prefix(n) == Some(b"FAST") {
        3
    } else if n[8..].eq_ignore_ascii_case(b"BMP") && n[0].eq_ignore_ascii_case(&b'S') {
        2
    } else {
        return None;
    };
    let p = VDIRS[k].1.len();
    let d = &n[p..p + 3];
    if !d.iter().all(|c| c.is_ascii_digit()) || n[p + 3..8].iter().any(|&c| c != b' ') {
        return None;
    }
    Some((k, (d[0] - b'0') as u16 * 100 + (d[1] - b'0') as u16 * 10 + (d[2] - b'0') as u16))
}

/// 8.3 name of table id `id` on this volume ("LOG007  CSV").
pub(crate) fn vname(id: u16) -> [u8; 11] {
    let (pre, ext) = (VDIRS[kind(id)].1, &VDIRS[kind(id)].2);
    crate::calbackup::numbered_name(pre, id & 0x3FF, ext)
}

/// 8.3 name of table id `id` on the SPI disk ("LOG007  DLG").
pub(crate) fn spi_name(id: u16) -> [u8; 11] {
    let ext: &[u8; 3] = if kind(id) == 2 { b"BMP" } else { b"DLG" };
    crate::calbackup::numbered_name(VDIRS[kind(id)].1, id & 0x3FF, ext)
}

#[cfg_attr(target_os = "none", link_section = ".text.vdisk_rd")]
pub(crate) fn nclus(size: u32) -> u32 {
    size.div_ceil(CB)
}

/// Every SPI file of ours (root, LOGS/, SCOPE/, SHOTS/) to `f(id, start,
/// size)`, except `exclude` (8.3 SPI name). False: no disk / a read failed.
pub(crate) fn scan_spi(exclude: Option<[u8; 11]>, f: &mut dyn FnMut(u16, u16, u32)) -> bool {
    let mut d = dlog::disk();
    let Some(g) = dlog::geometry(&mut d) else {
        return false;
    };
    let dirs = [None, dlog::dir_start(&mut d, &g, &dlog::DIR_LOGS), dlog::dir_start(&mut d, &g, &dlog::DIR_SCOPE), dlog::dir_start(&mut d, &g, &dlog::DIR_SHOTS)];
    for (i, start) in dirs.iter().enumerate() {
        if i > 0 && start.is_none() {
            continue;
        }
        let ok = dlog::each_entry(&mut d, &g, *start, &mut |e| {
            if e[0] == 0xE5 || e[11] & 0x18 != 0 || e[11] & 0x3F == 0x0F {
                return true;
            }
            let mut n = [0u8; 11];
            n.copy_from_slice(&e[..11]);
            if Some(n) == exclude {
                return true;
            }
            if let Some((k, num)) = ours(&n) {
                let start = e[0x1A] as u16 | (e[0x1B] as u16) << 8;
                let size = u32::from_le_bytes([e[0x1C], e[0x1D], e[0x1E], e[0x1F]]);
                f((k as u16) << 10 | num, start, size);
            }
            true
        });
        if !ok {
            return false;
        }
    }
    true
}

/// Size on this volume of SPI file `id` (start, size): the generated CSV's
/// for a DLG (None: not a valid DLG), the file's own for a BMP.
fn vsize_of(id: u16, src: u16, ssize: u32) -> Option<u32> {
    if kind(id) == 2 {
        return (ssize > 0).then_some(ssize);
    }
    let mut d = dlog::disk();
    let g = dlog::geometry(&mut d)?;
    #[cfg(target_os = "none")]
    crate::wdt::kick_if_feeding();
    Gen::open(FatFile::new(d, g, src as u32, ssize), ssize).map(|g| g.info.csv)
}

/// Remove table entry `i` (its slot reads as deleted, its clusters stay
/// unused until a compaction).
fn remove(t: &mut Tab, i: usize) {
    t.e.copy_within(i + 1..t.n, i);
    t.n -= 1;
}

/// Renumber every file from the start: slots 2.. per directory, clusters
/// from dyn0, in table order. True if anything moved.
fn compact(t: &mut Tab) -> bool {
    let mut c = st_dyn0();
    let mut sl = [2u16; 3];
    let mut moved = false;
    for e in t.e[..t.n].iter_mut() {
        let k = dir(kind(e.id));
        moved |= e.slot != sl[k] || e.vstart as u32 != c;
        e.slot = sl[k];
        e.vstart = c as u16;
        sl[k] += 1;
        c += nclus(e.vsize);
    }
    moved |= t.hw != c || t.slot_hw != sl;
    t.hw = c;
    t.slot_hw = sl;
    t.want_compact = false;
    moved
}

/// Add the SPI files not in the table yet. Returns (added, left out for
/// want of slots / clusters, left out because the table is full).
fn add_new(t: &mut Tab, exclude: Option<[u8; 11]>) -> (u32, u32, u32) {
    let (mut added, mut out, mut full) = (0u32, 0u32, 0u32);
    scan_spi(exclude, &mut |id, src, ssize| {
        if t.e[..t.n].iter().any(|e| e.id & ID == id) {
            return; // present (or a duplicate number: the first one stays)
        }
        let Some(vsize) = vsize_of(id, src, ssize) else { return };
        let k = dir(kind(id));
        let need = nclus(vsize);
        if t.n == MAX_VF {
            full += 1;
            return;
        }
        if t.slot_hw[k] as u32 >= SLOTS || t.hw + need > CLUSTERS + 2 {
            out += 1;
            return;
        }
        t.e[t.n] = VF { id, slot: t.slot_hw[k], vstart: t.hw as u16, src, ssize, vsize };
        t.n += 1;
        t.slot_hw[k] += 1;
        t.hw += need;
        added += 1;
    });
    (added, out, full)
}

/// Rebuild LOGS / SCOPE / SHOTS from the SPI disk (main loop, when dlog.rs
/// `stale()` and no on-device writer holds the disk; see the module text;
/// not while frozen).
/// `exclude` = the SPI file being recorded. Raises the media change when the
/// volume changed. Returns the number of files.
pub fn refresh(exclude: Option<[u8; 11]>) -> usize {
    if state().is_none() {
        return 0;
    }
    // A host that can write holds the volume (vwrite.rs): its cached FAT /
    // directories must stay what it wrote; the change waits (stale stays
    // set) for the session's end.
    if crate::vwrite::frozen() {
        return tab().n;
    }
    dlog::clear_stale();
    let t = tab();
    // 1. Keep the files that are still there unchanged (same first cluster
    //    and size), drop the others.
    let mut seen = [0u32; MAX_VF.div_ceil(32)];
    let ok = scan_spi(exclude, &mut |id, src, ssize| {
        if let Some(i) = t.e[..t.n].iter().position(|e| e.id & ID == id && e.src == src && e.ssize == ssize) {
            seen[i / 32] |= 1 << (i % 32);
        }
    });
    if !ok {
        return t.n; // no disk / a read error: the volume stays as it is
    }
    let mut changed = false;
    for i in (0..t.n).rev() {
        if seen[i / 32] & (1 << (i % 32)) == 0 {
            remove(t, i);
            changed = true;
        }
    }
    // 2. Compaction (after a bus reset, or files left out last time), only
    //    while no host has the volume locked.
    let free = !crate::msc::host_mounted();
    if t.want_compact && free {
        changed |= compact(t);
    }
    // 3. New files, append-only; what does not fit for want of slots /
    //    clusters triggers a compaction (now if free, else after the unmount:
    //    `on_unlock`) and a second try. A full table stays full (MAX_VF).
    let (mut added, mut out, mut full) = add_new(t, exclude);
    if out > 0 && free && compact(t) {
        changed = true;
        let (a2, o2, f2) = add_new(t, exclude);
        added += a2;
        out = o2;
        full = f2;
    }
    t.want_compact |= out > 0;
    t.left_out = (out + full) as u16;
    if changed || added > 0 {
        CHANGED.store(true, core::sync::atomic::Ordering::Relaxed);
    }
    t.n
}

/// The first `refresh`, at boot before USB: no host has seen the volume
/// yet, so no media change is raised for it.
#[inline(never)]
pub fn boot_refresh() {
    refresh(None);
    CHANGED.store(false, core::sync::atomic::Ordering::Relaxed);
}

/// A file of ours was deleted on the device (csvlog::delete_on, 8.3 SPI
/// name): drop it from the volume at once (before the next refresh, so a
/// new file of the same name can never inherit its slot / clusters while a
/// host caches them).
pub fn forget(name: &[u8; 11]) {
    if state().is_none() {
        return;
    }
    let Some((k, num)) = ours(name) else { return };
    let t = tab();
    let id = (k as u16) << 10 | num;
    if let Some(i) = t.e[..t.n].iter().position(|e| e.id & ID == id) {
        if crate::vwrite::frozen() || t.e[i].id & T_ALL != 0 {
            // The host may hold it (or is deleting it): the entry stays as
            // the host knows it, its data reads zeros from now on.
            t.e[i].id |= T_SRC;
        } else {
            remove(t, i);
            CHANGED.store(true, core::sync::atomic::Ordering::Relaxed);
        }
    }
}

/// The table (vwrite.rs). Inlined: vwrite::poll (region B) calls it on
/// every main-loop pass and this module is in .cold (region C).
#[inline(always)]
pub(crate) fn entries() -> &'static mut [VF] {
    if state().is_none() {
        return &mut [];
    }
    let t = tab();
    // Not `&mut t.e[..t.n]`: that range index is a generic of this module
    // (placed in .cold) and was not inlined (emulator hooks: called on
    // every pass).
    let n = t.n.min(t.e.len());
    unsafe { core::slice::from_raw_parts_mut(t.e.as_mut_ptr(), n) }
}

/// Who owns cluster `c` in the generated layout (overlays not counted).
pub(crate) enum Owner {
    /// Media / reserved entries 0 and 1, or past the last cluster.
    Reserved,
    /// A directory, README.TXT, an INFO/ file, SPIFLASH.BIN.
    Fixed,
    /// Table entry `i`.
    Dyn(usize),
    Free,
}

pub(crate) fn owner(c: u32) -> Owner {
    if c < 2 || c >= CLUSTERS + 2 || state().is_none() {
        return Owner::Reserved;
    }
    if c < st_dyn0() {
        return Owner::Fixed;
    }
    let t = tab();
    let i = t.e[..t.n].partition_point(|e| e.vstart as u32 <= c);
    match i.checked_sub(1) {
        Some(j) if c < t.e[j].vstart as u32 + nclus(t.e[j].vsize) => Owner::Dyn(j),
        _ => Owner::Free,
    }
}

/// The generated directory entry of table entry `e` (vwrite.rs compares
/// host writes with it).
pub(crate) fn dyn_entry(e: &VF, out: &mut [u8]) {
    out[..32].fill(0);
    dirent(out, &vname(e.id), 0x01, e.vstart as u32, e.vsize);
}

/// Which generated directory cluster `c` is: Some(kind) for LOGS / SCOPE /
/// SHOTS, Some(3) for INFO / DEBUG, None otherwise.
pub(crate) fn dir_kind(c: u32) -> Option<usize> {
    match c {
        C_INFO | C_DEBUG => Some(3),
        C_VDIR..=6 => Some((c - C_VDIR) as usize),
        _ => None,
    }
}

/// The generated sector `lba` (overlays included), as read_block.
pub(crate) fn gen_sector(lba: u32, buf: &mut [u8]) -> bool {
    VDisk.read_block(lba, buf)
}

/// USB bus reset (msc.rs): a new session, compact at the next refresh; the
/// volume mode asked for (read-only / writable, vwrite.rs) takes effect.
pub fn on_bus_reset() {
    if state().is_some() {
        crate::vwrite::release_all(true);
        tab().want_compact = true;
        dlog::set_stale();
    }
    crate::vwrite::on_bus_reset();
}

/// (files, next free cluster, files left out) for DBG / tests.
pub fn stats() -> (usize, u32, u16) {
    if state().is_none() {
        return (0, 0, 0);
    }
    let t = tab();
    (t.n, t.hw, t.left_out)
}

/// The table's live entries / the fixed files, for the per-sector read
/// path (region B, `.text.vdisk_rd`): not `&t.e[..t.n]`, whose range index
/// is a generic of this module, placed in .cold and not always inlined (as
/// `entries`).
#[inline(always)]
fn live(t: &Tab) -> &[VF] {
    unsafe { core::slice::from_raw_parts(t.e.as_ptr(), t.n.min(t.e.len())) }
}
#[inline(always)]
fn fixed(st: &State) -> &[VFile] {
    unsafe { core::slice::from_raw_parts(st.files.as_ptr(), st.n.min(st.files.len())) }
}

/// The entry holding cluster `c` (>= dyn0): bisect on `vstart`.
#[cfg_attr(target_os = "none", link_section = ".text.vdisk_rd")]
fn at_cluster(t: &Tab, c: u32) -> Option<&VF> {
    let l = live(t);
    let i = l.partition_point(|e| e.vstart as u32 <= c);
    let e = l.get(i.checked_sub(1)?)?;
    (c < e.vstart as u32 + nclus(e.vsize)).then_some(e)
}

// --- read cursors ------------------------------------------------------------

/// The CSV generator of the last DLG read (key: SPI start, size), ~0.7 KiB
/// of .bss; a BMP's chain cursor likewise.
static mut CSV_CUR: dlog::Slot<(u16, u32)> = dlog::Slot::new();

#[inline(always)]
fn rd_cyc() -> u32 {
    #[cfg(target_os = "none")]
    return unsafe { crate::mmio::r32(0xE000_1004) };
    #[cfg(not(target_os = "none"))]
    return 0;
}

/// The second CSV cursor (`file_read`; SRAM on test/scope-chain-0.4, TCM
/// .bss on release/0.4.0: docs/native-boot.md "SRAM audit"; all zero =
/// empty) and the least recently used one; a cursor at most this far before
/// a read goes on from there.
#[cfg_attr(target_os = "none", link_section = ".sram_bss")]
static mut CSV_CUR2: dlog::Slot<(u16, u32)> = dlog::Slot::new();
static mut CSV_LRU: u8 = 0;
const CUR_NEAR: u32 = 1 << 20;

/// Read-path statistics (DBG:VDISK:RD?): read_block calls, their cycles
/// (total / 1024, max), CSV generator opens.
pub static RD: [core::sync::atomic::AtomicU32; 4] = [const { core::sync::atomic::AtomicU32::new(0) }; 4];
static mut BMP_KEY: (u16, u32) = (0, 0);
static mut BMP_CUR: core::mem::MaybeUninit<FatFile<FnRaw>> = core::mem::MaybeUninit::uninit();

#[cfg_attr(target_os = "none", link_section = ".text.vdisk_rd")]
fn geo() -> Option<(FnRaw, Geo)> {
    let mut d = dlog::disk();
    let g = dlog::geometry(&mut d)?;
    Some((d, g))
}

/// How far cursor `c` is behind `off` when it holds file `key` (within
/// CUR_NEAR). A fn, not a closure in `file_read`: a closure's section name
/// carries this module's path, so the .cold rule put it in region C (device
/// d8e249d: DBG:ISR? lat_pc = `file_read::{closure#0}`, max_lat 9 us while a
/// host read the volume during an AD2 19200 stream).
#[cfg_attr(target_os = "none", link_section = ".text.vdisk_rd")]
fn near(c: &dlog::Slot<(u16, u32)>, key: (u16, u32), off: u32) -> Option<u32> {
    match c.peek() {
        Some((k, p)) if k == key && p <= off && off - p < CUR_NEAR => Some(off - p),
        _ => None,
    }
}

/// Bytes `off..` of table file `e` into `buf` (zeros on a read error).
#[cfg_attr(target_os = "none", link_section = ".text.vdisk_rd")]
fn file_read(e: &VF, off: u32, buf: &mut [u8]) {
    let key = (e.src, e.ssize);
    if kind(e.id) == 2 {
        let cur = unsafe { &mut *core::ptr::addr_of_mut!(BMP_CUR) };
        let k = unsafe { &mut *core::ptr::addr_of_mut!(BMP_KEY) };
        if *k != key || k.1 == 0 {
            let Some((d, g)) = geo() else { return };
            cur.write(FatFile::new(d, g, e.src as u32, e.ssize));
            *k = key;
        }
        if !unsafe { cur.assume_init_mut() }.read(off, buf) {
            buf.fill(0);
            *k = (0, 0);
        }
        return;
    }
    // Two cursors: a host that reads two places in turn (an indexer at a
    // file's head while a copy runs further on) keeps one each. The one
    // just behind `off` (within CUR_NEAR) of this file, else the one used
    // least recently (reopened for another file).
    let slots = unsafe { [&mut *core::ptr::addr_of_mut!(CSV_CUR), &mut *core::ptr::addr_of_mut!(CSV_CUR2)] };
    let lru = unsafe { &mut *core::ptr::addr_of_mut!(CSV_LRU) };
    let i = match (near(slots[0], key, off), near(slots[1], key, off)) {
        (Some(a), Some(b)) => (b < a) as usize,
        (Some(_), None) => 0,
        (None, Some(_)) => 1,
        (None, None) => *lru as usize,
    };
    *lru = (i == 0) as u8; // the other one is now the least recent
    let [c0, c1] = slots;
    let cur = if i == 0 { c0 } else { c1 };
    let open = || {
        RD[3].fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        let (d, g) = geo()?;
        Gen::open(FatFile::new(d, g, e.src as u32, e.ssize), e.ssize)
    };
    if let Some(g) = cur.get(key, open) {
        g.read(off, buf);
    }
}

// --- the block device --------------------------------------------------------------

/// Sectors `lba .. lba + n` include one served from the slow flash bank
/// (`SLOW_LO`).
#[cfg_attr(target_os = "none", link_section = ".text.vdisk_rd")]
fn slow_overlap(st: &State, lba: u32, n: u32) -> bool {
    st.slow.iter().any(|&(lo, hi)| lba < hi && lba.saturating_add(n) > lo)
}

/// The LBA ranges `read_busy` refuses while the stream runs (host test).
#[allow(dead_code)]
pub(crate) fn slow_ranges() -> [(u32, u32); NF] {
    state().map_or([(0, 0); NF], |st| st.slow)
}

// --- the slow-bank mirror ------------------------------------------------------------

/// Boot (native.rs pre_main, before USB; no stream yet): make the mirror
/// equal to the flash and note where it is. calbackup::write_file compares
/// the file with the flash first (every boot: 65 SPI blocks read, ~0.2 s)
/// and writes only when it differs (the first boot of a new image or after
/// a calibration change: a new chain, copy-on-write, read back; the old
/// chain freed), so an unchanged mirror costs no erase. Valid only with
/// this boot's PRESENT / WRITTEN: a stale file left by a failed rewrite
/// (no space, a write error) is never served.
#[cfg(target_os = "none")]
#[inline(never)]
pub fn mirror_boot() {
    use crate::calbackup::Status;
    let t0 = rd_cyc();
    // Read-only view of the internal flash (no stream runs at boot).
    let src: &[u8] = unsafe { core::slice::from_raw_parts(SLOW_LO as *const u8, (MIR_HI - SLOW_LO) as usize) };
    let mut disk = crate::spiflash::SpiFlash::new();
    let r = crate::calbackup::write_file(&mut disk, MIR_NAME, &src);
    let m = unsafe { &mut *core::ptr::addr_of_mut!(MIR) };
    m.status = r.status as u8;
    m.written = r.status == Status::Written;
    let mut ok = matches!(r.status, Status::Present | Status::Written) && r.conflict == 0;
    if ok {
        ok = match crate::calbackup::chain_of(&mut disk, &MIR_NAME, &mut m.cl) {
            Some((data, spc, size, n)) if size == MIR_HI - SLOW_LO && n as u32 * spc * 4096 >= size => {
                m.data = data * 4096;
                m.cb = spc * 4096;
                true
            }
            _ => false,
        };
    }
    m.ms = rd_cyc().wrapping_sub(t0) / (crate::clock::HCLK_HZ / 1000);
    MIR_OK.store(ok, core::sync::atomic::Ordering::Release);
}

/// DBG:VDISK:MIR?: (served, calbackup::Status of the boot's write_file as
/// a number: 0 NOTRUN 1 NOMEDIUM 2 BADBPB 3 PRESENT 4 WRITTEN 5 NOSPACE
/// 6 ERROR, rewritten at this boot, `mirror_boot`'s ms). Inlined into the
/// SCPI parser (region B: a host may ask during a stream).
#[inline(always)]
pub fn mirror_info() -> (bool, u8, bool, u32) {
    let m = unsafe { &*core::ptr::addr_of!(MIR) };
    (MIR_OK.load(core::sync::atomic::Ordering::Relaxed), m.status, m.written, m.ms)
}

/// Flash bytes `addr ..` (inside `SLOW_LO .. MIR_HI`) from the mirror on
/// the SPI disk. False: no valid mirror, outside it, or the SPI read failed
/// (which ends the mirror for this boot: NOT READY from then on).
#[cfg_attr(target_os = "none", link_section = ".text.vdisk_rd")]
fn mir_read(addr: u32, buf: &mut [u8]) -> bool {
    use core::sync::atomic::Ordering;
    if !MIR_OK.load(Ordering::Acquire) || addr < SLOW_LO || addr as u64 + buf.len() as u64 > MIR_HI as u64 {
        return false;
    }
    let m = unsafe { &*core::ptr::addr_of!(MIR) };
    let mut o = addr - SLOW_LO;
    let mut p = 0usize;
    while p < buf.len() {
        let ci = (o / m.cb) as usize;
        if ci >= MIR_NCL {
            return false;
        }
        let w = o % m.cb;
        let n = ((m.cb - w) as usize).min(buf.len() - p);
        let at = m.data + (m.cl[ci] as u32 - 2) * m.cb + w;
        if !dlog::disk().read_at(at, &mut buf[p..p + n]) {
            MIR_OK.store(false, Ordering::Release);
            return false;
        }
        p += n;
        o += n as u32;
    }
    true
}

// --- pacing the mirror's reads against the fast logger --------------------------------
//
// A mirror sector costs the main loop its whole SPI read (512 bytes polled,
// ~0.97 us each on the device, plus a wait for a page program in the chip),
// ~1.1 ms of wall time with the stream ISR at ~54 % of the CPU at 76 800
// S/s. A host copying LOADER.BIN (128 KiB, all mirrored) back to back took
// that from the fast logger's coder pass by pass: 21 745 samples lost per
// copy (device c785807 M2c), and a copy inside the writer trial made it
// fall back to 38 400. So while the stream runs a mirror sector is read
// only when this pass grants it (`mirror_pass`, main.rs, once per pass
// before the MSC poll; at most one sector per pass) and the chip is idle
// (no page program of the logger's to wait for). Until then the READ just
// waits in its data phase (msc.rs `read_wait`: the host's IN tokens are
// NAKed; no NOT READY, nothing to retry). Bounded: a sector that has
// waited MIR_WAIT_SOFT_MS goes ahead unless the logger is pressed (its
// ring over half full, or the writer trial), and after MIR_WAIT_HARD_MS in
// any case (a 240-sector READ, Linux's largest here, then takes at most
// 240 x 50 ms = 12 s against its 30 s timeout).

const MIR_WAIT_SOFT_MS: u32 = 10;
const MIR_WAIT_HARD_MS: u32 = 50;
/// This pass's level (`mirror_pass`); MIR_PASS_USED once a sector was read.
static MIR_LEVEL: core::sync::atomic::AtomicU8 = core::sync::atomic::AtomicU8::new(MIR_FREE);
const MIR_PASS_USED: u8 = 0xFF;
/// DWT cycles when the sector now due began to wait (0: none waits).
static MIR_WAIT_T: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
/// DBG:VDISK:MIR? extras: sectors read from the mirror while streaming,
/// times a sector was held back (one per poll), the longest hold (ms).
pub static MIR_PACE: [core::sync::atomic::AtomicU32; 3] = [const { core::sync::atomic::AtomicU32::new(0) }; 3];

/// main.rs, once per pass before the MSC poll: the fast logger's state
/// (fastlog::mirror_level: MIR_FREE / MIR_BEHIND / MIR_PRESSED) for this
/// pass's mirror sector.
#[inline(always)]
pub fn mirror_pass(level: u8) {
    MIR_LEVEL.store(level, core::sync::atomic::Ordering::Relaxed);
}

/// A READ's next sector `lba` must wait (msc.rs `read_wait`): it is served
/// from the mirror while the stream runs and this pass does not grant it.
/// Granting takes the pass's sector.
#[cfg_attr(target_os = "none", link_section = ".text.vdisk_rd")]
fn mir_hold(lba: u32) -> bool {
    use core::sync::atomic::Ordering::Relaxed;
    if !crate::hy3131::streaming() || !MIR_OK.load(Relaxed) || !state().is_some_and(|st| slow_overlap(st, lba, 1)) {
        // Not a paced sector: one that waited before was served or its
        // READ ended (a stream end, a host reset); the next starts afresh.
        MIR_WAIT_T.store(0, Relaxed);
        return false;
    }
    let lv = MIR_LEVEL.load(Relaxed);
    let now = rd_cyc() | 1;
    let t0 = MIR_WAIT_T.load(Relaxed);
    let waited_ms = if t0 == 0 { 0 } else { now.wrapping_sub(t0) / (crate::clock::HCLK_HZ / 1000) };
    let go = match lv {
        MIR_PASS_USED => false,
        MIR_FREE => !flash_busy(),
        MIR_BEHIND => waited_ms >= MIR_WAIT_SOFT_MS,
        _ => waited_ms >= MIR_WAIT_HARD_MS,
    } || waited_ms >= MIR_WAIT_HARD_MS;
    if go {
        MIR_LEVEL.store(MIR_PASS_USED, Relaxed);
        MIR_WAIT_T.store(0, Relaxed);
        MIR_PACE[0].fetch_add(1, Relaxed);
        MIR_PACE[2].fetch_max(waited_ms, Relaxed);
        return false;
    }
    if t0 == 0 {
        MIR_WAIT_T.store(now, Relaxed);
    }
    MIR_PACE[1].fetch_add(1, Relaxed);
    true
}

/// The SPI chip runs an erase / page program (one RDSR; no waiting).
#[cfg_attr(target_os = "none", link_section = ".text.vdisk_rd")]
fn flash_busy() -> bool {
    #[cfg(target_os = "none")]
    return crate::spiflash::chip_busy();
    #[cfg(not(target_os = "none"))]
    return false;
}

/// Source bytes of an INFO/ file of `Kind::Mem`: the slow-bank part from
/// the mirror while the stream runs, the rest from memory.
#[cfg_attr(target_os = "none", link_section = ".text.vdisk_rd")]
fn mem_src(st: &State, addr: u32, buf: &mut [u8]) -> bool {
    let end = addr as u64 + buf.len() as u64;
    if !crate::hy3131::streaming() || end <= SLOW_LO as u64 || addr >= SLOW_HI {
        (st.mem)(addr, buf);
        return true;
    }
    let k = (SLOW_LO.saturating_sub(addr) as usize).min(buf.len());
    (st.mem)(addr, &mut buf[..k]);
    mir_read(addr + k as u32, &mut buf[k..])
}

/// FIRMWARE.BIN's body bytes (calbackup::BodyRead; the body is the flash
/// from FW_BASE): those from SLOW_LO from the mirror while the stream runs.
#[cfg_attr(target_os = "none", link_section = ".text.vdisk_rd")]
fn fw_body(body: &[u8], q: usize, dst: &mut [u8]) -> bool {
    let a = crate::calbackup::FW_BASE + q as u32;
    let n = dst.len();
    if !crate::hy3131::streaming() || a + n as u32 <= SLOW_LO {
        dst.copy_from_slice(&body[q..q + n]);
        return true;
    }
    let k = (SLOW_LO.saturating_sub(a) as usize).min(n);
    dst[..k].copy_from_slice(&body[q..q + k]);
    mir_read(a + k as u32, &mut dst[k..])
}

/// The USB block device (a handle; the volume itself is `STATE` / `TAB`).
pub struct VDisk;

impl BlockDevice for VDisk {
    #[cfg_attr(target_os = "none", link_section = ".text.vdisk_rd")]
    fn present(&self) -> bool {
        state().is_some()
    }
    #[cfg_attr(target_os = "none", link_section = ".text.vdisk_rd")]
    fn block_size(&self) -> u32 {
        BS as u32
    }
    #[cfg_attr(target_os = "none", link_section = ".text.vdisk_rd")]
    fn block_count(&self) -> u32 {
        TOTAL
    }
    #[cfg_attr(target_os = "none", link_section = ".text.vdisk_rd")]
    fn read_only(&self) -> bool {
        crate::vwrite::read_only()
    }
    #[cfg_attr(target_os = "none", link_section = ".text.vdisk_rd")]
    fn gates_writes(&self) -> bool {
        true
    }
    #[cfg_attr(target_os = "none", link_section = ".text.vdisk_rd")]
    fn write_error(&self) -> u8 {
        crate::vwrite::last_error()
    }
    #[cfg_attr(target_os = "none", link_section = ".text.vdisk_rd")]
    fn product(&self) -> &'static [u8; 16] {
        b"DM40 files      "
    }
    #[cfg_attr(target_os = "none", link_section = ".text.vdisk_rd")]
    fn media_change_immediate(&self) -> bool {
        true
    }
    #[cfg_attr(target_os = "none", link_section = ".text.vdisk_rd")]
    fn read_block(&mut self, lba: u32, buf: &mut [u8]) -> bool {
        let t0 = rd_cyc();
        let ok = self.read_block_in(lba, buf);
        let dt = rd_cyc().wrapping_sub(t0);
        use core::sync::atomic::Ordering::Relaxed;
        RD[0].fetch_add(1, Relaxed);
        RD[1].fetch_add(dt >> 10, Relaxed);
        RD[2].fetch_max(dt, Relaxed);
        ok
    }
    #[inline(never)] // .cold (link-native.x): host writes only, not inlined into run
    fn write_block(&mut self, lba: u32, buf: &[u8]) -> bool {
        if state().is_none() || lba >= TOTAL || buf.len() < BS {
            return false;
        }
        crate::vwrite::write(lba, &buf[..BS])
    }
    #[cfg_attr(target_os = "none", link_section = ".text.vdisk_rd")]
    fn read_busy(&self, lba: u32, n: u32) -> bool {
        crate::hy3131::streaming() && !MIR_OK.load(core::sync::atomic::Ordering::Acquire) && state().is_some_and(|st| slow_overlap(st, lba, n))
    }
    #[cfg_attr(target_os = "none", link_section = ".text.vdisk_rd")]
    fn read_wait(&mut self, lba: u32) -> bool {
        mir_hold(lba)
    }
    #[cfg_attr(target_os = "none", link_section = ".text.vdisk_rd")]
    fn take_media_changed(&mut self) -> bool {
        CHANGED.swap(false, core::sync::atomic::Ordering::Relaxed)
    }
}

impl VDisk {
    #[cfg_attr(target_os = "none", link_section = ".text.vdisk_rd")]
    #[inline(always)]
    fn read_block_in(&mut self, lba: u32, buf: &mut [u8]) -> bool {
        let Some(st) = state() else {
            return false;
        };
        if lba >= TOTAL || buf.len() < BS {
            return false;
        }
        let buf = &mut buf[..BS];
        buf.fill(0);
        if lba == 0 {
            boot_sector(st, buf);
        } else if lba < ROOT_START {
            fat_sector(st, (lba - FAT_START) % FAT_SECS, buf);
        } else if lba < DATA_START {
            root_sector(lba - ROOT_START, buf);
        } else {
            return data_sector(st, lba - DATA_START, buf);
        }
        true
    }
}

#[inline(always)]
fn put16(b: &mut [u8], o: usize, v: u16) {
    b[o..o + 2].copy_from_slice(&v.to_le_bytes());
}

#[inline(always)]
fn put32(b: &mut [u8], o: usize, v: u32) {
    b[o..o + 4].copy_from_slice(&v.to_le_bytes());
}

fn boot_sector(st: &State, b: &mut [u8]) {
    b[..3].copy_from_slice(&[0xEB, 0x3C, 0x90]);
    b[3..11].copy_from_slice(b"OPENDM40");
    put16(b, 0x0B, BS as u16);
    b[0x0D] = SPC as u8;
    put16(b, 0x0E, RSVD as u16);
    b[0x10] = NFATS as u8;
    put16(b, 0x11, ROOT_ENTRIES as u16);
    if TOTAL <= 0xFFFF {
        put16(b, 0x13, TOTAL as u16);
    } else {
        put32(b, 0x20, TOTAL);
    }
    b[0x15] = 0xF8; // fixed media
    put16(b, 0x16, FAT_SECS as u16);
    put16(b, 0x18, 63); // sectors per track
    put16(b, 0x1A, 255); // heads
    b[0x24] = 0x80; // drive number
    b[0x25] = crate::vwrite::boot_state(); // the host's dirty flag (Linux)
    b[0x26] = 0x29; // extended boot signature
    // Volume serial: from the UID (stable per unit, differs between units;
    // hosts remember volumes by it).
    put32(b, 0x27, !crc32_upd(!0, &st.uid));
    b[0x2B..0x36].copy_from_slice(&LABEL);
    b[0x36..0x3E].copy_from_slice(b"FAT16   ");
    // Boot code: an endless loop (never booted).
    b[0x3E..0x40].copy_from_slice(&[0xEB, 0xFE]);
    b[0x1FE] = 0x55;
    b[0x1FF] = 0xAA;
}

/// FAT entry of cluster `c`: media / EOC marks, one-cluster directories and
/// README, the contiguous chains of the INFO / DEBUG files and of every
/// table file, 0 = free.
#[cfg_attr(target_os = "none", link_section = ".text.vdisk_rd")]
fn fat_entry(st: &State, c: u32) -> u16 {
    match c {
        0 => 0xFFF8,
        1 => crate::vwrite::fat1(),
        C_INFO..=C_README => 0xFFFF,
        _ if c < st.spi_start => fixed(st).iter().find_map(|f| chain(c, f.start, f.size)).unwrap_or(0),
        _ if c < st.dyn0 => chain(c, st.spi_start, SPI_BYTES).unwrap_or(0),
        _ => match crate::vwrite::pair(c) {
            Some(v) => v,
            None => at_cluster(tab(), c).filter(|e| e.id & T_FAT == 0).and_then(|e| chain(c, e.vstart as u32, e.vsize)).unwrap_or(0),
        },
    }
}

/// FAT entry of cluster `c` in a contiguous chain from `start` (`size`
/// bytes); None: not in it.
#[inline(always)]
fn chain(c: u32, start: u32, size: u32) -> Option<u16> {
    let end = start + nclus(size);
    (start != 0 && c >= start && c < end).then_some(if c + 1 == end { 0xFFFF } else { (c + 1) as u16 })
}

#[cfg_attr(target_os = "none", link_section = ".text.vdisk_rd")]
fn fat_sector(st: &State, s: u32, b: &mut [u8]) {
    let per = (BS / 2) as u32;
    for i in 0..per {
        put16(b, (i * 2) as usize, fat_entry(st, s * per + i));
    }
}

/// A short directory entry.
#[cfg_attr(target_os = "none", link_section = ".text.vdisk_rd")]
fn dirent(e: &mut [u8], name: &[u8; 11], attr: u8, start: u32, size: u32) {
    e[..11].copy_from_slice(name);
    e[11] = attr;
    put16(e, 0x10, DATE); // created
    put16(e, 0x12, DATE); // accessed
    put16(e, 0x18, DATE); // written
    put16(e, 0x1A, start as u16);
    put32(e, 0x1C, size);
}

/// Root entry `i` (0 = label).
#[cfg_attr(target_os = "none", link_section = ".text.vdisk_rd")]
fn root_entry(i: usize, e: &mut [u8]) {
    match i {
        0 => {
            e[..11].copy_from_slice(&LABEL);
            e[11] = 0x08; // volume label
            put16(e, 0x18, DATE);
        }
        1 => dirent(e, b"README  TXT", 0x01, C_README, readme(0, &mut [])),
        2 => dirent(e, b"INFO       ", 0x10, C_INFO, 0),
        3 => dirent(e, b"DEBUG      ", 0x10, C_DEBUG, 0),
        4..=6 => dirent(e, &VDIRS[i - 4].0, 0x10, C_VDIR + (i - 4) as u32, 0),
        _ => {}
    }
}

#[cfg_attr(target_os = "none", link_section = ".text.vdisk_rd")]

fn root_sector(s: u32, b: &mut [u8]) {
    let per = (BS / 32) as u32;
    for i in 0..per {
        let idx = (s * per + i) as usize;
        let e = &mut b[(i * 32) as usize..(i * 32 + 32) as usize];
        if idx <= ROOT_N {
            root_entry(idx, e);
        } else if let Some(r) = crate::vwrite::root_rec(idx) {
            e.copy_from_slice(&r);
        }
    }
}

/// Sector `s` of a one-cluster subdirectory at cluster `c`: "." / "..",
/// then its files by slot (a slot below the high-water mark without a file
/// = a deleted entry, 0xE5).
#[cfg_attr(target_os = "none", link_section = ".text.vdisk_rd")]
fn dir_sector(st: &State, c: u32, s: u32, b: &mut [u8]) {
    let per = (BS / 32) as u32;
    let (lo, hi) = (s * per, s * per + per);
    let ent = |slot: u32| {
        let o = ((slot - lo) * 32) as usize;
        o..o + 32
    };
    for slot in lo..hi.min(2) {
        let name = if slot == 0 { b".          " } else { b"..         " };
        let r = ent(slot);
        dirent(&mut b[r], name, 0x10, if slot == 0 { c } else { 0 }, 0);
    }
    match c {
        C_INFO => {
            for (i, f) in fixed(st).iter().enumerate() {
                let slot = i as u32 + 2;
                if (lo..hi).contains(&slot) {
                    let r = ent(slot);
                    dirent(&mut b[r], &f.name, 0x01, f.start, f.size);
                }
            }
        }
        C_DEBUG => {
            if (lo..hi).contains(&2) {
                let r = ent(2);
                dirent(&mut b[r], b"SPIFLASHBIN", 0x01, st.spi_start, SPI_BYTES);
            }
        }
        _ => {
            let k = (c - C_VDIR) as usize;
            let t = tab();
            for slot in lo.max(2)..hi.min(t.slot_hw[k] as u32) {
                let r = ent(slot);
                b[r.start] = 0xE5;
            }
            for e in live(t).iter().filter(|e| dir(kind(e.id)) == k && (lo..hi).contains(&(e.slot as u32))) {
                let r = ent(e.slot as u32);
                let o = r.start;
                dyn_entry(e, &mut b[r]);
                if e.id & T_DIR != 0 {
                    b[o] = 0xE5; // the host deleted it: as it wrote it
                }
            }
        }
    }
}

/// Data sector `s` (0 = the first sector of cluster 2). False: the slow-bank
/// mirror could not be read (`mir_read`).
#[cfg_attr(target_os = "none", link_section = ".text.vdisk_rd")]
fn data_sector(st: &State, s: u32, b: &mut [u8]) -> bool {
    let c = s / SPC + 2;
    let within = (s % SPC) * BS as u32;
    // (file start cluster, size): the bytes of this sector.
    let span = |start: u32, size: u32| -> Option<(u32, usize)> {
        let off = (c - start) * CB + within;
        (off < size).then(|| (off, (size - off).min(BS as u32) as usize))
    };
    match c {
        C_INFO..=C_DEBUG | C_VDIR..=6 => dir_sector(st, c, s % SPC, b),
        C_README => {
            if let Some((off, len)) = span(C_README, readme(0, &mut [])) {
                readme(off as usize, &mut b[..len]);
            }
        }
        _ if c < st.spi_start => {
            let Some(f) = fixed(st).iter().find(|f| f.start != 0 && c >= f.start && c < f.start + nclus(f.size)) else { return true };
            let Some((off, len)) = span(f.start, f.size) else { return true };
            let d = &mut b[..len];
            match f.kind {
                Kind::Mem => return mem_src(st, f.addr + off, d),
                Kind::Txt => {
                    describe(st, off as usize, d);
                }
                Kind::Fw => {
                    if let Some(fw) = st.fw.as_ref() {
                        return fw.fill_with(off, d, fw_body);
                    }
                }
            }
        }
        _ if c < st.dyn0 => {
            if let Some((off, len)) = span(st.spi_start, SPI_BYTES) {
                if !dlog::disk().read_at(off, &mut b[..len]) {
                    b.fill(0);
                }
            }
        }
        _ => {
            if crate::vwrite::staged_read(c, s % SPC, b) {
                return true; // a host file's data (vwrite.rs)
            }
            let Some(e) = at_cluster(tab(), c).copied() else { return true };
            if e.id & (T_FAT | T_SRC) != 0 {
                return true; // freed by the host / gone: zeros
            }
            if let Some((off, len)) = span(e.vstart as u32, e.vsize) {
                file_read(&e, off, &mut b[..len]);
            }
        }
    }
    true
}

// --- README.TXT -------------------------------------------------------------------

/// README.TXT's length: the text padded with spaces (the volume's layout,
/// and so the host writes captured for the tests, do not change with the
/// text or the version string).
const README_LEN: usize = 2048;

/// README.TXT bytes `base .. base + out.len()` into `out`; returns its
/// whole length (README_LEN).
#[cfg_attr(target_os = "none", link_section = ".text.vdisk_rd")]
fn readme(base: usize, out: &mut [u8]) -> u32 {
    let mut w = W { b: out, base, n: 0 };
    w.s(b"DM40 (opendm40 ");
    w.s(VERSION.as_bytes());
    w.s(b") USB disk\r\n\r\n");
    w.s(b"LOGS\\   DMM recordings (CSV)\r\n");
    w.s(b"SCOPE\\  scope records (CSV)\r\n");
    w.s(b"SHOTS\\  screenshots (BMP)\r\n");
    w.s(b"INFO\\   calibration, chip data, FIRMWARE.BIN (read-only)\r\n");
    w.s(b"DEBUG\\  SPIFLASH.BIN, the internal storage (read-only)\r\n\r\n");
    w.s(b"Manual: https://github.com/opendm40/opendm40/blob/main/docs/manual/README.md\r\n");
    debug_assert!(w.n + 2 <= README_LEN);
    while w.n + 2 < README_LEN {
        w.c(b' ');
    }
    w.s(b"\r\n");
    w.n as u32
}

// --- BACKUP.TXT ---------------------------------------------------------------

/// Text writer over a window: byte `n` of the text goes to `b[n - base]`
/// when that is inside `b`; `n` counts the whole text (its length at the end).
struct W<'a> {
    b: &'a mut [u8],
    base: usize,
    n: usize,
}

// Region B (`.text.vdisk_rd`): README.TXT (`readme`) is sized on every root
// directory read, maybe while a stream runs.
impl W<'_> {
    #[cfg_attr(target_os = "none", link_section = ".text.vdisk_rd")]
    fn c(&mut self, ch: u8) {
        if let Some(d) = self.n.checked_sub(self.base).and_then(|i| self.b.get_mut(i)) {
            *d = ch;
        }
        self.n += 1;
    }
    #[cfg_attr(target_os = "none", link_section = ".text.vdisk_rd")]
    fn s(&mut self, t: &[u8]) {
        for &ch in t {
            self.c(ch);
        }
    }
    #[cfg_attr(target_os = "none", link_section = ".text.vdisk_rd")]
    fn hex(&mut self, v: u32) {
        for i in (0..8).rev() {
            self.c(b"0123456789ABCDEF"[(v >> (4 * i)) as usize & 15]);
        }
    }
    /// Right-aligned in 7 columns.
    #[cfg_attr(target_os = "none", link_section = ".text.vdisk_rd")]
    fn dec(&mut self, mut v: u32) {
        let mut t = [b' '; 7];
        for i in (0..7).rev() {
            t[i] = b'0' + (v % 10) as u8;
            v /= 10;
            if v == 0 {
                break;
            }
        }
        self.s(&t);
    }
    /// 8.3 name as "NAME.EXT", padded to 13.
    fn name(&mut self, n: &[u8; 11]) {
        let start = self.n;
        self.s(n[..8].split(|&c| c == b' ').next().unwrap_or(&[]));
        self.c(b'.');
        self.s(n[8..].split(|&c| c == b' ').next().unwrap_or(&[]));
        while self.n < start + 13 {
            self.c(b' ');
        }
    }
}

/// BACKUP.TXT bytes `base .. base + out.len()` into `out`; returns the
/// text's whole length (constant for a given file table). In region B with
/// the rest of the volume's read path (`.text.vdisk_rd`, link-native.x): a
/// host may read BACKUP.TXT while a stream runs.
#[cfg_attr(target_os = "none", link_section = ".text.vdisk_rd")]
fn describe(st: &State, base: usize, out: &mut [u8]) -> usize {
    let mut w = W { b: out, base, n: 0 };
    w.s(b"DM40 INFO: generated on the fly by opendm40 ");
    w.s(VERSION.as_bytes());
    w.s(b" from the\r\nMCU memory, read-only. Raw copies of internal flash / chip data;\r\n");
    w.s(b"CRC-32 as zlib, 7-Zip, crc32.\r\n\r\n");
    w.s(b"file         address     bytes  crc32");
    // `fixed`, not a slice index: the outlined <[VFile; 9] as Index>::index
    // landed in region C (`.text.*vdisk*`) and ran on this read path while
    // a stream ran (emu/regionc.py, 2026-10-23).
    for f in fixed(st) {
        w.s(b"\r\n");
        w.name(&f.name);
        if f.kind == Kind::Txt {
            w.s(b"(this file)");
            continue;
        }
        w.hex(f.addr);
        w.s(b" ");
        w.dec(f.size);
        w.s(b"  ");
        w.hex(f.crc);
    }
    w.s(b"\r\n\r\nUID ");
    for ch in st.uid.chunks(4) {
        w.c(b' ');
        w.hex(u32::from_le_bytes([ch[0], ch[1], ch[2], ch[3]]));
    }
    w.s(b"\r\nFirmware opendm40 ");
    w.s(VERSION.as_bytes());
    match st.fw.as_ref() {
        Some(fw) => {
            w.s(b", loader record at 08008000:");
            for ch in fw.header().chunks(4) {
                w.c(b' ');
                w.hex(u32::from_le_bytes([ch[0], ch[1], ch[2], ch[3]]));
            }
            w.s(b"\r\n\r\nFIRMWARE.BIN = the running firmware as its vendor update file. To\r\n");
            w.s(b"reinstall it: tools/reload.sh --image FIRMWARE.BIN, or copy it as\r\n");
            w.s(b"dm40_opendm40_v");
            w.s(VERSION.as_bytes());
            w.s(b".bin to the vendor loader's own USB disk (see the manual).\r\n");
        }
        None => w.s(b"; no FIRMWARE.BIN (no valid loader record at 08008000).\r\n"),
    }
    w.s(b"\r\nFull restore after a mass erase (SWD): write ROMBOOT.BIN at 0x08000000,\r\n");
    w.s(b"then LOADER.BIN at 0x080A0000 and CALIB.BIN at 0x080C0000, then install\r\n");
    w.s(b"a firmware. BOOTDATA.BIN = LOADER.BIN bytes 0x4000..0x7FFF. The loader\r\n");
    w.s(b"data is bound to UID.BIN: this unit only.\r\n");
    w.s(b"Copy these files to a PC: they are made from the MCU flash and go\r\n");
    w.s(b"with it. The meter's SPI flash keeps one write-once safety copy of\r\n");
    w.s(b"CALIB.BIN, BOOTDATA.BIN and UID.BIN (written when missing, never\r\n");
    w.s(b"overwritten; it survives an SWD mass erase of the MCU).\r\n");
    debug_assert!(w.n <= TXT_MAX as usize);
    w.n
}


/// The host released the volume (Allow): a compaction it held back can run
/// now (msc.rs).
pub fn on_unlock() {
    if state().is_some() {
        if crate::vwrite::release() || tab().want_compact {
            dlog::set_stale();
        }
    }
}
