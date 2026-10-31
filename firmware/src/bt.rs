//! Bluetooth: transparent SPP / BLE module on UART4 (PA0 TX / PA1 RX, AF8,
//! 115200 8N1; read back from the stock setup). Reception by circular DMA
//! (DMA0 channel 2, request 4) so nothing is lost while the main loop is busy;
//! transmission from a ring by DMA (DMA0 channel 4, request 4 = UART4_TX, the
//! STM32F4-compatible request map that also gives channel 2 = UART4_RX), so a
//! long SCPI reply never blocks the main loop.
//!
//! The RX stream carries two protocols (bt_scpi.rs, docs/bt-scpi.md):
//!   - stock frames (README.md "DM40 Multimeter Protocol"):
//!       request  AF <id 05 03 | FF FF> <cmd> <len> <data..> <crc>
//!       response DF 05 03 <cmd> <len> <data..> <crc>   (discovery: DF FF FF ..)
//!       crc makes the byte sum 0 mod 256. `poll` returns them.
//!   - SCPI lines (letter / '*' / ':' .. '\n'), the same parser as USBTMC.
//!     `take_line` returns them; the transport is UNTRUSTED (an open radio
//!     link): DBG:* is refused before the parser sees it (ScpiPort::trusted);
//!     main.rs also refuses MMEM:DEL and LOG:CSV ON from it.
//!     Replies go out with a '\n' terminator (also after a #-block).
//!
//! Throughput: 115200 8N1 = 11.5 kB/s, 87 us/byte. READ? reply (15 B) 1.3 ms;
//! WAV:DATA? (#3400 + 400 + LF = 406 B) 35 ms; LOG:DATA? (<= 6144 B) 0.53 s.
//! No RTS/CTS on this module wiring: over BLE (20-byte notifications) the
//! module may not keep up with a long burst; unverified.

#[path = "bt_scpi.rs"]
pub mod proto;

use crate::clock::{Clock, HCLK_HZ};
use crate::dmm::{Dmm, Val};
use crate::meter::Meter;
use crate::mmio::{modify32, r32, w32};
use core::sync::atomic::{compiler_fence, Ordering};
pub use proto::Frame;
use proto::{Event, RxCursor, TxRing, LINE_MAX};

const GPIOA: u32 = 0x4002_0000;
/// GPIOC input data register: PC14 = BT link up (the same read as
/// `power::Power::bt_connected`, repeated here because main.rs does not pass
/// `Power` to `init_module`).
const GPIOC_IDR: u32 = 0x4002_0800 + 0x10;
/// GPIOC bit set/reset register: PC0 = BT module power (bt_meas_v2_6.h
/// section 1: low at the stock BT UART init, high when the meter page is
/// left; high = off). board::init leaves it high (as the stock boot); `power`
/// drives it.
const GPIOC_BOP: u32 = 0x4002_0800 + 0x18;
/// After the module is powered: no AT init until then (it needs a module
/// that takes AT; no host can be connected before it is up).
const BOOT_MS: u64 = 1000;
/// Stock: PC0 high 100 ms after a successful cmd 07 (module restart).
const PULSE_MS: u64 = 100;
const RCU_APB1EN: u32 = 0x4002_3840;
const UART4: u32 = 0x4000_4C00;
const STAT: u32 = UART4 + 0x00;
const DATA: u32 = UART4 + 0x04;
const BAUD: u32 = UART4 + 0x08;
const CTL0: u32 = UART4 + 0x0C;
const CTL2: u32 = UART4 + 0x14;
const DWT_CYCCNT: u32 = 0xE000_1004;

const DMA0: u32 = 0x4002_6000;
const fn ch_reg(ch: u32, off: u32) -> u32 {
    DMA0 + off + 0x18 * ch
}
/// RX: channel 2 (flags in INTF0/INTC0 at bit 16).
const RX_CTL: u32 = ch_reg(2, 0x10);
const RX_CNT: u32 = ch_reg(2, 0x14);
const RX_PADDR: u32 = ch_reg(2, 0x18);
const RX_M0ADDR: u32 = ch_reg(2, 0x1C);
const DMA_INTF0: u32 = DMA0 + 0x00;
const DMA_INTC0: u32 = DMA0 + 0x08;
/// TX: channel 4 (flags in INTF1/INTC1 at bit 0). gen.rs uses channel 5, the
/// scope DMA1; nothing else in opendm40 uses DMA0.
const TX_CTL: u32 = ch_reg(4, 0x10);
const TX_CNT: u32 = ch_reg(4, 0x14);
const TX_PADDR: u32 = ch_reg(4, 0x18);
const TX_M0ADDR: u32 = ch_reg(4, 0x1C);
const DMA_INTC1: u32 = DMA0 + 0x0C;

