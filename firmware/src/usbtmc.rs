//! USBTMC / USB488 interface (class 0xFE, subclass 3, protocol 1).
//!
//! Bulk-OUT carries DEV_DEP_MSG_OUT (a command) and REQUEST_DEV_DEP_MSG_IN (the
//! host asking for a response); Bulk-IN carries DEV_DEP_MSG_IN. Every message
//! has a 12-byte header and is padded to a multiple of 4. The application side
//! is two calls: `take_command` for a complete command (EOM seen) and
//! `respond` to queue the response text.
//!
//! Every response ends with the IEEE 488.2 response message terminator, a
//! newline ("NL^END": '\n' as the last byte of the EOM transfer), added
//! here (`terminate`): text, joined compound replies, and definite-length
//! blocks (`#N<len><data>` then '\n', as Keysight / Rigol send them). A host
//! that reads until '\n' (libscopehal's SCPITMCTransport::ReadReply: a
//! one-byte reply used to hang it) and one that reads until EOM (Linux
//! usbtmc, libsigrok, pyvisa) both see a complete reply. Text that already
//! ends with '\n' (the multi-line DBG reports) gets no second one. The
//! Bluetooth transport ends its lines the same way (bt.rs, docs/bt-scpi.md).

use usb_device::class_prelude::*;
use usb_device::control::{Recipient, RequestType};
use usb_device::Result;
use core::sync::atomic::{AtomicU32, Ordering::Relaxed};

const CLASS: u8 = 0xFE;
const SUBCLASS: u8 = 0x03;
const PROTOCOL_USB488: u8 = 0x01;

const DEV_DEP_MSG_OUT: u8 = 1;
const REQUEST_DEV_DEP_MSG_IN: u8 = 2;
const DEV_DEP_MSG_IN: u8 = 2;

// Class requests.
const INITIATE_ABORT_BULK_OUT: u8 = 1;
const CHECK_ABORT_BULK_OUT_STATUS: u8 = 2;
const INITIATE_ABORT_BULK_IN: u8 = 3;
const CHECK_ABORT_BULK_IN_STATUS: u8 = 4;
const INITIATE_CLEAR: u8 = 5;
const CHECK_CLEAR_STATUS: u8 = 6;
const GET_CAPABILITIES: u8 = 7;
const INDICATOR_PULSE: u8 = 64;
const READ_STATUS_BYTE: u8 = 128;
const REN_CONTROL: u8 = 160;
const GO_TO_LOCAL: u8 = 161;
const LOCAL_LOCKOUT: u8 = 162;

const STATUS_SUCCESS: u8 = 0x01;
const STATUS_PENDING: u8 = 0x02;
/// Abort: no transfer in progress (and nothing left in the FIFO).
const STATUS_FAILED: u8 = 0x80;
/// Abort: a transfer is in progress, but with another bTag.
const STATUS_TRANSFER_NOT_IN_PROGRESS: u8 = 0x81;

const MPS: usize = 64;
pub const CMD_MAX: usize = 256;
pub const RSP_MAX: usize = 6144;
/// A Bulk-OUT message left incomplete for this long is dropped
/// (`expire_partial`).
pub const OUT_STALE_MS: u32 = 1000;
const OUT_STALE_CYC: u32 = OUT_STALE_MS * (crate::clock::HCLK_HZ / 1000);
/// DWT cycle count of the last Bulk-OUT bytes. It wraps every ~21 s; the
/// check of every main-loop pass sees a stale message long before that
/// (only a pass blocked for over ~20 s could alias). A static, not a field:
/// the Usbtmc lives in main.rs `run`'s frame (the worst stack path).
static OUT_CYC: AtomicU32 = AtomicU32::new(0);
/// The response terminator (`terminate`).
const NL: u8 = b'\n';

const CAPABILITIES: [u8; 24] = [
    STATUS_SUCCESS, 0,
    0x00, 0x01,       // bcdUSBTMC 1.00
    0x04,             // interface: INDICATOR_PULSE
    0x01,             // device: TermChar supported
    0, 0, 0, 0, 0, 0,
    0x00, 0x01,       // bcdUSB488 1.00
    0x06,             // USB488.2 interface, REN/GTL/LLO accepted
    0x08,             // SCPI
    0, 0, 0, 0, 0, 0, 0, 0,
];

