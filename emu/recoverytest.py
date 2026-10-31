#!/usr/bin/env python3
"""opendm40 boot decisions on the native image (src/boot.rs shim, src/recovery.rs)
through the whole boot chain: ROM -> FS-loader -> opendm40, the real vendor
FS-loader included (run.py chain mode).

  cd firmware && ./build.sh
  emu/recoverytest.py --app firmware/target/native/thumbv7em-none-eabihf/release/opendm40

The emulator models what the shim decides on (emu/peripherals/FWDGT.cs):
the free watchdog resets the machine ~20 s after the last reload, RCU_RSTSCK
keeps BOR|PIN|POR after power-on, FWDGT|PIN after a watchdog reset, SW|PIN
after a SYSRESETREQ, until RSTFC; the backup SRAM and the flash keep their
content over a reset. The power key is PD6 (pressed = 1); "from off" = the
key pressed at t = 0, when the emulated unit powers up. A hang is injected by
setting the PC to opendm40's `wait_for_watchdog` (what a panic or DBG:RELEASE
leaves: nothing feeds the watchdog any more; the shim sees the same FWDGT
reset). Case `release` sends the real DBG:RELEASE over USBTMC (USB/IP).

Repeated hangs never enter the loader (owner 2026-10-29). The key hold and
DBG:LOADER enter it through PA2 (boot::loader_pa2: PA2 output low + GPIOA
lock, jump to the ROM's reset handler); the marker stays valid, so a power
cycle boots opendm40 again. The emulator's GPIO model enforces the GPIOA
lock until a machine reset (peripherals/GD32_GPIO.cs); a power cycle is a
`machine Reset` (the flash and the backup SRAM keep their content).

Cases (virtual times; opendm40 is entered ~0.69 s after power-on):
  normal      no key: one boot, no recovery, marker valid
  key-short   key pressed at power-on, released at 0.9 s (a normal power-on)
  key-abort   key held to the beep (shim + 4 s), released in the 0.5 s
              confirm window: normal boot
  key-hold    key held from off for 7 s: request word, SYSRESETREQ, PA2
              entry, ROM -> FS-loader update screen with PA2 low and locked,
              marker valid; then a power cycle: opendm40 boots
  loader      the PA2 request word set + SYSRESETREQ (what DBG:LOADER does):
              as key-hold
  wdt-one     one hang: watchdog reset, opendm40 boots again (crash count 1)
  wdt-two     a second hang ~10 s after the first watchdog reboot: a reboot
              again (crash count 2), no recovery
  wdt-three   three hangs ~10 s after each reboot: opendm40 keeps restarting
              (crash count 3), no recovery
  wdt-healthy the second hang after > 60 s of main loop: the crash counter
              was cleared at 60 s (crash count 1 again)
  request     the backup-SRAM request word (0x52454356) set, then a hang:
              recovery = loader marker zeroed, SYSRESETREQ, ROM -> FS-loader
              update screen, opendm40 not entered
  release     DBG:RELEASE over USB twice in a row: WDT_RETRY twice (crash
              count 1, then 2), no recovery
  usb-loader  DBG:LOADER over USB: as `loader`, from the real command

Checks per case: entries of the ROM / FS-loader / opendm40 reset handlers and of
recovery::run (cheap per-address hooks), then at the end the loader marker
(0x08008000), the backup-SRAM boot words (BKP_CRASH / BKP_TRIG / BKP_LAST)
and where the CPU is. Exit status 0 = PASS. Per-process names, private disk
copies; cases run in parallel (--jobs).
"""
import argparse
import glob
import os
import re
import shutil
import struct
import subprocess
import sys
import threading
import time
from concurrent.futures import ThreadPoolExecutor

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
OUT = os.path.join(HERE, 'out')
sys.path.insert(0, HERE)
import run  # noqa: E402
import bootinfo  # noqa: E402

CASES = ['normal', 'key-short', 'key-abort', 'key-hold', 'loader', 'wdt-one', 'wdt-two', 'wdt-three', 'wdt-healthy',
         'request']

ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
ap.add_argument('--app', required=True, help='opendm40 native ELF (symbols needed)')
ap.add_argument('--cases', default=','.join(CASES), help='comma list (default: all but the USB cases); add release / '
                                                         'usb-loader for DBG:RELEASE / DBG:LOADER over USB')
