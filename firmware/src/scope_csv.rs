//! Scope record on the SPI-flash USB disk (UF-13, the stock MF Settings
//! "Save CSV"): the stored record of a STOPPED scope, written as the next
//! free `SCOP%03u.DLG` by the screenshot writer (screenshot.rs, kind Csv:
//! non-blocking slices, data -> FAT -> directory -> CRC verify, the same
//! disk-sharing rules: refused while a PC has the disk mounted, host writes
//! refused while it runs, one disk job at a time).
//!
//! The file is the compact binary form (dlog.rs kind 2, docs/
//! compact-log-format.md): the CSV header text, the code -> volts map and
//! the sample time, then the raw 10-bit codes as u16: 53 KB for the full
//! 26624-sample record instead of the CSV's 692 KB. The CSV exists on read
//! (SCOP%03u.CSV on the read-only LUN 1, MMEM:DATA?, tools/dlog.py), byte
//! for byte what this module wrote as text before:
//!   # Alientek DM40A, opendm40 0.1, scope record (no real-time clock)
//!   # timebase 200us/div, 1V/div, probe x1, coupling DC, sample rate 12.5MS/s, samples 26624
//!   # trigger Edge rising 993mV, mode Auto, triggered
//!   # time_s = seconds from the trigger point (untriggered: from the screen centre); volts at the probe tip
//!   time_s,volts            (a current record of the Wave view: time_s,amperes)
//!   -1.06496e-03,+1.2345e+00
//! (fixed-width 26-byte rows, dlog::scope_row). The file length is known
//! before the first block is written (screenshot.rs allocates the whole
//! chain first) and any block can be filled on its own (`fill`). The whole
//! record (up to scope::CAP samples, ~10 s) is written, not only the screen.
//!
//! The record lives in the scope buffer: while it is written the scope is
//! held stopped (`Scope::hold`; RUN / AUTO / SCPI RUN do not restart it)
//! and the code -> volts map is the one taken at `start` (a probe or V/div
//! change meanwhile does not change the file).

#![allow(dead_code)]

/// CSV row width (the generated CSV, dlog::scope_row).
pub const ROW: usize = crate::dlog::SCOPE_ROW;
const HDR_MAX: usize = 512;
/// Largest record (scope::CAP, asserted in scope_page.rs) and file.
pub const REC_MAX: usize = 26 * 1024;
pub const MAX_LEN: u32 = (crate::dlog::FHDR + HDR_MAX + crate::dlog::SCOPE_PAR + 8 * crate::dlog::SBLK_MAX + 4
    + REC_MAX.div_ceil(crate::dlog::SBLK) * crate::dlog::SBLK_RAW) as u32;
const _: () = assert!(REC_MAX <= crate::dlog::SBLK * crate::dlog::SBLK_MAX);

struct Snap {
    /// None until the first `set` (an Option so SNAP is all zero: .bss, not
    /// 548 B of the .data load image in region C; region C audit #1).
    rec: Option<&'static [u16]>,
    /// Volts = a + b * code; seconds per sample; trigger / centre index.
    a: f32,
    b: f32,
    dt: f32,
    t0: usize,
    hdr: [u8; HDR_MAX],
    hdr_n: usize,
    /// Coded blocks (dlog::SBLK samples each; PLAN holds their coding).
    nb: usize,
    /// PLAN's fill cursor is valid.
    cur_ok: bool,
}

static mut SNAP: Snap = Snap { rec: None, a: 0.0, b: 0.0, dt: 0.0, t0: 0, hdr: [0; HDR_MAX], hdr_n: 0, nb: 0, cur_ok: false };

/// Per block: mode (fixed predictor order / raw), coded bytes, CRC-32.
/// Written by `set` before any use: SRAM (.dmabuf, not zeroed at reset),
/// not TCM.
struct Plan {
    mode: [u32; crate::dlog::SBLK_MAX],
    len: [u32; crate::dlog::SBLK_MAX],
    crc: [u32; crate::dlog::SBLK_MAX],
    /// The fill cursor: block `cb` coded up to `enc.pos` (valid while
    /// Snap::cur_ok).
    enc: crate::dlog::SEnc,
    cb: usize,
}

// Backup SRAM (.bkp_bss, link-native.x; CPU only, zeroed at reset;
// docs/native-boot.md "Extra RAM areas").
#[cfg_attr(target_os = "none", link_section = ".bkp_bss")]
static mut PLAN: Plan = Plan {
    mode: [0; crate::dlog::SBLK_MAX],
    len: [0; crate::dlog::SBLK_MAX],
    crc: [0; crate::dlog::SBLK_MAX],
    enc: crate::dlog::SEnc::new(0, 0),
    cb: 0,
};

fn plan() -> &'static mut Plan {
    unsafe { &mut *core::ptr::addr_of_mut!(PLAN) }
}

fn snap() -> &'static mut Snap {
    unsafe { &mut *core::ptr::addr_of_mut!(SNAP) }
}

