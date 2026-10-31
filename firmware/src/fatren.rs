//! Rename the INSTALLED `dm40*.bin` on the SPI-flash FAT16 volume to
//! `_DM40PRV.BIN` so the FS-loader stops re-installing it.
//!
//! Why: in its install state the FS-loader (0x080a0000) installs the first
//! dm40*.bin in the root, even an identical image, and deletes it only after
//! a successful install (`f_unlink`, stock/re/docs/fsloader-install-trigger.md). A copy
//! left behind would be re-flashed (~480 KB) on its next install-state boot.
//! Renaming (not deleting) keeps the image: remove the leading underscore.
//!
//! Which file (review 2026-10-13 H1): only a dm40*.bin whose decoded 12-byte
//! update header equals the 12-byte record at 0x08008000 that the loader
//! wrote when it installed the running image. A cold power-on does NOT mean the
//! loader ran (the ROM launches a valid app directly when PA2 reads high), so a
//! freshly staged, not yet installed file must stay where it is; it is only
//! counted (`staged`). The caller passes the marker (`None` = invalid marker:
//! nothing is renamed). No reset-cause gate: a file equal to the marker is
//! installed whatever the reset was (this also covers DBG:UPDATE, after which
//! the loader resets with SYSRESETREQ, not a power-on).
//!
//! When: native normal boot, before USB exists (the host cannot have the
//! volume mounted).
//!
//! Observed on the device (T3): the FS-loader itself deletes the installed
//! file, so normally there is nothing to rename. calbackup (runs after this)
//! regenerates `_DM40PRV.BIN` from flash. The rename stays a safety net: it
//! only replaces `_DM40PRV.BIN` with the installed image (the same firmware):
//! the two never fight (calbackup only re-stamps that entry once).
//!
//! What, in this order (an interruption between steps leaves at worst lost
//! clusters, never two live entries with the same name or a live entry on
//! freed clusters):
//!   1. If `_DM40PRV.BIN` already exists (first one in the root): mark its 8.3
//!      entry and its LFN entries 0xE5, then free its cluster chain (0x0000)
//!      in every FAT copy. Only one backup is kept. Done only when step 2 has
//!      an installed file to rename.
//!   2. The FIRST root entry (directory order) whose 8.3 name is `DM40*` +
//!      `BIN` or whose long name matches `dm40*.bin` (both case-insensitive)
//!      AND whose header equals the marker: 8.3 name -> `_DM40PRV` `BIN` (case
//!      flags cleared), its LFN entries marked 0xE5 (no long name is left that
//!      could still match). No new LFN entries are written.
//!   3. Every other match is left alone and only counted: `staged` = header
//!      differs from the marker (not installed: the loader installs it on its
//!      next run), `others` = all matches left untouched.
//! Not a FAT writer itself (src/calbackup.rs reuses the helpers below to write
//! its backup files). Each 4 KiB block goes
//! through the BlockDevice (spiflash.rs: skip if unchanged, program without
//! erase where possible, erase + program otherwise, read back), and is read
//! back again here and compared. The volume is not touched unless the boot
//! sector is a sane FAT16 with sector size == device block size (4096).
//!
//! Host test: tools/fatren-test/run.sh (mkfs.fat image, mtools, fsck.fat).

// Host tests include only parts of it.
#![cfg_attr(not(target_os = "none"), allow(dead_code))]

use crate::msc::BlockDevice;

const BLK: usize = 4096;
/// LFN entries for one name (255 chars / 13 = 20).
const MAX_LFN: usize = 20;
/// dm40*.bin matches whose headers are checked (further ones only counted).
const MAX_HITS: usize = 8;
/// The backup's 8.3 name (name + extension, space padded).
pub const BACKUP: [u8; 11] = *b"_DM40PRVBIN";
/// The recovery trigger file an earlier build wrote (0 bytes): a dm40*.bin
/// that fails the loader's header check, so a boot through the loader enters
/// its update mode instead of launching the app ("dm40rcvr.bin update
/// error"). A normal boot deletes a leftover one (`run`, step 0).
pub const FAKE: [u8; 11] = *b"DM40RCVRBIN";

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Status {
    /// Not run (warm boot, or a build without the call).
    NotRun,
    /// No SPI flash / medium.
    NoMedium,
    /// Boot sector does not look like the expected FAT16 volume; untouched.
    BadBpb,
    /// No installed dm40*.bin in the root (staged ones may be there).
    Clean,
    /// Dry run: `found` matches, nothing written (`renamed` = would rename).
    DryRun,
    /// The first match was renamed (and an older backup deleted if present).
    Renamed,
    /// A read, write or verify failed; see the other fields for progress.
    Error,
}

