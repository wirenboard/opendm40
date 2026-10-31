#!/usr/bin/env python3
"""STOP always leaves a record: RUN / STOP in a loop at several timebases,
the record read after each STOP (WAV:POIN MAX, WORD) against the input.

  emu/stoptest.py --app firmware/target/native/thumbv7em-none-eabihf/release/opendm40

Scope page, a 1 kHz 0..2 V square on CH1 (0.5 V/div, DC, Auto, edge rising,
the software trigger unless --hwtrig). Two passes: the level at 3 V (above
the square: no record triggers, Auto shows one every 100 ms and the others
are not shown) and at 1 V (triggered). Per pass and timebase (10 us,
100 us, 200 us, 1 ms/div) --n rounds of RUN, a wait that differs per
round, STOP; then WAV:PRE? must give points > 0 and WAV:DATA? that many
words, scaled with the preamble the input square (its two levels; with
three periods or more in the record, 1 kHz). Before the fix about 1 STOP
in 8 at 100 us/div on the device gave 0 points: a STOP between a record
poll() did not show (no edge in its 0.8 ms trigger window, Auto before
its timeout) and the next one's start.
Exit status 0 = all passed.
"""
import argparse
import os
import shutil
import struct
import subprocess
import sys
import time

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
ap.add_argument('--app', required=True, help='opendm40 native ELF')
ap.add_argument('--port', type=int, default=None, help='USB/IP port (default: run.free_port)')
ap.add_argument('--n', type=int, default=12, help='RUN / STOP rounds per timebase (default 12)')
ap.add_argument('--tb', default='1e-5,1e-4,2e-4,1e-3', help='timebases, s/div (default 1e-5,1e-4,2e-4,1e-3)')
ap.add_argument('--hwtrig', action='store_true', help='keep the comparator trigger on')
ap.add_argument('--mips', default='200')
A = ap.parse_args()
sys.path.insert(0, HERE)
import run  # noqa: E402
if A.port is None:
    A.port = run.free_port()
os.environ['USBIP_PORT'] = str(A.port)
import usbip_client as uc  # noqa: E402

FAIL = []
HZ, LO, HI = 1000.0, 0.0, 2.0


def check(ok, what):
    if not ok:
        print('  FAIL %s' % what, flush=True)
        FAIL.append(what)
    return ok


OUT = os.path.join(HERE, 'out')
os.makedirs(OUT, exist_ok=True)
tag = 'stop-%d-%d' % (A.port, os.getpid())
log = os.path.join(OUT, '%s.log' % tag)
disk = os.path.join(OUT, tag + '.disk.bin')
shutil.copy(os.path.join(ROOT, 'stock/re/dumps/spi_dumps/dm40_msc_full_2026-10-13.bin'), disk)
cmd = [sys.executable, os.path.join(HERE, 'run.py'), 'fwrs', '--mips', A.mips, '--app', A.app,
       '--usbip', '0.01', '--usbip-port', str(A.port), '--run-for', '3000', '--name', tag, '--log', log,
       '--disk', disk, '--scope', 'square,%g,%g,%g' % (HZ, HI - LO, (HI + LO) / 2)]
emu = subprocess.Popen(cmd, cwd=ROOT, stdout=subprocess.DEVNULL, stderr=subprocess.STDOUT, start_new_session=True)

