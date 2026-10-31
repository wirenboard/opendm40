#!/usr/bin/env python3
"""Generate src/stockfont_data.rs: the stock DM40 bitmap fonts embedded in our
own image, so stockfont.rs no longer reads them from the stock app's absolute
flash addresses (which disappear once opendm40 is flashed over 0x08004000).

    /usr/bin/python3 -I tools/mkstockfont.py [dm40_v_2_6.dec.bin]

Default source: ../dm40_v_2_6.dec.bin (the decoded stock update file: a 12-byte
header + the body that is flashed at 0x08004000). A 1 MB flash view is
synthesised as 0xFF*0x4000 + body so an absolute address A maps to offset
A-0x08000000.

The parse mirrors src/stockfont.rs exactly (it used to read these same
structures live from flash): font object F = { D*, vtable*, u16 line_h,
i16 descent }; D = { u8 *bits, entry *tab, u32 n, u8 fmt }; class A entry (12 B)
= { u32 code, u32 off, i8 yoff, u8 w, u8 h, u8 adv }; class B entry (8 B)
= { u32 code, u32 off<<8|w }, h = line_h, adv = w, yoff = 0; fmt 4 = 4 bpp else
1 bpp; bitmap is a column-major MSB-first bit stream.

Emitted storage (docs/fonts.md; the drawn pixels stay the stock ones, checked
here by decoding every glyph back and by tools/font-matrix): glyphs from
U+2E80 up (CJK, full-width) are dropped; a 4 bpp glyph becomes Huffman-coded
row tokens (huf_build / huf_encode, tables HUF_CNT / HUF_SYM shared by all 4 bpp
fonts), a 1 bpp glyph keeps only its ink rows (crop_glyph);
one blob per font as a named array in section .rodata.fonts.<NAME>_B, 10-byte
entries E(code, off, w, h, adv, yoff, t, ih) in .rodata.fonts.<NAME>_E.
"""
import collections
import heapq
import os
import struct
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
FW = os.path.dirname(HERE)
ROOT = os.path.dirname(FW)

BASE = 0x08000000
VT_A, VT_B, VT_BPLUS, VT_COMP = 0x08023509, 0x080236F5, 0x08023425, 0x080237A5

# name -> font-object address, as in src/stockfont.rs. Order = emit order.
FONTS = [
    ('F78', 0x08065070),
    ('UNIT', 0x0805ECB8),
    ('LETTERS', 0x0805DAB4),
    ('F21', 0x080667F8),
    ('F14', 0x0805B138),
    ('HIST', 0x0806C3E8),
    ('ASCII26', 0x080592F0),
    ('AUX_TAG', 0x0805965C),
    ('MODE', 0x08059D00),
    ('BTN_GLYPH', 0x0805A9FC),
    ('BATTERY', 0x0804FA4C),
    ('LOCK', 0x0806BF24),
    ('SAVE_BT', 0x0805B2E8),
    ('ON_TAG', 0x0806C724),
    ('LIGHTNING', 0x0805A0E0),
    ('TEXT22', 0x0806B13C),
    # Fonts constructed inline by address in src/ui.rs (kept address-keyed).
    ('G_08059428', 0x08059428),  # 42x42 box glyph
    ('K_FONT', 0x0805C5DC),      # keypad font
    ('ICONS', 0x0805592C),       # menu icons
]


# Fonts whose bitmap blob is a named static in its own section
# `.rodata.a2.<NAME>`, which link-native.x places in region A2 (zero-wait
# flash between the shim and the loader marker, 0x0800460c..0x08008000;
# docs/native-boot.md "Region A2"). The other blobs are named statics in
# `.rodata.fonts.<NAME>_B`, region B. A2 holds ~14.8 KB; build.sh warns below 2 KiB free. ASCII26 is
# the logger panel's font (read per draw while the AD2 stream runs; device
# c50425f showed it in slow flash stalling the ISR), F14 the small texts;
# LETTERS and UNIT since region B audit #4 (2026-10-25: B at 15 KB free, A2
# at 7.9 KB after the font rework; A2 keeps ~4 KB for the shim); LOCK
# (255 B) with the 60 ohm range. SC_PROBE (1 242 B) was here from region C
# audit #2 to region B audit #6 (2026-10-27: back in B, A2 above its 2 KiB
# warning again).
A2_FONTS = ('ASCII26', 'F14', 'LETTERS', 'UNIT', 'LOCK')


