#!/usr/bin/env python3
"""DM40 emulator driver (Renode).

    emu/run.py stock|fwrs|chain [options]

Modes
  stock   pristine V2.6 application at 0x08004000 (from dm40_v_2_6.dec.bin),
          ROM/FS-loader/calibration from the flash dump, start at the app.
  fwrs    opendm40 native image at 0x08004000 (--app <opendm40.bin|body.bin|ELF>,
          default: build from a clean worktree of HEAD), start at the app.
  chain   start at the ROM bootloader (0x08000000): ROM -> FS-loader -> app.
          The app is the stock one unless --app is given; --app dump keeps the
          flash dump's own application.

Images are composed in emu/out/ and never written back to the dumps.
--model A|B|C sets the DM40 model byte of the calibration record (flash
0x080C0168, record CRC rewritten) in the composed image; both the stock
firmware and opendm40 read the model there (firmware/docs/model-variants.md).
Default: the base dump's own byte (this unit: 0 = DM40A).
Events (--tap/--key/--shot/--vin/--script) are scheduled in virtual time.
"""
import argparse
import glob
import os
import re
import signal
import shutil
import struct
import subprocess
import time
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
OUT = os.path.join(HERE, 'out')
sys.path.insert(0, os.path.join(ROOT, 'stock'))
import dm40fw  # noqa: E402

DUMP = os.path.join(ROOT, 'stock/re/dumps/full-20261013/flash_08000000_1MB.bin')
OTP = os.path.join(ROOT, 'stock/re/dumps/full-20261013/otp_1FFF7800.bin')
UID = os.path.join(ROOT, 'private/memrw-kit-test/full/uid_1FFF7A10.bin')
STOCK_DEC = os.path.join(ROOT, 'dm40_v_2_6.dec.bin')
DISK_SRC = os.path.join(ROOT, 'stock/re/dumps/spi_dumps/dm40_msc_full_2026-10-13.bin')
RENODE = os.path.join(HERE, '.renode/current/renode')

APP_BASE = 0x08004000
LOADER_BASE = 0x080A0000
MARKER = 0x08008000


def rel(p):
    return os.path.relpath(p, os.getcwd())


def read(p):
    with open(p, 'rb') as f:
        return f.read()


# Self-chosen ports (free_port): above the kernel's ephemeral range
# (net.ipv4.ip_local_port_range 32768-60999), so no client socket holds one,
# and outside the fixed ranges handed to parallel runs (3240, 39000-39999,
# 4xxxx per agent). DM40_EMU_PORTS=lo-hi (environment) narrows it to a
# range of one's own, e.g. DM40_EMU_PORTS=48000-48999.
AUTO_PORTS = (61000, 64999)


def free_port(lo=None, hi=None):
    """A TCP port in lo..hi (default DM40_EMU_PORTS or AUTO_PORTS) nobody
    listens on now (random order, so two tests started together rarely try
    the same one): the default USB/IP port of the tests that start their
    own emulator (mscresettest.py, ejecttest.py)."""
    import random
    import socket
    if lo is None:
        env = os.environ.get('DM40_EMU_PORTS')
        lo, hi = (int(x) for x in env.split('-')) if env else AUTO_PORTS
    ports = list(range(lo, hi + 1))
    random.SystemRandom().shuffle(ports)
    for p in ports:
        s = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        try:
            s.bind(('0.0.0.0', p))
            return p
        except OSError:
            continue
        finally:
            s.close()
    raise OSError('no free TCP port in %d..%d' % (lo, hi))


def emu_uid(port, base=UID):
    """The emulated chip's 96-bit UID (12 bytes, memory order) for USB/IP
    port `port`: this unit's UID file (a fixed pattern without it) with bytes
    4 and 9 = the port, big-endian. The stock genuine check (UID against the
    dump's 0x080A4000 block) never reads bytes 4 and 9
    (stock/re/docs/genuine-check.md), so the stock still gets its 200 MHz PLL,
    while opendm40's USB / SCSI / *IDN? serial
    number (the UID as hex, firmware/src/uid.rs) differs from the real meter's
    and between instances on different ports: udisks merges disks with the
    same vendor / model / serial into one drive."""
    u = bytearray(read(base)) if os.path.exists(base) else bytearray(b'EMU-DM40-UID')
    real = bytes(u)
    u[4], u[9] = port >> 8 & 0xFF, port & 0xFF
    if bytes(u) == real:
        u[4] ^= 0x80
    return bytes(u)


def uid_arg(spec, port):
    """--uid: none = emu_uid(port); 'unit' = the UID file as it is (this
    unit's own serial: never next to the real meter on one PC); 24 hex
    digits (memory order, as opendm40's serial); or a 12-byte file."""
    if not spec:
        return emu_uid(port)
    if spec == 'unit':
        return read(UID)
    if re.fullmatch(r'[0-9A-Fa-f]{24}', spec):
        return bytes.fromhex(spec)
    u = read(spec)[:12]
    if len(u) != 12:
        sys.exit('--uid %s: not 12 bytes' % spec)
    return u


