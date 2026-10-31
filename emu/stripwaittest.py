#!/usr/bin/env python3
"""opendm40 combined view before the first power reading (#108,
firmware/src/app_power.rs pw_strip_wait): the meter strip names the
function at once (the purple POWER badge, the range badge, dimmed dashes)
while the I channel ranges, instead of an empty strip.

  cd firmware && ./build.sh
  emu/stripwaittest.py --app firmware/target/native/thumbv7em-none-eabihf/release/opendm40 [--port P] [--shots DIR]

5 V / 4 mA DC on the jacks. Main menu > Combined view, DC V, then
CONF:POW; the strip is read (DBG:LCD?) for 1.5 s of uptime, then once
10 s later. At least one frame before the first reading (no yellow digits
in the value's x 100..175, y 26..58), each with the function badge
(purple pixels in x 16..90, y 24..42); the last frame with the reading.
Exit status 0 = PASS.
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
ap.add_argument('--shots', help='directory for PNG screenshots of the frames')
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


tag = 'stripwait-%d-%d' % (A.port, os.getpid())
cmd = [sys.executable, os.path.join(HERE, 'run.py'), 'fwrs', '--app', A.app, '--fast',
       '--usbip', '0.01', '--usbip-port', str(A.port), '--run-for', '3000', '--name', tag,
       '--dmm', 'vsrc:5;isrc:0.004@0']
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
        sys.exit('stripwaittest: no USB/IP device on port %d' % A.port)

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
        im = Image.new('RGB', (480, 80))
        im.putdata(lcd(0, 0, 480, 80))
        im.save(os.path.join(A.shots, name + '.png'))

    print('*IDN?', q('*IDN?'), flush=True)
    wait_ms(4000)
    q('DBG:KEY 0;*OPC?')            # main menu
    wait_ms(1500)
    q('DBG:TAP 240,298;*OPC?')      # Combined view: DMM + Scope
    wait_ms(1500)
    q('CONF:VOLT:DC;*OPC?')
    wait_ms(3000)
    check(q('DBG:VIEW?').split(',')[0].strip('"') == 'DMM + Scope', 'combined view')
    q('CONF:POW;*OPC?')
    def frame(k):
        badge = sum(1 for r, g, b in lcd(16, 24, 74, 18) if r > 120 and g < 60 and b > 200)
        digits = sum(1 for r, g, b in lcd(100, 26, 75, 32) if r > 200 and g > 200 and b < 80)
        shot('f%02d' % k)
        return badge, digits

    # The ranging window: the frames right after CONF:POW. (Reading the
    # panel without a pause holds the power meter's first reading off,
    # also on main: the first reading is checked after a quiet wait.)
    frames = []
    t_end = up() + 1500
    while up() < t_end:
        frames.append(frame(len(frames)))
    time.sleep(10)
    last = frame(len(frames))
    print('  frames (badge px, value px): %s; after 10 s: %d/%d' % (' '.join('%d/%d' % f for f in frames), *last), flush=True)
    pre = [f for f in frames if f[1] == 0]
    check(len(pre) > 0, 'frames before the first reading: %d' % len(pre))
    check(all(f[0] > 0 for f in pre), 'the POWER badge in every frame before it (empty: %d)' % sum(1 for f in pre if f[0] == 0))
    check(last[0] > 0 and last[1] > 0, 'the first reading comes (FETC:POW:ALL? %s)' % q('FETC:POW:ALL?').split(',')[0])
except Exception as e:
    check(False, 'run: %r' % e)
finally:
    try:
        os.killpg(emu.pid, 15)
    except Exception:
        pass
    emu.wait()
print('FAIL: %d checks' % len(FAIL) if FAIL else 'PASS')
sys.exit(1 if FAIL else 0)
