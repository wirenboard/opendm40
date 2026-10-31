#!/usr/bin/env python3
"""opendm40 automatic power-off on battery only (firmware/src/app.rs tick:
USB power, PB10 = Power::on_charger, holds the shutdown timer off, and the
timer counts afresh from the unplug).

  cd firmware && ./build.sh
  emu/usbofftest.py --app firmware/target/native/thumbv7em-none-eabihf/release/opendm40

PB10 (charger input power) is driven from the Renode monitor (run.py
--script, as chargetest.py): high from 1 s, low from --t-unplug s (virtual).
DBG:IDLE 60000 sets a 60 s power-off time. Then, with no traffic (a query
counts as host activity, so the test only asks at the checks):
  plugged    silent for > 65 s of virtual time before the unplug: the meter
             answers;
  fresh      silent from > 60 s before the unplug to 5..55 s after it: the
             meter answers (the time counts from the unplug, not from the
             last activity);
  off        silent for 70 s more: the meter is off.
The waits are wall-clock sleeps sized from the emulator's measured speed; a
check whose virtual time misses its window fails as "window missed" (rerun
on an idle machine).
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
ap.add_argument('--app', required=True, help='opendm40 native ELF / image')
ap.add_argument('--port', type=int, default=None, help='USB/IP port (default: a free one, run.free_port)')
ap.add_argument('--t-unplug', type=float, default=200.0, help='virtual s at which PB10 goes low (default 200)')
A = ap.parse_args()
sys.path.insert(0, HERE)
import run  # noqa: E402  (emu/run.py: free_port)
A.port = A.port or run.free_port()
os.environ['USBIP_PORT'] = str(A.port)
import usbip_client as uc  # noqa: E402

OFF_MS = 60000
TU = int(A.t_unplug * 1000)
OUT = os.path.join(HERE, 'out')
TAG = 'usboff-%d-%d' % (A.port, os.getpid())
FAIL = []


def check(ok, what):
    print('  %s %s' % ('ok  ' if ok else 'FAIL', what), flush=True)
    if not ok:
        FAIL.append(what)


os.makedirs(OUT, exist_ok=True)
script = os.path.join(OUT, TAG + '.script')
with open(script, 'w') as f:
    f.write('1.0 monitor sysbus.gpioPortB OnGPIO 10 true\n')
    f.write('%.1f monitor sysbus.gpioPortB OnGPIO 10 false\n' % A.t_unplug)

emu = subprocess.Popen([sys.executable, os.path.join(HERE, 'run.py'), 'fwrs', '--app', A.app, '--fast', '--usbip', '0.01',
                        '--usbip-port', str(A.port), '--run-for', '3000', '--name', TAG, '--script', script],
                       cwd=ROOT, stdout=open(os.path.join(OUT, TAG + '.out'), 'w'), stderr=subprocess.STDOUT,
                       start_new_session=True)
try:
    t = None
    for _ in range(150):
        try:
            t = uc.Usbtmc(uc.Dev('1-0'))
            break
        except Exception:
            time.sleep(2)
    if t is None:
        sys.exit('usbofftest: no USB/IP device on port %d' % A.port)

    def q(line, timeout=10):
        t.write(line)
        return t.read(timeout=timeout).decode(errors='replace').strip()

    def up():
        return int(q('DBG:PING?').split(',')[0])

    print('*IDN?', q('*IDN?', timeout=30), flush=True)
    while up() < 3000:
        time.sleep(0.5)
    pb10 = q('DBG:CHG?').split(',')[0]
    check(pb10 == '1', 'PB10 reads %s while plugged' % pb10)
    t.write('DBG:IDLE %d' % OFF_MS)
    # Emulator speed: virtual ms per wall s.
    v0, w0 = up(), time.time()
    time.sleep(5)
    v1, w1 = up(), time.time()
    rate = (v1 - v0) / (w1 - w0)
    print('  speed %.0f virtual ms / wall s' % rate, flush=True)

    def sleep_to(v_from, v_to):
        time.sleep(max(0.0, (v_to - v_from) / rate))

    # plugged: wake in the middle of [v1 + OFF + 5 s, TU - OFF - 5 s].
    lo, hi = v1 + OFF_MS + 5000, TU - OFF_MS - 5000
    sleep_to(v1, (lo + hi) / 2)
    try:
        v2 = up()
        check(v2 - v1 > OFF_MS + 5000, 'plugged: the meter answers after %d ms on USB power without traffic' % (v2 - v1))
        check(v2 < TU - OFF_MS, 'plugged: window (asked at %d ms, before %d)' % (v2, TU - OFF_MS))
    except OSError as e:
        check(False, 'plugged: the meter answers on USB power without traffic (%r)' % e)
        raise SystemExit
    # fresh: wake 5..55 s after the unplug.
    sleep_to(v2, TU + 25000)
    try:
        v3 = up()
        check(TU + 5000 <= v3 <= TU + OFF_MS - 5000,
              'fresh: window (asked at %d ms, unplug at %d, last traffic %d ms before it)' % (v3, TU, TU - v2))
        check(True, 'fresh: the meter answers %d ms after the unplug' % (v3 - TU))
        pb10 = q('DBG:CHG?').split(',')[0]
        check(pb10 == '0', 'PB10 reads %s after the unplug' % pb10)
    except OSError as e:
        check(False, 'fresh: the meter answers after the unplug (%r)' % e)
        raise SystemExit
    # off: silent for the power-off time + 10 s.
    sleep_to(0, OFF_MS + 20000)
    try:
        r = q('*IDN?')
    except OSError:
        r = None
    check(r is None, 'off: %d s on battery without traffic: the meter is off (*IDN? %s)'
          % (OFF_MS / 1000 + 20, 'unanswered' if r is None else 'answered %r' % r))
except SystemExit:
    pass
except Exception as e:
    check(False, 'the meter answers: %r' % e)
finally:
    try:
        os.killpg(emu.pid, 15)
    except Exception:
        pass
    emu.wait()
print('FAIL: %d checks' % len(FAIL) if FAIL else 'PASS')
sys.exit(1 if FAIL else 0)
