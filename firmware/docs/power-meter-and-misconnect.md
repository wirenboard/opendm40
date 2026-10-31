# Power meter and wrong-connection detection

How opendm40 measures power (V and I at the same time on the HY3131: the
function W) and how it detects wrong connections (current in the A jack while
measuring V, voltage on V/ohm while measuring current, a live circuit in ohm)
with the converter blocks each function leaves idle. Section 3 is the power
meter, section 4 the wrong-connection alerts.

Related: the user-facing behaviour is in the manual
([Power meter](../../docs/manual/README.md#power-meter),
[Safety and alerts](../../docs/manual/README.md#safety-and-alerts),
[Burden voltage](../../docs/manual/README.md#burden-voltage)); the SCPI
commands in [scpi-reference.md](../../docs/manual/scpi-reference.md)
([Power meter](../../docs/manual/scpi-reference.md#power-meter),
[Burden voltage](../../docs/manual/scpi-reference.md#burden-voltage),
[Debug commands](../../docs/manual/scpi-reference.md#debug-commands)); the
front end (relay, shunts, MOSFETs, HY3131 nodes) in
[hw/README.md DMM front end](../../hw/README.md#dmm-front-end-hy3131).

Evidence: untagged statements come from the cited source: the HY3131
datasheet ([`hycon-hy3131-datasheet-v10.pdf`](../../hw/datasheets/hycon-hy3131-datasheet-v10.pdf),
transcribed in [`emu/hy3131/ref/registers.md`](../../emu/hy3131/ref/registers.md)),
the stock rows (`stock/re/analysis/hy3131_v2_6.h` (in the private analysis repo),
byte-identical in [`src/hy3131_tables.rs`](../src/hy3131_tables.rs)), the
code, or a device check with its commit
([device-check-2026-10-21.md](../../docs/history/device-check-2026-10-21.md)).

* **INFERRED** = what the row bytes and the stock scaling imply about the
  board wiring (there is no netlist), or chip behaviour the datasheet does
  not state.
* **UNKNOWN** = needs the device or the schematic.

**Device status.** In every device session so far the A jack was not
wired. Everything that needs a current through A is **untested on the
device**: the power meter's P, PF, current channel, DC power and burden
(T2..T6, section 3.8), the A-JACK, A-PROT and OVERLOAD trips with a real
current, the fuse check's real resistance. What ran on the device is named
in each section.

## What is built

* **Power meter** (function 18, the 7th DMM button "W"): I on the stock AC A
  path (OP1 -> AD2), V on OP2 <- SENSE -> AD3, DC-coupled, P from the
  chip's X*Y RMS statistic. Couplings DC / AC / AC+DC, energy (Wh, VAh),
  per-channel autorange, burden compensation ("At load", off by default),
  a continuous-current limit, phase calibration, V gain and zero jobs, W
  limits, Bluetooth. Code: [`src/powermeter.rs`](../src/powermeter.rs),
  [`src/app_power.rs`](../src/app_power.rs),
  [`src/ui_power.rs`](../src/ui_power.rs). Sections 3.1..3.17.
* **Burden voltage readout** in the DMM A functions (3.15).
* **Wrong-connection alerts**
  ([`src/sentinel.rs`](../src/sentinel.rs), hooks in
  [`src/dmm.rs`](../src/dmm.rs)): A-JACK, V-JACK?, LIVE?, the DC? / AC? /
  FUSE? hints (4.2, 4.4, 4.6, 4.7); OVERLOAD (the uA / mA shunt over range,
  4.7); A-PROT (a current through A that would overheat the 47.5 ohm
  resistor in DC V / mV moves A to the 10 mohm shunt, 4.8); the fuse check
  (4.7); the danger alarm (loud two-tone, 4.10,
  [`src/danger.rs`](../src/danger.rs)).
* The "Leads in A?" reminder was removed on 2026-10-24 (4.9).

---------------------------------------------------------------------------

## 1. What the HY3131 offers (datasheet)

* Two fast 19-bit sigma-delta ADCs, AD2 and AD3, same clock (AD2CLK =
  SYSCLK / 2 = 2.4576 MHz), same OSR2, independent input muxes:
  `SAD2IP / SAD3IP = OP1O, OP2O, PB4, PB7`; `SAD2IN / SAD3IN = RLU, AGND,
  PB3, PB5`; gains 0.5 / 1.0 / 1.5 / 2.0 each (AD2IG, AD3IG); a common
  reference (SAD2RH/RL). Datasheet 13.2 note 8: "when AD2 and AD3 are used
  at the same time, connect AD2IN and AD3IN to AGND or to different PB<x>".
* A digital block after a high-pass filter on each channel: `RMS<37:0>` =
  `sum(X^2)/N` (SDO23 = 0, SVXI = 0), `sum(X*Y)/N` (SDO23 = 0, SVXI = 1),
  `sum(Y^2)/N` (SDO23 = 1), X = AD2, Y = AD3, N = OSR4 (LPFBW). One
  accumulator: the three statistics are **sequential**, not simultaneous.
  The LPF (DC mean) follows SDO23: LPF of AD2 (0) or of AD3 (1). RMS and LPF
  results arrive together at the LPF rate (device: every 106.7 ms with
  LPFBW = 111, [`RESEARCH.md`](../../emu/hy3131/RESEARCH.md) 3 / 8.1). The
  result is 38-bit signed: `sum(X*Y)` can be negative. No register bit
  bypasses the HPF (device probe, [hw/README.md, Measured node behaviour](../../hw/README.md#measured-node-behaviour)).
* `SPHACAL<7:0>` (R36, HY3131 only): "This function is to measure Power,
  set AD2 and AD3 output in the same phase. Set SPHACAL first, then ENAD2
  and ENAD3 = 1". Bit 7 = AD2 leads (1) / lags (0); bits 6:0 = the delay in
  AD2CLK periods: a step is 0.407 us, at most 127 steps = 51.7 us (0.93 deg
  at 50 Hz, 1.12 deg at 60 Hz).
* Two op amps OP1, OP2 with input muxes `SOP1P = SENSE, FB, RLU, AGND, PB0,
  PB1, PB2, PB8`, `SOP2P = SENSE, FB, RLU, AGND, PB0, PB1, PB2, PB3`.
  `OPS<1>` / `OPS<2>` (R25 bits 0 / 1) select the negative input: the
  external OPxN pin (the board's gain network, x10) or the output (unity
  buffer). Common-mode input range 0.1 V .. VDDA - 1.1 V.
* Datasheet 13.1 note 7: "When AD1 and AD2/AD3 is on, users must not turn
  on/off AD to avoid different AD1 Offset problem": toggling AD2 / AD3
  while AD1 converts shifts the AD1 offset. Every sentinel and power row
  below obeys it (AD1 enabled last, AD2 / AD3 never switched while AD1
  converts).
* Comparator: input `SCMPI = SENSE, FB, OP1O, PB0, PB1, RLD, PB3, PB4`,
  thresholds from the AGNDP / AGNDN taps (with REFO = 1.25 V: AGNDP<1> =
  AGND + 20.8 mV, AGNDN<1> = -20.8 mV, <2> +-41.7 mV, <3> +-62.5 mV ...),
  outputs readable in CTSTA (ACPO bit 6, CMPHO bit 5, CMPLO bit 4; the
  values the device shows: [hw/README.md, Measured node behaviour](../../hw/README.md#measured-node-behaviour)).

## 2. What the stock does with these blocks (rows)

Row bytes are R20..R35 (index 0..21). The decoded facts used below:

| row | R25 | R26 | R27 | R29 | R32 | R34 | R35 | meaning |
|---|---|---|---|---|---|---|---|---|
| f0 AC V r0 / r1..3 | 0x43 / 0x40 | 0x80 | 0x9d | 0xfc | 0x38 | 0x88 | 0x94 | AD2 = PB4 - AGND; AD3 = PB4 - AGND; ENOP1, OP1+ = SENSE; OPS1 = OPS2 = 1 in r0 (x10) else 0 (x1); RMS = sum(X*Y), LPF of AD2 |
| f2 AC mV r0 / r1 | 0x43 / 0x40 | 0x80 | 0x5d | 0xf8 | 0xaa | 0x88 | 0x54 | AD2 = OP2O - AGND; AD3 = OP2O - AGND; OP1+ = OP2+ = RLU; OPS2 = 1 in r0: **OP2 has a working x10 network on this board** |
| f4 AC A r0 / r1 | 0x43 / 0x40 | 0x80 | 0x2d | 0xf4 | 0x3e | 0x88 | 0x24 | AD2 = OP1O - PB3; AD3 = OP1O - PB3; OP1+ = PB2 (10 mohm shunt); OPS1 x10 / x1 |
| f5_6 AC mA/uA | 0x43 / 0x40 | 0x80 | 0x2d | 0xfc | 0x3d | 0x88 | 0x24 | as f4 with OP1+ = PB1 (0.5 ohm / 47.5 ohm shunt) |
| f1 DC V r0 | 0x40 | 0x00 | - | 0x00 | 0x38 | 0x00 | - | AD1 = OP1O - RLU, OP1+ = SENSE x1, tap PA7; AD2 / AD3 off |
| f1 DC V r1..3 | 0x40 | 0x00 | - | 0x00 | 0x33 | 0x00 | - | AD1 = SENSE - PB5 (R24 0x07, R23 0x83 input buffers on), both op amps off; taps PA6 / PA5 / PA4+PA5 |
| f7 / f8_9 DC A | 0x01 / 0x00 | 0x00 | - | 0x00 | 0x3e / 0x3d | 0x00 | - | AD1 = OP1O - PB3, OP1+ = PB2 / PB1 |

* **The stock uses sum(X*Y) in every AC row, with AD3 on the same node as
  AD2** (PB4, OP2O or OP1O): a cross-correlation that cancels the
  uncorrelated ADC noise, never two different quantities. R36 (SPHACAL) is
  written 0x00 at every mode change. The stock has no power function. AD2
  and AD3 on the same node differ by ~0.1 % in gain
  ([hw/README.md, Measured node behaviour](../../hw/README.md#measured-node-behaviour)).
* **Relay and shunts** ([hw/README.md DMM front end](../../hw/README.md#dmm-front-end-hy3131)):
  the V and A functions use the same PA9 / PA10 relay state, and every V
  function leaves PE0 = PE1 = 0 (the 47.5 ohm chain). So **in every V mode
  the divider and the shunt network are connected at the same time**: a
  power row energises nothing new; it only chooses which converter looks at
  which node.
* **Both stock AC paths run through OP1** (R32 = 0x38 SENSE in AC V, 0x3e /
  0x3d PB2 / PB1 in AC A), and the AC V node PB4 is downstream of OP1O
  through the coupling capacitor. OP1 can buffer the divider tap or the
  shunt, not both: one of them must move to OP2.

## 3. Power meter

### 3.1 Node assignment

The power row ("assignment B") takes the current on the stock path and the
voltage through OP2, DC-coupled:

| block | setting | source |
|---|---|---|
| I path | OP1+ = PB2 (A) or PB1 (mA / uA), OPS1 = 1 (x10, low range) / 0 (x1), **AD2 = OP1O - PB3**, AD2IG 1.0 | the stock AC A rows f4 / f5_6, byte for byte |
| V path | the divider tap on SENSE through the switch bytes R2A..R30 of a stock V row (PA7 /10, PA6 /100, PA5 /1000, PA4 + PA5 /10000: 3.2), **OP2+ = SENSE** x1, ENOP2, **AD3 = OP2O - AGND**, AD3IG 1.0 | SOP2P = SENSE exists (mux table); OP2 -> AD2 / AD3 is the stock AC mV path; device T1 (3.8): linear, with a per-range gain (3.12) |
| references | SAD2RH/RL = PB6 / AGND (R27 low nibble 0x0d, as every AC row) | |
| note 8 | AD2IN = PB3, AD3IN = AGND: different | |
| RMS / LPF | R29 = 0xf8 (ENRMS, ENLPF, LPFBW 111, no peak hold); R34 = 0x88 (SVXI) for P, 0x80 for I, 0x8c for V (SDO23 also moves the LPF to AD3) | |
| AD1 | off (R23 = 0x03) as in the stock AC rows: no note-7 exposure | |
| relay / MOSFETs | PA state 0; PE = the current range's shunt in every write | |
| comparator / counter | SCMPI = SENSE, ENCMP, ENCTR: the frequency of the voltage; the window follows the tap's DC (3.12) | |
| capacitor array | SMODE<5> + ACC as the f0 row (0xa0 / R30 0x80): the divider's AC compensation | |

The V row's power-bus bytes are required: with the stock I row's R2E / R2F
(power bus as SGND, PA8 P) the V signal drops to the noise floor (device T1,
3.8). Why B: no coupling capacitor in either path, so the only V-to-I phase
difference is the op amps' (negligible at mains); the LPFs give V_dc and
I_dc, so the same row is also a DC wattmeter; the current channel keeps the
stock AC A calibration. The alternative (V on the stock AC V path, I through
OP2) is AC-coupled on V and was not built.

### 3.2 Ranges

V and I range independently (different hardware), any pair:

| V range | tap | tap at full scale | AD3 at IG 1.0 |
|---|---|---|---|
| 6 V | PA7 (/10, the DC V r0 tap) | 0.6 V | 48 % |
| 60 V | PA6 (/100) | 0.6 V | 48 % |
| 600 V | PA5 (/1000) | 0.6 V | 48 % |
| 1000 V | PA4 + PA5 (/10000) | 0.1 V | 8 % |

Op amp common mode: AGND = 0.5 VDDA, so -1.7 .. +0.7 V around AGND. A sine
at full scale (0.85 V peak at the tap) clips on its positive half-waves:
the V calibration accepts AC only up to 70 % of the range (3.12); autorange
acts on the RMS.

| I range (`pw::I_LIM`) | shunt | OP1 gain | stock row |
|---|---|---|---|
| 600 uA / 6 mA | 47.5 ohm (PE none) | x10 / x1 | f5_6 (mode 6) |
| 60 mA / 600 mA | 0.5 ohm (PE1) | x10 / x1 | f5_6 r0 / r1 |
| 6 A / 10 A | 10 mohm (PE0) | x10 / x1 | f4 r0 / r1 |

Autorange (`pw_autorange`), per channel after its own result: up above 1.03
x the range, down below 0.1 x the range / 1.03. Entry on 600 V and 10 A
(least burden). The I floor follows the A key's AUTO / AUTO+ (AUTO+ never
enters the 47.5 ohm shunt). An over range on the uA / mA shunt in AUTO is a
one-hop up-range to 6 A (3.14). A range change rewrites the row, settles
550 ms and restarts that channel's statistic; the other channel's parts are
kept (~0.9 s per step). Manual ranges per channel (Power Settings, SCPI);
the AUTO key sets both to auto.

### 3.3 Scaling and sign

With X = AD2 = I path, Y = AD3 = V path (codes of 1.25 V / 2^18 at the
converter input, IG 1.0):

```
V_tap  = Y * 1.25 / 2^18                 Vin = V_tap * kV[r]      kV = 10, 100, 1000, 10000
V_sh   = X * 1.25 / 2^18                 I   = V_sh / (shunt[u] * opgain[r])
P_ac   = RMSxy * (1.25 / 2^18)^2 * kV[r] / (shunt[u] * opgain[r])     [W]
Vrms_ac = sqrt(RMSyy) * 1.25 / 2^18 * kV[r]
Irms_ac = sqrt(RMSxx) * 1.25 / 2^18 / (shunt[u] * opgain[r])
I_dc   = LPF(X) * (scale of I);   V_dc = LPF(Y) * (scale of V)
P      = P_ac + V_dc * I_dc        (the HPF removes DC from X and Y; the DC product is added back)
Vrms   = hypot(Vrms_ac, V_dc)      Irms = hypot(Irms_ac, I_dc)
S      = Vrms * Irms               PF = P / S (signed with P)        Q = sqrt(S^2 - P^2) (magnitude)
E     += P * dt                    dt = the time since the previous reading
```

Example: 230 V on the /1000 tap = 48.2k counts rms, 1 A on 10 mohm x1 =
2097 counts rms, PF 1 -> RMSxy = 1.01e8 -> P = 230 W; ~0.1 W per count of
the I channel there.

Calibration: the I channel takes the stock AC A RMS gain (cal record 0xC4 +
4 ci) and LPF gain (0xF4 + 4 ci); where the LPF gain is the record's
default 1.0 (never calibrated on the dumped unit), I_dc takes the RMS gain
(INFERRED: T4 compares I_dc with DC A). The V channel takes the per-range
factory-derived gain or a self-cal (3.12). The LPF zeros come from
`POW:ZERO` (3.12).

**Sign**: `hy3131::rms_signed` sign-extends from bit 37 (the stock and
`poll_ac` read 37 bits unsigned, right for sum(X^2), wrong for sum(X*Y));
`lpf19` sign-extends bit 18. Positive = current into the A jack, out of
COM: a consumer reads P > 0, a source P < 0.

### 3.4 Update rate: the three statistics are sequential

One accumulator: P = sum(X*Y), I = sum(X^2), V = sum(Y^2) need three R34
settings, one result every ~107 ms. The cycle (`CYCLE`) is **P -> V -> P ->
I**, with `DISC_DEFAULT` = 2 results discarded after each R34 switch: P
every 642 ms, a full set every 1.28 s (`POW:CYC?` = `2,642,1284`). Each
accepted part goes through the stock EMAs (V: the AC V windows, I: the AC A
windows, P: a 5 % relative jump, 0.25). The device (T3, zero current, 3.8):
YY is clean from the first result after an XY block, XX at the noise floor
needs 2 discards; the cost with a real current is untested.
`DBG:POWer:DISC` changes the count (persisted).

### 3.5 Phase, bandwidth, accuracy

* Phase sources: the op amps (2 MHz: < 0.002 deg at 60 Hz), the divider
  with its ACC compensation (set as the stock f0 row), the resistive shunt,
  the AD2 / AD3 sample alignment. `hy3131::configure_ph` writes the row
  with ENAD2 / ENAD3 cleared, then R36 (SPHACAL), then R34 (AD3) and R26
  (AD2): the datasheet order plus the AD3-first enable that avoids the
  AD2 / AD3 "beat" (sum(X*Y) above the Cauchy-Schwarz bound when AD2 starts
  first,
  `stock/re/docs/history/acdc-stock-vs-opendm40.md` (in the private analysis repo)
  10).
* SPHACAL is set by the reactive phase calibration (3.13); the resistive
  method is refused.
* Bandwidth: AD2 / AD3 at 76.8 kHz (OSR2 32), harmonics to the 20th at
  50 Hz well inside. A block is 8192 samples = 106.7 ms = 5.33 cycles at
  50 Hz: the non-integer cycle count gives a per-result ripple on P of up
  to ~+-3 % before the EMA, < 1 % after it (INFERRED).
* Expected accuracy (INFERRED): V and I as the stock AC functions
  (+-0.5 %), P +-1 % at PF >= 0.5 after a reactive phase calibration.
* Noise floor of the V channel (device T1, sqrt of YY): 47 codes on 60 V
  (22 mV rms), 50 codes on 6 V (2.4 mV); the stock AC V floor is ~31
  codes. In sum(X*Y) the two channels' noise is uncorrelated and averages
  out: the P floor is far below V x I_floor.

### 3.6 Wiring, burden and safety (COM is shared)

The meter has V/ohm, COM and one A jack, isolated from the USB / MCU ground
([hw/README.md Isolation boundary](../../hw/README.md#isolation-boundary)).
V is measured V-COM, I A-COM: **the load current flows through COM**, and
COM is at the potential of the conductor you break into.

```
 source (L / +) ---------------------+------------------- load (+)
                                     |
                                   [V/ohm]
                              DM40  [A]   -------------------- load (-)   (return conductor broken here)
                                    [COM] ---+                  fuse, MOSFETs, shunt (R_path) between A and COM
                                             |
 source (N / -) ------------------------------+
```

This is the wiring the entry dialog draws ("wiring 1"): the load current
enters A and leaves through COM (the stock's positive direction), so a
consumer reads P > 0. V-COM = V_load + I x R_path: the meter's own drop is
inside the voltage reading, P_meas = P_load + I_rms^2 x R_path. V/ohm on
the load side is impossible (the load's other terminal is the A jack).

R_path = shunt + fuse + MOSFET + traces. The burden at full scale per range
is in [hw/README.md DMM front end](../../hw/README.md#dmm-front-end-hy3131)
("Current path"); fuse, MOSFET and trace resistances are UNKNOWN. The
firmware keeps an R_path per shunt (`r_path`, default `SHUNT_OHM` = the bare
shunt, a lower bound; `POW:BURD`) and, with "At load" on, shows the load's
values (3.16): P_L = P - I^2 R, V_L^2 = V^2 - 2 R P + R^2 I^2 (exact for
any waveform: v_L = v - R i), S_L = V_L I, Q and PF from those. The burden
voltage I x R_path is shown in the side column (amber when "At load" is off
and it exceeds 1 % of V).

Safety:

* One 10 A ceramic fuse protects all shunts (DM40 manual, section 5); the
  manual gives no time limit at 10 A. The firmware has a continuous-current
  limit (default `I_CONT_DEFAULT` 6 A for `T_LIM_DEFAULT` 30 s, `POW:LIM`,
  persisted): above it a countdown in the side column, at expiry a toast and
  3 x 50 ms every 2 s at the key / alert volume while it stays above. Above
  10 A for 1 s the danger alarm sounds (4.10).
* The range MOSFETs are 30 V parts (RUH30J51M,
  [hw/README.md Main parts](../../hw/README.md#main-parts)): they see only
  the drop across the shunt path while the fuse is intact.
* The power meter is the same hardware state as plain V mode (the shunt
  network is always in circuit). The mode is explicit: no autorange leads
  into it, and the first entry shows the wiring dialog (3.12).
* The auto power-off (and its warning) is held while the energy integrates
  with I > 50 uA (`I_FLOOR`); the DMM rail stays on in every view.

### 3.7 VA-only fallback (`POW:METH ALT`)

For a unit where OP2 on SENSE would not work, `POW:METHod ALTernate`
alternates the stock AC V row and the stock AC A row of the chosen ranges,
as AC+DC does: Vrms and Irms with the stock calibration, S and VAh; no P,
no PF (no phase relation between the phases). The shunt MOSFETs follow the
current range also in the V phase (the stock AC V row would select the 47.5
ohm shunt with the load current flowing). The V phase settles 600 ms and
drops 4 results (`ALT_V_SETTLE_MS`, `ALT_V_SKIP`: the AC-coupled node). T1
showed method B works, so this is SCPI only (shown in the Power Settings
status line).

### 3.8 Device tests

| test | what | status |
|---|---|---|
| T1 OP2 on SENSE | V channel vs AC V / DC V, noise floor | **done, e606771**: GO; table below |
| T2 two nodes | resistive load: PF 1.000 +- 0.005, P = V I; reversed current: P < 0; reactive load: PF ~0, `POW:PHAS:CAL` | not run (needs current through A) |
| T3 switch cost | results per R34 switch that deviate | zero current only (e606771): YY clean, XX needs 2; with a current not run |
| T4 DC | PSU + resistor: V_dc I_dc vs DC V and DC A | not run |
| T5 ranges | every V range x the I ranges | not run |
| T6 burden | 5 V / 0.4 A on the mA range, R_path from a second meter | not run |

The test hooks: `DBG:POWer:ROW?` (the row bytes R20..R35 + R36), `:RAW?`
(the last 16 RMS / LPF results, raw), `:POKE <index>,<byte>` (up to 4
overrides, `CLEar`), `:STAT XY|XX|YY|AUTO` (freeze R34), `:DISC <n>`. Each
rewrites the row (550 ms settle).

**T1 results** (e606771; `DBG:POW:STAT YY`): GO. The V channel (tap + OP2
x1 + AD3) has a constant per-range gain error (-2 % on 6 V, -1.6 % on 60 V,
-0.3 % on 600 V) and an LPF zero on each channel, not a wrong node (the
numbers: [hw/README.md, Measured node behaviour](../../hw/README.md#measured-node-behaviour)). The gain is fixed by the factory-derived V gain (3.12),
the zeros by `POW:ZERO` (3.12). Overrides: the I row's R2E / R2F kill the
signal; AD3IN = RLU or PB5 is no better: the default row stays.

Re-checks without current (c50425f, d8e249d): `POW:ZERO` Y 1072, X ~890 /
~160; V vs DC V within 0.063 % on 6 / 60 V in DC and AC+DC after the
factory gain; f 40.0018 Hz on the square (R21 0xBF); `POW:VOLT:CAL` with a
DC reference: 1.020998 on 6 V (factory 1.0209), 1.016612 on 60 V (factory
1.0170). Open: everything in the table marked "not run", the AC reference
of the V cal (the calibrator gives at most 4.77 V AC). Status per item:
[`TODO.md`](../TODO.md) "Power-meter mode".

### 3.9 Readings, UI, SCPI

Readings: P (W, signed, the main value), Vrms, Irms, S, Q, PF (signed), f,
Wh and VAh (signed), the elapsed time, average P = E / t, V_dc / I_dc, the
burden voltage. In AC+DC the DC and AC parts of V, I and P are shown too
(3.17).

The page uses the stock DMM page slots: aux row V and I (the load's with
"At load"), the main F78 readout P (a tap cycles P -> Wh -> VAh, display
only: `READ?`, BT and the Reading's SI value stay P), the lower row E | PF
| S | Q | f, the strip chart of the main readout, the side column (burden
voltage, VAh, the energy box with RUN / PAUSE and the time, AVG, the
ranges or the limit countdown). The stock DMM faces have no W: a real W
glyph for UNIT, LETTERS and SC_DIGITS is in
[`src/stockfont_w.rs`](../src/stockfont_w.rs) (generated by
[`tools/mkwglyph.py`](../tools/mkwglyph.py) from FreeSans Bold through each
font's own V ink box; the HIST pixel font uses its M mirrored). Units: W
(mW below 1 W of full scale, kW for 1000 V x 10 A); a value that rounds to
zero shows +0 (`pw::no_neg_zero`). How it is used:
[manual, Power meter](../../docs/manual/README.md#power-meter).

SCPI: `CONFigure:POWer`, `MEASure:POWer?`, `READ?` / `FETCh?` = P,
`FETCh:POWer:ALL?`, `MEASure:POWer:<x>?` and `[SENSe:]POWer:...`:
[scpi-reference.md Power meter](../../docs/manual/scpi-reference.md#power-meter).
`CONFigure?` answers `"POW <vrange>,<irange>"`, `FUNCtion?` `"POW"`.

### 3.10 The power row (`power_row`)

`power_row(vr, k, stat)` builds the 23 bytes R20..R36 from two stock rows:
the I row (f4 / f5_6 of the current range: R25 OPS1, R27, R32 SOP1P) and
the V row's divider bytes R2A..R30 (6 V: the DC V r0 /10 tap; 60 / 600 /
1000 V: AC V r1..r3), then the overrides: R32 |= ENOP2 | SOP2P = SENSE,
R35 = 0x54 (AD3 = OP2O - AGND, IG 1.0), R23 = 0x03 (AD1 off), R20 = SCMPI
SENSE + ENCMP, R21 = 0xdd, R29 = 0xf8, R34 = the statistic. Host test
`power_row_bytes`: every byte equals the I row's, the V row's R2A..R30 or a
listed override, plus the decoded intent (OP1 on PB2 / PB1, OP2 on SENSE
x1, AD2 = OP1O - PB3, AD3 = OP2O - AGND, AD1 off). The DC coupling's row is
`dc_row` (3.12).

### 3.11 Code and emulator

[`src/powermeter.rs`](../src/powermeter.rs) is a child of dmm.rs (`Dmm::pw`;
row builder, phase machine, scaling, energy, autorange, burden, limit, jobs;
host tests); [`src/ui_power.rs`](../src/ui_power.rs) (page slots, wiring
dialog, Power Settings page); [`src/app_power.rs`](../src/app_power.rs)
(taps, settings words, SCPI); hooks in [`src/dmm.rs`](../src/dmm.rs),
[`src/hy3131.rs`](../src/hy3131.rs) (`configure_ph`, `configure_dc`,
`rms_signed`, `lpf19`), [`src/app.rs`](../src/app.rs),
[`src/scpi.rs`](../src/scpi.rs), [`src/meter.rs`](../src/meter.rs),
[`src/persist.rs`](../src/persist.rs).

* **Function code 18** (16 / 17 are the V / A AC+DC functions). Internally
  the HY mode stays the AC A mode of the current range (`hy_mode` 4..6)
  with `pw.on`, so the relay and the catch-all code paths are AC A's; any
  other function choice (a button, BT cmd 06, SCPI `CONF`) leaves. Leaving
  through the A button goes to DC A on the power meter's current range (its
  shunt kept); the others as the stock (the 47.5 ohm resting shunt; a
  current still flowing then trips A-JACK).
* Readings once P, V and I were measured; every accepted result gives one.
  Until the first full set the page shows V in the aux1 slot and "I
  ranging..." in the main area (also "Zeroing: leads open" / "V
  calibration..." during the jobs).
* The state (`Pw`, ~0.6 KB with the `DBG:POW:RAW?` ring) lives in SRAM
  (`.sram_bss`).
* **Energy**: Wh and VAh from the totals of the previous reading over the
  time since it (a range-change gap holds the last P); i64 mJ plus an f32
  remainder (host test: 1 mW s steps on top of 1 kWh). RUN / PAUSE (the
  energy box, `POW:ENER:STAT`), reset (a long press on the energy box or
  the side column asks "Reset energy?", 60 s to answer, `dialog::ENERGY_MS`;
  Power Settings "Reset Wh" and `POW:ENER:RES` reset directly). Not
  persisted: a restart in the power meter comes back in W with the energy
  at 0 (`pw_enter`;
  [decision #4](../../docs/history/decisions-2026-10-22.md#4-power-meter-energy-counter-across-power-off)).
* **Settings** (persist v10): p[48..58] SPHACAL, method, the flags ("At
  load", wiring dialog seen, coupling, zeros valid), discards, the current
  limit, R_path x 3, the 4 V gains, the zeros; p[59..62] the DC row's AD1
  zeros; p[62..64] the W limits.
* **Emulator**: the Power role in [`emu/hy3131`](../../emu/hy3131/README.md)
  (`HY3131.cs`: X = I x shunt x OP1 gain, Y = V / k, signed XY with the
  phase, SPHACAL on the cross term, mid-block R34 mixing, the LPF and the
  counter; the device's zeros with `powz:1`, the factory tap gain, the DC
  row with AD1 on the current) and
  [`emu/hy3131/powtest.py`](../../emu/hy3131/powtest.py) end to end over
  USB/IP (resistive, PF 0.5, reactive, reversed, DC, DC source, autorange,
  small currents, burden, manual ranges, the fallback, phase cal, energy,
  zero, V cal, frequency, the couplings, limits, BT, the entry and AC
  floors). The model implements this doc's assumptions, so it cannot tell
  the R34 switch cost with a current, the AD2 / AD3 gain match or the true
  phase error: that is T2..T6.

### 3.12 V gain, zeros, frequency, DC coupling, the W button

**V gain per range.** The V channel's gain error is the divider tap's (plus
OP2 / AD3), and the factory record has measured the same taps through the
stock AC V path (OP1 x1 -> AD2). `pw::v_gain_factory`:

| range | tap | default gain | dumped unit | T1 needs | residual |
|---|---|---|---|---|---|
| 6 V | PA7 /10 | DC V r0 gain (cal 0x04) x AC V r1 / DC V r1 (0x38 / 0x08: the AD2 / AD1 ratio on a tap both measure) | 1.02065 | 1.0209 | -0.03 % |
| 60 V | PA6 /100 | AC V r1 (0x38) | 1.01614 | 1.0170 | -0.08 % |
| 600 V | PA5 /1000 | AC V r2 (0x3C) | 1.00453 | 1.0033 | +0.12 % |
| 1000 V | PA4+PA5 | AC V r3 (0x40) | 0.99815 | - | not measured |

(The AC V / DC V ratio is 1.00849 on both r1 and r2 of the dump: a constant
AD2-vs-AD1 converter factor, which is why the 6 V derivation works.) The
dumped unit is assumed to be the T1 unit (INFERRED: its AC V r1 gain
reproduces the T1 ratio to 0.04 %). The gains apply as `raw x gain` to Vrms
and V_dc; a missing record or a derived gain outside 0.9..1.1 gives 1.0.

**V self-cal** (`POW:VOLT:CAL`, Power Settings "V cal"): a steady AC or DC
voltage on V-COM, the V range fixed (~8 s AC, ~3 s DC): power row (R34
frozen at YY, 1 dropped + 3 averaged) -> the stock reference row of the
same range -> power row -> reference. The kind follows the larger part of
the first round: **DC** = the stock DC V row (AD1) vs V_dc; **AC** = the
stock AC V row (2 s settle: the AC-coupled node starts from the power row's
OP1O level) vs V_ac. Gain = mean(ref / raw) of the two rounds, stored per
range. Refusals (`POW:VOLT:CAL?`): SMALL (< 10 % of the range), BIG (> 70 %
AC, > 100 % DC, or |DC| + sqrt(2) x AC RMS above the range: the peak
clips), UNSTABLE (the rounds differ > 0.3 %), RANGE (gain outside
0.9..1.1), METHOD (the fallback), ABORT (a range / function change).
`POW:VOLT:CAL DEF` returns to the factory-derived gain.

**LPF zeros** (`POW:ZERO`, Power Settings "Zero": leads open, nothing in A,
~12 s): the six current ranges in turn (R34 XX: the X LPF; 1 dropped + 3
averaged), the V channel once (YY; the zero is after the divider, the same
on every V range), and the DC row's AD1 zero per current range. Refused
(SIGNAL) when sqrt(RMS) > 400 codes or |LPF - old zero| > 4000 codes.
Stored per unit, subtracted from V_dc / I_dc only (the RMS statistics are
after the HPF). An internal zero (SOP2P = AGND) is not used: the reference
nodes differ by ~0.7 mV (the T1 POKE test: AD3IN = RLU -175 codes, PB5
-136) and the X zeros differ per range (155 / 880 codes), which would need
a row rewrite per range change.

**Frequency.** The counter runs on SENSE (DC-coupled). The rows' R21 0xDD
window is +-20.8 mV around AGND, which a unipolar signal never crosses on
both sides (T1's 0..9.6 V square read f = 0). `pw_cmp_select` (after each V
/ I result) picks the R21 thresholds nearest around the tap's DC
(`cmp_window`, the AGNDP / AGNDN and VDSC taps, at least 6 mV off the DC,
inside the swing; a bipolar signal keeps 0xDD; the T1 square gets 0xBF).
With no usable V window but >= 25 mV rms of current AC at OP1O, the counter
moves to PB4 = the current's frequency (INFERRED from the stock AC A rows'
R20 0xF2). Else f is "--" (NaN, SCPI 9.91E37). The sentinel is off in the
power meter (`sent.configured(Kind::Off)`), so R20 / R21 belong to the
counter.

**DC coupling: one fixed row** (`dc_row`):

| | AC / AC+DC (method B) | DC (`dc_row`) |
|---|---|---|
| row | the power row, R34 P V P I | the power row frozen at YY plus AD1 as the stock DC A row of the current range sets it (R22 OSR 8192 chopped, R23 ENAD1, R24 AD1 = OP1O - PB3, R28 refs PB6 / AGND, R33 OP1 chopper / pre-filter); host test `dc_row_bytes` |
| legality | - | AD2IN != AD3IN (note 8); OP1 on the shunt read by AD1 and AD2 at once; `configure_dc` writes it with all three converters off, then AD3, AD2 and **AD1 last** (note 7); nothing is written while measuring |
| relays | PA 0, PE = the I range's shunt | the same |
| V | OP2 -> AD3, RMS (AC) + LPF (DC), V gain, Y zero | the same channel: Y LPF = V_dc; Y RMS = V_ac, used for the AC? hint (3.16) |
| I | OP1 -> AD2 LPF / RMS, AC A gain, X zeros | AD1 24 bit, the `poll_dca` scaling with the factory DC A gain (0x94); offset: the row's own AD1 zero from `POW:ZERO` (the stock's factory offset was taken with AD2 / AD3 off and is used only until then) |
| P | P_ac (+ V_dc I_dc in AC+DC) | V_dc I_dc (signed) |
| cadence | full set 1.28 s | a reading per AD1 result (100 ms), V every 107 ms, simultaneous |

Expected DC accuracy (INFERRED from the stock specs and T1): I as the stock
DC A path, +-(0.15..0.2 % + 5 d); V via the AD3 LPF with a resolution of
0.48 mV / 4.8 mV / 48 mV on 6 / 60 / 600 V (the DC V function resolves 0.1 /
1 / 10 mV) and the factory gain, ~+-(0.1..0.2 % + 2 LPF codes) after
`POW:ZERO`; P ~+-0.3 % + digits.

**The W button.** The DMM row (and the combined view's band) has 7 buttons
of 64 px at a 68 px pitch from x 4; every button centres its glyph run; the
4 px gaps split 2 / 2 between the hit boxes. The W button opens the wiring
dialog until it was confirmed once (persisted), then enters directly; Power
Settings > Wiring shows it again; a long press on the A button is a
shortcut. In the power meter the V / A buttons show their plain DC glyph.

### 3.13 Bluetooth, limits in W, phase calibration

**Bluetooth: the stock phone protocol.** The protocol
([bt-protocol.md](../../docs/bt-protocol.md)) has no unit field: the phone
app derives the unit from the mode byte's group and the format byte's
prefix, and the mode byte has no free group for a 16th function. The power
meter reports the **DC V byte with P as a V-group value** (`Dmm::mode_byte`:
0x28 auto, the V channel's range field when manual): the app shows "230.02
V" for 230.02 W, i.e. the digits, decimals, sign and m / k scale of P are
right and only the letter is V. The alternatives (AC A with P: wrong unit
and magnitude; AC A with Irms or AC V with Vrms: no power at all) were
rejected. cmd 09's main value = `pw::bt_main` (P, S in the fallback; mW ->
"mV", W -> "V", kW -> W with 0 decimals; at most 65534 counts; OL ->
0xFFFF), value 2 / 3 = 0xFFFF as in a stock DC V frame; the value follows
HOLD and stays P while the readout shows Wh / VAh. cmd 06 with that same
byte keeps the power meter (`apply_mode_byte`); any other byte leaves it.
BT SCPI runs the power commands as USB does. Device 8b9cf79 (no current):
cmd 09 = 0x28, value 2 / 3 FFFF, cmd 06 0x28 keeps POW, BT `FETC:POW:ALL?`
and `CALC:LIM` work; the phone app itself is untested.

**Limits in W.** `meter::LIM_SLOTS` slot 9 "Power" (`LIM_POWER`), compared
with the Reading's SI value = P (S in the fallback), also while the readout
shows Wh / VAh; a channel over range counts as OL (HI with an upper limit).
`CALC:LIM:...` works in the power meter; the Hold & Limits page opens from
Power Settings > Limits, titled "Hold & Limits: Power", values in the unit
of P. The OK / HI / LO badge takes the slot left of the coupling glyph.
Persisted at p[62..64] (`persist::W_LIM_W`). No energy (Wh) limits: the
energy only grows, so a limit there would be a one-shot budget alarm, not a
pass / fail band.

**Phase calibration** (`POW:PHAS:CAL [REACtive]`, AC or AC+DC): on a
reactive load (C or L, PF ~0) P / S is averaged over 8 results at the
current SPHACAL and 48 steps away, then a Newton step nulls it (the slope's
sign comes from the measurement). Refusals instead of storing garbage:
|P / S| > 0.32 at the start (`CAL_PF_MAX`, sin 18.6 deg: a resistive or
mixed load) -> NOTREACTIVE, a null outside +-127 -> RANGE, no slope ->
FLAT; SPHACAL kept in each; the DC coupling and the fallback refuse.
`POW:PHAS:CAL?` = `<NONE|RUN|OK|FLAT|NOTREACTIVE|RANGE>,<SPHACAL>`.

`POW:PHAS:CAL RES` is refused (`CAL_RES_REFUSAL`): with PF = 1, P / S =
cos(phi) and the residual phase phi ~ 0 sits where the slope of cos is ~0.
One SPHACAL step (0.0073 deg at 50 Hz) moves P / S by ~1e-8 there, the
whole +-127 span by ~3e-5, while every 107 ms block carries ~1e-2 of
ripple (3.5) and V, I, P come from different blocks: a fit would need
~1e5..1e6 results. The AD2 / AD3 decimators run the same OSR, so their
group delays cancel; what SPHACAL corrects is the analog paths, which only
a measurement shows. At PF ~0 the slope is the largest (d(P/S)/dphi = 1).
The emulator runs it on a 1 kHz reactive load; a device run on a capacitor
is untested.

### 3.14 Coupling, I autorange, Power Settings

**Coupling** (`pw::next_coupling`): a tap on the active W button steps DC
-> AC -> AC+DC -> DC, as the V / A buttons do; `POW:COUP DC|AC|ACDC`
(`AUTO` is accepted for older scripts and selects ACDC).

| | DC | AC | AC+DC |
|---|---|---|---|
| row | `dc_row` (3.12) | method B (power row, R34 P V P I) | method B |
| P | V_dc I_dc | P_ac (sum X Y after the HPF) | P_ac + V_dc I_dc |
| V / I | V_dc / I_dc | Vrms / Irms of the AC parts | hypot(AC, DC) |
| S / PF / Q / f | "--" | V I, P / S, sqrt(S^2 - P^2), f | as AC, of the totals |
| Vdc / Idc (`FETC:POW:ALL?`) | measured | NaN (9.91E37) | measured |
| glyph above W / on the button | stock DC | stock AC | stock AC+DC |

AC drops the DC parts, so it does not depend on the LPF zeros. AC <-> AC+DC
switch without a row write; only DC <-> the others rewrite the row. The
burden compensation uses the parts shown. Energy integrates the P shown.
Persisted in 2 bits (`pw::coup_of`): 0 = AC+DC, 1 = DC, 3 = AC; the value 2
(a removed AUTO coupling) loads as AC+DC. Entering W from another function
starts in DC (3.16).

**I autorange without a false OVERLOAD.** The I channel enters on 10 A and
walks down, so a low current leaves it on the mA / uA shunt. In AUTO an
over range there (|I| > 1.03 x the range) is a normal up-range: **one hop to
the 6 A range** (ik 4) at the first over-range result, from any uA / mA
range (those shunts have no fuse of their own, so no intermediate step), no
latch, alert or beep; > 6.18 A then steps to 10 A. With a manual I range
the OVERLOAD rule of 4.7 applies. Latency: in method B the I channel is
judged on its XX result, at most one P V P I set (1.28 s) after the step,
then the 550 ms settle; in DC every AD1 result (100 ms). Emulator: powtest
`istep`, `istep-ua`, `iman-ovl`; senttest `ovl-pow-auto-*`, `ovl-pow`.

**Power Settings** (MF in the power meter; [`src/ui_power.rs`](../src/ui_power.rs)):
V ranges + AUTO, I ranges + AUTO, the coupling row DC / AC / AC+DC, "At
load", Zero and V cal (each through a confirm dialog, `Overlay::PowJob`),
Reset Wh, Wiring, Limits, OK; a help line (y 302) explains the selected
coupling or "At load"; the status line shows the V gain in force and the
zero state. `sf::label` lays out at most 48 glyphs: the help lines are <= 47
(ui-render guard). Help 7/7 "Power meter" summarises the page. Device
d8e249d: the coupling cycle and the Power Settings dialogs; the steps that
need current are open.

### 3.15 Burden voltage in the A functions

The DMM page shows the meter's own drop in the circuit, `I x R_path` of the
shunt in use (as the EEVblog 121GW), so the user sees the error a current
measurement adds to a low-voltage circuit.

* **One R_path setting**: the power meter's per-shunt `r_path` (3.6).
  `POW:BURDen <ohm>|DEFault` acts on the shunt of the DMM A function in use
  (DC / AC / AC+DC A, followed through the autorange), else on the power
  meter's current range's shunt. The default (the shunt alone) is a lower
  bound.
* **Slots**: DC A (A, mA, uA): aux1 (empty there in the stock; the DC mA
  loop percentage keeps aux2). AC A: aux2, instead of the duty cycle (the
  frequency stays in aux1). An F14 caption "Burden" precedes each value.
  AC+DC and the AUTO coupling hold the AC / DC parts in the aux rows: no
  readout there, but `FETC:BURD?` answers the total RMS x R_path. The power
  meter has its own burden value (3.6).
* **Value**: DC signed (current into A = positive), AC the RMS; from the raw
  current (REL and smoothing do not change it; HOLD freezes it with the
  reading). The unit / decimals follow the range's burden at the OL limit
  (fixed while the reading moves, <= 4 digits); OL shows "0L".
* **Toggle**: Settings 3/3 "Burden voltage" OFF (default, stock look) / ON,
  persisted in p[38] bit 14 (`meter::EXT_BURDEN`).
* **SCPI**: `FETCh:BURDen?` (whether the readout is on or not),
  `DISPlay:BURDen`:
  [scpi-reference.md Burden voltage](../../docs/manual/scpi-reference.md#burden-voltage).
* **Bluetooth**: not sent (the app would read it in the function's unit);
  with the readout on, AC A sends no value 3 (the duty cycle) either.
* **No guided R_path step**: with the DM40's own ohm function only the uA
  chain is reachable (the fuse check, ~48 ohm); the mA and 10 A paths need a
  known current and a second meter across A-COM (R_path = V / I), entered
  with `POW:BURD` in that A range.

Code: `Dmm::burden_val`, `Reading::burden`, `ui::aux_shown` /
`BURDEN_CAP`, the strip prefix "Bd ", `App::burden_scpi` / `burden_shunt`,
`Dmm::burden_of`. Emulator:
[`emu/hy3131/burdentest.py`](../../emu/hy3131/burdentest.py)
(`FETC:BURD?` = `READ?` x R_path within 0.2 % in every phase, both
couplings). Device 33b44bd (no current): the setting and its reboot; the
values with a current are untested.

### 3.16 "At load", the entry coupling, AC floors, coupling hints

**"At load"** (the burden compensation of 3.6) is **off by default**: V and
P are what the jacks measure (`POW:BURD:STAT?` 0). With it off, the side
column's burden voltage turns amber above 1 % of V. Persisted as bit 10 of
the power settings word (opt-in); bit 9 is still written (= !ON) for older
builds; every record without bit 10 loads OFF. While on, "@load" marks (F14,
the value colour) follow the top V's unit and sit at the main readout's top
left (x 4..39, y 66..79); the other compensated values (S, Q, PF, AVG, the
energy) carry no mark of their own, the help line says what the switch
touches. The combined-view strip and BT are not marked.

**Entry in DC.** Entering W from another function (the W button, the
wiring dialog's OK, `CONF:POW`, `MEAS:POW?`, BT cmd 06) starts in DC, as
the stock V / A functions do. A restart while in W keeps its coupling
(`App::restore` re-applies the persisted one after `select`). So set
`POW:COUP` after `CONF:POW` in scripts.

**AC floors** (`pw::FLOOR_V` = `AUTO_FLOOR_V[1]` = 0.5 % of the range,
`FLOOR_I` = 0.5 %; applied in `pw_vals`, method B only):

| coupling | V_ac or I_ac below its floor |
|---|---|
| AC | that part reads 0; P 0 (no P_ac without both); PF / S / Q "--" (NaN, SCPI 9.91E37); f "--" when the counter's channel is below its floor |
| AC+DC | the part leaves the totals (V = hypot(0, V_dc)), P_ac 0, Q 0; S, PF of the totals stay; f as AC |

`FLOOR_I` is 0.5 %, not the AUTO coupling's 2 % (`AUTO_FLOOR_A`, sized for
the uA path's ~1.5 % RMS residue): 2 % blanked a real 0.1 A on the manual
6 A range (powtest `manual`).

**Q of noise.** Uncorrelated noise on V and I adds to S but not to P: to
first order a resistive load shows Q_noise = hypot(V_n I, V I_n). With
`pw::NOISE_V` 0.1 % / `NOISE_I` 0.25 % of the range (a few x the
emulator's pure-DC residues; the device's are a check item), Q below that
reads 0 in AC and AC+DC: at 230 V / 1 A on 600 V / 6 A that is 3.5 var
(PF > ~0.9999).

**Coupling hints** (the 4.4 hint style: grey badge, silent, one toast,
assert after 2 s, clear 1 s after; `SYST:ALER?` DC / AC):

* AC+DC with no AC part (V_ac and I_ac below their floors, a DC part above
  one: `HINT_NO_AC`) -> DC? "No AC: DC is more precise".
* DC with an AC part on V (`HINT_AC`) -> AC? "AC present: use AC+DC". The
  DC row is the power row frozen at YY, so its RMS (V_ac) is read on every
  result without a register write (no note-7 exposure). I_ac would need R34
  at XX while AD1 converts: not done. V_ac is the right condition anyway:
  the AC power DC misses is mean(v_ac i_ac). A ripple on V with a pure DC
  current still raises the hint.
* A row write resets the hint timers. The toasts are `pw::HINT_TOASTS`; the
  badges and `SYST:ALER?` names are the DC V / AC V hints'.

Device 92fb8fa (no current): "At load" off after the update, the toggle and
marks, entry in DC, the AC floors on 1 V DC, the DC? and AC? hints. The
steps with a current (V at 1 / 2 A, the residues against the floors, mains
PF) are open.

### 3.17 AC+DC: the DC and AC parts on the page

In **AC+DC coupling only**, F14 sub-lines show the parts: "DC x V AC y V"
under V, "DC x A AC y A" under I (y 61, right-aligned to the V unit's end
201 / to 423), "DC x W + AC y W" under P (y 155, to 394, only while the main
readout shows P). "DC" / "AC" / "+" dim, the values in the status colour,
each in the readout format of its range.

**Values** (`PowerVals::vp` / `ip` / `pp`, [DC, AC], `pw_vals`): V_dc / I_dc
the LPFs, V_ac / I_ac the RMS statistics, P_dc = V_dc I_dc, P_ac the X*Y
statistic; none derived from the totals. The AC floors apply. "At load"
applies per part (v_L = v - R i): V_dc - R I_dc, sqrt(V_ac^2 - 2 R P_ac +
R^2 I_ac^2), P_dc - R I_dc^2, P_ac - R I_ac^2; so P = P_dc + P_ac and V^2 =
V_dc^2 + V_ac^2 hold for what is shown. I is never compensated.
`FETC:POW:ALL?` Vdc / Idc stay the measured DC parts.

**Geometry** (`ui::pw_parts`, set with the static draw of the DMM page in
AC+DC with method B; `App::pw_tick` redraws the page when it changes; false
everywhere else, so no other page moves):

| item | DC / AC | AC+DC parts |
|---|---|---|
| main F78 cells, unit, coupling glyph, limit badge, "@load" mark | y 68 / 114 / 76 / 72 / 66 | +7 (`PARTS_DY`) |
| lower row (E, PF, S, Q, f) | 153..171 | 170..188 (`PARTS_ROW_DY` 17) |
| chart frame | 176, 97 rows | 192, 81 rows |
| side column | burden 176, VAh 188, energy box 205, AVG 245, ranges 259 | burden 192, no VAh (the main value tap still shows VAh), box 208, AVG 249, ranges 263 |
| taps | value 64..150, row 153..171, box 205..242 | value 64..167, row 170..188, box 208..245, the chart column from 192 |

The aux row's boxes (y 36..65) overlap the sub-lines' slots (61..74): a
repaint of an aux box invalidates its sub-line (`PwCache::sub_invalidate`).
The HV lightning's box can overlap the moved "@load" mark: its toggle
invalidates the mark. Code: `ui::set_pw_parts` / `main_dy` / `chart_geom`
/ `chart_rows`, `ui::pw::sublines` / `side_geom` / `row_hit`,
`App::pw_parts_want`. Device 2f56ff9 / 92fb8fa (no current): the layout,
the Wh / VAh tap, back to DC with no leftover pixels; a coupling switch
keeps the old layout for up to ~4 s (until the first reading of the new
coupling).

---------------------------------------------------------------------------

## 4. Wrong-connection detection

The A-JACK, A-PROT and OVERLOAD trips with a real current through A are
untested on the device (the A jack was never wired); the device checks so
far ran with the A jack open and the calibrator on V-COM, i.e. they tested
the absence of false alerts, V-JACK? and LIVE?.

### 4.1 The resting state is the key fact

In every non-current function PE0 = PE1 = 0: the A jack rests on the
~48 ohm chain (10 mohm + 0.5 ohm + 47 ohm) to COM
([hw/README.md DMM front end](../../hw/README.md#dmm-front-end-hy3131),
"Current path"; a user confirms the idle input "rests on the 47 ohm mA
shunt", [#59918](https://t.me/S2084K/59918)). Consequences:

* A voltage source across A-COM while in V / ohm / ... draws V / 48 ohm:
  5 V -> 0.1 A (0.5 W in the 47 ohm resistor), 12 V -> 0.25 A (3 W), 24 V
  -> 0.5 A (12 W), mains -> 4.8 A until the fuse opens. The firmware sees
  every one of these currents at once on the shunt node PB1 (47.5 ohm x I:
  1 mA = 47.5 mV); above ~125 mA it moves A to the 10 mohm shunt (A-PROT,
  4.8).
* A current flowing in the A jack while the user thinks he is in V (leads
  in A / COM, function V) is the same signal: current in PB1 while the
  function is not a current one (the Fluke "Input Alert" case). The DM40
  has no jack sensing, so the shunt node is the only way.

### 4.2 Sentinel per function

Each function watches PB1 (or, in the current functions, SENSE) with a block
its stock row leaves idle (`sentinel_row`; host tests: the byte diff against
the stock row is exactly the intended fields). The comparator is preferred
where it is idle: it needs no converter, so no note-7 exposure.

| function (mode) | sentinel (`Kind`) | row override | threshold |
|---|---|---|---|
| DC V 6 V (1 r0), DC mV (3), ohm (10), temperature (15, TC and resistive sensors) | A-JACK: comparator on PB1 (`CmpA`); CTSTA on every poll, ORed into 100 ms windows | R20 = 0x90 (SCMPI PB1, ENCMP) | the rows' R21 0xdd window: +-20.8 mV = 0.44 mA on 47.5 ohm |
| DC V 60 / 600 / 1000 V (1 r1..3) | none (the PB1 comparator trips in these rows: A-JACK at 10 V with the A jack open, device f80524f; CTSTA 0x50 at every R21 window also with V-COM open, while AD1 reads PB1 at ~2 mV, 2026-10-27; [hw/README.md, Measured node behaviour](../../hw/README.md#measured-node-behaviour)) | stock row | - |
| AC V (0), AC mV (2) | A-JACK: AD1 peak hold on PB1 - PB3 (`PeakA`); AD1 off in the stock rows | R22 0x00 (OSR 256 unchopped), R23 \|= ENAD1, R24 0x95, R25 AD1IG 3.6, R29 ENPKH + PKHSEL 01, R33 OP1CHOP 00 (with AD1 on, OP1's chopper puts a ~37 mV RMS floor on AC V 6 V: [hw/README.md, Measured node behaviour](../../hw/README.md#measured-node-behaviour)) | `PK_THR` 17 900 codes = 0.5 mA |
| continuity (11), diode (12), capacitance r0..4 (13, RC oscillator) | A-JACK: AD2 peak hold on OP2 x1 <- PB1, AD2 = OP2O - PB3 (`Ad2A`) | R25 AD2IG 2.0 + OPS2 0, R26 0x83, R27 0x6D, R29 ENPKH, R32 0xD0 \| OP1; the row goes out with AD1 off and ENAD1 last (note 7) | `PK2_THR` 9 900 codes = 0.5 mA |
| DC A / mA / uA (7..9) | V-JACK?: comparator on SENSE via the /1000 tap (`CmpV`; SS5 only: the A rows run the power bus as SGND, PS5 would ground the tap) | R20 0x10, R2C \|= 0x10 | ~21 V on V/ohm |
| AC A / mA / uA (4..6) | V-JACK?: AD1 peak hold on SENSE - AGND via the /1000 tap (`PeakV`) | R22 00, R23 \|= ENAD1, R24 0x03, R25 \|= 0x30, R29 PKH AD1, R2C \|= 0x10, R33 OP1CHOP 00 | the swing rule of 4.11 / 4.12: 2 V rms AC on V |
| AC+DC V / A | the DC and AC phases' own sentinels; the DC phase's R20 goes through `acdc_src` because `freq_secondary` rewrites R20 every 350 ms (AC+DC shows no frequency) | as the plain functions | as the plain functions; V-JACK?'s "no current" = < 5 % of the range |
| ohm, continuity, diode | LIVE?: no hardware (4.6) | none | - |
| frequency (14) | none (below) | stock row | - |
| capacitance r5..7 (ramp) | none (below) | stock row | - |
| power meter | none (R20 / R21 are the counter's, 3.12) | - | - |

Gaps (no fake coverage):

* **Frequency**: PB1 - PB3 follows the input from ~3 V on (-59 / -89 mV at
  3 / 3.7 V, 2.5 mV at rest; [hw/README.md, Measured node behaviour](../../hw/README.md#measured-node-behaviour)).
  The standing ~54 mV of device d8e249d came with the row's SAD1I = 11 (AD1
  on the temperature sensor inputs, *inferred*): an AD1 sentinel there needs
  R33 SAD1I = 00.
* **Capacitance r5..7** (the ramp): both op amps on PB0, AD1 and the
  comparator busy.
* **DC V 60 V+** (above) and so AC+DC V above 6 V in both phases (an
  AC-phase-only A-JACK would be cleared and re-asserted every phase switch).
* **Temperature**: `poll` passes its settle deadline only ~1.4 times a
  second (the sensor phases), so the comparator is sampled that rarely: an
  AC current trips and clears more slowly.

Every sentinel takes no sample for 150 ms after a row write and 100 ms
after its settle, and the peak hold is cleared at the settle's end
(`BLANK_ROW_MS`, `BLANK_SETTLED_MS`). In the AC rows ENPKH is toggled after
each harvest to restart the hold (`PKH_RESTART`).

### 4.3 What cannot be detected (and what helps instead)

* **A voltage source across A-COM while in a current function** (the
  fuse-blower): on 10 A the path is tens of mohm; a supply current-limits
  and reads as a plausible current, mains blows the fuse in < 1 ms. No
  signal tells it apart; protection is the fuse's business.
* **A blown fuse is invisible**: the A side of the fuse has no path into the
  chip, so the meter reads 0 A forever. Mitigations: the **fuse check** and
  the **FUSE?** hint after 60 s of zero in an A function (4.7).
* **The uA / mA shunt over-current** (47.5 ohm or 0.5 ohm under amps): the
  **OVERLOAD** rule moves the reading to the 10 A shunt (manual range) or
  hops quietly to 6 A (AUTO) (4.7).
* **A source across A-COM in a V function** that would cook the 47.5 ohm
  resistor: **A-PROT** moves A to the 10 mohm shunt where the probe can
  measure it (DC V / mV, 4.8); elsewhere A-JACK warns.
* **A live circuit in ohm / diode / continuity**: the ohm node sits at the
  source's open-circuit voltage, so a positive external voltage cannot be
  told from "open" by level; a negative one can (LIVE?, 4.6).
* **The stock warns about nothing here**: only the V-mode colour (red from
  30 V), the HV glyph (from 100 V) and "Low battery"; opendm40 keeps those.

### 4.4 Alerts

One alert at a time, by priority, computed per poll in `dmm.rs` and carried
in `Reading.alert` (`sentinel::Alert`):

| alert | where | condition | assert / clear | UI and sound |
|---|---|---|---|---|
| **OVERLOAD** (red, not maskable) | DC / AC / AC+DC A with a manual mA / uA range; the power meter with a manual I range on the mA / uA shunt | the reading over its range (> 1.03 x) | at the first reading after the settle; released by a user choice or 5 s back in range (4.7) | badge OVLD red for 3 s, then amber "10A" while latched; toast "Overload: 10 A shunt selected"; one 300 ms beep at the key / alert volume |
| **A-PROT** (red, not maskable) | DC V, DC mV | the probe reads >= 300 mA at once or >= 125 mA for 400 ms (4.8) | latched until the user acts and the current is gone | badge A-PROT, toast "A jack current: 10 A shunt on", the danger alarm while the current flows (4.10) |
| **A-JACK** (red) | the non-current functions of 4.2 | 2 hit windows in a row (`AJACK_WINDOWS`) | clear 1 s after the last hit | badge A-JACK, toast "Current in A jack: check leads", the danger alarm where V-COM cannot fake it (4.10) |
| **V-JACK?** (amber) | DC / AC / AC+DC A | voltage on V/ohm while \|I\| < 0.5 % of the range (DC A; 5 % in AC A and AC+DC) | 1 s / 0.5 s | badge, toast "Voltage on V jack, no current"; silent |
| **LIVE?** (amber) | ohm, continuity, diode | a result only an external source gives (4.6) | 0.3 s / 1 s | badge, toast "Voltage on circuit - power off"; one 100 ms beep |
| **DC?** (grey hint) | AC V; the power meter in AC+DC | a DC voltage on V/ohm and (almost) no AC (4.7, 3.16) | 2 s / 1 s | badge, one toast; silent |
| **AC?** (grey hint) | DC V 6 V; the power meter in DC | the V tap swings both ways (4.7, 3.16) | 2 s / 1 s | badge, one toast; silent |
| **FUSE?** (grey hint) | DC / AC / AC+DC A | 60 s at zero (4.7) | clears at the first reading above | badge, one toast; silent |

The red alerts alternate with HOLD in the DMM status slot (119..201) and in
the combined-view strip badge (decision #5 of
[decisions-2026-10-22.md](../../docs/history/decisions-2026-10-22.md));
the hints sit below every alert and never alternate with HOLD. Settings 3/3
"Jack alerts": ON (all) / A-JACK only (no V-JACK?, LIVE?, hints) / OFF,
persisted in p[38] bits 11..13; a disabled sentinel writes the stock row.
SCPI: `SYSTem:ALERt?` (NONE | OVERLOAD | APROT | AJACK | VJACK | LIVE | DC |
AC | FUSE), `SYSTem:ALERt:ENABle`, `STATus:QUEStionable` (bit 9 while a red
alert is on, bit 1 for OVERLOAD), `DBG:SENTinel?`:
[scpi-reference.md](../../docs/manual/scpi-reference.md). What the user
sees: [manual, Safety and alerts](../../docs/manual/README.md#safety-and-alerts).

### 4.5 Implementation plan

Superseded by what was built: 4.2, 4.6..4.8.

### 4.6 A-JACK, V-JACK?, LIVE?

Code: [`src/sentinel.rs`](../src/sentinel.rs) (row builder, alert state
machine, host tests), hooks in [`src/dmm.rs`](../src/dmm.rs)
(`write_config`, `poll`, `poll_ac`, ohm / continuity / diode),
[`src/app.rs`](../src/app.rs) / [`src/ui.rs`](../src/ui.rs) (badge, toast,
beep, Settings 3/3), [`src/scpi.rs`](../src/scpi.rs).

* **A-JACK, comparator**: the CTSTA read (1 byte) on every `poll`, ORed into
  100 ms windows (`WIN_MS`); 2 hit windows in a row assert, 1 s without a
  hit clears. In DC V 6 V the comparator alternates between PB1 and SENSE
  for the AC? hint, and in AC V AD1 alternates for the DC? hint (4.7), so
  A-JACK sees every other window there and asserts on two PB1 windows in a
  row (~0.3 s).
* **A-JACK, AD1 / AD2 peak**: PKHMIN / PKHMAX come with `poll_ac`'s register
  read (AD1) or the 100 ms harvest (AD2); a hit is |peak| above the
  threshold of 4.2.
* **A-JACK in ohm / temperature (`V_GATE`)**: with the A jack open, 5 / 10
  V DC or the 0..9.6 V square on V-COM raised A-JACK in ohm and temperature
  (devices 22c85ba, c2ddd2a; 1 V did not): PB1 picks up the V input in
  those rows, and no threshold separates that from a current. These rows
  measure the V jack themselves, so A-JACK asserts only when a V-side
  result since the hit run began said V-COM is free: ohm: no negative AD1
  code (a negative source gives the negative code LIVE? uses; a positive one
  above the open-circuit voltage reads OL on the device, 519fb8a 7b, so ohm
  also needs the jack check of 4.13); thermocouple: |V| <=
  80 mV (`T_VDRV_MV`; type E is 76.4 mV at 1000 C; an open TC read ~7.6 mV
  on the device); the resistive sensors use the ohm sign. An asserted
  A-JACK drops when the V side turns driven. `DBG:SENT?` field 12: -1 not
  gated / no result yet, 0 free, 1 driven. Residual gap: a current in A
  together with a source on V-COM is not reported in ohm / temperature
  (LIVE? is). The ohm-network rows' A-JACK does not sound the danger alarm
  (4.10).
* **V-JACK? in DC A**: hit windows for 1 s while |I| < 0.5 % of the range;
  clears 0.5 s after the last hit or when current flows. Entering DC A after
  a row that leaves SENSE "blind" runs the temperature row first (4.12).
  After leaving ohm or a chip restart the comparator reads 0x70, then 0x50,
  for seconds (device 469f354, [hw/README.md, Measured node behaviour](../../hw/README.md#measured-node-behaviour)): within 7 s of the exit a both-bit read is no
  hit, nor CMPLO alone within 2.5 s of the exit / the last 0x70 read
  (`OHM_EXIT_*`); CMPHO alone counts at once.
* **V-JACK? in AC A**: 4.11 / 4.12.
* **LIVE?** (ohm, continuity, diode; no hardware): an ohm AD1 code below
  -20 000 (`LIVE_CODE`; the formula would otherwise drop the sign), a
  continuity result below -2 ohm before the clamp, a diode voltage below
  -0.05 V. A positive source in ohm never shows LIVE? (by design). Device
  c50425f: LIVE? / HOLD alternate ~1 s in continuity with 1 V.
* `DBG:SENTinel?` (kind, alert, mask, ctsta, pkhmin, pkhmax, streak,
  hit_windows, samples, probe_mA, aprot_ms, vside, soft);
  `DBG:SENTinel:LOG?` (the last 8 alerts); `DBG:SENTinel:VARiant <bits>`
  restores earlier rows for a device comparison (1 the AC V row's OP1 chopper, 2
  no AC V sentinel, 4 the DC V 60 V+ comparator, 8 the stock rows for every
  sentinel added after f80524f, 16 no hint alternations, 32 the AC A
  SENSE - PB5 row, 64 AC A AD1 at IG 0.9).

Device results (A jack open): T7 (the readings with Jack alerts ON vs OFF:
DC V, DC mV, ohm, AC mV, Hz equal within noise; f80524f), AC V ON vs OFF
within 0.04 mV after the chopper fix, DC V 1 <-> 10 V soak 5 min without a
hit (c50425f); no A-PROT / false A-JACK over 168 function switches with
V-COM open (device-check "E6"). Emulator: the HY3131 model's sentinel nodes
and [`emu/hy3131/senttest.py`](../../emu/hy3131/senttest.py) (A-JACK,
V-JACK?, LIVE?, OVERLOAD, hints, fuse check, A-PROT scenarios).

### 4.7 OVERLOAD, hints, fuse check

One 10 A ceramic fuse sits in front of all shunts (section 5): the 0.5 ohm
mA shunt and the 47.5 ohm uA chain have no protection of their own. Code:
`sentinel::overload_trips`, the latch in `dmm.rs` / `powermeter.rs`.

**OVERLOAD** (red, not maskable):

| | |
|---|---|
| where | DC A / AC A / AC+DC A (and the AUTO coupling) on the mA or uA shunt with a **manual** range; the power meter's I channel on ik 0..3 with a manual I range. Never in V / ohm / continuity / diode / cap / Hz / temperature (host test `overload_rule`) |
| AUTO | the shunt's top range (6000 uA / 600 mA) over range is a quiet hop to the 6 A range (DC: mode 7 r0, 250 ms; AC: mode 4 r0, 650 ms; AC+DC / AUTO coupling: its DC phase), no latch, beep, toast or badge; > 6.18 A then steps to 10 A |
| trigger | a reading over its range (\|I\| > 1.03 x; AC+DC: the total) on a manual range |
| action | the 10 A range at once (DC: mode 7 r1, 250 ms settle; AC: mode 4 r1, 650 ms; AC+DC: its DC phase; power: ik 5, the V parts kept); `relay_pe(0)` through `write_config` |
| release | any user range / function / AUTO choice, or \|I\| below max(0.85 x the full scale it returns to, 2 mA) for 5 s (`OVL_RELEASE_FRAC`, `OVL_ZERO_A`, `OVL_RELEASE_MS`; OL readings never count): back to the manual range it left. A re-trip within 30 s (`OVL_RETRIP_MS`) makes the latch sticky (user choice only) |
| timing and UI | detection at the first conversion after the settle (DC ~0.35 s, AC ~0.75 s after the overload): enough for a thermal protection. One 300 ms beep, toast "Overload: 10 A shunt selected", badge OVLD red for `OVL_RED_MS` 3 s (TEXT22: "OVERLOAD" does not fit the 83 px slot), then an amber "10A" badge (`Alert::Ovl10A`, display only) while the latch holds |
| AC A after a range change | over-range results are held back for 500 ms after the first result of a row (`AC_OVER_HOLD_MS`, 4.11) |
| SCPI | `SYST:ALER?` OVERLOAD and `STAT:QUES:COND?` 514 (bit 9 + bit 1) for the whole latch |

**Hints** (grey, silent, one toast; on with Jack alerts ON):

* **DC?** (AC V, not AC+DC): AD1 alternates per 100 ms window between PB1 -
  PB3 (A-JACK) and SENSE - PB5 (the range's divider tap: AC V r0 / r1 use
  the /100 tap, INFERRED from R2C / R2D; r2 /1000, r3 /10000), IG 3.6. The
  V-tap window's (PKHMIN + PKHMAX) / 2 is the DC; hint when |DC| > max(0.5
  V, 2 % of the range) and the AC reading < 10 % of it (`DC_HINT_V`,
  `DC_HINT_AC`). ENPKH goes off with the input switch and back on 3 ms
  later (`pkh_due`), so the one conversion of mixed inputs never reaches the
  hold.
* **AC?** (DC V 6 V only: the other ranges have no sentinel): the
  comparator alternates per window between PB1 and SENSE (V / 10); both
  CMPHO and CMPLO in one SENSE window = the input swings both ways by >
  0.21 V around zero. The first CTSTA after each switch is dropped; a
  window counts only while the DC V reading is below 2 V (`AC_HINT_DC_MAX`)
  and after the row's first reading; the V-tap window never counts a
  both-bit read.
* DC? and AC? need their condition in every window for the 2 s
  (`HINT_ASSERT_MS`); 1 s to clear.
* **FUSE?** (DC / AC / AC+DC A, not the power meter): 60 s (`FUSE_HINT_MS`)
  with |I| below 0.1 % of the range (DC, `FUSE_FRAC`) or 5 % (AC, AC+DC:
  their RMS floor, `AC_FLOOR_FRAC`), not OL. It is a question: with the
  fuse intact and nothing connected the meter reads the same zero.

**Fuse check** (Current Settings "Fuse check", `SYST:FUSE:TEST`,
`SYST:FUSE?`): a dialog "Lead from V/ohm jack to A, nothing else
connected" -> Start -> ohm 600 ohm, manual, the stock row with every
sentinel off (the check drives the ohm current through the A shunt on
purpose), PE = 2 as every ohm row: V/ohm -> lead -> A -> fuse -> 10 mohm +
0.5 + 47 ohm -> COM. The first final reading of the 600 ohm row whose ohm
vet saw two conversions agree (dmm.rs `OHM_VET_*`) decides: 44..54 ohm FUSE
OK (`FUSE_OHM`), OL FUSE OPEN ("Replace: 10 A ceramic fuse"), anything else
"Check leads" (a short V-COM, the lead in COM); no reading in 8 s: "No ohm
reading". Then the function before comes back (a manual range too; from
the power meter the energy restarts); Cancel / a key aborts and restores;
"Again" repeats; a function choice from SCPI / BT ends it without a
restore. The emulator reads 47.50 ohm; the real resting resistance on the
device is untested.

**STATus:QUEStionable**: `:CONDition?` bit 9 while a red alert (A-JACK,
A-PROT, OVERLOAD) is on, bit 1 while OVERLOAD (SCPI-99 CURRent; in the
34401A bit 9 means "ohms overload", so hosts using `SYST:IDN:EMUL 34401A`
may read it so); `[:EVENt]?` latches since the last read / `*CLS`. No
enable / transition registers; `*STB?` stays 0.

Device: the OVERLOAD, fuse check and FUSE? steps need the A jack wired and
are untested; DC? / AC? ran on c50425f and later (no false AC? after
switches since the 2026-10-23 rules).

### 4.8 A-jack protection (A-PROT)

The resting state of 4.1 stays in every non-current function (~48 ohm
A-COM: the sensitive A-JACK from ~0.44 mA), but when a current through A
would overheat the 47 ohm resistor, the A jack moves to the 10 mohm shunt
(PE0 = 1): a weak source then flows harmlessly, a stiff one blows the 10 A
fuse instead of burning the resistor.

**What it costs the resistor** (a MELF 0207, 0.4..1 W depending on the
series, INFERRED from the photo in
[hw/README.md DMM front end](../../hw/README.md#dmm-front-end-hy3131);
assume 1 W, derate to 0.5 W): I^2 x 47.5 ohm. 5 V across A-COM = 104 mA =
0.52 W, 12 V = 250 mA = 3 W, 24 V = 0.5 A = 12 W.

**The probe** (`sentinel::probe_row`, `probe_amps`). PB1 is far outside any
converter range at these currents (100 mA = 4.75 V). PB2, the top of the
10 mohm element, which every A-jack current passes in every PE state
(INFERRED from the shunt chain), sits at I x 10 mohm (0.25 A = 2.5 mV). OP2
(x1, unused in DC V / DC mV) on PB2, AD2 = OP2O - PB3, IG 2.0, OSR 256, the
AD2 peak hold read every 100 ms: `PROBE_CODES_PER_A` 4 194 codes per A, the
larger of |PKHMAX| / |PKHMIN| (DC of either sign; AC: the peak, >= the RMS).
The probe goes into the row at the first comparator hit window (one
rewrite, AD1 last: note 7; the DC V reading skips ~120 ms) and leaves when
the run of hits ended and nothing is latched. After every row write with
the probe in, the peak hold is cleared 30 ms later (`PROBE_SETTLE_MS`) and
the first window counts from there: AD2 restarts with the row and its first
results are not the input (device d8e249d: a false A-PROT on the first
window). Without an A-jack current AD2 is never on: the stock DC V
calibration conditions stay untouched.

**Only DC V and DC mV.** In ohm and temperature the probe reads the V input
(~0.34 A per volt on V-COM with the A jack open: 5 V read 1.7 A; device
22c85ba), so those keep the comparator's A-JACK with its V gate (4.6).

**The rule** (`sentinel::aprot_trips`), only while the comparator run is
still going: >= 300 mA (`APROT_NOW_A`, 4.3 W) at the first probe window
(~130 ms after the first hit window), or >= 125 mA (`APROT_HOT_A`) for
400 ms (`APROT_HOT_MS`) from the first hit window (~0.5 s from the onset).
125 mA, not 100 mA (0.5 W): 5 V across A-COM (104 mA, 0.52 W) stays a
warning (A-JACK only), and 125 mA = 0.74 W is still below the assumed 1 W
rating; 12 V (250 mA) is well above it.

**Action and latch** (`Dmm::aprot_poll`): PE0 = 1 at once (a GPIO write, no
row rewrite), red A-PROT (above A-JACK, below OVERLOAD, not maskable).
While latched every function keeps PE0 = 1 (`write_config`) and DC V (all
ranges, also 60 V+) / DC mV keep the probe in the row, now measuring on the
10 mohm path. The latch ends when **both** the user acted (any function /
range / AUTO choice) and the probe read < 50 mA for 1 s
(`APROT_GONE_A`, `APROT_GONE_MS`), in either order; then the row is written
again with the resting PE state. A current function or the power meter ends
it at once (their own shunt selection and OVERLOAD take over). In a
function without the probe the latch holds until the user goes back to one
with it or to a current function.

**Limits.**
* **> 30 V on A-COM** (mains, a battery pack): the MOSFETs are 30 V parts,
  so they may break down before or while PE0 switches. The fuse is the only
  protection there.
* **Jack alerts OFF** (or A-JACK masked) turns A-PROT off too: the
  comparator hit is the probe's trigger.
* **AC+DC** (its phases rewrite the row every cycle), **AC V / mV,
  frequency, continuity, diode, capacitance, ohm, temperature**: no probe,
  A-JACK only.
* The probe's numbers are INFERRED (the PB2 node, the OP2 x1 path); the
  model's peak-hold noise is ~30 mA.

Emulator ([`emu/hy3131/senttest.py`](../../emu/hy3131/senttest.py)
`aprot-*`; A-PROT latency = `DBG:SENT?` field 11, ms from the first hit
window): 5 V across A-COM in DC V A-JACK only; 12 V A-PROT after 409 ms; 24 V
after 154 ms; an AD2 start-up glitch of 0.72 A at 1 V: A-JACK only; ohm /
temperature at 12 V: A-JACK only; the user acting before the source is gone:
latched until 1 s after it went; `CONF:CURR:DC` ends the latch. Device: no
false A-PROT with the A jack open since the d8e249d fixes; the trips with a
real source are untested (never above 30 V across A-COM).

### 4.9 "Leads in A?" reminder (removed 2026-10-24)

A toast on every change from a current function to another, removed because
it looked like a false "Current in A jack" alert; `SYST:ALER:ENAB` bit 3 and
p[38] bit 15 are ignored; A-JACK catches a lead left in A once a current
flows.

### 4.10 Danger alarm: loud, whatever the volume

A wrong connection that is actually dangerous beeps at maximum volume,
overriding the key / alert volume. Code:
[`src/danger.rs`](../src/danger.rs) (pattern, event rules, the 10 A rule;
host tests), `App::alert` / `App::alert_beep` / `App::alarm_hit`
([`src/app.rs`](../src/app.rs)). The alarm reads the alert of each reading
and `Sentinel::probe_a`. The stock has no misconnect alarm and no mute key;
its loudest beep is 49.5 % duty at volume 100 % on the TIM14 buzzer
([hw/README.md Power key, PA2, buzzer](../../hw/README.md#power-key-pa2-buzzer)).

| alert / cue | alarm? | why |
|---|---|---|
| **A-PROT** (4.8) | **yes**, while its current flows (`probe_a` >= `APROT_GONE_A` 50 mA) | a source across A-COM pushed >= 125 mA: a stiff one now runs into the 10 A fuse, and above 30 V the MOSFETs are at risk. The latch without current waits for the user: badge, no alarm. In a function without the probe the last reading stands: removing the source first, then changing function, stays quiet |
| **A-JACK** (4.6) where V-COM cannot fake it (`Sentinel::ajack_alarm()`: DC V 6 V, DC mV, AC V / mV, the thermocouple with its V gate, AC+DC V) | **yes** | the meter is a ~48 ohm path in series with whatever the probes touch; outside DC V / mV there is no probe, so A-JACK is the only warning before a 12-24 V source cooks the 47 ohm resistor |
| A-JACK in the ohm-network rows (ohm, continuity, diode, capacitance, the resistive temperature sensors; `Sentinel::soft`, `DBG:SENT?` last field 1) | no (badge, toast) | its evidence there is PB1, which follows a voltage on V-COM in those rows (false A-JACK at 5 V in ohm and continuity on the device), so the badge stays silent there. A voltage on V-COM gives LIVE? there since the jack check (4.13); a real current in A while in ohm shows the A-JACK badge without the alarm; a switch to DC V sounds it within ~1 s |
| **current above the 10 A rating** (no badge of its own) | **yes** | the DMM's A functions reading > 10 A or OL on the 10 A range itself (AC+DC: the total), the power meter's I channel on 10 A reading > 10 A or OL, for >= 1 s (`I10_ASSERT_MS`: an inrush is the fuse's normal life). OL on a lower range says nothing about the amps |
| OVERLOAD / OVLD, amber 10A | no | the meter protected itself and measures fine: one 300 ms beep at the key / alert volume |
| power meter current limit expired | no | a user-set budget (default 6 A for 30 s): 3 x 50 ms every 2 s at the key / alert volume |
| V-JACK?, LIVE?, DC? / AC? / FUSE?, the fuse check | no | advisories, or a guided test |

**Pattern** (`danger::tone_at`): a burst of four 125 ms tones (`STEP_MS`),
high / low / high / low (3.57 kHz = the stock ARR 11200, and 2.86 kHz =
`BUZZER_ARR_LO` 14000), then 500 ms quiet: one burst per second. Duty 49.5
% of each tone's period (CCR1 5544 / 6930) = the stock's 100 % volume. ARR
and CCR1 are preloaded, so a tone change lands whole at the next 0.3 ms
period.

**Event rules** (`danger::Alarm`):

* an event starts when the condition turns on and sounds at once;
* **silence** for this event: a tap on the alert badge (the DMM status slot
  119..202 x 1..33, the combined view's strip badge; that tap does nothing
  else), a tap on the alert's toast, or any key (MENU / RUN / AUTO / MF,
  which still do their job; the power key too). This works also with the
  touch lock on, a dimmed screen or a screenshot running;
* **re-arm**: the condition gone and back is a new event;
* **stop**: the pass after the condition clears, and when the DMM stops
  measuring (the scope / generator views switch its rail off);
* **cap**: the full pattern for 30 s (`FULL_MS`), then one burst every 10 s
  (`REMIND_MS`) until the event ends or is silenced;
* while it sounds the alarm owns the buzzer: it re-asserts its tone every
  main-loop pass (written only when TIM14 differs), so a key beep in between
  cannot mute it; at its end CCR1 = 0 and ARR = 11199 (the stock pitch).

**Setting.** Settings 3/3 "Danger alarm" (the second row): **Loud**
(default) -> **Volume** (the same pattern at the key / alert volume; 0 % =
silent) -> **Off** (badge and toast only). Persisted in p[38] bits 16..17
(`meter::EXT_DANGER_SHIFT`, 0 = Loud). SCPI `SYSTem:ALERt:ALARm`,
`SYSTem:BEEPer:VOLume`, `DBG:ALARm?`. The "Jack alerts" mask still decides
whether A-JACK exists at all (OFF / A-JACK only: no A-JACK, no A-PROT, no
alarm).

The drive is the stock's own (TIM14 PWM on PA7, <= 49.5 % duty), from the
main loop only. Emulator:
[`emu/hy3131/loudtest.py`](../../emu/hy3131/loudtest.py) (`run.py
--buzzer-log` logs every ARR / CCR1 change: A-PROT, A-JACK, silencing by
tap, key and toast, re-arm, the Volume setting, the 10 A rule, A-JACK in ohm
and continuity quiet); the 30 s cap and the power meter's 10 A rule are host
tests only. Untested on the device (needs a current in A): the sound
itself, whether the buzzer current moves a DC V reading, whether the 2.86
kHz tone is as loud as the 3.57 kHz one (if not, the pair moves to 3.57 /
4.17 kHz via `BUZZER_ARR_LO`).

### 4.11 AC A and DC A V-JACK?, AC A over range (device 40728b8)

From the device check "40728b8"
([device-check-2026-10-21.md](../../docs/history/device-check-2026-10-21.md); the A jack not
wired, the calibrator on V-COM only).

**CTSTA on the device**: four values only, and CMPHO never without CMPLO
([hw/README.md, Measured node behaviour](../../hw/README.md#measured-node-behaviour)). The firmware rules do not depend on how the bits are read (any
bit set is a hit); the emulator keeps the older reading. DC A's V-JACK? has
a 3 mV margin at 24 V on V/ohm (24 mV at the /1000 tap against 20.8 mV).

**AC A V-JACK?: the swing.** AD1 on SENSE - PB5 drifted to the negative rail
within ~2 s at any V ([hw/README.md, Measured node behaviour](../../hw/README.md#measured-node-behaviour)). Now AD1
= SENSE - AGND (R24 0x03), and since the A rows' SENSE offset is tens of
mV, a DC level means nothing: AC A counts the hold's swing only, PKHMAX -
PKHMIN > `PKV_SWING_THR` 4 270 codes = 2 V rms AC on V, plus the reversal
rule of 4.12. Mains on V in an AC current function is the hazard and is
caught; **DC on V in plain AC A is not flagged** (DC A's comparator and the
DC phase of AC+DC / AUTO A cover DC). In AC+DC / AUTO A the AC phase's
swing-less windows carry the DC phase's comparator run instead of ending it
(`Sentinel::bridge`).

**AC A over range after a range change.** 10 A -> 60 mA changes the shunt,
OP1's input and gain at once, and the first block(s) carry that step: a
false OVERLOAD. Plain AC A holds over-range results back for 500 ms after
the first result of a row (`AC_OVER_HOLD_MS`); a real over range acts
after the hold. Device d2aff22: 10 of 10 switches stay on 60 mA.

**DC A after DC V 60 V+, ohm or a chip restart: 24 V missed** (device
d2aff22, [hw/README.md, Measured node behaviour](../../hw/README.md#measured-node-behaviour)). No register carries this history: `write_config`
writes all 22 bytes R20..R35 from the row every time and zeroes R26 / R34 /
R36 before (`quiesce`), and `DBG:SENTinel:REG?` read the same R20..R37 in
every entry. Only a temperature row before DC A made the comparator see
24 V. Fix: the precondition of 4.12.

**Continuity at 5 V DC**: PB1 follows V-COM there, as in ohm
([hw/README.md, Measured node behaviour](../../hw/README.md#measured-node-behaviour)). The badge stays; the danger alarm does not sound (4.10).

Emulator knobs ([`emu/hy3131/README.md`](../../emu/hy3131/README.md)):
`pb5drift`, `acastep`, `vjblind`; switchtest `aca-drift`, `aca-ovl`,
`real-vjack-ac`, `real-vjack-blind`, `real-vjack-blind-prec`.

### 4.12 DC A precondition, AC A swing reversal, stale readings (device d2aff22)

From the device check "d2aff22"
([device-check-2026-10-21.md](../../docs/history/device-check-2026-10-21.md)).

**The DC A precondition (100 ms by default).** 100 / 300 / 1000 ms of the
temperature row before the DC A row brought V-JACK? back at 1.9 / 2.1 /
2.8 s after DC V 60 V and after ohm, with no false hit at 5 V or with the
source off. `Dmm::precond_ms` defaults to `PRECOND_MS` = 100. It runs only
when a DC A / mA / uA row is entered as a function change while `a_blind`
(set by DC V 60 V+, an ohm-network row or a chip restart; cleared by a
temperature row or the precondition itself), with V-JACK? enabled and no
fuse check. `DBG:SENTinel:PRECond 0` turns it off. Cost: the blocking
100 ms, the relay pulse there and back (2 x 10 ms) and the AD1 grid; the
emulator's first fresh DC A reading after ohm went from 0.34 to 0.55 s.
Device ea73ea5: 24 V V-JACK? in every entry within 1.4..2.8 s (8 s within
the restart mask after Scope -> back), 5 V and off quiet in all 14 entries;
blind DC A +94 ms, blind mA +192 ms, non-blind entries unchanged.

**AC A: SENSE itself is the drifting node** (device d2aff22,
[hw/README.md, Measured node behaviour](../../hw/README.md#measured-node-behaviour)): with AD1 on SENSE - AGND the hold still ran to the negative
rail, and a ramp gave more codes per hold than the swing threshold: a false
V-JACK?. **The swing rule needs a
reversal**: the hold's range must overlap the previous hold's range by more
than `PKV_SWING_THR`. Consecutive holds of a ramp barely overlap; an AC
signal fills the same span in every window, even one clipped at a rail.
Cost: one hold more (~107 ms) to the first hit. `DBG:SENT:VAR +64` runs this
AD1 at IG 0.9 to see where SENSE sits.

**No reading crosses a range change.** For ~0.9 s after `CONF:CURR:AC 10`
-> `0.06`, `FETC?` answered the 10 A row's value, then the AC EMA's step
from it (device d2aff22). `FETC?` now answers +9.91E37 while the shown
reading is not from the row the chip runs (`Dmm::reading_current`: mode,
AC+DC either phase, and range; not in Hz, whose readings carry the
counter's range, nor in the power meter, nor under HOLD / AutoHold). The AC
filters (`f_ac`, `f_lpf`, `ac_raw`) restart on every range step. The
emulator does not reproduce the stale `FETC?`.

Emulator: `senseneg`, `sensetau`; switchtest `aca-ramp`.

### 4.13 A voltage on V-COM: the jack check, the AC mV gate (2026-10-24)

With the A jack empty, 5 / 10 V DC or the 0 / 9.5 V square on V/Ω-COM
raised A-JACK in RES and continuity (519fb8a 7b / E4, 40728b8 item 4,
d2aff22 S6: the AD2 peak on PB1 -99 100 / +124 800 at 5 V) and in AC mV
(2f56ff9 new bug 1: `AJACK,AD1PK_PB1` and the loud alarm, AC V AUTO on
60 mV). In each case PB1 picks up a level from the V side (*inferred*: the
source against OP3's forced VREF through the ~1.4 kΩ input protection in
the ohm-network rows; the input clamp's current in AC mV, emulator `mvpb1`).
The device shows only that PB1 trips from 3-5 V in RES (519fb8a 7c), not at
1 V.

**Ohm, continuity, the resistive temperature sensors (`Sentinel::jchk`).**
With the leads in A / COM and V-COM open the input node sits at the row's
own source: ohm 0.78-2.17 V open on the stock taps, 1.38 V on the 60 MΩ
current-source row ([device check, "Ohm test current
map"](../../docs/history/device-check-2026-10-21.md)); continuity ~-0.48 V. A source on V-COM
moves it. While PB1 hits, AD2 reads OP1O - PB3 (OP1 is the row's own x1
buffer of the input, the numerator of the stock ratio) at IG 0.5 (±2.5 V)
with the peak hold (`sentinel::jack_row`, `jack_driven`):

| row | driven when the 100 ms peak is | at its own source |
|---|---|---|
| ohm, RTD / NTC | above 2.35 V or below -0.9 V | 0.78-2.17 V open, lower with an Rx |
| continuity | above +0.3 V or below -0.9 V | ~-0.48 V open, ~0 V shorted |

Ohm and the resistive sensors take the jack row through the A-PROT probe's
row rewrite at the first hit window (AD2 is off in those rows; AD1 written
last, note 7; no A-PROT there). In continuity AD2 is the sentinel itself, so
`Dmm::cont_jack` points it at OP1O for one window (R25 / R27; settle 30 ms,
100 ms window, back, settle 30 ms), every 300 ms while the hits last. A-JACK
needs a "free" verdict since the hit run began; "driven" drops an A-JACK in
force and counts as a live result, so LIVE? shows. `DBG:SENT?` fields 14 /
15: the verdict (-1 / 0 / 1) and its peak (mV), of the present hit run or of
an A-JACK in force; -1 / 0 once the run is over and after a range or
function change (`Sentinel::new_run`). Up to c6a3ac3 they kept the last
verdict until the next run (check list item 2). `cont_jack` and
`jack_result` are in `.cold`: they run only while PB1 hits and talk to the
HY3131 themselves, so never with the bus handed to a stream (not checked
with the region C hooks).

Device 1a11eda ([device check, "1a11eda: bug-fix recheck"](../../docs/history/device-check-2026-10-21.md))
showed that OP1O does not follow a source above the open-circuit voltage
(2.02-2.25 V with 5 / 10 V on the jack, [hw/README.md, Measured node behaviour](../../hw/README.md#measured-node-behaviour)): all "free", so RES still
raised A-JACK there. Only the square on 600 Ω read 2.38 V (driven); the same
2.38-2.45 V showed in the first windows after the source was switched off.
On 600 Ω no fixed OP1O line separates these values from the row's own 2.17 V.

**The comparator's both-bit reads (`v_both`, since the 1a11eda check).** In
those RES runs the comparator on PB1 read CTSTA 0x70, CMPHO and CMPLO in one
read: an overdriven comparator, as on DC V's V tap at 5 V and in DC A at 24 V
([hw/README.md, Measured node behaviour](../../hw/README.md#measured-node-behaviour)). *Inferred* (the A jack has not been wired): a
current in A puts I x 47.5 Ω on PB1 (0.5 mA to tens of mA = 24 mV to ~1 V),
on one side of the window, so it gives one bit per read; an AC current sets
CMPHO and CMPLO in different reads. So in the jack-check
comparator rows (ohm, the resistive temperature sensors) a both-bit read
within the last `AJACK_CLEAR_MS` (1 s) counts as "driven", as an OP1O peak
above the line does: no A-JACK, LIVE?. It is not held for the whole hit
run: a source taken off with a current in A at once keeps the run going, and
A-JACK then comes ~1 s later. `DBG:SENT?` field 14 then reads
**2**. Continuity (an AD2 peak sentinel, no CTSTA) is unchanged.
*Inferred*: a current in A large enough to push PB1 out of the comparator's
input range (tens of mA, PB1 above ~2.4 V) may also read 0x70. It would then
show LIVE? instead of the A-JACK badge. This is not checked: the A jack has
not been wired.

Device c6a3ac3 ([device check, "c6a3ac3: the RES jack fix"](../../docs/history/device-check-2026-10-21.md#c6a3ac3-the-res-jack-fix-428df52-both-bit-reads-live-after-the-source-2026-10-25)),
A jack empty: in RES AUTO and 600 Ω, 5 V and 10 V read CTSTA 112 on all but
the first poll, field 14 = 2, LIVE?, no A-JACK; the square read 112 on about
half of the polls (0 on the rest), field 14 = 2, LIVE?, no A-JACK. 1 V and
3 V: no hits. The OP1O peaks (field 15) were those of 1a11eda. The current-in-A
side (check list item 4) is not run.

**LIVE? after the source goes off.** Device 1a11eda note 3: in RES 600 Ω,
LIVE? came on again at every poll (5-10 ms apart) once it had cleared, until
the next ohm reading reset its run (+24 `DBG:SENT:LOG?` entries). A run of
live results whose last one is `LIVE_CLEAR_MS` old no longer asserts.
Device c6a3ac3: after the off, LIVE? stayed ~1.0-1.25 s and did not return;
`DBG:SENT:LOG?` +0 entries in 5 of 5 (5 V x 3, 10 V, the square).

**Continuity / diode: the probes touched together (`V_GATE` on OL, 2026-10-26).**
Owner video, leads in V / COM, A jack empty: touching the probes in
continuity showed A-JACK for ~1.5 s while the readout went OL -> 814 -> 277
-> 28.85 -> 0.67 Ω. The jack check calls both the open input (~-0.48 V) and
a shorted one (~0 V) "free", so PB1 hits during the contact asserted A-JACK.
What makes PB1 swing then is *unknown* (the device's PB1 during a contact
was not recorded; *inferred*: the source's current step on the shared COM).
The case A-JACK is for, the leads in A / COM, leaves V-COM open, so these
rows read OL. Continuity and diode now take the reading as their V side
(`Dmm::v_side_ohm`): a reading under OL within the last 1.5 s
(`CLOSED_HOLD_MS`; a bouncing contact reads OL in between) = closed, no
A-JACK, and an A-JACK in force drops. An OL reading counts as "open" only
from 450 ms after the hit run began (`CLOSED_VGATE_LAG_MS`: a 400 ms
OSR_SLOW conversion that ends just after the contact still reads OL). Cost
(emulator, 2 mA DC / 5 mA AC in A, V-COM open before): A-JACK 0.6-0.9 s
after the onset instead of 0.16-0.36 s; up to ~1.5 s when V-COM read a part
until the onset (the hold). A current in A while a part is on V-COM (a
third lead) shows no A-JACK there. Emulator:
the model's `contbump` (a PB1 burst on every V-COM resistor change in
continuity / diode, shape *inferred*), senttest `cont-bounce` /
`diode-bounce` (two bouncing taps, then a current in A with V-COM open).
Device, the calibrator's resistance output on V-COM switched between open and
0 Ω (its relay) or 0 and 400 Ω: every step put AD2 peaks of 14 000-231 000
codes on PB1 for 1-2 windows, with the jack check "free"; on 6c686ce8 that
asserted A-JACK on 3 of 8 closures (shown 0.3-1.5 s after each), with the
fix on none of 28 steps.

**AC mV (`V_GATE` on the row's own result).** Plain AC mV takes |LPF| +
1.414 x RMS of each result (the quantity of `MV_CLIP_K`): above 1.5 x the
range = driven (`MV_DRV_K`; a full-scale sine is 1.41 x, the device's
clipped square on 600 mV ~2.4 x). A-JACK needs a "free" result at least
250 ms after the hit run began (`MV_VGATE_LAG_MS`, *inferred*: the RMS /
LPF block lags the peak hold). A result over the converter range gives no
verdict. No LIVE? here: the reading is over range. Cost: a real current in
A asserts ~0.25 s later in AC mV.

**Gaps.** A current in A together with a voltage on V-COM shows LIVE? (ohm,
continuity) or nothing (AC mV, AC+DC mV). The diode row is unchanged
(badge only).

**AC+DC V on 600 / 60 mV.** Its AC phase is the AC mV row with the same AD1
peak sentinel; it had no gate up to 7ab5c0e (`vgate` was plain AC mV only),
so the square or 5 V DC on V-COM with manual 600 mV raised A-JACK in the
emulator (`mvclamp` / `mvpb1`; not seen on the device: AUTO lands on 6 V
with these inputs). Both phases now set `vgate` and share the run, so the
AC phase's result is the verdict for the DC phase's hits too (9a191ea); a
real current in A asserts after the next "free" AC result (~2.2 s after
the onset in the emulator). Emulator senttest
`acdcmv-sq600` / `acdcmv-dc5`: FAIL on 7ab5c0e (A-JACK from ~1.5 s after
the voltage on), pass with the gate.

**AC+DC mV after the source goes off.** Device
[185c066](../../docs/history/device-check-2026-10-21.md#185c066-plain-ac-first-readings-8ed9499-acdc-mv-a-jack-9a191ea-two-open-triage-items-scope-trigger-level-in-volts--nearest-marker-drag-f6c6ac3-2ad7feb-2026-10-25)
(manual 600 mV): A-JACK 0.45-0.9 s after the square or 5 V was switched
off in 8 of 13 runs, held while the input was open, the readout OL; plain
AC mV 0 of 6. How the input holds a charge there and why PB1 hits are
*inferred*; the AC phase's results are under 1.5 x the range, i.e.
"free". Now a DC phase result whose magnitude is over the range
(`Dmm::mv_dc_drv`, the readout's OL) makes the AC phase's results say
driven until the next DC result. Emulator: the model's `mvhold` (the open
input keeps 0.75 V and PB1 picks up 40 mV after the source goes off, seen
by both mV rows or, `mvholdrows:1`, by the DC mV row only); senttest
`acdcmv-sq600-off` / `acdcmv-dc5-off` / `acdcmv-off-dcrow`: FAIL on
5a4dbce (A-JACK ~1 s after the off), pass now; a current in A with a
source back on V-COM still asserts A-JACK there.

**Emulator.** Since the 1a11eda check the model reproduces the device's
RES numbers: OP1O in the ohm rows stops at 2.47 - 0.044 x V for a source of
V volts (`ohmop1k`, 2.25 V at 5 V, 2.03 V at 10 V; the line is *inferred*
from those points), and a source at or above 4 V on the jack sets CMPHO and
CMPLO together on PB1 (`ohmboth`, *inferred* between the device's 3 V and
5 V). Failing on f59bac2 (A-JACK at 5 V, verdict 0, field 15 2250 / 2030 mV,
as the device), passing on the fix: senttest `ohm-v5`, `ohm-v5-600`,
switchtest `dc5-pickup`, `sq-pickup`, loudtest `ajack-res`; a real current
in A still asserts (their last phases, switchtest `real-ajack-pickup`).
Before that, the model read OL for a positive source above the ohm OCV
(`OhmRevOl`, default, as the device; `ohmrevol:0` = the old negative code),
adds `Pb1VjackMv` to PB1 in the continuity row, and gives AD2 = OP1O - PB3
(the node across Rx within -1.18 .. +2.55 V, *inferred* swing) in the ohm /
continuity rows. Failing on ad10749, passing now: senttest `ohm-v5` (now
LIVE? at 5 / 10 V and the square), `ohm-v5-600`, `cont-v5`, `acmv-sq60`,
`acmv-sq600`, `acmv-dc5` (no A-JACK in `DBG:SENT:LOG?` from the voltage on;
A-JACK with a current in A afterwards); switchtest `dc5-pickup`,
`sq-pickup`; loudtest `ajack-res`, `ajack-cont-v5`, `ajack-acmv` (quiet
with the square, loud with a current). Passing on both (regression checks):
switchtest `real-ajack-pickup`, loudtest `ajack-acv-auto` (AUTO mV-first:
the clip step of 2f56ff9's fix already leaves mV in the model).

#### Device check list (jack check)

A jack empty unless stated; alarm Off (quiet rule); read `DBG:SENT?` and
`DBG:SENT:LOG?`:
1. RES (AUTO and manual 600 Ω), continuity: V-COM open, then 1, 3, 5, 10 V
   DC and the 0 / 9.5 V square, 10 s each. Expect no A-JACK; LIVE? from 5 V
   (RES) / 1 V (continuity). Note fields 14 / 15 wherever field 8 (hits)
   grows: they settle the OP1O point above.
2. RES and continuity, V-COM open: fields 14 / 15 stay -1 / 0, also after
   a switch from another ohm-network row (RES AUTO -> 600 Ω, RES ->
   continuity) once field 8 (hits) stops growing. Device c6a3ac3 (before
   the fix): -1 under RES AUTO; after such a switch they kept the verdict of
   the switch's hits (0 / 2150 mV in 600 Ω), no alert (device-check note 3).
3. AC V AUTO (mV-first) and manual 60 / 600 mV with the square: no A-JACK in
   the log.
4. With a current in A (not wired yet), V-COM open: RES, continuity, AC mV:
   A-JACK within ~1 s; field 14 = 0, field 15 near 2.17 V (RES 600 Ω) /
   -0.48 V (continuity); in RES field 4 (CTSTA) 32 / 96 (one bit), never
   112.
5. RES (AUTO and 600 Ω) with 5 V, 10 V and the square: field 4 reads 112
   (0x70) while field 8 grows, field 14 = 2, LIVE?, no A-JACK. Then switch the
   source off: `DBG:SENT:LOG?` grows by at most one entry. Device c6a3ac3:
   **PASS** (and items 1 and 3 with manual 60 / 600 mV; the mV-first AUTO,
   item 4 and RTD / NTC not run).

## 5. Fuse, CAT and current limits (the DM40 manual)

From the DM40 user manual (EN v1.x, section 2.4 and Table 2.3.1):

- ONE current jack "A mA uA", "10A (Max)", "Built-in 10A ceramic fuse.
  Replace with the same model if burned": a single 10 A ceramic fuse in
  front of all shunts. No time limit for 10 A, no separate mA / uA fuse.
  Current ranges (DM40C): 600 uA / 6000 uA / 60 mA / 600 mA / 6 A / 10 A; AC
  V max 750 V, DC V 1000 V.
- CAT: the manual states none for the DMM inputs (only the scope probe: x1
  150 Vrms CAT II, x10 300 Vrms CAT II); the case print reads "CAT IV 600V
  CAT III 1000V" (the vendor's claim, unconfirmed).
- The defaults that follow: hard limit 10 A (the danger alarm, 4.10), a 30 s
  countdown above 6 A in the power meter (the top regular range; no vendor
  duty-cycle figure, so the common hand-held practice), and the OVERLOAD
  rule of 4.7 (a uA / mA overload heats the 47 ohm resistor long before the
  fuse opens).

### HPF bypass probe (device d150919, 2026-10-22)

No reserved bit of R26 / R34 / R35 bypasses the RMS block's high-pass
filter (the probe: [hw/README.md, Measured node behaviour](../../hw/README.md#measured-node-behaviour)): DC power stays a software product (the LPF
means or AD1); the hardware multiplier covers the AC part only.
