#!/usr/bin/env python3
"""opendm40 extra RAM areas (firmware/link-native.x .bkp_bss in the backup
SRAM after the settings record, .usbhs_bss in the USBHS FIFO RAM): the
buffers there start zeroed whatever the areas held, and the settings record
and its sequence words survive a reset while the buffers are in use.

  cd firmware && ./build.sh
  emu/xramtest.py --app firmware/target/native/thumbv7em-none-eabihf/release/opendm40

One run:
  1. Before the firmware starts (run.py --script at 0 s), .bkp_bss and the
     USBHS RAM are filled with 0xA5A5A5A5, as stale bytes of an earlier
     image (the backup SRAM keeps them over a reset and an update). The
     record (0x40024000..), the sequence and boot words (0x40024FE8..) are
     left as they are.
  2. DBG:BOOT? SETTINGS,DEFAULTS; SYST:ALER:ALAR VOLUME; wait for SAVE,OK.
  3. The buffers in use: a screenshot (DBG:SWIPE D,3: screenshot::JOB), a
     CSV recording of --long seconds (csvlog::TAIL in the USBHS RAM: several
     commits, more than one 4 KiB block), the touch test page opened and
     closed (app_touchtest::TT), MMEM:CAT?. The recording's LOGnnn.DLG and
     LOGnnn.CSV are read with MMEM:DATA?: the CSV must equal
     firmware/tools/dlog.py of the DLG byte for byte. dm40.repl maps the
     USBHS RAM as peripherals/WordOnlyRam.cs (a byte store fills the whole
     word, as on the device), so a byte-written TAIL fails here.
  4. DBG:RESET: DBG:BOOT? SETTINGS,BKP with the same record sequence number,
     SYST:ALER:ALAR? VOLUME; again a screenshot and a short recording work.
USB/IP port: --port, else a free one in 64600-64699. Exit status 0 = PASS.
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
ap.add_argument('--app', required=True, help='opendm40 native ELF')
ap.add_argument('--port', type=int, default=None, help='USB/IP port (default: a free one in 64600-64699)')
ap.add_argument('--long', type=float, default=480, help='first recording, wall seconds (default 480: the emulator runs at about half speed; several commits, the DLG past one 4 KiB TAIL block)')
A = ap.parse_args()
sys.path.insert(0, HERE)
import run  # noqa: E402
if A.port is None:
    A.port = run.free_port(64600, 64699)
print('USB/IP port %d' % A.port, flush=True)
os.environ['USBIP_PORT'] = str(A.port)
import usbip_client as uc  # noqa: E402
import bootinfo  # noqa: E402

SERIAL = run.emu_uid(A.port).hex().upper()
TAG = 'xram-%d-%d' % (A.port, os.getpid())
FAIL = []


def check(ok, what):
    print('  %s %s' % ('ok  ' if ok else 'FAIL', what), flush=True)
    if not ok:
        FAIL.append(what)


def nm(elf):
    tools = sorted(__import__('glob').glob(os.path.join(ROOT, 'toolchain/rustup/toolchains/*/lib/rustlib/*/bin/llvm-nm')))
    if not tools:
        sys.exit('no llvm-nm under toolchain/')
    syms = {}
    for l in subprocess.check_output([tools[0], A.app], text=True).splitlines():
        p = l.split()
        if len(p) == 3:
            syms[p[2]] = int(p[0], 16)
    return syms


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


def settings(l):
    """The settings fields of DBG:BOOT? as a list (bootinfo.py)."""
    return bootinfo.settings(l.q('DBG:BOOT?'))


def wait(l, what, cond, tries=60, dt=1.0):
    for _ in range(tries):
        if cond():
            return True
        time.sleep(dt)
    check(False, 'timed out: %s' % what)
    return False


def shots(l):
    return [f for f in l.q('MMEM:CAT?').split(',') if 'BMP' in f.upper()]


def block(r):
    """The data of an IEEE 488.2 definite-length block (#<n><len><data>)."""
    if r[:1] != b'#' or not r[1:2].isdigit():
        return None
    n = int(r[1:2])
    ln = int(r[2:2 + n])
    d = r[2 + n:2 + n + ln]
    return d if len(d) == ln else None


def get_file(l, name):
    """A disk file in MMEM:DATA? pieces of 6000 bytes (firmware/tools/dm40scpi.py get_file)."""
    data = b''
    while True:
        l.t.write('MMEM:DATA? "%s",%d' % (name, len(data)))
        r = l.t.read(timeout=60)
        d = block(r)
        if d is None:
            print('  MMEM:DATA? "%s",%d: %r' % (name, len(data), r[:60]), flush=True)
            return None
        data += d
        if len(d) < 6000:
            return data


def check_log(l, pre, rows):
    """The newest LOGnnn: its CSV (generated from the DLG on the disk, whose
    bytes went through csvlog::TAIL in the USBHS RAM) equals dlog.py of the
    DLG byte for byte, and has a line per row."""
    logs = sorted(e.strip('"').split(',')[0] for e in l.q('MMEM:CAT?').split(',"')[1:] if e.startswith('LOG'))
    if not logs:
        check(False, '%s: no LOG file in MMEM:CAT?' % pre)
        return
    base = logs[-1].rsplit('.', 1)[0]
    dlg = get_file(l, base + '.DLG')
    csv = get_file(l, base + '.CSV')
    check(dlg is not None and dlg[:4] == b'DLG1', '%s: %s.DLG read back (%s bytes, starts %r)'
          % (pre, base, len(dlg) if dlg else '-', dlg[:4] if dlg else None))
    check(csv is not None, '%s: %s.CSV read back (%s bytes)' % (pre, base, len(csv) if csv else '-'))
    if not dlg or csv is None:
        return
    fn = os.path.join(OUT, '%s-%s.DLG' % (TAG, base.replace('/', '-')))
    open(fn, 'wb').write(dlg)
    try:
        ref = subprocess.run([sys.executable, os.path.join(ROOT, 'firmware/tools/dlog.py'), fn],
                             capture_output=True).stdout
        chk = subprocess.run([sys.executable, os.path.join(ROOT, 'firmware/tools/dlog.py'), '--check', fn],
                             capture_output=True, text=True).stdout.strip()
    finally:
        os.remove(fn)
    print('  dlog.py --check: %s' % chk.replace('\n', ' | '), flush=True)
    check(ref == csv and len(ref) > 0, '%s: MMEM:DATA? %s.CSV == dlog.py %s.DLG (%d / %d bytes)'
          % (pre, base, base, len(csv), len(ref)))
    lines = [x for x in csv.decode(errors='replace').splitlines() if x[:1].isdigit()]
    check(len(lines) == rows, '%s: %s.CSV has %d data lines for %d rows' % (pre, base, len(lines), rows))
    return len(dlg)


def use_buffers(l, pre, secs, min_dlg=0):
    n0 = len(shots(l))
    l.w('DBG:SWIPE D,3')
    ok = wait(l, 'screenshot', lambda: len(shots(l)) > n0, tries=90)
    check(ok, '%s: a screenshot was saved (%d -> %d BMP entries)' % (pre, n0, len(shots(l))))
    # MMEM:CAT? while the shot holds the disk answers "Mass storage busy".
    for _ in range(20):
        if l.q('SYST:ERR?').startswith('0,'):
            break
    # A long recording commits every COMMIT_MS (60 s): later commits append
    # at an offset that is not a multiple of 4 inside TAIL, and the file
    # crosses into a second 4 KiB block.
    l.w('LOG:CSV ON')
    t0 = time.time()
    while time.time() - t0 < secs:
        time.sleep(min(30, secs))
        st = l.q('LOG:CSV?')
        print('  %3.0f s LOG:CSV? %s' % (time.time() - t0, st), flush=True)
    st = l.q('LOG:CSV?')
    check(st.split(',')[0] != '0', '%s: CSV recording running (LOG:CSV? %s)' % (pre, st))
    l.w('LOG:CSV OFF')
    # state 3 = done (scpi.rs LOG:CSV?): every byte committed.
    ok = wait(l, 'recording end', lambda: l.q('LOG:CSV?').split(',')[0] == '3')
    st = l.q('LOG:CSV?')
    f = st.split(',')
    rows = int(f[2]) if len(f) > 4 else 0
    check(ok and rows > 0 and f[3] == f[4], '%s: recording committed (LOG:CSV? %s)' % (pre, st))
    n = check_log(l, pre, rows)
    if min_dlg:
        check((n or 0) > min_dlg, '%s: the DLG is %s bytes (> %d: more than one TAIL block)' % (pre, n, min_dlg))
    l.w('DBG:TOUCH:TEST ON')
    time.sleep(1)
    check(l.q('DBG:TOUCH:TEST?') == '1', '%s: touch test page open' % pre)
    l.w('DBG:TOUCH:TEST OFF')
    time.sleep(1)
    check(l.q('DBG:TOUCH:TEST?') == '0', '%s: touch test page closed' % pre)
    e = l.q('SYST:ERR?')
    check(e.startswith('0,'), '%s: SYST:ERR? %s' % (pre, e))


def main():
    syms = nm(A.app)
    bss = (syms['__bkp_bss_start'], syms['__bkp_bss_end'])
    rec_end = syms['__bkp_record_end']
    print('.bkp_bss 0x%08X..0x%08X (record ends 0x%08X); .usbhs_bss 0x%08X..0x%08X'
          % (bss[0], bss[1], rec_end, syms['__usbhs_bss_start'], syms['__usbhs_bss_end']), flush=True)
    check(rec_end <= bss[0] and bss[1] <= 0x40024FE8, 'the linked .bkp_bss is between the record and the sequence words')
    script = os.path.join(OUT, TAG + '.script')
    with open(script, 'w') as f:
        for a in list(range(bss[0], bss[1], 4)) + list(range(0x40060000, 0x40061000, 4)):
            f.write('0 monitor sysbus WriteDoubleWord 0x%08X 0xA5A5A5A5\n' % a)
    disk = os.path.join(OUT, TAG + '-disk.bin')
    shutil.copy(os.path.join(ROOT, 'stock/re/dumps/spi_dumps/dm40_msc_full_2026-10-13.bin'), disk)
    log = open(os.path.join(OUT, TAG + '.out'), 'w')
    emu = subprocess.Popen([sys.executable, os.path.join(HERE, 'run.py'), 'fwrs', '--app', A.app, '--usbip', '0.01',
                            '--usbip-port', str(A.port), '--uid', SERIAL, '--name', TAG, '--disk', disk,
                            '--script', script, '--run-for', '3000'],
                           cwd=ROOT, stdout=log, stderr=subprocess.STDOUT, start_new_session=True)
    try:
        l = Link()
        s = settings(l)
        print('  boot 1: %s' % ','.join(s), flush=True)
        check(s[1] == 'DEFAULTS', 'first boot: defaults (a new machine)')
        l.w('SYST:ALER:ALAR VOLUME')
        ok = wait(l, 'settings save', lambda: (lambda s: s[s.index('SAVE') + 1] == 'OK')(settings(l)), tries=120)
        check(ok, 'the change saved (DBG:BOOT? SAVE,OK)')
        use_buffers(l, 'boot 1', A.long, 4096 if A.long >= 480 else 0)
        s1 = settings(l)
        print('  before reset: %s' % ','.join(s1), flush=True)
        l.w('DBG:RESET')
        l.close()
        time.sleep(3)
        for _ in range(60):
            l = Link()
            try:
                s2 = settings(l)
                break
            except Exception:
                l.close()
                time.sleep(2)
        print('  after DBG:RESET: %s' % ','.join(s2), flush=True)
        check(s2[1] == 'BKP', 'after DBG:RESET: the settings from the backup SRAM')
        seq1 = s1[s1.index('SAVE') + 3]
        check(s2[3] == seq1, 'after DBG:RESET: the record sequence number %s (saved %s)' % (s2[3], seq1))
        check(l.q('SYST:ALER:ALAR?') == 'VOLUME', 'after DBG:RESET: SYST:ALER:ALAR? VOLUME')
        use_buffers(l, 'boot 2', 4)
        l.close()
    except Exception as e:
        check(False, 'run: %r' % (e,))
    finally:
        try:
            os.killpg(emu.pid, 15)
        except Exception:
            pass
        emu.wait()
        for p in (disk, script):
            try:
                os.remove(p)
            except OSError:
                pass
    print('PASS' if not FAIL else 'FAIL: %d\n  %s' % (len(FAIL), '\n  '.join(FAIL)), flush=True)
    sys.exit(1 if FAIL else 0)


main()
