//! The protocol decoder's frame log (Mode Log of the scope's Decode tab,
//! DECode:MODE LOG; TODO "Protocol decoders"): View::Decode (ui_log.rs), a
//! list of the frames decoded while the scope input streams, for as long
//! as it runs; DECode:FRAMes? reads it as a FIFO.
//!
//! Memory: the scope's record buffer, idle meanwhile (scope.rs `log_*`):
//! its first LOG_RING samples the stream's ring, above them `Log` (the
//! decoder and the view state) and the frame store, a circular byte log
//! of records (a header, the frame's bytes) that drops the oldest when
//! full. Leaving the view (SCOPE, MENU, Mode Band) ends the log: the store
//! is gone, the scope starts a new record. Nothing in TCM or SRAM but a
//! pointer.
//!
//! The decode runs every main-loop pass (`log_poll`, from scope_poll and
//! between the parts of the view's drawing) for up to PASS_CYC: levels
//! from the signal's min / max (or the manual threshold), the bit rate
//! (set, or auto-baud), then decode.rs `chars_step` / `Framer` over the
//! ring. Region B: it runs while the stream does.

use super::*;
use crate::decode as dc;
use crate::scope_ui::dec as ud;
use crate::scope_ui::log as ul;
use super::app_decode::ds;
use crate::scope_ui::{C_ORANGE, C_WHITE};

/// Rows drawn per main-loop pass (the decoder runs between them).
const ROWS_PER_PASS: usize = 1;
/// Counter cells / top bar refresh, ms.
const STAT_MS: u64 = 250;
/// A pass decodes for up to this long (cycles: 3 ms).
const PASS_CYC: u32 = 3 * crate::clock::HCLK_HZ / 1000;
/// Samples a slice of the decode looks at.
const SLICE: usize = 4096;
/// Auto-baud: characters between the checks for framing errors.
const RECHECK: u16 = 32;
/// Samples for the levels (min / max).
const LEVEL_N: usize = 4096;
/// Auto-baud looks at most this many samples, then starts again.
const AUTO_MAX: usize = 65_536;
const RING: usize = scope::LOG_RING;
/// The decoder stays this far behind the DMA at most (else: samples lost).
const BEHIND_MAX: usize = RING - RING / 8;

/// Record header bytes (store): seq u32 (WRAP: the rest of the store is
/// unused, the next record is at 0), size u16 (all of it, a multiple of
/// 4), len u16 (data bytes), t_us u64 (the start, from the log's start),
/// req u32 (the request's seq, u32::MAX none), dur_us u32, flags u8, kind
/// u8, the waveform snippet: samples u16 (0: none), its rate u32, the
/// frame's start in it u16, its bytes u16. The data bytes follow, then the
/// snippet (`snip_write`).
const HDR: usize = 36;
/// A snippet: at most this many samples (~8 a bit, 2 bits before and
/// after the frame; longer frames: their start).
const SNIP_MAX: usize = 4096;
/// A snippet's bytes at most (its samples stop there).
const SNIP_BYTES: usize = 2048;
/// A snippet is coded only in the first part of a pass (cycles: 0.5 ms):
/// about one a pass.
const SNIP_CYC: u32 = CYC_MS / 2;
const CYC_MS: u32 = crate::clock::HCLK_HZ / 1000;
/// Snippet codes: the 10-bit code / 4 (the waveform's look keeps; the
/// coder sees less of the noise: ~1-2 bits a sample).
const SNIP_SH: u32 = 2;
const WRAP: u32 = u32::MAX;
/// Data bytes kept a frame (Modbus RTU: 256).
const FMAX: usize = 256;

const P_LEVEL: u8 = 0;
const P_BAUD: u8 = 1;
const P_RUN: u8 = 2;

pub(super) struct Log {
    // ---- the stream's decode ----
    /// The stream generation decoded (scope `log_poll`), its rate, the
    /// sample position 0 in stream samples, the stream's start (us from
    /// the log's start).
    gen: u32,
    fs: u32,
    base: u64,
    t0_us: u64,
    /// The log's start (ms clock).
    start_ms: u64,
    phase: u8,
    /// Levels: the min / max of LEVEL_N samples from `mm_a`.
    mm_a: usize,

    lvl: Option<(u16, u16, u16)>,
    /// The bit rate (0: not known), auto-baud's start, "too fast" seen.
    rate: u32,
    baud: dc::Baud,
    baud_a: usize,
    too_fast: bool,
    line: dc::Line,
    cur: dc::Cursor,
    fr: dc::Framer,
    /// The store's seq of the framer's frame 0.
    fseq0: u32,
    /// The bytes of the frame being collected.
    fb: [u8; FMAX],
    fbn: u16,
    /// The ring and the samples written (decode positions), for snippets.
    ring: *const u16,
    avail: usize,
    /// The pass's start (cycles; the snippet budget).
    t_pass: u32,
    /// The frame whose snippet is shown (seq + 1; 0: the list), and the
    /// snippet's decoded samples (SNIP_MAX, in BUF).
    snip: u32,
    scr: *mut u16,
    // ---- the store ----
    sbase: *mut u8,
    cap: u32,
    head: u32,
    tail: u32,
    /// The oldest frame's seq, the next one's, the host's FIFO cursor.
    first: u32,
    next: u32,
    host: u32,
    /// The last lookup (seq, offset).
    lk: (u32, u32),
    /// Characters / framing errors since the last check (auto-baud), and
    /// the rate to measure again.
    recent: (u16, u16),
    remeasure: bool,
    /// Frames, bytes, framing / parity / CRC errors, samples lost (ring
    /// overruns).
    cnt: [u32; 6],
    // ---- the view ----
    paused: bool,
    live: bool,
    top: u32,
    shown: [u32; ul::ROWS],
    note: u8,
    stat_at: u64,
    cshown: [u32; 4],
    top_key: [u32; ul::TOP_ITEMS],
    /// Time zero of the list: the first frame since the last clear.
    base_seq: u32,
    base_us: u64,
    popup: bool,
    pressed: u8,
    drag: Option<(i16, u32)>,
    full: bool,
}

static mut LOG: *mut Log = core::ptr::null_mut();

/// The log (only while `scope.log_on()`).
fn lg() -> &'static mut Log {
    // SAFETY: set by `log_open` into BUF, which stays lent until `log_close`.
    unsafe { &mut *LOG }
}

