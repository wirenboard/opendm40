//! Host test of src/bt_scpi.rs: the Bluetooth RX dispatcher (stock binary
//! frames and SCPI lines on one byte stream), the untrusted-transport command
//! filter, and the TX ring. No hardware. run.sh builds and runs it.
//!
//! The dispatcher must give the same events however the stream is split
//! across reads (the RX DMA ring is drained every ~5 ms, BT delivers RFCOMM /
//! BLE packets of any size), so every scenario is fed whole, at every single
//! split point, byte by byte and in pseudo-random chunks.

#[path = "../../src/bt_scpi.rs"]
#[allow(dead_code)]
mod bt_scpi;

use bt_scpi::*;
use std::collections::VecDeque;

#[derive(Debug, Clone, PartialEq)]
enum Ev {
    F(u8, Vec<u8>, bool),
    L(String),
    O,
}

fn frame_id(id: [u8; 2], cmd: u8, data: &[u8]) -> Vec<u8> {
    let mut v = vec![0xAF, id[0], id[1], cmd, data.len() as u8];
    v.extend_from_slice(data);
    let s = v.iter().fold(0u8, |a, &x| a.wrapping_add(x));
    v.push(0u8.wrapping_sub(s));
    v
}

fn frame(cmd: u8, data: &[u8]) -> Vec<u8> {
    frame_id([0x05, 0x03], cmd, data)
}

fn f(cmd: u8, data: &[u8]) -> Ev {
    Ev::F(cmd, data.to_vec(), false)
}

fn l(s: &str) -> Ev {
    Ev::L(s.to_string())
}

/// Feed (chunk, time ms) pairs; drain all events after each chunk.
fn run(rx: &mut Rx, chunks: &[(Vec<u8>, u64)]) -> Vec<Ev> {
    let mut out = Vec::new();
    for (c, t) in chunks {
        let mut q: VecDeque<u8> = c.iter().copied().collect();
        loop {
            let mut src = || q.pop_front();
            match rx.next(*t, &mut src) {
                Some(Event::Frame(fr)) => out.push(Ev::F(fr.cmd, fr.data[..fr.len].to_vec(), fr.broadcast)),
                Some(Event::Line(s)) => {
                    assert!(!s.is_empty(), "empty line event");
                    out.push(Ev::L(String::from_utf8_lossy(s).into_owned()))
                }
                Some(Event::Overrun) => out.push(Ev::O),
                None => break,
            }
        }
        assert!(q.is_empty(), "dispatcher left input unread");
    }
    out
}

fn once(stream: &[u8]) -> Vec<Ev> {
    run(&mut Rx::new(), &[(stream.to_vec(), 1000)])
}

struct Lcg(u64);
impl Lcg {
    fn next(&mut self, n: usize) -> usize {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        ((self.0 >> 33) as usize) % n
    }
}

/// Same events for every way of splitting `stream` (all at one time).
fn check_splits(name: &str, stream: &[u8], want: &[Ev]) {
    assert_eq!(once(stream), want, "{name}: whole");
    for k in 1..stream.len() {
        let got = run(&mut Rx::new(), &[(stream[..k].to_vec(), 1000), (stream[k..].to_vec(), 1003)]);
        assert_eq!(got, want, "{name}: split at {k}");
    }
    let bytes: Vec<(Vec<u8>, u64)> = stream.iter().enumerate().map(|(i, &b)| (vec![b], 1000 + i as u64 / 8)).collect();
    assert_eq!(run(&mut Rx::new(), &bytes), want, "{name}: byte by byte");
    let mut r = Lcg(0x5EED ^ stream.len() as u64);
    for round in 0..200 {
        let mut chunks = Vec::new();
        let mut i = 0;
        let mut t = 1000;
        while i < stream.len() {
            let n = 1 + r.next(40);
            let e = (i + n).min(stream.len());
            chunks.push((stream[i..e].to_vec(), t));
            t += r.next(20) as u64;
            i = e;
        }
        assert_eq!(run(&mut Rx::new(), &chunks), want, "{name}: random chunking {round}");
    }
    println!("ok  {name} ({} bytes, {} events)", stream.len(), want.len());
}

fn cat(parts: &[&[u8]]) -> Vec<u8> {
    parts.concat()
}

