//! The protocol decoder's bus band on the scope page (src/ui_decode.rs) and
//! its setup popup: synthetic UART / Modbus signals decoded by src/decode.rs
//! the way app_decode.rs does it (a host copy of `dec_frame`), the trace and
//! the band drawn by the firmware's code. The band lives on the stock-replica
//! graph: texts must not collide or spill out of their box (the replica
//! rules); every label is picked only when it fits its box with 3 px either
//! side (ui_decode `pick`). Matrix: UART and Modbus at 24 zoom levels (from
//! 2 to 600 characters on the screen) x 3 pan offsets, both views, errors,
//! the bar, the "rate too low" note, the Wave view; the setup popup under
//! the full rules.
use super::*;
use decode as dc;
use scope_ui::dec as ud;

/// 3.3 V logic at the decoder's codes (512 = 0 V, 53.75 codes / V).
const LO: f64 = 512.0;
const HI: f64 = 689.0;

/// Bits of `frames` (UART, `fmt`), with `gap` idle bits after each and
/// `err` = index of a byte sent with a low stop bit.
fn bits(frames: &[Vec<u8>], fmt: u8, gap: u32, err: Option<usize>) -> Vec<bool> {
    let (nb, par) = [(8, 0), (8, 1), (8, 2), (7, 1)][fmt as usize];
    let mut v = vec![true; 20];
    let mut k = 0;
    for f in frames {
        for &b in f {
            v.push(false);
            let mut ones = 0;
            for i in 0..nb {
                let x = b >> i & 1 == 1;
                ones += x as u32;
                v.push(x);
            }
            match par {
                1 => v.push(ones % 2 == 1),
                2 => v.push(ones % 2 == 0),
                _ => {}
            }
            v.push(err != Some(k));
            k += 1;
        }
        v.extend(std::iter::repeat(true).take(gap as usize));
    }
    v.extend(std::iter::repeat(true).take(60));
    v
}

/// Codes at `spb` samples per bit (a little ripple, no noise).
fn codes(b: &[bool], spb: f64) -> Vec<u16> {
    let n = (b.len() as f64 * spb) as usize;
    (0..n).map(|i| {
        let k = ((i as f64 + 0.37) / spb) as usize;
        (if b[k.min(b.len() - 1)] { HI } else { LO } + (i % 3) as f64) as u16
    }).collect()
}

/// The trace columns of `c[off..off + n]` (rows of the 280-row model).
fn trace(c: &[u16], off: usize, n: usize) -> stock::Trace {
    let row = |v: u16| (140.0 - (v as f64 - 512.0) * 17.5 / 53.75).round() as i32;
    let (mut a, mut b) = (vec![], vec![]);
    for i in 0..400 {
        let s = off + i * n / 400;
        let e = (off + (i + 1) * n / 400).max(s + 1).min(c.len());
        let (lo, hi) = c[s.min(c.len() - 1)..e.max(s.min(c.len() - 1) + 1)].iter().fold((u16::MAX, 0), |m, &v| (m.0.min(v), m.1.max(v)));
        a.push(row(hi));
        b.push(row(lo));
    }
    stock::Trace { a, b }
}