def app_from_file(path):
    """-> (header12, body) from a .dec.bin, a packed update file, a raw body or an ELF."""
    d = read(path)
    if d[:4] == b'\x7fELF':
        body = elf_body(path)
        return dm40fw.split(dm40fw.decode(dm40fw.build(body, pad=True)))[:3], body
    if len(d) > 12 and struct.unpack_from('<I', d, 0)[0] == 0x2002F7F8:
        body = d  # raw body (word 0 = stock SP)
        w0, ln, w2, _ = dm40fw.split(dm40fw.decode(dm40fw.build(body, pad=True)))
        return (w0, ln, w2), body
    try:
        w0, ln, w2, body = dm40fw.split(d)  # already decoded (12-byte header)
        if ln == len(body) and dm40fw.header_word0(ln, w2) == w0:
            return (w0, ln, w2), body
    except Exception:
        pass
    w0, ln, w2, body = dm40fw.split(dm40fw.decode(d))
    return (w0, ln, w2), body


def elf_symbol(path, prefix):
    """Address of the first symbol of ELF `path` whose demangled name starts
    with `prefix`, or None."""
    nm = shutil.which('llvm-nm') or (sorted(glob.glob(os.path.join(
        ROOT, 'toolchain/rustup/toolchains/*/lib/rustlib/*/bin/llvm-nm'))) or [None])[0]
    if not nm:
        return None
    for l in subprocess.run([nm, '-C', path], capture_output=True, text=True).stdout.splitlines():
        p = l.split(None, 2)
        if len(p) == 3 and p[2].startswith(prefix):
            return int(p[0], 16)
    return None


def elf_body(path):
    objcopy = shutil.which('arm-none-eabi-objcopy') or shutil.which('llvm-objcopy')
    if not objcopy:
        r = subprocess.run(['sh', '-c', 'find %s/toolchain/rustup -name llvm-objcopy | head -1' % ROOT],
                           capture_output=True, text=True)
        objcopy = r.stdout.strip()
    # Per process: parallel runs (test harnesses, several agents) shared one
    # app-from-elf.body.bin, and a run could read another's half-written body.
    tmp = os.path.join(OUT, 'app-from-elf.%d.body.bin' % os.getpid())
    try:
        subprocess.check_call([objcopy, '-O', 'binary', '--gap-fill=0xff', path, tmp])
        return read(tmp)
    finally:
        try:
            os.remove(tmp)
        except OSError:
            pass


CAL_REC = 0x080C0000 - 0x08000000     # settings + calibration record (800 bytes)
CAL_REC_LEN = 800
MODEL_OFF = CAL_REC + 0x168           # calibration struct byte 0 = model
MODELS = 'ABC'


def flash_model(flash):
    """-> 'A' / 'B' / 'C' as the stock reads it: record version 1 and CRC ok,
    byte clamped to 2; otherwise the stock defaults (model 0 = 'A')."""
    rec = flash[CAL_REC:CAL_REC + CAL_REC_LEN]
    c = dm40fw.crc16(rec[4:])
    ok = struct.unpack_from('<HHI', rec) == (c, c ^ 0xffff, 1)
    return MODELS[min(flash[MODEL_OFF], 2)] if ok else 'A'


def set_model(flash, letter):
    """Patch the model byte and rewrite the record's CRC words (u16 crc16 of
    bytes 4..799, u16 ~crc; hy3131_v2_6.h)."""
    flash[MODEL_OFF] = MODELS.index(letter)
    c = dm40fw.crc16(bytes(flash[CAL_REC + 4:CAL_REC + CAL_REC_LEN]))
    struct.pack_into('<HH', flash, CAL_REC, c, c ^ 0xffff)


def compose_flash(mode, app, out_path, base=DUMP, zero_marker=False, model=None):
    flash = bytearray(read(base))
    assert len(flash) == 0x100000, 'flash dump must be 1 MiB'
    if model:
        set_model(flash, model)
    if app == 'dump':
        pass
    else:
        (w0, ln, w2), body = app
        assert len(body) <= LOADER_BASE - APP_BASE, 'app body reaches the FS-loader'
        off = APP_BASE - 0x08000000
        flash[off:LOADER_BASE - 0x08000000] = b'\xff' * (LOADER_BASE - APP_BASE)
        flash[off:off + len(body)] = body
        # The 12-byte marker the ROM/loader manage = the update header (type 1,
        # length, body CRC); the ROM launches the app only if it matches.
        flash[MARKER - 0x08000000:MARKER - 0x08000000 + 12] = struct.pack('<3I', w0, ln, w2)
    if zero_marker:
        flash[MARKER - 0x08000000:MARKER - 0x08000000 + 12] = b'\0' * 12
    with open(out_path, 'wb') as f:
        f.write(flash)
    return out_path


