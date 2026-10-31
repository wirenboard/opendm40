#!/usr/bin/env python3
"""USBTMC stress test of opendm40 in the emulator, over USB/IP (emu/usbip_client.py).

    emu/run.py fwrs --fast --usbip 4 --run-for 3000 &        # USB/IP on TCP 3240
    emu/usbtmc_stress.py [--port 3240] [--iters 5000] [--seed N] [--cases a,b,..]

Thousands of back-to-back command / query exchanges with varied shapes, each
reply checked byte-exactly where it is deterministic:

  pair     set command then its query (SYST:IDN:EMUL 34401A|OFF, then
           SYST:IDN:EMUL? and *IDN?), each its own DEV_DEP_MSG_OUT
  combo    several commands in one message (';'), incl. multi-packet
           Bulk-OUT messages up to the opendm40 256-byte command buffer
  split    one command in two DEV_DEP_MSG_OUT transfers (EOM only on the second)
  big      multi-packet replies: WAV:DATA? (#3400 block + '\n': a 420-byte
           transfer, 7 packets),
           DBG:PROF?, DBG:LCD? (up to 3 KiB: ~50 packets, a ZLP when the
           transfer ends on a packet boundary)
  small    a reply fetched in several REQUEST_DEV_DEP_MSG_IN rounds
           (TransferSize 1..40 bytes, EOM only on the last)
  pipe     DEV_DEP_MSG_OUT, REQUEST and the Bulk-IN URB submitted without
           waiting for each other's completion
  stale    a REQUEST_DEV_DEP_MSG_IN never read, then a new command: the
           device must forget it (no reply shifted into the next query)
  clear    query + REQUEST, then INITIATE_CLEAR / CHECK_CLEAR_STATUS
           (reading Bulk-IN while PENDING), then a fresh query must answer
  abortin  query + REQUEST, INITIATE_ABORT_BULK_IN / CHECK_ABORT_BULK_IN_STATUS
  abortout a multi-packet command cut after its first packet,
           INITIATE_ABORT_BULK_OUT / CHECK_ABORT_BULK_OUT_STATUS / CLEAR_FEATURE
  read     READ? (a DMM reading; any number)
  partial  a dead client's message cut after its first packet (header +
           52 bytes of 150, no abort, no clear: Linux usbtmc kills the URB
           on close and sends nothing else), then after 2 s (virtual
           time, from the DBG:PING? uptime) a plain
           query must answer (opendm40 drops the stale part after
           usbtmc::OUT_STALE_MS; before that the next header was read as
           its payload)
  session  tools/dm40dbg.py's own upload (cmd_upload, over this USB/IP
           link: DbgLink) of a small valid update file, after an earlier
           client left an error in the queue (an undefined command) and,
           half the time, a cut message (as `partial`): the upload must
           succeed (it failed with the stale -113 "Undefined header" at
           DBG:UPLOAD:BEGIN, TODO "reload.sh first-upload failures"); a
           second BEGIN + ABORT then deletes the DM40FW.BIN again
  compound one message mixing commands the main loop runs later (OUTP,
           SOUR:FREQ with suffixes and with bad values (no reply, the
           error queue in order),
           SOUR:FUNC, POW:PHAS:CAL, MMEM:DATA?) with text queries, a
           reading wait (READ?) and SYST:ERR?: every query answered in
           order, ';'-joined, in one response (opendm40 scpi_msg.rs); a block
           query (MMEM:DATA?) beside other queries is refused ("Query
           error"), alone with a missing file it answers empty

Random delays (0..30 ms) go between the stages. Renode's USB/IP server
handles URBs strictly in order and an IN URB blocks it until the device
sends a packet, so the test submits a Bulk-IN URB only when a reply is due;
a reply that never comes shows up as a RET_SUBMIT timeout (--timeout, wall
seconds) and the run stops with the history of the last exchanges (and,
with --monitor PORT, the USBFS registers read through the Renode monitor).
"""
import argparse
import atexit
import contextlib
import io
import random
import re
import socket
import struct
import sys
import tempfile
import time
import os

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import usbip_client as u  # noqa: E402
sys.path.insert(0, os.path.join(os.path.dirname(HERE), 'firmware', 'tools'))
sys.path.insert(0, os.path.join(os.path.dirname(HERE), 'stock'))
import dm40dbg  # noqa: E402
import dm40fw  # noqa: E402

