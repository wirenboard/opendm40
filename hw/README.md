# DM40 hardware

What is inside the Alientek DM40 (DM40A / DM40B / DM40C), compiled from the
code and docs of this repo, the teardown photos in [photos/](photos/README.md)
and the mysku.club review they come from (CTAHOK, 2025-04-25,
https://mysku.club/blog/aliexpress/103047.html). Datasheets are in
[datasheets/](datasheets/README.md).

The teardown photos on this page are © CTAHOK (mysku.club review, 2025-04-25), kept
for reference only and not under this repo's license; the photos taken for this
repo are under its MIT licence. Sources and details in
[photos/README.md](photos/README.md).

This page is about hardware only: chips, pins, circuits. Flash layout and boot
chain: see [firmware/docs/native-boot.md](../firmware/docs/native-boot.md) (and
`stock/re/docs/fsloader-install-trigger.md`, `stock/re/ANALYSIS.md`). The
`stock/re/...` paths on this page are in the private stock-firmware analysis
repo, cloned into `stock/re/` by those with access
([firmware/README.md](../firmware/README.md#stock-firmware-analysis-private)).

No schematic of the DM40 is public. Facts without a mark are stated by the
source after them; the rest are marked *inferred* or listed under
[Unknowns](#unknowns). The pointer after a fact
says where to check it: stock code (RE headers in
`stock/re/analysis`), opendm40 code, a photo or a
datasheet. *Inferred*: deduced from register settings, scaling constants, the
community BOM or the photos. *BOM only*: the community BOM is the only
source.

Name clash: the [HY3131](datasheets/hycon-hy3131-datasheet-v10.pdf) has its own analog pins called PA0..PA9 and PB0..PB8.
On this page they are always written "HY3131 PBn"; a bare "PBn" is an MCU pin.

Sources used most: [`firmware/src/board.rs`](../firmware/src/board.rs) and the other drivers in [`firmware/src/`](../firmware/src),
`stock/re/analysis/*_v2_6.h`, [`firmware/docs/dac-adc-paths.md`](../firmware/docs/dac-adc-paths.md),
[`firmware/docs/power-meter-and-misconnect.md`](../firmware/docs/power-meter-and-misconnect.md), [`emu/hy3131/RESEARCH.md`](../emu/hy3131/RESEARCH.md),
the community BOM of board V1.1 ("BOM" below: posted by jebem in the
[EEVblog DM40C thread](https://www.eevblog.com/forum/testgear/alientek-dm40c-multimeter-%28dm40a-dm40b-dm40c%29/),
retold from the [mydigit.cn teardown](https://www.mydigit.cn/thread-486604-1-1.html),
registration needed; first collected in this repo in
[docs/dm40-feature-request.md](../docs/history/dm40-feature-request.md)), the DM40
manual V1.5 ([ManualsLib](https://www.manualslib.com/manual/3720530/Alientek-Dm40.html)),
[`firmware/docs/user-feedback.md`](../firmware/docs/user-feedback.md) (reports from the DM40 owners' Telegram chat [@S2084K](https://t.me/S2084K),
cited by message number).

## Contents

- [Block diagram](#block-diagram)
  - [Isolation boundary](#isolation-boundary)
- [Main parts](#main-parts)
- [MCU and clocks](#mcu-and-clocks)
- [SPI flash (USB disk)](#spi-flash-usb-disk)
- [LCD](#lcd)
- [Touch panel and touch keys](#touch-panel-and-touch-keys)
- [Power key, PA2, buzzer](#power-key-pa2-buzzer)
- [Debug header and hardware recovery](#debug-header-and-hardware-recovery)
  - [Hardware recovery](#hardware-recovery)
- [USB](#usb)
- [Bluetooth](#bluetooth)
- [DMM front end (HY3131)](#dmm-front-end-hy3131)
  - [Measured node behaviour](#measured-node-behaviour)
- [Oscilloscope](#oscilloscope)
- [Generator](#generator)
- [Power and battery](#power-and-battery)
- [Pin table](#pin-table)
- [Models DM40A / B / C](#models-dm40a--b--c)
- [Unknowns](#unknowns)

## Block diagram

The DMM part is galvanically isolated from the rest of the meter. The barrier
is the [CA-IS3741](datasheets/chipanalog-ca-is374x-v1.08.pdf) digital isolator (SPI), the optocouplers and the two
[VPS8504B](datasheets/vpsc-vps8504b-cn.pdf) + LT61 isolated supplies (markings in [Main parts](#main-parts), photos in
[Isolation boundary](#isolation-boundary)). Everything else shares one ground.

In the two block diagrams: dashed outline = inferred, rounded nodes are jacks,
edge labels are MCU buses and pins, the thick box is the isolation barrier. The
DMM front end is a simplified schematic with its own legend. Sources and
`render.sh`: [diagrams/](diagrams/).

**Overview.**

<img src="diagrams/overview.svg" width="900" alt="Block diagram: USB-C, power, MCU with scope and generator, the isolation barrier (CA-IS3741, optocouplers, VPS8504B + LT61) and the isolated DMM with its jacks">

**Rest of the meter: MCU, scope, generator.**

<img src="diagrams/mcu.svg" width="900" alt="Block diagram: GD32F427VGT6 with the scope chain, generator chain, LCD, SPI flash, Bluetooth, touch, buzzer and debug header">

**DMM front end (isolated).**

<img src="diagrams/dmm_frontend.svg" width="900" alt="Schematic: HY3131 with the V/ohm input (PTC, MOV, HFD4/3-LS relay, TC1746B6 divider taps), the shunt chain with the RUH30J51M MOSFETs and taps PB1 / PB2 / PB3, MAX6190 on PB6, the crystal, and the parts crossing the isolation barrier">

The same as text:

- Rest of the meter (one ground): USB-C with a TVS on VBUS; [TP4056](datasheets/toppower-tp4056-cn.pdf)
  charger and the LN425272 1S 2000 mAh cell with protection PCB; system supply
  (regulators not identified); [GD32F427VGT6](datasheets/gigadevice-gd32f427xx-datasheet-rev1.9.pdf) at 200 MHz from a
  25 MHz crystal, USBFS on PA11 / PA12; [ILI9488](https://www.crystalfontz.com/controllers/uploaded/ILI9488%20Data%20Sheet_100.pdf) 3.5" IPS 480 x 320 LCD on FSMC 16-bit, backlight PWM on PA6; touch
  (Hynitron 0x5A, plus 4 touch keys) on I2C2; [W25Q128JV](datasheets/winbond-w25q128jv-revI.pdf) 16 MiB (USB disk) on SPI1; Jieli BP2A838 BT
  (24 MHz crystal, PCB antenna) on UART4; buzzer PWM on TIM14 PA7; debug
  header: UART PA2 / PA3, SWD PA13 / PA14; scope: CHA jack -> /50 relay PC13, AC/DC
  PE3, offset PC8 -> 4051 + op amp 8091 gain -> [3PA1030](datasheets/3peak-3pa1030-reva1.pdf) 10-bit 50 MS/s on PB0..PB9, clocked by
  PC6 at 50 MHz; generator: DAC0 PA4 or TIM12 PB14 -> SGM3157 (PB12) ->
  output stage -> GEN jack; rails PE2 (to the isolated supplies) and PC4 (scope / generator).
- Isolation barrier: CA-IS3741 carries SPI2
  (PC10..PC12 + CS PD2); optocouplers (2 x EL3H7, 2 x "174 F163H") carry the drive of PA9 / PA10 (relay)
  and PE0 / PE1 (shunt MOSFETs) (*inferred*); 2 x VPS8504B with LT61-117 /
  LT61-113 transformers supply the DMM side.
- DMM (isolated, shielded): [HY3131](datasheets/hycon-hy3131-datasheet-v10.pdf) with 4.9152 MHz crystal, [MAX6190](https://www.analog.com/media/en/technical-documentation/data-sheets/MAX6190-MAX6198.pdf)
  1.25 V reference, 10 M divider with taps (TC1746B6 on B / C, discrete on
  A), [HFD4/3-LS](datasheets/hongfa-hfd4.pdf) latching relay, V/Ω input network (PTC, MOVs), fuse and shunt
  chain 10 mΩ + 0.5 Ω + 47.5 Ω with RUH30J51M MOSFETs; jacks V/Ω, COM,
  A/mA/µA.

### Isolation boundary

![Isolation strip: CA-IS3741HW, LT61 transformers with VPS8504B drivers, optocouplers, relay, HY3131 shield can](photos/dmm-isolation-hy3131-shield-area.jpg)

*The isolation strip: CA-IS3741HW, LT61-117 / LT61-113 with VPS8504B drivers, EL3H7 and "174 F163H" optocouplers, HFD4/3-LS relay, HY3131 shield can, CHA / GEN jacks.*

![Two VPS8504B drivers under the LT61 transformers](photos/isolated-supply-vps8504b-lt61.jpg)

*The two isolated supplies: VPS8504B drivers under the LT61-1xx transformers.*

The DMM part ([HY3131](datasheets/hycon-hy3131-datasheet-v10.pdf), reference, divider, relay, shunts, banana jacks)
is galvanically isolated from the rest of the meter through the
[CA-IS3741](datasheets/chipanalog-ca-is374x-v1.08.pdf) digital isolator, the optocouplers and the
[VPS8504B](datasheets/vpsc-vps8504b-cn.pdf) + LT61 isolated supplies. Everything else (MCU, USB, battery,
scope, generator) shares one ground.

## Main parts

![The opened DM40: main board, shield plate over the DMM area, back cover with battery](photos/opened-case-overview.jpg)

*The case opened: main board with the shield plate over the DMM area (fuse top left), MCU area below; back cover with foil lining and the battery.*

| Function | Part | Source |
|---|---|---|
| MCU | GigaDevice [GD32F427VGT6](datasheets/gigadevice-gd32f427xx-datasheet-rev1.9.pdf), Cortex-M4F, LQFP100 | marking `GD32F427 VGT6` in photo [main-board-mcu-gd32f427.jpg](photos/main-board-mcu-gd32f427.jpg); review; BOM |
| MCU crystal | 25.000 MHz | photo ([main-board-mcu-gd32f427.jpg](photos/main-board-mcu-gd32f427.jpg), crystal next to the SPI flash) and PLL read back ([`firmware/src/clock.rs`](../firmware/src/clock.rs)). The community BOM says 26 MHz (contradiction, see [Unknowns](#unknowns)) |
| SPI flash | Winbond [W25Q128JV](datasheets/winbond-w25q128jv-revI.pdf), 16 MiB | code ([`firmware/src/spiflash.rs`](../firmware/src/spiflash.rs), JEDEC check at boot); BOM. The BOM calls it QSPI: the chip can do quad I/O, but the board wires it as a plain 4-wire SPI ([SPI flash](#spi-flash-usb-disk)) |
| LCD controller | [ILI9488](https://www.crystalfontz.com/controllers/uploaded/ILI9488%20Data%20Sheet_100.pdf) (ID read 0x9488) | ID 0x9488 read by `board::init` ([`firmware/TODO.md`](../firmware/TODO.md) T1); stock init sequence `stock/re/analysis/lcd_buzzer_v2_6.h`. Not in the BOM |
| LCD panel | 3.5" IPS, 480 x 320, active area 74 x 50 mm | review |
| Touch | Hynitron CST-type controller at I2C 0x5A (chip type byte 0xA8) on this unit; the stock also supports a Goodix GT1158 at 0x14 | [`firmware/src/touch.rs`](../firmware/src/touch.rs); GT1158 path from the stock code only. Not in the BOM (the touch controller sits on the panel FPC) |
| Bluetooth | Jieli BP2A838-20A2, 24 MHz crystal, PCB meander antenna | BOM (custom Jieli chip, no public datasheet); antenna and 24.000 MHz crystal next to a chip with the Jieli logo in [main-board-mcu-gd32f427.jpg](photos/main-board-mcu-gd32f427.jpg) |
| DMM front end | Hycon [HY3131](datasheets/hycon-hy3131-datasheet-v10.pdf) | code (register protocol, `stock/re/analysis/hy3131_v2_6.h`); the chip itself is under the shield can (4.9152 MHz crystal and VDD/VSS silkscreen next to it in [dmm-isolation-hy3131-shield-area.jpg](photos/dmm-isolation-hy3131-shield-area.jpg)) |
| HY3131 crystal | 4.9152 MHz | photo and stock constant 4915.2 (`stock/re/analysis/hy3131_v2_6.h`) |
| Voltage reference | [MAX6190](https://www.analog.com/media/en/technical-documentation/data-sheets/MAX6190-MAX6198.pdf) (1.25 V, 5 ppm/°C) on HY3131 PB6 | BOM only for the part (placement: see [Unknowns](#unknowns)); the 1.25 V value matches the stock scaling ([`emu/hy3131/RESEARCH.md`](../emu/hy3131/RESEARCH.md) 7) |
| Divider network | TC1746B6 (B / C), 0.1 %, 10 ppm/°C; discrete resistors on the A | BOM only; chat reports [#25087](https://t.me/S2084K/25087), [#51830](https://t.me/S2084K/51830) ([`firmware/docs/user-feedback.md`](../firmware/docs/user-feedback.md) 12) |
| Digital isolator | Chipanalog [CA-IS3741HW](datasheets/chipanalog-ca-is374x-v1.08.pdf) | marking in photos [dmm-isolation-hy3131-shield-area.jpg](photos/dmm-isolation-hy3131-shield-area.jpg), [main-board-mcu-gd32f427.jpg](photos/main-board-mcu-gd32f427.jpg) |
| Isolated supplies | 2 x [VPS8504B](datasheets/vpsc-vps8504b-cn.pdf) push-pull driver + transformers LT61-117 and LT61-113; micropower (hundreds of mW, BOM) | markings in photos [isolated-supply-vps8504b-lt61.jpg](photos/isolated-supply-vps8504b-lt61.jpg), [dmm-isolation-hy3131-shield-area.jpg](photos/dmm-isolation-hy3131-shield-area.jpg) |
| Optocouplers | 2 x EL3H7, 2 x "174 F163H" | what they carry is *inferred*: the BOM guesses the HY3131 INT plus the relay and MOSFET drive; see [Unknowns](#unknowns). BOM; the "174 F163H" (4-pin) next to the relay and the EL3H7 (marked "D424 EL3H7") are in [dmm-isolation-hy3131-shield-area.jpg](photos/dmm-isolation-hy3131-shield-area.jpg) |
| DMM relay | Hongfa [HFD4/3-LS](datasheets/hongfa-hfd4.pdf) (DPDT, 3 V, latching per BOM) | marking in photos [input-jacks-shunts-fuse-relay.jpg](photos/input-jacks-shunts-fuse-relay.jpg), [relay-hfd4-and-varistor-closeup.jpg](photos/relay-hfd4-and-varistor-closeup.jpg); review |
| Input protection | varistors 07D681K (680 V), blue disc parts, 2 W wirewound/film resistors; FMMT619 pair (*inferred*, chat report [#41820](https://t.me/S2084K/41820)) | photos ([relay-hfd4-and-varistor-closeup.jpg](photos/relay-hfd4-and-varistor-closeup.jpg), [input-jacks-shunts-fuse-relay.jpg](photos/input-jacks-shunts-fuse-relay.jpg)); which disc is a PTC and which a MOV is *inferred* from the markings |
| Current shunts | 10 mohm (wire shunt), 0.5 ohm (SMD 2512 "R500"), 47 ohm (MELF 0207, *inferred*; uA shunt 0.5 + 47 = 47.5 ohm) | 0.5 ohm: marking in [input-jacks-shunts-fuse-relay.jpg](photos/input-jacks-shunts-fuse-relay.jpg); values from the stock scaling and opendm40 checks (below) |
| Current range MOSFETs | RUH30J51M dual N-MOSFET, 30 V / 50 A, and a second MOSFET marked RU30J30M (not in the BOM); switch the current ranges inside the one A jack | BOM for the RUH30J51M; both markings on the board bottom next to the A jack in [bottom-a-jack-mosfets-ru30j30m-ruh30j51m.jpg](photos/bottom-a-jack-mosfets-ru30j30m-ruh30j51m.jpg); what the RU30J30M switches is *inferred* |
| Fuse | ceramic cartridge in clips, next to the A jack | photo [input-jacks-shunts-fuse-relay.jpg](photos/input-jacks-shunts-fuse-relay.jpg) and [opened-case-overview.jpg](photos/opened-case-overview.jpg); one 10 A ceramic fuse for all shunts by the Alientek DM40 manual (not in the repo) |
| Scope ADC | 3PEAK [3PA1030](datasheets/3peak-3pa1030-reva1.pdf), 10 bit, 50 MSPS, parallel | BOM; 10-bit parallel bus on PB0..PB9 in code ([`firmware/src/scope.rs`](../firmware/src/scope.rs)) |
| Scope gain mux | [HC4051](datasheets/ti-cd74hc4051-schs122o.pdf) (header says "HCF4051") | BOM, `stock/re/analysis/scope_gen_v2_6.h` |
| Scope op amp | "8091" (Gainsil [GS8091](datasheets/gainsil-gs8091.pdf)?) | BOM only, after gadefox in the EEVblog thread |
| Generator path switch | SGM3157 SPDT | *inferred* from chat report [#24902](https://t.me/S2084K/24902); [`firmware/docs/dac-adc-paths.md`](../firmware/docs/dac-adc-paths.md) 1 |
| Charger | [TP4056](datasheets/toppower-tp4056-cn.pdf) (marked "TP4056 2413"), linear, 1 cell, not isolated (on VBUS) | marking in photo [charger-tp4056-and-power.jpg](photos/charger-tp4056-and-power.jpg); review |
| Battery | LN425272 Li-ion polymer, 3.7 V, 2000 mAh, 7.4 Wh, 4.2 V charge limit, made 2025-02-10 by 东莞市立能电子科技有限公司, with protection PCB | label in [battery-ln425272-label.jpg](photos/battery-ln425272-label.jpg) |
| Connectors | USB-C; two coaxial (MCX per BOM) jacks silkscreened CHA (scope) and GEN (generator); banana V/ohm, COM, A/mA/uA | photos; review ("coaxial, not BNC") |
| Debug header | two rows of 4 holes: "VCC RX TX GND" (TX = PA2, the boot strap) and "V D C G" (SWD: D = PA13 SWDIO, C = PA14 SWCLK; the BOM lists only this 4-pin service header) | see [Debug header and hardware recovery](#debug-header-and-hardware-recovery). Flash read protection is set (level 1 in the option bytes; `stock/re/ANALYSIS.md` 1) |

Small SOT-23 / SOT-89 / MSOP parts in [charger-tp4056-and-power.jpg](photos/charger-tp4056-and-power.jpg)
("A2SHB", "93N44", "XFHVB", "K14", "S1K", "NE.", inductor "2R2", tantalum
"J227") are not identified; they make up the power path and regulators.

## MCU and clocks

![MCU area of the main board: GD32F427VGT6, SPI flash, crystals, Bluetooth, CA-IS3741HW, USB-C, debug pads](photos/main-board-mcu-gd32f427.jpg)

*MCU area: GD32F427VGT6, SPI flash, 25 MHz crystal, Bluetooth chip with 24 MHz crystal and antenna, CA-IS3741HW, LT61 transformers, USB-C, debug pads.*

- [GD32F427VGT6](datasheets/gigadevice-gd32f427xx-datasheet-rev1.9.pdf): Cortex-M4F, 1 MB internal flash, 256 KB SRAM as 64 KB TCM
  (0x10000000) + 128 KB SRAM0/1 (0x20000000) + 64 KB ADDSRAM (0x20020000).
  (`stock/re/ANALYSIS.md` 4: dump sizes and stack top).
- HXTAL 25 MHz, PLL M=25 N=400 P=2 -> 200 MHz SYSCLK / HCLK; APB1 50 MHz,
  timer kernel clocks 200 MHz (TIMPRE). RCU_PLL read back on the device
  ([`firmware/src/clock.rs`](../firmware/src/clock.rs), [`firmware/src/genplan.rs`](../firmware/src/genplan.rs)).
- USB 48 MHz: the stock PLLQ gives 44.4 MHz; opendm40 runs PLLSAI (P=6 -> 48 MHz)
  for USBFS ([`firmware/src/usb.rs`](../firmware/src/usb.rs)).
- On-chip blocks the board uses: USBFS, two 12-bit DAC channels (both used,
  [Generator](#generator) and [Oscilloscope](#oscilloscope)), DMA, ADC0.
- Backup domain (RTC, backup SRAM): not powered while the meter is off. After
  a power-off by the key, the next boot shows the POR and BOR reset flags and
  the 4 KiB backup SRAM content is gone; a reset or a firmware update keeps
  it. opendm40 therefore keeps a copy of its settings on the SPI flash
  ([`firmware/src/setfile.rs`](../firmware/src/setfile.rs)). LSE (32 kHz crystal): not traced.

## SPI flash (USB disk)

[W25Q128JV](datasheets/winbond-w25q128jv-revI.pdf), 16 MiB, on SPI1 (GD32 naming; base 0x40003800): SCK PD3, MISO PC2,
MOSI PC3 (AF5), CS PE4 (GPIO, active low), mode 3. The stock runs it at
3.125 MHz, opendm40 at 12.5 MHz after a boot self-check. The whole chip is one
FAT16 volume with 4096-byte sectors: the USB disk of the meter
([`firmware/src/spiflash.rs`](../firmware/src/spiflash.rs), `stock/re/analysis/spiflash_v2_6.h`). The FS-loader also uses it for
updates.

## LCD

- [ILI9488](https://www.crystalfontz.com/controllers/uploaded/ILI9488%20Data%20Sheet_100.pdf), 16-bit 8080 bus on FSMC bank 1 NE1: D0..D15 on PD0 PD1 PD8 PD9
  PD10 PD14 PD15 PE7..PE15, NOE PD4, NWE PD5, NE1 PD7, RS = A16 on PD11
  (command 0x6001FFFE, data 0x60020000). No reset GPIO, no DMA
  ([`firmware/src/board.rs`](../firmware/src/board.rs), [`firmware/src/lcd.rs`](../firmware/src/lcd.rs), `stock/re/analysis/lcd_buzzer_v2_6.h`).
- FSMC timing as the stock: BCR1 0x5091, BTR1 0x0F04, BWTR1 0x0FF00201
  (device read-back, [`firmware/TODO.md`](../firmware/TODO.md) T1).
- Pixel format RGB565 on writes. A GRAM read returns one dummy word, then
  RGB666 at 3 bytes per pixel, packed big-endian into the 16-bit words
  ([`lcd.rs`](../firmware/src/lcd.rs)). The ID register 0xD3 reads `00 94 88` on this unit.
- Backlight: PA6 AF9 = TIM13_CH1, 20 kHz PWM ([`board.rs`](../firmware/src/board.rs)).
- Panel: 3.5" IPS 480 x 320 (review). The panel's FPC pinout is unknown.
- Tearing-effect output (TE): whether it reaches an MCU pin is unknown;
  *inferred* not wired. The stock init sequence has no TEON (0x35) and the
  stock drawing has no tearing control (`stock/re/analysis/lcd_buzzer_v2_6.h`,
  `gui_render_v2_6.h`); neither firmware configures a pin for it. The only
  candidates are the pins no code is known to use: PA3, PB13, PC1, PC15
  ([Pin table](#pin-table)). `DBG:TE?` ([`lcd.rs`](../firmware/src/lcd.rs)
  `te_probe`) counts the level changes of those four pins for 100 ms each
  with TE off, on in mode 0 (V-blank) and on in mode 1 (V- and H-blank),
  then sends TEOFF; it only reads the pins. A pin that changes only while
  TE is on (~60 Hz in mode 0, much faster in mode 1) is TE; floating pins
  can change in all three phases. Not run on a device yet.
- If TE is wired: with opendm40's MADCTL (MV = 1) the panel scans along
  our x axis (*inferred*; the scan direction is not checked), so writes
  synced to TE would have to go in vertical bands (columns of the
  landscape screen), not in the full-width strips of
  [`lcd.rs`](../firmware/src/lcd.rs) "strip rendering". Those strips
  write every pixel once in its final colour, so without TE a refresh
  during a page change shows the new page above a seam and the
  old page below it.

## Touch panel and touch keys

- I2C2 (GD32 naming; STM32 "I2C3", base 0x40005C00): SCL PA8, SDA PC9 (AF4),
  INT PE5 (falling edge), reset PE6 ([`firmware/src/touch.rs`](../firmware/src/touch.rs),
  `stock/re/analysis/touch_v2_6.h`).
- This unit: Hynitron CST-type at 7-bit 0x5A, chip type byte 0xA8. The stock
  probes a Goodix GT1158 at 0x14 first (absent here), so other units may carry
  it.
- The four touch keys left of the screen (MENU, RUN, AUTO, MF) are reported by the touch controller itself as key codes 0..3,
  not by GPIOs ([`touch.rs`](../firmware/src/touch.rs), measured X 372/628/884/1140).
- Pull-up strength unknown; opendm40 measures the rise time before going to
  400 kHz.

## Power key, PA2, buzzer

- **Power key PD6**: the one physical button (side). Input with pull-down,
  pressed = 1 ([`firmware/src/boot.rs`](../firmware/src/boot.rs) `PD6_PRESSED_HIGH`, stock
  FUN_0800b7e0). How the key also switches the unit on from off (it must reach
  the power path) is unknown.
- **PA2**: the "TX" pad of the debug header. The Alientek primary bootloader and the FS-loader enable a
  pull-up on PA2 and poll it ~1000 times each at boot; low for the whole poll
  sends the boot to the FS-loader's install state (Boot decision chain (`stock/re/docs/fsloader-install-trigger.md`)). PA2 is not the power key.
  See [Debug header and hardware recovery](#debug-header-and-hardware-recovery).
- **Buzzer**: PA7 AF9 = TIM14_CH1 PWM,
  200 MHz / 5 / 11200 = 3.57 kHz, loudness = duty
  (`stock/re/analysis/lcd_buzzer_v2_6.h`, opendm40 [`app.rs`](../firmware/src/app.rs)). The continuity / diode beep is not
  made by the MCU: the stock only toggles the [HY3131](datasheets/hycon-hy3131-datasheet-v10.pdf) CMP pin buffer (ENPCMPO),
  so the HY3131 comparator output drives the buzzer stage (*inferred*,
  [`firmware/docs/parity.md`](../firmware/docs/parity.md)). The only buzzer-like part in
  the photos is a magnetic buzzer in the DMM area next to the HY3131 can
  ([dmm-isolation-hy3131-shield-area.jpg](photos/dmm-isolation-hy3131-shield-area.jpg)); how PA7 reaches it across the
  isolation is unknown.

## Debug header and hardware recovery

**Where.** At the end of the main board opposite the banana jacks, next to
the Bluetooth antenna and the 24 MHz crystal: two rows of four through-holes,
unpopulated, silkscreened. Seen with the board as in
[opened-case-overview.jpg](photos/opened-case-overview.jpg) (jacks at the top), left to right:

| Group | Pads, left to right | Square pad |
|---|---|---|
| left | G, C, D, V | V |
| right | GND, TX, RX, VCC | VCC |

![Debug header: TX (PA2) and GND marked](photos/debug-header-annotated.jpg)

*A marked-up crop of [main-board-mcu-gd32f427.jpg](photos/main-board-mcu-gd32f427.jpg)
(photo © CTAHOK, mysku.club, see [photos/README.md](photos/README.md)).*

| Pad | Signal |
|---|---|
| TX | **PA2** (pull-up enabled by the bootloader and the FS-loader) |
| GND | MCU-domain ground, next to TX |
| RX | **PA3** (USART1_RX on the GD32F4; unused by the stock firmware and opendm40) |
| VCC | ~3.1 V while the meter is on (measured); **not** wired directly to the MCU supply; what sits in between is not traced |
| V, D, C, G | SWD: V = VCC, D = **PA13** (SWDIO), C = **PA14** (SWCLK), G = GND |

SWD itself has not been tried. The option bytes (dumped from the running
firmware, `stock/re/ANALYSIS.md` 1) show read protection level 1:
with it a debugger cannot read or write the flash, and lifting it
mass-erases the whole MCU flash: the Alientek bootloader, the FS-loader, the
calibration and the UID-bound boot data. Use SWD only as a last resort, after
the opendm40 backup ([the manual, Back up your meter](../docs/manual/README.md#back-up-your-meter)),
and restore in the order given in its `INFO/BACKUP.TXT`.

PA2's alternate function on the GD32F4 is USART1_TX, so the pad is labelled
as a UART TX. The stock firmware never enables USART1 (per the RE notes), and
neither does opendm40: in practice the pad is only the boot strap.
PA2 reads low only when it is shorted to GND, or when something connected to
the header pulls it low.

### Hardware recovery

It works whatever the application does, because the decision is taken by the Alientek
bootloader and the FS-loader before the application runs.

1. Switch the meter off. Open the case (back cover) to reach the header.
2. Short the **TX** pad (PA2) to the neighbouring **GND** pad (a tweezers or a
   wire), then switch on with the power key.
3. Keep the short until the update screen appears ("open file error" and the
   FS-loader's own USB disk on the PC).
4. Copy an update file onto that disk under a `dm40*.bin` name: the official
   [`dm40_v_2_6.bin`](../stock/vendor/dm40_v_2_6.bin) from [`stock/vendor/`](../stock/vendor) in this repo (back to stock) or an opendm40 image (e.g. `dm40_opendm40_v0.1.bin`),
   then press "update" on the screen.
5. Remove the short.

Untested: whether the short also works when applied during a reset. It must
hold through both polls: the ROM's 1000 reads, then the FS-loader's
(about 0.5 s each in the emulator; any high read launches the application).

opendm40 does the same in software: `DBG:LOADER` and the power-key hold at
power-on drive PA2 low and lock the pin before jumping to the bootloader
([native-boot.md, PA2 entry](../firmware/docs/native-boot.md#pa2-entry-bootloader_pa2)).
The short is needed only when opendm40 hangs before its shim reads the key;
the user steps are in [the manual, Reach the update loader](../docs/manual/README.md#reach-the-update-loader).

## USB

USB-C, USBFS only: DM PA11, DP PA12 (AF10). No VBUS sensing (PA9/PA10, the
usual VBUS/ID pins, drive the relay) ([`firmware/src/usb.rs`](../firmware/src/usb.rs)).
GSNPSID reads 0 on this GD32; GRSTCTL.AHBIDL shows that the USBFS core is
clocked. A core reset keeps the D+ pull-up, so the host sees no disconnect:
the firmware drives PA11 / PA12 low for 100 ms to force one. The USB port
also charges the battery ([TP4056](datasheets/toppower-tp4056-cn.pdf)). USB data and the DMM are galvanically
separate ([Isolation boundary](#isolation-boundary)); USB ground is the MCU,
scope and generator ground (*inferred*).

## Bluetooth

- Module / chip: Jieli BP2A838-20A2 (BOM and the photo), own 24 MHz crystal, PCB meander antenna at the board edge
  ([main-board-mcu-gd32f427.jpg](photos/main-board-mcu-gd32f427.jpg)). Classic SPP (RFCOMM channel 1,
  [`docs/bt-protocol.md`](../docs/bt-protocol.md)) and BLE ([`firmware/src/bt.rs`](../firmware/src/bt.rs)).
- UART4: TX PA0, RX PA1 (AF8), 115200 8N1, no RTS/CTS ([`bt.rs`](../firmware/src/bt.rs)).
- PC0 = module power, **low = on**, high = off ([`bt.rs`](../firmware/src/bt.rs),
  `stock/re/analysis/bt_meas_v2_6.h`; [`board.rs`](../firmware/src/board.rs) sets it high at boot = off, as the stock).
- PC14 = link status input, high = connected ([`power.rs`](../firmware/src/power.rs), [`bt.rs`](../firmware/src/bt.rs)).
- AT commands the firmware sends: `AT+SBN<name>`, `AT+SLED0`, `AT+RESET`,
  `AT+CBN` ([`bt.rs`](../firmware/src/bt.rs), [`docs/bt-protocol.md`](../docs/bt-protocol.md)).

## DMM front end (HY3131)

Overview of the chip: [emu/hy3131/RESEARCH.md](../emu/hy3131/RESEARCH.md) 2
and the datasheet in [datasheets/](datasheets/README.md). The 60 stock
configuration rows (R20..R35 per mode and range) and their decoding are in
`stock/re/analysis/hy3131_v2_6.h`; the terminal-level model is
[emu/hy3131/README.md](../emu/hy3131/README.md).

**Bus.** SCK PC10, MISO PC11, MOSI PC12, CS PD2 (active low), mode 0. The stock
bit-bangs it; opendm40 uses SPI2 (AF6) at 3.125 MHz
([`firmware/src/hy3131.rs`](../firmware/src/hy3131.rs), [`firmware/docs/dmm-input-scope-feasibility.md`](../firmware/docs/dmm-input-scope-feasibility.md) 8). The
signals cross the [CA-IS3741](datasheets/chipanalog-ca-is374x-v1.08.pdf) isolator (the channel assignment is *inferred*: it has 3 forward and 1
reverse channel, which fits SCK, MOSI, CS out and MISO back; datasheet
[`chipanalog-ca-is374x-v1.08.pdf`](datasheets/chipanalog-ca-is374x-v1.08.pdf)). The [HY3131](datasheets/hycon-hy3131-datasheet-v10.pdf) IRQ output is not used.

**Converters** (datasheet facts, [`RESEARCH.md`](../emu/hy3131/RESEARCH.md) 2; SYSCLK = 4.9152 MHz):

| Block | What | Rates on the DM40 |
|---|---|---|
| AD1 | 24-bit sigma-delta, IG 0.9..3.6, reference PB6 (1.25 V) or internal | AD1CLK = SYSCLK / 30; OSR 256..32768. Stock steady 32768, moving 8192 ([`firmware/src/dmm.rs`](../firmware/src/dmm.rs)) |
| AD2 / AD3 | 19-bit fast converters, shared clock and OSR | 76.8 kS/s in the stock AC rows (SYSCLK / 2 / 32); ladder down to 1200 S/s; chopping (ENCHOPAD2) does not change the rate (one unit, device f80524f; [`dmm-input-scope-feasibility.md`](../firmware/docs/dmm-input-scope-feasibility.md) 1.1) |
| RMS / LPF | digital true RMS (sum X^2 or X*Y) and low-pass of AD2 | ~106.7 ms per result measured with LPFBW 111 (half the datasheet formula, [`RESEARCH.md`](../emu/hy3131/RESEARCH.md) 8) |
| Peak hold | on AD2, AD1<23:5> or LPF | used by opendm40's A-jack probe ([`firmware/src/sentinel.rs`](../firmware/src/sentinel.rs)) |
| Comparator | window comparator, thresholds from R21; output CMPO / CMP pin | continuity beeper, capacitance ramp, frequency, opendm40 sentinels |
| Counter | CTA / CTB / CTC, gate preset 0xFC0000 = 0x40000 SYSCLK = 53.3 ms | frequency (reciprocal), capacitance (RC oscillator) |
| Op amps | OP1, OP2 (2 MHz), OP3 (ohm / diode source) | x1 / x10 gain stages on the shunt and mV paths |

AD1 on a periodic input aliases (the "AD1 beat"): the sinc² comb and its blind spots per OSR are in [`firmware/docs/dmm-rate-filter-survey.md`](../firmware/docs/dmm-rate-filter-survey.md#13-the-owners-observations-explained) 1.3, the measured response in [`device-check-2026-10-21.md`, "AD1 OSR characterization"](../docs/history/device-check-2026-10-21.md#ad1-osr-characterization-ea73ea5-2026-10-24-1433---1545).
AC bandwidth is about 20 kHz (-3 dB, one rough chat measurement, [#16732](https://t.me/S2084K/16732)).
The AC V path has a coupling network into PB4 with τ = 0.34 s (one unit; R · C only) and the
FTP-FTN pre-filter capacitor is ~26 nF; the stock AC V rows' FS9 switch shapes the HF response, the ACC
array does nothing measurable ([`firmware/docs/hy3131-frontend-2026-10-26.md`](../firmware/docs/hy3131-frontend-2026-10-26.md)).

**MCU GPIOs on the DMM side** (the complete list the stock drives,
`stock/re/analysis/hy3131_v2_6.h` lines 14-18, [`firmware/src/dmm.rs`](../firmware/src/dmm.rs) `relay_pa` / `relay_pe`):

| MCU pin | Function | Notes |
|---|---|---|
| PA9 / PA10 | [HFD4/3-LS](datasheets/hongfa-hfd4.pdf) latching relay, 10 ms pulse. Arg 0: PA10 = 1, PA9 = 0; arg 1: PA10 = 0, PA9 = 1 | V/A path vs the ohm / mV network. One latching relay with a single coil driven in either polarity is *inferred* from the drive pattern (one relay in the photos; the BOM says DPDT, 3 V, latching); the drive crosses the isolation (optocouplers, *inferred*) |
| PE0 / PE1 | shunt MOSFETs: PE0 = 1 -> A (10 mohm), PE1 = 1 -> mA (0.5 ohm), both 0 -> uA (47.5 ohm chain) | the resting state in every non-current mode is both 0 (the stock rows in [`hy3131_tables.rs`](../firmware/src/hy3131_tables.rs); a user report, [#59918](https://t.me/S2084K/59918)) |
| PC7 | 1 only in the frequency mode (mode 14) | what it switches is unknown |

![HFD4/3-LS relay and a 07D681K varistor](photos/relay-hfd4-and-varistor-closeup.jpg)

*Input network: HFD4/3-LS relay and a 07D681K (680 V) varistor.*

**Input network (HY3131 pins, *inferred* from the stock rows and scaling):**

- V/ohm input through a 10 M divider with taps: /10 on HY3131 PA7, /100 PA6,
  /1000 PA5, /10000 PA4 + PA5; the buffered input appears as SENSE
  ([`power-meter-and-misconnect.md`](../firmware/docs/power-meter-and-misconnect.md) 3.1, [`RESEARCH.md`](../emu/hy3131/RESEARCH.md) 7). The taps follow the
  datasheet's 10 M / 1.111 M / 101.01 k / 10.01 k / 1 k network.
- Ohm: OP3 drives reference resistors on HY3131 PA2..PA7 (Rref 1 k .. 63 M per
  range); ratiometric AD1 (`stock/re/analysis/hy3131_v2_6.h` mode 10). Ohm source
  short-circuit current per range (600 Ω / 6 k / 60 k / 600 k / 6 MΩ):
  0.90 mA / 197 µA / 20 µA / 2.2 µA / 215 nA in the default variant (OP3
  VREF tap VDS15), 0.28 mA / 58 µA / 6.6 µA / 0.66 µA / 86 nA in the "ON"
  variant (AGNDP7; AGNDP8 on 6 MΩ). Open-circuit voltage at the jack (the
  MR9270S+ calibrator's voltage input): 2.12 V (default), 0.67 V (ON). Measured
  on one unit with `DBG:SRC`, jacks shorted / open
  ([`device-check-2026-10-21.md`](../docs/history/device-check-2026-10-21.md#ohm-test-current-map-dbgsrc),
  `dmm::OHM_ISC` in [`firmware/src/dmm.rs`](../firmware/src/dmm.rs)).
  60 Ω and 60 MΩ (the current-source row) not measured; the firmware shows
  the 600 Ω default value on 60 Ω (*inferred*). *Inferred* from the drops:
  only 600 Ω is limited by OP3; above it the ~2 V source behind Rref sets
  the current. Diode test 0.756 mA, open-circuit 3.23 V (measured on one unit, the RLD current-source path; device check).
- Diode: OP3 source with the 900 k / 100 k divider on HY3131 PB0 / PB8.
- Capacitance: RC relaxation (R = 1020 k .. 1.5 k) or constant-current ramp
  switched on HY3131 PA4..PA7.
- mV: OP1+ on RLU (the stock DC mV rows), OP1 x10 on 60 mV, x1 on 600 mV,
  AD1 = OP1O - AGND; RLU as the jack's mV node is *inferred*. Noise and gain
  options: [`hy3131-frontend-2026-10-26.md`](../firmware/docs/hy3131-frontend-2026-10-26.md)
  section 4.
- Thermocouple: OP1+ on HY3131 PB0 (the stock temperature rows, R32 0x3C), OP1
  x10 on r0. Cold junction = the HY3131 die sensor.
- AC V: OP1 output -> external coupling capacitor -> HY3131 PB4 -> AD2 (the capacitor is *inferred* from the
  exponential decay at PB4 and the Hycon demo board's C16 / R2, APD-DMM001 p.22).
  The coupling network's time constant is 0.34 s (one unit, a DC step at PB4); R and C separately are
  not known (the demo board: 1 µF and 1 MΩ to ground, 1 s).
- AD1 pre-filter: an external capacitor between HY3131 FTP and FTN, ~26 nF (one unit: τ = 2.56 ms
  with the on-chip 100 k, SFT1 = 00; the demo board fits 27 nF). The stock rows set SFT1 = open in
  V / A / Hz and most capacitance rows, 10 k in the first DC mA / µA row (CFG_F8_9 r0), 0 Ω in ohm
  60 Ω..600 kΩ, continuity, diode and the two last capacitance rows, 100 k in ohm 6 MΩ / 60 MΩ.
- Divider switches in the stock V rows (R2A..R2E, `hy3131_tables.rs` CFG_F0 / CFG_F1):

  | row | r0 | r1 | r2 | r3 |
  |---|---|---|---|---|
  | AC V | FS1, PS6 SS6, FS9 PS8 | as r0 | FS1, PS5 SS5, FS9 PS8 | FS1, PS5 SS5 PS4 SS4, FS9 PS8 |
  | DC V | DS0 FS0, PS7 SS7, FS9 PS8 | DS0 FS0, PS6 SS6, DS9 FS9 PS8 | DS0 FS0, PS5 SS5, DS9 FS9 PS8 | DS0 FS0, PS5 SS5 PS4 SS4, DS9 FS9 PS8 |

  AC V r0 and r1 share the tap and differ in the OP1 gain (R25). What the PA9 network is on the
  DM40 is not known (APD-DMM003 §4 shows 30 k + 5 pF on PA9 for the AC V compensation). Clearing
  FS9 in the AC V rows raises the HF response by the same amount on the 6 V and 600 V taps
  (+3.5 % RMS of a square at 5 kHz, +17 % at 20 kHz), so its element does not load the tap
  (*inferred*); the ACC array (R30, SMODE5 set in the AC V rows) changes nothing measurable
  ([`hy3131-frontend-2026-10-26.md`](../firmware/docs/hy3131-frontend-2026-10-26.md)).

![DMM input area: relay, varistors, R500 shunt, wire shunt, fuse, COM and A jacks](photos/input-jacks-shunts-fuse-relay.jpg)

*DMM input area under the shield plate: relay, input resistors and varistors, 0.5 ohm "R500" shunt, wire shunt between the jacks, fuse in clips, COM and A/mA/uA jacks.*

**Current path.** One A jack for A / mA / uA (silkscreen "A/mA/uA"); the
MOSFETs switch the range, not the user (chat report [#26977](https://t.me/S2084K/26977)). Series chain
from the A jack: fuse -> 10 mohm -> 0.5 ohm -> 47 ohm -> COM, with PE0 / PE1
MOSFETs shorting the lower parts (*inferred* from the mode table;
[`sentinel.rs`](../firmware/src/sentinel.rs) fuse check, [`power-meter-and-misconnect.md`](../firmware/docs/power-meter-and-misconnect.md) 4.1 / 4.8).
Measurement nodes: HY3131 PB2 = top of the 10 mohm element (every A-jack
current passes it), PB1 = the node between 10 mohm and 0.5 ohm, PB3 = the low
side at COM (*inferred*: with PE1 shorting the 47 ohm resistor, PB1 - PB3
reads 0.5 ohm in mA and 0.5 + 47 = 47.5 ohm in uA, the stock's two shunt
values). Shunt values: 0.5 ohm from the "R500" marking; 10 mohm and
47.5 ohm (0.5 + 47) *inferred* from the stock scaling and the opendm40 fuse check, which
reads ~48 ohm A-COM through the whole chain on the device
([`sentinel.rs`](../firmware/src/sentinel.rs)). The 10 mohm element is the bent
copper-coloured wire bridge between the jacks in
[input-jacks-shunts-fuse-relay.jpg](photos/input-jacks-shunts-fuse-relay.jpg) (*inferred*).

| Range | Shunt | Burden at full scale (*inferred*) |
|---|---|---|
| 10 A | 10 mohm | 0.2..0.5 V incl. fuse and MOSFET |
| mA | 0.5 ohm | 0.24..0.6 V |
| uA | 0.5 + 47 = 47.5 ohm | ~0.19 V |

The 47 ohm part is the blue MELF resistor between "R500" and the fuse in
[input-jacks-shunts-fuse-relay.jpg](photos/input-jacks-shunts-fuse-relay.jpg) (*inferred* from its position).
Scaled against the 2512 "R500" next to it (6.4 x 3.2 mm), its body is about
5.5 x 2 mm: a MELF 0207, a size usually rated 0.4..1 W depending on the
series (*inferred*). Its colour bands read yellow-violet-black-brown, i.e.
47 ohm 1 % (*inferred* from the photo). With the 0.5 ohm above it this gives
the stock's 47.5 ohm uA shunt.

**No jack sensing.** The DM40 has no jack sense switches or pins: the stock
DMM GPIO list is the one above. opendm40 detects leads in the wrong jack only
through the shunt node (HY3131 PB1 comparator) and the divider tap
([`sentinel.rs`](../firmware/src/sentinel.rs), [`power-meter-and-misconnect.md`](../firmware/docs/power-meter-and-misconnect.md) 4.1). A blown fuse cannot be
seen by the meter (the A side of the fuse has no path into the chip); opendm40's
"Fuse check" measures A-COM in ohm mode instead.

### Measured node behaviour

What the HY3131 nodes and blocks did on one DM40A with opendm40 rows
(the commit is the firmware that ran; the A jack was never wired, the
MR9270S+ calibrator was on V-COM). The firmware rules built on these are in
[`power-meter-and-misconnect.md`](../firmware/docs/power-meter-and-misconnect.md)
section 4 (sentinels) and 3 (power meter).

- **AD2 and AD3 on the same node** differ by ~0.1 % in gain: sum X^2 = 9919,
  sum Y^2 = 9930 in plain AC V r1
  (`stock/re/docs/history/acdc-stock-vs-opendm40.md` 10.1).
- **V on OP2 x1 -> AD3** (the power row: the divider tap through OP2 instead
  of OP1), against the stock AC V / DC V on the same input (device e606771,
  the power meter's T1 test; 40 Hz square 0..9.6 V and DC 1 V / 5 V):

  | range | AC part vs stock AC V | DC part vs DC V |
  |---|---|---|
  | 6 V | -2.27 % (the square's 0.96 V peak at the tap clips) | slope -2.05 % |
  | 60 V | -1.63 % | -1.62 %, slope -1.67 % |
  | 600 V | -0.33 % | -0.32 % |

  The AC and the offset-corrected DC part agree to ~0.1 % on every range and
  every square frequency: a constant per-range gain of tap + OP2 + AD3. The
  LPF zero is ~1080 codes on AD3 (~5 mV at AD3, a phantom DC of 0.05 / 0.5 /
  5 V on 6 / 60 / 600 V) and ~888 codes on AD2 in the x10 current ranges. AD3IN = RLU reads
  the same gain, AD3IN = PB5 0.58 % lower.
- **The RMS block's high-pass filter cannot be bypassed** (device d150919):
  power row, AC, 6 V range, 1.000 V DC on V-COM, R34 frozen at YY. Each
  reserved bit set alone (R34 bits 6, 1, 0; R26 bit 6; R35 bits 0, 1) left
  RMS (sum Y^2 / N) at 1803-1863 codes (the noise; 1830 without a poke) and
  LPF at 21589-21600 codes (the DC); a bypass would give ~21592^2 = 4.7e8.
- **CTSTA** took only 0x00, 0x10, 0x50 and 0x70 over ~233 000 `DBG:SENT?`
  reads, never 0x20 or 0x60 (device 40728b8): CMPHO never comes without
  CMPLO. *Inferred* reading: CMPHO = node above VRH, CMPLO = node above VRL,
  ACPO (bit 6) a latch set above VRH and reset below VRL; 0x70 = above the
  window, 0x50 = inside coming from above, 0x10 = inside coming from below,
  0x00 = below. It fits DC V's V-tap window (+1 V: 0x10, +5 V: 0x70). It
  does not fit PB1 (2026-10-27): with PB1 at ~2 mV (AD1, below) the PB1
  comparator reads 0x00 in the ±20.8 / 41.7 / 62.5 mV windows (R21 0xdd /
  0xcc / 0xbb), and 0x10 with nothing connected whenever the low threshold
  is AGNDN4..6 (0xaa, 0x99, 0x88, 0xba); AGNDP4..6 over AGNDN3 (0xab, 0x9b,
  0x8b) read 0x00. What the bits mean is *unknown*.
- **Both bits in one read (0x70)** also mark an overdriven comparator: on
  PB1 in the ohm rows with 5 V or 10 V DC or the 0 / 9.5 V square on V-COM
  (1 V and 3 V: no hit), on DC V's V tap with a steady 5 V, and on SENSE in
  DC A with 24 V on V-COM (devices 1a11eda, f540f0d). After leaving ohm or a
  chip restart the A rows' SENSE comparator reads 0x70 for ~4.4 s, then 0x50
  (device 469f354); after the DMM rail came back from the Scope page, 0x70
  for 7.75 s (device 6bc494a).
- **PB1 follows the V side** in rows that leave it to the input network:
  - ohm rows: the PB1 comparator trips with 5 V or 10 V on V-COM, not up to
    3 V (519fb8a); the ohm row's input buffer OP1O then peaks at 2.14 V (5 V,
    60 MΩ row), 2.25 V (5 V, 600 Ω), 2.02-2.03 V (10 V), 2.13 V (the square)
    and does not follow the source (1a11eda; why is unknown, *inferred*:
    clipped or limited);
  - continuity: the AD2 peak on PB1 read -97 000..+123 000 (~±230 mV at PB1)
    at 5 V DC and ±400 at 1 V (40728b8);
  - frequency: the AD1 peak on PB1 - PB3 (the AC rows' settings) stands at
    ~+41 100 codes (~54 mV) with 0..2 V or nothing on V-COM, ~3 100 at 5 V,
    -850 at 10 V (d8e249d); the row leaves the input divider open (R2C / R2D
    0). That row also keeps R33 SAD1I = 11 (AD1 on the temperature sensor
    inputs), so these codes are *inferred* not to be PB1: with SAD1I = 00
    PB1 - PB3 reads 2.5 mV at rest and -59 / -89 mV at 3 / 3.7 V (table
    below);
  - DC V 60 / 600 / 1000 V: the comparator on PB1 tripped for as long as
    10 V was on V-COM, never on 6 V at 1 V (f80524f). Those rows drive the
    power bus from OP3 (R2F SMODE 0100, PS6 + PS8 closed); the 6 V row leaves
    it off. With that comparator row back (`DBG:SENT:VAR 4`) CTSTA reads 0x50
    at every R21 from ±20.8 to ±417 mV, also with V-COM open, while AD1 reads
    PB1 at 2.1 mV up to 3.7 V (2026-10-27): the trip is not PB1 pickup.
- **PB1 with AD1** (2026-10-27, A jack open, manual ranges; AD1 on PB1 -
  PB3, then PB1 - AGND, IG 0.9, REFO / AGND, SAD1I = 00, in the row the
  function runs; branch `exp/pb1-meas`, `DBG:SENT:PB1?`). mV, PB1 - PB3 /
  PB1 - AGND, by the V-COM DC level (>1389 = AD1 at its limit):

  | row | V-COM off | 1 V | 3 V | 3.7 V | 5 V | 10 V |
  |---|---|---|---|---|---|---|
  | DC V 6 V | 2.2 / 2.2 | 2.2 / 2.2 | 2.2 / 2.2 | 2.2 / 2.2 | 2.2 / 2.2 | 2.2 / 2.2 |
  | DC V 60 / 600 / 1000 V | 2.1 / 2.1 | 2.1 / 2.1 | 2.1 / 2.1 | 2.1 / 2.1 | 2.1 / 2.1 | 2.1 / 2.1 |
  | DC mV 600 / 60 mV | 2.2 / 2.2 | 2.2 / 2.2 | 4.1 / 11.7 | 67 / 240 | 350 / 652 | 1015 / >1389 |
  | AC V 6 / 60 V | 1.4..3.0 | 1.4..3.0 | 1.4..3.0 | 1.4..3.0 | 1.4..2.7 | 1.4..1.5 |
  | AC mV 600 mV | 1.5 / 1.7 | 1.4 / 2.6 | -48 / 109 | -74 / 170 | -94 / 235 | -112 / 358 |
  | ohm 600 Ω | 2.2 / 2.2 | 2.2 / 2.2 | 2.5 / 2.4 | 4.1 / 4.0 | 238 / 353 | 961 / 1289 |
  | ohm 6 kΩ | 2.2 / 2.2 | 2.2 / 2.2 | 2.3 / 2.8 | -23 / 155 | 191 / 538 | 818 / 1332 |
  | ohm 60 MΩ | 2.2 / 2.2 | 2.2 / 2.2 | 4.6 / 12.7 | 62 / 264 | 357 / 698 | 1000 / >1389 |
  | temperature (TC) | 2.2 / 2.2 | 2.2 / 2.2 | -0.7 / 6.2 | 48 / 174 | 319 / 550 | 999 / 1340 |
  | Hz | 2.5 / 1.8 | 1.8 / 1.8 | -59 / 64 | -89 / 108 | -116 / 164 | -140 / 291 |
  | capacitance (AUTO) | 2.1 / 2.0 | 2.1 / 2.0 | -42 / 5.0 | 3.8 / 4.5 | -148 / 11 | -242 / 22 |

  The difference of the two is PB3 - AGND: it follows the input too (Hz at
  10 V: +431 mV, DC mV at 5 V: +302 mV). The ohm, DC mV and temperature
  rows read 0x70 on the PB1 comparator at 5 and 10 V with R21 0xdd, 0xcc,
  0xbb and 0x8b alike; DC V 6 V reads 0x00 at 10 V.
- **OP1's chopper** (OP1CHOP = 10, 2 kHz in the stock AC V row, with AD1 off)
  starts when AD1 is turned on in that row: a 2 kHz square of OP1's offset
  (x10 in 6 V) at OP1O -> PB4 put a constant ~37 mV RMS floor on AC V 6 V
  (f80524f). *Inferred*: the chopper clock runs only while AD1 converts (the
  stock pairs them in DC V 6 V and DC A).
- **SENSE and PB5 in the A rows are not driven** (*inferred*: those rows run
  the power bus as SGND with PA8 P):
  - PB5 is defined only by the DC V 60 V+ rows (their power bus / OP3). In
    the AC A rows AD1 on SENSE - PB5 ran from ~-117 000 codes at 0.7 s to the
    negative rail (-262 144) by ~2 s with V-COM open, 1, 5, 10 or 24 V alike
    (40728b8);
  - AD1 on SENSE - AGND (the /1000 tap through SS5) sat at the negative rail
    (below -347 mV at IG 3.6) from ~0.9 s with 1 V or the source off and
    ramped there over ~3 s with 24 V DC, 9 000-12 000 codes per 107 ms peak
    hold (d2aff22);
  - the DC A comparator on SENSE reads 0 (below VRL: SENSE > ~21 mV low) with
    V-COM open; 24 V on V/Ω is 24 mV at the /1000 tap against the 20.8 mV
    threshold (40728b8). After a DC V 60 V+ row, an ohm-network row or a chip
    restart it missed 24 V in DC A, with R20..R37 reading back the same in
    every entry; 100 / 300 / 1000 ms of the temperature row before DC A
    brought the trip back at 1.9 / 2.1 / 2.8 s (d2aff22). *Inferred*: board
    state (the divider and its capacitors, the power bus / PA8 node, the relay).

Sources: [device-check-2026-10-21.md](../docs/history/device-check-2026-10-21.md)
sections by commit ("f80524f", "d8e249d", "519fb8a", "469f354", "6bc494a",
"40728b8", "d2aff22", "1a11eda: bug-fix recheck"); e606771 (T1) and d150919
(HPF) are recorded here only.

## Oscilloscope

- ADC: [3PA1030](datasheets/3peak-3pa1030-reva1.pdf), 10 bit, data on PB0..PB9 read as GPIOB IDR by DMA1, clocked by
  TIM8_CH1 on PC6 (AF3) at 50 MHz ([`firmware/src/scope.rs`](../firmware/src/scope.rs)).
- Sample rates: 50 MS/s down to 50 S/s (timer-paced DMA from the 50 MHz
  stream). The ADC never runs faster than 50 MS/s; at 100 and 200 ns/div the
  display upsamples. Record 26 K samples ([`scope.rs`](../firmware/src/scope.rs) TB table). Spec 10 MHz bandwidth,
  10 mV..10 V/div (x1), 100 ns..50 s/div (review); chat reports measure flat to
  ~3 MHz and -3 dB at 11.5..17 MHz, input capacitance ~30..35 pF
  ([`user-feedback.md`](../firmware/docs/user-feedback.md) 6.3).
- Front end (meaning of the pins *inferred* from the gain steps,
  `stock/re/analysis/scope_gen_v2_6.h`, [`dac-adc-paths.md`](../firmware/docs/dac-adc-paths.md) 3):
  - PC13: input attenuator relay, 1 = direct, 0 = /50; 200 ms settle.
  - PB15 / PD12 / PD13: 4051 select code, gain x0.5 .. x20 in the amplifier
    feedback (which pin is A / B / C is unknown).
  - PE3: coupling, 1 = DC, 0 = AC.
  - PC8 = TIM3_CH3 (AF2) PWM ~10 kHz: vertical offset.
  - Trigger (stock only): DAC channel 2 on PA5 sets the level of an external
    comparator (chip not identified) whose output goes to PA15 = TIM2_CH1
    (AF1); the output is inverted and ~2 us late ([`firmware/src/hwtrig.rs`](../firmware/src/hwtrig.rs)).
- Supply: PC4 rail (below). Ground: shared with USB and the MCU (*inferred*).
- Connector: coaxial "CHA" (MCX per BOM); probe x1/x10 with compensation
  trimmer.

## Generator

- DAC path: DAC channel 1 (`DAC_OUT0`) on PA4, paced by TIM6 TRGO,
  DMA0 stream 5. Up to 10 MS/s. Output 0..3 V unipolar (DAC
  reference 3.0 V, assumed in code); the stock turns the DAC output buffer
  off for sine / triangle / ramp, so an external buffer stage follows
  (*inferred*) ([`firmware/src/gen.rs`](../firmware/src/gen.rs), `stock/re/analysis/scope_gen_v2_6.h`).
- PWM path (square above 50 kHz): TIM12_CH1 on PB14 (AF9), 200 MHz / D.
- PB12 selects the path: 1 = PWM, 0 = DAC; the switch is the SGM3157
  (*inferred* from a chat report). No attenuator or relay after it.
- PA4 can be read back by ADC0 channel 4 (`DIAG:DAC?`).
- Connector: coaxial "GEN". No protection: 12 V on the output killed the
  switch in one chat report ([#24902](https://t.me/S2084K/24902)).
- Spec (review): sine / triangle / sawtooth 1 Hz..50 kHz, square up to 10 MHz,
  0.5..3 V.

## Power and battery

![Power section: TP4056 charger, VBAT connector, USB-C, regulators](photos/charger-tp4056-and-power.jpg)

*Power section: TP4056 charger, VBAT connector, USB-C, unidentified regulators, 2R2 inductor.*

![Battery label LN425272 3.7 V 2000 mAh](photos/battery-ln425272-label.jpg)

*Battery label: LN425272, 3.7 V, 2000 mAh, 7.4 Wh, made 2025-02-10.*

- Battery: 1-cell Li-ion polymer LN425272, 2000 mAh, with protection PCB,
  2-pin connector "VBAT" (photo). Runtime 10..15 h (chat reports; review:
  12 h DMM / 10 h scope).
- Charger: [TP4056](datasheets/toppower-tp4056-cn.pdf), linear, from USB VBUS. Charging takes ~2 h 35 min (chat
  report [#16625](https://t.me/S2084K/16625)). Status: PB10 = input power present, PB11 = STDBY
  (*inferred*, see the pin table and [Unknowns](#unknowns) 5).
- Battery voltage: PC5 = ADC0 channel 15, 1:2 divider, 3.0 V reference
  assumed: Vbat = raw * 3.0 / 4096 * 2. Firmware powers off below ~3.31 V.
  ([`firmware/src/power.rs`](../firmware/src/power.rs), `stock/re/analysis/dmm_v2_6.h`); the divider ratio is
  *inferred* from the scaling.
- Rails switched by the MCU ([`board.rs`](../firmware/src/board.rs), [`scope.rs`](../firmware/src/scope.rs) `rails`,
  [`boot.rs`](../firmware/src/boot.rs)):
  - **PE2** = DMM analog supply (the isolated side, via a [VPS8504B](datasheets/vpsc-vps8504b-cn.pdf), *inferred*)
    and part of the power hold.
  - **PC4** = scope / generator analog supply.
  - The Alientek primary bootloader raises PE2 and PC4 first thing on every
    reset; the stock power-off drops both and the unit switches off once the
    key is released (on battery). Which of the two, or both, keeps the system
    supply on is unknown.
  - PC0 = Bluetooth module power (low = on).
- MCU die temperature: ADC0 channel 16 (internal sensor), typical constants.

## Pin table

GD32 peripheral names (UART4, SPI1, SPI2, I2C2, DMA0 / DMA1) follow the
GD32F4xx user manual as the opendm40 sources do. The source column names the
opendm40 or stock code that drives the pin.

| Pin | Function | Dir | Source |
|---|---|---|---|
| PA0 | UART4_TX -> Bluetooth (AF8) | out | [`bt.rs`](../firmware/src/bt.rs) |
| PA1 | UART4_RX <- Bluetooth (AF8) | in | [`bt.rs`](../firmware/src/bt.rs) |
| PA2 | debug header "TX" pad: boot strap of the bootloader / FS-loader (pull-up, low = update mode); AF USART1_TX unused | in | bootloader / FS-loader code |
| PA4 | DAC channel 1: generator; ADC0 ch4 read-back | analog | [`gen.rs`](../firmware/src/gen.rs), [`power.rs`](../firmware/src/power.rs) |
| PA5 | DAC channel 2: scope trigger level (stock) | analog | stock, [`hwtrig.rs`](../firmware/src/hwtrig.rs) |
| PA6 | TIM13_CH1 (AF9) backlight PWM 20 kHz | out | [`board.rs`](../firmware/src/board.rs) |
| PA7 | TIM14_CH1 (AF9) buzzer | out | `stock/re/analysis/lcd_buzzer_v2_6.h` |
| PA8 | I2C2 SCL, touch (AF4) | od | [`touch.rs`](../firmware/src/touch.rs) |
| PA9 | DMM relay drive (pulse) | out | [`dmm.rs`](../firmware/src/dmm.rs) |
| PA10 | DMM relay drive (pulse) | out | [`dmm.rs`](../firmware/src/dmm.rs) |
| PA11 | USB DM (AF10) | io | [`usb.rs`](../firmware/src/usb.rs) |
| PA12 | USB DP (AF10) | io | [`usb.rs`](../firmware/src/usb.rs) |
| PA13 / PA14 | SWD (pads "D" / "C") | io | debug header |
| PA15 | TIM2_CH1 (AF1): scope trigger comparator (stock) | in | stock, [`hwtrig.rs`](../firmware/src/hwtrig.rs) |
| PB0..PB9 | [3PA1030](datasheets/3peak-3pa1030-reva1.pdf) data D0..D9 | in | [`scope.rs`](../firmware/src/scope.rs) |
| PB10 | charger input power present: 1 against the MCU pull-down with USB power, 0 within ~8 ms of a USB power cut. Driven low, the charge stops; released, the charger goes to standby until the next replug (above ~4.1 V); a charger input (CE or TEMP, *inferred*). The stock pulses it low ~10 ms per 2 s on USB power below 80..85 % for its battery reading (Unknowns 5) | in (stock: od, released) | device 2026-10-29 (`DBG:CHG?`, `DBG:CHG:LOG?`), stock code (`FUN_0800b5f6`, `FUN_08023b04`), [`power.rs`](../firmware/src/power.rs) |
| PB11 | TP4056 STDBY (*inferred*): 0 against the pull-up when charged on USB power, 1 without input power and for 3.5..4.5 s after USB power returns (a new cycle on a full battery) | in, pull-up | device 2026-10-29, stock code (`FUN_0800b5f6`), [`power.rs`](../firmware/src/power.rs) |
| PB12 | generator path select, 1 = PWM, 0 = DAC | out | [`gen.rs`](../firmware/src/gen.rs) |
| PB14 | TIM12_CH1 (AF9) generator square | out | [`gen.rs`](../firmware/src/gen.rs) |
| PB15 | scope 4051 select bit | out | [`scope.rs`](../firmware/src/scope.rs) |
| PC0 | Bluetooth module power, low = on | out | [`bt.rs`](../firmware/src/bt.rs) |
| PC2 | SPI1 MISO, flash (AF5) | in | [`spiflash.rs`](../firmware/src/spiflash.rs) |
| PC3 | SPI1 MOSI, flash (AF5) | out | [`spiflash.rs`](../firmware/src/spiflash.rs) |
| PC4 | scope / generator analog rail | out | [`scope.rs`](../firmware/src/scope.rs) |
| PC5 | ADC0 ch15, battery / 2 | analog | [`power.rs`](../firmware/src/power.rs) |
| PC6 | TIM8_CH1 (AF3) 50 MHz scope ADC clock | out | [`scope.rs`](../firmware/src/scope.rs) |
| PC7 | DMM frequency mode (1 only in mode 14) | out | [`dmm.rs`](../firmware/src/dmm.rs); function unknown |
| PC8 | TIM3_CH3 (AF2) scope offset PWM | out | [`scope.rs`](../firmware/src/scope.rs) |
| PC9 | I2C2 SDA, touch (AF4) | od | [`touch.rs`](../firmware/src/touch.rs) |
| PC10 | SPI2 SCK -> [HY3131](datasheets/hycon-hy3131-datasheet-v10.pdf) (AF6) | out | [`hy3131.rs`](../firmware/src/hy3131.rs) |
| PC11 | SPI2 MISO <- HY3131 (AF6) | in | [`hy3131.rs`](../firmware/src/hy3131.rs) |
| PC12 | SPI2 MOSI -> HY3131 (AF6) | out | [`hy3131.rs`](../firmware/src/hy3131.rs) |
| PC13 | scope attenuator relay, 1 = direct, 0 = /50 | out | [`scope.rs`](../firmware/src/scope.rs) |
| PC14 | Bluetooth link up (high = connected) | in | [`power.rs`](../firmware/src/power.rs) |
| PD0, PD1, PD8, PD9, PD10, PD14, PD15 | FSMC data (AF12) | io | [`board.rs`](../firmware/src/board.rs) |
| PD2 | HY3131 CS (active low) | out | [`hy3131.rs`](../firmware/src/hy3131.rs) |
| PD3 | SPI1 SCK, flash (AF5) | out | [`spiflash.rs`](../firmware/src/spiflash.rs) |
| PD4 / PD5 | FSMC NOE / NWE | out | [`board.rs`](../firmware/src/board.rs) |
| PD6 | power key, pressed = 1 (pull-down) | in | [`boot.rs`](../firmware/src/boot.rs) |
| PD7 | FSMC NE1 (LCD CS) | out | [`board.rs`](../firmware/src/board.rs) |
| PD11 | FSMC A16 = LCD RS | out | [`board.rs`](../firmware/src/board.rs) |
| PD12 / PD13 | scope 4051 select bits | out | [`scope.rs`](../firmware/src/scope.rs) |
| PE0 | A shunt MOSFET (10 mohm) | out | [`dmm.rs`](../firmware/src/dmm.rs) |
| PE1 | mA shunt MOSFET (0.5 ohm) | out | [`dmm.rs`](../firmware/src/dmm.rs) |
| PE2 | DMM analog rail / power hold | out | [`boot.rs`](../firmware/src/boot.rs) |
| PE3 | scope coupling, 1 = DC, 0 = AC | out | `stock/re/analysis/scope_gen_ui_v2_6.h` |
| PE4 | SPI1 CS, flash | out | [`spiflash.rs`](../firmware/src/spiflash.rs) |
| PE5 | touch INT (falling edge) | in | [`touch.rs`](../firmware/src/touch.rs) |
| PE6 | touch reset | out | [`touch.rs`](../firmware/src/touch.rs) |
| PE7..PE15 | FSMC data (AF12) | io | [`board.rs`](../firmware/src/board.rs) |
| PH0 / PH1 | 25 MHz HXTAL | - | *inferred* (standard pins) |

Not used by the stock firmware or opendm40 as far as traced: PA3 (the debug
header's RX pad), PB13, PC1, PC15. Whether they are connected is unknown.

## Models DM40A / B / C

- The firmware tells the models apart only by one byte in the calibration
  record; the stock [HY3131](datasheets/hycon-hy3131-datasheet-v10.pdf) rows, relays and scope / generator code are the
  same for all three ([firmware/docs/model-variants.md](../firmware/docs/model-variants.md)).
- Hardware difference reported in the chat: the A has a discrete divider instead
  of the TC1746B6 network ([#25087](https://t.me/S2084K/25087), [#34902](https://t.me/S2084K/34902), [#51830](https://t.me/S2084K/51830)). Whether anything else
  differs (reference, shunts) is unknown.
- The A measures AC+DC as well; the stock hides it by model (chat reports,
  [`user-feedback.md`](../firmware/docs/user-feedback.md) UF-03).
- Chat reports say units before 2025 are unshielded; the reviewed unit (2025, battery
  dated 2025-02-10) has shield cans and a foil-lined back cover (photos,
  [`user-feedback.md`](../firmware/docs/user-feedback.md) 12).

## Unknowns

1. No schematic; circuit details beyond the parts list are inferred from the
   BOM and the photos.
2. Which rail(s) hold the power on: PE2, PC4 or both; how the power key turns
   the unit on from off. [`firmware/src/boot.rs`](../firmware/src/boot.rs) and [`native-boot.md`](../firmware/docs/native-boot.md) (shim
   steps) treat PE2 as the power hold. The ROM bootloader drives PE2 and PC4
   high at every reset (`0x08000574`), and the loader's and the stock app's
   power-off path drives both low; their role as the hold is *inferred*.
3. Whether anything else on the board drives PA2.
4. The MCU crystal: 25 MHz by the PLL read-back and the photo, 26 MHz in the
   community BOM (jebem, EEVblog thread, after mydigit.cn). The 25 MHz value
   is the one the running firmware depends on.
5. Charge status pins. Measured (2026-10-29, battery 4.23 V): PB10 follows USB power; PB11 is low
   when charged, high without input power and during a charge cycle. *Inferred*: PB11 = STDBY (the
   only TP4056 pin that fits all states), PB10 = a charger input tied to VIN (CE or TEMP; PROG would
   not hold 1 against the pull-down in standby), the CHRG pin on no MCU pin. The stock shows charging
   when PB10 = PB11 = 1 (`FUN_0800b776`, `FUN_08023840`). It drives PB10 low below 85 % (a signed
   integer compare of the float fraction, `0x08023b36`..`0x08023b3c`); low PB10 releases STDBY on the
   device.
   Measured while charging (2026-10-29, after ~2 h on battery; battery ADC while charging ~4.20 V):
   PB10 low stops the charge at once (battery 4.205 -> 4.14 V, PB11 stays 1; USB current 0.166 A,
   the meter's own load); released, the charger goes to standby (PB11 = 0) and does not charge again
   until the next USB replug (battery 4.11..4.13 V, above its recharge threshold). Twice, same result.
   At a full battery (C2, [`c2meter.py`](../firmware/tools/c2meter.py)) PB10 low or released makes no
   difference to the USB current (0.1966 / 0.1963 A). USB current right after a replug, as the battery
   drained (15 min blocks on battery): 0.36, 0.48, 0.56, 0.67, 0.70 A, the meter's own ~0.17..0.20 A
   included. *Inferred*: PB10 is the charger's CE (or TEMP) line; a low pulse ends a charge cycle
   above the recharge threshold.
   Stock code (disassembly; the only PB10 users are the init `0x0800af60` and the battery task): two
   timers alternate, `0x08023bae` (pins) and `0x08023aac` (battery). Every 2 s the pins task reads
   PB10 / PB11 (PB10 released) and drives PB10 low (push-pull, BC) when PB10 reads 1 and the battery
   fraction (Vbat - 3.3) / 0.9 from the last battery read is below 0.85 on a 0 -> 1 edge of PB10
   (plug-in), or below 0.80 otherwise (`cmp` of the float bits as signed integers + `bge` = skip at
   >= 0.85 / 0.80, `0x08023b36`, `0x08023b62`). 10 ms later the battery task reads the ADC and then
   releases PB10 (open drain high). So PB10 is low for ~10 ms per 2 s, only on USB power below 80..85 %,
   and the battery is read during that pulse. *Inferred* purpose: a battery reading without the charge
   current, below the level (~4.06 V) where the charger restarts after the pulse; above it a pulse would
   end the charge (measured above). opendm40 never drives PB10: its charger runs the plain TP4056 cycle
   (charges to termination after a replug, measured); its battery reading on USB power includes the
   charge current's rise.
6. The fuse's breaking capacity and time curve (the manual gives 10 A, ceramic, one fuse).
   The 47 ohm resistor's exact power rating (MELF 0207 by size, series unknown).
7. PC7's function in the frequency mode; the [HY3131](datasheets/hycon-hy3131-datasheet-v10.pdf) CNT pin source; how the
   buzzer is driven (PA7 and the HY3131 CMP pin) across the isolation.
8. The scope trigger comparator chip; which of PB15 / PD12 / PD13 is the 4051
   A / B / C; the exact gain resistors.
9. The scope op amp ("8091") and the generator output stage after the SGM3157.
10. The regulators and the power path parts in [charger-tp4056-and-power.jpg](photos/charger-tp4056-and-power.jpg).
11. The debug headers' VCC pads: ~3.1 V while on, not the MCU supply
    directly; what sits between them is not traced.
12. LCD panel FPC pinout (whether the ILI9488 TE output reaches an MCU pin: [LCD](#lcd)); touch panel vendor beyond the controller ID.
13. Exact HY3131 wiring (ohm reference resistors, divider taps, shunt nodes):
    inferred from the stock scaling only ([`emu/hy3131/RESEARCH.md`](../emu/hy3131/RESEARCH.md) 8.5).
14. [MAX6190](https://www.analog.com/media/en/technical-documentation/data-sheets/MAX6190-MAX6198.pdf) placement: the BOM post says the reference sits outside the
    shielded zone; this page's diagram draws it in the (shielded) DMM domain,
    which is about the isolated side, not the shield can. The photo
    [dmm-area-max6190-shunt-fuse.jpg](photos/dmm-area-max6190-shunt-fuse.jpg)
    shows it on the top side in the area under the DMM shield plate, next to the
    AGND test point, outside the small shield can; whether that plate counts as
    the BOM's "shielded zone" is unknown.
15. What the four optocouplers carry: the BOM post guesses the HY3131 INT plus
    the relay and MOSFET drive; the stock firmware does not use the HY3131
    IRQ ([DMM front end](#dmm-front-end-hy3131)), and the MCU drives four
    DMM-side GPIOs (PA9 / PA10 relay, PE0 / PE1 MOSFETs) that must cross the
    barrier somehow, so the relay and MOSFET drive is the likelier load.
16. RTC power while off: the feature request (2026-10-13) assumed the RTC is
    always powered from the main battery; nothing in this repo traces VBAT or
    the LSE crystal, and it depends on which rail holds the power (item 2).