def build_fwrs(worktree=None, rev='HEAD'):
    """Build opendm40 native at `rev` of the main repo (default: its HEAD) in a
    separate worktree; skip the build when the ELF already matches the opendm40
    tree of that commit. A lock file serialises parallel runs (the worktree
    checkout is not reentrant)."""
    import fcntl
    wt = worktree or os.path.join(OUT, 'fwrs-worktree')
    commit = subprocess.check_output(['git', 'rev-parse', rev], cwd=ROOT, text=True).strip()
    # The firmware directory and ELF name of that commit (fw-rs/ and dm40-fw
    # before the firmware/ layout).
    sub = 'firmware' if subprocess.run(['git', 'cat-file', '-e', commit + ':firmware/build.sh'],
                                       cwd=ROOT).returncode == 0 else 'opendm40'
    cargo = subprocess.check_output(['git', 'show', commit + ':' + sub + '/Cargo.toml'], cwd=ROOT, text=True)
    name = re.search(r'^name = "([^"]+)"', cargo, re.M).group(1)
    tree = subprocess.check_output(['git', 'rev-parse', commit + ':' + sub], cwd=ROOT, text=True).strip()
    elf = os.path.join(wt, sub, 'target/native/thumbv7em-none-eabihf/release', name)
    stamp = os.path.join(wt, sub, 'target/native/built-tree')
    os.makedirs(OUT, exist_ok=True)
    with open(os.path.join(OUT, 'fwrs-worktree.lock'), 'w') as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        built = read(stamp).decode().strip() if os.path.exists(stamp) else ''
        if built == tree and os.path.exists(elf):
            print('opendm40: tree %s of %s already built (%s)' % (tree[:12], commit[:12], rel(elf)))
            return elf
        if not os.path.exists(os.path.join(wt, '.git')):
            subprocess.check_call(['git', 'worktree', 'add', '--detach', wt, commit], cwd=ROOT)
        else:
            subprocess.check_call(['git', 'checkout', '--quiet', '--detach', commit], cwd=wt)
        tc = os.path.join(wt, 'toolchain')
        if not os.path.exists(tc):
            os.symlink(os.path.join(ROOT, 'toolchain'), tc)
        print('opendm40: building %s (%s)' % (commit[:12], subprocess.check_output(
            ['git', 'log', '-1', '--format=%s', commit], cwd=ROOT, text=True).strip()))
        # `native`: the only build now (a no-op argument); older commits (--rev)
        # need it to select the native image instead of their RAM default.
        subprocess.check_call(['./build.sh', 'native'], cwd=os.path.join(wt, sub))
        with open(stamp, 'w') as f:
            f.write(tree + '\n')
    return elf


def merge_models():
    """All emu/peripherals/*.cs as ONE compile unit (Renode compiles each
    include separately: 15 files took 3.4 s, one file 1 s)."""
    usings, bodies = [], []
    for f in sorted(glob.glob(os.path.join(HERE, 'peripherals', '*.cs'))):
        body = []
        for l in read(f).decode().splitlines():
            if re.match(r'^using [A-Za-z0-9_.]+;\s*$', l):
                if l not in usings:
                    usings.append(l)
            else:
                body.append(l)
        bodies.append('// ---- %s ----\n%s' % (os.path.relpath(f, HERE), '\n'.join(body)))
    out = os.path.join(OUT, 'dm40-models.cs')
    text = '\n'.join(usings) + '\n' + '\n'.join(bodies) + '\n'
    if not os.path.exists(out) or read(out).decode() != text:
        # Write, then rename: a parallel run's Renode never compiles a
        # truncated file.
        tmp = '%s.%d.tmp' % (out, os.getpid())
        with open(tmp, 'w') as f:
            f.write(text)
        os.replace(tmp, out)
    return out


def q(path):
    return '@' + os.path.abspath(path)


class Script:
    def __init__(self):
        self.lines = []
        self.events = []  # (time, order, cmd lines)

    def add(self, line):
        self.lines.append(line)

    def at(self, t, *cmds):
        self.events.append((float(t), len(self.events), list(cmds)))

    def render(self, end, quit_at_end):
        out = list(self.lines)
        now = 0.0
        for t, _, cmds in sorted(self.events):
            if t > now:
                out.append('emulation RunFor "%.6f"' % (t - now))
                now = t
            out.extend(cmds)
        if end > now:
            out.append('emulation RunFor "%.6f"' % (end - now))
        if quit_at_end:
            out.append('quit')
        else:
            out.append('start')
        return '\n'.join(out) + '\n'


