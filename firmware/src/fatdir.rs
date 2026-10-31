//! Subdirectories on the SPI-flash FAT16 volume (the internal storage; it is
//! no longer exported over USB in the native build, see vdisk.rs).
//!
//! Layout: the root keeps what the vendor FS-loader and the recovery need
//! (dm40*.bin update files, _DM40PRV.BIN, calbackup.rs's write-once
//! CALIB / BOOTDATA / UID .BIN safety copy); our own files go to
//!   LOGS/   LOGnnn.DLG   DMM recordings (csvlog.rs)
//!           FASTnnn.DLG  fast logger streams (fastlog.rs)
//!   SHOTS/  Snnn.BMP     screenshots (screenshot.rs)
//!   SCOPE/  SCOPnnn.DLG  scope records (screenshot.rs `start_csv`)
//! (`dir_of`: the directory follows from the name). Files of ours that
//! earlier builds wrote to the root STAY there (no boot-time migration: a
//! move is two directory writes with a window in which either two live
//! entries share one chain or the file is lost; leaving them costs every
//! reader one more directory to list, see `seq`). The writers pick the
//! lowest number free in the root AND the directory, so names never clash;
//! the readers (gallery.rs, csvlog.rs list / delete, dlog.rs, vdisk.rs)
//! list both.
//!
//! A directory of ours is created with room for NEED entries (1000 numbered
//! files + "." + ".."; 8 clusters = 32 KiB on this disk), all clusters
//! zeroed, so a writer never has to grow it: deleted slots (0xE5) are
//! reused, and at most 1000 numbers exist. `ensure` grows a shorter one (a
//! directory made by a PC on the vendor loader's USB disk, mtools' one
//! cluster) to that size once.
//!
//! Write order, as calbackup.rs / screenshot.rs (a power cut anywhere leaves
//! at worst lost clusters, never a live entry on free or unwritten clusters):
//!   create  clusters zeroed ("." and ".." in the first block) -> chain in
//!           every FAT copy (FAT 1 first) -> the root entry (attribute 0x10,
//!           size 0);
//!   grow    new clusters zeroed -> their chain (EOC) in every FAT copy ->
//!           the old last cluster linked to the first new one, every copy.
//! Every block write is read back. All synchronous (a create is ~11 block
//! writes, < ~1 s; once per directory: native boot creates all three,
//! `ensure` from a writer only when one went missing).
//!
//! The directory view is a `Vol` (fatren.rs) with the chain in `dcl[..dn]`:
//! the writers address its blocks with `dlba` exactly as they did the root.
//!
//! Host test: tools/fatren-test/run.sh (mkdir / grow / delete, power cut
//! after every block write, fsck.fat after every step).

#![cfg_attr(not(target_os = "none"), allow(dead_code))]

use crate::fatren::{le16, sfn_is, verify, Vol, DIR_MAX_CL};
use crate::msc::BlockDevice;

const BLK: usize = 4096;
const PER: u32 = (BLK / 32) as u32;
const FPER: u32 = (BLK / 2) as u32;
/// Directory dates 2026-10-13 (no RTC), as the files.
pub const DATE: u16 = (46 << 9) | (10 << 5) | 13;

pub const LOGS: [u8; 11] = *b"LOGS       ";
pub const SHOTS: [u8; 11] = *b"SHOTS      ";
pub const SCOPE: [u8; 11] = *b"SCOPE      ";
pub const DIRS: [[u8; 11]; 3] = [LOGS, SHOTS, SCOPE];
/// Entry slots of a directory of ours: 1000 numbered files + "." + "..".
pub const NEED: u32 = 1002;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Err {
    /// A read, write or verify failed.
    Io,
    /// No free root slot / not enough free clusters.
    Full,
    /// The name belongs to a file, or the directory chain is broken or
    /// longer than DIR_MAX_CL clusters: left alone.
    Bad,
}

/// The directory of one of our files, from its 8.3 name: S*.BMP -> SHOTS,
/// LOG*.DLG / FAST*.DLG -> LOGS, SCOP*.DLG -> SCOPE (case-insensitive).
/// None: not ours.
#[cfg_attr(target_os = "none", link_section = ".text.fat_rd")]
pub fn dir_of(n: &[u8]) -> Option<&'static [u8; 11]> {
    let up = |c: u8| c.to_ascii_uppercase();
    let ext = [up(n[8]), up(n[9]), up(n[10])];
    if &ext == b"BMP" && up(n[0]) == b'S' {
        Some(&SHOTS)
    } else if &ext == b"DLG" && (n[..3].iter().map(|&c| up(c)).eq(*b"LOG") || n[..4].iter().map(|&c| up(c)).eq(*b"FAST")) {
        Some(&LOGS)
    } else if &ext == b"DLG" && n[..4].iter().map(|&c| up(c)).eq(*b"SCOP") {
        Some(&SCOPE)
    } else {
        None
    }
}

