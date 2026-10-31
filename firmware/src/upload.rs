//! Firmware update file upload over USBTMC (tools/dm40dbg.py upload,
//! tools/reload.sh): the PC can no longer write the SPI disk over USB (the
//! USB volume is generated and read-only, vdisk.rs), so the update file
//! that the vendor FS-loader installs from the SPI disk's root (any
//! dm40*.bin) arrives over SCPI instead and is written here as DM40FW.BIN.
//! Then DBG:UPDATE (zero the loader marker + reset) makes the loader
//! install it (and delete it). The recovery path is untouched: the loader's
//! own update screen exposes its own USB disk.
//!
//!   DBG:UPLoad:BEGin <size>,<crc32>   every dm40*.bin in the root is deleted
//!                                     (fatren::delete_updates), the upload
//!                                     starts; size 128 .. MAX bytes, CRC-32
//!                                     (zlib) of the whole file, decimal;
//!   DBG:UPLOAD:DATA <offset>,<bytes>  raw binary after the comma up to the
//!                                     end of the USBTMC message (main.rs
//!                                     takes it before the SCPI parser, which
//!                                     would upper-case / cut it); offset =
//!                                     bytes received so far (else "Data out
//!                                     of order");
//!   DBG:UPLoad:END?                   -> <size>,<crc32> once the file is
//!                                     complete, verified and in the root;
//!   DBG:UPLoad:ABORt, DBG:UPLoad?     -> <state 0 idle 1 active>,<received>,<size>.
//! Errors go to SYST:ERR? (the upload is then aborted; the first error
//! is kept: later DATA is dropped silently, END? reports it).
//!
//! Writing: the data is collected in diskbuf::blocks()[0]; every full
//! 4 KiB block goes to the next free cluster (first fit, ascending, FAT 1
//! untouched: `write_block` + read back). At END the last block (0xFF
//! padded) is written, every block re-read and the CRC-32 compared, the
//! first block's decoded header must be a valid update header
//! (fatren::header_valid), then the chain in every FAT copy (FAT 1 first),
//! then the root entry DM40FW.BIN (archive, size): data -> FAT -> directory,
//! a power cut leaves at worst lost clusters. While an upload is active it
//! owns the disk and blocks(): screenshot.rs / csvlog.rs / gallery.rs wait
//! (`busy`), and BEGIN is refused while one of them writes. An upload with
//! no DATA for TIMEOUT_MS is dropped (`poll`; nothing to undo: the clusters
//! were never linked).
//!
//! Host test: tools/fatren-test/run.sh "upload".

use crate::calbackup;
use crate::diskbuf;
use crate::fatren::{self, le16, parse_bpb, verify};
use crate::msc::BlockDevice;

const BLK: usize = 4096;
const FPER: u32 = (BLK / 2) as u32;
/// Largest update file: 12-byte header + the app area 0x08004000..0x080A0000.
pub const MAX: u32 = 12 + calbackup::FW_MAX_BODY as u32;
/// Clusters of the largest file (4 KiB clusters).
const MAXCL: usize = (MAX as usize).div_ceil(BLK);
pub const NAME: [u8; 11] = *b"DM40FW  BIN";
/// An active upload without DATA for this long is dropped.
pub const TIMEOUT_MS: u64 = 30_000;
/// Directory date 2026-10-13 (no RTC), as the other writers.
const DATE: u16 = (46 << 9) | (10 << 5) | 13;

/// Upload state. Zero = idle (SRAM, .sram_bss: zeroed at reset).
struct Up {
    active: bool,
    size: u32,
    crc: u32,
    got: u32,
    /// CRC-32 register over the bytes received.
    run: u32,
    /// Clusters written so far (ascending), the next one to try.
    cl: [u16; MAXCL],
    ncl: usize,
    hint: u32,
    /// Blocks written (spc blocks per cluster).
    blocks: u32,
    /// Bytes of the current block in blocks()[0].
    fill: usize,
    t_last: u64,
    /// The first error of the upload (it ended there): later DATA is
    /// dropped silently and END? reports this one, so SYST:ERR? shows the
    /// cause, not a "not started" of the next chunk.
    err: Option<&'static str>,
}

// Backup SRAM (.bkp_bss, link-native.x; CPU only, zeroed at reset;
// docs/native-boot.md "Extra RAM areas").
#[cfg_attr(target_os = "none", link_section = ".bkp_bss")]
static mut UP: Up = Up { active: false, size: 0, crc: 0, got: 0, run: 0, cl: [0; MAXCL], ncl: 0, hint: 0, blocks: 0, fill: 0, t_last: 0, err: None };
/// The device's own SpiFlash handle (as screenshot.rs).
#[cfg(target_os = "none")]
static mut DISK: Option<crate::spiflash::SpiFlash> = None;

