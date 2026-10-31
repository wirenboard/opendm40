#!/usr/bin/env python3
"""opendm40 scope: main-loop cost of the coded ring (firmware/src/cring.rs)
at a slow timebase, and the stopped record's redraw, in the emulator.

  cd firmware && ./build.sh
  emu/ringperf.py --app firmware/target/native/thumbv7em-none-eabihf/release/opendm40 [--scope noise,1,8]

At --mips 110 (the device's speed per instruction, emu/README.md) the
scope page runs at 100 ms/div (25 kS/s, the coded ring) on --scope; after
6 s DBG:PROF? is reset and read 10 s later: ACQ (Scope::poll: draining the
staging ring, coding a full block), FRAME, MEAS, the main-loop period.
Then STOP (one more capture), and the stopped record redrawn at 10 ms/div
(a window of it) and at 1 s/div (all of it, zoomed out), DBG:PROF? after
each; then roll at 200 ms/div for 25 s; then a DIGitize record at
100 ms/div (the coded ring) redrawn at 10 ms, 100 ms and 1 s/div. Prints
the figures; no pass / fail.
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
ap.add_argument('--mips', default='110')
ap.add_argument('--scope', default='sine,50,1', help='run.py --scope SPEC')
ap.add_argument('--vdiv', default='0.2')
A = ap.parse_args()
sys.path.insert(0, HERE)
import run  # noqa: E402
if A.port is None:
    A.port = run.free_port()
os.environ['USBIP_PORT'] = str(A.port)
import usbip_client as uc  # noqa: E402

OUT = os.path.join(HERE, 'out')
os.makedirs(OUT, exist_ok=True)
tag = 'ringperf-%d-%d' % (A.port, os.getpid())
cmd = [sys.executable, os.path.join(HERE, 'run.py'), 'fwrs', '--mips', A.mips, '--app', A.app,
       '--usbip', '0.01', '--usbip-port', str(A.port), '--run-for', '3000', '--name', tag,
       '--log', os.path.join(OUT, tag + '.log'), '--scope', A.scope]
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
        sys.exit('ringperf: no USB/IP device on port %d' % A.port)

    def q(line, timeout=120):
        t.write(line)
        if '?' not in line:
            return None
        return t.read(maxlen=1 << 20, timeout=timeout).decode(errors='replace').strip()

    def qs(line):
        return q(line)

    def up():
        return int(q('DBG:PING?').split(',')[0])

    def wait(ms):
        e = up() + ms
        while up() < e:
            time.sleep(0.1)

    def prof(what):
        p = q('DBG:PROF?')
        keep = [l for l in p.replace(';', '\n').splitlines() if any(k in l for k in ('acq', 'frame', 'meas', 'loop', 'scope', 'x2', 'd.read', 'd.chart'))]
        print('--- %s\n%s' % (what, '\n'.join(keep) if keep else p), flush=True)

    print('*IDN?', q('*IDN?'), flush=True)
    wait(3000)
    # The software trigger (as emu/wavtest.py: the model's comparator path
    # sometimes never completes a frame).
    q('DBG:HWTRIG:ENABLE 0')
    for _ in range(10):
        q('DBG:KEY 0')
        wait(1000)
        q('DBG:TAP 341,107')  # the Oscilloscope tile
        wait(2000)
        if qs('DBG:VIEW?').startswith('Oscilloscope'):
            break
    print('view', qs('DBG:VIEW?'), flush=True)
    for c in ('CHAN1:SCAL %s' % A.vdiv, 'TRIG:SWE AUTO', 'TRIG:LEV 0', 'TIM:SCAL 0.1', 'RUN'):
        q(c)
    wait(6000)
    q('DBG:PROF:RESET')
    wait(10000)
    prof('running, 100 ms/div, %s' % A.scope)
    q('STOP')
    while q('TRIG:STAT?') != 'STOP':
        time.sleep(0.2)
    q('WAV:POIN MAX')
    print('record', q('WAV:POIN?'), 'samples', flush=True)
    for tb in ('0.01', '1'):
        q('DBG:PROF:RESET')
        q('TIM:SCAL %s' % tb)
        wait(2000)
        prof('stopped, redrawn at %s s/div' % tb)
    # Roll at 200 ms/div: the newest screen (25 000 samples) read from the
    # ring every 100 ms, once it holds coded blocks.
    q('TIM:SCAL 0.2')
    q('RUN')
    wait(15000)
    q('DBG:PROF:RESET')
    wait(10000)
    prof('roll, 200 ms/div, %s' % A.scope)
    # DIGitize at 100 ms/div: a long coded record, redrawn stopped.
    q('TIM:SCAL 0.1')
    q('DIG')
    while q('TRIG:STAT?') != 'STOP':
        time.sleep(0.2)
    print('DIGitize record', q('WAV:POIN?'), 'samples', flush=True)
    for tb in ('0.01', '0.1', '1'):
        q('DBG:PROF:RESET')
        q('TIM:SCAL %s' % tb)
        wait(3000)
        prof('DIGitize record, redrawn at %s s/div' % tb)
finally:
    try:
        os.killpg(emu.pid, 15)
    except Exception:
        pass
    emu.wait(timeout=30)
