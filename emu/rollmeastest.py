#!/usr/bin/env python3
"""opendm40 scope measurements in roll mode (#128) in the emulator: a roll
frame has no linear record, so the measurements and SCOP:MEAS:*? are empty
(9.91E37, or 0 on builds before #84) instead of the last non-roll frame's
values; after STOP they measure the roll record again.

  emu/rollmeastest.py --app firmware/target/native/thumbv7em-none-eabihf/release/opendm40 [--port P]

Input: a 10 Hz sine, 2 Vpp, 0.2 V offset. 20 ms/div: SCOP:MEAS:FREQ? 10 Hz.
1 s/div (roll, TRIG:STAT? RUN): FREQ? and VAVG? are no longer the 20 ms/div
values. STOP after 12 s (the record fills the screen): FREQ? 10 Hz again. Exit status 0 = all passed.
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
tag = 'rollmeas-%d-%d' % (A.port, os.getpid())
log = os.path.join(OUT, '%s.log' % tag)
disk = os.path.join(OUT, tag + '.disk.bin')
shutil.copy(os.path.join(ROOT, 'stock/re/dumps/spi_dumps/dm40_msc_full_2026-10-13.bin'), disk)
cmd = [sys.executable, os.path.join(HERE, 'run.py'), 'fwrs', '--fast', '--app', A.app,
       '--usbip', '0.01', '--usbip-port', str(A.port), '--run-for', '3000', '--name', tag, '--log', log,
       '--disk', disk, '--scope', 'sine,10,2.0,0.2']
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
        sys.exit('rollmeastest: no USB/IP device on port %d' % A.port)

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
            time.sleep(0.2)

    def meas():
        return [float(q('SCOP:MEAS:%s?' % k)) for k in ('FREQ', 'VAVG', 'VPP')]

    print('*IDN?', q('*IDN?'), flush=True)
    q('DBG:HWTRIG:ENABLE 0')
    q('CHAN1:SCAL 0.5')
    q('TRIG:LEV 0.2')
    q('TIM:SCAL 0.02')
    q('RUN')
    wait_ms(3000)
    f0, a0, p0 = meas()
    print('  20 ms/div: FREQ %g VAVG %g VPP %g' % (f0, a0, p0), flush=True)
    check(abs(f0 - 10) < 0.2, '20 ms/div: FREQ? 10 Hz (%g)' % f0)
    q('TIM:SCAL 1')
    wait_ms(12000)  # the roll record fills the screen (10 s): STOP then measures it
    st = q('TRIG:STAT?')
    check(st == 'RUN', '1 s/div rolls: TRIG:STAT? RUN (%s)' % st)
    f1, a1, p1 = meas()
    print('  roll: FREQ %g VAVG %g VPP %g' % (f1, a1, p1), flush=True)
    empty = lambda v: v > 9e37 or v == 0.0  # noqa: E731 (0: SCOP:MEAS NaN before #84)
    check(empty(f1) and empty(a1) and empty(p1), 'roll: no measurements (not the 20 ms/div ones)')
    q('STOP')
    wait_ms(1500)
    f2, a2, p2 = meas()
    print('  stopped: FREQ %g VAVG %g VPP %g' % (f2, a2, p2), flush=True)
    check(abs(f2 - 10) < 0.2 and abs(p2 - p0) < 0.05 * p0, 'STOP: the roll record measured (FREQ %g, VPP %g)' % (f2, p2))
finally:
    run.kill_group(emu)
    for p in (disk,):
        if os.path.exists(p):
            os.remove(p)

print('PASS' if not FAIL else 'FAIL: %d checks' % len(FAIL))
sys.exit(1 if FAIL else 0)
