# Contributing

**Bugs:** open an [issue](https://github.com/opendm40/opendm40/issues/new/choose).
Give the model and the opendm40 version (`*IDN?`), and for a hang or a restart
also the output of `python3 firmware/tools/dm40dbg.py boot`.

**Code:** pull requests go against `main`, and CI must pass. A firmware change
bumps the version in [`firmware/Cargo.toml`](firmware/Cargo.toml). Build and
tests are described in [firmware/README.md](firmware/README.md).

**Docs:** the README and the user manual exist in English and Russian; change
both in the same pull request ([AGENTS.md](AGENTS.md#docs-in-two-languages)).

**Licence:** contributions are MIT ([LICENSE](LICENSE)). Do not add vendor code
or data read from your own meter: calibration, chip ID or flash dumps.