pub struct Usbtmc<'a, B: UsbBus> {
    iface: InterfaceNumber,
    ep_out: EndpointOut<'a, B>,
    ep_in: EndpointIn<'a, B>,

    // Bulk-OUT message being received.
    hdr: [u8; 12],
    hdr_len: usize,
    out_left: u32, // payload bytes still to come
    out_pad: u32,  // padding bytes still to skip
    out_eom: bool,
    cmd: [u8; CMD_MAX],
    cmd_len: usize,
    cmd_ready: bool,

    // Bulk-IN side.
    req_tag: Option<u8>, // REQUEST_DEV_DEP_MSG_IN waiting
    req_max: u32,
    /// RSP_MAX bytes for the application (`rsp_buf`) + 1 for the '\n'.
    rsp: [u8; RSP_MAX + 1],
    /// Length of the whole response with its '\n': `rsp[..rsp_len]`, or
    /// with `ext` the head `rsp[..head]`, `ext_len` bytes from `ext`, '\n'.
    rsp_len: usize,
    ext_len: usize,
    /// A streamed block reply (`respond_stream`): the payload after the
    /// head is read from `ext` packet by packet as it goes out, so it can
    /// be longer than RSP_MAX (the scope record, WAV:DATA?).
    ext: Option<fn(usize, &mut [u8])>,
    head: usize,
    /// Bytes of `rsp` already sent (a response longer than the host's
    /// TransferSize goes out in several REQUEST_DEV_DEP_MSG_IN rounds).
    rsp_off: usize,
    rsp_ready: bool,
    /// The Bulk-IN transfer in progress: header (12 B) + `rsp[tx_base ..
    /// tx_base + tx_n]` + zero padding to a multiple of 4, `tx_len` bytes in
    /// all, `tx_off` of them written. Packets are assembled from `rsp` as
    /// they go (`pump_in`); there is no second copy of the response (6 KiB
    /// less on the main stack, device check 2026-10-21).
    tx_hdr: [u8; 12],
    tx_base: usize,
    tx_n: usize,
    tx_len: usize,
    tx_off: usize,
    tx_zlp: bool,
    last_tag_in: u8,
    pub status_byte: u8,
    /// Set by INITIATE_CLEAR, INITIATE_ABORT_BULK_IN and a bus reset; the
    /// application drops any response it is still preparing (`take_cleared`).
    cleared: bool,
    /// Debug: count of control requests seen and the last four
    /// (bmRequestType, bRequest, wIndex lo, wLength lo each).
    pub dbg_ctl: (u8, [u8; 16]),
}

impl<'a, B: UsbBus> Usbtmc<'a, B> {
    pub fn new(alloc: &'a UsbBusAllocator<B>) -> Self {
        Usbtmc {
            iface: alloc.interface(),
            ep_out: alloc.bulk(MPS as u16),
            ep_in: alloc.bulk(MPS as u16),
            hdr: [0; 12],
            hdr_len: 0,
            out_left: 0,
            out_pad: 0,
            out_eom: false,
            cmd: [0; CMD_MAX],
            cmd_len: 0,
            cmd_ready: false,
            req_tag: None,
            req_max: 0,
            rsp: [0; RSP_MAX + 1],
            rsp_len: 0,
            ext_len: 0,
            ext: None,
            head: 0,
            rsp_off: 0,
            rsp_ready: false,
            tx_hdr: [0; 12],
            tx_base: 0,
            tx_n: 0,
            tx_len: 0,
            tx_off: 0,
            tx_zlp: false,
            last_tag_in: 0,
            status_byte: 0,
            cleared: false,
            dbg_ctl: (0, [0; 16]),
        }
    }

    /// A complete command (up to the EOM message), if one arrived. A
    /// response still queued but not handed to the IN endpoint is dropped
    /// here: nobody asked for it since (normally the new DEV_DEP_MSG_OUT
    /// already dropped it; this also covers a late reply queued between a
    /// multi-packet command's header and its last packet).
    pub fn take_command(&mut self) -> Option<&[u8]> {
        if self.cmd_ready {
            self.cmd_ready = false;
            self.rsp_ready = false;
            let n = self.cmd_len;
            self.cmd_len = 0;
            Some(&self.cmd[..n])
        } else {
            None
        }
    }

