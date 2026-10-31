//! On-board SPI NOR flash (W25Q128JV, 16 MiB) as the USB disk medium
//! (stock/re/analysis/spiflash_v2_6.h).
//!
//! SPI1 @ 0x40003800 (APB1 50 MHz): SCK = PD3, MISO = PC2, MOSI = PC3 (AF5),
//! CS = PE4 (GPIO, active low). Mode 3, MSB first, polled byte exchange, the
//! same setup as the stock driver. The whole chip is one FAT16 volume with
//! 4096-byte sectors at offset 0, so block N = flash bytes N * 4096.., which is
//! also the erase unit (SE4K): a block write is erase + 16 page programs.
//!
//! A write first compares against the old contents: unchanged blocks are not
//! touched, and blocks that only need 1 -> 0 bits are programmed without the
//! erase (45..400 ms, the main loop stalls meanwhile in `write_block`).
//! `write_step` does the same write in slices of at most ~2 ms per call, with
//! the erase / page programs running in the chip between calls (screenshot.rs:
//! the main loop keeps serving USB and the DMM). Every block read or write
//! first waits for BUSY to clear, so a `SpiFlash` instance never talks to the
//! chip while another one (the MSC disk vs the screenshot writer) has left an
//! erase or program running.
//!
//! Clock: SPI1 PSC /4 = 12.5 MHz (stock /16 = 3.125 MHz). The W25Q128JV is
//! rated 50 MHz for READ 03h and 133 MHz for every other command used here
//! (datasheet fR / FR), and PCLK1 / 2 = 25 MHz is the GD32 SPI limit; /4 keeps
//! a 4x margin on the slowest command while making a 4 KiB block read ~2.7 ms
//! instead of ~10.5 ms. Every write is read back and compared, so a marginal
//! clock would show as a failed write; a marginal READ, though, would let a
//! read-modify-write (fatren / calbackup root and FAT blocks at boot, host
//! reads) carry a bad bit back to the disk "verified". Hence the boot
//! self-check (`clock_check`, once, in the first `new()`, i.e. before fatren /
//! calbackup / the USB disk touch the volume): block 0 and the first FAT
//! block are read at /16 (CRC-32 of each, boot sector 0x55AA), then twice at
//! /4; any difference, a missing 0x55AA or no known chip -> /16 for this boot.
//! `clock()` reports the divider and why (DBG:BOOT? `SPICLK,<div>,<why>`).
//! FORCE_SLOW = true keeps the stock /16 without the check (stock and the
//! FS-loader only ever use /16; /4 is in spec but new on this board).

use crate::mmio::{modify32, r32, w16, w32};
use crate::msc::{BlockDevice, Step};
use crate::wdt;

const RCU_AHB1EN: u32 = 0x4002_3830;
const RCU_APB1EN: u32 = 0x4002_3840;
const GPIOC: u32 = 0x4002_0800;
const GPIOD: u32 = 0x4002_0C00;
const GPIOE: u32 = 0x4002_1000;
const MODER: u32 = 0x00;
const OTYPER: u32 = 0x04;
const OSPEED: u32 = 0x08;
const PUPD: u32 = 0x0C;
const BOP: u32 = 0x18;
const AFRL: u32 = 0x20;

const SPI: u32 = 0x4000_3800;
const CTL0: u32 = SPI + 0x00;
const STAT: u32 = SPI + 0x08;
const DATA: u32 = SPI + 0x0C;
const I2SCTL: u32 = SPI + 0x1C;
const STAT_RBNE: u32 = 1 << 0;
const STAT_TBE: u32 = 1 << 1;
/// TRANS: a byte is shifting or waits in the buffer.
const STAT_TRANS: u32 = 1 << 7;
/// Mode 3, master, software NSS, 8 bit. PSC bits 5:3: 011 = /16 = 3.125 MHz
/// (the stock value 0x031F), 001 = /4 = 12.5 MHz (see the module notes).
const CTL0_SLOW: u32 = 0x031F;
const CTL0_FAST: u32 = 0x030F;
/// Never leave the stock /16 clock (skips the self-check).
pub const FORCE_SLOW: bool = false;
const CTL0_SPIEN: u32 = 1 << 6;

const CS: u32 = 4; // PE4

