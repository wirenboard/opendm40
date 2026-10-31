#!/usr/bin/env python3
"""The continuity / diode beeper indicator (firmware/src/app.rs `beep_ind`, ui.rs
`short_sync`) in the emulator: opendm40 native + the HY3131 model, the LCD write
trace (`sysbus.lcd Trace`, emu/lcdtrace.py), SCPI over USB/IP for the function and
the volume.

The model's continuity comparator (HY3131.cs, Role.Cont) follows the input at once;
the firmware reads it (CTSTA bit 6) every DMM poll. The input is switched at fixed
virtual times (run.py --dmm SPEC@T); the trace is replayed and the indicator's band
(ui::SHORT_RECT, ui::SHORT_BG) and the green digits (ui::SHORT_FG) are sampled after
every LCD write burst. Phases (L s each, the commands sent ~2 s before the first
switch):
  cont50  CONT, volume 50 %   short 2.0..3.0 s, then a chattering contact
  cont0   CONT, volume 0 %    (20 ms open / short for 300 ms from 3.5 s)
  diod50  DIOD, volume 50 %   short 2.0..3.0 s, a 0.6 V diode 4.0..5.5 s
  diod0   DIOD, volume 0 %
Checked: the band and the digits on within MAX_ON_MS of every short (SHORT in
continuity and on a diode-mode short), off within HOLD + MAX_ON_MS after it ends
and not before the hold (150 ms); the chattering contact gives one on / off;
the diode gives the band (BEEP) and it goes again (the pattern plays for the first
readings only); nothing green while open. Latencies are printed (virtual time,
--mips 200 = the device's CPU speed, inferred; the FSMC bus is not modelled).
Exit status 0 = PASS. Results: emu/out/contvis.json.

  emu/hy3131/contvistest.py --app firmware/target/native/thumbv7em-none-eabihf/release/opendm40 --port 40310
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
ap.add_argument('--port', type=int, default=40310, help='USB/IP port (default 40310)')
ap.add_argument('--mips', type=float, default=200, help='CPU MIPS (default 200: real speed)')
ap.add_argument('--tag', default='', help='name suffix of the results (e.g. mute)')
ap.add_argument('--replay', metavar='TRACE', help='only check an LCD trace of an earlier run')
ap.add_argument('--dump', action='store_true', help='print the indicator timeline per event')
A = ap.parse_args()
os.environ['USBIP_PORT'] = str(A.port)
sys.path.insert(0, EMU)
import lcdtrace  # noqa: E402
import usbip_client as uc  # noqa: E402

SHORT_BG, SHORT_FG, BG = 0x04A9, 0x07E0, 0x0883
BAND_PX = (200, 39)          # inside ui::SHORT_RECT (8, 38, 464, 26), above its text
DIGITS = (68, 148, 40, 400)  # rows, columns of the main readout
HOLD_MS = 150
MAX_ON_MS = 40
T0, L = 9.0, 6.0
SHORT, OPEN, DIODE = 'r:0.5', 'open', 'diode:0.6'
PHASES = [
    ('cont50', ['SYST:BEEP:VOL 50', 'CONF:CONT'], 'cont'),
    ('cont0', ['SYST:BEEP:VOL 0'], 'cont'),
    ('diod50', ['SYST:BEEP:VOL 50', 'CONF:DIOD'], 'diod'),
    ('diod0', ['SYST:BEEP:VOL 0'], 'diod'),
    ('dcv', ['CONF:VOLT:DC'], 'none'),
    ('entry', ['CONF:CONT'], 'entry'),
]
# Entering CONT with the probes already together: the indicator from the
# row write on (the beeper sounds at once), not after the entry settle.
ENTRY_MAX_MS = 200
CHAT_T, CHAT_N, CHAT_MS = 3.5, 15, 20
# An input event the model logs with the virtual time and the firmware's
# clock ("input: open at 6.0000 fw 3.28"): the firmware clock's offset, so
# that the SCPI commands land LEAD s before each phase.
SYNC_T, LEAD = T0 - 3.0, 0.8


def schedule():
    """[(virtual s, spec)] and the expected events per phase."""
    ev = [(0.0, OPEN), (SYNC_T, OPEN)]
    exp = []
    for i, (ph, _, kind) in enumerate(PHASES):
        s = T0 + i * L
        if kind == 'none':
            # DC V: shorted from 4.5 s on, no indicator; still shorted
            # when the next phase enters CONT.
            ev.append((s + 4.5, SHORT))
            exp.append((ph, 'none', s + 4.5, s + L - LEAD - 0.05))
            continue
        if kind == 'entry':
            # ta: when CONF:CONT landed (`main` sets it from the firmware clock).
            ev.append((s + 2.0, OPEN))
            exp.append((ph, 'entry', s - LEAD, s + 2.0))
            continue
        ev += [(s + 2.0, SHORT), (s + 3.0, OPEN)]
        exp.append((ph, 'short', s + 2.0, s + 3.0))
        if kind == 'cont':
            for k in range(CHAT_N):
                ev.append((s + CHAT_T + k * CHAT_MS / 1000.0, SHORT if k % 2 == 0 else OPEN))
            ev.append((s + CHAT_T + CHAT_N * CHAT_MS / 1000.0, OPEN))
            exp.append((ph, 'chatter', s + CHAT_T, s + CHAT_T + CHAT_N * CHAT_MS / 1000.0))
        else:
            ev += [(s + 4.0, DIODE), (s + 5.5, OPEN)]
            exp.append((ph, 'diode', s + 4.0, s + 5.5))
    return ev, exp


def replay(path):
    """[(t, band_on, digits_green)] after every write burst that changed them."""
    recs = lcdtrace.load(path)
    sc = lcdtrace.Screen()
    out = []
    last = None
    r0, r1, c0, c1 = DIGITS
    for rec in recs:
        sc.apply(rec)
        _, _, xs, xe, ys, ye = rec[:6]
        band = int(sc.px[BAND_PX[1], BAND_PX[0]]) == SHORT_BG
        green = bool((sc.px[r0:r1, c0:c1] == SHORT_FG).any()) if ye >= r0 and ys < r1 else (last[2] if last else False)
        st = (band, green)
        if last is None or st != last[1:]:
            out.append((rec[0], band, green))
            last = (rec[0], band, green)
    return out


def state_at(tl, t):
    s = (False, False)
    for tt, b, g in tl:
        if tt > t:
            break
        s = (b, g)
    return s


def check(tl, exp, bad):
    """The indicator against the input schedule; appends to `bad`."""
    rows, lat_on, lat_off = [], [], []

    def anyon(x):
        return x[1] or x[2]

    for ph, what, ta, tb in exp:
        seg = [x for x in tl if ta - 0.5 <= x[0] <= tb + 1.0]
        if what == 'none':
            if any(anyon(x) for x in tl if ta <= x[0] <= tb) or any(state_at(tl, ta)):
                bad.append('%s: an indicator outside continuity / diode' % ph)
            print('%-6s %-7s %s' % (ph, what, 'off' if not bad or not bad[-1].startswith(ph) else 'ON'), flush=True)
            continue
        if A.dump:
            for x in seg:
                print('   %9.4f band %d digits %d' % x)
        before = state_at(tl, ta - 0.01)
        if before != (False, False):
            bad.append('%s %s: indicator on before the input (%s)' % (ph, what, before))
        ons = [x for x in seg if x[0] >= ta and anyon(x)]
        if not ons:
            bad.append('%s %s: no indicator' % (ph, what))
            continue
        t_on = ons[0][0]
        both = next((x for x in seg if x[0] >= ta and x[1] and x[2]), None)
        # Off: the first write burst after the input's end that leaves
        # neither part on (diode: the patterns before the short's steady
        # tone come and go, as the beeper does).
        offs = [x for x in seg if x[0] > (t_on if what == "diode" else tb) and not anyon(x)]
        t_off = offs[0][0] if offs else None
        on_ms = (t_on - ta) * 1000.0
        row = {'ph': ph, 'what': what, 'in_on': ta, 'in_off': tb, 'on': t_on, 'both': both and both[0],
               'off': t_off, 'on_ms': round(on_ms, 2)}
        if t_off is not None:
            row['off_ms'] = round((t_off - tb) * 1000.0, 2)
        # Indications (rising edges) from the input's start to the end.
        end = t_off if t_off is not None else tb + 1.0
        pre = [(ta - 0.01,) + state_at(tl, ta - 0.01)]
        rises = sum(1 for a, b in zip(pre + seg, seg) if not anyon(a) and anyon(b) and ta <= b[0] <= end)
        row['rises'] = rises
        rows.append(row)
        print('%-6s %-7s on %+7.1f ms  off %s  indications %d' % (ph, what, on_ms, '%+7.1f ms' % row['off_ms'] if t_off else '-', rises), flush=True)
        if what == 'entry':
            if on_ms > ENTRY_MAX_MS:
                bad.append('%s: on %.1f ms after CONF:CONT on shorted probes (> %d)' % (ph, on_ms, ENTRY_MAX_MS))
            if t_off is None:
                bad.append('%s: indicator never off' % ph)
            continue
        if what == 'diode':
            if t_off is None:
                bad.append('%s diode: the BEEP band stayed on (the pattern plays for the first readings only)' % ph)
            if not any(x[1] for x in seg if x[0] >= ta):
                bad.append('%s diode: no band' % ph)
            continue
        lat_on.append(on_ms)
        if on_ms > MAX_ON_MS:
            bad.append('%s %s: on %.1f ms after the input (> %d)' % (ph, what, on_ms, MAX_ON_MS))
        if both is None or (both[0] - ta) * 1000.0 > MAX_ON_MS:
            bad.append('%s %s: band + green digits not both on within %d ms' % (ph, what, MAX_ON_MS))
        if not anyon(state_at(tl, tb - 0.01)):
            bad.append('%s %s: indicator off before the input ended' % (ph, what))
        if t_off is None:
            bad.append('%s %s: indicator never off' % (ph, what))
        else:
            off_ms = (t_off - tb) * 1000.0
            lat_off.append(off_ms)
            if off_ms < HOLD_MS - 5 or off_ms > HOLD_MS + MAX_ON_MS:
                bad.append('%s %s: off %.1f ms after the input (want %d..%d)' % (ph, what, off_ms, HOLD_MS, HOLD_MS + MAX_ON_MS))
        # Continuity: one indication per contact, also a chattering one.
        if ph.startswith('cont') and rises != 1:
            bad.append('%s %s: %d indications (flicker)' % (ph, what, rises))
    return rows, lat_on, lat_off


def main():
    os.makedirs(OUT, exist_ok=True)
    if A.replay:
        bad = []
        check(replay(A.replay), schedule()[1], bad)
        print('RESULT replay %s; %s' % ('OK' if not bad else 'FAIL', '; '.join(bad)))
        return not bad
    name = 'contvis%s.%d' % (A.tag, os.getpid())
    trace = os.path.join(OUT, '%s.trace' % name)
    ev, exp = schedule()
    run_for = T0 + len(PHASES) * L + 1.0
    script = os.path.join(OUT, '%s.script' % name)
    with open(script, 'w') as f:
        f.write('1.0 monitor sysbus.lcd Trace "%s"\n' % trace)
        for t, spec in ev:
            f.write('%.3f dmm %s\n' % (t, spec))
    cmd = [sys.executable, os.path.join(EMU, 'run.py'), 'fwrs', '--mips', str(A.mips), '--usbip', '3',
           '--usbip-port', str(A.port), '--run-for', str(run_for), '--name', name, '--script', script] \
        + (['--app', os.path.abspath(A.app)] if A.app else [])
    runlog = open(os.path.join(OUT, '%s.out' % name), 'w')
    p = subprocess.Popen(cmd, cwd=ROOT, stdout=runlog, stderr=subprocess.STDOUT)
    tm = None
    t_wall = time.time()
    while p.poll() is None and tm is None and time.time() - t_wall < 600:
        try:
            tm = uc.Usbtmc(uc.Dev('1-0'))
        except Exception as e:
            print('attach:', type(e).__name__, e, flush=True)
            time.sleep(2)
    bad = []

    def q(s):
        tm.write(s)
        return tm.read(timeout=120).decode(errors='replace').strip()

    sent = []
    if tm is None:
        bad.append('no USB/IP attach')
    else:
        try:
            off = None
            log = os.path.join(OUT, '%s.log' % name)
            while off is None and p.poll() is None:
                try:
                    with open(log, errors='replace') as f:
                        for ln in f:
                            m = re.search(r'input: \S+ at ([0-9.]+) fw ([-0-9.]+)', ln)
                            if m and abs(float(m.group(1)) - SYNC_T) < 1e-3:
                                off = float(m.group(1)) - float(m.group(2))
                except OSError:
                    pass
                time.sleep(1)
            print('fw clock = virtual time - %.3f s' % off, flush=True)
            for i, (ph, scpi, _) in enumerate(PHASES):
                # LEAD s before the phase (virtual), after the previous
                # phase's last switch.
                due = T0 + i * L - LEAD - off
                while True:
                    t = int(q('DBG:PING?').split(',')[0]) / 1000.0
                    if t >= due:
                        break
                    time.sleep(0.2)
                for c in scpi:
                    tm.write(c)
                t = int(q('DBG:PING?').split(',')[0]) / 1000.0
                sent.append((ph, t))
                exp = [(e[0], e[1], t + off, e[3]) if e[0] == ph and e[1] == 'entry' else e for e in exp]
                print('%-6s sent at fw %.2f s (phase from %.2f)' % (ph, t, T0 + i * L), flush=True)
                if t + off > T0 + i * L:
                    bad.append('%s: commands late (virtual %.2f s)' % (ph, t + off))
        except Exception as e:
            bad.append('link ended: %s' % e)
    try:
        p.wait(timeout=1800)
    except subprocess.TimeoutExpired:
        p.kill()
        p.wait()
    runlog.close()
    try:
        tl = replay(trace)
    except (OSError, SystemExit) as e:
        bad.append('trace: %s' % e)
        tl = []
    rows, lat_on, lat_off = check(tl, exp, bad)
    ok = not bad
    res = {'ok': ok, 'bad': bad, 'rows': rows, 'sent': sent,
           'on_ms_max': max(lat_on) if lat_on else None, 'off_ms': lat_off}
    with open(os.path.join(OUT, 'contvis%s.json' % A.tag), 'w') as f:
        json.dump(res, f, indent=1)
    if ok:
        for x in (trace, script, runlog.name, os.path.join(OUT, '%s.log' % name), os.path.join(OUT, '%s.resc' % name), os.path.join(OUT, 'flash-%s.bin' % name)):
            try:
                os.remove(x)
            except OSError:
                pass
    print('RESULT contvis%s %s on <= %s ms; %s' % (A.tag, 'OK' if ok else 'FAIL', res['on_ms_max'], '; '.join(bad[:6])), flush=True)
    return ok


if __name__ == '__main__':
    sys.exit(0 if main() else 1)
