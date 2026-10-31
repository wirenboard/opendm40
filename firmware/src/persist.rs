//! Settings: the live copy in the 4 KiB backup SRAM (0x40024000), a copy on
//! the SPI disk in `_SETTING.BIN` (setfile.rs). The backup SRAM keeps its
//! content over a reset and a firmware update, but it is not powered while
//! the meter is off (device 2026-10-24: POR + BOR after a key power-off, the
//! record gone), so the file copy is what survives a power-off. The stock
//! firmware keeps its own settings in config.bin on the same disk; neither
//! firmware touches the other's file.
//!
//! Backup SRAM layout: magic, version, payload, CRC32; anything that does not
//! check out falls back to the file copy, then to the defaults. The file
//! slots carry the same version and payload (setfile.rs), decoded through
//! the same `words_of` table, so an older record migrates the same way from
//! either copy. `seq` (two words below the boot shim's) is the file
//! sequence number the backup SRAM copy continues: at boot a file slot with
//! a higher one is newer than the backup SRAM. The boot shim's words at the
//! end of the 4 KiB (boot.rs, 0x40024FF0..) are outside the record (v11:
//! 0x40024000..0x40024123).

#[cfg_attr(not(target_os = "none"), allow(unused_imports))]
use crate::mmio::{modify32, r32, w32};

