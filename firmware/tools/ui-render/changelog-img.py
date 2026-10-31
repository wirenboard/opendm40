#!/usr/bin/env python3
"""Before / after pictures for a CHANGELOG section (docs/changelog/<tag>/).

    tools/ui-render/changelog-img.py v0.3.0 PREV_REF MATRIX

Before = docs/manual/img/<name>.png at PREV_REF (the previous release),
after = the same file in the working tree; MATRIX is a `run.sh OUT` folder
(2x renders) for pictures the manual has no copy of. Each picture is cropped
below the status bar and saved as a palette PNG. Needs PIL.
"""
import io
import os
import subprocess
import sys

from PIL import Image, ImageDraw, ImageFont

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.normpath(os.path.join(HERE, '../../..'))
IMG = 'docs/manual/img'
FONT = '/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf'
BOLD = '/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf'
WHITE, DARK, GREY = (255, 255, 255), (40, 40, 40), (190, 190, 190)

tag, prev, matrix = sys.argv[1:4]
DST = os.path.join(ROOT, 'docs/changelog', tag)
os.makedirs(DST, exist_ok=True)


def at(ref, name):
    b = subprocess.run(['git', '-C', ROOT, 'show', f'{ref}:{IMG}/{name}.png'], capture_output=True, check=True).stdout
    return Image.open(io.BytesIO(b)).convert('RGB')


def now(name):
    return Image.open(os.path.join(ROOT, IMG, name + '.png')).convert('RGB')


def m2(name):
    """A 2x matrix render at 480 x 320."""
    return Image.open(os.path.join(matrix, name + '.png')).convert('RGB').resize((480, 320), Image.NEAREST)


def save(im, name):
    q = im.quantize(colors=256, method=Image.Quantize.LIBIMAGEQUANT, dither=Image.Dither.NONE)
    q.save(os.path.join(DST, name + '.png'), optimize=True)
    print(name)


def pair(name, before, after, crop):
    """Before | after side by side, captioned with the two versions."""
    a, b = before.crop(crop), after.crop(crop)
    w, h = a.size
    f = ImageFont.truetype(BOLD, 16)
    im = Image.new('RGB', (2 * w + 30, h + 40), WHITE)
    d = ImageDraw.Draw(im)
    for i, (s, t) in enumerate([(a, prev), (b, tag)]):
        x = 10 + i * (w + 10)
        d.text((x, 8), t, font=f, fill=DARK)
        im.paste(s, (x, 32))
        d.rectangle((x - 1, 31, x + w, 32 + h), outline=GREY)
    save(im, name)


def single(name, src, crop):
    s = src.crop(crop)
    w, h = s.size
    im = Image.new('RGB', (w + 20, h + 20), WHITE)
    im.paste(s, (10, 10))
    ImageDraw.Draw(im).rectangle((9, 9, 10 + w, 10 + h), outline=GREY)
    save(im, name)


def side(name, parts):
    """Pictures (image, crop, caption) side by side, top-aligned."""
    cs = [im.crop(c) for im, c, _ in parts]
    f = ImageFont.truetype(BOLD, 16)
    w = sum(c.size[0] for c in cs) + 10 * (len(cs) + 1)
    h = max(c.size[1] for c in cs)
    im = Image.new('RGB', (w, h + 40), WHITE)
    d = ImageDraw.Draw(im)
    x = 10
    for c, (_, _, t) in zip(cs, parts):
        d.text((x, 8), t, font=f, fill=DARK)
        im.paste(c, (x, 32))
        d.rectangle((x - 1, 31, x + c.size[0], 32 + c.size[1]), outline=GREY)
        x += c.size[0] + 10
    save(im, name)


BODY = (0, 34, 480, 178)  # below the status bar: the reading and the bargraph
if tag == 'v0.3.0':
    pair('accuracy', at(prev, 'dmm-bar'), now('dmm-acc'), BODY)
    pair('ohm-60', at(prev, 'range-ohm'), now('range-ohm'), (0, 40, 480, 140))
    pair('tb-list', at(prev, 'scope-tb-list'), now('scope-tb-list'), (60, 20, 220, 215))
    pair('hz', at(prev, 'dmm-hz'), now('dmm-hz'), BODY)
    single('splash', m2('splash'), (90, 70, 390, 250))
    single('negative-ohm', m2('a22-acc-ohm-neg'), BODY)
elif tag == 'v0.4.0':
    side('ohm-600m', [(now('range-ohm-600m'), (0, 40, 480, 182), 'Range page'),
                      (now('dmm-ohm-600m'), (0, 0, 480, 172), 'Reading')])
    pair('record-button', at(prev, 'range-a'), now('range-a'), (0, 0, 480, 290))
    pair('cont-short', at(prev, 'dmm-cont'), now('dmm-cont'), BODY)
