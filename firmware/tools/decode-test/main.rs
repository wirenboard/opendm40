//! Host test of src/decode.rs (UART / Modbus RTU decoded from a scope
//! record window) on synthetic sample streams: bit timing from 4 to 40
//! samples per bit and several phases, filtered edges (the Wave view's AD2
//! stream: a sinc^3-like smoothing over 3 samples, INFERRED shape),
//! auto-baud (standard and odd rates, too few samples per bit = TooFast),
//! the formats and parity, polarity, framing errors and breaks, noise and
//! glitches, baud mismatch, the levels (no signal, manual), a window cut
//! through characters and frames (partial), Modbus CRC / gap / pairing, the
//! frame text. Needs no hardware.
//!   tools/decode-test/run.sh
#[allow(dead_code)]
#[path = "../../src/pack10.rs"]
mod pack10;
#[allow(dead_code)]
#[path = "../../src/decode.rs"]
mod decode;
use decode::*;

static mut BAD: u32 = 0;
static mut N: u32 = 0;
fn t(name: &str, ok: bool) {
    unsafe {
        N += 1;
        if !ok {
            BAD += 1;
            println!("FAIL {name}");
        }
    }
}

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 16) as u32
    }
    fn f(&mut self) -> f64 {
        (self.next() % 20001) as f64 / 10000.0 - 1.0
    }
}

/// A line signal: segments of (seconds, mark).
struct Sig {
    seg: Vec<(f64, bool)>,
}

impl Sig {
    fn new() -> Sig {
        Sig { seg: Vec::new() }
    }
    fn idle(&mut self, s: f64) -> &mut Sig {
        self.seg.push((s, true));
        self
    }
    fn space(&mut self, s: f64) -> &mut Sig {
        self.seg.push((s, false));
        self
    }
    /// parity 3 = a wrong even bit; stop false = framing error.
    fn ch(&mut self, baud: f64, v: u16, bits: u8, parity: u8, stop: bool) -> &mut Sig {
        let b = 1.0 / baud;
        self.seg.push((b, false));
        let mut ones = 0;
        for i in 0..bits {
            let bit = v >> i & 1 == 1;
            ones += bit as u32;
            self.seg.push((b, bit));
        }
        match parity {
            1 => self.seg.push((b, ones % 2 == 1)),
            2 => self.seg.push((b, ones % 2 == 0)),
            3 => self.seg.push((b, ones % 2 == 0)),
            _ => {}
        }
        self.seg.push((b, stop));
        self
    }
    /// A DALI frame: start bit, `n` bits of `v` MSB first (a 1 low then
    /// high), each half bit `te` s; `bad`: bit `bad` without its centre
    /// transition (both halves low).
    fn dali(&mut self, te: f64, v: u32, n: u8, bad: Option<u8>) -> &mut Sig {
        self.seg.push((te, false));
        self.seg.push((te, true));
        for i in (0..n).rev() {
            let bit = v >> i & 1 == 1;
            if bad == Some(n - 1 - i) {
                self.seg.push((2.0 * te, false));
            } else {
                self.seg.push((te, !bit));
                self.seg.push((te, bit));
            }
        }
        self
    }
    fn bytes(&mut self, baud: f64, d: &[u8], fmt: u8) -> &mut Sig {
        let (bits, par) = [(8, 0), (8, 1), (8, 2), (7, 1)][fmt as usize];
        for &v in d {
            self.ch(baud, v as u16, bits, par, true);
        }
        self
    }
    /// Codes at fs; `filt`: each sample the mean of the line over the
    /// last `filt` sample periods (0: instantaneous).
    fn codes(&self, fs: f64, lo: f64, hi: f64, inv: bool, noise: f64, ph: f64, glitch: usize, filt: usize, rng: &mut Rng) -> Vec<u16> {
        let total: f64 = self.seg.iter().map(|s| s.0).sum();
        let n = ((total * fs) as usize).max(1);
        let level = |t: f64| -> f64 {
            let mut acc = 0.0;
            for s in &self.seg {
                if t < acc + s.0 {
                    return if s.1 != inv { hi } else { lo };
                }
                acc += s.0;
            }
            if self.seg.last().unwrap().1 != inv { hi } else { lo }
        };
        // Segment starts for a fast lookup.
        let mut starts = Vec::with_capacity(self.seg.len());
        let mut acc = 0.0;
        for s in &self.seg {
            starts.push(acc);
            acc += s.0;
        }
        let fast = |t: f64| -> f64 {
            let k = match starts.binary_search_by(|x| x.partial_cmp(&t).unwrap()) {
                Ok(k) => k,
                Err(k) => k.saturating_sub(1),
            };
            if self.seg[k.min(self.seg.len() - 1)].1 != inv { hi } else { lo }
        };
        let _ = level;
        let mut out = Vec::with_capacity(n);
        for i in 0..n {
            let t = (i as f64 + ph) / fs;
            let mut v = if filt == 0 {
                fast(t)
            } else {
                // sinc^3-like: a triangle-weighted mean over filt periods.
                let m = 12 * filt;
                let (mut sw, mut s) = (0.0, 0.0);
                for k in 0..m {
                    let tt = t - k as f64 / (12.0 * fs);
                    let wgt = 1.0 - ((k as f64 + 0.5) / m as f64 - 0.5).abs() * 2.0 + 0.05;
                    s += wgt * fast(tt.max(0.0));
                    sw += wgt;
                }
                s / sw
            };
            v += noise * rng.f();
            if glitch > 0 && i % glitch == glitch / 2 {
                v = if v > (lo + hi) / 2.0 { lo } else { hi };
            }
            out.push(v.round().clamp(0.0, 1023.0) as u16);
        }
        out
    }
}

