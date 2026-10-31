#!/usr/bin/env python3
"""opendm40: the INFO/ slow-bank mirror on the SPI disk (src/vdisk.rs
`mirror_boot`, docs/native-boot.md "The slow flash bank while an AD2 stream
runs") across boots. Each boot runs the emulator for --run-for virtual
seconds with --save-disk / --save-flash, asks DBG:VDISK:MIR? (served,
status 3 PRESENT / 4 WRITTEN, rewritten, ms) and then checks the saved SPI
image: the root file _INFOMIR.BIN, followed through FAT 1, is byte-identical
to the internal flash 0x08080000..0x080C1000 of that boot.

  1 fresh disk (no mirror), image A          -> WRITTEN
  2 the disk of 1, image A                   -> PRESENT, the disk unchanged
  3 the disk of 2, image B (--app2)          -> WRITTEN (a new image)
  4 the disk of 3, image B, a byte of the calibration sector's unused tail
    changed in the flash (--base copy)       -> WRITTEN (a calibration change)
  5 the disk of 4, the same flash as 4       -> PRESENT
  6 a blank SPI disk, image A: no mirror (BADBPB); an AD2 stream that
    starts inside a READ(10) crossing into the slow bank: the command fails
    at the first slow sector (residue, 02/04/01), the retry after the stream
    reads the same bytes; an AD2 stream to the host while INFO/ is read: NOT
    READY from exactly the slow-bank sectors

  cd firmware && ./build.sh && DM40_TDIR=target/ks DM40_OUT=/tmp/ks.bin ./build.sh --features keep-screen
  emu/mirrortest.py --app firmware/target/native/thumbv7em-none-eabihf/release/opendm40 \\
      --app2 firmware/target/ks/thumbv7em-none-eabihf/release/opendm40

Never prints the mirrored bytes (calibration / loader data are per-unit
private). Exit status 0 = PASS.
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
ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
ap.add_argument('--app', required=True, help='opendm40 native ELF (image A)')
ap.add_argument('--app2', required=True, help='another opendm40 native ELF (image B, e.g. --features keep-screen)')
ap.add_argument('--port', type=int, default=None, help='USB/IP port (default: run.free_port, DM40_EMU_PORTS)')
ap.add_argument('--run-for', type=float, default=30, help='virtual seconds per boot (the disk is saved at the end)')
ap.add_argument('--fallback-only', action='store_true', help='only boot 6 (a blank SPI disk: no mirror, NOT READY)')
A = ap.parse_args()
sys.path.insert(0, HERE)
import run  # noqa: E402
if A.port is None:
    A.port = run.free_port()
os.environ['USBIP_PORT'] = str(A.port)
import usbip_client as uc  # noqa: E402

OUT = os.path.join(HERE, 'out')
os.makedirs(OUT, exist_ok=True)
TAG = 'mirror-%d-%d' % (A.port, os.getpid())
LO, HI = 0x08080000, 0x080C1000
FAIL = []
MADE = []


def check(ok, what):
    print('  %s %s' % ('ok  ' if ok else 'FAIL', what), flush=True)
    if not ok:
        FAIL.append(what)


def mirror_file(disk):
    """_INFOMIR.BIN of the SPI image (4 KiB-sector FAT16): bytes or None."""
    b = disk
    bps, spc, rsvd, nfat, nroot = struct.unpack_from('<HBHBH', b, 11)
    fatsz = struct.unpack_from('<H', b, 22)[0]
    root = (rsvd + nfat * fatsz) * bps
    data = root + nroot * 32
    cb = spc * bps
    fat = rsvd * bps
    for i in range(nroot):
        e = b[root + 32 * i:root + 32 * i + 32]
        if e[0] == 0:
            break
        if e[0] == 0xE5 or e[11] & 0x3F == 0x0F or e[11] & 0x18:
            continue
        if e[:11] == b'_INFOMIRBIN':
            c, size = struct.unpack_from('<HI', e, 26)
            out = b''
            while len(out) < size:
                if not 2 <= c < 0xFFF0:
                    return None
                out += b[data + (c - 2) * cb:data + (c - 1) * cb]
                c = struct.unpack_from('<H', b, fat + 2 * c)[0]
            return out[:size]
    return None


def boot(n, app, disk_in, base=None, during=None, run_for=None):
    disk = os.path.join(OUT, '%s-disk%d.bin' % (TAG, n))
    flash = os.path.join(OUT, '%s-flash%d.bin' % (TAG, n))
    shutil.copy(disk_in, disk)
    MADE.extend([disk, flash])
    cmd = [sys.executable, os.path.join(HERE, 'run.py'), 'fwrs', '--app', app, '--usbip', '0.01',
           '--usbip-port', str(A.port), '--run-for', str(run_for or A.run_for), '--name', '%s-%d' % (TAG, n),
           '--disk', disk, '--save-disk', disk, '--save-flash', flash]
    if base:
        cmd += ['--base', base]
    emu = subprocess.Popen(cmd, cwd=ROOT, stdout=open(os.path.join(OUT, '%s-%d.out' % (TAG, n)), 'w'),
                           stderr=subprocess.STDOUT, start_new_session=True)
    mir = None
    try:
        for _ in range(150):
            try:
                d = uc.Dev('1-0')
                break
            except Exception:
                time.sleep(2)
        else:
            check(False, 'boot %d: no USB/IP device' % n)
            return None, None, None
        t = uc.Usbtmc(d)
        t.write('DBG:VDISK:MIR?')
        mir = [int(x) for x in t.read(timeout=60).decode().strip().split(',')]
        if during:
            during(d, t, mir)
        d.close()
        emu.wait(timeout=3600)
    finally:
        if emu.poll() is None:
            os.killpg(emu.pid, 15)
            emu.wait()
    with open(disk, 'rb') as f:
        dd = f.read()
    with open(flash, 'rb') as f:
        ff = f.read()
    return mir, dd, ff[LO - 0x08000000:HI - 0x08000000]


def verify(n, what, mir, dd, win, want_status):
    print('boot %d (%s): DBG:VDISK:MIR? %s' % (n, what, mir), flush=True)
    if mir is None:
        check(False, 'boot %d: DBG:VDISK:MIR? answered' % n)
        return
    check(mir[0] == 1 and mir[1] == want_status and mir[2] == (want_status == 4),
          'boot %d: served, status %d (want %d), rewritten %d, %d ms' % (n, mir[1], want_status, mir[2], mir[3]))
    m = mirror_file(dd)
    check(m is not None and len(m) == HI - LO and m == win,
          'boot %d: _INFOMIR.BIN on the saved SPI disk == flash 0x%08X..0x%08X (%s bytes)'
          % (n, LO, HI, len(m) if m is not None else None))


src = os.path.join(OUT, 'disk.bin')
fresh = src if os.path.exists(src) else os.path.join(ROOT, 'stock/re/dumps/spi_dumps/dm40_msc_full_2026-10-13.bin')
check(mirror_file(open(fresh, 'rb').read()) is None, 'the starting SPI disk has no mirror')
try:
    if not A.fallback_only:
        m1, d1, w1 = boot(1, A.app, fresh)
        verify(1, 'fresh disk, image A', m1, d1, w1, 4)
        p1 = os.path.join(OUT, '%s-disk1.bin' % TAG)
        m2, d2, w2 = boot(2, A.app, p1)
        verify(2, 'image A again', m2, d2, w2, 3)
        check(d2 == d1, 'boot 2: the SPI disk is unchanged (no write without a change)')
        m3, d3, w3 = boot(3, A.app2, os.path.join(OUT, '%s-disk2.bin' % TAG))
        verify(3, 'image B', m3, d3, w3, 4)
        check(w3 != w2, 'boot 3: image B differs from A inside the mirrored range')
        # A calibration-sector change: the base dump with one byte of the unused
        # tail of 0x080C0000..0x080C1000 changed (never printed).
        base = os.path.join(OUT, '%s-base.bin' % TAG)
        MADE.append(base)
        b = bytearray(open(run.DUMP, 'rb').read())
        off = 0x080C0FF0 - 0x08000000
        b[off] ^= 0x5A
        open(base, 'wb').write(b)
        m4, d4, w4 = boot(4, A.app2, os.path.join(OUT, '%s-disk3.bin' % TAG), base)
        verify(4, 'image B, calibration sector changed', m4, d4, w4, 4)
        check(w4 != w3, 'boot 4: the flash window changed')
        m5, d5, w5 = boot(5, A.app2, os.path.join(OUT, '%s-disk4.bin' % TAG), base)
        verify(5, 'the same again', m5, d5, w5, 3)
        check(d5 == d4, 'boot 5: the SPI disk is unchanged')

    # The fallback: a blank SPI disk (no FAT, so no mirror): not served, and
    # while an AD2 stream runs (LOG:DEST HOST, nothing on the disk) the
    # slow-bank sectors answer NOT READY 02/04/01, exactly those.
    import infovol  # noqa: E402

    def isr_active(q):
        return q('DBG:ISR?').split(',')[1] == '1'

    def mid_read(m, q):
        """A stream that starts inside a multi-block READ(10): the next
        slow-bank block fails the command with the residue, sense 02/04/01;
        the rest of the data phase is fill bytes. Without a mirror the vdisk
        read of that block (`mir_read`) fails as well, so this checks the
        behaviour, not msc.rs's own mid-transfer check. The READ
        crosses from FIRMWARE.BIN's zero-wait part into its slow-bank tail;
        the stream starts after PRE_N of its sectors were taken."""
        fw_lba, _ = infovol.vol_dir(m, 'INFO')['FIRMWARE.BIN']
        slow = fw_lba + infovol.SLOW_FILES['FIRMWARE.BIN'] // 512
        lo, n, pre_n, fast_n = slow - 64, 128, 32, 64
        ref = m.read(0, lo, n, 512, retries=2)
        m.tag += 1
        want_tag = m.tag
        cb = struct.pack('>BBIBHB', 0x28, 0, lo, 0, n, 0)
        m.d.bulk_out(m.ep_out, struct.pack('<4sIIBBB16s', b'USBC', m.tag, n * 512, 0x80, 0, len(cb), cb.ljust(16, b'\0')))
        data = b''
        while len(data) < pre_n * 512:
            c = m.d.bulk_in(m.ep_in, 4096)
            if not c:
                break
            data += c
        q('LOG:STATe ON')
        on = False
        for _ in range(20):
            on = isr_active(q)
            if on:
                break
            time.sleep(0.5)
        check(on, 'boot 6: the stream started inside the READ (DBG:ISR? active)')
        while len(data) < n * 512:
            c = m.d.bulk_in(m.ep_in, min(4096, n * 512 - len(data)))
            if not c:
                break
            data += c
        sig, tag, residue, status = struct.unpack('<4sIIB', m.d.bulk_in(m.ep_in, 13))
        sense = m.sense(0)
        check(sig == b'USBS' and tag == want_tag and status == 1 and residue == (n - fast_n) * 512 and len(data) == n * 512,
              'boot 6: READ(10) %d x %d, stream on after %d: CSW status %d (want 1), residue %d (want %d), %d bytes'
              % (lo, n, pre_n, status, residue, (n - fast_n) * 512, len(data)))
        check(sense == infovol.NOT_READY, 'boot 6: then sense %s (want %s)' % (sense, infovol.NOT_READY))
        check(data[:fast_n * 512] == ref[:fast_n * 512], 'boot 6: the %d sectors before the slow bank == the read without a stream'
              % fast_n)
        st, residue, _ = m.cmd(0, cb, n * 512)
        sense = m.sense(0)
        check(st == 1 and residue == n * 512 and sense == infovol.NOT_READY,
              'boot 6: the retry while streaming fails before its data: status %d, residue %d, sense %s' % (st, residue, sense))
        q('LOG:STATe OFF')
        for _ in range(20):
            if not isr_active(q):
                break
            time.sleep(0.5)
        again = infovol.read(m, lo, n)
        check(again == ref, 'boot 6: the retry after the stream stopped reads the same %d sectors' % n)

    def stream_read(d, t, mir):
        def q(s):
            t.write(s)
            return t.read(timeout=60).decode().strip() if '?' in s else None
        q('CONF:VOLT:AC')
        q('LOG:DEST HOST')
        q('LOG:SOURce AD2')
        q('LOG:SRATe 4800')
        q('LOG:CHOP ON')
        time.sleep(3)
        m = uc.Msc(d)
        mid_read(m, q)
        q('LOG:STATe ON')
        isr = q('DBG:ISR?').split(',')
        refused, whole, other = infovol.read_info(m)
        isr2 = q('DBG:ISR?').split(',')
        q('LOG:STATe OFF')
        check(isr[1] == '1' and isr2[1] == '1', 'boot 6: the AD2 stream ran during the reads (DBG:ISR? active %s / %s)'
              % (isr[1], isr2[1]))
        check(refused == infovol.SLOW_FILES and not other,
              'boot 6: no mirror -> NOT READY from exactly the slow-bank sectors: %s'
              % sorted((k, hex(v)) for k, v in refused.items()))
    blank = os.path.join(OUT, '%s-blank.bin' % TAG)
    MADE.append(blank)
    open(blank, 'wb').write(bytes(16 << 20))
    # Boot 6's callback (two READ(10)s around a stream, then INFO/ during
    # another) takes longer than the other boots': 30 virtual s more.
    m6, _, _ = boot(6, A.app, blank, during=stream_read, run_for=A.run_for + 30)
    print('boot 6 (blank SPI disk): DBG:VDISK:MIR? %s' % m6, flush=True)
    check(m6 is not None and m6[0] == 0 and m6[1] == 2, 'boot 6: blank disk -> mirror not served, status BADBPB (%s)' % m6)
finally:
    for p in MADE:
        if os.path.exists(p):
            os.remove(p)
print('FAIL: %d checks' % len(FAIL) if FAIL else 'PASS')
sys.exit(1 if FAIL else 0)
