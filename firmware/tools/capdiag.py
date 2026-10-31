#!/usr/bin/env python3
"""Capacitance diagnostic log (#74): FETC?, the range and DBG:CAP? (the firmware's capacitance
internals, dmm.rs cap_info) at a fixed poll rate into a CSV, to tell range hunting from the
measurement itself or noise.

    tools/capdiag.py OUT.csv [--secs 60] [--hz 4] [--range AUTO|6e-9..6e-2] [--label 100uF]
    tools/capdiag.py OUT.csv --emu 37101 ...      the emulator (USB/IP port), no kernel attach

--range: CONF:CAP <range> first (a fixed range, SCPI only), AUTO (default) = CONF:CAP.
--keep: do not send CONF:CAP (log what the meter is doing now).
The meter: USBTMC as tools/dm40dbg.py (DM40_USBTMC picks the device). Columns: host time s,
uptime ms, FETC? (F), range (F, CONF?), then DBG:CAP?'s 18 numbers (docs/manual/
scpi-reference.md): range index, manual, phase (0 oscillator, 1 discharge, 2 ramp), oscillator
Hz, shown C uF, ramp V mV, dV mV, dt ms, median dV mV, C of the poll uF, oscillator C uF, class,
samples, comparator, polls without a ramp, up count, no final value yet (1), same-range restarts. A summary at the end:
range changes, the values' spread per range.
"""
import argparse
import csv
import os
import statistics
import sys
import time

HERE = os.path.dirname(os.path.abspath(__file__))
COLS = ['t_s', 'uptime_ms', 'fetch_F', 'range_F', 'r', 'manual', 'phase', 'osc_hz', 'c_uF', 'v_mV', 'dv_mV', 'dt_ms',
        'dvm_mV', 'c_poll_uF', 'c_osc_uF', 'class', 'n', 'cmp', 'idle', 'cnt_up', 'first', 'swaps']


def link(a):
    if a.emu:
        os.environ['USBIP_PORT'] = str(a.emu)
        sys.path.insert(0, os.path.join(HERE, '..', '..', 'emu'))
        import usbip_client as uc
        t = uc.Usbtmc(uc.Dev('1-0'))

        def q(s):
            t.write(s)
            return t.read(timeout=10).decode(errors='replace').strip() if '?' in s else ''
        return q
    sys.path.insert(0, HERE)
    import dm40dbg
    ln = dm40dbg.open_link(10)
    if ln is None:
        sys.exit('capdiag: no opendm40 USBTMC device')

    ln.fresh()

    def q(s):
        if '?' in s:
            return ln.query(s).decode(errors='replace').strip()
        ln.write(s)
        return ''
    return q


def num(s):
    try:
        return float(s)
    except ValueError:
        return float('nan')


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument('out')
    ap.add_argument('--secs', type=float, default=60)
    ap.add_argument('--hz', type=float, default=4)
    ap.add_argument('--range', default='AUTO')
    ap.add_argument('--keep', action='store_true')
    ap.add_argument('--label', default='')
    ap.add_argument('--emu', type=int, default=0, help='emulator USB/IP port')
    a = ap.parse_args()
    q = link(a)
    if not q('DBG:CAP?'):
        sys.exit('capdiag: no DBG:CAP? (firmware without it?)')
    if not a.keep:
        q('CONF:CAP' if a.range.upper() == 'AUTO' else 'CONF:CAP %s' % a.range)
        err = q('SYST:ERR?')
        if not err.startswith(('0', '+0')):
            sys.exit('capdiag: CONF:CAP %s: %s' % (a.range, err))
    rows = []
    t0 = time.time()
    with open(a.out, 'w', newline='') as f:
        w = csv.writer(f)
        f.write('# capdiag %s range %s %s\n' % (a.label, a.range, time.strftime('%Y-%m-%d %H:%M:%S')))
        w.writerow(COLS)
        while time.time() - t0 < a.secs:
            tn = time.time()
            up = q('DBG:PING?').split(',')[0]
            v = num(q('FETC?'))
            conf = q('CONF?').strip('"')
            rng = num(conf.split()[1].split(',')[0]) if ' ' in conf else float('nan')
            cap = [num(x) for x in q('DBG:CAP?').split(',')]
            row = [round(tn - t0, 3), up, v, rng] + cap
            w.writerow(row)
            f.flush()
            rows.append(row)
            print('%7.2f %-14s %-9g %s' % (row[0], '%g' % v, rng, ' '.join('%g' % x for x in cap)), flush=True)
            time.sleep(max(0.0, 1.0 / a.hz - (time.time() - tn)))
    ch = sum(1 for p, n in zip(rows, rows[1:]) if p[3] != n[3])
    print('# %d polls, %d range changes' % (len(rows), ch))
    for r in sorted({x[3] for x in rows if x[3] == x[3]}):
        vs = [x[2] for x in rows if x[3] == r and x[2] < 1e37]
        if vs:
            print('# range %g: %d values, median %g, min %g, max %g, stdev %g' % (
                r, len(vs), statistics.median(vs), min(vs), max(vs), statistics.pstdev(vs)))


if __name__ == '__main__':
    main()
