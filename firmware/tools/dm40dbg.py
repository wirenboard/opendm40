#!/usr/bin/env python3
"""Host side of the opendm40 debug commands (USBTMC, VID:PID 1209:D040; 1209:0001 before 0.1.0).

    dm40dbg.py ping
    dm40dbg.py hold [seconds]            keep the watchdog fed while running
    dm40dbg.py scpi 'CMD' ...            send SCPI, print replies to queries
    dm40dbg.py lcdread x y w h nwords    raw GRAM read-back (dummy word first)
    dm40dbg.py shot out.png [--format rgb565|rgb666]
    dm40dbg.py diag                      diagnostic block (hex)
    dm40dbg.py tap x y | left | right    inject a touch tap / view swipe
    dm40dbg.py drag x0 y0 x1 y1 [--ms N] [--hold MS]
                                         one-finger drag through the gesture
                                         tracker (DBG:DRAG): hold MS still at
                                         x0,y0, move to x1,y1 in N ms (300)
    dm40dbg.py key 0..3 [--hold MS]      side key click (0 MENU, 1 RUN, 2 AUTO,
                                         3 MF); --hold: release after MS ms
                                         (long press; RUN >= 1000 ms in a DMM
                                         view = AutoHold on/off)
    dm40dbg.py boot [--expect FILE] [--raw]
                                         DBG:BOOT? decoded (native); --expect
                                         compares the loader marker with FILE's
                                         update header ("installed OK")
    dm40dbg.py upload FILE               NATIVE: write FILE (an update file,
                                         checked first) as DM40FW.BIN into the
                                         SPI disk's root over USBTMC
                                         (DBG:UPLOAD, src/upload.rs; the USB
                                         disk is read-only); every other
                                         dm40*.bin there is deleted first
    dm40dbg.py update [--wait S] [--expect FILE]
                                         NATIVE: DBG:UPDATE = zero the loader
                                         marker + reset; the vendor loader then
                                         installs the one dm40*.bin of the SPI
                                         disk's root. --wait: wait for opendm40 to
                                         come back and print `boot`.
                                         tools/reload.sh does the whole flow
                                         (upload, update, check)
    dm40dbg.py image FILE                offline: validate a dm40*.bin update
                                         file (header CRC, body CRC, size)
    dm40dbg.py release                   stop feeding the watchdog: the WDT
                                         reset (~20 s) counts a crash and
                                         reboots opendm40
    dm40dbg.py reset                     reboot opendm40
    dm40dbg.py loader                    NATIVE: DBG:LOADER = reset into the
                                         vendor update screen through PA2; the
                                         marker is kept, a power cycle boots
                                         opendm40 again (`loader -h`)
    dm40dbg.py recover                   = loader (DBG:RECOVER)

Debug commands are SCPI DBG:... (see firmware/src/scpi.rs); needs read/write
access to /dev/usbtmcN (udev rule). Native boot: firmware/docs/native-boot.md.
"""
import argparse
import glob
import os
import signal
import struct
import sys
import time
import zlib

VID, PID = 0x1209, 0xD040
# Builds before 1209:D040 enumerated as the pid.codes test PID 0001.
PIDS = (0xD040, 0x0001)
STOCK_VID, STOCK_PID = 0x28e9, 0x028f   # stock "USB Connect" disk = loader update disk
W, H = 480, 320
FIRMWARE = os.path.join(os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__)))),
                        'stock')
APP_BASE, APP_END = 0x08004000, 0x080A0000

RECOVER_HELP = """\
opendm40 (native image at 0x08004000, docs/native-boot.md "PA2 entry"):
DBG:LOADER (alias DBG:RECOVER) resets; the shim then drives PA2 (the debug
header's TX pad) low, locks it and jumps to the Alientek ROM, which with the
FS-loader sees PA2 low as with a TX-GND short and shows the loader's update
screen with its own USB disk (28e9:028f). The 12-byte app marker is not
touched: copy a dm40*.bin (e.g. dm40_v_2_6.bin) onto that disk and press
"update", or power-cycle the meter (the loader's key switches it off) to boot
opendm40 again. A dm40*.bin already in the SPI disk's root is installed
straight away. DBG:UPDATE (`update`) instead zeroes the marker: the loader
then stays until a good install.

Back to opendm40 from stock: firmware/tools/reload.sh --from-stock (stages the image
on the stock disk).
"""


def usb_present(vid, pid):
    """True if a USB device vid:pid is enumerated (any interface)."""
    for f in glob.glob('/sys/bus/usb/devices/*/idVendor'):
        base = os.path.dirname(f)
        try:
            v = int(open(f).read(), 16)
            p = int(open(os.path.join(base, 'idProduct')).read(), 16)
        except (OSError, ValueError):
            continue
        if v == vid and (p == pid or (vid, pid) == (VID, PID) and p in PIDS):
            return True
    return False


