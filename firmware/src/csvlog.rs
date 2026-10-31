//! Recording of DMM readings to the SPI-flash USB disk (TODO "CSV-to-disk
//! recording", FR-35, UF-32; "Compact binary logs (DLG)") and the
//! file-gallery backend (FR-16).
//!
//! File: the next free `LOGS/LOG%03u.DLG` (fatdir.rs; lowest number used
//! neither in LOGS/ nor in the root, where earlier builds wrote them), in the
//! compact binary format of
//! dlog.rs (docs/compact-log-format.md): ~1.1..1.9 bytes per reading instead
//! of ~21, CRC-protected chunks, one per commit. The CSV exists only on read
//! (virtual LOGS/LOG%03u.CSV on the read-only USB volume, SCPI MMEM:DATA?,
//! tools/dlog.py) and is byte for byte what this module wrote before DLG:
//! CRLF line ends, '.' decimal point, ASCII only:
//!   # Alientek DM40A, opendm40 0.1.0, function DC V, range AUTO, started uptime 12.345 s
//!   # time_s = seconds since the start of this recording (no real-time clock); empty value = overflow
//!   time_s,value,unit,flags
//!   0.250,1.23456,V,
//!   0.750,,V,OL
//! There is no RTC in opendm40 (the dates in calbackup / screenshot are a
//! constant for the same reason), so "started" is the uptime and time_s counts
//! from `start`. `value` is the caller's (SI) value with 6 significant digits,
//! plain decimal for 1e-3 <= |v| < 1e6, else d.dddddE+xx; empty for overload
//! (flag OL) or a non-finite value. `flags`: space-separated OL REL HOLD HI LO,
//! plus GAP on the first row after rows were dropped (buffer full).
//! Which value (app.rs `reading`): the UNSMOOTHED reading with REL applied
//! (`meter::Update::raw`), one row per Dmm reading. The FR-05 display
//! smoothing is a display preference, like for MIN / MAX / AVG (meter.rs
//! header): a log of a moving average would hide the spread and lag steps,
//! and the time column would no longer be the time of the value.
//!
//! Interval mode (Record page, ../docs/history/decisions-2026-10-22.md #1; `set_next`
//! before `start`, also for the swipe-down REC and LOG:CSV ON): one row per
//! interval I (1 s / 10 s / 1 min) instead of per reading. Interval k covers
//! readings at [(k-1) I, k I) from the start; its row has time_s = k I (the
//! END), value = the mean of its non-OL readings, flags = the OR of its
//! readings' flags. An OL reading makes it an OL row, which carries no value
//! (the DLG / CSV format: an OL row prints none). An interval without a
//! reading writes no row; `stop` writes the open, partial interval at its
//! nominal end. A duration (10 min .. 24 h) ends the recording by itself
//! (`push` / `poll`) with the usual result. Interval files have one more
//! header line ("# interval N s: ..."); every-reading files are unchanged.
//!
//! On-change mode (Record page "On change", LOG:CSV:INTerval CHANge; event
//! logging as the Fluke 289 / Keysight U1281A, TODO FR-04): a reading
//! becomes a row, at its own time as in every-reading mode, when it is
//! the first, when |value - the last row's value| > N % of the range's
//! full scale (`push` `fs`, per reading: autorange changes it), when its
//! OL / HI / LO flags differ from the last row's, or when the last row is
//! HEARTBEAT_MS old. N = CHANGES (0.1 / 1 / 5 %). One more header line
//! ("# event: change > N % ...").
//!
//! Non-blocking, like screenshot.rs: `start` / `push` / `stop` only touch RAM;
//! `poll` (every main-loop pass) does one bounded slice of a flush (one block
//! read, one Alloc FAT block, one `write_step` slice of <= ~2 ms of a data
//! block, or one whole FAT / root block write).
//! FAT and root blocks are written with `write_block` (erase + programs +
//! read back in one call, <= ~0.45 s, watchdog kicked in spiflash.rs), never
//! in `write_step` slices: a FAT / root block must not be left erased or half
//! programmed across a main-loop pass, where a power-off, a reset or a host
//! READ could see it (review 60605d2 H1, as screenshot.rs).
//!
//! RAM: rows are coded into FIFO (8 KiB ring, native) as they arrive (the
//! file header at `start`, then per chunk a CHDR-byte header placeholder
//! and the coded rows; the chunk is closed and its header + CRC patched in
//! the ring when a commit starts); the
//! file's last, partially filled 4 KiB block lives in TAIL. A flush ("commit")
//! moves the FIFO into TAIL block by block and writes it, in this order:
//!   Data  TAIL to its block (`write_step`); a full TAIL moves on to the next
//!         block, a new cluster when the block starts one (Alloc: first free
//!         FAT 1 entry from the last cluster on, one FAT block per pass);
//!   Fat   the new clusters chained + EOC and the old last cluster linked to
//!         the first new one, in every FAT copy (FAT 1 first; per copy the
//!         block holding the link last);
//!   Dir   the LOGS/ entry: start cluster and size (= every byte written).
//! Data -> FAT -> directory, as screenshot.rs / calbackup: the directory size
//! only ever covers written, linked data.
//!
//! Append without erase: unused bytes of TAIL are 0xFF, so rewriting the same
//! block with more rows only clears bits; spiflash.rs `write_step` then
//! programs the changed pages without the 4 KiB erase. Rows already committed
//! are therefore never inside an erase: a power cut while the tail block is
//! written cannot lose them. Only the first write to a new cluster erases it
//! (it held nothing of ours). The FAT blocks (new entries go 0 -> non-zero)
//! and the root block (the size field) need an erase per commit, as every
//! FAT writer; the second FAT copy is written after the first.
//!
//! When it commits: right after `start` (the file appears with its header),
//! when TAIL fills (one block; on this disk 1 cluster = 1 block, so the chain
//! grows by one cluster per commit), every COMMIT_MS with new rows, when FIFO
//! is 3/4 full, and at `stop`. Root-block wear: one erase per commit, i.e. at
//! ~1.2 bytes per row one per 4 KiB (~1 h at 1 reading/s, ~6 min at 10/s) or
//! per minute, whichever is first: ~1440 a day of continuous logging
//! against the W25Q128's 100k cycles. FAT block: one erase per copy per
//! commit that adds a cluster. Power-loss window: rows not yet committed (at
//! most COMMIT_MS or one block of rows).
//!
//! Power cut, worst case (tools/fatren-test, cut after every block write):
//!   during Data           nothing visible changes (lost clusters at worst:
//!                         none, the new cluster is not linked yet);
//!   between Fat and Dir   the chain is one cluster longer than the size in
//!                         the entry (fsck "cluster chain longer than file
//!                         size; truncating" = that cluster is freed), or the
//!                         FAT copies differ (FAT 1 ahead). Unavoidable with
//!                         two separately written blocks; no committed byte
//!                         is lost and no other file is touched;
//!   in an erase           of a FAT or root block: that block (FAT 1 / the
//!                         root block) is damaged, as for every FAT writer
//!                         including the screenshot and a PC.
//!
//! Sharing the disk (one writer at a time):
//!   * Host: the native build does not export this disk (USB shows the
//!     generated read-only volume, vdisk.rs; the finished log appears there
//!     as LOGS/LOGnnn.CSV). Builds that still export it refuse host
//!     WRITE(10)s for the whole session (NOT READY, msc.rs `LOG_SESSION`)
//!     and a commit does not start while one is in progress (`HOST_WRITING`).
//!   * Screenshot: a commit never starts while `screenshot::busy()`; the
//!     screenshot waits before its FAT scan while a commit runs
//!     (msc.rs `LOG_FLUSHING`). Both use diskbuf::blocks() only while they
//!     hold the disk. Rows keep going into FIFO meanwhile (~27 s at 10 rows/s
//!     before rows are dropped and the next row is flagged GAP).
//!   * Before every link the old last cluster must still be end-of-chain in
//!     FAT 1 and the new ones free; before every directory update the entry
//!     must still be this file with the last committed size. Otherwise the
//!     recording stops with Error and nothing more is written.
//!
//! Gallery (FR-16): `list_on` (S*.BMP, LOG*.DLG, SCOP*.DLG in the root,
//! LOGS/, SHOTS/, SCOPE/, sizes, free space) and `delete_on` (the entry + its
//! long-name entries 0xE5 first, then the chain freed in every FAT copy: a
//! cut in between = lost clusters).
//! Synchronous (a list is ~4 block reads, a delete one root-block erase + FAT
//! programs, ~50..400 ms); refused while a screenshot or a commit holds the
//! disk, while a host write is in progress, and for the file being recorded.

use core::sync::atomic::Ordering;

use crate::calbackup;
use crate::diskbuf;
use crate::dlog;
use crate::fatdir;
use crate::fatren::{le16, le32, parse_bpb, Vol};
use crate::msc::{BlockDevice, Step};

