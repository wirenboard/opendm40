//! The protocol decoder on the scope page (TODO "Protocol decoders"): a
//! stored record is decoded once, the bytes and frames kept (ui_decode.rs
//! `ANN`, positions in record samples), and every frame the page draws
//! (a running frame, a stopped record's zoom / pan) shows the screen's part
//! of them as the bus band. Settings: MF > Settings > Decode (`Popup::Dec`)
//! or DECode:...; DECode:FRAMes? lists the kept frames.
//!
//! Which records: running, one that triggered (Normal / Single, Auto when
//! the trigger fired), not a roll record and not the wave view's; stopped,
//! the stored record. The decode (`Job`) runs in the main loop
//! (`dec_step`, from `scope_poll`), `SLICE` samples a pass, never in the
//! frame draw; while it runs on a running scope the next acquisition waits
//! (`Scope::hold_restart`: it would overwrite the record). More bytes /
//! frames than the lists hold: stopped, the screen's window is decoded
//! again when a pan / zoom leaves the part kept.

use super::*;
use crate::decode as dc;
use crate::scope_ui::dec as ud;

/// Auto-baud: records decoded with the rate found last before it is
/// measured again.
const REUSE: u8 = 8;

/// A decode ends after this many passes whatever it holds.
const MAX_PASSES: u16 = 500;

/// Threshold stepper, V.

const THR_STEP: f32 = 0.1;
/// Auto-baud looks at most this many samples.
const AUTO_MAX: usize = 65_536;
/// Samples a slice looks at (the character scan; auto-baud half).
const SLICE: usize = 4096;
/// A pass decodes slices for up to this long (cycles: 3 ms): a decode
/// spread one slice a pass would hold the next acquisition for as many
/// passes (each with its paint).
const PASS_CYC: u32 = 3 * crate::clock::HCLK_HZ / 1000;

pub(super) struct DecState {
    pub cfg: dc::Cfg,
    /// Manual threshold, V at the probe tip (NaN: auto).
    pub thr_v: f32,
    /// ud::MODE_BAND / MODE_LOG.
    pub mode: u8,
    /// Records left that take the rate found last without measuring.
    reuse: u8,

    /// Changes with what the band shows (Model::dec_gen; 0 = blank).
    gen: u16,
}

static mut DS: DecState = unsafe { core::mem::zeroed() };

pub(super) fn ds() -> &'static mut DecState {
    unsafe { &mut *core::ptr::addr_of_mut!(DS) }
}

/// Boot: the non-zero defaults.
pub(super) fn init() {
    let s = ds();
    s.cfg = dc::Cfg::DEFAULT;
    s.thr_v = f32::NAN;
    s.gen = 1;
    unsafe { (*core::ptr::addr_of_mut!(JOB)).write(None) };
}

/// A record decode in progress.
struct Job {
    /// The samples to decode, the window it must cover (the screen, to
    /// decode again when the lists fill up before it).
    a: usize,
    b: usize,
    win: (usize, usize),
    /// Auto-baud in progress (None: the rate is known), from `auto0`.
    auto: Option<dc::Baud>,
    auto0: usize,
    line: dc::Line,
    cur: dc::Cursor,
    fr: dc::Framer,
    /// The frame number of each kept frame (the reply -> request link).
    fno: [u32; ud::NF],
    /// Decoding the screen's window (not the whole record).
    windowed: bool,
    /// Passes so far (a guard: the next acquisition waits for it).
    passes: u16,
}

/// The job's DALI scan (`Job::cur` follows its sample). Not in `Job`: no
/// bytes without `dec-dali` (every use is under `dc::is_dali`).
static mut DL: dc::Dali = dc::Dali::at(0);

fn dl() -> &'static mut dc::Dali {
    unsafe { &mut *core::ptr::addr_of_mut!(DL) }
}

// Zero bytes until `init` writes None (Option<Job>'s None is not all-zero:
// a `.data` image in region B otherwise).
static mut JOB: core::mem::MaybeUninit<Option<Job>> = core::mem::MaybeUninit::zeroed();
/// Cycles the decode in progress took so far (prof DJOB).
static mut JOB_CYC: u32 = 0;

fn job() -> &'static mut Option<Job> {
    // SAFETY: `init` (App::new, before any decode) wrote None.
    unsafe { (*core::ptr::addr_of_mut!(JOB)).assume_init_mut() }
}

/// Lines of DECode:FRAMes?: the reply buffer, frames before `first` and
/// those that do not fit left out.
struct Out<'a> {
    b: &'a mut [u8],
    n: usize,
    first: u32,
    full: bool,
}

impl Out<'_> {
    fn w(&mut self, f: impl FnOnce(&mut dc::W)) -> bool {
        let mut t = [0u8; 64];
        let mut w = dc::W { b: &mut t, n: 0 };
        f(&mut w);
        let n = w.n;
        if self.full || self.n + n > self.b.len() {
            self.full = true;
            return false;
        }
        self.b[self.n..self.n + n].copy_from_slice(&t[..n]);
        self.n += n;
        true
    }
}