def find_dev(required=True):
    """/dev/usbtmcN of the first opendm40 (1209:D040, or 1209:0001 of older builds). DM40_USBTMC=/dev/usbtmcN picks
    one; DM40_USBTMC=emu the emulator's (attached with `usbip attach`, under
    vhci_hcd), DM40_USBTMC=real the one that is not."""
    sel = os.environ.get('DM40_USBTMC', '')
    if sel.startswith('/dev/'):
        return sel
    for d in sorted(glob.glob('/sys/class/usbmisc/usbtmc*')):
        usb = os.path.realpath(os.path.join(d, 'device', '..'))
        if sel in ('emu', 'real') and ('vhci_hcd' in usb) != (sel == 'emu'):
            continue
        try:
            v = int(open(os.path.join(usb, 'idVendor')).read(), 16)
            p = int(open(os.path.join(usb, 'idProduct')).read(), 16)
        except OSError:
            continue
        if v == VID and p in PIDS:
            return '/dev/' + os.path.basename(d)
    if not required:
        return None
    sys.exit('opendm40 USBTMC device not found (is the firmware running and the cable in?)')


# linux/usb/tmc.h: _IO(USBTMC_IOC_NR = 91, 2)
USBTMC_IOCTL_CLEAR = 0x5B02


class Link:
    """USBTMC through the Linux usbtmc driver: one message per write, one
    reply per read.

    A reply longer than 4084 bytes (LOG:DATA?, MMEM:DATA? pieces, WAV:DATA?,
    DBG:LCD?) is read by the driver in two steps, and only the second can be
    interrupted by a signal; it then kills its URBs and leaves the rest of
    the reply on the meter. Every later read gets that stale rest first, the
    driver drops it ("wrong MsgID" / "wrong bTag" in dmesg) and returns 0
    bytes, and the meter's reply to it is read by the next query: each reply
    one query late, also for new processes, until a USBTMC device clear
    (TODO "dm40scpi.py log stopped by SIGINT", device c830e09). So `read`
    holds SIGINT for the duration of the read (Ctrl-C acts once the reply is
    in), and `query` takes an empty reply (opendm40 never sends one: every reply
    ends with '\\n'; a missing reply times out instead) as that state: it
    clears the device and asks once more."""

    def __init__(self, dev=None):
        self.fd = os.open(dev or find_dev(), os.O_RDWR)
        self.cleared = 0  # device clears done by `query` (desync repairs)

    def write(self, cmd):
        os.write(self.fd, cmd.encode() + b'\n')

    def write_raw(self, msg):
        """One message of raw bytes, nothing appended (DBG:UPLOAD:DATA)."""
        os.write(self.fd, msg)

    def read(self, n=1 << 16):
        """One reply. SIGINT stays pending until the read returns (a timeout
        ends it after the driver's 5 s), so Ctrl-C never cuts a reply."""
        try:
            old = signal.pthread_sigmask(signal.SIG_BLOCK, {signal.SIGINT})
        except (AttributeError, ValueError):  # not on this platform / thread
            return os.read(self.fd, n)
        try:
            return os.read(self.fd, n)
        finally:
            signal.pthread_sigmask(signal.SIG_SETMASK, old)

    def clear(self):
        """USBTMC INITIATE_CLEAR (+ the driver's CHECK_CLEAR_STATUS loop,
        reading the Bulk-IN until a short packet): the meter forgets any
        command, pending query and reply in progress (src/usbtmc.rs clear)."""
        import fcntl
        fcntl.ioctl(self.fd, USBTMC_IOCTL_CLEAR)

    def query(self, cmd):
        self.write(cmd)
        r = self.read()
        if r:
            return r
        # Out of step with the meter (see the class doc): clear and ask again.
        self.clear()
        self.cleared += 1
        print('dm40: empty USBTMC reply to %s: device cleared, asked again' % cmd.split(None, 1)[0][:40],
              file=sys.stderr)
        self.write(cmd)
        return self.read()

    def fresh(self):
        """Start a session from a known state: a USBTMC device clear (an
        earlier client may have died in the middle of a message or a reply:
        Linux usbtmc sends no clear on close), then *CLS. The SCPI error
        queue is a FIFO that lives across clients, so without this the first
        SYST:ERR? of a session reports an error some earlier client left
        (TODO "reload.sh first-upload failures": a stale -113 "Undefined
        header" read as DBG:UPLOAD:BEGIN's). The stale errors are returned
        (read before the *CLS), for the record."""
        self.clear()
        stale = []
        for _ in range(9):  # opendm40 keeps 8 (scpi.rs ERRQ) + 1
            e = self.query('SYST:ERR?').decode(errors='replace').strip()
            if not e or e.startswith(('0,', '+0,')):
                break
            stale.append(e)
        self.write('*CLS')
        return stale

    def try_query(self, cmd):
        """(reply, None) or (None, error text). An unknown query gets no reply:
        the usbtmc read times out (~5 s); the error is then taken from
        SYST:ERR?."""
        try:
            r = self.query(cmd)
        except OSError as e:
            r, why = b'', str(e)
        else:
            why = 'empty reply'
        if r.strip():
            return r, None
        try:
            err = self.query('SYST:ERR?').decode(errors='replace').strip()
        except OSError as e:
            err = str(e)
        return None, '%s; SYST:ERR? -> %s' % (why, err)

    def block(self, cmd):
        """Query returning an IEEE 488.2 definite block (#Nlen...)."""
        d = self.query(cmd)
        if d[:1] != b'#':
            raise IOError('not a block: %r' % d[:32])
        nd = int(d[1:2])
        n = int(d[2:2 + nd])
        d = d[2 + nd:]
        while len(d) < n:
            d += self.read()
        return d[:n]

    def ping(self, keepalive_ms=0):
        up, flags, left = (int(v) for v in self.query('DBG:PING? %d' % keepalive_ms).split(b','))
        return dict(uptime_ms=up, pllsai=bool(flags & 1), usb_core=bool(flags & 2),
                    keepalive_left_ms=left)

    def lcdread(self, x, y, w, h, nwords):
        d = self.block('DBG:LCD? %d,%d,%d,%d,%d' % (x, y, w, h, nwords))
        return list(struct.unpack('<%dH' % (len(d) // 2), d))


def open_link(timeout):
    """Wait up to `timeout` s for the USBTMC device and open it (udev may
    need a moment to set the permissions after enumeration)."""
    t_end = time.time() + timeout
    while True:
        dev = find_dev(required=False)
        if dev:
            try:
                return Link(dev)
            except OSError:
                pass
        if time.time() > t_end:
            return None
        time.sleep(0.5)


# ---- update files ----------------------------------------------------------

def image_info(path):
    """Validate a dm40*.bin update file (the vendor format, stock/dm40fw.py).
    Returns dict(header=(w0, length, w2), sp, reset, end, last, problems=[...]);
    the header is what the loader writes to the marker at 0x08008000; last =
    the length of the last 128-byte obfuscation block (128 = whole blocks). A
    short last block whose key position is outside it (dm40fw.tail_hazard) is
    a problem: the FS-loader decodes it with a stale key, the body CRC fails
    and the app is erased."""
    sys.path.insert(0, FIRMWARE)
    import dm40fw
    data = open(path, 'rb').read()
    try:
        w0, length, w2, body = dm40fw.split(dm40fw.decode(data))
    except (ValueError, struct.error) as e:
        return dict(header=None, problems=['not an update file (%s) -- a raw body '
                                           'or a decoded file?' % e])
    probs = []
    last = len(data) % dm40fw.BLOCK or dm40fw.BLOCK
    haz = dm40fw.tail_hazard(data)
    if haz:
        n, ka, kp = haz
        probs.append('short last block (%d bytes) with key position 0x%02x outside it '
                     '([0x1F] = 0x%02x): the FS-loader mis-decodes it ("update error", app '
                     'erased); repack with stock/dm40fw.py pack --pad' % (n, kp, ka))
    if w0 != dm40fw.header_word0(length, w2):
        probs.append('header word0 (CRC of words 1/2) wrong: 0x%08x' % w0)
    if w2 >> 16 != 1 or (w2 & 0xFFFF) != dm40fw.body_checksum(body):
        probs.append('word2 0x%08x != 0x1%04x (body CRC)' % (w2, dm40fw.body_checksum(body)))
    end = APP_BASE + length
    if end > APP_END:
        probs.append('body ends at 0x%08x, past 0x%08x (loader)' % (end, APP_END))
    sp, reset = struct.unpack_from('<2I', body, 0) if len(body) >= 8 else (0, 0)
    if not (reset & 1 and APP_BASE <= reset - 1 < end):
        probs.append('reset vector 0x%08x not a thumb address inside the body' % reset)
    return dict(header=(w0, length, w2), sp=sp, reset=reset, end=end, last=last, problems=probs)


def print_image(path):
    i = image_info(path)
    print('%s:' % path)
    if i['header']:
        w0, length, w2 = i['header']
        print('  header (= loader marker after install): 0x%08X 0x%08X 0x%08X' % (w0, length, w2))
        print('  body 0x%08x..0x%08x (%d bytes), SP 0x%08x, reset 0x%08x'
              % (APP_BASE, i['end'], length, i['sp'], i['reset']))
        print('  last 128-byte block: %d bytes%s' % (i['last'], ' (whole blocks)' if i['last'] == 128 else ''))
    for p in i['problems']:
        print('  BAD: ' + p)
    print('  OK' if not i['problems'] else '  -> do NOT install this file')
    return not i['problems']


# ---- DBG:BOOT? -------------------------------------------------------------

# DBG:BOOT? (boot.rs put_info + native.rs boot_words): "B2," then hex
# words (no prefix) in this order, then fatren's renamed 8.3 name.
BOOT_FIELDS = ['mode', 'reason', 'clock', 'rstsck', 'rstsck_after', 'last', 'last_trigger',
               'marker_w0', 'marker_w1', 'marker_w2', 'marker_valid', 'crash_word', 'healthy', 'fmc_last',
               'bcr1', 'btr1', 'bwtr1', 'lcd_id',
               'fatren', 'found', 'old_backup', 'old_deleted', 'freed_clusters', 'others', 'rcvr_deleted', 'staged',
               'calbk', 'calbk_written', 'calbk_conflict', 'calbk_differs', 'calbk_ms',
               'stack_used', 'stack_size', 'spiclk_div', 'spiclk_check',
               'fatdir_ok', 'fatdir_created', 'fatdir_failed', 'calrec_ok',
               'settings_from', 'settings_slot', 'settings_seq', 'setfile', 'save', 'saves', 'save_top',
               'ms_board', 'ms_fatren', 'ms_calbk', 'ms_fatdir', 'ms_setfile', 'ms_mirror', 'ms_vdisk', 'ms_refresh',
               'main_app', 'main_frame', 'main_touch']
# Enum fields: the firmware's declaration order (as u32).
BOOT_ENUMS = {
    'mode': ['NORMAL'],  # 1 was the removed recovery mode
    'reason': ['NONE', 'WDT_AFTER_LOADER', 'WDT_STALE', 'WDT_RETRY', 'KEY_HOLD_ABORTED', '-', 'CLOCK_FAIL'],
    'clock': ['HXTAL', 'IRC16M', 'FAILED'],
    'fatren': ['NOTRUN', 'NOMEDIUM', 'BADBPB', 'CLEAN', 'DRYRUN', 'RENAMED', 'ERROR'],
    'calbk': ['NOTRUN', 'NOMEDIUM', 'BADBPB', 'SAFECOPY_PRESENT', 'SAFECOPY_WRITTEN', 'NOSPACE', 'ERROR'],
    'spiclk_check': ['NOTRUN', 'OK', 'FORCED', 'NOCHIP', 'NOSIG', 'DIFF1', 'DIFF2'],
    'settings_from': ['NOTRUN', 'BKP', 'FILE', 'DEFAULTS'],
    'setfile': ['NOTRUN', 'PRESENT', 'CREATED', 'NOFILE', 'ERROR'],
    'save': ['NONE', 'OK', 'DEFERRED', 'BADRECORD', 'ERROR', 'NOFILE'],
}
# spiflash.rs ClockCheck (spiclk_check).
SPICLK_CHECK = {'OK': '/4 (12.5 MHz) read block 0 + FAT block equal to /16 twice',
                'FORCED': 'spiflash::FORCE_SLOW: stock /16',
                'NOCHIP': 'no known chip (JEDEC ID): /16',
                'NOSIG': 'no 0x55AA in block 0 at /16 (blank/foreign disk): /16, not checked',
                'DIFF1': '1st /4 read DIFFERED from /16: fell back to /16',
                'DIFF2': '2nd /4 read DIFFERED from /16: fell back to /16',
                'NOTRUN': 'no SpiFlash created yet'}
RST_FLAGS = [(31, 'LP'), (30, 'WWDGT'), (29, 'FWDGT'), (28, 'SW'), (27, 'POR'), (26, 'EP'), (25, 'BOR')]
# calbackup (write-once safety copy) written / conflict / differs bits.
CALBK_FILES = ['CALIB', 'BOOTDATA', 'UID']
CALBK_STATUS = {'SAFECOPY_PRESENT': 'safety copy on the SPI disk, nothing written',
                'SAFECOPY_WRITTEN': 'missing safety-copy files written',
                'NOSPACE': 'no room on the SPI disk for the safety copy',
                'BADBPB': 'SPI disk is not the expected FAT16: untouched',
                'NOMEDIUM': 'no SPI flash', 'ERROR': 'SPI disk read/write/verify failed',
                'NOTRUN': 'not run'}
TRIGGERS = {0x54520012: 'PA2 entry into the loader', 0x54520013: 'PA2 entry, GPIO lock NOT confirmed'}


def parse_boot(text):
    """DBG:BOOT? -> (dict name -> int, with the enum fields as their names
    and 'renamed' as text; warnings). None for 'NONE'."""
    text = text.strip()
    if text == 'NONE':
        return None, []
    t = text.split(',')
    warn = []
    if t[0] != 'B2':
        return {}, ['not the B2 format (%s): firmware older or newer than this decoder' % t[0]]
    vals, renamed = t[1:-1], t[-1]
    if len(vals) != len(BOOT_FIELDS):
        warn.append('%d words, decoder knows %d: names may be off' % (len(vals), len(BOOT_FIELDS)))
    f = {}
    for i, v in enumerate(vals):
        name = BOOT_FIELDS[i] if i < len(BOOT_FIELDS) else 'word%d' % i
        x = int(v, 16)
        e = BOOT_ENUMS.get(name)
        f[name] = (e[x] if x < len(e) else '?%d' % x) if e else x
    f['renamed'] = renamed
    return f, warn


def _note(name, x, f):
    if name in ('rstsck', 'rstsck_after'):
        return ' '.join(n for b, n in RST_FLAGS if x >> b & 1) or 'no flags'
    if name == 'last':
        b = struct.pack('<I', x)
        return b.decode() if all(0x20 <= c < 0x7F for c in b) else ('none' if x == 0 else '')
    if name == 'crash_word':
        return 'count %d' % (x & 0xFF) if x >> 8 == 0x435200 else 'not initialised'
    if name == 'fmc_last':
        return 'clean' if x == 0 else 'FMC error/timeout'
    if name == 'last_trigger':
        return TRIGGERS.get(x, 'none' if x == 0 else '?')
    if name in ('calbk_written', 'calbk_conflict', 'calbk_differs'):
        s = '+'.join(n for i, n in enumerate(CALBK_FILES) if x >> i & 1) or '-'
        if name == 'calbk_differs' and x:
            s += ' (copy kept: write-once; the flash changed after it was made)'
        return s
    if name == 'calbk':
        return CALBK_STATUS.get(x, '?')
    if name == 'spiclk_div':
        return {4: '12.5 MHz', 16: '3.125 MHz (stock)'}.get(x, '?')
    if name == 'spiclk_check':
        return SPICLK_CHECK.get(x, '?')
    if name == 'calbk_ms':
        return 'DWT at 200 MHz; 12.5x low on IRC16M'
    if name == 'stack_size' and x and f.get('stack_used'):
        return '%.0f %% used' % (100.0 * f['stack_used'] / x)
    return ''


BOOT_GROUPS = [
    ('boot', ['mode', 'reason', 'clock']),
    ('reset flags', ['rstsck', 'rstsck_after']),
    ('previous boot', ['last', 'last_trigger']),
    ('crash', ['crash_word', 'healthy', 'fmc_last']),
    ('loader marker', ['marker_w0', 'marker_w1', 'marker_w2', 'marker_valid']),
    ('fatren', ['fatren', 'found', 'renamed', 'old_backup', 'old_deleted', 'freed_clusters',
                'others', 'rcvr_deleted', 'staged']),
    ('calbackup', ['calbk', 'calbk_written', 'calbk_conflict', 'calbk_differs', 'calbk_ms']),
    ('calibration record', ['calrec_ok']),
    ('settings', ['settings_from', 'settings_slot', 'settings_seq', 'setfile', 'save', 'saves', 'save_top']),
    ('fatdir', ['fatdir_ok', 'fatdir_created', 'fatdir_failed']),
    ('boot phases ms', ['ms_board', 'ms_fatren', 'ms_calbk', 'ms_fatdir', 'ms_setfile', 'ms_mirror',
                        'ms_vdisk', 'ms_refresh', 'main_app', 'main_frame', 'main_touch']),
    ('stack', ['stack_used', 'stack_size']),
    ('SPI flash clock', ['spiclk_div', 'spiclk_check']),
    ('FSMC/LCD seen', ['bcr1', 'btr1', 'bwtr1', 'lcd_id']),
]
HEX_FIELDS = {'rstsck', 'rstsck_after', 'last', 'last_trigger', 'marker_w0', 'marker_w1', 'marker_w2',
              'crash_word', 'fmc_last', 'bcr1', 'btr1', 'bwtr1', 'lcd_id'}


def print_boot(text, expect=None, raw=False):
    """Print DBG:BOOT? decoded. Returns True/False for the --expect check
    (None without --expect)."""
    if raw:
        print(text)
    f, warn = parse_boot(text)
    if f is None:
        print('DBG:BOOT? -> NONE (no boot info: not booted through the shim)')
        return None if expect is None else False
    for w in warn:
        print('warning: ' + w)
    shown = set()
    for title, names in BOOT_GROUPS:
        parts = []
        for nm in names:
            if nm not in f:
                continue
            shown.add(nm)
            v = f[nm]
            if nm == 'renamed':
                txt = repr(v.strip()) if v.strip() else '-'
            elif isinstance(v, int) and nm in HEX_FIELDS:
                txt = '0x%08X' % v
            else:
                txt = str(v)
            n = _note(nm, v, f) if nm != 'renamed' else ''
            parts.append('%s=%s%s' % (nm, txt, ' (%s)' % n if n else ''))
        if parts:
            print('  %-15s %s' % (title + ':', '  '.join(parts)))
    rest = ['%s=%s' % (k, v) for k, v in f.items() if k not in shown]
    if rest:
        print('  %-15s %s' % ('other:', '  '.join(rest)))
    if expect is None:
        return None
    info = image_info(expect)
    want = info['header']
    have = tuple(f.get('marker_w%d' % i) for i in range(3))
    print('  expected marker (header of %s): %s' % (
        os.path.basename(expect), ' '.join('0x%08X' % w for w in want) if want else 'n/a (bad file)'))
    if want and have == tuple(want) and f.get('marker_valid') == 1:
        print('installed OK: the loader marker is the header of %s' % expect)
        return True
    print('NOT the expected image: marker %s %s' % (
        ' '.join('0x%08X' % f['marker_w%d' % i] if isinstance(f.get('marker_w%d' % i), int) else '?' for i in range(3)),
        'VALID' if f.get('marker_valid') == 1 else 'INVALID'))
    return False


UPLOAD_CHUNK = 232  # 256-byte USBTMC command buffer - 'DBG:UPLOAD:DATA <offset>,'


def syst_err(ln):
    r, err = ln.try_query('SYST:ERR?')
    return r.decode(errors='replace').strip() if r else err


def cmd_upload(ln, path, quiet=False):
    """DBG:UPLOAD BEGIN / DATA (raw binary, 232 bytes a message) / END?:
    `path` becomes DM40FW.BIN in the SPI disk's root (src/upload.rs)."""
    data = open(path, 'rb').read()
    crc = zlib.crc32(data)
    stale = ln.fresh()
    if stale:
        print('dm40: %d error(s) an earlier client left in the meter\'s queue (cleared, not from this upload): %s'
              % (len(stale), '; '.join(stale)), file=sys.stderr)
    ln.write('DBG:UPLOAD:BEGIN %d,%d' % (len(data), crc))
    e = syst_err(ln)
    if not e.startswith('0,'):
        hint = ''
        if 'Undefined header' in e and ln.try_query('DBG:UPLOAD?')[0] is None:
            hint = ' (DBG:UPLOAD? does not answer either: a firmware without DBG:UPLOAD?)'
        sys.exit('DBG:UPLOAD:BEGIN failed: the meter reports SCPI error %s%s' % (e, hint))
    t0 = time.time()
    for i, off in enumerate(range(0, len(data), UPLOAD_CHUNK)):
        ln.write_raw(b'DBG:UPLOAD:DATA %d,' % off + data[off:off + UPLOAD_CHUNK])
        if i % 256 == 255:
            e = syst_err(ln)
            if not e.startswith('0,'):
                sys.exit('DBG:UPLOAD:DATA failed at offset %d: the meter reports SCPI error %s' % (off, e))
            if not quiet:
                print('\r%d / %d bytes' % (off + UPLOAD_CHUNK, len(data)), end='', flush=True)
    r, err = ln.try_query('DBG:UPLOAD:END?')
    if not quiet:
        print()
    want = '%d,%d' % (len(data), crc)
    got = r.decode(errors='replace').strip() if r else None
    if got != want:
        sys.exit('DBG:UPLOAD:END? failed: %s (want %s)' % (got or 'the meter reports %s' % err, want))
    print('uploaded %s: %d bytes, CRC-32 %08x, as DM40FW.BIN in %.1f s' % (path, len(data), crc, time.time() - t0))
    return 0


def cmd_update(ln, wait, expect):
    """DBG:UPDATE; True once the device dropped off USB (= marker zeroed, reset)."""
    ln.fresh()  # the SYST:ERR? below must be DBG:UPDATE's, not an earlier client's
    ln.write('DBG:UPDATE')
    # The reset waits for pending disk writes first (a CSV recording's last
    # commit, a screenshot: app::finish_disk_writes, up to ~14 s).
    t_end = time.time() + 20
    while time.time() < t_end:
        if not usb_present(VID, PID):
            break
        time.sleep(0.1)
    else:
        r, err = ln.try_query('SYST:ERR?')
        sys.exit('DBG:UPDATE: the device did not reset (SYST:ERR? -> %s).\n'
                 '"Flash error" = the marker did not read back as zero (nothing changed);\n'
                 '"Undefined header" = an opendm40 build without DBG:UPDATE.'
                 % (r.decode(errors='replace').strip() if r else err))
    try:
        os.close(ln.fd)
    except OSError:
        pass  # usbtmc's flush on close fails with ENODEV once the device is gone
    print('DBG:UPDATE: marker zeroed, device reset; the vendor loader installs the dm40*.bin.')
    if not wait:
        print('Wait for opendm40 to re-enumerate (1209:D040), then: dm40dbg.py boot --expect FILE')
        return 0
    print('waiting up to %d s for opendm40 (first boot after an install is slow and dark: '
          'do not power-cycle) ...' % wait)
    t0 = time.time()
    stock_since = None
    while True:
        if usb_present(VID, PID) and find_dev(required=False):
            break
        el = time.time() - t0
        if usb_present(STOCK_VID, STOCK_PID):
            stock_since = stock_since or time.time()
            if time.time() - stock_since > 20:
                print('28e9:028f (vendor loader / stock disk) has been up for 20 s instead of opendm40:\n'
                      '  the loader update screen means the install failed or found no file\n'
                      '  (bad/missing dm40*.bin), or a stock image was installed. To retry:\n'
                      '  tools/reload.sh --from-stock --no-build  (stages opendm40.bin on\n'
                      '  that disk), then press "update" on the meter.')
                return 5
        else:
            stock_since = None
        if el > wait:
            print('timeout: opendm40 did not come back within %d s. Look at the meter: loader\n'
                  'screen ("open file error"/"update error") -> copy opendm40.bin as\n'
                  'dm40_opendm40_v<version>.bin (any dm40*.bin) onto its disk and press "update".' % wait)
            return 4
        time.sleep(0.5)
    print('opendm40 is back after %.0f s' % (time.time() - t0))
    ln = open_link(15)
    if ln is None:
        print('USBTMC device present but cannot be opened (udev permissions?)')
        return 6
    r, err = ln.try_query('DBG:BOOT?')
    if r is None:
        print('DBG:BOOT? failed: %s' % err)
        return 6
    ok = print_boot(r.decode(errors='replace'), expect)
    return 0 if ok in (None, True) else 3


def decode(words, fmt, npix):
    """Raw RAMRD words (dummy dropped) -> list of (r, g, b) 8-bit."""
    px = []
    if fmt == 'rgb565':
        for v in words[:npix]:
            px.append(((v >> 8) & 0xF8, (v >> 3) & 0xFC, (v << 3) & 0xF8))
    else:  # rgb666: one byte per component, packed big-endian into the words
        b = b''.join(struct.pack('>H', v) for v in words)
        for i in range(npix):
            r, g, bl = b[3 * i:3 * i + 3]
            px.append((r & 0xFC, g & 0xFC, bl & 0xFC))
    return px


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument('--dev')
    sub = ap.add_subparsers(dest='cmd', required=True)
    sub.add_parser('ping')
    p = sub.add_parser('hold'); p.add_argument('seconds', type=float, nargs='?', default=0)
    p = sub.add_parser('scpi'); p.add_argument('commands', nargs='+')
    p = sub.add_parser('lcdread')
    for k in 'x y w h n'.split():
        p.add_argument(k, type=int)
    p = sub.add_parser('shot'); p.add_argument('out')
    p.add_argument('--format', choices=['rgb565', 'rgb666'], default='rgb666')
    sub.add_parser('diag')
    p = sub.add_parser('tap'); p.add_argument('x', type=int); p.add_argument('y', type=int)
    p = sub.add_parser('drag', help='one-finger drag (DBG:DRAG x0,y0,x1,y1,ms,hold)')
    for k in 'x0 y0 x1 y1'.split():
        p.add_argument(k, type=int)
    p.add_argument('--ms', type=int, default=300, help='move time, ms (default 300)')
    p.add_argument('--hold', type=int, default=0, metavar='MS', help='hold still at x0,y0 first, ms')
    sub.add_parser('left')
    sub.add_parser('right')
    p = sub.add_parser('key', help='side key click (DBG:KEY n[,ms]); --hold MS = long press')
    p.add_argument('code', type=int, help='0 MENU, 1 RUN, 2 AUTO, 3 MF')
    p.add_argument('--hold', type=int, default=0, metavar='MS',
                   help='release MS ms after the press (default 0 = click; max 60000); '
                        'RUN held >= 1000 ms in a DMM view toggles AutoHold')
    p = sub.add_parser('boot', help='DBG:BOOT? decoded (native builds)')
    p.add_argument('--expect', metavar='FILE', help='update file whose header the loader marker must match')
    p.add_argument('--raw', action='store_true', help='also print the raw reply')
    p = sub.add_parser('upload', help='NATIVE: write an update file as DM40FW.BIN into the SPI '
                       'root over USBTMC (DBG:UPLOAD)')
    p.add_argument('file')
    p = sub.add_parser('update', help='NATIVE: DBG:UPDATE (zero the loader marker + reset; the '
                       'loader installs the dm40*.bin staged on the SPI disk)')
    p.add_argument('--wait', type=int, default=0, metavar='S',
                   help='wait up to S s for opendm40 to come back, then print `boot`')
    p.add_argument('--expect', metavar='FILE', help='with --wait: check the marker against FILE')
    p = sub.add_parser('image', help='offline: validate a dm40*.bin update file')
    p.add_argument('file')
    sub.add_parser('release')
    sub.add_parser('reset')
    for n in ('loader', 'recover'):
        sub.add_parser(n, description=RECOVER_HELP, formatter_class=argparse.RawDescriptionHelpFormatter,
                       help='NATIVE: DBG:LOADER, reset into the vendor update screen through PA2 '
                            '(marker kept)')
    a = ap.parse_args()

    if a.cmd == 'image':
        sys.exit(0 if print_image(a.file) else 1)
    if a.cmd == 'upload' and image_info(a.file)['problems']:
        print_image(a.file)
        sys.exit(1)
    if a.cmd in ('update', 'boot') and a.expect and image_info(a.expect)['problems']:
        print_image(a.expect)
        sys.exit(1)

    if a.cmd == 'boot' and not a.dev:
        # may run right after re-enumeration: udev can lag with the permissions
        ln = open_link(10) or Link(find_dev())
    else:
        ln = Link(a.dev)
    if a.cmd == 'ping':
        print(ln.ping())
    elif a.cmd == 'hold':
        t_end = time.time() + a.seconds if a.seconds else None
        while t_end is None or time.time() < t_end:
            print(ln.ping(15000), flush=True)
            time.sleep(5)
    elif a.cmd == 'scpi':
        for c in a.commands:
            if '?' in c:
                print(ln.query(c).decode(errors='replace').rstrip())
            else:
                ln.write(c)
    elif a.cmd == 'lcdread':
        print(' '.join('%04x' % v for v in ln.lcdread(a.x, a.y, a.w, a.h, a.n)))
    elif a.cmd == 'shot':
        from PIL import Image
        img = Image.new('RGB', (W, H))
        per_px = 1 if a.format == 'rgb565' else 1.5
        rows = 4
        t0 = time.time()
        for y in range(0, H, rows):
            n = int(W * rows * per_px + 0.5)
            words = ln.lcdread(0, y, W, rows, 1 + n)[1:]
            for i, c in enumerate(decode(words, a.format, W * rows)):
                img.putpixel((i % W, y + i // W), c)
        img.save(a.out)
        print('saved %s in %.1f s' % (a.out, time.time() - t0))
    elif a.cmd == 'diag':
        d = ln.block('DBG:DIAG?')
        print(d.hex(' '))
        if len(d) > 61:  # power.rs pins(): the charge status pins, USB state
            print('PB10=%d PB11=%d usb_configured=%d' % (d[61] & 1, d[61] >> 1 & 1, d[61] >> 2 & 1))
    elif a.cmd == 'tap':
        ln.write('DBG:TAP %d,%d' % (a.x, a.y))
    elif a.cmd == 'drag':
        ln.write('DBG:DRAG %d,%d,%d,%d,%d,%d' % (a.x0, a.y0, a.x1, a.y1, a.ms, a.hold))
    elif a.cmd == 'left':
        ln.write('DBG:SWIPE L')
    elif a.cmd == 'right':
        ln.write('DBG:SWIPE R')
    elif a.cmd == 'key':
        # Older firmware reads only n and sends key-down alone.
        ln.write('DBG:KEY %d,%d' % (a.code, a.hold))
    elif a.cmd == 'boot':
        r, err = ln.try_query('DBG:BOOT?')
        if r is None:
            sys.exit('DBG:BOOT? not available (%s)' % err)
        ok = print_boot(r.decode(errors='replace'), a.expect, a.raw)
        sys.exit(3 if ok is False else 0)
    elif a.cmd == 'upload':
        sys.exit(cmd_upload(ln, a.file))
    elif a.cmd == 'update':
        sys.exit(cmd_update(ln, a.wait, a.expect))
    elif a.cmd == 'release':
        ln.write('DBG:RELEASE')
    elif a.cmd == 'reset':
        ln.write('DBG:RESET')
    elif a.cmd in ('loader', 'recover'):
        ln.write('DBG:LOADER')
        print('DBG:LOADER sent. The meter restarts into the vendor update screen with\n'
              'its USB disk (28e9:028f): copy a dm40*.bin there and press "update", or\n'
              'power-cycle it to boot opendm40 again. See `dm40dbg.py loader -h`.')


if __name__ == '__main__':
    main()