/// The ring as a decode.rs source: sample i of the decode at `off + i`.
struct RingSrc {
    p: *const u16,
    off: usize,
}

impl crate::pack10::Src for RingSrc {
    fn len(&self) -> usize {
        usize::MAX
    }
    #[inline(always)]
    fn at(&self, i: usize) -> u16 {
        // SAFETY: masked into the ring.
        unsafe { *self.p.add((self.off.wrapping_add(i)) & (RING - 1)) }
    }
}

// ---- the store ----

impl Log {
    fn p(&self, off: u32) -> *mut u8 {
        // SAFETY: off < cap (callers).
        unsafe { self.sbase.add(off as usize) }
    }
    fn r32(&self, off: u32) -> u32 {
        unsafe { core::ptr::read_unaligned(self.p(off) as *const u32) }
    }
    fn r16(&self, off: u32) -> u16 {
        unsafe { core::ptr::read_unaligned(self.p(off) as *const u16) }
    }
    fn r64(&self, off: u32) -> u64 {
        unsafe { core::ptr::read_unaligned(self.p(off) as *const u64) }
    }
    fn w32(&self, off: u32, v: u32) {
        unsafe { core::ptr::write_unaligned(self.p(off) as *mut u32, v) }
    }
    fn w16(&self, off: u32, v: u16) {
        unsafe { core::ptr::write_unaligned(self.p(off) as *mut u16, v) }
    }
    fn w64(&self, off: u32, v: u64) {
        unsafe { core::ptr::write_unaligned(self.p(off) as *mut u64, v) }
    }
    fn count(&self) -> u32 {
        self.next - self.first
    }
    /// The record after the one at `off` (a WRAP or the store's end: 0).
    fn after(&self, off: u32) -> u32 {
        let n = off + self.r16(off + 4) as u32;
        if n + HDR as u32 > self.cap || self.r32(n) == WRAP { 0 } else { n }
    }
    fn drop_oldest(&mut self) {
        if self.count() == 0 {
            return;
        }
        self.first += 1;
        if self.count() == 0 {
            self.head = 0;
            self.tail = 0;
        } else {
            self.head = self.after(self.head);
        }
    }
    /// Room for a record of `size` bytes at the tail (the oldest go).
    fn alloc(&mut self, size: u32) -> Option<u32> {
        if size > self.cap {
            return None;
        }
        if self.count() == 0 {
            self.head = 0;
            self.tail = 0;
        }
        if self.tail + size > self.cap {
            // The records between the tail and the end go, then from 0.
            while self.count() > 0 && self.head >= self.tail {
                self.drop_oldest();
            }
            if self.tail + 4 <= self.cap {
                self.w32(self.tail, WRAP);
            }
            self.tail = 0;
        }
        while self.count() > 0 && self.head >= self.tail && self.head < self.tail + size {
            self.drop_oldest();
        }
        let off = self.tail;
        self.tail += size;
        Some(off)
    }
    /// The record of frame `seq` (None: dropped or not yet there).
    fn find(&mut self, seq: u32) -> Option<u32> {
        if seq < self.first || seq >= self.next {
            return None;
        }
        let (mut s, mut off) = if self.lk.0 >= self.first && self.lk.0 <= seq { self.lk } else { (self.first, self.head) };
        while s < seq {
            off = self.after(off);
            s += 1;
        }
        self.lk = (seq, off);
        Some(off)
    }
    /// Frame `seq`: (header fields as a Fr with the first bytes, t_us,
    /// req seq, dur_us) and up to `out.len()` data bytes into `out` (n).
    fn get(&mut self, seq: u32, out: &mut [u8]) -> Option<(dc::Fr, u64, u32, u32, usize)> {
        let off = self.find(seq)?;
        let len = self.r16(off + 6);
        let n = (len as usize).min(out.len());
        for (k, b) in out[..n].iter_mut().enumerate() {
            // SAFETY: inside the record.
            *b = unsafe { *self.p(off + HDR as u32 + k as u32) };
        }
        let mut head = [0u8; 8];
        for (k, h) in head.iter_mut().enumerate().take((len as usize).min(8)) {
            *h = unsafe { *self.p(off + HDR as u32 + k as u32) };
        }
        let fl = unsafe { *self.p(off + 24) };
        let kind = unsafe { *self.p(off + 25) };
        let f = dc::Fr { start: 0, end: 0, len, head, flags: fl, kind };
        Some((f, self.r64(off + 8), self.r32(off + 16), self.r32(off + 20), n))
    }
}

/// The frame just ended (the framer's out): into the store.
fn store_frame(f: &dc::Fr, req: u32) {
    let l = lg();
    let n = (l.fbn as usize).min(f.len as usize).min(FMAX);
    // Room for SNIP_BYTES of snippet; the record shrinks to what it took
    // (it is the newest: the tail moves back). A
    // snippet only while the pass has time for it (SNIP_CYC): frames that
    // come faster are kept without.
    let snip = crate::prof::cyc().wrapping_sub(l.t_pass) < SNIP_CYC;
    let raw = if snip { SNIP_BYTES } else { 0 };
    let size = ((HDR + n + raw + 3) & !3) as u32;
    let Some(off) = l.alloc(size) else { return };
    let (sn, sfs, sfirst, sbytes) = if snip { snip_write(f, l.p(off + (HDR + n) as u32), raw) } else { (0, 0, 0, 0) };
    let size = ((HDR + n + sbytes as usize + 3) & !3) as u32;
    l.tail = off + size;
    let fs = l.fs.max(1) as u64;
    let t = l.t0_us + (l.base + f.start as u64) * 1_000_000 / fs;
    let dur = ((f.end.saturating_sub(f.start)) as u64 * 1_000_000 / fs) as u32;
    l.w32(off, l.next);
    l.w16(off + 4, size as u16);
    l.w16(off + 6, n as u16);
    l.w64(off + 8, t);
    l.w32(off + 16, if req == u32::MAX { u32::MAX } else { l.fseq0.wrapping_add(req) });
    l.w32(off + 20, dur);
    l.w16(off + 26, sn);
    l.w32(off + 28, sfs);
    l.w16(off + 32, sfirst);
    l.w16(off + 34, sbytes);
    unsafe {
        *l.p(off + 24) = f.flags;
        *l.p(off + 25) = f.kind;
        core::ptr::copy_nonoverlapping(l.fb.as_ptr(), l.p(off + HDR as u32), n);
    }
    l.next += 1;
    l.cnt[0] += 1;
    l.cnt[4] += (f.flags & dc::F_CRC != 0) as u32;
    l.fbn = 0;
}

