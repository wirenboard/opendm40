#!/usr/bin/env python3
"""Static worst-case stack depth of the opendm40 native image. opendm40 runs with
interrupts masked, so one call tree from the reset path covers it, plus the
one interrupt handler a build may have (ISRS below: the AD2 stream ISR of
the `ad2-isr` feature, ../docs/history/decisions-2026-10-22.md #3): it runs on the same
MSP on top of ANY main-loop frame, so its own worst path and the hardware
exception frame (26 words with the FPU's lazy-stacking space, + 4 bytes of
alignment) are added to the main worst path.

    tools/stack-depth/stackdepth.py ELF [--root SYM] [--top N] [--check BYTES]

Disassembles ELF with the toolchain's llvm-objdump, takes each function's
frame from its prologue (push / stmdb sp! / vpush / sub sp,#imm) and its
callees from bl / b.w / b to a symbol (tail calls included), and prints the
deepest call path from the root (default opendm40::native::start, the native
entry after the shim) plus the largest frames.

Limits: indirect calls (blx rN: trait objects, fn pointers) are not
followed, except the known ones in INDIRECT below (added as calls); recursion is cut at the first repeat; a dynamic `sub sp, rN` is
flagged. On 2026-10-21 the result matched the emulator's measured high-water
(DBG:BOOT? STACK) to within 16 bytes, so these do not hide the worst path
today; re-check against DBG:BOOT? after big changes.

--check BYTES: exit 1 unless (stack size - worst path) >= BYTES, where the
stack size is _stack_top - __bss_end (the TCM left after .data/.bss).
build.sh runs it with 16 KiB (device check 2026-10-21).
"""
import argparse, glob, os, re, subprocess, sys

HERE = os.path.dirname(os.path.abspath(__file__))


def tool(name):
    # build.sh's toolchain (DM40_LLVMBIN), else the repo's toolchain/, else
    # the active rustc's sysroot (llvm-tools component).
    pats = [os.path.join(HERE, '../../../toolchain/rustup/toolchains/*/lib/rustlib/*/bin/' + name)]
    if os.environ.get('DM40_LLVMBIN'):
        pats.insert(0, os.path.join(os.environ['DM40_LLVMBIN'], name))
    else:
        try:
            sysroot = subprocess.run(['rustc', '--print', 'sysroot'], capture_output=True,
                                     text=True).stdout.strip()
            if sysroot:
                pats.append(os.path.join(sysroot, 'lib/rustlib/*/bin/' + name))
        except OSError:
            pass
    for pat in pats:
        m = sorted(glob.glob(pat))
        if m:
            return m[0]
    sys.exit('stackdepth: %s not found (toolchain/ or rustc --print sysroot)' % name)


def nregs(lst):
    n = 0
    for r in lst.split(','):
        r = r.strip()
        if '-' in r:
            a, b = r.split('-')
            n += int(b[1:]) - int(a[1:]) + 1
        elif r:
            n += 1
    return n


def parse(lines):
    fn_re = re.compile(r'^([0-9a-f]{8}) <(.*)>:$')
    ins_re = re.compile(r'^\s*([0-9a-f]+):\s+(\S+)\s*(.*)$')
    funcs, cur = {}, None
    for l in lines:
        m = fn_re.match(l)
        if m:
            cur = m.group(2)
            funcs[cur] = {'frame': 0, 'calls': set(), 'ind': False, 'dyn': False}
            continue
        if cur is None:
            continue
        m = ins_re.match(l)
        if not m:
            continue
        op, args = m.group(2), m.group(3).split(' @')[0]
        f = funcs[cur]
        if op.startswith('push') or (op.startswith('stmdb') and args.startswith('sp!')):
            g = re.search(r'\{(.*)\}', args)
            if g:
                f['frame'] += 4 * nregs(g.group(1))
        elif op.startswith('vpush'):
            g = re.search(r'\{(.*)\}', args)
            if g:
                f['frame'] += 8 * nregs(g.group(1))
        elif re.match(r'(sub|subs|sub\.w|subw)$', op) and re.match(r'sp,\s*(sp,\s*)?#', args):
            f['frame'] += int(re.search(r'#(0x[0-9a-f]+|\d+)', args).group(1), 0)
        elif re.match(r'(sub|sub\.w)$', op) and re.match(r'sp,\s*(sp,\s*)?r\d+', args):
            f['dyn'] = True
        elif op == 'blx' and not re.search(r'<', args):
            f['ind'] = True
        elif re.match(r'(bl|blx|b)(eq|ne|cs|hs|cc|lo|mi|pl|vs|vc|hi|ls|ge|lt|gt|le|al)?(\.w)?$', op):
            # bl / b and their conditional forms (an IT block's `bllo`, a
            # conditional tail call `bne.w`).
            # A call / tail call targets a function's first instruction;
            # a target inside another function (`<foo+0x34a>`) is not one:
            # it is a disassembled literal / padding after a return.
            t = re.search(r'<(.*)>', args)
            if t and not re.search(r'\+0x[0-9a-f]+$', t.group(1)) and t.group(1) != cur:
                f['calls'].add(t.group(1))
    return funcs