DEV_DEP_MSG_OUT, REQUEST_DEV_DEP_MSG_IN = 1, 2
# The native identity; its serial field (the chip UID as 24 hex digits,
# firmware/src/uid.rs, emu/run.py --uid) is taken from the first *IDN? (main).
IDN = None
IDN_RE = re.compile(rb'Alientek,DM40[ABC],[0-9A-F]{24},opendm40 0\.1')
IDN_34401A = b'HEWLETT-PACKARD,34401A,0,opendm40 0.1'


class Hang(Exception):
    pass


class Tmc:
    def __init__(self, d, timeout):
        found = u.find_bulk(u.parse_config(d.config_descriptor()), 0xFE, 3)
        self.iface, self.ep_out, self.ep_in = found
        d.set_configuration(1)
        self.d, self.tag, self.timeout = d, 0, timeout
        self.log = []

    def note(self, s):
        self.log.append('%.3f %s' % (time.monotonic(), s))
        del self.log[:-40]

    def next_tag(self):
        self.tag = self.tag % 255 + 1
        return self.tag

    # ---- raw URBs (seq-matched RET_SUBMIT) ----
    def submit(self, direction, ep, setup=b'\0' * 8, out_data=b'', in_len=0):
        self.d.send_submit(direction, ep, setup, out_data, in_len)
        return self.d.seq

    def ret(self, what):
        self.d.s.settimeout(self.timeout)
        try:
            rh = u.recvn(self.d.s, 48)
            cmd, seq, _, dirn, _, status, actual = struct.unpack('>IIIIIIi', rh[:28])
            data = u.recvn(self.d.s, actual) if dirn == u.DIR_IN and actual > 0 else b''
        except socket.timeout:
            raise Hang('no RET_SUBMIT within %.0f s for %s' % (self.timeout, what))
        finally:
            self.d.s.settimeout(20)
        if cmd != u.RET_SUBMIT:
            raise IOError('reply cmd 0x%x' % cmd)
        return seq, status, data

    def out_msg(self, payload, eom=True, tag=None):
        t = tag or self.next_tag()
        hdr = struct.pack('<BBBBIB3x', DEV_DEP_MSG_OUT, t, 0xFF ^ t, 0, len(payload), 1 if eom else 0)
        return hdr + payload + b'\0' * ((-len(payload)) % 4)

    def req_msg(self, maxlen):
        t = self.next_tag()
        return t, struct.pack('<BBBBIBB2x', REQUEST_DEV_DEP_MSG_IN, t, 0xFF ^ t, 0, maxlen, 0, 0)

    def bulk_out(self, data, what):
        seq = self.submit(u.DIR_OUT, self.ep_out, out_data=data)
        s, st, _ = self.ret(what)
        if s != seq or st != 0:
            raise IOError('%s: RET seq %d/%d status %d' % (what, s, seq, st))

    def bulk_in(self, n, what):
        seq = self.submit(u.DIR_IN, self.ep_in & 0x7F, in_len=n)
        s, st, data = self.ret(what)
        if s != seq or st != 0:
            raise IOError('%s: RET seq %d/%d status %d' % (what, s, seq, st))
        return data

    def ctl_in(self, bmrt, req, value, index, length, what):
        setup = struct.pack('<BBHHH', bmrt, req, value, index, length)
        seq = self.submit(u.DIR_IN, 0, setup, in_len=length)
        s, st, data = self.ret(what)
        if s != seq or st != 0:
            raise IOError('%s: RET seq %d/%d status %d' % (what, s, seq, st))
        return data

    def ctl_out(self, bmrt, req, value, index, what):
        setup = struct.pack('<BBHHH', bmrt, req, value, index, 0)
        seq = self.submit(u.DIR_OUT, 0, setup)
        s, st, _ = self.ret(what)
        if s != seq or st != 0:
            raise IOError('%s: RET seq %d/%d status %d' % (what, s, seq, st))

    # ---- message layer ----
    def write(self, text, delay=0.0):
        self.note('OUT %r' % text[:60])
        self.bulk_out(self.out_msg(text.encode() + b'\n'), 'DEV_DEP_MSG_OUT %r' % text[:40])
        if delay:
            time.sleep(delay)

    def read_transfer(self, tag, what):
        """One DEV_DEP_MSG_IN transfer (ends on a short packet): payload, EOM."""
        buf = b''
        while True:
            pkt = self.bulk_in(64, what)
            buf += pkt
            if len(pkt) < 64:
                break
        if len(buf) < 12:
            raise IOError('%s: short transfer %r' % (what, buf))
        mid, btag, btag_inv, _, size, attr = struct.unpack_from('<BBBBIB', buf)
        if mid != 2 or btag != tag or btag_inv != 0xFF ^ tag:
            raise IOError('%s: header %s (expected bTag %d)' % (what, buf[:12].hex(), tag))
        exp = 12 + size + (-size) % 4
        if len(buf) != exp:
            raise IOError('%s: transfer %d bytes, header says %d (+pad)' % (what, len(buf), size))
        return buf[12:12 + size], bool(attr & 1)

    def read(self, maxlen=4096, delay=0.0, what='reply'):
        out = b''
        while True:
            tag, req = self.req_msg(maxlen)
            self.note('REQ tag %d max %d' % (tag, maxlen))
            self.bulk_out(req, 'REQUEST (%s)' % what)
            if delay:
                time.sleep(delay)
            data, eom = self.read_transfer(tag, what)
            if len(data) > maxlen:
                raise IOError('%s: %d bytes > TransferSize %d' % (what, len(data), maxlen))
            out += data
            self.note('IN %d bytes eom %d: %r' % (len(data), eom, data[:40]))
            if eom:
                return out
            if not data:
                raise IOError('%s: empty transfer without EOM' % what)

    def query(self, text, **kw):
        self.write(text)
        return self.read(what=text[:40], **kw)

    def device_clear(self):
        """INITIATE_CLEAR, CHECK_CLEAR_STATUS until done (reading Bulk-IN
        up to a short packet while PENDING), CLEAR_FEATURE(HALT) of
        Bulk-OUT: what Linux usbtmc's USBTMC_IOCTL_CLEAR sends."""
        st = self.ctl_in(0xA1, 5, 0, self.iface, 1, 'INITIATE_CLEAR')
        self.note('INITIATE_CLEAR -> %s' % st.hex())
        if st[0] != 0x01:
            raise IOError('INITIATE_CLEAR %s' % st.hex())
        for _ in range(200):
            cs = self.ctl_in(0xA1, 6, 0, self.iface, 2, 'CHECK_CLEAR_STATUS')
            self.note('CHECK_CLEAR_STATUS -> %s' % cs.hex())
            if cs[0] == 0x02:
                if cs[1] & 1:
                    while len(self.bulk_in(64, 'clear drain')) == 64:
                        pass
                continue
            if cs[0] != 0x01:
                raise IOError('CHECK_CLEAR_STATUS %s' % cs.hex())
            break
        else:
            raise IOError('clear: still pending')
        self.ctl_out(0x02, 1, 0, self.ep_out, 'CLEAR_FEATURE(HALT) OUT')


