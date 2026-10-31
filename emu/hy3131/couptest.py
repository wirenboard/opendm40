#!/usr/bin/env python3
"""opendm40 AUTO coupling of V / A (firmware/src/dmm.rs AUTO_*, stock/re/docs/history/acdc-stock-vs-opendm40.md
section 11) in the emulator: opendm40 native + the HY3131 model, a timed
connection script per scenario (DC -> AC -> mixed signals, both ways through
the hysteresis band), SCPI over USB/IP (setup, then DBG:PING? uptime,
VOLT:COUP? / CURR:COUP?, FETCh?, FUNCtion? in a loop); the last samples of
every phase must match its expectation.

  emu/hy3131/couptest.py                          # all scenarios, opendm40 built from HEAD
  emu/hy3131/couptest.py --app firmware/opendm40.bin --port 3391 v
  emu/hy3131/couptest.py --list
  emu/hy3131/couptest.py --meth C v i        # on AC+DC method B / C (acdcm.rs)

--meth needs an image built with `--features acdc-meth` (Cargo.toml; the
default build refuses DBG:ACDC:METH / STAT?): pass it with --app.

Results: emu/out/coup-<name>.json (+ .png of the end); the Renode log of that
run: emu/out/coup-<name>-<port>-<pid>.log (capped by DM40_EMU_LOG_MAX_MB).
One emulator per port; a busy port is refused.
"""
import argparse
import json
import math
import os
import subprocess
import sys
import time

HERE = os.path.dirname(os.path.abspath(__file__))
EMU = os.path.dirname(HERE)
ROOT = os.path.dirname(EMU)
OUT = os.path.join(EMU, 'out')
ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
ap.add_argument('names', nargs='*')
ap.add_argument('--app', help='opendm40 native image (default: run.py builds HEAD)')
ap.add_argument('--port', type=int, default=3391, help='USB/IP port (default 3391)')
ap.add_argument('--list', action='store_true')
ap.add_argument('--meth', choices='ABC', help='AC+DC method first (DBG:ACDC:METH, opendm40 acdcm.rs; needs an acdc-meth build; default: the firmware\'s, A)')
A = ap.parse_args()
os.environ['USBIP_PORT'] = str(A.port)
sys.path.insert(0, EMU)
import usbip_client as uc  # noqa: E402
import emusync  # noqa: E402


def num(s):
    try:
        v = float(s)
    except ValueError:
        return float('nan')
    return float('nan') if abs(v) > 1e37 else v


def near(x, want, rel, ab=0.0):
    return not math.isnan(x) and abs(x - want) <= max(abs(want) * rel, ab)


def shows(coup, value, rel=0.01, ab=0.002):
    """VOLT:COUP? / CURR:COUP? answers `coup` and FETCh? is `value`."""
    return lambda d: d['C'] == coup and near(d['V'], value, rel, ab)


def tie(want, rel):
    """A tie (AC = DC): either part, or +9.91E37 (num: NaN) while AUTO has not decided yet
    (a tie can take ~10 s; FETCh? answers +9.91E37 until the first decision,
    opendm40 docs/history/switch-latency.md 6.2)."""
    return lambda d: d['C'] in ('AUTO,AC', 'AUTO,DC') and (near(d['V'], want, rel) or math.isnan(d['V']))


def both(*fs):
    return lambda d: all(f(d) for f in fs)


def func(name):
    return lambda d: d['F'] == '"%s"' % name


