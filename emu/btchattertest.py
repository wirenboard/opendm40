#!/usr/bin/env python3
"""opendm40: the Bluetooth module's start-up text is not SCPI, and a host's
first command is answered (device-check doc "f59bac2" item 6 and note 4).

  emu/btchattertest.py --app firmware/opendm40.bin --port 64530

The BT model (emu/peripherals/BtModule.cs) does what the device's module
does: on every start (power-up, AT+RESET) PC14 is high for 650 ms with no
host, and at 150 ms it prints its 7-line banner (BLE03:VER.., CBN:.., CBMAC:..,
CSN:.., CSMAC:.., CBSW:1, CSSW:1 = -102 -113 -113 -102 -113 -113 -113 for
the next client, device f59bac2); a host links no sooner than 320 ms after
a start, and its bytes pass 400 ms before PC14 rises.

  A. boot (start at power-up, again after the AT init's AT+RESET): a host
     links at 10 s and sends SYST:ERR:COUN? at once: the reply must come
     (device f59bac2: dropped while PC14 was low) and be 0; *IDN? works and
     a bad command of its own is queued (-113), so the queue is live.
  B. Settings: Bluetooth OFF, a host waits, ON: it links inside the start's
     PC14 pulse, right after the banner; its SYST:ERR:COUN? sent at once must
     be answered with 0.
  C. (in A's link) "AT" inside the firmware's data to a linked host: the DMM
     reads 2.1569 V DC (--dmm), so a stock cmd 09 reply frame ends in the
     value 21569 = 0x5441 little-endian, the bytes "AT", then the checksum.
     The BT model's AT parser once answered "OK" to that, which the
     firmware took as a host line (-113; the logfmttest --bt-port flake on
     LOG:DATA? blocks). Five cmd 09 frames: each reply carries "AT", and
     SYST:ERR:COUN? stays 0.
  D. a host sends MEAS:CAP?;MEAS:RES?;*IDN? and leaves at once; the next
     host's first reply is its own *IDN?, and no part of the old message
     follows (#122).
  E. (in A's link) MEAS:RES?, then MEAS:VOLT:DC? (a function change: ~3 s
     of settling) and, behind it at once, 1.5 KiB of SYST:REM (a no-op) and
     a *IDN?: more than the 1 KiB RX ring while it waits (#123). The query
     and *IDN? are answered, and the only error queued is -363 (input
     overrun), none for a line cut by the lost input. The BT model paces
     its bytes to the firmware at 115200 (RxByteUs), as the device's UART.
Exit status 0 = passed.
"""
import argparse
import os
import socket
import subprocess
import sys
import tempfile
import time

HERE = os.path.dirname(os.path.abspath(__file__))
ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
ap.add_argument('--app', required=True, help='opendm40 native image / ELF')
ap.add_argument('--port', type=int, default=4834, help='BT passthrough TCP port (default 4834)')
A = ap.parse_args()
sys.path.insert(0, HERE)
import usbip_client as uc  # noqa: E402
if uc.port_busy(A.port):
    sys.exit('btchattertest: port %d is in use: pass a free --port' % A.port)

mk = tempfile.mkdtemp(prefix='btchat-')
TAG = 'btchat-%d-%d' % (A.port, os.getpid())
B_OFF, B_ON = 40.0, 44.0
script = ['0 dmm dcv:2.1569', '10.0 shot %s/A.png' % mk,
          # B: MENU -> Settings, the Bluetooth row OFF, then ON.
          '%.1f key menu' % (B_OFF - 2), '%.1f tap 339 219' % (B_OFF - 1), '%.1f tap 240 178' % B_OFF,
          '%.1f shot %s/B.png' % (B_OFF + 0.5, mk), '%.1f tap 240 178' % B_ON]
sp = os.path.join(mk, 'script.txt')
open(sp, 'w').write('\n'.join(script) + '\n')
cmd = [sys.executable, os.path.join(HERE, 'run.py'), 'fwrs', '--app', A.app, '--script', sp, '--run-for', '120',
       '--bt-port', str(A.port), '--name', TAG, '--log', os.path.join(mk, 'renode.log')]
p = subprocess.Popen(cmd, stdout=open(os.path.join(mk, 'run.out'), 'w'), stderr=subprocess.STDOUT,
                     start_new_session=True)
bad = []


def check(ok, what):
    print('  %s %s' % ('ok  ' if ok else 'FAIL', what), flush=True)
    if not ok:
        bad.append(what)


def marker(name):
    while not os.path.exists(os.path.join(mk, name)):
        if p.poll() is not None:
            sys.exit('btchattertest: emulator ended before %s (%s)' % (name, os.path.join(mk, 'run.out')))
        time.sleep(0.2)