# Codes from here up (CJK, full-width punctuation) are dropped: opendm40 draws
# English text only (no string in src/ has one; docs/fonts.md).
CJK_FROM = 0x2E80
# Widest glyph (F78 'O' = 64): the row buffer of stockfont.rs (MAXW there).
MAXW = 64


def pixels(data, w, h, four):
    """Coverage 0..15 per pixel of a stock bitmap, column-major (x * h + y)."""
    if four:
        return [(data[p >> 1] >> 4) if p % 2 == 0 else (data[p >> 1] & 15) for p in range(w * h)]
    return [15 if data[p >> 3] & (0x80 >> (p & 7)) else 0 for p in range(w * h)]


# ---- 4 bpp glyphs: row tokens, static canonical Huffman (docs/fonts.md) ----
#
# Each row of a glyph is a sequence of tokens that never crosses the row end:
#   L(v)    one pixel of coverage v = 1..14;
#   F0(n), F15(n)  n pixels of 0 / 15;
#   C(E)    as a row's first token only: the whole row is the row above again.
# A run length n is a class (CLS_LO / CLS_EB: 1..6, 7-8, 9-12, 13-20, 21-36,
# 37-68) plus CLS_EB extra bits, or class EOL = up to the end of the row.
# One byte per symbol: kind << 4 | v (L) or class (runs). Every token is one
# Huffman code from the table of its context, then the extra bits, MSB first;
# a glyph starts at a byte boundary. Context: the previous token's kind in
# the row (S = row start, L, F0, F15). docs/fonts.md has the measured
# alternatives (contexts by the pixel above with partial row copies).
# The encoder parses each row optimally (dynamic programming on the code
# lengths) and alternates parse and Huffman construction a few times; the
# tables are shared by all 4 bpp fonts.
CLS_LO = (1, 2, 3, 4, 5, 6, 7, 9, 13, 21, 37)
CLS_EB = (0, 0, 0, 0, 0, 0, 1, 2, 3, 4, 5)
EOL = 11
K_L, K_F0, K_F15, K_C = 0, 1, 2, 3
HUF_ITERS = 6
NCTX = 4


def cls_of(n):
    assert 1 <= n <= CLS_LO[-1] + (1 << CLS_EB[-1]) - 1, n
    i = len(CLS_LO) - 1
    while CLS_LO[i] > n:
        i -= 1
    return i


def huf_parse_row(r, p, cost):
    """Cheapest token list of row r (p = the row above or None) under
    cost(ctx, sym): [(ctx, sym, nextra, extra, n)]. ctx = the previous
    token's kind in the row: 0 row start, 1 + K_L / K_F0 / K_F15."""
    w = len(r)
    run = [0] * (w + 1)
    for x in range(w - 1, -1, -1):
        run[x] = 1 + (run[x + 1] if x + 1 < w and r[x + 1] == r[x] else 0)
    cp = [0] * (w + 1)
    if p is not None:
        for x in range(w - 1, -1, -1):
            cp[x] = 1 + cp[x + 1] if r[x] == p[x] else 0
    best = [dict() for _ in range(w + 1)]
    best[0][0] = (0, None)
    for x in range(w):
        v = r[x]
        for pk, (c0, _) in sorted(best[x].items()):
            cx = pk
            cands = []
            if 1 <= v <= 14:
                cands.append((K_L << 4 | v, 0, 0, 1, 1 + K_L))
            runs = []
            if v == 0:
                runs.append((K_F0, run[x]))
            if v == 15:
                runs.append((K_F15, run[x]))
            if x == 0 and p is not None and cp[0] == w:
                cands.append((K_C << 4 | EOL, 0, 0, w, 1 + K_C))
            for kind, m in runs:
                for n in range(1, min(m, 68) + 1):
                    if x + n == w:
                        cands.append((kind << 4 | EOL, 0, 0, n, 1 + kind))
                    c = cls_of(n)
                    cands.append((kind << 4 | c, CLS_EB[c], n - CLS_LO[c], n, 1 + kind))
            for s, ne, ev, n, k in cands:
                c = c0 + cost(cx, s) + ne
                nb = best[x + n]
                if k not in nb or c < nb[k][0]:
                    nb[k] = (c, (x, pk, cx, s, ne, ev, n))
    k = min(sorted(best[w]), key=lambda kk: best[w][kk][0])
    out = []
    x = w
    while x > 0:
        _, (px, pk, cx, s, ne, ev, n) = best[x][k]
        out.append((cx, s, ne, ev, n))
        x, k = px, pk
    return out[::-1]