const BLK: usize = 4096;
const PER: u32 = (BLK / 32) as u32;
const FPER: u32 = (BLK / 2) as u32;
/// Write / create date 2026-10-13, time 0 (no RTC), as screenshot / calbackup.
const DATE: u16 = (46 << 9) | (10 << 5) | 13;
const NONE: u32 = u32::MAX;
const STEP0: Step = Step { started: false, phase: 0, page: 0, dirty: 0, erase: false, t0: 0 };
/// New rows are committed at least this often (ms).
pub const COMMIT_MS: u64 = 60_000;
const FW: &str = crate::ident::VERSION;

/// Row FIFO (formatted text). The RAM test image has no SRAM to spare.
const FIFO: usize = 8192;
const _: () = assert!(FIFO.is_power_of_two());
/// Clusters a single commit may add (FIFO / block + the block in TAIL).
const MAXNEW: usize = FIFO / BLK + 2;

#[cfg_attr(target_os = "none", link_section = ".dmabuf")]
static mut FIFO_BUF: [u8; FIFO] = [0; FIFO];
crate::wordram::usbhs_static! {
    /// The block being filled. In the USBHS FIFO RAM, which takes whole
    /// words only (wordram.rs; CPU only: the SPI flash takes it by CPU
    /// writes; docs/native-boot.md "Extra RAM areas"); `start` fills it with
    /// 0xFF.
    static TAIL: WordRam<{ BLK / 4 }>;
}

/// The row FIFO of a recording. Its third tenant, the MMEM:CAT? cache
/// (`Cat`), is dropped here: a recording's bytes overwrite it.
fn fifo() -> &'static mut [u8; FIFO] {
    cat().ok = false;
    unsafe { &mut *core::ptr::addr_of_mut!(FIFO_BUF) }
}

/// The row FIFO lent to the fast logger (fastlog.rs: its coded bytes on
/// the way to the flash pages). The two never run together: fastlog::begin
/// refuses while a recording is active, `start` refuses while a fast
/// session holds the disk; nothing of a finished recording is left in it.
/// Drops the MMEM:CAT? cache, as `fifo`.
#[inline(always)]
pub(crate) fn lend_fifo() -> &'static mut [u8; FIFO] {
    fifo()
}

/// The FIFO holds no bytes of a recording or a fast-log session: the
/// MMEM:CAT? cache may be written into it (`Cat`).
fn fifo_idle() -> bool {
    !active() && !crate::fastlog::active()
}

/// `push` flags.
pub const F_OL: u8 = 1;
pub const F_REL: u8 = 2;
pub const F_HOLD: u8 = 4;
pub const F_HI: u8 = 8;
pub const F_LO: u8 = 16;

/// Recorder state (`status().state`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum State {
    /// Never started since boot.
    Idle,
    Recording,
    /// `stop` called, the last commit pending / running.
    Stopping,
    /// Stopped, every row committed.
    Done,
    /// Disk or root directory full: what fitted is committed, recording ended.
    Full,
    /// No disk / not the expected FAT16 volume.
    NoDisk,
    /// A read / write failed, or the file changed under us: recording ended,
    /// the file holds what was committed before (at worst lost clusters).
    Error,
}

#[derive(Clone, Copy, Debug)]
pub struct Status {
    pub state: State,
    /// LOG%03u.DLG number, once chosen (first commit).
    pub file: Option<u16>,
    /// Rows accepted (dropped ones not counted).
    pub rows: u32,
    /// File (DLG) bytes accepted (header included), committed or pending.
    pub bytes: u32,
    /// File size on the disk (directory entry).
    pub committed: u32,
    /// Rows dropped because FIFO was full.
    pub dropped: u32,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    None,
    Bpb,
    Root,
    Data,
    Alloc,
    Fat,
    Dir,
}

struct Job {
    state: State,
    phase: Phase,
    stopping: bool,
    /// Disk full met in this commit: finish after it.
    full: bool,
    v: Option<Vol>,
    // --- file
    n: u16,
    slot: u32,
    /// Directory entry written at least once.
    has_file: bool,
    first: u32,
    last: u32,
    nlinked: u32,
    /// Clusters allocated in this commit, not yet in the FAT.
    newcl: [u32; MAXNEW],
    nnew: usize,
    /// Next cluster to try (Alloc), clusters scanned so far.
    hint: u32,
    scanned: u32,
    /// TAIL holds file block `tb`, bytes 0..tfill.
    tb: u32,
    tfill: u32,
    size: u32,
    csize: u32,
    // --- commit in flight
    tx_left: u32,
    filled: bool,
    lba: u32,
    st: Step,
    /// FAT 1 block cached in blocks()[1].
    cur: u32,
    fat_k: u32,
    fbs: [u32; MAXNEW + 1],
    nfb: usize,
    fbi: usize,
    i: u32,
    taken: [u32; 32],
    /// A block write was started this session (disk changed).
    wrote: bool,
    // --- rows
    fh: u32,
    ft: u32,
    rows: u32,
    bytes: u32,
    dropped: u32,
    gap: bool,
    t0_ms: u64,
    last_commit: u64,
    now: u64,
    unit: [u8; 12],
    ulen: usize,
    /// Chunk coder; FIFO index of the open chunk's header placeholder.
    enc: dlog::Enc,
    hpos: u32,
    // --- interval mode (Record page, decision 2026-10-22 #1)
    /// Row interval (ms, 0 = one row per reading) and recording length
    /// (ms, 0 = until stopped) of this recording (`set_interval` /
    /// `set_duration` at `start`).
    ival: u32,
    dur: u32,
    /// The open interval: its index k (it ends at k * ival), the sum and
    /// count of its non-OL readings, its readings, its ORed flags.
    ik: u32,
    isum: f64,
    inum: u32,
    iall: u32,
    iflags: u8,
    // --- on-change mode
    /// Threshold as a fraction of full scale (0 = not on-change mode); the
    /// last row's value (NaN: OL / none), flags (OL HI LO) and time (ms);
    /// a row was written.
    chg: f32,
    ev_v: f32,
    ev_f: u8,
    ev_t: u64,
    ev_any: bool,
}

const JOB0: Job = Job {
    state: State::Idle,
    phase: Phase::None,
    stopping: false,
    full: false,
    v: None,
    n: 0,
    slot: NONE,
    has_file: false,
    first: 0,
    last: 0,
    nlinked: 0,
    newcl: [0; MAXNEW],
    nnew: 0,
    hint: 2,
    scanned: 0,
    tb: 0,
    tfill: 0,
    size: 0,
    csize: 0,
    tx_left: 0,
    filled: false,
    lba: 0,
    st: STEP0,
    cur: NONE,
    fat_k: 0,
    fbs: [0; MAXNEW + 1],
    nfb: 0,
    fbi: 0,
    i: 0,
    taken: [0; 32],
    wrote: false,
    fh: 0,
    ft: 0,
    rows: 0,
    bytes: 0,
    dropped: 0,
    gap: false,
    t0_ms: 0,
    last_commit: 0,
    now: 0,
    unit: [0; 12],
    ulen: 0,
    enc: dlog::ENC0,
    hpos: 0,
    ival: 0,
    dur: 0,
    ik: 1,
    isum: 0.0,
    inum: 0,
    iall: 0,
    iflags: 0,
    chg: 0.0,
    ev_v: 0.0,
    ev_f: 0,
    ev_t: 0,
    ev_any: false,
};

/// Record page choices (decision 2026-10-22 #1): row interval in s (0 =
/// every reading, the default and today's behaviour) and recording length
/// in s (0 = until stopped). SCPI LOG:CSV:INTerval / DURation accept these
/// values only; persisted as their indices (app.rs, persist.rs W_REC).
pub const INTERVALS_S: [u32; 4] = [0, 1, 10, 60];
pub const DURATIONS_S: [u32; 5] = [0, 600, 3600, 8 * 3600, 24 * 3600];
/// Interval index of the on-change mode (after INTERVALS_S; SCPI
/// LOG:CSV:INTerval CHANge).
pub const ON_CHANGE: u8 = INTERVALS_S.len() as u8;
/// On-change thresholds in % of full scale (texts: header, Record page,
/// LOG:CSV:CHANge, which accepts these only); default index 1 (1 %).
pub const CHANGES: [&str; 3] = ["0.1", "1", "5"];
const CHANGE_FR: [f32; 3] = [0.001, 0.01, 0.05];
/// On-change mode: a row at least this often (ms) while readings arrive.
pub const HEARTBEAT_MS: u64 = 60_000;
/// Threshold index of the next recording (CHANGES).
static mut NEXT_CHG: u8 = 1;

/// The on-change threshold of the next recording (index into CHANGES;
/// out of range = the default 1 %).
pub fn set_change(i: u8) {
    unsafe { *core::ptr::addr_of_mut!(NEXT_CHG) = if (i as usize) < CHANGES.len() { i } else { 1 } };
}

