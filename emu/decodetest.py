#!/usr/bin/env python3
"""opendm40 protocol decoder band on the scope page (src/decode.rs,
src/app_decode.rs, src/ui_decode.rs; DECode:...) in the emulator.

  cd firmware && ./build.sh
  emu/decodetest.py --app firmware/target/native/thumbv7em-none-eabihf/release/opendm40 [--shots DIR]

Signals: bit patterns from watched files the test rewrites while the
emulator runs: the scope input (peripherals/ScopeFeeder.cs SetBitsFile)
and the V-COM input of the HY3131 model for the Wave view (hy3131/HY3131.cs
SetBitsFile: the AD2 samples through a sinc^2-like filter, INFERRED).

Cases (DECode:FRAMes? checked against the bytes sent; the screenshot's
band against FRAMes? of the screen: one box per byte / frame at the time
each starts, red boxes for errors):
  * MCX UART 9600 8N1 (auto-baud): Auto untriggered running (no decode),
    STOP (decoded); Auto triggered at 1 ms/div running (a decode per record) and
    stopped, 200 us/div (two characters on the screen), 5 ms/div (the
    bar), 100 ms/div (roll; STOP: the sample rate too low, the note);
  * a framing error (a byte with a low stop bit) drawn red;
  * MCX Modbus RTU 19200 8E1: request / reply / exception / bad CRC, the
    pairing, at 2 and 5 ms/div, stopped;
  * MCX DALI (DECode:PROTocol DALI, feature dec-dali; skipped without
    it): forward, backward, 24-bit and a bad-Manchester frame at 10 ms/div,
    stopped; FRAMes? texts, DECode:RATE? 1200, the band's boxes at 2 ms/div;
  * the Wave view (DC V, 5 V TTL on V-COM): UART at 2400 and 4800 baud,
    Modbus at 2400 8E1, at 2 ms/div (38.4 kS/s), decoded on STOP;
    19200 baud there: the note;
  * cost: DBG:PROF? (dec.job = one record's decode, x2 = its slices,
    s.frame = the frame) at 1, 10 and 100 ms/div with 9600 and 115200 baud
    traffic, the decoder off and on, running, STOP and a pan; a table;

  * region C (emu/regionc.py, gated on hy3131::HANDED: the Wave view's
    AD2 stream): no .cold code while the stream ISR owns the bus.
--mips: CPU speed (default 110 = the device's speed per instruction).
Exit status 0 = all passed.
"""
import argparse
import os
import subprocess
import sys
import time

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
sys.path.insert(0, HERE)
import run as emurun  # noqa: E402

ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
ap.add_argument('--app', required=True, help='opendm40 native ELF / image')
ap.add_argument('--port', type=int, default=0, help='USB/IP port (default: a free one)')
ap.add_argument('--mips', default='110')
ap.add_argument('--shots', help='save screenshots here')
ap.add_argument('--no-region-c', action='store_true', help='without the region C hooks')
ap.add_argument('--no-wave', action='store_true', help='skip the Wave view cases')
ap.add_argument('--only', default='mcx,cost,modbus,dali,wave,log', help='the sections to run (default: all)')
A = ap.parse_args()
if not A.port:
    A.port = emurun.free_port()
os.environ['USBIP_PORT'] = str(A.port)
import usbip_client as uc  # noqa: E402
import regionc  # noqa: E402

FAIL = []
# Region C functions called while the log streamed that are not its own
# (c_check_log).
LOG_OTHER = set()
# The frame log's code that is region C on purpose (link-native.x
# `.text.log_cold`): the waveform view, the counters, the page draw, the
# setup popup, the view's touches and its panel buttons; the per-pass
# decode and rows are not.
LOG_COLD = ('log_snip_show', 'log_stats', 'log_page', 'log_setup_draw', 'log_gesture', 'log_setup_changed',
            'log_unpress', 'log_page_step', 'log_pause', 'log_snip_hide', 'log::panel_button', 'log::panel_hit')


def want(s):
    return s in A.only.split(',')


COST = []



def check(ok, what):
    print('  %s %s' % ('ok  ' if ok else 'FAIL', what), flush=True)
    if not ok:
        FAIL.append(what)


OUT = os.path.join(HERE, 'out')
os.makedirs(OUT, exist_ok=True)
TAG = 'decode-%d-%d' % (A.port, os.getpid())
BITS = os.path.join(OUT, TAG + '.bits')
DBITS = os.path.join(OUT, TAG + '.dbits')


def uart_bits(data, fmt='8N1', err=None):
    nb = 7 if fmt[0] == '7' else 8
    s = ''
    for k, b in enumerate(data):
        s += '0'
        ones = 0
        for i in range(nb):
            bit = b >> i & 1
            ones += bit
            s += str(bit)
        if fmt[1] == 'E':
            s += str(ones & 1)
        elif fmt[1] == 'O':
            s += str(1 - (ones & 1))
        s += '0' if err == 'all' or k == err else '1'
    return s


