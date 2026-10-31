#!/usr/bin/env python3
"""Make the screenshots of the user manual (docs/manual/img/*.png).

    tools/ui-render/run.sh OUT --manual     # manual.rs screens (all of them)
    tools/ui-render/run.sh OUT2 --readme    # README screens (readme.rs)
    tools/ui-render/run.sh OUT3             # the full matrix (main menu, inrush)
    emu/gallerytest.py --app ELF --port P --shots OUT4   # the gallery
    tools/stock-ui/run.sh OUT5               # the stock scope page (optional)
    tools/ui-render/manual-img.py OUT OUT2 OUT3 OUT4 [OUT5]

Each source PNG is scaled to 480 x 320 (the matrix renders are 2x) and
stored as an exact palette PNG (the screens have fewer than 256 colours;
otherwise it is quantised to 256). Without OUT4 (no such directory) the
gallery images stay as they are. Then manual-annot.py makes the composed
and annotated pictures. Needs PIL.
"""
import glob
import subprocess
import os
import sys

from PIL import Image

HERE = os.path.dirname(os.path.abspath(__file__))
DST = os.path.normpath(os.path.join(HERE, '../../../docs/manual/img'))

if len(sys.argv) not in (5, 6):
    sys.exit(__doc__)
man, readme, matrix, gallery = sys.argv[1:5]
# Renders that only feed manual-annot.py (crops), not manual images.
ANNOT_ONLY = {'contact-sheet', 'status-all', 'status-usb', 'status-locked', 'settings3-edit', 'dmm-temp-f', 'confirm-install'}
SRC = {os.path.basename(p)[:-4]: p for p in glob.glob(f'{man}/*.png') if os.path.basename(p)[:-4] not in ANNOT_ONLY}
SRC.update({n: f'{readme}/{n}.png' for n in ['dmm', 'auto-acdc', 'burden', 'ajack', 'combined', 'generator', 'power', 'scope']})
SRC['main-menu'] = f'{matrix}/m04-main-menu.png'
SRC['inrush'] = f'{matrix}/i02-inrush-dc.png'
SRC['gallery'] = f'{gallery}/g01-grid-p1.png'
SRC['gallery-view'] = f'{gallery}/g05-overlay.png'

os.makedirs(DST, exist_ok=True)
for name, path in sorted(SRC.items()):
    if not os.path.exists(path) and name.startswith('gallery') and os.path.exists(os.path.join(DST, name + '.png')):
        print(f'{name:15s} kept')
        continue
    im = Image.open(path).convert('RGB')
    if im.size == (960, 640):
        im = im.resize((480, 320), Image.NEAREST)
    assert im.size == (480, 320), (name, im.size)
    colours = im.getcolors(256)
    if colours is None:
        q = im.quantize(colors=256, method=Image.Quantize.MEDIANCUT)
    else:
        q = im.convert('P', palette=Image.Palette.ADAPTIVE, colors=len(colours))
    out = os.path.join(DST, name + '.png')
    q.save(out, optimize=True)
    print(f'{name:15s} {os.path.getsize(out):6d}')

subprocess.run([sys.executable, os.path.join(HERE, 'manual-annot.py'), man] + sys.argv[5:6], check=True)
