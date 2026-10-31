//! Write-once safety copy of the unit's irreplaceable data on the SPI-flash
//! FAT16 volume (the USB disk, LUN 0): CALIB.BIN, BOOTDATA.BIN and UID.BIN,
//! each written only when the root has no entry of that name, never
//! overwritten, never deleted.
//!
//! Why on the SPI disk at all: everything else is served read-only from the
//! MCU memory on the second USB LUN (vdisk.rs: CALIB, BOOTDATA, UID,
//! OPTBYTES, OTP, ROMBOOT, LOADER, BACKUP.TXT, FIRMWARE.BIN, always current,
//! no disk writes). But that volume is generated from the internal flash, and
//! the one recovery path for a unit whose flash is unusable, an SWD / BOOT0
//! session with read protection level 1, can only lift RDP by a MASS ERASE of
//! the whole internal flash, the calibration (0x080C0000) and the UID-bound
//! loader data (0x080A4000) included. The SPI flash is a separate chip and
//! survives that, so one copy of the calibration has to live there before it
//! is needed (docs/user-feedback.md UF-20; docs/native-boot.md "Calibration
//! safety copy"). Write-once: the first copy is the unit's factory state
//! (or the state of the first opendm40 boot); a later boot never replaces it
//! with whatever the flash holds then (e.g. a corrupted or foreign record),
//! the owner deletes it to have it made again.
//!
//! Files (root directory, 8.3 names, exact raw bytes; bit i of `written` /
//! `conflict` / `differs` = row i, = the first rows of `FILES`, which
//! vdisk.rs serves in full):
//!   0 CALIB.BIN     0x080C0000   4096  settings + calibration record (sector 10).
//!                   The stock record is 800 bytes (u16 crc, u16 ~crc, u32
//!                   version 1, data; calibration struct at +0x168, hy3131_v2_6.h);
//!                   in this unit's 1 MiB dump the whole 128 KiB sector is 0xFF
//!                   beyond 0x31F. 4 KiB = exactly one cluster / one flash block
//!                   and leaves room for a longer record of another stock version.
//!   1 BOOTDATA.BIN  0x080A4000  16384  UID-bound FS-loader data (sector 9). The
//!                   device-specific block starts at 0x080A4000 (2 KiB dump
//!                   dumps/dump_boot_data_uid.bin; the first 0x14 bytes are data,
//!                   b[0x10..0x13] = the version bytes BT command 02 reports).
//!   2 UID.BIN       0x1FFF7A10     12  96-bit unique ID (says which unit the
//!                   loader data belongs to).
//! The other FILES rows (OPTBYTES, OTP, ROMBOOT, LOADER) are only on the
//! information volume now. Earlier builds wrote all of them + BACKUP.TXT +
//! _DM40PRV.BIN to the SPI disk at every boot (compare-first, rewritten on
//! change); copies left by those builds are not touched (not updated, not
//! deleted: the owner's files). fatren.rs still renames an installed
//! dm40*.bin to _DM40PRV.BIN (loader re-install protection, not a backup).
//!
//! When: every normal boot, before USB exists (the host cannot have the volume
//! mounted): from native.rs `pre_main` (after board init and fatren). Cost of a boot with the copy present: the
//! boot sector, the root block, FAT 1 and the six data blocks of the three
//! files are read (the files are compared with the flash: `differs`, for
//! DBG:BOOT?), nothing is written. First boot: 6 data blocks + 2 FAT blocks
//! + 1 root block written.
//!
//! Write order (one pass, so a power loss anywhere leaves at worst lost
//! clusters, never a live entry on free or half-written clusters):
//!   1. allocate clusters first-fit from FAT 1;
//!   2. write the data blocks (read, overlay, write, read back);
//!   3. link the chains (EOC at the end) in every FAT copy, one write per
//!      touched FAT block and copy;
//!   4. directory entries (start cluster, size, date, archive bit), one write
//!      per touched root block;
//!   5. re-read every written file through its new entry and compare.
//! Every block write is verified by a read-back. The volume is not touched
//! unless fatren::parse_bpb accepts it (FAT16, 4096-byte sectors = device
//! block, sane geometry).
//!
//! The same writer also keeps a general `write_file` (compare, new chain,
//! entry switch, old chain freed, re-read; copy-on-write) and the helpers the
//! on-device writers use (screenshot.rs, csvlog.rs, gallery.rs:
//! `numbered_name`, `free_number`, `crc32_upd`; their block buffers are
//! diskbuf.rs `blocks`). `FwFile` (the running firmware as its update
//! file) is vdisk.rs's FIRMWARE.BIN.
//!
//! Host test: tools/fatren-test/run.sh.

#![allow(dead_code)]

