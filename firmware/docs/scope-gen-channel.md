# Scope extra channels: GEN (computed) and DMM CH2

Owner request 2026-10-27: the generator as a scope channel, mainly as a
trigger source ("it need not be captured and digitised: compute when the
trigger fires and draw accordingly"), and the DMM's wave stream as an
optional second channel on the MCX scope page.

## 1. GEN: the generator as a computed channel

### 1.1 Why it can be computed

Both the generator and the MCX acquisition are timer-driven from the same
200 MHz clock (PLL from the 25 MHz crystal; HCLK = APB timer clocks =
DWT, [clock.rs](../src/clock.rs)):

| what | clock | position readable |
|---|---|---|
| generator, DAC path | TIM6 update = one DAC sample, M = (PSC+1)(ARR+1) ticks; DMA0 S5 plays a circular table of L samples holding K periods | S5 NDTR (table position), TIM6 CNT (ticks into the sample) |
| generator, PWM path (square > 50 kHz) | TIM12, D = (PSC+1)(ARR+1) ticks per period | TIM12 CNT |
| MCX scope | TIM1 CC4 = one sample, (PSC+1)(ARR+1) ticks; sample n at CC4 of period n after the enable | the enable instant (DWT) |

So one reading of the generator at the scope's start fixes the
generator's phase at every sample of that record, with no drift (one
crystal). Nothing is digitised; the GEN channel needs no wire.

### 1.2 The model ([genplan.rs](../src/genplan.rs) `Shape`, `Sync`; host test tools/gen-test)

* Phase 0 = table sample 0: sine mid-rising, square rising edge, triangle
  and ramp at the bottom; PWM: count 0 (its rising edge).
