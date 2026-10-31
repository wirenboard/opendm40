#!/usr/bin/env python3
"""Profile of the opendm40 image's zero-wait flash (region B) or region C:
who owns every byte, and which of it can run while the AD2 stream ISR owns
the HY3131 bus (docs/native-boot.md "Region B audit #9").

    tools/region-profile/rprof.py ELF [--trace FILE|DIR ...] [--region b|c]
                                      [--top N] [--json OUT] [--candidates]

Ownership (every byte of .text / .rodata in the region and the .data load
image):
  * code: the function symbol (STT_FUNC), and inside it the innermost
    inlined callee of each instruction (llvm-symbolizer --inlining over
    every instruction address; the source file names the module / crate:
    src/scope_ui.rs -> opendm40::scope_ui, library/core/src/fmt -> core::fmt,
    crates/usb-device-0.3.2 -> usb-device);
  * data: named statics (STT_OBJECT, e.g. the font blobs); anonymous rodata
    (constants, strings, tables: no symbol) is cut at every address some
    code or data refers to (literal pool words, movw / movt pairs, pointers
    inside data) and each piece is charged to its referrers.

Reachability (static, from the disassembly): calls (bl / b / blx to a
symbol) and references (a literal / movw-movt / data word that holds a
function's address: address-taken, e.g. a vtable entry or a fn pointer).
A data item reachable from a function makes every function it points to
reachable (a vtable read means its methods may run). Classes:
  isr      reachable from the stream ISR (ad2_stream_irq): must be fast;
  traced   executed while hy3131::HANDED was set in an emulator run
           (--trace: emu/run.py with DM40_BTRACE=DIR, peripherals/BTrace.cs;
           --trace DIR@ELF: a trace of another build, matched by function
           name), or data referred to by such code: must be fast;
  loop     reachable from the main loop (opendm40::run) without passing
           through region C code, but not seen in the traces: may run
           under the stream (the static graph cannot tell which page /
           state is live); a move needs a reason it never does;
  c-only   reachable only through region C code (or only from start-up):
           never under the stream while the region C rule holds (region C
           code never runs while HANDED, emu/regionc.py checks it);
  none     no static path from any root (vectors, start-up, loop, ISR).
Without --trace there is no `traced` class.

--candidates: the `c-only` and `loop`-not-traced functions with their
callers in B (what a link-native.x pattern or an #[inline(never)] would
move). --json: every item with its class, size and module (for diffs).

Needs: pyelftools; llvm-objdump (toolchain/ or the active rustc's
llvm-tools); llvm-symbolizer (any LLVM, e.g. /usr/lib/llvm-*/bin) for the
inlined-callee table (skipped without it).
"""
import argparse
import bisect
import collections
import glob
import json
import os
import re
import shutil
import struct
import subprocess
import sys

from elftools.elf.elffile import ELFFile

HERE = os.path.dirname(os.path.abspath(__file__))
B_LO, B_HI = 0x08008010, 0x08080000
C_LO, C_HI = 0x08080000, 0x080A0000
FLASH_LO, FLASH_HI = 0x08000000, 0x08100000
ISR_ROOTS = ['ad2_stream_irq']
LOOP_ROOT = 'opendm40::run'


def tool(name):
    pats = []
    if os.environ.get('DM40_LLVMBIN'):
        pats.append(os.path.join(os.environ['DM40_LLVMBIN'], name))
    pats.append(os.path.join(HERE, '../../../toolchain/rustup/toolchains/*/lib/rustlib/*/bin/' + name))
    try:
        sysroot = subprocess.run(['rustc', '--print', 'sysroot'], capture_output=True, text=True).stdout.strip()
        if sysroot:
            pats.append(os.path.join(sysroot, 'lib/rustlib/*/bin/' + name))
    except OSError:
        pass
    pats.append('/usr/lib/llvm-*/bin/' + name)
    for pat in pats:
        m = sorted(glob.glob(pat))
        if m:
            return m[-1]
    return shutil.which(name)


