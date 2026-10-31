#!/usr/bin/env python3
"""opendm40 analog rails after a boot with "Both always" (firmware/src/scope.rs
`rails`; field report 2026-10-26: grey screen on opening the scope, and on the
DM40A the meter switched itself off).

  cd firmware && ./build.sh
  emu/railstest.py --app firmware/target/native/thumbv7em-none-eabihf/release/opendm40

Run 1: SYST:RAIL BOTH, wait for the settings file save (DBG:BOOT? SAVE,OK);
its SPI flash is run 2's disk. Run 2 boots with Both always (run.py
--rail-log logs each PE2 / PC4 change), then over USB: MENU -> Oscilloscope,
SYST:RAIL AUTO on the Scope page, MENU -> Multimeter. Checks:
  - PC4 (scope rail) goes high before the switch to Auto (the 0.2.9 code
    believed it on from boot and never raised it: the scope read code 0);
  - PE2 and PC4 are never both low (the power hold: 0.2.9 dropped PE2 at the
    switch to Auto with PC4 still low, and the meter switched off);
  - at the end PE2 high, PC4 low (the Multimeter page, Auto).
Exit status 0 = PASS. USB/IP port in 50000-50999 unless --port.
"""
import argparse
import os
import re
import shutil
import subprocess
import sys
import time

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
OUT = os.path.join(HERE, 'out')
ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
ap.add_argument('--app', required=True, help='opendm40 native ELF / image')
ap.add_argument('--port', type=int, default=None, help='USB/IP port (default: a free one in 50000-50999)')
A = ap.parse_args()
sys.path.insert(0, HERE)
import run  # noqa: E402
import bootinfo  # noqa: E402
if A.port is None:
    A.port = run.free_port(50000, 50999)
print('USB/IP port %d' % A.port, flush=True)
os.environ['USBIP_PORT'] = str(A.port)
import usbip_client as uc  # noqa: E402

SERIAL = run.emu_uid(A.port).hex().upper()
TAG = 'rails-%d-%d' % (A.port, os.getpid())
FAIL = []


def check(ok, what):
    print('  %s %s' % ('ok  ' if ok else 'FAIL', what), flush=True)
    if not ok:
        FAIL.append(what)


class Link:
    def __init__(self):
        for _ in range(150):
            try:
                self.d = uc.Dev('1-0')
                break
            except Exception:
                time.sleep(2)
        else:
            raise RuntimeError('no USB/IP device on port %d' % A.port)
        self.t = uc.Usbtmc(self.d)

    def q(self, s, timeout=30):
        self.t.write(s)
        return self.t.read(timeout=timeout).decode(errors='replace').strip()

    def w(self, s):
        self.t.write(s)

    def up(self):
        return int(self.q('DBG:PING?').split(',')[0])

    def wait_up(self, ms):
        while self.up() < ms:
            time.sleep(0.2)

    def close(self):
        try:
            self.d.close()
        except Exception:
            pass


def start(name, disk, extra):
    log = open(os.path.join(OUT, name + '.out'), 'w')
    return subprocess.Popen([sys.executable, os.path.join(HERE, 'run.py'), 'fwrs', '--app', A.app, '--usbip', '0.01',
                             '--usbip-port', str(A.port), '--uid', SERIAL, '--name', name, '--disk', disk] + extra,
                            cwd=ROOT, stdout=log, stderr=subprocess.STDOUT, start_new_session=True)


def stop(emu):
    try:
        os.killpg(emu.pid, 15)
    except Exception:
        pass
    emu.wait()


def save_ok(l):
    return bootinfo.fields(l.q('DBG:BOOT?')).get('save') == 'OK'


os.makedirs(OUT, exist_ok=True)
t_auto = 0
src = os.path.join(ROOT, 'stock/re/dumps/spi_dumps/dm40_msc_full_2026-10-13.bin')
disk0 = os.path.join(OUT, TAG + '-disk0.bin')
disk1 = os.path.join(OUT, TAG + '-disk1.bin')
log2 = os.path.join(OUT, TAG + '-2.renode.log')
shutil.copy(src, disk0)
print('run 1: SYST:RAIL BOTH, saved', flush=True)
emu = start(TAG + '-1', disk0, ['--run-for', '30', '--save-disk', disk1])
try:
    l = Link()
    l.w('SYST:RAIL BOTH')
    check(l.q('SYST:RAIL?') == 'BOTH', 'SYST:RAIL? BOTH')
    t0 = l.up()
    while not save_ok(l) or l.up() < t0 + 8000:
        time.sleep(1)
    l.close()
    emu.wait(timeout=3600)
except Exception as e:
    check(False, 'run 1: %r' % e)
    stop(emu)
check(os.path.exists(disk1), 'disk saved at the end of run 1')
if os.path.exists(disk1):
    print('run 2: boot with Both always', flush=True)
    emu = start(TAG + '-2', disk1, ['--run-for', '3000', '--rail-log', '--log', log2])
    try:
        l = Link()
        check(l.q('SYST:RAIL?') == 'BOTH', 'boot: SYST:RAIL? BOTH')
        l.wait_up(6000)
        l.w('DBG:KEY 0')
        l.wait_up(l.up() + 1000)
        l.w('DBG:TAP 341,107')
        l.wait_up(l.up() + 2000)
        v = l.q('DBG:VIEW?')
        check(v.startswith('Oscilloscope'), 'Scope page: DBG:VIEW? %s' % v)
        t_auto = l.up()
        l.w('SYST:RAIL AUTO')
        l.wait_up(t_auto + 2000)
        check(l.q('SYST:RAIL?') == 'AUTO', 'SYST:RAIL? AUTO')
        l.w('DBG:KEY 0')
        l.wait_up(l.up() + 1000)
        l.w('DBG:TAP 140,107')
        l.wait_up(l.up() + 2000)
        v = l.q('DBG:VIEW?')
        check(v.startswith('Multimeter'), 'Multimeter page: DBG:VIEW? %s' % v)
        l.close()
    except Exception as e:
        check(False, 'run 2: %r' % e)
    finally:
        stop(emu)
    ev = []
    for line in open(log2, errors='replace'):
        m = re.search(r'RAIL PE2 (\d) PC4 (\d) t=(\d+):(\d+):([\d.]+)', line)
        if m:
            ms = ((int(m.group(3)) * 60 + int(m.group(4))) * 60 + float(m.group(5))) * 1000
            ev.append((ms, int(m.group(1)), int(m.group(2))))
    for e in ev:
        print('  rail t=%8.0f ms PE2 %d PC4 %d' % e)
    check(bool(ev), 'rail changes logged (run.py --rail-log)')
    check(not any(pe2 == 0 and pc4 == 0 for _, pe2, pc4 in ev), 'PE2 and PC4 never both low')
    check(any(pc4 for ms, _, pc4 in ev if ms < t_auto), 'PC4 high before the switch to Auto (%d ms)' % t_auto)
    check(bool(ev) and ev[-1][1:] == (1, 0), 'end: PE2 high, PC4 low')
for f in (disk0, disk1):
    try:
        os.remove(f)
    except OSError:
        pass
print('FAIL: %d checks' % len(FAIL) if FAIL else 'PASS')
sys.exit(1 if FAIL else 0)