# Interrupt handlers (vector table symbols) whose stack adds to the main
# path when present in the image.
ISRS = ['ad2_stream_irq']
EXC_FRAME = 26 * 4 + 4

# Known indirect calls, (caller regex, callee regex): every function matching
# the callee pattern is added as a call of every caller matching the first.
# The SCPI reply port is a `&mut dyn bt::ScpiPort` in main.rs `run`; its
# `respond_with` calls the reply builder, a `run` closure, through
# `&mut dyn FnMut` (Bt's has a 6 KiB stack buffer, main.rs MMEM / LOG:DATA).
INDIRECT = [
    (r'^opendm40::run$', r' as opendm40::bt::ScpiPort>::'),
    (r' as opendm40::bt::ScpiPort>::respond_with$', r'^opendm40::run::\{closure#\d+\}$'),
    # stockfont::paint hands its pixel closure to paint_dyn as &mut dyn FnMut
    # (one copy of the loop); the closures are the scope page's (scope_ui,
    # App::sp_*): every one of those counts as a callee (an upper bound).
    (r'^opendm40::stockfont::paint_dyn$', r'^(opendm40::scope_ui::|<opendm40::app::App>::sp_)\S*::\{closure#\d+\}$'),
]


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument('elf')
    ap.add_argument('--root', default='opendm40::native::start')
    ap.add_argument('--top', type=int, default=15, help='largest frames to list (0: none)')
    ap.add_argument('--check', type=int, metavar='BYTES', help='required headroom; exit 1 below it')
    a = ap.parse_args()
    dis = subprocess.run([tool('llvm-objdump'), '-d', '--no-show-raw-insn', '-C', a.elf],
                         capture_output=True, text=True, check=True).stdout.split('\n')
    funcs = parse(dis)
    for cpat, tpat in INDIRECT:
        targets = [n for n in funcs if re.search(tpat, n)]
        for n in funcs:
            if re.search(cpat, n):
                funcs[n]['calls'].update(t for t in targets if t != n)
    sys.setrecursionlimit(20000)
    memo, rec = {}, set()

    def depth(fn, stack=()):
        if fn in memo:
            return memo[fn]
        if fn in stack:
            rec.add(fn)
            return (0, [])
        if fn not in funcs:
            return (0, [])
        best = (0, [])
        for c in funcs[fn]['calls']:
            d = depth(c, stack + (fn,))
            if d[0] > best[0]:
                best = d
        r = (funcs[fn]['frame'] + best[0], [fn] + best[1])
        memo[fn] = r
        return r

    if a.root not in funcs:
        sys.exit('stackdepth: root %s not in the disassembly' % a.root)
    worst, path = depth(a.root)
    isr_txt, isr_paths = '', []
    for isr in ISRS:
        if isr in funcs:
            d, ip = depth(isr)
            isr_paths.append((isr, d, ip))
    if isr_paths:
        isr, d, ip = max(isr_paths, key=lambda t: t[1])
        worst_main = worst
        worst = worst_main + d + EXC_FRAME
        isr_txt = ' (main %d + ISR %s %d + exception frame %d)' % (worst_main, isr, d, EXC_FRAME)
    syms = {}
    nm = subprocess.run([tool('llvm-nm'), a.elf], capture_output=True, text=True).stdout
    for line in nm.split('\n'):
        p = line.split()
        if len(p) == 3:
            syms[p[2]] = int(p[0], 16)
    size = syms['_stack_top'] - syms['__bss_end']
    print('stack: %d bytes (_stack_top - __bss_end); static worst path from %s: %d bytes%s; headroom %d'
          % (size, a.root, worst, isr_txt, size - worst))
    acc = 0
    for p in path:
        f = funcs[p]
        acc += f['frame']
        flags = (' [indirect calls]' if f['ind'] else '') + (' [dynamic sp]' if f['dyn'] else '')
        print('  %6d %6d  %s%s' % (f['frame'], acc, p[:140], flags))
    for isr, d, ip in isr_paths:
        acc = 0
        print('ISR %s (+%d exception frame):' % (isr, EXC_FRAME))
        for p in ip:
            f = funcs[p]
            acc += f['frame']
            flags = (' [indirect calls]' if f['ind'] else '') + (' [dynamic sp]' if f['dyn'] else '')
            print('  %6d %6d  %s%s' % (f['frame'], acc, p[:140], flags))
    if rec:
        print('  (recursion cut at: %s)' % ', '.join(sorted(r[:60] for r in rec)[:5]))
    if a.top:
        print('largest frames:')
        for n in sorted(funcs, key=lambda n: -funcs[n]['frame'])[:a.top]:
            print('  %6d  %s' % (funcs[n]['frame'], n[:140]))
    if a.check is not None and size - worst < a.check:
        print('stackdepth: headroom %d < %d bytes required' % (size - worst, a.check), file=sys.stderr)
        sys.exit(1)


if __name__ == '__main__':
    main()
