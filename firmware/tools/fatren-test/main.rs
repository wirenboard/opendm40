//! Host test for src/fatren.rs, src/calbackup.rs (+ src/vdisk.rs) against a real FAT16 image
//! (run.sh builds one with mkfs.fat/mtools in the DM40 geometry: 4096-byte
//! sectors, 2 FATs, 512 root entries, 16 MiB).
//!   main <image> dry|live [marker-hex]  fatren::run (marker = the 12-byte
//!                                       record at 0x08008000; none = invalid)
//!   main <image> trigger                fatren::make_trigger
//!   main <image> backup <dir>           calbackup::safe_copy (write-once safety copy)
//!                                       of calbackup::FILES[..SAFE] read from
//!                                       <dir>/CALIB.BIN, BOOTDATA.BIN, UID.BIN
//!   main <out> vdisk <flash> <uid> <otp> <optbytes> [step]...
//!                                       vdisk.rs (the read-only USB volume) over the
//!                                       1 MiB <flash> image (0x08000000..) and the
//!                                       system-memory files: every sector read through
//!                                       read_block into the sparse <out>; write_block /
//!                                       out-of-range reads refused; steps (spi <img>,
//!                                       locked, free, reset, forget <NAME>) each
//!                                       followed by a dump to <out>.<i> (vdisk_dump)
//!   main <image> fatdir boot|ensure <NAME>|rm <NAME> [cut <k>]
//!                                       fatdir.rs (fatdir_cmd)
//!   main <image> upload <file> [crc <hex>] [order] [chunk <n>] [cut <k>]
//!                                       upload.rs as main.rs drives it (upload_cmd)
//!   main <image> shot                   screenshot::take_on with the synthetic screen
//!                                       of the `lcd` stub below (pattern()).
//!   main <image> shotcut [k [abort]]    the same slice by slice on a NOR model of
//!                                       write_step, cut after k slices (or stopped
//!                                       there by screenshot::abort_on); see shotcut().
//!   main <image> csv rec <rows> [cut <k> [half]] [shot <row>]... [host <row> <rows>] [mount|mountwin <row> <rows>]
//!                                       csvlog recording of <rows> synthetic rows
//!                                       (row(i) below), polled pass by pass;
//!                                       cut = power cut before the k-th changing
//!                                       block write (half: a program-only write is
//!                                       cut half way, 8 of 16 pages); shot = start
//!                                       a screenshot at that row (interleaved);
//!                                       host = HOST_WRITING set for that many rows;
//!                                       mount = msc::host_mounted() for that many rows
//!                                       (no effect since the PC cannot mount the SPI
//!                                       disk; kept for the knob); mount_writes = block
//!                                       writes in that window.
//!   main <image> csv list | del <NAME> | busydel
//!                                       gallery list / delete; busydel = delete the
//!                                       file being recorded (must be refused).
//! Live runs rewrite the image.
use std::{env, fs};

/// println! that ignores a closed stdout. run.sh pipes the output into
/// `grep -q`, which exits at its first match: std's println! then panics
/// on EPIPE, and a panic before the `fs::write` of the image at the end of
/// a command loses everything the command wrote (seen as random failures:
/// a file still there after a delete, a missing SHOTS/S000.BMP, a rename
/// not done), depending on whether grep had exited before the next line.
macro_rules! println {
    ($($t:tt)*) => {{
        use std::io::Write as _;
        let _ = writeln!(std::io::stdout(), $($t)*);
    }};
}

/// Writes a disk image sparse (only its non-zero 4 KiB blocks), as
/// mkimg's `truncate` makes them: /tmp is a tmpfs with a per-user quota
/// shared with everything else of the user, and ~40 dense 16 MiB images
/// per run made parallel runs fail with EDQUOT at random points.
fn save(path: &str, d: &[u8]) {
    use std::io::{Seek, SeekFrom, Write};
    let mut f = fs::File::create(path).unwrap();
    for (i, b) in d.chunks(4096).enumerate() {
        if b.iter().any(|&x| x != 0) {
            f.seek(SeekFrom::Start(i as u64 * 4096)).unwrap();
            f.write_all(b).unwrap();
        }
    }
    f.set_len(d.len() as u64).unwrap();
}

#[allow(dead_code)]
mod msc {
    /// Same trait as opendm40 src/msc.rs (kept in sync by hand; 10 methods,
    /// write_step with its default = the whole write at once).
    pub trait BlockDevice {
        fn present(&self) -> bool;
        fn block_size(&self) -> u32;
        fn block_count(&self) -> u32;
        fn read_block(&mut self, lba: u32, buf: &mut [u8]) -> bool;
        fn write_block(&mut self, lba: u32, buf: &[u8]) -> bool;
        fn read_only(&self) -> bool {
            false
        }
        fn read_wait(&mut self, lba: u32) -> bool {
            let _ = lba;
            false
        }
        fn take_media_changed(&mut self) -> bool {
            false
        }
        fn media_change_immediate(&self) -> bool {
            false
        }
        fn product(&self) -> &'static [u8; 16] {
            b"DM40 disk       "
        }
        fn gates_writes(&self) -> bool {
            false
        }
        fn write_error(&self) -> u8 {
            0
        }
        fn read_busy(&self, lba: u32, n: u32) -> bool {
            let _ = (lba, n);
            false
        }
        fn write_step(&mut self, lba: u32, buf: &dyn crate::wordram::BlockSrc, st: &mut Step) -> Option<bool> {
            let _ = st;
            Some(self.write_block(lba, &src_bytes(buf)))
        }
    }
    /// The bytes of a `write_step` source (src/wordram.rs `BlockSrc`).
    pub fn src_bytes(buf: &dyn crate::wordram::BlockSrc) -> Vec<u8> {
        (0..buf.len() / 4).flat_map(|i| buf.word(i).to_le_bytes()).collect()
    }
    #[derive(Clone, Copy, Default)]
    pub struct Step {
        pub started: bool,
        pub phase: u8,
        pub page: u8,
        pub dirty: u16,
        pub erase: bool,
        pub t0: u32,
    }
    pub static HOST_WRITING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    pub static LOG_SESSION: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    pub static LOG_FLUSHING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    /// Bumped by the Img model's every changing write / program / erase (as
    /// spiflash.rs `write_enable`): screenshot.rs `prep_on` restarts on it.
    pub static FLASH_WRITES: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    pub fn flash_wrote() {
        FLASH_WRITES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }
    /// Test knob for msc.rs `host_mounted()` (a host sent PREVENT MEDIUM REMOVAL).
    pub static HOST_MOUNTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    pub fn host_mounted() -> bool {
        HOST_MOUNTED.load(std::sync::atomic::Ordering::Relaxed)
    }
}

/// The AD2 stream ISR owns the bus (opendm40 hy3131::streaming, the HANDED
/// flag): vdisk.rs `read_busy` refuses the slow-bank sectors then.
mod hy3131 {
    pub static STREAMING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    pub fn streaming() -> bool {
        STREAMING.load(std::sync::atomic::Ordering::Relaxed)
    }
}

/// Stubs of the opendm40 modules calbackup.rs refers to.
mod wdt {
    pub fn kick_if_feeding() {}
}
#[allow(dead_code)]
mod mmio {
    /// The 4 KiB backup SRAM (persist.rs, setfile test); every other
    /// address reads 0 (calbackup's DWT cycle counter).
    pub static mut BKP: [u32; 1024] = [0; 1024];
    const LO: u32 = 0x4002_4000;
    pub unsafe fn r32(a: u32) -> u32 {
        if (LO..LO + 4096).contains(&a) {
            BKP[((a - LO) / 4) as usize]
        } else {
            0
        }
    }
    pub unsafe fn w32(a: u32, v: u32) {
        if (LO..LO + 4096).contains(&a) {
            BKP[((a - LO) / 4) as usize] = v;
        }
    }
    pub unsafe fn modify32(a: u32, clr: u32, set: u32) {
        w32(a, (r32(a) & !clr) | set);
    }
    /// A power-off: the backup SRAM is not powered (device 2026-10-24).
    pub fn bkp_lost() {
        unsafe { BKP = [0; 1024] };
    }
}
#[allow(dead_code)]
mod clock {
    pub const HCLK_HZ: u32 = 200_000_000;
}
#[allow(dead_code)]
mod spiflash {
    pub struct SpiFlash;
    impl SpiFlash {
        pub fn new() -> Self {
            SpiFlash
        }
    }
    impl crate::msc::BlockDevice for SpiFlash {
        fn present(&self) -> bool {
            false
        }
        fn block_size(&self) -> u32 {
            4096
        }
        fn block_count(&self) -> u32 {
            0
        }
        fn read_block(&mut self, _: u32, _: &mut [u8]) -> bool {
            false
        }
        fn write_block(&mut self, _: u32, _: &[u8]) -> bool {
            false
        }
    }
}

/// Stub of the opendm40 LCD: a synthetic 480x320 RGB565 screen (the same
/// pattern run.sh checks the BMP against).
#[allow(dead_code)]
mod lcd {
    pub const W: u16 = 480;
    pub const H: u16 = 320;
    pub type Color = u16;
    /// Rows y % 4 = 0..2: runs of 13, 3 and 2 equal pixels (run / literal
    /// coding of screenshot.rs CAP), else noise (CAP overflows part way, so
    /// the later rows come from the GRAM while writing).
    pub fn pattern(x: u32, y: u32) -> Color {
        match y % 4 {
            0 => ((x / 13) * 0x1234 + y) as u16,
            1 => ((x / 3) * 0x0F0F + y) as u16,
            2 => ((x / 2) * 0x3333 + y) as u16,
            _ => ((x * 0x9E37 + y * 0x79B9) ^ (y << 11)) as u16,
        }
    }
    pub fn read_span(x: u16, y: u16, n: u16, mut out: impl FnMut(Color)) {
        assert!(x as u32 + n as u32 <= W as u32 && y < H, "span outside the screen");
        for i in 0..n as u32 {
            out(pattern(x as u32 + i, y as u32));
        }
    }
}

#[path = "../../src/pack10.rs"]
mod pack10;
#[path = "../../src/ident.rs"]
mod ident;
#[path = "../../src/screenshot.rs"]
#[allow(dead_code)]
mod screenshot;

#[path = "../../src/scope_csv.rs"]
mod scope_csv;

#[path = "../../src/dlog.rs"]
mod dlog;

#[path = "../../src/wordram.rs"]
#[allow(dead_code)]
mod wordram;
#[path = "../../src/csvlog.rs"]
#[allow(dead_code)]
mod csvlog;
#[path = "../../src/fastenc.rs"]
#[allow(dead_code)]
mod fastenc;
#[path = "../../src/fastlog.rs"]
#[allow(dead_code)]
mod fastlog;

#[path = "../../src/fatdir.rs"]
mod fatdir;
#[path = "../../src/upload.rs"]
#[allow(dead_code)]
mod upload;
#[path = "../../src/fatren.rs"]
#[allow(dead_code)]
mod fatren;

#[path = "../../src/calbackup.rs"]
mod calbackup;
#[path = "../../src/diskbuf.rs"]
mod diskbuf;
#[path = "../../src/persist.rs"]
#[allow(dead_code)]
mod persist;
#[path = "../../src/setfile.rs"]
mod setfile;

#[path = "../../src/vdisk.rs"]
mod vdisk;
#[path = "../../src/vwrite.rs"]
mod vwrite;

/// Memory map for vdisk's reader: (address, bytes).
static MEM: std::sync::OnceLock<Vec<(u32, Vec<u8>)>> = std::sync::OnceLock::new();

fn mem_read(addr: u32, buf: &mut [u8]) {
    for (base, d) in MEM.get().unwrap() {
        if addr >= *base && (addr - base) as usize + buf.len() <= d.len() {
            let o = (addr - base) as usize;
            buf.copy_from_slice(&d[o..o + buf.len()]);
            return;
        }
    }
    panic!("vdisk read outside the memory map: {addr:#010x} + {}", buf.len());
}

