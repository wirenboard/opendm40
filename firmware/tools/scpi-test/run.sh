#!/bin/bash
# Host test of the SCPI parser (src/scpi.rs, Scpi::handle): suffix
# multipliers, set commands with bad values (no reply, the setting kept, an
# error), the error queue (SYSTem:ERRor?, codes, overflow, *CLS). The modules
# scpi.rs reaches are stubs (main.rs); no hardware.
set -e
cd "$(dirname "$0")"
RUSTUP_HOME="$(cd ../../../toolchain/rustup && pwd)"; CARGO_HOME="$(cd ../../../toolchain/cargo && pwd)"; export RUSTUP_HOME CARGO_HOME; PATH="$CARGO_HOME/bin:$PATH"
B=$(mktemp -d)
trap 'rm -rf "$B"' EXIT
# The version as Cargo passes it; the test checks src/ident.rs derives the
# same string as tools/fw-version.sh (the install file name).
# DBG:OHM6 parses only with its feature (Cargo.toml dbg-ohm6): the parser test has it.
CARGO_PKG_VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' ../../Cargo.toml | head -1) \
    rustc --edition 2021 -O -A warnings --cfg 'feature="dbg-ohm6"' main.rs -o "$B/t"
FW_VERSION=$(../fw-version.sh) "$B/t"
