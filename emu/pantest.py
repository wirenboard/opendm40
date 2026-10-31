#!/usr/bin/env python3
"""opendm40 stopped scope record: pan by touch and zoom across the long
record, in the emulator.

  cd firmware && ./build.sh
  emu/pantest.py --app firmware/target/native/thumbv7em-none-eabihf/release/opendm40 [--port N]

A 1.6 Hz 2-12 V square on the MCX (period 250 px at 100 ms/div; the low
level off the CH1 zero line), 5 V/div, EDGE 7 V, NORM, DIGitize at 100 ms/div. The
stopped record's length, sample rate and trigger point come from WAV:PRE?
(RAW): 65 520 samples (2.62 s, the trigger mid-record) from the packed
ring, more from the coded ring (src/cring.rs); the drags to the record's
ends and the true-scale zoom are sized from them. The graph is read back over
DBG:LCD? and each column classed as high / low (yellow at the square's top /
bottom row, learnt from the first screen).

Each drag (DBG:DRAG, sideways across the graph) comes right after a short
DBG:LCD? read, so it is released while drawing is held (300 ms after a GRAM
read-back): the re-render of the panned record must wait for the hold, not
be dropped (device 2026-10-26: the screen kept the drag preview, the record
beyond the old screen blank).

  * after every drag both halves of the graph show the square (>= 60 high
    and >= 60 low columns each);
  * a half-screen drag moves the trace by 200 px (the half that stays on
    screen matches the previous screen's other half);
  * a drag back restores the first screen;
  * drags past the record's start stop there (the screen is unchanged by
    the next one), and the first drag back moves at once;
  * zoom while stopped: 200 ms/div (2 s, inside the record) shows the
    square across the graph; 500 ms/div, or 1 / 2 s/div for a longer
    record (a window longer than the record, centred where the pan to the
    end left it), the record at its true scale (2.62 s at 500 ms/div:
    about 210 columns), nothing beyond;
  * V/div while stopped: 10 V/div shows the trace across the graph, 5 V/div
    the square again;
  * WAV:PRE? (RAW) points at least 65 520, the same throughout.

Exit status 0 = all passed.
"""
import argparse
import math
import os
import subprocess
import sys
import time

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
ap.add_argument('--app', required=True, help='opendm40 native ELF')
ap.add_argument('--port', type=int, default=None, help='USB/IP port (default: run.free_port)')
ap.add_argument('--mips', default='200')
ap.add_argument('--out', help='save each screen as PNG in this directory')
A = ap.parse_args()
sys.path.insert(0, HERE)
import run  # noqa: E402
if A.port is None:
    A.port = run.free_port()
os.environ['USBIP_PORT'] = str(A.port)
import usbip_client as uc  # noqa: E402

FAIL = []


def check(ok, what):
    print('  %s %s' % ('ok  ' if ok else 'FAIL', what), flush=True)
    if not ok:
        FAIL.append(what)


OUT = os.path.join(HERE, 'out')
os.makedirs(OUT, exist_ok=True)
tag = 'pantest-%d-%d' % (A.port, os.getpid())
log = os.path.join(OUT, '%s.log' % tag)
cmd = [sys.executable, os.path.join(HERE, 'run.py'), 'fwrs', '--mips', A.mips, '--app', A.app,
       '--usbip', '0.01', '--usbip-port', str(A.port), '--run-for', '3000', '--name', tag, '--log', log,
       '--scope', 'square,1.6,10,7']
emu = subprocess.Popen(cmd, cwd=ROOT, stdout=subprocess.DEVNULL, stderr=subprocess.STDOUT, start_new_session=True)

# Graph columns x0 .. x0 + 398, rows y0 .. y1 (scope page).
X0, Y0, Y1 = 14, 22, 298
W = 399