# name: (setup SCPI, coupling query, run_for, [(t, spec, check, label)], {t: command});
# t: the phase's time (spec None: the phase starts with an action at t, no input change).
# Timing is keyed on the input markers (emu/emusync.py, README "Input markers"): a phase
# starts at its input's fw uptime as logged by the model; an action at t runs at its
# distance from the start of the phase it falls in (fw uptime); a phase is checked on its
# samples 2.7 .. 0.7 s before the next phase's input marker (4.5 .. 2.5 s before a phase
# that starts with an action; the last phase: 2.7 .. 0.3 s before the run's end). The
# script puts the input changes emusync.BOOT_S later in virtual time (and runs that much
# longer), so they land at fw ~t; a check never depends on that (the offset was assumed
# 1.5-1.8 s here, while it had grown to 2.85 s).
# AUTO rule (dmm.rs): to AC when AC > 1.5 |DC| (and above the floor), to DC
# when AC < 0.75 |DC|; in between the part shown stays (AC = |DC| below).
SC = {
    'v': (['VOLT:COUP AUTO'], 'VOLT:COUP?', 72, [
        (0, 'open;dcv:5', both(shows('AUTO,DC', 5.0), func('VOLT:AUTO')), 'DC 5 V: the DC part, FUNC VOLT:AUTO'),
        (12, 'open;acv:2@50', shows('AUTO,AC', 2.0), 'AC 2 V: -> the AC part'),
        (24, 'open;acv:1@50,dc=1', shows('AUTO,AC', 1.0), 'mixed AC 1 + DC 1 (band): stays AC'),
        (36, 'open;acv:0.2@50,dc=1', shows('AUTO,DC', 1.0), 'mixed AC 0.2 + DC 1: -> DC'),
        (48, 'open;acv:1@50,dc=1', shows('AUTO,DC', 1.0), 'mixed AC 1 + DC 1 (band): stays DC'),
        (60, 'open;acv:1@50,dc=-0.3', shows('AUTO,AC', 1.0), 'mixed AC 1 + DC -0.3: -> AC'),
    ], {}),
    'i': (['CURR:COUP AUTO'], 'CURR:COUP?', 50, [
        (0, 'open;dca:0.3', both(shows('AUTO,DC', 0.3, ab=0.0005), func('CURR:AUTO')), 'DC 300 mA: the DC part, FUNC CURR:AUTO'),
        (12, 'open;aca:0.2@60', shows('AUTO,AC', 0.2, ab=0.0005), 'AC 200 mA: -> AC'),
        (24, 'open;aca:0.02@60,dc=0.2', shows('AUTO,DC', 0.2, ab=0.0005), 'mixed AC 20 + DC 200 mA: -> DC'),
        (37, 'open;aca:0.2@60,dc=-0.05', shows('AUTO,AC', 0.2, ab=0.0005), 'mixed AC 200 + DC -50 mA: -> AC'),
    ], {}),
    # Entering AUTO with an AC current above the mA shunt's 600 mA (owner
    # 2026-10-22: AC 1 A gave OVERLOAD at once; TODO "A AUTO coupling").
    # A enters on DC mA r0 (stock); 1 A AC there is a normal up-range in
    # AUTO, quiet (no latch / alert), and the DC phase must not walk down on
    # hypot(0, ~0) before the AC part is known (dmm.rs acdc_down_ok). Every
    # sample of the run is checked (ALWAYS below): SYST:ALER? NONE, never
    # the uA shunt, so a trip or a wrong step anywhere fails it. The AC 300 mA phase is 16 s
    # (was 13): AUTO walks 6 A -> 60 mA (AUTO,DC, the DC part ~0) -> 600 mA there and settles
    # 8.5 .. 10.6 s after the change (+9.91E37 until then), which the 13 s phase's window
    # caught in 1 of 40 loaded runs (2026-10-24).
    'iac': (['CURR:COUP AUTO'], 'CURR:COUP?', 43, [
        (0, 'open;aca:1@60', shows('AUTO,AC', 1.0, ab=0.005), 'enter AUTO with AC 1 A: AC, no OVERLOAD'),
        (14, 'open;aca:0.3@60', shows('AUTO,AC', 0.3, ab=0.001), 'AC 300 mA: AC (down to the mA shunt)'),
        (30, 'open;dca:0.3', shows('AUTO,DC', 0.3, ab=0.001), 'DC 300 mA: -> DC'),
    ], {}),
    # Entering AUTO with DC 1 A (TODO "AC+DC / AUTO down-range on one part"): the quiet hop
    # mA -> A is a unit change (reset_values: the DC part reads 0 until measured again); a
    # method whose AC result comes first (B / C) saw hypot(~0, 0) and stepped back down onto
    # the mA shunt with 1 A flowing. Every sample once on the A shunt: 6 A range or above.
    'idc': (['CURR:COUP AUTO'], 'CURR:COUP?', 30, [
        (0, 'open;dca:1', shows('AUTO,DC', 1.0, ab=0.005), 'enter AUTO with DC 1 A: DC, no OVERLOAD'),
        (16, 'open;dca:0.3', shows('AUTO,DC', 0.3, ab=0.001), 'DC 300 mA: down to the mA shunt'),
    ], {}),
    # AC 1 A with the model's RMS fill (rmsfill: the AC results ramp up over 1.5 s after each
    # AC row write, as the device's RMS window filling; the time is INFERRED and longer than
    # an AC phase, so the shown value stays low: only the coupling and the ranges are
    # checked). The AC part's settle (ACS_STEP) then outlasts the AC phase and the AC part
    # stays 0 from the unit change's reset_values (the quiet hop mA -> A); the DC phase saw
    # hypot(0, ~0) for ACDC_DOWN_MS and stepped down on it: 6 A -> 60 mA with 1 A flowing,
    # twice (no guard: fw 10.96 and 19.87 s). A step down now needs the AC part measured
    # since the reset (dmm.rs acdc_down_ok).
    'iacfill': (['CURR:COUP AUTO'], 'CURR:COUP?', 40, [
        (0, 'open;aca:1@60;rmsfill:1.5', lambda d: d['C'] == 'AUTO,AC', 'AC 1 A, slow RMS fill: AC, no OVERLOAD'),
        (24, 'open;aca:0.3@60;rmsfill:1.5', lambda d: d['C'] == 'AUTO,AC' and conf_range(d) < 1, 'AC 300 mA: AC, down to the mA shunt'),
    ], {}),
    # Entering V AUTO on 230 V AC: A AUTO's twin (TODO "AC+DC / AUTO down-range on one part";
    # not a repro: the model's DC conversion of 230 V AC on 6 V is over range, so the entry
    # steps up). A regression check: never the mV range, and on 600 V or above while 230 V once
    # method C's range visits (an anchor step per range: 600 V at fw ~12 s) are done; until the
    # 0.3 V phase's marker (script 26).
    'vac230': (['VOLT:COUP AUTO'], 'VOLT:COUP?', 40, [
        (0, 'open;acv:230@50', shows('AUTO,AC', 230.0, 0.005), 'enter AUTO on AC 230 V: AC, 600 V range'),
        (26, 'open;acv:0.3@50', shows('AUTO,AC', 0.3, 0.01), 'AC 0.3 V: down to 6 V'),
    ], {}),
    # Touch: three taps on the active V button (DC -> AC -> AC+DC -> AUTO;
    # from 5 s: a tap at 3 s fell into the boot), the fourth back to DC;
    # then VOLT:COUP ACDC and FUNC "VOLT:AUTO".
    'tap': ([], 'VOLT:COUP?', 52, [
        (0, 'open;acv:2@50', both(shows('AUTO,AC', 2.0), func('VOLT:AUTO')), '3 taps on V: AUTO, AC 2 V'),
        (18, None, both(shows('DC', 0.0, ab=0.01), func('VOLT')), '4th tap: DC'),
        (28, None, both(shows('ACDC', 2.0), func('VOLT:ACDC')), 'VOLT:COUP ACDC'),
        (40, None, both(lambda d: d['C'] == 'AUTO,AC', func('VOLT:AUTO'), lambda d: d['X'].get('CURR:COUP?') == 'DC'), 'FUNC "VOLT:AUTO"; CURR:COUP? outside A = DC'),
    ], {5: 'DBG:TAP 36,297', 7: 'DBG:TAP 36,297', 9: 'DBG:TAP 36,297', 18: 'DBG:TAP 36,297', 28: 'VOLT:COUP ACDC', 40: 'FUNC "VOLT:AUTO"'}),
    # The first decision after entering AUTO with a signal inside the band
    # (0.75 |DC| <= AC <= 1.5 |DC|): no hysteresis state yet, the larger
    # part wins (AC 1.2 + DC 1 -> AC; AC 0.8 + DC 1 -> DC) and then stays.
    # Each phase enters AUTO afresh (VOLT:COUP DC, then AUTO, 2 s after the
    # signal changed); a tie (AC 1 + DC 1): either part, but no flapping
    # (one part through the whole window); near ties (+-5 %) the larger.
    'band': (['VOLT:COUP AUTO'], 'VOLT:COUP?', 76, [
        (0, 'open;acv:1.2@50,dc=1', shows('AUTO,AC', 1.2), 'enter in band, AC 1.2 + DC 1: AC (larger)'),
        (14, 'open;acv:0.8@50,dc=1', shows('AUTO,DC', 1.0), 're-enter in band, AC 0.8 + DC 1: DC (larger)'),
        (28, 'open;acv:1.2@50,dc=-1', shows('AUTO,AC', 1.2), 're-enter in band, AC 1.2 + DC -1: AC (larger)'),
        (40, 'open;acv:1@50,dc=1', tie(1.0, 0.02), 're-enter, tie AC 1 + DC 1: either part'),
        (52, 'open;acv:1.05@50,dc=1', shows('AUTO,AC', 1.05), 're-enter, near tie AC 1.05 + DC 1: AC (larger)'),
        (64, 'open;acv:0.95@50,dc=1', shows('AUTO,DC', 1.0), 're-enter, near tie AC 0.95 + DC 1: DC (larger)'),
    ], {15: 'VOLT:COUP DC', 16: 'VOLT:COUP AUTO', 29: 'VOLT:COUP DC', 30: 'VOLT:COUP AUTO', 41: 'VOLT:COUP DC', 42: 'VOLT:COUP AUTO',
        53: 'VOLT:COUP DC', 54: 'VOLT:COUP AUTO', 65: 'VOLT:COUP DC', 66: 'VOLT:COUP AUTO'}),
    # The device check of 20a91d0 (method C by hand): AUTO on the calibrator's 40 Hz square
    # (0..9.55 V, AC = DC: a tie, either part), then 5 V DC: C stayed on the AC part
    # (~4.056 V) and hunted 6 <-> 60 V; A showed DC 4.998 V. 5.5 V the same; 4.5 V was fine.
    'sq5': (['VOLT:COUP AUTO'], 'VOLT:COUP?', 54, [
        (0, 'open;acv:4.776@40,dc=4.776,shape=square', tie(4.776, 0.01), 'square 0..9.55 V @40: either part (a tie)'),
        (14, 'open;dcv:5', shows('AUTO,DC', 5.0, 0.002), '5 V DC after the square: -> DC'),
        (30, 'open;dcv:5.5', shows('AUTO,DC', 5.5, 0.002), '5.5 V DC: DC'),
        (42, 'open;dcv:4.5', shows('AUTO,DC', 4.5, 0.002), '4.5 V DC: DC'),
    ], {}),
    # The device check of ac1e852 (par.py): AUTO entered on the 40 Hz square (AC 4.760 ~= DC
    # 4.764 V: a tie) went to DC with C and, after ~3.5 s, to AC with A (A's first AC part
    # overshot, 4.91 V, and the first evaluation decided). dmm.rs AUTO_TIE: a tie shows DC until
    # the AC part has settled on one side, then the larger part (DC when it does not settle).
    # Three fresh entries: either part, but no AC shown before the decision (DC, then at most
    # one change, to the decided part) and the same part in every entry (STABLE below: not
    # decided by a transient).
    'sqtie': (['VOLT:COUP AUTO'], 'VOLT:COUP?', 50, [
        (0, 'open;acv:4.776@40,dc=4.776,shape=square', tie(4.776, 0.01), 'square: either part (a tie)'),
        (16, None, tie(4.776, 0.01), 're-enter on the square'),
        (32, None, tie(4.776, 0.01), 're-enter on the square'),
    ], {16: 'VOLT:COUP DC', 17: 'VOLT:COUP AUTO', 32: 'VOLT:COUP DC', 33: 'VOLT:COUP AUTO'}),
}
# DBG:ACDC:STAT? (the method's state, failure context only) exists in acdc-meth builds only.
EXTRA = {'tap': ['CURR:COUP?'], 'sqtie': ['DBG:ACDC:STAT?'] if A.meth else [], 'iac': ['SYST:ALER?', 'CONF?'], 'idc': ['SYST:ALER?', 'CONF?'], 'vac230': ['CONF?'], 'iacfill': ['SYST:ALER?', 'CONF?']}


