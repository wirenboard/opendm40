# opendm40 vs stock DM40 V2.6 — parity report

Item-by-item comparison of opendm40 ([`firmware/src`](../src)) with the
reverse-engineered stock behaviour (`stock/re/analysis/*_v2_6.h`). Status per
item: **MATCH**, **PARTIAL** (what is missing in parentheses), **MISSING**,
**DEVIATION** (deliberate), **NEW** (no stock equivalent), **UNKNOWN** (needs an
on-device check). The device in hand is a DM40A (model byte 0). The stock offers
AC+DC and the 50/60 k-count ranges only on DM40B/C; opendm40 offers both on
every model ([model-variants.md](model-variants.md)).

Overall: the DMM, scope and generator engines are close ports of the stock
numerics. The open items are tracked in [TODO.md](../TODO.md) (see
[Open items](#open-items) below).

---

## 1. Multimeter (DMM)

Stock: `hy3131_v2_6.h`, `dmm_v2_6.h`. opendm40: [`dmm.rs`](../src/dmm.rs),
[`hy3131.rs`](../src/hy3131.rs), [`hy3131_tables.rs`](../src/hy3131_tables.rs),
[`meter.rs`](../src/meter.rs).

| Item | Status | Notes |
|---|---|---|
| All 16 HY3131 config tables f0..f15 | MATCH | `Dmm::select` picks the same rows per mode/range; tables in `hy3131_tables.rs`, `CFG_F13_R7` and `CFG_F13_DISCHARGE` in `dmm.rs`. |
| Relays PA/PE, PC7 per mode | MATCH | `relay_pa` / `relay_pe` / `pc7`; PA pulsed only on change (`pa_now`). Relay wiring: [hw/README.md](../../hw/README.md#dmm-front-end-hy3131). |
| DC V/mV formulas, neg-gain, r0 linearity | MATCH | `poll_dcv`; `DCV_NEGK`, 1e-4 correction, 1000 V clamp. |
| DC A/mA/uA, shunt/opgain, 10 A filter | MATCH | `poll_dca`; 4-20 mA loop % aux on mode 8. |
| AC V/mV/A: sqrt(RMS), LPF, AC/DCp, EMA windows | MATCH | `poll_ac`; overload guard `sq > 262_144` matches `> 0x40000`. |
| Autorange thresholds + settle deadlines | MATCH | Per-mode settle values match the deadline table in `dmm_v2_6.h` §"SETTLE DEADLINES". |
| AD1 OSR switching (R22 jump thresholds) | MATCH | `Dmm::osr`; DC V 0x206, DC mV 0xE22-0x960·r, DC A 0x168, cont 0x206. |
| Ohm: ratiometric, q-based autorange, p8set/clr | MATCH | `poll_ohm`; R22 0x13/0x17 switching, 2500 ms fast-down gate. |
| Continuity | MATCH (beep is hardware) | `poll_cont`; the stock drives the buzzer from the HY3131 CMPO pin, not from firmware, so opendm40 does nothing. UNKNOWN whether the comparator-to-buzzer path works without the stock init (TODO.md UF-01). |
| Diode + beeper pattern classes | MATCH | `poll_diode` + `diode_beep`; classes/steps/`beep_n<=6` gating match; aux "ohm" = V·gain/0.765. |
| Capacitance RC (r0..4) + ramp (r5..7) | MATCH | `poll_cap`; median-5, 35-sample batch, comparator-flip flush, R21 by slope class, discharge row. |
| Frequency / duty, dual-source in mode 14 | MATCH, extended | `poll_freq_mode` / `freq_secondary`; reciprocal counter, 53.3 ms minimum gate. The stock reads and restarts it every 350 ms, so a gate still waiting for its closing edge gives no or a partial value (below about 6.7 Hz, depending on the phase; inferred from the code in the private analysis, `dmm_v2_6.h` section 5). opendm40 reads it when it has ended (`gate_check`, down to 0.5 Hz) and adds the small digit where one SYSCLK tick over the gate resolves it (`freq_ext`). AC+DC keeps the stock's 350 ms read. |
| Temperature (3-phase K, cold junction) | DEVIATION (minor) | `poll_temp`; °F = °C·1.8 + 32 on the LCD and in the Bluetooth reply (`Bt::serve`), vs the stock factor 1.82 (`BT_TEMP_F_FACTOR` in `dmm_v2_6.h`, a stock bug). |
| Model counts (DM40A/B/C factor/over) | MATCH | `FACTOR` / `OVER`; model from the calibration record (`Dmm::model_raw`), clamped to 2. |
| HOLD / REL / MIN-MAX / 5 history slots | MATCH | `meter.rs`; REL freezes the range for modes <11 and 14, as the stock. |
| Strip chart 160/320/640 ms/div | MATCH | Chart in [`app.rs`](../src/app.rs), rate in `meter.rs`. |
| HV warning ≥100 V, main value colour | DEVIATION | Main value drawn red (requirement) instead of the stock orange/red-from-30 V split. |
| MF manual-range popup, AUTO/AUTO+ | MATCH | `Dmm::set_range`; V/A/ohm items, AUTO+ flags. |
| Calibration read from 0x080C0000 +0x168 | MATCH | `cal()`, sane only when the record's CRC (checked once at start-up, `load_record`) and version word 1 pass (`cal_ok`); **read-only**. |
| AC+DC sub-mode | DEVIATION | Every model (stock: DM40B/C only). The total is shown only once both the AC and the DC part are measured; the stock first shows a total computed from the DC part alone. `dmm.rs`, [`acdcm.rs`](../src/acdcm.rs). |

---

## 2. Oscilloscope

Stock: `scope_gen_v2_6.h`, `scope_gen_ui_v2_6.h`. opendm40: [`scope.rs`](../src/scope.rs),
[`scope_meas.rs`](../src/scope_meas.rs), [`scope_page.rs`](../src/scope_page.rs).

| Item | Status | Notes |
|---|---|---|
| Timebase hardware (TIM1/8 psc/arr, DMA) | MATCH | `HW` table = stock `scope_timebase[31]`. |
| User timebase range | MATCH | `TB_MIN=3`..`TB_MAX=29` = 100 ns/div..50 s/div, as the stock `SCOPE_TB_USER_MIN`/`MAX`; YT (non-roll) up to `TB_YT_MAX` = 1 s/div, 200 ms..50 s/div in roll mode. |
| V/div 10 mV..10 V, front-end GPIO rows | MATCH | `VDIV` / `GPIO_ROW` = `scope_range_gpio`; PC13 relay 200 ms wait kept. |
| Probe x0.1..x1000 | MATCH | `Settings::probe` scales readouts/labels only, as the stock; the full stock list `scope_probe[13]` (`PROBES`) in the scope settings window, Settings tab (UF-12). |
| Gain/offset calibration from record | MATCH | `cal_pair` reads +0x08 (gain) / +0xA8 (offset) with defaults; CCR3 offset PWM formula matches `FUN_080302d6`. |
| DC/AC coupling (PE3) | MATCH | `Scope::apply`. |
| Trigger: level→code→DAC, hysteresis ±2, edge | MATCH | `trig_code` / `find_trigger`; software refinement with arm band ±2 as `FUN_080123ce`. The comparator trigger ([`hwtrig.rs`](../src/hwtrig.rs)) is on by default. |
| Trigger modes Auto/Normal/Single | MATCH | `Scope::poll`; Auto untriggered-timeout table matches `scope_auto_timeout_ms`. |
| Roll mode from 200 ms/div | MATCH | `roll_poll`, circular DMA, per-column min/max, 100 ms. |
| 20 measurement items, 4 shown, defaults | MATCH | `ITEMS` + `DEFAULT_SEL`; Freq/Vpp/Cycl/Vrms default. |
| Measurement formulas (Vpp/rms/top/base, pulse) | MATCH | `compute()`; histogram top/base 30 % band, 10/50/90 % edge machine. |
| Cursors X2-1 / Xf / Y2-1 | MATCH | Items 7/8/17; formulas in `app.rs` / `scope_meas.rs`. |
| Persistence | MATCH | Trace persistence in `app.rs` (commit 01972ab). |
| Settings window (MF) | MATCH | Scope settings window tabs (`scope_page.rs`). |
| AUTO-set | PARTIAL | Two-phase state machine (`autoset_start` / `autoset_step` in `scope_page.rs`; the last run's decisions via `autoset_log`). Untested whether it reproduces the stock period→timebase table (`FUN_08033388`) exactly. |
| Zoom in STOP | PARTIAL (shorter record) | While stopped, a timebase or V/div change re-renders the stored record and a drag pans it (`scope.rs` "Stop-and-zoom"). The record is up to `CAP` = 45 056 samples; the stock keeps 65 520. |
| Math / FFT channel | MATCH, extended | [`fft.rs`](../src/fft.rs): 2048 points, six windows (Rectangular, Blackman, Hanning, Hamming, Triangular, Flat-top), dB or Vrms; ABS math mode. opendm40 adds the dB/div, centre-frequency, Hz/div and peak readouts. |

---

## 3. Generator

Stock: `scope_gen_v2_6.h` §GENERATOR, `scope_gen_ui_v2_6.h` §5. opendm40: [`gen.rs`](../src/gen.rs),
[`genplan.rs`](../src/genplan.rs).

| Item | Status | Notes |
|---|---|---|
| Waveforms sine/square/triangle/ramp | MATCH | `Gen::dac`; same sample formulas incl. the sine (c,w)=(50,4045)/(200,4095) split, DAC buffer on for square only. |
| DAC path n / TIM6 PSC | MATCH | f>5000: PSC=0, n=10e6/f; else k=round(5000/f). |
| PWM square >50 kHz | MATCH | `Gen::pwm`; timer values match `gen_pwm_*`. |
| Vpp, duty 0..10000 | DEVIATION | Vpp 0..3.0 (stock 0.5..3.0), so that an offset with 0 Vpp is a DC output; adjustable offset (stock: fixed 0..Vpp). `AMP_MIN`. |
| Runs while other pages shown (DMA/timers) | MATCH | Output is DMA/timer driven; state shown on the scope bar. |
| Default wave/freq | MATCH | Square, 2 MHz, 3 Vpp, output off (`Settings::DEFAULT`), as the stock. |
| Freq snap 50k..100k | DEVIATION | The stock snaps a square of 50k–100k down to 50 kHz. opendm40 runs it on the PWM path at the nearest frequency 200 MHz / D can make (`plan_pwm`) and shows that real frequency. |
| Non-square >50 kHz | DEVIATION | The stock sets the frequency to 0 Hz (silent, on-flag kept). opendm40 runs at 50 kHz and shows "max 50 kHz"; the set value is kept (`target_hz`, `max_note`). |
| Keypads (freq H/K/M, numeric Vpp/duty) | MATCH | `View::Keypad`, stock layouts (commit 9ca1c9c). |
| RUN toggles output | MATCH | RUN handling in `app.rs`. |

---

## 4. Bluetooth phone-app protocol

Stock: `bt_meas_v2_6.h`. opendm40: [`bt.rs`](../src/bt.rs) (SCPI over the same
link: [bt-scpi.md](bt-scpi.md)).

| Item | Status | Notes |
|---|---|---|
| Frame format, CRC, id 0503/FFFF, len≤0xC2 | MATCH | `Bt::poll`; broadcast detection, resync on bad id/len. |
| Transport: UART4, 115200 8N1, DMA RX | MATCH (DMA vs ISR) | The stock uses the RBNE interrupt, opendm40 circular DMA (IRQs are masked). Functionally equivalent. |
| cmd 00 discover (model letter) | MATCH | `Bt::serve`. |
| cmd 01 nop, 02 version (boot data) | MATCH | Version bytes from the boot data (`boot_ver`). |
| cmd 03 reset, 04 hold, 05 rel | MATCH | REL gating (not HOLD/not OL, modes <11 or 14). |
| cmd 06 mode byte (fwd + reverse) | MATCH | `Dmm::mode_byte` / `Dmm::apply_mode_byte`; full group/range/sub decode. |
| cmd 07 set name / 08 get name (AT) | MATCH | AT+SBN / AT+CBN, "OK"/"CBN+" parsing. |
| cmd 09 measurement reply (fmt + values) | MATCH | `encode()` follows the format-byte rules incl. continuity 0xFFF0 bit 7 and diode value-3. The power meter sends the DC V frame with P as the value. |
| Flags byte (bit0-2 battery, bit3 charge, bit6 lock) | PARTIAL (untested) | Built from `meter.hold` (bit 7), the key lock (bit 6), REL (bit 5), `Power::charging` (bit 3), `Power::bars` (bits 0-2). The meaning of bits 6 and 3 is inferred from the header; untested against the phone app (TODO in `Bt::serve`). |
| Startup AT init (SBNDM40x/SLED0/RESET) | MATCH (once per power-on) | `Bt::init_module`: AT+SBNDM40\<model\>, on OK AT+SLED0 and AT+RESET. The stock runs it once ever (settings 0x115 bit 7); opendm40 once per power-on, not while a phone is connected (PC14 high). |
| PC14 "connected" / PC0 power control | MATCH | `Power::bt_connected` reads PC14; `Bt::power` drives PC0 low (module on) while Bluetooth is ON and high when OFF, plus the stock's 100 ms high pulse after a successful cmd 07. Deviation: the stock powers the module only on its meter page, opendm40 on every view (UF-36). Pins: [hw/README.md](../../hw/README.md#bluetooth). |
| Temperature F in reply = 1.82 (stock bug) | DEVIATION | opendm40 sends main and cold junction in °C and °F = °C·1.8 + 32 as value 3, so the phone app shows a slightly different °F than with the stock. |

---

## 5. GUI / menus

Stock: `gui_v2_6.h`, `gui_render_v2_6.h`, `lcd_buzzer_v2_6.h`. opendm40: [`ui.rs`](../src/ui.rs),
`app.rs`, [`stockfont.rs`](../src/stockfont.rs), [`lcd.rs`](../src/lcd.rs) ([fonts.md](fonts.md)).

| Item | Status | Notes |
|---|---|---|
| DMM screen layout, stock fonts | MATCH | Rendered from the stock app fonts; main value red. |
| Main menu tiles | DEVIATION | Stock: 4 tiles + the "正点原子" logo. opendm40: `ui::main_menu` without the Alientek logo (requirement), plus the combined-view tile. |
| Settings: dim, shutdown, backlight, volume, BT, start page | MATCH | `View::Settings`; dim indices 5/10/30/never, shutdown 10/30/60/never. |
| Language selection (zh-CN/zh-TW/en) | MISSING (by design) | English only; the stock has 3 languages (`CFG_LANGUAGE`). |
| Device / About page (Vendor/Web/Model/version) | MATCH | Settings > More > More > Device (`View::Device`): vendor, web, model, bootloader, software and hardware versions, plus the board temperature. |
| Per-function DMM settings pages (MF) | MATCH, extended | MF opens the per-function page (`View::Range`, titled "Voltage Settings" / "Current Settings" / ... as the stock): ranges, coupling, plus opendm40 extras (fuse check, inrush). A function without a range page opens Hold & Limits (`View::Limits`). Option-by-option comparison with the stock not done. |
| "Other Settings" toggles (Anti-drop, Mode Remember, BT) | MATCH | Anti-drop = `Dmm::reassert_relay`; mode remember and the toggles in backup SRAM instead of config.bin. |
| Idle dim + auto power-off | MATCH | `App::tick`: dims the backlight, power-off dialog/action. |
| Combined view (DMM + scope) | NEW | `View::DmmScope`; no stock equivalent. |

---

## 6. Touch

Stock: `touch_v2_6.h`. opendm40: [`touch.rs`](../src/touch.rs), [`gesture.rs`](../src/gesture.rs).
Panel wiring: [hw/README.md](../../hw/README.md#touch-panel-and-touch-keys).

| Item | Status | Notes |
|---|---|---|
| Hynitron 0x5A protocol (D1 01 / D2 04 / D1 09, D0 00 frame, 0xAB, ack) | MATCH | `Touch::new` / `read_hyn`; chip-type 0xA8 check, extra points read, D0 00 AB ack. |
| Coordinate decode X/Y, landscape | MATCH | `decode()`. |
| Side keys MENU/RUN/AUTO/MF from key-report X | MATCH | Thresholds 500/750/1000 (measured 372/628/884/1140). |
| Debounce / long-press filtering | MATCH (opendm40 adds more) | `Debounce`: `DEBOUNCE_MS`, two pressed frames or one plus silence; key reports during a touch or within 300 ms after one are dropped (the controller's long-press gesture). The stock relies on controller filtering only. |
| Gestures (tap/long-press/swipe/drag) | NEW | `gesture.rs`; the stock has no gestures. Used for view switching. |
| Power key PD6 (hold 1.5 s off, double click lock) | MATCH | `App::tick`. Key wiring: [hw/README.md](../../hw/README.md#power-key-pa2-buzzer). |
| I2C transport | MATCH (no DMA) | Hardware I2C2 (400/100 kHz by rise time, bit-bang fallback) + EXTI5 pending-bit gating; no DMA. The stock uses I2C2 + DMA + the EXTI interrupt. Raw bus/IRQ counters in DBG:DIAG? bytes 20..59. |
| Goodix GT1158 fallback (0x14) | MATCH (untested) | Probed first at 0x14 as the stock, then 0x5A (`Touch::probe`, `read_goodix`). Untested: the unit in hand has the 0x5A chip. UNKNOWN which panels ship. |
| Multi-touch (up to 5 points) | MATCH | `Event::pts` carries up to `MAX_POINTS` = 5 points; pinch on the device is untested (TODO.md). |

---

## 7. USB

Stock: one mass-storage disk ("USB Connect": the SPI flash's FAT16 as it is,
`spiflash_v2_6.h`) and no other USB function; the phone-app protocol is
Bluetooth only. USBTMC / SCPI is an opendm40 addition.

opendm40: [`usb.rs`](../src/usb.rs), [`usbtmc.rs`](../src/usbtmc.rs), [`scpi.rs`](../src/scpi.rs),
[`scpi_msg.rs`](../src/scpi_msg.rs), [`msc.rs`](../src/msc.rs), [`vdisk.rs`](../src/vdisk.rs),
[`vwrite.rs`](../src/vwrite.rs). One composite device, always present: USBTMC (USB488) plus one
mass-storage LUN.

| Item | Status | Notes |
|---|---|---|
| Mass storage | CHANGED | Not the raw SPI FAT16: a FAT16 volume generated on the fly (`vdisk.rs`): `LOGS/*.CSV` and `SCOPE/*.CSV` made from the DLG files, `SHOTS/*.BMP`, `INFO/` (calibration, boot data, UID, option bytes, OTP, loader images, `FIRMWARE.BIN`, `BACKUP.TXT`), `DEBUG/SPIFLASH.BIN` (the raw 16 MiB), `README.TXT`. |
| Writes from the PC | CHANGED | "Files live" (default): read-only. "PC can edit" (Settings 3/3, at the next plug): deletes in LOGS / SCOPE / SHOTS and a copied `dm40*.bin` (checked, then "Install it now?") are taken (`vwrite.rs`); other files are not kept. The stock disk keeps whatever the PC writes. |
| Firmware update from the disk | MATCH | Any `dm40*.bin` (stock app or the vendor loader's disk); opendm40 also stages one copied to its own disk, or sent with `DBG:UPLOAD` ([`tools/reload.sh`](../tools/reload.sh)). |
| USBTMC/USB488 + SCPI | NEW | DMM, power meter, inrush, alerts, scope, generator, files and the logger; `DBG:*` debug commands (refused over Bluetooth). Command list: [docs/manual/scpi-reference.md](../../docs/manual/scpi-reference.md). |
| HP 34401A identity | NEW | `SYST:IDN:EMUL 34401A`: libsigrok / SmuView's 34401A profile drives the meter. |

---

## 8. Combined / simultaneous operation (opendm40 only)

The stock switches instruments; opendm40 runs DMM + scope + generator at once and
only switches the view. Constraints and how opendm40 handles them:

- **Analog power rails** (scope / DMM side, [hw/README.md](../../hw/README.md#power-and-battery)):
  the stock powers one side per page. opendm40's policy is `SYST:RAIL AUTO|BOTH`
  (default AUTO, persisted): AUTO powers the side the view needs (both in the
  combined view and with a host scope command), BOTH keeps both up. Measured with
  both rails on (device check d8e249d, [device-check-2026-10-21.md](../../docs/history/device-check-2026-10-21.md)):
  DC within 0.04 %; a 40 Hz 0..9.6 V square reads +0.19 % more in V (edge-rate
  dependent coupling of the powered scope front end), which is why AUTO keeps the
  scope rail off on the DMM page.
- **Generator independence**: DAC+DMA / PWM, no CPU involvement, so it runs
  during any view (the stock keeps it running with the scope; opendm40 also with
  the DMM).
- **HY3131 vs scope ADC**: separate chips and buses (the HY3131 on hardware SPI2,
  `hy3131.rs`; the scope ADC on a GPIO port read by timer-driven DMA), no bus
  contention. The SPI flash is on a third SPI peripheral.
- **SPI-flash writes**: done in bounded slices from the main loop (CSV commits,
  screenshots, the fast logger's pre-erased clusters), so the DMM, USB and touch
  keep running; a screenshot freezes the screen for ~1.5 s
  ([ram-budget.md](ram-budget.md)).

---

## 9. Data logger (FR-38) — new feature

The stock has no logger. opendm40 ([`logger.rs`](../src/logger.rs),
[`fastlog.rs`](../src/fastlog.rs), [`csvlog.rs`](../src/csvlog.rs)):

- **Record page** (`csvlog.rs`): DMM readings at an interval (every reading,
  1 s, 10 s, 60 s, mean per row) to `LOGS/LOGnnn.DLG`, shown as CSV on the USB
  disk; `LOG:CSV`.
- **Fast logger** (`logger.rs`): raw AD1 samples (40 .. 640 S/s, LOG:OSR /
  LOG:CHOP) or the AD2 stream (1200 .. 76 800 S/s, `ad2-isr` builds), each
  record with a sequence number and a device microsecond timestamp, so a gap
  is visible; streamed to the host (`LOG:DATA?`) or to the SPI disk
  (`LOG:DEST DISK`: `LOGS/FASTnnn.DLG`, `fastlog.rs`).
- Over Bluetooth only AD1 sessions (`Rate too high for Bluetooth`).
- Formats: [compact-log-format.md](compact-log-format.md).

---

## Open items

Tracked in [TODO.md](../TODO.md) ("Remaining work", the parity sub-list and
"Next up"):

- Continuity / diode beeper on the device (UF-01).
- Both-rails crosstalk: closed as analog for DC; the open part is the first
  Hz readings after a function change (fix on branch freqfirst, device retest).
- BT flags bits 3 / 6 against the phone app.
- Goodix GT1158 path (needs a unit with that panel); pinch on the device.
- Scope: full 65 K record (DMA RAM budget); AUTO-set against the stock table.

Deliberate deviations (not gaps): red main value, no Alientek logo, English-only
UI, temperature °F factor 1.8 (stock bug 1.82), AC+DC on every model, the
generator's 0 Vpp minimum, 50 kHz clamp and nearest-frequency square, combined
views, USBTMC, touch gestures.
