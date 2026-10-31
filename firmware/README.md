# opendm40 firmware: developer README

Sources, tools and docs of opendm40. Features, install, backup and recovery
are in the [user manual](../docs/manual/README.md); all documents are listed
in the [documentation index](../docs/README.md). Paths below are relative to
`firmware/`, and the commands run there.

## Build

```
./build.sh                    # -> opendm40.bin + dm40_opendm40_v<version>.bin
tools/reload.sh               # build, upload over USBTMC, install, check
tools/ui-render/run.sh OUT    # render every screen to PNG + layout check
tools/scpi-doc-check/run.sh   # SCPI parser vs docs/manual/scpi-reference.md
tools/linkcheck.py ..         # every relative link and #anchor in the *.md files
tools/doc-lang-check.sh origin/main  # English and Russian README/manual in step
../emu/run.py fwrs --run-for 4 --shot out.png   # run opendm40 in the emulator
```

`build.sh` links with [`link-native.x`](link-native.x), checks the stack
([`tools/stack-depth/stackdepth.py`](tools/stack-depth/stackdepth.py)) and
the flash / SRAM budgets, and packs the body into the stock update format with
[`../stock/dm40fw.py`](../stock/dm40fw.py). The emulator
([emu/README.md](../emu/README.md)) needs a flash dump of your own meter (not
in the repo).

The version is the package version in [`Cargo.toml`](Cargo.toml) as it is
(`0.2.0`, `0.2.1-rc1`; [`src/ident.rs`](src/ident.rs),
[`tools/fw-version.sh`](tools/fw-version.sh)): `*IDN?`, Settings > Device,
the USB disk texts, the CSV headers and the install file name
`dm40_opendm40_v<version>.bin` take it. Any `dm40*.bin` name installs: the
FS-loader takes the first `dm40*.bin` in the root, and a long name's 8.3
alias (`DM40_O~1.BIN`) starts with `DM40` too.

### Toolchain

A pinned Rust toolchain in `toolchain/` at the repo root (ignored, local).
`build.sh` and the host tests use `../toolchain`. Create it once (tested with
stable 1.99), from the repo root:

```
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs -o /tmp/rustup-init.sh
RUSTUP_HOME=$PWD/toolchain/rustup CARGO_HOME=$PWD/toolchain/cargo \
    sh /tmp/rustup-init.sh -y --no-modify-path --profile minimal \
    --default-toolchain stable --target thumbv7em-none-eabihf --component llvm-tools
```

The host tests also need `mtools` ([`tools/fatren-test`](tools/fatren-test)).