def main(argv=None):
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument('mode', choices=['stock', 'fwrs', 'chain'])
    ap.add_argument('--flash', help='use this 1 MiB flash image as-is (no composition)')
    ap.add_argument('--base', default=DUMP, help='1 MiB flash dump to compose from')
    ap.add_argument('--app', help='application: .dec.bin / packed dm40*.bin / body / ELF, or "dump"')
    ap.add_argument('--rev', default='HEAD', help='fwrs: build opendm40 from this commit of the main repo (default HEAD)')
    ap.add_argument('--disk', help='16 MiB SPI-flash image (default: working copy of the MSC dump in emu/out/)')
    ap.add_argument('--disk-add', action='append', default=[], metavar='FILE[::NAME]',
                    help='copy FILE into the working disk image (mtools), e.g. opendm40.bin::DM40_FW.BIN')
    ap.add_argument('--model', type=str.upper, choices=list(MODELS),
                    help='DM40 model A|B|C: patch the calibration model byte (default: the dump\'s own)')
    ap.add_argument('--zero-marker', action='store_true',
                    help='zero the 12-byte loader marker at 0x08008000 (chain: the FS-loader then installs dm40*.bin)')
    ap.add_argument('--save-disk', help='write the SPI-flash image here at the end')
    ap.add_argument('--save-flash', help='write the internal flash here at the end')
    ap.add_argument('--uid', help='chip UID at 0x1FFF7A10: 24 hex digits (memory order), a 12-byte file, or "unit" '
                                  '(the unit\'s own UID file as is); default: the unit\'s UID with bytes 4 / 9 = '
                                  '--usbip-port (emu_uid: a USB serial of its own, the stock genuine check still '
                                  'passes)')
    ap.add_argument('--otp', default=OTP, help='OTP dump (0x1FFF7800)')
    ap.add_argument('--run-for', type=float, default=3.0, help='virtual seconds to run (default 3)')
    ap.add_argument('--shot', action='append', default=[], metavar='PNG[@T]',
                    help='screenshot at the end (or at virtual time T)')
    ap.add_argument('--autoshot', type=int, default=0, metavar='N', help='PNG every N frames into emu/out/frames/')
    ap.add_argument('--tap', action='append', default=[], metavar='X,Y[@T]', help='tap the screen at T (default 2.5 s)')
    ap.add_argument('--drag', action='append', default=[], metavar='X0,Y0,X1,Y1[@T]', help='drag over 0.3 s')
    ap.add_argument('--key', action='append', default=[], metavar='NAME[@T]',
                    help='side key menu|run|auto|mf, or power (PD6), or pa2')
    ap.add_argument('--vin', action='append', default=[], metavar='V[@T]', help='DMM input voltage (DC V on V/COM)')
    ap.add_argument('--dmm', action='append', default=[], metavar='SPEC[@T]',
                    help='what is connected to the DMM jacks (DM40 front-end model, sysbus.hy3131 SetInput): '
                         'vsrc:1.234[,ac=1.0,f=50,shape=square,duty=0.3,rs=4700] | isrc:0.0123 | r:4700 | c:100e-9 | '
                         'diode:0.65[,rev=1] | tc:100 | cj:25 | die:35 | avsrc:0.5 | short | open; several with ";" '
                         '(elements add up); the older dcv:/acv:/dca:/aca:/ohm:/cap:/hz:/temp: keys still work '
                         '(see emu/hy3131/README.md)')
    ap.add_argument('--scope', action='append', default=[], metavar='SPEC[@T]',
                    help='scope input waveform, e.g. sine,1000,1.0 (shape,Hz,Vpp[,offset])')
    ap.add_argument('--script', help='event file: lines "<t> <tap|drag|key|shot|vin|scope|monitor> args"')
    ap.add_argument('--gdb', nargs='?', const=3333, type=int, help='start a GDB server (port)')
    ap.add_argument('--interactive', action='store_true', help='leave the Renode monitor open')
    ap.add_argument('--pause', action='store_true', help='do not start the CPU (use with --gdb)')
    ap.add_argument('--bt-port', type=int, default=0, help='TCP port for the Bluetooth UART')
    ap.add_argument('--usbip', type=float, default=None, metavar='T',
                    help='start a USB/IP server (TCP 3240) at virtual time T and export the USB device as busid 1-0 '
                         '(use with a long --run-for or --interactive); 0 = from the start')
    ap.add_argument('--spi-disk-gap-ns', type=float, default=330, metavar='NS',
                    help='SPI disk bus: delay of a byte written to an idle shifter (default 330 = the device\'s ~0.97 us '
                         'per polled byte at 12.5 MHz; 0 = the old model)')
    ap.add_argument('--usbip-port', type=int, default=3240, metavar='P', help='USB/IP server TCP port (default 3240)')
    ap.add_argument('--usbip-renode', action='store_true',
                    help="use Renode's own USB/IP server instead of the model's (peripherals/DWC2_OTG_USBIP.cs); "
                         'the kernel (usbip attach) cannot enumerate through it')
    ap.add_argument('--log', help='Renode log file (default emu/out/<name>.log)')
    ap.add_argument('--buzzer-log', metavar='FILE',
                    help='log TIM14 ARR / CCR1 changes (the buzzer pitch / duty) as "<virtual ms> <reg> <value>"')
    ap.add_argument('--rail-log', action='store_true',
                    help='log every change of the analog rails PE2 / PC4 (the power hold: both low = the meter '
                         'switches off on the device) as "RAIL PE2 <0|1> PC4 <0|1> t=<virtual time>" warnings')
    ap.add_argument('--name', help='base name of the generated .resc/.log (default: the mode)')
    ap.add_argument('--pa2', type=float, default=None, metavar='T',
                    help='chain: release PA2 (power key) at T seconds; default 0.55 = ROM -> FS-loader -> app; '
                         '0 = ROM -> app directly; never (-1) = FS-loader install screen')
    ap.add_argument('--debug', default='', metavar='P1,P2',
                    help='peripherals to log at debug level, e.g. sysbus.i2c3,sysbus.i2c3.touch')
    ap.add_argument('--trace-boot', action='store_true',
                    help='log when the ROM, FS-loader and app reset vectors are entered')
    ap.add_argument('--pc-samples', type=int, default=0, metavar='N',
                    help='debug: print N PC samples (13 ms apart) at the end')
    ap.add_argument('--fast', action='store_true',
                    help='screenshot mode: --mips 50 (the idle GUI loops run 4x fewer iterations; '
                         'UI work takes up to 4x longer in virtual time, so allow ~1 s more)')
    ap.add_argument('--mips', type=float, default=None,
                    help='CPU PerformanceInMips (default 200 = real speed; lower = fewer busy-loop '
                         'iterations per virtual ms, faster wall clock, compute takes longer virtual time)')
    ap.add_argument('--quantum', default=None, help='emulation global quantum in seconds (e.g. 0.0005)')
    ap.add_argument('--stats', action='store_true', help='print virtual time, instructions and wall time')
    ap.add_argument('--live', metavar='FILE', help='export the LCD to a memory-mapped file for emu/live.py')
    ap.add_argument('--dry', action='store_true', help='only write the .resc')
    a = ap.parse_args(argv)

    os.makedirs(OUT, exist_ok=True)
    if not os.path.exists(RENODE):
        sys.exit('Renode missing: run emu/get-renode.sh')

    # ---- flash image ----
    if a.flash:
        flash = a.flash
        if a.model:  # patched copy; the given image is not changed
            img = bytearray(read(a.flash))
            set_model(img, a.model)
            flash = os.path.join(OUT, 'flash-%s-model.bin' % (a.name or a.mode))
            with open(flash, 'wb') as f:
                f.write(img)
    else:
        if a.app == 'dump':
            app = 'dump'
        elif a.app:
            app = app_from_file(a.app)
        elif a.mode == 'fwrs':
            app = app_from_file(build_fwrs(rev=a.rev))
        else:
            app = app_from_file(STOCK_DEC)
        flash = compose_flash(a.mode, app, os.path.join(OUT, 'flash-%s.bin' % (a.name or a.mode)), a.base,
                              a.zero_marker, a.model)
    print('flash image:', rel(flash), '(model DM40%s)' % flash_model(read(flash)))

    # ---- SPI disk ----
    disk = a.disk or os.path.join(OUT, 'disk.bin')
    if not a.disk and not os.path.exists(disk):
        # Copy, then rename: a parallel run never loads a half-copied image.
        tmp = '%s.%d.tmp' % (disk, os.getpid())
        shutil.copy(DISK_SRC, tmp)
        os.replace(tmp, disk)
    if a.disk_add:
        # Work on a private copy so the shared working image is not changed.
        priv = os.path.join(OUT, 'disk-%s.bin' % (a.name or a.mode))
        shutil.copy(disk, priv)
        disk = priv
        for spec in a.disk_add:
            src, _, name = spec.partition('::')
            name = name or os.path.basename(src).upper()
            subprocess.check_call(['mcopy', '-o', '-i', disk, src, '::' + name], env=dict(os.environ, MTOOLS_SKIP_CHECK='1'))
            print('disk: added %s as %s' % (rel(src), name))
    print('disk image: ', rel(disk))

    s = Script()
    name = a.name or a.mode
    log = a.log or os.path.join(OUT, '%s.log' % name)
    s.add('logFile %s' % q(log))
    s.add('include %s' % q(merge_models()))
    s.add('mach create "dm40"')
    s.add('machine LoadPlatformDescription %s' % q(os.path.join(HERE, 'dm40.repl')))
    s.add('sysbus LoadBinary %s 0x08000000' % q(flash))
    if os.path.exists(a.otp):
        s.add('sysbus LoadBinary %s 0x1FFF7800' % q(a.otp))
    if not a.uid and not os.path.exists(UID):
        print('warning: no UID file (%s): the stock genuine check fails -> 16 MHz' % UID)
    uid = uid_arg(a.uid, a.usbip_port)
    for i in range(3):
        s.add('sysbus WriteDoubleWord 0x%08X 0x%08X' % (0x1FFF7A10 + 4 * i, struct.unpack_from('<I', uid, 4 * i)[0]))
    print('chip UID:   ', uid.hex().upper(), '(opendm40 USB serial)')
    s.add('sysbus WriteWord 0x1FFF7A22 0x0400')
    # DM40_BTRACE=DIR (firmware/tools/region-profile/rprof.py --trace): record
    # the code blocks executed while hy3131::HANDED is set (the AD2 stream
    # ISR owns the bus) into DIR/<name>-<pid>.btrace (peripherals/BTrace.cs).
    # Needs an ELF --app (the HANDED address from its symbols).
    if os.environ.get('DM40_BTRACE') and a.app and read(a.app)[:4] == b'\x7fELF':
        handed = elf_symbol(a.app, 'opendm40::hy3131::HANDED')
        if handed is None:
            print('DM40_BTRACE: no hy3131::HANDED in %s' % a.app)
        else:
            os.makedirs(os.environ['DM40_BTRACE'], exist_ok=True)
            path = os.path.join(os.path.abspath(os.environ['DM40_BTRACE']), '%s-%d.btrace' % (name, os.getpid()))
            s.add('sysbus.btrace Arm 0x%08X %s' % (handed, q(path)))
            print('btrace:     ', rel(path))
    s.add('sysbus.spi2.spiflash Load %s' % q(disk))
    # The disk bus's write-wait-read byte as on the device (~0.97 us at 12.5
    # MHz, opendm40 spiflash.rs `xfer`; peripherals/GD32_SPI.cs IdleStartNs).
    s.add('sysbus.spi2 IdleStartNs %g' % a.spi_disk_gap_ns)
    s.add('sysbus.gpioPortD OnGPIO 6 false')  # power key released
    # Reset cause (peripherals/FWDGT.cs): RCU_RSTSCK's flag bits come from the
    # watchdog model (sticky across resets, cleared by RSTFC), and a
    # SYSRESETREQ (AIRCR VECTKEY + bit 2) marks the next reset as SW.
    # Each hook only on its register (a subrange): no Python call for the
    # other NVIC / RCC accesses (ISRs write ICPR / ICER).
    hooks = [('AfterPeripheralRead', 'rcc', 0x74, 'if offset == 0x74:\n'
              '    value = (value & 0x00FFFFFF) | sysbus.ReadDoubleWord(0x400033F0)'),
             ('BeforePeripheralWrite', 'rcc', 0x74, 'if offset == 0x74 and value & 0x01000000:\n'
              '    sysbus.WriteDoubleWord(0x400033F0, 0)'),
             ('BeforePeripheralWrite', 'nvic', 0xD0C, 'if offset == 0xD0C and (value & 0xFFFF0004) == 0x05FA0004:\n'
              '    sysbus.WriteDoubleWord(0x400033F4, 1)')]
    for kind, per, off, code in hooks:
        s.add('sysbus SetHook%s sysbus.%s """\n%s\n""" <0x%X +4>' % (kind, per, code, off))
    if a.mode == 'chain':
        # PA2 low while the power key is held at power-on: the ROM then runs the
        # FS-loader, which re-tests PA2 and launches the app once it is released.
        s.add('sysbus.gpioPortA OnGPIO 2 false')
    if a.fast and not a.mips:
        a.mips = 50
    if a.mips:
        s.add('sysbus.cpu PerformanceInMips %d' % int(a.mips))
    if a.quantum:
        s.add('emulation SetGlobalQuantum "%s"' % a.quantum)
    vtor = 0x08000000 if a.mode == 'chain' else APP_BASE
    s.add('cpu VectorTableOffset 0x%08X' % vtor)
    # SYSRESETREQ (the FS-loader after an install, opendm40 recovery): Renode runs
    # the machine's `reset` macro; the flash keeps its (possibly updated)
    # content, only the vector table base is restored.
    s.add('macro reset\n"""\n    cpu VectorTableOffset 0x%08X\n"""' % vtor)
    s.add('runMacro $reset')
    for p in [x for x in a.debug.split(',') if x]:
        s.add('logLevel -1 %s' % p)
    if a.trace_boot:
        # Log every transition between the ROM, app and FS-loader flash regions
        # (block granularity; the hook slows the run down a little).
        hook = ("global _boot_region\n"
                "r = 'ROM' if pc < 0x08004000 else ('app' if pc < 0x080A0000 else ('FS-loader' if pc < 0x080C0000 else 'other'))\n"
                "if pc >= 0x08000000 and pc < 0x08100000 and r != globals().get('_boot_region'):\n"
                "    _boot_region = r\n"
                "    self.Log(LogLevel.Info, 'boot: entered %s at 0x%08x, t=%s' % (r, pc, machine.ElapsedVirtualTime.TimeElapsed))\n")
        s.add('sysbus.cpu SetHookAtBlockBegin """\n%s"""' % hook)
    if a.buzzer_log:
        s.add('sysbus.timer14 LogWrites %s' % q(os.path.abspath(a.buzzer_log)))
    if a.rail_log:
        # A write to OCTL (0x14) or BOP (0x18) of GPIOE (PE2) or GPIOC (PC4):
        # the levels after it, logged when one of the two changes.
        for port, base, n in (('gpioPortE', 0x40021000, 2), ('gpioPortC', 0x40020800, 4)):
            code = ('e = sysbus.ReadDoubleWord(0x40021014) >> 2 & 1\n'
                    'c = sysbus.ReadDoubleWord(0x40020814) >> 4 & 1\n'
                    'old = %s\n'
                    'new = (value >> %d & 1) if offset == 0x14 else (1 if value & %d else (0 if value & %d else old))\n'
                    'if new != old:\n'
                    '    e, c = (new, c) if %d == 2 else (e, new)\n'
                    '    self.Log(LogLevel.Warning, "RAIL PE2 %%d PC4 %%d t=%%s" %% (e, c, '
                    'machine.ElapsedVirtualTime.TimeElapsed))'
                    % ('e' if n == 2 else 'c', n, 1 << n, 1 << (n + 16), n))
            s.add('sysbus SetHookBeforePeripheralWrite sysbus.%s """\n%s\n""" <0x14 +8>' % (port, code))
    if a.live:
        s.add('sysbus.lcd LiveFile %s 30' % q(a.live))
    if a.autoshot:
        s.add('sysbus.lcd AutoShot %s %d' % (q(os.path.join(OUT, 'frames')), a.autoshot))
    if a.bt_port:
        s.add('sysbus.btmodule Listen %d' % a.bt_port)
    if a.gdb:
        s.add('machine StartGdbServer %d' % a.gdb)
    # RunFor starts the emulation itself (it is refused after `start`); an
    # interactive session is left running at the end instead.

    def parse_at(spec, default):
        if '@' in spec:
            v, t = spec.rsplit('@', 1)
            return v, float(t)
        return spec, default

    def tap(t, x, y):
        s.at(t, 'sysbus.i2c3.touch Tap %d %d' % (int(x), int(y)))

    def drag(t, x0, y0, x1, y1):
        s.at(t, 'sysbus.i2c3.touch Drag %d %d %d %d 300' % (int(x0), int(y0), int(x1), int(y1)))

    def key(t, name):
        name = name.lower()
        keys = {'menu': 0, 'run': 1, 'auto': 2, 'mf': 3}
        if name in keys:
            s.at(t, 'sysbus.i2c3.touch Key %d' % keys[name])
        elif name == 'power':
            s.at(t, 'sysbus.gpioPortD OnGPIO 6 true')
            s.at(t + 0.2, 'sysbus.gpioPortD OnGPIO 6 false')
        elif name == 'power-hold':
            s.at(t, 'sysbus.gpioPortD OnGPIO 6 true')
            s.at(t + 2.0, 'sysbus.gpioPortD OnGPIO 6 false')
        elif name == 'pa2':
            s.at(t, 'sysbus.gpioPortA Release 2')  # floats high (pull-up)
        else:
            sys.exit('unknown key ' + name)

    def shot(t, path):
        s.at(t, 'sysbus.lcd Screenshot %s' % q(path))

    def vin(t, v):
        s.at(t, 'sysbus.hy3131 SetVin %s' % v)

    def dmm(t, spec):
        s.at(t, 'sysbus.hy3131 SetInput "%s"' % spec)

    def scope(t, spec):
        parts = spec.split(',')
        shape = parts[0]
        hz = float(parts[1]) if len(parts) > 1 else 1000.0
        vpp = float(parts[2]) if len(parts) > 2 else 1.0
        off = float(parts[3]) if len(parts) > 3 else 0.0
        s.at(t, 'sysbus.scopefeed SetWave "%s" %g %g %g' % (shape, hz, vpp, off))

    for spec in a.tap:
        v, t = parse_at(spec, 2.5)
        tap(t, *v.split(','))
    for spec in a.drag:
        v, t = parse_at(spec, 2.5)
        drag(t, *v.split(','))
    for spec in a.key:
        v, t = parse_at(spec, 2.5)
        key(t, v)
    for spec in a.vin:
        v, t = parse_at(spec, 0.0)
        vin(t, v)
    for spec in a.dmm:
        v, t = parse_at(spec, 0.0)
        dmm(t, v)
    for spec in a.scope:
        v, t = parse_at(spec, 0.0)
        scope(t, v)
    if a.usbip is not None:
        if a.usbip_renode:
            s.at(a.usbip, 'emulation CreateUSBIPServer %d' % a.usbip_port, 'host.usb Register sysbus.usbfs')
        else:
            s.at(a.usbip, 'sysbus.usbfs StartUSBIPServer %d' % a.usbip_port)
    if a.mode == 'chain':
        pa2 = 0.55 if a.pa2 is None else a.pa2
        if pa2 >= 0:
            key(pa2, 'pa2')
    if a.script:
        with open(a.script) as f:
            for line in f:
                line = line.split('#', 1)[0].strip()
                if not line:
                    continue
                parts = line.split()
                t = float(parts[0])
                cmd, args = parts[1], parts[2:]
                if cmd == 'tap':
                    tap(t, *args[:2])
                elif cmd == 'drag':
                    drag(t, *args[:4])
                elif cmd == 'key':
                    key(t, args[0])
                elif cmd == 'shot':
                    shot(t, args[0])
                elif cmd == 'vin':
                    vin(t, args[0])
                elif cmd == 'dmm':
                    dmm(t, args[0])
                elif cmd == 'scope':
                    scope(t, args[0])
                elif cmd == 'monitor':
                    s.at(t, ' '.join(args))
                else:
                    sys.exit('bad script line: ' + line)
    end = a.run_for
    for spec in a.shot:
        v, t = parse_at(spec, None)
        if t is None:
            s.at(end, 'sysbus.lcd Screenshot %s' % q(v))
        else:
            shot(t, v)
    if a.stats:
        s.at(end, 'sysbus.cpu ExecutedInstructions')
    if a.save_disk:
        s.at(end, 'sysbus.spi2.spiflash Save %s' % q(a.save_disk))
    if a.save_flash:
        s.at(end, 'sysbus.memtool Save %s 0x08000000 0x100000' % q(a.save_flash))

    resc = os.path.join(OUT, '%s.resc' % name)
    with open(resc, 'w') as f:
        f.write(s.render(end, not a.interactive))
    print('script:     ', rel(resc))
    if a.dry:
        return resc
    cmd = [RENODE, '--console', '--disable-xwt', '-e', 'i @' + os.path.abspath(resc)]
    if not a.interactive:
        cmd.insert(1, '--hide-log')
        cmd += ['-e', 'quit']  # also quit when the script aborts on an error
    import time
    t0 = time.time()
    rc = run_renode(cmd, quiet=not a.interactive, log=log)
    if a.stats:
        wall = time.time() - t0
        print('wall %.1f s for %.2f s virtual (%.1fx slower than real time)' % (wall, end, wall / end))
    return rc


