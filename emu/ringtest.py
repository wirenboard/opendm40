#!/usr/bin/env python3
"""opendm40 scope: the coded record ring (firmware/src/cring.rs) at the slow
timebases, in the emulator, for one input signal per run.

  cd firmware && ./build.sh
  emu/ringtest.py --app firmware/target/native/thumbv7em-none-eabihf/release/opendm40 --signal sine|noise|mixed

  sine   1 kHz 1 Vpp (the model has no ADC noise: it codes small)
  noise  full-scale white noise (every block stays packed: the floor)
  mixed  sine and noise alternating every 1.5 s (--script `scope`)

On the scope page (comparator trigger off, as emu/wavtest.py):
  * roll at 200 ms/div (12.5 kS/s), STOP after 25 s: the record is all the
    ring keeps (sine: 12.5 samples a period, >= 80 000 samples; noise / mixed >= 65 520, all at
    most cring::NB blocks = 262 144);
  * DIGitize at 100 ms/div (25 kS/s): a record of 65 520 .. 180 000
    samples with the trigger point 52 156 + a few samples before its end
    (sine, mixed: the point on the sine's level, rising);
  * sine: the record is the input sine (frequency, no step larger than the
    sine's: no seam at the block boundaries);
  * MF > Settings > Save CSV of the DIGitize record: the DLG file decodes
    (tools/dlog.py) to the volts of WAV:DATA? row by row, t = 0 at the
    trigger sample.
Exit status 0 = all passed.
"""
import argparse
import math
import os
import struct
import subprocess
import sys
import time

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
ap.add_argument('--app', required=True)
ap.add_argument('--port', type=int, default=None)
ap.add_argument('--signal', choices=['sine', 'noise', 'mixed'], default='sine')
ap.add_argument('--mips', default='200')
A = ap.parse_args()
sys.path.insert(0, HERE)
import run  # noqa: E402
if A.port is None:
    A.port = run.free_port()
os.environ['USBIP_PORT'] = str(A.port)
import usbip_client as uc  # noqa: E402
sys.path.insert(0, os.path.join(ROOT, 'firmware', 'tools'))
import dlog  # noqa: E402

HZ, VPP = 1000.0, 1.0
SINE = 'sine,%g,%g' % (HZ, VPP)
NOISE = 'noise,1,1.5'
FAIL = []


def check(ok, what):
    print('  %s %s' % ('ok  ' if ok else 'FAIL', what), flush=True)
    if not ok:
        FAIL.append(what)


OUT = os.path.join(HERE, 'out')
os.makedirs(OUT, exist_ok=True)
tag = 'ringtest-%s-%d-%d' % (A.signal, A.port, os.getpid())
cmd = [sys.executable, os.path.join(HERE, 'run.py'), 'fwrs', '--mips', A.mips, '--app', A.app,
       '--usbip', '0.01', '--usbip-port', str(A.port), '--run-for', '5000', '--name', tag,
       '--log', os.path.join(OUT, tag + '.log'), '--scope', NOISE if A.signal == 'noise' else SINE]
if A.signal == 'mixed':
    script = os.path.join(OUT, tag + '.script')
    with open(script, 'w') as f:
        for k in range(400):
            f.write('%g scope %s\n' % (1.5 * (k + 1), NOISE if k % 2 == 0 else SINE))
    cmd += ['--script', script]
emu = subprocess.Popen(cmd, cwd=ROOT, stdout=subprocess.DEVNULL, stderr=subprocess.STDOUT, start_new_session=True)


def block(b):
    assert b[:1] == b'#', b[:16]
    d = int(b[1:2])
    n = int(b[2:2 + d]) if d else 0
    return b[2 + d:2 + d + n]