/// A DALI frame's line "seq,time_s,DALI,hex,OK|ERR,info\n" (`ns` from
/// the trigger point) and its counts (frames, bytes, errors); the reply
/// ends before a line that does not fit.
fn dali_line(o: &mut Out, seq: u32, ns: i64, f: &dc::Fr, cnt: &mut [u32; 5]) {
    cnt[0] += 1;
    cnt[1] += f.len as u32;
    cnt[2] += (f.flags != 0) as u32;
    if seq < o.first || o.full {
        return;
    }
    let n0 = o.n;
    if !o.w(|w| {
        w.u(seq).c(b',').secs_ns(ns).s(",DALI,");
        for &v in &f.head[..f.len as usize] {
            w.hex(v);
        }
        w.s(if f.flags != 0 { ",ERR," } else { ",OK," });
        dc::dali_text(w, f, false);
        w.c(b'\n');
    }) {
        o.n = n0;
    }
}

impl App {
    /// The band shown: 0 none, 1 UART, 2 Modbus, 3 DALI.
    pub(super) fn dec_band(&self) -> u8 {
        match ds().cfg.proto {
            dc::P_UART => 1,
            dc::P_MODBUS if dc::MODBUS => 2,
            dc::P_DALI if dc::DALI => 3,
            _ => 0,
        }
    }

    /// Model::dec_gen: blank while a drag / pinch moves the trace.
    pub(super) fn dec_gen_shown(&self) -> u16 {
        if self.sp.frozen_or_pinch() { 0 } else { ds().gen }
    }

    fn dec_bump() {
        ds().gen = ds().gen.wrapping_add(1).max(1);
    }

    /// Manual threshold as a code (the scope's code -> row map inverted).
    pub(super) fn dec_thr_code(&self) -> Option<u16> {
        let v = ds().thr_v;
        if !v.is_finite() {
            return None;
        }
        let r0 = self.scope.row_of_code(0.0);
        let slope = (self.scope.row_of_code(1000.0) - r0) / 1000.0;
        let c = (self.scope.row_of_volts(v) - r0) / slope;
        Some(if c.is_finite() { c.clamp(0.0, 1023.0) as u16 } else { 512 })
    }

    /// The level in use, V (NaN: none).
    fn dec_level(&self) -> f32 {
        if self.scope.log_on() {
            return self.log_level().unwrap_or(f32::NAN);
        }
        let a = ud::ann();
        if a.mid == 0 { f32::NAN } else { self.scope.volts(a.mid as f32) }
    }

    pub(super) fn dec_setup_vals(&self) -> ud::SetupVals {
        let s = ds();
        ud::SetupVals { proto: s.cfg.proto, mode: s.mode, fmt: s.cfg.fmt, inv: s.cfg.inverted, baud: s.cfg.baud, thr: s.thr_v, level: self.dec_level() }
    }

    /// The bar (zoomed out): the columns where the trace spans more than
    /// half the screen's swing, gaps up to `gap` px (a character) closed.
    fn dec_bar(&mut self, gap: usize) {
        let a = ud::ann();
        a.mode = ud::M_BAR;
        a.bar = [0; 50];
        let seg = &scope_page::cache_seg();
        let (mut lo, mut hi) = (i16::MAX, i16::MIN);
        for &(p, q) in seg.iter().filter(|s| s.0 <= s.1) {
            lo = lo.min(p);
            hi = hi.max(q);
        }
        let need = ((hi as i32 - lo as i32) / 2).max(3);
        let mut last: Option<usize> = None;
        for (i, &(p, q)) in seg.iter().enumerate().take(399) {
            if p <= q && (q as i32 - p as i32) >= need {
                let from = match last {
                    Some(l) if i - l <= gap.max(3) => l + 1,
                    _ => i,
                };
                for k in from..=i {
                    a.bar[k / 8] |= 1 << (k % 8);
                }
                last = Some(i);
            }
        }
    }

    /// The screen's smallest and largest code from the frame's column rows
    /// (sp_frame already took every column's min / max). A row is ~1.5
    /// codes at 2 V/div: plenty for the levels.
    fn dec_min_max(&self) -> (u16, u16) {
        let r0 = self.scope.row_of_code(0.0);
        let slope = (self.scope.row_of_code(1000.0) - r0) / 1000.0;
        let (mut lo, mut hi) = (i16::MAX, i16::MIN);
        for &(p, q) in scope_page::cache_seg().iter().take(399).filter(|s| s.0 <= s.1) {
            lo = lo.min(p);
            hi = hi.max(q);
        }
        if lo > hi || slope == 0.0 {
            return (u16::MAX, 0);
        }
        let code = |r: i16| ((r as f32 - r0) / slope).clamp(0.0, 1023.0) as u16;
        let (a, b) = (code(lo), code(hi));
        (a.min(b), a.max(b))
    }

    /// The record's number for the cache (0: none).
    fn dec_rec(&self) -> u32 {
        self.scope.rec_gen.wrapping_add(1).max(1)
    }

