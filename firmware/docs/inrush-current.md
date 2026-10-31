# Inrush current

The inrush capture of the A function, as on the Fluke 376 / 323 clamps: arm
on the 10 A range, trigger when the current steps up past a threshold, show
the RMS over the first 100 ms and the peak, keep a 2 s trace with a
pre-trigger part and save it as a FAST*.DLG record. User-facing behaviour:
[manual, Inrush current](../../docs/manual/README.md#inrush-current).

Device status: the capture, the trigger, the results and the saved file are
**untested on the device** (the A jack was not wired in any device check;
section 9). Everything about them below is host-test and emulator evidence.

Code: [`src/inrush.rs`](../src/inrush.rs) (the detector, the trace, the
results, the saved records; pure, host-tested),
[`src/app_inrush.rs`](../src/app_inrush.rs) (open / arm / close, the end of
a capture, the save glue; region C),
[`src/app_inrush_live.rs`](../src/app_inrush_live.rs) (the ring drain, the
live state line, the state queries; region B),
[`src/ui_inrush.rs`](../src/ui_inrush.rs) (the page),
[`src/main.rs`](../src/main.rs) (the stream start / stop, the save session),
[`emu/hy3131/inrushtest.py`](../../emu/hy3131/inrushtest.py) (the emulator
scenario).

## 1. Data path

- **Source**: the HY3131 AD2 stream of the stock AC A row in the **10 A
  range**: OP1 x1 across the 10 mohm shunt, DC-coupled, +-125 A at the AD2
  rails ([dmm-input-scope-feasibility.md](dmm-input-scope-feasibility.md)
  section 1.3). The same row as the wave view's A trace. The RMS of the raw
  samples covers AC and DC loads alike; the scale is the DMM's AD2
  calibration of the row (`dmm::ad2_scale`), the zero is measured at the
  arm (section 4).
- **Stream**: a wave-style logger session (`Logger::start_wave(4, 1,
  inrush::RATE_HZ)`), i.e. the phase-locked stream ISR of
  [dmm-input-scope-feasibility.md](dmm-input-scope-feasibility.md) section
  11 (DMA frames) at **9600 S/s**: 960 samples in the 100 ms window, a
  104 us grid for the peak. The ring belongs to the page: `LOG:DATA?` is
  empty and `LOG:STATe ON` is refused ("Settings conflict"), as for the wave
  view. The DMM reading pauses while armed (main.rs `busy`) and resumes when
  the capture is complete.
- **Ring and trace**: every main-loop pass drains the logger ring
  (`logger::wave::drain`, 2048 samples = 213 ms of slack) into the detector
  and into a ring of 16-bit codes `(raw + 4) >> 3` (3.8 mA per code; the AD2
  noise is ~32 raw codes = 4 of these) in the **scope's record buffer**
  (the first `scope::INRUSH_LEN` = 26 624 slots of `scope::BUF`). `Scope::lend_buf` stops the MCX
  acquisition (its DMA writes BUF) and drops the stored scope record while
  the page is open (a scope CSV in progress refuses the page);
  `return_buf` gives it back. The capture needs no SRAM of its own.
- **Trace length**: 200 ms before the trigger + 1.8 s after = **2.0 s**
  (`PRE` 1920 + `POST` 17 280 samples), 100 ms per chart division. At
  9600 S/s with 16-bit codes BUF holds at most 2.77 s. A lamp's inrush is
  over in ~0.1-0.3 s, a small motor's start in ~0.5-1.5 s.

## 2. Trigger rule

`inrush::Cap::push`: the **mean of |I| over the last ~1 ms (`SHORT` = 10
samples) exceeds the largest such mean of the previous ~100 ms by the step
threshold** (default 1 A; the page's stepper 0.2 / 0.5 / 1 / 2 / 5 / 10 A,
`INR:THR` 0.1..50 A). The "envelope" is the maximum over `NBLK` = 10 blocks
of 10 ms; the trigger compares against completed blocks only.

- Nothing flowing: an absolute threshold (|I| > step), either polarity (a
  reversed DC lead triggers too).
- A load already running: only a **rise by the step** triggers. The
  envelope holds the idle current's own peaks: an AC load of 2 A rms has
  1 ms means up to its peak (2.83 A), so it does not trigger a 1 A step
  (host test; emulator: 6 s of a running 2 A rms 50 Hz load, then a step to
  5 A rms triggers).
- **Why not an absolute level on the raw sample** (the wave view's ISR edge
  trigger): on an AC load an absolute level is crossed every half period; a
  step from the idle level needs an idle estimate; and the ISR's 10-bit
  trigger code (0.24 A per code in this row) is too coarse for sub-ampere
  steps. The detector runs in the main loop on every drained sample, in
  order, so it has no dead time.
- **Armed** once the zero is measured (110 ms, section 4) and 200 ms + 1 ms
  of history are in (`Phase::Arming` until then: ~0.3 s after the arm,
  ~0.2 s after a hole).
- **t = 0** is the first sample of the ~1 ms window that crossed: at most
  ~1 ms before the step reached the threshold. The 100 ms window starts
  there and can hold up to ~1 ms of the idle level (host test: within 0.3 %
  of the exact window for a 12 A / 6 A step).
- **Not detected**: a rise slower than the step per ~100 ms (the envelope
  follows it). A soft starter's ramp is not an inrush.

## 3. Holes in the stream

`logger::wave::dropped()` (ring full + ISR misses) is checked every pass:
- while waiting: the history starts again (`Phase::Arming`, ~0.2 s);
- after the trigger: counted (`Res::lost`, the 4th field of `INR:FETCh?`,
  the page's state line). The saved file's stamps skip the lost stretch at
  the first hole, so the CSV's `lost_before` shows it. The trace on the page
  closes the gap (the samples are contiguous in BUF).

## 4. Results

Over the linear trace after the capture (`Cap::results`, region C: the
stream has stopped):

| value | definition |
|---|---|
| Irms 100 ms | sqrt(mean(I^2)) of the 960 samples from t = 0, DC-coupled (AC + DC rms) |
| Peak | the sample of largest abs(I) after t = 0, signed; **OL** when a sample reached the AD2 rail (abs(code) >= `CODE_OL` 32 700 of 32 767, ~125 A) |
| Idle | rms of the pre-trigger part (200 ms - 1 ms) |
| Lost | samples lost after the trigger (section 3) |

The zero (exact in the results, rounded to a code in the detector) is
measured at every arm (`Cap::arm`, `inrush::ZERO_*`): the first 10 ms of the
stream are skipped, the next 100 ms averaged. Their mean is the zero when it
is within 0.2 A of the calibration's offset and their rms about it is below
50 mA (no load running); otherwise the calibration's offset stays. The cal
record has no LPF offset for the AC A rows (`0x10C`.., all 0 on this unit),
and the 10 A row's AD2 mean read +0.0717 A with the A jack open (device
[185c066](../../docs/history/device-check-2026-10-21.md#185c066-plain-ac-first-readings-8ed9499-acdc-mv-a-jack-9a191ea-two-open-triage-items-scope-trigger-level-in-volts--nearest-marker-drag-f6c6ac3-2ad7feb-2026-10-25)
step 4: rms about the mean 1.72 mA), so the page read "Armed: 0.069 A now"
with nothing in A before the zero was measured. A hole while the zero is
measured starts it again; one later keeps it.

## 5. The saved file

Each complete capture is saved as **LOGS/FASTnnn.DLG** (kind 3, the fast
logger's format: [compact-log-format.md](compact-log-format.md) "Fast
stream"), served as LOGS/FASTnnn.CSV on the USB volume and by `MMEM:DATA?`,
decoded by [`tools/dlog.py`](../tools/dlog.py), listed by the gallery like
every FAST file. It goes through the fast logger
([`src/fastlog.rs`](../src/fastlog.rs)): `FastReq::Inrush` ->
`fastlog::begin` (19 200 x 3 B + 8 KiB planned) -> Prep (allocate / erase,
region C, the stream has stopped) -> `Ev::Start` -> main.rs gives the header
(`App::inrush_head`) instead of starting a logger session -> `poll(false,
drain)` with `drain` = the trace's records (`Cap::records`): the run ends at
once (no logger), the Drain phase writes the whole trace, then Stop trims
the file. The GUI stays live during the save (main.rs `logging` leaves the
inrush save out).

The FAST prefix is kept because fatdir, vdisk, dlog, csvlog's catalogue and
the gallery know it; a new "INRnnn" would touch all of them. The header
says what it is:

    # Alientek DM40A, opendm40 0.0.1, inrush capture, AD2, AC A row 10 A (10 mohm, OP1 x1, DC-coupled), nominal 9599.969 S/s
    # trigger: 1 ms mean |I| above the idle peak + 1.000 A; t0 at t_s 0.200000; Irms(100 ms) 7.5672 A, peak 12.016 A, idle 0.0506 A rms, lost 0
    # t_s from the first sample (nominal rate); raw = AD2 code / 8; value = the DMM's AD2 calibration; lost_before = conversions missing before the row
    t_s,raw,value_A,lost_before

- `raw` = the 16-bit code, `value_A` = a + b x raw (b = the row's A per AD2
  code x 8, a = minus the calibration offset), as `INR:PREamble?`.
- `t_s` from the first trace sample at the nominal rate (the ISR samples on
  the chip's conversion grid; the locked rate is within ~1 ppm of it on the
  device), t0 = 0.200000 s.
- Emulator: 19 200 rows, an 883 634-byte CSV; the DLG ~40 KB.

A save that cannot start (a CSV recording, an upload, no disk) or fails
shows "Captured, not saved: ..." on the page; the result stays on the page
and over SCPI. Arm is refused while the save runs (~1 s; the Prep erase can
take a few s on a dirty disk). Closing the page during the save finishes the
save first, then returns BUF (a toast says where it went).

## 6. UI

- **Entry**: MF in the A function -> Current Settings -> **Inrush** (the
  note row beside Fuse check). The capture is a sub-function of the A
  function, as on the Fluke clamps, not a Record page entry: the Record page
  holds recordings of readings
  ([decision #1](../../docs/history/decisions-2026-10-22.md#1-on-device-logger-page-interval-recording-to-the-disk)),
  and an inrush capture is a triggered single event of the A input. Builds
  without the stream ISR refuse it ("Inrush: no stream in this build").
- **The page** (`View::Inrush`, the range-page look; the DMM page's strip
  chart geometry; layout in [`src/ui_inrush.rs`](../src/ui_inrush.rs)):
  title, the threshold stepper, the Irms 100 ms and Peak boxes, a state
  line, the 2 s trace (100 ms window shaded, t = 0 a line; a min / max
  envelope per column, so an AC load shows as a band), IDLE and FILE in the
  right column, Back and Arm / Cancel / Saving.
- Opening selects AC A, 10 A (manual) and arms at once. Closing (Back,
  MENU, MF or any view change) stops the stream, restores the A function's
  sub-mode (DC / AC / AC+DC) and autorange, and returns BUF to the scope.
  RUN = Arm / Cancel; AUTO is refused (the range is held). A host change of
  the DMM function stops the capture ("Stopped: DMM changed").
- Refused with a toast: in the power meter, while a CSV / fast recording
  runs, while a scope CSV holds the record, while a save still runs.
- ui-render scenes i01..i07 (armed, DC lamp, AC motor, OL with the longest
  texts, stopped, idle, triggered) with the overlap check, and each live
  update drawn over a page equal to a fresh page.

## 7. SCPI

The commands (`INRush[:STATe]`, `:ARM`, `:THReshold`, `:CONDition?`,
`:FETCh?`, `:FILE?`, `:PREamble?`, `:DATA?`) are in
[scpi-reference.md Inrush current](../../docs/manual/scpi-reference.md#inrush-current).
Over Bluetooth the setters are refused ("Command protected": a capture
writes to the disk, as `LOG:CSV ON`); the queries work.

While a capture runs, poll **`INR:CONDition?`** (or `INR:STATe?`): those
two run from region B. `INR:FETCh?`, `:DATA?`, `:FILE?`, `:PREamble?` and
the setters are region C (slow flash): each costs one stall of the stream
ISR's entry (~10-20 us against ~47 us of margin at 9600 S/s,
[dmm-input-scope-feasibility.md](dmm-input-scope-feasibility.md) section
11.4). That should not lose a sample; it is not measured on the device. The
page's full repaint at the arm (region C) runs while the stream is still in
`Arming`, where a hole only restarts the 0.2 s history.

## 8. Emulator

[`emu/hy3131/inrushtest.py`](../../emu/hy3131/inrushtest.py) (the
front-end model's current source switched at fixed virtual times; a private
disk copy and its own log per process): MF -> Inrush from DC A, then

| case | input | result |
|---|---|---|
| DC "lamp" | nothing in A with the model's AD2 offset (`acaoff:0.717`, +0.0717 A), then 12 A for 20 ms, 6 A for 80 ms, 3 A | the measured zero (`INR:PRE?` a = -0.0717 A); Irms(100 ms) ~7.57 A (exact window 7.589; t = 0 up to 1 ms early), peak ~12.01 A, idle below 20 mA, lost 0; LOGS/FAST000.DLG: 19 200 rows, t0 at 0.200000 s, raw = `INR:DATA?` |
| AC "motor" | 2 A rms 50 Hz running, then 5 A rms | no trigger by the running load; Irms 5.105 A, abs(peak) 7.229 A, idle 2.047 A |
| host `CONF:VOLT:DC` while armed | | capture stopped ("Stopped: DMM changed"), stream off, DC V reads |

`INR:STAT OFF` puts DC A back (`FUNC?` CURR); the DMM's AC A reading of the
5 A rms load after the capture: 5.000 A.

The AC case reads 2 % high, the DC case does not: an emulator artefact. The
model's raw AD2 sine in this row is 2.2 % above its DC scale (a host LOG
session on AD2 in the same row: +-15 150 codes peak for 5 A rms, 14 830
expected; 5 A DC gives 10 486 codes, exact), while the DMM's own AC reading
comes from the model's RMS registers. The real AD2 gain on AC is unknown
until the device check.

## 9. Device check

**Status: not run.** The capture needs a load current through the A jack,
which no device session had ([device-check-2026-10-21.md](../../docs/history/device-check-2026-10-21.md):
session 825080c item 7b "Inrush save: Not run", session 33b44bd "Inrush
page taps while armed: not run"); TODO "Inrush current" still lists the device
check as open ([`TODO.md`](../TODO.md)). What did run on the device:

- the page opens and arms (`INR ON` -> ARMED); the threshold taps (2 / 5 /
  2 / 1 A) keep it ARMED; Cancel -> IDLE, Back -> DC V; `DBG:ISR?` while
  armed: late 0, dropped 0, max_lat 14.4 / 16.2 us (device check 2f56ff9,
  item 7d). The page read "Armed: 0.069 A now" with nothing in A (the
  100 ms rms of the stream; the plan below expects an idle below ~20 mA);
- `LOG:CSV ON` on the armed page is refused with "Settings conflict: inrush
  page" (device check c92b605, item 6).

The plan (about 25 minutes): meter on USB
([`tools/dm40scpi.py`](../tools/dm40scpi.py)), the load on A-COM, a second
meter (an inrush clamp or a DMM on its 10 A range) or the scope with a shunt
for comparison. The A input has one 10 A ceramic fuse in front of all
shunts ([power-meter-and-misconnect.md](power-meter-and-misconnect.md)
section 5); its time-current behaviour is unknown: keep a motor's start
(5-10x its run current) short and the run current under 10 A.

| step | how | pass |
|---|---|---|
| entry | A function, MF -> Inrush | AC A 10 A, "Armed: 0.000 A now" after ~0.3 s; `INR:COND?` ARMED; `DBG:ISR?` late / slips 0 |
| lamp (12 V car bulb on a bench supply, or 230 V 40-100 W with care) | switch it on | "Triggered", 2 s later the result + "Saved LOGS/FASTnnn.DLG"; peak ~5-10x the run current; Irms 100 ms between the peak and the run current; the trace a decay over ~50-200 ms |
| second meter | an inrush clamp (Fluke 376 / 323 / UT21x) or the scope + shunt on the same wire | Irms 100 ms within ~5 % of the clamp's inrush value (clamps differ in their window start); peak vs the scope's |
| motor (drill, fan, small pump) | switch it on | trigger; the start-up band; `INR:FETC?` lost 0 |
| running load | motor running, `INR:ARM`, wait 30 s | stays ARMED; a second load on the same wire triggers |
| file | `MMEM:DATA? "LOGS/FASTnnn.CSV"` or the USB volume | 19 200 rows, t0 at 0.2 s, values in A; `tools/dlog.py` decodes the DLG |
| DMM after | Back; DC A / AC A on a steady load | the reading as before; `FUNC?` = the function before the page |
| fuse | after the motor runs: MF -> Fuse check | FUSE OK |
| offset / AC gain | nothing connected: `INR:ARM`, Idle; a 50 / 60 Hz load: Irms 100 ms vs the DMM's AC A | Idle < ~20 mA; Irms within ~1 % of AC A (decides the AD2 gain on AC at x1, section 8) |
| host polling | `INR:ARM`, then `INR:FETC?` + `INR:DATA? 0,3000` + `DBG:ISR?` every 0.2 s through a lamp capture | lost 0, `DBG:ISR?` late 0 (region C SCPI during a capture) |

Also note the `DBG:PROF?` loop maximum during a capture and whether a toast
or redraw while armed caused a hole (`lost`).

## 10. Budgets

The capture adds ~5.4 KB to region B (the drain, the detector, the live
state line, the state queries), no SRAM (the trace lives in BUF) and ~200 B
of stack (the session state is a TCM static); measured on 32d4d4b. The
zero's measurement (`Cap::zero_in`, run with the stream) added ~0.2 KB to
region B (18532a6). Region C
holds app_inrush.rs, the page and trace drawing, `Cap::results` /
`linearize` / `records`, the SCPI fetch / data / setters. Current numbers:
[native-boot.md](native-boot.md) "Memory budget".

## 11. Limits

- 9600 S/s only: 19 200 / 38 400 S/s would halve / quarter the trace in
  BUF. The 104 us grid and the ~2.4 kHz AD2 band are enough for
  mains-frequency inrush; a sub-ms spike's height is under-read.
- One capture at a time; a re-arm replaces it on the page (the file stays).
- The threshold is not persisted (RAM, default 1 A).
- The 6 A range (x10, +-12.5 A) is not offered: an inrush past 12.5 A would
  clip.
- The trace is not zoomable; the CSV has every sample.
- The zero is measured at the arm: a DC current below 0.2 A that already
  flows then (with less than 50 mA rms on it) is taken for the zero, and
  the results read relative to it. With a larger or AC load running at the
  arm the calibration's offset stays (0 on the AC A rows), i.e. the AD2
  offset (~0.07 A on the unit checked) is not taken out.
