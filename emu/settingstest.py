#!/usr/bin/env python3
"""opendm40 settings across a power-off (firmware/src/setfile.rs: the copy in
_SETTING.BIN on the SPI disk; the backup SRAM is not powered while the meter
is off, device 2026-10-24).

  cd firmware && ./build.sh
  emu/settingstest.py --app firmware/target/native/thumbv7em-none-eabihf/release/opendm40

The power cycle: two emulator runs. The first one's SPI flash is saved at
its end (run.py --save-disk) and is the second one's disk; the second run
is a new machine, so its backup SRAM starts zeroed and RCU_RSTSCK holds the
power-on flags, as after a real power-off.

Cases (each on a fresh copy of the MSC dump: no _SETTING.BIN yet):
  timed  DBG:BOOT? SETFILE,CREATED and SETTINGS,DEFAULTS on the first boot;
         SYST:ALER:ALAR VOLUME, then DBG:BOOT? SAVE,OK within ~10 s (5 s
         debounce after the 2 s backup SRAM save); the run ends without a
         power-off (a battery pulled). Second boot: SETTINGS,FILE,
         SETFILE,PRESENT, SYST:ALER:ALAR? VOLUME.
  key    SYST:ALER:ALAR OFF at uptime ~29 s, the power key held at virtual
         30..32 s (run.py power-hold: off at the release), so no timed save
         can come first (DBG:BOOT?: the save count unchanged after the change). Second boot:
         SETTINGS,FILE, SYST:ALER:ALAR? OFF.
  remember  Mode Remember ON (MEM:STAT:REC:AUTO ON), then resistance on the
         fixed 600 ohm range, RATE FAST and two taps on the trend chart's
         timebase chip (320 -> 640 -> 1280 ms/div); the power key as in
         `key`. Second boot: RES, RANG:AUTO? 0 at 600 ohm, FAST, 1280 ms/div
         (DBG:VIEW? field 4), Mode Remember still ON.
  default  The same changes with Mode Remember OFF. Second boot: DC V in
         autorange, MED, 320 ms/div (a power-off starts a new measurement).
Exit status 0 = PASS. Per-process names, private disk copies; USB/IP port in
63900-63999 unless --port.
"""
import argparse
import os
import shutil
import subprocess
import sys
import time

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
OUT = os.path.join(HERE, 'out')
ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
ap.add_argument('--app', required=True, help='opendm40 native ELF / image')
ap.add_argument('--port', type=int, default=None, help='USB/IP port (default: a free one in 63900-63999)')
ap.add_argument('--cases', default='timed,key,remember,default')
ap.add_argument('--run1', type=float, default=45.0, help='virtual seconds of the first run (default 45)')
A = ap.parse_args()
sys.path.insert(0, HERE)
import run  # noqa: E402
import bootinfo  # noqa: E402
if A.port is None:
    A.port = run.free_port(63900, 63999)
print('USB/IP port %d' % A.port, flush=True)
os.environ['USBIP_PORT'] = str(A.port)
import usbip_client as uc  # noqa: E402

SERIAL = run.emu_uid(A.port).hex().upper()
TAG = 'settings-%d-%d' % (A.port, os.getpid())
FAIL = []
KEY_AT = 30.0


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

    def close(self):
        try:
            self.d.close()
        except Exception:
            pass


def settings(boot):
    """The settings fields of a DBG:BOOT? reply as a list (bootinfo.py)."""
    return bootinfo.settings(boot)


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


def trend(l):
    """The trend chart's ms/div (DBG:VIEW? field 4)."""
    return l.q('DBG:VIEW?').split(',')[3]


def dmm_state(l):
    return (l.q('FUNC?'), l.q('RES:RANG:AUTO?'), l.q('RES:RANG?'), l.q('RATE?'), trend(l), l.q('MEM:STAT:REC:AUTO?'))


