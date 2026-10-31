//! Entry after the boot decision: RAM init, then recovery or the normal opendm40
//! main with the board bring-up the stock firmware used to do.

use core::sync::atomic::{AtomicU32, Ordering};

use crate::boot::{self, BootInfo, Mode};
use crate::mmio;

#[path = "splash.rs"]
pub mod splash;

/// Boot phase times for DBG:BOOT? `BOOTMS` / `MAINMS`: [1..=8] = each
/// boot phase's DWT cycles (`phase`; [0] unused: the counter is not
/// cleared by a software reset, so the shim's own time cannot be told from
/// the loader's before it); [9..=11] = main's ms after App::new, after the
/// first frame, after the touch reset (`main_phase`). SRAM
/// (`.sram_bss`, zeroed in init_ram), not the TCM: the stack's headroom.
#[cfg_attr(target_os = "none", link_section = ".sram_bss")]
static PHASE: [AtomicU32; 12] = [const { AtomicU32::new(0) }; 12];
/// DWT at the end of the last phase (board::init restarts the counter, so
/// 0 = its start). SRAM, as PHASE.
#[cfg_attr(target_os = "none", link_section = ".sram_bss")]
static T_LAST: AtomicU32 = AtomicU32::new(0);
const DWT_CYCCNT: u32 = 0xE000_1004;

/// End of boot phase `i` (1..=8): its time.
fn stamp(i: usize) {
    let now = unsafe { mmio::r32(DWT_CYCCNT) };
    PHASE[i].store(now.wrapping_sub(T_LAST.load(Ordering::Relaxed)), Ordering::Relaxed);
    T_LAST.store(now, Ordering::Relaxed);
}

/// End of pre_main phase `i` (1..=6): its time, the splash bar's next
/// part, the watchdog fed.
fn phase(i: usize) {
    stamp(i);
    splash::step();
    boot::feed();
}

/// main.rs: `i` 0 = App::new done (the splash bar's last part), 1 = the
/// first frame drawn (the splash gone), 2 = the touch reset done; `ms` =
/// main's clock. Never inlined: its code stays in region C, not in main's
/// (B).
#[inline(never)]
pub fn main_phase(i: usize, ms: u64) {
    PHASE[9 + i].store(ms as u32, Ordering::Relaxed);
    if i == 0 {
        splash::step();
    }
    if i == 1 {
        splash::done();
    }
}

extern "C" {
    static mut __bss_start: u32;
    static mut __bss_end: u32;
    static mut __sram_bss_start: u32;
    static mut __sram_bss_end: u32;
    static mut __bkp_bss_start: u32;
    static mut __bkp_bss_end: u32;
    static mut __usbhs_bss_start: u32;
    static mut __usbhs_bss_end: u32;
    static mut __sdata: u32;
    static mut __edata: u32;
    static __sidata: u32;
    static __VECTORS: u32;
    static _stack_top: u32;
}

const SCB_VTOR: u32 = 0xE000_ED08;
/// Stack paint word (review 2026-10-13 B1: high-water mark in DBG:BOOT?).
const STACK_PAINT: u32 = 0x5354_4B21; // "!KTS"
/// Not painted below the SP at paint time (init_ram's own frame + margin).
const PAINT_GAP: u32 = 256;

/// Copy .data, zero .bss, VTOR = our table.
unsafe fn init_ram() {
    let mut src = core::ptr::addr_of!(__sidata);
    let mut dst = core::ptr::addr_of_mut!(__sdata);
    let dend = core::ptr::addr_of_mut!(__edata);
    while dst < dend {
        dst.write_volatile(src.read_volatile());
        dst = dst.add(1);
        src = src.add(1);
    }
    let mut p = core::ptr::addr_of_mut!(__bss_start);
    let end = core::ptr::addr_of_mut!(__bss_end);
    while p < end {
        p.write_volatile(0);
        p = p.add(1);
    }
    zero_sram_bss();
    // Paint the free stack (__bss_end .. SP - PAINT_GAP) for the high-water mark.
    let sp: u32;
    core::arch::asm!("mov {}, sp", out(reg) sp);
    let mut q = core::ptr::addr_of_mut!(__bss_end);
    while (q as u32) < sp - PAINT_GAP {
        q.write_volatile(STACK_PAINT);
        q = q.add(1);
    }
    mmio::w32(SCB_VTOR, core::ptr::addr_of!(__VECTORS) as u32);
}