/// The SPI-disk image behind dlog's reader (vdisk ... spi <img>), replaced
/// between the refresh steps.
static SPI: std::sync::Mutex<Vec<u8>> = std::sync::Mutex::new(Vec::new());

/// Raw SPI reads by dlog's reader (`vread`: calls, bytes).
static SPI_CALLS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static SPI_BYTES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn spi_read(addr: u32, buf: &mut [u8]) -> bool {
    SPI_CALLS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    SPI_BYTES.fetch_add(buf.len() as u64, std::sync::atomic::Ordering::Relaxed);
    let d = SPI.lock().unwrap();
    let o = addr as usize;
    if o + buf.len() > d.len() {
        return false;
    }
    buf.copy_from_slice(&d[o..o + buf.len()]);
    true
}

/// The whole volume through read_block into the sparse file `out` (only
/// non-zero sectors written); every written sector read a second time and
/// compared.
fn vdisk_write(d: &mut vdisk::VDisk, out: &str) {
    use msc::BlockDevice;
    use std::io::{Seek, SeekFrom, Write};
    let n = d.block_count();
    let bs = d.block_size() as usize;
    let mut f = fs::File::create(out).unwrap();
    f.set_len(n as u64 * bs as u64).unwrap();
    let mut b = vec![0u8; bs];
    let mut c = vec![0u8; bs];
    let mut nz = 0u32;
    for lba in 0..n {
        assert!(d.read_block(lba, &mut b), "read {lba}");
        if b.iter().any(|&x| x != 0) {
            assert!(d.read_block(lba, &mut c) && b == c, "sector {lba} differs between two reads");
            f.seek(SeekFrom::Start(lba as u64 * bs as u64)).unwrap();
            f.write_all(&b).unwrap();
            nz += 1;
        }
    }
    println!("  dump {out}: {n} sectors, {nz} non-zero");
}

/// `main <out> vdisk <flash> <uid> <otp> <optbytes> [step]...`: vdisk.rs
/// over the 1 MiB <flash> image (0x08000000..) and the system-memory files;
/// the volume dumped to <out> after init, and to <out>.<i> after step i:
///   spi <img>        the SPI disk is now <img>; refresh
///   locked / free    a host holds / released the medium lock (msc stub);
///                    refresh
///   reset            USB bus reset (vdisk::on_bus_reset); refresh
///   forget <NAME>    vdisk::forget (8.3 SPI name, e.g. LOG001.DLG)
fn vdisk_dump(a: &[String]) {
    use msc::BlockDevice;
    use std::sync::atomic::Ordering::Relaxed;
    let flash = fs::read(&a[3]).unwrap();
    assert_eq!(flash.len(), 1 << 20, "1 MiB flash image");
    let body: &'static [u8] = Vec::leak(flash[0x4000..0x4000 + calbackup::FW_MAX_BODY].to_vec());
    MEM.set(vec![
        (0x0800_0000, flash),
        (0x1FFF_7A10, fs::read(&a[4]).unwrap()),
        (0x1FFF_7800, fs::read(&a[5]).unwrap()),
        (0x1FFF_C000, fs::read(&a[6]).unwrap()),
    ])
    .unwrap();
    dlog::set_disk(spi_read);
    let mut d = vdisk::VDisk;
    println!("present_before_init={}", d.present());
    vdisk::init(mem_read, body);
    println!("media_changed_after_init={}", d.take_media_changed());
    let mut b = vec![0u8; 512];
    let past = d.read_block(d.block_count(), &mut b);
    // Read-only by default (decision 2026-10-22 #6): the write refused;
    // then the writable volume ("PC can edit"): a zeroed boot sector refused.
    let ro = d.read_only();
    let wr_ro = d.write_block(0, &b);
    vwrite::set_read_only(false);
    vwrite::on_bus_reset();
    let wr = d.write_block(0, &b);
    println!("vdisk sectors={} bs={} read_only={} write_ro={} rw={} write_block={} read_past_end={}", d.block_count(), d.block_size(), ro, wr_ro, !d.read_only(), wr, past);
    // read_busy: nothing while no stream runs; with one, exactly the
    // slow-bank ranges (each edge, and a 64-sector READ across the start).
    let ranges: Vec<(u32, u32)> = vdisk::slow_ranges().into_iter().filter(|r| r.1 > r.0).collect();
    let idle = ranges.iter().all(|&(lo, hi)| !d.read_busy(lo, hi - lo)) && !d.read_busy(0, d.block_count());
    hy3131::STREAMING.store(true, Relaxed);
    let inside = |x: u32| ranges.iter().any(|&(lo, hi)| x >= lo && x < hi);
    let edges = ranges.iter().all(|&(lo, hi)| {
        [lo - 1, lo, hi - 1, hi].iter().all(|&x| d.read_busy(x, 1) == inside(x))
            && d.read_busy(lo - 1, 2)
            && d.read_busy(lo - 63, 64)
            && d.read_busy(lo - 64, 64) == (lo - 64..lo).any(inside)
    });
    let s: Vec<String> = ranges.iter().map(|r| format!("{}-{}", r.0, r.1)).collect();
    println!("slow={} idle_none={} streaming_edges={}", s.join(","), idle, edges);
    hy3131::STREAMING.store(false, Relaxed);
    vdisk_write(&mut d, &a[1]);
    let (mut k, mut step) = (7, 0);
    while k < a.len() {
        step += 1;
        match a[k].as_str() {
            "spi" => {
                *SPI.lock().unwrap() = fs::read(&a[k + 1]).unwrap();
                k += 2;
            }
            "locked" | "free" => {
                msc::HOST_MOUNTED.store(a[k] == "locked", Relaxed);
                k += 1;
            }
            "reset" => {
                vdisk::on_bus_reset();
                k += 1;
            }
            "forget" => {
                let n = csvlog::parse_name(a[k + 1].as_bytes()).unwrap();
                vdisk::forget(&n);
                println!("step {step}: forget {} media_changed={}", a[k + 1], d.take_media_changed());
                vdisk_write(&mut d, &format!("{}.{step}", a[1]));
                k += 2;
                continue;
            }
            x => panic!("{x}"),
        }
        let n = vdisk::refresh(None);
        let (_, hw, out) = vdisk::stats();
        println!("step {step}: {} files={n} next_cluster={hw} left_out={out} media_changed={} stale={}", a[k - 1], d.take_media_changed(), dlog::stale());
        let n2 = vdisk::refresh(None);
        println!("  again: files={n2} media_changed={}", d.take_media_changed());
        vdisk_write(&mut d, &format!("{}.{step}", a[1]));
    }
}

/// The SPI disk of `vserve` as a block device (4 KiB blocks) over `SPI`
/// (the same bytes dlog's reader sees): vwrite.rs stages into it,
/// csvlog::delete_on / upload::commit_on write it. `cut`: a power cut
/// before changing write number `cut` (nothing written from then on).
struct SpiDev {
    writes: usize,
    cut: Option<usize>,
    dead: bool,
}

impl msc::BlockDevice for SpiDev {
    fn present(&self) -> bool {
        true
    }
    fn block_size(&self) -> u32 {
        4096
    }
    fn block_count(&self) -> u32 {
        (SPI.lock().unwrap().len() / 4096) as u32
    }
    fn read_block(&mut self, lba: u32, buf: &mut [u8]) -> bool {
        let d = SPI.lock().unwrap();
        let o = lba as usize * 4096;
        buf[..4096].copy_from_slice(&d[o..o + 4096]);
        true
    }
    fn write_block(&mut self, lba: u32, buf: &[u8]) -> bool {
        let mut d = SPI.lock().unwrap();
        let o = lba as usize * 4096;
        if self.dead {
            return false;
        }
        if d[o..o + 4096] != buf[..4096] {
            if self.cut == Some(self.writes) {
                self.dead = true;
                return false;
            }
            self.writes += 1;
        }
        d[o..o + 4096].copy_from_slice(&buf[..4096]);
        true
    }
}

/// `main <spi.img> vserve <flash> <uid> <otp> <optbytes> [cut <k>]`: the USB
/// volume (vdisk.rs + vwrite.rs) over the SPI image, served on stdin /
/// stdout for tools/fatren-test/usbip_msc.py (a USB/IP mass-storage device
/// for a real kernel) and vreplay.py (captured write sequences). Requests:
/// one op byte + a u32 LE argument, replies one status byte unless noted:
///   R lba        -> status, 512 bytes (status 0 ok)
///   W lba + 512  -> 0 accepted, 1 + vwrite::last_error() refused
///   T ms         the main loop at time ms: vwrite::poll, the install /
///                dialog state, vdisk::refresh when dlog is stale
///   L 0|1        PREVENT (1) / ALLOW (0) MEDIUM REMOVAL (vdisk::on_unlock)
///   B            USB bus reset (vdisk::on_bus_reset)
///   U            -> 1 if a UNIT ATTENTION is due (one-shot)
///   P            -> 1 if the volume is write-protected (MODE SENSE WP)
///   M m          ask for the volume mode (Settings "USB disk"): bit 0 =
///                read-only (vwrite::set_read_only), in force at the next B;
///                bit 1 = in force now (the tests' starting mode, no bus
///                reset: the captured layouts stay as they are)
///   D            -> the meter's dialog: 0 none, 1 "install?", 2 "not an
///                update file" (vwrite::take_dialog)
///   A 0|1        answer the dialog (No / Yes)
///   I            -> 1 if Yes was answered (install: zero marker + reset)
///   S            save the SPI image to <spi.img>.out
///   V n          dump the volume (sparse) to <spi.img>.vol<n>
///   Q            -> u16 length + text: DBG:VDISK:WR? and STAT? fields
///   G + 11 bytes a delete on the meter (csvlog::delete_on, 8.3 SPI name)
///   X            exit
fn vserve(a: &[String]) {
    use msc::BlockDevice;
    use std::io::{Read, Write};
    use std::sync::atomic::Ordering::Relaxed;
    let flash = fs::read(&a[3]).unwrap();
    assert_eq!(flash.len(), 1 << 20, "1 MiB flash image");
    let body: &'static [u8] = Vec::leak(flash[0x4000..0x4000 + calbackup::FW_MAX_BODY].to_vec());
    MEM.set(vec![
        (0x0800_0000, flash),
        (0x1FFF_7A10, fs::read(&a[4]).unwrap()),
        (0x1FFF_7800, fs::read(&a[5]).unwrap()),
        (0x1FFF_C000, fs::read(&a[6]).unwrap()),
    ])
    .unwrap();
    *SPI.lock().unwrap() = fs::read(&a[1]).unwrap();
    dlog::set_disk(spi_read);
    let cut = a.iter().position(|x| x == "cut").map(|i| a[i + 1].parse::<usize>().unwrap());
    let dev: &'static mut SpiDev = Box::leak(Box::new(SpiDev { writes: 0, cut, dead: false }));
    let devp: *mut SpiDev = dev;
    vwrite::set_dev(dev);
    vdisk::init(mem_read, body);
    vdisk::boot_refresh();
    let mut d = vdisk::VDisk;
    let mut inp = std::io::stdin().lock();
    let mut out = std::io::stdout().lock();
    let mut hdr = [0u8; 5];
    let mut sec = vec![0u8; 512];
    let (mut gen, mut gen_t, mut now) = (dlog::gen(), 0u64, 0u64);
    while inp.read_exact(&mut hdr).is_ok() {
        let arg = u32::from_le_bytes([hdr[1], hdr[2], hdr[3], hdr[4]]);
        let mut reply: Vec<u8> = vec![0];
        match hdr[0] {
            b'R' => {
                let ok = d.read_block(arg, &mut sec);
                reply[0] = !ok as u8;
                reply.extend_from_slice(&sec);
            }
            b'W' => {
                inp.read_exact(&mut sec).unwrap();
                msc::HOST_WRITING.store(true, Relaxed);
                reply[0] = if d.write_block(arg, &sec) { 0 } else { 1 + d.write_error() };
                msc::HOST_WRITING.store(false, Relaxed);
            }
            b'T' => {
                now = arg as u64;
                vwrite::poll(now, false);
                // main.rs: refresh once dlog::gen() has not moved for 1 s.
                if dlog::gen() != gen {
                    gen = dlog::gen();
                    gen_t = now;
                } else if dlog::stale() && now.saturating_sub(gen_t) >= 1000 {
                    vdisk::refresh(None);
                }
            }
            b'L' => {
                msc::HOST_MOUNTED.store(arg != 0, Relaxed);
                if arg == 0 {
                    vdisk::on_unlock();
                }
            }
            b'B' => {
                msc::HOST_MOUNTED.store(false, Relaxed);
                vdisk::on_bus_reset();
            }
            b'U' => reply[0] = d.take_media_changed() as u8,
            b'P' => reply[0] = d.read_only() as u8,
            b'M' => {
                vwrite::set_read_only(arg & 1 != 0);
                if arg & 2 != 0 {
                    vwrite::on_bus_reset();
                }
            }
            b'D' => reply[0] = match vwrite::take_dialog() {
                None => 0,
                Some(true) => 1,
                Some(false) => 2,
            },
            b'A' => vwrite::answer(arg != 0),
            b'I' => reply[0] = vwrite::take_install() as u8,
            b'S' => save(&format!("{}.out", a[1]), &SPI.lock().unwrap()),
            b'V' => {
                vdisk_write_quiet(&mut d, &format!("{}.vol{arg}", a[1]));
            }
            b'Q' => {
                let n = vwrite::stats();
                let (rec, pairs, st, fz, stg, fw) = vwrite::state();
                let (files, hw, left) = vdisk::stats();
                let cutn = unsafe { (*devp).writes };
                let t = format!(
                    "ok={} refused={} busy={} last_refused={} deleted={} fw_ok={} fw_bad={} rec={rec} pairs={pairs} staged={st} frozen={fz} staging={stg} fw={fw} files={files} hw={hw} left_out={left} spi_writes={cutn} cut_dead={} now={now}",
                    n.ok, n.refused, n.busy, n.last_refused, n.deleted, n.fw_ok, n.fw_bad, unsafe { (*devp).dead }
                );
                reply = (t.len() as u16).to_le_bytes().to_vec();
                reply.extend_from_slice(t.as_bytes());
            }
            b'G' => {
                // A delete on the meter (gallery / MMEM:DEL): 11-byte 8.3 name.
                let mut n = [0u8; 11];
                inp.read_exact(&mut n).unwrap();
                reply[0] = csvlog::delete_on(unsafe { &mut *devp }, &n).is_err() as u8;
            }
            b'Z' => {
                let t = vwrite::debug_text();
                reply = (t.len() as u16).to_le_bytes().to_vec();
                reply.extend_from_slice(t.as_bytes());
            }
            b'X' => break,
            x => panic!("vserve: op {x}"),
        }
        out.write_all(&reply).unwrap();
        out.flush().unwrap();
    }
}

