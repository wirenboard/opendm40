# opendm40 documentation

All documentation of opendm40, by reader. Start at the
[front page](../README.md) if you are new.

## Use the meter

- [User manual](manual/README.md): install and update, every feature, safety
  and alerts, connecting from a computer, backup, recovery and back to stock,
  known limitations. In Russian: [manual/README.ru.md](manual/README.ru.md).
- [SCPI reference](manual/scpi-reference.md): every SCPI command, over USB
  and Bluetooth.
- [CHANGELOG.md](../CHANGELOG.md): the changes per release.
- [CONTRIBUTING.md](../CONTRIBUTING.md): how to report a bug.

## Develop

- [CONTRIBUTING.md](../CONTRIBUTING.md): pull requests, commit messages,
  the documentation style.
- [firmware/README.md](../firmware/README.md): build, toolchain, the USB debug
  link, the developer loop, tests, CI, releasing, the source layout.
- [emu/README.md](../emu/README.md): the emulator (Renode), which runs
  opendm40 and the stock firmware without a meter.
- The HY3131 model in the emulator: [README](../emu/hy3131/README.md),
  [research notes](../emu/hy3131/RESEARCH.md),
  [registers](../emu/hy3131/ref/registers.md).

### Reference

- [native-boot.md](../firmware/docs/native-boot.md): the boot chain, the
  update format, the flash map, the shim, recovery, the USB disk, the serial
  number.
- [ram-budget.md](../firmware/docs/ram-budget.md): the RAM users and when each
  is live.
- [compact-log-format.md](../firmware/docs/compact-log-format.md): the DLG log
  format on the disk, CSV on read.
- [bt-scpi.md](../firmware/docs/bt-scpi.md): SCPI over Bluetooth next to the
  stock phone-app protocol; USBTMC on Windows.
- [bt-protocol.md](bt-protocol.md): the stock Bluetooth phone-app protocol, as
  opendm40 keeps it.
- [model-variants.md](../firmware/docs/model-variants.md): how the firmware
  tells DM40A / B / C apart, and what changes.
- [dmm-accuracy.md](../firmware/docs/dmm-accuracy.md): the manual's accuracy
  table and the DMM page's accuracy label.
- [parity.md](../firmware/docs/parity.md): item by item against the stock
  firmware V2.6.
- [fonts.md](../firmware/docs/fonts.md): the stock bitmap fonts, their storage
  and cost.

### Design notes

- [power-meter-and-misconnect.md](../firmware/docs/power-meter-and-misconnect.md):
  the power meter and the wrong-connection alerts.
- [inrush-current.md](../firmware/docs/inrush-current.md): inrush current
  capture.
- [dmm-input-scope-feasibility.md](../firmware/docs/dmm-input-scope-feasibility.md):
  the HY3131 AD2 stream: the Wave view and the fast logger.
- [acdc-approaches.md](../firmware/docs/acdc-approaches.md): AC+DC in other
  meters and DMM chips, and what fits the HY3131.
- [dmm-rate-filter-survey.md](../firmware/docs/dmm-rate-filter-survey.md): the
  DMM reading rate, integration time and filtering (survey).
- [hy3131-firmware-comparison.md](../firmware/docs/hy3131-firmware-comparison.md):
  the HY3131 configuration per measuring mode in the stock firmware and six other sources.
- [hy3131-frontend-2026-10-26.md](../firmware/docs/hy3131-frontend-2026-10-26.md):
  the FTP-FTN pre-filter capacitor, the AC V coupling and compensation switches, measured.
- [ohm-600m.md](../firmware/docs/ohm-600m.md): the experimental 600 MΩ range:
  method, the V/Ω node's leak, device checks.
- [dac-adc-paths.md](../firmware/docs/dac-adc-paths.md): DAC to ADC paths for
  self-test.
- [combined-view-design.md](../firmware/docs/combined-view-design.md): the
  multimeter and scope combined view.
- [scope-scale-ux.md](../firmware/docs/scope-scale-ux.md): the scope timebase
  and V/div by touch.
- [scope-gen-channel.md](../firmware/docs/scope-gen-channel.md): the
  generator as a computed scope channel (trigger source, trace) and the DMM as CH2.

### Planning

- [firmware/TODO.md](../firmware/TODO.md): the developers' working list.
- [user-feedback.md](../firmware/docs/user-feedback.md): DM40 owners'
  feedback, triaged into UF-nn items.
- [dm40-feature-request.md](history/dm40-feature-request.md): the FR-nn feature list
  (Russian, archived).

### Test records

- [device-check-2026-10-21.md](history/device-check-2026-10-21.md):
  the device test log, per build (archived).
- [Accuracy against calibrators](../firmware/docs/data/README.md): DC V, DC A,
  frequency and resistance, with the CSV files.

## Hardware

- [hw/README.md](../hw/README.md): the board: block diagram, parts, pins, the
  multimeter front end, the scope, power, hardware recovery.
- [Teardown photos](../hw/photos/README.md) and
  [datasheets](../hw/datasheets/README.md) (third-party material).
- [stock/README.md](../stock/README.md): tools for the stock update file; the
  [stock firmware V2.6](../stock/vendor/README.md); the
  [stock patch images](../stock/patch/README.md) and the
  [memrw backup kit](../stock/patch/kit/README.md).

## History

- [history/README.md](history/README.md): finished plans, reviews,
  investigations and decision logs. Not maintained.
