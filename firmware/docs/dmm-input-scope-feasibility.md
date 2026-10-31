# DMM inputs as a scope source (AD2 streaming): feasibility

How opendm40 streams the HY3131's fast converter AD2 from the DMM jacks: as
the data-logger source `LOG:SOURce AD2` (idea **(B)**, a logger faster than
AD1's 640 S/s) and as the DMM **wave view**, a slow isolated scope on the DMM
jacks shown on the stock-scope page (idea **(A)**). Sections 0-6 are the
2026-10-21 study, with its predictions replaced by what was built; sections
7-13 describe each implementation step (E0-E7) and its device results. Code
and [TODO.md](../TODO.md) cite this doc by section number.

Tags: **INFERRED** = follows from the cited sources but is stated nowhere;
**UNKNOWN** = not known. Untagged facts come from the source cited with them:
the HY3131 datasheet ([`hycon-hy3131-datasheet-v10.pdf`](../../hw/datasheets/hycon-hy3131-datasheet-v10.pdf),
transcribed in [`emu/hy3131/ref/registers.md`](../../emu/hy3131/ref/registers.md) and
[`emu/hy3131/RESEARCH.md`](../../emu/hy3131/RESEARCH.md)), the stock rows
(`stock/re/analysis/hy3131_v2_6.h`, in the private analysis repo), the opendm40 source,
the GD32F4xx User Manual, or a device measurement with its commit. Short
names: "acdc" = `stock/re/docs/history/acdc-stock-vs-opendm40.md` (in the private analysis repo),
"power doc" = [power-meter-and-misconnect.md](power-meter-and-misconnect.md)
(sections 1-3: the AD2 / AD3 mux, OP1 / OP2 routing, the row facts),
"device check" = [device-check-2026-10-21.md](../../docs/history/device-check-2026-10-21.md).
The board (pins, isolator, input network, shunt nodes):
[hw/README.md "DMM front end (HY3131)"](../../hw/README.md#dmm-front-end-hy3131).

## 0. Verdict

| idea | 2026-10-21 verdict | outcome |
|---|---|---|
| (A) DMM-input scope | GO as a *slow* scope, 5-20 ms/div .. 50 s/div, conditional on a gap-free readout (an ISR) | built (section 10, default build): 1 ms/div .. 50 s/div at 1200 .. 76 800 S/s, V / mV / A on the isolated jacks, DC- or AC-coupled |
| (B) AD2 logger | GO, low effort, first | built (sections 7, 9, 11): `LOG:SOURce AD2`, 1200 .. 76 800 S/s, 0 lost on the device up to 38.4 kS/s to the host (11.10, 11.11) and 76.8 kS/s to the disk (11.12) |

The two unknowns of the study are settled: the AD2 data register holds a
result for a whole period and a read never tears (device f80524f, 1.4), and
the main loop's stalls are taken out of the path by one timer ISR that owns
the bus during a stream (sections 9, 11).

## 1. AD2 / AD3 facts

### 1.1 Rate, resolution, range (datasheet 11, 13.2; registers.md)

* AD2CLK = SYSCLK/2 (SAD2CLK = 0) or /4 (SAD2CLK = 1); SYSCLK = 4.9152 MHz
  crystal. Output rate = FAD2CLK / OSR2, OSR2 = 32, 64, 128, 256, 512, 1024
  (AD2OSR codes 5, 6, 7 are all 1024). AD3 shares the clock and OSR.
  ENCHOPAD2 does **not** change the rate (device f80524f: R26 0x25 and 0x05
  both 422 us per flag-synced sample, 0x24 and 0x04 both 232 us), although
  the study assumed it halves it. The ladder (`hy3131::ad2_r26_for`, the
  same rungs with and without the chopper):

  | OSR2 | /2 clock | /4 clock |
  |---|---|---|
  | 32 | 76 800 (13.02 us, the stock AC rows) | 38 400 |
  | 64 | 38 400 | 19 200 |
  | 128 | 19 200 | 9 600 |
  | 256 | 9 600 | 4 800 |
  | 512 | 4 800 | 2 400 |
  | 1024 | 2 400 | 1 200 (the slowest the chip has) |

  The logger uses /2 for 2400 .. 76 800 S/s and /4 only for 1200. Below
  that the LPF (OSR4 = 128 .. 16384 on the AD2 stream) gives a decimated
  stream down to a few S/s, but its real output period on the device is half
  the datasheet formula (RESEARCH.md 8.1: 106.7 ms measured vs 213 ms for
  LPFBW = 111); no LPF rate is used as a source.
* Data: AD2<18:0> signed 19 bit in R03..R05 (LSB first, bits 23:19 = sign
  copy; max 0x3FFFF, min 0xFC0000). Transfer (datasheet 13.2, RESEARCH 2):
  code = IG * Vin / (RG * VR) * 2^18, IG = 0.5 / 1.0 / 1.5 / 2.0 (AD2IG), RG =
  1 / 0.333 (AD2RG), VR = AD2RH - AD2RL = PB6 - AGND = 1.25 V in every stock row.
  Full scale at the input node: **+-1.25 V / IG**, i.e. +-1.25 V at IG 1.0,
  +-2.5 V at IG 0.5; 4.77 uV / code at IG 1.0.
* Inputs (mux, registers.md): AD2IP = OP1O, OP2O, PB4, PB7; AD2IN = RLU, AGND,
  PB3, PB5. SDO23 = 1 routes AD3's data into the same R03..R05 register
  instead (one data register for both fast ADCs, 13.2 "MUX").
* Gain drift 10 (typ) / 30 ppm/C (AD2/AD3, table 6) vs 2.5 ppm/C for AD1; no
  AD2 noise, ENOB, linearity or bandwidth figure anywhere in the datasheet
  (the only bandwidth entries are OP1/OP2 2 MHz and the comparator 6 MHz).

### 1.2 Resolution actually available

The device floor in the stock AC V 4 V row is 31..33 `sqrt(RMS)` counts with a
quiet input (acdc 6a, 6b). RMS = sum(X^2)/N of the high-passed AD2 stream,
so that is the per-sample rms noise: **~32 codes against the 2^19 full span
at 76.8 kS/s = 14.0 ENOB** by Hycon's own definition (APD-DMM001 eq. 3: ENOB =
log2(FSR / rms noise), FSR = the full code span; the same convention is used
for AD1 below); ~11.3 noise-free bits (pp ~ 6.6 sd). It includes the divider /
OP1 front-end noise.

Measured with `DBG:AD2:NOISE?` on the open AC A path (device f80524f, the
converter floor): 47.5 codes sd = 13.4 ENOB at 76.8 kS/s, 2.7..7.2 codes =
16.1..17.6 ENOB at the slower rungs. For comparison AD1 at its fastest (OSR
256 unchopped, 640 S/s): 25 counts sd at OSR 32768 chopped (acdc 6c) scaled
by 1/sqrt(OSR) gives ~280 counts of 2^24, ~15.9 ENOB (INFERRED, the
emulator's model). AD2 buys rate, not bits.

### 1.3 Coupling: which jack reaches AD2, and how (stock rows + device)

| path | stock row | AD2 input | coupling | evidence |
|---|---|---|---|---|
| V (4 .. 1000 V) | f0 AC V r0..r3: R27 0x9d, R32 0x38 | PB4 - AGND, PB4 fed from OP1O (OP1+ = SENSE tap) through an external capacitor ([hw/README.md](../../hw/README.md#dmm-front-end-hy3131) input network) | **AC**, tau ~0.3 s (fc ~0.5 Hz) | acdc 5 / 6a transient; RESEARCH 7; power doc 2 |
| mV (40 / 400 mV) | f2 AC mV: R27 0x5d, R32 0xaa | OP2O - AGND, OP1+ = OP2+ = RLU | **DC** (the LPF reports the full DC: 6a "LPF = 495 mV") | acdc 6a |
| A / mA / uA | f4, f5_6 AC A: R27 0x2d, R32 0x3e / 0x3d | OP1O - PB3, OP1+ = PB2 (10 mohm) / PB1 (0.5 ohm, 47.5 ohm), x10 / x1 (shunt nodes: [hw/README.md](../../hw/README.md#dmm-front-end-hy3131) current path) | **DC** (6b: LPF = 8.8 uA constant offset on the open uA range) | acdc 6b |
| V, DC-coupled | none in the stock | **register-only new row**: the AC V row of the range with SAD2IP = 00 (OP1O) instead of PB4, OP1+ = SENSE (R32 0x38, which the stock itself writes in the AC+DC DC phase), SAD2IN = AGND, IG 1.0 | DC | device test 6.5 (section 13): linear to 0.003 % of the range, 3..10 codes sd at 4800 S/s; implemented (E7, [`ad2exp.rs`](../src/ad2exp.rs) `dcv_row`) |

So current waveforms (through the shunt, DC included) and mV waveforms need
no new row: the stock AC A / AC mV rows already put the DC-coupled signal on
AD2; only the readout changes. Voltage waveforms are AC-coupled with the
stock row and DC-coupled with the new row.

Enable rules that matter for a streaming mode (datasheet notes):
* 13.1 note 7: do not switch AD2/AD3 on or off while AD1 converts (AD1 offset
  shifts; measured in 13.1 / 13.2). Enable AD2 *with* the row and leave it;
  for the AC rows it already is.
* Turning ENAD2 off clears R03..R05 (13.2 (1)).
* AD2 and AD3 together: enable AD3 first ([`hy3131.rs`](../src/hy3131.rs) `configure`, the
  beat fix, acdc 10). The stream runs AD2 alone with AD3 and RMS / LPF off
  (`hy3131::set_ad2_stream`), so AD2F paces one grid only.

### 1.4 Readability of single conversions

* Register: R03..R05 (3 bytes, auto-increment read in one CS frame).
  **Width** 19 bit signed. **Flag**: INTF.1 AD2F set on every AD2 (and AD3)
  conversion; INTF is cleared by *reading it* (all flags at once, AD1F / RMSF /
  LPFF included) or by writing 0s; flags accumulate until read (datasheet 10).
  Optional **IRQ pin**: with INTE.AD2IE = 1, DO (= MISO, PC11) is high while CS
  is high and a flag is pending (datasheet 7.2 (4)); unused by the stock
  (INTE stays 0) and by opendm40.
* **No FIFO, no buffer.** The datasheet lists one data register per converter
  and no depth; an unread conversion is overwritten by the next (INFERRED
  from the register set).
* **No torn reads.** Device f80524f (`DBG:AD2:TORN?`, device check "f80524f"
  1): signbad 0 and no difference without a new conversion on every rung, no
  bump at 9 or 17 bits in the difference histogram; flag-synced pairs at the
  slow rungs are 2000 of 2000 identical. R03..R05 hold one result for a whole
  period; the stream ISR relies on that (11.1, 11.2). The read still has to
  fit between two conversion edges (11.2, 11.12).

## 2. Readout bottleneck

### 2.1 What a sample cost on the bit-banged bus (2026-10-21)

The bit-banged bus of the study is gone (reads since d150919, writes since
42cbf35: section 8). Its costs, for the record: a flag check (`read1(R1E)`) 45
us, a 3-byte AD2 read 77 us, both 122 us; the 20-byte stock AC poll 349 us.
INTF (R1E) and AD2 (R03) are not adjacent, so the flag needs its own frame.
Measured link ceiling on the device: ~6.2-6.6 kS/s (f80524f), not the ~8 kS/s
predicted.

### 2.2 Hardware SPI on the same pins

* **Pins.** PC10 / PC11 / PC12 are SPI3 SCK / MISO / MOSI on alternate
  function 6 in the STM32F4 pin map that the GD32F4xx follows (GD32 naming:
  **SPI2**, base 0x4000_3C00, APB1; the AF assignment in the GD32F450xx
  datasheet Rev1.1 table 5 (p. 29: PC10 SPI2_SCK, PC11 SPI2_MISO, PC12
  SPI2_MOSI, all AF6), SPI2EN = RCU_APB1EN bit 15, SPI_CTL0 / SPI_STAT per
  GD32F4xx UM V2.2 21.11.1 / 21.11.3; pins: [hw/README.md](../../hw/README.md#dmm-front-end-hy3131)).
  CS = PD2 stays a GPIO (the hardware NSS toggles per transfer burst or per
  word in TI mode, neither matches a multi-byte HY3131 frame). SPI mode 0 (CK
  idle low, DO changes on the falling edge: datasheet 7.2 (7)/(8)), MSB first.
* **Clock.** APB1 = 50 MHz (RCU_CFG0 0x940A read back on the device: APB1 /4,
  [`clock.rs`](../src/clock.rs)). SPI prescalers give 25 / 12.5 / 6.25 / 3.125 MHz. HY3131
  limits (table 6): TCKHI >= 20 ns, TCKLO >= 60 ns, DO ready 60 ns after the
  falling edge. The bus crosses the CA-IS3741 isolator
  ([hw/README.md](../../hw/README.md#isolation-boundary)), so /16 = 3.125 MHz is
  the bus and /8 = 6.25 MHz needs the CKPH = 1 data phase (8.2, 11.3).
* **The 9th clock.** A read is 8 command bits, one blank "read period" clock,
  then 8n data bits (7.2 (2)). The CPU frames make it as one GPIO pulse on
  PC10 between the SPI bytes (the exact datasheet frame, section 8); the DMA
  frames clock cmd + (n + 1) bytes and shift the received bits by one
  ("shift framing", 11.8). Writes need no extra clock (7.2 (3)).
* **Cost at /16** (device, `DBG:HYSPI:SELF?`, 8.6 / 11.3 / 11.9): 1-byte read
  8.0 us, 6-byte read 21.6 us, 23-byte read 69 us, 1-register write ~10 us.
* **DMA.** SPI2_RX -> DMA0 channel 0, SPI2_TX -> DMA0 channel 7 (UM table
  10-5; the other channels' users: 11.8). A fully autonomous timer-paced
  capture is not possible on this board (11.8); the stream ISR starts each
  DMA frame (~1 us of CPU per sample).

### 2.3 Who runs the read: the main loop vs an interrupt

At the time of the study opendm40 ran every task from one main loop with
interrupts masked. Two `DBG:PROF?` captures on the DMM page (2026-10-21,
104-120 s each; raw files not committed):

| metric | capture 1 | capture 2 |
|---|---|---|
| passes / s | 22 247 | 33 523 |
| loop avg / ema | 44 / 27 us | 29 / 37 us |
| loop max, max in the last 1 s | **24.9 ms, 9.5 ms** | **16.0 ms, 4.5 ms** |
| passes >= 5 ms | 5 289 (~50 / s) | 48 |
| `d.read` (DMM poll, bit-bang) max | - | 15.5 ms |
| `d.chart` (strip chart redraw) | - | 0.9 .. 3.2 ms |

The 1-25 ms stalls are LCD redraws, the DMM poll, the 10 ms relay pulse and
SPI-flash writes. With no FIFO, a polled reader loses every conversion inside
a stall (48 per 5 ms at 9.6 kS/s). The main-loop sampler (section 7) showed
it on the device: 8 % lost at 4800 S/s and 11 % at 9600 over SPI2 (d150919),
and the DMM page repaint costs a 26 ms gap every 100 ms (f80524f).

So the stream got **one interrupt** (decision #3 of
[decisions-2026-10-22.md](../../docs/history/decisions-2026-10-22.md)): TIMER6 owns
the HY3131 bus while a stream runs, reads each conversion and pushes it into
the logger's ring, which the main loop drains (section 9; its loss fixed in
11). The study's ceiling of 19.2 kS/s for flag-synchronised reads does not
apply to the blind, phase-locked reads of 11.2: 38.4 kS/s with 0 lost on the
device at 17.5 % CPU (DMA frames, 11.10) and 76.8 kS/s with hybrid frames
(11.12).

## 3. What that gives as a scope

### 3.1 Timebases

The stock-scope screen is 400 columns / 10 divisions (scope.rs COLS). The
wave view's bands (`scope::wave_fs_for`, from `WAVE_TB_MIN`):

| s/div | AD2 rate | samples per screen |
|---|---|---|
| 1 ms | 76 800 (hybrid DMA frames, 11.12) | 768 |
| 2 ms | 38 400 | 768 |
| 5 ms | 9 600 | 480 |
| 10 / 20 ms | 4 800 | 480 / 960 |
| 50 / 100 ms | 2 400 | 1 200 / 2 400 |
| >= 200 ms | 1 200; roll from 200 ms/div as the MCX scope (TB_ROLL), per-column min / max | |

Decimation for the slow timebases comes from the chip (OSR2) or the
per-column min / max of the full stream, never from dropping samples
(aliasing).

### 3.2 Bandwidth and anti-aliasing (INFERRED)

The AD2 decimator is a comb (sinc) filter of order 3 (the block diagram label
"Comb Filter ³", datasheet 13.2). For decimation N from the 2.4576 MHz
modulator rate, |H| = (sin(pi f N / fm) / (N sin(pi f / fm)))^3: -3 dB at
~0.26 of the output rate, -12 dB at half the output rate, nulls at the output
rate and its multiples, i.e. the folding bands are attenuated (> 50 dB at an
offset of 10 % of fs from the null). So the useful band is **~0.25 x output
rate**: 2.5 kHz at 9.6 kS/s, 1.2 kHz at 4.8 kS/s, 20 kHz at 76.8 kS/s (the
stock AC rows). Above the AD2 band the DMM front end limits anyway (AC
bandwidth ~20 kHz: [hw/README.md](../../hw/README.md#dmm-front-end-hy3131)), and the
0.3 s coupling time constant sets the low end of the AC V path at ~0.5 Hz.
The real scope channel: 10 MHz, 50 MS/s, 10 bit, 1 Mohm (manual 1.2).

**The comb's droop in the measurements (device d8e249d, 2026-10-22).** The
Wave view's Vrms of a 40 Hz square read 4.74 V up to 20 ms/div and 4.69 V
(-1.0 % more) from 50 ms/div (2400 S/s); the logger's own CSVs of the same
square gave -1.6 % at 2400 and -1.2 % at 4800 against the DMM's AC V (which
converts at 76.8 kS/s). Not the window: the RMS of the logger CSVs never went
through `scope_meas`, and |x| of a square is constant except at its edges, so
a partial period cannot move its RMS. It is the sinc^3 decimator: each sample
is a weighted mean over 3 output periods, so every edge becomes a ~3/fs ramp
and the harmonics above ~0.25 fs drop out. A 40 Hz square through it, RMS of
the samples (host model, same as the device within its calibrator drift):

| AD2 rate | 40 Hz square | 50 Hz sine | 400 Hz sine |
|---|---|---|---|
| 76 800 (stock AC rows) | -0.06 % | 0 | -0.01 % |
| 9 600 | -0.47 % | -0.01 % | -0.85 % |
| 4 800 | -0.95 % | -0.05 % | -3.4 % |
| 2 400 | -1.9 % | -0.21 % | -12.9 % |
| 1 200 | -3.8 % | -0.85 % | -43 % |

So it cannot be fixed after the fact: the samples are already band-limited
(an inverse-sinc FIR would only lift the passband and amplify the noise). The
rate is the lever (a faster band for the slower YT timebases); not done
(TODO "Vrms -1.6 % from 50 ms/div"). The Wave view's RMS is the RMS of the band-limited
waveform it shows; the DMM's AC reading is the reference for a true RMS of
signals with fast edges. The emulator's AD2 model samples the input
point-wise (no comb), so wavetest's sine passes at every rate and cannot show
this.

### 3.3 Vertical ranges, scaling, accuracy

Full scale at the AD2 node is +-1.25 V / IG (1.1). With the stock rows (IG 1.0,
R25 0x40; r0 rows 0x43 = IG 1.0 with OPS1/OPS2 x10):

| DMM function / range | node gain | AD2 window at the jack | code size at the jack |
|---|---|---|---|
| V 4 V (tap /10) | 1/10 | +-12.5 V peak | 47.7 uV |
| V 40 / 400 / 1000 V | 1/100 .. 1/10000 | +-125 / +-1250 / (+-12 500) V | 0.48 / 4.8 / 48 mV |
| mV 40 / 400 mV | x10 / x1 | +-125 mV / +-1.25 V | 0.48 / 4.8 uV |
| A 4 A / 10 A (10 mohm, x10 / x1) | 0.1 / 0.01 V/A | +-12.5 A / +-125 A (fuse 10 A) | 48 / 480 uA |
| mA 40 / 400 mA (0.5 ohm) | 5 / 0.5 V/A | +-250 mA / +-2.5 A | 0.95 / 9.5 uA |
| uA 400 / 4000 uA (47.5 ohm) | 475 / 47.5 V/A | +-2.6 / +-26 mA | 10 / 100 nA |

Headroom above the DMM range is ~3x (peak), enough for crest factors of mains
/ PWM waveforms.

Scaling (`dmm::ad2_scale_of`): `V = code * 1.25 / 2^18 / IG * k` with the
stock range factor k, exactly the LPF formula of the stock; the stock
calibration record holds a **gain and offset for the LPF (DC mean of AD2) of
every AC row** (0x64/0x74 V, 0x84/0x8c mV, 0xf4/0x10c A, hy3131_v2_6.h), i.e.
a calibrated DC scale of the AD2 path for the DC-coupled rows (mV, A), which
the wave view and the logger apply to the raw samples. The DC-coupled V row
has no stock calibration: it is self-calibrated against AD1 at every start
(13.3 `dc_enter`), within 0.02 % of the range on the device (13.4).

### 3.4 Isolation (the plus) and conflicts

* The DMM side is the isolated domain: HY3131, reference, divider, shunts and
  the banana jacks sit behind the CA-IS3741 digital isolator and the
  VPS8504B isolated supplies; the MCU, USB, scope and generator share one
  ground ([hw/README.md](../../hw/README.md#isolation-boundary)). Hence a
  **floating, 1000 V / 10 A waveform channel**: mains-side voltage waveforms,
  current waveforms through the A shunt (half-wave rectifier draw, PWM motor
  current, inrush, section 12) with the USB cable connected, none of which
  the MCX scope input (40 Vrms CAT II at x1, shared ground) can do. The scope
  has no current input at all.
* DMM readings: the stream needs AD2 at the stream's OSR with RMS / LPF off,
  and every INTF read clears the DMM's flags too, so the DMM reading
  **pauses** while a stream runs (decision #3: exclusive bus hand-off); the
  DMM row is rewritten afterwards. A reading alongside (a request queue for
  the DMM's frames between two samples) is not built (10.5). AD2F is also
  set by AD3 conversions (13.2 "in common with AD2"): the stream turns AD3
  off.
* Row / relay switching: every range or function change is the stock
  sequence (quiesce, 10 ms relay pulse, 22-byte row, settle 300-900 ms;
  [`dmm.rs`](../src/dmm.rs) `write_config`), a gap of ~0.5-1 s in the waveform.
  The DMM autorange does not run while streaming; the wave view's V/div
  steps change the DMM range only where a step needs another range (10.1).
* Functions with no waveform: ohm, continuity, diode, capacitance, frequency,
  temperature. The wave view is offered for V, mV and A.
* AC V row: the AC coupling means the trace has no DC and a 0.3 s settling
  after any switch (acdc 6a); the wave view labels it "AC" and offers the DC
  row (13.3).

## 4. Data logger via AD2

| | AD1 (logger.rs) | AD2 |
|---|---|---|
| rates | 40 / 80 / 160 / 320 / 640 S/s nominal (chopped 2048 .. unchopped 256); 160 .. 640 S/s unchopped on the stream ISR | 1200 .. 76 800 S/s (1.1), each rung on the stream ISR |
| resolution | 24 bit, ~16 ENOB at 640 S/s, chopper-cancelled offset, 2.5 ppm/C | 19 bit, 13.4 ENOB at 76.8 kS/s .. 16-17.6 at the slow rungs (1.2), offset via ENCHOPAD2 (the logger default), 10-30 ppm/C |
| functions | DC V / mV / A (AD1 rows) | mV, A directly (DC-coupled AC rows), DC V with the E7 row (13), AC-coupled V with the stock row; refused in AC+DC / AUTO coupling / the power meter (alternating rows) |
| gaps | marked by the seq / timestamp scheme ([`logger.rs`](../src/logger.rs) `missed`, `overwritten`, LOG:GAPs?) | the same; the stream ISR's own counters in `DBG:ISR?` (11.2) |
| ring | 2048 x 8 B = 16 KiB in SRAM: 51 s at 40 S/s | 427 ms at 4800 S/s, 26.7 ms at 76.8 kS/s: the host must drain `LOG:DATA?` often, else `overwritten` counts the loss |
| to the host | USBTMC `LOG:DATA?` | rows of 12 B, or `LOG:FORMat PACKed \| DLG` ([compact-log-format.md](compact-log-format.md)) for the faster rates; a raw Python drain reached ~37-46 k rows/s on the device (11.10, device check "c830e09") |
| to the disk | [`fastlog.rs`](../src/fastlog.rs) (the Record page / `LOG:DEST DISK`): DLG files | up to 76.8 kS/s after a page-program probe of the SPI flash, else 38.4 kS/s (11.12) |
| BT 115200 | fine | the link, not the chip, is the limit |

AD2 pays off for the rates AD1 cannot do (> 640 S/s) and for the AC-coupled
and current paths; at 2.4 kS/s AD2 averaged 4:1 matches AD1's 640 S/s figure.

## 5. Recommendation (2026-10-21) and what was built

### 5.1 Go / no-go

(B) first, then (A) as a slow floating scope, conditional on a gap-free
readout. Both were built in that order (sections 7-11).

### 5.2 Architecture

As built:

```
HY3131 --SPI2 (PC10/11/12, /16 = 3.125 MHz; /8 + CKPH for the data phase) + CS PD2 GPIO
   |
   |  main loop: dmm.rs poll (paused while a stream runs)
   |  stream: hy3131::stream hands the bus to TIMER6 (IRQ 55), logger::isr
   |          phase-locked blind reads (ad2pll.rs), CPU or DMA frames
   v
logger ring (2048 x 8 B, SPSC) --+--> LOG:DATA? (host)
                                 +--> fastlog.rs (disk)
                                 +--> logger::wave::drain --> scope.rs record (10-bit) --> scope_page / scope_ui
                                       trigger per sample in the ISR (trigsoft::Stream)
```

1. **Bus** ([`hy3131.rs`](../src/hy3131.rs)): SPI2 is the only bus, reads and writes (section 8,
   11.7); no bit-banged back end.
2. **One ISR**: TIMER6 (not the EXTI on the HY3131 IRQ), phase-locked to the
   chip's conversions (11.2), the bus exclusively its own while a stream runs;
   any main-loop bus access stops the stream first (9.1 guard). No request
   queue and no INTF shadow: the DMM reading pauses.
3. **Scope source**: the record holds 10-bit codes like the MCX record, not
   u16 = 19 bit >> 3 (10.1, deviation note), so the renderer, measurements,
   cursors, STOP / zoom, CSV and FFT run unchanged.
4. **UI**: "Wave" on the MF range page of V / A and the source badge
   "MCX | DMM" on the scope page open the same view; V/div steps a list of
   screen-only and range steps (10.1);
   the coupling key switches V between the DC and the AC row (10.1, 13.3).
5. **Logger**: `LOG:SOURce AD1|AD2`, `LOG:SRATe`, `LOG:SCALe?`; scaling on
   the host from `LOG:PREamble?` / `LOG:SCALe?` (`tools/dm40scpi.py`).

### 5.3 Effort

| step | what | status |
|---|---|---|
| E0 | device experiment hooks (section 6) | done (7.1, `DBG:AD2:*`) |
| E1 | main-loop AD2 logger | done (7) |
| E2 | SPI2 back end | done (8) |
| E3 / E4 | the bus hand-off and the stream ISR | done (9, replaced by 11) |
| E5 / E6 | the wave source and UI | done (10) |
| E7 | DC-coupled V row + AD1 self-cal | done (13) |

### 5.4 Risks

1. Torn reads: none on the device (1.4).
2. The ISR policy: one ISR, one SPSC ring, the bus handed over exclusively
   (9.3); a build without the `ad2-isr` feature has no interrupt.
3. SPI2 across the isolator: /16 clean; /8 needs the CKPH = 1 data phase
   (11.3).
4. The DC V row on AD2: works after the AD1 self-cal (13.4); AD1 shifts while
   AD2 converts (13.1, 13.2), so the DMM's DC V row keeps AD2 off.
5. CPU share: 17.5 % at 38.4 kS/s with DMA frames, ~46 % at 76.8 kS/s
   (11.10, 11.12).
6. The DMM poll during a stream: paused (3.4).

## 6. Device experiment plan (one session, before any UI work)

All through `DBG:` SCPI and `tools/dm40scpi.py`. Each step and where its
result is:

1. **SPI2 pins and speed** (`DBG:HYSPI:SELF?`): reads clean at /16 and /32,
   /8 only with the CKPH = 1 data phase; writes needed a fix (8.5, 8.6, 11.3,
   11.6, 11.7).
2. **Torn reads** (`DBG:AD2:TORN?`): none at any rung (1.4; device check
   "f80524f" 1).
3. **Loss per rate** (the logger with `DBG:AD2:DRAW 0|1|2`): the main-loop
   sampler loses on every redraw (2.3); the stream ISR 0 (11.6-11.11).
4. **Noise / ENOB** (`DBG:AD2:NOISE?`): 1.2.
5. **DC-coupled V row** (`DBG:AD2:DCV?`): the row works; AD1 shifts while AD2
   converts (13.1).
6. **Waveform sanity**: AC V and AC mV waveforms on f80524f (device check
   "f80524f" 1); the wave view on d8e249d and a239aa6 (10.6, 11.11). The
   current waveform with a PWM load is not run (TODO "(A) device test of
   the Wave view").

## 7. (B) implemented: main-loop AD2 logger + the E0 hooks (2026-10-21)

E0 (the DBG hooks of section 6) and E1 (the AD2 logger sampled from the main
loop). Since 42cbf35 the default build streams on the timer ISR (sections 9,
11) with the same ring, counters and host side; the main-loop sampler is what
a build without the `ad2-isr` feature uses (`--no-default-features
--features vdisk`).

### 7.1 What exists

| piece | where | what |
|---|---|---|
| rate ladder | `hy3131::ad2_r26_for`, `ad2_period_ns` | requested S/s -> R26 low bits: 1200 = /4 OSR 1024, 2400 .. 76 800 = /2 OSR 1024 .. 32; ENCHOPAD2 set with `LOG:CHOP ON` (the default), the same rate either way (1.1). Nearest rung in log scale; the achieved rung is reported, never silently another rate. The stream's read divider caps the rung (`stream::max_rate_hz`, 11.3). Tested against the 1.1 table (meter-test). |
| stream setup | `hy3131::set_ad2_stream` | after the full stock row (dmm.start): ENAD1 cleared if set (the misconnect sentinel runs AD1 in AC V / mV; off first, so note 7 cannot apply), R34 = 0 (AD3 off: the row starts it before AD2, so with both on AD2F would come twice per period on two skewed grids), R29 = 0 (RMS / LPF off), R26 = ENAD2 \| rung, INTF cleared. ENAD2 never toggled. LOG:STATe OFF rewrites the row, sentinel included. |
| rows | `Logger::mode_ok` | AD2: AC mV (2) and AC A / mA / uA (4..6) **DC-coupled** (OP2O / OP1O, stock rows unchanged), AC V (0) **AC-coupled** (PB4, ~0.5 Hz; preamble `coupling` = 1), DC V (1) and DC mV (3) since E7 (section 13): the DC-coupled V row / the AC mV row. AC+DC, AUTO coupling and the power meter refused (alternating rows). |
| sampling (main-loop sampler) | `Logger::poll` | AD2F-gated (INTF read, then R03..R05), P/2 gate from the last sample, timestamp at the flag observation; a call keeps sampling while the next result is due within `AD2_BURST_US` = 500 us of the call. A read whose bits 23:19 are not a copy of bit 18 is dropped and counted (`bad`, a torn read for sure). |
| honesty | LOG:COUNt?, LOG:GAPs?, LOG:PREamble?, panel | main-loop sampler: `lost` = round(span / nominal period) + 1 - captured (counts the steady "slightly late" losses of a rate the link cannot sustain, which the per-gap `missed` cannot); stream ISR: `lost` = A entries - captured (11.2). Gap histogram in periods (2, 3-4, 5-16, 17-128, >128) + max gap; preamble `...,source,req_hz,nominal_mhz,r26,coupling` (requested vs chip rate; measured_hz in field 5); the on-screen panel shows "AD2 measured/nominal Sa/s", samples and "lost N (x.x%)". |
| ring | `logger.rs` `RING`, `CAP` | 2048 records of 8 B (seq implicit, 9ce523d), 16 KiB of SRAM: 213 ms at 9600/s, so the host must drain `LOG:DATA?` (511 records per block) at least ~5x/s, else `overwritten` counts the loss. Single producer / single consumer since 9.1. Wire format: 12-byte {seq, ts_us, raw} rows, or the compact formats ([compact-log-format.md](compact-log-format.md)). |
| hooks (E0) | [`src/ad2exp.rs`](../src/ad2exp.rs), `DBG:AD2:*` | `TORN? r26,n[,SYNC]`: n back-to-back pairs of R03..R05 reads, INTF after each pair; same / differ-with-new-conversion / differ-without / signbad + a histogram of bit_length(\|a-b\|) (tears show as a bump at 9 or 17 above the noise). `NOISE? r26,n[,op1chop]`: n flag-synced samples: mean, sd (1/1000 code), min, max, readout us; ENOB = log2(2^19/sd) on the host. `DRAW 0\|1\|2`: during a log session no panel / the panel / panel + a full DMM-view repaint every 100 ms (the DMM page's redraw load). Bounded (n <= 2000 per call, watchdog kicked), refused while logging or outside an AC row (no reply: `dm40scpi.py ad2` checks FUNC? first), the row is rewritten afterwards. |
| host | `tools/dm40scpi.py` | `log out.csv --source ad2 --rate N [--chop off] -d S`: CSV `seq,t_s,raw,value_<unit>,lost_before` (scaled with `LOG:SCALe?`, lost_before from the timestamps and the seq), trailer with measured rate, counts and gaps; `ad2 torn\|noise [--sync] [--op1chop k] [--rungs ...]` sweeps the rungs. `--usbip BUSID` talks to the emulator. |
| emulator | `emu/hy3131/HY3131.cs` (peripherals/ symlink), tests | AD2 on its own grid (one AD2F and one fresh result per period), per-result noise model. `emu/hy3131/tests`: `Ad2Stream` (960 conversions in 100 ms at 9600/s, one flag each, code of 12.3 mA in AC mA r0, noise model). |

### 7.2 Emulator results (plumbing only, not the device)

`emu/run.py fwrs --fast` (50 MIPS: a quarter of the device's 200 MHz, so the
bit-banged readout costs ~4x the device's) with `isrc:0.5,ac=0.3,f=50`, AC A
(the meter picked mA r1), 15-20 s per run, USB/IP draining LOG:DATA?. The
rungs are the 2026-10-21 labels (the chop-ON rate error of 1.1 was in the
model too):

| requested | chip | measured | lost | gaps (2 / 3-4 / 5-16 / 17-128 / >128), max |
|---|---|---|---|---|
| 600 | 600.0 | 593 | 1.1 % | 0 / 51 / 0 / 0 / 0, 6.5 ms |
| 2400 | 2400.0 | 2030 | 15.4 % | 5616 / 7 / 96 / 0 / 0, 6.1 ms |
| 9600, panel (DRAW 1) | 9600.0 | 2011 | 79 % | 0 / 10913 / 19660 / 48 / 0, 6.0 ms |
| 9600, no panel (DRAW 0) | 9600.0 | 2055 | 79 % | 0 / 11514 / 19493 / 0 / 0, 1.1 ms |
| 9600, DMM-view repaint every 100 ms (DRAW 2) | 9600.0 | 685 | 93 % | 0 / 2182 / 3095 / 333 / 83, 47 ms |

`ad2 noise` (DC 50 mA, the emulator's noise model): sd 31.5 codes at 76.8
kS/s (ENOB 14.0) .. 8.3 at the slowest rung (16.0); `ad2 torn --sync`:
signbad 0 (the model latches by construction). Readout 426 us per
flag-synced sample at 50 MIPS. The AD2 path, the ladder, the counters and the
host CSV work end to end; the rates are the emulator's.

### 7.3 Device session (f80524f)

The 30-minute plan (torn reads, loss per rate with DRAW 0 / 1 / 2, noise,
waveforms, profile) ran on f80524f with the bit-banged bus; the results are
in the device check, "f80524f" 1:
* no torn reads at any rung (1.4);
* the chop-ON rungs ran at the chop-OFF rate (1.1; the ladder was fixed);
* the bit-banged link topped out at ~6.2-6.6 kS/s; the DMM page repaint
  (DRAW 2) cost a ~26 ms gap every 100 ms at every rate;
* the converter floor 13.4 ENOB at 76.8 kS/s, 16-17.6 at the slow rungs
  (1.2);
* AC V and AC mV waveforms of the 40 Hz square and of a DC level as
  expected, uncalibrated (-1.2..-1.7 %).

## 8. E2 implemented: the HY3131 on the hardware SPI2 (2026-10-22)

There is no runtime switch between bit-bang and SPI2: SPI2 is the bus. Reads
moved in 0d0fd76 / d150919 (device check passed, 8.6), writes in 42cbf35
(11.7). The bit-banged bus and its build feature `hyspi-bitbang` are gone.

### 8.1 What exists

| piece | where | what |
|---|---|---|
| bus | [`src/hy3131.rs`](../src/hy3131.rs) (`init_pins`, `spi::read` / `write`) | SPI2 @ 0x4000_3C00, PCLK1 50 MHz / 16 = **3.125 MHz**, master, mode 0 (CKPL = CKPH = 0), MSB first, 8 bit, software NSS; PC10 / PC11 / PC12 AF6 (2.2), OSPEED 25 MHz on SCK / MOSI; CS = PD2 GPIO. `read` / `write` are used by the DMM, the logger, ad2exp, the sentinel and the stream ISR's CPU frames. Polled byte exchange (TBE, DATA, RBNE), TRANS idle before CS goes high. The stream's own variants: `read_fast` (pipelined), the CKPH = 1 data phase at /8 (11.3), DMA frames (11.8) and hybrid frames (11.12). |
| read period | `spi::read` | command byte on SPI2; wait TRANS = 0 (SCK low); PC10 MODER AF -> output (ODR bit = 0, the SPI idle level: no edge), one pulse (~100 ns high, ~100 ns low), back to AF; then the n data bytes (MOSI = 1). The datasheet frame exactly (8 + 1 + 8n clocks), so the register after the frame sees what it saw before (its MSB fetched at the last falling edge). The "shift" framing (cmd + n + 1 bytes, RX shifted one bit) is used only by the DMA frames of R03..R05 (11.8): after other registers it clocks bits of reg + n, which after the counter frame (R15..R1D) is R1E = INTF, clear-on-read. |
| pin hand-over | `init_pins` | CS high first, SCK low as GPIO, SPI2 configured and enabled (drives SCK low), then AFRH, then MODER: SCK never moves while CS is high, CS never glitches (and the chip ignores CK while CS is high, DS 7.2 (6)). Only at init. |
| CS timing | `spi::{begin,end}` | CS low -> first edge ~200 ns, last edge -> CS high ~100 ns, CS high >= ~500 ns between frames (the datasheet gives no minimum). |
| self-test | [`src/hyspi.rs`](../src/hyspi.rs), `DBG:HYSPI:SELF? n[,div[,cpha]]` | in the current row (run it in an AC row: AD2 on): reference = 8 reads of R1F..R35 (INTE + the row: the HY3131 has no ID / version register), bytes that differ between them are live and masked; n x {read1 of a stable byte, read(R00,6) AD2 sign copy, read(R1F,23) = ref, write(R36, pattern) + read1(R36) = pattern}; reply `div,n,bad_r1,bad_r23,bad_w1,signbad,us_r1,us_r6,us_r23,us_w1,stable,ref`. `div` 8 / 16 / 32 runs that call at 6.25 / 3.125 / 1.5625 MHz, `cpha` 1 the data phase at CKPH = 1. The write-pattern check is absolute (a consistently shifted bus fails it even though the reference would shift too). DMM paused and its row rewritten afterwards, refused while logging, n <= 2000. `DBG:HYSPI:WR? mode[,div]` is the write experiment of 11.6. |
| emulator | `emu/hy3131/HY3131.cs` (symlinked as `emu/peripherals/HY3131.cs`), `emu/dm40.repl` | `HY3131SpiPort` (ISPIPeripheral) on Renode's STM32SPI `spi3` @ 0x40003C00 hands each byte to `HY3131.SpiByte`, which runs the bit engine for 8 mode-0 clocks (sample DO, rising, falling); the GPIO pulse on PC10 lands in the same engine. Host tests (`emu/hy3131/tests`, "Hardware SPI byte path"): R1F..R35, R36 and live R00..R05 equal on bit-bang / SPI-pulse / SPI-shift, 256 SPI writes read back, a 22-byte row written over SPI, INTF side effects of the frame tails, a byte with CS high is ignored. Timed SPI bytes since 11.12. |

### 8.2 Timing budget (why /16)

Mode 0: the master changes MOSI and the chip changes DO after the falling
edge; both sample at the rising edge, half a period later. The SPI crosses
a CA-IS3741 isolator (~10-15 ns per direction, INFERRED, not from its
datasheet).

| | /8 = 6.25 MHz (80 ns half) | **/16 = 3.125 MHz (160 ns)** | /32 = 1.5625 MHz (320 ns) |
|---|---|---|---|
| DO path: SCK out through the isolator + TDODL <= 60 ns + back = ~80-90 ns, vs the half period | **-0 .. -10 ns** (fails or marginal) | **+70 ns** | +230 ns |
| DI setup TDISET >= 60 ns (+ channel skew, few ns) | +15 ns | +95 ns | +255 ns |
| TCKLO >= 60 / TCKHI >= 20 ns | ok | ok | ok |
| byte | 1.28 us | 2.56 us | 5.12 us |

So /16 is the bus. /8 in mode 0 shows read errors on the device (11.3); /8
works with the data phase sampled at CKPH = 1 (11.3), which the stream uses.

### 8.3 Emulator results (plumbing only, not the device)

`emu/run.py fwrs --fast` (50 MIPS), DC V 1.2 V / AC V 0.5 V 50 Hz, the SPI2
build against the bit-banged build of the same tree: readings equal, SELF?
200 clean at /8, /16 and /32, the AD2 logger at 9600 S/s 8.3 % lost (bit-bang
79 %). The emulator's SPI bytes were instantaneous then, so its us per frame
were CPU overhead, not bus time.

### 8.4 Device check (10 minutes)

The plan (READ?, `DBG:HYSPI:SELF?` at /16, /32, /8, the DMM functions, the
logger, `DBG:PROF?`) ran on 0d0fd76: 8.5.

### 8.5 Device result of 0d0fd76 and the DIAG image (2026-10-22)

Calibrator 1.000 V DC on V/COM. SPI2 build: DC V READ? = -0.0002 V (wrong);
`SELF? 2000` -> `16,2000,0,1024,508,0,7.71,21.38,68.33,7.26`; `,32` ->
`32,2000,0,965,505,0,12.57,...`; `,8` -> `8,2000,65,1745,626,0,5.31,...`.
Bit-bang build of the same commit: DC V 0.99978 V; `SELF? 500` ->
`0,500,0,257,0,0,51.87,144.00,457.66,58.72`.

Reading it:
- r1 (1-byte read of a constant row byte, pulse framing) is clean at /16 and
  /32: mode 0, AF6, the GPIO read-period pulse and the data-phase sampling
  work. /8: 65 errors = the DO-path margin of 8.2, as predicted.
- us_r1 7.71 at /16 = 16 clocks x 320 ns + ~2.6 us, 12.57 at /32 = 16 x 640
  + ~2.3 us: APB1 is 50 MHz and the PSC coding is right.
- w1 (write R36, read back) fails ~25 % at /16 AND /32: independent of the
  bit period. DC V wrong fits: single-byte config writes lost one time in
  four. The cause is in 8.6.
- r23 was unusable as evidence: 51 % "bad" on the bit-banged bus too (the
  reference was one read; bytes of the row change between reads). SELF? now
  takes 8 reads and masks live bytes (8.1).

A DIAG image (6d35c53, removed since) made every bus parameter a runtime
setting and added a three-way write / read-back test (`DBG:HYSPI:RAW?`) for
8.6.

### 8.6 Device bring-up with the DIAG image 6d35c53, and the fix (2026-10-22)

Run over USBTMC on the device, 1.000 V DC calibrator on V/COM.
`DIAG?`: spin(1000) = 5017 cycles, i.e. one spin = 25 ns at 200 MHz.

`RAW?` with 0d0fd76's parameters: `A=064,B=000,C=064`. Every mismatch is
`App>(pp|1)`: exactly the 64 values with bit 7 = 1 and bit 0 = 0, written
over SPI2, arrive with bit 0 = 1 (A: SPI2 write / bit-bang read; C: SPI2
both); B (bit-bang write, SPI2 read) is clean for all 256 values. The same
64 values, unchanged, with: CS setup / hold / idle 8/40/40, 40/4/20 and
40/200/200 spins (0.2 .. 5 us); pin speed OSPEED 0 (2 MHz) and 3 (200 MHz);
/32; the shift framing. CPHA = 1 wrote nothing (`A=255`, READ? overload).
SELF? (masked reference, 23 stable bytes): reads clean at /8, /16 and /32
(bad_r1 = bad_r23 = signbad = 0 in 2000 iterations; us_r1 5.6 / 8.0 / 12.8,
us_r6 13.4 / 21.8 / 38.6, us_r23 40.4 / 69.2 / 126.8), w1 501/2000 bad at
every divider (= the 25 % of patterns with bit 7 = 1, bit 0 = 0). The
SPI2-written row read back as 0091..9128A181.. against the bit-banged
0090..9028A080..: interior bytes of a multi-byte write are hit too (every
transfer ends after its byte, the next one is loaded on RBNE).

Conclusion: the chip takes a data byte's LSB after the GD32 has stopped
holding it on MOSI (the peripheral drives the byte's MSB level there once the
byte is out; a 1 wins). Command bytes always have bit 7 = 0, so reads are
not affected. Not a timing margin: no divider, CS time or edge rate changes
it.

Fix on d150919: reads on SPI2 (/16, pulse framing), writes bit-banged with
the three pins switched to GPIO for the frame. Device check of d150919 (TODO, 2026-10-22):
DC V 0.99979..0.99990 V on the 1 V calibrator, `SELF? 2000` at /16 and /32
clean, the main-loop AD2 logger 8.0 % lost at 4800 S/s and 11.1 % at 9600.
Writes moved to SPI2 in 42cbf35 with a dummy byte whose MSB equals the data's
LSB (11.6, 11.7).

## 9. E3/E4 implemented: the AD2 stream on one timer ISR (2026-10-22)

Decision #3 of [decisions-2026-10-22.md](../../docs/history/decisions-2026-10-22.md):
one timer ISR owns the HY3131 bus while an AD2 session runs, behind the Cargo
feature `ad2-isr` (in the default features since 42cbf35; a build without it
has no interrupt at all). Section 9's first read scheme (a tick at 4 x the
rate, a 7/8 P gate, an INTF read per sample) lost conversions on the device
(9.5) and was replaced by the phase-locked blind reads of 11.2; the hand-off,
the guard, the ring and the preemption rules below still hold.

### 9.1 What exists

| piece | where | what |
|---|---|---|
| vector | [`src/shim.rs`](../src/shim.rs) | the table runs to IRQ 55 (72 words); IRQ 55 = the stream ISR's entry (`ad2_irq_entry`, which passes the interrupted PC for `lat_pc`) with the feature, `default_handler` without (cfg'd in the asm, no weak symbol: LTO rejects a weak + strong pair in one module). |
| timer | `hy3131::stream` | TIMER6 (STM32 TIM7, 0x4000_1400, APB1EN bit 5; free: TIM1/2/3/5/8 scope + hwtrig, TIM6/12 generator, TIM13/14 backlight / buzzer). 200 MHz kernel, PSC 3 = 50 MHz count; the ISR writes ARR per interval (11.2). |
| hand-off | `hy3131::stream::start` / `stop` | start (main loop, after the session's last bus access `set_ad2_stream`): TIM7 set up, SysTick off + PENDSTCLR / PENDSVCLR, ICER and ICPR on all 8 NVIC words (whatever the FS-loader left), IPR[55] = 0, `HANDED` = true, ISER bit 55, `cpsie i`, CEN. stop (main loop): CEN / UIE off, ICER bit 55, `cpsid i`, SR and ICPR bit 55 cleared, `HANDED` = false; the read divider back to /16 mode 0 for the DMM. PRIMASK is cleared nowhere else. Nothing to wait for on one core: the main loop runs only between ISR entries. |
| guard | `hy3131::read` / `write` / `set_div` | every main-loop bus call checks `HANDED` and, if set, stops the stream BEFORE its frame: a main-loop frame can never interleave with the ISR's. One main-loop path legitimately gets here (a function / range change while logging: touch, key, CONF -> dmm.start), so it is a runtime guard in every build. `forced_stops` counts (DBG:ISR? `forced`), `Logger::poll` ends the session and main.rs reports "Settings conflict". |
| ring | `logger::RING` | 2048 x 8 B (+ counters) in `.dmabuf`. Single producer (the ISR, or the main loop for the main-loop samplers), single consumer (LOG:DATA?, the disk writer or the wave view): `head` / `tail` u32 atomics, Release / Acquire; every counter has one writer; the 64-bit span is two words read hi-lo-hi. Full ring: the NEWEST sample is dropped (the producer must not move the consumer's tail), `overwritten` counts it, the seqs stay consecutive and the host sees a timestamp gap (before 9.1: oldest overwritten, a seq gap). |
| ISR | `logger::isr` | the A / B / C entries of 11.2 / 11.8 / 11.12. Status waits capped (`spi::wait`, 2000 polls ~100 us; `timeouts`). No register write, no dmm.rs, no LCD / flash / watchdog, no float, no panic path. |
| counters | `DBG:ISR?` | the field list in 11.2 ([`scpi.rs`](../src/scpi.rs) has the current one). LOG:COUNt? / GAPs? / PREamble? unchanged. |
| default rate | `logger::DEFAULT_AD2_HZ` | 4800 S/s with the feature (2400 without). |
| stack, memory | `tools/stack-depth/stackdepth.py`, build.sh | the ISR runs on the MSP on top of any main-loop frame: worst = main path + ISR path + exception frame (26 words with the FPU's lazy-stacking space + 4 alignment = 108 B). The tool follows IT-block calls (`bllo`) and conditional tail calls. The budgets: [native-boot.md](native-boot.md) "Memory budget". |
| emulator | `emu/dm40.repl`, `emu/hy3131/ad2streamtest.py` | timer7 is a `GD32_Timer` (IRQ -> nvic@55) instead of Renode's STM32_Timer; the ISR's SPI2 frames go through `HY3131SpiPort` as the main loop's. The test boots the default build and checks logger sessions plus a function change mid-stream. |

### 9.2 Tick rate and stamps (deviation from "~1.05 x")

The record stamp is the observation time (decision #2: "when the firmware
saw it"). A tick at ~1.05 x the rate (the study's plan) makes the stamp lag
walk and wrap every ~21 samples: a 2-period stamp step that looks like a
miss (emulator: 5 % false gaps). Section 9 therefore ticked at 4 x the rate
and read one period after the last read (a 7/8 P gate). That still read
INTF on every sample, which lost conversions to the INTF read-clear race
(11.1). Since 11.2 the stamp is the DWT time of the A entry, one per period,
and the stamps are as regular as the two crystals plus the entry latency.

### 9.3 Preemption and shared state

- **One interrupt.** NVIC: every line ICER'd and its pending bit cleared at
  each start, only IRQ 55 enabled; SysTick (SYST_CSR = 0) and PendSV can not
  fire; faults go to `default_handler` (`b .`) as before: the FWDGT resets.
  The ISR never kicks the watchdog: a main loop that stops kicking still
  resets.
- **What masks the ISR.** Only the ~20 instructions of `stream::stop`.
  critical-section is implemented with compiler fences only (main.rs
  `NoMask`, 11.4): the synopsys-usb-otg endpoint copies no longer mask it.
  Measured entry latency on the device: 0.24-0.28 us (11.9-11.11), as long as
  no code or data above 0x08080000 is read during a stream (11.4).
- **What the ISR preempts.** The main loop anywhere: LCD (FSMC), USB polling,
  spin-delays (stretched by up to one ISR entry; all tolerate it), BT UART
  polling. DMA users (BT UART4, generator DAC / TIM6 and PWM / TIM12, scope
  DMA1 + TIM1/5/8) share no register with SPI2 / TIM7 / PC10..12 and keep
  running (11.11 sharing matrix).
- **GPIOC MODER.** The ISR's read-period pulse is a read-modify-write of
  GPIOC MODER (PC10 AF -> output -> AF). If it lands inside a main-loop RMW of
  the same register, the main loop writes back the value it read, which has
  PC10 = AF: the ISR restored exactly that, so nothing is lost. The main loop
  never changes PC10..12 while handed off (only `hy3131::write` does, and the
  guard stops the stream first).
- **INTF.** Reading INTF clears AD1F / RMSF / LPFF too: harmless, the DMM
  poll is paused while streaming and restarts the row after.
- **Shared state** = `RING` (atomics + slots published by `head`) and
  `HANDED`; the ISR's clock / stats state is written by the main loop only
  before the hand-off.

### 9.4 Emulator results (plumbing only, not the device)

`emu/run.py fwrs --mips 200`, AC V 0.5 V 50 Hz, LOG:DATA? drained: the
main-loop sampler lost 23 % at 4800 S/s with DRAW 2 and 0.4-1.2 % with DRAW 1;
the 4 x tick lost 0 at 1200 / 4800 / 9600 S/s with DRAW 1 / 2, and a
CONF:VOLT:DC mid-stream stopped the stream first (forced + 1, "Settings
conflict"). The emulator's INTF read had no race then, so it could not show
the device's loss (9.5, 11.5).

### 9.5 Device check (10 minutes)

Run on c50425f (device check "c50425f (+ad2-isr)" 1): **FAIL**, 1.4 % lost at
4800 S/s and 2.6 % at 9600, in ~11.5 ms bursts every ~3948 conversions
(11.1), and max_lat ~20 us whenever the logger panel drew (11.4). The DMM
after the streams, SELF? and the mid-stream function change passed. Fixed in
section 11.

## 10. (A) implemented: the DMM wave view (E5 / E6, 2026-10-22)

The stock-replica scope page with a second source: the HY3131 AD2 stream of
the DMM's row, i.e. a slow, floating, isolated scope on the DMM jacks (V, mV,
A, mA, uA). Decision #3 (one timer ISR, exclusive bus hand-off). In the
default build; without the `ad2-isr` feature the entries are not drawn and
the code is dead ([`app_wave.rs`](../src/app_wave.rs) `WAVE_BUILD`). User-facing
behaviour: [docs/manual/README.md](../../docs/manual/README.md).

### 10.1 What exists

| piece | where | what |
|---|---|---|
| entry | `ui::wave_button` (MF range page, V / A: "Wave"), the scope page's source badge "MCX \| DMM" (`scope_ui::src_badge`, left of the record bar, which is shorter then) | DC V / DC mV stream their DC rows (13.3); from DC A the AC A row (DC-coupled) is selected first (toast "Wave: AC A row (DC-coupled)"); refused (toast) in AC+DC / AUTO coupling, the power meter, ohm / cap / ..., during a CSV recording; a host LOG session closes it ("Data logger running") |
| exit | the badge, MENU, any view change (`App::show`), a host scope setting that is MCX-only (`CHAN1:SCAL`, `CHAN1:COUP`, `DBG:HWTRIG`) | the view change waits for the stream's stop (`App::wave_exit_done`, dda802c: no region C code under the stream); MCX settings restored (`Scope::mcx_settings` is what is persisted), the DMM autorange restored (AUTO / AUTO+), the DMM row rewritten by main.rs (the logger's ON -> OFF edge: `dmm.start`) |
| ownership | `app_wave.rs` `WaveReq` -> main.rs, `Logger::start_wave` | the stream is a logger session flagged `wave` (the host's LOG:SOURce / SRATe kept; LOG:STATe ON refused "Settings conflict", LOG:STATe? 0, LOG:DATA? an empty block). main.rs `busy` (logger active: no DMM poll, no BT serve / AT init) vs `logging` (a host session: GUI frozen, no touch, no scope poll). A range or coupling step is stop -> `dmm.set_range` / `set_sub` -> start in one pass (the new scale's repaint before the restart), so the bus guard never fires for it; anything else that touches the bus (a host CONF, the locked power key's sub-mode cycle) stops the stream (forced) and the view closes ("Wave stopped: DMM changed") |
| record | `scope.rs` `WaveSource` | every poll drains the ring (`logger::wave::drain`) into BUF as a continuous circular record of 10-bit codes (`trigsoft::ad2_code10`: the full AD2 window +-1.25 V / IG over 0..1023); frames are linear windows (BUF rotated so the oldest sample is at 0), so the renderer, measurements, cursors, STOP / zoom / pan, Save CSV and FFT work unchanged: the MCX record buffer and roll columns are reused (the MCX acquisition is idle). The ring is drained again between the slow parts of a scope pass (eb5a929: 0 drops at 1 ms/div) |
| holes | `logger::wave::dropped` = ring full + ISR misses | a hole restarts the record at the newest sample (no triggered frame spans it); roll goes on with a short gap |
| timebases | `scope::WAVE_TB_MIN` = 15, `wave_fs_for` | 1 ms/div .. 50 s/div, the bands of 3.1; a band change restarts the stream (~1 pass). Roll from 200 ms/div as the MCX scope (per-column min / max accumulated while draining, so 50 s/div = 600 k samples needs no buffer); YT up to 1 s/div (12 000 samples at 1200 S/s). The 76 800 band needs DMA frames: with CPU frames (`DBG:AD2:DMA 0`) the view closes ("Wave: rate needs DMA frames") |
| vertical | `dmm::ad2_scale_of`, `scope::WaveScale`, `scope::WAVE_STEPS_V` / `_A` | SI = the DMM's own LPF formula with its calibration gain / offset (3.3), the E7 self-cal for DC V; the V/div is screen only (`WaveScale::vdiv`; the codes do not depend on it). Step list per function: one full-scale step per range, +-range on the 8 divisions = range / 4 (60 mV -> 15 mV/div, 600 mV -> 150 mV, 6 V -> 1.5 V, 60 V -> 15 V, 600 V -> 150 V, 1000 V -> 250 V; 600 uA -> 150 uA, 6 mA -> 1.5 mA, 60 mA -> 15 mA, 600 mA -> 150 mA, 6 A -> 1.5 A, 10 A -> 2.5 A) and the 1-2-5 steps between, each on the lowest range whose AD2 window (3.3) holds its +-4 div with 20 % to spare (2 V/div on 6 V, 200 mV/div on 600 mV, 20 mV/div on 60 mV); 10 A has its full-scale step only (2 A/div runs on 6 A). A step on the current range only rescales the screen (`Scope::wave_vdiv_set`: the acquisition re-arms, the trigger level keeps its SI value); another range's step is a `WaveReq::Range` and `wave_rescale` keeps the step's V/div. V/div + / - (panel magnifiers) and the pinch (`scope::wave_pinch_step`) step the list; the entry, AUTO (centred) and the clip autorange take the range's full-scale step. 60 mV (AC / DC mV r0) is MF item 5 (`ad2_scale_of` item). The step index is derived from the V/div shown, not persisted. Edge: the trace rows are clamped to 0..278 of the 280-row model (`scope_ui::trace_fy`, stock), +-4 div are rows -1 / 281, so the outer ~1 % (top) / ~2 % (bottom) of +-full is drawn on the frame edge (the MCX page the same) |
| coupling | the V/div badge glyph, the CH1 button / V/div badge tap | V: the coupling key switches DC (the E7 row) <-> AC (the stock AC V row, AC-coupled, ~0.5 Hz) at the same range (`WaveReq::Coupling`); A: a note that the AC A row is DC-coupled |
| labels | `Model::{src, amps, wvdiv, ch1_txt}` | CH1 button "AC V" / "AC mA", V/div "1.5V" / "200uA" (`fmt_div`: one decimal for 1.5 / 2.5), trigger badge and cells in A for the current rows ("Ip-p", "Irms": `scope_meas::format_unit`), CSV header "source DMM AC mA (AD2, isolated jacks)" and the value column's unit |
| SCPI | `App::scpi` | TIM:SCAL, TRIG:LEV / SLOP / SWE, RUN / STOP / SING, SCOP:MEAS:*?, WAV:DATA? / PRE?, CHAN1:SCAL?, CONF? (from the streamed row) act on the wave view (CHAN1:SCAL? = the step shown); the MCX-only setters close it ([docs/manual/scpi-reference.md](../../docs/manual/scpi-reference.md)) |

Deviation from 5.2 point 3 of the study ("19 bit -> u16 >> 3"): the record
holds 10-bit codes, as the MCX record, because the whole display /
measurement / trigger stack is 10-bit (histogram of 1024 codes, the +-2 code
hysteresis, `& 0x3FF` masks), and the V/div steps that need finer codes run
on a lower range (`WAVE_STEPS_V` / `_A`: the screen spans >= ~1/6 of the
range's AD2 window). Cost: 1 code = 512 AD2 codes = 0.2 % of the AD2 window
(e.g. 24.4 mV in the 6 V range, whose full-scale step shows +-6 V = 492 codes
over 280 rows), so Vrms /
Vpp of a full-scale signal are good to ~0.1-0.3 % (noise dithers the mean),
small signals (< 10 % of the range) are coarse. A 16-bit record is a
follow-up that parametrises the code width.

### 10.2 Trigger in the ISR (no dead time)

The trigger is detected **per sample in the stream ISR** (`logger::isr::trig`,
`trigsoft::Stream`): the same edge (armed beyond the +-2 code band, then
across the level), pulse-width (> / < / = within 5 %, whole samples:
`trigsoft::pulse_window`) and runt rules as the MCX software search, on the
record's 10-bit code, integers only. Host-tested against the slice functions
(tools/trig-test: the first event at or after a minimum is the same, 78
cases). The ISR reports events, the main loop picks frames:

* config words (`Ring::trg_w`, then `trg_gen`, Release; main loop) -> the ISR
  reloads its private copy and resets the detector on a new generation;
* every event's sample index into an 8-slot event ring (`trg_ev`, `trg_ev_n`
  Release; the main loop reads the 7 newest safely) and the first event at
  or after a minimum index (`trg_first_*`: Single's exact "first event after
  arming");
* Normal / Auto show the newest event whose post-trigger part is in (or wait
  for the earliest incomplete one), at most 25 frames/s; Single the first
  event after the pre-trigger history, then STOP.

Because the record is continuous (10.1) and the detector sees every sample,
there is no dead time: an isolated event (a glitch, a single pulse) is
always caught, unlike the MCX scope's search of each finished record.
Timing: the trigger point is a sample: jitter = 1 sample = 13 us .. 833 us
at 76 800 .. 1200 S/s (under one pixel at every band's timebase). Cost: a few
loads and compares per stored sample, no loop, no float; the detector
inlines into the handler's frame. Decision #3 invariant 4 is amended for
this.

The HY3131 comparator as an alternative / assist: **not used**. SCMPI
(R20 bits 7:5) can select OP1O or PB4, the AD2 nodes of the V / A rows,
but its thresholds SCMPRH / SCMPRL (R21) are fixed ladder taps (VDSC*,
AGNDP* / AGNDN*), not a programmable level, so it cannot implement a
trigger level; its outputs are CMPHO / CMPLO in CTSTA (R14) and the event /
period counters (R15..R1D, the frequency path): no edge timestamp, each
read is an SPI frame (~8 us, or the 9-byte counter frame) the ISR would add
to every tick; and R20 / R21 / the counters belong to the frequency mode
and the misconnect sentinel ([`dmm.rs`](../src/dmm.rs), [`sentinel.rs`](../src/sentinel.rs)). Events
faster than the AD2 band (~0.25 x the rate) would not be on the trace
anyway. It could count such events (a glitch counter) in a later feature,
not trigger.

### 10.3 Emulator results (plumbing only, not the device)

`emu/hy3131/wavetest.py` (200 MIPS), driven like a user (DBG:KEY MF, DBG:TAP
the Wave button / the panel / the source badge), checked over SCPI,
screenshots via DBG:LCD?:

| case | result |
|---|---|
| V/COM 1 V rms 50 Hz sine, MF -> Wave from DC V | AC V row (before E7), DBG:ISR? active, 2 V/div (6 V range); 5 / 10 / 20 / 50 / 100 ms/div: FREQ 50.00 (49.98-50.04) Hz, VRMS 1.003-1.004 V, VPP 2.836 V |
| NORM +0.5 V rising / -0.8 V falling | the WAV:DATA? frame crosses the level at the trigger column (0.505 V / -0.875 V at column 200: within a sample) on consecutive frames; SINGle: one frame then STOP |
| roll 500 ms / 5 s per div | data from the right edge, RUN chip, ROLL bar |
| V/div + / - | 6 V -> 60 V range (20 V/div): VRMS 0.995 V, VPP 2.71 V (coarser codes), back to 2 V/div |
| STOP / RUN, MENU | record kept / 50 Hz again; the ISR stops, no forced stop, READ? 1.000 V AC (DMM resumed), SYST:ERR? empty |
| source badge, then CONF:VOLT:DC mid-wave | the badge reopens the view; CONF: forced stop + 1, the view closes, DC V reads 0 |
| A-COM 10 mA DC + 20 mA rms 40 Hz square (`--case current`) | AC mA row (DC-coupled), 20 mA/div, FREQ 40.00 Hz, mean 10.01 mA = the DC A reading 10.00 mA, Ip-p 40.0 mA, cells / badges in A |

wavetest also has `--case dcv` (E7) and polls SCPI at 2 / 1 ms/div (dda802c).
Host: tools/trig-test (Stream = the slice rules), tools/ui-render (the badge,
the shortened record bar, the DMM V/div / CH1 / trigger texts no wider than
the stock texts in the same boxes, current cells; no overlaps), the stock
pixel diff (tools/ui-render/stock-diff.sh: the stock page draws no badge,
`Model::src` = SRC_NONE).

### 10.4 Limits

* The record is 10-bit (10.1).
* The DMM reading pauses while the view is open (decision #3: exclusive
  bus); the last reading is not shown on the scope page (the stock page has
  no DMM either).
* Roll above 5 s/div: STOP keeps the newest 65 520 samples (54 s at 1200
  S/s) as the stored record (zoom / CSV), not the whole screen.
* The probe glyph of the V/div badge shows x1 (no probe on the DMM jacks).
* The Vrms of signals with fast edges reads low at the slow bands (3.2).

### 10.5 Not done / next

16-bit record; the DMM reading alongside (decision #3's request queue); a
glitch counter on the HY3131 comparator.

### 10.6 Device test plan (15 minutes)

Default image. A signal generator and a DC source; the meter on USB
(`tools/dm40scpi.py`).

| min | step | how | pass |
|---|---|---|---|
| 0-2 | entry | DMM AC V, 1 V rms 50 Hz on V/COM: MF -> Wave | AC V row; T'D; 2 V/div; cells Freq 50.0 Hz, Vrms within 1 % of the DMM's AC V reading taken before; `scpi "DBG:ISR?"` active 1 |
| 2-4 | timebases | tap the corner tabs 1 ms .. 50 s/div | a trace at every step, Freq 50 Hz up to 100 ms/div wherever two periods or more are on screen, roll from 200 ms/div; no stuck frame at a band change |
| 4-6 | trigger | Normal, level drag to +1 V / -1 V, rising / falling; Single | the crossing at the T marker on every frame (+-1 sample); Single: one frame, STOP |
| 6-7 | glitch | generator: 50 Hz with a single 2 ms pulse burst (or a manual 5 V step on a 1 V sine), Normal, level above the sine | each event shown (no dead time) |
| 7-9 | range | V/div + / - | 6 -> 60 -> 600 V range and back, the trace the same in volts; 600 mV (AC mV, DC-coupled) |
| 9-11 | current | A jack, 0.5 A DC + a load switching at 10-100 Hz: A -> MF -> Wave | AC A row, DC-coupled: the mean = the DC A reading (1 %), edges of the PWM visible, cells in A |
| 11-12 | STOP / Save CSV | RUN key STOP; MF Settings Save CSV | SCOPnnn.CSV with "source DMM AC A", values in A matching the screen |
| 12-13 | exit | MENU; DMM page | the reading of the function returns within ~1 s (the row rewritten), `scpi "DBG:ISR?"` active 0, forced unchanged |
| 13-15 | conflicts | in the wave view: `scpi "CONF:VOLT:DC"`; `scpi "LOG:STATe ON"` | CONF: the view closes with "Wave stopped: DMM changed", DC V reads; LOG:STATe ON: "Settings conflict", the view stays |

**Device d8e249d** (device check "d8e249d" 8; the calibrator's 40 Hz 0..9.6 V
square instead of a sine, no A jack): entry, trigger, range, STOP / Save
CSV, exit and conflicts as the table says. Found: roll from 5 s/div moved at
the 2 s/div speed (fixed: samples per column from the timebase), the DC V
entry toast was clipped (fixed), 5 ms/div shows no Freq with two periods on
screen, a Save CSV refreshed the volume in region C under the stream
(max_lat 21.7 us, 15 overruns; fixed: the refresh waits for the stream).
**Device a239aa6** (11.11 item 3): every band 5 ms .. 1 s/div, trigger and
exit pass. The current step (9-11 min) is not run yet (TODO "(A) device test
of the Wave view").

## 11. The stream ISR phase-locked to the chip; slow flash; SPI2 writes; DMA frames (2026-10-22)

Device check of c50425f (device check "c50425f (+ad2-isr)", 9.5):
1.4 % lost at 4800 S/s and 2.6 % at 9600, in ~11.5 ms bursts every ~3948
conversions, and max_lat ~20 us whenever the logger panel drew. Both are
fixed here (device b48048e, 11.6). Section 9's design (a 4 x tick, a 7/8 P
gate, a flag read per sample) is replaced. `ad2-isr` is a default feature
since 42cbf35.

### 11.1 Why section 9 lost samples

Reading INTF clears AD2F. A conversion that completes while the INTF frame
is under way sets the flag after its bit was sampled, and the same read
clears it. The flag is lost; the data is not, because R03..R05 hold it for
a whole period. The 4 x tick ran +63 ppm against the chip (ARR rounding:
50 MHz / 19200 = 2604.17 -> 2604). Its read phase walked onto the edge
every 1 / (4 x 63 ppm) = 3950 conversions and stayed there for window /
drift = 0.72 us / 63 ppm = 11.5 ms. After an empty read every tick read
INTF, so the tick at that phase kept swallowing. In the emulator's race
model (11.5) section 9's image loses 0.6 %.

### 11.2 Design: a blind data read half way between edges (src/ad2pll.rs)

- **Entry A**, once per conversion period: R03..R05, with no flag read. The
  register holds one result for a whole period, so a read placed between
  two edges takes each conversion exactly once.
- **Where the edges are**: every 4th period there is an extra entry **B**,
  `beta` after A, at the place the edge should be. B does one INTF read,
  which clears AD2F. The next A reads INTF again, after its data frame:
  AD2F set means the edge came after B's read; clear means it came before
  (or B's read swallowed it, which is the same side). The race can only
  blur that probe; it can never cost a sample.
- **Loop**: a bang-bang, second-order loop.
  - A result moves the next period by `kp` counts. The step starts at P/64,
    halves on each sign change down to P/2048, and doubles after 8 equal
    results.
  - It also moves the period word `f` by `ki` = f / 2^18 (3.8 ppm). `f` is
    in Q16 timer counts and the fraction is carried, so the mean period is
    exact to 2^-16 counts. `f` is clamped to +-5000 ppm (+-1000 until
    2026-10-23, 11.12). The reported rate
    is `f` averaged over ~64 probes.
- **beta = (P + frame + 1 us) / 2**: the data frame sits in the middle of the
  edge-free time, with a margin of (P - frame - 1 us) / 2 on each side:
  ~97 us at 4.8 kS/s and 19 us at 19.2 kS/s (both at /16).
- **Late entries**:
  - An A whose entry latency exceeds `beta - frame - 1 us` (the frame could
    reach the next edge) skips its read. It counts as `late`, the host sees
    a 2-period stamp gap, and it counts as lost.
  - A B later than 2 us drops its probe.
  - The ISR writes ARR at the start of every interval (ARPE = 0). An
    interval whose end had already passed is stretched and the next one
    shortened by as much (`slips`), so the grid keeps its phase.
- **Start**: the main loop picks the divider and caps the rung (11.3). It
  then polls INTF for one edge (bounded: 3 periods) and starts TIM7 so the
  first A comes P - beta after that edge. The frequency word starts on the
  last session's lock. The ISR probes every period until the phase step is
  at its minimum, then every 4th.
- **Stamps** are the DWT time of A. For an ISR session, **lost**
  (LOG:COUNt?) = A entries - captured, which is exact under a crystal
  offset. The old span / nominal-period estimate reported a 100 ppm offset
  as 100 ppm "lost".
- **Ladder**: 19200 (OSR 128) and 38400 S/s (OSR 64) are new (R26 low bits 2
  and 1).
- **Wave view trigger** (10.2): unchanged, `trig` runs after each push.
- **DBG:ISR?** fields: feature, active, ticks, captured, late, probes,
  dropped, bad, timeouts, overruns, max_isr_ns, max_lat_ns, rate_mhz (the
  locked rate), forced, locked, ppm_x10, slips, max_run, edges_after_b, div,
  cpu_permille, lat_pc (the PC the worst-latency entry interrupted, passed
  in by the shim's `ad2_irq_entry`), dma.

### 11.3 Read divider, the CKPH = 1 data phase, rate cap

8.6 found SPI2 reads clean at /8, but 8.2 put the DO-path margin at
~0 ns. Device b48048e then showed /8 unreliable: a start-up check
(R20..R35 at /8 against /16) passed in two of four sessions, and those
read 61 / 100 sign-copy errors in 1.15 M / 2.36 M samples. On 42cbf35 a
plain /8 SELF? 2000 gave 193 bad single-byte reads.

**The fix is in the sampling edge, not the clock.** In mode 0 the master
samples MISO at the rising edge, half a period after the chip changed DO
(at the falling edge): 80 ns at /8, against ~80-90 ns of isolator + TDODL.
With CKPH = 1 for the data phase the master samples at the falling edge, a
full period after the change, with the hold covered by the same delays.
The command byte stays mode 0, because the chip takes DI on the rising
edge. The switch is made between the hand-made read-period pulse and the
data bytes (SPE off, CKPH, SPE on; SCK idles low in both modes) and undone
at the frame end (`hy3131 set_data_cpha`, `spi::cpha`). Device 42cbf35:
`DBG:HYSPI:SELF? 2000,8,1` clean twice (all 22 stable bytes; us_r1 5.8,
us_r6 13.9, against 8.0 / 21.6 at /16).

**Defaults** (`stream::pick_div`, at each stream start; `stop` restores
/16 mode 0 for the DMM):
- DMA frames at /16, the stream default since a239aa6 (DBG:ISR? div = 16;
  1016 is not possible: the CKPH switch inside a frame needs the CPU).
- CPU frames (`DBG:AD2:DMA 0`) at /8 + CKPH (DBG:ISR? div = 1008).
- `DBG:AD2:DIV 8|16|32[,cpha]` forces another setting.

**Rate cap** (`max_rate_hz(div, dma)`):

| frames | divider | cap |
|---|---|---|
| CPU | /16 | 19200 S/s (38400 at /16 ran the CPU at 80 % and lost the lock on the device) |
| CPU | /8 + CKPH | 38400 S/s |
| DMA | /16 | 38400 S/s |
| DMA | /8 + CKPH (hybrid, 11.12; automatic above 38400) | 76800 S/s |

The CPU frames pipeline the data bytes (`spi::read_fast`). The DMM itself
reads at /16 mode 0 (decision #8: it stays there until the request queue of
decision #3 is built).

### 11.4 The ~20 us entry latency: code and data above 0x08080000

Only the low 512 KiB of the GD32F4 flash are zero-wait
([native-boot.md](native-boot.md)). The c50425f image ended at
0x08083074, and the tail of `.text` was font data: TEXT22 at 0x080800ec,
and ASCII26 (the logger panel's font) at 0x08080f98. A read there stalls
the bus, and the interrupt cannot be taken during a stalled access. That
gives max_lat ~20 us exactly when the panel (DRAW 1) or the DMM view
(DRAW 2) drew, and 4.6 us with DRAW 0. The disassembly has no other
PRIMASK / BASEPRI use: only the synopsys-usb-otg critical sections and
`stream::stop`.

So [`link-native.x`](../link-native.x) splits the flash: region **B**
(0x08008010..0x08080000, zero-wait) holds all code and read-only data that
may run or be read while a stream does, and the linker fails when it does
not fit; region **C** (0x08080000..0x080A0000, `.cold`) holds what never
runs in a pass of a running stream (boot-only and rarely run modules, DBG
tests). The module list is in link-native.x; the budget and the data loads
(calibration record, USB volume files) in [native-boot.md](native-boot.md)
"Memory budget" and "The slow flash bank while an AD2 stream runs".
`emu/regionc.py` watches region C fetches and loads in the emulator tests.

**Rule for every change**: code that may run while a stream runs (the
main-loop pass, USB / SCPI, drawing) stays in B. A module may go to `.cold`
only if it never runs in a pass of a running stream. Example: `persist` was
in `.cold` in 00e088f and went back to B, because its crc runs on every
main-loop pass (lat_pc in persist::crc, 9..18 us, 11.9).

The remaining ~4.7 us latency (b48048e: lat_pc in
`Endpoint<Out>::read`) came from critical_section::with in
synopsys-usb-otg. opendm40 now implements critical-section with compiler
fences only (main.rs `NoMask`): the only ISR shares nothing those sections
protect.

### 11.5 Emulator: the race, a crystal offset, SPI2 with DMA

- `emu/hy3131/HY3131.cs` (symlinked as `emu/peripherals/HY3131.cs`):
  - An INTF read advances the chip to its own time. A conversion completed
    within `IntfRaceWindow` before the read is cleared unreported
    (`Swallowed`). The default is 0.7 us in the Renode peripheral
    (`IntfRaceUs`); 0 = an ideal flag.
  - `ClockPpm` scales the chip's time against the machine's.
  - `IntfPhaseHist()` (monitor) gives the INTF reads by phase after the last
    conversion.
  - Host tests ("AD2 flag race and chip clock offset"): the flag is
    swallowed at +0.3 us and reported at +0.9 us, the data is kept, and
    +-100 ppm gives 96000 x (1 +- 1e-4) conversions in 10 s.
- `emu/peripherals/GD32_Timer.cs`: an ARR write on a running counter keeps
  the count phase. Before, each write lost up to one count, a +60 ppm bias
  with this ISR.
- `emu/peripherals/GD32_SPI.cs` (spi3 = SPI2): DATA, STAT and CTL1 DMA
  requests to GD32_DMA, through a `StreamEnabled` hook for TBE as a level.
  A CPU-written byte is "in flight" until the next DATA write, a STAT read
  with RBNE clear, or a DATA read, so the cut dummy of a write (11.7) never
  reaches the chip. Timing is not modelled.
- `emu/hy3131/ad2streamtest.py --ppm P --race-us W --sessions
  rate:draw:secs[:dma],..`.

Results at 200 MIPS, race 0.7 us, AC V 0.5 V 50 Hz, LOG:DATA? drained:

| image | chip ppm | sessions | lost / stamp gaps |
|---|---|---|---|
| c50425f (section 9) | 0 | 4800 | 381 of 63526 (0.6 %) / 260 |
| b48048e | -100 / 0 / +100 | 4800 (DRAW 2), 9600, 19200, 38400, 1200 | 0 / 0 in every session; locked rate within 2 ppm of the chip (38400: within 8) |
| 42cbf35 | -100 / +100 | DMA frames 9600, 38400, 4800 (DRAW 2); CPU 19200, 9600 | 0 / 0 |

In every session late, slips, overruns and timeouts were 0. READ? read
0.5 V after each stream, and a CONF mid-stream stopped the stream first.
wavetest.py PASS (sine V and `--case current`).

Host model (`src/ad2pll.rs` tests, also run by tools/meter-test):
1200..38400 S/s x -100..+100 ppm over 200 000 periods, with no read across
an edge and no conversion skipped or taken twice. Also covered: any start
phase locks; jitter up to 3 us plus 20 us spikes (skipped, never misread);
the rate within 3 ppm after 10 000 periods; DMA frames with the C read-back
at 19.2k / 38.4k (/16) and 76.8k (a 7.2 us frame); a 3 % offset is caught
by the model's own check.

### 11.6 Device check of b48048e (2026-10-22)

AC V, 6 V range, calibrator 40 Hz square 0..9.6 V (READ? 4.767 V).
`dm40scpi.py log --source ad2`, 60 s each, LOG:DATA? drained.

| run | div | lost | stamp gaps | DBG:ISR? late / slips / overruns | max_lat | max_isr | CPU | lock |
|---|---|---|---|---|---|---|---|---|
| 4800, DRAW 2 | 16 | **0 of 288 197** | 0 | 0 / 0 / 0 | 4.66 us | 22.9 us | 8.9 % | -0.9 ppm |
| 9600, DRAW 1 | 16 | **0 of 576 092** | 0 | 0 / 0 / 0 | 4.66 us | 22.9 us | 17.7 % | -1.1 ppm |
| 19200, DRAW 1 | 8 (auto) | 61 of 1 152 203 = the 61 `bad` | 61 x 2 P | 0 / 0 / 0 | 6.08 us | 15.5 us | 23.7 % | -0.8 ppm |
| 38400, DRAW 1 | 8 (auto) | 100 = `bad`; host overwritten 337 | 100 | 0 / 0 / 0 | 5.96 us | 15.5 us | 47.6 % | -0.8 ppm |
| 19200, DRAW 2 | 16 (forced) | **0 of 1 153 647** | 0 | 0 / 0 / 0 | 5.54 us | 22.9 us | 35.6 % | -1.2 ppm |
| 38400 | 16 (forced) | overloaded: overruns 11 M, slips 2.2 M, ppm at the clamp | | | 23 us | | 80 % | lost |

- **The loss is gone.** At /16, 0 lost at every rate up to 19200 S/s; the
  only losses are the /8 sign-copy errors (`bad`).
- **The latency is gone.** max_lat dropped from 21.8 to 4.7 us with DRAW 2.
  The remaining worst case (lat_pc 0x0803d2b8) was inside
  synopsys-usb-otg `Endpoint<Out>::read`; it is fixed in 42cbf35 (11.4).
- **The chip's crystal** is -0.8..-1.2 ppm against the MCU's.
- ***IDN? while streaming** (undrained): p50 0.33 / 0.33 / 0.44 ms and max
  0.58 / 0.73 / 0.63 ms at 4800 / 9600 / 19200.
- **After the streams**: READ? 4.768 V, and DBG:HYSPI:SELF? 2000 had every
  error count at 0.
- **DBG:HYSPI:WR?** (256 single writes of R36 + 256 two-register writes of
  R35..R36, bad counts):

  | mode | /8 | /16 | /32 |
  |---|---|---|---|
  | 0 bit-bang | - | 0 / 0 | - |
  | 3 byte by byte (0d0fd76) | - | 64 / 99 (1xxxxxx0 -> 1) | 65 / 65 |
  | 1 frame per register, 0x00 dummy cut | 0 / 2 | 0..1 / 0..6 (LSB 1 -> 0), 9 runs | 0 / 0, 9 runs |
  | 2 continuous, 0x00 dummy cut | 0 / 65 | 0 / 64 | 0 / 64 |

  The chip takes each byte's LSB late and sees the MOSI level that follows
  it: after a gap, the GD32 drives the same byte's MSB again; in a
  continuous frame, the next byte's MSB; with a cut dummy, the dummy's MSB.
  At /32 that is late enough not to matter.
  42cbf35: one frame per register with the dummy's MSB = the LSB (0x00 /
  0xFF) is the default write (11.7).

### 11.7 SPI2 writes (42cbf35)

`hy3131::write` writes over SPI2, one frame per register: [cmd, data,
dummy]. The dummy is 0xFF when the data's bit 0 is 1 and 0x00 otherwise.
CS goes high at the data byte's RBNE, with the dummy's first bit in
flight. The chip drops the incomplete byte; a completed one would write
reg + 1.

The cost is ~10 us per register at /16, against ~57 us bit-banged. The
bit-banged write fallback (`DBG:HYSPI:WMODE 0` in 42cbf35) was removed after
the device check of 11.9. `DBG:HYSPI:WR? mode[,div]` is the test.

### 11.8 DMA frames (42cbf35)

**Resources**:
- DMA0 (STM32 DMA1): channel 0 = SPI2 RX, channel 7 = SPI2 TX, both
  peripheral request 0 (the STM32F4 map: SPI3_RX S0/C0, SPI3_TX S7/C0).
  bt.rs uses channels 2 and 4 (UART4), gen.rs 5 (DAC); DMA1 3 / 4 belong
  to the scope.
- Buffers in SRAM (`.sram_bss`), because DMA0 cannot reach the TCM.

**The frame** uses the shift framing: the command (R03, read) + 4 x 0xFF.
The 9th clock is the read period, so the data arrive one bit late:
`decode_shift` takes bits 30..7 of bytes 1..4. A starts the frame (CS low,
both channels enabled) and returns. The next entry checks the RX count and
idle, raises CS, decodes, and stores with A's stamp. The probe read-back
is a separate entry C, `c_off` after the frame (`Pll::late_read`), because
the bus is busy at A. Probes run every 8th period, and samples are taken
only once locked: the edge search's error is ~ an INTF frame.

**Selection**: `DBG:AD2:DMA 0|1` = CPU frames / DMA frames (the default since
a239aa6, decision 2026-10-22 #7, device check 11.11). The
CPU frames at /8 + CKPH already reach 38.4 kS/s at ~14 % CPU (11.9). DMA
frames at /16 take ~1 us of CPU per sample; the B / C probe reads are
2-byte DMA frames too (AD2F = bit 0 of reply byte 1). A CPU INTF frame,
~8 us at /16, overran the B -> A interval at 38.4 kS/s on 42cbf35: the
lock was lost (11.9).

**Why not a CPU-free chain** (timer -> DMA -> GPIO / SPI):
- CS is PD2, a GPIO, not SPI2's NSS.
- The only free timer with DMA requests on DMA0 is TIM4 (UP on channel 6,
  CH2 on channel 3; CH3 conflicts with SPI2 TX on channel 7). It has no
  repetition counter, and the advanced timers with one (TIM1 / TIM8) belong
  to the scope.
- A frame of exactly 5 bytes then needs a DMA channel re-armed (CNT, then
  EN) per sample. A DMA cannot write NDTR before CR with an incrementing
  peripheral address.
- A free-running SPI with CS edges by timer DMA would need CS to switch in
  the 80..160 ns between two bytes.

So the per-sample DMA start stays in the ISR (~1 us per sample).

**76.8 kS/s** needs /8 (a 7.2 us frame); /16 (12.8 us) does not fit the
13 us period. /8 is not clean in mode 0 (11.3). The CKPH = 1 data phase
(11.3) gives the full period of setup margin, but it cannot be switched
inside a DMA frame without the CPU: the hybrid frames of 11.12 do that
switch on the CPU. The ENOB trade-off (1.2, white-noise scaling from the
13.4 ENOB measured at 76.8 k, INFERRED): ~13.9 at 38.4 k, ~14.4 at 19.2 k,
~14.9 at 9.6 k.

### 11.9 Device check of 42cbf35 (2026-10-22)

**DMM and writes.**
- With the calibrator at 1 V DC, DC V read 0.99971 / 0.99968 V.
- `SELF? 2000`: every count 0, us_w1 9.87, against 57.5 for the bit-banged
  write.
- `WR? 1` (frame per register, matched dummy) gave 0 / 0 at /8, /16 and /32,
  and 0 / 0 in 7 more runs at /16. `WR? 2` (continuous) gave 0 / 64 as
  expected.
- Ohm, AC V and FREQ answered (no input for them).

**Streams**: AC V, 40 Hz square, 60 s each, `dm40scpi.py log`:

| run | frames | lost | stamp gaps | late / slips / overruns / timeouts | max_lat | max_isr | CPU |
|---|---|---|---|---|---|---|---|
| 4800, DRAW 2 | CPU /16 | 0 of 288 200 | 0 | 0 / 0 / 0 / 0 | **0.24 us** | 23.5 us | 9.1 % |
| 9600 | CPU /16 | 0 of 576 093 | 0 | 0 | 0.24 us | 23.4 us | 18.2 % |
| 19200, DRAW 2 | CPU /16 | 0 of 1 153 197 | 0 | 0 | 0.24 us | 23.4 us | 36.4 % |
| 9600 | DMA /16 | 0 of 575 866 | 0 | 0 | 0.24 us | 10.8 us | **5.1 %** |
| 19200 | CPU /8 + CKPH | 0 of 1 152 209 | 0 | 0 | 0.24 us | 16.8 us | 25.6 % |
| 38400 | CPU /8 + CKPH | 35.8 M captured in ~15 min, bad 0, timeouts 0, late 0 (ring overwritten: the host CSV writer cannot keep up at 38.4 k) | | 0 / 0 / 0 / 0 | 0.24 us | 16.8 us | 14.3 % |
| 38400 | DMA /16 | FAIL: the lock was lost (CPU INTF probe frames overran the B -> A interval) | | | | | |

- max_lat fell from 4.7 to 0.24 us with critical-section unmasked.
- The 38.4 k DMA failure is fixed in 7ed5b31 (probes as DMA frames,
  emulator PASS); it needs a device run.
- 38.4 kS/s needs a host drain without CSV, e.g. a raw LOG:DATA? counter.

**00e088f** (raw drain `LOG:DATA?` without CSV, 60 s; calibrator square):

| run | frames | lost (LOG:COUNt?) | stamp gaps | CPU | max_lat, lat_pc |
|---|---|---|---|---|---|
| 4800, DRAW 2 | CPU /8 + CKPH | 0 of 288 193 | 0 | 6.4 % | 0.24 us |
| 19200, DRAW 2 | CPU /8 + CKPH | 0 of 1 153 105 | 0 | 25.6 % | 0.24 us |
| 38400 | CPU /8 + CKPH | 1 (`late` 1) of 2 304 885; 60 676 overwritten (the host drained ~37 kS/s) | 2573 (ring full) | 51 % | 9.3 us, persist::crc |
| 9600 | DMA /16 | 0 of 191 999 (20 s) | 0 | 3.6 % | 0.24 us |
| 19200 / 38400 | DMA /16 | FAIL: bad ~all, the lock ran to the clamp | | | 18.2 / 5.4 us, persist::crc |

Two faults:
- `persist` was in `.cold`, but its crc runs on every pass: a 9..18 us stall
  under a stream.
- A DMA frame still running at the next entry (here: an A delayed by that
  stall) was aborted. Its last byte stayed in flight, and every later frame
  read one byte shifted: bad samples and garbage probes.

Fixed after 00e088f:
- persist is back in B.
- `dma_finish` waits for the frame (bounded), ends the shift, and empties
  the receive register.
- `dma_kick` empties it too.

Emulator PASS (DMA 19200 / 38400, CPU 4800 DRAW 2, -63 ppm).

### 11.10 Device check of 0471860 (2026-10-22): passed

Calibrator 40 Hz square on V/COM, AC V. Raw `LOG:DATA?` drain (a small
Python loop, no CSV), 60 s each:

| run | frames | lost (LOG:COUNt?) | late / slips / overruns / timeouts | CPU | max_lat |
|---|---|---|---|---|---|
| 38400 | DMA /16 | **0 of 2 304 149** | 0 / 0 / 0 / 0 | **17.5 %** | 5.9 us (boot::bkp_enable, once) |
| 19200, DRAW 2 | DMA /16 | **0 of 1 152 855** | 0 | 7.8 % | 0.26 us |
| 38400 | CPU /8 + CKPH | **0 of 2 304 471** (host overwrote 50 168: the Python drain tops out at ~37 kS/s) | 0 | 51 % | 0.24 us |
| 4800, DRAW 2 | CPU /8 + CKPH | 0 of 288 190 | 0 | 6.4 % | 0.24 us |
| 9600 | CPU /8 + CKPH | 0 of 576 050 | 0 | 12.8 % | 0.24 us |

- The chip's crystal sits at -0.6..-1.2 ppm against the MCU's.
- One 38.4 k DMA stamp gap was 41 us (1.6 P): entry jitter, not a loss.
- The only codes beyond the square's ~+-107 k are the first one or two
  samples of a CPU-frame session, in every read mode (/16 included). They
  are the transient after the R26 rewrite. DMA sessions skip them (samples
  only once locked).
- After the streams: DC V 0.99961 V on the 1 V calibrator; SELF? 2000 had
  every count 0.

**What the CPU numbers mean**:
- 19.2 kS/s costs 26 % of the CPU with CPU frames (/8 + CKPH) and 8 % with
  DMA frames.
- 38.4 kS/s costs 51 % with CPU frames and 17.5 % with DMA frames.

**Defaults**: CPU frames (DBG:AD2:DMA 0) at 0471860. Switching to DMA
frames (`set_dma_req` default 1) is one line, and the wave view would gain
the most from it: done in a239aa6 (decision 2026-10-22 #7), device check
11.11.

**76.8 kS/s** (done since, 11.12, as described here) needs the DMA data phase at /8 + CKPH: the
CPU sends the command byte and switches CKPH, then DMA takes the 3 data
bytes. It also needs probe frames that fit the 13 us period.

### 11.11 Device check of a239aa6 (2026-10-22): DMA frames as the default

a239aa6 = 1de50a5 (the memory rework) + `set_dma_req` default 1 (decision
2026-10-22 #7). Healthy after the install, crash count 0. No `DBG:AD2:DMA`
sent in the default runs. Calibrator 40 Hz square on V/COM (the decision's
item 3 asked for a 50 Hz sine; none on this bench), AC V.

**Emulator**: `ad2streamtest.py` with no `:dma` suffix
(it sends nothing now and expects the default) at -100 and +100 ppm, 4800
DRAW 2 / 9600 / 19200 / 38400: PASS (DMA 1, div 16 in every session).
`wavetest.py`: PASS.

**1. 60 s runs** (raw `LOG:DATA?` drain, DBG:AD2:DIV 0 = automatic):

| run | lost (LOG:COUNt?) | seq / stamp gaps | late / slips / overruns / timeouts / bad | CPU | max_isr / max_lat | div |
|---|---|---|---|---|---|---|
| 4800, DRAW 2 | 0 of 288 137 | 0 / 0 | 0 | 1.9 % | 3.6 us / 0.26 us | 16 |
| 9600 | 0 of 575 999 | 0 / 0 | 0 | 3.9 % | 3.6 us / 0.24 us | 16 |
| 19200, DRAW 2 | 0 of 1 152 864 | 0 / 0 | 0 | 7.8 % | 3.6 us / 0.26 us | 16 |
| 38400 | 0 of 2 304 171 | 0 / 0 | 0 | 17.5 % | 5.3 us / 0.28 us | 16 |

CPU matches 11.10 (7.8 % / 17.5 %). Codes stay within -106 k..+108 k. No
R26 transient samples (DMA sessions start after the lock).

**2. Sharing matrix**, 38400 for 900 s with the generator ON in DAC mode at
10 MS/s (sine 10 kHz, 2 Vpp: n = 1000, M = 20):
- lost 0 of 34 560 690, seq / stamp gaps 0 / 0, bad / timeouts / overruns
  / late / slips 0, CPU 17.6 %, max_isr 5.3 us, max_lat 0.28 us, locked
  -1.7 ppm.
- The generator stayed on: SOUR:FUNC? SIN, FREQ? 10000, OUTP? 1 after.
- The scope page cannot check the generator's output on this bench: the
  MCX input is not wired to the generator. The GEN badge reads 10.0 kHz.
- **BT not exercised here**: the host could not connect to the meter's SPP
  address ("Host is down"; the cause was the BT module's power pin, fixed
  since). Run later on 8b9cf79 (device check "Bluetooth (8b9cf79)"): 900 s
  at 38 400 S/s with DMA frames and a host polling over BT the whole time,
  34 560 269 records, lost 0, bad 0, CPU 17.5 %, BT answered right after.

**3. Wave view** (MF range page -> Wave, generator still on), every band
5 ms .. 1 s/div. "First sample" = from `TIM:SCAL` to the first captured
sample of the restarted session (DBG:ISR? polled over USBTMC, a few ms of
host resolution):

| tb/div | band S/s | first sample DMA / CPU | FREQ | VRMS | VPP | bad / timeouts / overruns / slips | CPU DMA / CPU frames |
|---|---|---|---|---|---|---|---|
| 5 ms | 9600 | 14 / 10 ms | 0 (2 periods on screen, both kinds) | 4.767 V | 10.01 V | 0 | 4.0 / 13.0 % |
| 10 ms | 4800 | 20 / 12 ms | 40.000 Hz | 4.745 V | 9.99 V | 0 | 2.0 / 6.5 % |
| 20 ms | 4800 | no restart | 40.000 Hz | 4.745 V | 9.96 V | 0 | |
| 50 ms | 2400 | 27 / 10 ms | 40.000 Hz | 4.699 V | 9.91 V | 0 | 1.0 / 3.2 % |
| 100 ms | 2400 | no restart | 40.000 Hz | 4.696 V | 9.89 V | 0 | |
| 200 ms | 1200 | 51 / 22 ms | 40.000 Hz | 4.696 V | 9.89 V | 0 | 0.4 / 1.6 % |
| 500 ms, 1 s | 1200 | no restart | 40.000 Hz | 4.696 V | 9.89 V | 0 | |

- The DMA and CPU-frame measurements are the same to the last digit.
- DMA sessions sample only once locked, so a band change takes longer
  to show the first sample: 14..51 ms. That is still far under one screen
  (50 ms .. 10 s).
- No "Wave stopped", forced stops 0, SYST:ERR? empty.
- Trigger NORM at 10 ms/div: +2 V rising and -2 V falling cross the level
  at the trigger column on two frames each.
- MENU closes the view (ISR stopped), READ? AC 4.775 V after.
- The FREQ 0 at 5 ms/div is a 40 Hz square showing only two periods. Both
  frame kinds give it.

**4. After the streams**: DC V AUTO+ on the 1 V calibrator reads 1.00002 ..
1.00021 V (within 0.05 %). `DBG:HYSPI:SELF? 2000` has every count 0.

**Verdict**: items 1, 3 and 4 pass. Item 2 passes for the generator DAC
here and for BT on 8b9cf79. DMA frames stay the default (a239aa6).

**Memory rework on the device** (1de50a5's numbers, a239aa6):
- STACK at boot: 18 328 of 46 620 B.
- After a scope walk (popups, MF menu tabs, FFT, STOP + Save CSV, AUTO, a
  screenshot with the FFT on): **21 616 / 46 620 B (46 %)**.
- The static worst path is 21.8 KB, and the emulator walk gave 21.4 KB.
  The current budget: [native-boot.md](native-boot.md) "Memory budget".

### 11.12 76.8 kS/s: hybrid frames (2026-10-22)

The chip's top AD2 rate (OSR 32 at /2, R26 low bits 0: the stock AC rows'
own setting) is now a rung of the ladder (`ad2_r26_for`), with DMA frames
only. Decision 2026-10-22 #9 has the choices; this section the design, the
emulator results and the device outcome: host drain and Wave view passed on
20a91d0, the disk lost 54 % there and passes since 825080c (TODO "76.8
kS/s"), and the loop of c830e09 locks at the datasheet rate (below).

**The frame** (`hy3131 spi::hybrid_kick`, logger.rs `isr`, `Dma::hyb`):
- /16 frames (13.6 us) do not fit the 13.02 us period; /8 mode 0 data is not
  clean (11.3); CKPH cannot change inside a DMA frame. So the CPU does the
  part around the switch, DMA the data: the channels are armed, CS low, the
  command byte (R03, read) by the CPU in mode 0 (waited for: 1.28 us), the
  read-period pulse on the PC10 GPIO, CKPH = 1, then RX / TX enabled for 3
  bytes (MOSI 0xFF). No shift framing: R03..R05 arrive as they are (`s24`).
- `dma_finish` (the next entry) puts CKPH back to 0 after the idle wait.
- The probe frames (B: clear, C: read back) stay 2-byte DMA frames in the
  shift framing, at /8 **mode 0**: a marginal DO bit only blurs a probe
  (AD2F read as 0), never a sample. Emulator with MisoErrRate 0.05 (50x the
  default) at +100 ppm: locked, lost 0.
- Entries B and C finish the last data frame, start their probe frame and
  only then store the sample (ring push + wave trigger); A does the same
  with its own data frame. RXB has two halves (the next frame starts
  before the last one is decoded), RXI holds the probe replies.
- Probes every 16th period at this rate (np 16, 8 for /16 frames).
- `Producer::push` no longer divides 64-bit per sample (only for a gap of
  more than 1.25 periods): `__aeabi_uldivmod` was ~0.5 us on the device.

**Geometry** (`Pll::hybrid`, logger.rs HYB_*; counts of 20 ns): A's entry to
the data's last bit `frame` = 370 (device estimate 7.3 us: latency 0.25,
finishing the last frame ~1.0, CS 0.2, command 1.3, pulse / CKPH / DMA start
~0.6, 3 x 1.28); a probe entry to the chip's INTF latch `delta` = 160 (3.2
us; at least 82 = CS setup + command + one clock); C right after A's frame
(`c_off` = frame). The window for A after an edge runs from the probe
frame's tail (~1.1 us after the edge) to `P - c_off - delta` (C's latch
before the next edge); beta = (P + c_off - delta) / 2 = 430 (8.6 us)
centres A in it: A ~1.2 us after an edge on the device (~2.5 us in the
emulator, whose ISR is faster: the same window). late_max = beta + 82 -
frame - 25 = 117 (2.3 us: a later A skips its read, `late`); a C later than
beta - c_off - 25 = 35 counts after its slot drops its probe (`c_late`; the
/16 sessions keep no check, as before). Margins on the device estimate:
C's latch ~1.2 us before the next edge, A's command ~1.5 us after B's tail.
Host model `ad2pll::tests::hybrid_frames_at_76k8`: 200 000 periods at -100
/ 0 / +100 ppm with device-like, emulator-like and slower (A 7.6 us, probe
3.6 us) timings, 0.2..1 us jitter: no frame across an edge, no conversion
skipped or doubled, no probe read back after the following edge; 2 us
latency spikes every 500 entries are absorbed (the A still reads); a 9.6 us
frame is caught by the model.

**The 76.8 kS/s rung's rate and the lopsided probe window (2026-10-23).**
Every 76800 session on the device (20a91d0, 825080c) ran with `f` on its
+1000 ppm clamp (rate_mhz 76 723.4-.8), 1.48 ISR entries per sample, ~0.24
probes per sample, 86 % of them "edge after B", max run 57; the record
stamps gave 76 574.4-.9 S/s in every session, and a 40 Hz calibrator square
had 956..959 samples per half period. The same chip and crystal lock at
-1.6 ppm at 4800 / 38400, also with these hybrid frames (DBG:AD2:DIV 8,1).
c830e09 took that as the chip's rate (0.29 % below FAD2CLK / OSR2) and made
76 574.6 the rung's nominal (hy3131.rs `AD2_OSR32_REAL_MHZ`, the emulator's
`Ad2Osr32Ppm` -2935). **That was wrong**: with the loop fix below, device
c830e09 (device check "c830e09: the 76.8 kS/s PLL retest") locks at
rate_mhz 76 800 0xx from a cold start, gap-free disk data has 1919.95
samples per 40 Hz period (the old builds 1914.27) and the Wave view's
record 960 per half period. The 76 574 came from the railed loop, not the
chip. Since 69abac4 `AD2_OSR32_REAL_MHZ` = the datasheet's rate again (and
`Ad2Osr32Ppm` 0); the loop fix and MAX_PPM 5000 stay.

Why it railed, and what that cost: this geometry's window is lopsided. B
may be early by up to P - beta + c_off (~590 counts) and still be read
right, but late by only P minus that (~61 counts, 1.2 us). A B later than
that has its read-back C after the NEXT edge and reads "edge after B": the
kicks then walk the phase forward a whole period to that next edge (a slip;
frames tear on the way). The old loop also stepped `f` by ki on every result
of the walk, all "lengthen". With the chip 1935 ppm beyond the clamp the
phase was held by kicks and such walks: in the 20a91d0 square dumps a
transition sample's value against its stamp's phase scatters over the whole
edge ramp (~+-6 us; 0.5 us at 38400), i.e. conversions doubled / skipped /
torn without any counter seeing it. In the host model the old loop also
railed from a word started ~600 ppm long or a chip that much fast.

The loop fix (`Pll::result` / `on_a`, hybrid geometry only): `f` holds
during a run of one sign longer than RUN_GEAR_UP (a walk), the probes stay
dense (every period C allows) for CALM = 256 results after the start and
after a run longer than 2 x RUN_GEAR_UP, with KI_DENSE = 4 x the frequency
step, so a word still off the chip's rate is pulled in before a sparse
16-period interval can drift the phase out of the window; MAX_PPM is 5000.
Device c830e09: 1.125 ISR entries per sample (was 1.478), cpu 455-457
permille (was 546-548), late / bad / overruns / slips 0, the transition
samples within 0.41-0.62 us rms of their stamps (was 3.0-3.45 us). The 76.8 kS/s
rung carries its own lock to its next session (logger.rs `REL_HYB`); the
shared `REL` takes a lock only within 1000 ppm. Host model
(`hybrid_76k8_far_from_the_nominal`): chips -3000 .. +3000 ppm, starts -1000
/ 0 / +1000 ppm: the word on the chip's rate, no bad sample after
acquisition, ~1 probe in 16 periods. A walk at a session start (the edge
search's error, ~3 us) still shows in about every second emulator session,
as with the old build (max run ~45, TornAd2 ~28, a slip or two), but no
longer drags `f`.

Emulator at c830e09, --mips 110 --isr-stall-ns 700 (the device's speed),
AC V 0.5 V 50 Hz; ISR entries per sample = DBG:ISR? ticks / captured. The
"76 574.2" rows model the chip at the rate the old loop reported, which the
device later showed to be wrong (above):

| build | chip at 76800 (model) | session | rate_mhz | entries / sample | ISR | doubled / skipped (model) |
|---|---|---|---|---|---|---|
| old (825080c) | 76 800 (datasheet) | ad2streamtest 3 x 10 s | 76 800.0 | 1.126-1.128 | 52 % | 4-6 / 5-8 (start walks) |
| old | 76 574.2 (the device's) | ad2streamtest 10 s | 76 723.4 (clamp) | 1.69 | 69 % | 98 / 98, 11 955 overruns |
| old | 76 574.2 | fastlogtest 76800 to disk, 2 s | 76 723.4 (clamp) | 1.63 | 67 % | lost 1133 |
| this | 76 574.6 (= nominal) | ad2streamtest 3 x 10 s | 76 574.6 | 1.133-1.135 | 52 % | 2 / 3 (start walks) |
| this | 76 800 (datasheet, +2935 ppm) | ad2streamtest 2 x 10 s | 76 800.0 | 1.135-1.151 | 52-53 % | 4 / 6 (start walks) |
| this | 76 574.6 | fastlogtest 38400 -> 76800 -> 76800 | 76 574.6 | 1.127-1.128 | 52 % | lost 0, trial kept |
| this | 76 800 | fastlogtest 76800 x 2 | 76 800.0 | 1.128-1.131 | 52 % | lost 0, trial kept |

(ad2streamtest's own drain / stamp-gap checks fail at --mips 110: the
main loop at the device's speed drains ~35 k rows/s over USB, as the
device does; its default 200 MIPS run passes.)

**Emulator honesty** (this change, emu/peripherals/GD32_SPI.cs, HY3131.cs):
before, SPI bytes were instantaneous, the chip latched at CS low, and the
virtual time stood still inside a CPU quantum (no `ICPU.SyncTime`), so no
frame could ever be "too long". Now: bytes take their SCK time with a TX
buffer and DMA in time; a frame from R03 updates the chip per byte and
counts **TornAd2** when it crosses a conversion edge (ad2streamtest checks
0); mode 0 at /8 samples a changed bit at its old level (MisoErrRate,
default 1e-3), CKPH 1 samples a full period late and garbles a command
byte; the timer interrupt is scheduled to the ns; DWT / TIMER / the flash
read the synced time; a STAT read that finds a byte shifting stalls the
CPU to the byte's end (BaseCPU.SkipTime, the same virtual time as the
spin). The ISR's CPU share is still ~0.57x the device's
(no APB / AHB wait states): DMA /16 4800 1.1 % vs 1.9 %, 38400 9.8 % vs
17.5 %.

**Emulator results** (200 MIPS, AC V 0.5 V 50 Hz, LOG:DATA? drained):

| session | chip ppm | captured | lost / late / slips / overruns / timeouts / bad / torn | ISR CPU (emu) | max_isr / max_lat |
|---|---|---|---|---|---|
| 76800 DRAW 1, 15 s | -100 | 78 312 | 0 | 29.5 % | 3.8 / 0.1 us |
| 76800 DRAW 1, 15 s + 10 s, MisoErrRate 0.05 | +100 | 86 008 + 62 158 | 0 | 29.6 % | 3.8 / 0.1 us |
| 76800 DRAW 2 (full DMM repaint every 100 ms), 8 s | -100 / +100 | 42 662 / 43 552 | lost 0; ring **overwritten 7.8 / 9.3 %** | 29.5 % | |
| 4800 DRAW 2 .. 38400 DMA, 1200, CPU 19200 / 38400 | -100 / +100 | | 0 | as before | |
| Wave view 1 ms/div (76800), 4 s | 0 | 500 313 | 0, ring overwritten 0 | 30.2 % | 3.9 us |

Also `DBG:AD2:DIV 8,1` (hybrid frames forced) at 4800 and 38400: lost 0, div
1008. The locked rate is the chip's within 0.1 ppm. DRAW 2 at 76.8 kS/s: the
repaint (~22 ms of main loop at 200 MIPS, more with the ISR's share) is
longer than the ring (2048 samples = 26.7 ms at this rate); the stream
loses nothing, the host sees the drops as stamp gaps (`overwritten`).
SRAM has 10.9 KB free on an 8 KB floor: the ring cannot double. The
logger panel (DRAW 1), the Wave view and the disk logger keep up.

**CPU on the device**: cpu_permille 455-457 at 76.8 kS/s (c830e09; 546-548
with the railed loop of 20a91d0 / 825080c), against an estimate of 40-46 %
from the emulator. For comparison: CPU frames at 38.4 kS/s ran 51 % with 0
lost (11.10).

**Disk** (fastlogtest, the disk's SPI timed too: ~0.2 ms of polled CPU per
page):

| flash | input | rung | B/sample | B/s | pages/s | FIFO high water | lost |
|---|---|---|---|---|---|---|---|
| typ (tPP 0.4 ms) | 0.5 V sine | 76800 | 1.19 | 91 KB/s | 356 | 308 B | 0 |
| typ | + 900 codes noise | 76800 | 1.75 | 134 KB/s | 525 | 500 B | 0 |
| typ | + 75 000 codes noise (~full-scale white) | 76800 | 2.56 | 196 KB/s | 767 | 460 B | 0 |
| max (tPP 3 ms) | 0.5 V sine | 76800 (no gate) | 1.51 | | 453 | 8170 B | **28 %** |
| max | + 900 codes noise | 76800 (no gate) | 2.38 | | 715 | 8174 B | **54 %** |
| max | either | 38400 | 1.26 / 1.69 | 48 / 65 KB/s | 189 / 254 | 471 / 626 B | 0 |

So a typical chip keeps up with room to spare and a worst-case one cannot:
76.8 kS/s goes to the disk only after a **page-program probe** (decision #9):
a disk session above 38.4 kS/s programs 4 pages of zeros in its last
cluster at the end of Prep, times each to BUSY clear, erases the cluster
again; mean <= 800 us and max <= 1.5 ms -> 76800, else the session runs at
38400 (LOG:SRATe? / LOG:PREamble? say so; LOG:DISK? fields 9 / 10 = the
probe's mean / max us). Emulator, gated (1ad38aa, fastlogtest LOG:DUR 2):

| flash | probe tPP mean / max | rung | rows | lost / ring overwritten | FIFO high water |
|---|---|---|---|---|---|
| typ | 400 / 400 us | 76800 | 153 601 | 0 / 0 | 319 B |
| max | 3000 / 3000 us | 38400 (capped) | 76 802 | 0 / 0 | 468 B |

`emu/fastlogtest.py --app <elf> --sessions ad2:38400,ad2:76800 [--noise 30 |
2500]` reproduces the tables (the max profile takes ~15 min of wall time).

**Wave view**: 1 ms/div = 76800, 2 ms/div = 38400 (768 samples on the 10
divisions), 5 ms/div and slower as before (`WAVE_TB_MIN` 15). With CPU
frames (DBG:AD2:DMA 0) the rung is capped at 38400 and the view closes
("Wave: rate needs DMA frames") rather than draw a wrong time base.

**Host drain.** The chopper at OSR 32 (CHOP ON, the logger's default) runs
on the chip like CHOP OFF (c830e09 item 1). 12-byte rows at 76.8 kS/s need
921 KB/s; the Python drain reached ~46 k rows/s on c830e09, so `LOG:DATA?`
in rows shows `overwritten`. The compact formats `LOG:FORMat PACKed | DLG`
(3 B and ~1.2-2.6 B per sample, [compact-log-format.md](compact-log-format.md))
are lossless at 76.8 kS/s in the emulator without the logger panel; not
measured on the device. The disk and the Wave view are the full-rate
consumers.

## 12. Inrush current on the stream (2026-10-22)

The AC A row of the 10 A range (1.3: OP1 x1 on the 10 mohm shunt,
DC-coupled, +-125 A) streamed at 9600 S/s by the section 11 ISR feeds a
step detector and a 2 s trace in the scope's BUF: Irms over the first
100 ms, the peak, a FASTnnn.DLG of the trace: [inrush-current.md](inrush-current.md).

## 13. E7: DC-coupled V on AD2 (2026-10-22)

### 13.1 Device test 6.5 (f4c792e, tools/experiments/ad2dcv.py)

Script: [`tools/experiments/ad2dcv.py`](../tools/experiments/ad2dcv.py).

`DBG:AD2:DCV? range,r26,n,flags` (src/ad2exp.rs `dcv_test`) writes the row
of 1.3 last row: the stock AC V row of the range (CFG_F0: its tap, OP1 on
SENSE, R32 0x38; 6 V = the /100 tap with OP1 x10) with SAD2IP = OP1O
instead of PB4 (the coupling capacitor), AD2IN AGND (or RLU), AD3 / RMS /
LPF off, AD1 on SENSE - PB5 (R22 0x14). The relays are DC V's = AC V's
(dmm.rs write_config: PA 0, PE 2 for both): the row is register only. Four
phases: A = AD1 alone, B = AD1 + AD2 (the AD2 mean over the same AD1
conversions), C = AD2 alone (the stream's state), D = C with OP1+ on AGND
(the zero). MR9270 calibrator on V/COM, 0..24 V (30 V requested: the
calibrator gives 23.97 V), READ? (the DMM's DC V row) as the reference;
4800 S/s chopped (R26 0x24), n = 8; the 6 V points above 6 V are over the
DMM range (no reference, dropped from the fits).

| range | gain fit / nominal | 0 V code (fit) vs zero D | max residual | sd (codes) | sd at the jack | ENOB |
|---|---|---|---|---|---|---|
| 6 V (/100, OP1 x10) | +0.84 % | 906 / 903 | 0.0013 % FS | 9.5 (6 at <= 2 V) | 0.46 mV | 15.8 |
| 60 V (/100) | +1.79 % | 164 / 171 | 0.0029 % FS | 3.6 | 1.8 mV | 17.1 |
| 600 V (/1000) | +0.53 % (to 24 V) | 164 / 174 | 0.0016 % FS | 3.0 | 14 mV | 17.4 |
| 1000 V (/10000) | +0.27 % (to 24 V: +-0.5 %) | 169 / 174 | 0.008 % FS | 3.1 | 0.15 V | 17.4 |

(OP1 AGND chop off: the firmware's row.) Variants: AD2IN = RLU moves the
zero by ~90 codes, nothing else; OP2 x1 instead of OP1 (the power meter's
path) is as linear and as quiet, with a smaller zero (~70 codes). OP1's
stock 2 kHz chopper (OP1CHOP 10) puts a +-280 code square (6 V; 28 at
60 V+) on AD2 while AD1 converts and holds still when AD1 is off, at the
same mean as the chopper held off (`sentinel.rs`'s finding): the row holds
it off (R33 OP1CHOP 00).

**AD1 while AD2 converts.** AD1 in this row (SENSE - PB5) with AD2 off (A)
matches the DMM's READ? within 0.3 mV at 6 V, 2 mV at 60 V (0.01 % of the
range; the DC V calibration of the tap applied). With AD2 converting (B) it
reads lower: 6 V -310 .. -365 counts at every level 0..10 V (steady: the
sd of a mean is ~9); 60 V -160 .. -240 at 0 V growing to -850 at 24 V
(an offset plus ~-0.05 % of reading); 600 V -150 .. -235 steady; 1000 V
-50 .. -130 with two +282 outliers. Mostly a steady offset that could be
calibrated per range, not on 60 V (level-dependent) or 1000 V (jumps).
The self-cal does not need it: it reads AD1 alone first.

### 13.2 Datasheet note 7 in the stock DC rows (1ad2943, tools/experiments/ad2n7.py)

Script: [`tools/experiments/ad2n7.py`](../tools/experiments/ad2n7.py).

The question: could the DC V "AC?" hint run on AD2 + RMS, enabled once
with the DC V row and left on, instead of the comparator alternating PB1 /
SENSE? `DBG:AD2:N7? item,state,n` writes the stock DC row (CFG_F1 /
CFG_F3, no sentinel) with AD1 enabled last, the converters never toggled
while AD1 converts, in state 0 (stock: AD2 / AD3 off), 1 (AD2 on OP1O -
AGND, OP1 on SENSE where the row has it off, R26 0x80 = 76.8 kS/s), 2 (1 +
AD3 on OP1O and RMS / LPF on, R29 0xF8 / R34 0x88 as the AC rows), and
averages 20 AD1 conversions at the row's R22 (0x15: 100 ms, chopped) after
3 dropped; per range at 0 V and one level, states 0, 1, 2, 0; two runs
minutes apart. Counts are AD1 codes (6 V: 1.66 uV, 60 V: 16.6 uV, 600 V:
166 uV, 1000 V: 1.66 mV, 600 mV: 0.17 uV per count at the jack).

| range | offset, AD2 on (run 1 / 2) | offset, AD2 + AD3 + RMS | gain, AD2 on | gain, + RMS | sd of one conversion | 0 -> 0 drift |
|---|---|---|---|---|---|---|
| 600 mV (DC mV) | -597 / -646 (-0.10 mV) | -2344 / -2430 (-0.40 mV) | -2270 / -1531 ppm | -1309 / -4735 ppm | 107 .. 880 (the 0.3 V level was noisy) | -92 .. +1841 |
| 6 V (AD1 on OP1O) | +899 / +898 (+1.5 mV) | -1750 / -1754 (-2.9 mV) | -3201 / -3261 ppm | -47 / -107 ppm | 25 .. 90 | -22 .. +137 |
| 60 V | +158 / -221 (+-3 mV) | +997 / +1008 (+16.7 mV) | -833 / -555 ppm | -1774 / -1023 ppm | 19 .. 30 | -8 .. +46 |
| 600 V | -236 / -222 (-38 mV) | +989 / +1000 (+0.165 V) | (24 V = 4 % of the range: not measurable) | | 19 .. 23 | -7 .. +19 |
| 1000 V | -152 / -129 (-0.23 V) | -961 / -948 (-1.6 V) | (2.4 % of the range) | | 25 .. 33 | -5 .. +4 |

So **(a)** AD2 + AD3 + RMS left on for the whole DC V row shift AD1 by
30 .. 90 x its noise: an offset of -2.9 mV (6 V) / +17 mV (60 V) / +0.17 V
(600 V) / -1.6 V (1000 V), i.e. 0.03 .. 0.16 % of the range = 15 .. 100
display counts at 60 000, beyond the DC V spec, plus gain shifts that on 60
V did not repeat (-1774 vs -1023 ppm). The offsets repeated within ~1 %
between the two runs (the AD2-only offset on 60 V did not: +158 vs -221,
OP1's chopper running on SENSE with AD1 is the suspect), so a per-range
correction is conceivable but would need its own calibration and a
stability study (temperature, time); the DC reading is cleanest with
AD2 / AD3 off, as the stock and opendm40 run it. **(b)** the 6.5 shift (AD1
with AD2 converting in the AC V row) is likewise mostly a steady offset;
on 6 V it is constant across 0 .. 10 V (+-25 counts), on 60 V it grows
with the level, on 1000 V it jumps. The supply current was not measured:
the meter has no readout of it.

### 13.3 What exists (1ad2943)

| piece | where | what |
|---|---|---|
| the row | ad2exp.rs `dcv_row` | as 13.1, AD2IN AGND, OP1's chopper held, AD3 / RMS / LPF off |
| entry self-cal | ad2exp.rs `dc_enter` (cold, before the stream ISR takes the bus) | at every AD2 start in DC V (logger.rs `start`): 1. AD1 alone on SENSE - PB5, 4 x 50 ms after 2 dropped, in volts with the DC V calibration of the tap (`dmm::dcv_volts`); 2. AD1 off, AD2 on, OP1+ on AGND: the zero z over 100 ms; 3. OP1+ on SENSE: the signal c over 100 ms (whole 50 / 60 Hz periods; z and c time-weighted means: a loop pause during a rippled input does not bias them - the emulator's 2 V + 0.3 V rms case was 0.6 % off without). per_code = V / (c - z) when the two points are >= 10 000 codes apart and within 3 % of the 6.5 table (`dmm::DCV_AD2_GAIN`: +0.84 / +1.79 / +0.53 / +0.27 %), else the table's; offset = c x per_code - V (the AD1 anchor). ~0.45 s. No cal (the nominal scale) if a step timed out or AD2 clipped |
| DC mV | `dc_enter` | streams the stock AC mV row (OP2O - AGND, DC-coupled; relays as DC mV), the AC mV LPF calibration |
| scale | dmm.rs `ad2_scale_of(1 / 3, r)`, `Dmm::ad2_scale`, `dc_cal` | the wave view, the fast logger's header, LOG:SCALe? |
| sources | logger.rs `mode_ok`, `ad1_first` | LOG:SOURce AD2 accepts DC V / mV (and the fast logger with it); the Record page's implicit source stays AD1 there (24 bit); a host LOG:STATe ON is refused in AC+DC / AUTO coupling / the power meter (alternating rows; before, only the disk path checked it) |
| fast logger | logger.rs `dc_prepare`, main.rs `fast_start` | the cal before the file header (its a / b), the DMM row back for the disk preparation, the session's `start` rewrites the row and keeps the cal |
| LOG:SCALe? | scpi.rs, main.rs `log_scale` | a,b of the AD2 session (value = a + b raw); dm40scpi.py `log --source ad2` writes calibrated values with it |
| wave view | app_wave.rs | DC V / mV enter their DC rows (no switch to AC V); the coupling key (CH1 / the V/div badge) switches V DC <-> AC at the same range item (`WaveReq::Coupling`); autorange: entered in AUTO and no range picked since, a clip (\|code\| > 257 000) steps the range up (the DMM's autorange chose by DC; AC on top clips); V/div - from 6 V DC at 500 mV/div goes to 200 mV/div on 600 mV (DC mV, the AC mV row); leaving the view restores AUTO when it was AUTO at the entry, the coupling the key chose stays |
| tests | emu/hy3131/dcvtest.py, wavetest.py `--case dcv`, the model (HY3131.cs: OP1O = g (SENSE + Vos), the chopper, `op1vos:`, `ad2dcg:`) | |

### 13.4 Device check of 1ad2943 (E7)

Calibrator on V/COM, DC V AUTO, LOG:SOURce AD2 4800 S/s host sessions of
3 s (~14 400 records, 0 lost), the mean of a + b raw vs READ? before and
after, two sessions per level:

| level | range | b (V/code) | mean - READ? | sd |
|---|---|---|---|---|
| 0 V | 6 V | 4.808e-5 (the table: < 10 000 codes) | +0.06 / -0.37 mV | 0.9 mV |
| 1 V | 6 V | 4.817 / 4.819e-5 (two-point) | -0.36 / +0.03 mV | 0.3 mV |
| 3 V | 6 V | 4.812 / 4.811e-5 | -0.30 / +0.06 mV | 0.3 mV |
| 5 V | 6 V | 4.809 / 4.810e-5 | -0.93 / -0.45 mV | 0.5 mV |
| 10 V | 60 V | 4.866e-4 | -1.08 / -1.03 mV | 1.6 mV |
| 24 V | 60 V | 4.859e-4 | -1.54 / -2.98 mV | 2.3 mV |

i.e. within 0.02 % of the range. LOG:STATe ON takes 0.77 s (the cal). The
fast logger (`dm40scpi.py fast --source ad2 --rate 9600 --dur 10`, DC V 5
V): 96 002 samples, 0 lost, mean 4.99750 V vs READ? 4.99786 V. Wave view
(MF -> Wave in DC V AUTO at 0.3 V, the calibrator then raised): 2 V/div,
VAVG 0.290 / 2.998 / 4.992 V (the display's 80 mV per pixel); the coupling
key: AC V, VAVG 0.012 V, and back; V/div -: 200 mV/div, the DC mV row (5
V clips at 1.2 V, 0.3 V reads 0.294 V); MENU: DC V reads again. Not seen
on the device: the clip autorange (the DMM's AUTO stayed on 6 V at 0.3 V;
the emulator's `--case dcv` covers it).

33b44bd (the 100 ms trapezoid-weighted AD2 windows): 0 V +0.06 / -0.17 mV,
1 V -0.51 / -0.92 mV, 5 V -0.99 / -0.58 mV, 24 V (60 V) -1.8 / -2.1 mV;
LOG:STATe ON 0.56 s.

### 13.5 Limits

* Gain without a big enough input (the two points < 10 000 codes: < 0.48 /
  4.8 / 48 / 480 V on 6 / 60 / 600 / 1000 V) is the 6.5 table of this
  unit; r2 / r3 measured to 24 V only.
* AD2 noise: 6 V 0.3 .. 0.9 mV sd per sample at 4800 S/s; the DC V
  autorange does not run while streaming (logger sessions keep the range
  in force; the wave view steps up on a clip only when entered in AUTO).
* Offset drift during a long session is not tracked (re-cal at each start).

