#!/usr/bin/env python3
"""Eject of the opendm40 USB volume through the real Linux usb-storage path:
the emulator's USB/IP device attached to the kernel (vhci_hcd, `sudo usbip
attach`), mounted and ejected with udisks / gio as a desktop does.
(src/msc.rs: after START STOP UNIT LoEj=1 Start=0 the medium stays absent,
02/3A/00, until a re-plug; Windows used to mount it again at once.)

  cd firmware && ./build.sh
  emu/ejecttest.py --app firmware/target/native/thumbv7em-none-eabihf/release/opendm40 [--port P]

Steps (exit status 0 = PASS):
  live       "Files live" (read-only) volume: mounted, README.TXT read,
             `gio mount -e` (unmount + eject): DBG:VDISK:EJECT? = 1 (last
             CB[4] 02), udisks MediaAvailable false, no re-mount for 10 s,
             `udisksctl mount` fails; re-plug (usbip detach + attach): the
             disk is back, mounts, README.TXT equal;
  poweroff   `udisksctl power-off` (STOP without LoEj, then the USB device
             removed): the disk goes away; re-plug: back;
  install    "PC can edit" (DBG:VDISK:RO 0 + re-plug): rw mount, the built
             update image copied as dm40_opendm40_v0.1.bin and ejected at once (`gio
             mount -e`: flush, umount, Allow, eject): the meter commits it
             (DBG:VDISK:WR? firmware files committed + 1, state ask / asked),
             the medium stays ejected; re-plug: back, still writable.
The usbtmc of the emulator is used for SCPI (DM40_USBTMC=emu: the one
under vhci_hcd; the real meter's is never opened), the block device is the
one under vhci_hcd only. `usbip attach` only connects to TCP 3240: a proxy
here listens there and forwards to the emulator on --port. The emulator gets
a chip UID of its own (run.py --uid, emu_uid: the unit's UID with bytes 4 / 9
= the port), so opendm40's USB serial (the UID as hex, firmware/src/uid.rs)
differs from the real meter's: udisks takes two disks with the same vendor /
model / serial for one multipath drive, and with a real DM40 plugged in a
drive-level Eject / PowerOff could then act on the real meter. (Until
2026-10-23 every unit said "0001" and this proxy rewrote it to "EMU1".)
Every attach checks that the emulator's disk has a udisks drive of its own,
named with the emulator's serial and not the unit's (else the test stops
before any eject). Needs passwordless `sudo -n usbip
attach|detach|port`, udisks, gio; 3240 must be free.
"""
import argparse
import glob
import os
import shutil
import socket
import subprocess
import sys
import threading
import time

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
ap.add_argument('--app', required=True, help='opendm40 native ELF')
ap.add_argument('--image', default=os.path.join(ROOT, 'firmware/opendm40.bin'),
                help='update file copied in the install step (default firmware/opendm40.bin)')
ap.add_argument('--port', type=int, default=None,
                help='emulator USB/IP port (default: a free one in 61000-64999, run.free_port)')
ap.add_argument('--steps', default='live,poweroff,install')
A = ap.parse_args()
os.environ['DM40_USBTMC'] = 'emu'
sys.path.insert(0, os.path.join(ROOT, 'firmware/tools'))
import dm40dbg  # noqa: E402
sys.path.insert(0, HERE)
import run  # noqa: E402  (emu/run.py: emu_uid, UID, free_port)
import bootinfo  # noqa: E402
if A.port is None:
    A.port = run.free_port()
print('emulator USB/IP port %d' % A.port, flush=True)

# The emulator's serial (opendm40: the chip UID as 24 hex digits, memory order)
# and the real unit's, which the emulator's udisks drive must never carry.
EMU_SERIAL = run.emu_uid(A.port).hex().upper()
UNIT_SERIAL = run.read(run.UID).hex().upper() if os.path.exists(run.UID) else None
assert EMU_SERIAL != UNIT_SERIAL

OUT = os.path.join(HERE, 'out')
TAG = 'eject-%d-%d' % (A.port, os.getpid())
FAIL = []


def check(ok, what):
    print('  %s %s' % ('ok  ' if ok else 'FAIL', what), flush=True)
    if not ok:
        FAIL.append(what)


def sh(*a, ok=False):
    r = subprocess.run(a, capture_output=True, text=True, timeout=120)
    if ok and r.returncode:
        raise RuntimeError('%s: %s' % (' '.join(a), (r.stderr or r.stdout).strip()))
    return r


def vhci_block():
    """sdX of the disk under vhci_hcd (never the real meter's)."""
    for b in glob.glob('/sys/block/sd*'):
        if 'vhci_hcd' in os.path.realpath(os.path.join(b, 'device')):
            return os.path.basename(b)
    return None


