#!/usr/bin/env python3
"""opendm40 random input ("monkey") soak: seeded random taps, side keys,
drags, view swipes and SCPI commands (edge values included) across every
page, menu and dialog, with health checks between them. Optional tool, not
in CI.

  cd firmware && ./build.sh --features mute
  emu/monkeytest.py --app firmware/target/native/thumbv7em-none-eabihf/release/opendm40 \
      --seed 1 --minutes 20

Inputs go over USBTMC: DBG:TAP, DBG:KEY 0..3[,hold], DBG:DRAG, DBG:SWIPE L/R
and plain SCPI. Never sent: the power key, DBG:SWIPE D,2 (power off),
DBG:RELEASE / RESET / UPDATE / UPLOAD. The automatic power-off is pushed out
with DBG:IDLE after every boot.

Checks:
  - every --ping steps: DBG:PING? answers within --timeout s and the uptime
    only grows (a drop = the firmware restarted: a panic or a watchdog);
  - every --home steps: MENU (up to 4 presses) reaches the main menu, and
    the Multimeter tile opens the multimeter (a stuck page otherwise);
    then the error queue drained (random SCPI fills it; not checked);
  - the Renode log's "fwdgt: timeout" lines (watchdog resets);
  - the deepest stack use (DBG:BOOT? STACK, the paint high-water mark),
    sampled at every home check and printed at the end.
A failure prints the last --tail inputs (with the firmware uptime) and the
seed (a stuck page also a screenshot in emu/out/) and exits 1 (--keep-going: logs it and
goes on). Every input is also logged to emu/out/monkey-<port>.log. Exit
status 0 = PASS.

The seed fixes the input sequence, not its timing (the inputs are paced in
wall time): cut a finding down to a short scripted repro before filing it.
"""
import argparse
import os
import random
import re
import struct
import subprocess
import sys
import time

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
OUT = os.path.join(HERE, 'out')
ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
ap.add_argument('--app', required=True, help='opendm40 native ELF')
ap.add_argument('--port', type=int, default=None, help='USB/IP port (default: a free one)')
ap.add_argument('--seed', type=int, default=1)
ap.add_argument('--minutes', type=float, default=20, help='virtual run time of the inputs (default 20)')
ap.add_argument('--steps', type=int, default=0, help='stop after this many inputs (0 = by time)')
ap.add_argument('--ping', type=int, default=10, help='uptime check every N inputs (default 10)')
ap.add_argument('--home', type=int, default=60, help='MENU / main menu check every N inputs (default 60)')
ap.add_argument('--timeout', type=float, default=15, help='a reply later than this (wall s) = a hang (default 15)')
ap.add_argument('--scpi', type=float, default=0.25, help='share of SCPI commands among the inputs (default 0.25)')
ap.add_argument('--no-mips', action='store_true', help='default CPU speed instead of --fast')
ap.add_argument('--disk', help='SPI disk image (copied first; default: a copy of emu/out/disk.bin, the working copy '
                'run.py makes from the MSC dump)')
ap.add_argument('--keep-going', action='store_true', help='log a failure and go on')
ap.add_argument('--tail', type=int, default=25, help='inputs printed before a failure (default 25)')
A = ap.parse_args()
sys.path.insert(0, HERE)
import run  # noqa: E402
import bootinfo  # noqa: E402
A.port = A.port or run.free_port()
os.environ['USBIP_PORT'] = str(A.port)
import usbip_client as uc  # noqa: E402

R = random.Random(A.seed)
os.makedirs(OUT, exist_ok=True)
name = 'monkey-%d' % A.port
LOG = open(os.path.join(OUT, name + '.log'), 'w')
HIST = []
FAILS = []

# ---- the inputs -------------------------------------------------------------

