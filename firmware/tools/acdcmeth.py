#!/usr/bin/python3
"""Device experiments of docs/acdc-approaches.md section 8 for the AC+DC
methods A / B / C (src/acdcm.rs, DBG:ACDC:METH), with what the MR9270S+
calibrator gives: DC up to 24 V (mr9270.VOLTAGE_MAX: a 30 V request puts out
23.99 V) and its small 40 Hz square (tools/mr9270.py,
'frequency' output). The meter on opendm40 over USB (USBTMC), leads on V / COM,
on an image built with `--features acdc-meth` (Cargo.toml; the default build
refuses DBG:ACDC:METH / STAT? / ZERO / R0).

  e1  note 7 / B's zero: per V range (DBG:ACDC:RANG 1..4) at 0 V and the
      calibrator's top level (24 V), B entered N times (each entry measures the zero:
      the stock DC V row vs the merged row's AD1): the zero, its spread over
      the repeats and against the level (offset vs gain), and B's DC part vs
      the DC V function (READ?).
  e2  AD1 inside the AC V row: the 40 Hz square on 6 V / 60 V: the AC part
      of A (stock alternation), B (AD1 converting next to the RMS) and C (the
      DC-coupled node); the DC part of each against the DC V function. The
      square is 40 Hz: an AD1 notch at 10 S/s, so this is not a ripple test.
  e3  ripple leak of the DC part (25 .. 400 Hz): NEEDS A GENERATOR (the
      calibrator's 40 Hz is a notch of every AD1 method). Skipped; the
      40 Hz rows of e2 are the notch control point.
  e4  low-frequency AC part (20 .. 45 Hz): NEEDS A GENERATOR. Skipped.
  e5  C's self-cal over time: C re-entered every --period s for --minutes at
      the e5 level (default 24 V, 60 V range, and 3 V, 6 V range): per_code,
      offset (DBG:ACDC:STAT?) and C's DC part vs READ? of the DC V function;
      drift in ppm against the first entry (warm the meter meanwhile:
      backlight on, charger).
  e6  entry and range change: from DC V into AC+DC per method: time to the
      first reading and to within 0.1 % + 2 digits of the final value
      (FETCh? polled); then 3 V -> 24 V (6 V -> 60 V autorange) and back:
      the settle times; the reading rate (n_rd / s).
  e7  B's 6 V row: DBG:ACDC:R0 1 (AD1 SENSE - PB5, the 60 V calibration)
      vs 2 (OP1O - RLU after x10) at 1 / 3 / 5 V: B's DC part (zero applied)
      vs the DC V function; the gain of (ii) from the slope; the 40 Hz square
      with each to see the AC part unchanged.
  e8  C's DC part away from its anchor (the 20a91d0 fix: the anchor is the
      DC V row's READ?, the gain from the internal zero): DC V READ? at each
      level first (manual range), then C entered once at the top level and
      the calibrator stepped down without leaving: 60 V at 24 / 12 / 5 / 1 /
      0 V, 6 V at 5 / 3 / 1 / 0 V; C's DC part vs READ? (rule 0.05 % + 5
      digits). Then C re-entered at 5 V on 60 V (span < 10 k codes: the gain
      kept from the 24 V anchor, STAT? gspan) against READ?.
      e8s: the same with 4 s per level before the 3 s mean (C's second
      reference after a level change, 6bf38f4: ~3 s). e8up: 60 V only,
      entered at 0 V and stepped up 0 / 1 / 5 / 12 / 24 V, 4 s per level
      (the gain from two references from the first step >= 10 k codes on);
      run it first after a power-on (the gain is kept per range until then).
      e8l / e8upl: e8s / e8up with 10 s per level (since 10.12 C waits for
      the LPF to stop creeping before its second reference: ~4..8 s after a
      step, so e8s's 4 s may end a level before it). STAT? gsrc (0 table, 1
      internal zero, 2 two refs, 3 two refs over a biased zero), gdis (the
      last two-ref gain vs the one in use over its span, codes), slope.
  tc6 the 80d5b01 6 V failure as a time course: entered at 5 V on 6 V, step
      to 3 V, STAT? at ~10 Hz for 15 s, then C vs READ? at 3 / 1 / 0 V (10 s
      per level). Run after a power cycle or `dm40dbg.py reset`.
  e9  the >= 5 V DC fault of 20a91d0 (6 V range, AUTO range): C in AC+DC at
      4.0 / 4.5 / 5.0 / 5.5 V for 12 s each: range changes, readings / s,
      the AC part (max) and its source (STAT? acsrc: 0 RMS, 1 the peak
      bound, 2 none), the DC part vs READ?. Pass: no range change after the
      entry, >= 6 readings / s, AC part < 30 mV, DC within 0.05 % + 5 dig.
  e10 AUTO after the square (20a91d0: C stayed on AC ~4.056 V): VOLT:COUP
      AUTO with C on the 40 Hz square (12 s), then 5.0 / 5.5 / 4.5 V DC (15
      s each): VOLT:COUP?, FETCh?, range; pass: AUTO,DC within 0.05 % +
      5 digits of READ? and one range over each level's last 8 s.
  e11 AUTO on the 40 Hz square (ac1e852: A went to AC on its first AC
      part's overshoot, C stayed DC), A and C entered 3 times each,
      VOLT:COUP? for 15 s: pass = DC, then at most one change, the same part
      in every entry (dmm.rs AUTO_TIE: a tie within 1 % is DC).

    /usr/bin/python3 tools/acdcmeth.py e1 e2 e5 e6 e7 -o acdcmeth.jsonl
    /usr/bin/python3 tools/acdcmeth.py e8 e9 e10 e2 e6 -o acdcfix.jsonl  # retest
    /usr/bin/python3 tools/acdcmeth.py e5 --minutes 3 -o acdcfix.jsonl      # + e5, short
    /usr/bin/python3 tools/acdcmeth.py e6 --no-cal          # input left alone
    tools/acdcmeth.py e6 --usbip 1-0 --no-cal               # the emulator

Each result line goes to stdout and, as JSON, to -o. At the end: the
method back to A, CONF:VOLT:DC (AUTO), the calibrator to 1 V DC.
"""
import argparse
import json
import math
import os
import sys
import time

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)