/// `vdisk_write` without the log line (vserve's stdout is the protocol).
fn vdisk_write_quiet(d: &mut vdisk::VDisk, out: &str) {
    use msc::BlockDevice;
    use std::io::{Seek, SeekFrom, Write};
    let n = d.block_count();
    let mut f = fs::File::create(out).unwrap();
    f.set_len(n as u64 * 512).unwrap();
    let mut b = vec![0u8; 512];
    for lba in 0..n {
        assert!(d.read_block(lba, &mut b), "read {lba}");
        if b.iter().any(|&x| x != 0) {
            f.seek(SeekFrom::Start(lba as u64 * 512)).unwrap();
            f.write_all(&b).unwrap();
        }
    }
}

struct Img {
    d: Vec<u8>,
    writes: Vec<u32>,
    reads: Vec<u32>,
    /// NOR model: erases per block (a write that needs any 0 -> 1 bit).
    erases: std::collections::HashMap<u32, u32>,
    programs: u32,
    /// Power cut before changing write number `cut` (0-based); `half`: a
    /// program-only write being cut keeps its first 8 pages.
    cut: Option<usize>,
    half: bool,
    dead: bool,
    cut_kind: &'static str,
    /// NOR model of spiflash.rs `write_step` (screenshot tests): the first
    /// call compares and, if any bit must go 0 -> 1, erases the block (all
    /// 0xFF on the "chip" from then on); each further call programs 4 pages
    /// (old & new); the last one reads back. Off: `write_step` = `write_block`.
    nor_step: bool,
    /// Fast logger (`fast`): every changing write_block, page program and
    /// erase is one operation; power cut before operation `fcut`.
    fops: usize,
    fcut: Option<usize>,
    /// Polls the "chip" stays busy after a program / erase.
    fbusy: u32,
    nor_progs: u32,
    nor_erases: u32,
    /// Blocks that read blank but do not take a program until erased (a
    /// sector whose erase a power cut interrupted): `write_step` leaves them
    /// 0xFF, so its read back fails.
    weak: Vec<u32>,
    /// `write_step` calls that started at phase 1 (no compare).
    blind: u32,
}

impl Img {
    fn new(d: Vec<u8>) -> Self {
        Img {
            d,
            writes: vec![],
            reads: vec![],
            erases: Default::default(),
            programs: 0,
            cut: None,
            half: false,
            dead: false,
            cut_kind: "none",
            nor_step: false,
            fops: 0,
            fcut: None,
            fbusy: 0,
            nor_progs: 0,
            nor_erases: 0,
            weak: vec![],
            blind: 0,
        }
    }
    /// One fast-logger operation: false = the power is gone.
    fn fop(&mut self) -> bool {
        if self.dead {
            return false;
        }
        if self.fcut == Some(self.fops) {
            self.dead = true;
            return false;
        }
        self.fops += 1;
        true
    }
}

impl fastenc::Nor for Img {
    fn busy(&mut self) -> bool {
        if self.fbusy > 0 {
            self.fbusy -= 1;
            return true;
        }
        false
    }
    fn program(&mut self, addr: u32, d: &[u8]) {
        assert_eq!(self.fbusy, 0, "program while busy");
        assert_eq!(addr / 256, (addr + d.len() as u32 - 1) / 256, "program crosses a page");
        if !self.fop() {
            return;
        }
        msc::flash_wrote();
        for (i, &b) in d.iter().enumerate() {
            self.d[addr as usize + i] &= b;
        }
        self.nor_progs += 1;
        self.fbusy = 1;
    }
    fn erase(&mut self, addr: u32, big: bool) {
        if !self.fop() {
            return;
        }
        msc::flash_wrote();
        let n = if big { 0x1_0000 } else { 0x1000 };
        let a = addr as usize & !(n - 1);
        self.d[a..a + n].fill(0xFF);
        self.weak.retain(|&l| (l as usize * 4096) < a || l as usize * 4096 >= a + n);
        self.nor_erases += 1;
        self.fbusy = 3;
    }
    fn read(&mut self, addr: u32, b: &mut [u8]) -> bool {
        b.copy_from_slice(&self.d[addr as usize..addr as usize + b.len()]);
        true
    }
}

impl msc::BlockDevice for Img {
    fn present(&self) -> bool {
        true
    }
    fn block_size(&self) -> u32 {
        4096
    }
    fn block_count(&self) -> u32 {
        (self.d.len() / 4096) as u32
    }
    fn read_block(&mut self, lba: u32, buf: &mut [u8]) -> bool {
        self.reads.push(lba);
        let o = lba as usize * 4096;
        buf[..4096].copy_from_slice(&self.d[o..o + 4096]);
        true
    }
    fn write_block(&mut self, lba: u32, buf: &[u8]) -> bool {
        let o = lba as usize * 4096;
        if self.dead {
            return false;
        }
        if self.d[o..o + 4096] != buf[..4096] {
            if self.fcut.is_some() && !self.fop() {
                return false;
            }
            msc::flash_wrote();
            let erase = self.d[o..o + 4096].iter().zip(&buf[..4096]).any(|(&a, &b)| a & b != b);
            if self.cut == Some(self.writes.len()) {
                self.dead = true;
                self.cut_kind = if erase { "erase-write (not started)" } else { "program-only" };
                if self.half && !erase {
                    self.d[o..o + 2048].copy_from_slice(&buf[..2048]);
                    self.cut_kind = "program-only, half applied";
                }
                return false;
            }
            self.writes.push(lba);
            if erase {
                *self.erases.entry(lba).or_insert(0) += 1;
            } else {
                self.programs += 1;
            }
        }
        self.d[o..o + 4096].copy_from_slice(&buf[..4096]);
        true
    }
    fn write_step(&mut self, lba: u32, buf: &dyn wordram::BlockSrc, st: &mut msc::Step) -> Option<bool> {
        let v = msc::src_bytes(buf);
        let buf = &v[..];
        if !self.nor_step {
            return Some(self.write_block(lba, buf));
        }
        let o = lba as usize * 4096;
        match st.phase {
            0 => {
                st.started = true;
                if self.d[o..o + 4096] == buf[..4096] {
                    return Some(true);
                }
                // `|=`: a forced erase (screenshot.rs `data`, the retry) stays.
                st.erase |= self.d[o..o + 4096].iter().zip(&buf[..4096]).any(|(&a, &b)| a & b != b);
                msc::flash_wrote();
                if st.erase {
                    self.weak.retain(|&l| l != lba);
                    self.d[o..o + 4096].fill(0xFF);
                    *self.erases.entry(lba).or_insert(0) += 1;
                } else {
                    self.programs += 1;
                }
                self.writes.push(lba);
                st.phase = 2;
                st.page = 0;
                None
            }
            1 => {
                // Programs without the compare (screenshot.rs: a cluster
                // the preparation found blank).
                msc::flash_wrote();
                self.programs += 1;
                self.writes.push(lba);
                self.blind += 1;
                st.started = true;
                st.phase = 2;
                st.page = 0;
                None
            }
            2 => {
                if self.weak.contains(&lba) {
                    st.page += 4; // the program does not take
                    if st.page == 16 {
                        st.phase = 3;
                    }
                    return None;
                }
                for p in st.page as usize..st.page as usize + 4 {
                    for i in o + p * 256..o + p * 256 + 256 {
                        self.d[i] &= buf[i - o];
                    }
                }
                st.page += 4;
                if st.page == 16 {
                    st.phase = 3;
                }
                None
            }
            _ => Some(self.d[o..o + 4096] == buf[..4096]),
        }
    }
}

/// Screenshot power-cut test (review 60605d2 H1 / L4): the shot driven slice
/// by slice (`poll_on`) on the NOR model, stopped after `k` slices (power
/// cut: the image as it is then, incl. a half-written data block), or with
/// `abort` stopped there by `screenshot::abort_on` (the power-off / reset
/// path). Prints the slice count of an uncut shot, the phase sequence, and
/// the FAT / root pages that are all 0xFF (an erased or half-programmed
/// metadata block; a real FAT / root page is never all 0xFF here).
fn shotcut(a: &[String], img: &mut Img) {
    img.nor_step = true;
    let k: Option<usize> = a.get(3).map(|x| x.parse().unwrap());
    let abort = a.get(4).map(|x| x == "abort").unwrap_or(false);
    assert!(screenshot::start());
    let mut n = 0usize;
    let mut res = None;
    let mut fat_ops = vec![];
    loop {
        if Some(n) == k {
            if abort {
                screenshot::abort_on(img);
                res = screenshot::take_result();
            }
            break;
        }
        let w0 = img.writes.len();
        let r = screenshot::poll_on(img);
        n += 1;
        if img.writes.len() > w0 {
            fat_ops.push((n, img.writes[w0]));
        }
        if let Some(r) = r {
            screenshot::take_result();
            res = Some(r);
            break;
        }
    }
    let b = &img.d;
    let le16 = |o: usize| b[o] as u32 | (b[o + 1] as u32) << 8;
    let fat_start = le16(0x0E) as usize;
    let data = fat_start + b[0x10] as usize * le16(0x16) as usize + le16(0x11) as usize * 32 / 4096;
    let dirs = dir_blocks(b);
    let pages: Vec<usize> = (fat_start * 16..data * 16).chain(dirs.iter().flat_map(|&l| l as usize * 16..l as usize * 16 + 16)).collect();
    let erased: Vec<usize> = pages.into_iter().filter(|&p| b[p * 256..p * 256 + 256].iter().all(|&x| x == 0xFF)).collect();
    let meta: Vec<String> = fat_ops.iter().filter(|&&(_, l)| (l as usize) < data || dirs.contains(&l)).map(|(n, l)| format!("{n}:{l}")).collect();
    println!("shotcut slices={} result={:?} busy={} erased_meta_pages={:?}", n, res, screenshot::busy(), erased);
    println!("  metadata writes (slice:lba) {:?}", meta);
}

