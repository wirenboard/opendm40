#!/usr/bin/env python3
"""opendm40 scope: trigger-level drag responsiveness in the emulator.

  cd firmware && ./build.sh
  emu/dragperf.py --app firmware/target/native/thumbv7em-none-eabihf/release/opendm40 [--port N]

The scope page runs on a 90 Hz square (the owner's calibrator case). Real
touch-controller drags (emu/peripherals/CST_Touch.cs Drag: a report every
20 ms) move the trigger-level marker (right edge) up and down, scheduled in
virtual time (--script) once the page is up; DBG:PROF? is reset before
them and read after: the main-loop period while dragging, the GEST section
(App::gesture per move report) and the SCOPE section (the drag preview's
graph flush), touch->pixel latency. The trigger level is read with
TRIG:LEV? after each drag (it must move). Prints the figures; exit status
1 only if the level did not move.
"""
import argparse
import os
import subprocess
import sys
import time

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
ap.add_argument('--app', required=True)
ap.add_argument('--port', type=int, default=None)
ap.add_argument('--mips', default='200')
ap.add_argument('--t0', type=float, default=20.0, help='emulator s of the first drag (the page must be up)')
A = ap.parse_args()
sys.path.insert(0, HERE)
import run  # noqa: E402
if A.port is None:
    A.port = run.free_port()
os.environ['USBIP_PORT'] = str(A.port)
import usbip_client as uc  # noqa: E402

OUT = os.path.join(HERE, 'out')
os.makedirs(OUT, exist_ok=True)
tag = 'dragperf-%d-%d' % (A.port, os.getpid())
script = os.path.join(OUT, '%s.script' % tag)
# Trigger-level marker at x 406, row 105 -> y 125 (the default level);
# four 1 s drags of 100 px (5 px per report), down and back up.
drags = [(A.t0 + 2.0 * k, 125 if k % 2 == 0 else 225, 225 if k % 2 == 0 else 125) for k in range(4)]
with open(script, 'w') as f:
    for t, y0, y1 in drags:
        f.write('%g monitor sysbus.i2c3.touch Drag 406 %d 406 %d 1000\n' % (t, y0, y1))
cmd = [sys.executable, os.path.join(HERE, 'run.py'), 'fwrs', '--mips', A.mips, '--app', A.app,
       '--usbip', '0.01', '--usbip-port', str(A.port), '--run-for', '3000', '--name', tag,
       '--log', os.path.join(OUT, tag + '.log'), '--scope', 'square,90,3.0', '--script', script]
emu = subprocess.Popen(cmd, cwd=ROOT, stdout=subprocess.DEVNULL, stderr=subprocess.STDOUT, start_new_session=True)
rc = 0
try:
    t = None
    for _ in range(150):
        try:
            t = uc.Usbtmc(uc.Dev('1-0'))
            break
        except Exception:
            time.sleep(2)
    if t is None:
        sys.exit('dragperf: no USB/IP device on port %d' % A.port)

    def q(line, timeout=60):
        t.write(line)
        if '?' not in line:
            return None
        return t.read(maxlen=16384, timeout=timeout).decode(errors='replace').strip()

    def up():
        return int(q('DBG:PING?').split(',')[0])

    def until(ms):
        while up() < ms:
            time.sleep(0.05)

    print('*IDN?', q('*IDN?'), flush=True)
    # DBG:PROF? / DBG:PROF:RESET only in a build with the feature dbg-prof
    # (else -113 "Undefined header"): without it the figures are left out.
    q('DBG:PROF:RESET')
    has_prof = q('SYST:ERR?').startswith('0,')
    if not has_prof:
        print('no DBG:PROF? in this build (--features dbg-prof): no main-loop figures', flush=True)
    until(3000)
    q('DBG:KEY 0')
    until(up() + 500)
    q('DBG:TAP 341,107')  # the Oscilloscope tile
    until(up() + 1500)
    print('view', q('DBG:VIEW?'), flush=True)
    q('CHAN1:SCAL 1')
    until(up() + 300)
    g = q('DBG:GEST?').split('|POS ')[1].split('|')[0]
    hpos, row, pos = (int(v) for v in g.split(','))
    # The level at row 105 (y 125), the first drag's start.
    q('TRIG:LEV %.4f' % ((140 + pos - 105) * 4.0 / 141.0))
    until(up() + 300)
    g = q('DBG:GEST?').split('|POS ')[1].split('|')[0]
    print('positions (hpos, trigger row, CH1 pos):', g, flush=True)
    # The script runs in emulator time, which is ahead of the firmware's
    # uptime by the boot: follow the drags by DBG:GEST?'s count instead.
    def ngest():
        return int(q('DBG:GEST?').split(';')[0].split('|')[0])
    n0 = ngest()
    if has_prof:
        q('DBG:PROF:RESET')
    lv = [float(q('TRIG:LEV?'))]
    t_end = time.time() + 600
    for k in range(len(drags)):
        while ngest() < n0 + k + 1 and time.time() < t_end:
            time.sleep(0.1)
        lv.append(float(q('TRIG:LEV?')))
    if ngest() != n0 + len(drags):
        print('FAIL %d drags logged, want %d (setup too slow? raise --t0)' % (ngest() - n0, len(drags)), flush=True)
        rc = 1
    if has_prof:
        print(q('DBG:PROF?'), flush=True)
    print('TRIG:LEV? before / after each drag: %s' % ['%.3f' % v for v in lv], flush=True)
    if not all(abs(lv[i + 1] - lv[i]) > 0.1 for i in range(len(drags))):
        print('FAIL the level did not move with every drag', flush=True)
        rc = 1
finally:
    try:
        os.killpg(emu.pid, 15)
    except Exception:
        pass
    emu.wait(timeout=30)
sys.exit(rc)