fn dispatcher() {
    // 1. Phone-app polling and SCPI interleaved, frame payloads with ASCII,
    //    '\n' and 0xAF inside (cmd 07 name: must stay in the frame).
    let mut name = [0u8; 20];
    name[..12].copy_from_slice(b"READ?\n*IDN?\n");
    let mut name2 = [0u8; 20];
    name2[..4].copy_from_slice(&[0xAF, 0x05, 0x03, 0x0A]);
    let s = cat(&[
        &frame(0x09, &[]),
        b"*IDN?\n",
        &frame(0x06, &[0x15]),
        b"MEAS:VOLT:DC?\r\n",
        &frame(0x07, &name),
        b":SYST:ERR?\r",
        &frame_id([0xFF, 0xFF], 0x00, &[]),
        b"\r\n\n  conf:res;read?\n",
        &frame(0x07, &name2),
        b"WAV:DATA?\n",
    ]);
    check_splits(
        "mixed frames + lines",
        &s,
        &[
            f(0x09, &[]),
            l("*IDN?"),
            f(0x06, &[0x15]),
            l("MEAS:VOLT:DC?"),
            f(0x07, &name),
            l(":SYST:ERR?"),
            Ev::F(0x00, vec![], true),
            l("conf:res;read?"),
            f(0x07, &name2),
            l("WAV:DATA?"),
        ],
    );

    // 2. A stray 0xAF (or 0xAF + a valid first id byte) right before a line:
    //    the bytes after it are re-fed, the line survives.
    check_splits("stray AF before line", &cat(&[&[0xAF], b"*IDN?\n"]), &[l("*IDN?")]);
    check_splits("AF 05 before line", &cat(&[&[0xAF, 0x05], b"*IDN?\n"]), &[l("*IDN?")]);
    // 0xAF 'A' '\n' 'B' '\n': the refeed itself yields a line, then another.
    check_splits("lines inside a refeed", &[0xAF, b'A', b'\n', b'B', b'\n'], &[l("A"), l("B")]);
    // Double 0xAF: the second one starts the real frame.
    check_splits("AF AF frame", &cat(&[&[0xAF], &frame(0x01, &[])]), &[f(0x01, &[])]);
    // Bad length (> 0xC2): resync, the frame / line behind is found.
    check_splits(
        "bad length then frame",
        &cat(&[&[0xAF, 0x05, 0x03, 0x09, 0xFF], &frame(0x09, &[]), b"READ?\n"]),
        &[f(0x09, &[]), l("READ?")],
    );

    // 3. Bad checksum: the frame is dropped (as stock), the line after it kept.
    let mut bad = frame(0x04, &[1]);
    *bad.last_mut().unwrap() ^= 0x55;
    check_splits("bad crc then line", &cat(&[&bad, b"*OPC?\n", &frame(0x04, &[0])]), &[l("*OPC?"), f(0x04, &[0])]);

    // 4. 0xAF inside a line aborts the line and starts a frame.
    check_splits("AF aborts line", &cat(&[b"*ID", &frame(0x09, &[]), b"READ?\n"]), &[f(0x09, &[]), l("READ?")]);
    // Other non-ASCII inside a line: the line is dropped.
    check_splits("non-ASCII aborts line", &cat(&[b"*ID\xC3\xA9N?\n", b"READ?\n"]), &[l("READ?")]);

    // 5. Junk outside a line / frame.
    check_splits("junk", b"123 abc\n\x00\x01\xFE*IDN?\n?;\r\n:READ?\n", &[l("*IDN?"), l(":READ?")]);

    // 6. Line length: exactly LINE_MAX is fine; one more = Overrun, the rest
    //    of that line is dropped, the next line works.
    let mut ok = vec![b'A'; LINE_MAX];
    ok.push(b'\n');
    check_splits("LINE_MAX line", &ok, &[Ev::L("A".repeat(LINE_MAX))]);
    let mut long = vec![b'A'; LINE_MAX + 40];
    long.extend_from_slice(b"\n*IDN?\n");
    check_splits("overrun", &long, &[Ev::O, l("*IDN?")]);
    let mut long2 = vec![b'B'; LINE_MAX + 1];
    long2.extend(frame(0x09, &[]));
    check_splits("overrun then frame", &long2, &[Ev::O, f(0x09, &[])]);

    // 7. Timeouts (between bytes of one request).
    let mut rx = Rx::new();
    let got = run(&mut rx, &[(b"REA".to_vec(), 0), (b"D?\n".to_vec(), LINE_GAP_MS)]);
    assert_eq!(got, vec![l("READ?")], "line within the gap");
    let mut rx = Rx::new();
    let got = run(&mut rx, &[(b"REA".to_vec(), 0), (b"\n*IDN?\n".to_vec(), LINE_GAP_MS + 1)]);
    assert_eq!(got, vec![l("*IDN?")], "stale partial line dropped");
    let fr = frame(0x06, &[0x15]);
    let mut rx = Rx::new();
    let got = run(&mut rx, &[(fr[..3].to_vec(), 0), (fr[3..].to_vec(), FRAME_GAP_MS)]);
    assert_eq!(got, vec![f(0x06, &[0x15])], "frame within the gap");
    let mut rx = Rx::new();
    let got = run(&mut rx, &[(fr[..5].to_vec(), 0), (b"*IDN?\n".to_vec(), FRAME_GAP_MS + 1)]);
    assert_eq!(got, vec![l("*IDN?")], "stale partial frame dropped");
    // Inside the gap the bytes belong to the truncated frame: "*I" completes
    // it (bad CRC, dropped) and the rest is a garbled line. This is what the
    // timeout prevents for a later request.
    let mut rx = Rx::new();
    let got = run(&mut rx, &[(fr[..5].to_vec(), 0), (b"*IDN?\n".to_vec(), 10)]);
    assert_eq!(got, vec![l("DN?")], "inside the gap the bytes belong to the frame");
    // Skip state (junk) also expires.
    let mut rx = Rx::new();
    let got = run(&mut rx, &[(b"123".to_vec(), 0), (b"*IDN?\n".to_vec(), LINE_GAP_MS + 1)]);
    assert_eq!(got, vec![l("*IDN?")], "stale junk dropped");
    // reset() forgets a partial request (new connection).
    let mut rx = Rx::new();
    let _ = run(&mut rx, &[(b"*ID".to_vec(), 0)]);
    rx.reset();
    assert_eq!(run(&mut rx, &[(b"READ?\n".to_vec(), 1)]), vec![l("READ?")], "reset");
    println!("ok  timeouts / reset");

    // 7b. Rx has no link gate (device f59bac2: a host's first line came
    // ~0.34 s before PC14 rose and was dropped): every line is reported,
    // the module's start text is consumed by bt.rs before Rx sees it.
    let mut rx = Rx::new();
    assert_eq!(run(&mut rx, &[(b"*IDN?\n".to_vec(), 0)]), vec![l("*IDN?")], "a line with no link state");
    println!("ok  no link gate");

    // 8. Long pseudo-random session: N requests of both kinds, random
    //    chunking; every request comes out once, in order.
    let mut r = Lcg(42);
    let mut stream = Vec::new();
    let mut want = Vec::new();
    let cmds = ["*IDN?", "READ?", "MEAS:VOLT:DC?", ":CONF:RES", "SYST:ERR?", "WAV:DATA?", "fetch?"];
    for _ in 0..500 {
        if r.next(2) == 0 {
            let cmd = r.next(10) as u8;
            let n = r.next(30);
            let data: Vec<u8> = (0..n).map(|_| r.next(256) as u8).collect();
            stream.extend(frame(cmd, &data));
            want.push(f(cmd, &data));
        } else {
            let c = cmds[r.next(cmds.len())];
            stream.extend_from_slice(c.as_bytes());
            stream.extend_from_slice([&b"\n"[..], b"\r\n", b"\r"][r.next(3)]);
            want.push(l(c));
        }
    }
    let mut chunks = Vec::new();
    let mut i = 0;
    while i < stream.len() {
        let e = (i + 1 + r.next(64)).min(stream.len());
        chunks.push((stream[i..e].to_vec(), 5000 + i as u64));
        i = e;
    }
    assert_eq!(run(&mut Rx::new(), &chunks), want, "long session");
    println!("ok  long random session ({} requests, {} bytes)", want.len(), stream.len());
}

