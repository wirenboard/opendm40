#!/usr/bin/env python3
"""opendm40 Hold & Limits page: MENU and the title bar's Back go one page up
(#111, firmware/src/app.rs limits_back): to the range page (DMM range page,
Power Settings) it was opened from; to the multimeter when MF opened it
directly (continuity, diode, frequency). The Record page returns to the
page it was opened from: Hold & Limits (its Record row), or the range page /
temperature page / Power Settings (their Record button). Start on the Record
page returns to the multimeter, the range page's Record button turns red
while recording, Stop ends it.

  cd firmware && ./build.sh
  emu/limitsbacktest.py --app firmware/target/native/thumbv7em-none-eabihf/release/opendm40

Keys and taps through DBG:KEY / DBG:TAP, the page through DBG:VIEW?.
Exit status 0 = PASS.
"""
import argparse
import os
import subprocess
import sys
import time

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
OUT = os.path.join(HERE, 'out')
ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
ap.add_argument('--app', required=True, help='opendm40 native ELF / image')
ap.add_argument('--port', type=int, default=None, help='USB/IP port (default: a free one)')
A = ap.parse_args()
sys.path.insert(0, HERE)
import run  # noqa: E402
A.port = A.port or run.free_port()
os.environ['USBIP_PORT'] = str(A.port)
import usbip_client as uc  # noqa: E402

FAIL = []


def check(ok, what):
    print('  %s %s' % ('ok  ' if ok else 'FAIL', what), flush=True)
    if not ok:
        FAIL.append(what)


os.makedirs(OUT, exist_ok=True)
name = 'limitsback-%d-%d' % (A.port, os.getpid())
log = open(os.path.join(OUT, name + '.out'), 'w')
emu = subprocess.Popen([sys.executable, os.path.join(HERE, 'run.py'), 'fwrs', '--app', A.app, '--usbip', '0.01',
                        '--usbip-port', str(A.port), '--uid', run.emu_uid(A.port).hex().upper(), '--name', name,
                        '--vin', '1.0@0', '--run-for', '3000'],
                       cwd=ROOT, stdout=log, stderr=subprocess.STDOUT, start_new_session=True)
