#!/usr/bin/env python3
"""opendm40 screen transitions: what the panel shows while a page, a popup
or a DMM function is drawn (owner report 2026-10-29: "jerky, ugly screen
updates when switching modes, opening menus").

  cd firmware && ./build.sh --features mute
  emu/smoothtest.py --app firmware/target/native/thumbv7em-none-eabihf/release/opendm40
      [--mips 110] [--max-transient 2] [--out DIR]

One emulator run (no USB/IP or Bluetooth port) with the LCD write trace
(`sysbus.lcd Trace`, emu/peripherals/ILI9488.cs) through the transitions
below; emu/lcdtrace.py replays the trace at the panel's refresh rate
(60 Hz, inferred) and measures each transition's redraw. Fails (exit 1)
when a transition has no redraw, or when at some moment more than
--max-transient % (default 2) of the screen shows neither the old nor the
new picture for half a refresh or longer (a background cleared before its
widgets, a popup closed and opened again). Before the fix of 2026-10-29:
up to 44 % (the main menu: a cleared screen, then the tiles); after: 2.8 %;
with the strip rendering (firmware/src/lcd.rs): 0.6 %. The pages drawn in
strips (STRIPS) also fail when any pixel shows a third picture at all
(peak) or when the redraw writes more than one screen of pixels (each pixel
once: 153 600, 1 % slack; the generator page 5 %: its values are drawn
again after the page). About 2 min of wall time. Times are emulator times (--mips 110: the device's CPU
speed, inferred; the FSMC bus time is not modelled) and are printed, not
checked. --out DIR: a filmstrip per transition and a contact sheet.
"""
import argparse
import os
import shutil
import subprocess
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
sys.path.insert(0, HERE)
import lcdtrace  # noqa: E402

# (time, action, name): taps / keys of run.py --script.
STEPS = [
    (5.0, 'tap 172 298', 'V-ohm'),
    (6.5, 'tap 104 298', 'ohm-A'),
    (8.0, 'tap 36 298', 'A-V'),
    (9.5, 'key auto', 'range-step'),
    (11.0, 'key mf', 'range-page'),
    (12.5, 'key mf', 'range-close'),
    (14.0, 'key menu', 'menu'),
    (15.5, 'tap 339 105', 'scope'),
    (17.5, 'tap 447 300', 'scope-MF'),
    (19.0, 'tap 165 41', 'MF-tab'),
    (20.5, 'key mf', 'MF-close'),
    (22.0, 'tap 447 80', 'CH1-open'),
    (23.5, 'tap 447 80', 'CH1-close'),
    (25.0, 'key menu', 'menu-2'),
    (26.5, 'tap 141 219', 'gen'),
    (28.0, 'key menu', 'menu-3'),
    (29.5, 'tap 339 219', 'settings'),
    (31.0, 'key menu', 'menu-4'),
    (32.5, 'tap 141 105', 'dmm'),
]

# Pages App::repaint draws in strips: (max peak %, max pixels written in
# screens).
STRIPS = {
    'range-page': (0.0, 1.01),
    'menu': (0.0, 1.01),
    'scope': (0.0, 1.01),
    'menu-2': (0.0, 1.01),
    'gen': (0.5, 1.05),
    'menu-3': (0.0, 1.01),
    'settings': (0.0, 1.01),
    'menu-4': (0.0, 1.01),
}

ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
ap.add_argument('--app', required=True)
ap.add_argument('--mips', default='110', help='CPU MIPS (110: the device model)')
ap.add_argument('--max-transient', type=float, default=2.0, metavar='PCT')
ap.add_argument('--out', help='filmstrips into DIR')
ap.add_argument('--keep', action='store_true', help='keep the trace and the emulator log')
A = ap.parse_args()

OUT = os.path.join(HERE, 'out')
tag = 'smoothtest-%d' % os.getpid()
work = os.path.join(OUT, tag)
os.makedirs(work, exist_ok=True)
trace = os.path.join(work, 'lcd.trace')
script = os.path.join(work, 'script')
with open(script, 'w') as f:
    f.write('0.1 dmm dcv:1.234\n')
    f.write('3.0 monitor sysbus.lcd Trace "%s"\n' % trace)
    for t, act, _ in STEPS:
        f.write('%.3f %s\n' % (t, act))
t_end = STEPS[-1][0] + 1.0
cmd = [sys.executable, os.path.join(HERE, 'run.py'), 'fwrs', '--mips', A.mips, '--app', A.app,
       '--run-for', '%.3f' % t_end, '--name', tag, '--log', os.path.join(work, 'emu.log'), '--script', script]
rc = subprocess.call(cmd, cwd=ROOT, stdout=open(os.path.join(work, 'emu.out'), 'w'), stderr=subprocess.STDOUT)
if rc != 0 or not os.path.exists(trace):
    print('FAIL: emulator run (exit %d), see %s' % (rc, work))
    sys.exit(1)
res = lcdtrace.analyse(lcdtrace.load(trace), [(t, n) for t, _, n in STEPS], out=A.out)
bad = []
for r in res:
    why = None
    if not r['burst']:
        why = 'no redraw'
    elif r['transient'] > A.max_transient:
        why = 'transient %.1f %% > %.1f %%' % (r['transient'], A.max_transient)
    elif r['name'] in STRIPS:
        peak, slack = STRIPS[r['name']]
        if r['peak'] > peak:
            why = 'strips: peak %.1f %% > %.1f %%' % (r['peak'], peak)
        elif r['written'] > slack * lcdtrace.W * lcdtrace.H:
            why = 'strips: %d pixels written > %.2f screens' % (r['written'], slack)
    if why:
        bad.append(r)
        print('FAIL: %s: %s' % (r['name'], why))
if not A.keep:
    shutil.rmtree(work)
    for p in os.listdir(OUT):
        if p.startswith(tag) or p == 'flash-%s.bin' % tag:
            os.remove(os.path.join(OUT, p))
print('%s: %d transitions, worst transient %.1f %%' % ('FAIL' if bad else 'PASS', len(res), max(r['transient'] for r in res)))
sys.exit(1 if bad else 0)