/// Volume geometry of an image for the prep checks: (FAT 1 byte offset,
/// data start LBA, clusters).
fn geo(b: &[u8]) -> (usize, u32, u32) {
    let le16 = |o: usize| b[o] as u32 | (b[o + 1] as u32) << 8;
    let rsvd = le16(0x0E);
    let data = rsvd + b[0x10] as u32 * le16(0x16) + le16(0x11) * 32 / 4096;
    let tot = if le16(0x13) != 0 { le16(0x13) } else { le16(0x20) | le16(0x22) << 16 };
    (rsvd as usize * 4096, data, tot - data)
}

/// Free clusters of an image, first fit.
fn free_clusters(b: &[u8]) -> Vec<u32> {
    let (fat, _, n) = geo(b);
    (2..n + 2).filter(|&c| b[fat + 2 * c as usize] == 0 && b[fat + 2 * c as usize + 1] == 0).collect()
}

fn cluster_blank(b: &[u8], c: u32) -> bool {
    let (_, data, _) = geo(b);
    let o = (data + c - 2) as usize * 4096;
    b[o..o + 4096].iter().all(|&x| x == 0xFF)
}

/// Drive screenshot.rs's idle-time preparation (`prep_on`, 5 ms per main-
/// loop pass, everything allowed) until it is Ready / Off, the power is
/// gone, or `max` steps. Returns (state, steps).
fn prep_drive(img: &mut Img, now: &mut u64, max: usize) -> (u32, usize) {
    let mut n = 0;
    loop {
        *now += 5;
        let st = screenshot::prep_on(img, *now, true);
        n += 1;
        if st >= 2 || img.dead || n >= max {
            return (st, n);
        }
    }
}

/// `main <img> prep run [cut <k>] | shot | race [nobump] | weak`: the idle-
/// time preparation of the next screenshot's clusters (screenshot.rs
/// `prep_on`) on the NOR model.
///   run: to Ready; FAT / root / every allocated cluster unchanged, only
///        free clusters written, the first 76 free ones (all if fewer)
///        blank; `cut k` = power cut before the k-th program / erase;
///   shot [stale]: prepared, then a shot slice by slice: no data block
///        erased in the shot, every one programmed without the compare;
///        then prepared again (the next 76); `stale`: the 4th prepared
///        cluster overwritten behind the write counter before the shot;
///   race: for every step k of a preparation, another writer links the
///        first free cluster (FAT 1 + 2 entry EOC, data 0xA5) after k steps
///        (`nobump`: behind the write counter's back, i.e. only the FAT
///        re-read before an erase protects it); its data must survive;
///   weak: the first free cluster reads blank but does not take a program
///        (an erase cut short): the shot's read back fails once, the block
///        is written again with an erase, Saved.
fn prep_cmd(a: &[String], img: &mut Img) {
    let mut now = 0u64;
    let blk_of = |b: &[u8], c: u32| (geo(b).1 + c - 2) as usize * 4096;
    match a[3].as_str() {
        "run" => {
            if a.get(4).is_some_and(|x| x == "cut") {
                img.fcut = Some(a[5].parse().unwrap());
            }
            let before = img.d.clone();
            let free = free_clusters(&before);
            let (fat, data, _) = geo(&before);
            let _ = fat;
            let (st, steps) = prep_drive(img, &mut now, 1_000_000);
            let meta_same = img.d[..data as usize * 4096] == before[..data as usize * 4096];
            let mut used_same = true;
            let mut changed_free = 0;
            for blk in data as usize..img.d.len() / 4096 {
                let c = blk as u32 - data + 2;
                let o = blk * 4096;
                if img.d[o..o + 4096] != before[o..o + 4096] {
                    if free.contains(&c) {
                        changed_free += 1;
                    } else {
                        used_same = false;
                    }
                }
            }
            let want = free.len().min(76);
            let blank = free[..want].iter().filter(|&&c| cluster_blank(&img.d, c)).count();
            let p = screenshot::prep_stats();
            println!(
                "prep state={} found={} erases={} steps={} meta_same={} used_same={} changed_free={} blank_first={}/{} free={} dead={}",
                st, p[1], p[2], steps, meta_same, used_same, changed_free, blank, want, free.len(), img.dead
            );
        }
        "shot" => {
            let (st, _) = prep_drive(img, &mut now, 1_000_000);
            let e0 = screenshot::prep_stats()[2];
            if a.get(4).is_some_and(|x| x == "stale") {
                // A prepared cluster written behind the write counter's
                // back: the shot programs it without the compare, the read
                // back fails, the retry erases it.
                let c = free_clusters(&img.d)[3];
                let o = blk_of(&img.d, c);
                img.d[o..o + 4096].fill(0);
            }
            img.nor_step = true;
            assert!(screenshot::start());
            let r = loop {
                if let Some(r) = screenshot::poll_on(img) {
                    break r;
                }
            };
            screenshot::take_result();
            let s = screenshot::stats();
            let free = free_clusters(&img.d);
            let ready_before = free[..free.len().min(76)].iter().filter(|&&c| cluster_blank(&img.d, c)).count();
            let (st2, _) = prep_drive(img, &mut now, 1_000_000);
            let ready_after = free[..free.len().min(76)].iter().filter(|&&c| cluster_blank(&img.d, c)).count();
            let p = screenshot::prep_stats();
            println!(
                "prep state={} erases={} shot={:?} shot_erased={} no_compare={} next_blank_before={} state2={} found2={} erases2={} next_blank_after={}",
                st, e0, r, s[3], img.blind, ready_before, st2, p[1], p[2] - e0, ready_after
            );
        }
        "race" => {
            let nobump = a.get(4).is_some_and(|x| x == "nobump");
            let base = img.d.clone();
            screenshot::prep_reset();
            let mut probe = Img::new(base.clone());
            let mut t = 0u64;
            let (_, total) = prep_drive(&mut probe, &mut t, 1_000_000);
            let (mut ok, mut erased_any) = (0, 0);
            // Every step of the first 3000 (the scan, the first clusters'
            // checks and erases), then every 7th.
            let ks: Vec<usize> = (0..total.min(3000)).chain((3000..total).step_by(7)).collect();
            for &k in &ks {
                screenshot::prep_reset();
                let mut m = Img::new(base.clone());
                let mut t = 0u64;
                prep_drive(&mut m, &mut t, k);
                // The other writer: the first free cluster now.
                let c = free_clusters(&m.d)[0];
                let (fat, _, _) = geo(&m.d);
                let o = blk_of(&m.d, c);
                let nf = m.d[0x10] as usize;
                let fsz = (m.d[0x16] as usize | (m.d[0x17] as usize) << 8) * 4096;
                if nobump {
                    m.d[o..o + 4096].fill(0xA5);
                    for f in 0..nf {
                        m.d[fat + f * fsz + 2 * c as usize..fat + f * fsz + 2 * c as usize + 2].copy_from_slice(&[0xFF, 0xFF]);
                    }
                } else {
                    let mut b = vec![0xA5u8; 4096];
                    msc::BlockDevice::write_block(&mut m, o as u32 / 4096, &b);
                    for f in 0..nf {
                        let lba = ((fat + f * fsz + 2 * c as usize) / 4096) as u32;
                        msc::BlockDevice::read_block(&mut m, lba, &mut b);
                        let i = (fat + f * fsz + 2 * c as usize) % 4096;
                        b[i..i + 2].copy_from_slice(&[0xFF, 0xFF]);
                        msc::BlockDevice::write_block(&mut m, lba, &b);
                    }
                }
                let (st, _) = prep_drive(&mut m, &mut t, 1_000_000);
                if m.d[o..o + 4096].iter().all(|&x| x == 0xA5) && st == 2 {
                    ok += 1;
                }
                erased_any += screenshot::prep_stats()[2].min(1) as usize;
            }
            println!("prep race{} steps={} cases={} intact={} (with erases {})", if nobump { " nobump" } else { "" }, total, ks.len(), ok, erased_any);
        }
        "weak" => {
            let c = free_clusters(&img.d)[0];
            let o = blk_of(&img.d, c);
            img.d[o..o + 4096].fill(0xFF);
            img.weak.push(o as u32 / 4096);
            img.nor_step = true;
            assert!(screenshot::start());
            let r = loop {
                if let Some(r) = screenshot::poll_on(img) {
                    break r;
                }
            };
            screenshot::take_result();
            println!("prep weak shot={:?} shot_erased={} weak_left={}", r, screenshot::stats()[3], img.weak.len());
        }
        x => panic!("prep {x}?"),
    }
}

fn unhex(s: &str) -> Vec<u8> {
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
}