/// 89 ms at 115200: covers a long main-loop iteration (a full redraw).
/// Input is not read while an SCPI line waits to be taken; more than this
/// meanwhile is an overrun (`RxCursor`: "Input buffer overrun", resync).
const RX_LEN: usize = 1024;
/// Holds the largest reply (RSP_MAX block + LF) plus stock frames: REPLY_MAX
/// + TX_SLACK (6 401) and 255 bytes more, so a line is taken with a few
/// stock frames still queued. Not smaller: a reply goes into the ring whole
/// (`send`); built on the stack (`respond_with`) it has nowhere else to
/// wait, and LOG:DATA? cannot be built again in parts (it takes the
/// logger's ring). Was 8 KiB (docs/native-boot.md "SRAM audit").
const TX_LEN: usize = 6656;
/// Largest reply built in place (`ScpiPort::respond_with`), as USBTMC.
const BLK_LEN: usize = crate::usbtmc::RSP_MAX;
/// The next SCPI line is taken only with room for the largest reply of one
/// message (a block + LF; text replies are joined in Scpi's 512-byte buffer
/// and a block cannot share a message) plus proto::TX_SLACK.
const REPLY_MAX: usize = BLK_LEN + 1;
const _: () = assert!(REPLY_MAX + proto::TX_SLACK <= TX_LEN);
/// Lines that arrive this soon after an AT exchange are the module's late
/// answer ("OK", "CBN+..."), not a host command.
const AT_QUIET_MS: u64 = 500;
/// Module start text (`StartText`): its last line, and how long after the
/// start it may still come (device: done 180-195 ms after power-up).
const START_LAST: &[u8] = b"CSSW:";
const START_MAX_MS: u64 = 1000;

/// The module's own text after each start (power-up, AT+RESET). Device
/// 7b1b6d1 (docs/device-check-2026-10-21.md "f59bac2"): a 0x00, then 145-195
/// ms after the start "BLE03:VER2.1-20240615\0", "CBN:<BLE name>",
/// "CBMAC:<mac>", "CSN:<SPP name>", "CSMAC:<mac>", "CBSW:1", "CSSW:1" (CR LF
/// each) = 7 SCPI errors -102 -113 -113 -102 -113 -113 -113 for the next
/// client; PC14 reads high the first ~650 ms of a start, with no host, so a
/// PC14 gate cannot tell this text from a host's. `Bt::poll` hands it all
/// to this, not to `proto::Rx`, up to the CSSW line, or START_MAX_MS
/// after the start if that never comes. A host can link 0.32 s after
/// power-up (device), after the text.
struct StartText {
    /// Start time (own ms); None = no start text pending.
    at: Option<u64>,
    /// First bytes of the current line.
    head: [u8; 5],
    n: usize,
}

impl StartText {
    const fn new() -> Self {
        StartText { at: None, head: [0; 5], n: 0 }
    }

    fn start(&mut self, now: u64) {
        self.at = Some(now);
        self.n = 0;
    }

    /// Take the module's bytes from `src` while its start text runs. True:
    /// still running (nothing for the parser yet).
    fn consume(&mut self, now: u64, src: &mut dyn FnMut() -> Option<u8>) -> bool {
        let Some(at) = self.at else { return false };
        while let Some(b) = src() {
            if b == b'\r' || b == b'\n' {
                let last = self.n == START_LAST.len() && self.head == *START_LAST;
                self.n = 0;
                if last {
                    self.at = None;
                    return false;
                }
            } else if b != 0 && self.n < self.head.len() {
                self.head[self.n] = b;
                self.n += 1;
            }
        }
        if now >= at + START_MAX_MS {
            self.at = None;
            return false;
        }
        true
    }
}

#[link_section = ".dmabuf"]
static mut RX: [u8; RX_LEN] = [0; RX_LEN];
#[link_section = ".dmabuf"]
static mut TX: [u8; TX_LEN] = [0; TX_LEN];

/// Where an SCPI reply goes: USBTMC (trusted: a cable to the user's own
/// computer) or Bluetooth (untrusted). main.rs routes `scpi::Out` results
/// through this so both transports share one code path.
pub trait ScpiPort {
    /// Queue a complete reply (the transport adds its terminator).
    fn respond(&mut self, data: &[u8]);
    /// Buffer to build a large reply in place, then `respond_len`: USBTMC
    /// only (the DBG:* replies). Bluetooth has none (empty): DBG:* never runs
    /// from it (`trusted`); its large replies go through `respond_with`.
    fn rsp_buf(&mut self) -> &mut [u8];
    fn respond_len(&mut self, n: usize);
    /// A large reply built in place (MMEM:CAT?, MMEM:DATA?, LOG:DATA?): `f`
    /// fills the buffer (usbtmc::RSP_MAX bytes) and returns the reply's
    /// length, which is then queued (None: no reply).
    fn respond_with(&mut self, f: &mut dyn FnMut(&mut [u8]) -> Option<usize>);
    /// A block reply of any length: `head`, then `len` bytes from `fill`,
    /// read while it goes out (usbtmc `respond_stream`). False: the
    /// transport cannot (Bluetooth: its replies are copied into the TX ring
    /// at once), nothing was queued.
    fn respond_stream(&mut self, _head: &[u8], _len: usize, _fill: fn(usize, &mut [u8])) -> bool {
        false
    }
    /// False: DBG:* (reflash / reset / watchdog release / input injection /
    /// debug reads) must never run from this transport. Bt::take_line
    /// already refuses such lines; main.rs checks this again before acting
    /// on `scpi::Out::Dbg` (defence in depth), and refuses MMEM:DEL and
    /// LOG:CSV ON (disk changes) from it.
    fn trusted(&self) -> bool;
}

