#!/bin/bash
# Build check of the non-default Cargo feature sets (Cargo.toml), so none of
# them rots unnoticed (the recovery-usb build did: recovery.rs kept calling a
# 3-argument Msc::new for weeks):
#   recovery-usb      our own USB mass-storage recovery disk (src/recovery.rs)
#   recovery-file     DM40RCVR.BIN + power off (src/recovery.rs, fatren.rs)
#   no-ad2-isr        --no-default-features --features vdisk (no interrupt)
#   spi-disk          --no-default-features --features ad2-isr (the writable
#                     SPI disk as the USB LUN, no generated volume)
#   fatren-dry-run, keep-screen
#   acdc-meth         the AC+DC prototype methods B / C (src/acdcm.rs, ~1 100
#                     lines no default build compiles)
# Each one is a full build.sh run (link asserts: TCM stack, SRAM, regions
# A2 / B / C; stackdepth.py --check 20480; pack + whole 128-byte blocks) into
# its own DM40_TDIR / DM40_OUT under target/variants/, so firmware/target/native
# and opendm40.bin (the image tools/reload.sh installs) are never
# replaced. Not run on any device. One line per variant, then PASS / FAIL.
# ~1-2 min per variant (a full release link each).
set -u
cd "$(dirname "$0")/../.."
V=target/variants
mkdir -p "$V"
fail=0
run() { # name, build.sh args...
    local n=$1; shift
    if DM40_TDIR=$V/$n DM40_OUT=$V/$n/opendm40-$n.bin ./build.sh "$@" > "$V/$n.log" 2>&1; then
        echo "ok   $n: $(grep -m1 '^stack:' "$V/$n.log" | sed 's/.*headroom/stack headroom/'); $(grep -m1 'region B free' "$V/$n.log" | sed 's/ *(.*//; s/^ *//; s/  *= */ /'); $(grep -m1 'region C free' "$V/$n.log" | sed 's/ *(.*//; s/^ *//; s/  *= */ /')"
    else
        echo "FAIL $n (build.sh $*): $V/$n.log"
        grep -E '^error|error:|assert|Error' "$V/$n.log" | head -5 | sed 's/^/     /'
        fail=1
    fi
}
run recovery-usb --features recovery-usb
run recovery-file --features recovery-file
run no-ad2-isr --no-default-features --features vdisk
run spi-disk --no-default-features --features ad2-isr
run fatren-dry-run --features fatren-dry-run
run keep-screen --features keep-screen
run acdc-meth --features acdc-meth
[ -f opendm40.bin ] && echo "opendm40.bin untouched: $(stat -c %y opendm40.bin)"
if [ $fail = 0 ]; then echo PASS; else echo FAIL; exit 1; fi