use crate::fatren::{free_chain, le16, le32, parse_bpb, sfn_is, verify, Vol};
use crate::msc::BlockDevice;

const BLK: usize = 4096;
/// Most clusters one run allocates (the safety copy: 6; `write_file` up to
/// the largest firmware update file, 12 + 0x9C000 bytes = 157 clusters).
const MAX_NEW: usize = 160;
/// Root slots per 4 KiB block.
const PER: u32 = (BLK / 32) as u32;
/// Write date 2026-10-13 (FAT: (year - 1980) << 9 | month << 5 | day), time 0.
/// Constant (no RTC).
const DATE: u16 = (46 << 9) | (10 << 5) | 13;

/// The regions: 8.3 name, source address, length. vdisk.rs serves all of
/// them; the first `SAFE` are also the write-once copy on the SPI disk.
pub const FILES: [([u8; 11], u32, u32); 7] = [
    (*b"CALIB   BIN", 0x080C_0000, 4096),
    (*b"BOOTDATABIN", 0x080A_4000, 16384),
    (*b"UID     BIN", 0x1FFF_7A10, 12),
    (*b"OPTBYTESBIN", 0x1FFF_C000, 16),
    (*b"OTP     BIN", 0x1FFF_7800, 528),
    (*b"ROMBOOT BIN", 0x0800_0000, 16384),
    (*b"LOADER  BIN", 0x080A_0000, 131072),
];
/// FILES[..SAFE] = CALIB.BIN, BOOTDATA.BIN, UID.BIN: the safety copy.
pub const SAFE: usize = 3;
/// Most files one writer run handles.
const N: usize = FILES.len();
const _: () = assert!(N <= 16, "written / conflict / differs are u16 bit masks");

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Status {
    /// Not run (yet).
    NotRun,
    NoMedium,
    /// Boot sector is not the expected FAT16 volume; untouched.
    BadBpb,
    /// Every file already there (safety copy: present, whatever its
    /// contents; `write_file`: equal): nothing written.
    Present,
    /// The files in `written` were written and verified.
    Written,
    /// Root directory full or not enough free clusters; nothing written.
    NoSpace,
    /// A read, write or verify failed (see `written` for what completed).
    Error,
}

#[derive(Clone, Copy, Debug)]
pub struct Result {
    pub status: Status,
    /// Bit i = file i written (it was missing).
    pub written: u16,
    /// Bit i = the name is taken by a directory / volume label: skipped.
    pub conflict: u16,
    /// Bit i = a copy is present but differs from the source now (size or
    /// contents; safety copy only, which keeps it): e.g. the calibration was
    /// changed after the first copy. Informational (DBG:BOOT?).
    pub differs: u16,
    /// Old chains freed (clusters; `write_file` only).
    pub freed: u32,
    /// `boot()` duration in ms (DWT cycles at 200 MHz; 12.5x too low when
    /// the core runs at 16 MHz). 0 = not measured.
    pub ms: u32,
}

static mut RESULT: Result = Result { status: Status::NotRun, written: 0, conflict: 0, differs: 0, freed: 0, ms: 0 };

pub fn result() -> Result {
    unsafe { RESULT }
}

/// One file: 8.3 name, source address, contents.
#[derive(Clone, Copy)]
pub struct Src<'a> {
    pub name: [u8; 11],
    pub addr: u32,
    pub data: &'a dyn Source,
}

/// File contents for the writer: a byte slice, or bytes generated on demand
/// (screenshot.rs streams the LCD GRAM; nothing holds the whole file).
pub trait Source {
    /// File size, bytes.
    fn size(&self) -> u32;
    /// Copy bytes `off .. off + buf.len()` (never past `size`) into `buf`.
    /// Must return the same bytes every time (the writer reads them twice:
    /// to write and to verify).
    fn fill(&self, off: u32, buf: &mut [u8]);
    /// CRC-32 of the whole contents (vdisk.rs BACKUP.TXT).
    fn crc(&self) -> u32 {
        let mut t = [0u8; 256];
        let mut c = !0u32;
        let mut off = 0u32;
        while off < self.size() {
            let n = (self.size() - off).min(t.len() as u32) as usize;
            self.fill(off, &mut t[..n]);
            c = crc32_upd(c, &t[..n]);
            off += n as u32;
        }
        !c
    }
}

impl Source for &[u8] {
    fn size(&self) -> u32 {
        self.len() as u32
    }
    fn fill(&self, off: u32, buf: &mut [u8]) {
        let o = off as usize;
        buf.copy_from_slice(&self[o..o + buf.len()]);
    }
    fn crc(&self) -> u32 {
        crc32(self)
    }
}