#[inline(always)]
fn up() -> &'static mut Up {
    unsafe { &mut *core::ptr::addr_of_mut!(UP) }
}

/// An upload owns the SPI disk (and diskbuf::blocks()): this one over
/// SCPI, or a firmware file being copied onto the USB volume (vwrite.rs
/// staging). Inlined: diskbuf::fft_work (the scope FFT, every pass) asks
/// it, and this module is .cold.
#[inline(always)]
pub fn busy() -> bool {
    up().active || staging()
}

#[cfg(feature = "vdisk")]
#[inline(always)]
fn staging() -> bool {
    crate::vwrite::staging()
}

#[cfg(not(feature = "vdisk"))]
#[inline(always)]
fn staging() -> bool {
    false
}

/// The SCPI upload (not the USB volume's staging) is active.
pub fn scpi_busy() -> bool {
    up().active
}

/// Inlined: vwrite::poll reaches it on every main-loop pass (through
/// `spi`) and this module is in .cold (region C, link-native.x).
#[cfg(target_os = "none")]
#[inline(always)]
fn disk() -> &'static mut crate::spiflash::SpiFlash {
    let d = unsafe { &mut *core::ptr::addr_of_mut!(DISK) };
    d.get_or_insert_with(crate::spiflash::SpiFlash::new)
}

/// The SPI flash as a block device (vwrite.rs).
#[cfg(target_os = "none")]
#[inline(always)]
pub fn spi() -> &'static mut dyn BlockDevice {
    disk()
}

/// Drop the upload (nothing on the disk to undo).
pub fn abort() {
    let u = up();
    u.active = false;
    u.err = None;
}