class Image:
    def __init__(self, path, region):
        self.path = path
        f = open(path, 'rb')
        self.elf = ELFFile(f)
        self.segs = []  # (vaddr/lma, data) of the flash load image
        for s in self.elf.iter_sections():
            if s['sh_type'] == 'SHT_PROGBITS' and s['sh_flags'] & 2 and s['sh_size']:
                self.segs.append((s['sh_addr'], s.data(), s.name))
        # The .data load image (LMA in B): from the program headers.
        for seg in self.elf.iter_segments():
            if seg['p_type'] == 'PT_LOAD' and seg['p_paddr'] != seg['p_vaddr'] and seg['p_filesz']:
                self.segs.append((seg['p_paddr'], seg.data(), '.data(load)'))
        self.syms = {}  # name -> value (linker symbols)
        funcs, objs = {}, {}
        st = self.elf.get_section_by_name('.symtab')
        for s in st.iter_symbols():
            v, sz, typ = s['st_value'], s['st_size'], s['st_info']['type']
            self.syms.setdefault(s.name, v)
            if not s.name or s.name.startswith('$'):
                continue
            if typ == 'STT_FUNC' and FLASH_LO <= v < FLASH_HI:
                funcs.setdefault(v & ~1, (s.name, sz))
            elif typ in ('STT_OBJECT', 'STT_NOTYPE') and FLASH_LO <= v < FLASH_HI and sz:
                objs.setdefault(v, (s.name, sz))
        self.mangled_funcs = funcs
        self.mangled_objs = objs
        dem = {}
        nm = tool('llvm-nm')
        out = subprocess.run([nm, '-C', '--defined-only', path], capture_output=True, text=True).stdout
        raw = subprocess.run([nm, '--defined-only', path], capture_output=True, text=True).stdout
        for a, b in zip(out.splitlines(), raw.splitlines()):
            pa, pb = a.split(None, 2), b.split(None, 2)
            if len(pa) == 3 and len(pb) == 3:
                dem[pb[2]] = pa[2]
        self.dem = dem
        self.b_end = self.syms.get('__b_end', B_HI)
        self.c_lo = self.syms.get('__cold_start', C_LO)
        self.c_hi = self.syms.get('__cold_end', C_HI)
        self.region = region
        if region == 'b':
            self.lo, self.hi = B_LO, self.b_end
        else:
            self.lo, self.hi = self.c_lo, self.c_hi
        self.data_lma = self.syms.get('__sidata')

    def word(self, a):
        for base, d, _ in self.segs:
            if base <= a and a + 4 <= base + len(d):
                return struct.unpack_from('<I', d, a - base)[0]
        return None

    def in_c(self, a):
        return self.c_lo <= a < self.c_hi

    def in_b(self, a):
        return B_LO <= a < self.b_end


def module_of_file(fn):
    if not fn:
        return None
    m = re.search(r'/library/(core|alloc|std)/src/(.*)$', fn)
    if m:
        parts = m.group(2).split('/')
        sub = parts[0][:-3] if parts[0].endswith('.rs') else parts[0]
        return '%s::%s' % (m.group(1), sub)
    m = re.search(r'compiler[-_]builtins', fn)
    if m:
        return 'compiler_builtins'
    m = re.search(r'(?:crates|registry/src/[^/]+)/([A-Za-z0-9_\-]+?)-\d+\.\d+[^/]*/', fn)
    if m:
        return m.group(1)
    m = re.search(r'vendor/([A-Za-z0-9_\-]+)/', fn)
    if m:
        return m.group(1)
    m = re.search(r'(?:^|/)src/(.*)\.rs$', fn)
    if m:
        p = m.group(1).split('/')
        if p[-1] in ('main', 'lib', 'mod'):
            p = p[:-1]
        return 'opendm40::' + '::'.join(p) if p else 'opendm40'
    return fn.rsplit('/', 1)[-1]


def module_of_name(n):
    n = n.lstrip('<')
    m = re.match(r'([A-Za-z_][A-Za-z0-9_]*)(?:::([A-Za-z_][A-Za-z0-9_]*))?', n)
    if not m:
        return '?'
    if m.group(1) in ('core', 'alloc', 'opendm40') and m.group(2):
        return '%s::%s' % (m.group(1), m.group(2))
    if m.group(1).startswith('__aeabi') or m.group(1).startswith('__') or m.group(1) in ('memcpy', 'memset', 'memmove', 'memcmp'):
        return 'compiler_builtins'
    return m.group(1)