#[derive(Clone, Copy, Debug)]
pub struct Result {
    pub status: Status,
    /// Root entries matching dm40*.bin.
    pub found: u8,
    /// 8.3 name of the (first) match that was / would be renamed.
    pub renamed: [u8; 11],
    /// An older _DM40PRV.BIN existed (and was / would be deleted).
    pub old_backup: bool,
    /// Old backup actually deleted, and clusters freed for it.
    pub old_deleted: bool,
    pub freed_clusters: u32,
    /// Matches left untouched (found - renamed).
    pub others: u8,
    /// Matches whose header differs from the marker: staged, not installed.
    pub staged: u8,
    /// A leftover DM40RCVR.BIN recovery trigger was present and deleted first.
    pub fake_deleted: bool,
}

impl Result {
    const fn new(status: Status) -> Self {
        Result {
            status,
            found: 0,
            renamed: [b' '; 11],
            old_backup: false,
            old_deleted: false,
            freed_clusters: 0,
            others: 0,
            staged: 0,
            fake_deleted: false,
        }
    }
}

static mut RESULT: Result = Result::new(Status::NotRun);

pub fn result() -> Result {
    unsafe { RESULT }
}

#[derive(Clone, Copy)]
pub(crate) struct Vol {
    pub spc: u32,
    pub data_start: u32,
    pub fat_start: u32,
    pub fat_blocks: u32,
    pub nfats: u32,
    pub root_start: u32,
    pub root_blocks: u32,
    pub root_entries: u32,
    /// Data clusters (valid cluster numbers 2..=clusters + 1).
    pub clusters: u32,
    /// Directory view (src/fatdir.rs): 0 = the root directory; else a
    /// subdirectory whose cluster chain is `dcl[..dn]` (`dblocks` /
    /// `dentries` / `dlba` address either one). `parse_bpb` gives the root.
    pub dn: u8,
    pub dcl: [u16; DIR_MAX_CL],
}

/// Clusters of a subdirectory at most (src/fatdir.rs; 16 x 4 KiB = 2048
/// entries on this disk).
pub(crate) const DIR_MAX_CL: usize = 16;

/// Region B (`.text.fat_rd`): fatdir::for_each (MMEM:CAT?) uses these.
impl Vol {
    /// Blocks of the directory this view addresses (the root or `dcl`).
    #[cfg_attr(target_os = "none", link_section = ".text.fat_rd")]
    pub fn dblocks(&self) -> u32 {
        if self.dn == 0 { self.root_blocks } else { self.dn as u32 * self.spc }
    }
    /// Entry slots of that directory.
    #[cfg_attr(target_os = "none", link_section = ".text.fat_rd")]
    pub fn dentries(&self) -> u32 {
        if self.dn == 0 { self.root_entries } else { self.dblocks() * (BLK / 32) as u32 }
    }
    /// LBA of directory block `b` (< `dblocks()`).
    #[cfg_attr(target_os = "none", link_section = ".text.fat_rd")]
    pub fn dlba(&self, b: u32) -> u32 {
        if self.dn == 0 {
            self.root_start + b
        } else {
            self.clba(self.dcl[(b / self.spc) as usize] as u32) + b % self.spc
        }
    }
    /// First LBA of cluster `c`.
    #[cfg_attr(target_os = "none", link_section = ".text.fat_rd")]
    pub fn clba(&self, c: u32) -> u32 {
        self.data_start + (c - 2) * self.spc
    }
    /// The root view of the same volume.
    #[cfg_attr(target_os = "none", link_section = ".text.fat_rd")]
    pub fn root(&self) -> Vol {
        Vol { dn: 0, ..*self }
    }
}

#[cfg_attr(target_os = "none", link_section = ".text.fat_rd")]
pub(crate) fn le16(b: &[u8], o: usize) -> u32 {
    b[o] as u32 | (b[o + 1] as u32) << 8
}

#[cfg_attr(target_os = "none", link_section = ".text.fat_rd")]
pub(crate) fn le32(b: &[u8], o: usize) -> u32 {
    le16(b, o) | le16(b, o + 2) << 16
}

