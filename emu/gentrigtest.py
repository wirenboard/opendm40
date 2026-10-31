#!/usr/bin/env python3
"""opendm40 scope trigger source GEN (firmware/docs/scope-gen-channel.md)
in the emulator: the generator's phase is read on the DAC path (sine,
triangle, the 10 kHz square: DBG:GENTRIG:STAT? counts readings, not
failures) and the computed trigger fires in Normal sweep on a stream
timebase; the PWM path (square 2 MHz) triggers at 100 ns and 1 us/div;
TRIG:SOUR CHAN1 (the short form) is accepted; the generator off: WAIT.

  cd firmware && ./build.sh
  emu/gentrigtest.py --app firmware/target/native/thumbv7em-none-eabihf/release/opendm40

The emulator's TIM6 counts but moves no DMA (PwmTimer, GD32_DMA): the DAC
table position stands still, so the DAC phase is not checked here (nor the
linear-record timebases, whose timed start needs it), only that it is
read. Nor are bus wait states modelled: the read window the device needs
short (gen.rs `reads`) is the device check's DBG:GENTRIG:STAT?.

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
tag = 'gentrig-%d-%d' % (A.port, os.getpid())
log = os.path.join(OUT, '%s.log' % tag)
disk = os.path.join(OUT, tag + '.disk.bin')
shutil.copy(os.path.join(ROOT, 'stock/re/dumps/spi_dumps/dm40_msc_full_2026-10-13.bin'), disk)
cmd = [sys.executable, os.path.join(HERE, 'run.py'), 'fwrs', '--mips', A.mips, '--app', A.app,
       '--usbip', '0.01', '--usbip-port', str(A.port), '--run-for', '3000', '--name', tag, '--log', log,
       '--disk', disk, '--scope', 'sine,1000,3.0']
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
        sys.exit('gentrigtest: no USB/IP device on port %d' % A.port)

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
    check(q('SYST:ERR?').startswith('0'), 'no error before')
    q('TRIG:SOUR CHAN1')
    check(q('SYST:ERR?').startswith('0'), 'TRIG:SOUR CHAN1 accepted (the short form)')
    q('TRIG:SOUR GEN')
    check(q('TRIG:SOUR?') == 'GEN', 'TRIG:SOUR? GEN')
    q('TRIG:SWE NORM')
    q('TRIG:LEV 1.5')
    q('CHAN1:SCAL 1')

    def status(what, n=4):
        got = []
        for _ in range(n):
            wait_ms(300)
            got.append(q('TRIG:STAT?'))
        check(got.count('TD') >= n - 1, '%s: TRIG:STAT? %s' % (what, ','.join(got)))

    def stat():
        return [int(x) for x in q('DBG:GENTRIG:STAT?').split(',')]

    q('SOUR:VOLT 3')
    q('SOUR:VOLT:OFFS 1.5')
    q('OUTP ON')
    # DAC path: the phase is read for every record (readings grow, no
    # failure), and a stream timebase triggers.
    # 2 ms/div and slower: the stream path (<= 1.25 MS/s); faster: the
    # linear record with its timed start (the phase standing still here).
    for fn, f, tbs in (('SIN', 1000, ['2e-3', '2e-4', '2e-5']), ('TRI', 1000, ['5e-3', '2e-4']), ('SQU', 10000, ['2e-3', '1e-5'])):
        q('SOUR:FUNC %s' % fn)
        q('SOUR:FREQ %d' % f)
        for tb in tbs:
            q('TIM:SCAL %s' % tb)
            wait_ms(300)
            a = stat()
            wait_ms(1000)
            b = stat()
            check(b[0] - a[0] >= 3 and b[1] - a[1] <= 1, 'GEN %s %d Hz at %s s/div: phase readings %d, failed %d (DBG:GENTRIG:STAT? %s)'
                  % (fn, f, tb, b[0] - a[0], b[1] - a[1], b))
            if float(tb) >= 2e-3:
                status('GEN %s %d Hz at %s s/div' % (fn, f, tb))
    q('SOUR:FUNC SQU')
    q('SOUR:FREQ 2000000')
    for tb in ('1e-7', '1e-6'):
        q('TIM:SCAL %s' % tb)
        status('GEN SQU 2 MHz (PWM path) at %s s/div' % tb)
    q('OUTP OFF')
    wait_ms(500)
    got = [q('TRIG:STAT?') for _ in range(3)]
    check('TD' not in got, 'generator off: no GEN trigger (%s)' % ','.join(got))
    q('TRIG:SOUR CHAN1')
    q('TRIG:SWE AUTO')
finally:
    try:
        os.killpg(emu.pid, 15)
    except Exception:
        pass
    try:
        emu.wait(timeout=30)
    except Exception:
        os.killpg(emu.pid, 9)
    for f in (disk,):
        try:
            os.remove(f)
        except OSError:
            pass

print('%s: %d failure(s)' % ('gentrigtest', len(FAIL)), flush=True)
sys.exit(1 if FAIL else 0)