/// `main <out> vread <flash> <uid> <otp> <optbytes> <spi.img> <DIR> <NAME.EXT> [head]`:
/// the USB volume over <spi.img>; file DIR/NAME.EXT read the way a host
/// does (its clusters in order, sector by sector through read_block) into
/// <out>, with the raw SPI reads behind it (calls, bytes) and a device
/// estimate: the W25Q128 at 12.5 MHz (0.64 us per byte, ~6 us per read
/// for RDSR + command + address + CS), CPU time not included.
fn vread(a: &[String]) {
    use msc::BlockDevice;
    use std::sync::atomic::Ordering::Relaxed;
    let flash = fs::read(&a[3]).unwrap();
    let body: &'static [u8] = Vec::leak(flash[0x4000..0x4000 + calbackup::FW_MAX_BODY].to_vec());
    MEM.set(vec![
        (0x0800_0000, flash),
        (0x1FFF_7A10, fs::read(&a[4]).unwrap()),
        (0x1FFF_7800, fs::read(&a[5]).unwrap()),
        (0x1FFF_C000, fs::read(&a[6]).unwrap()),
    ])
    .unwrap();
    dlog::set_disk(spi_read);
    *SPI.lock().unwrap() = fs::read(&a[7]).unwrap();
    let mut d = vdisk::VDisk;
    vdisk::init(mem_read, body);
    let t0 = std::time::Instant::now();
    vdisk::refresh(None);
    let t_refresh = t0.elapsed();
    let (rc, rb) = (SPI_CALLS.swap(0, Relaxed), SPI_BYTES.swap(0, Relaxed));
    let mut b = vec![0u8; 512];
    assert!(d.read_block(0, &mut b));
    let le16 = |b: &[u8], o: usize| u16::from_le_bytes([b[o], b[o + 1]]) as u32;
    let le32 = |b: &[u8], o: usize| u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]);
    let (bps, spc, rsvd, nfat, nroot, fatsz) = (le16(&b, 11), b[13] as u32, le16(&b, 14), b[16] as u32, le16(&b, 17), le16(&b, 22));
    assert_eq!(bps, 512);
    let root = rsvd + nfat * fatsz;
    let data = root + nroot * 32 / 512;
    let name = |s: &str| {
        let (n, e) = s.split_once('.').unwrap_or((s, ""));
        let mut x = [b' '; 11];
        x[..n.len()].copy_from_slice(n.as_bytes());
        x[8..8 + e.len()].copy_from_slice(e.as_bytes());
        x
    };
    let fat = |d: &mut vdisk::VDisk, c: u32| {
        let mut f = vec![0u8; 512];
        assert!(d.read_block(rsvd + c * 2 / 512, &mut f));
        le16(&f, (c * 2 % 512) as usize)
    };
    // The entry `n` in the directory at `start` (None: the root).
    let find = |d: &mut vdisk::VDisk, start: Option<u32>, n: [u8; 11]| -> Option<(u32, u32)> {
        let secs: Vec<u32> = match start {
            None => (root..data).collect(),
            Some(mut c) => {
                let mut v = vec![];
                while (2..0xFFF0).contains(&c) {
                    v.extend((0..spc).map(|k| data + (c - 2) * spc + k));
                    c = fat(d, c);
                }
                v
            }
        };
        let mut e = vec![0u8; 512];
        for s in secs {
            assert!(d.read_block(s, &mut e));
            for k in 0..16 {
                let x = &e[k * 32..k * 32 + 32];
                if x[0] == 0 {
                    return None;
                }
                if x[..11] == n {
                    return Some((le16(x, 26), le32(x, 28)));
                }
            }
        }
        None
    };
    let (dc, _) = find(&mut d, None, name(&a[8])).expect("directory");
    let (mut c, size) = find(&mut d, Some(dc), name(&a[9])).expect("file");
    let t1 = std::time::Instant::now();
    let mut out = Vec::with_capacity(size as usize);
    let mut sectors = 0u32;
    // `head` (an extra argument): every 16th sector also re-read the file's
    // first sector (an indexer at its head while a copy runs), compared.
    let head = a.get(10).is_some_and(|x| x == "head");
    let first = c;
    let mut hb = vec![0u8; 512];
    while (out.len() as u32) < size && (2..0xFFF0).contains(&c) {
        for k in 0..spc {
            if out.len() as u32 >= size {
                break;
            }
            assert!(d.read_block(data + (c - 2) * spc + k, &mut b));
            sectors += 1;
            let n = (size as usize - out.len()).min(512);
            out.extend_from_slice(&b[..n]);
            if head && sectors % 16 == 0 {
                assert!(d.read_block(data + (first - 2) * spc, &mut hb));
                assert!(hb[..512.min(size as usize)] == out[..512.min(size as usize)], "head re-read differs");
            }
        }
        c = fat(&mut d, c);
    }
    let t_read = t1.elapsed();
    assert_eq!(out.len() as u32, size, "the cluster chain ends early");
    fs::write(&a[1], &out).unwrap();
    let (c2, b2) = (SPI_CALLS.load(Relaxed), SPI_BYTES.load(Relaxed));
    let est = |calls: u64, bytes: u64| (calls as f64 * 6.0 + bytes as f64 * 0.64) / 1e6;
    let spi_s = est(c2, b2);
    println!(
        "vread {}/{}: {} bytes, {} sectors; refresh: spi {} reads {} B (~{:.2} s on the device), host {:.3} s; read: spi {} reads {} B ({:.3} B per CSV byte, ~{:.2} s on the device = {:.0} KB/s SPI-bound), host {:.3} s",
        a[8], a[9], size, sectors, rc, rb, est(rc, rb), t_refresh.as_secs_f64(), c2, b2, b2 as f64 / size.max(1) as f64, spi_s, size as f64 / 1024.0 / spi_s.max(1e-9), t_read.as_secs_f64()
    );
}

/// `t names`: csvlog::parse_path, the MMEM name argument (device 2f56ff9:
/// "SHOTS/LOG005.DLG" found LOGS/LOG005.DLG). A prefix must name the file's
/// own directory; none = any (its directory, then the root).
fn names() {
    let mut fails = 0;
    for (arg, want) in [
        ("LOG005.DLG", Some(("LOG005  DLG", true))),
        ("\"LOGS/LOG005.DLG\"", Some(("LOG005  DLG", true))),
        ("logs\\log005.dlg", Some(("LOG005  DLG", true))),
        ("/LOGS/LOG005.CSV", Some(("LOG005  CSV", true))),
        ("LOGS/FAST001.DLG", Some(("FAST001 DLG", true))),
        ("LOGS/FAST001.CSV", Some(("FAST001 CSV", true))),
        ("SHOTS/S012.BMP", Some(("S012    BMP", true))),
        ("SCOPE/SCOP000.DLG", Some(("SCOP000 DLG", true))),
        ("SCOPE/SCOP000.CSV", Some(("SCOP000 CSV", true))),
        ("SHOTS/LOG005.DLG", Some(("LOG005  DLG", false))),
        ("SCOPE/LOG005.CSV", Some(("LOG005  CSV", false))),
        ("LOGS/S012.BMP", Some(("S012    BMP", false))),
        ("LOGS/SCOP000.DLG", Some(("SCOP000 DLG", false))),
        ("SHOTS/FAST001.DLG", Some(("FAST001 DLG", false))),
        ("LOGS/README.TXT", Some(("README  TXT", false))),
        ("README.TXT", Some(("README  TXT", true))),
        ("INFO/UID.BIN", None),
        ("LOGS/", None),
        ("TOOLONGNAME.DLG", None),
    ] {
        let got = csvlog::parse_path(arg.as_bytes());
        let got = got.map(|(n, ok)| (String::from_utf8_lossy(&n).into_owned(), ok));
        let want = want.map(|(n, ok)| (n.to_string(), ok));
        if got == want {
            println!("ok   names {arg} -> {got:?}");
        } else {
            println!("FAIL names {arg} -> {got:?}, want {want:?}");
            fails += 1;
        }
    }
    if fails > 0 {
        std::process::exit(1);
    }
    println!("names PASS");
}

fn main() {
    let a: Vec<String> = env::args().collect();
    if a.get(1).is_some_and(|x| x == "names") {
        names();
        return;
    }
    if a[2] == "vread" {
        vread(&a);
        return;
    }
    if a[2] == "vdisk" {
        vdisk_dump(&a);
        return;
    }
    if a[2] == "vserve" {
        vserve(&a);
        return;
    }
    let mut img = Img::new(fs::read(&a[1]).unwrap());
    if a[2] == "csv" {
        csv(&a, &mut img);
        save(&a[1], &img.d);
        return;
    }
    if a[2] == "fast" {
        fast(&a, &mut img);
        save(&a[1], &img.d);
        return;
    }
    if a[2] == "upload" {
        upload_cmd(&a, &mut img);
        save(&a[1], &img.d);
        return;
    }
    if a[2] == "prep" {
        prep_cmd(&a, &mut img);
        save(&a[1], &img.d);
        return;
    }
    if a[2] == "setfile" {
        setfile_cmd(&a, img);
        return;
    }
    if a[2] == "fatdir" {
        fatdir_cmd(&a, &mut img);
        save(&a[1], &img.d);
        return;
    }
    if a[2] == "trigger" {
        let t = fatren::make_trigger(&mut img);
        println!("trigger={:?}\n  changed blocks: {:?}", t, img.writes);
        save(&a[1], &img.d);
        return;
    }
    if a[2] == "backup" {
        let data: Vec<Vec<u8>> = calbackup::FILES[..calbackup::SAFE]
            .iter()
            .map(|(n, _, _)| {
                let name = format!(
                    "{}.{}",
                    String::from_utf8_lossy(&n[..8]).trim_end(),
                    String::from_utf8_lossy(&n[8..]).trim_end()
                );
                fs::read(format!("{}/{}", a[3], name)).unwrap()
            })
            .collect();
        let slices: Vec<&[u8]> = data.iter().map(|d| &d[..]).collect();
        let srcs: Vec<calbackup::Src> = calbackup::FILES
            .iter()
            .zip(&slices)
            .map(|(&(name, addr, _), d)| calbackup::Src { name, addr, data: d })
            .collect();
        let r = calbackup::safe_copy(&mut img, &srcs);
        println!(
            "backup={:?} written=0x{:03x} conflict=0x{:03x} differs=0x{:03x}",
            r.status, r.written, r.conflict, r.differs
        );
        // Geometry from the boot sector: blocks >= data_start are file data.
        let b = &img.d;
        let le16 = |o: usize| b[o] as u32 | (b[o + 1] as u32) << 8;
        let data_start = le16(0x0E) + b[0x10] as u32 * le16(0x16) + le16(0x11) * 32 / 4096;
        let data_reads: Vec<u32> = img.reads.iter().copied().filter(|&l| l >= data_start).collect();
        println!("  changed blocks: {:?}", img.writes);
        println!("  reads={} data_reads={:?}", img.reads.len(), data_reads);
        save(&a[1], &img.d);
        return;
    }
    if a[2] == "shotcut" {
        shotcut(&a, &mut img);
        save(&a[1], &img.d);
        return;
    }
    if a[2] == "scopecsv" {
        // A synthetic scope record (sine, 10-bit codes) of n samples as
        // SCOPnnn.CSV, driven slice by slice; a screenshot request while it
        // runs is refused. "amps" after n: a current record (the Wave view's
        // A function): its column is "amperes".
        let n: usize = a.get(3).map_or(26 * 1024, |v| v.parse().unwrap());
        let amps = a.get(4).is_some_and(|v| v == "amps");
        // Packed as the scope keeps it (pack10.rs; + 2 words of read slack).
        let raw: &'static mut [u16] = Box::leak(vec![0u16; n + 4].into_boxed_slice());
        for (i, c) in raw[..n].iter_mut().enumerate() {
            *c = (512.0 + 400.0 * (i as f32 * 0.01).sin()) as u16;
        }
        let p = raw.as_mut_ptr() as *mut u8;
        pack10::pack_in_place(p, (n + 3) & !3);
        let rec = pack10::Codes::new(p, 0, n, n);
        let mut h = scope_csv::Hdr::new();
        h.s("# test record\r\n");
        scope_csv::set(rec, -5.0, 10.0 / 1023.0, 1.0e6, n / 2, &h, amps);
        assert!(screenshot::start_csv());
        let refused = !screenshot::start();
        let mut slices = 0;
        let r = loop {
            screenshot::poll_on(&mut img);
            slices += 1;
            if let Some(r) = screenshot::take_csv_result() {
                break r;
            }
        };
        println!("csv={:?} len={} slices={} shot_refused={} shot_result={:?}", r, scope_csv::len(), slices, refused, screenshot::take_result());
        save(&a[1], &img.d);
        return;
    }
    if a[2] == "shot" {
        let r = screenshot::take_on(&mut img);
        let mc = screenshot::MEDIUM_CHANGED.load(std::sync::atomic::Ordering::Relaxed);
        println!("shot={:?} medium_changed={}", r, mc);
        println!("  changed blocks: {:?}", img.writes);
        save(&a[1], &img.d);
        return;
    }
    let dry = a[2] == "dry";
    let marker = a.get(3).map(|h| {
        let mut m = [0u8; 12];
        m.copy_from_slice(&unhex(h));
        m
    });
    let r = fatren::run(&mut img, dry, marker);
    println!(
        "status={:?} found={} renamed='{}' old_backup={} old_deleted={} freed_clusters={} others={} staged={} fake_deleted={}",
        r.status,
        r.found,
        String::from_utf8_lossy(&r.renamed),
        r.old_backup,
        r.old_deleted,
        r.freed_clusters,
        r.others,
        r.staged,
        r.fake_deleted
    );
    println!("  changed blocks: {:?}", img.writes);
    if !dry {
        save(&a[1], &img.d);
    }
}