ap.add_argument('--jobs', type=int, default=3, help='emulators at the same time (default 3)')
A = ap.parse_args()

BKP_CRASH, BKP_TRIG, BKP_REQ, BKP_LAST = 0x40024FF0, 0x40024FF4, 0x40024FF8, 0x40024FFC
NRML, RCVR = 0x4C4D524E, 0x52564352
REQ_MAGIC = 0x52454356
LOADER_MAGIC = 0x4C324150
TRIG_PA2 = 0x54520012
GPIOA_IDR, GPIOA_LCKR = 0x40020010, 0x4002001C
AIRCR = 0xE000ED0C
CRASH0, CRASH1, CRASH2, CRASH3 = 0x43520000, 0x43520001, 0x43520002, 0x43520003
TRIG_ZEROED = 0x54520010
MARKER = run.MARKER
LOADER_LO, LOADER_HI = 0x080A0000, 0x080C0000
APP_LO = 0x08004000
TAG = 'RCVT'


def nm(elf):
    tools = glob.glob(os.path.join(ROOT, 'toolchain/rustup/toolchains/*/lib/rustlib/*/bin/llvm-nm'))
    if not tools:
        sys.exit('no llvm-nm under toolchain/')
    out = subprocess.check_output([tools[0], '-C', elf], text=True)
    syms = {}
    for l in out.splitlines():
        p = l.split(None, 2)
        if len(p) == 3:
            syms[p[2]] = int(p[0], 16)
    return syms


def word(path, off):
    with open(path, 'rb') as f:
        f.seek(off)
        return struct.unpack('<I', f.read(4))[0]


SYMS = nm(A.app)
HANG = SYMS['opendm40::wait_for_watchdog']
RECOVERY = SYMS['opendm40::recovery::run']
START = SYMS['opendm40::native::start']  # after the shim's decision
HELD_FOR = SYMS['opendm40::boot::held_for']  # the shim's key-hold windows (4 s, then 0.5 s)
PA2 = SYMS['opendm40::boot::loader_pa2']
with open(A.app, 'rb') as f:
    ELF_ENTRY = struct.unpack_from('<I', f.read(0x40), 0x18)[0] & ~1
ROM_RESET = word(run.DUMP, 4) & ~1
LOADER_RESET = word(run.DUMP, 0xA0004) & ~1
HOOKS = [('rom', ROM_RESET), ('loader', LOADER_RESET), ('app', ELF_ENTRY), ('recovery', RECOVERY), ('start', START),
         ('held_for', HELD_FOR), ('pa2', PA2)]
PRINT_LOCK = threading.Lock()


def say(*a):
    with PRINT_LOCK:
        print(*a, flush=True)