try:
    for _ in range(150):
        try:
            d = uc.Dev('1-0')
            break
        except Exception:
            time.sleep(2)
    else:
        raise RuntimeError('no USB/IP device on port %d' % A.port)
    t = uc.Usbtmc(d)

    def q(s):
        t.write(s)
        return t.read(timeout=30).decode(errors='replace').strip()

    def px(x, y):
        """One LCD pixel as RGB (DBG:LCD?, lcd.rs read_span's decode)."""
        t.write('DBG:LCD? %d,%d,1,1,3' % (x, y))
        b = t.read(timeout=30)
        nd = int(b[1:2])
        d = b[2 + nd:2 + nd + int(b[2:2 + nd])]
        wd = [d[i] | d[i + 1] << 8 for i in range(2, len(d), 2)]
        bs = bytearray()
        for v in wd:
            bs += bytes([v >> 8, v & 0xFF])
        return bs[0] & 0xF8, bs[1] & 0xFC, bs[2] & 0xF8

    def csv_state():
        return int(q('LOG:CSV?').split(',')[0])

    def up():
        return int(q('DBG:PING?').split(',')[0])

    def wait(ms):
        t0 = up()
        while up() < t0 + ms:
            time.sleep(0.05)

    def act(cmd):
        q(cmd + ';*OPC?')
        wait(600)
        return q('DBG:VIEW?').split(',')[0].strip('"')

    MENU, MF = 'DBG:KEY 0', 'DBG:KEY 3'
    BACK = 'DBG:TAP 40,15'          # the title bar's "< Back"
    HL_RANGE = 'DBG:TAP 64,259'     # range / temperature page: Limits
    HL_POWER = 'DBG:TAP 249,259'    # Power Settings: Limits
    RECORD = 'DBG:TAP 100,298'      # Hold & Limits: Record (the ninth row)
    REC_RANGE = 'DBG:TAP 181,259'   # range / temperature page: Record
    REC_POWER = 'DBG:TAP 338,259'   # Power Settings: Record
    START = 'DBG:TAP 240,178'       # Record page: Start / Stop recording (row 4)

    def path(conf, steps, what):
        q(conf + ';*OPC?')
        wait(2500)
        got = []
        for cmd, want in steps:
            v = act(cmd)
            got.append(v)
            if v != want:
                check(False, '%s: %s -> %r, want %r (path %s)' % (what, cmd, v, want, ' > '.join(got)))
                q(MENU + ';*OPC?')
                wait(600)
                return
        check(True, '%s: %s' % (what, ' > '.join(got)))
        # Back on the multimeter page for the next case.
        for _ in range(3):
            if q('DBG:VIEW?').split(',')[0].strip('"') == 'Multimeter':
                break
            act(MENU)

    path('CONF:VOLT:DC', [(MF, 'Range'), (HL_RANGE, 'Hold & Limits'), (MENU, 'Range'), (MENU, 'Multimeter')],
         'DC V: MENU on Hold & Limits')
    path('CONF:VOLT:DC', [(MF, 'Range'), (HL_RANGE, 'Hold & Limits'), (BACK, 'Range'), (MENU, 'Multimeter')],
         'DC V: Back on Hold & Limits')
    path('CONF:VOLT:DC', [(MF, 'Range'), (HL_RANGE, 'Hold & Limits'), (RECORD, 'Record'), (MENU, 'Hold & Limits'),
                          (MENU, 'Range')], 'DC V: Record > Hold & Limits > range page')
    path('CONF:TEMP', [(MF, 'Range'), (HL_RANGE, 'Hold & Limits'), (MENU, 'Range')], 'temperature page')
    path('CONF:CONT', [(MF, 'Hold & Limits'), (MENU, 'Multimeter')], 'continuity: MF opens Hold & Limits, MENU back')
    path('CONF:FREQ', [(MF, 'Hold & Limits'), (BACK, 'Multimeter')], 'frequency: Back to the multimeter')
    path('CONF:POW', [(MF, 'Range'), (HL_POWER, 'Hold & Limits'), (MENU, 'Range'), (MENU, 'Multimeter')],
         'power meter: MENU on Hold & Limits')
    # The Record button (range page, temperature page, Power Settings).
    path('CONF:VOLT:DC', [(MF, 'Range'), (REC_RANGE, 'Record'), (MENU, 'Range'), (MENU, 'Multimeter')],
         'DC V: Record button, MENU back to the range page')
    path('CONF:VOLT:DC', [(MF, 'Range'), (REC_RANGE, 'Record'), (BACK, 'Range'), (MENU, 'Multimeter')],
         'DC V: Record button, Back to the range page')
    path('CONF:CURR:DC', [(MF, 'Range'), (REC_RANGE, 'Record'), (MENU, 'Range')], 'DC A: Record button')
    path('CONF:RES', [(MF, 'Range'), (REC_RANGE, 'Record'), (BACK, 'Range')], 'ohm: Record button')
    path('CONF:CAP', [(MF, 'Range'), (REC_RANGE, 'Record'), (MENU, 'Range')], 'capacitance: Record button')
    path('CONF:TEMP', [(MF, 'Range'), (REC_RANGE, 'Record'), (MENU, 'Range')], 'temperature: Record button')
    path('CONF:POW', [(MF, 'Range'), (REC_POWER, 'Record'), (BACK, 'Range'), (MENU, 'Multimeter')],
         'power meter: Record button')
    # Start / Stop through the Record button.
    q('CONF:VOLT:DC;*OPC?')
    wait(2500)
    red = lambda c: c[0] >= 0xC0 and c[1] < 0x40 and c[2] < 0x40
    v = [act(MF), act(REC_RANGE)]
    check(v == ['Range', 'Record'], 'DC V: MF > Record: %s' % ' > '.join(v))
    v = act(START)
    check(v == 'Multimeter' and csv_state() != 0, 'Start: back on %r, LOG:CSV? %s' % (v, q('LOG:CSV?')))
    v = act(MF)
    c = px(130, 245)
    check(v == 'Range' and red(c), 'recording: range page Record button red (%s, pixel %s)' % (v, c))
    v = [act(REC_RANGE), act(START)]
    check(v == ['Record', 'Record'] and csv_state() in (0, 3), 'Stop: %s, LOG:CSV? %s' % (' > '.join(v), q('LOG:CSV?')))
    v = act(MENU)
    c = px(130, 245)
    check(v == 'Range' and not red(c), 'stopped: MENU to the range page, button not red (%s, pixel %s)' % (v, c))
    act(MENU)
except Exception as e:
    check(False, 'run: %r' % e)
finally:
    try:
        os.killpg(emu.pid, 15)
    except Exception:
        pass
    emu.wait()
print('FAIL: %d checks' % len(FAIL) if FAIL else 'PASS')
sys.exit(1 if FAIL else 0)