/// The ring, decimated: snippet sample j = the mean of `d` ring samples
/// from decode position `a + j d`, >> SNIP_SH.
struct DecSrc {
    p: *const u16,
    off: usize,
    a: usize,
    d: usize,
    n: usize,
}

impl crate::pack10::Src for DecSrc {
    fn len(&self) -> usize {
        self.n
    }
    fn at(&self, j: usize) -> u16 {
        let s = self.a + j * self.d;
        let mut sum = 0u32;
        for k in 0..self.d {
            // SAFETY: masked into the ring.
            sum += (unsafe { *self.p.add((self.off.wrapping_add(s + k)) & (RING - 1)) } & 0x3FF) as u32;
        }
        (sum / self.d as u32 >> SNIP_SH) as u16
    }
}

/// Write the snippet of frame `f` (decode positions) after the record's
/// data at `at` (at most `room` bytes): (samples, rate, the frame's start
/// in it, bytes); samples 0 when the ring no longer holds the frame. A
/// snippet that would take more than `room` stops there (fewer samples).
///
/// The code, for logic-like waveforms (DLG's adaptive Rice spends ~6 bits
/// a sample on a square wave): the 8-bit samples' differences, a byte
/// each: 0x00..0x7F a run of 1..128 zero differences, 0x80..0xFE one
/// difference of -64..62, 0xFF then a sample's value (the first, a jump).
fn snip_write(f: &dc::Fr, at: *mut u8, room: usize) -> (u16, u32, u16, u16) {
    let l = lg();
    let spb = (l.line.spb >> 8) as usize;
    let d = ((spb + 4) / 8).max(1);
    let m = 2 * spb;
    let a = (f.start as usize).saturating_sub(m);
    let b = ((f.end as usize) + m).min(l.avail);
    if l.ring.is_null() || b <= a || l.avail - a > RING - RING / 16 {
        return (0, 0, 0, 0);
    }
    let n = ((b - a) / d).min(SNIP_MAX);
    let src = DecSrc { p: l.ring, off: (l.base as usize) & (RING - 1), a, d, n };
    let mut k = 0usize;
    let put = |k: &mut usize, x: u8| {
        // SAFETY: k < room (checked below), inside the record.
        unsafe { *at.add(*k) = x };
        *k += 1;
    };
    let (mut prev, mut run, mut j) = (-1i32, 0u8, 0usize);
    while j < n {
        let v = crate::pack10::Src::at(&src, j) as i32;
        let dv = v - prev;
        // At most 3 bytes for this sample (a run, then a jump).
        if k + 3 > room {
            break;
        }
        if prev >= 0 && dv == 0 {
            run += 1;
            if run == 128 {
                put(&mut k, 127);
                run = 0;
            }
        } else {
            if run > 0 {
                put(&mut k, run - 1);
                run = 0;
            }
            if prev >= 0 && (-64..=62).contains(&dv) {
                put(&mut k, 0x80 | (dv + 64) as u8);
            } else {
                put(&mut k, 0xFF);
                put(&mut k, v as u8);
            }
        }
        prev = v;
        j += 1;
    }
    if run > 0 {
        put(&mut k, run - 1);
    }
    (j as u16, l.fs / d as u32, ((f.start as usize - a) / d) as u16, k as u16)
}

/// Decode a snippet (`nb` bytes, `n` samples; `snip_write`) into `out`.
fn snip_read(bytes: *const u8, nb: usize, n: usize, out: *mut u16) {
    let (mut v, mut j, mut i) = (0i32, 0usize, 0usize);
    // SAFETY: i < nb inside the record; out holds SNIP_MAX >= n.
    let emit = |v: i32, j: &mut usize| {
        if *j < n {
            unsafe { *out.add(*j) = (((v.clamp(0, 255) as u16) << SNIP_SH) | (1 << (SNIP_SH - 1))) & 0x3FF };
            *j += 1;
        }
    };
    while i < nb && j < n {
        let x = unsafe { *bytes.add(i) };
        i += 1;
        match x {
            0x00..=0x7F => {
                for _ in 0..=x {
                    emit(v, &mut j);
                }
            }
            0xFF => {
                v = if i < nb { (unsafe { *bytes.add(i) }) as i32 } else { v };
                i += 1;
                emit(v, &mut j);
            }
            _ => {
                v += (x & 0x7F) as i32 - 64;
                emit(v, &mut j);
            }
        }
    }
}

/// A character decoded: the counters, the frame's bytes, the framer.
fn on_char(c: dc::Ch) -> bool {
    let l = lg();
    l.cnt[1] += 1;
    l.cnt[2] += (c.err & dc::F_FE != 0) as u32;
    l.cnt[3] += (c.err & dc::F_PE != 0) as u32;
    // Auto-baud: framing errors in a quarter of RECHECK characters, the
    // rate is measured again (the line changed its rate).
    l.recent.0 += 1;
    l.recent.1 += (c.err & dc::F_FE != 0) as u16;
    if l.recent.0 >= RECHECK {
        l.remeasure |= l.recent.1 * 4 >= RECHECK && ds().cfg.baud == 0;
        l.recent = (0, 0);
    }
    // A frame the character ends is stored before it joins the next.
    l.fr.push(c, &mut |f, req| store_frame(f, req));
    let l = lg();
    if (l.fbn as usize) < FMAX {
        l.fb[l.fbn as usize] = c.v as u8;
        l.fbn += 1;
    }
    true
}

/// Stream rate for a bit rate (0: auto, the highest): ~16 samples a bit.
fn fs_for(baud: u32) -> u32 {
    if baud == 0 { scope::LOG_FS_MAX } else { (baud * 16).clamp(50_000, scope::LOG_FS_MAX) }
}

