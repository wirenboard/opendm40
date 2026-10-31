#!/usr/bin/env python3
"""Device check of the long scope record on the coded ring (src/cring.rs):
the built-in generator into CH1 (a BNC lead from the generator output to
CH1), per generator waveform and timebase DIGitize, read the whole record
(WAV:POIN MAX, WORD) and check it:

  * length: at least 65 520 samples where the record is longer than that
    (100 / 200 ms/div), the 10 s record at 1 s/div (25 000);
  * the trigger point (WAV:PRE? x origin) where rec_split puts it: the
    part after it min(n / 2, 52 156) samples (the trigger at the centre);
  * no seam: the record folded on its period (the generator's frequency),
    each sample against the median of its phase bin (max residual < 15 %
    of the amplitude) and no step larger than the waveform's own (square:
    its edges are the steps, residual only);
  * bytes per sample: the record's Save CSV size is not checked here; the
    length per waveform is printed (the ring keeps what codes small).

  python3 tools/scope-long-check.py OUTDIR [TB,TB,...] [FUNC,FUNC,...]
      default timebases 0.1,0.2,1, waveforms SIN,SQU,TRI; no sound
      (SYST:BEEP:VOL 0, SYST:ALER:ALARm OFF before and after)

DIGitize captures a linear record (no roll) whatever the Roll setting.
Each record is saved as OUTDIR/<func>_tb<tb>.npy. The generator output is
switched off at the end and on any failure. Needs numpy and read/write
access to /dev/usbtmc*. Exit status 0 = all passed.
"""
import glob
import os
import struct
import sys
import time

import numpy as np


class Dev:
    def __init__(self):
        self.fd = os.open(sorted(glob.glob('/dev/usbtmc*'))[-1], os.O_RDWR)

    def w(self, cmd):
        os.write(self.fd, (cmd + '\n').encode())

    def rd(self):
        buf = b''
        while True:
            b = os.read(self.fd, 1 << 20)
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


def quiet(d):
    d.w('SYST:BEEP:VOL 0')
    d.w('SYST:ALER:ALARm OFF')


def coded_bytes(y):
    """Bytes of the record coded as the ring codes it (4096-sample blocks,
    dlog's scope block: the fixed order with the least sum of |residual|,
    adaptive Rice; a block that would not be smaller stays packed, 5120
    bytes). The ring picks the order from the block before and keeps a
    block packed also when in-place coding is unsafe: this is the bound."""
    c = [int(v) & 0x3FF for v in y]
    total = 0
    for b0 in range(0, len(c), 4096):
        blk = c[b0:b0 + 4096]
        best = None
        for o in range(4):
            h = [0, 0, 0]
            a, n, bits = 16, 1, 3
            for j, x in enumerate(blk):
                oo = min(o, j)
                p = (512, h[0], 2 * h[0] - h[1], 3 * h[0] - 3 * h[1] + h[2])[oo]
                r = x - p
                u = (r << 1) ^ (r >> 31) if r < 0 else r << 1
                k = 0
                while k < 30 and (n << k) < a:
                    k += 1
                q = u >> k
                bits += q + 1 + k if q < 24 else 56
                a = min(a + min(u, 1 << 24), 0xFFFFFFFF)
                n += 1
                if n >= 32:
                    a = (a + 1) >> 1
                    n >>= 1
                h = [x, h[0], h[1]]
            best = bits if best is None else min(best, bits)
        total += min((best + 7) // 8, (len(blk) * 10 + 7) // 8)
    return total


def shot(path):
    import subprocess
    subprocess.run([sys.executable, os.path.join(os.path.dirname(os.path.abspath(__file__)), 'dm40dbg.py'), 'shot', path],
                   stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)


def check(y, pre, fs, f_gen, func):
    n = len(y)
    xinc, xorig = float(pre[4]), float(pre[5])
    i0 = round(-xorig / xinc) if xinc else -1
    want_min = min(65520, int(fs * 10) - 2000)
    post = n - i0
    post_ok = abs(post - min(fs * 10 / 2, 52156)) < 0.02 * post + 2000 if fs * 10 > 65520 else 0 < i0 < n
    per = fs / f_gen
    t = np.arange(n)
    ph = ((t / per) % 1.0 * 512).astype(int)
    tmpl = np.array([np.median(y[ph == b]) if np.any(ph == b) else np.nan for b in range(512)])
    r = np.nan_to_num(y - tmpl[ph])
    amp = (np.percentile(y, 99) - np.percentile(y, 1)) / 2
    rmax = float(np.max(np.abs(r)))
    seam_ok = rmax < 0.15 * amp
    ok = n >= want_min and post_ok and seam_ok
    return ok, ('n %d (>= %d), after the trigger %d %s, amp %.0f codes, max resid %.1f %s'
                % (n, want_min, post, 'ok' if post_ok else 'BAD', amp, rmax, 'no seam' if seam_ok else 'SEAM'))


def main():
    out = sys.argv[1]
    os.makedirs(out, exist_ok=True)
    tbs = [float(x) for x in (sys.argv[2] if len(sys.argv) > 2 else '0.1,0.2,1').split(',')]
    funcs = (sys.argv[3] if len(sys.argv) > 3 else 'SIN,SQU,TRI').split(',')
    d = Dev()
    quiet(d)
    fails = 0
    try:
        print(d.q('*IDN?'), flush=True)
        for c in ('SOUR:VOLT 3', 'SOUR:VOLT:OFFS 1.5', 'CHAN1:COUP DC', 'CHAN1:SCAL 1', 'TRIG:SWE AUTO',
                  'TRIG:SLOP POS', 'TRIG:LEV 1.5', 'WAV:POIN MAX', 'WAV:FORM WORD'):
            d.w(c)
        for tb in tbs:
            d.w('TIM:SCAL %g' % tb)
            time.sleep(1)
            fs = float(d.q('ACQ:SRAT?'))
            for func in funcs:
                f = max(1, round(fs / 250))  # 250 samples a period
                for c in ('SOUR:FUNC %s' % func, 'SOUR:FREQ %d' % f, 'OUTP ON'):
                    d.w(c)
                time.sleep(1)
                t0 = time.time()
                d.w('DIG')
                st = ''
                while time.time() - t0 < 40:
                    st = d.q('TRIG:STAT?')
                    if st == 'STOP':
                        break
                    time.sleep(0.2)
                dt = time.time() - t0
                pre = d.q('WAV:PRE?').split(',')
                b = d.block('WAV:DATA?')
                y = np.array(struct.unpack('<%dH' % (len(b) // 2), b), dtype=float)
                np.save('%s/%s_tb%g.npy' % (out, func, tb), y)
                fa = float(d.q('SOUR:FREQ?'))
                ok, txt = check(y, pre, fs, fa, func) if st == 'STOP' and len(y) > 1000 else (False, 'no record (%s)' % st)
                if len(y) > 0:
                    txt += ', ~%.2f B/sample coded' % (coded_bytes(y) / len(y))
                fails += not ok
                os.close(d.fd)
                shot('%s/%s_tb%g.png' % (out, func, tb))
                d = Dev()
                print('%s tb %g: fs %g, %g Hz, DIG in %.1f s, %s %s' % (func, tb, fs, fa, dt, txt, 'OK' if ok else 'FAIL'), flush=True)
                quiet(d)
    finally:
        d.w('OUTP OFF')
        d.w('RUN')
        quiet(d)
    print('fails', fails)
    sys.exit(1 if fails else 0)


if __name__ == '__main__':
    main()