def conf_range(d):
    """The range (full scale) from CONF? ("VOLT:AUTO +6.000000E+02,+1E-02")."""
    try:
        return float(d['X']['CONF?'].strip('"').split()[1].split(',')[0])
    except (KeyError, IndexError, ValueError):
        return float('nan')


# Checked on every sample from t0 to t1: (t0, t1, check, label); t0 / t1 in script time,
# placed as the actions (emusync.Timeline.at: a phase's start time = its input's marker).
ALWAYS = {
    'iac': [(0, 99, lambda d: d['X'].get('SYST:ALER?') == 'NONE', 'SYST:ALER? NONE (no OVERLOAD)'),
            # 1 A / 300 mA never on the uA shunt (47.5 ohm; ranges 600 / 6000 uA)
            (0, 99, lambda d: not conf_range(d) < 0.05, 'never on the uA shunt')],
    'idc': [(0, 99, lambda d: d['X'].get('SYST:ALER?') == 'NONE', 'SYST:ALER? NONE (no OVERLOAD)'),
            (0, 99, lambda d: not conf_range(d) < 0.05, 'never on the uA shunt'),
            # 1 A: once up on the A shunt (fw ~2 s), never back on the mA shunt until
            # the 0.3 A input (script 16: up to its marker)
            (6, 16, lambda d: conf_range(d) >= 6, 'A shunt while 1 A')],
    'iacfill': [(0, 99, lambda d: d['X'].get('SYST:ALER?') == 'NONE', 'SYST:ALER? NONE (no OVERLOAD)'),
                (0, 99, lambda d: not conf_range(d) < 0.05, 'never on the uA shunt'),
                # the hop comes at fw ~9 s (the fill delays the first AC result); up
                # to the 0.3 A phase's marker (script 24)
                (10, 24, lambda d: conf_range(d) >= 6, 'A shunt while 1 A')],
    # 230 V never on 6 V or the mV range once the first up-range is done (fw ~4 s)
    'vac230': [(0, 99, lambda d: not conf_range(d) < 1, 'never the mV range'),
               (14, 26, lambda d: conf_range(d) >= 600, '600 V range or above while 230 V')],
}
# Scenarios for some AC+DC methods only (default: all). iacfill: the RMS fill model is longer
# than method A's AC phase on purpose; B / C keep one row and refill it at every range change
# (1.2 s apart there), so under that model their AC part never reaches the input: no test of
# them.
METHS = {'iacfill': 'A'}
# Scenarios whose AUTO part inside a phase (from its first AUTO reading after the phase's last
# command) is DC and then at most one change (the decision), the same part in every phase.
STABLE = {'sqtie'}


