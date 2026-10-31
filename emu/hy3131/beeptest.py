#!/usr/bin/env python3
"""Continuity / diode beeper and the key & alert volume (UF-28; firmware/src/hy3131.rs
`set_beeper`, `beeper_sync`) in the emulator: opendm40 native + the HY3131 model, shorted
leads, SCPI over USB/IP.

The beeper is the HY3131 CMP pin buffer, R20 bit 2 (ENPCMPO): CFG_F11 writes R20 0x56,
`diode_beep` 0xb4 (on) / 0xb0 (off). At volume 0 % the bit must never go out; at 50 % as
before. Phases (each held HOLD s on the fw clock; R20 from DBG:SENTinel:REG?):
  diod0   VOL 0, DIOD on a short (class 0: the pattern is "on"): R20 0xB0 only
  cont0   CONT, VOL 0: R20 0x52
  cont50  VOL 50 while in CONT (no mode change): R20 0x56
  cont0b  VOL 0 while in CONT: R20 0x52
  diod50  VOL 50, DIOD: R20 0xB4
The model's write log (run.py --debug sysbus.hy3131, "R20 <= 0x..") is checked too: every
R20 write before the first one with bit 2 (the cont50 phase) has bit 2 clear, and those
include the diode pattern's writes (>= 20 x 0xB0) and the masked continuity row (0x52).
Exit status 0 = PASS. Results: emu/out/beep.json.

  emu/hy3131/beeptest.py --app firmware/target/native/thumbv7em-none-eabihf/release/opendm40 --port 64761
"""
import argparse
import json
import os
import re
import subprocess
import sys
import time

HERE = os.path.dirname(os.path.abspath(__file__))
EMU = os.path.dirname(HERE)
ROOT = os.path.dirname(EMU)
OUT = os.path.join(EMU, 'out')
ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
ap.add_argument('--app', help='opendm40 native image (default: run.py builds HEAD)')
ap.add_argument('--port', type=int, default=64761, help='USB/IP port (default 64761)')
A = ap.parse_args()
os.environ['USBIP_PORT'] = str(A.port)
sys.path.insert(0, EMU)
import usbip_client as uc  # noqa: E402

HOLD, SETTLE = 4.0, 1.5
RUN_FOR = 120
SHORT = 'open;r:0.5;noise:0'
# name, SCPI, the R20 every sample after SETTLE must show
PHASES = [
    ('diod0', ['SYST:BEEP:VOL 0', 'CONF:DIOD'], 0xB0),
    ('cont0', ['CONF:CONT'], 0x52),
    ('cont50', ['SYST:BEEP:VOL 50'], 0x56),
    ('cont0b', ['SYST:BEEP:VOL 0'], 0x52),
    ('diod50', ['SYST:BEEP:VOL 50', 'CONF:DIOD'], 0xB4),
]


def log_check(path):
    """R20 writes in the model's log: all before the first bit-2 write are clear of it."""
    w = []
    with open(path, errors='replace') as f:
        for l in f:
            m = re.search(r'R20 <= 0x([0-9A-F]{2})', l)
            if m:
                w.append(int(m.group(1), 16))
    first = next((i for i, x in enumerate(w) if x & 4), None)
    if first is None:
        return ['log: no R20 write with bit 2 (the VOL 50 phases)'], w
    pre = w[:first]
    bad = []
    if pre.count(0xB0) < 20:
        bad.append('log: %d x R20 0xB0 before the first bit-2 write (want >= 20)' % pre.count(0xB0))
    if 0x52 not in pre:
        bad.append('log: no masked continuity row (R20 0x52) before the first bit-2 write')
    if 0xB4 not in w[first:]:
        bad.append('log: no diode "on" write (0xB4) at VOL 50')
    return bad, w


def main():
    os.makedirs(OUT, exist_ok=True)
    name = 'beep.%d' % os.getpid()
    log = os.path.join(OUT, '%s.log' % name)
    cmd = [sys.executable, os.path.join(EMU, 'run.py'), 'fwrs', '--fast', '--usbip', '3',
           '--usbip-port', str(A.port), '--run-for', str(RUN_FOR), '--dmm', SHORT, '--name', name,
           '--debug', 'sysbus.hy3131'] \
        + (['--app', os.path.abspath(A.app)] if A.app else [])
    p = subprocess.Popen(cmd, cwd=ROOT, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)
    tm = None
    t_wall = time.time()
    while p.poll() is None and tm is None and time.time() - t_wall < 300:
        try:
            tm = uc.Usbtmc(uc.Dev('1-0'))
        except Exception as e:
            print('attach:', type(e).__name__, e, flush=True)
            time.sleep(1)
    bad, rows = [], []

    def q(s):
        tm.write(s)
        return tm.read(timeout=60).decode(errors='replace').strip()

    if tm is None:
        bad.append('no USB/IP attach')
    else:
        # R20 read back needs --features dbg-sent (DBG:SENTinel:REG?;
        # refused: an empty reply): without it the phases still run and
        # only the emulator log's R20 writes are checked.
        has_reg = bool(q('DBG:SENT:REG?'))
        if not has_reg:
            tm.write('*CLS')
            print('beeptest: DBG:SENTinel:REG? not in this build (--features dbg-sent): R20 read back skipped', flush=True)
        try:
            for ph, scpi, want in PHASES:
                for c in scpi:
                    tm.write(c)
                t0 = int(q('DBG:PING?').split(',')[0]) / 1000.0
                seen = 0
                while True:
                    t = int(q('DBG:PING?').split(',')[0]) / 1000.0
                    if t - t0 > HOLD:
                        break
                    if not has_reg:
                        seen += 1
                        time.sleep(0.2)
                        continue
                    reg = q('DBG:SENT:REG?')
                    r20 = int(reg[:2], 16)
                    rows.append({'ph': ph, 't': t, 'REG?': reg})
                    print('%-6s %6.2f R20 %02X' % (ph, t, r20), flush=True)
                    time.sleep(0.1)  # a few samples per virtual s, not thousands
                    if t - t0 < SETTLE:
                        continue
                    seen += 1
                    if r20 != want:
                        bad.append('%s %.2f R20 %02X, want %02X' % (ph, t, r20, want))
                if not seen:
                    bad.append('%s: no samples' % ph)
        except Exception as e:
            bad.append('link ended: %s' % e)
    p.kill()
    p.communicate()
    try:
        lb, w = log_check(log)
        bad += lb
        print('log: %d R20 writes' % len(w), flush=True)
    except OSError as e:
        bad.append('log: %s' % e)
    ok = not bad
    with open(os.path.join(OUT, 'beep.json'), 'w') as f:
        json.dump({'ok': ok, 'bad': bad, 'rows': rows}, f, indent=1)
    if ok:
        os.remove(log)  # tens of MB of debug lines
    print('RESULT beep %s %s' % ('OK' if ok else 'FAIL', '; '.join(bad[:6])), flush=True)
    return ok


if __name__ == '__main__':
    sys.exit(0 if main() else 1)