/// Clusters a directory of NEED entries takes.
pub fn need_clusters(v: &Vol) -> u32 {
    (NEED * 32).div_ceil(v.spc * BLK as u32)
}

/// Directory blocks of the sequence "the root, then the subdirectory `d`"
/// (`d.dn == 0`: the root alone): the scan the writers and readers use,
/// legacy files of ours being in the root.
pub fn seq_blocks(d: &Vol) -> u32 {
    d.root_blocks + if d.dn > 0 { d.dblocks() } else { 0 }
}

/// Block `i` of that sequence: (lba, in the subdirectory, slot index of its
/// first entry within its own directory, slots of that directory).
pub fn seq(d: &Vol, i: u32) -> (u32, bool, u32, u32) {
    if i < d.root_blocks {
        (d.root_start + i, false, i * PER, d.root_entries)
    } else {
        let b = i - d.root_blocks;
        (d.dlba(b), true, b * PER, d.dentries())
    }
}

/// The root entry named `name` (live, not a long-name entry) and the first
/// free root slot: (Some((slot, attribute, start cluster)), free slot).
#[cfg_attr(target_os = "none", link_section = ".text.fat_rd")]
fn find_root(dev: &mut dyn BlockDevice, v: &Vol, name: &[u8; 11], buf: &mut [u8; BLK]) -> Result<(Option<(u32, u8, u32)>, Option<u32>), Err> {
    let mut free = None;
    for b in 0..v.root_blocks {
        if !dev.read_block(v.root_start + b, buf) {
            return Err(Err::Io);
        }
        for k in 0..PER {
            let idx = b * PER + k;
            if idx >= v.root_entries {
                return Ok((None, free));
            }
            let e = &buf[(k * 32) as usize..(k * 32 + 32) as usize];
            if e[0] == 0x00 {
                return Ok((None, free.or(Some(idx))));
            }
            if e[0] == 0xE5 {
                free.get_or_insert(idx);
                continue;
            }
            if e[11] & 0x3F == 0x0F || e[11] & 0x08 != 0 {
                continue; // long name, volume label
            }
            if sfn_is(e, name) {
                return Ok((Some((idx, e[11], le16(e, 0x1A))), free));
            }
        }
    }
    Ok((None, free))
}

/// FAT 1 entry `c` through a one-block cache (`cur` = cached block).
#[cfg_attr(target_os = "none", link_section = ".text.fat_rd")]
fn fat_get(dev: &mut dyn BlockDevice, v: &Vol, c: u32, fat: &mut [u8; BLK], cur: &mut u32) -> Option<u32> {
    let fb = c / FPER;
    if fb >= v.fat_blocks {
        return None;
    }
    if *cur != fb {
        *cur = u32::MAX;
        if !dev.read_block(v.fat_start + fb, fat) {
            return None;
        }
        *cur = fb;
    }
    Some(le16(fat, ((c % FPER) * 2) as usize))
}

/// The directory view of the chain starting at `start`.
#[cfg_attr(target_os = "none", link_section = ".text.fat_rd")]
fn chain(dev: &mut dyn BlockDevice, v: &Vol, start: u32, buf: &mut [u8; BLK]) -> Result<Vol, Err> {
    let max = v.clusters + 1;
    let mut d = v.root();
    let mut c = start;
    let mut cur = u32::MAX;
    loop {
        if c < 2 || c > max || d.dn as usize == DIR_MAX_CL || d.dcl[..d.dn as usize].contains(&(c as u16)) {
            return Err(Err::Bad);
        }
        d.dcl[d.dn as usize] = c as u16;
        d.dn += 1;
        let n = fat_get(dev, v, c, buf, &mut cur).ok_or(Err::Io)?;
        if n >= 0xFFF8 {
            return Ok(d);
        }
        c = n;
    }
}

