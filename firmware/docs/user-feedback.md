# DM40 user community feedback -> opendm40 improvement items

Firmware complaints, bugs, wishes and praise from the Russian-language
Telegram owners' chat about the Alientek DM40 (A/B/C,
[t.me/S2084K](https://t.me/S2084K)), triaged against opendm40
([TODO.md](../TODO.md), [parity.md](parity.md),
[ui-review.md](../../docs/history/ui-review.md),
[dm40-feature-request.md](../../docs/history/dm40-feature-request.md)).

New actionable items carry an **UF-nn** id; they are mirrored as one-liners in
[TODO.md](../TODO.md) ("From user community feedback"), which tracks their
status. The triage text below is as of 2026-10-13; later status notes are
marked with their date or commit, and §14 has the current status of every
item.

---

## 0. Source and method

- **Source:** Telegram topic export "Alientek DM40(A,B,C)", 26 Feb 2025 to
  13 Oct 2026. It has 4218 text/media messages from about 83 participants;
  3279 remained after dropping link-only, media-only and very short messages.
  Most users write in Russian, some in Ukrainian or English. The export is
  treated as untrusted text.
- **Population:** mostly **DM40C** owners. There are a few A and B owners, and
  this repo's unit is a DM40A. Stock firmware versions discussed: V1.7 to V2.6.
  Changelogs for V2.0 and V2.1 were quoted in the chat.
- **Method:**
  1. HTML parsed to one line per message, with anonymised sender tags (`U1`…).
  2. Four parallel extraction passes, one per quarter of the chat, using a
     fixed schema: ids, users, type, theme, fw/hw, model, version, quote, gloss.
  3. A keyword cross-check over the full text (прошивк, калибр, термопар,
     прозвон, триггер, генератор, завис, …).
  4. Manual triage against the opendm40 docs and the source (`app.rs`, `gen.rs`,
     `ui.rs`, `dmm.rs`, `scope.rs`, `main.rs`).
- **Counting caveat:** "users" means **distinct sender tags**. The export
  attributes many early posts (Feb–Mar 2025) to the topic author, so tag `U1`
  merges several real people. Counts are therefore lower bounds, and a single
  `U1` can stand for 2+ people. Message ids (`#nnnnn`) are given so any claim
  can be re-checked in the export. No user names appear in this document.
- **Excluded:** the last three messages ([#62809](https://t.me/S2084K/62809)/[#62883](https://t.me/S2084K/62883)/[#62884](https://t.me/S2084K/62884)), where another
  custom-firmware author asks "what's missing in the firmware?". They got no
  answers. They do show that "USB SCPI" is what that author wanted most.

### Volume by theme (distinct users / extracted items)

| Theme | Users | Items | Character |
|---|---:|---:|---|
| DMM accuracy / drift / calibration | 17 + 11 | 16 + 13 | praise for accuracy; wish for user calibration |
| Bluetooth app / PC software | 17 + 9 | 20 + 12 | app bugs, PC app broken, third-party apps |
| Scope UI | 16 | 14 | **mostly complaints** (8 of 14) |
| Probes / connectors / build | 16 + 20 | 21 + 15 | hardware |
| Temperature | 13 | 16 | **thermocouple ~4 °C low; offset wish** |
| Continuity / diode | 12 | 13 | **mostly praise** (7 of 13) |
| Generator | 12 | 11 | bugs + "too basic" |
| Touch UI / keys | 12 + 5 | 13 + 4 | split: praised vs "too fiddly, want keys" |
| Scope bandwidth / channels | 12 | 10 | hardware |
| AC/DC, AC+DC | 10 | 10 | praise; AC+DC unlocked on A via app |
| Capacitance | 10 | 12 | stray-C nulling, slow large caps |
| Battery / auto-off / power gestures | 9 | 11 | runtime 10–15 h; gestures praised |
| Firmware distribution / updates / bricking | 18 + 9 | 14 + 6 | hard to get; fear of bricking; no full dump |
| HOLD / REL / MIN-MAX, trend chart | 7 + 6 | 9 + 7 | REL + chart praised; min/max reset regressions |

---

## 1. Executive summary

**What owners love and opendm40 must not lose:**
1. The instant continuity beeper with 3-level diode beeps.
2. DMM readings that settle immediately, about 10 per second.
3. REL available in every mode.
4. The trend chart.
5. Finger control of the scope (tap the corners for timebase, drag trigger/zero).
6. Gestures: 2-finger swipe-down = power off, 3-finger swipe-down = screenshot.
7. Fast boot and resume-to-last-mode.
8. Uptime counter.
9. Generator that runs independently of the scope.

The screenshot gesture (UF-17) and the 2-finger power-off (UF-18) were
missing at triage time; both exist since 67fa077. The continuity beeper path
is still untested on the device (UF-01).

**Top complaints that firmware can fix:**
1. **Thermocouple / cold-junction reads ~3.6–6 °C low** on early units, with no
   user offset (5+ users, raised for 18 months).
2. **Scope UI:**
   - AUTO resets AC coupling to DC.
   - Trace-offset and trigger controls are tangled.
   - Horizontal and vertical drag are coupled.
   - The +/- direction on V/div feels inverted.
   - Cursors are buried in a menu.
   - There is no trigger-position setting.
   - FFT has no numeric axis.
3. **Generator:**
   - 50–100 kHz silently becomes 0 Hz.
   - Exact frequencies are hard to set.
   - The sawtooth runs the opposite way to the on-screen icon.
4. **No user calibration.**

**Notable cases where opendm40 copied stock bugs:**
- opendm40 reproduces "AUTO forces DC": `app.rs` `autoset_start` sets `st.ac = false`, and the successful exit of `autoset_step` keeps it; only the failure path restores the saved settings.
- opendm40 reproduces "non-square > 50 kHz → 0 Hz silent". This was ticked in
  `TODO.md` as a parity item, but owners call it a bug.
- opendm40 very likely reproduced the inverted sawtooth: the `gen.rs` ramp table fell while
  the preview in `ui.rs` `gen_preview` rose.

All three are cheap to fix. Stock parity is the wrong target for them. All
three are fixed since: UF-04 (81e02a7), UF-14 and UF-16 (ae3e655).

**Most valuable new items:**
- UF-03: AC+DC on the DM40A. Owners proved the A hardware does it when the
  phone app commands it.
- UF-02: cold-junction self-heating is a new risk, because opendm40 keeps both
  analog rails powered.
- UF-19: battery runtime and power policy for the always-on instruments.
- UF-17 / UF-18: the screenshot and power-off gestures.
- UF-04 / UF-14 / UF-16: the three stock-bug fixes listed above.
- UF-20: back up calibration to the disk.
- UF-21: AUTO+ (V-first) as the default autorange.

---

## 2. Praise: behaviour to preserve

| # | What owners praise | Users | Evidence | opendm40 status |
|---|---|---:|---|---|
| P1 | **Continuity is instant** ("можно морзянку передавать, реакция мгновенная"; "прозвонка тут лучшая ВООБЩЕ"). The 3-level diode beep is copied by a rival that "успевает раз через три". | 7+ (U1,U2,U3,U52,U60,U68 …) | [#15792](https://t.me/S2084K/15792), [#16299](https://t.me/S2084K/16299), [#26465](https://t.me/S2084K/26465), [#42341](https://t.me/S2084K/42341), [#44659](https://t.me/S2084K/44659), [#59912](https://t.me/S2084K/59912), [#59917](https://t.me/S2084K/59917) | Parity: MATCH, but the beep is driven by the HY3131 CMPO pin and is **unknown on the device** (parity §1). → **UF-01** |
| P2 | Diode mode shows the forward drop **and** an equivalent ohm value; "киллер-фича". | 3 | [#15857](https://t.me/S2084K/15857), [#16000](https://t.me/S2084K/16000), [#44659](https://t.me/S2084K/44659) | MATCH (`diode_beep`, aux ohm) |
| P3 | **DMM settles instantly**, without the seconds-long last-digit refinement of other HY3131 meters; ~10 readings/s; faster than Rigol DM858. | 6 | [#15802](https://t.me/S2084K/15802), [#32899](https://t.me/S2084K/32899), [#32918](https://t.me/S2084K/32918), [#36272](https://t.me/S2084K/36272), [#38172](https://t.me/S2084K/38172) | Ported stock numerics. **Keep FR-05 smoothing off by default** so this does not regress. |
| P4 | REL ("свой ноль") in every mode: probe-lead and stray-C nulling, milliohms with REL. | 4 | [#15802](https://t.me/S2084K/15802), [#19581](https://t.me/S2084K/19581), [#21993](https://t.me/S2084K/21993), [#22009](https://t.me/S2084K/22009) | MATCH; discoverability poor (see §5.3) |
| P5 | Trend chart: "бомбическая фича", "сказка"; adjustable timebase. | 5 | [#15935](https://t.me/S2084K/15935), [#19324](https://t.me/S2084K/19324), [#54283](https://t.me/S2084K/54283) | MATCH (160/320/640 ms/div) |
| P6 | Simultaneous DC + AC + AC+DC readout (B/C). | 4 | [#15857](https://t.me/S2084K/15857), [#16029](https://t.me/S2084K/16029), [#17730](https://t.me/S2084K/17730) | Not on A; see **UF-03** |
| P7 | Scope finger control: tap the top-left/right to step the timebase; drag trigger, zero and FFT; "пушка, бомба, петарда". | 2 (+1 vs rival) | [#15791](https://t.me/S2084K/15791), [#16558](https://t.me/S2084K/16558), [#44584](https://t.me/S2084K/44584) | At triage: no corner taps. Done in ff29cd3: the stock halves or bottom-corner zones (default) with preview tabs, see [scope-scale-ux.md](scope-scale-ux.md). → **UF-07** |
| P8 | **3-finger swipe-down = screenshot** on any screen ("PriSkr OK"). | 4 | [#16839](https://t.me/S2084K/16839), [#17435](https://t.me/S2084K/17435), [#19950](https://t.me/S2084K/19950), [#50265](https://t.me/S2084K/50265) | Missing at triage; done in 67fa077. → **UF-17** |
| P9 | **2-finger swipe-down = power off**, preferred over the awkward side button. | 2 | [#15928](https://t.me/S2084K/15928), [#16743](https://t.me/S2084K/16743) | Missing at triage; done in 67fa077. → **UF-18** |
| P10 | Fast boot (power-on "10 из 10"); deleting/replacing the logo makes it instant. | 4 | [#15801](https://t.me/S2084K/15801), [#16641](https://t.me/S2084K/16641), [#18322](https://t.me/S2084K/18322), [#26090](https://t.me/S2084K/26090) | Done (no logo) |
| P11 | Resume to the last mode at power-on (V2.0 "mode memory"), optional. | 2 | [#16313](https://t.me/S2084K/16313), [#16314](https://t.me/S2084K/16314) | Done (backup-SRAM persist) |
| P12 | Uptime counter since power-on. | 1 | [#15793](https://t.me/S2084K/15793) | Done (status bar) |
| P13 | Generator runs independently of the scope; scope can watch the generator. | 3 | [#50397](https://t.me/S2084K/50397), [#61569](https://t.me/S2084K/61569), [#59900](https://t.me/S2084K/59900) | Done, and extended (everything runs at once) |
| P14 | In-circuit ("ON line", low-current) resistance mode works in ~80 % of cases. | 3 | [#20559](https://t.me/S2084K/20559), [#22994](https://t.me/S2084K/22994), [#54211](https://t.me/S2084K/54211) | `ohm_var` toggle ported (second tap on the ohm button, `Dmm::set_ohm_var`); 2026-10-21: the stock "ON" tag drawn on the ohm button (DMM page + combined band) and a Normal / "Low current ON" pair on the Resistor Settings page; untested on the device |
| P15 | Single current jack with automatic MOSFET shunt switching. | 1 | [#26977](https://t.me/S2084K/26977) | MATCH (hardware + stock logic) |
| P16 | Short-press power-on (V2.1). | 2 | [#18842](https://t.me/S2084K/18842), [#18883](https://t.me/S2084K/18883) | Must survive the native-boot shim's PD6 handling. → **UF-37** |
| P17 | Auto-off countdown with a 10 s buzzer warning (V2.0). | – | changelog [#15892](https://t.me/S2084K/15892) | opendm40 has the idle dim/off; confirm the warning (FR-10) |
| P18 | Probe attenuation range wider than rivals'. | 1 | [#44584](https://t.me/S2084K/44584) | Partial at triage (x1/x10/x100); the full list since 2124249. → **UF-12** |
| P19 | BT live update down to 50 ms; app reconnects on its own. | 2 | [#26030](https://t.me/S2084K/26030), [#57179](https://t.me/S2084K/57179) | `bt.rs` served every loop; confirm the 50 ms poll rate. → **UF-36** |
| P20 | Smooth trace drawing (a sine peak looks smooth vs a rival's stair-steps). | 2 | [#59901](https://t.me/S2084K/59901), [#59909](https://t.me/S2084K/59909) | Linear interpolation at triage; sin(x)/x since 086f734, see UF-41 |

---

## 3. Temperature (13 users, the most persistent firmware complaint)

| Item | Users | Evidence | opendm40 status / proposal |
|---|---:|---|---|
| Thermocouple reads **3.6–6 °C low** on early units. The internal cold-junction sensor is off by the same amount: "внутренний термодатчик врёт ровно на то же значение что и внешняя термопара", "Надоело постоянно в уме плюсовать 3.6 °C". Newer HW revisions (v1.2, bootloader v4, units with Chinese text under the glass) appear factory-calibrated and read 100.0 °C in boiling water. | 5 complain (U1,U4,U13 + 2 more via quotes); 5 report OK units (U49,U57,U58,U60,U66) | [#17423](https://t.me/S2084K/17423), [#17698](https://t.me/S2084K/17698), [#19967](https://t.me/S2084K/19967), [#40389](https://t.me/S2084K/40389), [#41448](https://t.me/S2084K/41448), [#42052](https://t.me/S2084K/42052), [#48877](https://t.me/S2084K/48877), [#54829](https://t.me/S2084K/54829) | **FR-26** (offset) exists as P2. The community makes it the #1 DMM firmware wish → **raise FR-26 to P1**. Implement it as a CJ offset (°C) in Settings, stored in our own persist area, plus an optional TC offset. Never write the stock cal block. **S.** |
| CJ sensor **self-heats** in power-hungry modes. In AC+DC volts the internal temperature rises, and right after switching to °C the reading is wrong until it cools (~1 h idle matched a weather station). | 1 (U1) | [#17615](https://t.me/S2084K/17615), [#17606](https://t.me/S2084K/17606) | **NEW, UF-02, P1, M.** (Device check 40728b8: no CJ self-heating from the scope rail, below 0.5 °C; the stock comparison is open, [TODO.md](../TODO.md).) At triage opendm40 kept **both analog rails on** and runs the scope and generator concurrently, so the board runs warmer than stock in any mode. The CJ reading will then be biased by a fixed offset. Steps: (1) measure the CJ sensor vs ambient with stock vs opendm40 (DMM only / both rails / scope running) for 60 min; (2) if the bias is > 0.5 °C, apply a rail-state-dependent correction or a warm-up model, or power the scope rail down while in °C mode; (3) show "CJ xx.x °C" in the aux line (stock already does) so users can see it. Risk: the correction is unit-specific. |
| **PT100 / PT1000 / NTC** probe support ("заказал себе PT100... А вдруг поддерживается"; a thermistor probe "dm40 его не поддерживает"). Requested from the vendor twice. | 3 (U1,U60 + U1 again) | [#17647](https://t.me/S2084K/17647), [#17687](https://t.me/S2084K/17687), [#18273](https://t.me/S2084K/18273), [#59112](https://t.me/S2084K/59112), [#59918](https://t.me/S2084K/59918) | **NEW, UF-23, P2, M.** (Done; tested on the calibrator 2026-10-13.) Add an RTD sub-mode on the ohm path: 2-wire, lead-null via REL, Callendar–Van Dusen for Pt100/Pt1000 and Steinhart–Hart/β for NTC 10k. Pure arithmetic on the existing ohm reading; uses the lowest ohm range's test current. Risk: self-heating at 0.85 mA is negligible for Pt100. |
| Thermocouple type choice (only K) | 1 | [#59918](https://t.me/S2084K/59918) | **FR-26** (K/J/T), confirmed |
| Bundled thermocouple is rated to only 300 °C while the meter reads to 1000 °C. | 2 | [#54853](https://t.me/S2084K/54853) | HW/accessory; mention it in the help page (UF-29) |
| V1.7 fixed "temperature reading occasionally jumps". | – | [#15892](https://t.me/S2084K/15892) | Keep the stock 3-phase CJ filter (parity MATCH) |

## 4. DMM accuracy, calibration, resolution, speed

| Item | Users | Evidence | opendm40 status / proposal |
|---|---:|---|---|
| Factory calibration excellent ("откалиброван он очень хорошо", 4th-decimal agreement with a certified shunt; ±5 V symmetric). | 6 | [#15886](https://t.me/S2084K/15886), [#16523](https://t.me/S2084K/16523), [#25359](https://t.me/S2084K/25359), [#32884](https://t.me/S2084K/32884), [#38171](https://t.me/S2084K/38171) | Preserve: opendm40 reads the stock cal **read-only** (parity §1) |
| **No user calibration**; repeatedly requested; vendor: "пользовательская калибровка не доступна". A hidden `_CALI`/SPP BT device on new firmware suggests wireless factory calibration. Another HY3131 meter's command set (`DMMFinalizeCalibP/N <ref>`) shows separate +/- calibration points. | 8 (U1,U9,U28,U32,U34,U60 …) | [#17433](https://t.me/S2084K/17433), [#25348](https://t.me/S2084K/25348), [#40293](https://t.me/S2084K/40293), [#40361](https://t.me/S2084K/40361), [#41625](https://t.me/S2084K/41625), [#51339](https://t.me/S2084K/51339), [#52804](https://t.me/S2084K/52804), [#56499](https://t.me/S2084K/56499) | **FR-36**, confirmed P1. Suggest exposing it over USBTMC first (`CAL:` SCPI with a password, separate P/N points, writes to our own record, never the stock one). That is safer than an on-screen flow and matches what owners with calibrators want. |
| Fear of losing calibration: a dead DM40A MCU could not be revived ("защита стоит", no full dump; "а что делать с калибровками"). | 4 | [#51645](https://t.me/S2084K/51645), [#51648](https://t.me/S2084K/51648), [#51660](https://t.me/S2084K/51660), [#51667](https://t.me/S2084K/51667) | **NEW, UF-20, P1, S.** (Done; CALBAK.BIN written on the device in the T3 run.) On first boot opendm40 writes `CALBAK.BIN`, a verbatim copy of the 0x080C0000 cal record + 0x080A4000 boot data + UID with a CRC, to the USB disk, written when missing or changed (compare-first). Owners can then archive their unit's calibration. The repo has a full flash dump of this unit; owners do not. Overlaps the FR-36 export criterion. |
| Temperature drift: ~20–27 ppm/°C on 6 V DC, ~7–9 ppm/°C on ohms (several home tests; the sign varies per unit). Wish: compensate using the internal temperature sensor. | 6 | [#17345](https://t.me/S2084K/17345), [#18203](https://t.me/S2084K/18203), [#24946](https://t.me/S2084K/24946), [#51913](https://t.me/S2084K/51913), [#59918](https://t.me/S2084K/59918) | **NEW, UF-24, P2, L.** A per-unit coefficient (ppm/°C per function) applied as `x·(1 − k·(T−Tcal))` with T from the CJ sensor. Needs a two-temperature characterisation by the user (via `CAL:`), so it depends on FR-36. Low value-for-effort; park it. **Decision 2026-10-21: parked, not implemented.** The formula is well defined, but nothing to feed it: (1) the CJ sensor (`Dmm::t_cj`) is read only in the thermocouple mode; the always-on sensor is the MCU die (`power.mcu_t`, SYST:TEMP?), which tracks the board and the rail load (UF-02), not the HY3131 reference; (2) k is per unit and per function with a varying sign (the reports above), so it needs a user two-temperature characterisation and a CAL/SCPI surface to store it (FR-36, not done); (3) Tcal is unknown (no factory calibration temperature in the cal block). Revisit after FR-36 and the UF-02 CJ-vs-ambient measurement. |
| "+1 разряд" (one more digit), "я просто знаю что это возможно"; A→C unlock wish (4 users). | 2 + 4 | [#59919](https://t.me/S2084K/59919), [#59920](https://t.me/S2084K/59920), [#16374](https://t.me/S2084K/16374), [#39608](https://t.me/S2084K/39608) | **NEW, UF-25, P2, S.** (Decided in 8d75505: no option, the extra digit is always on.) An optional "extended resolution" display that shows the HY3131's extra digit in grey/smaller type (A: 40 k → 60 k counts) with a clear "uncalibrated digit" note. **Do not** call it A→C: the A lacks the TC1746 network ([#25087](https://t.me/S2084K/25087), [#51830](https://t.me/S2084K/51830)). Risk: noise on the extra digit; default off. |
| Last digit "dances" (continuity resistance readout [#16302](https://t.me/S2084K/16302); last digit flickers 0–10 on a 10 V reference, [#42490](https://t.me/S2084K/42490)). | 3 | [#16302](https://t.me/S2084K/16302), [#42490](https://t.me/S2084K/42490) | **FR-05** (smoothing), confirmed. Default off (see P3). |
| Bargraph should update ≥ 40×/s. | 1 | [#36280](https://t.me/S2084K/36280) | **NEW, UF-30, P2, M.** A thin analog bar fed from the HY3131 fast (19-bit) channel at 40–50 Hz, drawn as a dirty-column diff like the chart. Shares the fast path with FR-03 peak and FR-38. **2026-10-21:** the stock has no bargraph. Widget `src/bargraph.rs` (b6a4d10, host test tools/bargraph-test): bipolar / unipolar, 60000-count scale from `Val::dec`, column-diff redraw; wired 2026-10-21 in the history row's place. There is no separate HY3131 fast channel in opendm40: a Reading comes per AD1 conversion, 10/s (FAST, moving) / 2.5/s (steady MEDIUM/SLOW at OSR 32768). >= 40/s needs OSR 0x13 (~38/s, the logger) plus a firmware average for the main readout: a measurement-chain decision, open. |
| Heavy averaging suspected ("Походу выборок так 100-200"). | 1 | [#54794](https://t.me/S2084K/54794) | Informational; consistent with stock OSR; no action |
| AC weaker than Fluke 287/289; AC −3 dB ~20 kHz. | 3 | [#24765](https://t.me/S2084K/24765), [#16732](https://t.me/S2084K/16732) | HW. The 20 kHz figure belongs in our docs (help page). |
| DC component of an AC signal shown badly/slowly; wish to see ripple **p-p** while in DC V. | 3 | [#29158](https://t.me/S2084K/29158), [#29139](https://t.me/S2084K/29139), [#29174](https://t.me/S2084K/29174) | Partly **FR-03** (peak) + FR-20 (aux). A DM40A can also use the concurrent scope in the DMM+Scope view. **Note only.** |

## 5. Ranges, continuity, resistance, capacitance, current, HOLD/REL

### 5.1 Ranges / autorange
| Item | Users | Evidence | opendm40 status / proposal |
|---|---:|---|---|
| **Auto starts on the mV range, Auto+ on V**; "такие вещи стоит явно писать что хана будет". In the same thread another unit's input protection (2× FMMT619) died after probing a 380 V panel. That is a separate report; the mV-first start is only a plausible contributing factor. | 3 | [#41828](https://t.me/S2084K/41828), [#41833](https://t.me/S2084K/41833), [#41840](https://t.me/S2084K/41840), [#41820](https://t.me/S2084K/41820) | **NEW, UF-21, P1, S.** (Done in 4c157a8.) No new ranging logic is needed: opendm40 already ports both modes (`dmm.rs` `set_range`: `v_auto` 3 = AUTO, mV-first; 1 = AUTO+, V-first and up-only), and the default is `v_auto: 3` (AUTO). Make **AUTO+ the power-up/default autorange** for V. Keep mV-first AUTO selectable from the Range page with a one-line warning ("mV range: max input …"). Risk: small signals take one extra settle period before stepping down. |
| July 2025 stock firmware "значительно поправили" autoranging speed (current). | 1 | [#26434](https://t.me/S2084K/26434) | opendm40 uses the V2.6 settle table (parity MATCH) |
| Range switch point around 60 000–62 000 counts. | 2 | [#42303](https://t.me/S2084K/42303) | Informational |

### 5.2 Continuity / diode
| Item | Users | Evidence | opendm40 status / proposal |
|---|---:|---|---|
| Instant beeper = the meter's best feature | 7+ | see P1 | **NEW, UF-01, P0, S: check on the device.** The CMPO→buzzer path works under opendm40 init; latency is under 10 ms (scope the buzzer vs a probe short); the 3-level diode classes sound like stock. A regression here would be the most visible one possible. **2026-10-27:** the beeper sounds under opendm40 in continuity and diode (the owner's meter, UF-28); latency and the diode classes are unchecked. |
| False single beep when a nearby SMPS switches on (fixed by foil shielding); readout dances while shorted. | 3 | [#18016](https://t.me/S2084K/18016), [#19408](https://t.me/S2084K/19408), [#16302](https://t.me/S2084K/16302) | HW (EMI) + **FR-21** latch/anti-vibration, confirmed |
| "Volume" setting confuses: at 0 % the auto-off warning beep is silent, but the continuity beeper is unaffected (the owner wanted a quiet beeper). | 2 | [#26211](https://t.me/S2084K/26211), [#26228](https://t.me/S2084K/26228) | **NEW, UF-28, P2, S.** (Done in 8d75505: "Key/alert volume", the power-off warning sounds at volume 0.) Rename it to "Key & alert volume". Always sound the auto-off warning (or flash the screen). Investigate whether the TIM14 duty also scales the CMPO beep; if not, say so in help. **2026-10-27 (0.3.1):** it does not: the beeper is the HY3131 CMP pin, not TIM14/PA7 (hw/README.md "Buzzer", *inferred*), so it has no loudness; at volume 0 the firmware turns it off (R20 bit 2), the manual says so. |
| Diode test current 0.75 mA (< 1 mA) | 2 | [#15847](https://t.me/S2084K/15847), [#18340](https://t.me/S2084K/18340) | HW |

### 5.3 HOLD / REL / MIN-MAX / history
| Item | Users | Evidence | opendm40 status / proposal |
|---|---:|---|---|
| REL is "максимально неочевидно" (hold right of the chart, undocumented). | 3 | [#54966](https://t.me/S2084K/54966), [#39132](https://t.me/S2084K/39132), [#39134](https://t.me/S2084K/39134) | ui-review §1/§3 (REL badge), confirmed. Also add REL to the side-key legend / help. |
| MIN/MAX reset: added V2.4 ("очень не хватало"), lost in V2.5, restored as a long-press on the chart. | 1 (U1, strong) | [#33447](https://t.me/S2084K/33447), [#37831](https://t.me/S2084K/37831), [#38479](https://t.me/S2084K/38479) | **NEW, UF-35, P2, S.** (Done in 8d75505: a long press on the MIN/MAX/AVG column.) Check that opendm40 has an explicit MIN/MAX reset (long-press on the MIN/MAX column or the chart) that is distinct from the history-row long-press clear. |
| Can't save a reading to a history slot while paused (RUN/HOLD). | 2 | [#19323](https://t.me/S2084K/19323), [#19325](https://t.me/S2084K/19325) | **NEW, UF-27, P2, S.** (Done in 8d75505.) Allow the tap-to-store while HOLD is active (store the held value). Check `app.rs` history tap gating. |
| Average on the trend chart; chart zoom, time axis, timestamps. | 3 | [#19324](https://t.me/S2084K/19324), [#44962](https://t.me/S2084K/44962) | **FR-02** (AVG) + **FR-04** (chart over log buffer), confirmed |
| Sub-mode not remembered at runtime: continuity → V → back lands on diode; "Mode Remember" only covers power cycles. Reported to the vendor; not fixed. | 2 | [#24836](https://t.me/S2084K/24836), [#24851](https://t.me/S2084K/24851), [#26071](https://t.me/S2084K/26071) | **FR-13**, confirmed P0. Check the opendm40 per-button sub-mode memory. |

### 5.4 Capacitance / current / resistance
| Item | Users | Evidence | opendm40 status / proposal |
|---|---:|---|---|
| ~32–48 pF stray C must be REL-nulled; newer units ship factory-nulled. | 4 | [#32807](https://t.me/S2084K/32807), [#19575](https://t.me/S2084K/19575), [#51371](https://t.me/S2084K/51371), [#54841](https://t.me/S2084K/54841) | **FR-24** (persistent stray-C REL), confirmed. A DM40A from older batches benefits most. |
| Large capacitors: V2.1 claimed a fix but the problem remains; slow vs a Richmeters RM406B. Others call it fast. | 3 | [#18850](https://t.me/S2084K/18850), [#26435](https://t.me/S2084K/26435), [#50397](https://t.me/S2084K/50397), [#51384](https://t.me/S2084K/51384) | **FR-24** ("charging…" progress), confirmed |
| Very small C (~1 pF-class, scope input) reads 10× low. | 2 | [#16519](https://t.me/S2084K/16519) | Low priority; note in FR-24 |
| Online/offline ohm: test current 0.27 vs 0.85 mA, purpose unclear. | 2 | [#16919](https://t.me/S2084K/16919), [#16978](https://t.me/S2084K/16978) | **FR-19** (rename to "In-circuit (low-I)" with the current shown), confirmed |
| 4–20 mA "%" in DC mA confuses hobbyists; in AC A a negative % appears. | 3 | [#24372](https://t.me/S2084K/24372), [#44917](https://t.me/S2084K/44917) | opendm40 shows the % only in mode 8 (DC mA). **NEW, UF-26, P2, S** (done in 8d75505, 11c4f9b): label it "4-20mA xx%" and hide it outside −25…125 %. |
| Idle current input should rest on the 10 A shunt, not the 47 Ω mA shunt. | 1 | [#59918](https://t.me/S2084K/59918) | Mostly HW (an unpowered meter has no state). **Note only:** opendm40 current autorange should enter on the A shunt (unchecked). |

## 6. Oscilloscope

### 6.1 Scope UI (16 users; 8 complaint items, 2 praise)
| Item | Users | Evidence | opendm40 status / proposal |
|---|---:|---|---|
| **AUTO switches AC coupling back to DC** ("ставишь вход AC, жмешь авто и он перескакивает на DC"), making ripple checks tedious. | 4 (U28,U30,U52,U55) | [#44566](https://t.me/S2084K/44566), [#44570](https://t.me/S2084K/44570), [#44573](https://t.me/S2084K/44573), [#44575](https://t.me/S2084K/44575) | **NEW, UF-04, P1, S.** (Done in 81e02a7.) opendm40 copied this: `app.rs` `autoset_start` sets `st.ac = false`. Keep the user's coupling. If AC is selected, run the V/div search in AC; if the signal is flat in AC, fall back to DC and show "AUTO: DC". |
| Trace offset and trigger level share one control spot; "тыкать несколько раз и присматриваться что ты выбрал это боль". A rival puts them on opposite sides. | 2 | [#58939](https://t.me/S2084K/58939), [#59147](https://t.me/S2084K/59147) | ui-review §4 #8 (trigger handle + markers), confirmed. Concrete: **zero/offset handle on the left edge, trigger handle on the right edge**, each grabbed independently with a ~20 px zone. |
| Horizontal drag also nudges vertical ("двигается немного и верх-низ"). | 1 | [#44587](https://t.me/S2084K/44587) | **NEW, UF-05, P1, S.** (Done: d154f60, 100eb48, 74b5c16; a press on a marker locks to its axis, Settings "Scope drag" Axis lock.) Axis lock in `gesture.rs`/`app.rs`: after 12 px of travel, lock to the dominant axis (\|dx\| > 2\|dy\| or the reverse) for the rest of the drag. Use the same dominance logic as the drag-vs-swipe fix (ui-review #1). |
| V/div "+" at the top makes the trace **smaller** (it increases V/div); counter-intuitive. | 1 (+1 disagrees) | [#26398](https://t.me/S2084K/26398), [#26405](https://t.me/S2084K/26405) | **NEW, UF-10, P2, S.** (Done in c13d61a: magnifier icons.) Show the buttons as magnifier icons (zoom in/out on the trace) or ▲/▼ volts. Do not just swap them. |
| Timebase: tapping a tiny box cycles round-robin; hard to find the horizontal control. Contrast with the praised stock corner-tap. | 2 + praise | [#26399](https://t.me/S2084K/26399), [#26921](https://t.me/S2084K/26921), [#15791](https://t.me/S2084K/15791) | **NEW, UF-07, P2, S.** (Done in ff29cd3 with bottom corners and a timebase list on a tap of the timebase badge: [scope-scale-ux.md](scope-scale-ux.md).) Keep the right-panel +/- and **also** support stock-style taps on the top-left/top-right graph corners (timebase −/+). A long-press on the timebase or V/div readout opens a list picker instead of cycling. |
| Cursors "запрятаны" deep in a menu; a user could not find them; a rival is praised for easy cursors. | 3 | [#17520](https://t.me/S2084K/17520), [#17517](https://t.me/S2084K/17517), [#58939](https://t.me/S2084K/58939) | **NEW, UF-08, P2, S.** (Done in c13d61a: a "CUR" tab at the graph bottom.) A "CUR" toggle in the scope right panel (or long-press on the graph) cycles off → X → Y; readouts appear in the measurement row. |
| No trigger-position setting; no "center X/Y". | 1 | [#44586](https://t.me/S2084K/44586) | **NEW, UF-09, P1, M.** (Done: the stock T marker drag, 100eb48; double tap, c13d61a.) Horizontal trigger position (pre-trigger 10–90 %) from the stored record. `TODO.md` lists horizontal pan as not done; extend it. Double-tap on the graph = centre position + trigger at 50 %. |
| Colour choice changes the whole UI theme, so most are unusable; want only the trace colour. | 2 | [#44588](https://t.me/S2084K/44588), [#44597](https://t.me/S2084K/44597) | opendm40 has a fixed palette, so the problem does not arise. Optional trace-colour setting: P2, not filed. |
| Scope "сделали на отвали", "софт сырой"; scope is a token feature to some. | 5 | [#44663](https://t.me/S2084K/44663), [#59147](https://t.me/S2084K/59147), [#38124](https://t.me/S2084K/38124) | Context: owners judge the scope by its UI, not its 10 MHz hardware. UI fixes above are the leverage. |
| Scope feels sluggish on newer HW revision. | 1 | [#49740](https://t.me/S2084K/49740) | n/a (vendor); keep opendm40 frame rate ≥ stock |

### 6.2 Trigger / memory / measurements / FFT
| Item | Users | Evidence | opendm40 status / proposal |
|---|---:|---|---|
| FFT: no numeric axis; peak frequency only by dragging the peak to the centre; over-sensitive drag; display blanks while adjusting. FFT window choice and cursors on FFT are praised; FFT is better than a rival's. | 4 | [#15794](https://t.me/S2084K/15794), [#15948](https://t.me/S2084K/15948), [#51067](https://t.me/S2084K/51067), [#16554](https://t.me/S2084K/16554), [#44584](https://t.me/S2084K/44584) | Missing at triage; done in 386f1ed, 0f4dffb. **NEW, UF-11, P2, M.** 1024/2048-pt real FFT on the stored record (CMSIS-style radix-2 on the M4F; Hann/Flat-top/Rect windows), dBV/div 10–50, labelled frequency axis, an automatic **peak marker readout (f, dBV)** and a cursor. Keep the last frame while dragging. |
| Waveform record to memory, "advanced triggers" for anomalies. | 1 | [#50392](https://t.me/S2084K/50392), [#50395](https://t.me/S2084K/50395) | **NEW, UF-13, P2, M.** (Done: 4146aae, 20fc636.) Save the stored record as CSV (+ screenshot) to the disk, sharing the FAT writer of the CSV/FR-16 items. Pulse-width/runt trigger as a software search over the record. |
| Probe attenuation list x0.1…x1000 valued over rivals' 1/10/100. | 1 | [#44584](https://t.me/S2084K/44584) | Partial at triage; done in 2124249, 7b1b83c. **NEW, UF-12, P2, S.** Restore the stock 13-value list (`scope_probe[13]`). |
| Cursor timing agrees with a Rigol DHO814; voltage reads high vs a DHO802 on ripple (probe/frontend). | 2 | [#17512](https://t.me/S2084K/17512), [#16009](https://t.me/S2084K/16009), [#38222](https://t.me/S2084K/38222) | Informational. Keep the stock gain/offset cal. |
| V1.7–V2.1 fixed trigger position at some timebases and AUTO anomalies. | – | [#15892](https://t.me/S2084K/15892), [#18850](https://t.me/S2084K/18850) | opendm40 ports V2.6 behaviour |
| Smooth trace at fast timebases | 2 | [#59901](https://t.me/S2084K/59901) | **UF-41, P2, M (optional):** sin(x)/x interpolation when samples < columns (the trace was linear); only at ≤ 200 ns/div. **2026-10-21:** `src/sinc.rs` (e433aad): Lanczos-3 windowed sinc, `sinc::at(&record, p)`; tools/sinc-test: at 4..8 samples/period the error is 8..3 codes vs 104..30 for linear (amplitude 400). Wired in 086f734 into the scope page's "fewer samples than columns" branch (100..500 ns/div). |

### 6.3 Scope hardware (not fixable in firmware)
Measured bandwidth is flat to ~3 MHz with −3 dB at 11.5–17 MHz. 50 MS/s
aliases above 25 MHz, so crystals are only seen up to ~20 MHz ([#15789](https://t.me/S2084K/15789), [#16738](https://t.me/S2084K/16738),
[#38120](https://t.me/S2084K/38120)). There is one channel; users wish for two channels and ~1 GS/s
([#15900](https://t.me/S2084K/15900), [#44582](https://t.me/S2084K/44582), [#50216](https://t.me/S2084K/50216)). Input capacitance is ~30–35 pF, so Rigol/Owon/Fnirsi
x10 probes cannot be compensated; one owner trimmed an on-board capacitor
([#30339](https://t.me/S2084K/30339), [#43276](https://t.me/S2084K/43276)). The probe compensation trimmer is flimsy ([#28400](https://t.me/S2084K/28400)). The MCX
connector is unusual ([#15801](https://t.me/S2084K/15801)).

## 7. Generator (12 users)

| Item | Users | Evidence | opendm40 status / proposal |
|---|---:|---|---|
| **50–100 kHz resets to 0** ("не дає виставити частоту 50кГц-100кГц, скидає на 0"). An over-range value clamps to 10 MHz for square but becomes 0 for other waves. | 1 (bug report) + 2 "hard to set" | [#28180](https://t.me/S2084K/28180), [#50400](https://t.me/S2084K/50400) | **NEW, UF-14, P1, S.** (Done in ae3e655.) opendm40 deliberately copied this (TODO: "non-square >50k = 0 Hz silent"; "snap square 50k–100k DOWN to 50k"). Replace it with: clamp to the waveform's real limit (sine/tri/ramp ≤ 50 kHz, square PWM grid) **and** show a 2 s note ("max 50 kHz for Sine"). Keep the RUN state. ui-review #10 asks for the visible warning; this item also drops the silent 0 Hz. |
| Exact frequency hard to set. Newer stock replaced the slider with numeric entry, but integers only: "32.768 кГц не поставить". | 3 | [#50400](https://t.me/S2084K/50400), [#50443](https://t.me/S2084K/50443), [#50450](https://t.me/S2084K/50450), [#50886](https://t.me/S2084K/50886) | opendm40 keypad has '.', so 32.768K → 32768 Hz is enterable (`freq_hz: u32`). But the DAC path quantises (`n = 10e6/f`; 32768 Hz → n=305 → 32787 Hz). **NEW, UF-15, P1, S** (done in ae3e655): show the **actual** output frequency under the set value and in `SOUR:FREQ?`. Follow-up (M): a fixed-rate DDS phase accumulator for sub-Hz resolution below ~100 kHz. |
| **Sawtooth ramps opposite to the on-screen icon** (compared on a Rigol). | 2 | [#16991](https://t.me/S2084K/16991), [#16996](https://t.me/S2084K/16996) | **NEW, UF-16, P2, S.** (Done in ae3e655: the ramp rises.) opendm40 likely reproduced it: the `gen.rs` ramp (`4095 − 4095/n·i`, falling) vs `ui.rs` `gen_preview` (`v = ph`, rising). Check with the scope (the output stage may invert), then make the preview match the output, or offer Ramp↑/Ramp↓. |
| Generator "совсем база", "Гена это ужас"; wants offset, sweep, etc. | 2 | [#44588](https://t.me/S2084K/44588), [#44668](https://t.me/S2084K/44668) | **FR-28** (offset), **FR-29** (units), **FR-32** (arb/sweep/burst), confirmed P2 |
| Runs independently of the scope; scope can view its output | 3 | P13 | Done |
| 12 V on the output killed its SGM3157 switch; no protection. | 3 | [#24902](https://t.me/S2084K/24902), [#25389](https://t.me/S2084K/25389) | HW (TVS). Mention in help. |

## 8. Battery, power, auto-off, charging

| Item | Users | Evidence | opendm40 status / proposal |
|---|---:|---|---|
| Runtime 10–15 h (one test: 30 min scope + DMM → 11.5 h with 2 bars left; 14 h BT logging used ~80 %). Opinions split between "too small" and "lasts long". | 5 | [#16619](https://t.me/S2084K/16619), [#25610](https://t.me/S2084K/25610), [#54285](https://t.me/S2084K/54285), [#61298](https://t.me/S2084K/61298), [#61311](https://t.me/S2084K/61311) | **NEW, UF-19, P1, M.** (Open. Since triage the analog power default is "Auto (per view)": each view powers the side it needs, Settings "Analog power".) At triage opendm40 ran DMM + scope ADC + generator concurrently with both rails on by default. Steps: (1) measure battery current (stock DMM page vs opendm40 DMM view vs opendm40 DMM+Scope) via the fuel gauge or an inline meter; (2) if opendm40 draws much more, power the scope front end/ADC clock only while a scope view, SCPI scope query or trigger is active, with a Settings choice "Scope: always on / on demand"; (3) show the estimated runtime. This conflicts with the "both rails on by default" requirement in `TODO.md`, so the user should decide after measuring. Also affects UF-02. |
| 2-finger swipe-down power-off (V1.9/V2.0) preferred to the side button (the button barely protrudes from the bumper). | 3 | [#15928](https://t.me/S2084K/15928), [#16743](https://t.me/S2084K/16743), [#44653](https://t.me/S2084K/44653) | **NEW, UF-18, P1, S.** (Done in 67fa077.) The touch controller reports a point count (`touch.rs` status bits 3..0); recognise ≥ 2 points moving down ≥ 80 px → the stock "slide to power off" confirm. |
| Auto-off countdown with beeps (V2.0), activity-based auto-off | – | [#15892](https://t.me/S2084K/15892) | **FR-10**, confirmed. Make sure the warning sounds at volume 0 (UF-28). |
| Short-press power-on (V2.1) praised | 2 | [#18842](https://t.me/S2084K/18842) | **UF-37, P1, S (done; untested on the device):** the native-boot shim (power key held 4 s, then 0.5 s more after the cue = recovery) must still latch power on a short press and must not need a long hold for normal boot. |
| Charging: 2 h 35 min, PD trigger OK; 21 V on USB blew the TVS (it saved the unit). | 4 | [#16625](https://t.me/S2084K/16625), [#36290](https://t.me/S2084K/36290) | HW. FR-11 (measure while charging) is not raised by the community. |

## 9. Touch UI, menus, display, discoverability

| Item | Users | Evidence | opendm40 status / proposal |
|---|---:|---|---|
| Split opinion. Praise: "очень продуманное управление", one-handed light taps (5 users). Complaint: touch is fiddly, small targets, "в дм40 просто боль меня на пару недель хватило", wants physical keys (4–5 users). | 10 | [#15801](https://t.me/S2084K/15801), [#38175](https://t.me/S2084K/38175), [#44652](https://t.me/S2084K/44652), [#26950](https://t.me/S2084K/26950), [#54968](https://t.me/S2084K/54968), [#54975](https://t.me/S2084K/54975), [#59921](https://t.me/S2084K/59921) | **FR-14** (direct access), **FR-17** (big buttons), confirmed. Physical keys = **HW-2**. ui-review's pressed-state feedback (#11) addresses "need to tap precisely". |
| Mode buttons too small; wish haptic feedback. | 2 | [#26950](https://t.me/S2084K/26950), [#26952](https://t.me/S2084K/26952) | FR-17 + ui-review #11 (no vibration motor = HW-2) |
| Passive stylus barely registers (works 1 in 2–3 taps); touch resolution low. | 2 | [#50846](https://t.me/S2084K/50846), [#54986](https://t.me/S2084K/54986) | **NEW, UF-31, P2, S (investigate).** Check whether the Hynitron 0x5A / GT1158 exposes a sensitivity/threshold register. Only change it if it is documented or read back from the stock init; risk of false touches. **2026-10-21:** no documented sensitivity / threshold register: the stock writes only D1 01, D2 04 (+read), D1 09 to the 0x5A chip (touch_v2_6.h, "nothing else is written"), and the CST226SE-class protocol exposes none; not changed. A firmware-side candidate: `touch.rs` `Debounce` drops a touch whose single pressed frame is followed by a release frame (the release then equals the idle state), which would lose short, weak stylus contacts. Device check first: DBG:TOUCH? log of stylus taps; if lone pressed frames show up, add a "Touch: Stylus" setting that accepts them. |
| Touch's left-side nav zone stops responding near a strong HF source; recovers without reboot. | 1 | [#29263](https://t.me/S2084K/29263) | HW/EMI. opendm40 touch debounce must not latch a stuck state; log it as a test case. |
| No printed manual; many functions undiscoverable (screenshot gesture, REL hold, cursors, lock). | 5 | [#48875](https://t.me/S2084K/48875), [#54957](https://t.me/S2084K/54957), [#54966](https://t.me/S2084K/54966), [#50263](https://t.me/S2084K/50263), [#17517](https://t.me/S2084K/17517) | **NEW, UF-29, P2, S.** (Done in 7b58f84: Settings -> Help.) A "Help" page in Menu (one screen per view: gestures, side keys, REL/MIN-MAX resets, input limits: Hz/AC bandwidth, generator max, thermocouple rating). It complements ui-review's side-key legend and page dots. |
| Font unappealing; wish selectable fonts. | 2 | [#16315](https://t.me/S2084K/16315) | opendm40 uses the stock fonts; no action (ui-review §0 font options) |
| Screen glare at the stand's 45° angle. | 1 | [#50822](https://t.me/S2084K/50822) | HW |

## 10. Bluetooth app, USB / PC software, files

| Item | Users | Evidence | opendm40 status / proposal |
|---|---:|---|---|
| Laptop BT can't see the meter; it **only connects while on the DMM page**. | 3 | [#51232](https://t.me/S2084K/51232), [#51235](https://t.me/S2084K/51235), [#51245](https://t.me/S2084K/51245) | Done by design: `bt.serve` runs every main-loop pass regardless of view (`main.rs` main loop). **UF-36, P2, S:** check that the app connects from Scope/Gen views and keeps up with its 50 ms poll setting. **2026-10-21:** confirmed by code: `bt.poll` runs every 5 ms in every view, `serve` answers cmd 09 from the shown reading, and the DMM rail stays on while the module reports a link (PC14), so the scope / generator pages keep measuring. Device check: `firmware/tools/dm40scpi.py --bt btpoll -p 50` on each page (replies/s, latency) and DBG:PROF? loop time < 50 ms on the scope page (the RX ring covers 89 ms). Exception (deliberate): while the FR-38 logger session runs, main.rs neither serves BT frames nor runs the module AT init (the logger owns the HY3131), so the phone app sees no replies on any page then. |
| AC+DC on the **DM40A**: the A menu lacks it, but pressing AC+DC in ATK-XTOOL 2.0.0 / DM40Visor / DM40GUI makes the A measure and display AC+DC V and A. App 2.0.11 closed the loophole. | 4 + 4 | [#50705](https://t.me/S2084K/50705), [#50798](https://t.me/S2084K/50798), [#50803](https://t.me/S2084K/50803), [#50723](https://t.me/S2084K/50723), [#50812](https://t.me/S2084K/50812) | **NEW, UF-03, P1, M.** (Done: AC+DC on all models; untested on the device.) The A hardware does AC+DC; stock gates it by model only. opendm40 parity marks AC+DC "N/A (DM40A) by design". Port the B/C AC+DC sub-mode (config rows + `poll_ac` DC/AC split, aux AC/DC components) and enable it on A. The BT mode byte decode already covers the sub-mode. Risk: the A's discrete divider may show slightly worse AC+DC accuracy; label it "AC+DC (A: uncalibrated)" until checked against a reference. |
| Official Windows app broken (dead scan button, won't connect after clean install), Chinese-only; CSV uses a **decimal comma**; Defender flags third-party DM40GUI. | 6 | [#34445](https://t.me/S2084K/34445), [#46191](https://t.me/S2084K/46191), [#45878](https://t.me/S2084K/45878), [#45836](https://t.me/S2084K/45836), [#50868](https://t.me/S2084K/50868), [#51237](https://t.me/S2084K/51237) | opendm40 USBTMC/SCPI is the answer (done; on-device test pending in TODO). **NEW, UF-32, P1, S** (done; no RTC, so time since the start instead of ISO dates): opendm40 CSV files (logger, CSV record, waveform save) use '.' decimals, ',' separators, ISO-8601 timestamps and a header line (model, fw, function, range, unit). |
| Wish: one PC app for all devices; chart zoom/time axis; long autonomous logging to a file (~24 h car battery drain) without a phone. | 3 | [#34609](https://t.me/S2084K/34609), [#44962](https://t.me/S2084K/44962), [#57099](https://t.me/S2084K/57099) | FR-04 / TODO "CSV-to-disk recording", confirmed. FR-38 logger done (untested). A small reference host script (Python, pyvisa/usbtmc) for SCPI logging would answer the "PC software" wish: **UF-38, P2, S.** **2026-10-21:** `firmware/tools/dm40scpi.py` (a907213): info / conf / read / log CSV (ISO-8601, '.' decimals, header) / scpi / btpoll, over USBTMC or Bluetooth (the dm40dbg / dm40bt_scpi transports; pyvisa not needed). |
| Outside disk mode USB enumerates as an unknown device (VID_0000/PID_0002). | 1 | [#16670](https://t.me/S2084K/16670) | Done (composite USBTMC+MSC always present) |
| Disk holds only ~28 screenshots (~15 MB, half used); a settings file sits in the root. | 2 | [#16839](https://t.me/S2084K/16839), [#50880](https://t.me/S2084K/50880) | Note for UF-17: write screenshots as RLE/PNG-lite or 16-bit BMP (stock is likely 24-bit); keep the opendm40 disk compatible with stock files. |
| Phone app: English only partial, must uninstall to update, iOS later; view screenshots in the app; multiple curves. | 6 | [#61791](https://t.me/S2084K/61791), [#61821](https://t.me/S2084K/61821), [#18907](https://t.me/S2084K/18907), [#25149](https://t.me/S2084K/25149) | Vendor app; out of scope. Keep the BT protocol compatible (FR-34). |
| Some BT 5.x PC dongles incompatible with the DM40 BLE. | 1 | [#46537](https://t.me/S2084K/46537) | HW/module |

## 11. Firmware distribution, updates, bricking

| Item | Users | Evidence | opendm40 status / proposal |
|---|---:|---|---|
| Updates hard to obtain (Baidu, needs a Chinese phone; English site lags; download link 403). | 5 | [#15908](https://t.me/S2084K/15908), [#18835](https://t.me/S2084K/18835), [#19339](https://t.me/S2084K/19339), [#43558](https://t.me/S2084K/43558) | Process note: publish opendm40 releases + the stock update files' hashes openly. Not a code item. |
| Updates praised: "обновы действительно значительно его улучшали", changelogs kept. Users also fear development has stopped; vendor says engineers are busy. | 6 + 5 | [#38481](https://t.me/S2084K/38481), [#51339](https://t.me/S2084K/51339), [#59170](https://t.me/S2084K/59170), [#44677](https://t.me/S2084K/44677), [#59166](https://t.me/S2084K/59166) | Context: a demand for an alternative firmware exists. |
| Custom firmware interest vs brick fear ("Желающих добровольно окирпичить мультик пока не встречалось"); dead MCU cannot be revived (RDP, no full dump, cal loss). | 4 | [#54079](https://t.me/S2084K/54079), [#54115](https://t.me/S2084K/54115), [#51645](https://t.me/S2084K/51645), [#51660](https://t.me/S2084K/51660) | The native-boot plan (TODO: FS-loader recovery, marker-zero, stock ↔ opendm40 toggle) is exactly what owners need; **community confirms its P0 status**. Pair it with UF-20 (cal backup) and a one-page "how to go back to stock" in the README. |
| Update procedure: enable USB disk, copy `.bin`, power off; works from a phone over USB-C. App OTA update added in mid-2025. | 3 | [#18904](https://t.me/S2084K/18904), [#18905](https://t.me/S2084K/18905), [#27990](https://t.me/S2084K/27990) | Keep the stock update flow intact (opendm40 disk = stock disk). App OTA over BT is not in scope. |
| Custom boot logo (V2.1 `logo.jpg`) → some units then boot with no logo and instantly. | 2 | [#26090](https://t.me/S2084K/26090), [#40376](https://t.me/S2084K/40376) | n/a (opendm40 has no logo) |

## 12. Hardware-only issues (cannot be fixed in firmware)

The board itself (parts, pins, the cited owner reports in context) is
described in [hw/README.md](../../hw/README.md); this table is the owners'
view.

| Issue | Users | Evidence |
|---|---:|---|
| Scope bandwidth roll-off from ~3 MHz; 50 MS/s aliasing; single channel | 12 | §6.3 |
| Scope input C ~30–35 pF vs standard probes; flimsy compensation trimmer; no ground spring; MCX not BNC | 8 | [#30339](https://t.me/S2084K/30339), [#43276](https://t.me/S2084K/43276), [#28400](https://t.me/S2084K/28400), [#51063](https://t.me/S2084K/51063), [#15801](https://t.me/S2084K/15801) |
| Pre-2025 units unshielded (noise, EMI false beeps); the factory foil floats; owners add grounded copper foil | 6 | [#15917](https://t.me/S2084K/15917), [#17950](https://t.me/S2084K/17950), [#37204](https://t.me/S2084K/37204) |
| The meter radiates RF (must be switched off in an anechoic chamber) | 1 | [#16019](https://t.me/S2084K/16019) |
| Temperature drift ~20–27 ppm/°C on DC V (unit-dependent); doubts about the "precision" network | 6 | §4 |
| DM40A: discrete divider instead of TC1746B6; 40 k counts; cannot become a C | 8 | [#25087](https://t.me/S2084K/25087), [#34902](https://t.me/S2084K/34902), [#51830](https://t.me/S2084K/51830) |
| Input protection (FMMT619 pair) failed after 380 V; generator output dies at 12 V (SGM3157) | 4 | [#41820](https://t.me/S2084K/41820), [#24902](https://t.me/S2084K/24902) |
| Battery small (10–15 h); bundled USB cable 250–300 mΩ; 21 V on USB blows the TVS | 6 | §8, [#46594](https://t.me/S2084K/46594), [#36290](https://t.me/S2084K/36290) |
| Side power button recessed in the bumper; wish for physical function keys | 5 | [#17422](https://t.me/S2084K/17422), [#44653](https://t.me/S2084K/44653), [#54975](https://t.me/S2084K/54975) (= HW-2) |
| Probe needles wear and no spares; tight caps; leads break at the handle; L vs straight plugs vary by batch | 8 | [#19910](https://t.me/S2084K/19910), [#20172](https://t.me/S2084K/20172), [#19050](https://t.me/S2084K/19050), [#19006](https://t.me/S2084K/19006) |
| Kickstand hinge detaches; magnets missing in some kits; LCD fragile; glare | 8 | [#22636](https://t.me/S2084K/22636), [#40192](https://t.me/S2084K/40192), [#62036](https://t.me/S2084K/62036), [#50822](https://t.me/S2084K/50822) |
| Bundled thermocouple only 300 °C; no inductance mode | 3 | [#54853](https://t.me/S2084K/54853), [#32823](https://t.me/S2084K/32823) |

---

## 13. Existing items the community confirms (no new work, priority signal only)

- **Native-format firmware / unbrickable recovery** (TODO top priority): owners fear bricking and losing calibration (§11).
- **FR-01 AutoHold / FR-02 AVG**: AVG on the chart is asked for ([#19324](https://t.me/S2084K/19324)).
- **FR-04 / CSV-to-disk recording** and **FR-38 logger**: 24 h autonomous logging ([#57099](https://t.me/S2084K/57099)).
- **FR-05 smoothing**: last-digit dance (default off: speed is praised).
- **FR-09 REL** and **FR-24 capacitance** (persistent stray-C null, "charging…" indicator).
- **FR-10 auto-off** (activity based, warning beep).
- **FR-13 sub-mode memory**: continuity → V → back = diode ([#24836](https://t.me/S2084K/24836), reported to the vendor and still unfixed).
- **FR-14 direct access / FR-17 big buttons**: touch is fiddly (§9).
- **FR-19 texts**: online/offline ohm naming.
- **FR-21 continuity latch / anti-vibration**: EMI false beeps.
- **FR-26 temperature offset and TC type**: **raise to P1**; most persistent DMM complaint.
- **FR-28/29/32 generator offset/units/arb**: the generator is "too basic".
- **FR-34 BT protocol compatibility**.
- **FR-36 user calibration**: repeated requests; the vendor refuses.
- **USBTMC/SCPI**: the official PC app is broken; another fw author's first wish was "USB SCPI".
- **ui-review** items:
  - #1: drag vs swipe; the community reports a related H/V drag coupling (UF-05).
  - #4: scope button labels.
  - #5: page dots / side-key legend.
  - #8: trigger handle; the community asks for offset and trigger on opposite sides.
  - #10: generator snap feedback (UF-14).
  - #11: pressed-state feedback.

## 14. New items (UF) at a glance

| ID | Item | Prio | Effort | Section | Status ([TODO.md](../TODO.md)) |
|---|---|---|---|---|---|
| UF-01 | Check the continuity CMPO beeper + diode beep classes on device (latency < 10 ms) | P0 | S | §5.2 | beeper sounds; latency, classes open |
| UF-02 | Cold-junction self-heating with both rails / concurrent instruments: measure, correct | P1 | M | §3 | device check 40728b8: no CJ self-heating from the scope rail; stock comparison open |
| UF-03 | AC+DC sub-mode on the DM40A (hardware proven via the app) | P1 | M | §10 | done |
| UF-04 | Scope AUTO keeps the AC/DC coupling (`autoset_start` forced DC at triage) | P1 | S | §6.1 | done (81e02a7) |
| UF-05 | Scope drag axis lock (dominant axis) | P1 | S | §6.1 | done (d154f60, 100eb48, 74b5c16) |
| UF-07 | Stock-style corner taps for timebase + list picker on long-press | P2 | S | §6.1 | done (ff29cd3) |
| UF-08 | One-tap scope cursors (CUR button) | P2 | S | §6.1 | done (c13d61a) |
| UF-09 | Horizontal trigger position + double-tap centre | P1 | M | §6.1 | done (100eb48, c13d61a) |
| UF-10 | V/div +/- shown as zoom in/out | P2 | S | §6.1 | done (c13d61a) |
| UF-11 | FFT with labelled axes + automatic peak readout | P2 | M | §6.2 | done (386f1ed, 0f4dffb) |
| UF-12 | Full probe attenuation list x0.1…x1000 | P2 | S | §6.2 | done (2124249, 7b1b83c) |
| UF-13 | Save waveform CSV to disk; software pulse-width/runt trigger | P2 | M | §6.2 | done (4146aae, 20fc636) |
| UF-14 | Generator: clamp + visible note instead of a silent 0 Hz (revert the parity snap) | P1 | S | §7 | done (ae3e655) |
| UF-15 | Generator: show the actual (quantised) output frequency; later DDS | P1 | S | §7 | done (ae3e655; no DDS) |
| UF-16 | Generator: ramp direction vs preview (check, align) | P2 | S | §7 | done (ae3e655) |
| UF-17 | 3-finger swipe-down screenshot to the USB disk (stock parity gap) | P1 | M | §2, §10 | done (67fa077) |
| UF-18 | 2-finger swipe-down power-off | P1 | S | §8 | done (67fa077) |
| UF-19 | Measure battery draw of opendm40 vs stock; scope-on-demand power policy | P1 | M | §8 | open |
| UF-20 | Write a calibration + boot-data backup file to the USB disk on first boot | P1 | S | §4 | done; tested on the device (T3) |
| UF-21 | Make AUTO+ (V-first, up-only; already ported) the default autorange; warn on mV-first AUTO | P1 | S | §5.1 | done (4c157a8) |
| UF-23 | PT100/PT1000/NTC temperature via the ohm path | P2 | M | §3 | done; tested on the calibrator |
| UF-24 | Reading tempco compensation from the internal sensor (after FR-36) | P2 | L | §4 | parked (2026-10-21) |
| UF-25 | Optional extended-resolution (extra grey digit) display | P2 | S | §4 | decided: the extra digit always on (8d75505) |
| UF-26 | Label "4-20mA %" and hide it out of range | P2 | S | §5.4 | done (8d75505, 11c4f9b) |
| UF-27 | Allow storing a history slot while HOLD is active | P2 | S | §5.3 | done (8d75505) |
| UF-28 | Volume setting semantics; auto-off warning audible at 0 % | P2 | S | §5.2 | done (8d75505); continuity / diode beeper off at 0 % (0.3.1) |
| UF-29 | On-device Help page (gestures, resets, input limits) | P2 | S | §9 | done (7b58f84) |
| UF-30 | Fast bargraph (≥ 40 Hz) from the HY3131 fast channel | P2 | M | §4 | done (b6a4d10; 10/s, the AD1 rate) |
| UF-31 | Touch sensitivity for stylus (investigate controller threshold) | P2 | S | §9 | no sensitivity register; device check of the debounce open |
| UF-32 | CSV format: '.' decimals, ISO-8601 timestamps, header | P1 | S | §10 | done (no RTC: no ISO dates) |
| UF-35 | Explicit MIN/MAX reset gesture distinct from history clear | P2 | S | §5.3 | done (8d75505) |
| UF-36 | BT works from every view and at the app's 50 ms poll | P2 | S | §10 | by design; device 8b9cf79: 400/400 replies at 50 ms |
| UF-37 | Native shim keeps short-press power-on | P1 | S | §8 | done |
| UF-38 | Reference host script for SCPI logging (pyvisa) | P2 | S | §10 | done (a907213, `tools/dm40scpi.py`) |
| UF-41 | Optional sin(x)/x interpolation at fast timebases | P2 | M | §6.2 | done (e433aad, 086f734) |

"done" means implemented; unless the status says otherwise it is untested on
the device.
