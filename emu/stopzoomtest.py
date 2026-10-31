#!/usr/bin/env python3
"""Stopped scope: every zoom re-renders the stored record on the screen, also
when the screen was read back just before (DBG:LCD?, as a host screenshot;
an on-device screenshot holds the panel the same way).

  emu/stopzoomtest.py --app firmware/target/native/thumbv7em-none-eabihf/release/opendm40 [--shots DIR]

Scope page, a 1 kHz sine, 5 ms/div, STOP. Then TIM:SCAL 2 ms, 1 ms, 500 us,
2 ms/div, each right after a read-back of the graph: the graph must change
with each step (before the fix the re-render frame was dropped while the
panel was held, and the old trace stayed). Then DIGitize right after a
read-back, the input meanwhile a square: the one-shot capture is drawn.

Exit status 0 = all passed.
"""
import argparse
import os
import shutil
import subprocess
import sys
import time

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
ap.add_argument('--app', required=True, help='opendm40 native ELF')
ap.add_argument('--port', type=int, default=None, help='USB/IP port (default: run.free_port)')
ap.add_argument('--shots', help='directory for PNG screenshots of each step')
ap.add_argument('--mips', default='200')
A = ap.parse_args()
sys.path.insert(0, HERE)
import run  # noqa: E402
if A.port is None:
    A.port = run.free_port()
os.environ['USBIP_PORT'] = str(A.port)
import usbip_client as uc  # noqa: E402

FAIL = []
# Virtual second at which the input becomes a square (the DIGitize case).
SWITCH_S = 40


def check(ok, what):
    print('  %s %s' % ('ok  ' if ok else 'FAIL', what), flush=True)
    if not ok:
        FAIL.append(what)


OUT = os.path.join(HERE, 'out')
os.makedirs(OUT, exist_ok=True)
tag = 'stopzoom-%d-%d' % (A.port, os.getpid())
log = os.path.join(OUT, '%s.log' % tag)
disk = os.path.join(OUT, tag + '.disk.bin')
shutil.copy(os.path.join(ROOT, 'stock/re/dumps/spi_dumps/dm40_msc_full_2026-10-13.bin'), disk)
cmd = [sys.executable, os.path.join(HERE, 'run.py'), 'fwrs', '--mips', A.mips, '--app', A.app,
       '--usbip', '0.01', '--usbip-port', str(A.port), '--run-for', '3000', '--name', tag, '--log', log,
       '--disk', disk, '--scope', 'sine,1000,2.0', '--scope', 'square,1000,2.0@%d' % SWITCH_S]
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
        sys.exit('stopzoomtest: no USB/IP device on port %d' % A.port)

    def q(line, timeout=60, raw=False):
        t.write(line)
        if '?' not in line:
            return None
        b = t.read(maxlen=65536, timeout=timeout)
        return b if raw else b.decode(errors='replace').strip()

    def up():
        return int(q('DBG:PING?').split(',')[0])

    def wait_ms(ms):
        t0 = up()
        while up() - t0 < ms:
            time.sleep(0.1)

    def block(b):
        nd = int(b[1:2])
        return b[2 + nd:2 + nd + int(b[2:2 + nd])]

    def lcd(y0, h, x0=0, w=480):
        """Panel rows as RGB565 (lcd.rs read_span's decode, as shottest.py)."""
        px = []
        for y in range(y0, y0 + h, 4):
            rows = min(4, y0 + h - y)
            n = (w * rows * 3 + 1) // 2 + 1
            d = block(q('DBG:LCD? %d,%d,%d,%d,%d' % (x0, y, w, rows, n), raw=True))
            wd = [d[i] | d[i + 1] << 8 for i in range(2, len(d), 2)]
            bs = bytearray()
            for v in wd:
                bs += bytes([v >> 8, v & 0xFF])
            for i in range(w * rows):
                r, g, b = bs[3 * i], bs[3 * i + 1], bs[3 * i + 2]
                px.append((r & 0xF8) << 8 | (g & 0xFC) << 3 | b >> 3)
        return px

    def shot(name):
        if not A.shots:
            return
        from PIL import Image
        os.makedirs(A.shots, exist_ok=True)
        px = lcd(0, 320)
        im = Image.new('RGB', (480, 320))
        im.putdata([((v >> 11) << 3, ((v >> 5) & 63) << 2, (v & 31) << 3) for v in px])
        im.save(os.path.join(A.shots, name + '.png'))

    def graph():
        """The graph's trace rows (without the top bar, the tabs and the
        measurement row)."""
        return lcd(40, 220, 20, 380)

    print('*IDN?', q('*IDN?'), flush=True)
    wait_ms(2500)
    q('DBG:KEY 0')  # main menu
    wait_ms(500)
    q('DBG:TAP 341,107')  # the Oscilloscope tile
    wait_ms(1500)
    v = q('DBG:VIEW?')
    check(v.startswith('Oscilloscope'), 'on the scope page (%s)' % v)
    q('TRIG:SWE AUTO')
    q('TRIG:LEV 0')
    q('CHAN1:SCAL 1')
    q('TIM:SCAL 5e-3')
    q('RUN')
    wait_ms(2500)
    q('STOP')
    wait_ms(1500)
    check(q('TRIG:STAT?') == 'STOP', 'stopped')
    shot('stopzoom-0')
    prev = graph()
    for i, tb in enumerate(('2e-3', '1e-3', '5e-4', '2e-3')):
        # `prev` was just read back: the panel is held for a moment, the
        # change comes within that hold.
        q('TIM:SCAL %s' % tb)
        wait_ms(1500)
        cur = graph()
        diff = sum(a != b for a, b in zip(prev, cur))
        check(diff > 50, 'TIM:SCAL %s while stopped: the graph re-rendered (%d pixels changed)' % (tb, diff))
        shot('stopzoom-%d' % (i + 1))
        prev = cur
    # A one-shot capture that completes within the hold (PR #78 review):
    # the input turns into a square at SWITCH_S (run.py --scope ...@T) while
    # the stopped sine is shown; read back the graph, DIGitize at once.
    q('DBG:HWTRIG:ENABLE 0')  # the software trigger: a frame within the hold
    while up() < SWITCH_S * 1000 + 1000:
        time.sleep(0.2)
    prev = graph()
    q('DIGitize')
    wait_ms(1500)
    check(q('TRIG:STAT?') == 'STOP', 'DIGitize ended in STOP')
    cur = graph()
    diff = sum(a != b for a, b in zip(prev, cur))
    check(diff > 50, 'DIGitize right after a read-back: the new capture drawn (%d pixels changed)' % diff)
    shot('stopzoom-digitize')
finally:
    try:
        os.killpg(emu.pid, 15)
    except ProcessLookupError:
        pass
    try:
        emu.wait(10)
    except subprocess.TimeoutExpired:
        os.killpg(emu.pid, 9)
    for p in (disk,):
        try:
            os.unlink(p)
        except OSError:
            pass

print('FAIL: %d' % len(FAIL) if FAIL else 'PASS', flush=True)
sys.exit(1 if FAIL else 0)