// --- The running firmware as its update file ------------------------------------

/// Flash address of the update body (stock and opendm40) and of the loader
/// marker inside it; the body may reach at most up to the FS-loader.
pub const FW_BASE: u32 = 0x0800_4000;
pub const FW_MARKER_OFF: usize = 0x4000;
pub const FW_MAX_BODY: usize = 0x080A_0000 - 0x0800_4000;

/// FIRMWARE.BIN (vdisk.rs): the update file of the firmware that is running,
/// regenerated from flash, byte-identical to the dm40*.bin the FS-loader
/// installed (which the loader deletes after a good install). Decoded file =
/// the 12-byte header || body; the header is exactly the 12-byte record the
/// loader wrote to 0x08008000 after verifying it (its CRC covers word1 =
/// body length, word2 = type + body CRC); the body is flash 0x08004000 ..
/// +length, except its bytes 0x4000..0x400B (the marker itself, excluded
/// from the body CRC by the loader) which are 0xFF as in the packed file
/// (dm40fw.py pack; the vendor's dm40_v_2_6.bin has 0xFF there too). Then
/// encoded in 128-byte blocks (fatren::encode_block). Rename it to
/// dm40*.bin and the loader reinstalls exactly this firmware.
///
/// Whole blocks: opendm40 build.sh packs with `dm40fw.py pack --pad` (body padded
/// with 0xFF so the update file, 12 + word1 bytes, is a multiple of 128;
/// word1 and the body CRC include the padding), and this file is 12 + word1
/// bytes, so an image built that way gives full blocks only. The
/// header cannot be changed here (it is the loader's record of the installed
/// image), so an image with a short last block is reproduced as it is, unless
/// that block is one the FS-loader mis-decodes (`tail_hazard`): then there is
/// no firmware file at all (from_flash = None).
#[derive(Clone, Copy)]
pub struct FwFile<'a> {
    hdr: [u8; 12],
    body: &'a [u8],
}

impl<'a> FwFile<'a> {
    /// `flash` = the bytes from 0x08004000 (at most FW_MAX_BODY). None when
    /// the marker is not a valid update header (fatren::header_valid) or its
    /// length does not fit.
    pub fn from_flash(flash: &'a [u8]) -> Option<Self> {
        let m = flash.get(FW_MARKER_OFF..FW_MARKER_OFF + 12)?;
        let mut hdr = [0u8; 12];
        hdr.copy_from_slice(m);
        if !crate::fatren::header_valid(&hdr) {
            return None;
        }
        let len = crate::fatren::le32(&hdr, 4) as usize;
        if len < FW_MARKER_OFF + 12 || len > FW_MAX_BODY || len > flash.len() {
            return None;
        }
        let f = FwFile { hdr, body: &flash[..len] };
        if f.tail_hazard() {
            return None;
        }
        Some(f)
    }

    /// The decoded 12-byte update header (= the loader record at 0x08008000).
    pub fn header(&self) -> &[u8; 12] {
        &self.hdr
    }

    /// The FS-loader hazard (dm40fw.py tail_hazard): the file's last block is
    /// short with 0x1F < n < 128 (n = size % 128) and its key position kp
    /// (a = plain byte 0x1F of the block, kp = a if a < 128 else a >> 1) is
    /// >= n. The encoder stores such a block as is, but the real FS-loader
    /// decodes it with a stale key from its 128-byte buffer: the body CRC
    /// fails, "update error", the app is erased. Safe: whole blocks, n <= 0x1F
    /// (stored as is by both sides), kp < n (the vendor's dm40_v_2_6.bin: 36
    /// bytes, kp 8).
    pub fn tail_hazard(&self) -> bool {
        let size = self.size() as usize;
        let n = size % 128;
        if n <= 0x1F {
            return false;
        }
        let a = self.plain(size - n + 0x1F);
        let kp = if a < 128 { a } else { a >> 1 } as usize;
        kp >= n
    }

    /// Decoded (plain) file byte `p`.
    #[cfg_attr(target_os = "none", link_section = ".text.vdisk_rd")]
    fn plain(&self, p: usize) -> u8 {
        if p < 12 {
            self.hdr[p]
        } else if (FW_MARKER_OFF..FW_MARKER_OFF + 12).contains(&(p - 12)) {
            0xFF
        } else {
            self.body[p - 12]
        }
    }
}

impl Source for FwFile<'_> {
    fn size(&self) -> u32 {
        12 + self.body.len() as u32
    }
    /// The USB volume's per-sector read of INFO/FIRMWARE.BIN (vdisk.rs):
    /// region B with the rest of that path (`.text.vdisk_rd`), as a host
    /// may read it during a fast-log session or the Wave view (device
    /// 825080c: max_lat 4.66 us here while it was region C).
    #[cfg_attr(target_os = "none", link_section = ".text.vdisk_rd")]
    fn fill(&self, off: u32, buf: &mut [u8]) {
        self.fill_with(off, buf, body_copy);
    }
}