impl<'a, B: usb_device::bus::UsbBus> ScpiPort for crate::usbtmc::Usbtmc<'a, B> {
    fn respond(&mut self, data: &[u8]) {
        crate::usbtmc::Usbtmc::respond(self, data)
    }
    fn rsp_buf(&mut self) -> &mut [u8] {
        &mut crate::usbtmc::Usbtmc::rsp_buf(self)[..]
    }
    fn respond_len(&mut self, n: usize) {
        crate::usbtmc::Usbtmc::respond_len(self, n)
    }
    fn respond_with(&mut self, f: &mut dyn FnMut(&mut [u8]) -> Option<usize>) {
        if let Some(n) = f(&mut crate::usbtmc::Usbtmc::rsp_buf(self)[..]) {
            crate::usbtmc::Usbtmc::respond_len(self, n);
        }
    }
    fn respond_stream(&mut self, head: &[u8], len: usize, fill: fn(usize, &mut [u8])) -> bool {
        crate::usbtmc::Usbtmc::respond_stream(self, head, len, fill);
        true
    }
    fn trusted(&self) -> bool {
        true
    }
}

impl ScpiPort for Bt {
    fn respond(&mut self, data: &[u8]) {
        self.send(&[data, b"\n"]);
    }
    fn rsp_buf(&mut self) -> &mut [u8] {
        &mut []
    }
    fn respond_len(&mut self, _n: usize) {}
    /// The reply is built on the stack, in this never-inlined frame, and
    /// copied into the TX ring at once (`send`, all or nothing): a 6 KiB
    /// static (was `.dmabuf` BLK) would hold SRAM for a buffer that lives
    /// only during this call, and this path (main loop -> SCPI dispatch ->
    /// here -> the builder) is far shallower than the drawing paths that
    /// set the stack's worst case (tools/stack-depth).
    #[inline(never)]
    fn respond_with(&mut self, f: &mut dyn FnMut(&mut [u8]) -> Option<usize>) {
        let mut blk = [0u8; BLK_LEN];
        if let Some(n) = f(&mut blk) {
            self.send(&[&blk[..n.min(BLK_LEN)], b"\n"]);
        }
    }
    fn trusted(&self) -> bool {
        false
    }
}

/// Note how far the RX DMA has written (overrun check, `RxCursor::see`):
/// the write index with channel 2's half / full transfer flags (INTF0 bits
/// 20 / 21), read again until no byte came in between (one byte is 87 us),
/// then the flags seen are cleared.
fn rx_see(cur: &mut RxCursor) {
    let pos = || RX_LEN - (unsafe { r32(RX_CNT) } as usize).min(RX_LEN);
    let (mut wr, mut f) = (0, 0);
    for _ in 0..4 {
        wr = pos();
        f = unsafe { r32(DMA_INTF0) } & (0x30 << 16);
        if pos() == wr {
            break;
        }
    }
    if f != 0 {
        unsafe { w32(DMA_INTC0, f) };
    }
    cur.see(wr, f & (0x10 << 16) != 0, f & (0x20 << 16) != 0);
}

fn rx_byte(cur: &mut RxCursor) -> Option<u8> {
    rx_see(cur);
    let i = cur.next()?;
    Some(unsafe { core::ptr::read_volatile(core::ptr::addr_of!(RX[i])) })
}

pub struct Bt {
    rd: RxCursor,
    rx: proto::Rx,
    tx: TxRing,
    /// Bytes of the DMA transfer in progress (consumed when it is done).
    in_flight: usize,
    /// Start time and deadline of that transfer.
    tx_due: u64,
    /// TX DMA works. Cleared if a transfer never finishes (wrong request
    /// mapping on this chip): then TX falls back to blocking writes, as
    /// before the DMA path, so the stock protocol keeps working.
    pub tx_dma_ok: bool,
    /// An SCPI line waiting for `take_line` (input is held meanwhile).
    line: [u8; LINE_MAX],
    line_n: usize,
    line_ready: bool,
    /// main.rs takes lines (set by the first `take_line`). Until then lines
    /// are dropped so they cannot hold up the stock frames.
    lines_on: bool,
    /// Error for the BT SCPI session (input overrun, output overflow).
    err: Option<&'static str>,
    link: bool,
    /// The host left (PC14 fell) since main.rs last asked (`take_left`).
    left: bool,
    quiet_until: u64,
    /// Own millisecond count from DWT_CYCCNT (Clock is not passed to poll).
    t_last: u32,
    t_acc: u32,
    t_ms: u64,
    /// Bytes dropped because the TX ring was full (diagnostics).
    pub tx_dropped: u32,
    /// One-shot module AT init done this power-on (bt_meas_v2_6.h, FUN_0800d848).
    inited: bool,
    /// PC0 low (module powered), and since when (own ms).
    powered: bool,
    on_at: u64,
    /// cmd 07 restart pulse: PC0 high from `.0` to `.1` (own ms).
    pulse: Option<(u64, u64)>,
    /// The module's start text, consumed before the parser sees input.
    start: StartText,
}