const CMD_PP: u8 = 0x02;
const CMD_READ: u8 = 0x03;
const CMD_RDSR1: u8 = 0x05;
const CMD_WREN: u8 = 0x06;
const CMD_SE4K: u8 = 0x20;
const CMD_JEDEC_ID: u8 = 0x9F;

const SECTOR: usize = 4096;
const PAGE: usize = 256;
/// Longest BUSY wait of `wait_ready` (cycles, 1 s at 200 MHz; tSE max
/// 400 ms), whatever the SPI clock (review 60605d2 L2: a poll count meant
/// ~11 s of stall on a dead chip at the /16 fallback).
const BUSY_CYC: u32 = crate::clock::HCLK_HZ;
/// Backstop in RDSR polls in case the DWT cycle counter is not running
/// (~2.8 s at /4, ~11 s at /16).
const BUSY_POLLS: u32 = 4_000_000;
/// `new`: longest wait for an erase / program left running by a reset
/// (cycles, 500 ms; tSE max 400 ms) before the JEDEC ID is read.
const BOOT_BUSY_CYC: u32 = crate::clock::HCLK_HZ / 2;
/// `write_step`: CPU time per call (cycles, 2 ms at 200 MHz) and the longest
/// wait for one erase / page program before the chip counts as failed (1 s;
/// tSE max 400 ms, tPP max 3 ms).
const STEP_CYC: u32 = crate::clock::HCLK_HZ / 500;
const OP_TIMEOUT_CYC: u32 = crate::clock::HCLK_HZ;
const DWT_CYCCNT: u32 = 0xE000_1004;

fn cyc() -> u32 {
    unsafe { r32(DWT_CYCCNT) }
}

/// Outcome of the boot clock self-check (`clock()`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum ClockCheck {
    /// No `SpiFlash` created yet.
    NotRun = 0,
    /// /4 checked equal to /16: running /4.
    Ok = 1,
    /// FORCE_SLOW: /16.
    Forced = 2,
    /// No known chip (JEDEC ID): /16, nothing to compare.
    NoChip = 3,
    /// No 0x55AA in block 0 at /16 (blank / foreign disk): nothing known to
    /// compare against, /16.
    NoSig = 4,
    /// The 1st / 2nd /4 read differed from the /16 one: /16.
    Diff1 = 5,
    Diff2 = 6,
}

impl ClockCheck {
    /// DBG:BOOT? text (native.rs).
    #[allow(dead_code)]
    pub fn name(self) -> &'static [u8] {
        match self {
            ClockCheck::NotRun => b"NOTRUN",
            ClockCheck::Ok => b"OK",
            ClockCheck::Forced => b"FORCED",
            ClockCheck::NoChip => b"NOCHIP",
            ClockCheck::NoSig => b"NOSIG",
            ClockCheck::Diff1 => b"DIFF1",
            ClockCheck::Diff2 => b"DIFF2",
        }
    }
}

static CLOCK: core::sync::atomic::AtomicU8 = core::sync::atomic::AtomicU8::new(0);

/// SPI clock divider in use (4 or 16) and why.
pub fn clock() -> (u32, ClockCheck) {
    let c = match CLOCK.load(core::sync::atomic::Ordering::Relaxed) {
        1 => ClockCheck::Ok,
        2 => ClockCheck::Forced,
        3 => ClockCheck::NoChip,
        4 => ClockCheck::NoSig,
        5 => ClockCheck::Diff1,
        6 => ClockCheck::Diff2,
        _ => ClockCheck::NotRun,
    };
    (if c == ClockCheck::Ok { 4 } else { 16 }, c)
}

/// (Re)configure and enable SPI1 with `cfg` (idle bus only).
fn spi_config(cfg: u32) {
    unsafe {
        w32(CTL0, 0);
        modify32(I2SCTL, 1 << 11, 0);
        w32(CTL0, cfg);
        w32(CTL0, cfg | CTL0_SPIEN);
        // Drop a byte left in the receive buffer.
        let _ = r32(DATA);
    }
}

/// CRC-32 of flash block `lba` read at the current clock; also bytes
/// 0x0E..0x10 (BPB reserved sectors) and 0x1FE..0x200 (signature).
fn block_crc(lba: u32) -> (u32, u16, u16) {
    let mut b = [0u8; PAGE];
    let mut c = !0u32;
    let (mut rsvd, mut sig) = (0u16, 0u16);
    for p in 0..SECTOR / PAGE {
        read(lba * SECTOR as u32 + (p * PAGE) as u32, &mut b);
        if p == 0 {
            rsvd = u16::from_le_bytes([b[0x0E], b[0x0F]]);
        }
        if p == 1 {
            sig = u16::from_be_bytes([b[0xFE], b[0xFF]]);
        }
        c = crate::calbackup::crc32_upd(c, &b);
    }
    (!c, rsvd, sig)
}