impl App {
    /// Mode Log: open the log view and start the stream. Err: the SCPI
    /// error (and a toast).
    pub(super) fn log_open(&mut self, clk: &mut Clock) -> Result<(), &'static str> {
        let now = clk.now_ms();
        let why = if self.scope.wave_on() {
            Some("Decode log: MCX input only")
        } else if self.scope.held() {
            Some("Decode log: scope record in use")
        } else if self.view == View::Inrush {
            Some("Decode log: inrush page open")
        } else if crate::decode::is_dali(ds().cfg.proto) {
            // The log decodes UART / Modbus only.
            Some("Decode log: no DALI")
        } else if crate::hy3131::streaming() {
            // An AD2 stream (LOG:STAT ON, a fast-log session): the view's
            // page / touch code is in region C (link-native.x).
            Some("Decode log: DMM stream on")
        } else {
            None
        };
        if let Some(t) = why {
            self.toast(now, t, false);
            return Err("Settings conflict");
        }
        ds().mode = ud::MODE_LOG;
        if !self.scope.log_on() {
            // A band decode in flight: its record goes with BUF.
            self.dec_abort();
            let fs = self.scope.log_begin(fs_for(ds().cfg.baud), now);
            let (p, n) = self.scope.log_mem();
            let a = p.align_offset(8);
            // SAFETY: BUF is lent to the log until `log_close`; Log fits.
            unsafe {
                let lp = p.add(a) as *mut Log;
                core::ptr::write_bytes(lp as *mut u8, 0, core::mem::size_of::<Log>());
                LOG = lp;
            }
            let used = (a + core::mem::size_of::<Log>() + 3) & !3;
            let l = lg();
            l.scr = unsafe { p.add(used) as *mut u16 };
            let used = used + SNIP_MAX * 2;
            l.sbase = unsafe { p.add(used) };
            l.cap = ((n - used) & !3) as u32;
            l.lk = (u32::MAX, 0);
            l.live = true;
            l.drag = None;
            l.lvl = None;
            l.base_us = u64::MAX;
            l.gen = u32::MAX;
            l.fs = fs;
            l.start_ms = now;
            l.cur = dc::Cursor::at(0);
            l.fr = dc::Framer::new(false, 1, fs, 9600, 0, u32::MAX);
            self.log_reset_decode();
        }
        if self.view != View::Decode {
            self.show(View::Decode, clk);
        }
        Ok(())
    }

    /// Leaving the view: the log ends (its store with it), the scope gets
    /// BUF back and starts a new record; Mode Band.
    pub(super) fn log_close(&mut self) {
        if self.scope.log_on() {
            self.scope.log_end();
            unsafe { LOG = core::ptr::null_mut() };
        }
        ds().mode = ud::MODE_BAND;
    }

    /// Settings changed: decode again from the levels (manual threshold /
    /// set rate: at once); the list stays.
    pub(super) fn log_reset_decode(&mut self) {
        if !self.scope.log_on() {
            return;
        }
        let l = lg();
        l.lvl = None;
        l.rate = ds().cfg.baud;
        l.too_fast = false;
        // A new generation: `log_poll` starts over (at the rate for the
        // set bit rate, or the highest for auto).
        let now = self.now;
        if !l.paused {
            l.fs = self.scope.log_begin(fs_for(ds().cfg.baud), now);
        }
        l.gen = u32::MAX;
        l.top_key = [u32::MAX; ul::TOP_ITEMS];
        l.note = u8::MAX;
    }

    /// Decode what the stream wrote (every main-loop pass, and between the
    /// view's drawing parts).
    #[inline(never)]
    pub(super) fn log_poll(&mut self, now: u64) {
        if !self.scope.log_on() {
            return;
        }
        let Some((wr, gen, laps)) = self.scope.log_poll(now) else { return };
        let (ring, fs) = self.scope.log_ring();
        let l = lg();
        if gen != l.gen {
            // A new stream (the start, a settings change, RUN): from its
            // sample 0, the time on from now.
            l.gen = gen;
            l.fs = fs;
            l.base = 0;
            l.t0_us = now.saturating_sub(l.start_ms) * 1000;
            // The levels again: a restart may follow a V/div change.
            l.lvl = None;

            l.mm_a = 0;
            l.phase = if l.lvl.is_none() { P_LEVEL } else { P_BAUD };
            self.log_restart_at(0);
            return;
        }
        let l = lg();
        let avail = (wr - l.base) as usize;
        let pos = match l.phase {
            P_LEVEL => l.mm_a,
            P_BAUD => l.baud.n,
            _ => l.cur.n,
        };
        if laps > 0 || avail.saturating_sub(pos) > BEHIND_MAX {
            // The decoder fell behind the DMA: what it lost is counted, it
            // goes on half a ring behind.
            l.cnt[5] = l.cnt[5].saturating_add(1);
            let p = avail - RING / 2;
            l.mm_a = p;
            self.log_restart_at(p);
        }
        let src = RingSrc { p: ring.as_ptr(), off: (lg().base as usize) & (RING - 1) };
        let l = lg();
        l.ring = ring.as_ptr();
        l.avail = avail;
        l.t_pass = crate::prof::cyc();

        let t0 = crate::prof::cyc();
        crate::prof::enter(crate::prof::X2);
        while crate::prof::cyc().wrapping_sub(t0) < PASS_CYC {
            let l = lg();
            match l.phase {
                P_LEVEL => {
                    if avail < l.mm_a + LEVEL_N {
                        break; // more samples next pass
                    }
                    let e = l.mm_a + LEVEL_N;
                    let mm = dc::min_max(&src, l.mm_a, e);
                    l.mm_a = e;
                    match dc::levels(mm.0, mm.1, self.dec_thr_code()) {
                        Some(v) => {
                            let l = lg();
                            l.lvl = Some(v);
                            l.phase = P_BAUD;
                            l.top_key = [u32::MAX; ul::TOP_ITEMS];
                            self.log_restart_at(e.saturating_sub(LEVEL_N));
                        }
                        None => continue, // no swing yet: the next samples
                    }
                }
                P_BAUD => {
                    if l.rate != 0 {
                        l.phase = P_RUN;
                        let a = l.baud_a;
                        self.log_set_line(a);
                        continue;
                    }
                    let end = avail.min(l.baud_a + AUTO_MAX);
                    l.baud.step(&src, end.min(l.baud.n + SLICE));
                    if !l.baud.enough() && l.baud.n < end {
                        if l.baud.n >= avail {
                            break;
                        }
                        continue;
                    }
                    match l.baud.result(l.fs) {
                        Ok(b) => {
                            l.rate = b;
                            l.too_fast = false;
                            l.top_key = [u32::MAX; ul::TOP_ITEMS];
                            l.phase = P_RUN;
                            if fs_for(b) < l.fs {
                                // A slower stream for this rate (a longer
                                // ring in time): it starts again there.
                                l.fs = self.scope.log_begin(fs_for(b), now);
                                break;
                            }
                            let a = l.baud_a;
                            self.log_set_line(a);
                        }
                        Err(e) => {
                            // Too few edges / too fast: again from here.
                            l.too_fast = e == dc::NoRate::TooFast;
                            let n = l.baud.n;
                            self.log_restart_at(n);
                            if n >= avail {
                                break;
                            }
                        }
                    }
                }
                _ => {
                    let lim = avail.min(l.cur.n + SLICE);
                    let mut cur = l.cur;
                    let line = l.line;
                    dc::chars_step(&src, 0, avail, lim, &line, &mut cur, &mut on_char);
                    let l = lg();
                    // The window's end is the stream's present: go on.
                    cur.done = false;
                    l.cur = cur;
                    l.fr.idle(avail as u32, &mut |f, req| store_frame(f, req));
                    if l.remeasure {
                        // The levels and the rate again (at the stream's
                        // highest rate).
                        l.remeasure = false;
                        l.recent = (0, 0);
                        self.log_reset_decode();
                        break;
                    }
                    self.log_rebase();
                    if lg().cur.n + 2 >= avail || lim >= avail {
                        break;
                    }
                }
            }
        }
        crate::prof::exit(crate::prof::X2);
    }

    /// Levels known: auto-baud (or the set rate) from sample `p`.
    fn log_restart_at(&mut self, p: usize) {
        let (ring, _) = self.scope.log_ring();
        let l = lg();
        let src = RingSrc { p: ring.as_ptr(), off: (l.base as usize) & (RING - 1) };
        if let Some((lo, _, hi)) = l.lvl {
            l.baud = dc::Baud::new(&src, p, lo, hi);
        }
        l.baud_a = p;
        if l.phase == P_RUN {
            self.log_set_line(p);
        }
    }

    /// The rate known: the line and a new framer, the decode from `p`.
    fn log_set_line(&mut self, p: usize) {
        let l = lg();
        let s = ds();
        let Some((lo, mid, hi)) = l.lvl else { return };
        let (bits, parity) = s.cfg.bits();
        l.line = dc::Line { lo, mid, hi, inv: s.cfg.inverted, spb: (l.fs as u64 * 256 / l.rate.max(1) as u64) as u32, bits, parity };
        l.cur = dc::Cursor::at(p);
        let cs = l.line.char_samples();
        l.fr = dc::Framer::new(s.cfg.proto == dc::P_MODBUS, cs, l.fs, l.rate, p as u32, u32::MAX);
        l.fseq0 = l.next;
        l.fbn = 0;
        if l.line.spb < dc::MIN_SPB << 8 {
            // Above the stream's rate: back to auto-baud (the top bar says).
            l.too_fast = true;
            l.rate = 0;
            l.phase = P_BAUD;
        }
    }

    /// Positions are u32 in the decode: move the numbering on before they
    /// grow large (between frames).
    fn log_rebase(&mut self) {
        let l = lg();
        if l.cur.n < 1 << 30 || l.fr.open_len() != 0 {
            return;
        }
        let d = l.cur.n & !(RING - 1);
        l.base += d as u64;
        l.cur.n -= d;
        l.fr.shift(d as u32);
        l.baud_a = l.baud_a.saturating_sub(d);
    }

    // ---- drawing ----

    /// The whole view (draw_static).
    #[inline(never)]
    #[cfg_attr(target_os = "none", link_section = ".text.log_cold")]
    pub(super) fn log_page(&mut self) {
        let now = self.now;
        ul::chrome(&mut || self.log_poll(now));
        if !self.scope.log_on() {
            return;
        }
        let l = lg();
        ul::panel(l.live, l.paused, &mut || self.log_poll(now));
        let l = lg();
        l.shown = [u32::MAX; ul::ROWS];
        l.note = u8::MAX;
        l.top_key = [u32::MAX; ul::TOP_ITEMS];
        l.cshown = [u32::MAX; 4];
        l.stat_at = 0;
        l.full = false;
        if l.popup {
            self.log_setup_draw();
        } else if l.snip != 0 {
            let s = l.snip - 1;
            if !self.log_snip_show(s) {
                lg().snip = 0;
            }
        }
    }

    /// One pass of the view: the top bar / counters when they changed (at
    /// most every STAT_MS), then up to ROWS_PER_PASS changed rows.
    #[inline(never)]
    pub(super) fn log_tick(&mut self, now: u64) {
        if !self.scope.log_on() {
            return;
        }
        let l = lg();
        if l.full {
            self.redraw = true;
            return;
        }
        if l.popup {
            return;
        }
        if l.snip != 0 {
            // A frame's waveform: the counters only.
            if now >= l.stat_at {
                if !self.log_stats() {
                    lg().stat_at = now + STAT_MS;
                }
            }
            return;
        }
        if now >= l.stat_at {
            crate::prof::enter(crate::prof::STAT);
            let drew = self.log_stats();
            crate::prof::exit(crate::prof::STAT);
            if drew {
                self.log_poll(now);
                return;
            }
            lg().stat_at = now + STAT_MS;
        }
        let l = lg();
        // Time zero: the first frame since the clear.
        if l.base_us == u64::MAX && l.next > l.base_seq {
            let s = l.base_seq.max(l.first);
            if let Some((_, t, ..)) = l.get(s, &mut []) {
                l.base_us = t;
            }
        }
        let n = ul::ROWS as u32;
        // LIVE: the list moves by half a page when the newest frame passes
        // its end (a burst: to the newest page), so a new frame costs one
        // row, not a redraw of every row.
        if l.live && l.next > l.top + n {
            l.top = (l.top + n / 2).max(l.next.saturating_sub(n));
        }
        let top = l.top.clamp(l.first, l.next.saturating_sub(1).max(l.first));
        l.top = top;
        if l.next == l.first {
            // An empty list: one note.
            let k = if l.paused {
                1
            } else if l.lvl.is_none() {
                2
            } else if l.rate == 0 && l.too_fast {
                5
            } else if l.rate == 0 {
                3
            } else {
                4
            };
            if l.note != k {
                l.note = k;
                let t = ["", "Stopped", "Waiting for a signal (no swing)", "Measuring the bit rate", "No frames yet", "Bit rate too high for 1.25 MS/s"][k as usize];
                ul::row(0, Some(&ul::Row { time: "", text: t, ascii: "", status: "", fg: 0 }));
                l.shown[0] = 0;
                for i in 1..ul::ROWS {
                    if l.shown[i] != 0 {
                        ul::row(i, None);
                        l.shown[i] = 0;
                    }
                }
            }
            return;
        }
        l.note = 0;
        let mut drawn = 0;
        for i in 0..ul::ROWS {
            let l = lg();
            let seq = top + i as u32;
            let want = if seq < l.next { seq + 1 } else { 0 };
            if l.shown[i] == want {
                continue;
            }
            l.shown[i] = want;
            if want == 0 {
                ul::row(i, None);
            } else {
                self.log_row(i, seq);
            }
            drawn += 1;
            self.log_poll(now);
            if drawn >= ROWS_PER_PASS {
                break;
            }
        }
    }

    /// List row i: frame `seq` (time since the list's zero, UART hex +
    /// ASCII / the Modbus fields and the reply time, the status).
    fn log_row(&mut self, i: usize, seq: u32) {
        let l = lg();
        let mut d = [0u8; 24];
        let Some((f, t_us, req, _, n)) = l.get(seq, &mut d) else {
            ul::row(i, None);
            return;
        };
        let mut tb = [0u8; 16];
        let mut w = dc::W { b: &mut tb, n: 0 };
        let rel = t_us.saturating_sub(if l.base_us == u64::MAX { t_us } else { l.base_us });
        secs(&mut w, rel);
        let tn = w.n;
        let mut cb = [0u8; 64];
        let mut c = dc::W { b: &mut cb, n: 0 };
        let mut ab = [0u8; 16];
        let mut an = 0;
        if f.kind == dc::K_UART || f.kind == dc::K_MB {
            // UART: hex + ASCII; a Modbus frame without fields: its hex.
            for (k, &b) in d[..n].iter().enumerate() {
                if k > 0 {
                    c.c(b' ');
                }
                c.hex(b);
                if f.kind == dc::K_UART && k < ab.len() {
                    ab[k] = if (0x20..0x7F).contains(&b) { b } else { b'.' };
                    an = k + 1;
                }
            }
        } else {
            dc::mb_text(&mut c, &f);
            // A reply: the time since its request ended.
            if req != u32::MAX {
                if let Some((_, qt, _, qd, _)) = l.get(req, &mut []) {
                    let us = t_us.saturating_sub(qt + qd as u64);
                    c.s(" ").u((us / 1000) as u32).c(b'.').u((us / 100 % 10) as u32).s("ms");
                }
            }
            // A read reply: its registers / coil bytes.
            if f.kind == dc::K_RSP && (1..=4).contains(&d[1]) {
                let words = d[1] >= 3;
                let end = n.min((f.len as usize).saturating_sub(2));
                let mut k = 3;
                while k < end {
                    c.c(b' ');
                    if words && k + 1 < end {
                        c.hex(d[k]).hex(d[k + 1]);
                        k += 2;
                    } else {
                        c.hex(d[k]);
                        k += 1;
                    }
                }
            }
        }
        let cn = c.n;
        let uart = f.kind == dc::K_UART;
        let mut fb = [0u8; 64];
        let fnn = ul::fit(ul::txt(&cb[..cn]), ul::text_w(uart), if uart { 0 } else { 1 }, &mut fb);
        let fg = if f.kind == dc::K_EXC { C_ORANGE } else { C_WHITE };
        let mut af = [0u8; 64];
        let afn = ul::fit_ascii(ul::txt(&ab[..an]), &mut af);
        crate::prof::enter(crate::prof::LABEL);
        ul::row(i, Some(&ul::Row { time: ul::txt(&tb[..tn]), text: ul::txt(&fb[..fnn]), ascii: ul::txt(&af[..afn]), status: dc::status(f.flags), fg }));

        crate::prof::exit(crate::prof::LABEL);
    }

    /// The first top-bar item or counter cell that changed, drawn (one
    /// paint per pass: a badge is a few ms); false when none changed.
    /// Region C (link-native.x): at most every STAT_MS, while only the
    /// log's DMA streams.
    #[inline(never)]
    #[cfg_attr(target_os = "none", link_section = ".text.log_cold")]
    fn log_stats(&mut self) -> bool {

        let l = lg();
        let s = ds();
        let mb = s.cfg.proto == dc::P_MODBUS;
        let state = if l.paused {
            2
        } else if l.lvl.is_none() || l.rate == 0 {
            0
        } else {
            1
        };
        let mut rb = [0u8; 24];
        let mut r = dc::W { b: &mut rb, n: 0 };
        if s.cfg.baud == 0 {
            r.s("Auto ");
        }
        if l.rate != 0 {
            r.u(l.rate).c(b' ');
        }
        r.s(dc::FMT_NAMES[s.cfg.fmt as usize % 4]);
        let rn = r.n;
        let mut hb = [0u8; 16];
        let hn = self.log_thr_text(&mut hb);
        let t = ul::Top {
            state,
            proto: if mb { "MODBUS" } else { "UART" },
            rate: ul::txt(&rb[..rn]),
            thr: ul::txt(&hb[..hn]),
            inv: s.cfg.inverted,
            battery: self.power.bars.min(5),
            charging: self.power.charging(),
        };
        let l = lg();
        for k in 0..ul::TOP_ITEMS {
            let key = ul::top_key(k, &t);
            if l.top_key[k] != key {
                l.top_key[k] = key;
                ul::top_item(k, &t);
                return true;
            }
        }
        let c = l.cnt;
        let v = [c[0], c[2], c[3], c[4], c[5], mb as u32];
        for i in 0..4 {
            let key = ul::cell_key(i, &v);
            if l.cshown[i] != key {
                l.cshown[i] = key;
                ul::cell_draw(i, &v);
                return true;
            }
        }
        false
    }

    /// The threshold text: "1.65V" set, "A 1.65V" auto with the level in
    /// use, "A --" before one is known.
    fn log_thr_text(&self, b: &mut [u8; 16]) -> usize {
        let s = ds();
        let v = if s.thr_v.is_finite() { Some(s.thr_v) } else { self.log_level() };
        let mut w = dc::W { b, n: 0 };
        if !s.thr_v.is_finite() {
            w.s("A ");
        }
        match v {
            Some(v) => {
                let mut t = [0u8; 16];
                w.s(crate::dmm::format(&mut t, v, 2)).s("V");
            }
            None => {
                w.s("--");
            }
        }
        w.n
    }

    /// The level in use, V.
    pub(super) fn log_level(&self) -> Option<f32> {
        if !self.scope.log_on() {
            return None;
        }
        lg().lvl.map(|t| self.scope.volts(t.1 as f32))
    }

    /// Show frame `seq`'s waveform in the list area: its row on top, the
    /// trace, the bus band under it (ui_decode's band, from the snippet
    /// decoded again). False: no snippet (the ring had moved on). Region C
    /// (link-native.x): a tap's one-off drawing.
    #[inline(never)]
    #[cfg_attr(target_os = "none", link_section = ".text.log_cold")]
    fn log_snip_show(&mut self, seq: u32) -> bool {

        let l = lg();
        let Some(off) = l.find(seq) else { return false };
        let n = l.r16(off + 26) as usize;
        if n < 2 || l.lvl.is_none() {
            return false;
        }
        let len = l.r16(off + 6) as usize;
        let fs = l.r32(off + 28);
        let first = l.r16(off + 32) as u32;
        let nb = l.r16(off + 34) as usize;
        snip_read(l.p(off + (HDR + len) as u32), nb, n, l.scr);
        // SAFETY: scr holds SNIP_MAX samples; n <= SNIP_MAX.
        let s: &[u16] = unsafe { core::slice::from_raw_parts(l.scr, n) };
        // The frame's row on top, the trace, then the band.
        let i0 = l.shown;
        self.log_row(0, seq);
        let l = lg();
        l.shown = i0;
        l.shown[0] = u32::MAX;
        ul::snippet(s);
        let a = ud::ann();
        a.reset(0, ud::C_DONE);
        a.mode = ud::M_BOXES;
        a.off = 0;
        a.n = n as u32;
        a.fs = fs;
        a.t0 = first;
        let st = ds();
        let (lo, mid, hi) = l.lvl.unwrap_or((0, 512, 1023));
        let (bits, parity) = st.cfg.bits();
        let line = dc::Line { lo, mid, hi, inv: st.cfg.inverted, spb: (fs as u64 * 256 / l.rate.max(1) as u64) as u32, bits, parity };
        dc::chars(&s, 0, n, &line, &mut |c| {
            let a = ud::ann();
            if (a.nb as usize) < ud::NB {
                a.b[a.nb as usize] = ud::BAnn { s0: c.start, s1: c.end, v: c.v as u8, fl: c.err };
                a.nb += 1;
            }
        });
        if st.cfg.proto == dc::P_MODBUS {
            // The frame box: the frame as the log decoded it (the snippet
            // alone shows no gap before it).
            let l = lg();
            if let Some((f, _, _, dur, _)) = l.get(seq, &mut []) {
                let s1 = first + (dur as u64 * fs as u64 / 1_000_000) as u32;
                let a = ud::ann();
                a.f[0] = ud::FAnn { s0: first, s1, kind: f.kind, fl: f.flags, len: f.len, head: f.head, req: 0xFF };
                a.nf = 1;
            }
        }
        ud::band(self.dec_band(), false);

        // Not a scope record's: the band's cache is for no record.
        ud::ann().gen = 0;
        true
    }

    /// Back from a waveform to the list.
    #[inline(never)]
    #[cfg_attr(target_os = "none", link_section = ".text.log_cold")]
    fn log_snip_hide(&mut self) {
        let l = lg();
        l.snip = 0;
        l.shown = [u32::MAX; ul::ROWS];
        l.note = u8::MAX;
        crate::lcd::fill_rect(14, 21, 399, 279, crate::scope_ui::C_FILL);
    }

    /// The bit rate in use (0: none yet).

    pub(super) fn log_rate(&self) -> u32 {
        if self.scope.log_on() { lg().rate } else { 0 }
    }

    // ---- setup popup ----

    #[inline(never)]
    #[cfg_attr(target_os = "none", link_section = ".text.log_cold")]
    fn log_setup_draw(&mut self) {
        let v = self.dec_setup_vals();
        ul::setup(&v);
    }

    // ---- input ----

    /// Touches and keys of the view: Some(captured) when handled.
    #[inline(never)]
    #[cfg_attr(target_os = "none", link_section = ".text.log_cold")]
    pub(super) fn log_gesture(&mut self, clk: &mut Clock, g: Gesture) -> Option<bool> {
        let now = clk.now_ms();
        if !self.scope.log_on() {
            return None;
        }
        let l = lg();
        match g {
            Gesture::KeyDown(k) => {
                self.beep(now, 50);
                match k {
                    0 if l.popup => {
                        l.popup = false;
                        self.redraw = true;
                    }
                    0 => self.show(View::Scope, clk),
                    1 => self.log_pause(now),
                    3 => {
                        l.popup = !l.popup;
                        self.redraw = true;
                    }
                    _ => {}
                }
                Some(false)
            }
            Gesture::Down(p) if !l.popup => {
                if let Some(k) = ul::panel_hit(p.x, p.y) {
                    l.pressed = k as u8 + 1;
                    ul::panel_button(k, l.live, l.paused, true);
                } else if ul::LIST.has(p.x as i16, p.y as i16) {
                    l.drag = Some((p.y as i16, l.top));
                }
                Some(false)
            }
            Gesture::Drag(_, p, _) | Gesture::Move(_, p) if !l.popup => {
                if let Some((y0, t0)) = l.drag {
                    let rows = (y0 - p.y as i16) / ul::ROW_H;
                    let t = (t0 as i64 + rows as i64).clamp(l.first as i64, l.next.saturating_sub(1).max(l.first) as i64) as u32;
                    if rows != 0 || !l.live {
                        let live = t + ul::ROWS as u32 >= l.next && rows > 0;
                        self.log_scroll(t, live);
                    }
                }
                Some(true)
            }
            Gesture::Up(_) | Gesture::Cancel => {
                l.drag = None;
                self.log_unpress();
                Some(false)
            }
            Gesture::Tap(p) => {
                self.beep(now, 30);
                self.log_unpress();
                let l = lg();
                if l.popup {
                    match ul::setup_hit(p.x as i16, p.y as i16) {
                        None => {
                            l.popup = false;
                            self.redraw = true;
                        }
                        Some(h) => self.dec_setup_tap(clk, h, now),
                    }
                    return Some(false);
                }
                if ul::LIST.has(p.x as i16, p.y as i16) {
                    // A row: its waveform; the waveform: back to the list.
                    if l.snip != 0 {
                        self.log_snip_hide();
                    } else if let Some(i) = ul::row_at(p.y as i16) {
                        let seq = l.top + i as u32;
                        if seq < l.next && self.log_snip_show(seq) {
                            lg().snip = seq + 1;
                        }
                    }
                    return Some(false);
                }
                match ul::panel_hit(p.x, p.y) {
                    Some(0) => self.log_page_step(false),
                    Some(1) => self.log_scroll(0, true),
                    Some(2) => self.log_page_step(true),
                    Some(3) => {
                        l.popup = true;
                        self.redraw = true;
                    }
                    Some(4) => self.log_clear(),
                    Some(5) => self.log_pause(now),
                    Some(6) => self.show(View::Scope, clk),
                    _ => {}
                }
                Some(false)
            }
            Gesture::MultiSwipeDown(_) => None,
            _ => Some(false),
        }
    }

    /// The setup popup changed a setting (app_decode `dec_setup_tap`).
    #[inline(never)]
    #[cfg_attr(target_os = "none", link_section = ".text.log_cold")]
    pub(super) fn log_setup_changed(&mut self) {
        if self.view == View::Decode && self.scope.log_on() {
            self.log_reset_decode();
            if lg().popup && !self.redraw {
                self.log_setup_draw();
            }
        }
    }

    #[inline(never)]
    #[cfg_attr(target_os = "none", link_section = ".text.log_cold")]
    fn log_unpress(&mut self) {
        let l = lg();
        if l.pressed != 0 {
            let k = l.pressed as usize - 1;
            l.pressed = 0;
            if !l.popup {
                ul::panel_button(k, l.live, l.paused, false);
            }
        }
    }

    fn log_scroll(&mut self, top: u32, live: bool) {
        let l = lg();
        if live != l.live {
            l.live = live;
            ul::panel_button(1, live, l.paused, false);
        }
        l.top = top;
    }

    #[inline(never)]
    #[cfg_attr(target_os = "none", link_section = ".text.log_cold")]
    fn log_page_step(&mut self, down: bool) {
        let l = lg();
        let n = ul::ROWS as u32;
        let cur = l.top;
        let t = if down { cur + n - 1 } else { cur.saturating_sub(n - 1).max(l.first) };
        let live = down && t + n >= l.next;
        self.log_scroll(t.min(l.next.saturating_sub(1).max(l.first)), live);
    }

    /// CLEAR / DECode:CLEar: the list and the counters start again.
    pub(super) fn log_clear(&mut self) {
        if !self.scope.log_on() {
            return;
        }
        let l = lg();
        l.first = l.next;
        l.host = l.next;
        l.head = 0;
        l.tail = 0;
        l.cnt = [0; 6];
        l.base_seq = l.next;
        l.base_us = u64::MAX;
        l.top = l.next;
        l.shown = [u32::MAX; ul::ROWS];
        l.note = u8::MAX;
        l.cshown = [u32::MAX; 4];
        // The framer's request link points into the store: a new one.
        l.fseq0 = l.next.wrapping_sub(l.fr.nframes);
        if !l.live {
            let n = l.next;
            self.log_scroll(n, true);
        }
    }

    #[inline(never)]
    #[cfg_attr(target_os = "none", link_section = ".text.log_cold")]
    fn log_pause(&mut self, now: u64) {
        let l = lg();
        l.paused = !l.paused;
        if l.paused {
            self.scope.log_halt();
        } else {
            self.scope.log_resume(now);
        }
        let l = lg();
        ul::panel_button(5, l.live, l.paused, false);
        l.top_key = [u32::MAX; ul::TOP_ITEMS];
        l.note = u8::MAX;
    }

    // ---- SCPI ----

    /// DECode:COUNt? in Log mode: frames, bytes, framing, parity, CRC,
    /// lost.
    pub(super) fn log_count(&self) -> [u32; 6] {
        if self.scope.log_on() { lg().cnt } else { [0; 6] }
    }

    /// DECode:FRAMes? [<n>] in Log mode: the host's unread frames, oldest
    /// first, at most `max` and as many whole lines as fit `b`; the host
    /// cursor moves past them. Returns the bytes written.
    pub(super) fn log_lines(&mut self, max: u32, b: &mut [u8]) -> usize {
        if !self.scope.log_on() {
            return 0;
        }
        let modbus = ds().cfg.proto == dc::P_MODBUS;
        let mut w = dc::W { b, n: 0 };
        let mut k = 0;
        let l = lg();
        l.host = l.host.max(l.first);
        let mut d = [0u8; FMAX];
        while l.host < l.next && k < max {
            let Some((f, t_us, req, _, n)) = l.get(l.host, &mut d) else { break };
            let n0 = w.n;
            w.u(l.host).c(b',');
            w.u((t_us / 1_000_000) as u32).c(b'.');
            let mut fr = t_us % 1_000_000;
            let mut dd = 100_000;
            for _ in 0..6 {
                w.c(b'0' + (fr / dd) as u8);
                fr %= dd;
                dd /= 10;
            }
            w.s(if modbus { ",MB," } else { ",UART," });
            for &x in &d[..n] {
                w.hex(x);
            }
            w.c(b',').s(dc::status(f.flags)).c(b',');
            if modbus {
                dc::mb_text(&mut w, &f);
                if req != u32::MAX {
                    w.s(" req=").u(req);
                }
            }
            w.c(b'\n');
            if w.n >= w.b.len() {
                // Did not fit: the next reply.
                w.n = n0;
                break;
            }
            l.host += 1;
            k += 1;
        }
        w.n
    }
}

/// "12.3456" seconds from us (4 decimals below 100 s, 2 below 10 000 s).
fn secs(w: &mut dc::W, us: u64) {
    let dec = if us < 100_000_000 { 4 } else if us < 10_000_000_000 { 2 } else { 0 };
    w.u((us / 1_000_000) as u32);
    if dec > 0 {
        w.c(b'.');
        let mut f = us % 1_000_000;
        let mut d = 100_000;
        for _ in 0..dec {
            w.c(b'0' + (f / d) as u8);
            f %= d;
            d /= 10;
        }
    }
}