def run(name):
    setup, cq, run_for, phases, acts = SC[name]
    if (A.meth or 'A') not in METHS.get(name, 'ABC'):
        print('RESULT %s SKIP (method %s: the scenario is for %s)' % (name, A.meth, METHS[name]), flush=True)
        return True
    setup = (['DBG:ACDC:METH ' + A.meth] if A.meth else []) + setup
    acts = dict(acts)
    script = [(t, spec) for t, spec, _, _ in phases if spec]
    if uc.port_busy(A.port):
        print('RESULT %s FAIL port %d in use (another emulator?): pass a free --port' % (name, A.port), flush=True)
        return False
    tag = 'coup-' + name + (('-' + A.meth) if A.meth else '')
    # The run's own files (event script, Renode log, flash image, .resc) per
    # process: parallel runs (distinct --port) share none; results under tag.
    rtag = '%s-%d-%d' % (tag, A.port, os.getpid())
    os.makedirs(OUT, exist_ok=True)
    sp = os.path.join(OUT, rtag + '.script')
    with open(sp, 'w') as f:
        for t, spec in script:
            f.write('%g dmm %s\n' % (t + emusync.BOOT_S if t > 0 else t, spec))
    cmd = [sys.executable, os.path.join(EMU, 'run.py'), 'fwrs', '--fast', '--usbip', '3',
           '--usbip-port', str(A.port), '--run-for', str(run_for + emusync.BOOT_S), '--script', sp, '--name', rtag,
           '--shot', os.path.join(OUT, tag + '.png')] + (['--app', os.path.abspath(A.app)] if A.app else [])
    p = subprocess.Popen(cmd, cwd=ROOT, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)
    mk = emusync.Markers(emusync.log_path(OUT, rtag))
    tl = emusync.Timeline([(t, spec) for t, spec, _, _ in phases], run_for + emusync.BOOT_S, mk, vt=emusync.shifted(phases))
    rows = []
    t_wall = time.time()
    tm = None
    while p.poll() is None and tm is None and time.time() - t_wall < 300:
        try:
            tm = uc.Usbtmc(uc.Dev('1-0'))
        except Exception as e:
            print('attach:', type(e).__name__, e, flush=True)
            time.sleep(1)
    if tm is not None:
        try:
            for c in setup:
                tm.write(c)
            if A.meth:
                tm.write('SYST:ERR?')
                err = tm.read(timeout=60).decode(errors='replace').strip()
                if err.startswith('-113'):
                    print('RESULT %s FAIL --meth needs an image built with --features acdc-meth (%s)' % (name, err), flush=True)
                    p.kill()
                    p.communicate()
                    return False
            # The actions in fw uptime, at their distance from their phase's start
            # (predicted by the clock marker; the same as the input marker will say).
            if mk.wait_clock() is None:
                print(name, 'no clock marker (dwt: CYCCNT write) in the log', flush=True)
            acts = {tl.at(ta, predict=True) if mk.zero is not None else ta: c for ta, c in acts.items()}
            while p.poll() is None:
                row = {}
                for q in ['DBG:PING?', cq, 'FETC?', 'FUNC?'] + EXTRA.get(name, []):
                    tm.write(q)
                    row[q] = tm.read(timeout=60).decode(errors='replace').strip()
                t = int(row['DBG:PING?'].split(',')[0]) / 1000.0
                d = {'t': t, 'C': row[cq], 'V': num(row['FETC?']), 'F': row['FUNC?'], 'X': row}
                rows.append(d)
                print('%s %6.2f %-8s %-14s %s %s' % (name, t, d['C'], row['FETC?'], d['F'], ' '.join(row[q] for q in EXTRA.get(name, []))), flush=True)
                for ta in sorted(acts):
                    if t >= ta:
                        tm.write(acts.pop(ta))
        except Exception as e:
            print(name, 'link ended:', e, flush=True)
    out, _ = p.communicate()
    for x in (sp, os.path.join(OUT, 'flash-%s.bin' % rtag), os.path.join(OUT, rtag + '.resc')):
        if os.path.exists(x):
            os.remove(x)
    bad = emusync.check_sync(mk, tl, name)
    ok = not bad
    for i, (t, _, chk, label) in enumerate(phases):
        w = tl.window(i)
        seen = [r for r in rows if w and w[0] <= r['t'] <= w[1]]
        if not seen:
            ok = False
            bad.append('%s: no samples' % label)
            continue
        fails = [r for r in seen if not chk(r)]
        if fails:
            ok = False
            r = fails[-1]
            bad.append('%s: %.2f %s %g %s' % (label, r['t'], r['C'], r['V'], r['F']))
        else:
            r = seen[-1]
            print('  ok  %-50s %s %g' % (label, r['C'], r['V']), flush=True)
    for t0, t1, chk, label in ALWAYS.get(name, []):
        f0, f1 = tl.at(t0), tl.at(t1)
        fails = [r for r in rows if f0 is not None and f1 is not None and f0 <= r['t'] < f1 and not chk(r)]
        if fails:
            ok = False
            r = fails[0]
            bad.append('%s: %d samples, first %.2f %s %s' % (label, len(fails), r['t'], r['C'], ' '.join(r['X'].get(q, '') for q in EXTRA.get(name, []))))
        else:
            print('  ok  %-50s every sample' % label, flush=True)
    if name in STABLE:
        parts = []
        for i, (t, _, _, label) in enumerate(phases):
            t1 = phases[i + 1][0] if i + 1 < len(phases) else run_for
            t0 = tl.at(max([ta for ta in SC[name][4] if t <= ta < t1] + [t]))
            e = tl.stop(i)
            seq = [r['C'] for r in rows if t0 is not None and e is not None and t0 + 0.2 <= r['t'] <= e - 1.5 and r['C'].startswith('AUTO,')]
            flips = sum(1 for a, b in zip(seq, seq[1:]) if a != b)
            rle = []
            for x in seq:
                if rle and rle[-1][0] == x[5:]:
                    rle[-1][1] += 1
                else:
                    rle.append([x[5:], 1])
            print('  %s: %s, %d flips (%s)' % (label, seq[-1] if seq else '-', flips, ' '.join('%s x%d' % tuple(q) for q in rle)), flush=True)
            if not seq or flips > 1 or (flips and seq[0] != 'AUTO,DC'):
                ok = False
                bad.append('%s: %d flips' % (label, flips))
            parts.append(seq[-1] if seq else '-')
        if len(set(parts)) > 1:
            ok = False
            bad.append('entries differ: %s' % ' '.join(parts))
    res = {'name': name, 'ok': ok, 'bad': bad, 'rows': rows, 'rc': p.returncode, 'log': out[-600:] if p.returncode else '',
           'sync': emusync.summary(mk, tl)}
    with open(os.path.join(OUT, tag + '.json'), 'w') as f:
        json.dump(res, f, indent=1)
    print('RESULT %s %s %s' % (name, 'OK' if ok else 'FAIL', '; '.join(bad[:5])), flush=True)
    return ok


if __name__ == '__main__':
    if A.list:
        for n, (su, cq, rf, ph, ac) in SC.items():
            print('%-4s %s | %s' % (n, ' '.join(su), ' | '.join('%g %s: %s' % (t, s, l) for t, s, _, l in ph)))
        sys.exit(0)
    allok = True
    for n in A.names or list(SC):
        allok &= run(n)
    sys.exit(0 if allok else 1)