const CYC_PER_MS: u32 = HCLK_HZ / 1000;

/// Spin until `done()` or `ms` milliseconds (DWT cycles), feeding the
/// watchdog (when the main loop feeds it). False: timed out.
fn wait_until(ms: u32, mut done: impl FnMut() -> bool) -> bool {
    let t0 = unsafe { r32(DWT_CYCCNT) };
    while !done() {
        if unsafe { r32(DWT_CYCCNT) }.wrapping_sub(t0) > ms * CYC_PER_MS {
            return false;
        }
        crate::wdt::kick_if_feeding();
    }
    true
}

impl Bt {
    #[inline(never)] // .cold (link-native.x): start-up
    pub fn new() -> Self {
        for (i, b) in BOOT_VER.iter().enumerate() {
            b.store(unsafe { core::ptr::read_volatile((BOOT_DATA + 0x10 + i as u32) as *const u8) }, Ordering::Relaxed);
        }
        unsafe {
            modify32(RCU_APB1EN, 0, 1 << 19); // UART4
            // PA0 / PA1: AF8.
            modify32(GPIOA + 0x20, 0xFF, 0x88);
            modify32(GPIOA + 0x08, 0xF, 0xA);
            modify32(GPIOA + 0x00, 0xF, 0xA);

            w32(CTL0, 0);
            w32(BAUD, 434); // 50 MHz / 434 = 115200
            w32(CTL2, (1 << 7) | (1 << 6)); // DENT, DENR: DMA transmit + receive

            // DMA0 channel 2, request 4 = UART4 RX, P->M, byte, MINC, circular.
            w32(RX_CTL, 0);
            let _ = wait_until(10, || r32(RX_CTL) & 1 == 0);
            w32(DMA_INTC0, 0x3D << 16); // clear channel 2 flags
            w32(RX_PADDR, DATA);
            w32(RX_M0ADDR, core::ptr::addr_of!(RX) as u32);
            w32(RX_CNT, RX_LEN as u32);
            w32(RX_CTL, (4 << 25) | (1 << 10) | (1 << 8) | 1);

            // DMA0 channel 4 (TX): idle until `pump` starts a segment.
            w32(TX_CTL, 0);
            let _ = wait_until(10, || r32(TX_CTL) & 1 == 0);
            w32(DMA_INTC1, 0x3D); // clear channel 4 flags
            w32(TX_PADDR, DATA);

            // UEN, TEN, REN (no interrupts: we run with IRQs masked).
            w32(CTL0, (1 << 13) | (1 << 3) | (1 << 2));
        }
        Bt {
            rd: RxCursor::new(RX_LEN),
            rx: proto::Rx::new(),
            tx: TxRing::new(TX_LEN),
            in_flight: 0,
            tx_due: 0,
            tx_dma_ok: true,
            line: [0; LINE_MAX],
            line_n: 0,
            line_ready: false,
            lines_on: false,
            err: None,
            link: false,
            left: false,
            quiet_until: 0,
            t_last: unsafe { r32(DWT_CYCCNT) },
            t_acc: 0,
            t_ms: 0,
            tx_dropped: 0,
            inited: false,
            powered: false,
            on_at: 0,
            pulse: None,
            start: StartText::new(),
        }
    }

    /// Module power from the Bluetooth setting (call every main-loop pass;
    /// writes PC0 only on a change). The stock powers the module only while
    /// BT is on (and only on its meter page; opendm40 serves BT on every view).
    /// Native opendm40 left PC0 high from board::init: the module stayed off and
    /// the meter was unreachable (Host is down, no inquiry / advertising).
    pub fn power(&mut self, on: bool) {
        let now = self.ms();
        if self.pulse.is_some_and(|(_, end)| now >= end) {
            self.pulse = None;
        }
        let on = on && !self.pulse.is_some_and(|(start, _)| now >= start);
        if on == self.powered {
            return;
        }
        self.powered = on;
        unsafe { w32(GPIOC_BOP, if on { 1 << 16 } else { 1 }) };
        if on {
            self.on_at = now;
            self.start.start(now);
            // As after AT+RESET (init_module): the start text is StartText's,
            // and a host may link before an AT answer's quiet window ends
            // (cmd 07: the restart pulse 20 ms after a 300 ms answer wait).
            self.quiet_until = 0;
        }
    }

    fn ms(&mut self) -> u64 {
        let c = unsafe { r32(DWT_CYCCNT) };
        self.t_acc += c.wrapping_sub(self.t_last);
        self.t_last = c;
        self.t_ms += (self.t_acc / CYC_PER_MS) as u64;
        self.t_acc %= CYC_PER_MS;
        self.t_ms
    }