# The calibrator's top DC level (tools/mr9270.py VOLTAGE_MAX; not imported
# from there: mr9270 needs pymodbus, --no-cal runs do not). Before
# 2026-10-23 the levels said 30 V, but the calibrator put out 23.99 V.
VMAX = 24.0
# DBG:ACDC:STAT? fields after `slope` (since 10.14: the last second-reference
# attempt's residual rms, bias threshold, slope, step into the level)
STAT_X = ('psig', 'bthr', 'pslope', 'pstep')
# columns of the per-level trace (--trace: e8 / e8s / e8l / e8up)
TR_COLS = ('step', 'lpf', 'pc', 'off', 'gspan', 'gsrc', 'gdis', 'slope', 'pp', 'dc') + STAT_X
FULL = {1: 6.0, 2: 60.0, 3: 600.0, 4: 1000.0}
E1_LEVELS = {1: [0.0, 3.0], 2: [0.0, VMAX], 3: [0.0, VMAX], 4: [0.0, VMAX]}


class Run:
    def __init__(self, a):
        import dm40scpi
        self.d = dm40scpi
        self.link = dm40scpi.UsbipLink(a.usbip) if a.usbip else dm40scpi.UsbLink()
        self.cal = None
        if not a.no_cal:
            import mr9270
            self.cal = mr9270.MR9270(a.cal).connect()
        self.out = open(a.out, 'a') if a.out else None
        self.a = a
        self.link.query('*CLS')
        self.w('DBG:ACDC:METH A')
        err = self.q('SYST:ERR?')
        if err.startswith('-113'):
            sys.exit('acdcmeth.py: the meter needs an image built with --features acdc-meth (%s)' % err)

    def q(self, line):
        return self.link.query(line).strip()

    def w(self, line):
        self.link.query(line)

    def log(self, **kv):
        kv['t'] = round(time.time(), 2)
        print(' '.join('%s=%s' % (k, ('%.6g' % v) if isinstance(v, float) else v) for k, v in kv.items()), flush=True)
        if self.out:
            self.out.write(json.dumps(kv) + '\n')
            self.out.flush()

    def src_dc(self, v):
        if abs(v) > VMAX:
            sys.exit('acdcmeth: %g V above the %g V cap' % (v, VMAX))
        if self.cal:
            self.cal.set_output('voltage', v, enable=True)
            time.sleep(self.a.settle)

    def src_sq(self, hz=40):
        if self.cal:
            self.cal.set_output('frequency', hz, enable=True)
            time.sleep(self.a.settle)

    def stat(self):
        f = self.q('DBG:ACDC:STAT?').split(',')
        k = ['meth', 'step', 'mode', 'range', 'ac', 'dc', 'n_ac', 'n_dc', 'n_rd', 'hz', 'corr', 'pc', 'off', 'lpf', 'sq',
             'pp', 'acsrc', 'gspan', 'gsrc', 'gdis', 'slope'] + list(STAT_X)
        s = {n: (float(x) if '.' in x or 'E' in x else int(x)) for n, x in zip(k, f)}
        if getattr(self, 'tr', None) is not None:
            self.tr.append([round(time.time() - self.t_step, 2)] + [s.get(c) for c in TR_COLS])
        return s

    def sleep_tr(self, secs):
        """sleep, or poll STAT? at ~10 Hz while a trace is on (--trace)."""
        if getattr(self, 'tr', None) is None:
            time.sleep(secs)
            return
        t_end = time.time() + secs
        while time.time() < t_end:
            self.stat()
            time.sleep(0.07)

    def fetch(self):
        try:
            v = float(self.q('FETC?'))
        except ValueError:
            return float('nan')
        return float('nan') if abs(v) > 1e37 else v

    def dcv_ref(self, item, n=3):
        """READ? of the DC V function at range item (manual), then back."""
        self.w('CONF:VOLT:DC')
        time.sleep(0.5)
        self.w('DBG:ACDC:RANG %d' % item)
        time.sleep(1.5)
        v = [float(self.q('READ?')) for _ in range(n)]
        return sum(v) / n

    def enter(self, meth, item=None, wait=4.0):
        """AC+DC with method `meth` (re-entry: the zero / self-cal again);
        returns once the method runs (step 2) with 3 new readings, or after
        30 s, then `wait` - 2 s more (at least 1 s)."""
        self.w('DBG:ACDC:METH %s' % meth)
        self.w('CONF:VOLT:DC')
        time.sleep(0.3)
        self.w('VOLT:COUP ACDC')
        if item is not None:
            time.sleep(0.3)
            self.w('DBG:ACDC:RANG %d' % item)
        time.sleep(0.5)
        n0 = self.stat()['n_rd']
        t0 = time.time()
        while time.time() - t0 < 30:
            s = self.stat()
            if s['step'] == 2 and s['n_rd'] >= n0 + 3:
                break
            time.sleep(0.2)
        time.sleep(max(1.0, wait - 2.0))

    def mean_parts(self, secs=3.0):
        t0, ac, dc, tot = time.time(), [], [], []
        s0 = self.stat()
        while time.time() - t0 < secs:
            s = self.stat()
            ac.append(s['ac'])
            dc.append(s['dc'])
            tot.append(self.fetch())
        s1 = self.stat()
        m = lambda x: sum(x) / len(x)
        sd = lambda x: math.sqrt(max(0.0, m([y * y for y in x]) - m(x) ** 2))
        rate = (s1['n_rd'] - s0['n_rd']) / max(1e-3, time.time() - t0)
        return dict(ac=m(ac), ac_sd=sd(ac), dc=m(dc), dc_sd=sd(dc), tot=m(tot), tot_sd=sd(tot), rate=rate, corr=s1['corr'], pc=s1['pc'], off=s1['off'])

    def close(self):
        try:
            self.w('DBG:ACDC:METH A')
            self.w('CONF:VOLT:AC')
            self.w('CONF:VOLT:DC')
        except Exception:
            pass
        if self.cal:
            try:
                self.cal.safe_shutdown()
            finally:
                self.cal.close()
        self.link.close()


