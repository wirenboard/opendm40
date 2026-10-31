#!/usr/bin/env python3
"""opendm40 Record page "On change" (firmware/src/csvlog.rs on-change mode,
LOG:CSV:INTerval CHANge) in the emulator: a stepped DC input gives rows only
at the steps and as the 60 s heartbeat.

  cd firmware && ./build.sh
  emu/onchangetest.py --app firmware/target/native/thumbv7em-none-eabihf/release/opendm40

Input on V-COM (virtual time): 1 V, 2 V from T1, 2.01 V from T2 (+0.17 % of
the 6 V range: below the 1 % threshold), 3 V from T3, held to the end
(> T3 + 60 s). DC V AUTO (6 V range), LOG:CSV:INT CHAN, LOG:CSV:CHAN 1,
LOG:CSV ON before T1, OFF after T3 + 70 s; the CSV is read back with
MMEM:DATA?. Checks:
  * INT? / CHAN? replies, the header line "# event: change > 1 % ...";
  * every row is the first one, within 2 s after the 1 -> 2 V or 2 -> 3 V
    step, or a heartbeat 60..60.6 s after the row before it;
  * a row at each step, none for the 2.01 V step, at least one heartbeat;
  * the values: 1, 2 or 3 V within 5 mV once settled (a row in a step
    window may be a settling conversion).
Exit status 0 = PASS.
"""
import argparse
import os
import subprocess
import sys
import time

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
ap.add_argument('--app', required=True, help='opendm40 native ELF / image')
ap.add_argument('--port', type=int, default=None, help='USB/IP port (default: a free one)')
A = ap.parse_args()
sys.path.insert(0, HERE)
import run  # noqa: E402
A.port = A.port or run.free_port()
os.environ['USBIP_PORT'] = str(A.port)
import usbip_client as uc  # noqa: E402

T1, T2, T3 = 25, 35, 45  # virtual s
TEND = T3 + 72
FAIL = []


def check(ok, what):
    print('  %s %s' % ('ok  ' if ok else 'FAIL', what), flush=True)
    if not ok:
        FAIL.append(what)


OUT = os.path.join(HERE, 'out')
os.makedirs(OUT, exist_ok=True)
name = 'onchange-%d-%d' % (A.port, os.getpid())
log = open(os.path.join(OUT, name + '.out'), 'w')
emu = subprocess.Popen([sys.executable, os.path.join(HERE, 'run.py'), 'fwrs', '--app', A.app, '--usbip', '0.01',
                        '--usbip-port', str(A.port), '--uid', run.emu_uid(A.port).hex().upper(), '--name', name,
                        '--dmm', 'vsrc:1.0', '--dmm', 'vsrc:2.0@%d' % T1, '--dmm', 'vsrc:2.01@%d' % T2,
                        '--dmm', 'vsrc:3.0@%d' % T3, '--run-for', '3000'],
                       cwd=ROOT, stdout=log, stderr=subprocess.STDOUT, start_new_session=True)
