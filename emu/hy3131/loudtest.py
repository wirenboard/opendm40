#!/usr/bin/env python3
"""opendm40 danger alarm (firmware/src/danger.rs, firmware/docs/power-meter-and-misconnect.md
4.10) in the emulator: opendm40 native + the HY3131 model, a timed connection /
touch / key script per scenario, the buzzer timer's register writes logged
by the model (run.py --buzzer-log: TIM14 ARR = pitch, CCR1 = duty, in virtual
ms) and SCPI over USB/IP (SYST:ALER?, DBG:ALARm? in a loop).

Checked (virtual time, the script's clock): the loud duty (CCR1 = 0.495 x
the period: 5544 at ARR 11199, 6930 at ARR 13999) with the key / alert
volume at 0 %, the two-tone burst pattern (125 ms steps, a burst per second),
silencing by a tap on the alert badge / the alert toast / a side key, the
re-arm when the condition comes back, the stop when it clears, the stock
pitch (ARR 11199) and CCR1 0 after every stop, the setting Off (no alarm),
Volume (the alarm at the key volume), V-JACK? (no alarm) and a current above
the 10 A rating.

  emu/hy3131/loudtest.py --app firmware/opendm40.bin --port 53101          # all
  emu/hy3131/loudtest.py --app firmware/opendm40.bin --port 53101 aprot ajack
  emu/hy3131/loudtest.py --list

The key / alert volume is set to 0 % (SYST:BEEP:VOL 0, the owner's setting):
at 100 % a key beep (CCR1 5544) would look like the alarm's high tone.

Results: emu/out/loud-<name>.json, emu/out/loud-<name>.buzz (the register
log). One emulator per port; run scenarios in parallel on different ports.
"""
import argparse
import json
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
ap.add_argument('--port', type=int, default=53101, help='USB/IP port (default 53101)')
ap.add_argument('--list', action='store_true')
A = ap.parse_args()
os.environ['USBIP_PORT'] = str(A.port)
sys.path.insert(0, EMU)
import usbip_client as uc  # noqa: E402
import emusync  # noqa: E402

ARR_HI, ARR_LO = 11199, 13999
LOUD = {(ARR_HI, 5544), (ARR_LO, 6930)}
# Volume 40 %: duty 0.4 / 3 x 0.5 of the period.
VOL40 = {(ARR_HI, int(11200 * 0.4 / 3 * 0.5)), (ARR_LO, int(14000 * 0.4 / 3 * 0.5))}
BADGE = 'tap 160 17'      # DMM status slot (ui::hold_label 119..202 x 1..33)
TOAST = 'tap 240 160'     # dialog::TOAST centre

