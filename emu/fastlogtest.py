#!/usr/bin/env python3
"""opendm40 fast logger to the SPI disk (src/fastlog.rs, LOG:DEST DISK) in the
emulator, with the W25Q128 model's busy timing on (page program / erase
times, virtual time): the sustainable rate into the flash per rung.

  cd firmware && ./build.sh
  emu/fastlogtest.py --app firmware/target/native/thumbv7em-none-eabihf/release/opendm40

Per flash profile (--profiles: typ = tPP 0.4 ms, tSE 45 ms, tBE64 150 ms;
max = 3 ms, 400 ms, 2 s; datasheet W25Q128JV) and per session (--sessions:
AD2 rates in AC V, AD1 rates in DC V: 640 / 320 / 160 = OSR 0 / 1 / 2
unchopped (the stream ISR: must lose nothing), 80 / 40 = OSR 2 / 3 chopped
(the main-loop sampler); the INTF race applied to AD1F too):
  LOG:DEST DISK, LOG:DURation, LOG:STATe ON; LOG:DISK? polled until the
  session ends; the Prep time (DBG:PING? uptime), LOG:COUNt? (overwritten =
  the ring was full: the disk did not keep up), the file over MMEM:DATA?
  decoded by tools/dlog.py: rows, sum of lost_before, bytes per second,
  pages per second. A session "keeps up" when no sample is lost.
--region-c --host-read: no region C function runs and no data is read from
the slow flash bank (>= 0x08080000, emu/regionc.py data watch) during a
session while a host reads the INFO/ files (FIRMWARE.BIN first) off the USB
volume and polls DBG:PROF?, LOG:DEST?, LOG:DURation?; the files' slow-bank
sectors answer NOT READY 02/04/01 (and only those, from FIRMWARE.BIN's
sector 0x7C000), and after the session every file reads whole with the same
bytes.
--ui-nav: UI navigation by DBG:KEY / DBG:TAP while each session streams
(the main menu, Settings 1..3, Device, MENU, the Oscilloscope tile, its
source badge, MENU, the Multimeter tile; each landing checked with
DBG:VIEW?), each session started on the MCX scope page; before the sessions
the page tree with no stream (MENU on the Device page = the main menu). Real
touch / side keys cannot do this (any press stops a disk session, main.rs
`fast_touch`): it audits the page code a stream could meet. With
--region-c: none of it may run region C code or read the slow bank.
Every session must record ~rate x LOG:DURation rows.
Bluetooth (--bt-port passthrough): LOG:STATe ON refused for AD2 ("Rate too
high for Bluetooth") and with LOG:DEST DISK ("Command protected"); an AD1
40 S/s session drains over BT LOG:DATA?.

Timing: the disk's SPI bus has its byte time (GD32_SPI on spi2, 12.5 MHz:
~0.2 ms of polled CPU per 256-byte page, charged since 2026-10-22), the
chip's busy time is the profile's, the HY3131 bus and its DMA are timed as
on the device; the CPU runs at --mips (default 200 = one instruction per
cycle, no flash / APB wait states). The device's speed: --mips 110
--isr-stall-ns 700 (peripherals/CpuCost.cs; the 76.8 kS/s device check of
20a91d0: Run pass, passes/s and the stream ISR's cpu_permille).
--noise raises the AD2 noise (more bytes per sample, the coder's worst
case). Exit status 0 = every check passed; the rate table is printed
(docs/compact-log-format.md "Fast stream: rate limits"; fifo_hw = the
LOG:DISK? FIFO high water, bytes of 8192 coded but not yet programmed).
"""
import argparse
import os
import socket
import struct
import subprocess
import sys
import time

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
ap.add_argument('--app', required=True, help='opendm40 native ELF / image')
ap.add_argument('--port', type=int, default=4817, help='USB/IP port (default 4817)')
ap.add_argument('--bt-port', type=int, default=4818, help='BT passthrough port (default 4818)')
ap.add_argument('--dur', type=int, default=None, help='LOG:DURation per session, s (device time; default 2, '
                'with --ui-nav 4: a NAV round is 10 steps of >= 300 ms)')
ap.add_argument('--profiles', default='typ,max')
ap.add_argument('--sessions', default='ad1:640,ad2:4800,ad2:9600,ad2:19200,ad2:38400')
ap.add_argument('--extra-script', metavar='FILE', help='event-script lines (run.py --script) appended to the generated one, e.g. Renode hooks (opendm40 region C audit)')
ap.add_argument('--no-bt', action='store_true')
ap.add_argument('--ppm', type=float, default=0.0, help='HY3131 crystal offset vs the MCU, ppm (model ClockPpm)')
ap.add_argument('--osr32-ppm', type=float, default=None, help='the 76.8 kS/s setting\'s own rate offset, ppm (model Ad2Osr32Ppm; '
                'default 0 = the datasheet\'s 76 800, as the device (opendm40 device check of c830e09))')
ap.add_argument('--vol-read', action='store_true', help='after each session read LOGS/FASTnnn.CSV off the USB volume (MSC READ(10), 64 sectors each): == tools/dlog.py of the DLG; the virtual-time rate')
ap.add_argument('--no-ad1-race', action='store_true', help='AD1F without the INTF read-clears race (model IntfRaceAd1)')
ap.add_argument('--noise', type=float, default=None, help='HY3131 model noise scale (SetNoise; AD2: 30 codes rms x scale at OSR 32): a '
                'noisier signal codes to more bytes per sample (the model\'s 0.5 V sine + 1x noise is ~1.4 B; real '
                'inputs 1.9..2.9 B, docs/compact-log-format.md)')