fn filter() {
    let refused = [
        "DBG:UPDATE",
        "DBG:RECOVER",
        "dbg:recover",
        ":DBG:RESET",
        "::DBG:RELEASE",
        "  :dbg:ping? 100",
        "DBG?",
        "Dbg:Flash?",
        "DBG:KEY 1,2000",
        "DBG:SWIPE D,2",
        "DBG:LCD? 0,0,10,10,100",
        "DBG:GENUINE?",
        "DBG:BOOT?",
        "DBG:FUTURE:THING",
        "*IDN?;DBG:RELEASE",
        "MEAS?;  :DBG:KEY 1",
        "CONF:VOLT\nDBG:UPDATE",
        "READ?\rdbg:reset",
        "DBG",
        "DBG:UPDATE;*IDN?",
        "DIAG?",
        "DIAG:DAC?",
        "diag:pwm?",
        ":DIAG:PWM?",
        "*IDN?;DIAG:PWM?",
    ];
    for m in refused {
        assert_eq!(untrusted_refusal(m.as_bytes()), Some(REFUSED), "must refuse {m:?}");
    }
    let allowed = [
        "*IDN?",
        "SYST:ERR?",
        "MEAS:VOLT:DC?",
        "CONF:VOLT;READ?",
        ":READ?",
        "DIAGX?",
        "LOG:DATA?",
        "WAV:DATA?",
        "SOUR:FUNC SIN",
        "DBGX:FOO",
        "SYST:DBG?",
        "MEAS:TEMP? DBG",
        "",
        ";;",
    ];
    for m in allowed {
        assert_eq!(untrusted_refusal(m.as_bytes()), None, "must allow {m:?}");
    }
    println!("ok  untrusted filter ({} refused, {} allowed)", refused.len(), allowed.len());
}