class Case:
    """One emulator run: timed events, read-outs (`read`, in order) and the
    reset-handler hook log."""

    def __init__(self, name):
        self.name = name
        self.lines = []
        self.reads = []
        self.fails = []
        self.notes = []
        for n, a in HOOKS:
            self.at(0, 'sysbus.cpu AddHook 0x%08X "self.Log(LogLevel.Warning, \'%s %s t=%%s\' %% '
                       'machine.ElapsedVirtualTime.TimeElapsed)"' % (a, TAG, n))

    def at(self, t, cmd):
        self.lines.append('%.3f monitor %s' % (t, cmd))

    def key(self, t, down):
        self.at(t, 'sysbus.gpioPortD OnGPIO 6 %s' % ('true' if down else 'false'))

    def hang(self, t):
        self.at(t, 'sysbus.cpu PC 0x%08X' % HANG)

    def read(self, t, label, addr=None):
        """addr None = the PC."""
        self.at(t, 'sysbus.cpu PC' if addr is None else 'sysbus ReadDoubleWord 0x%08X' % addr)
        self.reads.append(label)

    def end_reads(self, t, pre=''):
        for i in range(3):
            self.read(t, pre + 'marker%d' % i, MARKER + 4 * i)
        self.read(t, pre + 'crash', BKP_CRASH)
        self.read(t, pre + 'trig', BKP_TRIG)
        self.read(t, pre + 'last', BKP_LAST)
        self.read(t, pre + 'idr', GPIOA_IDR)
        self.read(t, pre + 'lckr', GPIOA_LCKR)
        self.read(t, pre + 'pc')

    def shot(self, t, png):
        self.at(t, 'sysbus.lcd Screenshot @%s' % os.path.abspath(png))

    def run(self, run_for, extra=()):
        tag = 'rcv-%s-%d' % (self.name, os.getpid())
        self.tag = tag
        script = os.path.join(OUT, tag + '.script')
        with open(script, 'w') as f:
            f.write('\n'.join(self.lines) + '\n')
        disk = os.path.join(OUT, tag + '-disk.bin')
        src = os.path.join(OUT, 'disk.bin')
        shutil.copy(src if os.path.exists(src) else run.DISK_SRC, disk)
        self.log = os.path.join(OUT, tag + '.log')
        self.png = os.path.join(OUT, tag + '.png')
        cmd = [sys.executable, os.path.join(HERE, 'run.py'), 'chain', '--app', A.app, '--name', tag,
               '--script', script, '--disk', disk, '--run-for', '%.3f' % run_for, '--shot', self.png] + list(extra)
        t0 = time.time()
        p = subprocess.run(cmd, cwd=ROOT, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)
        with open(os.path.join(OUT, tag + '.out'), 'w') as f:
            f.write(p.stdout)
        vals = [int(l.split()[0], 16) for l in p.stdout.splitlines() if re.match(r'^0x[0-9A-Fa-f]+\s*$', l)]
        if p.returncode != 0 or len(vals) != len(self.reads):
            self.fail('emulator run: rc %d, %d of %d read-outs (%s)' % (p.returncode, len(vals), len(self.reads),
                                                                     os.path.relpath(self.log, ROOT)))
            self.v = {}
        else:
            self.v = dict(zip(self.reads, vals))
        self.ev = []
        if os.path.exists(self.log):
            for l in open(self.log, errors='replace'):
                m = re.search(TAG + r' (\w+) t=(\d+):(\d+):([\d.]+)', l)
                if m:
                    self.ev.append((m.group(1), int(m.group(2)) * 3600 + int(m.group(3)) * 60 + float(m.group(4))))
        self.wall = time.time() - t0
        os.remove(disk)

    def fail(self, what):
        self.fails.append(what)

    def check(self, ok, what):
        (self.notes if ok else self.fails).append(what)

    def times(self, name):
        return [t for n, t in self.ev if n == name]

    # ---- the end states ----
    def marker_valid(self, pre=''):
        m = [self.v.get(pre + 'marker%d' % i) for i in range(3)]
        return None not in m and m != [0, 0, 0] and m != [0xFFFFFFFF] * 3

    def expect_normal(self, boots, crash):
        """opendm40 running after `boots` entries, no recovery."""
        if not self.v:
            return
        self.check(len(self.times('app')) == boots, 'opendm40 entered %d time(s) (%s)' % (boots, self.fmt_ev()))
        self.check(not self.times('recovery'), 'recovery::run not entered')
        self.check(self.marker_valid(), 'loader marker valid')
        self.check(self.v['last'] == NRML, 'BKP_LAST = NRML (0x%08X)' % self.v['last'])
        self.check(self.v['crash'] in crash, 'BKP_CRASH 0x%08X in %s' % (self.v['crash'], ['0x%08X' % c for c in crash]))
        pc = self.v['pc']
        self.check(APP_LO <= pc < LOADER_LO or 0x10000000 <= pc < 0x20030000,
                   'CPU in opendm40 at the end (PC 0x%08X)' % pc)
        scr = self.update_screen()
        if scr is not None:
            self.check(not scr, 'no FS-loader update screen')

    def expect_recovery(self, t_lo, t_hi):
        """recovery::run in [t_lo, t_hi], then ROM -> FS-loader with the
        marker zeroed, opendm40 not entered again."""
        if not self.v:
            return
        rec = self.times('recovery')
        self.check(len(rec) == 1 and t_lo <= rec[0] <= t_hi,
                   'recovery::run once, in %.1f..%.1f s (%s)' % (t_lo, t_hi, self.fmt_ev()))
        if rec:
            after = [(n, t) for n, t in self.ev if t >= rec[0] and n != 'recovery']
            self.check([n for n, _ in after] == ['rom', 'loader'],
                       'after it: ROM -> FS-loader only (%s)' % after)
        self.check([self.v['marker%d' % i] for i in range(3)] == [0, 0, 0], 'loader marker zeroed')
        self.check(self.v['trig'] == TRIG_ZEROED, 'BKP_TRIG = 0x%08X (marker zeroed)' % self.v['trig'])
        self.check(self.v['last'] == RCVR, 'BKP_LAST = RCVR (0x%08X)' % self.v['last'])
        self.check(self.v['crash'] == CRASH0, 'BKP_CRASH cleared on recovery (0x%08X)' % self.v['crash'])
        self.check(LOADER_LO <= self.v['pc'] < LOADER_HI, 'CPU in the FS-loader at the end (PC 0x%08X)' % self.v['pc'])
        scr = self.update_screen()
        if scr is not None:
            self.check(scr, 'FS-loader update screen ("Update" button) in %s' % os.path.relpath(self.png, ROOT))

    def expect_pa2_loader(self, t_lo, t_hi, png, t_end, pre='a_'):
        """PA2 entry in [t_lo, t_hi]: opendm40's shim -> loader_pa2 -> ROM ->
        FS-loader, its update screen, PA2 low and locked, marker valid; read
        out (`pre`) before t_end."""
        if not self.v:
            return
        pa2 = [t for t in self.times('pa2') if t_lo <= t <= t_hi]
        self.check(len(pa2) == 1, 'loader_pa2 once, in %.1f..%.1f s (%s)' % (t_lo, t_hi, self.fmt_ev()))
        if pa2:
            after = [n for n, t in self.ev if pa2[0] <= t < t_end and n != 'pa2']
            self.check(after[:2] == ['rom', 'loader'] and 'app' not in after,
                       'after it: ROM -> FS-loader, opendm40 not entered (%s)' % after)
        self.check(not self.times('recovery'), 'recovery::run not entered')
        self.check(self.marker_valid(pre), 'loader marker still valid')
        self.check(self.v[pre + 'trig'] == TRIG_PA2, 'BKP_TRIG = 0x%08X (PA2 entry, lock confirmed)' % self.v[pre + 'trig'])
        self.check(self.v[pre + 'last'] == RCVR, 'BKP_LAST = RCVR (0x%08X)' % self.v[pre + 'last'])
        self.check(self.v[pre + 'lckr'] == 0x10004, 'GPIOA_LCKR = 0x%08X (PA2 locked)' % self.v[pre + 'lckr'])
        self.check(self.v[pre + 'idr'] & 4 == 0, 'PA2 low in the loader (IDR 0x%04X)' % self.v[pre + 'idr'])
        self.check(LOADER_LO <= self.v[pre + 'pc'] < LOADER_HI,
                   'CPU in the FS-loader (PC 0x%08X)' % self.v[pre + 'pc'])
        scr = self.update_screen(png)
        if scr is not None:
            self.check(scr, 'FS-loader update screen ("Update" button) in %s' % os.path.relpath(png, ROOT))

    def expect_power_cycle(self, t_cycle):
        """After the power cycle at t_cycle: ROM -> opendm40, lock gone."""
        if not self.v:
            return
        after = [n for n, t in self.ev if t >= t_cycle]
        self.check(after[:2] == ['rom', 'app'] and 'loader' not in after,
                   'power cycle: ROM -> opendm40 (%s)' % after)
        self.check(self.marker_valid(), 'loader marker valid after the power cycle')
        self.check(self.v['lckr'] == 0, 'GPIOA_LCKR cleared by the reset (0x%08X)' % self.v['lckr'])
        self.check(self.v['last'] == NRML, 'BKP_LAST = NRML (0x%08X)' % self.v['last'])
        pc = self.v['pc']
        self.check(APP_LO <= pc < LOADER_LO or 0x10000000 <= pc < 0x20030000,
                   'CPU in opendm40 at the end (PC 0x%08X)' % pc)
        scr = self.update_screen()
        if scr is not None:
            self.check(not scr, 'no FS-loader update screen at the end')

    def update_screen(self, png=None):
        """The FS-loader's update screen in a screenshot (default: the end
        one): its blue "Update" button (0x0082CE) at 165..315 x 210..270
        (None: no PIL / PNG)."""
        try:
            from PIL import Image
            im = Image.open(png or self.png).convert('RGB')
        except Exception:
            return None
        n = sum(im.getpixel((x, y)) == (0, 130, 206) for x in range(170, 311, 4) for y in range(214, 267, 4))
        return n > 0.9 * len(range(170, 311, 4)) * len(range(214, 267, 4))

    def fmt_ev(self):
        return ' '.join('%s@%.2f' % e for e in self.ev)


