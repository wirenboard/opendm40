#!/usr/bin/env python3
"""opendm40 misconnect sentinel through function / range / coupling switches
(firmware/src/sentinel.rs, owner 2026-10-23: "the AC hint and A-JACK / A-PROT
trip falsely when switching from other modes") in the emulator: opendm40 native
+ the HY3131 model with a fixed connection, a sequence of switches over SCPI
over USB/IP (CONF:<f> [range], VOLT:COUP, the MF -> Wave view and back),
each held for DWELL s of fw uptime. After every step the firmware's
assertion log (DBG:SENTinel:LOG?: every alert that came on, with its time,
so a transient between two polls is not missed) and the SYST:ALER? samples
of the step must hold only the alerts the connection allows in that
function.

  emu/hy3131/switchtest.py                        # all scenarios, opendm40 built from HEAD
  emu/hy3131/switchtest.py --app firmware/opendm40.bin --port 44101 open dc5
  emu/hy3131/switchtest.py --full open            # every ordered pair of the functions (long)
  emu/hy3131/switchtest.py --list

The real-* scenarios hold a real condition (a current in A, 24 V across
A-COM, AC or DC on V-COM, 24 V on V in an A function) and check how soon
after every switch into the function its alert comes on (fw uptime from the
switch to the logged assertion), so a blank cannot hide the real thing.
Results: emu/out/switch-<name>.json; the Renode log emu/out/switch-<name>-
<port>-<pid>.log. One emulator per port (a busy port is refused).
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
ap.add_argument('--port', type=int, default=44100, help='USB/IP port (default 44100)')
ap.add_argument('--dwell', type=float, default=3.0, help='fw-uptime seconds per step (default 3)')
ap.add_argument('--full', action='store_true', help='every ordered pair of the functions (long)')
ap.add_argument('--list', action='store_true')
A = ap.parse_args()
os.environ['USBIP_PORT'] = str(A.port)
sys.path.insert(0, EMU)
import usbip_client as uc  # noqa: E402
import emusync  # noqa: E402

# Steps: a name -> the SCPI commands that select it. 'WAVE' = MF -> Wave view
# (V / A functions), 3 s there, MENU back to the DMM. 'SCOPE' = MENU -> the
# Oscilloscope tile (the MCX scope page: the DMM rail off), 3 s there, MENU ->
# the Multimeter tile (the rail back: a chip restart).
F = {
    'dcv': ['CONF:VOLT:DC'], 'dcmv': ['CONF:VOLT:DC 0.6'], 'dcv60': ['CONF:VOLT:DC 60'],
    'acv': ['CONF:VOLT:AC'], 'acmv': ['CONF:VOLT:AC 0.6'], 'acmv60': ['CONF:VOLT:AC 0.06'], 'vacdc': ['CONF:VOLT:ACDC'], 'vauto': ['CONF:VOLT:AUTO'],
    'dca': ['CONF:CURR:DC'], 'dcma': ['CONF:CURR:DC 0.06'], 'dcua': ['CONF:CURR:DC 0.0006'],
    'aca': ['CONF:CURR:AC'], 'acma': ['CONF:CURR:AC 0.06'], 'acua': ['CONF:CURR:AC 0.0006'], 'aca10': ['CONF:CURR:AC 10'],
    'aacdc': ['CONF:CURR:ACDC'], 'aauto': ['CONF:CURR:AUTO'],
    'res': ['CONF:RES'], 'cont': ['CONF:CONT'], 'diod': ['CONF:DIOD'], 'cap': ['CONF:CAP'],
    'freq': ['CONF:FREQ'], 'temp': ['CONF:TEMP'], 'pow': ['CONF:POW'],
    'vdcac': ['VOLT:COUP AC'], 'vacdcc': ['VOLT:COUP DC'],
}
WAVE_OK = {'dcv', 'acv', 'acmv', 'dcmv', 'aca', 'acma', 'dca', 'dcma'}
CUR = {'dca', 'dcma', 'dcua', 'aca', 'acma', 'acua', 'aca10', 'aacdc', 'aauto'}
SLOW = {'res', 'temp', 'pow', 'vacdc', 'aacdc', 'vauto', 'aauto'}

# The default sequence: every function entered from several others (V <-> A
# <-> ohm / temperature / diode / capacitance / frequency / power, the
# coupling commands, the Wave view), ~60 steps.
SEQ = ['dcv', 'acv', 'dcv', 'vacdc', 'dcv', 'vauto', 'acv', 'res', 'dcv', 'temp', 'dcv', 'cont', 'dcv', 'diod',
       'dcv', 'cap', 'dcv', 'freq', 'dcv', 'dca', 'dcv', 'aca', 'acv', 'aacdc', 'acv', 'dcma', 'res', 'acua', 'temp',
       'acmv', 'dcmv', 'acmv', 'vacdc', 'res', 'vauto', 'cont', 'acv', 'WAVE', 'dcv', 'WAVE', 'aca', 'WAVE',
       'dcv', 'pow', 'dcv', 'dcv60', 'dcv', 'vdcac', 'vacdcc', 'diod', 'acv', 'cap', 'acv', 'freq', 'acv', 'temp',
       'aauto', 'dcv', 'dcua', 'dcv', 'res', 'acv', 'dcv']
FULL = ['dcv', 'dcmv', 'acv', 'acmv', 'vacdc', 'vauto', 'dca', 'aca', 'aacdc', 'aauto', 'res', 'cont', 'diod', 'cap',
        'freq', 'temp', 'pow']


def full_seq():
    """An Eulerian circuit of the complete digraph on FULL: every ordered
    pair (a -> b) once."""
    n = len(FULL)
    left = {a: [b for b in range(n) if b != a] for a in range(n)}
    stack, path = [0], []
    while stack:
        v = stack[-1]
        if left[v]:
            stack.append(left[v].pop())
        else:
            path.append(stack.pop())
    return [FULL[i] for i in reversed(path)]


def allowed_for(inp, step):
    """Alerts the connection `inp` may raise in `step` (besides NONE)."""
    a = set()
    if inp == 'ajack':
        a |= {'AJACK'}
    if inp == 'aprot':
        a |= {'AJACK', 'APROT'}
    if inp == 'dc24aca':
        if step in CUR - {'aca', 'acma', 'acua', 'aca10'}:
            a |= {'VJACK'}
        if step in ('acv', 'vdcac'):
            a |= {'DC'}
    if inp in ('dc5', 'v24', 'dc5a'):
        if step in ('acv', 'vdcac'):
            a |= {'DC'}
        if step in CUR and not (inp == 'dc5a' and step in ('dca', 'dcma', 'dcua')):
            a |= {'VJACK'}  # AC A's peak (also an AC+DC / AUTO AC phase): 2 V threshold
        if step in ('res', 'cont', 'diod'):
            a |= {'LIVE'}
        if step == 'pow':
            a |= {'DC', 'AC'}
    if inp == 'sq':
        # the 0 / 9.5 V 40 Hz square: what dc5 and ac2 allow
        return allowed_for('dc5', step) | allowed_for('ac2', step)
    if inp == 'ac2':
        if step in ('dcv', 'vacdcc'):
            a |= {'AC'}
        if step in CUR:
            a |= {'VJACK'}
        if step in ('res', 'cont', 'diod'):
            a |= {'LIVE'}
        if step == 'pow':
            a |= {'DC', 'AC'}
    return a


# name: (connection spec, input class for allowed_for, monitor lines, sequence or None
# (= SEQ / --full), {step: (alert, max s from the switch)}: a real condition that must
# come on that fast after every switch into that step - a blank must not hide it).
SC = {
    'open': ('open', 'open', [], None, {}),
    'dc5': ('open;vsrc:5', 'dc5', [], None, {}),
    'ac2': ('open;vsrc:0,ac=2,f=50', 'ac2', [], None, {}),
    # A voltage on V-COM, the A jack empty, with the model's PB1 pickups (Pb1VjackMv: ohm /
    # temperature / continuity, device 519fb8a 7b / 40728b8 item 4; mvclamp / mvpb1: the AC mV
    # clamp, device 2f56ff9): no A-JACK in any function, also through AC V AUTO's mV steps
    # (acv: mV-first; CONF:VOLT:AC 0.06 / 0.6 manual) and ohm / continuity entries.
    'dc5-pickup': ('open;vsrc:5;mvclamp:1.212;mvpb1:10', 'dc5', ['sysbus.hy3131 Pb1VjackMv 10'],
                   ['dcv', 'res', 'dcv', 'cont', 'acv', 'res', 'acmv', 'cont', 'acmv60', 'dcv', 'acv', 'temp', 'res',
                    'diod', 'cont', 'acv', 'dcv'], {}),
    'sq-pickup': ('open;vsrc:4.776,ac=4.776,f=40,shape=square;mvclamp:1.212;mvpb1:10', 'sq',
                  ['sysbus.hy3131 Pb1VjackMv 10'],
                  ['dcv', 'res', 'dcv', 'cont', 'acv', 'res', 'acmv', 'cont', 'acmv60', 'dcv', 'acv', 'temp', 'res',
                   'diod', 'cont', 'acv', 'dcv'], {}),
    # ... and a real current in A (V-COM open) is still caught after every switch in
    'real-ajack-pickup': ('open;isrc:0.001;mvclamp:1.212;mvpb1:10', 'ajack', ['sysbus.hy3131 Pb1VjackMv 10'],
                          ['dcv', 'res', 'dcv', 'cont', 'dcv', 'acmv', 'dcv', 'acmv60', 'dcv', 'res'],
                          {'res': ('AJACK', 2.0), 'cont': ('AJACK', 1.2), 'acmv': ('AJACK', 2.0), 'acmv60': ('AJACK', 2.0)}),
    # model knobs: the AD1 decimation memory (a mux switch without a restart mixes the old
    # input into the next 3 conversions) and AD2's start-up results 3000 codes off (d8e249d)
    'open-mix': ('open;ad1mix:1', 'open', ['sysbus.hy3131 Ad2StartGlitch 3000'], None, {}),
    'dc5-mix': ('open;vsrc:5;ad1mix:1', 'dc5', ['sysbus.hy3131 Ad2StartGlitch 3000'], None, {}),
    # the model's switch transient (HY3131.cs SwTrMv, a HYPOTHETICAL ring of the sentinel nodes
    # after every row change, until device numbers come): 300 mV, tau 50 ms, 25 Hz
    'open-swtr': ('open;swtr:300;swtau:50', 'open', [], None, {}),
    'dc5-swtr': ('open;vsrc:5;swtr:300;swtau:50', 'dc5', [], None, {}),
    # device 469f354 (nothing connected): RES -> A / mA, CMP_SENSE CTSTA 0x70 for ~3.5 s then 0x50
    # to ~5 s -> a false V-JACK? from 1.5-1.95 s; the model's ohmexit: replays that CTSTA
    'ohmexit': ('open;ohmexit:3.5;ohmexitlo:1.2', 'open', [],
                ['res', 'dca', 'res', 'dcma', 'res', 'dcua', 'res', 'aauto', 'res', 'aacdc', 'res', 'aca', 'cont', 'dca',
                 'temp', 'dcma', 'diod', 'dca', 'cap', 'dcma', 'res', 'dcv', 'res', 'dca'], {}),
    # device 6bc494a new bug 4: A / mA -> Scope -> DMM with 5 V on V-COM, CTSTA 0x70 to 7.75 s
    # and 0x50 to 8.01 s after the switch, then 0 (ohmexitrst: the trace also from the chip
    # reset when the DMM rail comes back); the 7 s cap let the 0x70 tail count: V-JACK? at
    # 8.06 s. 5 V through the /1000 tap is below the comparator: no V-JACK? in DC A (dc5a).
    # Dwell 11 s (DWELL) so the step outlasts the tail.
    'ohmexit-long': ('open;vsrc:5;ohmexit:7.75;ohmexitlo:0.26;ohmexitrst:1', 'dc5a', [],
                     ['dca', 'SCOPE', 'dcma', 'SCOPE', 'dca', 'res', 'dca', 'res', 'dcma'], {}),
    'real-vjack-ohmexit': ('open;vsrc:24;ohmexit:3.5;ohmexitlo:1.2', 'v24', [], ['dcv', 'dca', 'res', 'dca', 'res', 'dcma'],
                           {'dca': ('VJACK', 6.5), 'dcma': ('VJACK', 6.5)}),
    'real-ajack-swtr': ('open;isrc:0.001;swtr:300;swtau:50', 'ajack', [],
                        ['acv', 'dcv', 'res', 'dcv', 'temp', 'dcv', 'vacdc', 'dcv', 'acv', 'cont', 'acv'],
                        {'dcv': ('AJACK', 1.0), 'acv': ('AJACK', 1.6), 'cont': ('AJACK', 1.0), 'vacdc': ('AJACK', 2.5)}),
    # real conditions: how fast after a switch in (settle + the detector)
    'real-ajack': ('open;isrc:0.001', 'ajack', [], ['acv', 'dcv', 'res', 'dcv', 'temp', 'dcv', 'aca', 'dcv', 'acv', 'cont', 'acv'],
                   {'dcv': ('AJACK', 1.0), 'acv': ('AJACK', 1.6), 'cont': ('AJACK', 1.0)}),
    # 24 V across A-COM connected at virtual 12 s (fw ~9.2 s), during RES (A-JACK only there, no
    # probe), then the switch into DC V: A-PROT after the switch (a source on from boot latches in
    # DC V at once). RES holds until 1 s after the connection's marker (`later` in run()).
    'real-aprot': ('open', 'aprot', [], ['res', 'dcv'], {'dcv': ('APROT', 1.5)}, [(12, 'avsrc:24')]),
    'real-achint': ('open;vsrc:0,ac=2,f=50', 'ac2', [], ['acv', 'dcv', 'res', 'dcv', 'temp', 'dcv'], {'dcv': ('AC', 4.0)}),
    'real-dchint': ('open;vsrc:5', 'dc5', [], ['dcv', 'acv', 'res', 'acv', 'temp', 'acv'], {'acv': ('DC', 6.0)}),
    'real-vjack': ('open;vsrc:24', 'v24', [], ['dcv', 'dca', 'res', 'dca', 'aca', 'dca'], {'dca': ('VJACK', 4.0)}),
    # device 40728b8 2a: AC A's AD1 V-JACK? peak on SENSE - PB5 ran to the negative rail in ~2 s
    # whatever was on V (pb5drift replays it): a false V-JACK? in every AC A range
    'aca-drift': ('open;pb5drift:0.6;pb5tau:2.2', 'open', [],
                  ['dcv', 'aca', 'dcv', 'acma', 'dca', 'acua', 'res', 'aca10', 'acma', 'aacdc', 'aauto', 'dcv'], {}),
    # device 40728b8 new bug 1: CONF:CURR:AC 10, then 0.06, the A jack empty, 1 V on V: the first
    # 60 mA result read OL -> OVERLOAD, back to 10 A (acastep replays the first-block step)
    # device d2aff22 new bug 2: 24 V DC on V, SENSE - AGND ramps to the rail in AC A (senseneg):
    # the ramp's per-hold swing was a false V-JACK? at 1.5-3.5 s in 60 mA / 600 uA
    'aca-ramp': ('open;vsrc:24;senseneg:0.75;sensetau:5', 'dc24aca', [],
                 ['dcv', 'aca', 'dcv', 'acma', 'dcv', 'acua', 'dcv', 'aca10', 'dcv'], {}),
    'aca-ovl': ('open;vsrc:1;acastep:200000;acastepms:600', 'dc5', [],
                ['aca10', 'acma', 'aca10', 'acma', 'dcma', 'acma', 'acua', 'acma'], {}),
    # a real AC source on V while in AC A (24 V rms): V-JACK? from the AD1 swing
    'real-vjack-ac': ('open;vsrc:0,ac=24,f=50', 'ac2', [], ['dcv', 'aca', 'dcv', 'acma', 'res', 'aca10', 'dca', 'acua'],
                      {'aca': ('VJACK', 3.0), 'acma': ('VJACK', 3.0), 'aca10': ('VJACK', 3.0), 'acua': ('VJACK', 3.0)}),
    # device 40728b8 3 / 2b: 24 V on V-COM read in DC A only after TEMP; DC V 60 V, RES and a chip
    # restart left it blind (vjblind replays the observation). The firmware's mitigation is the
    # opt-in experiment DBG:SENTinel:PRECond (SETUP below); without it this scenario fails.
    'real-vjack-blind': ('open;vsrc:24;vjblind:1', 'v24', [], ['dcv60', 'dca', 'temp', 'dca', 'res', 'dcma', 'dcv', 'dca'],
                         {'dca': ('VJACK', 4.0), 'dcma': ('VJACK', 4.0)}),
    'real-vjack-blind-prec': ('open;vsrc:24;vjblind:1', 'v24', [], ['dcv60', 'dca', 'temp', 'dca', 'res', 'dcma', 'dcv', 'dca'],
                              {'dca': ('VJACK', 4.0), 'dcma': ('VJACK', 4.0)}),
}
# Seconds per step instead of --dwell.
DWELL = {'ohmexit-long': 11.0}
# Commands sent once after the boot, before the sequence (device experiments).
SETUP = {'real-vjack-blind-prec': ['DBG:SENT:PREC 300']}


def fmt(ents, ts):
    return ' '.join('%s/%s@+%.2f' % (e[0], e[1], e[2] - ts) for e in ents)


def run(name):
    spec, inp, mon, seq, real = SC[name][:5]
    later = SC[name][5] if len(SC[name]) > 5 else []
    seq = seq or (full_seq() if A.full else SEQ)
    if uc.port_busy(A.port):
        print('RESULT %s FAIL port %d in use (another emulator?): pass a free --port' % (name, A.port), flush=True)
        return False
    tag = 'switch-' + name
    rtag = '%s-%d-%d' % (tag, A.port, os.getpid())
    os.makedirs(OUT, exist_ok=True)
    sp = os.path.join(OUT, rtag + '.script')
    with open(sp, 'w') as f:
        for m in mon:
            f.write('0 monitor %s\n' % m)
        f.write('0 dmm %s\n' % spec)
        for t, sp2 in later:
            f.write('%g dmm %s\n' % (t, sp2))
    run_for = 30 + len(seq) * (DWELL.get(name, A.dwell) + 4) + sum(5 for s in seq if s in ('WAVE', 'SCOPE'))
    cmd = [sys.executable, os.path.join(EMU, 'run.py'), 'fwrs', '--fast', '--usbip', '3',
           '--usbip-port', str(A.port), '--run-for', str(int(run_for)), '--script', sp, '--name', rtag] \
        + (['--app', os.path.abspath(A.app)] if A.app else [])
    p = subprocess.Popen(cmd, cwd=ROOT, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)
    mk = emusync.Markers(emusync.log_path(OUT, rtag))
    t_wall = time.time()
    tm = None
    while p.poll() is None and tm is None and time.time() - t_wall < 300:
        try:
            tm = uc.Usbtmc(uc.Dev('1-0'))
        except Exception as e:
            print('attach:', type(e).__name__, e, flush=True)
            time.sleep(1)
    bad, steps = [], []

    def q(line):
        tm.write(line)
        return tm.read(timeout=60).decode(errors='replace').strip()

    def up():
        return int(q('DBG:PING?').split(',')[0]) / 1000.0

    # DBG:SENTinel? needs --features dbg-sent (refused: an empty reply); it
    # is only recorded here.
    HAS_SENT = tm is not None and bool(q('DBG:SENT?'))
    if tm is not None and not HAS_SENT:
        tm.write('*CLS')

    def log():
        s = q('DBG:SENT:LOG?').split(';')
        return int(s[0]), [(a, k, int(t) / 1000.0) for a, k, t in (e.split(',') for e in s[1:])]

    if tm is not None:
        try:
            q('*IDN?')
            while up() < 6.0 and p.poll() is None:  # past the boot's own settle
                time.sleep(0.3)
            # `later` inputs (virtual t) land during the first step: in fw uptime (their
            # markers, emu/emusync.py; predicted by the clock marker) it holds until 1 s after
            # them, whatever the boot's length; one before the first step fails the run.
            if later and mk.wait_clock() is None:
                bad.append('no clock marker (dwt: CYCCNT write) in the log')
            pend = sorted(mk.at(t) for t, _ in later) if later and mk.zero is not None else []
            prev = None
            for c in SETUP.get(name, []):
                tm.write(c)
                print('%s setup %s -> %s' % (name, c, q('SYST:ERR?')), flush=True)
            seen_n, _ = log()
            for i, st in enumerate(seq):
                if st == 'WAVE':
                    if prev not in WAVE_OK:
                        continue
                    tm.write('DBG:KEY 3')
                    time.sleep(0.6)
                    tm.write('DBG:TAP 298,259')
                    w = up() + 3.0
                    while up() < w and p.poll() is None:
                        time.sleep(0.2)
                    ts = up()
                    tm.write('DBG:KEY 0')
                    fn = prev
                elif st == 'SCOPE':
                    tm.write('DBG:KEY 0')
                    w = up() + 0.4
                    while up() < w and p.poll() is None:
                        time.sleep(0.1)
                    tm.write('DBG:TAP 341,107')
                    w = up() + 3.0
                    while up() < w and p.poll() is None:
                        time.sleep(0.2)
                    tm.write('DBG:KEY 0')
                    w = up() + 0.4
                    while up() < w and p.poll() is None:
                        time.sleep(0.1)
                    ts = up()
                    tm.write('DBG:TAP 140,107')
                    fn = prev
                else:
                    ts = up()
                    for c in F[st]:
                        tm.write(c)
                    fn = st
                rec = []
                dwell = DWELL.get(name, A.dwell)
                until = ts + max(dwell, real[fn][1] + 1.0 if fn in real else 0) + (2.5 if st in SLOW else 0)
                if i == 0 and pend:
                    if pend[0] <= ts:
                        bad.append('step 0 %s: a later input landed before it (fw %.2f s, step at %.2f s)' % (st, pend[0], ts))
                    until = max([until] + [x + 1.0 for x in pend])
                while p.poll() is None:
                    t = up()
                    rec.append((t, q('SYST:ALER?'), q('DBG:SENT?') if HAS_SENT else ''))
                    if t >= until:
                        break
                n, ents = log()
                new = ents[-(n - seen_n):] if n > seen_n else []
                if n - seen_n > len(ents):
                    bad.append('step %d %s: %d assertions, the log holds %d' % (i, st, n - seen_n, len(ents)))
                seen_n = n
                ok_set = allowed_for(inp, fn) | {'NONE'}
                wrong = [e for e in new if e[0] not in ok_set] + [(r[1], 'poll', r[0]) for r in rec if r[1] not in ok_set]
                func = q('FUNC?')
                lat = None
                if fn in real and st != 'WAVE':
                    want, lim = real[fn]
                    hit = [e for e in new if e[0] == want] + [(r[1], 'poll', r[0]) for r in rec if r[1] == want]
                    lat = min(e[2] for e in hit) - ts if hit else None
                    # an alert that stayed on through the switch (latched A-PROT): held
                    held = bool(rec) and all(r[1] == want for r in rec) and not new
                    if not held and (lat is None or lat > lim):
                        bad.append('step %d %s<-%s: %s after %s s, want <= %g' % (i, st, prev, want, lat, lim))
                steps.append({'i': i, 'step': st, 'from': prev, 'func': func, 'ts': ts, 'new': new, 'rec': rec, 'lat': lat})
                print('%s %3d %-6s <- %-6s %-14s t %7.2f log %s%s %s' % (
                    name, i, st, prev, func, ts, fmt(new, ts) or '-', '' if lat is None else ' lat %.2f' % lat,
                    'BAD' if wrong else ''), flush=True)
                if wrong:
                    bad.append('step %d %s<-%s: %s' % (i, st, prev, fmt(wrong[:3], ts)))
                prev = fn
        except Exception as e:
            print(name, 'link ended:', type(e).__name__, e, flush=True)
            bad.append('link ended: %s' % e)
    p.terminate()
    try:
        out, _ = p.communicate(timeout=60)
    except subprocess.TimeoutExpired:
        p.kill()
        out, _ = p.communicate()
    for x in (sp, os.path.join(OUT, 'flash-%s.bin' % rtag), os.path.join(OUT, rtag + '.resc')):
        if os.path.exists(x):
            os.remove(x)
    ok = not bad and len(steps) >= len([s for s in seq if s not in ('WAVE', 'SCOPE')])
    with open(os.path.join(OUT, tag + '.json'), 'w') as f:
        json.dump({'name': name, 'ok': ok, 'bad': bad, 'steps': steps}, f, indent=1)
    print('RESULT %s %s %s' % (name, 'OK' if ok else 'FAIL', '; '.join(bad[:6])), flush=True)
    return ok


if __name__ == '__main__':
    if A.list:
        for n, v in SC.items():
            print('%-11s %-26s %s' % (n, v[0], ' '.join('%s:%s<=%gs' % (k, a, t) for k, (a, t) in v[4].items())))
        sys.exit(0)
    allok = True
    for n in A.names or list(SC):
        allok &= run(n)
    sys.exit(0 if allok else 1)