Without `../toolchain` (CI, another checkout) `build.sh` takes the rustup
install that `RUSTUP_HOME` / `CARGO_HOME` name (default `~/.rustup`,
`~/.cargo`), with the same target and `llvm-tools`. The image bytes depend
only on the commit and the rustc version: `build.sh` remaps every absolute
path (checkout, rustup, cargo registry) and nothing in the image is
time-stamped, so rustc 1.99.0 gives the same `opendm40.bin` in any
directory and with a fresh `CARGO_HOME`. CI pins 1.99.0; that its image
matches a local build is inferred (compare the artifact's `SHA256SUMS`).

`build.sh` compiles with `-Zlocation-detail=none` (panic sites without
file / line; ~31 KB of zero-wait flash) and sets `RUSTC_BOOTSTRAP=1` for
that one `cargo build`, so the stable toolchain takes the nightly flag
([docs/native-boot.md](docs/native-boot.md), "Region B audit #6"). A plain
`cargo build` and the host tests do not use it. `RUSTC_BOOTSTRAP` also puts
the dependencies' build scripts on their nightly paths (proc-macro2): recheck
the build after a toolchain or dependency update.

## USB debug link

opendm40 enumerates as a composite USBTMC + mass-storage device (`1209:D040`; builds before 0.1.0: `1209:0001`)
on USBFS. Debug commands are SCPI `DBG:...` over USBTMC
([`src/scpi.rs`](src/scpi.rs)), refused over Bluetooth. The host tool
[`tools/dm40dbg.py`](tools/dm40dbg.py) (PIL for screenshots) needs
read/write access to `/dev/usbtmc*`: the udev rule in the
[user manual](../docs/manual/README.md#access-without-root).
[`tools/dm40scpi.py`](tools/dm40scpi.py) is the SCPI client for readings,
CSV logs and the fast logger (USB or Bluetooth).

The USB disk is a read-only volume generated on the fly; how it is built and
what a PC may write: [native-boot.md, Unit data](docs/native-boot.md#unit-data-the-usb-volume-srcvdiskrs-the-spi-disk-srcfatdirrs-and-the-calibration-safety-copy-srccalbackuprs).

```
python3 tools/dm40dbg.py ping
python3 tools/dm40dbg.py shot out.png        # full screen in ~0.8 s
python3 tools/dm40dbg.py tap X Y             # touch tap; also left / right (view swipe)
python3 tools/dm40dbg.py drag X0 Y0 X1 Y1 [--ms N] [--hold MS]
python3 tools/dm40dbg.py key 0..3 [--hold MS]  # side key: 0 MENU, 1 RUN, 2 AUTO, 3 MF
python3 tools/dm40dbg.py diag                # DBG:DIAG? block (hex)
python3 tools/dm40dbg.py boot                # DBG:BOOT? decoded: reset cause, marker, crash counter...
python3 tools/dm40dbg.py scpi 'DBG:FLASH?'   # any SCPI; this one: SPI flash JEDEC id, blocks, write error
python3 tools/dm40dbg.py upload FILE         # update file -> DM40FW.BIN in the SPI root (DBG:UPLOAD)
python3 tools/dm40dbg.py update [--wait S] [--expect FILE]   # DBG:UPDATE: the loader installs it
python3 tools/dm40dbg.py image FILE          # offline: is FILE a valid dm40*.bin update file?
python3 tools/dm40dbg.py reset               # reboot opendm40
python3 tools/dm40dbg.py release             # stop feeding the watchdog (counts as a crash)
python3 tools/dm40dbg.py recover             # DESTRUCTIVE: into the vendor update screen (recover -h)
```

`python3 tools/dm40dbg.py -h` lists the rest (`hold`, `lcdread`).

**Watchdog.** The main loop feeds it while it runs. A hang, a panic (magenta
bar at the bottom) or `DBG:RELEASE` resets the meter after ~20 s and opendm40
reboots, every time (no automatic entry into the loader; the crash count is in
`DBG:BOOT?`). `dm40dbg.py loader` (`DBG:LOADER`) opens the vendor loader with
the marker kept: a power cycle boots opendm40 again. Details: [native-boot.md, Normal native mode](docs/native-boot.md#normal-native-mode).

**Bring-up failures** show as red squares at the top left (`fail` in
[`src/main.rs`](src/main.rs)): slot 0 = PLLSAI (the 48 MHz USB clock) not
locked, slot 1 = USBFS core not answering (GRSTCTL.AHBIDL; GSNPSID reads 0 on
this GD32).

**USB re-attach.** Resetting USBFS does not make the host see a disconnect
(the D+ pull-up stays), so `usb::init_hw` drives PA11 / PA12 low for 100 ms
first ([`src/usb.rs`](src/usb.rs)).

**Screenshots** read the LCD's GRAM back (0x2E): one dummy word, then RGB666
as 3 bytes per pixel packed big-endian into the 16-bit words
([`src/lcd.rs`](src/lcd.rs)).

## Developer workflow

**Reload while opendm40 runs** (the normal loop):

```
tools/reload.sh [cargo args]     # e.g. --features keep-screen
```

1. `./build.sh [cargo args]`; `dm40dbg.py image` checks the file (vendor
   header CRC, body CRC, size, reset vector) before anything is staged.
2. A running opendm40 that does not answer `DBG:BOOT?` is refused; so is the
   stock "USB Connect" disk or the loader update screen (`28e9:028f`) unless
   `--from-stock`.
3. `dm40dbg.py upload`: `DBG:UPLOAD` writes the file as `DM40FW.BIN` into the
   SPI flash's root (every other `dm40*.bin` there removed first), the meter
   re-reads it and checks the CRC-32 ([`src/upload.rs`](src/upload.rs));
   then `DBG:FLASH?` must report no SPI write error.
4. `dm40dbg.py update --wait 90 --expect FILE`: `DBG:UPDATE` zeroes the loader
   marker and resets; the vendor loader installs the file and deletes it. If
   the device is still on USB after 5 s, `SYST:ERR?` is reported ("Flash
   error" = marker not zeroed, nothing changed). The first boot after an
   install is dark for several seconds: do not power-cycle. If `28e9:028f`
   stays up for 20 s instead, the install failed (loader screen). Then
   `DBG:BOOT?` is printed and the loader marker compared with the file's
   12-byte update header: "installed OK" (~1 min in all).

Options (`tools/reload.sh --help`): `--no-build`, `--image FILE`, `--wait S`,
`--stock FILE`, `--from-stock`.

**First install** (the meter runs stock): `tools/reload.sh --from-stock`
stages the file on the stock "USB Connect" disk and tells you to press
"update" or power-cycle, then waits up to 300 s for opendm40. By hand:
[the manual, Install](../docs/manual/README.md#install).

**Back to stock** from a running opendm40: `tools/reload.sh --stock FILE`
with the official [`../stock/vendor/dm40_v_2_6.bin`](../stock/vendor/dm40_v_2_6.bin)
or `../stock/patch/build-memrw/dm40_patched.bin` (patched stock with
Bluetooth memrw, built by [`../stock/patch/kit/mkkit.sh`](../stock/patch/kit/mkkit.sh)).
Same flow, no wait (stock does not enumerate `1209:D040`).

**Recovery** (opendm40 hangs or does not start):
[the manual, Reach the update loader](../docs/manual/README.md#reach-the-update-loader). `INFO/FIRMWARE.BIN` on the
USB volume (the running image) or `opendm40.bin` both work as the image.

## CI and releases

[`.github/workflows/ci.yml`](../.github/workflows/ci.yml) runs on every push
and pull request to `main`; a push that changes only documentation (Markdown, `docs/`, `firmware/docs/`, `hw/`, images, data) skips the Build and Host tests jobs:

| Job | What |
|---|---|
| Build | `build.sh --locked` (stack, region and update-file checks fail it), [`tools/build-variants`](tools/build-variants/run.sh); the image and `SHA256SUMS` as a workflow artifact |
| Host tests | [`tools/ci-tests.sh`](tools/ci-tests.sh): the host suites listed in the script, `JOBS` at a time (default: the CPU count; each suite's log printed whole after all finish); `stock-ui` decodes [`dm40_v_2_6.bin`](../stock/vendor/dm40_v_2_6.bin) first |
| Doc checks | [`tools/linkcheck.py`](tools/linkcheck.py), [`tools/scpi-doc-check`](tools/scpi-doc-check/run.sh), [`tools/doc-lang-check.sh`](tools/doc-lang-check.sh) (the English and Russian README and manual: same headings and images; on a pull request both languages changed, see [AGENTS.md](../AGENTS.md)) |
| Version | [`tools/version-check.sh`](tools/version-check.sh): fails when the firmware changed since the last `v*` tag but the version did not; on a pull request [`tools/pr-version-check.sh`](tools/pr-version-check.sh) also fails when the PR changes the firmware without raising the version above `main`'s |

The emulator tests ([emu/README.md](../emu/README.md)) are not in CI: they
need a flash dump of a meter.

### PR builds

On a pull request the Build job sets `DM40_VERSION_SUFFIX=-pr<N>.<head commit>`
([`build.rs`](build.rs), [`tools/fw-version.sh`](tools/fw-version.sh)): the
version becomes e.g. `0.3.2-pr148.1a2b3c4`, the file
`dm40_opendm40_v0.3.2-pr148.1a2b3c4.bin`, the artifact the same name without
`.bin`. Pushes, releases and local builds keep the plain version. After CI,
[`pr-build.yml`](../.github/workflows/pr-build.yml) (`workflow_run`, with
write permission, never runs PR code) runs
[`tools/pr-build.sh`](tools/pr-build.sh) `publish`: the `.bin` goes to the
prerelease [`pr-builds`](https://github.com/opendm40/opendm40/releases/tag/pr-builds)
(one file per PR, the previous one deleted), and one comment per PR links it
with its SHA-256 and commits. On close (`pull_request_target`, no checkout of
the PR) `remove` deletes the PR's file. The `pr-builds` tag is not a `v*` tag:
the release workflow and the version checks skip it. These jobs run from
`main` only; by hand: `gh workflow run pr-build.yml -f run_id=<CI run id>`.

### Releasing

Releases follow [Semantic Versioning](https://semver.org/): the version is
`MAJOR.MINOR.PATCH` in [`Cargo.toml`](Cargo.toml), shown as it is, and the tag
is `v` + the version. While MAJOR is 0:

| Change | Bump | Example |
|---|---|---|
| New features, changed behaviour, or anything incompatible (SCPI, settings file, USB ID) | MINOR | `0.2.0` -> `0.3.0` |
| Fixes only | PATCH | `0.2.0` -> `0.2.1` |

MAJOR becomes 1 when the SCPI command set and the settings file are declared
stable; after that an incompatible change bumps MAJOR. Pre-release suffixes
(`-rc1`) are allowed but not used so far.

1. Set `version` in [`Cargo.toml`](Cargo.toml) and run `./build.sh` once, so
   that `Cargo.lock` follows (CI builds with `--locked` and fails on a stale
   lock).
2. In [`../CHANGELOG.md`](../CHANGELOG.md), rename `## Unreleased` to
   `## v<version> (<date>)` and start a new empty `## Unreleased` above it.
   Keep each entry to a line or two; before / after pictures go in
   `docs/changelog/v<version>/` ([`tools/ui-render/changelog-img.py`](tools/ui-render/changelog-img.py)).
3. Commit both files, push the branch, wait for CI.
4. Tag and push the tag: `git tag -a v0.1.0-pre2 -m "opendm40 v0.1.0-pre2"`,
   `git push origin v0.1.0-pre2`.

The tag starts [`.github/workflows/release.yml`](../.github/workflows/release.yml):
the CI jobs, then a check that the tag is `v` + [`tools/fw-version.sh`](tools/fw-version.sh)
(a mismatch fails the release), a clean `build.sh --locked`, and the GitHub
release with `dm40_opendm40_v<version>.bin` and `SHA256SUMS`, a pre-release
for a `-pre` / `-rc` tag. The notes ([`tools/release-notes.sh`](tools/release-notes.sh))
are the install, backup and recovery steps, then the tag's
`CHANGELOG.md` section, or GitHub's generated notes when there is none.

After a release, the next firmware change needs a new version before it is
pushed (the Version job fails otherwise): raise PATCH for a fix or MINOR for a
feature, in the same commit.

## Layout

| Path | What |
|---|---|
| [`src/`](src) | The firmware (crate `opendm40`) |
| [`tools/`](tools) | Host tools ([`dm40dbg.py`](tools/dm40dbg.py) USB debug link, [`dm40scpi.py`](tools/dm40scpi.py) SCPI client, [`dm40bt_scpi.py`](tools/dm40bt_scpi.py), [`reload.sh`](tools/reload.sh), [`dlog.py`](tools/dlog.py), [`c2meter.py`](tools/c2meter.py) (Alientek C2 USB tester over BLE), font and table generators) and the host test suites (`tools/*/run.sh`) |
| [`docs/`](docs) | Firmware docs, see [Documentation](#documentation) |
| [`ref/`](ref) | Reference screenshot of the stock firmware |
| [`vendor/usbd-storage/`](vendor/usbd-storage) | usbd-storage 3.0.0 with the DM40 patches ([`Cargo.toml`](Cargo.toml)) |
| [`TODO.md`](TODO.md) | What is done and what is open |

## Documentation

Every document, grouped by reader (reference, design notes, planning, test
records, hardware, history): [docs/README.md](../docs/README.md).

Host tests: `tools/*/run.sh`, one suite per directory in [`tools/`](tools)
(for example [`fatren-test`](tools/fatren-test), [`scpi-test`](tools/scpi-test),
[`touch-test`](tools/touch-test), [`ui-render`](tools/ui-render)), and
[`tools/linkcheck.py`](tools/linkcheck.py),
[`tools/scpi-doc-check`](tools/scpi-doc-check/run.sh) and
[`tools/doc-lang-check.sh`](tools/doc-lang-check.sh) for the docs. The
emulator tests: [emu/README.md](../emu/README.md).

### Stock-firmware analysis (private)

The reverse-engineering material on the stock firmware is not public. Those
with access clone it into `stock/re/` (ignored by git), so that the
`stock/re/...` paths named in the code and the docs resolve. The emulator
needs the developer's own flash and SPI flash dumps in `stock/re/dumps/`
([emu/README.md](../emu/README.md)). The build and the CI suites do not need
it.