try:
    t = None
    for _ in range(150):
        try:
            t = uc.Usbtmc(uc.Dev('1-0'))
            break
        except Exception:
            time.sleep(2)
    if t is None:
        sys.exit('ringtest: no USB/IP device on port %d' % A.port)

    def q(line, timeout=120, maxlen=1 << 20):
        t.write(line)
        if '?' not in line:
            return None
        return t.read(maxlen=maxlen, timeout=timeout)

    def qs(line):
        return q(line).decode(errors='replace').strip()

    def up():
        return int(qs('DBG:PING?').split(',')[0])

    def wait(ms):
        e = up() + ms
        while up() < e:
            time.sleep(0.1)

    def wait_stop(s):
        e = up() + 1000 * s
        while up() < e:
            if qs('TRIG:STAT?') == 'STOP':
                return True
            time.sleep(0.2)
        return False

    def record():
        f = qs('WAV:PRE?').split(',')
        p = dict(xinc=float(f[4]), xorig=float(f[5]), yinc=float(f[7]), yorig=float(f[8]), yref=float(f[9]))
        d = block(q('WAV:DATA?'))
        codes = struct.unpack('<%dH' % (len(d) // 2), d)
        return p, codes, [(c - p['yref']) * p['yinc'] + p['yorig'] for c in codes]

    def scop_files():
        cat = qs('MMEM:CAT?')
        return {f.split(',')[0]: int(f.split(',')[2]) for f in cat.split('"')[1::2] if f.upper().startswith('SCOP')}

    def save_csv(p, codes, what):
        before = scop_files()
        q('DBG:KEY 3')  # MF popup
        wait(500)
        q('DBG:TAP 330,40')  # Settings tab
        wait(500)
        q('DBG:TAP 154,93')  # Save CSV
        new = {}
        e = up() + 120000
        while up() < e and not new:
            wait(1000)
            q('*CLS')
            new = {k: z for k, z in scop_files().items() if k not in before}
        check(len(new) == 1, '%s: Save CSV made %s' % (what, new))
        if not new:
            return
        name, size = next(iter(new.items()))
        d = bytearray()
        while len(d) < size:
            b = block(q('MMEM:DATA? "%s",%d,6000' % (name, len(d))))
            if not b:
                break
            d += b
        errors = []
        text, _ = dlog.decode(bytes(d), errors)
        rows = [r.split(',') for r in text.splitlines() if r[:1] in '+-']
        n = len(codes)
        ok = len(rows) == n and not errors
        dv = max(abs(float(rows[i][1]) - (p['yorig'] + p['yinc'] * (codes[i] - p['yref']))) for i in range(n)) if ok else float('nan')
        i0 = round(-p['xorig'] / p['xinc'])
        t0 = min(range(len(rows)), key=lambda i: abs(float(rows[i][0]))) if rows else -1
        check(ok and dv < 0.003 and t0 == i0, '%s: %s %d bytes (%.2f B/sample), %d rows = WAV:DATA? within %.4f V, t = 0 at row %d (trigger %d) %s'
              % (what, name, size, size / max(n, 1), len(rows), dv, t0, i0, errors))

    print('*IDN?', qs('*IDN?'), flush=True)
    wait(3000)
    q('DBG:HWTRIG:ENABLE 0')
    q('DBG:KEY 0')
    wait(500)
    q('DBG:TAP 341,107')  # the Oscilloscope tile
    wait(1500)
    for c in ('CHAN1:COUP DC', 'CHAN1:SCAL 0.2', 'CHAN1:OFFS 0', 'TRIG:SWE AUTO', 'TRIG:SLOP POS', 'TRIG:LEV 0',
              'WAV:FORM WORD', 'WAV:POIN MAX'):
        q(c)
    step = 2 * math.pi * HZ * VPP / 2

    # --- roll at 200 ms/div, 25 s
    q('TIM:SCAL 0.2')
    q('RUN')
    wait(25000)
    q('STOP')
    check(wait_stop(10), 'roll 200 ms/div: STOP')
    p, codes, v = record()
    n = len(codes)
    lo = 80000 if A.signal == 'sine' else 65520
    check(lo <= n <= 262144, 'roll 200 ms/div: %d samples (%d .. 262 144)' % (n, lo))
    if A.signal == 'sine':
        jump = max(abs(v[i + 1] - v[i]) for i in range(n - 1))
        check(jump < step * p['xinc'] + 0.05 * VPP, 'roll: no seam (largest step %.3f V, sine %.3f)' % (jump, step * p['xinc']))

    # --- DIGitize at 100 ms/div
    q('TIM:SCAL 0.1')
    q('DIG')
    check(wait_stop(30), 'DIGitize 100 ms/div: STOP')
    p, codes, v = record()
    n = len(codes)
    i0 = round(-p['xorig'] / p['xinc'])
    check(65520 <= n <= 180000, 'DIGitize 100 ms/div: %d samples (65 520 .. 180 000)' % n)
    if A.signal != 'noise':
        sstep = step * p['xinc']
        ok = 2 <= i0 < n - 2 and 52156 <= n - i0 <= 52156 + 2000
        check(ok and abs(v[i0]) < sstep + 0.05 * VPP and v[i0 + 1] > v[i0 - 1],
              'DIGitize: trigger sample %d of %d (%d before the end), %.3f V, rising' % (i0, n, n - i0, v[i0] if 0 <= i0 < n else float('nan')))
    if A.signal == 'sine':
        jump = max(abs(v[i + 1] - v[i]) for i in range(n - 1))
        ups = [i for i in range(1, n) if v[i - 1] < 0 <= v[i]]
        f = (len(ups) - 1) / ((ups[-1] - ups[0]) * p['xinc']) if len(ups) > 1 else 0
        check(jump < sstep + 0.05 * VPP and abs(f - HZ) < 0.02 * HZ, 'DIGitize: %.2f Hz, largest step %.3f V (sine %.3f)' % (f, jump, sstep))
    save_csv(p, codes, 'DIGitize record')
finally:
    try:
        os.killpg(emu.pid, 15)
    except Exception:
        pass
    emu.wait(timeout=30)

print('FAIL: %d checks' % len(FAIL) if FAIL else 'PASS')
sys.exit(1 if FAIL else 0)