class Host:
    def __init__(self):
        self.s = socket.create_connection(('127.0.0.1', A.port), timeout=30)
        self.buf = b''

    def q(self, c):
        """Send at once; the reply line, None if none within 30 s (wall)."""
        self.s.sendall(c.encode() + b'\n')
        return self.recv_line()

    def recv_line(self):
        while b'\n' not in self.buf:
            try:
                d = self.s.recv(4096)
            except socket.timeout:
                return None
            if not d:
                return None
            self.buf += d
        line, _, self.buf = self.buf.partition(b'\n')
        return line.decode(errors='replace').strip()

    def frame(self, cmd, data=b''):
        """A stock request frame (AF 05 03 cmd len data sum); the reply
        frame DF 05 03 cmd len data sum, None if none within 30 s."""
        f = bytes([0xAF, 0x05, 0x03, cmd, len(data)]) + data
        self.s.sendall(f + bytes([-sum(f) & 0xFF]))
        while len(self.buf) < 5 or len(self.buf) < 6 + self.buf[4]:
            try:
                d = self.s.recv(4096)
            except socket.timeout:
                return None
            if not d:
                return None
            self.buf += d
        n = 6 + self.buf[4]
        r, self.buf = self.buf[:n], self.buf[n:]
        return r


try:
    marker('A.png')
    h = Host()
    n = h.q('SYST:ERR:COUN?')
    check(n is not None, 'A: the first command, sent at once, answered (%s)' % n)
    check(n is not None and n.lstrip('+') == '0', 'A: SYST:ERR:COUN? after the boot start text: %s (want 0)' % n)
    if n is not None and n.lstrip('+') not in ('', '0'):
        errs = [h.q('SYST:ERR?') for _ in range(int(n.lstrip('+')))]
        print('       queued: %s' % errs)
    i = h.q('*IDN?')
    check(i is not None and 'opendm40' in i, 'A: *IDN? %s' % i)
    h.s.sendall(b'NOSUCH:CMD\n')
    e = h.q('SYST:ERR?')
    check(e is not None and e.startswith('-113'), 'A: the host\'s own bad command: SYST:ERR? %s (want -113)' % e)
    at = 0
    for _ in range(5):
        r = h.frame(9)
        at += bool(r and r[:4] == b'\xdf\x05\x03\x09' and b'AT' in r)
        time.sleep(0.5)
    n = h.q('SYST:ERR:COUN?')
    check(at == 5, 'C: cmd 09 replies holding "AT" (the value 21569): %d of 5' % at)
    check(n is not None and n.lstrip('+') == '0', 'C: SYST:ERR:COUN? after them: %s (want 0)' % n)
    if n is not None and n.lstrip('+') not in ('', '0'):
        print('       queued: %s' % [h.q('SYST:ERR?') for _ in range(int(n.lstrip('+')))])
    # E: more than the RX ring behind a waiting MEAS:VOLT:DC? (#123).
    h.q('MEAS:RES?')
    h.s.sendall(b'MEAS:VOLT:DC?\n' + b'SYST:REM\n' * 170)
    r = h.q('*IDN?')
    check(r is not None and r.startswith('+2.15'), 'E: MEAS:VOLT:DC? before a 1.5 KiB burst answered: %s' % r)
    i = h.recv_line()
    check(i is not None and 'opendm40' in i, 'E: *IDN? after the burst: %s' % i)
    n = h.q('SYST:ERR:COUN?')
    errs = [h.q('SYST:ERR?') for _ in range(int(n.lstrip('+')))] if n and n.lstrip('+').isdigit() else [n]
    check(errs and all(e and e.startswith('-363') for e in errs), 'E: errors after the burst: %s (want -363 only)' % errs)
    h.s.close()
    marker('B.png')
    h = Host()   # linked by the model once the module is on again
    n = h.q('SYST:ERR:COUN?')
    check(n is not None, 'B: the first command after BT OFF / ON answered (%s)' % n)
    check(n is not None and n.lstrip('+') == '0', 'B: SYST:ERR:COUN? after the start text: %s (want 0)' % n)
    if n is not None and n.lstrip('+') not in ('', '0'):
        print('       queued: %s' % [h.q('SYST:ERR?') for _ in range(int(n.lstrip('+')))])
    h.s.close()
    # D: a host leaves while its query waits for readings (#122).
    time.sleep(1)
    h = Host()
    time.sleep(3)  # PC14 up (the model: 400 ms virtual after the connect)
    h.q('*OPC?')
    h.s.sendall(b'MEAS:CAP?;MEAS:RES?;*IDN?\n')
    time.sleep(0.05)
    h.s.close()
    time.sleep(1)
    h = Host()
    i = h.q('*IDN?')
    check(i is not None and i.startswith('Alientek'), 'D: the next host\'s first reply is its *IDN?: %s' % i)
    h.s.settimeout(4)
    try:
        extra = h.buf + h.s.recv(4096)
    except socket.timeout:
        extra = h.buf
    check(not extra, 'D: nothing of the old host\'s message after it: %r' % extra[:60])
    h.s.close()
finally:
    p.terminate()
    try:
        p.wait(30)
    except subprocess.TimeoutExpired:
        pass
    subprocess.run(['pkill', '-9', '-f', TAG])
print('btchattertest: %s' % ('PASS' if not bad else 'FAIL ' + '; '.join(bad)))
sys.exit(1 if bad else 0)
