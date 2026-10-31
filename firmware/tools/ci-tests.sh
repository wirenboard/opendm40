#!/bin/bash
# The host test suites CI runs (.github/workflows/ci.yml); every suite runs,
# the failures are listed at the end (exit 1).
#   tools/ci-tests.sh [suite ...]      default: all of the list below
#   JOBS=<n> tools/ci-tests.sh         suites run at once (default: nproc;
#                                      1 = one after the other, output live)
# With JOBS > 1 each suite's output goes to its own log, printed whole (one
# ::group:: per suite, list order) once all have finished; a line per suite
# is printed as it finishes. The longest suites start first.
# Needs the toolchain (README "Toolchain"; the suites take ../../../toolchain,
# CI links toolchain/rustup and toolchain/cargo to its rustup install),
# python3 with Pillow, mtools and dosfstools (fatren-test, gallery-test).
# Not in the list: build-variants (a separate CI step), the emulator tests
# (emu/, Renode).
cd "$(dirname "$0")"
ROOT=$(cd ../.. && pwd)
SUITES="accuracy-test bargraph-test bt-scpi-test dbgsrc-test dlog-test fft-test gen-test
logfmt-test meter-test scpi-test scpi-msg-test sinc-test ch2-test touch-test trig-test
ui-render gallery-test fatren-test font-matrix stock-ui"
# Start order: the long ones first (seconds on a GitHub runner, 2026-10-25:
# ui-render 275, fatren-test 231, dlog-test 52, font-matrix 33, the rest < 10).
LONG="ui-render fatren-test dlog-test font-matrix"
[ $# -gt 0 ] && SUITES="$*"
JOBS=${JOBS:-$(nproc)}
OUT=$(mktemp -d)
trap 'rm -rf "$OUT"' EXIT
trap 'kill $(jobs -p) 2>/dev/null; exit 130' INT TERM
# stock-ui maps the decoded stock image (repo root, untracked); decode it from
# the vendor update file in git when missing.
case " $SUITES " in *" stock-ui "*)
    [ -f "$ROOT/dm40_v_2_6.dec.bin" ] ||
        python3 -I "$ROOT/stock/dm40fw.py" decode "$ROOT/stock/vendor/dm40_v_2_6.bin" "$ROOT/dm40_v_2_6.dec.bin" ;;
esac

# run <suite>: the suite's run.sh; "<rc> <seconds>" to $OUT/<suite>.rc.
run() {
    local s=$1 args t0 rc
    case $s in
    font-matrix) args="$OUT/font-matrix.txt" ;;
    ui-render|bargraph-test|gallery-test|stock-ui) args="$OUT/$s" ;;
    *) args= ;;
    esac
    t0=$(date +%s)
    "./$s/run.sh" $args
    rc=$?
    echo "$rc $(( $(date +%s) - t0 ))" > "$OUT/$s.rc"
    return $rc
}
result() { # <suite> -> "ok (N s)" / "FAIL (N s)"
    local rc t
    read -r rc t < "$OUT/$1.rc" || { rc=1; t=?; }
    [ "$rc" = 0 ] && echo "ok ($t s)" || echo "FAIL ($t s)"
}

if [ "$JOBS" -le 1 ]; then
    for s in $SUITES; do
        echo "::group::$s"
        run $s
        echo "::endgroup::"
        echo "$s: $(result $s)"
    done
else
    order=
    for s in $LONG; do case " $(echo $SUITES) " in *" $s "*) order="$order $s" ;; esac; done
    for s in $SUITES; do case " $LONG " in *" $s "*) ;; *) order="$order $s" ;; esac; done
    echo "running $(echo $SUITES | wc -w) suites, $JOBS at a time"
    for s in $order; do
        while [ "$(jobs -rp | wc -l)" -ge "$JOBS" ]; do wait -n; done
        { run $s > "$OUT/$s.log" 2>&1; echo "$s: $(result $s)"; } &
    done
    wait
    for s in $SUITES; do
        echo "::group::$s"
        cat "$OUT/$s.log"
        echo "::endgroup::"
        echo "$s: $(result $s)"
    done
fi
failed=
for s in $SUITES; do
    case $(result $s) in ok*) ;; *) failed="$failed $s" ;; esac
done
if [ -n "$failed" ]; then
    echo "FAILED:$failed"
    exit 1
fi
echo "all passed: $(echo $SUITES | wc -w) suites"
