#!/usr/bin/env python3
"""opendm40 scope waveform readout (src/scope_wav.rs: WAV:POINts / FORMat /
PREamble? / DATA?, DIGitize, TRIGger:STATus?, CHANnel1:OFFSet, ACQ:SRATe?)
in the emulator, against the scope input model's sine.

  cd firmware && ./build.sh
  emu/wavtest.py --app firmware/target/native/thumbv7em-none-eabihf/release/opendm40

Checks (USBTMC over USB/IP, Bluetooth over --bt-port):
  * the default WAV:DATA? is unchanged: #3400 + 400 bytes, joinable with
    *OPC?; the 10-field preamble (format 0, type 0, 400 points) scales it
    to the input sine;
  * WAV:POIN MAX while running: an empty block "#10" + "Settings conflict";
  * DIGitize / SINGle end in TRIG:STAT? STOP; the frozen record read as
    WORD (streamed over several REQUEST_DEV_DEP_MSG_IN rounds of 4096 bytes
    and in one round of 64 KiB: the same bytes), BYTE (= WORD >> 2) and
    WAV:POIN <n> (decimated: = every stride-th WORD sample); scaled with
    the preamble it is the input sine (frequency, Vpp, mean), the trigger
    point (t = 0) on the level with the right slope, x increment = 1 /
    ACQ:SRAT?; reading it twice gives the same bytes;
  * CHAN1:OFFSet moves the trace (pos) but not the volts the preamble
    gives;
  * the main loop keeps running during a streamed read (DBG:DIAG? loop
    maximum), an abandoned stream (a new command after the first round)
    releases the scope: RUN runs again;
  * Bluetooth: the full WORD record is refused ("Too much data", "#10"),
    WAV:POIN 2000 BYTE and the default 400 bytes come.
The comparator trigger (hwtrig.rs) is switched off (DBG:HWTRIG:ENABLE 0,
the software trigger path) unless --hwtrig: in the emulator the comparator
path sometimes never completes a frame after a (re)start (TRIG:STAT? WAIT
for good, also on 5af9a5c without these changes; 2026-10-23).
Exit status 0 = all passed.
"""
import argparse
import math
import os
import socket
import struct
import subprocess
import sys
import time

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
ap.add_argument('--app', required=True, help='opendm40 native ELF / image')
ap.add_argument('--port', type=int, default=24140, help='USB/IP port (default 24140)')
ap.add_argument('--bt-port', type=int, default=24141, help='BT passthrough port (default 24141)')
ap.add_argument('--hz', type=float, default=1000.0, help='input sine frequency')
ap.add_argument('--vpp', type=float, default=1.0, help='input sine Vpp')
ap.add_argument('--mips', default='200')
ap.add_argument('--hwtrig', action='store_true', help='keep the comparator trigger on (default: off, see below)')
A = ap.parse_args()
os.environ['USBIP_PORT'] = str(A.port)
sys.path.insert(0, HERE)
import usbip_client as uc  # noqa: E402

FAIL = []


def check(ok, what):
    print('  %s %s' % ('ok  ' if ok else 'FAIL', what), flush=True)
    if not ok:
        FAIL.append(what)


for p in (A.port, A.bt_port):
    if uc.port_busy(p):
        sys.exit('wavtest: port %d is in use (another emulator?)' % p)
tag = 'wav-%d' % A.port
cmd = [sys.executable, os.path.join(HERE, 'run.py'), 'fwrs', '--mips', A.mips, '--app', A.app,
       '--usbip', '0.01', '--usbip-port', str(A.port), '--bt-port', str(A.bt_port), '--run-for', '3000',
       '--name', tag, '--log', os.path.join(HERE, 'out', tag + '.log'),
       '--scope', 'sine,%g,%g' % (A.hz, A.vpp)]
emu = subprocess.Popen(cmd, cwd=ROOT, stdout=subprocess.DEVNULL, stderr=subprocess.STDOUT, start_new_session=True)


def block(b):
    """#<d><len><data>'\n' (USBTMC: the block, then the response terminator)
    -> data (asserts the length and the newline)."""
    assert b[:1] == b'#', b[:16]
    d = int(b[1:2])
    n = int(b[2:2 + d]) if d else 0
    data = b[2 + d:]
    assert len(data) == n + 1 and data[-1:] == b'\n', (len(data), n, data[-4:])
    return data[:n]