try:
    t = None
    for _ in range(150):
        try:
            t = uc.Usbtmc(uc.Dev('1-0'))
            break
        except Exception:
            time.sleep(2)
    if t is None:
        raise RuntimeError('no USB/IP device on port %d' % A.port)

    def q(line, raw=False, timeout=120):
        t.write(line)
        if '?' not in line:
            return None
        b = t.read(maxlen=8192, timeout=timeout)
        return b if raw else b.decode(errors='replace').strip()

    def up():
        return int(q('DBG:PING?').split(',')[0])

    def wait_until(ms):
        while up() < ms:
            time.sleep(0.5)

    def block(b):
        nd = int(b[1:2])
        return b[2 + nd:2 + nd + int(b[2:2 + nd])]

    print('*IDN?', q('*IDN?'), flush=True)
    q('CONF:VOLT:DC')
    q('LOG:CSV:INT CHAN')
    q('LOG:CSV:CHAN 1')
    check(q('LOG:CSV:INT?') == 'CHAN' and q('LOG:CSV:CHAN?') == '1', 'LOG:CSV:INT? CHAN, LOG:CSV:CHAN? 1')
    wait_until(8000)
    q('LOG:CSV ON')
    e = q('SYST:ERR?')
    check(e.startswith('0,') or e.startswith('+0,'), 'LOG:CSV ON: %s' % e)
    t_last = 0
    while True:
        u = up()
        if u >= TEND * 1000:
            break
        if u - t_last >= 10000:
            print('  uptime %.0f s' % (u / 1000), flush=True)
            t_last = u
        time.sleep(1)
    q('LOG:CSV OFF')
    st = q('LOG:CSV?')
    for _ in range(120):
        st = q('LOG:CSV?')
        if st.split(',')[0] in ('3', '4', '5', '6'):
            break
        time.sleep(0.5)
    print('  LOG:CSV?', st)
    f = st.split(',')
    check(f[0] == '3' and int(f[1]) >= 0, 'recording done (%s)' % st)
    fname = 'LOGS/LOG%03d.CSV' % int(f[1])
    data = bytearray()
    while True:
        d = block(q('MMEM:DATA? "%s",%d,6000' % (fname, len(data)), raw=True))
        if not d:
            break
        data += d
    csv = data.decode()
    open(os.path.join(OUT, name + '.csv'), 'w').write(csv)
    lines = csv.split('\r\n')
    print('\n'.join('    ' + x for x in lines[:4]))
    check(lines[2].startswith("# event: change > 1 % of the range's full scale"), 'header: %s' % lines[2])
    t0 = float(lines[0].split('started uptime ')[1].split(' ')[0])
    rows = []
    for x in lines[4:]:
        if x:
            c = x.split(',')
            rows.append((t0 + float(c[0]), float(c[1]) if c[1] else None, c[3]))
    for r in rows:
        print('    t=%7.3f %s %s' % r)
    # The scheduled input times are emulator time, the rows uptime: the
    # offset between them (boot) is taken from the first row past T1 - 5 s
    # (the 2 V step), within 5 s.
    off = next((r[0] - T1 for r in rows if r[0] > T1 - 5), 99)
    print('  uptime - input schedule: %.3f s' % off)
    check(abs(off) < 5, 'the 2 V step row %.3f s after its scheduled time' % off)
    T1, T2, T3 = T1 + off, T2 + off, T3 + off
    kinds = []
    for i, (tt, v, fl) in enumerate(rows):
        if i == 0:
            k = 'first'
        elif T1 <= tt <= T1 + 2 or T3 - 0.5 <= tt <= T3 + 2:
            k = 'step'
        elif 60.0 <= tt - rows[i - 1][0] <= 60.6:
            k = 'heartbeat'
        else:
            k = None
        kinds.append(k)
        check(k is not None, 'row at %.3f s (%s V): %s' % (tt, v, k or 'not at a step, not a heartbeat'))
    check(any(T3 - 0.5 <= r[0] <= T3 + 2 for r in rows), 'a row at the 3 V step')
    check(not any(T2 - 0.5 <= r[0] <= T2 + 3 for r in rows), 'no row for the 2.01 V step (below 1 %)')
    check(kinds.count('heartbeat') >= 1, 'heartbeats: %d' % kinds.count('heartbeat'))
    want = lambda tt: 1.0 if tt < T1 - 0.5 else 2.0 if tt < T2 - 0.5 else 2.01 if tt < T3 - 0.5 else 3.0  # noqa: E731
    for tt, v, fl, k in [r + (k,) for r, k in zip(rows, kinds)]:
        if k != 'step':
            check(v is not None and abs(v - want(tt)) < 0.005, 'value at %.3f s: %s V (want %g)' % (tt, v, want(tt)))
except Exception as e:
    check(False, 'run: %r' % e)
finally:
    try:
        os.killpg(emu.pid, 15)
    except Exception:
        pass
print('PASS' if not FAIL else 'FAIL: %d' % len(FAIL))
sys.exit(1 if FAIL else 0)
