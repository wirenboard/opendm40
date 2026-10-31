# opendm40 TODO

## Goal

> feature parity со стоковой прошивкой в режиме мультиметра, осциллографа, генератора, включая bluetooth. по usb - usbtmc. В отличие от стоковй прошивке не надо "переключать" режимы, рбаотать должно быть всё одновременно. Переключаться должен только режим отображения: каждый на своём экране (как в исходной) или какие-то комбинации одновременно (сам придумай как лучше). Меню тоже надо сделать.

Additional requirements:
- Main DMM value drawn in red.
- No Alientek logo in menus.
- Touch incl. side keys and gestures (swipes), with debounce and long-press filtering.
- DMM digits and chart redraw as fast as stock (~6 ms).
- Both analog rails on by default.
- USB composite = USBTMC + mass-storage disk, always present (like the stock disk mode).
- Debug commands live in USBTMC as `DBG:*` SCPI commands (no CDC).
- Data-logger mode (see FR-38 in `../docs/history/dm40-feature-request.md`).

- [ ] A current in the A jack is missed or misnamed in some modes (device, 2026-10-26: owner's PSU 20 mA CC
      into A / COM, V empty; the same on main 0.2.5 and on the #25 + #26 test build).
      - AC mV: no alert; the readout reads OL.
      - Ohm: LIVE instead of A-JACK (31 MΩ). Device 2026-10-29 (600 MΩ branch): the same on 600 Ω, 60 MΩ,
        AUTO and 600 MΩ; the ~31 MΩ comes with the PSU's COM lead alone (a ground loop, inferred), not the A current.
      - Hz: no alert.
      DC V / AC V / continuity / diode / capacitance raise A-JACK in 0.9-1.8 s. On main, DC mV also raised a false
      A-PROT at 20 mA (limit 125 mA), latched in every later mode; #25 removes it.
      The cause in AC mV / Ohm / Hz is not traced (sentinel.rs decide / V_GATE per mode).
      Logs: scratchpad acur4.log (test build), acur-main.log.
      Open PR #44 (draft): the fix; its device check needs a current in A.
- [ ] A-JACK thresholds higher (owner 2026-10-27). The PB1 comparator window is the stock R21 0xdd, ±20.8 mV
      = 0.44 mA on 47.5 Ω; A-PROT needs only a trigger for the probe (125 mA = 5.9 V at PB1). The taps allow
      ±62.5 / 104 / 208 / 417 mV (AGNDP3..6 / AGNDN3..6, REFO 1.25 V: 1.3 / 2.2 / 4.4 / 8.8 mA); the AD1 / AD2
      peak thresholds (`PK_THR`, `PK2_THR`, 0.5 mA) are in software.
      - [x] PB1 measured (device, 2026-10-27, 0.3.0 + the tools of branch `exp/pb1-meas`, A open, manual
        ranges, V–COM from the calibrator). `DBG:SENT:PB1?` writes the running row with AD1 on PB1 − PB3, then
        on PB1 − AGND (IG 0.9, REFO / AGND, SAD1I = 00), and writes the row back. Table:
        [hw/README.md, Measured node behaviour](../hw/README.md#measured-node-behaviour) ("PB1 with AD1").
        Up to 3 V the comparator rows (DC V, DC mV, ohm, temperature) keep PB1 within 13 mV of AGND; DC mV,
        ohm and temperature reach 155..264 mV at 3.7 V, 353..698 mV at 5 V and 1.29..>1.39 V at 10 V. DC V
        (every range) stays at 2.2 mV up to 10 V. Not measured: continuity and diode (their comparator drives
        the buzzer at any volume), A shorted to COM.
      - [x] Comparator window per R21 (`DBG:SENT:R21`; 10 CTSTA reads per point; DC V 6 V, DC mV, ohm 600 Ω /
        60 MΩ, temperature, DC V 60 / 600 V with variant 4):
        - 0xdd, 0xcc, 0xbb and AGNDP4..6 over AGNDN3 (0xab, 0x9b, 0x8b): CTSTA 0 from V–COM off to 3 V; DC V
          6 V (0xdd .. 0x8b) also at 5 / 10 V.
        - AGNDN4..6 as the low threshold (0xaa, 0x99, 0x88, 0xba): CTSTA 0x10 with nothing connected, A-JACK in
          every row. These windows cannot be used.
        - 3.7 V: ohm 60 MΩ, DC mV and temperature trip at every window (0x70; 0x50 at 0x9b / 0x8b), ohm 600 Ω
          at none. 5 / 10 V: ohm 600 Ω / 60 MΩ, DC mV and temperature trip at 0xdd, 0xcc, 0xbb and 0x8b.
        - DC V 60 / 600 V: CTSTA 0x50 and A-JACK at every R21, also with V–COM off, while AD1 reads PB1 at
          ~2 mV. That is no pickup: in these rows the comparator does not compare PB1 with the AGND taps
          (cause *unknown*). No window brings that sentinel back.
      - Result: no window change. No measured pickup lies between ±20.8 mV and the usable maximum (+417 /
        -62.5 mV): up to 3 V every row stays inside 0xdd, and from 3.7 V every row that trips also trips at
        0xbb / 0x8b (at 5 V PB1 − AGND is 353..698 mV there, at 10 V 1.29 V or more). The pickup rows
        keep their V gates (ohm / temperature: V_GATE and the jack check; DC mV: #35). The firmware keeps
        0xdd (0.44 mA). Owner decision: keep it, or 0xbb (1.3 mA; nothing measured gets fixed).
      - Hz: the standing ~54 mV (device d8e249d) came with the row's R33 SAD1I = 11, the AD1 temperature
        sensor inputs (*inferred*). With SAD1I = 00, PB1 is 2.5 mV at rest and follows the input from 3 V on.
        An AD1 sentinel in Hz (#44) needs SAD1I = 00; PB1 − PB3 there reaches -116 / -140 mV at 5 / 10 V
        (#44's `PK_DC_THR` is ~180 mV, one-sided). PB3 moves too (Hz 10 V: PB3 − AGND +431 mV).
      - Left (owner): the rows with A shorted to COM; continuity / diode; a real current in A
        (20 mA and ~1 mA) per row. Tool: branch `exp/pb1-meas` (`DBG:SENT:PB1? [n]`, `DBG:SENT:R21 <v>`;
        build with `--features mute`; not for merging, region B -1.5 KB).
- [ ] Experimental 600 MΩ (ohm r7, [ohm-600m.md](docs/ohm-600m.md)): check it against a resistor of
      100 MΩ .. 1 GΩ known from another meter; the open-jack zero (Vfb − Vx, `DBG:OHM6?`) and its drift;
      the settling after a resistor is connected; then `DBG:OHM6` gain / offset as constants. Checked so
      far up to 5.57 MΩ only. The leak is steady over 13 min (±1.5 %), 0.041 .. 0.059 nS over an evening. Also: whether the leak (learned on open leads, ~21 GΩ on one unit) is a
      resistor across the jacks or depends otherwise on the jack voltage; the jack check (AD2 = OP1O) reads
      FB in phase B, so A-JACK there leans on V_GATE (`live`, every ~2 s): a current in A on 600 MΩ gives LIVE, as
      every ohm range does (the A-jack item above).

- [ ] Scope extra channels (owner 2026-10-27: "dmm (wave) as a second channel of the scope, optional; the generator
      as a channel too, as a trigger source, computed, not captured"): built on branch claude/friendly-gauss-8m2e3g,
      design and device checks in [docs/scope-gen-channel.md](docs/scope-gen-channel.md).
      - GEN: MF > Trigger > Source GEN (computed from TIM6 / DMA or TIM12 against TIM1), MF > Display > GEN trace,
        `TRIG:SOUR GEN`. Device check 1.5 (the generator looped back to CH1); `DBG:GENTRIG:LAG` to measure.
      - CH2: MF > Display > CH2 (DMM), the AD2 stream time-aligned by its DWT stamps; the DMM reading pauses.
        Device check 2.4; `DBG:CH2:LAG` to measure. Not yet: a CH2 position (0 is the centre row), CH2 in roll
        beyond 3.4 s of screen, CH2 in the CSV / WAV:DATA?, CH2 measurements, CH2 as a trigger source.

## Open, by kind (2026-10-27)

One line per group of open items; quoted words are the start of an item. Open PRs (#35, #41, #44, #55, #56, #57,
#60, #61) are marked at their items.

**(a) Needs the owner on the device**
- A current through the A jack: the two A-jack items above (#44; #35's 66 kHz V-COM case too), "Device T9 / T10",
  "Device retest "dmm-bugs"", "Device: doc 4.10" ([danger alarm](#loud-danger-alarm-owner-2026-10-24-dangerous-misconnects-beep-at-maximum-volume-volume-kept-at-0)),
  "Device: doc 4.13" item 4, the power meter's "Verify on device" lists (§3.12 / §3.16 / §3.17), "(A) device test
  of the Wave view" (its current step), "Make method C the AC+DC default" (A / mA / µA ranges).
- "Owner-only device checks" ([below](#owner-decisions-pending-2026-10-25)): generator ON, AC+DC mV with a current
  in A, DC V against a third reference.
- Physical keys, touch, sound: "Device check (3 min): (1) `CALC:AHOL?`" (side keys, #57 partial), the
  [trigger-level drag](#scope-trigger-level-drag-lag-owner-2026-10-25-branch-fixscope-drag-lag) checks, "T3 remaining
  on the device", the "UI/UX review punch-list" remainder, UF-01 (beeper latency).
- Screen changes: the [jerky screen changes](#jerky-screen-changes-owner-2026-10-29-branch-fixsmooth-redraw) "Device
  check (2 min" (PR fix/smooth-redraw); the [strip rendering](#strip-rendering-owner-2026-10-30-branch-featstrip-render)
  "Device check (3 min" and `DBG:TE?`.
- Power cycles, a cold meter: "Device: VOS over a warm-up" (~1 h), "Device: the disk-buffer and preparation cases".
- Leads shorted and a known R: the ohm VOS that "Unchopped FAST in ohm" needs first; "Ohm gain with a DC
  offset"; "Ohm OL above 60 000 counts" (calibrator); "Experimental 600 MΩ" (100 MΩ .. 1 GΩ resistors).
- Calibrator or source set by hand: the [Hz below 7 Hz](#hz-below-7-hz-owner-2026-10-25-v020-with-the-mmb30-2-hz-and-5-hz-gave-no-reading-branch-fixlow-freq-merged-43-v030)
  check (MMB3.0 frequency), the [500 ohm](#500-ohm-from-the-mmb30-reads-ol-on-600-ohm-owner-2026-10-25-v020-branch-fixohm500-ol-merged-7)
  check (MMB3.0 500 Ω, a passive resistor), "E7 open" and "Method C's internal zero on 60 V" (> 30 V).
- Generator on / looped back to CH1: "Scope in ngscopeclient" (3), (9); RUN key: its (2), (7).
- Host systems and the stock: Windows / macOS in "Verify on device: swipe REC" and the two "Device: (... decision
  #6)" items; stock comparisons UF-02, UF-19.
- USB cable and charger: the [charging indicator](#battery-charging-indicator-and-usb-badge-user-report-2026-10-28-branch-fixbattery-charging-indicator)
  checks.

**(b) Agent-doable: code, docs, device runs without a human (quiet-device rules, `--features mute`, #53)**
- Code: none open. "A stream starting inside a multi-block READ(10)" (#60, test only) and
  "SCOP006.CSV once saved" (#61) are in PRs; "Unchopped FAST in ohm" is under (a), "Left: `vdisk::mirror_boot`"
  under (c).
- Unknowns to trace: "Why the two conversions after an early R22 change", "AUTO coupling decided at 5.76-5.85 s",
  "76800 CHOP ON", "C's lag-side creep allowance", "C's second reference".
- Device runs: "Fast logger device check" (open steps), "(B) device session", "Device check (AUTO coupling)",
  "Device: doc 4.13" (the mV-first AC AUTO, RTD / NTC).

**(c) Owner decisions**
- [Owner decisions pending](#owner-decisions-pending-2026-10-25): the three "Docs, owner decision" items (front photo,
  font licence, AI policy), "Delete the bt-protocol branch", USB PID (waits for pid.codes PR 1331).
- Merges: open PRs #41, #55, #56, #57, #60, #61; drafts #35, #44 (device check first) and #47 (`DBG:HY:CAP?`).
- Defaults and rules: "Make method C the AC+DC default", "DMM reads at /8" (decision #8), the crosstalk item's
  partial-gate value at signal off, the long press on a toast ("Owner question (not changed)").
- Boot time (bug-fix-only rule): "Left: `vdisk::mirror_boot`" (0.3 s per boot).
- New features (bug-fix-only rule): "UF-24 tempco", [Backlog](#backlog-user-ideas) "Inductance", "Browser client",
  "Protocol decoders", "Hardware note (HW-1", "sigrok/SmuView" (SmuView on the device, upstream).

## Battery charging indicator and USB badge (user report 2026-10-28, branch fix/battery-charging-indicator)

User: the battery icon does not show charging; ideally show the PC connection. Fix (#68): PB11 with the stock
pull-up; green on the charger (PB10 = 1), the top bar blinking while charging (PB11 = 1), steady when charged
(PB11 = STDBY low); a USB symbol while a host has the meter configured. Pin findings: hw/README.md pin table.
Device 2026-10-29 (battery full, 4.23 V): charged state, replug (charging 3.5..4.5 s, then charged), PB10 low
/ released checked with `DBG:CHG?` / `DBG:CHG:LOG?`.
- [ ] Device check with a battery below 80 % (needs hours on battery first): the top bar blinks while charging;
      the battery voltage rises (`DBG:CHG?` battery ADC); charge time vs the stock's ~2 h 35 min.
- [x] The stock's PB10 drive measured on opendm40 (2026-10-29, hw/README.md Unknowns 5): PB10 low stops the
      charge, released the charger goes to standby until the next replug. opendm40 does not drive PB10 and
      charges to termination. The stock pulses PB10 low ~10 ms per 2 s below 80..85 % and reads the battery
      during the pulse (disassembly; purpose *inferred*: a reading without the charge current).
- [ ] (c) Owner decision: copy the stock's 10 ms PB10 pulse for the battery reading on USB power (the bars
      read high while charging without it), or keep PB10 undriven.
- [ ] A power-key reboot after a replug (the user's "shows again after a reboot"): not tried; a software reset
      keeps the charged state.

## Owner decisions pending (2026-10-25)

What waits for the owner; each line links to the material.

- [x] DC rate / filter: option B or E ([dmm-rate-filter-survey.md](docs/dmm-rate-filter-survey.md) §7-8 and "Option E";
      device numbers in [device-check-2026-10-21.md](../docs/history/device-check-2026-10-21.md) "Option E"). Recommendation: B.
      **Owner 2026-10-25: FAST unchopped with a firmware offset, MED / SLOW chopped, all three rejecting 50 / 60 Hz**
      (the "bug fixes only" rule does not apply to this one). Done on branch feat/rate-unchopped-fast:
      [fastvos.rs](src/fastvos.rs), dmm.rs FR-05 (the rate's R22 in the row), survey §8.4, the manual;
      emu/hy3131/fastratetest.py.
      - [x] Device 1184b096: SENS:RATE:PER? 0.10 / 0.20 / 0.40 s (FAST / MED / SLOW), ohm 0.2 / 0.4 / 0.4 s;
            0 V on 6 V FAST - MED +24 uV; ohm 300 ohm FAST = MED within 0.6 mohm.
      - [x] Device 1184b096: VOS per path differs (6 V 13 235, 60 V+ ~12 600, A 6 A 12 865: survey §8.4):
            per-path defaults in fastvos VOS_DEFAULT (60 mV not measured).
      - [x] Device 1184b096: the -95 ppm gain was wrong (FAST - MED -87 / -90 ppm with it, alternating blocks):
            GAIN = 1. The refresh gate rejected 20 of 45 on the calibrator's noisy 1 V..24 V output: now the
            00-mean shift against the groups' scatter.
      - [x] Device 4d8ea4e6 (rebased): the gate change and the defaults: 1 V calibrator output, 65 s FAST: 2 used,
            0 rejected; 10 V on 60 / 600 V, FAST in the first 2 s after the range change +0.3 / +0.4 counts from MED,
            after the refresh -0.3 / +0.1.
      - [x] 60 mV (device 71a85c8d, open input): 2 refreshes used, 2 rejected in 60 s; the rejected ones fell on
            the open input's wander (10 s spans of 5..76 uV), so the gate works there. On the calibrator's 0 V
            output (~120 uV sd at that scale) no refresh starts (SIG_MAX), as intended; that VOS stays the default.
      - [x] Device 71a85c8d, room temperature: FAST on 6 V, output off, 25 min: 53 refreshes used, 0 rejected;
            VOS 13 239..13 257 codes (+-15 uV), per-minute reading means 108..144 uV, no wild reading.
      - [ ] Device: VOS over a warm-up (a cold meter, ~1 h, FAST on 6 V, DBG:FAST? every minute).
      - [x] Device 71a85c8d: the wild +-X read pairs (survey §8.4) do not occur at a fixed R22: AD1 logger 300 s
            at 0x06 (2990 records) and at 0x16 (1493), none beyond 3000 codes but the session's first record.
            With FAST's polarity writes (2 per 30 s) none in the 25 min run above.
      - [ ] Unchopped FAST in ohm (an offset per range of the autorange): not done, ohm FAST = 0x16 for now.
            Needs the device first: VOS per ohm range (R22 0x06 against 0x16, leads shorted and a known R) for
            fastvos VOS_DEFAULT, which covers the first 2 s after every autorange step; whether ohm's unchopped
            conversion reads VX +- VOS as DC V does is not known. The emulator's one `ad1vos` supplies neither.
      - [ ] Ohm OL above 60 000 counts (owner 2026-10-29). Ohm is ratiometric (AD1 = 0.9 · Rx / Rref), so at
            60 000 counts AD1 is at 0.54 FS on r0..r5; AD1 saturates at 1.11 · Rref = 1.85 × the range (r6, Rref
            63 M: 70 MΩ, 1.17 ×). OL is set by `poll_ohm` (`kraw / lim > over()`), not by the converter. Gain:
            10 × the resolution for 1.0..1.85 × the range (700 Ω on 600 Ω as 700.00 instead of 0.7000 k on 6 k).
            Needs: linearity above 60 000 counts against the calibrator; a 6-digit format in the readout,
            history, chart, BT and CSV.
      - [ ] Ohm gain with a DC offset (inferred, owner 2026-10-29). AD1OS (R22 7:5, DCOS ±0.25..0.75 FS) is
            derived from the AD1 reference, so in ohm it scales with the Rref drop and keeps the ratio. AD1IG 2.7
            + DCOS -0.75 would fit 0..648 Ω on the 600 Ω range at 226 codes per count (75.5 now). It helps only
            if AD1 is the noise source (~54 codes now). Device, FAST (unchopped): noise on 600 Ω at IG 2.7 +
            DCOS -0.75 against IG 0.9. Also find out whether AD1CHOP 01 inverts DCOS (in DC V FAST at 0 V with
            AD1OS 101: does the fastvos VOS estimate move by -0.25 FS?). With chopping (MED / SLOW) DCOS may
            cancel. The Hycon reference app and Temco use DCOS -0.25 only unchopped (capacitance charge,
            [hy3131-firmware-comparison.md](docs/hy3131-firmware-comparison.md#capacitance)).
- [x] After a power-off (owner 2026-10-25): a new measurement. DC V as the stock, and the measurement settings
      at their defaults: ranges / AUTO flags, AutoHold, limits, rate, smoothing, the scope's acquisition, trigger,
      math and measurements, the power meter, the Record page. Preferences are kept (display, sound, power, BT,
      alerts, units, sensor, scope look and touch, the generator's setup). A reset or an update continues all.
      Done: app.rs `restore` (`same_session` = from the backup SRAM after a normal boot); emu/settingstest.py PASS.
      Device check: AC V, AutoHold on, a limit set, 10 ms/div on the scope; power off with the key and on: DC V,
      AutoHold off, no limit, the scope's defaults, the brightness kept; the same after `DBG:RESET`: all kept.
      **Device 705d2e9 (2026-10-25): PASS.** AC V 60 V manual, AutoHold, upper limit 5 V, 10 ms/div; power key off /
      on: SETTINGS,FILE, DC V, AUTO, AutoHold 0, limits off, 200 us/div, the alarm LOUD kept. The same set, then
      `DBG:RESET`: SETTINGS,BKP, AC V, AutoHold 1, limit on, 10 ms/div; the fixed range came back as AUTO (as the
      manual says: a fixed range is never kept).
- [ ] Mode Remember (user report 2026-10-28: "ohm on the low range, trend 640 ms/div; off / on -> DC V and
      320"; asked for a user choice and slower trend steps). The stock has the switch: Settings > Other
      Settings > Mode Remember, default OFF (config.bin 0x1ab; its record: function, V / A coupling, V / A
      autorange flags, ohm variant, chart rate; no fixed range). Done (branch fix/power-on-state, 0.4.0):
      Settings 3/3 row 9 "Mode Remember" OFF / ON, SCPI `MEMory:STATe:RECall:AUTO`; ON restores after a
      power-off the function + coupling, AUTO / AUTO+, the ohm test current, a fixed V / A / ohm range (not
      under REL / in AUTO coupling), rate + smoothing, the trend timebase (app.rs `restore`, W_DMM_EXT bits
      20..27, no record version change; 0 = the old behaviour). Not AutoHold, limits, REL, HOLD, the scope,
      the power meter's settings, the Record page. The trend timebase is now persisted (a reset kept it
      neither before) and has 1.28 / 2.56 s/div (ui::TREND_RATES; the 321-point ring is unchanged).
      emu/settingstest.py cases `remember` / `default` PASS.
      Device check (3 min, any input, the mute build is fine):
      1. Settings > More > More: the last row reads **Mode Remember OFF**; tap: ON.
      2. Multimeter: Ω, range page 600 Ω; rate chip to FAST; tap the trend's timebase label through 640 ms,
         1.28 s, 2.56 s, 160 ms, 320 ms and 640 ms to 1.28 s/div (the label fits its chip, the chart runs).
      3. Wait 10 s, power off with the key, power on: Ω, **600Ω** (not AUTO) in the status bar, FAST,
         1.28 s/div. `MEM:STAT:REC:AUTO?` 1.
      4. Mode Remember OFF, wait 10 s, power off / on: DC V, AUTO+, MED, 320 ms/div.
      5. `DBG:RESET` with Mode Remember OFF in Ω 600 Ω: Ω in AUTO (a reset continues the function, a
         fixed range only with Mode Remember ON), the timebase kept.
- [x] RES AUTO: include the 60 Ω range or not. Owner 2026-10-25: yes. Done in 1f264d3e (dmm.rs `poll_ohm`,
      `ohm_auto_floor`). The stock autorange never selects r0 (its clamp is r1..r6,
      `stock/re/analysis/dmm_v2_6.h` (in the private analysis repo) section 2), so the step uses the stock rule of the other ohm steps:
      600 -> 60 Ω below 0.1 x limit / over = 58.25 Ω, 60 -> 600 Ω above limit x over = 61.8 Ω. With 0.28 mA ON
      AUTO stops at 600 Ω (60 Ω always drives 0.90 mA); a manual 60 Ω still works. emu/hy3131/resautotest.py PASS.
      Device check (calibrator on V-COM, RES AUTO, `RES:RANG?` and the status bar after each step):
      1. 10 Ω and 50 Ω: 60 Ω range, milliohm digits, test current I 0.90 mA; no range change while it stays.
      2. 100 Ω and 300 Ω: 600 Ω range. Then 50 Ω -> 300 Ω -> 50 Ω: 60 -> 600 -> 60 Ω, no back-and-forth.
      3. Near the band, if the calibrator resolves it: 57 Ω from 600 Ω -> 60 Ω; 60 Ω from either range stays.
      4. Open input (output off and the leads removed), from 60 Ω: AUTO ends on 60 MΩ with OL, no step down.
      5. RES AUTO on 50 Ω (60 Ω range), then 0.28 mA ON on the range page: 600 Ω with I 0.28 mA, and AUTO stays
         there; ON off again: back to 60 Ω. With ON the 60 Ω item still selects 60 Ω (I 0.90 mA).
      6. Manual 60 Ω, AUTO key held 1 s: AUTO, still on 60 Ω; short presses cycle 60 Ω .. 60 MΩ as before.
- [x] [emu/hy3131/resautotest.py](../emu/hy3131/resautotest.py) is not stable on 058a1d36 (2026-10-25, host load ~5-6):
      in whole runs `r30-300-30` failed 4 of 5 (ranges 60, 60 M, 6000, 600, 60: the first step down from 60 MΩ is
      4 rows, not 5, so 4 changes, not 3) and `r100k-key` 1 of 5 (MANU on 4 polls after AUTO); `r30-300-30` alone
      failed 1 of 4. The cause is unknown (inferred: the scenario is the run's last, where its scheduled input
      changes drift against the firmware's uptime, the file's own note on `later`).
      With the 60 Ω range removed, `r30-300-30` stays on 600 Ω (no range change) and passed in a whole run;
      `r100k-key` failed that run (MANU on 3 polls after AUTO) and passed alone.
      Cause (`r100k-key`, firmware): the AUTO key held 1 s ([app.rs](src/app.rs) `auto_key_end`, long press)
      set the autorange without `auto_entered`, so the last manual reading stayed shown with MANU until the
      next conversion; on ohm RES:RANG? stepped 6 M -> 600 k at +1.25 s under it and MANU lasted to
      +1.43 .. 1.48 s after the DBG:KEY, against the row's 1.5 s grace (`late`): pass or fail on the
      conversion phase. Fix (0.2.1): `auto_key_end` calls `auto_entered`; AUTO and dashes at +1.01 s.
      The test: the windows sit on each input's logged fw uptime (emu/emusync.py `Markers`), not on
      BOOT_OFS 0.906 (the boot is ~2.8 s: the inputs, `later` ones too, came ~1.9 s early on the fw clock;
      now `later` lands at +4.95 / +9.95 s for +5 / +10); the held-key row fails when the range leaves the
      manual one while the bar reads MANU. Results: new test on the old firmware 3 of 3 FAIL; on 0.2.1
      `r100k-key` alone 3 of 3 and whole runs 4 of 4 (4 in parallel, host load ~8) PASS; autokeytest.py
      PASS. The old `r30-300-30` failures were with the 60 Ω range (its own cause not found, branch
      feat/ohm60) and do not occur on main.
      (verify on device: RES, `CONF:RES 6E6` on a 100 kΩ resistor, AUTO key held 1 s: the status bar shows
      AUTO and dashes at the beep, never the 6 MΩ reading with MANU after it.)
- [x] Repository split (owner 2026-10-25): stock/re, the stock-UI notes, genuine-check, fsloader-install-trigger,
      stock-scope-ui, native-review / native-boot-plan / native-boot-review and acdc-stock-vs-opendm40 (with their
      old fw-rs/ names) are in a private repository, cloned into stock/re/. The public
      history was rewritten (git-filter-repo: those paths removed; the UID, its derived block and two BT MACs
      replaced); a rescan in every text form and in the blobs finds none. Doc hashes mapped to the new history.
- [x] RES AUTO with 60 Ω (device 59bffd8a, 2026-10-25): the calibrator's resistance output on V-COM, AUTO:
      10 / 50 Ω -> 60 Ω range, 100 / 300 Ω -> 600 Ω, back to 50 / 10 Ω -> 60 Ω; the output off -> 60 MΩ (the
      calibrator's off state reads ~50 MΩ, not open). PASS. Open: on 60 Ω the readings were 8.88 Ω for 10 Ω and
      48.13 Ω for 50 Ω (100 Ω read 100.13, 300 Ω 302.25); whether that is the 60 Ω row's zero / the leads or the
      calibrator's low-resistance accuracy is unknown (a 4-wire reference or REL on shorted leads decides).
- [x] Superseded (next item: the range was removed, then came back in #42). 60 Ω range gain (fix/ohm60-gain): device 2026-10-25, manual ranges, mean of 5 READ?: 10 / 20 / 50 Ω read
      0.9730 / 0.9734 / 0.9738 x the 600 Ω range. Cause: r0's calibration slot 0x124[0] is the unused 1.005 and
      the row reads 2.9 % below the stock scale (Rref 0.1 k) for a reason not known (the OP1 x10 pairs of the
      record point the other way; [dmm.rs](src/dmm.rs) `OHM_R0_K`). Fix: r0 gain = r1's calibration gain x 1.0291
      (`ohm_row_gain`), fitted to those same points, so the check below confirms the fit, not an independent
      calibration; another meter may need its own factor. Emulator: the r0 source models the same factor
      ([HY3131.cs](../emu/hy3131/HY3131.cs) `OhmR0K`), [resautotest.py](../emu/hy3131/resautotest.py) checks
      30 / 59.5 Ω on 60 Ω within 0.3 %. Device check: the calibrator's resistance output on V-COM, 10 / 20 / 50 Ω,
      manual 60 Ω and manual 600 Ω, mean of 5 READ? each: the ratio 60 / 600 within 1 +- 0.003 for each value.
- [x] 60 Ω range: removed from main 2026-10-25 (owner); back in v0.3.0 (#42, merged 2026-10-26):
      the range page item, RES AUTO down to 60 Ω, the AUTO key cycle, `RES:RANG 60`. Earlier findings:
      the stock V2.6 never selects r0 (its range is clamped to 1..6, `stock/re/analysis/dmm_v2_6.h` (in the
      private analysis repo) section 2); r0's calibration slot 0x124[0] holds a round 1.005, an uncalibrated
      default (inferred); the stock r0 row is the r1 row plus R25 0x01 (OPS1, OP1 x10) and R2B 0x46 vs 0x40
      (PA2 DS2 + FS2 closed).
      **PA2 (device 2026-10-26, MR9270 resistance output on V-COM, manual 60 / 600 Ω, 2 alternating blocks of
      6 READ? (MED) per value, r0 read with r1's gain x 1.0291 in both images):**

      | set Ω | 600 Ω | 60 Ω, R2B 0x46 | 600 Ω | 60 Ω, R2B 0x40 | raw r0 / r1, 0x46 / 0x40 |
      |---|---|---|---|---|---|
      | 0 | 0.675 | 0.669 | 0.669 | 0.689 | |
      | 10 | 9.112 | 9.102 | 9.114 | 9.461 | 0.9707 / 1.0087 |
      | 20 | 19.079 | 19.077 | 19.085 | 19.829 | 0.9716 / 1.0096 |
      | 50 | 49.417 | 49.399 | 49.414 | 51.354 | 0.9714 / 1.0099 |

      PA2 DS2 / FS2 closed reads 3.8 % lower; what PA2 is wired to on the board is not known. With it open
      r0 reads 1.0 % high; the record's DC mV x10 / x1 gains (0x24 / 0x28: a ratio of 0.9895 on the dumped
      unit, the same OP1 x10 stage) match that. Change: r0 = the stock row with R2B 0x40
      ([dmm.rs](src/dmm.rs) `OHM_R0_ROW`), gain = r1's x 0x24 / 0x28 (`ohm_row_gain`), no measured factor.
      Predicted from the table: r0 / r1 = 0.9981 / 0.9990 / 0.9993 at 10 / 20 / 50 Ω. Device 8d1ef2bd
      (0.3.0), same method: 600 Ω / 60 Ω = 0.669 / 0.666 (0 Ω), 9.102 / 9.090, 19.084 / 19.059, 49.417 /
      49.375, r0 / r1 = 0.9987 / 0.9987 / 0.9991; 100 and 300 Ω on manual 60 Ω: OL (600 Ω: 100.11, 302.26).
      RES AUTO: 50 / 10 Ω -> 60 Ω; 100 / 300 Ω -> 600 Ω (from 60 Ω by way of 60 MΩ, the stock over-range
      rule); calibrator steps 57 .. 63 Ω up: 60 Ω through 62, 600 Ω at 63 (62.82); down: 600 Ω through 59
      (58.61), 60 Ω at 58 (57.66); no back-and-forth. Output off from 60 Ω: 60 MΩ (the calibrator's off
      state reads ~96 kΩ there, not open). The 0 Ω row is the
      calibrator's own residual (~0.67 Ω at "0"; 10 Ω reads 9.11 on 600 Ω), not the meter's zero: whether
      the MR9270 is accurate at 10 Ω is unknown; a short at the jacks or a 4-wire reference decides. Region B
      paid with the LOCK font blob moved to A2 ([native-boot.md](docs/native-boot.md)).
- [x] Touch (device eea7cd6 build, 2026-10-25): the stale 0xAB reads were the cause of the lost taps and the
      mixed-up keys. touch-tap-check before: every still tap LOST; after: every tap D T. Offsets to the aims:
      -2,+2 (top-left), +11,+3 (top-right), +2,+20 (bottom-left, y at the 319 edge), +11,+17 (bottom-right),
      -1,+9 (centre). The stock applies no scale or offset either (stock/re/analysis/touch_v2_6.h); whether the
      remaining offsets are the panel or the aim is unknown: the touch test screen (owner request) shows it.
      AUTO key cycle on the device (DC V, 1 V in): 6 V -> 60 V -> 600 V -> 1000 V -> 60 mV -> 600 mV -> 6 V,
      held 1.2 s -> AUTO+: PASS. The status bar shows "60V" on a fixed 60 V range (screenshot): PASS.
- [ ] USB PID (owner 2026-10-25; repo public 2026-10-26, request open: https://github.com/pidcodes/pidcodes.github.com/pull/1331; firmware already uses D040): request 1209:D040 from pid.codes right after the repo is public (their rule:
      a public source repo with a LICENSE). The PR is prepared locally (an org entry + 1209/D040, MIT, site and
      source = github.com/opendm40/opendm40): fork pidcodes/pidcodes.github.com, push, open the PR. After the
      merge: nothing left in opendm40 (firmware, udev rules and docs use 1209:D040 since v0.2.0). PR 1331 still
      open on 2026-10-27.
      Their howto: a software-only project "may" be asked for justification (the hardware is the vendor's).
- [x] Public release (owner 2026-10-25): done; the repository is public (2026-10-26), v0.2.0 and v0.3.0 (#51)
      released through CI. The one blocker was the touch fix (screen and the four touch keys).
      **Done (device b98717b7, 2026-10-25):** the stale 0xAB reads fixed (taps no longer lost, no phantom MENU),
      the key lag / AUTO-RUN mix-up fixed, a per-device touch calibration on the touch test page. The owner
      calibrated this unit: x = 0.9887x - 0.0076y + 0.08, y = 0.0057x + 0.9555y - 2.77 (DBG:TOUCH:CAL?), "the
      calibration helps". The four touch keys are reported by the controller as fixed points (x 372 / 627 / 883
      / 1140); a touch between two keys gives no report (device log), so there is nothing to show or map there. Still
      to do at that time, not blockers: the licence of stock/patch, the vendor .bin and the datasheets in docs/;
      delete the rust-fw-poc and bt-protocol branches; cut v0.1.0-pre2 through CI and drop v0.1-pre1.
      2026-10-27: rust-fw-poc is gone; v0.2.0 replaced the pre2 plan; the datasheets are in hw/datasheets (#1);
      the licence of stock/patch and the vendor .bin: status not checked here.
  - [ ] Delete the bt-protocol branch and the v0.1-pre1 pre-release (both still on GitHub 2026-10-27).
- [ ] Owner-only device checks (2026-10-27: the three are kept here; they were also listed in the items named):
      - The generator-ON output glitch with OUTP? polled ("Generator queries re-apply the generator", 469f354).
      - A current into A with V-COM open: AC+DC mV A-JACK ("AC+DC mV (manual 600 mV) A-JACK after the voltage
        goes off", 57e1241).
      - DC V against a third reference (a 6.5-digit DMM or a 2.5 / 5 V reference IC on V/COM); the MR9270's DC V
        output is only good to ~0.1 % and wanders, so it cannot check DC V to the 0.05 % class. Until then, 1 V
        readings of 0.9987..1.0006 V are the calibrator (device check 540e056).
- [ ] **Docs, owner decision:** a photo of a real DM40 running opendm40 for the front
      [README.md](../README.md). For now it shows the rendered multimeter screen (docs/manual/img/dmm.png).
- [ ] **Docs, owner decision:** the licence wording for the stock V2.6 bitmap fonts that opendm40 copies into
      its image ([src/stockfont_data.rs](src/stockfont_data.rs), [fonts.md](docs/fonts.md)). README "Licence"
      now only lists them as not under MIT, with no licence statement.
- [ ] **Docs, owner decision:** the AI-contribution policy for CONTRIBUTING.md (whether AI-written
      contributions are accepted, and how they are marked and reviewed).

## Next up (groomed 2026-10-14)

Replaced by [Open, by kind](#open-by-kind-2026-10-27) (2026-10-27). Its entries are items of their own below:
mirror-bw (done, d2aff22), UF-01, the both-rails crosstalk (closed, bb39569), T3 by hand, DBG:PROF? (the redraw
items, done), pinch ("Scope touch", verify note), UF-02 / UF-19, the faster redraw (done), UF-30 (done), sigrok,
the scope full record.

## Memory budget (2026-10-22 rework, docs/native-boot.md "Memory budget")

- [x] Stack headroom 16.8 -> 24.9 KB (ad2-isr), SRAM free 1.8 -> 12.9 KB, region B free 0.3 -> 24.5 KB;
      build checks: stackdepth --check 20480, TCM stack >= 42 KiB, SRAM >= 8 KiB free, region B >= 8 KiB
      free (warning < 16 KiB).
- [x] Device: DBG:BOOT? STACK high-water after a scope walk + FFT + screenshot (static path 21.8 KB):
      a239aa6 21 616 / 46 620 B (46 %) after scope page, popups, FFT on, AUTO, Save CSV, 3-finger shot
      (boot 18 328); docs/dmm-input-scope-feasibility.md 11.11.
- [x] Device: screenshot with the scope FFT on (40 KiB CAP while the FFT holds the tail): a239aa6
      SHOTS/S007.BMP 307 266 B holds the spectrum, the FFT keeps updating after the shot (11.11).
- [x] Device: a shot of a big screen (AC+DC / power view) then the FFT: the spectrum resumes after the shot.
      d8e249d (docs/device-check-2026-10-21.md "d8e249d" 4): W page AC+DC shot as SHOTS/S009.BMP, back on the Scope page s.fft 5012 -> 5866 in 3 s.
- [x] `.cold` (region C, not zero-wait) audit 2026-10-22, emulator hooks on every function of C (a Renode
      AddHook per symbol from t = 6 s, hit counts logged at powers of 2; DMM idle, swipe-down CSV recording
      16 s, combined view, settings pages, generator page): vwrite::poll (B) called upload::disk,
      vdisk::entries and vdisk's generic `[VF; 400][..n]` index on EVERY main-loop pass (~24k calls/s in
      idle) -> inline(always) / a plain slice; the DLG row coder (Enc::row, BitW, Ad, fmt_row's CSV length,
      crc32_upd, ~1.4 KB) ran per reading while recording -> `.text.dlog_row` (B); app_press's guard runs on
      every touch report (scope drags) -> inlined into App::gesture, the body (press_track, taps only)
      stays in C. fatdir runs ONCE per recording start (csvlog bpb), not per commit (the earlier note was
      wrong): stays in C. After: no C function runs in idle / combined view / while recording; C runs at a
      recording start / stop (fatdir, the vdisk refresh), on taps and boot. Not covered by the run: MSC reads
      by a host (vdisk read_block in C per sector: host-driven I/O, fine) and the scope FFT / AD2 stream
      (the budget run of 1de50a5: none in C). Region B: -3 KB for decision #6 + the audit (20.0 KB free on 0eefdf1).
- [x] Device: DBG:PROF? / lat_pc while recording a CSV and with the USB volume mounted (Files live: a
      vdisk refresh per commit, ~60 s) to confirm no stall from region C.
      d8e249d (docs/device-check-2026-10-21.md "d8e249d" 5): no region C stall; loop max1s 1-2.5 ms, one 74 ms pass per commit (84 ms at the
      start) in `csv` = the blocking FAT / dir write_block erase (by design); a recording appears on the
      volume at its stop, not per commit (finish -> set_stale). Region C found elsewhere (6a / 6c):
      vdisk::file_read's closure (host volume reads during a stream, max_lat 9 us: fixed) and the volume
      refresh under the Wave view's stream (below).
- [x] Region B audit #2 2026-10-22 (after the fast logger: 9.4 KB free): 30.4 KB free (ad2-isr; 37.8 KB
      without), no change in function (docs/native-boot.md "Region B audit #2"). ~20 KB of `.cold` modules'
      code was inlined into hot callers (Gallery::poll + DLG decoders in App::tick, VDisk::write_block in run,
      boot::put_info in Scpi::handle, help / temperature pages, scope Save CSV, csvlog delete_on /
      catalog_scpi_on) -> `#[inline(never)]` / patterns; poll_cap's f32 quicksort -> insertion sort. Region C
      31.7 KB left. Hooks on every C function (per-second counts): idle, CSV recording, combined view, AD2
      stream 38400 (DMA), fastlogtest typ: C only at a start / stop, but LOG:DISK? (polled during a disk session) ran
      fast_scpi in C (f5eae17) -> its reply fast_status in B. `Logger::drain` has one copy (no
      duplicate to drop). build.sh: a banner at the end below 16 KiB. emu/hy3131/ad2streamtest.py and
      emu/fastlogtest.py take `--extra-script FILE` (e.g. the hook lines).
- [x] Region B audit #6 (#54, 2026-10-27): region B 16.4 -> 50.9 KB free (`-Zlocation-detail=none`: no panic
      locations), A2 above 2 KiB ([docs/native-boot.md](docs/native-boot.md) "Region B audit #6").

## Remaining work

- [x] On-device help, Gestures page (ui.rs help table): "Scope: right edge / Drag trigger level" was
      wrong while the trigger-level marker T was on the left edge; the marker moved to the graph's
      right edge (scope_ui.rs `strip_items`), so the help text is now correct.
- [x] `tools/dm40dbg.py` ported from CDC/pyserial to `/dev/usbtmc` + `DBG:` commands (d158db9).
      `tools/reload.sh` is the native reload flow (3c20dc1); no serial/ttyACM references remain.
- [x] On-device test of USBTMC (`*IDN?`, `DBG:PING?`/`DIAG?`/`LCD?` screenshot, `DBG:TAP`/`SWIPE`) and
      the mass-storage disk (W25Q128, db29835).  (device-verified: used in every device run since;
      CALBK files written, dm40_patched.bin copied onto the disk for DBG:UPDATE)
- [x] Faster redraw of the main DMM value and trend chart (target ~6 ms like stock: render to RAM,
      single window flush, chart column ring). 95e4808 sped it up; measure with DBG:PROF? (c85afc1).
      Done: dbf0278 (d.read / d.chart prof sections), 83c2b05 (chart column-span cache, aux rows repainted
      only on change, faster stockfont::label; pixel-identical). (verify on device: DBG:PROF? d.read / d.chart)
- [x] Item-by-item parity check vs stock -> `docs/parity.md`. DMM/scope/gen/BT/GUI/touch/USB
  tabulated; punch-list extracted. Deliberate deviations recorded (red value, no logo,
  English-only, temp F 1.8 not the stock 1.82 bug, combined views, USBTMC, gestures).
  Remaining parity gaps (from the punch-list):
  - [x] BT cmd-09 flags byte: real battery (power.bars) / charging / key-lock; bits per bt_meas_v2_6.h.
  - [x] BT module startup AT init (AT+SBNDM40<model> -> if OK: AT+SLED0 -> AT+RESET), once per power-on.
  - [x] Generator defaults Square/2 MHz; snap square 50k-100k DOWN to 50k; non-square >50k = 0 Hz silent.
        (superseded by UF-14 clamp + note.)
  - [x] Touch: Goodix GT1158 fallback (addr 0x14, probe ID "1158" first, coords GUIx=GoodixY;
        side keys normalised to the same 1<<code). 0x5A path byte-identical. Untestable on this
        0x5A unit.
  - [x] Scope: slow timebases to 50 s/div (TB_MAX 29, roll from 200 ms/div, YT ceiling 1 s/div);
        stop-and-zoom re-renders the stored 26 K record at the selected timebase. Horizontal
        position = the stock T marker drag (stock scope port). Not done: full 65 K record (DMARAM budget).
  - [x] Scope hardware (comparator) trigger, src/hwtrig.rs (H0..H8 of ../docs/history/hwtrig-integration.md),
        DBG:HWTRIG[?|:ENABLE|:CAL?|:STAT?|:STAT:RESET]. On by default since 4f876cf (enabled in
        Scope::new; hwtrig::DEFAULT_ENABLED stays the module default). (device-verified: Auto/Normal/
        Single, 100 us..100 ms/div all T'D, 0 misses)
  - [x] Confirm simultaneous DMM+scope accuracy (both-rails crosstalk). Closed 2026-10-27: the fix this item waited
        for passed on bb39569 (end of the item). From "Next up": device d8e249d, the 40 Hz 0..9.6 V square reads
        +0.19 % with the scope rail on (SYST:RAIL AUTO <-> BOTH on the DMM page: +9.3 mV at 40 Hz, +14.1 mV at
        400 Hz, DC +-0.03 mV): analog, the powered scope front end couples into V / COM; no firmware fix, AUTO
        (default) keeps the scope rail off on the DMM page. Left as the owner's call: the partial-gate value at
        signal off (below). Fold in: combined-view FREQ
        read 13.3 Hz on a 40 Hz input (= 40/3; with/without the scope probe on the node), and AC V
        of a 0.5 V DC source reading 23-56 mV and decaying slowly (compare with stock first).
        Device 40728b8 (device-check doc "Crosstalk and cold-junction self-heating (40728b8)"): DC V reads
        -0.05 / -0.24 / -0.39 mV at 0 / 1 / 5 V in the combined view vs the DMM page (pairwise, sem 0.06 mV,
        noise unchanged 0.12-0.14 mV). Mostly the scope rail: RAIL BOTH on the DMM page gives -0.24 mV at 5 V.
        Analog, < 0.01 %. AC V of 0.5 V DC: 33 mV first reading, < 1 mV by 1.4 s, floor 0.34 mV, the same in
        both views (the slow decay is gone). FREQ: 40.0017 Hz steady in both views (~440 combined readings,
        AC+DC -> Hz, Scope -> combined), 13.3 Hz not seen. The remaining bug is not caused by the combination:
        DC V -> Hz soon after a view change gives 1-2 wrong first READ? (0 / 0.94 / 0.99 / 17.0 / 19.9 Hz) on
        both pages. Proposed fix (not done): restart the counter (R20 0xF0, CTA 0xFC0000, R20 0xF2) on the Hz row
        write, freq.next = deadline + 350, src_alt = false, drop each source's first result / no "last = 0"
        win. Close this item after that fix.
        **Fixed (branch freqfirst, emulator only):** every non-AC+DC row with ENCTR presets the gate at the row
        write; the first counter access past the settle (Hz, the AC V / A secondary line, the power meter's F)
        only restarts it, so the first result is a gate begun after the settle; a source's first result ranks
        with change 1e6 (shown only while alone), no source = no reading (was 0.00 Hz), a source at 1/2, 1/3,
        2/3 of the other loses; NaN ("--") before the first result. Not done: the proposed "two results agree"
        (+0.35..0.7 s to the first reading). AC+DC / AUTO left alone (they show no frequency). The model now
        keeps CTA<23:8> over ENCTR = 0 (datasheet) and has ctsettle: (every other edge missed after a row
        change); `emu/hy3131/freqtest.py`: 69f3dfe first READ? 34-45 Hz (19.9-30.2 Hz with ctsettle:400) at
        +1.05 s, freqfirst 40.0007 Hz at +1.40 s (the device's clean first readings: 1.37-1.41 s).
        **Device retest** (40 Hz square, calibrator; DMM page and combined view, as `chk4/fentry.py` / `f40dc`):
        (1) 10x: CONF:VOLT:DC, 3 s, CONF:FREQ, 4 READ? with times: every value 40.00 Hz (no 0 / 0.99 / 17 /
        19.9), the first at ~1.4 s; half the entries within 5 s of a view change (DMM <-> combined, Scope ->
        combined). (2) CONF:FREQ, Scope page, back (rail wake): first READ? 40.00. (3) AC V -> Hz and AC+DC -> Hz
        5x each: 40.00 from the first. (4) AC V on the 40 Hz square: the secondary line reads "--" then 40.00
        Hz, never a wrong value. (5) CONF:POW on 230 V / 50 Hz (or the 40 Hz square): F "--" then 50.00, after
        an entry and after a range change. (6) Capacitance 1 uF / 100 nF entries (the gate now preset at the
        row write): readings as before.
        **Device d2aff22 (device-check-2026-10-21.md "d2aff22"): (1)-(3) FAIL, (4)-(5) PASS, (6) not run.**
        Every Hz entry (DC V / AC V / AC+DC -> Hz, Scope -> back, both pages, 26 of 26) reads **0.000 Hz / 0.0 %
        three times** (1.37 / 1.71 / 2.06 s, amber, READ?, MIN / AVG), 40.0017 from 2.42 s (40728b8: 40.00 at 1.37
        s). Fits: source 0 (0xF0) reads f = 0 in its first gates; f = 0 is not a glitch, so it sets seen / last = 0
        and wins alone, then the 1e6 tie with source 1's first result. Fix: f == 0 is no result for `seen`, and a
        1e6 tie goes to the non-zero source. AC V secondary line "--" then 40.00 from 0.7 s; power meter F
        +9.91E37 then 40.00, unchanged by CONF:POW 60.
        **Fixed (branch freqfirst2, emulator only):** `Freq::take`: a gate without edges (CTB 0), one still
        inside its 53.3 ms (CTA >= 0xfc0000, `ctr_valid`), f = 0 or a glitch is no result (the source drops
        out, the other shows); nothing valid = no reading (the placeholder "--" stays, READ? and the
        statistics wait); 0.000 Hz only when both sources counted nothing twice in a row (once each after a
        value was shown). The first gate after the settle is source 1's (0x08), as 40728b8's clean 1.37 s
        readings were. Model: `ct0dead:MS` (the Hz row's comparator source counts nothing for MS after the row
        change); freqtest `entry-dead` / `wake` / `acv` (ct0dead:2500) fail on f6ec475 with 0.000 Hz first
        readings, `nosig` checks 0.000 Hz with nothing connected.
        **Device retest (freqfirst2):** as 1-3 above (10x DC V -> Hz with 5 READ?, Scope -> back 3x per page,
        AC V / AC+DC -> Hz 5x): every READ? 40.00 Hz, the first at ~1.4 s (Scope -> back ~0.9 s), the screen
        "--" (never "0.000 Hz" / "0.0 %") until then, MIN / MAX / AVG start at 40.00. (7) Nothing on V-COM
        (calibrator off), CONF:FREQ: READ? 0.000 Hz within ~3 s, no hang. (8) Calibrator 40 Hz -> off while in
        Hz: 0.000 Hz within ~1 s. (9) Optional, if a DBG counter readout is added: does source 0 (0xf0) ever
        read the signal on the Hz row?
        **Device bb39569 (device-check-2026-10-21.md "bb39569"): (1)-(4) and (7)-(8) PASS.** 165 of 165 READ?
        40.00 Hz (DC V / AC V / AC+DC -> Hz, Scope -> back, both pages, half after a view change), the first at
        1.37-1.40 s (Scope -> back 0.86 s DMM / 1.04 s combined); screen "- -" at 0.5 / 1.0 s, 40.002 Hz with
        MIN / MAX / AVG 40.002 at 1.5 s, never 0.000. No signal: 0.000 Hz at 2.42 s. 40 Hz -> off: 0.000 after
        0.45-1.02 s; in 3 of 12 one partial-gate value (3.2 / 6.1 Hz) for ~0.7 s before it (open, owner's call).
        (9) not done (no DBG counter readout).
  - [x] Device/About page: Menu->Settings->Device (Vendor/Web/Model/Bootloader/Software/Hardware);
        HW+bootloader ver read from the stock boot-data block 0x080A4000 (survives app updates).
        SN + BT name omitted (not sourceable without inventing a value / a blocking AT round-trip).
- [x] **Native-format firmware (top priority, user 2026-10-13).** (Stage 1 on device 2026-10-13;
  T3 recovery triggers still open below.) Drop the RAM/sector-8 relocation
  scheme: build a full firmware in the stock update-file format, flashed like the stock one at
  0x08004000. Our image starts with a tiny shim "bootloader": power key held ~2 s at start
  (optionally with USB cable) or a watchdog-reset cause -> start the native updater (the one
  that brings up mass storage); otherwise run opendm40. Shim as small and simple as possible.
  Must be unbrickable and allow easy stock <-> opendm40 toggling.
  HARD CONSTRAINT (user): recovery must NOT depend on opendm40 running or on opendm40's MSC. With the
  app dead there is otherwise no way to put a file on the SPI FAT. The shim itself must give the
  host a way to place a stock dm40*.bin: jump into the FS-loader's own host-facing disk mode if
  it has one, else carry a minimal self-contained USB-MSC + SPI/FatFs in the shim.
  - [x] Architecture doc `docs/native-boot.md`. Key finding: a permanent FS-loader at
        0x080a0000 (own USB MSC + FatFs) reflashes any dm40*.bin from the SPI FAT on boot
        and is never overwritten by an app update -> unbrickable recovery already exists.
        Shim stays tiny (RCU_RSTSCK FWDGTRSTF, PD6 power key, SRAM magic + boot counter).
        dm40fw.py `pack` already computes both header words. opendm40 must move to XIP from
        0x08004000 (clock.rs to set PLL+FMC waitstates+cache; link.x reserve 0x08008000 hole;
        drop the SRAM copy in startup.rs).
  - [x] Opus review (`stock/re/docs/history/native-boot-review.md`). Verdict: Stage 0 SAFE; Stage 1 NOT until
        dead-app recovery is closed. App reachable ONLY via FS-loader (runs every boot); it
        installs ANY dm40*.bin on EVERY boot (must delete after good install). MANDATORY before
        Stage 1: shim carries its OWN USB-MSC (Option B; opendm40 MSC is dead when recovery needed);
        PD6 release-then-hold gesture; clock.rs own PLL+FMC-WS+cache + VTOR=0x08004000; boot magic
        in backup SRAM; 12-byte 0x08008000 hole truly 0xFF.
  - [x] Stage 0 PROVEN on device (2026-10-13): staged dm40_v_2_6.bin on the FAT via opendm40 MSC,
        then COLD power-on -> the loader installed it to 0x08004000 and deleted the file (confirmed
        gone). App-independent recovery works. KEY FINDING: a WARM reset (SYSRESETREQ / DBG:RECOVER)
        does NOT trigger the loader install; only a COLD power-on does. So the shim's recovery =
        host stages a valid dm40*.bin via the shim's own MSC (Option B) + power-cycle. See the
        FS-loader-trigger investigation (stock/re/docs/fsloader-install-trigger.md) for whether a marker can
        make install happen on a warm reset.
  - [x] Own fonts (prereq for independence): tools/mkstockfont.py extracts the 19 used fonts
        (714 glyphs, ~83 KB) from the stock image into src/stockfont_data.rs; stockfont.rs reads
        embedded data. Public API unchanged, pixel-identical. Feature-gated `embedded-fonts`
        (default build keeps reading stock flash so the RAM loader still links; native build turns
        it on). The native image has room for the 83 KB; sector-8 RAM layout does not.
  - [x] clock.rs: own PLL bring-up (`init_core`, measured stock RCU_PLL 0x29406419, no FLASH_ACR
        write, bounded waits, IRC16M fallback). (device-verified by T1)
  - [x] Native build (`./build.sh native` -> opendm40.bin): shim (FWDGT, power hold, reset
        flags, PD6 hold 2.5 s, WDT only after our own normal boot), recovery = zero the loader
        marker + reset, board init audit, fatren (rename installed dm40*.bin to _DM40PRV.BIN,
        cold boot + valid marker only). See docs/native-boot.md "Implementation".
  - [x] VENDOR UPDATE MODE found and proven on device (user's idea, 2026-10-13). Any dm40*.bin on the
        FAT (even 0 bytes / bad header) makes the loader zero the 0x08008000 marker, print
        "update error" and expose a WORKING USB disk on every boot until a valid image is installed
        via its "update" button. Header is checked before any erase (app sectors untouched).
        Boot gate is PA2, not the reset cause: ROM polls PA2 x1000; high -> ROM verifies the marker +
        body CRC and launches the app itself; low or invalid app -> loader install state. (The earlier "cold installed / warm didn't"
        reading of PA2 is withdrawn: no observation supports it.) Full analysis: stock/re/docs/fsloader-install-trigger.md.
  - [x] Recovery = program 12 zero bytes over the 0x08008000 marker + SYSRESETREQ (works on warm reset,
        no FAT/SPI/USB/LCD in the shim); DBG:UPDATE / DBG:RECOVER do the same (dev flow: copy a
        dm40*.bin onto opendm40's disk, DBG:UPDATE). Built (recovery ~490 B, test-recovery image
        1712 B); DM40RCVR.BIN / own-USB variants kept behind recovery-file / recovery-usb.
        (device-verified by T2 and the T3 way back)
  - [x] Test: power-on holding the power key ~5 s with no dm40*.bin -> stock booted. No vendor
        button entry; PA2 is not the power key. 2026-10-24 (owner, device): PA2 = the debug
        header's TX pad (pull-up); a TX-GND short enters the loader's install state = the hardware
        recovery (hw/README.md "Hardware recovery"). So our marker-zero recovery is required; per code
        an invalid marker sends the ROM to the loader regardless of PA2 (to be confirmed by T2).
  - [x] Full verified flash dump (1 MiB, OB, OTP) taken before Stage 1: stock/re/dumps/full-20261013/.
        RDP is level 1 (SWD/DFU only via mass erase), so the software recovery path matters.
  - [x] Review of the native image + shim (stock/re/docs/history/native-review-2026-10-13.md): T1/T2 GO, T3 NO-GO until
        B1 (native stack overflow: fatren buffers live during main) and B2 (shim reads PD6 without
        pull-down; stock = input + pull-down, pressed = 1) are fixed; H1 fatren renames only the file whose
        header == marker; H2 key-hold trap (light backlight at once, cue before zeroing); H3 stale flags;
        crash counter so one panic doesn't force a reinstall. (fixed in 9575616 before T3)
  - [x] T1 on device (test-initcore over stock): init_core HXTAL OK, LCD/USB/disk/DMM/BT OK, FSMC live
        BCR1 0x5091 BTR1 0x0F04 BWTR1 0x0FF00201 (board.rs aligned), panel 0x9488, marker VALID.
  - [x] T2 on device (test-recovery): marker zeroed + WARM reset -> loader 'open file error' + working
        USB disk. Restored dm40_patched.bin through it.
  - [x] T3 / Stage 1 on device (2026-10-13 17:28): native dm40_fwrs.bin (commit 9575616) installed via
        the loader screen; DBG:BOOT? NORMAL/HXTAL/VALID marker, CALBK WRITTEN (8 backup files on the disk),
        STACK 30760/39000, PD6 pull-down, DMM reading OK. Way back proven: dm40_patched.bin on the opendm40
        disk + DBG:UPDATE -> loader installed patched stock (marker = its header, memrw answers).
        The loader (not the stock app) deletes the installed dm40*.bin (340faca) -> the running
        firmware's update file is regenerated from flash as a backup on the disk (439b251).
  - [x] T3 recovery triggers in the emulator (emu/recoverytest.py, ROM -> FS-loader -> opendm40 with the real
        loader; FWDGT + sticky RCU_RSTSCK flags modelled in emu/peripherals/FWDGT.cs): normal boot, short
        power-on press, key held to the beep and released in the 0.5 s window (normal), key held from off
        7 s (marker zeroed -> loader "open file error / Update" screen at shim + 4.8 s), one hang (reboot,
        crash word 0x43520001), a second hang 10 s / 50 s of main loop after the reboot (recovery, the
        second one = resets 70 s apart), a second hang after > 60 s of main loop (reboot only), and
        DBG:RELEASE twice over USB (WDT_RETRY, then recovery). All PASS on 1372e957 (= 37d3252's image); no
        shim bug found. The 60 s window is 60 s of RUNNING main loop since the last boot, not 60 s between
        resets (README wording fixed).
  - [ ] T3 remaining on the device: the boot and watchdog steps by hand (docs/native-boot.md
        "Boot and watchdog: owner steps"; the hang-count entry into the loader was removed in 0.4.0,
        owner 2026-10-29; the key hold and DBG:LOADER enter it through PA2, marker kept: "PA2 entry");
        scope view stack high-water; first-boot timing.
  - [x] Native bugs seen: boots into AC+DC mV (persisted-mode mapping after the persist v3 change?);
        trend chart plots the separate AC/DC phases; AC part 40 mV on a DC calibrator input (AC zero?).
        (c44ca81; verify on device)
  - [x] Stage 2: retire stub/, flash.py, BT loader path; update README/tools. (3c20dc1; verify on device)
  - [x] RAM build removed (owner 2026-10-22; it no longer linked, ~200 KB over): build.sh builds
        only the native image (`native` kept as a no-op argument), link.x / link-test.x /
        startup.rs, the test-initcore / test-recovery (T1/T2) images, stub/, tools/legacy/
        (flash.py, dm40_loader.py) and reload.sh --ram are gone; features native /
        embedded-fonts / calbackup / fatren-live dropped (always on / unused), vdisk is default.
- [x] Data-logger mode (FR-38) implemented: src/logger.rs, main-loop polled constant interval,
      timestamps + sequence numbers + drop counter, HY3131 fast mode, LOG:* SCPI. Not tested on device.
- [x] Code review 2026-10-13 (docs/review-2026-10-13.md): H2/M1/M2/M3/M5 + lows fixed; H1/M4 call sites
      in main.rs/msc.rs applied by the native agent.
- [x] UI: HOLD repaint fixed (label only drawn inside reading(), skipped while held), Settings back
      navigation (parent chain), Settings rows like vendor (8 rows, value next to label, "< Back"
      title bar), compact DMM+scope view (22 px bar, 30 px value, scope +56 px). Not verified on device.
- [ ] UI/UX review punch-list (../docs/history/ui-review.md §4) remainder. Done: #1 drag-misread-as-swipe
      (gesture.rs: a drag never swipes), #2 unified status block with lock/BT/battery on every bar,
      #5 page dots, #4/#8 superseded by the stock scope port (stock right panel, markers, T/trigger
      handles). Done 2026-10-21 (verify on device):
      - #3 wake gate (c41d960): after an idle dim the first touch / side key only wakes; input is
        ignored for 500 ms (stock FUN_08020248) and the rest of the waking touch is dropped.
      - #10 generator keypad prefill (24023c7): current value + unit, drawn selected; the first
        digit / '.' / backspace replaces it; OK on an untouched prefill keeps the setting.
      - #9 generator bar status + tappable RUN/STOP (98e3b7d).
      - #11 pressed-key feedback on the keypads (f5f9a8b); since 2026-10-22 also the menu tiles /
        rows (stock tile style's pressed 0x4419, an RE guess), the list-page rows of Settings 1-3,
        Hold & Limits, Record (0x04FC, value white), their -/+ stepper keys and the Start / Stop row
        (0x7BEF, the stock popups' pressed colour) and the generator boxes + wave drop-down (0x04FC):
        drawn from touch-down until release / roll-off, one item redrawn (app_press.rs, in .cold;
        ui::set_row_only = the page's own draw call limited to one row). ui-render: every item
        pressed, then released = the page as drawn. (verify on device)
        2026-10-23 (125a423): also the MF range pages (Voltage / Current / Resistor / Capacitance
        Settings: range grid, coupling row, ohm variants, Hold/Limits, OK, Fuse check, Wave, Inrush) and
        every button of Power Settings, pressed 0x04FC (the list rows'), one button redrawn
        (ui::range_press / ui::pw::settings_press, .cold). ui-render 0 violations (55704 cases; new
        renders m03d..g, p31 / p31b); emulator: press on 600mV drawn pressed, a roll-off restores the
        page byte-exact, press + release on AC/DC selects it. Device retest: MF on DC V, hold a finger on
        a range button (lighter blue), slide off (normal again, nothing selected), tap AC/DC (selects);
        the same on Power Settings (power meter, MF).
        **Device d330877 (device-check-2026-10-21.md "d330877", item 2): PASS** on V / A / Ohm / F and Power Settings: slide off = the page
        byte-equal, nothing selected; a tap selects (AC, Low current ON, AC); set back. The held (lighter blue)
        state cannot be screenshotted with DBG:DRAG / DBG:TAP (one loop pass): needs a real finger.
        Not done (left for the owner): the confirm / fuse / wiring dialog buttons (the dialog closes on
        the tap, its own feedback; the fuse / job labels vary per stage), the Inrush page (its keys run
        while AD2 streams: a .cold redraw per touch there needs a device timing check first), Gallery /
        Help navigation; #7's remainder (scope controls in the combined view) is the combined-view design's.
      - #12 backlight / volume -/+ steppers, clamped (83c2b05 drawing, 3f4b609 logic).
      - #13 already in: DMM lightning >= 100 V (stock), combined strip HV badge >= 30 V and red
        ring >= 100 V. DMM page >= 30 V: done 2026-10-21 as the stock cue itself (value 0xC9A0 red in V
        functions from 30 V, gui_v2_6.h:175; the value is orange otherwise).
      - #14 won't do: the preview is the stock geometry (4,85,239,231; stock UI is the baseline);
        the bar now carries the status (#9).
      - #15 the duplicate Keypad/ScopeMenu arms are gone (ScopeMenu removed by the scope port).
        Dead code removed (2026-10-21): ui::bar / button_row / DMM_BUTTONS / sub_label / val_line,
        src/fonts.rs + src/text.rs (only those used them), tools/mkfont.py + fonts/Roboto-*.
      Remainder (c13d61a):
      - [x] UF-08 cursor tab "CUR" at the graph bottom (108,280): off -> X -> Y -> off, as the
        stock cursor measurements (X2-1 + Xf / Y2-1) in the measurement row; "off" restores the
        selection. (c13d61a; verify on device)
      - [x] UF-10 CH1 V/div buttons drawn as magnifiers: top = zoom in (smaller V/div), bottom =
        zoom out (c13d61a; stock-diff: btn_vdiv_up/down ~9 % by design; verify on device)
      - [x] UF-09 double tap = CH1 position 0 + trigger at 50 % (stopped: centred on the trigger
        point); only where a single tap does nothing (Corners: the graph middle; Off: anywhere;
        never in Halves, every tap steps there). (c13d61a; verify on device)
- [x] CSV-to-disk recording on the DMM screen, swipe-triggered (user). (7b1e0c0; verify on device)
      decided: see ../docs/history/decisions-2026-10-22.md #1 (Record page: intervals, mean per row, duration, estimate)
  - [x] Record page (decision #1): "Record" row on Hold & Limits -> View::Record: Interval (every reading /
        1 s / 10 s / 1 min, -/+ stepper), Duration (until stopped / 10 min / 1 h / 8 h / 24 h), "Free: about
        N h left" (free clusters x 2 B/row at the interval or the measured reading period, scanned on page
        entry, "disk busy" while a commit holds it, capped at the 49.7-day limit), Start / Stop (Start returns
        to the DMM page). csvlog.rs: interval rows = mean of the readings, time_s = interval END (k x I),
        flags ORed; an OL in the interval makes it an OL row (the DLG / CSV format carries no value on an OL
        row: decision text "mean of the non-OL readings" is not representable without a format change);
        no row for an interval without readings; the partial last interval at stop is written at its
        nominal end; the duration ends the recording (push and poll) with the normal "Saved" toast. One
        extra "# interval N s: ..." header line in interval mode; every-reading mode byte-identical (the
        fatren-test md5). SCPI LOG:CSV:INTerval / DURation (discrete values; Settings conflict while
        recording) / FREE?. Persist v8 word W_REC. The idle auto power-off is held while recording (it was not). Swipe REC uses the same settings. Help page 5.
        fatren-test "interval recording". (verify on device: checklist in the decisions doc #1)
        Device c50425f (docs/device-check-2026-10-21.md "c50425f" 3): Hold & Limits 9th row, Record page, 1 s via
        the page 131 s -> LOG000.CSV 132 rows t = 1..132 on the USB volume == MMEM:DATA?; 10 s / 10 min stopped
        itself, 60 rows t = 10..600; INT / DUR / FREE? and Settings conflict OK; settings survive DBG:RESET.
        Not verified: swipe REC with the interval, the estimate within 2x, the power-off hold (no DBG field,
        Shutdown Never on the unit).
      src/csvlog.rs: one-finger swipe down on the DMM page = start / stop,
      toasts, red REC in the status blocks, LOG:CSV ON|OFF / LOG:CSV?, power-off / reset commit the tail.
      (Since the USB storage redesign the PC cannot write or mount the SPI disk: no "Disk in use"
      refusals any more; files in LOGS/ on the SPI flash.)
- [x] Compact binary logs, CSV only on read (owner 2026-10-21: "not CSV on the disk, RLE or a
      decided: see ../docs/history/decisions-2026-10-22.md #2 (timestamps stay real main-loop time, no grid snap)
      standard compressed format; CSV only for export"). docs/compact-log-format.md: own DLG format
      (adaptive Rice of delta-of-delta time + 6-digit value deltas, CRC chunk per commit) chosen over
      gzip / sigrok / FLAC on measured data: 0.9..1.9 B per DMM reading vs ~21 B CSV (11..26x; 13 MB of
      disk = 325 h at 10/s instead of 17 h); scope record 53 KB vs 692 KB.
      - [x] src/dlog.rs codec + CSV generator; tools/dlog-test: decoded CSV byte-identical to the old
            writer for 9 data sets, random access, truncation at every byte keeps every whole chunk, bit
            flips contained. tools/dlog.py: standalone decoder (same bytes).
      - [x] csvlog.rs records LOGnnn.DLG (same swipe / LOG:CSV UI); numbers shared with legacy LOGnnn.CSV;
            fatren-test: md5 of the decoded CSV == the old writer's LOG000.CSV, cut sweeps, disk full.
      - [x] scope "Save CSV" writes SCOPnnn.DLG, CSV md5 == the old SCOPnnn.CSV. Coded (owner
            2026-10-21, ../docs/history/log-format-standards.md 4): fixed predictor order 0-3 per 4096-sample
            block (or raw 10-bit) + adaptive Rice, 3.7-5.8x smaller than u16 (emulator record 4231 B);
            the length is planned at Save, the writer fills 512-byte pieces per pass.
            (verify on device: a real noisy record's size; the ~10-20 ms planning stall at Save)
      - [x] Owner 2026-10-21: no text LOG*.CSV / SCOP*.CSV on the SPI disk any more (gallery CSV
            parser, shared numbering, list / delete removed); CSV only as the export.
      - [x] Export: virtual LOGnnn.CSV / SCOPnnn.CSV on the read-only LUN 1 (dlog::refresh ->
            vdisk::set_extra, refreshed when a recording ends / a delete / a host write ends);
            SCPI MMEM:DATA? "<name>"[,off[,len]] (LOGnnn.CSV generated, any file raw);
            tools/dm40scpi.py get LOG000.CSV. Gallery reads .DLG (exact counts).
      - [x] Emulator: LOG:CSV ON 60 s (154 readings at 2.5/s): LOG000.DLG 455 B, CSV 3108 B; MMEM:DATA?
            CSV == dlog.py of the DLG == LUN-1 LOG000.CSV read over USB/IP MSC.
      - [ ] Verify on device: swipe REC 10 min, the USB disk shows LOGS/LOG000.CSV on Windows / Linux
            right after the recording (UNIT ATTENTION at once, also while mounted), open in a
            spreadsheet; scope Save CSV -> SCOPE/SCOP000.CSV; gallery tile of a .DLG; DBG:PROF? csv lap
            during a refresh with a large new log (~0.1 s per 100 KB expected); DBG:DIAG? STACK
            high-water after a refresh with several logs.
            (Device f80524f, the old LUN-1 layout: scope Save CSV -> no SCOP*.CSV on DM40-INFO after
            unmount/mount; the Save CSV job never called dlog::set_stale. Re-check on the new layout.)
            Emulator on c9f6951 (new layout): scope STOP -> MF -> Settings -> Save CSV -> SCOPE/SCOP000.CSV on
            LUN 0 at the next read (UNIT ATTENTION 06/28 first, then the listing has it; screenshot.rs finish()
            calls set_stale since 80c07f3); LOG:CSV ON/OFF -> LOGS/LOG000.CSV likewise. Device re-check pending.
            Device c50425f (Linux): SCOPE/SCOP000-002.CSV and LOGS/LOG000-001.CSV on the mounted volume right after
            the recordings, no remount, == MMEM:DATA?. Open: Windows, gallery .DLG tile, PROF csv lap, STACK.
            Device c92b605, Windows 11 VM (device-check-2026-10-21.md "c92b605: ... Windows 11"): LOGS/LOG000-004.CSV
            and SCOPE/SCOP000-009.CSV are listed and open in Notepad / PowerShell; the copied SCOP000.CSV is complete
            (26 624 rows). Open: on Windows, a recording made while mounted (+ F5); spreadsheet; gallery .DLG tile;
            PROF csv lap; STACK.
      - [x] Owner 2026-10-21: drop legacy text LOG*.CSV / SCOP*.CSV support on the SPI disk (gallery parser,
            numbering, list/delete): only .DLG is written and read; CSV exists only as the generated export.
            (4be89c1)
      - [x] FR-38 logger to disk: src/fastlog.rs + src/fastenc.rs, DLG kind 3 (docs/compact-log-format.md
            "Fast stream (kind 3)"): LOGS/FASTnnn.DLG pre-allocated (duration x rate x 3 B, bounded by the free
            space) and pre-erased (blank sectors skipped, BE64 where a 64 KiB block is all ours) before the
            logger starts; the run only programs pages (<= 300 us + one page per pass, no erase / FAT / dir
            write, region B); entry size + FAT trim at stop. LOG:DEST DISK|HOST, LOG:DURation, LOG:DISK?;
            Record page interval "Fast" (AD2 in AC rows, AD1 in DC V/mV/A, LOG:SRATe / OSR rate; tap to
            stop / cancel). CSV on the USB volume (LOGS/FASTnnn.CSV, fixed width: t_s,raw,value,lost_before),
            MMEM:DATA?, dlog.py. Host: dlog-test fast sets (round trip, truncation, power cut per chip op),
            fatren-test fastlog (sessions, ring holes, Full, abandon, the USB volume's CSV, a cut before
            every block write / program / erase). Emulator: emu/fastlogtest.py (W25Q128 busy model).
            Device test plan: "Fast logger device check" below.

## Feature requests to add (from ../docs/dm40-feature-request.md; user: add autohold + useful ones)
- [x] **FR-01 AutoHold (P0, critical).** Auto-freeze the reading when it stabilises, re-arm on a new
      signal, beep on capture (Fluke/Keysight style). (8f98171; verify on device) (f44e63d re-capture: verified on
      device f80524f: 1.10 -> 1.20 -> 1.10 -> 1.12 V captured, 1.11 V not, as meter-test)
- [x] FR-02 MIN/MAX/AVG (P0): MIN/MAX exist; add AVG (running average, no auto-off while active).  (8f98171; verify on device)
- [x] FR-05 smoothing + selectable sample rate (P1): slow/fast conversion + display smoothing.  (4c157a8, c7ec2c1; verify on device)
      Rate chip under the DMM chart timebase (tap = SLOW/MED/FAST, grey "n/a" where the
      rate does not apply), Settings 2/2 "DMM rate" / "Smoothing", SENS:RATE[:PER?], CALC:SMO[:STAT|:RESP],
      persisted in p[38]. CSV log and MIN/MAX/AVG stay unsmoothed. Rates: see "Measure the real DMM
      conversion period" below.
- [x] FR-06 Hi/Lo limits (P1): pass/fail thresholds with a beep.  (8f98171; verify on device)
- [x] FR-16 screenshot + log gallery (P1): save screenshots/CSV to the SPI disk and browse them.  (f02ce6e, 18ae461; verify on device)
      Backend + SCPI (csvlog list/delete, MMEM:CAT? / MMEM:DEL, USB only for DEL); on-screen gallery
      src/gallery.rs, opened from the main menu's top row "Gallery: screenshots and logs".
- [x] FR-28 generator DC offset (P1): adjustable offset (note: real bipolar offset is HW-4).  (ae3e655, 38d9983; verify on device)
      Offset box on the generator page (keypad; OK on an empty field = auto), SOUR:VOLT:OFFS <V>|AUTO, persisted p[39].
- [x] FR-35 CSV export from DMM (P1): = the CSV-to-disk item above, with timestamps.  (verify on device)
- [x] FR-09 REL, FR-10 idle auto-off (+ countdown warning), FR-12 screen dim, FR-18 touch/key lock,
      FR-27 generator keeps running in DMM: present in code (cf1978b, 98c00f7, concurrent instruments).
      FR-33 is USBTMC+MSC (we chose DBG over CDC). (verify on device)
- [x] Repo tidy-up (user request): ../TIDY-PLAN.md executed 2026-10-13 (b06da47..369cad7); the repo is
      now private. Left to the user: whether vendor-derived material
      (stock/re/analysis/app_decompile.c) and BT MACs stay tracked if it ever goes public again.

## From user community feedback (docs/user-feedback.md)
- [ ] UF-01 (P0) Verify continuity CMPO beeper + diode beep classes on device, latency <10 ms (most-praised stock feature). §5.2
- [x] UF-28 (P2) Continuity / diode beeper off at Key/alert volume 0 % (owner 2026-10-27: it beeped at volume 0).
      hy::write clears R20 bit 2 (ENPCMPO) while the volume is 0; Dmm::poll rewrites R20 on a change in
      continuity. Volume 1..100 cannot scale it (CMP pin, not TIM14; hw/README.md "Buzzer"). Emulator:
      emu/hy3131/beeptest.py. (PR #56; verify on device)
      Device check (2 min, shorted probes): Key/alert volume 0 -> CONT and DIOD silent; 50 -> both beep;
      in CONT switch 50 -> 0 -> 50 (silent / beeps at once); power off and on into CONT at 0 -> silent.
      DBG:SENT:REG? first byte: 52 / B0 at 0 %, 56 / B4 at 50 %.
- [x] UF-28b Visual beeper indicator (owner 2026-10-30, testing #56: "a visual indicator of a short").
      Green SHORT / BEEP band over the aux row + green digits (strip: badge + value) while the beeper's
      condition holds, held 150 ms (App::beep_ind); continuity reads CTSTA bit 6 (ACPO, the CMP pin's
      source per the datasheet) every 5 ms DMM poll (also in the entry settle), diode follows
      `diode_beep`. Emulator: emu/hy3131/contvistest.py (CONT: on 1-3 ms after the input, CPU time only).
      Device check: shorted probes in CONT at volume 0 and 50 -> band + green digits at once, off
      ~0.15 s after release; CONT entered with the probes together -> at once; DIOD short -> SHORT, a
      silicon diode -> BEEP briefly. If CONT never shows it: CTSTA bit 6 does not follow CMPO there.
- [x] UF-04 (P1) Scope AUTO keeps the user's AC/DC coupling (app.rs autoset_start forces DC = stock bug, 4 users). §6.1  (81e02a7; verify on device)
      AC search first; nothing in AC -> the same search in DC, kept with the toast "AUTO: DC (no AC signal)".
- [x] UF-14 (P1) Generator: clamp to the wave's limit + visible note instead of a silent 0 Hz (revert the parity snap; owners call it a bug). §7  (ae3e655; verify on device)
- [x] UF-15 (P1) Generator: show the actual quantised output frequency (and in SOUR:FREQ?); later a DDS for finer steps. §7  (ae3e655; verify on device)
- [x] UF-03 (P1) AC+DC sub-mode on DM40A (owners proved the A hardware does it when the app commands it). §10  (verify on device)
- [ ] UF-02 (P1) Cold-junction self-heating with both rails/concurrent instruments: measure vs stock, correct or power down. §3
      Device 40728b8 (device-check doc "Crosstalk and cold-junction self-heating (40728b8)"): CJ 25.62 °C on the
      DMM page (25 min), 25.58 °C in the combined view (25 min, no step, +-0.1 °C), 25.43 °C back on the DMM page
      (10.5 min). The MCU die meanwhile +1.0..1.6 °C with the scope on (tau ~2 min). No CJ self-heating from the
      scope rail: below the 0.5 °C threshold, no correction needed. Stock not measured (no stock number in
      user-feedback §3, only "~1 h idle"). Open: a stock run for the comparison, and the CJ vs a room thermometer
      (FR-26).
- [ ] UF-19 (P1) Measure opendm40 battery draw vs stock (10-15 h baseline); decide on a scope-on-demand power policy. §8
- [x] UF-17 (P1) 3-finger swipe-down screenshot to the USB disk (stock gesture, missing in opendm40; shares the FAT writer). §2, §10  (67fa077; verify on device)
- [x] UF-18 (P1) 2-finger swipe-down power-off (stock gesture; touch point count is available). §8  (67fa077; verify on device)
- [x] UF-20 (P1) On first boot write CALBAK.BIN (cal record + boot data + UID, CRC) to the USB disk, written when missing or changed (compare-first). §4  (device-verified: CALBK WRITTEN in T3)
- [x] UF-21 (P1) Make AUTO+ (V-first, already ported: v_auto=1) the default V autorange; warn when mV-first AUTO is chosen. §5.1  (4c157a8; verify on device: an upgraded unit boots in AUTO+; Range page warning)
- [x] UF-05 (P1) Scope drag axis lock (user: horizontal drags knock the vertical): dominant axis decided at
      the slop and locked until release; relative trigger drag; drag mode Stock / Axis lock with the
      hold-to-grab time in Settings. §6.1  (d154f60, stock port 100eb48, 74b5c16; verify on device)
- [x] UF-09 (P1) Scope horizontal trigger position: the stock T marker drag (stock port 100eb48). §6.1
      Double-tap to centre: see the UI punch-list remainder.  (verify on device)
- [x] UF-32 (P1) CSV files: '.' decimals, ISO-8601 timestamps, header (model/fw/function/range). §10  (verify on device. No RTC: time_s since the start + the start uptime, no ISO dates)
- [x] UF-37 (P1) Native shim keeps the short-press power-on (stock V2.1 behaviour). §8  (verify on device)
- [x] UF-16 (P2) Generator ramp direction: gen.rs falls, preview rises; verify on scope and align. §7  (ae3e655: ramp rises; verify on device)
- [x] UF-11 (P2) Scope FFT with labelled axes + automatic peak (f, dBV) readout. §6.2  (386f1ed, 0f4dffb; verify on device)
      src/fft.rs: 2048-pt real FFT (1024-pt complex radix-2, f32), 6 windows (stock 5 + flat-top),
      dBV / Vrms, sliced: load on the frame's pass, 3 butterfly stages per pass, split + dB in one
      pass. Source: the record around the frame (decimated by the mean of k samples when the screen
      holds >= 2048, else 2048 record samples centred on it). Stock math pill / popup (Mode FFT/ABS,
      Color, Hz/Div = 4 1-2-5 values, Window, Unit) live; the M strip marker (0 dBV row) and the M
      top marker (FFT centre, drag pans) as the stock; opendm40: "10dB/div", "50kHz/div" boxes, peak
      readout "f dBV" + white peak marker. ABS math too. Emulator (1 IPC, so ~1.5x on the device):
      FFT section avg 0.54 ms, max 1.3 ms per pass; frame 6 ms avg. Persist v5 (48 words).
      Not in roll (no linear record).
- [x] UF-12 (P2) Full probe attenuation list x0.1..x1000 (stock scope_probe[13]). §6.2  (2124249, 7b1b83c; verify on device)
      Scope MF -> Settings tab, 13 buttons; persisted. No SCPI probe command exists (none added); TRIG:LEV <V>
      now takes probe-tip volts like TRIG:LEV? answers.
- [x] UF-07 (P2) Scope corner-tap timebase + list picker: Corners (default) / Halves / Off, preview tabs,
      timebase list popup. §6.1  (ff29cd3, docs/scope-scale-ux.md; verify on device)
      UF-08 (one-tap cursors) and UF-10 (V/div +/- as zoom): done in c13d61a (the UI punch-list remainder).
- [x] UF-13 (P2) Save scope record as CSV to disk; software pulse-width/runt trigger. §6.2  (4146aae, 20fc636; verify on device)
      MF Settings "Save CSV": stopped scope only (stock "Please stop sample"); the whole stored record
      (<= 26624 rows, ~690 KB, ~10 s) as SCOPnnn.CSV through the screenshot writer (fixed 26-byte rows,
      header timebase / V/div / probe / coupling / trigger / sample rate); the scope is held stopped
      while it writes; refused while a PC has the disk mounted. Trigger tab Type Edge / Pulse (> < =,
      1-2-5 width 20 ns..10 s, = within 5 %) / Runt (second level = orange T marker, drag).
      Limits (src/trigsoft.rs): searched after each complete linear record (no comparator), dead time
      between records misses events, width resolution 1 sample, slow timebases wait for a full
      26624-sample record (10.6 s at 1 s/div).
- [x] UF-23 (P2) PT100/PT1000/NTC temperature via the ohm path. §3  (device-verified on the calibrator, 19:10 run)
- [x] UF-25/26/27/28/35 (P2) DMM small fixes: extra-digit option, 4-20mA% label, store-while-HOLD, volume semantics, MIN/MAX reset. §4-§5  (8d75505; verify on device)
      UF-25: no option (owner: the extra digit is always on). UF-26: DC mA loop % captioned "4-20mA", hidden
      outside -25..125 % (OL); 11c4f9b hides it below 3.6 mA (device 92fb8fa: DC mA 60 mA range, A jack open,
      0.001 mA: no %, PASS; the % above 3.6 mA needs current through A). UF-27: a history tap stores the held value while HOLD / AutoHold. UF-28: row
      renamed "Key/alert volume"; idle power-off announced 30/20/10 s before (toast + beep at >= 50 % even at
      volume 0). The continuity / diode beeper is the HY3131 CMPO pin (not TIM14): off at volume 0 (UF-28 above, PR #56).
      UF-35: long-press on the MIN/MAX/AVG column = statistics reset, distinct from the history-row clear (existed).
- [x] UF-29 (P2) On-device Help page (gestures, resets, input limits). §9  (7b58f84; verify on device)
      Settings 2/2 -> Help: 4 list pages (gestures, side keys, multimeter taps, limits; ui::HELP); tap = next page,
      title bar / MENU = back. No input voltage ratings (none sourced in the repo).
- [ ] UF-24/30/31/36/38/41 (P2) §3-§10, worked 2026-10-21 (decisions in docs/user-feedback.md):
  - [ ] UF-24 tempco compensation: parked, documented (needs FR-36 + a per-unit two-temperature
        characterisation; the only always-on sensor is the MCU die, biased by the rails, UF-02).
  - [x] UF-30 fast bargraph: widget src/bargraph.rs + tools/bargraph-test (b6a4d10); wired 2026-10-21
        (see the DMM batch below). Rate = the AD1 conversion rate (10/s FAST /
        moving, 2.5/s steady); >= 40/s needs OSR 0x13 + a firmware average for the readout.
  - [x] UF-31 stylus: no sensitivity register (stock writes only D1 01 / D2 04 / D1 09); documented.
        Possible firmware cause found: touch.rs Debounce drops a touch whose only pressed frame is
        followed by a release frame. Device check first: DBG:TOUCH? log of stylus taps.
  - [x] UF-36 BT on every view: by design (bt.poll every 5 ms in all views, DMM rail kept on while
        linked); check tool `tools/dm40scpi.py --bt btpoll -p 50` (a907213). Not during an FR-38
        logger session (BT frames are not served then, by design).  Device 8b9cf79 (docs/device-check-2026-10-21.md "Bluetooth (8b9cf79)"): 400 / 400
        replies at 50 ms on DMM, DMM+Scope, Scope, Gen (median 37 ms). Caveat: a link opened while on Scope /
        Gen does not wake the DMM (stale value; see the open item below).
  - [x] UF-38 tools/dm40scpi.py: info / conf / read / log CSV / scpi / btpoll, USBTMC or --bt (a907213).
        Device 8b9cf79: --bt info / read -n 20 / btpoll OK.
  - [x] UF-41 sin(x)/x: src/sinc.rs (Lanczos-3) + tools/sinc-test (e433aad), wired (086f734): the
        "fewer samples than columns" branch (100..500 ns/div). Emulator: frame ~5.2 ms avg at 100 ns/div
        vs ~3.3 ms at 1 us/div (linear path). (verify on device)
- Community-confirmed existing items (priority signal only): native boot/recovery; FR-26 temperature offset (raise to P1);
  FR-36 user calibration; FR-13 sub-mode memory; FR-01/FR-02 (AVG on chart); FR-04/CSV record + FR-38 logger; FR-05;
  FR-09/FR-24 stray-C REL; FR-10; FR-14/FR-17; FR-19; FR-21; FR-28/29/32; FR-34; USBTMC; ui-review #1, #4, #5, #8, #10, #11.

## User decisions 2026-10-13 (on community feedback)
- [x] Temperature: user offset (cold-junction / reading), plus sensors PT100, PT1000 and common NTCs
      (10k B3950, 10k B3435, 100k, ...). (device-verified on the calibrator, 19:10 run)
- [x] Gestures: 3-finger swipe = screenshot to the SPI disk, 2-finger swipe = power off (as stock).  (67fa077; verify on device)
- [x] AC+DC on ALL models (DM40A hardware supports it).  (verify on device)
- [x] Always 60000 counts (DM40C behaviour) regardless of the model byte; IDN/Device keep real model,
      + physical power-key short/long/double press as in stock; PD6 pressed level.  (verify on device)
- [x] Scope UI redesign (docs/scope-ui-plan.md): superseded by the stock scope port
      (docs/stock-scope-port-plan.md, 6be1399..29555fe, S1-S9) + opendm40 extras (3d094a4, ff29cd3).
- [x] Calibration (0x080C0000) + boot data (0x080A4000) + UID copied to the SPI disk at every boot
      (only when missing/changed) so they're reachable over USB.  (device-verified: CALBK WRITTEN in T3)
- [x] Analog rails: default OFF (stock-like alternation per page); both on automatically in the
      DMM+Scope combined view; SCPI command to force both on (e.g. SYST:RAIL BOTH|AUTO). Consumption/
      heating vs stock: UF-02 / UF-19.  (verify on device)
- [x] +1 display digit (at least DC voltage), smaller font after the main digits, always on.  (cfeac1f; verify on device)
- [x] Milliohm resolution on the lowest resistance range (units of mΩ).  (cfeac1f; device-verified: 10.117 Ohm, 19:10 run)

- [x] AutoHold idle threshold absolute on small ranges (meter.rs ah_cfg): DC V/mV idle below max(300 counts,
      5 mV) (60 mV range: 300 counts = 0.3 mV < the 0.487 mV open-lead residual, stock/re/docs/history/acdc-stock-vs-opendm40.md 6c),
      DC A below max(300 counts, 1 uA). Device: open leads on DC mV manual 60 mV / uA never capture.  (2fcf286; verify on device)

- [x] BT phone-app function change (cmd 06) goes through the app's function-change path (App::host_func_changed
      = func_changed: meter reset, chart restart, function bar / static view redrawn), like touch and SCPI.  (ce2f422; verify on device)

- [x] Temperature sensor type shown large (owner): TEXT22 chip with the full sensor name in the DMM page's aux2
      slot and as the meter strip's second sub row.  (671006d; verify on device)

- [x] Temperature offset slider (owner: +/- buttons "absolutely unintuitive"): ui::Slider, -20..+20 C in 0.1
      steps, drag / touch-down sets, -/+ end buttons step 0.1, "Offset 0" resets.  (3287199; verify on device)

## Backlog (user ideas)
- [ ] Inductance (owner 2026-10-24). Options, none started:
      (1) GEN square -> external R -> L, scope on L: fit tau, L = tau * (R + R_gen + R_L); ~100 uH .. 1 H.
      (2) GEN sweep + known C (measured by the DM40's cap mode): resonance peak, L = 1 / ((2 pi f0)^2 C).
      (3) GEN square -> L -> A jack (0.5 / 47.5 ohm shunt) -> COM, AD2 at 76.8 kS/s logs i(t): tau from the
          shape, R_total = V_gen / I_inf, L = tau * V_gen / I_inf; no external R; from ~3-6 mH (R_gen ~50 ohm
          inferred, current path bandwidth ~20 kHz).
      (4) OP3 step through Rref 1 k on V/ohm (DBG:SRC): only L >= ~100 mH.
      A real LCR (two synchronous channels, V and I phase) is not possible with one scope channel.
- [x] **Settings are lost at power-off** (device 2026-10-24): the backup SRAM is not powered while the
      meter is off (POR + BOR flags after a key power-off; a marker setting and the crash word were gone; a
      reset and a firmware update keep it). Keep a copy of the settings on the SPI disk (as the stock's
      config.bin): written a few seconds after a change and before a power-off, read at boot when the backup
      SRAM copy is invalid. Then update the manual's "What is kept when you switch off" and Known limitations.
      Done (src/setfile.rs): `_SETTING.BIN` in the SPI root, 2 one-block slots (magic, persist version, seq,
      words, CRC-32), made once at boot, saves rewrite one slot in place; 5 s debounce, a 20-save budget
      refilled every 3 min, deferred while the disk is held / an AD1 session / the AD2 stream; flushed in
      power_off. Boot: backup SRAM unless the file is newer by seq (persist::seq), else the newest valid
      slot, else defaults. DBG:BOOT? SETTINGS / SETFILE / SAVE. fatren-test "setfile", emu/settingstest.py
      (power cycle = two runs, the disk carried over). Device 25b6ec3 (2026-10-24): first boot SETFILE CREATED seeded
      from BKP; SYST:BEEP:VOL 40 -> SAVE OK seq 2; key power-off + on: POR+BOR, SETTINGS,FILE,1,2, volume 40. Open: after a power-off the
      DMM function starts on DC V (persist::same_session needs BKP_LAST, which is gone).
- [x] Longer scope record (owner 2026-10-24: "worth doing, not now"; done 2026-10-26, branch
      feat/scope-long-record): was 26 624 samples vs the stock 65 520 (u16 per 10-bit sample makes 65 520 =
      131 KB; the screenshot buffer the item once named is gone since 2026-10-22). Now 65 520 packed 4 in 5
      bytes at <= 25 kS/s (from 100 ms/div) and in the wave view, 43 008 raw (packed when it stops) faster;
      Single / DIGitize fill the whole record. Design and costs: docs/ram-budget.md "The scope record".
      Emulator: emu/wavtest.py (lengths at 1 / 100 / 200 ms/div, no seam, the trigger at the centre, Save
      CSV of the long record = WAV:DATA?, stopped zoom).
- [x] Device check of the long scope record (a6c177d8, 0.3.0, 2026-10-26). PASS: MCX DIGitize (Auto, nothing
      on the MCX) at 1 us / 20 us / 1 ms/div: 43 008 samples; 100 ms/div: 49 932 and 500 ms/div: 25 417 (the
      untriggered record ends at the Auto timeout); wave view (DC V, 4800 S/s) on an MR9270 20.001 Hz square:
      STOP after 14 s keeps 65 520 samples, 20.001 Hz, every period 239..240 samples (no seam), AD2 dropped /
      late / overruns 0; Save CSV of it = WAV:DATA? (65 520 rows); stopped zoom; a screenshot. Triggered
      MCX on the device (2026-10-26, this branch + #30 rails fix; MR9270 50 Hz 0-10 V square on CH1, EDGE POS
      5 V, NORM, DIG): 1 ms/div 43 008 samples, 100 ms/div 65 520 samples, the trigger point on the rising edge
      mid-record, 50.001 Hz from the record. Without #30 the scope read code 0 after a boot with Analog power
      Both (nothing triggered).
- [x] Long scope record: pan through a stopped record by touch on the device (796e2970 with #30, 2026-10-26;
      MR9270 1.6 Hz 0-10 V square, 100 ms/div, NORM, DIG, 65 520 samples): DBG:DRAG by half a screen moves
      the trace 200 px, both ends stop the pan, a drag back from the start moves at once; 200 / 500 ms/div
      while stopped show the record (at 500 ms/div ~210 columns). Before 796e2970 a drag right after a
      screenshot (DBG:LCD? holds drawing 300 ms) left the drag preview on the screen, half of it blank.
- [ ] Not possible / not done for the long record (docs/ram-budget.md "The scope record"): 65 520 at the
      fast timebases needs a 131 KB raw ring in one piece (BUF + logger::RING + calbackup::SCRATCH + the
      csvlog buffers + gen::WAVE lent for the capture: no logging / screenshot / CSV / FFT / generator
      meanwhile); an 8-bit byte-DMA capture cannot work on this board (the ADC's 10 bits are PB0..PB9, a
      byte read of GPIOB gives PB0..7 or PB8..15).
- [x] reload.sh --image: the first upload failed twice in a row on 2026-10-24 ("upload failed, nothing
      installed"), the immediate retry worked both times (once with the Scope view open). Cause: the SCPI
      error queue is a FIFO that lives across USBTMC clients, and dm40dbg.py upload took the first
      SYST:ERR? after DBG:UPLOAD:BEGIN as BEGIN's result: an error some earlier client left (here -113
      "Undefined header" of an unknown command; before the error queue, the single error slot) failed the
      upload although BEGIN had started it; that read popped it, so the retry passed. Fix: dm40dbg.py
      Link.fresh (device clear + the stale errors read and reported + *CLS) before BEGIN and DBG:UPDATE;
      the meter's SCPI error printed as such; reload.sh retries the upload once. Also a cut message of a
      dead client (Linux usbtmc sends no clear on close) took the next client's header as its payload:
      opendm40 drops a Bulk-OUT message left incomplete for 1 s (usbtmc::expire_partial, checked every
      main-loop pass and when the next packet arrives).
      emu/usbtmc_stress.py cases `session` (dm40dbg.py's own cmd_upload over USB/IP after a stale error
      and a cut message; failed with the -113 before) and `partial` (hung before). Device 2f56ff9
      (device-check doc "2f56ff9" 4): a real cut message (raw USBTMC_IOCTL_WRITE, 52 of 150 bytes) then a
      new client 1.5 s later 10 / 10 clean; 0.3 s later the query is eaten (5 s timeout, recovers alone, as
      designed); Link.fresh() reports the stale -113 6 / 6. The upload / reload.sh retry not run on the device.
- [x] Auto power-off during a remote session (device 2026-10-24: the meter switched off during a firmware
      flash over USB): the idle shutdown counts USBTMC messages, Bluetooth SCPI lines / phone-app frames and
      now USB disk READ / WRITE commands (msc::HOST_IO) as activity; the idle dim counts touches and keys
      only (a dimmed screen costs a remote session nothing). DBG:IDLE <ms> / DBG:IDLE? (test power-off
      time, idle times); emu/idletest.py (usb, bt, msc keep it on for 40 s past a 20 s power-off time,
      off with no traffic; msc failed without the hook). Manual "Power, battery and sleep". Device 2f56ff9
      (device-check doc "2f56ff9" 3): DBG:IDLE 20000 + a disk read every 4 s, no USBTMC: on 68 s past the
      limit; with no traffic off 20.5 s after DBG:IDLE 20000.
      After the power-on: DBG:IDLE? 1800000, settings from the file (seq 71), volume 50, alarm LOUD.
- [x] Wave view (scope on the DMM inputs): finer V/div steps (owner 2026-10-24, 2026-10-25: the range's
      full scale did not fit any 1-2-5 step). `scope::WAVE_STEPS_V` / `_A`: per DMM range a full-scale step
      (+-range on the 8 divisions: 15 mV / 150 mV / 1.5 V / 15 V / 150 V / 250 V per div for 60 mV .. 1000 V;
      150 uA / 1.5 mA / 15 mA / 150 mA / 1.5 A / 2.5 A for 600 uA .. 10 A) and the 1-2-5 steps between, each
      on the lowest range whose AD2 window holds it; a step on the current range only rescales the screen.
      V/div + / -, the pinch and AUTO (the full-scale step, centred) use it; entry and the clip autorange take
      the full-scale step; labels "1.5V" / "2.5A"; 60 mV joins the list (MF item 5). Not persisted (the
      settings file is unchanged). touch-test (tables vs dmm::ad2_scale_of, pinch, labels), ui-render
      (every range, full-scale + one intermediate step), wavetest (steps, AUTO; `--case fullv / fulla`:
      +-96.7 % of 6 V / 60 mA inside the drawn rows). docs/dmm-input-scope-feasibility.md 10.1.
      Open: 0..range (a unipolar signal over the 8 divisions) is a position drag, not a step; the trace rows
      clamp at 0..278 (stock), so the outer ~1-2 % of +-full sits on the frame edge.
      **Device 98116d2 (2026-10-25): PASS.** DC V 6 V range, 1.0 V from the calibrator, MF -> Wave: 1.5 V/div;
      + 2, + 5 (the 60 V range), - 2, 1.5, 1, 0.5 (6 V), - 0.2 (the 600 mV range), + 0.5 (6 V); AUTO key 1.5 V/div
      on 6 V. The trace at 1.0 V (2/3 div above the centre at 1.5 V/div), Vrms 1.00 V (screenshot).
- [x] SCPI over Bluetooth serial: the same parser as USBTMC (session per transport, DBG:* refused on BT),
      stock binary frames (0xAF) dispatched alongside, DMA TX ring. docs/bt-scpi.md,
      tools/dm40bt_scpi.py.  (74ed868, d06b423; device 8b9cf79: SCPI, POW / CALC:LIM, LOG:DATA? over BT OK;
      LOG:DATA? drains ~918 records/s = ~11 KB/s; WAV:DATA? throughput not measured)
- [x] Compound SCPI messages (`POW:PHAS:CAL RES;:SYST:ERR?` hung the USBTMC read): Scpi::handle kept only
      the last command the main loop runs (Instr / POW / LOG / MMEM / DBG / READ?) and main.rs cleared the
      text replies before it. Now scpi_msg.rs runs a message one command at a time (USBTMC and BT alike):
      replies in order, ';'-joined, one response; READ? / MEAS? / FETC? pause the rest until the reading;
      a message with a query always gets a response (empty if none could answer, reason in SYST:ERR?; also
      a lone MMEM:DATA? / MMEM:CAT? that failed, which hung before); block replies (MMEM:DATA? / CAT?,
      LOG:DATA?, DBG:LCD? / PROF? / TOUCH? / HYSPI? / AD2?) only as the sole query ("Query error" next to
      others); joined reply <= 512 B ("Out of memory"). tools/scpi-msg-test, usbtmc_stress.py compound.
      Open (pre-existing): headers after ';' are absolute (no IEEE 488.2 relative-path rule); a ';' inside
      a quoted argument splits; a BT line refused as DBG:* that holds a query gets no response.
      Device 8b9cf79 over BT and USB: `POW:PHAS:CAL RES;:SYST:ERR?;*OPC?` -> `-100,"Settings conflict; RES:
      ..."; 1` in one line, `*IDN?;SYST:ERR?` -> `...;0,"No error"`.
- [x] Auto AC/DC for V and I (as UNI-T UT197 "Auto AC/DC Voltage"): an AUTO coupling next to DC / AC /
      AC+DC that picks DC or AC by itself from the signal and shows which one it chose. Decisions
      (stock/re/docs/history/acdc-stock-vs-opendm40.md section 11; dmm.rs AUTO_*):
      - measurement: AUTO runs the AC+DC alternation (the only place both parts are seen: AC V is
        AC-coupled) and shows one part, the DC part (signed) or the AC part's RMS; autorange on the total.
      - rule / hysteresis (once per ~1.9 s cycle, both parts fresh): to AC when AC > 1.5 x |DC| and above
        the floor (V 1 %, A 3 % of the range), to DC when AC < 0.75 x |DC| or below the off floor (V 0.5 %,
        A 2 %); between: unchanged (factor-2 band; a half-wave rectified sine keeps what it showed).
      - switch time: 2 evaluations in a row (a probe moved between two DC-dominant sources must not flash
        AC for the one cycle that mixes old and new parts): the switch shows 2..6 s after the change
        (emulator: 2.2..4.5 s); the first evaluation after entry decides at once (~2 s, as AC+DC's first
        reading). Changed after the device check of ac1e852 (acdc-approaches.md 10.9 item 2): DC until the
        first decision (AC only with no DC part), which waits for a settled AC part; a tie within 1 % is DC.
      - display: the V / A button = the setting: the AC/DC mark `~/⎓` (our BTN_GLYPH 0x09; with the
        part's glyph a tap into AUTO looked like no switch, owner 2026-10-22); the glyph above the unit =
        the part shown, "AUTO" in the limit badge's slot (a verdict wins); aux row = AC part / DC part tagged, as AC+DC (it does not move on a
        switch); strip badge AutoDC / AutoAC. A switch restarts REL, statistics, the AutoHold window (HOLD
        and a capture stay); limits use the slot of the part shown; a CSV recording ends on a switch.
      - touch: V / A button cycle DC -> AC -> AC+DC -> AUTO -> DC; range page coupling row DC / AC / AC+DC
        / "AC/DC" (4 buttons on the range grid).
      - SCPI: [SENS:]VOLT:COUP / CURR:COUP DC|AC|ACDC|AUTO (select the function), ? -> DC / AC / ACDC /
        AUTO,DC / AUTO,AC; CONF / MEAS / FUNC VOLT:AUTO / CURR:AUTO, FUNC? "VOLT:AUTO" (codes 19 / 20).
        BT: the mode byte reports the part shown (the app echoing it keeps AUTO). LOG / wave view refuse it
        as AC+DC.
      Device 33b44bd (V side; device-check doc): taps, glyph, range row, DC 5 V, SCPI, reboot PASS; square
      (AC 0.999 x DC) -> 1 V DC switched in 5.1 / 5.4 s. Fixed: REL dropped by a switch kept the range hold
      (MANU). Open: entering AUTO on a near tie picks AC (first AC results +7 % high on the device, the doc
      says -1 %); HOLD across a switch redraws the bargraph scale under the held value.
      Device 92fb8fa: the range is released (60 V -> 6 V, 4 decimals) but the status bar still said MANU
      until the next static redraw: the status-bar cache key could not tell MANU from AUTO+ (both have
      auto_plus). Fixed (ui.rs DmmView: the key holds r.auto).
      - persistence: the function code (19 / 20) in p[2], no layout change; V / A keep no coupling of
        their own (entry from another function = DC, stock).
      Tests: emu/hy3131/couptest.py v / i / tap (DC -> AC -> mixed both ways through the band), band
      (entry inside the band: the larger part, a tie stays put), ui-render auto_* cases (d02b..d02d,
      c19 / c20 / c21, m03c), touch-test coupling row.
      Not on the UT197 model: there it is "Auto-V LoZ", the auto selection comes with a ~2 kohm input
      load (no LoZ hardware here, feature-request HW-6), so ours is the 10 Mohm variant: ghost voltages
      (hum on open leads) still read, as AC above the floor.
  - [ ] Device check (AUTO coupling): see stock/re/docs/history/acdc-stock-vs-opendm40.md section 11 "Device check".
  - [x] FETC:BURD? in AUTO used the part shown (understated a mixed current): now the total RMS (AC+DC,
        what drops across the shunt), OL by the total; the aux burden readout is not shown in AUTO (aux
        rows = the parts, as AC+DC). burdentest AC+DC / AUTO phases (200 mA DC + 100 mA AC).

## Found in the screenshot session 2026-10-13 (native 439b251)
- [x] READ?/MEAS? take ~8 s (wait for a full averaging window that never fills -> 20 conversions).
      (de57d3d; device-verified: READ? ~0.5 s, 19:10 run)
- [x] USBTMC wedges after a timed-out query (pending waits expire, new commands cancel them, class
      CLEAR/ABORT resync).  (de57d3d; device-verified: survives a libusb timeout, 19:10 run)
- [x] SCPI function change (CONF/MEAS) does not redraw like a touch.  (de57d3d; device-verified, 19:10 run)
- [x] AC+DC sub-readings line: AC tilde / DC sign placement.  (90b615d; device-verified, 19:10 run)
- [x] Temperature sensor label in the status bar: glyphs overlap for PT100/PT1000/NTC names.  (90b615d; device-verified, 19:10 run)
- [x] AC+DC current reads ~150 uA AC with nothing connected: stock overload guard ported, reads 0 on
      device (per owner; commit not identified). The AC V decay half moved to the crosstalk item.
- [x] 3-finger screenshot blocks the main loop ~8 s (BMP write to SPI flash); USB and DMM stall.
      Now incremental (screenshot.rs state machine, <= ~3 ms per main-loop pass; frame captured
      run-length coded into 96 KiB SRAM, screen frozen only while captured); SPI1 /16 -> /4 (12.5 MHz).
      (d154f60, f6e30e9; verify: loop max in DBG:DIAG? during a shot, SCPI READ? keeps answering, S00x.BMP correct)
- [x] Screen frozen ~4-5 s on every shot since the shot streams from the panel (CAP dropped, 9890cd0):
      the next shot's first-fit free clusters erased in idle time, nothing reserved / linked
      (screenshot.rs "idle-time preparation", docs/ram-budget.md). Emulator (W25Q128 timing):
      frozen 5.0 s -> 1.4 s, 0 data blocks erased. (8d1d8d4; device check below)
- [x] Device: screenshot preparation (8d1d8d4). DBG:FLASH? fields 4.. = frozen_ms, total_ms,
      slices_frozen, blocks_erased, prep state (2 ready), clusters ready, prep erases.
      1. Boot, wait ~15 s: prep state 2, 76 ready. A shot of the DMM page and of AC+DC: frozen_ms
         (expect ~1.1-1.6 s), blocks_erased 0; BMP equals `dm40dbg.py shot`; ~5-10 s later state 2 again.
      2. A shot right after another: frozen ~3-5 s (unprepared), Saved.
      3. Power cut (battery out) while prep state = 1 right after a shot: boot, MMEM:CAT? lists the files,
         fsck of DEBUG/SPIFLASH.BIN clean (no lost clusters), prep reaches 2, the next shot Saved.
      4. Disk filled to < 76 free clusters: prep state 2 with the free count, a shot says "Disk full".
      5. CSV recording on: after each 60 s commit prep goes 1 -> 2; fast-log session: prep stays put
         during it; "PC can edit" + a DM40*.BIN copied: prep pauses, the file installs.
      6. DBG:DIAG? loop max while prep works (state 1): no worse than idle + ~1 ms.
      Device 540e056 (2026-10-23, device-check doc "540e056"): 1 PASS (frozen_ms 1357 DC V / 1362-1383 AC+DC,
      0 erased, BMP = the panel at the gesture 0 px, touch back by 1.54 s, prep 2 / 76 again ~8 s later);
      2 PASS (back-to-back on blank flash 2332 ms, 0 erased; onto dirty clusters 1954 ms with 14 erased, ~42 ms
      each); 4 PASS (free 0: state 2 / 0 ready, "Disk full", no BMP); 5 CSV commits PASS (0 -> 1 -> 2 in
      2.3-2.8 s), fast-log PASS (state 0 all session, resumes 2 s after); 6 PASS (state 1 with 76 erases: loop
      max 13.9 ms vs idle 12.0 on the AC+DC page). Step 3 (battery pull) PASS on 2f56ff9 (device-check "Screenshot preparation: power cut"). 5's "PC can edit" + DM40*.BIN install PASS on 1a11eda (2026-10-24: file copied after a hub-port re-plug, OK on the meter, back in 62 s with a SW reset, the file deleted by the loader, settings kept, MMEM prefix check live).
      Device 2f56ff9 (2026-10-24, docs/device-check-2026-10-21.md "2f56ff9: screenshot preparation, the 76.8 kS/s nominal"): 1 PASS (frozen_ms 1304 DC V /
      1324 / 1314 AC+DC, 0 erased, BMP = the panel 0 px x3, prep 2 / 76 again 9 s after the job); 2 PASS (second
      shot 4.9 s after the first, prep at 12 ready: frozen 5328 ms, 63 erased, saved; above the 3-5 s estimate); 4 PASS (free 0: 2 / 0 ready,
      "Disk full", no BMP); 5 CSV commits and fast-log PASS; 6 PASS (state 1 max 15.1 ms = idle 15.1).
- [ ] Device: the disk-buffer and preparation cases of docs/ram-budget.md that no run has covered.
      1. Power off (hold the key) and DBG:RESET while the preparation erases (DBG:FLASH? prep state 1, right
         after a shot): the off / reset waits for the erase (`prep_finish`, <= 0.4 s); the next boot lists
         every file, prep reaches 2, the next shot Saved.
      2. MMEM:DATA? of a screenshot and the on-device gallery while the preparation erases: the read waits
         for the erase in flight (<= 45 ms typ., once) and returns the file's bytes (compare with a read in
         prep state 2).
      3. The scope FFT through a whole fast-log session (Record page fast logger or LOG:DEST DISK): the
         spectrum holds, then resumes after the session; no garbage spectrum.
- [x] Data logger (LOG:STAT ON) has no on-screen indicator: "DATA LOGGING" panel (dialog::log_panel,
      rate + sample count, 1 s refresh), view repainted on stop.  (d154f60; verify on device)
- [ ] Browser client (owner 2026-10-22: later, not now; 2026-10-24: the old stock-protocol web/ client was
      removed, the new one replaces it). One local HTML page (firmware/tools/web/dm40.html) that talks SCPI to
      opendm40 over BOTH links and covers ALL functions: big DMM reading with every function / range /
      coupling / rate, trend + CSV, power meter, scope via WAV:PRE? / WAV:DATA? (timebase, V/div, trigger,
      RUN/STOP, measurements), generator, loggers (LOG:*), the USB-disk / gallery files, settings, alerts,
      and an SCPI console.
      - USB: WebUSB (Chrome/Edge) on the USBTMC interface (class 0xFE; the MSC interface stays the OS's).
        Linux needs a udev rule that unbinds the usbtmc kernel driver from 1209:0001 and grants access
        (Chrome cannot claim a kernel-bound interface); Windows needs WinUSB (Zadig).
      - Bluetooth: Web Bluetooth to the "DM40A" module carrying opendm40's SCPI-over-BT (docs/bt-scpi.md).
        Check that the module exposes a BLE GATT serial service Chrome can open: Web Bluetooth has no
        classic SPP/RFCOMM. If it doesn't, the BT path needs a different route, e.g. Web Serial to an OS
        RFCOMM port.
      - A claude.ai artifact sandbox likely blocks WebUSB.
- [x] Boot indication (owner 2026-10-24: "the firmware takes several seconds to boot; at least show
      'Loading'"). Done on feat/boot-splash (merged #38, v0.3.0): a splash ("opendm40", version, "Loading...", a bar per
      phase) right after board::init; the first frame drawn before USB attaches, so the "USB configured
      or 3 s" draw gate is gone; vdisk::init + boot_refresh after the first frame; DBG:BOOT? BOOTMS /
      MAINMS. Device 108eb55, normal boot: the meter page about 0.68 s after pre_main starts (before:
      about 4.1 s, inferred from the phase times plus the 3 s gate, not measured on main).
  - [ ] Left: `vdisk::mirror_boot` (0.3 s every boot, 5.4 s on the first boot of a new image) still
        runs in pre_main under the splash. Moving it after the first frame would leave the meter page
        frozen for those seconds on the first boot; a chunked compare in idle time needs
        calbackup::write_file split up, and the mirror served only once compared (before any stream).
        The step line on the slow first boot is done (splash.rs `note`, drawn when calbackup rewrites a
        file). The rest saves 0.3 s per boot: not a bug, so an owner decision (bug-fix-only rule).
- [ ] Protocol decoders (owner 2026-10-23): UART, Modbus RTU, etc. The usual problem with scope decoders is that
      they only decode what fits in one short record (here ~53 KB). Make long traffic convenient instead: decode
      **while streaming**, keep only the decoded frames and let the raw samples go.
      - Source: the scope's continuous circular-DMA stream mode (up to 1.25 MS/s, the roll / software-stream path),
        which gives UART up to ~115200 baud at >= 10 samples/bit. Faster links use one 50 MS/s record with the
        trigger on a start bit. Single channel, so only one-wire or differential protocols. In scope (owner
        2026-10-23): UART / RS-232 (inverted), RS-485 (probe across A-B), Modbus RTU and ASCII on top of UART,
        1-Wire (reset / presence, ROM commands, CRC-8; standard and overdrive speed), DMX512 (break / MAB, 512
        slots at 250 kbit/s: needs >= 1.25 MS/s, so check margin or use triggered records), CAN (CANH-CANL or
        RX; bit stuffing, ID / DLC / data, CRC-15, ACK; 125-250 kbit/s fits the stream, 500 k / 1 M via
        records). Maybe later: LIN, IR NEC/RC5. No I2C / SPI (they need two channels).
      - Decoder: threshold with hysteresis (auto from the signal's min/max), bit timing from the edges (auto-baud
        from the shortest pulse plus a list of standard rates), 8N1 / 8E1 / 7E1 etc. Modbus: frames split on the
        3.5-character gap, CRC-16 check, address / function / registers decoded, request <-> reply paired.
      - UI: a frame list (time, direction or address, hex + ASCII or Modbus fields, CRC OK/BAD), scrollable over the
        whole session; tap a frame to show its waveform when that piece is still in the record. Error counters
        (framing, parity, CRC, gaps). Trigger / filter on a decoded value (address, function, a byte pattern).
      - Logging: decoded frames to the disk (LOGS/UARTnnn.CSV, plus a raw byte .BIN), unlimited length like the
        fast logger. SCPI: DEC:UART:... config and a frame-FIFO read, so a host can tail the bus live.
      - Constraints: the decoder runs per DMA half-buffer in the main loop, so it must keep up at 1.25 MS/s and stay
        in region B (no region-C code while streaming). DMM / generator keep running alongside.
      - [x] Phase 1 (PR #63, draft, on feat/scope-dlg-ring): UART and Modbus RTU as a band on the scope page
            (owner 2026-10-28: an overlay on the scope page, not a separate view; the first streaming view of
            the PR was dropped). Owner 2026-10-28 (2): not in real time: each record is decoded once, a
            running one that triggered (Normal / Single, Auto when it fired; not roll, not the Wave view while
            running) and the stored one on STOP; the bytes / frames kept in TCM (80 / 16, positions in the
            record) and every redraw, zoom and pan draws from them; a record with more: the screen's window
            again when stopped. The decode runs in the main loop (`dec_step`, 3 ms a pass), never in the frame
            draw; the next acquisition waits for it (Scope::hold_restart). Settings: an MF tab (Decode, the
            fifth). src/decode.rs (levels auto / manual, auto-baud from
            the shortest runs with a TooFast cap, 8N1 / 8E1 / 8O1 / 7E1, inverted polarity, characters with a
            quarter-bit idle step, a Modbus framer with a running CRC and request / reply pairing),
            app_decode.rs (the decode per frame; DECode:...), ui_decode.rs (the band: byte boxes with hex and
            the character, Modbus frame boxes with the fields and the reply time, errors red, a bar zoomed
            out, "Rate too low for N baud" under 4 samples per bit; the MF Decode tab). DECode:FRAMes?: the
            kept frames (stopped with more than the lists hold: the record decoded again on the query). The
            cache (`ANN`, ~1.3 KB) in TCM; no SRAM used. Tests: tools/decode-test (incl. filtered edges
            at 8 samples per bit), ui-render `--decode` (24 zooms x 3 pans x both views, 0 violations),
            scpi-test, emu/decodetest.py (MCX and the Wave view: ScopeFeeder / HY3131 SetBitsFile).
      - [x] Frame log (owner 2026-10-28, Mode Log of the Decode tab, DECode:MODE LOG): the first draft's
            streaming list back, in the scope's record memory instead of a static: scope.rs `log_*` (a
            16 K-sample circular DMA ring at the start of BUF, 1.25 MS/s for auto-baud, ~16 samples a bit
            for a set rate), app_log.rs (levels, auto-baud again after framing errors in a quarter of 32
            characters, decode.rs chars_step / Framer over the ring in up to 4 ms a pass; a circular byte
            store of the frames in the rest of BUF, ~44 KB, the oldest dropped), ui_log.rs (the list,
            counters, LIVE / page / CLEAR / STOP / SCOPE, the Decode page as SETUP). Each frame keeps its
            waveform: ~8 samples a bit from 2 bits before to 2 after (`snip_write`: 8-bit means, a byte-wise
            run / difference code, ~150 B for an 8-byte Modbus frame; dlog's DLG coder took 6-8 bits a
            sample on a square wave), coded when the pass has time (about one a pass: at 115200 not every
            frame has one); tap a row to see it with the band (region C, `log_cold`). Emulator: 267 Modbus
            19200 frames with waveforms kept; 9600 and 115200 (4 588 frames in 3 s, none lost), pairing.
            DECode:FRAMes? reads it as a FIFO, DECode:CLEar. Leaving the view ends it (BUF back to the
            scope, a new record). The Wave view refuses it.

      - [ ] Phase 2: tap a frame to centre the record on it; trigger / filter on a decoded value (address,
            function, a byte pattern); disk logging (LOGS/UARTnnn.CSV); Modbus ASCII; the other protocols of
            the list above.
            Known limits: the frame costs ~0.1 ms more with the decoder on (s.frame 6.98 against 6.89 ms at
            1 ms/div); a record's decode is 2-5 ms at 1 ms/div (9600 / 115200, up to 8 ms when it measures the
            rate), and the next acquisition waits for it (70 against 79 frames in 3 s at 1 ms/div, 9600;
            emulator, native-boot.md "Protocol decoder"); the band covers the graph's bottom 35 px (the trace is not drawn there); a long
            coded record (Single at 100 ms/div and slower) decodes at ~1.3 us a sample; at 1 ms/div a 9600
            baud screen holds 9 characters, the first may be misread when the screen starts inside a run of
            back-to-back characters (they realign at an idle gap); the AD2 filter's edge shape in the
            emulator is INFERRED (a sinc^2-like mean over 2 periods).
      - [ ] Device check (a USB-UART adapter or a Modbus device: the generator cannot make UART): (1) 3.3 V
            USB-UART TX -> OSC, text at 9600 / 115200 8N1: the band at 200 us .. 5 ms/div, DEC:RATE?,
            DEC:FRAM? after STOP matches what was sent; (2) 8E1 / 7E1 and a wrong parity -> red boxes, PE;
            (3) RS-232 levels with Inverted; (4) Modbus RTU master + slave on RS-485, probe across A-B, 9600 /
            19200 8E1: REQ / RSP boxes, the reply time vs the master's log, a corrupted frame red; (5) the Wave
            view, DC V, RS-485 A-B on V-COM at 2400 / 4800 (and 9600 at 1 ms/div): the AD2 filter's real edge
            shape, DEC:RATE? after STOP; (6) DBG:PROF? dec.job (a record's decode), x2 and s.frame at 1 / 10 /
            100 ms/div with 9600 and 115200 traffic, frames off / on, against the emulator's numbers
            (native-boot.md "Protocol decoder"); (7) Mode Log: 115200 text for 60 s, DEC:COUN? lost 0 and
            the FIFO complete; a tapped row's waveform matches the scope's look of the same bytes; Modbus
            pairing and the reply time; leaving Log, the scope captures again; Log in the Wave view refused.

- [ ] Scope in ngscopeclient (owner-approved 2026-10-22: a libscopehal driver + a full-record readout).
      Done in the emulator (branch scope-full-record): opendm40 WAV:POINts SCReen|MAXimum|<n>, WAV:FORMat BYTE|WORD,
      WAV:SOURce CHAN1, 10-field Keysight/Rigol WAV:PREamble? (was 4 fields), DIGitize, TRIGger:STATus?,
      CHANnel1:OFFSet, ACQuire:SRATe? (src/scope_wav.rs, docs/bt-scpi.md "Scope waveform readout"); the
      default WAV:DATA? is unchanged. The 53 KB record streams over USBTMC from the scope buffer
      (usbtmc::respond_stream, packets polled within the disk-READ budget per pass; scope held stopped
      until the last packet; given up after 2 s without progress); refused while running and, when longer
      than 6 KiB, over Bluetooth.
      emu/wavtest.py passes (USBTMC multi-round + 64 KiB rounds, BYTE / WORD / decimated, the sine's
      frequency / Vpp / trigger point, abandoned stream, BT refusal). libscopehal driver DM40Oscilloscope
      ("alientek_dm40", usbtmc / lan / uart; generator as a FunctionGenerator) in
      ../../ngscopeclient-dm40 (scopehal-apps/lib branch alientek-dm40, README.md there); ngscopeclient
      showed the emulator's 1 kHz sine for 10 min (2557 records), test/dm40cli passes over lan and uart.
      Emulator finding: with the comparator trigger on, the scope sometimes never completes a frame after a
      start / restart (TRIG:STAT? WAIT for good; also on 5af9a5c); wavtest and run-emulator.sh use
      DBG:HWTRIG:ENABLE 0. Model or firmware? Check on the device (retest 3).
      Device retest: (1) wavtest's sequence by hand with tools/dm40scpi.py or pyvisa: WAV:POIN MAX + WAV:FORM
      WORD after DIG -> 53 256-byte block, Linux usbtmc read in one call, time it; DBG:DIAG? loop max during the
      read; (2) WAV:DATA? while running -> "#10" + Settings conflict; RUN pressed on the meter during a long
      read waits; (3) DIG in a loop (100x) at 1 ms/div with the comparator on (the default), Auto and Normal,
      on a 1 kHz generator loopback: every DIG ends in STOP; (4) CHAN1:OFFS 0.5 then DIG: the record's volts
      unchanged (the offset PWM shifts the codes; the emulator does not model that); (5) ngscopeclient-dm40 on
      /dev/usbtmcN: Run / Single / Stop, sample rate 50 MS/s .. 2.5 kS/s, V/div, offset, trigger level /
      slope, generator on / shape / frequency; (6) Bluetooth: WAV:POIN MAX -> "Too much data", the driver
      over rfcomm (--uart) gets 1902-point records; (7) kill the host mid-read (python: read one 4096-byte
      round, exit): front-panel RUN works again after ~2 s (the stall timeout); (8) the wave view (Source
      DMM, e.g. DC V on a 50 Hz signal): STOP, WAV:POIN MAX -> the record in V at the jacks (yinc / yorig
      from the DMM scale), compare with the screen; (9) the 50 MS/s rates (100 ns .. 50 us/div share the
      50 MS/s record) on the generator's 1 MHz square.
      Device 540e056 (2026-10-23, device-check doc "540e056"): (1) PASS: #553248 + 53 248 B in one read() call,
      66 ms, DIG -> STOP 0.21 s; (2) PASS "#10" + Settings conflict; RUN during a read not isolated (needs a
      hand on the key: DBG:KEY is a new command and ends the stream); (3) blocked, no loopback; (4) PASS
      (codes +59 / -64 per 0.5 V, volts unchanged within the open input's drift); (5) **FAIL, host side**: the
      driver hangs at OUTP? (1-byte reply "0", no newline: SCPITMCTransport::ReadRawData loops on
      bytes_fetched == bytes_requested == 1, 5 s timeouts forever; lan / uart never hit it) -> item below;
      (6) PASS with dm40bt_scpi.py ("Too much data"; 1902-point record), the driver over rfcomm not run;
      (7) PASS partly: after a killed mid-read the next command answers at once, no USBTMC clear; the 2 s
      stall timeout itself not isolated; (8) PASS on 1 V DC only (record 1.0007 V = screen; AC not run: the
      calibrator could not be driven); (9) blocked.
      Left (moved from "Device check 540e056", 2026-10-27): (5) fixed, PASS on decaf60 (next item); (3) / (9) need
      the generator looped back to the scope input; (2) RUN during a long read and (7)'s 2 s stall timeout need a
      hand on the RUN key; (8) on an AC signal and (6) the driver over rfcomm not run.
- [x] ngscopeclient-dm40 over USBTMC hangs on any 1-byte reply (OUTP? "0"), device 540e056: opendm40 ends USBTMC
      replies without '\n' (Bluetooth adds one); scopehal's SCPITMCTransport::ReadReply reads 1 byte at a time
      until '\n' and ReadRawData's inner loop reads again when it got exactly len bytes. Done (tmcnl-fix,
      "opendm40 USBTMC: every reply ends with '\n'"): usbtmc.rs `terminate` ends every reply with '\n' (text,
      ';'-joined, DBG blocks, definite-length blocks after the data incl. the streamed WAV:DATA?; text that
      already ends in '\n' keeps one). The scopehal loop is left as is (with the newline no reply has
      exactly the asked length). Emulator on vhci_hcd (the real Linux usbtmc driver): OUTP? -> "0\n", a
      52 000-byte WAV:DATA? in one read() ending in '\n'; ngscopeclient-dm40 test/dm40cli usbtmc PASS (3
      records, 1 kHz, 0.90 Vpp) and the GUI shows the sine (TRIGGERED); the 540e056 image there hangs as on
      the device (STOPPED, generator 0 mHz). dm40scpi info / read / get (307 kB BMP over MMEM:DATA?) / log
      (AD1, AD2 4800: 0 lost), dm40dbg ping / boot / genuine / shot, sigrok over tcp-raw and usbtmc, and
      DM40_USBTMC=emu all fine; emu scpi-tcp sends a block's own '\n' (no second one), wavtest checks it.
      Emulator only, also with 540e056: DBG:UPLOAD:END? takes ~7 s there (usbip_client upload: 45 s, OK),
      over the kernel driver it hits the 5 s usbtmc timeout and the link then reads empty until re-attach.
      Device retest: ngscopeclient-dm40 /dev/usbtmc2 Run (trace, generator fields), test/dm40cli usbtmc
      /dev/usbtmc2; reload.sh --image (DBG:UPLOAD / BOOT? / UPDATE); dm40scpi read / get / log; sigrok
      --show / -c range=; pyvisa USB query('OUTP?'). Also open: the driver's two threads interleave
      queries on one transport; the -L log is empty when ngscopeclient is killed; DM40Oscilloscope.cpp's
      comment "nothing follows it on usbtmc" is now stale (the read takes the '\n' either way).
      Device decaf60 (2026-10-23, device-check doc "decaf60" B1 / B2): PASS. ngscopeclient-dm40 --headless
      on /dev/usbtmc2: TRIGGERED, flat trace (open input), generator fields filled (Square 2 MHz 50 % 3 V
      1.5 V, GEN OFF); test/dm40cli usbtmc: all protocol checks ok, 5 records in 0.84 s, the 3 signal
      checks FAIL as expected (no loopback: flat 0.8577 V); dm40cli sets the generator to sine 1 kHz 1 Vpp
      (output stays off; restored by hand). dm40scpi info / read / log (readings, AD1, AD2 4800: 0 lost) /
      get (17 kB and 307 kB BMP), dm40dbg shot, pyvisa-py USB query('OUTP?') -> '0\n' (1 ms) all fine;
      pyvisa-py leaves the usbtmc driver detached (host side, reattach.py).
- [ ] sigrok/SmuView (merged from three items): libsigrok scpi-dmm DM40 profile exists as
      ../libsigrok-dm40/0001-scpi-dmm-add-support-for-Alientek-DM40A-B-C-dm40-fw-.patch (command set
      like the 34401A: CONF:<f>, READ?, FUNC?; 3 s delay after RES/CAP/TEMP/FREQ/CONT/DIOD). DC V works
      on the device via usbtmc/1209.0001; RES was blocked by H1 (fixed in 60605d2). 2026-10-21: every
      function passes with sigrok-cli in the emulator (emu/usbip_client.py scpi-tcp, tcp-raw; replies
      <= 0.86 s), AC+DC now also waits 3 s (first reading 1.4-1.6 s); patch regenerated as one squashed
      commit (branch alientek-dm40-upstream). Done in opendm40 (19c3592): CONF? (Agilent-style reply),
      INIT/ABORT no-ops, SYST:IDN:EMUL OFF|34401A (persisted, default OFF) for unpatched
      libsigrok >= 0.5.2 / SmuView (verified with the distro libsigrok 0.5.2 in the emulator).
      Remaining: SmuView on the device (needs a SmuView built against libsigrok-dm40/install; the
      stock AppImage only with SYST:IDN:EMUL 34401A), send upstream (owner's approval).
      2026-10-22 function change on the meter mid-acquisition (was open): the 1 s stop itself was
      already gone with 6c80e06 (READ? capped at 0.85 s after a change not made by this host); left
      were the samples around it. Chosen fix = both sides, since libsigrok's USBTMC Bulk-IN timeout
      is a hard 1000 ms in the transport (no profile setting reaches it) and only the meter knows a
      reading is missing: opendm40 answers "no reading" with +9.91E37 (SCPI NaN) instead of the overload
      +9.9E37 (READ? / MEAS? / FETCh? "Data stale"); a READ? waiting when the function changes on the
      meter answers at once with NaN, and so does a READ? / FETCh? whose preceding FUNC? / CONF?
      named another function and the host made no change since (scpi::take_asked / forget_asked:
      no reading filed under the old function); an
      autorange step V <-> mV or between the A shunts is no function change any more (scpi::func_id;
      it used to drop the host's settle wait). The profile keeps 9.9E37 = +inf and skips 9.91E37
      (model infinity_limit 9.95e37, now honoured by scpi_dmm_get_meas_agilent; receive_data no longer
      ends the acquisition on a skipped sample, which it did for every +1). Also in the profile:
      VOLT:AUTO / CURR:AUTO = "voltage" / "current" without a coupling flag (each sample DC or AC from
      VOLT:COUP? / CURR:COUP?; before, "VOLT:AUTO" matched the "VOLT" prefix = DC), POW read as
      power in W (before: unknown FUNC? = acquisition stopped) but not selectable (range hunt of up
      to 15 s; CONF:POW + READ? took 3.2 s after sigrok's 3 s delay), TEMP in F when UNIT:TEMP F.
      Emulator (sigrok-cli, tcp-raw, READ? bracketed by DBG:PING? = device time): every function
      selected from sigrok gives 6 samples, max READ? 0.85 s; one acquisition across 17 changes on the
      meter (Ohm, Cap, diode, continuity, Hz, temp, V DC/AC/AC+DC/AUTO, A DC/AC/AC+DC/AUTO, W via the
      wiring dialog, V) ran 1583 samples with no stop and no sample under the wrong function (29 NaN
      skipped, max READ? 0.851 s); a change injected between FUNC? and READ? (DC V -> AC V, Ohm -> DC
      V, DC V -> temp) gives NaN, then the new function.
      Device (33b44bd = eb39672 + later commits, USBTMC usbtmc/3.29, libsigrok 48aecd35, 2026-10-22;
      calibrator 1 V DC on V/COM, A jack open): --show lists voltage / current without coupling flag
      (AUTO) and no power; each of the 14 selectable functions --samples 10 exit 0, no "Timed out" /
      "transfer error" (READ? max 0.40 s DC V / ohm, 0.85 s cap / temp in OL); AUTO flagged DC on
      1 V DC, AC on the near-0 input; UNIT:TEMP F -> samples in degF (TC open: OL). One --time 175 s
      acquisition while BT changed the function 16 times (RES, CAP, DIOD, CONT, FREQ, TEMP, V DC /
      AC / ACDC / AUTO, A DC / AC / ACDC / AUTO, POW, V DC; BT = "the other transport", like a tap):
      911 READ?, 880 samples, 31 NaN skipped, max READ? 0.853 s, no error, every change followed
      (POW read as W).
      2026-10-23 Range + Digits (SmuView showed both greyed out: the profile had neither): opendm40
      CONF:<f> <range>[,<res>] (CONF without one = autorange, also on the current function),
      [SENS:]<f>:RANG <v>|AUTO|MIN|MAX|DEF / ?, :RANG:AUTO ON|OFF|ONCE / ?, :RES <v>|MIN|MAX|DEF / ?
      (scpi.rs header "Ranges and resolution"; RANG = the MF page's items, AUTO = the AUTO key,
      AUTO+ / AUTO kept; RES = the FR-05 rate FAST / MED); CONF? now names the real range (was 6E+03
      on 1000 V: derived from the readout). Profile: SR_CONF_RANGE (labels 600mV .. 1000V, 600uA ..
      10A, 60R .. 60MR, "Auto"; CAP / FREQ Auto only) and SR_CONF_DIGITS ("5" = RATE FAST 60000
      counts, "6" = RATE MED, +1 digit where the range has it), per-sample digits from CONF?
      (replaces FUNC? before each READ?). Digits is not a fake: the rate is the meter's real
      resolution switch (no 6000-count mode exists, see the FAST/MED/SLOW item). Emulator: every
      function selected + a range set from sigrok-cli acquires (bt-scpi.md "Range and digits").
      Device retest (USBTMC, libsigrok-dm40/install): --show lists the ranges per function; -c
      range=<each> in DC V (1 V in: 6V 1.0000, 60V 1.000, 1000V 1.0), A, ohm; the meter's range page
      highlights the set item, MANU in the status bar; range=Auto keeps AUTO+; digits=5 / 6 toggles
      the rate chip FAST / MED and the +1 digit; SmuView Range / Digits combos follow; AC+DC / AUTO
      coupling with a manual V range (emulator: an AC-part transient of ~2-5 s after the change, the
      MF page path too); CONF? on 1000 V / 10 A.
      Device 540e056 (2026-10-23, device-check doc "540e056"): --show PASS (V / A / ohm lists, cap / freq Auto,
      cont / temp none); range= each in DC V / A / ohm PASS (6V 999.1 mV, 60V 998 mV, 600V 1.00 V, 1000V
      1.0 V, 600mV OL); range page highlight + MANU PASS; Auto keeps AUTO+ PASS; digits 5 / 6 -> FAST 10/s /
      MED 2.5/s + extra digit PASS (SLOW reads as 6); CONF? 1000 V / 10 A PASS; the AC+DC transient is real
      (item below); SmuView combos left to the owner. libsigrok-dm40 leaves the usbtmc kernel driver
      detached after every run (/dev/usbtmcN gone; re-attach with USBDEVFS_CONNECT, scratchpad reattach.py):
      host side. Fixed (libsigrok 32030b22, ../libsigrok-dm40/0002-scpi-dmm-close-the-SCPI-connection-in-
      dev_close.patch, installed in libsigrok-dm40/install): an upstream bug, scpi-dmm's dev_close()
      returned early on status <= INACTIVE, which sr_dev_close() sets before calling it (since f1ba6b4b),
      so --show / --get / --set never closed the device (the transport's close re-attaches usbtmc).
      Emulator on vhci: /dev/usbtmcN stays after --show / --get / --samples / --set (gone with the old
      build). Device retest: the node stays after each sigrok-cli run (no reattach.py).
      Device decaf60 (device-check doc "decaf60" B3): PASS. --show and six -c range= runs (6V x3, 60V x2,
      Auto) with -d scpi-dmm:conn=usbtmc/3.71: rc 0, /dev/usbtmc2 still there after every run.
- [x] AC+DC (and AUTO) with a manual V range, device 540e056 (1 V DC, method A): the AC part reads 0.35 V
      (1.06 V via the MF page) for ~2-4 s after 6 -> 60 V, then a tail 18 -> 1 mV over ~10 s; 0.15 V for ~2 s
      after 60 -> 6 V. AC+DC READ? +6 % at 2.2 s, +0.9 % to 3.3 s. AUTO stays DC. Done (tmcnl-fix, dmm.rs
      ACS_*): a range / unit change in AC+DC / AUTO (manual, MF page, SCPI, autorange; not the entry or a
      phase switch) discards method A's AC results, and moves no range on them, until two results in a
      row fell by < 1 % each (or rose), or one is < 0.05 % of full scale, or 6 s passed; the first good
      one seeds the EMA. AC+DC gives no reading meanwhile (READ? NaN after 0.85 s, as at entry); AUTO
      shows its part with the AC part held and does not re-decide. Emulator (1 V DC; its node gave a 5.3 V
      AC part, readings up to 5.36 V, AUTO even flipped to AC): readings resume ~4 s after 6 -> 60 V and ~2 s
      after 60 -> 6 V, all within 0.1 %, AUTO stays DC; acdcmethtest A: DC 12 V + 0.5 V (6 -> 60 V
      autorange) settles in 4.2 s (540e056: 4.7 s). Device retest (1 V DC, method A): VOLT:ACDC:RANG 60 / 6 and the MF
      page items 6 <-> 60 V: no READ? off by > 0.1 %, the gap until the first reading (expect ~4-6 s,
      capped at 6 s), the AC part in DBG:ACDC:STAT? after it (the 60 V tail: ACS_STEP / ACS_FLOOR may need
      tuning; a device transient falling < 1 % per result would be let through early); AUTO coupling the same with the DC part shown throughout; a 40 Hz square on AC+DC: a range
      change gives the right AC part after the gap; autorange in AC+DC across 6 V (no hunting).
      Device decaf60 (device-check doc "decaf60" B5): 1 V DC, 30 AC+DC changes with READ? polling (SCPI /
      MF page, 6 <-> 60 V) + 16 with DBG:ACDC:STAT? polling: no fresh reading off by > 0.1 %, except 1 (MF 6 -> 60 V: DC part 0.526 V, item below); gap to the
      first fresh reading 3.0-4.3 s (5.7 s once); AC part 15-27 mV at the first 60 V reading, 1-2.5 mV
      after 9-12 s; 2-7 mV on 6 V. AUTO coupling (8 changes): AUTO,DC throughout, readings on at once,
      1 x DC part 0.528 V (MF 6 -> 60 V, item below). Square: 6 -> 60 V and 60 -> 600 V right after the gap
      (AC part 4.768 V), but 600 -> 60 V let the HPF tail through: 6.783 / 6.762 V (+0.55 / +0.24 %) for
      1.7 s (item below). AC+DC autorange across 6 V (DC 4..7 V): at most one range step per level, no
      hunting; a 0.5-0.8 V step down can make one excursion 6 -> 60 -> 6 V (item below). During the gap
      READ? returns the pre-change reading, not NaN (item below).
- [x] DC V: setting the present range again (SCPI <f>:RANG <same>, sigrok range=, or the MF page tap on the
      highlighted item) gives 1-2 readings at 25-51 % of the input at 0.46 / 0.66 s, on about every other
      repeat (device 540e056, 1 V DC: 0.257, 0.509, 0.375, 0.403 V); a real range change does not. The chart and
      MIN record it. Dmm::set rewrites the config with a 450 ms deadline but resets nothing when the mode is
      unchanged. Done (tmcnl-fix): set_range skips the rewrite when the chip already runs that mode (AC+DC:
      either phase) on that range (Dmm::same_row), it only makes the range manual; also V / A / ohm. The
      emulator has no partial conversion; there the re-set no longer costs a reading. Device retest (1 V DC):
      VOLT:RANG 6 / 60 x10 each (manual and from AUTO+), sigrok range= the present one, the highlighted MF
      item tapped: no READ? / chart MIN below 0.99 V; a real change 6 <-> 60 <-> 600 still fine; the one
      0.781 V after VOLT:RANG:AUTO ON from 600 V (real change) not addressed: watch for it.
      Device decaf60 (device-check doc "decaf60" B4): PASS for the same-range fix. VOLT:RANG 6 / 60 x10 each
      from manual and from AUTO+ (40 sets), 13 MF taps on the highlighted item, sigrok range= the
      present range: lowest READ? / CALC:AVER:MIN? 0.99869 V on 6 V, 0.99853 V on 60 V
      (0.99789 V in the real 6 -> 60 V changes). The 0.781 V case is
      real and reproducible: item below.
- [x] Scope touch: pinch = scale (timebase / V/div, 1-2-5 snap, value bubble), one-finger drag =
      offset, draggable trigger level, no clash with view swipes.  (320bc81, 100eb48, 6c0a484, b34f89a;
      verify on device, two-finger report rate first)
- [x] Combined DMM+scope view design: meter strip inside the scope page, 3-size type hierarchy, gesture
      map, renderer choice (own layered compositor, no GUI library) -> docs/combined-view-design.md (35703a5).
- [x] Combined view C1-C5 (abb5d5e), later the stock page + meter strip (d16a9c4); G1 drag/grab switch
      (74b5c16), G3 pinch (above), R1 redraw profiling (5c16fa5).  (verify on device; G4 A/B on device)
- [x] AC+DC: the AC part read ~5% low and wandered phase to phase (4.5..5.3 V on a 40 Hz 0..9.6 V
      square): AD3 enabled before AD2 (bfc92cc). (device-verified: AC+DC 6.8000..6.8004 V, sd 0.3 mV;
      stock/re/docs/history/acdc-stock-vs-opendm40.md §10.) FREQ 13.3 Hz in the combined view: moved to the crosstalk item.

## Device run 2026-10-13 19:10, native de57d3d (+docs) installed OK
- Verified: READ? ~0.5 s, MEAS:VOLT:AC? 0.5 s, MEAS:FREQ? 1.0 s; USBTMC survives a libusb timeout
  (sigrok) and answers afterwards; SCPI mode change highlights the right function button and clears
  the chart; AC+DC sub-reading signs next to their values; PT100/NTC labels legible; mOhm resolution
  (10.117 Ohm); MIN/AVG/MAX + session time on the chart.
- Calibrator (MR9270S+, resistance mode func 3 mode 1, i32 = 0.1 Ohm, accepted up to 4000 = 400 Ohm):
  with the DM40's test current the simulated resistance saturates at ~101 Ohm. In range: 10.03 @ 10,
  50.28 @ 50, 100.98 @ 100 Ohm (+1 % near the top = simulator compression); 0 Ohm reads 1.59 (sim
  floor). PT100 -100 C -> -99.76; -50 C -> -48.77; NTC10k3950 200 C -> 198.94; PT1000/NTC100k only
  below the simulator's ceiling -> OL (correct, out of table). Re-check with real resistors (PT1000,
  NTCs at room temperature need 1-100 kOhm).
- [x] H1: MEAS:RES? (and CONF:RES + READ?) answered 9.9E37 "Data stale" (ohm settle 2.5 s, wait expired
      at 1.9 s): wait from max(now, dmm.deadline()), single READ? capped at 850 ms for sigrok.
      (60605d2; verify on device)
- [x] Review M1 (CHECK_CLEAR_STATUS pending only while a reply is in flight), L1 (ABORT_BULK_IN marks
      cleared), L2 (Bulk-OUT left while a command is pending), L3 (toggle reset never writes EPENA/EPDIS)
      from stock/re/docs/history/native-review-2026-10-13.md "Pre-flash review de57d3d".  (60605d2, 571534c; verify on device)
- [x] AC agent questions (a) 40 Hz PWM AC part low, (b) AC+DC AC part = plain AC V, (de57d3d wander):
      answered by bfc92cc (above). (c) A input open: the 150 uA item. (d) FREQ: the crosstalk item.
- [x] stock/dm40fw.py is untracked but build.sh needs it (packs the native image): commit it. (544dd40)

## UI toolkit decision (user, 2026-10-13)
- Keep our own renderer + layer compositor (docs/combined-view-design.md §7) for now. Considered:
  Slint (native Rust, MCU line renderer, real widgets/swipes; GPLv3 or commercial for embedded),
  LVGL via lv_binding_rust (C, MIT, experimental bindings), egui (needs std + framebuffer/GPU-style
  rasteriser: does not fit 125 KB SRAM). Revisit with a one-screen Slint/LVGL prototype if the
  hand-made widgets stay clumsy. Meanwhile: layout rules + automated overlap check for every screen.
- [x] Measure the real DMM conversion period per rate — resolved by the datasheet + emulator
      (emu/hy3131/RESEARCH.md 3), device confirmation pending: AD1 period = OSR * 30 / SYSCLK, x2
      with AD1CHOP (set in every stock R22): 0x15 (OSR 8192) 100 ms, 0x17 (OSR 32768) 400 ms. Both
      old guesses were wrong (~9.4/s at OSR 32768 and OSR / SYSCLK ~150/s). opendm40: FAST 10/s, MED
      10/s moving / 2.5/s steady, SLOW as MED (one reading per conversion, 16-conversion window, 32
      on ohm r0..r3); hy3131::ad1_period_us, dmm.rs FR-05 table. Device: SENS:RATE:PER? per rate on
      a steady input (expect 0.4 s for MED and SLOW, 0.1 s for FAST).
      Decided 2026-10-14: SLOW reports every conversion (it was one per 4 = 0.625/s, sized for the
      old ~107 ms guess, so READ? never got a fresh SLOW reading). MEAS:<f>? after a function change can no
      longer reach a full +1-digit window within SCPI_SETTLE_MS 1.5 s (needs ~3.2 s); kept for
      the 2 s PyVISA default.
- [x] Data logger runs at 38 Hz at OSR 2048 — that is the chip, not the SPI link (datasheet +
      emulator, device confirmation pending): OSR 2048 chopped = 25 ms = 40/s. The SPI read is ~122
      us/sample. Faster rates are now opt-in: LOG:OSR 0..2 (OSR 256 chopped 320/s) and LOG:CHOP OFF
      (AD1CHOP = 00, x2: OSR 256 unchopped 1.5625 ms = 640/s, the chip's max; converter offset not
      cancelled, more low-frequency noise); LOG:PREamble? reports chop as a 6th field. The logger's
      nominal period (gate, `missed`) was 16x too short before: `missed` counted ~60 phantom
      samples per real one. Device test: LOG:OSR 0..3 x LOG:CHOP ON/OFF sweep with LOG:RATE?,
      LOG:COUNt? (missed) and the noise / offset (counts) per setting on a steady 0.5 V and a
      shorted input, to fix the real rate / noise / offset table.

## Owner hands-on feedback 2026-10-13 night (build 97fb89a / experiment e73fa96)
- [x] Scope drag lags: while a drag is in progress freeze acquisition/redraw of new frames and only
      re-render the shift of the frame already shown (column diff); resume on release.
      (100eb48, b1b4cea, 5c16fa5; verify on device with DBG:PROF?)
- [x] Scope trigger mode Auto and scope AUTO (autoset) "strange": trigger position counted twice +
      blind 800-sample window (9e83d8a), stock AUTO-set (100eb48), comparator trigger on by default
      (4f876cf, device-verified), AUTO period search / clip checks / mid-swing trigger (2ec4247,
      c16b37b, 89a42d6; verify on device).
- [x] Temperature PT100/PT1000/NTC: show the sensor type much larger.  (671006d; verify on device)
- [x] Thermocouple/temperature offset setting: slider instead of +/- buttons.  (3287199; verify on device)
- [x] Buttons feel imprecise to press: touch-down confirmed on any two pressed frames (7eb7365), hit
      slop on lists/keypad/menu/range + tap after an uncaptured long press (87b6936, d7e31bf),
      DBG:TOUCH? log + tools/touch-log.py.  (verify with the owner's finger)
- [x] DMM frequency: ~700 Hz shown as "1 kHz": Hz readout decimals.  (af9c931; verify on device)
- [x] Pinch-to-zoom on the scope: see "Scope touch" above (320bc81, 100eb48).

## Owner notes 2026-10-14 (../замечания.txt)
Firmware:
- [x] °F shown in the top-left status slot is odd; move the °C/°F choice to a global Settings option and show only the chosen unit.
      (26d64d4: Settings 2/2 Temp unit, UNIT:TEMP; verified on device f80524f: UNIT:TEMP F/C, reading, CJ, SYST:TEMP?)
- [x] Temperature settings: remove the unclear "Offset 0" button (reset via the slider/long-press instead).
      (68dfc42: offset stepper + keypad, no Offset 0; verify on device)
- [x] "NTC user" is unclear: make its R25/Beta editable right there (or remove); explain on screen.
      (68dfc42: R25 / Beta fields on the temperature page; verify on device)
- [x] Second resistance mode present in the stock is missing in opendm40 — check stock parity (dmm_v2_6.h) and add it.
      (5cd0d5b: stock "ON" low-current variant made visible; verify on device)
- [x] Tap on the DMM value cycles averaging/resolution: fewer digits = faster readings; compute digits vs OSR/noise from the
      HY3131 timing + measured noise (not ad hoc); the +1 digit always drawn small at the end.
      (1710c38: FAST/MED/SLOW with a counts / rate toast; verified on device f80524f)
- [x] Power-meter mode: HY3131 can measure V and I at once (AD2/AD3 + MUL → active power in RMSDATA); Vrms, Irms, P (and
      decided: see ../docs/history/decisions-2026-10-22.md #4 (energy counter resets on boot)
      energy) by switching internal modes — check feasibility with the DM40 front-end wiring first.
      Researched: GO, docs/power-meter-and-misconnect.md §3 (I on the stock OP1 path, V via OP2→AD3 DC-coupled,
      signed 38-bit X·Y RMS read, ~10-12 days). Gate: device test T1 (OP2 on SENSE). Owner: fuse/CAT/10 A limits.
      Done (powermeter.rs / ui_power.rs / app_power.rs, doc §3.11): function 18, row B with SPHACAL before the
      AD3-first enables, P→V→P→I on R34 (2 discards), DC parts from the LPFs, S/Q/PF, Wh/VAh, per-channel autorange,
      burden compensation, current-limit countdown, auto-off held; fallback (stock AC V / AC A alternated, VA only)
      behind POW:METH / the page; long press on A or "Power W" on the Current page → wiring dialog; Power Settings
      page; SCPI CONF:POW / FETC:POW:ALL? / [SENS:]POW:…; DBG:POW:ROW?/POKE/STAT/RAW?/DISC. Emulator Power role
      + emu/hy3131/powtest.py (11 scenarios OK). (verify on device: T1, T2, T3 of doc §3.11, then T5/T6)
      Device 2026-10-22 (e606771, calibrator on V-COM, A not wired; doc §3.11 "Device results"): T1 GO -
      OP2 on SENSE linear, default row best (POKE 14/15 kills the signal, R35 0x44/0x74 no better), but raw
      gain -2.05 % / -1.63 % / -0.33 % (6 / 60 / 600 V) -> 0.5 % only after a per-range gain; T3 at zero
      current: YY clean from the 1st result, XX needs the 2 discards (keep DISC 2); UI dialog / settings OK.
      T2, T3 with current, T4-T6 pending (need current through A).
  - [x] Device follow-ups (doc §3.12): per-range V gain derived from the factory AC V / DC V gains (residual
        -0.03 / -0.08 / +0.12 % vs T1) + POW:VOLT:CAL self-cal (AC V / DC V reference, persisted); LPF zeros by
        POW:ZERO (leads open; internal AGND zero assessed, not default); f in power mode (adaptive R21 window
        around the tap's DC, PB4 = the current as fallback; cause: the unipolar T1 square); V shown + "I ranging"
        on entry, per-channel parts kept, own statistic first (~0.9 s per autorange step); I_dc with the AC A RMS
        gain while the LPF gain is uncalibrated; DISC 2 kept (XY / XX with current untested).
  - [x] DC power (owner): POW:COUP DC|ACDC|AUTO; DC = one fixed row (I on AD1 with the DC A path / factory gain
        and its own POW:ZERO zero, V on OP2 -> AD3 LPF at the same time, AD1 enabled last: note 7), a reading per
        100 ms, burden comp; AUTO by the AC/DC ratio (B) and the Y ripple (DC row). Persist v9.
  - [x] 7th DMM function button "W" (64 px buttons, 68 px pitch, DMM page + combined band); the wiring dialog
        only until confirmed once; restored after a reboot; "Power W" on the Current page removed. Persist v7.
  - [x] Power page rounds 2 / 3 (doc §3.12): real W glyphs (stockfont_w.rs); coupling the stock way (active W
        button: DC -> AC+DC -> AUTO, glyph on the button / above the unit, Settings row DC / AC+DC / AUTO); V / I
        on top, the energy box below (E | PF | S | Q | f), tap on the main value cycles P / Wh / VAh, reset with a
        confirm; the energy is not persisted over a power cycle (owner's call).
  - [x] Owner round 4 (doc §3.14): no false OVERLOAD in the power meter with I AUTO (an over range on the uA / mA
        shunt is a one-hop up-range to 6 A, no alert; the red OVERLOAD stays for a manual I range); coupling DC /
        AC / AC+DC (AC = the AC parts only, DC parts and zeros not used; AUTO dropped, POW:COUP AUTO = ACDC,
        deprecated; persist value 3 = AC, the former AUTO loads as AC+DC); stock DC / AC / AC+DC glyphs; Power
        Settings readable ("At load" with its help line, Zero / V cal dialogs, coupling meaning, status text);
        Help 7/7 "Power meter". powtest istep / istep-ua / iman-ovl / ac, senttest ovl-pow-auto-*.
        (verify on device: the check list of doc §3.14) Device d8e249d: items 3 and 5 PASS (docs/device-check-2026-10-21.md "d8e249d" 1);
        1, 2, 4 need current through A; 6 not run.
  - [x] Owner round 5 (doc §3.16): "At load" OFF by default (persist bit 10 = ON opt-in; every older record
        loads OFF: an old explicit ON is indistinguishable from the old default), "@load" marks after the V unit
        and at the main readout's top left while on, the help line says when (V leads at the source, meter in
        series); entering W starts in DC as V / A (a restart in W keeps its coupling); AC floors (V 0.5 % of range = dmm
        AUTO_FLOOR_V "to DC"; I 0.5 %, not AUTO's 2 %, which blanked 0.1 A on 6 A): AC coupling below them V / I 0, PF / S / Q / f "--" (SCPI 9.91E37),
        AC+DC Q 0; Q of noise hypot(Vn I, V In) with Vn / In 0.1 % / 0.25 % of range reads 0; coupling hints DC?
        (AC+DC, no AC part) / AC? (DC with V_ac from the DC row's own YY RMS: no register write, no note-7
        risk; I_ac not available without an R34 switch). powtest entry / ac-floor / acdc-floor.
        - [ ] Verify on device: the check list of doc §3.16 (the pure-DC residues of V_ac / I_ac against the
              floors and NOISE_V / NOISE_I).
              Device 92fb8fa (device-check doc "92fb8fa" 3), no current through A: "At load" off after the
              update (POW:BURD:STAT? 0), the toggle, help lines, "@load" marks, kept by a reboot; W entry in DC,
              the tap cycle, a reboot in AC+DC keeps AC+DC; AC on 1 V DC: V / I / P 0, PF / S / Q / f "--" and
              9.91E37; AC+DC: Q 0, "DC?" + toast "No AC: DC is more precise"; DC on the 40 Hz square: "AC?" +
              toast "AC present: use AC+DC": PASS. Left (needs current): steps 1, 2 (V at 1 / 2 A), 4 / 5 with a
              load (residues, mains PF), 6's Settings hints-off case.
  - [x] AC+DC parts on the page (doc §3.17, the owner's V2): "DC x / AC y" sub-lines under V and I, "DC x W +
        AC y W" under P, from the measured LPF / RMS / X*Y parts with the AC floors and "@load" per part; main
        readout +7 px, lower row +17, chart 81 px from 192, side column without VAh, in AC+DC only (every other
        page pixel-identical). ui-render p26..p29, touch-test both layouts.
        Device 2f56ff9 (device-check doc "2f56ff9" 7b, open jacks): steps 1 / 3 layout PASS; steps 2 / 4 need current.
        - [ ] Verify on device: the check list of doc §3.17.
              Device 92fb8fa (no current): sub-lines under V / I / P, tap to Wh / VAh empties the P line, back
              to DC with no leftover pixels: PASS. Cosmetic: a coupling switch keeps the old layout / values for
              up to ~4 s (until the first reading of the new coupling). Left: steps 2 and 4 (current).
  - [ ] Verify on device: the re-check list of doc §3.12 (zero, V cal, f on the square, W entry, DC method /
        AUTO, then T2 / T3 / T4 with current through A).
        Device c50425f (device-check "c50425f" 5): PASS for W entry (dialog once, then direct), tap-W coupling
        cycle + glyphs, POW:ZERO (Y 1072, X ~890 / ~160), V vs DC V <= 0.063 % on 6 / 60 V in DC and AC+DC, f 40.0018
        Hz on the square (R21 0xBF), P / Wh / VAh tap, "Reset energy?" (1st Yes did not reset, 2nd did), reboot in W.
        Open: POW:VOLT:CAL, T2-T4 with current through A.
        Device d8e249d (docs/device-check-2026-10-21.md "d8e249d" 1): PASS for the coupling tap cycle DC -> AC -> AC+DC (round 4), Power Settings
        dialogs (At load, Zero -> Start, V cal -> Cancel, Wiring, Limits), POW:ZERO, f 40.0018 Hz, energy
        reset with Yes 13 s after the long press, no "-0"; POW:VOLT:CAL DC reference 6 V: 1.020998 (doc
        1.0209), 60 V with 10 V: 1.016612 (doc 1.0170; over-corrects 0.06 %); 60 V with 5 V = SMALL (the list
        above is wrong: 8 % < the 10 % window); AC reference blocked (calibrator: 4.77 V AC max). Gains set
        back to factory. Still open: T2-T4 with current through A.
        - [x] V cal on 6 V AC-coupled with the 0..9.6 V square returned OK (1.021410) although the peak clips
              the 6 V range: the DC-kind path has no clipping guard. Fixed: BIG when |DC| + sqrt(2) x AC RMS
              exceeds the range, either kind (powtest vcal-clip). (The dialog's "10-70 %" vs DC's 100 %: open.)
        Fixed after c50425f: the 1st Yes came ~13 s after the long press (screenshot 38 -> 39: 12.9 s; the
        2nd try 2.4 s) and the confirm auto-cancelled at 10 s, so the tap hit the bare page: now 60 s
        (dialog::ENERGY_MS, as the wiring / firmware questions). Emulator: powtest.py ereset (Yes 13 s
        after the long press resets; FAILs on 3c5f891). Negative zero: every power readout (main, E box,
        PF / S / Q, AVG, V / I) shows +0 for a value that rounds to zero (pw::no_neg_zero; ui-render
        p15 / p16, meter-test). Generic DMM readouts unchanged (dmm::format). Unit case: the E box already
        says "kWh" (F14 has k); the main readout / aux row now too: our LETTERS 'k' (tools/mkwglyph.py,
        FreeSans Bold k scaled by the stock h ink box, stockfont_w.rs OWN_GLYPHS); unit_text keeps 'k'
        only in the energy units (KΩ / KHz stay as the stock writes them). The chart axis labels and
        history boxes (ui::num_unit, all charts) show +0 too ("-0.000mW" on the power chart).
  - [x] TODO remainder (doc §3.13): BT cmd 09 / 06 = the DC V byte (0x28 auto) with P as a V-group value
        ("230.02 V" = 230.02 W; mW as mV; value 2 / 3 FFFF; cmd 06 echoing the byte keeps the power meter),
        BT SCPI runs every POW: / FETC:POW / CALC:LIM command (only DBG:* refused); limits in W: 10th slot
        "Power" (CALC:LIM in the power meter, the "Settings conflict" refusal removed; Hold & Limits from
        Power Settings > Limits; badge in the AUTO label's slot; fail beep; persist v10 p[62..64]); Wh limits
        not offered (one-shot budget, not a band); RESistive phase cal stays refused, now with the reason
        (flat cos optimum: the step fit needs ~3e-5 of P / S vs 1e-2 block ripple; OSR group delays cancel),
        REAC refuses NOTREACTIVE (|P/S| > 0.32) / RANGE / FLAT instead of storing a clamped value,
        POW:PHAS:CAL? = outcome,SPHACAL; "tap cycles P / S / PF / E" superseded by the owner's P / Wh / VAh.
        powtest limits / phasecal-ref / bt (all 21 OK); ui-render badge matrix + p17 / p18 (0 violations).
        (verify on device: the check list of doc §3.13 - phone app, W limits + beep, cal refusals, BT SCPI)
        Device 8b9cf79 without current (docs/device-check-2026-10-21.md "Bluetooth (8b9cf79)"): cmd 09 = 0x28, "0.0000 mV" for 0 mW, value 2 / 3
        FFFF; cmd 06 0x28 keeps POW; BT CONF:POW / FETC:POW:ALL? / CALC:LIM:UPP 100 (FAIL? 0) / UPP -1 (FAIL?
        1, HI badge) OK; RES refusal text OK. Left: the phone app itself, limits / beep with a real load,
        persist v10 over a reboot, REAC cal on a capacitor.
- [x] Detect wrong connection (voltage on the A jack / current mode with voltage present, leads in the wrong jacks) and warn.
      decided: see ../docs/history/decisions-2026-10-22.md #5 (A-JACK / LIVE? alternate with HOLD in the status slot)
      Done (decision #5): ui::slot_alert / alert_alternates; DmmView.alt = the app's 1 s strip_alt (now
      toggled in every view); the live alert (meter.live) is drawn while HOLD / AutoHold freeze the readout
      and an alert change under HOLD repaints at once (shown_state); strip_data the same rule. ui-render:
      both phases over the DMM page / strip matrices + a phase-flip stale-pixel check (0 violations).
      (verify on device: HOLD then current into A in DC V -> HOLD / A-JACK alternate 1 s each)
      Device c50425f: LIVE? / HOLD alternate ~1 s in continuity with 1 V, slot clean (A-JACK needs the A jack
      wired; Ω with a positive source never shows LIVE?: by design, needs a negative result).
      Researched: GO for current-in-A-jack in non-current modes (idle comparator on PB1, 47.5 Ω shunt, trips ~0.44 mA),
      partial for "V leads while in A mode"; blown fuse not detectable (fuse-check dialog instead). Doc §4, ~4-5 days.
      Done (sentinel.rs, doc §4.6): A-JACK by the comparator on PB1 (DC V / mV / ohm / temp) and the AD1 peak
      hold on PB1-PB3 (AC V / mV); V-JACK? by the comparator on the /1000 tap (DC A, SS5 only); LIVE? from the
      ohm / continuity / diode results; status-slot / strip badge, toast, 3x50 ms beep every 2 s; Settings 3/3
      "Jack alerts"; SYST:ALER? / :ENAB, DBG:SENT?. Emulator model + 18 USB/IP scenarios; cross table unchanged.
      (verify on device: T7, T9, T10 of the doc; T9 decides PKH_RESTART)
      Device f80524f FAILS (docs/device-check-2026-10-21.md "f80524f" 4): AC V 6 V range reads a ~37.2 mV floor
      with alerts ON (0.15-0.35 mV OFF, any DC input) -> PKH_RESTART / AD1 in AC V moves the RMS; A-JACK false
      trip in DC V with 10 V on V-COM, A jack open (calibrator 1 <-> 10 V autorange soak: streak 96, AJACK).
      T7 equal within noise for DC V / DC mV / ohm / AC mV / Hz. T9 / T10 not run (A jack not wired).
      Fixed (doc device-check "Fixes after f80524f"): AC V sentinel row holds OP1CHOP = 00 (OP1's 2 kHz chopper
      starts with AD1: offset square x10 -> 37 mV RMS floor; AC mV reads OP2, clean); DC V 60 / 600 / 1000 V
      carry no comparator sentinel (those rows drive the power bus from OP3; r0 keeps it, and leads in A/COM
      leave V-COM open = r0). DBG:SENTinel:VARiant 0..7 restores the f80524f rows per bit for the re-check.
      Host tests + senttest dcv-step. (verify on device: re-check list of that section)
      Verified on device c50425f: AC V ON == OFF within 0.04 mV (VAR 1 brings back 37.15 mV: the chopper was the
      cause); DC V 1 <-> 10 V soak 5 min +0 hits, kind OFF at 10 V; VAR 4 on 60 V trips again; 10 V on manual 6 V
      no trip.
  - [x] The rest of §4 (doc §4.7, 2026-10-22): OVERLOAD shunt rule (uA / mA over range -> 10 A at once,
        latched, OVLD badge + 300 ms beep + toast, not maskable), sentinels in AC+DC (both phases), AC A (AD1 on
        the /1000 V tap), Hz (AD1 peak on PB1), continuity / diode / cap r0..4 (AD2 + OP2 on PB1); cap r5..7 and
        DC V 60 V+ stay uncovered (documented gaps); DC? (AC V) / AC? (DC V 6 V) / FUSE? hints; fuse check
        (Current Settings "Fuse check", SYST:FUSE:TEST / SYST:FUSE?); STATus:QUEStionable bit 9 (+ bit 1
        OVERLOAD). Emulator: model + 27 new senttest scenarios, 362 model checks; host tests; ui-render 0.
        (verify on device: doc §4.7 device test plan T8, T11..T16; DBG:SENTinel:VARiant +8 / +16 back out the
        new rows / the alternation if a reading moves)
  - [x] BUG (owner 2026-10-22): A AUTO coupling (and AC+DC A, plain AC / DC A AUTO), AC 1 A applied ->
        OVERLOAD at once, though an AUTO up-range is normal operation. Emulator: couptest `iac` (enter
        CURR:COUP AUTO with aca:1@60) has SYST:ALER? OVERLOAD from fw uptime 2 s on HEAD 11c4f9b. Cause: A
        enters on DC mA r0 (stock, `select`: FUNC_A_AUTO -> 8); `overload_trips` treats the mA / uA shunt's
        top range in AUTO like a manual range (red latch, beep, toast), only the power meter got the quiet
        rule (doc 3.14). Proposed: in AUTO a quiet one-hop up-range to the A shunt's 6 A range (DC 7 r0, AC 4
        r0, AC+DC its DC phase), no latch / alert; OVERLOAD only for a manual range. Update senttest
        `ovl-dcma` / `ovl-acma` / `ovl-acdc` (they expect OVERLOAD in AUTO), doc 4.7 table. Open: enter A on
        the A shunt instead (user-feedback §5.4 note), so 1 A never flows through 0.5 ohm for the ~1-2 s of
        the walk up.
        **Fixed (073b309):** the proposal as written (`dmm.rs` ovl_trip): AUTO / AUTO+ over range on the
        shunt's top range = one hop to 6 A (DC 7 r0, AC 4 r0, AC+DC / AUTO the DC phase), no latch / alert /
        beep / toast; OVERLOAD only for a manual range (doc 4.7 table). Emulator on 4ed771d vs fix:
        couptest `iac` (enter AUTO, AC 1 A; every sample checked) OVERLOAD from fw 2.0-5.4 s in methods A / B
        / C -> NONE in all three; new `idc` (DC 1 A) the same; senttest `ovl-dcma` / `ovl-acma` / `ovl-acdc`
        OVERLOAD -> NONE with the reading in its window, new `ovl-dcma-man` / `ovl-acma-man` keep OVERLOAD
        for a manual range, `aprot-to-dca` expects NONE. Still open (not this fix): enter A on the A shunt
        (the ~1-2 s of 1 A through 0.5 ohm before the hop remain). Device retest: list "dmm-bugs" below.
  - [x] BUG (code reading, same session): AC+DC / AUTO down-range on one part. After `reset_values`
        (entry, every unit change) `ac_raw` = 0, and the DC phase autoranges on hypot(ac_raw, DC): a pure
        AC signal reads ~0 there and walks down. A: mA -> uA (a_auto 3) with 1 A AC flowing (47.5 ohm!),
        then the AC phase trips; V: 230 V AC -> 6 V, with AUTO+ into mV (by the code, not run). The AC phase
        after a unit change has dc_val = 0 likewise. Proposed: a down-range needs the other part measured
        since the reset (`acdc_seen` bit: DC phase needs AC, AC phase needs DC); up-ranges never wait. The
        stock's S+0x60 hypot likely has the same gap (INFERRED). Test: couptest `iac` phase 2 (AC 300 mA ->
        mA shunt, NONE) and a V scenario entering AUTO on AC 230 V (range stays >= 600 V, no mV).
        **Fixed (073b309):** `acdc_down_ok`: a step down needs the other part measured since the last reset
        (`Dmm::acdc_known`, bit 0 AC / bit 1 DC: `acdc_seen` without its range-change clear); method A's DC
        phase (`acdc_down`) waits for the AC part, B / C's AC result for the DC part; ups never wait.
        Reproduced only with the model's new `rmsfill:` (the stock AC rows' RMS window filling, INFERRED
        1.5 s): the AC part's settle (ACS_STEP) outlasts the AC phase, `ac_raw` stays 0 from the quiet hop's
        reset, and the DC phase's hypot(0, ~0) asks for a step down for ACDC_DOWN_MS across phases. couptest
        `iacfill` (method A; B / C skipped: their single row refills at every range change under that model):
        without the guard 6 A -> 60 mA with 1 A AC flowing at fw 10.96 and 19.87 s, with it the A shunt
        throughout. Without the fill (`iac`, `vac230`) the AC part settles within the first AC phase and the
        gap does not show; `vac230` (V AUTO on 230 V, never the mV range, >= 600 V while 230 V) stays as a
        regression check. In B / C both parts usually come from the same result, so the guard rarely acts
        there. Whether the device's settle outlasts an AC phase is not known: retest list below.
  - [x] OVERLOAD badge stays after the overload is gone (owner 2026-10-22): the latch shows red OVLD while
        the meter measures fine on 10 A. Release = 5 s below max(half the scale it returns to, 2 mA), so
        e.g. manual 60 mA with 40 mA now never returns; AUTO after a uA overload keeps the badge for any
        current > 3 mA; a re-trip within 30 s makes it sticky. Proposed: red only at the trip (beep, toast,
        a few s), then a grey / amber "10A^" info badge (tap = back); return when the reading fits the old
        range with margin (~0.8-0.9 x FS; re-trip is at 1.03 x, the 30 s sticky rule covers flapping).
        Mostly moot in AUTO if the quiet hop above lands.
        **Fixed (073b309):** AUTO has no latch any more (above). Manual: red OVLD with the beep and toast for
        `OVL_RED_MS` = 3 s, then an amber "10A" badge / strip (`Alert::Ovl10A`, display only via
        `Sentinel::shown`; SYST:ALER? and STAT:QUES keep OVERLOAD for the whole latch, no new toast / beep);
        release below 0.85 x the scale it returns to (`OVL_RELEASE_FRAC`, was 0.5) for 5 s, the 30 s sticky
        rule unchanged. senttest `ovl-dcma-man` (manual 60 mA, 2 A then 40 mA: back on manual 60 mA with
        40 mA; at 0.5 x it never returned), host tests `overload_badge_red_then_amber`, ui-render d25 / c22
        (amber, 0 violations). "Tap = back" not done (the user's range keys do it already).
  - [x] 4-20 mA % hidden below 3.6 mA (NAMUR NE 43; 0 mA read -25 %): 11c4f9b.
  Compared with UNI-T UT197 (2026-10-22, its manual §16.2 + the A / mA fuse notes): the UT197 senses a PLUG in
  the mA/uA or A jack (no signal needed) and flashes "InErr" + beeps in any non-current position, i.e. before
  the probe touches a circuit; the sense path runs through the fuse, so "InErr shows" doubles as the fuse
  check. Ours sees only a current already flowing (mains on A-COM in V mode blows the fuse first, §4.3), and an
  unconnected plug is electrically invisible. Follow-ups, in order:
  - [ ] Device T9 / T10 with the A jack wired (never run), plus the f80524f re-check list (AC V floor with
        alerts ON, DC V 1 <-> 10 V soak without A-JACK). Gate for everything below.
        (re-check list PASS on c50425f; T9 / T10, AC mV / TC soaks still open)
  - [x] "Leads in A?" reminder (closest firmware stand-in for InErr): on a change from a current function to
        a non-current one, toast "Leads in the A jack? Move to V" + a beep, at the moment the UT197 would warn.
        Fires also when the lead was already moved: own option in Settings 3/3 "Jack alerts" (and in the
        SYST:ALER:ENAB mask); decide whether it repeats or needs a tap to dismiss.
        Done (doc 4.9): any path A (DC / AC / AC+DC A, power meter) -> non-A; one 200 ms beep at once + one
        toast "Leads in A jack? Move to V" (the TODO text leaves 1 px to the frame) after the new function's
        first reading (<= 1 s). Decided: no repeat, no tap (the firmware cannot see whether the lead moved;
        A-JACK takes over when a current flows); no alert state, so decision #5's HOLD alternation is
        untouched. Fuse check exempt. Settings 3/3 row "A-lead reminder" ON / OFF (default ON, p[38] bit 15),
        SYST:ALER:ENAB bit 3 (ON = 15), DBG:LEADs?. Host test leads_reminder, emu/hy3131/leadstest.py, ui-render.
        (verify on device: doc 4.9 device check list) Device 33b44bd: steps 1, 2, 4 (volume 0: toast, no
        beep), 5 (OFF / ENAB 7 / Jack alerts cycle / reboot) PASS; 3, 6, 7 not run. Open (cosmetic): the
        toast comes at the first reading, before the page's chart / stats repaint: ~1.5 s of the old A
        page's chart labels / REL unit under it.
        **Removed 2026-10-24 (owner):** the toast on A -> V read as a false "Current in A jack" (doc 4.9).
        The toast, beep, Settings row, ENAB bit 3 (ignored now), DBG:LEADs? and leadstest are gone;
        p[38] bit 15 is ignored on load and not reused.
  - [x] Fuse check (UT197's procedure, our doc §4.3 design): guided dialog in ohm mode (600 ohm, PE = 2),
        lead V/ohm -> A, ~48 ohm = OK, OL = FUSE OPEN, else "Check leads"; the 0.05 ohm (PE0) step left out
        (it tests the MOSFET, not the fuse). FUSE? hint: 60 s below 0.1 % of the range (AC 5 %). Doc §4.7.
        (verify on device: T14, T15)
  - [x] Sentinel coverage: AC+DC, AC A, Hz, continuity, diode, cap r0..4 done (doc §4.7); open: cap r5..7
        (both op amps and AD1 busy: no path) and DC V 60 V+ (the f80524f false trip). T8 gates the AD2 rows.
  - [x] OVERLOAD shunt rule (§4.3 / §4.7). (verify on device: T11, with a current-limited supply first)
  - [ ] Hardware note (HW-1, for a board revision / mod): a sense contact in the A jack wired to a spare
        HY3131 input through a high-value resistor would give a real InErr and the fuse check with it.
- [x] (B) AD2 fast data logger, main-loop (E1, no ISR): LOG:SOURce AD1|AD2, LOG:SRATe 600..9600 (R26 ladder,
      hy3131::ad2_r26_for), AD2F-gated 19-bit reads with a sign-copy torn-read check, AD3/RMS/LPF off, burst
      polling (500 us), LOG:COUNt? lost (span-based), LOG:GAPs?, LOG:PREamble? source/req/nominal/r26/coupling,
      panel "AD2 meas/nominal Sa/s" + loss row; ring unchanged (2048 x 8 B: 213 ms at 9600/s). Rows: AC mV / AC A /
      mA / uA (DC-coupled), AC V (AC-coupled). Host: dm40scpi.py `log --source ad2 --rate N` (CSV with
      lost_before), `ad2 torn|noise`; emulator AD2 grid + noise model. Hooks: DBG:AD2:TORN? / NOISE? / DRAW.
      Emulator only so far (50 MIPS: ~2 kS/s readout, not the device's): docs/dmm-input-scope-feasibility.md 7.
- [ ] (B) device session (30 min, feasibility doc 7.3): torn reads per OSR, loss per rate with / without redraw,
      decided: see ../docs/history/decisions-2026-10-22.md #3 (one TIMER ISR, exclusive bus hand-off, gated on SPI2)
      shorted-input ENOB, a waveform; then decide the rate cap and the ISR (E2-E4).
      Partly done on device f80524f (docs/device-check-2026-10-21.md "f80524f" 1): torn reads none at any rung;
      waveforms AC V / AC mV ok; converter floor ENOB 16-17.6 (AC A open). FOUND: the chop-ON rungs give AD2F
      ~2x faster than ad2_r26_for's nominal (600 -> 1128/s, 2400 -> 3832/s measured; 0x24 = 0x04 readout), so
      LOG:COUNt? lost clamps at 0 below 9600; link ceiling ~6.5 kS/s; DRAW 2 = 26 ms gap per 100 ms. Open: the
      --chop off pair, a shorted-input mV noise run, then the rate cap.
      Fixed: ENCHOPAD2 does not slow AD2 (0x25 = 0x05, 0x24 = 0x04): ad2_period_ns ignores it, the ladder is
      1200 / 2400 / 4800 / 9600 S/s with or without chop (600 does not exist: /4 OSR 1024 = 1200), so nominal /
      lost are right; the bit-banged link topped out at ~6.5 kS/s (9600 showed ~32 % lost; SPI2 (E2) should lift that). Emulator model
      and tests follow. (verify on device: log 1200 / 2400 / 4800 measured == nominal, lost 0; 9600 lost vs the bus build)
      Device c50425f (ad2-isr): ladder right, captured + lost = nominal at 1200 / 2400 / 4800 / 9600, chop on / off.
- [x] (E2) HY3131 on the hardware SPI2 (src/hy3131.rs; feasibility doc 8): SPI2 @ 0x4000_3C00, PC10/11/12 AF6
      (VERIFIED: GD32F450xx DS table 5, UM V2.2), mode 0, /16 = 3.125 MHz, CS PD2 GPIO, the read-period clock as
      one GPIO pulse on PC10 between SPI bytes (exact datasheet frame). It is THE bus (no runtime switch); the old
      bit-bang stays only as the fallback build feature `hyspi-bitbang`. DBG:HYSPI:SELF? n[,div]: constant-row /
      write-pattern / AD2-sign check + us per frame (div 8 / 16 / 32 for one call). Emulator: HY3131SpiPort on
      spi3; AD2 logger loss at 2400 / 9600 S/s 16 % / 79 % (BB) -> 1.5 % / 8 % (SPI2), plumbing only.
- [x] (E2) device check (10 min, feasibility doc 8.4): SPI2 build, SELF? at 16 / 32 / 8, readings vs the
      bit-bang fallback build, AD2 logger loss; on failure flash the `hyspi-bitbang` build. Then drop the
      fallback feature and the bit-bang code.
      0d0fd76 on the device FAILED (doc 8.5): DC V wrong, w1 write/read-back ~25 % bad at /16 and /32, reads
      fine -> frame-boundary timing (CS hold / idle vs the HY3131's 4.9 MHz SYSCLK), not the bit clock.
      DIAG image 6d35c53 on the device (doc 8.6): SPI2-written data bytes with bit 7 = 1 and bit 0 = 0 arrive
      with bit 0 = 1 (64 / 256 values, every divider / CS timing / pin speed); SPI2 reads clean at /8 /16 /32.
      Fixed: reads on SPI2 /16, writes bit-banged (pins switched per write frame); DIAG / RAW removed.
- [x] (E2) device check of the read-SPI2 / write-bit-bang image d150919: PASS 2026-10-22. DC V 0.99979..0.99990 V
      (1 V cal), SELF? 2000 at /16 and /32: all error counts 0, 23 stable bytes, 7.65 us 1-byte / 21.4 us AD1+AD2
      read; AD2 logger 9600 S/s -> 8534 S/s measured, 11.1 % lost, 0 bad, max gap 2.05 ms (bit-bang ceiling was
      ~6.5 kS/s); 4800 S/s -> 8.0 % lost (main-loop gaps: the ISR decision).
- [x] Drop the `hyspi-bitbang` feature (bit-banged reads) now that d150919 passed: done (bit-banged WRITES stay,
      DBG:HYSPI:SELF? stays).
- [x] Pure-SPI2 writes (feasibility 11.6 / 11.7): device b48048e DBG:HYSPI:WR?: the chip takes each byte's LSB
      late (the next MOSI level); 42cbf35 writes one frame per register with a CS-cut dummy whose MSB = the LSB:
      0 bad in 8 x 512 writes at /8, /16, /32, DC V 0.9997 V on 1 V, SELF? w1 9.9 us (bit-bang 57 us). The
      bit-banged write is removed.
- [x] (E3/E4) AD2 stream on ONE timer ISR, decision #3 (docs/dmm-input-scope-feasibility.md 9): Cargo feature
      `ad2-isr` (OFF by default until the device acceptance below), TIMER6 = TIM7 IRQ 55 the only NVIC line
      (shim.rs table to IRQ 55, everything else ICER'd + SysTick / PendSV cleared at the hand-off, PRIMASK
      cleared only while streaming), exclusive HY3131 bus hand-off (hy3131.rs `stream`, every main-loop
      read / write stops the stream first: DBG:ISR? forced), SPSC ring (logger.rs `RING`, the same 16 KiB,
      full = newest dropped), ISR = INTF + R03..R05 over SPI2 with capped waits, tick 4 x the rate + 7/8 P
      read gate (DEVIATION from "~1.05 x": 1.05 x gave ~P stamp jitter and 5 % false gaps, doc 9.2), default
      LOG:SRATe 4800 with the feature. DBG:ISR?. Stack check counts the ISR (+284 B; stackdepth.py also
      sees IT-block `bl` now: main path +336 B, headroom 17456). Emulator (200 MIPS, DRAW 2): 4800 / 9600
      / 1200 S/s lost 0, no stamp gap (main-loop sampler: 23 % lost at 4800 DRAW 2);
      emu/hy3131/ad2streamtest.py PASS.
- [x] (E3/E4) device acceptance of the `ad2-isr` image. c50425f FAILED (1.4 % / 2.6 % lost in bursts: the INTF
      read-clears race at a +63 ppm tick; max_lat ~20 us). Fixed in b48048e (feasibility 11): the ISR is
      phase-locked to the conversion grid (src/ad2pll.rs: blind R03..R05 read half way between edges, INTF
      probe every 4th period), hot code / data below 0x08080000 (link-native.x region B / .cold; the fonts had
      grown into the non-zero-wait flash). Device b48048e, 60 s each: lost 0 of 288 197 at 4800 DRAW 2, 0 of
      576 092 at 9600, 0 of 1 153 647 at 19200 (/16, DRAW 2, CPU 35.6 %); max_lat 4.7 us (was 21.8); chip
      crystal -1 ppm; *IDN? max 0.73 ms; READ? / SELF? clean after. /8 reads NOT clean (61 / 100 sign errors in
      1.15 M / 2.36 M): stream stays /16. `ad2-isr` is DEFAULT since 42cbf35.
- [x] (E3/E4) device check of 42cbf35 (feasibility 11.9): critical-section without masking -> max_lat 0.24 us;
      CPU /16 4800 DRAW 2 / 9600 / 19200 DRAW 2 lost 0; DMA frames 9600 lost 0 at 5 % CPU; /8 + CKPH = 1 data phase
      clean (SELF?, 19200 lost 0, 38400 35.8 M samples 0 bad at 14 % CPU) -> the stream default (CPU frames).
      DMA at 38400 lost the lock (CPU probe frames overran): probes as DMA frames since 7ed5b31.
- [x] Device 0471860 (feasibility 11.10): DMA /16 38400 lost 0 of 2.3 M at 17.5 % CPU, 19200 DRAW 2 lost 0 at
      7.8 %; CPU /8 + CKPH 38400 lost 0 at 51 %, 4800 DRAW 2 / 9600 lost 0. Host: a raw drain tops out at ~37 kS/s.
- [x] Decide: DMA frames as the stream default; the wave view gains the most.
      decided: see ../docs/history/decisions-2026-10-22.md #7 (DMA frames default, gated on one device run with the
      default flipped: 60 s runs, a 15-min 38.4 k sharing matrix with gen DAC + BT, the wave bands)
      done a239aa6 (`set_dma_req` default 1; DBG:AD2:DMA 0 = CPU frames). Device 11.11: 60 s 4800 DRAW 2 /
      9600 / 19200 DRAW 2 / 38400 lost 0 at 1.9 / 3.9 / 7.8 / 17.5 % CPU; 900 s 38400 with the generator
      DAC at 10 MS/s lost 0 of 34.56 M at 17.6 %; Wave view 5 ms..1 s/div bad 0, the same measurements as
      CPU frames, first sample 14..51 ms after a band change (CPU 10..22 ms); DC V / SELF? clean after.
      Emulator ad2streamtest +-100 ppm (default, no :dma) + wavetest PASS.
- [x] Device: decision #7 sharing matrix, BT part: 38400 for 15 min with BT connected (phone or host SPP),
      lost / bad 0, BT answers after the stream. Not run on a239aa6: the host got "Host is down" from the
      meter's SPP address for the whole session (11.11). Cause: the next item.
      Done on 8b9cf79 (docs/device-check-2026-10-21.md "Bluetooth (8b9cf79)"): 900 s 38400 DMA frames with a host btpoll (50 ms) linked the whole time:
      34 560 269 records, lost 0, bad 0, seq / stamp gaps 0, max gap 27 us, CPU 17.5 %; BT cmd 09 replies
      during the stream 0 (by design, UF-36), BT answered right after.
- [x] BT module power (PC0): native opendm40 (since cdd2fc3) left PC0 high from board::init, the stock boot
      level = module off, and never lowered it (the stock does at its BT UART init); the RAM-loaded builds
      had inherited a powered module from the stock app. bt.rs `Bt::power` now follows the Bluetooth setting
      (low = on, high = off), waits 1 s after power-on before the AT init, pulses PC0 high
      100 ms after a successful cmd 07 (stock); the status-bar BT glyph needs Bluetooth ON (PC14 is
      meaningless with the module off). Emulator BtModule models PC0 (on only while driven low; a host
      connect waits for power, else refused): powtest bt FAIL on the old image, OK fixed; modetest fwrs dcv/acv OK; stock dcv
      OK (the stock's PC0 handling passes the model). (verify on device: BT glyph off with no host; the
      classic XX:XX:XX:XX:XX:XX answers `dm40scpi.py --bt info`; then the pending BT checks: doc 3.13 power
      meter list, decision #7 BT part, phone app (BLE "DM40A"))
      Device 8b9cf79 (docs/device-check-2026-10-21.md "Bluetooth (8b9cf79)"): glyph off with no host, on while linked, off after; --bt info OK;
      reconnect 0.7-1.3 s; Settings Bluetooth OFF -> not discoverable, connect times out; ON -> discoverable,
      answers. Phone app (BLE) not run.
- [x] BT link opened while the DMM rail is off (Scope / Gen page) does not wake the DMM: cmd 09, BT FETC? /
      READ? and USB READ? return the reading from before the rail went off and ignore a 1 -> 2 V step (3/3).
      After >= 3 such connect / disconnect cycles the DMM stays frozen even back on the DMM page (> 9 min,
      PROF d.read 0, "AC?" hint; 2/2), until `DBG:HYSPI:SELF?` (its `app.dmm.start`). A link held while
      leaving the DMM keeps it live. Suspect: `Dmm::start` from `set_suspended(false)` right after PE2 goes on.
      Repro: docs/device-check-2026-10-21.md "Bluetooth (8b9cf79)" §1.
      Cause (emulator, PE2 now power-gates the HY3131 model): a wake that leaves PC4 on (BT link / host query
      on Scope / Gen) wrote the row in the same pass as the PE2 rise (the page switch has the 10 ms
      make-before-break pause, the wake had none): the chip powered up with its POR row, nothing ever
      rewrote it; with the rail then kept on (host_dmm after a READ?, the link) the DMM page did not restart
      it either. Fix (dmm.rs): the row goes out RAIL_SETTLE_MS = 50 ms after any rail rise (`wake_at`,
      the deadline moves at once so READ? waits for the fresh reading); a stall watchdog (no reading for 2 s
      past the deadline -> R2A..R31 read back, all 0x00 / 0xFF = the row is lost -> restart; no reading for
      max(10 s, 8 x period), not in capacitance -> restart). Counters in SYST:RAIL:STATe? (wakes, lost,
      silent). emu/hy3131/btwaketest.py: 3bae462 fails 1g / 1h / a chip POR, the fix passes. The device 1h
      (no READ?, the rail toggling, the page switch's 10 ms pause) is not fully explained by the model: the
      counters on the device re-check discriminate (wakes per link cycle; lost / silent).
      Device e656df3 (device-check-2026-10-21.md "DMM rail wake and fast logger"): 1g / 1h PASS, wakes +1 per
      link (the rail follows PC14), lost / silent 0 everywhere incl. 35 s idle in Hz / temp / ohm / power /
      diode / cap. AC V: the first reading after a wake was +8 % -> e6980ac: 2 s settle after a wake in the AC
      functions, and leaving the Wave view no longer drops the DMM rail (wavetest "DMM resumed" regression).
      Device e6980ac: AC V wake READ? -0.8 % (first answer after 2 s), Wave view MENU -> READ? correct with no
      rail drop, 1g / 1h PASS again.
- [ ] DMM reads at /8 + CKPH = 1 too (2.6x faster bus); needs its own device check of every function.
      decided: see ../docs/history/decisions-2026-10-22.md #8 (DMM stays at /16 mode 0; acceptance check defined for the
      revisit with decision #3's request queue)
- [x] B (zero-wait flash) is FULL: the branch overflowed by 4 bytes at 7ed5b31; the cold modules' named
      .rodata moved to `.cold` (28 bytes left), the bit-banged write removed. A real reclaim is needed; never put
      per-pass code in `.cold` (feasibility 11.4 rule).
      Region B audit #3 2026-10-22 (on 1ad38aa: 11.9 KB free): 26.2 KB free (ad2-isr; 32.8 KB without), no
      change in function but the Wave view items below (docs/native-boot.md "Region B audit #3"):
      App::tap / key / show / wave_open, rec_start, Logger::start, page / dialog / menu / keypad / generator
      page draws, DBG / calibration replies, start-up constructors, vwrite's post-write steps -> `.cold`; the
      CSV generators of the volume -> B. Fonts stay in B (anonymous rodata read per draw while streaming; the
      emulator has no wait-state model to measure it). Region C now has 10.3 KB left (was 25.3 KB): the next
      reclaim needs room there.
      Region A2 2026-10-22 (docs/native-boot.md "Region A2"): the zero-wait flash after the shim, below the
      loader marker (0x0800460c..0x08008000, 14.8 KB, in the update body) holds the ASCII26 + F14 font blobs
      (tools/mkstockfont.py A2_FONTS, `.rodata.a2.*`; 2.65 KB left, build.sh warns below 2 KiB, the link fails
      at the marker); App::tap / key / show and vdisk::describe back from C to B. B 26.1 -> 30.6 KB free
      (33.3 -> 37.8 without ad2-isr), C 10.1 -> 17.8 KB (12.2 -> 19.9).
- [x] Device check of region A2 (device 92fb8fa: marker VALID = the image header, INFO/FIRMWARE.BIN = the
      installed file byte for byte, logger panel text correct, max_lat 0.28 / 0.30 us at AD2 38400 with the
      panel, 0 lost; the inrush taps while armed: device 2f56ff9, threshold +/- taps and Cancel while ARMED work, late 0 / dropped 0, max_lat 14-16 us while armed, not chased): install with reload.sh (the marker valid after the install, FIRMWARE.BIN on
      the volume = the installed file), then the logger panel / DMM view / Wave view text (ASCII26 / F14 from
      A2) drawn correctly and DBG:ISR? max_lat at AD2 38400 with the logger panel no worse than before; taps on
      the inrush page while armed (App::tap now in B).
- [x] Region C audit #1 2026-10-22 (docs/native-boot.md "Region C audit #1"): on 5af9a5c region C 17.4 ->
      23.8 KB free (from 9.9 KB on 36b6aae: +13.9 KB with upstream's A2 move). Shrunk 2.7 KB: three statics
      .data -> .bss (gallery G, scope_csv SNAP, csvlog JOB), App::with_temp_page without a closure parameter
      (4 copies -> 1), gallery Tb::c out of line; moved: the .data load image (1 KB) and ad2exp + dbg_dcv
      (5.2 KB, DBG:AD2:*) to region B (B 78.2 -> 72.0 KB free). No core::fmt in the image, no duplicated
      decoder in the gallery (single-copy dense code). 23 of 24 dead-code warnings fixed (0 bytes: LTO).
      New emu/gallerytest.py (open / swipe pages / viewer = BMP bytes / Delete Cancel + Delete / back).
      Left as candidates there (more DBG-only code to B ~6.8 KB, gating the AD2 experiments 2.6 KB, one
      shared fat_get ~0.2 KB).
- [x] Region C audit #2 2026-10-25 ([docs/native-boot.md](docs/native-boot.md) "Region C audit #2"): on 058a1d36
      region C 6 016 -> 8 632 B free (the build.sh warning gone), B 18 820 -> 17 444 B, A2 4 188 -> 2 948 B. The
      SC_PROBE font blob B -> A2; the SCPI range / resolution commands and ui::help_page C -> B. No code changed.
- [x] Region C audit #3 2026-10-29 ([docs/native-boot.md](docs/native-boot.md#region-c-audit-3-2026-10-29-choreregion-c-headroom)): on 3b142426
      (mute) region C 7 720 -> 13 352 B free, B 50 572 -> 44 924 B: ad2exp, dbg_dcv / dbg_n7 and
      scpi::dcv_arg / n7_arg (DBG:AD2:*, E7's dc_enter) C -> B. No code changed.
- [x] Device check of region C audit #2 (2 min): the scope page's probe labels (SC_PROBE, now in A2) drawn as
      before; `CONF:VOLT:DC 60`, `VOLT:RANG 6`, `RANG?`, `RES:RANG:AUTO ON` answer as before; Settings -> Help
      pages open.
      **Device 0.3.0 mute (2026-10-27): PASS.** The probe glyph (DC x1, AC x1) equals
      [scope.png](../docs/manual/img/scope.png) / [scope-ch1.png](../docs/manual/img/scope-ch1.png) at RGB565.
      `VOLT:RANG?` 60 -> 6, `RES:RANG:AUTO?` 1, no error; a bare `RANG?` is -113 (the syntax needs `<f>:`).
      Help 1/7 .. 7/7 open, the tap wraps to 1/7; 1/7 equals [help.png](../docs/manual/img/help.png).
- [x] Device check of region C audit #1: boot and settings restore (the .data image now in B), the gallery
      (open, page swipe, view, delete), a CSV recording start / stop and a scope Save CSV (csvlog JOB,
      scope_csv SNAP now zero-initialised), the temperature page keys, DBG:AD2:NOISE? / DCV? still answer.
      **Device 0.3.0 mute (2026-10-27): PASS.** `DBG:RESET` with AC V, AutoHold on, 10 ms/div: all three back.
      Gallery: page 2 and back, the viewer equals the file (a fresh screenshot), Delete -> dialog -> Delete removes
      it. `LOG:CSV ON` / `OFF`: LOG022 (45 rows, header right); scope STOP + Save CSV: SCOP011, header and rows right.
      Temperature page: sensor keys and offset +/- work. `DBG:AD2:NOISE?` (AC V) and `DBG:AD2:DCV?` (DC V) answer.
      The power-key off / on restore was not repeated (device 705d2e9 above).
- [x] Font storage 2026-10-22 (docs/fonts.md): the stock fonts were 104 KB of data in region B (90.9 KB
      bitmaps + 13.2 KB tables). CJK / full-width glyphs dropped (232, never drawn), 4 bpp glyphs as row-major
      nibble runs (RLE4, decoded straight into the LCD writes), 1 bpp glyphs cropped to their ink rows,
      10-byte entries, named sections `.rodata.fonts.*` (region B; ASCII26 / F14 stay in A2): 41.1 KB. On af4be54:
      region B free 26.4 -> 82.2 KB, A2 free 2.7 -> 7.9 KB (+61.1 KB together).
      Pixel-identical (tools/font-matrix 178k glyph cases, ui-render per-case screen hashes UIR_HASH, stock-diff);
      emulator draw time down (DMM page 17.1 -> 15.7 ms, power page 11.3 -> 10.5 ms, d.read avg 646 -> 484 us).
      Lossy / vector-font options evaluated (doc sec. 4): <= 6 KB more for a visible change, not done.
- [x] Device check of the font storage (device 92fb8fa, device-check doc "92fb8fa" 1-2: menu, Settings
      1-3, Hold & Limits, keypad, Record, Power Settings pixel-identical to the 33b44bd device shots, Help
      1-7 pixel-identical to ui-render, every other page looked at: no glyph damage; tap -> draw loop max1s
      18-29 ms vs 23-51 ms on 33b44bd; DMM page full draw 21.0 ms, power page 14.2 ms): DBG:PROF? d.read / d.chart and a page switch's draw time vs the
      previous image; DBG:ISR? max_lat with the logger panel drawing while AD2 streams (unchanged expected).
- [x] Device check of audit #3: DBG:PROF? lat_pc / DBG:ISR? max_lat at AD2 38400 to disk with the volume
      mounted and read; Wave view: STOP + Save CSV and a screenshot (max_lat, overruns 0, the trace and the
      stream back after), a host copying SCOPnnn.CSV / LOGnnn.CSV during the Wave view; taps on the inrush
      page while armed; tap latency on the range / power settings / keypad / menu / generator pages.
      Device 33b44bd (device-check doc "33b44bd"): AD2 38400 to disk + volume reader 0 lost, max_lat 0.28 us
      (not C); Wave view 4800 / 38400 STOP + Save CSV, screenshot: overruns 0, max_lat <= 0.3 us, stream back,
      SCOPnnn.CSV after leaving; tap latency 23..51 ms (loop max1s) on every page. Found: a host copying CSVs
      during the Wave view ran `dlog::FPar::parse` in C (21.4 us, fixed: region B); DBG:PROF? is in C (12 us,
      1 sample lost at 38400: debug only, open); App::show at the Wave view exit (11..18 us, 1 overrun or
      2 late, open). Left: inrush taps while armed; re-check of the FPar fix.
      Device 92fb8fa: the FPar fix holds: a host copying 17 CSVs (12 MB, 95 KB/s) during the Wave view at
      38400: max_lat 0.30 us, overruns 0 (was 21.4 us); `dropped` +831 once, near the end of the copy (the
      view's ring, not the ISR). Exit: 4.0 / 5.8 us, late 1 (the open item below).
      Inrush taps while armed: device 2f56ff9 (the region A2 item above). Closed 2026-10-27.
      - [x] DBG:PROF? (`prof::report`, C) during a fast session loses samples: region B (~1 KB) or a note.
            **Fixed (dda802c):** prof::report is region B. Emulator: emu/fastlogtest.py --region-c --host-read
            (DBG:PROF? between the LOG:DISK? polls of every session) and wavetest's host polling at 2 / 1 ms/div:
            no region C call (the old build: prof::report in both). Device c92b605: PASS (38400, 39 polls,
            280 ns, late 0, lost 0).
      - [x] Wave view exit: App::key / App::show (C) run before the stream stops (1 overrun at 38400).
            **Fixed (dda802c):** App::key / show are region B since region A2, but the exit still ran the
            menu's pressed feedback (`App::press_track`, app_press.rs, C) with the stream on (the emulator's
            region C hooks, MENU at 2 ms/div). Now `show` leaving the live Wave view only asks for the stop
            (WaveReq::Stop) and holds the view change; main.rs shows it right after the stream stopped
            (`App::wave_exit_done`, before the DMM row is restored). wavetest: MENU exit at 38400, no region C
            call, late / overruns / timeouts unchanged. Device c92b605: PASS (2 ms/div x3: 300 ns, late 0, overruns 0;
            1 ms/div: late / overruns unchanged; the menu, then the DMM).
      - [x] SCOP006.CSV once saved the previous 4800 S/s record under a 2 ms/div header. Reproduced in the emulator:
            STOP at 20 ms/div, 2 ms/div while stopped (the stopped zoom), Save CSV: the stored record (4800 S/s, its
            rate right) under the timebase shown. Fixed: the header's timebase is the record's (scope.rs
            `record_tb`); emu/hy3131/wavetest.py `--case csvtb`.
- [x] 76.8 kS/s (emulator; device check below): hybrid frames (feasibility 11.12, decision 2026-10-22 #9): the CPU
      sends the command byte (mode 0, /8) and switches CKPH = 1 after the read-period pulse, DMA reads R03..R05;
      probe frames 2-byte DMA at /8 mode 0 (a marginal bit only blurs a probe), every 16th period; the probe
      frame starts before the sample is stored; Pll geometry `Pll::hybrid` (beta 8.6 us, A ~1.2 us after an
      edge on the device). Ladder rung 76800 (LOG:SRATe; DMA frames only: CPU frames / a forced /16 cap it),
      Wave view 1 ms/div = 76800 and 2 ms/div = 38400, disk only after a page-program probe at the end of Prep
      (mean <= 800 us, max <= 1.5 ms, else the session runs at 38400; LOG:DISK? +fifo_hw,programs,tpp,tpp_max).
      Emulator made honest first (SPI byte time + DMA in time on both SPI buses, CPU-synced virtual time,
      TornAd2, the /8 mode-0 DO margin, CKPH 1): the old firmware passes every test under it.
      Emulator: 76800 at -100 / +100 ppm, also with 5 % probe bit errors: lost / late / slips / overruns /
      timeouts / bad / torn 0, locked to the chip within 0.1 ppm; ISR CPU 29.5 % in the emulator, **~43 %
      estimated on the device** (40..46 %); DRAW 2 (full DMM repaint) overflows the 26.7 ms ring (7.8-9.3 %
      overwritten, lost 0); Wave view 1 ms/div: no overflow; disk typ flash keeps up to 196 KB/s (767 pages/s,
      FIFO high water 460 B), max flash (tPP 3 ms) lost 28-54 % ungated -> the probe gate; gated (1ad38aa): typ
      probe 400 / 400 us -> 76800, 153 601 rows lost 0; max probe 3000 / 3000 us -> 38400, lost 0, FIFO 468 B
      (`emu/fastlogtest.py --app <elf> --sessions ad2:38400,ad2:76800`, ~15 min wall with the max profile).
      Forced hybrid (`DBG:AD2:DIV 8,1`, ad2streamtest session field 5) at 4800 / 38400: lost 0, div 1008.
      Device check (image and steps: feasibility 11.12 / the hand-over):
      1. `scpi "LOG:SOURce AD2" "LOG:SRATe 76800"`, AC V on the 40 Hz square, `log a768.csv --source ad2
         --rate 76800 -d 60` (raw drain): DBG:ISR? div 1008, dma 1, lost 0 (LOG:COUNt?), late / slips /
         overruns / timeouts / bad 0, locked, cpu_permille (estimate ~430), max_isr, max_lat < 1 us;
         overwritten = what the host could not drain (note the drain rate).
      2. The same with `LOG:CHOP OFF` (the stock rows' unchopped OSR 32) and with CHOP ON (the default:
         ENCHOPAD2 at OSR 32 never ran on the chip): codes within the square's +-107 k, no stuck samples.
      3. `DBG:AD2:DIV 8,1` at 4800 / 38400 (hybrid frames at a slow rung): lost 0, bad 0.
      4. Disk: LOG:DEST DISK, LOG:DUR 60 at 76800: LOG:DISK? fields 9 / 10 (the chip's tPP: expect ~0.4..0.7
         ms), the rung LOG:PREamble? nominal, lost_before sum 0, fifo_hw (of 8192), pages/s = programs / 60;
         with a noisy input (the calibrator's sine + the generator's noise, or V/COM open) once more.
      5. Wave view 1 ms/div and 2 ms/div on the square: the trace, Vpp, no "Wave stopped", DBG:ISR? lost 0,
         overwritten (ring) 0 for 30 s; 5 ms/div and slower as before.
      6. After the streams: DC V 1 V within 0.05 %, `DBG:HYSPI:SELF? 2000` all 0.
      Device 20a91d0 (2026-10-23, docs/device-check-2026-10-21.md "20a91d0" 2): 1-3, 5, 6 PASS (div 1008, dma 1,
      lost / late / slips / overruns / timeouts / bad 0, locked, max_lat 300 ns; codes in range CHOP ON / OFF;
      DIV 8,1 at 4800 / 38400 lost 0; Wave view 1 / 2 ms/div overwritten 0 in 30 s; DC V 1 V -0.005 %, SELF? 0).
      **4 FAIL**: disk at 76800 kept 2 095 713 of 4 599 040 conversions (lost_before 2 505 919), see below.
- [x] 76800 to the disk loses 54 % (device 20a91d0): rung 76800 chosen (tpp 382 / 389 us), fifo_hw 525 B,
      336 pages/s, a hole of ~27-38 conversions after every 32 rows: one BATCH per Run pass, RUN_BUDGET_CYC
      (300 us of cycles, the ISR's share counted) used up by it with the ISR at cpu_permille 544 (estimate
      ~430, emulator 295). Main loop 1272 passes/s, `usb` (Run) avg 718 us. The host drain also tops out
      at ~35 k rows/s (not the CPU: the device's main loop idles at 11 k passes/s there, the host's
      LOG:DATA? round trips are the limit). 38400 to the disk: lost 0.
      **Fixed in the emulator (branch fastlog-768-drain, 2026-10-23; docs/compact-log-format.md "The CPU");
      device check pending.** Emulator made honest first: emu/peripherals/CpuCost.cs, `--mips 110`
      (the device's idle pass 22 vs 23 us and its 76.8 kS/s Run pass ~650 vs 718 us) + `--isr-stall-ns 700`
      (cpu_permille ~520 vs 544): the old firmware loses 37-46 % at 76800 there. PC samples of the main
      code at the ISR entries showed the coder at ~10 us / row (64-bit shifts, a CRC call per byte, 12-byte
      records, the log panel's 64-bit divisions every pass) and the pages at ~0.25 ms each. Now: the Run
      budget is the pass's own CPU (ISR time out; 2x while the ring is over half full; 2 ms wall cap), the
      coder ~2.5 us / row (byte-identical stream), pages back to back (~0.17 ms), and the writer trial
      (fastlog `Gate`): the first 1 s coded with dry pages, then the file starts in the same logger session
      (kept) or the session restarts at 38400 (the ring's low water rising or a drop)
      (LOG:SRATe? 38400, LOG:DISK? fields 11..13 gate / ISR permille / ring high water). Emulator: 0 lost at
      76800 (1.2-2.45 B/sample, ISR 52-62 %), trial kept; a slower main loop (`--mips 80` / `70`) -> the trial
      falls back, 38400, 0 lost.
      Device retest (image and list in the hand-over):
      1. AC V, the 40 Hz square, LOG:SOUR AD2, LOG:SRAT 76800, LOG:DEST DISK, LOG:DUR 60, LOG:STAT ON:
         LOG:DISK? at the end: fields 11..13 (gate 1 expected: kept; ISR permille ~540; high water: the
         emulator 80-1840 of 2048, the log panel's refresh rows at 5-10 ms per pass; >= ~1900 on the
         device: split the panel's row draw or hold the refresh while the ring is over half full);
         LOG:PRE? rung 76800, LOG:SRATe? 76800; dlog.py of the file: lost_before sum 0, rows ~4.6 M;
         DBG:ISR? late / bad / overruns / slips 0, max_lat < 1 us; LOG:COUNt? overwritten 0; pages/s.
      2. `DBG:PROF:RESET` at state 3, `DBG:PROF?` after a 20 s session: passes/s, `usb` avg (was 718 us).
      3. The same with a noisy input (the calibrator's sine + the generator's noise, or V/COM open): lost 0,
         or the trial fell back (gate 2, LOG:SRATe? 38400) and lost 0 at 38400.
      4. A stop (LOG:STAT OFF) within the first second after state 3 (the trial): the session ends, no
         FASTnnn left (MMEM:CAT?), the next session works.
      5. 38400 and 4800 to the disk (no trial: gate 0): lost 0 as before; AD1 640 to the disk: lost 0.
      5a. DC V via AD2 at 76800 to the disk (LOG:SOUR AD2 in DC V): lost 0; after a fallback (gate 2) the
         restart runs the AD2 self-cal (`dc_prepare`) again: the 38400 file starts later.
      5b. A 76800 session started 11-21 s after power-on (CYCCNT's upper half; the emulator's bug of
         130d2e8's parent: the trial saw a busy chip there and fell back): gate 1.
      6. The Wave view 1 / 2 ms/div and a host LOG:DATA? session at 76800 as in 20a91d0 2.1 / 2.5 (unchanged
         paths; the ring drain for the disk only is new).
      7. The record page's fast-logger start (tap) at 76800, the inrush save: as before.
      **Device 825080c (device-check doc "825080c"): fixed.** 76800 AC V 40 Hz square: gate 1 (kept),
      4 331 399 rows, lost_before 0 (ended Full at 56.6 s: the 5.9 MB free disk), ring high water 1017-1170 of
      2048 in every kept 76800 session (never near ~1900), ISR permille 546 / 548, 1.478 entries per sample,
      436 pages/s; DBG:PROF? 8987 passes/s, `usb` avg 83 us (was 1272 / 718). Stop in the trial, 38400 /
      4800 / AD1 640, DC V via AD2, a start at 11.7 / 16.4 s uptime, the Record page's Fast start, the host
      drain (40.5 k rows/s, was 35 k) and the Wave view: no sample lost (their max_lat / overruns:
      range_cmd, next item). Not done: a noisy input (no sine / noise
      source without the generator; the 100 Hz square instead: 0 lost), a DC V fallback (gate 2) restart,
      the Record stop by a finger (DBG:TAP does not reach `fast_touch`), the inrush save (no A-COM load).
- [x] SCPI range_cmd in region C ran for every SCPI command (379e49c; found on device 825080c): max_lat
      1.6-20 us at range_cmd's entry with late / overruns 1-3 / slips 1 during 76800 streams (host drain,
      Wave view, disk polling), and a 20 Hz LOG:DISK? poll made one writer trial fall back to 38400 (ring
      2048). Fixed in decaf60 (branch fastlog-768-retest): the dispatcher calls it only for RANGe /
      RESolution headers. Device check: as in the device-check doc's "Re-check on decaf60" (max_lat < 1 us,
      late / overruns / slips 0 at 76800 with the polls, the range commands still answer).
      Device decaf60 (2026-10-23, device-check doc "decaf60" A): fixed. 76800 AC V 40 Hz square to the disk
      (60 s, ended Full at 56.5 s): 4 329 787 rows, lost_before 0, max_lat 320 ns (spiflash::xfer), late /
      overruns / slips 0; lat.py 76800 10 poll x3 (327-335 LOG:DISK? polls): gate 1, max_lat 320 ns
      (spiflash::xfer), 0 / 0 / 0 each; p2.py host drain 30 s: 38.3 k rows/s, max_lat 280 ns, seq gaps 0;
      p5.py Wave view 1 / 2 ms/div: max_lat 280 / 260 ns, dropped +0, 0 / 0 / 0. The range commands answer
      (VOLT:RANG? / 60 / SENS:VOLT:RANG:AUTO ON / VOLT:RES? / CURR:DC:RANG? = +9.91E37 + "Settings conflict"
      while in V, as documented).
- [x] 76.8 kS/s to the host in real time: `LOG:FORMat ROWS|PACKed|DLG` (branch feat/log-format; logger.rs
      "Compact LOG:DATA?"). PACKed = 3 B per sample on the grid t0 + i / rate, a new block (own seq, t0,
      lost_before) at every sample more than half a period off the grid; DLG = the disk fast stream's kind-3
      chunks (fastenc.rs `FastEnc`) in a 4 KiB FIFO of the reply buffer. Host: `dm40scpi.py log --format`,
      dlog.py `fast_chunk`; tests tools/logfmt-test/run.sh, emu/logfmttest.py (--mips 110 --isr-stall-ns 700,
      30 s of device time per session; the emulator's virtual time runs ~7x slower than the host's, so its
      host-side numbers are not the meter's):
      38400: ROWS / PACKed / DLG all lossless (12.7 / 8.0 / 8.2 B per sample: the host polls faster than the
      ring fills, small replies). 76800: ROWS lost 18 301 (overwritten), PACKed 643, DLG 1 635 with the logger
      panel on; with DBG:AD2:DRAW 0 (no panel) PACKed and DLG lossless (2 304 469 / 2 304 218 rows, 4.0 /
      2.5 B per sample, 1891 / 1412 replies/s of device time). So in the emulator the logger panel's draws
      stall the main loop longer than the ring's 26 ms at 76.8 kS/s (max LOG:DATA? round trip 343-419 ms of
      host time with the panel, 43-66 ms without); not measured on the device (its disk sessions at 76800,
      same panel: ring high water 1017-1170 of 2048, device 825080c). Over Bluetooth (emulator): AD1 640 S/s
      lossless in all three formats, AD2 still refused.
      Reply size kept at 6144 (usbtmc::RSP_MAX): the device's drain ran at ~79 replies/s with 511 ROWS records
      (40.5 k rows/s, 825080c); a PACKed reply holds 2034 samples (6138 B), a DLG one ~2-4 k (4 KiB of chunks
      at ~1-2 B per sample), so 76 800 S/s needs ~38 PACKed replies/s or fewer DLG ones: 2x margin if the
      round-trip rate stays the device's (unmeasured with these formats). A larger reply would grow
      usbtmc's `rsp`, Bt::respond_with's 6 KiB stack frame and csvlog::CAT_MAX (main.rs assert) together,
      and `#4nnnn` caps it at 9999 B.
      Device check (untested on the meter): AC V 40 Hz square, LOG:SRAT 76800, `dm40scpi.py log --source ad2
      --rate 76800 --format packed -d 30` and `--format dlg`: lost 0 / overwritten 0 in the summary, the
      replies/s; the first blocks' lost_before (the panel); the same over Bluetooth at AD1 640 S/s.
      **Device 2f56ff9 (device-check doc "2f56ff9" 1, open input instead of the square):** PACKed / DLG
      30 s lost 0, 978 / 802 replies/s (round trip p50 0.9 / 1.0 ms, max 28 ms), host CPU 3.5 / 6.8 s per
      30 s; the only hole is one block of 113-275 samples overwritten right after the first 2048 (t 29-31
      ms, the logger panel's first draw; none with DBG:AD2:DRAW 0). Bluetooth AD1 640 S/s ROWS / PACKed /
      DLG lost 0, 15 / 33 / 31 replies/s.
- [x] 76.8 kS/s host stream: the logger panel's first draw overwrote 113-275 samples once at the start of
      every session (device 2f56ff9). Fixed: a panel part is drawn only while the ring holds < 512 records
      (a quarter of logger::CAP) or nothing drained it for 200 ms (dialog.rs `log_panel_room`). Emulator
      (--mips 110 --isr-stall-ns 700, panel on): emu/logfmttest.py 76800 PACKed / DLG 10 s overwritten 0
      (two runs; 455 in 3 s before), 38400 all formats lossless; emu/fastlogtest.py disk 38400 / 76800 lost 0.
      Device 1a11eda (device-check doc "1a11eda: bug-fix recheck" 4): PACKed 76 800 x 30 s twice, lost 0, overwritten 0; the panel draws
      during a session (screenshot). DLG format not rerun.
- [x] `VOLT:RANG:AUTO?` in AC V and `CURR:RANG:AUTO?` in AC A answered +9.91E37 (device d330877, device-check
      "d330877" new bug 4): a bare VOLT / CURR header meant the DC function. Branch smallbugs: a bare VOLTage /
      CURRent in [SENS:]<q>:RANG / :RANG:AUTO / :RES is the quantity in any coupling (scpi.rs `ANY_CPL`; DC, AC,
      AC+DC, AUTO: one range state per V / A button), in the power meter RANG? / RANG:AUTO? answer its V / I
      channel (as POW:VOLT:RANG?; sets there stay "Settings conflict"). A query never raises "Settings
      conflict" now (coordinator 2026-10-24): another function's / coupling's RANG:AUTO? = 1 (selecting it
      starts in autorange), its RANG? / RES? = +9.91E37 with no error; sets naming it stay "Settings conflict"
      (CONF:VOLT stays DC, 34401A). The decaf60 note above ("CURR:DC:RANG? = +9.91E37 + Settings conflict in V")
      is superseded: +9.91E37, no error. Swept every function x the range / function queries in the emulator:
      no other +9.91E37 outside those. emu/hy3131/autorangetest.py --queries-only (every function and coupling, the power meter, TEMP).
      Device retest: CONF:VOLT:AC / :ACDC / :AUTO and CONF:CURR:AC / :ACDC / :AUTO: VOLT:RANG:AUTO? /
      CURR:RANG:AUTO? = 1 (0 after <q>:RANG:AUTO OFF), VOLT:RANG? / CURR:RANG? the range, SYST:ERR? 0; in
      CONF:POW VOLT:RANG? = POW:VOLT:RANG?'s value.
      **Device c785807: PASS** (doc "c785807" R1; every row SYST:ERR? 0).
- [x] DC V AUTO+ from 600 V or 1000 V (VOLT:RANG:AUTO ON = two steps down to 6 V), 1 V in: one reading of
      0.767-0.790 V (77-79 %) in 5 of 35 tries (600 V: 3 of 20 + 1 of 5, 1000 V: 1 of 10; from 60 V, one
      step: 0 of 10). It is the second reading on 6 V, ~0.8 s after the command; the chart MIN records it
      (device decaf60, device-check doc "decaf60" B4). Not from the averaging window as such (write_config
      resets it on every step). Suspects, unchecked: a conversion that straddles a register write (the OSR
      write in `osr()` after the first 6 V conversion) or the 250 ms autorange settle being shorter than a
      SLOW conversion. Not fixed; needs a raw AD1 code trace around the second step.
      Cause (from dc781.json, branch settle-fix): the 600 / 1000 V row at R22 0x17 -> the 60 V row at 0x15
      restarts AD1; the 6 V row 250 ms later is again 0x15 with ENAD1 kept, so AD1 is NOT restarted and
      its decimation filter still holds 60 V-tap samples. The bad READ? (0.81 s) is the first conversion
      read on 6 V (0.31 s = the 600 V conversion, 0.56 s = the 60 V one that made the second step), and in
      the bad tries the next one came 10 ms later (0.81 / 0.82 s; good tries 0.81 / 0.91 s): the 6 V
      deadline fell on a conversion end and the poll read the older of the two, still mixed. From 60 V
      the row changes R22 (0x17 -> 0x15), AD1 restarts: clean. Fixed (dmm.rs `vet_ad1`, VET_*): the first
      2 conversions on a new range (DC V / mV / A) count only when each agrees with the one before within
      0.5 % (or 4000 codes); others are held (no reading, no step down, not averaged), at most 4.
      Costs ~1 conversion per range step. Emulator: HY3131 model option ad1mix:1 (the filter memory, a
      sinc^3 over 3 periods; model unit test Ad1Mix) and emu/hy3131/autorangetest.py: the base build gives
      0.55-0.85 V readings from 600 / 1000 V (two 1 ms apart, the device signature), the fix none. Device check: dc781.py (600 V x20, 1000 V x10,
      60 V x10): no READ? / CALC:AVER:MIN? below 0.99 V on 6 V; time to the first 6 V reading +0.1-0.3 s.
      Device f6c6e05 (device-check doc "f6c6e05" 1): PASS. No READ? below 0.99 V on 6 V in 40 tries (216 readings,
      lowest 0.99890 V); CALC:AVER:MIN? >= 0.99670 V except 1000 V #0: 0.98396 V, the 1000 V range's own
      conversion at 0.31 s (that range read 0.984-1.021 V, as on decaf60); 600 V's read 0.9967-0.9997 V. First 6 V conversion read: from
      600 V 0.88-0.92 s (decaf60 0.81 s: +0.1 s), from 60 V 0.61 s (0.56-0.60 s), from 1000 V 1.22 s (1.06 s:
      +0.16 s); +0.05-0.16 s, inside the expected +0.1-0.3 s except from 60 V.
      Recheck 2026-10-24 (emulator, autorangetest with ad1mix:1): PASS on ad10749 and with R22_LATE. The
      cover is `vet_ad1` (0ddfacf), not R22_DROP (2bc61aa): the 6 V row goes out at the R22 the 60 V row
      already had, so no R22 changes there.
- [x] AC+DC / AUTO coupling, 1 V DC, a range change by the MF page 6 -> 60 V: the DC part read 0.526 /
      0.528 V (53 %) for one DC phase (~1 s, READ? 0.526 V; in AUTO shown as AUTO,DC 0.528 V) in 2 of 4
      such changes in the first run (one AC+DC, one AUTO), then 0 of 9 in two AC+DC repeats (READ? or
      DBG:ACDC:STAT? polling); 0 of 30 SCPI changes and 0 of 13 MF 60 -> 6 V. Same signature as the 25-51 % partial conversions of the 540e056 same-range item.
      Rare, cause open (device decaf60, device-check doc "decaf60" B5).
      Branch settle-fix: the MF and SCPI paths both call Dmm::set_range; the difference is timing (the
      tap and the range page's redraw delay the DMM poll, so the writes after a result, a phase switch or
      acdc_osr's one R22 = 0x17 in the first DC phases on a new range, land mid-conversion). Not proven.
      Guarded whatever the cause: the AC+DC DC phase vets its first 8 results on a new range (`vet_ad1`,
      through the second DC phase); a result that disagrees with the one before by > 0.5 % is held (the DC
      part stays the previous one, the phase count goes on; a phase with no DC part yet gets one more
      result, and the AC phase does not step down before the DC part is known). Device check: dc5.py MF 6 -> 60 V x10 in
      AC+DC and AUTO with DBG:ACDC:STAT? at 25 Hz: no DC part below 0.99 V.
      **Device f6c6e05: FAIL, reopened** (device-check doc "f6c6e05" 2). The DC part (STAT dc = dc_val, set
      only by a vetted result) read 0.5212 / 0.5224 V and READ? published **0.5244 / 0.5243 / 0.5240 V** as
      the first fresh readings after the NaN gap: 3 of 116 AC+DC changes 6 <-> 60 V, all 3 on 6 -> 60 V (MF
      1 of 44, SCPI 2 of 24; 0 of 48 with the change put 0-0.6 s into a DC phase, dcph.py), 0 of 20 in AUTO. So the SCPI path has it too, and the NaN rule makes the bad value the
      first one a host sees. Signature (both STAT-traced hits): the change lands in a DC phase (mode
      unchanged: no reset_values, so acdc_seen keeps bit 2 and dc_val the old range's part), that DC phase
      ends with no vetted result, one AC phase without readings, then the next DC phase gives one good
      result (0.999 V) and ~0.3 s later the 0.52 V one, accepted, at the switch to AC; the DC part stays
      0.52 V for that AC phase (~1.7 s, 4-5 readings). The vet passes a result that agrees with the
      conversion before it, or any after VET_HELD_MAX holds, so a bad state lasting 2+ conversions or 5
      holds gets through; 0.521-0.528 V in all 5 hits since decaf60 (not the 25-51 % scatter of a single
      straddled write). Not fixed (cause unknown): needs vet_left / vet_held / the raw AD1 codes of the DC
      phase (a DBG ring; LOG:SOUR AD1 reprograms the converter and cannot watch the DMM's own conversions).
      **Cause found and fixed (device-check doc "ad97fd7 / c8c2f02 / a8048ba"):** DBG:ACDC:TRACE (ad97fd7) caught 5
      of 5 hits (40 SCPI changes aimed into DC phases; 4 of 7 at 0.75 s after the AC -> DC switch). The stock's
      once-flag OSR rule (`acdc_osr`) wrote R22 = 0x17 after the first result of the DC phase after the change;
      that result is read at the settle deadline, up to a conversion late, so the write landed mid-conversion:
      the next result came ~155 ms later at 52 % (60 V; 40 % on 6 V), the rule's 0x15 write gave another at the
      same 52 % 100 ms later, the two agreed and `vet_ad1` took it. Fix: no R22 write in the AC+DC DC phase
      (8c086fa). Device: 0 of 110 on c8c2f02 and 0 of 110 on a8048ba (80 SCPI + 30 MF each, aimed 0.55-1.05 s
      into a DC phase, most at 0.75 s).
- [x] AC+DC: READ? during the AC settle after a range change answers the reading from before the change
      (3-6 times, 0.85 s apart, until the first fresh one 3-6 s later), not +9.91E37 as the tmcnl-fix item
      expected ("READ? NaN after 0.85 s, as at entry"; scpi.rs: "the latest one if none comes within
      SCPI_QUICK_MS"). Harmless on a steady input; after an autorange step it held a transient value:
      AC+DC autorange, calibrator 5.8 -> 6.3 V, READ? 3.3885 V for 5 s (device decaf60, B5). Decide: drop the
      latest reading on a range change in AC+DC (NaN until the first fresh one) or keep the doc's rule.
      Decided (owner): NaN. Branch settle-fix: Dmm::reading_pending from any range / function change
      (manual, SCPI, range page, autorange step) to the first reading of the new range; READ? that
      expires then answers +9.91E37 + "Data stale" (main.rs), FETCh? +9.91E37 at once (not under HOLD);
      scpi.rs header. Emulator: autorangetest's AC+DC part (no READ? number before CALC:AVER:COUNt? > 0).
      Device check: tl.py after VOLT:ACDC:RANG 60 / 6 and the MF page: READ? +9.91E37 until the first
      fresh reading, SYST:ERR? "Data stale"; FETCh? +9.91E37 in the gap; libsigrok `-c range=60V
      --samples 5` in AC+DC still gets readings (it skips the NaN).
      Device f6c6e05 (device-check doc "f6c6e05" 3): PASS. nan3.py, SCPI and MF 6 <-> 60 V x2: FETCh?
      +9.91E37 and READ? +9.91E37 + -100,"Data stale" every 0.85 s until the first fresh reading (3.3-5.0 s
      after SCPI, 3.4-4.2 s after the tap); none of the old reading. sigrok range=60V / 6V / 60V --samples
      5: rc 0, 999.7-1000 mV, CONF? follows, /dev/usbtmc2 stays; it leaves -100,"Data stale" in the queue.
- [x] AC+DC method A settle (ACS_STEP 1 %): on the 40 Hz square 600 -> 60 V the AC part came back as
      4.821 / 4.790 V (true 4.768) and READ? 6.783 / 6.762 V (+0.55 / +0.24 %) for 1.7 s: the HPF tail falls
      < 1 % per result and passes the "flat" test, as the TODO feared. 60 -> 600 and 6 -> 60 V were right
      (device decaf60, B5). Options: a stricter step (0.2 %) or a minimum time after the change.
      Branch settle-fix (`acs_flat`, stock/re/docs/history/acdc-stock-vs-opendm40.md 12): flat = fell by <= 0.1 % (ACS_STEP)
      and, with two falls in a row in one AC phase, the rest of the decay d q / (1 - q) <= 0.05 %
      (ACS_REM); ACS_MAX_MS 6 -> 5 s, and the cap also ends the settle in a DC phase with the last AC
      result (`ac_settle_cap`): gap < 6 s. On the device's numbers 4.821 / 4.790 are not flat, the next
      phase's 4.7678 V is (host test meter-test acs_flat_tail_and_steady). Device check: sq5.py (40 Hz
      square, AC+DC, 60 <-> 600, 6 -> 60 V): every fresh READ? within 0.1 % of 6.746 V, gap <= 6 s; the
      1 V DC changes of dc5.py: gap 3-5 s (was 3.0-4.3 s).
      Device f6c6e05 (device-check doc "f6c6e05" 4): PASS. Square 6.7328 V on 60 V (4.759 V AC; the
      calibrator gave 6.746 V on decaf60): every fresh READ? within 0.012 % of the run's level after 600 ->
      60 and 6 -> 60 V, 0.075 % on 600 V; gaps 4.2 / 4.3 / 5.4 s (600 -> 60, 6 -> 60, 60 -> 600). 1 V DC
      (nan3.py, dcmf.py): gap 3.3-5.0 s.
- [x] AC+DC autorange: a DC step of 0.5-0.8 V gives an AC part of up to 3.4 V for ~1 s (the AC path's HPF
      on the step; the calibrator's own step is clean within 0.3 s), hypot > 6 V, one up-range and back
      after the 6 s settle (5.8 -> 5.0 V: on 60 V from 0.25 to 5.98 s). One excursion, no hunting (device
      decaf60, B5). Could hold the up-range while the AC part falls fast.
      Branch settle-fix: the same settle on an input step: a DC part that moved by > 0.5 % of the range
      between two DC results (ACS_DC_STEP), or an AC result > 1.5 x the AC part + 0.5 % of the range
      (ACS_JUMP, a step seen in the AC phase first). Its AC results are discarded and move no range; AC+DC
      keeps reading with the AC part held, and its DC phase steps no range down meanwhile (the
      magnitude would use the stale AC part). Emulator: couptest `i --meth A` (0.3 A DC -> 0.2 A AC,
      AUTO coupling; failed since b3cbfbf: ~10 s with no reading while the DC phase walked mA -> uA ->
      10 A -> mA on the stale AC part) passes again on 28f3029, as the full couptest. Device check: ar6.py / ar6b.py (DC 4..7 V, AC+DC AUTO range):
      no 6 -> 60 -> 6 V excursion on the 5.8 -> 5.0 V or 6.3 -> 5.8 V steps, a step across 6.0 V still
      moves the range once; READ? follows the DC step within ~1 s with the AC part held.
      Device f6c6e05 (device-check doc "f6c6e05" 5): PASS on its checks. 6.3 -> 5.8 V: one range step (ar6,
      ar6b); 5.8 -> 5.0 V: none (ar6b); every step across 6.0 V moved the range once (5.8 -> 6.3, 4.0 -> 6.5,
      6.5 -> 5.0, 6.3 -> 5.8); READ? on the new level at 0.83 s on the same-range steps down (6.3 -> 5.8,
      5.5 -> 4.0, 6.5 -> 5.0; decaf60: 6.737 / 5.679 / 5.957 V there for 2-5 s). The calibrator dips to
      2.2-3.9 V for ~0.2 s on every level change (calstep2.py, DC V FAST; the SLOW calstep.py missed it):
      the 2.3-5.3 V READ?s right after a step are that dip. Residual: next item.
- [x] AC+DC autorange on the calibrator's level change (a ~0.2 s dip to 2-4 V, then the new level): 4.0 ->
      5.8 V took AC results of 2.42 / 2.53 V as the AC part (STAT ac, readings published; ACS_JUMP did
      not hold them) and went 6 -> 60 -> 6 V (1.0 -> 6.4 s); 7.0 -> 6.3 V went 60 -> 6 -> 60 V (the dip's
      DC result stepped down). One excursion each, no hunting (device f6c6e05, device-check doc "f6c6e05" 5).
      Decide whether a dip shorter than one DC phase should move the range.
      **Fixed (device-check doc "ad97fd7 / c8c2f02 / a8048ba" 3):** traced causes: the step's AC results rise
      as the RMS window fills (0.08 -> 1.17 -> 2.38 V) and "rose: flat" ended the settle (now a rise is flat
      only within ACS_STEP); a step ~4.6 s after an autorange step met the settle's 5 s limit, which took the
      jump (now a jump at the limit restarts an input-step settle); a DC result in the dip stepped down (AC+DC
      now steps down only in the DC phase after ACDC_DOWN_MS = 400 ms of results asking for it). Cost: an
      AC+DC step down comes one DC phase later (60 -> 6 V 4.0-5.5 s after a 6.3 -> 4.0 / 5.8 V step on a8048ba,
      was 1.8-3.7 s). Steps up unchanged. Device a8048ba: 27 steps, no excursion.

- [x] Fast logger cosmetics (device 825080c): LOG:DISK? keeps naming the file of a session stopped in its
      trial (the file is removed); its tpp fields keep the last probe's 381 / 387 in a 38400 / 4800 / AD1
      session (documented as 0 = none); the Record page's Fast estimate says "0 min left" under a minute
      (76800 on 5.9 MB = ~25 s at its 3 B/sample; the real 1.36 B/sample gave 56 s).
      **Fixed (dda802c):** the trial's unlink clears the file (LOG:DISK? file -1); `fastlog::begin` zeroes the
      probe's numbers (a session without a probe reports 0 / 0); the Record page says "25 s left" under 2 min
      (seconds; still the 3 B/sample worst case, so the real time is longer). fastlogtest checks the first
      two (--stop-in-trial: file -1; tpp 0 / 0 in every session below 76800). Device c92b605: PASS (file -1;
      tpp 0,0 in 4800 / AD1 / 38400 after 76800; "76800/s, 57 s left" in AC V).
- [x] 4800 disk session on device 825080c: max_lat 4.66 us at calbackup::FwFile::fill (region C: the info
      volume's FIRMWARE.BIN read by a host during the session). No sample lost at
      4800; at 76800 a host reading INFO/FIRMWARE.BIN would likely stall the ISR as range_cmd did.
      **Fixed (dda802c):** FwFile::fill / plain are `.text.vdisk_rd` (region B, with the rest of the volume's
      read path). Emulator: fastlogtest --region-c --host-read reads every INFO/ file (FIRMWARE.BIN first)
      during 4800 / 38400 / 76800 sessions: no region C call (the old build: FwFile::fill). Device retest:
      copy INFO/FIRMWARE.BIN during a 76800 disk session, DBG:ISR? max_lat / late.
      **Device c92b605 (device-check-2026-10-21.md "c92b605"): FAIL.** The code is fine; the DATA is the problem:
      any INFO/ file read from internal flash >= 0x08080000 (the FIRMWARE.BIN tail from offset ~0x7C000, LOADER,
      BOOTDATA, CALIB) stalls the AD2 ISR 21.8 us per read (lat_pc memcpy / vdisk::mem_read). At 38400: late
      ~3.7 k, 10 k samples lost; at 76800: 289 k lost. FIRMWARE.BIN 0..0x7C000 and ROMBOOT.BIN: 280 ns. The
      emulator's region C hook sees fetches only.
- [x] Region C DATA reads during a stream (above). **Fixed (a60525d, f0d0e0a, b727863; docs/native-boot.md "The
      slow flash bank while an AD2 stream runs"):** nothing reads flash >= 0x08080000 while `hy3131::streaming()`.
      The USB volume answers a READ(10) of a slow-bank sector (CALIB / BOOTDATA / LOADER whole, FIRMWARE.BIN
      from offset 0x7C000) NOT READY 02/04/01, the host retries (option a; b cannot hold LOADER / the
      FIRMWARE.BIN tail, c meets the 30 s command timeout). The settings / calibration record is read through
      a 1 KiB SRAM copy made at start-up (dmm.rs `load_record`: `ad2_scale` per Wave view frame and
      LOG:SCALe?, `dmm::cal`, scope gain / offset pairs, hwtrig's map, all found reading it while streaming);
      the boot-data version bytes (BT 02, Device page) copied by `Bt::new`; Logger::start / isr::start back in
      region B (their tails ran after stream::start), so is LOG:SCALe? (main.rs log_scale, region C code read
      while streaming). Emulator: peripherals/FlashWatch.cs + regionc.py watch
      DATA loads from 0x08080000..0x08100000 (and slow-bank PCs the ISR interrupts) while HANDED; fastlogtest
      `--region-c --host-read` FAILs on 2f5d3b8 (memcpy <- 0x08080000, vdisk::mem_read <- 0x080C0000,
      Logger::start) and PASSes on b727863 at 640 (AD1) / 4800 / 38400 / 76800, NOT READY exactly on those
      sectors, the same bytes after the session; `--poll` with the scope / LOG / identity queries at 9600:
      PASS; wavetest (INFO/ in the Wave view), ad2streamtest, inrushtest, fatren-test, tools/*/run.sh,
      emu/hy3131/tests: PASS. Linux over the real usb-storage
      path (emu/kvmtest/run.sh, KVM guest, vhci-hcd): a 6 s session = the copy waits ~65 s, exit 0, bytes
      equal, no kernel message; past 180 s (6 x the 30 s sd timeout, scsi_cmd_runtime_exceeced) the kernel
      logs an I/O error, O_DIRECT reads fail EIO (buffered cat survived via the page-cache re-read).
      **Device retest** (c92b605 item 3 again, on the image from b727863 or later): (1) 38400 and 76800 disk sessions,
      host `cp INFO/*.BIN` (FIRMWARE.BIN first) during state 3: DBG:ISR? max_lat <= ~300 ns, late 0, overruns 0,
      the file lost_before 0; the cp finishes after the session ends, files byte-equal to copies made
      before (dmesg: no I/O error when the session is < 3 min). (2) The same in the Wave view at 1 ms/div
      (76800): cp blocks while the view is open, completes after MENU. (3) O_DIRECT split test of the c92b605
      run (`fwdirect.py` 0x7C000..0x82000 and CALIB / LOADER / BOOTDATA) during 38400: each read waits, no
      max_lat jump; `fwdirect.py` 0..0x70000 still reads at once (280 ns). A buffered `cp` of the FIRMWARE.BIN
      head may also wait: a READ(10) is refused whole when any of its sectors is slow, and the kernel's
      readahead near offset 0x7C000 spans the boundary. (4) UID / OTP / OPTBYTES.BIN (system memory 0x1FFF7xxx / 0x1FFFC000, not refused: assumed
      fast, not measured on c92b605) read during a 76800 session: max_lat. (5) SCOP:MEAS:VPP? / WAV:PRE? /
      DBG:HWTRIG? / LOG:SCALe? / LOG:DEST? / LOG:DURation? polled during a 38400 disk session, and BT
      command 02 in the Wave view: late 0. (6) Device page / *IDN? / SYST:MOD? / BT 00 / 02 values unchanged (copies). (7) Windows: copy
      INFO/CALIB.BIN during a 60 s session: does Explorer wait or fail, and after how long? (The read-only
      volume itself passed on Windows 11, c92b605, above.)
      **Device 519fb8a (device-check-2026-10-21.md "519fb8a"): 1-4, 6 PASS, 5 PASS for the listed queries;
      (7) Windows not run.** 38400 / 76800 disk sessions with `cp INFO/*.BIN` (FIRMWARE.BIN first): max_lat
      280 ns, late / overruns / lost_before 0, the cp returns as the session ends, the copies are byte-equal, no
      sda dmesg line. Wave view 1 ms/div: cp blocked 40 s, returned at MENU, late 0. O_DIRECT splits: slow ranges
      wait the whole session, the head reads at once, 280 ns. UID / OTP / OPTBYTES at 76800: 867 reads, 280 ns.
      BT 00 / 02 in the Wave view: late 0, but not answered (stock frames wait for the stream's end, UF-36).
- [x] **Generator queries re-apply the generator, in region C (device 519fb8a).** `OUTP?` polled during a 38400
      disk session: late 25, max_lat 21.1 us at lat_pc `Gen::apply`, 29 samples lost in 10 s (an ngscopeclient
      polling `OUTP?` did the same to three of the retest runs). `App::scpi` called `gen_set` after every
      generator command, queries included (also a stop_outputs + restart glitch on a running output, any time).
      Fixed on branch devchk-35fc: `gen_set` only for settings (`reply.is_none()`), SOUR:FREQ? from a cached
      `Gen::out_hz` (gen::plan::Timing::hz is region C), `Out::duty_pct` inline. Emulator fastlogtest
      `--region-c --host-read --poll 'OUTP?;SOUR:FREQ?;SOUR:FUNC?;SOUR:VOLT?'`: base FAIL (Gen::apply), fix
      PASS (also SOUR:VOLT:OFFS? / DCYC?), the 640 / 4800 / 38400 / 76800 region C run PASS. Device retest: OUTP? / SOUR:*? polled in a 38400 disk session: late 0; with
      the generator ON (owner only), OUTP? no longer glitches the output. Left: a generator SETTING during a
      stream still runs Gen::apply in region C.
      **Device 469f354 (device-check-2026-10-21.md "469f354"): PASS.** OUTP?, SOUR:FREQ?, SOUR:FUNC?, SOUR:VOLT?,
      SOUR:VOLT:OFFS?, SOUR:FUNC:SQU:DCYC? every 0.25 s in AD2 disk sessions: 38400 (360 queries) and 76800
      (384): late 0, overruns 0, max_lat 280 ns, lost_before 0; OUTP? 0 throughout (OFF stays OFF). Left: the
      generator-ON glitch check (in "Owner-only device checks", top) and the SETTING-during-a-stream remainder (fixed, next lines).
      **SETTING remainder fixed (branch tmcstress):** Gen::apply and gen::plan (~3.7 KB) moved from region C to B
      (link-native.x; B free 47.6 -> 43.8 KB, >= 16 KB): moving was cheaper and simpler than deferring a set to the
      stream's end, which would also change what SOUR:*? answers meanwhile. The generator page (taps, ui::gen_*) stays in C:
      touch is off / the page is not shown while an AD2 stream runs (re-check once stream-ui's UI nav during a
      fast-log session merges). Emulator fastlogtest --region-c --host-read --sessions ad2:38400,ad2:76800 --poll
      'OUTP ON;SOUR:FREQ 1000;SOUR:FUNC SQU;SOUR:FREQ 2345;SOUR:VOLT 1.5;SOUR:FUNC SIN;SOUR:VOLT:OFFS 0.2;OUTP
      OFF;SOUR:FREQ 100000;SOUR:FUNC:SQU:DCYC 30;OUTP?' (generator ON in the emulator only): before FAIL at both
      rates (Gen::apply, Settings::with_amp), after PASS (the 76800 session ran at the 38400 rung after its writer
      trial in both runs, typ profile, lost 0). Device retest: the same settings with the output OFF
      (no OUTP ON on the device): late 0 at 38400 / 76800.
      **Device 40728b8: PASS** (doc "40728b8" 4a / 4b): SOUR:FREQ 1000; SOUR:FUNC SQU; SOUR:VOLT 1.5; SOUR:FREQ 2345
      during AD2 disk sessions with the 6 queries polled: late 0, overruns 0, max_lat 280 ns, 576 002 / 1 152 003
      rows, lost 0; OUTP? 0 throughout. Open: the generator-ON check (owner only).
- [x] A copy of a slow-bank INFO/ file while the meter streams for more than ~3 min (an open Wave view, a long
      fast-log session) can fail on Linux (EIO after 180 s; buffered reads usually recover) and maybe sooner on
      Windows. **Fixed (branch info-mirror, 12a1d9c; docs/native-boot.md "The slow flash bank while an AD2 stream
      runs", "USB volume, the mirror"):** flash 0x08080000..0x080C1000 (every slow-bank INFO/ byte: the
      FIRMWARE.BIN tail, LOADER with BOOTDATA, CALIB; 260 KiB) is mirrored as `_INFOMIR.BIN` on the SPI disk and
      served from there while `streaming()`; compared byte for byte with the flash at every boot
      (calbackup::write_file, rewritten copy-on-write only on a difference: a new image / calibration), served only
      after this boot's PRESENT / WRITTEN. NOT READY stays as the fallback (no / full SPI disk, a failed read).
      Cost: 65 of the SPI disk's 4 KiB clusters, 144 B SRAM, region B +~0.5 KB; boot: a compare ~0.22 s
      (emulator), a rewrite ~1.0 s (4.3 s with the chip's typ erase / program times). DBG:VDISK:MIR? = served,
      status (3 PRESENT / 4 WRITTEN), rewritten, ms.
      Emulator: fastlogtest `--region-c --host-read --dur 20` at 38400 / 76800 (typ chip timing): every
      slow-bank sector of the four files read during the session (37 / 24 MB of INFO/ in all), no NOT READY,
      the same bytes after, no region C call / slow-bank load, lost 0 at both rates (76800 kept its rung); the
      same with `--profiles max` (tPP 3 ms: 76800 runs at the 38400 rung as before): PASS, lost 0.
      mirrortest.py (new): fresh disk WRITTEN, same image PRESENT with the SPI image unchanged, image B
      WRITTEN, a calibration-sector byte changed WRITTEN, then PRESENT; each time the file == the flash window.
      kvmtest (Linux usb-storage): a 25 s session, `cat` of the four files (and an O_DIRECT CALIB.BIN) while it
      streams, 10.7 s, exit 0, identical; with `dm40wait=240` (copied 240 s wall into the stream, past the old
      180 s limit): 7.4 s, exit 0, identical, no kernel I/O error. wavetest (Wave view at 1 ms/div): every INFO/
      file read whole while the view streams, the bytes equal to a read after the view closed, region C clean
      (the allowed Save CSV handler aside), PASS. Boot cost: +0.22 s every boot (the compare); the first boot
      of a new image ~1.0 s, 4.3 s typ / 8.8 s at the datasheet's max erase / program times (fastlogtest
      `--profiles max`). If that is too slow: a header with the loader marker + the CRCs vdisk::init already
      computes, compared before the full walk. A failed SPI read turns the mirror off until the next boot.
      **Device retest:** (1) first boot of the new image: DBG:VDISK:MIR? `1,4,1,<ms>` (note the boot time),
      second boot `1,3,0,<ms>`; DBG:BOOT? as before. (2) 76800 and 38400 disk sessions of >= 5 min with `cp
      INFO/*.BIN` (FIRMWARE.BIN first) during state 3: the cp returns at once, files byte-equal to copies made
      before, DBG:ISR? max_lat 280 ns / late 0, LOG:COUNt? lost 0, LOG:DISK? fields unchanged vs a session
      without the copy (the SPI reads share the chip with the logger's page programs). (3) The same in the
      Wave view at 1 ms/div for > 3 min. (4) Windows Explorer copy of INFO/ during a session.
      **Device c785807 (device-check-2026-10-21.md "c785807" M1-M5): (1) PASS** (first boot `1,4,1,5164`,
      after DBG:RESET `1,3,0,302`). **(2) 38400 PASS, 76800 FAIL**: 38400, 286 s until the disk was full, 5 cp
      rounds of the 8 files: each returns in ~1.1 s, byte-equal, lost 0, late 0, max_lat 280 ns. 76800: every
      cp of a mirrored file after the writer trial drops samples (LOADER.BIN alone: 21 745; the 8 files: ~46 000
      per round; CALIB.BIN 4 KiB / non-mirrored files: 0); a cp during the trial makes it fall back to 38400
      (gate 2). >= 5 min was not possible: the free 12.9 MB hold ~286 s at 38400 and ~130 s at 76800.
      (3) Wave view 200 s, 5 rounds: copies fast and byte-equal, late 0 / overruns 0; ring `dropped` grows
      ~50/s with or without a copy (open, below). (4) not run.
- [x] **76800 disk session + cp of a mirrored INFO/ file loses samples** (device c785807, doc "c785807" M2).
      **Fixed on branch mirror-bw (emulator; device retest pending).** Cause: not the chip's busy time but the
      main loop's CPU. A mirror sector is a polled 512-byte SPI read (~0.97 us per byte on the device, plus a
      wait for a page program in the chip), ~1.1 ms of wall time with the stream ISR at ~54 % of the CPU; a
      host copying back to back got one or more of those every pass and the fast logger's coder fell behind
      (memory-served INFO/ files cost almost nothing: loss in proportion to the mirrored bytes, LOADER.BIN 128
      KiB ~21 700, a round ~46 000). The emulator missed it: its polled byte took 0.64 us (no APB / TBE-to-shift
      latency). Fix (vdisk.rs `mirror_pass` / `mir_hold`, msc.rs `read_wait`, fastlog.rs `mirror_level`):
      while the stream runs a mirror sector is read only when the pass grants it: at most one per main-loop
      pass, only with the chip idle, and only when the fast logger has caught up (ring <= 384 records and
      FIFO <= 1 KiB at the pass's start); behind: after 10 ms of waiting; pressed (ring over half full, or the
      writer trial): after 50 ms. Until then the READ waits in its data phase (IN NAKed, no NOT READY, nothing
      for the host to retry); a 240-sector READ takes <= 12 s even when every sector waits the full 50 ms.
      DBG:VDISK:MIR? fields 5-7: mirror sectors read while streaming, holds, longest hold (ms). LOG:DISK?
      field 14 (new): the writer's share of the main loop's CPU (permille, ISR out) = 1000 - headroom.
      Emulator model (24547cf): GD32_SPI IdleStartNs 330 (run.py --spi-disk-gap-ns) gives the device's 0.97 us
      polled byte; W25Q128 StrictBusy ignores a READ / program during BUSY. Results (--mips 110
      --isr-stall-ns 700, typ chip, LOG:DUR 10, `fastlogtest --host-read --host-files LOADER.BIN --host-only
      --host-delay 3`): 9cfa6a6 lost 6 428 with the old model, **119 853 of 768 000** with the new one (~10 000
      per LOADER.BIN copy; device ~21 700); a copy inside the trial: fell back to 38400 (as on the device).
      This branch: **lost 0**, rung kept, 2 279 mirror sectors read in the run, longest hold 39 ms; copy in
      the trial: kept 76800, lost 0; every INFO/ file + DBG:PROF? polls at 38400 / 76800 with `--region-c`:
      lost 0, no region C code / slow-bank load, no NOT READY; --noise 1000 (2.43 B/sample, 730 pages/s) +
      LOADER.BIN: lost 0. Linux usb-storage (emu/kvmtest, KVM_MIPS=110 KVM_ISR_STALL_NS=700, dm40rate=76800):
      `cat` of CALIB / BOOTDATA / LOADER / FIRMWARE.BIN with drop_caches, byte-identical, no kernel sense /
      reset lines; started 40 s into the run, 2 rounds: lost 0; started inside the writer trial: 9cfa6a6 fell
      back to 38400, this branch kept 76800 with lost 0 in one run and 20 (0.26 ms, cause not isolated) in
      another. The copy is slower than before (4 files: ~27-35 s of guest time vs ~11 s; the emulator runs
      ~10x slower than real time). Headroom (LOG:DISK? field 14, no copy): the writer takes 589 permille of the main
      loop at 76800 / 1.21 B/sample, 702 at 2.43 B/sample, 300 at 38400. Device retest: below.
- [x] Wave view at 1 ms/div: DBG:ISR? `dropped` (ring full) grows ~50/s on c785807. **Fixed on branch
      mirror-bw (emulator).** `dropped` (field 6) = logger ring overflows: the stream ISR found the 2048-record
      ring (26.7 ms at 76800) full and dropped the newest samples; each one also restarts the wave record (no
      triggered frame across a hole), so it was a real loss of displayed frames, not decimation. Cause: a
      scope pass with a status change (status badge ~6 ms + record bar ~7.6 ms, ~5/s with a 10 Hz square in
      AUTO at 1 ms/div) plus one measurement cell (~10 ms) after the acquisition took ~30 ms (emulator
      --mips 110: `emu/wavedroptest.py --dmm vsrc:0,ac=2,f=10,shape=square`: ~400 drops/s, passes up to 30.5
      ms; a flat DC input: 0, as the device's idle windows). Fix (app.rs `sp_paint_pass`, scope.rs
      `wave_keep`): the ring is drained into the record before the paint and before the cell, and the cell
      waits for the next pass after a paint over 4 ms. After: 0 drops in 40 s, loop max 20.6 ms. Device
      retest: below.
- [x] **Device retest of mirror-bw** (the image built from the branch's last commit): (1) 76800 `LOG:DUR 0`
      disk session, cp of the 8 INFO/*.BIN every 30 s (M2b's sessM.py) and LOADER.BIN 6 s after state 3
      (M2c): lost_before / LOG:COUNt? lost 0, gate 1 (76800 kept), copies byte-equal; note each cp's time and
      DBG:VDISK:MIR? fields 5-7 (expect cp LOADER.BIN slower than 0.5 s, all holds <= 50 ms). (2) cp of
      LOADER.BIN inside the writer trial: gate 1 (no fallback), lost 0. (3) LOG:DISK? field 14 at 76800 with
      no copy: the device's headroom (1000 - it; emulator 411). (4) 38400 with cp every 60 s: lost 0, cp times
      as M2a (~1.1 s per round of LOADER.BIN). (5) Wave view at 1 ms/div with the 10 Hz square (wv.py 200 s,
      cp rounds every 40 s): DBG:ISR? dropped +0, DBG:PROF? loop max < 26 ms. (6) dmesg on the host: no
      usb-storage reset / timeout during the copies.
      **Device d2aff22 (device-check-2026-10-21.md "d2aff22"): PASS 1-6.** (1) 76800 until full, 5 rounds: 9 820 200
      rows, lost 0, gate 1, copies byte-equal (FIRMWARE.BIN 1.7 s, LOADER.BIN 0.7 s), longest hold 11 ms; LOADER.BIN
      at state 3 + 6 s: lost 0 (was 21 745). (2) in the trial x3: gate 1, lost 0, copy 1.65 s, holds up to 50 ms.
      (3) field 14 ~508 (headroom ~492; emulator 411). (4) 38400 240 s: lost 0, FIRMWARE.BIN 0.87 s. (5) Wave view
      200 s with 5 rounds: dropped 0, loop max 17.5 ms. (6) kernel log clean.
- [x] UI navigation during a fast-log session (menu pages / the scope page with the scope source open while the
      session streams). **Done in 3f32bc3 / 7225b36** (`fastlogtest --ui-nav`, DBG:VIEW? checks each landing).
      Note: on the device only DBG:KEY / DBG:TAP can do this; real touch and the side keys stop a disk session
      at the first press (main.rs `fast_touch`), a host session ignores them, BT serve is gated. Found and
      fixed: (1) **a disk session started by a host while the meter showed the Scope page recorded 0 rows**
      (the DMM rail was off: `fast_start` powered it and wrote the row at once, the chip ignored it; the same
      path serves LOG:DEST HOST): `Dmm::start` now waits out the rail-wake settle (<= 50 ms), and a logger
      session keeps the DMM rail on in every view (`App::logger_busy`; the Oscilloscope tile during a session
      dropped it). fastlogtest now checks rows ~ rate x duration for every session (the 0-row session had
      passed every other check). (2) Region C under the stream: `App::press_track` (pressed feedback, now
      skipped while `hy3131::streaming()`) and `ui::main_menu_hit` + its literal pool (now region B). (3) The
      source badge during a session ran `wave_open` (region C; it also steps the DMM range, which ends the
      session with "Settings conflict"): refused with a toast "Wave: data logger running".
      Emulator (typ, --dur 8, --region-c --ui-nav, 38400 and 76800): every landing right, no region C / slow-
      bank read, rows 307 201 / 614 401, lost 0; `--ui-nav-steps 0` (start on the Scope page only): before
      0 rows, after 153 601 at 38400 x 4 s. Device retest: on the Scope page (MCX), `LOG:DEST DISK`,
      `LOG:DURation 10`, `LOG:STATe ON` at 38400 and 76800: the DLG has ~rate x 10 rows, lost 0; the same
      with `LOG:DEST HOST` (LOG:DATA? drains records).
      **Device d330877 (device-check-2026-10-21.md "d330877" 1a): PASS**: MCX page, DISK 384 002 / 768 002 rows in 10 s, lost 0, late 0;
      HOST 38400 lost 0, 76800 lost 0 with the known host-side ring overwrite. The badge during a session is refused
      (1d), but the "Wave: data logger running" toast is hidden under the DATA LOGGING panel: open (cosmetic).
- [x] A toast during a logger session (device d330877 1d: the badge's "Wave: data logger running" was drawn
      under the DATA LOGGING panel, only its red ends showed, and they stayed: nothing repaints the view during a
      session). Branch smallbugs: while the panel is up (main.rs `dialog::log_panel_on`) every toast goes into
      the panel's body (dialog.rs `LOG_TOAST`, under the title), the panel's refresh is held while it is up, and
      `toast_gone` (expiry / dismiss, app.rs) draws the whole panel again, which covers it exactly. The panel is
      now as wide as a toast (40..440, was 70..410) so every toast text fits at spacing 2. The firmware-install
      confirm (a file written to the USB disk) waits for the session's end (its box is taller than the panel).
      All toasts reach this path (one draw function); the ones that can fire in a session: the source badge,
      "Stopped: data logger running" (inrush page, host session), FW_BAD, DBG:TAP / DBG:KEY driven page toasts.
      Confirms opened by DBG:TAP in a session (power entry) are not covered (no real touch reaches the UI in a
      session: any press stops a disk session, a host session reads no touch). ui-render 6d' (every toast text
      in the panel, refresh held, the panel byte-equal after); emulator (badge scenario, 38400 disk session on
      the MCX page: toast in the panel body, nothing outside the panel box changed, the panel back after it;
      with emu/regionc.py hooks from the tap to 4 s after it, the toast's end and the panel's redraw included:
      no region C code, no slow-bank read, the session runs on); fastlogtest --ui-nav (no --region-c) PASS.
      ~~Open (emulator harness): `fastlogtest --ui-nav --region-c` stalls without an error in nav_tree's second
      sequence~~ **Fixed (branch uinav-stall, harness only):** not a firmware hang and not a test wait. The
      screenshot pre-erase (`screenshot::prep_step` / `screenshot::disk`, region C, main.rs `prep_poll`) starts
      after 2 s of disk quiet (PREP_QUIET_MS, uptime ~2.3 s: the end of nav_tree's first sequence) and, with
      fastlogtest's W25Q128 busy timing (45 ms per 4 KiB at typ), runs for minutes of device time. The Python
      hooks of emu/regionc.py on those two functions slowed the emulator ~50x (each 300 ms step 1-3 min wall;
      the virtual time crawled, so `wait_dev` never looked stuck). Bisected: hooks without FlashWatch hang,
      FlashWatch alone passes, the 277 other hooks alone pass, those 2 hooks alone hang; a plain run.py without
      the flash timing is fast even with them (the prep ends at once). Why Renode is that slow there (only ~0.5
      hook calls per wall second) is not chased. Fix: fastlogtest excludes both from the hooks (as fastlog::prep
      / fatren); `prep_poll` never runs in a session, and if it did, FlashWatch would still report prep_step's
      literal-pool loads from region C. `fastlogtest --profiles typ --sessions ad2:38400,ad2:76800 --dur 8
      --ui-nav --region-c`: before: hung after "page tree ... ok" (killed at 10 min; coordinator 40 min);
      after: PASS in 11 min wall (659 s, host shared with other emulators), no region C / slow-bank read in
      either session, rows 307 201 / 614 401, lost 0.
      Device retest: as d330877 1d (MCX page, AD2 38400 disk session, DBG:TAP 177,10): the toast reads in the
      panel's body under "DATA LOGGING" for 1.5 s, then the panel's rows come back; no red left outside it.
      **Device c785807: PASS** (doc "c785807" T1: toast in the panel body, panel back at 1.9 s, nothing outside).
- [x] A stream starting inside a multi-block READ(10) (fails at the next slow block with the residue,
      sense 02/04/01; the host retries the whole command).
      Device 2f56ff9 (docs/device-check-2026-10-21.md "2f56ff9: screenshot preparation, the 76.8 kS/s nominal" item 4): not provoked. `read_busy` refuses only while
      the slow-bank mirror is off (MIR_OK false after a failed SPI read); `DBG:VDISK:MIR?` 1 (OK), so a healthy
      meter cannot reach the path from the host; the device has no switch that turns the mirror off.
      Emulator (a blank SPI disk: no mirror): emu/mirrortest.py `--fallback-only` boot 6, a READ(10) of 128
      sectors across FIRMWARE.BIN's slow-bank start, the AD2 stream started after 32: CSW status 1, residue =
      the 64 slow sectors, sense 02/04/01, the 64 sectors before them correct; the retry during the stream fails
      before its data; after the stream it reads the same bytes. With msc.rs's mid-transfer check removed the
      result is the same (vdisk `fw_body` -> `mir_read` fails the slow sector without a mirror while the stream
      runs; msc.rs takes the sense from `read_busy`), so msc.rs's own mid-transfer check is not exercised.
      Known and left: region C DBG commands (DBG:GENUINE? reads the loader data at 0x080A4000 in region C
      code, DBG:BOOT?, the AD2 experiments) still stall the ISR when sent during a stream (debug only).
- [x] `fast_scpi` (LOG:DEST / LOG:DURation) in region C (device c92b605: LOG:DEST? / DUR? in the Wave view,
      late 1, 2.8 / 5.8 us). Fixed in 8c5ac07 (out of .cold; on rust-fw-poc). Emulator: fastlogtest
      `--host-read` and wavetest's 2 / 1 ms/div polls now send LOG:DEST? / LOG:DURation? while streaming
      (region C hooks: none). Device retest: LOG:DEST? / DUR? in the Wave view at 1 ms/div, late 0.
- [x] MMEM:CAT? in the Wave view at 76800: 123 ms in one pass, `dropped` +6813 per query (the view's ring; ISR
      late 0). Device c92b605 `wdrop.py`. **Fixed in 3f32bc3:** the reply is cached (csvlog.rs `Cat`, 6 KiB
      SRAM) and valid while `msc::FLASH_WRITES` (every program / erase) is unchanged; `catalog_warm` builds
      it before the Wave view's / the inrush capture's stream starts (also after a Save CSV / screenshot
      pause); "Mass storage busy" as before while a writer holds the disk. Emulator wavetest 1 ms/div,
      MMEM:CAT? x3 while streaming: before overwritten +7904, after 0 (2 ms/div 0 both). Device retest:
      `wdrop.py` in the Wave view at 1 ms/div: `dropped` +0 per MMEM:CAT?; after a Save CSV the new SCOPnnn
      shows in MMEM:CAT?.
      **Device d330877 (device-check-2026-10-21.md "d330877" 1b): PASS**: MMEM:CAT? 7-20 ms, dropped +0, late +0 (x3); Save CSV not
      repeated.
- [x] 76800 stream / rate (loop fix PASSED on device c830e09; its nominal was wrong, fixed in 69abac4, device
      check PASSED on 2f56ff9). **Device c830e09 (device-check-2026-10-21.md "c830e09: the 76.8 kS/s PLL retest"):** the
      loop locks from a cold start within 5 s at rate_mhz 76 800 0xx = ppm_x10 -29 35x (-2935 ppm) against the 76 574.6
      nominal, 1.125 ISR entries per sample (was 1.478), cpu 455-457 permille, probes / captured 0.063,
      edges_after_b / probes 0.50, late / bad / overruns / slips 0, the transition-sample scatter 0.41-0.62 us
      rms (was 3.0-3.45), disk 76800 lost_before 0 with ring high water 52 (was ~1050). The rate is the
      datasheet's: gap-free disk data 1919.95 samples per 40 Hz calibrator period (decaf60: 1914.27), the
      Wave view's record 960 per half period. **So the "real rate 76 574.6" below was the old loop's doing,
      not the chip's** (a slower chip under that read pattern or skipped conversions: not separated), and
      c830e09's nominal made LOG:PRE? / the DLG and CSV headers / the Wave view 0.29 % off (39.88 Hz for the
      40 Hz square). 69abac4: AD2_OSR32_REAL_MHZ = ad2_rate_mhz(0), emulator Ad2Osr32Ppm default 0; the loop
      and MAX_PPM 5000 stay (owner: keep 5000?). Device check of 69abac4: a fresh-boot 76800 host drain locks
      near ppm_x10 0 (as 38400: -2..-16), entries ~1.125; LOG:PRE? nominal_mhz 76 799 016; ACQ:SRAT? 76799 and
      the 1 ms/div record's 40 Hz square at 40.00 Hz from WAV:PRE? xinc; the DLG header nominal 76 799.016.
      **Device 2f56ff9 (docs/device-check-2026-10-21.md "2f56ff9: screenshot preparation, the 76.8 kS/s nominal") item 2: PASS**: first AD2 session after boot,
      ppm_x10 -2..-4 from 5 s, entries 1.1252, cpu 461, error counters 0; LOG:PRE? 76 799 016; CSV / DLG headers
      76799.016; record xinc 13.02084 us -> 40.0011 Hz; 1919.948 samples per calibrator period. ACQ:SRAT? is
      76 800, not 76 799: `wave_real_fs` uses ad2_rate_mhz(0) = exactly 76 800 000 mHz, the logger's 76 799.016 is
      1e12 / the rounded 13 021 ns period (12.8 ppm apart; the prediction above was wrong, not the firmware).
      Open: owner's call on MAX_PPM 5000.
      The history as investigated before the retest (its premise is wrong, see above):
      Device 20a91d0 / 825080c: rate_mhz 76 723.4-.8 =
      the Pll's +1000 ppm clamp in every 76800 session, 1.476 / 1.478 ISR entries per sample, cpu_permille
      544-548; the record stamps 76 574.5 / 76 574.4-.9 S/s, LOG:PRE? measured_hz 76 574. Investigated
      2026-10-23 (docs/dmm-input-scope-feasibility.md 11.12 "The 76.8 kS/s rung's rate and the lopsided
      probe window"):
      - The rung's real rate is 76 574.6 S/s, 0.29 % below the datasheet's FAD2CLK / OSR2 = 76 800: the
        stamps agree to +-3 ppm over two builds, fresh boots, disk / host / Wave view sessions, CHOP on /
        off; a 40 Hz calibrator square has 956..959 samples per half period (960 nominal); the same chip
        and crystal lock at -1.6 ppm at 4800 / 38400, also with the same hybrid frames (DBG:AD2:DIV 8,1,
        probes as often), so neither the crystal nor the read pattern. 64.188 SYSCLK per result (64 + 3/16
        would be 76 574.85). Deterministic as far as one unit shows -> hy3131.rs AD2_OSR32_REAL_MHZ: the
        stream's Pll nominal, LOG:PRE? nominal_mhz / the CSV nominal_sps / the DLG header and the Wave view's
        measurements, FFT and CSV at 1 ms/div (scope.rs wave_real_fs; they read 0.29 % high) use it; the
        rung keeps its name 76800 (LOG:SRATe, the bands). The emulator's chip does the same (HY3131
        Ad2Osr32Ppm, default -2935; 0 = the datasheet).
      - Why it railed and what that cost: the hybrid geometry's probe window is lopsided: B may be early by
        up to ~590 counts (11.8 us) but late by only ~61 (1.2 us, P - (C - B)); a B later than that has its
        read-back C after the NEXT edge and reads "edge after B", so the kicks walk the phase forward a whole
        period to that next edge (one conversion slips, frames tear on the way), and the old loop stepped
        the word on every result of the walk, all "lengthen". With the chip 2935 ppm off the clamp's 1000,
        the phase was held by kicks with such walks over and over: in the 20a91d0 square dumps a transition
        sample's value against its stamp's phase scatters over the whole edge ramp (~+-6 us, 3 us rms; 0.5
        us at 38400): conversions doubled / skipped / torn unseen (bad / lost / slips 0: nothing counts a
        slip, the sign-copy check cannot see a torn frame). The emulator at the device's rate and the old
        build: railed at 76 723.4, 1.69 entries per sample, 98 doubled / 98 skipped conversions in 10 s.
      - Fix, both (the nominal alone would leave a unit at the datasheet's rate +2935 ppm out, and the
        walks' integration made any big offset rail): the nominal above; MAX_PPM 1000 -> 5000; and the loop
        (ad2pll.rs, hybrid geometry only): the word does not follow a run of one sign longer than 8 (a
        walk); the probes stay dense, with a 4x frequency step, for 256 results after the start and after a
        run longer than 16 (a word off the chip's rate is pulled in before a sparse 16-period interval can
        drift the phase out of the window). The 76.8 kS/s rung carries its own lock to its next session
        (logger.rs REL_HYB); the shared REL takes a lock only within 1000 ppm. Host model: chips -3000 ..
        +3000 ppm from starts at -1000 / 0 / +1000 ppm lock on the chip's rate, no bad sample after
        acquisition (the old loop: railed from a start ~600 ppm long or a chip that fast).
      - Emulator (--mips 110 --isr-stall-ns 700): see the feasibility doc's table; start-of-session walks
        (the edge search's error, max run ~45, a slip or two) remain as in the old build but no longer
        drag the word.
      Device retest: done on c830e09 (above).
- [x] Wave view at 76800, 1 ms/div (device c830e09): late 1, DBG:ISR? max_lat 6.16 us at lat_pc
      `scope_wav::query` (region C), with ACQ:SRAT? / SCOP:MEAS:FREQ? / VPP? polled every 5 s (decaf60's
      p5.py polled only DBG:ISR? / LOG:COUNt?: 280 ns). The range_cmd pattern: the SCOPe / ACQuire query path
      fetches from C while the stream runs.
      **Fixed (dda802c):** scope_wav.rs and its parser scpi::wav_cmd are region B (~2 KB). wavetest polls
      ACQ:SRAT? / SCOP:MEAS:FREQ? / VPP? / TRIG:STAT? / WAV:POIN? / FORM? / PRE? / DATA? / CHAN1:OFFS? /
      SCAL? / TIM:SCAL? / DBG:PROF? at 2 and 1 ms/div for 4 s each under the region C hooks: none (the old
      build: scope_wav::query, prof::report). Device c92b605: PASS (1 ms/div, 61 s, the c830e09 polling + CONF? /
      MMEM:CAT?: 260 ns, late 0). Its new whole-run check found three more of the kind, now
      region B too: CONF? (App::range_si) and MMEM:CAT? (csvlog::catalog_scpi with fatdir's for_each / open /
      find_root / chain / fat_get / dir_of and fatren's le16 / le32 / sfn_is / parse_bpb / Vol helpers,
      `.text.fat_rd`) in the Wave view. Region B 57.7 -> 51.5 KB free, C 16.9 -> 22.8 KB. Device retest: the c830e09 polling
      at 1 ms/div, max_lat / late. The FREQ? item below stays open. Also: SCOP:MEAS:FREQ? answers 0 (screen "Freq: --") at 1 and
      2 ms/div on a 40 Hz signal (less than one period on the 10 / 20 ms screen), so the Wave view cannot
      show a 76800 frequency below ~100 Hz; the record (WAV:POIN MAX, 348 ms) would have the periods.
- [x] dm40scpi.py `log` stopped by SIGINT mid-drain (device c830e09): afterwards every USBTMC query read
      returned empty (new processes too) until a USBTMC INITIATE_CLEAR (ioctl USBTMC_IOCTL_CLEAR); the
      logger itself had stopped (LOG:STAT? 0). A pending bulk-IN reply left half read; the tool could catch
      SIGINT and clear, and dm40dbg.Link could clear on an empty read.
      **Mechanism (emulator over vhci, 2026-10-23, reproduced exactly):** Linux usbtmc reads a reply in two
      steps: the first 4 KiB with usb_bulk_msg_killable (only a fatal signal stops it), the rest in
      usbtmc_generic_read (wait_event_interruptible: SIGINT kills its URBs). A LOG:DATA? block is 6006 B, so a
      Ctrl-C there leaves the rest of the transfer on the meter. The next read gets those stale packets, the
      driver logs "wrong MsgID" and returns 0 bytes (no -EPROTO; auto_abort is off by default); opendm40 then
      starts the reply to that query, which nobody reads, and since a transfer already handed to the IN
      endpoint survives a new DEV_DEP_MSG_OUT (usbtmc.rs on_out_bytes) every later read gets the previous
      reply ("wrong bTag N != N+2" in dmesg, 0 bytes to the process): one reply late for ever, across
      processes. Only replies over 4084 B can start it (LOG:DATA?, MMEM:DATA? pieces, WAV:DATA?, DBG:LCD?).
      INITIATE_CLEAR recovers it (CHECK_CLEAR_STATUS pending while in flight, the driver reads the Bulk-IN to a
      short packet): checked in the emulator, so **opendm40 unchanged**; a firmware-side fix would need a DWC2
      Tx-FIFO flush on a new command (cutting the unsent rest alone keeps the one-behind offset).
      Fixed host side (tools): dm40dbg.Link.read holds SIGINT during the read (Ctrl-C acts once the reply is
      in, at most the driver's 5 s timeout later); Link.query takes an empty reply (opendm40 never sends one; a
      missing reply times out instead) as the desync: USBTMC clear, ask again, a note on stderr; Link.clear().
      dm40scpi's UsbLink goes through Link.query (every dm40scpi / acdcmeth / ad2dcv / ad2n7 query self-heals
      after another program left the meter out of step). `log --source`: the first Ctrl-C ends the session
      after the block in flight (LOG:STAT OFF, the rest drained, the trailer with `# stopped_by,ctrl-c`), a
      second one abandons the drain; LOG:STAT OFF on every way out. Also `fast`: *CLS first (a "Data stale"
      left by a READ? failed its LOG settings check), and a Ctrl-C before the first LOG:DISK? answer no longer
      crashes the end.
      Emulator: SIGINT sent while /proc/PID/wchan = usbtmc_generic_read during `log --source ad2 --rate
      38400`: the old tool "LOG:DATA? answered b''" and *IDN? empty afterwards (= the device); the new one
      10 / 10 clean exits with the trailer, *IDN? / LOG:STAT? 0 right after; a desync made on purpose
      (repro: a read cut by SIGALRM) is repaired by the next dm40scpi query. Double Ctrl-C: exit 130, LOG:STAT? 0.
      Device retest (host only, any opendm40 image): `dm40scpi.py log x.csv --source ad2 --rate 38400 -d 60`,
      Ctrl-C after ~5 s, several times: clean end with the trailer, `dm40scpi.py info` answers at once, no
      "empty USBTMC reply" note; then make the desync on purpose (kill -INT while `cat /proc/PID/wchan` reads
      usbtmc_generic_read, with the old tool from 9f4edb3) and check that `dm40scpi.py info` repairs it.
      **Device retest done (9f36bbe, 2026-10-23, device-check doc "d34413c host tools"):** 5 / 5 SIGINT runs clean
      with the trailer, `info` at once, no note. The recipe's wchan does not happen on the real meter during
      `log` (it sends the rest of the 6006 B block before the read waits; 15.7 M polls, no usbtmc_generic_read),
      so the desync was made with the old Link reading 6136 B DBG:LCD? blocks (`lcdread(0,0,320,48,15361)` in a
      loop, SIGINT at wchan = usbtmc_generic_read): *IDN? empty; the new `info` printed the note and repaired it.
      The same cut against the new Link: 3 / 3 in step.
- [x] emu/hy3131/dcvtest.py fails on 825080c and later (found 2026-10-23, not caused by the 76.8 kS/s Pll
      change: the same with the old model): LOG:SCALe? per-code 4.767e-05 / 4.812e-04 against the model's
      4.721e-05 / 4.721e-04 (+0.97 / +1.9 %, the check allows 0.3 %). modetest's stock cap-big (100 uF reads
      104 uF) fails the same way with either model.
      The test, not the firmware or the model: +0.97 / +1.9 % are exactly the model's per-range per_code factor
      of the DC-coupled row (HY3131.cs Ad2DcGainR 1.0096 / 1.0188, the device's, since fb96992, not 7ee7c09),
      which dcvtest did not expect. dcvtest now expects nominal x that factor / ad2dcg: PASS (acdc-approaches.md
      10.9 item 3). The stock cap-big case is unrelated (it fails on af4be54 already).
- [ ] 76800 CHOP ON: one isolated mid-level glitch per ~2 M samples (seq 752877: -65616 between -98272s;
      seq 133301: 82960 between ~98300s), bad 0; none in 2.1 M with CHOP OFF or at 4800 / 38400.
- [x] dm40scpi.py `log --source ad2 -d N`: drain() returns only on a block < 500 records, so above the
      host's ~35 k/s the duration is never checked (ran 150 s until killed).
      Fixed (tools): -d (seconds from LOG:STAT ON, host clock) and -n are checked after every LOG:DATA? block;
      at the end LOG:STAT OFF and the ring drained (the samples captured until then); -n writes exactly N rows;
      the trailer says `# stopped_by,duration|count|ctrl-c (after S s)`. Emulator over vhci (the emulator
      drains slower than 38400 S/s, so it hung like the device): -d 5 at 38400 ran until killed before, ends
      after 4.3 s now; -n 1234 = 1234 rows. Device retest: `-d 10` at 38400 ends after ~10 s.
      **Device retest done (9f36bbe, 2026-10-23):** `-d 10` at 38400: `stopped_by,duration (after 10.028 s)`,
      360 637 rows; `-n 5000`: exactly 5000 rows, `stopped_by,count`.
- [x] (B) DC-coupled V on AD2 (E7, feasibility 1.3 last row / section 13): device test 6.5 (f4c792e,
      DBG:AD2:DCV?, tools/ad2dcv.py): the AC V row with SAD2IP = OP1O is linear to 0.003 % of the
      range, 3..10 codes sd, gain +0.84 / +1.79 / +0.53 / +0.27 % vs nominal; AD1 reads 0.3..0.9 k
      counts low while AD2 converts. Implemented (1ad2943): LOG:SOURce AD2 / the fast logger / the
      wave view in DC V (and DC mV = the AC mV row), a sequential entry self-cal (AD1 alone, then the
      AD2 zero and signal: two-point gain above 10 000 codes, else the table; offset at the AD1
      point), LOG:SCALe?, the wave's coupling key DC <-> AC and an up-range on a clip when entered in
      AUTO. Device: host sessions within 0.02 % of the range of READ? (6 / 60 V), fast logger 0 lost.
      Note 7 in the stock DC rows (DBG:AD2:N7?, tools/ad2n7.py, section 13.2): AD2 + AD3 + RMS left
      on shift AD1 by 0.03..0.16 % of the range: no AD2-based "AC?" hint in DC V without its own cal.
  - [ ] E7 open: 600 / 1000 V gain table from >= 100 V (the calibrator stops at 24 V); offset drift over
        long sessions (no re-cal). The clip autorange: device 33b44bd, 15 V on 2 V/div -> 20 V/div (PASS);
        Wave view CONF? in DC V / DC mV answered the uA format (fixed; device 92fb8fa: "VOLT +6,+1E-04", PASS).
- [x] (B) AD2 samples to the SPI disk (.DLG): the fast logger (LOG:DEST DISK, above / docs/compact-log-format.md
      "Fast stream"). Over BT: the ~870 records/s link ceiling (docs/bt-scpi.md) is below AD2's slowest rung
      (1200 S/s): LOG:STATe ON from BT is refused for AD2 / > 800 S/s ("Rate too high for Bluetooth"), AD1
      (<= 640 S/s) streams over BT LOG:DATA?; BT LOG:DATA? of a faster USB session: empty block + the error.
      Device 8b9cf79 (before this refusal existed): AD2 1200 S/s started and drained over BT gave 918 records/s,
      lost 0, 6 593 of 36 418 overwritten (ring full): confirms the ceiling (device-check "Bluetooth (8b9cf79)").
- [ ] Fast logger device check (fastlog.rs; ~30 min; emulator results in docs/compact-log-format.md):
      1. DC V, LOG:SOUR AD1, LOG:OSR 0, LOG:CHOP OFF, LOG:DEST DISK, LOG:DUR 60, LOG:STAT ON: LOG:DISK? goes
         1 (prep, note the seconds) -> 3 -> 5; LOGS/FAST000.CSV on the USB volume == dlog.py of MMEM:DATA?
         "FAST000.DLG"; lost_before all 0 (640 S/s), LOG:COUNt? overwritten 0; DBG:PROF? loop max.
      2. AC V 0.5 V 50 Hz, LOG:SOUR AD2, LOG:SRAT 4800 / 9600 / 19200 / 38400, LOG:DUR 30 each: sum of
         lost_before and LOG:COUNt? overwritten per rate (expect 0 up to 19200; 38400 = the chip's tPP);
         DBG:ISR? max_lat while pages program (SPI1 is not the HY3131 bus: expect unchanged ~0.24 us).
      3. Record page: interval "Fast", Duration 10 min, Start in DC V: the panel shows "Preparing disk" /
         %, then rate / samples / FASTnnn KB; a tap stops it; toast "Saved FASTnnn.DLG"; a tap during
         Prep cancels (no file).
      4. LOG:DUR 0 on a disk with ~1 MB free: Prep allocates all, the session ends Full (LOG:DISK? 6).
      5. Power: pull the battery mid-run once; after boot fsck the SPI image (DEBUG/SPIFLASH.BIN):
         FAST file with the allocation's size, decodes up to the cut.
      6. BT (dm40bt_scpi.py): AD2 LOG:STAT ON -> "Rate too high for Bluetooth"; AD1 OSR 0 CHOP OFF over BT:
         dm40scpi.py --bt log --source ad1 --osr 0 --chop off for 60 s, LOG:COUNt? overwritten 0 (the
         870/s ceiling: 640/s should hold), then OSR 0 chop on (320/s).
      7. Prep time on the real chip for LOG:DUR 60 at 38400 (~7 MB): erase time vs the emulator's model.
      Device e656df3 (device-check-2026-10-21.md "DMM rail wake and fast logger"): (1) AD1 640/s: 1 122 of
      ~38 400 lost (1.8 %, g2 gaps, max 4.7 ms) - FAIL, to investigate; CSV on the volume == the tool's.
      (2) AD2 4800 / 19200: 0 lost; 38400: 1 lost; max_lat 1.6-12 us (not ~0.24). Host stop mid-run: clean
      (done, decodes to the last row). 3-7 not run.
      Fixes (2026-10-22, emulator / host; device re-check = items 8-11 below):
      - (1) AD1 unchopped 160 .. 640 S/s runs on the stream ISR (logger.rs `ad1_on_isr`): R00..R02 read blind
        mid-period, phase-locked by INTF.AD1F probes (ad2pll.rs, the timer at 40 / 80 / 160 ns per
        count, PSC 4 << KSH - 1, so a 1.56 .. 6.25 ms period fits the 16-bit ARR; CPU frames at /16).
        The slower AD1 settings keep the main-loop sampler, which now reads R00..R02 blind once when
        AD1F does not come 1.25 periods after the last sample (the read-clear race; DBG:ISR? `late`).
        Emulator (AD1F race on, `IntfRaceAd1`): AD1 640 / 160 to disk 0 lost.
        Device 7674c85 (2026-10-22): AD1 640 to disk FAILED: no AD1 edge in the 3-period search, the
        gated CPU stream then never probed (probes 0, captured 0 for 700 s) -> probes while gated, an
        8-period AD1 search. AD1 40 (OSR 3 chopped, main loop) host session 60 s: 2320 samples, LOG:GAPs?
        all 0, 83 blind rescues (`late`; 3 / 91 equal to the previous value, as elsewhere): the device
        DOES lose ~4 % of AD1F flags to the read-clear race at a tight poll; the chip's real period
        there is 26.3 ms (nominal 25: the 3 % "lost" is the span vs the nominal) -> rescue stamps /
        window from the measured period; the ISR only for the unchopped settings (the Pll locks to
        the nominal within +-1000 ppm). AD2 38400 / 19200 to disk 30 s: 0 lost (was 1); max_lat 6.8 /
        6.1 us, lat_pc VDisk::block_size (region C: the desktop polls the mounted volume) -> every
        VDisk BlockDevice method and the directory sectors in region B. Volume read of FAST006.CSV
        (53 MB): ~39 KB/s, the main loop's USB section ~15 ms per pass -> DBG:VDISK:RD? to see where.
        Device c28c91c: AD1 640 / 320 / 160 (chop off) to disk 60 s: 38402 / 19201 / 9602 samples,
        0 lost, gaps 0, locked, -0.9 ppm, max_lat 0.24 us. AD1 40 host 60 s: gaps 0, 76 rescues,
        stamps 26 ms apart (LOG:COUNt? still says 73 lost: the span vs the nominal 25 ms -> fixed: the
        main-loop AD1 sampler's `lost` divides the span by the mean of the measured flag-to-flag
        periods (the rescue's p_meas, >= 8 of them; else the nominal); meter-test logger lost; verify
        on device: AD1 40 host 60 s, LOG:COUNt? lost 0 (or the real holes)). AD2 38400
        / 19200 to disk with the volume mounted: 0 lost, max_lat 0.28 us. CSV read still ~40 KB/s,
        also with strictly sequential 64 KiB reads (FADV_RANDOM): DBG:VDISK:RD? 6.4 ms per
        read_block, ~44 SPI reads of ~28 bytes (header-sized) per sector, 1 open; a host replay of
        the device's own SPI image (fatren-test vread) reads 0.07 per sector: not reproduced. ->
        two CSV cursors + seek checkpoints (a head-reading indexer no longer drags one cursor), a
        lock timeout (re-acquire after 2 s unlocked with nothing captured, then end the session),
        DBG:VDISK:RD? also counts non-cursor seeks, headers read, rows decoded: to be measured.
        Device d8e249d: FAST003.CSV sequential 64 KiB reads 571 KB/s (was ~40), RD? delta per sector
        0.21 ms, 0.1 SPI reads, 200 non-cursor seeks / 842 headers in 68 k sectors; `dd bs=1M` of
        FAST002.CSV (26 MB) 1.4 MB/s. The c28c91c slowness (one cursor walked headers per sector)
        matches the indexer interleave the second cursor + checkpoints address; not proven which.
        Sanity: AD1 640 and AD2 19200 to disk 30 s, 0 lost, locked, max_lat 0.24 / 0.28 us.
      - Emulator: the AD2 19200 DMA lock never converged in 3 of 9 sessions (7674c85; 0 of 9 on
        eb85190): a coin-toss probe around the edge geared kp up before it reached kp_min ->
        ad2pll `result`: locked also at kp <= kp_max / 4 after 64 probes (9 of 9 pass).
      - (2) max_lat 12 us at the disk session's start / stop: region C code ran while the ISR streamed
        (`fast_events` / `fast_start` (.cold) started the logger and went on in C: `fast_head`,
        `prep::run_begin`; at the end `fast_events` ran before it stopped the logger). Now
        `Logger::plan` + the header first (C), `logger.start` / `logger.stop` in main.rs (B). Emulator
        hooks on every region C function, flagged when entered with the bus handed to the ISR (the
        hy3131 HANDED byte): eb85190 `fast_events` at the stop of an AD2 38400 disk session; now none
        in disk sessions AD1 640 / 160 / 40, AD2 38400 (fastlog::prep / fatren / fatdir left unhooked:
        per-pass Prep code, too slow under hooks). Also the USB volume's read path is in B now
        (a host reading the volume while a stream runs no longer stalls the ISR).
      - (3) FAST CSV reads: no full CRC scan at the volume refresh / open (headers + the last 4
        chunks), no FAT re-walk per chunk (quadratic), the read path in region B, several USB packets
        per pass while a READ runs (docs/compact-log-format.md 5). Host (fatren-test `vread`, 50.6 MB
        CSV): SPI reads 171 k -> 13 k, ~1.5 s of SPI time; refresh ~1.5 s -> 0.04 s. Emulator
        (fastlogtest --vol-read, MSC READ(10) of FAST000.CSV, 3.5 MB): == dlog.py; ~2.1 MB/s of
        virtual time before and after (its USB moves a whole URB per poll and its flash is all
        zero-wait: the packet pumping and region C are the device's to measure, item 10).
      Open (emulator, once in 4 runs of the max-flash profile, 25f3879, 4 emulators sharing the CPU):
      an AD2 19200 disk session's DMA-frame lock never converged (DBG:ISR? locked 0, probes alternating,
      max_run 7: the phase step geared up and down), so the gated stream took no sample and the session
      never ended; two reruns of the same sequence passed, as did eb85190's once. AD2 path untouched by
      this change. Wants: a lock timeout (restart / error after N s unlocked) and a repro with --ppm.
      8. Device: AD1 640 S/s to disk 60 s (as 1): LOG:COUNt? lost 0, LOG:GAPs? all 0; DBG:ISR? active 1
         during it, rate_mhz ~640 000, locked 1, late 0, max_lat < 2 us; also 320 / 160 (OSR 1 / 2, chop
         off). AD1 40 (OSR 3) host session over USB 60 s: gaps 0, rescue stamps ~26.3 ms apart.
      9. Device: AD2 38400 / 19200 to disk 60 s: max_lat < 2 us (lat_pc if not), 0 lost; also with the
         USB volume read meanwhile (cp a big CSV): max_lat still < 2 us.
      10. Device: copy a ~50 MB FAST CSV off the volume (`time cp`): >= 500 KB/s, DBG:VDISK:RD? before
          / after (read_block calls, ms, max us, opens, flash reads / bytes); also a small one
          and LOG000.CSV; `cmp` with `dm40scpi.py get` / dlog.py of the DLG. The volume refresh after a
          large session: no visible stall (was ~1.5 s per 2 MB of DLG).
      11. Device: DMM page while a host copies: the reading keeps updating (2 ms of USB per pass).
      Device d8e249d (docs/device-check-2026-10-21.md "d8e249d" 5 / 6): 11 PASS (READ? every 0.40 s through a 45 s copy at 584 KB/s). 9 with a
      host reading the volume: AD2 19200 10 s to disk 0 lost but **max_lat 8.98 us, lat_pc
      vdisk::file_read::{closure#0} in region C** (the `near` closure got its own symbol, matched by the
      `*7dm40_fw5vdisk*` .cold rule) -> a fn in .text.vdisk_rd (fixed, to re-check). Still in region C on
      the volume read path: the DMM-log / scope CSV generators (Gen::advance, RowDec::*, SDec::*, chunk_at,
      Gen::open): a host reading LOG*.CSV / SCOP*.CSV during a stream runs C per row.
      Region B audit #3: in `.text.vdisk_rd` (B) now, with dlog::open / find / geometry / each_entry (generic,
      inline(always): no closure symbol in C) / parse_file / from_scale / s_pred, dlog::data (MMEM:DATA?),
      vdisk::readme (root directory reads) and calbackup::crc32_upd (LTO merged dlog's identical CRC into it).
      wavetest: a host reading SCOPE/SCOP000.CSV and LOGS/LOG000.CSV off the volume, and MMEM:DATA? of
      their DLGs, during the Wave view's stream: no region C call (76b79e0: SDec::get, dlog::data,
      vdisk::readme).
      - [x] **Device hang + watchdog reset (once)**: `dm40scpi.py fast --source ad2 --rate 19200 --dur 30`
            with a host reading FAST003.CSV over the volume: the session completed (FAST005.DLG intact),
            the volume's UNIT ATTENTION at the end failed the host's reads, the tool's MMEM:DATA? of
            FAST005.CSV timed out, usb-storage reset the device 33 s later, then FWDGT (DBG:BOOT? WDT_RETRY).
            Not reproduced: MMEM:DATA? alone / with a volume read of another or the same file / the session
            end with a volume read but no MMEM:DATA?. Suspect the overlap of the session end, the volume
            refresh, the host's revalidation reads and Gen::open of the new FAST file over USBTMC; repro in
            the emulator (fastlogtest --vol-read + MMEM:DATA? right after done).
            Root cause (device-check doc "d8e249d follow-up" 6b): the USB disk. usbd-storage's unstall after
            every CSW (a DIEPCTL read-modify-write, the stream ISR widening it) re-armed the IN endpoint with
            XFRSIZ 0: a spurious ZLP broke the host's next command; Linux's reset recovery then gave usbd-storage
            an invalid CBW, it called our callback with the old command and `set_status` panicked -> FWDGT.
            Fixed: usbd-storage vendored with fixes (firmware/vendor, "dm40 patch"), msc.rs / usb::abort_in.
            Emulator: emu/fastendtest.py (76b79e0 2 of 6 pass, fix 3 of 3), emu/mscresettest.py.
            Device 22c85ba: the 6b repro PASS (0 lost, the reader went on after the UNIT ATTENTION, MMEM:DATA?
            of the new 26.5 MB CSV complete, no usb-storage reset), a port reset during a volume read PASS.
- [x] (A) DMM-input "slow floating scope" via AD2 = the Wave view (E5/E6; docs/dmm-input-scope-feasibility.md 10,
      src/app_wave.rs; feature `ad2-isr` only, hidden without it): MF range page "Wave" (V / A) and the scope page's
      "MCX | DMM" source badge; the AD2 stream drained into the scope record (continuous, 10-bit codes), the stock
      page unchanged around it: 1 ms..50 s/div (76 800 / 38 400 / 9600 / 4800 / 2400 / 1200 S/s per band, roll from 200 ms/div; opens at 10 ms/div),
      V/div = the DMM range (+ / - step it), coupling = the row's (AC V AC-coupled, mV / A DC-coupled, no DC V
      row: says so), units V / A, measurements / cursors / STOP / zoom / Save CSV / FFT as the MCX scope; trigger
      (edge / pulse / runt) detected per sample IN the stream ISR (trigsoft::Stream; no dead time; decision #3
      amended: config words + event ring), HY3131 comparator evaluated, not used (10.2). DMM paused while open,
      row rewritten on exit; a host CONF closes it. Emulator: emu/hy3131/wavetest.py (sine V, square A, trigger,
      roll, range steps, exit, conflicts) PASS; ui-render wave scenes 0 violations; stock pixel diff unchanged.
- [x] Device d8e249d (docs/device-check-2026-10-21.md "d8e249d" 8): Wave view 10.6 on the 40 Hz square: entry, Freq / Vrms (-0.6 %), trigger
      Normal / Single, range 2 / 20 V/div / 200 mV/div, STOP + Save CSV, MENU exit, conflicts PASS. Fixed:
      roll at 5..50 s/div moved at the 2 s/div speed (w.per from the clamped `screen`), the DC V entry toast
      and two more toasts were wider than the box. Current steps blocked (A not wired).
      - [x] 5 ms/div: Freq / Cycl "--" with two periods on screen: fixed (the falls when fewer than two whole
            rises are inside; meter-test meas).
      - [x] Vrms -1.6 % from 50 ms/div (1200-2400 S/s): the chip's sinc^3 AD2 decimator, not the window
            (the logger CSVs' RMS, no scope_meas, showed it too; a square's |x| is constant but at the
            edges). Each sample averages ~3 periods: a 40 Hz square's RMS reads -0.47 / -0.95 / -1.9 /
            -3.8 % at 9600 / 4800 / 2400 / 1200 S/s (sines below ~0.1 fs: < 0.25 %). Band-limited data:
            not fixable after the fact; documented (dmm-input-scope-feasibility.md 3.2, scope.rs
            wave_fs_for). A 9600 S/s band up to 100 ms/div would quarter it (open, not done: the 213 ms
            drain deadline on every YT timebase; the emulator's AD2 has no comb to test it).
      - [x] A volume refresh (Save CSV, a screenshot) runs region C (vdisk::ours / refresh / scan_spi) under
            the Wave view's stream: max_lat 21.7 us, 15 overruns at 4800 S/s. Defer the refresh while the
            wave streams, or move the refresh path to region B (~3 KB).
            Region B audit #3: the refresh waits while the stream ISR owns the bus (main.rs, hy3131::streaming;
            a host sees a Wave-view save once the view closes), and the Wave view's stream pauses while a disk
            job (Save CSV, a screenshot: screenshot.rs, region C every pass) runs: the emulator showed the DLG
            write itself in C under the stream too. emu/hy3131/wavetest.py checks it (hooks on every C function
            flagged when entered with hy3131::HANDED set): 76b79e0 ran Gen::open, vdisk::refresh / scan_spi
            under the stream at the save; now only the tap's sp_save_csv. Device re-check: below.
      - [x] CONF? in the wave view on 200 mV/div (AC mV) still answers VOLT:AC +6: it built the range
            from `meter.live()`, the last reading before the view (the DMM poll is paused). Now from the
            streamed row (mode / range): "VOLT:AC +6.000000E-01,+1.000000E-05"; wavetest checks 20 V/div,
            200 mV/div and back. (verify on device: CONF? at 2 V / 20 V / 200 mV/div in the Wave view)
- [ ] (A) device test of the Wave view (feasibility 10.6, 15 min) - after the ISR loss fix (E3/E4 device item:
      its holes restart the wave record, 10.4). Follow-ups: 16-bit record, DMM reading alongside (request
      queue), DC-coupled V row (E7). Done: the timebase list starts at 1 ms/div in the wave view (WAVE_TB_MIN)
      (scope_ui::set_tb_lo from app_wave; touch-test + ui-render s01-tb-list-wave).
- [x] Device 2026-10-21 (docs/device-check-2026-10-21.md "Fixes"): stack 37468/46048 B (81 %) on 5c6b565.
      9ce523d: no USBTMC tx copy, 8-byte logger records, .sram_bss (EP_MEMORY, MSC_IO, MSC block, touch
      log, dlog TAB); build.sh native checks the static worst path (tools/stack-depth) >= 16 KiB headroom,
      link-native.x stack >= 28 KiB + 16 KiB. Emulator walk 38580/43448 -> 28532/48344 B.
      (verify on device: DBG:BOOT? STACK after the scope walk, expect ~28.5 K used of ~48.3 K)
      device a239aa6 (after the 2026-10-22 rework): 21.6 K used of 46.6 K (feasibility 11.11)
- [x] Scope 32 ms loop passes ~3/s (meas 11.4 + frame 5 + cells 12.3 ms in one pass): b155172 integer
      measurement maths, graph paint and cells on later passes (one changed cell per pass); 1849cc4 a
      trace-only graph flush stops after 4 ms. Emulator 1 ms/div loop max: noise 10.9 -> 5.7 ms, 40 Hz
      square 21.5 -> 8-10.5 ms. (verify on device: DBG:PROF? loop max <= 12 ms, 400 DBG:PING? p99) Device f80524f: no 20-50 ms passes any more;
      scope page open input loop max 12.38 ms (marginal), ping p99 0.7 ms; square after AUTO 10.65 ms. Combined
      view: loop max 11.4 ms but ping p50 11.8 / p99 16.5 ms.
      Cause (device profile): not the view but a noise-triggered trace at 49.8 fps: every column changes, the
      paint is ~10 ms per frame (frame pass 6.2 ms incl. a 4 ms flush slice + 2-3 more 4 ms slices), so the loop
      ran 2-10 ms passes back to back (scope 98 % CPU, 56 % of it in sp_tick outside the nested sections).
      Fix: flush slice 1.5 ms, the paint starts on the pass after the frame (frames in back-to-back passes still
      paint at once), DBG:PROF? "s.paint" (was x1) times it. Not measurable in the emulator (it does not reach
      that frame rate at 1 ms/div). (verify on device: combined view, open input, 1 ms/div: ping p50 <= 2 ms)
      Device c50425f: 56.6 fps, s.paint max 1.68 ms, loop max 12.3 ms, ping p50 3.3 / p99 18.1 ms (was 11.8 /
      16.5): better, p50 still above 2 ms.
      Fix after c50425f: the combined view paints at most 25 fps (app.rs COMBO_FRAME_MS 40; Auto / Normal
      running records; Single, STOP, roll, the wave view and the Scope page uncapped; a frame not drawn
      still measures and its paint pass goes on) and its flush slice is 0.8 ms (scope_ui
      set_flush_short; Scope page 1.5 ms as before). Expected: paint ~11 ms x 25 = ~28 % CPU instead of
      63 %, most passes short. Emulator (~9 fps there, cap not reached): with the cap at 200 ms the
      combined view drops to 4.7 fps and the Scope page stays at 9.5. (verify on device: combined view,
      open input, 1 ms/div, 400 x DBG:PING? p50 <= 2 ms, DBG:PROF? ~25 fps, s.paint max <= 0.9 ms)
- [x] Scope ~9 fps at 1 ms/div after AUTO: three comparator misses (AUTO's fast-timebase sweep) disabled
      the hardware trigger for good; 93d688b suspends it until the next range / timebase / coupling /
      edge / mode / type change. The emulator's comparator model barely fires (ScopeFeeder TIM2 gate
      timing; separate), so the fps itself is unverified. (verified on device f80524f: AUTO on the 40 Hz square,
      then 1 ms/div: 40.0 fps, DBG:HWTRIG? fields 5/6 = 1,1, 38 fps during level drags)
- [x] STOP then Save CSV gave "No record to save" (Auto untriggered / STOP mid-acquisition): b1ddcb9 the
      samples captured so far become an untriggered stored record on STOP. (verified on device f80524f: Auto open
      input, Auto square mid-acquisition, Normal WAIT -> SCOP000..002.DLG, "untriggered" headers, dlog.py == MMEM)
- [x] Experiment 2026-10-21 (5947de9 on the device, Linux 7.0, GNOME/udisks): a UNIT ATTENTION 06/28/00 on the
      MOUNTED read-only LUN 1 (DBG:VDISK:FORCE? 1 + ADD?) -> kernel rescans sdb, udev change DISK_MEDIA_CHANGE=1,
      the mounted vfat lists the new files at once with correct content, no I/O errors, no unmount. Held UA (the
      default) shows nothing until unmount. Simulated eject (EJECT? 3: NOT READY x3): capacity 0 -> back, stays
      mounted. Nautilus: no auto refresh, F5 shows the new files (owner: acceptable).
      => live file lists on a virtual read-only volume work on Linux without MTP if the UA is NOT held
      while locked; keep existing files' clusters fixed (cached inodes). Windows / macOS / Nautilus refresh: untested.
- [x] USB storage redesign (owner 2026-10-21): stop exporting the SPI FAT as LUN 0. The SPI flash becomes internal
      storage (FAT16 kept for the vendor FS-loader: dm40*.bin in the root + the calibration safety copy) with
      subdirectories (LOGS/, SHOTS/, SCOPE/) in our FAT code. USB exports ONE generated volume: LOGS/*.CSV,
      SCOPE/*.CSV (from DLG), SHOTS/*.BMP, INFO/ (today's LUN-1 files), DEBUG/SPIFLASH.BIN (raw image), with the
      media-change UA sent at once (not held while locked; existing files keep their clusters). Then: delete from
      the PC (recognise directory-entry 0xE5 writes) and dropping dm40*.bin for an update (DAPLink-style write
      recognition) as follow-ups. Removes "disk in use" refusals and host-side FAT corruption.
      docs/native-boot.md "Unit data".
      - [x] src/fatdir.rs: subdirectories in our FAT code (create with . / .., grow, rmdir, directory views
            for the writers, for_each over root + dirs); LOGS/ SHOTS/ SCOPE/ made at native boot (8 clusters =
            1002 slots each), DBG:BOOT? FATDIR. Writers -> directories, numbering over root + directory;
            files of earlier builds STAY in the root (no boot-time move: two-live-entries / lost-file window),
            every reader lists both. Host: create / grow / power cut before every write, fsck after each.
      - [x] src/vdisk.rs: ONE read-only LUN "DM40 files", 1.98 GiB FAT16 / 32 KiB clusters: README.TXT,
            INFO/ (the old LUN-1 files), DEBUG/SPIFLASH.BIN, LOGS/ SCOPE/ (CSV from DLG), SHOTS/ (BMP
            streamed); table of 400 files (SRAM): existing files keep slot + clusters, append-only, compaction
            only when out of space / after a bus reset AND unlocked; UA at once also while locked; refresh 1 s
            after the last change. Host test: sparse dump, fsck, mtools, CSV == dlog.py, BMP == SPI file,
            SPIFLASH.BIN == the SPI image, places kept across refreshes, compaction. Emulator (USB/IP): luns,
            mscdump (fsck clean, files equal their sources), screenshot under PREVENT -> UA after 4 s, new
            SHOTS/S003.BMP appended, the rest in place.
      - [x] "Disk in use by PC" refusals removed (screenshot, scope record, recording start / commits, gallery
            and MMEM delete); MMEM:CAT? lists "LOGS/LOG000.DLG", names accept a LOGS/ SHOTS/ SCOPE/ prefix.
      - [x] Firmware update without the writable disk: DBG:UPLOAD BEGIN / DATA (raw binary, before the SCPI
            parser) / END? / ABORT (src/upload.rs: DM40FW.BIN in the SPI root, other dm40*.bin removed, CRC
            re-read, header checked, data -> FAT -> directory); tools/dm40dbg.py upload; tools/reload.sh uses it
            (falls back to the USB disk for an older opendm40); emu/usbip_client.py upload (emulator: 480 KB in
            7.9 s). Recovery path (the loader's own disk) untouched.
      - [ ] Device: (the default "Files live" volume, decision #6) Linux kernel attach + udisks ro mount of the 2 GiB volume; a recording / screenshot while
            mounted -> new file visible after F5 (UA at once); Windows Explorer and macOS Finder with the UA
            while mounted; `fsck.fat -n` on a copied DEBUG/SPIFLASH.BIN; first boot after the update: FATDIR
            0x7 created 0x7 (~1 s extra boot, once), boot time of vdisk::boot_refresh with many logs;
            reload.sh end-to-end (upload ~10-20 s?, DBG:UPDATE, "installed OK"); MMEM:CAT? / DATA? / DEL with
            directories (dm40scpi.py get LOGS/LOG000.CSV).
            If Explorer does not show new files after the UA: try a volume serial that changes with every
            refresh (Windows may key "same medium" on it).
            Device d8e249d (docs/device-check-2026-10-21.md "d8e249d" 3a, Linux): PASS: udisks ro mount, a device screenshot listed ~9 s after the
            gesture, a recording at its stop (== MMEM:DATA?); fsck of SPIFLASH.BIN clean (dirty bit only).
            A host read in flight at the UA gets EIO (reopen works). Windows / macOS, reload.sh not run.
            Device c92b605, Windows 11 VM (device-check-2026-10-21.md "c92b605: ... Windows 11", read-only checks):
            PASS: AutoPlay -> E: "DM40", FAT16 (Get-Partition), 2 129 920 000 B, read-only, 41 files; CSV / INFO
            open; copies exact (SPIFLASH.BIN ~348 KB/s, SCOP000.CSV ~78 KB/s, FIRMWARE.BIN ~834 KB/s); Eject ->
            "Safe To Remove" (then remounted, item below). Still open: the UA while mounted on Windows (a new file
            + F5), macOS, reload.sh.
            Device 2f56ff9 (docs/device-check-2026-10-21.md "2f56ff9: screenshot preparation, the 76.8 kS/s nominal" item 3, Linux): PASS: kernel attach (1.98 GiB, Write
            Protect on), udisks ro mount, a screenshot listed 3.3-5.9 s after the gesture and a recording 2.8 s after
            its stop (both == MMEM:DATA?), FATDIR 7,0,0, MMEM:CAT? / DATA? / DEL with LOGS/ SHOTS/ prefixes. fsck of
            SPIFLASH.BIN: clean when read with the page cache dropped; a plain `cp` returned stale FAT pages (Linux
            keeps the unchanged-size file's cache across the UA; new item below). Not run: FATDIR first boot
            (created 0x7) and boot_refresh time (need an install), reload.sh, Windows UA + F5, macOS (untested).
      - [x] Linux: DEBUG/SPIFLASH.BIN read through the page cache can be stale after a refresh (device 2f56ff9,
            docs/device-check-2026-10-21.md "2f56ff9: screenshot preparation, the 76.8 kS/s nominal" note 2): its size and entry never change, so the kernel keeps
            its cached pages across the UNIT ATTENTION; a copy after a shot had the new BMP's directory entry but
            not its FAT chain. No firmware change: by the kernel sources (master, 2026-10-24; inferred, not tested)
            `disk_check_media_change` only marks a partition rescan, a positive vfat dentry is always valid
            (`vfat_revalidate`) and `fat_build_inode` returns the cached inode by its entry position, so a new
            date, size or slot of the entry does not drop the pages while mounted. Workaround (fadvise DONTNEED,
            drop_caches, remount; `dd iflag=direct` untested) in docs/manual/README.md "Copying
            DEBUG/SPIFLASH.BIN on Linux". Device: not rechecked.
      - [x] 76.8 kS/s: two nominals for one rung (device 2f56ff9): LOG:PRE? / the CSV and DLG headers said
            76 799.016 (`Logger::nominal_mhz` = 1e12 / the rounded 13 021 ns period), ACQ:SRAT? / the Wave view
            76 800 (`ad2_rate_mhz(0)`), 12.8 ppm apart. Fixed: `Logger::plan` keeps the chip rate
            (`hy::ad2_real_rate_mhz` / `hy::ad1_rate_mhz`, the stream ISR's), so LOG:PRE? nominal_mhz is
            76 800 000, which the CSV / DLG headers print (no header generated in a test; dlog-test /
            logfmt-test fixtures keep their own period-derived rates). AD1 rungs likewise (640.000, not
            639.795 from 1563 us). meter-test `nominal_is_the_chip_rate`. Device 1a11eda: LOG:PRE? nominal_mhz 76 800 000, CSV
            `# nominal_sps,76800.000` (device-check doc "1a11eda: bug-fix recheck" 4). The DLG header on the disk not rechecked.
      - [x] `nr3` (scpi.rs) printed 76 800 as `+7.680001E+04` (repeated f32 division by 10). Fixed: one
            scaling in f64; scpi-test `nr3_exact` (exact values, a 200 000-value sweep vs `{:.6E}`).
            Device 1a11eda: ACQ:SRAT? in the Wave view +7.680000E+04 / +3.840000E+04 / +4.800000E+03, scope
            +1.250000E+07 (device-check doc "1a11eda: bug-fix recheck" 5).
      - [x] `pow10f` (scpi.rs) divides an f32 by 10 once per step, so VOLT:RES? on DC V 6 V MED answers
            `+9.999999E-06` (device 1a11eda; the exact nr3 now prints the f32 it gets; CONF? on the same
            range says +1.000000E-05). Fixed: a table of the nearest f32 for 10^-16 .. 10^8 (its other user,
            the RES set threshold, gets the same values). scpi-test `pow10f_exact` (fails before: 57 checks,
            e.g. RES of 6 V fine +9.999999E-06). Device 0278d6e: VOLT:RES? on DC V 6 V MED +1.000000E-05, the other
            V / RES / CURR ranges exact powers of ten too (device-check doc "0278d6e" 5).
            CONF? on that range answers +1.000000E-04 for 3.0-3.5 s after a range set, then +1.000000E-05: it
            takes the live readout's digits, RES? the rate (doc "0278d6e" note 2). Cosmetic; left as is.
      - [x] MMEM names: the LOGS/ SHOTS/ SCOPE/ prefix was dropped, not checked (`MMEM:DATA? "SHOTS/LOG005.DLG"`
            returned LOGS/LOG005.DLG; device 2f56ff9). Fixed: `csvlog::parse_path`; a prefix of another directory
            is -256 for MMEM:DATA? and MMEM:DELete (nothing deleted). fatren-test `names`, scpi-test
            `mmem_dirs`, emu/shottest.py (LOGS/Snnn.BMP). Device 1a11eda: `"SHOTS/` / `"SCOPE/FAST000.DLG"`
            -256 for DATA? and DEL, the file still listed; `LOGS/` and no prefix read it (device-check doc "1a11eda: bug-fix recheck" 5).
            `parse_path` was in .cold (0b9d580), so MMEM:DATA? during an AD2 stream ran region C (wavetest
            "a host reading SCOPE/SCOP000.CSV"); back in region B since fix/regressions-1025 (item below).
      - [x] Windows Eject does not keep the volume ejected (device c92b605): the shell's Eject sends START STOP
            UNIT, and Windows shows "Safe To Remove". msc.rs then only records it (STOP_UNIT, msc.rs:585, used by
            recovery), so the medium is still present and E: is mounted again at the next access. Harmless on the
            read-only volume. Option: report NOT READY / 3A (as DBG:VDISK:EJECT does) until the next bus reset or
            a new medium (refresh), so the drive stays ejected. Check first that a Linux `eject` / udisks
            power-off and the "PC can edit" commit at the eject still work.
            Done (d64e1f9; native-boot.md "SCSI" Eject): LoEj=1 Start=0 -> 02/3A/00 for everything but INQUIRY /
            REQUEST SENSE / START STOP / PREVENT until a bus reset / unconfigure or a host load (LoEj=1 Start=1:
            one UA 06/28/00); plain STOP / START and power conditions ignored; any 0x1B still sets STOP_UNIT
            (recovery-usb unchanged). DBG:VDISK:EJECT? = ejected, 0x1B count, last CB[4]. Found on the way and
            fixed: `cp dm40fw.bin` + eject at once on Linux lost the install (the Allow's `release` dropped the
            staged file before its SETTLE_MS check; emulator kernel path, WR? fw_ok 0); the session end now waits
            for the check (END_WAIT_MS 1.5 s, frozen meanwhile; fatren-test vwrite "Allow at once").
            Emulator (d64e1f9): mscresettest 9/9 cases PASS (stop-start, eject-load: TUR / READ / READ CAPACITY /
            READ FORMAT CAPACITIES 02/3A/00, INQUIRY answers, kept over a Bulk-Only reset, load -> one UA;
            eject-port: back after a port reset); emu/ejecttest.py PASS (Linux usb-storage + udisks over vhci_hcd:
            `gio mount -e` -> ejected, size 0, MediaAvailable false, no re-mount, `udisksctl mount` fails;
            re-plug -> back, README equal; `udisksctl power-off` sends 1B/00 = not an eject, the USB device
            removed, re-plug -> back; PC can edit: cp of the image + `gio mount -e` at once -> committed, the meter
            asks, medium ejected, re-plug -> back and still writable); fatren-test, all tools/*/run.sh PASS.
            Note: udisks merges two disks with the same vendor / model / serial ("ALIENTEK DM40 files" 0001) into
            one multipath drive, so two DM40s (or the emulator next to the real meter) share one udisks drive
            and a drive-level Eject / PowerOff may act on the other one; ejecttest.py rewrote the emulator's
            serial in its proxy. Done: a per-unit USB serial from the chip UID (item below).
            Device retest pending: Linux (gio / Nautilus Eject -> stays ejected, re-plug -> back; PC can edit +
            cp + Eject at once -> Install? dialog); Windows 11 VM (Explorer Eject -> E: stays without medium,
            re-plug -> back).
            **Device 469f354 (Linux, device-check "469f354" C):** udisks Drive.Eject (D-Bus) -> 02/3A/00:
            EJECT? 1,2,2, size 0, MediaAvailable false, mount refused, still ejected after 60 s, USBTMC fine; a port
            reset (USBDEVFS_RESET, the re-plug stand-in) -> back, auto-mounted ro, README equal. **`gio mount -e`
            (= Files / Nautilus Eject) does NOT eject on this host:** it unmounts, then gvfs runs util-linux
            `eject` as the user -> "cannot open /dev/sda: Permission denied" (root:disk 0660); the meter never sees
            a 0x1B (EJECT? 0,0,0). Host side, not opendm40 (ejecttest.py passed through its Drive.Eject fallback).
            Not run: PC can edit + cp + Eject (owner setting + disk write), host load (eject -t needs /dev/sda),
            Windows.
      - [x] Windows: the USBTMC interface (MI_00, class FE/03/01) has no in-box driver. It shows as "DM40
            multimeter (opendm40)" under Other devices with code 28 (device c92b605). The MSC interface works
            regardless. Document it in README / Help: for SCPI on Windows, use NI-VISA / Keysight IO Libraries
            (USBTMC), or WinUSB via Zadig + pyvisa-py. Done (d64e1f9): README "USB on a PC", docs/bt-scpi.md
            "USBTMC on Windows" (not tried on Windows yet; Help text not changed).
      - [x] Per-unit serial number (2026-10-23; src/uid.rs, docs/native-boot.md "USB serial number"): the
            GD32 96-bit UID (0x1FFF7A10) as 24 uppercase hex digits in memory order (= `xxd -p -u
            INFO/UID.BIN`), read once in native::start (system memory, not the slow bank; a TCM copy), is the
            USB iSerial (normal and recovery-usb; was "0001" / "REC1"), the SCSI unit serial (INQUIRY VPD 0x80;
            new: VPD 0x00 lists 00 / 80, other pages 05/24/00; an EVPD request used to get the standard
            INQUIRY data) and the *IDN? serial field (was "0"; SYST:IDN:EMUL 34401A keeps "0"). libsigrok
            (scpi-dmm: 4 comma fields, serial only stored -> [S/N: ..]) and ngscopeclient (SCPIDevice
            `%127[^,]`) take any comma-free serial. Privacy note in the doc: readable by any USB host / BT
            SCPI peer; UID.BIN and the FAT volume ID already exposed it; a hash would be as trackable.
            Emulator: run.py --uid (default emu_uid: the unit's UID with bytes 4 / 9 = the USB/IP port; the
            stock genuine check never reads them), so each instance has its own serial; ejecttest.py no longer
            rewrites the serial, its safety check requires the emulator's serial (not the unit's) in the
            udisks drive and no other disk on it; mscresettest.py case `serial`; usbtmc_stress.py takes the
            serial from the first *IDN?.
            Emulator (24d49d7): mscresettest 10/10 cases PASS (serial: iSerial == VPD 0x80 == *IDN? field ==
            emu_uid hex, VPD 0x00 = 00 80, VPD 0x83 -> 05/24/00); ejecttest PASS next to the real meter (meter
            still "0001": udisks drives ALIENTEK_DM40_files_0001 = sda and ALIENTEK_DM40_files_<emu hex> = sdb,
            /dev/disk/by-id/usb-ALIENTEK_DM40_files_<hex>-0:0; the meter's sda untouched and mounted); sigrok-cli
            (patched libsigrok, tcp-raw): "Alientek DM40A dm40-fw 0.1 [S/N: <hex>]", samples read; usbtmc_stress
            pair / combo / split PASS (its big / compound cases fail on WAV:DATA? sizes: stale since before this
            change, not looked into); all tools/*/run.sh PASS. Budgets: region B 48 156 B free (-216), C 23 696
            (-148), stack headroom 21 344 (-24).
            Device retest pending: `lsusb -v -d 1209:0001` iSerial == `xxd -p -u INFO/UID.BIN` == *IDN? field
            (USBTMC and BT); udisks drive / by-id name carries it, Eject / Power Off still act on the meter;
            [device 469f354: iSerial = UID.BIN = *IDN? field 3 over USBTMC and BT, udisks drive / by-id
            ALIENTEK_DM40_files_<UID>, Drive.Eject acts on the meter, sigrok-cli -d scpi-dmm:conn=usbtmc/3.10
            --scan "[S/N: <UID>]", DBG:FLASH? OK: PASS. Power Off, ngscopeclient, Windows, ejecttest: not run]
            `sigrok-cli -d scpi-dmm:conn=usbtmc/...  --scan` [S/N: ..]; ngscopeclient connects; recovery-usb is
            NOT flashed (test build only); Windows 11 VM: a new device instance (re-install, a Zadig WinUSB
            binding must be redone), Explorer mount + Eject still work, `Get-PhysicalDisk` SerialNumber (VPD 0x80);
            reload.sh / DBG:UPDATE flow unchanged (by VID:PID); with the meter on this build attached, run
            emu/ejecttest.py: check_drive passes, two distinct ALIENTEK_DM40_files_<hex> drives, the meter's
            disk untouched (the emulator run above had the meter still on "0001").
      - [x] recovery-usb build broken (recovery.rs called the 3-argument Msc::new; then its 4 KiB MSC_BLK in
            TCM failed the 42 KiB TCM stack assert): fixed (MSC_BLK in .sram_bss when the SPI disk is the
            LUN). tools/build-variants/run.sh builds recovery-usb, recovery-file, no-ad2-isr, spi-disk (no
            vdisk), fatren-dry-run and keep-screen with all build.sh checks into target/variants/ (build.sh
            DM40_TDIR / DM40_OUT; opendm40.bin untouched), ~2 min; part of the tools/*/run.sh suite.
            (24d49d7: all six ok; recovery-usb stack headroom 21 856, B 46 780, C 23 004 free.)
      - [x] Follow-ups (2026-10-22, src/vwrite.rs; docs/native-boot.md "Host writes"): the volume is
            writable (no WP bit; DBG:VDISK:RO 1 = the read-only volume as before). Every 512-byte WRITE is
            diffed against the generated sector: a LOGS / SCOPE / SHOTS delete (entry 0xE5 + chain freed ->
            the SPI file deleted; a move to the trash = deleted at the eject), a DM40*.BIN in the root
            (staged in free SPI clusters by content, any write order; checked as the loader does, linked as
            DM40FW.BIN, dialog "Firmware received / Install it now?" Yes = DBG:UPDATE, No / 60 s =
            deleted), the hosts' own trash / indexer / Finder files (kept for the session in hidden
            _VHOSTnn.TMP files, deleted at the eject / next boot), dirty flags and dates; everything else
            DATA PROTECT. Layout frozen while the host has it mounted (new recordings / screenshots appear
            at the eject / re-plug: the cost of writability; a UNIT ATTENTION would make Linux drop writes).
            decided: see ../docs/history/decisions-2026-10-22.md #6 (read-only "Files live" by default; Settings 3/3
            "USB disk" = "PC can edit" -> the writable volume, volatile, applied at the next USB plug)
            IMPLEMENTED 2026-10-22: vwrite.rs mode in force / asked for (applied at the bus reset), Settings 3/3
            row + toast "Re-plug USB to apply", Help 6/6 "USB disk", README.TXT, DBG:VDISK:RO? = in force,
            asked for; fatren-test vwrite "mode" + "rw-ro" scenarios (captures unchanged, they run writable).
            Emulator + kernel attach: RO mount, LOG:CSV while mounted -> listed at once; row tapped, re-attach
            -> Write Protect off, rm 2 files -> deleted on the SPI disk, cp dm40fw.bin -> install dialog, No.
            Linux sequences captured with a real kernel (usbip_msc.py: rm, cp of 500 KB, trash + cp,
            trash + empty) and replayed in run.sh; fsck after every scenario, power cuts in staging,
            commit, park, purge. Emulator (real firmware, kernel attach): rm 2 files, cp 500 KB (47 s in
            the emulator), dialog Yes -> reset into the loader.
      - [ ] Device: (in "PC can edit" after a re-plug, decision #6) Linux rm / Nautilus Delete (trash) / cp dm40fw.bin + Yes -> installed + No ->
            DM40FW.BIN gone; DBG:VDISK:WR? after each; DBG:BOOT? STACK after a cp; SPIFLASH.BIN fsck; DMM
            and a CSV recording during a cp (staging waits / busy retries); Windows Explorer (System
            Volume Information, delete, copy) and macOS Finder (.fseventsd, .Trashes, ._ files): untested,
            expected per docs/native-boot.md "Host OS notes".
            Device d8e249d (docs/device-check-2026-10-21.md "d8e249d" 3b, Linux): PASS for rm (deleted at once), gio trash (deleted at the
            eject), cp dm40fw.bin (601 KB in 29.8 s) -> dialog -> No -> no dm40*.bin on the SPI disk, WR?
            refused 0, stack 23 376 B, fsck clean, Files live restored. Re-plug without the cable: `usbreset
            1209:0001` (the bus reset applies the mode) + DBG:VDISK:EJECT 3 (the kernel re-reads WP only at a
            medium change). Yes (install), the DMM / a recording during a cp, Windows / macOS not run.
- [x] A-jack protection hybrid (owner 2026-10-22): non-current modes keep the stock resting shunt state (PE0 = PE1 = 0,
      ~48 ohm A-COM: sensitive A-JACK from ~0.44 mA); when the sentinel measures a current that would overheat the
      47.5 ohm resistor (owner: package looks like 2512 / ~2506 -> assume 1 W, derate to 0.5 W continuous: > ~100 mA for
      > 0.5 s, or > ~300 mA (4.3 W) at once) switch to the 10 mohm shunt (PE0 = 1): a weak source then flows harmlessly,
      a stiff one blows the 10 A fuse instead of burning the resistor / MOSFETs (30 V parts). Latch with a red alert +
      beep until the user picks a function/range and the current is gone; measure the current on the 10 mohm path
      while latched (AD1/AD2 on PB2) to know when it is safe to return. >30 V on A-COM (mains) cannot be protected by
      switching (MOSFET breakdown first); document. Doc: power-meter-and-misconnect.md section 4 (update the old
      "do not auto-switch MOSFETs in V modes" note).
      Done (doc §4.8): the probe (OP2 x1 on PB2 = the 10 mohm element, AD2 peak, 4 194 codes / A) enters the
      DC V / mV / ohm / temperature row at the first comparator hit; >= 300 mA at once or >= 125 mA for 400 ms
      from the first hit (~0.5 s from the onset; 125 mA keeps 5 V = 104 mA a warning) -> PE0 = 1, red A-PROT
      (badge / toast / beep, SYST:ALER? APROT, QUES bit 9), latched until the user acted AND the probe read
      < 50 mA for 1 s; a current function / the power meter ends it. Gaps: > 30 V (MOSFETs), Jack alerts OFF,
      AC V / Hz / cont / diode / cap (no free pair). Emulator: PB2 node + AD2 probe; senttest aprot-* (12 V:
      409 ms, 24 V: 105 ms, ohm / temp ~0.41 s, 5 V: A-JACK only).
      (verify on device: the A-jack protection check list of doc §4.8)
      - [x] Device d8e249d (docs/device-check-2026-10-21.md "d8e249d" alerts): **A-PROT found latched with the
            A jack open** at the end of the run (DBG:SENT? field 11 = 105 ms: the ">= 300 mA at once" rule
            in the first probe window; probe 15 mA afterwards; CONF:VOLT:DC over SCPI did not release it,
            DC A did). When / which function is unknown (DC V 1 / 5 / 10 V and the square on the DMM page,
            combined view and Scope page did not trip it again). Log the trip (function, probe mA, time).
            Fixed: the peak hold caught AD2's start-up after the probe's row write; now cleared 30 ms after
            every probe row write, the first window from there, and a trip needs the comparator run going
            (senttest aprot-glitch with the model's Ad2StartGlitch: 76b79e0 A-PROT, fix A-JACK only).
            Still open: log the trip's function / time.
            Device 22c85ba: A-PROT again in TEMP with the square on V-COM: in ohm / temperature the probe
            follows the V input (~0.34 A per volt; 5 V DC read 1.7 A). Fixed: the probe in DC V / mV only,
            ohm / temperature A-JACK only (doc 4.8).
      - [x] False A-JACK in ohm / temperature with a voltage on V-COM (5 / 10 V DC, the square; A jack open):
            the comparator on PB1 hits there; LIVE? is the right alert (device 22c85ba).
            **Fixed by detection, not documented as a gap (073b309, doc 4.8 "V_GATE").** Why not a gap as in
            Hz: the comparator alone cannot tell (two bits per window; the pickup grows with the voltage, so
            no threshold separates it from a current), but unlike Hz these rows measure V-COM themselves:
            ohm's AD1 code goes negative for any source above the ohm source's ~1.4 V or below COM (LIVE?'s
            sign), the thermocouple input reads > 80 mV (`T_VDRV_MV`; type E 76.4 mV at 1000 C, an open TC
            read ~212 C = ~7.6 mV on the device), RTD / NTC use the ohm sign. The case A-JACK is for (leads in
            A / COM, V-COM open) shows neither. So in ohm / temperature A-JACK asserts only after a V-side
            result since the hit run began said "free" (ohm: next conversion; TC: up to ~2.2 s), and drops
            when it says "driven"; ohm then shows LIVE?. DBG:SENT? field 12: -1 / 0 free / 1 driven.
            Emulator: the model's `Pb1VjackMv` (PB1 picks up the V jack in those rows, 10 mV / V INFERRED:
            1 V under and 5 V over the 23.75 mV threshold, as the device); senttest `ohm-v5` / `temp-v5`
            (5 V, 10 V, the square, then 2 / 5 mA in A with V-COM open): 4ed771d A-JACK from fw 10 s, fix
            NONE / LIVE? and A-JACK in the last phase. Residual gaps (doc): a source between the pickup
            threshold and ~1.4 V in ohm if the real k is > ~17 mV / V; a current in A together with a source
            on V-COM (LIVE? is shown, not A-JACK); an open TC input reading > 80 mV would hide A-JACK in TC.
            Device retest: list below (item 5 also bounds the pickup k).
      - [x] **False A-JACK in Hz** (A jack open, reproducible): FREQ with 1 V DC or the 40 Hz square on V-COM
            -> SYST:ALER? AJACK (kind AD1PK_PB1), DMM page and combined view. Also an "AC" hint in DC V on
            the calibrator's 5 V DC (not at 1 / 10 V).
            Cause: the frequency row leaves its divider open, PB1 - PB3 stands at ~54 mV (+41 100 codes) with
            0..2 V or nothing on V-COM, ~3 100 at 5 V, -850 at 10 V. Fixed: no sentinel in Hz (doc 4.7 gap;
            senttest freq-gap). Open: the AC hint in DC V on 5 V DC.
      - [x] **False alerts after DMM function switches (owner, device 469f354, device-check "469f354" E; raw log
            in that check's scratchpad `chk/out/E_all_sent.txt`).** 397 switches (42-pair matrix, chain, AUTO, UI
            taps, scope / Wave view -> DMM) with V-COM open, 1 V and 5 V DC; A jack open; A-PROT never.
            (E1) **V-JACK? after RES -> A or RES -> mA**, 11 / 11 with V-COM open or 1 V (0 / 4 at 5 V): CMP_SENSE
            ctsta 0x70 in every window from 0.46-0.73 s (after the frozen row change), V-JACK at streak 10-11
            (1.47-1.97 s), 0x50 at ~4.3 s, 0 at ~4.5 s, off 4.9-5.5 s. Likely the V input still charged by the
            ohm source. (E2) V-JACK? after scope page -> DMM in A at 5 V (on 1.42 s, off 6.0 s, ctsta 0x70 from
            0.51 s); direct TEMP -> A at 5 V: none. (E3) **AC? in DC V on 5 V DC** (the item above), 26 / 26 entries,
            on at 2.6 s, stays on: the V-tap window's CTSTA is 0x70 = CMPHO and CMPLO both set on a steady +5 V
            (1 V: 0x10), and sample_cmp reads both bits as "swings both ways". (E4) A-JACK in RES at 5 V, 12 / 12,
            vside 0: the known 519fb8a 7b, owner decision. DC? in AC V on 1 / 5 V DC is right. **Fixed by the
            jack check (doc 4.13, branch fix/ajack-false): LIVE? instead; device retest pending.**
            **Device d330877 (device-check-2026-10-21.md "d330877" section 4, raw `chk2/out/E_e1_*`, `E_e4_*`, `ctsta_summary.txt`):** E1 still
            there (RES -> A / mA: 0x70 0.47-0.67 -> 4.6 s open, -> 5.1 s at 1 V; V-JACK 1.5-1.8 -> 5.2-5.9 s).
            RES -> DC V 1 s -> A: ctsta 0, no alert (open and 1 V, 3 / 3 each). RES -> AC A 2 s -> A: 0 (open),
            0x70 only 0.65-1.0 s (1 V), no alert. A real 24 V on V-COM reads 0x70 in DC A entered from TEMP / AC V
            (V-JACK at 1.7 s) but **0 for 10 s when entered from DC V** (10 V: 0 from DC V, 0x70 only 0.66 s from
            TEMP): the misconnect hint depends on the previous function. Never 0x20 / 0x10 in DC A.
            **New: V-JACK? in every AC A range with V-COM open** (AD1PK_SENSE, pk -262144, from 1.66 s, still on
            at 30 s). At 24 V DC in AC V a short A-JACK (3.1-4.1 s).
            **Device 40728b8 (doc "40728b8"):** E1 / E2 / E3 gone. 2a V-JACK? in AC A unchanged (every range, all
            levels, from 1.68-1.84 s; AD1 pk ramps -117 000 -> -262 144 over 0.7-2.0 s). DC A from DC V at 24 V:
            still ctsta 0 (also through AC V; also after RES and after the Scope round trip). The short A-JACK in
            AC V at 24 V did not come back (5 entries). VOLT:RANG:AUTO? in AC V / CURR:RANG:AUTO? in AC A: still
            +9.91E37 + "Settings conflict"; VOLT:AC:RANG:AUTO? / CURR:AC:RANG:AUTO? answer (owner: is the DC-only
            plain form wanted?).
      - [x] Switch latency (owner: "several seconds, stock was instant"; device 469f354, settle.py at 1 / 5 V,
            RATE FAST): first valid FETCh? DC V <-> AC V 0.46 s, RES -> DC V 0.52-0.56 s, DC V -> mA 0.56 s,
            DC V -> A 0.75-0.76 s, 6 V -> 60 V 0.55-0.56 s, AC V -> AC+DC 1.32-1.37 s, **DC V -> RES 2.67 s**; the
            screen is blank until then. AC V of a DC input settles into 0.1 % in 2.5-2.9 s. Stock not measured.
            **Done on branch fastswitch (../docs/history/switch-latency.md 6, owner-approved proposals 1-6; 7-8 wait):**
            the new function's frame at once (unit + grey dashes, never 0.0000); ohm entry 150 ms (no 2.5 s
            deadline; the stock's single-step window works again); V / A entry 300 ms; manual V range 250 ms;
            no held first conversion where a function change restarted AD1 (R22 changed); provisional values
            (vet hold, AC+DC before both parts / AC settling, AUTO deciding) drawn grey, never to the meter,
            autorange, BT or SCPI. In AUTO, FETCh? is +9.91E37 until the first decision; READ? / MEAS?
            after the host's change wait for it (main.rs AUTO_WAIT_MS: until 3.9 s after the entry, a later
            query 0.85 s; then the part AUTO shows, never "Data stale"; emulator autowaittest: MEAS? DC
            1.28 s, clear AC 3.46 s, a tie 3.91 s; libsigrok's READ? 3 s after CONF 0.42 / 0.85 s;
            docs/bt-scpi.md). Emulator (switchlat, MED):
            DC V -> ohm 3.08 -> 1.19 s (FAST 2.83 -> 0.93), DC V -> A 0.72 -> 0.33, ohm / A -> DC V 0.53 ->
            0.33, DC V -> AC V 0.48 -> 0.32, manual 6 <-> 60 V 0.5 -> 0.3, AC+DC 60 -> 6 V 1.69 -> 1.50;
            AC+DC -> AUTO first FETCh? 1.29 -> 3.56 (by design). Suites: switchtest (15 / 15), senttest (all
            in the final run; aprot-to-dca failed once in an earlier loaded run, passes alone), autorangetest
            (ad1mix 1), couptest, powtest (acdc-floor flaky under load, base too; bt: its last row was cut by
            the run's end, a test race fixed in powtest.py (emu: drop a row the run's end cut off), then OK
            alone), leadstest, acdcmethtest, dcvtest, flowtest, meter-test, touch-test, ui-render 0
            violations, autowaittest. Region C (final image, emu/regionc.py hooks): fastlogtest
            --region-c --host-read / --ui-nav at AD2 38400 and wavetest volt / current / dcv: no region C
            code or slow-bank read while streaming.
            **Harness timing under host load (not this branch):** the fw uptime's offset from the virtual
            time is bimodal under load (couptest idc: the 0.3 A input at fw 13.2 or 14.8 s for the same
            virtual 16 s), so a phase's check window can see the next phase's input: couptest idc / band /
            iac / sq5 and powtest bt phase 1 fail that way on base 6e9962f and on fastswitch alike (idc:
            base 1 of 2, fastswitch 2 of 2 at 13.2 s; band fails on base too); they passed on fastswitch at
            lower load. The tests' windows should key on an input marker, not on uptime: follow-up.
            **Done (branch harness-sync, emulator only):** the offset is not bimodal. It is a fixed 2.85 s
            (main's DWT CYCCNT write after pre_main's SPI-disk work on the fresh disk image: fatdir,
            calbackup, mirror_boot), identical in every run, loaded or not (virtual time is deterministic;
            4 parallel boot probes and 4 parallel idc runs: the 0.3 A input at fw 13.1504 s in all). The
            tests assumed 1.5-1.8 s, so their windows reached 0.35 s past the next input; the A AUTO
            coupling reads in bursts every ~1.9 s, and whether a burst fell in those 0.35 s decided pass /
            fail (13.2 = this burst, 14.8 = the next). Fix: the model logs a clock marker (DWT.cs: every
            CYCCNT write) and an input marker (HY3131 SetInput: virtual time + fw uptime), emu/emusync.py
            places phases, actions and windows on them (emu/README.md "Input markers"); couptest, powtest,
            senttest, burdentest, leadstest, acdcmethtest, autowaittest, switchlat, inrushtest, dcvtest,
            loudtest, switchtest converted. Under load (5 emulators at once, host load ~12-20): before
            idc 0/3, band 0/3, iac 0/3, sq5 0/3, powtest bt 2/3; after 3/3 each. couptest iac's AC 300 mA
            phase is 16 s now: AUTO walks 6 A -> 60 mA -> 600 mA there and settles 8.5-10.6 s after the
            change (watch on the device: a 0.3 A AC step down from 1 A in A AUTO).
            `fastlogtest --ui-nav --region-c` "MENU on Device -> main menu": not timing, not a firmware
            regression (6e9962f's build fails the same): be01de5 put "Danger alarm" into Settings 3/3, Device
            moved to row 5 and the tap at y 177 toggled Burden voltage. NAV taps y 207 now: PASS.
            **Device retest** (RATE FAST and MED, 1 V / 4.7 kohm / 50 mA as in 469f354, settle.py):
            1. DC V -> RES on 4.7 kohm: screen shows "- - Mohm" within ~0.2 s, the number by ~1.0-1.3 s (was
               3.0 s), no range stuck on 60 Mohm; an open input reads 0L, a 10 Mohm / 100 ohm resistor settles
               on its range (single steps for 2.5 s, then the stock's multi-range steps).
            2. DC V <-> DC A / mA on 50 mA, RES -> DC V, AC V -> DC V: first FETCh? ~0.35 s (was 0.5-0.75 s);
               the first A reading against the calibrator within spec (the A-jack relays now get 300 ms, not
               650: watch the first reading after DC V -> A for a relay transient).
            3. MF / SCPI VOLT:RANG 6 <-> 60 at 1 V: first FETCh? ~0.3 s; no 25-51 % or 0.78 V reading in 30
               switches (the vet still holds a disagreeing first conversion).
            4. VOLT:RANG:AUTO ON from 600 V and the 600 -> 60 -> 6 V two-step autorange (decaf60): never
               0.767-0.790 V (vet unchanged on autorange steps).
            5. AC+DC entry (1 V AC + 0.5 V DC): grey |DC| with "AC --" first, then 1.118 V orange; an AC+DC
               range step 60 -> 6 V: grey values while the AC part settles, READ? +9.91E37 until settled.
            6. AUTO coupling on 1 V AC + 0.5 V DC: grey DC part until the decision, then the AC part orange;
               READ? +9.91E37 until then. HOLD / REL during these: the frozen value stays, no grey over it.
            7. No false V-JACK? / A-JACK / AC? after RES -> A / mA and scope -> DMM (device check E): the
               sentinel blanks count from the switch, the readings now come ~0.2 s earlier.
            **Device 6bc494a (2026-10-24, device-check-2026-10-21.md "6bc494a: switch latency on the device";
            calibrator 1 / 5 V, 390 / 100 ohm, 10 / 40 Hz square; no 4.7 kohm, no current, no open input):**
            1. frame PASS (dashes <= 0.06 s), time PASS (V source 2672 -> 319 ms; 390 ohm right at 1.16-1.38 s
               FAST, 1.91 s MED), **values FAIL**: the ohm hunt shows / answers wrong values first (8.8 / 7.1 /
               12 Mohm, then 219 / 126 / 57 / 30 ohm, amber, FETC? / READ?; MED holds one ~0.8 s). Needs a real
               resistor to split meter vs simulator; fix idea: vet the first in-range conversion after an ohm
               range step.
            2. PASS: 0.31-0.40 s (40728b8 0.46-0.76 s); `CONF:CURR:DC 10` stays 0.76 s (manual 10 A row, 650 ms,
               by design; its placeholder shows "mA" for 0.8 s: cosmetic). A reads 0.000, no OVERLOAD.
            3. PASS (30 switches FAST and MED, 0.31-0.51 s, no 25-51 % / 0.767-0.790 V).
            4. PASS (8 x FAST and MED).
            5. PASS on 1 V DC (grey |DC| + "AC --" at 0.6 s, amber at 1.5 s); 40 Hz square grey through 3 s,
               FETC? +9.91E37 to 4.7 s. Observation: on the 10 Hz square an amber 5.36 V (READ? 5.32 V) on the
               6 V range before the overrange step.
            6. MEAS:VOLT:AUTO? PASS (1.28 / 3.45 / 3.91 s, never "Data stale"); grey PASS; HOLD after the
               decision PASS. **FAIL: FETCh? while AUTO decides blocks 1-3 s and answers the grey DC part, not
               +9.91E37** (main.rs Fetch -> scpi_wait -> auto_prov). **FAIL: RUN while AUTO decides** leaves
               "RUN" in the status bar and freezes the grey value and the decision until RUN again. REL cannot
               engage before the decision (no last_raw), by design.
            7. PASS for RES -> A / mA (70 switches, 1 / 5 V). A-JACK in RES at 5 V (known) now from 0.46 s.
               **New: 0.5 s V-JACK? at 8.06 s after A / mA -> Scope -> DMM at 5 V** (ctsta 0x70 tail to 7.75 s,
               past the 7 s cap; c785807 6.34 s).
            **Fixed on branch fastswitch2 (../docs/history/switch-latency.md 6.2; each with a test that fails on 6bc494a):**
            ohm hunt vet (a conversion is final only when the one before it on the same ohm row agrees within
            0.1 % + 8 counts; grey before; ohmhunt.py with the model's `ohmtau`), FETCh? +9.91E37 at once while
            AUTO decides (autowaittest ac-fetch / dc-fetch), HOLD before the first settled reading armed: HOLD
            label at once, holds the first decided value (ac-hold, meter-test), the RES / restart V-JACK? mask
            ends 1 s after the last 0x70 (max 12 s; switchtest ohmexit-long with a SCOPE step), the
            placeholder follows CONF's range (a10-unit / ma-unit). New `DBG:READout?`.
            Fuse check (owner on ea73ea5: 18 ohm instead of 48, "Check leads"): it now decides on the first
            final reading whose ohm vet saw two conversions agree on the 600 ohm row (8 s timeout kept);
            senttest fuse-ok-lag (ohmtau 800 ms): old FAIL,38.92, new OK,47.34.
            Suites on fastswitch2 rebased on 6bbff98 (sentinel3 + tidy): ohmhunt (10), autowaittest (13),
            switchtest all 22 OK (real-vjack-blind now passes with the PRECOND default; ohmexit-long OK),
            senttest 63 OK (incl. fuse-ok-lag), loudtest 7, freqtest 6, meter-test 102, ui-render 0
            violations, couptest 10 / 10 and powtest 30 / 30 on both base 6bbff98 and fastswitch2 side by side
            (powtest acdc-floor failed once under load on fastswitch2, OK alone; known flaky), fastlogtest
            --region-c --host-read ad2:38400: no region C code or slow-bank read while streaming. couptest
            sqtie / band tie checks accept +9.91E37 while a tie is undecided (the FETCh? rule).
            Device retest step 6: the fuse check (lead V/ohm -> A) 10 times: always FUSE OK 44..54 ohm.
            **Device retest of fastswitch2** (calibrator 390 / 100 ohm and, if at hand, real 100 ohm / 4.7 kohm /
            10 Mohm resistors; RATE FAST and MED; settle.py / shots2.py / extra.py of chk6):
            1. DC V -> RES on each resistor: no amber value and no FETC? / READ? answer off the final one by more
               than 0.1 % (the 8.8 / 7.1 / 12 Mohm and 219 / 126 / 57 / 30 ohm of 6bc494a may still appear, but
               grey); `CONF:RES;:READ?` answers the right value (MED: ~2-2.5 s, never "Data stale"); first right
               value expected ~0.1-0.4 s later than 6bc494a's (one vet conversion). Real resistors vs the
               calibrator: whether the wrong values come from the meter or from the simulator's lag (log
               DBG:READout? every 20 ms: grey values and their times).
               10 Mohm (or open) at FAST: the 60 Mohm range's value turns amber at most ~1.5 s after the
               hunt's last range step (OHM_VET_MAX_MS), never stuck grey; same on a live source (LIVE?).
            2. `VOLT:COUP AUTO` on the 10 / 40 Hz square, FETC? every 0.1 s: +9.91E37 within ~10 ms each until the
               decision, then the value; MEAS:VOLT:AUTO? / READ? as before (1.28 / 3.45 / 3.91 s).
            3. RUN at 1 s after `VOLT:COUP AUTO` (10 Hz square): "HOLD" in the status bar at once, the grey value
               keeps moving, the decided value (amber) appears and then stays frozen; RUN again releases it.
               DBG:READout? shows G,..,1 then F,..,1.
            4. A / mA -> Scope -> DMM with 5 V on V-COM, 10 times (seqE): no V-JACK? (6bc494a: 0.5 s at 8.06 s);
               DBG:SENT? ctsta: note the end of the 0x70 tail (7.75 s on 6bc494a). A real V-JACK? source (24 V on
               V in DC A after RES): V-JACK? within ~2 s if it reads CMPHO alone, else by ~13 s.
            5. `CONF:CURR:DC 10` from DC V: the placeholder shows "A" from the first frame (shots2.py).
            **Device v0.1-pre1 / grab-power, 2026-10-24 (device-check-2026-10-21.md "v0.1-pre1"):** 1 **FAIL**
            (6 of 12: the right value grey, then two agreeing ~32 % conversions after the in-range OSR
            write, final in amber: 125.9 / 122.1 / 57.3 / 30.0 ohm, 134 kohm for 420 kohm), 2 PASS (+9.91E37
            in <= 31 ms; the decision came at 5.76-5.85 s, once not within 6 s), 3 PASS, 4 PASS (no V-JACK?,
            no 0x70 at all), 5 PASS, fuse check on open jacks **FAIL** (OPEN 1, FAIL,279-286 ohm 9 of 10:
            the same ~32 % pair after the 0x13 write). Fixed in "First readings after a switch" below.
- [x] Burden voltage in the A / mA / uA modes (as the EEVblog 121GW, owner 2026-10-22): I * R_path of the
      shunt in use in the aux row, "Burden" caption: DC A aux1 (signed), AC A aux2 instead of the duty
      (RMS); none in AC+DC (both rows hold the parts) and in the power meter (its own At-load line). R_path
      = the power meter's per-shunt setting (default the bare shunt = a lower bound), POW:BURDen now sets
      the shunt of the DMM A function in use. Settings 3/3 "Burden voltage" (persisted p[38] bit 14,
      default OFF = stock look), DISPlay:BURDen ON|OFF / ?, FETCh:BURDen? (V, pre-REL, works with the
      readout off). Not sent over BT. Emulator emu/hy3131/burdentest.py, ui-render d21..d24 / c16..c18
      (docs/power-meter-and-misconnect.md 3.15). Device 33b44bd (no current): the setting, its reboot
      persistence, DISP:BURD, FETC:BURD? (DC V 9.9E37, uA 47.5 ohm x I), the aux rows PASS.
      Left: the device check (3.15 list: a second meter across
      A-COM per range, then POW:BURD the measured R_path); the guided "measure R_path" step is not built
      (own ohm function reaches only the uA chain: the fuse check value could seed it later).
- [x] Inrush current (as the Fluke 376 / 323 clamps, owner 2026-10-22): arm in the 10 A range (autorange
      off), wait for the current to step up from ~0 past a threshold, then show the RMS over the first 100 ms
      after the trigger (Fluke's window) plus the peak, and keep the captured trace (a short chart / scope-like
      view, saved like the FAST*.DLG records). Data path: AD2 on the 10 mohm shunt at x1 (+-125 A window,
      dmm-input-scope-feasibility.md 1.3) at the fast logger's 9.6 kS/s (src/fastlog.rs; ~960 samples in
      100 ms), with a pre-trigger ring. Open: the trigger rule (absolute A vs a step from the idle level), AC
      and DC loads (RMS of the raw samples covers both), what the 10 A fuse tolerates (rating / type UNKNOWN:
      a motor start can be 5-10x the run current), AD2 bandwidth and offset at x1 vs the DMM reading, UI entry
      (A mode sub-function vs Record page), SCPI (arm / fetch RMS, peak, trace), emulator scenario (a stepped
      current into A), device check with a lamp / motor and a second meter or the scope + shunt.
      Done (docs/inrush-current.md): MF in A -> Current Settings -> "Inrush" (A sub-function, as the Fluke
      clamps; not the Record page) opens the Inrush Current page: AC A row of the 10 A range (manual), the
      AD2 stream (wave-view style session, 9600 S/s, the section 11 ISR), the DMM paused while armed.
      Trigger: the ~1 ms mean of |I| exceeds the largest such mean of the previous ~100 ms by the step
      (default 1 A, stepper 0.2..10 A, INR:THR 0.1..50 A): an absolute threshold from ~0, a rise by the
      step on a running load (a running 2 A rms AC load does not trigger 1 A); either polarity; t = 0 = the
      window's first sample (<= ~1 ms early). Trace: 200 ms pre + 1.8 s post as 16-bit codes (raw >> 3,
      3.8 mA) in the scope's BUF (lent: the MCX acquisition idles, the stored scope record dropped; no new
      SRAM). Results: Irms over 100 ms from t = 0 (AC + DC rms), the signed peak (OL at the AD2 rail),
      idle rms, lost samples. Saved through the fast logger as LOGS/FASTnnn.DLG (kind 3: header with the
      results, t_s from the first sample, t0 = 0.2 s; FASTnnn.CSV on the USB volume / MMEM:DATA?), the GUI
      live meanwhile. SCPI INRush[:STATe] / :ARM / :THReshold / :CONDition? / :FETCh? / :FILE? /
      :PREamble? / :DATA? (setters refused over BT). Closing restores the A sub-mode / autorange and BUF.
      Emulator emu/hy3131/inrushtest.py PASS: DC 12 A 20 ms / 6 A 80 ms / 3 A step -> Irms 7.568 A (exact
      window 7.589), peak 12.012 A, idle 0.050 A, CSV 19 200 rows, t0 0.200000 s; 2 A rms AC running 6 s no
      trigger, 5 A rms step -> Irms 5.104 A (the DMM's AC A 5.000 A: the model's raw AD2 sine is 2.2 % above
      its DC scale, an emulator artefact, doc section 8), host CONF stops it. Host tests (tools/meter-test inrush), ui-render i01..i07.
      Budgets (on 32d4d4b): region B 13.9 KB free (19.4 before: +5.4 KB), SRAM unchanged (10.9 KB free),
      stack headroom 21.2 KB (21.4 before).
      (verify on device: docs/inrush-current.md section 9: lamp, motor, a second meter / clamp)
Emulator (emu/):
- [x] Front-end schematic: label the jack wires with the HY3131 pins the firmware's register settings actually
      use per function (from the rows / mux selections: e.g. which PA tap, SENSE, PB0..PB7, FB/RLU), not "the
      datasheet's typical circuit" (owner 2026-10-21). (4fea6af first cut; db5d93f
      "jack-wire pins from the registers only": no role table left - the pins the enabled blocks
      select (ENCMP alone for the comparator, SAD1I for AD1, SDIO) put on a jack by a per-pin board table,
      PB4 follows OP1's input, RLD on the Ω net, every closed PA switch with its P / D / F / S letters, the
      divider ratio from the SS taps (power meter /1000 etc.), the test-source band names the node by its
      pins and derives its AD1 / comparator chain; columns reserved so nothing moves between functions.
      Checked on 24 opendm40 emulator states + all 47 table rows at 700 / 1000 / 1400 px. Visible change for the
      owner to confirm: an enabled comparator nothing reads (stock R20 0xf0, SCMPI = PB4) now shows PB4 into
      CMP in the DC rows, and AC V r0 says divider /100 (PA6 tap; the old range table said /10))
- [x] Signal-flow diagram jumped every 100 ms in DC V 6 V (owner: "скачет, некрасиво"): the comparator's
      PB1 / SENSE time-share added / removed the A-jack PB1 row and re-routed the CMP wire. The layout is now
      held per function / range: every pin / input any state of the row can use is reserved (opendm40's switches
      predicted from their register signature, the rest learnt from the state history), an alternation seen in
      the last 8 updates is drawn steadily (both wires dashed, "CMP PB1 ⇄ SENSE", "alternates every 100 ms
      (A-jack / AC? hint)"). Same for AC V's AD1 PB1 - PB3 / SENSE - PB5, the A-PROT probe (OP2 on PB2, AD2),
      the frequency counter's CMP / CNT source, the cap comparator's PB0 -> OP1O, the TC phases (SAD1I), the
      ENPKH restarts and AC+DC (the AC V / DC V rows held together). emu/flowtest.py checks it (0 moves).
- [x] HY3131 model at terminal level: the user sets what is connected to V-Ω/COM/A (sources/loads, incl. voltage AND current
      at the same time); the firmware's mux/relay choices route it through a modelled DM40 front-end (dividers, shunts, relays,
      switches) — no per-mode magic inputs. (e48d384 DM40 front-end model + connection editor, 8bdca47 cross table)
- [x] Show the firmware's current mux/relay/switch selection as feedback in the GUI (draw the front-end path).
      (e48d384 schematic, a4bf604 HY3131 pins + internal switching, cee364b Frontend state line)
- [x] Cold-junction temperature and the HY3131 die temperature are separate inputs (die temp always set; default equal).
      (e48d384 CJ/die split)
- [x] GUI: Apply on Enter in the HY3131 input fields; show only the fields relevant to the chosen source (no freq/DC offset
      for plain DC etc.). (e48d384 GTK connection editor: Enter applies, per-source fields)
- [x] Virtual backup files (owner 2026-10-21): second USB MSC LUN = read-only FAT16 volume DM40-INFO
      generated from the MCU memory (src/vdisk.rs: CALIB / BOOTDATA / UID / OPTBYTES / OTP / ROMBOOT / LOADER .BIN,
      BACKUP.TXT, FIRMWARE.BIN = running firmware as update file); per-LUN SCSI in msc.rs (INQUIRY names, WP bit,
      WRITE -> 07/27/00, PREVENT / UNIT ATTENTION per LUN). SPI disk: only a write-once safety copy of
      CALIB/BOOTDATA/UID (DBG:BOOT? CALBK SAFECOPY_PRESENT/WRITTEN + differs). Host-tested (fatren-test) and
      emulator-tested over USB/IP (userspace client). docs/native-boot.md "Unit data".
      Superseded by the USB storage redesign (one generated volume with INFO/ etc., above).

## DMM page / settings batch (owner notes ../замечания.txt, 2026-10-21)
- [x] Second stock resistance mode = the stock m+8 "ON" variant (p8set rows, lower test current for
      in-circuit use). It was ported (second tap on the ohm button, persisted in p[2]) but invisible:
      now the stock "ON" tag (ON_TAG font, glyph 0x20) on the ohm button (DMM page + combined band)
      and Normal / "Low current ON" buttons on the Resistor Settings page. (verify on device: tag shows,
      the ~0.27 mA vs ~0.85 mA test current)
- [x] ui-review #15 dead code: ui::bar / button_row / DMM_BUTTONS / sub_label / val_line and four unused
      layout constants, src/fonts.rs and src/text.rs (the anti-aliased Roboto set; nothing else used them),
      tools/mkfont.py and fonts/Roboto-*; host-test module lists updated (ui::button_hit stays: live).
- [x] Continuity: a negative result (source / reversed diode / charged capacitor on the jacks) clamps to
      0.00 ohm as the stock (was -666 / -1114 ohm; emulator cross table). (emulator-verified: V 1.234|cont = 0; verified on device f80524f: 0.00 ohm + LIVE?)
- [x] AC V on a pure DC source reads ~18 mV in the emulator (stock ~0): NOT a firmware bug. The model's
      coupling capacitor charges to the raw jack DC (UpdateAcNode: src = dc * NodeRatioAc) while the
      seen step uses the calibration-folded node DC (NodeDcAc: DcQ(...)), so step = dcNode - vc never
      reaches 0: residual = 1.5 x V x (cal gain/offset error) = 17.7 mV at 1.234 V. With `cal:0` opendm40
      settles at 0.64 mV (the model's noise floor). The stock escapes it because its mV-first AUTO steps
      to AC mV (DC coupled in the model, no coupling term); opendm40 stays on the 6 V range (AUTO+, UF-21).
      Fix belongs in emu/hy3131/HY3131.cs (charge vc towards the same DcQ-folded level).
      Fixed in e2c9ab0 (vc charges to the calibration-adjusted level; test settles to the floor).
- [x] UF-30 bargraph wired: in the history row's place (8,154,464,18; the 7 px strip under the digits
      cannot keep 3 px from the F78 ink and the history boxes), Settings 2/2 "DMM row" Bargraph (default) /
      History (p[38] bit 10). Fed in App::reading with the raw reading (pre-REL, pre-smoothing; keeps
      moving while HOLD / AutoHold freeze the digits); bipolar in DC V / mV / A (not AC+DC), unipolar
      elsewhere, empty track in temperature, red track on OL. History taps / long press only in History
      mode. DBG:PROF? section "d.bar". (verify on device: d.bar cost, bar at 10/s moving / 2.5/s steady)
- [x] Temperature unit deg C / deg F as one global setting (owner: the deg F readout in the top-left status
      slot was odd): Settings 2/2 "Temp unit", SCPI UNIT:TEMPerature C|F / ?, persisted (p[38] bit 9).
      Applied to TC / PT100 / PT1000 / NTC readings, the cold junction (aux row, temperature page, MEAS:TEMP:CJ?)
      and SYST:TEMP? (MCU die). The second deg F readout is gone (status slot shows AUTO / MANU again).
      A unit change restarts the meter like a function change and converts the temperature limits; offsets
      stay in C (0.1 C trim steps). Bluetooth keeps the stock frame (main C, CJ C, value 3 deg F), converted
      back in bt.rs. (verify on device: phone app in F mode still shows C/F correctly)
- [x] Temperature page redesign (owner notes): "Offset 0" key removed; the offset is a settings-style
      stepper "Offset [-] +7.3 C [+]" (0.1 C steps) and a tap on the value opens the signed keypad (the
      0.12 C/px slider is gone); slot 8 of the sensor grid = the global unit (Unit: C / F). "Fuzzy" buttons
      found: the page's hit boxes were exactly the drawn boxes (4302 dead px in the 5..6 px gaps, touch-test
      reach L+0 T+0 R+0 B+0) and nothing showed a press: now 3 px slop on every key (dead px 953, all in
      the wide gaps) and pressed feedback from touch-down (ui-review #11 rule). The sensor type stays the large
      TEXT22 chip on the DMM page (671006d).
- [x] NTC user R25 / Beta editable on the temperature page (fields shown with "NTC user" selected; numeric
      keypad, R25 in kohm, limits 100 ohm..10 Mohm / Beta 1000..10000 K as dmm.rs set_ntc_user), persisted
      (p[16] / p[17], existing), SCPI [SENS:]TEMP:NTC <R25>,<Beta> (existing). A change while NTC user is
      active re-enters the sensor (NTC entry range follows R25).
- [x] Fix of 26d64d4: Settings 2/2 had the "Temp unit" value but not its row name (rows shifted by one:
      tapping "Scope swipe" toggled the unit). Row added.
- [x] Tap on the DMM main value = resolution step FAST -> MED -> SLOW (fewer digits = faster), with a
      1.5 s toast of what the step really gives here: FAST "60000 counts / 10/s" (one OSR-8192 chopped
      conversion: 163.84 kHz / 8192 / 2); MED / SLOW "600000 counts / <window> s" where the range has the
      +1 digit (8 / 16 steady OSR-32768 conversions of 400 ms, ohm 16 / 32), else "60000 counts / 2.5/s".
      The +1 digit stays the small half-size glyph. Functions without a rate choice: low beep + toast.
      Not added: a 6000-count / ~40/s step (OSR 0x13, the logger's measured 38/s): its noise on AD1 is not
      measured (dmm.rs: noise floor open); add it after a device noise run.
- [x] UF-30 bargraph restyled (owner: "unclear what it is"): a classic segmented DMM bargraph: 50 segments
      (2 % each; bipolar 25 per side from a centre zero), the 10 % marks as full-height segments (ruler), lit
      segments orange, all red + a red "OL" marker on overload, F14 scale labels under it: 0 / half / full
      scale with unit (bipolar -FS / 0 / +FS unit), full scale = the range's (1000 V / 10 A where limited).
      Rows 147..172 (3 px from the F78 ink and the chart frame), probed for the ui-render ink check;
      (verified on device f80524f: visible, segmented, bipolar; d.bar 1 us)
      incremental update repaints only the changed segments (98..150 bus words / update; bargraph-test:
      every update equals a fresh draw).
- [x] ui-review #13 remainder: the DMM page value turns the stock red (0xC9A0) in the V functions from 30 V
      (live reading, also while HOLD; the stock cue, gui_v2_6.h:157/175), lightning still from 100 V. No bar
      tint: the stock has none. (verify on device: 30 V on the calibrator)
- [x] Device bug (5c6b565, 40 Hz square 0..10.4 V): CONF:VOLT:ACDC then CONF:FREQ read 40.002 / 13.334 /
      26.668 Hz with duty 100.0 % for ~11 s (MIN / AVG polluted); DC V -> Hz clean. Not reproduced in the
      emulator (its counter model has no settling input path: clean 40.0007 Hz at once). Firmware side:
      the frequency mode's two-source choice kept its history across functions (last / change / source
      phase never reset) and accepted results with duty 100 % (periods counted while the input is never
      seen low = edges missed). Now: source history reset on every function change; a result with f > 0
      and duty >= 99.95 % neither wins the choice nor reaches the readout / MIN / AVG (no reading that gate).
      Cause of the missed edges (input path settling after the AC+DC relay pulses?) still open: retest on
      the device; if the wrong readings persist, compare the stock's AC+DC -> Hz entry (R20 / relay order).
      (verified on device f80524f: AC+DC -> Hz 2 x 43 reads all 40.002 Hz, duty 50 %, MIN/MAX clean)
- [x] DMM batch polish: bargraph full scale in continuity / diode = their OL ceilings 1000 ohm / 3 V
      (bargraph::mode_full_scale; ui-render d18 / d19, bargraph-test); Help 3/5 rows "Tap / hold history:
      Store / clear" + "History row shown: Settings: DMM row"; ui::list_page asserts one value per row on
      the host (ui-render / touch-test; touch-test passed 11 values to the 9-row pages, fixed).

## Device check 92fb8fa (2026-10-22; docs/device-check-2026-10-21.md "92fb8fa")

- [x] Status bar kept "MANU" after a reading released the range hold (REL ended by an AUTO coupling switch):
      DmmView's status key (auto && !auto_plus, .., auto_plus) was equal for MANU (v_auto 1 under a manual
      range) and AUTO+. Key = r.auto now (ui.rs). Verify on device: AUTO + REL on the square -> 1 V DC: AUTO+.
- [x] Combined view strip: the unit box (ST_UNIT, 40 px, SC_DIGITS) clipped power units ("-0.0001 mW"
      read "mV" plus a sliver; device g7-combined.png, ui-render c12-strip-power). Fixed (scope_ui.rs,
      ui-fix-92fb8fa): value box 84..191 (108 px: "-444.44" / "1562.00" ink), the unit from 196 as wide as
      its text, SC_DIGITS up to 59 px ("mW", "MHz", "VAh" stay large), else SC_F20 on the digits'
      baseline ("mWh", "kWh", "mVA", "kVAh"); the sub rows start 4 px after the unit, with GEN 15 px before
      its column (64 px now: "10.00MHz" was cut too), too wide -> no spaces -> spacing 0. Renders
      c12b-strip-mw / c12c-strip-mwh / c12d-strip-mhz / c12e-strip-acdc-gen.
- [x] ui-render clip rule (stockfont probe_fit, cfg(test)): every text's ink inside its layout box. It
      found more than the strip: the DMM page unit ("MHz", "mW", "mWh" cut at the left: LETTERS / TEXT22
      fallback), the chart side MIN/MAX/AVG ("-444.44": minus cut; a decimal fewer when too wide), the
      axis labels ("-100.049kWh": a decimal fewer), the power side AVG (fewer significant digits), "REC
      99:59:59" (the time alone, orange), the extra digit with a 5-digit value ("1562.00" F: no extra
      digit), the BT status icon (16 px glyph in a 15 px box), the Ohm "ON" tag box, the wave view's trigger
      badge ("-1.23mA" -> "-1.2mA"), the range page note ("AUTO+ recommended: never steps to mV"). The
      stock brackets of the replica top bar (4 px boxes, stock-diff exact) are exempt (sf::fit_exempt).
- [x] Power page coupling switch: the old layout and values stayed up to ~4 s. Fixed (app_power.rs
      pw_tick / pw_coup_changed, powermeter.rs pw_pending): any coupling change (W button, Power
      Settings, POW:COUP) drops the old reading (meter reset, as a function change), redraws the page in
      the new coupling's layout and shows "--" for every measured value (energy, time, settings kept)
      until the new coupling's first reading. Emulator: DC -> AC, AC -> AC+DC, AC+DC -> DC (W taps) and
      POW:COUP AC all show the pending page at once (scratchpad uifix/emu-pow). Verify on device.
      Device 540e056: PASS (W taps DC -> AC -> AC+DC -> DC: "--" page in the new layout in the first read,
      0.05 s). Cosmetic: on AC -> AC+DC bits of the AC layout's right column ("VAh" / "AVG" rows) stay ~2.5 s;
      the pending page's chart labels have no unit.
- [x] Help 5/7 title ran into "< Back"; Help 2/7 "Power key held 1.5 s" touched its value column (and
      three more rows were as close: help 1 "Scope: drag sideways", help 3 "Tap value, rate chip", "Tap a
      function again"). Fixed (ui.rs): page_title starts a title 28 px after "< Back" when centring would
      come closer; the topic box ends HELP_GAP (19 px) before the value column; the four topics shortened.
      Why the check missed them: its only spacing rule is >= 3 px between ink boxes; both were 8-11 px
      apart, under one TEXT22 word space (~13 px), so they read as one phrase. New ui-render rule
      `line_gap`: two texts of one size on one line need >= 1.5 word spaces (the smaller word gap of the
      two) unless both are single words (value + unit, tags); the power sub-lines' pieces are one text
      (probe::group).
- [x] DMM chart axis labels kept the previous function's unit while a function reads OL / no value (cap
      "0.050MOhm", diode "MOhm", temperature "Hz"). Fixed (app.rs): a function change clears the chart
      unit; an empty chart takes the reading's unit / digits even while it reads OL. Emulator: FREQ ->
      DIOD (OL) labels "0.0050V" (uifix/emu-chart). Verify on device.
      Device 540e056: PASS (Hz, diode OL, cap OL, ohm, temperature OL each with its own unit). Strip units
      "mW" / "mWh" PASS on the device ("MHz" not reachable there); Help 2/7 and 5/7 PASS (title 14 px after
      "< Back", topic -> value >= 21 px on all pages).
- [x] Wave view entry from DC V: DBG:ISR? `dropped` +7087 (~185 ms, the DC V self-cal at entry) before
      the view drains; AC V entry 0. Harmless (the view starts after), note in feasibility 13 or skip the count.
      **Fixed (dda802c):** not the self-cal (it runs before the ISR starts) but the full scope repaint after
      it: the new cal moves the wave scale's a / b, `wave_live` took any scale change for a repaint, and the
      repaint ran with the stream on (the ring holds ~50 ms at 38400). Now only a V/div / unit / coupling
      change repaints. Emulator (wavetest --case dcv, a DC V entry at 2 ms/div): overwritten 3555 -> 0.
      Device c92b605: PASS (3 entries, field 6 = 0 throughout).
      (A V/div step still repaints with the stream on: the record restarts after it anyway.) Device retest:
      DC V entry at 2 ms/div, DBG:ISR? field 6.


## Device check 9d56ca6, the RAM round (2026-10-23; docs/device-check-2026-10-21.md "9d56ca6")

- [x] Screenshot streamed from the panel: BMP = the frozen panel, 0 px, on a menu, AC+DC, a running
      Wave-view trace and the CHA colour picker (exact PALETTE values; GRAM R/B low bits 0, so 666 -> 565
      is lossless; no dummy-word slip). READ? and the DMM run through a shot (a 1 -> 2 -> 1 V step
      followed); data-phase passes <= 4.5 ms; 3 passes of 50-78 ms = the FAT x2 + dir block writes (since
      048f425). FFT in the arena: stops for a shot, resumes with the same peak; CSV commits pause it
      < 0.2 s; MMEM:CAT? bursts barely. calbackup SAFECOPY_PRESENT, CALIB / BOOTDATA / UID unchanged.
- [x] Freeze is ~5.4 s (touch back) and the toast ~6.1 s, not 4-5 s; with steady DBG:LCD? traffic the
      same job ends in 2.3-4 s (why?). Pre-erasing the next shot's clusters in idle (ram-budget.md
      follow-up) would cut it to the page programs. Done (d222516); device 540e056: frozen 1.36-1.38 s,
      touch back by 1.54 s, toast ~2.4 s.
- [x] A shot during a fast-log session wrote nothing at once but ran after the session, on a half-repainted
      page (S023); the same after a host LOG:STAT session (S027). Fixed in 20a91d0 (App::shot_request:
      "Disk busy" at the gesture; a shot waits 1 s after a full repaint). Verify on device: the swipe
      during `LOG:DEST DISK` + `LOG:STAT ON` shows "Disk busy" at once and no S*.BMP appears after; during
      a host LOG:STAT session, the shot after LOG:STAT OFF holds the reading and the chart.
      Device 20a91d0 (2026-10-23): PASS. "Disk busy" in the first panel read (0-0.14 s), no BMP; after a
      host session S012 / S013 hold the reading and the chart, the shot 1.07 s after LOG:STAT OFF.
- [x] Wave view: `LOG:CSV ON` is accepted but records no rows (READ? 9.9E37); ram-budget.md says csvlog is
      refused there. Refuse it (Settings conflict) or fix the doc.
      **Fixed (dda802c):** refused: SYST:ERR? -100,"Settings conflict: Wave view" (and ": inrush page" on the
      inrush page, the same paused DMM), LOG:CSV? stays off (App::rec_conflict). wavetest checks it.
      Device c92b605: PASS (the Wave view and the inrush page: the error, LOG:CSV? off, no new file).
- [x] Wave view, not under wavetest's per-step region C / ring checks yet (the whole-run check covers what
      the test does): the source badge exit (DMM -> MCX: `wave_close` without `show`, so the MCX page runs one
      pass with the stream on), a V/div step at 2 / 1 ms/div (a full repaint with the stream on: the ring
      overflows, the record restarts), the inrush page's LOG:CSV ON refusal (inrushtest).
      **Done in 7225b36 (tests) / 3f32bc3 (fixes).** V/div + / - at 2 / 1 ms/div: before overwritten +3164 /
      +3877 per step, after 0 (`wave_rescale`: the repaint runs before the stream restarts). Badge exit at
      2 ms/div: passed in the emulator before too (no forced stop, no region C); the close is now held for the
      stream's stop anyway (`wave_badge_close`, as `show`), so the rail switch and the MCX page never run
      under the stream. inrushtest (region C hooks now): LOG:CSV ON on the armed page refused, still ARMED, no
      region C. wavetest volt / dcv, inrushtest: PASS. Device retest: Wave view at 1 ms/div, V/div + and -:
      `wdrop.py` dropped +0, the trace back within ~1 s; the badge out at 2 ms/div: late 0, overruns 0.
      **Device d330877 (device-check-2026-10-21.md "d330877" 1b): PASS**: V/div + / - at 1 ms/div dropped 0, late 0, overruns 0; badge exit
      at 2 ms/div late 0, overruns 0, MCX page drawn.
- [x] MENU on the Device page returned to Settings 3/3 (device 519fb8a, the "State at the end" note).
      Stock: Device is a section of the one Settings screen; what MENU does there is not traced, the main
      menu is the likely answer. **3f32bc3:** MENU on Device -> the main menu; a tap on Device still goes
      to Settings 3/3; MENU on Settings 3/3 / 2/3 still steps back a page (opendm40's own paging; Help -> Settings
      unchanged). Emulator: fastlogtest --ui-nav page tree. Device retest: Settings > More > More > Device,
      MENU: the main menu.
      **Device d330877 (device-check-2026-10-21.md "d330877" 1c): PASS.**
- [x] AUTO OVERLOAD on AC 1 A (50b9a16, couptest iac repro): blocked, the A jack is not wired.
      Fixed in 073b309 with the 2026-10-22 BUG (quiet hop in AUTO); the device check is in the list below.
- [x] powtest `ereset` FAILs on 9f4edb3, 4ed771d, 2f5d3b8 + dmm-bugs alike, also run alone (the Yes tap
      at 310,204 at 21 s, 13 s after the long press, does not reset the energy: Wh keeps counting; it passed
      at a8048ba per the device-check doc). (`acdc-floor` failed once in the 20-emulator parallel run and
      passes alone, as noted at a8048ba.)
      Done (5301fb7): no regression to bisect. a8048ba..9f4edb3 changes only acdcm.rs (method C) in opendm40 and
      the emulator's LPF creep (38a63eb); a8048ba itself FAILs on its own emulator and f7c99e8 passes there,
      run to run. A race with two sides:
      1. Test: pure DC on AC+DC raises the DC? hint ~2 s after the first reading (5..9 s in the emulator);
         its toast (40,132 400x56, 1.5 s) covers the long press point 58,179, and a touch on a toast's box
         is the toast's (e206875), so the long press at 8 s never opened the confirm (failing run: SYST:ALER?
         DC from 6.99 s). The long press now waits for the hint + 2 s (powtest GATE: 8..14 s), Yes 13 s
         after it.
      2. Firmware: `App::toast` and the auto power-off warning took the overlay slot unconditionally, so a
         toast during a confirm replaced it without its No path (the reset's Yes hit the bare page; a fuse
         check in stage 1 would keep measuring, the firmware question stay unanswered). Now a toast that
         comes while a modal overlay is up (`Overlay::modal`) is kept (one slot, the latest wins) and shown
         once the confirm has gone and the page is drawn again.
      Emulator: new `ereset-hint` (AC part on V, long press at 8 s, pure DC from 12 s = the hint under the
      confirm, Yes at 21 s): FAIL on f7c99e8, OK on 5301fb7; `ereset` OK 2 of 2 alone (gate 6.85 / 7.07 s).
      Full powtest on 935acd6 (one emulator): 29 of 30 OK incl. ereset / ereset-hint / entry; `zero` failed
      once (one DC-row AD1 zero 158 vs 300 +- 130: the noise tolerance) and passes 2 of 2 alone on 935acd6
      and on f7c99e8 (flaky, not from this change). leadstest OK. ejecttest (the firmware question) could
      not run here: usbip attach needs vhci_hcd (not loaded on this host).
      Device retest (power meter, AC+DC, V-COM on a DC source, nothing in A): (a) right after entering the
      meter, wait for the "No AC: DC is more precise" toast, then long press E: the reset confirm; Yes resets
      Wh and the time. (b) Long press E within 2 s of entering (before the hint): the confirm stays up when
      the hint comes (DC? badge), Yes still resets, and the hint toast shows after the confirm closes.
      Owner question (not changed): a long press on a toast's box does nothing (only a tap there acts, e206875),
      so a long press on E while a hint toast covers it is lost; should a long press go through to the page?
      **Device d330877 (device-check-2026-10-21.md "d330877" item 3): PASS** (a) and (b): the confirm survives the DC hint, Yes resets Wh / time,
      the hint toast shows 0.15 s after the confirm closes. In AC+DC the E row is at y 170..192 (the parts row).
      Under the confirm the page is not redrawn (no DC? badge until it closes).
- [ ] Device retest "dmm-bugs" (073b309; most need a current in the A jack: a current-limited bench
      supply + load or the calibrator's current output; the meter here has the A jack open):
      1. A AUTO coupling (CURR:COUP AUTO), AC 1 A @ 50 Hz: SYST:ALER? NONE throughout, CONF? 6 A range
         within ~2 s, the reading 1 A; then 300 mA: back on the mA shunt (CONF? 0.6), NONE. Same for CURR:AC,
         CURR:DC with DC 1 A, CURR:ACDC with AC 1 A and with DC 1 A (AUTO everywhere).
      2. The same with AC 1 A held 60 s and CONF? polled ~5 / s: never 0.06 / 0.6 / uA while 1 A flows
         (the down-range guard: decides whether the device's RMS settle outlasts an AC phase, rmsfill).
      3. Manual 600 mA (CURR:AC:RANG 0.6), 2 A: red OVLD + one 300 ms beep + toast, after ~3 s amber "10A"
         (status slot and combined-view strip), SYST:ALER? OVERLOAD throughout; 2 A off: back on 600 mA
         after ~5 s, NONE.
      4. Manual 60 mA (CURR:DC:RANG 0.06), 2 A then 40 mA: back on manual 60 mA after ~5 s (was never).
      5. A jack open, calibrator on V-COM: RES and TEMP with 1 V, 5 V, 10 V DC, -5 V and the 0..9.6 V
         40 Hz square, 10 s each: no A-JACK; RES with >= 5 V shows LIVE?; DBG:SENT? field 12 = 1 (driven)
         where LIVE? shows. The comparator still counts (field 8, hits, grows while gated): step RES through
         1.0 / 1.5 / 2 / 2.5 / 3 V DC; the lowest level with hits bounds the PB1 pickup, k ~ 23.75 mV / V_trip
         (replaces the inferred 10 mV / V of the model; k > ~17 mV / V would leave the 1.4 V gap of doc 4.8).
      6. Leads in A / COM, V-COM open, a few mA (2-5 mA) through A in RES and TEMP (K TC and an NTC type):
         A-JACK within ~1 s (RES) / ~2.5 s (TC), field 12 = 0.
      **Device 519fb8a (device-check-2026-10-21.md "519fb8a" 7a-7d):** 5 TEMP PASS (NONE, field 12 = 1 at
      5 / 10 V). **5 RES FAIL:** A-JACK at 1 / 5 / 10 V and the square, never LIVE?, field 12 = 0 (free); FETC?
      is OL (positive), not negative, on every range 600 ohm..60 Mohm at 5 V: the ohm AD1 code does not go
      negative for a positive source, so V_GATE's premise fails on the device (owner: what tells a driven V-COM
      in RES then? a negative source not tried). Comparator hits: none at 1.0-3.0 V DC, hits at 5 V: the
      pickup k ~5-8 mV / V. 1-4, 6 blocked (no current in A).
      Item 5 RES: superseded by the jack check (doc 4.13; device c6a3ac3 PASS, item "Device: doc 4.13" below).

## Device check 20a91d0: shot fix, 76.8 kS/s, AC+DC methods (2026-10-23; docs/device-check-2026-10-21.md "20a91d0")

- [x] Shot fix re-check: PASS (items above). 76.8 kS/s: host / Wave view / DIV 8,1 PASS, the disk FAIL (items
      under "76.8 kS/s").
- [x] AC+DC methods session (acdc-approaches.md §10.5): e6 / e1 / e7 / e2 / e5 run. The decision rule for C as
      the default is **not met**: the AC part is within 0.5 % of A's (+0.13 % on 6 V, +0.23 % on 60 V), but
      the DC part on 60 V is +0.09..0.10 % at 24 V (e5, 21 of 21 entries over 17 mV) and +0.28 % at 4.78 V
      (e2): C's 60 V self-cal is ~+11 mV + ~0.05 % off. 6 V passes (max 1.38 of 2.0 mV). B fails e1: the zero
      is bimodal per entry (6 V +12.6 / -30.5 mV, 1000 V +1.9 / -1.5 V), residuals up to 7.5 digits (60 V) and
      16.6 (1000 V). Default left at A (owner's decision).
- [x] Method C at >= 5 V DC on the 6 V range (AC+DC and AUTO): the AC part reads 2.11 V (6 V range) / 3.83 V
      (60 V range) on a DC input and the range hunts 6 <-> 60 V every ~3 s (~0.7 readings / s; AUTO after the
      square stays on AC, "~4.056 V" for 5 V DC). Fine at 4.0 / 4.5 V. A in the same sequence: PASS.
      Fixed in the emulator (acdc-approaches.md §10.6 item 2): not node clipping; the RMS block's HPF transient after each
      row write plus garbage RMS blocks above ~100 k codes of DC. The AC part is used only when sqrt(RMS) <=
      half of AD2's peak-hold p-p (else the last RMS value / the bound on a steady node; no usable RMS with AC
      for 2.5 s: up a range, remembered); up-range on the peak hold at 90 %; no self-cal per range visit.
      Emulator: reproduced on 540e056 (model "hpftau:" / "rmsbig:"), fixed (couptest sq5). **Device retest: §10.7
      e9 / e10.**
- [x] C's 60 V DC part: +11 mV offset / +0.05 % gain against DC V (above): check the self-cal's 60 V path
      (AD1 on SENSE - PB5 vs the AD2 node) before C can be the default.
      Fixed in the emulator (§10.6 item 1): the anchor is now the DC V row's READ? against the run row's own LPF
      (AD3 + RMS on), per range and session; the gain from the internal zero, kept per range. Emulator: 540e056
      ~+16 mV at 12 V reproduced, the fix within 1.3 mV. **Device retest: §10.7 e8.**
- [x] C's AC part +0.13 / +0.23 % against A (e2): through the factory AC V gain / offset now (§10.6 item 3;
      e2 recomputed: +0.014 / -0.003 %). **Device retest: §10.7 e2.**
- [x] Device retest of method C (acdc-approaches.md §10.7, results §10.8, device-check doc "ac1e852"): e9 / e10 /
      e2 / e6 / e5 and the by-hand AUTO 5 V case PASS (5 V: AUTO DC 4.9981 V on 6 V, 9.4 rd/s; AC part vs A
      +0.012 / -0.004 %; e5 60 V +0.5 / -1.3 / +0.5 mV, was +21..25); the R29 peak-hold restart leaves the RMS /
      LPF alone (9.2 rd/s, acsrc 0 throughout). **Decision rule not met**: e8 60 V at 1 / 0 V -7.2 / -8.0 mV
      (limit 5.5 / 5.0; same-moment READ? -6.4..-7.1 mV). A stays the default (owner's decision).
- [x] C's 60 V gain from the internal zero is +0.030 % (per_code 4.85309e-4 vs ~4.8516e-4 implied; the
      internal zero ~15-16 codes / ~7.5 mV off a real 0 V input): DC part -7..-8 mV at 0 V on a manual 60 V
      range. 6 V is fine. Options in §10.8 (zero correction per range, two-anchor gain, a device table
      per_code); measure the offset's repeatability over entries / power cycles first.
      Fixed in the emulator (acdc-approaches.md §10.9 item 1): once the input settles >= 10 k codes from the
      anchor C takes the stock DC V reading again and the gain between the two references (kept per range until
      power-off, replaces the internal zero's); no hardware constant. Model "agndz:" (15 codes on 60 V);
      acdcmethtest --e8 down: ad0587c -6.7 / -7.3 mV at 1 / 0 V, the fix +0.4 / +0.2 mV; --e8 up (entered at 0 V,
      the table's gain): ad0587c -20.5 mV at 24 V, the fix +0.4. **Device retest: §10.10 (e8s / e8up).**
      Device 80d5b01 (§10.11): 60 V PASS (e8s 1 / 0 V +1.8 / +1.9 mV, was -7.2 / -8.0; e8up 24 V +1.2 mV).
- [x] AUTO coupling on a signal with AC ~= DC (the 40 Hz square): C stays DC, A goes AC (its first AC part
      overshoots); the first evaluation (AC > |DC|) decides and the 1.5 x hysteresis keeps it. Decide whether
      that tie needs a rule (e.g. prefer AC within a few % of DC) before C's default.
      Rule (§10.9 item 2, dmm.rs AUTO_TIE): DC until the first decision (AC only with no DC part); the first
      decision waits until the AC part has settled on one side (4 x its last change < its distance to |DC|),
      then the larger part; a tie within 1 % or no settling in 4 s: DC. couptest sqtie (three entries on the
      square): ad0587c DC -> AC with A and C, the fix DC throughout with both. **Device retest: §10.10 e11.**
      Device 80d5b01 (§10.11): e11 DC in all 6 entries, 0 changes; by hand AUTO DC with A and C: PASS.
- [x] dcvtest LOG:SCALe? 1 % off (since fb96992): the test did not expect the model's per-range per_code factor
      (Ad2DcGainR, the device's); fixed in the test (§10.9 item 3).
- [x] Device retest of §10.9 (§10.10, results §10.11, device-check doc "80d5b01"): e8up, e8s 60 V, e8, e11, e10,
      e9, e5 PASS. **Decision rule not met**: e8s 6 V at 0 V -0.86 mV (limit 0.5), tc6 2 of 3 power cycles. A stays
      the default (owner's decision).
- [x] C's 6 V two-point gain over a short span (5 -> 3 V, 41.6 k codes) is +143..+195 ppm against the internal zero's
      (104 k), so 0 V reads -0.25..-0.92 mV after it: m2 (ST_MEAS) is taken ~3.6 s after the step while the AD2 LPF
      still creeps 5-12 codes for ~8 s (DC V settles in ~1 s). Fix candidates in §10.11: (b) take m2 only once the
      LPF stops drifting (removes the cause); (a) keep the larger-span gain (brings back 60 V's AGND bias);
      (c) a larger CAL_GAIN_CODES on 6 V. Then device: e8s 6 V and tc6 (rt29/tc6.py) over 3 power cycles.
      Device a8048ba (contains adcb15f; device-check doc "ad97fd7 / c8c2f02 / a8048ba" 5, 10.13's list): e8s 6 V at
      0 V -0.25 / +0.18 / -0.59 / +0.21 mV (3 of 4 within 0.5 mV; the miss took a biased-judged second reference
      at 3 V, gsrc 3 gdis +8.95, then gsrc 2 at 1 V); e8up, e8, e5 pass.
      Fixed in the emulator (§10.12, 4e71686): (b) as a ~3 s slope window (< 1 code / s, >= 5 s after the step,
      timeout ~13 s), the new point taken before the reference row write; a shorter-span two-point gain replaces
      an internal zero's only when it shows that zero biased beyond the noise (60 V yes, 6 V no); pairing with the
      session's farthest reference point. Model "lpfcreep:" (two 2.5 s lags, 2.4e-4 of a step) + LPF noise;
      acdcmethtest --e8 tc6: af7d2e4 -0.75 mV at 0.1 V (+177 ppm), 4e71686 +0.07 mV. **Device retest: §10.13.**
- [x] Device retest of §10.12 (§10.13): e8up first after the install, e8s x 4 after `dm40dbg.py reset` (6 V 0 V
      within 0.5 mV in all four; gsrc 1 on 6 V), e8, e5; optionally tc6 (is the device's tail the model's?), e8l,
      e11 / e10 / e9. Then the owner decides on C as the default.
      a8048ba: 3 of 4 (one false "biased" judgement of 6 V's internal zero). Data (§10.14, device-check doc
      "acdc-bias"): every 6 V gdis beyond 1 code positive (the LPF creep's lag; 3 V level +3.9..+13.0, 10 of 10),
      every 60 V one negative (12 of 12). The suggested options (same sign over 2 references, repeat, noise
      scaling) fail on that data. Fixed (8e5ca2d, 9f36bbe): a creep allowance of 4e-4 x the step on the
      lag side only. Emulator: three cascaded lags reproduce the false judgement on 219817c (38a63eb).
      **Device 8e5ca2d full set + 9f36bbe confirmation (§10.15): PASS**: 6 V 0 V -0.34..+0.09 mV in 9 of 9
      e8s, gsrc 1 throughout on 6 V, 60 V gsrc 3 every session; e8up, e8, e5, e11, e10, e9 pass. C meets the
      decision rule; A stays the default (owner, next item).
- [ ] Make method C the AC+DC default, after a device check of C on the A / mA / µA ranges with real current
      (owner, 2026-10-23). Plan: acdc-approaches.md §10.16 (i8 / i8up e8-style levels per range, imix DC + 50 Hz
      AC, iauto CURR:COUP AUTO, ie5 drift; bench supply + load, a second meter in series as the reference; an
      acdcmeth.py variant driving the supply and the second meter).
- [ ] C's lag-side creep allowance (CREEP_K 4e-4): the largest lag-side gdis on the device is +16.2 codes
      against a threshold of 24.6 (9f36bbe). Watch it in later e8s logs (STAT? gdis / bthr); and why the 1 V
      level's lag is ~1/3 of the 3 V level's at the same step is open (§10.14).
- [ ] C's second reference now comes ~5-8 s after a level change (was ~1.5 s): e8s's 4 s levels may end before
      it. If the device tail is shorter than the model's, lower SLOPE_MIN (48 results) after the tc6 trace.
- [x] The first V tap after `CONF:VOLT:DC` on the 40 Hz square is sometimes lost (C 5 of 6, A 1 of 3; not at 1 V
      DC): device-check doc "80d5b01" 3. Not chased.
      **Cause found (device f6c6e05, device-check doc "f6c6e05" 6):** the tap lands on the grey hint toast "AC
      voltage here: use AC V?" (sentinel AC?, HINT_ASSERT_MS 2 s; shown from ~2.6 s after the CONF for
      TOAST_MS 1.5 s), and a toast takes the gesture (app.rs: "a toast just goes"). Method A: lost 14 of 30
      with the tap 2.8-4.1 s after the CONF, 0 of 30 at 0.5-2.75 s, 0 of 6 at 5 s; the range stayed 6 V
      throughout (not an autorange). 80d5b01's tap29.py tapped at 3 s, in the window. Decide: let a tap
      outside the toast's box reach the page (and dismiss the toast), or keep the stock-like swallow.
      **Decided (owner): pass through.** e206875: a toast is not modal; a tap on its box dismisses it, any other
      touch, swipe or side key dismisses it and goes on to the page; the confirmations stay modal. Device
      c8c2f02 (device-check doc "ad97fd7 / c8c2f02 / a8048ba" 4): toast up, V tap outside it 10 of 10 advanced
      the coupling (and the toast went); a tap on the toast 10 of 10 dismissed it, coupling unchanged.
- [ ] Method C's internal zero on 60 V (the ~15 codes of ac1e852): repeatability over entries / power cycles,
      and 600 / 1000 V (needs > 30 V), if C's first-entry gain is ever to be corrected by a constant.
- [x] DC V at the calibrator's 1 V read 0.99897..0.99949 V after the AC+DC session (0.999951 before it; the
      3 V reference moved -160 ppm during e5). **Calibrator, not the meter** (device-check doc "DC V at the
      calibrator's 1 V: meter or calibrator?"): no opendm40 state in the DC V path, the factory record unchanged,
      the 6 V and 60 V ranges agree, a level pattern (1 / 2 / 3 / 5 / 10 / 20 V: -0.9 / -1.4 / -2.5 / -2.0 /
      -3.0 / -1.3 mV) that is neither gain nor offset, the same pattern on 2026-10-22 20:24 before 20a91d0.
- [x] acdcmeth.py: the calibrator's "30 V" is 23.99 V (MR9270 limit?): the e1 / e5 / e6 30 V points are 24 V.
      Fixed (tools): mr9270.VOLTAGE_MAX = 24.0, set_value caps a larger voltage request with a warning on
      stderr (the value register reads back the setpoint, so the tool cannot see the real output);
      acdcmeth.py's levels, labels and docstring say 24 V (VMAX 24: e1's top level, e5 on 60 V, e6's step
      "to 24V"), ad2dcv.py's 30 V points are 24 V (ad2n7.py already used 24). Older logs' "30 V" rows = 24 V.
      Also: acdcmeth settle_time returned 3 NaNs where e6 unpacks 4 (a level without readings crashed e6).
      Device (9f36bbe, 2026-10-23): a 30 V request printed the warning, wrote 24000 and the meter read 23.985 V.
      Device e6 not run (permission denied in that session); settle_time's NaN path checked offline (4 NaNs; the
      9f4edb3 version 3, ValueError).

## USBTMC stress / ports / build warning (branch tmcstress, 2026-10-23)

- [x] emu/usbtmc_stress.py big / compound failed on WAV:DATA? sizes: the test was wrong (every USBTMC reply ends with
      '\n' since b3cbfbf, a block too); it now strips and requires the terminator (9d405d6).
- [x] USBTMC: a 64-byte transfer's ZLP left in the endpoint after an abort (`*IDN?` is exactly 64 bytes since the
      24-digit serial): Usbtmc::poll could write the ZLP before the data packet's XFRC came in, and that XFRC
      ended the transfer; INITIATE_ABORT_BULK_IN / CHECK_*_STATUS then said done and the next query read the ZLP.
      pump_in ends a transfer only once EPENA is clear (9d405d6). Emulator: usbtmc_stress 3000 iterations, all
      cases, PASS. Device retest: tools/dm40scpi.py `*IDN?` x many + a Linux usbtmc clear / abort (ioctl) mid-read.
      **Device 40728b8: PASS** (doc "40728b8" 3a-3c): pyusb with exact 64-byte IN URBs (usbtmc detached from
      interface 0 only, re-attached), c_abortin / stale / clear incl. the abort after `*IDN?`'s 64-byte packet
      with its ZLP pending: 2000 / 2000 (SUCCESS, PENDING, the drain gets the ZLP, next `*IDN?` exact); through
      the Linux usbtmc generic API 500 / 500; 2000 plain `*IDN?` / READ? / WAV:PRE? with no repair: 0 empty, 0 stale.
- [x] mscresettest / ejecttest default ports 4851 / 40200 -> a free one (run.free_port, 61000-64999 or
      DM40_EMU_PORTS=lo-hi; emu/README.md) (0a0c77c). mscresettest PASS; ejecttest not run (needs sudo usbip).
- [x] Build warning `csv_busy is never used`: removed (never called on rust-fw-poc or stream-ui) (cdd3367).

## Device check 540e056: shot pre-erase, SCPI Range / Digits + sigrok, scope full record, UI fixes (2026-10-23; docs/device-check-2026-10-21.md "540e056")

- [x] Screenshot preparation: items 1, 2, 4, 5 (CSV, fast-log), 6 PASS (item above). Owner: the battery pull
      during prep and the "PC can edit" + DM40*.BIN half.
- [x] SCPI Range / Digits + sigrok: retest 1-5 and 8 PASS; 7 = the AC+DC transient (open item above); SmuView
      to the owner.
- Moved 2026-10-27: the scope full record's leftovers are in "Scope in ngscopeclient" ((5) fixed on decaf60).
- [x] UI fixes 9861a29: strip units, Help 2/7 + 5/7, power pending page, chart axis PASS (cosmetic notes in the
      92fb8fa items).
- [x] DC V at the calibrator's 1 V read 0.9987-0.9993 V all session (sigrok 998.9-999.6 mV) with the calibrator
      at 1 V for hours: the 20a91d0 "warm after 24 V" idea does not explain it. Closed: the calibrator's
      setpoint error + its +-0.4 mV wander over minutes (item above); not a regression.
- Moved 2026-10-27: "Owner: confirm the DM40's DC V against a third reference" is in "Owner-only device checks" (top).
- [x] Minor: the 60 V range reads 0.2-0.36 mV below the 6 V range at 1 / 3 / 5 V (within spec; not chased).
      Not run on 40728b8 (meter needed for a flash). Use alternated blocks with pairwise differencing (chk4 xt.py):
      the calibrator wanders by the same 0.2-0.5 mV over minutes.
      Measured (device 0.2.1, 2026-10-26, DC V SLOW, FETC? mean over 3-4 s per block, 6 / 60 / 6 ... V blocks,
      60 V minus the mean of its neighbours): 0 V -0.24..-0.33 mV (mean -0.29), 1 V -0.15..-0.32 (-0.21, -0.27;
      two sessions), 3 V -0.35..-0.54 (-0.42), 5 V -0.21..-0.65 (-0.38, -0.62). Same sign in every pair, and as
      large at 0 V: a zero offset of the 60 V range against the 6 V range (0.3 of the 60 V range's 1 mV digit),
      i.e. the factory calibration's offsets (cal slots 0x14 / 0x18), not a firmware row or table setting. The
      only per-range firmware term, the 6 V range's `v += 1e-4 - |v| / 20000` (dmm.rs poll_dcv), would move the
      difference by -0.05 / +0.05 / +0.15 mV at 1 / 3 / 5 V, not the way the data go. Closed: no firmware cause.
- [x] Cosmetic: the rate chip cycles FAST -> SLOW -> MED (rate_cycle, enum order), a tap on the value FAST ->
      MED -> SLOW (resolution_cycle). Done (tmcnl-fix, app::next_rate): the chip, the value tap and Settings
      2/3 "Rate" all go FAST -> MED -> SLOW -> FAST (resolution order; the stock has no rate control, FAST is
      its 10/s reading). Device retest: chip and value tap from each rate.
      Device decaf60 (device-check doc "decaf60" B6): PASS. Chip SLOW -> FAST -> MED -> SLOW and
      FAST -> MED -> SLOW -> FAST -> MED; value tap (2.5 s apart) MED -> SLOW -> FAST -> MED -> SLOW and
      SLOW -> FAST -> MED -> SLOW -> FAST (RATE?). A second value tap 1 s after the first did nothing
      (SLOW -> FAST -> FAST -> MED); the chip at 1 s did not skip. Cosmetic, not chased.

## Loud danger alarm (owner 2026-10-24: dangerous misconnects beep at maximum volume; volume kept at 0)

- [x] Emulator, branch loudalarm (docs/power-meter-and-misconnect.md 4.10): src/danger.rs. Dangerous =
      A-PROT while its current flows, A-JACK, a current above the 10 A rating for 1 s; not V-JACK?, LIVE?,
      hints, OVERLOAD (the meter protected itself), the power meter's user current limit, reminders.
      Two-tone burst (3.57 / 2.86 kHz, 4 x 125 ms each second) at the stock's 100 % duty whatever the
      volume; silenced per event by a tap on the badge / toast or any key; re-arms; stops when the
      condition clears; 30 s full, then a burst per 10 s. Settings 3/3 "Danger alarm" Loud (default) /
      Volume / Off, persisted; SCPI SYST:ALER:ALARm, SYST:BEEP:VOL, DBG:ALARm?. Test emu/hy3131/loudtest.py
      (run.py --buzzer-log: TIM14 ARR / CCR1 writes).
- [ ] Device: doc 4.10 "Device check list (danger alarm)" (needs a current in the A jack); the measurement
      coupling check (item 4) and the low tone's loudness (item 5).
      Device c785807 (doc "c785807" D1-D4), no current: Settings row and SYST:ALER:ALARm? read, V-JACK? and
      OVERLOAD leave DBG:ALARm? idle. Not run: the coupling check (no debug command makes it sound; and the
      owner asked for no sound at night). **New: the known false A-JACK in RES with 5 V on V-COM (519fb8a 7b)
      now sounds the loud alarm** (badge only since cdc008a; LIVE? instead since the jack check, doc 4.13) (6 events during the E1 subset). The setting is OFF for the night (owner).

## False AC? / A-JACK / A-PROT / V-JACK? after switches (owner 2026-10-23, on 469f354 / 519fb8a)

- [x] Emulator, branch falsehint (0e8085c, bd6824a, 807ed14). Tool: emu/hy3131/switchtest.py (~60
      switches per connection over SCPI and the MF -> Wave view; every assertion is checked through the new
      DBG:SENTinel:LOG?, so short transients are caught too; real-* scenarios time real alerts after a
      switch in). The plain model shows no false alert on the old code (no front-end transients).
      Two opt-in model knobs reproduce the failures: `ohmexit:3.5;ohmexitlo:1.2`, which replays the
      device trace below, and `swtr:300;swtau:50`, a HYPOTHETICAL ring of PB1 / the V taps after each row
      change (shape not from the device).
      **Device 469f354, check E (397 switches: V-COM open, 1 V, 5 V; the A jack open; raw log
      chk/out/E_all_sent.txt in the session scratchpad):** no A-PROT, no false A-JACK after a switch.
      - E1: RES -> A / mA, V-COM open or at 1 V, 11 of 11 (0 of 4 at 5 V): V-JACK? from 1.47-1.97 s to
        ~5 s. CMP_SENSE CTSTA reads 0 for 0.5-0.7 s, then 0x70 (CMPHO + CMPLO in one read) in every
        window; the last 0x70 comes at 4.2-4.4 s, the last 0x50 0.1-0.2 s later.
      - E2: A at 5 V DC, the Scope page, MENU back, seen once: V-JACK? 1.4-6.0 s; 0x70 from 0.51 s to
        5.24 s.
      - E3: AC? in DC V on a steady 5 V DC, 26 of 26: the V-tap window reads 0x70 at 5 V and 0x10 at 1 V.
      - E4: A-JACK in RES at 5 V (owner decision, below).
      - Other switches into A read 0x70 for under 1 s (no alert).
      So 0x70 is the comparator overdriven or unsettled, not a node level.
      Old code + ohmexit (the E1 trace replayed): 6 false V-JACK? in 24 steps, including AUTO / AC+DC A.
      Old code + swtr: A-JACK on entries to V AC+DC / V AUTO and to diode.
      Causes:
      1. RES exit and the chip restart (E1 / E2): the A rows' comparator on SENSE reads both bits at
         once for seconds, and the old code counted that as a V-jack voltage. Why it lasts ~4 s is not
         known (the netlist is unknown). The stock does nothing special on the exit: R26 / R34 / R36 = 0,
         relays, the row (hy3131_v2_6.h). Device experiment below.
      2. The peak hold (AD1 in AC V / AC mV / AC A, AD2 in continuity / diode / cap) runs from the row
         write. Its first harvest held the whole settle; diode (100 ms settle) took two such windows.
      3. AC+DC / AUTO sample the comparator through the phase settles, from right after the PA relay
         pulse of every phase.
      4. The first comparator window after a settle closed on one sample. The hit run survived a range
         change and dmm.start (Wave view exit, logger, wake).
      5. Hints: one "both" window in every <1 s span kept the 2 s count going. The device also showed
         AC? on 5 V DC and on the 0..9.5 V square (device-check "False A-JACK in Hz", "6. The lost first
         V tap").
      Fix (sentinel.rs, dmm.rs):
      - V-JACK? after an ohm exit or a chip restart (dmm.start): for 7 s (OHM_EXIT_CAP_MS) a 0x70 read
        is no hit. A CMPLO-only read is no hit within 2.5 s (OHM_EXIT_TAIL_MS) of the event or of the last
        0x70 read. A read with CMPHO alone counts at once, so such a source keeps its ~2 s timing. A
        source that itself reads 0x70 or CMPLO alone gets V-JACK? at up to ~8 s; the device's 0x70 / 0x50
        polarity for a real >= 21 V source is not measured yet. After 7 s, 0x70 counts as before.
        V-JACK? is the amber advisory; A-JACK / A-PROT (PB1) are not touched by this rule.
      - Row write: no comparator / peak sample for 150 ms (BLANK_ROW_MS).
      - Settle end: the hold is cleared and samples wait 100 ms more (BLANK_SETTLED_MS); the first
        window is a whole one. In AC+DC the hold restarts at 150 ms and the comparator keeps running,
        so A-JACK / V-JACK? bridge the phases. An empty hold is no window.
      - Dmm::set restarts the hit run (an alert in force stays). dmm.start starts the history over.
        The A-jack probe's in / out rewrites skip the blank.
      - The DMM hints AC? and DC? need the condition in every window for the 2 s. The power meter's
        hints keep the old rule (no report against them; scope kept narrow). powtest `ereset` is the
        known flake (item above): 1 of 2 runs FAIL on 469f354 and on this branch alike. AC? counts
        a V-tap window only with |DC V| < 2 V (AC_HINT_DC_MAX), after the row's first reading, and never
        a read with both bits.
      A-PROT's rule (aprot_trips, the probe, the latch) is unchanged.
      Latency after a switch into the function with the condition present (switchtest, old -> new):

      | alert | old | new |
      |---|---|---|
      | A-JACK, DC V | 0.59 s | 0.79 s |
      | A-JACK, AC V | 0.64 s | 0.86 s |
      | A-JACK, continuity | 0.31 s | 0.41 s |
      | AC? | 2.56 s | 2.76-2.98 s |
      | DC? | 2.67 / 3.32 s | 2.88 / 3.32 s |
      | V-JACK? | 1.75-2.17 s | 1.96-2.32 s |
      | A-PROT, 24 V across A-COM, RES -> DC V | 0.62 s | 0.82 s |
      | V-JACK? after RES, with the model's 0x70 replay (a 24 V source also reads 0x70 while it lasts) | n/a | 5.6-5.7 s |

      A fault that starts after the switch is detected exactly as before. AC? came ~2.8-3.0 s after a
      switch in and still does: HINT_ASSERT_MS is 2 s by design, not retuned (the task said ~1 s).
      What the device confirmed (check E): the V-JACK? (E1 / E2) and AC? (E3) false alerts. No false
      A-JACK / A-PROT after a switch was seen there (A jack open). The peak-hold / first-window / AC+DC
      blanks fix code-level flaws (shown only with the hypothetical swtr knob) and are preventive
      against the owner's A-JACK / A-PROT report; they cost the ~0.2 s above. AC? no longer comes for an
      AC on more than 2 V DC, or one that overdrives the tap in every read. The device read 0x10 (CMPLO)
      for +1 V in DC V's V-tap window, which does not fit the model's V / 10: worth a look (retest 2).
      Not run: switchtest --full (every ordered pair).
      Tests on the final tree: switchtest (all scenarios), senttest 61/61, autorangetest, couptest,
      powtest, acdcmethtest, flowtest, meter-test (switch_blank_rules), the model's unit tests.
      Not changed (owner decision pending): RES with 5 V on V-COM reads OL, not negative, so V_GATE says
      "free" and A-JACK shows (519fb8a item 5). That is a steady state, and the blanks do not touch it.
- [x] Device retest (falsehint image), alerts ON, A jack open unless stated (run on 40728b8; its leftovers
      went to sentinel2 / sentinel3, both run on the device, closed 2026-10-25):
      1. Nothing connected, then the calibrator at 1 V and 5 V DC on V-COM: 20+ switches through DC V,
         AC V, AC+DC, AUTO, A / mA / uA, ohm, diode, continuity, cap, Hz, temp, W, and MF -> Wave -> MENU,
         with RES -> A / mA / AUTO A several times. Expect no A-JACK / A-PROT, no V-JACK? after RES, and
         no AC? in DC V. Read DBG:SENT:LOG? after the run: it lists every assertion with its ms. Expected
         and allowed: DC? in AC V and LIVE? where the source explains it; RES at 5 V is the open item
         above.
      2. DC V with 5 V DC and with the 0..9.5 V 40 Hz square: no AC? (before: AC? at 2.6 s). If a
         symmetric AC source is at hand (>= 0.5 Vpk around 0): AC? within ~3 s.
      3. 24 V DC on V-COM: RES -> DC A and DC A -> Scope -> back, V-JACK? within ~2.5 s, or by ~8 s if
         the comparator reads 0x70 / 0x50 for it; log the ctsta values (they say what a real source reads
         and whether OHM_EXIT_CAP_MS can shrink).
      4. Leads in A / COM, 1-5 mA through A, switch into DC V / AC V / continuity / ohm: A-JACK within
         ~1 s. Then 12 V across A-COM in DC V: A-PROT as before (~0.5 s from the onset).
      5. RES-exit experiment (why 0x70 lasts 3.5 s): RES -> DC V for 1 s -> DC A, and RES -> AC A ->
         DC A. Does DC A still read 0x70? If a pass through a V row clears it, a fast exit sequence can
         replace the tail.
      6. Any remaining false alert: DBG:SENT? for the 2 s after that switch (kind, ctsta, pkmin / pkmax,
         streak, hits) and DBG:SENT:LOG?. These size BLANK_* / OHM_EXIT_*.
      **Device 40728b8 (device-check-2026-10-21.md "40728b8", raw `chk3/out/E_*_raw.json`,
      `ctsta_summary.txt` in the session scratchpad), 302 switches:** 1 PASS: no V-JACK? after RES (0 of 21
      RES -> A / mA / AUTO A / A-tap entries, open, 1 V, 5 V, 8-10 s watches; ctsta still 0x70 to ~4.9 s,
      masked), no AC? in DC V at 5 V (0 of 20), no V-JACK? after A / mA -> Scope -> back (0 of 8), no A-PROT.
      2 PASS (5 V DC and the 40 Hz square). **3 FAIL**: at 24 V RES -> DC A ends in ctsta 0 after the 0x70 / 0x50
      tail (no V-JACK?), and a real V-JACK? is lost after A -> Scope -> back (0x70 to 5.3 s, then 0). A real
      source reads 0x70 for good (TEMP -> A, V-JACK? now at 8.0 s: the cap) or 0x50 for good (TEMP -> AC V -> A,
      1.76 s); DC V 60 V, the RES exit and the Scope round trip leave the A row blind (0), TEMP makes it see. 4 not run (A jack not wired). 5: RES -> DC V 1 s / AC A 2 s -> A: ctsta 0.
      Left: (a) [ ] the 24 V misses (3, and 2b below); (b) [ ] V-JACK? in AC A (2a, unchanged); (c) [ ] new:
      false OVERLOAD in AC A at 1 / 5 V on V-COM, `CONF:CURR:AC 0.06` after 10 A falls back to 10 A at 0.5 s
      (AD1PK_SENSE pk ~-210 000, the 2a drift); (d) [ ] new: A-JACK in continuity at 5 V DC (AD2PK_PB1, 0.41 s,
      stays; LIVE at 1 V), likely the RES 5 V V_GATE item. **(d) fixed by the jack check (doc 4.13): LIVE?.**
      Closed 2026-10-25 (triage): (a) the 24 V misses -> DBG:SENT:PREC default 100 ms (sentinel3, device
      ea73ea5 item 1 PASS); (b) / (c) -> sentinel2 (device ea73ea5 item 3 PASS); (d) -> the jack check. What is
      left is the doc 4.13 device check list (item below).
- [x] 40728b8 follow-up, emulator (branch sentinel2; doc power-meter-and-misconnect.md 4.11):
      (b) V-JACK? in AC A: FIXED. AD1 was on SENSE - PB5, and PB5 is defined only by the DC V
      60 V+ rows: in the A rows it drifted to the rail at any V. Now SENSE - AGND, and AC A counts
      the hold's swing only (2 V rms AC on V); DC on V in plain AC A is no longer flagged (DC A's
      comparator / AC+DC A's DC phase cover DC). VAR +32 = the old row. (c) false OVERLOAD 10 A ->
      60 mA: FIXED by a hold: plain AC A ignores over-range results for 500 ms after the first
      result of a row (AC_OVER_HOLD_MS; it is the RMS / LPF step of the shunt + OP1 input + gain
      change, not the AD1 drift). (a) 24 V missed in DC A: NOT fixed. All of R20..R35 (+ R26 / R34 /
      R36 zeroed) are written for every row; DBG:SENTinel:REG? (new) reads R20..R37 back and the
      A row reads the same bytes after DC V 60 V / RES / TEMP / AC V in the emulator: the history
      is board state, and 24 V is 24 mV at the tap against a 20.8 mV window (3 mV margin). The
      device never read CTSTA 0x20 / 0x60 in ~233 000 reads: CMPLO = "above VRL" (a Schmitt
      pair), so 0 = below the window and the open A row's SENSE sits > 21 mV low. Experiment,
      default off: DBG:SENTinel:PRECond <ms> (the temperature row first). OHM_EXIT_* unchanged.
      (d) continuity at 5 V: documented, not changed (AD2 peak -97 k..+123 k on PB1 = PB1 swinging
      with V-COM, as the RES 5 V owner item). DBG:SENT? ctsta = -1 in non-comparator kinds (was
      stale: "AC V 0x70 after TEMP" was TEMP's read).
      Emulator knobs pb5drift / acastep / vjblind replay 2a, the OVERLOAD and the 24 V table;
      switchtest aca-drift / aca-ovl / real-vjack-blind FAIL on 9fde7f1, aca-drift / aca-ovl pass
      now, real-vjack-blind still FAILs (by design: the open item), real-vjack-blind-prec passes.
- [x] Device retest (sentinel2 image), A jack open, calibrator on V-COM (run on d2aff22; its new bug and the
      PREC default went to sentinel3, closed 2026-10-25):
      1. AC A, V-COM open / 1 V / 24 V DC: CONF:CURR:AC 10 / 0.06 / 0.0006, 30 s each: no
         V-JACK?; DBG:SENT? pkmin / pkmax stay near a constant (no ramp to -262 144). Then
         DBG:SENT:VAR 32 (the old row): the ramp is back (confirms PB5). VAR 0 after.
      2. AC A with an AC source on V (the calibrator's square, or >= 5 V rms AC): V-JACK? within
         ~3 s; note pkmax - pkmin.
      3. 1 V and 5 V on V-COM: CONF:CURR:AC 10, 2 s, CONF:CURR:AC 0.06 (x5): CONF? stays 60 mA,
         no OVERLOAD. If it still trips, read FETC? / DBG:SENT? every 50 ms for 1.5 s after the
         switch (how long the step lasts: AC_OVER_HOLD_MS).
      4. Real overload still works where a wire is possible: manual 600 uA / 60 mA with > 1.03 x
         FS through A: OVERLOAD ~0.5 s later than before.
      5. 24 V on V-COM: DBG:SENT:REG? in DC A after DC V 60 V, RES, TEMP, AC V and Scope -> back
         (expect the same 48 hex digits; a_blind 1 / 1 / 0 / per the previous one / 1). Log ctsta
         per entry as before. Then DBG:SENT:PREC 300 (and 100, 1000): DC V 60 V -> DC A, RES -> DC
         A, Scope -> back: does V-JACK? come (ctsta 0x70 / 0x50 after the precondition)? With V-COM
         open after RES -> PREC -> DC A: any 0x70 tail (false V-JACK? risk: PREC skips the RES-exit
         mask). PREC 0 after.
      6. Continuity at 5 V DC (the owner's RES 5 V decision covers it): DBG:SENT? pkmin / pkmax.
      7. Danger alarm (Loud, volume 0 %): RES and continuity with 5 V DC on V-COM, 30 s each, and
         the E1 subset (RES -> A / mA at open / 1 V / 5 V): the A-JACK badge may show (owner item)
         but DBG:ALARm? events must not grow and the buzzer stays silent; DBG:SENT? last field 1
         (soft) there. With a current in A (if a wire becomes possible): RES / CONT badge only,
         CONF:VOLT:DC -> the loud alarm within ~1 s.
      8. AC+DC A / AUTO A with 24 V DC on V-COM after TEMP (the seeing state): V-JACK? holds
         across the phases (no flapping each AC phase).
      **Device d2aff22 (device-check-2026-10-21.md "d2aff22"; alarm Off, no sound):** 1 no false
      V-JACK? at 1 V / off, but the AD1 peak sits at the rail (-262 144) from ~0.9 s, and at 24 V it
      still ramps to the rail with a swing that gives a 2 s V-JACK? in 60 mA / 600 uA (new bug); VAR 32
      no longer separates (the swing rule). 2 PASS (V-JACK? at 1.9-2.1 s on the 10 / 40 Hz square). 3
      PASS (10 of 10 stay 60 mA). 4 skipped (wire). 5: REG? identical in every DC A entry (no register
      history, confirmed); missed after DC V 60 V / RES / AC V / DC V / Scope -> back, seen only after
      TEMP (7.98 s). **PREC 100 / 300 / 1000 brings V-JACK? back at 1.9 / 2.1 / 2.8 s** after DC V 60 V
      and RES; off and 5 V stay quiet with PREC. -> Next fix: make the precondition the default (100 ms).
      6 continuity 5 V pk -99 100 / +124 800. 7 soft 1 in RES / CONT, DBG:ALARm? events 0 (Off). 8 PASS
      (V-JACK? from 8.3 / 8.8 s, held across phases).
- [x] Device retest (sentinel3 image; doc power-meter-and-misconnect.md 4.12), A jack open (run on ea73ea5;
      item 5 fixed in 25b6ec3, device PASS; closed 2026-10-25):
      (list from the sentinel3 branch, ea73ea5; copied here with the device result)
      1. 24 V DC on V-COM, no DBG:SENT:PREC command (the default 100 ms): DC A / mA after DC V 60 V,
         RES, AC V, DC V auto (60 V) and Scope -> back: V-JACK? within ~2.5 s (Scope -> back: the
         restart mask, ~8 s). DBG:SENT:PREC? answers 100. Source off and 5 V: same entries, no alert.
      2. Switch latency (fastswitch's settle.py / switchlat): DC V -> DC A first reading, once
         right after RES (blind: expect ~+0.2 s, emulator 0.34 -> 0.55 s) and once from DC V 6 V with
         no blind row before (expect unchanged).
      3. AC A at 24 V DC, 1 V and off, 10 A / 60 mA / 600 uA, 30 s each: no V-JACK? (was 1.5-3.5 s
         in 60 mA / 600 uA at 24 V). The square on V: V-JACK? within ~2.2 s (was 1.9-2.1 s).
      4. DBG:SENT:VAR 64 (AD1 at IG 0.9) in AC A at 1 V / off / 24 V: where pkmin / pkmax sit (the
         SENSE level in the A rows). VAR 0 after.
      5. CONF:CURR:AC 10, 2 s, CONF:CURR:AC 0.06, FETC? every 50 ms: +9.91E37 until the first 60 mA
         reading, which is not 1 % of the 10 A value.
      **Device ea73ea5 (device-check-2026-10-21.md "ea73ea5"; RATE FAST, alarm Off, no sound):** 1 PASS:
      PREC? 100; 24 V V-JACK? in every entry: DC A after DC V 60 V 2.76 / 1.88 s, RES 2.16 / 2.16, AC V
      1.76 / 1.85, DC V auto 2.78 / 2.80, AC V -> Scope -> back -> DC A 1.88 / 1.88 s (9 s after the
      back; 3 s after it: 4.98 s = the restart mask), back into DC A 8.03 s (mask); mA 1.41-1.53 s
      (5.03 s when 3 s after Scope -> back); 5 V / off quiet in all 14 entries. 2 PASS: blind DC A
      +94 ms (FAST: 463 vs 369 ms with PREC 0), blind mA +192 ms (540 vs 348); non-blind unchanged
      (360 / 362 ms). 3 PASS: 0 V-JACK? in 9 x 30 s (24 V / 1 V / off x 3 ranges); the square: 1.65-1.94 s, held. 4: SENSE at IG 0.9 sits at
      ~-86 500 codes (~-0.46 V) in every A range at 1 V / off / 24 V (24 V starts at ~-32 000 and
      decays there in ~6-10 s). 5 **FAIL**: no 10 A value carried and no +9.91E37 (the first FETC?
      blocks 0.4-0.9 s), but the first 60 mA reading is **9-44 mA on the open jack in 9 of 16 entries**,
      amber on the screen, decaying over ~0.2 s. New: DC mA entry at MED / SLOW gives a first reading
      of +-33..56 mA (ma1st 3 of 24 MED / SLOW, settle SLOW 2 of 2 from DC V 6 V; none at FAST),
      and `CONF:CURR:DC 0.06` ended on the 10 A row 4 of 11 times at MED / SLOW (OVERLOAD in
      DBG:SENT:LOG?; 0 of 5 at FAST).
      **Fixed in 25b6ec3 (devchk-pre1r2, "DMM first readings after a switch"); emulator only, device check
      below ("First readings after a switch").** v0.1-pre1 reproduced both: AC A 10 A -> 60 mA 28.7-59.5 mA
      first (6 of 6), CONF:CURR:DC 0.06 at MED / SLOW on the 10 A range 5 of 10.
- [x] Danger alarm only for confirmed A-JACK (coordinator 2026-10-24, device c785807: the RES
      5 V false A-JACK sounded the loud alarm at night): ohm / continuity / diode / cap / the
      resistive temperature sensors set Sentinel::soft; danger keys on ajack_alarm() (doc 4.10
      row). loudtest ajack-res / ajack-cont FAIL on 9cfa6a6, pass on sentinel2.

- [x] First readings after a switch (device v0.1-pre1, 2026-10-24, device-check-2026-10-21.md "v0.1-pre1";
      25b6ec3): the entry's first conversion is vetted at every rate (the switch-latency.md proposal 5
      shortcut, which took it unvetted after an R22 change, is gone: DC A -> DC V read 0.90-2.13 V for 1 V,
      DC mA +-44..56 mA / OL, OVERLOAD to 10 A); a held conversion steps up / trips OVERLOAD only as the
      second over-range one in a row (OVER_TWICE); an R22 change within 2 s of the row write, or by the row
      write itself, drops the next two conversions (R22_DROP: DC V 0.376-0.405 V for 1 V after `osr`'s
      0x17, RES 125.9 / 57.3 / 30.0 ohm and 134 kohm after the in-range OSR, the fuse check FAIL,279-286
      ohm on open jacks after the 0x13 write); plain AC A holds results after a row write until two agree
      (ACA_VET). Emulator: firstrdtest (dcma-med / -slow, dcv-acv-med, dcv-res-slow, res-med, fuse-open,
      aca-step: all fail on v0.1-pre1, pass on the fix), model knobs r22part / r22off / r22n / acasteptau
      (shapes INFERRED from the device numbers). Cost: one to two conversions more after a switch at MED /
      SLOW, one AC A result after a row write.
      Device 25b6ec3 (device-check-2026-10-21.md "v0.1-pre1", "Fix on the device"): PASS. AC A 10 A ->
      60 mA first final 1.05 s, max 1.1e-6 A (6 of 6); DC mA entries 0 of 20 on 10 A, max 1.2e-6 A; DC V
      entries 0 wrong of 24; RES 0 wrong of 12; fuse check open OPEN 10 of 10, calibrator 48 ohm OK,47.04-
      47.06 10 of 10. Costs: first value DC V from RES 1.65 s, RES FAST 1.72 s / MED 2.6 s, CONF -> READ?
      DC V / A at MED 1.5-1.8 s (was 0.5-1.0 s with a wrong value possible).
  - [x] The MED / SLOW cost above: R22_DROP drops two conversions after every early R22 change, also
        where the device may not need it (an in-function OSR step did not show the effect). A narrower
        rule needs the mechanism (next item). Done without it (dmm.rs R22_LATE): the step to the slow OSR
        (0x17) waits until R22_DROP_WIN_MS (2 s) after the row write, so no 400 ms conversions are
        dropped; until then the readings come at OSR 8192 (100 ms, unaveraged, no +1 digit), the average
        starts after the step. Ohm's in-range OSR in the window is OSR_FAST (two 100 ms conversions
        dropped), the steady one after it. No drop at the late step: that it is clean is INFERRED from
        the device's later in-function steps (osrstep.py), not measured at 2 s. Emulator (firstrdtest
        with the model's new `r22win:1000`, partials only within 1 s of a row write, INFERRED): no wrong
        value; DC V from AC V at MED first final 0.4-0.5 s and the next 0.1 s later (25b6ec3: the next
        0.9 s later), from RES at SLOW first final 0.5 s (1.3 s), RES at MED 1.8-1.9 s (2.3-2.6 s), DC mA
        gap 0.4 s (0.9 s). Device check: t1rt.py MED (CONF -> READ? DC V / A, expect ~0.5-0.8 s with its
        0.4 s of polling), dcv1st.py (DC V 6 V from AC V / DC A / RES at MED / SLOW: no value off
        > 0.1 %, also around 2 s, where the slow step now lands), t1ohm.py (RES MED first final, no
        wrong value), osrstep.py (unchanged).
        Device f59bac2 (device-check doc "f59bac2: the old DMM bugs"): PASS. CONF -> READ? at MED DC V
        0.36-0.62 s, DC A 0.51-0.60 s (t1rt.py 0.40-0.71 s); DC V 6 V entries 24, largest deviation from the
        settled value 0.048 %, first final 0.33-0.50 s, 0.1 s cadence to 2.0-2.1 s then 0.4 s with no gap;
        RES on 100 / 390 ohm no wrong final, first final MED 1.72-1.83 s, FAST 1.67-2.03 s (open jacks not
        tested: the calibrator's disabled output is no open); osrstep.py unchanged. The +1 digit comes back
        at ~5-6 s after a switch (AVG_FULL 8 x 0.4 s after the step), not at 2 s.
  - [ ] Why the two conversions after an early R22 change read ~32-40 %: UNKNOWN. The same OSR steps later
        in a function (DC V MED / SLOW, 1 <-> 2 V) did not. A stream at 640/s across a CONF change would
        show where the bad conversions sit. The HY3131 datasheet (DS-HY3131-V10_EN 13.1, p. 30) does
        not explain it: AD1 is a sigma-delta modulator with a sinc^2 comb ("Comb Filter^2"; AD2 / AD3
        ^3), AD1CHOP 1x (0x13 / 0x15 / 0x17) swaps the input every OSR period and halves the output
        rate, and there is nothing on an OSR change in a running conversion or on settling. A plain
        sinc^2 with its decimation changed gives one transitional result (~72 % for 8192 -> 32768 at a
        period boundary), not two at ~32-40 %, so no "drop exactly N" follows from it; R22_DROP_N stays
        2 (the device count). (The model's `ad1mix` memory is a sinc^3 shape, also INFERRED.)
  - [ ] AUTO coupling decided at 5.76-5.85 s on the 10 Hz square (3 of 4; one not within 6 s), grab-power
        image; 6bc494a's MEAS:VOLT:AUTO? took 1.28-3.91 s. Not looked into.
- [x] AD1 logger copies (device ea73ea5, AD1 OSR check: right after a 640/s session each conversion
      recorded twice, 52 ms apart, 121 of 294 at 0x15, 149 of 289 at 0x16): logger.rs `sample` took the
      session's first flag-to-flag interval (a stale flag to the first conversion, half a period) as p_meas
      and, with `rescued != 0` at every later flag, never measured it again. Now the period is measured
      between two real flags from the session's second on (25b6ec3). Device 25b6ec3: still 16 copies in
      232 records in 1 of 3 sessions: the flags came 78.4 / 123.2 ms apart in turn. Then (dae568d): a
      rescue that reads the last code again pushes nothing, and a flag right after a rescue that took
      its conversion is not pushed again. Emulator logduptest (model knobs chophalf, chopskew): v0.1-pre1
      28-33 copies per session, 25b6ec3 7-17, dae568d 0. Device dae568d: 0 copies in 6 sessions (998
      records; 640/s, then OSR 8192 / 16384 chopped, 20 s each).

## Owner decisions 2026-10-24 (batch, delegated)

- [x] 1. Install file `dm40_opendm40_v<version>.bin` (build.sh writes it next to opendm40.bin,
      reload.sh stages it, README.TXT / BACKUP.TXT / READMEs name it; a release asset
      `dm40_opendm40_v<version>[-preN].bin`). Any dm40*.bin still installs: vwrite matches the 8.3
      alias (a long name's alias, DM40_O~1.BIN, starts with DM40; fatren-test copies
      dm40_opendm40_v0.1.bin), upload.rs always writes DM40FW.BIN, the FS-loader takes the first
      dm40*.bin (dm40_fwrs.bin, also a long name, installed from the loader screen 2026-10-13).
- [x] 2. One version: Cargo.toml 0.1.0 -> "0.1" (src/ident.rs, tools/fw-version.sh) for *IDN?, the
      34401A emulation ("HEWLETT-PACKARD,34401A,0,opendm40 0.1"), Settings > Device, README.TXT,
      BACKUP.TXT, the CSV headers (with the calibration record's model). sigrok 34401A path in the
      emulator: `sigrok-cli -d scpi-dmm:conn=tcp-raw/...` read samples.
- [x] 3. Power-meter limit 6 A / 30 s in the docs and the wiring render (the code's default).
- [x] 4. FUSE? hint in AC A (0 A below 5 % of the range for 60 s): kept as it is.
- [x] 5. SCPI: IEEE 488.2 suffixes (M milli, MA mega, MHZ / MOHM mega, units per parameter); a bad
      set value leaves the setting and queues -224 / -109 / -222 (no query reply); error queue of 8
      with standard codes, SYST:ERR:COUN?, *CLS clears it; -108 / -102; DIAG refused over
      Bluetooth. tools/scpi-test (host), emu/usbtmc_stress.py (compound cases).
- [x] 6. UI: CH1 "Type Vol/Cur" removed; AUTO does nothing on the generator page; scope CSV with
      the real model, a Wave view current record has time_s,amperes; gallery count = what it shows.
- [x] 7. 60 mV (DC / AC mV r0): on the range page ("60mV"), SCPI (VOLT:RANG 0.06 / MIN), and the
      mV-first AUTO steps 600 mV -> 60 mV below 58.3 mV, back above 61.8 mV (the stock factors;
      the stock has the row, no item and only up-steps in mV). emu/hy3131/mvrangetest.py.
      AUTO+ never enters mV (unchanged). Device 2f56ff9 (device-check doc "2f56ff9" 2): range page item,
      VOLT:RANG 0.05 / MIN, AC 60 mV, DC AUTO 58.0 mV down, 60.4 / 59.0 mV stay, 62.1 mV up, AC AUTO down on
      the hum and up on a square: PASS. Untested: the Wave view opened on 60 mV (its V/div steps start at
      600 mV).
- [x] 60 mV follow-ups (device 2f56ff9, device-check doc "2f56ff9" new bugs 2, 4):
      AUTO -> AUTO+ on the range page while on 60 mV stayed on 60 mV until the function was entered again:
      `set_range` AUTO+ now leaves mV for 6 V at once (also the Wave view / inrush restore of AUTO+).
      AC V AUTO (mV-first) stayed on 600 mV reading 0.606 V for 12 s under a 40 Hz 0 / 9.5 V square: the mV
      input clamps it (a ~0 .. 1.21 V node swing, INFERRED), its RMS stays under 1.03 x 600 mV. Plain AC mV
      now steps up when |LPF| + 1.414 x RMS > 2.2 x the range (MV_CLIP_K; the row is DC-coupled), and AC V
      6 V then holds off the step back into mV until its AC part changes (`mv_clip`). The A-JACK alert does
      not hold the range (the range code does not read it). emu/hy3131/mvautotest.py with the model's
      `mvclamp:1.212;mvpb1:10` (without the fix it reproduces 0.606 V and AJACK,AD1PK_PB1). Device 1a11eda
      (device-check doc "1a11eda: bug-fix recheck" 1, 2): the square from 60 mV: 600 mV at 0.3 s, 6 V at 0.76 s, 4.835 V; no A-JACK, no
      alarm. AUTO -> AUTO+ on 60 mV: 6 V within 0.9-1.0 s (AC and DC).
- [x] **False A-JACK with a voltage on V-COM in RES / continuity / AC mV** (**RES: failed on the device
      1a11eda; the both-bit rule fixes it, PASS on the device c6a3ac3: the jack-check item below**; A jack empty; 519fb8a 7b / E4,
      40728b8 item 4, 2f56ff9 new bug 1; doc power-meter-and-misconnect.md 4.13). RES / RTD and continuity:
      the jack check (AD2 on OP1O - PB3, IG 0.5, while PB1 hits): the input above 2.35 V (ohm) / +0.3 V
      (continuity) or below -0.9 V = driven -> LIVE?, no A-JACK; at its own source (ohm 0.78-2.17 V open,
      continuity -0.48 V) = free -> A-JACK as before. AC mV: V_GATE on the AC result (above). INFERRED: OP1O
      above ~2.4 V on the device (DBG:SENT? fields 14 / 15 show it). Emulator (fail on ad10749, pass now):
      senttest ohm-v5 / ohm-v5-600 / cont-v5 / acmv-sq60 / acmv-sq600 / acmv-dc5, switchtest dc5-pickup /
      sq-pickup / real-ajack-pickup, loudtest ajack-res / ajack-cont-v5 / ajack-acmv / ajack-acv-auto. Model:
      OhmRevOl (default: OL above the OCV, the device), Pb1VjackMv in continuity, AD2 on OP1O.
      Open (doc 4.13 gaps): AC+DC V on manual 600 mV has the same AD1 peak sentinel without the gate.
      **Fixed (9a191ea, branch bugtriage-7ab5c0e):** both phases of AC+DC mV set `vgate`; the AC phase's
      result is the verdict. senttest acdcmv-sq600 / acdcmv-dc5, loudtest ajack-acdcmv: FAIL on 7ab5c0e, pass.
      Cost: a real current in A asserts ~2.2 s after the onset there (the next "free" AC-phase result).
- [ ] Device: doc 4.13 "Device check list (jack check)". **Device 1a11eda (device-check doc "1a11eda: bug-fix recheck" 1): RES still
      raises A-JACK** (badge only, no sound) with 5 V, 10 V DC and the square on V-COM: the jack check reads
      OP1O peaks of 2139-2140 mV (5 V, AUTO on 60 MΩ), 2249-2251 mV (5 V, 600 Ω), 2019-2030 mV (10 V),
      2132-2133 mV (square, AUTO), all under the 2.35 V "driven" line, so the verdict is "free" and A-JACK
      stays for the whole 10 s. Only the square on manual 600 Ω read 2381 mV (driven: no A-JACK, alert NONE).
      So OP1O does not swing above ~2.35 V for a source on V-COM; it sits near the ohm row's own open level
      (600 Ω open: 2.17 V, doc 4.13), and a fixed threshold cannot split them on 600 Ω (inferred). Continuity
      (LIVE?, 974-1760 mV) and AC mV (V_GATE driven, no A-JACK) pass; RES at 1 / 3 V: no PB1 hits. Not run:
      check list item 4 (a current in A); "V-COM open" was the calibrator's disabled output (RES reads
      ~24 Ω on 600 Ω, 0.10 MΩ on 6 MΩ, OL on 600 kΩ), not an open input.
      **Fixed (branch ajres-fix, doc 4.13 "both-bit reads"):** in those runs CTSTA read 0x70 (CMPHO and
      CMPLO in one read: the comparator overdriven), which a current in A (one side of the window) cannot
      give (inferred). In the jack-check comparator rows (ohm, RTD / NTC) a both-bit read within the last 1 s =
      driven: LIVE?, no A-JACK; DBG:SENT? field 14 = 2. The OP1O check stays. Emulator model
      corrected first (OP1O stops at 2.47 - 0.044 x V, `ohmop1k`; >= 4 V on the jack sets both bits,
      `ohmboth`): senttest ohm-v5 / ohm-v5-600, switchtest dc5-pickup / sq-pickup, loudtest ajack-res fail
      on f59bac2 and pass. INFERRED gap: a current in A large enough to push PB1 out of the comparator's
      range may read 0x70 too (LIVE? then, not the badge). Device: doc 4.13 check list items 4-5.
      **Device c6a3ac3 (device-check doc "c6a3ac3: the RES jack fix"): PASS** for item 5 and the
      regression items: RES AUTO / 600 Ω with 5 V and 10 V field 4 = 112 on all but the first poll, field
      14 = 2, LIVE?, never A-JACK, alarm events 0; the square 112 on about half the polls, field 14 = 2, LIVE?;
      1 / 3 V no hits; continuity LIVE? from 1 V, no A-JACK; AC V manual 60 / 600 mV with the square: no
      A-JACK. Still open here: item 4 (a current in A: the A jack is not wired), the mV-first AC AUTO
      (the meter is on AUTO+), RTD / NTC.
- [x] FETCh? does not answer within 5 s while RES AUTO hunts (device 1a11eda, device-check doc "1a11eda:
      bug-fix recheck" note 1): on the calibrator's disabled output, RES AUTO stepped 6 MΩ <-> 600 kΩ (6 MΩ
      reads 0.104 MΩ, 600 kΩ reads OL), DBG:READout? `P,MΩ,NAN`, and 8 of 8 FETC? timed out at the
      driver's 5 s (then -410 Query INTERRUPTED). The scpi.rs timing notes say FETCh? answers +9.91E37 at
      once after a range change.
      **The hunt.** Both readings fit an input that takes at most ~0.2 uA (INFERRED; e.g. a leakage / bias
      current of the calibrator's output stage): 6 MΩ drives 0.215 uA (docs/data/short.csv), the input takes
      nearly all of it, the node sits near 0 V: 0.104 MΩ; 600 kΩ drives 2.17 uA, the input takes a tenth: OL.
      The 600 Ω reading (~24 Ω) does not fit any static element; not explained. AUTO on 6 MΩ stepped down
      (0.104 MΩ is under 600 kΩ's tenth), 600 kΩ read OL and stepped back up, for good; every conversion was a
      step, so no reading ever came (`P,MΩ,NAN`). Fixed (dmm.rs OHM_BAND): an OL on a row a step down reached,
      before any in-range conversion, that the next conversion confirms, steps back to the range it came from,
      which then stays while it reads within x2 of the value it stepped down on (a reading outside x2, OL, a
      function / range / AUTO change end it). AUTO now stays on 6 MΩ and shows ~0.10 MΩ. Limit: a resistor
      connected without an open in between that reads within x2 of the held value (e.g. 100 kΩ after the
      0.104 MΩ) stays on 6 MΩ, one digit fewer, until the reading leaves the band or goes OL.
      **FETCh?** With no reading since the entry it armed a wait, and the ohm hunt extension (meant for READ?
      after CONF:RES) moved its expiry with every step, up to 8 s. Fixed (main.rs): +9.91E37 at once once a
      range step came since the entry (`Dmm::range_moving`), and a FETCh? wait keeps its SCPI_WAIT_MS expiry.
      READ? after CONF:RES followed the same 8 s cap through the endless hunt; with the hunt fixed it answers
      ~0.10 MΩ (emulator 1.37 s).
      Emulator: the model's `isink:<A>` (a current-limited path V-COM, DM40Frontend.cs); emu/hy3131/ohmsinktest.py
      (`isink:1.377e-7`: the model's 6 MΩ test current is ~0.139 uA, so 6 MΩ reads ~0.10 MΩ, 600 kΩ OL as on the
      device). Before: sink-fetch FETC? 8.00 s, sink-med 35 range changes in 9 s with no final readout, sink-read
      READ? 8.00 s "Data stale"; after: FETC? max 1.34 s (the entry's first wait), no range change after the
      hunt, READ? 1.37 s, the 4.7 kΩ swap final 0.3 s after the change shows (sink-leave).
      (verify on device: the calibrator's disabled output, RES AUTO: settles on 6 MΩ at ~0.10 MΩ, `RES:RANG?`
      steady; FETC? every 0.2 s for 20 s answers within 1 s each (+9.91E37 at most for the first ~2 s, no
      -410); `CONF:RES;:READ?` < 5 s; then a 4.7 kΩ / 100 kΩ resistor: AUTO leaves 6 MΩ at once)
      Device c6a3ac3 (its fix not in that image): the hunt is still there, 6 MΩ <-> 600 kΩ every
      0.20-0.26 s, 30-34 changes in 8-10 s, from DC V and from each manual RES range. FETCh? not retried.
      Device 0278d6e (device-check doc "0278d6e"): the hunt's input was not there (the disabled output read
      in range on 600 kΩ, 417-421 kΩ, in 7 calibrator states; AUTO settled, 0 range changes in 19.5 s), so the
      6 MΩ hold and its exits are **untested** on the device. FETC? every 0.2 s: the first (the entry wait)
      0.99-1.05 s with a reading, later ones ≤ 1 ms; forced range steps (calibrator 100 Ω on / off) gave
      +9.91E37 for 0.6-0.8 s, then the value; no -410, no timeout. `CONF:RES;:READ?` 0.99-1.08 s. Swaps from the
      600 kΩ settle: 100 Ω final in 1.7-1.9 s, 390 Ω (no open between) 1.2 s, back to the open 1.6-2.0 s.
      Still to check: the 6 MΩ hold with an input that reads OL on 600 kΩ; 4.7 kΩ / 100 kΩ after it.
- [x] RES AUTO set over SCPI from a manual RES range leaves the screen on the manual reading (device
      c6a3ac3, device-check doc "c6a3ac3: the RES jack fix" note 2): `CONF:RES 6E6`, then `CONF:RES` +
      `RES:RANG:AUTO ON` on the calibrator's disabled output: RES:RANG:AUTO? 1, RES:RANG? hunting, but the
      status bar MANU and a frozen 0.1043 MΩ (DBG:READout? `F`) for 8 s; from DC V the same commands show
      AUTO and dashes. Not tested on a resistor; cause not looked into.
      Device 0278d6e (no hunt that day): from `CONF:RES 6E6` / `CONF:RES 600` the same commands gave
      `DBG:READout?` the manual value for 0.5 s, then the 600 kΩ readings, changing per conversion; the status
      bar not looked at.
      Cause: the status bar's AUTO / MANU is the shown reading's flag (ui.rs), and `Dmm::auto_key` /
      `set_range(None)` only clear `manual`: the manual range's last reading stayed on screen, with MANU,
      until AUTO's first reading. That takes one conversion (or a range step) on a resistor, and the whole
      autorange on the hunt's input (c6a3ac3 had no hunt fix: 8 s; 0278d6e, no hunt: 0.5 s). The AUTO key
      and the range page did the same. Fixed (20e0af9, app.rs `auto_entered`): manual -> AUTO puts the
      placeholder up (AUTO, dimmed dashes), as a function change does. Where AUTO stays on the manual
      range, the dashes now show until the next conversion. `DBG:READout?` field 5 = the status bar's
      AUTO / AUTO+ / MANU. Emulator emu/hy3131/resautotest.py (100 kΩ from 6 MΩ / 600 Ω and by the AUTO key,
      the `isink` input of ohmsinktest from 6 MΩ / 600 Ω, the same from DC V, an open input from 600 Ω):
      on 57e1241 5 of 6 rows FAIL (MANU and the manual final for 0.25-1.1 s after the command), PASS now.
      (verify on device: `CONF:RES 6E6`, 3 s, then `CONF:RES;:RES:RANG:AUTO ON;:DBG:READout?` -> `P,..,AUTO`
      in the same reply; screen AUTO and dashes, never `F,..,MANU` after the command (`DBG:READout?` every
      0.1 s for 8 s); the same from `CONF:RES 600`, from the range page's AUTO item and with the AUTO key;
      a 4.7 kΩ / 100 kΩ resistor: its value within ~1 s; ideally on the calibrator's disabled output while
      it gives the hunt's input)
- [x] DBG:SENT? fields 14 / 15 keep the verdict of the PB1 hits at a switch between ohm-network rows
      (device c6a3ac3 note 3: RES AUTO -> 600 Ω, 0 / 2150 mV for the whole open level; RES -> continuity
      0 / -4 mV for ~3 s). No alert; doc 4.13 check list item 2 expects -1 / 0. Diagnostics only.
      Cause: `jack_t` / `jack_drv` / `jack_mv` were cleared only at a function / family change
      (`Sentinel::configured`), and `diag_text` showed the last verdict whatever run it came from. The
      600 Ω peak (2150 mV, that row's own open level) says the verdict came from hits after the switch
      (INFERRED), so a reset at the switch alone would not do. Fixed (238adb6): `new_run` (a range /
      function change) drops the verdict, and fields 14 / 15 show it only for the present hit run or an
      A-JACK in force, else -1 / 0. Alert decisions unchanged (they took only the present run's verdict).
      After a source on V-COM goes off field 14 now goes to -1 once the hits stop (c6a3ac3 step 3 logged
      2 -> 1). Host test jack_fields_reset_at_a_row_switch (fails without the fix). The model gives no
      hits at a switch, so no emulator case reproduces the device's.
      (verify on device: doc 4.13 check list item 2: RES AUTO -> `CONF:RES 600` and RES -> `CONF:CONT`,
      V-COM open: fields 14 / 15 -1 / 0 once field 8 stops growing; item 5: with 5 V field 14 = 2 while
      field 8 grows, -1 within ~1 s after the off)
- [x] LIVE flaps in RES 600 Ω after a 5 V source on V-COM is switched off (device 1a11eda, same doc note 3):
      DBG:SENT:LOG? +24 entries, the last 8 `LIVE,CMP_PB1` 5-10 ms apart. Which step made them is inferred.
      Cause: once LIVE? cleared (the last live result 1 s old), the next poll re-asserted it from the old
      run (`live_t0` stays until the next ohm reading). Fixed: a run whose last live result is
      LIVE_CLEAR_MS old asserts no more (sentinel test live_does_not_flap_after_the_source). Device: doc
      4.13 check list item 5. **Device c6a3ac3: PASS**, DBG:SENT:LOG? +0 after the off in 5 of 5 (5 V x 3,
      10 V, the square); LIVE? clears 1.0-1.25 s after the off and stays off.
- [ ] False A-JACK / A-PROT from a 66 kHz signal on V-COM (owner 2026-10-26, 0.2.0, A jack empty, modes through
      AUTO): AC mV 600 mV `AJACK,AD1PK_PB1` (26.57 mV), DC mV 600 mV OL with `CMP_PB1` A-JACK then A-PROT, AC mV
      60 mV A-PROT (the latch from DC mV), AC V 6 V a DC? hint (1.162 mV). Cause: the RMS block misses a signal
      above the ~20 kHz AC bandwidth, so AC mV's V_GATE said "free"; plain DC mV had no V gate and its A-PROT probe
      on PB2 read the pickup; DC? compared the V tap's DC with an AC reading that misses the AC part. Fix (branch
      fix/hf-false-ajack, 0.2.1, [doc 4.13](docs/power-meter-and-misconnect.md#413-a-voltage-on-v-com-the-jack-check-the-ac-mv-gate-2026-10-24)):
      the counter above `V_HF_KHZ` (10 kHz) = driven in plain AC mV and no DC? in AC V; DC mV OL = driven, also for
      the A-PROT trip. That the counter sees the signal in those rows is INFERRED (AC mV: not, below). Emulator: senttest acmv-hf600 /
      acmv-hf60 / dcmv-hf600 / dcmv-hf600-slow / acv-hf / acv-auto-hf, loudtest ajack-acmv-hf / ajack-dcmv-hf FAIL
      on 8a0f59d1, pass; aprot-dcmv-24 latency 137 -> 444 ms. The two AC mV ranges disagree on the same input
      (26.57 mV on 600 mV, 2.16 mV on 60 mV, ~12x), so neither reading is the input's RMS (cause unknown).
      **Device 0.2.4 (the MR9270's 66 kHz 0 / 9.5 V square, 2026-10-26): FAIL in AC mV 600 mV** (AJACK on all
      polls, field 12 = 0: the counter did not say driven), AC mV 60 mV / DC mV / AC V pass, DC V AUTO logged
      one `AJACK,CMP_PB1` (CTSTA 0x70 / 0). Second fix (doc 4.13): AC mV |LPF| > 300 mV = driven
      (`MV_DC_DRV_MV`; the clipped square's LPF 0.509 V on the device); DC V / DC mV windows of 0x70 mixed with empty
      reads are no hit (MIXED in `sample_cmp`). Emulator acmv-sq66k-600 / dcv-auto-sq66k / dcmv-sq66k: FAIL on
      0.2.4, pass. **Device 0.2.5 (2026-10-26): PASS** for steps 1-5 below (no alert, no log entry in any; AC mV
      field 12 = 1; AC+DC 600 mV AC-phase LPF 0.509 V). Step 6 (a current in A) not run: the A jack is not wired.
      Device check (alarm OFF, the MR9270 at 66 kHz on V-COM, A jack empty; `SYST:ALER?`, `DBG:SENT?`,
      `DBG:SENT:LOG?`, `READ?` ~20 polls per step; script: hfdev4.py pattern):
      1. AC mV 600 mV, then 60 mV (manual): no AJACK, no new log entry; `DBG:SENT?` field 12 = 1.
      2. DC mV 600 mV: OL, no AJACK / APROT; DC V AUTO: no new log entry (field 8, hits, may stay near 0).
      3. AC V 6 V and AC V AUTO: no DC, no AJACK.
      4. AC+DC 600 mV: `DBG:ACDC?` dcp of the AC phase (the LPF the fix keys on; expect ~0.6 V).
      5. 5 V DC from the calibrator: DC mV and DC V AUTO, no AJACK.
      6. If the A jack can be wired: a current in A with V-COM open in AC mV, DC mV, DC V: AJACK within ~1 s.
- [x] False A-JACK from a large AC signal on V-COM in AC mV (device 2f56ff9 new bug 1: AJACK,AD1PK_PB1 and
      the danger alarm with the A jack empty, the 0 / 9.5 V square, AC V AUTO on 60 mV). The square drives
      the mV input into its clamp; the clamp current shows on PB1, where the AD1 peak hold watches the A-jack
      shunt (INFERRED: the emulator's `mvpb1` models it, ratio unknown). With the clip step (above) AC mV is
      left within ~2 s of the square on the emulator and the alert does not assert there (without the fix it
      did); whether it still would on the device is not known. **Fixed (branch fix/ajack-false, doc 4.13):**
      plain AC mV gates A-JACK by its own result (V_GATE: |LPF| + 1.414 x RMS > 1.5 x the range = driven,
      a "free" result >= 250 ms into the hit run needed). With the false A-JACKs in RES / continuity below
      (the jack check). Device: doc 4.13 check list.
- [x] Ohm test-current label stays on 60 M after 6 M -> 60 M (test_current_label returns without clearing
      its box; device 2f56ff9, both variants). Fixed: the box is cleared when the range has no value;
      tools/ui-render range pairs ending on 60 M against a fresh draw. Device 1a11eda: 6 M -> 60 M clears
      the label in both variants (device-check doc "1a11eda: bug-fix recheck" 3). The labels on the other ranges, diode 0.75 mA and "Overload"
      next to 0L: PASS on the device.
- [x] Bluetooth: stale errors on the first connection (device 2f56ff9, device-check doc "2f56ff9" new bug 5:
      the first `--bt` client found -113, -102, -113 queued; later connections none). USB and BT already had
      separate queues. Cause (INFERRED, no device trace): the module's own text reaches the SCPI parser while
      no host is linked: an `OK` = -113, an echoed `AT+SBNDM40A` = -102 ('+' in a header), `OK` = -113, e.g.
      init_module's AT exchange when an answer comes later than its 50 / 100 ms window + AT_QUIET_MS. Fix:
      bt_scpi.rs `Rx::set_link` (PC14 every poll): with no link no SCPI line or overrun is reported (stock
      frames as before); a line in progress when the link changes is dropped to its terminator.
      tools/bt-scpi-test 7b; emu/btchattertest.py (the module text with no link, then a host: base
      -113 / -102 / -113 / -363, fix 0). Device check: power the meter on, wait 10 s, connect
      `dm40scpi.py --bt` and send SYST:ERR:COUN? first: 0; repeat after a Settings BT off / on. A connect
      banner printed after PC14 rises would still be parsed (would show as -113 there).
      Device f59bac2 (device-check doc "f59bac2: the old DMM bugs" item 6, note 4): **FAIL**. After every
      module power-up (first connection of the session, Settings BT off / on with 10 s and 30 s waits: 5 of
      5) the first client finds 7 errors, always -102 / -113 / -113 / -102 / -113 / -113 / -113; reconnects
      without a power-up 0. The status-bar BT glyph (PC14) never showed with no host (sampled 2-30 s after
      BT on; a short pulse would be missed). Where the lines come from: unknown (not through a PC14-high
      window, INFERRED). New with the fix: a line sent before the meter sees PC14 high is dropped without
      a reply or an error (first query 0-0.3 s after the connect: no reply in 14 of 17 connects; at
      0.5 s 6 of 6 answered), so a client that sends at once loses its first query.
      Cause (device trace, diagnostic image 7b1b6d1): on every module start (power-up, Settings BT on,
      AT+RESET) PC14 reads high for ~650 ms with no host, and 145-195 ms in, the module prints 7 lines
      (`BLE03:VER2.1-20240615`, `CBN:`, `CBMAC:`, `CSN:`, `CSMAC:`, `CBSW:1`, `CSSW:1`): the 7 errors in
      that order. The PC14 gate let them through. The AT init's own answers are not the cause: it runs
      once per boot, and its post-RESET banner fell in the 2 s quiet window. On a connect, the host's first
      bytes come ~0.34 s before PC14 rises. A host can link 0.32 s after power-up, after the banner. Fix:
      bt.rs `StartText` takes all input from each start up to the `CSSW:` line (1 s fallback), before the
      parser sees any of it; PC14 gates nothing (a drop still resets a partial line). Emulator:
      BtModule.cs models the banner, the PC14 pulse and the 400 ms PC14 lag; emu/btchattertest.py A (boot,
      first command at once) and B (Settings OFF / ON, link in the pulse): base FAIL (no reply; 8 queued),
      fix PASS. Device 0e69979 (device-check doc "7b1b6d1 / 0e69979"): first SYST:ERR:COUN? 0 after
      boot and after 3 BT off / on cycles; *IDN? 0-0.3 s after a connect answered 15 / 15; a link 0.4 s
      after BT on answered; stock btpoll 200 / 200.

## Bug triage 2026-10-25 (branch bugtriage-7ab5c0e)

- [x] Plain AC (V / mV / A) autorange step published a 0 reading: the result that stepped the range went on to
      publish `f_ac`, which `set` had just reset (the reset added after device d2aff22), as a final reading on the new range (FETCh?,
      READ?, the screen); then the EMA's 0.99 step from 0 read 1 % low and closed over ~1 s, also at every plain
      AC entry (DC V -> AC V on 1 V AC: 0.990 V first). Fixed (8ed9499): no reading from the stepping result;
      the first used result after a reset seeds `f_ac` / `f_lpf`. Emulator firstrdtest acv-auto-dn / acv-auto-up /
      aca-auto-dn / acv-entry: FAIL on 7ab5c0e, pass. Device check: AC V 1 V 50 Hz, CONF:VOLT:AC 60 then
      VOLT:AC:RANG:AUTO ON, FETC? / DBG:READout? every 50 ms for 2 s: no 0 and no reading 1 % low; the same
      for CONF:VOLT:DC 6 -> CONF:VOLT:AC. [device-check 185c066](../docs/history/device-check-2026-10-21.md#185c066-plain-ac-first-readings-8ed9499-acdc-mv-a-jack-9a191ea-two-open-triage-items-scope-trigger-level-in-volts--nearest-marker-drag-f6c6ac3-2ad7feb-2026-10-25) (the 0 / 9.5 V square, no 1 V sine source): no 0 and no 1 % low
      reading at the 60 -> 6 V step (the first final +7 %: the coupling step); the entry fails differently (next item).
- [x] Plain AC V entry: the first final reads 1/√2 of the input (device 185c066, DC V -> AC V on the square at 20 / 50 /
      90 Hz: 3.39-3.41 V for 4.81 V; the next result 0.15 s later is right). `DBG:ACDC?`: that result is read 4 ms after
      the settle deadline with INTF 0x1f, the next one only 18 ms later, so the chip latched it during the settle
      (INFERRED: half its window before the AC row carried the input). 8ed9499 seeds `f_ac` with it, so it shows as
      final. Fixed (3c8d3fe; AC V / mV only since 5aafba5: AC A's own vet, ACA_VET_*, already holds results
      until two agree, and the drop there put switchtest real-vjack-ac's V-JACK? at 3.42 s, over its 3 s, base 2.89 s):
      after a plain AC V / mV reset (entry, range step; dmm.rs `ac_lead`) the first poll past the settle drops
      what INTF latched (its read clears it) and the next result too (its window began before that read, the device's
      2 % low one); the result after seeds the filters. The first final comes ~0.1 s later (emulator: 0.43 s after
      the entry, was 0.30 s; an autorange step: 0.65-0.71 s between readings, was 0.46-0.54 s). Emulator: the model's `acarrive` (the AC V node carries the AC from 160 ms after the
      entry, INFERRED; the block over it reads 0.706 of the input); firstrdtest acv-entry-sq (the device's square) /
      acv-entry-arr (1 V sine): FAIL on 5a4dbce (3.378 V for 4.776 V, 0.707 V for 1 V), pass; acv-auto-* /
      aca-auto-dn (8ed9499's) pass. Device check (5 min): as [device-check 185c066](../docs/history/device-check-2026-10-21.md#185c066-plain-ac-first-readings-8ed9499-acdc-mv-a-jack-9a191ea-two-open-triage-items-scope-trigger-level-in-volts--nearest-marker-drag-f6c6ac3-2ad7feb-2026-10-25) step 1 (CONF:VOLT:DC 6 -> CONF:VOLT:AC on
      the square at 20 / 50 / 90 Hz, `DBG:READout?` every 50 ms for 2 s, `DBG:ACDC?`): the first final within 1 % of
      the steady value; the 60 -> 6 V autorange step: no 0, no 1 % low reading; also with a sine if one is available.
      `DBG:ACDC?` does not list the two dropped results (the drop comes before its entry): the first one logged is
      ~0.13 s past the deadline. Not changed: AC+DC's phase switches (its own settle, `ac_settle_step`).
      **Device 57e1241 (2026-10-25): PASS.** DC V 6 -> AC V on the calibrator's 0 / 9.5 V square, 2 runs each at 20 / 50 /
      90 Hz: the first final (0.47-0.48 s) within 0.19 % of the steady value (4.796 / 4.810 / 4.826 V). CONF:VOLT:AC 60 ->
      VOLT:AC:RANG:AUTO ON on the 50 Hz square, 4 runs: lands on 6 V, no 0 and no reading 1 % low; but the first finals
      on 6 V read high, +3.6..+4.0 % (5.00 / 4.93 / 4.89 V for 4.81 V), settling within 1 % ~0.35 s after the step
      (item below).
- [x] AC V autorange step 60 -> 6 V overshoot (device 57e1241, 2026-10-25): on the 0 / 9.5 V 50 Hz square the first
      finals on 6 V read +3.6..+4.0 % high (5.00, 4.93, 4.89 V for 4.81 V) and settle within 1 % ~0.35 s after the
      step; the cause is unknown (the AC filter seeded from a block that spans the range switch is one candidate,
      INFERRED). Script: scratchpad acauto.py pattern (CONF:VOLT:AC 60, 3 s, VOLT:AC:RANG:AUTO ON, DBG:READout?
      every 50 ms for 4 s).
      Cause: not the filter (ACV_WIN lets a > 10 mV change through at 0.99). The RMS results themselves are high:
      6 V and 60 V share a tap, OP1's gain changes, its output's DC steps and the coupling into PB4 (tau 0.34 s,
      [hy3131-frontend 2](docs/hy3131-frontend-2026-10-26.md)) carries it to AD2. `DBG:ACDC?` 0.2 s after the
      settle (device 0.2.1): LPF 1.33, 0.99, 0.74, 0.55, 0.41, 0.31 V, and RMS = sqrt(AC^2 + LPF^2) within ~0.1 V
      throughout. 6 -> 60 V (manual) is far worse: LPF -25 V first, finals 21-26 V for 4.84 V, ~2 s.
      Fixed (fix/acv-autorange-overshoot, dmm.rs ACV_DC_*): after a plain AC V range step a result is provisional
      (grey, no step down) while |LPF| > 0.05 x RMS + 0.5 % of the range, at most 2 s after the row write. Emulator
      (HY3131.cs HpfTransientGain 1.5 -> 1.0, the device's sqrt(AC^2 + LPF^2)): firstrdtest acv-auto-dn-sq (+-0.3 %)
      and acv-up-sq (+-0.5 %) FAIL on 0.2.1 (4.802 V, 11.3 V for 4.776 V), pass. Device (fix build, the
      square, 4 runs each): 60 -> 6 V at 50 Hz first final +0.08..+0.20 % at 1.29-1.40 s (base +3.6..+4.5 % at
      0.68-0.78 s), at 20 Hz -0.15..0.00 % at 1.35-1.40 s (base +4.2..+4.5 %); 6 -> 60 V first final at the
      2 s limit, +0.30..+0.64 %. A step on a signal without DC is not delayed (emulator 1 V sine: 0.71 s, as base).
- [x] AC+DC mV (manual 600 mV) A-JACK after the voltage goes off (device 185c066): none while the square or 5 V is on
      (13 runs), but `AJACK,AD1PK_PB1` 0.45-0.9 s after the source is switched off in 8 of 13 runs, held while the
      input is open (the readout OL: a charge left on the floating mV input). Plain AC mV, the same cycles: 0 of 6.
      How the AC phase's free verdict meets the hit run there is not traced (sentinel.rs `decide`). [device-check 185c066](../docs/history/device-check-2026-10-21.md#185c066-plain-ac-first-readings-8ed9499-acdc-mv-a-jack-9a191ea-two-open-triage-items-scope-trigger-level-in-volts--nearest-marker-drag-f6c6ac3-2ad7feb-2026-10-25) note 2.
      Fixed (9e49722): a DC phase result whose magnitude is over the range (the readout's OL; dmm.rs `mv_dc_drv`) makes the
      AC phase's V_GATE results say driven until the next DC result, so the "free" AC result (|LPF| + 1.41 x RMS under
      1.5 x the range) no longer asserts A-JACK while the input reads OL
      ([doc 4.13](docs/power-meter-and-misconnect.md#413-a-voltage-on-v-com-the-jack-check-the-ac-mv-gate-2026-10-24)).
      Emulator: the model's `mvhold` (INFERRED: the open input keeps 0.75 V, PB1 picks up 40 mV, until the next source;
      `mvholdrows:1`: only the DC mV row sees it); senttest acdcmv-sq600-off / acdcmv-dc5-off / acdcmv-off-dcrow: FAIL on
      5a4dbce (A-JACK ~1 s after the off), pass; their last phase (a current in A) and acdcmv-sq600 / acdcmv-dc5 /
      acmv-* still A-JACK. Not covered: an open input that reads within the range while PB1 still hits (no device
      trace of it). Device check (10 min): as [device-check 185c066](../docs/history/device-check-2026-10-21.md#185c066-plain-ac-first-readings-8ed9499-acdc-mv-a-jack-9a191ea-two-open-triage-items-scope-trigger-level-in-volts--nearest-marker-drag-f6c6ac3-2ad7feb-2026-10-25) step 2 (alarm OFF, AC+DC manual 600 mV, the square / 5 V on and
      off, 6 runs each, `SYST:ALER?` + `DBG:SENT:LOG?` every 0.1 s for 6 s after the off): no A-JACK; then a current in
      A (V-COM open): A-JACK.
      **Device 57e1241 (2026-10-25): PASS.** Alarm OFF, CONF:VOLT:ACDC 0.6, the calibrator's 50 Hz square and 5 V DC,
      6 runs each: SYST:ALER? / DBG:SENT:LOG? every 0.1 s for 6 s after the off, no A-JACK in 12 of 12. Not done: a
      current into A with V-COM open (needs a current source on A).
- [x] **False A-JACK in continuity when the probes touch** (owner video 2026-10-26, leads in V / COM, A jack empty):
      A-JACK for ~1.5 s while the readout went OL -> 814 -> 277 -> 28.85 -> 0.67 ohm. The jack check calls a shorted
      input (~0 V) "free" as it does the open one (~-0.48 V), so PB1 hits during the contact asserted A-JACK; what
      moves PB1 then is unknown (not recorded on the device). Fixed (branch fix/cont-false-ajack, 0.2.8): continuity
      and diode gate A-JACK on their reading (V_GATE): under OL within 1.5 s = V-COM closed, no A-JACK; an OL reading
      counts from 450 ms into the hit run ([doc 4.13](docs/power-meter-and-misconnect.md#413-a-voltage-on-v-com-the-jack-check-the-ac-mv-gate-2026-10-24)).
      Cost (emulator): A-JACK 0.6-0.9 s after the onset (was 0.16-0.36 s), up to ~1.5 s right after a part left
      V-COM; none with a part on V-COM and a current in A. Emulator: model `contbump`,
      senttest cont-bounce / diode-bounce (FAIL on 6c686ce8, pass), cont-a / diode-a now open V-COM in their current
      phase, cont-closed-a.
      Device (the MR9270's resistance output on V-COM, its output relay on / off at 0 ohm x 8, 0 <-> 400 ohm x 12):
      every step puts AD2 peaks of 14 000-231 000 codes on PB1 (threshold 9 900), streaks of 1-2 windows, the jack
      check "free" (-435 / -87 / 0 mV). Main 6c686ce8: A-JACK on 3 of the 8 relay closures, shown 0.3-1.5 s after
      each; the fix: none in 28 steps. Not run: probe taps by hand (owner session pending), a current in A.
- [x] `MEAS:VOLT:DC?` answers at ~1.5 s, before the full average (device f59bac2 note 5): as decided on
      2026-10-14 (SCPI_SETTLE_MS for the 2 s PyVISA default; TODO "SLOW reports every conversion"). The
      dmm.rs `AVG_FULL` comment said MEAS? waits for the full window; since R22_LATE that window comes ~5.3 s
      after a switch, so MEAS? answers with the OSR 8192 reading at the limit. Comments fixed (5afb143).
- [x] Back-to-back full-frame `DBG:LCD?` grabs freeze the panel (device f59bac2 note 3): by design. Every
      `DBG:LCD?` chunk holds drawing for 300 ms (main.rs `draw_hold_until`, a consistent frame across the
      chunks of one grab), so grabs less than 300 ms apart never let a frame through. Not the screenshot
      (`screenshot::frozen`, the item "Device: screenshot preparation"). Space grabs >= 0.4 s apart.
- [x] AC V entry from AC mV in the emulator: READ? 2.78-3.9 V at 0 V right after 600 mV -> 6 V (mvautotest,
      firstrdtest-style probe), and under the 0 / 9.5 V square a 6 -> 60 -> 6 V excursion (mvautotest "ranges
      [... 6.0, 60.0, 6.0]") with a 4.90 V tail. Cause in the model: the AC node's coupling capacitor sits at
      `Op1OffLevel` (-0.6 V, INFERRED) while OP1 is off (AC mV) and recharges on the AC V row. The device does
      not show it where it was looked at: f540f0d 2a went 60 mV -> 600 mV -> 6 V (no 60 V) and read 4.835 V
      (the Wave view's 4.84 V); DC V -> AC V gives a small tail (33 mV from 0.5 V DC). No firmware change
      (a vet would cost ~0.1 s per AC entry for a step the device may not have). Device check (2 min): 0 V on
      V-COM, CONF:VOLT:AC 0.6, 3 s, CONF:VOLT:AC 6 (and VOLT:AC:RANG 6), DBG:READout? every 50 ms for 3 s; the
      square from CONF:VOLT:AC 0.6 -> CONF:VOLT:AC with VOLT:AC:RANG? every 50 ms. A first final above ~0.1 V
      at 0 V or a 60 V visit = real: then hold plain AC V results after a row write until two agree
      (as ACA_VET) and let held results move no range; else lower the model's level.
      [device-check 185c066](../docs/history/device-check-2026-10-21.md#185c066-plain-ac-first-readings-8ed9499-acdc-mv-a-jack-9a191ea-two-open-triage-items-scope-trigger-level-in-volts--nearest-marker-drag-f6c6ac3-2ad7feb-2026-10-25): first final 0.018-0.036 V at 0 V (4 runs), `VOLT:AC:RANG?` 6 V only under the square (3 runs; then
      a +20-23 % overshoot 0.3 s after the first final, steady after ~1.3 s). So the model's level is what to lower.
      Done (fix/regressions-1025): `Op1OffLevel` -0.6 -> -0.005 V. The model now reads 0.027, 0.018, 0.012, 0.008
      ... 0.003 V by 1 s at 0 V (the device 0.018-0.036, 15-17, 9-10, 5-6, 3 mV), and under the square 6 V only
      with a first final of 5.42 V (device 5.03-5.39 V). The device's +20-23 % overshoot 0.3 s later is not
      modelled (the model decays from the first final). Tests: firstrdtest acmv-acv-zero / acmv-acv-sq and
      mvautotest "no 60 V range on the way up" fail with -0.6 and pass with -0.005.
- [x] Inrush page "Armed: 0.069 A now" with nothing in A (device 2f56ff9 7d; docs/inrush-current.md 9): the
      live value is the rms about the zero code from the AC A 10 A row's LPF calibration offset
      (`ad2_scale`, cal 0x10C), so a DC offset of the AD2 stream against it, or noise, both show; which one is
      not known (the emulator has no offset there). Device check (2 min): AC A 10 A (CONF:CURR:AC 10), A jack
      open, `dm40scpi.py log out.csv --source ad2 --rate 9600 -d 1`: the mean of the samples (A) against the cal
      0x10C offset, and their rms about the mean. Mean off = arm with a measured zero; noise = the plan's
      ~20 mA figure is what changes.
      [device-check 185c066](../docs/history/device-check-2026-10-21.md#185c066-plain-ac-first-readings-8ed9499-acdc-mv-a-jack-9a191ea-two-open-triage-items-scope-trigger-level-in-volts--nearest-marker-drag-f6c6ac3-2ad7feb-2026-10-25): mean +150.4 codes = +0.0717 A, rms about the mean 1.72 mA; the scale's offset is 0 A (`LOG:SCALe?`;
      the 10 A row's cal offset 0x110 not set or not used). An offset: arm with a measured zero.
      Fixed (18532a6): the cal record has 0 at 0x10C..0x120 (all AC A LPF offsets; gain 1), so every arm measures the zero
      (inrush.rs `Cap::arm` / `zero_in`: 10 ms skipped, the mean of the next 100 ms when it is within 0.2 A of the zero
      given and its rms below 50 mA, else the zero given; that is the last one measured since boot, else the cal's:
      app_inrush.rs `inr_zero`; it survives a page close because nothing resets `Session::cap` there);
      `INR:PREamble?` a = minus that zero. Limit: a DC load under 0.2 A
      already flowing at the arm is taken for the zero
      ([inrush-current.md 11](docs/inrush-current.md#11-limits)). Emulator: the model's `acaoff:0.717` (+0.0717 A in
      the AC A rows' AD2, INFERRED where it arises); inrushtest (now nothing in A before the lamp step): FAIL on 5a4dbce
      (`INR:PRE?` a 0), pass (a -0.0717 A, idle below 20 mA); host tests inrush::measured_zero_*. Device check (2 min):
      A jack open, MF -> Inrush: "Armed: 0.00x A now", `INR:PREamble?` a ~ -0.07 A; a lamp capture's idle below
      ~20 mA.
      **Device 57e1241 (2026-10-25): PASS.** A jack open, INR:STAT ON: ARMED, `INR:PREamble?` a -0.0757 A. The lamp
      capture's idle not checked.

## Failing / flaky tests 2026-10-25 (branch fix/regressions-1025, from 5a4dbce)

- [x] emu/hy3131/btwaketest.py failed (on 9fbfc29 and on 5a4dbce): the BT host's first line got no reply, so
      the test waited until the emulator ended. Firmware: `Bt::init_module` reads the AT+SLED0 answer, which
      sets `quiet_until` 500 ms ahead (AT_QUIET_MS), then sends AT+RESET. A host links 0.32 s after a module
      start (device), inside that window, so its first line was taken as a late module answer and dropped.
      Fixed: a module start (AT+RESET, a power-up, cmd 07's restart pulse) ends the quiet window; `StartText`
      already takes everything up to the end of the start-up text. btwaketest fails on 5a4dbce and passes with the fix. Device: not checked.
- [x] emu/wavedroptest.py --host-read failed with "IMPORT refused, status 1" (also on the base build): a test
      harness bug. It imported the USB/IP device a second time for the MSC reads while the USBTMC import was
      open, which the server refuses. Fixed: the MSC reads use the same import (`uc.Msc(t.d)`, as
      wavetest.py does). It passes now: 1.17 MB of INFO/LOADER.BIN read in 10 s at 1 ms/div, 0 dropped.
- [x] emu/logfmttest.py --bt-port ended once with -113. Cause in the BT model (emu/peripherals/BtModule.cs): its AT
      parser ran on every firmware byte, also while a host was linked, so an "A" "T" pair inside a reply
      (LOG:DATA? PACKed / DLG blocks, a stock cmd 09 frame) started an "AT line" and the model sent "OK" into
      the firmware's UART, which ran it as a host line (-113 Undefined header). Fixed in the model: while a
      host is linked an AT line has to start a line of the firmware's output (after CR / LF or 2 ms of quiet)
      and be one the firmware sends linked (AT+SBN / AT+CBN for cmd 07 / 08; also AT+SLED / AT+RESET). What
      the real module does with AT text while linked is not known. emu/btchattertest.py case C (the DMM at
      2.1569 V: each cmd 09 reply ends in "AT"): 5 x -113 with the old model, 0 with the new one.
- [x] emu/hy3131/wavetest.py (volt) failed "a host reading SCOPE/SCOP000.CSV: no region C code while the stream
      ISR owns the bus" (also on 0655312): `csvlog::parse_path`, the MMEM name argument, sat in .cold
      (link-native.x, 0b9d580 / 3da128a), and the test's MMEM:DATA? of the DLG runs it while the Wave view
      streams (on the device a region C fetch stalls the ISR entry ~20 us). There is no cold half to split
      off (MMEM:DATA? and MMEM:DELete both need the prefix check, `fatdir::dir_of` is already in B), so it is
      back in region B, written more compactly (the same results: fatren-test `names`, and the old and the
      new function compared on 3 M random names). Region B free 15 052 -> 14 748 B (the build.sh warning
      stays; the link fails below 8 KiB). wavetest volt fails on 5a4dbce and passes; dcv / current pass.
- [x] [emu/hy3131/modetest.py](../emu/hy3131/modetest.py) stock cap-big (100 uF) read 104.1 uF against 100 uF +- 3 %;
      on 5a4dbce it got no readings. The 104 is the stock's own arithmetic, not the HY3131 model or the test setup
      (`stock/re/analysis/dmm_v2_6.h` (in the private analysis repo) section 4; FUN_08013734 case 0xd in
      `stock/re/analysis/app_decompile.c` (in the private analysis repo)): C = gain * I * dt / median5(dV); a read is due 255 ms
      after the previous one (deadline = that read + 0xff) and runs on the DMM screen's 10 ms poll, so dt = 260 ms,
      while dV spans whole 50 ms AD1 conversions (ramp row OSR 4096, chopper): 5 between most reads, 6 between one
      in 5, median 250 ms. 260 / 250 = 1.040; the r5..r7 calibration gains are 1.0. With the model's ramp-row AD1
      period set to 52 ms (an experiment, reverted) the stock read 100.1 uF. modetest now expects 104 uF from the
      stock (`stock_want`, same 3 % tolerance) and 100 uF from opendm40 (101.9 .. 102.1 uF), and prints run.py's exit code and
      last line when the emulator does not start. That the real meter also reads +4 % is inferred, not measured. The
      5a4dbce "no readings" is inferred to be the setup: a worktree without `dm40_v_2_6.dec.bin` at the top (run.py
      exits 1 with FileNotFoundError, modetest showed "0 readings") or without `private` (no genuine check pass)
      gives no readings; the 5a4dbce log is not at hand. Notes in [emu/hy3131/README.md](../emu/hy3131/README.md)
      under the modetest results table.

## Scope trigger-level drag lag (owner 2026-10-25, branch fix/scope-drag-lag)

- [x] "Dragging the trigger level feels laggy". Each move report of a level drag repaints the frame held since
      the press ([src/scope_page.rs](src/scope_page.rs) `sp_move`: no acquisition while the finger moves);
      [emu/draglat.py](../emu/draglat.py) times report -> marker on the screen in emulator time. At
      `--mips 110` (the device model, [emu/peripherals/CpuCost.cs](../emu/peripherals/CpuCost.cs)), 5 ms/div:
      a trigger-level step was a 10-20 ms pass, the marker on the screen 15 ms after the report: the guide line's
      old and new rows repainted per graph column (two LCD windows in each of 399 columns,
      [src/scope_ui.rs](src/scope_ui.rs) `Graph::flush`), and the trigger badge (3.3 ms) drawn before the graph.
      Fixed: changed horizontal lines painted as row runs between the holes after the columns; the badge after
      the graph. Report -> marker 7 ms (median of 15 move reports); CH1 drag 3 ms before and after (its marker is in the
      strip, drawn before the graph). The pass is still 10-20 ms per trigger-level step (the marker columns and
      the badge). ui-render: each drag step equals its fresh draw (tabs, cursors, FFT boxes over the guide row).
- [ ] Device check (5 min): scope page, 5 ms/div, Auto, a signal on CH1. (1) Drag the trigger-level marker slowly
      and fast, up and down: marker and dashed guide line under the finger, no dashes left behind on the old
      rows, the badge value follows. (2) The same with the corner tabs shown (Settings "Scope tap step"
      Corners) and with the X / Y cursors on: the guide line does not paint over the tabs / cursor tab, the
      cursor lines stay whole where the guide line crosses them. (3) `DBG:PROF:RESET`, one 1 s level drag,
      `DBG:PROF?`: note `s.frame` max_us and the loop histogram (emulator after the fix: s.frame max 12.5 ms).
      (4) CH1 and trigger-position drags as before; after the release `TRIG:LEV?` is the dragged level.
- [ ] After the release the trace re-triggers only with the next acquisition (the release applies the level and
      restarts it, `Scope::apply`). Inferred from [src/scope.rs](src/scope.rs) `auto_timeout` (500 ms at 5 ms/div):
      a level outside the signal shows the next (untriggered) frame after ~0.5 s, the "updates at the
      acquisition rate" seen after a drag; a level inside it, a triggered frame within ~0.1 s (a 5 ms/div record is 53 ms). Not
      measured: the emulator shows no frames at 2 ms/div and slower with the comparator trigger on (DBG:PROF?
      0 fps at 2 / 5 / 10 ms/div, 10.5 fps at 1 ms/div; [emu/README.md](../emu/README.md): "the comparator path
      sometimes never completes a frame after a restart"). Device check: drag the level outside the signal ->
      untriggered frame after ~0.5 s; back inside -> a frame within ~0.1 s.
- Not changed: the touch-down debounce ([src/touch.rs](src/touch.rs) `Debounce`) reports the second pressed
      frame's point as the down point, so a finger that already moves in its first report loses that
      report's travel (5 px in `emu/draglat.py --still 0`); a landing finger (one still report) loses none.
- [x] Side keys "lag", "AUTO mixed with RUN", "as if related to AutoHold" (owner, build 57e1241 and later). Cause:
      [src/gesture.rs](src/gesture.rs) `Tracker::step` turned a change of the debounced key mask into one gesture,
      with the code of the lowest changed bit and "down" if any key was down in the new mask. The debounce
      ([src/touch.rs](src/touch.rs) `Debounce`) goes from one key straight to another when the release frame is
      replaced inside its 40 ms by the next key's report (or the release report is lost / overtaken in one read
      after a main-loop stall): RUN -> AUTO gave KeyDown(RUN) for the AUTO press and no KeyUp(RUN), so
      [src/app.rs](src/app.rs) `run_down` stayed set and AutoHold toggled 1 s later (it is persisted, so it then
      stays on over a power-off); MENU -> any key gave MENU again ("the previous press at the next one").
      Emulator (CST_Touch model, RUN 150 ms, AUTO 20 ms after its release): `DBG:TOUCH?` `G,K,1` twice, then
      `G,k,2`; `CALC:AHOL?` 1. Fixed: `Tracker::key_step` reports one key per call, releases first, the rest at the
      next event or tick (<= 10 ms): `G,K,1 G,k,1 G,K,2 G,k,2`, `CALC:AHOL?` 0. Host test:
      [tools/touch-test](tools/touch-test/main.rs) `key_tests` (failed 3 of 4 before). Commit "Side keys: one gesture per key change".
      Main-loop stalls (emulator): the save after a boot with the W25Q128 erase at its 400 ms datasheet maximum
      made a 370 ms pass (`DBG:PROF?` tick max); keys pressed during such a pass were not exercised (the
      timed saves under test wrote blank slots, no erase). One EXTI edge is latched for the whole pass, so a
      release and the next press inside it read as one key-to-key change: the case fixed above (inferred).
- [x] Device check (3 min): (1) `CALC:AHOL?`; if 1, `CALC:AHOL OFF` (or hold RUN 1 s): AutoHold may be on from this
      bug. (2) DMM page, `DBG:TOUCH 0`; press RUN, release, and press AUTO at once (roll the thumb down the strip),
      10 times: no A-HOLD label appears, HOLD is off after each AUTO, `CALC:AHOL?` 0. (3) Press MENU then MF quickly:
      the range page opens (not the menu twice). (4) `firmware/tools/touch-log.py --raw`: every key shows
      `G,K,<code>` and `G,k,<code>` in pairs with its own code. (5) Hold RUN 1 s: A-HOLD on, again: off (unchanged).
      - Device 0.3.0 mute (2026-10-27): (1) `CALC:AHOL?` 0. (5) by `DBG:KEY 1,1200` in the DMM view: A-HOLD on
        (`CALC:AHOL?` 1), again: off. Left for the owner: (2)-(4) with the physical keys (`DBG:KEY` goes to the app
        directly, past the touch debounce and `gesture::Tracker`).
      - Owner, test build 0.3.1 (all open PRs, 2026-10-29): (2) and (3) with the physical keys as expected. (4)
        not logged: the key-report question below stays open.
- [ ] Unknown (device data decides): whether the 0x5A controller repeats the key report while a key is held, and
      whether it ever drops the release report. The (4) log above shows it (frames `F,e,k<code>` per press). If
      releases are dropped, a held RUN still toggles AutoHold after 1 s; a key timeout would then need the
      repeat period. Every key-down is accepted 40 ms after its single report (`DEBOUNCE_MS`; the stock has no
      debounce, `stock/re/analysis/touch_v2_6.h` (in the private analysis repo) "No other debounce"): inferred, not the cause.
- [x] AUTO key range cycle (owner 2026-10-25): V / A / ohm, [src/app.rs](src/app.rs) `auto_key_end` on the release
      (short) or after `RUN_LONG_MS` held (long; the RUN key's timer `run_down`, `run_key` 2). Short: autorange ->
      the present range fixed; manual -> the next higher item of `ui::range_items` (60 mV, 60 ohm included), the
      highest wraps to the lowest. Long: V AUTO+, A and ohm AUTO. Both clear HOLD and REL; AutoHold kept
      (`Meter::set_hold` does not touch `ahold`). Power meter, capacitance, continuity / diode, Hz / temperature,
      scope, inrush, generator: as before. The short press acts on the release now, not at the key-down (so a
      long press does not step a range first). Emulator test: [emu/autokeytest.py](../emu/autokeytest.py).
      The test's two DBG:READout? checks were stale (4 fields, HOLD read from the last one) after the range field was
      added: now 5 fields, HOLD from the fourth; the firmware releases HOLD on a short press (PASS).
- [x] Device check (2 min): DC V with ~1 V in: AUTO -> MANU 6 V; AUTO x6 -> 60, 600, 1000 V, 60 mV, 600 mV, 6 V
      (relays click per step); hold AUTO 1 s -> beep, AUTO+. RUN (HOLD), AUTO: HOLD off. AutoHold on (hold RUN 1 s),
      AUTO: A-HOLD stays. Ohm open: AUTO, AUTO -> 60 ohm. Scope page AUTO: auto-set as before.
      **Device 0.3.0 mute (2026-10-27), keys by `DBG:KEY`, MR9270 1 V on V-COM: PASS.** AUTO -> MANU 6 V, then 60, 600,
      1000 V, 60 mV (OL), 600 mV (OL), 6 V; AUTO 1.2 s -> AUTO+. RUN: HOLD 1; AUTO: HOLD 0, 6 -> 60 V. RUN 1.2 s:
      A-HOLD (`CALC:AHOL?` 1); AUTO: A-HOLD stays, 60 -> 600 V. Under A-HOLD the status bar keeps the held reading's
      range ("60V") while the bargraph scale shows the new range (+-600). Ohm (the calibrator's output off, not an
      open input: 484 kohm): AUTO -> MANU 600 kohm; manual 60 Mohm, AUTO -> 60 ohm. Scope page AUTO (input open):
      20 ms/div, 1 V/div, level 0 V. The beep is not checked (mute build).

## Hz below 7 Hz (owner 2026-10-25, v0.2.0 with the MMB3.0: 2 Hz and 5 Hz gave no reading; branch fix/low-freq, merged #43, v0.3.0)

- [x] Cause: the HY3131 counter is reciprocal (DS-HY3131 14: after the 0x40000-tick minimum, 53.3 ms, it counts
      until a whole number of input cycles, then stops), but [src/dmm.rs](src/dmm.rs) read and restarted it blindly
      every 350 ms, as the stock does. A gate takes up to two input periods + 53.3 ms (the wait for its opening
      edge, the minimum, the closing edge): over 350 ms below about 6.7 Hz, so such gates were cut off (no result,
      or a partial count past the overflow). Fixed: `gate_check` reads the counter from 350 ms after the restart,
      then every 20 ms, and takes the counts once CTA stands still past the overflow (the counter stopped); it gives
      up after 2.2 s without counts (no signal at once: `Freq::take` `quiet`), 4.4 s with counts, or 3.5 periods of the
      shown value + 150 ms (at least 350 ms). Hz and the AC V / AC A secondary readout (`freq_secondary`; AC+DC keeps
      the stock's blind read: its phases change the source). Hz shows the small extra digit when one SYSCLK tick
      over the gate is at most one count of it (`freq_ext`). Tests: [tools/meter-test](tools/meter-test/main.rs)
      `freq_gate_low_frequencies` (a simulated gate, 0.5 Hz .. 1 kHz, every phase), `freq_gate_limits`,
      `freq_extra_digit`, `freq_take_quiet_gate`; emulator [emu/hy3131/freqtest.py](../emu/hy3131/freqtest.py)
      `low-0.5` .. `low-50`.
- [ ] Unknown (device data decides): that CTA stands still after the gate's end on the chip (datasheet: "then it
      will stop counting"; the emulator models it so). If it does not, every gate times out: Hz reads 0.000 Hz at any
      frequency (step 1 below shows it at once). Inferred, not measured: the gate opens on an input edge (the 10 Hz
      readings of the recording are within 27 ppm; a gate from the restart would be off by up to a period); a chip
      that misses the opening edge needs a period more: covered above ~0.9 Hz (the 2.2 s window without counts).
- [ ] Device check (5 min, MMB3.0 frequency output on V-COM): (1) Hz function, 10 Hz: a reading within 1 s, new values
      every ~0.35 s, the reading shows the small digit (10.000x Hz). (2) 50 Hz, 1 kHz, 10 kHz as in
      [docs/data/freq-steps-2026-10-25.csv](docs/data/freq-steps-2026-10-25.csv): the same values as that recording
      (50.000 Hz without the small digit, 1.0000x kHz with it). (3) 5 Hz: a reading (~5.0000x Hz) within ~2 s, a new
      value every ~0.4 s or faster. (4) 2 Hz: a reading within ~3 s, new values about once a second; duty ~50 %.
      (5) Output off / leads apart while 10 Hz shows: 0.000 Hz within ~1 s; Hz entered with nothing connected: 0.000 Hz
      after ~3.3 s ("--" before). (6) AC V with 2 Hz on the output (if the MMB3.0 gives an AC
      level at 2 Hz): the frequency above the reading reads ~2 Hz. (7) Optionally a CSV recording at 2 / 5 / 10 Hz
      (Log, Hz) for docs/data.

## 500 ohm from the MMB3.0 reads OL on 600 ohm (owner 2026-10-25, v0.2.0, branch fix/ohm500-ol, merged #7)

- [x] "500 Ом с калибратора вызывают оверлоад на пределе 600 Ом". Record of the run (DM40A, RES, manual, MED):
      [docs/data/res-steps-2026-10-25.csv](docs/data/res-steps-2026-10-25.csv). 0.01 .. 200 ohm read right on
      600 ohm; the step 200 -> 500 ohm at 99.8 s gave one mixed reading (231 ohm), then OL at every conversion
      for 32 s (rows 25 ms apart: R22 0x13, [src/dmm.rs](src/dmm.rs) `poll_ohm` on OL). Every later block of
      0.1 s rows after a 0.45 .. 1.1 s gap is a row write (OSR_EARLY for 2 s after it, R22_LATE); there are six
      between 132 and 140 s: the range was changed on the meter (inferred: the AUTO key's short press, the next
      higher range). After them 500 ohm reads 499.8 .. 500.2 ohm. 1697 / 2064 ohm at 137.1 s and 736 ohm at
      148.5 s (no row write from 140.2 s on) are not OL, so those blocks are on 6 kohm or higher; the range of the
      block at 132.5 .. 134.1 s (499.6 .. 500.0 ohm) is unknown (a long AUTO press there would land on 600 ohm).
      The ranges after 134.1 s are inferred from the values only.
      Cause, two hypotheses (inferred, neither confirmed; the calibrator's resistance output is an active
      simulator, inferred):
      1. Current limit: the calibrator does not give 500 ohm at the 600 ohm range's test current, 0.90 mA
         (`OHM_ISC` r1, Rref 1 k); it gives 200 ohm on the same current and 500 ohm .. 4 kohm on 6 kohm
         (0.20 mA). Its readings get worse as the current falls (60 kohm range: 460 .. 547 ohm; 600 kohm:
         1.2 .. 2.1 kohm; 60 Mohm: OL), as a simulator's would. The MMB3.0's resistance specification (the
         test current per range) is unknown here.
      2. Transient lock: the calibrator's switch (an open moment: the node goes to the open-circuit 2.17 V)
         leaves its output failed while the current runs, and a row write (relays, a current interruption)
         clears it. Fits the 132.0 s row write if that block is on 600 ohm.
      The device check below tells them apart: a static 500 ohm on 600 ohm reads OL -> 1; it reads but the
      200 -> 500 ohm step stays OL -> 2.
      Ruled out in the firmware:
      - The limit: `COUNTS_MODEL` is pinned to 2 for every model byte (DM40A too), so the 600 ohm range's OL
        limit is 6 x 0.1 kohm x 1.03 = 618 ohm (`FACTOR` / `OVER`), not the stock DM40A's 4 x 1.05 = 420 ohm.
      - The range: `Dmm::set_range` / `scpi::range_item_code` map the first ohm item and `CONF:RES 600` to r1
        (Rref 1 k, the stock f10 row); 0.01 .. 200 ohm read right on it in the same record.
      - A latch: `poll_ohm` takes OL afresh from every conversion (the average restarts on any R22 but the
        steady one, the vet accepts two OL in a row, no hold), and the 25 ms rows show one OL decision per
        conversion for the 32 s. Emulator ([emu/hy3131/ohm600test.py](../emu/hy3131/ohm600test.py)): manual
        600 ohm at SLOW / MED / FAST, 200 -> open -> 500, 700 -> 500 and open -> 500 ohm read 500 ohm from the
        first conversion after the step; 590 / 610 ohm in range, 620 ohm OL; AUTO lands a passive 500 ohm on
        600 ohm, also with the emulator's node lag after each range step (`ohmtau:1000`).
      - The UI: "0L" with "Overload" next to it is the plain OL readout (`ui.rs` `ol_word`), not the sentinel's
        OVERLOAD alert (that one is the uA / mA shunt, DC / AC A only).
      Not changed: the firmware.
- [x] Device check (10 min, MMB3.0 and a passive resistor; when the owner allows; `SYST:ALER:ALARm OFF` first
      if the owner is asleep, or the `--features mute` build, #53). Each step: `CONF:RES 600;:SENS:RATE MED`, then 20 x
      `RES:RANG?;:DBG:READout?;:DBG:SENTinel?` 0.5 s apart.
      (1) A passive 470 .. 560 ohm resistor: in range, the control.
      (2) MMB3.0 500 ohm, set before entering 600 ohm (no step on the range): OL = hypothesis 1. Note the DBG:SENTinel? `jack` field (1 / 2 = the node is driven, as by a
      source) and the alert.
      (3) The nodes with the simulator on: `firmware/tools/srcmap.py --vrefs VDS15 --paths R1K,R10K --load 500`
      (the stock r1 / r2 rows): I and V_jack on 600 ohm and on 6 kohm. A simulator that cannot follow shows
      V_jack near the open-circuit 2.17 V or I far from 0.90 mA; a passive 500 ohm gives V_jack = I x 500.
      (4) MMB3.0 300 / 400 / 450 ohm on 600 ohm: where OL starts (the simulator's current limit, inferred).
      (5) MMB3.0 500 ohm on 600 ohm with the low-current variant ON (MF page, ON button: 0.28 mA, `OHM_ISC` r1
      ON): in range = the current limit confirmed, and the way to check the 600 ohm range with this calibrator.
      (6) If (2) reads 500 ohm: the 200 -> 500 ohm step on 600 ohm; OL until a range change = hypothesis 2.
      (7) MMB3.0 500 ohm on AUTO (long AUTO press): `RES:RANG?`. A passive 500 ohm lands on 600 ohm (emulator);
      6 kohm means AUTO tried 600 ohm, read OL and went back (OHM_BAND hold).
      - Device 0.3.0 mute (2026-10-27), the MR9270S+ resistance output instead of the MMB3.0: the MR9270 gives at
        most 400 ohm (a 500 ohm write reads back 4000 = 400.0 ohm once the output is on), so 500 ohm was not
        tested. Same recipe (MED, 0.5 s polls), every row CMP_PB1 / NONE, `jack` 0: 300 ohm 301.71; 400 ohm
        set on 6 kohm (396.45), then `CONF:RES 600`: 398.30 ohm, 20 of 20, no OL; 200 -> 400 ohm on 600 ohm:
        one mixed reading (281.06 at the 200 step), then 398.30, no OL; 0.28 mA ON on 600 ohm: 396.78; AUTO:
        600 ohm, 398.30. Step (3), 400 ohm: R1K I 0.868 mA, V_jack 0.493 V; R10K I 0.189 mA, V_jack 0.305 V.
        V_jack / I is not 400 ohm on either row (568 / 1615 ohm); why is unknown (no passive reference was run).
        Left for the owner: steps (1), (2), (4) .. (7) with the MMB3.0 at 500 ohm and a passive resistor.
      - Owner, MMB3.0, test build 0.3.1 (2026-10-29): 500 ohm on 600 ohm reads OL with 0.90 mA and in range with
        0.28 mA ON (499.53 .. 499.59 ohm over SCPI). Hypothesis 1: the MMB3.0's resistance output cannot carry
        the 0.90 mA test current at 500 ohm; not a firmware fault. With this calibrator the 600 ohm range is
        checked with 0.28 mA ON.
        A passive 500 ohm resistor reads in range with both 0.90 mA and 0.28 mA ON (step 1, the control).
      Open PR #57: the MR9270 only (it stops at 400 Ω): at 400 Ω no OL static, after 200 -> 400 Ω, with 0.28 mA
      ON or on AUTO (lands on 600 Ω); the MMB3.0 500 Ω steps and the passive resistor stay open.

## Jerky screen changes (owner 2026-10-29, branch fix/smooth-redraw)

Cause and fix: [lcd.rs](src/lcd.rs) "deferred fills"; before / after: [emu/smoothtest.py](../emu/smoothtest.py).
- [ ] Device check (2 min, the `--features mute` build): menu > each tile and back, DMM V -> Ω -> A, MF range page
      open / close, scope MF menu open, its tabs, close. Expected: no blank screen between pages; the old page
      stays until the new one covers it.
- [x] Scope page entry: a top-to-bottom wipe in strips since branch feat/strip-render (below); ~130 ms in the
      emulator, as before; the device time is unknown.

### Strip rendering (owner 2026-10-30, branch feat/strip-render)

A full page repaint is drawn in 12-row strips into `diskbuf::ARENA` (then `calbackup::SCRATCH`) and each strip sent once
([lcd.rs](src/lcd.rs) "strip rendering", [ram-budget.md](docs/ram-budget.md#the-disk-buffer-arena-diskbufarena-12-kib)):
menu, settings, range page (not power / temperature), help, generator, scope page.
- [ ] Device check (3 min, `--features mute`): menu > each tile and back, Settings pages, MF range page, the scope
      page entry. Expected: each page appears top to bottom in one pass, no widget half drawn. `DBG:PROF?` after a
      few Settings / Generator / Scope entries: their draw code in region C runs once per strip (27 times), the
      emulator has no flash wait states.
- [ ] `DBG:TE?` once ([hw/README.md, LCD](../hw/README.md#lcd)): a pin that changes only with TE on is the TE line.
      If there is one: strips in the panel's scan direction synced to TE.
- [ ] Still drawn directly (deferred fills): the DMM page (its chart, reading and status are drawn after the page,
      outside `draw_static`), the combined view, popups (open / uncover), keypad, limits, record, inrush,
      gallery, touch test, device page, the power pages.
- [ ] The scope page in strips takes as long as before (~130 ms emulator): the side buttons' `paint` closures
      (~300 instructions per pixel) and the graph (~20 per pixel) are the CPU cost; the strip passes add ~4 ms.
- [ ] Push a strip by DMA (DMA1 memory-to-memory to 0x60020000) while the next one is drawn: two half-size
      strips, at most the bus time (~20 ns per pixel, ~3 ms per screen; inferred from BWTR1) saved per repaint.
      The emulator completes a memory-to-memory DMA at once: measure on the device.
- [ ] 32-bit stores to the data register in `push` (two bus writes each, RS stays high; to be checked on the
      GD32).