def app_time(c, n):
    t = c.times('app')
    return t[n] if len(t) > n else None


def case_normal():
    c = Case('normal')
    c.end_reads(6.0)
    c.run(6.2)
    c.expect_normal(1, (0, CRASH0))
    return c


def case_key_short():
    c = Case('key-short')
    c.key(0.0, True)
    c.key(0.9, False)
    c.end_reads(6.0)
    c.run(6.2)
    c.expect_normal(1, (0, CRASH0))
    return c


def case_key_abort():
    # opendm40 at ~0.69 s; the shim's 4 s window ends ~4.7 s, the beep ~5.0 s,
    # the confirm window ~5.0..5.5 s: release at 5.2.
    c = Case('key-abort')
    c.key(0.0, True)
    c.key(5.2, False)
    c.end_reads(9.0)
    c.run(9.2)
    c.expect_normal(1, (0, CRASH0))
    h = c.times('held_for')
    c.check(len(h) == 2 and h[1] < 5.2, 'held to the cue, released in the 0.5 s confirm window (key windows at %s)'
            % ['%.2f' % t for t in h])
    c.check(not c.times('pa2'), 'no PA2 entry')
    return c


def mid_png(c):
    return os.path.join(OUT, 'rcv-%s-%d-loader.png' % (c.name, os.getpid()))