# name: dict(func=SCPI CONF or None, run_for, setup=[SCPI after attach],
#            script=[(virtual s, run.py script line)], acts={virtual s: SCPI},
#            checks=[...], alerts=[(t0, t1 virtual-ish fw uptime, alert that must be seen)])
# acts are sent when the fw uptime reaches their virtual time on the fw clock (the clock
# marker, emu/emusync.py, README "Input markers"): on the script's clock, as the checks
# (they were fw uptime values written for a ~1.5 s offset: 16.5 / 20.5 / 31.5, now the
# virtual 18 / 22 / 33 they meant; with the 2.85 s boot of a fresh disk the last one came
# at virtual 34.35, 0.15 s before the end of its 'starts' window).
# checks: ('quiet', t0, t1)                   no alarm duty written in t0..t1 (s)
#         ('loud', t0, t1, set)               the alarm sounds in t0..t1: >= 2 bursts of
#                                             (ARR, CCR1) pairs from `set`, both tones
#         ('pattern', t0, t1)                 125 ms tone steps, bursts 1 s apart (+-35 ms: a
#                                             main-loop pass can run ~30 ms late)
#         ('starts', t0, t1, set)             the first alarm write after t0 is before t1
#         ('stops', t, t1)                    CCR1 0 and ARR 11199 (stock pitch) at t1 (a
#                                             'quiet' check from t1 follows each one)
# never: (alert, t0, t1): no SCPI row with it in t0..t1 (virtual s) and no assertion of it there
#        in DBG:SENTinel:LOG? (a transient between two rows)
# seen: (SYST:ALER?, DBG:ALARm? prefix) pairs that must appear in some SCPI row
#       (DBG:ALARm? = sounding,silenced,events,setting)
SC = {
    # A-PROT (12 V across A-COM in DC V): loud at A-JACK (~0.3 s), A-PROT 0.5 s later;
    # badge tap at 15.45 (inside a burst: they start ~0.25-0.3 s after each whole second
    # here) -> silent at once; the source removed at 18 (the latch holds, the probe reads
    # no current: not dangerous); AUTO at 21 (the user acts: the latch ends); 12 V again
    # at 25 -> a new event, loud; removed at 31 -> stops.
    'aprot': dict(func=None, run_for=35, setup=['SYST:BEEP:VOL 0'],
                  script=[(0, 'dmm open'), (10, 'dmm avsrc:12'), (15.45, BADGE), (18, 'dmm open'),
                          (21, 'key auto'), (25, 'dmm avsrc:12'), (31, 'dmm open')],
                  checks=[('quiet', 6, 10), ('starts', 10, 11.2, LOUD), ('loud', 10, 15, LOUD), ('pattern', 10.5, 15),
                          ('stops', 15.45, 15.65), ('quiet', 15.65, 25), ('starts', 25, 26.2, LOUD), ('loud', 25, 31, LOUD),
                          ('stops', 31, 32.0), ('quiet', 32.0, 35)],
                  # sounding in the first event, silenced, then latched with no current: quiet
                  seen=[('APROT', '1,0,1,0'), ('APROT', '0,1,1,0'), ('APROT', '1,0,2,0'), ('APROT', '0,0,2,0')]),
    # A-JACK (1 mA into A in DC V): RUN key at 15.45 (inside a burst) silences (and toggles
    # HOLD); the current off at 18 / on at 21: loud again; off at 26: stops within the 1 s
    # A-JACK clear; on at 29, a tap on its toast at 29.5 (inside the first burst) silences.
    'ajack': dict(func=None, run_for=34, setup=['SYST:BEEP:VOL 0'],
                  script=[(0, 'dmm open;vsrc:1.234'), (10, 'dmm isrc:0.001'), (15.45, 'key run'), (18, 'dmm isrc:none'),
                          (21, 'dmm isrc:0.001'), (26, 'dmm isrc:none'), (29, 'dmm isrc:0.001'), (29.5, TOAST)],
                  checks=[('quiet', 6, 10), ('starts', 10, 11.0, LOUD), ('loud', 10, 15, LOUD), ('pattern', 10.5, 15),
                          ('stops', 15.45, 15.65), ('quiet', 15.65, 21), ('starts', 21, 22.0, LOUD), ('loud', 21, 26, LOUD),
                          ('stops', 26, 27.8), ('quiet', 27.8, 29), ('starts', 29, 29.45, LOUD), ('stops', 29.5, 29.7),
                          ('quiet', 29.7, 34)],
                  seen=[('AJACK', '1,0,1,0'), ('AJACK', '0,1,1,0'), ('AJACK', '1,0,2,0'), ('AJACK', '0,1,3,0')]),
    # A-PROT, the source removed (15 s), then ohm (the user acts in a function without the
    # probe; the latch holds there): no new alarm.
    'aprot-ohm': dict(func=None, run_for=26, setup=['SYST:BEEP:VOL 0'],
                      script=[(0, 'dmm open'), (10, 'dmm avsrc:12'), (15, 'dmm open')],
                      acts={18: ['CONF:RES']},
                      checks=[('quiet', 6, 10), ('starts', 10, 11.2, LOUD), ('loud', 10, 15, LOUD), ('stops', 15, 15.6),
                              ('quiet', 15.6, 26)],
                      seen=[('APROT', '1,0,1,0'), ('APROT', '0,0,1,0')],
                      # the latch held in ohm, quiet (FUNC? "RES")
                      seen_func=[('APROT', '0,0,1,0', '"RES"')]),
    # V-JACK? (24 V on V/ohm in DC A, no current): an advisory, no alarm. Then Off: A-JACK
    # in DC V makes no sound; then Volume at 40 %: the alarm at the key volume.
    'vjack-off': dict(func='CURR:DC', run_for=40, setup=['SYST:BEEP:VOL 0'],
                      script=[(0, 'dmm open'), (10, 'dmm vsrc:24'), (19, 'dmm vsrc:none;vsrc:1.234'), (26, 'dmm isrc:0.001')],
                      acts={22: ['SYST:ALER:ALARm OFF', 'CONF:VOLT:DC'], 33: ['SYST:BEEP:VOL 40', 'SYST:ALER:ALARm VOL']},
                      checks=[('quiet', 6, 33), ('starts', 33, 34.5, VOL40), ('loud', 33, 40, VOL40)],
                      seen=[('VJACK', '0,0,0,0'), ('AJACK', '0,0,1,2'), ('AJACK', '1,0,1,1')]),
    # A-JACK where V-COM can fake it (doc 4.10 / 4.11; device c785807: the RES 5 V false A-JACK
    # sounded the loud alarm): RES with 5 V on V-COM and PB1 picking it up (Pb1VjackMv 10, the
    # device's ohm-row pickup), then the same row's evidence from a real 2 mA in A with V-COM
    # open: the sensors read the same in both, so both are badge only (sentinel `soft`), no
    # alarm. CONF:VOLT:DC at ~24 (virtual) with the current still in A: DC V's comparator (V-COM free) is
    # evidence a driven V-COM cannot give -> loud; off at 30 -> stops.
    'ajack-res': dict(func='RES', run_for=34, setup=['SYST:BEEP:VOL 0'],
                      script=[(0, 'monitor sysbus.hy3131 Pb1VjackMv 10'), (0, 'dmm open;r:4700'), (8, 'dmm vsrc:5'),
                              (14, 'dmm open;isrc:0.002;aiohm:31e6'), (30, 'dmm isrc:none')],
                      acts={24.0: ['CONF:VOLT:DC']},  # virtual s (it was fw uptime 21.0, ~3 s behind)
                      checks=[('quiet', 6, 24), ('starts', 24, 25.6, LOUD), ('loud', 24.5, 29.5, LOUD), ('stops', 30, 31.8),
                              ('quiet', 31.8, 34)],
                      # 5 V on V-COM: LIVE? (the jack check), the real current: A-JACK
                      seen=[('AJACK', '0,0,0,0'), ('AJACK', '1,0,1,0')],
                      seen_func=[('AJACK', '0,0,0,0', '"RES"'), ('LIVE', '0,0,0,0', '"RES"')]),
    # Continuity (the AD2 peak on PB1), a real 2 mA in A: badge only, no alarm (40728b8 item 4:
    # 5 V on V-COM gives the same A-JACK there).
    'ajack-cont': dict(func='CONT', run_for=22, setup=['SYST:BEEP:VOL 0'],
                       script=[(0, 'dmm open;r:10'), (10, 'dmm open;isrc:0.002')],
                       checks=[('quiet', 6, 22)],
                       seen_func=[('AJACK', '0,0,0,0', '"CONT"')]),
    # Continuity with 5 V on V-COM (device 40728b8 item 4): LIVE?, quiet; then a real 2 mA in A
    # with V-COM open: A-JACK, badge only (soft).
    'ajack-cont-v5': dict(func='CONT', run_for=26, setup=['SYST:BEEP:VOL 0'],
                          script=[(0, 'monitor sysbus.hy3131 Pb1VjackMv 10'), (0, 'dmm open;r:10'), (8, 'dmm open;vsrc:5'),
                                  (16, 'dmm open;isrc:0.002')],
                          checks=[('quiet', 6, 26)],
                          seen_func=[('LIVE', '0,0,0,0', '"CONT"'), ('AJACK', '0,0,0,0', '"CONT"')]),
    # AC mV (manual 600 mV) with the 0 / 9.5 V 40 Hz square on V-COM, the A jack empty (device
    # 2f56ff9: AJACK,AD1PK_PB1 and the loud alarm; the model's mvclamp / mvpb1): quiet, no
    # A-JACK row; then the square off and a real 5 mA AC in A: loud (AC mV is not a soft row);
    # off at 26 -> stops.
    'ajack-acmv': dict(func='VOLT:AC 0.6', run_for=30, setup=['SYST:BEEP:VOL 0'],
                       script=[(0, 'dmm open;mvclamp:1.212;mvpb1:10;vsrc:0,ac=0.003,f=50'),
                               (8, 'dmm vsrc:4.776,ac=4.776,f=40,shape=square'),
                               (18, 'dmm vsrc:0,ac=0.003,f=50;isrc:0,ac=0.005,f=60'), (26, 'dmm isrc:none')],
                       checks=[('quiet', 6, 18), ('starts', 18, 20.5, LOUD), ('loud', 19, 25.5, LOUD), ('stops', 26, 27.8),
                               ('quiet', 27.8, 30)],
                       seen=[('AJACK', '1,0,1,0')], never=[('AJACK', 6, 18)]),
    # A 66 kHz signal on V-COM (device 2026-10-26: A-JACK / A-PROT and the alarm with the A jack
    # empty; the model's hf knobs, senttest HF): quiet in AC mV and DC mV, loud with a current in A.
    'ajack-acmv-hf': dict(func='VOLT:AC 0.6', run_for=30, setup=['SYST:BEEP:VOL 0'],
                          script=[(0, 'dmm open;hfbw:6000;hfdc:0.3;hfpb1:10;hfpb2:1;vsrc:0,ac=0.003,f=50'),
                                  (8, 'dmm vsrc:0,ac=3,f=66000'),
                                  (18, 'dmm vsrc:0,ac=0.003,f=50;isrc:0,ac=0.005,f=60'), (26, 'dmm isrc:none')],
                          checks=[('quiet', 6, 18), ('starts', 18, 20.5, LOUD), ('loud', 19.5, 25.5, LOUD), ('stops', 26, 27.8),
                                  ('quiet', 27.8, 30)],
                          seen=[('AJACK', '1,0,1,0')], never=[('AJACK', 6, 18), ('APROT', 0, 30)]),
    'ajack-dcmv-hf': dict(func='VOLT:DC 0.6', run_for=30, setup=['SYST:BEEP:VOL 0'],
                          script=[(0, 'dmm open;hfbw:6000;hfdc:0.3;hfpb1:10;hfpb2:1;vsrc:0.1'),
                                  (8, 'dmm vsrc:0,ac=3,f=66000'),
                                  (18, 'dmm vsrc:none;isrc:0.002'), (26, 'dmm isrc:none')],
                          checks=[('quiet', 6, 18), ('starts', 18, 20.5, LOUD), ('loud', 19.5, 25.5, LOUD), ('stops', 26, 27.8),
                                  ('quiet', 27.8, 30)],
                          seen=[('AJACK', '1,0,1,0')], never=[('AJACK', 6, 18), ('APROT', 0, 30)]),
    # The same in AC+DC V on manual 600 mV (doc 4.13: V_GATE in both phases): quiet with the
    # square; the real AC current asserts at the next "free" AC-phase result (~2.2 s).
    'ajack-acdcmv': dict(func='VOLT:ACDC 0.6', run_for=30, setup=['SYST:BEEP:VOL 0'],
                         script=[(0, 'dmm open;mvclamp:1.212;mvpb1:10;vsrc:0,ac=0.003,f=50'),
                                 (8, 'dmm vsrc:4.776,ac=4.776,f=40,shape=square'),
                                 (18, 'dmm vsrc:0,ac=0.003,f=50;isrc:0,ac=0.005,f=60'), (26, 'dmm isrc:none')],
                         checks=[('quiet', 6, 18), ('starts', 18, 21.5, LOUD), ('loud', 21.5, 25.5, LOUD),
                                 ('stops', 26, 28.0), ('quiet', 28.0, 30)],
                         never=[('AJACK', 6, 18)]),
    # The same square in AC V AUTO (mV-first, the device's case): the range walks up out of mV
    # (~2 s); quiet and no A-JACK through it.
    'ajack-acv-auto': dict(func='VOLT:AC', run_for=26, setup=['SYST:BEEP:VOL 0'],
                           script=[(0, 'dmm open;mvclamp:1.212;mvpb1:10;vsrc:0,ac=0.003,f=50'),
                                   (10, 'dmm vsrc:4.776,ac=4.776,f=40,shape=square')],
                           checks=[('quiet', 6, 26)], never=[('AJACK', 6, 26)]),
    # Above the 10 A rating in DC A: 12 A (OL on the 10 A range) for 1 s -> loud; 5 A -> stops.
    'i10': dict(func='CURR:DC', run_for=30, setup=['SYST:BEEP:VOL 0'],
                script=[(0, 'dmm open'), (10, 'dmm isrc:5'), (16, 'dmm isrc:12'), (24, 'dmm isrc:5')],
                checks=[('quiet', 6, 16.9), ('starts', 16.9, 19, LOUD), ('loud', 17, 24, LOUD), ('stops', 24, 25.5),
                        ('quiet', 25.5, 30)],
                seen=[('NONE', '1,0,1,0')]),
}


