#!/usr/bin/env python3
"""opendm40 AC+DC methods A / B / C (firmware/src/acdcm.rs, DBG:ACDC:METH) in the
emulator: one run per method, the same timed signals (V then A: DC, AC,
mixed, DC with 50 Hz ripple, DC with off-notch 45 Hz ripple, V DC again
on the 6 V range after the 60 V one: a range visited again), SCPI over
USB/IP (DBG:PING?, FETCh?, DBG:ACDC:STAT? in a loop). Per phase it reports
the error of the shown total against hypot(AC, DC) of the source (mean and
peak-to-peak over the phase's 6th to 1.5th last second), the reading rate
(DBG:ACDC:STAT? n_rd: AC+DC readings / s) and the settle time (first sample
from which the total stays within 0.1 % + 2 digits of the truth; nan:
never in the phase).

Method C is held to the device's rules on top (C_CHECK, acdc-approaches.md
10.4-10.6): the DC part within 0.05 % + 5 digits of the source, the AC part
of a DC input below 0.5 % of the range and of a sine within 0.05 %, >= 6
readings / s, no range change in the window. The 60 V phases (DC 24 V, the
40 Hz square of the device check) and the 5 V DC ones (6 V range, after the
square) are the device check of 20a91d0's failures.

The HY3131 model's note 7 (AD1 shifted while AD2 / AD3 convert, the device's
per-range offsets, feasibility 13.2) is on in every run; `B~` adds a
non-repeating AD1 gain shift of +-0.2 % per AD1 enable (n7g, the device's
60 V gain did not repeat), which B's entry zero cannot remove.

  emu/hy3131/acdcmethtest.py --app firmware/opendm40.bin             # A B C B~ in parallel
  emu/hy3131/acdcmethtest.py --app X --meths C --port 18120
  emu/hy3131/acdcmethtest.py --app X --meths C --e8 down --port 29100   # device test e8

`--e8 down` / `--e8 up` is the device's e8 (ac1e852: C's 60 V gain from the
internal zero 0.030 % high): AC+DC entered on the manual 60 V range at 24 V
(down: 24 / 12 / 5 / 1 / 0 V) or at 0 V (up: 0 / 1 / 5 / 12 / 24 V) and
stepped without leaving, then the manual 6 V range (5 / 3 / 1 / 0 V; `tc6`:
this part alone, entered at 5 V, the last level 0.1 V: the 80d5b01 6 V
failure, its 0 V level the device's reading without a new reference); the
DC part against the source with the device's rule (0.05 % + 5 digits) in
two windows: the device script's (1..4 s after the step) and the late one
(4..8.5 s). The model's internal zero sits 15 codes off on 60 V
("agndz:", HY3131.cs AgndZeroCodes), as the device's.
Needs an image built with `--features acdc-meth` (firmware/Cargo.toml): the
default build has method A only and refuses DBG:ACDC:METH / STAT?.
Results: emu/out/acdcmeth-<meth>[-e8<x>]-p<port>.json, the table on stdout. Exit 1 when a
method misses its accuracy bound (CHECK below) in a phase.
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
ap.add_argument('--app', help='opendm40 native image (built with --features acdc-meth)')
ap.add_argument('--port', type=int, default=18110, help='first USB/IP port (one per method)')
ap.add_argument('--meths', default='A,B,C,B~')
ap.add_argument('--child', action='store_true', help=argparse.SUPPRESS)
ap.add_argument('--analyse', action='store_true', help='only the table of the last runs (emu/out/acdcmeth-*.json)')
ap.add_argument('--e8', choices=['down', 'up', 'tc6'], help="the device's e8: manual 60 V levels, then manual 6 V; tc6: 6 V only")
ap.add_argument('--model', default='', help="extra HY3131 model options for every phase (e.g. 'agndz:0;')")
A = ap.parse_args()
if A.child:
    os.environ['USBIP_PORT'] = str(A.port)
sys.path.insert(0, EMU)
import usbip_client as uc  # noqa: E402
import emusync  # noqa: E402

PH = 12.0  # s per phase
# (signal spec, dc, ac rms, unit 'V' / 'A', label)
SIG = [
    ('open;dcv:5', 5.0, 0.0, 'V', 'V DC 5'),
    ('open;acv:2@50', 0.0, 2.0, 'V', 'V AC 2 @50'),
    ('open;acv:1@50,dc=1', 1.0, 1.0, 'V', 'V AC 1 + DC 1'),
    ('open;acv:0.5@50,dc=12', 12.0, 0.5, 'V', 'V DC 12 + 0.5 @50'),
    ('open;acv:0.5@45,dc=12', 12.0, 0.5, 'V', 'V DC 12 + 0.5 @45'),
    ('open;dcv:24', 24.0, 0.0, 'V', 'V DC 24'),
    ('open;acv:4.776@40,dc=4.776,shape=square', 4.776, 4.776, 'V', 'V square 0..9.55 @40'),
    ('open;dcv:5', 5.0, 0.0, 'V', 'V DC 5 after sq (r1->r0)'),
    ('open;dca:0.3', 0.3, 0.0, 'A', 'A DC 0.3'),
    ('open;aca:0.2@50', 0.0, 0.2, 'A', 'A AC 0.2 @50'),
    ('open;aca:0.2@50,dc=0.2', 0.2, 0.2, 'A', 'A AC 0.2 + DC 0.2'),
    ('open;aca:0.05@45,dc=0.3', 0.3, 0.05, 'A', 'A DC 0.3 + 0.05 @45'),
]
# --e8: (level V, MF range item) per phase; the range item is sent at the
# first phase of each range (DBG:ACDC:RANG: 2 = 60 V, 1 = 6 V).
E8_PH = 10.0
E8 = {
    'down': [(24.0, 2), (12.0, 2), (5.0, 2), (1.0, 2), (0.0, 2), (5.0, 1), (3.0, 1), (1.0, 1), (0.0, 1)],
    'up': [(0.0, 2), (1.0, 2), (5.0, 2), (12.0, 2), (24.0, 2), (5.0, 1), (3.0, 1), (1.0, 1), (0.0, 1)],
    # the device's 0 V step came 62.4 k codes from the 3 V anchor, at the edge of the 1.5 x rule
    # (no new reference there on the device): 0.1 V keeps it below in the model
    'tc6': [(5.0, 1), (3.0, 1), (1.0, 1), (0.1, 1)],
}
# The device script's window (1 s after the step, 3 s) and the late one.
E8_WIN = ((1.0, 4.0), (4.0, 8.5))
if A.e8:
    PH = E8_PH
    SIG = [('open;dcv:%g' % v, v, 0.0, 'V', 'e8 %s V %g' % ('60' if it == 2 else '6', v)) for v, it in E8[A.e8]]
# The phases in fw uptime: each starts at its input's marker (emu/emusync.py,
# README "Input markers"; the fw uptime trails the script by the boot, 2.85 s on a
# fresh disk, once assumed a constant 1.34 s here). A sample at fw t is at script
# time sv(t) = i * PH + (t - start_i) in phase i. LAG: only for result files
# written before the markers (no 'starts' in them).
LAG = 1.34
T_A = sum(1 for x in SIG if x[3] == 'V') * PH  # the A phases start here (CURR:COUP ACDC)
RUN_FOR = len(SIG) * PH + 2
# Accuracy bound per phase (|mean error| %, p-p %) every method must meet.
CHECK = (0.3, 1.0)
# Method C on top (the device's rules, acdc-approaches.md 10.4 / 10.5): per V phase
# (DC part bound in V = 0.05 % + 5 digits of the range or None, AC part: (truth, bound in V) or
# None, the least readings / s, no range change in the window).
C_CHECK = {
    'V DC 5': (0.05e-2 * 5 + 5e-4, (0.0, 0.03), 6.0),
    'V AC 2 @50': (None, (2.0, 2.0 * 0.05e-2), 6.0),
    'V DC 24': (0.05e-2 * 24 + 5e-3, (0.0, 0.3), 6.0),
    'V square 0..9.55 @40': (0.05e-2 * 4.776 + 5e-3, (4.776, 4.776 * 0.1e-2), 6.0),
    'V DC 5 after sq (r1->r0)': (0.05e-2 * 5 + 5e-4, (0.0, 0.03), 6.0),
}


def num(s):
    try:
        v = float(s)
    except ValueError:
        return float('nan')
    return float('nan') if abs(v) > 1e37 else v


def run(meth, port, res):
    tag = 'acdcmeth-' + meth.replace('~', 'g') + ('-e8' + A.e8 if A.e8 else '')
    if uc.port_busy(port):
        res[meth] = {'err': 'port %d busy' % port}
        return
    rtag = '%s-%d-%d' % (tag, port, os.getpid())
    os.makedirs(OUT, exist_ok=True)
    sp = os.path.join(OUT, rtag + '.script')
    extra = ('n7g:0.002;' if meth == 'B~' else '') + A.model
    with open(sp, 'w') as f:
        for i, (spec, *_x) in enumerate(SIG):
            f.write('%g dmm %s%s\n' % (i * PH, extra, spec))
    cmd = [sys.executable, os.path.join(EMU, 'run.py'), 'fwrs', '--fast', '--usbip', '3', '--usbip-port', str(port),
           '--run-for', str(RUN_FOR), '--script', sp, '--name', rtag, '--app', os.path.abspath(A.app)]
    p = subprocess.Popen(cmd, cwd=ROOT, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)
    mk = emusync.Markers(emusync.log_path(OUT, rtag))
    tl = emusync.Timeline([(i * PH, spec) for i, (spec, *_x) in enumerate(SIG)], RUN_FOR, mk)
    rows = []
    tm = None
    t_wall = time.time()
    while p.poll() is None and tm is None and time.time() - t_wall < 300:
        try:
            tm = uc.Usbtmc(uc.Dev('1-0'))
        except Exception:
            tm = None
            time.sleep(1)
    switched = False
    # --e8: the range item at the first phase of each range
    cmds = []
    if A.e8:
        last = None
        for i, (_v, it) in enumerate(E8[A.e8]):
            if it != last:
                cmds.append((i * PH, 'DBG:ACDC:RANG %d' % it))
                last = it
    if tm is not None and mk.wait_clock() is None:
        print(meth, 'no clock marker (dwt: CYCCNT write) in the log', flush=True)

    def fw(ts):
        # script time -> fw uptime (predicted by the clock marker = the input markers' value)
        return tl.at(ts, predict=True) if mk.zero is not None else ts - LAG
    cmds = [(fw(ts + (0.3 if ts == 0 else 0.0)), c) for ts, c in cmds]
    t_switch = fw(T_A + 0.3)
    if tm is not None:
        try:
            m = meth[0]
            tm.write('DBG:ACDC:METH %s' % m)
            tm.write('SYST:ERR?')
            err = tm.read(timeout=60).decode(errors='replace').strip()
            if err.startswith('-113'):
                raise RuntimeError('the image needs --features acdc-meth (%s)' % err)
            tm.write('VOLT:COUP ACDC')
            while p.poll() is None:
                row = {}
                for q in ['DBG:PING?', 'FETC?', 'DBG:ACDC:STAT?', 'FUNC?']:
                    tm.write(q)
                    row[q] = tm.read(timeout=60).decode(errors='replace').strip()
                t = int(row['DBG:PING?'].split(',')[0]) / 1000.0
                st = row['DBG:ACDC:STAT?'].split(',')
                rows.append({'t': t, 'v': num(row['FETC?']), 'st': st, 'f': row['FUNC?']})
                while cmds and t >= cmds[0][0]:
                    tm.write(cmds.pop(0)[1])
                if A.e8:
                    continue
                if t >= t_switch and not switched:
                    tm.write('CURR:COUP ACDC')
                    switched = True
        except Exception as e:
            print(meth, 'link ended:', e, flush=True)
    out, _ = p.communicate()
    for x in (sp, os.path.join(OUT, 'flash-%s.bin' % rtag), os.path.join(OUT, rtag + '.resc')):
        if os.path.exists(x):
            os.remove(x)
    bad = emusync.check_sync(mk, tl, meth)
    res[meth] = {'rows': rows, 'rc': p.returncode or (4 if bad else 0), 'log': out[-800:] if p.returncode else '; '.join(bad),
                 'starts': [tl.start(i) for i in range(len(SIG))], 'sync': emusync.summary(mk, tl)}
    with open(os.path.join(OUT, '%s-p%d.json' % (tag, port)), 'w') as f:
        json.dump(res[meth], f)


def script_time(d):
    """fw uptime -> script time for run result d: phase i (the last whose start,
    its input marker, is at or before t) at i * PH + (t - start_i)."""
    st = d.get('starts')
    if not st or any(x is None for x in st):
        return lambda t: t + LAG

    def sv(t):
        i = max([k for k, b in enumerate(st) if b <= t] or [0])
        return i * PH + (t - st[i])
    return sv


def analyse(meth, d, w=None):
    """Per phase: (label, mean err %, p-p %, rate /s, settle s, n samples);
    `w` (from, to) s after the phase start, else its 6th to 1.5th last s."""
    rows = d.get('rows', [])
    sv = script_time(d)
    out = []
    for i, (_spec, dc, ac, unit, label) in enumerate(SIG):
        t0, t1 = i * PH, (i + 1) * PH
        truth = math.hypot(dc, ac)
        w0, w1 = (t0 + w[0], t0 + w[1]) if w else (t1 - 6.0, t1 - 1.5)
        ph = [r for r in rows if t0 <= sv(r['t']) < t1]
        win = [r for r in ph if w0 <= sv(r['t']) <= w1 and not math.isnan(r['v'])]
        if not win:
            out.append((label, float('nan'), float('nan'), float('nan'), float('nan'), 0, None))
            continue
        e = [(r['v'] - truth) / max(truth, 1e-3) * 100 for r in win]
        mean = sum(e) / len(e)
        pp = max(e) - min(e)
        # reading rate: DBG:ACDC:STAT? n_rd (AC+DC readings, every method)
        n = [int(r['st'][8]) for r in win]
        rate = (n[-1] - n[0]) / max(1e-3, win[-1]['t'] - win[0]['t'])
        tol = 0.001 * truth + 0.002 * (1 if unit == 'V' else 0.1)
        settle = float('nan')
        for k, r in enumerate(ph):
            if all(not math.isnan(x['v']) and abs(x['v'] - truth) <= tol for x in ph[k:]):
                settle = sv(r['t']) - t0
                break
        # method C extras: DC / AC parts (DBG:ACDC:STAT? 5 / 4) against the source, range changes
        dcp = [num(r['st'][5]) for r in win]
        acp = [num(r['st'][4]) for r in win]
        rng = set(r['st'][3] for r in win)
        extra = (sum(dcp) / len(dcp) - dc, sum(acp) / len(acp), max(acp), len(rng))
        out.append((label, mean, pp, rate, settle, len(win), extra))
    return out


def e8_report(meths, res):
    """--e8: C's DC part against the source per level and window, the
    device's rule 0.05 % + 5 digits of the range."""
    ok = True
    for m in meths:
        d = res.get(m, {})
        tabs = [analyse(m, d, w) for w in E8_WIN]
        print('\n%s e8 %s: DC part - source (mV) in the windows %s s, rule (mV), ranges, gspan' % (m, A.e8, E8_WIN))
        for i, (_spec, dc, _ac, _u, label) in enumerate(SIG):
            dig = 1e-3 if '60' in label.split()[1] else 1e-4
            rule = 0.05e-2 * abs(dc) + 5 * dig
            line = '%-14s' % label
            for k, tab in enumerate(tabs):
                lab, mean, pp, rate, settle, n, extra = tab[i]
                if not n:
                    line += '   (none) '
                    ok = ok and i == 0  # the entry's first reading may come after the first window
                    continue
                bad = abs(extra[0]) > rule
                line += ' %+8.3f%s' % (extra[0] * 1e3, '*' if bad else ' ')
                ok = ok and not bad
            sv = script_time(d)
            rows = [r for r in d.get('rows', []) if i * PH <= sv(r['t']) < (i + 1) * PH]
            gs = rows[-1]['st'][17] if rows and len(rows[-1]['st']) > 17 else '?'
            if rows and len(rows[-1]['st']) > 20:
                gs += ' src %s dis %s' % (rows[-1]['st'][18], rows[-1]['st'][19])
            rg = sorted(set(r['st'][3] for r in rows if E8_WIN[0][0] <= sv(r['t']) - i * PH))
            line += '  %6.2f  %s  %s' % (rule * 1e3, ','.join(rg), gs)
            print(line)
        if d.get('err') or d.get('rc'):
            print(m, 'run problem:', d.get('err', ''), d.get('log', '')[-300:])
            ok = False
    print('RESULT', 'OK' if ok else 'FAIL')
    return ok