# Numbers for SCPI arguments: ordinary, zero, negative, tiny, huge, the SCPI
# "no value" 9.9E37, words.
EDGE = ['0', '-0', '1', '-1', '0.5', '1E-9', '-1E-9', '1E9', '-1E9', '1E20', '-1E20', '9.9E37', '-9.9E37',
        '3.4E38', '1E39', '65535', '65536', '4294967296', 'MIN', 'MAX', 'DEF', 'AUTO', 'ON', 'OFF', '', 'X',
        '1,2', '1e', '.', '-', '+', '1.5E+3', '#H10']
VALUED = [  # SCPI headers that take a number
    'CONF:VOLT', 'CONF:VOLT:AC', 'CONF:CURR', 'CONF:RES', 'CONF:POW', 'VOLT:RANG', 'CURR:RANG', 'RES:RANG',
    'VOLT:RES', 'RES:RES', 'CALC:LIM:LOW', 'CALC:LIM:UPP', 'TEMP:OFFS', 'TEMP:NTC', 'SYST:BEEP:VOL',
    'POW:VOLT:RANG', 'POW:CURR:RANG', 'POW:BURD', 'POW:LIM', 'POW:PHAS', 'TIM:SCAL', 'CHAN1:SCAL', 'CHAN1:OFFS',
    'TRIG:LEV', 'WAV:POIN', 'SOUR:FREQ', 'SOUR:VOLT', 'SOUR:VOLT:OFFS', 'SOUR:FUNC:SQU:DCYC',
    'SYST:ALER:ENAB', 'DBG:IDLE 86400000;:DBG:PINCH', 'DBG:GRAB']
PLAIN = [  # SCPI commands and queries without a free number
    'CONF:VOLT:DC', 'CONF:VOLT:AC', 'CONF:VOLT:ACDC', 'CONF:VOLT:AUTO', 'CONF:CURR:DC', 'CONF:CURR:AC',
    'CONF:CURR:AUTO', 'CONF:RES', 'CONF:CONT', 'CONF:DIOD', 'CONF:CAP', 'CONF:FREQ', 'CONF:TEMP', 'CONF:TEMP PT100',
    'CONF:TEMP TCK', 'CONF:POW', 'CONF?', 'READ?', 'FETC?', 'MEAS:VOLT?', 'MEAS:RES?', 'MEAS:CAP?', 'MEAS:FREQ?',
    'MEAS:TEMP?', 'MEAS:POW?', 'FETC:POW:ALL?', 'FUNC?', 'RATE FAST', 'RATE MED', 'RATE SLOW', 'RATE?',
    'VOLT:RANG:AUTO ON', 'VOLT:RANG:AUTO OFF', 'VOLT:RANG:AUTO ONCE', 'CALC:AVER:STAT ON', 'CALC:AVER:STAT OFF',
    'CALC:AVER:CLE', 'CALC:AVER?', 'CALC:LIM:STAT ON', 'CALC:LIM:STAT OFF', 'CALC:LIM:CLE', 'CALC:LIM:FAIL?',
    'CALC:AHOL ON', 'CALC:AHOL OFF', 'CALC:SMO ON', 'CALC:SMO OFF', 'DISP:BURD ON', 'DISP:BURD OFF',
    'RUN', 'STOP', 'SING', 'DIG', 'TRIG:STAT?', 'TRIG:SWE AUTO', 'TRIG:SWE NORM', 'TRIG:SLOP NEG', 'TRIG:SLOP POS',
    'CHAN1:COUP AC', 'CHAN1:COUP DC', 'ACQ:SRAT?', 'SCOP:MEAS:VPP?', 'SCOP:MEAS:FREQ?', 'WAV:PRE?', 'WAV:DATA?',
    'WAV:FORM WORD', 'WAV:FORM BYTE', 'SOUR:FUNC SIN', 'SOUR:FUNC SQU', 'SOUR:FUNC TRI', 'SOUR:FUNC RAMP',
    'SOUR:VOLT:OFFS AUTO', 'OUTP ON', 'OUTP OFF', 'OUTP?', 'SYST:RAIL BOTH', 'SYST:RAIL AUTO', 'SYST:RAIL:STAT?',
    'SYST:ALER?', 'SYST:TEMP?', 'SYST:FUSE:TEST', 'SYST:FUSE?', 'UNIT:TEMP F', 'UNIT:TEMP C', 'POW:ENER:RES',
    'POW:METH ALT', 'POW:METH SIM', 'POW:COUP AC', 'POW:COUP DC', 'LOG:STAT?', 'LOG:STAT ON', 'LOG:STAT OFF',
    'LOG:CSV ON', 'LOG:CSV OFF', 'LOG:CSV?', 'LOG:DATA?',
    'LOG:COUN?', 'MMEM:CAT?', '*IDN?', '*CLS', '*RST', '*OPC?', 'SYST:ERR?', 'DBG:VIEW?', 'DBG:READ?', 'DBG:PROF?',
    'DBG:HWTRIG:ENABLE 0', 'DBG:GEST?', 'DBG:ALAR?', 'DBG:SWIPE D,3', 'SYST:IDN:EMUL 34401A', 'SYST:IDN:EMUL OFF',
    ':::', 'VOLT:RANG', 'BOGUS:CMD 1', 'CONF:VOLT 6;:READ?', 'A' * 300]