    /// Service the TX DMA: retire a finished transfer, start the next
    /// contiguous segment. Cheap; called by poll / every send, and may be
    /// called every main-loop iteration.
    pub fn pump(&mut self) {
        if !self.tx_dma_ok {
            return self.pump_blocking();
        }
        unsafe {
            if r32(TX_CTL) & 1 != 0 {
                // Running (CHEN clears itself at the end). Twice the wire
                // time + 50 ms without finishing: the DMA request never comes.
                if self.ms() > self.tx_due {
                    w32(TX_CTL, 0);
                    let _ = wait_until(10, || r32(TX_CTL) & 1 == 0);
                    let sent = self.in_flight - (r32(TX_CNT) as usize).min(self.in_flight);
                    self.tx.consume(sent);
                    self.in_flight = 0;
                    self.tx_dma_ok = false;
                    self.pump_blocking();
                }
                return;
            }
            compiler_fence(Ordering::SeqCst);
            if self.in_flight > 0 {
                self.tx.consume(self.in_flight);
                self.in_flight = 0;
            }
            let (s, n) = self.tx.segment();
            if n == 0 {
                return;
            }
            let n = n.min(0xFFFF);
            w32(DMA_INTC1, 0x3D);
            w32(TX_M0ADDR, core::ptr::addr_of!(TX) as u32 + s as u32);
            w32(TX_CNT, n as u32);
            compiler_fence(Ordering::SeqCst);
            // Request 4, M->P (TM = 01), byte, MINC, enable.
            w32(TX_CTL, (4 << 25) | (1 << 10) | (1 << 6) | 1);
            self.in_flight = n;
            self.tx_due = self.ms() + (n as u64) / 5 + 50;
        }
    }

    /// Fallback TX: write the whole queue out, byte by byte (blocks; a full
    /// 6.5 KiB ring is ~0.6 s at 115200, the watchdog fed meanwhile). Bounded
    /// (review 6595d46 L2): a TBE that does not come within 10 ms (one byte
    /// is 87 us) or 2 s in all drops the rest of the queue (counted in
    /// `tx_dropped`).
    fn pump_blocking(&mut self) {
        let buf = unsafe { &*core::ptr::addr_of!(TX) };
        let t0 = unsafe { r32(DWT_CYCCNT) };
        loop {
            let (s, n) = self.tx.segment();
            if n == 0 {
                return;
            }
            for (k, &b) in buf[s..s + n].iter().enumerate() {
                let over = unsafe { r32(DWT_CYCCNT) }.wrapping_sub(t0) > 2000 * CYC_PER_MS;
                // TBE
                if over || !wait_until(10, || unsafe { r32(STAT) } & (1 << 7) != 0) {
                    self.tx.consume(k);
                    let left = self.tx.used();
                    self.tx_dropped = self.tx_dropped.wrapping_add(left as u32);
                    self.tx.consume(left);
                    return;
                }
                unsafe { w32(DATA, b as u32) };
            }
            self.tx.consume(n);
        }
    }

    /// Queue `parts` as one unit (all or nothing) and start sending.
    fn send(&mut self, parts: &[&[u8]]) -> bool {
        let buf = unsafe { &mut *core::ptr::addr_of_mut!(TX) };
        let ok = self.tx.push(buf, parts);
        if !ok {
            self.tx_dropped = self.tx_dropped.wrapping_add(parts.iter().map(|p| p.len() as u32).sum());
            self.err = Some("Output buffer overflow");
        }
        self.pump();
        ok
    }

    /// Wait (bounded) until everything queued is on the wire.
    fn flush(&mut self) {
        let end = self.ms() + 1000;
        loop {
            self.pump();
            if self.tx.is_empty() && self.in_flight == 0 {
                break;
            }
            if self.ms() > end {
                break;
            }
        }
    }

    /// Next complete, valid request frame, if any. SCPI lines met on the way
    /// are kept for `take_line` (input then waits until the line is taken).
    pub fn poll(&mut self) -> Option<Frame> {
        self.pump();
        let now = self.ms();
        // Also while a line waits (input held): the ring may wrap meanwhile.
        rx_see(&mut self.rd);
        if self.rd.take_lost() {
            // The DMA overtook unread input: skip to the next terminator so
            // no command cut by the gap runs.
            self.rx.resync(now);
            if self.lines_on {
                self.err = Some("Input buffer overrun");
            }
        }
        // PC14 means nothing while the module is off. It is no gate for
        // input: a host's first bytes come ~0.34 s before it rises (device
        // 7b1b6d1), and it is high, with no host, while the module starts
        // (`StartText`).
        let up = self.powered && unsafe { r32(GPIOC_IDR) } & (1 << 14) != 0;
        if up != self.link {
            self.link = up;
            if !up {
                // The host left: no partial request or untaken line of it
                // is run later, and main.rs drops its pending query.
                self.rx.reset();
                self.line_ready = false;
                self.left = true;
            }
        }
        {
            let rd = &mut self.rd;
            let mut src = || rx_byte(rd);
            if self.start.consume(now, &mut src) {
                return None;
            }
        }
        loop {
            if self.line_ready {
                return None;
            }
            let rd = &mut self.rd;
            let mut src = || rx_byte(rd);
            match self.rx.next(now, &mut src)? {
                Event::Frame(f) => return Some(f),
                Event::Line(l) => {
                    if self.lines_on && now >= self.quiet_until {
                        self.line[..l.len()].copy_from_slice(l);
                        self.line_n = l.len();
                        self.line_ready = true;
                    }
                }
                Event::Overrun => {
                    if self.lines_on {
                        self.err = Some("Input buffer overrun");
                    }
                }
            }
        }
    }