def e1(R):
    for item, levels in E1_LEVELS.items():
        for v in levels:
            R.src_dc(v)
            ref = R.dcv_ref(item)
            zs = []
            for k in range(R.a.repeats):
                R.enter('B', item)
                z = float(R.q('DBG:ACDC:ZERO?').split(';')[1].split(',')[item - 1])
                p = R.mean_parts(2.0)
                zs.append(z)
                R.log(exp='e1', item=item, level=v, k=k, ref=ref, zero=z, b_dc=p['dc'], b_err=p['dc'] - ref, digits=(p['dc'] - ref) / (FULL[item] / 60000))
            m = sum(zs) / len(zs)
            R.log(exp='e1sum', item=item, level=v, zero_mean=m, zero_pp=max(zs) - min(zs), zero_pp_digits=(max(zs) - min(zs)) / (FULL[item] / 60000))


def e2(R):
    R.src_sq(40)
    for item in (1, 2):
        ref = R.dcv_ref(item)
        for meth in 'ABC':
            R.enter(meth, item, wait=6.0 if meth == 'A' else 4.0)
            p = R.mean_parts(6.0 if meth == 'A' else 3.0)
            R.log(exp='e2', item=item, meth=meth, ref_dc=ref, dc_err=p['dc'] - ref, **p)


