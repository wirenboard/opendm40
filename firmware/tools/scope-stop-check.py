#!/usr/bin/env python3
"""Device check of the stopped scope record: the built-in generator's sine
into CH1 (a BNC lead from the generator output to CH1), per timebase RUN,
wait, STOP, wait for TRIG:STAT? STOP, read the whole record (WAV:POIN MAX,
WORD) and check it:

  * length: the STOP's capture (scope.rs `rec_split`, `stop_req`): the
    screen + 0.5 s, at most 42 144 .. 43 008 samples (37 500 at 100 ms/div,
    untriggered up to ~50 000); STOP done within the screen's time + 1 s;
  * the trigger point (WAV:PRE? x origin) inside the record, on the level
    (the generator's offset: the sine's mid code) and rising;
  * no seam: the record folded on its fitted period, each sample against
    the median of its phase bin (max residual < 15 % of the amplitude) and
    no step larger than the sine's own largest step plus noise.

  python3 tools/scope-stop-check.py OUTDIR [TB,TB,...] [REPS]
      default timebases 0.001,0.005,0.05,0.1, 3 STOPs each

Each record is saved as OUTDIR/tb<tb>_<k>.npy and a screenshot as .png
(tools/dm40dbg.py shot). The generator output is switched off at the end
and on any failure. Needs numpy and read/write access to /dev/usbtmc*.
Exit status 0 = all passed.
"""
import glob
import os
import struct
import subprocess
import sys
import time

import numpy as np

HERE = os.path.dirname(os.path.abspath(__file__))


class Dev:
    def __init__(self):
        self.path = sorted(glob.glob('/dev/usbtmc*'))[-1]
        self.fd = os.open(self.path, os.O_RDWR)

    def close(self):
        os.close(self.fd)

    def w(self, cmd):
        os.write(self.fd, (cmd + '\n').encode())

    def rd(self):
        buf = b''
        while True:
            b = os.read(self.fd, 65536)
            buf += b
            if buf.startswith(b'#'):
                nd = int(buf[1:2])
                n = int(buf[2:2 + nd]) if nd else 0
                if len(buf) >= 2 + nd + n:
                    return buf
            elif buf.endswith(b'\n') or not b:
                return buf

    def q(self, cmd):
        self.w(cmd)
        return self.rd().decode(errors='replace').strip()

    def block(self, cmd):
        self.w(cmd)
        r = self.rd()
        nd = int(r[1:2])
        n = int(r[2:2 + nd]) if nd else 0
        return r[2 + nd:2 + nd + n]