/// Parse and sanity-check the boot sector; None = do not touch the volume.
///
/// This, `le16` / `le32`, `sfn_is` and `Vol::dlba` are region B
/// (`.text.fat_rd`, not this module's .cold): MMEM:CAT? (csvlog
/// catalog_scpi) runs them, and a host may list the files while the Wave
/// view's stream ISR runs (emu/hy3131/wavetest.py, region C hooks).
#[cfg_attr(target_os = "none", link_section = ".text.fat_rd")]
pub(crate) fn parse_bpb(b: &[u8], dev_blocks: u32) -> Option<Vol> {
    let bps = le16(b, 0x0B);
    let spc = b[0x0D] as u32;
    let rsvd = le16(b, 0x0E);
    let nfats = b[0x10] as u32;
    let root_entries = le16(b, 0x11);
    let tot16 = le16(b, 0x13);
    let fatsz = le16(b, 0x16);
    let tot = if tot16 != 0 { tot16 } else { le32(b, 0x20) };
    if !(b[0] == 0xEB || b[0] == 0xE9) || b[0x1FE] != 0x55 || b[0x1FF] != 0xAA {
        return None;
    }
    if bps as usize != BLK || spc == 0 || spc > 128 || !spc.is_power_of_two() {
        return None;
    }
    if rsvd == 0 || !(1..=2).contains(&nfats) || root_entries == 0 || fatsz == 0 {
        return None;
    }
    if (root_entries * 32) % bps != 0 || tot == 0 || tot > dev_blocks {
        return None;
    }
    let root_blocks = root_entries * 32 / bps;
    let data_start = rsvd + nfats * fatsz + root_blocks;
    if data_start >= tot {
        return None;
    }
    let clusters = (tot - data_start) / spc;
    // FAT16 by the cluster-count rule, and the FAT must cover every cluster.
    if !(4085..65525).contains(&clusters) || fatsz * bps / 2 < clusters + 2 {
        return None;
    }
    Some(Vol {
        spc,
        data_start,
        fat_start: rsvd,
        fat_blocks: fatsz,
        nfats,
        root_start: rsvd + nfats * fatsz,
        root_blocks,
        root_entries,
        clusters,
        dn: 0,
        dcl: [0; DIR_MAX_CL],
    })
}

fn up(c: u8) -> u8 {
    c.to_ascii_uppercase()
}

/// 8.3 name `DM40*.BIN` (case-insensitive).
fn sfn_matches(e: &[u8]) -> bool {
    up(e[0]) == b'D' && up(e[1]) == b'M' && e[2] == b'4' && e[3] == b'0' && up(e[8]) == b'B' && up(e[9]) == b'I' && up(e[10]) == b'N'
}

#[cfg_attr(target_os = "none", link_section = ".text.fat_rd")]
pub(crate) fn sfn_is(e: &[u8], name: &[u8; 11]) -> bool {
    e[..11].iter().zip(name.iter()).all(|(&a, &b)| up(a) == b)
}

fn sfn_checksum(e: &[u8]) -> u8 {
    let mut s = 0u8;
    for &c in &e[0..11] {
        s = ((s & 1) << 7).wrapping_add(s >> 1).wrapping_add(c);
    }
    s
}

/// Long name `dm40*.bin` (case-insensitive) from the LFN entries `run`
/// (directory order: the last entry holds the first 13 characters).
fn lfn_matches(run: &[[u16; 13]]) -> bool {
    let mut name = [0u16; MAX_LFN * 13];
    let mut n = 0;
    'outer: for part in run.iter().rev() {
        for &c in part {
            if c == 0 || c == 0xFFFF {
                break 'outer;
            }
            name[n] = c;
            n += 1;
        }
    }
    let lc = |c: u16| if (b'A' as u16..=b'Z' as u16).contains(&c) { c + 32 } else { c };
    let eq = |s: &[u16], t: &[u8]| s.len() == t.len() && s.iter().zip(t).all(|(&a, &b)| lc(a) == b as u16);
    n >= 8 && eq(&name[..4], b"dm40") && eq(&name[n - 4..n], b".bin")
}

