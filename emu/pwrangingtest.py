#!/usr/bin/env python3
"""opendm40 power meter before its first reading (#96): while the page shows
"I ranging...", the chart's right column is not the multimeter's (no MAX /
AVG / REL box / MIN).

  cd firmware && ./build.sh
  emu/pwrangingtest.py --app firmware/target/native/thumbv7em-none-eabihf/release/opendm40 [--port P] [--shots DIR]

5 V / 4 mA DC on the jacks (the I channel walks down from 10 A). W, then OK
in the wiring dialog; the screen is read every ~0.3 s for 8 s. On every
frame with the ranging text (pixels in its box, ui_power.rs RANGING_TXT)
and no I value yet (its slot, x 290..469, y 36..65, background), the REL
box (405,205,71,38) must be background; at least one such frame, and the
power reading must come (MEAS:POW? 20 mW within 2 %).
Exit status 0 = all passed.
"""
import argparse
import os
import subprocess
import sys
import time

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
ap.add_argument('--app', required=True, help='opendm40 native ELF')
ap.add_argument('--port', type=int, default=None, help='USB/IP port (default: run.free_port)')
ap.add_argument('--shots', help='directory for PNG screenshots of the ranging frames')
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


tag = 'pwranging-%d-%d' % (A.port, os.getpid())
cmd = [sys.executable, os.path.join(HERE, 'run.py'), 'fwrs', '--app', A.app, '--fast',
       '--usbip', '0.01', '--usbip-port', str(A.port), '--run-for', '3000', '--name', tag,
       '--dmm', 'open;pow:0@50,i=0,dc=5,idc=0.004@0']
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
        sys.exit('pwrangingtest: no USB/IP device on port %d' % A.port)

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

    def lcd(x0, y0, w, h):
        """Panel pixels as RGB tuples (lcd.rs read_span's decode, as shottest.py)."""
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
                px.append((bs[3 * i] & 0xF8, bs[3 * i + 1] & 0xFC, bs[3 * i + 2] & 0xF8))
        return px

    def shot(name):
        if not A.shots:
            return
        from PIL import Image
        os.makedirs(A.shots, exist_ok=True)
        im = Image.new('RGB', (480, 320))
        im.putdata(lcd(0, 0, 480, 320))
        im.save(os.path.join(A.shots, name + '.png'))

    print('*IDN?', q('*IDN?'), flush=True)
    wait_ms(4000)
    q('DBG:TAP 448,297')  # W: the wiring dialog (first time)
    wait_ms(1500)
    q('DBG:TAP 355,282')  # OK
    bg = None
    ranging = 0
    bad = 0
    t_end = up() + 8000
    k = 0
    while up() < t_end:
        txt = lcd(40, 92, 360, 32)  # RANGING_TXT
        rel = lcd(405, 205, 71, 38)
        if bg is None:
            bg = lcd(300, 150, 1, 1)[0]  # the main area's background under the text row
        cur = lcd(290, 36, 180, 30)  # the I value's slot: empty while ranging
        if any(p != bg for p in txt) and all(p == bg for p in cur):
            ranging += 1
            n = sum(p != bg for p in rel)
            if n:
                bad += 1
                print('  ranging frame %d: %d REL box pixels drawn' % (k, n), flush=True)
            shot('ranging-%d' % k)
        k += 1
        wait_ms(300)
    check(ranging > 0, '%d frames with the ranging text' % ranging)
    check(bad == 0, 'no DMM REL box while ranging (%d of %d frames had one)' % (bad, ranging))
    r = q('MEAS:POW?')
    try:
        p = float(r)
    except (TypeError, ValueError):
        p = None
    check(p is not None and abs(p - 0.02) < 0.0004, 'the power reading comes: MEAS:POW? %s (20 mW)' % r)
finally:
    try:
        os.killpg(emu.pid, 15)
    except Exception:
        pass
    try:
        emu.wait(timeout=30)
    except Exception:
        os.killpg(emu.pid, 9)

print('%s: %d failure(s)' % ('pwrangingtest', len(FAIL)), flush=True)
sys.exit(1 if FAIL else 0)