/// Subdirectory `name` of the root: Ok(None) = there is none. Region B
/// (`.text.fat_rd`, see `for_each`).
#[cfg_attr(target_os = "none", link_section = ".text.fat_rd")]
pub fn open(dev: &mut dyn BlockDevice, v: &Vol, name: &[u8; 11], buf: &mut [u8; BLK]) -> Result<Option<Vol>, Err> {
    match find_root(dev, v, name, buf)?.0 {
        None => Ok(None),
        Some((_, attr, start)) if attr & 0x10 != 0 => chain(dev, v, start, buf).map(Some),
        Some(_) => Err(Err::Bad),
    }
}

/// `k` free clusters, first fit, into `out[..k]`.
fn alloc(dev: &mut dyn BlockDevice, v: &Vol, out: &mut [u16], buf: &mut [u8; BLK]) -> Result<(), Err> {
    let mut cur = u32::MAX;
    let mut got = 0;
    let mut c = 2;
    while got < out.len() {
        if c > v.clusters + 1 {
            return Err(Err::Full);
        }
        if fat_get(dev, v, c, buf, &mut cur).ok_or(Err::Io)? == 0 {
            out[got] = c as u16;
            got += 1;
        }
        c += 1;
    }
    Ok(())
}

/// A directory entry (8.3 `name`, directory attribute, start cluster, size 0).
fn dir_entry(e: &mut [u8], name: &[u8; 11], start: u32) {
    e.fill(0);
    e[..11].copy_from_slice(name);
    e[11] = 0x10;
    for o in [0x10, 0x12, 0x18] {
        e[o..o + 2].copy_from_slice(&DATE.to_le_bytes());
    }
    e[0x1A..0x1C].copy_from_slice(&(start as u16).to_le_bytes());
}

/// Zero every block of clusters `cl`; with `dots` = (own cluster, parent),
/// the first block starts with "." and "..".
fn zero(dev: &mut dyn BlockDevice, v: &Vol, cl: &[u16], dots: Option<(u32, u32)>, buf: &mut [u8; BLK], tmp: &mut [u8; BLK]) -> Result<(), Err> {
    for (i, &c) in cl.iter().enumerate() {
        for s in 0..v.spc {
            buf.fill(0);
            if let (0, 0, Some((me, parent))) = (i, s, dots) {
                dir_entry(&mut buf[..32], b".          ", me);
                dir_entry(&mut buf[32..64], b"..         ", parent);
            }
            let lba = v.clba(c as u32) + s;
            if !(dev.write_block(lba, buf) && verify(dev, lba, buf, tmp)) {
                return Err(Err::Io);
            }
        }
    }
    Ok(())
}

/// Set FAT entries `ents` (cluster, value) in every FAT copy, FAT 1 first;
/// per copy one read-modify-write per touched block (blocks in the order of
/// their first entry). `want`: the value each entry must hold in FAT 1
/// before (None: not checked).
fn fat_set(dev: &mut dyn BlockDevice, v: &Vol, ents: &[(u32, u16)], want: Option<&dyn Fn(u32) -> bool>, buf: &mut [u8; BLK], tmp: &mut [u8; BLK]) -> Result<(), Err> {
    for k in 0..v.nfats {
        for (i, &(c0, _)) in ents.iter().enumerate() {
            let fb = c0 / FPER;
            if ents[..i].iter().any(|&(c, _)| c / FPER == fb) {
                continue; // block done with an earlier entry
            }
            let lba = v.fat_start + k * v.fat_blocks + fb;
            if !dev.read_block(lba, buf) {
                return Err(Err::Io);
            }
            for &(c, val) in ents.iter().filter(|&&(c, _)| c / FPER == fb) {
                let o = ((c % FPER) * 2) as usize;
                if k == 0 && want.is_some_and(|w| !w(le16(&buf[..], o))) {
                    return Err(Err::Io); // changed since the scan
                }
                buf[o..o + 2].copy_from_slice(&val.to_le_bytes());
            }
            if !(dev.write_block(lba, buf) && verify(dev, lba, buf, tmp)) {
                return Err(Err::Io);
            }
        }
    }
    Ok(())
}

/// A new chain over `cl` (each to the next, the last EOC) in every FAT copy;
/// the entries must be free in FAT 1.
fn link_new(dev: &mut dyn BlockDevice, v: &Vol, cl: &[u16], buf: &mut [u8; BLK], tmp: &mut [u8; BLK]) -> Result<(), Err> {
    let mut ents = [(0u32, 0u16); DIR_MAX_CL];
    for (i, &c) in cl.iter().enumerate() {
        ents[i] = (c as u32, cl.get(i + 1).copied().unwrap_or(0xFFFF));
    }
    fat_set(dev, v, &ents[..cl.len()], Some(&|x| x == 0), buf, tmp)
}