def e5(R):
    for item, v in ((2, VMAX), (1, 3.0)):
        R.src_dc(v)
        t_end = time.time() + R.a.minutes * 60
        first = None
        while time.time() < t_end:
            ref = R.dcv_ref(item)
            R.enter('C', item)
            p = R.mean_parts(3.0)
            first = first or p['pc']
            # (since the 20a91d0 fix the gain is kept per range, so pc_ppm stays 0 after the
            # first entry; the anchor's offset is measured at every entry)
            R.log(exp='e5', item=item, level=v, ref=ref, c_dc=p['dc'], err=p['dc'] - ref, pc=p['pc'], off=p['off'],
                  pc_ppm=(p['pc'] / first - 1) * 1e6 if first else 0.0, gspan=R.stat().get('gspan', 0))
            time.sleep(max(0.0, R.a.period - 12))


def settle_time(R, final_tol, secs=12.0, t0=None, n0=None):
    """Poll FETCh? and DBG:ACDC:STAT? for `secs` from `t0`; (first AC+DC
    reading s = n_rd above `n0`, or the first FETCh? change without `n0`;
    settled s; final value; FETCh? changes / s over the last half)."""
    t0 = t0 or time.time()
    xs = []
    first = float('nan')
    v0 = R.fetch() if n0 is None else None
    while time.time() - t0 < secs:
        v = R.fetch()
        t = time.time() - t0
        if math.isnan(first) and (R.stat()['n_rd'] > n0 if n0 is not None else v != v0):
            first = t
        xs.append((t, v))
    fin = [v for _, v in xs[-10:] if not math.isnan(v)]
    if not fin:
        return float('nan'), float('nan'), float('nan'), float('nan')
    f = sum(fin) / len(fin)
    tol = final_tol(f)
    st = float('nan')
    for i, (t, _v) in enumerate(xs):
        if all(not math.isnan(v) and abs(v - f) <= tol for _, v in xs[i:]):
            st = t
            break
    half = [x for x in xs if x[0] >= secs / 2]
    ch = sum(1 for a, b in zip(half, half[1:]) if a[1] != b[1])
    rate = ch / max(1e-3, half[-1][0] - half[0][0]) if len(half) > 1 else float('nan')
    return first, st, f, rate