/// The boot clock self-check (see the module notes). Called with SPI1 at
/// /16 and an idle chip of `blocks` blocks; leaves SPI1 at the chosen clock.
fn clock_check(blocks: u32) -> ClockCheck {
    if FORCE_SLOW {
        return ClockCheck::Forced;
    }
    if blocks == 0 {
        return ClockCheck::NoChip;
    }
    let (b0, rsvd, sig) = block_crc(0);
    if sig != 0x55AA {
        return ClockCheck::NoSig;
    }
    // First FAT block = the reserved sectors (1 on this disk).
    let fat = if rsvd >= 1 && (rsvd as u32) < blocks { rsvd as u32 } else { 1 };
    let (f0, _, _) = block_crc(fat);
    spi_config(CTL0_FAST);
    for pass in 0..2 {
        let (b, _, s) = block_crc(0);
        let (f, _, _) = block_crc(fat);
        if b != b0 || f != f0 || s != 0x55AA {
            spi_config(CTL0_SLOW);
            return if pass == 0 { ClockCheck::Diff1 } else { ClockCheck::Diff2 };
        }
    }
    ClockCheck::Ok
}

pub struct SpiFlash {
    /// JEDEC ID (mfr << 16 | type << 8 | capacity), as read at init.
    pub id: u32,
    blocks: u32,
    /// Sticky: a busy wait timed out, later writes fail without trying.
    failed: bool,
}

fn af_pin(port: u32, pin: u32) {
    unsafe {
        modify32(port + AFRL, 0xF << (4 * pin), 5 << (4 * pin));
        modify32(port + OTYPER, 1 << pin, 0);
        modify32(port + OSPEED, 0, 3 << (2 * pin));
        modify32(port + PUPD, 3 << (2 * pin), 0);
        modify32(port + MODER, 3 << (2 * pin), 2 << (2 * pin));
    }
}

#[inline(always)]
fn cs(active: bool) {
    let bit = if active { 1 << (CS + 16) } else { 1 << CS };
    unsafe { w32(GPIOE + BOP, bit) };
}

#[inline(always)]
fn xfer(b: u8) -> u8 {
    unsafe {
        while r32(STAT) & STAT_TBE == 0 {}
        w16(DATA, b as u16);
        while r32(STAT) & STAT_RBNE == 0 {}
        r32(DATA) as u8
    }
}

/// Transmit only, back to back (the next byte goes into the buffer while one
/// shifts): 0.64 us per byte at 12.5 MHz against ~0.97 us for `xfer`'s
/// write-wait-read per byte (the fast logger's page programs: ~1/3 of their
/// CPU on the device model). The received bytes are dropped: at the end the
/// last byte is waited out (TRANS), DATA read (RBNE) and STAT read (clears
/// the receive overrun), so the next `xfer` starts clean.
fn tx_burst(d: &[u8]) {
    unsafe {
        for &b in d {
            while r32(STAT) & STAT_TBE == 0 {}
            w16(DATA, b as u16);
        }
        while r32(STAT) & (STAT_TRANS | STAT_TBE) != STAT_TBE {}
        let _ = r32(DATA);
        let _ = r32(STAT);
    }
}

fn cmd_addr(cmd: u8, addr: u32) {
    xfer(cmd);
    xfer((addr >> 16) as u8);
    xfer((addr >> 8) as u8);
    xfer(addr as u8);
}

fn read(addr: u32, buf: &mut [u8]) {
    cs(true);
    cmd_addr(CMD_READ, addr);
    for b in buf.iter_mut() {
        *b = xfer(0xFF);
    }
    cs(false);
}

/// One RDSR on the configured bus: the chip runs an erase / page program
/// (vdisk.rs: a mirror sector is read between the fast logger's page
/// programs, never waiting for one).
pub fn chip_busy() -> bool {
    SpiFlash::busy()
}