def vhci_ports():
    r = sh('sudo', '-n', 'usbip', 'port')
    return [int(l.split()[1].rstrip(':')) for l in r.stdout.splitlines()
            if l.startswith('Port ') and l.split()[1].rstrip(':').isdigit()]


class SafetyError(Exception):
    pass


CONNS = []  # the proxy's open sockets (kernel side, emulator side)


def unplug():
    """Close the proxy's connections: the cable pulled for both sides (the
    emulator releases the import; after `udisksctl power-off` the kernel no
    longer detaches the port by itself)."""
    while CONNS:
        c = CONNS.pop()
        try:
            c.shutdown(socket.SHUT_RDWR)
        except OSError:
            pass
        c.close()


def proxy_main(lport, uport, stop):
    """TCP 127.0.0.1:lport -> 127.0.0.1:uport for `usbip attach` (the
    emulator's own serial needs no rewrite: run.py emu_uid)."""
    ls = socket.socket()
    ls.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    ls.bind(('127.0.0.1', lport))
    ls.listen(4)
    ls.settimeout(0.5)

    def pump(a, b):
        try:
            while True:
                d = a.recv(65536)
                if not d:
                    break
                b.sendall(d)
        except OSError:
            pass
        try:
            b.shutdown(socket.SHUT_WR)
        except OSError:
            pass

    while not stop.is_set():
        try:
            c, _ = ls.accept()
        except socket.timeout:
            continue
        u = socket.create_connection(('127.0.0.1', uport))
        CONNS.extend((c, u))
        for s_ in (c, u):
            s_.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)
        threading.Thread(target=pump, args=(c, u), daemon=True).start()
        threading.Thread(target=pump, args=(u, c), daemon=True).start()
    ls.close()


class Meter:
    def __init__(self):
        self.port = None
        self.dev = None
        self.ln = None
        self.drive = None

    def attach(self):
        before = set(vhci_ports())
        for _ in range(20):
            r = sh('sudo', '-n', 'usbip', 'attach', '-r', '127.0.0.1', '-b', '1-0')
            if r.returncode == 0:
                break
            time.sleep(3)
        else:
            raise RuntimeError('usbip attach: %s' % r.stderr.strip())
        for _ in range(60):
            self.dev = vhci_block()
            path = dm40dbg.find_dev(required=False)
            if self.dev and path:
                break
            time.sleep(0.5)
        else:
            raise RuntimeError('no block device / usbtmc under vhci_hcd')
        new = set(vhci_ports()) - before
        self.port = min(new) if new else None
        self.check_drive()
        time.sleep(0.5)
        self.ln = dm40dbg.Link(path)
        print('  attached: vhci port %s, /dev/%s, %s' % (self.port, self.dev, path), flush=True)

    def check_drive(self):
        """The udisks drive of the emulator's disk must be its own: udisks
        takes block devices with the same vendor / model / serial for one
        (multipath) drive, and a drive-level Eject / PowerOff could then act
        on the real meter's disk. The emulator has a serial of its own
        (EMU_SERIAL, run.py --uid); refuse to go on unless the drive is named
        with it, not with the unit's, and no other disk shares it."""
        drive = None
        for _ in range(20):
            r = sh('udisksctl', 'info', '-b', '/dev/' + self.dev)
            for l in r.stdout.splitlines():
                if l.strip().startswith('Drive:'):
                    drive = l.split(':', 1)[1].strip().strip("'")
            if drive and drive != '/':
                break
            time.sleep(0.5)
        others = []
        for b in glob.glob('/sys/block/sd*'):
            n = os.path.basename(b)
            if n == self.dev:
                continue
            r = sh('udisksctl', 'info', '-b', '/dev/' + n)
            if ("'%s'" % drive) in r.stdout:
                others.append(n)
        if (not drive or drive == '/' or EMU_SERIAL not in drive or (UNIT_SERIAL and UNIT_SERIAL in drive)
                or others):
            raise SafetyError('udisks drive of /dev/%s is %s, shared with %s: no drive-level eject / power-off'
                              % (self.dev, drive, others))
        self.drive = drive

    def eject(self, mp):
        """As a file manager: `gio mount -e` (unmount, then udisks' drive
        Eject); when gio cannot, udisksctl unmount + Drive.Eject over D-Bus.
        Only after check_drive (the drive is the emulator's alone)."""
        assert self.drive and EMU_SERIAL in self.drive
        r = sh('/usr/bin/gio', 'mount', '-e', mp)
        print('  gio mount -e %s: %d %s' % (mp, r.returncode, (r.stderr or r.stdout).strip()[:100]), flush=True)
        if r.returncode == 0:
            return True
        if self.mountpoint():
            sh('udisksctl', 'unmount', '--no-user-interaction', '-b', '/dev/' + self.dev)
        r = sh('/usr/bin/gdbus', 'call', '--system', '--dest', 'org.freedesktop.UDisks2', '--object-path', self.drive,
               '--method', 'org.freedesktop.UDisks2.Drive.Eject', '{}')
        print('  udisks Drive.Eject %s: %d %s' % (self.drive, r.returncode, (r.stderr or r.stdout).strip()[:100]), flush=True)
        return r.returncode == 0

    def detach(self):
        if self.ln:
            try:
                os.close(self.ln.fd)
            except OSError:
                pass
            self.ln = None
        # Our port even when `usbip port` no longer lists it (after a
        # power-off the USB device is gone but the connection stays).
        ports = vhci_ports()
        p = self.port if self.port is not None else (ports[0] if len(ports) == 1 else None)
        if p is not None:
            r = sh('sudo', '-n', 'usbip', 'detach', '-p', str(p))
            if r.returncode:
                print('  usbip detach -p %d: %s' % (p, (r.stderr or r.stdout).strip()[:100]), flush=True)
        time.sleep(0.5)
        unplug()
        for _ in range(40):
            if not vhci_block():
                break
            time.sleep(0.5)
        self.port = None
        self.drive = None
        time.sleep(1)

    def replug(self):
        self.detach()
        self.attach()

    def q(self, s):
        return self.ln.query(s).decode(errors='replace').strip()

    def mountpoint(self):
        r = sh('/usr/bin/findmnt', '-n', '-o', 'TARGET', '-S', '/dev/' + self.dev)
        return r.stdout.strip().splitlines()[0] if r.stdout.strip() else None

    def mount(self):
        """The mount point (GNOME's automount, or udisksctl mount)."""
        for _ in range(10):
            m = self.mountpoint()
            if m:
                return m
            time.sleep(0.5)
        sh('udisksctl', 'mount', '--no-user-interaction', '-b', '/dev/' + self.dev, ok=True)
        return self.mountpoint()

    def media_available(self):
        r = sh('udisksctl', 'info', '-d', os.path.basename(self.drive))
        for l in r.stdout.splitlines():
            if l.strip().startswith('MediaAvailable:'):
                return l.split(':', 1)[1].strip() == 'true'
        return None

    def options(self):
        return sh('/usr/bin/findmnt', '-n', '-o', 'OPTIONS', '-S', '/dev/' + self.dev).stdout.strip()