def e6(R):
    tol = lambda f: 0.001 * abs(f) + 2 * (0.0001 if abs(f) < 6 else 0.001)
    for meth in 'ABC':
        R.src_dc(3.0)
        R.w('DBG:ACDC:METH %s' % meth)
        R.w('CONF:VOLT:DC')
        time.sleep(3.0)
        n0 = R.stat()['n_rd']
        t0 = time.time()
        R.w('VOLT:COUP ACDC')
        first, st, f, rate = settle_time(R, tol, t0=t0, n0=n0)
        s0 = R.stat()
        time.sleep(3.0)
        s1 = R.stat()
        R.log(exp='e6', meth=meth, step='entry 3V', first_s=first, settle_s=st, value=f, fetch_changes=rate,
              n_rd_rate=(s1['n_rd'] - s0['n_rd']) / 3.0)
        for v in (VMAX, 3.0):
            t0 = time.time()
            if R.cal:
                R.cal.set_output('voltage', v, enable=True)
            first, st, f, rate = settle_time(R, tol, t0=t0)
            R.log(exp='e6', meth=meth, step='to %gV' % v, first_s=first, settle_s=st, value=f, fetch_changes=rate)


def e7(R):
    for r0 in (1, 2):
        R.w('DBG:ACDC:R0 %d' % r0)
        pts = []
        for v in (1.0, 3.0, 5.0):
            R.src_dc(v)
            ref = R.dcv_ref(1)
            R.enter('B', 1)
            p = R.mean_parts(3.0)
            pts.append((ref, p['dc']))
            R.log(exp='e7', r0=r0, level=v, ref=ref, b_dc=p['dc'], err=p['dc'] - ref, zero=p['corr'], dc_sd=p['dc_sd'])
        (x0, y0), (x1, y1) = pts[0], pts[-1]
        R.log(exp='e7sum', r0=r0, gain=(y1 - y0) / (x1 - x0) if x1 != x0 else float('nan'))
        R.src_sq(40)
        R.enter('B', 1)
        p = R.mean_parts(3.0)
        R.log(exp='e7sq', r0=r0, **p)
    R.w('DBG:ACDC:R0 1')


def digits(item):
    return FULL[item] / 60000


def rule(item, ref):
    """0.05 % + 5 digits of DC V range item (the e5 rule)."""
    return 0.0005 * abs(ref) + 5 * digits(item)


def e8(R, order='down', settle=1.0):
    ranges = ((2, (24.0, 12.0, 5.0, 1.0, 0.0)), (1, (5.0, 3.0, 1.0, 0.0)))
    if order == 'up':
        ranges = ((2, (0.0, 1.0, 5.0, 12.0, 24.0)),)
    exp = 'e8' if order == 'down' else 'e8up'
    for item, levels in ranges:
        refs = {}
        for v in levels:
            R.src_dc(v)
            refs[v] = R.dcv_ref(item)
        R.src_dc(levels[0])
        R.enter('C', item)
        for v in levels:
            R.tr = [] if R.a.trace else None
            R.t_step = time.time()
            R.src_dc(v)
            R.sleep_tr(settle)
            p = R.mean_parts(3.0)
            s = R.stat()
            err = p['dc'] - refs[v]
            R.log(exp=exp, item=item, level=v, ref=refs[v], c_dc=p['dc'], err=err, digits=err / digits(item),
                  ok=abs(err) <= rule(item, refs[v]), pc=p['pc'], off=p['off'], gspan=s.get('gspan', 0), ac=p['ac'],
                  gsrc=s.get('gsrc', -1), gdis=s.get('gdis', 0), slope=s.get('slope', 0), pp=s.get('pp', -1),
                  **{k: s[k] for k in STAT_X if k in s})
            if R.tr is not None:
                R.log(exp=exp + 'trace', item=item, level=v, cols='t ' + ' '.join(TR_COLS), trace=R.tr)
            R.tr = None
    if order == 'up':
        return
    # the gain kept over sessions: re-entered at 5 V on 60 V (a span < 10 k codes)
    R.src_dc(5.0)
    ref = R.dcv_ref(2)
    R.enter('C', 2)
    p = R.mean_parts(3.0)
    s = R.stat()
    R.log(exp='e8kept', item=2, level=5.0, ref=ref, c_dc=p['dc'], err=p['dc'] - ref, ok=abs(p['dc'] - ref) <= rule(2, ref),
          pc=p['pc'], gspan=s.get('gspan', 0))


