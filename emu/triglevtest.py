#!/usr/bin/env python3
"""TRIGger:LEVel keeps the level as given (#80): TRIG:LEV? answers the set
value at several V/div, also for a level off screen, and across a V/div
change; the trigger fires on the set level.

  emu/triglevtest.py --app firmware/target/native/thumbv7em-none-eabihf/release/opendm40

Before the fix the level was the screen row below the value: TRIG:LEV 0.1
at 500 mV/div answered 0.1135, and an off-screen level became the edge
row's volts.

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
tag = 'triglev-%d-%d' % (A.port, os.getpid())
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
        sys.exit('triglevtest: no USB/IP device on port %d' % A.port)

    def q(line, timeout=60):
        t.write(line)
        if '?' not in line:
            return None
        return t.read(maxlen=8192, timeout=timeout).decode(errors='replace').strip()

    def up():
        return int(q('DBG:PING?').split(',')[0])

    def wait_ms(ms):
        t0 = up()
        while up() - t0 < ms:
            time.sleep(0.1)

    print('*IDN?', q('*IDN?'), flush=True)
    wait_ms(2500)
    q('DBG:KEY 0')  # main menu
    wait_ms(500)
    q('DBG:TAP 341,107')  # the Oscilloscope tile
    wait_ms(1500)
    v = q('DBG:VIEW?')
    check(v.startswith('Oscilloscope'), 'on the scope page (%s)' % v)
    # The software trigger: the emulator's comparator map is not the
    # meter's calibration (emu/README "Scope trigger").
    q('DBG:HWTRIG:ENABLE 0')
    q('TRIG:SWE NORM')
    q('TIM:SCAL 5e-4')
    q('CHAN1:OFFS 0')
    for vdiv, lev in (('0.5', 0.1), ('0.5', 0.2), ('0.5', -0.5), ('1', 0.31), ('2', 0.25), ('0.5', 3.0)):
        q('CHAN1:SCAL %s' % vdiv)
        q('TRIG:LEV %g' % lev)
        got = float(q('TRIG:LEV?'))
        check(abs(got - lev) < 1e-4, '%s V/div: TRIG:LEV %g -> TRIG:LEV? %g' % (vdiv, lev, got))
    # Kept across a V/div change (also the one set off screen above).
    q('CHAN1:SCAL 2')
    got = float(q('TRIG:LEV?'))
    check(abs(got - 3.0) < 1e-4, 'TRIG:LEV 3 (off screen at 500 mV/div), then 2 V/div: TRIG:LEV? %g' % got)
    # The trigger fires on a level inside the signal (about +-0.9 V in the
    # emulator), never on one above it.
    q('CHAN1:SCAL 0.5')
    q('TRIG:LEV 0.3')
    wait_ms(2000)
    st = q('TRIG:STAT?')
    check(st == 'TD', 'TRIG:LEV 0.3 on the sine, Normal: %s (want TD)' % st)
    q('TRIG:LEV 1.5')
    wait_ms(2000)
    st = q('TRIG:STAT?')
    check(st == 'WAIT', 'TRIG:LEV 1.5 above the sine, Normal: %s (want WAIT)' % st)
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