/// Reads body bytes `q .. q + dst.len()` of an update file into `dst`
/// (`FwFile::fill_with`; `body` = the body as mapped). False: they cannot
/// be read now.
pub type BodyRead = fn(body: &[u8], q: usize, dst: &mut [u8]) -> bool;

/// The plain `BodyRead`: the bytes of `body`.
#[cfg_attr(target_os = "none", link_section = ".text.vdisk_rd")]
fn body_copy(body: &[u8], q: usize, dst: &mut [u8]) -> bool {
    dst.copy_from_slice(&body[q..q + dst.len()]);
    true
}

impl FwFile<'_> {
    /// `Source::fill` with the body bytes from `rd` (vdisk.rs: the slow
    /// bank's part from its SPI mirror while the AD2 stream runs). Only the
    /// 128-byte blocks made of body bytes alone go through `rd`; the header
    /// block and the marker's (both far below 0x08080000) read `body`.
    /// False: `rd` failed (`buf` partly filled).
    #[cfg_attr(target_os = "none", link_section = ".text.vdisk_rd")]
    pub fn fill_with(&self, off: u32, buf: &mut [u8], rd: BodyRead) -> bool {
        let size = self.size() as usize;
        let (start, end) = (off as usize, off as usize + buf.len());
        let mut p = start;
        while p < end {
            let b0 = p / 128 * 128;
            let blen = (size - b0).min(128);
            let mut blk = [0u8; 128];
            let q = b0.wrapping_sub(12); // body offset of the block start
            if b0 >= 12 && (q + blen <= FW_MARKER_OFF || q >= FW_MARKER_OFF + 12) {
                if !rd(self.body, q, &mut blk[..blen]) {
                    return false;
                }
            } else {
                for (i, d) in blk[..blen].iter_mut().enumerate() {
                    *d = self.plain(b0 + i);
                }
            }
            crate::fatren::encode_block(&mut blk[..blen]);
            let e = (end - b0).min(blen);
            buf[p - start..b0 + e - start].copy_from_slice(&blk[p - b0..e]);
            p = b0 + e;
        }
        true
    }
}

// --- Boot: the write-once safety copy ---------------------------------------------

/// Boot entry: the safety copy of FILES[..SAFE].
/// Never inlined: its frame is gone before main() runs (review 2026-10-13 B1).
#[inline(never)]
pub fn boot() {
    let t0 = unsafe { crate::mmio::r32(DWT_CYCCNT) };
    let mut slices: [&[u8]; SAFE] = [&[]; SAFE];
    for (s, &(_, addr, len)) in slices.iter_mut().zip(FILES.iter()) {
        // Read-only views of internal flash / system memory.
        *s = unsafe { core::slice::from_raw_parts(addr as *const u8, len as usize) };
    }
    static EMPTY: &[u8] = &[];
    let mut srcs = [Src { name: [0; 11], addr: 0, data: &EMPTY }; SAFE];
    for ((s, &(name, addr, _)), d) in srcs.iter_mut().zip(FILES.iter()).zip(slices.iter()) {
        *s = Src { name, addr, data: d };
    }
    let mut disk = crate::spiflash::SpiFlash::new();
    safe_copy(&mut disk, &srcs);
    let cyc = unsafe { crate::mmio::r32(DWT_CYCCNT) }.wrapping_sub(t0);
    unsafe { RESULT.ms = cyc / (crate::clock::HCLK_HZ / 1000) };
}

const DWT_CYCCNT: u32 = 0xE000_1004;

/// Write each of `srcs` whose name has no root entry yet; leave every
/// present one as it is (`differs` says which no longer equal their source).
/// Records and returns the result.
pub fn safe_copy(dev: &mut dyn BlockDevice, srcs: &[Src]) -> Result {
    static EMPTY: &[u8] = &[];
    let mut fs = [F { name: [0; 11], src: &EMPTY }; N];
    let k = srcs.len().min(N);
    for (f, s) in fs.iter_mut().zip(srcs[..k].iter()) {
        *f = F { name: s.name, src: s.data };
    }
    let mut r = run_inner(dev, &fs[..k], true);
    r.ms = unsafe { RESULT.ms }; // previous value; boot() stores this run's after
    unsafe { RESULT = r };
    r
}

/// A file for run_inner: 8.3 name + contents.
#[derive(Clone, Copy)]
struct F<'a> {
    name: [u8; 11],
    src: &'a dyn Source,
}

