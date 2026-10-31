# DM40 emulator (Renode)

The emulator runs the DM40 firmware on a PC, without a meter. It is a
[Renode](https://renode.io/) machine: a Cortex-M4F with models of the
[GD32F427](../hw/datasheets/gigadevice-gd32f427xx-datasheet-rev1.9.pdf)
peripherals, the [ILI9488](https://www.crystalfontz.com/controllers/uploaded/ILI9488%20Data%20Sheet_100.pdf)
LCD, the touch controller, the SPI flash disk, the Bluetooth module and the
[HY3131](../hw/datasheets/hycon-hy3131-datasheet-v10.pdf) DMM chip with the
meter's front end ([emu/hy3131/README.md](hy3131/README.md)). The USB device
is exported over USB/IP. It runs the stock firmware V2.6, opendm40 and the
whole boot chain ROM -> FS-loader -> application from a flash dump. You
script touches, keys and the signals on the DMM jacks and the scope input,
and take screenshots. Nothing here talks to a real meter.

| stock firmware V2.6 | opendm40 |
|---|---|
| ![stock DMM page](img/stock-dmm.png) | ![opendm40 DMM page](img/fwrs-dmm.png) |

*3.3012 V DC on the V-COM jacks (`--dmm dcv:3.3012`): `emu/run.py stock`
(left) and `emu/run.py fwrs` (right).*

## Contents

1. [Requirements and setup](#requirements-and-setup)
2. [Quick start](#quick-start)
3. [run.py options](#runpy-options)
4. [Live window](#live-window)
5. [USB over USB/IP](#usb-over-usbip)
6. [Tests](#tests)
7. [Input markers](#input-markers)
8. [What is modelled](#what-is-modelled)
9. [Model limits](#model-limits)
10. [Files](#files)
11. [Links](#links)

## Requirements and setup

| What | Where | Notes |
|---|---|---|
| Renode 1.17.0 | `emu/.renode/` | [`emu/get-renode.sh`](get-renode.sh) downloads the portable build (about 150 MB, no sudo, git-ignored) |
| Python 3 | | [`run.py`](run.py) uses the standard library; `mtools` for `--disk-add`. Some tests need PIL (screenshot checks). The [live window](#live-window) needs `/usr/bin/python3` with PyGObject, libadwaita, pycairo, numpy and PIL |
| opendm40 build | `firmware/target/native/thumbv7em-none-eabihf/release/opendm40` | [firmware/README.md](../firmware/README.md). Without `--app`, `run.py fwrs` builds HEAD itself in `emu/out/fwrs-worktree` |
| Flash dump of your meter, 1 MiB | `stock/re/dumps/full-20261013/flash_08000000_1MB.bin` | Required: ROM, FS-loader, loader data and the calibration. `--base` or `--flash` names another file |
| OTP dump | `stock/re/dumps/full-20261013/otp_1FFF7800.bin` | Optional (`--otp`) |
| Chip UID, 12 bytes | `private/memrw-kit-test/full/uid_1FFF7A10.bin` | Optional. Without it the stock firmware's UID check fails and it runs at 16 MHz instead of 200 MHz. `--uid` sets the UID |
| SPI flash dump, 16 MiB | `stock/re/dumps/spi_dumps/dm40_msc_full_2026-10-13.bin` | Required unless `--disk`. Copied once to `emu/out/disk.bin`, the working copy |
| Stock application | `dm40_v_2_6.dec.bin` (repo root) | Only for `stock` and `chain` without `--app`: `python3 stock/dm40fw.py decode stock/vendor/dm40_v_2_6.bin dm40_v_2_6.dec.bin` ([stock/README.md](../stock/README.md)) |

The dumps and the UID file come from your own meter and are not in the
repository. The dumps live in the private clone in `stock/re/` ([firmware/README.md](../firmware/README.md#stock-firmware-analysis-private)).
The emulator only reads them: images are composed in `emu/out/` (git-ignored).

Each instance gets a chip UID of its own: by default bytes 4 and 9 of the
UID are the USB/IP port (`emu_uid` in [`run.py`](run.py)). opendm40 derives
its USB serial number from the UID, so an emulated meter never has the real
meter's serial.

## Quick start

```sh
emu/get-renode.sh                                     # once
(cd firmware && ./build.sh)                           # build opendm40
APP=firmware/target/native/thumbv7em-none-eabihf/release/opendm40
emu/run.py fwrs --app $APP --dmm dcv:1.234 --run-for 4 --shot emu/out/fwrs.png
emu/run.py fwrs --app $APP --fast --usbip 0.01 --run-for 600 &    # USB/IP on TCP 3240
emu/usbip_client.py usbtmc 1-0 "*IDN?"                # Alientek,DM40A,<serial>,opendm40 <version>
```

The stock firmware: `emu/run.py stock --run-for 3 --shot emu/out/stock.png`.
The full boot chain: `emu/run.py chain --trace-boot --run-for 4`.

Modes:

| Mode | Application at 0x08004000 | Starts at |
|---|---|---|
| `stock` | stock V2.6 (`dm40_v_2_6.dec.bin`) | the application |
| `fwrs` | opendm40 (`--app` ELF, body or `dm40*.bin`; default: built from HEAD) | the application |
| `chain` | stock, or `--app`; `--app dump` keeps the dump's own application | the ROM at 0x08000000 |

Screen coordinates are the 480x320 landscape frame. The stock main menu
tiles (190x106 each) are at (46,52) Multimeter, (244,52) Oscilloscope,
(46,166) Signal, (244,166) Settings.

## run.py options

The full list: `emu/run.py --help`. Times (`@T`, `--run-for`) are in virtual
seconds.

| Option | What it does |
|---|---|
| `--app FILE` | the application: ELF, body, `dm40*.bin` update file, `.dec.bin`, or `dump` |
| `--rev REV` | `fwrs` without `--app`: build this commit instead of HEAD |
| `--model A\|B\|C` | DM40 model: patches the calibration model byte and its CRC in the composed image ([model-variants.md](../firmware/docs/model-variants.md)) |
| `--flash IMG`, `--base IMG` | use a 1 MiB image as it is; compose from another dump |
| `--disk IMG`, `--disk-add FILE[::NAME]` | another SPI disk image; copy a file into a private copy of the disk |
| `--save-disk OUT`, `--save-flash OUT` | write the SPI disk / internal flash at the end |
| `--uid HEX24\|FILE\|unit` | chip UID; `unit` = the UID file as it is |
| `--run-for S` | run time (default 3) |
| `--shot PNG[@T]`, `--autoshot N` | screenshot at T (default: at the end); a PNG every N frames |
| `--tap X,Y[@T]`, `--drag X0,Y0,X1,Y1[@T]` | touch; a drag takes 0.3 s |
| `--key NAME[@T]` | `menu`, `run`, `auto`, `mf`, `power`, `power-hold`, `pa2` |
| `--dmm SPEC[@T]` | what is connected to the DMM jacks, several specs with `;`: elements such as `vsrc:1.234`, `isrc:0.0123`, `r:4700`, `c:100e-9`, `open`, or the older keys `dcv:1.234`, `acv:1.0@50`, `ohm:4700`. Syntax: [emu/hy3131/README.md](hy3131/README.md) |
| `--vin V[@T]` | DC volts on V-COM (older form of `--dmm`) |
| `--scope SHAPE,HZ,VPP[,OFFSET][@T]` | scope input: `sine`, `square`, `triangle`, `ramp`, `dc`, `noise` |
| `--script FILE` | timed events, one per line: `T tap X Y`, `drag`, `key K`, `shot PNG`, `dmm SPEC`, `vin V`, `scope SPEC`, `monitor <Renode command>` |
| `--usbip T`, `--usbip-port P` | start the USB/IP server at T (default port 3240) |
| `--usbip-renode` | use Renode's own USB/IP server (only [`usbip_client.py`](usbip_client.py) works with it) |
| `--bt-port P` | Bluetooth UART on TCP port P (stock protocol or opendm40 SCPI) |
| `--fast`, `--mips N` | CPU speed: `--fast` = 50 MIPS, about real time; default 200. `--mips 110` matches the device's speed per instruction ([`CpuCost.cs`](peripherals/CpuCost.cs)) |
| `--pa2 T`, `--zero-marker`, `--trace-boot` | `chain`: release the power key at T (default 0.55; `0` = ROM -> application, `-1` = stay in the FS-loader); zero the loader marker so the FS-loader installs a `dm40*.bin`; log the boot stages |
| `--gdb [PORT]`, `--pause`, `--interactive`, `--dry` | GDB server (default 3333); do not start the CPU; keep the Renode monitor; only write the `.resc` |
| `--debug P1,P2`, `--pc-samples N`, `--stats`, `--buzzer-log FILE` | Renode debug log of peripherals (e.g. `sysbus.hy3131,sysbus.btmodule`); PC samples; wall and virtual time; buzzer register log |
| `--name NAME`, `--log FILE` | names of the generated `.resc` / `.log` in `emu/out/` |

With `--fast` the UI takes up to 4x longer in virtual time (opendm40 takes
the first key after about 4 s instead of 2.5 s): add about 1 s to the times.
In `chain` mode with `--fast`, use `--pa2 2.2`.

GDB:

```sh
emu/run.py fwrs --app $APP --gdb 3333 --pause --interactive
arm-none-eabi-gdb $APP -ex 'target remote :3333'
```

`monitor` commands in GDB go to the Renode monitor, e.g.
`monitor sysbus.lcd Screenshot @/tmp/x.png`.

## Live window

```sh
/usr/bin/python3 emu/live.py fwrs --fast --app $APP   # or stock | chain, plus run.py options
```

![GTK live window](img/gtk-panel.png)

A window drawn like the device: the live LCD, the touch keys, the power
button, and cards for the DMM jacks, the scope input and the Renode monitor.
The mouse on the display is the touch; `m r a f` = MENU RUN AUTO MF, `p` =
power, `s` = screenshot to `emu/out/live-*.png`. The header bar switches the
firmware and the model (DM40A/B/C), Fast and the real-time cap. The window
exports USB/IP on TCP 3240 (`--no-usbip`, `--usbip-port P`). All keys and
options: the header of [`live.py`](live.py). Logs: `emu/out/live-renode.out`,
`emu/out/live-monitor.log`. `--tk` uses a plain Tk window.

The signal-flow card under the device shows the route the firmware chose
from the jacks through the front end to the HY3131 converters:

![signal flow window](img/gtk-signal-flow.png)

## USB over USB/IP

`--usbip T` starts the model's USB/IP server
([`DWC2_OTG_USBIP.cs`](peripherals/DWC2_OTG_USBIP.cs); its header says what
it adds to Renode's own server). The device is busid `1-0`, full speed.
opendm40 enumerates as 1209:D040 with USBTMC and a mass-storage disk. The
stock firmware has a USB device only after Settings > Other > USB Connect
(28e9:028f, the 16 MiB SPI disk).

[`usbip_client.py`](usbip_client.py) needs no root and no kernel module:

| Command | What it does |
|---|---|
| `list`, `desc 1-0`, `config 1-0` | the exported devices; the device descriptor; interfaces and endpoints |
| `usbtmc 1-0 "*IDN?"` | one SCPI command or query over USBTMC |
| `scpi-tcp 1-0 5025` | SCPI lines on TCP 5025, e.g. for `sigrok-cli -d scpi-dmm:conn=tcp-raw/127.0.0.1/5025` |
| `msc 1-0 [LBA [LUN]]`, `luns 1-0` | INQUIRY, READ CAPACITY, READ(10); per-LUN checks |
| `mscdump 1-0 LUN OUT` | the FAT volume to a file (metadata and used clusters), for `fsck.fat` / mtools |
| `upload 1-0 FILE` | opendm40 `DBG:UPLOAD`: FILE -> `DM40FW.BIN` on the disk |
| `read 1-0 ADDR N`, `write 1-0 ADDR HEX` | vendor EP0 memory read / write (the `usbrw` patched stock image) |

`USBIP_PORT=P` selects a server on another port.

To attach the emulated meter to the Linux kernel (root):

```sh
emu/run.py fwrs --app $APP --fast --usbip 0.01 --run-for 3600 &  # usbip attach uses TCP 3240
sudo modprobe vhci-hcd
sudo usbip attach -r 127.0.0.1 -b 1-0
DM40_USBTMC=emu firmware/tools/dm40scpi.py info    # emu = the usbtmc under vhci_hcd
udisksctl mount -b /dev/sdX                        # sdX: the disk under vhci_hcd
sudo usbip port; sudo usbip detach -p 0
```

`DM40_USBTMC=emu` makes [`dm40scpi.py`](../firmware/tools/dm40scpi.py) and
[`dm40dbg.py`](../firmware/tools/dm40dbg.py) use the emulator and never the
real meter. Without root, [`kvmtest/run.sh`](kvmtest/run.sh) attaches the
emulator inside a KVM guest. [`usbip_proxy.py`](usbip_proxy.py) logs every
USB/IP PDU between `usbip attach` and the emulator.

## Tests

The emulator tests are not in CI: they need the dumps. Most tests are
Python scripts that start their own emulator; exit status 0 = PASS. The usage, the cases and the options are in each script's
header. Most take `--app <opendm40 ELF>` and `--port P` (the USB/IP port).

**Ports.** "free" = a free port in 61000-64999 (`run.free_port`), or in the
range of `DM40_EMU_PORTS=lo-hi`. A fixed default means two copies of that
test (or two tests with the same default) cannot run at the same time: give
each its own `--port`.

**Runtime.** Wall time, where the repository records one; empty = not
recorded.

### DMM ([`emu/hy3131/`](hy3131))

| Test | What it checks | Runtime | Port |
|---|---|---|---|
| [`modetest.py`](hy3131/modetest.py) | stock and opendm40 through every DMM function against the scripted input (results: [emu/hy3131/README.md](hy3131/README.md#tests)) | 25-80 s per scenario at `-j 6` | free (BT) |
| [`autorangetest.py`](hy3131/autorangetest.py) | DC V autorange after a multi-step range change; READ? across a range change | | 30410 |
| [`mvautotest.py`](hy3131/mvautotest.py) | V autorange around the mV ranges | | 63130 |
| [`mvrangetest.py`](hy3131/mvrangetest.py) | the 60 mV range: AUTO in and out, the range page, SCPI | | 63120 |
| [`resautotest.py`](hy3131/resautotest.py) | RES AUTO set from a manual RES range | | 64731 |
| [`ohmhunt.py`](hy3131/ohmhunt.py) | ohm range hunt: no unvetted value shown as final | | 58610 |
| [`ohmsinktest.py`](hy3131/ohmsinktest.py) | RES AUTO on an input the 6 Mohm and 600 kohm ranges disagree on | | 64810 |
| [`ohmnegtest.py`](hy3131/ohmnegtest.py) | ohm readings below 0 (RES:NEG ON / OFF) on shorted leads; AUTO stays on 60 ohm | | 64751 |
| [`ohm600mtest.py`](hy3131/ohm600mtest.py) | the experimental 600 Mohm range: manual only (explicit, MAX, AUTO, the AUTO key), READ? / MEAS? answer in time | | 64761 |
| [`contvistest.py`](hy3131/contvistest.py) | the beeper indicator (SHORT / BEEP band, green digits) against the input in CONT and DIOD at volume 0 / 50 %: on latency, 150 ms hold, a chattering contact, from the LCD write trace | | 40310 |
| [`beeptest.py`](hy3131/beeptest.py) | continuity / diode beeper (R20 bit 2) off at key/alert volume 0 %, on at 50 %, also after a volume change in continuity | | 64761 |
| [`couptest.py`](hy3131/couptest.py) | AUTO coupling (AC/DC) of V and A | | 3391 |
| [`autowaittest.py`](hy3131/autowaittest.py) | READ? / MEASure? while AUTO coupling decides | | 47600 |
| [`acdcmethtest.py`](hy3131/acdcmethtest.py) | AC+DC methods A / B / C | | 18110 |
| [`capdowntest.py`](hy3131/capdowntest.py) | capacitance AUTO: a small capacitor after a large one, open leads and a short (also the device's no-ramp case, AD1 forced) end on their own range; large capacitors keep theirs | | 47750 |
| [`captimetest.py`](hy3131/captimetest.py) | capacitance AUTO timing from DC V: time to the first grey and the first final value (1 nF .. 1 mF, absorption, open leads, a resistor) | | 47760 |
| [`freqtest.py`](hy3131/freqtest.py) | frequency: the first readings after entering the function | | 55100 |
| [`freqsteptest.py`](hy3131/freqsteptest.py) | frequency: steps of the input frequency while Hz reads (no wrong final value) | | 48110 |
| [`firstrdtest.py`](hy3131/firstrdtest.py) | the first readings after a function / range change | | 47700 |
| [`fastratetest.py`](hy3131/fastratetest.py) | FAST / MED / SLOW rates in DC V | | 47740 |
| [`dcvtest.py`](hy3131/dcvtest.py) | the DC-coupled V row on AD2 | | 3611 |
| [`switchlat.py`](hy3131/switchlat.py) | switch latency to the first and the settled reading, opendm40 and stock | | 45010 |
| [`senttest.py`](hy3131/senttest.py) | misconnect sentinel: each phase shows only its expected alerts | | 3377 |
| [`switchtest.py`](hy3131/switchtest.py) | misconnect sentinel through function / range / coupling switches | | 44100 |
| [`loudtest.py`](hy3131/loudtest.py) | danger alarm (buzzer register log) | | 53101 |
| [`powtest.py`](hy3131/powtest.py) | power meter | | 3381 |
| [`pwrangingtest.py`](pwrangingtest.py) | power meter before its first reading: no DMM column beside "I ranging..." (#96) | | free |
| [`stripwaittest.py`](stripwaittest.py) | combined view: the meter strip names the power meter before its first reading (#108) | ~2 min | free |
| [`burdentest.py`](hy3131/burdentest.py) | burden voltage readout | | 3378 |
| [`inrushtest.py`](hy3131/inrushtest.py) | inrush current capture | | 3597 |
| [`pwdialogtest.py`](pwdialogtest.py) | power meter dialogs (wiring, Reset energy?) and the fuse check dialog close on a SCPI function change (#104, #143) | | free |
| [`btwaketest.py`](hy3131/btwaketest.py) | DMM rail (PE2) wake with a Bluetooth link | | 4833 |
| [`tests/run.sh`](hy3131/tests/run.sh) | host unit tests of the HY3131 model, no emulator (dotnet 8) | | - |

### Scope

| Test | What it checks | Runtime | Port |
|---|---|---|---|
| [`wavtest.py`](wavtest.py) | waveform readout (WAV:DATA?, DIGitize, preamble) against the input sine, over USBTMC and Bluetooth | ~5 min | 24140, BT 24141 |
| [`hy3131/wavetest.py`](hy3131/wavetest.py) | the DMM Wave view (scope page, source DMM); build with `--features ad2-isr` | | 3591 |
| [`railstest.py`](railstest.py) | analog rails (PE2 / PC4, `run.py --rail-log`) after a boot with Both always and a switch to Auto on the Scope page: the scope rail comes on, never both rails off | | free (50000-50999) |
| [`wavedroptest.py`](wavedroptest.py) | the Wave view's ring drops no samples (`--ch2`: the scope page with CH2 (DMM) on) | | free |
| [`trigdragtest.py`](trigdragtest.py) | marker drags with a wobbling finger: one item, one axis per gesture | | free |
| [`sratetest.py`](sratetest.py) | the timebase list's per-entry sample rates = the ui-render `s07-srate-*` renders and ACQ:SRAT? across timebases, roll and the Wave view; picks in the open list (`--refs` = a `tools/ui-render/run.sh` output directory) | | free |
| [`rollmeastest.py`](rollmeastest.py) | roll mode: no measurements while rolling (not the last frame's), a stopped roll record measured | | free |
| [`gentrigtest.py`](gentrigtest.py) | trigger source GEN: DAC-path phase readings (`DBG:GENTRIG:STAT?`) and a stream-timebase trigger, the PWM path triggering, `TRIG:SOUR CHAN1`, the generator off. The DAC phase itself stands still here (no DMA from TIM6) | | free |
| [`draglat.py`](draglat.py) | level-marker drag latency on the screen | | free |
| [`popupbeep.py`](popupbeep.py) | CH1 popup open / close: redraw length as the tap beep length (buzzer register log), the CH1 label after it | ~2 min | free |
| [`popupscpitest.py`](popupscpitest.py) | scope: SCPI timebase / V/div changes while the CH1 popup is open show in the top bar at once (#82) | ~2 min | free |
| [`dragperf.py`](dragperf.py) | trigger-level drag: main-loop period and touch-to-pixel latency (prints figures) | | free |
| [`triglevtest.py`](triglevtest.py) | `TRIG:LEV` keeps the level as given (V/div steps, off screen); Normal triggers on it | ~2 min | free |
| [`stopzoomtest.py`](stopzoomtest.py) | stopped scope: each timebase change re-renders the stored record, also right after a screen read-back | ~2 min | free |

### Logger and the AD2 stream

| Test | What it checks | Runtime | Port |
|---|---|---|---|
| [`hy3131/ad2streamtest.py`](hy3131/ad2streamtest.py) | the AD2 stream ISR end to end; build with `--features ad2-isr` | | 3378 |
| [`hy3131/logduptest.py`](hy3131/logduptest.py) | AD1 stream logger records no conversion twice | | 47710 |
| [`logfmttest.py`](logfmttest.py) | LOG:DATA? in each LOG:FORMat is lossless | | 63610 |
| [`fastlogtest.py`](fastlogtest.py) | fast logger to the SPI disk: the sustainable rate with the flash busy timing | | 4817, BT 4818 |
| [`fastendtest.py`](fastendtest.py) | end of a disk session while a host reads the USB volume | | 4827 |
| [`csvsharetest.py`](csvsharetest.py) | LOG / FAST CSVs read over MMEM:DATA? and the USB volume in turn equal dlog.py (shared generators) | ~4 min | free |

### USB, disk, settings and boot

| Test | What it checks | Runtime | Port |
|---|---|---|---|
| [`usbtmc_stress.py`](usbtmc_stress.py) | USBTMC: thousands of command / query exchanges; needs an emulator started with `--usbip` | 508 s for 3000 iterations | 3240 (`--port`) |
| [`mscresettest.py`](mscresettest.py) | mass storage recovery: Bulk-Only reset, CLEAR_FEATURE(HALT), port reset | | free |
| [`ejecttest.py`](ejecttest.py) | eject through the real Linux usb-storage path; needs `sudo usbip` | | free; its proxy needs 3240 |
| [`kvmtest/run.sh`](kvmtest/run.sh) | the Linux mass-storage path in a KVM guest, no root; copies during a fast-log session | | argument |
| [`mirrortest.py`](mirrortest.py) | the INFO/ slow-bank mirror on the SPI disk across boots; needs two builds (`--app2`) | | free |
| [`gallerytest.py`](gallerytest.py) | the gallery: grid, viewer, Delete | ~8 min | 19310 |
| [`shottest.py`](shottest.py) | the on-device screenshot BMP equals the screen | | 17310 |
| [`settingstest.py`](settingstest.py) | settings survive a power-off | | 63900-63999 |
| [`idletest.py`](idletest.py) | USB, Bluetooth and disk traffic and an armed inrush page hold off the automatic power-off | | free, BT free |
| [`chargetest.py`](chargetest.py) | the battery blinks green while PB10 / PB11 read "charging" (driven from the monitor), "USB" while a host is attached | | free |
| [`usbofftest.py`](usbofftest.py) | no automatic power-off on USB power (PB10 driven from the monitor); after the unplug the time counts afresh | ~7 min | free |
| [`recoverytest.py`](recoverytest.py) | boot decisions through the whole boot chain: repeated hangs boot opendm40 again; the power-key hold and the PA2 request word open the loader with PA2 low and locked (the GPIO model enforces GPIOx_LOCK until a reset), a power cycle then boots opendm40; the recovery request word zeroes the marker; `release` (DBG:RELEASE) and `usb-loader` (DBG:LOADER) on request | ~8 min (10 cases, `--jobs 8`) | free |

### Bluetooth, keys, touch and UI

| Test | What it checks | Runtime | Port |
|---|---|---|---|
| [`btchattertest.py`](btchattertest.py) | the module's start-up text is not taken as SCPI; a host's first command is answered; RX overrun reported (-363) | ~4 min | BT 4834 |
| [`autokeytest.py`](autokeytest.py) | the AUTO key in the DMM | | free |
| [`limitsbacktest.py`](limitsbacktest.py) | MENU / Back on Hold & Limits and Record: one page up (the range page or the multimeter); the range pages' Record button, Start / Stop | ~2 min | free |
| [`chartunittest.py`](chartunittest.py) | DMM trend chart after a range change to another unit that reads OL (#93) | | free |
| [`chartreltest.py`](chartreltest.py) | DMM trend chart keeps its points on REL on / off, as the stock (#98) | | free |
| [`touchpagetest.py`](touchpagetest.py) | the touch test page and touch calibration | | 65200-65300 |
| [`xramtest.py`](xramtest.py) | the extra RAM areas (backup SRAM after the settings record, USBHS FIFO RAM) start zeroed over stale bytes; the settings record survives DBG:RESET while their buffers are in use | ~4 min | 64600-64699 |
| [`smoothtest.py`](smoothtest.py) | page, popup and DMM function changes: no refresh shows a large part of the screen neither old nor new (a cleared background, a popup closed and reopened); prints each redraw's time | ~2 min | - |
| [`tempinfotest.py`](tempinfotest.py) | the temperature page's cold junction line follows an offset step (#142) | | free |
| [`monkeytest.py`](monkeytest.py) | seeded random taps, keys, drags, swipes and SCPI (edge values) on every page: no restart, no hang, MENU always reaches the main menu, the deepest stack use; optional soak (`--seed`, `--minutes`) | about real time (`--fast`) | free |
| [`flowtest.py`](flowtest.py) | the live window's signal-flow diagram does not move; no emulator | | - |

### Helpers

| File | What it is |
|---|---|
| [`emusync.py`](emusync.py) | reads the [input markers](#input-markers) from the Renode log |
| [`regionc.py`](regionc.py) | Renode hooks: which region C functions run while the AD2 stream ISR owns the bus |
| [`lcdtrace.py`](lcdtrace.py) | replays an LCD write trace (`sysbus.lcd Trace PATH`) at the panel's refresh rate: redraw time, transient pixels, filmstrips |
| [`infovol.py`](infovol.py) | reads the USB volume's INFO/ files during an AD2 stream |
| [`hy3131/runemu.py`](hy3131/runemu.py) | `run.py` with a separate HY3131 compile unit |
| [`usbip_client.py`](usbip_client.py), [`usbip_proxy.py`](usbip_proxy.py) | USB/IP client and logging proxy ([USB over USB/IP](#usb-over-usbip)) |
| [`live.py`](live.py), [`gtkpanel.py`](gtkpanel.py), [`panel_draw.py`](panel_draw.py) | the [live window](#live-window); `panel_draw.py` also draws the device pictures of the user manual |

## Input markers

Scenario tests change the input at fixed virtual times, but read the
firmware's uptime (`DBG:PING?`). The two clocks differ by the boot time,
which changes whenever the boot does. So the models log both clocks, and the
tests key on these lines in the Renode log:

```
dwt: CYCCNT write 0x00000000 at 2.849631           (DWT.cs: opendm40's clock starts)
hy3131: input: open;dca:0.3 at 16.0000 fw 13.1504  (HY3131 SetInput: each input change)
```

[`emusync.py`](emusync.py) reads them (`Markers`) and places a scenario's
phases, actions and check windows in firmware uptime (`Timeline`). A run
whose markers are missing, or whose firmware clock restarted, fails. Use it
in every new scenario test instead of a guessed boot offset.

## What is modelled

Models are C# files compiled by Renode at start-up; the platform is
[`dm40.repl`](dm40.repl). The detail is in each file's header.

| Block | Model | Notes |
|---|---|---|
| CPU, NVIC, SysTick | Renode `cortex-m4f` | 200 MIPS by default |
| DWT CYCCNT | [`DWT.cs`](peripherals/DWT.cs) | from virtual time; logs the clock marker |
| CPU cost | [`CpuCost.cs`](peripherals/CpuCost.cs) | `--mips 110` and `IsrStallNs` approximate the device's speed; off by default |
| Flash, SRAM, TCM, backup SRAM, OTP / UID | memory | |
| FMC flash controller | Renode `STM32F4_FlashController` | erase and program as plain writes; BUSY never set |
| RCC, PWR | Renode STM32F4 models | ready bits follow the enables; every timer clock is 200 MHz |
| LCD (FSMC bank 1) | [`ILI9488.cs`](peripherals/ILI9488.cs) | RGB666 read-back as the panel; `Screenshot`, `AutoShot`, `Trace` (every RAM write burst with its time) |
| GPIO | [`GD32_GPIO.cs`](peripherals/GD32_GPIO.cs) | pull-ups give the level of undriven inputs |
| TIM1, 2, 5, 7, 8 | [`GD32_Timer.cs`](peripherals/GD32_Timer.cs) | slave modes, one-pulse, capture, PWM, TRGO; events at their exact virtual time. TIM7 is opendm40's AD2 stream tick |
| TIM3, 6, 12-14 | [`PwmTimer.cs`](peripherals/PwmTimer.cs) | registers and counter only, no events |
| TIM4, 9-11 | Renode `STM32_Timer` | tick interrupts |
| DMA | [`GD32_DMA.cs`](peripherals/GD32_DMA.cs) | circular, double buffer, M2M |
| SPI flash (W25Q128) | [`GD32_SPI.cs`](peripherals/GD32_SPI.cs), [`W25Q128.cs`](peripherals/W25Q128.cs) | file-backed 16 MiB; program / erase busy times; byte timing as the device (`--spi-disk-gap-ns`) |
| Touch (I2C) | [`GD32_I2C.cs`](peripherals/GD32_I2C.cs), [`CST_Touch.cs`](peripherals/CST_Touch.cs) | `Tap`, `Press`, `Move`, `Release`, `Drag`, `Key` |
| HY3131 and the front end | [`hy3131/HY3131.cs`](hy3131/HY3131.cs), [`hy3131/DM40Frontend.cs`](hy3131/DM40Frontend.cs) | registers, conversion timing, every DMM function, with the meter's calibration: [emu/hy3131/README.md](hy3131/README.md) |
| Scope ADC and comparator | [`ScopeFeeder.cs`](peripherals/ScopeFeeder.cs) | one sample per TIM1 update; comparator on PA15 with the board's delay and hysteresis |
| Bluetooth module (UART4) | [`BtModule.cs`](peripherals/BtModule.cs) | AT set-up, start-up banner, PC14 "connected", `--bt-port`; its bytes to the firmware at most 115200 (`RxByteUs`), the firmware's TX at once |
| Battery ADC | [`SimpleADC.cs`](peripherals/SimpleADC.cs) | channel 15 = 4.0 V (`SetVolts ch v`) |
| Watchdog, reset flags | [`FWDGT.cs`](peripherals/FWDGT.cs) | a timeout resets the machine; RSTSCK flags |
| USB OTG FS | `DWC2_OTG*.cs` | Renode's Synopsys OTG model with fixes for the GD32 drivers; [USB/IP](#usb-over-usbip) |
| RTC, CRC, RNG | Renode models | |

## Model limits

- **Speed.** At 200 MIPS the emulator runs 2-4x slower than real time;
  with `--fast` about real time. `run.py` is not capped; the live window caps
  virtual time to the wall clock.
- **CPU timing.** The device is about 1.8x slower per instruction than 200
  MIPS. `--mips 110` approximates it; code timing is not cycle-exact.
- **Clocks.** Clock values are not modelled: every timer runs at 200 MHz.
  No MCO, no low-power modes.
- **Generator.** The analog output (DAC1, TIM6 DMA) is not modelled: the
  firmware runs, nothing listens.
- **Not observed.** Buzzer, backlight, relays and power rails exist as GPIO
  / timer registers only (the buzzer has `--buzzer-log`).
- **HY3131.** The front end's per-row scale factors are the stock's nominal
  constants (the netlist is unknown); see the open points in
  [emu/hy3131/README.md](hy3131/README.md#open-points).
- **Boot timing.** The ROM's and FS-loader's PA2 (debug header TX) poll
  timing is set empirically: a release at 0.55 s gives the three-stage chain.
- **Scope trigger.** opendm40's comparator path sometimes never completes a
  frame after a restart; [`wavtest.py`](wavtest.py) turns it off.
- **Gestures.** Horizontal swipes from the touch model were not recognised
  by opendm40's gesture engine; not investigated. The tests swipe with
  `DBG:SWIPE` over USB.
- **USB.** Isochronous and interrupt endpoints, suspend and resume are
  untested. With the model's server, the `usbrw` image (a control OUT with a
  data stage) has not been run. The stock firmware never answers
  GET_DESCRIPTOR(string 4): the kernel times out twice and goes on.

## Files

```
emu/get-renode.sh      download Renode into emu/.renode/
emu/run.py             driver: compose the images, write the .resc, run, screenshot
emu/live.py            the live window
emu/dm40.repl          Renode platform description
emu/peripherals/*.cs   device models
emu/hy3131/            HY3131 model, its tests and the DMM scenario tests
emu/img/               the pictures in this README
emu/out/               generated images, scripts, logs, PNGs (git-ignored)
```

More screenshots from the emulator:

| | |
|---|---|
| ![opendm40 combined view](img/fwrs-combined.png) | ![opendm40 DMM wave view](img/fwrs-wave.png) |
| *Combined DMM + scope view: AC V 2.5 V 50 Hz on the jacks, a 1 kHz 2 Vpp sine on the scope input.* | *DMM Wave view: a 50 Hz square, 30 % duty, 2.5 V RMS on V-COM.* |
| ![opendm40 power meter](img/fwrs-power.png) | ![opendm40 generator page](img/fwrs-gen.png) |
| *Power meter: 230 V 50 Hz and 1.2 A at PF 0.8 (`--dmm 'open;pow:230,f=50,i=1.2,pf=0.8@0'`).* | *Generator page in its default state (the output is not modelled).* |
| ![stock scope page](img/stock-scope.png) | ![opendm40 scope page](img/fwrs-scope.png) |
| *Stock scope page, a 1 kHz 1 Vpp sine at 10 ms/div.* | *opendm40 scope page, a 1 kHz 2 Vpp sine at 200 us/div.* |

## Links

- [emu/hy3131/README.md](hy3131/README.md): the HY3131 and front-end model,
  the `--dmm` syntax, its tests.
- [firmware/README.md](../firmware/README.md): build, the developer loop,
  the host tests.
- [Documentation index](../docs/README.md).
- [The previous version of this README](../docs/history/emu-readme-2026-10-25.md):
  speed measurements, the USB/IP server fixes, the scope-trigger
  observations, the history of the input-marker offsets.