def case_remember(name):
    """Mode Remember ON / OFF across a power-off by the key."""
    print(name, flush=True)
    on = name == 'remember'
    src = os.path.join(ROOT, 'stock/re/dumps/spi_dumps/dm40_msc_full_2026-10-13.bin')
    disk0 = os.path.join(OUT, '%s-%s-disk0.bin' % (TAG, name))
    disk1 = os.path.join(OUT, '%s-%s-disk1.bin' % (TAG, name))
    shutil.copy(src, disk0)
    extra = ['--run-for', str(A.run1), '--save-disk', disk1, '--key', 'power-hold@%g' % KEY_AT]
    emu = start('%s-%s-1' % (TAG, name), disk0, extra)
    try:
        l = Link()
        check(trend(l) == '320', 'first boot: trend 320 ms/div')
        l.w('MEM:STAT:REC:AUTO %s' % ('ON' if on else 'OFF'))
        l.w('CONF:RES')
        l.w('RES:RANG 600')
        l.w('RATE FAST')
        for want in ('640', '1280'):
            l.w('DBG:TAP 40,200')
            for _ in range(50):
                if trend(l) == want:
                    break
                time.sleep(0.2)
            check(trend(l) == want, 'tap on the timebase chip: %s ms/div' % want)
        st = dmm_state(l)
        print('  before: %s' % (st,), flush=True)
        check(st[0] == '"RES"' and st[1] == '0' and st[3] == 'FAST' and st[5] == ('1' if on else '0'),
              'run 1 state %s' % (st,))
        up = int(l.q('DBG:PING?').split(',')[0])
        check(up < (KEY_AT - 1.0) * 1000, 'changes made before the power key (uptime %d ms)' % up)
        l.close()
        emu.wait(timeout=3600)
    except Exception as e:
        check(False, '%s run 1: %r' % (name, e))
        stop(emu)
        return
    check(os.path.exists(disk1), 'disk saved at the end of run 1')
    if not os.path.exists(disk1):
        return
    emu = start('%s-%s-2' % (TAG, name), disk1, ['--run-for', '3000'])
    try:
        l = Link()
        s = settings(l.q('DBG:BOOT?'))
        print('  boot 2: %s' % ','.join(s), flush=True)
        check(s[1] == 'FILE', 'second boot: settings from the file')
        st = dmm_state(l)
        print('  after:  %s' % (st,), flush=True)
        if on:
            check(st[0] == '"RES"', 'Mode Remember ON: resistance (%s)' % st[0])
            check(st[1] == '0', 'Mode Remember ON: a fixed range (RES:RANG:AUTO? %s)' % st[1])
            check(abs(float(st[2]) - 600.0) < 1e-3, 'Mode Remember ON: 600 ohm (RES:RANG? %s)' % st[2])
            check(st[3] == 'FAST', 'Mode Remember ON: RATE FAST (%s)' % st[3])
            check(st[4] == '1280', 'Mode Remember ON: trend 1280 ms/div (%s)' % st[4])
            check(st[5] == '1', 'Mode Remember still ON')
        else:
            check(st[0] == '"VOLT"', 'Mode Remember OFF: DC V (%s)' % st[0])
            check(l.q('VOLT:RANG:AUTO?') == '1', 'Mode Remember OFF: autorange')
            check(st[3] == 'MED', 'Mode Remember OFF: RATE MED (%s)' % st[3])
            check(st[4] == '320', 'Mode Remember OFF: trend 320 ms/div (%s)' % st[4])
            check(st[5] == '0', 'Mode Remember still OFF')
        l.close()
    except Exception as e:
        check(False, '%s run 2: %r' % (name, e))
    finally:
        stop(emu)
    for f in (disk0, disk1):
        try:
            os.remove(f)
        except OSError:
            pass


def case(name):
    if name in ('remember', 'default'):
        return case_remember(name)
    print(name, flush=True)
    src = os.path.join(ROOT, 'stock/re/dumps/spi_dumps/dm40_msc_full_2026-10-13.bin')
    disk0 = os.path.join(OUT, '%s-%s-disk0.bin' % (TAG, name))
    disk1 = os.path.join(OUT, '%s-%s-disk1.bin' % (TAG, name))
    shutil.copy(src, disk0)
    want = {'timed': 'VOLUME', 'key': 'OFF'}[name]
    extra = ['--run-for', str(A.run1), '--save-disk', disk1]
    if name == 'key':
        extra += ['--key', 'power-hold@%g' % KEY_AT]
    emu = start('%s-%s-1' % (TAG, name), disk0, extra)
    try:
        l = Link()
        b = l.q('DBG:BOOT?')
        s = settings(b)
        print('  boot 1: %s' % ','.join(s), flush=True)
        check(s[1] == 'DEFAULTS' and 'CREATED' in s, 'first boot: defaults, the file made')
        before = l.q('SYST:ALER:ALAR?')
        check(before != want, 'setting before: %s' % before)
        if name == 'timed':
            l.w('SYST:ALER:ALAR %s' % want)
            t0 = time.time()
            ok = False
            for _ in range(120):
                s = settings(l.q('DBG:BOOT?'))
                if s[s.index('SAVE') + 1] == 'OK':
                    ok = True
                    break
                time.sleep(1)
            up = l.q('DBG:PING?').split(',')[0]
            check(ok, 'DBG:BOOT? SAVE,OK after the change (%s; %.0f s wall, uptime %s ms)' % (','.join(s), time.time() - t0, up))
        else:
            while int(l.q('DBG:PING?').split(',')[0]) < (KEY_AT - 1.0) * 1000:
                time.sleep(0.1)
            # (The defaults of the first boot were saved ~5 s after it.)
            n0 = settings(l.q('DBG:BOOT?'))
            l.w('SYST:ALER:ALAR %s' % want)
            s = settings(l.q('DBG:BOOT?'))
            up = l.q('DBG:PING?').split(',')[0]
            check(s[s.index('SAVE') + 2] == n0[n0.index('SAVE') + 2],
                  'no save of the change before the key (%s, uptime %s ms)' % (','.join(s), up))
        check(l.q('SYST:ALER:ALAR?') == want, 'setting now %s' % want)
        l.close()
        # The run's end saves the SPI flash (--save-disk).
        emu.wait(timeout=3600)
    except Exception as e:
        check(False, '%s run 1: %r' % (name, e))
        stop(emu)
        return
    check(os.path.exists(disk1), 'disk saved at the end of run 1')
    if not os.path.exists(disk1):
        return
    emu = start('%s-%s-2' % (TAG, name), disk1, ['--run-for', '3000'])
    try:
        l = Link()
        b = l.q('DBG:BOOT?')
        s = settings(b)
        print('  boot 2: %s' % ','.join(s), flush=True)
        check(s[1] == 'FILE' and 'PRESENT' in s, 'second boot: settings from the file')
        got = l.q('SYST:ALER:ALAR?')
        check(got == want, 'second boot: SYST:ALER:ALAR? %s (want %s)' % (got, want))
        l.close()
    except Exception as e:
        check(False, '%s run 2: %r' % (name, e))
    finally:
        stop(emu)
    for f in (disk0, disk1):
        try:
            os.remove(f)
        except OSError:
            pass


os.makedirs(OUT, exist_ok=True)
for c in A.cases.split(','):
    case(c)
print('FAIL: %d checks' % len(FAIL) if FAIL else 'PASS')
sys.exit(1 if FAIL else 0)
