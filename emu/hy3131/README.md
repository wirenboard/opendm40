# HY3131 model for the DM40 emulator

[`emu/hy3131/HY3131.cs`](HY3131.cs) is the detailed model of the Hycon [HY3131](../../hw/datasheets/hycon-hy3131-datasheet-v10.pdf) DMM front end
([`emu/peripherals/HY3131.cs`](../peripherals/HY3131.cs) and [`emu/peripherals/DM40Frontend.cs`](../peripherals/DM40Frontend.cs) are relative
symlinks to the files here, so there is one copy to edit; emu/run.py reads them
through the links). What it is based on: [`RESEARCH.md`](RESEARCH.md) (datasheet, application note,
Hycon tool data, open-source drivers) and the stock firmware analysis in
`stock/re/analysis/hy3131_v2_6.h` (in the private analysis repo) / `stock/re/analysis/dmm_v2_6.h` (in the private analysis repo).

```
emu/hy3131/HY3131.cs      the model: Hy3131Core (chip, no Renode types) + HY3131 (Renode peripheral)
emu/hy3131/DM40Frontend.cs the DM40 front end: jacks, relays, shunts, divider / ohm network (emu/peripherals/DM40Frontend.cs links here)
emu/hy3131/tests/         host unit tests (dotnet 8): tests/run.sh
emu/hy3131/modetest.py    stock + opendm40 through every DMM mode in the emulator, results below
emu/hy3131/powtest.py     opendm40 power meter end to end over USB/IP (firmware/docs/power-meter-and-misconnect.md 3.11)
emu/hy3131/couptest.py    opendm40 AUTO coupling of V / A: DC -> AC -> mixed signals through the hysteresis band, taps, SCPI (stock/re/docs/history/acdc-stock-vs-opendm40.md 11)
emu/hy3131/autorangetest.py opendm40 DC V AUTO from 600 / 1000 / 60 V with the model's AD1 filter memory (ad1mix:1), READ? +9.91E37 through an AC+DC range change (device decaf60; stock/re/docs/history/acdc-stock-vs-opendm40.md 12)
emu/hy3131/mvautotest.py  opendm40 V autorange around the mV ranges: AUTO -> AUTO+ on 60 mV leaves mV at once, AC V AUTO (mV-first) under a square the mV input clamps (mvclamp / mvpb1, device 2f56ff9) up to 6 V and back
emu/hy3131/switchtest.py  opendm40 misconnect sentinel through ~60 function / range / coupling switches (and the Wave view) per connection: every alert assertion (DBG:SENTinel:LOG?) must be one the connection explains; real-* scenarios time A-JACK / A-PROT / AC? / DC? / V-JACK? after a switch in; swtr: the model's hypothetical switch transient
emu/hy3131/ohmhunt.py      opendm40 ohm range hunt with the model's ohm node lag (ohmtau): no FETCh? / READ? / amber readout value off the final one (device 6bc494a new bug 1; docs/history/switch-latency.md 6.2)
emu/hy3131/autowaittest.py opendm40 READ? / MEAS? / FETCh? and HOLD while AUTO coupling decides, the placeholder unit after CONF (switch-latency.md 6, 6.2)
emu/hy3131/ad2streamtest.py opendm40 AD2 stream ISR (feature ad2-isr: TIM7 -> IRQ 55 -> SPI2 reads) end to end (firmware/docs/dmm-input-scope-feasibility.md 9)
emu/hy3131/wavetest.py    opendm40 DMM wave view (scope page, source DMM; feature ad2-isr) driven by DBG:KEY / TAP, checked over SCPI, --case current, --shots DIR (feasibility 10)
emu/hy3131/burdentest.py  opendm40 burden voltage readout: FETC:BURD? = READ? x R_path per shunt / AC / POW:BURD, DISP:BURD, screenshots (firmware/docs/power-meter-and-misconnect.md 3.15)
emu/hy3131/inrushtest.py  opendm40 inrush current (A jack, 10 A AC row on the AD2 stream): timed isrc steps (DC lamp-like, AC load), MF -> Inrush, INR:* SCPI, the saved FAST CSV; private disk copy + log per process, --shots DIR (firmware/docs/inrush-current.md)
emu/hy3131/btwaketest.py  opendm40 DMM rail (PE2) wake: BT link on the Scope page, link cycles, a chip POR (device check 2026-10-21 Bluetooth 1g / 1h)
emu/hy3131/runemu.py      emu/run.py with a separate merged compile unit (the same files since the peripherals/ symlinks)
emu/hy3131/RESEARCH.md    sources, facts, open points
emu/hy3131/ref/           transcribed register map and mux tables
```

## Terminal-level model (DM40 front end)

[`DM40Frontend.cs`](DM40Frontend.cs) (`Dm40Frontend`) sits between the jacks and the chip. The
user says what is **connected** to V-ohm / COM / A; the firmware's own relay
pulses (PA9 / PA10 latching pair = V/A path or the ohm-mV network, PE0 / PE1 =
A / mA / none shunt, PC7 = frequency mode; [`firmware/src/dmm.rs`](../../firmware/src/dmm.rs) relay_pa /
relay_pe / pc7, stock FUN_08012780 / FUN_080127be) and its [HY3131](../../hw/datasheets/hycon-hy3131-datasheet-v10.pdf) row decide
the route, so a fixed connection behaves like on the real meter when the
function changes:

| connected | DC V | DC A | ohm | diode | cap | temperature |
|---|---|---|---|---|---|---|
| voltage source V-COM | V | 0 (nothing in A) | KCL node of the ohm source against the source: clipped (OL) or a wrong value | OL / Vsource | no oscillation | reads it as thermocouple mV |
| current source A-COM | 0 (nothing on V) | I x relay-selected shunt | OL | OL | - | - |
| resistor | 0 | 0 | R (ratiometric) | 0.765 mA x R | no oscillation (R < 3 Rref) | 0 mV |
| diode | 0 | 0 | Vf / (Vs - Vf) x Rref | Vf (reversed: OL) | no oscillation | - |
| capacitor | 0 | 0 | charges through Rref: rising, then OL | OL | f = 1 / (2 ln2 R C) or the current ramp | - |
| thermocouple | E(T) - E(Tcj) | 0 | - | - | - | T (die sensor as cold junction) |
| voltage source in the A jack | 0 | V / (Rs + shunt) -> OL | OL | | | |

Queries the chip model makes: `VJack(path)` (Thevenin of the V-COM elements
into 10 M, gated by the relay: 0 when the jack is routed elsewhere),
`ACurrent()` (through the shunt the relays selected; the row's OP1 gain and
tap scale it), `OhmNode(Vs, Rref)` (DC operating point: the OP3 source at VREF
through the reference resistor against everything on the jack; a plain
resistor gives `Vx / Vref = Rx / Rref` exactly, the continuity row's swapped
reference mux gives the negative code), `DiodeNode(0.765 mA, 3.3 V)`,
`CapOscC(R)` / `CapRampC(I, Vthr)` (0 or NaN when the network cannot
oscillate / ramp), `TcEmf()`. The calibration record is folded in physically:
Rref x gain (ohm r0: 0.1 k x r1's gain x the DC mV x10 / x1 gain ratio, as
the firmware's `ohm_row_gain`), the ohm offset as lead resistance, the V / A gains on the
source. Cold junction (`cj`, the jacks) and HY3131 die (`die`, what SAD1I 10/11
report) are separate; the die follows the cold junction unless set
(`die:35`): a thermocouple at 100 C with cj 25 and die 35 reads ~110 C on both
firmwares, because they take the die sensor as the cold junction.

Unknowns: the DM40 netlist (the per-row scale factors are the
stock's nominal constants), the ohm source open-circuit voltage (SFUVR table
at VDDA 3.6 V: 1.4 V on the normal ohm rows, 0.625 V on the "ON" variant,
-0.625 V continuity; measured on one unit with DBG:SRC, 600 ohm range:
about 2.17 V open / 0.90 mA shorted in the default variant, 0.78 V /
0.28 mA in the "ON" variant, [device check](../../docs/history/device-check-2026-10-21.md#ohm-test-current-map-dbgsrc);
not modelled) which only the cross cases depend on, and PE2 (DMM rail /
power hold) which is not modelled as chip power.

## What is modelled

Chip (datasheet DS-HY3131-V10):

* Register file R00..R3F with the datasheet semantics: data registers read
  only; INTF cleared when read, writing 0s clears bits, 1s cannot be written;
  INTE and the IRQ on DO while CS is high; CTSTA (ACPO in bit 6, PCNTI, CTBOV
  cleared by reading); CTA<23:8> writable only while ENCTR = 0, CTA<7:0> held
  at 0; ENCTR = 0 clears CTB / CTC / CTBOV; ENAD1 / ENAD2 / ENLPF / ENRMS = 0
  clear their data; ENPKH = 0 presets PKHMAX / PKHMIN; R37 = 0x60 resets the
  chip; reserved "0" bits read 0, "-" bits are kept; auto-increment reads and
  writes, the read period clock, DO changing on the falling edge (`HY3131`).
* Conversion timing: AD1 period = `OSR1 * 30 / 4.9152 MHz`, doubled with
  AD1CHOP = 1x (R22 0x13 / 0x15 / 0x17 -> 25 / 100 / 400 ms; RESEARCH.md 3);
  AD2 / AD3 at `OSR2 * 2 (or 4) / SYSCLK` (13.02 us); LPF and RMS results every
  `AD2 period * OSR4 * 0.5` (the measured 106.7 ms for LPFBW = 111); AD1F,
  AD2F, LPFF, RMSF, CTF set at those times and accumulated until INTF is read.
  AD2 runs on its own grid from the last R26 change (as AD1): one AD2F and one
  fresh R03..R05 per period, the input at the grid point plus a per-result
  noise model (30 codes at OSR2 32, white scaling with OSR2; ENCHOPAD2 changes
  neither the rate nor the noise, as on the device:
  `Ad2NoiseCodes`; a model, not a measurement), an unread result overwritten;
  a read is never torn (the chip advances at CS low only). The row
  recognition also accepts an AC row with AD3 / RMS / LPF off and another
  AD2 rate (opendm40 data logger, LOG:SOURce AD2).
  Conversions are evaluated lazily when the firmware touches the bus (no
  Renode timers): `Hy3131Core.Update(now)`.
* AD1 transfer: `code = IG * Vin / (RG * VR) * 2^23 + DCOS / RG * 2^23`, IG /
  RG / DCOS from R25 / R23 / R22, clipped to +-8388607; AD2 / LPF 19-bit, RMS
  38-bit = sum(X^2)/N of the AC part (HPF) in AD2 codes, `SDO23` / `SVXI`
  selection; peak hold of AD2 / AD1<23:5> / LPF.
* Frequency counter: gate opens on the first input edge after ENCTR = 1, CTA
  counts SYSCLK from the preset, after the overflow the count stops on the next
  edge: CTB whole cycles, CTC high time, CTF; no edges -> CTA keeps running,
  CTB = 0, no CTF; CTBOV on CTB overflow. Source ACPO (comparator) or the CNT
  pin (ENCNTI). A change of the input frequency (`SetInput`) restarts the
  waveform with a rising edge at the change (INFERRED from the MR9270 on the
  device: a gate across a 1 Hz -> 0.5 Hz step read 1.40 Hz); a gate open
  across it counts the old period up to the change, then the new one.
* opendm40 misconnect sentinel nodes (firmware/src/sentinel.rs): the comparator on
  SCMPI = PB1 sees the A current times the relay-selected shunt (DC V / mV /
  ohm / temperature roles), on SCMPI = SENSE in DC A the V jack / 1000 when SS5
  is closed (CMPHO / CMPLO instantaneous, DC + AC waveform); AD1 on PB1 - PB3
  in the AC V / mV roles, with the AD1 peak hold over every elapsed conversion.
  Row matching ignores AD1's fields in rows that leave AD1 off, PKHSEL /
  ENPKH, and SS5 in the current rows.
* opendm40 power meter rows ([`firmware/docs/power-meter-and-misconnect.md`](../../firmware/docs/power-meter-and-misconnect.md) 3.1,
  assignment B; role `Power`, mode 16, `Power vr<V range> iu<I unit> ir<I
  range>`, Range = vr * 6 + iu * 2 + ir): the 16 distinct rows of the design's
  builder (`Hy3131Core.PowerRow`; iu 1 / 2 share the bytes, PE tells mA from
  uA) with R34 SVXI / SDO23, R35 AD3IN / AD3IG, R20, R22, R36 and the I row's
  R2E / R2F = 08 81 free. X = AD2 = A current x relay-selected shunt x OP1 (x10
  with OPS1) x AD2IG; Y = AD3 = V jack / kV[vr] (10 .. 10000) x OP2 (x10 with
  OPS2) x AD3IG / AD3RG; codes of 1.25 V / 2^18, both DC coupled. RMS block:
  SDO23 -> sum(Yac^2)/N, else SVXI -> sum(Xac*Yac)/N (signed: 38-bit two's
  complement in RMS<37:0>, bits 39:38 sign copies), else sum(Xac^2)/N; LPF =
  DC of Y with SDO23, else of X; R03..R05 = Y with SDO23, else X. The cross
  term is Xac * Yac * mean(u_V(t) u_I(t - ph)) over one period of the unit
  waveforms (sine: cos ph; square / triangle in phase: 1; different V / I
  frequencies: 0); XY noise is the uncorrelated residue (floor x
  sqrt((X^2 + Y^2 + floor^2) / N)), XX / YY keep the 30-code floor. SPHACAL
  (R36) shifts only the cross term: <6:0> AD2CLK periods (0.407 us) of the I
  channel, bit 7 = 1 advances I (phase lag reduced by 360 f n / 2.4576 MHz
  deg), bit 7 = 0 delays it. An R34 statistic change in the middle of a block
  makes that block (RMS and LPF) the time-weighted mix of the selections it
  saw; the next block is clean (`powmix:0`: no mixing; the real chip's
  behaviour is unknown, design 3.4 / device test T3). Calibration: X carries
  the stock AC A gains folded in (AC part / cal(0xC4 + 4 ci), DC part /
  cal(0xF4 + 4 ci) or, while that is the default 1.0, / cal(0xC4 + 4 ci);
  ci = iu * 2 + ir, no offsets); Y carries the factory-derived tap gain
  (6 V: DC V r0 x AC V r1 / DC V r1, else AC V r1..r3; opendm40
  pw::v_gain_factory) times `powvg:` (a deviation for the V self-cal tests).
  `powz:1` adds the device's LPF zeros (Y +1080 codes, X +888 with OP1 x10,
  +155 with x1; doc 3.11 T1 / T3). The comparator / counter: SCMPI = SENSE
  sees the V tap DC-coupled (dc + AC against VRH / VRL: a unipolar signal
  never crosses a negative VRL), SCMPI = PB4 the current's AC at OP1O. The
  opendm40 DC row (powermeter.rs `dc_row`: the power row at YY plus AD1 = OP1O
  - PB3 with the DC A row's AD1 bytes) is a power row with AD1 on the
  current, the stock DC A calibration folded in (`powz:1`: +300 codes of
  AD1 zero). The f8_9 DC A rows fold the uA calibration when PE selects the
  47.5 ohm shunt (stock mode 9).
* opendm40 DC-coupled V row (ad2exp.rs `dcv_row`, feasibility E7): an AC V row
  with SAD2IP = OP1O is DC-coupled: OP1O = g (SENSE + Vos), SENSE = the V
  jack through the row's tap (6 V: /100 with OP1 x10), OP1+ on AGND = Vos
  alone; OP1's chopper (OP1CHOP 1 / 2) flips Vos only while AD1 converts.
  Options `op1vos:V` (default 0.37 mV, the AC V sentinel row's device floor)
  and `ad2dcg:` (AD2 gain deviation in that row, default 1). AD1 on SENSE -
  PB5 in an AC V row carries the DC V calibration of the tap and integrates
  (the DC part) at OSR >= 2048.
* Comparator: VRHCMP / VRLCMP from the R21 mux tables and the reference divider
  (VDSC<n> = K1 * VDDA, AGNDP/N<n> = AGND +- K2 * REFO), latch -> ACPO, CMPO on
  the CMP pin when ENPCMPO = 1 (`CmpPin`: the continuity beeper).
* Temperature sensor on SAD1I = 10 / 11 (differential pair, sum removes the
  offset), with the stock's constants (RESEARCH.md 7).
* Noise: Gaussian, 25 AD1 counts at OSR 32768 scaled by 1/sqrt(OSR) (acdc doc
  6c), RMS floor 30 counts; `noise:0` turns it off (deterministic seed).
* Empirical, optional: the AD2/AD3 enable-order beat of
  `stock/re/docs/history/acdc-stock-vs-opendm40.md` (in the private analysis repo) 10 (`beat:1`; off by default: the stock's
  emulated SPI skew is not the real one).

Board (DM40, from the stock rows and formulas): the input terminals are a
`Hy3131Input` (V/COM waveform: DC + AC RMS at a frequency, sine / square /
triangle, duty; A terminal current; R / C / diode on the ohm path; thermocouple
and die temperature). Each stock configuration row (hy3131_v2_6.h, 42 rows) is
recognised by a masked compare of R20..R35 (R21 / R22 ignored, enable bits,
SFUVR, SOP1P / ENOP1 and SAD1I masked, so opendm40's enable-order variant and the
run-time R22 / R32 / R33 writes still match) and gives the analog path:

| role (stock mode) | converter and path | chain used by the model |
|---|---|---|
| DC V 1 (r0..r3) | AD1, ref 1.25 V | `code = V / (k[r] * 1.25/0.9 / 2^23)`, k = 10 .. 10000; negative-side gains and the r0 correction of the stock inverted |
| DC mV 3, DC A 7..9 | AD1 through OP1 (x10 / x1) | stock constants: mv_div, shunt * opgain * 0.9 |
| AC V 0 | AD2 + AD3 on PB4 (AC coupled, tau 0.3 s), RMS + LPF | `Vnode = V / (k[r] * AD2IG)`; LPF = remaining DC step on the coupling capacitor (charges to the OP1O level, -5 mV while OP1 is off: fitted to the AC mV -> AC V entry of device 185c066, `Op1OffLevel`), RMS = AC + 1.5 x that step |
| AC mV 2, AC A 4..6 | AD2 through OP1, DC coupled | LPF sees the full DC |
| ohm 10 (r0..r6), continuity 11 | AD1 ratiometric to Rref[r] (0.1 k .. 63 M, 1 k) | `code = 0.9 * Rx / Rref * 2^23` (negative in continuity: reference order), clipped (open = OL); comparator window 41.7 mV / 0.625 mA = 67 ohm -> CMP pin |
| diode 12 | AD1 on the /10 divider (SDIO) | `code = Vf / 10 / 1.389 * 2^23`, open = 3.3 V (OL) |
| capacitance 13 r0..r4 | RC oscillator into the counter | `f = 0.72135 / (R[r] * (C + stray))`, R = 1020 k .. 1.5 k |
| capacitance 13 r5..r7 | constant current ramp on AD1 | `dV/dt = I[r] / C` (4.06 / 41 / 400 uA at the AD1 node) between the R21 thresholds, ACPO toggles, discharge row -> 0 V |
| frequency 14 | comparator on the input (+-20.8 mV) or CNT pin | counter; AD2 RMS of the level |
| temperature 15 | AD1: sensor phases (R33), thermocouple on the mV x10 path | type K EMF (NIST ITS-90) of (hot - cold junction) |
| power (opendm40, mode 16) | AD2 = I (stock AC A path), AD3 = V tap through OP2, RMS = XX / XY / YY by R34, LPF | `X = I * shunt * OP1 * AD2IG`, `Y = V / kV[vr] * OP2 * AD3IG` in 1.25 V / 2^18 codes; XY signed (above) |

The real PA / PB wiring of the DM40 is not known (hy3131_v2_6.h); the chain
constants are the stock's nominal scale factors, i.e. the design intent of the
AFE + board, applied per recognised row. A configuration that matches no stock
row reads 0 (role `None`); `sysbus.hy3131 Role` shows the match.

### Calibration consistency

The meter multiplies every raw reading by the gains of its calibration record
(flash 0x080C0168; e.g. DC V r0 gain 1.012 on the dumped unit). The model
therefore produces the raw codes of *this* board: at the first SPI transfer it
reads the record from the emulated flash and inverts the stock's correction
(`raw = (x + offset) / (gain * scale)` per path), so the stock and opendm40
display the scripted input. `SetCalConsistent false` (or `cal:0` in the input
spec) gives the ideal chain instead (the stock then shows 1.234 V as 1.2488 V).

## C# API (Renode monitor: `sysbus.hy3131 <method> args`)

| call | meaning |
|---|---|
| `SetInput "spec"` | the inputs; several specs separated by `;` (below) |
| `SetVin V` | DC voltage on V / COM (old scripts) |
| `SetAd1 code` / `ClearAd1` | force a raw AD1 code |
| `SetAd2 code` | force a raw AD2 code |
| `SetCalConsistent true/false` | apply the meter's calibration deviations (default true) |
| `SetNoise scale` | converter noise scale, 0 = off |
| `SetCntPin true/false` | level of the CNT pin (when the firmware selects it) |
| `BrownOut` | chip reset with BORF set |
| `RailModel true/false`, `PowerUpMs ms` | PE2 power gate (default on, 2 ms): PE2 low = unpowered (writes ignored, DO low); a PE2 rise = power-on reset (registers cleared, BORF) and the bus ignored for `PowerUpMs`. Until the firmware first raises PE2 the chip counts as powered (a ROM-raised rail, the unit tests). `RailUps`, `DeafFrames`: rises seen, frames ignored |
| `Status`, `Role`, `Mode`, `Range`, `Intf`, `Ad1`, `CmpPin`, `Transfers` | state for scripts and tests |

Connection spec (`emu/run.py --dmm SPEC[@T]`, script lines `T dmm SPEC` or
`T monitor sysbus.hy3131 SetInput "SPEC"`, the GTK panel). Elements add up
(several with `;`), `open` clears everything, `KEY:none` removes one:

| spec | connected |
|---|---|
| `vsrc:1.234[,ac=1.0,f=50,shape=sine|square|triangle,duty=0.3,rs=4700]` | voltage source V-COM: DC, AC rms, source resistance |
| `isrc:0.0123[,ac=,f=,shape=,duty=,compl=10,ph=DEG]` | current source A-COM (compliance V); `ph`: phase of its AC part behind the V source's AC part (positive = current lags, inductive; PF = cos ph) |
| `pow:230@50,i=1[,pf=0.8\|ph=DEG,dc=VDC,idc=IDC,f=HZ,shape=]` | power meter shorthand: V source (AC rms 230 at 50 Hz + dc) on V-COM and current source (AC rms 1 A at the same frequency + idc) into A, the current lagging by `ph` or `acos(pf)` (`pf < 0` = energy flowing out, P < 0; `ph` wins over `pf`); negative `idc` = negative DC power. Replaces only the V and A sources (other elements stay; prefix `open;` for a clean connection). With `emu/run.py --dmm` the last `@` is the time: write `pow:230,f=50,i=1` or `pow:230@50,i=1@0` |
| `r:4700`, `short` (= `r:0`), `c:100e-9`, `diode:0.65[,rev=1]` | resistor, short, capacitor, diode (anode at V unless `rev`) between V and COM |
| `tc:100`, `tcmv:4.096`, `tcopen` | type K thermocouple: hot junction C, or its mV, or connected but open |
| `cj:25`, `die:35` (`die:none` = follows cj) | cold junction (jacks) and [HY3131](../../hw/datasheets/hycon-hy3131-datasheet-v10.pdf) die temperature |
| `avsrc:0.5[,rs=]`, `isrcv:0.001`, `aload:100` | wrong-jack cases: voltage source across A-COM, current source across V-COM, resistor across A-COM |
| `dcv:`, `acv:V@Hz`, `hz:F,duty=,amp=`, `dca:`, `aca:`, `ohm:`, `cap:`, `temp:T,cj=`, `chip:` | the older per-function keys: each replaces the whole connection with the equivalent one |
| `noise:0`, `cal:0`, `beat:1`, `powmix:0`, `ad1mix:1`, `swtr:300;swtau:50;swhz:25`, `ohmexit:3.5;ohmexitlo:1.2`, `ohmexitrst:1`, `ohmtau:150`, `pb5drift:0.6;pb5tau:2.2`, `acastep:200000;acastepms:600`, `vjblind:1` | model options (device 40728b8 replays, off by default: `pb5drift:V;pb5tau:s`: in the AC A rows AD1 on SENSE - PB5 sees PB5 rise by V x (1 - exp(-t / tau)) from the first AC A row (to the negative rail in ~2 s), SENSE - AGND does not; `acastep:codes;acastepms:ms`: AC A 10 A -> mA / uA, AD2's LPF / RMS blocks overlapping the first ms of the row carry a step of codes (the length is chosen, not measured); `vjblind:1`: the DC A rows' SENSE comparator reads 0 unless a temperature row ran since the last DC V r1+ / ohm / continuity / diode / capacitance / frequency row or reset; `ohmexit:s;ohmexitlo:s`: after the relay leaves the ohm network while an ohm row runs, the A rows' SENSE comparator reads CMPHO+CMPLO for ohmexit s, then CMPLO for ohmexitlo s more, the device 469f354 trace, off by default; `ohmexitrst:1`: the same trace also from a chip reset (the DMM rail back after the Scope page: device 6bc494a, 0x70 to 7.75 s), off by default; `ohmtau:ms`: in the ohm rows the node across Rx relaxes to its value with this time constant after every row change (an ohm range step), from 0 V outside them (the input RC / a calibrator's resistance simulation catching up; the constant is chosen, not measured: device 6bc494a read 219 / 57 ohm on the right range before 390 / 100 ohm), off by default; `swtr:mV`: a HYPOTHETICAL switch transient, from every row change PB1, the V tap and the A rows' SENSE tap carry mV x exp(-t / swtau ms) x cos(2 pi swhz t), default tau 60 ms, 25 Hz, off by default; shape not yet from the device; `powmix:0`: an R34 switch in the power role does not mix the block in progress; `ad1mix:1`: a row write that does not restart AD1, same R22 and ENAD1 kept, leaves the next conversions a sinc^3 mix of the old and the new input over 3 periods, as the device's 2-step DC V autorange, decaf60; off by default; `mvclamp:V`: in the AC mV rows the V jack is clipped to +-V before the node (the clipped waveform's mean and AC RMS; device 2f56ff9 read 0.606 V on 600 mV with a 0 / 9.5 V square: `mvclamp:1.212`; where the clamp sits and its hard shape are INFERRED), off by default; `mvpb1:k`: with mvclamp, PB1 in the AC mV rows carries k mV per V of the jack beyond the clamp (the same square's AJACK,AD1PK_PB1; k INFERRED), off by default; `ohmrevol:0`: the ohm rows' plain ratio for a positive source above the ohm source's open-circuit voltage (a negative code, LIVE?); default 1 = OL, as the device reads it (519fb8a 7b); `ohmop1k:k`: in the ohm rows OP1O (the jack check's AD2) stops at 2.47 - k x the jack's volts, default 0.044 (device 1a11eda: 2.25 V at 5 V, 2.03 V at 10 V; the line is INFERRED), 0 = the plain swing clip; `ohmboth:V`: in the ohm rows a jack at or above V volts (instantaneous) sets CMPHO and CMPLO together on PB1 (CTSTA 0x70, device 1a11eda / f540f0d E4; 3 V no hit, 5 V 0x70: 4 V INFERRED), default 4, 0 = off; `acarrive:ms`: after the AC V row is entered from another role the node carries the jack's AC only from ms later, an RMS block over that moment reports its share of the mean square (device 185c066: DC V -> AC V read 0.707 of the input once; `acarrive:160` puts the last block before the 300 ms settle half over it; the delay is INFERRED), off by default; `mvhold:V;mvholdpb1:mV[;mvholdrows:n]`: after a source on V-COM goes off (`vsrc:none` / `open`) the mV rows read V DC on the open input and PB1 carries mV DC, until the next source (n: 1 the DC mV row only, 2 AC mV only, 3 both, the default; device 185c066's A-JACK after the off in AC+DC mV; the charge and the pickup are INFERRED), off by default; `acaoff:mV`: the AC A rows' AD2 path reads mV (at the shunt's side, through the row's OP1 gain) as a DC offset in the LPF and the AD2 samples (device 185c066: +0.0717 A on the 10 A row with the A jack open = `acaoff:0.717`; where it arises is INFERRED), off by default; `hfbw:Hz;hfdc:k;hfpb1:mV;hfpb2:mV`: a signal on V-COM above the AC bandwidth (device 2026-10-26, a 66 kHz generator, A jack empty: A-JACK / A-PROT on the mV ranges, DC? in AC V): the AC V / AC mV RMS / LPF blocks see the jack's AC part / (1 + (f / Hz)^2), the counter and the instantaneous samples are unchanged; above Hz the DC mV node and the AC V row's V tap carry k x the jack's AC rms as DC, PB1 in the AC mV / DC mV rows mV per V of the jack's AC (`hfpb1`), PB2 in the DC mV row the same (`hfpb2`); all INFERRED, chosen to replay the device's alerts (senttest `hfbw:6000;hfdc:0.3;hfpb1:10;hfpb2:1`), off by default; `hfnocnt:1`: the AC mV row's counter reads nothing for such a jack (device 2026-10-26, the 66 kHz square); `hfboth:p`: in the DC V / DC mV rows each comparator update on PB1 reads CMPHO + CMPLO with probability p, else nothing, for such a jack (the device's CTSTA 0x70 / 0 in DC V AUTO and DC mV); INFERRED, off by default; `pb1cmp70:0`: PB1 above the comparator's window reads CMPHO alone (the model before 2026-10-26); default 1 = CMPHO + CMPLO (0x70), as the device reads every level above the window (CMPHO never alone, 40728b8; a current in A reading 0x70 is INFERRED); `hzpb1:mV`: the frequency row's PB1 - PB3 stands at mV (PR #55, SAD1I 00: +2.5 with V-COM open, -140 at 10 V DC; d8e249d's ~54 came with SAD1I 11), off by default; `ailift:k;ainz:mV[;ainzhz:Hz]`: a current in A with the V jack open puts k x its PB1 drop (DC) and mV RMS at Hz (default 50 kHz) on the AC mV row's input (device 2026-10-26, the owner's PSU 20 mA in A: AC mV OL, Hz ~50 kHz, DC mV +139 mV; senttest `ailift:0.146;ainz:1500`; INFERRED), off by default. `aiohm:ohm`: the same current pulls the open V input like ohm to COM (RES AUTO read 31 Mohm there: `aiohm:31e6`; INFERRED), off by default. Always on (opendm40 jack check, sentinel.rs JACK CHECK): AD2 = OP1O - PB3 in the ohm / continuity rows reads the node across Rx (the source's AC part included, within -1.18 .. +2.55 V); the monitor's `Pb1VjackMv` also puts the V jack on PB1 in the continuity row) |

State: `sysbus.hy3131 Frontend` returns one line of `key=value` tokens (relay
and shunt state, role / mode / range, which converters and switches are on,
the V-jack voltage, the A current and shunt drop, the ohm node, the
connection); `StateFile @path` writes the same line to a file on change (the
GTK panel's schematic reads it).

## Tests

[`emu/hy3131/tests/run.sh`](tests/run.sh) (dotnet 8; `DOTNET=` to point at an SDK) compiles
the real [`HY3131.cs`](HY3131.cs) with a 60-line Renode shim and drives it through the SPI
pins like the firmware: 297 checks of the register semantics, the frame,
conversion timing, DC / AC / A / ohm / continuity / diode / capacitance /
frequency / temperature physics, calibration consistency, the AC coupling
transient, the AC+DC cadence, the (optional) enable-order beat, the misconnect
sentinel and the opendm40 power rows (recognition, PF 1 / 0.5 / 0 / -1, DC, the
38-bit sign, SPHACAL, statistic-switch mixing, the counter, the stock AC V /
AC A rows with both jacks driven).

[`emu/hy3131/modetest.py`](modetest.py) runs the stock firmware and opendm40 in the emulator:
the input is scripted, the stock is switched with Bluetooth command 06 and
polled with command 09 (the values the LCD shows, `stock/re/analysis/bt_meas_v2_6.h` (in the private analysis repo)), opendm40
with `CONF:<f>` / `READ?` over the same link. Results (last reported value
after settling; stock Bluetooth is enabled by patching settings byte 0x115 in
the emulated flash with a recomputed CRC):

| scenario | input | expected | stock shows (BT cmd 09) | opendm40 READ? |
|---|---|---|---|---|
| dcv | `dcv:1.234` | 1.234 V | 1.2340 | 1.234 |
| dcv-neg | `dcv:-12.5` | -12.5 V | -12.500 | -12.5 |
| dcv-hv | `dcv:230` | 230 V | 230.01 | 230.0 |
| dcmv | `dcv:0.2345` | 234.5 mV (auto -> mV) | 234.5 | 234.494 |
| acv | `acv:1.0@50` | 1.0 V rms | 1.0000, 50.00 Hz | 0.999982 |
| acv-hv | `acv:230@50` | 230 V rms | 230.0, 50 Hz | 229.998 |
| acdcv | `acv:1.0@50,dc=0.5` | 1.1180 V (AC+DC) | 1.1181 | 1.11806 |
| dca | `dca:0.5` | 0.5 A (auto mA -> A) | 0.5001 | 0.499997 |
| dcma | `dca:0.0123` | 12.3 mA | 12.30 | 12.2999 |
| aca | `aca:0.1@60` | 0.1 A rms | 99.99 mA | 99.9989 mA |
| ohm | `ohm:4700` | 4700 ohm | 4700 | 4699.99 |
| ohm-1m | `ohm:1e6` | 1 Mohm | 1.0001 M | 999987 |
| cont | `ohm:10` | 10 ohm | 10.00 (beeper: CMP pin) | 10.005 |
| diode | `diode:0.65` | 0.65 V | 0.6501 | 0.650016 |
| cap | `cap:100e-9` | 100 nF (RC oscillator) | 100.0 nF | 100.002 nF |
| cap-big | `cap:100e-6` | 100 uF (current ramp); stock: 104 uF | 104.0 uF (+4 %, see below) | 102.03 uF (+2 %) |
| hz | `hz:1000,duty=0.25,amp=1.0` | 1000 Hz, 25 % | 1000.0 Hz, 25.0 % | 1000.02 |
| temp | `temp:100,cj=25` | 100 C, CJ 25 C | 100.3 C, CJ 25.0 | 100.29 |

(`modetest.py -j 6` at 2026108.) All within the
meter's specification except the capacitance ramp ranges. The stock's +4 % is its
own arithmetic (`stock/re/analysis/dmm_v2_6.h` (in the private analysis repo) section 4):
C = gain * I * dt / median5(dV), dt from its millisecond counter between two reads.
A read waits until 255 ms after the previous one (deadline = that read + 0xff) on the
DMM screen's 10 ms poll, so dt = 260 ms; dV spans whole AD1 conversions (50 ms at the
ramp row's OSR 4096 with the chopper), 5 of them between most reads and 6 between one
read in 5, so the median dV is 250 ms of ramp and C = 260 / 250 x 100 uF = 104.0 uF.
The factory calibration leaves the r5..r7 gains at 1.0. With the model's AD1 period
in the ramp row stretched to 52 ms (an experiment, not kept) the stock reads 100.1 uF.
modetest therefore expects 104 uF from the stock (`stock_want`) and 100 uF from
opendm40. That the real meter shows the same +4 % is inferred (its 10 ms poll and AD1
period are taken to match the emulator's); it is not measured. opendm40's +2 % is not
analysed here. Run time per scenario: 8..18 virtual seconds,
25..80 s wall at `-j 6`.


### Fixed connections x functions (cross table)

`modetest.py --cross`: one connection, every function, both firmwares
(readings as reported over Bluetooth cmd 09 / SCPI READ?; OL = no valid
reading; cells are the last value after the settle; blank = not run):

| connection | fw | dcv | acv | dca | aca | ohm | cont | diode | cap | hz | temp |
|---|---|---|---|---|---|---|---|---|---|---|---|
| V 1.234 `vsrc:1.234` | stock | 1.234 | 0.00012 | -2.71e-06 | 7.2e-06 | OL | 0 | 1.234 | 0 | 0 | OL |
| V 1.234 `vsrc:1.234` | fwrs | 1.23399 | 0.017731 | -2.71462e-06 | 7.20722e-06 | OL | -665.907 | 1.23402 | 3.14321e-15 | 0 | OL |
| V 1.234 + I 12.3 mA `vsrc:1.234;isrc:0.0123` | stock | 1.234 | 0.00012 | 0.012301 | 2.2e-05 | OL | 0 | 1.234 | 0 | 0 | OL |
| V 1.234 + I 12.3 mA `vsrc:1.234;isrc:0.0123` | fwrs | 1.23399 | 0.017731 | 0.0123003 | 2.19533e-05 | OL | -665.908 | 1.23401 | 3.14321e-15 | 0 | OL |
| I 12.3 mA `isrc:0.0123` | stock | 0 | 0.00012 | 0.0123 | 2.2e-05 | OL | OL | OL | 0 | 0 | 24.7 |
| I 12.3 mA `isrc:0.0123` | fwrs | -1.55273e-06 | 0.00083487 | 0.0122997 | 2.19349e-05 | OL | OL | OL | 3.52156e-15 | 0 | 24.6698 |
| R 4.7 k `r:4700` | stock | 0 | 0.00012 | -2.71e-06 | 7.2e-06 | 4700 | OL | OL | 0 | 0 | 24.7 |
| R 4.7 k `r:4700` | fwrs | 2.00805e-06 | 0.000830868 | -2.70869e-06 | 7.20722e-06 | 4699.99 | OL | OL | 3.14321e-15 | 0 | 24.6942 |
| 0.1 V behind 4.7 k `vsrc:0.1,rs=4700` | stock | 0.09995 | 0.00012 | -2.71e-06 | 7.2e-06 | 4.213e+06 | OL | OL | 0 | 0 | OL |
| 0.1 V behind 4.7 k `vsrc:0.1,rs=4700` | fwrs | 0.0999482 | 0.00144192 | -2.70207e-06 | 7.20592e-06 | 779228 | OL | OL | 3.14321e-15 | 0 | OL |
| diode 0.65 `diode:0.65` | stock | 0 | 0.00012 | -2.71e-06 | 7.2e-06 | OL | OL | 0.65 | 0 | 0 | 24.7 |
| diode 0.65 `diode:0.65` | fwrs | -1.55273e-06 | 0.000843113 | -2.70032e-06 | 7.20592e-06 | 4.31845e+07 | 3.01894e-06 | 0.650005 | 3.14321e-15 | 0 | 24.6698 |
| C 100 nF `c:100e-9` | stock | 0 | 0.00012 | -2.69e-06 | 7.2e-06 | 3.8684e+07 | OL | OL | 1e-07 | 0 | 24.7 |
| C 100 nF `c:100e-9` | fwrs | -1.55273e-06 | 0.000833298 | -2.7066e-06 | 7.20592e-06 | 5.77772e+07 | OL | OL | 1.00002e-07 | 0 | 24.6698 |
| TC 100 C (cj 25, die 25) `tc:100;cj:25` | stock | 0.00309 | 0.00012 | -2.71e-06 | 7.2e-06 | 12.2 | 4.68 | 0.0108 | 0 | 0 | 100.3 |
| TC 100 C (cj 25, die 25) `tc:100;cj:25` | fwrs | 0.00309109 | 0.000834009 | -2.70207e-06 | 7.20592e-06 | 12.196 | 4.68485 | 0.0107508 | 3.52156e-15 | 0 | 100.314 |
| TC 100 C, die 35 `tc:100;cj:25;die:35` | stock | 0.00309 |  |  |  |  |  |  |  |  | 110.3 |
| TC 100 C, die 35 `tc:100;cj:25;die:35` | fwrs | 0.00309109 |  |  |  |  |  |  |  |  | 110.315 |
| 0.5 V in the A jack `avsrc:0.5` | stock | 0 |  | 8.197 | 0.0011 | OL |  |  |  |  |  |
| 0.5 V in the A jack `avsrc:0.5` | fwrs | -1.55273e-06 |  | 8.19661 | 0.000255177 | OL |  |  |  |  |  |

Reading the table: a voltage source alone in ohm gives OL (the ratiometric
node clips); in continuity the stock shows "0.00" (it clamps the negative
code to 0) and opendm40 the negative value: a firmware difference, not the
model; the same source in diode mode reads its own voltage (1.234 V < 3 V
full scale); in AC V the stock settles at 0 while opendm40 keeps ~18 mV of the
coupling transient on a DC input (the device-measured stock autoranges to mV
and reads 1 mV, acdc doc 6a); 0.5 V across the A jack reads 8.2 A on the
auto-selected 10 A range (no fuse in the model); a thermocouple with the die
10 C above the cold junction reads 110 C on both (they take the die sensor as
the cold junction); 1 V behind 4.7 k in ohm is OL, 0.1 V behind 4.7 k shows
12 k instead of 4.7 k.

## Open points

See RESEARCH.md 8: LPF / RMS output rate (modelled with the measured value),
the empirical AC-node and enable-order parts, the temperature sensor constants
(stock vs datasheet), the DM40 CNT pin and netlist.