pub fn change() -> u8 {
    unsafe { *core::ptr::addr_of!(NEXT_CHG) }
}
/// Row interval / length of the next recording (indices into the above).
static mut NEXT: (u8, u8) = (0, 0);

/// Settings of the next recording (Record page, LOG:CSV:INTerval /
/// DURation; the swipe-down REC uses them too). Out-of-range indices = the
/// defaults. A running recording keeps the values it started with.
pub fn set_next(ival_idx: u8, dur_idx: u8) {
    let i = if ival_idx <= ON_CHANGE { ival_idx } else { 0 };
    let d = if (dur_idx as usize) < DURATIONS_S.len() { dur_idx } else { 0 };
    unsafe { *core::ptr::addr_of_mut!(NEXT) = (i, d) };
}

/// (interval index, duration index) of the next recording.
pub fn next() -> (u8, u8) {
    unsafe { *core::ptr::addr_of!(NEXT) }
}

/// Before the first `start` (which sets JOB0) only `state` = Idle is read:
/// the sentinels JOB0 sets (slot / cur = NONE, hint 2, ik 1, the coder's
/// start state) are zero here, so JOB is .bss, not 512 B of the .data load image in region C (region C
/// audit #1).
static mut JOB: Job = Job { slot: 0, hint: 0, cur: 0, ik: 0, enc: dlog::ENC_ZERO, ..JOB0 };
/// The last finished recording, not yet shown (`take_result`).
static mut RESULT: Option<Status> = None;
/// The device's own SpiFlash handle (created on first use).
static mut DISK: Option<crate::spiflash::SpiFlash> = None;

fn job() -> &'static mut Job {
    unsafe { &mut *core::ptr::addr_of_mut!(JOB) }
}

fn set_flags(session: bool, flushing: bool) {
    crate::msc::LOG_SESSION.store(session, Ordering::Relaxed);
    crate::msc::LOG_FLUSHING.store(flushing, Ordering::Relaxed);
}

// ------------------------------------------------------------- recording API

/// `JOB0` as a region C template (`.rodata.cold_tpl`, link-native.x):
/// its readers (`start`, from App::rec_start) are region C code; a const copied there was an
/// anonymous Job image in region B.
#[repr(transparent)]
struct JobTpl(Job);
// SAFETY: read-only plain data.
unsafe impl Sync for JobTpl {}
#[cfg_attr(target_os = "none", link_section = ".rodata.cold_tpl")]
static JOB0_C: JobTpl = JobTpl(JOB0);

/// Start a recording: `func` / `range` go into the header (e.g. "DC V",
/// "AUTO"), `unit` into every row (e.g. "V"; non-ASCII is transliterated:
/// "\u{3a9}" -> "Ohm", "\u{b0}" -> "deg", "\u{b5}" -> "u"). `now_ms` = uptime,
/// the zero of time_s. False: a recording is already active, or a PC has
/// the disk mounted: no longer possible (USB exports only the generated
/// volume, vdisk.rs).
pub fn start(func: &str, unit: &str, range: &str, now_ms: u64) -> bool {
    let j = job();
    // The fast logger holds the disk and this FIFO (fastlog.rs).
    if active() || crate::fastlog::holds_disk() {
        return false;
    }
    // SAFETY: plain data, no Drop.
    unsafe { core::ptr::copy_nonoverlapping(&JOB0_C.0, j, 1) };
    j.state = State::Recording;
    j.t0_ms = now_ms;
    j.last_commit = now_ms;
    j.now = now_ms;
    TAIL.fill_words(0xFFFF_FFFF);
    j.ulen = ascii(unit, &mut j.unit);
    let mut h = [0u8; 384];
    let mut w = W { b: &mut h, n: 0 };
    w.s("# Alientek ");
    w.s(crate::ident::model_name());
    w.s(", opendm40 ");
    w.s(FW);
    w.s(", function ");
    w.a(func);
    w.s(", range ");
    w.a(range);
    w.s(", started uptime ");
    w.ms(now_ms);
    w.s(" s\r\n# time_s = seconds since the start of this recording (no real-time clock); empty value = overflow\r\n");
    let (ii, di) = next();
    j.ival = INTERVALS_S.get(ii as usize).map_or(0, |s| s * 1000);
    j.dur = DURATIONS_S[di as usize] * 1000;
    if ii == ON_CHANGE {
        let c = change() as usize;
        j.chg = CHANGE_FR[c];
        w.s("# event: change > ");
        w.s(CHANGES[c]);
        w.s(" % of the range's full scale, an OL / HI / LO change, or 60 s since the last row; time_s = time of the reading\r\n");
    }
    if j.ival > 0 {
        // Interval mode (Record page): one more comment line; every
        // reading mode keeps the header byte for byte.
        w.s("# interval ");
        w.u(j.ival as u64 / 1000);
        w.s(" s: one row per interval with readings, value = mean of its readings, time_s = end of the interval; OL = an overflow in it\r\n");
    }
    w.s("time_s,value,unit,flags\r\n");
    let n = w.n;
    let mut f = [0u8; dlog::FHDR + 384];
    let u = j.unit;
    let fl = dlog::file_header(dlog::KIND_DMM, &u[..j.ulen], &h[..n], &mut f);
    j.put(&f[..fl]);
    j.enc = dlog::Enc::new(n);
    set_flags(true, false);
    unsafe { *core::ptr::addr_of_mut!(RESULT) = None };
    true
}

/// One reading: `t_ms` = uptime (same clock as `start`), `value` in the
/// recording's unit, `flags` = F_* bits, `fs` = the full scale of the
/// reading's range in that unit (on-change mode). Every-reading mode: one
/// row each; interval mode: added to the open interval (`Job::add`);
/// on-change mode: a row if `Job::event` says so. A row that does
/// not fit FIFO is dropped (and counted); the next row that fits carries
/// GAP. Ignored unless recording. Ends the recording at its duration and
/// at the 49.7-day limit of the u32 ms DLG time.
pub fn push(t_ms: u64, value: f32, flags: u8, fs: f32) {
    let j = job();
    if j.state != State::Recording || j.stopping {
        return;
    }
    let t = t_ms.saturating_sub(j.t0_ms);
    if (j.dur > 0 && t >= j.dur as u64) || t > u32::MAX as u64 {
        stop();
        return;
    }
    if j.chg > 0.0 {
        j.event(t, value, flags & 31, fs);
    } else if j.ival == 0 {
        j.row(t, value, flags & 31);
    } else {
        j.add(t, value, flags & 31);
    }
}

/// End the recording: the remaining rows are committed by the next polls,
/// then `take_result` reports it.
pub fn stop() {
    let j = job();
    if j.state == State::Recording {
        // Interval mode: the open (partial) interval becomes the last row,
        // stamped at its nominal end like every other row.
        j.close_interval();
        j.stopping = true;
        j.state = State::Stopping;
    }
}

/// A recording is active (recording or stopping): show REC.
pub fn active() -> bool {
    matches!(job().state, State::Recording | State::Stopping)
}

/// The 8.3 name of the file being recorded (once it exists).
pub fn recording_name() -> Option<[u8; 11]> {
    let j = job();
    (active() && j.has_file).then(|| calbackup::numbered_name(b"LOG", j.n, b"DLG"))
}

/// A commit is writing the disk right now.
pub fn writing() -> bool {
    job().phase != Phase::None
}

pub fn status() -> Status {
    status_of(job())
}

fn status_of(j: &Job) -> Status {
    Status {
        state: j.state,
        file: j.has_file.then_some(j.n),
        rows: j.rows,
        bytes: j.bytes,
        committed: j.csize,
        dropped: j.dropped,
    }
}

/// The outcome of the last recording, once (when it ended).
pub fn take_result() -> Option<Status> {
    unsafe { (*core::ptr::addr_of_mut!(RESULT)).take() }
}

fn disk() -> &'static mut crate::spiflash::SpiFlash {
    let d = unsafe { &mut *core::ptr::addr_of_mut!(DISK) };
    d.get_or_insert_with(crate::spiflash::SpiFlash::new)
}

/// One bounded slice (main loop, every pass).
pub fn poll(now_ms: u64) {
    if active() {
        poll_on(disk(), now_ms);
    }
}

/// Drive a stop to the end now (power-off path): blocks for the last commit
/// (normally < 0.5 s). The caller must not run a screenshot meanwhile.
pub fn finish_blocking(now_ms: u64) {
    stop();
    let mut k = 0u32;
    while active() && k < 200_000 {
        poll(now_ms);
        crate::wdt::kick_if_feeding();
        k += 1;
    }
}