const RCU_AHB1EN: u32 = 0x4002_3830;
const RCU_APB1EN: u32 = 0x4002_3840;
const PMU_CTL: u32 = 0x4000_7000;
const PMU_CS: u32 = 0x4000_7004;
const BKP: u32 = 0x4002_4000;
const MAGIC: u32 = 0x4446_5753; // "SWFD"
/// v4: 40 words (v3 had 24, v2 16; an older record is loaded zero-extended,
/// every new field reads 0 = its default). Layout: app.rs persist_words().
/// p[38] / p[39] were spare in v4 and are now W_DMM_EXT / W_GEN_OFFS. A v4
/// record from before reads 0 there = defaults, so no version change.
/// v5: 48 words; p[40..48] the scope page's math / FFT and trigger-type
/// settings (scope_page.rs sp_persist_ext; 0 = defaults, so a v4 record
/// loads zero-extended with nothing to migrate).
/// v6: 52 words; p[48..52] the power meter's settings (app_power.rs
/// pw_words; 0 = defaults, so a v5 record loads zero-extended).
/// v7: 58 words; the power block grows to 10 words (p[48..58]: V gains,
/// LPF zeros, the wiring-dialog flag; 0 = defaults, v6 loads zero-extended).
/// v8: 59 words; p[58] W_REC, the Record page's interval / duration
/// (0 = every reading / until stopped, so v7 loads zero-extended).
/// v9: 62 words; p[59..62] W_POWER_ZD, the power meter DC row's AD1 zeros
/// (app_power.rs; 0 = none, so v8 loads zero-extended).
/// v10: 64 words; p[62..64] W_LIM_W, the power meter's W limits (lower /
/// upper, f32 bits; their on / set bits are bit 17 of p[18] and bits 9 / 25
/// of p[19], 0 in older records = off / unset).
/// v11: 70 words; p[64..70] W_TOUCH_CAL, the touch calibration
/// (touchcal.rs Cal: Q16 deltas from the identity; 0 = none, so v10 loads
/// zero-extended as the identity).
pub const VERSION: u32 = 11;
pub const WORDS: usize = 70;
const V10_WORDS: usize = 64;
/// The touch calibration (6 words, touchcal::Cal; a preference).
pub const W_TOUCH_CAL: usize = 64;
const V9_WORDS: usize = 62;
/// The power meter's limit slot (meter::LIM_POWER): lower, upper (2 words).
pub const W_LIM_W: usize = 62;
const V8_WORDS: usize = 59;
/// The power meter DC row's AD1 zeros (app_power::PW_ZD_WORDS words).
pub const W_POWER_ZD: usize = 59;
const V7_WORDS: usize = 58;
/// Record page (decision 2026-10-22 #1): bits 0..3 the row interval
/// (csvlog::INTERVALS_S index; csvlog::ON_CHANGE = On change), bits 4..7
/// the duration (DURATIONS_S index), bits 8..9 the On change threshold
/// (0 = 1 %, the default, 1 = 0.1 %, 2 = 5 %; app.rs swap01). Older
/// records read 0 there = the defaults: no version change.
pub const W_REC: usize = 58;
const V6_WORDS: usize = 52;
/// First word of the power-meter block (app_power::PW_WORDS words).
pub const W_POWER: usize = 48;
const V5_WORDS: usize = 48;
/// First word of the scope page's extension block (8 words).
pub const W_SCOPE_EXT: usize = 40;
const V4_WORDS: usize = 40;
/// FR-05 conversion rate + display smoothing, UF-21 V-autorange migration
/// flag: meter::ext_word() / meter::ext_settings() pack and decode it.
pub const W_DMM_EXT: usize = 38;
/// FR-28 generator offset: 0 = auto (stock 0..Vpp), else the offset in V as
/// f32 bits with the sign bit set (app.rs persist_words / restore).
pub const W_GEN_OFFS: usize = 39;
const V3_WORDS: usize = 24;
/// The file sequence number the backup SRAM copy continues (`seq`): the
/// value and its complement, below the boot shim's words.
const BKP_SEQ: u32 = 0x4002_4FE8;
/// The record (magic, version, WORDS, CRC) must end below the sequence words
/// and they below the boot shim's words at 0x40024FF0 (boot.rs
/// BKP_CRASH..BKP_LAST).
const _: () = assert!(BKP + 4 * (WORDS as u32 + 3) <= BKP_SEQ, "persist record overlaps the sequence words");
const _: () = assert!(BKP_SEQ + 8 <= 0x4002_4FF0, "persist sequence words overlap the boot words");
/// link-native.x ends the backup SRAM region (`BKPSRAM`) at the sequence words.
const _: () = assert!(BKP_SEQ == 0x4002_4FE8, "link-native.x BKPSRAM ends at BKP_SEQ: change both");
/// The record's place at the start of the backup SRAM, for the linker only:
/// link-native.x puts this first in `BKPSRAM` (section `.bkp_record`,
/// asserted at 0x40024000), so the CPU-only buffers in `.bkp_bss` start
/// after the record whatever `WORDS` is. Never read or written through this
/// static (the record is accessed at `BKP` + offsets); NOLOAD, so the image
/// does not touch the record either.
#[cfg(target_os = "none")]
#[used]
#[link_section = ".bkp_record"]
static RECORD_SPACE: [u32; WORDS + 3] = [0; WORDS + 3];
const V2_WORDS: usize = 16;

/// Payload words of a record of format `version` (2..VERSION), None for a
/// version this build does not know. Shared by the backup SRAM and the file
/// slots (setfile.rs): an older record is loaded zero-extended to `WORDS`.
pub fn words_of(version: u32) -> Option<usize> {
    Some(match version {
        VERSION => WORDS,
        10 => V10_WORDS,
        9 => V9_WORDS,
        8 => V8_WORDS,
        7 => V7_WORDS,
        6 => V6_WORDS,
        5 => V5_WORDS,
        4 => V4_WORDS,
        3 => V3_WORDS,
        2 => V2_WORDS,
        _ => return None,
    })
}

/// CRC-32 (IEEE, as calbackup::crc32) of the words' little-endian bytes.
pub fn crc(words: &[u32]) -> u32 {
    let mut c = 0xFFFF_FFFFu32;
    for &w in words {
        for b in w.to_le_bytes() {
            c ^= b as u32;
            for _ in 0..8 {
                c = if c & 1 != 0 { (c >> 1) ^ 0xEDB8_8320 } else { c >> 1 };
            }
        }
    }
    !c
}

