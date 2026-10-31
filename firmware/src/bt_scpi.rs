//! Bluetooth UART stream logic, hardware-free (host-tested by
//! tools/bt-scpi-test): the RX dispatcher that splits the byte stream into
//! stock binary request frames and SCPI text lines, the policy check that
//! keeps dangerous commands off the radio, and the TX ring bookkeeping that
//! bt.rs drains by DMA. Included by bt.rs (`#[path]`), so main.rs needs no
//! `mod` line for it.
//!
//! RX dispatch (docs/bt-scpi.md):
//!   - 0xAF starts a stock frame (README.md "DM40 Multimeter Protocol"):
//!     AF <05 03 | FF FF> <cmd> <len <= 0xC2> <data..> <crc>, byte sum 0.
//!     Inside a frame every byte belongs to the frame (payloads may be ASCII).
//!     A bad id / length restarts the scan at the byte after the 0xAF (the
//!     bytes are re-fed, so a line or a frame hidden behind a stray 0xAF is
//!     still found); a bad checksum drops the frame, as the stock parser does.
//!   - a letter, '*' or ':' starts an SCPI line; '\n' or '\r' ends it (CR LF
//!     gives one line: the empty one is ignored). A non-ASCII byte drops the
//!     line up to its terminator (0xAF instead starts a frame); a line longer than LINE_MAX is
//!     discarded up to its terminator (Event::Overrun).
//!   - other bytes outside a frame / line: control bytes and non-ASCII are
//!     ignored, other printable ASCII discards up to the next terminator.
//!   - a partial frame / line older than FRAME_GAP_MS / LINE_GAP_MS since its
//!     last byte is dropped before the next byte is taken (a stray letter or
//!     a truncated frame cannot swallow the next request). The time is the
//!     drain time of the RX ring (bt.rs polls it every ~5 ms), so a main-loop
//!     stall longer than the gap can drop a request that arrived split around
//!     the stall; the gaps are far above any normal loop time.
//!   - RxCursor reads the RX DMA ring and detects an overrun (the DMA wrapped
//!     past unread bytes); bt.rs then calls `Rx::resync` (skip to the next
//!     terminator) and reports "Input buffer overrun".

pub const MAX_DATA: usize = 0xC2;
/// Longest SCPI line (main.rs copies a message into a 256-byte buffer).
pub const LINE_MAX: usize = 256;
/// A partial line is dropped after this long without a byte. Generous: BT
/// fragments a write, and a person may type into a raw terminal.
pub const LINE_GAP_MS: u64 = 2000;
/// A partial binary frame is dropped after this long without a byte (the
/// phone app writes a whole frame, <= 200 bytes = 17 ms at 115200).
pub const FRAME_GAP_MS: u64 = 500;

#[derive(Clone, Copy)]
pub struct Frame {
    pub cmd: u8,
    pub len: usize,
    pub data: [u8; MAX_DATA],
    /// Request used the broadcast id FF FF.
    #[cfg_attr(not(test), allow(dead_code))] // tools/bt-scpi-test
    pub broadcast: bool,
}