/// `poll` on any block device (host tests).
pub fn poll_on(disk: &mut dyn BlockDevice, now_ms: u64) {
    let j = job();
    j.now = now_ms;
    if !active() {
        return;
    }
    // Duration reached with no reading to notice it (push checks too).
    if j.state == State::Recording && j.dur > 0 && now_ms.saturating_sub(j.t0_ms) >= j.dur as u64 {
        stop();
    }
    if j.phase == Phase::None {
        let pending = j.fh.wrapping_sub(j.ft);
        let due = !j.has_file
            || j.stopping
            || j.tfill + pending >= BLK as u32
            || pending >= (FIFO as u32 / 4) * 3
            || j.enc.nrows >= 60_000
            || now_ms.saturating_sub(j.last_commit) >= COMMIT_MS;
        if pending == 0 {
            if j.stopping {
                j.finish(State::Done);
            }
            return;
        }
        // Deferred while a screenshot / scope record / an update-file upload
        // (upload.rs) holds the disk, or a
        // host WRITE(10) is in progress (only builds that still export the
        // SPI disk writable). Rows queue in the FIFO meanwhile; an overflow
        // drops them and flags the next row GAP.
        if !due || crate::screenshot::busy() || crate::upload::busy() || crate::msc::HOST_WRITING.load(Ordering::Relaxed) {
            return;
        }
        if !disk.present() || disk.block_size() as usize != BLK {
            j.finish(State::NoDisk);
            return;
        }
        j.close_chunk();
        j.tx_left = j.fh.wrapping_sub(j.ft);
        j.cur = NONE; // blocks()[1] was free for others since the last commit
        j.phase = if j.v.is_none() { Phase::Bpb } else { Phase::Data };
        set_flags(true, true);
    }
    let r = match j.phase {
        Phase::None => None,
        Phase::Bpb => j.bpb(disk),
        Phase::Root => j.root(disk),
        Phase::Data => j.data(disk),
        Phase::Alloc => j.alloc(disk),
        Phase::Fat => j.fat(disk),
        Phase::Dir => j.dir(disk),
    };
    if let Some(s) = r {
        j.finish(s);
    }
}

impl Job {
    /// Code one row (`t` ms since the start) into FIFO: the CSV row's
    /// length (the chunk header carries the CSV length), then the coded
    /// row; all or nothing (a full FIFO drops it, the next row that fits
    /// carries GAP).
    fn row(&mut self, t: u64, value: f32, flags: u8) {
        let fl = flags | if self.gap { dlog::F_GAP } else { 0 };
        let v = dlog::dec6(value);
        let u = self.unit;
        let mut row = [0u8; dlog::ROW_MAX];
        let len = dlog::fmt_row(t, v, &u[..self.ulen], fl, &mut row);
        let mut e = self.enc;
        let opening = !e.open;
        let mut b = [0u8; dlog::ROW_BYTES];
        let k = e.row(t as u32, v, fl, len as u32, &mut b);
        // Room for the chunk header (a new chunk) and the close's pad byte.
        let need = k + if opening { dlog::CHDR } else { 0 } + 1;
        let used = self.fh.wrapping_sub(self.ft) as usize;
        if FIFO - used >= need {
            if opening {
                self.hpos = self.fh;
                self.put(&[0u8; dlog::CHDR]);
            }
            self.put(&b[..k]);
            self.enc = e;
            self.rows += 1;
            self.gap = false;
        } else {
            self.dropped += 1;
            self.gap = true;
        }
    }

    /// On-change mode: a row for this reading when it is the first, its OL
    /// / HI / LO flags differ from the last row's, its value is more than
    /// `chg` x `fs` away from the last row's (both not OL), or the last
    /// row is HEARTBEAT_MS old.
    fn event(&mut self, t: u64, value: f32, flags: u8, fs: f32) {
        let ev = flags & (F_OL | F_HI | F_LO);
        let v = if flags & F_OL == 0 && value.is_finite() { value } else { f32::NAN };
        if !self.ev_any || ev != self.ev_f || (v - self.ev_v).abs() > self.chg * fs || t >= self.ev_t + HEARTBEAT_MS {
            self.row(t, value, flags);
            self.ev_any = true;
            self.ev_f = ev;
            self.ev_v = v;
            self.ev_t = t;
        }
    }

    /// Interval mode: a reading at `t` ms. Interval k covers
    /// [(k-1) I, k I); a reading at or past its end first closes it (one row
    /// at k I) and opens the interval it falls in (intervals without a
    /// reading write no row: a 2.2 s capacitance reading in 1 s intervals
    /// leaves holes in time_s, not empty rows that would read as OL).
    fn add(&mut self, t: u64, value: f32, flags: u8) {
        let i = self.ival as u64;
        if t >= self.ik as u64 * i {
            self.close_interval();
            self.ik = (t / i + 1) as u32;
        }
        self.iall += 1;
        self.iflags |= flags;
        if flags & F_OL == 0 && value.is_finite() {
            self.isum += value as f64;
            self.inum += 1;
        }
    }

    /// Write the open interval's row (if it has readings): time = its end
    /// k I, value = the mean of its non-OL readings, flags = OR of its
    /// readings' flags. OL in any reading makes it an OL row, whose value
    /// the DLG / CSV format does not carry (an OL row prints no value).
    fn close_interval(&mut self) {
        if self.ival == 0 || self.iall == 0 {
            return;
        }
        let v = if self.inum > 0 { (self.isum / self.inum as f64) as f32 } else { f32::NAN };
        let t = self.ik as u64 * self.ival as u64;
        let f = self.iflags;
        self.row(t, v, f);
        self.isum = 0.0;
        self.inum = 0;
        self.iall = 0;
        self.iflags = 0;
    }

    /// Close the open chunk (if any): pad byte, header with CRC patched
    /// into its placeholder in FIFO (not flushed yet: a commit takes FIFO
    /// only after this).
    fn close_chunk(&mut self) {
        if !self.enc.open {
            return;
        }
        let mut b = [0u8; 1];
        let n = self.enc.close(&mut b);
        // Room for it was reserved by push.
        let _ = self.put(&b[..n]);
        let mut h = self.enc.header();
        let f = fifo();
        let mut c = dlog::crc32_upd(!0, &h[8..]);
        let mut i = self.hpos.wrapping_add(dlog::CHDR as u32);
        while i != self.fh {
            c = dlog::crc32_upd(c, &[f[i as usize % FIFO]]);
            i = i.wrapping_add(1);
        }
        h[4..8].copy_from_slice(&(!c).to_le_bytes());
        for (k, &x) in h.iter().enumerate() {
            f[self.hpos.wrapping_add(k as u32) as usize % FIFO] = x;
        }
    }

    /// Append bytes to FIFO, all or nothing.
    fn put(&mut self, b: &[u8]) -> bool {
        let used = self.fh.wrapping_sub(self.ft) as usize;
        if FIFO - used < b.len() {
            return false;
        }
        let f = fifo();
        for &c in b {
            f[self.fh as usize % FIFO] = c;
            self.fh = self.fh.wrapping_add(1);
        }
        self.bytes += b.len() as u32;
        true
    }

    fn finish(&mut self, s: State) {
        self.phase = Phase::None;
        self.state = s;
        set_flags(false, false);
        // A finished LOG%03u.DLG appears as LOG%03u.CSV on LUN 1.
        #[cfg(feature = "vdisk")]
        dlog::set_stale();
        if self.wrote {
            crate::screenshot::MEDIUM_CHANGED.store(true, Ordering::Relaxed);
        }
        let st = status_of(self);
        unsafe { *core::ptr::addr_of_mut!(RESULT) = Some(st) };
    }

    /// End of a commit (its directory entry is written).
    fn committed(&mut self) -> Option<State> {
        self.phase = Phase::None;
        self.csize = self.size;
        self.last_commit = self.now;
        set_flags(true, false);
        if !self.has_file {
            self.has_file = true;
            crate::screenshot::MEDIUM_CHANGED.store(true, Ordering::Relaxed);
        }
        if self.full {
            return Some(State::Full);
        }
        if self.stopping && self.fh == self.ft {
            return Some(State::Done);
        }
        None
    }

    fn vol(&self) -> Vol {
        self.v.unwrap_or_else(|| unreachable!())
    }

    fn bpb(&mut self, disk: &mut dyn BlockDevice) -> Option<State> {
        let buf = &mut diskbuf::blocks()[0];
        if !disk.read_block(0, buf) {
            return Some(State::Error);
        }
        let Some(v) = parse_bpb(&buf[..], disk.block_count()) else {
            return Some(State::NoDisk);
        };
        // LOGS/ (fatdir.rs; created at boot, here only if it went missing:
        // synchronous, ~1 s once). blocks()[1] (FAT cache) is overwritten.
        let v = match fatdir::ensure(disk, &v, &fatdir::LOGS, diskbuf::blocks()) {
            Ok(d) => d,
            Err(fatdir::Err::Io) => return Some(State::Error),
            Err(_) => return Some(State::Full),
        };
        self.cur = NONE;
        self.v = Some(v);
        self.i = 0;
        self.phase = Phase::Root;
        None
    }