/// Create subdirectory `name` in root slot `slot` (free): NEED entries.
fn create(dev: &mut dyn BlockDevice, v: &Vol, name: &[u8; 11], slot: u32, s: &mut [[u8; BLK]; 3]) -> Result<Vol, Err> {
    let [buf, tmp, _] = s;
    let k = need_clusters(v) as usize;
    if k > DIR_MAX_CL {
        return Err(Err::Bad);
    }
    let mut cl = [0u16; DIR_MAX_CL];
    alloc(dev, v, &mut cl[..k], buf)?;
    zero(dev, v, &cl[..k], Some((cl[0] as u32, 0)), buf, tmp)?;
    link_new(dev, v, &cl[..k], buf, tmp)?;
    let lba = v.root_start + slot / PER;
    if !dev.read_block(lba, buf) {
        return Err(Err::Io);
    }
    let o = ((slot % PER) * 32) as usize;
    if buf[o] != 0x00 && buf[o] != 0xE5 {
        return Err(Err::Io); // taken since the scan
    }
    dir_entry(&mut buf[o..o + 32], name, cl[0] as u32);
    if !(dev.write_block(lba, buf) && verify(dev, lba, buf, tmp)) {
        return Err(Err::Io);
    }
    let mut d = v.root();
    d.dcl[..k].copy_from_slice(&cl[..k]);
    d.dn = k as u8;
    Ok(d)
}

/// Append `k` zeroed clusters to directory `d`; returns the longer view.
pub fn grow(dev: &mut dyn BlockDevice, d: &Vol, k: usize, s: &mut [[u8; BLK]; 3]) -> Result<Vol, Err> {
    let [buf, tmp, _] = s;
    let n = d.dn as usize;
    if n == 0 || n + k > DIR_MAX_CL {
        return Err(Err::Bad);
    }
    let mut cl = [0u16; DIR_MAX_CL];
    alloc(dev, d, &mut cl[..k], buf)?;
    zero(dev, d, &cl[..k], None, buf, tmp)?;
    link_new(dev, d, &cl[..k], buf, tmp)?;
    let last = d.dcl[n - 1] as u32;
    fat_set(dev, d, &[(last, cl[0])], Some(&|x| x >= 0xFFF8), buf, tmp)?;
    let mut g = *d;
    g.dcl[n..n + k].copy_from_slice(&cl[..k]);
    g.dn += k as u8;
    Ok(g)
}

/// Subdirectory `name` with at least NEED slots: opened, created, or grown.
/// `s` = three block buffers (diskbuf::blocks()).
pub fn ensure(dev: &mut dyn BlockDevice, v: &Vol, name: &[u8; 11], s: &mut [[u8; BLK]; 3]) -> Result<Vol, Err> {
    let (hit, free) = find_root(dev, v, name, &mut s[0])?;
    let d = match hit {
        Some((_, attr, start)) if attr & 0x10 != 0 => chain(dev, v, start, &mut s[0])?,
        Some(_) => return Err(Err::Bad),
        None => return create(dev, v, name, free.ok_or(Err::Full)?, s),
    };
    let have = d.dn as u32;
    let need = need_clusters(v);
    if have >= need {
        return Ok(d);
    }
    grow(dev, &d, (need - have) as usize, s)
}

#[allow(dead_code)] // host tests (tools/fatren-test)
/// Remove subdirectory `name` if it holds nothing but "." and "..": its root
/// entry 0xE5 first, then its chain freed in every FAT copy (a cut between =
/// lost clusters). Ok(false): not empty / no such directory (untouched).
pub fn remove_empty(dev: &mut dyn BlockDevice, v: &Vol, name: &[u8; 11], s: &mut [[u8; BLK]; 3]) -> Result<bool, Err> {
    let [buf, tmp, fat] = s;
    let (Some((slot, attr, start)), _) = find_root(dev, v, name, buf)? else {
        return Ok(false);
    };
    if attr & 0x10 == 0 {
        return Err(Err::Bad);
    }
    let d = chain(dev, v, start, buf)?;
    for b in 0..d.dblocks() {
        if !dev.read_block(d.dlba(b), buf) {
            return Err(Err::Io);
        }
        for k in 0..PER {
            let e = &buf[(k * 32) as usize..(k * 32 + 32) as usize];
            if e[0] == 0x00 {
                break;
            }
            if e[0] != 0xE5 && e[0] != b'.' {
                return Ok(false);
            }
        }
    }
    let lba = v.root_start + slot / PER;
    if !dev.read_block(lba, buf) {
        return Err(Err::Io);
    }
    buf[((slot % PER) * 32) as usize] = 0xE5;
    if !(dev.write_block(lba, buf) && verify(dev, lba, buf, tmp)) {
        return Err(Err::Io);
    }
    match crate::fatren::free_chain(dev, v, start, fat, buf, tmp) {
        (true, _) => Ok(true),
        _ => Err(Err::Io),
    }
}

