#!/usr/bin/python3 -I
"""Test files for tools/gallery-test: BMPs in every format src/gallery.rs
reads (plus unsupported / damaged ones) and csvlog-format CSVs.

  gen.py <out dir>

Per BMP <NAME>.BMP a reference <NAME>.ref: u32 w, u32 h, then w*h RGB565
(u16 LE) top-down; 0xFFFFFFFF in place of w = "must be unsupported".
Per CSV <NAME>.CSV an expectation <NAME>.exp: rows vals min max avg span_ms
(min / max / avg of the values written, as parsed back from the text).
Sets (one file list per disk image): set-<name>.txt, files in copy order.
"""
import math
import os
import struct
import sys

OUT = sys.argv[1]
os.makedirs(OUT, exist_ok=True)


def rgb565(r, g, b):
    return ((r & 0xF8) << 8) | ((g & 0xFC) << 3) | (b >> 3)


def to888(c):
    return ((c >> 11) * 255 // 31, ((c >> 5) & 63) * 255 // 63, (c & 31) * 255 // 31)


def scope_like(w, h, seed=0):
    """A screenshot-ish picture: dark background, grid, a trace, a bar;
    low bits vary per pixel so a sampling / decoding slip shows."""
    px = []
    for y in range(h):
        row = []
        for x in range(w):
            r, g, b = 8, 16 + (y * 40) // h, 24 + (x * 60) // w
            if y < h // 12:
                r, g, b = 64, 64, 72
            if x % max(1, w // 12) == 0 or y % max(1, h // 8) == 0:
                r, g, b = 90, 90, 90
            yy = h / 2 + (h / 3) * math.sin((x + seed * 37) * 2 * math.pi / max(1, w / 2))
            if abs(y - yy) < max(1.5, h / 100):
                r, g, b = 255, 230, 0
            if (x // 8 + y // 8 + seed) % 23 == 0:
                r, g, b = 200, 60, 40
            c = rgb565(r, g, b) ^ (((x * 31 + y * 17 + seed) & 0x3) | (((x + 2 * y) & 1) << 5))
            row.append(c)
        px.append(row)
    return px


def write_ref(name, w, h, px):
    with open(os.path.join(OUT, name + '.ref'), 'wb') as f:
        f.write(struct.pack('<II', w, h))
        for row in px:
            f.write(struct.pack('<%dH' % w, *row))


def write_unsup(name):
    with open(os.path.join(OUT, name + '.ref'), 'wb') as f:
        f.write(struct.pack('<II', 0xFFFFFFFF, 0))


def bmp(name, w, h, bpp, comp, pixdata, pal=None, masks=None, topdown=False, hdr_extra=b''):
    info = struct.pack('<IiiHHIIiiII', 40, w, -h if topdown else h, 1, bpp, comp, len(pixdata), 0, 0,
                       len(pal) if pal else 0, 0)
    extra = b''
    if masks:
        extra = struct.pack('<III', *masks)
    if pal:
        extra += b''.join(struct.pack('<BBBB', b, g, r, 0) for (r, g, b) in pal)
    off = 14 + 40 + len(extra) + len(hdr_extra)
    data = info + extra + hdr_extra + pixdata
    fh = b'BM' + struct.pack('<IHHI', 14 + len(data), 0, 0, off)
    with open(os.path.join(OUT, name + '.BMP'), 'wb') as f:
        f.write(fh + data)
    return 14 + len(data)


def rows_bottom_up(px, topdown):
    return px if topdown else list(reversed(px))


def bmp565(name, w, h, px, topdown=False):
    out = bytearray()
    for row in rows_bottom_up(px, topdown):
        b = struct.pack('<%dH' % w, *row)
        out += b + b'\0' * ((-len(b)) % 4)
    write_ref(name, w, h, px)
    return bmp(name, w, h, 16, 3, bytes(out), masks=(0xF800, 0x07E0, 0x001F), topdown=topdown)


def bmp555(name, w, h, px):
    out = bytearray()
    ref = []
    for row in px:
        ref.append([])
    for y, row in enumerate(px):
        for c in row:
            r, g, b = (c >> 11) & 31, (c >> 6) & 31, c & 31  # 5 bits each
            ref[y].append((r << 11) | (g << 6) | ((g >> 4) << 5) | b)
    for row in reversed(px):
        vals = [(((c >> 11) & 31) << 10) | (((c >> 6) & 31) << 5) | (c & 31) for c in row]
        b = struct.pack('<%dH' % w, *vals)
        out += b + b'\0' * ((-len(b)) % 4)
    write_ref(name, w, h, ref)
    return bmp(name, w, h, 16, 0, bytes(out))


def bmp24(name, w, h, px, bpp=24, topdown=False):
    out = bytearray()
    ref = []
    for row in px:
        ref.append([rgb565(*to888(c)) for c in row])
    for row in rows_bottom_up(px, topdown):
        b = bytearray()
        for c in row:
            r, g, bb = to888(c)
            b += bytes([bb, g, r]) + (b'\0' if bpp == 32 else b'')
        out += b + b'\0' * ((-len(b)) % 4)
    write_ref(name, w, h, ref)
    return bmp(name, w, h, bpp, 0, bytes(out), topdown=topdown)


PAL16 = [(0, 0, 0), (255, 255, 255), (255, 0, 0), (0, 200, 0), (0, 0, 255), (255, 255, 0), (0, 255, 255),
         (255, 0, 255), (128, 128, 128), (64, 64, 64), (255, 128, 0), (0, 128, 255), (128, 0, 255), (32, 64, 96),
         (200, 200, 160), (16, 32, 48)]


def idx_pic(w, h, seed=0):
    px = []
    for y in range(h):
        row = []
        for x in range(w):
            v = 15
            if y < h // 10:
                v = 13
            if (x // 24 + y // 24 + seed) % 5 == 0:
                v = (x // 24) % 13
            if abs(y - (h / 2 + h / 4 * math.sin(x / 30 + seed))) < 2:
                v = 5
            if (x * 7 + y * 3) % 61 == 0:
                v = (x + y) % 16
            row.append(v)
        px.append(row)
    return px


def pal_ref(name, w, h, idx, pal, skip=None):
    ref = [[rgb565(*pal[i]) if i is not None else 0 for i in row] for row in idx]
    write_ref(name, w, h, ref)


def bmp4(name, w, h, idx):
    out = bytearray()
    for row in reversed(idx):
        b = bytearray()
        for x in range(0, w, 2):
            hi = row[x]
            lo = row[x + 1] if x + 1 < w else 0
            b.append(hi << 4 | lo)
        out += b + b'\0' * ((-len(b)) % 4)
    pal_ref(name, w, h, idx, PAL16)
    return bmp(name, w, h, 4, 0, bytes(out), pal=PAL16)


def bmp1(name, w, h):
    pal = [(10, 20, 40), (250, 140, 0)]
    idx = [[1 if (x // 5 + y // 7) % 3 == 0 or (x * y) % 97 == 1 else 0 for x in range(w)] for y in range(h)]
    out = bytearray()
    for row in reversed(idx):
        b = bytearray()
        for x in range(0, w, 8):
            v = 0
            for k in range(8):
                if x + k < w and row[x + k]:
                    v |= 0x80 >> k
            b.append(v)
        out += b + b'\0' * ((-len(b)) % 4)
    pal_ref(name, w, h, idx, pal)
    return bmp(name, w, h, 1, 0, bytes(out), pal=pal)


def rle4_row(row):
    out = bytearray()
    x, n = 0, len(row)
    while x < n:
        r = 1
        while x + r < n and r < 255 and row[x + r] == row[x]:
            r += 1
        if r >= 3:
            out += bytes([r, row[x] << 4 | row[x]])
            x += r
            continue
        # literal stretch up to the next run of >= 3
        e = x
        while e < n and e - x < 255:
            q = 1
            while e + q < n and q < 3 and row[e + q] == row[e]:
                q += 1
            if q >= 3:
                break
            e += 1
        L = e - x
        if L < 3:
            a = row[x]
            b = row[x + 1] if L == 2 else 0
            out += bytes([L, a << 4 | b])
        else:
            out += bytes([0, L])
            lit = bytearray()
            for k in range(0, L, 2):
                lit.append(row[x + k] << 4 | (row[x + k + 1] if k + 1 < L else 0))
            if len(lit) % 2:
                lit.append(0)
            out += lit
        x += L
    return out


def bmp_rle4(name, w, h, idx, deltas=False):
    """BI_RLE4; deltas: row 10 (from the bottom) jumps 7 px at x = 20, and
    rows 30..31 are skipped by a (0, 2) delta: those pixels stay black."""
    out = bytearray()
    ref = [list(r) for r in idx]
    bu = list(reversed(idx))
    k = 0
    while k < h:
        row = bu[k]
        if deltas and k == 10:
            out += rle4_row(row[:20]) + bytes([0, 2, 7, 0]) + rle4_row(row[27:])
            for x in range(20, 27):
                ref[h - 1 - k][x] = None
        elif deltas and k == 30:
            out += bytes([0, 2, 0, 2])  # x += 0, rows 30 and 31 skipped
            for kk in (30, 31):
                for x in range(w):
                    ref[h - 1 - kk][x] = None
            k += 2
            out += rle4_row(bu[k]) + bytes([0, 0])
            k += 1
            continue
        else:
            out += rle4_row(row)
        out += bytes([0, 0])
        k += 1
    out[-2:] = bytes([0, 1])
    pal_ref(name, w, h, ref, PAL16)
    size = bmp(name, w, h, 4, 2, bytes(out), pal=PAL16)
    return size


def bmp8(name, w, h):
    pal = [(i, i, i) for i in range(256)]
    out = bytes(((x + y) & 255) for y in range(h) for x in range(((w + 3) // 4) * 4))
    write_unsup(name)
    return bmp(name, w, h, 8, 0, out, pal=pal)


def truncated(name, w, h):
    bmp565(name, w, h, scope_like(w, h, 9))
    p = os.path.join(OUT, name + '.BMP')
    d = open(p, 'rb').read()
    open(p, 'wb').write(d[:len(d) // 2])
    write_unsup(name)


def fmt6(v):
    if v == 0:
        return '0'
    a = abs(v)
    if 1e-3 <= a < 1e6:
        s = '%.6g' % v
        if 'e' in s:
            s = '%f' % v
        return s
    return '%.5E' % v


def csv(name, func, unit, rng, n, dt_ms, fn, flags=lambda i: ''):
    lines = ['# Alientek DM40A, opendm40 0.1.0, function %s, range %s, started uptime 12.345 s' % (func, rng),
             '# time_s = seconds since the start of this recording (no real-time clock); empty value = overflow',
             'time_s,value,unit,flags']
    vals = []
    t_last = 0
    for i in range(n):
        t = i * dt_ms
        v = fn(i)
        fl = flags(i)
        if v is None:
            vs = ''
            fl = ('OL ' + fl).strip()
        else:
            vs = fmt6(v)
            vals.append(float(vs))
        lines.append('%d.%03d,%s,%s,%s' % (t // 1000, t % 1000, vs, unit, fl))
        t_last = t
    data = ('\r\n'.join(lines) + '\r\n').encode()
    open(os.path.join(OUT, name + '.CSV'), 'wb').write(data)
    if vals:
        exp = (n, len(vals), min(vals), max(vals), sum(vals) / len(vals), t_last if n else 0)
    else:
        exp = (n, 0, 0, 0, 0, t_last if n else 0)
    open(os.path.join(OUT, name + '.exp'), 'w').write('%d %d %.9g %.9g %.9g %d\n' % exp)
    return len(data)


def sets(name, files):
    open(os.path.join(OUT, 'set-%s.txt' % name), 'w').write('\n'.join(files) + '\n')


# ---- files
bmp565('S000', 480, 320, scope_like(480, 320, 0))
bmp565('S001', 480, 320, scope_like(480, 320, 1))
bmp24('S002', 200, 150, scope_like(200, 150, 2))
bmp_rle4('S003', 480, 320, idx_pic(480, 320, 3))
bmp_rle4('S004', 120, 80, idx_pic(120, 80, 4), deltas=True)
bmp24('S005', 333, 211, scope_like(333, 211, 5), bpp=32, topdown=True)
bmp4('S006', 101, 67, idx_pic(101, 67, 6))
bmp1('S007', 250, 90)
bmp555('S008', 160, 120, scope_like(160, 120, 8))
bmp565('S009', 480, 320, scope_like(480, 320, 9), topdown=True)
bmp24('S010', 641, 480, scope_like(641, 480, 10))  # larger than the screen: scaled down
bmp24('S011', 7, 999, scope_like(7, 999, 11))  # tall and thin
bmp8('S012', 64, 48)
truncated('S013', 480, 320)
bmp565('SWWWWWWW', 480, 320, scope_like(480, 320, 4))
bmp24('S014', 96, 64, scope_like(96, 64, 14))

csv('LOG000', 'DC V', 'V', 'AUTO', 50, 250, lambda i: 1.0 + i * 0.01)
csv('LOG001', 'Resistance', 'Ohm', '40k', 7000, 500, lambda i: 4700 + 300 * math.sin(i / 300), lambda i: 'GAP' if i == 3000 else '')
csv('LOG002', 'Temperature', 'degC', 'AUTO', 70000, 1000, lambda i: 25 + 10 * math.sin(i / 2000) + (i % 7) * 0.01)
csv('LOG003', 'AC V', 'V', 'AUTO', 0, 250, lambda i: 0)
csv('LOG004', 'DC A', 'A', '10A', 40, 250, lambda i: None)
csv('LOG005', 'DC V', 'V', '6V', 600, 100, lambda i: None if 200 <= i < 260 else 0.004 * (i % 100) - 0.2,
    lambda i: 'HOLD' if i % 50 == 0 else '')
csv('LOG99999', 'Capacitance', 'F', 'AUTO', 120, 1000, lambda i: 4.7e-6 * (1 + 0.001 * i))
csv('LOG006', 'Frequency', 'Hz', 'AUTO', 3, 0, lambda i: 50.0)  # zero time span

# Small files for the 99-file disk.
for i in range(60):
    bmp24('S1%02d' % i, 96, 64, scope_like(96, 64, 20 + i))
for i in range(39):
    csv('LOG1%02d' % i, 'DC V', 'V', 'AUTO', 30 + i, 250, lambda k, i=i: math.sin(k / 3 + i))

sets('e0', [])
sets('e1', ['S000.BMP'])
sets('e6', ['S003.BMP', 'LOG000.CSV', 'S002.BMP', 'LOG001.CSV', 'S000.BMP', 'LOG005.CSV'])
sets('e7', ['S012.BMP', 'LOG003.CSV', 'SWWWWWWW.BMP', 'LOG99999.CSV', 'S013.BMP', 'LOG004.CSV', 'S001.BMP'])
sets('e99', ['S1%02d.BMP' % i for i in range(60)] + ['LOG1%02d.CSV' % i for i in range(39)])
sets('fmt', ['S000.BMP', 'S002.BMP', 'S003.BMP', 'S004.BMP', 'S005.BMP', 'S006.BMP', 'S007.BMP', 'S008.BMP',
             'S009.BMP', 'S010.BMP', 'S011.BMP', 'S012.BMP', 'S013.BMP', 'LOG000.CSV', 'LOG001.CSV', 'LOG002.CSV',
             'LOG003.CSV', 'LOG004.CSV', 'LOG005.CSV', 'LOG006.CSV', 'LOG99999.CSV'])
print('generated in', OUT)
