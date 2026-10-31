#!/usr/bin/env python3
"""Capacitance AUTO (dmm.rs poll_cap, issue #74): the range a reading ends on does not depend
on what was on the leads before. A small capacitor after a large one ends on its own range with
its value (before: 100 nF after 1 mF stayed on 600 uF, 3 .. 44 uF as final values); open leads
end on the 60 nF range (the lowest) whatever came before (before: after a short, or after 4.7 mF, the meter
stayed on "0L mF"); a short (the emulator's: the oscillator runs) reads ~0 on the 60 nF range; a large capacitor measures and keeps its
range (no hunt).

A scenario is a list of phases (fw uptime, from each phase input's marker: emu/emusync.py):
each phase connects its input at its start (CONF:CAP at +1 s of the first one), then
`CONF?;:FETC?` every 0.2 s. At the phase's end the last LAST polls must show the phase's range
and value: a number (within TOL), `ol` (+9.9E37), `small` (below 0.2 nF: the open leads'
stray, 0.04 nF in the emulator), `zero` (0); `stable` s: no range change in the phase's last
`stable` s; an optional 6th item: a range the phase must never show (overshoot).

  emu/hy3131/capdowntest.py --app firmware/target/native/thumbv7em-none-eabihf/release/opendm40 --port 47750
"""
import argparse
import os
import subprocess
import sys
import time

HERE = os.path.dirname(os.path.abspath(__file__))
EMU = os.path.dirname(HERE)
ROOT = os.path.dirname(EMU)
OUT = os.path.join(EMU, 'out')
sys.path.insert(0, EMU)
import emusync  # noqa: E402

