#!/usr/bin/env python3
"""Comparator-trigger record length and content (src/hwtrig.rs, scope.rs
hw_poll) in the emulator: DIGitize and SINGle at 1 us .. 200 ms/div with
the comparator on (the default), Normal sweep; other trigger positions
(hpos): firmware/tools/trig-test.

  cd firmware && ./build.sh
  emu/hwtrigrectest.py --app firmware/target/native/thumbv7em-none-eabihf/release/opendm40

Two emulator runs (one input sine each: 20 kHz for 1 us .. 1 ms/div,
100 Hz for 10 .. 200 ms/div). Per timebase and mode the record (WAV:POIN
MAX, WORD) must be
  * the software linear record's length: lin_len = clamp(fs / 10, 26 K,
    CAP) (45 056 from 5 ms/div down, 26 624 at 10 ms/div and slower);
  * triggered by the comparator (DBG:HWTRIG:STAT? hits grow, misses rare)
    at the edge position of the stock layout: pre_for = screen * hpos / 400
    + 416 samples from the record start, refined within the 800-sample
    window (here pre + 16 .. pre + 816);
  * the input: frequency, Vpp; the trigger sample on the level (0 V) with a
    rising slope.
--sw runs the same steps with the software trigger (DBG:HWTRIG:ENABLE 0)
and only prints the lengths (stream records at <= 1.25 MS/s are not
lin_len long). Exit status 0 = all passed.
"""
import argparse
import os
import struct
import subprocess
import sys
import time

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
ap.add_argument('--app', required=True, help='opendm40 native ELF / image')
ap.add_argument('--port', type=int, default=24170, help='USB/IP port (default 24170)')
ap.add_argument('--cap', type=int, default=45056, help='scope::CAP of the build (default 45056)')
ap.add_argument('--vpp', type=float, default=1.0, help='input sine Vpp')
ap.add_argument('--mips', default='200')
ap.add_argument('--sw', action='store_true', help='software trigger, lengths only')
ap.add_argument('--only', choices=('fast', 'slow'), help='one of the two runs')
A = ap.parse_args()
os.environ['USBIP_PORT'] = str(A.port)
sys.path.insert(0, HERE)
import usbip_client as uc  # noqa: E402

FAIL = []
GROUPS = [('fast', 20000.0, (1e-6, 1e-5, 1e-4, 1e-3)), ('slow', 100.0, (1e-2, 2e-2, 2e-1))]


def check(ok, what):
    print('  %s %s' % ('ok  ' if ok else 'FAIL', what), flush=True)
    if not ok:
        FAIL.append(what)


def block(b):
    assert b[:1] == b'#', b[:16]
    d = int(b[1:2])
    n = int(b[2:2 + d]) if d else 0
    data = b[2 + d:]
    assert len(data) == n + 1 and data[-1:] == b'\n', (len(data), n, data[-4:])
    return data[:n]


class T:
    def __init__(self, u):
        self.u = u

    def w(self, line):
        self.u.write(line)

    def qraw(self, line, maxlen=4096, timeout=30):
        self.u.write(line)
        return self.u.read(maxlen=maxlen, timeout=timeout)

    def q(self, line):
        return self.qraw(line).decode(errors='replace').strip()

    def vtime(self):
        return int(self.q('DBG:PING?').split(',')[0]) / 1000.0

    def wait_stop(self, s=15, wall=300):
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

    def wait_v(self, s):
        v0 = self.vtime()
        while self.vtime() - v0 < s:
            time.sleep(0.2)


def connect():
    for _ in range(150):
        try:
            return T(uc.Usbtmc(uc.Dev('1-0')))
        except Exception:
            time.sleep(2)
    sys.exit('hwtrigrectest: no USB/IP device on port %d' % A.port)


def stat(t):
    f = t.q('DBG:HWTRIG:STAT?').split(',')
    return int(f[0]), int(f[1])


