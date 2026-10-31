#!/usr/bin/env python3
"""Touch accuracy check on the meter: the owner taps known points, the
script reads the touch log (DBG:TOUCH?, src/touch.rs) after each one and
prints where the panel saw the tap against where it was aimed.

    touch-tap-check.py                  all steps (points, then buttons)
    touch-tap-check.py --only points    only the corner / centre points
    touch-tap-check.py --out DIR        raw replies saved here (default
                                        ./touch-tap-<time>/)

Needs the meter on USB (the Linux usbtmc driver, tools/dm40dbg.py); no
firmware change: DBG:TOUCH? logs every raw frame point, the debounced
events and the gesture decision. Per step: the log is cleared, the owner
taps (aim at the marked point; one tap, lift), presses Enter, the log is
read and summarised (touch-log.py's summary with the target rect).

Points: before them lock the touch with a double click of the power key
(the lock blocks the app, not the touch log), so a tap changes nothing;
unlock afterwards. Buttons: unlocked, DMM page, bar graph off.

The table at the end: per step the aimed point, the debounced down point,
the offset (down - aim) and the gesture result (T tap, N1xx moved xx px,
L long press, < > ^ v swipes; touch-log.py). A constant offset over all
points is a calibration shift (src/touch.rs OFFSET is 0,0: the controller's
coordinates are used as they come); offsets that grow towards the edges
are a scale error; N1xx on a still finger is jitter past the tap slop.
"""
import argparse
import os
import sys
import time

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import importlib.util  # noqa: E402

spec = importlib.util.spec_from_file_location('touchlog', os.path.join(HERE, 'touch-log.py'))
tl = importlib.util.module_from_spec(spec)
spec.loader.exec_module(tl)

# (name, aim x, aim y, target rect for the IN / MISS check)
POINTS = [
    ('top-left corner', 20, 20),
    ('top-right corner', 459, 20),
    ('bottom-left corner', 20, 299),
    ('bottom-right corner', 459, 299),
    ('centre', 240, 160),
]
# DMM page buttons (src/app.rs tap): REL twice (on, off), the history row.
BUTTONS = [
    ('REL button (on)', 440, 224, (402, 205, 479, 240)),
    ('REL button (off)', 440, 224, (402, 205, 479, 240)),
    ('history row, left', 60, 162, (0, 153, 399, 171)),
    ('history row, right', 340, 162, (0, 153, 399, 171)),
]


def down_point(rows):
    """First debounced touch-down (x, y) and the gesture codes after it."""
    down, res = None, []
    for p in rows:
        if p[1] == 'E' and int(p[2]) > 0 and down is None:
            down = (int(p[4]), int(p[5]))
        elif p[1] == 'G' and down is not None and p[2] not in 'Mc':
            res.append(p[2] + (p[3] if p[2] == 'N' else ''))
    return down, ' '.join(res) or '-'


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument('--only', choices=['points', 'buttons'])
    ap.add_argument('--out')
    a = ap.parse_args()
    import dm40dbg
    ln = dm40dbg.Link()
    out = a.out or 'touch-tap-%s' % time.strftime('%Y%m%d-%H%M%S')
    os.makedirs(out, exist_ok=True)
    steps = []
    if a.only != 'buttons':
        input('Lock the touch: double click the power key. Enter when locked. ')
        steps += [(n, x, y, (x - 12, y - 12, x + 12, y + 12)) for n, x, y in POINTS]
    if a.only != 'points':
        steps.append(None)
        steps += BUTTONS
    table = []
    for k, st in enumerate(steps):
        if st is None:
            if a.only != 'buttons':
                input('Unlock the touch: double click the power key. Enter when unlocked. ')
            input('DMM page, bar graph off. Enter. ')
            continue
        name, x, y, rect = st
        ln.write('DBG:TOUCH 0')
        input('[%d] tap the %s (x %d, y %d), lift, then Enter: ' % (k, name, x, y))
        text = tl.fetch()
        with open(os.path.join(out, '%02d.txt' % k), 'w') as f:
            f.write(text)
        _, rows = tl.parse(text)
        tl.summarise(rows, rect)
        d, res = down_point(rows)
        table.append((name, x, y, d, res))
    print('\n%-22s %9s %9s %9s  %s' % ('step', 'aim', 'down', 'offset', 'result'))
    for name, x, y, d, res in table:
        if d:
            print('%-22s %4d,%-4d %4d,%-4d %+4d,%+-4d  %s' % (name, x, y, d[0], d[1], d[0] - x, d[1] - y, res))
        else:
            print('%-22s %4d,%-4d %9s %9s  %s' % (name, x, y, 'LOST', '', res))
    print('raw replies in %s/' % out)


if __name__ == '__main__':
    main()