/// End the upload with error `e` (the first one is kept, see `Up::err`).
fn fail(e: &'static str) -> &'static str {
    let u = up();
    u.active = false;
    *u.err.get_or_insert(e)
}

/// Main loop: drop a stalled upload.
pub fn poll(now_ms: u64) {
    let u = up();
    if u.active && now_ms.saturating_sub(u.t_last) > TIMEOUT_MS {
        fail("Upload timed out");
    }
}

/// `DBG:UPLoad?`: (active, received, size).
pub fn status() -> (bool, u32, u32) {
    let u = up();
    (u.active, u.got, u.size)
}

type R<T> = Result<T, &'static str>;
const E_IO: &str = "Mass storage error";

/// DBG:UPLoad:BEGin on the device's SPI flash.
#[cfg(target_os = "none")]
#[inline(never)] // .cold (link-native.x): not inlined into the hot caller
pub fn begin(size: u32, crc: u32, now_ms: u64) -> R<()> {
    begin_on(disk(), size, crc, now_ms)
}

pub fn begin_on(dev: &mut dyn BlockDevice, size: u32, crc: u32, now_ms: u64) -> R<()> {
    abort();
    if crate::screenshot::busy() || crate::csvlog::writing() || staging() || crate::fastlog::holds_disk() {
        return Err("Mass storage busy");
    }
    if !(128..=MAX).contains(&size) {
        return Err("Illegal parameter value");
    }
    if !dev.present() || dev.block_size() as usize != BLK {
        return Err(E_IO);
    }
    let [buf, tmp, _] = diskbuf::blocks();
    if fatren::delete_updates(dev, buf, tmp).is_none() {
        return Err(E_IO);
    }
    #[cfg(feature = "vdisk")]
    crate::dlog::set_stale(); // SPIFLASH.BIN changes (no generated file does)
    *up() = Up { active: true, size, crc, got: 0, run: !0, cl: [0; MAXCL], ncl: 0, hint: 2, blocks: 0, fill: 0, t_last: now_ms, err: None };
    Ok(())
}

/// The payload of a `DBG:UPLOAD:DATA ` message: `<offset>,<bytes>`.
#[cfg(target_os = "none")]
#[inline(never)] // .cold (link-native.x): not inlined into the hot caller
pub fn data_msg(m: &[u8], now_ms: u64) -> R<()> {
    data_msg_on(disk(), m, now_ms)
}

pub fn data_msg_on(dev: &mut dyn BlockDevice, m: &[u8], now_ms: u64) -> R<()> {
    if !up().active && up().err.is_some() {
        return Ok(()); // failed earlier: the rest of the stream is dropped
    }
    data_inner(dev, m, now_ms).map_err(fail)
}

fn data_inner(dev: &mut dyn BlockDevice, m: &[u8], now_ms: u64) -> R<()> {
    let u = up();
    if !u.active {
        return Err("Upload not started");
    }
    let comma = m.iter().position(|&c| c == b',').ok_or("Illegal parameter value")?;
    let off: u32 = core::str::from_utf8(&m[..comma]).ok().and_then(|s| s.trim_ascii().parse().ok()).ok_or("Illegal parameter value")?;
    let d = &m[comma + 1..];
    if off != u.got {
        return Err("Data out of order");
    }
    if u.got + d.len() as u32 > u.size {
        return Err("Data past the size");
    }
    u.t_last = now_ms;
    u.run = calbackup::crc32_upd(u.run, d);
    u.got += d.len() as u32;
    let mut d = d;
    while !d.is_empty() {
        let buf = &mut diskbuf::blocks()[0];
        let n = (BLK - u.fill).min(d.len());
        buf[u.fill..u.fill + n].copy_from_slice(&d[..n]);
        u.fill += n;
        d = &d[n..];
        if u.fill == BLK {
            flush(dev, u)?;
        }
    }
    Ok(())
}

/// Write blocks()[0] (0xFF after `fill`) as the next block: a new cluster
/// when the block starts one.
fn flush(dev: &mut dyn BlockDevice, u: &mut Up) -> R<()> {
    let [buf, fat, tmp] = diskbuf::blocks();
    buf[u.fill..].fill(0xFF);
    if !dev.read_block(0, tmp) {
        return Err(E_IO);
    }
    let v = parse_bpb(&tmp[..], dev.block_count()).ok_or(E_IO)?;
    if u.blocks % v.spc == 0 {
        if u.ncl == MAXCL {
            return Err("Data past the size");
        }
        u.cl[u.ncl] = next_free(dev, &v, &mut u.hint, fat)?;
        u.ncl += 1;
    }
    let lba = v.clba(u.cl[u.ncl - 1] as u32) + u.blocks % v.spc;
    if !(dev.write_block(lba, buf) && verify(dev, lba, buf, tmp)) {
        return Err(E_IO);
    }
    u.blocks += 1;
    u.fill = 0;
    Ok(())
}

/// The next free cluster from `hint` on (FAT 1 unchanged while an upload /
/// a staging owns the disk, so the clusters taken since are never handed
/// out again: `hint` only grows). `fat` = a block buffer.
pub(crate) fn next_free(dev: &mut dyn BlockDevice, v: &crate::fatren::Vol, hint: &mut u32, fat: &mut [u8; BLK]) -> R<u16> {
    let mut cur = u32::MAX;
    loop {
        if *hint < 2 {
            *hint = 2;
        }
        if *hint > v.clusters + 1 {
            return Err("Mass storage full");
        }
        let fb = *hint / FPER;
        if fb != cur {
            if !dev.read_block(v.fat_start + fb, fat) {
                return Err(E_IO);
            }
            cur = fb;
        }
        let free = le16(&fat[..], ((*hint % FPER) * 2) as usize) == 0;
        *hint += 1;
        if free {
            return Ok((*hint - 1) as u16);
        }
    }
}

/// DBG:UPLoad:END? on the device's SPI flash: (size, crc).
#[cfg(target_os = "none")]
#[inline(never)] // .cold (link-native.x): not inlined into the hot caller
pub fn end() -> R<(u32, u32)> {
    end_on(disk())
}

pub fn end_on(dev: &mut dyn BlockDevice) -> R<(u32, u32)> {
    if let (false, Some(e)) = (up().active, up().err) {
        up().err = None;
        return Err(e);
    }
    let r = end_inner(dev).map_err(fail);
    abort();
    r
}

fn end_inner(dev: &mut dyn BlockDevice) -> R<(u32, u32)> {
    let u = up();
    if !u.active {
        return Err("Upload not started");
    }
    if u.got != u.size {
        return Err("Data missing");
    }
    if !u.run != u.crc {
        return Err("CRC mismatch");
    }
    if u.fill > 0 {
        flush(dev, u)?;
    }
    let [buf, _, tmp] = diskbuf::blocks();
    if !dev.read_block(0, tmp) {
        return Err(E_IO);
    }
    let v = parse_bpb(&tmp[..], dev.block_count()).ok_or(E_IO)?;
    // Re-read everything: CRC of what is on the flash; the header.
    let mut c = !0u32;
    for b in 0..u.blocks {
        let lba = v.clba(u.cl[(b / v.spc) as usize] as u32) + b % v.spc;
        if !dev.read_block(lba, buf) {
            return Err(E_IO);
        }
        if b == 0 && !fatren::header_valid(&fatren::header_of(&buf[..])) {
            return Err("Not an update file");
        }
        let n = (u.size - b * BLK as u32).min(BLK as u32) as usize;
        c = calbackup::crc32_upd(c, &buf[..n]);
    }
    if !c != u.crc {
        return Err("Verify failed");
    }
    link_and_enter(dev, &v, &u.cl[..u.ncl], u.size, &NAME, 0x20)?;
    #[cfg(feature = "vdisk")]
    crate::dlog::set_stale();
    Ok((u.size, u.crc))
}

/// A firmware file copied onto the USB volume (vwrite.rs): its data is
/// already in the free clusters `cl` (file order, verified by the caller);
/// every other dm40*.bin in the root is deleted (fatren::delete_updates),
/// then the chain and the entry DM40FW.BIN as at END.
pub fn commit_on(dev: &mut dyn BlockDevice, cl: &[u16], size: u32) -> R<()> {
    if cl.is_empty() || cl.len() > MAXCL || !(128..=MAX).contains(&size) || (size as usize).div_ceil(BLK) != cl.len() {
        return Err("Illegal parameter value");
    }
    let [buf, tmp, _] = diskbuf::blocks();
    if fatren::delete_updates(dev, buf, tmp).is_none() {
        return Err(E_IO);
    }
    if !dev.read_block(0, tmp) {
        return Err(E_IO);
    }
    let v = parse_bpb(&tmp[..], dev.block_count()).ok_or(E_IO)?;
    if v.spc != 1 {
        return Err(E_IO); // one 4 KiB block per cluster (the stock disk)
    }
    let r = link_and_enter(dev, &v, cl, size, &NAME, 0x20);
    #[cfg(feature = "vdisk")]
    crate::dlog::set_stale();
    r
}

/// A file of the free clusters `cl` (already written, any order) in the
/// root: the chain, then the entry `name` (`attr`). vwrite.rs parks a host
/// session's data this way (hidden system temp files).
pub(crate) fn link_named(dev: &mut dyn BlockDevice, cl: &[u16], size: u32, name: &[u8; 11], attr: u8) -> R<()> {
    let tmp = &mut diskbuf::blocks()[2];
    if !dev.read_block(0, tmp) {
        return Err(E_IO);
    }
    let v = parse_bpb(&tmp[..], dev.block_count()).ok_or(E_IO)?;
    if v.spc != 1 || cl.is_empty() {
        return Err(E_IO);
    }
    link_and_enter(dev, &v, cl, size, name, attr)
}

/// The chain `cl` in every FAT copy (FAT 1 first; per copy one write per
/// touched block, fewer when the clusters ascend), then the root entry
/// `name` (`attr`) of `size` bytes.
fn link_and_enter(dev: &mut dyn BlockDevice, v: &crate::fatren::Vol, cl: &[u16], size: u32, name: &[u8; 11], attr: u8) -> R<()> {
    let [buf, fat, tmp] = diskbuf::blocks();
    for k in 0..v.nfats {
        let mut i = 0;
        while i < cl.len() {
            let fb = cl[i] as u32 / FPER;
            let lba = v.fat_start + k * v.fat_blocks + fb;
            if !dev.read_block(lba, fat) {
                return Err(E_IO);
            }
            while i < cl.len() && cl[i] as u32 / FPER == fb {
                let o = ((cl[i] as u32 % FPER) * 2) as usize;
                if k == 0 && le16(&fat[..], o) != 0 {
                    return Err(E_IO); // taken since (cannot happen: we own the disk)
                }
                let next = cl.get(i + 1).copied().unwrap_or(0xFFFF);
                fat[o..o + 2].copy_from_slice(&next.to_le_bytes());
                i += 1;
            }
            if !(dev.write_block(lba, fat) && verify(dev, lba, fat, tmp)) {
                return Err(E_IO);
            }
        }
    }
    // The root entry.
    let slot = fatren::free_root_slot(dev, v, buf).ok_or(E_IO)?.ok_or("Mass storage full")?;
    let lba = v.root_start + slot / (BLK as u32 / 32);
    if !dev.read_block(lba, buf) {
        return Err(E_IO);
    }
    let o = ((slot % (BLK as u32 / 32)) * 32) as usize;
    let e = &mut buf[o..o + 32];
    e.fill(0);
    e[..11].copy_from_slice(name);
    e[11] = attr;
    for d in [0x10, 0x12, 0x18] {
        e[d..d + 2].copy_from_slice(&DATE.to_le_bytes());
    }
    e[0x1A..0x1C].copy_from_slice(&cl[0].to_le_bytes());
    e[0x1C..0x20].copy_from_slice(&size.to_le_bytes());
    if !(dev.write_block(lba, buf) && verify(dev, lba, buf, tmp)) {
        return Err(E_IO);
    }
    Ok(())
}