pub enum Event<'a> {
    /// A complete, valid stock request frame.
    Frame(Frame),
    /// One SCPI message without its terminator (never empty).
    Line(&'a [u8]),
    /// A line exceeded LINE_MAX and was discarded.
    Overrun,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum St {
    Idle,
    Frame,
    Line,
    /// Discard up to the next '\n' / '\r' (0xAF still starts a frame).
    Skip,
}

const FB: usize = 5 + MAX_DATA + 1;

pub struct Rx {
    st: St,
    fb: [u8; FB],
    fnb: usize,
    lb: [u8; LINE_MAX],
    ln: usize,
    /// Bytes to process again before taking new input (a failed frame
    /// header). Invariant: replay + frame bytes <= 5, so 8 is plenty.
    rp: [u8; 8],
    rp_i: usize,
    rp_n: usize,
    /// Time of the last byte taken in Frame / Line / Skip.
    last_ms: u64,
}

fn line_start(b: u8) -> bool {
    b.is_ascii_alphabetic() || b == b'*' || b == b':'
}

impl Rx {
    pub const fn new() -> Self {
        Rx { st: St::Idle, fb: [0; FB], fnb: 0, lb: [0; LINE_MAX], ln: 0, rp: [0; 8], rp_i: 0, rp_n: 0, last_ms: 0 }
    }

    /// Forget any partial frame / line (e.g. on a new BT connection).
    pub fn reset(&mut self) {
        self.st = St::Idle;
        self.fnb = 0;
        self.ln = 0;
        self.rp_n = 0;
        self.rp_i = 0;
    }

    /// After lost input (RX ring overrun): forget any partial frame / line
    /// and discard up to the next '\n' / '\r', so no command cut by the gap
    /// runs (0xAF still starts a frame).
    pub fn resync(&mut self, now: u64) {
        self.reset();
        self.st = St::Skip;
        self.last_ms = now;
    }

    /// Process input from `src` (None = nothing more right now) until the
    /// next event. Bytes not consumed stay in `src`.
    pub fn next(&mut self, now: u64, src: &mut dyn FnMut() -> Option<u8>) -> Option<Event<'_>> {
        loop {
            let b = if self.rp_i < self.rp_n {
                let b = self.rp[self.rp_i];
                self.rp_i += 1;
                b
            } else {
                self.rp_i = 0;
                self.rp_n = 0;
                let b = src()?;
                let gap = match self.st {
                    St::Frame => FRAME_GAP_MS,
                    St::Line | St::Skip => LINE_GAP_MS,
                    St::Idle => u64::MAX,
                };
                if now.saturating_sub(self.last_ms) > gap {
                    self.st = St::Idle;
                }
                self.last_ms = now;
                b
            };
            match self.step(b) {
                Step::None => {}
                Step::Frame(f) => return Some(Event::Frame(f)),
                Step::Line(n) => return Some(Event::Line(&self.lb[..n])),
                Step::Overrun => return Some(Event::Overrun),
            }
        }
    }

    fn start_frame(&mut self) {
        self.st = St::Frame;
        self.fb[0] = 0xAF;
        self.fnb = 1;
    }

    /// The frame header is invalid: re-feed everything after its 0xAF.
    fn refeed(&mut self) {
        let n = self.fnb - 1;
        let rest = self.rp_n - self.rp_i;
        let mut t = [0u8; 8];
        t[..n].copy_from_slice(&self.fb[1..1 + n]);
        t[n..n + rest].copy_from_slice(&self.rp[self.rp_i..self.rp_n]);
        self.rp[..n + rest].copy_from_slice(&t[..n + rest]);
        self.rp_i = 0;
        self.rp_n = n + rest;
        self.st = St::Idle;
        self.fnb = 0;
    }

    fn step(&mut self, b: u8) -> Step {
        match self.st {
            St::Idle => {
                if b == 0xAF {
                    self.start_frame();
                } else if line_start(b) {
                    self.st = St::Line;
                    self.lb[0] = b;
                    self.ln = 1;
                } else if b.is_ascii_graphic() {
                    self.st = St::Skip;
                }
                Step::None
            }
            St::Skip => {
                if b == b'\n' || b == b'\r' {
                    self.st = St::Idle;
                } else if b == 0xAF {
                    self.start_frame();
                }
                Step::None
            }
            St::Line => {
                if b == b'\n' || b == b'\r' {
                    self.st = St::Idle;
                    let n = self.ln;
                    self.ln = 0;
                    return Step::Line(n);
                }
                if b >= 0x80 {
                    // Not SCPI (7-bit): drop the line; 0xAF is a frame.
                    self.ln = 0;
                    if b == 0xAF {
                        self.start_frame();
                    } else {
                        self.st = St::Skip;
                    }
                    return Step::None;
                }
                if self.ln == LINE_MAX {
                    self.ln = 0;
                    self.st = St::Skip;
                    return Step::Overrun;
                }
                self.lb[self.ln] = b;
                self.ln += 1;
                Step::None
            }
            St::Frame => {
                self.fb[self.fnb] = b;
                self.fnb += 1;
                let n = self.fnb;
                if n == 3 {
                    let id = (self.fb[1], self.fb[2]);
                    if id != (0x05, 0x03) && id != (0xFF, 0xFF) {
                        self.refeed();
                        return Step::None;
                    }
                }
                if n == 5 && self.fb[4] as usize > MAX_DATA {
                    self.refeed();
                    return Step::None;
                }
                if n >= 5 && n == 5 + self.fb[4] as usize + 1 {
                    self.st = St::Idle;
                    self.fnb = 0;
                    let sum = self.fb[..n].iter().fold(0u8, |a, &x| a.wrapping_add(x));
                    if sum != 0 {
                        return Step::None;
                    }
                    let len = self.fb[4] as usize;
                    let mut f = Frame { cmd: self.fb[3], len, data: [0; MAX_DATA], broadcast: self.fb[1] == 0xFF };
                    f.data[..len].copy_from_slice(&self.fb[5..5 + len]);
                    return Step::Frame(f);
                }
                Step::None
            }
        }
    }
}

