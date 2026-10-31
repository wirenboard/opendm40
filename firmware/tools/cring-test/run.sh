#!/bin/bash
# Host test of src/cring.rs (the scope's coded sample ring): read-back of
# every live sample, the 65 520-sample floor (white noise), coded blocks =
# the DLG scope block coding, the views; bytes per sample per signal.
#   tools/cring-test/run.sh
set -e
cd "$(dirname "$0")"
RUSTUP_HOME="$(cd ../../../toolchain/rustup && pwd)"; CARGO_HOME="$(cd ../../../toolchain/cargo && pwd)"; export RUSTUP_HOME CARGO_HOME; PATH="$CARGO_HOME/bin:$PATH"
B=$(mktemp -d)
trap 'rm -rf "$B"' EXIT
rustc --edition 2021 -O main.rs -o "$B/t"
"$B/t"