# Renode log cap (run_renode): env DM40_EMU_LOG_MAX_MB, default 512.
LOG_MAX_MB = int(os.environ.get('DM40_EMU_LOG_MAX_MB', '512'))
LOG_KEEP_MB = 16


def kill_group(p, grace=2.0):
    """Terminate Renode's whole process group (Renode + its .NET children)."""
    if p.poll() is not None:
        return
    try:
        os.killpg(p.pid, signal.SIGTERM)
    except ProcessLookupError:
        return
    try:
        p.wait(grace)
    except subprocess.TimeoutExpired:
        try:
            os.killpg(p.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
        p.wait()


def run_renode(cmd, quiet=True, log=None):
    """Run Renode in its own process group; Ctrl-C / SIGTERM stop it (and every
    child) and exit non-zero. In quiet mode stdin stays an open pipe for the
    whole run: on EOF Renode's console reader spins and kills long runs
    ("semaphore ... maximum count")."""
    if quiet:
        p = subprocess.Popen(cmd, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                             stderr=subprocess.STDOUT, text=True, start_new_session=True)
    else:
        # Interactive: Renode owns the terminal, but in its own session so that
        # Ctrl-C reaches us (and we stop the group) rather than the monitor.
        p = subprocess.Popen(cmd, start_new_session=True)

    def on_signal(signum, _frame):
        raise KeyboardInterrupt

    # Log size cap: a run that logs in a loop (an exception per tick, a
    # peripheral warning per access) once wrote a 44 GB emu/out/*.log and
    # filled the disk (2026-10-22). Over LOG_MAX_MB the whole Renode group is
    # stopped and the log truncated to its first LOG_KEEP_MB.
    capped = []
    if log:
        import threading
        lim = LOG_MAX_MB << 20

        def watch():
            while p.poll() is None:
                try:
                    big = os.path.getsize(log) > lim
                except OSError:
                    big = False
                if big:
                    capped.append(1)
                    print('run.py: %s over %d MiB: stopping Renode (log spam?)' % (rel(log), LOG_MAX_MB), file=sys.stderr)
                    kill_group(p, 0.5)
                    try:
                        os.truncate(log, LOG_KEEP_MB << 20)
                    except OSError:
                        pass
                    return
                time.sleep(1.0)
        threading.Thread(target=watch, daemon=True).start()
    old = signal.signal(signal.SIGTERM, on_signal)
    old_int = signal.signal(signal.SIGINT, on_signal)  # also when started with SIGINT ignored
    try:
        out = p.stdout.read() if quiet else ''
        rc = p.wait()
        if capped:
            rc = rc or 3
    except KeyboardInterrupt:
        print('\ninterrupted: stopping Renode', file=sys.stderr)
        kill_group(p)
        return 130
    finally:
        signal.signal(signal.SIGTERM, old)
        signal.signal(signal.SIGINT, old_int)
        if quiet and p.stdin:
            try:
                p.stdin.close()
            except OSError:
                pass
        kill_group(p, 0.5)  # nothing may outlive us
    if quiet:
        for l in out.splitlines():
            l = re.sub(r'\x1b\[[0-9;]*m', '', l)
            if l.startswith('0x') or 'error' in l.lower() or 'screenshot' in l.lower():
                print(l)
        if log:
            print('log:        ', rel(log))
    return rc


if __name__ == '__main__':
    sys.exit(main())