BOOT_OFS = emusync.BOOT_S
T0 = 8.0
GAP = 2.0
TOL = 0.03
# The ramp ranges (600 uF up): the emulator's 470 uF reads +3.1 % (its ramp arithmetic,
# emu/hy3131/README.md: the stock's 100 uF reads +4 %).
TOL_RAMP = 0.05
LAST = 5
NF6 = 6e-8  # the lowest range: 60 nF (dmm::CAP_R_MIN)
# name, [(phase s, input, range F, value, stable s)]
SCEN = [
    ('1mF-100nF', [(14, 'c:1000e-6', None, None, 0), (20, 'c:100e-9', 6e-7, 100e-9, 0)]),
    ('470uF-22uF', [(14, 'c:470e-6', None, None, 0), (20, 'c:22e-6', 6e-5, 22e-6, 0)]),
    ('4.7mF-330uF', [(14, 'c:4.7e-3', None, None, 0), (20, 'c:330e-6', 6e-4, 330e-6, 0)]),
    ('open', [(20, 'open', NF6, 'small', 4)]),
    ('short-open', [(16, 'short', NF6, 'small', 8), (20, 'open', NF6, 'small', 8)]),
    ('4.7mF-open', [(30, 'c:4.7e-3', 6e-3, 4.7e-3, 10), (25, 'open', NF6, 'small', 8)]),
    ('10uF', [(20, 'c:10e-6', 6e-5, 10e-6, 10)]),
    ('100uF', [(25, 'c:100e-6', 6e-4, 100e-6, 12)]),
    ('470uF', [(25, 'c:470e-6', 6e-4, 470e-6, 12)]),
    ('1mF', [(30, 'c:1000e-6', 6e-3, 1e-3, 15)]),
    # An electrolytic's dielectric absorption (capda: the slope 60 % low right after the
    # discharge, tau 1 s; device 832f89a4 went to 60 mF on ~1 mF): no overshoot past 6 mF.
    ('1mF-da', [(30, 'c:1000e-6;capda:0.6;capdatau:1', 6e-3, 1e-3, 15, 6e-2)]),
    ('4.7mF-da', [(30, 'c:4.7e-3;capda:0.6;capdatau:1', 6e-3, 4.7e-3, 15, 6e-2)]),
    # Up and down ramps reading differently (capasym: +-2 %, the device's per-poll split;
    # device 40c25e2b's results alternated
    # 1227 / 974 uF): the shown value does not swing (2 % over the last 15 s).
    ('1mF-asym', [(35, 'c:1000e-6;capda:0;capasym:0.02', 6e-3, 1e-3, 15, 6e-2)]),
    ('4.7mF-asym', [(35, 'c:4.7e-3;capda:0;capasym:0.02', 6e-3, 4.7e-3, 10, 6e-2)]),
    # The device's open / short on a ramp range (#74): the node at its clamp / at 0 V, no
    # ramp. The emulator's open ramps (an aliased triangle), so AD1 is forced (SetAd1, ~1 V /
    # 0 V) for these phases; the oscillator ranges do not read AD1.
    ('4.7mF-clamp', [(30, 'c:4.7e-3;capasym:0', 6e-3, 4.7e-3, 0), (12, 'open|SetAd1 6000000', NF6, 'small', 4)]),
    ('4.7mF-short-open', [(30, 'c:4.7e-3', 6e-3, 4.7e-3, 0), (12, 'short|SetAd1 0', None, 'olsmall', 4),
                          (30, 'open', NF6, 'small', 5)]),
    # A resistor on the leads (device 138d145f: 50 ohm held the discharge at ~34 mV for good,
    # then 600 uF -> 60 uF -> 60 nF -> 600 uF with a grey 0): the discharge ends on the flat
    # node and the meter ends on a short's result (#74) without cycling. The emulator's
    # discharge decays to 0 and its oscillator reads a resistor as ~0, so AD1 is forced to the
    # device's 34 mV (SetAd1) and the ramp ranges are entered from DC V (AUTO starts on the
    # last value's range). `scpi:` parts are sent at the phase's start.
    ('r50-dcv', [(14, 'c:1000e-6', None, None, 0), (4, 'r:50|SetAd1 205350|scpi:CONF:VOLT:DC', None, None, 0),
                 (30, 'r:50|SetAd1 205350|scpi:CONF:CAP', None, 'olsmall', 8, NF6)]),
    ('r50-after-1mF', [(14, 'c:1000e-6', None, None, 0), (25, 'r:50|SetAd1 205350', None, 'olsmall', 8)]),
    ('r1k-dcv', [(14, 'c:1000e-6', None, None, 0), (4, 'r:1000|scpi:CONF:VOLT:DC', None, None, 0),
                 (30, 'r:1000|scpi:CONF:CAP', None, 'olsmall', 8)]),
    ('10mF', [(40, 'c:10e-3', 6e-2, 10e-3, 15)]),
    ('47mF', [(40, 'c:47e-3', 6e-2, 47e-3, 15)]),
]

ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
ap.add_argument('--app', required=True, help='opendm40 native ELF / image')
ap.add_argument('--port', type=int, default=47750, help='USB/IP port')
ap.add_argument('-v', '--verbose', action='store_true', help='print every range / value change')
ap.add_argument('names', nargs='*', help='scenarios (default all)')
A = ap.parse_args()
os.environ['USBIP_PORT'] = str(A.port)
import usbip_client as uc  # noqa: E402

scen = [s for s in SCEN if not A.names or s[0] in A.names]
os.makedirs(OUT, exist_ok=True)
tag = 'capdown-%d-%d' % (A.port, os.getpid())
script = os.path.join(OUT, tag + '.script')
# Virtual start of every phase: (scenario, phase) -> s.
starts = {}
t = T0
for k, (_, phases) in enumerate(scen):
    for j, ph in enumerate(phases):
        starts[k, j] = t
        t += ph[0]
    t += GAP
end = t + 2 + BOOT_OFS
with open(script, 'w') as f:
    for (k, j), s in sorted(starts.items(), key=lambda kv: kv[1]):
        spec, *mon = scen[k][1][j][1].split('|')
        f.write('%.3f dmm open;%s\n' % (s + BOOT_OFS, spec))
        for m in mon:
            if m.startswith('scpi:'):
                continue
            f.write('%.3f monitor sysbus.hy3131 %s\n' % (s + BOOT_OFS + 0.001, m))
cmd = [sys.executable, os.path.join(EMU, 'run.py'), 'fwrs', '--app', A.app, '--usbip', '0.01', '--usbip-port', str(A.port),
       '--run-for', '%.1f' % end, '--name', tag, '--script', script]
emu = subprocess.Popen(cmd, cwd=ROOT, stdout=subprocess.DEVNULL, stderr=subprocess.STDOUT, start_new_session=True)
bad = []