    /// The frame the page just built (scope_page `sp_frame`): the band's
    /// window of the kept decode, and a decode started when this record
    /// has none yet. No decoding here.
    #[inline(never)]
    pub(super) fn dec_frame(&mut self, f: &scope::Frame) {
        let a = ud::ann();
        if ds().cfg.proto == dc::P_OFF {
            a.mode = ud::M_NONE;
            return;
        }
        Self::dec_bump();
        let n = f.data.len();
        a.n = n.max(1) as u32;
        if f.cols.is_some() || n < 2 {
            // Roll / zoomed out past the record: the bar of the columns.
            self.dec_bar(0);
            return;
        }
        let rec = self.scope.record();
        let Some(off) = rec.offset_of(&f.data) else {
            a.mode = ud::M_NONE;
            return;
        };
        a.off = off as u32;
        let gen = self.dec_rec();
        let run = self.scope.s.run;
        let (_, _, trig) = self.scope.record_info();
        let wanted = !run || (trig && f.trig.is_some() && !self.scope.rolling() && !self.scope.wave_on());
        if a.gen != gen && wanted {
            let mm = self.dec_min_max();
            self.dec_start(gen, mm, (off, off + n));
        } else if a.gen == gen && !run && a.state == ud::C_DONE && !a.covers(off as u32, n as u32) && job().is_none() {
            // Stopped, more than the lists hold: the new window.
            let mm = self.dec_min_max();
            self.dec_start(gen, mm, (off, off + n));
        }
        self.dec_view();
    }

    /// The band's mode for the window `a.off`, `a.n` from the kept decode.
    fn dec_view(&mut self) {
        let a = ud::ann();
        let fresh = a.gen == self.dec_rec() && a.state != ud::C_BUSY && job().is_none();
        a.mode = match a.state {
            _ if a.gen != self.dec_rec() => ud::M_NONE,
            ud::C_SLOW => ud::M_NOTE,
            ud::C_NOSIG | ud::C_NONE => ud::M_NONE,
            _ => ud::M_BOXES,
        };
        if a.mode == ud::M_BOXES || (a.mode == ud::M_NONE && a.baud != 0) {
            // A character narrower than MIN_BOX px: the bar (also while the
            // decode runs, from the rate found last).
            let cs = (a.fs.max(1) as u64 * ds().cfg.char_bits() as u64 / a.baud.max(1) as u64) as i64;
            if a.baud != 0 && cs * 399 / (a.n.max(1) as i64) < ud::MIN_BOX as i64 {
                self.dec_bar((cs * 399 / a.n.max(1) as i64 + 1) as usize);
            } else if !fresh {
                a.mode = ud::M_NONE;
            }
        }
    }

    /// Start decoding the stored record `gen` (levels from `mm`; `win`: the
    /// screen). The whole record, or, when the record is already decoded
    /// (more bytes than the lists hold), the window with a margin.
    fn dec_start(&mut self, gen: u32, mm: (u16, u16), win: (usize, usize)) {
        let a = ud::ann();
        let rec = self.scope.record();
        let (fs, t0, _) = self.scope.record_info();
        let again = a.gen == gen && a.state == ud::C_DONE;
        // Auto, running: the rate found last, for REUSE records in a row (a
        // record with a framing error measures it again); a stopped record
        // is measured.
        let reuse = again || (self.scope.s.run && ds().reuse > 0 && a.baud != 0);

        let baud0 = if reuse { a.baud } else { 0 };
        if !again {
            ds().reuse = if baud0 != 0 { ds().reuse - 1 } else { REUSE };
        }
        a.reset(gen, ud::C_BUSY);
        a.fs = fs;
        a.t0 = t0 as u32;
        self.dec_abort();
        let s = ds();
        let Some(lvl) = dc::levels(mm.0, mm.1, self.dec_thr_code()) else {
            a.state = ud::C_NOSIG;
            a.mid = 0;
            return;
        };
        let baud = if dc::is_dali(s.cfg.proto) {
            dc::DALI_BAUD
        } else if s.cfg.baud != 0 {
            s.cfg.baud
        } else {
            baud0
        };
        let line = dc::Line { lo: lvl.0, mid: lvl.1, hi: lvl.2, inv: s.cfg.inverted, spb: 0, bits: 8, parity: 0 };
        let mut j = Job {
            a: 0,
            b: rec.len(),
            win,
            auto: None,
            auto0: 0,
            line,
            cur: dc::Cursor::at(0),
            fr: dc::Framer::new(false, 1, fs, 9600, 0, 1),
            fno: [u32::MAX; ud::NF],
            windowed: again,
            passes: 0,
        };

        if again {
            j.a = self.dec_margin(win.0, false, baud, fs);
            j.b = self.dec_margin(win.1, true, baud, fs).min(rec.len());
        }
        a.mid = lvl.1;
        a.lo = lvl.0;
        a.hi = lvl.2;

        a.c0 = j.a as u32;
        *job() = Some(j);
        if baud != 0 {
            self.dec_set_rate(baud);
        } else {
            // From the screen's start first: the traffic shown (a long
            // record may begin with older traffic at another rate).
            let j = job().as_mut().unwrap();
            j.auto0 = win.0.clamp(j.a, j.b.saturating_sub(2).max(j.a));
            j.auto = Some(dc::Baud::new(&rec, j.auto0, lvl.0, lvl.2));
        }
        // Held while a decode is left (the rate may have ended it).
        self.scope.hold_restart = self.scope.s.run && job().is_some();
    }