/// Write one file `name` (8.3, upper case) from `src` through the same
/// ordered copy-on-write path as the backup: unchanged = nothing written; new
/// or different = new clusters, data, FAT links, directory entry, old chain
/// freed, re-read and compared with `src`. Does not touch `result()`.
pub fn write_file(dev: &mut dyn BlockDevice, name: [u8; 11], src: &dyn Source) -> Result {
    run_inner(dev, &[F { name, src }], false)
}

/// Where root file `name` (8.3, upper case; the first live non-directory
/// entry, as `write_file` finds it) lies: (first data block, blocks per
/// cluster, file size, clusters), its chain through FAT 1 into `out`.
/// None: no medium / volume, no such file, a malformed chain (a bad link,
/// no EOC where the size ends) or more clusters than `out` holds.
pub fn chain_of(dev: &mut dyn BlockDevice, name: &[u8; 11], out: &mut [u16]) -> Option<(u32, u32, u32, usize)> {
    if !dev.present() || dev.block_size() as usize != BLK {
        return None;
    }
    let [buf, _, fat] = crate::diskbuf::blocks();
    if !dev.read_block(0, buf) {
        return None;
    }
    let v = parse_bpb(&buf[..], dev.block_count())?;
    let mut ent = None;
    'scan: for blk in 0..v.root_blocks {
        if !dev.read_block(v.root_start + blk, buf) {
            return None;
        }
        for k in 0..PER {
            if blk * PER + k >= v.root_entries {
                break 'scan;
            }
            let e = &buf[(k * 32) as usize..(k * 32 + 32) as usize];
            if e[0] == 0x00 {
                break 'scan;
            }
            if e[0] == 0xE5 || e[11] & 0x3F == 0x0F || e[11] & 0x18 != 0 {
                continue;
            }
            if sfn_is(e, name) {
                ent = Some((le16(e, 0x1A), le32(e, 0x1C)));
                break 'scan;
            }
        }
    }
    let (start, size) = ent?;
    let n = nclusters(&v, size) as usize;
    if n == 0 || n > out.len() {
        return None;
    }
    let mut cur = u32::MAX;
    let mut c = start;
    for (i, o) in out[..n].iter_mut().enumerate() {
        if c < 2 || c > v.clusters + 1 {
            return None;
        }
        *o = c as u16;
        let next = fat_get(dev, &v, c, fat, &mut cur)?;
        if i + 1 == n {
            if next < 0xFFF8 {
                return None;
            }
        } else {
            c = next;
        }
    }
    Some((v.data_start, v.spc, size, n))
}

/// 8.3 name `<prefix>NNN.<ext>` (prefix 1..5 upper-case chars, NNN = `n` with
/// three digits), e.g. ("S", 7, "BMP") -> "S007    BMP" = S007.BMP.
#[cfg_attr(target_os = "none", link_section = ".text.vdisk_rd")]
pub fn numbered_name(prefix: &[u8], n: u16, ext: &[u8; 3]) -> [u8; 11] {
    let mut name = [b' '; 11];
    let k = prefix.len().min(5);
    name[..k].copy_from_slice(&prefix[..k]);
    name[k] = b'0' + (n / 100 % 10) as u8;
    name[k + 1] = b'0' + (n / 10 % 10) as u8;
    name[k + 2] = b'0' + (n % 10) as u8;
    name[8..].copy_from_slice(ext);
    name
}

/// Lowest n in 0..=999 with no root entry `numbered_name(prefix, n, ext)`
/// (any entry of that name counts, also a directory; deleted and LFN slots do
/// not) - the stock screenshot naming "s%.3d.bmp", first free number. None:
/// no medium, not the expected FAT16 volume, a read error, or all 1000 taken.
pub fn free_number(dev: &mut dyn BlockDevice, prefix: &[u8], ext: &[u8; 3]) -> Option<u16> {
    if !dev.present() || dev.block_size() as usize != BLK {
        return None;
    }
    let buf = &mut crate::diskbuf::blocks()[0];
    if !dev.read_block(0, buf) {
        return None;
    }
    let v = parse_bpb(buf, dev.block_count())?;
    let k = prefix.len().min(5);
    let mut used = [0u32; 32]; // bit n = number n taken (0..=999)
    'scan: for blk in 0..v.root_blocks {
        if !dev.read_block(v.root_start + blk, buf) {
            return None;
        }
        for i in 0..PER {
            if blk * PER + i >= v.root_entries {
                break 'scan;
            }
            let e = &buf[(i * 32) as usize..(i * 32 + 32) as usize];
            if e[0] == 0x00 {
                break 'scan; // end of directory
            }
            if e[0] == 0xE5 || e[11] & 0x3F == 0x0F {
                continue;
            }
            let d = &e[k..k + 3];
            if !d.iter().all(|c| c.is_ascii_digit()) {
                continue;
            }
            let n = (d[0] - b'0') as u16 * 100 + (d[1] - b'0') as u16 * 10 + (d[2] - b'0') as u16;
            if sfn_is(e, &numbered_name(prefix, n, ext)) {
                used[(n / 32) as usize] |= 1 << (n % 32);
            }
        }
    }
    (0..1000u16).find(|&n| used[(n / 32) as usize] & (1 << (n % 32)) == 0)
}