#[derive(Clone, Copy)]
struct Hit {
    /// Global root-entry index of the 8.3 entry, its LFN entries, start cluster.
    sfn: u32,
    lfn: [u32; MAX_LFN],
    nlfn: usize,
    start: u32,
    size: u32,
    name: [u8; 11],
}

const NO_HIT: Hit = Hit { sfn: 0, lfn: [0; MAX_LFN], nlfn: 0, start: 0, size: 0, name: [b' '; 11] };

struct Scan {
    /// The first MAX_HITS dm40*.bin other than the trigger file (directory
    /// order), the count of all of them.
    hits: [Hit; MAX_HITS],
    nhits: usize,
    found: u8,
    backup: Option<Hit>,
    fake: Option<Hit>,
    /// First free root slot (0xE5 or the 0x00 end marker).
    free_slot: Option<u32>,
}

/// Scan the root directory (read-only).
fn scan(dev: &mut dyn BlockDevice, v: &Vol, buf: &mut [u8; BLK]) -> Option<Scan> {
    let mut s = Scan { hits: [NO_HIT; MAX_HITS], nhits: 0, found: 0, backup: None, fake: None, free_slot: None };
    // Consecutive live LFN entries immediately before the current entry.
    let mut run = [0u32; MAX_LFN];
    let mut run_ck = [0u8; MAX_LFN];
    let mut run_txt = [[0u16; 13]; MAX_LFN];
    let mut nrun = 0usize;
    let per = (BLK / 32) as u32;
    for blk in 0..v.root_blocks {
        if !dev.read_block(v.root_start + blk, buf) {
            return None;
        }
        for k in 0..per {
            let idx = blk * per + k;
            if idx >= v.root_entries {
                return Some(s);
            }
            let e = &buf[k as usize * 32..k as usize * 32 + 32];
            if e[0] == 0x00 {
                s.free_slot.get_or_insert(idx);
                return Some(s); // end of directory
            }
            if e[0] == 0xE5 {
                s.free_slot.get_or_insert(idx);
                nrun = 0;
                continue;
            }
            let attr = e[11];
            if attr & 0x3F == 0x0F {
                if nrun == MAX_LFN {
                    run.copy_within(1.., 0);
                    run_ck.copy_within(1.., 0);
                    run_txt.copy_within(1.., 0);
                    nrun -= 1;
                }
                run[nrun] = idx;
                run_ck[nrun] = e[13];
                for (j, o) in [1, 3, 5, 7, 9, 14, 16, 18, 20, 22, 24, 28, 30].iter().enumerate() {
                    run_txt[nrun][j] = le16(e, *o) as u16;
                }
                nrun += 1;
                continue;
            }
            let lfn_run = nrun;
            nrun = 0;
            // Plain files only: no volume label, directory or device entries.
            if attr & 0x18 != 0 {
                continue;
            }
            // The LFN entries that belong to this 8.3 entry: the trailing ones
            // of the run whose checksum matches.
            let ck = sfn_checksum(e);
            let mut h = Hit { sfn: idx, start: le16(e, 0x1A), size: le32(e, 0x1C), ..NO_HIT };
            h.name.copy_from_slice(&e[0..11]);
            let mut i = lfn_run;
            while i > 0 && run_ck[i - 1] == ck {
                i -= 1;
                h.lfn[h.nlfn] = run[i];
                h.nlfn += 1;
            }
            let long = &run_txt[i..lfn_run];
            if sfn_is(e, &FAKE) {
                s.fake.get_or_insert(h);
            } else if sfn_matches(e) || (!long.is_empty() && lfn_matches(long)) {
                s.found = s.found.saturating_add(1);
                if s.nhits < MAX_HITS {
                    s.hits[s.nhits] = h;
                    s.nhits += 1;
                }
            } else if sfn_is(e, &BACKUP) && s.backup.is_none() {
                s.backup = Some(h);
            }
        }
    }
    Some(s)
}

/// Read back block `lba` and compare with `want`.
pub(crate) fn verify(dev: &mut dyn BlockDevice, lba: u32, want: &[u8; BLK], tmp: &mut [u8; BLK]) -> bool {
    dev.read_block(lba, tmp) && tmp[..] == want[..]
}