    /// Drop a Bulk-OUT message that stopped half-way: a header, payload or
    /// padding still to come, or a transfer without EOM waiting for the rest
    /// of its message, and no byte of it for OUT_STALE_MS. A host whose
    /// process died in the middle of a write (Linux usbtmc kills the URB on
    /// close and sends no clear) leaves that state; the next client's header
    /// would then be read as the old message's payload and its command be
    /// garbled or lost. Checked every main-loop pass and again when the
    /// next packet arrives: a pass that blocked longer than OUT_STALE_MS
    /// reads the new client's bytes before its own check, and their gap to
    /// the old ones still tells (in the emulator the per-pass check alone
    /// let a cut message eat the next query now and then; the cause was not
    /// found). The other side, not observed: a main-loop stall of over
    /// OUT_STALE_MS between two packets of one host message drops that
    /// message. Not while a complete command waits for the application
    /// (the endpoint is NAKed then, no byte can arrive).
    #[inline(always)]
    pub fn expire_partial(&mut self) {
        if self.partial() && crate::prof::cyc().wrapping_sub(OUT_CYC.load(Relaxed)) >= OUT_STALE_CYC {
            self.abort_out();
        }
    }

    /// A Bulk-OUT message is part-way in (`expire_partial`).
    #[inline(always)]
    fn partial(&self) -> bool {
        !self.cmd_ready && (self.hdr_len > 0 || self.out_left > 0 || self.out_pad > 0 || self.cmd_len > 0)
    }

    /// True once after INITIATE_CLEAR / INITIATE_ABORT_BULK_IN / a bus
    /// reset: forget a pending query (no late reply).
    pub fn take_cleared(&mut self) -> bool {
        core::mem::take(&mut self.cleared)
    }

    /// Response buffer to fill in place (then `respond_len`).
    pub fn rsp_buf(&mut self) -> &mut [u8] {
        self.cut_unsent();
        &mut self.rsp[..RSP_MAX]
    }

    /// `rsp[..n]` (n <= RSP_MAX) + the response terminator: the length to
    /// send. Text already ending in '\n' keeps it as the terminator; a
    /// block (`#...`) always gets its own, whatever its last data byte.
    fn terminate(&mut self, n: usize) -> usize {
        if n > 0 && self.rsp[n - 1] == NL && self.rsp[0] != b'#' {
            return n;
        }
        self.rsp[n] = NL;
        n + 1
    }

    /// A new response is about to overwrite `rsp` while the previous one
    /// still has payload to send from it (only when the host sent a new
    /// command in the middle of a read, i.e. gave that read up): end that
    /// transfer after the packets already written (`stop_in`) rather than
    /// send it a mix of both.
    fn cut_unsent(&mut self) {
        if self.tx_off < self.tx_len {
            self.stop_in();
        }
    }

    /// Queue the first `n` bytes of `rsp_buf()` as the response.
    pub fn respond_len(&mut self, n: usize) {
        self.ext = None;
        self.rsp_len = self.terminate(n.min(RSP_MAX));
        self.rsp_off = 0;
        self.rsp_ready = true;
        self.try_start_in();
    }

    /// Queue response text; sent when the host requests it.
    pub fn respond(&mut self, data: &[u8]) {
        let n = data.len().min(RSP_MAX);
        self.cut_unsent();
        self.rsp[..n].copy_from_slice(&data[..n]);
        self.ext = None;
        self.rsp_len = self.terminate(n);
        self.rsp_off = 0;
        self.rsp_ready = true;
        self.try_start_in();
    }

    /// Queue a response of `head` (at most RSP_MAX bytes) followed by `len`
    /// bytes that `fill(offset, out)` produces while the response goes out
    /// and the terminating '\n'
    /// (`pump_in`, a packet at a time; any offset order the host's
    /// TransferSize rounds give). The source must stay unchanged until
    /// `streaming` is false.
    pub fn respond_stream(&mut self, head: &[u8], len: usize, fill: fn(usize, &mut [u8])) {
        let h = head.len().min(RSP_MAX);
        self.cut_unsent();
        self.rsp[..h].copy_from_slice(&head[..h]);
        self.ext = Some(fill);
        self.head = h;
        self.ext_len = len;
        self.rsp_len = h + len + 1;
        self.rsp_off = 0;
        self.rsp_ready = true;
        self.try_start_in();
    }

