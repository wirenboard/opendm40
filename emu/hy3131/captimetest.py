#!/usr/bin/env python3
"""Capacitance AUTO timing (dmm.rs poll_cap, PR #90: a fast rough value, refined later): 1 nF
on the 60 nF range first (AUTO starts on the last value's range: 60 nF, as after power-on),
DC V, the scenario's input, then CONF:CAP, then DBG:READout? every 0.1 s. Per scenario: the
time (fw uptime, from CONF:CAP) to the first grey value (kind G with a number), to the first
final (kind F), the final value, and the ranges passed. The checks: the last LAST finals' value
(a number within TOL, `small`: below 0.2 nF, `olsmall`: OL or below 0.2 nF), the first final
within `fmax` s (virtual time; PR #90 lists the times), and no final OL before the last range
(an over-range on the way up is dashes).

  emu/hy3131/captimetest.py --app firmware/target/native/thumbv7em-none-eabihf/release/opendm40 --port 47760
"""
import argparse
import math
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
GAP = 1.0
# Before each scenario's input: PRE_IN on CAP for PRE s (DC V from PRE - 0.5 s).
PRE_IN, PRE = 'c:1e-9', 1.5
LAST = 5
TOL = 0.05
# name, input (| SetAd1 monitor parts as capdowntest), phase s, want, first final within s
# [, (pre input, pre s)]
SCEN = [
    ('1nF', 'c:1e-9', 8, 1e-9, 1),
    ('10uF', 'c:10e-6', 10, 10e-6, 1.5),
    ('100uF', 'c:100e-6', 14, 100e-6, 7),
    ('1mF', 'c:1000e-6', 16, 1e-3, 7.5),
    # An electrolytic's dielectric absorption (capdowntest's model: the early slope 60 % low).
    ('1mF-da', 'c:1000e-6;capda:0.6;capdatau:1', 18, 1e-3, 8),
    # A narrow comparator window (capcmpw, the device's ~1 mF on 6 mF after a charge: class 1
    # turned every 1 .. 2 polls) and an AD1 reading averaged over 0.2 s (capvavg: a part of the
    # dV in the poll after a turn): the class widens, a final value comes.
    ('1mF-narrow', 'c:1000e-6;capcmpw:0.25;capvavg:0.2', 18, 1e-3, 10),
    ('1mF-narrow-da', 'c:1000e-6;capcmpw:0.25;capvavg:0.2;capda:0.6;capdatau:1', 18, 1e-3, 10),
    # Every class's window x 0.6 (device 4c31e6da, ~1 mF on 6 mF: class 2 swung ~35 mV, 3 polls
    # per half ramp, too short for a direction pair; the final came with class 3 at 11 .. 13 s).
    ('1mF-short', 'c:1000e-6;capcmpw:0.6;capvavg:0.2', 18, 1e-3, 7),
    # Absorption on a capacitor inside 600 uF: its early slope reads it over the range (the
    # chip options stay set: capcmpw / capvavg back to their defaults).
    ('470uF-da', 'c:470e-6;capcmpw:1;capvavg:0;capda:0.6;capdatau:1', 18, 470e-6, 9),
    ('open', 'open', 8, 'small', 1),
    # Open leads entered from a 1 mF value's range (AUTO starts on 6 mF: discharge, ramp).
    ('open-1mF', 'open', 10, 'small', 8, ('c:1000e-6', 9)),
    # A resistor (capdowntest r50-dcv: the device's 34 mV on the node, forced; the emulator's
    # oscillator reads it as ~0 on 60 nF).
    ('r47', 'r:47|SetAd1 205350', 20, 'olsmall', 1),
    # The same after a 1 mF value: AUTO starts on its ramp range.
    ('r47-1mF', 'r:47|SetAd1 205350', 20, 'olsmall', 7, ('c:1000e-6', 9)),
]

ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
ap.add_argument('--app', required=True, help='opendm40 native ELF / image')
ap.add_argument('--port', type=int, default=47760, help='USB/IP port')
ap.add_argument('-v', '--verbose', action='store_true', help='print every readout change')
ap.add_argument('names', nargs='*', help='scenarios (default all)')
A = ap.parse_args()
os.environ['USBIP_PORT'] = str(A.port)
import usbip_client as uc  # noqa: E402

scen = [s for s in SCEN if not A.names or s[0] in A.names]
os.makedirs(OUT, exist_ok=True)
tag = 'captime-%d-%d' % (A.port, os.getpid())
script = os.path.join(OUT, tag + '.script')
starts = []
pres = [s[5] if len(s) > 5 else (PRE_IN, PRE) for s in scen]
t = T0
for s, (_, pre) in zip(scen, pres):
    starts.append(t)
    t += pre + 1.0 + s[2] + GAP
