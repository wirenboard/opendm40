# Native boot: image, shim, recovery and the USB volume

opendm40 is a full firmware in the stock update format: the vendor FS-loader
installs it at `0x08004000` like a stock update, and a small shim at its reset
vector decides between a normal boot and recovery. This document describes
the boot chain, the update file, the flash layout and memory budget, the
shim, recovery, the boot-time disk work and the USB volume.

The design plan of 2026-10-13 and the T1-T3 device test plans are archived in
`stock/re/docs/history/native-boot-plan.md`. The `stock/re/...` paths in this
document are in the private stock-firmware analysis repo, cloned into
`stock/re/` by those with access ([firmware/README.md](../README.md#stock-firmware-analysis-private)).

## Boot chain

In short:
- **ROM** (sector 0) launches the application when PA2 is high and the
  application's marker and CRCs are valid; otherwise it starts the FS-loader.
- **FS-loader** (`0x080a0000`) makes the same test; failing it, it installs the
  first `dm40*.bin` from the SPI-flash FAT, deletes the file and resets, or
  shows its update screen with a USB disk.
- **Update file:** a 12-byte header (the marker) plus the body flashed at
  `0x08004000` ("Update file format" below).
- **opendm40's shim** at the image's reset vector starts the watchdog and
  the clock and boots opendm40; a hang reboots it. Two ways into the
  FS-loader from opendm40:
  - **PA2 entry** (`DBG:LOADER` / `DBG:RECOVER`, the power-key hold at
    power-on): PA2 driven low and locked, jump to the ROM; the marker stays
    valid, a power cycle boots opendm40 ("PA2 entry" below);
  - **marker zeroing** (`DBG:UPDATE`, the on-meter install of a copied
    image): the loader stays until a good install ("Recovery mode" below).
  No hang count leads to the loader (removed in 0.4.0).

Stock flash layout (`stock/re/ANALYSIS.md` §1):

```
0x08000000  ROM (primary bootloader)  16 KB  sector 0      never touched by updates
0x08004000  application              624 KB sectors 1..8  = update body offset 0 (zero-wait below 0x08080000)
0x080a0000  FS-loader                  64 KB sector 9      never touched by updates
0x080a4000  UID-bound bootloader data        sector 9      never erase
0x080c0000  settings / calibration           sector 10     opendm40 only reads it
0x080ff000  one-word boot flag               sector 11     read / programmed by the stock app
```

Each stage starts the next with `msr msp` + a jump through its vector
table; there is no A/B slot and no jump to the GD32 system bootloader. SWD is
read-locked. The boot gate and install in detail, with addresses:
`stock/re/docs/fsloader-install-trigger.md`.

- **ROM:** polls PA2 (the debug header's TX pad, pull-up) ~1000 times. PA2
  high at any poll and a valid application (the 12-byte marker at
  `0x08008000`: type, header CRC; then the body CRC) → the ROM launches the
  application itself. Otherwise → the FS-loader.
- **FS-loader:** repeats the same test and launches a valid application with
  PA2 high. With PA2 low throughout, or an invalid application, it enters
  its install state: it installs the first `dm40*.bin` in the SPI-flash FAT
  root (directory order, not the newest; no version compare), writing the
  body to `0x08004000` (writer bounded to `0x08004000..0x080a0000`) and the
  file's 12-byte header to the marker. On success it deletes the file and
  resets. On any failure, or with no file, it shows its update screen with a
  working USB disk (`28e9:028f`, the SPI-flash FAT).
- **The marker is the switch.** As soon as the loader finds a `dm40*.bin` it
  zeroes the marker; a bad file therefore leaves the application unbootable
  and the unit in the loader until a good install. opendm40 uses the same
  switch from software where a staged image must be installed: zeroing the
  marker and resetting reaches the install state from any reset, also after
  a power loss on the way ("Recovery mode" below). To only open the update
  screen it uses PA2 ("PA2 entry" below).
- Neither the ROM nor the loader reads the reset cause.
- The stock application never deletes or renames the file; on leaving USB
  Connect with a `dm40*.bin` present it zeroes the marker and resets.

So the unit cannot be bricked by an update: the ROM and the FS-loader are
outside the application area, and a failed install always ends on the
loader's update screen with its disk. The user procedure is in
[the manual, Backup, recovery and back to stock](../../docs/manual/README.md#backup-recovery-and-back-to-stock); the hardware entry (TX-GND
short) in [hw/README.md, Hardware recovery](../../hw/README.md#hardware-recovery).

### PA2 entry (`boot::loader_pa2`)

The way into the loader that keeps the marker: opendm40 does in software what
the TX-GND short does at power-on. `DBG:LOADER` / `DBG:RECOVER` and the
power-key hold use it.

How the ROM and the loader read PA2 (disassembly of the 2026-10-13 dump):
- ROM `0x080005f4`: GPIOA clock on (`0x08001308`, `RCU_AHB1EN` bit 0), then
  `gpio_mode_set(GPIOA, input, pull-up, pin 2)` (`0x08000fa6`: MODER and
  PUPDR rewritten for the pin), one 20000-iteration delay (`0x080005dc`),
  then up to 1000 reads of IDR bit 2 (`0x08000612`, `0x08000f96`), each
  followed by the same delay. One read of 1 launches a valid application.
- FS-loader: begin `0x080a1bd8` first sets PA2 to AF7 (USART1 TX) with
  pull-up (`0x080a4b14` → `0x080a5158` → `0x080a9a58`), then the same input +
  pull-up set-up and 1000 reads (`0x080a1c0e`..`0x080a1c48`, delay
  `0x080a1bc2`).
- Timing in the emulator (instruction-count time, not cycle-exact): the ROM's
  poll takes ~0.5 s, the loader's about the same.
- Neither writes `RCU_AHB1RST` (GPIOA is never reset) or `GPIOA_LCKR`, and
  neither writes PA2's output bit (their GPIO output writes are PE2, PC4 and
  PA11 / PA12).

So a plain output-low does not survive: the ROM's mode write turns PA2 back
into an input with pull-up, and the first read is high. What survives is a
**locked** pin. `loader_pa2`:
1. PA2 = 0 (BOP), push-pull, no pull, output; then the GPIOx_LOCK key
   sequence (offset `0x1C`: LKK + pin, pin, LKK + pin, read, read). MODER,
   OTYPER, OSPEEDR, PUPDR and AFR of PA2 then ignore writes until the next
   system reset; the output register is not locked.
2. `BKP_TRIG` = `0x54520012` (lock read back) or `0x54520013` (not: the ROM's
   pull-up wins, opendm40 boots again).
3. SysTick off, NVIC disabled and cleared, `VTOR = 0x08000000`, MSP from
   `0x08000000`, CONTROL = 0, `cpsie i`, jump to the ROM's reset handler
   (`0x080001c1`).

The ROM then sets its clock and FWDGT up again, reads PA2 low 1000 times and
starts the loader, which reads it low 1000 times and enters its install state:
a `dm40*.bin` in the SPI-flash root is installed, otherwise the update screen
with its USB disk. Ways out, each a reset that clears the lock (PA2 reads
high, the valid marker boots opendm40):
- an install: the loader resets itself;
- the power key held on the update screen (a 3 s hold in the emulator; a
  0.2 s press did nothing): the loader's tick `0x080a1cf4` returns exit code
  1, `0x080a5134` drops PE2 / PC4 (power off on battery), and its main
  returns into the stack the ROM left. In the emulator the CPU then jumps to
  a garbage address (the emulation aborts). Inferred for the device on USB
  power: a fault, nothing feeds the FWDGT any more, reset within ~20 s;
- switching the meter off and on.

The loader has no timeout (its event loop feeds the FWDGT), and a USB VBUS
cycle does not reset the meter while the battery holds it up (inferred).

It runs as the first step of the shim, after a SYSRESETREQ with
`BKP_REQ` = `0x4C324150` ("PA2L"): the CPU is then in the state the ROM
leaves when it launches the application (its PLL at 200 MHz, its FWDGT, no
interrupt enabled), the same state the loader sees after the ROM on every
cold boot. A jump straight from the running firmware would have to undo
opendm40's USB, DMA, timers and clock first. The key hold therefore also
sets the word and resets.

Device, 2026-10-29: with the running image staged as `DM40FW.BIN`,
`DBG:LOADER` ended in the loader's install and its reset (`last_trigger`
`0x54520012`, reset flag SW, the file deleted, marker = the image header).
Not yet tried there: the update screen without a file, the key hold. If the
lock did not hold, the result would be a normal reboot with `last_trigger`
`0x54520013` in `DBG:BOOT?`.

**PA2 low across a reset instead (not used).** Drive PA2 low, then
SYSRESETREQ, and rely on the pin's capacitance: the reset returns GPIOA to
its reset state (PA2 a floating input), the ROM enables the pull-up
(~40 kΩ typical) and waits one delay loop before the first of 1000 reads
spread over ~0.5 s. A pad and a short trace (inferred: tens of pF) charge
through the pull-up in microseconds, and any single high read launches the
application, so this cannot work, not just unreliably. The emulator's GPIO
model has no capacitance (a reset clears the pin), so it says nothing here.

Emulator: [`emu/peripherals/GD32_GPIO.cs`](../../emu/peripherals/GD32_GPIO.cs)
models the lock (key sequence, locked fields ignored, cleared on a machine
reset); `machine Reset` stands for a power cycle.
[`emu/recoverytest.py`](../../emu/recoverytest.py) cases `loader`,
`key-hold`, `usb-loader` (DBG:LOADER over USB/IP): ROM → FS-loader update
screen with PA2 low and locked, marker valid; after a power cycle opendm40
boots.

## Update file format

Decoded file = 12-byte LE header `word0 word1 word2` + body (flashed at
`0x08004000`). [`stock/dm40fw.py`](../../stock/dm40fw.py) implements it and
reproduces all 11 official files byte for byte
(`stock/re/ANALYSIS.md` §2):

- `word1` = body length (`dm40fw.py` `build`).
- `word0` = `(~X<<16)|X`, `X` = CRC-16 (poly `0x8005`, init `0xFFFF`,
  MSB-first, no reflection, no final XOR) over the 8 bytes `word1 ++ word2`
  (`header_word0`, `crc16`).
- `word2` = `0x00010000 | L2`; `L2` = the same CRC-16 over the body **with the
  12 bytes at body offset `0x4000` (flash `0x08008000`) excluded**, the
  loader-managed marker (`body_checksum`, `MARKER_OFFSET=0x4000`,
  `MARKER_LEN=12`).

**Obfuscation and the short last block.** The whole file (header included)
is obfuscated in 128-byte blocks: `a = block[0x1F]`, key position
`kp = a if a < 128 else a >> 1`, `k = block[kp]`, plain = `(c ^ k) - a`
except at `0x1F` and `kp`. When the file length is not a multiple of 128 the
last block is short. If it is **longer than `0x1F` bytes and `kp` >= its
length**, `dm40fw.py` stores it as is, but the real FS-loader takes the key
from a stale byte of its 128-byte buffer and corrupts the tail (seen in the
Renode emulator on the real loader: `bf->41`, `ff->81`), so the body CRC
fails: "update error", with the application already erased. Safe: whole
blocks, a last block of <= `0x1F` bytes (stored as is by both), or `kp` < its
length (the vendor `dm40_v_2_6.bin`: 36 bytes, `kp` 8). Rule: **every update
file opendm40's tools generate is padded with 0xFF so 12 + body is a multiple
of 128** (`word1` and `L2` include the padding; the loader flashes it like
any body byte):
- `dm40fw.py pack --pad` / `build(pad=True)` pads; `pack` without `--pad`
  packs as is (vendor round trip) but refuses a hazardous body; `info` /
  `unpack` warn about a hazardous input (`tail_hazard`);
- [`build.sh`](../build.sh) packs with `--pad` and fails unless the file is a
  multiple of 128; `stock/patch/patcher.py` builds with `pad=True`;
- `INFO/FIRMWARE.BIN` on the USB volume (`calbackup::FwFile`) is 12 +
  `word1` bytes, so whole blocks for every image built that way; for an
  installed image whose file would end in a hazardous block,
  `FwFile::from_flash` returns None (no firmware file);
- `tools/dm40dbg.py image` prints the last-block length and rejects a
  hazardous file; `tools/reload.sh` refuses to stage it;
- host test: `tools/fatren-test/run.sh` ("short last 128-byte block").

## Failure modes

| Failure | What happens | Why it is recoverable |
|---|---|---|
| Update file with a bad header | the loader zeroes the marker, keeps the application bytes and the file, shows "update error" and its disk | header checked before any erase; delete the file, copy a good one, press "update" |
| Header OK, body bad / short / hazardous last block | application erased, "update error" / "app crc error", disk up | same: install a good file from the loader's disk |
| Power loss while the loader flashes | marker zeroed, application partly written | the next boot enters the install state again and re-installs the file (deleted only after success) |
| opendm40 hangs | FWDGT reset (~20 s), opendm40 boots again, every time | once its USB is up: `DBG:LOADER`, `DBG:UPDATE` or the on-meter update |
| opendm40 hangs before its USB is up (every boot) | a reboot every ~20 s | the power-key hold at power-on (the shim runs before the hang) or TX (PA2) to GND at power-on ([hw/README.md, Hardware recovery](../../hw/README.md#hardware-recovery)); neither works if the shim itself hangs |
| SPI-flash FAT unreadable | the loader fast-formats it on `FR_NO_FILESYSTEM` (`stock/re/docs/fsloader-install-trigger.md`, "Install state, call order") | an empty FAT on the loader's disk takes a new file |
| Wrong-model update file | installs; the model comes from the calibration record (`0x080C0000` +0x168, [model-variants.md](model-variants.md)) | labels / ranges only |

Still open: whether the SWD pads allow a reflash after a mass erase (it would
erase the calibration: "Calibration safety copy" below) and whether `BOOT0`
is exposed.

## Implementation

- **Recovery** hands over to the vendor loader's update screen by **zeroing
  the 12-byte marker at `0x08008000`**, then resetting (SYSRESETREQ). There is
  no SPI, FAT, USB or LCD in recovery. Earlier variants are kept behind
  non-default features.
- **fatren** renames an installed `dm40*.bin` that is still on the SPI disk to
  `_DM40PRV.BIN` on a normal boot. Normally the loader has already deleted it
  ("fatren" below); the running firmware's update file is
  `INFO/FIRMWARE.BIN` on the USB volume ("Unit data").
- **Device-authenticity check** (`stock/re/docs/genuine-check.md`, in the
  private analysis repo): the stock's; opendm40 does not run it.

## Files

| File | Role |
|---|---|
| [`link-native.x`](../link-native.x) | the image layout (below), the only linker script |
| [`build.sh`](../build.sh) | builds and packs the image, checks the budgets (`native` accepted as a no-op argument) |
| [`src/shim.rs`](../src/shim.rs) | vector table, `reset` (asm), `shim_main`; all in region A |
| [`src/boot.rs`](../src/boot.rs) | shim decision `shim()`, key-hold gesture, PA2 entry `loader_pa2()`, reset flags, power-key pin, power hold on/off, backup-SRAM words, loader marker check + `zero_marker()` (FMC), `DBG:BOOT?` text |
| [`src/native.rs`](../src/native.rs) | after the decision: `.data`/`.bss` init, then recovery, or `pre_main` + `main` |
| [`src/board.rs`](../src/board.rs) | board bring-up (everything the stock firmware used to do before opendm40 ran) |
| [`src/recovery.rs`](../src/recovery.rs) | recovery (only on the request word, set by nobody now): zero the loader marker + SYSRESETREQ; features `recovery-file` (`DM40RCVR.BIN` + power off) and `recovery-usb` (own USB-MSC) keep the earlier variants for tests |
| [`src/fatren.rs`](../src/fatren.rs) | FAT16 root-directory edits: rename the installed image, remove a leftover `DM40RCVR.BIN`; with `recovery-file`, create `DM40RCVR.BIN` |
| [`src/clock.rs`](../src/clock.rs) | `init_core(Src)`: measured stock values, bounded waits, IRC16M fallback, no FLASH_ACR write |
| [`src/wdt.rs`](../src/wdt.rs) | `start::fwdgt()` (shim); `powered_off()` (end of the app's power-off) |
| [`src/msc.rs`](../src/msc.rs) | `STOP_UNIT` flag on SCSI START STOP UNIT |
| [`src/scpi.rs`](../src/scpi.rs) | `DBG:BOOT?`; `DBG:UPDATE`: zero the marker + reset; `DBG:LOADER` = `DBG:RECOVER`: PA2 entry |
| [`tools/fatren-test/run.sh`](../tools/fatren-test/run.sh) | host tests of fatren, calbackup, fatdir, upload, vdisk and vwrite on mkfs.fat images (fsck.fat + mtools) |

Cargo features ([`Cargo.toml`](../Cargo.toml); the stock fonts are always
embedded, the calibration safety copy always runs;
[`tools/build-variants/run.sh`](../tools/build-variants/run.sh) builds every
non-default set, with all of build.sh's checks, into `target/variants/`,
never over `opendm40.bin`):
- `vdisk` (default: the generated USB volume);
- `ad2-isr` (default: the AD2 stream timer ISR;
  [dmm-input-scope-feasibility.md](dmm-input-scope-feasibility.md) 11);
- `fatren-dry-run`;
- `recovery-file`, `recovery-usb`;
- `keep-screen`.

The RAM-loaded build (image in flash sector 8, copied to SRAM, loaded over
Bluetooth into the running stock firmware) was removed on 2026-10-22.

## Native memory map and the loader marker

The image is the update body: it starts at `0x08004000` (sector 1), the 12
bytes at `0x08008000` stay 0xFF in the file (the loader writes the installed
file's header there), and only the low 512 KiB of the flash are zero-wait
(region C is above that).

```
0x08004000  .vectors: word0 0x2002F7F8, word1 reset                region A, 16 KiB
            .shim:    reset, default_handler, boot::shim,
                      clock::init_core, wdt::start::fwdgt, shim_main
0x0800460c  .a2: hot named rodata (.rodata.a2.*: ASCII26, F14 blobs) region A2 (zero-wait)
            0xFF padding (objcopy --gap-fill=0xff) up to the marker
0x08008000  12-byte loader/ROM marker, 0xFF in our image             GAP
0x0800800C  4 bytes 0xFF padding
0x08008010  .text + .rodata (XIP): everything hot, then the .data   region B (zero-wait)
            load image (read once by the reset copy)
0x08080000  .cold (rarely run code)                                 region C (NOT zero-wait)
            end of image below 0x080A0000 (FS-loader above)
RAM: TCM  0x10000000 .data/.bss, then the stack down from 0x10010000;
     SRAM 0x20000000 .dmabuf (DMA buffers, the scope record, the disk
          buffer arena, the logger ring, ...), then .sram_bss (CPU-only, zeroed)
```

### Memory budget (2026-10-22 rework)

Numbers from `build.sh` (stackdepth.py and the link map) on the ad2-isr
build (default since 42cbf35) / without the feature
(`--no-default-features --features vdisk`):

| | before (00e088f) | after |
|---|---|---|
| static worst stack path | 31 040 / 30 676 B | 21 760 / 21 396 B |
| TCM stack (64 KiB - .data/.bss) | 47 860 / 47 912 B | 46 620 / 46 688 B |
| stack headroom | 16 820 / 17 236 B | 24 860 / 25 292 B |
| emulator stack high-water (DBG:BOOT? STACK, scope + FFT + screenshot) | 30 700 B | 21 420 B |
| SRAM free (above .sram_bss) | 1 840 / 2 024 B | 12 856 / 13 040 B |
| region B free (zero-wait flash) | 348 / 8 188 B | 24 508 / 31 884 B |

Where it came from (no change in function; the one timing difference is
the FFT / screenshot sharing below):

- **main's frame** (20.5 KB): `App::new` was inlined into `main`, and its
  ~11 KB of temporaries (the sub-structs before they move into the App) sat
  under every main-loop call. `main` now builds the App in a static (`APP`,
  TCM .bss) through `new_app` (never inlined, `.cold`) and hands it to `run`
  (the loop); the start-up frame is popped first. The large SCPI reply
  builders (MMEM:CAT?, MMEM:DATA?, LOG:DATA?) run as closures through
  `ScpiPort::respond_with` instead of inlined into `run`. `run`'s frame is
  ~12.3 KB (the Usbtmc with its 6 KiB reply buffer is the largest local).
- **BT reply buffer** (6 KiB `.dmabuf` BLK): only ever live inside one SCPI
  dispatch (built, then copied into the TX ring at once), so it is now a
  local of `Bt::respond_with` (never inlined): that path, run -> respond_with
  -> builder (~21.4 KB), is shallower than the drawing paths that set the
  worst case. stackdepth.py follows the dyn calls (its `INDIRECT` list).
- **FFT work buffer** (superseded the same day, [ram-budget.md](ram-budget.md): the
  screenshot capture buffer CAP is gone, the screenshot streams from the
  panel, and the FFT work buffer lives in the disk buffer arena
  `diskbuf::ARENA` (then `calbackup::SCRATCH`) with an ownership check; SRAM free 10.9 -> 60.7 KB.
  As it was before that:) (8 KiB of the 12 KB `scope_page::FFT` in TCM): an
  arena with an explicit owner, the last 8 KiB of the screenshot capture
  buffer (`screenshot::CAP`, 48 KiB). The FFT renews a lease every time it
  uses the buffer (a few main-loop passes); a screenshot started during a
  lease captures into CAP below the tail only (40 KiB; rows past it are read
  from the GRAM while writing, as for a full CAP); one started without a
  lease takes the whole CAP until its Data phase ends (the FFT job in flight,
  if any, is dropped and the frame asked again after), but gives the tail
  back as soon as its rows fit below it (~0.1 s). Effect: a screenshot of a
  screen over 40 KiB of RLE (the AC+DC and power views) taken while the
  scope FFT runs keeps the panel frozen longer; a spectrum is not updated
  while a screenshot of such a screen holds the tail. The FFT result (`db`,
  4 KB) stays in TCM. The host renders of the scope FFT views code to
  36-38 KiB.
- **Ink / Glyph**: `Glyph` keeps its blob slice from the glyph's first byte
  (no separate offset): 16 bytes instead of 20, ~1 KB off the drawing
  frames (every `Ink` holds 24 glyphs).
- **EP_MEMORY** 320 -> 64 words: USBFS has 4 endpoints with at most 64-byte
  packets, so 64 words is the most synopsys-usb-otg's OUT buffers can take.
- **screenshot `Job::off`** u32 -> u16 (word offsets into CAP): 642 B.
- Rebalanced: the touch log (1.9 KB) and the USB EP / MSC buffers (1.3 KB)
  moved from `.sram_bss` to TCM .bss (SRAM is the tighter one now).
- **Region B**: start-up and settings / page / dialog code and the debug /
  file replies moved to `.cold` (link-native.x lists them; see there), with
  `#[inline(never)]` on the helpers that were inlined into hot callers
  (`run`, `Scpi::handle`). The per-pass entry points of the `.cold`
  screenshot module (`poll`, `busy`, `frozen`, `fft_work`, `fft_free`) are
  `inline(always)` so a pass without a shot never reaches region C.

**Region B audit #2 (2026-10-22, after the fast logger)**: 9 304 / 16 568 B
free (ad2-isr / without) -> 30 368 / 37 840 B, no change in function. A map
of region B by inlined callee (addr2line -i over every address of `.text`
below 0x08080000) showed ~20 KB of `.cold` modules' code inlined into hot
callers, where the module patterns cannot reach it: `Gallery::poll` (with
the DLG decoders) in `App::tick`, `Gallery::touch` / `open` / `back`,
`VDisk::write_block` (host MSC writes) in `run`, `boot::put_info`
(DBG:BOOT?) in `Scpi::handle`, `upload::end`, `ui::help_page` and the
temperature page (`App::with_temp_page`) in `draw_static` / `gesture`,
the scope's Save CSV (`App::sp_save_csv` with `scope_csv::set`) in
`sp_gesture`, and `csvlog::delete_on` / `catalog_scpi_on` (patterns
without the `_on`): now `#[inline(never)]` (and a pattern where the module
is hot). `Dmm::poll_cap`'s two `sort_unstable_by` (5 and <= 35 values)
instantiated the core quicksort (~3.3 KB): an insertion sort (same order).
Stack and SRAM unchanged (headroom 22 328 B, SRAM free 11 872 B); region C
(`.cold`) grew by 16.7 KB, 31 700 B are left below 0x080A0000. Emulator
hooks on every function of C (from t = 3-6 s, per-second hit counts): DMM
idle, swipe-down CSV recording, combined view, an AD2 stream at 38400 S/s
(DMA frames, DRAW 1, ad2streamtest) and fast-logger disk sessions
(fastlogtest typ): C ran only at a recording / session start or stop and
for the DBG:HYSPI self-test after the stream, except LOG:DISK? (`fast_scpi`,
`.cold` since f5eae17), which a host polls while a disk session streams:
its reply (`fast_status`) is in region B again.

**Region B audit #3 (2026-10-22, on 1ad38aa)**: 11 928 / 20 672 B free
(ad2-isr / without) -> 26 160 / 32 808 B; region C (`.cold`) 25 328 /
25 768 -> 10 256 / 12 352 B left below 0x080A0000 (C is now the tighter
one: the next reclaim from B needs room there or a cut). Stack / SRAM
unchanged (headroom 21 132 -> 21 452 B, SRAM free 10 888 B). Same method
(addr2line -i over region B, then `#[inline(never)]` + a link-native.x
pattern per function, an nm diff of C against the old image: only the
intended symbols moved). To C:
- event code inlined into `App::gesture`: `App::tap` (5.3 KB with
  `pw_tap`, `kp_apply`, `resolution_cycle`, the range hits), `App::key`;
  `App::show` (view change), `App::wave_open` (`tap`, `key`, `show`: back
  in B since region A2, below);
- one-shot starts: `App::rec_start` (with `csvlog::start`); the outlined
  `Logger::start` / `logger::isr::start` (the Wave view's `start_wave`; the
  copies LTO inlined into `run` for LOG:STATe stay in B);
- page / dialog draws on a view change: `ui::pw::settings_page` (not
  `settings_status`: per tick on that page), `ui::device_page`,
  `dialog::fuse_check`; the menu / list / keypad helpers (`menu_item`,
  `main_menu_hit`, `list_page`, `page_title`, `set_stepper`,
  `keypad_input` / `_key` / `_hit`, `App::kp_labels` / `lim_unit`) and the
  generator page's boxes and 1 s status: no stream runs on those pages
  (leaving the Wave view or the inrush page stops theirs; touch and drawing
  are off while the logger runs);
- DBG / calibration replies: `Dmm::acdc_diag_text`, `Sentinel::diag_text`,
  `Scope::hw_calibrate` + `HwTrig::calibrate`;
- start-up, inlined into `run`: `Touch::new`, `Bt::new`, `SpiFlash::new`,
  `usb::init_hw` (not `Bt::init_module`: called every pass, returns early);
- vwrite's temp purge and post-write steps (`purge_tmp`, `park`, `check`,
  `finalize`, `commit`: after a host write session; MSC writes are refused
  while a logger session runs).
To B (the device check of d8e249d, [device-check-2026-10-21.md](../../docs/history/device-check-2026-10-21.md) 6a / 6c):
- the DMM-log and scope CSV generators of the USB volume (`Gen::open` /
  `advance`, `RowDec::*`, `SDec::*`, `chunk_at`, `parse_file`,
  `from_scale`, `s_pred`, `dlog::open` / `find` / `geometry` / `dir_start`
  / `dir_of` / `dlg_prefix`) in `.text.vdisk_rd`; `each_entry` is generic
  and inline(always), so no caller's closure gets a symbol of its own in C
  (the `near` bug of 784c0e0); `dlog::data` (MMEM:DATA?),
  `vdisk::readme` with its text writer `W` (sized on every root directory
  read), `vdisk::geo`, `vdisk::mem_read`,
  `calbackup::crc32_upd` (LTO merged dlog's identical CRC-32 into it) and
  `boot::bkp_enable` (`mark_healthy` at 60 s of uptime);
- the Save CSV tap's block plan (`scope_csv::set`, `dlog::scope_plan` /
  `scope_order`, `SEnc`; `scope_block` generic and inline(always)) in
  `.text.scope_plan`: it codes every block once (~1 ms each) in the tap,
  before the job starts and the stream pauses (wavetest with the emulator's
  SPI byte timing of 40dfb28 caught `scope_block` in C there).
Two Wave view changes (main.rs): the volume refresh waits while the stream
ISR owns the bus (`hy3131::streaming`; a host sees a save made in the Wave
view once the view closes), and the Wave view's stream pauses while a disk
job (Save CSV, a screenshot: screenshot.rs, region C every pass) runs and
restarts after it (the stopped record is kept, as on a range step): the
emulator showed the DLG write itself in C under the stream, not only the
refresh the device's lat_pc named.
Left in B on purpose: `scpi::parse_pow` / `App::pw_scpi` (FETC:POW:ALL? is
polled), the scope page's gestures (the Wave view's taps and drags, per
touch report while it streams), the logger panel, `persist`,
`autoset_step`. Taps: in the Wave view the scope page takes them
(`sp_gesture`, B), so `App::tap` meets a stream only on the inrush page
(whose `inrush_tap` was already in C): one tap = a burst of slow fetches,
each <= ~20 us of ISR entry latency (11.4), within the 20 us spike ad2pll
tolerates.
**Fonts stay in B** (except ASCII26 and F14, in region A2 since
2026-10-22, below). The glyph blobs are anonymous rodata (`.Lanon.*` after
the small named `*_E` descriptors: F78 25.3 KB, TEXT22 16.2 KB, ASCII26
11.5 KB, ...; 90.9 KB in all), not routable by a pattern without named
statics, and they
are read per draw on the screens a stream shows (the logger panel, the
DMM view, the Wave view). The emulator has no flash wait-state model (200
MIPS, no slow region), so it cannot measure the draw cost; the device did
([dmm-input-scope-feasibility.md](dmm-input-scope-feasibility.md) 11.4: ASCII26 / TEXT22 above 0x08080000 gave max_lat
18.8-21.8 us vs 4.6 us). The other ~55 KB of anonymous rodata (merged
strings, constants, panic locations) cannot be routed either.
**Check**: `emu/hy3131/wavetest.py` hooks every function of C and fails on
one entered while `hy3131::HANDED` is set (first call logged, then the hook
removed: a Python hook per call slows a busy function to a crawl): STOP +
Save CSV in the Wave view, then a host reading SCOPE/SCOP000.CSV and
LOGS/LOG000.CSV off the volume plus MMEM:DATA? of their DLGs while it
streams. 76b79e0 and abeb71f fail it (`Gen::open`, `vdisk::refresh`, `scan_spi`'s
closure at the save; `SDec::get`, `dlog::data`, `vdisk::readme` at the
read); now only the Save CSV tap's own `sp_save_csv` runs. The same hooks
(a scratch script, `--extra-script`) in ad2streamtest (4800 DRAW 2, 9600
DRAW 1, 1200 DRAW 2; 38400 DMA DRAW 1 and 2), fastlogtest typ with
`--vol-read` and inrushtest: no region C call with the bus handed over,
except in inrushtest `App::inrush_live` once at the capture's start and
`inrush_scpi` for the test's own INR: setters / FETCh? while armed (by
design, app_inrush_live.rs: the polled INR:STATe? / CONDition? are in B).
Still in C on the volume's read path: the firmware image file (`FwFile`),
read only when a host opens it (its data is the installed image, region C
included, so its code in B would gain little). BACKUP.TXT (`describe`) is
in B since region A2 (`.text.vdisk_rd`).

### Region A2 (2026-10-22)

The zero-wait flash between the shim and the loader marker,
`0x0800460c..0x08008000` (14 836 B; "Flash outside the image" below), is
region A2: an output section `.a2` in `FLASH_A` right after `.vectors`
(link-native.x), claimed before `.cold` / `.text` (first matching rule
wins), taking `.rodata.a2.*` and `.text.a2.*`. It is part of the update
body (flash sector 1, with the vectors and the shim), so the FS-loader
writes it on every install like any other body byte; the body CRC `L2`
covers it; nothing in the install path reads that range but the marker
(`dm40fw.py` MARKER_OFFSET, reload.sh's 12-byte read at `0x08008000`,
emu/run.py `--zero-marker`, `boot::zero_marker`), which stays 0xFF in
the image. Only hot, region-B-class content belongs there; nothing the
shim or recovery needs (they stay in `.vectors` / `.shim`).

**What is in A2**: the ASCII26 (11 546 B) and F14 (638 B) font blobs,
named statics `#[link_section = ".rodata.a2.<NAME>"] pub static <NAME>_B:
[u8; N]` that `tools/mkstockfont.py` emits for the fonts in its
`A2_FONTS` (the others stay anonymous byte strings in B; the regenerated
`stockfont_data.rs` differs from the old one only in those two blobs'
declarations). Data, not code: a fixed size, no inlining that a pattern
could miss, and ASCII26 is the font with device evidence (the logger
panel, read per draw while the AD2 stream runs; feasibility 11.4). A2 has
2 652 B left; ASCII26 + F14 + HIST (484 B) would leave 2 168 B, just
above the warning, so HIST stays in B to leave the shim room to grow.

**What came back from C to B** with the 12.2 KB B gained: `App::tap`
(5.8 KB), `App::key`, `App::show` (their audit #3 patterns removed: taps
meet a stream on the inrush page, the slowest event code to have in C)
and `vdisk::describe` (`.text.vdisk_rd`). An nm diff of C against
06011ac: only these four symbols left it.

| (ad2-isr / without) | before (06011ac) | after |
|---|---|---|
| region A2 free | 14 836 B (unused) | 2 652 / 2 652 B |
| region B free | 26 136 / 33 320 B | 30 572 / 37 836 B |
| region C free | 10 120 / 12 240 B | 17 840 / 19 880 B |
| stack headroom (static worst path) | 21 436 / 21 888 B | unchanged |
| SRAM free | 10 856 / 11 080 B | unchanged |

Rebased onto a847991 (the power meter's AC+DC parts and region C audit
#1 (1/3) landed meanwhile), the landed tree b8c368d: A2 2 652 B, B 21 996
/ 28 660 -> 26 360 / 33 112 B, C 10 436 / 12 572 -> 18 220 / 20 276 B,
stack headroom 21 236 / 21 672 B and SRAM free 10 848 / 11 072 B
unchanged.

**Budget**: the link fails when `.a2` reaches the marker (`FLASH_A` is
`0x08004000..0x08008000`, plus an assert with the message naming
`A2_FONTS`); `build.sh` prints `region A2 free` and ends with a banner
below 2 KiB. Its 0xFF check of the gap now runs from the end of A2 to
`0x08008010`. The shim grows into A2's room: a shim change that fails the
A2 assert takes a font out of `A2_FONTS`.

**Install-format check** (the image of this change): `dm40fw.py info` /
`unpack` (the unpacked body equals the objcopy body + 0xFF padding, the
12 bytes at body offset 0x4000 are 0xFF), `tools/dm40dbg.py image` (OK,
whole 128-byte blocks), and the emulator's full chain (`emu/run.py chain
--app dump --zero-marker --disk-add opendm40.bin::DM40_FW.BIN`):
the real FS-loader installs the file, the flash at `0x08004000` equals
the body, the marker holds the update header, and opendm40 boots from it.

### Region C audit #1 (2026-10-22)

Asked for: at least 12 KB more free in region C without losing a
feature, the gallery (17.6 KB of C) the first suspect. Measured with nm
sizes per symbol in `0x08080000..0x080A0000`, a DWARF walk of every
inlined copy in C (per abstract origin: copies, bytes, and whether an
out-of-line copy exists too) and inline trees of the largest functions.

What the image does NOT have (the usual suspects):
- **no `core::fmt` machinery** in the image (no `Formatter`, `write_str`,
  `pad_integral`; only `panic_fmt` (8 B) and two `Debug::fmt` stubs of 22
  / 40 B, all in B; panics are `panic=abort`, C calls the
  `panic_bounds_check` / `slice_index_fail` / `div_by_zero` stubs in B);
- **no duplicated BMP decode**: one `Bmp::parse` (692 B, inlined once),
  one `Bmp::color` (208 B), one `Job::pix_block` (992 B). The gallery is
  dense single-copy code: `Gallery::poll` 6.6 KB = `poll_on` -> `work` ->
  `step` (4.4 KB: BMP head / pixel blocks, and `step_dlg` 1.9 KB = the
  DLG / FAST decoders) + catalog / page scan / delete; `begin_b` 1.5 KB
  (the log viewer's texts); its formats (16 / 24 / 32 / 4 / 1 bpp, RLE4,
  DLG and FAST logs) are features;
- **no costly inlining**: inlined copies in C next to an out-of-line copy
  total < 1 KB (mostly 10-40 B leaves: `mmio::*`, `stockfont::rect`,
  `copy_from_slice`, clamps, where a call costs as much as the body);
- **one monomorphization**: `App::with_temp_page(impl FnOnce)`, four
  copies of ~330 B (fixed below).

Done (36b6aae 9 936 B free; 2ddeecc 7 984 B after E7 grew C):
| change | region C | commit |
|---|---|---|
| `gallery::G` (672 B .data image) -> `.bss`, set on the first `get` | +592 | 9c2041d |
| `scope_csv::SNAP` (548 B): `rec: Option<&[u16]>`, all zero -> `.bss` | +580 | 9c2041d |
| `csvlog::JOB` (512 B): zero sentinels until `start` sets JOB0, `dlog::ENC_ZERO` -> `.bss` | +512 | 9c2041d |
| `App::with_temp_page`: a key argument instead of a closure (4 copies -> 1) | +944 | 9c2041d |
| `gallery::Tb::c` `#[inline(never)]` (9 inlined copies next to its symbol) | +76 | eecea9a |
| 23 of 24 dead-code warnings (LTO had dropped it all) | 0 (noise) | 09de55b |
| the `.data` load image (1 056 B) -> region B after `.text` | +1 056 | 3b9f5d9 |
| relocation: `ad2exp` (DBG:AD2:TORN? / NOISE? / DCV? / N7?, E7's `dc_enter`) and `dbg_dcv` -> region B | +5 208 | 133f81e |

Meanwhile upstream: region A2 / `App::tap` / `key` / `show` /
`vdisk::describe` back to B (33f4eba, +7.7 KB of C) and the font rework
(d030b2c: B 26.4 -> 82.2 KB free). On 5af9a5c (ad2-isr / without):
region C 17 420 / 19 484 -> 23 804 / 25 800 B free, B 78 192 / 85 016 ->
71 980 / 78 900 B, A2 7 920 B, stack headroom 21 228 / 21 656 -> 21 236 /
21 664 B, SRAM unchanged. From the task's start (36b6aae, 9 936 B): +13.9
KB, of which this audit's shrinking 2.7 KB (9c2041d + Tb::c), its
relocation (the .data image and the DBG experiments, B had room) 6.3 KB,
the rest upstream (A2 move +7.7 KB, net of E7 and later growth). The relocated code is on-demand DBG / experiment
code (zero-wait flash only makes it cheaper); `git revert 133f81e` puts
it back in C if B gets tight.

Checks: ui-render and gallery-test renders pixel-identical to the base, 0
violations; tools/font-matrix identical; every `tools/*/run.sh`;
`emu/gallerytest.py` (new: open, page swipes, the viewer = the BMP bytes,
Delete -> Cancel / Delete, back out) PASS on base and change, screenshots
identical; wavetest (hooks every function of C while the stream owns the
bus: none called), ad2streamtest, inrushtest, dcvtest, couptest, powtest,
senttest, leadstest, burdentest, btwaketest, the HY3131 model unit tests.
Two wall-clock checks failed once under a load average of ~60 and passed
on a rerun (ad2streamtest "every captured record drained": 861 < 1000
records at 1200 S/s; powtest acdc-floor); modetest's opendm40 scenarios get
no BT reply on the base image as well (0 readings, pre-existing).

Candidates left (measured or estimated, not done):
- DBG-only code still in C, if B keeps its room: `hyspi` (DBG:HYSPI:SELF?
  / WR?, 2.3 KB; ad2streamtest uses both), `boot::put_info` (DBG:BOOT?,
  1.2 KB), `prof::report` (DBG:PROF?, 1.0 KB; usbtmc_stress, device
  checks), `touch_diag` (DBG:DIAG?, 0.9 KB), `Dmm::acdc_diag_text`
  (DBG:ACDC?, 0.7 KB), `HwTrig::calibrate` (0.7 KB): ~6.8 KB by relocation.
- Gating the AD2 experiments behind a non-default feature: 2 580 B on
  2ddeecc (trial build), but the open TODO (B) device session still uses
  `ad2 noise` (DBG:AD2:NOISE?), so not done.
- One `fatren::fat_get` for fatdir / fastlog prep / csvlog / gallery
  (four inlined one-block-cache copies, ~0.2 KB; csvlog's runs during a
  recording, so the shared one must live in B); screenshot.rs and
  calbackup.rs have two more (the RAM-overlay change owns those files).
- `.data` statics of hot modules (scope_ui SHOWN 308 B, scope WAVE 224 B,
  inrush SES 200 B, STRIP_SHOWN 128 B): now B, not C, since 3b9f5d9.
- The four text builders (`gallery::Tb`, `dlog::Txt`, `csvlog::W`,
  `vdisk::W`): different rules (Tb maps non-ASCII to '?', Txt has u64 /
  ms / Val), ~0.3 KB at most; panic sites in C (64 calls, ~0.5 KB with their checks).

**Link-time asserts** (`link-native.x`):
- region A ends at or below `0x08008000`;
- region A2 starts right after region A and ends at or below `0x08008000`
  (`build.sh` ends with a warning banner on stderr below 2 KiB free);
- `.text` starts at or above `0x0800800C`;
- the image ends at or below `0x080A0000`, with >= 4 KiB of region C free
  below it (`build.sh` ends with a warning banner on stderr below 8 KiB);
- region B (`.text`, hot) fits below `0x08080000` and leaves >= 8 KiB free
  (`build.sh` ends with a warning banner on stderr below 16 KiB);
- TCM stack (64 KiB - .data/.bss) >= 42 KiB;
- SRAM >= 8 KiB free above `.sram_bss`.

`build.sh` also requires >= 20 KiB between the TCM stack and the static
worst call path (`tools/stack-depth/stackdepth.py --check 20480`, incl. the
AD2 ISR and its exception frame), and prints region B / region C / SRAM
free.

Region C thresholds: C grows by whole functions moved out of B (0.3..5 KB
each, audit #3); 4 KiB still takes one more small move
after the 8 KiB warning. Relief for C is a cut, or `.cold` code moved back
to B while B's own budget allows: nothing above `0x080A0000` is reachable
(next section).

### Region C audit #2 (2026-10-25)

On 058a1d36 region C had 6 016 B free (below the 8 KiB warning), region B
18 820 B (2 436 B above its 16 KiB warning), region A2 4 188 B. A search
for a cut found no large one: C holds code only (no named tables), and its
biggest functions are constructors and state machines (`new_app` with
`App::new` / `App::restore` / `Dmm::new` inlined, 7.6 KB; `Gallery::poll`,
6.9 KB); the most repeated inlined helper is `stockfont::rect` (1.7 KB over
133 sites), a struct constructor, so an out-of-line copy saves nothing.

The relief is a move in two steps, no code changed:
- the SC_PROBE font blob (1 242 B, the scope's probe labels) from B to A2
  (`A2_FONTS` in [`tools/mkstockfont.py`](../tools/mkstockfont.py); both
  zero-wait; the regenerated
  [`stockfont_data.rs`](../src/stockfont_data.rs) differs only in its
  `link_section` line);
- from C back to B: the SCPI range / resolution commands
  (`scpi::range_cmd`, `conf_cmd`, `range_arg`, `res_arg`, `step_exp`,
  `pow10f`, `App::scpi_range`; 2 042 B) and `ui::help_page` (572 B): their
  patterns are gone from [`link-native.x`](../link-native.x). Code moved
  into B cannot stall the AD2 stream ISR, so the region C rule is not
  touched.

| | before (058a1d36) | after |
|---|---|---|
| region A2 free | 4 188 B | 2 948 B |
| region B free | 18 820 B | 17 444 B |
| region C free | 6 016 B | 8 632 B |
| stack headroom (static worst path) | 20 548 B | unchanged |
| SRAM free | 45 160 B | unchanged |

Without `ad2-isr` (`--no-default-features --features vdisk`): A2 2 948 B,
B 27 480 B, C 9 680 B, stack headroom 21 008 B.

A2 keeps ~0.9 KB above its 2 KiB warning for the shim; B ~1 KB above its
16 KiB warning. Since 0.6.0 (the DMM accuracy label,
[dmm-accuracy.md](dmm-accuracy.md#4-memory)): the label's code in A2 (A2
1 844 B free, below its warning), `ui::help_page` back in `.cold` (B
16 484 B, C 7 152 B). The next relief for C needs a real cut (the candidates of
audit #1 above) or a larger blob moved out of B.

### The long scope record (2026-10-26)

The scope record buffer `scope::BUF` grew from 53 248 B to 86 016 B (the
design: [ram-budget.md, The scope record](ram-budget.md#the-scope-record-raw-ring-and-coded-ring)).
Flash for the new code (the packed ring, the `pack10` reads in every
record user) came from `stockfont::paint`: it was one copy per closure
type (23 copies, 4.0 KB in region B); it is now a thin generic wrapper
around one `paint_dyn` that takes the closure as `&mut dyn FnMut` (region
B +1.3 KB). Emulator (emu/dragperf.py, the scope page with trigger-level
drags): `s.paint` maximum 4 790 -> 4 960 us per pass, the average
unchanged. With that room, `scope_ui::drag_target` and
`Scope::hw_calibrate` (378 B) moved from region C back to B.

| | before (d0436633) | after |
|---|---|---|
| scope::BUF | 53 248 B | 86 016 B |
| SRAM free | 44 944 B | 12 136 B |
| region B free | 16 500 B | 16 716 B |
| region C free | 8 024 B | 8 316 B |
| stack headroom (static worst path) | 20 548 B | 20 516 B |

Without `ad2-isr` (`--no-default-features --features vdisk`): B 26 320 ->
26 328 B, C 9 064 -> 9 392 B, stack headroom 21 008 -> 20 984 B, SRAM
free 45 204 -> 12 396 B.

The new ring state (`scope::RING`) is in `.sram_bss`, not in the TCM
`Scope`, so the stack keeps its headroom. The per-poll code (the staging
drain, the packed reads) is in region B.

### Region B audit #5 (2026-10-26, fix/low-freq)

The low-frequency Hz counter (`Dmm::freq_gate`, the gate checks, the
provisional pick of `Freq::take`) is per-pass code: region B. It took B from
16 500 B (main) to 15 532 B. Relief:
- the USB endpoint allocation of usb-device / synopsys-usb-otg
  (`UsbBus::alloc_ep`, `EndpointAllocator::alloc`, `UsbBusAllocator::alloc`
  / `bulk`; 756 B) to `.cold`: only `run` building the USBTMC / MSC classes
  calls it, before its loop;
- `Gate` without fields (no out-of-line `PartialEq`) and `gate_limit_ms`
  shorter (144 B).

| | before | after |
|---|---|---|
| region B free | 15 532 B | 16 468 B |
| region C free | 7 820 B | 7 064 B |
| stack headroom (static worst path) | 20 508 B | unchanged |

C is now below 7 KiB by ~100 B; the next relief for B needs a cut.

The 60 Ω range (feat/ohm60, 0.3.0) costs ~180 B of B; the LOCK font blob
(255 B) moved from B to A2 for it (A2 2 692 B, B 16 572 B free).

### Region B audit #6 (2026-10-27, chore/region-b-audit-6)

On 0.3.0 (cae46446) region B had 16 388 B free, A2 1 128 B, C 4 736 B
(recovery-file 4 212 B, 116 B above the link floor). C had no room for a
move out of B, so this audit only cuts: code that core / libm functions
pulled into B for one or two callers, one inlined helper, and part of the
`.data` load image; then (owner's decision, 2026-10-27) panic sites without
their source location, and with that room one A2 font blob back to B.
Nothing moved into region C, so the region C hooks
see the same set of functions as before.

| change | region B | other |
|---|---|---|
| no core string searcher, no Unicode trim (below) | +2 160 | C +36 |
| `Scpi::put_conf_fmt`: `chars().nth(1)` for `chars().count() > 1` (`do_count_chars`, 320 B) | +304 | |
| `touch::log` `#[inline(never)]` (five copies; once per touch frame / gesture event) | +456 | |
| `ui::gen_preview`: the phase fraction as `x - trunc(x)` for f32 `% 1.0` (libm `fmodf` and its reduction helper, ~0.7 KB; exact for x in 0..2) | +672 | C -4 |
| `scope::WAVE`, `scope_ui::SHOWN`, `STRIP_SHOWN` in SRAM `.dmabuf` (below) | +536 | stack +672, SRAM -664 |
| `-Zlocation-detail=none` (build.sh, below) | +31 600 | C +3 168, stack +24 |
| the SC_PROBE font blob (1 242 B) from A2 back to B (below) | -1 240 | A2 +1 244 |

- **String searcher**: `stockfont::unit_text` tested `u.contains('°')`; a
  non-ASCII `char` pattern searches for its UTF-8 bytes as a `&str`, so the
  one out-of-line `<str>::contains::<char>` (shared with two ASCII callers)
  carried core's two-way searcher (`StrSearcher::new`, `TwoWaySearcher::*`,
  ~1.4 KB). `unit_text` now looks for the two bytes `C2 B0`, the two `'.'`
  tests (`app::trim_zeros`, `DmmView::bar_labels`) use `as_bytes().contains`,
  and the generator page's `note.split(": ")` became a `find(':')`.
  `str::trim` (the DMM value text, `upload::data_inner`'s offset) pulled the
  Unicode whitespace table and its decoder: `trim_ascii` (the texts are
  ASCII).
- **Statics**: their non-zero initialisers were 660 B of the `.data` load
  image in B. On the device they are now in `.dmabuf` (NOLOAD, as
  `scope_ui::GRAPH`) and set up in place: `Scope::new` writes `WAVE` (it
  already reset it), `scope_ui::init` (`init_shown`, `#[inline(never)]` so
  the code stays in B and not in `new_app` in C) writes the other two. All
  three are main-loop only (the AD2 ISR never touches them). The host
  builds keep the initialisers. `inrush::SES` (232 B) was tried the same
  way and gained nothing: its start-up copy needs `Session::NEW` as a
  232-byte constant in B.
- **Panic locations**: every bounds check, `unwrap`, slice copy and
  division hands the panic path a `core::panic::Location` (file, line,
  column: 16 B of anonymous rodata per site, plus a literal pool entry and
  the source path strings). A scan of the image for path / length pairs
  finds ~1 530 of them (inferred to be these records). `main.rs` `panic`
  never reads them: it paints the magenta bar and waits for the watchdog
  (the shim counts the reset in `BKP_CRASH`; no record holds a file or
  line). `build.sh` compiles with `-Zlocation-detail=none`, so every
  location is the same redacted record. The flag is a nightly one:
  `build.sh` sets `RUSTC_BOOTSTRAP=1` for its own `cargo build` only (the
  pinned stable rustc then accepts it; CI and release.yml build through
  `build.sh --locked`). A plain `cargo build` (`.cargo/config.toml`) and
  the host test builds (`tools/*/run.sh`, their own rustc calls) are
  unchanged and keep their locations: a failing host test still names its
  line. The panic path in the image: `panic_bounds_check` -> `panic_fmt` ->
  `rust_begin_unwind` (the bar, `wait_for_watchdog`); none of them loads
  the location.
- **A2**: with B at 52 KB free the SC_PROBE blob left `A2_FONTS`
  ([`tools/mkstockfont.py`](../tools/mkstockfont.py); the regenerated
  [`stockfont_data.rs`](../src/stockfont_data.rs) differs only in its
  `link_section` line; both regions are zero-wait): A2 is above its 2 KiB
  warning again.

Tried, not done:
- `Scpi::put_bytes` `#[inline(never)]` (11 copies): B -16 B.
- `vdisk::put16` out of line (2 copies, ~400 B): it runs per FAT entry on
  the USB volume's per-sector read path, which a host may read during a
  stream.
- The font descriptor tables (`*_E`, 10 B per glyph, 869 glyphs, B) keep
  each glyph's code also in the 455 entries of fonts whose codes run
  without a gap (ASCII26, SC_F12 / F16 / F20, ...): ~0.9 KB with a
  `first` code per font and an index lookup (tools/mkstockfont.py and the
  `Font::glyph` search change).
- The f64 soft-float routines (~3.5 KB): their callers (`scope_meas`
  sums, the Wh / VAh accumulators, `nr3_scale`, `gen::plan` timing) need
  the precision.

The rows above add up to 50 876 B; the final tree (version 0.3.1) has:

| | before (cae46446) | after |
|---|---|---|
| region A2 free | 1 128 B | 2 372 B |
| region B free | 16 388 B | 50 892 B |
| region C free | 4 736 B | 7 940 B |
| stack headroom (static worst path) | 20 500 B | 21 196 B |
| SRAM free | 44 576 B | 43 912 B |

The variants (tools/build-variants/run.sh), B / C free and stack headroom
(A2 after: 2 372 B; no-ad2-isr 2 456 B, mute 2 396 B):

| variant | before | after |
|---|---|---|
| recovery-usb | 15 044 / 4 800 / 21 012 B | 49 940 / 7 984 / 21 708 B |
| recovery-file | 16 084 / 4 212 / 20 500 B | 50 748 / 7 452 / 21 196 B |
| no-ad2-isr | 26 264 / 5 816 / 20 968 B | 60 384 / 9 016 / 21 640 B |
| spi-disk | 37 064 / 14 904 / 22 012 B | 68 692 / 17 924 / 22 692 B |
| fatren-dry-run | 16 452 / 5 152 / 20 500 B | 50 876 / 8 380 / 21 196 B |
| keep-screen | 16 428 / 4 724 / 20 500 B | 50 924 / 7 932 / 21 196 B |
| mute (`--features mute`, not in build-variants) | | 50 572 / 7 808 / 21 196 B |

### Region C audit #3 (2026-10-29, chore/region-c-headroom)

On 3b142426 (0.3.1, `--features mute`) region C had 7 720 B free (below
the 8 KiB warning), region B 50 572 B. The AD2 experiment DBG code goes
back to region B, where region C audit #1 had put it (133f81e) and region
B audit #4 took it out of (B at 15 KB then): `ad2exp` (DBG:AD2:TORN? /
NOISE? / DCV? / N7?, E7's `dc_enter` / `dcv_row`), `main.rs` `dbg_dcv` /
`dbg_n7`, `scpi::dcv_arg` / `n7_arg` (C +5 632 B, B -5 648 B). Their patterns are gone
from [`link-native.x`](../link-native.x); no source changed. An nm diff
of the two images: only these 21 symbols changed region (the
`OUTLINED_FUNCTION_*` are renumbered, 8 B smaller in all). Code moved into
B cannot stall the AD2 stream ISR (B is zero-wait), so the region C rule
is not touched; `dc_enter` (in `Logger::start`, before the stream takes
the bus) now runs from zero-wait flash.

| | before (3b142426) | after |
|---|---|---|
| region A2 free | 2 396 B | 2 396 B |
| region B free | 50 572 B | 44 924 B |
| region C free | 7 720 B | 13 352 B |
| stack headroom (static worst path) | 21 196 B | 21 196 B |
| SRAM free | 43 912 B | 43 912 B |

The variants (tools/build-variants/run.sh) after the move, B / C free and
stack headroom (each gains the same ~5.6 KB of C):

| variant | after |
|---|---|
| recovery-usb | 44 284 / 13 544 / 21 708 B |
| recovery-file | 45 092 / 13 004 / 21 196 B |
| no-ad2-isr | 54 736 / 14 568 / 21 640 B |
| spi-disk | 63 028 / 23 480 / 22 692 B |
| fatren-dry-run | 45 228 / 13 932 / 21 196 B |
| keep-screen | 45 292 / 13 484 / 21 196 B |

Next candidates if C gets tight again (DBG-only, still in C): `hyspi`
(DBG:HYSPI:SELF? / WR?, 2.3 KB), `boot::put_info` (DBG:BOOT?, 1.6 KB).

### Region B audit #7 (2026-10-30, test/scope-chain-0.4)

The scope chain #41 (long record) + #59 (coded ring) + #63 (protocol
decoder) merged on release/0.4.0 (8eecb058, with #44) did not link: the
decoder's ~23 KB of region B code had been budgeted against an older
base. Free flash in all was B -6 296 + C 10 824 + A2 2 208 = 6 736 B
against 12 288 B of floors, so moves alone could not fit it; about 5.6 KB
had to leave the image.

| | merged, before | after |
|---|---|---|
| region A2 free | 2 208 B | 156 B |
| region B free | -6 296 B (no link) | 8 704 B |
| region C free | 10 824 B | 7 204 B |
| stack headroom (static worst path) | 20 320 B | 21 560 B |
| SRAM free | 7 244 B | 11 484 B |

RAM:
- `scope_ui::dec::ANN` (1 236 B) TCM -> `.sram_bss` (stack headroom).
- `app_decode::JOB` zero bytes until `init` writes None (no `.data` image).
- `acdctr::TR_N` 192 -> 64 entries, `TR_POST` 32 (1 792 B of SRAM); with
  the trace off (below) one entry.

Moves (no code removed):
- `fft::QSIN` (2 052 B, read per butterfly) -> region A2 (`.rodata.a2`):
  as fast as B. A2 is left with 156 B (build.sh banner below 2 KiB).
- `K_FONT` (2 420 B) -> C: its only reader is `ui::keypad_key`, already
  in C (the Keypad page / popup, no stream).
- The decoder frame log's page draw, setup popup, touches and panel
  buttons (`log_page`, `log_setup_draw`, `log_gesture`, ... ~2 KB) ->
  `.text.log_cold`. The per-pass decode and row drawing (`log_poll`,
  `log_tick`, `log_row`, `store_frame`) stay in B. The decode tab
  (`dec::page`, `dec_setup_tap`) and the DECode:* queries stay in B: the
  Wave view's MF popup and a host reach them while its AD2 stream runs.
  New guards, so that no AD2 stream runs with the log: `log_open`
  refuses while `hy3131::streaming()`, `LOG:STAT ON` is refused while the
  log is open (`log_on_check`). Before, a host `LOG:STAT ON` with the log
  open could run the existing `.text.log_cold` code under the ISR.

Left out of this test image (cargo features, not default here; build with
them for a debug session, which does not fit region B with the decoder):
- `ad2-exp`: DBG:AD2:TORN? / NOISE? / DCV? / N7? (`ad2exp` torn / noise /
  dcv_test / n7_test, `dbg_dcv`, `dbg_n7`, `dcv_arg`, `n7_arg`; E7's
  `dc_enter` / `dcv_row` stay): B +5 040 B.
- `acdc-trace`: DBG:ACDC:TRACE records nothing (the query answers an empty
  ring): B +864 B, SRAM +1 768 B.
- `dbg-src`: DBG:SRC (`dbgsrc_dmm`, the parser): B +2 640 B, C +984 B.

Back to region B: `App::sp_popup_close`, `sp_uncover`, `scope_ui::uncover`
(292 B; C since region B audit #3). MENU with an MF popup open in the
Wave view closes it while the AD2 stream runs (hy3131/wavetest.py "exit
(MENU)", reached once its Settings tap missed the five-tab MF bar).

Not done: the decode tab to C (it would need the tab hidden in the Wave
view); the gallery's `dlog` decoder copies for `GSrc` (~0.6 KB of B,
`.text.vdisk_rd` on the generic functions); glyph tables without the
code field where dense (~0.65 KB).

### Region B audit #8 (2026-10-30, test/scope-chain-0.4 on 0.4.0 + GEN / CH2)

The chain merged again on release/0.4.0 (e3229f7a: #64 / #177 GEN and
DMM CH2, #172, #180) did not link: region B -6 304 B, SRAM -6 924 B
(release alone: B 8 280, SRAM 24 244), with region C 6 588 B and A2 156 B
free. With CH2 the AD2 stream also runs on the MCX scope page, so no
scope-page code can go to region C; about 12 KB of flash and 15 KB of
SRAM had to leave the image.

| | merged | after |
|---|---|---|
| region A2 free | 156 B | 156 B |
| region B free | -6 304 B (no link) | 9 440 B |
| region C free | 6 588 B | 6 064 B |
| stack headroom (static worst path) | 21 312 B | 21 312 B |
| SRAM free | -6 924 B | 9 460 B |

Left out (cargo features, not default here):
- `dec-log`: the decoder's frame log (Mode Log, DECode:MODE LOG, the
  FRAMes? FIFO of the log; src/app_log.rs, a stub app_log_off.rs refuses
  it): B ~9 KB, C ~5 KB. The Decode tab's Mode row shows Band only.
- `dec-modbus`: Modbus RTU in the decoder (decode.rs `MODBUS`; UART
  only, DECode:PROTocol MODBus refused): B 1 176 B. The host tests build
  it always.

SRAM: `scope::CAP` 42 K -> 34 K u16 (-16 384 B): the raw ring of the fast
timebases 43 008 -> 34 816 samples, the packed ring (`PCAP`, white
noise) 65 520 -> 52 400, the post-trigger part of a coded Single at the
centre 52 156 -> 39 036; a compressible signal keeps ~0.8 of what it did
(tools/cring-test). The `recovery-usb` split of CAP is gone (it was the
smaller one).

Moves (functions whose every caller is region C code, from a call graph
of the ELF with every address-taken function counted as hot): the
generator page's wave preview (`ui::gen_preview`, from `gen_values`),
vwrite's `stage` / `ours_live` (`VDisk::write_block`), `HwTrig::sweep`
(`calibrate`), `csvlog::ascii`, `ui::ok_and_limits_buttons`,
`ui::pw::outline`, `ui::record_button`, `app::kp_unit_text`,
`spiflash::block_crc`: B +2 992 B. The struct templates copied by region
C code only (`fastlog::JOB0` 1 864 B for `fastlog::begin`, `csvlog::JOB0`
512 B for `start` from `App::rec_start`, `app_touchtest::TT0` for
`tt_open`) are statics in `.rodata.cold_tpl` (C): B +2 552 B. A `const`
copied by `*j = JOB0` is an anonymous image in region B whoever reads it.

Still off: `ad2-exp`, `acdc-trace`, `dbg-src` (audit #7).

### Protocol decoder (2026-10-28, feat/decoder-uart)

The decoder band (src/decode.rs, app_decode.rs, ui_decode.rs) decodes each
stored record once (running: one that triggered; stopped: on STOP), in
the main loop (`dec_step`, up to 3 ms a pass) while the next acquisition
waits (`Scope::hold_restart`), and keeps its bytes and frames
(`ui_decode::ANN`: 80 bytes, 16 frames, ~1.3 KB) in TCM; every redraw
draws from them. SRAM is untouched (the scope ring of feat/scope-dlg-ring
leaves 8.5 KB). All of it is region B (emu/decodetest.py: no region C
function while the Wave view's AD2 stream ISR owns the bus).

| | feat/scope-dlg-ring (4f4294eb) | with the decoder |
|---|---|---|
| region B free | 46 300 B | 29 940 B |
| region C free | 8 196 B | 8 240 B |
| stack headroom (static worst path) | 22 220 B | 20 656 B |
| SRAM free | 8 524 B | 8 524 B |

Variants (B / C free, headroom): recovery-usb 28 532 / 8 276 / 21 168 B,
recovery-file 29 780 / 7 752 / 20 656 B, no-ad2-isr 39 000 / 9 244 /
21 128 B, spi-disk 48 064 / 18 216 / 22 244 B, fatren-dry-run 29 924 /
8 664 / 20 656 B, keep-screen 29 972 / 8 232 / 20 656 B, mute 29 620 /
8 120 / 20 656 B.

Cost (emulator at --mips 110, DBG:PROF?: `dec.job` one record's decode,
`x2` a pass's part of it, `s.frame` the frame; UART traffic on MCX, Auto
sweep with the trigger level in the swing, 3 s running; emu/decodetest.py):

| s/div | baud | frames off / on | frame avg, us (off / on) | decode per record avg / max, us |
|---|---|---|---|---|
| 1 ms | 9600 | 79 / 70 | 6 887 / 6 978 | 3 049 / 8 328 |
| 10 ms | 9600 | 24 / 24 | 6 887 / 7 247 | 2 296 / 2 797 |
| 100 ms | 9600 | 3 / 3 | 6 887 / 6 978 | 115 / 169 (the note) |
| 1 ms | 115200 | 89 / 69 | 6 887 / 7 253 | 4 098 / 4 904 |
| 10 ms | 115200 | 25 / 25 | 6 887 / 6 978 | 133 / 178 (the note) |
| 100 ms | 115200 | 3 / 3 | 6 887 / 6 978 | 123 / 140 (the note) |

STOP: one decode of the stored record (1 ms/div 3.9-4.3 ms); a pan or zoom
inside what is kept: none; outside it (a record with more than the lists
hold): the window, ~2 ms. A pass decodes for up to 3 ms (`x2` max
3.3 ms); the next acquisition waits for the decode: at 1 ms/div 70 / 69
frames in 3 s instead of 79 / 89.

The frame log (Mode Log, app_log.rs, ui_log.rs) lends BUF from the scope
(`Scope::log_begin`): its first 16 K samples a circular DMA ring, then
`app_log::Log` (the decoder and the view, under 1 KB), a snippet's
decoded samples (8 KB), then the frame store (~44 KB with CAP 42 K; 32 KB
in recovery-usb's 36 K, which has no scope). TCM: a pointer; SRAM:
nothing. Region B 18 780 B free, region C 5 668 B (the waveform view and
the log's top bar / counters, `.text.log_cold`; variants: B 16 900 ..
36 576 B, C 5 180 .. 15 684 B, mute 17 804 / 5 564 B). No idle power-off
while the log runs. While the log
streams, the settings save and the screenshot clusters' erase wait (an
SPI erase outlasts the 13 ms ring at 1.25 MS/s). The static worst path is 24 728 B (headroom 20 600 B).
Emulator (--mips 110): a pass decodes for at most 3 ms (`x2` max 1.6 ms
at 115200 with traffic), 4 588 frames in 3 s at 115200 without a lost
sample; 267 Modbus 19200 frames with their waveforms fill the store.

### Region B audit #9 (2026-10-30, feat/region-b-profile)

Asked for: why so much code is in region B, and room for the decoder's
`dec-log` / `dec-modbus` on test/scope-chain-0.4 (#174). Tool:
[`tools/region-profile/rprof.py`](../tools/region-profile/rprof.py). It
charges every byte of B to a function, a named static or a piece of
anonymous rodata (cut at every address that code or data refers to, and
charged to the referrers), with the innermost inlined callee and its
module, and puts each item in a class:
- `isr`: reachable from `ad2_stream_irq`;
- `traced`: executed while `hy3131::HANDED` was set in an emulator run
  (`DM40_BTRACE=DIR`, [`emu/peripherals/BTrace.cs`](../../emu/peripherals/BTrace.cs)),
  or data such code refers to;
- `loop`: reachable from `run` without passing through region C code, and
  not executed under the stream in these runs: not exercised, not proven
  cold;
- `c-only`: reachable only through region C code, so it never runs under
  the stream while the region C rule holds.

Profile of #174 (e7c11934) with this branch merged, `--features
dec-log,dec-modbus`. Traces: decodetest (MCX UART / Modbus, the Wave view)
and an MCX scope page with CH2 on (timebases 1 ms .. 500 ms/div, STOP /
RUN, a pan, the MF tabs, DEC:PROT UART) on the full-decoder build;
wavetest on the default build (matched by function name).

| class | code | data | total |
|---|---|---|---|
| `isr` | 6 234 | 114 | 6 348 |
| `traced` | 233 344 | 48 568 | 281 912 |
| `loop` | 169 830 | 8 503 | 178 333 |
| `c-only` | 4 788 | 9 957 | 14 745 |
| unreferenced (alignment) | | 294 | 294 |
| `.data` load image | | 544 | 544 |
| region B used | | | 482 176 |

60 % of B ran under the stream in these runs. The largest `loop` items run
under the stream in cases these runs do not reach: `App::gesture` / `tap`
(touches in the Wave view and on the inrush page), `VDisk::read_block` and
`dlog::Gen::*` (a host reading the USB volume during a stream),
`gen::plan` / `Gen::apply` (SOUR:* from a host). `DmmView::reading` (7.7
KB) is the DMM view, which the stream pauses. A move of a `loop` function
to C needs a reason why it never runs under the stream.

Not found: `core::fmt`; generic copies (every function with several
monomorphisations: < 1 KB above the largest copy); compiler settings that
help (the release profile is opt-level "z" already, so a separate "z"
crate gains nothing; LLVM `-inline-threshold` 25 / 50 and opt-level "s"
grow the image by 6 / 32 KB, -20 grows region B by 2 KB;
`-Zinline-mir=no` changes nothing).

Rodata: 16.3 KB of B are printable string runs. 10.0 KB of B's rodata is
referred to only by region C code. It cannot move: region C is the fuller
region, and an anonymous string has no section name for a link-native.x
pattern (each would need a named static in a `.rodata.cold*` section, as
`fastlog::JOB0`).

Done:

| change | region B | region C |
|---|---|---|
| README.TXT of the USB disk: one line per folder, the version and a link to the manual (3 075 -> 314 B of text; the file stays 2 048 B) | | |
| on-screen help: 2 pages of 5 rows (gestures, keys) instead of 7 pages of 8 rows (1 359 -> 230 B of text) | +3 984 (both texts) | +36 |
| DBG:HYSPI ([`src/hyspi.rs`](../src/hyspi.rs)) and DBG:ACDC? behind cargo features `dbg-hyspi`, `dbg-acdc` (without them: undefined header) | +432 | +3 132 |

| | release/0.4.0 (ae0dc031) | this branch |
|---|---|---|
| region A2 free | 2 208 B | 2 208 B |
| region B free | 8 728 B | 13 144 B |
| region C free | 13 036 B | 16 204 B |
| stack headroom (static worst path) | 20 744 B | 20 744 B |
| SRAM free | 24 244 B | 24 244 B |

#174 (e7c11934) with `--features dec-log,dec-modbus`:

| | #174 | #174 + this branch |
|---|---|---|
| region A2 free | 156 B | 156 B |
| region B free | 3 888 B (no link) | 9 328 B |
| region C free | 1 536 B (no link) | 4 216 B |
| stack headroom (static worst path) | 20 644 B | 20 644 B |
| SRAM free | 8 308 B | 8 308 B |

`dec-log` costs ~5 KB of region C (`.text.log_cold`), so with both
features region C, not B, is the tighter region. Next, not done (bytes
per decision):
- more DBG replies behind features (region C): `boot::put_info` (DBG:BOOT?,
  1.5 KB; a dozen emu tests read it), `touch_diag` (DBG:DIAG?, 0.7 KB),
  `acdctr::Trace::text` (0.6 KB), `Dmm::ohm600_dbg` (DBG:OHM6, 0.5 KB),
  `Sentinel::diag_text` (DBG:SENT?, 0.5 KB);
- texts over ~300 B in B, unchanged: the power meter's zero / V cal
  messages (487 B, `c-only`), the jack / overload codes (462 B), the
  generator notes (450 B), the settings file keys (428 B, a file format),
  the DBG:PROF? labels (368 B);
- with room in region C: the `c-only` code (4.8 KB: `dlog::SEnc`,
  `cring::min_max`, the gallery's `dlog` decoder copies, ...) and the
  named `c-only` rodata, function by function under the region C hooks.

### Region C audit #4 (2026-10-30, feat/dbg-shrink)

The owner's decisions after region B audit #9: the remaining debug replies
behind features, a smaller DBG:BOOT?, shorter texts.

| change | region B | region C |
|---|---|---|
| `dbg-diag`: DBG:DIAG? (`touch_diag`, `Touch::diag`, `scope::debug`) | +520 | +652 |
| `dbg-ohm6`: DBG:OHM6 (`Dmm::ohm600_dbg`) | +536 | +504 |
| `dbg-sent`: DBG:SENTinel? / :REG? (`Sentinel::diag_text`, `Dmm::reg_text`) | -56 | +824 |
| `dbg-src`: DBG:SRC (`dbgsrc_dmm`, `Dmm::src_text`; the same lines as test/scope-chain-0.4's feature) | +2 768 | +704 |
| the four together | +3 528 | +2 688 |
| DBG:BOOT? as `B2` + 57 hex words, one formatter loop, no PD6 fields | +624 | +1 128 |
| power meter status lines, the generator's limit note, the fast log's header comment | +392 | +156 |

Each feature's row: the build with that feature alone against the build
with none (LTO places code differently per build, so the rows do not add
up exactly). Without its feature a command is an undefined header; the emu
tests that read one probe it and skip those checks. DBG:ACDC:TRACE is
`acdc-trace` (feat/sram-extra-areas).

| | release/0.4.0 (4cce2a4b) | this branch |
|---|---|---|
| region A2 free | 2 208 B | 2 208 B |
| region B free | 17 592 B | 22 136 B |
| region C free | 16 608 B | 20 580 B |

### SRAM audit (2026-10-30, feat/sram-overlays)

Goal: room for the long scope record (`scope::CAP` 42 K u16) next to
0.4.0's GEN / CH2. The firmware runs DMM + scope + generator together, with
the USB disk and BT at any time, so a buffer is overlaid only where the code
rules out both being live. Who is live when:
[ram-budget.md](ram-budget.md#every-static-of-1-kib-or-more).

| change | SRAM | TCM |
|---|---|---|
| `csvlog::CAT` reply into the row FIFO (below) | +6 156 | -12 |
| `bt::TX` 8 192 -> 6 656 B (below) | +1 536 | |
| `touch::LOG` 160 -> 16 entries without `dbg-touch` | | +1 728 |
| `prof::P` and the profiler hooks only with `dbg-prof` | | +984 |
| `dmm::REC_COPY`, `vdisk::CSV_CUR2`, `lcd::DEFER` from `.sram_bss` to TCM `.bss` (CPU only, no DMA) | +2 688 | -2 688 |

The profiler hooks also leave region B (+4.2 KB). DBG:TOUCH? / DBG:TOUCH 0
and DBG:PROF? / DBG:PROF:RESET are undefined headers without their
features. A `--features dbg-touch,dbg-prof` build puts both buffers in
`.sram_bss` (TCM has no room for them next to the stack); host tests build
touch.rs with `dbg-touch`.

**MMEM:CAT? cache in the row FIFO.** `Cat` keeps only `ok` / `stamp` /
`len`; the reply is in `FIFO_BUF[..CAT_MAX]`
([csvlog.rs](../src/csvlog.rs) `Cat`).
- Writers of the FIFO: a CSV recording (`fifo()`, main loop) and a fast-log
  session (`lend_fifo()`, main loop: `Job::run`, the drain / flush,
  `run_begin`). No ISR touches it: the AD2 stream ISR writes only
  `logger::RING`.
- Both accessors clear `ok`. The cache is written only while `fifo_idle()`
  (`!csvlog::active() && !fastlog::active()`): in `catalog_warm` and after
  a scan in `catalog_scpi`. It is read only while `ok`.
- A fast-log session holds the disk (`gallery_free` false: MMEM:CAT? is
  Busy) and writes it, so it loses no valid cache.
- During a CSV recording every MMEM:CAT? reads the disk (~10 block reads).
  Before, a second query between two commits (every commit writes the
  flash, at most 60 s apart) was a copy. No stream that the cache is for
  runs then: the Wave view and inrush refuse to start during a recording,
  CH2 stops for one ([app_wave.rs](../src/app_wave.rs),
  [app_inrush.rs](../src/app_inrush.rs), [app_ch2.rs](../src/app_ch2.rs)
  `csvlog::active`), and the inrush page and `fastlog::begin` also refuse
  a fast-log session. An SCPI LOG:STATe ON session never warmed the
  cache.

**`bt::TX` not at 4 KiB.** A BT reply goes into the ring whole (`send`).
`Bt::respond_with` builds it on the stack and returns, so a 6 145-byte
reply (MMEM:CAT?, MMEM:DATA?, LOG:DATA?, INRush:DATA?, WAV:DATA?) has nowhere
else to wait. Sending it in parts would need one of:
- a wait in the main loop (~0.2 s at 115 200 baud): the Wave view's ring
  drops samples, which the MMEM:CAT? cache exists to prevent;
- building the reply again per part: LOG:DATA? takes records out of the
  logger's ring, so it cannot be built twice;
- a shorter BT block: `tools/dm40scpi.py get` takes an MMEM:DATA? piece
  under 6 000 bytes as the end of the file.

The ring is now REPLY_MAX + TX_SLACK + 255 B.

Pairs checked and not overlaid (each can be live with the other):

| pair | why both can be live |
|---|---|
| `vdisk::TAB` / `CSV_CUR` / `CSV_CUR2` with any scope buffer | the host reads the USB volume at any time, also while the scope page draws |
| `logger::RING` with `csvlog::FIFO_BUF` | a fast-log session drains the ring into the FIFO (`fastlog` `Job::run`) |
| `logger::RING` with `csvlog::TAIL` | LOG:STATe ON does not refuse a recording, nor LOG:CSV ON a host session or CH2 (`App::rec_conflict` covers the Wave view and the inrush page only) |
| `scope_ch2::STORE` with `csvlog::TAIL` / `FIFO_BUF` | a recording can start while CH2 streams (as above); the FIFO head is the MMEM:CAT? cache that CH2 streams use |
| `diskbuf::ARENA` with `csvlog::TAIL` | a commit uses ARENA [0] / [1] and TAIL together |
| `fastlog::JOB` with `csvlog::JOB` | the sessions exclude each other, but each job keeps its result for LOG:CSV? / the fast logger's status after it ends |
| `bt::TX` with the USBTMC reply buffer | both transports answer at the same time |
| `scope_ui::GRAPH`, `scope_page::CACHE` with the inrush page | no inrush-only buffer to put there (the inrush record is `scope::BUF`, lent) |
| `gen::WAVE` | the DAC's circular DMA source while the generator outputs, with everything |

Not changed here: `dmm::acdctr::TR` (5 388 B, DBG:ACDC:TRACE); the next
section puts it behind a feature.

| | before (release/0.4.0, e3229f7a) | after |
|---|---|---|
| region A2 free | 2 208 B | 2 208 B |
| region B free | 8 280 B | 12 400 B |
| region C free | 13 556 B | 13 996 B |
| stack headroom (static worst path) | 20 744 B | 20 760 B |
| SRAM free | 24 244 B | 34 628 B |

On test/scope-chain-0.4 (the decoder's paths: static worst stack path
24 800 B, 240 B more than 0.4.0) TCM has room for `dmm::REC_COPY` and
`ui::GEN_SP` (484 B) only; `vdisk::CSV_CUR2` and `lcd::DEFER` stay in
`.sram_bss`, and `touch::LOG` was in SRAM there already. The SRAM
freed takes `scope::CAP` from 34 K to 40 K u16, not to 42 K (that needs
4 KB more): the raw ring of the fast timebases 34 816 -> 40 960 samples,
the packed ring (`PCAP`, white noise) 52 400 -> 62 240, the post-trigger
part of a coded Single at the centre 39 036 -> 48 876. The `recovery-usb`
variant keeps CAP 36 K / PCAP 55 000 again (it has its own MSC buffers).

| | before (a5352720, CAP 34 K) | after (CAP 40 K) |
|---|---|---|
| region A2 free | 156 B | 156 B |
| region B free | 9 440 B | 14 056 B |
| region C free | 6 064 B | 6 472 B |
| stack headroom (static worst path) | 21 312 B | 20 620 B |
| SRAM free | 9 460 B | 8 308 B |

Then (feat/sram-extra-areas, the next sections) `scope::CAP` goes back to
42 K u16 / `PCAP` 65 520 (the raw ring 43 008, the post-trigger part of a
coded Single at the centre 52 156; the test floors as on
feat/scope-long-record). On this branch `ui::GEN_SP` also moves from TCM
to the backup SRAM, except in a `dbg-prof` build (`prof::P` takes that
room).

| | CAP 40 K, release/0.4.0 + extra areas merged | CAP 42 K |
|---|---|---|
| region A2 free | 156 B | 156 B |
| region B free | 14 904 B | 14 912 B |
| region C free | 6 624 B | 6 624 B |
| stack headroom (static worst path) | 20 596 B | 21 076 B |
| SRAM free | 14 756 B | 10 660 B |
| backup SRAM free | 1 424 B | 936 B |

### AC+DC trace behind a feature (2026-10-30, feat/sram-extra-areas)

The owner does not want DBG:ACDC:TRACE in the firmware. The cargo feature
`acdc-trace` ([Cargo.toml](../Cargo.toml), off by default; the same feature
as on `test/scope-chain-0.4`, commit 33dd4cb9) builds it:
- without it [acdctr.rs](../src/acdctr.rs) keeps one ring entry, `live()`
  is constant false and the `Dmm` hooks test `acdctr::ON` first, so their
  code is dropped; DBG:ACDC:TRACE[?] is an undefined header;
- with it the ring has 64 entries (was 192) and 32 events after the
  trigger (was 48).

| | before (0e7ffa6e) | after |
|---|---|---|
| region B free | 12 872 B | 13 912 B |
| region C free | 13 476 B | 14 188 B |
| SRAM free | 34 628 B | 39 980 B |

`--features acdc-trace`: SRAM 38 212 B, B 12 888 B, C 13 508 B.

### AC+DC methods behind a feature (2026-10-31, feat/acdc-meth)

The AC+DC prototype methods B and C ([acdcm.rs](../src/acdcm.rs),
[acdc-approaches.md](acdc-approaches.md) section 10) are debug code: the
cargo feature `acdc-meth` (off by default) builds them. Without it
`dmm.rs` has an empty `am` module (`am_on()` constant false, so method A's
code is as before and the hooks are dropped), no `AM` static, and
DBG:ACDC:METH / STAT? / ZERO / R0 are an undefined header. DBG:ACDC:RANG
stays.

| | before (fe59a782) | after |
|---|---|---|
| region B free | 25 032 B | 31 536 B |
| region C free | 21 216 B | 24 316 B |
| SRAM free | 45 460 B | 45 940 B |

`--features acdc-meth`: SRAM 45 460 B, B 24 840 B, C 21 216 B.

### Extra RAM areas (2026-10-30, feat/sram-extra-areas)

The device probe of [PR #183](https://github.com/opendm40/opendm40/pull/183)
found two RAMs outside [link-native.x](../link-native.x)'s TCM + SRAM, both
~5 cycles a load:

| area | range | size | kept over |
|---|---|---|---|
| backup SRAM after the settings record | `0x40024124..0x40024FE7` | 3 780 B | a reset and an update, not a power-off |
| USBHS FIFO RAM (the DM40 has no USBHS port) | `0x40060000..0x40060FFF` | 4 KiB | needs `RCU_AHB1EN` bit 29 (USBHSEN); **32-bit stores only** |

**The USBHS FIFO RAM ignores the store size.** A byte store writes the
whole 32-bit word, the stored byte in all four lanes (a halfword store
both halves: *inferred*). A CSV
recording with `csvlog::TAIL` written byte by byte left every word of the
DLG as one byte ×4 (`DLG1` → `31 31 31 31`), and `MMEM:DATA?` of its CSV
answered -250 "Mass storage error". Reads of any size and word stores
work. So a buffer there is a `WordRam` ([wordram.rs](../src/wordram.rs)):
declared only by its `usbhs_static!`, reached only by `read(i)` /
`write(i, u32)`, one volatile word access each; partial words are merged
in a register and written whole. build.sh fails when `.usbhs_bss` is
named outside wordram.rs. The probe results:
[PR #183](https://github.com/opendm40/opendm40/pull/183) `hidden-memory.md`.

**Link.** Two `MEMORY` regions and three NOLOAD sections:
- `.bkp_record` holds `persist::RECORD_SPACE`, a `[u32; WORDS + 3]` that
  is never accessed: it puts the settings record's size from
  [persist.rs](../src/persist.rs) into the link. Asserted at 0x40024000 and
  non-empty.
- `.bkp_bss` follows it; asserted to start at or after the record's end and
  to end at or below `0x40024FE8` (`persist::BKP_SEQ`: the sequence words,
  then the boot words; persist.rs asserts the same constant).
- `.usbhs_bss`, asserted inside the 4 KiB; only `wordram::usbhs_static!`
  places anything there (build.sh).

**Boot.** `native::extra_ram_init`, right after `init_ram` and before the
recovery / normal split: `boot::bkp_enable` (PMU clock, BKPWEN, BKPSRAM
clock), USBHSEN, then both sections zeroed word by word. `.bss` zeroing
does not cover them; the backup SRAM keeps the previous image's bytes over
a reset; the FIFO RAM is random after power-up. `board::init` sets USBHSEN
too. The record and the words above `.bkp_bss` are never written.

**What moved.** CPU-only buffers, none touched by DMA and none on the AD2
stream ISR's paths:

| buffer | size | area | from | who touches it |
|---|---|---|---|---|
| `csvlog::TAIL` (`WordRam<1024>`) | 4 096 | USBHS | SRAM `.dmabuf` | `rec_start` (C: 0xFFFFFFFF word fill); `csvlog::poll` while a CSV recording commits: the FIFO bytes packed into words (`put_bytes`: the partial first / last word read, merged, written back), then `write_step`, which reads it word by word (wordram.rs `BlockSrc`; the SPI flash takes it by CPU writes) |
| `screenshot::JOB` | 452 | backup | SRAM `.dmabuf` | `init`; per pass `busy` / `frozen` flags (tick, run, msc poll); the shot itself is C (`step`, `finish`) |
| `vdisk::MIR` | 144 | backup | `.sram_bss` | `mir_read` (a host read of the mirror file), `Scpi::handle`, `mirror_boot` (C) |
| `vdisk::STATE` | 372 | backup | SRAM `.dmabuf` | `init` (C); a few field loads per 512-byte sector in `read_block` / `read_busy` / `read_wait` |
| `upload::UP` | 368 | backup | `.sram_bss` | per pass `active` flag; the upload is C (`begin`, `data_msg`, `flush`, `end`) |
| `scope_csv::PLAN` | 264 | backup | SRAM `.dmabuf` | C only (`sp_save_csv`, `screenshot::step`) |
| `app_touchtest::TT` | 168 | backup | `.sram_bss` | C only (the touch test page) |
| `prof::P` (`dbg-prof` builds) | 984 | backup | `.sram_bss` | the profiler hooks (debug) |

Proofs:
- **DMA.** Every DMA memory address the firmware programs: `bt::RX` /
  `bt::TX` (bt.rs, `*_M0ADDR`), `scope::LATCH` and the scope record
  (scope.rs, S3 / S4; `scope::BUF` or the inrush record lent from it),
  `gen::WAVE` (gen.rs, S5), and the HY3131 SPI frames `TXI` / `RXI` /
  `TXB` / `RXB` (logger.rs → hy3131.rs `dma_arm`). None of the buffers
  above. (`.dmabuf` held PLAN, JOB, STATE only as NOLOAD SRAM.) USBFS moves
  its FIFO by CPU, the LCD is CPU writes over FSMC, the SPI flash has no DMA.
- **The AD2 stream ISR.** build.sh disassembles every function reachable
  from `ad2_stream_irq` (34 today) and fails when one holds an address in
  `.bkp_bss` or `.usbhs_bss` (literal-pool word or movw / movt pair). The
  addresses the ISR loads are TCM / SRAM statics (`logger::isr::*`, its
  DMA frames) and peripherals. In the `no-ad2-isr` variant the main-loop
  sampler (`Logger::poll` / `sample`, inlined into `run`; `Producer::push`
  / `tick`) names none of the moved modules, and push / tick hold no such
  address.
- **Hot paths.** Region C code never runs while a stream does
  ("The slow flash bank while an AD2 stream runs"), so the C-only buffers
  are idle then. The rest cost a few 5-cycle loads per main-loop pass
  (flags) or per USB sector. `csvlog::TAIL` is touched once per commit of
  a CSV recording: the bytes are packed in a register, one 5-cycle word
  store per 4 bytes (before: a 5-cycle store per byte), and `write_step`
  reads 1 024 words per pass (compare, program, read back) through a
  `BlockSrc` call. *Estimated* ≤ 40 000 cycles (0.2 ms) per 4 KiB block,
  spread over the `write_step` slices, next to the 3 × 4 096 SPI byte
  transfers and a block program of several ms. A recording refuses the Wave view and
  inrush and stops CH2; only an SCPI LOG:STATe ON session can stream
  meanwhile, and the ISR does not wait for the main loop.
- **Size.** The link asserts above; build.sh prints both areas' free bytes.

Checked and left where they are:

| buffer | why not |
|---|---|
| `lcd::DEFER` | read by every `fill_rect` / `set_window_cmd` (the drawing hot path, also while the scope streams) |
| `vdisk::CSV_CUR` / `CSV_CUR2`, `dlog::SCPI_GEN` | CSV / MMEM:DATA? generators that run in place, per byte of a host read |
| `fastlog::JOB`, `csvlog::JOB`, `scope_wav::SCREEN` | per sample / row / frame while a stream or a recording runs |
| `vwrite::W0` (1 376) | read on every pass and every USB sector read by many region B functions; the next candidate if SRAM gets short |
| `ui::GEN_SP`, `dmm::REC_COPY`, `gallery::G`, `scope_csv::SNAP` | fit and are cold, but they are in TCM on the scope chain, where SRAM is short; room kept for `prof::P` |
| `touch::LOG` (`dbg-touch`, 1 924), `acdctr::TR` (`acdc-trace`, 1 804) | do not fit next to `prof::P`; stay in `.sram_bss` |

`emu/dm40.repl` maps the FIFO RAM as
[WordOnlyRam.cs](../../emu/peripherals/WordOnlyRam.cs): a byte store writes
the byte into all four lanes of its word, as on the device (a halfword
store both halves: *inferred*); the clock gate is not modelled. The backup
SRAM was already plain memory, kept over DBG:RESET, zero in a new run.
[emu/xramtest.py](../../emu/xramtest.py) fills both areas with 0xA5 before
the firmware starts, saves a setting, takes a screenshot, records a CSV
long enough for several commits and a second 4 KiB block, reads its DLG
and CSV with MMEM:DATA? and compares the CSV with
[dlog.py](../tools/dlog.py) of the DLG byte for byte, opens the touch test
page, then DBG:RESET: SETTINGS,BKP with the same record sequence number,
the setting kept, and the same steps work again. The byte-written TAIL
fails it.

| | before (11eb22cc) | after |
|---|---|---|
| region B free | 13 912 B | 13 912 B |
| region C free | 14 188 B | 14 104 B |
| stack headroom (static worst path) | 20 760 B | 20 736 B |
| SRAM free | 39 980 B | 45 852 B |
| backup SRAM free | | 2 008 B |
| USBHS RAM free | | 0 B |

Feature builds (SRAM / backup SRAM free): `dbg-prof` 45 852 / 1 016,
`dbg-touch` 43 932 / 2 008, `acdc-trace` 44 084 / 2 008, all three
42 164 / 1 016 (region B 8 688).

Device checks still open:
- The supply current with USBHSEN on (*unknown*).
- The settings record and file over power cycles with this image.

### Flash outside the image (2026-10-22, from the 2026-10-13 dump; not tried)

The GD32F427 here has 1 MiB (flash density 0x0400 at `0x1FFF7A22`, as
emu/run.py models it): sectors 0-3 16 KiB, 4 64 KiB, 5-11 128 KiB, erase
by whole sector. Only the low 512 KiB are zero-wait. The FS-loader writes
an update body only to `0x08004000..0x080A0000`, so anything outside would
have to be programmed by opendm40 itself (from a companion file on the SPI disk
or a compressed copy in the body), not by the update file.

| range | sector | 0xFF in the dump | usable? |
|---|---|---|---|
| `0x08000000..0x08004000` | 0 (ROM bootloader) | ~9.7 KB, scattered | no: the boot ROM's sector |
| `0x0800460c..0x08008000` | 1 (region A tail) | 14.8 KB | inside the image, zero-wait: used since 2026-10-22 as region A2 (the `.a2` output section, "Region A2" above) |
| `0x080AE700..0x080C0000` | 9 (FS-loader + boot data at `0x080A4000`) | ~70 KB | write-once: it can be programmed but never erased (the erase takes the FS-loader and the UID-bound data with it), so never updated |
| `0x080C0400..0x080E0000` | 10 (calibration / settings) | ~127 KB | write-once likewise (an erase loses the calibration); whether the stock app rewrites this sector on a settings save is not known: a return to stock could wipe it |
| `0x080E0000..0x080FF000` | 11 (`0x080FF000` boot word) | ~124 KB | erasable, but the FS-loader erases sector 11 at the start of every install (`stock/re/docs/fsloader-install-trigger.md`): opendm40 would re-program it after each update |

Sector 11 is the only candidate. Its risks: a version mismatch between the
image and the overlay (needs a CRC / version check and a "not available"
path, never a jump into stale code); ~124 KB programmed at boot under the
watchdog; power loss mid-program (the check must catch a partial overlay);
a wrong sector code in the erase (SN = code >> 3: 0x58 = 11, 0x50 = 10 =
calibration, 0x48 = 9 = FS-loader) bricks the recovery path or loses the
calibration; slow flash, so `.cold`-class code only; and the
`0x080FF000` word (stock app's boot flag) shares the sector.

**`build.sh`** checks the binary again:
- the 16 bytes at `0x08008000`, and all padding from the end of region A2 to
  `0x08008010`, are 0xFF;
- region A2 starts at the end of region A and ends at or below the marker;
- the end is below `0x080A0000`;
- word0 = `0x2002F7F8`;
- the reset vector is a Thumb address in region A.

It then runs `../stock/dm40fw.py pack --pad`, which pads the body with 0xFF
to whole 128-byte file blocks ("Update file format" above) and computes both
header words itself (no `--like`):
- word1 = body length (incl. the padding);
- word2 = `0x10000 | CRC16(body without the 12 marker bytes)`;
- word0 = CRC16 of words 1–2, with its complement in the high half.

Finally it runs `dm40fw.py info` (prints the three words, SP and reset
vector).
`dm40fw.py unpack` of the packed file gives back the body byte-for-byte plus
the 0xFF padding (< 128 bytes). `build.sh` fails if the packed file is not a
multiple of 128 bytes or the padded body reaches `0x080A0000`.

**Word0 is the stock app's own initial SP** (`0x2002F7F8`). The FS-loader
loads MSP from word 0 of the vector table at `0x08004000` and jumps to the
reset vector in word 1. The launch tests of the ROM and the loader check the marker
and the body CRC ("Boot chain" above; `stock/re/docs/fsloader-install-trigger.md`, "Boot decision chain");
no SP check is known, and with the stock value any would pass as for stock.
`reset` moves SP to the TCM top before anything uses the stack.

## Shim (reset path)

The letter-number tags (B2, H2, ...) are items of the safety review
`stock/re/docs/history/native-review-2026-10-13.md`.

`reset` (asm):
1. `cpsid i`.
2. `CONTROL = 0` (MSP, privileged; L3).
3. TCM clock on.
4. FPU on (CPACR).
5. `SP = 0x10010000`.
6. `VTOR = 0x08004000`.
7. Call `shim_main` → `boot::shim()`. This uses only the stack, no
   `.data`/`.bss`:

0. **PA2 entry:** backup SRAM on; if `BKP_REQ` = `0x4C324150`,
   `loader_pa2()` ("PA2 entry" above); it does not return.
1. **Watchdog:** `wdt::start::fwdgt()` writes `0xCCCC`, `0x5555`, waits
   (bounded) for PUD/RUD, sets PSC = 6 (/256) and RLD = 2500 (~16–20 s), then
   `0xAAAA`. From here on any hang resets into the shim. Every wait below
   feeds the watchdog.
2. **Power hold, then light.**
   - **Power:** PE2 = 1 (push-pull output, level set before the mode).
     - The ROM already raises PE2 and PC4 first thing on every reset
       (`0x08000574`). The stock app keeps PE2; this re-asserts it.
     - PE2 is the DMM analog supply; the stock power-off drops PE2 and PC4,
       so one of them holds the power (which one is unknown:
       [hw/README.md, Power and battery](../../hw/README.md#power-and-battery)).
   - **Light:** the backlight goes full on right away, as plain GPIO PA6 high
     (H2): the power-on press lights the screen within milliseconds.
   - `board::init` later hands PA6 to TIM13 with the duty at 0, until
     `App::new` sets the level.
3. **Reset flags:** read `RCU_RSTSCK` (`0x40023874`), write RSTFC (bit 24),
   read it back.
   - Flag bits: 25 BOR, 26 PIN, 27 POR, 28 SW, 29 FWDGT, 30 WWDGT, 31 LP.
   - The stock never clears these flags; a running stock showed `0x3E000002`.
4. **PD6 input with pull-down** (B2; the key:
   [hw/README.md, Power key, PA2, buzzer](../../hw/README.md#power-key-pa2-buzzer)), as the stock key init does
   (`FUN_0800b7e0` in `stock/re/analysis/app_decompile.c`:
   input, PUPD = 2). Then about 10 µs for the pull to settle.
   - The ROM and loader leave PD6 floating, so without this a floating 1
     would read as a held key (the `recovery-usb` variant reads the key
     without `board::init`).
   - Pressed = 1 is confirmed in the stock code.
5. **Clock:** try `init_core(Hxtal)`, then `init_core(Irc16m)`. If both fail,
   SYSCLK stays on the 16 MHz RC (`clock` = FAILED).
   - The `Irc16m` path runs the PLL from the internal RC with M = 16. The PLL
     input is the same 1 MHz, so SYSCLK is 200 MHz and PLLSAI gives 48 MHz,
     only with RC accuracy.
   - `init_core`:
     - IRC16M on, AHB /2, SYSCLK → IRC16M, PLL and HXTAL off, PLL reset value;
     - HXTAL on (Hxtal path only);
     - LDOVS = 0b11, APB2 /2, APB1 /4;
     - **RCU_PLL = `0x29406419`** (measured stock value), PLL on;
     - over-drive handshake (ODEN, ODSWEN);
     - SYSCLK → PLL, TIMPRE.
   - **FLASH_ACR is not written.**
   - Every wait is a bounded poll.
   - The ROM and the loader spin forever on a dead HXTAL. So a failure here
     can only be transient (M4).
6. **Backup SRAM:** read and **consume** the request word; read the
   previous-mode word and the crash counter.
   **Key hold** (`key_gesture`, DWT cycle counter): key held 4 s after the
   shim started → cue (backlight off, ~0.3 s beep on PA7 as plain GPIO,
   backlight on); still held 0.5 s more → `BKP_REQ` = `0x4C324150`,
   SYSRESETREQ → step 0 on the next pass. Released in the 0.5 s →
   `KEY_HOLD_ABORTED`, normal boot. Every wait has a pass cap that counts as
   a release (N1).
7. **Decide.** The first matching row wins.

   A watchdog reset is **genuine** (one of our own hangs) when FWDGTRSTF is set,
   neither SWRSTF nor PORRSTF is set, and `BKP_LAST` = NRML (H3). The shim
   clears the flags on every native boot, so a hang during a native session
   shows FWDGT alone. A loader install adds SW, and a power-on adds POR. A
   stale FWDGT from a stock session therefore never counts.

| Condition | Mode | `reason` |
|---|---|---|
| `BKP_REQ` = `0x52454356` (consumed; set by nobody now) | recovery | `REQUESTED` |
| key held to the cue, released in the 0.5 s | normal | `KEY_HOLD_ABORTED` |
| clock failed, crash counter +1 below `CLOCK_RETRIES` (2) | **SYSRESETREQ**, retry the boot | (`CLOCK_FAIL`) |
| clock failed, counter +1 at 2 or more | normal, on the clock it has | `CLOCK_FAIL` |
| genuine watchdog (crash counter +1) | **normal** | `WDT_RETRY` |
| FWDGTRSTF, previous mode = recovery | normal | `WDT_AFTER_RECOVERY` |
| FWDGTRSTF otherwise (stale / with SW or POR / unknown previous mode) | normal | `WDT_STALE` |
| otherwise | normal | `NONE` |

8. **Hand over:** the previous-mode word and the crash counter are updated
   as follows:
   - Write the chosen mode to `BKP_LAST`.
   - On recovery, clear the crash counter: a fresh start after the
     reinstall.
   - Call `native::start`.

**Crash counter** (M1, M4): a diagnostic only (`DBG:BOOT?` `crash_word`).
- Every genuine watchdog reset adds 1 and reboots normally (`WDT_RETRY`);
  no count leads to the loader (removed in 0.4.0, owner 2026-10-29). A
  panic stops feeding, so it ends in the same watchdog reset.
- `boot::mark_healthy(now_ms)` clears it once after 60 s of running main
  loop since the boot ([`src/main.rs`](../src/main.rs)), so it shows the
  hangs in a row. Emulator test: [`emu/recoverytest.py`](../../emu/recoverytest.py)
  (wdt-one, wdt-two, wdt-three, wdt-healthy, release).
- **Clock failure** counts too: while the count is below 2 it retries the
  boot with a SYSRESETREQ, which redoes the ROM/loader clock set-up; after
  that opendm40 runs on the clock it has (`CLOCK_FAIL`).

**Backup-SRAM words** (4 KiB at `0x40024000`; [`src/persist.rs`](../src/persist.rs)
keeps the settings record from `0x40024000` and two sequence words at
`0x40024FE8`, below these):

| Address | Use |
|---|---|
| `0x40024FF0` `BKP_CRASH` | crash counter `0x435200nn` |
| `0x40024FF4` `BKP_TRIG` | last loader entry: `0x54520010` marker zeroed, `0x54520011` failed, `0x54520012` PA2 entry, `0x54520013` PA2 entry without the lock read back |
| `0x40024FF8` `BKP_REQ` | request word, consumed by the shim: `0x4C324150` PA2 entry (`DBG:LOADER`, key hold); `0x52454356` recovery (set by nobody now) |
| `0x40024FFC` `BKP_LAST` | mode of the previous boot: `0x4C4D524E` normal / `0x52564352` recovery or PA2 entry; **cleared by `DBG:UPDATE`** (H3) |

**A hang that repeats on every boot** (opendm40 dies before its USB is
up) is a reboot every ~20 s. The key hold at power-on still reaches the
loader, because the shim reads the key before `native::start`; if the shim
itself hangs, only the TX-GND short is left
([hw/README.md, Hardware recovery](../../hw/README.md#hardware-recovery)).
A broken recovery (request word) does not repeat itself: a hang in recovery
(previous mode = recovery) is followed by a normal boot.

**First boot after an install:**
- After a loader install (SW reset) or a power-on, FWDGT is never "genuine",
  so the boot is normal.
- A stale stock FWDGT flag shows up as `WDT_STALE`, or the reason is `NONE`
  when the flags were clear.

## Recovery mode (`src/recovery.rs`)

Recovery uses the loader's own update screen and disk, reached by zeroing the
marker ("Boot chain" above; `stock/re/docs/fsloader-install-trigger.md`,
"Update-loader install & error paths").

**Why the `DM40RCVR.BIN` + power-off variant is fragile.** If the user releases
the power key quickly at the next power-on, PA2 reads high and the ROM launches
our (possibly hung) app directly. The loader never sees the file.

**Recovery action (default):**
1. Keep the power hold (PE2) and feed the FWDGT.
2. `boot::zero_marker()` programs three words of `0x00000000` at
   `0x08008000..0x0800800B` with the FMC, exactly like the loader's
   `0x080a2564`/`0x080a7aec`:
   - Unlock if `LK` is set: KEY `0x45670123` then `0xCDEF89AB`.
   - Bounded wait for BUSY (the FWDGT is fed while waiting).
   - Clear STAT with `0x1F3` (every error bit tested; the loader writes
     `0xD3`, which leaves PGMERR set if it was).
   - CTL: PSZ = 32-bit (`0x200`), then PG.
   - For each word: write `0`, `dsb`, bounded BUSY wait, then check the error
     bits (`0x1F2`).
   - Clear PG, set LK.
   - Verify that all three words read back as 0.

   Details:
   - There is no erase: programming 0-bits over existing bits is what the
     loader itself does.
   - Words already 0 are skipped.
   - The address is a constant; **no other flash address is ever written.**
3. On success: `BKP_TRIG = 0x54520010`, then SYSRESETREQ.
   - The ROM rejects the marker whatever PA2 reads, so the loader runs.
   - The loader shows "open file error" and its **working USB disk** appears.
   - This works from a warm reset; no file is needed.
4. On failure: the three words do not read back as 0. The read-back is the
   only success criterion. FMC error flags and a timeout are recorded
   separately as `fmc_last` (M5).
   - `BKP_TRIG = 0x54520011`, and the FWDGT stops being fed.
   - The watchdog resets the unit; the shim sees that the previous boot was
     recovery and boots normally.
   - The next normal boot shows `last_trigger` = `0x54520011` in `DBG:BOOT?`.
     Only that backup-SRAM word survives the reset: `fmc_last` is a `.bss`
     static, zeroed by `init_ram`, so the FMC error bits / timeout of the
     failed attempt are **not** visible after the reset (`fmc_last` then reads
     0). `fmc_last` is only meaningful after a `DBG:UPDATE` that answered
     "Flash error" (no reset follows then).

This is a **one-way door into the vendor updater.** The app bytes stay intact,
but nothing can make that installed copy bootable again except a vendor install
(SWD is read-locked).

**Recovery code size** (2026-10-13, `llvm-nm`): about 490 bytes:
- `recovery::run` 0x38, `zero_marker` 0xE4, `fmc_wait` 0x38;
- `sysreset` 0x34, `record_trigger` 0x18, `latch_on` 0x40.

**User procedure:** the meter restarts into the loader's update screen
("open file error") with its USB disk (28e9:028f); what to copy and press is in
[the manual, Install a file from the update loader](../../docs/manual/README.md#install-a-file-from-the-update-loader).

**Other entries** to the same state from a running native opendm40:
- `DBG:UPDATE`:
  - Clear `BKP_LAST` (H3), zero the marker, then the deferred SYSRESETREQ.
  - "Flash error" with no reset only if the marker did not read back as zero.
  - **Sync and eject** a just-copied file first (L5).
- **Developer flow:** put a new `dm40*.bin` in the SPI root (`DBG:UPLOAD`, or a
  copy onto the volume in "PC can edit" mode: "Firmware update without the
  writable disk" and "Host writes" below), then `DBG:UPDATE`. The loader
  finds the file and installs it.
- The shim still honours the backup-SRAM request word (`BKP_REQ`), but nothing
  sets it now. It is kept for a later in-app "enter recovery".

**Non-default variants (tests only):**
- **`recovery-file`:** writes one 0-byte `DM40RCVR.BIN` root entry on the SPI
  FAT, only the root block, FAT untouched (`fatren::make_trigger`), then powers
  off (PE2 = PC4 = 0).
  - The update screen appears only with PA2 held low (debug-header TX shorted to GND).
- **`recovery-usb`:** our own minimal USB-MSC disk on the W25Q128 (msc.rs path,
  no USBTMC; serial = the chip UID as in normal mode, `REC1` until 2026-10-23).
  - Any SCSI START STOP UNIT, or a key hold of 2 s or more, powers off
    (an eject also makes the disk report no medium for that last second).
  - A short key press reboots.

## Normal native mode

`native::start` → `pre_main` → `main()`. `pre_main`
([`src/native.rs`](../src/native.rs)), before USB exists, feeding the FWDGT
between steps: `board::init` → fatren → `calbackup::boot` (safety copy) →
`fatdir::boot` (LOGS / SHOTS / SCOPE) → `setfile::boot` (settings copy) →
`vdisk::mirror_boot` (slow-bank mirror), with the boot splash
([`src/splash.rs`](../src/splash.rs)) from `board::init` on. Then `main()`
draws the first frame, and `native::before_usb` runs `vdisk::init` →
`vdisk::boot_refresh` before the touch reset and USB.

**Board bring-up** ([`src/board.rs`](../src/board.rs)): what the stock
firmware initialised before opendm40 ran when it was RAM-loaded, found by an
audit of all modules, with values from the stock boot `FUN_0800af34` (the
pins: [hw/README.md, Pin table](../../hw/README.md#pin-table)):

- **GPIO port clocks A..E.**
  - The first `AHB1EN |= 0x1F` used to be in `Scope::new`.
  - `lcd`, `Dmm::new` (`hy3131::init_pins`, run once), `dmm.select` (relays)
    and `buzzer_init` (PA7 AF) all ran earlier. On a cold boot their GPIO
    writes would have been lost.
- **DMA0 clock (AHB1EN bit 21).** opendm40 never enabled it. The BT UART RX DMA
  and the generator DAC DMA only worked because stock had done it. DMA1
  (bit 22) is enabled too.
- **Rails as at stock boot:** PC0 = 1 (BT module), PE2 = 1 (DMM supply / power
  hold), 10 ms, PC4 = 0. PE2 used to be driven first by `show()`/`power_both`,
  after the HY3131 had already been configured.
- **PD6** as input.
- **FSMC bank 1 NE1:**
  - AHB3EN clock is enabled first, then AF12 with pull-up and very high speed
    on PD0/1/4/5/7/8/9/10/11/14/15 and PE7..15.
  - BCR1 = `(old & 0xFFF70081 & ~1) | 0x5011`, then MBKEN.
  - BTR1 = `0x00000F04` (read: ADDSET 4, DATAST 15).
  - BWTR1 = `0x00000201` (write: ADDSET 1, DATAST 2).
  - Decoded from the stock `FUN_0800506c` call in
    `stock/re/analysis/app_decompile.c` and its mask
    literal `0xFFF70081`. The live stock values read on the device
    (2026-10-13): BCR1 `0x5091`, BTR1 `0x0F04`, BWTR1 `0x0FF00201`.
- **ILI9488:**
  - the stock FUN_0800a0e4 sequence (ID read 0xD3, then C0 … E1);
  - 0x11, wait 120 ms, 0x29;
  - MADCTL 0xE8 and a black screen.
- **Backlight TIM13:**
  - APB1EN bit 7; PSC 99, ARR 99; CCMR1 0x68 (PWM1 + preload); CCER CC1E.
  - CCR1 = 0, dark until `App::new` sets the level.
  - CR1 = ARPE | CEN; PA6 AF9.
  - opendm40 used to write CCR1 only.
- **Clock, TIMPRE and the PLLSAI input:** set by the shim's `init_core`.
  PLLSAI itself stays in `usb.rs`.
- **FWDGT:** started by the shim. opendm40 used to only feed it.
- **Fonts:** embedded in the image ([fonts.md](fonts.md)).
- **Unchanged:**
  - spiflash, usb and persist were already self-contained.
  - touch and power configure their own pins, but needed the port clocks.
  - Calibration (`0x080C0000`) and boot data (`0x080A4000`) are only read.

**Watchdog policy:**
- **Main loop feeds while it runs:** `INITIAL_HOLD_MS` = `u64::MAX/4`, so the
  loop feeds the FWDGT for as long as it runs.
- **Hang or panic → reboot:** a panic shows the magenta bar and stops
  feeding; a hang stops feeding too. Every FWDGT reset reboots normally
  (`WDT_RETRY`, crash counter +1).
- **`DBG:RELEASE`:** stop feeding, so a WDT reset follows after about 20 s.
  It counts as a crash, the same as a hang.
- **`DBG:RESET`:** SYSRESETREQ → normal boot.
- **`DBG:UPDATE` / `DBG:RECOVER`:** `boot::enter_loader_update`: clear the
  previous-mode word `BKP_LAST` (H3) **and reset the crash counter**
  `BKP_CRASH` to `0x43520000` (N2: the count starts at 0 after the
  reinstall), zero the loader marker, then SYSRESETREQ → the vendor update
  screen with its USB disk (see Recovery).
- **`DBG:BOOT?`** returns `B2`, 57 hex words and fatren's renamed 8.3 name
  (field list: [SCPI reference, DBG:BOOT?](../../docs/manual/scpi-reference.md#dbgboot);
  [`tools/dm40dbg.py`](../tools/dm40dbg.py) `boot` decodes it): mode,
  reason, clock, reset flags, `last`, `last_trigger`, the marker words and
  their validity, `crash_word` (`435200nn`), healthy, `fmc_last`, the FSMC
  values found before `board::init` rewrote them, fatren's result, the
  calibration safety copy, the stack high-water mark, the SPI-flash clock,
  fatdir, the calibration record's CRC, the settings fields and the
  start-up phase times (`native::boot_words`).

  `last_trigger` codes: `0x54520010` = marker zeroed, `0x54520011` = zeroing
  failed. With `recovery-file`, `0x5452000n`, n = 0 Created, 1 AlreadyThere,
  2 NoMedium, 3 BadBpb, 4 NoSpace, 5 Error.
- **`DBG:PING?`:** its `keepalive_left` field saturates at `4294967295`
  (`u32::MAX`): the boot hold (`u64::MAX/4`) reads as no limit.
- **Power-off** (key held 1.5 s, or idle timeout): if the unit stays
  powered (USB), `wdt::powered_off()` turns the backlight off and keeps
  feeding. The next key press after a release reboots normally. Without this,
  a charging meter would reboot 20 s after the idle shutdown.

## fatren (`src/fatren.rs`)

The FS-loader deletes the file it installed, after a successful install
("Boot chain" above; `stock/re/docs/fsloader-install-trigger.md`, "Who deletes the
installed dm40*.bin"),
so normally fatren finds nothing to rename. The delete's result is ignored
by the loader; a file it could not delete (inferred: a read-only one) would
be installed again on every entry into the install state. fatren renames such
a file to `_DM40PRV.BIN`, so the image is kept and the user restores it by
removing the leading underscore.

**Which file (review H1).** A cold power-on does **not** mean the loader ran:
when PA2 reads high the ROM launches a valid app directly. So fatren renames
only a
`dm40*.bin` whose **decoded 12-byte update header equals the 12-byte record at
`0x08008000`** — the loader writes exactly that header there when it installs
an image. The first 128-byte block of the file is decoded as in
`stock/dm40fw.py` `decode_block` (`fatren::header_of`). Every other match is
left alone and counted as `staged` (the loader installs it on its next run).

**When:**
- Native normal mode, from `native::pre_main` before `main()`. USB does not
  exist yet, so the host cannot have the volume mounted.
- Every normal boot, whatever the reset cause (no PORRSTF gate any more: a
  file equal to the marker is installed in any case; this also covers
  `DBG:UPDATE`, after which the loader resets with SYSRESETREQ).
- Only when the `0x08008000` marker passes the ROM/loader header test (type
  0x0001, complement, CRC16 of word1‖word2; `boot::marker_valid`); otherwise
  nothing is renamed (`marker = None`).
- Never in recovery.

fatren runs once per normal boot, before USB (which `main()` brings up
first), after:
- the shim has decided normal;
- the clock is up;
- the SPI chip is identified;
- the BPB has passed its checks.

**Checks** (any failure → volume untouched, status `BADBPB`):
- jump byte and the 0x55AA signature;
- 4096 bytes/sector, equal to the device block;
- sectors/cluster a power of 2;
- 1–2 FATs;
- the root a whole number of sectors;
- total sectors ≤ device;
- FAT16 by cluster count (4085..65524), with the FAT covering every cluster.

**Steps.** The order means an interruption leaves at worst lost clusters,
never two live entries with the same name or a live entry on freed clusters.
1. **Scan** the root, read-only, in directory order.
   - A match is an 8.3 `DM40*` + `BIN`, or a long name `dm40*.bin`,
     case-insensitive.
   - Labels and directories are skipped.
   - A file's LFN entries are the checksum-matching run just before its 8.3
     entry.
   - `DM40RCVR.BIN` is handled separately.
2. **Leftover `DM40RCVR.BIN`:** mark it 0xE5. Its cluster chain is freed if it
   has one; the 0-byte trigger has none.
3. **Old backup:** if `_DM40PRV.BIN` exists, mark its 8.3 + LFN entries 0xE5
   and free its cluster chain to 0x0000 in **every** FAT copy. Only one backup
   is kept.
4. **Rename** the first match **whose header equals the marker** to
   `_DM40PRV` `BIN`, with the case flags cleared, and mark its LFN entries
   0xE5. No new LFN entries are written. Every other match is left alone:
   `staged` = header differs from the marker, `others` = all matches left
   untouched. (Step 3 only runs when there is such a file to rename.)
5. **Write and verify.** Every changed 4 KiB block goes through `spiflash.rs`
   `write_block`:
   - unchanged blocks are skipped;
   - a block that only clears bits is programmed without an erase;
   - any other block is erased (4 KiB) and programmed;
   - the write is read back.
   
   fatren then reads the block again and compares it. A final re-scan must find
   one match fewer, with the backup in place of the renamed entry. The status
   is `RENAMED`, otherwise `ERROR`.
6. **Restore:** remove the leading underscore on the disk.

`fatren-dry-run` only reports.

**Host test:** [`tools/fatren-test/run.sh`](../tools/fatren-test/run.sh). It uses mkfs.fat images in
the DM40 geometry: 4096 B sectors, 2 FATs, 512 root entries, 4087 clusters.
- **Rename** (update files are real packed images, `dm40fw.py pack`; the
  marker passed to the test is the decoded header of the installed one):
  - dry run writes nothing;
  - with an old backup, an LFN `dm40_v_2_6.bin` (installed), `DM40FW.BIN`
    (staged), a directory `dm40dir.bin` and other files present: the
    installed file is renamed, the old backup is deleted, and `DM40FW.BIN` is
    left (`staged=1`, `others=1`);
  - fsck.fat is clean and contents are unchanged;
  - the same marker again: `CLEAN`, nothing written; no marker: nothing
    renamed; the marker of `DM40FW.BIN`: it is renamed, replacing the backup;
    then `CLEAN`, nothing written;
  - H1 scenario: a staged stock file with opendm40 installed is not renamed and
    the backup survives; once the marker is that file's header it is renamed;
  - a long name with leading spaces is handled.
- **Recovery trigger:**
  - it reuses a 0xE5 slot;
  - only root block 5 is written, and the FAT is byte-identical;
  - the file is 0 bytes, and fsck.fat is clean;
  - a second call returns `AlreadyThere`;
  - nothing is written when a `dm40*.bin` is already staged;
  - a following normal run removes `DM40RCVR.BIN` and renames the image.
- **Not exercised:** a long name that matches while its 8.3 name does not.
  mtools derives the 8.3 name from the long one, so this case cannot be built.

## Unit data: the USB volume (`src/vdisk.rs`), the SPI disk (`src/fatdir.rs`) and the calibration safety copy (`src/calbackup.rs`)

The SPI flash is **internal storage only**; USB exports **one
volume generated on the fly** (DAPLink-style virtual FAT; writable since
2026-10-22 only for deletes and a firmware file, "Host writes" below): the
unit's irreplaceable data ([user-feedback.md](user-feedback.md) UF-20), the recordings / screenshots / scope
records converted to PC formats, and a raw image of the SPI flash. No host
can write the SPI disk or keep a stale copy of its FAT any more, so the
"Disk in use by PC - eject first" refusals are gone (screenshot, scope
record, recording, gallery / MMEM delete), and the on-device writers never
wait for an eject. (It replaced a two-LUN layout that exported the SPI disk.)

The **calibration safety copy** stays: ONE write-once copy of `CALIB.BIN`,
`BOOTDATA.BIN` and `UID.BIN` in the SPI disk's root (below).

Earlier builds (up to 31adbb5) wrote nine files to the SPI disk at every
boot; **files those builds left are not touched** (they stay in the SPI root
and show only in `DEBUG/SPIFLASH.BIN`). fatren still renames an installed
`dm40*.bin` to `_DM40PRV.BIN` (loader re-install protection).

### The SPI disk (internal storage, `src/fatdir.rs`)

FAT16 as the stock firmware made it (the vendor FS-loader needs `dm40*.bin`
in its root for updates and recovery). Root: update files, `_DM40PRV.BIN`,
the safety copy. Our files live in subdirectories:

| Directory | Files | Writer |
|---|---|---|
| `LOGS/` | `LOGnnn.DLG` DMM recordings ([compact-log-format.md](compact-log-format.md)) | csvlog.rs |
| `SHOTS/` | `Snnn.BMP` screenshots | screenshot.rs |
| `SCOPE/` | `SCOPnnn.DLG` scope records | screenshot.rs `start_csv` |

`fatdir::boot` (native `pre_main`, after calbackup) creates the three with
NEED = 1002 slots (8 clusters of 4 KiB: 1000 numbered files + `.` + `..`, all
zeroed), so a writer never grows one; a writer that finds its directory
missing creates it itself (`fatdir::ensure`, ~1 s once), a shorter one (made
by a PC on the loader's disk, mtools' one cluster) is grown once. Order:
create = clusters zeroed (`.` / `..` in the first block) → chain in every
FAT copy → root entry (attribute 0x10, size 0); grow = new clusters zeroed →
their chain → the old last cluster linked. A power cut anywhere = at worst
lost clusters (host-tested with a cut before every block write).
`DBG:BOOT?` reports fatdir's ok / created / failed masks (bit i = LOGS,
SHOTS, SCOPE).

**Files of ours from earlier builds stay in the root** (no boot-time move:
a move is two directory writes with a window in which either two live
entries share a chain or the file is lost). Every reader lists the root
too: the writers pick the lowest number free in the root AND the directory
(names never clash); the gallery shows root + LOGS + SHOTS; csvlog's list /
delete / `MMEM:CAT?` and dlog's `MMEM:DATA?` look in the file's directory
(fatdir::dir_of: the name decides it), then the root; the USB volume shows
both in its `LOGS/` / `SHOTS/` / `SCOPE/`.

### The USB volume (`src/vdisk.rs`)

One LUN (`Msc::new(.., 1)`, feature `vdisk`, default; builds
without it still export the writable SPI disk; recovery-usb keeps its own).
INQUIRY product `DM40 files`. **Read-only by default** ("Files live",
[decisions-2026-10-22](../../docs/history/decisions-2026-10-22.md) #6): the WP bit set, every WRITE fails DATA PROTECT
07/27/00, a change goes out as a UNIT ATTENTION at once. Settings 3/3
"USB disk" = "PC can edit" (or `DBG:VDISK:RO 0`) asks for the writable
volume: volatile (read-only again at every boot) and in force from the next
USB bus reset (a re-plug; a host reads the WP bit when it mounts, so the
mode never changes under a host); then a WRITE is judged sector by sector
(`src/vwrite.rs`, "Host writes" below), anything not accepted fails DATA
PROTECT 07/27/00. Volume label `DM40`, serial = CRC-32 of the UID. Geometry: 512-byte
sectors, 64 per cluster (32 KiB), 1 reserved sector, 2 FATs of 254 sectors,
512 root entries, 65000 clusters (FAT16), 4160541 sectors = 1.98 GiB (room
for SPIFLASH.BIN, the CSV expansion of a full SPI disk of logs, ~10-20x, and
the append-only allocation below).

| Path | Content |
|---|---|
| `README.TXT` | What is where; the two modes (Files live: new files while mounted, refresh; PC can edit + re-plug: delete / update from the PC); delete on the meter / MMEM:DEL; firmware update (copy dm40*.bin here in PC can edit, reload.sh / DBG:UPLOAD in any mode, or the loader's own disk). |
| `INFO/CALIB.BIN` … `LOADER.BIN` | Raw MCU regions, as the old LUN 1 (table below). |
| `INFO/BACKUP.TXT` | Addresses, sizes, CRC-32s (zlib), UID, firmware version, loader record, restore notes. |
| `INFO/FIRMWARE.BIN` | The running firmware as its vendor update file (`calbackup::FwFile`); only with a valid loader record and no hazardous last block. Reinstall: `tools/reload.sh --image FIRMWARE.BIN`, or copy it as `dm40fw.bin` to the loader's own disk. |
| `DEBUG/SPIFLASH.BIN` | The raw 16 MiB SPI flash, read on access (`fsck.fat -n` / mtools on it show the internal FAT16). A host may keep it in its page cache: copy it right after mounting for a current image. |
| `LOGS/LOGnnn.CSV` | The CSV generated from each `LOGnnn.DLG` (dlog.rs `Gen`; the exact text of the old CSV writer). |
| `SCOPE/SCOPnnn.CSV` | From `SCOPnnn.DLG`. |
| `SHOTS/Snnn.BMP` | The screenshot, streamed from its SPI file (FAT chain cursor). |

INFO sources:

| File | Source | Bytes |
|---|---|---|
| `CALIB.BIN` | `0x080C0000` | 4096 |
| `BOOTDATA.BIN` | `0x080A4000` | 16384 |
| `UID.BIN` | `0x1FFF7A10` | 12 |
| `OPTBYTES.BIN` | `0x1FFFC000` | 16 |
| `OTP.BIN` | `0x1FFF7800` | 528 |
| `ROMBOOT.BIN` | `0x08000000` | 16384 |
| `LOADER.BIN` | `0x080A0000` | 131072 |

Layout: clusters 2..6 = `INFO`, `DEBUG`, `LOGS`, `SCOPE`, `SHOTS` (one
cluster each: 1024 slots), 7 = `README.TXT`, then the INFO files, then
`SPIFLASH.BIN` (512 clusters), then the dynamic area. The INFO CRC-32s are
computed once before USB attaches (`vdisk::init`, `native::before_usb`); the first table of LOGS / SCOPE
/ SHOTS too (`vdisk::boot_refresh`: a DLG's CSV size costs ~10 ms per 100 KB
of logs; no media change for it).

**The dynamic files** (`TAB`, MAX_VF = 400 entries of 16 bytes in SRAM:
kind + number, directory slot, first cluster here, SPI start cluster + size,
size here). Numbered names only (`LOGnnn.DLG`, `SCOPnnn.DLG`, `Snnn.BMP`,
from the root or the directories); the file being recorded is left out
until it ends. `vdisk::refresh` (main loop: `dlog::stale()` after a
recording ended / a shot or scope record / a delete / an upload, run once
`dlog::gen()` has not moved for 1 s and no writer holds the disk) scans the
SPI directories and:

- keeps every file that is still there with the same SPI start + size **at
  the same directory slot and the same clusters** (a host's cached
  directory, FAT and inode stay right for it);
- appends a new file at the next slot of its directory and clusters from the
  high-water mark (a freed slot reads as a deleted entry, 0xE5; freed
  clusters stay unused);
- drops a deleted / changed file; `vdisk::forget` (csvlog `delete_on`) does
  that at once, so a new file of the same name never inherits its place;
- compacts (renumbers every file from the start) only when a file did not
  fit (slots or clusters exhausted) or after a USB bus reset (re-plug), and
  only while no host holds the medium locked (PREVENT = mounted on Linux);
  otherwise the new file waits for the Allow (unmount: `vdisk::on_unlock`).

Any change raises a one-shot UNIT ATTENTION 06/28/00, sent at once, also
while a host holds the volume **read-only** (the default, "Files live": the
2026-10-21 experiment: Linux then re-reads the mounted read-only volume and
shows new files at once, a file manager after F5; emulator + kernel attach
2026-10-22: a `LOG:CSV ON` / `OFF` while mounted, `LOGS/LOG000.CSV` listed
by `ls` within 2 s). With the writable volume ("PC can edit") no change is
made while a host has it mounted (`vwrite::frozen`, "Host writes"): the
change and its UNIT ATTENTION wait for the eject / re-plug. Emulator (read-only, 2026-10-21): a
screenshot taken while a PREVENT was active gave the UNIT ATTENTION 4 s
later, the new `SHOTS/S003.BMP` appended at a new slot and cluster, the
other files unchanged.

Sector generation (`VDisk::read_block`): boot sector from constants; FAT
entries from the fixed layout and, past it, a bisection of `TAB` (sorted by
first cluster: allocation is append-only, a compaction keeps the order); a
subdirectory sector = `.` / `..` + the files of that directory by slot; data
→ the owning file (MCU regions by byte loads, `BACKUP.TXT` / `README.TXT`
through a windowed text writer, `FIRMWARE.BIN` encoded per 128-byte block,
SPIFLASH by raw SPI reads, a CSV through the cached `dlog::Gen`, a BMP
through a cached FAT chain cursor). `DBG:VDISK:STAT?` → `<files>,<next free
cluster>,<files left out>`; `DBG:VDISK:RD?` → `<read_block calls>,<their
total ms>,<max us>,<CSV generator opens>,<SPI flash reads>,<their bytes>,<kind-3
seeks not from the cursor>,<chunk headers read>,<rows decoded>`
(cumulative since boot; diff two readings around a copy); `DBG:VDISK:EJECT n` = "no medium" for n commands,
then a UNIT ATTENTION (the earlier ADD / FORCE experiment hooks are gone).

### SCSI (`src/msc.rs`)

Per LUN as before (INQUIRY name from the device, removable; READ CAPACITY
/ FORMAT CAPACITIES; MODE SENSE WP; WRITE to a read-only device takes its
data phase and fails 07/27/00; the generated volume answers each block
itself (`gates_writes`: no global write inhibit; a refused block fails the
command with the device's sense, 07/27/00 refused / 02/04/01 busy, retried
by the host / 03/0C/00, after the rest of the data phase is drained);
sense per LUN; PREVENT / ALLOW tracked per
LUN, `msc::host_mounted()` = LUN 0's lock). UNIT ATTENTION: a device with
`media_change_immediate` (the generated volume) gets it at the next command
(not INQUIRY / REQUEST SENSE, never mid-transfer) whatever the lock; the
writable SPI disk of the builds without `vdisk` keeps the old rule (held
back while locked). A bus reset or unconfigure clears the locks and asks the
volume for a compaction at its next refresh. `MAX_BLOCK` = 512 in the native
build (the volume's sector), 4096 where the SPI disk is exported (its
buffer `MSC_BLK` then in SRAM `.sram_bss`, not TCM: the 4 KiB cost the TCM
stack its 42 KiB). INQUIRY with EVPD: page 0x00 (supported: 00, 80) and
0x80 (unit serial number = the USB serial, below); any other page fails
ILLEGAL REQUEST, INVALID FIELD IN CDB (05/24/00). Until 2026-10-23 an EVPD
request got the standard INQUIRY data. Linux usb-storage never asks for VPD
pages (`skip_vpd_pages`); Windows and macOS do.

**Eject.** START STOP UNIT with LoEj = 1, Start = 0 (Linux `eject` / `gio
mount -e` / a file manager's Eject; Windows Explorer's Eject and "Safely
remove") ejects the LUN: every command but INQUIRY, REQUEST SENSE, START
STOP UNIT and PREVENT ALLOW then fails NOT READY, MEDIUM NOT PRESENT
(02/3A/00) until a bus reset / unconfigure (a re-plug) or a host load (LoEj
= 1, Start = 1, e.g. `eject -t`), which raises the one-shot UNIT ATTENTION
06/28/00. Before this (up to c92b605) the medium stayed present and Windows
mounted E: again at the next access. A plain STOP / START (LoEj = 0: Linux
sd around a suspend, `udisksctl power-off` before it removes the USB
device) and a power-condition request change nothing. Pending media-change
UNIT ATTENTIONs are kept while ejected. The CDB byte is read from the CBW
in the transport buffer as for PREVENT (usbd-storage hides it); a CBW not
found exactly once is ignored. Any 0x1B still sets `STOP_UNIT` (recovery-usb
powers off on it). "PC can edit": the session ends at the Allow (Linux sends
it before the eject) or after the quiet time (Windows); the firmware file
is committed by `vwrite::poll` once its data has settled, whether the
medium is ejected or not, and an Allow that comes before that check (`cp`
+ eject at once: the install used to be lost, also before the eject
change) waits for it, at most 1.5 s (`END_WAIT_MS`, frozen meanwhile). `DBG:VDISK:EJECT?` →
`<LUN 0 ejected>,<START STOP UNITs since boot>,<CB[4] of the last, 256 =
ignored>`. Tests: emu/mscresettest.py `stop-start`, `eject-load`,
`eject-port` (raw SCSI over USB/IP), emu/ejecttest.py (the kernel's
usb-storage + udisks: eject, power-off, a "PC can edit" install ejected at
once, re-plug).

### USB serial number (`src/uid.rs`)

Every unit used to report the USB serial `0001` (and `0` in `*IDN?`).
udisks merges disks with the same vendor / model / serial into one
(multipath) drive, so two DM40s, or the emulator next to a real meter, on
one Linux PC shared a udisks drive, and a drive-level Eject or Power Off
could act on the other unit. Since 2026-10-23 the serial is the GD32's
96-bit unique ID (`0x1FFF7A10`, 12 bytes) as 24 uppercase hex digits, bytes
in memory order (word 0 low byte first): the digits of `xxd -p -u
INFO/UID.BIN`. The same string is:

- the USB iSerial (normal mode and `recovery-usb`);
- the SCSI unit serial number, INQUIRY VPD page 0x80 (msc.rs);
- the third field of `*IDN?`: `Alientek,DM40A,<serial>,opendm40 0.1.0`.
  libsigrok's scpi-dmm DM40 profile (`sr_scpi_get_hw_id`: four
  comma-separated fields, the serial is only stored) shows it as
  `[S/N: ...]`; ngscopeclient (`SCPIDevice`: `%127[^,]`) as the serial.
  Neither compares it. `SYST:IDN:EMUL 34401A` keeps the HP identity with
  serial `0`, as before.

The UID is in system memory, not in the slow flash bank (`0x08080000..`),
and the serial paths do not read it at request time: `uid::init` (region C,
`.cold`) reads it once in `native::start`, before the recovery / normal
decision is acted on, into a 24-byte TCM static; the USB descriptors, the
VPD page and `*IDN?` use that copy. (INFO/UID.BIN, `vdisk::mem_read`, still
reads 0x1FFF7A10 directly, as before.) The VID:PID (1209:0001) is unchanged, so udev rules keyed on it
still match; `/dev/disk/by-id/usb-ALIENTEK_DM40_files_...` names and the
Windows device instance path change once (a new device instance: Windows
installs it again, a WinUSB binding made with Zadig must be made again).

Privacy. The serial identifies the unit (as most USB disks' serials do):
any USB host reads it without mounting (`lsusb -v`), any Bluetooth peer
that may send SCPI reads it with `*IDN?`. It was already readable over USB:
`INFO/UID.BIN` on the volume and the FAT volume ID (CRC-32 of the UID). A
hash of the UID would be just as stable a tracking identifier, so it would
not hide more; the only thing the raw UID adds is the input of the stock
genuine check (`stock/re/docs/genuine-check.md`), whose stored block is on the volume
too (`INFO/BOOTDATA.BIN`). There is no setting to hide it.

Emulator: `emu/run.py --uid` (default `emu_uid`: this unit's UID with bytes
4 and 9 = the USB/IP port, so every instance has its own serial and the
stock genuine check, which never reads those two bytes, still passes).
`emu/ejecttest.py` no longer rewrites the serial in its proxy; its safety
check requires the emulator's udisks drive to carry the emulator's serial,
not the unit's, and no other disk on it. Test: emu/mscresettest.py `serial`
(USB iSerial, VPD 0x80 / 0x00 / a missing page, `*IDN?`).

### The slow flash bank while an AD2 stream runs (2026-10-23)

Internal flash from `0x08080000` to the end of the 1 MiB is not zero-wait
(region C, the FS-loader, its UID data, the settings / calibration record).
One CPU load from it stalls the bus ~21.8 us, and the stream ISR's entry
with it, whether the load is an instruction fetch (region C code, the old
rule) or a DATA read. Device c92b605 (device-check-2026-10-21.md "c92b605"
item 3): a host copying INFO/CALIB.BIN, BOOTDATA.BIN, LOADER.BIN or the
FIRMWARE.BIN tail during a disk session lost 10 k samples at 38400 S/s and
289 k at 76800 (ROMBOOT.BIN and the FIRMWARE.BIN head, zero-wait: 280 ns).

Rule: **nothing reads the slow bank while `hy3131::streaming()`** (the ISR
owns the HY3131 bus: a fast-log session, the Wave view, the inrush page,
the AD1 stream ISR).

- **USB volume, the mirror** (`vdisk.rs` `mirror_boot` / `mir_read`,
  2026-10-24): every INFO/ byte from the slow bank lies in one range,
  `0x08080000..0x080C1000` (the FIRMWARE.BIN tail, LOADER.BIN with
  BOOTDATA.BIN inside it, CALIB.BIN), 260 KiB. It is kept as the raw file
  `_INFOMIR.BIN` in the SPI disk's root, and while `streaming()` the volume
  reads those bytes from there (`mem_src`; FIRMWARE.BIN through
  `FwFile::fill_with` + `fw_body`, which splits the one 128-byte block that
  straddles 0x08080000 between the flash and the mirror). So a copy of any
  INFO/ file runs at full speed during a session or in the Wave view, with
  no wait and no host timeout. Outside a stream the volume reads the flash
  as before.
  - Consistency: at every boot (`native.rs pre_main`, before USB, no
    stream) `calbackup::write_file` compares the file with the flash byte
    for byte; equal = nothing written (PRESENT), different or missing = a
    new chain written, read back, the entry switched, the old chain freed
    (WRITTEN, copy-on-write: a cut leaves the old file or lost clusters).
    The mirror is served only after this boot's PRESENT / WRITTEN (and its
    chain resolved, `calbackup::chain_of`): a stale file left by a failed
    rewrite is never used. Nothing else writes those clusters (on-device
    deletes take numbered names only; the host staging and the fast logger
    use free clusters). So it is the running image and calibration, byte
    for byte; a new image or a calibration changed by the stock firmware
    is picked up at the next boot.
  - Cost: 260 KiB = 65 clusters of the 16 MiB SPI disk (1.6 %; 130 while a
    rewrite holds both chains). Wear: only a changed mirror is written (65
    erases + 2 FAT and 1 root block per new image); an unchanged boot
    reads it (65 blocks). RAM: 144 bytes SRAM (the cluster list); region
    B ~0.5 KB (`mir_read`, `fw_body`, `body_copy`). Boot time: DBG:VDISK:
    MIR? field 4 (emulator: PRESENT 219 ms every boot; WRITTEN ~1.0 s, 4.3 s
    with the W25Q128 model's typ erase / program times; device: retest).
  - DBG:VDISK:MIR? = served (1/0), the writer's status (0 NOTRUN 1
    NOMEDIUM 2 BADBPB 3 PRESENT 4 WRITTEN 5 NOSPACE 6 ERROR), rewritten at
    this boot (1/0), ms.
  - MMEM:CAT? (csvlog `catalog_scpi`, its cache): lists gallery files only
    (S*.BMP, LOG*.DLG, SCOP*.DLG), so never `_INFOMIR.BIN`; its <used>
    count includes the mirror's 260 KiB. The cache keys on
    `msc::FLASH_WRITES`; the mirror is written only at boot, before any
    catalog, and its reads during a stream write nothing, so the cache
    stays valid while a host copies INFO/.
  - Privacy: CALIB / BOOTDATA bytes were already on the SPI disk (the
    write-once safety copy, calbackup.rs) and leave the meter only through
    the USB volume as before (INFO/, DEBUG/SPIFLASH.BIN). Nothing logs them.
  - Pacing (2026-10-24, branch mirror-bw; device c785807 M2: a 76800 disk
    session lost ~21 700 samples per host copy of LOADER.BIN). A mirror
    sector costs the main loop its whole SPI read (512 polled bytes, ~0.97
    us each, plus a wait for a page program in the chip): ~1.1 ms of wall
    time with the stream ISR at ~54 %, taken from the fast logger's coder.
    So while `streaming()` a mirror sector is read only when the pass grants
    it (`vdisk::mirror_pass`, main.rs once per pass before the MSC poll, the
    level from `fastlog::mirror_level`; `mir_hold` via msc.rs
    `BlockDevice::read_wait`): at most one per pass, with the chip idle
    (one RDSR, `spiflash::chip_busy`), and only when the logger has caught
    up (ring <= 384 records, FIFO <= 1 KiB at the pass's start). Behind:
    after 10 ms of waiting; pressed (ring over half full, the writer
    trial): after 50 ms. Otherwise the READ waits in its data phase (the
    host's IN tokens NAK; no NOT READY, no retry, no 180 s exposure); a
    240-sector READ needs <= 12 s against its 30 s timeout even at the
    50 ms bound. Without a fast-log session (Wave view, inrush, AD1 / host
    sessions) the level is always free: one sector per pass. Memory-served
    sectors are not paced (they cost ~nothing; emulator: 7.7 MB of
    ROMBOOT.BIN during a 76800 session, lost 0). Region B
    (`.text.vdisk_rd`), checked with emu/regionc.py. DBG:VDISK:MIR? fields
    5-7: sectors read from the mirror while streaming, holds (one per poll),
    the longest hold (ms). The fast logger's own CPU share: LOG:DISK?
    field 14.
  - A failed SPI read of the mirror (`mir_read`) switches it off for the
    rest of the boot; msc.rs then answers that READ NOT READY (read_busy)
    instead of MEDIUM ERROR, and the fallback below applies.
- **USB volume, fallback without a mirror** (no SPI disk, NOSPACE, ERROR;
  `vdisk.rs` `read_busy`, `msc.rs`): a READ(10) touching a
  sector served from the slow bank fails CHECK CONDITION, NOT READY,
  LOGICAL UNIT IS IN PROCESS OF BECOMING READY (02/04/01) before its data
  phase (the CSW goes out at once; Linux takes it as "device skipped data
  phase"); a transfer in progress fails at the first such block. The
  sectors: CALIB.BIN, BOOTDATA.BIN and LOADER.BIN whole, FIRMWARE.BIN from
  the sector holding file offset 12 + 0x7C000 (body 0x08080000; with the
  12-byte header and the 128-byte encoding blocks: file offset 0x7C000).
  Every other sector (directories, README, BACKUP.TXT, ROMBOOT, UID, OTP,
  OPTBYTES, the FIRMWARE.BIN head, logs, shots) reads as before. The
  answer is per command: a READ(10) is refused whole when any of its
  sectors is slow, so a host request (readahead, a 120 KB bio) that spans
  FIRMWARE.BIN offset 0x7C000 waits too.
- **The settings / calibration record** (`dmm.rs` `load_record` /
  `rec_u32`): read once at start-up into a 1 KiB SRAM copy; `Dmm::ad2_scale`
  (every Wave view frame, LOG:SCALe?), `dmm::cal`, the scope's gain /
  offset pairs and hwtrig's trigger map read the copy. opendm40 never writes
  the record.
- **Boot-data version bytes** (0x080A4010..12, BT command 02, Device page):
  copied by `Bt::new` (`bt::boot_ver`).
- `Logger::start` / `logger::isr::start` are region B again (their tails
  ran after `stream::start`), and so is LOG:SCALe? (`main.rs log_scale`,
  asked while a session streams).

Why NOT READY (decision 2026-10-23, of three):
- (a) NOT READY 02/04/01, the host retries: no RAM, every file, exact (only
  the slow sectors), the precedent is the busy write (`write_error` 2).
  Linux (drivers/scsi, v7.0): `scsi_check_sense` retries 04/01 at once
  (NEEDS_RETRY), then `scsi_io_completion_action` ACTION_DELAYED_RETRY
  (requeue, device busy, ~3 ms) until `scsi_cmd_runtime_exceeced`:
  (retries + 1) x timeout from the request = (5 + 1) x 30 s = **180 s**
  (`/sys/block/sdX/device/timeout` scales it). Chosen.
- (b) a RAM cache filled before the stream: CALIB 4 KiB + BOOTDATA 16 KiB
  would fit SRAM (52 KiB free) but LOADER (128 KiB) and the FIRMWARE.BIN
  tail (~104 KiB) cannot, and the kernel's 240 KB readahead pulls the tail
  in on any FIRMWARE.BIN read: a partial cache leaves the usual copy
  stalling. Rejected.
- (c) hold the reply until the stream stops: a SCSI command timeout (Linux
  30 s, then abort + BOT reset) against sessions of minutes or an open-ended
  Wave view. Rejected.
- (d, 2026-10-24) the SPI mirror above: answers at once for any stream
  length, so (a) is now only the fallback. Not taken: a DMA copy of the
  slow pages during the stream (its loads go through the same flash
  interface the ISR's fetches need; whether that stalls the ISR is not
  measured and the emulator cannot show it), and RAM stays too small (b).

Tested over the real Linux path (emu/kvmtest/run.sh: a KVM guest with the
Ubuntu 7.0.0-38 kernel attaching the emulator over vhci-hcd, usb-storage,
sd, vfat; the NOT READY change, then the final image b727863):
- 6 s session (52-67 s of wall time in the loaded emulator): `cat` of the
  four files waited until the session ended, exit 0, byte-identical to
  copies made before, no kernel message (b727863: 51.7 s).
- 20-25 s sessions (> 180 s wall): the kernel logs "timing out command,
  waited 180s", "Not Ready / Logical unit is in process of becoming ready",
  "I/O error, dev sda, sector 925" (CALIB.BIN); an O_DIRECT read (`dd
  iflag=direct`) fails EIO; a buffered `cat` still completed with the
  right bytes (the page cache re-reads a failed readahead with a new
  request, a new 180 s). So: a copy made while the meter streams for more
  than ~3 minutes (an open Wave view, a long fast-log session) can fail;
  close the view / end the session and copy again.
- Windows (classpnp) is expected to retry 02/04/01 too; how long it keeps
  trying is unknown (device retest).

Pacing (branch mirror-bw, emulator --mips 110 --isr-stall-ns 700 with the
device's polled SPI byte, 24547cf): a host reading LOADER.BIN back to back
from 3 s into a 76800 run lost 119 853 of 768 000 samples on 9cfa6a6, 0
with the pacing (rung kept; a copy inside the writer trial no longer
makes it fall back); every INFO/ file + DBG:PROF? polls at 38400 / 76800
`--region-c`: lost 0, no region C, no NOT READY. Device: retest (TODO
"Device retest of mirror-bw").

Tested with the mirror (2026-10-24, 12a1d9c, emulator): fastlogtest
`--region-c --host-read --dur 20` at 38400 / 76800 S/s (typ chip timing):
every slow-bank sector of the four files read during the session (37 / 24
MB of INFO/ READs in all), no NOT READY, the same bytes as after the
session, no region C call or slow-bank load, lost 0 at both rates (76800
kept its rung). `emu/mirrortest.py`: fresh disk WRITTEN; the same image
PRESENT with the saved SPI image unchanged; another image WRITTEN; one
byte of the calibration sector changed WRITTEN, then PRESENT; each time
`_INFOMIR.BIN` equals flash 0x08080000..0x080C1000. kvmtest (Linux
usb-storage): `cat` of the four files and an O_DIRECT CALIB.BIN during a
38400 session finish while it streams (10.7 s), byte-identical; with
`dm40wait=240` (started 240 s of wall time into the stream, past the old
180 s limit) 7.4 s, exit 0, identical, no kernel I/O error.

Emulator check: `emu/regionc.py` arms `peripherals/FlashWatch.cs` (tlib's
memory-access hook): every CPU data load from 0x08080000..0x08100000 while
`hy3131::HANDED` is set, and every slow-bank PC the stream ISR interrupts,
is reported next to the region C function hooks; fastlogtest
`--region-c --host-read` (and wavetest, which reads INFO/ in the Wave view)
fail on any. Before the fix (2f5d3b8): memcpy from 0x08080000.. (the
FIRMWARE.BIN tail), `vdisk::mem_read` from 0x080C0000, `Logger::start`'s
literal pool, `dmm::cal` (Wave view), `scope::cal_pair` /
`hwtrig::rec_pair` (`--poll "SCOP:MEAS:VPP?;DBG:HWTRIG?"` in a disk
session), `main.rs log_scale` (LOG:SCALe?, region C code). On b727863:
fastlogtest `--region-c --host-read` at 640 (AD1) / 4800 / 38400 / 76800,
the same at 9600 with `--poll` of SCOP:MEAS:VPP? / DBG:HWTRIG? / WAV:PRE?
/ CHAN1:SCAL? / SYST:MOD? / *IDN? / LOG:SCALe? / LOG:SRATe? / LOG:SOURce? /
CONF?, wavetest (INFO/ in the Wave view, the polls with LOG:DEST? /
LOG:DURation? / *IDN? / SYST:MOD?), ad2streamtest, inrushtest: PASS, no
region C call and no slow-bank load while streaming (the Save CSV tap's own
handler, allowed as before, aside).

UI and host paths added 2026-10-23 (3f32bc3, emulator): `fastlogtest
--ui-nav` walks the menu, Settings 1..3, Device and the MCX scope page by
DBG:KEY / DBG:TAP while a disk session streams (real touch stops a session
at the first press; this audits the page code) and starts each session on
the Scope page: no region C / slow-bank read after `ui::main_menu_hit` moved
to B and `press_track` is skipped while streaming. wavetest checks MMEM:CAT?
(now from a cache, csvlog.rs `Cat`: a scan in one pass at 76800 dropped
~90 ms of the Wave view's ring), the V/div step (repaint before the restart)
and the badge exit per step.

### Host writes (`src/vwrite.rs`): delete from the PC, drag-and-drop update

The volume is writable but stores nothing a host writes as sectors: each
512-byte WRITE is compared with the sector the volume generates and accepted
only as one of the changes below, kept as small records (~1.4 KiB SRAM,
.sram_bss) from which the volume regenerates **exactly what the host
wrote** (a host that re-reads a sector after evicting it from its cache
sees its own data); anything else fails DATA PROTECT 07/27/00.

| Write | Accepted as |
|---|---|
| boot sector byte 0x25 | Linux's dirty flag on a rw mount (written at mount and umount) |
| FAT[1] bits 15 / 14 | the Windows / macOS clean / error flags |
| dates / times of an existing entry | access date (relatime), the parent's mtime: accepted and dropped (a re-read shows the old date) |
| a LOGS / SCOPE / SHOTS entry → 0xE5 (rest unchanged) | `T_DIR` on its table entry: shown 0xE5 + the rest |
| its whole chain → 0 in a FAT sector | `T_FAT`: shown free, data zeros |
| FAT entries of free clusters | the host's allocations (`pair`, 40) |
| free root slots: 0xE5 / long-name entries; an 8.3 `DM40*.BIN` file (≤ upload::MAX); a directory / file of the hosts' own (`.Trash*`, `._*`, `.DS_Store`, `.fseventsd`, `.Spotlight-V100`, `.TemporaryItems`, `.metadata_never_index*`, `.com.apple.*`, `System Volume Information`, `$RECYCLE.BIN`, by long name) | raw 32-byte records (10) |
| data in free clusters | staged in free SPI clusters (`stage`, 28 volume clusters) |

Refused: writing into README.TXT, INFO/, DEBUG/ or our files, an INFO
delete, entries created in our directories, a rename, a truncation, other
names / directories in the root, the volume label, past the capacities.
On Linux a refused write-back shows as one `Buffer I/O error ... lost async
page write` (the mount stays rw; the buffer is re-read later); the host's
in-memory view of that item stays inconsistent until the re-plug (a copied
`notes.txt` seems to be there until then). That is why the hosts' own
housekeeping is **accepted** for the session: when the trash directory's
root entry was refused (first capture), vfat kept the directory's inode
keyed by the slot position and a later `cp ... dm40fw.bin` into that slot
failed with "Is a directory".

**Delete.** When both marks of a file are seen (any order; Linux `rm`:
FAT 1, FAT 2, then the directory sector, at once), the main loop
(`vwrite::poll`, under the gallery lock) deletes the SPI file
(`csvlog::delete_on`) if its SPI entry is still the same file (start +
size); a delete on the meter meanwhile (`vdisk::forget` while frozen: the
entry stays, data zeros) is never repeated on a new file of the same name.
The 0xE5 alone is a move (Nautilus / gio "Move to Trash": the file's start
+ size written as an entry into a host directory, `T_MOVED`, noted when
that data is staged or when the 0xE5 comes later): the file is deleted at
the session's end if the trash still holds it; emptying the trash frees the
chain = a whole delete at once; a file taken out of the trash again (its
new entry in our directory is refused) stays. A refused rename keeps the
file (it is back after the re-plug).

**Firmware file.** Data in free clusters is staged whatever the order
(Linux writes the data, FAT and directory in separate write-backs; the
entry may come first with size 0): per volume cluster up to 8 SPI clusters
(first fit, SPI FAT untouched: the staging owns the SPI disk like
DBG:UPLOAD, `upload::busy`; screenshot / csvlog commits wait). A root
record `DM40*.BIN` whose chain is complete in `pair` with every sector up to
its size staged, quiet for 300 ms, is decoded (128-byte blocks as the
loader) and checked: the loader's header test, word1 = size - 12, the body
CRC-16 (word2, the 12 marker bytes excluded), no hazardous short last block.
Then `upload::commit_on`: every other dm40*.bin in the SPI root deleted,
the staged clusters linked as `DM40FW.BIN` (data → FAT → directory). The
meter asks **"Firmware received / Install it now?"** (dialog.rs; ui-render
`t04-confirm-fw-install`): Yes = `DBG:UPDATE` (zero the loader marker,
reset; the loader installs it), No, a side key or 60 s = `DM40FW.BIN`
deleted. A complete `DM40*.BIN` that is no update file: toast "Not a DM40
update file", nothing on the SPI disk. An overwrite (`cp` again) copies the
committed blocks first (copy-on-write) and commits again. The host's view
of its file stays exact (its reads come from the staged / committed
clusters; after No, zeros).

**Session.** While the host has the volume locked (PREVENT: Linux and
macOS mount) the layout is **frozen**: `vdisk::refresh` waits, no UNIT
ATTENTION is sent (on a removable disk Linux fails the I/O after a UNIT
ATTENTION and the dirty buffers written back later would carry the old
FAT / directories), so recordings / screenshots made meanwhile appear at
the eject / re-plug. That is the cost of the writable volume, and why it
is not the default (decisions-2026-10-22 #6: the read-only "Files live"
volume keeps the live updates, without delete / update from the PC; the
mode changes only at a bus reset, so a writable session is never cut short
by it: asked for "Files live" while a writable session runs, the volume
stays frozen and writable until the re-plug, then the session's records are
dropped as at any session end, tools/fatren-test vwrite "rw-ro"). A host that never locks: frozen while it has
written in the last 60 s. The session ends at the Allow (umount / eject)
or a bus reset: every record dropped, the flags cleared (except a delete in
progress), `refresh` regenerates the volume and raises the UNIT ATTENTION.
A DM40*.BIN whose check is still due at the Allow (its data came less than
300 ms before: `cp` + eject at once) holds the end back until `poll` has
decided it, at most 1.5 s (`END_WAIT_MS`; frozen meanwhile; fatren-test
vwrite "Allow at once"). Staged data idle for 5 s is **parked**: its SPI clusters linked as a
hidden system file `_VHOSTnn.TMP` in the SPI root (the SPI disk is free for
the on-device writers again; a later write to a parked block copies it
first; at most 100 temp files a session, then a later burst's data is
dropped: reads zeros); all `_VHOST*.TMP` are deleted at the session's end
and at the first main-loop pass after a boot (a power cut / unplug in a
session). Staging writes answer "busy" (02/04/01, the host retries) while
a screenshot / CSV commit / DBG:UPLOAD writes the SPI disk or the
constant-interval logger runs (LOG:STATe ON). Power cuts:
the staging never writes the SPI FAT; a commit / park is data → FAT →
directory, a purge directory → FAT: at worst lost clusters.

`DBG:VDISK:WR?` → `<accepted>,<refused>,<busy>,<last refused LBA>,<SPI
files deleted>,<firmware files committed>,<rejected>,<root records>,<FAT
pairs>,<staged clusters>,<frozen>,<staging>,<firmware state>`;
`DBG:VDISK:RO 0|1` = the Settings row (applies at the next bus reset) /
`RO?` → `<in force>,<asked for>` (1 = read-only; `1,1` after a boot).

Captured on Linux (kernel 7.0, GNOME automount: `rw, relatime, flush,
shortname=mixed, errors=remount-ro`; `tools/fatren-test/usbip_msc.py` = a
USB/IP mass-storage device running the volume code at host speed,
`kcapture.sh`, captures replayed by run.sh): mount = PREVENT + boot sector;
`rm` = FAT 1, FAT 2, directory sector at once; the parent's mtime / access
dates ~60 s later (root / directory sector, dates only); `gio trash` =
`.Trash-1000` (LFN + 8.3, 3 directory clusters zeroed + the trashinfo),
the 0xE5 in SHOTS and the entry in `files/`; `cp` of 500 KB = one
write-back burst at close (`flush`), FAT + root + 980 data sectors; umount =
boot sector, ALLOW, then a UNIT ATTENTION and udisks' re-probe (no
re-mount). The captures hold the volume's layout (README.TXT is padded to
a fixed 2048 bytes for that): a change of the generated CSV text or the
INFO files needs a new capture (`kcapture.sh`; the captures run writable:
`usbip_msc.py` serves "PC can edit" unless `--ro`; the README text of
decision #6 kept 2048 bytes and the replays passed unchanged). Emulator
(real firmware, kernel attach, read-only by default since decision #6:
Settings 3/3 tapped to "PC can edit", detach + attach = re-plug, `Write
Protect is off`, `RO?` `0,0`; 2026-10-22): `rm` of 2
files → deleted on the SPI disk; `cp` of a 500 KB update file (47 s at the
emulator's speed) → committed, the dialog declined at 60 s → DM40FW.BIN
gone (SPIFLASH.BIN fsck clean); `cp` again → committed, `DBG:TAP` on Yes →
reset into the loader. USBTMC queries during a staging can time out (the
main loop is busy with SPI writes; one SPI block read-modify-write per
512-byte sector).

### Firmware update without the writable disk (`src/upload.rs`)

`tools/reload.sh` / `tools/dm40dbg.py upload FILE`: `DBG:UPLOAD:BEGIN
<size>,<crc32>` (every `dm40*.bin` in the SPI root is deleted:
`fatren::delete_updates`), then `DBG:UPLOAD:DATA <offset>,<raw bytes>`
messages of 232 bytes (taken by main.rs before the SCPI parser, which
upper-cases and cuts at 64 bytes; offset checked), then `DBG:UPLOAD:END?` →
`<size>,<crc32>`: the last block written, every block re-read and the
CRC-32 compared, the first block's decoded header must be a valid update
header, then the chain in every FAT copy and the root entry `DM40FW.BIN`
(data → FAT → directory: a power cut = lost clusters, never a partial
file). The data goes to free clusters as it arrives (first fit, ascending,
FAT untouched until END); while an upload is active it owns the disk
(screenshot / csvlog commit / gallery wait), it is dropped after 30 s
without data. Then `DBG:UPDATE` as before (the loader installs the file and
deletes it). ~8 s for 480 KB in the emulator. reload.sh falls back to
staging on the writable USB disk when the running opendm40 has no
`DBG:UPLOAD` (older build). The recovery path is untouched (the loader's
update screen exposes its own USB disk).

### Host OS notes

The single removable volume avoids the two-LUN questions (one `sd` device
on Linux, one drive letter on Windows). FAT16 with 32 KiB clusters on a 2
GiB volume is the standard large-FAT16 format (Windows, macOS, Linux mount
it). Default mode "Files live" (read-only, decision 2026-10-22 #6): the
host writes nothing at all (Windows / macOS housekeeping files included),
the UNIT ATTENTION goes out at once on every change; whether Explorer and
Finder show new files then (with or without a refresh) is open (device
check; if Explorer does not refresh, try a volume serial that changes per
refresh, TODO). The notes below are about "PC can edit". Linux: captured
(above). **Windows and macOS are untested** (no capture possible here);
designed for:
- Windows: writes `System Volume Information\IndexerVolumeGuid` /
  `WPSettings.dat` on the first rw mount (accepted for the session, gone at
  the re-plug, written again next time), may set FAT[1] dirty bits
  (accepted). Explorer's Delete on a removable drive deletes at once (no
  recycle bin): both marks. Quick-removal policy = write-through, so its
  writes come in operation order. Probably no PREVENT: frozen for 60 s
  after its last write, then released with a UNIT ATTENTION (Windows then
  re-reads the volume; whether Explorer shows new files: open).
- macOS: `.fseventsd`, `.Spotlight-V100` (many files: the 28 staged
  clusters may run out, then its writes are refused), `.Trashes`, `._*`
  AppleDouble files, `.DS_Store` in the root (accepted), `.DS_Store` in
  LOGS / SHOTS / SCOPE (refused: one write error, harmless). Finder's
  "Move to Trash" = `.Trashes/501/` move (deleted at the eject, as Linux).
  `fsck_msdos` at mount only when the volume is dirty.
Untested on the device: Linux kernel attach + udisks mount of the
real meter, the UNIT ATTENTION after the eject on Windows and macOS,
Explorer / Finder behaviour.

### Calibration safety copy (SPI disk root)

**Why it still exists:** the USB volume's INFO files are generated from the
internal flash. The one recovery path for a unit whose internal flash is
unusable — an SWD / BOOT0 session with read protection level 1 — can only
lift RDP by a **mass erase of the whole internal flash**, the calibration
(`0x080C0000`) and the UID-bound loader data (`0x080A4000`) included. The
W25Q128 is a separate chip and survives that, so one copy of the calibration
must already be there before it is needed.

**What:** `CALIB.BIN` (4096 B from `0x080C0000`), `BOOTDATA.BIN` (16384 B
from `0x080A4000`), `UID.BIN` (12 B; says which unit the loader data belongs
to) — `calbackup::FILES[..SAFE]`.

**Rule: write-once.** Each file is written only when the root has **no entry
of that name**; a present one is never overwritten (even when it differs, so
a later boot cannot replace the factory copy with a corrupted or foreign
record) and never deleted. The owner deletes a file to have it made again
from the flash as it is then. Present files are compared with the flash
(boot sector, root, FAT 1, 6 data blocks read; nothing written) only to
report `differs`.

**When:** every normal boot in `pre_main` (after board init and fatren,
before USB). First boot: 6 data blocks + FAT 1 + FAT 2 + 1 root block written
(~1 s); later boots: reads only.

**Write order** (unchanged writer, `run_inner`): allocate first-fit from FAT
1 → data blocks (each read back) → chains in every FAT copy → directory
entries → re-read every written file and compare. A power loss leaves at
worst lost clusters. The volume is only touched if `fatren::parse_bpb`
accepts it; a name taken by a directory or volume label is skipped
(`conflict`); not enough free clusters / root slots → `NOSPACE`, nothing
written.

**Status** (`calbackup::result()`), in `DBG:BOOT?` (`native::boot_words`):
status `NOTRUN|NOMEDIUM|BADBPB|SAFECOPY_PRESENT|SAFECOPY_WRITTEN|NOSPACE|ERROR`
(as numbers 0..6); written / conflict / differs = bit masks (bit 0 CALIB, 1 BOOTDATA,
2 UID); `differs` = a kept copy no longer equals the flash. Then
`CALMS,<ms>` = `calbackup::boot` duration. `tools/dm40dbg.py boot` decodes it
(and recognises the old `UNCHANGED` / `WRITTEN` / `_NOFW` statuses of earlier
builds).

### Settings copy (SPI disk root, [`src/setfile.rs`](../src/setfile.rs))

The backup SRAM, the live copy of the settings ([`src/persist.rs`](../src/persist.rs)),
is not powered while the meter is off ([hw/README.md](../../hw/README.md) "Backup
domain"). `_SETTING.BIN` (8 KiB, root) holds two one-block slots, each with
the persist version, a sequence number and a CRC-32; `setfile::boot`
(`pre_main`, after `fatdir::boot`) makes it once (all 0xFF; seeded from a
valid backup SRAM record), later saves rewrite one slot's block in place (no
FAT or directory write). App::new takes the backup SRAM record unless the
newest slot's sequence is higher than `persist::seq`; without a valid
backup SRAM record the newest valid slot; else the defaults. Saves: 5 s
after the last change (main loop, `setfile::poll`), deferred while another
disk writer, an AD1 logger session or the AD2 stream runs; at every
power-off (`power_off`, after `finish_disk_writes`). `DBG:BOOT?` reports
where the settings came from, the slot and sequence, the file's state, the
last save, the saves this boot and the newest sequence.
Host test: fatren-test "setfile"; emulator: [`emu/settingstest.py`](../../emu/settingstest.py).

### Host tests (`tools/fatren-test/run.sh`, every check fatal)

Safety copy: first boot writes exactly the three files (nothing else); second
boot `PRESENT`, no block written (11 reads); a changed calibration is kept
(`differs=0x001`); a deleted `CALIB.BIN` is made again; a foreign `UID.BIN`
is kept (`differs=0x004`); a disk of an earlier build with all nine files →
nothing written or deleted, directory identical; a directory `UID.BIN` =
conflict; fatren on the same volume; nearly full disk → `NOSPACE`, nothing
written. fatdir: the three directories created (8 clusters, `.` / `..`),
nothing written on the second boot, a cut before every block write of the
first boot and of a grow (fsck at worst lost clusters / FAT 1 ahead, the
next boot completes), slots across directory clusters, numbering over root +
directory, rmdir. Writers on directories (screenshot / csvlog cut sweeps count
directory blocks as metadata). Upload: byte-identical `DM40FW.BIN`, staged
files deleted, `_DM40PRV.BIN` kept, fatren recognises it, chunk sizes 1 /
255 / 4096, wrong CRC / repeated chunk / not an update file refused, a cut
before every block write (never a partial file). USB volume (every sector
through `read_block` into a sparse dump, every non-zero sector read twice):
fsck.fat clean, label, geometry, root layout, the 7 INFO region files equal
their sources, `BACKUP.TXT` sizes and CRC-32s, read-only attribute,
`write_block` refused; `FIRMWARE.BIN` byte-identical for the native build,
stock `dm40_v_2_6.bin`, an unpadded body and a `pack --pad` image; no valid
record / a hazardous last block → none. Over a SPI image with logs, scope
records, screenshots and root files of earlier builds: every CSV == dlog.py
of its DLG, BMPs == the SPI files, `SPIFLASH.BIN` == the SPI image (fsck
clean), foreign / unnumbered files not shown, media change once per change;
then a new file (appended, everything else at the same slot / cluster), a
deleted one (slot shown deleted), a lock + bus reset (no compaction), unlock
(compacted: slots without holes), `forget` (gone at once).

Host writes (`vwrite_test.py`, run.sh "vwrite", every scenario through
`main ... vserve` = vdisk.rs + vwrite.rs over a SPI image; host operations
made by mtools on the volume dump, their sector diffs written in five
orders): deletes of LOGS / SHOTS / SCOPE and root legacy files (only those
SPI files gone, fsck clean, the volume = the host image until the Allow,
then regenerated with a UNIT ATTENTION); one mark only (nothing deleted,
back after the session); a delete on the meter while frozen; firmware
copies (`dm40fw.bin`, `DM40FW.BIN`, long names; data-first / metadata-first
/ random), Yes / No, into freed clusters (a non-contiguous chain), an
overwrite, a non-update `DM40*.BIN`; refusals (new file / directory in the
root or LOGS, README, INFO delete, rename, label: SPI files unchanged, the
volume after a bus reset = the original); trash (move: deleted at the
Allow; emptied: at once; unrelated file; taken back out); a temp file left
by a power cut deleted at boot; a lockless host; power cuts before every
SPI write of a firmware commit (81 of 1042: every commit cut + every 25th
staging write) and of a trash session + the next boot (402); the four
kernel captures replayed with the same accept / refuse answers.

Emulator (2026-10-21, `emu/usbip_client.py luns` / `mscdump` / `upload`,
`USBIP_PORT`): one LUN `DM40 files`, 4160541 × 512 B, WP, WRITE → 07/27/00,
no UNIT ATTENTION at the first command after boot; the volume dumped over
USB/IP (575 clusters, ~2 min): fsck clean, `LOGS/LOG000.CSV` (from a root
`LOG000.DLG`) == dlog.py, `SHOTS/S000..S002.BMP` == the stock root
screenshots, `DEBUG/SPIFLASH.BIN` fsck clean with LOGS / SHOTS / SCOPE and the
safety copy; `upload` of the native image: `END? -> 480512,<crc>` in 7.9 s;
a 3-finger-swipe screenshot under PREVENT: UNIT ATTENTION after 4 s, new
`SHOTS/S003.BMP` appended. (The emulated DWC2 sometimes stops answering bulk
URBs after a few dozen transfers; `mscdump` saves progress and resumes after
an emulator restart.)

## Stack depth

The stack is TCM from `__bss_end` up to `_stack_top` (`0x10010000`). Its size
and the static worst call path are checked at build time ("Memory budget"
above: `tools/stack-depth/stackdepth.py --check 20480`, a link-time assert
of >= 42 KiB). `native::normal` calls the never-inlined `pre_main` and then
`main()`, so the boot-time disk buffers (fatren ~9 KiB, calbackup, fatdir,
setfile, vdisk) are popped before `main()`'s frame is pushed (review B1).

**Run-time check:** `native::init_ram` paints the free stack
(`__bss_end` .. SP − 256) with `0x53544B21`; `native::stack_use()` returns the
high-water mark (first non-paint word from `__bss_end` up) and the stack size,
reported in `DBG:BOOT?` as `stack_used`, `stack_size` (`used == size` means
it reached `.bss`).

The other `native::boot_words` fields of `DBG:BOOT?`: `staged` = fatren's
count of `dm40*.bin` left because their header is not the marker; `calbk_ms`
= `calbackup::boot`'s time in ms; `spiclk_div` / `spiclk_check` = the
SPI-flash divider in use (4 or 16) and the boot self-check verdict of [`src/spiflash.rs`](../src/spiflash.rs):
`OK` = block 0 and the first FAT block read identically at /16 and twice at
/4 with 0x55AA, else /16 for this boot because of `DIFF1` / `DIFF2` (a /4
read differed), `NOSIG` (no 0x55AA at /16), `NOCHIP` or `FORCED`
(`spiflash::FORCE_SLOW`); fatdir and the settings: "The SPI disk" and
"Settings copy" above.

## Risks / open items

- **RCU_RSTSCK clearing:** the shim clears the reset flags, which the stock
  firmware never does. Neither the ROM nor the loader reads them
  (`stock/re/docs/fsloader-install-trigger.md`, "Reset cause").
- **Review L2** (FWDGT: wait for PUD / RUD *after* the PSC / RLD writes, in
  [`src/wdt.rs`](../src/wdt.rs)) is not done; `start::fwdgt` waits only
  before them. Effect: the first period may be up to ~32 s.
- **Review L4** (power loss during a fatren root-block rewrite) is described
  in the review.
- **Zeroing the marker is irreversible** without a vendor install. It is used
  only where a staged image must be installed: `DBG:UPDATE`, the on-meter
  update, or the request word (set by nobody now). `DBG:LOADER` and the key
  hold use the PA2 entry instead.
- **The PA2 entry depends on the GPIO lock** (device: the staged-file
  install through it, "PA2 entry"); without it the meter reboots into
  opendm40 (`last_trigger` `0x54520013`).
- **A hang inside the shim on every boot** has no software way out: the
  TX-GND short (case open) is the only one.

## Boot and watchdog: owner steps (2026-10-29)

What the shim does since 0.4.0, checked in the emulator with the real ROM
and FS-loader ([`emu/recoverytest.py`](../../emu/recoverytest.py)): a power
key held 4.5 s at power-on opens the loader through PA2 (marker kept);
every hang (or `DBG:RELEASE`) is a reboot ~20 s later (`WDT_RETRY`, the crash word counts
up, cleared after 60 s of main loop). The crash count, last trigger and
previous mode are in backup SRAM (`0x40024FF0`, `FF4`, `FFC`); they survive
resets, not necessarily a power-off without battery.

Before: opendm40 running, USB connected; on the PC a copy of the installed
image (`firmware/opendm40.bin` of the installed build, or
`INFO/FIRMWARE.BIN` from the opendm40 USB drive).

1. **Short press.** Power off, press the power key ~0.5 s. Expect the
   backlight at once, opendm40 normally; `tools/dm40dbg.py boot`: `NORMAL`,
   reason `NONE`.
2. **Long press.** Power off, hold the key ~7 s from power-on. Expect a
   beep at ~4 s, then the loader's update screen and its USB disk
   (28e9:028f). Power-cycle (the loader's key switches off): opendm40;
   `boot`: marker `VALID`, `last_trigger` `0x54520012`. Letting go right
   after the beep instead: opendm40, reason `KEY_HOLD_ABORTED`.
3. **Hangs in a row.** `tools/dm40dbg.py release`; ~20 s later opendm40
   reboots: `boot` `WDT_RETRY`, crash word `0x43520001`. Right away
   `release` again: another reboot, `WDT_RETRY`, `0x43520002`, no loader.
4. **Healthy clear.** Wait 70 s, then `boot`: `HEALTHY`, `0x43520000`.
5. **Loader, marker kept.** `tools/dm40dbg.py loader`: the update screen;
   power-cycle: opendm40, marker `VALID`.
6. **Loader on request.** `tools/dm40dbg.py update` with no file staged:
   the loader's "open file error / Update" screen and its USB disk.
   Reinstall the image (copy it as the only `dm40*.bin`, `sync`, eject,
   press "Update"); `boot`: marker `VALID`, crash `0x43520000`.

## Developer workflow

Build, reload (`tools/reload.sh`), first install and the way back: [firmware/README.md, Developer workflow](../README.md#developer-workflow).