class DbgLink(dm40dbg.Link):
    """tools/dm40dbg.py's Link over this USB/IP connection instead of
    /dev/usbtmcN: its own query / try_query / upload code runs unchanged."""

    def __init__(self, tmc):
        self.t, self.fd, self.cleared = tmc, None, 0

    def write(self, cmd):
        self.t.write(cmd)

    def write_raw(self, msg):
        self.t.note('OUT raw %r' % msg[:40])
        self.t.bulk_out(self.t.out_msg(msg), 'DEV_DEP_MSG_OUT raw %r' % msg[:30])

    def read(self, n=1 << 16):
        return self.t.read(what='dm40dbg read')

    def clear(self):
        self.t.device_clear()


def nl(got, what):
    """The reply without its '\\n' terminator: every opendm40 USBTMC reply ends
    with one since b3cbfbf (scpi.rs header), a block reply too (after the
    block, as the last byte of the EOM transfer: docs/bt-scpi.md "Scope
    waveform readout"). An empty reply (a failed block query) has none."""
    if not got:
        return got
    if not got.endswith(b'\n'):
        raise IOError('%s: reply without the final newline: %r' % (what, got[-40:]))
    return got[:-1]


def expect(got, want, what):
    got = nl(got, what)
    if got != want:
        raise IOError('%s: got %r, expected %r' % (what, got[:120], want[:120]))