    /// A streamed response (`respond_stream`) still has data to send: its
    /// source must not change. False once its last packet left, and as soon
    /// as the host gave it up (a new command, a clear, an abort, a reset).
    pub fn streaming(&self) -> bool {
        self.ext.is_some() && (self.rsp_ready || self.tx_len != 0)
    }

    /// A response transfer the host asked for is going out (any reply,
    /// streamed or not): main.rs polls on while it does.
    pub fn sending(&self) -> bool {
        self.tx_len != 0
    }

    /// Progress of a streamed response: bytes handed to the IN endpoint so
    /// far (rises while the host reads; main.rs `STREAM_STALL_MS`).
    pub fn stream_pos(&self) -> usize {
        self.rsp_off + self.tx_off
    }

    /// Give a streamed response up (the host stopped reading it and sent
    /// nothing since, e.g. its process died: Linux usbtmc sends no clear on
    /// close): the transfer in flight ends after the packets already in the
    /// FIFO, nothing more is read from the source, no further round starts.
    pub fn cancel_stream(&mut self) {
        if self.ext.is_some() {
            self.rsp_ready = false;
            self.stop_in();
            self.ext = None;
        }
    }

    /// Response bytes `a..a + dst.len()` (the head from `rsp`, the rest of
    /// a streamed response from its source, then its '\n').
    fn payload(&self, a: usize, dst: &mut [u8]) {
        let h = if self.ext.is_some() { self.head } else { RSP_MAX + 1 };
        let n1 = h.saturating_sub(a).min(dst.len());
        if n1 > 0 {
            dst[..n1].copy_from_slice(&self.rsp[a..a + n1]);
        }
        if let (Some(f), true) = (self.ext, n1 < dst.len()) {
            // Source bytes from offset `o`, then the terminator.
            let o = a + n1 - h;
            let n2 = self.ext_len.saturating_sub(o).min(dst.len() - n1);
            if n2 > 0 {
                f(o, &mut dst[n1..n1 + n2]);
            }
            if n1 + n2 < dst.len() {
                dst[n1 + n2] = NL;
            }
        }
    }

    /// Device clear (INITIATE_CLEAR): input and output buffers emptied, a
    /// pending REQUEST forgotten, a transfer in progress cut short
    /// (`stop_in`); the application is told through `take_cleared`.
    fn clear(&mut self) {
        self.abort_out();
        self.cmd_ready = false;
        self.req_tag = None;
        self.rsp_ready = false;
        self.stop_in();
        self.cleared = true;
    }

    /// Bus reset: everything (the core dropped its FIFOs and toggles).
    fn reset_all(&mut self) {
        self.clear();
        self.tx_len = 0;
        self.tx_off = 0;
        self.tx_zlp = false;
    }

    /// Drop a Bulk-OUT message being received.
    fn abort_out(&mut self) {
        self.hdr_len = 0;
        self.out_left = 0;
        self.out_pad = 0;
        self.cmd_len = 0;
    }

    /// End the Bulk-IN transfer in progress after the packets already in the
    /// FIFO (those cannot be taken back): no further data, and a zero-length
    /// packet if the last one written was full-size, so the host's read ends
    /// on a short packet (USBTMC 4.2.1.4 / 4.2.1.6). tx_len stays non-zero
    /// until that last packet has gone out (endpoint_in_complete -> pump_in);
    /// CHECK_*_STATUS look at `in_flight` instead.
    fn stop_in(&mut self) {
        if self.tx_len == 0 {
            return;
        }
        if self.tx_off == 0 {
            self.tx_len = 0;
            self.tx_zlp = false;
            return;
        }
        self.tx_zlp = self.tx_off % MPS == 0 && self.tx_off < self.tx_len || self.tx_zlp && self.tx_off == self.tx_len;
        self.tx_len = self.tx_off;
    }