ap.add_argument('--mips', type=int, default=200, help='CPU PerformanceInMips (all code; 200 = one instruction per cycle; '
                '110 = the device\'s speed, peripherals/CpuCost.cs)')
ap.add_argument('--isr-stall-ns', type=int, default=0, help='a stall per stream ISR entry, ns (peripherals/CpuCost.cs IsrStallNs; '
                'with --mips 110, 700 gives the device\'s cpu_permille ~520 at 76800)')
ap.add_argument('--expect-rung', type=int, default=None, help='a session above 38400 must run at this rung (76800: the writer '
                'trial kept it; 38400: it fell back), else either is accepted')
ap.add_argument('--stop-in-trial', action='store_true', help='first an AD2 76800 session stopped (LOG:STATe OFF) as soon as it '
                'runs, i.e. in the writer trial: it ends Done with no rows and no LOGS/FASTnnn left')
ap.add_argument('--start-at', type=float, default=0.0, help='LOG:STATe ON not before this device uptime, s (DBG:PING?): e.g. '
                '11 puts a 76800 writer trial in CYCCNT\'s upper half (10.7..21.5 s), where a stale 0 deadline reads as busy')
ap.add_argument('--prof', action='store_true', help='DBG:PROF:RESET when the run starts, DBG:PROF? at its end (printed)')
ap.add_argument('--region-c', action='store_true', help='Renode hooks on every function of region C (emu/regionc.py): none may '
                'run while the stream ISR owns the bus (hy3131::HANDED), i.e. during a session; with --host-read')
ap.add_argument('--host-read', action='store_true', help='while each session runs: a host reading the INFO/ files '
                '(FIRMWARE.BIN first; device 825080c: calbackup::FwFile::fill, region C, max_lat 4.66 us) off the USB '
                'volume (MSC READ(10), 64 sectors each) and DBG:PROF? (prof::report) between the polls')
ap.add_argument('--host-files', default='', help='with --host-read: only these INFO/ files, ","-separated, in this order '
                '(e.g. LOADER.BIN: all of it from the SPI-disk mirror; device c785807 M2c: ~22 000 lost per copy at 76800)')
ap.add_argument('--host-sectors', type=int, default=64, help='with --host-read: sectors per READ(10) (Linux cp: up to 240)')
ap.add_argument('--host-delay', type=float, default=0.0, help='with --host-read: start the READs this many device seconds after '
                'the run began (LOG:DISK? state 3), e.g. 3: after a 76800 writer trial (device c785807 M2c: LOADER.BIN 6 s '
                'after state 3 lost 21 745; inside the trial the session fell back to 38400)')
ap.add_argument('--host-only', action='store_true', help='with --host-read: no SCPI queries between the READs (back to back, '
                'as a host cp; LOG:DISK? still polled every 8 READs)')
ap.add_argument('--poll', default='', help='with --host-read: more queries sent at each step, ";"-separated (e.g. '
                '"SCOP:MEAS:VPP?;DBG:HWTRIG?": scope commands read the calibration record, slow bank)')
ap.add_argument('--ui-nav', action='store_true', help='UI navigation while each session streams (DBG:KEY / DBG:TAP, '
                'each landing checked with DBG:VIEW?): MENU, the Settings tile, More, More, Device, MENU (the main menu), '
                'the Oscilloscope tile (the MCX scope page), its source badge (the Wave view: refused while a session '
                'streams), MENU, the Multimeter tile; the next session starts on the scope page. Before the sessions, the '
                'same page tree with no stream (MENU on the Device page = the main menu). With --region-c: no region C '
                'code / slow-bank read on those paths')
ap.add_argument('--ui-nav-steps', type=int, default=None, help='with --ui-nav: at most this many NAV steps per session '
                '(0: only start the session on the scope page)')
A = ap.parse_args()
if A.dur is None:
    A.dur = 4 if A.ui_nav and A.ui_nav_steps is None else 2
RC = None
PROF = {'typ': (400, 45_000, 120_000, 150_000), 'max': (3000, 400_000, 1_600_000, 2_000_000)}
os.environ['USBIP_PORT'] = str(A.port)
sys.path.insert(0, HERE)
import usbip_client as uc  # noqa: E402

FAIL = []
T = []
# DBG:VDISK:MIR? at the start (opendm40 vdisk.rs `mirror_boot`): served,
# writer status, rewritten at this boot, ms. With the mirror served, the
# slow-bank sectors read during a session like any other (from the SPI
# disk); without it they answer NOT READY.
MIR = [0, 0, 0, 0]


def check(ok, what):
    print('  %s %s' % ('ok  ' if ok else 'FAIL', what), flush=True)
    if not ok:
        FAIL.append(what)


