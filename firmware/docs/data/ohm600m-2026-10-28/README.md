# 600 MΩ method data (2026-10-28, one DM40A)

See [ohm-600m.md](../../ohm-600m.md).

- [leak-open-op1o.csv](leak-open-op1o.csv), [leak-open-pb0.csv](leak-open-pb0.csv):
  `tools/srcmap.py --load open`, R100K / R1M / R10M, sense OP1O / PB0.
- [cap-blocks.txt](cap-blocks.txt): `DBG:HY:CAP?` on the 6 MΩ row, R22 0x16, R31 AGNDP9, R28 0x31; Vx =
  OP1O − PB3 (R24 0x35), Vfb = FB − PB3 on AD1 directly (R24 0x15). Open jacks, 0.9925 MΩ, 5.5693 MΩ.
- [phases-5m57.csv](phases-5m57.csv): every conversion on 5.5693 MΩ; cfg A = OP1 on PB0 (R32 0x3C), B1 = FB on
  AD1 directly (R24 0x15), B2 = FB through OP1 (R32 0x39), 2 results dropped after each row write.
- [stock-readings.txt](stock-readings.txt): the stock 6 MΩ / 60 MΩ rows on the same resistors.
- [build-0.4.0-5m57.txt](build-0.4.0-5m57.txt): the 0.4.0 build's r7 on 5.5693 MΩ (`DBG:READout?`, `DBG:OHM6?`).
- [build-0.4.0-open.txt](build-0.4.0-open.txt): r7 on open jacks, Vfb − Vx per reading (the leak), before the
  leak correction.
- [build-0.4.0-two.txt](build-0.4.0-two.txt), [build-0.4.0-5m57-leak.txt](build-0.4.0-5m57-leak.txt): r7 with the
  leak learned, 0.9925 MΩ (twice) and 5.5693 MΩ; the 5th `DBG:OHM6?` field is the leak (nS).
- [build-35a47b9e-final.txt](build-35a47b9e-final.txt): the final build on 5.5693 MΩ: `CONF:RES 600E6;:READ?`, the
  readings, AUTO ON (to 6 MΩ), MAX (60 MΩ).
- [build-b64b287c-open-13min.csv](build-b64b287c-open-13min.csv): r7 on open jacks for 13 min: the readout, the learned
  leak (nS), Vx, Vfb and their difference per reading.
- [build-b64b287c-a-current.txt](build-b64b287c-a-current.txt), [build-b64b287c-a-current-ranges.txt](build-b64b287c-a-current-ranges.txt):
  a PSU on A / COM: the alerts and readouts on 600 MΩ over time, then 600 MΩ / 60 MΩ / 600 Ω / AUTO with the current on.