end = t + 2 + BOOT_OFS
with open(script, 'w') as f:
    for s, (_, inp, *_r), (pin, pre) in zip(starts, scen, pres):
        spec, *mon = inp.split('|')
        f.write('%.3f dmm open;%s\n' % (s + BOOT_OFS, pin))
        f.write('%.3f dmm open;%s\n' % (s + pre + BOOT_OFS, spec))
        for m in mon:
            f.write('%.3f monitor sysbus.hy3131 %s\n' % (s + pre + BOOT_OFS + 0.001, m))
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
    if want == 'small':
        return 0 <= v < 0.2e-9
    if want == 'olsmall':
        return v > 1e37 or 0 <= v < 0.2e-9
    return abs(v - want) <= want * TOL


try:
    t = None
    for _ in range(150):
        try:
            t = uc.Usbtmc(uc.Dev('1-0'))
            break
        except Exception:
            time.sleep(2)
    if t is None:
        sys.exit('captimetest: no USB/IP device on port %d' % A.port)
    mk = emusync.Markers(emusync.log_path(OUT, tag))
    if mk.wait_clock() is None:
        sys.exit('captimetest: no clock marker (dwt: CYCCNT write) in %s' % mk.log)

    def q(line, timeout=60):
        t.write(line)
        return t.read(maxlen=8192, timeout=timeout).decode(errors='replace').strip()

    def up():
        return int(q('DBG:PING?').split(',')[0]) / 1000.0

    def wait_until(s):
        while up() < s:
            time.sleep(0.05)

    print('%-8s %7s %7s  %-12s %s' % ('', 'grey s', 'final s', 'final', 'ranges'), flush=True)
    for k, (name, inp, dur, want, fmax, *_p) in enumerate(scen):
        pre = pres[k][1]
        w0 = mk.at(starts[k] + BOOT_OFS)
        wait_until(w0 + 0.1)
        # 1 nF on the 60 nF range (manual: its value is the last one, AUTO starts there).
        q('CONF:CAP 6E-8;*OPC?' if pres[k][0] == PRE_IN else 'CONF:CAP;*OPC?')
        wait_until(w0 + pre - 0.5)
        q('CONF:VOLT:DC;*OPC?')
        wait_until(w0 + pre + 1.0)
        q('CONF:CAP;*OPC?')
        c0 = up()
        t_g = t_f = None
        finals = []
        ranges = []
        last = None
        early_ol = False
        while True:
            ro = q('DBG:READout?').split(',')
            cap = q('DBG:CAP?').split(',')
            s = up() - c0
            kind, v = ro[0], num(ro[2])
            r = int(num(cap[0])) if cap and not math.isnan(num(cap[0])) else -1
            if not ranges or ranges[-1] != r:
                ranges.append(r)
            if A.verbose and (kind, ro[2], r) != last:
                print('       %s %6.2f r%d %s %s' % (name, s, r, kind, ro[2]), flush=True)
            last = (kind, ro[2], r)
            if kind == 'G' and t_g is None and not math.isnan(v):
                t_g = s
            if kind == 'F':
                if t_f is None:
                    t_f = s
                finals.append((s, r, v))
            if s > dur:
                break
            time.sleep(0.1)
        why = []
        tail = finals[-LAST:]
        if len(tail) < LAST:
            why.append('%d finals' % len(finals))
        elif any(not value_ok(v, want) for _, _, v in tail):
            why.append('finals %s (want %s)' % (' '.join('%g' % v for _, _, v in tail), want))
        if t_f is None or t_f > fmax:
            why.append('first final at %s s (want <= %g)' % ('-' if t_f is None else '%.2f' % t_f, fmax))
        if tail and want not in ('olsmall',):
            rl = tail[-1][1]
            ols = [(s, r) for s, r, v in finals if v > 1e37 and r != rl]
            if ols:
                why.append('final OL on r%d at %.2f s' % (ols[0][1], ols[0][0]))
        print('%-8s %7s %7s  %-12s %s%s' % (name, '-' if t_g is None else '%.2f' % t_g, '-' if t_f is None else '%.2f' % t_f,
                                          '%g' % tail[-1][2] if tail else '-', ' '.join('r%d' % r for r in ranges),
                                          '' if not why else '  FAIL: ' + '; '.join(why)), flush=True)
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