    /// `x` moved out by the margin a window decode needs (a frame's gap,
    /// Modbus, or two characters, plus two bits).
    fn dec_margin(&self, x: usize, up: bool, baud: u32, fs: u32) -> usize {
        let cb = ds().cfg.char_bits() as u64;
        let cs = (fs as u64 * cb / baud.max(1) as u64) as usize;
        let p = ds().cfg.proto;
        let m = cs * if dc::is_mb(p) || dc::is_dali(p) { 6 } else { 2 } + (fs as u64 * 2 / baud.max(1) as u64) as usize;
        if up { x.saturating_add(m) } else { x.saturating_sub(m) }
    }

    /// The bit rate is known: the line and the framer.
    fn dec_set_rate(&mut self, baud: u32) {
        let a = ud::ann();
        let s = ds();
        let Some(j) = job().as_mut() else { return };
        let (bits, parity) = s.cfg.bits();
        j.line.spb = (a.fs as u64 * 256 / baud.max(1) as u64) as u32;
        j.line.bits = bits;
        j.line.parity = parity;
        j.auto = None;
        a.baud = baud;
        // DALI: MIN_SPB samples a half bit.
        let dali = dc::is_dali(s.cfg.proto);
        if j.line.spb < dc::MIN_SPB << (8 + dali as u32) {
            a.note_baud = baud;
            self.dec_end(ud::C_SLOW);
            return;
        }
        let cs = j.line.char_samples();
        j.fr = dc::Framer::new(s.cfg.proto == dc::P_MODBUS && dc::MODBUS, cs, a.fs, baud, j.a as u32, j.b as u32);
        j.cur = dc::Cursor::at(j.a);
        if dali {
            *dl() = dc::Dali::at(j.a);
        }
    }

    /// The decode ends (`state`): the band follows.
    fn dec_end(&mut self, state: u8) {
        let a = ud::ann();
        if let Some(j) = job().take() {
            if state == ud::C_DONE {
                a.c1 = if a.full { j.cur.n as u32 } else { j.b as u32 };
            }
        }
        a.state = state;
        self.scope.hold_restart = false;
        self.dec_view();
        Self::dec_bump();
    }

    /// Drop a decode in progress (the record goes away, settings change).
    pub(super) fn dec_abort(&mut self) {
        if job().take().is_some() {
            self.scope.hold_restart = false;
        }
        unsafe { *core::ptr::addr_of_mut!(JOB_CYC) = 0 };

    }

    /// Main loop (`scope_poll`): one slice of the decode in progress.
    #[inline(never)]
    pub(super) fn dec_step(&mut self) {
        if job().is_none() {
            return;
        }
        let a = ud::ann();
        if ds().cfg.proto == dc::P_OFF || a.gen != self.dec_rec() {
            self.dec_abort();
            return;
        }
        crate::prof::enter(crate::prof::X2);
        let t0 = crate::prof::cyc();
        while job().is_some() && crate::prof::cyc().wrapping_sub(t0) < PASS_CYC {
            Self::dec_timed(|| self.dec_slice(SLICE));
        }
        crate::prof::exit(crate::prof::X2);

        if let Some(j) = job().as_mut() {
            j.passes += 1;
            if j.passes >= MAX_PASSES {
                // Never reached (a record is at most CAP * 8 / 5 samples):
                // the scope must not wait for ever.
                self.dec_end(ud::C_DONE);
            }
        }
    }

    /// Run the decode in progress to its end (DECode:FRAMes?).
    fn dec_finish(&mut self) {
        while job().is_some() {
            Self::dec_timed(|| self.dec_slice(usize::MAX / 4));
        }
    }

    /// A slice of the decode, timed: a decode that ends counts its time
    /// (prof DJOB).
    fn dec_timed(f: impl FnOnce()) {
        let t = crate::prof::cyc();
        f();
        let c = unsafe { &mut *core::ptr::addr_of_mut!(JOB_CYC) };
        *c = c.wrapping_add(crate::prof::cyc().wrapping_sub(t));
        if job().is_none() {
            crate::prof::add(crate::prof::DJOB, *c);
            *c = 0;
        }
    }

