# AC+DC on the DM40: how other meters and DMM chips do it, and what fits the HY3131

How other meters and DMM chips measure AC+DC (the true RMS of AC with its DC
part), which approaches fit the HY3131, and the device record of the two
that opendm40 built as alternatives to the stock alternation (section 10,
[`src/acdcm.rs`](../src/acdcm.rs)). The stock method, which opendm40 runs by default
(method A), alternates whole HY3131 rows: an AC phase (AD2 + RMS after the
HPF on the AC-coupled PB4 node) and a DC phase (AD1), ~1.9 s per cycle,
total = hypot(AC, DC) ([`src/dmm.rs`](../src/dmm.rs) `dc_out` / `poll_ac`).

Tags: **INFERRED** = follows from the cited sources but is stated nowhere;
**UNKNOWN** = not known. Untagged facts come from the cited datasheet or
manual page, a stock row byte, the opendm40 source or a device measurement.
Short names: "acdc" = `stock/re/docs/history/acdc-stock-vs-opendm40.md` (in the private analysis repo)
(the stock's AC+DC measured live), "feasibility" =
[dmm-input-scope-feasibility.md](dmm-input-scope-feasibility.md), "power" =
[power-meter-and-misconnect.md](power-meter-and-misconnect.md), "device check" =
[device-check-2026-10-21.md](../../docs/history/device-check-2026-10-21.md). Sources: section 9.

## 0. Summary

* **Two camps in the industry.** (1) *Separate AC and DC measurements combined
  as sqrt(AC^2 + DC^2)*: Fluke 187/189 and 287/289 ("AC+DC is defined as
  sqrt(ac^2 + dc^2)", the two parts are also shown as a dual display), Hioki
  DT4281/4282 (DC+ACV at half the display rate, autorange "depending on the
  DC+AC calculation result", the DC part keeps the DCV spec plus a published
  2 % penalty for ripple under 45 Hz), Keysight U1281A/U1282A (the AC+DC spec
  is the AC spec plus ~0.05 % and 5..10 counts, i.e. the DC path's error
  added), and Keysight's own application tip (measure DC with >= 10 PLC and
  AC separately, combine by the formula: "better than the built-in AC+DC
  function"). (2) *A DC-coupled RMS converter*: Brymen BM869s (one "DC AC &
  AC+DC" accuracy table, 0.7 % + 80 d even for pure DC against 0.03 % + 2 d in
  the DC function), the EEVblog 121GW (HY3131 **plus an external AD8436 RMS
  chip**: AC+DC 1.0 % + 10 vs AC 0.3 % + 10 and DC 0.05 % + 5), the Keysight
  3458A in its DC-coupled sampling mode (an extra 0.15 % of reading + 0.25 %
  of range when DC > 10 % of AC). The DC-coupled camp pays with DC accuracy;
  the separate-path camp pays with update rate and a ripple-leak term on the
  DC part. Fluke 87V and Owon XDM1041 have no AC+DC at all (AC is
  AC-coupled); for the Keysight 3446xA none is listed either, but its data
  sheet was not retrievable here. Keysight U1272A and Cyrustek ES51968 (both
  on the question's list) were not covered: no primary document was found
  in the time spent; the U1272A belongs to the same U1270 family whose
  U1280 sibling is in the table. "DTA0660" was read as the Dream Tech
  **DTM0660**; if a different part was meant, that row is open.
* **No DMM chip surveyed has a native AC+DC mode.** The HY3131's RMS block is
  defined after a high-pass filter (datasheet 13.2), no reserved bit bypasses
  it (device probe, [hw/README.md, Measured node behaviour](../../hw/README.md#measured-node-behaviour)), and HYCON gives no AC+DC
  recipe anywhere: APD-DMM003 configures ACV and DCV separately, and HYCON's
  own ENOB tool runs its ACV reference row with **AD1 off** (R23 = 0x03).
  DTM0660 (DSP RMS, 1 kHz) lists no AC+DC mode; FS9922-DMM4 is an
  average-responding rectifier; Cyrustek ES51966 needs an external ES636 RMS
  converter (which is what gives the Brymen its DC-coupled AC+DC). The 121GW
  is the only HY3131 meter found with AC+DC, and it does not use the HY3131
  RMS block for it. So the DM40's alternation is not an odd choice: every
  HY3131 design must build AC+DC in software from the two converters.
* **What matters for accuracy is the combination formula.** With
  T = sqrt(AC^2 + DC^2):
  `dT/T = (AC^2 * (dAC/AC) + DC^2 * (dDC/DC)) / (AC^2 + DC^2)`. The DC part's
  relative error counts only by its share of the power. On an AC-dominant
  signal the DC path's 0.03 % vs 0.1 % is irrelevant; on the ripple-on-a-supply
  case (the reason AC+DC exists, Fluke's own example) the DC error dominates
  and a 0.1 % DC path costs ~0.07 % of the total against AD1.
* **Outcome (section 10).** Methods B (option (b)) and C (option (c)) run
  next to the stock alternation A, selected with `DBG:ACDC:METH`, in builds
  with the cargo feature `acdc-meth` (off by default: A only). B failed
  its device rule (its zero is bimodal per entry, 10.5). C meets the rule on
  V on this unit (10.15); A stays the default until C is checked on the
  current ranges with real current (10.16, TODO). Option (d) is not built.
* **Recommendation of 2026-10-22 (section 7).** V: one fixed row with AD1 for the DC part
  (factory DC V calibration kept) and AD2 + AD3 + RMS for the AC part, option
  (b), **conditional** on the note-7 measurement running now: the AD1 offset
  shift with AD2 on must be a stable per-range constant; then it is removed by
  a zero taken in the row's enable order. If it is not stable, option (c): the
  DC part from the DC-coupled AD2 row of test 6.5 with a per-range gain
  self-calibrated against AD1 at entry (the `POW:VOLT:CAL` pattern), the AD1
  row kept only for that calibration. Either way the cycle goes from 1.9 s to
  a reading every ~107 ms, and the DC part's ripple rejection is the
  chosen converter's, not the stock's "last of three unfiltered". Keep (a) as
  the stock-parity mode. A: the power meter's DC row (power 3.12) is already a
  working single-row AD1 + AD2 + AD3 precedent on the shunt; the shunt node is
  DC-coupled, so even the LPF gives I_dc with the AC A RMS gain. (d),
  software RMS of the DC-coupled stream, is an experiment for low frequencies
  and arbitrary waveforms, not the main reading: its limit is the DC x AC
  cross term over a non-integer number of cycles, not CPU.

## 1. How bench and handheld meters do AC+DC

Accuracies are +-(% of reading + counts), the band 45 Hz .. ~1 kHz unless
said otherwise. "Dual display" = the meter shows AC and DC as two readings.

| meter | AC+DC method (evidence) | DC spec | AC spec | AC+DC spec | update / notes |
|---|---|---|---|---|---|
| Fluke 87V | **none**: "AC conversions are ac coupled, true rms responding" (83V/87V spec, note 2) | 0.05 % + 1 | 0.7 % + 2 (45-65 Hz) | - | - |
| Fluke 187/189 | separate parts: "AC+DC: for dc volts and dc amps functions, reading represents the rms total of ac and dc measurements"; AC over DC / DC over AC dual displays (users manual, display symbols table, rotary switch table) | 0.025 % | - | in the 187/189 extended spec (not retrieved as text) | dual display; bar graph off in ac+dc |
| Fluke 287/289 | separate parts: "AC+DC is defined as sqrt(ac^2 + dc^2)" (calibration manual p. 5); autoranging "controlled by the underlying values of the ac and dc signals and not by the sum" (users manual p. 26-27) | 0.025 % + 2 (5 / 50 V) | 0.3 % + 25 (45-65 Hz), 0.4-0.6 % + 25 (65 Hz-10 kHz) | **0.5 % + 80** (45 Hz-1 kHz), 2 % + 80 (20-45 Hz), 1.5 % + 40 (1-20 kHz), 5 % + 40 (20-35 kHz); + 20 counts in dual display; specified 2..140 % of range | bar graph, Hz, peak, duty not available in AC+DC; forum reports ~1 reading/s logging |
| Keysight U1281A/U1282A | the AC+DC spec = AC spec + 0.05 % + 5..10 counts in every band and range (data sheet 5992-0847EN p. 11-12), i.e. the DC path's error added on top of the AC path's: separate parts (INFERRED from the spec structure) | 0.025 % + 5 | 0.3 % + 25 (45 Hz-1 kHz) | **0.35 % + 30** (6 V..1000 V), 0.35 % + 35 (60 mV); current 0.65-0.95 % + 30..45 vs AC 0.6 % + 25 | 100 kHz bandwidth (U1282A) |
| Keysight 3446xA (34461A) | no AC+DC function is listed (AC-coupled ACV; the Truevolt data sheet itself could not be retrieved as text, so this rests on the product listings and the forum); Keysight's tip for its DMMs: take a DC reading with >= 10 PLC (rejects the AC) and an AC reading, combine as sqrt(AC^2 + DC^2); "will yield a better result [than a built-in AC+DC function], particularly when the dc and ac values are of different magnitudes" | 35 ppm (34461A) | - | - | - |
| Keysight 3458A | native ACDCV in all three AC methods; in synchronous sub-sampling "the input signal is DC-coupled for both the ACV and ACDCV function; in ACV the DC components are mathematically subtracted"; the data sheet adds an ACDCV error to the ACV accuracy: 0.0 % + 0.02 % of range when DC < 10 % of AC, **0.15 % of reading + 0.25 % of range when DC > 10 % of AC** (100 mV-1000 V, SYNC); analog mode 0.008 % of range (DC < 10 % AC) | - | - | - | reading rates ACV vs ACDCV nearly equal (data sheet "Second / reading" table) |
| Brymen BM869s | DC-coupled RMS converter (the manual file is named `...-ES636-...`; the function table is headed "DC AC & AC+DC AC Voltage" and has a "DC, 45 Hz ~ 300 Hz" column: the same spec for a DC input); forum: "DCV range has two paths: directly DC volt and True-RMS volt, they will not show exactly the same value" | 0.03 % + 2 d (5 / 50 V), 0.02 % + 2 d (500 mV) | 0.3 % + 20 d (5 / 50 V, 45-300 Hz) | **0.7 % + 80 d** (5 / 50 V), 0.45 % + 40 d (500 mV), 0.7 % + 40 d (500 / 1000 V); 1.5 % + 40 d (20-45 Hz) | mV AC+DC "signal peak including DC bias < 1000 mV peak" (the converter's input window) |
| EEVblog 121GW (**HY3131** + STM32L152) | AC+DC via an **external AD8436** true-RMS chip ("RMS Sensing: True RMS sensing (AD8436)"), i.e. a DC-coupled analog RMS path, not the HY3131 RMS block; modes DC / AC / DC+AC on the V position only (not mV) | 0.05 % + 5 (5-500 V) | 0.3 % + 10 (45-400 Hz), 1.5 % + 10 (0.4-5 kHz) | **1.0 % + 10** (45-1000 Hz, 5 V..600 V) | 5 readings/s; manual caution: "ACV autoranging should not be used with large DC offsets in the presence of AC voltages" |
| Hioki DT4281/DT4282 | separate parts: SHIFT during DC+ACV shows "DCV (main display) ... the accuracy of the 45 Hz to 65 Hz [DC+AC] table is applied; **2 % rdg. should be added when there is AC voltage superimposition less than 45 Hz**", "ACV (sub display): according to the accuracy table"; "Auto range: the range moves depending on the DC+AC calculation result" (manual p. 86-89) | 0.025 % + 2 (6 / 60 V) | 0.2 % + 25 (6 V..1000 V, 45-65 Hz) | **0.3 % + 30** (45-65 Hz), 0.4 % + 30 (65 Hz-10 kHz), 1.2 % + 65 (20-45 Hz, 6 V only); 6 V..1000 V only (no mV) | **DC+ACV 2.5 readings/s vs 5/s** for everything else; 1 MOhm AC input; crest factor 3 |
| UNI-T UT61E+ | mode exists; retailer spec (secondary) DC+AC 1.8 % + 70 (2.2..220 V), 4 % + 70 (1000 V) vs DC 0.05 % + 5, AC 0.8..1.2 % + 10..50 | 0.05 % + 5 | 0.8-1.2 % | 1.8 % + 70 (retailer) | chip not confirmed (forum: ES51997 family; datasheet download blocked) |
| UNI-T UT171B/C | "AC+DC measurement" listed; forum: HY3131 inside; mechanism UNKNOWN | 0.025 % + 5 | 0.4 % + 40 | not found | - |
| Owon XDM1041 | none (AC and DC are separate functions) | - | - | - | - |

Reading of the table:

1. Every maker that publishes an AC+DC spec makes it **worse than both the DC
   and the AC spec**, by 0.05 % + 5 counts (Keysight: separate parts, just the
   DC error added) up to 0.7 % + 80 d (Brymen: the DC-coupled RMS converter
   measures DC with the AC path's accuracy). The 121GW, the only HY3131 meter
   with AC+DC, is 3x worse than its AC spec because the AD8436 path is not the
   calibrated AD1 path.
2. The separate-path meters **halve the update rate** (Hioki 2.5/s vs 5/s;
   Fluke 289 ~1/s) and **publish a ripple-leak term on the DC part** (Hioki's
   +2 % rdg below 45 Hz; Fluke's 2 % + 80 in the 20-45 Hz band of the AC+DC
   column vs 1.5 % + 60 in the plain AC column). The DC part is a DC
   integrating converter whose notches sit at the mains frequency; off-notch
   ripple leaks into the DC reading. That is exactly the AD1 situation here
   (section 4).
3. Autorange practice differs: Fluke ranges on the **underlying parts** (up
   when either part exceeds the range, down when both fall below 10 %), Hioki
   on the **calculated total**. The stock DM40 ranges on hypot(AC raw, DC)
   (acdc 2.3, S+0x60) = the Hioki way. Either is defensible; Fluke's protects
   the AC part from clipping when a large DC offset rides on it (the 121GW
   warns about exactly this failure), which the total alone does not see when
   both parts are moderate but their sum of peaks is not. For the DM40 the
   peak criterion is the AD2 window (+-1.25 V / IG at the node, feasibility
   1.1): |DC| + sqrt(2) x AC_rms must stay inside it, the same rule
   `POW:VOLT:CAL` already applies (power 3.12 "BIG").
4. Nobody shows frequency, bar graph or peak in AC+DC (Fluke) or shows it at
   full rate (Hioki). The stock DM40 shows no frequency in AC+DC either (power
   4.2 "AC+DC shows no frequency").

## 2. DMM chips

| chip | AC method | AC+DC | source |
|---|---|---|---|
| HYCON HY3131 / HY3130 | digital: AD2 (19-bit, 76.8 kS/s) -> **HPF** -> RMS = sum(X^2)/N (or sum(X*Y)/N with AD3, HY3131 only), LPF = DC mean of the same stream; AD1 24-bit for DC | **none**. X and Y are "the data that passed High Pass Filter" (DS 13.2 RMS Converter (3)); no control bit for the HPF in the register map; the reserved bits of R26 / R34 / R35 were poked on the device and none bypasses it (d150919, [hw/README.md, Measured node behaviour](../../hw/README.md#measured-node-behaviour)). APD-DMM003 has separate DCV (sec. 3) and ACV (sec. 4) configurations and nothing combined; APN-HY3131001 is about SPI timing only; HYCON's "DemoCode" zips contain only a "website under construction" readme. HYCON's ENOB tool (`HY3131-ENOB.zip`, `Config/ACV/HYCONFIG-0.txt`) sets R23 = 0x03 (**ENAD1 = 0**), R26 = 0x80, R29 = 0xF8 for ACV, R22 = 0x14 / R23 = 0x8B / R26 = 0x00 for DCV: HYCON's own reference rows run one converter family at a time | DS-HY3131 p. 29-34; APD-DMM003 p. 11-17 |
| HYCON HY3132 | nothing found on the web (no datasheet, no mention) | UNKNOWN | - |
| Dream Tech DTM0660 | "built-in DSP, it can measure true RMS up to 1 kHz without any additional components", "AC Rectification: built-in true RMS processor ... 1 kHz bandwidth, error < 0.5 %", 14-bit sigma-delta, 3 readings/s | not in the feature list; function table has DC/AC only | DTM0660 datasheet p. 1-2 (shaddack mirror) |
| Fortune FS9922-DMM4 | "AC rectifier OPAMP" (average responding, no RMS) | none (DC/AC mode bit per function) | FS9922-DMM4 DS rev 1.3, sec. 12.10, function table |
| Cyrustek ES51966 | dual-slope ADC + "on chip OP amp for AC/DC conversion" (average responding); "RMS-to-DC converter chip will be needed to obtain the correct true RMS value", application with the **ES636** analog RMS converter | none native; an external ES636 is DC-coupled or AC-coupled by the board = how the Brymen gets AC+DC | ES51966 DS p. 1, sec. 4.2 |
| Cyrustek ES51997 (UT61E+ family) | 60 000-count dual-slope "SADC", 630 mV full scale (alldatasheet summary); the PDF itself could not be downloaded (403) | UNKNOWN; the UT61E+ has a DC+AC mode, so the chip or the meter firmware combines two readings | alldatasheet listing only |
| analog RMS converters (AD536A, AD636, AD737, AD8436, ES636) | compute the RMS of whatever is at the input; DC-coupled -> AC+DC, with a series capacitor -> AC only | the "camp 2" building block | AD536A/AD8436 datasheets (web) |

So: a **true DC-coupled RMS** needs either an analog converter in front
(121GW, Brymen) or a digitizer whose RMS is computed without a high-pass
(3458A SYNC). The HY3131 can only give the second kind if the MCU computes the
RMS from the raw AD2 stream (option (d)); its own RMS block is AC-only by
construction. Everyone building on an HY3131 with its own blocks is in camp 1.

## 3. HY3131 facts that bound the options

* **AD1 rate.** AD1CLK = SYSCLK/30 = 4.9152 MHz / 30 = 163.84 kHz; output
  rate = AD1CLK / OSR1 (DS 13.1 (10), (11)). OSR 8192 -> 20 S/s; AD1CHOP = 1x
  halves it ("ADC output rate will be one time slower", DS 13.1 (2)) -> **10
  S/s, 100 ms per result**, which is what the stock's DC phase shows (3
  results in 120..320 ms after the deadline, acdc 3). OSR 32768 chopped ->
  2.5 S/s (400 ms), the plain DC V setting of the stock.
* **AD1 ripple rejection.** Datasheet "Normal Mode Rejection: offset 500 mV,
  AC 50 mV, 50 Hz / 60 Hz +-1 Hz: 120 dB at 5 sps, **75 dB at 10 sps**" (DS
  p. 12). The block diagram labels the AD1 decimator "Comb Filter 2" and the
  AD2/AD3 one "Comb Filter 3" (DS p. 29, 31) - a sinc^2 and a sinc^3. Nothing
  else about the response is published. INFERRED envelope for frequencies
  off the mains notches at 10 S/s chopped (sinc^2 at 20 S/s times the
  two-sample chop average): ~-33 dB at 25 Hz, -45 dB at 37 Hz, -43 dB at
  45 Hz, -58 dB at 105 Hz, notches at every multiple of 10 Hz (so 40 Hz, the
  calibrator PWM of the acdc doc, is a notch: that is why the stock's DC part
  was flat to 0.25 mV there). The datasheet's 75 dB at 50/60 +-1 Hz is
  better than this envelope predicts, so the model is pessimistic; **the
  off-notch leak is a device measurement (section 8, test 3)**. At OSR 32768
  chopped (2.5 S/s) all of this improves by ~24 dB but costs 400 ms per
  result.
* **AD1 noise.** sd 25 counts (4.2 uV) at OSR 32768 chopped on DC mV (acdc
  6c); white scaling to OSR 8192 gives ~50 counts (INFERRED). One AD1 count
  at the jack: 1.67 uV on the 6 V range, 16.7 uV on 60 V, 167 uV on 600 V
  (acdc 4: 288676 counts = 4.8155 V at r1), so the noise is < 1 display digit
  (0.1 mV on 6 V) at either OSR.
* **AD1 offset with AD2/AD3 on (note 7).** "When AD1 and AD2/AD3 is on, users
  must not turn on/off AD to avoid different AD1 Offset problem" (DS 13.1
  note 7, p. 29). Device (feasibility 13.1): AD1 reads **0.05..0.9 k counts
  low while AD2 converts** in the AC V row, mostly a steady offset per range,
  level-dependent on 60 V. With AD2 + AD3 + RMS on for the whole DC V row
  (feasibility 13.2): offsets of -2.9 mV / +17 mV / +0.17 V / -1.6 V on 6 / 60
  / 600 / 1000 V that repeat to ~1 %, and a gain shift up to -0.18 % on 60 V
  that does not repeat. This decided against (b) (section 10).
* **AD2 / AD3.** 19-bit, 76.8 kS/s at OSR2 = 32; full scale +-1.25 V / IG at
  the input node; 4.77 uV per code at IG 1.0; no noise / linearity /
  bandwidth figure in the datasheet; gain drift **10 typ / 30 max ppm/C
  against 2.5 ppm/C for AD1** (DS p. 11). Device floor in the stock AC V 4 V
  row: 31..33 sqrt(RMS) counts ~ 14 ENOB per sample (feasibility 1.2).
* **AD2 as a DC converter** (test 6.5, feasibility 13.1,
  [`tools/experiments/ad2dcv.py`](../tools/experiments/ad2dcv.py)). The register-only
  DC-coupled V row (feasibility 1.3 last row: the DC V tap, OP1 x1 on SENSE,
  SAD2IP = OP1O, IG 1.0) is **linear to <= 0.003 % of range**, noise **3..10
  codes** (over the ~100 ms of one AD1 conversion), gain vs the nominal
  K x 1.25 / 2^18: **+0.84 / +1.79 / +0.53 / +0.27 %** on the 6 / 60 / 600 /
  1000 V ranges. One code at the jack = 4.77 uV x K: 47.7 uV on 6 V (0.48
  digit), 0.48 mV on 60 V, 4.8 mV on 600 V, 48 mV on 1000 V. So the AD2 DC
  path is good to ~0.005 % after a per-range gain and offset calibration and
  resolves 1.5..5 digits of noise, against AD1's < 1 digit. The factory
  record has no gain for this row (the AC V LPF gains at 0x64/0x74 belong to
  the AC-coupled PB4 node, feasibility 3.3), so the gain must be self-measured
  against the DC V function.
* **LPF ripple rejection.** Not in the datasheet. Upper bound from the stock
  capture: with 4.8 V rms of 40 Hz on the input the LPF of the AC-coupled node
  read 0.073..0.096 V in all 796 samples (acdc 4); the 20 mV spread includes
  the real DC creep of the node, so the 40 Hz leak is **<= 0.5 % of the AC
  amplitude** (INFERRED, one frequency). The RMS and LPF results come every
  106.7 ms at LPFBW = 111 where the datasheet formula gives 213 ms
  (feasibility 1.1), so the window shape is not known either.
* **HPF corner.** Not published. The analog part is the PB4 coupling
  capacitor: tau ~0.3 s, fc ~0.5 Hz (acdc 5 / 6a); the digital HPF inside the
  RMS block is unspecified. The stock's AC V reads 40 Hz flat to 1 count
  (acdc 4); the only HY3131 meter with a published AC spec, the 121GW,
  specifies AC from **45 Hz** up (and 20 Hz-45 Hz is where every meter in
  section 1 degrades to 1-2 %).
* **Two converters on one node.** Done already: the misconnect sentinel runs
  AD1 inside the AC V / AC mV rows (feasibility 7.1, power 4.6-4.7), and the
  power meter's DC row runs AD1 (on OP1O - PB3) with AD2 and AD3 in one row,
  written "with all three converters off, then AD3, AD2 and AD1 last, so AD2 /
  AD3 never switch while AD1 converts" (power 3.12). Note 8 (AD2IN != AD3IN or
  both AGND) is met by the stock AC V row (both AGND).
* **Row differences at r0 (6 V)** ([`src/hy3131_tables.rs`](../src/hy3131_tables.rs) CFG_F0 /
  CFG_F1, switch bytes R2C..R2E decoded with `emu/hy3131/ref/registers.md`:
  R2D = PS7 DS7 FS7 SS7 | PS6 DS6 FS6 SS6). f0 AC V r0: R2D 0x09 = **tap PA6
  (/100)** with R25 0x43 = OPS1 = OPS2 = 1 (OP1 through the external x10
  network), so the PB4 node sees x0.1 of the input; f0 r1 is the same PA6 tap
  with OP1 x1 (x0.01). f1 DC V r0: R2D 0x90 = **tap PA7 (/10)**, R25 0x40
  (OP1 x1), AD1 = OP1O - RLU (x0.1 as well, but through a different divider
  and no gain network); f1 r1: PA6, AD1 = SENSE - PB5 with the input buffers
  (R24 0x07). r2 / r3 (R2C 0x90 / 0x99: PA5, PA4+PA5) are identical taps in
  both functions with OP1 x1 and AD1 = SENSE - PB5. So a merged row at r1..r3
  is the AC V row plus the DC V row's AD1 bytes (what the stock's AC+DC DC
  phase already writes: R32 = 0x38 after the f1 row, acdc 2.5). At **r0 the
  two functions use different taps and OP1 gains**, so a merged r0 row has
  two candidates, neither of which is the stock DC V r0 path: (i) the AC V r0
  row (PA6, OP1 x10 to PB4) plus AD1 = SENSE - PB5 on the PA6 tap, which is
  exactly the DC V **r1** AD1 configuration and can use its factory gain, at
  10x less signal (1 AD1 count = 16.7 uV at the jack, ~50 counts of noise =
  0.8 mV = 8 digits of the 6 V display per 100 ms conversion, so it needs
  averaging or OSR 32768); (ii) AD1 = OP1O - RLU after the x10 network, full
  resolution but a per-unit gain to self-calibrate against the DC V function.
  Section 8 test 7 compares them.

## 4. Error propagation for a combined reading

Total T = sqrt(AC^2 + DC^2). With independent relative errors eAC = dAC/AC
and eDC = dDC/DC:

```
dT/T = (AC^2 * eAC + DC^2 * eDC) / (AC^2 + DC^2)
```

| signal | AC^2 share | example: eAC = 0.5 %, eDC = 0.03 % (AD1) | eDC = 0.1 % (AD2 after gain cal) | eDC = 0.7 % (DC-coupled analog RMS, Brymen) |
|---|---|---|---|---|
| mains, DC offset 10 % of AC | 0.99 | 0.50 % | 0.50 % | 0.50 % |
| AC = DC (the acdc-doc PWM) | 0.5 | 0.27 % | 0.30 % | 0.60 % |
| supply with 10 % ripple (DC dominant) | 0.01 | 0.035 % | 0.10 % | 0.70 % |

Counts add the same way: a DC-part offset of n digits shifts T by
n x DC / T digits. The HY3131 has no DC-coupled RMS converter, so the Brymen
column is only what option (d) without a precise DC calibration would look
like; the real choice on the DM40 is between the AD1 column and the AD2
column, and the table says the difference is **0..0.07 % of the total**, at
most 7 digits on 10 000, and only when DC dominates. That is the whole
accuracy argument for keeping AD1 in the loop - real, but small against the
AC spec; it does not justify a 1.9 s cycle on its own.

The second error term that only the separate-path method has: **ripple
leaking into the DC part**. A ripple of amplitude A_r at an off-notch
frequency reaches the DC reading attenuated by H(f) (section 3); the DC part
then wobbles by +-A_r x H(f), which the total sees weighted by DC / T. With
H = -40 dB and AC = DC: +-1 % of the DC part, +-0.5 % of the total, as flicker
between readings, unless the readings are averaged. Hioki publishes this as
"+2 % rdg below 45 Hz" and Fluke as 2 % + 80 in the 20-45 Hz AC+DC band.

The third term, only for a software RMS of a DC-coupled stream (option (d)):
over a window T_w that does not hold an integer number of cycles, the mean
square of D + A sin(wt) carries a cross term 2DA x mean(sin) with
|mean(sin)| <= 1 / (pi f T_w). Relative to D^2 + A^2 / 2 at D = A that is
**0.42 / (f T_w)** on the mean square: 8 % (4 % on the RMS) at 50 Hz over
106.7 ms, 0.85 % (0.42 %) over 1 s, 0 when the window is an integer number of
cycles. The hardware path has no such term because the HPF removes D before
squaring; its own residual, (A^2/2) x sinc(2 f T_w), is 1/(6 pi f T_w) ~ 1 %
of the mean square at 50 Hz over 106.7 ms - yet the device shows the hardware
RMS flat to one count at 40 Hz (acdc 4), so the RMS block's window is longer
or shaped; a plain box in the MCU would not match it without synchronisation.

## 5. The options on the DM40

Notation: d = display digit (60 000 counts: 0.1 mV on 6 V, 1 mV on 60 V);
"cycle" = time until both parts have been refreshed. Accuracies are the
expected contribution of the method, **not** a spec for the meter; the AC
part's accuracy is the stock AC V path's in every option but (d).

### (a) The stock alternation (method A, the opendm40 default; acdc 2-3, 8)

* Rows: AC phase = f0 AC V row (AD2 + AD3 sum(X*Y), RMS/LPF, AC-coupled PB4),
  800 ms settle + 4 RMS results; DC phase = f1 DC V row + R32 = 0x38, AD1 at
  OSR 8192 chopped, 600 ms settle + 3 results, the last shown unfiltered.
  Cycle 1.8..2.2 s, mean 1.91 s.
* DC accuracy: the DC V function's (factory gain and offset, AD1 alone in the
  row: note 7 does not apply) minus the averaging: one 100 ms conversion, no
  filter -> the full ripple leak H(f) of section 3 and ~50 counts of noise
  (< 1 d). 50/60 Hz: -75 dB, negligible. Off-notch LF ripple: up to a few %
  of its amplitude (INFERRED), as reading-to-reading flicker of the DC part.
* AC accuracy: the AC V function's. The AC-coupled node is left charged by
  the DC phase so no re-settle transient (acdc 5, 10.3 after the AD3-first
  fix).
* Low-frequency limit: the AC path's (HPF unknown, ~45 Hz by the 121GW
  analogy; 20 Hz at reduced accuracy) and the DC path's leak.
* Update: **~1.9 s** for a complete cycle; the shown total is recomputed on
  every result (acdc 2.6), so the display moves ~7 times per cycle but each
  part is ~1 s old. A step on the input needs up to two cycles (~4 s) to
  settle both parts; the AUTO coupling's first decision waits until the AC
  part has settled on one side of |DC| (dmm.rs `AUTO_SETTLE_K`, `AUTO_TIE`;
  10.9 item 2), ~2 s with A.
* Calibration: none beyond the factory record.
* Costs: a relay pulse and a 22-byte row write every ~1 s (acdc 7 items 1-3),
  no frequency display, the fast logger / wave view refuse AC+DC (feasibility
  7.1), a range change restarts the phase (600-800 ms).
* Industry match: the Fluke / Hioki camp, but ~4x slower than Hioki's 2.5/s
  because the DM40 re-settles the analog path at every switch.

### (b) One fixed row: AD1 (DC) + AD2/AD3 + RMS (AC), both on continuously

* Row: the f0 AC V row with the f1 DC V row's AD1 bytes merged (R22 OSR /
  chop, R23 ENAD1 + buffers, R24 AD1 mux SENSE - PB5 at r1..r3, R28
  references), written with all converters off, then AD3, AD2, AD1 last
  (the power DC row's order, power 3.12), ENAD2 / ENAD3 never toggled while
  AD1 is on (note 7). Legal by note 8 (AD2IN = AD3IN = AGND). At r0 the AC V
  row sits on the PA6 tap with OP1 x10 while DC V r0 uses PA7: the merged r0
  row reads AD1 either as DC V r1 does (SENSE - PB5 on PA6, factory r1 gain,
  10x less signal) or after the x10 network (own gain) - section 3 last
  bullet, section 8 test 7.
* DC accuracy: AD1 with the factory DC V gain, **plus the note-7 offset**
  (0.3..0.9 k counts observed = 5..15 d at 6 V and 60 V). If the shift is a
  stable constant of (range, row) -> a per-range zero removes it to the
  AD1 noise level (< 1 d): measure AD1 in the row with AD2/AD3 off, then
  enable them in the row's order and measure again; the difference is the
  zero. Two levels (0 V and ~half range) tell offset from gain. If it wanders
  with time / temperature / input level -> (b) is not better than (c) and
  costs a conversion family more. (Device: it wanders, 10.5 e1.) Ripple leak:
  the same H(f) as (a), but now the DC readings come at 10 S/s continuously,
  so they can be averaged (e.g. 5 results = 0.5 s, another sinc with notches
  at multiples of 2 Hz: -14 dB more at 25 Hz, -10 dB at 37 Hz, INFERRED) or
  the OSR raised to 32768 (2.5 S/s, -24 dB more) without lengthening a
  "phase" - there is none.
* AC accuracy: the AC V function's, if AD1 converting does not disturb AD2 on
  PB4. Evidence it does not: the stock's DC phase (AD1 running, AD2 off) left
  the PB4 node at 0.08 V (acdc 4); the sentinel's AD1 inside the AC V row did
  not change the AC readings (power 4.6-4.7 device checks); the power DC row
  reads AD1 and AD2 on **the same node** (power 3.12 device results). The
  reverse direction (AD2 on -> AD1 offset) is the note-7 effect above.
  Measured in the merged V row: AD1 converting does not move the RMS (10.5 e2).
* Low-frequency limit: the AC path's, unchanged.
* Update: AC every ~107 ms (RMS/LPF cadence), DC every 100 ms (OSR 8192
  chopped) or 400 ms (OSR 32768); total recomputed on either -> **~10
  readings/s raw, 5/s on the display**, the Hioki figure x2. No settle after
  the first ~2 s (PB4 recharge, acdc 6a) except after a range change (one row
  write, 550-800 ms, then AD1 last).
* Calibration: the factory DC V and AC V records as today, plus the per-range
  note-7 zero (self-measured at mode entry or stored once per unit, like the
  POW:ZERO slots, power 3.12).
* Extras it enables: frequency from the counter on PB4 in parallel (the stock
  restarts it every 350 ms in AC+DC anyway, acdc 2.1), a bargraph of the
  total, the logger at the AC cadence, no relay pulses.
* Industry match: Keysight U1282A-like (DC path's error simply added), at the
  Hioki update rate or better.
* **Outcome:** built as method B (10.1). On the device its per-entry zero
  takes one of two values (6 V +12.6 / -30.5 mV) and leaves up to 7.5 digits
  on 60 V and 16.6 on 1000 V: it fails the 2-digit rule (10.5 e1), dropped;
  the code stays for comparison. B's AD1 does not move the RMS (10.5 e2).

### (c) DC from the AD2/AD3 path instead of AD1

* Row: the test 6.5 row (DC V tap, OP1 x1, AD2 on OP1O, DC-coupled) for the
  DC part, read either as the LPF (DC mean, SDO23 = 0) **or** as the mean of
  single AD2 conversions in the MCU (what 6.5 measured). But the AC part
  needs AD2 on the **AC-coupled PB4 node** with the HPF+RMS, and one AD2
  cannot sit on two nodes: either AD3 takes the DC node (AD3 = OP1O - AGND
  DC-coupled, AD2 = PB4 - AGND, SDO23 switching the single data/LPF register
  between them, losing the sum(X*Y) noise cancellation and the LPF
  simultaneity, like the power meter's P -> V -> I sequencing, power 3.4), or
  the two statistics alternate on AD2 (a row write per switch, back to a
  cycle). The tempting variant - AD3 on the DC-coupled node, AD2 on PB4, R34
  frozen, LPF of AD3 for DC and RMS of AD2 for AC - does not exist in the
  register map: SDO23 selects **both** the LPF source and the RMS statistic
  (SDO23 = 1 -> LPF[AD3] and RMS = sum(Y^2), DS 13.2 table), so RMS (AC from
  AD2) and LPF (DC from AD3) cannot be read in one configuration. The MCU
  would take the DC part from single AD3 conversions (R03..R05 with SDO23 =
  1) while the RMS block runs on AD2 (SDO23 = 0), toggling SDO23 between the
  two reads - whether that disturbs the running RMS accumulation is UNKNOWN.
  **The register plumbing of (c) is the open design question, not the
  converter.** The variant that needs no plumbing is (c) as the *fallback DC
  source* for (b)'s row, read as single AD2 conversions on the DC node only
  when AD1 cannot be trusted - see section 7.
* DC accuracy: 6.5 results: linear <= 0.003 % of range, noise 3..10 codes =
  1.5..5 d on every range, gain +0.27..+1.79 % **before** calibration, 10-30
  ppm/C gain drift (0.03-0.09 % over 30 C) against AD1's 2.5 ppm/C, offset
  from OP1 (chopper 20 uV, DS p. 11) and the reference nodes (~150 codes
  between RLU and PB5 seen on T1, power 3.12 "LPF zeros"). After a per-range
  gain + offset self-cal against the DC V function at entry (~3 s, the
  `POW:VOLT:CAL` DC kind): **~0.05..0.1 % + 2..5 d**, temperature-dependent.
  Ripple leak: the LPF's (<= 0.5 % at 40 Hz observed, shape UNKNOWN) or, for a
  software mean over a 100 ms box, sinc(f x 0.1 s): -26 dB at 50 Hz, -24 dB at
  45 Hz - **worse than AD1 at the mains frequencies** unless the mean is taken
  over an integer number of cycles (frequency counter) or over >= 1 s.
* AC accuracy: the AC V function's (same node and block), minus the sum(X*Y)
  cancellation if AD3 is taken away (sum(X^2) was 9919 vs sum(X*Y) 9921.5,
  both stable to 1 count once the enable order was fixed, acdc 10.1 - so
  nothing measurable is lost).
* Low-frequency limit: the AC path's.
* Update: 107 ms for both parts (if the plumbing allows both in one
  configuration), else like the power meter's sequencing (0.9-1.3 s).
* Calibration: a new per-range gain/offset for the DC node (self-cal against
  AD1 at entry, or stored per unit), re-done with temperature.
* Industry match: the 121GW / Brymen direction (DC through the AC
  converter's family) with a far better converter (19-bit sigma-delta vs an
  analog RMS chip), hence ~0.1 % instead of 0.7-1 %.
* **Outcome:** built as method C (10.1), and the plumbing question resolved
  without SDO23 switching: AD2 and AD3 both on the DC-coupled node, the LPF
  gives the DC part and the RMS block's HPF takes the DC out of sum(X*Y), so
  one fixed row gives both parts every LPF block (~9 readings / s). The DC
  part is anchored per range on the stock DC V reading and its gain measured
  between two such readings (10.6-10.14); the AC part goes through the
  factory AC V calibration. On the device it meets the 0.05 % + 5 digit rule
  on V (10.15).

### (d) DC-coupled RMS computed in the MCU from the AD2 stream (E7 row)

* Row: the 6.5 DC-coupled V row, AD2 alone at 76.8 kS/s (or 9.6-19.2 kS/s),
  the stream ISR of feasibility 11 (DMA frames, phase-locked) feeding a
  running sum(x) and sum(x^2) in 64-bit: 19-bit codes squared over 76 800
  samples fit in 2^54. CPU: one multiply-accumulate per 13 us is small
  against the ISR's own ~6 us per sample at 76.8 kS/s (feasibility 11.12);
  the zero-wait flash budget (region B, [native-boot.md](native-boot.md)
  "Memory budget") is the constraint to check, not the arithmetic.
* What it gives: the **true** AC+DC rms in one number (no hypot, no two
  paths) plus the mean (DC) and sqrt(ms - mean^2) (AC) from the same window;
  any waveform, crest factor limited only by the +-1.25 V / IG window at the
  node (6 V range at the /10 tap: +-12.5 V peak at the jack, so a 6 V rms sine
  on a 3 V offset still fits). Resolution: 14 ENOB per sample, the noise
  power (32 codes)^2 = 1024 code^2 adds to a mean square of ~10^10 at half
  scale: 10^-7, nothing; the noise on the DC mean is 6.5's 3..10 codes.
* DC accuracy: as (c) (same converter, same gain cal needed, same drift). AC
  accuracy: as the hardware RMS **only if** the window handles the two
  residual terms of section 4: the cross term 2 x DC x AC / (pi f T_w) (4 %
  on the RMS at 50 Hz over 107 ms, 0.4 % over 1 s) and the double-frequency
  term. Both vanish for an integer number of cycles: the frequency counter
  (CTF, restarted every 350 ms by `freq_secondary`) gives the period, the
  window is cut to N cycles. Without sync a 1 s box plus an EMA is a
  0.4 %-class AC reading, worse than the hardware's.
* Low-frequency limit: the best of all options once synced - down to a few Hz
  with a long window (the 3458A's SYNC mode goes to 1 Hz the same way) - and
  the only option with a DC-coupled AC part (no HPF, no 0.5 Hz node).
* Ripple on the DC part: the mean over an integer number of cycles rejects
  the fundamental and its harmonics exactly; off-sync the box sinc applies.
* Update: per window: 100 ms .. 1 s, plus an EMA.
* Calibration: the DC node's gain/offset (as (c)); the AC scale is the same
  gain (one converter for both), which is the attraction: one calibration.
* Costs: a new reading machine in the ISR, the autorange on peaks, the HPF /
  RMS block unused for V (the stock's AC V path stays for the plain AC
  function), and no stock reference to pixel-match. **Experiment first, main
  path later if the synced window proves out.**
* **Outcome:** not built. The wave view's Vrms of the DC-coupled V row
  (feasibility 10, 13) is a box RMS of the same stream, with the sinc^3
  droop of the slower rungs (feasibility 3.2).

### Options table

| | (a) stock alternation | (b) fixed row, AD1 + AD2/AD3 RMS | (c) DC from the AD2/AD3 path | (d) MCU RMS of the DC-coupled AD2 stream |
|---|---|---|---|---|
| DC part accuracy | DC V function (factory), 1 unfiltered 100 ms conversion | DC V function + note-7 zero (unstable on the device, 10.5 e1), averaging possible | ~0.05-0.1 % + 2-5 d after self-cal; 10-30 ppm/C | as (c) |
| AC part accuracy | AC V function | AC V function (if AD1 does not disturb PB4: test 2) | AC V function (sum(X^2) instead of sum(X*Y) if AD3 moves) | hardware-class only with cycle-synced windows; else ~0.4 % |
| 50/60 Hz ripple on the DC part | -75 dB (AD1 10 S/s, DS) | -75 dB, more with averaging / OSR 32768 | LPF: UNKNOWN (<= -46 dB at 40 Hz observed); 100 ms box: -26 dB | exact at sync; box sinc off-sync |
| off-notch LF ripple on the DC part | up to -33..-45 dB (INFERRED), unfiltered | same, averaged (-10..-14 dB more) | as above | exact at sync |
| low-frequency limit | AC path (~45 Hz spec-class, 20 Hz degraded) | same | same | lowest; set by the window length |
| update | 1.9 s cycle; parts ~1 s old | ~10/s raw, 5/s display | 107 ms (as built: no SDO23 switching needed, 10.1) | per window 0.1-1 s |
| settle after entry / range change | 600-800 ms every phase | ~2 s once (PB4), then 0.55-0.8 s per range change | as (b) | as (b) |
| calibration beyond the factory record | none | per-range note-7 zero | per-range gain + offset of the DC node | same + nothing for AC |
| new unknowns | none | note-7 stability; r0 OP1 gain; AD1 -> PB4 crosstalk | SDO23 register plumbing; LPF window shape; drift | window sync; ISR budget |
| industry analogue | Fluke 289 / Hioki (slower) | Keysight U1282A at Hioki speed | 121GW / Brymen with a better converter | 3458A SYNC |

## 6. What the stock parity costs and buys

The stock alternation is the only one of the four that needs **no new
calibration constant and no new row**, which is why it is right for the
"replicate stock first" baseline. Its price is the 1.9 s cycle, the relay
pulses, no frequency, no logger / wave in AC+DC, and a DC part that is a
single unfiltered conversion. Every meter in section 1 that chose the
separate-path method runs both paths **continuously** and halves the display
rate at most; none re-settles its analog front end twice a second. The DM40
does because the stock firmware had one AD2 node for AC (PB4, AC-coupled)
and chose to switch rows rather than keep AD1 on next to it - plausibly
because of note 7, whose effect is measured in feasibility 13.1-13.2.

## 7. Recommendation

The recommendation as written on 2026-10-22, with the status of each item:
1 kept (A is the default); 2 built as method B and dropped by its device
rule (10.5); 3 built as method C, passes on V (10.15), the default waits for
the current check (10.16); 4 not built; 5 built into C (up-range on AD2's
peak hold at 90 % of its span, 10.6 item 2). For A: C's A path is built
(the cheaper (c)-A below), not yet device-checked (10.16).

**V (6 V .. 1000 V).**

1. Keep (a) as the stock-parity mode (pixel-diff baseline, the AUTO coupling
   already built on it, acdc 11).
2. Build **(b)** behind a constant: the AC V row with AD1 merged (r1..r3 from
   the stock's own DC-phase bytes; r0 after the OP1-gain check of section 3),
   enable order AD3, AD2, AD1 last, nothing toggled while measuring. Decision
   rule from the note-7 test: if AD1(AD2 on) - AD1(AD2 off) is **constant per
   range to within ~1-2 d over time, two input levels and a temperature
   excursion**, store it as a per-range zero (POW:ZERO-style slots, or
   measured at mode entry in ~1 s with AD1 alone then AD2/AD3 added) and ship
   (b): DC part = factory DC V calibration minus the zero, averaged over
   0.5 s (or OSR 32768), AC part = the stock RMS (sum(X*Y), unchanged),
   total on every result, ~5 readings/s, frequency from PB4 in parallel.
   Expected: DC part within the DC V spec (0.03 % + 5 d) plus the zero's
   residual, AC part = AC V spec, total per section 4.
3. If the shift wanders: **(c)** with the DC part from the DC-coupled node,
   per-range gain/offset self-cal against the DC V row at entry (the
   `POW:VOLT:CAL` DC kind, ~3 s, repeated on a temperature change or every N
   minutes), after settling the SDO23 plumbing (AD3 on the DC node, single
   conversions for DC, RMS on AD2). Expected DC part ~0.1 % + 5 d; the total
   is within 0.07 % of the AD1 version even on a pure DC input (section 4).
4. **(d)** as a DBG experiment on the existing stream (sum / sum-of-squares
   in the ISR, window = N periods from the counter): if the AC part matches
   the hardware RMS to < 0.1 % at 50 Hz and holds at 20 Hz, it becomes the
   low-frequency / arbitrary-waveform AC+DC and the "true rms" the Fluke blog
   describes; otherwise it stays a scope measurement.
5. Autorange: on the total (stock, Hioki) **and** on the AD2 node's peak
   window (|DC| + sqrt(2) x AC_rms <= 1.25 V / IG at the node): up-range when
   either trips (Fluke's rule for the parts), down only when the total is
   below 10 % of range. The 121GW's warning (AC autorange failing with a
   large DC offset) is the failure this avoids.

**A (600 uA .. 10 A).**

* The shunt node is DC-coupled at AD2 already (AC A rows: AD2 = OP1O - PB3,
  LPF = I_dc with a constant offset, acdc 6b, feasibility 1.3), and the power
  meter's DC row runs **AD1 + AD2 + AD3 on this node in one row on the
  device** (power 3.12, device-checked). So for A the fixed row exists:
  AD1 = OP1O - PB3 for I_dc with the factory DC A gain (0x94) and the row's
  own AD1 zero (POW:ZERO, note 7 again), AD2 + AD3 RMS for I_ac with the
  AC A RMS gain. That is (b) with the hardware already proven. The cheaper
  (c) is also available here at no plumbing cost: I_dc = LPF of the same AD2
  (the RMS gain applies to the same path; the factory LPF gains are 1.0,
  "never calibrated", power 3.12) with the open-input zero - ~0.1-0.2 %, fine
  against the AC A spec, and it frees AD1 for the sentinels. Pick (b)-A if
  the note-7 zero is stable (same test), else (c)-A.
* (d) on A is the natural inrush / motor-current measurement (the inrush
  feature already streams the 10 A AC row, `inrush-current.md`); a DC-coupled
  rms over N cycles is a small addition there.

**mV.** The AC mV row is DC-coupled on OP2O (acdc 6a: LPF = the full DC) and
the stock has no AC+DC mV; the same (c)-style DC from the LPF applies if ever
wanted. Hioki and the 121GW offer no AC+DC on mV either.

## 8. Device experiments that settle it (in order)

All through DBG SCPI on opendm40, the MR9270S+ calibrator on V/COM (<= 30 V), the
generator for AC; log with `dm40scpi.py` / `tools/acdcmeth.py`. Status: 1, 2,
5, 6, 7 ran (feasibility 13.2; 10.5 e1 / e2 / e5 / e6 / e7); 3 and 4 need a
generator and were not run.

1. **Note-7 stability.** Per range r0..r3, input 0 V and ~half
   range: AD1 alone; AD1 with AD2 (and AD3) enabled in the row's order; 60 s
   each, then again after 10 min and after warming the meter (backlight on,
   charger): the shift in counts, its sd, its dependence on level (offset vs
   gain) and on OSR (8192 / 32768). Pass for (b): shift constant to <= 2 d of
   the range over all of it.
2. **AD1 inside the AC V row.** The merged row of (b) at r1 (60 V) and r2
   (600 V): 10 V DC + 5 V rms 50 Hz. AD1 (zero applied) vs the DC V function;
   sqrt(RMS) with AD1 on vs off (does AD1 converting move the PB4 node or the
   RMS?). Pass: AD1 within 5 d of DC V, RMS within 1 count of AD1-off.
3. **Ripple leak of the DC part.** DC 5 V + 1 V rms at 25, 37, 45, 50, 55,
   100, 105, 400 Hz on the 60 V range: sd and p-p of (i) AD1 single results
   at OSR 8192 chopped and 32768, (ii) the LPF of the DC-coupled AD3/AD2 node,
   (iii) the MCU mean over 100 ms and over N cycles. Gives H(f) for the three
   DC methods; the datasheet only has 50/60 Hz +-1 Hz. Decides the averaging
   depth of (b) and whether (c)'s LPF is usable at mains frequencies.
4. **Low-frequency AC part.** 20, 30, 45 Hz, 5 V rms with 0 and 5 V DC: the
   hardware RMS result spread (window shape), the MCU box RMS over 107 ms
   and 1 s, the MCU N-cycle RMS. Settles the HPF/RMS low-frequency behaviour
   that the datasheet does not give and the (d) sync requirement.
5. **(c) gain stability.** The 6.5 row's gain vs the DC V function once per
   minute for 1 h with the meter warming: drift in ppm/C against the 10-30
   ppm/C datasheet figure; how often a self-cal would be needed.
6. **Entry and range change of the fixed row.** From DC V into (b): time to
   the first RMS result within 1 count of steady state (PB4 recharge, expect
   ~2 s); a range change r1 -> r2 with the row rewritten AD1-last: AD1 and RMS
   settle times, and whether the note-7 zero of the new range holds (test 1
   per range).
7. **r0 row bytes.** The 6 V range on the AC V r0 row (PA6 tap, OP1 x10 to
   PB4): (i) AD1 = SENSE - PB5 (the DC V r1 bytes, factory r1 gain) vs (ii)
   AD1 = OP1O - RLU after the x10 network (self-calibrated gain), each on
   1 / 3 / 5 V DC against the DC V function, with 2 V rms 50 Hz added to see
   the AC part unchanged: which reads DC within the DC V spec and keeps the AC
   V spec; one of them is the r0 row of (b). (The stock DC V r0 path, PA7 with
   OP1 x1, cannot be merged: it needs a different tap than the AC V r0 row.)

## 9. Sources

HYCON documents in [hw/datasheets/](../../hw/datasheets/README.md): the HY3131
datasheet [`hycon-hy3131-datasheet-v10.pdf`](../../hw/datasheets/hycon-hy3131-datasheet-v10.pdf)
(electrical table p. 11-12; 13.1 AD1 p. 29-30 incl. note 7; 13.2 AD2/AD3,
LPF, RMS p. 31-34), APD-DMM003
[`hycon-hy313x-configurations-apd-dmm003.pdf`](../../hw/datasheets/hycon-hy313x-configurations-apd-dmm003.pdf)
(3. DCV p. 11, 4. ACV p. 17 and the ACV block diagrams: AD2 -> HPF -> LPF /
RMS), APD-DMM001
[`hycon-hy313x-eva-test-tool-manual-apd-dmm001.pdf`](../../hw/datasheets/hycon-hy313x-eva-test-tool-manual-apd-dmm001.pdf)
(ENOB definition eq. 2-3; SETDMM calibration of ADC1 or RMS), APN-HY3131001
[`hycon-hy313x-spi-appnote-apn-hy3131001.pdf`](../../hw/datasheets/hycon-hy313x-spi-appnote-apn-hy3131001.pdf)
(SPI MOSI timing only). Not in the repo: HYCON's `HY3131-ENOB.zip`
(`Config/ACV/HYCONFIG-0.txt`, `Config/DCV/HYCONFIG-0.txt`) and the three
`*DemoCode.zip` (readme placeholders only). Sibling docs as cited.

Web (retrieved 2026-10-22):

* Fluke 287/289 Calibration Manual (AC+DC definition, AC and DC/AC+DC voltage
  and current tables, footnotes): https://assets.fluke.com/manuals/287_289_cmeng0000.pdf
* Fluke 287/289 Users Manual (Measuring AC and DC Signals, ranging in AC+DC):
  https://assets.fluke.com/manuals/287_289_umeng0200.pdf
* Fluke 187/189 Users Manual ("rms total of ac and dc measurements"):
  https://assets.fluke.com/manuals/187_189_umeng0100.pdf
* Fluke 83V/87V detailed specifications ("AC conversions are ac coupled"):
  https://valuetronics.com/cdn/shop/files/Fluke_83V_87V_PRV240.pdf
* Fluke, "Facts about true-rms measurement" (AC-coupled vs ac+dc, the 189,
  the sqrt(AC^2 + DC^2) formula): https://www.fluke.com/en/learn/blog/digital-multimeters/facts-about-true-rms-measurement
* Keysight, "Tip 2: Measuring an AC signal with a DC offset" (manual
  combination with >= 10 PLC beats the built-in function):
  https://www.keysight.com/mx/en/lib/resources/training-materials/tip-2-measuring-an-ac-signal-with-a-dc-offset-458371.html
* Keysight U1280 Series data sheet 5992-0847EN (AC and AC+DC voltage p. 11,
  current p. 12): https://rapid-tech.com.au/wp-content/uploads/2021/10/Keysight-U1282A-Datasheet.pdf
  (mirror of https://www.mouser.com/datasheet/2/878/5992_0847EN-1143121.pdf)
* Keysight 3458A data sheet (ACDCV additional error tables, reading rates):
  https://assets.testequity.com/te1/Documents/pdf/keysight/3458A-ds.pdf ;
  the DC-coupled synchronous sampling statement is from the Keysight
  knowledge-centre / user's guide excerpts found via search
  (https://www.keysight.com/us/en/assets/9018-01001/user-manuals/9018-01001.pdf,
  not retrievable as text here).
* Brymen BM867s/BM869s User's Manual (p. 18-20: DC Voltage, AC Voltage, "DC
  AC & AC+DC AC Voltage" tables):
  http://www.brymen.com/images/ProductsList/BM860s_List/BM860s-manual-2-033Ed2-ES636-Print1.pdf ;
  lygte-info review (two DC paths): https://lygte-info.dk/review/DMMBrymen%20BM869s%20UK.html
* EEVblog 121GW User Manual rev. 2025-03-03 (p. 15-17 specifications, p. 36
  modes, autoranging caution): https://www.eevblog.com/files/EEVblog-121GW-Manual.pdf
* Hioki DT4281/DT4282 Instruction Manual DT4281A981-12 (p. 85 display update
  rate, p. 86-89 accuracy tables and DC+ACV notes):
  https://www.hioki.com/download/30687
* UNI-T UT61E+ retailer specification (secondary): https://toolboom.com/en/digital-multimeter-uni-t-ut61eplus/ ;
  UT171B: https://toolboom.com/en/digital-multimeter-uni-t-ut171b/ ;
  HY3131 in the UT171 series (forum): https://eevblog.com/forum/testgear/eevblog-121gw-discussion-thread/2025
* Owon XDM1041 (functions list): https://www.elektor.com/products/owon-xdm1041-true-rms-multimeter
* DTM0660 datasheet (mirror): https://shaddack.mauriceward.com/projects/reveng_multimeters/chips/DTM0660.pdf
* Fortune FS9922-DMM4 datasheet rev 1.3: https://cdn.hackaday.io/files/21300911299200/FS9922-DMM4-DS-13_EN.pdf
* Cyrustek ES51966 datasheet (ES636 for true RMS): https://shaddack.mauriceward.com/projects/reveng_multimeters/chips/ES51966.pdf
* Cyrustek ES51997 (listing only, PDF blocked): https://www.alldatasheet.com/datasheet-pdf/pdf/456328/CYRUSTEK/ES51997.html
* Alientek DM40 DC spec +-(0.03 % + 5 d) (forum; the official manual is a
  220 MB RAR behind a JS button on en.alientek.com and was not fetched):
  https://www.eevblog.com/forum/testgear/alientek-dm40c-multimeter-(dm40a-dm40b-dm40c)

## 10. Prototype: methods B and C in opendm40 (2026-10-22, `src/acdcm.rs`)

Behind a RAM-only switch (`DBG:ACDC:METH`), default A (the stock
alternation, unchanged), so a device session compares them. Built only with
the cargo feature `acdc-meth` ([`Cargo.toml`](../Cargo.toml); `./build.sh
native --features acdc-meth`): the default build has A alone and refuses
`DBG:ACDC:METH` / `STAT?` / `ZERO` / `R0` (undefined header);
`tools/acdcmeth.py`, `emu/hy3131/acdcmethtest.py` and `couptest.py --meth`
need such an image. Device input
before the prototype (feasibility 13.2): with AD2 + AD3 + RMS on for the
whole DC V row AD1 shifts by 30..90 x its noise, offsets -2.9 mV / +17 mV /
+0.17 V / -1.6 V on 6 / 60 / 600 / 1000 V (0.03..0.16 % of the range,
repeating to ~1 %), and a gain shift up to -0.18 % on 60 V that did **not**
repeat. So **C is the primary candidate**; B stays for the comparison (its
zero removes the repeating offset, not the gain). 10.1 is what runs now;
10.2-10.16 are the dated record of how C got there.

### 10.1 What runs

| | V (6..1000 V) | A (600 uA..10 A) |
|---|---|---|
| A | stock alternation ([`dmm.rs`](../src/dmm.rs), acdc 2-3) | same |
| B | AC V row + the DC V row's AD1 bytes (R22 / R23 / R24 / R28, AD1IG), OP1's chopper held (AD1 converting starts its 2 kHz square at OP1O -> PB4 -> the RMS), written AD1 off, AD3, AD2, AD1 last; nothing toggled after. DC = mean of the last 5 AD1 results (0.5 s) through the DC V calibration, minus a per-range zero = (merged-row AD1) - (the stock DC V row's reading, AD1 alone, R32 0x38 as the stock DC phase), taken at the first visit of a range in the session or fixed with `DBG:ACDC:ZERO`. AC = the stock RMS (sum(X*Y) on PB4, factory AC V cal). 6 V: `DBG:ACDC:R0 1` = candidate (i) AD1 on SENSE - PB5 at the /100 tap (DC V 60 V's input and calibration), `2` = (ii) OP1O - RLU after the x10 network (DC V 6 V's calibration, gain nominal, offset in the zero). Failed its device rule (10.5); kept for comparison | AC A row + the DC A row's AD1 bytes (AD1 = OP1O - PB3), OP1's chopper held (AD2 sits on OP1O here), the AC row's pre-filter (R33) kept; zero against the stock DC A row as V |
| C | E7's DC-coupled row (`ad2exp::dcv_row`: AD2 on OP1O, OP1's chopper held) with AD3 on the same node (R35 0x14), RMS / LPF on (R29 0xF8, R34 0x88) and AD2's peak hold. **DC part** = LPF x per_code + offset. Anchor per range and session (`am_cal_step`): the stock DC V row's own reading (3 results, what READ? shows) against 4 LPF results of this very row. Gain: from the internal zero (OP1+ on AGND) when the span is >= 10 k codes and within 3 % of the table (dmm.rs `DCV_AD2_GAIN`), else the table; then, once the input has settled >= 10 k codes from a reference point of the session (`am_refine`: LPF slope < 1 code / s over ~3 s, >= ~5 s at the level, no real AC on the node), a second stock DC V reference and the gain between the two references, which replaces the internal zero's when its span is >= the one in use, or when it shows the internal zero biased beyond the noise plus a creep allowance on the lag side (CREEP_K, 10.14). Gains kept per range until power-off. **AC part** = sqrt(RMS) through the factory AC V gain and offset of the range, used only when sqrt(RMS) <= half of AD2's peak-hold p-p (else the RMS is in its HPF transient or garbage: on a steady node the last RMS value or the peak bound stands in; no usable RMS with AC on the node for 2.5 s: up a range). Up-range also when the peak hold reaches 90 % of AD2's span (one window for DC + AC, section 7 item 5), when \|DC\| alone is over the range, or on the total; down only with a usable AC part. Both parts every LPF block (106.7 ms), no row switching, no AD1 while measuring | the stock AC A row unchanged (AD2 on the DC-coupled shunt node): AC = RMS (factory AC A gain), DC = LPF x the same gain, corrected against the stock DC A row at entry: a gain factor when the current is >= 10 % of the range (within 5 %), else an offset. One point only: a zero-current session gets the offset, a loaded one the gain. Not device-checked (10.16) |

Common: mV (modes 2 / 3) stays on A (no merged mV row); no misconnect
sentinel in the method rows (the AC V one uses AD1, the DC V one takes R20
which the frequency counter needs); one reading per RMS result (~9.3 / s);
the frequency counter runs (Hz in `DBG:ACDC:STAT?`, not shown); a range
change writes the new range's method row (B: and its reference first if
the range has no zero yet; C V: its anchor, or the kept one on a revisit);
AUTO coupling (sub-mode 3) evaluates once both parts were refreshed since
its last evaluation (~107 ms; the decision rule in 10.9 item 2).

SCPI: `DBG:ACDC:METH A|B|C` / `?` (a change restarts AC+DC / AUTO),
`DBG:ACDC:STAT?`, `DBG:ACDC:ZERO k,v|AUTO` / `?`, `DBG:ACDC:R0 1|2`,
`DBG:ACDC:RANG <MF item>|AUTO`, `DBG:ACDC:TRACE` / `TRACE?`; the fields are
listed in the DBG block of [`src/scpi.rs`](../src/scpi.rs). Host:
[`tools/acdcmeth.py`](../tools/acdcmeth.py) runs the device experiments (e1 .. e11, e8s,
e8up, tc6, `--trace`).

### 10.2 Emulator model changes (HY3131.cs)

* **Note 7**: AD1 on the V divider (DC V rows, the AC V rows' SENSE - PB5 /
  OP1O - RLU) reads the device's per-range offsets while AD2 or AD3
  converts (`N7Volts`, `n7:` scale, default on); `n7g:` a gain shift drawn
  at every AD1 enable (the non-repeating 60 V gain); `n7a:` codes for the
  A rows (default 0: not measured).
* **AD1's window** at OSR >= 2048: a sinc^2 sub-conversion (triangle over
  2T, T = OSR / AD1CLK) averaged with the previous one when chopped, i.e.
  |H| = sinc^2(fT) |cos(pi f T)| (section 3, INFERRED): notches every
  10 Hz at OSR 8192 chopped (40 / 50 / 60 Hz vanish), 45 Hz leaks
  ~-43 dB. Applied in DC V, DC A and the AC rows' AD1 paths (the stock
  AC+DC DC phase had no ripple leak at all before).
* **LPF window**: sinc^2 over two LPF periods (213 ms at LPFBW 111),
  INFERRED from the datasheet's 213 ms and the device bound <= 0.5 % at
  40 Hz: -51 dB at 50 Hz. In AC A and the DC-coupled AC V row.
* AC V row with AD3's inputs free (method C), AD1 = OP1O - RLU in AC V and
  OP1O - PB3 in AC A modelled. Host tests `AcdcMethods` (396 pass).

### 10.3 Emulator comparison (`emu/hy3131/acdcmethtest.py`)

One run per method, the same timed input (12 s per phase), error of the
shown total against hypot(AC, DC) of the source over 4.5 s of each phase
(its 6th to 1.5th last second), rate = AC+DC readings / s (`DBG:ACDC:STAT?`
n_rd, every method; A recomputes the total on every result of either
phase), settle = from the phase start until the total stays within 0.1 % +
2 digits (- : never inside that band in the phase, e.g. with a 0.3 % p-p
ripple, or a sample of the phase before inside the window). Note 7 on (the
device's offsets); B~ = B with `n7g:0.002` (a random draw per AD1 enable:
its errors are the draw's). The last V phase returns to the 6 V range
after the 60 V one (a range visited again: C re-runs its self-cal there,
E7's cal has one slot). Run of the commit after 03aa3da:

| phase | A err % / p-p % / rd/s / settle s | B | C | B~ |
|---|---|---|---|---|
| V DC 5 | -0.001 / 0.004 / 3.8 / 3.4 | +0.011 / 0.029 / 9.3 / 3.3 | -0.005 / 0.000 / 9.1 / - | -0.016 / 0.031 / 9.3 / 3.3 |
| V AC 2 @50 | +0.002 / 0.004 / 3.8 / 2.5 | +0.000 / 0.004 / 9.3 / 2.3 | -0.004 / 0.005 / 9.1 / 0.1 | -0.000 / 0.003 / 9.4 / - |
| V AC 1 + DC 1 @50 | -0.003 / 0.015 / 4.0 / 2.4 | +0.016 / 0.061 / 9.1 / 0.8 | -0.005 / 0.298 / 9.3 / 0.1 | +0.032 / 0.082 / 9.3 / 0.8 |
| V DC 12 + 0.5 @50 | +0.003 / 0.027 / 3.6 / 3.9 | +0.001 / 0.016 / 9.6 / 1.0 | +0.012 / 0.028 / 9.3 / 0.9 | +0.093 / 0.015 / 9.1 / 1.5 |
| V DC 12 + 0.5 @45 | +0.016 / 0.067 / 3.1 / 0.0 | +0.001 / 0.012 / 8.9 / - | +0.010 / 0.016 / 9.3 / 0.0 | +0.096 / 0.015 / 9.1 / 0.0 |
| V DC 5 again (60 V -> 6 V) | +0.001 / 0.006 / 3.6 / 2.5 | +0.007 / 0.025 / 9.1 / - | +0.010 / 0.000 / 9.3 / - | +0.111 / 0.016 / 9.4 / 1.5 |
| A DC 0.3 | +0.002 / 0.010 / 2.7 / 0.4 | +0.001 / 0.008 / 9.3 / - | -0.002 / 0.000 / 9.3 / - | +0.006 / 0.013 / 9.6 / 0.4 |
| A AC 0.2 @50 | -0.001 / 0.002 / 2.9 / - | +0.001 / 0.004 / 9.6 / 0.1 | +0.000 / 0.004 / 9.6 / - | -0.000 / 0.005 / 9.3 / 0.1 |
| A AC 0.2 + DC 0.2 @50 | -0.002 / 0.009 / 2.7 / - | +0.001 / 0.009 / 9.3 / - | -0.000 / 0.324 / 9.3 / - | +0.005 / 0.006 / 9.6 / - |
| A DC 0.3 + 0.05 @45 | +0.010 / 0.323 / 2.7 / 0.9 | -0.007 / 0.334 / 9.6 / 9.4 | +0.001 / 0.065 / 9.6 / 0.0 | +0.004 / 0.142 / 9.3 / 0.0 |

Earlier runs of the same code (a884ac9 / 6d30cff, before the fb0b347
GD32_SPI timing) gave the same picture within 0.05 %; one of them drew a
B~ gain of -0.12 % at 12 V.

Reading of it (all within the model; the LPF window and AD1's off-notch
response are INFERRED, the note-7 numbers are the device's):

* **Rate**: B and C ~9.3 readings / s against A's 2.7..4 (each of A's
  totals carries a part up to ~1.9 s old); C settles in 0.0..1 s after a
  signal change, B 0.1..3.3 s (PB4's recharge after the entry and the
  5-result mean).
* **Accuracy**: every method within 0.02 % on mean where the model has no
  gain error; B~ shows what B cannot fix: a gain shift drawn per AD1
  enable stays (+0.09..+0.11 % on 60 V and 6 V in this run, -0.12 % in
  an earlier one) - the device's 60 V gain jump would look like that.
* **Ripple at 50 Hz on the DC part**: AD1 (A, B) sits on a notch; C's LPF
  leaks (0.30 / 0.32 % p-p of the total with AC = DC at 50 Hz, the INFERRED
  -51 dB). At **45 Hz** AD1 leaks (A 0.047 %, B 0.030 % p-p with the
  5-result mean, A's 0.32 % on the A row's 45 Hz case) and C's LPF does
  better (0.016 / 0.062 %). Which one wins on the device is test 3 (needs
  a generator).
* AUTO coupling: couptest `v`, `i`, `tap`, `band` pass with `--meth A`,
  `B` and `C`.

### 10.4 Device session plan (`tools/acdcmeth.py`, calibrator on /dev/ttyACM1)

The calibrator gives DC <= 30 V and a small 40 Hz square only; 40 Hz is an
AD1 notch, so the ripple-leak and low-frequency tests (section 8 items 3, 4)
need a generator and were skipped. Experiments: `e6` entry and range-change
timing, `e1` B's zero per range (5 entries per level), `e7` B's 6 V
candidates, `e2` the 40 Hz square on 6 / 60 V (C's AC part against A's),
`e5` C re-entered every 60 s for 20 min (drift), and AUTO with C by hand.

Decision rule: C becomes the AC+DC default when e2 shows its AC part
within the AC V spec of A's (0.5 % class) and e5 its DC part within
0.05 % + 5 digits over the warm-up; B is dropped if e1's zero spread or the
level dependence exceeds 2 digits on any range. Run on 20a91d0: 10.5.

### 10.5 Device results (20a91d0, 2026-10-23 01:06-01:56, docs/device-check-2026-10-21.md "20a91d0" 3)

`DM40_USBTMC=real tools/acdcmeth.py e6 e1 e7 e2 e5`, calibrator on V/COM. The calibrator's "30 V" is
**23.99 V** (DC V READ? 23.986): every 30 V point below is ~24 V.

**e6** (entry from DC V at 3 V; then 3 -> 24 -> 3 V):

| | first reading s | settled s | readings / s (n_rd) | FETCh? changes / s | to 24 V settle s | back to 3 V settle s |
|---|---|---|---|---|---|---|
| A | 1.28 | 1.28 | 3.7 | 3.0 | 3.49 | 2.66 |
| B | 1.48 | 1.48 | 9.3 | 9.2 | 2.55 | 2.24 |
| C | 1.02 | 1.31 | 9.0 | 7.5 | 1.64 | 1.99 |

The 24 V value: A 23.9860, B 23.9790, C 24.0077 V (C +0.09 %, see e5).

**e1** (B's zero, 5 entries per level; zero_pp in digits of the range):

| range | level | zeros (V) | zero p-p digits | B's DC part - READ?, digits |
|---|---|---|---|---|
| 6 V | 0 | +0.0127 x4, -0.0283 | 411 | +1.5 -2.4 +4.1 -0.4 +1.7 |
| 6 V | 3 V | -0.0304, +0.0125, +0.0125, +0.0120, -0.0306 | 431 | +5.4 -0.6 0.0 +3.7 +4.2 |
| 60 V | 0 | -0.0192, +0.0173, +0.0176, +0.0173, +0.0161 | 37 | +0.6 +0.1 -0.3 +0.1 +1.1 |
| 60 V | 24 V | +0.0171, -0.0302, +0.0203, -0.0302, +0.0207 | 51 | -4.2 +2.4 -7.4 +2.1 -7.5 |
| 600 V | 0 | +0.171, +0.169, +0.170, -0.169, +0.169 | 34 | 0.0 +0.1 -0.2 +0.3 +0.1 |
| 600 V | 24 V | -0.194, +0.235, +0.239, -0.197, -0.194 | 44 | +1.8 -6.8 -7.2 +2.0 +1.5 |
| 1000 V | 0 | +1.884, +1.981, +1.966, +1.916, +1.945 | 6 | +1.8 -5.3 -3.6 +0.1 -2.9 |
| 1000 V | 24 V | -1.492, -1.497, -1.447, +2.217, -1.480 | 223 | -2.7 -1.7 -5.8 -16.6 -2.6 |

The zero takes one of two values per entry, about the same size with opposite sign on 600 V (the AD1 chopper
phase at the merge? not investigated). The per-entry zero absorbs most of it, but the residual is up to 7.5
digits at 24 V on 60 V and 16.6 on 1000 V. **B fails the 2-digit rule** on every range.

**e7** (B's 6 V row candidates, zero per entry): (i) R0 1: err -0.22 / +0.44 / -0.82 mV at 1 / 3 / 5 V,
gain 0.99985; (ii) R0 2: err -0.09 / -0.11 / +0.03 mV, gain 1.00003, its zero 0.052 / 0.083 / 0.111 V grows
with the level (~1.5 % raw gain taken up by the per-entry zero). AC part on the square: (i) 4.7758, (ii)
4.7765 V; A's 4.7761.

**e2** (40 Hz square 0..9.55 V; DC V READ? 4.7765 V on 6 V, 4.7763 on 60 V):

| range | method | AC part V | vs A | DC part - DC V | readings / s |
|---|---|---|---|---|---|
| 6 V | A | 4.77609 | - | +0.24 mV | 3.8 |
| 6 V | B | 4.77610 | +0.000 % | -0.13 mV | 9.0 |
| 6 V | C | 4.78217 | **+0.127 %** | +0.88 mV | 9.3 |
| 60 V | A | 4.77129 | - | -0.70 mV | 3.8 |
| 60 V | B | 4.77074 | -0.012 % | -1.04 mV | 8.7 |
| 60 V | C | 4.78204 | **+0.225 %** | **+13.6 mV (+0.28 %)** | 9.0 |

B's AD1 does not move the RMS (6 V: identical). C's AC part rides the self-cal's DC gain and reads
0.13-0.23 % high against the factory AC V calibration: within the 0.5 % class.

**e5** (C re-entered every 60 s, 20 min per level; err = C's DC part - DC V READ?):

| range | level | entries | err | limit 0.05 % + 5 dig | per_code vs first | offset |
|---|---|---|---|---|---|---|
| 60 V | 24 V | 21 | **+20.9..+24.9 mV** (mean +22.7, +0.087..0.104 %) | 17 mV: **FAIL** | -132..+31 ppm | 0.0991..0.1038 V |
| 6 V | 3 V | 21 | -0.34..+1.38 mV (mean +0.45) | 2.0 mV: PASS | -88..+321 ppm | 0.0480..0.0486 V |

No warm-up trend: the 60 V error is constant over the 20 min, a calibration error, not drift. With e2's
+13.6 mV at 4.78 V it fits ~+11 mV offset + ~+0.05 % gain in C's 60 V self-cal.

**By hand, C in AUTO / AC+DC:** on the square, AUTO picks AC (4.772 V, DC 4.777 V). **With >= 5 V DC on the
6 V range the AC part is wrong** (2.11 V on 6 V, 3.83 V on 60 V for a DC input) and the range hunts
6 <-> 60 V every ~3 s, ~0.7 readings / s; in AUTO after the square the page stays on AC ("~4.056 V" for 5 V
DC). 4.0 and 4.5 V are fine (AC part ~1 mV). A in the same sequence settles on DC 4.998 V.

**Decision rule: not met** (e5 60 V fails; the >= 5 V AC-part fault on top). The default stays A. B: drop, by the rule (e1).

Raw by-hand output of the >= 5 V case (`p3b.py`, DBG:ACDC:STAT? every 1.5 s, the session transcript; not in
the jsonl): AUTO entered at 5 V: 6 V AC 2.114 / 2.109 / 2.108 / 2.117 / 2.115 V, 60 V AC 3.834 / 3.834 / 3.834 /
3.834 V, n_rd +1 per range visit (~3 s each, ~20 RMS results per visit: all but one dropped as sqrt(RMS) >
2^18); 5.5 V: 6 V AC 0.635 V, 60 V AC 4.19 V; 4.0 / 4.5 V: AC 1.0..1.6 mV, 9 readings / s. The 6 V value repeats
to 0.3 % over visits: a deterministic transient after the row write, not noise.

### 10.6 Method C after the 20a91d0 check (2026-10-23, branch acdc-c-fix; on the device in 10.8)

Method A stays the default until the retest (10.7) passes.

**1. The 60 V DC part (+11 mV + 0.05 %).** E7's self-cal anchored the DC part on AD1 alone on SENSE - PB5 in
the *AC V row's* switch configuration (R2A FS1, R2E / R2F without DS9 / SMODE2; the DC V row has DS0 FS0 /
DS9 / SMODE2) with the DC V 60 V calibration, and on the time-weighted mean of raw AD2 samples with AD3 and the
RMS / LPF off; the run then reads the LPF with AD3 and the RMS on. At the anchor point the gain does not matter
(the offset absorbs it), so the DC part there is off by (AD1 in that row - READ?) + (LPF in the run row - AD2
mean in the cal row) x per_code. e2 (4.78 V, span 9.8 k codes < 10 k: the table's per_code 4.853726e-4, e5
measured 4.8581e-4 = +0.09 %) and e5 (24 V, measured gain) fit +0.047 % on the anchor and +23 codes (11.2 mV)
run-vs-cal: 13.4 / 22.5 mV against the measured 13.6 / 22.7. The data do not say which part is which (the two
could also be one LPF gain + offset); 6 V is inside its scatter (+0.45 +- 0.5 mV at 3 V). Of the candidates:
the gain fallback mattered in e2 only away from the anchor; the old anchor ran AD1 with AD2 / AD3 *off* (no
note 7 in it), but the run differs from the cal by AD3 + the RMS on and by LPF vs raw AD2.
Fix (`acdcm.rs` `am_cal_step`): the anchor is the stock DC V row's reading (REF_N = 3 results of the DC V
function's own row and calibration: what READ? shows, 6 V on its /10 tap) against the mean of 4 LPF results of
the *run row itself* (AD3, RMS, LPF on), per range and AC+DC session; per_code from the internal zero (the run
row with OP1+ on AGND, 2 LPF results) when the span is >= 10 k codes and within 3 % of the table, kept per
range until power-off (re-measured at a 1.5 x larger span), else the table (dmm.rs DCV_AD2_GAIN). Nothing differs
between anchor and run but the time, so whichever split is true, the DC part equals READ? at the anchor; away
from it the gain error remains (two-point, internal zero). The input moving between the reference and the
signal (spread > 1 % or the signal not where the reference says, 2 % + 2500 codes) takes the reference again.
The DC V autorange no longer steps down inside a method's reference step (it went down to 6 V under a 0..9.55
V square on 60 V and back: a REF / range loop in the emulator). A range visited again in the session uses its
anchor (no 0.45 s blocking self-cal per visit, which also fed the hunting below); first entry ~2.5 s to the
first reading (REF ~1 s, zero ~0.6 s, signal ~0.6 s), a later one ~1.9 s.

**2. >= 5 V DC on 6 V (bogus AC part, hunting).** Not node clipping: at 5 V on 6 V the node is 0.5 V of AD2's
+-1.25 V, on 60 V 0.05 V, and e2's square drove the same 6 V node to 0.955 V with a correct AC part (+0.13 %);
the power meter's YY statistic with 104 k codes of DC was fine (power doc 3.11 T1). What the by-hand data show
(10.5): one reading per range visit; the 6 V AC part 0.42 x the DC, repeating to 0.3 % (the first RMS block
read after the row write, 320..427 ms, for a first-order HPF that starts at 0 when the RMS is enabled:
tau ~0.43 s, INFERRED); the other ~19 RMS results of a visit dropped as garbage (sqrt(RMS) > 2^18); the up-range
then came from an accepted garbage value (|LPF| + 1.414 sqrt(RMS) or the total over range), on 60 V the
transient kept the total over the down threshold until it decayed; every visit re-ran E7's self-cal (an
AGND -> SENSE step). The garbage appears between 4.5 V (95 k codes) and 5 V (105 k); its mechanism is not known.
Fix: the AC part is taken from the RMS only when sqrt(RMS) <= half of AD2's peak-hold p-p over the last two
results + 24 codes (PKHSEL AD2, restarted after each result; no waveform has an RMS above half its p-p,
Popoviciu), i.e. not during the HPF transient and not when garbage. Otherwise, with a steady node (the bound
below 0.5 % of the range), the AC part is the RMS's last value of the last 2 s if the bound allows it, else the
bound (an upper limit, ~3 x the noise: 6 mV on 6 V); with AC on the node and no usable RMS for 2.5 s: up a
range, and that range is not entered again downwards while |DC| >= 0.9 x the level it failed at (forgotten when
the range above measures a total over the lower one: a large AC part, not an RMS fault). Up-range
also when AD2's peak hold reaches 90 % of its span (the node's real clip; replaces |LPF| + 1.414 sqrt(RMS)),
when |DC| alone is over the range, or on the total with a usable AC part; down only with a usable AC part.
The first AC part of a session is taken as it is (the stock EMA's 0.99 jump from 0 showed it 1 % low).

**3. The AC part's gain (+0.13 / +0.23 % against A).** C's sqrt(RMS) now goes through the factory AC V gain
and offset of the range (cal 0x34 / 0x44, A's formula). e2 recomputed: sqrt(RMS) = 4.78217 / 4.809514e-5 =
99 432 codes -> 4.77675 V on 6 V (A 4.77609: +0.014 %), 9 852.3 codes -> 4.77117 V on 60 V (A 4.77129: -0.003 %).
The self-cal's per_code is the DC gain of LPF(AD2) against AD1; sqrt(X Y) carries AD3's gain too and the tap's
AC ratio; the factory AC V calibration was taken on the same tap, OP1 gain and AD2 x AD3 with only PB4's coupling
capacitor between (unity at 40 Hz), so it is the right one for the RMS: the residual against A is 10 x smaller.

**Emulator model (HY3131.cs, INFERRED where marked).** The DC-coupled row's per-range DC per_code factor (6 V
1.0096, 60 V 1.0188, 600 V 1.0053, 1000 V 1.0027: e5 / test 6.5) with its RMS through the factory AC V cal; the
60 V split of item 1 ("ad1acg:", "ad3sh:"); the RMS block's HPF (tau 0.43 s, state 0 at ENRMS / ENAD2, "hpftau:");
garbage RMS blocks with probability 0.85 above 102.5 k codes of DC on the X x Y node ("rmsbig:", "rmsbigp:";
phenomenological); AD2's peak hold over the whole gap between two accesses (it saw the last 256 samples
only; the spread samples draw their noise from their own generator, so the main noise sequence
is the one of before: powtest `ereset` failed with it shared, also on 540e056); the LPF's window exactly for a sine (sinc^2) and a square (piece by piece): the sampled window aliased a
40 Hz square into a -6 % DC bias of the LPF. Host tests 491.

Reproduction (emulator, the final model; acdcmethtest with method C held to the device's rules, couptest
`sq5` = AUTO on the square, then 5 / 5.5 / 4.5 V DC):

| phase | 540e056 (as on the device) | the fix |
|---|---|---|
| V DC 5 (6 V) | hunting 6 <-> 60 V, 0 readings / s, AC part 6.1 V (max 12.0) | 6 V only, 9.3 / s, DC part -0.0000 V, AC part 4.8 mV (max 7.3, the peak bound) |
| V AC 2 @50 | AC part 2.0023 V (+0.115 %) | 2.0000 V |
| V DC 12 + 0.5 @50 (60 V) | total +0.138 % (DC part ~+16 mV) | -0.005 % |
| V DC 24 (60 V) | DC part -11.6 mV (anchored at 12 V, its gain) | -1.2 mV |
| V square 0..9.55 @40 (60 V) | DC part +33 mV, AC part +0.08 % | DC part -0.8 mV, AC part 4.7760 V |
| V DC 5 after the square (60 -> 6 V) | hunting, AC part 5.4 V | 6 V, 8.9 / s, DC part -0.0000 V, AC 0.7 mV |
| couptest sq5 (AUTO) | 5.5 V: DC +0.24 %, the rest ok | all ok (5 V: AUTO,DC 5.0012) |

The emulator's LPF window (sinc^2 over 213 ms, INFERRED) leaves +-0.4 % of
a 40 Hz square on the DC part (the C square phase's 0.39 % p-p); the device's
C DC part on the square had an sd of 0.10 / 0.29 mV (6 / 60 V, e2): its LPF
rejects 40 Hz far better than the model.

### 10.7 Device retest of method C (`tools/acdcmeth.py`; run on ac1e852: 10.8)

Calibrator on V / COM (DC <= 24 V and the 40 Hz square), `DM40_USBTMC=real`,
`tools/acdcmeth.py e8 e9 e10 e2 e6`, then `e5 --minutes 3`. New experiments:
`e8` (C entered at the top level of 60 V and 6 V, stepped down to 0 V
without leaving: the gain away from the anchor), `e8kept` (re-entry with
the gain kept), `e9` (4.0 .. 5.5 V on 6 V in AC+DC: one range, >= 6
readings / s, AC part < 30 mV), `e10` (AUTO on the square, then 5.0 / 5.5 /
4.5 V). Device assumption the fix rests on: rewriting R29 to restart AD2's
peak hold (ENPKH off / on) does not restart the RMS / LPF block (the stock
AC rows' sentinel does the same in `poll_ac`). Decision rule unchanged
(10.4), plus e8 within 0.05 % + 5 digits and e9 / e10 passing.

### 10.8 Device results of the retest (ac1e852, 2026-10-23 04:57-05:22, docs/device-check-2026-10-21.md "ac1e852")

`DM40_USBTMC=real tools/acdcmeth.py e8 e9 e10 e2 e6`, then `e5 --minutes 3`; calibrator on V / COM. Two extra
scripts: `z60.py` (C vs READ? at the same moment, 0 / 1 V on 60 V) and `par.py` (AUTO on the square, A vs C).
Errors are against the meter's own DC V READ? (the calibrator wanders ~0.8 mV, device-check "DC V at the
calibrator's 1 V").

| test | pass | result |
|---|---|---|
| e8 6 V (anchor 5 V) | 0.05 % + 5 dig | 5 / 3 / 1 / 0 V: +0.07 / +0.10 / +0.44 / +0.33 mV: PASS |
| e8 60 V (anchor 24 V) | 0.05 % + 5 dig | 24 / 12 / 5 V: -0.47 / -4.34 / -6.12 mV PASS; **1 V -7.23 (limit 5.5), 0 V -8.01 mV (5.0): FAIL** |
| z60 (READ? right after C) | same | 0 V -6.89 / -7.06, 1 V -6.39 / -6.58 mV: **FAIL** (not the calibrator) |
| e8kept (5 V on 60 V, gain kept) | same | -1.24 mV: PASS |
| e9 4.0 / 4.5 / 5.0 / 5.5 V | 1 range, >= 6 rd/s, AC < 30 mV | 6 V only, 9.18-9.27 rd/s, AC max 0.27-0.31 mV, DC -0.15..+0.42 mV, acsrc 0 in all 479 samples: PASS |
| e10 square -> 5.0 / 5.5 / 4.5 V | AUTO,DC in rule, 1 range | AUTO,DC, -0.21 / -0.28 / -0.28 mV, 6 V only: PASS |
| e2 AC part C vs A | 0.05 % | 6 V +0.012 %, 60 V -0.004 % (was +0.127 / +0.225 %): PASS; DC part +0.32 / -0.33 mV |
| e6 | ~2.5 s, ~9 rd/s | entry 1.71 s, 9.3 rd/s; 3 -> 24 V 0.59 s, back 1.01 s (A 1.27 s, 3.7 rd/s, 3.41 / 2.47 s) |
| e5 24 V / 60 V, 3 entries | 17 mV | +0.53 / -1.29 / +0.53 mV (was +20.9..+24.9): PASS |
| e5 3 V / 6 V, 3 entries | 2.0 mV | -0.04 / +0.37 / -0.09 mV: PASS |
| by hand: AUTO, square -> 5 V | the 20a91d0 case | AUTO DC 4.9981 V, 6 V range, AC 0.2 mV, 9.4 rd/s: PASS |

**The R29 assumption holds.** Restarting AD2's peak hold (ENPKH off / on) leaves the RMS / LPF running: e9 / e10
kept ~9.2 readings / s and the RMS usable in every sample (`acsrc` 0). At 5 / 5.5 V on 6 V no garbage RMS block
was seen in steady state; the peak bound stood in once, in the transient after the square -> 5 V step (AUTO,
acsrc 1, 12.7 mV at 2 s, RMS usable again by 5 s): the only device evidence of that path, and it behaved as designed.

**What fails: C's 60 V gain.** The five e8 60 V points fit err = -7.77 mV + 0.302 mV/V x level (residuals
<= 0.24 mV). per_code from the internal zero (4.85309e-4, span 49 422 codes) is 0.030 % high (the data say
~4.8516e-4; the table's 4.853726e-4 is 0.043 % high, so the table fallback would be worse). Equivalently, the
internal zero (the run row with OP1+ on AGND) is ~14-16 codes (~7-8 mV) away from a real 0 V input on 60 V
(by the fit's sign, the AGND zero's code above SENSE at 0 V; inferred, to check before a zero correction).
The anchor removes the error at the anchor level, so it grows toward 0 V: over the rule below ~2 V on the
manual 60 V range. AUTO range puts such levels on 6 V, which has no such error (slope ~-0.007 %), so the
failing case is a manual 60 V range or a large AC part on a small DC part. Candidate fixes (not made): a
per-range correction of the internal zero (needs its repeatability over entries and power cycles), the gain
from two READ? anchors when a session sees two levels, or a device-measured table per_code for 60 V.
**Fixed in 10.9** (the gain from two references).

**AUTO on the square changed to DC.** On the 40 Hz square (AC 4.760, DC 4.764 V) C's AUTO stays on DC and A's
goes to AC after ~3.5 s, deterministic over two entries each. dmm.rs's first evaluation shows AC only when
AC > |DC|, and a later switch to AC needs AC > 1.5 x |DC|, so the first evaluation decides. A's first AC part
overshoots (4.91 V); C's first AC part is taken as it is. On 20a91d0 C's AC part was 0.13 % high and it picked
AC. This is a tie of the rule, not an error of C. **Rule changed in 10.9** (a tie is DC, the decision waits for a
settled AC part).

**Decision rule: not met** (e8 60 V at 0 / 1 V). Everything else passes, and e5's 60 V failure of 20a91d0 is
gone. The default stays A.

### 10.9 After the ac1e852 retest: C's 60 V gain and the AUTO tie (2026-10-23, branch acdc-c-fix; device: 10.10)

Method A stays the default.

**1. C's 60 V gain: from a second stock DC V reference.** The three candidates of 10.8:

| candidate | what it assumes | verdict |
|---|---|---|
| a per-range constant for the internal zero (~15 codes on 60 V, "derived from the factory DC V calibration") | that the AGND zero's offset against SENSE at 0 V is a fixed property of the unit and range. One device, one session, its sign inferred from a fit; nothing in the factory record measures the AD2 path's AGND-vs-SENSE difference (the factory DC V offsets are AD1's, on SENSE - PB5). Its repeatability over entries / power cycles is not measured (TODO). | not taken: unverifiable here, and wrong by its own error on any other unit |
| a device-measured table per_code for 60 V | the same, for the gain (the table is already 0.043 % high on this unit) | not taken |
| **the gain between two READ? references the session sees** | nothing of the hardware: the rule judges C against the meter's own DC V reading, so a gain measured between two such readings is measured against the reference that judges it | **taken** |

How (`acdcm.rs` `am_refine`, in ST_RUN, every LPF result): a steady window of STEADY_N = 12 LPF results (~1.3 s)
within the anchor's spread rule (max(60 codes, 1 %)); when its mean is >= CAL_GAIN_CODES (10 k codes: 4.9 V on 60 V,
0.49 V on 6 V) from the range's anchor and more than 1.5 x the span of the last two-point attempt there, and the AC
part has a source (RMS or the peak bound: acsrc != 2), C takes the stock DC V row's reading again (REF_N = 3 DC V
results: the DC part shown meanwhile is that reading, the AC part held; ~1 s), then 4 LPF results of the run row
(ST_MEAS), and per_code = (v2 - v1) / (m2 - m1) between the old anchor and the new point, if within 3 % of the table.
The anchor moves to the new point (offset = m2 x per_code - v2). That gain is kept for the range until power-off (as
the internal zero's) and replaces it; a later internal zero does not overwrite it, a later two-point gain only over a
1.5 x larger span (the attempts' spans grow geometrically: at most ~8 per range per power-on, and a failed attempt
counts). The internal zero stays the first estimate of a session entered at a level >= 10 k codes from 0. Cost: one
~1.5 s of DC V readings (no AC update) after the first level change of >= 10 k codes on a range; e9 (entered per
level) does not trigger it, e10's 5 -> 5.5 V (10.3 k codes on 6 V) may.

Emulator model (HY3131.cs, both copies): `AgndZeroCodes` = {0, 15, 0, 0} ("agndz:" scales): with OP1+ on AGND the
DC-coupled row reads 15 codes above SENSE at 0 V on 60 V, INFERRED from 10.8 (6 V: no such error on the device,
600 / 1000 V not measured). `acdcmethtest.py --e8 down|up` is the device's e8 (manual 60 V levels, then manual 6 V;
the DC part against the source in the device script's window, 1..4 s after the step, and a late one, 4..8.5 s).

| e8 (emulator), DC part - source, mV | 24 | 12 | 5 | 1 | 0 V (60 V) | rule |
|---|---|---|---|---|---|---|
| ad0587c, down (as the device: -0.5 / -4.3 / -6.1 / **-7.2 / -8.0**) | -0.44 | -3.62 | -6.00 | **-6.74** | **-7.29** | 17 / 11 / 7.5 / 5.5 / 5.0 |
| the fix, down (late window; device window the same except 12 V: -0.81) | -0.39 | +0.14 | -0.08 | +0.42 | +0.18 | |
| ad0587c, up (entered at 0 V: the table's gain) | **-20.5** | -9.73 | -3.94 | -0.02 | +0.80 | |
| the fix, up | +0.41 | +0.06 | -0.64 | -1.33 | -0.71 | |

6 V in the same runs: within 0.25 mV (rule 0.5..3 mV) before and after in `down`; in `up` ad0587c read -6.0 / -3.6 /
-1.3 mV at 5 / 3 / 1 V (anchored at 0 V in AUTO with the table's gain, which the model puts 0.12 % off), the fix
-0.02 / -0.04 / -0.05 mV after the first step. "up" is the case the device has not run: a C session entered at 0 V on
60 V with the table gain (0.043 % high on the device) would read ~+10 mV at 24 V (rule 17 mV).

**2. AUTO on a tie (the 40 Hz square).** The first evaluation decided at once, and the 1.5 x / 0.75 x hysteresis
kept it, so the first AC part's transient decided a tie: A's first AC part overshoots (+3 % on the device's square,
+7 % on 33b44bd, up to +52 % in the emulator after the 6 -> 60 V up-range), C's is ~0.5 % high in the emulator.
Reproduced: couptest `sqtie` (three fresh AUTO entries on the square, the part shown from the first AUTO reading on)
on ad0587c: A and C both DC -> AC within the first 1..2 s, then AC (the emulator's square is an exact tie, so the side
is noise; on the device the 0.08 % margin went to C's DC and A's overshoot). New rule (`dmm.rs` auto_eval /
auto_shows_ac, AUTO_SETTLE_K / AUTO_TIE / AUTO_TIE_MS):
* before the first decision the reading shows DC; AC only when there is no DC part (|DC| below the off floor);
* the first decision waits until the AC part has settled on one side: K = 4 x its change since the evaluation before
  (an EMA's remaining drift) is less than its distance to |DC|; then the larger part (as before). A clear case
  decides at the second evaluation (A ~2 s, C ~0.1 s after the first); an AC part below the floor: DC at once;
* a tie (|AC - |DC|| <= 1 % of |DC|) or no settling within 4 s: DC. 1 % covers the parts' noise (A ~0.03 % phase to
  phase; C's LPF ripple of the square +-0.3 % in the emulator, ~0.006 % on the device).
Either part is fair at a tie; DC is the stock's default coupling and needs no further rule. After the fix couptest
`sqtie`: DC in all three entries with A and with C, no change of the part shown. `band` unchanged (the larger part at
1.2 / 0.8 / 1.05 / 0.95: outside the 1 % tie). Cost: a mixed signal entering AUTO shows DC ~2 s (A) / ~0.2 s (C)
before it shows AC.

**3. dcvtest** (`LOG:SCALe?` 1 % off since fb96992): the model is right, the test was not: fb96992 gave the
DC-coupled row the device's per-range per_code factor (`Ad2DcGainR`, 6 V 1.0096, 60 V 1.0188, from test 6.5 / e5)
and dcvtest still expected nominal / ad2dcg. Now it expects nominal x that factor / ad2dcg (6 V and the 25 V session
on 60 V): PASS.

Emulator: acdcmethtest A / B / C / B~ and `--e8 down` / `up` pass (down: 60 V
within -1.3..+1.4 mV over four runs at every level, 6 V within 0.1 mV; up:
within 1.9 mV after the first step); couptest `sqtie` as above.

### 10.10 Device retest of 10.9 (`tools/acdcmeth.py`, calibrator on V / COM, `DM40_USBTMC=real`; run on 80d5b01: 10.11)

The two-point gain is kept per range until power-off, so `e8up` runs first
after a power-on: `e8up`, then `e8s e8 e11 e10 e9`, then `e5 --minutes 3`.
New experiments: `e8up` (60 V manual, entered at 0 V on the table's gain,
then 1 / 5 / 12 / 24 V, 4 s per level + a 3 s mean: the two-point gain from
the 5 V step on), `e8s` (ac1e852's e8 with 4 s per level), `e11` (AUTO on the
40 Hz square, A and C, 3 entries each: DC from the first AUTO reading, at
most one change). Pass: every level within 0.05 % + 5 digits of READ? (5.0 /
5.5 / 7.5 / 11 / 17 mV on 60 V at 0 / 1 / 5 / 12 / 24 V). Decision rule
unchanged (10.4 / 10.7).

### 10.11 Device results of the 10.10 retest (80d5b01, 2026-10-23 08:59-09:40, docs/device-check-2026-10-21.md "80d5b01")

Run as 10.10, with e8up first after the install's boot and a `dm40dbg.py reset` before e8s. Errors are C's DC part
minus the meter's DC V READ? on the same manual range, taken before the C session and again right after it (they
agree within 0.05-0.33 mV).

| test | pass | result |
|---|---|---|
| e8up 60 V, 0 / 1 / 5 / 12 / 24 V | 5.0 / 5.5 / 7.5 / 11 / 17 mV | -2.2 / -1.3 / -0.1 / -2.8 / +1.2 mV, gspan 0 -> 10.3 k -> 39.1 k: PASS |
| e8s 60 V, 24 / 12 / 5 / 1 / 0 V | 17 / 11 / 7.5 / 5.5 / 5.0 | +2.5 / +1.5 / +1.6 / **+1.8 / +1.9** (ac1e852 -7.2 / -8.0 at 1 / 0 V): PASS |
| e8s 6 V, 5 / 3 / 1 / 0 V | 3.0 / 2.0 / 1.0 / 0.5 | -0.14 / -0.21 / -0.55 / **-0.86 mV: FAIL at 0 V** (ac1e852 +0.07..+0.44) |
| tc6 (the e8s 6 V step, 3 power cycles) | 0 V 0.5 | 0 V -0.92 / -0.25 / -0.63: 2 of 3 FAIL; two-point pc +143..+195 ppm over the internal zero's |
| e8 (1 s per level, gains kept) | as e8s | 60 V -0.8..+3.6, 6 V -0.02..+0.31 mV: PASS |
| e11 AUTO on the square, A / C x 3 | DC, <= 1 change | DC in all 6, 0 changes; by hand (taps, screenshots) AUTO DC with A and C: PASS |
| e10 / e9 | as 10.8 | AUTO,DC 0.0 / -0.3 / -0.6 mV; e9 -0.26..+0.07 mV, 6 V only, 9.2 rd/s, acsrc 0: PASS |
| e5 24 V / 60 V; 3 V / 6 V | 17 / 2.0 mV | -1.9..+1.5; -0.24..+0.02 mV: PASS |

**60 V: fixed.** The two-point gain (4.8515-4.8523e-4 over 24.7-39.1 k codes) replaces the internal zero's
+0.025 % (4.85281e-4; ac1e852 +0.030 %). The table start of e8up also lands within the rule.

**6 V: a regression of 10.9.** On 6 V the internal zero's gain was right (104 k codes). The first two-point attempt
of >= 10 k codes replaces it, and over the 5 -> 3 V step (41.6 k codes) that gain came out 143-195 ppm high in all four
power-on sessions. The 83 k span of e8 (5 -> 1 V) gave the internal zero's value to 2 ppm. The cause is in the
STAT? trace: ST_MEAS ends ~3.6 s after the step, when the AD2 LPF code is still 5-12 codes above the value it reaches
~8 s later. The DC V row on the same step settles within 0.1 mV in ~1 s, so the calibrator is not the cause. On a
step down, a high m2 gives a per_code that is too large, and an anchor that reads low afterwards (-0.1..-0.3 mV at 3 V);
both signs match. The 60 V anchor errors of 1-4 mV (inside the rule) are probably the same tail. Fix candidates
(not made here; (b) made in 10.12, with a span rule close to (a)): (b) is the one that removes the cause:
(a) keep the larger-span gain unless the new span is >= 1.5 x it (the internal zero's included). This brings back
    60 V's AGND bias for 24 -> 12 V, unless it is made per range, which 10.9 rejected;
(b) take m2 only after the LPF stops drifting (a slope test over the steady window / ST_MEAS, < ~1 code/s; up to
    ~8 s more on the first level change of a range);
(c) a larger CAL_GAIN_CODES on 6 V.

**Decision rule: not met** (e8s 6 V at 0 V; tc6 2 of 3). Everything else passes, including the ac1e852 failure
case. The default stays A.

Also seen: with C, the first V tap after `CONF:VOLT:DC` on the square was lost in 5 of 6 tries, and with A in 1 of 3.
It did not happen at 1 V DC. It is not specific to C (device-check doc, "80d5b01" 3).


### 10.12 After the 80d5b01 retest: the second reference waits for the LPF (2026-10-23, branch acdc-c-lpfwait; device: 10.13)

Method A stays the default. Commits 0249356 (emulator), 48f6e51 + 4e71686 (opendm40).

**1. Emulator: the LPF's creep (HY3131.cs, both copies; INFERRED, phenomenological).** The 80d5b01 `tc6` trace
(3 power cycles, 5 -> 3 V on 6 V, 10 Hz STAT?) relative to the value ~10 s after the step: +5 codes at 0.7-1.6 s,
+8..+10 right after the second reference's row write (3.6-4.3 s), +4 at 5-6 s, +3 at 8 s, 0 at 10-12 s; noise
~1.7 codes rms. Not a first-order tail (that would fall fastest at the start). Model: the LPF of the DC-coupled
row reads LpfCreep x (s - x) codes extra, x the node's code of the jack's DC and s the same through two cascaded
2.5 s lags (a step leaves (1 + t / tau) e^(-t / tau) of LpfCreep x step): 2.4e-4 gives +10 / +9.4 / +5.8 / +3.1 / +1.7
codes at 0 / 1 / 3.6 / 6 / 8 s after the 41.6 k step. Keyed on input steps while that row runs on one range (it
survives the ~1 s DC V reference, restarts after a > 3 s gap or a range change): the device's sessions entered
seconds after the calibrator's step had an accurate internal zero, and the e8 two-point gain over 5 -> 1 V
(83 k) agreed with it to 2 ppm, which a creep proportional to every step would not give (the device data are
not fully consistent with any one simple model; this one reproduces the failing case). Plus the LPF's own noise
(1.7 codes rms; the model's LPF had none). Options "lpfcreep:" (scale, 0 = off), "creeptau:", "creepord:" 1 / 2,
"lpfn:"; DM40Frontend now also accepts "agndz:" (it was missing from the option list). Host tests 501.

**2. Reproduction.** `acdcmethtest.py --e8 tc6` (6 V alone: entered at 5 V, then 3 / 1 / 0.1 V, 10 s each; the
last level 0.1 V because the device's 0 V step was 62.4 k codes from the 3 V anchor, just inside the 1.5 x rule,
and 0 V in the model lands just outside it and takes a new reference). DC part - source, mV, late window
(4..8.5 s), rule 0.05 % + 5 digits:

| | 5 V | 3 V | 1 V | 0.1 V (rule 0.55) | the 6 V gain |
|---|---|---|---|---|---|
| af7d2e4 (as 80d5b01) | -0.12 | -0.27 | -0.56 | **-0.75** | two-point over 5 -> 3 V (41.5 k), **+177 ppm** over the internal zero's (device +143..+195) |
| 4e71686 | -0.12 | +0.05 | +0.07 | +0.07 | the internal zero's (104 k) kept: the 3 V two-point differs by +4.0 codes at 3 V, inside the noise rule |

**3. The fix (`acdcm.rs`).**
* `am_refine`: a sliding window of the last 28 LPF results (~3 s) within the spread rule. The second reference
  starts only when the window is >= 48 results (~5 s) old, its least-squares slope is below 1 code / s, AD2's
  peak-hold p-p is within 2 % of |LPF| + 1000 codes (no real AC on the node: the row rewrite's RMS HPF transient
  passes the p-p / 2 bound when the node carries a large AC part, an AC spike of +0.7 % on the 40 Hz square in
  acdcmethtest, and the LPF ripple enters the window), and the usual conditions hold (>= 10 k codes, 1.5 x the
  last attempt, an AC source). Not settled after 120 results (~13 s) at one level: no reference until the input
  moves; a timeout does not use up the span (`ctry` unchanged). The 5 s minimum: the device's tail stayed flat for
  ~4 s and then fell, so a slope alone passes too early.
* The new point is the fitted line's end, taken **before** the reference row is written; the run row's results
  after the reference only have to agree within 12 codes (else the input moved: no gain, the anchor from those
  results as before). The anchor moves to the new point.
* The new point pairs with the session's reference point on the range (lowest / highest code: the entry anchor
  and every refinement) that is farther from it, not only the last anchor: on 60 V 24 -> 12 -> 5 V gives a 39 k
  span at 5 V instead of 12 -> 5 V's 14 k (below 1.5 x 24.7 k: no attempt).
* Span rule (`am_cal_step`): a two-point gain replaces the gain in use when that is the table's or its span is >=
  the one in use. Over a **shorter** span only when the gain in use is an internal zero's that this measurement
  shows biased: the two lines differ at the new point by more than 3 x the window's residual rms (the scale of the
  noise of each of the four inputs: two LPF means, two READ?s; ~1.7 codes) + |slope| x 2.5 s (what a tail still
  creeping at the measured slope may leave; the trace's lag), at least 2 codes. A two-point gain over a shorter
  span never replaces a two-point one. "Biased" is judged per attempt from the data, not from a per-range list:
  60 V's AGND zero (15 codes in the model, ~15 on the device) shows as ~8 codes at the 5 V point (39 k), 6 V's
  zero as noise (2..4.5 codes at 3 / 1 V).
* A range revisited in the session whose gain is still the table's is anchored again (with the internal zero when
  the span allows) instead of running on its old anchor: before, the wait made e8 `up` (6 V first visited at 0 V
  in AUTO, back at 5 V) show the table gain's 0.12 % for ~6 s.
* DBG:ACDC:STAT? appends gain_src (0 table, 1 internal zero, 2 two references, 3 two references over a biased
  zero), gain_dis (the last two-point gain against the one in use at its new point, codes) and the window's slope
  (codes / s); tools/acdcmeth.py logs them (gsrc, gdis, slope) and has `e8l` / `e8upl` (10 s per level) and
  `tc6`.

Cost: a first level change on a range costs ~5-8 s before the second
reference (it was ~1.5 s); the reference itself is unchanged (~1 s of DC V
readings, the AC part held).

**4. Emulator runs of 4e71686** (the image of the commit; `--e8` DC part - source, mV, windows 1..4 / 4..8.5 s):

| e8 down | 24 | 12 | 5 | 1 | 0 V (60 V) | 5 | 3 | 1 | 0 V (6 V) |
|---|---|---|---|---|---|---|---|---|---|
| 4e71686 | - / -1.16 | -1.21 / -1.15 | -0.99 / -0.53 | +0.49 / +0.17 | -0.00 / -0.13 | +0.05 / +0.04 | +0.41 / +0.10 | +0.23 / -0.01 | +0.21 / +0.08 |
| gain | iz 49.4 k | iz (dis -3.9) | **two-point 39.1 k over the biased zero** (dis -8.3) | | | iz 104 k | iz (dis +4.5) | iz (dis +2.2) | iz |

`up`: within the rule everywhere (60 V 5 V -5.7 mV in the first window, the table gain until the first second
reference ~6 s after the 1 -> 5 V step; rule 7.5; 6 V, first visited at 0 V in AUTO with the table's gain,
re-anchored at 5 V with its internal zero, then a two-point gain against the 0 V point (104 k, the same span):
within 0.4 mV).
The 6 V 3 V level's first window shows the creep itself (+0.41 mV = 8.5 codes, rule 2.0); the device's DC part
creeps the same way, only the gain no longer takes it in.

On 48f6e51 (before the AC gate) acdcmethtest failed C's square: the AC part
4.811 V (max) after the reference's row rewrite; 4e71686 passes acdcmethtest
A / B / C / B~, `--e8 down` / `up` / `tc6` and couptest with all three
methods.

### 10.13 Device retest of 10.12 (`tools/acdcmeth.py`, calibrator on V / COM, `DM40_USBTMC=real`)

Order (gains are kept per range until power-off; `tools/dm40dbg.py reset`
stands in for a power cycle): `e8up` first after the install's boot, then 4 x
(reset, `e8s`), `e8`, `e5 --minutes 3`; optionally `tc6` (the 6 V time course
after a reset) and `e8l` (e8s with 10 s per level), and `e11 e10 e9` as a
regression. Pass: 6 V 0 V within 0.5 mV in all four e8s (80d5b01: -0.86 mV);
6 V gsrc 1 (the internal zero's gain kept) at every level; 60 V 1 / 0 V within
5.5 / 5.0 mV, gsrc 3 from the 5 V level on (the AGND zero judged biased, gdis
~-8 codes). Decision rule (10.4 / 10.7) unchanged.

Run on a8048ba (device check "ad97fd7 / c8c2f02 / a8048ba" 5): e8s 6 V at 0 V
-0.25 / +0.18 / -0.59 / +0.21 mV, 3 of 4 within 0.5 mV; the miss took a
second reference at 3 V judged biased (gsrc 3, gdis +8.95), then a two-point
gain at 1 V. e8up, e8 and e5 pass. Analysis: 10.14.

### 10.14 After the a8048ba retest: the bias test and the LPF creep's lag (2026-10-23, branch acdc-bias; device: 10.15)

Method A stays the default. Commits 8e5ca2d, 9f36bbe (opendm40), 38a63eb (emulator).

**1. The data (device a8048ba; `tools/acdcmeth.py --trace`: STAT? at ~10 Hz per level).** The 10.13 miss was
a 6 V internal zero judged biased (gsrc 3, gdis +8.95 codes at 3 V). gdis is the two-point gain against the
internal zero's, in codes at the new point. Collected: e8up and 6 x (`dm40dbg.py reset`, e8s) on 2026-10-23
12:35-12:53, plus the four e8s of 10.13 (the log of device-check "ad97fd7 / c8c2f02 / a8048ba" 5):

| gdis against the internal zero's gain | n | values (codes) | sign |
|---|---|---|---|
| 6 V, 3 V level (5 -> 3 V, 41.6 k step, span 41.6 k) | 10 | +4.0 +6.5 **+9.0** +5.6 / +7.6 **+13.0** +7.6 +3.9 +5.1 +8.0 | 10 of 10 positive |
| 6 V, 1 V level (3 -> 1 V, span 83 k to the 5 V entry) | 8 | **+9.7** -0.9 -0.1 / +2.8 +0.7 +3.3 +3.6 +2.0 | 6 of 8; the 2 negative within 1 code |
| 60 V, 12 V level (24 -> 12 V, span 24.7 k) and 5 V | 12 | -8.6 -5.2 -10.0 -9.3 / -2.5 -3.4 -10.1 -6.5 -9.0 -9.3; 5 V -2.5 -6.9 | 12 of 12 negative |

Bold: judged biased (gsrc 3) by the 10.12 rule. The rule is 3 x the window's residual rms + |slope| x 2.5 s,
at least 2 codes. Residual rms from the trace: 6 V 1.4-2.9 codes (median ~1.9), 60 V 0.7-1.6 (median ~1.0).
6 V 0 V in today's six e8s: +0.13 / +0.04 / +0.02 / +0.08 / +0.21 / +0.06 mV. All six pass, including run 2,
which took the false gsrc 3 (+13.0) and then a 5 -> 1 V two-point gain. 10.13's -0.59 mV is the same path
landing worse.

**It is not noise.** On 6 V, gdis is one-signed: every value beyond +-1 code is positive. The trace explains
why. Before the reference, the 3 V level's LPF reads 3..12 codes above the line the 3 V DC V reading puts it on,
and it is flat (window slope < 1 code / s). After the reference it is lower (device-check doc "acdc-bias" 1).
This is the creep's lag toward the level before: the new point sits too close to the old level, so the two-point
gain comes out too large. That happens on a step down or up alike, whenever the farther reference point lies
beyond the level before, which is the case in every e8 variant. On 60 V the biased AGND zero (10.8) has the
opposite sign on this unit, so its gdis is negative (the lag only makes it smaller).

**The three options given, on this data.** All three fail:
* "the same sign across 2 references": on 6 V the 3 V and 1 V references are both positive in every session
  where both are beyond 1 code. It would have confirmed sessions 1 and 3 of 10.13 and run 2 here.
* "require the biased judgement to repeat": the same problem. A systematic lag repeats.
* "scale the threshold by the measured LPF noise": it already is (3 x the residual rms). A one-signed offset of
  +4..+13 codes against an rms of ~1.9 is not a noise problem. A k large enough to pass +13 (k ~6) would also
  miss most of the 60 V cases (|gdis| 2.5-10 against rms ~1).

**2. The rule taken: a creep allowance on the lag side only (`acdcm.rs` am_cal_step, CREEP_K).**
* The LPF level before the input step is `prevl`. It is the newest result of a window that was steady for
  >= PREV_N = 10 results when the jump restarted it, or the new point of a second reference (9f36bbe). It is
  invalid after a range change or an AC+DC entry.
* At the reference, `pstep` = new point - prevl.
* The lag side is where gdis x pstep x (new point - p) > 0, p being the paired reference point. There the
  threshold is the 10.12 noise term + CREEP_K x |pstep|, with CREEP_K = 4e-4 codes per code of step: 16.6
  codes at 6 V's 41.6 k step, and 9.9 at 60 V's 24.7 k.
* On the other side, the noise term alone (unchanged).
* CREEP_K: the largest lag-side gdis seen is +13.0 at 41.6 k (3.1e-4). 4e-4 carries it without the noise term.
  The threshold on 6 V came out at 22.9-25.5 codes on the device (10.15), against lag-side gdis up to +12.8.
* Why this and not "negative only": the lag's sign follows from the geometry of the step and the points, not
  from this unit. A unit whose AGND zero is biased the other way still has its bias caught on the
  non-lag side.
* **The cost:** a zero biased in the lag's direction on a given step is caught only when it exceeds
  noise + CREEP_K x step: ~25 codes on 6 V for 41.6 k, ~13 on 60 V for 24.7 k. Below that, C keeps the internal
  zero's gain, as before 10.9. A two-point gain over a span >= the one in use replaces it unconditionally, as
  before; the lag inflates it by ~gdis / span (e.g. 8 / 104 k on 6 V `up`: ~0.4 mV at 5 V, inside the rule).
* Not explained: at the same 41.6 k step the 1 V level's gdis (~+2.5 median) is smaller than the 3 V level's
  (~+7). A pure "lag proportional to the step" model predicts equal values. The allowance is an upper bound, so
  the rule does not depend on this, but the mechanism is not pinned down.
* STAT? appends four fields: psig (the window's residual rms), bthr (the threshold gdis was held to), pslope,
  pstep. They describe the last attempt. `acdcmeth.py --trace` logs STAT? per level.

**3. Emulator (HY3131.cs, both copies; 38a63eb).** The creep is now three cascaded 2.5 s lags (`creepord:3`,
the default). Two lags left only ~3 codes at the ~5.6 s reference with the slope below 1 code / s; three are
flat for the first seconds and leave ~6 codes (+10 / +9.2 / +6.1 / +3.8 / +1.4 codes at 0 / 2.5 / 5.6 / 8 / 12 s
after the 41.6 k step). The LPF noise is per range: 6 V 1.7, 60 V 1.0 codes rms (`LpfDcNoiseR`; "lpfn:" scales
the 6 V value). The host test of the creep expects the order-3 value (7.8 codes; got 8). Reproduction on the
new model, 219817c (= a8048ba's firmware):
* `--e8 down`: 6 V 3 V gsrc 3 (gdis +6.1), then a 5 -> 1 V gain (gsrc 2, -7.8).
* `--e8 tc6`: gsrc 3 at +8.2 (`creepord:3`) and +9.9 (`creeptau:3`) in the trial runs; +8.0 not taken in a later run.

The 0 V error stays inside 0.5 mV in the model (-0.13 mV). The device's -0.59 mV is not reproduced in
magnitude, only the false judgement.

| emulator, C DC part - source (mV, late window) | 60 V 12 / 5 / 1 / 0 V | 6 V 3 / 1 / 0 V | 6 V gain |
|---|---|---|---|
| 219817c `down` | -0.71 / -1.22 / -1.73 / -2.44 | +0.04 / -0.17 / -0.13 | **gsrc 3 at 3 V** (+6.1), gsrc 2 at 1 V |
| 9f36bbe `down` | -0.86 / -0.45 / +0.09 / -0.69 | +0.13 / -0.08 / -0.21 | iz kept (gdis +7.2, +6.8) |
| 9f36bbe `tc6` | | +0.01 / +0.04 / -0.02 (0.1 V) | iz kept (+7.2) |
| 9f36bbe `up` | +0.55 / -1.71 (5 V) / ... / +0.39 (24 V) | 6 V 0 V +0.31 | |

60 V in `down` on 9f36bbe: gsrc 3 at 5 V (gdis -7.4, the non-lag side), 0 V -0.69 mV. With the
larger creep, the 12 V level's gdis is only -2.2..-4.4 in the model (device -5..-12), so the model often catches
the bias one level later.

Emulator on 9f36bbe with the 38a63eb model: acdcmethtest A / B / C / B~,
`--e8 down` / `up` / `tc6` and couptest pass.

### 10.15 Device results of 10.14 (8e5ca2d full set, 9f36bbe confirmation; 2026-10-23 13:05-13:50, device-check doc "acdc-bias")

Run in the 10.13 order: reset, e8up; 6 x (reset, e8s), all with `--trace`; e8; e5 `--minutes 3`; e11 e10 e9. The
9f36bbe confirmation was reset, e8up, then 3 x (reset, e8s). Errors are C's DC part minus READ? of the DC V
function on the same manual range. Rule: 0.05 % + 5 digits.

| test | rule (mV) | 8e5ca2d | 9f36bbe |
|---|---|---|---|
| e8up 60 V 0 / 1 / 5 / 12 / 24 V | 5.0 / 5.5 / 7.5 / 11 / 17 | -0.63 / +1.83 / +2.95 / -1.13 / +1.35 | +0.90 / +1.66 / +0.55 / +0.02 / +1.19 |
| e8s 60 V 1 / 0 V (x6; x3) | 5.5 / 5.0 | -0.25..+0.80 / +0.03..+1.78 | -0.29..+1.06 / +0.46..+1.00 |
| e8s 60 V gain | | gsrc 3 at 12 V in 6 of 6 (gdis -5.2..-12.0, bthr 2.8-4.3), then gsrc 2 at 5 V | gsrc 3 at 12 V 2 of 3 (-9.4, -11.3); once at 5 V (12 V -3.3 < bthr 5.6; 5 V -11.8 > 7.3) |
| e8s 6 V 5 / 3 / 1 / 0 V | 3.0 / 2.0 / 1.0 / 0.5 | 0 V **+0.07 / -0.09 / +0.02 / -0.01 / +0.06 / -0.34** | 0 V **-0.14 / +0.09 / +0.02**; 3 V -0.58..+0.08 |
| e8s 6 V gain | | gsrc 1 at every level in 6 of 6; lag-side gdis +1.2..+12.8, bthr 22.9-40.3 | gsrc 1 in 3 of 3; gdis +3.0..**+16.2**, bthr 21.7-26.7 |
| e8 (1 s per level, gains kept) | as e8s | 60 V -1.04..+2.71; 6 V +0.29..+0.80 (0 V +0.29) | not run |
| e8kept 5 V on 60 V | 7.5 | +1.15 / -1.01 / -0.86 / -0.11 / +0.44 / +0.66 / +1.34 | +0.16 / +0.56 / +0.29 |
| e5 24 V / 60 V; 3 V / 6 V | 17 / 2.0 | -0.05..+1.88; -0.49..+0.27 | not run |
| e11 AUTO on the square, A / C x 3 | DC, <= 1 change | DC in all 6, 0 changes | not run |
| e10 square / 5.0 / 5.5 / 4.5 V | | AUTO,DC; -0.19 / -0.01 / -0.38 mV, one range, 9.3-9.4 rd/s | not run |
| e9 4.0 / 4.5 / 5.0 / 5.5 V | | +0.11 / +0.25 / -0.10 / -0.06 mV, one range, 9.3 rd/s, acsrc 0 | not run |

* The 6 V internal zero's gain was kept in all 9 e8s sessions.
* The largest lag-side gdis is now +16.2 codes (9f36bbe run 2, 3 V), against bthr 24.6. CREEP_K x step alone is
  16.6, so the margin over the largest value seen is ~8 codes, i.e. 3 sigma at 6 V. If later data shows lag-side
  values above ~20, raise CREEP_K, not the noise term.
* 8e5ca2d's 83 k pstep (e8s run 2, 1 V: bthr 40.3) is the late-reference case that 9f36bbe fixes. In 9f36bbe's
  runs every 1 V pstep is 41.2-41.6 k.

**Verdict: method C meets the decision rule (10.4 / 10.7 / 10.13) on this unit.**
* DC within 0.05 % + 5 digits at every level and in every e8s run: 9 of 9 sessions, both images.
* e8up, e8 and e5 pass.
* The AC part is within spec: e9 / e10 AC part 0.24-0.35 mV on DC (bound 30 mV); e11 / e10 as 10.8 / 10.11.
* Nothing in this change touches the A path or the AC part.

**Decision (2026-10-23): method A stays the AC+DC default** until C has been checked on the
A / mA / µA ranges with real current (TODO, plan in 10.16).

### 10.16 Plan: method C with real current (A / mA / µA)

C's A path (`acdcm.rs`, the C A entry reference: an offset, and a gain from C_A_GAIN x full scale) has been run
only in the emulator (acdcmethtest's A phases, couptest `i`). The calibrator has no current output, so this needs:
* a bench supply in CC (or CV with a series resistor);
* a load: a power resistor for A, a resistor decade for mA / µA;
* a second meter in series as the reference.
For a mixed signal, a transformer secondary or a function generator with a buffer, plus a DC bias through the
load.

| step | ranges | what | pass |
|---|---|---|---|
| i8 (e8-style) | 10 A / 600 mA / 60 mA / 6 mA / 600 µA (manual) | C entered at the top level, then 100 / 50 / 20 / 5 / 0 % of the range's full scale without leaving, 4 s + 3 s mean per level. Each level's DC A READ? from the meter itself first; the second meter logged throughout | DC part within the DC A spec of READ? at every level; the gain judgement (STAT? gsrc / gdis) logged as e8s; the shunt's self-heating on 10 A (> 5 A: record the drift over 60 s) |
| i8up | the same | entered at 0, stepped up | as i8 |
| imix | 600 mA, 60 mA | DC 50 % + 50 Hz AC 20 % of full scale (a transformer through a resistor, DC from the supply through the same load) | DC part as i8; AC part within the AC A spec of the second meter's AC reading; total = hypot |
| iauto | each range | CURR:COUP AUTO on DC, AC and the mix | as e11 / e10: DC on DC, AC on AC, the larger part on the mix, at most one change |
| ie5 | 600 mA at 50 % | C re-entered every 60 s for 5 min | DC part drift within the spec |

Scripts: a `tools/acdcmeth.py` variant with the supply's and the second meter's SCPI in place of the calibrator
(the MR9270 calls are the only source-specific code). Run i8 first after a power-on, as e8up.