    /// The next SCPI message from the BT link: Ok(line without terminator),
    /// or Err(error for SYST:ERR?): a refused line (DBG:* over an untrusted
    /// link: nothing of it runs), an input overrun or an output overflow.
    /// Call it only when the previous BT query has been answered: lines are
    /// then handled in order like on a serial instrument (no interrupted
    /// queries). A line is held until the TX ring has room for its largest
    /// reply (REPLY_MAX). Lines arrive through `poll`, so call that too.
    pub fn take_line(&mut self) -> Option<Result<&[u8], &'static str>> {
        self.lines_on = true;
        if let Some(e) = self.err.take() {
            return Some(Err(e));
        }
        // Not before its reply fits the TX ring: a host pipelining block
        // queries would otherwise overflow it (the last reply dropped).
        // Meanwhile the line (and input after it) waits.
        if !self.line_ready || !self.tx.room_for(REPLY_MAX) {
            return None;
        }
        self.line_ready = false;
        let l = &self.line[..self.line_n];
        if let Some(e) = proto::untrusted_refusal(l) {
            return Some(Err(e));
        }
        Some(Ok(l))
    }

    /// True once after the host left (PC14 fell): main.rs then drops the BT
    /// session's pending READ? / MEAS? and the rest of its message, so the
    /// next host's first reply is its own (#122).
    pub fn take_left(&mut self) -> bool {
        core::mem::take(&mut self.left)
    }

    /// Send a response frame (queued; ~(6 + len) * 87 us on the wire).
    pub fn reply(&mut self, cmd: u8, data: &[u8], broadcast: bool) {
        let hdr = if broadcast { [0xDF, 0xFF, 0xFF] } else { [0xDF, 0x05, 0x03] };
        let mut sum = 0u8;
        for &b in hdr.iter().chain(&[cmd, data.len() as u8]).chain(data) {
            sum = sum.wrapping_add(b);
        }
        self.send(&[&hdr, &[cmd, data.len() as u8], data, &[0u8.wrapping_sub(sum)]]);
    }

    /// Send a raw AT command to the module (name changes etc.); returns when
    /// it is on the wire, so an answer window can start.
    pub fn at(&mut self, cmd: &[u8]) {
        self.send(&[cmd]);
        self.flush();
    }
}

/// Bootloader data block in internal flash (read only): version bytes.
const BOOT_DATA: u32 = 0x080A_4000;

/// Its version bytes +0x10..+0x12 (HW major, loader major / minor), copied
/// once by `Bt::new` at start-up: BT command 02 and the Device page read
/// the copy. 0x080A4000 is in the slow flash bank (not zero-wait): a load
/// there stalls the AD2 stream ISR ~21.8 us (device c92b605), and either
/// may be used while a stream runs.
static BOOT_VER: [core::sync::atomic::AtomicU8; 3] = [const { core::sync::atomic::AtomicU8::new(0) }; 3];

/// The boot data's version bytes (see BOOT_VER).
pub fn boot_ver() -> [u8; 3] {
    BOOT_VER.each_ref().map(|b| b.load(Ordering::Relaxed))
}

/// Unit prefix step (x1000) relative to the base unit of the mode group
/// (V group: mV, A group: uA, ohm, nF, Hz; diode V, duty %, temperature C/F).
fn prefix(mode: u8, unit: &str) -> u8 {
    match (mode, unit) {
        (0..=3, "V") => 1,
        (4..=9, "mA") => 1,
        (4..=9, "A") => 2,
        (_, u) if u.starts_with('k') => 1,
        (_, u) if u.starts_with('M') => 2,
        (13, "\u{b5}F") => 1,
        (13, "mF") => 2,
        _ => 0,
    }
}

/// Format byte + 16-bit value (bt_meas_v2_6.h section 3). OL -> 0xFFFF.
fn encode(mode: u8, v: Option<&Val>, dec_override: Option<u8>) -> (u8, u16) {
    let Some(v) = v else { return (0, 0xFFFF) };
    let dec = dec_override.unwrap_or(v.dec).min(4);
    let mut fmt = prefix(mode, v.unit) << 4 | dec << 1;
    if v.ol {
        return (fmt, 0xFFFF);
    }
    if v.v < 0.0 {
        fmt |= 1;
    }
    let mut k = 1.0f32;
    for _ in 0..dec {
        k *= 10.0;
    }
    let mut n = ((if v.v < 0.0 { -v.v } else { v.v }) * k + 0.5) as u32;
    if mode == 11 && n > 0xFFF0 {
        n -= 0xFFF0;
        fmt |= 0x80;
    }
    (fmt, n as u16)
}