impl SpiFlash {
    /// Raw bytes at flash address `addr` (any length; dlog.rs reads the
    /// binary logs for the CSV export without a 4 KiB block buffer). Waits
    /// for a running erase / program first, as `read_block`.
    pub fn read_at(&mut self, addr: u32, buf: &mut [u8]) -> bool {
        if addr as u64 + buf.len() as u64 > self.blocks as u64 * SECTOR as u64 || (!self.failed && !self.wait_ready()) {
            return false;
        }
        read(addr, buf);
        true
    }

    /// Set up the pins and SPI1 and identify the chip. A missing or unknown
    /// chip leaves the medium absent rather than showing 0xFF garbage.
    pub fn new() -> Self {
        unsafe {
            modify32(RCU_AHB1EN, 0, (1 << 2) | (1 << 3) | (1 << 4));
            modify32(RCU_APB1EN, 0, 1 << 14);
        }
        cs(false);
        unsafe {
            modify32(GPIOE + MODER, 3 << (2 * CS), 1 << (2 * CS));
            modify32(GPIOE + OTYPER, 1 << CS, 0);
            modify32(GPIOE + OSPEED, 0, 3 << (2 * CS));
        }
        af_pin(GPIOD, 3);
        af_pin(GPIOC, 2);
        af_pin(GPIOC, 3);
        // /16 for the ID (and the self-check); /4 only once checked.
        spi_config(CTL0_SLOW);

        // A reset (DBG:RESET, the watchdog) can land while the chip runs a
        // 4 KiB erase (up to 400 ms; it does not stop on a CPU reset), and a
        // busy chip ignores 9Fh: the ID would read as no chip and the disk
        // stay absent for the whole session (review 60605d2 L1). So wait for
        // WIP to clear first, at most 500 ms, unless an earlier instance
        // already found no chip.
        if CLOCK.load(core::sync::atomic::Ordering::Relaxed) != ClockCheck::NoChip as u8 {
            let t0 = cyc();
            let mut n = 0u32;
            while Self::busy() && cyc().wrapping_sub(t0) < BOOT_BUSY_CYC && n < BUSY_POLLS {
                n += 1;
                if n & 0x3FF == 0 {
                    wdt::kick_if_feeding();
                }
            }
        }

        cs(true);
        xfer(CMD_JEDEC_ID);
        let id = (xfer(0xFF) as u32) << 16 | (xfer(0xFF) as u32) << 8 | xfer(0xFF) as u32;
        cs(false);
        let (mfr, cap) = (id >> 16, id & 0xFF);
        // 3-byte addressing covers up to 16 MiB (capacity code 0x18).
        let blocks = if mfr != 0 && mfr != 0xFF && (0x11..=0x18).contains(&cap) {
            (1u32 << cap) / SECTOR as u32
        } else {
            0
        };
        // The clock self-check, once per boot (the first instance; nothing
        // else can have the chip busy then).
        use core::sync::atomic::Ordering::Relaxed;
        if CLOCK.load(Relaxed) == ClockCheck::NotRun as u8 {
            CLOCK.store(clock_check(blocks) as u8, Relaxed);
        } else if clock().0 == 4 {
            spi_config(CTL0_FAST);
        }
        SpiFlash { id, blocks, failed: false }
    }

    /// A busy wait timed out (writes disabled until reset).
    pub fn failed(&self) -> bool {
        self.failed
    }

    /// Wait for BUSY to clear, kicking the watchdog while the main loop does
    /// (an erase takes up to 400 ms, longer than the loop may stall).
    fn wait_ready(&mut self) -> bool {
        if self.failed {
            return false;
        }
        cs(true);
        xfer(CMD_RDSR1);
        let t0 = cyc();
        let mut n = 0;
        while xfer(0xFF) & 1 != 0 {
            n += 1;
            if n & 0x3FF == 0 {
                wdt::kick_if_feeding();
            }
            if n >= BUSY_POLLS || cyc().wrapping_sub(t0) > BUSY_CYC {
                self.failed = true;
                break;
            }
        }
        cs(false);
        !self.failed
    }

    /// One RDSR: an erase / program is still running (no waiting).
    fn busy() -> bool {
        cs(true);
        xfer(CMD_RDSR1);
        let b = xfer(0xFF) & 1 != 0;
        cs(false);
        b
    }

    fn write_enable() {
        let w = &crate::msc::FLASH_WRITES;
        w.store(w.load(core::sync::atomic::Ordering::Relaxed).wrapping_add(1), core::sync::atomic::Ordering::Relaxed);
        cs(true);
        xfer(CMD_WREN);
        cs(false);
    }