def run_emu(profile):
    p = PROF[profile]
    script = os.path.join(HERE, 'out', 'fastlog-%d.script' % A.port)
    with open(script, 'w') as f:
        f.write('0 monitor sysbus.spi2.spiflash ProgramUs %d\n' % p[0])
        f.write('0 monitor sysbus.spi2.spiflash Erase4kUs %d\n' % p[1])
        f.write('0 monitor sysbus.spi2.spiflash Erase32kUs %d\n' % p[2])
        f.write('0 monitor sysbus.spi2.spiflash Erase64kUs %d\n' % p[3])
        f.write('0 monitor sysbus.hy3131 ClockPpm %g\n' % A.ppm)
        if A.osr32_ppm is not None:
            f.write('0 monitor sysbus.hy3131 Ad2Osr32Ppm %g\n' % A.osr32_ppm)
        if A.isr_stall_ns:
            f.write('0 monitor sysbus.cpucost IsrStallNs %d\n' % A.isr_stall_ns)
        if A.noise is not None:
            f.write('0 monitor sysbus.hy3131 SetNoise %g\n' % A.noise)
        if not A.no_ad1_race:
            f.write('0 monitor sysbus.hy3131 IntfRaceAd1 true\n')
        if A.extra_script:
            f.write(open(A.extra_script).read())
        if RC:
            f.write(RC.lines())
    cmd = [sys.executable, os.path.join(HERE, 'run.py'), 'fwrs', '--mips', str(A.mips), '--app', A.app,
           '--usbip', '0.01', '--usbip-port', str(A.port), '--run-for', '5000', '--name', 'fastlog-%d' % A.port,
           '--dmm', 'vsrc:0.2,ac=0.5,f=50', '--script', script]
    if not A.no_bt:
        cmd += ['--bt-port', str(A.bt_port)]
    return subprocess.Popen(cmd, cwd=ROOT, stdout=subprocess.DEVNULL, stderr=subprocess.STDOUT, start_new_session=True)


def stop_emu(emu):
    try:
        os.killpg(emu.pid, 15)
    except Exception:
        pass
    emu.wait()


def connect():
    for _ in range(150):
        try:
            return uc.Usbtmc(uc.Dev('1-0'))
        except Exception:
            time.sleep(2)
    sys.exit('fastlogtest: no USB/IP device on port %d' % A.port)


def decode(dlg, tag):
    """tools/dlog.py of the bytes: (rows, sum lost_before, header ok)."""
    path = os.path.join(HERE, 'out', 'fastlog-%d-%s.dlg' % (A.port, tag))
    open(path, 'wb').write(dlg)
    r = subprocess.run([sys.executable, '-I', os.path.join(ROOT, 'firmware/tools/dlog.py'), path], capture_output=True, text=True)
    if r.returncode:
        return 0, 0, False
    rows = [l for l in r.stdout.splitlines() if l[:1] in ' 0123456789']
    return len(rows), sum(int(l.rsplit(',', 1)[1]) for l in rows), True


TABLE = []
VOL = []


