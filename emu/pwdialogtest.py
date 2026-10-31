#!/usr/bin/env python3
"""opendm40 dialogs after a SCPI function change (#104, #143): the power
meter's wiring dialog and "Reset energy?" and the fuse check's start dialog
close, the new function's page shows.

  cd firmware && ./build.sh
  emu/pwdialogtest.py --app firmware/target/native/thumbv7em-none-eabihf/release/opendm40 [--port P] [--shots DIR]

12 V / 2 A DC on the jacks:
  * DMM page, tap W: the wiring dialog (its blue title bar, DBG:LCD?);
    `CONF:POW`: after 2 s no title bar, CONF? "POW ...";
  * long press the power page's right column: "Reset energy?" (its title
    bar); `CONF:VOLT:DC`: after 2 s no title bar, DBG:READout? unit V;
  * `CONF:CURR:DC`, MF, Fuse check: its start dialog (title bar);
    `CONF:VOLT:DC`: after 2 s no title bar, FUNC? "VOLT".
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


# Dialog title bars (x, y, w, h) and their colour (RGB666 read-back).
WIRING_TITLE = (20, 14, 440, 20)
RESET_TITLE = (100, 84, 280, 24)
FUSE_TITLE = (60, 62, 360, 22)
TITLE = (0, 156, 224)

tag = 'pwdialog-%d-%d' % (A.port, os.getpid())
cmd = [sys.executable, os.path.join(HERE, 'run.py'), 'fwrs', '--app', A.app, '--fast',
       '--usbip', '0.01', '--usbip-port', str(A.port), '--run-for', '3000', '--name', tag,
       '--dmm', 'open;pow:0@50,i=0,dc=12,idc=2@0']
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
        sys.exit('pwdialogtest: no USB/IP device on port %d' % A.port)

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
                px.append((bs[3 * i], bs[3 * i + 1], bs[3 * i + 2]))
        return px

    def title(box):
        """Share of the box in the dialog title colour."""
        px = lcd(*box)
        return sum(p == TITLE for p in px) / len(px)

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
    q('DBG:TAP 448,297')  # W: the wiring dialog (not confirmed yet)
    wait_ms(1500)
    f = title(WIRING_TITLE)
    check(f > 0.5, 'W opens the wiring dialog (title bar %.0f %%)' % (100 * f))
    q('CONF:POW')
    wait_ms(2000)
    shot('1-conf-pow')
    f = title(WIRING_TITLE)
    check(f < 0.05, 'CONF:POW closes the wiring dialog (title bar %.0f %%)' % (100 * f))
    c = q('CONF?')
    check(c.startswith('"POW'), 'CONF? %s' % c)
    if f >= 0.05:
        q('DBG:TAP 110,282')  # still open (the bug): Cancel, for the second case

    wait_ms(6000)
    q('DBG:DRAG 440,185,440,185,50,1200')  # long press: Reset energy?
    wait_ms(1200)
    f = title(RESET_TITLE)
    check(f > 0.5, 'a long press opens Reset energy? (title bar %.0f %%)' % (100 * f))
    q('CONF:VOLT:DC')
    wait_ms(2000)
    shot('2-conf-volt')
    f = title(RESET_TITLE)
    check(f < 0.05, 'CONF:VOLT:DC closes Reset energy? (title bar %.0f %%)' % (100 * f))
    r = q('DBG:READout?')
    check(r.split(',')[1:2] == ['V'], 'DBG:READout? %s (unit V)' % r)

    q('CONF:CURR:DC')
    wait_ms(2000)
    q('DBG:KEY 3')  # MF: Current Settings
    wait_ms(1500)
    q('DBG:TAP 236,159')  # Fuse check
    wait_ms(1500)
    f = title(FUSE_TITLE)
    check(f > 0.5, 'Fuse check opens its start dialog (title bar %.0f %%)' % (100 * f))
    q('CONF:VOLT:DC')
    wait_ms(2000)
    shot('3-fuse-conf-volt')
    f = title(FUSE_TITLE)
    check(f < 0.05, 'CONF:VOLT:DC closes the fuse check dialog (title bar %.0f %%)' % (100 * f))
    r = q('FUNC?')
    check(r == '"VOLT"', 'FUNC? %s' % r)
finally:
    try:
        os.killpg(emu.pid, 15)
    except Exception:
        pass
    try:
        emu.wait(timeout=30)
    except Exception:
        os.killpg(emu.pid, 9)

print('%s: %d failure(s)' % ('pwdialogtest', len(FAIL)), flush=True)
sys.exit(1 if FAIL else 0)
