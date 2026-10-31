//! Scope waveform readout over SCPI (WAVeform:*, docs/bt-scpi.md "Scope"):
//! the 400 screen columns (the default, as before) or the acquisition
//! record, whole or decimated, as bytes (code >> 2) or 16-bit words (the
//! 10-bit ADC code, little endian), with a Keysight / Rigol style preamble
//! so a host needs no DM40 scale tables:
//!
//!   WAV:PRE? -> format,type,points,count,xinc,xorig,xref,yinc,yorig,yref
//!     format 0 BYTE, 1 WORD; type 0 screen columns (NORMal), 2 the record
//!     (RAW); count 1; point i: t = (i - xref) * xinc + xorig seconds from
//!     the trigger point (untriggered: the screen centre), volts at the
//!     probe tip (wave view: SI of the DMM jacks) = (data - yref) * yinc +
//!     yorig.
//!
//! The record lives in the scope buffer (scope::BUF, up to scope::CAP
//! samples = 53 KB as words) and is only consistent while the scope is
//! stopped: STOP, a Single sweep after its trigger, or DIGitize. Reading
//! it while running is refused ("Settings conflict", an empty block). A
//! reply longer than the transport's in-place buffer is streamed over
//! USBTMC from the buffer itself (usbtmc `respond_stream`: a packet at a
//! time from the main loop's USB polling, so the loop is never blocked;
//! the scope is held stopped until the last packet left, `Scope::host_read`),
//! and refused over Bluetooth ("Too much data": 11.5 kB/s, and the reply
//! has to fit the TX ring at once); WAV:POIN <n> keeps it small enough.
//!
//! Region B (zero-wait flash), with its parser scpi::wav_cmd: a host polls
//! these while the Wave view's stream ISR runs, and a fetch from region C
//! then stalls the ISR's entry (device c830e09: max_lat 6.16 us at
//! `query`). `fill` runs per USB packet of a streamed reply.

use crate::app::App;
use crate::scope;
use crate::scpi::{Cmd, Scpi};

/// What `fill` reads: the record (`rec`, every `stride`-th sample) or the
/// screen columns (`SCREEN`; that reply always fits the in-place buffer,
/// so it is copied at once, never streamed); `n` points. All zero at reset
/// (SRAM .sram_bss, not the TCM: the stack's headroom).
struct Src {
    rec: Option<&'static [u16]>,
    stride: usize,
    n: usize,
    word: bool,
    screen: bool,
}

#[cfg_attr(target_os = "none", link_section = ".sram_bss")]
static mut SRC: Src = Src { rec: None, stride: 0, n: 0, word: false, screen: false };

/// The last frame's 400 screen columns, mean 10-bit codes (App::scope_poll).
#[cfg_attr(target_os = "none", link_section = ".sram_bss")]
static mut SCREEN: [u16; scope::COLS] = [0; scope::COLS];

#[inline(always)] // per USB packet (`fill`): never a call into region C
fn src() -> &'static mut Src {
    // SAFETY: main loop only (SCPI dispatch and the USB polling it runs).
    unsafe { &mut *core::ptr::addr_of_mut!(SRC) }
}

/// The screen columns (written per frame by App::scope_poll).
#[inline(always)]
pub fn screen() -> &'static mut [u16; scope::COLS] {
    // SAFETY: main loop only.
    unsafe { &mut *core::ptr::addr_of_mut!(SCREEN) }
}

/// Bytes `off..off + out.len()` of the block's data part. Per USB packet
/// of a streamed reply (its own section, as when this module was .cold).
#[cfg_attr(target_os = "none", link_section = ".text.scope_wav_fill")]
fn fill(off: usize, out: &mut [u8]) {
    let s = src();
    for (j, o) in out.iter_mut().enumerate() {
        let p = off + j;
        let k = if s.word { p / 2 } else { p };
        if k >= s.n {
            *o = 0;
            continue;
        }
        let c = if s.screen {
            screen()[k]
        } else {
            s.rec.and_then(|r| r.get(k * s.stride.max(1))).copied().unwrap_or(0)
        } & 0x3FF;
        *o = if !s.word {
            (c >> 2) as u8
        } else if p & 1 == 0 {
            c as u8
        } else {
            (c >> 8) as u8
        };
    }
}

/// Record points for WAV:POINts `pts` (u32::MAX = all) of a record of
/// `len` samples: (stride, points).
fn decimate(len: usize, pts: u32) -> (usize, usize) {
    if len == 0 {
        return (1, 0);
    }
    let want = (pts as usize).clamp(1, len);
    let stride = len.div_ceil(want);
    (stride, len.div_ceil(stride))
}

/// The record readable now: stopped with a stored record.
fn record(app: &App) -> &'static [u16] {
    if app.scope.s.run { &[] } else { app.scope.record() }
}

/// Points the next WAV:DATA? holds.
fn points(app: &App) -> usize {
    if app.wav_points == 0 {
        scope::COLS
    } else {
        decimate(record(app).len(), app.wav_points).1
    }
}

/// CHAN1:OFFSet volts -> the position (rows; Scope::v_centre: the centre
/// row is at pos * 4 V/div / 141 volts, the offset is its negative).
#[inline(never)] // region B: not inlined into the hot caller
pub fn offset_pos(v: f32, vdiv: f32) -> i32 {
    let p = -v * 141.0 / (4.0 * vdiv);
    if p.is_finite() { (p + if p < 0.0 { -0.5 } else { 0.5 }) as i32 } else { 0 }.clamp(-392, 392)
}