def disassemble(img):
    """-> {func_addr: [(addr, op, args)]} for every function symbol."""
    od = tool('llvm-objdump')
    out = subprocess.run([od, '-d', '--no-show-raw-insn', img.path], capture_output=True, text=True).stdout
    ins_re = re.compile(r'^\s*([0-9a-f]+):\s+(\S+)\s*(.*)$')
    addrs = sorted(img.mangled_funcs)
    res = collections.defaultdict(list)
    for l in out.splitlines():
        m = ins_re.match(l)
        if not m:
            continue
        a = int(m.group(1), 16)
        i = bisect.bisect_right(addrs, a) - 1
        if i < 0:
            continue
        fa = addrs[i]
        if a >= fa + max(img.mangled_funcs[fa][1], 2):
            continue
        res[fa].append((a, m.group(2), m.group(3)))
    return res


def build(img, dis):
    """Nodes: ('f', addr) functions, ('d', addr) data items. Edges and sizes."""
    funcs = img.mangled_funcs
    faddrs = sorted(funcs)
    # Data items: named objects + anonymous pieces cut at referenced addresses.
    refs = collections.defaultdict(set)   # node -> referenced flash addresses
    calls = collections.defaultdict(set)  # func -> called func addrs

    def func_at(a):
        i = bisect.bisect_right(faddrs, a) - 1
        if i >= 0 and a < faddrs[i] + max(funcs[faddrs[i]][1], 2):
            return faddrs[i]
        return None

    for fa, ins in dis.items():
        regs = {}
        for a, op, args in ins:
            if op == '.word':
                v = int(args.split()[0], 0)
                if FLASH_LO <= v < FLASH_HI:
                    refs[('f', fa)].add(v)
                continue
            mm = re.match(r'(bl|blx|b)(eq|ne|cs|hs|cc|lo|mi|pl|vs|vc|hi|ls|ge|lt|gt|le|al)?(\.w)?$', op)
            if mm:
                t = re.match(r'(0x)?([0-9a-f]+)', args.strip())
                if t:
                    ta = int(t.group(2), 16)
                    if ta in funcs and ta != fa:
                        calls[fa].add(ta)
                continue
            if op.startswith('ldr') and '[pc' in args:
                # A literal-pool load: objdump names the pool word's address.
                g = re.search(r'@ 0x([0-9a-f]+)', args)
                if g:
                    v = img.word(int(g.group(1), 16))
                    if v is not None and FLASH_LO <= v < FLASH_HI:
                        refs[('f', fa)].add(v)
                continue
            if op.startswith('movw'):
                g = re.match(r'(r\d+|lr|r1[0-2]),\s*#(0x[0-9a-f]+|\d+)', args)
                if g:
                    regs[g.group(1)] = int(g.group(2), 0)
            elif op.startswith('movt'):
                g = re.match(r'(r\d+|lr|r1[0-2]),\s*#(0x[0-9a-f]+|\d+)', args)
                if g and g.group(1) in regs:
                    v = regs.pop(g.group(1)) | (int(g.group(2), 0) << 16)
                    if FLASH_LO <= v < FLASH_HI:
                        refs[('f', fa)].add(v)
    # Literal-pool loads: objdump decodes $d mapping-symbol ranges as .word,
    # so the loop above has every pool word. Data boundaries:
    cuts = set()
    for s in refs.values():
        for v in s:
            if func_at(v & ~1) is None:
                cuts.add(v)
    objs = dict(img.mangled_objs)
    # The region's covered ranges: functions and named objects; the rest of
    # [lo, hi) is anonymous data.
    covered = sorted([(a, a + max(funcs[a][1], 0)) for a in faddrs] + [(a, a + objs[a][1]) for a in objs])
    lo, hi = min(B_LO, img.c_lo), max(img.b_end, img.c_hi)
    gaps = []
    cur = lo
    for a, e in covered:
        if a > cur:
            gaps.append((cur, a))
        cur = max(cur, e)
    if cur < hi:
        gaps.append((cur, hi))
    # Cut each gap at referenced addresses; also scan named data words.
    items = {}  # addr -> (name, size, kind)
    for a, (n, sz) in objs.items():
        items[a] = (img.dem.get(n, n), sz, 'obj')
    # Iterate: pointers inside data add more cuts.
    for _ in range(3):
        new = set()
        pieces = []
        cs = sorted(cuts)
        for g0, g1 in gaps:
            # the C/B boundary is a cut too
            pts = [g0] + [c for c in cs[bisect.bisect_left(cs, g0):bisect.bisect_left(cs, g1)] if c > g0]
            for x in (B_HI, img.c_lo, img.b_end):
                if g0 < x < g1:
                    pts.append(x)
            pts = sorted(set(pts)) + [g1]
            for p0, p1 in zip(pts, pts[1:]):
                pieces.append((p0, p1))
        for p0, p1 in pieces:
            for w in range((p0 + 3) & ~3, p1 - 3, 4):
                v = img.word(w)
                if v is not None and FLASH_LO <= v < FLASH_HI and func_at(v & ~1) is None and v not in cuts:
                    new.add(v)
        if not new:
            break
        cuts |= new
    for p0, p1 in pieces:
        items.setdefault(p0, ('anon@%08x' % p0, p1 - p0, 'anon'))
    iaddrs = sorted(items)

    def item_at(a):
        i = bisect.bisect_right(iaddrs, a) - 1
        if i >= 0 and a < iaddrs[i] + max(items[iaddrs[i]][1], 1):
            return iaddrs[i]
        return None

    # Pointers inside data items.
    for ia, (n, sz, kind) in items.items():
        for w in range((ia + 3) & ~3, ia + sz - 3, 4):
            v = img.word(w)
            if v is not None and FLASH_LO <= v < FLASH_HI:
                refs[('d', ia)].add(v)
    # Edges.
    edges = collections.defaultdict(set)
    for fa, cs in calls.items():
        for t in cs:
            edges[('f', fa)].add(('f', t))
    for node, vs in refs.items():
        for v in vs:
            f = func_at(v & ~1)
            if f is not None:
                if f == (v & ~1) and v & 1:
                    edges[node].add(('f', f))  # address-taken (thumb bit)
                continue
            it = item_at(v)
            if it is not None and it != node[1]:
                edges[node].add(('d', it))
    return items, edges, func_at, item_at


