#!/usr/bin/env python3
"""Read voltage and current from an Alientek C2 USB tester over Bluetooth LE
(read-only: it sends only the "register", "connect" and "real data" queries).

    c2meter.py [--name C2_BLE] [--period 1.0] [--secs N] [--csv FILE]

Prints one line per poll: host time, V, A, W (the mean of the 10 samples in
each reply) and the min / max current of those samples.

Protocol (the same framing as the DM40's stock Bluetooth protocol,
docs/bt-protocol.md; the C2's commands and the real-data layout as described
by github.com/bruno-ntmr/atk-c2, checked here against a C2 Std):
  * GATT service 0xFFF0; requests are written to 0xFFF3 (write without
    response), replies come as notifications on 0xFFF2 in pieces of <= 20
    bytes, to be joined.
  * Frame: 0xAF (request) / 0xDF (reply), device type (u16 LE: 0xFFFF for
    any, the C2 is 778 = 0x030A), function, length (1 byte), data, check
    byte = 256 - (sum of all bytes before it) mod 256.
  * Functions: 0 register (reply: device type u16 LE + a model byte),
    1 connect, 2 version (contains the serial number: not used here),
    4 real data.
  * Real-data reply, 199 data bytes: text fields (PD protocol / contract,
    0..55), then 10 voltage samples (u24 LE, unit 10 uV) at 57, 10 current
    samples (u24 LE, unit 10 uA) at 87, then 10 each of D+, D-, CC1, CC2
    (u16 LE, mV) from 117. The current has no sign field: the direction
    through the tester is not reported (*inferred*).
Needs bleak.
"""
import argparse
import asyncio
import sys
import time

from bleak import BleakClient, BleakScanner

U = '0000%s-0000-1000-8000-00805f9b34fb'
C2 = 778


def frame(func, dev=C2, data=b''):
    b = bytes([0xAF, dev & 0xFF, dev >> 8, func, len(data)]) + data
    return b + bytes([(256 - sum(b) % 256) % 256])


class Link:
    def __init__(self, cl):
        self.cl = cl
        self.buf = bytearray()
        self.q = asyncio.Queue()

    def rx(self, _h, d):
        self.buf += d
        while True:
            i = self.buf.find(0xDF)
            if i < 0:
                self.buf.clear()
                return
            del self.buf[:i]
            if len(self.buf) < 6 or len(self.buf) < 6 + self.buf[4]:
                return
            n = 6 + self.buf[4]
            f = bytes(self.buf[:n])
            del self.buf[:n]
            if sum(f) % 256 == 0:
                self.q.put_nowait((f[1] | f[2] << 8, f[3], f[5:-1]))

    async def ask(self, func, dev=C2, timeout=2.0):
        while not self.q.empty():
            self.q.get_nowait()
        await self.cl.write_gatt_char(U % 'fff3', frame(func, dev), response=False)
        end = time.monotonic() + timeout
        while time.monotonic() < end:
            try:
                d, f, p = await asyncio.wait_for(self.q.get(), end - time.monotonic())
            except asyncio.TimeoutError:
                break
            if f == func:
                return p
        return None


def real(p):
    if p is None or len(p) < 147:
        return None
    u24 = lambda o: p[o] | p[o + 1] << 8 | p[o + 2] << 16
    v = [u24(57 + 3 * i) / 1e5 for i in range(10)]
    a = [u24(87 + 3 * i) / 1e5 for i in range(10)]
    return sum(v) / 10, sum(a) / 10, min(a), max(a)


async def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument('--name', default='C2_BLE')
    ap.add_argument('--period', type=float, default=1.0)
    ap.add_argument('--secs', type=float, default=0, help='stop after N s (0 = until Ctrl-C)')
    ap.add_argument('--csv', help='also append "time,V,A,Amin,Amax" lines here')
    a = ap.parse_args()
    # The C2 refuses a connection for a while after the previous one ended
    # (a TimeoutError from bleak): retry.
    for k in range(6):
        dev = await BleakScanner.find_device_by_name(a.name, timeout=30)
        if dev is None:
            sys.exit('c2meter: %s not found' % a.name)
        try:
            await run(a, dev)
            return
        except asyncio.TimeoutError:
            print('c2meter: connect timed out, retry %d' % (k + 1), file=sys.stderr, flush=True)
            await asyncio.sleep(5)
    sys.exit('c2meter: no connection')


async def run(a, dev):
    async with BleakClient(dev, timeout=20) as cl:
        ln = Link(cl)
        await cl.start_notify(U % 'fff2', ln.rx)
        r = await ln.ask(0, dev=0xFFFF)
        if not r or (r[0] | r[1] << 8) != C2:
            sys.exit('c2meter: not a C2 (register reply %r)' % r)
        await ln.ask(1)
        out = open(a.csv, 'a') if a.csv else None
        t0 = time.time()
        while not a.secs or time.time() - t0 < a.secs:
            m = real(await ln.ask(4))
            if m:
                line = '%.2f,%.5f,%.5f,%.5f,%.5f' % (time.time(), *m)
                print('%s  %.4f V  %.4f A  %.3f W  (A %.4f..%.4f)' % (time.strftime('%H:%M:%S'), m[0], m[1], m[0] * m[1], m[2], m[3]), flush=True)
                if out:
                    out.write(line + '\n')
                    out.flush()
            await asyncio.sleep(a.period)


if __name__ == '__main__':
    try:
        asyncio.run(main())
    except (KeyboardInterrupt, EOFError):
        pass  # EOFError: BlueZ's D-Bus reply to the disconnect at the end (bleak)