def case_key_hold():
    c = Case('key-hold')
    png = mid_png(c)
    c.key(0.0, True)
    c.key(7.0, False)
    c.end_reads(12.0, 'a_')
    c.shot(12.0, png)
    c.at(13.0, 'machine Reset')  # power cycle
    c.end_reads(19.0)
    c.run(19.2)
    a = app_time(c, 0) or 0.69
    # 4 s hold + ~0.3 s beep + 0.5 s confirm after the shim started, then a
    # SYSRESETREQ and the next shim pass.
    c.check(len(c.times('held_for')) == 2, 'both key windows (4 s, then the 0.5 s confirm) ran')
    c.expect_pa2_loader(a + 4.7, a + 6.0, png, 13.0)
    c.expect_power_cycle(13.0)
    return c


def case_loader():
    c = Case('loader')
    png = mid_png(c)
    c.at(6.0, 'sysbus WriteDoubleWord 0x%08X 0x%08X' % (BKP_REQ, LOADER_MAGIC))
    c.at(6.05, 'sysbus WriteDoubleWord 0x%08X 0x05FA0004' % AIRCR)  # SYSRESETREQ
    c.end_reads(13.0, 'a_')
    c.shot(13.0, png)
    c.at(14.0, 'machine Reset')  # power cycle
    c.end_reads(20.0)
    c.run(20.2)
    c.expect_pa2_loader(6.0, 8.0, png, 14.0)
    if c.v:
        c.check(c.v['a_crash'] in (0, CRASH0), 'crash counter not counted (0x%08X)' % c.v['a_crash'])
    c.expect_power_cycle(14.0)
    return c


def case_wdt_one():
    c = Case('wdt-one')
    c.hang(6.0)
    c.end_reads(34.0)
    c.run(34.2)
    c.expect_normal(2, (CRASH1,))
    t = c.times('app')
    if len(t) == 2:
        c.check(25.5 <= t[1] <= 27.5, 'watchdog reboot ~20 s after the hang (opendm40 again at %.2f s)' % t[1])
    return c


def case_wdt_two():
    c = Case('wdt-two')
    c.hang(6.0)          # reset ~26 s, opendm40 ~26.2 s
    c.hang(36.0)         # ~10 s after the reboot -> reset ~56 s
    c.end_reads(62.0)
    c.run(62.2)
    c.expect_normal(3, (CRASH2,))
    return c