def ejected_ok(m, what, last=2):
    """After an eject: the meter says ejected, the medium stays gone."""
    e = None
    for _ in range(20):
        e = m.q('DBG:VDISK:EJECT?').split(',')
        if e[0] == '1':
            break
        time.sleep(0.5)
    check(e[0] == '1' and e[2] == str(last), '%s: DBG:VDISK:EJECT? %s (ejected, last CB[4] %d)' % (what, ','.join(e), last))
    time.sleep(10)  # udisks / the kernel poll the removable disk meanwhile
    check(m.mountpoint() is None, '%s: not mounted again after 10 s' % what)
    check(m.media_available() is False, '%s: udisks MediaAvailable false' % what)
    size = open('/sys/block/%s/size' % m.dev).read().strip()
    check(size == '0', '%s: /sys/block/%s/size %s' % (what, m.dev, size))
    r = sh('udisksctl', 'mount', '--no-user-interaction', '-b', '/dev/' + m.dev)
    check(r.returncode != 0, '%s: udisksctl mount fails (%s)' % (what, (r.stderr or r.stdout).strip()[:80]))
    check(m.q('DBG:VDISK:EJECT?').split(',')[0] == '1', '%s: still ejected' % what)


def back_ok(m, what, readme):
    check(m.q('DBG:VDISK:EJECT?').split(',')[0] == '0', '%s: DBG:VDISK:EJECT? not ejected' % what)
    mp = m.mount()
    check(mp is not None, '%s: mounted at %s (%s)' % (what, mp, m.options() if mp else '-'))
    if mp:
        t = open(os.path.join(mp, 'README.TXT'), 'rb').read()
        check(t == readme, '%s: README.TXT equal' % what)
    return mp


ss = subprocess.run(['ss', '-ltn'], capture_output=True, text=True).stdout
if ':3240 ' in ss:
    sys.exit('ejecttest: TCP 3240 in use (another usbip test?)')
if vhci_block():
    sys.exit('ejecttest: a vhci_hcd disk is attached already')