def vol_read(q, n, tag):
    """LOGS/FASTnnn.CSV over the MSC interface (the generated volume,
    vdisk.rs): read as a host does, 64 sectors per READ(10); compared with
    tools/dlog.py of the DLG; KB per second of device (virtual) time."""
    ref = os.path.join(HERE, 'out', 'fastlog-%d-%s.dlg' % (A.port, tag))
    r = subprocess.run([sys.executable, '-I', os.path.join(ROOT, 'firmware/tools/dlog.py'), ref, '-o', ref + '.csv'], capture_output=True)
    want = open(ref + '.csv', 'rb').read() if r.returncode == 0 else None
    m = uc.Msc(T[0].d)
    name = b'FAST%03d CSV' % n
    for _ in range(40):  # the volume refresh: 1 s after the disk went quiet
        time.sleep(1)
        try:
            m.cmd(0, bytes(6))  # TEST UNIT READY (a media change: UNIT ATTENTION once)
            b = m.read(0, 0, 1, 512)
        except IOError:
            continue
        bps, spc, rsvd, nfat, nroot = struct.unpack_from('<HBHBH', b, 11)
        fatsz = struct.unpack_from('<H', b, 22)[0]
        root = rsvd + nfat * fatsz
        data = root + nroot * 32 // 512
        lba = lambda c: data + (c - 2) * spc
        def find(secs, nm):
            for s_ in secs:
                d = m.read(0, s_, 1, 512)
                for k in range(16):
                    e = d[k * 32:k * 32 + 32]
                    if e[0] == 0:
                        return None
                    if e[:11] == nm:
                        return struct.unpack_from('<H', e, 26)[0], struct.unpack_from('<I', e, 28)[0]
            return None
        logs = find(range(root, data), b'LOGS       ')
        try:
            f = logs and find(range(lba(logs[0]), lba(logs[0]) + spc), name)
        except IOError:
            f = None
        if f:
            break
    else:
        check(False, 'volume: %s not listed' % name.decode())
        return
    start, size = f
    # The generated files are contiguous (vdisk.rs).
    t0 = int(q('DBG:PING?').split(',')[0])
    got = b''
    w0 = time.time()
    while len(got) < size:
        k = min(64, (size - len(got) + 511) // 512)
        got += m.read(0, lba(start) + len(got) // 512, k, 512, retries=2)
    t1 = int(q('DBG:PING?').split(',')[0])
    got = got[:size]
    kbs = size / 1024 / max((t1 - t0) / 1000, 1e-3)
    VOL.append((tag, size, (t1 - t0) / 1000, kbs, time.time() - w0))
    check(want is not None and got == want, 'volume: %s (%d bytes) == dlog.py of the DLG' % (name.decode(), size))
    print('  volume read %s: %d bytes in %.2f s device time (%.0f KB/s), %.1f s wall' % (name.decode(), size, (t1 - t0) / 1000, kbs, time.time() - w0), flush=True)


import infovol  # noqa: E402
from infovol import SLOW_FILES, vol_dir  # noqa: E402


class HostRead:
    """A host reading the INFO/ files (FIRMWARE.BIN first) in 64-sector
    READ(10)s, round and round, plus DBG:PROF? and LOG:DEST? / LOG:DURation?
    (fast_scpi, device c92b605: region C) (`step` between the LOG:DISK?
    polls). A READ refused with NOT READY, BECOMING READY (the slow-bank
    sectors, vdisk.rs `read_busy`) is counted per file and the reader moves
    on to the next file; the first refusal of each file is narrowed down
    to its first refused sector. `verify` (after the session): every file
    reads whole, without a refusal, and the bytes read during the session
    are the same."""

    def __init__(self, q):
        self.q = q
        self.m = None
        self.files = []
        self.k = 0
        self.off = 0
        self.bytes = 0
        self.profs = 0
        self.has_prof = None
        self.seen = set()
        self.refused = {}   # name: NOT READY count
        self.first = {}     # name: file offset of its first refused sector
        self.other = []     # other errors
        self.got = {}       # (name, offset): bytes read during the session

    def _read(self, lba, n):
        """bytes, or None when refused NOT READY (else raises)."""
        return infovol.read(self.m, lba, n)

    def step(self):
        if self.m is None:
            self.m = uc.Msc(T[0].d)
            for _ in range(3):
                try:
                    d = vol_dir(self.m, 'INFO')
                    self.files = sorted(d.items(), key=lambda x: x[0] != 'FIRMWARE.BIN')
                    if A.host_files:
                        self.files = [(n, d[n]) for n in A.host_files.split(',')]
                    break
                except IOError:
                    time.sleep(0.5)
        if self.files:
            name, (lba, size) = self.files[self.k]
            n = min(A.host_sectors, (size - self.off + 511) // 512)
            ok = True
            if n > 0:
                try:
                    b = self._read(lba + self.off // 512, n)
                except IOError as e:
                    self.other.append('%s @%#x: %s' % (name, self.off, e))
                    b = b''
                if b is None:
                    ok = False
                    self.refused[name] = self.refused.get(name, 0) + 1
                    if name not in self.first:
                        for i in range(n):
                            if self._read(lba + self.off // 512 + i, 1) is None:
                                self.first[name] = self.off + i * 512
                                break
                else:
                    self.bytes += len(b)
                    self.got[(name, self.off)] = b
            self.seen.add(name)
            self.off += n * 512
            if not ok or self.off >= size:
                self.off = 0
                self.k = (self.k + 1) % len(self.files)
        if A.host_only:
            self.profs += 1
            return
        # DBG:PROF? only in a build with the feature dbg-prof: without it
        # the poll counts without the query.
        if self.has_prof is None:
            self.q('DBG:PROF?')
            self.has_prof = not self.q('SYST:ERR?').startswith('-113')
        if not self.has_prof or 'PROF' in self.q('DBG:PROF?'):
            self.profs += 1
        self.q('LOG:DEST?')
        self.q('LOG:DURation?')
        for c in filter(None, A.poll.split(';')):
            self.q(c)

    def verify(self, tag):
        """After the stream: every file whole, no refusal, same bytes."""
        whole = {}
        bad = []
        for name, (lba, size) in self.files:
            d = b''
            while len(d) < size:
                n = min(64, (size - len(d) + 511) // 512)
                b = self._read(lba + len(d) // 512, n)
                if b is None:
                    bad.append(name)
                    break
                d += b
            whole[name] = d
        check(not bad, '%s: after the session every INFO/ file reads without NOT READY (%s)' % (tag, ' '.join(bad) or 'all'))
        diff = [('%s@%#x' % k) for k, b in self.got.items() if whole.get(k[0], b'')[k[1]:k[1] + len(b)] != b]
        check(not diff, '%s: the INFO/ bytes read during the session == after it (%d reads%s)'
              % (tag, len(self.got), (', differ: ' + ' '.join(diff[:5])) if diff else ''))
        check(not self.other, '%s: no other READ error during the session (%s)' % (tag, '; '.join(self.other[:3]) or 'none'))
        if MIR[0]:
            # The mirror is served: no refusal, and every slow-bank sector
            # of each file was read during the session (from the SPI disk).
            step = A.host_sectors * 512
            cov = {n: all((n, o) in self.got for o in range(SLOW_FILES[n] // step * step, len(whole[n]), step))
                   for n in SLOW_FILES if n in whole}
            check(not self.refused and (cov or A.host_files) and all(cov.values()),
                  '%s: mirror served (DBG:VDISK:MIR? %s): no NOT READY (%s), every slow-bank sector read during the '
                  'session: %s' % (tag, ','.join(map(str, MIR)), sorted(self.refused.items()) or 'none',
                                   ' '.join('%s=%s' % (n, 'yes' if c else 'NO') for n, c in sorted(cov.items()))))
            return
        want = {n for n in SLOW_FILES if n in whole}
        check(set(self.refused) == want and all(self.first.get(n) == SLOW_FILES[n] for n in want),
              '%s: NOT READY for exactly the slow-bank sectors: refused %s, first refused offset %s (want %s)'
              % (tag, sorted(self.refused.items()), sorted((n, hex(o)) for n, o in self.first.items()),
                 sorted((n, hex(o)) for n, o in SLOW_FILES.items() if n in want)))


# UI navigation (--ui-nav): (command, the DBG:VIEW? page it must land on).
# Coordinates: ui.rs TILES (main menu), the settings rows (SET_Y0 44, pitch
# 30: row 8 "More" y 297, Settings 3/3 row 4 "Device" y 177, row 3 "Burden voltage" y 147, since the "A-lead
# reminder" row went), scope_ui.rs
# SRC_BADGE (the source badge, top bar).
NAV = [('DBG:KEY 0', 'Main menu'), ('DBG:TAP 341,221', 'Settings,1'), ('DBG:TAP 240,297', 'Settings,2'),
       ('DBG:TAP 240,297', 'Settings,3'), ('DBG:TAP 240,177', 'Device'), ('DBG:KEY 0', 'Main menu'),
       ('DBG:TAP 341,107', 'Oscilloscope'), ('DBG:TAP 177,10', 'Oscilloscope'), ('DBG:KEY 0', 'Main menu'),
       ('DBG:TAP 140,107', 'Multimeter')]


def wait_dev(q, ms):
    t0 = int(q('DBG:PING?').split(',')[0])
    while int(q('DBG:PING?').split(',')[0]) - t0 < ms:
        time.sleep(0.05)


def nav_to(q, steps, what):
    """Run `steps` (NAV entries) with ~300 ms (device) between them; every
    DBG:VIEW? must start with the expected page."""
    bad = []
    for c, want in steps:
        q(c)
        wait_dev(q, 300)
        v = q('DBG:VIEW?')
        if not v.startswith(want):
            bad.append('%s -> %s (want %s)' % (c, v, want))
    check(not bad, '%s: every step landed (%s)' % (what, '; '.join(bad) or ' / '.join(w for _, w in steps)))


def nav_tree(q):
    """No stream: the page tree; MENU on the Device page = the main menu
    (device check 519fb8a: it returned to Settings 3/3), a tap on Device =
    Settings 3/3, MENU from the menu = back to the instrument (DMM)."""
    nav_to(q, NAV[:6], 'page tree, MENU on Device -> the main menu')
    nav_to(q, NAV[1:5] + [('DBG:TAP 240,177', 'Settings,3'), ('DBG:KEY 0', 'Settings,2'), ('DBG:KEY 0', 'Settings,1'),
                          ('DBG:KEY 0', 'Main menu'), ('DBG:KEY 0', 'Multimeter')], 'a tap on Device -> Settings 3/3, MENU back up')


class UiNav:
    """--ui-nav while a session streams: one NAV step per LOG:DISK? poll
    (round and round); the landings are checked (`done`)."""

    def __init__(self, q):
        self.q = q
        self.k = 0
        self.n = 0
        self.bad = []

    def step(self):
        c, want = NAV[self.k]
        self.q(c)
        wait_dev(self.q, 300)
        v = self.q('DBG:VIEW?')
        # The source badge in a session: the Wave view is refused (no
        # stream of its own while the logger owns the HY3131).
        if not v.startswith(want) or (c == 'DBG:TAP 177,10' and v.split(',')[2:3] != ['0']):
            self.bad.append('%s -> %s (want %s)' % (c, v, want))
        self.k = (self.k + 1) % len(NAV)
        self.n += 1

    def done(self, tag):
        check(self.n >= min(len(NAV), A.ui_nav_steps or len(NAV)) and not self.bad, '%s: UI navigation while streaming, %d steps landed (%s)'
              % (tag, self.n, '; '.join(self.bad[:4]) or 'all'))


def session(q, src, rate, profile):
    q('LOG:STATe OFF')
    q('*CLS')
    if src == 'ad2':
        q('CONF:VOLT:AC')
        q('LOG:SOURce AD2')
        q('LOG:SRATe %d' % rate)
        q('LOG:CHOP ON')
    else:
        q('CONF:VOLT:DC')
        q('LOG:SOURce AD1')
        # 640 / 320 / 160: OSR 256 / 512 / 1024 unchopped (the stream ISR);
        # 80 / 40: OSR 1024 / 2048 chopped (the main-loop sampler).
        osr, chop = {640: (0, 'OFF'), 320: (1, 'OFF'), 160: (2, 'OFF'), 80: (2, 'ON'), 40: (3, 'ON')}[rate]
        q('LOG:OSR %d' % osr)
        q('LOG:CHOP %s' % chop)
    time.sleep(3)
    if A.ui_nav:
        # The session starts on the MCX scope page (the scope source open).
        for _ in range(5):
            if q('DBG:VIEW?').startswith('Main menu'):
                break
            q('DBG:KEY 0')
            wait_dev(q, 300)
        nav_to(q, [('DBG:TAP 341,107', 'Oscilloscope')], '%s %d: the scope page before the session' % (src, rate))
    q('LOG:DEST DISK')
    q('LOG:DURation %d' % A.dur)
    while int(q('DBG:PING?').split(',')[0]) < A.start_at * 1000:
        time.sleep(0.2)
    up0 = int(q('DBG:PING?').split(',')[0])
    q('LOG:STATe ON')
    err = q('SYST:ERR?')
    check(err.startswith('0,'), '%s %d: LOG:STATe ON accepted (%s)' % (src, rate, err))
    t_run = None
    st = []
    hr = HostRead(q) if A.host_read else None
    un = UiNav(q) if A.ui_nav else None
    if RC:
        RC.reset()
    for _ in range(2000):
        st = [int(x) for x in q('LOG:DISK?').split(',')]
        if st[0] == 3 and t_run is None:
            t_run = int(q('DBG:PING?').split(',')[0])
            if A.prof:
                q('DBG:PROF:RESET')
        if st[0] >= 5:
            break
        nav = un and (A.ui_nav_steps is None or un.n < A.ui_nav_steps)
        if hr and A.host_delay and st[0] == 3 and int(q('DBG:PING?').split(',')[0]) < t_run + A.host_delay * 1000:
            time.sleep(0.2)
            continue
        if (hr or nav) and st[0] == 3:
            if hr:
                for _ in range(8 if A.host_only else 1):
                    hr.step()
            if nav:
                un.step()
            continue
        time.sleep(0.5)
    if hr:
        check(('FIRMWARE.BIN' in hr.seen or A.host_files) and hr.bytes > 0 and hr.profs > 0,
              '%s %d: a host read %d bytes of INFO/ (%s) and DBG:PROF? %d times during the run'
              % (src, rate, hr.bytes, ' '.join(sorted(hr.seen)), hr.profs))
    if un and A.ui_nav_steps != 0:
        un.done('%s %d' % (src, rate))
    if RC:
        got = RC.calls()
        print('  region C functions / slow-bank data reads while streaming: %s' % (', '.join(sorted(got)) or 'none'), flush=True)
        check(not got, '%s %d: no region C code and no slow-bank data read while the stream ISR owns the bus%s'
              % (src, rate, '' if not got else ' (%s)' % ', '.join(sorted(got))))
    if hr:
        while int(q('LOG:DISK?').split(',')[0]) < 5:
            time.sleep(0.5)
        hr.verify('%s %d' % (src, rate))
        print('  DBG:VDISK:MIR? %s (served, status, rewritten, ms; mirror sectors read while streaming, held, longest hold ms)'
              % q('DBG:VDISK:MIR?'), flush=True)
    up1 = int(q('DBG:PING?').split(',')[0])
    if A.prof:
        print(q('DBG:PROF?'), flush=True)
    cnt = [int(x) for x in q('LOG:COUNt?').split(',')]
    isr = q('DBG:ISR?').split(',')
    print('  LOG:COUNt? %s  DBG:ISR? %s' % (','.join(map(str, cnt)), ','.join(isr)), flush=True)
    if src == 'ad1' and rate >= 160:
        check(isr[1] == '1' or isr[2] != '0', '%s %d: AD1 on the stream ISR (DBG:ISR? ticks %s)' % (src, rate, isr[2]))
    pre = q('LOG:PREamble?').split(',')
    nominal = int(pre[8]) / 1000 if len(pre) > 8 else rate
    check(st[0] in (5, 6), '%s %d: the session ended (LOG:DISK? %s)' % (src, rate, st))
    name = 'LOGS/FAST%03d.DLG' % st[1]
    dlg = b''
    while True:
        b = q('MMEM:DATA? "%s",%d' % (name, len(dlg)), raw=True)
        n = int(b[2:6])
        dlg += bytes(b[6:6 + n])
        if n < 6000:
            break
    if len(st) > 13:
        # LOG:DISK? field 14 (opendm40 fastlog.rs `writer_permille`): the Run
        # passes' share of the main loop's CPU, the stream ISR out.
        print('  writer: %d permille of the main loop\'s CPU (headroom %d)' % (st[13], 1000 - st[13]), flush=True)
    rows, lost, ok = decode(dlg, '%s-%s-%d' % (profile, src, rate))
    if A.vol_read:
        vol_read(q, st[1], '%s-%s-%d' % (profile, src, rate))
    check(ok and rows == st[4] and len(dlg) == st[3], '%s %d: %s decodes: %d rows, %d bytes' % (src, rate, name, rows, len(dlg)))
    # The whole session recorded (a session started on the Scope page, DMM
    # rail off, ran with 0 rows and passed every other check).
    check(rows >= 0.9 * nominal * A.dur, '%s %d: rows %d for %d s at %.0f S/s' % (src, rate, rows, A.dur, nominal))
    prep_s = ((t_run or up1) - up0) / 1000
    run_s = (up1 - (t_run or up1)) / 1000
    span = rows / nominal if nominal else 0
    bps = len(dlg) / max(span, 1e-9)
    hw = st[6] if len(st) > 6 else -1  # FIFO high water (bytes of 8192 not yet programmed)
    if src == 'ad2' and rate > 38400:
        # The page-program probe (decision 2026-10-22 #9): 76800 to the disk
        # only when the chip programs a page in <= 800 us mean / 1.5 ms max,
        # else the session runs at 38400; then the writer trial (fastlog.rs
        # `Gate`, LOG:DISK? fields 11..13: 1 kept / 2 fell back, the
        # window's ISR permille, ring high water): 38400 when the CPU is
        # short. Either way nothing is lost.
        tpp, tpp_max = (st[8], st[9]) if len(st) > 9 else (0, 0)
        gate, g_pm, g_fill = (st[10], st[11], st[12]) if len(st) > 12 else (0, 0, 0)
        fast = 0 < tpp <= 800 and tpp_max <= 1500
        want = rate if fast and gate == 1 else 38400
        print('  writer trial: %s, ISR %d permille, ring high water %d' % ({0: 'none', 1: 'kept', 2: 'fell back'}.get(gate, gate), g_pm, g_fill), flush=True)
        # nominal = the rung's rate (opendm40 hy3131.rs AD2_OSR32_REAL_MHZ at
        # 76800): which rung ran, to 0.4 %.
        check(abs(nominal / want - 1) < 0.004 and (gate in (1, 2) if fast else gate == 0),
              '%s %s %d: probe tPP %d / %d us, trial %d -> rung %.0f S/s' % (profile, src, rate, tpp, tpp_max, gate, nominal))
        srate = q('LOG:SRATe?')
        check(srate.strip() == str(want), '%s %s %d: LOG:SRATe? %s (the rung run)' % (profile, src, rate, srate))
        if A.expect_rung:
            check(abs(nominal / A.expect_rung - 1) < 0.004, '%s %s %d: the expected rung %d' % (profile, src, rate, A.expect_rung))
        check(cnt[2] == 0 and cnt[4] == 0, '%s %s %d: the disk kept up at the rung (ring overwritten %d, lost %d)'
              % (profile, src, rate, cnt[2], cnt[4]))
    elif len(st) > 9:
        # No page-program probe below 76800: LOG:DISK? tpp 0 / 0 = none (it
        # used to keep the last probe's numbers, device 825080c).
        check(st[8] == 0 and st[9] == 0, '%s %s %d: no probe, LOG:DISK? tpp %d / %d = none' % (profile, src, rate, st[8], st[9]))
    TABLE.append((profile, src, rate, prep_s, rows, lost, cnt[2], bps, bps / 256, len(dlg) / max(rows, 1), hw, int(isr[20]) / 10))
    print('  (device uptime: LOG:STATe ON %.1f s, run %.1f s, end %.1f s)' % (up0 / 1000, (t_run or up1) / 1000, up1 / 1000))
    print('  %s %s %d: prep %.1f s, rows %d, lost %d (ring overwritten %d), %.0f B/s, %.0f pages/s, %.2f B/sample, FIFO high water %d B, ISR CPU %.1f %%'
          % (profile, src, rate, prep_s, rows, lost, cnt[2], bps, bps / 256, len(dlg) / max(rows, 1), hw, int(isr[20]) / 10), flush=True)
    return lost


def stop_in_trial(q, profile):
    """A 76800 disk session stopped in its writer trial (fastlog.rs `Gate`):
    abandoned as in Ready: Done, no rows, the file removed."""
    q('LOG:STATe OFF')
    q('CONF:VOLT:AC')
    q('LOG:SOURce AD2')
    q('LOG:SRATe 76800')
    time.sleep(3)
    cat0 = q('MMEM:CAT? "LOGS"').count('FAST')
    q('LOG:DEST DISK')
    q('LOG:DURation 10')
    q('LOG:STATe ON')
    st = []
    stopped = None
    for _ in range(4000):
        st = [int(x) for x in q('LOG:DISK?').split(',')]
        if st[0] == 3 and stopped is None:
            q('LOG:STATe OFF')
            stopped = st
        if st[0] >= 5:
            break
        time.sleep(0.05)
    cat1 = q('MMEM:CAT? "LOGS"').count('FAST')
    print('  stop in the trial: stopped at %s, end %s' % (stopped, st), flush=True)
    if 0 < st[8] <= 800 and st[9] <= 1500:
        check(stopped is not None and st[0] == 5 and st[4] == 0 and (len(st) < 11 or st[10] == 0) and cat1 == cat0,
              '%s: a 76800 session stopped in its writer trial ends Done, no rows, no file left (FAST files %d -> %d)' % (profile, cat0, cat1))
        check(st[1] == -1, '%s: LOG:DISK? names no file after the trial removed it (file %d)' % (profile, st[1]))
    else:
        # A slow chip: no trial (the session runs at 38400 at once), a stop
        # is an ordinary one: the file stays.
        check(stopped is not None and st[0] == 5 and st[10] == 0 and cat1 == cat0 + 1,
              '%s: tPP %d us, no trial: the early stop keeps the file (%d rows, FAST files %d -> %d)' % (profile, st[8], st[4], cat0, cat1))


def bt_checks():
    try:
        s = socket.create_connection(('127.0.0.1', A.bt_port), timeout=5)
    except OSError as e:
        check(False, 'BT passthrough (%s)' % e)
        return
    s.settimeout(20)
    buf = b''

    def line(cmd, reply=True):
        nonlocal buf
        s.sendall(cmd.encode() + b'\n')
        if not reply:
            return None
        while b'\n' not in buf:
            d = s.recv(65536)
            if not d:
                return None
            buf += d
        l, _, buf = buf.partition(b'\n')
        return l

    def block(cmd):
        nonlocal buf
        s.sendall(cmd.encode() + b'\n')
        while len(buf) < 6:
            buf += s.recv(65536)
        n = int(buf[2:6])
        while len(buf) < 6 + n + 1:
            buf += s.recv(65536)
        d, buf = buf[6:6 + n], buf[6 + n + 1:]
        return d

    try:
        line('*IDN?')
        line('LOG:STATe OFF', False)
        line('LOG:DEST HOST', False)
        line('CONF:VOLT:AC', False)
        time.sleep(2)
        line('LOG:SOURce AD2', False)
        line('LOG:SRATe 1200', False)
        line('*CLS', False)
        line('LOG:STATe ON', False)
        e = line('SYST:ERR?').decode()
        st = line('LOG:STATe?').decode()
        check('Rate too high for Bluetooth' in e and st == '0', 'BT: AD2 1200 S/s refused (%s, STATe? %s)' % (e, st))
        line('LOG:DEST DISK', False)
        line('LOG:STATe ON', False)
        e = line('SYST:ERR?').decode()
        check('Command protected' in e, 'BT: LOG:DEST DISK session refused (%s)' % e)
        line('LOG:DEST HOST', False)
        line('CONF:VOLT:DC', False)
        time.sleep(2)
        line('LOG:SOURce AD1', False)
        line('LOG:OSR 3', False)
        line('LOG:CHOP ON', False)
        line('LOG:STATe ON', False)
        e = line('SYST:ERR?').decode()
        n = 0
        t0 = time.time()
        while time.time() - t0 < 20:
            n += len(block('LOG:DATA?')) // 12
            time.sleep(0.5)
        line('LOG:STATe OFF', False)
        check(e.startswith('0,') and n > 20, 'BT: AD1 40 S/s session drained over BT LOG:DATA? (%d records, %s)' % (n, e))
    except Exception as x:
        check(False, 'BT: %r' % x)
    finally:
        s.close()


if A.region_c:
    sys.path.insert(0, HERE)
    import regionc  # noqa: E402
    # Not hooked: the Prep / Stop code (fastlog::prep and the FAT writer
    # fatren it drives; link-native.x: before / after the logger session,
    # never while the ISR streams). Hot during Prep: a Python hook per call
    # stretched a 76800 Prep to many minutes.
    # Nor the screenshot pre-erase (screenshot::prep_step and its disk(),
    # main.rs `prep_poll`): from 2 s of disk quiet (PREP_QUIET_MS) it scans
    # the FAT and erases free clusters on the busy-timed W25Q128 (45 ms / 4 KiB
    # with --profiles typ) for minutes of device time, i.e. through the whole
    # --ui-nav page tree. With a Python hook on those two functions the
    # emulator crawled (the 300 ms steps after uptime 2.3 s took 1-3 min each;
    # 2026-10-24: --ui-nav --region-c hung in nav_tree; the 277 other hooks
    # alone: normal speed). Never during a session (`prep_poll`'s `ok`
    # excludes a logger session); if that gate broke, the FlashWatch data
    # watch would still report it (prep_step loads its literal pool from
    # region C: "data screenshot::prep_step+0x.. <- 0x0808....").
    RC = regionc.Hooks(A.app, os.path.join(HERE, 'out'), 'fastlog-%d-%d' % (A.port, os.getpid()),
                       exclude=('fastlog::prep::', 'opendm40::fatren::',
                                'opendm40::screenshot::prep_step', 'opendm40::screenshot::disk'))
    if not RC.script:
        sys.exit('fastlogtest: --region-c needs llvm-nm and the ELF symbols')
for profile in A.profiles.split(','):
    print('profile %s: tPP %d us, tSE %d us, tBE64 %d us' % (profile, PROF[profile][0], PROF[profile][1], PROF[profile][3]), flush=True)
    emu = run_emu(profile)
    try:
        t = connect()
        T[:] = [t]

        def q(line, raw=False, timeout=120):
            t.write(line)
            if '?' not in line:
                return None
            b = t.read(maxlen=8192, timeout=timeout)
            return b if raw else b.decode(errors='replace').strip()

        print('*IDN?', q('*IDN?'), flush=True)
        MIR[:] = [int(x) for x in q('DBG:VDISK:MIR?').split(',')]
        print('DBG:VDISK:MIR? %s (served, status, rewritten, ms)' % ','.join(map(str, MIR)), flush=True)
        if A.host_read:
            check(MIR[0] == 1 and MIR[1] in (3, 4), 'the slow-bank mirror is served (DBG:VDISK:MIR? %s)' % ','.join(map(str, MIR)))
        if A.ui_nav:
            nav_tree(q)
        if A.stop_in_trial:
            stop_in_trial(q, profile)
        for sv in A.sessions.split(','):
            src, rate = sv.split(':')
            lost = session(q, src, int(rate), profile)
            if src == 'ad1' and int(rate) < 160:
                # The AD1 main-loop sampler drops a conversion on an
                # occasional long pass (logger.rs; LOG:GAPs?), disk or not.
                check(lost <= 3, '%s %s %s: at most an occasional main-loop gap (%d)' % (profile, src, rate, lost))
            elif src == 'ad1':
                # 160 .. 640 S/s: the stream ISR (logger.rs `isr`, AD1).
                check(lost == 0, '%s %s %s: no sample lost (AD1 stream ISR)' % (profile, src, rate))
            elif profile == 'typ' and int(rate) <= 9600:
                check(lost == 0, '%s %s %s: no sample lost' % (profile, src, rate))
        if not A.no_bt and profile == A.profiles.split(',')[0]:
            bt_checks()
    finally:
        stop_emu(emu)

print('\nprofile  src  rate   prep_s   rows   lost  ring_ovw   B/s   pages/s  B/sample fifo_hw isr_cpu%')
for r in TABLE:
    print('%-7s %-4s %6d %7.1f %7d %6d %8d %7.0f %7.0f %8.2f %7d %7.1f' % r)
for v in VOL:
    print('volume %-16s %9d bytes %6.2f s device %7.0f KB/s %6.1f s wall' % v)
if RC:
    RC.cleanup()
print('FAIL: %d checks' % len(FAIL) if FAIL else 'PASS')
sys.exit(1 if FAIL else 0)