struct Res {
    chars: Vec<Ch>,
    frames: Vec<(Fr, u32)>,
    baud: Result<u32, NoRate>,
}

/// Decode `c[a..b]` as the firmware does (levels from the window, the
/// rate set or found).
fn decode(c: &[u16], a: usize, b: usize, fs: u32, cfg: Cfg, manual: Option<u16>) -> Res {
    let (mn, mx) = min_max(c, a, b);
    let Some((lo, mid, hi)) = levels(mn, mx, manual) else {
        return Res { chars: vec![], frames: vec![], baud: Err(NoRate::Few) };
    };
    let baud = if cfg.baud != 0 { Ok(cfg.baud) } else { auto_baud(c, a, b, lo, hi, fs) };
    let Ok(bd) = baud else { return Res { chars: vec![], frames: vec![], baud } };
    let (bits, parity) = cfg.bits();
    let l = Line { lo, mid, hi, inv: cfg.inverted, spb: (fs as u64 * 256 / bd as u64) as u32, bits, parity };
    let mut chars = vec![];
    let mut frames = vec![];
    let mut fr = Framer::new(cfg.proto == P_MODBUS, l.char_samples(), fs, bd, a as u32, b as u32);
    decode::chars(&c, a, b, &l, &mut |ch| {
        chars.push(ch);
        fr.push(ch, &mut |f, r| frames.push((*f, r)));
    });
    fr.finish(&mut |f, r| frames.push((*f, r)));
    Res { chars, frames, baud }
}

/// DALI frames of `c[a..b]` (levels from the window; the scan in slices
/// of `slice` samples).
fn dali(c: &[u16], a: usize, b: usize, fs: u32, inv: bool, slice: usize) -> Vec<Fr> {
    let (mn, mx) = min_max(c, a, b);
    let Some((lo, mid, hi)) = levels(mn, mx, None) else { return vec![] };
    let l = Line { lo, mid, hi, inv, spb: (fs as u64 * 256 / DALI_BAUD as u64) as u32, bits: 8, parity: 0 };
    let mut d = Dali::at(a);
    let mut v = vec![];
    let mut guard = 0;
    while !d.done && guard < 1_000_000 {
        let lim = d.n.saturating_add(slice);
        dali_step(&c, b, lim, &l, &mut d, &mut |f| {
            v.push(*f);
            true
        });
        guard += 1;
    }
    v
}

fn dtext(f: &Fr) -> String {
    let mut b = [0u8; 40];
    let mut w = W { b: &mut b, n: 0 };
    dali_text(&mut w, f, false);
    w.as_str().to_string()
}

fn dtexts(v: &[Fr]) -> Vec<String> {
    v.iter().map(dtext).collect()
}

fn uart(baud: u32) -> Cfg {
    Cfg { proto: P_UART, baud, fmt: 0, inverted: false }
}

fn bytes_of(r: &Res) -> Vec<u8> {
    r.chars.iter().map(|c| c.v as u8).collect()
}

fn fe(r: &Res) -> usize {
    r.chars.iter().filter(|c| c.err & F_FE != 0).count()
}

fn contains(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len()).any(|w| w == needle)
}

fn mb(d: &[u8]) -> Vec<u8> {
    let mut v = d.to_vec();
    let c = crc16(d);
    v.push(c as u8);
    v.push((c >> 8) as u8);
    v
}