    /// One directory block (the root, then LOGS/, fatdir.rs `seq`):
    /// LOG%03u.DLG numbers in use in either, the first free slot of LOGS/.
    fn root(&mut self, disk: &mut dyn BlockDevice) -> Option<State> {
        let v = self.vol();
        let (lba, sub, base, lim) = fatdir::seq(&v, self.i);
        let buf = &mut diskbuf::blocks()[0];
        if !disk.read_block(lba, buf) {
            return Some(State::Error);
        }
        let mut end = false;
        for k in 0..PER {
            let idx = base + k;
            if idx >= lim {
                end = true;
                break;
            }
            let e = &buf[(k * 32) as usize..(k * 32 + 32) as usize];
            if e[0] == 0x00 || e[0] == 0xE5 {
                if sub && self.slot == NONE {
                    self.slot = idx;
                }
                if e[0] == 0x00 {
                    end = true;
                    break;
                }
                continue;
            }
            if e[11] & 0x3F == 0x0F || !e[3..6].iter().all(|c| c.is_ascii_digit()) {
                continue;
            }
            let n = (e[3] - b'0') as u16 * 100 + (e[4] - b'0') as u16 * 10 + (e[5] - b'0') as u16;
            if crate::fatren::sfn_is(e, &calbackup::numbered_name(b"LOG", n, b"DLG")) {
                self.taken[(n / 32) as usize] |= 1 << (n % 32);
            }
        }
        self.i += 1;
        if end {
            self.i = if sub { fatdir::seq_blocks(&v) } else { v.root_blocks };
        }
        if self.i < fatdir::seq_blocks(&v) {
            return None;
        }
        let Some(n) = (0..1000u16).find(|&n| self.taken[(n / 32) as usize] & (1 << (n % 32)) == 0) else {
            return Some(State::Full);
        };
        if self.slot == NONE {
            return Some(State::Full);
        }
        self.n = n;
        self.hint = 2;
        self.phase = Phase::Data;
        None
    }

    /// FAT 1 entry `c` through the one-block cache in blocks()[1].
    fn fat_get(&mut self, disk: &mut dyn BlockDevice, c: u32) -> Option<u32> {
        let v = self.vol();
        let fat = &mut diskbuf::blocks()[1];
        let fb = c / FPER;
        if fb >= v.fat_blocks {
            return None;
        }
        if fb != self.cur {
            if !disk.read_block(v.fat_start + fb, fat) {
                self.cur = NONE;
                return None;
            }
            self.cur = fb;
        }
        Some(le16(fat, ((c % FPER) * 2) as usize))
    }

    /// The cluster holding TAIL (the chain's last, linked or new).
    fn tail_cluster(&self) -> u32 {
        if self.nnew > 0 {
            self.newcl[self.nnew - 1]
        } else {
            self.last
        }
    }

    /// Fill TAIL from FIFO (once) and write it.
    fn data(&mut self, disk: &mut dyn BlockDevice) -> Option<State> {
        let v = self.vol();
        if !self.filled {
            if self.tx_left == 0 {
                self.phase = if self.nnew > 0 { self.fat_begin() } else { Phase::Dir };
                return None;
            }
            // A full TAIL stays in RAM until the next block starts (disk
            // full: the size is then cut back to its last whole row).
            let next = if self.tfill == BLK as u32 { self.tb + 1 } else { self.tb };
            if next / v.spc >= self.nlinked + self.nnew as u32 {
                if self.nnew == MAXNEW {
                    // Rare (a very long deferral): commit, go on next pass.
                    self.phase = self.fat_begin();
                } else {
                    self.scanned = 0;
                    self.phase = Phase::Alloc;
                }
                return None;
            }
            if self.tfill == BLK as u32 {
                self.tb += 1;
                self.tfill = 0;
                TAIL.fill_words(0xFFFF_FFFF);
            }
            let k = (BLK as u32 - self.tfill).min(self.tx_left);
            let f = fifo();
            let mut ft = self.ft;
            TAIL.put_bytes(self.tfill as usize, k as usize, || {
                let b = f[ft as usize % FIFO];
                ft = ft.wrapping_add(1);
                b
            });
            self.ft = ft;
            self.tfill += k;
            self.size += k;
            self.tx_left -= k;
            self.lba = v.data_start + (self.tail_cluster() - 2) * v.spc + self.tb % v.spc;
            self.filled = true;
            self.wrote = true;
            self.st = STEP0;
        }
        match disk.write_step(self.lba, &TAIL, &mut self.st)? {
            false => Some(State::Error),
            true => {
                self.filled = false;
                None
            }
        }
    }

    /// One FAT 1 block of the free-cluster search (from `hint`, wrapping).
    fn alloc(&mut self, disk: &mut dyn BlockDevice) -> Option<State> {
        let v = self.vol();
        let max = v.clusters + 1;
        if self.hint < 2 || self.hint > max {
            self.hint = 2;
        }
        let fb = self.hint / FPER;
        while self.scanned < v.clusters && self.hint / FPER == fb {
            let c = self.hint;
            let Some(e) = self.fat_get(disk, c) else {
                return Some(State::Error);
            };
            self.hint = if c == max { 2 } else { c + 1 };
            self.scanned += 1;
            if e == 0 && !self.newcl[..self.nnew].contains(&c) && c != self.last {
                self.newcl[self.nnew] = c;
                self.nnew += 1;
                self.phase = Phase::Data;
                return None;
            }
            if self.hint == 2 {
                break; // wrapped: next block is 0
            }
        }
        if self.scanned >= v.clusters {
            // Disk full: commit what is written (TAIL = the last, full
            // block), then stop. The chunk cut there is a torn tail: the
            // readers (dlog.rs, dlog.py) skip it, every whole chunk before
            // it stays readable.
            self.full = true;
            self.tx_left = 0;
            self.phase = if self.nnew > 0 { self.fat_begin() } else { Phase::Dir };
        }
        None
    }

    /// FAT blocks to update, the one with the link last.
    fn fat_begin(&mut self) -> Phase {
        let link = (self.nlinked > 0).then_some(self.last / FPER);
        self.nfb = 0;
        let add = |fb: u32, s: &mut Self| {
            if Some(fb) != link && !s.fbs[..s.nfb].contains(&fb) {
                s.fbs[s.nfb] = fb;
                s.nfb += 1;
            }
        };
        for i in 0..self.nnew {
            add(self.newcl[i] / FPER, self);
        }
        if let Some(l) = link {
            self.fbs[self.nfb] = l;
            self.nfb += 1;
        }
        self.fat_k = 0;
        self.fbi = 0;
        self.filled = false;
        Phase::Fat
    }

    fn fat(&mut self, disk: &mut dyn BlockDevice) -> Option<State> {
        let v = self.vol();
        if !self.filled {
            let fb = self.fbs[self.fbi];
            self.lba = v.fat_start + self.fat_k * v.fat_blocks + fb;
            let buf = &mut diskbuf::blocks()[0];
            if !disk.read_block(self.lba, buf) {
                return Some(State::Error);
            }
            let first = self.fat_k == 0;
            let mut set = |c: u32, val: u32, want_free: bool| -> bool {
                if c / FPER != fb {
                    return true;
                }
                let o = ((c % FPER) * 2) as usize;
                let old = le16(&buf[..], o);
                if first && (if want_free { old != 0 } else { old < 0xFFF8 }) {
                    return false; // changed since the scan / the last commit
                }
                buf[o..o + 2].copy_from_slice(&(val as u16).to_le_bytes());
                true
            };
            for i in 0..self.nnew {
                let next = if i + 1 < self.nnew { self.newcl[i + 1] } else { 0xFFFF };
                if !set(self.newcl[i], next, true) {
                    return Some(State::Error);
                }
            }
            if self.nlinked > 0 && !set(self.last, self.newcl[0], false) {
                return Some(State::Error);
            }
            self.filled = true;
            return None; // the write from the next pass
        }
        // Whole block in one call (H1: never left erased across a pass).
        let buf = &diskbuf::blocks()[0];
        match disk.write_block(self.lba, &buf[..]) {
            false => Some(State::Error),
            true => {
                self.filled = false;
                self.fbi += 1;
                if self.fbi == self.nfb {
                    self.fbi = 0;
                    self.fat_k += 1;
                    if self.fat_k == v.nfats {
                        if self.nlinked == 0 {
                            self.first = self.newcl[0];
                        }
                        self.nlinked += self.nnew as u32;
                        self.last = self.newcl[self.nnew - 1];
                        self.nnew = 0;
                        self.cur = NONE; // FAT 1 changed
                        self.phase = Phase::Dir;
                    }
                }
                None
            }
        }
    }