class Stress:
    def __init__(self, tmc, rnd):
        self.t, self.r = tmc, rnd
        self.emul = None  # unknown until set
        # DBG:PROF? only in a build with the feature dbg-prof (else -113).
        tmc.query('DBG:PROF?')
        self.has_prof = not tmc.query('SYST:ERR?').startswith(b'-113')

    def d(self):
        r = self.r.random()
        return 0.0 if r < 0.5 else self.r.uniform(0, 0.03)

    def idn(self):
        return IDN_34401A if self.emul else IDN

    def set_emul(self, on):
        self.t.write('SYST:IDN:EMUL %s' % ('34401A' if on else 'OFF'), delay=self.d())
        self.emul = on

    def check_emul(self):
        expect(self.t.query('SYST:IDN:EMUL?', delay=self.d()), b'34401A' if self.emul else b'OFF', 'SYST:IDN:EMUL?')

    # ---- cases ----
    def c_pair(self):
        self.set_emul(self.r.random() < 0.5)
        self.check_emul()
        expect(self.t.query('*IDN?', delay=self.d()), self.idn(), '*IDN?')

    def c_combo(self):
        on = self.r.random() < 0.5
        n = self.r.randint(0, 12)
        parts = ['*CLS'] * n + ['SYST:IDN:EMUL %s' % ('34401A' if on else 'OFF'), ':SYST:IDN:EMUL?', '*OPC?', '*IDN?']
        cmd = ';'.join(parts)
        self.emul = on
        want = b';'.join([b'34401A' if on else b'OFF', b'1', self.idn()])
        expect(self.t.query(cmd, delay=self.d()), want, 'combo(%d B)' % len(cmd))

    def c_split(self):
        cmd = b'*IDN?\n'
        k = self.r.randint(1, len(cmd) - 1)
        self.t.note('OUT split %r | %r' % (cmd[:k], cmd[k:]))
        self.t.bulk_out(self.t.out_msg(cmd[:k], eom=False), 'split 1')
        if self.r.random() < 0.5:
            time.sleep(self.d())
        self.t.bulk_out(self.t.out_msg(cmd[k:], eom=True), 'split 2')
        expect(self.t.read(delay=self.d(), what='split *IDN?'), self.idn(), 'split *IDN?')

    def c_big(self):
        k = self.r.randint(0, 2)
        if k == 1 and not self.has_prof:
            k = 2
        if k == 0:
            rep = nl(self.t.query('WAV:DATA?', delay=self.d()), 'WAV:DATA?')
            if len(rep) != 405 or not rep.startswith(b'#3400'):
                raise IOError('WAV:DATA?: %d bytes %r' % (len(rep), rep[:8]))
        elif k == 1:
            rep = nl(self.t.query('DBG:PROF?', delay=self.d()), 'DBG:PROF?')
            if len(rep) < 64:
                raise IOError('DBG:PROF?: %d bytes' % len(rep))
        else:
            # n words -> 6 + 2n bytes + '\n'; pick n so that some transfers
            # end on a 64-byte boundary (ZLP): 12 + 7 + 2n + 1 (pad) = 64 m.
            if self.r.random() < 0.5:
                m = self.r.randint(2, 48)
                n = 32 * m - 10
            else:
                n = self.r.randint(1, 1500)
            rep = nl(self.t.query('DBG:LCD? 0,0,40,40,%d' % n, delay=self.d()), 'DBG:LCD?')
            want_hdr = b'#4%04d' % (2 * n)
            if len(rep) != 6 + 2 * n or rep[:6] != want_hdr:
                raise IOError('DBG:LCD? n=%d: %d bytes %r' % (n, len(rep), rep[:6]))

    def c_small(self):
        self.t.write('*IDN?')
        rep = self.t.read(maxlen=self.r.randint(1, 40), delay=self.d(), what='*IDN? small')
        expect(rep, self.idn(), '*IDN? in small rounds')

    def c_pipe(self):
        cmd = self.t.out_msg(b'*IDN?\n')
        tag, req = self.t.req_msg(4096)
        self.t.note('PIPE OUT+REQ+IN tag %d' % tag)
        s1 = self.t.submit(u.DIR_OUT, self.t.ep_out, out_data=cmd)
        s2 = self.t.submit(u.DIR_OUT, self.t.ep_out, out_data=req)
        s3 = self.t.submit(u.DIR_IN, self.t.ep_in & 0x7F, in_len=64)
        rets = {}
        for _ in range(3):
            s, st, data = self.t.ret('pipelined *IDN?')
            rets[s] = (st, data)
        pkt = rets[s3][1]
        buf = pkt
        while len(pkt) == 64:
            pkt = self.t.bulk_in(64, 'pipelined rest')
            buf += pkt
        if buf[1] != tag or nl(buf[12:12 + struct.unpack_from('<I', buf, 4)[0]], 'pipelined *IDN?') != self.idn():
            raise IOError('pipelined *IDN?: %r' % buf)

    def c_stale(self):
        # A REQUEST the host never reads, then a new command and query.
        self.t.write('*OPC?')
        tag, req = self.t.req_msg(4096)
        self.t.note('stale REQ tag %d' % tag)
        self.t.bulk_out(req, 'stale REQUEST')
        time.sleep(self.d())
        # The device may already have the reply in the IN FIFO: the host
        # would have to read it (or abort). Abort it as a host would.
        self.abort_in(tag)
        expect(self.t.query('*IDN?', delay=self.d()), self.idn(), '*IDN? after stale')

    def abort_in(self, tag):
        st = self.t.ctl_in(0xA2, 3, tag, self.t.ep_in, 2, 'INITIATE_ABORT_BULK_IN')
        self.t.note('ABORT_IN tag %d -> %s' % (tag, st.hex()))
        if st[0] == 0x01:
            for _ in range(200):
                cs = self.t.ctl_in(0xA2, 4, 0, self.t.ep_in, 8, 'CHECK_ABORT_BULK_IN_STATUS')
                self.t.note('CHECK_ABORT_BULK_IN_STATUS -> %s' % cs.hex())
                if cs[0] == 0x02:  # PENDING: read Bulk-IN up to a short packet
                    if cs[1] & 1:
                        while True:
                            pkt = self.t.bulk_in(64, 'abort-in drain')
                            self.t.note('IN drain %d bytes' % len(pkt))
                            if len(pkt) < 64:
                                break
                    continue
                if cs[0] != 0x01:
                    raise IOError('CHECK_ABORT_BULK_IN_STATUS %s' % cs.hex())
                break
            else:
                raise IOError('abort in: still pending')
            self.t.ctl_out(0x02, 1, 0, self.t.ep_in, 'CLEAR_FEATURE(HALT) IN')
        elif st[0] not in (0x80, 0x81):
            raise IOError('INITIATE_ABORT_BULK_IN %s' % st.hex())
        # 0x80 FAILED / 0x81 not in progress: nothing was sent yet (the
        # REQUEST is forgotten by the device all the same).

    def c_clear(self):
        # Also a compound message cleared while its READ? waits (no late
        # reply of its rest after the clear).
        self.t.write(self.r.choice(['*IDN?', '*IDN?', '*IDN?;READ?;*OPC?']))
        if self.r.random() < 0.7:
            tag, req = self.t.req_msg(self.r.choice([4096, 20, 5]))
            self.t.bulk_out(req, 'REQUEST before clear')
        time.sleep(self.d())
        self.t.device_clear()
        expect(self.t.query('*IDN?', delay=self.d()), self.idn(), '*IDN? after clear')

    def c_abortin(self):
        self.t.write(self.r.choice(['*IDN?', 'WAV:DATA?', 'DBG:LCD? 0,0,40,40,1000']))
        tag, req = self.t.req_msg(4096)
        self.t.note('REQ tag %d max 4096' % tag)
        self.t.bulk_out(req, 'REQUEST before abort')
        time.sleep(self.d())
        if self.r.random() < 0.5:
            pkt = self.t.bulk_in(64, 'first packet before abort')  # a transfer in progress
            self.t.note('IN first packet %d bytes' % len(pkt))
        self.abort_in(tag)
        expect(self.t.query('*IDN?', delay=self.d()), self.idn(), '*IDN? after abort-in')

    def c_abortout(self):
        payload = (b'*CLS;' * 30)[:-1] + b'\n'  # 150 bytes: 3 packets
        msg = self.t.out_msg(payload)
        tag = msg[1]
        self.t.bulk_out(msg[:64], 'abort-out first packet')
        time.sleep(self.d())
        st = self.t.ctl_in(0xA2, 1, tag, self.t.ep_out, 2, 'INITIATE_ABORT_BULK_OUT')
        self.t.note('ABORT_OUT tag %d -> %s' % (tag, st.hex()))
        if st[0] != 0x01:
            raise IOError('INITIATE_ABORT_BULK_OUT %s' % st.hex())
        cs = self.t.ctl_in(0xA2, 2, 0, self.t.ep_out, 8, 'CHECK_ABORT_BULK_OUT_STATUS')
        if cs[0] != 0x01:
            raise IOError('CHECK_ABORT_BULK_OUT_STATUS %s' % cs.hex())
        self.t.ctl_out(0x02, 1, 0, self.t.ep_out, 'CLEAR_FEATURE(HALT) OUT')
        expect(self.t.query('*IDN?', delay=self.d()), self.idn(), '*IDN? after abort-out')

    def cut_message(self):
        """A dead client's message: its first packet only (header + 52 of
        150 bytes), no abort, no clear."""
        msg = self.t.out_msg((b'*CLS;' * 30)[:-1] + b'\n')
        self.t.note('cut message tag %d: 64 of %d bytes' % (msg[1], len(msg)))
        self.t.bulk_out(msg[:64], 'cut message first packet')

    def uptime(self):
        return int(nl(self.t.query('DBG:PING?'), 'DBG:PING?').split(b',')[0]), time.monotonic()

    def c_partial(self):
        # 2 s of virtual time, twice OUT_STALE_MS (the emulator runs slower
        # than the wall clock under load, and not evenly): its rate from two
        # DBG:PING? 0.5 s apart.
        v0, w0 = self.uptime()
        time.sleep(0.5)
        v1, w1 = self.uptime()
        rate = max((v1 - v0) / 1000 / max(w1 - w0, 1e-3), 0.05)
        self.cut_message()
        self.t.note('virtual time rate %.2f: waiting %.1f s' % (rate, min(2 / rate, 30)))
        time.sleep(min(2 / rate, 30))
        expect(self.t.query('*IDN?', delay=self.d()), self.idn(), '*IDN? after a cut message')

    def c_session(self):
        # The earlier client: an undefined command (its -113 stays queued:
        # nobody read SYST:ERR?), maybe a cut message.
        self.t.write('FOO:BAR')
        if self.r.random() < 0.5:
            self.cut_message()
        path = os.path.join(SESSION_DIR, 'dm40_session_%d.bin' % os.getpid())
        if not os.path.exists(path):
            with open(path, 'wb') as f:
                f.write(dm40fw.build(bytes(range(256)) * 8, pad=True))
            atexit.register(os.remove, path)
        ln = DbgLink(self.t)
        out = io.StringIO()
        try:
            with contextlib.redirect_stdout(out), contextlib.redirect_stderr(out):
                r = dm40dbg.cmd_upload(ln, path, quiet=True)
        except SystemExit as e:
            raise IOError('dm40dbg upload after a stale client: %s (output %r)' % (e, out.getvalue()))
        if r != 0 or 'earlier client' not in out.getvalue():
            raise IOError('dm40dbg upload returned %r, output %r (want 0 and the stale error reported)' % (r, out.getvalue()))
        # BEGIN deletes every dm40*.bin of the root: the disk as before.
        self.t.write('DBG:UPLOAD:BEGIN 128,0')
        self.t.write('DBG:UPLOAD:ABORT')
        self.drain_errors()

    def c_read(self):
        rep = nl(self.t.query('READ?', delay=self.d()), 'READ?')
        float(rep)

    def c_compound(self):
        k = self.r.randint(0, 8)
        if k == 0:
            on = self.r.random() < 0.5
            rep = self.t.query('*CLS;OUTP %s;:OUTP?;*OPC?;:SYST:ERR?' % ('ON' if on else 'OFF'), delay=self.d())
            expect(rep, b'%d;1;0,"No error"' % on, 'OUTP;OUTP?;*OPC?;SYST:ERR?')
            if on:
                self.t.write('OUTP OFF')
        elif k == 1:
            w = self.r.choice([b'SIN', b'SQU', b'TRI', b'RAMP'])
            rep = self.t.query('SOUR:FUNC %s;:SOUR:FUNC?;*IDN?' % w.decode(), delay=self.d())
            expect(rep, w + b';' + self.idn(), 'SOUR:FUNC;SOUR:FUNC?;*IDN?')
        elif k == 2:
            # The reading wait pauses the message; the rest runs after it.
            rep = nl(self.t.query('*IDN?;READ?;*OPC?;:SYST:ERR?', delay=self.d()), '*IDN?;READ?;*OPC?;SYST:ERR?')
            p = rep.split(b';')
            if len(p) != 4 or p[0] != self.idn() or p[2] != b'1' or not p[3].endswith(b'"'):
                raise IOError('*IDN?;READ?;*OPC?;SYST:ERR?: %r' % rep)
            float(p[1])
        elif k == 3:
            # The reported hang: a command the app runs, then a query.
            rep = nl(self.t.query('*CLS;POW:PHAS:CAL RES;:SYST:ERR?;*OPC?', delay=self.d()), 'POW:PHAS:CAL RES;SYST:ERR?')
            p = rep.rsplit(b';', 1)  # the error text may hold a ';'
            if len(p) != 2 or not (p[0].startswith(b'-') or p[0] == b'0,"No error"') or p[1] != b'1':
                raise IOError('POW:PHAS:CAL RES;SYST:ERR?: %r' % rep)
        elif k == 4:
            rep = self.t.query('*CLS;MMEM:DATA? "NOPE.DLG";:SYST:ERR?', delay=self.d())
            expect(rep, b'-400,"Query error"', 'MMEM:DATA?;SYST:ERR?')
        elif k == 5:
            # A failed block query alone: an empty response, the error queued.
            expect(self.t.query('*CLS;MMEM:DATA? "NOPE.DLG"', delay=self.d()), b'', 'MMEM:DATA? missing file')
            expect(self.t.query('SYST:ERR?'), b'-256,"File name not found"', 'SYST:ERR? after MMEM:DATA?')
        elif k == 7:
            # IEEE 488.2 suffixes: KHZ / MHZ / M (milli) / MA (mega); the
            # generator answers the frequency it produces (1 kHz exact).
            rep = self.t.query('*CLS;SOUR:FUNC SQU;:SOUR:FREQ 1KHZ;:SOUR:FREQ?;:SOUR:FREQ 0.001MA;:SOUR:FREQ?;:SYST:ERR?', delay=self.d())
            expect(rep, b'+1.000000E+03;+1.000000E+03;0,"No error"', 'SOUR:FREQ 1KHZ / 0.001MA')
        elif k == 8:
            # A bad value on a set command: no reply, the setting kept, the
            # error queued (FIFO: -224, then -109, then -222).
            rep = self.t.query('*CLS;SOUR:FUNC SQU;:SOUR:FREQ 1000;:SOUR:FREQ abc;:SOUR:FREQ;:SOUR:FREQ -5;:SOUR:FREQ?;'
                               ':SYST:ERR?;:SYST:ERR?;:SYST:ERR?;:SYST:ERR?', delay=self.d())
            expect(rep, b'+1.000000E+03;-224,"Illegal parameter value";-109,"Missing parameter";-222,"Data out of range";0,"No error"',
                   'bad SOUR:FREQ values')
        else:
            rep = nl(self.t.query('*OPC?;WAV:DATA?;*IDN?', delay=self.d()), '*OPC?;WAV:DATA?;*IDN?')
            if len(rep) != 2 + 405 + 1 + len(self.idn()) or not rep.startswith(b'1;#3400') or not rep.endswith(b';' + self.idn()):
                raise IOError('*OPC?;WAV:DATA?;*IDN?: %d bytes %r' % (len(rep), rep[:12]))

    def drain_errors(self):
        for _ in range(30):
            if self.t.query('SYST:ERR?').startswith(b'+0,') or self.t.query('SYST:ERR?').startswith(b'0,'):
                return