os.makedirs(OUT, exist_ok=True)
disk = os.path.join(OUT, TAG + '-disk.bin')
src = os.path.join(OUT, 'disk.bin')
shutil.copy(src if os.path.exists(src) else os.path.join(ROOT, 'stock/re/dumps/spi_dumps/dm40_msc_full_2026-10-13.bin'), disk)
emu = subprocess.Popen([sys.executable, os.path.join(HERE, 'run.py'), 'fwrs', '--app', A.app, '--fast', '--usbip', '0.01',
                        '--usbip-port', str(A.port), '--uid', EMU_SERIAL, '--run-for', '3600', '--name', TAG, '--disk', disk],
                       cwd=ROOT, stdout=open(os.path.join(OUT, TAG + '.out'), 'w'), stderr=subprocess.STDOUT,
                       start_new_session=True)
proxy = None
m = Meter()
try:
    for _ in range(150):
        if (':%d ' % A.port) in subprocess.run(['ss', '-ltn'], capture_output=True, text=True).stdout:
            break
        time.sleep(2)
    time.sleep(5)
    stop = threading.Event()
    proxy = threading.Thread(target=proxy_main, args=(3240, A.port, stop), daemon=True)
    proxy.start()
    time.sleep(1)
    m.attach()
    print('*IDN?', m.q('*IDN?'), flush=True)
    mp = m.mount()
    readme = open(os.path.join(mp, 'README.TXT'), 'rb').read()
    print('mounted at %s (%s), README.TXT %d bytes' % (mp, m.options(), len(readme)), flush=True)
    for step in A.steps.split(','):
        print(step, flush=True)
        if step == 'live':
            mp = m.mount()
            check('ro' in m.options().split(','), 'live: mounted read-only')
            check(m.eject(mp), 'live: ejected')
            ejected_ok(m, 'live')
            m.replug()
            back_ok(m, 'live, re-plugged', readme)
        elif step == 'poweroff':
            mp = m.mount()
            sh('udisksctl', 'unmount', '--no-user-interaction', '-b', '/dev/' + m.dev)
            n0 = int(m.q('DBG:VDISK:EJECT?').split(',')[1])
            r = sh('udisksctl', 'power-off', '--no-user-interaction', '-b', '/dev/' + m.dev)
            check(r.returncode == 0, 'poweroff: udisksctl power-off (%s)' % (r.stderr or r.stdout).strip()[:80])
            time.sleep(3)
            check(vhci_block() is None, 'poweroff: the disk is gone (%s)' % vhci_block())
            m.replug()
            e = m.q('DBG:VDISK:EJECT?').split(',')
            print('  DBG:VDISK:EJECT? %s (START STOP UNITs before: %d)' % (','.join(e), n0), flush=True)
            back_ok(m, 'poweroff, re-plugged', readme)
        elif step == 'install':
            check(m.q('DBG:VDISK:RO 0') == 'OK', 'install: DBG:VDISK:RO 0')
            m.replug()
            check(m.q('DBG:VDISK:RO?') == '0,0', 'install: writable in force')
            mp = m.mount()
            check('rw' in m.options().split(','), 'install: mounted read-write (%s)' % m.options())
            wr0 = m.q('DBG:VDISK:WR?').split(',')
            shutil.copyfile(A.image, os.path.join(mp, 'dm40_opendm40_v0.1.bin'))
            t0 = time.time()
            check(m.eject(mp), 'install: copied, ejected at once (%.1f s)' % (time.time() - t0))
            wr = wr0
            for _ in range(120):
                wr = m.q('DBG:VDISK:WR?').split(',')
                if int(wr[5]) > int(wr0[5]) or int(wr[6]) > int(wr0[6]):
                    break
                time.sleep(0.5)
            check(int(wr[5]) == int(wr0[5]) + 1 and wr[12] in ('1', '2'),
                  'install: the firmware file committed, the meter asks (DBG:VDISK:WR? %s)' % ','.join(wr))
            ejected_ok(m, 'install')
            m.replug()
            check(m.q('DBG:VDISK:RO?') == '0,0', 'install: still writable after the re-plug')
            back_ok(m, 'install, re-plugged', readme)
    b = m.q('DBG:BOOT?')
    check('WDT' not in str(bootinfo.fields(b).get('reason')), 'no watchdog reset (DBG:BOOT? %s)' % b)
except Exception as e:
    check(False, 'the test ran: %r' % e)
finally:
    try:
        if m.dev and m.mountpoint():
            sh('udisksctl', 'unmount', '--no-user-interaction', '-f', '-b', '/dev/' + m.dev)
        m.detach()
    except Exception as e:
        print('detach: %r' % e)
    if proxy:
        stop.set()
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