    /// `n` samples of the decode.
    fn dec_slice(&mut self, n: usize) {
        let rec = self.scope.record();
        let a = ud::ann();
        let Some(j) = job().as_mut() else { return };
        if rec.len() < j.b {
            self.dec_end(ud::C_NONE);
            return;
        }
        if let Some(m) = j.auto.as_mut() {
            // It stops at 32 bit-length runs; an idle line is a scan of
            // AUTO_MAX samples. Every sample is looked at: half a slice.
            let end = j.b.min(j.auto0 + AUTO_MAX);
            m.step(&rec, end.min(m.n + n / 2));
            if !m.enough() && m.n < end {
                return;
            }
            let r = m.result(a.fs);
            if r == Err(dc::NoRate::Few) && j.auto0 != j.a {
                // Too few edges from the screen on: from the start.
                j.auto0 = j.a;
                j.auto = Some(dc::Baud::new(&rec, j.a, a.lo, a.hi));
                return;
            }
            match r {
                Ok(b) => self.dec_set_rate(b),
                Err(dc::NoRate::TooFast) => {
                    a.note_baud = 0;
                    self.dec_end(ud::C_SLOW);
                }
                // Too few edges: the rate found last.
                Err(_) if a.baud != 0 => {
                    let b = a.baud;
                    self.dec_set_rate(b);
                }
                Err(_) => self.dec_end(ud::C_NOSIG),
            }
            return;
        }
        let modbus = ds().cfg.proto == dc::P_MODBUS && dc::MODBUS;
        let lim = j.cur.n.saturating_add(n);
        let (b, line) = (j.b, j.line);
        let fr = &mut j.fr;
        let fno = &mut j.fno;
        let mut keep_frame = |f: &dc::Fr, req: u32, nframes: u32| {
            let a = ud::ann();
            if a.nf as usize >= ud::NF {
                a.full = true;
                return;
            }
            let k = a.nf as usize;
            let r = fno[..k].iter().position(|&x| x == req).map_or(0xFF, |p| p as u8);
            fno[k] = nframes;
            a.f[k] = ud::FAnn { s0: f.start, s1: f.end, kind: f.kind, fl: f.flags, len: f.len, head: f.head, req: r };
            a.nf += 1;
        };
        let start = j.a;
        if dc::is_dali(ds().cfg.proto) {
            let d = dl();
            dc::dali_step(&rec, b, lim, &line, d, &mut |f| {
                keep_frame(f, u32::MAX, 0);
                !ud::ann().full
            });
            j.cur.n = d.n;
            j.cur.done = d.done;
        } else {
        dc::chars_step(&rec, start, b, lim, &line, &mut j.cur, &mut |c| {
            let a = ud::ann();
            if a.nb as usize >= ud::NB {
                a.full = true;
                return false;
            }
            a.b[a.nb as usize] = ud::BAnn { s0: c.start, s1: c.end, v: c.v as u8, fl: c.err };
            a.nb += 1;
            if c.err & dc::F_FE != 0 {
                ds().reuse = 0;
            }
            if modbus {
                let nf = fr.nframes;
                fr.push(c, &mut |f, req| keep_frame(f, req, nf));
            }
            true
        });
        }
        if !j.cur.done {
            return;
        }
        if modbus && !a.full {
            let nf = j.fr.nframes;
            j.fr.finish(&mut |f, req| keep_frame(f, req, nf));
        }
        if a.full && !j.windowed && j.cur.n < j.win.1 {
            // The lists ended before the screen's end: the screen's window
            // (with a margin) instead.
            let (win, baud, fs) = (j.win, a.baud, a.fs);
            j.a = self.dec_margin(win.0, false, baud, fs);
            j.b = self.dec_margin(win.1, true, baud, fs).min(rec.len());
            j.fno = [u32::MAX; ud::NF];
            j.windowed = true;
            let gen = a.gen;
            a.reset(gen, ud::C_BUSY);
            a.c0 = j.a as u32;
            self.dec_set_rate(baud);
            return;
        }

        self.dec_end(ud::C_DONE);
    }

    // ---- setup popup ----

    pub(super) fn dec_setup_tap(&mut self, clk: &mut Clock, h: ud::SetupHit, now: u64) {
        let s = ds();
        let old = s.cfg;
        let old_thr = s.thr_v;
        match h {
            // The frame log decodes UART / Modbus only.
            ud::SetupHit::Proto(p) if dc::is_dali(p) && self.scope.log_on() => return,
            ud::SetupHit::Proto(p) => s.cfg.proto = p,
            ud::SetupHit::Mode(m) => {
                if m != s.mode {
                    self.dec_mode(clk, m);
                }
                if self.sp_on() && !self.redraw {
                    self.sp_popup_draw();
                }
                return;
            }
            ud::SetupHit::Fmt(f) => s.cfg.fmt = f,
            ud::SetupHit::Inv(i) => s.cfg.inverted = i,
            ud::SetupHit::Baud(up) => {
                let i = ud::BAUDS.iter().position(|&b| b == s.cfg.baud).unwrap_or(0);
                let j = if up { (i + 1).min(ud::BAUDS.len() - 1) } else { i.saturating_sub(1) };
                s.cfg.baud = ud::BAUDS[j];
            }
            ud::SetupHit::ThrAuto(auto) => {
                s.thr_v = if auto { f32::NAN } else { self.dec_manual_start() };
            }
            ud::SetupHit::Thr(up) => {
                let v = if s.thr_v.is_finite() { s.thr_v } else { self.dec_manual_start() };
                let q = v / THR_STEP;
                let q = if q >= 0.0 { (q + 0.5) as i32 } else { (q - 0.5) as i32 };
                s.thr_v = ((q + if up { 1 } else { -1 }) as f32 * THR_STEP).clamp(-100.0, 100.0);
            }
            _ => return,
        }
        if s.cfg != old || s.thr_v.to_bits() != old_thr.to_bits() {
            self.dec_changed(old);
        }
        if self.view == View::Decode {
            self.log_setup_changed();
        } else {
            self.sp_popup_draw();
        }
        let _ = now;
    }

