#!/usr/bin/env python3
"""Replay an ILI9488 write trace (the model's `sysbus.lcd Trace PATH`) and
measure screen transitions: what the panel shows while a screen is drawn.

    emu/lcdtrace.py TRACE --event T[:NAME] ... [--out DIR] [--json FILE]

For each event (virtual time T, e.g. a tap) the transition is the burst of
LCD writes that starts after T and ends before a write gap of --gap ms (or
--window s after T). Reported per transition:

  ms         first to last write of the burst (emulator time; at
             `run.py --mips 110` near the device's CPU speed, inferred;
             the FSMC bus time per pixel is not modelled)
  frames     panel refreshes (--hz, default 60) inside the burst
  transient  the largest share of the screen that shows neither the old
             nor the new picture for half a refresh or longer at a moment
             of the burst (sampled every 1/600 s), e.g. a background
             cleared before its text, a popup closed and opened again
  peak       the same without the half-refresh hold (a box filled 1 ms
             before its label counts)
  overdraw   pixels written / pixels that changed

--out DIR writes a filmstrip per transition (the screen at each refresh,
the transient pixels tinted magenta in a second row) and a contact sheet.
Only opendm40's landscape MADCTL (0xE8: column = x, page = y) is replayed.
"""
import argparse
import json
import os
import struct
import sys

import numpy as np

W, H = 480, 320
REC = struct.Struct('<cddHHHHBI')
MINOR = 32  # pixels
PX_S = 50e-9
STEP = 1 / 600  # s: transient sampling (10 per 60 Hz refresh)
HOLD = 5  # samples: half a refresh  # upper bound of one pixel write in the emulator


def load(path):
    """Records (t0, t1, xs, xe, ys, ye, runs as (counts, values) arrays)."""
    out = []
    with open(path, 'rb') as f:
        data = f.read()
    o = 0
    n = len(data)
    while o + REC.size <= n:
        tag, t0, t1, xs, xe, ys, ye, mad, nr = REC.unpack_from(data, o)
        if tag != b'W':
            raise SystemExit('bad trace record at %d' % o)
        o += REC.size
        if o + nr * 6 > n:
            break  # truncated tail (run still writing)
        a = np.frombuffer(data, dtype=np.dtype([('n', '<u4'), ('v', '<u2')]), count=nr, offset=o)
        o += nr * 6
        cnt = a['n'].astype(np.int64)
        # t1 is the next command, which may come much later (the controller
        # stays in RAMWR): bound it by the burst's own length.
        t1 = min(t1, t0 + 2e-5 + int(cnt.sum()) * PX_S)
        out.append((t0, t1, xs, xe, ys, ye, cnt, a['v']))
    return out


class Screen:
    def __init__(self):
        self.px = np.zeros((H, W), dtype=np.uint16)
        self.writes = np.zeros((H, W), dtype=np.int32)

    def apply(self, rec):
        _, _, xs, xe, ys, ye, cnt, val = rec
        if xe < xs or ye < ys or not len(cnt):
            return
        w, h = xe - xs + 1, ye - ys + 1
        total = int(cnt.sum())
        flat = np.repeat(val, cnt)
        # The controller wraps to the window's top after its last pixel.
        vals = np.zeros(w * h, dtype=np.uint16)
        m = np.zeros(w * h, dtype=bool)
        for st in range(0, total, w * h):
            chunk = flat[st:st + w * h]
            vals[:len(chunk)] = chunk
            m[:len(chunk)] = True
        vals = vals.reshape(h, w)
        m = m.reshape(h, w)
        # Clip to the screen.
        x1, y1 = min(xe + 1, W), min(ye + 1, H)
        if xs >= W or ys >= H:
            return
        vv = vals[: y1 - ys, : x1 - xs]
        mm = m[: y1 - ys, : x1 - xs]
        reg = self.px[ys:y1, xs:x1]
        reg[mm] = vv[mm]
        self.writes[ys:y1, xs:x1] += mm