def rec(t):
    t.w('WAV:POIN MAX')
    t.w('WAV:FORM WORD')
    f = t.q('WAV:PRE?').split(',')
    n, xinc, xorig = int(f[2]), float(f[4]), float(f[5])
    yinc, yorig, yref = float(f[7]), float(f[8]), float(f[9])
    d = block(t.qraw('WAV:DATA?', maxlen=1 << 18, timeout=120))
    codes = struct.unpack('<%dH' % (len(d) // 2), d)
    v = [(c - yref) * yinc + yorig for c in codes]
    return n, xinc, round(-xorig / xinc) if xinc else -1, v


def fit(v, xinc):
    lo, hi = min(v), max(v)
    mid = (lo + hi) / 2
    hyst = (hi - lo) * 0.1
    ups, armed = [], False
    for i, x in enumerate(v):
        if x < mid - hyst:
            armed = True
        elif armed and x > mid:
            ups.append(i)
            armed = False
    f = (len(ups) - 1) / ((ups[-1] - ups[0]) * xinc) if len(ups) >= 2 else 0.0
    return f, hi - lo


def step(t, hz, tb, mode):
    s0 = stat(t)
    t.w('RUN')
    t.wait_v(0.3 + 25 * tb)
    t.w(mode)
    if not t.wait_stop(15 + 60 * tb):
        check(False, '%s %g s/div: TRIG:STAT? STOP' % (mode, tb))
        return None
    sr = float(t.q('ACQ:SRAT?'))
    n, xinc, ti, v = rec(t)
    s1 = stat(t)
    tag = '%s %g s/div (%g S/s)' % (mode, tb, sr)
    if A.sw:
        print('  %s: %d points, trigger index %d' % (tag, n, ti), flush=True)
        return n
    exp = min(max(int(sr) // 10, 26 * 1024), A.cap)
    check(n == exp == len(v), '%s: %d points (expected lin_len %d), %d samples read' % (tag, n, exp, len(v)))
    # The counts cover the running frames before DIG / SING too: at 50 MS/s
    # an occasional miss there (1..2 per ~100 hits, emulator) is not this
    # record's; the record's own trigger is checked against the window.
    check(s1[0] > s0[0] and s1[1] - s0[1] <= max(2, (s1[0] - s0[0]) // 20),
          '%s: comparator hits %d -> %d, misses %d -> %d' % (tag, s0[0], s1[0], s0[1], s1[1]))
    screen = round(10 * tb * sr)
    pre = screen * 200 // 400
    # The comparator stop puts the edge at pre_for, refined within the
    # 800-sample window from pre + 16 on (hw_poll Done).
    check(pre + 16 <= ti < pre + 816, '%s: trigger at %d, window %d .. %d (pre_for %d)'
          % (tag, ti, pre + 16, pre + 816, pre + 416))
    f, vpp = fit(v, xinc)
    check(abs(f - hz) < hz * 0.03 and abs(vpp - A.vpp) < A.vpp * 0.2, '%s: %.1f Hz, %.3f Vpp' % (tag, f, vpp))
    if 0 < ti < len(v) - 1:
        k = max(2, int(sr / hz / 20))
        k = min(k, ti, len(v) - 1 - ti)
        a = sum(v[ti - k:ti]) / k
        b = sum(v[ti:ti + k]) / k
        sl = abs(v[ti] - v[ti - 1])
        check(a < b and abs(v[ti]) <= 0.05 * A.vpp + sl,
              '%s: trigger sample %.3f V on the 0 V level, rising (%.3f -> %.3f V)' % (tag, v[ti], a, b))
    return n


def run(name, hz, tbs):
    print('== %s run: sine %g Hz %g Vpp' % (name, hz, A.vpp), flush=True)
    if uc.port_busy(A.port):
        sys.exit('hwtrigrectest: port %d is in use (another emulator?)' % A.port)
    tag = 'hwtrigrec-%d' % A.port
    cmd = [sys.executable, os.path.join(HERE, 'run.py'), 'fwrs', '--mips', A.mips, '--app', A.app,
           '--usbip', '0.01', '--usbip-port', str(A.port), '--run-for', '3000',
           '--name', tag, '--log', os.path.join(HERE, 'out', tag + '.log'),
           '--scope', 'sine,%g,%g' % (hz, A.vpp)]
    emu = subprocess.Popen(cmd, cwd=ROOT, stdout=subprocess.DEVNULL, stderr=subprocess.STDOUT, start_new_session=True)
    lens = {}
    try:
        t = connect()
        print('*IDN?', t.q('*IDN?'), flush=True)
        if A.sw:
            t.w('DBG:HWTRIG:ENABLE 0')
        for c in ('*CLS', 'CHAN1:COUP DC', 'CHAN1:SCAL 0.2', 'CHAN1:OFFS 0', 'TRIG:SWE NORM', 'TRIG:SLOP POS',
                  'TRIG:LEV 0', 'TRIG:SOUR CHAN1'):
            t.w(c)
        e = t.q('SYST:ERR?')
        check(e.startswith('0,'), 'set-up accepted (%s)' % e)
        hw = t.q('DBG:HWTRIG?').split(',')
        check(A.sw or hw[4:6] == ['1', '1'], 'comparator enabled and available (DBG:HWTRIG? %s)' % ','.join(hw[4:6]))
        for tb in tbs:
            t.w('TIM:SCAL %g' % tb)
            for mode in ('DIG', 'SING'):
                lens[(tb, mode)] = step(t, hz, tb, mode)
            t.w('TRIG:SWE NORM')
        e = t.q('SYST:ERR?')
        check(e.startswith('0,'), 'no SCPI error (%s)' % e)
    finally:
        try:
            os.killpg(emu.pid, 15)
        except ProcessLookupError:
            pass
        emu.wait(timeout=30)
    return lens


allv = {}
for name, hz, tbs in GROUPS:
    if A.only and A.only != name:
        continue
    allv.update(run(name, hz, tbs))
print('lengths:', ', '.join('%s %g: %s' % (m, tb, n) for (tb, m), n in allv.items()))
print('FAILED: %d' % len(FAIL) if FAIL else 'all passed')
sys.exit(1 if FAIL else 0)