/// `main <img> fatdir boot|ensure <NAME>|rm <NAME> [cut <k>]`: fatdir.rs on
/// the image (boot = the native boot step: LOGS, SHOTS, SCOPE ensured);
/// cut = power cut before the k-th changing block write.
fn fatdir_cmd(a: &[String], img: &mut Img) {
    let mut k = 4;
    let name = a.get(4).filter(|_| a[3] != "boot").map(|n| {
        k = 5;
        let mut b = [b' '; 11];
        b[..n.len()].copy_from_slice(n.as_bytes());
        b
    });
    if a.get(k).is_some_and(|x| x == "cut") {
        img.cut = Some(a[k + 1].parse().unwrap());
    }
    let s = diskbuf::blocks();
    let mut v = || {
        let mut b = [0u8; 4096];
        msc::BlockDevice::read_block(img, 0, &mut b);
        fatren::parse_bpb(&b, (img.d.len() / 4096) as u32).unwrap()
    };
    match a[3].as_str() {
        "boot" => {
            let r = fatdir::boot(img);
            println!("fatdir boot ok={:#x} created={:#x} failed={:#x}", r.ok, r.created, r.failed);
        }
        "ensure" => {
            let v = v();
            match fatdir::ensure(img, &v, &name.unwrap(), s) {
                Ok(d) => println!("fatdir ensure ok clusters={:?}", &d.dcl[..d.dn as usize]),
                Err(e) => println!("fatdir ensure err={e:?}"),
            }
        }
        "rm" => {
            let v = v();
            println!("fatdir rm {:?}", fatdir::remove_empty(img, &v, &name.unwrap(), s));
        }
        x => panic!("{x}"),
    }
    println!("  writes={} changed blocks: {:?} cut_kind={}", img.writes.len(), img.writes, img.cut_kind);
}

/// `main <img> upload <file> [crc <hex>] [order [keep]] [chunk <n>] [cut <k>]`:
/// upload.rs as main.rs drives it (BEGIN, DATA messages of `chunk` bytes,
/// END?); crc = announce this CRC instead of the file's; order = send the
/// second chunk twice; cut = power cut before the k-th changing write.
fn upload_cmd(a: &[String], img: &mut Img) {
    let f = fs::read(&a[3]).unwrap();
    let mut crc = calbackup::crc32(&f);
    let (mut order, mut chunk, mut keep) = (false, 240usize, false);
    let mut k = 4;
    while k < a.len() {
        match a[k].as_str() {
            "crc" => {
                crc = u32::from_str_radix(&a[k + 1], 16).unwrap();
                k += 2;
            }
            "order" => {
                order = true;
                k += 1;
            }
            // keep: go on after a DATA error (as the host tool streams) and
            // let END? report it.
            "keep" => {
                keep = true;
                k += 1;
            }
            "chunk" => {
                chunk = a[k + 1].parse().unwrap();
                k += 2;
            }
            "cut" => {
                img.cut = Some(a[k + 1].parse().unwrap());
                k += 2;
            }
            x => panic!("{x}"),
        }
    }
    let r = (|| -> Result<(u32, u32), &'static str> {
        upload::begin_on(img, f.len() as u32, crc, 0)?;
        let mut off = 0usize;
        let mut i = 0;
        while off < f.len() {
            let n = chunk.min(f.len() - off);
            let mut m = format!("{off},").into_bytes();
            m.extend_from_slice(&f[off..off + n]);
            let r = upload::data_msg_on(img, &m, i);
            if !keep {
                r?;
            }
            if order && i == 1 {
                let r = upload::data_msg_on(img, &m, i);
                if !keep {
                    r?;
                }
            }
            off += n;
            i += 1;
        }
        upload::end_on(img)
    })();
    println!("upload {:?} busy={} writes={} changed blocks: {:?}", r, upload::busy(), img.writes.len(), img.writes.len());
}

/// Blocks of the root's subdirectories (their clusters; 1 cluster = 1
/// block on this disk): directory metadata like the root region.
fn dir_blocks(b: &[u8]) -> Vec<u32> {
    let le16 = |o: usize| b[o] as u32 | (b[o + 1] as u32) << 8;
    let fat = le16(0x0E) as usize * 4096;
    let root = (le16(0x0E) + b[0x10] as u32 * le16(0x16)) as usize * 4096;
    let ne = le16(0x11) as usize;
    let data = (root / 4096 + ne * 32 / 4096) as u32;
    let mut out = vec![];
    for i in 0..ne {
        let e = &b[root + 32 * i..root + 32 * i + 32];
        if e[0] == 0 {
            break;
        }
        if e[0] == 0xE5 || e[11] & 0x3F == 0x0F || e[11] & 0x10 == 0 {
            continue;
        }
        let mut c = e[26] as u32 | (e[27] as u32) << 8;
        let mut n = 0;
        while (2..0xFFF0).contains(&c) && n < 64 {
            out.push(data + c - 2);
            c = le16(fat + 2 * c as usize);
            n += 1;
        }
    }
    out
}

/// Synthetic reading i: value over 1e-9 .. 1e4 magnitudes, flags pattern (run.sh
/// csvok() recomputes both).
fn row(i: u32) -> (f32, u8) {
    const P: [f32; 13] = [1e-9, 1e-8, 1e-7, 1e-6, 1e-5, 1e-4, 1e-3, 1e-2, 1e-1, 1e0, 1e1, 1e2, 1e3];
    let m = ((i * 7919) % 20001) as f32 - 10000.0;
    let mut f = 0u8;
    if i % 97 == 3 {
        f |= csvlog::F_OL;
    }
    if i % 5 == 0 {
        f |= csvlog::F_REL;
    }
    if i % 7 == 0 {
        f |= csvlog::F_HOLD;
    }
    if i % 11 == 0 {
        f |= csvlog::F_HI;
    }
    if i % 13 == 1 {
        f |= csvlog::F_LO;
    }
    (m * P[(i % 13) as usize], f)
}

/// Uptime of row i (ms): the recording starts at T0, a row every 100 ms.
const T0: u64 = 5000;
const POLLS_PER_ROW: u32 = 4;