/// Apply `f(entry)` to the 8.3 entry of `h` and 0xE5 to its LFN entries,
/// block by block (read, modify, write, verify).
fn edit_entries(dev: &mut dyn BlockDevice, v: &Vol, h: &Hit, f: &dyn Fn(&mut [u8]), buf: &mut [u8; BLK], tmp: &mut [u8; BLK]) -> bool {
    let per = (BLK / 32) as u32;
    for blk in 0..v.root_blocks {
        let (lo, hi) = (blk * per, blk * per + per);
        let in_blk = |i: u32| i >= lo && i < hi;
        if !in_blk(h.sfn) && !h.lfn[..h.nlfn].iter().any(|&i| in_blk(i)) {
            continue;
        }
        if !dev.read_block(v.root_start + blk, buf) {
            return false;
        }
        for &i in &h.lfn[..h.nlfn] {
            if in_blk(i) {
                buf[((i - lo) * 32) as usize] = 0xE5;
            }
        }
        if in_blk(h.sfn) {
            let o = ((h.sfn - lo) * 32) as usize;
            f(&mut buf[o..o + 32]);
        }
        if !(dev.write_block(v.root_start + blk, buf) && verify(dev, v.root_start + blk, buf, tmp)) {
            return false;
        }
    }
    true
}

/// Zero the entries flagged in `bits` (one bit per entry of FAT block `fb`) in
/// every FAT copy.
fn flush(dev: &mut dyn BlockDevice, v: &Vol, fb: u32, bits: &[u32; BLK / 2 / 32], buf: &mut [u8; BLK], tmp: &mut [u8; BLK]) -> bool {
    for k in 0..v.nfats {
        let lba = v.fat_start + k * v.fat_blocks + fb;
        if !dev.read_block(lba, buf) {
            return false;
        }
        for (w, &word) in bits.iter().enumerate() {
            for b in 0..32 {
                if word & (1 << b) != 0 {
                    let o = (w * 32 + b) * 2;
                    buf[o] = 0;
                    buf[o + 1] = 0;
                }
            }
        }
        if !(dev.write_block(lba, buf) && verify(dev, lba, buf, tmp)) {
            return false;
        }
    }
    true
}

/// Free the chain starting at `start`. Follows FAT 1; stops at end-of-chain,
/// at any invalid link (bad, free, reserved, out of range) or after
/// `clusters` steps (loop), freeing what was walked so far. Returns (ok, freed).
/// Freeing only clears bits (1 -> 0), so the FAT blocks are programmed
/// without an erase.
pub(crate) fn free_chain(dev: &mut dyn BlockDevice, v: &Vol, start: u32, fat1: &mut [u8; BLK], buf: &mut [u8; BLK], tmp: &mut [u8; BLK]) -> (bool, u32) {
    let per = (BLK / 2) as u32;
    let max = v.clusters + 1; // highest valid cluster number
    let mut bits = [0u32; BLK / 2 / 32];
    let mut cur_fb = u32::MAX;
    let mut freed = 0u32;
    let mut c = start;
    let mut steps = 0u32;
    if c < 2 || c > max {
        return (true, 0); // empty file (start 0) or nothing sane to free
    }
    loop {
        let fb = c / per;
        if fb >= v.fat_blocks {
            break;
        }
        if fb != cur_fb {
            if cur_fb != u32::MAX && !flush(dev, v, cur_fb, &bits, buf, tmp) {
                return (false, freed);
            }
            bits = [0; BLK / 2 / 32];
            cur_fb = fb;
            if !dev.read_block(v.fat_start + fb, fat1) {
                return (false, freed);
            }
        }
        let i = (c % per) as usize;
        let next = le16(fat1, i * 2);
        if next == 0 {
            break; // already free (e.g. looped back into a freed cluster)
        }
        bits[i / 32] |= 1 << (i % 32);
        freed += 1;
        steps += 1;
        if next >= 0xFFF8 || next < 2 || next > max || steps > v.clusters {
            break;
        }
        c = next;
    }
    if cur_fb != u32::MAX && bits.iter().any(|&w| w != 0) && !flush(dev, v, cur_fb, &bits, buf, tmp) {
        return (false, freed);
    }
    (true, freed)
}

/// Mark the entries of `h` 0xE5, then free its cluster chain in all FATs.
fn delete_file(dev: &mut dyn BlockDevice, v: &Vol, h: &Hit, res: &mut Result, buf: &mut [u8; BLK], tmp: &mut [u8; BLK]) -> bool {
    if !edit_entries(dev, v, h, &|e| e[0] = 0xE5, buf, tmp) {
        return false;
    }
    let mut fat1 = [0u8; BLK];
    let (ok, freed) = free_chain(dev, v, h.start, &mut fat1, buf, tmp);
    res.freed_clusters += freed;
    ok
}