    /// Start a 4 KiB sector erase (returns at once; BUSY while it runs).
    fn start_erase(addr: u32) {
        Self::write_enable();
        cs(true);
        cmd_addr(CMD_SE4K, addr);
        cs(false);
    }

    /// Start a page program (returns once the data is clocked out).
    fn start_program(addr: u32, data: &[u8]) {
        Self::write_enable();
        cs(true);
        cmd_addr(CMD_PP, addr);
        for &b in data {
            xfer(b);
        }
        cs(false);
    }

    /// `start_program` of one page given as PAGE / 4 little-endian words.
    fn start_program_words(addr: u32, word: impl Fn(usize) -> u32) {
        Self::write_enable();
        cs(true);
        cmd_addr(CMD_PP, addr);
        for j in 0..PAGE / 4 {
            for b in word(j).to_le_bytes() {
                xfer(b);
            }
        }
        cs(false);
    }

    fn erase(&mut self, addr: u32) -> bool {
        Self::start_erase(addr);
        self.wait_ready()
    }

    fn program(&mut self, addr: u32, data: &[u8]) -> bool {
        Self::start_program(addr, data);
        self.wait_ready()
    }
}

/// Raw page programs / erases for the fast logger (fastlog.rs, fastenc.rs):
/// never a wait, never a compare or read-back (the chunk CRCs check the
/// data), never a FAT / directory block (those go through `write_block`).
/// BE64 = 64 KiB block erase (D8h, tBE1 150 ms typ / 2 s max).
const CMD_BE64: u8 = 0xD8;

impl crate::fastenc::Nor for SpiFlash {
    #[inline(always)]
    fn busy(&mut self) -> bool {
        Self::busy()
    }
    fn program(&mut self, addr: u32, d: &[u8]) {
        if !self.failed && (addr as u64 + d.len() as u64) <= self.blocks as u64 * SECTOR as u64 {
            // As `start_program`, the data bytes back to back (`tx_burst`);
            // the chunk CRCs check them (no read-back here).
            Self::write_enable();
            cs(true);
            cmd_addr(CMD_PP, addr);
            tx_burst(d);
            cs(false);
        }
    }
    fn dry_page(&mut self, addr: u32, n: usize) {
        // A READ command and `n` bytes back to back, as `program` clocks a
        // page out (the chip only shifts data; nothing is written).
        if !self.failed && addr < self.blocks * SECTOR as u32 {
            cs(true);
            cmd_addr(CMD_READ, addr);
            tx_burst(&[0xFF; 256][..n.min(256)]);
            cs(false);
        }
    }
    fn erase(&mut self, addr: u32, big: bool) {
        if self.failed || addr >= self.blocks * SECTOR as u32 {
            return;
        }
        if big {
            Self::write_enable();
            cs(true);
            cmd_addr(CMD_BE64, addr);
            cs(false);
        } else {
            Self::start_erase(addr);
        }
    }
    fn read(&mut self, addr: u32, b: &mut [u8]) -> bool {
        self.read_at(addr, b)
    }
}

impl BlockDevice for SpiFlash {
    fn present(&self) -> bool {
        self.blocks != 0
    }
    fn block_size(&self) -> u32 {
        SECTOR as u32
    }
    fn block_count(&self) -> u32 {
        self.blocks
    }

    fn read_block(&mut self, lba: u32, buf: &mut [u8]) -> bool {
        // (A chip that already timed out once is read without waiting, as
        // before the wait existed; writes stay disabled.)
        if lba >= self.blocks || buf.len() < SECTOR || (!self.failed && !self.wait_ready()) {
            return false;
        }
        read(lba * SECTOR as u32, &mut buf[..SECTOR]);
        true
    }

