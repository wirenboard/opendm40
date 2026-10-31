#!/usr/bin/env python3
"""opendm40 DMM trend chart after a range change to another unit (#93):
the chart, its axis labels and the REL box header follow the readout's
unit, also when the new range reads OL.

  cd firmware && ./build.sh
  emu/chartunittest.py --app firmware/target/native/thumbv7em-none-eabihf/release/opendm40 [--port P] [--shots DIR]

3.3 V DC on V-COM:
  * `VOLT:RANG 0.6` right after the boot: OL in mV on an empty chart; the
    chart band (DBG:LCD?, rows 172..281) is the reference;
  * `VOLT:RANG 6`: the chart fills with the 3.3 V trace (the band differs);
  * `VOLT:RANG 0.6` again: OL in mV; the band equals the reference (before
    the fix: the 3.3 V trace, V labels and "REL V").
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
ap.add_argument('--shots', help='directory for PNG screenshots of each step')
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


# The chart band: axis labels, chips, the plot and the right column.
BAND = (172, 110)

tag = 'chartunit-%d-%d' % (A.port, os.getpid())
cmd = [sys.executable, os.path.join(HERE, 'run.py'), 'fwrs', '--app', A.app, '--fast',
       '--usbip', '0.01', '--usbip-port', str(A.port), '--run-for', '3000', '--name', tag,
       '--dmm', 'vsrc:3.3']
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
        sys.exit('chartunittest: no USB/IP device on port %d' % A.port)

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
        """Panel rows as RGB tuples (lcd.rs read_span's decode, as shottest.py)."""
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
        im.putdata(lcd(0, 320))
        im.save(os.path.join(A.shots, name + '.png'))

    def read():
        try:
            return float(q('READ?'))
        except (TypeError, ValueError):
            return None

    print('*IDN?', q('*IDN?'), flush=True)
    wait_ms(3000)
    q('VOLT:RANG 0.6')
    wait_ms(3000)
    r = read()
    check(r is not None and r > 9e37, '600 mV on 3.3 V reads OL (READ? %s)' % r)
    ref = lcd(*BAND)
    shot('1-ol-fresh')

    q('VOLT:RANG 6')
    wait_ms(5000)
    r = read()
    check(r is not None and abs(r - 3.3) < 0.01, '6 V reads 3.3 V (READ? %s)' % r)
    v = lcd(*BAND)
    check(sum(a != b for a, b in zip(v, ref)) > 100, 'the 6 V chart differs from the empty mV chart')
    shot('2-6v')

    q('VOLT:RANG 0.6')
    wait_ms(3000)
    v = lcd(*BAND)
    diff = sum(a != b for a, b in zip(v, ref))
    shot('3-ol-after-6v')
    check(diff == 0, 'OL in mV after 6 V: the chart band = the empty mV chart (%d pixels differ)' % diff)
finally:
    try:
        os.killpg(emu.pid, 15)
    except Exception:
        pass
    try:
        emu.wait(timeout=30)
    except Exception:
        os.killpg(emu.pid, 9)

print('%s: %d failure(s)' % ('chartunittest', len(FAIL)), flush=True)
sys.exit(1 if FAIL else 0)