/// The queries that need no block: WAV:POIN?, WAV:FORM?, TRIG:STAT?,
/// ACQ:SRAT?, CHAN1:OFFS?.
#[inline(never)] // region B: polled while the Wave view streams
pub fn query(app: &App, cmd: Cmd, out: &mut Scpi) {
    let sc = &app.scope;
    match cmd {
        Cmd::WavPoints(_) => out.put_u32(points(app) as u32),
        Cmd::WavFormat(_) => out.put_bytes(if app.wav_word { b"WORD" } else { b"BYTE" }),
        Cmd::TrigStatus => out.put_bytes(if !sc.s.run {
            b"STOP"
        } else {
            match sc.status {
                1 => b"AUTO",
                2 => b"TD",
                3 => b"RUN",
                _ => b"WAIT",
            }
        }),
        // Stopped with a stored record: its rate (what WAV:PRE? x increment,
        // the screen and Save CSV use), not the timebase shown after a zoom
        // (#158).
        Cmd::AcqSrate => out.put_f32(if record(app).is_empty() { sc.sample_rate() } else { sc.record_info().0 } as f32),
        _ => out.put_f32(-(sc.s.pos as f32) * 4.0 * sc.vdiv() / 141.0),
    }
}

/// WAV:PREamble? (module doc).
#[inline(never)] // region B: polled while the Wave view streams
pub fn preamble(app: &App, out: &mut Scpi) {
    let sc = &app.scope;
    let word = app.wav_word;
    let (ty, n, xinc, xorig) = if app.wav_points == 0 {
        // The screen: 10 divisions, the trigger at column hpos.
        let xinc = scope::SDIV[sc.s.tb.min(scope::SDIV.len() - 1)] * 10.0 / scope::COLS as f32;
        (0, scope::COLS, xinc, -(sc.s.hpos as f32) * xinc)
    } else {
        let rec = record(app);
        let (stride, n) = decimate(rec.len(), app.wav_points);
        let (fs, t0, _) = sc.record_info();
        let dt = if fs > 0 { 1.0 / fs as f32 } else { 0.0 };
        (2, n, stride as f32 * dt, -(t0 as f32) * dt)
    };
    // Volts per code over 1000 codes (f32: a one-code difference near
    // yorig would lose ~5 digits of the slope).
    // BYTE: a byte stands for codes 4b .. 4b + 3; yorig is the middle of
    // them (code 1.5), not code 0 (1.5 codes low on average, #130).
    let c0 = sc.volts(0.0);
    let k1 = (sc.volts(1000.0) - c0) / 1000.0;
    let (k, y0) = if word { (k1, c0) } else { (4.0 * k1, c0 + 1.5 * k1) };
    out.put_bytes(if word { b"1,".as_slice() } else { b"0,".as_slice() });
    out.put_u32(ty);
    out.put_bytes(b",");
    out.put_u32(n as u32);
    out.put_bytes(b",1,");
    out.put_f32(xinc);
    out.put_bytes(b",");
    out.put_f32(xorig);
    out.put_bytes(b",0,");
    out.put_f32(k);
    out.put_bytes(b",");
    out.put_f32(y0);
    out.put_bytes(b",0");
}

/// The reply has to be a block of its own (not joined with other replies
/// of a message): every WAV:DATA? except the 405-byte SCReen + BYTE one.
#[inline(always)] // main.rs per SCPI command: no call into region C
pub fn is_block(app: &App) -> bool {
    app.wav_points != 0 || app.wav_word
}

/// The reply in flight reads the scope buffer (`Scope::host_read` while it
/// streams).
#[inline(always)] // main.rs, every pass
pub fn reads_record() -> bool {
    !src().screen
}

/// WAV:DATA? as a block (`is_block`): #<d><len><data>. Streamed when it
/// does not fit the in-place buffer (USBTMC); refused over a transport
/// that cannot stream (Bluetooth) beyond that.
#[inline(never)] // region B: not inlined into the hot caller
pub fn data(app: &mut App, port: &mut dyn crate::bt::ScpiPort, scpi: &mut Scpi) {
    let s = src();
    s.word = app.wav_word;
    if app.wav_points == 0 {
        s.screen = true;
        s.n = scope::COLS;
    } else {
        let rec = record(app);
        if app.scope.s.run {
            // The buffer is being refilled: STOP / SINGle / DIGitize first.
            scpi.set_err("Settings conflict");
        }
        let (stride, n) = decimate(rec.len(), app.wav_points);
        s.screen = false;
        s.rec = Some(rec);
        s.stride = stride;
        s.n = n;
    }
    let len = s.n * if s.word { 2 } else { 1 };
    // #<digits><len>
    let mut h = [0u8; 12];
    let mut d = [0u8; 10];
    let mut k = d.len();
    let mut v = len;
    loop {
        k -= 1;
        d[k] = b'0' + (v % 10) as u8;
        v /= 10;
        if v == 0 {
            break;
        }
    }
    let nd = d.len() - k;
    h[0] = b'#';
    h[1] = b'0' + nd as u8;
    h[2..2 + nd].copy_from_slice(&d[k..]);
    let hn = 2 + nd;
    if hn + len <= crate::usbtmc::RSP_MAX {
        port.respond_with(&mut |b| {
            b[..hn].copy_from_slice(&h[..hn]);
            fill(0, &mut b[hn..hn + len]);
            Some(hn + len)
        });
    } else if port.respond_stream(&h[..hn], len, fill) {
        // Held from now on (main.rs renews it per pass while it streams).
        app.scope.host_read = !s.screen;
    } else {
        scpi.set_err("Too much data");
        s.n = 0;
        port.respond(b"#10");
    }
}
