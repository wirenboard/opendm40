#!/usr/bin/env python3
"""The experimental 600 Mohm range (dmm.rs OHM600, ohm r7) is manual only, in
the emulator: opendm40 native + the HY3131 model, jacks open, SCPI over USB/IP.
The model reads the ohm rows ratiometrically only (no absolute AD1 reference,
SOP1P masked), so the r7 value is not checked here (the device is); the range
rules are:

  explicit  CONF:RES 600E6: RES:RANG? 6E8, RES:RANG:AUTO? 0;
  read      CONF:RES 600E6;:READ? and MEAS:RES? 600E6: a reading (the
            model's OL, +9.9E37), not SCPI NaN (+9.91E37: no reading in
            time; the first r7 reading takes ~2.2 s);
  res       RES:RES 1E6 / 1E3 on 600 Mohm: SENS:RATE? unchanged, still 6E8;
  tap       a tap on the Ohm function button on 600 Mohm (a second tap:
            the test-current variant elsewhere): still 6E8, manual;
  max       CONF:RES MAX: 6E7 (MAX is the top stock range);
  value     RES:RANG 100E6 (a value above 60 Mohm): 6E8;
  auto      RES:RANG:AUTO ON from 600 Mohm: AUTO 1 and RES:RANG? at most 6E7
            for HOLD s (open jacks: the autorange's top, never r7);
  steps     from manual 60 Mohm, 9 short AUTO-key presses (DBG:KEY 2): every
            RES:RANG? below 6E8 (the key steps the items before AUTO);
  from600   a short AUTO-key press on 600 Mohm: 60 ohm (the ring's first);
  long      a long AUTO-key press on 600 Mohm (DBG:KEY 2,1500): AUTO, at
            most 6E7.
SYST:ERR? 0 at the end. Exit status 0 = PASS. Results: emu/out/ohm600m.json.

  emu/hy3131/ohm600mtest.py --app firmware/target/native/thumbv7em-none-eabihf/release/opendm40 --port 64761
"""
import argparse
import json
import os
import subprocess
import sys
import time

HERE = os.path.dirname(os.path.abspath(__file__))
EMU = os.path.dirname(HERE)
ROOT = os.path.dirname(EMU)
OUT = os.path.join(EMU, 'out')
ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
ap.add_argument('--app', help='opendm40 native image (default: run.py builds HEAD)')
ap.add_argument('--port', type=int, default=64761, help='USB/IP port (default 64761)')
A = ap.parse_args()
os.environ['USBIP_PORT'] = str(A.port)
sys.path.insert(0, EMU)
import usbip_client as uc  # noqa: E402

HOLD = 6.0
RUN_FOR = 150


def main():
    os.makedirs(OUT, exist_ok=True)
    name = 'ohm600m.%d' % os.getpid()
    cmd = [sys.executable, os.path.join(EMU, 'run.py'), 'fwrs', '--fast', '--usbip', '3',
           '--usbip-port', str(A.port), '--run-for', str(RUN_FOR), '--dmm', 'open;noise:0', '--name', name] \
        + (['--app', os.path.abspath(A.app)] if A.app else [])
    p = subprocess.Popen(cmd, cwd=ROOT, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)
    tm = None
    t_wall = time.time()
    while p.poll() is None and tm is None and time.time() - t_wall < 300:
        try:
            tm = uc.Usbtmc(uc.Dev('1-0'))
        except Exception as e:
            print('attach:', type(e).__name__, e, flush=True)
            time.sleep(1)
    bad, rows = [], []

    def q(s):
        tm.write(s)
        return tm.read(timeout=60).decode(errors='replace').strip()

    def up():
        return int(q('DBG:PING?').split(',')[0]) / 1000.0

    def wait(s):
        t0 = up()
        while up() - t0 < s:
            time.sleep(0.05)

    def rng():
        r = float(q('RES:RANG?'))
        a = q('RES:RANG:AUTO?')
        rows.append({'t': up(), 'RANG': r, 'AUTO': a})
        return r, a

    def check(ph, ok, what):
        print('%-9s %s %s' % (ph, 'ok  ' if ok else 'FAIL', what), flush=True)
        if not ok:
            bad.append('%s: %s' % (ph, what))

    if tm is None:
        bad.append('no USB/IP attach')
    else:
        try:
            tm.write('CONF:RES 600E6')
            wait(1.0)
            r, a = rng()
            check('explicit', abs(r - 6e8) < 1 and a == '0', 'RANG %g AUTO %s' % (r, a))
            for c in ['CONF:RES 600E6;:READ?', 'MEAS:RES? 600E6']:
                t1 = time.time()
                v = q(c)
                check('read', v.startswith('+9.90000000E+37'), '%s -> %s in %.1f s' % (c, v, time.time() - t1))
            rate = q('SENS:RATE?')
            for c in ['RES:RES 1E6', 'RES:RES 1E3']:
                tm.write(c)
                wait(0.5)
                r, a = rng()
                rt = q('SENS:RATE?')
                check('res', rt == rate and abs(r - 6e8) < 1, '%s: RATE %s (was %s), RANG %g' % (c, rt, rate, r))
            tm.write('DBG:TAP 172,298')
            wait(1.5)
            r, a = rng()
            check('tap', abs(r - 6e8) < 1 and a == '0', 'RANG %g AUTO %s' % (r, a))
            tm.write('CONF:RES MAX')
            wait(1.0)
            r, a = rng()
            check('max', abs(r - 6e7) < 1 and a == '0', 'RANG %g AUTO %s' % (r, a))
            tm.write('RES:RANG 100E6')
            wait(1.0)
            r, a = rng()
            check('value', abs(r - 6e8) < 1 and a == '0', 'RANG %g AUTO %s' % (r, a))
            tm.write('RES:RANG:AUTO ON')
            t0 = up()
            seen = []
            while up() - t0 < HOLD:
                seen.append(rng())
            check('auto', seen and all(a == '1' and r <= 6e7 + 1 for r, a in seen),
                  '%d polls, max RANG %g, AUTO %s' % (len(seen), max(r for r, _ in seen), sorted({a for _, a in seen})))
            tm.write('CONF:RES 60E6')
            wait(1.0)
            steps = []
            for _ in range(9):
                tm.write('DBG:KEY 2')
                wait(0.6)
                steps.append(rng()[0])
            check('steps', all(r < 6e8 - 1 for r in steps), 'RANG after each: %s' % ' '.join('%g' % r for r in steps))
            tm.write('CONF:RES 600E6')
            wait(1.0)
            tm.write('DBG:KEY 2')
            wait(0.6)
            r, a = rng()
            check('from600', abs(r - 60) < 1e-3 and a == '0', 'RANG %g AUTO %s' % (r, a))
            tm.write('CONF:RES 600E6')
            wait(1.0)
            tm.write('DBG:KEY 2,1500')
            wait(2.5)
            r, a = rng()
            check('long', r <= 6e7 + 1 and a == '1', 'RANG %g AUTO %s' % (r, a))
            e = q('SYST:ERR?')
            check('errors', e.startswith('0,'), e)
        except Exception as e:
            bad.append('link ended: %s' % e)
    p.kill()
    out, _ = p.communicate()
    ok = not bad
    with open(os.path.join(OUT, 'ohm600m.json'), 'w') as f:
        json.dump({'ok': ok, 'bad': bad, 'rows': rows}, f, indent=1)
    print('RESULT ohm600m %s %s' % ('OK' if ok else 'FAIL', '; '.join(bad[:6])), flush=True)
    return ok


if __name__ == '__main__':
    sys.exit(0 if main() else 1)