fn txring() {
    let mut buf = [0u8; 16];
    let mut t = TxRing::new(16);
    assert!(t.is_empty() && t.segment() == (0, 0));
    assert!(t.push(&mut buf, &[b"hello", b"\n"]));
    assert_eq!(t.segment(), (0, 6));
    assert_eq!(&buf[..6], b"hello\n");
    t.consume(6);
    assert!(t.is_empty() && t.segment() == (0, 0), "empty ring restarts at 0");
    assert!(t.push(&mut buf, &[b"0123456789AB"]));
    t.consume(10);
    assert_eq!(t.segment(), (10, 2));
    assert!(t.push(&mut buf, &[b"cdefgh", b"ij"]), "wrapping push");
    assert_eq!(t.segment(), (10, 6));
    let mut got = buf[10..16].to_vec();
    t.consume(6);
    assert_eq!(t.segment(), (0, 4));
    got.extend_from_slice(&buf[..4]);
    assert_eq!(&got, b"ABcdefghij");
    t.consume(4);
    assert!(t.is_empty());
    // All or nothing.
    assert!(t.push(&mut buf, &[b"0123456789"]));
    assert!(!t.push(&mut buf, &[b"abc", b"defg"]), "7 > 6 free");
    assert_eq!(t.free(), 6);
    assert!(t.push(&mut buf, &[b"abc", b"def"]));
    assert_eq!(t.free(), 0);

    // Random producer / DMA-like consumer: bytes come out in order.
    let mut r = Lcg(7);
    let mut buf = [0u8; 64];
    let mut t = TxRing::new(64);
    let (mut next_in, mut next_out) = (0u32, 0u32);
    let mut in_flight = 0usize;
    for _ in 0..100_000 {
        if r.next(2) == 0 {
            let n = 1 + r.next(20);
            let data: Vec<u8> = (0..n).map(|k| (next_in.wrapping_add(k as u32) % 251) as u8).collect();
            if t.push(&mut buf, &[&data]) {
                next_in = next_in.wrapping_add(n as u32);
            }
        } else {
            // "DMA done": consume what was in flight, start the next segment.
            t.consume(in_flight);
            let (s, n) = t.segment();
            for &b in &buf[s..s + n] {
                assert_eq!(b, (next_out % 251) as u8, "TX byte order");
                next_out = next_out.wrapping_add(1);
            }
            in_flight = n;
        }
    }
    println!("ok  tx ring ({next_out} bytes through a 64-byte ring)");
}

/// Model of the circular RX DMA: `w` bytes written in all, the half / full
/// transfer flags (sticky until a look clears them).
struct Dma<const CAP: usize> {
    ring: [u8; CAP],
    w: u64,
    half: bool,
    full: bool,
}