    /// A Bulk-IN transfer still has something for the host: data not yet
    /// written, a packet in the endpoint the host has not read (EPENA), or
    /// the closing ZLP. `tx_len != 0` alone is not enough: usb-device handles
    /// EP0 before the IN completion collected in the same poll, so a
    /// CHECK_*_STATUS sent right after the host read the last packet still
    /// sees `tx_len != 0`; answering PENDING then makes Linux usbtmc wait 5 s
    /// for a packet that never comes and fail the clear ioctl.
    fn in_flight(&self) -> bool {
        self.tx_len != 0
            && (self.tx_off < self.tx_len || self.tx_zlp || crate::usb::in_busy(u8::from(self.ep_in.address())))
    }

    fn on_out_bytes(&mut self, mut d: &[u8]) {
        self.expire_partial();
        OUT_CYC.store(crate::prof::cyc(), Relaxed);
        while !d.is_empty() {
            if self.out_left == 0 && self.out_pad == 0 {
                // Header.
                let k = (12 - self.hdr_len).min(d.len());
                self.hdr[self.hdr_len..self.hdr_len + k].copy_from_slice(&d[..k]);
                self.hdr_len += k;
                d = &d[k..];
                if self.hdr_len < 12 {
                    return;
                }
                self.hdr_len = 0;
                let h = self.hdr;
                if h[1] != !h[2] {
                    continue; // bad tag pair: drop
                }
                let size = u32::from_le_bytes([h[4], h[5], h[6], h[7]]);
                match h[0] {
                    DEV_DEP_MSG_OUT => {
                        // A new command: a response not yet handed to the IN
                        // endpoint answers a query the host gave up on, and a
                        // REQUEST_DEV_DEP_MSG_IN from before it is stale; both
                        // would shift every later reply (IEEE 488.2: query
                        // interrupted). A transfer already started stays.
                        self.rsp_ready = false;
                        self.req_tag = None;
                        self.out_left = size;
                        self.out_pad = (4 - size % 4) % 4;
                        self.out_eom = h[8] & 1 != 0;
                        if size == 0 && self.out_eom {
                            self.cmd_ready = true;
                        }
                    }
                    REQUEST_DEV_DEP_MSG_IN => {
                        self.req_tag = Some(h[1]);
                        self.req_max = size;
                        self.try_start_in();
                    }
                    _ => {}
                }
            } else if self.out_left > 0 {
                let k = (self.out_left as usize).min(d.len());
                let room = CMD_MAX - self.cmd_len;
                let c = k.min(room);
                self.cmd[self.cmd_len..self.cmd_len + c].copy_from_slice(&d[..c]);
                self.cmd_len += c;
                self.out_left -= k as u32;
                d = &d[k..];
                if self.out_left == 0 && self.out_pad == 0 && self.out_eom {
                    self.cmd_ready = true;
                }
            } else {
                let k = (self.out_pad as usize).min(d.len());
                self.out_pad -= k as u32;
                d = &d[k..];
                if self.out_pad == 0 && self.out_eom {
                    self.cmd_ready = true;
                }
            }
        }
    }

    fn try_start_in(&mut self) {
        if self.tx_len != 0 || !self.rsp_ready {
            return;
        }
        let Some(tag) = self.req_tag.take() else { return };
        let rem = self.rsp_len - self.rsp_off;
        let n = rem.min(self.req_max as usize);
        let t = &mut self.tx_hdr;
        t[0] = DEV_DEP_MSG_IN;
        t[1] = tag;
        t[2] = !tag;
        t[3] = 0;
        t[4..8].copy_from_slice(&(n as u32).to_le_bytes());
        t[8] = (n == rem) as u8; // EOM
        t[9..12].fill(0);
        self.tx_base = self.rsp_off;
        self.tx_n = n;
        self.rsp_off += n;
        let len = (12 + n + 3) & !3;
        self.tx_len = len;
        self.tx_off = 0;
        self.tx_zlp = len % MPS == 0;
        self.last_tag_in = tag;
        self.rsp_ready = self.rsp_off < self.rsp_len;
        self.pump_in();
    }