/// Zero .sram_bss (all-zero statics placed in SRAM to keep TCM for the
/// stack; the linker scripts' NOLOAD section).
unsafe fn zero_sram_bss() {
    let mut p = core::ptr::addr_of_mut!(__sram_bss_start);
    let end = core::ptr::addr_of_mut!(__sram_bss_end);
    while p < end {
        p.write_volatile(0);
        p = p.add(1);
    }
}

/// RCU_AHB1EN.USBHSEN: the clock of the unused USBHS core, whose 4 KiB FIFO
/// RAM at 0x40060000 holds `.usbhs_bss` (docs/native-boot.md "Extra RAM areas").
const RCU_AHB1EN: u32 = 0x4002_3830;
const USBHSEN: u32 = 1 << 29;

/// The extra RAM areas (link-native.x `.bkp_bss`, `.usbhs_bss`): clocks on,
/// then zeroed. Every boot, before anything uses them: the RCU is reset with
/// the core, the backup SRAM keeps the previous image's bytes over a reset
/// and an update, and the FIFO RAM holds random bytes after power-up. Only
/// `__bkp_bss_*` is written: the settings record below it and the sequence /
/// boot words above it are untouched (link-native.x asserts).
unsafe fn extra_ram_init() {
    boot::bkp_enable(); // PMU clock, BKPWEN, BKPSRAM clock (as persist::init)
    mmio::modify32(RCU_AHB1EN, 0, USBHSEN);
    let _ = mmio::r32(RCU_AHB1EN); // the clock is on before the first access
    for (mut p, end) in [
        (core::ptr::addr_of_mut!(__bkp_bss_start), core::ptr::addr_of_mut!(__bkp_bss_end)),
        (core::ptr::addr_of_mut!(__usbhs_bss_start), core::ptr::addr_of_mut!(__usbhs_bss_end)),
    ] {
        while p < end {
            p.write_volatile(0);
            p = p.add(1);
        }
    }
}

/// (deepest stack use so far, stack size) in bytes: the first word above
/// __bss_end that is no longer the paint. Stack = __bss_end .. _stack_top
/// (TCM); used == size means it reached .bss (overflow).
pub fn stack_use() -> (u32, u32) {
    let lo = core::ptr::addr_of!(__bss_end) as u32;
    let top = core::ptr::addr_of!(_stack_top) as u32;
    let mut a = lo;
    while a < top && unsafe { (a as *const u32).read_volatile() } == STACK_PAINT {
        a += 4;
    }
    (top - a, top - lo)
}

/// The rest of DBG:BOOT? (boot.rs `put_info`, words 26..56):
///   26 calbackup status (`calbackup::Status` order)  27 written
///   28 conflict  29 differs (bit i = file i of calbackup::FILES: CALIB,
///   BOOTDATA, UID)  30 calbackup ms  31 stack used  32 stack size (bytes)
///   33 SPI-flash divider (4 / 16)  34 its boot check (`ClockCheck`)
///   35..37 fatdir ok / created / failed  38 calibration record CRC ok (1 / 0)
///   39 settings from (`setfile::From`)  40 slot  41 seq  42 the file at
///   boot (`FileState`)  43 the last save (`Save`)  44 saves this boot
///   45 the newest slot's seq  46..53 boot phases in ms (board, fatren,
///   calbk, fatdir, setfile, mirror, vdisk, refresh)  54..56 main's clock in
///   ms at the end of App::new, of the first frame, of the touch reset.
/// The ms are DWT at 200 MHz: 12.5x low on IRC16M.
pub fn boot_words() -> [u32; 31] {
    let r = crate::calbackup::result();
    let (used, size) = stack_use();
    let (div, why) = crate::spiflash::clock();
    let f = crate::fatdir::boot_result();
    let (from, slot, seq, file, save, saves, top) = crate::setfile::info();
    let mut w = [
        r.status as u32,
        r.written as u32,
        r.conflict as u32,
        r.differs as u32,
        r.ms,
        used,
        size,
        div as u32,
        why as u32,
        f.ok as u32,
        f.created as u32,
        f.failed as u32,
        crate::dmm::rec_crc_ok_at_boot() as u32,
        from as u32,
        slot as u32,
        seq,
        file as u32,
        save as u32,
        saves,
        top,
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    ];
    for (i, p) in PHASE.iter().enumerate().skip(1) {
        let v = p.load(Ordering::Relaxed);
        w[19 + i] = if i <= 8 { v / 200_000 } else { v };
    }
    w
}