def huf_lengths(freq):
    """Huffman code lengths {sym: len} of one context (deterministic ties)."""
    items = sorted(freq.items())
    if len(items) == 1:
        return {items[0][0]: 1}
    heap = [(f, i, [s]) for i, (s, f) in enumerate(items)]
    heapq.heapify(heap)
    L = {s: 0 for s, _ in items}
    cnt = len(heap)
    while len(heap) > 1:
        f1, _, a = heapq.heappop(heap)
        f2, _, b = heapq.heappop(heap)
        for s in a + b:
            L[s] += 1
        heapq.heappush(heap, (f1 + f2, cnt, a + b))
        cnt += 1
    return L


def huf_canon(lens):
    """Canonical codes {sym: (code, len)}: by length, then symbol byte."""
    code, prev, out = 0, 0, {}
    for s in sorted(lens, key=lambda s: (lens[s], s)):
        code <<= lens[s] - prev
        prev = lens[s]
        out[s] = (code, prev)
        code += 1
    return out


def huf_build(glyphs):
    """glyphs: [(w, h, px column-major)] -> (token lists, per-context
    {sym: len}). Alternates the optimal parse and the code lengths."""
    rows = [[[px[x * h + y] for x in range(w)] for y in range(h)] for w, h, px in glyphs]
    lens = None
    for _ in range(HUF_ITERS):
        if lens is None:
            cost = lambda cx, s: 1 if (s >> 4) in (K_F0, K_C) else 4
        else:
            cost = lambda cx, s: lens[cx].get(s, 20)
        toks = [[t for y, r in enumerate(g) for t in huf_parse_row(r, g[y - 1] if y else None, cost)] for g in rows]
        freq = [collections.Counter() for _ in range(NCTX)]
        for gt in toks:
            for cx, s, _, _, _ in gt:
                freq[cx][s] += 1
        lens = [huf_lengths(f) if f else {} for f in freq]
    return toks, lens