    fn pump_in(&mut self) {
        if self.tx_off < self.tx_len {
            let end = (self.tx_off + MPS).min(self.tx_len);
            // Header, then payload from `rsp`, then the zero padding.
            let mut p = [0u8; MPS];
            let mut k = self.tx_off;
            while k < end {
                let o = k - self.tx_off;
                if k < 12 {
                    let e = end.min(12);
                    p[o..o + e - k].copy_from_slice(&self.tx_hdr[k..e]);
                    k = e;
                } else if k < 12 + self.tx_n {
                    let e = end.min(12 + self.tx_n);
                    self.payload(self.tx_base + k - 12, &mut p[o..o + e - k]);
                    k = e;
                } else {
                    break; // padding: already zero
                }
            }
            if let Ok(n) = self.ep_in.write(&p[..end - self.tx_off]) {
                self.tx_off += n;
            }
        } else if self.tx_len != 0 {
            if self.tx_zlp {
                if self.ep_in.write(&[]).is_ok() {
                    self.tx_zlp = false;
                }
            } else if crate::usb::in_busy(u8::from(self.ep_in.address())) {
                // The last packet (or the ZLP) is still in the endpoint: this
                // completion is the previous packet's. `poll` can write the
                // next one when the host took a packet between the driver's
                // XFRC check and `poll` (the host reads at any time); that
                // packet's XFRC then arrives after the write, and taking it
                // for the end of the transfer left the ZLP of a 64-byte
                // transfer (`*IDN?` since the 24-digit serial) in the FIFO
                // while CHECK_ABORT_BULK_IN_STATUS said done: the next query
                // read it as its reply (emu/usbtmc_stress.py abortin).
            } else {
                self.tx_len = 0;
                self.tx_off = 0;
            }
        }
    }
}

impl<B: UsbBus> Usbtmc<'_, B> {
    fn log(&mut self, req: &usb_device::control::Request, dir: u8) {
        self.dbg_ctl.0 = self.dbg_ctl.0.wrapping_add(1);
        self.dbg_ctl.1.copy_within(4..16, 0);
        self.dbg_ctl.1[12..16].copy_from_slice(&[
            dir | (req.request_type as u8) << 5 | req.recipient as u8,
            req.request,
            req.index as u8,
            req.length as u8,
        ]);
    }
}

