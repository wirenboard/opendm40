**English** | [Русский](README.ru.md)

# opendm40

Open-source replacement firmware for the Alientek DM40 multimeter,
oscilloscope and signal generator (DM40A, DM40B, DM40C).

The multimeter, the oscilloscope and the generator run at the same time; the
screen only chooses what you see, and the combined view shows the meter
reading above the live scope. opendm40 keeps the stock look and touch control
and adds a power meter, inrush current capture, wrong-connection alerts, data
logging, remote control over USB (USBTMC / SCPI) and Bluetooth, and a USB disk
with your logs and screenshots as CSV and BMP files.

It installs like a stock update file and does not touch the Alientek
bootloader, the update loader or your meter's calibration, so you can always
go back to the stock firmware.

![opendm40 on a DM40A: the multimeter page](docs/manual/img/dmm.png)

*opendm40 on a DM40A: the multimeter page.*

## Status

Experimental. Developed on a DM40A and tested on DM40A, DM40B and DM40C.
Everything that has not run on a real meter yet, chiefly
everything with a current through the A jack, is listed in the manual under
[Known limitations](docs/manual/README.md#known-limitations).
Measured accuracy against calibrators (DC V, DC A, frequency, resistance):
[firmware/docs/data](firmware/docs/data/README.md).

Downloads: [Releases](https://github.com/opendm40/opendm40/releases).
Changes per release: [CHANGELOG.md](CHANGELOG.md).

## What it does

| | |
|---|---|
| ![Multimeter and scope at once](docs/manual/img/combined.png) | ![Scope with FFT](docs/manual/img/scope.png) |
| Multimeter and scope at once | Scope: hardware trigger, 20 measurements, FFT, cursors, Save CSV |
| ![Power meter](docs/manual/img/power.png) | ![A-JACK alert](docs/manual/img/ajack.png) |
| Power meter: P, S, Q, PF, f, Wh with DC, AC or AC+DC coupling | Alerts for wrong connections, with a loud alarm for dangerous ones |
| ![Recording](docs/manual/img/record.png) | ![Generator](docs/manual/img/generator.png) |
| Recording to the internal disk, read as CSV over USB | Generator, running on every page |

- **Multimeter:** all 16 stock functions; 60000-count ranges and AC+DC on
  every model; AUTO AC/DC; HOLD, AutoHold, REL, MIN/MAX/AVG, limits, trend
  chart; burden voltage.
- **Oscilloscope:** stock timebases and V/div, Auto / Normal / Single, edge,
  pulse and runt triggers; a slow isolated Wave view on the multimeter jacks.
- **Generator:** sine, triangle and ramp up to 50 kHz, square up to 10 MHz,
  DC offset.
- **Power meter and inrush current** on the 10 A range.
- **Data:** a Record page, a fast logger of raw samples (up to 38.4 kS/s),
  screenshots and an on-screen gallery.
- **Computer:** a USB disk that needs no driver; USBTMC / SCPI in the style of
  the 34401A; SCPI and the stock phone-app protocol over Bluetooth; a libsigrok
  profile (not upstream yet) and a 34401A emulation for unpatched sigrok.

What is new compared with the stock firmware, feature by feature, and what
the stock firmware has that opendm40 does not (in short: the menus are
English only):
[the manual, What's new](docs/manual/README.md#whats-new-compared-with-the-stock-firmware).
Item by item against the stock firmware V2.6: [parity.md](firmware/docs/parity.md).

## Install

You need a DM40 with the stock firmware, a USB cable and a charged battery.

1. Download `dm40_opendm40_v<version>.bin` from the latest
   [release](https://github.com/opendm40/opendm40/releases/latest)
   (`SHA256SUMS` next to it), or build it ([firmware/README.md, Build](firmware/README.md#build)).
2. On the meter: **Settings > Other Settings > USB Connect**, and connect the
   cable. A USB disk appears.
3. Copy the file onto it, as the only `dm40*.bin` file there. Eject the disk.
4. Switch the meter off and on. The stock update procedure installs the file.
   The first start takes a few seconds (opendm40 shows a progress bar): do
   not switch off in the first 10 seconds.
5. Back up your meter: copy the `INFO/` folder from the opendm40 USB disk to
   your computer and keep it ([Back up](#back-up-with-opendm40)).

The same steps with pictures, and updating from a running opendm40:
[the manual, Getting started](docs/manual/README.md#getting-started).

<a id="back-up-with-opendm40"></a>
<a id="recovery"></a>

## Back up, recovery and back to stock

An update rewrites only the firmware part of the meter's memory. A bad update
file cannot brick the meter: the update loader then shows its update screen
and its own USB disk until a good file is installed.

- **Back up:** right after the install, copy the `INFO/` folder from the
  opendm40 USB disk. It holds the calibration and the boot data of your unit,
  which no update file contains
  ([what is in it](docs/manual/README.md#back-up-your-meter)).
- **Back to stock:** install the official
  [`dm40_v_2_6.bin`](stock/vendor/dm40_v_2_6.bin) the same way. Your
  calibration was never changed.
- **If opendm40 does not start:** from off, hold the power key until the
  meter beeps, then half a second more, and install a file from the update
  loader; or open the case and use the debug header
  ([Reach the update loader](docs/manual/README.md#reach-the-update-loader)).

Step by step: [the manual, Backup, recovery and back to
stock](docs/manual/README.md#backup-recovery-and-back-to-stock).

## Safety and warranty

- opendm40 is experimental software and comes **without any warranty**
  ([LICENSE](LICENSE)). Installing it may void the vendor's warranty.
- It is not certified. The meter's safety ratings come from its hardware and
  from Alientek's user manual; opendm40 does not change the input protection
  or the ratings. Follow the safety rules and limits in Alientek's user
  manual.
- The wrong-connection alerts work from what the meter measures and cannot
  catch every mistake
  ([What cannot be detected](docs/manual/README.md#what-cannot-be-detected)).
  Do not rely on them for your safety.

## Use it from a computer

The meter appears as a USB disk (no driver) and a USBTMC instrument.
[Connecting on Linux, Windows and macOS, PyVISA, sigrok, Bluetooth](docs/manual/README.md#remote-control-and-connecting-from-a-computer);
every command: [SCPI reference](docs/manual/scpi-reference.md).

## Documentation

- [User manual](docs/manual/README.md)
- [SCPI reference](docs/manual/scpi-reference.md)
- Developer documentation, hardware, the emulator and archived documents:
  [docs/README.md](docs/README.md)

## Build and develop

The firmware is written in Rust for the GD32F427 (Cortex-M4F). Build,
debug link, tests and releases: [firmware/README.md](firmware/README.md). The
[emulator](emu/README.md) runs opendm40 and the stock firmware without a
meter. The hardware as far as it is known: [hw/README.md](hw/README.md).
Tools for the stock update file: [stock/README.md](stock/README.md).
Bug reports and pull requests: [CONTRIBUTING.md](CONTRIBUTING.md).

## Releases

Releases follow [Semantic Versioning](https://semver.org/) and are published
on [GitHub Releases](https://github.com/opendm40/opendm40/releases) with the
install file `dm40_opendm40_v<version>.bin`; the changes per release are in
[CHANGELOG.md](CHANGELOG.md), the rules in
[firmware/README.md, Releasing](firmware/README.md#releasing).

## Licence

opendm40 is under the MIT licence ([LICENSE](LICENSE)), © the opendm40 contributors.

Not under the MIT licence:

- [`stock/vendor/dm40_v_2_6.bin`](stock/vendor/README.md): Alientek's
  unmodified update file, kept to return a meter to stock.
- The bitmap fonts of the stock firmware V2.6, which opendm40 copies into its
  image for the stock look
  ([`firmware/src/stockfont_data.rs`](firmware/src/stockfont_data.rs),
  [how they are made](firmware/docs/fonts.md)).
- [`hw/photos/`](hw/photos/README.md) and [`hw/datasheets/`](hw/datasheets/README.md):
  third-party material under its owners' copyright (the review author, the
  chip manufacturers), kept for reference.

[`stock/patch/`](stock/patch) contains addresses and symbol names of the
stock firmware V2.6.

The names Alientek and DM40 belong to their owner and are used here only to
identify the hardware. opendm40 is not affiliated with or endorsed by Alientek.