def c_check(label, rate, extra):
    """Method C's device rules for phase `label`: '' or what failed."""
    if label not in C_CHECK:
        return ''
    dcb, acb, rmin = C_CHECK[label]
    dce, acm, acmax, nrng = extra
    bad = []
    if dcb is not None and abs(dce) > dcb:
        bad.append('DC %+.4f' % dce)
    if acb is not None and (abs(acm - acb[0]) > acb[1] or abs(acmax - acb[0]) > 3 * acb[1]):
        bad.append('AC %.4f/%.4f' % (acm, acmax))
    if rate < rmin:
        bad.append('rate')
    if nrng > 1:
        bad.append('ranges %d' % nrng)
    return ' '.join(bad)


if __name__ == '__main__':
    meths = A.meths.split(',')
    res = {}
    if A.child:
        run(meths[0], A.port, res)
        sys.exit(0)
    ps = None if A.analyse else []
    for i, m in enumerate(meths if ps is not None else []):
        ps.append(subprocess.Popen([sys.executable, __file__, '--child', '--app', A.app, '--meths', m, '--port', str(A.port + i)]
                                   + (['--e8', A.e8] if A.e8 else []) + (['--model', A.model] if A.model else [])))
        time.sleep(2)
    for p in ps or []:
        p.wait()
    for m in meths:
        try:
            # runs on other ports (other images) write their own files
            name = 'acdcmeth-' + m.replace('~', 'g') + ('-e8' + A.e8 if A.e8 else '')
            p = os.path.join(OUT, '%s-p%d.json' % (name, A.port + meths.index(m)))
            with open(p if os.path.exists(p) or not A.analyse else os.path.join(OUT, name + '.json')) as f:
                res[m] = json.load(f)
        except OSError as e:
            res[m] = {'err': str(e)}
    if A.e8:
        sys.exit(0 if e8_report(meths, res) else 1)
    ok = True
    print('\n%-22s' % 'phase' + ''.join('| %-34s' % ('method ' + m) for m in meths))
    print('%-22s' % '' + ''.join('| %-34s' % 'err% mean  p-p   rd/s settle_s' for _ in meths))
    tabs = {m: analyse(m, res.get(m, {})) for m in meths}
    for i in range(len(SIG)):
        line = '%-22s' % SIG[i][4]
        for m in meths:
            lab, mean, pp, rate, settle, n, extra = tabs[m][i]
            line += '| %+7.3f %6.3f %5.1f %6.1f     ' % (mean, pp, rate, settle)
            if n == 0 or abs(mean) > CHECK[0] or pp > CHECK[1]:
                if m != 'B~':
                    ok = False
                line += '*'
        print(line)
        for m in meths:
            lab, mean, pp, rate, settle, n, extra = tabs[m][i]
            if m == 'C' and n and lab in C_CHECK:
                bad = c_check(lab, rate, extra)
                print('%-22s   C: DC part %+.4f V, AC part %.4f (max %.4f) V, ranges %d%s' % ('', extra[0], extra[1], extra[2], extra[3], ('  * ' + bad) if bad else ''))
                if bad:
                    ok = False
    for m in meths:
        if res.get(m, {}).get('err') or res.get(m, {}).get('rc'):
            print(m, 'run problem:', res[m].get('err', ''), res[m].get('log', '')[-300:])
            ok = False
    print('RESULT', 'OK' if ok else 'FAIL')
    sys.exit(0 if ok else 1)
