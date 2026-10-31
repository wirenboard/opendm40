#!/usr/bin/env python3
"""opendm40 USB mass storage (src/msc.rs over usbd-storage's Bulk-Only
transport) when the host recovers from a stuck or broken command, as Linux
usb-storage does after its 30 s command timeout: Bulk-Only Mass Storage
Reset (class request 0xFF) + CLEAR_FEATURE(HALT) of both bulk endpoints,
and a USB port reset when that does not help. (Device check d8e249d 6b: a
command never completed, usb-storage reset the meter twice, then the
meter hung and the watchdog reset it.)

  cd firmware && ./build.sh
  emu/mscresettest.py --app firmware/target/native/thumbv7em-none-eabihf/release/opendm40

Cases (each: then a TEST UNIT READY, READ(10) of the boot sector and of
README.TXT must give the right bytes, and *IDN? must answer):
  invalid-cbw   31 bytes that are no CBW (6.6.1: the device stalls both
                bulk endpoints until a reset), then the reset recovery;
  read-bot      READ(10) of 64 sectors, 1 of 64 data packets taken, then
                the reset recovery (the rest of the data phase dropped);
  read-port     the same, then a USB port reset (USB/IP re-import);
  ua-port       the volume changed (a screenshot: UNIT ATTENTION pending),
                a READ cut off by a port reset, then TEST UNIT READY;
  alloc-small   INQUIRY with an allocation length of 5: 5 bytes and the CSW
                (not the whole 36-byte reply: babble on the host);
  zero-alloc    REQUEST SENSE with no data phase: the CSW comes (it never
                did: the command stayed without status);
  stop-start    START STOP UNIT without LoEj (a plain stop, then start):
                the medium stays (TEST UNIT READY passes after each);
  eject-load    START STOP UNIT LoEj=1 Start=0 (eject): TEST UNIT READY,
                READ(10), READ CAPACITY, READ FORMAT CAPACITIES fail with
                02/3A/00, INQUIRY still answers,
                DBG:VDISK:EJECT? = 1; a Bulk-Only reset keeps it ejected;
                then LoEj=1 Start=1 (load): one UNIT ATTENTION 06/28/00;
  eject-port    eject, then a USB port reset (a re-plug): the medium is back;
  serial        the unit serial number (firmware/src/uid.rs: the chip UID as 24
                hex digits; run.py --uid gives the emulator its own): the USB
                iSerial string, INQUIRY VPD page 0x80, the serial field of
                *IDN?, all equal to it; VPD page 0x00 lists 00 and 80; another
                page fails 05/24/00.
Exit status 0 = PASS. Per-process names; the Renode log is capped by run.py.
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
ap.add_argument('--app', required=True, help='opendm40 native ELF / image')
ap.add_argument('--port', type=int, default=None,
                help='USB/IP port of the emulator this test starts (default: a free one in 61000-64999, run.free_port)')
ap.add_argument('--cases', default='serial,alloc-small,zero-alloc,invalid-cbw,read-bot,read-port,ua-port,stop-start,eject-load,'
                                   'eject-port')
A = ap.parse_args()
sys.path.insert(0, HERE)
import run  # noqa: E402  (emu/run.py: emu_uid, free_port)
if A.port is None:
    A.port = run.free_port()
print('USB/IP port %d' % A.port, flush=True)
os.environ['USBIP_PORT'] = str(A.port)
import usbip_client as uc  # noqa: E402
import bootinfo  # noqa: E402

SERIAL = run.emu_uid(A.port).hex().upper()

OUT = os.path.join(HERE, 'out')
TAG = 'mscreset-%d-%d' % (A.port, os.getpid())
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
            sys.exit('mscresettest: no USB/IP device on port %d' % A.port)
        self.m = uc.Msc(self.d)
        self.t = uc.Usbtmc(self.d)

    def q(self, s, timeout=30):
        self.t.write(s)
        return self.t.read(timeout=timeout).decode(errors='replace').strip()

    def cbw(self, cb, n, din=True):
        self.m.tag += 1
        self.d.bulk_out(self.m.ep_out, struct.pack('<4sIIBBB16s', b'USBC', self.m.tag, n, 0x80 if din else 0, 0,
                                                   len(cb), cb.ljust(16, b'\0')))

    def bot_reset(self):
        """Linux usb_stor_Bulk_reset: the class reset, then both halts
        cleared (a failed class reset makes Linux reset the port: True)."""
        try:
            self.d.control_out(0x21, 0xFF, 0, self.m.iface)
            ok = True
        except IOError as e:
            print('  Bulk-Only Mass Storage Reset refused (%s): Linux resets the port' % e, flush=True)
            ok = False
        for ep in (self.m.ep_in, self.m.ep_out):
            self.d.control_out(0x02, 0x01, 0, ep)
        if not ok:
            self.port_reset()
        return ok

    def port_reset(self):
        self.d.close()
        time.sleep(0.5)
        self.__init__()

    def ssu(self, b4):
        """START STOP UNIT with CB[4] = b4: the status."""
        st, _, _ = self.m.cmd(0, bytes([0x1B, 0, 0, 0, b4, 0]))
        return st

    def no_medium(self, what):
        st = self.tur()
        s = self.m.sense(0) if st else '-'
        check(st != 0 and s == '02/3A/00', '%s: TEST UNIT READY fails, sense %s' % (what, s))
        st, _ = self.read(0, 1)
        s = self.m.sense(0) if st else '-'
        check(st != 0 and s == '02/3A/00', '%s: READ(10) fails, sense %s' % (what, s))
        st, _, _ = self.m.cmd(0, bytes([0x25]) + bytes(9), 8)
        s = self.m.sense(0) if st else '-'
        check(st != 0 and s == '02/3A/00', '%s: READ CAPACITY fails, sense %s' % (what, s))
        st, _, d = self.m.cmd(0, bytes([0x23, 0, 0, 0, 0, 0, 0, 0, 12, 0]), 12)
        s = self.m.sense(0) if st else '-'
        check(s == '02/3A/00', '%s: READ FORMAT CAPACITIES fails, sense %s' % (what, s))
        st, _, d = self.m.cmd(0, bytes([0x12, 0, 0, 0, 36, 0]), 36)
        check(st == 0 and d[8:16] == b'ALIENTEK', '%s: INQUIRY answers' % what)

    def tur(self):
        st, _, _ = self.m.cmd(0, bytes(6))
        return st

    def read(self, lba, n):
        st, _, d = self.m.cmd(0, struct.pack('>BBIBHB', 0x28, 0, lba, 0, n, 0), n * 512)
        return st, d


def healthy(l, boot, readme, what):
    """TUR (after at most one UNIT ATTENTION), the boot sector, README.TXT,
    *IDN?, all within a few seconds."""
    try:
        st = l.tur()
        if st != 0:
            s = l.m.sense(0)
            st = l.tur()
            if st != 0:
                check(False, '%s: TEST UNIT READY status %d (sense %s)' % (what, st, s))
                return
        st, b = l.read(0, 1)
        check(st == 0 and b == boot, '%s: boot sector right' % what)
        st, r = l.read(readme[0], 1)
        check(st == 0 and r[:len(readme[1])] == readme[1], '%s: README.TXT right' % what)
        idn = l.q('*IDN?')
        check(idn.startswith('Alientek'), '%s: *IDN? %s' % (what, idn))
    except Exception as e:
        check(False, '%s: the meter answers (%r)' % (what, e))


disk = os.path.join(OUT, TAG + '-disk.bin')
src = os.path.join(OUT, 'disk.bin')
shutil.copy(src if os.path.exists(src) else os.path.join(ROOT, 'stock/re/dumps/spi_dumps/dm40_msc_full_2026-10-13.bin'), disk)
emu = subprocess.Popen([sys.executable, os.path.join(HERE, 'run.py'), 'fwrs', '--app', A.app, '--usbip', '0.01',
                        '--usbip-port', str(A.port), '--uid', SERIAL, '--run-for', '3000', '--name', TAG, '--disk', disk],
                       cwd=ROOT, stdout=open(os.path.join(OUT, TAG + '.out'), 'w'), stderr=subprocess.STDOUT,
                       start_new_session=True)
try:
    l = Link()
    print('*IDN?', l.q('*IDN?'), flush=True)
    # The reference: boot sector, README.TXT (root entry 1: cluster, size).
    while l.tur() != 0:
        l.m.sense(0)
    st, boot = l.read(0, 1)
    bps, spc, rsvd, nfat, nroot = struct.unpack_from('<HBHBH', boot, 11)
    fatsz = struct.unpack_from('<H', boot, 22)[0]
    root = rsvd + nfat * fatsz
    data = root + nroot * 32 // 512
    st, r = l.read(root, 1)
    e = r[32:64]
    assert e[:11] == b'README  TXT', e
    c = struct.unpack_from('<H', e, 26)[0]
    st, txt = l.read(data + (c - 2) * spc, 1)
    readme = (data + (c - 2) * spc, txt[:64])
    for case in A.cases.split(','):
        print(case, flush=True)
        if case == 'serial':
            dd = l.d.device_descriptor()
            i = dd[16]
            sd = l.d.control_in(0x80, 6, 0x0300 | i, 0x0409, 255) if i else b''
            got = sd[2:sd[0]].decode('utf-16-le') if sd else ''
            check(got == SERIAL, 'USB iSerial (string %d) %r, expected %s' % (i, got, SERIAL))
            st, _, d = l.m.cmd(0, bytes([0x12, 1, 0x80, 0, 255, 0]), 255)
            check(st == 0 and d[1] == 0x80 and d[3] == 24 and d[4:28] == SERIAL.encode(),
                  'INQUIRY VPD 0x80: status %d, %r' % (st, bytes(d[:28])))
            st, _, d = l.m.cmd(0, bytes([0x12, 1, 0x00, 0, 255, 0]), 255)
            check(st == 0 and bytes(d[:6]) == bytes([0, 0, 0, 2, 0x00, 0x80]), 'INQUIRY VPD 0x00: status %d, %s'
                  % (st, bytes(d[:6]).hex()))
            st, _, d = l.m.cmd(0, bytes([0x12, 1, 0x83, 0, 255, 0]), 255)
            s = l.m.sense(0) if st else '-'
            check(st != 0 and s == '05/24/00', 'INQUIRY VPD 0x83: fails, sense %s' % s)
            st, _, d = l.m.cmd(0, bytes([0x12, 0, 0, 0, 36, 0]), 36)
            check(st == 0 and d[8:16] == b'ALIENTEK', 'standard INQUIRY still answers')
            idn = l.q('*IDN?').split(',')
            check(len(idn) == 4 and idn[2] == SERIAL, '*IDN? serial field %r' % (idn[2:3],))
            healthy(l, boot, readme, case)
        elif case == 'alloc-small':
            try:
                st, res, d = l.m.cmd(0, bytes([0x12, 0, 0, 0, 5, 0]), 5)
                check(st == 0 and len(d) == 5 and d[:1] == b'\0', 'INQUIRY, 5 bytes asked: %d bytes, status %d' % (len(d), st))
            except Exception as e:
                check(False, 'INQUIRY, 5 bytes asked: %r' % e)
                l.bot_reset()
            healthy(l, boot, readme, case)
        elif case == 'zero-alloc':
            try:
                l.cbw(bytes([0x03, 0, 0, 0, 0, 0]), 0)
                l.d.s.settimeout(5)
                csw = l.d.bulk_in(l.m.ep_in, 13)
                check(csw[:4] == b'USBS', 'REQUEST SENSE with no data phase: CSW %s' % csw.hex())
            except Exception as e:
                check(False, 'REQUEST SENSE with no data phase: no CSW (%r)' % e)
                l.port_reset()
            finally:
                l.d.s.settimeout(20)
            healthy(l, boot, readme, case)
        elif case == 'invalid-cbw':
            l.tur()
            l.d.bulk_out(l.m.ep_out, b'\x55' * 31)
            time.sleep(0.5)
            st, _ = l.d.submit(uc.DIR_IN, l.m.ep_in & 0x7F, in_len=13)
            check(st != 0, 'invalid CBW: the bulk IN endpoint stalls (URB status %d)' % (st - (1 << 32) if st > 1 << 31 else st))
            l.bot_reset()
            healthy(l, boot, readme, case)
        elif case in ('read-bot', 'read-port', 'ua-port'):
            if case == 'ua-port':
                l.t.write('DBG:SWIPE D,3')
                time.sleep(15)  # the shot is written, the volume refreshed: UNIT ATTENTION pending
            l.cbw(struct.pack('>BBIBHB', 0x28, 0, data, 0, 64, 0), 64 * 512)
            l.d.bulk_in(l.m.ep_in, 64)
            if case == 'read-bot':
                l.bot_reset()
            else:
                l.port_reset()
            healthy(l, boot, readme, case)
        elif case == 'stop-start':
            for b4, what in ((0x00, 'STOP'), (0x01, 'START')):
                st = l.ssu(b4)
                check(st == 0, '%s (1B, CB[4] %02X): status %d' % (what, b4, st))
                st = l.tur()
                check(st == 0, 'after %s: TEST UNIT READY passes (status %d)' % (what, st))
                if st:
                    l.m.sense(0)
            check(l.q('DBG:VDISK:EJECT?').split(',')[0] == '0', 'DBG:VDISK:EJECT? not ejected')
            healthy(l, boot, readme, case)
        elif case in ('eject-load', 'eject-port'):
            l.tur() or l.m.sense(0)
            st = l.ssu(0x02)
            check(st == 0, 'eject (1B, LoEj=1 Start=0): status %d' % st)
            e = l.q('DBG:VDISK:EJECT?')
            check(e.split(',')[0] == '1' and e.split(',')[2] == '2', 'DBG:VDISK:EJECT? %s' % e)
            l.no_medium('after the eject')
            if case == 'eject-load':
                l.bot_reset()
                l.no_medium('after a Bulk-Only reset')
                st = l.ssu(0x03)
                check(st == 0, 'load (1B, LoEj=1 Start=1): status %d' % st)
                st = l.tur()
                s = l.m.sense(0) if st else '-'
                check(st != 0 and s == '06/28/00', 'after the load: one UNIT ATTENTION (status %d, sense %s)' % (st, s))
                st = l.tur()
                check(st == 0, 'then TEST UNIT READY passes (status %d)' % st)
            else:
                l.port_reset()
                st = l.tur()
                if st:
                    l.m.sense(0)
            check(l.q('DBG:VDISK:EJECT?').split(',')[0] == '0', 'DBG:VDISK:EJECT? not ejected')
            healthy(l, boot, readme, case)
        up = l.q('DBG:PING?')
        print('  DBG:PING? %s' % up, flush=True)
    b = l.q('DBG:BOOT?')
    reason = bootinfo.fields(b).get('reason')
    check(reason == 'NONE', 'no watchdog reset (DBG:BOOT? reason %s)' % reason)
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
print('FAIL: %d checks' % len(FAIL) if FAIL else 'PASS')
sys.exit(1 if FAIL else 0)