def pre(t):
    f = t.q('WAV:PRE?').split(',')
    assert len(f) == 10, f
    return dict(fmt=int(f[0]), type=int(f[1]), n=int(f[2]), count=int(f[3]), xinc=float(f[4]), xorig=float(f[5]),
                xref=float(f[6]), yinc=float(f[7]), yorig=float(f[8]), yref=float(f[9]))


def volts(p, data):
    if p['fmt'] == 1:
        codes = struct.unpack('<%dH' % (len(data) // 2), data)
    else:
        codes = data
    return [(c - p['yref']) * p['yinc'] + p['yorig'] for c in codes], codes


def sine_fit(v, xinc):
    """(frequency from rising mid crossings, Vpp, mean)."""
    lo, hi = min(v), max(v)
    mid = (lo + hi) / 2
    hyst = (hi - lo) * 0.1
    ups, armed = [], False
    for i, x in enumerate(v):
        if x < mid - hyst:
            armed = True
        elif armed and x > mid:
            # interpolate between i-1 and i
            a, b = v[i - 1], x
            ups.append(i - 1 + ((mid - a) / (b - a) if b != a else 0))
            armed = False
    f = (len(ups) - 1) / ((ups[-1] - ups[0]) * xinc) if len(ups) >= 2 else 0.0
    return f, hi - lo, sum(v) / len(v)


def loop_max(t):
    """Main-loop pass maximum (us) of the last full 1 s window (DBG:DIAG?;
    None in a build without --features dbg-diag: refused, an empty reply)."""
    r = t.qraw('DBG:DIAG?')
    if not r.strip():
        return None
    d = block(r)
    return d[97] | d[98] << 8


class T:
    def __init__(self, u):
        self.u = u

    def w(self, line):
        self.u.write(line)

    def qraw(self, line, maxlen=4096, timeout=20, rounds=None):
        self.u.write(line)
        return self.u.read(maxlen=maxlen, timeout=timeout, rounds=rounds)

    def q(self, line):
        return self.qraw(line).decode(errors='replace').strip()

    def err(self):
        return self.q('SYST:ERR?')

    def vtime(self):
        return int(self.q('DBG:PING?').split(',')[0]) / 1000.0

    def wait_stop(self, s=10, wall=180):
        """TRIG:STAT? STOP within `s` s of device time (the emulator may
        run far below real time on a loaded host)."""
        t0, v0 = time.time(), self.vtime()
        seen = set()
        while time.time() - t0 < wall and self.vtime() - v0 < s:
            st = self.q('TRIG:STAT?')
            if st == 'STOP':
                return True
            seen.add(st)
            time.sleep(0.2)
        print('    (TRIG:STAT? seen: %s)' % ','.join(sorted(seen)), flush=True)
        return False


def connect():
    for _ in range(150):
        try:
            return T(uc.Usbtmc(uc.Dev('1-0')))
        except Exception:
            time.sleep(2)
    sys.exit('wavtest: no USB/IP device on port %d' % A.port)


def bt_checks(t):
    try:
        s = socket.create_connection(('127.0.0.1', A.bt_port), timeout=5)
    except OSError as e:
        check(False, 'BT passthrough (%s)' % e)
        return
    s.settimeout(30)
    buf = b''

    def line(cmd, reply=True):
        nonlocal buf
        s.sendall(cmd.encode() + b'\n')
        if not reply:
            return None
        while b'\n' not in buf:
            d = s.recv(65536)
            if not d:
                return None
            buf += d
        l, _, buf = buf.partition(b'\n')
        return l

    def blk(cmd):
        nonlocal buf
        s.sendall(cmd.encode() + b'\n')
        while len(buf) < 2:
            buf += s.recv(65536)
        d = int(buf[1:2])
        while len(buf) < 2 + d:
            buf += s.recv(65536)
        n = int(buf[2:2 + d]) if d else 0
        while len(buf) < 2 + d + n + 1:
            buf += s.recv(65536)
        data, rest = buf[2 + d:2 + d + n], buf[2 + d + n:]
        assert rest[:1] == b'\n', rest[:4]
        buf = rest[1:]
        return data

    try:
        # A line arriving before the firmware saw the link come up (PC14) is
        # dropped: retry the first query.
        s.settimeout(10)
        idn = None
        for _ in range(6):
            try:
                idn = line('*IDN?')
                break
            except socket.timeout:
                buf = b''
        s.settimeout(30)
        check(bool(idn) and idn.startswith(b'Alientek'), 'BT: *IDN? %r' % idn)

        # A USB host that stops reading a streamed record and sends nothing
        # more (its process died): the firmware drops the stream after 2 s
        # without progress and the scope runs again (RUN over BT: no USBTMC
        # traffic that would end the stream by itself).
        t.w('WAV:POIN MAX')
        t.w('WAV:FORM WORD')
        t.w('DIG')
        check(t.wait_stop(), 'stalled stream: DIG ends in STOP')
        t.u.write('WAV:DATA?')
        part, eom = t.u._read1(4096, 20)
        check(not eom, 'stalled stream: first round only (%d bytes)' % len(part))
        line('RUN', False)
        held = line('TRIG:STAT?')
        time.sleep(20)  # >= 2 s of device time even at 0.1x
        line('RUN', False)
        time.sleep(3)
        st = line('TRIG:STAT?')
        check(held == b'STOP' and st != b'STOP', 'stalled stream: RUN held while streaming (%s), runs after the timeout (%s)'
              % (held.decode(), st.decode()))
        # A full record again (a STOP cuts the acquisition in flight short).
        t.w('DIG')
        check(t.wait_stop(), 'DIG after the stalled stream ends in STOP')
        line('*CLS', False)
        line('WAV:POIN MAX', False)
        line('WAV:FORM WORD', False)
        d = blk('WAV:DATA?')
        e = line('SYST:ERR?').decode()
        check(d == b'' and 'Too much data' in e, 'BT: full WORD record refused (%d bytes, %s)' % (len(d), e))
        line('WAV:POIN 2000', False)
        line('WAV:FORM BYTE', False)
        n = int(line('WAV:POIN?'))
        d = blk('WAV:DATA?')
        e = line('SYST:ERR?').decode()
        check(0 < len(d) == n <= 2000 and e.startswith('0,'), 'BT: WAV:POIN 2000 BYTE: %d bytes (%s)' % (len(d), e))
        line('WAV:POIN SCR', False)
        d = blk('WAV:DATA?')
        check(len(d) == 400, 'BT: default WAV:DATA? 400 bytes (%d)' % len(d))
    except Exception as x:
        check(False, 'BT: %r' % x)
    finally:
        s.close()


try:
    t = connect()
    print('*IDN?', t.q('*IDN?'), flush=True)
    vdiv = 0.2 if A.vpp <= 1.2 else 1.0
    if not A.hwtrig:
        t.w('DBG:HWTRIG:ENABLE 0')
    for c in ('*CLS', 'CHAN1:COUP DC', 'CHAN1:SCAL %g' % vdiv, 'CHAN1:OFFS 0', 'TIM:SCAL 1e-3', 'TRIG:SWE AUTO',
              'TRIG:SLOP POS', 'TRIG:LEV 0', 'WAV:POIN SCR', 'WAV:FORM BYTE', 'RUN'):
        t.w(c)
    e = t.err()
    check(e.startswith('0,'), 'set-up accepted (%s)' % e)

    # --- the default form, unchanged (after the first frames came)
    t0 = time.time()
    while True:
        d = block(t.qraw('WAV:DATA?'))
        if max(d) - min(d) > 20 or time.time() - t0 > 60:
            break
        time.sleep(1)
    check(len(d) == 400, 'default WAV:DATA?: #3400 + 400 bytes')
    p = pre(t)
    check((p['fmt'], p['type'], p['n'], p['count']) == (0, 0, 400, 1), 'default preamble: BYTE, screen, 400 points (%s)' % p)
    check(abs(p['xinc'] - 1e-3 * 10 / 400) < 1e-9, 'screen x increment = 10 div / 400 (%g)' % p['xinc'])
    v, _ = volts(p, d)
    f, vpp, mean = sine_fit(v, p['xinc'])
    check(abs(f - A.hz) < A.hz * 0.03 and abs(vpp - A.vpp) < A.vpp * 0.2,
          'screen columns scaled: %.1f Hz, %.3f Vpp, mean %.3f V' % (f, vpp, mean))
    j = t.qraw('*OPC?;WAV:DATA?')
    check(j[:7] == b'1;#3400' and len(j) == 408 and j[-1:] == b'\n', 'default WAV:DATA? still joins (*OPC?;WAV:DATA?: %d bytes)' % len(j))
    t.w('WAV:FORM WORD')
    pw = pre(t)
    dw = block(t.qraw('WAV:DATA?'))
    vw, cw = volts(pw, dw)
    fw, vppw, _ = sine_fit(vw, pw['xinc'])
    check(pw['fmt'] == 1 and pw['type'] == 0 and len(dw) == 800 and max(cw) <= 1023 and abs(fw - A.hz) < A.hz * 0.03
          and abs(vppw - A.vpp) < A.vpp * 0.2, 'SCReen + WORD: 800-byte block, %.1f Hz, %.3f Vpp' % (fw, vppw))
    j = t.qraw('*OPC?;WAV:DATA?')
    e = t.err()
    check(not j.startswith(b'1;#') and 'Query error' in e, 'SCReen + WORD is not joinable (%r..., %s)' % (j[:8], e))
    t.w('WAV:FORM BYTE')

    # --- record while running: refused
    t.w('WAV:POIN MAX')
    d = t.qraw('WAV:DATA?')
    e = t.err()
    check(d == b'#10\n' and 'Settings conflict' in e, 'record while running: "#10" + %s' % e)

    # --- DIGitize: one acquisition, then STOP
    t.w('DIG')
    check(t.wait_stop(), 'DIGitize ends in TRIG:STAT? STOP')
    sw = t.q('TRIG:SWE?')
    check(sw == 'AUTO', 'DIGitize keeps the sweep (TRIG:SWE? %s)' % sw)
    t.w('WAV:FORM WORD')
    n = int(t.q('WAV:POIN?'))
    p = pre(t)
    check(p['fmt'] == 1 and p['type'] == 2 and p['n'] == n and n > 400, 'record preamble: WORD, RAW, %d points (%s)' % (n, p))
    sr = float(t.q('ACQ:SRAT?'))
    check(abs(p['xinc'] * sr - 1) < 1e-4, 'x increment = 1 / ACQ:SRAT? (%g S/s)' % sr)
    # #158: a stopped zoom (another timebase shown) keeps the record's rate.
    tb0 = t.q('TIM:SCAL?')
    t.w('TIM:SCAL %g' % (float(tb0) / 10))
    sz = float(t.q('ACQ:SRAT?'))
    pz = pre(t)
    check(sz == sr and pz['xinc'] == p['xinc'], 'stopped zoom (TIM:SCAL %g): ACQ:SRAT? %g = the record\'s %g, record x increment kept' % (float(tb0) / 10, sz, sr))
    # The screen columns (WAV:POIN SCR, type 0) are the zoomed view: their x
    # increment follows the shown timebase (10 div / 400 columns).
    t.w('WAV:POIN SCR')
    ps = pre(t)
    check(ps['type'] == 0 and abs(ps['xinc'] - float(tb0) / 10 * 10 / 400) < 1e-12,
          'stopped zoom: screen preamble x increment %g = the shown timebase / 40' % ps['xinc'])
    t.w('WAV:POIN MAX')
    t.w('TIM:SCAL %s' % tb0)
    check(t.q('TRIG:STAT?') == 'STOP', 'the zoom keeps the scope stopped')
    lms = [loop_max(t) for _ in range(2) if time.sleep(1.1) is None]
    lm0 = None if None in lms else max(lms)
    rounds = []
    t0 = time.time()
    raw = t.qraw('WAV:DATA?', maxlen=4096, rounds=rounds)
    dt = time.time() - t0
    lm1 = loop_max(t)
    words = block(raw)
    check(len(words) == 2 * n, 'WORD record: %d bytes in %d transfers of <= 4096 (%.2f s wall)' % (len(words), len(rounds), dt))
    check(len(rounds) > 1 and all(r <= 4096 for r in rounds), 'streamed over %d REQUEST rounds' % len(rounds))
    if lm0 is None or lm1 is None:
        print('  skip the main-loop check (DBG:DIAG? not in this build: --features dbg-diag)', flush=True)
        t.w('*CLS')
    else:
        check(lm1 < lm0 + 4000, 'main loop kept running: pass max %d us during the read (%d us before)' % (lm1, lm0))
    raw2 = t.qraw('WAV:DATA?', maxlen=65536)
    check(raw2 == raw, 'one 64 KiB round: the same bytes (frozen record)')
    v, codes = volts(p, words)
    check(max(codes) <= 1023, '10-bit codes (max %d)' % max(codes))
    f, vpp, mean = sine_fit(v, p['xinc'])
    check(abs(f - A.hz) < A.hz * 0.02, 'record frequency %.2f Hz (input %g)' % (f, A.hz))
    check(abs(vpp - A.vpp) < A.vpp * 0.15, 'record Vpp %.3f V (input %g)' % (vpp, A.vpp))
    check(abs(mean) < 0.05 * A.vpp + 0.02, 'record mean %.3f V' % mean)
    i0 = round((0 - p['xorig']) / p['xinc'] + p['xref'])
    if 2 <= i0 < len(v) - 2:
        slope = v[i0 + 2] - v[i0 - 2]
        check(abs(v[i0]) < 0.08 * A.vpp and slope > 0, 'trigger point (t = 0, sample %d of %d): %.3f V, rising' % (i0, len(v), v[i0]))
    else:
        check(False, 'trigger point inside the record (sample %d of %d)' % (i0, len(v)))
    wcodes = codes

    t.w('WAV:FORM BYTE')
    pb = pre(t)
    b = block(t.qraw('WAV:DATA?'))
    check(len(b) == n and list(b) == [c >> 2 for c in wcodes], 'BYTE record = WORD >> 2 (%d bytes)' % len(b))
    check(abs(pb['yinc'] - 4 * p['yinc']) < 1e-5 * abs(p['yinc']), 'BYTE y increment = 4 x WORD (%g, %g)' % (pb['yinc'], p['yinc']))
    # #130: a byte's volts are the middle of its four codes, not the lowest.
    vb, _ = volts(pb, b)
    bias = (sum(vb) - sum(v)) / len(v) / p['yinc']
    check(abs(bias) < 0.5, 'BYTE mean = WORD mean (%+.2f codes; 1.5 codes low before #130)' % bias)

    t.w('WAV:POIN 1000')
    t.w('WAV:FORM WORD')
    pd = pre(t)
    dd = block(t.qraw('WAV:DATA?'))
    stride = round(pd['xinc'] / p['xinc'])
    dc = struct.unpack('<%dH' % (len(dd) // 2), dd)
    check(pd['n'] <= 1000 and len(dc) == pd['n'] and list(dc) == list(wcodes[::stride]),
          'WAV:POIN 1000: %d points, every %d-th sample' % (pd['n'], stride))
    check(abs(pd['xorig'] - p['xorig']) < 1e-12, 'decimated x origin unchanged')

    # --- an abandoned stream releases the scope
    t.w('WAV:POIN MAX')
    t.u.write('WAV:DATA?')
    part, eom = t.u._read1(4096, 20)
    check(not eom and part[:1] == b'#', 'first round of a streamed read (%d bytes, EOM %d)' % (len(part), eom))
    idn = t.q('*IDN?')
    check(idn.startswith('Alientek'), 'a new command after an abandoned stream answers (%s)' % idn)

    # --- SINGle: the sweep becomes Single, one triggered record, STOP
    t.w('SING')
    check(t.wait_stop(), 'SINGle ends in TRIG:STAT? STOP')
    t.w('WAV:POIN MAX')
    p2 = pre(t)
    v2, _ = volts(p2, block(t.qraw('WAV:DATA?', maxlen=65536)))
    f2, vpp2, mean2 = sine_fit(v2, p2['xinc'])
    i0 = round(-p2['xorig'] / p2['xinc'])
    check(abs(f2 - A.hz) < A.hz * 0.02 and 2 <= i0 < len(v2) - 2 and abs(v2[i0]) < 0.08 * A.vpp,
          'SINGle record: %.2f Hz, %.3f Vpp, trigger sample %d' % (f2, vpp2, i0))
    t.w('TRIG:SWE AUTO')

    # --- CHAN1:OFFSet: the position (the offset PWM shifts the ADC codes on
    # the device; the emulator's scope feeder does not model that shift, so
    # only the preamble's y origin moving by -offset is checked here)
    y0 = p2['yorig']
    t.w('CHAN1:OFFS 0.2')
    o = float(t.q('CHAN1:OFFS?'))
    check(abs(o - 0.2) < 0.01, 'CHAN1:OFFS? %.4f V' % o)
    t.w('DIG')
    check(t.wait_stop(), 'DIGitize with the offset ends in STOP')
    p3 = pre(t)
    check(abs((p3['yorig'] - y0) + o) < 0.002, 'y origin moved by -offset (%.4f -> %.4f V)' % (y0, p3['yorig']))
    t.w('CHAN1:OFFS 0')

    # --- RUN runs again after the reads
    t.w('RUN')
    time.sleep(1.5)
    st = t.q('TRIG:STAT?')
    check(st in ('AUTO', 'TD', 'WAIT'), 'RUN after the reads: TRIG:STAT? %s' % st)
    e = t.err()
    check(e.startswith('0,'), 'no error left (%s)' % e)

    t.w('STOP')
    bt_checks(t)
finally:
    try:
        os.killpg(emu.pid, 15)
    except Exception:
        pass
    emu.wait()

print('FAIL: %d checks' % len(FAIL) if FAIL else 'PASS')
sys.exit(1 if FAIL else 0)
