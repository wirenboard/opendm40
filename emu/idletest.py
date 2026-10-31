#!/usr/bin/env python3
"""opendm40 idle shutdown and host activity (firmware/src/app.rs
note_host_activity, DBG:IDLE): a SCPI command over USB or Bluetooth and a
USB disk READ / WRITE count as activity for the automatic power-off; the
idle dim counts touches and keys only.

  cd firmware && ./build.sh
  emu/idletest.py --app firmware/target/native/thumbv7em-none-eabihf/release/opendm40

DBG:IDLE 20000 sets a 20 s power-off time (instead of the Shutdown setting,
10 min at least). Cases (each runs --keep s, default 40, longer than that):
  usb   a USBTMC query (*IDN?) every 4 s: the meter stays on; DBG:IDLE?
        then shows the touch / key idle time grown past --keep s (host
        activity does not hold off the dim) and the shutdown idle time
        about zero;
  bt    a SCPI line over Bluetooth (run.py --bt-port) every 4 s, no USB:
        the meter stays on;
  msc   a USB disk READ(10) every 4 s, no SCPI: the meter stays on;
  inrush  the inrush page armed (INR:STAT ON, INR:COND? ARMED), then nothing
        for 60 s: the meter stays on, still armed (#144); INR:STAT OFF after;
  off   nothing for 30 s: the meter is off (USBTMC no longer answers). Runs
        last (the meter stays off).
"On" is checked with a USBTMC *IDN? after the case; the emulator's virtual
time for the case (DBG:PING? uptime) must be past the 20 s, so a slow
emulator cannot pass a case by not getting there.
Exit status 0 = PASS.
"""
import argparse
import os
import socket
import subprocess
import sys
import time

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
ap.add_argument('--app', required=True, help='opendm40 native ELF / image')
ap.add_argument('--port', type=int, default=None, help='USB/IP port (default: a free one, run.free_port)')
ap.add_argument('--bt-port', type=int, default=None, help='Bluetooth passthrough port (default: a free one)')
ap.add_argument('--cases', default='usb,bt,msc,inrush,off')
ap.add_argument('--keep', type=float, default=40.0, help='seconds of each keep-on case (default 40)')
A = ap.parse_args()
sys.path.insert(0, HERE)
import run  # noqa: E402  (emu/run.py: free_port)
A.port = A.port or run.free_port()
A.bt_port = A.bt_port or run.free_port()
os.environ['USBIP_PORT'] = str(A.port)
import usbip_client as uc  # noqa: E402

OFF_MS = 20000
OUT = os.path.join(HERE, 'out')
TAG = 'idle-%d-%d' % (A.port, os.getpid())
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
            sys.exit('idletest: no USB/IP device on port %d' % A.port)
        self.m = uc.Msc(self.d)
        self.t = uc.Usbtmc(self.d)

    def q(self, s, timeout=10):
        self.t.write(s)
        return self.t.read(timeout=timeout).decode(errors='replace').strip()

    def uptime(self):
        return int(self.q('DBG:PING?').split(',')[0])


def bt_link():
    for _ in range(30):
        try:
            s = socket.create_connection(('127.0.0.1', A.bt_port), timeout=5)
            s.settimeout(15)
            return s
        except OSError:
            time.sleep(1)
    raise IOError('no BT passthrough on %d' % A.bt_port)


def bt_query(s, cmd):
    s.sendall(cmd.encode() + b'\n')
    buf = b''
    while b'\n' not in buf:
        c = s.recv(4096)
        if not c:
            raise IOError('BT closed')
        buf += c
    return buf.partition(b'\n')[0].decode(errors='replace').strip()