/// Delete every dm40*.bin of the root (8.3 or long name; not
/// _DM40PRV.BIN, not the recovery trigger): entries first, then the chains
/// (upload.rs, before it writes a new one: the loader wants exactly one).
/// Some(count) / None = no sane volume or an I/O error.
pub fn delete_updates(dev: &mut dyn BlockDevice, buf: &mut [u8; BLK], tmp: &mut [u8; BLK]) -> Option<u32> {
    let mut n = 0;
    loop {
        if !dev.read_block(0, buf) {
            return None;
        }
        let v = parse_bpb(buf, dev.block_count())?;
        let s = scan(dev, &v, buf)?;
        if s.nhits == 0 {
            return Some(n);
        }
        let mut res = Result::new(Status::Clean);
        for h in &s.hits[..s.nhits] {
            if !delete_file(dev, &v, h, &mut res, buf, tmp) {
                return None;
            }
            n += 1;
        }
    }
}

/// The first free root slot (0xE5 or the end marker): Some(None) = full.
pub fn free_root_slot(dev: &mut dyn BlockDevice, v: &Vol, buf: &mut [u8; BLK]) -> Option<Option<u32>> {
    scan(dev, v, buf).map(|s| s.free_slot)
}

/// The decoded first 12 bytes (update header) of a dm40*.bin whose first
/// data block is `b`. The update file is encoded in 128-byte blocks
/// (stock/dm40fw.py decode_block): a = b[0x1F], key position kp = a if
/// a < 128 else a >> 1, k = b[kp]; plain = ((c ^ k) - a) except at 0x1F and kp.
pub(crate) fn header_of(b: &[u8]) -> [u8; 12] {
    let a = b[0x1F];
    let kp = if a < 128 { a } else { a >> 1 } as usize;
    let k = b[kp];
    let mut h = [0u8; 12];
    for (i, o) in h.iter_mut().enumerate() {
        *o = if i == kp { k } else { (b[i] ^ k).wrapping_sub(a) };
    }
    h
}

/// CRC-16/0x8005, init 0xFFFF, MSB first, no final XOR (dm40fw.py crc16).
pub(crate) fn crc16(data: &[u8]) -> u16 {
    let mut c = 0xFFFFu16;
    for &b in data {
        c ^= (b as u16) << 8;
        for _ in 0..8 {
            c = if c & 0x8000 != 0 { (c << 1) ^ 0x8005 } else { c << 1 };
        }
    }
    c
}

/// A decoded 12-byte update header (= the loader marker at 0x08008000 after
/// an install) passes the ROM / FS-loader header test: word2 type 0x0001
/// (bytes 10/11 = 01 00), word0 low half == ~high half == CRC-16 of word1 ||
/// word2 (dm40fw.py header_word0). The body CRC is not checked here.
pub fn header_valid(h: &[u8; 12]) -> bool {
    let w0 = le32(h, 0);
    let w2 = le32(h, 8);
    (w2 >> 16) == 1 && (w0 & 0xFFFF) == (!w0 >> 16) & 0xFFFF && crc16(&h[4..12]) as u32 == w0 & 0xFFFF
}

/// Encode one plain block of an update file in place (dm40fw.py
/// encode_block; the inverse of the decode in `header_of`): blocks of 128
/// bytes from file offset 0 (header included); a = p[0x1F], kp = a if a < 128
/// else a >> 1, k = p[kp]; c -> (c + a) ^ k except at 0x1F and kp. A block of
/// at most 0x1F bytes, or with kp outside it (the last, partial one), is
/// stored as is -- but the real FS-loader mis-decodes a short last block of
/// more than 0x1F bytes with kp outside it (stale key from its buffer), so
/// calbackup::FwFile never writes one (FwFile::tail_hazard).
pub(crate) fn encode_block(p: &mut [u8]) {
    const ADD_POS: usize = 0x1F;
    if p.len() <= ADD_POS {
        return;
    }
    let a = p[ADD_POS];
    let kp = if a < 128 { a } else { a >> 1 } as usize;
    if kp >= p.len() {
        return;
    }
    let k = p[kp];
    for c in p.iter_mut() {
        *c = c.wrapping_add(a) ^ k;
    }
    p[ADD_POS] = a;
    p[kp] = k;
}