def read_log(path):
    ev = []
    if os.path.exists(path):
        for line in open(path):
            p = line.split()
            if len(p) == 3:
                ev.append((float(p[0]) / 1000.0, p[1], int(p[2])))
    return ev


def states(ev):
    """(t, ARR, CCR1) after every write."""
    arr, ccr, out = 11199, 0, []
    for t, reg, v in ev:
        if reg == 'ARR':
            arr = v
        else:
            ccr = v
        # ARR and CCR1 of one tone change are written in the same pass (the
        # same virtual ms): one state
        if out and t - out[-1][0] < 0.001:
            out[-1] = (out[-1][0], arr, ccr)
        else:
            out.append((t, arr, ccr))
    return out


def check(st, chk):
    kind = chk[0]
    alarm = lambda s: s[2] != 0 and (s[1] == ARR_LO or s[2] >= 5000 or (s[1], s[2]) in VOL40)
    if kind == 'quiet':
        bad = [s for s in st if chk[1] <= s[0] < chk[2] and alarm(s)]
        return not bad, '%d alarm writes in %g..%g s%s' % (len(bad), chk[1], chk[2], ' (first %.3f)' % bad[0][0] if bad else '')
    if kind == 'starts':
        on = [s for s in st if s[0] >= chk[1] and s[2] != 0 and (s[1], s[2]) in chk[3]]
        ok = bool(on) and on[0][0] < chk[2]
        return ok, 'first alarm tone after %g s at %s (want < %g)' % (chk[1], '%.3f' % on[0][0] if on else None, chk[2])
    if kind == 'loud':
        w = [s for s in st if chk[1] <= s[0] < chk[2] and s[2] != 0]
        good = [s for s in w if (s[1], s[2]) in chk[3]]
        other = [s for s in w if (s[1], s[2]) not in chk[3]]
        tones = {s[1] for s in good}
        ok = len(good) >= 8 and tones == {ARR_HI, ARR_LO} and not other
        return ok, '%d alarm tones in %g..%g s, pitches %s, %d other duty writes %s' % (
            len(good), chk[1], chk[2], sorted(tones), len(other), sorted({(s[1], s[2]) for s in other})[:4])
    if kind == 'pattern':
        w = [s for s in st if chk[1] <= s[0] < chk[2]]
        # tone changes (start of each sounding step) and burst starts (silence -> tone)
        steps, bursts, prev = [], [], None
        for s in w:
            cur = (s[1], s[2]) if s[2] else None
            if cur != prev:
                if cur is not None:
                    steps.append(s[0])
                    if prev is None:
                        bursts.append(s[0])
                prev = cur
        # from the first whole burst (t0 may fall inside one)
        steps = [t for t in steps if bursts and t >= bursts[1]] if len(bursts) > 1 else []
        dsteps = [b - a for a, b in zip(steps, steps[1:]) if b - a < 0.3]
        dbursts = [b - a for a, b in zip(bursts[1:], bursts[2:])]
        ok = (len(dsteps) >= 6 and all(abs(d - 0.125) < 0.035 for d in dsteps)
              and len(dbursts) >= 2 and all(abs(d - 1.0) < 0.035 for d in dbursts))
        return ok, 'steps %s ms, bursts %s ms' % ([round(d * 1000) for d in dsteps[:8]], [round(d * 1000) for d in dbursts[:5]])
    if kind == 'stops':
        on = [s[0] for s in st if chk[1] <= s[0] < chk[2] and alarm(s)]
        at = [s for s in st if s[0] < chk[2]]
        end = at[-1] if at else (0, 11199, 0)
        ok = end[2] == 0 and end[1] == ARR_HI
        return ok, 'last tone in %g..%g s %s, off at %.3f s (ARR %d CCR1 %d at %g s)' % (
            chk[1], chk[2], '%.3f' % on[-1] if on else 'none', end[0], end[1], end[2], chk[2])
    return False, 'unknown check ' + kind