* `gen::now()` ([gen.rs](../src/gen.rs)): the generator's time in ticks
  into its table loop (q = M L ticks for K periods; PWM q = D, K = 1) and
  the DWT count of that instant. DAC: the output (DOR) holds table sample
  L - NDTR - 2 (each TIM6 update copies DHR to DOR, then the DMA loads the
  next sample into DHR), plus TIM6 CNT (PSC+1) ticks into it. Up to 16
  tries of DWT, NDTR, CNT, NDTR, DWT (PWM: DWT, TIM12 CNT, DWT) with only
  the loads between the DWT reads (`gen::reads`); a try counts unless NDTR
  moved during it, CNT is within 8 ticks of an update (the DMA may lag), or
  its window is 128 cycles or more (an interrupt). The first such try is
  taken. All arithmetic comes after the
  reads: with a u64 modulo inside the window every DAC try was over the
  old 64-cycle limit on the device, so the DAC path had no phase (no GEN
  trigger, no GEN trace; PR #64 device check). `DBG:GENTRIG:STAT?` counts
  the readings and the failed ones, with the last reading's window and the
  narrowest window of the last failure.
* `Scope::start` reads the generator right before the TIM1 enable and the
  DWT right after it: `Sync { t0, s, q, k, shape }` with
  t0 = g + (DWT_enable - DWT_g) + ccr4 (PSC+1) - lag, s = ticks per sample.
  Sample n is at generator time t0 + n s, phase frac((t0 + n s) k / q).
  More than 400 cycles between the two reads (the AD2 stream ISR, say):
  no phase for that record (no GEN trigger, no GEN trace).
* The trigger: `Shape::crossing(level, slope)` gives the phase where the
  output crosses the trigger level (V) in that direction (sine by
  bisection; square 0 / duty; triangle u/2 / 1 - u/2; ramp u / 0); a level
  not strictly inside the output's swing never triggers. `Sync::next(n0,
  ph)` = the first sample at or after the next crossing.
* The trace: `Sync::span(a, b)` = the output's min / max over samples
  [a, b) (the column), from the shape (ends plus the extremes / jumps in
  between). The DAC staircase and the output filter are not modelled.
* PWM path: the levels are 0 and 3.0 V (`PWM_HIGH_V`: CH1 showed 0..3.0 V,
  3.07 Vpp, on the device; the output stage, not the 3.3 V logic level).
* `gen::epoch()` counts output (re)starts: a record read in another epoch
  has no phase (the user changed the generator meanwhile).

### 1.3 How each acquisition path uses it ([scope.rs](../src/scope.rs))

| path | GEN trigger |
|---|---|
| linear record (fs > 1.25 MS/s) | `finish`: the first crossing in [pre, pre + CAP - screen]. That window (65 us at 100 us/div, 32 us at 50 MS/s) can be shorter than a generator period, so `gen_ready` starts the record at the generator phase that puts a crossing in the window's middle: busy-wait up to 1 ms (`GEN_SPIN`), else not this main-loop pass. Auto past its timeout starts at once (untriggered if no crossing falls in). |
| stream (fs <= 1.25 MS/s, circular) | `stream_poll`: the next crossing is computed instead of searched; emitted once its post-trigger part is in, as the CH1 stream path. A record without the phase (the start's reading failed, the generator restarted since) reads it again against the record's sample 0 (`gen_resync`). |
| comparator (hwtrig) | not used: off while the source is GEN or the GEN trace shows (its stop instant is not known, so its records have no phase). |
| roll | no trigger (as CH1); no GEN trace (column frames). |
| wave view (source DMM) | not offered: the HY3131 runs on its own crystal. |

The trigger type (pulse / runt) does not apply to GEN: edge only.

### 1.4 UI, SCPI, settings

* MF > Trigger > **Source: CH1 | GEN**; the trigger badge in the GEN
  colour (cyan) for GEN; a toast when GEN is picked with the generator off.
* MF > Display > **GEN trace: On | Off**: the computed output, cyan, under
  CH1, on CH1's V/div and position (a loopback to CH1 at x1 lies on it).
  Hidden while dragging / pinching (drawn again on release) and in roll /
  zoomed-out column frames.
* SCPI `TRIGger[:EDGE]:SOURce CHANnel1|GENerator` (`CHAN1` / `GEN`);
  `DBG:GENTRIG:LAG <ns>` (not kept) sets the lag below; `DBG:GENTRIG:STAT?`
  the phase readings (above).
* Kept across a restart in the same session with the other trigger
  settings (W_SCOPE_EXT word 3, bits 4 / 5).

### 1.5 Unknowns, device check

The lag between the timers and the voltage at the CH1 input (DAC settling,
the generator's output stage, the scope's front end and the ADC pipeline,
the TIM8 latch below 50 MS/s) defaults to 185 ns (37 ticks), measured on
the PWM path (`DBG:GENTRIG:LAG 188`: the CH1 crossing within 2 ns of T at
100 ns and 10 ns/div); the DAC path's lag is not measured (*unknown*). It matters at the fast
timebases only (40 px per division: a 100 ns lag is 40 px at 100 ns/div,
0.2 px at 20 us/div). Device check, the generator looped back to CH1 (x1):

1. Sine 1 kHz 3 Vpp, 200 us/div, Source GEN, level 1.5 V rising, Normal:
   T'D on every frame, the CH1 crossing on the T marker; GEN trace On: the
   cyan trace on CH1. Repeat with falling, level 0.5 V / 2.5 V, Single.
   `DBG:GENTRIG:STAT?`: the readings grow, the failed count stays (or
   nearly), the last window well under 128 cycles.
2. Level 3.2 V (outside the swing): WAIT in Normal, AUTO frames in Auto.
3. Square 2 MHz (PWM path), 100 ns/div: the edge on the T marker; read the
   offset in ns and set `DBG:GENTRIG:LAG`; repeat at 10 ns/div and 1 us/div
   (same offset expected). Then the DAC path: square 10 kHz, 1 us/div.
4. Slow generator against a fast timebase (1 kHz, 1 us/div): triggered
   frames (the timed start), no stall of the page (DBG:PING?).
5. Generator off with Source GEN: Normal waits, Auto runs untriggered.

## 2. DMM CH2 (the AD2 stream on the MCX page)

### 2.1 What it is

The wave view's stream ([dmm-input-scope-feasibility.md](dmm-input-scope-feasibility.md)
sections 9-11: the HY3131 AD2 at 1.2 .. 76.8 kS/s in one timer ISR) drawn
as a second trace (magenta) over the MCX record, time-aligned. MF > Display
**CH2 (DMM): On | Off** and its V/div **- / +** (the wave view's steps,
screen only: the DMM range stays); the graph shows "CH2 1.5V" (a hole
below the FFT's box). 0 A / 0 V of CH2 is on the centre row (no CH2
position yet). The rate follows the timebase as in the wave view
(`scope::wave_fs_for`, 1200 S/s from 200 ms/div), but at most 38.4 kS/s
(`app_ch2::CH2_MAX_FS`, 2 ms/div and faster): at the wave view's
76.8 kS/s the stream ISR takes ~46 % of the CPU and the logger ring
overflowed in the scope page's passes (device: ~1500 samples/s dropped at
1 and 0.5 ms/div). Faster than 2 ms/div CH2 shows as steps (fewer samples
than columns).

The DMM reading pauses while CH2 streams (decision #3: the stream owns
the HY3131 bus; the sentinel and BT serve pause with it, as in the wave
view); it comes back when CH2 is turned off or the page is left. CH2 runs
on V (DC / AC V, DC / AC mV) and AC A / mA / uA (DC-coupled rows); not in
DC A (no AD2 path: "CH2: needs AC A"), AC+DC / AUTO coupling, the power
meter, the other functions, or while a CSV recording runs.

### 2.2 Time alignment ([scope_ch2.rs](../src/scope_ch2.rs))

* The stream's samples carry the producer's microsecond stamp (`ts_us`,
  logger.rs: the DWT time of the ISR's read since the session began).
  `Logger::begin_cyc` is the DWT count of that begin (the ISR producer's
  `isr::begin_cyc`), so a stamp maps to the DWT clock: c0 + 200 ts_us.
* The store keeps the last 4096 samples (raw >> 3, 16 bit, 8 KB) and an
  anchor (sample index, DWT) every 128 samples; between anchors the index
  is linear in time (the stream is regular between holes). A hole (ring
  overflow, missed conversions), a STOP / RUN, a restart empties it.
* The MCX record's sample 0 is at DWT_enable + ccr4 (PSC+1) (TIM1 CC4, as
  for the GEN channel, `Scope::rec_time`); a linearised ring's BUF[0] is
  `rec_base` samples later. Each frame column is a DWT interval, and the
  CH2 samples in it give the column's min / max (`Store::span`). Roll:
  `Scope::roll_time` (the newest column's end and the column length).
* The comparator is off while CH2 is on (its records' stop instant is not
  known), as for the GEN trace.
* The stamp is the read, not the conversion: a conversion integrates over
  the period before it, and the AD2 filter delays it further.
  `DBG:CH2:LAG <us>` (0 until measured) takes that off; until then CH2 may
  lag CH1 by about a conversion period (26 us at 38.4 kS/s = 1 px at
  1 ms/div; 0.8 ms at 1200 S/s = 0.2 px at 200 ms/div). The lag is one
  value for every rung; one measured at 2 ms/div holds at the faster
  timebases (the same rung).
* DWT wraps every 21.5 s: anchors and the record's time are compared as
  wrapping i32 differences, which holds while they are within ~10 s of
  each other (the store spans at most 3.4 s).

### 2.3 Ownership ([app_ch2.rs](../src/app_ch2.rs), main.rs)

* main.rs, before the wave view's and the inrush page's requests: CH2's
  stream starts when `App::ch2_want` gives a rate (CH2 on, the Scope view
  with source MCX, a function with an AD2 row) and the logger is idle
  (`dmm.start` first, as the wave view's entry), restarts at a rate band
  change, stops when CH2 is not wanted (the DMM restarts on the logger's
  ON -> OFF edge as after the wave view). A stream it did not start (a
  host LOG session, the wave view, inrush) is left alone; entering the
  wave view stops CH2's stream first.
* A disk job (screenshot, Save CSV) stops it (region C code every pass,
  as the wave view's pause); it starts again after.
* A lost stream (a DMM change: CONF, the power key's sub-mode cycle, the
  bus guard) is retried after 2 s (`ch2_lost`), so a stream that cannot
  run does not rewrite the DMM row every pass.
* The DMM rail stays on in the Scope view while CH2 is wanted
  (`update_rails`).
* The store is drained every scope pass (also during a drag) and between
  the pass's slow parts (`wave_keep`); a stopped scope throws the stream
  away and keeps the store as it was (the stored record's CH2, zoom / pan).
* Kept: CH2 on / off (W_SCOPE_EXT word 3 bit 6); its V/div goes back to
  the range's full scale at each start.

### 2.4 Device check

1. A 50 Hz 1 V rms sine on V-COM (DMM AC V) and the same signal (or the
   generator, 50 Hz) on CH1: CH2 On: the magenta trace at 2 / 5 / 20 ms/div
   in phase with CH1 (read the offset; `DBG:CH2:LAG` to cancel it), no
   jumps between frames; `DBG:ISR?` lost 0, max_lat as in the wave view.
2. Timebase 1 ms -> 200 ms/div (roll) -> 1 ms: CH2 at every step; STOP,
   zoom, pan: CH2 follows the stored record.
3. DC V 6 V range with a DC step source: CH2 DC level right; CH2 V/div
   - / +.
4. CH2 On in DC A: the toast; AC A with 0.5 A DC: CH2 at the DMM's DC A.
5. Leave the page (MENU, the DMM page): the DMM reading back in ~1 s;
   back on the scope: CH2 again. Screenshot with CH2 on: CH2 back after.
6. A host `CONF:VOLT:DC` while CH2 runs: the DMM changes, CH2 back on the
   new row after ~2 s.