def scpi_line():
    if R.random() < 0.4:
        h = R.choice(VALUED)
        v = R.choice(EDGE)
        q = R.random() < 0.15
        return h + ('? ' if q else ' ') + v if v else h + ('?' if q else '')
    return R.choice(PLAIN)


def ui_input():
    r = R.random()
    if r < 0.55:
        # Taps: anywhere, or near the edges and the bottom button row (most
        # buttons live there).
        if R.random() < 0.3:
            return 'DBG:TAP %d,%d' % (R.randrange(480), R.choice([R.randrange(40), R.randrange(240, 320)]))
        return 'DBG:TAP %d,%d' % (R.randrange(480), R.randrange(320))
    if r < 0.75:
        k = R.choice([0, 1, 1, 2, 2, 3, 3, 3])
        return 'DBG:KEY %d,%d' % (k, R.choice([0, 0, 0, 1200]))
    if r < 0.9:
        x0, y0 = R.randrange(480), R.randrange(320)
        return 'DBG:DRAG %d,%d,%d,%d,%d,%d' % (x0, y0, max(0, min(479, x0 + R.randint(-200, 200))),
                                               max(0, min(319, y0 + R.randint(-150, 150))), R.choice([100, 300, 600]),
                                               R.choice([0, 0, 500]))
    return 'DBG:SWIPE %s' % R.choice('LR')


# ---- the link ---------------------------------------------------------------

class Link:
    def __init__(self):
        self.d = self.t = None

    def connect(self, wait):
        t_end = time.time() + wait
        while time.time() < t_end:
            try:
                self.close()
                self.d = uc.Dev('1-0')
                self.d.s.settimeout(A.timeout)
                self.t = uc.Usbtmc(self.d)
                return True
            except Exception:
                time.sleep(2)
        return False

    def close(self):
        try:
            if self.d:
                self.d.close()
        except Exception:
            pass
        self.d = self.t = None

    def send(self, line):
        """Write one line; return the reply of a query (an exception when none
        comes within --timeout)."""
        self.t.write(line)
        if '?' in line.split(' ')[0] or re.search(r';:?[A-Z:]*\?', line):
            return self.t.read(maxlen=1 << 16, timeout=A.timeout).decode(errors='replace').strip()
        return ''

    def q(self, line):
        return self.send(line)


L = Link()


def log(msg):
    LOG.write(msg + '\n')
    LOG.flush()


def fail(what):
    print('FAIL %s (seed %d, step %d)' % (what, A.seed, step), flush=True)
    print('  last inputs (uptime ms, input):')
    for h in HIST[-A.tail:]:
        print('   ', h)
    log('FAIL ' + what)
    FAILS.append(what)
    if not A.keep_going:
        finish(1)