def reach(roots, edges, blocked):
    seen = set()
    st = [r for r in roots]
    while st:
        n = st.pop()
        if n in seen:
            continue
        seen.add(n)
        if n in blocked:
            continue
        st.extend(edges.get(n, ()))
    return seen


def load_traces(paths):
    """{pc: instructions} of the traces of this ELF, and the demangled names
    of the functions hit in traces of another build (PATH@ELF)."""
    blocks, names = {}, set()
    for p in paths:
        p, _, other = p.partition('@')
        fs = sorted(glob.glob(os.path.join(p, '*.btrace'))) if os.path.isdir(p) else [p]
        if other:
            o = Image(other, 'b')
            oaddrs = sorted(o.mangled_funcs)
            for f in fs:
                for l in open(f, errors='replace'):
                    q = l.split()
                    if len(q) == 2:
                        a = int(q[0], 16)
                        i = bisect.bisect_right(oaddrs, a) - 1
                        if i >= 0 and a < oaddrs[i] + max(o.mangled_funcs[oaddrs[i]][1], 2):
                            n = o.mangled_funcs[oaddrs[i]][0]
                            names.add(o.dem.get(n, n))
            continue
        for f in fs:
            for l in open(f, errors='replace'):
                q = l.split()
                if len(q) == 2:
                    a, n = int(q[0], 16), int(q[1])
                    blocks[a] = max(blocks.get(a, 0), n)
    return blocks, names