def dali_bits(v, n, bad=None):
    """A DALI frame as half bits (send at 2400 'baud': Te = 416.67 us): a
    start bit, n bits of v MSB first, a 1 low then high; `bad`: that bit
    without its centre transition."""
    s = '01'
    for i in range(n):
        b = v >> (n - 1 - i) & 1
        s += '00' if i == bad else ('01' if b else '10')
    return s


def crc16(d):
    c = 0xFFFF
    for b in d:
        c ^= b
        for _ in range(8):
            c = (c >> 1) ^ 0xA001 if c & 1 else c >> 1
    return c


def mb(d):
    c = crc16(d)
    return bytes(d) + bytes([c & 0xFF, c >> 8])


def put(path, bits, baud, vlo, vhi, idle):
    """The pattern, then `idle` s of idle line, repeated."""
    tmp = path + '.tmp'
    repeat = len(bits) / baud + idle if idle else 0
    with open(tmp, 'w') as f:
        f.write('%.12g %g %g %g\n' % (1.0 / baud, vlo, vhi, repeat))
        f.write(bits)
    os.replace(tmp, path)


put(BITS, '1' * 10, 9600, 0.0, 3.3, 0)
put(DBITS, '1' * 10, 2400, 0.0, 5.0, 0)
# Gated on the Wave view's AD2 stream and on the frame log's stream.
RC = None if A.no_region_c else regionc.Hooks(A.app, OUT, TAG, gate=('opendm40::hy3131::HANDED', 'opendm40::scope::LOG_LIVE'))
SCRIPT = os.path.join(OUT, TAG + '.script')
with open(SCRIPT, 'w') as f:
    f.write('0.2 monitor sysbus.scopefeed SetBitsFile "%s"\n' % BITS)
    f.write('0.2 monitor sysbus.hy3131 SetBitsFile "%s"\n' % DBITS)
    if RC and RC.script:
        f.write(RC.lines())
if RC and not RC.script:
    print('(no llvm-nm / symbols: the region C checks are skipped)')
    RC = None
cmd = [sys.executable, os.path.join(HERE, 'run.py'), 'fwrs', '--mips', A.mips, '--app', A.app,
       '--usbip', '0.01', '--usbip-port', str(A.port), '--run-for', '3000', '--name', TAG,
       '--log', os.path.join(OUT, TAG + '.log'), '--script', SCRIPT, '--dmm', 'vsrc:5']
emu = subprocess.Popen(cmd, cwd=ROOT, stdout=subprocess.DEVNULL, stderr=subprocess.STDOUT, start_new_session=True)


def block(b):
    assert b[:1] == b'#', b[:16]
    d = int(b[1:2])
    n = int(b[2:2 + d]) if d else 0
    return b[2 + d:2 + d + n]


# Band geometry (ui_decode.rs) and colours (RGB565 -> the screenshot's RGB).
BYTE_Y, FRAME_Y = 259, 241


