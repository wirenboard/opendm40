"""Region C hooks for the emulator tests (firmware/link-native.x: .cold code in
0x08080000..0x080A0000, flash that is not zero-wait; a fetch from it while
the AD2 stream ISR owns the bus stalls the ISR's entry ~20 us on the device).

A Renode hook on every function symbol of region C writes the function's
address to a file on its first call while hy3131::HANDED is set (the ISR
owns the HY3131 bus) and removes itself (a Python hook per call of a busy
function slows the emulator to a crawl). Used by hy3131/wavetest.py and
fastlogtest.py:

    rc = regionc.Hooks(app_elf, out_dir, tag)   # rc.script: run.py --script
    rc.reset()                                  # before a scenario
    ...
    rc.calls()                                  # {function: 1} hit since
    rc.calls(every=True)                        # ... over the whole run

`gate`: the flag symbol the hooks test (default hy3131::HANDED, the AD2
stream ISR owns the bus), or a tuple of them (any set; the data watch
below takes the first).

`exclude`: name parts of functions left without a hook. Every call of a
hooked function runs a Python hook, handed or not: code that is hot outside
the stream (the fast logger's Prep; the screenshot pre-erase
screenshot::prep_step / disk, which runs from 2 s of disk quiet and, with
the W25Q128 busy timing on, for minutes: fastlogtest --ui-nav hung in its
page tree, 2026-10-24) slows the emulator to a crawl. Code that runs on
the main loop outside the sessions belongs here; the data watch below
still sees its literal-pool loads if it ever ran in a session.

A function is reported once per run (its hook is gone after the first hit):
a scenario's check sees only functions no earlier step called while the
ISR owned the bus, so the tests also check the whole run at the end.

DATA reads (2026-10-23, device c92b605: the USB volume's INFO/ files copied
out of flash >= 0x08080000 stalled the ISR 21.8 us per load, which the
fetch hooks above cannot see): peripherals/FlashWatch.cs records every CPU
data load from the slow bank 0x08080000..0x08100000 (region C, the
FS-loader, the loader data, the calibration record) while HANDED is set,
once per load instruction, and every PC of the slow bank the ISR interrupts
(code running there while streaming: a function entered before the stream
started, e.g. the tail of logger::Logger::start after stream::start, which
the entry hooks above miss). `calls` reports them as "data <function>+0x..
<- 0x0808xxxx" / "code <function>+0x.. (interrupted by the ISR)" next to
the region C functions, so every test that checks `calls` checks all. `data=False`: without the watch (it makes every load
and store a callout: slower).
"""
import glob
import os
import subprocess

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
C_LO, C_HI = 0x08080000, 0x080A0000
# The slow (not zero-wait) flash: from 0x08080000 to the end of the 1 MiB.
SLOW_LO, SLOW_HI = 0x08080000, 0x08100000


class Hooks:
    def __init__(self, app, out_dir, tag, exclude=(), data=True, gate='opendm40::hy3131::HANDED'):
        self.syms = {}
        self.funcs = []  # (address, name) of every function, sorted: data-read PCs
        self.hits = os.path.join(out_dir, tag + '.chits')
        self.dhits = os.path.join(out_dir, tag + '.dhits')
        self.script = None
        open(self.hits, 'w').close()
        open(self.dhits, 'w').close()
        nm = sorted(glob.glob(os.path.join(ROOT, 'toolchain/rustup/toolchains/*/lib/rustlib/*/bin/llvm-nm')))
        if not nm:
            return
        r = subprocess.run([nm[0], '-C', app], capture_output=True, text=True)
        gates = (gate,) if isinstance(gate, str) else tuple(gate)
        flags = {}
        for l in r.stdout.splitlines():
            p = l.split(None, 2)
            if len(p) < 3:
                continue
            a = int(p[0], 16)
            if p[1] in 'tT':
                self.funcs.append((a & ~1, p[2]))
            if p[1] in 'tT' and C_LO <= a < C_HI and not any(x in p[2] for x in exclude):
                self.syms[a & ~1] = p[2]
            else:
                for g in gates:
                    if p[2].startswith(g):
                        flags[g] = a
        if len(flags) != len(gates) or not self.syms:
            return
        handed = flags[gates[0]]
        cond = ' or '.join('(machine.SystemBus.ReadByte(0x%x)!=0)' % flags[g] for g in gates)
        code = ("(%s) and (open('%s','a').write('%%x 1'%%pc+chr(10)), self.RemoveHooksAt(pc))"
                % (cond, self.hits))

        self.script = os.path.join(out_dir, tag + '.cscript')
        self.funcs.sort()
        with open(self.script, 'w') as f:
            for a in sorted(self.syms):
                f.write('1 monitor sysbus.cpu AddHook 0x%08x "%s"\n' % (a, code))
            if data:
                f.write('1 monitor sysbus.flashwatch Arm 0x%08x 0x%08x 0x%08x "%s"\n'
                        % (SLOW_LO, SLOW_HI, handed, self.dhits))

    def lines(self):
        """The event-script lines (run.py --script), '' without hooks."""
        return open(self.script).read() if self.script else ''

    mark = 0
    dmark = 0

    def reset(self):
        """Start a scenario: `calls` reports the hits after this point."""
        self.mark = os.path.getsize(self.hits) if os.path.exists(self.hits) else 0
        self.dmark = os.path.getsize(self.dhits) if os.path.exists(self.dhits) else 0

    def func_of(self, pc):
        """'function+0xoff' of a code address."""
        import bisect
        i = bisect.bisect_right(self.funcs, (pc, '\uffff')) - 1
        if i < 0:
            return '0x%x' % pc
        a, n = self.funcs[i]
        return '%s+0x%x' % (n, pc - a)

    def data_reads(self, every=False):
        """{"data <function>+0x.. <- 0xaddr": 1}: slow-bank data loads while
        the ISR owned the bus (FlashWatch.cs), once per load instruction."""
        got = {}
        if os.path.exists(self.dhits):
            with open(self.dhits, 'rb') as f:
                f.seek(0 if every else self.dmark)
                for l in f.read().decode(errors='replace').splitlines():
                    p = l.strip('\0').split()
                    if len(p) == 2 and p[1] == 'code':
                        got['code %s (interrupted by the ISR)' % self.func_of(int(p[0], 16))] = 1
                    elif len(p) == 2:
                        got['data %s <- 0x%s' % (self.func_of(int(p[0], 16)), p[1])] = 1
        return got

    def calls(self, every=False):
        """{region C function: 1} called while the ISR owned the bus since
        the last reset (every: since the start of the run)."""
        got = {}
        if os.path.exists(self.hits):
            with open(self.hits, 'rb') as f:
                f.seek(0 if every else self.mark)
                for l in f.read().decode(errors='replace').splitlines():
                    p = l.strip('\0').split()
                    if len(p) == 2:
                        n = self.syms.get(int(p[0], 16), p[0])
                        got[n] = max(got.get(n, 0), int(p[1]))
        got.update(self.data_reads(every))
        return got

    def cleanup(self):
        for p in (self.script, self.hits, self.dhits):
            if p and os.path.exists(p):
                os.remove(p)