enum Step {
    None,
    Frame(Frame),
    Line(usize),
    Overrun,
}

/// Header roots never executed for an untrusted transport (Bluetooth: an
/// unauthenticated radio link anyone in range can open). DBG covers
/// DBG:UPDATE (zero the loader marker -> vendor update screen) and
/// DBG:LOADER / DBG:RECOVER (vendor update screen through PA2), executed
/// INSIDE Scpi::handle on native, so the check must run before the parser
/// sees the message, DBG:RESET, DBG:RELEASE (watchdog reset), the input injection (DBG:KEY / SWIPE D,2 = power off) and
/// the debug reads. Any future DBG:* command is refused by default. DIAG
/// (DIAG? / DIAG:DAC?: the generator DAC sampled through the ADC; DIAG:PWM?:
/// a 2 ms busy loop on PB14) is a bench diagnostic of the same kind.
pub const UNTRUSTED_DENY: &[&[u8]] = &[b"DBG", b"DIAG"];

/// SCPI error recorded when a message is refused (SCPI -203).
pub const REFUSED: &str = "Command protected";

/// For an untrusted transport: Some(error) if any command of `msg` has a
/// denied header root; the whole message is then refused (no part of it
/// runs). Splits like Scpi::handle (';', '\n', '\r'), but is wider than the
/// parser: any number of leading ':' and the full length are checked.
pub fn untrusted_refusal(msg: &[u8]) -> Option<&'static str> {
    for cmd in msg.split(|&b| b == b';' || b == b'\n' || b == b'\r') {
        let mut c = cmd;
        while let Some((&f, rest)) = c.split_first() {
            if f.is_ascii_whitespace() || f == b':' {
                c = rest;
            } else {
                break;
            }
        }
        let end = c
            .iter()
            .position(|&b| b == b':' || b == b'?' || b.is_ascii_whitespace())
            .unwrap_or(c.len());
        let root = &c[..end];
        if UNTRUSTED_DENY.iter().any(|d| root.eq_ignore_ascii_case(d)) {
            return Some(REFUSED);
        }
    }
    None
}

/// Read side of the circular RX DMA ring (`cap` bytes): the index of the
/// next byte to take, and whether the DMA overtook the reader. The DMA write
/// index alone cannot show a wrap, so this counts the bytes written between
/// two looks (`see`), and checks the count against the DMA's half / full
/// transfer flags (set when the writer passes cap/2 / the end): a flag the
/// count does not explain is a lap the count missed (a main-loop pass longer
/// than the ring's 89 ms at 115200). Exact while the writer moves less than
/// 1.5 * cap between looks; a longer gap can go unseen.
pub struct RxCursor {
    rd: usize,
    wr: usize,
    unread: usize,
    cap: usize,
    lost: bool,
}