/// Board bring-up + fatren + calibration backup, then opendm40 main (normal mode).
fn normal(_info: &BootInfo, dry: bool) -> ! {
    pre_main(dry);
    crate::main()
}

/// Everything before main(), in its own never-inlined frame so the SPI-disk
/// block buffers (fatren ~9 KiB, calbackup ~2 KiB + the diskbuf arena, each in its own
/// never-inlined function too) are popped before main()'s 26 KiB frame is
/// pushed (review 2026-10-13 B1).
#[inline(never)]
fn pre_main(dry: bool) {
    let before = crate::board::init();
    boot::set_board(before);
    // The splash and the backlight first: the disk phases below take 0.2 s
    // on a normal boot, seconds on the first boot after an update.
    splash::show();
    phase(1);
    // fatren (rename the INSTALLED dm40*.bin to _DM40PRV.BIN): only the file
    // whose update header equals the 0x08008000 marker the loader wrote (none
    // if the marker is not valid); any other dm40*.bin is staged and stays.
    // Before USB exists. No reset-cause gate (review 2026-10-13 H1).
    let marker = if boot::marker_valid() {
        let mut m = [0u8; 12];
        for (d, w) in m.chunks_mut(4).zip(boot::marker()) {
            d.copy_from_slice(&w.to_le_bytes());
        }
        Some(m)
    } else {
        None
    };
    {
        let mut disk = crate::spiflash::SpiFlash::new();
        crate::fatren::run(&mut disk, dry, marker);
    }
    phase(2);
    // Write-once calibration safety copy (CALIB.BIN, BOOTDATA.BIN, UID.BIN,
    // only the missing ones) on the same volume, after fatren, still before
    // USB. Not affected by fatren-dry-run: it only creates its own files; it never
    // touches dm40*.bin or _DM40PRV.BIN names.
    crate::calbackup::boot();
    phase(3);
    // LOGS/ SHOTS/ SCOPE/ for our files (fatdir.rs): created on the first
    // boot (~1 s), nothing written afterwards.
    {
        let mut disk = crate::spiflash::SpiFlash::new();
        crate::fatdir::boot(&mut disk);
    }
    phase(4);
    // The settings' copy (setfile.rs `_SETTING.BIN`): made on the first
    // boot (2 blocks), then only its slots are read here.
    {
        let mut disk = crate::spiflash::SpiFlash::new();
        crate::setfile::boot(&mut disk);
    }
    phase(5);
    // The INFO/ files' slow-bank bytes mirrored on the SPI disk (vdisk.rs
    // `mirror_boot`: compared every boot, rewritten only when they differ),
    // served from there while an AD2 stream runs.
    #[cfg(feature = "vdisk")]
    crate::vdisk::mirror_boot();
    phase(6);
}

/// main.rs `run`, after the first frame and before USB attaches (moved out
/// of pre_main 2026-10-26: ~0.47 s on the device that the meter page no
/// longer waits for). Never inlined: the buffers are popped before the main
/// loop; region C (no stream runs yet).
#[inline(never)]
pub fn before_usb() {
    T_LAST.store(unsafe { mmio::r32(DWT_CYCCNT) }, Ordering::Relaxed);
    // The generated USB volume (vdisk.rs): built-in tables + CRC-32s of the
    // INFO sources (~0.1 s), before USB.
    #[cfg(feature = "vdisk")]
    crate::vdisk::init(crate::vdisk::mem_read, unsafe {
        core::slice::from_raw_parts(crate::calbackup::FW_BASE as *const u8, crate::calbackup::FW_MAX_BODY)
    });
    stamp(7);
    crate::wdt::kick();
    // Its LOGS / SCOPE / SHOTS files from the SPI disk, so the host's first
    // look is complete (a DLG's CSV size: ~10 ms per 100 KB of logs).
    #[cfg(feature = "vdisk")]
    crate::vdisk::boot_refresh();
    stamp(8);
    crate::wdt::kick();
}

/// Called by the shim (src/shim.rs) with the decision.
pub unsafe fn start(info: BootInfo) -> ! {
    init_ram();
    extra_ram_init();
    // The serial number (USB iSerial, SCSI VPD 0x80, *IDN?) for both modes.
    crate::uid::init();
    boot::set_info(info);
    match info.mode {
        Mode::Recovery => crate::recovery::run(),
        Mode::Normal => normal(&info, cfg!(feature = "fatren-dry-run")),
    }
}