    /// Band / Log (ud::MODE_*): Log opens the frame log (app_log.rs), Band
    /// leaves it for the scope page.
    fn dec_mode(&mut self, clk: &mut Clock, m: u8) -> bool {
        if m == ud::MODE_LOG {
            self.log_open(clk).is_ok()
        } else {
            if self.view == View::Decode {
                self.show(View::Scope, clk);
            }
            self.log_close();
            true
        }
    }

    /// A manual threshold starts at the level in use (1.5 V without one).
    fn dec_manual_start(&self) -> f32 {
        let v = self.dec_level();
        if v.is_finite() { v } else { 1.5 }
    }

    /// Settings changed (`old`: before): the kept decode is dropped, the
    /// record decoded again (stopped: at once; running: the next record);
    /// the frame log decodes on with them.
    fn dec_changed(&mut self, old: dc::Cfg) {
        self.log_reset_decode();
        self.dec_abort();

        let a = ud::ann();
        let s = ds();
        if s.cfg.baud != old.baud || s.cfg.proto != old.proto {
            a.baud = 0;
        }
        a.reset(0, ud::C_NONE);
        a.mode = ud::M_NONE;
        Self::dec_bump();
        self.scope.refresh();
    }

    // ---- SCPI ----

    /// DECode:... except FRAMes?: true when a reply text is in `out`.
    #[inline(never)]
    pub fn dec_scpi(&mut self, clk: &mut Clock, c: crate::scpi::Dcd, out: &mut crate::scpi::Scpi) -> bool {
        use crate::scpi::Dcd as D;
        let s = ds();
        out.len = 0;
        let old = s.cfg;
        let old_thr = s.thr_v;
        match c {
            D::Proto(None) => out.put_bytes(match s.cfg.proto {
                dc::P_UART => b"UART",
                dc::P_MODBUS if dc::MODBUS => b"MODB",
                dc::P_DALI if dc::DALI => b"DALI",
                _ => b"OFF",
            }),
            D::Proto(Some(p)) if dc::is_dali(p) && self.scope.log_on() => {
                out.set_err("Settings conflict");
                return false;
            }
            D::Proto(Some(p)) if (p != dc::P_MODBUS || dc::MODBUS) && (p != dc::P_DALI || dc::DALI) => s.cfg.proto = p,
            D::Proto(Some(_)) => {
                out.set_err("Parameter not allowed");
                return false;
            }
            D::Baud(None) => {
                if s.cfg.baud == 0 {
                    out.put_bytes(b"AUTO");
                } else {
                    out.put_u32(s.cfg.baud);
                }
            }
            D::Baud(Some(b)) => s.cfg.baud = b,
            D::Rate if self.scope.log_on() => out.put_u32(self.log_rate()),
            D::Rate => {
                self.dec_ensure();
                let a = ud::ann();
                out.put_u32(if self.dec_band() != 0 && a.gen != 0 && a.state != ud::C_NOSIG { a.baud } else { 0 })
            }
            D::Fmt(None) => out.put_bytes(dc::FMT_NAMES[s.cfg.fmt as usize % 4].as_bytes()),
            D::Fmt(Some(f)) => s.cfg.fmt = f,
            D::Inv(None) => out.put_bytes(if s.cfg.inverted { b"INV" } else { b"NORM" }),
            D::Inv(Some(i)) => s.cfg.inverted = i,
            D::Thr(None) => {
                if s.thr_v.is_finite() {
                    out.put_f32(s.thr_v);
                } else {
                    out.put_bytes(b"AUTO");
                }
            }
            D::Thr(Some(v)) => s.thr_v = v,
            D::Level => {
                if !self.scope.log_on() {
                    self.dec_ensure();
                }
                let v = self.dec_level();
                if v.is_finite() {
                    out.put_f32(v)
                } else {
                    out.put_bytes(crate::scpi::NAN)
                }
            }
            D::Count => {
                let c = self.dec_count();
                for (i, v) in c.iter().enumerate() {
                    if i > 0 {
                        out.put_bytes(b",");
                    }
                    out.put_u32(*v);
                }
            }
            D::Mode(None) => out.put_bytes(if self.scope.log_on() { b"LOG" } else { b"BAND" }),
            D::Mode(Some(m)) => {
                if !self.dec_mode(clk, m) {
                    out.set_err("Settings conflict");
                }
                return false;
            }
            D::Clear => {
                self.log_clear();
                return false;
            }
            D::Frames(_) => return false,
        }
        if out.len == 0 && (s.cfg != old || s.thr_v.to_bits() != old_thr.to_bits()) {
            self.dec_changed(old);
            if self.sp.popup == crate::scope_ui::Popup::Mf(4) && !self.redraw {
                self.sp_popup_draw();
            }
        }
        out.len > 0
    }

