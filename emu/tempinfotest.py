#!/usr/bin/env python3
"""opendm40 temperature page (MF in the temperature function): the info line
"Cold junction ..." follows the readings while the page is open (it stayed
as drawn when the page opened: an offset step, which moves the cold
junction, did not show).

  cd firmware && ./build.sh
  emu/tempinfotest.py --app firmware/target/native/thumbv7em-none-eabihf/release/opendm40 [--port P] [--shots DIR]

A type K thermocouple at 100 C, cold junction 25 C (run.py --dmm tc / cj):
  * Multimeter, CONF:TEMP TCK, MF: the temperature page; its info line
    (DBG:LCD? rows 282..315) noted;
  * five taps on Offset "+": TEMP:OFFS? 0.5; after 6 s the info line has
    changed, and equals the line of the page opened again (MF to close,
    MF to open), which shows the cold junction 0.5 C higher.
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


# The info line (ui.rs T_INFO) and the Offset "+" key (ui.rs temp_rect).
INFO = (8, 282, 464, 34)
PLUS = (437, 162)

tag = 'tempinfo-%d-%d' % (A.port, os.getpid())
cmd = [sys.executable, os.path.join(HERE, 'run.py'), 'fwrs', '--app', A.app, '--fast',
       '--usbip', '0.01', '--usbip-port', str(A.port), '--run-for', '3000', '--name', tag,
       '--dmm', 'tc:100;cj:25']
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
        sys.exit('tempinfotest: no USB/IP device on port %d' % A.port)

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

    def lcd(x0, y0, w, h):
        """Panel pixels as RGB tuples, 4 rows per read (as gallerytest.py)."""
        out = []
        for y in range(y0, y0 + h, 4):
            rows = min(4, y0 + h - y)
            n = w * rows * 3 // 2 + 1
            b = q('DBG:LCD? %d,%d,%d,%d,%d' % (x0, y, w, rows, n), raw=True)
            nd = int(b[1:2])
            d = b[2 + nd:2 + nd + int(b[2:2 + nd])]
            v = [d[i] | d[i + 1] << 8 for i in range(2, len(d), 2)]
            bs = bytearray()
            for x in v:
                bs += bytes([x >> 8, x & 0xFF])
            out += [tuple(bs[3 * i:3 * i + 3]) for i in range(w * rows)]
        return out

    def shot(name):
        if not A.shots:
            return
        from PIL import Image
        im = Image.new('RGB', (480, 320))
        im.putdata(lcd(0, 0, 480, 320))
        os.makedirs(A.shots, exist_ok=True)
        im.save(os.path.join(A.shots, name + '.png'))

    def view():
        return q('DBG:VIEW?').split(',')[0]

    print('*IDN?', q('*IDN?'), flush=True)
    wait_ms(2000)
    # The multimeter, whatever the start page.
    if view() != 'Multimeter':
        for _ in range(3):
            if view() == 'Main menu':
                break
            q('DBG:KEY 0')
            wait_ms(800)
        q('DBG:TAP 140,105')
        wait_ms(1000)
    check(view() == 'Multimeter', 'the multimeter page (%s)' % view())
    q('CONF:TEMP TCK')
    q('TEMP:OFFS 0')
    wait_ms(6000)
    cj = q('MEAS:TEMP:CJ?')
    print('  MEAS:TEMP:CJ?', cj, flush=True)
    q('DBG:KEY 3')
    wait_ms(1500)
    shot('t1-page')
    a = lcd(*INFO)
    check(len(set(a)) > 1, 'info line drawn')

    for _ in range(5):
        q('DBG:TAP %d,%d' % PLUS)
        wait_ms(300)
    offs = float(q('TEMP:OFFS?'))
    check(abs(offs - 0.5) < 1e-3, 'TEMP:OFFS? 0.5 (%g)' % offs)
    wait_ms(6000)
    shot('t2-stepped')
    b = lcd(*INFO)
    check(b != a, 'info line changed after the offset steps')

    # The page drawn anew shows the same line.
    q('DBG:KEY 3')
    wait_ms(1500)
    q('DBG:KEY 3')
    wait_ms(1500)
    shot('t3-reopened')
    c = lcd(*INFO)
    check(c == b, 'info line equals the reopened page\'s')
    print('*IDN?', q('*IDN?'), flush=True)
    err = q('SYST:ERR?')
    check(err.startswith('0'), 'SYST:ERR? %s' % err)
finally:
    try:
        os.killpg(emu.pid, 15)
    except Exception:
        pass
    emu.wait()

print('PASS' if not FAIL else 'FAIL: %s' % '; '.join(FAIL))
sys.exit(1 if FAIL else 0)