// --- CRC-32 ----------------------------------------------------------------------

/// CRC-32 (ISO-HDLC, as zlib / `crc32` / 7-Zip report it). Nibble table
/// (64 bytes): ~4x the bit loop; vdisk.rs runs it over ~570 KiB at boot.
pub fn crc32(d: &[u8]) -> u32 {
    !crc32_upd(!0, d)
}

/// CRC-32 register update (no pre / post inversion). Region B: the same
/// code as dlog::crc32_upd, and LTO merged the two into this symbol, so
/// dlog's callers (a host reading a CSV off the volume while a stream runs)
/// called it in region C (wavetest's region C check).
#[cfg_attr(target_os = "none", link_section = ".text.dlog_row")]
pub(crate) fn crc32_upd(mut c: u32, d: &[u8]) -> u32 {
    const T: [u32; 16] = {
        let mut t = [0u32; 16];
        let mut i = 0;
        while i < 16 {
            let mut c = i as u32;
            let mut k = 0;
            while k < 4 {
                c = (c >> 1) ^ (0xEDB8_8320 & (c & 1).wrapping_neg());
                k += 1;
            }
            t[i] = c;
            i += 1;
        }
        t
    };
    for &b in d {
        c ^= b as u32;
        c = (c >> 4) ^ T[(c & 15) as usize];
        c = (c >> 4) ^ T[(c & 15) as usize];
    }
    c
}

// --- FAT writer --------------------------------------------------------------------

/// Bytes per cluster.
fn cbytes(v: &Vol) -> usize {
    v.spc as usize * BLK
}

fn nclusters(v: &Vol, len: u32) -> u32 {
    len.div_ceil(cbytes(v) as u32)
}

fn lba_of(v: &Vol, c: u32) -> u32 {
    v.data_start + (c - 2) * v.spc
}

/// FAT 1 entry `c`, with `fat`/`cur` as a one-block cache.
fn fat_get(dev: &mut dyn BlockDevice, v: &Vol, c: u32, fat: &mut [u8; BLK], cur: &mut u32) -> Option<u32> {
    let per = (BLK / 2) as u32;
    let fb = c / per;
    if fb >= v.fat_blocks {
        return None;
    }
    if fb != *cur {
        if !dev.read_block(v.fat_start + fb, fat) {
            return None;
        }
        *cur = fb;
    }
    Some(le16(fat, ((c % per) * 2) as usize))
}

/// Follow the chain of a file (`start`, `size` from its entry) through FAT 1.
/// Returns (well_formed, equal): well formed = exactly the clusters `size`
/// needs, every link valid, EOC at the end; equal = also well formed,
/// size == data.size() and the contents equal `data`. None = read error.
#[allow(clippy::too_many_arguments)]
fn walk(dev: &mut dyn BlockDevice, v: &Vol, start: u32, size: u32, data: &dyn Source, fat: &mut [u8; BLK], buf: &mut [u8; BLK], tmp: &mut [u8; BLK]) -> Option<(bool, bool)> {
    let n = nclusters(v, size);
    let max = v.clusters + 1;
    let len = data.size() as usize;
    if n == 0 {
        return Some((start == 0, start == 0 && len == 0));
    }
    if n > v.clusters {
        return Some((false, false));
    }
    let mut equal = size as usize == len;
    let mut cur = u32::MAX;
    let mut c = start;
    for i in 0..n {
        if c < 2 || c > max {
            return Some((false, false));
        }
        if equal {
            for b in 0..v.spc {
                let off = i as usize * cbytes(v) + b as usize * BLK;
                if off >= len {
                    break;
                }
                if !dev.read_block(lba_of(v, c) + b, buf) {
                    return None;
                }
                let end = (off + BLK).min(len);
                data.fill(off as u32, &mut tmp[..end - off]);
                if buf[..end - off] != tmp[..end - off] {
                    equal = false;
                    break;
                }
            }
        }
        crate::wdt::kick_if_feeding();
        let next = fat_get(dev, v, c, fat, &mut cur)?;
        if i + 1 == n {
            let ok = next >= 0xFFF8;
            return Some((ok, ok && equal));
        }
        c = next;
    }
    Some((false, false))
}