fn csv(a: &[String], img: &mut Img) {
    use std::sync::atomic::Ordering::Relaxed;
    let mc = &screenshot::MEDIUM_CHANGED;
    match a[3].as_str() {
        "cat" => {
            let mut b = [0u8; 6144];
            let n = csvlog::catalog_scpi_on(img, &mut b).expect("cat");
            println!("cat={}", String::from_utf8_lossy(&b[..n]));
            return;
        }
        "list" | "del" => {
            if a[3] == "del" {
                let n = csvlog::parse_name(a[4].as_bytes()).expect("name");
                // del <NAME> mounted: a PC has the disk mounted (refused).
                let m = a.get(5).is_some_and(|x| x == "mounted");
                msc::HOST_MOUNTED.store(m, Relaxed);
                println!("del={:?}", csvlog::delete_on(img, &n));
                msc::HOST_MOUNTED.store(false, Relaxed);
            }
            let mut out = [csvlog::FileInfo { name: [0; 11], size: 0, kind: csvlog::Kind::Csv, in_dir: false }; 64];
            let c = csvlog::list_on(img, &mut out).expect("list");
            println!("list n={} total={} free={} disk={}", c.n, c.total, c.free_bytes, c.disk_bytes);
            for f in &out[..c.n] {
                let mut b = [0u8; 12];
                println!("  {} {} {:?}", csvlog::name_str(&f.name, &mut b), f.size, f.kind);
            }
            println!("  changed blocks: {:?}", img.writes);
            return;
        }
        "ival" => {
            // ival <interval idx> <duration idx> <readings> [<push until ms>]:
            // interval mode (Record page). Reading i at T0 + 100 i ms, value
            // i, flags: i = 25 OL, 30..39 all OL, 42 HOLD, 43 REL, 44 HI,
            // 45 LO; readings 60..69 left out (an interval with none). Then
            // polls until the recording ends: by its duration, or a stop
            // after the last reading when the duration is 0.
            let (ii, di, n): (u8, u8, u32) = (a[4].parse().unwrap(), a[5].parse().unwrap(), a[6].parse().unwrap());
            csvlog::set_next(ii, di);
            assert_eq!(csvlog::next(), (ii, di));
            assert!(csvlog::start("DC V", "V", "AUTO", T0));
            let mut now = T0;
            for i in 0..n {
                now = T0 + 100 * i as u64;
                if (60..70).contains(&i) {
                    csvlog::poll_on(img, now);
                    continue;
                }
                let mut f = 0u8;
                if i == 25 || (30..40).contains(&i) {
                    f |= csvlog::F_OL;
                }
                f |= match i {
                    42 => csvlog::F_HOLD,
                    43 => csvlog::F_REL,
                    44 => csvlog::F_HI,
                    45 => csvlog::F_LO,
                    _ => 0,
                };
                csvlog::push(now, i as f32, f, 6.0);
                csvlog::poll_on(img, now);
                if csvlog::status().state != csvlog::State::Recording {
                    println!("  ended by its duration at reading {i}");
                    break;
                }
            }
            if di == 0 {
                csvlog::stop();
            }
            let mut k = 0u64;
            while csvlog::active() && k < 100_000_000 {
                now += 10;
                csvlog::poll_on(img, now);
                k += 1;
            }
            let st = csvlog::status();
            println!("ival={:?} file={:?} rows={} dropped={} end_ms={}", st.state, st.file, st.rows, st.dropped, now - T0);
            match csvlog::free_bytes_on(img) {
                Ok(f) => println!("free={f} est_1s={} est_every={} cap={}", csvlog::est_hours(f, 1000), csvlog::est_hours(f, 100), csvlog::EST_CAP_H),
                Err(e) => println!("free={e:?}"),
            }
            return;
        }
        "chg" => {
            // chg <threshold idx>: On change mode (Record page), full scale
            // 6 V, a reading every 100 ms: 1 V (0..9), 1.003 V (10..19),
            // 2 V (20..29), OL (30..34), 2 V (35..39), 2 V HI (40..44),
            // 2 V (45), 2 V REL (46), 2 V (47..1299), 2.05 V (1300),
            // 2.4 V (1301..1309); then stop.
            let ci: u8 = a[4].parse().unwrap();
            csvlog::set_next(csvlog::ON_CHANGE, 0);
            csvlog::set_change(ci);
            assert_eq!((csvlog::next().0, csvlog::change()), (csvlog::ON_CHANGE, ci));
            assert!(csvlog::start("DC V", "V", "AUTO", T0));
            let mut now = T0;
            for i in 0..1310u32 {
                now = T0 + 100 * i as u64;
                let (v, f) = match i {
                    0..=9 => (1.0, 0),
                    10..=19 => (1.003, 0),
                    30..=34 => (0.0, csvlog::F_OL),
                    40..=44 => (2.0, csvlog::F_HI),
                    46 => (2.0, csvlog::F_REL),
                    1300 => (2.05, 0),
                    1301.. => (2.4, 0),
                    _ => (2.0, 0),
                };
                csvlog::push(now, v, f, 6.0);
                csvlog::poll_on(img, now);
            }
            csvlog::stop();
            let mut k = 0u64;
            while csvlog::active() && k < 100_000_000 {
                now += 10;
                csvlog::poll_on(img, now);
                k += 1;
            }
            let st = csvlog::status();
            println!("chg={:?} file={:?} rows={} dropped={}", st.state, st.file, st.rows, st.dropped);
            return;
        }
        "busydel" => {
            assert!(csvlog::start("DC V", "V", "AUTO", T0));
            for k in 0..20 {
                csvlog::poll_on(img, T0 + k);
            }
            let st = csvlog::status();
            let mut b = [0u8; 12];
            let name = calbackup::numbered_name(b"LOG", st.file.expect("file"), b"DLG");
            println!("busydel {} -> {:?}", csvlog::name_str(&name, &mut b), csvlog::delete_on(img, &name));
            csvlog::stop();
            for k in 0..20 {
                csvlog::poll_on(img, T0 + 100 + k);
            }
            println!("then del -> {:?}", csvlog::delete_on(img, &name));
            return;
        }
        _ => {}
    }
    assert_eq!(a[3], "rec");
    let n: u32 = a[4].parse().unwrap();
    let mut shots: Vec<u32> = vec![];
    let mut host: Option<(u32, u32)> = None;
    // mount <row> <rows>: a PC has the disk mounted (msc::host_mounted) for
    // that many rows: no commit meanwhile (review 6595d46 H1).
    // mountwin <row> <rows>: the same window, not mounted (the baseline: the
    // writes a commit makes there).
    let mut mount: Option<(u32, u32)> = None;
    let mut mounted = true;
    // shotw <row>: from that row on, start a screenshot the moment a commit
    // is in flight (the shot must wait before its FAT scan).
    let mut shotw: Option<u32> = None;
    let mut shot_waits = 0u32;
    // del <row> <name>: gallery delete during the recording (retried while busy).
    let mut del: Option<(u32, [u8; 11])> = None;
    let mut k = 5;
    while k < a.len() {
        match a[k].as_str() {
            "cut" => {
                img.cut = Some(a[k + 1].parse().unwrap());
                k += 2;
            }
            "half" => {
                img.half = true;
                k += 1;
            }
            "shot" => {
                shots.push(a[k + 1].parse().unwrap());
                k += 2;
            }
            "del" => {
                del = Some((a[k + 1].parse().unwrap(), csvlog::parse_name(a[k + 2].as_bytes()).expect("name")));
                k += 3;
            }
            "shotw" => {
                shotw = Some(a[k + 1].parse().unwrap());
                k += 2;
            }
            "host" => {
                host = Some((a[k + 1].parse().unwrap(), a[k + 2].parse().unwrap()));
                k += 3;
            }
            "mount" | "mountwin" => {
                mount = Some((a[k + 1].parse().unwrap(), a[k + 2].parse().unwrap()));
                mounted = a[k] == "mount";
                k += 3;
            }
            x => panic!("{x}"),
        }
    }
    mc.store(false, Relaxed);
    assert!(csvlog::start("DC V", "\u{3a9}", "AUTO 6 V", T0));
    assert!(!csvlog::start("DC V", "V", "AUTO", T0), "second start must be refused");
    let mut shot_res = vec![];
    let mut host_writes = 0usize;
    let mut mount_writes = 0usize;
    let mut lock_ok = true;
    let mut i = 0u32;
    let mut extra = 0u32;
    loop {
        let now = T0 + 100 * (i as u64 + 1);
        if i < n {
            if shots.contains(&i) {
                println!("  shot start at row {i}: {}", screenshot::start());
            }
            let h = host.is_some_and(|(s, l)| i >= s && i < s + l);
            msc::HOST_WRITING.store(h, Relaxed);
            let m = mount.is_some_and(|(s, l)| i >= s && i < s + l);
            msc::HOST_MOUNTED.store(m && mounted, Relaxed);
            if let Some((r, name)) = del {
                if i >= r {
                    let res = csvlog::delete_on(img, &name);
                    if res != Err(csvlog::GErr::Busy) {
                        println!("  gallery delete at row {i}: {res:?}");
                        del = None;
                    }
                }
            }
            let (v, f) = row(i);
            csvlog::push(now, v, f, 6.0);
            let w0 = img.writes.len();
            for _ in 0..POLLS_PER_ROW {
                // One writer at a time: a commit never starts while a
                // screenshot is in flight (the shot waits for a running one).
                let (was, sb) = (csvlog::writing(), screenshot::busy());
                csvlog::poll_on(img, now);
                if !was && csvlog::writing() && sb {
                    lock_ok = false;
                }
                if shotw.is_some_and(|r| i >= r) && csvlog::writing() && !screenshot::busy() {
                    shotw = None;
                    println!("  shot start at row {i} during a commit: {}", screenshot::start());
                }
                if screenshot::busy() && msc::LOG_FLUSHING.load(Relaxed) {
                    shot_waits += 1;
                }
                if let Some(r) = screenshot::poll_on(img) {
                    shot_res.push(r);
                }
            }
            if h {
                host_writes += img.writes.len() - w0;
            }
            if m {
                mount_writes += img.writes.len() - w0;
            }
            i += 1;
            if i == n {
                msc::HOST_WRITING.store(false, Relaxed);
                msc::HOST_MOUNTED.store(false, Relaxed);
                csvlog::stop();
                assert!(csvlog::active() || img.dead || csvlog::status().state == csvlog::State::Full);
            }
        } else {
            csvlog::poll_on(img, now);
            if let Some(r) = screenshot::poll_on(img) {
                shot_res.push(r);
            }
            extra += 1;
            if (!csvlog::active() && !screenshot::busy()) || extra > 1_000_000 {
                break;
            }
        }
        if img.dead && i >= n {
            break;
        }
    }
    let st = csvlog::status();
    let res = csvlog::take_result();
    let mut sb = [0u8; 64];
    let sn = csvlog::status_scpi(&mut sb);
    println!("  LOG:CSV? -> {}", String::from_utf8_lossy(&sb[..sn]));
    println!(
        "csv={:?} file={:?} rows={} bytes={} committed={} dropped={} medium_changed={} result={}",
        st.state,
        st.file,
        st.rows,
        st.bytes,
        st.committed,
        st.dropped,
        mc.load(Relaxed),
        res.is_some()
    );
    println!(
        "  shots={:?} shot_waits={} host_writes={} mount_writes={} lock_ok={} session_flag={}",
        shot_res,
        shot_waits,
        host_writes,
        mount_writes,
        lock_ok,
        msc::LOG_SESSION.load(Relaxed)
    );
    // Geometry: classify erases.
    let b = &img.d;
    let le16 = |o: usize| b[o] as u32 | (b[o + 1] as u32) << 8;
    let fat_start = le16(0x0E);
    let root = fat_start + b[0x10] as u32 * le16(0x16);
    let data = root + le16(0x11) * 32 / 4096;
    let (mut fat_e, mut root_e, mut data_e, mut data_max) = (0, 0, 0, 0);
    let dirs = dir_blocks(b);
    for (&l, &c) in &img.erases {
        if l < root {
            fat_e += c;
        } else if l < data || dirs.contains(&l) {
            root_e += c;
        } else {
            data_e += c;
            data_max = data_max.max(c);
        }
    }
    println!(
        "  writes={} programs={} erases: fat={} dir={} data={} data_max_per_block={} cut={:?} cut_kind={}",
        img.writes.len(),
        img.programs,
        fat_e,
        root_e,
        data_e,
        data_max,
        img.cut,
        img.cut_kind
    );
}

/// `main <img> fast <samples> <rate> [cut <k>] [dur <s>] [abandon] [ringdrop]`:
/// a fast-logger session (src/fastlog.rs) as main.rs drives it: begin, the
/// Prep slices, Ev::Start -> run(header), then <samples> synthetic AD2
/// records at <rate> S/s fed through a 2048-record ring (a few per pass;
/// `ringdrop`: bursts the ring cannot hold, dropped newest as logger.rs
/// does), then a stop request and the Stop slices. `cut`: power cut before
/// operation k (block write, page program, erase); `abandon`: stopped while
/// preparing. Prints the outcome; writes the CSV of the records the session
/// drained (the reference) to <img>.ref.csv.
fn fast(a: &[String], img: &mut Img) {
    use std::collections::VecDeque;
    let n: u32 = a[3].parse().unwrap();
    let rate: u32 = a[4].parse().unwrap();
    let mut dur = n / rate + 2;
    let mut abandon = false;
    let mut ringdrop = false;
    let mut k = 5;
    while k < a.len() {
        match a[k].as_str() {
            "cut" => {
                img.fcut = Some(a[k + 1].parse().unwrap());
                k += 1;
            }
            "dur" => {
                dur = a[k + 1].parse().unwrap();
                k += 1;
            }
            "abandon" => abandon = true,
            "ringdrop" => ringdrop = true,
            x => panic!("fast: {x}?"),
        }
        k += 1;
    }
    let par = dlog::FPar { a: -0.0012, b: 4.76837e-6, rate_mhz: rate * 1000, flags: dlog::FP_SCALED, src: 2, mode: 2, range: 0 };
    let text = b"# fatren-test fast log\r\nt_s,raw,value_V,lost_before\r\n";
    let bytes = fastlog::plan_bytes(dur, rate, true);
    fastlog::begin(bytes, dur).expect("begin");
    let mut ring: VecDeque<(u32, i32)> = VecDeque::new();
    let mut drained: Vec<(u32, i32)> = Vec::new();
    let (mut i, mut ring_drops, mut polls) = (0u32, 0u32, 0u64);
    let (mut started, mut logger_on, mut stop_sent) = (false, false, false);
    let p = 1e6 / rate as f64;
    loop {
        polls += 1;
        assert!(polls < 50_000_000, "fast: no end");
        if abandon && fastlog::preparing() && polls > 3 {
            fastlog::request_stop();
        }
        if logger_on {
            // a few conversions per pass; ringdrop: a 3000-sample burst
            // every 10000 (a stall longer than the ring)
            let burst = if ringdrop && i % 10_000 == 5_000 { 3000 } else { 4 };
            for _ in 0..burst {
                if i >= n {
                    break;
                }
                let t = (i as f64 + 1.0) * p + ((i * 7919) % 5) as f64 - 2.0;
                let raw = (90_000.0 * (i as f64 * 0.013).sin()) as i32 + ((i * 2654435761u32) >> 28) as i32;
                if ring.len() < 2048 {
                    ring.push_back((t as u64 as u32, raw));
                } else {
                    ring_drops += 1;
                }
                i += 1;
            }
            if i >= n && !stop_sent {
                stop_sent = true;
                fastlog::request_stop();
            }
        }
        let mut drain = |b: &mut [fastlog::Rec]| {
            let mut o = 0;
            while o < b.len() {
                let Some((ts, raw)) = ring.pop_front() else { break };
                drained.push((ts, raw));
                b[o] = (ts, raw);
                o += 1;
            }
            o
        };
        match fastlog::poll_on(img, logger_on, &mut drain) {
            fastlog::Ev::Start => {
                started = true;
                logger_on = true;
                fastlog::run(&fastlog::Head { par, unit: b"V", text });
            }
            fastlog::Ev::Stop => logger_on = false,
            fastlog::Ev::Restart | fastlog::Ev::None => {}
        }
        if img.dead || fastlog::ended().is_some() || (!fastlog::active() && !started && polls > 3) {
            break;
        }
    }
    let st = fastlog::status();
    println!(
        "fast={:?} file={:?} alloc={} bytes={} rows={} drained={} ring_drops={} ops={} programs={} erases={} chunks={} started={}",
        st.state,
        st.file,
        st.alloc,
        st.bytes,
        st.rows,
        drained.len(),
        ring_drops,
        img.fops,
        img.nor_progs,
        img.nor_erases,
        st.chunks,
        started
    );
    // The reference CSV of what was drained (rows the file should hold).
    let mut out = text.to_vec();
    let mut tprev = 0u64;
    let mut last = 0u32;
    for (j, &(ts, raw)) in drained.iter().enumerate() {
        let dt = if j == 0 { 0 } else { ts.wrapping_sub(last) };
        let t = if j == 0 { ts as u64 } else { tprev + dt as u64 };
        tprev = t;
        last = ts;
        let mut r = [0u8; 96];
        let k = dlog::fast_row(t, raw, dlog::fast_lost(dt, par.rate_mhz), &par, &mut r);
        out.extend_from_slice(&r[..k]);
    }
    fs::write(format!("{}.ref.csv", a[1]), &out).unwrap();
}

