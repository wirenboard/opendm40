#!/bin/sh
# The firmware version as *IDN? and Settings > Device show it (src/ident.rs
# VERSION): Cargo.toml's package version as it is (0.1.0, 0.1.0-pre2,
# 0.1.2), + DM40_VERSION_SUFFIX when set (a CI pull request build:
# 0.3.2-pr148.1a2b3c4, build.rs). Used by build.sh
# and tools/reload.sh for the install name dm40_opendm40_v<version>.bin.
set -e
v=$(sed -n 's/^version = "\(.*\)"/\1/p' "$(dirname "$0")/../Cargo.toml" | head -1)
[ -n "$v" ] || { echo "fw-version.sh: no version in Cargo.toml" >&2; exit 1; }
echo "$v${DM40_VERSION_SUFFIX:-}"