/// Result of the boot-time `boot` (DBG:BOOT? FATDIR): bit i = DIRS[i]
/// present afterwards / created this boot / failed.
#[derive(Clone, Copy, Debug)]
pub struct Boot {
    pub ok: u8,
    pub created: u8,
    pub failed: u8,
}

static mut BOOT: Boot = Boot { ok: 0, created: 0, failed: 0 };

pub fn boot_result() -> Boot {
    unsafe { BOOT }
}

/// Native boot (before USB, after calbackup): make sure LOGS, SHOTS and
/// SCOPE exist with NEED slots. Nothing is written when they do. Never
/// inlined (the boot frames are popped before main()).
#[inline(never)]
pub fn boot(dev: &mut dyn BlockDevice) -> Boot {
    let s = crate::diskbuf::blocks();
    let mut r = Boot { ok: 0, created: 0, failed: 0 };
    if dev.present() && dev.block_size() as usize == BLK && dev.read_block(0, &mut s[0]) {
        if let Some(v) = crate::fatren::parse_bpb(&s[0][..], dev.block_count()) {
            for (i, name) in DIRS.iter().enumerate() {
                let existed = matches!(open(dev, &v, name, &mut s[0]), Ok(Some(_)));
                match ensure(dev, &v, name, s) {
                    Ok(_) => {
                        r.ok |= 1 << i;
                        if !existed {
                            r.created |= 1 << i;
                        }
                    }
                    Err(_) => r.failed |= 1 << i,
                }
                crate::wdt::kick_if_feeding();
            }
        }
    }
    unsafe { BOOT = r };
    r
}

/// Every live 8.3 file entry (not deleted, long-name, label or directory) of
/// the root and then of the subdirectories `dirs` that exist, in directory
/// order: `f(view, slot, entry)` (view = the directory holding it, slot its
/// index there); `f` returns false to stop. Reads with `buf`.
///
/// This, `open`, `find_root`, `chain`, `fat_get` and `dir_of` are region B (`.text.fat_rd`, not
/// this module's .cold): MMEM:CAT? (csvlog catalog_scpi) runs them, and a
/// host may list the files while the Wave view's stream ISR runs
/// (emu/hy3131/wavetest.py, region C hooks).
#[cfg_attr(target_os = "none", link_section = ".text.fat_rd")]
pub fn for_each(dev: &mut dyn BlockDevice, v: &Vol, dirs: &[&[u8; 11]], buf: &mut [u8; BLK], f: &mut dyn FnMut(&Vol, u32, &[u8]) -> bool) -> Result<(), Err> {
    let mut views = [v.root(); 4];
    let mut n = 1;
    for d in dirs.iter().take(3) {
        match open(dev, v, d, buf) {
            Ok(Some(d)) => {
                views[n] = d;
                n += 1;
            }
            Ok(None) | Err(Err::Bad) => {}
            Err(e) => return Err(e),
        }
    }
    for d in &views[..n] {
        'blocks: for b in 0..d.dblocks() {
            if !dev.read_block(d.dlba(b), buf) {
                return Err(Err::Io);
            }
            for k in 0..PER {
                let idx = b * PER + k;
                if idx >= d.dentries() {
                    break 'blocks;
                }
                let e = &buf[(k * 32) as usize..(k * 32 + 32) as usize];
                if e[0] == 0x00 {
                    break 'blocks;
                }
                if e[0] == 0xE5 || e[11] & 0x3F == 0x0F || e[11] & 0x18 != 0 {
                    continue;
                }
                if !f(d, idx, e) {
                    return Ok(());
                }
            }
        }
    }
    Ok(())
}