impl<const CAP: usize> Dma<CAP> {
    fn new() -> Self {
        Dma { ring: [0; CAP], w: 0, half: false, full: false }
    }
    fn put(&mut self, b: u8) {
        self.ring[(self.w % CAP as u64) as usize] = b;
        self.w += 1;
        match (self.w % CAP as u64) as usize {
            0 => self.full = true,
            i if i == CAP / 2 => self.half = true,
            _ => {}
        }
    }
    /// As bt.rs `rx_see`: the write index and the flags (cleared).
    fn look(&mut self, c: &mut RxCursor) {
        let (h, f) = (core::mem::take(&mut self.half), core::mem::take(&mut self.full));
        c.see((self.w % CAP as u64) as usize, h, f);
    }
}

/// RxCursor against the model DMA: bytes come out in order while the reader
/// keeps up; an overrun is reported exactly when unread input no longer
/// fits (also after a gap of up to 1.5 rings between looks), and reading
/// then resumes at the newest cap - 1 bytes.
fn rxcursor() {
    const CAP: usize = 16;
    let mut r = Lcg(11);
    let mut d = Dma::<CAP>::new();
    let mut c = RxCursor::new(CAP);
    let mut rd = 0u64; // bytes taken in all
    let (mut lost, mut laps) = (0, 0);
    for _ in 0..400_000 {
        if r.next(2) == 0 {
            // DMA: up to 1.5 rings between two looks (a long main-loop pass).
            let n = r.next(CAP * 3 / 2);
            laps += (n >= CAP) as usize;
            for _ in 0..n {
                d.put((d.w % 251) as u8);
            }
            d.look(&mut c);
            if d.w - rd >= CAP as u64 {
                assert!(c.take_lost(), "overrun not seen: {} unread", d.w - rd);
                lost += 1;
                rd = d.w - (CAP as u64 - 1);
            } else {
                assert!(!c.take_lost(), "false overrun: {} unread", d.w - rd);
            }
        } else {
            for _ in 0..r.next(CAP) {
                match c.next() {
                    Some(i) => {
                        assert_eq!(d.ring[i], (rd % 251) as u8, "RX byte order");
                        rd += 1;
                    }
                    None => {
                        assert_eq!(rd, d.w, "bytes left unread");
                        break;
                    }
                }
            }
        }
    }
    assert!(lost > 1000 && laps > 1000, "the run must exercise overruns ({lost}) and missed laps ({laps})");
    // While lost is pending nothing is handed out.
    let mut c = RxCursor::new(CAP);
    c.see(10, false, false);
    c.see(0, false, true); // 16 unread
    assert_eq!(c.next(), None);
    assert!(c.take_lost());
    assert_eq!(c.next(), Some(1), "the newest 15 bytes, oldest first");
    // A whole lap between two looks: only the flags show it.
    let mut c = RxCursor::new(CAP);
    c.see(3, false, false);
    c.see(3, true, true);
    assert!(c.take_lost(), "a missed lap");
    println!("ok  rx cursor ({lost} overruns, {laps} gaps over one ring, {rd} bytes)");
}