def num(r):
    try:
        return float(r)
    except ValueError:
        return float('nan')


def value_ok(v, want):
    if want is None:
        return True
    if want == 'ol':
        return v > 1e37
    if want == 'olsmall':
        # A short: OL on 60 mF (the device: no comparator turn), or ~0 on 60 nF
        # (the emulator's comparator still turns under the forced AD1).
        return v > 1e37 or 0 <= v < 0.2e-9
    if want == 'small':
        return 0 <= v < 0.2e-9
    if want == 'zero':
        return v == 0
    return abs(v - want) <= want * (TOL_RAMP if want > 60e-6 else TOL)


try:
    t = None
    for _ in range(150):
        try:
            t = uc.Usbtmc(uc.Dev('1-0'))
            break
        except Exception:
            time.sleep(2)
    if t is None:
        sys.exit('capdowntest: no USB/IP device on port %d' % A.port)
    mk = emusync.Markers(emusync.log_path(OUT, tag))
    if mk.wait_clock() is None:
        sys.exit('capdowntest: no clock marker (dwt: CYCCNT write) in %s' % mk.log)

    def q(line, timeout=60):
        t.write(line)
        return t.read(maxlen=8192, timeout=timeout).decode(errors='replace').strip()

    def up():
        return int(q('DBG:PING?').split(',')[0]) / 1000.0

    def wait_until(s):
        while up() < s:
            time.sleep(0.2)

    for k, (name, phases) in enumerate(scen):
        why = []
        for j, (dur, inp, rng, want, stable, *never) in enumerate(phases):
            w0 = mk.at(starts[k, j] + BOOT_OFS)
            if j == 0:
                wait_until(w0 + 1)
                q('CONF:CAP;*OPC?')
            for m in inp.split('|')[1:]:
                if m.startswith('scpi:'):
                    wait_until(w0 + 0.3)
                    q(m[5:] + ';*OPC?')
            rows = []
            last = None
            while True:
                conf, v = q('CONF?;:FETC?').split(';', 1)
                s = up() - w0
                r = num(conf.strip('"').split()[1].split(',')[0]) if ' ' in conf else float('nan')
                rows.append((s, r, num(v)))
                if A.verbose and (r, v) != last:
                    print('       %s %-6s %6.2f range %-10g %s' % (name, inp, s, r, v), flush=True)
                last = (r, v)
                if s > dur - 0.3:
                    break
                time.sleep(0.2)
            tail = rows[-LAST:]
            if rng is not None and any(abs(r - rng) > rng * 1e-3 for _, r, _ in tail):
                why.append('%s: range %s at the end (want %g)' % (inp, ' '.join('%g' % r for _, r, _ in tail), rng))
            if any(not value_ok(v, want) for _, _, v in tail):
                why.append('%s: values %s at the end (want %s)' % (inp, ' '.join('%g' % v for _, _, v in tail), want))
            for nr in never:
                if any(abs(r - nr) <= nr * 1e-3 for _, r, _ in rows):
                    why.append('%s: on the %g range at some point (overshoot)' % (inp, nr))
            if stable:
                seen = [r for s, r, _ in rows if s >= dur - stable]
                if len(set(seen)) > 1:
                    why.append('%s: range changes in the last %g s: %s' % (inp, stable, ' '.join('%g' % r for r in seen)))
                # No oscillation of the shown value (device 40c25e2b: 1227 / 974 / 1044 uF in turn).
                vals = sorted(v for s, _, v in rows if s >= dur - stable and 0 < v < 1e37)
                if isinstance(want, float) and vals and (vals[-1] - vals[0]) > 0.02 * vals[len(vals) // 2]:
                    why.append('%s: values %g .. %g in the last %g s (spread > 2 %%)' % (inp, vals[0], vals[-1], stable))
        print('%s %s: %s%s' % ('ok  ' if not why else 'FAIL', name, ' -> '.join(p[1].split('|')[0] for p in phases),
                               '' if not why else ': ' + '; '.join(why)), flush=True)
        if why:
            bad.append(name)
finally:
    try:
        os.killpg(emu.pid, 15)
    except Exception:
        pass
    emu.wait()
print('FAIL: %s' % ' '.join(bad) if bad else 'PASS')
sys.exit(1 if bad else 0)
