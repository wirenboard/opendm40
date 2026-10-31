#!/usr/bin/env python3
"""Scope page: SCPI setting changes while a popup is open show at once in the
top bar, beside the popup (#82; before: only after the popup closed).

  emu/popupscpitest.py --app firmware/target/native/thumbv7em-none-eabihf/release/opendm40 [--shots DIR]

Scope page, a 1 kHz sine. With no popup, TIM:SCAL 2e-4 + CHAN1:SCAL 2 give
the reference timebase and V/div badges; back to 1 ms/div, 1 V/div. Then
the CH1 popup is opened (a tap on CH1), the same two commands are sent and
the badges must equal the reference while the popup stays open (its box
mostly unchanged: not painted over by the graph).

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


def check(ok, what):
    print('  %s %s' % ('ok  ' if ok else 'FAIL', what), flush=True)
    if not ok:
        FAIL.append(what)


OUT = os.path.join(HERE, 'out')
os.makedirs(OUT, exist_ok=True)
tag = 'popupscpi-%d-%d' % (A.port, os.getpid())
log = os.path.join(OUT, '%s.log' % tag)
disk = os.path.join(OUT, tag + '.disk.bin')
shutil.copy(os.path.join(ROOT, 'stock/re/dumps/spi_dumps/dm40_msc_full_2026-10-13.bin'), disk)
cmd = [sys.executable, os.path.join(HERE, 'run.py'), 'fwrs', '--mips', A.mips, '--app', A.app,
       '--usbip', '0.01', '--usbip-port', str(A.port), '--run-for', '3000', '--name', tag, '--log', log,
       '--disk', disk, '--scope', 'sine,1000,2.0']
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
        sys.exit('popupscpitest: no USB/IP device on port %d' % A.port)

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

    def badges():
        """The timebase and the CH1 V/div badges of the top bar."""
        return lcd(0, 20, 79, 62) + lcd(0, 20, 286, 62)

    def popup():
        """The CH1 popup's box (scope_ui::popup_box)."""
        return lcd(20, 280, 214, 200)

    def settle():
        wait_ms(1500)  # past the timebase badge's flash (150 ms)

    print('*IDN?', q('*IDN?'), flush=True)
    wait_ms(2500)
    q('DBG:KEY 0')  # main menu
    wait_ms(500)
    q('DBG:TAP 341,107')  # the Oscilloscope tile
    wait_ms(1500)
    v = q('DBG:VIEW?')
    check(v.startswith('Oscilloscope'), 'on the scope page (%s)' % v)
    for c in ('TRIG:SWE AUTO', 'TRIG:LEV 0', 'RUN'):
        q(c)
    q('TIM:SCAL 2e-4')
    q('CHAN1:SCAL 2')
    settle()
    ref = badges()
    shot('popupscpi-ref')
    q('TIM:SCAL 1e-3')
    q('CHAN1:SCAL 1')
    settle()
    b0 = badges()
    check(sum(a != b for a, b in zip(b0, ref)) > 20, '1 ms/div, 1 V/div badges differ from 200 us/div, 2 V/div')
    q('DBG:TAP 446,73')  # CH1: the popup opens
    settle()
    p0 = popup()
    shot('popupscpi-open')
    check(badges() == b0, 'popup open: badges unchanged')
    q('TIM:SCAL 2e-4')
    q('CHAN1:SCAL 2')
    settle()
    b1 = badges()
    p1 = popup()
    shot('popupscpi-changed')
    n = sum(a != b for a, b in zip(b1, ref))
    check(n == 0, 'popup open, TIM:SCAL 2e-4 + CHAN1:SCAL 2: the badges show them (%d px differ from the reference)' % n)
    n = sum(a != b for a, b in zip(p0, p1))
    check(n < len(p0) // 10, 'the popup stays drawn (%d of %d px changed)' % (n, len(p0)))
    q('DBG:TAP 446,73')  # CH1 again: the popup closes
    settle()
    shot('popupscpi-closed')
    n = sum(a != b for a, b in zip(badges(), ref))
    check(n == 0, 'popup closed: the badges as the reference (%d px differ)' % n)
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