def tc6(R, lo=3.0, secs=15.0):
    """80d5b01's 6 V failure as a time course: C entered on manual 6 V at
    5 V, stepped to `lo`, STAT? at ~10 Hz for `secs` (step, LPF, gain, its
    source, the window's slope), then C vs READ? at lo / 1 / 0 V (READ?
    right after each C mean). Run after a power cycle / `dm40dbg.py reset`."""
    R.src_dc(5.0)
    ref5 = R.dcv_ref(1, n=5)
    R.enter('C', 1)
    s0 = R.stat()
    R.log(exp='tc6', phase='entered', ref=ref5, **{k: s0.get(k) for k in ('step', 'lpf', 'pc', 'off', 'gspan', 'gsrc', 'dc')})
    t0 = time.time()
    if R.cal:
        R.cal.set_output('voltage', lo, enable=True)
    tc = []
    while time.time() - t0 < secs:
        s = R.stat()
        tc.append([round(time.time() - t0, 2)] + [s.get(k) for k in ('step', 'lpf', 'pc', 'off', 'gspan', 'gsrc', 'gdis', 'slope', 'pp', 'dc')])
        time.sleep(0.08)
    R.log(exp='tc6trace', lo=lo, cols='t step lpf pc off gspan gsrc gdis slope pp dc', trace=tc)
    for v in (lo, 1.0, 0.0):
        if v != lo:
            R.src_dc(v)
            time.sleep(10.0)
        p = R.mean_parts(3.0)
        s = R.stat()
        ref = R.dcv_ref(1, n=5)
        R.log(exp='tc6', level=v, c_dc=p['dc'], ref_after=ref, err=p['dc'] - ref, ok=abs(p['dc'] - ref) <= rule(1, ref),
              pc=s['pc'], off=s['off'], gspan=s.get('gspan'), gsrc=s.get('gsrc'), gdis=s.get('gdis'), pp=s.get('pp'))
        if v != 0.0:
            R.enter('C', 1)  # back in C after the READ? (the gain kept)


def watch(R, secs):
    """Poll DBG:ACDC:STAT? for `secs`: ranges seen, readings / s, AC part
    mean / max, AC sources, DC part mean."""
    t0 = time.time()
    s0 = R.stat()
    rs, ac, dc, src = set(), [], [], {}
    while time.time() - t0 < secs:
        s = R.stat()
        rs.add(s['range'])
        ac.append(s['ac'])
        dc.append(s['dc'])
        src[s.get('acsrc', -1)] = src.get(s.get('acsrc', -1), 0) + 1
        time.sleep(0.1)
    s1 = R.stat()
    return dict(ranges=sorted(rs), rate=(s1['n_rd'] - s0['n_rd']) / max(1e-3, time.time() - t0),
                ac=sum(ac) / len(ac), ac_max=max(ac), dc=sum(dc) / len(dc), acsrc=src, gspan=s1.get('gspan', 0))


def e9(R):
    for v in (4.0, 4.5, 5.0, 5.5):
        R.src_dc(v)
        R.w('CONF:VOLT:DC')
        time.sleep(1.0)
        ref = sum(float(R.q('READ?')) for _ in range(3)) / 3
        R.enter('C')
        w = watch(R, 12.0)
        err = w['dc'] - ref
        ok = len(w['ranges']) == 1 and w['rate'] >= 6 and w['ac_max'] < 0.03 and abs(err) <= rule(1 if w['ranges'][0] == 0 else 2, ref)
        R.log(exp='e9', level=v, ref=ref, err=err, ok=ok, **w)