impl Bt {
    /// Collect an AT answer from the module for up to `ms` (blocking).
    fn at_answer(&mut self, clk: &mut Clock, ms: u32, out: &mut [u8; 64]) -> usize {
        let end = clk.now_ms() + ms as u64;
        let mut n = 0;
        while clk.now_ms() < end {
            self.pump();
            if let Some(b) = rx_byte(&mut self.rd) {
                if n < out.len() {
                    out[n] = b;
                    n += 1;
                }
            }
        }
        // A late part of the answer must not reach the SCPI parser.
        self.quiet_until = self.ms() + AT_QUIET_MS;
        n
    }

    /// One-shot module setup, issued the first time BT is on (bt_meas_v2_6.h
    /// lines 87-92, FUN_0800d848 / FUN_0800ceaa): name the module
    /// "DM40A"/"B"/"C" by model, and on an "OK" answer set the LED mode and
    /// reset the module. The stock persists a "done" flag (settings 0x115 bit 7)
    /// so it runs once ever; we have no such store, so `inited` runs it once per
    /// power-on. It waits BOOT_MS after the module is powered (`power`), so the
    /// attempt is not spent before the module is ready to take AT (judgment
    /// call, not from the header).
    ///
    /// Not while a phone is connected (PC14 high): the module is then in
    /// transparent mode, so the AT text would reach the phone app as data,
    /// at_answer would swallow 50 ms of request frames and AT+RESET would drop
    /// the link. The attempt is kept for after the phone disconnects. The
    /// caller must also not call this during a data-logger session (it blocks
    /// the main loop for ~150 ms).
    pub fn init_module(&mut self, clk: &mut Clock, model: u8) {
        if self.inited || !self.powered || self.ms() < self.on_at + BOOT_MS {
            return;
        }
        if unsafe { r32(GPIOC_IDR) } & (1 << 14) != 0 {
            return;
        }
        self.inited = true;
        self.at(b"AT+SBNDM40");
        self.at(&[b'A' + model]);
        self.at(b"\r\n");
        let mut a = [0u8; 64];
        let n = self.at_answer(clk, 50, &mut a);
        let ans = {
            let mut e = n;
            while e > 0 && (a[e - 1] == b'\r' || a[e - 1] == b'\n') {
                e -= 1;
            }
            &a[..e]
        };
        if ans == b"OK" {
            self.at(b"AT+SLED0\r\n");
            let mut _b = [0u8; 64];
            self.at_answer(clk, 100, &mut _b);
            self.at(b"AT+RESET\r\n");
            // The module restarts: its start text again (no "OK" first).
            // StartText takes everything up to that text's end, so the
            // AT+SLED0 answer's quiet window (AT_QUIET_MS) ends here: a host
            // links 0.32 s after a start (device), inside that window, and
            // its first line was dropped (emu btwaketest).
            let now = self.ms();
            self.start.start(now);
            self.quiet_until = 0;
        }
    }