def case_wdt_three():
    c = Case('wdt-three')
    c.hang(6.0)          # reset ~26 s
    c.hang(36.0)         # reset ~56 s
    c.hang(66.0)         # reset ~86 s
    c.end_reads(92.0)
    c.run(92.2)
    c.expect_normal(4, (CRASH3,))
    return c


def case_request():
    c = Case('request')
    c.at(6.0, 'sysbus WriteDoubleWord 0x%08X 0x%08X' % (BKP_REQ, REQ_MAGIC))
    c.hang(6.1)          # reset ~26 s: the shim consumes the request word
    c.end_reads(32.0)
    c.run(32.2)
    c.expect_recovery(25.5, 27.5)
    if c.v:
        c.check(c.v['crash'] == CRASH0, 'request: crash counter not counted (0x%08X)' % c.v['crash'])
    return c


def case_wdt_healthy():
    c = Case('wdt-healthy')
    c.hang(6.0)          # reset ~26 s; 60 s of main loop at ~88 s
    c.read(92.0, 'crash_before', BKP_CRASH)
    c.hang(92.5)         # reset ~112.5 s
    c.end_reads(118.0)
    c.run(118.2)
    if c.v:
        c.check(c.v['crash_before'] == CRASH0, 'crash count cleared after 60 s of main loop (0x%08X)'
                % c.v['crash_before'])
    c.expect_normal(3, (CRASH1,))
    return c


def case_release():
    """DBG:RELEASE over USBTMC (USB/IP), twice: 1st -> WDT_RETRY, crash 1;
    2nd (within 60 s of main loop) -> WDT_RETRY again, crash 2, no recovery."""
    import usbip_client as uc
    c = Case('release')
    port = run.free_port()
    os.environ['USBIP_PORT'] = str(port)
    uc.PORT = port
    end = 180.0
    c.end_reads(end - 0.1)
    res = {}

    def link():
        for _ in range(200):
            try:
                d = uc.Dev('1-0')
                return d, uc.Usbtmc(d)
            except Exception:
                time.sleep(1)
        raise IOError('no USB/IP device on port %d' % port)

    def q(t, s):
        t.write(s)
        return t.read(timeout=60).decode(errors='replace').strip()

    def after_reset(key, crash):
        """Re-import after the watchdog reboot: DBG:BOOT? must say WDT_RETRY
        with crash word `crash`."""
        for _ in range(120):
            d, t = link()
            try:
                b = q(t, 'DBG:BOOT?')
            except Exception:
                d.close()
                time.sleep(2)
                continue
            f = bootinfo.fields(b)
            if f.get('reason') == 'WDT_RETRY' and f.get('crash_word') == int(crash, 16):
                res[key] = b
                return d, t
            d.close()
            time.sleep(2)
        return None, None

    def host():
        try:
            d, t = link()
            res['boot1'] = q(t, 'DBG:BOOT?')
            t.write('DBG:RELEASE')
            d.close()
            time.sleep(5)
            d, t = after_reset('boot2', '0x43520001')
            if d:
                t.write('DBG:RELEASE')
                d.close()
                time.sleep(5)
                d, t = after_reset('boot3', '0x43520002')
                if d:
                    d.close()
        except Exception as e:
            res['err'] = repr(e)

    th = threading.Thread(target=host, daemon=True)
    th.start()
    c.run(end, ['--usbip', '0.01', '--usbip-port', str(port)])
    th.join(5)
    if 'err' in res:
        c.fail('USB host: %s' % res['err'])
    b1, b2, b3 = (bootinfo.fields(res.get(k, '')) for k in ('boot1', 'boot2', 'boot3'))

    def mrc(f):
        return '%s %s 0x%08X' % (f.get('mode'), f.get('reason'), f.get('crash_word', 0))
    c.check(b1.get('mode') == 'NORMAL' and b1.get('reason') == 'NONE', 'first boot NORMAL, NONE (%s)' % mrc(b1))
    c.check(b2.get('mode') == 'NORMAL' and b2.get('reason') == 'WDT_RETRY' and b2.get('crash_word') == 0x43520001,
            'after DBG:RELEASE: NORMAL, WDT_RETRY, crash word 0x43520001 (%s)' % mrc(b2))
    c.check(b3.get('mode') == 'NORMAL' and b3.get('reason') == 'WDT_RETRY' and b3.get('crash_word') == 0x43520002,
            'after the 2nd DBG:RELEASE: NORMAL, WDT_RETRY, crash word 0x43520002 (%s)' % mrc(b3))
    # The end read comes > 60 s after the last reboot: mark_healthy has
    # cleared the count (the 0x43520002 is checked in DBG:BOOT? above).
    c.expect_normal(3, (CRASH0,))
    return c