def screenshot(path):
    """The screen through DBG:LCD? (as firmware/tools/dm40dbg.py shot)."""
    try:
        from PIL import Image
    except ImportError:
        return
    img = Image.new('RGB', (480, 320))
    for y in range(0, 320, 4):
        n = 480 * 4 * 3 // 2
        L.t.write('DBG:LCD? 0,%d,480,4,%d' % (y, n + 1))
        r = L.t.read(maxlen=1 << 16, timeout=A.timeout)
        nd = int(r[1:2])
        ln = int(r[2:2 + nd])
        data = r[2 + nd:]
        while len(data) < ln:
            data += L.t.read(maxlen=1 << 16, timeout=A.timeout)
        b = b''.join(struct.pack('>H', v) for v in struct.unpack('<%dH' % (ln // 2), data[:ln])[1:])
        img.putdata([tuple(b[3 * i + k] & 0xFC for k in range(3)) for i in range(480 * 4)])
        if y == 0:
            full = Image.new('RGB', (480, 320))
        full.paste(img.crop((0, 0, 480, 4)), (0, y))
    full.save(path)
    print('  screenshot', path)


def boot_setup():
    L.q('DBG:IDLE 86400000')
    # The emulator's comparator trigger uses the default calibration and
    # misses; the software trigger is the model's path (emu/README.md).
    L.q('DBG:HWTRIG:ENABLE 0')


def recover(why):
    """No reply: wait for a watchdog restart, reconnect, report."""
    L.close()
    if not L.connect(90):
        fail('%s; no USB device after 90 s' % why)
        finish(1)
    up = int(L.q('DBG:PING?').split(',')[0])
    boot = L.q('DBG:BOOT?')
    fail('%s; back after a restart, uptime %d ms, boot %s' % (why, up, ','.join(boot.split(',')[:6])))
    boot_setup()
    return up


def fwdgt_count():
    try:
        with open(os.path.join(OUT, name + '-emu.log')) as f:
            return sum('fwdgt: timeout' in ln for ln in f)
    except OSError:
        return 0


# ---- the run ----------------------------------------------------------------

STACK = [0, 0]  # deepest stack use seen (DBG:BOOT? stack_used / stack_size), size


def stack_sample():
    f = bootinfo.fields(L.q('DBG:BOOT?'))
    if 'stack_size' in f:
        STACK[0] = max(STACK[0], f['stack_used'])
        STACK[1] = f['stack_size']


def stack_report():
    if not STACK[1]:
        return 'stack: no DBG:BOOT? stack fields'
    return 'stack: deepest use %d of %d bytes, %d never used' % (STACK[0], STACK[1], STACK[1] - STACK[0])


emu = None
step = 0
DISK = os.path.join(OUT, name + '-disk.bin')


def kill_emu():
    global emu
    if emu:
        try:
            os.killpg(emu.pid, 15)
        except OSError:
            pass
        emu.wait()
        emu = None


def finish(code):
    try:
        stack_sample()
    except Exception:
        pass
    kill_emu()
    ok = code == 0 and not FAILS
    if ok and os.path.exists(DISK):
        os.remove(DISK)  # kept after a failure: the screenshots / recordings on it
    print(stack_report())
    print('%s: %d inputs, seed %d, %d failure(s)' % ('PASS' if ok else 'FAIL', step, A.seed, len(FAILS)))
    sys.exit(code or (1 if FAILS else 0))


def main():
    global emu, step
    run_for = int(A.minutes * 60 + 600)
    args = [sys.executable, os.path.join(HERE, 'run.py'), 'fwrs', '--app', A.app, '--usbip', '0.01',
            '--usbip-port', str(A.port), '--uid', run.emu_uid(A.port).hex().upper(), '--name', name + '-emu',
            '--run-for', str(run_for)]
    if not A.no_mips:
        args.append('--fast')
    # The monkey takes screenshots and recordings: never on the shared
    # working copy of the disk.
    # No working copy yet (a fresh checkout): copy the MSC dump run.py
    # would copy, not let run.py create the shared one.
    src = A.disk or os.path.join(OUT, 'disk.bin')
    if not os.path.exists(src):
        src = run.DISK_SRC
    subprocess.run(['cp', src, DISK], check=True)
    args += ['--disk', DISK]
    emu = subprocess.Popen(args, cwd=ROOT, stdout=open(os.path.join(OUT, name + '-emu.out'), 'w'),
                           stderr=subprocess.STDOUT, start_new_session=True)
    if not L.connect(300):
        fail('no USB/IP device on port %d' % A.port)
        finish(1)
    boot_setup()
    last_up = int(L.q('DBG:PING?').split(',')[0])
    t_end = last_up + A.minutes * 60e3
    print('seed %d, port %d, uptime %d ms at the first input' % (A.seed, A.port, last_up), flush=True)
    wd = 0
    while True:
        step += 1
        if A.steps and step > A.steps:
            break
        if step % A.ping == 0:
            try:
                up = int(L.q('DBG:PING?').split(',')[0])
            except Exception as e:
                up = recover('no reply to DBG:PING? (%s)' % type(e).__name__)
            if up < last_up:
                fail('uptime went back %d -> %d ms: the firmware restarted' % (last_up, up))
                boot_setup()
            last_up = up
            n = fwdgt_count()
            if n > wd:
                fail('watchdog reset in the Renode log (%d)' % n)
                wd = n
            if up > t_end:
                break
            if step % (A.ping * 30) == 0:
                print('  step %d uptime %.0f s, %d failure(s)' % (step, up / 1e3, len(FAILS)), flush=True)
        if step % A.home == 0:
            home_check()
            continue
        line = scpi_line() if R.random() < A.scpi else ui_input()
        HIST.append('%8d %s' % (last_up, line[:100]))
        log('%d %d %s' % (step, last_up, line[:200]))
        try:
            L.send(line)
        except Exception as e:
            last_up = recover('no reply to %r (%s)' % (line[:60], type(e).__name__))
        time.sleep(R.choice([0, 0.05, 0.2, 0.5]))
    finish(0)


def vwait(ms):
    """Wait `ms` of firmware uptime (the emulator's speed varies with the
    host's load; a held key from DBG:KEY n,1200 ends first)."""
    t0 = int(L.q('DBG:PING?').split(',')[0])
    while int(L.q('DBG:PING?').split(',')[0]) < t0 + ms:
        time.sleep(0.1)


def home_check():
    try:
        vwait(1500)
        if L.q('DBG:VIEW?').startswith('Touch test'):
            # The touch test page takes the side keys for its test; only a
            # held power key leaves it (manual "Touch test").
            L.q('DBG:TOUCH:TEST OFF')
            vwait(500)
        for _ in range(4):
            v = L.q('DBG:VIEW?')
            if v.startswith('Main menu'):
                break
            L.q('DBG:KEY 0')
            vwait(800)
        else:
            v = L.q('DBG:VIEW?')
        HIST.append('%8s home: %s' % ('', v))
        log('%d home %s' % (step, v))
        if not v.startswith('Main menu'):
            try:
                screenshot(os.path.join(OUT, '%s-stuck-%d.png' % (name, step)))
            except Exception as e:
                print('  no screenshot (%s)' % type(e).__name__)
            fail('MENU x4 does not reach the main menu: DBG:VIEW? %s' % v)
            return
        L.q('DBG:TAP 140,105')
        vwait(1500)
        v = L.q('DBG:VIEW?')
        if not v.startswith('Multimeter'):
            fail('the Multimeter tile opens %s' % v)
        stack_sample()
        # The queue: drained only. Random SCPI fills it to its 8 entries
        # (scpi.rs ERRQ) between home checks, so neither the count nor an
        # overflow (-350) says anything.
        n = int(L.q('SYST:ERR:COUN?'))
        for _ in range(n + 1):
            if L.q('SYST:ERR?').startswith('0,'):
                break
    except Exception as e:
        recover('no reply in the home check (%s)' % type(e).__name__)


if __name__ == '__main__':
    try:
        main()
    except KeyboardInterrupt:
        finish(1)
    finally:
        kill_emu()  # any other exception: no emulator left running