try:
    t = None
    for _ in range(150):
        try:
            t = uc.Usbtmc(uc.Dev('1-0'))
            break
        except Exception:
            time.sleep(2)
    if t is None:
        sys.exit('pantest: no USB/IP device on port %d' % A.port)

    def q(line, timeout=60, raw=False):
        t.write(line)
        if '?' not in line:
            return None
        r = t.read(maxlen=8192, timeout=timeout)
        return r if raw else r.decode(errors='replace').strip()

    def wait_ms(ms):
        t0 = int(q('DBG:PING?').split(',')[0])
        while int(q('DBG:PING?').split(',')[0]) - t0 < ms:
            time.sleep(0.1)

    def block(b):
        nd = int(b[1:2])
        return b[2 + nd:2 + nd + int(b[2:2 + nd])]

    def screen(name):
        """Yellow pixels of the graph: {column: set of rows}."""
        ys = {}
        rows = 4
        img = []
        for y in range(Y0, Y1, rows):
            n = 480 * rows * 3 // 2 + 1
            d = block(q('DBG:LCD? 0,%d,480,%d,%d' % (y, rows, n), raw=True))
            w = [d[i] | d[i + 1] << 8 for i in range(2, len(d), 2)]  # word 0 = the dummy
            bs = bytearray()
            for v in w:
                bs += bytes([v >> 8, v & 0xFF])
            for i in range(480 * rows):
                r, g, b = bs[3 * i], bs[3 * i + 1], bs[3 * i + 2]
                img.append((r, g, b))
                x = i % 480 - X0
                if 0 <= x < W and r > 200 and g > 200 and b < 80:
                    ys.setdefault(x, set()).add(y + i // 480)
        if A.out:
            from PIL import Image
            im = Image.new('RGB', (480, len(img) // 480))
            im.putdata(img)
            im.save(os.path.join(A.out, name + '.png'))
        return ys

    LV = {}

    def classify(ys):
        """Per column: 'H' (top row), 'L' (bottom row), 'E' (both: an
        edge), '.' (no trace)."""
        s = ''
        for x in range(W):
            r = ys.get(x, set())
            h = any(abs(y - LV['hi']) <= 2 for y in r)
            lo = any(abs(y - LV['lo']) <= 2 for y in r)
            s += 'E' if h and lo else 'H' if h else 'L' if lo else '.'
        return s

    def full(s, what):
        a, b = s[:W // 2], s[W // 2:]
        ok = all(h.count('H') >= 60 and h.count('L') >= 60 for h in (a, b))
        check(ok, '%s: the square across the graph (high / low columns: left %d / %d, right %d / %d)' % (
            what, a.count('H'), a.count('L'), b.count('H'), b.count('L')))

    def same(a, b):
        """Fraction of equal H / L classes (edges and gaps skipped)."""
        n = m = 0
        for x, y in zip(a, b):
            if x in 'HL' or y in 'HL':
                n += 1
                m += x == y
        return m / n if n else 0.0

    def drag(x0, x1, name):
        q('DBG:LCD? 0,0,4,1,7', raw=True)  # drawing held for 300 ms
        q('DBG:DRAG %d,160,%d,160,400' % (x0, x1))
        wait_ms(2500)  # the guide lines gone (GUIDE_MS)
        return classify(screen(name))

    def pre():
        """WAV:PRE? (RAW): (points, x increment, x origin)."""
        q('WAV:POIN:MODE RAW')
        p = q('WAV:PRE?').split(',')
        q('WAV:POIN:MODE NORM')
        return int(p[2]), float(p[4]), float(p[5])

    print('*IDN?', q('*IDN?'), flush=True)
    wait_ms(2500)
    q('DBG:KEY 0')  # main menu
    wait_ms(500)
    q('DBG:TAP 341,107')  # the Oscilloscope tile
    wait_ms(1500)
    v = q('DBG:VIEW?')
    check(v.startswith('Oscilloscope'), 'on the scope page (%s)' % v)
    q('DBG:HWTRIG:ENABLE 0')  # the software trigger (wavtest.py: the comparator path in the emulator)
    for c in ('CHAN1:SCAL 5', 'CHAN1:COUP DC', 'TRIG:LEV 7', 'TRIG:SWE NORM', 'TIM:SCAL 0.1'):
        q(c)
    wait_ms(500)
    q('DIG')
    for _ in range(60):
        wait_ms(500)
        if q('TRIG:STAT?').startswith('STOP'):
            break
    check(q('TRIG:STAT?').startswith('STOP'), 'DIGitize stopped')
    N, XINC, XORG = pre()
    T = N * XINC  # the record's length (s)
    PRE_S = -XORG  # its part before the trigger (s)
    print('  record: %d samples, %.0f S/s, %.2f s (%.2f s before the trigger)' % (N, 1 / XINC, T, PRE_S), flush=True)
    check(N >= 65520 and 0.5 < PRE_S < T - 0.5, 'the stored record: >= 65 520 samples, the trigger inside')
    # The screen is 1 s (100 ms/div) centred on the trigger, a drag of 200 px
    # moves it 0.5 s: half-screen drags until its left edge passes the
    # record's start, and from there (after one drag back) to its end.
    to_start = max(2, math.ceil((PRE_S - 0.5) / 0.5))
    to_end = math.ceil((T - 1.5) / 0.5) + 1
    wait_ms(500)
    ys = screen('pan0')
    # The square's top / bottom rows: the most common highest / lowest
    # yellow row per column.
    tops = [min(r) for r in ys.values()]
    bots = [max(r) for r in ys.values()]
    LV['hi'] = max(set(tops), key=tops.count)
    LV['lo'] = max(set(bots), key=bots.count)
    print('  rows: high %d, low %d' % (LV['hi'], LV['lo']), flush=True)
    check(LV['lo'] - LV['hi'] > 50, 'two levels found')
    s0 = classify(ys)
    full(s0, 'the stopped screen')

    s1 = drag(300, 100, 'pan1')
    full(s1, 'half a screen left (later samples)')
    m = same(s1[:190], s0[200:390])
    check(m >= 0.95, 'the trace moved 200 px left (%.2f of the kept half matches)' % m)
    s2 = drag(100, 300, 'pan2')
    m = same(s2, s0)
    check(m >= 0.95, 'the drag back restores the first screen (%.2f)' % m)
    s3 = drag(100, 300, 'pan3')
    full(s3, 'half a screen right (earlier samples)')
    m = same(s3[200:390], s2[:190])
    check(m >= 0.95, 'the trace moved 200 px right (%.2f)' % m)
    for _ in range(to_start - 1):
        s4 = drag(100, 300, 'pan4')  # the screen's left edge at the record start
    full(s4, 'at the record start')
    s5 = drag(100, 300, 'pan5')
    m = same(s5, s4)
    check(m >= 0.95, 'a drag past the record start leaves the screen (%.2f)' % m)
    s6 = drag(300, 100, 'pan6')
    full(s6, 'the first drag back from the start')
    m = same(s6[:190], s5[200:390])
    check(m >= 0.95, 'it moves 200 px at once (%.2f)' % m)
    for _ in range(to_end):
        s7 = drag(300, 100, 'pan7')
    full(s7, 'at the record end')
    s8 = drag(300, 100, 'pan8')
    m = same(s8, s7)
    check(m >= 0.95, 'a drag past the record end leaves the screen (%.2f)' % m)

    # Zoom while stopped (the drawing hold again before each change).
    q('DBG:LCD? 0,0,4,1,7', raw=True)
    q('TIM:SCAL 0.2')
    wait_ms(1000)
    z1 = classify(screen('zoom200ms'))
    full(z1, '200 ms/div (2 s of the %.2f s record)' % T)
    # A window longer than the record: 500 ms/div, 1 or 2 s/div for longer
    # ones. The screen stays centred on the anchor the pan left (Scope::
    # pan_from: half a screen before the record end, c = T - 0.5 s), not
    # clamped once the window is longer (zoom_frame): the record is drawn
    # where the window overlaps it, nothing beyond.
    tb = next(x for x in (0.5, 1.0, 2.0) if 10 * x > T + 0.5)
    c = T - 0.5
    want = round((min(T, c + 5 * tb) - max(0.0, c - 5 * tb)) / (10 * tb) * 400)
    q('DBG:LCD? 0,0,4,1,7', raw=True)
    q('TIM:SCAL %g' % tb)
    wait_ms(1000)
    z2 = classify(screen('zoom%gs' % tb))
    n = sum(c != '.' for c in z2)
    check(abs(n - want) <= 15, '%g s/div: the record at its true scale (%d columns, %d expected)' % (tb, n, want))
    q('DBG:LCD? 0,0,4,1,7', raw=True)
    q('TIM:SCAL 0.1')
    wait_ms(1000)
    z3 = classify(screen('zoom100ms'))
    full(z3, 'back at 100 ms/div')
    # V/div while stopped: the trace across the graph at 10 V/div, the
    # square again at 5 V/div.
    q('DBG:LCD? 0,0,4,1,7', raw=True)
    q('CHAN1:SCAL 10')
    wait_ms(1000)
    ys = screen('vdiv10')
    n = [sum(1 for x in ys if a <= x < b) for a, b in ((0, W // 2), (W // 2, W))]
    check(min(n) >= 190, '10 V/div: the trace across the graph (columns: left %d, right %d)' % tuple(n))
    q('DBG:LCD? 0,0,4,1,7', raw=True)
    q('CHAN1:SCAL 5')
    wait_ms(1000)
    full(classify(screen('vdiv5')), 'back at 5 V/div')
    check(pre()[0] == N, 'the stored record still %d samples' % N)
finally:
    try:
        os.killpg(emu.pid, 15)
    except Exception:
        pass
    emu.wait(timeout=30)

print('%s: %d failure(s)' % ('pantest', len(FAIL)), flush=True)
sys.exit(1 if FAIL else 0)
