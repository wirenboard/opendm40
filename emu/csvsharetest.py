#!/usr/bin/env python3
"""opendm40: the CSV generators shared by the USB volume and MMEM:DATA?
(firmware/src/dlog.rs `csv_gen`: two cursors for both paths). Reads of a
generated CSV over the two paths, interleaved, must give the bytes of
firmware/tools/dlog.py of its DLG.

  cd firmware && ./build.sh
  emu/csvsharetest.py --app firmware/target/native/thumbv7em-none-eabihf/release/opendm40

One run (one USB/IP connection, USBTMC and MSC in turn):
  1. two CSV recordings (LOG:CSV ON, --rec s wall each) and a fast-logger
     disk session (AD2 --rate S/s, --dur s): LOGa, LOGb, FASTc;
  2. each DLG read raw with MMEM:DATA?, dlog.py of it = the reference CSV;
     the volume's LOGS/ entry of each CSV has the reference's size;
  3. interleaved, one MMEM:DATA? piece (6000 B) and one READ(10) of 8
     sectors in turn: LOGa over both paths; LOGa over SCPI with LOGb over
     USB and the other way round; FASTc over SCPI (the first --pieces
     pieces) with FASTc over USB from the head and from the middle in turn
     (three readers, two cursors). Every read stream == the reference.
DBG:VDISK:RD? field 4 (generator opens) is printed per step. USB/IP port:
--port, else a free one (run.py free_port). Exit status 0 = PASS.
"""
import argparse
import os
import shutil
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
ap.add_argument('--rec', type=float, default=40, help='each CSV recording, wall s (default 40)')
ap.add_argument('--rate', type=int, default=19200, help='fast logger AD2 S/s (default 19200)')
ap.add_argument('--dur', type=int, default=4, help='fast logger session, device s (default 4)')
ap.add_argument('--pieces', type=int, default=60, help='MMEM:DATA? pieces of FASTc (default 60)')
ap.add_argument('--mips', default='200')
A = ap.parse_args()
sys.path.insert(0, HERE)
import run  # noqa: E402
if A.port is None:
    A.port = run.free_port()
print('USB/IP port %d' % A.port, flush=True)
os.environ['USBIP_PORT'] = str(A.port)
import usbip_client as uc  # noqa: E402

TAG = 'csvshare-%d-%d' % (A.port, os.getpid())
FAIL = []
PIECE = 6000


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
        self.m = uc.Msc(self.d)

    def q(self, s, timeout=60, raw=False):
        self.t.write(s)
        r = self.t.read(maxlen=8192, timeout=timeout)
        return r if raw else r.decode(errors='replace').strip()

    def w(self, s):
        self.t.write(s)

    def opens(self):
        return int(self.q('DBG:VDISK:RD?').split(',')[3])

    def piece(self, name, off):
        """MMEM:DATA? "name",off: the block's data."""
        r = self.q('MMEM:DATA? "%s",%d' % (name, off), raw=True)
        if r[:2] != b'#4':
            raise IOError('MMEM:DATA? "%s",%d: %r' % (name, off, r[:60]))
        return r[6:6 + int(r[2:6])]

    def get(self, name):
        data = b''
        while True:
            d = self.piece(name, len(data))
            data += d
            if len(d) < PIECE:
                return data

    # --- the volume (LUN 0), as fastendtest.py Vol
    def read(self, lba, n):
        return self.m.ok(0, struct.pack('>BBIBHB', 0x28, 0, lba, 0, n, 0), n * 512)

    def layout(self):
        for _ in range(4):
            if self.m.cmd(0, bytes(6))[0] == 0:
                break
            self.m.sense(0)
        b = self.read(0, 1)
        _, spc, rsvd, nfat, nroot = struct.unpack_from('<HBHBH', b, 11)
        self.spc = spc
        self.root = rsvd + nfat * struct.unpack_from('<H', b, 22)[0]
        self.data = self.root + nroot * 32 // 512

    def lba(self, c):
        return self.data + (c - 2) * self.spc

    def find(self, secs, name):
        for s_ in secs:
            d = self.read(s_, 1)
            for k in range(16):
                e = d[k * 32:k * 32 + 32]
                if e[0] == 0:
                    return None
                if e[:11] == name:
                    return struct.unpack_from('<H', e, 26)[0], struct.unpack_from('<I', e, 28)[0]
        return None

    def vfile(self, n83):
        logs = self.find(range(self.root, self.data), b'LOGS       ')
        return logs and self.find(range(self.lba(logs[0]), self.lba(logs[0]) + self.spc), n83)