    /// The root entry: created on the first commit, then size updates.
    fn dir(&mut self, disk: &mut dyn BlockDevice) -> Option<State> {
        let v = self.vol();
        if !self.filled {
            if self.has_file && self.size == self.csize {
                return self.committed(); // nothing new (disk full at once)
            }
            self.lba = v.dlba(self.slot / PER);
            let buf = &mut diskbuf::blocks()[0];
            if !disk.read_block(self.lba, buf) {
                return Some(State::Error);
            }
            let o = ((self.slot % PER) * 32) as usize;
            let name = calbackup::numbered_name(b"LOG", self.n, b"DLG");
            let e = &mut buf[o..o + 32];
            let ok = if self.has_file {
                e[..11] == name[..] && le16(e, 0x1A) == self.first && le32(e, 0x1C) == self.csize
            } else {
                e[0] == 0x00 || e[0] == 0xE5
            };
            if !ok {
                return Some(State::Error); // taken / changed since
            }
            if self.size == 0 || self.first == 0 {
                return Some(State::Full); // nothing could be written
            }
            e.fill(0);
            e[..11].copy_from_slice(&name);
            e[11] = 0x20; // archive
            e[0x10..0x12].copy_from_slice(&DATE.to_le_bytes());
            e[0x12..0x14].copy_from_slice(&DATE.to_le_bytes());
            e[0x18..0x1A].copy_from_slice(&DATE.to_le_bytes());
            e[0x1A..0x1C].copy_from_slice(&(self.first as u16).to_le_bytes());
            e[0x1C..0x20].copy_from_slice(&self.size.to_le_bytes());
            self.filled = true;
            self.wrote = true;
            return None;
        }
        // Whole block in one call (H1: never left erased across a pass).
        let buf = &diskbuf::blocks()[0];
        match disk.write_block(self.lba, &buf[..]) {
            false => Some(State::Error),
            true => {
                self.filled = false;
                self.committed()
            }
        }
    }
}

// ------------------------------------------------------------- formatting

/// A tiny byte writer (silently truncates).
struct W<'a> {
    b: &'a mut [u8],
    n: usize,
}

impl W<'_> {
    fn c(&mut self, c: u8) {
        if self.n < self.b.len() {
            self.b[self.n] = c;
            self.n += 1;
        }
    }
    fn bytes(&mut self, s: &[u8]) {
        for &c in s {
            self.c(c);
        }
    }
    fn s(&mut self, s: &str) {
        self.bytes(s.as_bytes());
    }
    /// User text: ASCII, no CSV specials.
    fn a(&mut self, s: &str) {
        let mut t = [0u8; 24];
        let n = ascii(s, &mut t);
        self.bytes(&t[..n]);
    }
    fn u(&mut self, v: u64) {
        let mut d = [0u8; 20];
        let mut k = 0;
        let mut x = v;
        loop {
            d[k] = b'0' + (x % 10) as u8;
            k += 1;
            x /= 10;
            if x == 0 {
                break;
            }
        }
        while k > 0 {
            k -= 1;
            self.c(d[k]);
        }
    }
    /// Milliseconds as seconds with three decimals.
    fn ms(&mut self, ms: u64) {
        self.u(ms / 1000);
        self.c(b'.');
        let r = (ms % 1000) as u32;
        self.c(b'0' + (r / 100) as u8);
        self.c(b'0' + (r / 10 % 10) as u8);
        self.c(b'0' + (r % 10) as u8);
    }
}

/// ASCII copy of `s` (transliterating the meter's symbols, no ',' '"' or
/// control characters); returns the length.
fn ascii(s: &str, out: &mut [u8]) -> usize {
    let mut w = W { b: out, n: 0 };
    for ch in s.chars() {
        match ch {
            '\u{3a9}' | '\u{2126}' => w.s("Ohm"),
            '\u{b0}' => w.s("deg"),
            '\u{b5}' | '\u{3bc}' => w.c(b'u'),
            ',' | '"' => w.c(b' '),
            c if c.is_ascii_graphic() || c == ' ' => w.c(c as u8),
            _ => w.c(b'?'),
        }
    }
    w.n
}

/// Header name of a function code (meter::func_of / Dmm::func codes).
pub fn func_label(code: u8) -> &'static str {
    match code {
        0 | 2 => "AC V",
        1 | 3 => "DC V",
        4..=6 => "AC A",
        7..=9 => "DC A",
        10 => "Resistance",
        11 => "Continuity",
        12 => "Diode",
        13 => "Capacitance",
        14 => "Frequency",
        16 => "AC+DC V",
        17 => "AC+DC A",
        // dmm::FUNC_V_AUTO / FUNC_A_AUTO (literal: host tests build this
        // module without dmm.rs). Not used for a recording: one of AUTO's
        // parts is recorded, as DC or AC (app.rs rec_start).
        19 => "AUTO V",
        20 => "AUTO A",
        18 => "Power",
        _ => "Temperature",
    }
}

// ------------------------------------------------------------- gallery

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Bmp,
    Csv,
}

#[derive(Clone, Copy, Debug)]
pub struct FileInfo {
    /// 8.3 name, space padded ("LOG001  DLG"); `name_str` formats it.
    pub name: [u8; 11],
    pub size: u32,
    #[allow(dead_code)] // tests; the firmware's lists need only the count
    pub kind: Kind,
    /// In its directory (LOGS/, SHOTS/, SCOPE/), not a legacy root file.
    pub in_dir: bool,
}