def e10(R):
    refs = {}
    for v in (5.0, 5.5, 4.5):
        R.src_dc(v)
        R.w('CONF:VOLT:DC')
        time.sleep(1.0)
        refs[v] = sum(float(R.q('READ?')) for _ in range(3)) / 3
    R.src_sq(40)
    R.w('DBG:ACDC:METH C')
    R.w('CONF:VOLT:DC')
    time.sleep(0.5)
    R.w('VOLT:COUP AUTO')
    time.sleep(12.0)
    R.log(exp='e10', level='square', coup=R.q('VOLT:COUP?'), fetch=R.fetch(), **watch(R, 2.0))
    for v in (5.0, 5.5, 4.5):
        if R.cal:
            R.cal.set_output('voltage', v, enable=True)
        time.sleep(7.0)
        w = watch(R, 8.0)
        coup, f = R.q('VOLT:COUP?'), R.fetch()
        ok = coup == 'AUTO,DC' and abs(f - refs[v]) <= rule(1, refs[v]) and len(w['ranges']) == 1
        R.log(exp='e10', level=v, ref=refs[v], coup=coup, fetch=f, err=f - refs[v], ok=ok, **w)


def e11(R, entries=3, secs=15.0):
    """AUTO entered on the 40 Hz square (AC ~= DC, a tie), A and C, `entries`
    times each: VOLT:COUP? every ~0.3 s for `secs`. Pass (dmm.rs AUTO_TIE):
    DC from the first AUTO reading, then at most one change (the decision),
    and the same part in every entry of a method."""
    R.src_sq(40)
    for meth in ('A', 'C'):
        parts = []
        for k in range(entries):
            R.w('DBG:ACDC:METH %s' % meth)
            R.w('CONF:VOLT:DC')
            time.sleep(1.0)
            R.w('VOLT:COUP AUTO')
            t0, seq = time.time(), []
            while time.time() - t0 < secs:
                c = R.q('VOLT:COUP?')
                if c.startswith('AUTO,') and not math.isnan(R.fetch()):
                    seq.append(c[5:])
                time.sleep(0.3)
            flips = sum(1 for a, b in zip(seq, seq[1:]) if a != b)
            ok = bool(seq) and (flips == 0 or (flips == 1 and seq[0] == 'DC'))
            parts.append(seq[-1] if seq else '-')
            R.log(exp='e11', meth=meth, entry=k, part=parts[-1], flips=flips, first=seq[0] if seq else '-',
                  seq=''.join(x[0] for x in seq), ok=ok)
        R.log(exp='e11', meth=meth, parts=parts, ok=len(set(parts)) == 1)
    R.w('DBG:ACDC:METH A')


def main():
    ap = argparse.ArgumentParser(description=__doc__.split('\n')[0], formatter_class=argparse.RawDescriptionHelpFormatter, epilog=__doc__)
    ap.add_argument('exps', nargs='+', help='e1 e2 e3 e4 e5 e6 e7 e8 e8s e8l e8up e8upl tc6 e9 e10 e11')
    ap.add_argument('--usbip', metavar='BUSID', help="the emulator's USB/IP server")
    ap.add_argument('--cal', default='/dev/ttyACM1')
    ap.add_argument('--no-cal', action='store_true')
    ap.add_argument('--settle', type=float, default=2.0, help='s after a calibrator step')
    ap.add_argument('--repeats', type=int, default=5, help='e1: B entries per level')
    ap.add_argument('--minutes', type=float, default=20.0, help='e5: per level')
    ap.add_argument('--period', type=float, default=60.0, help='e5: s between C entries')
    ap.add_argument('--trace', action='store_true', help='e8*: log STAT? at ~10 Hz per level')
    ap.add_argument('-o', '--out', default='acdcmeth.jsonl')
    a = ap.parse_args()
    R = Run(a)
    try:
        for e in a.exps:
            if e in ('e3', 'e4'):
                print('%s: needs a signal generator (the calibrator has DC and a 40 Hz square only; '
                      '40 Hz is an AD1 notch): skipped' % e, flush=True)
                continue
            {'e1': e1, 'e2': e2, 'e5': e5, 'e6': e6, 'e7': e7, 'e8': e8, 'e8up': lambda R: e8(R, 'up', 4.0),
             'e8s': lambda R: e8(R, 'down', 4.0), 'e8l': lambda R: e8(R, 'down', 10.0),
             'e8upl': lambda R: e8(R, 'up', 10.0), 'tc6': tc6, 'e9': e9, 'e10': e10, 'e11': e11}[e](R)
    finally:
        R.close()


if __name__ == '__main__':
    main()