class UsbRd:
    """Sequential READ(10)s of a volume file from `off` (8 sectors each)."""

    def __init__(self, l, f, off=0):
        self.l, (self.start, self.size), self.off, self.got = l, f, off, b''

    def done(self):
        return self.off >= self.size

    def step(self):
        if self.done():
            return
        n = min(8, (self.size - self.off + 511) // 512)
        b = self.l.read(self.l.lba(self.start) + self.off // 512, n)
        self.got += b[:self.size - self.off]
        self.off += n * 512


class ScpiRd:
    def __init__(self, l, name, limit=None):
        self.l, self.name, self.limit, self.got, self.end = l, name, limit, b'', False

    def done(self):
        return self.end or (self.limit is not None and len(self.got) >= self.limit * PIECE)

    def step(self):
        if self.done():
            return
        d = self.l.piece(self.name, len(self.got))
        self.got += d
        self.end = len(d) < PIECE


def interleave(l, what, readers):
    o0, t0 = l.opens(), time.time()
    while not all(r.done() for r in readers):
        for r in readers:
            r.step()
    print('  %s: %.1f s wall, %d generator opens' % (what, time.time() - t0, l.opens() - o0), flush=True)


def newest(l, pre):
    cat = [e.strip('"').split(',')[0] for e in l.q('MMEM:CAT?').split(',"')[1:]]
    return sorted({x.rsplit('.', 1)[0] + '.DLG' for x in cat if x.rsplit('/', 1)[-1].startswith(pre)})


def record_csv(l):
    for _ in range(30):  # refused while the disk is busy after the boot
        l.w('LOG:CSV ON')
        time.sleep(2)
        if l.q('LOG:CSV?').split(',')[0] == '1':
            break
        print('  LOG:CSV ON: %s, again' % l.q('SYST:ERR?'), flush=True)
    time.sleep(A.rec)
    l.w('LOG:CSV OFF')
    for _ in range(120):
        st = l.q('LOG:CSV?')
        if st.split(',')[0] == '3':
            break
        time.sleep(1)
    check(st.split(',')[0] == '3', 'CSV recording committed (LOG:CSV? %s)' % st)


def record_fast(l):
    for c in ('LOG:STATe OFF', 'CONF:VOLT:AC', 'LOG:SOURce AD2', 'LOG:SRATe %d' % A.rate):
        l.w(c)
    time.sleep(2)
    for c in ('LOG:DEST DISK', 'LOG:DURation %d' % A.dur, '*CLS', 'LOG:STATe ON'):
        l.w(c)
    st = []
    for _ in range(600):
        st = [int(x) for x in l.q('LOG:DISK?').split(',')]
        if st and st[0] >= 5:
            break
        time.sleep(0.5)
    l.w('LOG:DEST HOST')
    check(st[:1] == [5], 'fast-logger session done (LOG:DISK? %s)' % st)


def ref_of(l, path):
    dlg = l.get(path)
    fn = os.path.join(OUT, '%s-%s' % (TAG, path.replace('/', '-')))
    open(fn, 'wb').write(dlg)
    try:
        ref = subprocess.run([sys.executable, os.path.join(ROOT, 'firmware/tools/dlog.py'), fn],
                             capture_output=True).stdout
    finally:
        os.remove(fn)
    print('  %s: %d B DLG, %d B CSV (dlog.py)' % (path, len(dlg), len(ref)), flush=True)
    return ref


def main():
    os.makedirs(OUT, exist_ok=True)
    disk = os.path.join(OUT, TAG + '-disk.bin')
    shutil.copy(os.path.join(ROOT, 'stock/re/dumps/spi_dumps/dm40_msc_full_2026-10-13.bin'), disk)
    log = open(os.path.join(OUT, TAG + '.out'), 'w')
    emu = subprocess.Popen([sys.executable, os.path.join(HERE, 'run.py'), 'fwrs', '--mips', A.mips, '--app', A.app,
                            '--usbip', '0.01', '--usbip-port', str(A.port), '--uid', run.emu_uid(A.port).hex().upper(),
                            '--name', TAG, '--disk', disk, '--dmm', 'vsrc:0.2,ac=0.5,f=50', '--run-for', '5000'],
                           cwd=ROOT, stdout=log, stderr=subprocess.STDOUT, start_new_session=True)
    try:
        l = Link()
        print('*IDN?', l.q('*IDN?'), flush=True)
        up0 = int(l.q('DBG:PING?').split(',')[0])
        record_csv(l)
        record_csv(l)
        record_fast(l)
        logs, fast = newest(l, 'LOG'), newest(l, 'FAST')
        check(len(logs) >= 2 and len(fast) >= 1, 'MMEM:CAT? lists the new files (%s %s)' % (logs[-2:], fast[-1:]))
        names = logs[-2:] + fast[-1:]
        refs = [ref_of(l, n) for n in names]
        csv = [n[:-4] + '.CSV' for n in names]
        # The volume after the refresh (a quiet second, then UNIT ATTENTION).
        vf = [None] * 3
        for _ in range(60):
            time.sleep(1)
            try:
                l.layout()
                vf = [l.vfile(n.rsplit('/', 1)[-1][:-4].ljust(8).encode() + b'CSV') for n in names]
            except IOError:
                continue
            if all(vf):
                break
        for n, f, r in zip(csv, vf, refs):
            check(f is not None and f[1] == len(r), '%s on the volume, %s B (dlog.py %d B)' % (n, f and f[1], len(r)))
        if not all(vf):
            return
        a, b, c = 0, 1, 2

        rs = [ScpiRd(l, csv[a]), UsbRd(l, vf[a])]
        interleave(l, 'same file %s, SCPI + USB' % csv[a], rs)
        check(rs[0].got == refs[a], '%s: MMEM:DATA? == dlog.py' % csv[a])
        check(rs[1].got == refs[a], '%s: USB volume == dlog.py' % csv[a])

        for x, y in ((a, b), (b, a)):
            rs = [ScpiRd(l, csv[x]), UsbRd(l, vf[y])]
            interleave(l, '%s over SCPI + %s over USB' % (csv[x], csv[y]), rs)
            check(rs[0].got == refs[x], '%s: MMEM:DATA? == dlog.py' % csv[x])
            check(rs[1].got == refs[y], '%s: USB volume == dlog.py' % csv[y])

        mid = (vf[c][1] // 2) & ~4095
        n = A.pieces * PIECE
        rs = [ScpiRd(l, csv[c], A.pieces), UsbRd(l, (vf[c][0], min(vf[c][1], n)), 0),
              UsbRd(l, (vf[c][0], min(vf[c][1], mid + n)), mid)]
        interleave(l, '%s over SCPI, USB from 0 and from %d' % (csv[c], mid), rs)
        check(rs[0].got == refs[c][:len(rs[0].got)] and len(rs[0].got) == min(n, len(refs[c])),
              '%s: MMEM:DATA? %d B == dlog.py' % (csv[c], len(rs[0].got)))
        check(rs[1].got == refs[c][:len(rs[1].got)] and rs[1].got, '%s: USB from 0, %d B == dlog.py' % (csv[c], len(rs[1].got)))
        check(rs[2].got == refs[c][mid:mid + len(rs[2].got)] and rs[2].got,
              '%s: USB from %d, %d B == dlog.py' % (csv[c], mid, len(rs[2].got)))
        up1 = int(l.q('DBG:PING?').split(',')[0])
        check(up1 > up0, 'no reboot (uptime %d -> %d ms)' % (up0, up1))
        e = l.q('SYST:ERR?')
        check(e.startswith('0,'), 'SYST:ERR? %s' % e)
    except Exception as e:
        check(False, 'the meter answers: %r' % e)
    finally:
        try:
            os.killpg(emu.pid, 15)
        except Exception:
            pass
        emu.wait()
        try:
            os.remove(disk)
        except OSError:
            pass


main()
print('FAIL: %d checks' % len(FAIL) if FAIL else 'PASS')
sys.exit(1 if FAIL else 0)