impl RxCursor {
    pub const fn new(cap: usize) -> Self {
        RxCursor { rd: 0, wr: 0, unread: 0, cap, lost: false }
    }

    /// One look: `wr`, the DMA's write index (0..cap), and its half / full
    /// transfer flags since the last look, read together (no byte written
    /// in between). If the unread bytes no longer fit the ring, the oldest
    /// are gone: the reader jumps to the newest `cap - 1` bytes and `lost`
    /// is set until `take_lost`.
    pub fn see(&mut self, wr: usize, half: bool, full: bool) {
        let wr = wr % self.cap;
        let d = (wr + self.cap - self.wr) % self.cap;
        // Writer at `b` at the last look counts as crossed: the DMA may set
        // the flag just after the index (a missed lap from there also sets
        // the other flag, which `d` does not explain).
        let crossed = |b: usize| (b + self.cap - self.wr) % self.cap <= d;
        let missed = (half && !crossed(self.cap / 2)) || (full && !crossed(0));
        self.unread += d + if missed { self.cap } else { 0 };
        self.wr = wr;
        if self.unread >= self.cap {
            self.unread = self.cap - 1;
            self.rd = (wr + 1) % self.cap;
            self.lost = true;
        }
    }

    /// Index of the next unread byte (None: none, or lost input not yet
    /// taken by `take_lost`, so no byte after the gap is parsed first).
    pub fn next(&mut self) -> Option<usize> {
        if self.unread == 0 || self.lost {
            return None;
        }
        let i = self.rd;
        self.rd = (self.rd + 1) % self.cap;
        self.unread -= 1;
        Some(i)
    }

    /// True once after input was lost (see `see`).
    pub fn take_lost(&mut self) -> bool {
        core::mem::take(&mut self.lost)
    }
}

/// TX ring room kept on top of the largest SCPI reply before the next
/// Bluetooth line is taken (`TxRing::room_for`): stock frame replies (<= 26
/// bytes each) queued while that line's query waits for a reading.
pub const TX_SLACK: usize = 256;

/// Byte ring for the TX side: indices only, the storage is passed in (a
/// static in DMA-reachable SRAM on the target). The reader takes contiguous
/// segments (one DMA transfer each); `consume` after the transfer is done.
pub struct TxRing {
    rd: usize,
    used: usize,
    cap: usize,
}

impl TxRing {
    pub const fn new(cap: usize) -> Self {
        TxRing { rd: 0, used: 0, cap }
    }

    pub fn free(&self) -> usize {
        self.cap - self.used
    }

    /// Room for a reply of up to `max` bytes plus TX_SLACK.
    pub fn room_for(&self, max: usize) -> bool {
        self.free() >= max + TX_SLACK
    }

    /// Bytes queued.
    pub fn used(&self) -> usize {
        self.used
    }

    pub fn is_empty(&self) -> bool {
        self.used == 0
    }

    /// Append all `parts` or nothing (false: not enough room).
    pub fn push(&mut self, buf: &mut [u8], parts: &[&[u8]]) -> bool {
        let total: usize = parts.iter().map(|p| p.len()).sum();
        if total > self.free() {
            return false;
        }
        let mut wr = (self.rd + self.used) % self.cap;
        for p in parts {
            let mut p = *p;
            while !p.is_empty() {
                let n = p.len().min(self.cap - wr);
                buf[wr..wr + n].copy_from_slice(&p[..n]);
                wr = (wr + n) % self.cap;
                p = &p[n..];
            }
        }
        self.used += total;
        true
    }

    /// The next contiguous run of queued bytes: (start, len); len 0 = empty.
    pub fn segment(&self) -> (usize, usize) {
        (self.rd, self.used.min(self.cap - self.rd))
    }

    /// Drop `n` bytes sent from the front.
    pub fn consume(&mut self, n: usize) {
        let n = n.min(self.used);
        self.rd = (self.rd + n) % self.cap;
        self.used -= n;
        if self.used == 0 {
            self.rd = 0; // next reply starts at 0: fewer split segments
        }
    }
}