CASES = dict(pair=8, combo=3, split=2, big=3, small=3, pipe=3, stale=2, clear=2, abortin=2, abortout=1, read=1, compound=3,
             partial=1, session=1)
# The small update file of the `session` case.
SESSION_DIR = tempfile.gettempdir()


def monitor_dump(port):
    """USBFS registers through the Renode monitor (renode -P PORT)."""
    regs = [('GINTSTS', 0x14), ('GRXSTSR', 0x1C), ('DIEPCTL1', 0x920), ('DIEPINT1', 0x928), ('DIEPTSIZ1', 0x930),
            ('DOEPCTL1', 0xB20), ('DOEPINT1', 0xB28), ('DOEPTSIZ1', 0xB30)]
    try:
        s = socket.create_connection(('127.0.0.1', port), timeout=5)
        time.sleep(0.5)
        s.recv(65536)
        out = []
        for name, off in regs:
            s.sendall(b'sysbus ReadDoubleWord 0x%08x\n' % (0x50000000 + off))
            time.sleep(0.3)
            out.append('%s %s' % (name, s.recv(65536).decode(errors='replace').strip().splitlines()[-2:]))
        return '\n'.join(out)
    except OSError as e:
        return 'monitor: %s' % e


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument('--port', type=int, default=int(os.environ.get('USBIP_PORT', 3240)))
    ap.add_argument('--busid', default='1-0')
    ap.add_argument('--iters', type=int, default=3000)
    ap.add_argument('--seed', type=int, default=None)
    ap.add_argument('--cases', default=','.join(CASES), help='comma-separated subset of: ' + ' '.join(CASES))
    ap.add_argument('--timeout', type=float, default=15.0, help='wall seconds for a RET_SUBMIT (virtual time is slower)')
    ap.add_argument('--monitor', type=int, default=None, help='Renode monitor port for a register dump on a hang')
    a = ap.parse_args()
    u.PORT = a.port
    seed = a.seed if a.seed is not None else random.randrange(1 << 30)
    rnd = random.Random(seed)
    names = [c for c in a.cases.split(',') if c]
    weights = [CASES[c] for c in names]
    while True:
        try:
            if any(x['busid'] == a.busid for x in u.devlist()):
                break
        except OSError:
            pass
        time.sleep(0.5)
    t = Tmc(u.Dev(a.busid), a.timeout)
    st = Stress(t, rnd)
    print('seed %d, cases %s' % (seed, ' '.join(names)), flush=True)
    st.set_emul(False)
    st.drain_errors()
    global IDN
    IDN = t.query('*IDN?').rstrip(b'\n')
    if not IDN_RE.fullmatch(IDN):
        print('FAIL: *IDN? %r (want Alientek,DM40A|B|C,<24 hex digits>,opendm40 0.1)' % IDN)
        return 1
    print('*IDN? %s' % IDN.decode(), flush=True)
    counts = {c: 0 for c in names}
    t0 = time.monotonic()
    i = 0
    try:
        for i in range(a.iters):
            c = rnd.choices(names, weights)[0]
            t.note('--- %d %s' % (i, c))
            getattr(st, 'c_' + c)()
            counts[c] += 1
            if i % 100 == 99:
                print('%6d  %.0f s  %s' % (i + 1, time.monotonic() - t0, ' '.join('%s=%d' % kv for kv in counts.items())), flush=True)
    except (Hang, IOError) as e:
        print('FAIL at iteration %d (seed %d): %s' % (i, seed, e))
        print('history:\n  ' + '\n  '.join(t.log))
        if a.monitor:
            print(monitor_dump(a.monitor))
        return 1
    finally:
        try:
            st.set_emul(False) if not isinstance(sys.exc_info()[1], Hang) else None
        except Exception:
            pass
    print('PASS %d iterations in %.0f s (seed %d): %s' % (a.iters, time.monotonic() - t0, seed,
                                                      ' '.join('%s=%d' % kv for kv in counts.items())))
    return 0


if __name__ == '__main__':
    sys.exit(main())