try:
    t = None
    for _ in range(150):
        try:
            t = uc.Usbtmc(uc.Dev('1-0'))
            break
        except Exception:
            time.sleep(2)
    if t is None:
        sys.exit('stoptest: no USB/IP device on port %d' % A.port)

    def q(line, timeout=60, raw=False, maxlen=65536):
        t.write(line)
        if '?' not in line:
            return None
        b = t.read(maxlen=maxlen, timeout=timeout)
        return b if raw else b.decode(errors='replace').strip()

    def up():
        return int(q('DBG:PING?').split(',')[0])

    def wait_ms(ms):
        t0 = up()
        while up() - t0 < ms:
            time.sleep(0.05)

    def block(b):
        assert b[:1] == b'#', b[:16]
        nd = int(b[1:2])
        n = int(b[2:2 + nd]) if nd else 0
        return b[2 + nd:2 + nd + n], n

    print('*IDN?', q('*IDN?'), flush=True)
    wait_ms(2500)
    q('DBG:KEY 0')  # main menu
    wait_ms(500)
    q('DBG:TAP 341,107')  # the Oscilloscope tile
    wait_ms(1500)
    v = q('DBG:VIEW?')
    check(v.startswith('Oscilloscope'), 'on the scope page (%s)' % v)
    if not A.hwtrig:
        q('DBG:HWTRIG:ENABLE 0')
    for c in ('CHAN1:COUP DC', 'CHAN1:SCAL 0.5', 'CHAN1:OFFS 0', 'TRIG:SOUR CHAN1', 'TRIG:LEV 1', 'TRIG:SLOP POS',
              'TRIG:SWE AUTO', 'WAV:FORM WORD'):
        q(c)
    print('  setup', q('SYST:ERR?'), flush=True)
    total = 0
    passes = [(lev, float(tb)) for lev in (3, 1) for tb in A.tb.split(',')]
    for lev, tb in passes:
        q('TRIG:LEV %g' % lev)
        pts = []
        for i in range(A.n):
            q('TIM:SCAL %g' % tb)
            q('RUN')
            # A different phase against the acquisition each round.
            wait_ms(300 + 37 * i)
            q('STOP')
            st = q('TRIG:STAT?')
            q('WAV:POIN MAX')
            f = q('WAV:PRE?').split(',')
            n = int(f[2])
            pts.append(n)
            what = 'level %g V, tb %g, round %d' % (lev, tb, i)
            total += 1
            if not check(st == 'STOP' and n > 0, '%s: STOP -> %s, %d points' % (what, st, n)):
                q('WAV:DATA?', raw=True)
                continue
            xinc, yinc, yorig = float(f[4]), float(f[7]), float(f[8])
            raw = q('WAV:DATA?', raw=True, maxlen=1 << 18, timeout=120)
            d, nb = block(raw)
            if not check(nb == 2 * n and len(d) == nb, '%s: WAV:DATA? %d bytes for %d points' % (what, len(d), n)):
                continue
            vs = [c * yinc + yorig for c in struct.unpack('<%dH' % n, d)]
            lo = sum(1 for x in vs if abs(x - LO) < 0.25)
            hi = sum(1 for x in vs if abs(x - HI) < 0.25)
            span = n * xinc
            # Either level, nearly all samples (the edges are a few samples);
            # both levels once the record spans a period.
            ok = lo + hi >= 0.98 * n and (span < 1.0 / HZ or (lo > 0.3 * n and hi > 0.3 * n))
            check(ok, '%s: the square (%d points over %.3g s: %d low, %d high)' % (what, n, span, lo, hi))
            if span >= 3.0 / HZ:
                ups = [k for k in range(1, n) if vs[k - 1] < (LO + HI) / 2 <= vs[k]]
                fr = (len(ups) - 1) / ((ups[-1] - ups[0]) * xinc) if len(ups) >= 2 else 0.0
                check(abs(fr - HZ) < 0.02 * HZ, '%s: %.1f Hz' % (what, fr))
            err = q('SYST:ERR?')
            check(err.startswith('0'), '%s: SYST:ERR? %s' % (what, err))
        print('  level %g V, tb %g: STOP points %s' % (lev, tb, pts), flush=True)
    print('  %d STOPs' % total, flush=True)
finally:
    try:
        os.killpg(emu.pid, 15)
    except ProcessLookupError:
        pass
    try:
        emu.wait(10)
    except subprocess.TimeoutExpired:
        os.killpg(emu.pid, 9)
    try:
        os.unlink(disk)
    except OSError:
        pass

print('FAIL: %d' % len(FAIL) if FAIL else 'PASS', flush=True)
sys.exit(1 if FAIL else 0)