def case_usb_loader():
    """DBG:LOADER over USBTMC (USB/IP): PA2 entry, loader update screen; then
    a power cycle boots opendm40."""
    import usbip_client as uc
    c = Case('usb-loader')
    png = mid_png(c)
    port = run.free_port()
    os.environ['USBIP_PORT'] = str(port)
    uc.PORT = port
    res = {}

    def host():
        try:
            for _ in range(200):
                try:
                    d = uc.Dev('1-0')
                    t = uc.Usbtmc(d)
                    break
                except Exception:
                    time.sleep(1)
            else:
                raise IOError('no USB/IP device on port %d' % port)
            t.write('*IDN?')
            res['idn'] = t.read(timeout=60).decode(errors='replace').strip()
            t.write('DBG:LOADER')
            res['sent'] = True
            d.close()
        except Exception as e:
            res['err'] = repr(e)

    th = threading.Thread(target=host, daemon=True)
    th.start()
    # The command arrives some (virtual) seconds after the USB/IP attach; the
    # loader screen is read 30 s in, the power cycle at 31 s.
    c.end_reads(30.0, 'a_')
    c.shot(30.0, png)
    c.at(31.0, 'machine Reset')
    c.end_reads(37.0)
    c.run(37.2, ['--usbip', '0.01', '--usbip-port', str(port)])
    th.join(5)
    if 'err' in res:
        c.fail('USB host: %s' % res['err'])
    c.check(res.get('sent'), 'DBG:LOADER sent (*IDN? %s)' % res.get('idn', '')[:40])
    c.expect_pa2_loader(1.0, 29.0, png, 31.0)
    c.expect_power_cycle(31.0)
    return c


FUNCS = {'normal': case_normal, 'key-short': case_key_short, 'key-abort': case_key_abort,
         'key-hold': case_key_hold, 'loader': case_loader, 'usb-loader': case_usb_loader,
         'wdt-one': case_wdt_one, 'wdt-two': case_wdt_two, 'wdt-three': case_wdt_three,
         'wdt-healthy': case_wdt_healthy, 'request': case_request, 'release': case_release}


def one(name):
    say('%s: started' % name)
    try:
        c = FUNCS[name]()
    except Exception as e:
        c = Case(name)
        c.fail('exception %r' % e)
        c.ev, c.wall = [], 0
    with PRINT_LOCK:
        print('%s: %s (%.0f s wall; events %s)' % (name, 'PASS' if not c.fails else 'FAIL', c.wall, c.fmt_ev()))
        for n in c.notes:
            print('  ok   ' + n)
        for f in c.fails:
            print('  FAIL ' + f)
        sys.stdout.flush()
    return c


def main():
    os.makedirs(OUT, exist_ok=True)
    names = [n for n in A.cases.split(',') if n]
    for n in names:
        if n not in FUNCS:
            sys.exit('unknown case %s' % n)
    print('app %s: entry 0x%08X, wait_for_watchdog 0x%08X, recovery::run 0x%08X; ROM 0x%08X, FS-loader 0x%08X'
          % (os.path.relpath(A.app), ELF_ENTRY, HANG, RECOVERY, ROM_RESET, LOADER_RESET), flush=True)
    with ThreadPoolExecutor(max_workers=max(1, A.jobs)) as ex:
        res = list(ex.map(one, names))
    bad = [c.name for c in res if c.fails]
    print('recoverytest: %s' % ('PASS (%d cases)' % len(res) if not bad else 'FAIL: ' + ', '.join(bad)))
    return 1 if bad else 0


if __name__ == '__main__':
    sys.exit(main())