    /// The kept decode, for a query: the one in progress finished; stopped
    /// with a record not decoded yet (the page not shown), decoded now
    /// (levels from the record's first AUTO_MAX samples).
    fn dec_ensure(&mut self) {
        if ds().cfg.proto == dc::P_OFF {
            return;
        }
        let gen = self.dec_rec();
        let rec = self.scope.record();
        if ud::ann().gen != gen && job().is_none() && !self.scope.s.run && rec.len() >= 2 {
            let mm = rec.min_max(0, rec.len().min(AUTO_MAX));
            self.dec_start(gen, mm, (0, rec.len()));
        }
        self.dec_finish();
    }

    /// DECode:COUNt?: frames, bytes, framing, parity, CRC errors of what
    /// FRAMes? reports.
    fn dec_count(&mut self) -> [u32; 6] {
        if self.scope.log_on() {
            return self.log_count();
        }
        let mut c = [0u32; 5];
        let mut buf = [0u8; 0];
        self.dec_lines(0, &mut buf, &mut c);
        [c[0], c[1], c[2], c[3], c[4], 0]
    }

    /// DECode:FRAMes? [<first>]: a #4nnnn block of frame lines (see
    /// `dec_lines`).
    #[inline(never)]
    pub fn dec_frames(&mut self, first: u32, buf: &mut [u8]) -> usize {
        let lim = buf.len().min(10_000);
        let mut c = [0u32; 5];
        let n = if self.scope.log_on() {
            // The frame log: the host's unread frames (at most `first`).
            self.log_lines(if first == 0 { u32::MAX } else { first }, &mut buf[6..lim])
        } else {
            self.dec_lines(first, &mut buf[6..lim], &mut c)
        };
        buf[0] = b'#';
        buf[1] = b'4';
        for (i, d) in [1000usize, 100, 10, 1].iter().enumerate() {
            buf[2 + i] = b'0' + ((n / d) % 10) as u8;
        }
        6 + n
    }

    /// `dec_lines` of the whole stored record, decoded again with the kept
    /// levels and rate (the lists hold only a part of it).
    fn dec_lines_record(&mut self, o: &mut Out, cnt: &mut [u32; 5], modbus: bool) {
        let a = ud::ann();
        let s = ds();
        let rec = self.scope.record();
        let len = rec.len();
        let (bits, parity) = s.cfg.bits();
        let l = dc::Line { lo: a.lo, mid: a.mid, hi: a.hi, inv: s.cfg.inverted, spb: (a.fs as u64 * 256 / a.baud.max(1) as u64) as u32, bits, parity };
        let (fs, t0) = (a.fs.max(1) as i64, a.t0 as i64);
        if dc::is_dali(s.cfg.proto) {
            let mut d = dc::Dali::at(0);
            let mut seq = 0;
            dc::dali_step(&rec, len, len, &l, &mut d, &mut |f| {
                dali_line(o, seq, (f.start as i64 - t0) * 1_000_000_000 / fs, f, cnt);
                seq += 1;
                true
            });
            return;
        }
        let mut fr = dc::Framer::new(modbus, l.char_samples(), a.fs, a.baud, 0, len as u32);
        // The line being written starts here (None: none / skipped).
        let mut open: Option<usize> = None;
        fn close(o: &mut Out, open: &mut Option<usize>, f: &dc::Fr, req: u32, cnt: &mut [u32; 5], modbus: bool) {
            cnt[0] += 1;
            cnt[4] += (f.flags & dc::F_CRC != 0) as u32;
            let Some(n0) = open.take() else { return };
            let ok = o.w(|w| {
                w.c(b',').s(dc::status(f.flags)).c(b',');
                if modbus {
                    dc::mb_text(w, f);
                    if req != u32::MAX {
                        w.s(" req=").u(req);
                    }
                }
                w.c(b'\n');
            });
            if !ok {
                o.n = n0;
            }
        }
        dc::chars(&rec, 0, len, &l, &mut |c| {
            cnt[1] += 1;
            cnt[2] += (c.err & dc::F_FE != 0) as u32;
            cnt[3] += (c.err & dc::F_PE != 0) as u32;
            fr.push(c, &mut |f, req| close(o, &mut open, f, req, cnt, modbus));
            if fr.open_len() == 1 && !o.full && fr.nframes >= o.first {
                // A new frame: its line.
                let n0 = o.n;
                let ns = (c.start as i64 - t0) * 1_000_000_000 / fs;
                let seq = fr.nframes;
                if o.w(|w| {
                    w.u(seq).c(b',').secs_ns(ns).s(if modbus { ",MB," } else { ",UART," });
                }) {
                    open = Some(n0);
                }
            }
            if let Some(n0) = open {
                let v = c.v as u8;
                if !o.w(|w| {
                    w.hex(v);
                }) {
                    o.n = n0;
                    open = None;
                }
            }
        });
        fr.finish(&mut |f, req| close(o, &mut open, f, req, cnt, modbus));
    }