def rgb(px):
    r = ((px >> 11) & 0x1F).astype(np.uint8)
    g = ((px >> 5) & 0x3F).astype(np.uint8)
    b = (px & 0x1F).astype(np.uint8)
    return np.dstack([(r << 3) | (r >> 2), (g << 2) | (g >> 4), (b << 3) | (b >> 2)])


def bursts(recs, i, lim, gap):
    """Bursts of writes from record i on, starting before lim: [(first, last)]."""
    out = []
    n = len(recs)
    while i < n and recs[i][0] < lim:
        if recs[i][6].sum() < MINOR:
            i += 1
            continue
        j = last = i
        while j + 1 < n and recs[j + 1][0] - recs[last][1] < gap:
            j += 1
            if recs[j][6].sum() >= MINOR:
                last = j
        out.append((i, last))
        i = j + 1
    return out


def transitions(recs, events, gap, window):
    """Per event: (name, t_event, i_first, i_last): the burst after the event
    that writes the most pixels (the new screen; a pressed-key feedback
    before it is a burst of its own)."""
    res = []
    ts = [r[0] for r in recs]
    import bisect
    for k, (t, name) in enumerate(events):
        t_next = events[k + 1][0] if k + 1 < len(events) else float('inf')
        lim = min(t + window, t_next)
        i = bisect.bisect_left(ts, t)
        bs = bursts(recs, i, lim, gap)
        if not bs:
            res.append((name, t, None, None))
            continue
        size = lambda b: sum(int(recs[x][6].sum()) for x in range(b[0], b[1] + 1))
        i, j = max(bs, key=size)
        res.append((name, t, i, j))
    return res


def analyse(recs, events, hz=60.0, gap=0.025, window=0.6, out=None, quiet=False):
    scr = Screen()
    period = 1.0 / hz
    results = []
    k = 0  # records applied
    strips = []
    for name, t, i, j in transitions(recs, events, gap, window):
        if i is None:
            results.append({'name': name, 't': t, 'burst': False, 'ms': 0.0, 'frames': 0, 'transient': 0.0, 'overdraw': 0.0, 'px': 0})
            continue
        while k < i:
            scr.apply(recs[k])
            k += 1
        before = scr.px.copy()
        scr.writes[:] = 0
        t_start, t_end = recs[i][0], recs[j][1]
        # Panel refreshes inside the burst, phase-locked to its start.
        # The screen every STEP inside the burst (the worst refresh phase);
        # the filmstrip shows the refreshes phase-locked to its start.
        samples = []
        n = 1
        while t_start + n * STEP < t_end:
            ts = t_start + n * STEP
            while k <= j and recs[k][1] <= ts:
                scr.apply(recs[k])
                k += 1
            samples.append((ts, scr.px.copy(), n % round(period / STEP) == 0))
            n += 1
        while k <= j:
            scr.apply(recs[k])
            k += 1
        after = scr.px.copy()
        changed = int((before != after).sum())
        written = int(scr.writes.sum())
        tr_frames = []
        refreshes = 0
        bads = np.zeros((len(samples), H, W), dtype=bool)
        for i, (ts_, f, show) in enumerate(samples):
            bads[i] = (f != before) & (f != after)
            if show:
                refreshes += 1
                tr_frames.append((ts_, f, bads[i]))
        peak = max((b.sum() for b in bads), default=0) / (W * H)
        # Persistent: in a state that is neither old nor new for HOLD
        # samples in a row (half a refresh: likely on the panel), by a
        # time opening of the per-pixel masks.
        worst = 0.0
        if len(bads) >= HOLD:
            e = bads[:len(bads) - HOLD + 1].copy()
            for d in range(1, HOLD):
                e &= bads[d:len(bads) - HOLD + 1 + d]
            p = np.zeros_like(bads)
            for d in range(HOLD):
                p[d:d + len(e)] |= e
            worst = max(x.sum() for x in p) / (W * H)
        r = {
            'name': name, 't': round(t, 4), 'burst': True,
            'ms': round((t_end - t_start) * 1e3, 1),
            'latency': round((t_start - t) * 1e3, 1),
            'frames': refreshes,
            'transient': round(worst * 100, 2),
            'peak': round(peak * 100, 2),
            'overdraw': round(written / changed, 2) if changed else 0.0,
            'px': changed,
            'written': int(written),
        }
        results.append(r)
        if not quiet:
            print('%-14s t=%7.3f  +%5.1f ms: %6.1f ms  %2d refreshes  transient %5.1f %%  peak %5.1f %%  overdraw %5.2f  changed %6d px'
                  % (name, t, r['latency'], r['ms'], r['frames'], r['transient'], r['peak'], r['overdraw'], changed))
        if out:
            strips.append((name, r, before, tr_frames, after))
    if out:
        write_strips(out, strips)
    return results