    fn write_block(&mut self, lba: u32, buf: &[u8]) -> bool {
        if lba >= self.blocks || buf.len() < SECTOR || !self.wait_ready() {
            return false;
        }
        let base = lba * SECTOR as u32;
        let data = &buf[..SECTOR];

        // Which pages differ, and can they be programmed over the old data?
        let mut dirty = 0u16;
        let mut need_erase = false;
        let mut old = [0u8; PAGE];
        for (p, new) in data.chunks(PAGE).enumerate() {
            read(base + (p * PAGE) as u32, &mut old);
            if old[..] != new[..] {
                dirty |= 1 << p;
                need_erase |= old.iter().zip(new).any(|(&o, &n)| o & n != n);
            }
        }
        if dirty == 0 {
            return true;
        }
        if need_erase {
            if !self.erase(base) {
                return false;
            }
            // Everything is 0xFF now: program every page that is not.
            dirty = 0;
            for (p, new) in data.chunks(PAGE).enumerate() {
                if new.iter().any(|&b| b != 0xFF) {
                    dirty |= 1 << p;
                }
            }
        }
        for (p, new) in data.chunks(PAGE).enumerate() {
            if dirty & (1 << p) != 0 && !self.program(base + (p * PAGE) as u32, new) {
                return false;
            }
        }
        // Read back.
        for (p, new) in data.chunks(PAGE).enumerate() {
            read(base + (p * PAGE) as u32, &mut old);
            if old[..] != new[..] {
                return false;
            }
        }
        true
    }

    /// `write_block` in slices: compare (phase 0), erase if any bit must go
    /// 0 -> 1 (1), program the pages that differ / are not blank (2), read
    /// back (3). Returns None after ~2 ms of CPU time or while the chip is
    /// busy; the erase (45..400 ms) and each page program (0.4..3 ms) run
    /// between calls. Page reads at 12.5 MHz take ~0.17 ms.
    fn write_step(&mut self, lba: u32, buf: &dyn crate::wordram::BlockSrc, st: &mut Step) -> Option<bool> {
        if lba >= self.blocks || buf.len() < SECTOR || self.failed {
            return Some(false);
        }
        let t_in = cyc();
        if !st.started {
            st.started = true;
            st.t0 = t_in;
        }
        let base = lba * SECTOR as u32;
        // `buf` is read word by word (it may be a word-only RAM, wordram.rs).
        const PW: usize = PAGE / 4;
        let word = |p: u8, j: usize| buf.word(p as usize * PW + j);
        let old_w = |old: &[u8; PAGE], j: usize| u32::from_le_bytes([old[4 * j], old[4 * j + 1], old[4 * j + 2], old[4 * j + 3]]);
        let mut old = [0u8; PAGE];
        loop {
            // Our erase / page program, or another instance's, still running.
            if Self::busy() {
                if cyc().wrapping_sub(st.t0) > OP_TIMEOUT_CYC {
                    self.failed = true;
                    return Some(false);
                }
                if cyc().wrapping_sub(t_in) >= STEP_CYC {
                    return None;
                }
                continue;
            }
            match st.phase {
                0 => {
                    read(base + st.page as u32 * PAGE as u32, &mut old);
                    for j in 0..PW {
                        let (o, n) = (old_w(&old, j), word(st.page, j));
                        if o != n {
                            st.dirty |= 1 << st.page;
                            st.erase |= o & n != n;
                        }
                    }
                    st.page += 1;
                    if st.page as usize == SECTOR / PAGE {
                        st.page = 0;
                        if st.dirty == 0 {
                            return Some(true);
                        }
                        if st.erase {
                            Self::start_erase(base);
                            st.t0 = cyc();
                            st.phase = 1;
                        } else {
                            st.phase = 2;
                        }
                    }
                }
                1 => {
                    // Erased: every page that is not all 0xFF must be programmed.
                    st.dirty = 0;
                    for p in 0..(SECTOR / PAGE) as u8 {
                        if (0..PW).any(|j| word(p, j) != 0xFFFF_FFFF) {
                            st.dirty |= 1 << p;
                        }
                    }
                    st.phase = 2;
                }
                2 => {
                    while (st.page as usize) < SECTOR / PAGE && st.dirty & (1 << st.page) == 0 {
                        st.page += 1;
                    }
                    if st.page as usize == SECTOR / PAGE {
                        st.page = 0;
                        st.phase = 3;
                    } else {
                        Self::start_program_words(base + st.page as u32 * PAGE as u32, |j| word(st.page, j));
                        st.t0 = cyc();
                        st.page += 1;
                    }
                }
                _ => {
                    read(base + st.page as u32 * PAGE as u32, &mut old);
                    if (0..PW).any(|j| old_w(&old, j) != word(st.page, j)) {
                        return Some(false);
                    }
                    st.page += 1;
                    if st.page as usize == SECTOR / PAGE {
                        return Some(true);
                    }
                }
            }
            if cyc().wrapping_sub(t_in) >= STEP_CYC {
                return None;
            }
        }
    }
}