impl<B: UsbBus> UsbClass<B> for Usbtmc<'_, B> {
    fn get_configuration_descriptors(&self, w: &mut DescriptorWriter) -> Result<()> {
        w.interface(self.iface, CLASS, SUBCLASS, PROTOCOL_USB488)?;
        w.endpoint(&self.ep_out)?;
        w.endpoint(&self.ep_in)?;
        Ok(())
    }

    fn reset(&mut self) {
        self.reset_all();
    }

    fn control_out(&mut self, xfer: ControlOut<B>) {
        let req = *xfer.request();
        self.log(&req, 0);
        // Standard CLEAR_FEATURE(ENDPOINT_HALT) on any endpoint (ours and the
        // disk's): reset the data toggle the driver leaves alone; usb-device
        // then clears STALL and acknowledges (not accepted here).
        if req.request_type == RequestType::Standard
            && req.recipient == Recipient::Endpoint
            && req.request == usb_device::control::Request::CLEAR_FEATURE
            && req.value == usb_device::control::Request::FEATURE_ENDPOINT_HALT
        {
            crate::usb::reset_toggle(req.index as u8);
        }
    }

    fn endpoint_out(&mut self, addr: EndpointAddress) {
        // The previous command not taken by the application yet: leave this
        // packet in the driver's buffer (the endpoint NAKs further packets
        // meanwhile) so the two commands are not concatenated. It is
        // re-reported on the next poll. With take_command called right after
        // dev.poll in every main-loop iteration this is a safety net.
        if addr != self.ep_out.address() || self.cmd_ready {
            return;
        }
        let mut buf = [0u8; MPS];
        if let Ok(n) = self.ep_out.read(&mut buf) {
            self.on_out_bytes(&buf[..n]);
        }
    }

    fn endpoint_in_complete(&mut self, addr: EndpointAddress) {
        if addr == self.ep_in.address() {
            self.pump_in();
        }
    }

    fn poll(&mut self) {
        // A packet (or the closing ZLP) whose write found the endpoint still
        // busy: retry here, not only on the next IN completion, which never
        // comes then (the emulator's DWC2 model once raised XFRC before
        // clearing EPENA; a stuck transfer starves every later reply).
        if self.tx_off < self.tx_len || (self.tx_zlp && self.tx_off == self.tx_len && self.tx_len != 0) {
            self.pump_in();
        }
        self.try_start_in();
    }

    fn control_in(&mut self, xfer: ControlIn<B>) {
        let req = *xfer.request();
        self.log(&req, 0x80);
        if req.request_type != RequestType::Class {
            return;
        }
        let ours = match req.recipient {
            Recipient::Interface => req.index == u8::from(self.iface) as u16,
            Recipient::Endpoint => {
                req.index as u8 == u8::from(self.ep_in.address())
                    || req.index as u8 == u8::from(self.ep_out.address())
            }
            _ => false,
        };
        if !ours {
            return;
        }
        let tag = req.value as u8;
        let _ = match req.request {
            GET_CAPABILITIES => xfer.accept_with(&CAPABILITIES),
            INITIATE_CLEAR => {
                self.clear();
                xfer.accept_with(&[STATUS_SUCCESS])
            }
            // Pending (bmClear D0 = 1: read Bulk-IN up to a short packet)
            // while the cut-short transfer is still leaving the FIFO.
            CHECK_CLEAR_STATUS => {
                if self.in_flight() {
                    xfer.accept_with(&[STATUS_PENDING, 1])
                } else {
                    xfer.accept_with(&[STATUS_SUCCESS, 0])
                }
            }
            INITIATE_ABORT_BULK_OUT => {
                // In progress = a header or payload partly received; its bTag
                // is hdr[1] once the header is complete.
                let busy = self.hdr_len > 0 || self.out_left > 0 || self.out_pad > 0;
                let st = if !busy {
                    STATUS_FAILED
                } else if self.hdr_len == 0 && self.hdr[1] != tag {
                    STATUS_TRANSFER_NOT_IN_PROGRESS
                } else {
                    self.abort_out();
                    STATUS_SUCCESS
                };
                // The host then clears the Bulk-OUT halt (toggle reset above).
                xfer.accept_with(&[st, tag])
            }
            CHECK_ABORT_BULK_OUT_STATUS => xfer.accept_with(&[STATUS_SUCCESS, 0, 0, 0, 0, 0, 0, 0]),
            INITIATE_ABORT_BULK_IN => {
                // The host gives up on its read: forget the REQUEST and any
                // unsent rest of the response either way, and tell the
                // application (no late reply to a READ? still waiting).
                self.rsp_ready = false;
                self.req_tag = None;
                self.cleared = true;
                let st = if self.tx_len == 0 {
                    STATUS_FAILED
                } else if self.last_tag_in != tag {
                    STATUS_TRANSFER_NOT_IN_PROGRESS
                } else {
                    self.stop_in();
                    STATUS_SUCCESS
                };
                xfer.accept_with(&[st, self.last_tag_in])
            }
            CHECK_ABORT_BULK_IN_STATUS => {
                // bmAbortBulkIn D0 = 1: the host must read until a short packet.
                let n = (self.tx_off.saturating_sub(12) as u32).to_le_bytes();
                if self.in_flight() {
                    xfer.accept_with(&[STATUS_PENDING, 1, 0, 0, n[0], n[1], n[2], n[3]])
                } else {
                    xfer.accept_with(&[STATUS_SUCCESS, 0, 0, 0, n[0], n[1], n[2], n[3]])
                }
            }
            INDICATOR_PULSE => xfer.accept_with(&[STATUS_SUCCESS]),
            READ_STATUS_BYTE => {
                let sb = self.status_byte | if self.rsp_ready { 0x10 } else { 0 }; // MAV
                xfer.accept_with(&[STATUS_SUCCESS, tag, sb])
            }
            REN_CONTROL | GO_TO_LOCAL | LOCAL_LOCKOUT => xfer.accept_with(&[STATUS_SUCCESS]),
            _ => xfer.reject(),
        };
    }
}