    /// The kept frames as lines "seq,time_s,proto,hex,status,info\n" from
    /// frame `first` on, as many as fit `b` (its length used); `cnt` counts
    /// all of them (frames, bytes, FE, PE, CRC). Modbus: the frames with
    /// their kept bytes; UART: the bytes grouped by a gap of two characters
    /// (at most 16 a line).
    fn dec_lines(&mut self, first: u32, b: &mut [u8], cnt: &mut [u32; 5]) -> usize {
        let s = ds();
        if s.cfg.proto == dc::P_OFF {
            return 0;
        }
        self.dec_ensure();
        let a = ud::ann();
        let mut o = Out { b, n: 0, first, full: false };
        // Running: the last decoded record (a newer one may wait for its
        // trigger or its decode); stopped: the stored one.
        if a.state != ud::C_DONE || (!self.scope.s.run && a.gen != self.dec_rec()) {
            return 0;
        }

        let modbus = s.cfg.proto == dc::P_MODBUS && dc::MODBUS;
        if !self.scope.s.run && (a.c0 > 0 || (a.c1 as usize) < self.scope.record().len()) {
            // Stopped, more than the lists hold: the whole record again,
            // straight into the reply.
            self.dec_lines_record(&mut o, cnt, modbus);
            return o.n;
        }
        let fs = a.fs.max(1) as i64;
        let cs = (a.fs as u64 * s.cfg.char_bits() as u64 / a.baud.max(1) as u64) as u32;
        let t0 = a.t0 as i64;
        let line = |o: &mut Out, seq: u32, s0: u32, s1: u32, fl: u8, f: Option<&ud::FAnn>, cnt: &mut [u32; 5]| {
            cnt[0] += 1;
            if seq < o.first || o.full {
                return;
            }
            let n0 = o.n;
            let ns = (s0 as i64 - t0) * 1_000_000_000 / fs;
            let mut ok = o.w(|w| {
                w.u(seq).c(b',').secs_ns(ns).s(if modbus { ",MB," } else { ",UART," });
            });
            for b in a.b[..a.nb as usize].iter().filter(|b| b.s0 >= s0 && b.s1 <= s1) {
                let v = b.v;
                ok = ok && o.w(|w| {
                    w.hex(v);
                });
            }
            ok = ok && o.w(|w| {
                w.c(b',').s(dc::status(fl)).c(b',');
                if let Some(f) = f {
                    let fr = dc::Fr { start: 0, end: 0, len: f.len, head: f.head, flags: f.fl, kind: f.kind };
                    dc::mb_text(w, &fr);
                    if f.req != 0xFF {
                        w.s(" req=").u(f.req as u32);
                    }
                }
                w.c(b'\n');
            });
            if !ok {
                // The line does not fit: the reply ends before it.
                o.n = n0;
                o.full = true;
            }
        };
        for b in &a.b[..a.nb as usize] {
            cnt[1] += 1;
            cnt[2] += (b.fl & dc::F_FE != 0) as u32;
            cnt[3] += (b.fl & dc::F_PE != 0) as u32;
        }
        if dc::is_dali(s.cfg.proto) {
            for (k, f) in a.f[..a.nf as usize].iter().enumerate() {
                let fr = dc::Fr { start: 0, end: 0, len: f.len, head: f.head, flags: f.fl, kind: f.kind };
                dali_line(&mut o, k as u32, (f.s0 as i64 - t0) * 1_000_000_000 / fs, &fr, cnt);
            }
            return o.n;
        }
        if modbus {
            for (k, f) in a.f[..a.nf as usize].iter().enumerate() {
                cnt[4] += (f.fl & dc::F_CRC != 0) as u32;
                line(&mut o, k as u32, f.s0, f.s1, f.fl, Some(f), cnt);
            }
            return o.n;
        }
        let mut k = 0;
        let mut seq = 0;
        while k < a.nb as usize {
            let (s0, mut s1, mut fl) = (a.b[k].s0, a.b[k].s1, a.b[k].fl);
            let mut j = k + 1;
            while j < a.nb as usize && j - k < 16 && a.b[j].s0.saturating_sub(s1) <= 2 * cs {
                s1 = a.b[j].s1;
                fl |= a.b[j].fl;
                j += 1;
            }
            line(&mut o, seq, s0, s1, fl, None, cnt);
            seq += 1;
            k = j;
        }
        o.n
    }
}