def write_strips(out, strips, scale=3):
    from PIL import Image, ImageDraw
    os.makedirs(out, exist_ok=True)
    w, h = W // scale, H // scale
    rows = []
    for idx, (name, r, before, frames, after) in enumerate(strips):
        cells = [('before', before, None)] + [('+%.0f ms' % ((ts - frames[0][0]) * 1e3 + 1e3 / 60), f, bad) for ts, f, bad in frames] + [('final', after, None)]
        cells = cells[:14] if len(cells) <= 14 else cells[:13] + [cells[-1]]
        im = Image.new('RGB', (len(cells) * (w + 4) + 4, 2 * (h + 14) + 18), (255, 255, 255))
        d = ImageDraw.Draw(im)
        d.text((4, 2), '%s: %.0f ms, %d refreshes, transient %.1f %%, overdraw %.2f' % (name, r['ms'], r['frames'], r['transient'], r['overdraw']), fill=(0, 0, 0))
        for c, (lbl, f, bad) in enumerate(cells):
            x = 4 + c * (w + 4)
            a = rgb(f)
            top = Image.fromarray(a).resize((w, h), Image.BILINEAR)
            im.paste(top, (x, 18))
            d.text((x, 18 + h + 1), lbl, fill=(0, 0, 0))
            if bad is not None:
                t = a.copy()
                t[bad] = (255, 0, 255)
                im.paste(Image.fromarray(t).resize((w, h), Image.NEAREST), (x, 18 + h + 14))
        fn = os.path.join(out, '%02d-%s.png' % (idx, ''.join(ch if ch.isalnum() or ch in '-_' else '_' for ch in name)))
        im.save(fn)
        rows.append(im)
    if rows:
        sw = max(i.size[0] for i in rows)
        sheet = Image.new('RGB', (sw, sum(i.size[1] for i in rows)), (255, 255, 255))
        y = 0
        for i in rows:
            sheet.paste(i, (0, y))
            y += i.size[1]
        sheet.save(os.path.join(out, 'contact-sheet.png'))


def parse_events(specs):
    ev = []
    for s in specs:
        t, _, n = s.partition(':')
        ev.append((float(t), n or s))
    return sorted(ev)


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument('trace')
    ap.add_argument('--event', action='append', default=[], metavar='T[:NAME]')
    ap.add_argument('--hz', type=float, default=60.0, help='panel refresh rate (default 60, inferred)')
    ap.add_argument('--gap', type=float, default=25.0, help='ms without writes that ends a transition')
    ap.add_argument('--window', type=float, default=0.6, help='s after the event at most')
    ap.add_argument('--out', help='filmstrip directory')
    ap.add_argument('--json', help='results as JSON')
    a = ap.parse_args()
    recs = load(a.trace)
    res = analyse(recs, parse_events(a.event), a.hz, a.gap / 1e3, a.window, a.out)
    if a.json:
        with open(a.json, 'w') as f:
            json.dump(res, f, indent=1)


if __name__ == '__main__':
    sys.exit(main())