    /// Serve one request frame (stock command set 00..09). Returns true when
    /// command 06 changed the DMM function or sub-mode: the caller then runs
    /// the app's common function-change path (App::host_func_changed: meter
    /// reset, chart restart, function bar redrawn), as for a touch or SCPI.
    /// A manual range change within the function returns false.
    pub fn serve(&mut self, f: &Frame, clk: &mut Clock, dmm: &mut Dmm, meter: &mut Meter, raw: Option<&crate::dmm::Reading>, power: &crate::power::Power, locked: bool) -> bool {
        const LENS: [usize; 10] = [0, 0, 0, 1, 1, 1, 1, 20, 0, 0];
        if f.cmd > 9 {
            return false;
        }
        let mut func_changed = false;
        let st = (f.len != LENS[f.cmd as usize]) as u8;
        match f.cmd {
            0x00 => self.reply(0, &[0x05, 0x03, b'A' + dmm.model()], true),
            0x01 => self.reply(1, &[st], false),
            0x02 => {
                let [hw, bl_major, bl_minor] = boot_ver();
                self.reply(2, &[hw.wrapping_mul(10), 26, bl_major.wrapping_mul(10).wrapping_add(bl_minor)], false);
            }
            0x03..=0x06 => {
                self.reply(f.cmd, &[st], false);
                let d = if f.len > 0 { f.data[0] } else { 0 };
                match f.cmd {
                    0x03 => {
                        meter.reset();
                        dmm.hold_range(false);
                    }
                    0x04 => meter.set_hold(d != 0),
                    0x05 => {
                        if d == 0 {
                            meter.rel_off();
                            dmm.hold_range(false);
                        } else if meter.rel_on(raw) && dmm.hy_mode() <= 10 || dmm.hy_mode() == 14 {
                            dmm.hold_range(true);
                        }
                    }
                    _ => func_changed = dmm.apply_mode_byte(clk, d),
                }
            }
            0x07 => {
                let name_len = f.data[..f.len.min(20)].iter().position(|&b| b == 0).unwrap_or(f.len.min(20));
                self.at(b"AT+SBN");
                self.at(&f.data[..name_len]);
                self.at(b"\r\n");
                let mut a = [0u8; 64];
                let n = self.at_answer(clk, 300, &mut a);
                let ok = a[..n].windows(2).any(|w| w == b"OK");
                self.reply(7, &[!ok as u8], false);
                if ok {
                    // Stock: PC0 high 100 ms after the reply (module restart,
                    // the new name takes effect). `power` runs the pulse; the
                    // reply (7 bytes, < 1 ms) is on the wire long before.
                    let t = self.ms() + 20;
                    self.pulse = Some((t, t + PULSE_MS));
                }
            }
            0x08 => {
                self.at(b"AT+CBN\r\n");
                let mut a = [0u8; 64];
                let n = self.at_answer(clk, 300, &mut a);
                let mut name = [0u8; 20];
                if let Some(p) = a[..n].windows(4).position(|w| w == b"CBN+") {
                    let rest = &a[p + 4..n];
                    let e = rest.iter().position(|&c| c == b'\r' || c == b'\n').unwrap_or(rest.len()).min(19);
                    name[..e].copy_from_slice(&rest[..e]);
                }
                self.reply(8, &name, false);
            }
            _ => {
                let mut d = [0u8; 11];
                let mode = dmm.hy_mode();
                d[0] = dmm.mode_byte();
                // Flags byte (bt_meas_v2_6.h section 3, lines 139-148): bit7 HOLD,
                // bit6 key lock (name guess), bit5 REL, bit4 always 0, bit3 charging
                // (meaning guess), bits0-2 battery level 0..5.
                // TODO: bits 6 and 3 are guesses from the header (before they were
                // added the byte was hold|rel|5). Check against the stock phone app
                // (lock / charge icon shown at the right time) before relying on
                // them; drop them if the app misreads them.
                d[1] = (meter.hold as u8) << 7
                    | (locked as u8) << 6
                    | (meter.rel_active() as u8) << 5
                    | (power.charging() as u8) << 3
                    | (power.bars & 0x07);
                let shown = meter.shown();
                // Temperature: the stock frame is main in C, value 2 the cold
                // junction in C, value 3 deg F; opendm40 readings follow the
                // global unit (Settings / UNIT:TEMP) and carry no deg F
                // second value, so rebuild the stock triple here.
                let to_c = |v: crate::dmm::Val| if v.unit == "\u{b0}F" { crate::dmm::Val { v: (v.v - 32.0) / 1.8, unit: "\u{b0}C", ..v } } else { v };
                let shown = shown.copied().map(|mut r| {
                    if r.mode == 15 {
                        r.main = to_c(r.main);
                        r.aux1 = r.aux1.map(to_c);
                        r.aux2 = Some(crate::dmm::Val { v: r.main.v * 1.8 + 32.0, unit: "\u{b0}F", ..r.main });
                    }
                    r
                });
                // Power meter: the DC V frame with P as the value
                // (powermeter.rs "Bluetooth"; no value 2 / 3).
                let pwr = shown.and_then(|r| r.power);
                let shown = shown.as_ref().filter(|_| pwr.is_none());
                let main = pwr.map(|p| crate::dmm::pw::bt_main(&p)).or(shown.map(|r| r.main));
                let (fm, vm) = encode(if pwr.is_some() { 1 } else { mode }, main.as_ref(), None);
                // AC+DC (sub 2) / AUTO coupling: value 2 = AC part, value 3 =
                // DC part, sent even when over range (only the LCD shows "0L"
                // for them).
                let acdc = shown.map(|r| r.parts()).unwrap_or(false);
                let unol = |v: crate::dmm::Val| crate::dmm::Val { ol: v.ol && !acdc, ..v };
                // Temperature: value 2 is the cold junction (C) for the stock
                // app; a resistive sensor puts its resistance in aux1 instead,
                // which is not sent (0xFFFF) so the app does not show ohms as C.
                // The burden voltage (A functions, doc 3.15) is not sent:
                // the app would show it in the function's unit (mA, A).
                let bd = shown.and_then(|r| r.burden()).map(|b| b.0);
                let a1 = shown.and_then(|r| r.aux1).filter(|_| bd != Some(0)).map(unol).filter(|v| mode != 15 || v.unit == "\u{b0}C");
                let a2 = shown.and_then(|r| r.aux2).filter(|_| bd != Some(1)).map(unol);
                // No value yet (NaN: the frequency line before its first
                // counter result): not sent, as an absent one.
                let (a1, a2) = (a1.filter(|v| !v.v.is_nan()), a2.filter(|v| !v.v.is_nan()));
                let d3 = match mode {
                    8 if !acdc => Some(2),
                    12 => Some(1),
                    _ => None,
                };
                let (f2, v2) = encode(mode, a1.as_ref(), None);
                let (f3, v3) = encode(mode, a2.as_ref(), d3);
                d[2] = if a1.is_some() { f2 } else { 0 };
                d[3] = if a2.is_some() { f3 } else { 0 };
                d[4] = fm;
                d[5..7].copy_from_slice(&(if a1.is_some() { v2 } else { 0xFFFF }).to_le_bytes());
                d[7..9].copy_from_slice(&(if a2.is_some() { v3 } else { 0xFFFF }).to_le_bytes());
                d[9..11].copy_from_slice(&vm.to_le_bytes());
                self.reply(9, &d, false);
            }
        }
        func_changed
    }
}