def keep(l, name, poke):
    """DBG:IDLE 20000, then only `poke()` every 4 s for --keep s."""
    l.t.write('DBG:IDLE %d' % OFF_MS)
    t0 = l.uptime()
    end = time.time() + A.keep
    n = 0
    while time.time() < end:
        poke()
        n += 1
        time.sleep(4)
    try:
        up = l.uptime()
    except OSError as e:
        check(False, '%s: the meter answers after %.0f s of %s traffic only (%r)' % (name, A.keep, name, e))
        return None
    check(up - t0 > OFF_MS + 5000, '%s: virtual time of the case %d ms > the %d ms power-off time + 5 s'
          % (name, up - t0, OFF_MS))
    print('  ok   %s: the meter answers after %d ms of %s traffic only (%d pokes)' % (name, up - t0, name, n), flush=True)
    return up - t0


emu = subprocess.Popen([sys.executable, os.path.join(HERE, 'run.py'), 'fwrs', '--app', A.app, '--fast', '--usbip', '0.01',
                        '--usbip-port', str(A.port), '--bt-port', str(A.bt_port), '--run-for', '3000', '--name', TAG],
                       cwd=ROOT, stdout=open(os.path.join(OUT, TAG + '.out'), 'w'), stderr=subprocess.STDOUT,
                       start_new_session=True)
try:
    l = Link()
    print('*IDN?', l.q('*IDN?', timeout=30), flush=True)
    cap = l.m.capacity(0)
    for case in A.cases.split(','):
        print(case, flush=True)
        if case == 'usb':
            dt = keep(l, 'usb', lambda: l.q('*IDN?'))
            if dt:
                v = [int(x) for x in l.q('DBG:IDLE?').split(',')]
                check(v[0] >= dt, 'usb: DBG:IDLE? %s: the touch / key idle time ran on (>= %d ms: no dim hold-off)' % (v, dt))
                check(v[1] < 2000 and v[2] == OFF_MS, 'usb: DBG:IDLE? %s: shutdown idle time about 0, power-off %d ms'
                      % (v, OFF_MS))
        elif case == 'bt':
            s = bt_link()
            bt_query(s, '*IDN?')
            keep(l, 'bt', lambda: bt_query(s, '*IDN?'))
            s.close()
        elif case == 'msc':
            keep(l, 'msc', lambda: l.m.read(0, 0, 1, cap[1]))
        elif case == 'inrush':
            l.t.write('INR:STAT ON')
            end = time.time() + 60
            while l.q('INR:COND?') != 'ARMED' and time.time() < end:
                time.sleep(1)
            l.t.write('DBG:IDLE %d' % OFF_MS)
            t0 = l.uptime()
            # Twice the off case's wait: the armed stream slows the emulator
            # (23 s of virtual time in 30 s).
            time.sleep(2 * (OFF_MS / 1000 + 10))
            try:
                up = l.uptime()
            except OSError as e:
                check(False, 'inrush: the meter answers after %d s armed without traffic (%r)' % (2 * (OFF_MS / 1000 + 10), e))
                continue
            check(up - t0 > OFF_MS + 5000, 'inrush: armed %d ms without traffic > the %d ms power-off time + 5 s'
                  % (up - t0, OFF_MS))
            c = l.q('INR:COND?')
            check(c == 'ARMED', 'inrush: the meter answers, INR:COND? %s' % c)
            l.t.write('INR:STAT OFF')
        elif case == 'off':
            l.t.write('DBG:IDLE %d' % OFF_MS)
            time.sleep(OFF_MS / 1000 + 10)
            try:
                r = l.q('*IDN?', timeout=10)
            except OSError:
                r = None
            check(r is None, 'off: no traffic for %d s: the meter is off (*IDN? %s)'
                  % (OFF_MS / 1000 + 10, 'unanswered' if r is None else 'answered %r' % r))
        else:
            check(False, 'unknown case %s' % case)
except Exception as e:
    check(False, 'the meter answers: %r' % e)
finally:
    try:
        os.killpg(emu.pid, 15)
    except Exception:
        pass
    emu.wait()
print('FAIL: %d checks' % len(FAIL) if FAIL else 'PASS')
sys.exit(1 if FAIL else 0)