def expected(fs):
    # The STOP's capture (scope.rs rec_split, stop_req): the screen (25 000
    # samples) + 0.5 s, at most the raw ring less the margins (42 144);
    # at the deadline (untriggered) what is in, up to the ring.
    lo = min(42144, 25000 + int(fs) // 2 - 500)
    return (lo, 65520 if fs <= 25000 else 43008)


def check_record(y, pre, fs):
    """-> (ok, text) for one record of codes y and its preamble fields."""
    n = len(y)
    nmin, nmax = expected(fs)
    xinc, xorig = float(pre[4]), float(pre[5])
    i0 = round(-xorig / xinc) if xinc else -1
    if n < 1000:
        return False, 'n %d (expected %d..%d)' % (n, nmin, nmax)
    # Period from the FFT peak refined by a least-squares sine fit.
    t = np.arange(n)
    Y = np.abs(np.fft.rfft(y - y.mean()))
    kpk = int(np.argmax(Y[1:]) + 1)
    best = None
    for c in np.linspace(kpk - 1, kpk + 1, 201):
        wv = 2 * np.pi * c / n
        A = np.c_[np.sin(wv * t), np.cos(wv * t), np.ones(n)]
        coef = np.linalg.lstsq(A, y, rcond=None)[0]
        e = np.sqrt(np.mean((y - A @ coef) ** 2))
        if best is None or e < best[0]:
            best = (e, c, coef)
    _, c, coef = best
    amp = np.hypot(coef[0], coef[1])
    mid = coef[2]
    # Periodic template: median per phase bin; a seam is a local departure.
    ph = ((t * c / n) % 1.0 * 1024).astype(int)
    tmpl = np.array([np.median(y[ph == b]) if np.any(ph == b) else np.nan for b in range(1024)])
    r = np.nan_to_num(y - tmpl[ph])
    rmax = float(np.max(np.abs(r)))
    step = float(np.max(np.abs(np.diff(y))))
    sstep = amp * 2 * np.pi * c / n
    len_ok = nmin <= n <= nmax
    # The trigger point on the fitted sine (the samples' noise is larger
    # than a slow sine's step): at the mid level, rising.
    wv = 2 * np.pi * c / n
    fit = coef[0] * np.sin(wv * i0) + coef[1] * np.cos(wv * i0)
    slope = coef[0] * np.cos(wv * i0) - coef[1] * np.sin(wv * i0)
    trig_ok = 2 <= i0 < n - 2 and abs(fit) < 0.15 * amp and slope > 0
    seam_ok = rmax < 0.15 * amp and step < sstep + 0.1 * amp + 4
    txt = ('n %d (%d..%d) %s, trigger %d %s, amp %.0f codes, max resid %.1f, max step %.0f (sine %.1f) %s'
           % (n, nmin, nmax, 'ok' if len_ok else 'BAD', i0, 'ok' if trig_ok else 'BAD', amp, rmax, step, sstep,
              'no seam' if seam_ok else 'SEAM'))
    return len_ok and trig_ok and seam_ok, txt


def main():
    out = sys.argv[1]
    os.makedirs(out, exist_ok=True)
    tbs = [float(x) for x in (sys.argv[2] if len(sys.argv) > 2 else '0.001,0.005,0.05,0.1').split(',')]
    reps = int(sys.argv[3]) if len(sys.argv) > 3 else 3
    d = Dev()
    d.w('SYST:BEEP:VOL 0')
    d.w('SYST:ALER:ALARm OFF')
    fails = 0
    try:
        print(d.q('*IDN?'), flush=True)
        for c in ('SOUR:FUNC SIN', 'SOUR:VOLT 3', 'SOUR:VOLT:OFFS 1.5', 'CHAN1:COUP DC', 'CHAN1:SCAL 1',
                  'TRIG:SWE AUTO', 'TRIG:SLOP POS', 'TRIG:LEV 1.5'):
            d.w(c)
        for tb in tbs:
            d.w('TIM:SCAL %g' % tb)
            time.sleep(1)
            fs = float(d.q('ACQ:SRAT?'))
            # ~20 periods over a 65 520-sample record at this rate.
            d.w('SOUR:FREQ %d' % max(1, round(fs * 20 / 65520)))
            d.w('OUTP ON')
            for k in range(reps):
                d.w('RUN')
                time.sleep(min(2 * 65520 / fs + 1.5, 15) + 0.3 * k)
                d.w('STOP')
                t0 = time.time()
                st = ''
                while time.time() - t0 < 20:
                    st = d.q('TRIG:STAT?')
                    if st == 'STOP':
                        break
                    time.sleep(0.1)
                dt = time.time() - t0
                d.close()
                subprocess.run([sys.executable, os.path.join(HERE, 'dm40dbg.py'), 'shot', '%s/tb%g_%d.png' % (out, tb, k)],
                               stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
                d = Dev()
                d.w('WAV:POIN MAX')
                d.w('WAV:FORM WORD')
                pre = d.q('WAV:PRE?').split(',')
                b = d.block('WAV:DATA?')
                y = np.array(struct.unpack('<%dH' % (len(b) // 2), b), dtype=float)
                np.save('%s/tb%g_%d.npy' % (out, tb, k), y)
                ok, txt = check_record(y, pre, fs) if st == 'STOP' else (False, 'no STOP (%s)' % st)
                # STOP -> TRIG:STAT? STOP within the screen's time + 1 s
                # (+ 0.5 s for the polling and USB).
                wait_ok = dt <= 10 * tb + 1.5
                ok = ok and wait_ok
                txt += ', wait %s' % ('ok' if wait_ok else 'TOO LONG')
                fails += not ok
                print('tb %g rep %d: fs %g, STOP after %.1f s, %s %s' % (tb, k, fs, dt, txt, 'OK' if ok else 'FAIL'),
                      flush=True)
    finally:
        d.w('OUTP OFF')
        d.w('RUN')
        d.w('SYST:BEEP:VOL 0')
        d.w('SYST:ALER:ALARm OFF')
    print('fails', fails)
    sys.exit(1 if fails else 0)


if __name__ == '__main__':
    main()