fn main() {
    let mut rng = Rng(0x1234_5678_9abc_def1);
    let text = b"Hello, opendm40! 0123456789\r\n";

    // ---- set rate, 8N1, samples per bit 4 .. 40, phases ----
    for &(baud, fs) in &[(115_200u32, 1_250_000u32), (115_200, 2_500_000), (115_200, 500_000), (9600, 125_000), (9600, 50_000), (4800, 38_400), (2400, 38_400), (300, 12_500)] {
        for ph in [0.0, 0.3, 0.7] {
            let mut s = Sig::new();
            s.idle(12.0 / baud as f64).bytes(baud as f64, text, 0).idle(30.0 / baud as f64);
            let c = s.codes(fs as f64, 300.0, 700.0, false, 4.0, ph, 0, 0, &mut rng);
            let r = decode(&c, 0, c.len(), fs, uart(baud), None);
            let spb = fs as f64 / baud as f64;
            t(&format!("8N1 {baud} at {fs} ({spb:.1} samples/bit) ph {ph}: text, no FE"), bytes_of(&r) == text && fe(&r) == 0);
        }
    }

    // ---- chars_step in slices (the firmware's decode over main-loop
    // passes) gives what one chars call gives ----
    for &(baud, fs, slice) in &[(9600u32, 125_000u32, 7usize), (9600, 125_000, 100), (115_200, 1_250_000, 33), (115_200, 1_250_000, 8192), (2400, 38_400, 1)] {
        let mut s = Sig::new();
        s.idle(12.0 / baud as f64).bytes(baud as f64, text, 0).idle(3.0 / baud as f64).bytes(baud as f64, text, 0).idle(30.0 / baud as f64);
        let c = s.codes(fs as f64, 300.0, 700.0, false, 4.0, 0.3, 0, 0, &mut rng);
        let (lo, mid, hi) = levels(300, 700, None).unwrap();
        let l = Line { lo, mid, hi, inv: false, spb: (fs as u64 * 256 / baud as u64) as u32, bits: 8, parity: 0 };
        let (a, b) = (5, c.len() - 3);
        let mut one = vec![];
        decode::chars(&c, a, b, &l, &mut |ch| one.push((ch.start, ch.end, ch.v, ch.err)));
        let mut many = vec![];
        let mut cur = decode::Cursor::at(a);
        let mut passes = 0;
        while !cur.done && passes < 1_000_000 {
            let lim = cur.n + slice;
            decode::chars_step(&c, a, b, lim, &l, &mut cur, &mut |ch| {
                many.push((ch.start, ch.end, ch.v, ch.err));
                true
            });
            passes += 1;
        }
        t(&format!("chars_step slices of {slice} at {baud} / {fs}: {} chars in {passes} passes, as one chars call", many.len()), one == many && one.len() == 2 * text.len());
        // Stopping (f returns false) ends the scan at that character.
        let mut cur = decode::Cursor::at(a);
        let mut got = 0;
        decode::chars_step(&c, a, b, b, &l, &mut cur, &mut |_| {
            got += 1;
            got < 5
        });
        t(&format!("chars_step stop after 5 at {baud}"), got == 5 && cur.done);
    }

    // ---- filtered edges (the Wave view's AD2 stream) ----
    for &(baud, fs, filt) in &[(4800u32, 38_400u32, 3usize), (2400, 38_400, 3), (9600, 76_800, 3), (4800, 38_400, 1)] {
        let mut s = Sig::new();
        s.idle(15.0 / baud as f64).bytes(baud as f64, text, 0).idle(30.0 / baud as f64);
        let c = s.codes(fs as f64, 300.0, 700.0, false, 3.0, 0.4, 0, filt, &mut rng);
        let r = decode(&c, 0, c.len(), fs, uart(0), None);
        t(&format!("filtered ({filt}) {baud} at {fs}: auto-baud {:?}, text", r.baud), r.baud == Ok(baud) && bytes_of(&r) == text);
    }
    {
        // 9600 at 38 400 (4 samples per bit), filtered: decodes or says
        // TooFast, never wrong bytes without an error flag.
        let (baud, fs) = (9600u32, 38_400u32);
        let mut s = Sig::new();
        s.idle(15.0 / baud as f64).bytes(baud as f64, text, 0).idle(30.0 / baud as f64);
        let c = s.codes(fs as f64, 300.0, 700.0, false, 3.0, 0.4, 0, 3, &mut rng);
        let r = decode(&c, 0, c.len(), fs, uart(0), None);
        let wrong = r.chars.iter().zip(text.iter()).filter(|(c, &b)| c.v as u8 != b && c.err == 0).count();
        println!("  9600 at 38400 filtered: auto {:?}, {} chars, {} wrong unflagged", r.baud, r.chars.len(), wrong);
        t("9600 at 38.4 kS/s filtered: text or TooFast, no silent garbage", bytes_of(&r) == text || r.baud == Err(NoRate::TooFast) || wrong == 0);
        // 19200 at 38 400 (2 samples per bit): TooFast.
        let mut s = Sig::new();
        s.idle(15.0 / 19200.0).bytes(19200.0, text, 0).idle(30.0 / 19200.0);
        let c = s.codes(fs as f64, 300.0, 700.0, false, 3.0, 0.4, 0, 3, &mut rng);
        let r = decode(&c, 0, c.len(), fs, uart(0), None);
        t(&format!("19200 at 38.4 kS/s: auto-baud TooFast ({:?})", r.baud), r.baud == Err(NoRate::TooFast));
        // 115200 at 125 kS/s (1 sample per bit): TooFast.
        let mut s = Sig::new();
        s.idle(0.001).bytes(115_200.0, text, 0).idle(0.001);
        let c = s.codes(125_000.0, 300.0, 700.0, false, 3.0, 0.0, 0, 0, &mut rng);
        let r = decode(&c, 0, c.len(), 125_000, uart(0), None);
        t(&format!("115200 at 125 kS/s: TooFast ({:?})", r.baud), r.baud == Err(NoRate::TooFast));
    }

    // ---- auto-baud ----
    for &(baud, fs) in &[(2400u32, 125_000u32), (9600, 250_000), (19200, 500_000), (38400, 1_250_000), (57600, 1_250_000), (115_200, 2_500_000)] {
        let mut s = Sig::new();
        s.idle(5.0 / baud as f64).bytes(baud as f64, text, 0).idle(30.0 / baud as f64);
        let c = s.codes(fs as f64, 250.0, 750.0, false, 6.0, 0.4, 0, 0, &mut rng);
        let r = decode(&c, 0, c.len(), fs, uart(0), None);
        t(&format!("auto-baud {baud} at {fs}: {:?}, text", r.baud), r.baud == Ok(baud) && bytes_of(&r) == text);
    }
    {
        let mut s = Sig::new();
        s.idle(0.0001).bytes(100_000.0, text, 0).idle(0.0005);
        let c = s.codes(2.5e6, 250.0, 750.0, false, 3.0, 0.0, 0, 0, &mut rng);
        let r = decode(&c, 0, c.len(), 2_500_000, uart(0), None);
        t(&format!("auto-baud 100000 (odd): {:?}, text", r.baud), r.baud.is_ok_and(|b| b.abs_diff(100_000) < 3000) && bytes_of(&r) == text);
        // One character only: too few edges.
        let mut s = Sig::new();
        s.idle(0.0001).bytes(115_200.0, b"\xFF", 0).idle(0.0005);
        let c = s.codes(2.5e6, 250.0, 750.0, false, 3.0, 0.0, 0, 0, &mut rng);
        let r = decode(&c, 0, c.len(), 2_500_000, uart(0), None);
        t(&format!("one 0xFF character: auto-baud Few ({:?})", r.baud), r.baud == Err(NoRate::Few));
    }

    // ---- formats, parity, framing, break ----
    for fmt in 0..4u8 {
        let (baud, fs) = (19200u32, 500_000u32);
        let data: Vec<u8> = if fmt == 3 { text.iter().map(|b| b & 0x7F).collect() } else { (0..=255u8).step_by(7).collect() };
        let mut s = Sig::new();
        s.idle(0.0005).bytes(baud as f64, &data, fmt).idle(0.002);
        let c = s.codes(fs as f64, 200.0, 800.0, false, 3.0, 0.3, 0, 0, &mut rng);
        let r = decode(&c, 0, c.len(), fs, Cfg { proto: P_UART, baud, fmt, inverted: false }, None);
        t(&format!("{}: data, no errors", FMT_NAMES[fmt as usize]), bytes_of(&r) == data && r.chars.iter().all(|c| c.err == 0));
    }
    {
        let (baud, fs) = (9600u32, 250_000u32);
        let mut s = Sig::new();
        s.idle(0.001);
        for i in 0..30u16 {
            s.ch(baud as f64, 0x40 + i, 8, if i % 3 == 0 { 3 } else { 1 }, true);
        }
        s.idle(0.005);
        let c = s.codes(fs as f64, 200.0, 800.0, false, 3.0, 0.0, 0, 0, &mut rng);
        let r = decode(&c, 0, c.len(), fs, Cfg { proto: P_UART, baud, fmt: 1, inverted: false }, None);
        let pe = r.chars.iter().filter(|c| c.err & F_PE != 0).count();
        t(&format!("8E1 wrong parity on 10 of 30: PE {pe}"), pe == 10 && r.chars.len() == 30);
        let mut s = Sig::new();
        s.idle(0.001).bytes(baud as f64, b"AB", 0).ch(baud as f64, 0x55, 8, 0, false).idle(0.001).space(0.01).idle(0.002).bytes(baud as f64, b"CD", 0).idle(0.005);
        let c = s.codes(fs as f64, 200.0, 800.0, false, 3.0, 0.0, 0, 0, &mut rng);
        let r = decode(&c, 0, c.len(), fs, uart(baud), None);
        let b = bytes_of(&r);
        t(&format!("stop bit low + a break: 2 FE ({}), AB .. CD ({b:02X?})", fe(&r)), fe(&r) == 2 && b.starts_with(b"AB") && b.ends_with(b"CD") && b.len() == 6);
    }

    // ---- polarity, noise, mismatch, levels ----
    {
        let (baud, fs) = (9600u32, 250_000u32);
        let mut s = Sig::new();
        s.idle(0.001).bytes(baud as f64, text, 0).idle(0.005);
        let c = s.codes(fs as f64, 0.0, 1023.0, true, 5.0, 0.7, 0, 0, &mut rng);
        let r = decode(&c, 0, c.len(), fs, Cfg { proto: P_UART, baud, fmt: 0, inverted: true }, None);
        t("inverted (RS-232 levels, clipped): text", bytes_of(&r) == text && fe(&r) == 0);
        let r = decode(&c, 0, c.len(), fs, uart(baud), None);
        t(&format!("the same as normal polarity: no text, FE {}", fe(&r)), !contains(&bytes_of(&r), b"Hello") && fe(&r) > 0);
        // Noise +-25 % of the swing, 1-sample glitches.
        let mut s = Sig::new();
        s.idle(0.001);
        for _ in 0..10 {
            s.bytes(115_200.0, text, 0);
        }
        s.idle(0.001);
        let c = s.codes(1.25e6, 300.0, 700.0, false, 100.0, 0.2, 997, 0, &mut rng);
        let r = decode(&c, 0, c.len(), 1_250_000, uart(115_200), None);
        let good = bytes_of(&r).windows(text.len()).filter(|w| *w == text).count();
        t(&format!("noise +-25 % + glitches: >= 9 of 10 lines ({good})"), good >= 9);
        // Baud mismatch.
        for (sig, set) in [(9600u32, 19200u32), (19200, 9600), (9600, 10400)] {
            let mut s = Sig::new();
            s.idle(0.001);
            for _ in 0..3 {
                s.bytes(sig as f64, text, 0).idle(0.002);
            }
            let c = s.codes(500_000.0, 250.0, 750.0, false, 3.0, 0.0, 0, 0, &mut rng);
            let r = decode(&c, 0, c.len(), 500_000, uart(set), None);
            t(&format!("signal {sig}, set {set}: FE ({}), no text", fe(&r)), fe(&r) > 5 && !contains(&bytes_of(&r), b"Hello"));
        }
        // No signal; manual level.
        let c: Vec<u16> = (0..50_000).map(|_| (512.0 + 8.0 * rng.f()).round() as u16).collect();
        t("open input (+-8 codes): no levels", levels(min_max(&c, 0, c.len()).0, min_max(&c, 0, c.len()).1, None).is_none());
        let mut s = Sig::new();
        s.idle(0.0002).bytes(115_200.0, text, 0).idle(0.0005);
        let c = s.codes(1.25e6, 512.0, 609.0, false, 4.0, 0.0, 0, 0, &mut rng);
        let r = decode(&c, 0, c.len(), 1_250_000, uart(115_200), Some(560));
        t("manual level 560 on a 97-code swing: text", bytes_of(&r) == text);
        let r = decode(&c, 0, c.len(), 1_250_000, uart(115_200), Some(700));
        t("manual level above the signal: nothing", r.chars.is_empty());
    }

    // ---- a window cut through the traffic (zoom / pan) ----
    {
        let (baud, fs) = (9600u32, 250_000u32);
        let mut s = Sig::new();
        s.idle(0.0005);
        for _ in 0..6 {
            s.bytes(baud as f64, text, 0).idle(0.0003);
        }
        let c = s.codes(fs as f64, 250.0, 750.0, false, 3.0, 0.0, 0, 0, &mut rng);
        let cs = (fs as f64 * 10.0 / baud as f64) as usize;
        for &(a, b) in &[(cs * 7 + cs / 3, cs * 40), (cs * 3 / 2, c.len() - cs * 5 / 2)] {
            let r = decode(&c, a, b, fs, uart(baud), None);
            // Every character reported lies wholly in the window; a window
            // that starts inside a character may misread the first ones
            // (back-to-back characters realign at the next idle gap: the
            // firmware decodes 16 characters before the screen), none after 16.
            let inside = r.chars.iter().all(|ch| ch.start as usize >= a && (ch.end as usize) < b);
            let late = r.chars.iter().skip(16).filter(|c| c.err != 0).count();
            if late != 0 {
                let e: Vec<(usize, u32, u8)> = r.chars.iter().enumerate().filter(|(_, c)| c.err != 0).map(|(i, c)| (i, c.start, c.v as u8)).collect();
                println!("  errors at {e:?} of {}", r.chars.len());
            }
            t(&format!("window {a}..{b}: {} characters, all inside, no error after the first 16", r.chars.len()), inside && late == 0 && !r.chars.is_empty());
        }
    }

    // ---- Modbus RTU ----
    let req = mb(&[0x01, 0x03, 0x00, 0x00, 0x00, 0x0A]);
    let mut rsp_d = vec![0x01, 0x03, 0x14];
    for i in 0..10u16 {
        rsp_d.push((i >> 8) as u8);
        rsp_d.push(i as u8 + 0x30);
    }
    let rsp = mb(&rsp_d);
    let w6 = mb(&[0x11, 0x06, 0x00, 0x05, 0x12, 0x34]);
    let w16 = mb(&[0x11, 0x10, 0x00, 0x10, 0x00, 0x02, 0x04, 0x00, 0x0A, 0x01, 0x02]);
    let w16r = mb(&[0x11, 0x10, 0x00, 0x10, 0x00, 0x02]);
    let exr = mb(&[0x01, 0x04, 0x00, 0x00, 0x00, 0x01]);
    let exc = mb(&[0x01, 0x84, 0x02]);
    let mut bad = req.clone();
    bad[7] ^= 0x55;
    for &(baud, fmt, fs, filt) in &[(9600u32, 1u8, 250_000u32, 0usize), (19200, 0, 500_000, 0), (115_200, 1, 2_500_000, 0), (4800, 1, 38_400, 3), (2400, 0, 38_400, 3)] {
        let cfg = Cfg { proto: P_MODBUS, baud: 0, fmt, inverted: false };
        let cht = (if fmt == 0 { 10.0 } else { 11.0 }) / baud as f64;
        let t35 = if baud > 19200 { 1.75e-3 } else { 3.5 * cht };
        let seq = [&req, &rsp, &w6, &w6, &w16, &w16r, &exr, &exc, &bad];
        let mut s = Sig::new();
        s.idle(t35 * 2.0);
        for (i, fr) in seq.iter().enumerate() {
            s.bytes(baud as f64, fr, fmt);
            s.idle(if i % 2 == 0 { t35 * 1.3 } else { t35 * 2.5 });
        }
        let c = s.codes(fs as f64, 250.0, 750.0, false, 5.0, 0.1, 0, filt, &mut rng);
        let r = decode(&c, 0, c.len(), fs, cfg, None);
        let kinds: Vec<u8> = r.frames.iter().map(|f| f.0.kind).collect();
        let want = vec![K_REQ, K_RSP, K_REQ, K_RSP, K_REQ, K_RSP, K_REQ, K_EXC, K_MB];
        t(&format!("Modbus {baud} {} (filt {filt}): rate {:?}, kinds {kinds:?}", FMT_NAMES[fmt as usize], r.baud), kinds == want);
        if kinds != want {
            continue;
        }
        let f = &r.frames;
        t("Modbus: lengths", f[0].0.len == 8 && f[1].0.len as usize == rsp.len() && f[4].0.len as usize == w16.len());
        t("Modbus: CRC bad on the last only", f.iter().enumerate().all(|(i, x)| (x.0.flags & F_CRC != 0) == (i == 8)));
        t("Modbus: replies point at their requests", f[1].1 == 0 && f[3].1 == 2 && f[5].1 == 4 && f[7].1 == 6 && f[0].1 == u32::MAX);
        let mut b = [0u8; 64];
        let mut w = W { b: &mut b, n: 0 };
        mb_text(&mut w, &f[0].0);
        t(&format!("Modbus text: {:?}", w.as_str()), w.as_str() == "REQ 01 03 @0000 n=10");
        let mut w = W { b: &mut b, n: 0 };
        mb_text(&mut w, &f[1].0);
        t(&format!("Modbus text: {:?}", w.as_str()), w.as_str() == "RSP 01 03 20B");
        let mut w = W { b: &mut b, n: 0 };
        mb_text(&mut w, &f[3].0);
        t(&format!("Modbus text: {:?}", w.as_str()), w.as_str() == "RSP 11 06 @0005 =1234");
        let mut w = W { b: &mut b, n: 0 };
        mb_text(&mut w, &f[7].0);
        t(&format!("Modbus text: {:?}", w.as_str()), w.as_str() == "EXC 01 84 E02");
        // A window starting in the reply and ending in the 16-request: partial ends.
        let a = (f[1].0.start + f[1].0.end) as usize / 2;
        let bb = (f[4].0.start + f[4].0.end) as usize / 2;
        let r2 = decode(&c, a, bb, fs, Cfg { baud: r.baud.unwrap(), ..cfg }, None);
        let k2: Vec<u8> = r2.frames.iter().map(|f| f.0.kind).collect();
        let part = |x: &(Fr, u32)| x.0.flags & F_PART != 0;
        t(&format!("Modbus window cut: first / last partial, the middle decoded ({k2:?})"), r2.frames.len() >= 3 && part(&r2.frames[0]) && part(r2.frames.last().unwrap()) && k2[1] == K_REQ && k2[2] == K_RSP);
    }
    {
        // A 1-character gap inside a frame does not split it; 4 do.
        let (baud, fs) = (9600u32, 250_000u32);
        let cht = 10.0 / baud as f64;
        let mut s = Sig::new();
        s.idle(cht * 5.0).bytes(baud as f64, &req[..4], 0).idle(cht).bytes(baud as f64, &req[4..], 0).idle(cht * 4.0).bytes(baud as f64, &rsp, 0).idle(cht * 5.0);
        let c = s.codes(fs as f64, 250.0, 750.0, false, 2.0, 0.0, 0, 0, &mut rng);
        let r = decode(&c, 0, c.len(), fs, Cfg { proto: P_MODBUS, baud, fmt: 0, inverted: false }, None);
        t(&format!("Modbus: 1-char gap kept, 4 split ({} frames)", r.frames.len()), r.frames.len() == 2 && r.frames[0].0.kind == K_REQ && r.frames[1].0.kind == K_RSP);
        // UART lines: split on 2 characters' gap, 16 bytes at most.
        let mut s = Sig::new();
        s.idle(cht * 3.0).bytes(baud as f64, b"abc", 0).idle(cht * 0.5).bytes(baud as f64, b"def", 0).idle(cht * 3.0).bytes(baud as f64, &[b'x'; 40], 0).idle(cht * 4.0);
        let c = s.codes(fs as f64, 250.0, 750.0, false, 2.0, 0.0, 0, 0, &mut rng);
        let r = decode(&c, 0, c.len(), fs, uart(baud), None);
        let lens: Vec<u16> = r.frames.iter().map(|f| f.0.len).collect();
        t(&format!("UART lines: abcdef | 16 + 16 + 8 ({lens:?})"), lens == vec![6, 16, 16, 8]);
    }
    // ---- DALI: forward 16, backward 8, 24-bit; timing; glitches ----
    {
        let te = 1.0 / 2400.0;
        let want = ["FF16:FE 80 BC DAPC", "BF8:FF", "FF16:A3 12 SPC", "FF24:C1 A2 B3", "FF16:05 00 S2 CMD", "FF16:9B 21 G13 CMD", "FF16:FD 05 BCU CMD"];
        let frames = |s: &mut Sig, k: f64| {
            s.idle(10.0 * te).dali(te * k, 0xFE80, 16, None).idle(10.0 * te);
            s.dali(te * k, 0xFF, 8, None).idle(30.0 * te);
            s.dali(te * k, 0xA312, 16, None).idle(22.0 * te);
            s.dali(te * k, 0xC1A2B3, 24, None).idle(22.0 * te);
            s.dali(te * k, 0x0500, 16, None).idle(22.0 * te);
            s.dali(te * k, 0x9B21, 16, None).idle(22.0 * te);
            s.dali(te * k, 0xFD05, 16, None).idle(12.0 * te);
        };
        // Samples per Te 4 .. 40 (the scope's 10 .. 100 kS/s), phases,
        // the rate -10 % / +10 %, inverted (the probe on the opto side).
        for &fs in &[9_600u32, 25_000, 50_000, 96_000] {
            for &k in &[1.0, 0.9, 1.1] {
                for (ph, inv) in [(0.0, false), (0.6, false), (0.3, true)] {
                    let mut s = Sig::new();
                    frames(&mut s, k);
                    let c = s.codes(fs as f64, 100.0, 800.0, inv, 6.0, ph, 0, 0, &mut rng);
                    let v = dali(&c, 0, c.len(), fs, inv, 777);
                    let got = dtexts(&v);
                    t(&format!("DALI at {fs} S/s, Te x{k} ph {ph} inv {inv}: {got:?}"), got == want && v.iter().all(|f| f.flags == 0 && f.kind == K_DALI));
                }
            }
        }
        // Positions: the forward frame from its start edge to its stop bits' end.
        {
            let fs = 50_000u32;
            let mut s = Sig::new();
            s.idle(10.0 * te).dali(te, 0xFE80, 16, None).idle(10.0 * te);
            let c = s.codes(fs as f64, 100.0, 800.0, false, 0.0, 0.0, 0, 0, &mut rng);
            let v = dali(&c, 0, c.len(), fs, false, usize::MAX / 4);
            let (s0, s1) = (10.0 * te * fs as f64, (10.0 + 38.0) * te * fs as f64);
            t(&format!("DALI frame span {:?} ~ {s0:.0}..{s1:.0}", v.first().map(|f| (f.start, f.end))), v.len() == 1 && (v[0].start as f64 - s0).abs() <= 2.0 && (v[0].end as f64 - s1).abs() <= 3.0);
        }
        // Off timing: Te x0.75 / x1.35 / x0.6 (outside the windows) is
        // ERR (x1.35: its 2.7 Te high runs also end the frame: several),
        // never bytes.
        for &k in &[0.75, 1.35, 0.6] {
            let mut s = Sig::new();
            s.idle(10.0 * te).dali(te * k, 0xFE80, 16, None).idle(12.0 * te);
            let c = s.codes(50_000.0, 100.0, 800.0, false, 2.0, 0.0, 0, 0, &mut rng);
            let got = dtexts(&dali(&c, 0, c.len(), 50_000, false, 500));
            t(&format!("DALI Te x{k}: ERR ({got:?})"), !got.is_empty() && got.iter().all(|g| g == "ERR"));
        }
        // Bad Manchester (no centre transition), a glitch inside a frame,
        // a 12-bit frame: ERR; the next frame decodes.
        for case in 0..3 {
            let fs = 50_000.0;
            let mut s = Sig::new();
            s.idle(10.0 * te);
            match case {
                0 => {
                    s.dali(te, 0xFE80, 16, Some(5));
                }
                1 => {
                    s.dali(te, 0xFE80, 16, None);
                }
                _ => {
                    s.dali(te, 0xABC, 12, None);
                }
            }
            s.idle(22.0 * te).dali(te, 0xFF, 8, None).idle(12.0 * te);
            let mut c = s.codes(fs, 100.0, 800.0, false, 2.0, 0.0, 0, 0, &mut rng);
            if case == 1 {
                // A 60 us spike low in the 9th bit's high half.
                let i0 = ((10.0 + 2.0 + 16.0 + 1.4) * te * fs) as usize;
                let mut first = c[i0] > 450;
                if !first {
                    // Not a high half there: the next one.
                    let i1 = i0 + (te * fs) as usize;
                    first = c[i1] > 450;
                    for x in &mut c[i1..i1 + 3] {
                        *x = 100;
                    }
                } else {
                    for x in &mut c[i0..i0 + 3] {
                        *x = 100;
                    }
                }
                t("DALI glitch case: a high half found", first);
            }
            let got = dtexts(&dali(&c, 0, c.len(), 50_000, false, 300));
            t(&format!("DALI error case {case}: {got:?}"), got == ["ERR", "BF8:FF"]);
        }
        // A glitch on the idle line (a 60 us low spike): ignored; a long
        // idle gap between frames; a frame cut by the window's end is not
        // reported, one begun before the window not decoded as data.
        {
            let fs = 50_000.0;
            let mut s = Sig::new();
            s.idle(10.0 * te).dali(te, 0xFE80, 16, None).idle(0.5);
            s.dali(te, 0x0121, 16, None).idle(22.0 * te).dali(te, 0xFF00, 16, None).idle(12.0 * te);
            let mut c = s.codes(fs, 100.0, 800.0, false, 2.0, 0.0, 0, 0, &mut rng);
            let g = ((10.0 + 38.0 + 30.0) * te * fs) as usize;
            for x in &mut c[g..g + 3] {
                *x = 100;
            }
            let got = dtexts(&dali(&c, 0, c.len(), 50_000, false, 4096));
            t(&format!("DALI idle glitch, 0.5 s gap: {got:?}"), got == ["FF16:FE 80 BC DAPC", "FF16:01 21 S0 CMD", "FF16:FF 00 BC CMD"]);
            // The window from the first frame's middle to the last one's.
            let (a, b) = (((10.0 + 15.0) * te * fs) as usize, c.len() - (20.0 * te * fs) as usize);
            let got = dtexts(&dali(&c, a, b, 50_000, false, 4096));
            t(&format!("DALI window cut: {got:?}"), got == ["FF16:01 21 S0 CMD"]);
        }
        // The line stuck low inside a frame: ERR once it goes high again.
        {
            let mut s = Sig::new();
            s.idle(10.0 * te).dali(te, 0xFE, 8, None).space(20.0 * te).idle(12.0 * te).dali(te, 0x42, 8, None).idle(12.0 * te);
            let c = s.codes(50_000.0, 100.0, 800.0, false, 2.0, 0.0, 0, 0, &mut rng);
            let got = dtexts(&dali(&c, 0, c.len(), 50_000, false, 4096));
            t(&format!("DALI stuck low: {got:?}"), got == ["ERR", "BF8:42"]);
        }
        // Too few samples per half bit (5 kS/s: 2): nothing.
        {
            let mut s = Sig::new();
            frames(&mut s, 1.0);
            let c = s.codes(5_000.0, 100.0, 800.0, false, 2.0, 0.0, 0, 0, &mut rng);
            t("DALI at 5 kS/s: nothing", dali(&c, 0, c.len(), 5_000, false, 4096).is_empty());
        }
        let mut b = [0u8; 40];
        let mut w = W { b: &mut b, n: 0 };
        dali_text(&mut w, &Fr { start: 0, end: 0, len: 2, head: [0xFE, 0x80, 0, 0, 0, 0, 0, 0], flags: 0, kind: K_DALI }, true);
        t(&format!("DALI short text {:?}", w.as_str()), w.as_str() == "FF16:FE 80");
    }

    t("crc16 of 01 03 00 00 00 0A = 0xCDC5", crc16(&[1, 3, 0, 0, 0, 0x0A]) == 0xCDC5);
    {
        let mut b = [0u8; 32];
        let mut w = W { b: &mut b, n: 0 };
        w.secs_ns(-1_234_567);
        t(&format!("secs_ns: {:?}", w.as_str()), w.as_str() == "-0.001234567");
    }

    // ---- speed (host, orders of magnitude) ----
    {
        let mut s = Sig::new();
        s.idle(0.0001);
        for _ in 0..40 {
            s.bytes(115_200.0, text, 0);
        }
        let c = s.codes(2.5e6, 250.0, 750.0, false, 2.0, 0.0, 0, 0, &mut rng);
        let t0 = std::time::Instant::now();
        let r = decode(&c, 0, c.len(), 2_500_000, uart(0), None);
        let dt = t0.elapsed().as_secs_f64();
        println!("  speed: {:.1} Msamples/s on the host ({} samples, {} chars)", c.len() as f64 / dt / 1e6, c.len(), r.chars.len());
    }

    let (n, bad) = unsafe { (N, BAD) };
    println!("decode-test: {n} checks, {bad} failed");
    std::process::exit(if bad == 0 { 0 } else { 1 });
}