#[cfg(target_os = "none")]
pub fn init() {
    unsafe {
        modify32(RCU_APB1EN, 0, 1 << 28); // PMU
        modify32(PMU_CTL, 0, 1 << 8); // BKPWEN: backup domain write access
        modify32(RCU_AHB1EN, 0, 1 << 18); // BKPSRAM clock
        modify32(PMU_CS, 0, 1 << 9); // BLDOON: keep backup SRAM on VBAT
    }
}

/// A record that passed the checks: its format version (2..4) and the
/// payload (a v2 record zero-extended to `WORDS`).
pub struct Record {
    pub version: u32,
    pub words: [u32; WORDS],
}

/// The stored record, or None when the magic, the version or the CRC does not
/// check out (never written, written by other firmware, a layout this build
/// does not know, a torn write): the caller then keeps its defaults.
pub fn load() -> Option<Record> {
    let rd = |i: usize| unsafe { r32(BKP + 4 * i as u32) };
    if rd(0) != MAGIC {
        return None;
    }
    let version = rd(1);
    let n = words_of(version)?;
    let mut p = [0u32; WORDS];
    for (i, w) in p[..n].iter_mut().enumerate() {
        *w = rd(2 + i);
    }
    if crc(&p[..n]) != rd(2 + n) { None } else { Some(Record { version, words: p }) }
}

/// Whether the DMM function in the record belongs to the session the user
/// just left. The record is only rewritten while opendm40 runs; the stock
/// firmware (own settings in config.bin, power-on default DC V) and the
/// vendor loader never touch it. So after a round trip through other
/// firmware it holds whatever opendm40 measured last, possibly days and another
/// firmware ago (seen: the first native boot came up in AC+DC from an old
/// session although the user had been measuring DC V on the stock firmware).
/// The boot shim records every normal opendm40 boot in BKP_LAST
/// (boot.rs; DBG:UPDATE / DBG:RECOVER clear it, a recovery leaves "RCVR"), so
/// only a previous boot that was a normal opendm40 one continues the mode.
#[cfg(target_os = "none")]
pub fn same_session() -> bool {
    // boot.rs LAST_NORMAL ("NRML"), as the shim found BKP_LAST.
    const LAST_NORMAL: u32 = 0x4C4D_524E;
    crate::boot::info().is_some_and(|i| i.last == LAST_NORMAL)
}

pub fn save(p: &[u32; WORDS]) {
    unsafe {
        w32(BKP, MAGIC);
        w32(BKP + 4, VERSION);
        for (i, &v) in p.iter().enumerate() {
            w32(BKP + 8 + 4 * i as u32, v);
        }
        w32(BKP + 8 + 4 * WORDS as u32, crc(p));
    }
}

/// Version and stored CRC of the backup SRAM record, without checking the
/// CRC (setfile.rs `poll`: a cheap "did it change" per main-loop pass; the
/// copy into the file checks it). None: no record of a known version.
#[inline(always)]
pub fn stored() -> Option<(u32, u32)> {
    let rd = |i: usize| unsafe { r32(BKP + 4 * i as u32) };
    if rd(0) != MAGIC {
        return None;
    }
    let n = words_of(rd(1))?;
    Some((rd(1), rd(2 + n)))
}

/// Payload word `i` of the backup SRAM record (setfile.rs copies it).
#[inline(always)]
pub fn word(i: usize) -> u32 {
    unsafe { r32(BKP + 8 + 4 * i as u32) }
}

/// The file sequence number the backup SRAM copy continues, None when the
/// two words do not check out (power-on, an older build).
pub fn seq() -> Option<u32> {
    let (s, n) = unsafe { (r32(BKP_SEQ), r32(BKP_SEQ + 4)) };
    if s == !n { Some(s) } else { None }
}

pub fn set_seq(s: u32) {
    unsafe {
        w32(BKP_SEQ, s);
        w32(BKP_SEQ + 4, !s);
    }
}