def symbolize(img, dis, lo, hi):
    """{instr_addr: (size, [frames innermost first: (func, file)])} over the region."""
    sym = tool('llvm-symbolizer')
    if not sym:
        return None
    addrs = []
    for fa, ins in dis.items():
        if not (lo <= fa < hi):
            continue
        end = fa + img.mangled_funcs[fa][1]
        for i, (a, op, args) in enumerate(ins):
            nxt = ins[i + 1][0] if i + 1 < len(ins) else end
            addrs.append((a, nxt - a, op))
    inp = '\n'.join('0x%x' % a for a, _, _ in addrs) + '\n'
    out = subprocess.run([sym, '--obj=' + img.path, '--inlining', '--output-style=JSON'],
                         input=inp, capture_output=True, text=True).stdout
    res = {}
    for (a, sz, op), l in zip(addrs, out.splitlines()):
        try:
            j = json.loads(l)
        except ValueError:
            continue
        fr = [(s.get('FunctionName', ''), s.get('FileName', '')) for s in j.get('Symbol', [])]
        res[a] = (sz, fr, op)
    return res


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument('elf')
    ap.add_argument('--trace', action='append', default=[], help='BTrace file or directory (emu DM40_BTRACE); PATH@ELF: a trace of another build '
                    '(ELF), matched by function name')
    ap.add_argument('--region', choices=['b', 'c'], default='b')
    ap.add_argument('--top', type=int, default=25)
    ap.add_argument('--json', help='write every item here')
    ap.add_argument('--candidates', action='store_true', help='list movable functions with their B callers')
    ap.add_argument('--no-inline', action='store_true', help='skip the llvm-symbolizer pass')
    a = ap.parse_args()

    img = Image(a.elf, a.region)
    dis = disassemble(img)
    items, edges, func_at, item_at = build(img, dis)
    funcs = img.mangled_funcs
    D = lambda n: img.dem.get(n, n)  # noqa: E731

    def name(node):
        k, ad = node
        return D(funcs[ad][0]) if k == 'f' else items[ad][0]

    def size(node):
        k, ad = node
        return funcs[ad][1] if k == 'f' else items[ad][1]

    def addr_in_c(node):
        return img.in_c(node[1])

    byname = {D(n): ('f', ad) for ad, (n, _) in funcs.items()}
    cnodes = {('f', ad) for ad in funcs if img.in_c(ad)} | {('d', ad) for ad in items if img.in_c(ad)}
    isr_roots = [byname[r] for r in ISR_ROOTS if r in byname]
    loop_roots = [byname[LOOP_ROOT]] if LOOP_ROOT in byname else []
    # Start-up: every function nothing calls or references (vectors, reset,
    # the shim) is a root of "any path".
    called = set()
    for s in edges.values():
        called |= s
    # Vector table words (region A) -> functions.
    vec = []
    for w in range(0x08004000, 0x08004000 + 0x200, 4):
        v = img.word(w)
        if v and v & 1 and func_at(v & ~1) == (v & ~1):
            vec.append(('f', v & ~1))
    all_roots = set(vec) | set(isr_roots) | set(loop_roots) | {('f', ad) for ad in funcs if ('f', ad) not in called}
    r_isr = reach(isr_roots, edges, cnodes)
    r_loop = reach(loop_roots, edges, cnodes)
    r_any = reach(all_roots, edges, set())

    # Dynamic trace -> executed functions (by instruction ranges).
    traced_f = set()
    blocks, other_names = load_traces(a.trace)
    if other_names:
        # Another build's trace: the same demangled function names here (the
        # mangled ones carry a crate hash that differs between feature sets).
        for ad, (n, _) in funcs.items():
            if D(n) in other_names:
                traced_f.add(('f', ad))
    if blocks:
        for pc, n in blocks.items():
            f = func_at(pc)
            if f is None:
                continue
            traced_f.add(('f', f))
            # A block can run on past a call's return into the next
            # function only by fall-through; n instructions from pc stay
            # inside f except at its end.
            ins = dis.get(f, [])
            i = bisect.bisect_left(ins, (pc, '', ''))
            if i + n > len(ins):
                nf = func_at(funcs[f][1] + f)
                if nf is not None:
                    traced_f.add(('f', nf))
    r_traced = set(traced_f)
    # Data referenced by traced code (and data that data points to).
    for n in list(traced_f):
        for t in edges.get(n, ()):
            if t[0] == 'd':
                r_traced |= {x for x in reach([t], edges, cnodes) if x[0] == 'd'}
    # Data read through ISR code is in r_isr already (data nodes included).

    def cls(node):
        if node in r_isr:
            return 'isr'
        if node in r_traced:
            return 'traced'
        if node in r_loop:
            return 'loop'
        if node in r_any:
            return 'c-only'
        return 'none'

    lo, hi = img.lo, img.hi
    nodes = [('f', ad) for ad in funcs if lo <= ad < hi] + [('d', ad) for ad in items if lo <= ad < hi]
    # .data load image (B only): charge it as one item.
    data_img = 0
    if a.region == 'b' and img.data_lma and img.data_lma < img.b_end:
        data_img = img.b_end - img.data_lma
        nodes = [n for n in nodes if not (n[0] == 'd' and n[1] >= img.data_lma)]
    tot = collections.Counter()
    totk = collections.Counter()
    for n in nodes:
        c = cls(n)
        sz = size(n)
        if n[0] == 'd' and n[1] + sz > hi:
            sz = hi - n[1]
        tot[c] += sz
        totk[(c, 'code' if n[0] == 'f' else 'data')] += sz
    used = sum(tot.values()) + data_img
    reg = 'B (0x%08x..0x%08x)' % (lo, hi) if a.region == 'b' else 'C (0x%08x..0x%08x)' % (lo, hi)
    print('region %s: %d bytes used%s' % (reg, used, (' (+ .data load image %d)' % data_img) if data_img else ''))
    if blocks or other_names:
        print('trace: %d blocks executed while HANDED (+ %d function names from other builds), %d functions'
              % (len(blocks), len(other_names), len(traced_f)))
    order = ['isr', 'traced', 'loop', 'c-only', 'none']
    print('\n%-8s %9s %9s %9s' % ('class', 'code', 'data', 'total'))
    for c in order:
        if tot[c]:
            print('%-8s %9d %9d %9d' % (c, totk[(c, 'code')], totk[(c, 'data')], tot[c]))
    if data_img:
        print('%-8s %9s %9d %9d' % ('.data', '', data_img, data_img))
    fast = tot['isr'] + tot['traced']
    print('must be fast (isr + traced): %d (%.0f%%); the rest %d' % (fast, 100.0 * fast / max(used, 1), used - fast))

    # Inline attribution.
    leaf_bytes = collections.Counter()
    leaf_sites = collections.Counter()
    leaf_cls = collections.defaultdict(collections.Counter)
    mod_cls = collections.defaultdict(collections.Counter)
    sy = None if a.no_inline else symbolize(img, dis, lo, hi)
    fcls = {n[1]: cls(n) for n in nodes if n[0] == 'f'}
    if sy:
        prev = None
        for ia in sorted(sy):
            sz, fr, op = sy[ia]
            f = func_at(ia)
            c = fcls.get(f, '?')
            if op in ('.word', '.short', '.byte'):
                key, fn = '(literal pool)', ''
                mod = module_of_name(D(funcs[f][0])) if f else '?'
                # charge pool words to the function's own module
                fn0 = fr[-1][1] if fr else ''
                mod = module_of_file(fn0) or mod
            else:
                key, fn = (fr[0] if fr else (D(funcs[f][0]) if f else '?', ''))
                mod = module_of_file(fn) or module_of_name(key)
            leaf_bytes[key] += sz
            leaf_cls[key][c] += sz
            mod_cls[mod][c] += sz
            # one "site" per contiguous run of the same innermost frame
            site = (f, key, len(fr))
            if site != prev:
                leaf_sites[key] += 1
            prev = site
    else:
        for n in nodes:
            if n[0] == 'f':
                mod_cls[module_of_name(name(n))][cls(n)] += size(n)
    for n in nodes:
        if n[0] == 'd':
            nm_ = name(n)
            mod = module_of_name(nm_) if not nm_.startswith('anon@') else '(anonymous rodata)'
            mod_cls[mod][cls(n)] += size(n)

    print('\nby module / crate (code by innermost inlined source file; data by symbol):')
    print('%-36s %8s  %s' % ('module', 'bytes', ' '.join('%7s' % c for c in order)))
    for m, cc in sorted(mod_cls.items(), key=lambda kv: -sum(kv[1].values()))[:max(a.top, 40)]:
        print('%-36s %8d  %s' % (m[:36], sum(cc.values()), ' '.join('%7d' % cc[c] for c in order)))

    print('\nlargest functions per class:')
    for c in order:
        fl = sorted((n for n in nodes if n[0] == 'f' and cls(n) == c), key=lambda n: -size(n))
        if not fl:
            continue
        print('  %s (%d functions, %d bytes):' % (c, len(fl), sum(size(n) for n in fl)))
        for n in fl[:a.top]:
            print('    %7d  %s' % (size(n), name(n)[:120]))
    print('\nlargest data items per class:')
    for c in order:
        dl = sorted((n for n in nodes if n[0] == 'd' and cls(n) == c), key=lambda n: -size(n))
        if not dl:
            continue
        print('  %s (%d items, %d bytes):' % (c, len(dl), sum(size(n) for n in dl)))
        for n in dl[:a.top]:
            refby = sorted({name(s)[:60] for s, ts in edges.items() if n in ts})[:3]
            print('    %7d  %-28s <- %s' % (size(n), name(n)[:28], ', '.join(refby)))
    if sy:
        print('\nlargest innermost (inlined) callees, bytes / contiguous sites:')
        for k, b in leaf_bytes.most_common(a.top * 2):
            cc = leaf_cls[k]
            print('    %7d %5d  %-90s %s' % (b, leaf_sites[k], k[:90],
                                           ' '.join('%s %d' % (c, cc[c]) for c in order if cc[c])))
        # Generic duplicates: symbol names equal once the generic arguments are dropped.
    groups = collections.defaultdict(list)
    for n in nodes:
        if n[0] == 'f':
            # Generic arguments only (`Name<..>`, `::<..>`), innermost
            # first; a leading `<Type>` / `<T as Trait>` path stays.
            base = name(n)
            while True:
                b2 = re.sub(r'([A-Za-z0-9_]|::)<[^<>]*>', r'\1<_>', base)
                b2 = b2.replace('<_<_>>', '<_>')
                if b2 == base:
                    break
                base = b2
            groups[re.sub(r'::\{closure#\d+\}', '::{closure}', base)].append(n)
    dups = [(sum(size(x) for x in g) - max(size(x) for x in g), k, g) for k, g in groups.items() if len(g) > 1]
    print('\nsymbols with several copies (same name without generic arguments; bytes above the largest copy):')
    for extra, k, g in sorted(dups, reverse=True)[:a.top]:
        print('    %7d  x%-3d %s' % (extra, len(g), k[:110]))

    if a.candidates:
        callers = collections.defaultdict(set)
        for s, ts in edges.items():
            for t in ts:
                callers[t].add(s)
        print('\nmove candidates (c-only, and loop without a trace hit), with their callers:')
        for c in ('c-only', 'loop'):
            fl = sorted((n for n in nodes if n[0] == 'f' and cls(n) == c), key=lambda n: -size(n))
            for n in fl[:a.top * 2]:
                cs = sorted(callers.get(n, ()), key=lambda s: -size(s))
                txt = ', '.join('%s%s' % ('C:' if addr_in_c(s) else '', name(s)[:50]) for s in cs[:3])
                print('  %-6s %6d  %-70s <- %s' % (c, size(n), name(n)[:70], txt))
    if a.json:
        out = []
        for n in nodes:
            out.append({'kind': n[0], 'addr': n[1], 'name': name(n), 'size': size(n), 'class': cls(n)})
        with open(a.json, 'w') as f:
            json.dump({'region': a.region, 'used': used, 'classes': dict(tot), 'items': out,
                       'modules': {m: dict(cc) for m, cc in mod_cls.items()}}, f, indent=0)


if __name__ == '__main__':
    main()
