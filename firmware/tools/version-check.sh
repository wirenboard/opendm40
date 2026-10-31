#!/bin/sh
# Version guard (README "Releasing"), run by CI on every push.
#   tools/version-check.sh            "not bumped since the last tag" check
#   tools/version-check.sh --tag vX   the tag names the Cargo.toml version
# Default: take the newest v* tag reachable from HEAD. If the Cargo.toml
# version (tools/fw-version.sh) still equals that tag's version while the
# firmware inputs changed since the tag, fail: a build from HEAD would carry
# the released version string. Fix: bump the version in Cargo.toml. Needs the
# history and the tags (CI: fetch-depth 0).
# --tag: exit 1 unless the tag is exactly v<fw-version.sh> (0.1.0 -> v0.1.0,
# 0.1.0-pre2 -> v0.1.0-pre2).
set -e
cd "$(dirname "$0")/.."
VER=$(DM40_VERSION_SUFFIX= tools/fw-version.sh) # Cargo.toml's, never a PR suffix
if [ "$1" = --tag ]; then
    if [ "$2" != "v$VER" ]; then
        echo "version-check: tag $2 does not match Cargo.toml (version $VER, tag v$VER expected)" >&2
        exit 1
    fi
    echo "version-check: tag $2 matches Cargo.toml"
    exit 0
fi
TAG=$(git describe --tags --match 'v*' --abbrev=0 HEAD 2>/dev/null) || {
    echo "version-check: no v* tag reachable from HEAD (a shallow clone? CI needs fetch-depth: 0)" >&2
    exit 1
}
# What goes into the image (build.sh): sources, manifest + lock, linker
# script, build script, cargo config, vendored crates.
INPUTS="src Cargo.toml Cargo.lock link-native.x build.rs .cargo vendor"
if [ "v$VER" != "$TAG" ]; then
    echo "version-check: ok, version $VER, last tag $TAG"
    exit 0
fi
if git diff --quiet "$TAG" HEAD -- $INPUTS; then
    echo "version-check: ok, HEAD builds the same firmware as $TAG"
    exit 0
fi
echo "version-check: the firmware changed since $TAG but Cargo.toml still says $VER:" >&2
git diff --stat "$TAG" HEAD -- $INPUTS | tail -1 >&2
echo "bump the version in firmware/Cargo.toml (firmware/README.md, Releasing)" >&2
exit 1