/// Is `h` the installed image: header equal to `marker`? Needs at least one
/// 128-byte encoding block and a sane first cluster.
fn installed(dev: &mut dyn BlockDevice, v: &Vol, h: &Hit, marker: &[u8; 12], buf: &mut [u8; BLK]) -> Option<bool> {
    if h.size < 128 || h.start < 2 || h.start > v.clusters + 1 {
        return Some(false);
    }
    if !dev.read_block(v.data_start + (h.start - 2) * v.spc, buf) {
        return None;
    }
    Some(header_of(&buf[..]) == *marker)
}

/// Run once. `dry`: only report. `marker`: the 12 bytes at 0x08008000 if they
/// form a valid header (None: nothing is renamed). Uses ~10 KiB of stack (two
/// block buffers + the scan); never inlined, so that frame is gone before the
/// caller goes on to main() (review 2026-10-13 B1).
#[inline(never)]
pub fn run(dev: &mut dyn BlockDevice, dry: bool, marker: Option<[u8; 12]>) -> Result {
    let r = run_inner(dev, dry, marker);
    unsafe { RESULT = r };
    r
}

fn run_inner(dev: &mut dyn BlockDevice, dry: bool, marker: Option<[u8; 12]>) -> Result {
    let mut res = Result::new(Status::Clean);
    if !dev.present() || dev.block_size() as usize != BLK {
        res.status = Status::NoMedium;
        return res;
    }
    let mut buf = [0u8; BLK];
    let mut tmp = [0u8; BLK];
    if !dev.read_block(0, &mut buf) {
        res.status = Status::Error;
        return res;
    }
    let Some(v) = parse_bpb(&buf, dev.block_count()) else {
        res.status = Status::BadBpb;
        return res;
    };
    let Some(s) = scan(dev, &v, &mut buf) else {
        res.status = Status::Error;
        return res;
    };
    res.found = s.found;
    res.others = s.found;
    // Which match is the installed image (the first whose header equals the
    // marker)? Matches beyond MAX_HITS are not checked: counted as staged.
    let mut first: Option<Hit> = None;
    let mut equal = 0u8;
    if let Some(m) = marker {
        for h in &s.hits[..s.nhits] {
            match installed(dev, &v, h, &m, &mut buf) {
                None => {
                    res.status = Status::Error;
                    return res;
                }
                Some(true) => {
                    equal += 1;
                    first.get_or_insert(*h);
                }
                Some(false) => {}
            }
        }
    }
    res.staged = s.found - equal;
    // 0. A leftover recovery trigger (written by an earlier build; we are
    //    booting normally, so the loader did install a valid image since): delete it, or it would put the
    //    loader back into update mode on the next cold boot.
    if let Some(f) = s.fake {
        if dry {
            res.status = Status::DryRun;
        } else {
            res.status = Status::Error;
            if !delete_file(dev, &v, &f, &mut res, &mut buf, &mut tmp) {
                return res;
            }
            res.fake_deleted = true;
            res.status = Status::Clean;
        }
    }
    let Some(first) = first else {
        return res; // nothing installed to rename, an old backup stays as it is
    };
    res.renamed = first.name;
    res.old_backup = s.backup.is_some();
    if dry {
        res.status = Status::DryRun;
        return res;
    }
    res.status = Status::Error;
    // 1. Older backup: entries first, then its clusters.
    if let Some(b) = s.backup {
        if !delete_file(dev, &v, &b, &mut res, &mut buf, &mut tmp) {
            return res;
        }
        res.old_deleted = true;
    }
    // 2. Rename the installed match, drop its long name.
    let rename = |e: &mut [u8]| {
        e[..11].copy_from_slice(&BACKUP);
        e[12] = 0; // NT case flags: plain upper-case 8.3 name
    };
    if !edit_entries(dev, &v, &first, &rename, &mut buf, &mut tmp) {
        return res;
    }
    res.others = s.found - 1;
    // 3. Confirm: one match fewer, and the backup is there.
    match scan(dev, &v, &mut buf) {
        Some(again) if again.found + 1 == s.found && again.backup.map(|b| b.sfn) == Some(first.sfn) => {
            res.status = Status::Renamed;
        }
        _ => {}
    }
    res
}