/// `main <img> setfile all|wrongsize`: setfile.rs (the settings' copy in
/// `_SETTING.BIN`) with persist.rs on the stub backup SRAM (mmio::BKP).
/// all: create, timed saves (debounce, the minimum gap, equal = no write,
/// a held disk defers, flush), only the slot blocks written after the
/// create; then boots: after a power-off (the newest slot), backup SRAM
/// valid (kept), the file newer (file), a CRC failure and a torn slot (the
/// other slot), both bad (defaults), a v9 slot (zero-extended), an unknown
/// version (skipped), sequence wrap. wrongsize: a 100-byte _SETTING.BIN is
/// replaced by the 8 KiB file.
fn setfile_cmd(a: &[String], img: Img) {
    use setfile::{From, Save};
    let img: &'static mut Img = Box::leak(Box::new(img));
    let raw: *mut Img = img;
    unsafe { setfile::HOST_DEV = Some(&mut *raw) };
    let im = || unsafe { &mut *raw };
    let mut fails = 0;
    let mut check = |ok: bool, what: &str| {
        println!("{}: setfile {what}", if ok { "ok" } else { "FAIL" });
        if !ok {
            fails += 1;
        }
    };
    let words = |tag: u32| -> [u32; persist::WORDS] {
        let mut w = [0u32; persist::WORDS];
        for (i, x) in w.iter_mut().enumerate() {
            *x = tag.wrapping_mul(0x9E37_79B9) ^ i as u32;
        }
        w
    };
    let boot = || {
        let i = im();
        setfile::boot(i);
        setfile::pick(persist::load())
    };
    if a[3] == "wrongsize" {
        // With a valid backup SRAM record (a unit updated to this build):
        // the new file gets it in slot 0 at once.
        persist::save(&words(1));
        setfile::boot(im());
        let (_, _, _, f, _, _, _) = setfile::info();
        check(f == setfile::FileState::Created, &format!("wrong-size file replaced ({f:?})"));
        let mut cl = [0u16; 2];
        let (data, _, size, _) = calbackup::chain_of(im(), &setfile::NAME, &mut cl).unwrap();
        let o = (data + cl[0] as u32 - 2) as usize * 4096;
        let seeded = setfile::record(&im().d[o..o + 4096]).is_some_and(|(r, q)| q == 1 && r.words == words(1));
        check(size == setfile::SIZE && seeded, "the new file seeded from the backup SRAM: slot 0 seq 1");
        let r = setfile::pick(persist::load());
        check(setfile::info().0 == From::Bkp && r.is_some(), "then the backup SRAM is used");
        save(&a[1], &im().d);
        std::process::exit(if fails == 0 { 0 } else { 1 });
    }
    // Pure parts.
    check(setfile::newest(&[Some(u32::MAX), Some(0)]) == Some(1), "sequence wraps (0 is newer than 0xFFFFFFFF)");
    check(setfile::newest(&[None, None]).is_none(), "no valid slot");
    check(setfile::newest(&[Some(7), None]) == Some(0), "the only valid slot");
    // First boot: the file is made (the backup SRAM is empty: nothing in it).
    let w0 = im().writes.len();
    let r = boot();
    let (from, _, _, f, _, _, _) = setfile::info();
    check(f == setfile::FileState::Created && r.is_none() && from == From::Defaults, &format!("first boot: {f:?}, {from:?}"));
    let create_writes = im().writes.len() - w0;
    println!("  create: {} block writes {:?}", create_writes, &im().writes[w0..]);
    // The slots' blocks, from the file entry.
    let mut cl = [0u16; 2];
    let (data, spc, size, n) = calbackup::chain_of(im(), &setfile::NAME, &mut cl).unwrap();
    check(size == setfile::SIZE && spc == 1 && n == 2, "file 8 KiB, 2 clusters of one block");
    let lba = [data + cl[0] as u32 - 2, data + cl[1] as u32 - 2];
    let slot = |k: usize| -> Vec<u8> { im().d[lba[k] as usize * 4096..][..4096].to_vec() };
    let seq_of = |k: usize| setfile::decode(&slot(k)).map(|d| d.1);
    let wstart = im().writes.len();
    // A change: saved DEBOUNCE_MS after it, not before.
    persist::save(&words(1));
    setfile::poll(0, false);
    setfile::poll(4000, false);
    check(seq_of(0).is_none() && seq_of(1).is_none(), "nothing written 4 s after a change");
    setfile::poll(5500, false);
    check(seq_of(0) == Some(1) && setfile::info().4 == Save::Ok, "written 5.5 s after the change: slot 0, seq 1");
    // Unchanged: nothing.
    for t in (6000..60_000).step_by(500) {
        persist::save(&words(1));
        setfile::poll(t, false);
    }
    check(im().writes.len() == wstart + 1, "equal record: no further write");
    // A second change: the other slot.
    persist::save(&words(2));
    setfile::poll(60_000, false);
    setfile::poll(66_000, false);
    check(seq_of(1) == Some(2) && seq_of(0) == Some(1), "second change: slot 1 seq 2, slot 0 kept");
    // A change while the disk is held: deferred, written once free.
    persist::save(&words(3));
    setfile::HOST_BUSY.store(true, std::sync::atomic::Ordering::Relaxed);
    setfile::poll(130_000, false);
    setfile::poll(136_000, false);
    check(seq_of(0) == Some(1) && setfile::info().4 == Save::Deferred, "disk held: deferred");
    setfile::poll(137_000, true);
    setfile::HOST_BUSY.store(false, std::sync::atomic::Ordering::Relaxed);
    check(seq_of(0) == Some(1), "AD1 logger session (hold): deferred");
    setfile::poll(137_500, false);
    check(seq_of(0) == Some(3), "disk free again: slot 0 seq 3");
    // flush (power-off): at once, no debounce, no gap.
    persist::save(&words(4));
    setfile::flush();
    check(seq_of(1) == Some(4), "flush: slot 1 seq 4 at once");
    setfile::flush();
    check(seq_of(0) == Some(3), "flush with nothing changed: nothing written");
    let only_slots = im().writes[wstart..].iter().all(|l| lba.contains(l));
    check(only_slots, &format!("saves wrote only the slot blocks ({:?})", &im().writes[wstart..]));
    // Boot after a power-off: the backup SRAM is gone, the newest slot.
    mmio::bkp_lost();
    let r = boot();
    let (from, sl, sq, f, ..) = setfile::info();
    check(f == setfile::FileState::Present && from == From::File && sl == 1 && sq == 4, &format!("power-off: {from:?} slot {sl} seq {sq}"));
    check(r.as_ref().is_some_and(|r| r.words == words(4) && r.version == persist::VERSION), "power-off: the settings of the last save");
    check(persist::seq() == Some(4), "the backup SRAM continues seq 4");
    // The backup SRAM valid and as new as the file: kept.
    persist::save(&words(5));
    let r = boot();
    check(setfile::info().0 == From::Bkp && r.is_some_and(|r| r.words == words(5)), "backup SRAM valid, seq = the file's: backup SRAM");
    // The backup SRAM valid but older than the file (seq 3 < 4): the file.
    persist::set_seq(3);
    let r = boot();
    check(setfile::info().0 == From::File && r.is_some_and(|r| r.words == words(4)), "file newer than the backup SRAM: file");
    // The backup SRAM without sequence words (an older build): kept.
    unsafe { mmio::BKP[0x3FA] = 0; mmio::BKP[0x3FB] = 0 };
    persist::save(&words(5));
    let r = boot();
    check(setfile::info().0 == From::Bkp && r.is_some_and(|r| r.words == words(5)) && persist::seq() == Some(4), "no sequence words: backup SRAM, continues seq 4");
    // CRC failure in the newest slot: the other one.
    let o = lba[1] as usize * 4096 + 40;
    im().d[o] ^= 0x10;
    mmio::bkp_lost();
    let r = boot();
    let (from, sl, sq, ..) = setfile::info();
    check(from == From::File && sl == 0 && sq == 3 && r.is_some_and(|r| r.words == words(3)), &format!("CRC failure in slot 1: slot {sl} seq {sq}"));
    // The next save goes to the bad slot (after the newest valid one).
    persist::save(&words(6));
    setfile::flush();
    check(seq_of(1) == Some(4) && seq_of(0) == Some(3), "save after a bad slot: slot 1 rewritten, seq 4");
    // Power cut in a save: slot 0 erased, its first page programmed only.
    let mut b = vec![0u8; 4096];
    setfile::encode(&mut b, persist::VERSION, 5, persist::WORDS, |i| words(7)[i]);
    let o = lba[0] as usize * 4096;
    im().d[o..o + 4096].fill(0xFF);
    im().d[o..o + 256].copy_from_slice(&b[..256]);
    mmio::bkp_lost();
    let r = boot();
    let (from, sl, sq, ..) = setfile::info();
    check(from == From::File && sl == 1 && sq == 4 && r.is_some_and(|r| r.words == words(6)), &format!("torn slot 0 (seq 5): slot {sl} seq {sq}"));
    // A power cut during the erase (slot all 0xFF): the same.
    im().d[o..o + 4096].fill(0xFF);
    let r = boot();
    check(setfile::info().0 == From::File && r.is_some_and(|r| r.words == words(6)), "erased slot 0: slot 1");
    // Both bad: defaults.
    let o1 = lba[1] as usize * 4096;
    im().d[o1..o1 + 4096].fill(0xFF);
    mmio::bkp_lost();
    let r = boot();
    check(setfile::info().0 == From::Defaults && r.is_none(), "both slots bad, no backup SRAM: defaults");
    // A v9 slot (62 words, before the power limits): zero-extended.
    let v9 = persist::words_of(9).unwrap();
    setfile::encode(&mut b, 9, 9, v9, |i| words(9)[i]);
    im().d[o..o + 4096].copy_from_slice(&b);
    let r = boot();
    let ok = r.as_ref().is_some_and(|r| r.version == 9 && r.words[..v9] == words(9)[..v9] && r.words[v9..].iter().all(|&w| w == 0));
    check(ok && setfile::info().0 == From::File, "v9 slot: loaded as version 9, zero-extended");
    // An unknown version (a newer build's), newer by seq: skipped.
    setfile::encode(&mut b, 99, 10, 70, |i| i as u32);
    im().d[o1..o1 + 4096].copy_from_slice(&b);
    let r = boot();
    check(r.is_some_and(|r| r.version == 9) && setfile::info().1 == 0, "unknown version 99 in slot 1: slot 0 used");
    // The next save from the v9 load: the current version, seq after 9.
    persist::save(&words(11));
    setfile::flush();
    check(seq_of(1) == Some(10) && setfile::decode(&slot(1)).is_some_and(|d| d.0 == persist::VERSION), "save after the v9 load: slot 1, current version, seq 10");
    // The wear budget: a change every 6 s; BUDGET saves, then one per
    // REFILL_MS (a fresh boot: a full budget).
    let r = boot();
    check(r.is_some(), "budget: boot");
    let n0 = setfile::info().5;
    let mut t = 0u64;
    let mut first = None;
    for i in 0..(setfile::BUDGET as u32 + 5) {
        persist::save(&words(100 + i));
        for _ in 0..12 {
            setfile::poll(t, false);
            if first.is_none() && setfile::info().5 > n0 {
                first = Some(t);
            }
            t += 500;
        }
    }
    let n = setfile::info().5 - n0;
    check(n == setfile::BUDGET as u32 && setfile::info().4 == Save::Deferred, &format!("budget: {n} saves of {} changes in {} s, then deferred", setfile::BUDGET + 5, t / 1000));
    let due = first.unwrap() + setfile::REFILL_MS;
    while t < due + 1000 {
        setfile::poll(t, false);
        t += 500;
    }
    check(setfile::info().5 - n0 == n + 1 && setfile::info().4 == Save::Ok, "budget: one more save REFILL_MS after the first");
    println!("  saves wrote {} blocks, erases per slot block: {:?} {:?}", im().writes.len() - wstart, im().erases.get(&lba[0]), im().erases.get(&lba[1]));
    save(&a[1], &im().d);
    if fails != 0 {
        println!("setfile: {fails} FAIL");
        std::process::exit(1);
    }
}