def run(name):
    sc = SC[name]
    if uc.port_busy(A.port):
        print('RESULT %s FAIL port %d in use (another emulator?): pass a free --port' % (name, A.port), flush=True)
        return False
    tag = 'loud-' + name
    rtag = '%s-%d-%d' % (tag, A.port, os.getpid())
    os.makedirs(OUT, exist_ok=True)
    sp = os.path.join(OUT, rtag + '.script')
    buzz = os.path.join(OUT, tag + '.buzz')
    if os.path.exists(buzz):
        os.remove(buzz)
    with open(sp, 'w') as f:
        for t, line in sc['script']:
            f.write('%g %s\n' % (t, line))
    cmd = [sys.executable, os.path.join(EMU, 'run.py'), 'fwrs', '--fast', '--usbip', '3',
           '--usbip-port', str(A.port), '--run-for', str(sc['run_for']), '--script', sp, '--name', rtag,
           '--buzzer-log', buzz, '--shot', os.path.join(OUT, tag + '.png')] + (['--app', os.path.abspath(A.app)] if A.app else [])
    p = subprocess.Popen(cmd, cwd=ROOT, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)
    mk = emusync.Markers(emusync.log_path(OUT, rtag))
    rows = []
    t_wall = time.time()
    tm = None
    while p.poll() is None and tm is None and time.time() - t_wall < 300:
        try:
            d = uc.Dev('1-0')
            tm = uc.Usbtmc(d)
        except Exception as e:
            print('attach:', type(e).__name__, e, flush=True)
            time.sleep(1)
    if tm is not None:
        try:
            for c in sc['setup'] + (['CONF:' + sc['func']] if sc['func'] else []):
                tm.write(c)
            if mk.wait_clock() is None and sc.get('acts'):
                print(name, 'no clock marker (dwt: CYCCNT write) in the log', flush=True)
            acts = {mk.fw(ta) if mk.zero is not None else ta: c for ta, c in sc.get('acts', {}).items()}
            while p.poll() is None:
                row = {}
                for q in ['DBG:PING?', 'SYST:ALER?', 'DBG:ALARm?', 'SYST:ALER:ALARm?', 'SYST:BEEP:VOL?', 'FUNC?'] + \
                        (['DBG:SENT:LOG?'] if sc.get('never') else []):
                    tm.write(q)
                    row[q] = tm.read(timeout=60).decode(errors='replace').strip()
                row['t'] = int(row['DBG:PING?'].split(',')[0]) / 1000.0
                rows.append(row)
                for ta in sorted(acts):
                    if row['t'] >= ta:
                        for c in acts.pop(ta):
                            tm.write(c)
                print('%s %6.2f %-6s %-12s %s %s %s' % (name, row['t'], row['SYST:ALER?'], row['DBG:ALARm?'],
                                                      row['SYST:ALER:ALARm?'], row['SYST:BEEP:VOL?'], row['FUNC?']), flush=True)
                time.sleep(0.1)
        except Exception as e:
            print(name, 'link ended:', e, flush=True)
    out, _ = p.communicate()
    for x in (sp, os.path.join(OUT, 'flash-%s.bin' % rtag), os.path.join(OUT, rtag + '.resc')):
        if os.path.exists(x):
            os.remove(x)
    st = states(read_log(buzz))
    ok, bad, res = True, [], []
    for chk in sc['checks']:
        c_ok, msg = check(st, chk)
        res.append((chk[:3], c_ok, msg))
        print('  %s %-8s %s' % ('ok  ' if c_ok else 'FAIL', chk[0], msg), flush=True)
        if not c_ok:
            ok = False
            bad.append('%s %s' % (chk[0], msg))
    seen = sorted({(r['SYST:ALER?'], r['DBG:ALARm?']) for r in rows})
    for a, pre in sc.get('seen', []):
        if not any(x[0] == a and x[1].startswith(pre) for x in seen):
            ok = False
            bad.append('row %s %s never seen' % (a, pre))
    print('  rows seen: %s' % ' '.join('%s/%s' % x for x in seen), flush=True)
    for a, pre, fn in sc.get('seen_func', []):
        if not any(r['SYST:ALER?'] == a and r['DBG:ALARm?'].startswith(pre) and r.get('FUNC?') == fn for r in rows):
            ok = False
            bad.append('row %s %s in %s never seen' % (a, pre, fn))
    for a, t0, t1 in sc.get('never', []):
        # no row with this alert in t0..t1 (virtual s, as the checks; on the fw clock), nor an
        # assertion in the firmware's log
        if mk.zero is not None:
            t0, t1 = mk.fw(t0), mk.fw(t1)
        hit = [r['t'] for r in rows if t0 <= r['t'] < t1 and r['SYST:ALER?'] == a]
        for r in rows:
            for e in r.get('DBG:SENT:LOG?', '').split(';')[1:]:
                f = e.split(',')
                if len(f) == 3 and f[0] == a and t0 <= int(f[2]) / 1000.0 < t1:
                    hit.append(int(f[2]) / 1000.0)
        if hit:
            ok = False
            bad.append('%s at %s' % (a, ' '.join('%.2f' % x for x in sorted(set(hit))[:6])))
    if not rows:
        ok = False
        bad.append('no SCPI rows')
    with open(os.path.join(OUT, tag + '.json'), 'w') as f:
        json.dump({'name': name, 'ok': ok, 'bad': bad, 'checks': res, 'rows': rows, 'rc': p.returncode,
                   'log': out[-600:] if p.returncode else ''}, f, indent=1)
    print('RESULT %s %s %s' % (name, 'OK' if ok else 'FAIL', '; '.join(bad[:5])), flush=True)
    return ok


if __name__ == '__main__':
    if A.list:
        for n, v in SC.items():
            print('%-10s %s' % (n, ' | '.join('%g %s' % x for x in v['script'])))
        sys.exit(0)
    allok = True
    for n in A.names or list(SC):
        allok &= run(n)
    sys.exit(0 if allok else 1)