/// Header text writer.
pub struct Hdr {
    b: [u8; HDR_MAX],
    n: usize,
}

impl Hdr {
    pub const fn new() -> Hdr {
        Hdr { b: [0; HDR_MAX], n: 0 }
    }
    pub fn s(&mut self, s: &str) -> &mut Hdr {
        for &c in s.as_bytes() {
            if self.n < HDR_MAX - 2 && c.is_ascii() {
                self.b[self.n] = c;
                self.n += 1;
            }
        }
        self
    }
}

/// Take the record for writing: `rec` (codes), value = a + b * code (volts,
/// or amperes for a current record of the Wave view), sample rate `fs`,
/// `t0` = sample index of time 0, the header lines (without the column
/// line), `amps`: the value column is "amperes" (else "volts"). The writer
/// is started separately
/// (screenshot::start_csv) and reads it through `fill`.
/// Region B (not this module's .cold): it runs in the Save CSV tap, while
/// the Wave view's stream may run (the stream pauses only for the write).
#[cfg_attr(target_os = "none", link_section = ".text.scope_plan")]
pub fn set(rec: &'static [u16], a: f32, b: f32, fs: f32, t0: usize, h: &Hdr, amps: bool) {
    let s = snap();
    s.rec = Some(rec);
    s.a = a;
    s.b = b;
    s.dt = if fs > 0.0 { 1.0 / fs } else { 0.0 };
    s.t0 = t0;
    let mut n = 0;
    for &c in &h.b[..h.n] {
        s.hdr[n] = c;
        n += 1;
    }
    let col: &[u8] = if amps { b"time_s,amperes\r\n" } else { b"time_s,volts\r\n" };
    for &c in col {
        if n < HDR_MAX {
            s.hdr[n] = c;
            n += 1;
        }
    }
    s.hdr_n = n;
    // Code every block once (~1 ms per block at 200 MHz): the file length
    // must be known before the writer allocates; `fill` codes them again.
    let p = plan();
    s.cur_ok = false;
    s.nb = rec.len().div_ceil(crate::dlog::SBLK).min(crate::dlog::SBLK_MAX);
    for (b, c) in rec.chunks(crate::dlog::SBLK).take(s.nb).enumerate() {
        (p.mode[b], p.len[b], p.crc[b]) = crate::dlog::scope_plan(c);
    }
}

/// Bytes before the first coded block: DLG header, parameters, table.
fn head_len(s: &Snap) -> usize {
    crate::dlog::FHDR + s.hdr_n + crate::dlog::SCOPE_PAR + 8 * s.nb + 4
}

/// File length in bytes (the DLG file).
pub fn len() -> u32 {
    let s = snap();
    let p = plan();
    (head_len(s) + p.len[..s.nb].iter().map(|&l| l as usize).sum::<usize>()) as u32
}

/// File bytes `off .. off + buf.len()`: the DLG header, parameters and
/// block table, then the coded blocks. Sequential calls continue the
/// coding where the last one stopped (screenshot.rs fills a disk block in
/// pieces: each piece costs only its own samples).
pub fn fill(off: u32, buf: &mut [u8]) {
    let s = snap();
    let p = plan();
    let off = off as usize;
    let head = head_len(s);
    let rec = s.rec.unwrap_or(&[]);
    let mut i = 0;
    if off < head {
        let mut h = [0u8; crate::dlog::FHDR + HDR_MAX + crate::dlog::SCOPE_PAR + 8 * crate::dlog::SBLK_MAX + 4];
        let k = crate::dlog::scope_header(&s.hdr[..s.hdr_n], s.a, s.b, s.dt, s.t0 as u32, rec.len() as u32, &mut h);
        crate::dlog::scope_table(&p.len[..s.nb], &p.crc[..s.nb], &mut h[k..]);
        i = (head - off).min(buf.len());
        buf[..i].copy_from_slice(&h[off..off + i]);
    }
    let codes = |b: usize| &rec[b * crate::dlog::SBLK..((b + 1) * crate::dlog::SBLK).min(rec.len())];
    while i < buf.len() {
        let at = (off + i) as u32;
        if !s.cur_ok || p.enc.pos != at {
            // Seek: the block holding `at`, coded from its start.
            let mut o = head as u32;
            let mut b = 0;
            while b < s.nb && o + p.len[b] <= at {
                o += p.len[b];
                b += 1;
            }
            if b == s.nb {
                return;
            }
            p.cb = b;
            p.enc = crate::dlog::SEnc::new(p.mode[b], o);
            while p.enc.pos < at && p.enc.next(codes(b)).is_some() {}
            s.cur_ok = true;
        }
        match p.enc.next(codes(p.cb)) {
            Some(x) => {
                buf[i] = x;
                i += 1;
            }
            None => {
                if p.cb + 1 >= s.nb {
                    return;
                }
                p.cb += 1;
                p.enc = crate::dlog::SEnc::new(p.mode[p.cb], at);
            }
        }
    }
}