/// The host copy of app_decode.rs `dec_frame` / `dec_boxes` / `dec_bar`.
fn fill(c: &[u16], off: usize, n: usize, fs: u32, cfg: dc::Cfg, tr: &stock::Trace) {
    // The firmware's job over the window (app_decode.rs: the window decode
    // a stopped record gets when it holds more than the lists).
    let a = ud::ann();
    a.reset(1, ud::C_DONE);
    a.baud = 0;
    a.mode = ud::M_NONE;
    a.bar = [0; 50];
    a.off = off as u32;
    a.n = n as u32;
    a.fs = fs;
    let (mn, mx) = dc::min_max(c, off, off + n);
    let Some((lo, mid, hi)) = dc::levels(mn, mx, None) else { return };
    a.mid = mid;
    let baud = if cfg.baud != 0 { Ok(cfg.baud) } else { dc::auto_baud(c, off, off + n, lo, hi, fs) };
    let baud = match baud {
        Ok(b) => b,
        Err(dc::NoRate::TooFast) => {
            a.mode = ud::M_NOTE;
            return;
        }
        Err(_) => return,
    };
    a.baud = baud;
    let (bits, parity) = cfg.bits();
    let l = dc::Line { lo, mid, hi, inv: cfg.inverted, spb: (fs as u64 * 256 / baud as u64) as u32, bits, parity };
    let cs = l.char_samples() as usize;
    if l.spb < dc::MIN_SPB << 8 {
        a.mode = ud::M_NOTE;
        a.note_baud = baud;
        return;
    }
    if (cs as i64 * 399 / n as i64) < ud::MIN_BOX as i64 {
        a.mode = ud::M_BAR;
        let (mut glo, mut ghi) = (i32::MAX, i32::MIN);
        for i in 0..399 {
            glo = glo.min(tr.a[i]);
            ghi = ghi.max(tr.b[i]);
        }
        let gap = (cs as i64 * 399 / n as i64 + 1) as usize;
        let mut last: Option<usize> = None;
        for i in 0..399 {
            if tr.b[i] - tr.a[i] >= ((ghi - glo) / 2).max(3) {
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
        return;
    }
    a.mode = ud::M_BOXES;
    let modbus = cfg.proto == dc::P_MODBUS;
    let margin = cs * if modbus { 6 } else { 2 } + (l.spb >> 8) as usize * 2;
    let (lo_s, hi_s) = (off.saturating_sub(margin), (off + n + margin).min(c.len()));
    let mut fr = dc::Framer::new(modbus, cs as u32, fs, baud, lo_s as u32, hi_s as u32);
    let mut fno = [u32::MAX; ud::NF];
    let (v0, v1) = (off as i64, (off + n) as i64);
    let mut on_frame = |f: &dc::Fr, req: u32, nf: u32| {
        let a = ud::ann();
        if (f.end as i64) < v0 || f.start as i64 >= v1 || a.nf as usize >= ud::NF {
            return;
        }
        let k = a.nf as usize;
        let r = fno[..k].iter().position(|&x| x == req).map_or(0xFF, |p| p as u8);
        fno[k] = nf;
        a.f[k] = ud::FAnn { s0: f.start, s1: f.end, kind: f.kind, fl: f.flags, len: f.len, head: f.head, req: r };
        a.nf += 1;
    };
    dc::chars(&c, lo_s, hi_s, &l, &mut |ch| {
        let a = ud::ann();
        if (ch.end as i64) >= v0 && (ch.start as i64) < v1 {
            if (a.nb as usize) < ud::NB {
                a.b[a.nb as usize] = ud::BAnn { s0: ch.start, s1: ch.end, v: ch.v as u8, fl: ch.err };
                a.nb += 1;
            } else {
                a.full = true;
            }

        }
        if modbus {
            let nf = fr.nframes;
            fr.push(ch, &mut |f, r| on_frame(f, r, nf));
        }
    });
    if modbus {
        let nf = fr.nframes;
        fr.finish(&mut |f, r| on_frame(f, r, nf));
    }
}

fn crc(d: &[u8]) -> Vec<u8> {
    let mut v = d.to_vec();
    let c = dc::crc16(d);
    v.push(c as u8);
    v.push((c >> 8) as u8);
    v
}

fn uart_frames() -> Vec<Vec<u8>> {
    let mut v = vec![];
    for _ in 0..12 {
        v.push(b"Hello, opendm40!\r\n".to_vec());
        v.push(b"T=23.5C\r\n".to_vec());
        v.push(vec![0x02, 0x10, 0xFF, 0x00, b'A', b'C', b'K', 0x03]);
    }
    v
}

fn modbus_frames(bad: bool) -> Vec<Vec<u8>> {
    let req = crc(&[0x01, 0x03, 0x00, 0x00, 0x00, 0x02]);
    let rsp = crc(&[0x01, 0x03, 0x04, 0x00, 0xEB, 0x01, 0x9A]);
    let w6 = crc(&[0x11, 0x06, 0x00, 0x05, 0x04, 0xD2]);
    let exr = crc(&[0x01, 0x04, 0x00, 0x10, 0x00, 0x01]);
    let exc = crc(&[0x01, 0x84, 0x02]);
    let mut b = req.clone();
    b[6] ^= 0x5A;
    let mut v = vec![];
    for k in 0..6 {
        v.extend([req.clone(), rsp.clone(), w6.clone(), w6.clone(), exr.clone(), exc.clone()]);
        if bad && k == 1 {
            v.push(b.clone());
        }
    }
    v
}

/// One scene: the signal `b` at `spb`, `chars` characters on the screen
/// from `pos` (0..1 of the record), the band of `cfg`.
#[allow(clippy::too_many_arguments)]
fn scene(ck: &mut Checker, name: &str, b: &[bool], spb: f64, cfg: dc::Cfg, chars: f64, pos: f64, combined: bool, stop: bool, wave: bool) {
    let c = codes(b, spb);
    let cb = cfg.char_bits() as f64;
    let n = ((chars * cb * spb) as usize).clamp(40, c.len());
    let off = ((c.len() - n) as f64 * pos) as usize;
    let fs = (spb * if cfg.baud != 0 { cfg.baud as f64 } else { 9600.0 }) as u32;
    let cfg_fill = dc::Cfg { baud: if cfg.baud != 0 { cfg.baud } else { 0 }, ..cfg };
    let tr = trace(&c, off, n);
    fill(&c, off, n, fs, cfg_fill, &tr);
    let base = &stock::scenes()[0];
    let mut m = base.m;
    m.combined = combined;
    m.dec = if cfg.proto == dc::P_MODBUS { 2 } else { 1 };
    m.dec_gen = 1;
    m.pos = 0;
    m.trig_row = 110;
    m.vdiv = 10; // 2 V/div
    if stop {
        m.status = scope_ui::ST_STOP;
        m.record = (pos as f32 * 0.8, pos as f32 * 0.8 + 0.2);
    }
    if wave {
        m.src = scope_ui::SRC_DMM;
        m.wvdiv = 2.0;
        m.ch1_txt = "DC V";
    }
    let sc = stock::Scene { name: "decode", m, popup: scope_ui::Popup::None, trace: Some(tr), persist: None, cols: 400, cells: base.cells.clone() };
    stock::draw(&sc);
    ck.check_replica(name);
}

/// DALI half bits (true = high): a start bit, `n` bits of `v` MSB first;
/// `bad`: a bit without its centre transition.
fn dali_halves(v: &mut Vec<bool>, x: u32, n: u8, bad: Option<u8>) {
    v.extend([false, true]);
    for i in (0..n).rev() {
        let b = x >> i & 1 == 1;
        if bad == Some(n - 1 - i) {
            v.extend([false, false]);
        } else {
            v.extend([!b, b]);
        }
    }
}

/// A DALI bus: forward / backward pairs, a 24-bit frame, an error.
fn dali_bits() -> Vec<bool> {
    let mut v = vec![true; 30];
    for k in 0..4u32 {
        dali_halves(&mut v, 0xFE80 + k, 16, None);
        v.extend([true; 14]);
        dali_halves(&mut v, 0x05A1, 16, None);
        v.extend([true; 12]);
        dali_halves(&mut v, 0xFF, 8, None);
        v.extend([true; 30]);
        dali_halves(&mut v, 0x9B21, 16, if k == 1 { Some(9) } else { None });
        v.extend([true; 24]);
        dali_halves(&mut v, 0xC1A2B3, 24, None);
        v.extend([true; 24]);
    }
    v.extend([true; 60]);
    v
}

/// The host copy of app_decode.rs for DALI (the box / bar choice of
/// `dec_view`, frames from `dali_step`).
fn dali_fill(c: &[u16], off: usize, n: usize, fs: u32, tr: &stock::Trace) {
    let a = ud::ann();
    a.reset(1, ud::C_DONE);
    a.mode = ud::M_NONE;
    a.bar = [0; 50];
    a.off = off as u32;
    a.n = n as u32;
    a.fs = fs;
    a.baud = dc::DALI_BAUD;
    let (mn, mx) = dc::min_max(c, 0, c.len());
    let Some((lo, mid, hi)) = dc::levels(mn, mx, None) else { return };
    a.mid = mid;
    let l = dc::Line { lo, mid, hi, inv: false, spb: (fs as u64 * 256 / dc::DALI_BAUD as u64) as u32, bits: 8, parity: 0 };
    let cs = (fs as u64 * 10 / dc::DALI_BAUD as u64) as i64;
    if cs * 399 / (n as i64) < ud::MIN_BOX as i64 {
        a.mode = ud::M_BAR;
        let (mut glo, mut ghi) = (i32::MAX, i32::MIN);
        for i in 0..399 {
            glo = glo.min(tr.a[i]);
            ghi = ghi.max(tr.b[i]);
        }
        for i in 0..399 {
            if tr.b[i] - tr.a[i] >= ((ghi - glo) / 2).max(3) {
                a.bar[i / 8] |= 1 << (i % 8);
            }
        }
        return;
    }
    a.mode = ud::M_BOXES;
    let (v0, v1) = (off as i64, (off + n) as i64);
    let mut d = dc::Dali::at(0);
    dc::dali_step(&c, c.len(), c.len(), &l, &mut d, &mut |f| {
        let a = ud::ann();
        if (f.end as i64) >= v0 && (f.start as i64) < v1 && (a.nf as usize) < ud::NF {
            a.f[a.nf as usize] = ud::FAnn { s0: f.start, s1: f.end, kind: f.kind, fl: f.flags, len: f.len, head: f.head, req: 0xFF };
            a.nf += 1;
        }
        true
    });
}

/// A DALI scene: `bits` (forward frames' worth, 19 bits each) on the
/// screen from `pos`, `sph` samples a half bit.
fn dali_scene(ck: &mut Checker, name: &str, b: &[bool], sph: f64, frames: f64, pos: f64, combined: bool, stop: bool) {
    let c = codes(b, sph);
    let n = ((frames * 38.0 * sph) as usize).clamp(40, c.len());
    let off = ((c.len() - n) as f64 * pos) as usize;
    let fs = (sph * 2400.0) as u32;
    let tr = trace(&c, off, n);
    dali_fill(&c, off, n, fs, &tr);
    let base = &stock::scenes()[0];
    let mut m = base.m;
    m.combined = combined;
    m.dec = 3;
    m.dec_gen = 1;
    m.pos = 0;
    m.trig_row = 110;
    m.vdiv = 10;
    if stop {
        m.status = scope_ui::ST_STOP;
        m.record = (pos as f32 * 0.8, pos as f32 * 0.8 + 0.2);
    }
    let sc = stock::Scene { name: "decode", m, popup: scope_ui::Popup::None, trace: Some(tr), persist: None, cols: 400, cells: base.cells.clone() };
    stock::draw(&sc);
    ck.check_replica(name);
}

pub fn run(ck: &mut Checker, out: &str) {
    let ub = bits(&uart_frames(), 0, 3, None);
    let mb = bits(&modbus_frames(true), 1, 50, None);
    let uart = dc::Cfg { proto: dc::P_UART, baud: 115_200, fmt: 0, inverted: false };
    let modbus = dc::Cfg { proto: dc::P_MODBUS, baud: 19200, fmt: 1, inverted: false };
    // The matrix: 24 zoom levels x 3 pan offsets x both views.
    for (proto, b, cfg) in [("uart", &ub, uart), ("modbus", &mb, modbus)] {
        for z in 0..24 {
            let chars = 2.0 * 1.28f64.powi(z);
            for (k, pos) in [0.0, 0.37, 1.0].iter().enumerate() {
                for combined in [false, true] {
                    scene(ck, &format!("decode/{proto}/z{z}/p{k}/{}", combined as u8), b, 20.0, cfg, chars, *pos, combined, k == 1, false);
                }
            }
        }
    }
    // DALI: 16 zoom levels (a quarter of a frame to 20 frames) x 3 pans
    // x both views.
    let db = dali_bits();
    for z in 0..16 {
        let frames = 0.25 * 1.34f64.powi(z);
        for (k, pos) in [0.0, 0.41, 1.0].iter().enumerate() {
            for combined in [false, true] {
                dali_scene(ck, &format!("decode/dali/z{z}/p{k}/{}", combined as u8), &db, 10.0, frames, *pos, combined, k == 1);
            }
        }
    }
    dali_scene(ck, "decode/pic/dali", &db, 10.0, 3.2, 0.0, false, true);
    save(out, "d13-decode-dali");
    dali_scene(ck, "decode/pic/dali-err", &db, 10.0, 3.2, 0.33, false, true);
    save(out, "d13-decode-dali-err");
    // Pictures (and the manual's, manual.rs).
    scene(ck, "decode/pic/uart-near", &ub, 20.0, uart, 6.0, 0.30, false, false, false);
    save(out, "d01-decode-uart-near");
    scene(ck, "decode/pic/uart-far", &ub, 20.0, uart, 30.0, 0.30, false, false, false);
    save(out, "d01-decode-uart-far");
    scene(ck, "decode/pic/uart-bar", &ub, 20.0, uart, 300.0, 0.30, false, false, false);
    save(out, "d01-decode-uart-bar");
    scene(ck, "decode/pic/modbus", &mb, 20.0, modbus, 34.0, 0.0, false, false, false);
    save(out, "d02-decode-modbus");
    scene(ck, "decode/pic/modbus-crc", &mb, 20.0, modbus, 30.0, 0.17, false, true, false);
    save(out, "d02-decode-modbus-crc");
    let eb = bits(&uart_frames(), 0, 3, Some(20));
    scene(ck, "decode/pic/uart-fe", &eb, 20.0, uart, 6.0, 0.054, false, false, false);
    save(out, "d03-decode-uart-error");
    scene(ck, "decode/pic/stop-pan", &ub, 20.0, uart, 12.0, 0.62, false, true, false);
    save(out, "d04-decode-stop-pan");
    scene(ck, "decode/pic/combined", &ub, 20.0, uart, 8.0, 0.3, true, false, false);
    save(out, "d04-decode-combined");
    // The Wave view: 4800 baud at 38.4 kS/s (8 samples per bit), and 19200
    // there (the note).
    let wv = dc::Cfg { proto: dc::P_UART, baud: 4800, fmt: 0, inverted: false };
    scene(ck, "decode/pic/wave", &ub, 8.0, wv, 8.0, 0.3, false, false, true);
    save(out, "d05-decode-wave");
    let wf = dc::Cfg { proto: dc::P_UART, baud: 19200, fmt: 0, inverted: false };
    scene(ck, "decode/pic/wave-note", &ub, 2.0, wf, 8.0, 0.3, false, false, true);
    save(out, "d05-decode-wave-note");
    log_view(ck, out);
    // The setup popup over the page: every state, the full rules on its
    // labels and steppers (the option lists are the MF popup's widgets).
    let base = &stock::scenes()[0];
    for (k, v) in [
        ud::SetupVals { proto: 1, mode: 0, fmt: 0, inv: false, baud: 0, thr: f32::NAN, level: 1.65 },
        ud::SetupVals { proto: 2, mode: 1, fmt: 1, inv: true, baud: 115_200, thr: -99.99, level: f32::NAN },
        ud::SetupVals { proto: 0, mode: 0, fmt: 3, inv: false, baud: 300, thr: f32::NAN, level: f32::NAN },
        ud::SetupVals { proto: 2, mode: 1, fmt: 2, inv: true, baud: 76_800, thr: 12.34, level: 1.0 },
        ud::SetupVals { proto: 3, mode: 0, fmt: 0, inv: false, baud: 0, thr: f32::NAN, level: 8.0 },
        ud::SetupVals { proto: 3, mode: 0, fmt: 2, inv: true, baud: 9600, thr: -12.34, level: f32::NAN },
    ]
    .iter()
    .enumerate()
    {
        for combined in [false, true] {
            let mut m = base.m;
            m.combined = combined;
            let sc = stock::Scene { name: "dec-setup", m, popup: scope_ui::Popup::Mf(4), trace: base.trace.clone(), persist: None, cols: 399, cells: base.cells.clone() };
            let pv = scope_ui::PopVals { dec: *v, ..stock::popvals(&m) };
            stock::draw_with(&sc, &pv);
            let name = format!("decode/setup/{k}/{}", combined as u8);
            ck.check_replica(&name);
            let page = Bx::new(14, 61, 399, ud::PAGE_H as i32 - 1);
            let dali = v.proto == dc::P_DALI;
            let mut add: Vec<Bx> = ud::SU_LABELS[..if dali { 5 } else { 6 }].iter().map(|r| rb(*r)).collect();
            let thr = ud::thr_rects(dali);
            for r in thr[1..].iter() {
                add.push(rb(*r));
            }
            if dali {
                add.push(rb(ud::DL_RATE));
            } else {
                for r in [ud::SU_BMINUS, ud::SU_BVAL, ud::SU_BPLUS] {
                    add.push(rb(r));
                }
            }
            ck.check_add(&name, &[("page", page)], &add, Bx::new(13, 20, 401, 40 + ud::PAGE_H as i32));
            if k == 0 && !combined {
                save(out, "d06-decode-tab");
            }
            if k == 1 && !combined {
                save(out, "d07-decode-tab-log");
            }
            if k == 0 && combined {
                save(out, "d07-decode-tab-combined");
            }
            if k == 4 && !combined {
                save(out, "d07-decode-tab-dali");
            }

        }
    }
    // The five tabs on the other pages (Settings: the shortest page).
    for tab in [0u8, 3] {
        let m = base.m;
        let sc = stock::Scene { name: "mf-tabs", m, popup: scope_ui::Popup::Mf(tab), trace: base.trace.clone(), persist: None, cols: 399, cells: base.cells.clone() };
        stock::draw(&sc);
        save(out, &format!("d08-mf-tab{tab}"));
    }
}


/// The frame log (View::Decode, src/ui_log.rs): the list (UART and Modbus
/// rows as app_log.rs formats them, long ones cut with ">"), the top bar,
/// the counters, the panel; its setup popup. Texts must not collide.
pub fn log_view(ck: &mut Checker, out: &str) {
    use scope_ui::log as ul;
    for (name, modbus) in [("d09-log-uart", false), ("d10-log-modbus", true)] {
        log_screen(ck, name, modbus);
        save(out, name);
    }
    // The setup popup (SETUP): the Decode tab's page under a header.
    host::reset(0);
    ul::chrome(&mut || {});
    let v = ud::SetupVals { proto: 2, mode: 1, fmt: 1, inv: false, baud: 19200, thr: f32::NAN, level: 1.65 };
    ul::setup(&v);
    let mut add: Vec<Bx> = ud::SU_LABELS.iter().map(|r| rb(*r)).collect();
    for r in [ud::SU_BMINUS, ud::SU_BVAL, ud::SU_BPLUS, ud::SU_TMINUS, ud::SU_TVAL, ud::SU_TPLUS] {
        add.push(rb(r));
    }
    let b = ul::SU_BOX;
    ck.check_add("decode/log/setup", &[("page", Bx::new(14, 61, 399, ud::PAGE_H as i32 - 1))], &add, Bx::new(b.x as i32, b.y as i32, b.w as i32, b.h as i32));
    save(out, "d11-log-setup");
    log_snippet(ck, "d12-log-wave-uart", false);
    save(out, "d12-log-wave-uart");
    log_snippet(ck, "d12-log-wave-modbus", true);
    save(out, "d12-log-wave-modbus");
}

/// A frame's waveform in the log (app_log.rs `log_snip_show`): its row,
/// the snippet (8 samples a bit, 2 bits around the frame), the band.
fn log_snippet(ck: &mut Checker, name: &str, modbus: bool) {
    use scope_ui::log as ul;
    let (frame, cfg, fmt): (Vec<u8>, dc::Cfg, u8) = if modbus {
        (modbus_frames(false)[1].clone(), dc::Cfg { proto: dc::P_MODBUS, baud: 19200, fmt: 1, inverted: false }, 1)
    } else {
        (b"Hello, opendm40!".to_vec(), dc::Cfg { proto: dc::P_UART, baud: 9600, fmt: 0, inverted: false }, 0)
    };
    let b = bits(&[frame.clone()], fmt, 2, None);
    // From 2 bits before the start bit to 2 after the last stop bit.
    let b = b[18..b.len() - 58].to_vec();
    let c = codes(&b, 8.0);
    let n = c.len();
    host::reset(0);
    ul::chrome(&mut || {});
    ul::panel(false, true, &mut || {});
    let t = ul::Top { state: 2, proto: if modbus { "MODBUS" } else { "UART" }, rate: if modbus { "Auto 19200 8E1" } else { "Auto 9600 8N1" }, thr: "A 1.65V", inv: false, battery: 4, charging: false };
    for k in 0..ul::TOP_ITEMS {
        ul::top_item(k, &t);
    }
    let hex: Vec<String> = frame.iter().map(|x| format!("{x:02X}")).collect();
    let mut tb = [0u8; 64];
    let tn = ul::fit(&hex.join(" "), ul::text_w(!modbus), 0, &mut tb);
    let mut ab = [0u8; 64];
    let an = ul::fit_ascii(&String::from_utf8_lossy(&frame), &mut ab);
    let text = if modbus { "RSP 01 03 4B 3.1ms 1234 5678" } else { ul::txt(&tb[..tn]) };
    ul::row(0, Some(&ul::Row { time: "1.2346", text, ascii: if modbus { "" } else { ul::txt(&ab[..an]) }, status: "OK", fg: scope_ui::C_WHITE }));
    ul::snippet(&c);
    let tr = trace(&c, 0, n);
    fill(&c, 0, n, cfg.baud * 8, cfg, &tr);
    if modbus {
        // As app_log.rs: the frame box from the log's frame.
        let a = ud::ann();
        let mut head = [0u8; 8];
        head.copy_from_slice(&frame[..8]);
        a.f[0] = ud::FAnn { s0: 16, s1: (n - 16) as u32, kind: dc::K_RSP, fl: 0, len: frame.len() as u16, head, req: 0xFF };
        a.nf = 1;
    }

    ud::band(if modbus { 2 } else { 1 }, false);
    let v = [123, 0, 0, 0, 0, modbus as u32];
    for i in 0..4 {
        ul::cell_draw(i, &v);
    }
    ck.check_replica(&format!("decode/log/{name}"));
}

/// One frame-log screen (UART or Modbus rows), checked.
fn log_screen(ck: &mut Checker, name: &str, modbus: bool) {
    use scope_ui::log as ul;
    let fit = |s: &str, uart: bool| -> String {
        let mut b = [0u8; 64];
        let n = ul::fit(s, ul::text_w(uart), if uart { 0 } else { 1 }, &mut b);
        String::from_utf8_lossy(&b[..n]).into_owned()
    };
    {
        host::reset(0);
        ul::chrome(&mut || {});
        ul::panel(true, false, &mut || {});
        let t = ul::Top { state: 1, proto: if modbus { "MODBUS" } else { "UART" }, rate: if modbus { "Auto 19200 8E1" } else { "Auto 115200 8N1" }, thr: "A 1.65V", inv: false, battery: 4, charging: false };
        for k in 0..ul::TOP_ITEMS {
            ul::top_item(k, &t);
        }
        let rows: Vec<(String, String, String, &str, u16)> = if modbus {
            vec![
                ("0.0000".into(), fit("REQ 01 03 @0000 n=10", false), String::new(), "OK", scope_ui::C_WHITE),
                ("0.0121".into(), fit("RSP 01 03 20B 3.2ms 1234 5678 9ABC DEF0 0011 2233 4455 6677 8899 AABB", false), String::new(), "OK", scope_ui::C_WHITE),
                ("0.0503".into(), fit("REQ 11 06 @0005 =ABCD", false), String::new(), "OK", scope_ui::C_WHITE),
                ("0.0611".into(), fit("EXC 01 84 E02 2.9ms", false), String::new(), "OK", scope_ui::C_ORANGE),
                ("0.0712".into(), fit("01 03 00 00 00 02 3B 0B", false), String::new(), "CRC", scope_ui::C_WHITE),
            ]
        } else {
            vec![
                ("0.0000".into(), fit("48 65 6C 6C 6F 2C 20 6F 70 65 6E 64 6D 34 30 21", true), "Hello, opendm40!".into(), "OK", scope_ui::C_WHITE),
                ("0.0013".into(), fit("30 31 32 33 34 35 36 37 38 39 0D 0A", true), "0123456789..".into(), "OK", scope_ui::C_WHITE),
                ("1.2500".into(), fit("FF 00", true), "..".into(), "FE", scope_ui::C_WHITE),
                ("12.5000".into(), fit("41", true), "A".into(), "PE", scope_ui::C_WHITE),
                ("12345.67".into(), fit("4D 57 4D 57 4D 57", true), "MWMWMW".into(), "OK", scope_ui::C_WHITE),
            ]
        };
        for (i, r) in rows.iter().enumerate() {
            let mut a = [0u8; 64];
            let n = ul::fit_ascii(&r.2, &mut a);
            ul::row(i, Some(&ul::Row { time: &r.0, text: &r.1, ascii: ul::txt(&a[..n]), status: r.3, fg: r.4 }));
        }
        let v = [123_456, 2, 1, 3, 0, modbus as u32];
        for i in 0..4 {
            ul::cell_draw(i, &v);
        }
        if !modbus {
            ul::row(rows.len(), Some(&ul::Row { time: "", text: "Waiting for a signal (no swing)", ascii: "", status: "", fg: 0 }));
        }
        ck.check_replica(&format!("decode/log/{name}"));
    }
}

/// The manual's pictures (manual.rs): UART, Modbus, the Wave view.

pub fn manual(ck: &mut Checker, out: &str, save1: &dyn Fn(&str, &str)) {
    let ub = bits(&uart_frames(), 0, 3, None);
    let mb = bits(&modbus_frames(true), 1, 50, None);
    let uart = dc::Cfg { proto: dc::P_UART, baud: 115_200, fmt: 0, inverted: false };
    let modbus = dc::Cfg { proto: dc::P_MODBUS, baud: 19200, fmt: 1, inverted: false };
    scene(ck, "manual/scope-decode", &ub, 20.0, uart, 6.0, 0.30, false, false, false);
    save1(out, "scope-decode");
    scene(ck, "manual/scope-decode-modbus", &mb, 20.0, modbus, 30.0, 0.17, false, true, false);
    save1(out, "scope-decode-modbus");
    log_screen(ck, "manual/scope-decode-log", true);
    save1(out, "scope-decode-log");
    log_snippet(ck, "manual/scope-decode-log-wave", false);
    save1(out, "scope-decode-log-wave");

}


fn rb(r: stockfont::Rect) -> Bx {
    Bx::new(r.x as i32, r.y as i32, r.w as i32, r.h as i32)
}