def rgb(c):
    return ((c >> 11) * 255 // 31, ((c >> 5) & 63) * 255 // 63, (c & 31) * 255 // 31)


C_EDGE, C_FEDGE, C_ERR = rgb(0xad55), rgb(0x059e), rgb(0xf800)


def near(p, c, tol=24):
    return all(abs(a - b) <= tol for a, b in zip(p, c))


try:
    t = None
    for _ in range(150):
        try:
            t = uc.Usbtmc(uc.Dev('1-0'))
            break
        except Exception:
            time.sleep(2)
    if t is None:
        sys.exit('decodetest: no USB/IP device on port %d' % A.port)

    def q(line, raw=False, timeout=60):
        t.write(line)
        if '?' not in line:
            return None
        b = t.read(maxlen=8192, timeout=timeout)
        return b if raw else b.decode(errors='replace').strip()

    def up():
        return int(q('DBG:PING?').split(',')[0])

    def wait_ms(ms):
        t0 = up()
        while up() - t0 < ms:
            time.sleep(0.2)

    def err():
        return q('SYST:ERR?')

    def frames(first=0):
        out = []
        for _ in range(40):
            d = block(q('DEC:FRAM? %d' % first, raw=True)).decode()
            if not d:
                break
            ls = [l.split(',') for l in d.splitlines()]
            out += ls
            first = int(ls[-1][0]) + 1
        return out

    def fifo():
        # The frame log: DECode:FRAMes? reads the unread frames.
        out = []
        for _ in range(40):
            d = block(q('DEC:FRAM?', raw=True)).decode()
            if not d:
                break
            out += [l.split(',') for l in d.splitlines()]
        return out

    def stop():
        q('STOP')
        for _ in range(100):
            if q('TRIG:STAT?') == 'STOP':
                return True
            wait_ms(100)
        return False

    def run():
        q('RUN')
        wait_ms(1200)

    shot_n = [0]

    def shot(name):
        """The screen as rows of RGB tuples (saved as PNG with --shots)."""
        rows = 4
        px = bytearray()
        for y in range(0, 320, rows):
            n = 480 * rows * 3 // 2 + 1
            b = q('DBG:LCD? 0,%d,480,%d,%d' % (y, rows, n), raw=True)
            nd = int(b[1:2])
            d = b[2 + nd:2 + nd + int(b[2:2 + nd])]
            w = [d[i] | d[i + 1] << 8 for i in range(2, len(d), 2)]
            bs = bytearray()
            for v in w:
                bs += bytes([v >> 8, v & 0xFF])
            px += bs[:480 * rows * 3]
        if A.shots:
            from PIL import Image
            os.makedirs(A.shots, exist_ok=True)
            Image.frombytes('RGB', (480, 320), bytes(px)).save(os.path.join(A.shots, name + '.png'))
        wait_ms(400)
        return lambda x, y: tuple(px[(y * 480 + x) * 3:(y * 480 + x) * 3 + 3])

    def edges(img, y, col):
        """x of every run of `col` on row y inside the graph (14..412)."""
        xs, prev = [], False
        for x in range(14, 413):
            on = near(img(x, y), col)
            if on and not prev:
                xs.append(x)
            prev = on
        return xs

    def red(img, y):
        return sum(1 for x in range(14, 413) if near(img(x, y), C_ERR))

    def prof_reset():
        if has_prof:
            q('DBG:PROF:RESET')

    def prof(what):
        # DBG:PROF? only in a build with the feature dbg-prof (else -113):
        # None, and the checks on its figures are skipped.
        if not has_prof:
            return None
        txt = q('DBG:PROF?', raw=True).decode(errors='replace')
        lines = {l.split()[0]: l.split() for l in txt.splitlines() if l and l.split()[0] in ('x2', 's.frame', 'dec.job', 'scope', 'tick')}
        z = ['', '0', '0', '0', '0', '0', '0']
        x2 = lines.get('x2', z)
        fr = lines.get('s.frame', z)
        jb = lines.get('dec.job', z)
        print('    %-34s records decoded %3s (avg %6s max %6s us, slices %4s max %5s us); frames %4s avg %5s max %5s us' % (what, jb[1], jb[3], jb[4], x2[1], x2[4], fr[1], fr[3], fr[4]), flush=True)
        return {'jobs': int(jb[1]), 'job_avg': int(jb[3]), 'job_max': int(jb[4]), 'slices': int(x2[1]), 'slice_max': int(x2[4]), 'frames': int(fr[1]), 'frame_avg': int(fr[3]), 'frame_max': int(fr[4])}

    def c_check(what, every=False):
        if not RC:
            return
        # The hooks are gated on the frame log's stream too: its own region
        # C code (LOG_COLD) runs then, never with the AD2 stream (log_open /
        # LOG:STAT ON refuse each other).
        got = [f for f in RC.calls(every) if f not in LOG_OTHER and not any(k in f for k in LOG_COLD)]
        print('  region C functions called while the AD2 stream ran: %s' % (', '.join(sorted(got)) or 'none'))
        check(not got, '%s: no region C code while the stream ISR owns the bus' % what)

    def c_check_log():
        # The frame log's stream is DMA: other main-loop code from region C
        # (a settings save, a screenshot) only slows the loop; the
        # decoder's own code must not be there.
        if not RC:
            return
        got = RC.calls()
        own = [f for f in got if any(k in f for k in ('decode', 'app_log', 'ui_log', 'scope', 'pack10'))]
        LOG_OTHER.update(f for f in got if f not in own)
        # Region C on purpose: LOG_COLD.
        own = [f for f in own if not any(k in f for k in LOG_COLD)]

        print('  region C functions called while the log streamed: %s' % (', '.join(sorted(got)) or 'none'))
        check(not own, 'the frame log: none of its code in region C (%s)' % ', '.join(own))

    print('decodetest: %s' % q('*IDN?'), flush=True)
    wait_ms(1500)
    q('DBG:HWTRIG:ENABLE 0')
    q('DBG:KEY 0')  # main menu
    wait_ms(600)
    q('DBG:TAP 339,105')  # Oscilloscope
    wait_ms(1500)
    # A build without Modbus RTU / the frame log (cargo features dec-modbus /
    # dec-log off) refuses them: those cases are skipped, and said so.
    q('*CLS')
    q('DEC:PROT MODB')
    has_mb = q('SYST:ERR?').split(',')[0].strip() in ('0', '+0')
    q('DEC:PROT OFF')
    q('*CLS')
    q('DEC:MODE LOG')
    has_log = q('SYST:ERR?').split(',')[0].strip() in ('0', '+0')
    if has_log:
        q('DEC:MODE BAND')
        wait_ms(1500)
    q('*CLS')
    q('DBG:PROF:RESET')
    has_prof = q('SYST:ERR?').split(',')[0].strip() in ('0', '+0')
    q('*CLS')
    print('decodetest: Modbus RTU %s, frame log %s, DBG:PROF? %s' % ('in' if has_mb else 'NOT in this build (skipped)',
                                                      'in' if has_log else 'NOT in this build (skipped)',
                                                      'in' if has_prof else 'NOT in this build (cost and decode counts skipped)'), flush=True)
    text = b'The quick brown fox 0123456789\r\n'
    sent = text * 4

    def has_text(data, k=8):
        return any(text[i:i + k] in data for i in range(0, len(text) - k))

    # Modbus RTU traffic: request / reply, two writes, an exception, a bad
    # CRC.
    req = mb([0x01, 0x03, 0x00, 0x00, 0x00, 0x02])
    rsp = mb([0x01, 0x03, 0x04, 0x12, 0x34, 0x56, 0x78])
    w6 = mb([0x11, 0x06, 0x00, 0x05, 0xAB, 0xCD])
    exr = mb([0x01, 0x04, 0x00, 0x10, 0x00, 0x01])
    exc = mb([0x01, 0x84, 0x02])
    bad = bytearray(req)
    bad[6] ^= 0xFF
    seq = [req, rsp, w6, w6, exr, exc, bytes(bad)]

    if want('mcx'):
        # ---- MCX UART 9600 auto ----
        put(BITS, '1' * 40 + uart_bits(text) + '1' * 40, 9600, 0.0, 3.3, 0.004)
        q('DEC:PROT UART;DEC:BAUD AUTO;DEC:FORM 8N1;DEC:POL NORM;DEC:THR AUTO')
        # Auto, the trigger level above the signal: free-running, untriggered
        # records are not decoded while running; STOP decodes the last one.
        q('CHAN1:SCAL 1;CHAN1:COUP DC;TRIG:SWE AUTO;TRIG:SLOP NEG;TRIG:LEV 8;TIM:SCAL 1E-3')
        run()
        wait_ms(800)
        prof_reset()
        wait_ms(1500)
        p = prof('1 ms/div Auto untriggered running')
        if p:
            check(p['jobs'] == 0 and p['slices'] == 0 and p['frames'] > 3, 'Auto, untriggered, running: no decode (%d records, %d frames)' % (p['jobs'], p['frames']))
        check(stop(), 'STOP (untriggered)')
        wait_ms(800)
        fr = frames()
        data = b''.join(bytes.fromhex(f[3]) for f in fr)
        # 17 ms wherever the sweep caught the traffic (up to 16 characters;
        # a record that starts inside back-to-back characters can be read
        # off by some bits until an idle gap: TODO, known limits).
        rate = q('DEC:RATE?')
        check(len(data) >= 4 and rate == '9600', 'STOP: the untriggered record decoded (%d frames, %r, rate %s)' % (len(fr), data[:40], rate))
        # Auto, triggered on the start bits (the level in the swing): each
        # triggered record decoded once.
        q('TRIG:LEV 1.6')
        run()
        wait_ms(800)
        prof_reset()
        wait_ms(1500)
        p = prof('1 ms/div 9600 Auto running')
        if p:
            check(p['jobs'] >= 1 and p['jobs'] <= p['frames'] + 1, 'Auto, triggered, running: a decode per triggered record (%d records, %d frames)' % (p['jobs'], p['frames']))
        check(q('DEC:PROT?') == 'UART', 'DEC:PROT? UART')
        check(q('DEC:RATE?') == '9600', 'auto-baud: DEC:RATE? 9600 (%s)' % q('DEC:RATE?'))
        fr = frames()
        data = b''.join(bytes.fromhex(f[3]) for f in fr)
        check(has_text(data, 5), 'running, 1 ms/div: the last record\'s frames are pieces of the text (%r, rate %s, COUN? %s)' % (data[:40], q('DEC:RATE?'), q('DEC:COUN?')))
        nbytes = len(data)
        img = shot('m01-uart-1ms-run')
        e = edges(img, BYTE_Y + 8, C_EDGE)
        # FRAMes? lists the record's bytes (the screen shows a part): adjacent
        # boxes share an edge run, at most 2 x bytes runs.
        check(2 <= len(e) <= 2 * nbytes + 2, 'band: %d box edge runs for %d bytes in the record' % (len(e), nbytes))
        check(stop(), 'STOP')
        fr = frames()
        data = b''.join(bytes.fromhex(f[3]) for f in fr)
        c = q('DEC:COUN?')
        # The record starts where the trigger caught the text (17 ms:
        # up to 16 characters).
        check(has_text(data, 4) and all(f[4] in ('OK', 'PART') for f in fr[1:]), 'stopped, 1 ms/div: the record (17 ms) decodes to pieces of the text (%d frames, COUN? %s)' % (len(fr), c))

        img = shot('m02-uart-1ms-stop')
        e = edges(img, BYTE_Y + 8, C_EDGE)
        print('    stopped band: %d box edge runs' % len(e), flush=True)
        # Pan: the stored record moved by a quarter screen (drag on the graph).
        q('DBG:DRAG 300,150,200,150,300')
        wait_ms(1500)
        img2 = shot('m03-uart-pan')
        e2 = edges(img2, BYTE_Y + 8, C_EDGE)
        check(e2 != e, 'pan: the band follows (%d -> %d edge runs)' % (len(e), len(e2)))
        # Zoom in: 200 us/div (2 characters on the screen).
        q('TIM:SCAL 200E-6')
        wait_ms(1500)
        img = shot('m04-uart-200us-stop')
        e = edges(img, BYTE_Y + 8, C_EDGE)
        check(len(e) <= 8, 'zoomed in, 200 us/div: %d box edge runs (wide boxes)' % len(e))
        fr2 = frames()
        check(len(fr2) == len(fr), 'FRAMes? of the stopped record does not depend on the zoom (%d / %d)' % (len(fr2), len(fr)))
        # 5 ms/div: the bar; stopped, the record (86 ms) holds whole lines.
        q('TIM:SCAL 5E-3')
        run()
        check(stop(), 'STOP at 5 ms/div')
        fr = frames()
        data = b''.join(bytes.fromhex(f[3]) for f in fr)
        check(text in data, 'stopped, 5 ms/div: the whole text in the record (%d frames)' % len(fr))
        img = shot('m05-uart-5ms-bar')
        bar = sum(1 for x in range(14, 413) if near(img(x, BYTE_Y + 8), C_EDGE))
        check(bar > 60 and len(edges(img, BYTE_Y + 8, C_EDGE)) < 12, '5 ms/div: the bar (%d px, %d runs)' % (bar, len(edges(img, BYTE_Y + 8, C_EDGE))))
        # 100 ms/div (25 kS/s, 2.6 samples per bit; roll: decoded on STOP):
        # the note, no frames.
        q('TIM:SCAL 100E-3')
        run()
        wait_ms(2000)
        check(stop(), 'STOP at 100 ms/div')
        wait_ms(1500)
        img = shot('m06-uart-100ms-note')
        grey = sum(1 for x in range(14, 413) if near(img(x, BYTE_Y + 8), rgb(0x7bef)))
        check(grey > 10, '100 ms/div: the note "Sample rate too low" (%d px)' % grey)
        check(frames() == [], '100 ms/div: no frames (too few samples per bit)')
    if want('cost') and not has_prof:
        print('cost: skipped (no DBG:PROF? in this build: --features dbg-prof)', flush=True)
    if want('cost') and has_prof:
        # Cost at 1 / 10 / 100 ms/div with 9600 and 115200 traffic, Auto
        # trigger: the decoder off, then on (running: per triggered record and
        # per frame; then STOP: the stored record's decode, and a pan).
        COST.append('| s/div | baud | decoder | records decoded | decode per record avg / max, us | slices max, us | frames | frame avg / max, us |')
        COST.append('|---|---|---|---|---|---|---|---|')
        for baud in (9600, 115200):
            put(BITS, '1' * 40 + uart_bits(text) + '1' * 40, baud, 0.0, 3.3, 20.0 / baud)
            for sc in ('1E-3', '10E-3', '100E-3'):
                q('TIM:SCAL ' + sc)
                for proto in ('OFF', 'UART'):
                    q('DEC:PROT ' + proto)
                    run()
                    wait_ms(1000)
                    prof_reset()
                    wait_ms(3000)
                    p = prof('%s s/div %d %s running' % (sc, baud, proto))
                    COST.append('| %s | %d | %s running | %d | %d / %d | %d | %d | %d / %d |' % (sc, baud, proto, p['jobs'], p['job_avg'], p['job_max'], p['slice_max'], p['frames'], p['frame_avg'], p['frame_max']))
                prof_reset()
                stop()
                wait_ms(1500)
                p = prof('%s s/div %d STOP' % (sc, baud))
                COST.append('| %s | %d | UART STOP | %d | %d / %d | %d | %d | %d / %d |' % (sc, baud, p['jobs'], p['job_avg'], p['job_max'], p['slice_max'], p['frames'], p['frame_avg'], p['frame_max']))
                prof_reset()
                q('DBG:DRAG 300,150,200,150,300')
                wait_ms(1500)
                p = prof('%s s/div %d stopped, pan' % (sc, baud))
                COST.append('| %s | %d | UART pan | %d | %d / %d | %d | %d | %d / %d |' % (sc, baud, p['jobs'], p['job_avg'], p['job_max'], p['slice_max'], p['frames'], p['frame_avg'], p['frame_max']))
        q('TRIG:SWE AUTO')

    if want('modbus'):
        # ---- framing errors (every stop bit low), drawn red ----
        put(BITS, '1' * 40 + uart_bits(text, err='all') + '1' * 40, 9600, 0.0, 3.3, 0.004)
        q('TIM:SCAL 1E-3')
        run()
        stop()
        fr = frames()
        check(any(f[4] == 'FE' for f in fr), 'framing error: a frame with status FE')
        img = shot('m07-uart-fe')
        check(red(img, BYTE_Y + 8) > 3 or red(img, BYTE_Y) > 3, 'framing error: a red box in the band')

        if has_mb:
            # ---- MCX Modbus RTU 19200 8E1 ----
            baud = 19200
            gap = '1' * 60
            bits = '1' * 80 + ''.join(uart_bits(f, '8E1') + gap for f in seq)
            put(BITS, bits, baud, 0.0, 3.3, 0.003)
            q('DEC:PROT MODB;DEC:BAUD AUTO;DEC:FORM 8E1')
            for sc in ('2E-3', '5E-3'):
                q('TIM:SCAL ' + sc)
                run()
                stop()
                fr = frames()
                for f in fr[:9]:
                    print('    ' + ','.join(f), flush=True)
                by = {f[0]: f for f in fr}
                pairs = [f for f in fr if f[5].startswith('RSP 01 03 4B req=') and by.get(f[5].split('req=')[1], [''] * 6)[5] == 'REQ 01 03 @0000 n=2']
                if sc == '2E-3':
                    # 34 ms of the ~60 ms cycle: where the trigger caught it.
                    check(any(f[5].startswith(('REQ', 'RSP', 'EXC')) for f in fr), '2 ms/div: Modbus frames decoded (%d)' % len(fr))
                if sc == '5E-3':
                    check(len(pairs) >= 1, '%s s/div: a 03 reply paired with its request' % sc)

                    # The 86 ms record holds a whole cycle.
                    check(any(f[5].startswith('EXC 01 84 E02 req=') for f in fr), '5 ms/div: the exception, paired')
                    check(any(f[4] == 'CRC' and f[3] == bytes(bad).hex().upper() for f in fr), '5 ms/div: the bad CRC')
                img = shot('m08-modbus-%s' % sc)
                if sc == '2E-3':
                    fe = edges(img, FRAME_Y + 8, C_FEDGE)
                    check(len(fe) >= 2, '2 ms/div: frame boxes in the band (%d edges)' % len(fe))
                else:
                    # 19200 at 5 ms/div: a character is 4.6 px, the bar.
                    bar = sum(1 for x in range(14, 413) if near(img(x, FRAME_Y + 8), C_EDGE))
                    check(bar > 60, '5 ms/div: the bar (%d px)' % bar)

    if want('dali'):
        q('*CLS')
        q('DEC:PROT DALI')
        if not q('SYST:ERR?').split(',')[0].strip() in ('0', '+0'):
            print('decodetest: DALI NOT in this build (skipped)', flush=True)
        else:
            # ---- MCX DALI: 1200 bit/s bi-phase, idle high ----
            gap = '1' * 24
            cyc = (gap + dali_bits(0xFE80, 16) + '1' * 14 + dali_bits(0xFF, 8) + gap + dali_bits(0xC1A2B3, 24) + gap
                   + dali_bits(0x9B21, 16, bad=9) + gap + dali_bits(0x0521, 16) + gap)
            put(BITS, cyc, 2400, 0.0, 3.3, 0.01)
            want_t = {'FF16:FE 80 BC DAPC', 'BF8:FF', 'FF24:C1 A2 B3', 'ERR', 'FF16:05 21 S2 CMD'}
            q('CHAN1:SCAL 1;CHAN1:COUP DC;TRIG:SWE AUTO;TRIG:SLOP NEG;TRIG:LEV 1.6;TIM:SCAL 10E-3')
            run()
            check(stop(), 'DALI: STOP at 10 ms/div')
            wait_ms(800)
            fr = frames()
            for f in fr[:8]:
                print('    ' + ','.join(f), flush=True)
            got = {f[5] for f in fr}
            check(q('DEC:PROT?') == 'DALI' and q('DEC:RATE?') == '1200', 'DALI: DEC:PROT? DALI, DEC:RATE? 1200')
            check(got == want_t and all(f[2] == 'DALI' for f in fr), 'DALI, 10 ms/div: every frame of the cycle, nothing else (%s)' % sorted(got))
            check(all((f[4] == 'ERR') == (f[5] == 'ERR') for f in fr) and any(f[3] == 'FE80' for f in fr), 'DALI: status ERR only on the bad frame, hex FE80')
            q('TIM:SCAL 2E-3')
            wait_ms(1500)
            img = shot('m09-dali-2ms')
            fe = edges(img, BYTE_Y + 8, C_FEDGE)
            check(len(fe) + red(img, BYTE_Y + 8) >= 2, 'DALI, 2 ms/div: frame boxes in the band (%d edge runs)' % len(fe))
            check({f[5] for f in frames()} == got, 'DALI: FRAMes? does not depend on the zoom')
        q('DEC:PROT OFF')

    if want('wave'):
        # ---- the Wave view (DC V, 5 V TTL on V-COM) ----
        if not A.no_wave:
            q('DEC:PROT UART;DEC:BAUD AUTO;DEC:FORM 8N1')
            put(DBITS, '1' * 40 + uart_bits(text) + '1' * 40, 2400, 0.0, 5.0, 0.01)
            q('CONF:VOLT:DC 6')
            q('DBG:KEY 0')  # main menu
            wait_ms(600)
            q('DBG:TAP 145,105')  # Multimeter
            wait_ms(4500)
            q('DBG:KEY 3')
            wait_ms(600)
            q('DBG:TAP 298,259')  # Wave (0.4.0 MF layout, as hy3131/wavetest.py)
            wait_ms(6000)
            q('TIM:SCAL 2E-3')  # 38.4 kS/s
            wait_ms(2000)
            if RC:
                RC.reset()
            for baud, fmt, proto in ((2400, '8N1', 'UART'), (4800, '8N1', 'UART')) + (((2400, '8E1', 'MODB'),) if has_mb else ()):
                if proto == 'MODB':
                    bits = '1' * 60 + ''.join(uart_bits(f, fmt) + '1' * 60 for f in seq)
                    put(DBITS, bits, baud, 0.0, 5.0, 0.01)
                else:
                    put(DBITS, '1' * 40 + uart_bits(text, fmt) + '1' * 40, baud, 0.0, 5.0, 0.01)
                q('DEC:PROT %s;DEC:FORM %s' % (proto, fmt))
                run()
                wait_ms(7000)  # the stored record reaches back seconds (coded ring)
                # The Wave view decodes on STOP (its record changes while running).
                stop()
                wait_ms(1500)
                rate = q('DEC:RATE?')
                img = shot('w01-wave-%s-%d' % (proto.lower(), baud))
                check(rate == str(baud), 'Wave view %s %d: DEC:RATE? %s' % (proto, baud, rate))
                fr = frames()
                data = b''.join(bytes.fromhex(f[3]) for f in fr)
                if proto == 'UART':
                    bad = [f for f in fr[1:] if f[4] not in ('OK', 'PART')]
                    check(has_text(data) and not bad, 'Wave view UART %d: the text from the record (%d frames, %s)' % (baud, len(fr), bad[:2]))
                else:
                    kinds = [f[5].split()[0] for f in fr if f[5]]
                    check('REQ' in kinds and 'RSP' in kinds and 'CRC' in [f[4] for f in fr], 'Wave view Modbus %d: REQ, RSP, a CRC error (%s)' % (baud, kinds[:8]))
            # 19200 at 38.4 kS/s: the note.
            put(DBITS, '1' * 40 + uart_bits(text) + '1' * 40, 19200, 0.0, 5.0, 0.01)
            q('DEC:PROT UART;DEC:BAUD AUTO')
            run()
            wait_ms(7000)
            stop()
            wait_ms(1500)
            img = shot('w02-wave-19200-note')
            grey = sum(1 for x in range(14, 413) if near(img(x, BYTE_Y + 8), rgb(0x7bef)))
            check(grey > 10, 'Wave view 19200 at 38.4 kS/s: the note (%d px)' % grey)
            c_check('the Wave view with the decoder')
            q('DBG:KEY 0')
            wait_ms(2500)

    if want('log') and has_log:
        # ---- the frame log (Mode Log): MCX, decoded while the input streams ----
        q('DEC:PROT UART;DEC:BAUD AUTO;DEC:FORM 8N1;DEC:POL NORM;DEC:THR AUTO')
        q('CHAN1:SCAL 1;CHAN1:COUP DC;TIM:SCAL 1E-3;TRIG:SWE AUTO;TRIG:LEV 1.6')
        put(BITS, '1' * 40 + uart_bits(text) + '1' * 40, 9600, 0.0, 3.3, 0.004)
        run()
        wait_ms(1000)
        if RC:
            RC.reset()
        q('DEC:MODE LOG')
        check(q('DEC:MODE?') == 'LOG', 'DEC:MODE LOG: the frame log (%s)' % q('DEC:MODE?'))
        # No DMM logger stream with the log open (its view code is region C).
        q('LOG:STAT ON')
        e = err()
        check('conflict' in e and q('LOG:STAT?') == '0', 'log open: LOG:STAT ON refused (%s)' % e)
        wait_ms(1000)
        fifo()
        prof_reset()
        wait_ms(3000)
        p = prof('log 9600, 3 s')
        check(q('DEC:RATE?') == '9600', 'log: DEC:RATE? 9600 (%s)' % q('DEC:RATE?'))
        fr = fifo()
        data = b''.join(bytes.fromhex(f[3]) for f in fr)
        check(text in data, 'log 9600: the whole text in the FIFO (%d frames)' % len(fr))
        fr2 = fifo()
        check(not fr2 or not fr or int(fr2[0][0]) > int(fr[-1][0]), 'log: the FIFO gives each frame once')
        c = q('DEC:COUN?').split(',')
        # The first character may be read from the middle of one (FE), once
        # at the start and once when the stream restarts at the rate found.
        check(len(c) == 6 and int(c[0]) >= len(fr) and int(c[2]) <= 2 and c[5] == '0', 'log: DEC:COUN? frames,bytes,FE,PE,CRC,lost %s' % ','.join(c))

        shot('l01-log-uart-9600')
        # A frame's waveform: STOP (the list stays), tap the first row.
        q('DBG:KEY 1')
        wait_ms(800)
        q('DBG:TAP 200,30')
        wait_ms(1500)
        img = shot('l04-log-snippet')
        e = edges(img, BYTE_Y + 8, C_EDGE)
        yellow = sum(1 for x in range(14, 413) for y in range(60, 230, 4) if near(img(x, y), (255, 255, 0)))
        check(len(e) >= 2 and yellow > 50, 'log: a row tapped shows its waveform (%d px of trace) and its bytes (%d box edges)' % (yellow, len(e)))
        q('DBG:TAP 200,150')
        wait_ms(1500)
        img = shot('l05-log-back')
        check(len(edges(img, BYTE_Y + 8, C_EDGE)) == 0, 'log: a tap on the waveform goes back to the list')
        q('DBG:KEY 1')
        wait_ms(800)
        # 115200 (auto: the highest stream rate, 10.9 samples a bit).
        put(BITS, '1' * 40 + uart_bits(text) + '1' * 40, 115200, 0.0, 3.3, 20.0 / 115200)
        wait_ms(2000)
        fifo()
        lost0 = int(q('DEC:COUN?').split(',')[5])
        prof_reset()
        wait_ms(3000)
        p = prof('log 115200, 3 s')
        fr = fifo()
        data = b''.join(bytes.fromhex(f[3]) for f in fr)
        c = q('DEC:COUN?').split(',')
        print('    log 115200: %d frames in 3 s, COUN? %s' % (len(fr), ','.join(c)), flush=True)
        check(q('DEC:RATE?') == '115200' and text in data, 'log 115200: rate %s, the whole text' % q('DEC:RATE?'))
        check(int(c[5]) == lost0, 'log 115200: the decode keeps up for 3 s (lost %d before, %s after)' % (lost0, c[5]))

        shot('l02-log-uart-115200')
        # Modbus RTU 19200 8E1: request / reply pairs, the exception, a bad CRC.
        q('DEC:PROT MODB;DEC:FORM 8E1')
        put(BITS, '1' * 80 + ''.join(uart_bits(f, '8E1') + '1' * 60 for f in seq), 19200, 0.0, 3.3, 0.003)
        wait_ms(2000)
        fifo()
        wait_ms(2000)
        fr = fifo()
        for f in fr[:8]:
            print('    ' + ','.join(f), flush=True)
        by = {f[0]: f for f in fr}
        pairs = [f for f in fr if f[5].startswith('RSP 01 03 4B req=') and by.get(f[5].split('req=')[1], [''] * 6)[5] == 'REQ 01 03 @0000 n=2']
        check(len(pairs) >= 1 and any(f[4] == 'CRC' for f in fr) and any(f[5].startswith('EXC') for f in fr), 'log Modbus: a 03 pair, the exception, a bad CRC (%d frames)' % len(fr))
        shot('l03-log-modbus')
        c_check_log()
        # Capacity: from a CLEar until the store is full, then STOP: the
        # FIFO holds every frame kept (Modbus 19200 8E1, with waveforms).
        q('DEC:CLE')
        wait_ms(12000)
        q('DBG:KEY 1')
        wait_ms(800)
        kept = fifo()
        c = q('DEC:COUN?').split(',')
        print('    log capacity: %d Modbus frames kept of %s (%s)' % (len(kept), c[0], ','.join(c)), flush=True)
        check(0 < len(kept) < int(c[0]), 'log: the store fills, the oldest go (%d of %s kept)' % (len(kept), c[0]))
        q('DBG:KEY 1')
        wait_ms(800)

        q('DEC:CLE')
        c = q('DEC:COUN?').split(',')
        check(c[0] == '0' or int(c[0]) < 5, 'DEC:CLEar: the counters start again (%s)' % ','.join(c))
        # Back to the band: the log ends, the scope captures again.
        q('DEC:PROT UART;DEC:FORM 8N1;DEC:MODE BAND')
        put(BITS, '1' * 40 + uart_bits(text) + '1' * 40, 9600, 0.0, 3.3, 0.004)
        check(q('DEC:MODE?') == 'BAND', 'DEC:MODE BAND')
        run()
        wait_ms(1500)
        check(stop(), 'STOP after the log')
        wait_ms(1000)
        fr = frames()
        check(has_text(b''.join(bytes.fromhex(f[3]) for f in fr), 4), 'after the log: the band decodes a new record (%d frames)' % len(fr))
        # Log / Band in and out while band decodes are in flight (1 ms/div,
        # 115200): the scope captures again each time.
        put(BITS, '1' * 40 + uart_bits(text) + '1' * 40, 115200, 0.0, 3.3, 20.0 / 115200)
        q('TIM:SCAL 1E-3')
        run()
        for k in range(5):
            wait_ms(300 + 70 * k)
            q('DEC:MODE LOG')
            wait_ms(400)
            q('DEC:MODE BAND')
        wait_ms(1500)
        st = q('TRIG:STAT?')
        check(stop(), 'Log / Band 5 times: STOP completes (status was %s)' % st)
        wait_ms(800)
        fr = frames()
        check(st not in ('STOP', 'WAIT') and fr, 'Log / Band 5 times: the scope captures and decodes again (%s, %d frames)' % (st, len(fr)))


    q('DEC:PROT OFF')
    check(err().startswith('0,'), 'no SCPI error left (%s)' % err())

    b = q('DBG:BOOT?').split(',')
    if 'STACK' in b:
        i = b.index('STACK')
        print('    stack high-water %s of %s bytes' % (b[i + 1], b[i + 2]), flush=True)
finally:
    if RC:
        try:
            c_check('the whole run', True)
        except Exception as e:
            print('region C check: %s' % e)
    os.killpg(emu.pid, 15)
    emu.wait()
    if RC:
        RC.cleanup()
    for p in (BITS, DBITS, SCRIPT, os.path.join(OUT, TAG + '.log'), os.path.join(OUT, TAG + '.resc'), os.path.join(OUT, 'flash-' + TAG + '.bin')):
        if os.path.exists(p):
            os.remove(p)

print('\n'.join(COST))
print('decodetest: %s' % ('PASS' if not FAIL else 'FAIL: ' + '; '.join(FAIL)))
sys.exit(1 if FAIL else 0)