#[derive(Clone, Copy, Debug)]
pub struct Catalog {
    /// Entries stored in `out` (directory order).
    pub n: usize,
    /// Matching files in the root (may exceed `out.len()`).
    pub total: usize,
    pub free_bytes: u32,
    pub disk_bytes: u32,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum GErr {
    NoDisk,
    /// A screenshot / a commit / a host write holds the disk, or the file is
    /// the one being recorded: try again later.
    Busy,
    NotFound,
    Error,
}

/// The disk is free for a synchronous gallery operation.
fn gallery_free() -> bool {
    !crate::screenshot::busy()
        && !writing()
        && !crate::upload::busy()
        && !crate::fastlog::holds_disk()
        && !crate::msc::HOST_WRITING.load(Ordering::Relaxed)
}

fn gallery_vol(disk: &mut dyn BlockDevice) -> Result<Vol, GErr> {
    if !gallery_free() {
        return Err(GErr::Busy);
    }
    if !disk.present() || disk.block_size() as usize != BLK {
        return Err(GErr::NoDisk);
    }
    let buf = &mut diskbuf::blocks()[0];
    if !disk.read_block(0, buf) {
        return Err(GErr::Error);
    }
    parse_bpb(&buf[..], disk.block_count()).ok_or(GErr::NoDisk)
}

fn kind_of(e: &[u8]) -> Option<Kind> {
    if e[11] & 0x18 != 0 || e[11] & 0x3F == 0x0F {
        return None; // directory, volume label, LFN
    }
    kind_of_name(&e[..11])
}

fn kind_of_name(e: &[u8]) -> Option<Kind> {
    let up = |c: u8| c.to_ascii_uppercase();
    let ext = [up(e[8]), up(e[9]), up(e[10])];
    if up(e[0]) == b'S' && &ext == b"BMP" {
        Some(Kind::Bmp)
    } else if &ext == b"DLG"
        && ((up(e[0]) == b'L' && up(e[1]) == b'O' && up(e[2]) == b'G')
            || e[..4].iter().map(|&c| up(c)).eq(*b"SCOP")
            || e[..4].iter().map(|&c| up(c)).eq(*b"FAST"))
    {
        // LOG*.DLG (DMM recordings), FAST*.DLG (the fast logger,
        // fastlog.rs) and SCOP*.DLG (scope records, UF-13), compact binary
        // logs (dlog.rs); their CSV is generated on read.
        Some(Kind::Csv)
    } else {
        None
    }
}

/// Root files S*.BMP and LOG*.CSV (directory order) into `out`, with the
/// free space. Synchronous: the root blocks + FAT 1.
pub fn list_on(disk: &mut dyn BlockDevice, out: &mut [FileInfo]) -> Result<Catalog, GErr> {
    let mut n = 0usize;
    let mut cat = for_each_file(disk, &mut |f| {
        if n < out.len() {
            out[n] = *f;
            n += 1;
        }
    })?;
    cat.n = n;
    Ok(cat)
}

/// Every gallery file to `f` (the root's legacy files, then LOGS/, SHOTS/,
/// SCOPE/; directory order); `n` = 0 in the result.
fn for_each_file(disk: &mut dyn BlockDevice, f: &mut dyn FnMut(&FileInfo)) -> Result<Catalog, GErr> {
    let v = gallery_vol(disk)?;
    let mut cat = Catalog { n: 0, total: 0, free_bytes: 0, disk_bytes: v.clusters * v.spc * BLK as u32 };
    let buf = &mut diskbuf::blocks()[0];
    fatdir::for_each(disk, &v, &[&fatdir::LOGS, &fatdir::SHOTS, &fatdir::SCOPE], buf, &mut |d, _, e| {
        if let Some(kind) = kind_of(e) {
            let mut name = [0u8; 11];
            name.copy_from_slice(&e[..11]);
            f(&FileInfo { name, size: le32(e, 0x1C), kind, in_dir: d.dn > 0 });
            cat.total += 1;
        }
        true
    })
    .map_err(|_| GErr::Error)?;
    cat.free_bytes = free_in(disk, &v, buf)?;
    Ok(cat)
}

/// Free bytes of the volume: the free clusters of FAT 1 (a few block reads).
fn free_in(disk: &mut dyn BlockDevice, v: &Vol, buf: &mut [u8; BLK]) -> Result<u32, GErr> {
    let mut free = 0u32;
    let mut fb = NONE;
    for c in 2..=v.clusters + 1 {
        if c / FPER != fb {
            fb = c / FPER;
            if !disk.read_block(v.fat_start + fb, buf) {
                return Err(GErr::Error);
            }
        }
        if le16(&buf[..], ((c % FPER) * 2) as usize) == 0 {
            free += 1;
        }
    }
    Ok(free * v.spc * BLK as u32)
}

/// Free bytes on the disk (Record page estimate, SCPI LOG:CSV:FREE?).
/// Synchronous (the boot block + the FAT 1 blocks, ~3 reads); Busy while a
/// screenshot, a commit, an upload or a host write holds the disk.
pub fn free_bytes_on(disk: &mut dyn BlockDevice) -> Result<u32, GErr> {
    let v = gallery_vol(disk)?;
    free_in(disk, &v, &mut diskbuf::blocks()[0])
}

pub fn free_bytes() -> Result<u32, GErr> {
    free_bytes_on(disk())
}

/// Recording time the free space holds at one row per `row_ms` (hours,
/// rounded down), at a conservative 2 bytes per row (DLG measures
/// 1.1..1.9), capped at the 49.7-day (u32 ms) limit of one recording.
pub fn est_hours(free: u32, row_ms: u32) -> u32 {
    let rows = free as u64 / 2;
    let ms = rows * row_ms.max(1) as u64;
    (ms.min(u32::MAX as u64) / 3_600_000) as u32
}

/// `est_hours` reaches the per-recording limit (shown as "max").
pub const EST_CAP_H: u32 = (u32::MAX as u64 / 3_600_000) as u32;

/// SCPI MMEMory:CATalog? reply into `out`: `<used>,<free>{,"<name>,<type>,<size>"}`
/// (bytes; type BIN: S*.BMP, LOG*.DLG, SCOP*.DLG; gallery files only). Entries
/// that do not fit are left out. Returns the length.
pub fn catalog_scpi_on(disk: &mut dyn BlockDevice, out: &mut [u8]) -> Result<usize, GErr> {
    // The two numbers first need the free count, known only after the scan:
    // write the entries after a 24-byte gap, then move them up.
    const GAP: usize = 24;
    if out.len() < GAP + 2 {
        return Err(GErr::Error);
    }
    let (head, rest) = out.split_at_mut(GAP);
    let mut w = W { b: rest, n: 0 };
    let cat = for_each_file(disk, &mut |f| {
        let mut nb = [0u8; 12];
        let mut e = [0u8; 48];
        let mut t = W { b: &mut e, n: 0 };
        t.s(",\"");
        if let (true, Some(d)) = (f.in_dir, fatdir::dir_of(&f.name)) {
            t.s(name_str(d, &mut nb));
            t.c(b'/');
        }
        t.s(name_str(&f.name, &mut nb));
        t.s(",BIN,");
        t.u(f.size as u64);
        t.c(b'"');
        let n = t.n;
        if w.n + n <= w.b.len() {
            w.bytes(&e[..n]);
        }
    })?;
    let body = w.n;
    let mut h = W { b: head, n: 0 };
    h.u((cat.disk_bytes - cat.free_bytes) as u64);
    h.c(b',');
    h.u(cat.free_bytes as u64);
    let hn = h.n;
    out.copy_within(GAP..GAP + body, hn);
    Ok(hn + body)
}

/// The last MMEM:CAT? reply, valid while no program / erase reached the SPI
/// flash since it was built (`msc::FLASH_WRITES`, bumped by every one: a
/// recording's commit, a screenshot, a host write, the shot pre-erase).
/// Device c92b605: MMEM:CAT? in the Wave view at 76800 S/s took 123 ms in
/// one main-loop pass (the BPB, the root and the three directories, FAT 1:
/// ~10 polled 4 KiB reads with the stream ISR at ~50 % CPU), the view's ring
/// (2048 samples, 27 ms) dropped +6813 per query. From the cache a reply is
/// one copy; `catalog_warm` fills it before a stream starts (main.rs: the
/// Wave view, the inrush page), so a host listing the files while the
/// stream runs finds it valid (nothing writes the disk then: a disk job
/// pauses the Wave view's stream and the restart warms it again).
///
/// The reply itself is kept in the first CAT_MAX bytes of the row FIFO
/// (`FIFO_BUF`; docs/native-boot.md "SRAM audit"), written only while
/// `fifo_idle`. `fifo` / `lend_fifo` clear `ok` before a recording or a
/// fast-log session writes there. A fast-log session holds the disk
/// (`gallery_free`: MMEM:CAT? is Busy); during a recording every query
/// reads the disk, but no stream the cache is for runs then: the Wave view
/// and inrush refuse to start during one, CH2 stops (app_wave.rs,
/// app_inrush.rs, app_ch2.rs `csvlog::active`).
struct Cat {
    ok: bool,
    stamp: u32,
    len: usize,
}

/// The reply buffers' size (usbtmc::RSP_MAX, the BT block: main.rs asserts).
pub const CAT_MAX: usize = 6144;
const _: () = assert!(CAT_MAX <= FIFO);

static mut CAT: Cat = Cat { ok: false, stamp: 0, len: 0 };

#[inline(always)]
fn cat() -> &'static mut Cat {
    unsafe { &mut *core::ptr::addr_of_mut!(CAT) }
}

/// The cached reply's bytes (the FIFO's head; see `Cat`). Read only while
/// `cat().ok`, written only while `fifo_idle()`.
#[inline(always)]
fn cat_buf() -> &'static mut [u8] {
    unsafe { &mut (&mut *core::ptr::addr_of_mut!(FIFO_BUF))[..CAT_MAX] }
}

fn flash_writes() -> u32 {
    crate::msc::FLASH_WRITES.load(Ordering::Relaxed)
}

/// Region B (not .cold): a host may list the files while the Wave view's
/// stream ISR runs (emu/hy3131/wavetest.py, region C hooks). Busy as
/// before while a writer holds the disk; else the cached reply when the
/// flash has not changed since it was built (`Cat`).
#[inline(never)] // not inlined into the hot caller
pub fn catalog_scpi(out: &mut [u8]) -> Result<usize, GErr> {
    if !gallery_free() {
        return Err(GErr::Busy);
    }
    let c = cat();
    let w = flash_writes();
    if c.ok && c.stamp == w && c.len <= out.len() {
        out[..c.len].copy_from_slice(&cat_buf()[..c.len]);
        return Ok(c.len);
    }
    let n = catalog_scpi_on(disk(), out)?;
    if n <= CAT_MAX && flash_writes() == w && fifo_idle() {
        cat_buf()[..n].copy_from_slice(&out[..n]);
        (c.ok, c.stamp, c.len) = (true, w, n);
    }
    Ok(n)
}

/// Build the MMEM:CAT? cache now if it is stale (main.rs, before a stream
/// starts: one scan, ~10 block reads, while the stream ISR is not running).
/// Nothing when the disk is busy or the FIFO is in use.
#[inline(never)]
pub fn catalog_warm() {
    let c = cat();
    let w = flash_writes();
    if (c.ok && c.stamp == w) || !gallery_free() || !fifo_idle() {
        return;
    }
    c.ok = false;
    if let Ok(n) = catalog_scpi_on(disk(), cat_buf()) {
        if flash_writes() == w {
            (c.ok, c.stamp, c.len) = (true, w, n);
        }
    }
}

/// `status()` as the SCPI LOG:CSV? reply: `<state>,<file>,<rows>,<bytes>,<committed>,<dropped>`
/// (state 0 idle, 1 recording, 2 stopping, 3 done, 4 full, 5 no disk,
/// 6 error; file -1 = none yet). Returns the length.
pub fn status_scpi(out: &mut [u8]) -> usize {
    let st = status();
    let mut w = W { b: out, n: 0 };
    w.u(st.state as u64);
    w.c(b',');
    match st.file {
        Some(n) => w.u(n as u64),
        None => w.s("-1"),
    }
    for v in [st.rows, st.bytes, st.committed, st.dropped] {
        w.c(b',');
        w.u(v as u64);
    }
    w.n
}