def huf_encode(toks, codes):
    bits = []
    for cx, s, ne, ev, _ in toks:
        c, n = codes[cx][s]
        bits += [(c >> (n - 1 - i)) & 1 for i in range(n)]
        bits += [(ev >> (ne - 1 - i)) & 1 for i in range(ne)]
    out = bytearray((len(bits) + 7) // 8)
    for i, b in enumerate(bits):
        out[i >> 3] |= b << (7 - (i & 7))
    return bytes(out), len(bits)


def huf_decode(b, w, h, cnt, syms):
    """Mirror of stockfont.rs `Glyph::token` (canonical decode from the
    per-length counts): the glyph's pixels, column-major."""
    pos = [0]

    def bit():
        v = (b[pos[0] >> 3] >> (7 - (pos[0] & 7))) & 1 if (pos[0] >> 3) < len(b) else 0
        pos[0] += 1
        return v

    rows = []
    for y in range(h):
        p = rows[-1] if rows else None
        r = []
        pk = 0
        while len(r) < w:
            x = len(r)
            cx = pk
            code = first = idx = 0
            for l in range(len(cnt[cx])):
                code |= bit()
                c = cnt[cx][l]
                if code - first < c:
                    s = syms[cx][idx + code - first]
                    break
                idx += c
                first = (first + c) << 1
                code <<= 1
            else:
                raise AssertionError('bad code')
            kind, c = s >> 4, s & 15
            if kind == K_L:
                r.append(c)
            else:
                if c == EOL:
                    n = w - x
                else:
                    e = 0
                    for _ in range(CLS_EB[c]):
                        e = e << 1 | bit()
                    n = CLS_LO[c] + e
                assert x + n <= w and (kind != K_C or (x == 0 and c == EOL))
                if kind == K_C:
                    r += p
                else:
                    r += [0 if kind == K_F0 else 15] * n
            pk = 1 + kind
        rows.append(r)
    return [rows[y][x] for x in range(w) for y in range(h)], pos[0]


def crop_glyph(px, w, h):
    """1 bpp glyph -> its ink rows t .. t + ih only (same column-major
    MSB-first bit stream, ih rows per column). An inkless glyph: ih = 0."""
    ink = [y for y in range(h) if any(px[x * h + y] for x in range(w))]
    if not ink:
        return b'', 0, 0
    t, ih = ink[0], ink[-1] - ink[0] + 1
    bits = [px[x * h + y] for x in range(w) for y in range(t, t + ih)]
    out = bytearray((len(bits) + 7) // 8)
    for p, v in enumerate(bits):
        if v:
            out[p >> 3] |= 0x80 >> (p & 7)
    return bytes(out), t, ih


def uncrop_glyph(b, w, h, t, ih):
    return [15 if t <= y < t + ih and b[(x * ih + y - t) >> 3] & (0x80 >> ((x * ih + y - t) & 7)) else 0
            for x in range(w) for y in range(h)]


def main():
    src = sys.argv[1] if len(sys.argv) > 1 else os.path.join(ROOT, 'dm40_v_2_6.dec.bin')
    body = open(src, 'rb').read()[12:]
    img = b'\xff' * 0x4000 + body

    def w32(a):
        return struct.unpack_from('<I', img, a - BASE)[0]

    def u16(a):
        return struct.unpack_from('<H', img, a - BASE)[0]

    def i16(a):
        return struct.unpack_from('<h', img, a - BASE)[0]

    def i8(a):
        return struct.unpack_from('<b', img, a - BASE)[0]

    def u8(a):
        return img[a - BASE]

    out = []
    out.append('// @generated by tools/mkstockfont.py from dm40_v_2_6 - do not edit by hand.')
    out.append('// The stock bitmap fonts, embedded so stockfont.rs needs no stock app in flash.')
    out.append('')

    def load(a):
        """Glyphs of the font object at `a`: (line_h, descent, four, [(code, w, h, adv, yoff, bytes)])."""
        D, vt = w32(a), w32(a + 4)
        f0 = w32(vt)
        if f0 not in (VT_A, VT_B, VT_BPLUS):
            raise SystemExit('@%08x: unexpected vtable %08x (composite/unknown not supported)' % (a, f0))
        class_a = (f0 == VT_A)
        bits, tab, n, fmt = w32(D), w32(D + 4), w32(D + 8), u8(D + 12)
        line_h, descent = u16(a + 8), i16(a + 10)
        four = (fmt == 4)
        bpp = 4 if four else 1
        gl = []
        prev_code = None
        for i in range(n):
            if class_a:
                code = w32(tab + 12 * i)
                off = w32(tab + 12 * i + 4)
                yoff = i8(tab + 12 * i + 8)
                gw = u8(tab + 12 * i + 9)
                gh = u8(tab + 12 * i + 10)
                adv = u8(tab + 12 * i + 11)
            else:
                code = w32(tab + 8 * i)
                v = w32(tab + 8 * i + 4)
                off = v >> 8
                gw = v & 0xFF
                gh = line_h
                adv = gw
                yoff = 0
            if prev_code is not None and code <= prev_code:
                raise SystemExit('@%08x: glyph table not strictly ascending at code %x' % (a, code))
            prev_code = code
            span = (gw * gh * bpp + 7) // 8
            abs_bits = bits + off
            data = img[abs_bits - BASE: abs_bits - BASE + span]
            if len(data) != span:
                raise SystemExit('@%08x code %x: bitmap out of image' % (a, code))
            gl.append((code, gw, gh, adv, yoff, bytes(data)))
        return line_h, descent, four, gl, ('A' if class_a else 'B')

    def load_class_c():
        """Class C font 0x08054828 (vtable 0x08054820, FUN_0802362c/FUN_080235bc):
        fixed 1 bpp bitmaps, ' (4x16) at 0x08054728, R U N S T O P W A I D H
        (10x16, 20 bytes each) from 0x08054730; line_h 16, descent 0."""
        gl = []
        for i, ch in enumerate("'RUNSTOPWAIDH"):
            if i == 0:
                a, w = 0x08054728, 4
            else:
                a, w = 0x08054730 + 20 * (i - 1), 10
            gl.append((ord(ch), w, 16, w, 0, img[a - BASE: a - BASE + (w * 16 + 7) // 8]))
        gl.sort()
        return 16, 0, False, gl, 'C'

    def load_merged(first, second):
        """B+ font: every code of `first` (0x0805471c) and, for the codes it
        lacks, the font's own table (`second`), as FUN_08023425 looks them up.
        Only codes < 0x80 of the own table (the CJK part is not used)."""
        lh, de, four, g1, _ = load(first)
        lh2, de2, four2, g2, _ = load(second)
        assert (lh, four) == (lh2, four2)
        have = set(g[0] for g in g1)
        gl = sorted(g1 + [g for g in g2 if g[0] < 0x80 and g[0] not in have])
        return lh, de, four, gl, 'B+'

    total_blob = 0
    total_glyphs = 0
    dropped = [0, 0]
    raw_total = [0]
    summary = []
    fonts = [(name, a, (lambda a=a: load(a))) for name, a in FONTS]
    fonts += [(name, key, src) for name, key, src in [
        # Stock scope page fonts (stock/re/docs/stock-scope-ui.md sec. 0.1), keyed by
        # the font-object address the stock page uses.
        ('SC_C', 0x08054828, load_class_c),                                  # status letters, "H"
        ('SC_F12', 0x0804F24C, lambda: load_merged(0x0805471C, 0x0804F24C)),  # 12 px texts
        ('SC_F20', 0x08054148, lambda: load(0x08054148)),                     # panel buttons
        ('SC_PROBE', 0x08054FC0, lambda: load(0x08054FC0)),                   # coupling / probe glyph
        ('SC_EDGE', 0x08055014, lambda: load(0x08055014)),                    # trigger edge
        ('SC_MARK', 0x08065558, lambda: load(0x08065558)),                    # 12x12 markers
        ('SC_BATT', 0x0806BD44, lambda: load(0x0806BD44)),                    # battery
        ('SC_F16', 0x0804F260, lambda: load(0x080534C4)),                     # 16 px popup ASCII (wrapper)
    ]]
    loaded = [(name, a, src()) for name, a, src in fonts]
    # The 4 bpp glyphs of every font, in emit order: one token parse and one
    # set of Huffman tables for all of them.
    four_gl = [(gw, gh, pixels(data, gw, gh, True)) for _, _, (_, _, four, gl, _) in loaded if four
               for (code, gw, gh, adv, yoff, data) in gl if code < CJK_FROM]
    toks, lens = huf_build(four_gl)
    codes = [huf_canon(ls) for ls in lens]
    maxl = max(max(ls.values()) for ls in lens if ls)
    # Every token (code + extra bits) fits the decoder's 25-bit window.
    assert maxl + max(CLS_EB) <= 25, maxl
    cnt = [[sum(1 for s in ls if ls[s] == l + 1) for l in range(maxl)] for ls in lens]
    syms = [sorted(ls, key=lambda s: (ls[s], s)) for ls in lens]
    four_toks = iter(toks)
    ntok = sum(len(gt) for gt in toks)
    short = [sum(1 for gt in toks for cx, s, _, _, _ in gt if lens[cx][s] <= k) * 100.0 / ntok for k in range(1, maxl + 1)]
    huf_bytes = sum(len(c) + len(s) for c, s in zip(cnt, syms))
    for name, a, (line_h, descent, four, gl, cls) in loaded:
        bpp = 4 if four else 1
        blob = bytearray()
        entries = []
        for (code, gw, gh, adv, yoff, data) in gl:
            if code >= CJK_FROM:
                dropped[0] += 1
                dropped[1] += len(data)
                continue
            px = pixels(data, gw, gh, four)
            if four:
                enc, nbits = huf_encode(next(four_toks), codes)
                # Cur keeps glyph-relative bit positions in a u16.
                assert nbits < 0x10000, (name, code)
                dec, used = huf_decode(enc, gw, gh, cnt, syms)
                assert dec == px and used == nbits, (name, code)
                t, ih = 0, gh
            else:
                enc, t, ih = crop_glyph(px, gw, gh)
                assert uncrop_glyph(enc, gw, gh, t, ih) == px, (name, code)
            assert len(blob) < 0x10000 and code < 0x10000 and gw <= MAXW, (name, code)
            entries.append((code, len(blob), gw, gh, adv, yoff, t, ih))
            blob += enc
            raw_total[0] += len(data)
        n = len(entries)

        total_blob += len(blob)
        total_glyphs += n
        summary.append((name, cls, bpp, line_h, descent, n, len(blob)))

        # Emit the glyph entry table.
        out.append('#[link_section = ".rodata.fonts.%s_E"]' % name)
        out.append('pub static %s_E: [Entry; %d] = [' % (name, n))
        line = '    '
        for (code, noff, gw, gh, adv, yoff, t, ih) in entries:
            cell = 'E(0x%X,%d,%d,%d,%d,%d,%d,%d),' % (code, noff, gw, gh, adv, yoff, t, ih)
            if len(line) + len(cell) > 116:
                out.append(line)
                line = '    '
            line += cell
        if line.strip():
            out.append(line)
        out.append('];')
        # Emit the bitmap blob as a wrapped byte-string literal, a named
        # [u8; N] in its own section: A2 fonts `.rodata.a2.<NAME>`
        # (link-native.x routes it to region A2), the others
        # `.rodata.fonts.<NAME>_B` (region B).
        if name in A2_FONTS and len(blob) > 0:
            out.append('#[link_section = ".rodata.a2.%s"]' % name)
        else:
            out.append('#[link_section = ".rodata.fonts.%s_B"]' % name)
        out.append('pub static %s_B: [u8; %d] = *b"\\' % (name, len(blob)))
        esc = ''.join('\\x%02x' % b for b in blob)
        for j in range(0, len(esc), 128):
            cont = '\\' if j + 128 < len(esc) else '"'
            out.append(esc[j:j + 128] + cont)
        if len(esc) == 0:
            out[-1] = 'pub static %s_B: [u8; 0] = *b"";' % name
        else:
            out.append(';')
        # Emit the FontData.
        out.append('pub static %s_FD: FontData = FontData{line_h:%d,descent:%d,fmt:%s,entries:&%s_E,blob:&%s_B};'
                   % (name, line_h, descent, 'HUF' if four else 'BIT1', name, name))
        out.append('')

    # The Huffman tables of the 4 bpp glyphs (docs/fonts.md): per context the
    # number of codes of each length 1..HUF_MAXL and the symbols in code order.
    out.append('/// Longest Huffman code, bits.')
    out.append('pub const HUF_MAXL: usize = %d;' % maxl)
    cc = codes[0].get(K_C << 4 | EOL)
    out.append('/// Code of C(E) (the row above again) at a row start, in its')
    out.append('/// HUF_COPY_LEN low bits (0: none).')
    out.append('pub const HUF_COPY_CODE: u32 = %d;' % (cc[0] if cc else 0))
    out.append('pub const HUF_COPY_LEN: u32 = %d;' % (cc[1] if cc else 0))
    out.append('/// Codes per length 1..HUF_MAXL, per context.')
    out.append('#[link_section = ".rodata.fonts.HUF_CNT"]')
    out.append('pub static HUF_CNT: [[u8; HUF_MAXL]; %d] = [' % len(cnt))
    for c in cnt:
        out.append('    [%s],' % ', '.join(str(v) for v in c))
    out.append('];')
    soff = [0]
    for sy in syms:
        soff.append(soff[-1] + len(sy))
    out.append('/// First symbol of each context in HUF_SYM.')
    out.append('#[link_section = ".rodata.fonts.HUF_SOFF"]')
    out.append('pub static HUF_SOFF: [u16; %d] = [%s];' % (len(soff), ', '.join(str(v) for v in soff)))
    out.append('/// Symbols (kind << 4 | coverage or length class), canonical code order.')
    out.append('#[link_section = ".rodata.fonts.HUF_SYM"]')
    out.append('pub static HUF_SYM: [u8; %d] = [' % soff[-1])
    for sy in syms:
        if sy:
            out.append('    %s,' % ', '.join('0x%02X' % v for v in sy))
    out.append('];')
    out.append('')

    # Address -> embedded FontData dispatch, so stockfont::Font can stay keyed
    # by the original stock flash address (no call site changes).
    out.append('/// Resolve a stock font-object address to its embedded data.')
    out.append('pub fn font_data(addr: u32) -> Option<&\'static FontData> {')
    out.append('    match addr {')
    for name, a, _ in fonts:
        out.append('        0x%08X => Some(&%s_FD),' % (a, name))
    out.append('        _ => None,')
    out.append('    }')
    out.append('}')
    out.append('')

    open(os.path.join(FW, 'src', 'stockfont_data.rs'), 'w').write('\n'.join(out) + '\n')

    # Self-check report.
    print('fonts: %d  glyphs: %d  blob bytes: %d (raw %d; dropped %d CJK glyphs, %d raw bytes)'
          % (len(fonts), total_glyphs, total_blob, raw_total[0], dropped[0], dropped[1]))
    print('4 bpp: %d contexts, max code %d bits, tables %d bytes (counts + symbols + offsets)'
          % (len(cnt), maxl, huf_bytes + 2 * len(soff)))
    print('  %d tokens; share with a code of <= k bits: %s' % (ntok, ' '.join('%d:%.1f%%' % (k + 1, v) for k, v in enumerate(short))))
    for (name, cls, bpp, lh, desc, n, nb) in summary:
        print('  %-9s class %-2s %dbpp line_h=%-3d descent=%-2d glyphs=%-4d blob=%d'
              % (name, cls, bpp, lh, desc, n, nb))


if __name__ == '__main__':
    main()