/// bt.rs `poll` / `take_line` on the 1 KiB ring: a host pipelines lines
/// while one waits to be taken (a READ? waiting for its reading). Past
/// 1 KiB: an overrun, no cut or joined line is handed out, and lines after
/// the gap are taken again.
fn rx_held(chunk: usize) {
    const CAP: usize = 1024;
    struct M {
        d: Dma<CAP>,
        c: RxCursor,
        rx: Rx,
        held: Option<String>,
        overruns: usize,
    }
    impl M {
        fn write(&mut self, b: &[u8]) {
            for &x in b {
                self.d.put(x);
            }
        }
        /// As bt.rs `poll`: look at the DMA, resync on lost input, parse
        /// while no line waits.
        fn poll(&mut self) {
            self.d.look(&mut self.c);
            if self.c.take_lost() {
                self.rx.resync(1000);
                self.overruns += 1;
            }
            while self.held.is_none() {
                let (c, ring) = (&mut self.c, &self.d.ring);
                let mut src = || c.next().map(|i| ring[i]);
                match self.rx.next(1000, &mut src) {
                    Some(Event::Line(l)) => self.held = Some(String::from_utf8_lossy(l).into_owned()),
                    Some(_) => panic!("unexpected event"),
                    None => break,
                }
            }
        }
    }
    let mut m = M { d: Dma::new(), c: RxCursor::new(CAP), rx: Rx::new(), held: None, overruns: 0 };
    let sent: Vec<String> = (0..400).map(|k| format!("SYST:ERR:COUN?;*OPC;:X{k}")).collect();
    let bytes = |ls: &[String]| -> Vec<u8> { ls.iter().flat_map(|l| l.bytes().chain([b'\n'])).collect() };
    // 45 lines (~1.1 KiB) arrive while the first waits, `chunk` bytes per
    // poll (64: 5 ms polls; all: one main-loop pass longer than the ring).
    m.write(&bytes(&sent[..1]));
    m.poll();
    assert!(m.held.is_some());
    let burst = bytes(&sent[1..45]);
    assert!(burst.len() > CAP);
    for ch in burst.chunks(chunk) {
        m.write(ch);
        m.poll();
    }
    // Lost input past the 1 KiB: an overrun at each look while it grows.
    let burst_overruns = m.overruns;
    assert!(burst_overruns >= 1, "overrun not seen");
    // The waiting query is answered: lines are taken again, one per poll
    // (5 ms), so the host sends no faster than that (16 bytes per poll).
    let rest = bytes(&sent[45..]);
    let mut it = rest.chunks(16);
    let mut got = Vec::new();
    for _ in 0..2000 {
        if let Some(h) = m.held.take() {
            got.push(h);
        }
        m.poll();
        if let Some(ch) = it.next() {
            m.write(ch);
        }
    }
    assert_eq!(m.overruns, burst_overruns, "no overrun once lines are taken");
    // Every line handed out is one the host sent, whole, in order; the ones
    // after the burst all arrive.
    let mut k = 0;
    for g in &got {
        let p = sent[k..].iter().position(|s| s == g).unwrap_or_else(|| panic!("cut / joined line {g:?}"));
        k += p + 1;
    }
    assert!(got.ends_with(&sent[45..]), "lines after the burst all taken");
    assert!(got.len() < 400 && got[0] == sent[0], "the burst lost lines, the waiting one kept");
    println!("ok  rx held line + 1.1 KiB burst, {} bytes per poll ({} of 400 lines, {burst_overruns} overruns)", chunk.min(burst.len()), got.len());
}

/// bt.rs `take_line` gate against a host pipelining block queries: the TX
/// ring (8 KiB) drains at 115200 baud, the main loop runs every 5 ms; a line
/// is taken only while `room_for(REPLY_MAX)`. No reply is dropped. Without
/// the gate the same streams overflow the ring (the test is meaningful).
fn tx_gate() {
    const TX: usize = 8192;
    const REPLY_MAX: usize = 6144 + 1;
    let streams: &[(&str, Vec<usize>)] = &[
        ("WAV:DATA? x3 (WORD, 2000 points)", vec![3807; 3]),
        ("LOG:DATA? x2", vec![REPLY_MAX; 2]),
        ("LOG:DATA? x4 + small", vec![REPLY_MAX, 15, REPLY_MAX, 15, 15, REPLY_MAX, 400, REPLY_MAX]),
    ];
    for (name, replies) in streams {
        for gate in [true, false] {
            let mut buf = vec![0u8; TX];
            let mut t = TxRing::new(TX);
            let (mut q, mut dropped, mut ms, mut credit) = (0usize, 0, 0u64, 0.0f64);
            while q < replies.len() || !t.is_empty() {
                // 5 ms of wire time: 11.52 bytes per ms.
                credit += 5.0 * 11.52;
                let n = (credit as usize).min(t.used());
                credit -= n as f64;
                t.consume(n);
                if q < replies.len() && (!gate || t.room_for(REPLY_MAX)) {
                    let r = vec![b'x'; replies[q]];
                    if !t.push(&mut buf, &[&r]) {
                        dropped += 1;
                    }
                    q += 1;
                }
                ms += 5;
                assert!(ms < 60_000);
            }
            if gate {
                assert_eq!(dropped, 0, "{name}: a reply dropped with the gate");
            } else {
                assert!(dropped > 0, "{name}: overflows without the gate");
            }
        }
    }
    println!("ok  tx gate ({} pipelined block streams, none dropped)", streams.len());
}

fn main() {
    dispatcher();
    filter();
    txring();
    rxcursor();
    rx_held(64);
    rx_held(2000);
    tx_gate();
    println!("PASS");
}