#[derive(Clone, Copy)]
struct Job {
    file: usize,
    /// Root slot (existing entry, or the free slot for a new one).
    slot: u32,
    new: bool,
    /// Old chain to free after the entry switch (0 = none / not well formed).
    old: u32,
    /// New clusters: cl[first..first + n].
    first: usize,
    n: usize,
}

const NO_JOB: Job = Job { file: 0, slot: 0, new: false, old: 0, first: 0, n: 0 };

/// The writer. `missing_only` (safety copy): a file whose name is present is
/// never rewritten, only compared (`differs`); else (`write_file`) a present
/// file that differs is replaced copy-on-write and its old chain freed.
fn run_inner(dev: &mut dyn BlockDevice, files: &[F], missing_only: bool) -> Result {
    let mut res = Result { status: Status::Error, written: 0, conflict: 0, differs: 0, freed: 0, ms: 0 };
    if !dev.present() || dev.block_size() as usize != BLK {
        res.status = Status::NoMedium;
        return res;
    }
    let [buf, tmp, fat] = crate::diskbuf::blocks();
    if !dev.read_block(0, buf) {
        return res;
    }
    let Some(v) = parse_bpb(&buf[..], dev.block_count()) else {
        res.status = Status::BadBpb;
        return res;
    };

    // Root scan: the live entry of each name (slot, start, size), free slots.
    let mut found: [Option<(u32, u32, u32)>; N] = [None; N];
    let mut free = [0u32; N];
    let mut nfree = 0usize;
    'scan: for blk in 0..v.root_blocks {
        if !dev.read_block(v.root_start + blk, buf) {
            return res;
        }
        for k in 0..PER {
            let idx = blk * PER + k;
            if idx >= v.root_entries {
                break 'scan;
            }
            let e = &buf[(k * 32) as usize..(k * 32 + 32) as usize];
            if e[0] == 0x00 {
                // End of directory: this and every later slot are free (FAT
                // spec: all entries after a 0x00 one are 0x00 too).
                let mut i = idx;
                while nfree < N && i < v.root_entries {
                    free[nfree] = i;
                    nfree += 1;
                    i += 1;
                }
                break 'scan;
            }
            if e[0] == 0xE5 {
                if nfree < N {
                    free[nfree] = idx;
                    nfree += 1;
                }
                continue;
            }
            if e[11] & 0x3F == 0x0F {
                continue; // LFN
            }
            for (i, f) in files.iter().enumerate() {
                if found[i].is_none() && res.conflict & (1 << i) == 0 && sfn_is(e, &f.name) {
                    if e[11] & 0x18 != 0 {
                        res.conflict |= 1 << i; // a directory / label of that name
                    } else {
                        found[i] = Some((idx, le16(e, 0x1A), le32(e, 0x1C)));
                    }
                }
            }
        }
    }

    // Which files differ? Plan new chains for them.
    let mut jobs = [NO_JOB; N];
    let mut nj = 0usize;
    let mut need = 0usize;
    let mut nnew = 0usize;
    for (i, f) in files.iter().enumerate() {
        if res.conflict & (1 << i) != 0 {
            continue;
        }
        let mut j = Job { file: i, first: need, n: nclusters(&v, f.src.size()) as usize, ..NO_JOB };
        match found[i] {
            Some((slot, start, size)) => {
                let Some((wf, eq)) = walk(dev, &v, start, size, f.src, fat, buf, tmp) else {
                    return res;
                };
                if !eq {
                    res.differs |= 1 << i;
                }
                if eq || missing_only {
                    continue;
                }
                j.slot = slot;
                j.old = if wf { start } else { 0 };
            }
            None => {
                if nnew == nfree {
                    res.status = Status::NoSpace;
                    return res;
                }
                j.slot = free[nnew];
                j.new = true;
                nnew += 1;
            }
        }
        need += j.n;
        jobs[nj] = j;
        nj += 1;
    }
    if nj == 0 {
        res.status = Status::Present;
        return res;
    }
    if need > MAX_NEW {
        res.status = Status::NoSpace;
        return res;
    }

    // 1. First-fit free clusters (FAT 1 entry 0).
    let mut cl = [0u32; MAX_NEW];
    let mut got = 0usize;
    let mut cur = u32::MAX;
    let mut c = 2u32;
    while got < need && c <= v.clusters + 1 {
        match fat_get(dev, &v, c, fat, &mut cur) {
            None => return res,
            Some(0) => {
                cl[got] = c;
                got += 1;
            }
            Some(_) => {}
        }
        c += 1;
    }
    if got < need {
        res.status = Status::NoSpace;
        return res;
    }

    // 2. Data blocks (the bytes past the end keep what the block held).
    for j in &jobs[..nj] {
        let d = files[j.file].src;
        let len = d.size() as usize;
        for (i, &c) in cl[j.first..j.first + j.n].iter().enumerate() {
            for b in 0..v.spc {
                let off = i * cbytes(&v) + b as usize * BLK;
                if off >= len {
                    break;
                }
                let lba = lba_of(&v, c) + b;
                let end = (off + BLK).min(len);
                // A partial last block keeps what it held past the end.
                if end - off < BLK && !dev.read_block(lba, buf) {
                    return res;
                }
                d.fill(off as u32, &mut buf[..end - off]);
                // A file's data is being (re)written: at boot only on the
                // first start after an update or a calibration change (the
                // INFO/ mirror, the safety copy, the settings file), which is
                // slow: the splash says so (once; nothing after the splash).
                #[cfg(target_os = "none")]
                crate::native::splash::note();
                if !(dev.write_block(lba, &buf[..]) && verify(dev, lba, buf, tmp)) {
                    return res;
                }
                crate::wdt::kick_if_feeding();
            }
        }
    }

    // 3. Chains in every FAT copy, one write per touched block and copy.
    let per = (BLK / 2) as u32;
    for k in 0..v.nfats {
        let mut done = 0u32; // FAT blocks already written (bit = block, < 32)
        for &c0 in &cl[..need] {
            let fb = c0 / per;
            if fb < 32 && done & (1 << fb) != 0 {
                continue;
            }
            let lba = v.fat_start + k * v.fat_blocks + fb;
            if !dev.read_block(lba, buf) {
                return res;
            }
            for j in &jobs[..nj] {
                for i in 0..j.n {
                    let c = cl[j.first + i];
                    if c / per == fb {
                        let next = if i + 1 == j.n { 0xFFFF } else { cl[j.first + i + 1] as u16 };
                        let o = ((c % per) * 2) as usize;
                        buf[o..o + 2].copy_from_slice(&next.to_le_bytes());
                    }
                }
            }
            if !(dev.write_block(lba, &buf[..]) && verify(dev, lba, buf, tmp)) {
                return res;
            }
            if fb < 32 {
                done |= 1 << fb;
            }
            crate::wdt::kick_if_feeding();
        }
    }

    // 4. Directory entries, one write per touched root block.
    for blk in 0..v.root_blocks {
        let (lo, hi) = (blk * PER, blk * PER + PER);
        if !jobs[..nj].iter().any(|j| j.slot >= lo && j.slot < hi) {
            continue;
        }
        let lba = v.root_start + blk;
        if !dev.read_block(lba, buf) {
            return res;
        }
        for j in jobs[..nj].iter().filter(|j| j.slot >= lo && j.slot < hi) {
            let o = ((j.slot - lo) * 32) as usize;
            let e = &mut buf[o..o + 32];
            if j.new {
                e.fill(0);
                e[..11].copy_from_slice(&files[j.file].name);
                e[0x10..0x12].copy_from_slice(&DATE.to_le_bytes()); // created
            }
            e[11] |= 0x20; // archive
            e[0x12..0x14].copy_from_slice(&DATE.to_le_bytes()); // accessed
            e[0x14..0x16].fill(0); // cluster high (FAT32 only)
            e[0x16..0x18].fill(0); // write time
            e[0x18..0x1A].copy_from_slice(&DATE.to_le_bytes()); // write date
            let start = if j.n == 0 { 0 } else { cl[j.first] as u16 };
            e[0x1A..0x1C].copy_from_slice(&start.to_le_bytes());
            e[0x1C..0x20].copy_from_slice(&files[j.file].src.size().to_le_bytes());
        }
        if !(dev.write_block(lba, &buf[..]) && verify(dev, lba, buf, tmp)) {
            return res;
        }
        crate::wdt::kick_if_feeding();
    }
    for j in &jobs[..nj] {
        res.written |= 1 << j.file;
    }

    // 5. Old chains (well-formed ones only).
    for j in &jobs[..nj] {
        if j.old != 0 {
            let (ok, freed) = free_chain(dev, &v, j.old, fat, buf, tmp);
            res.freed += freed;
            if !ok {
                return res;
            }
        }
    }

    // 6. Read every written file back through its new chain.
    for j in &jobs[..nj] {
        let d = files[j.file].src;
        let start = if j.n == 0 { 0 } else { cl[j.first] };
        match walk(dev, &v, start, d.size(), d, fat, buf, tmp) {
            Some((true, true)) => {}
            _ => return res,
        }
    }
    res.status = Status::Written;
    res
}