fn sfn_checksum(e: &[u8]) -> u8 {
    let mut s = 0u8;
    for &c in &e[0..11] {
        s = ((s & 1) << 7).wrapping_add(s >> 1).wrapping_add(c);
    }
    s
}

/// Delete gallery file `name` (8.3, space padded; S*.BMP / LOG*.DLG /
/// SCOP*.DLG) from its directory (fatdir::dir_of) or, a legacy file, from
/// the root: its entry and long-name entries 0xE5 (read back), then its
/// chain freed in every FAT copy (fatren::free_chain: bits cleared, no erase).
pub fn delete_on(disk: &mut dyn BlockDevice, name: &[u8; 11]) -> Result<(), GErr> {
    if kind_of_name(name).is_none() {
        return Err(GErr::NotFound);
    }
    let j = job();
    if active() && j.has_file && *name == calbackup::numbered_name(b"LOG", j.n, b"DLG") {
        return Err(GErr::Busy);
    }
    let root = gallery_vol(disk)?;
    let [buf, fat1, tmp] = diskbuf::blocks();
    let sub = match fatdir::dir_of(name).map(|d| fatdir::open(disk, &root, d, buf)) {
        Some(Ok(d)) => d,
        Some(Err(fatdir::Err::Io)) => return Err(GErr::Error),
        _ => None,
    };
    for v in [sub, Some(root)].into_iter().flatten() {
        match delete_in(disk, &v, name, buf, fat1, tmp) {
            Err(GErr::NotFound) => continue,
            r => return r,
        }
    }
    Err(GErr::NotFound)
}

/// `delete_on` in one directory (`v`'s view).
fn delete_in(disk: &mut dyn BlockDevice, v: &Vol, name: &[u8; 11], buf: &mut [u8; BLK], fat1: &mut [u8; BLK], tmp: &mut [u8; BLK]) -> Result<(), GErr> {
    // Find the entry; remember the live LFN run just before it (global indexes).
    let mut run = [0u32; 20];
    let mut nrun = 0usize;
    let mut hit: Option<(u32, u32, [u32; 20], usize)> = None;
    'blocks: for b in 0..v.dblocks() {
        if !disk.read_block(v.dlba(b), buf) {
            return Err(GErr::Error);
        }
        for k in 0..PER {
            let idx = b * PER + k;
            if idx >= v.dentries() {
                break 'blocks;
            }
            let e = &buf[(k * 32) as usize..(k * 32 + 32) as usize];
            if e[0] == 0x00 {
                break 'blocks;
            }
            if e[0] == 0xE5 {
                nrun = 0;
                continue;
            }
            if e[11] & 0x3F == 0x0F {
                if nrun == run.len() {
                    run.copy_within(1.., 0);
                    nrun -= 1;
                }
                run[nrun] = idx;
                nrun += 1;
                continue;
            }
            if e[11] & 0x18 == 0 && crate::fatren::sfn_is(e, name) {
                // The LFN run before it; only entries with its checksum are
                // marked below.
                hit = Some((idx, le16(e, 0x1A) | (sfn_checksum(e) as u32) << 16, run, nrun));
                break 'blocks;
            }
            nrun = 0;
        }
    }
    let Some((idx, start_ck, lfn, nl)) = hit else {
        return Err(GErr::NotFound);
    };
    let (start, ck) = (start_ck & 0xFFFF, (start_ck >> 16) as u8);
    // Entries 0xE5, block by block (LFN entries only with a matching checksum).
    for b in 0..v.dblocks() {
        let (lo, hi) = (b * PER, b * PER + PER);
        let inb = |i: u32| i >= lo && i < hi;
        if !inb(idx) && !lfn[..nl].iter().any(|&i| inb(i)) {
            continue;
        }
        if !disk.read_block(v.dlba(b), buf) {
            return Err(GErr::Error);
        }
        for &i in &lfn[..nl] {
            let o = ((i - lo) * 32) as usize;
            if inb(i) && buf[o + 13] == ck {
                buf[o] = 0xE5;
            }
        }
        if inb(idx) {
            buf[((idx - lo) * 32) as usize] = 0xE5;
        }
        if !(disk.write_block(v.dlba(b), buf) && crate::fatren::verify(disk, v.dlba(b), buf, tmp)) {
            return Err(GErr::Error);
        }
    }
    crate::screenshot::MEDIUM_CHANGED.store(true, Ordering::Relaxed);
    #[cfg(feature = "vdisk")]
    {
        dlog::set_stale();
        crate::vdisk::forget(name);
    }
    let (ok, _) = crate::fatren::free_chain(disk, v, start, fat1, buf, tmp);
    if ok {
        Ok(())
    } else {
        Err(GErr::Error)
    }
}

/// Device wrappers (the recorder's SpiFlash handle).
#[allow(dead_code)]
pub fn list(out: &mut [FileInfo]) -> Result<Catalog, GErr> {
    list_on(disk(), out)
}
#[inline(never)] // .cold (link-native.x): not inlined into the hot caller
pub fn delete(name: &[u8; 11]) -> Result<(), GErr> {
    delete_on(disk(), name)
}

/// "log1.csv" / "LOG001.CSV" -> "LOG1    CSV" (8.3, upper case). A
/// directory prefix "LOGS/", "SHOTS/" or "SCOPE/" (any case, '/' or '\\', a
/// leading '/' too) is accepted and dropped. None: not an 8.3 name.
#[allow(dead_code)] // host tests (tools/fatren-test); MMEM uses parse_path
pub fn parse_name(s: &[u8]) -> Option<[u8; 11]> {
    parse_path(s).map(|(n, _)| n)
}

/// `parse_name`, and whether a directory prefix names the file's own
/// directory (as MMEM:CAT? and the USB volume list it: S*.BMP in SHOTS/,
/// LOG* / FAST* .DLG / .CSV in LOGS/, SCOP* in SCOPE/; fatdir::dir_of).
/// False for a prefix of another directory, or for a file that is in none
/// (device 2f56ff9: "SHOTS/LOG005.DLG" found LOGS/LOG005.DLG): MMEM:DATA? /
/// MMEM:DELete then answer "File name not found". True without a prefix.
// Not .cold: MMEM:DATA? runs it during an AD2 stream (a host reading a
// file while the Wave view streams; emu/hy3131/wavetest.py). inline(never):
// one copy for both MMEM callers.
#[inline(never)]
pub fn parse_path(s: &[u8]) -> Option<([u8; 11], bool)> {
    let s = match s {
        [b'"', m @ .., b'"'] => m,
        _ => s,
    };
    let s = match s {
        [b'/' | b'\\', m @ ..] => m,
        _ => s,
    };
    // Upper-cased into a blank 8.3 field: the directory, the base, the extension.
    let put = |out: &mut [u8], src: &[u8]| -> Option<()> {
        if src.len() > out.len() {
            return None;
        }
        for (o, &c) in out.iter_mut().zip(src) {
            *o = c.to_ascii_uppercase();
        }
        Some(())
    };
    let mut dir = None;
    let s = match s.iter().position(|&c| c == b'/' || c == b'\\') {
        Some(i) => {
            let mut d = [b' '; 11];
            put(&mut d, &s[..i])?;
            dir = Some(fatdir::DIRS.iter().find(|x| **x == d)?);
            &s[i + 1..]
        }
        None => s,
    };
    let dot = s.iter().position(|&c| c == b'.').unwrap_or(s.len());
    let (base, ext) = (&s[..dot], s.get(dot + 1..).unwrap_or(&[]));
    let mut n = [b' '; 11];
    if base.is_empty() {
        return None;
    }
    put(&mut n[..8], base)?;
    put(&mut n[8..], ext)?;
    if !n.iter().all(|&c| c.is_ascii_graphic() || c == b' ') {
        return None;
    }
    // A generated LOGnnn.CSV / SCOPnnn.CSV lives where its .DLG does.
    let mut home = n;
    if home[8..] == *b"CSV" {
        home[8..].copy_from_slice(b"DLG");
    }
    Some((n, dir.map_or(true, |d| fatdir::dir_of(&home) == Some(d))))
}

/// "LOG001  CSV" -> "LOG001.CSV".
#[allow(dead_code)]
pub fn name_str<'a>(n: &[u8; 11], out: &'a mut [u8; 12]) -> &'a str {
    let mut k = 0;
    for &c in n[..8].iter().filter(|&&c| c != b' ') {
        out[k] = c;
        k += 1;
    }
    if n[8] != b' ' {
        out[k] = b'.';
        k += 1;
        for &c in n[8..].iter().filter(|&&c| c != b' ') {
            out[k] = c;
            k += 1;
        }
    }
    core::str::from_utf8(&out[..k]).unwrap_or("?")
}
