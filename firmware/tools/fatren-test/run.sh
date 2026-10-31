#!/bin/bash
# Host test of src/fatren.rs, src/calbackup.rs and src/vdisk.rs on a DM40-geometry FAT16
# image (4096-byte sectors, 2 FATs, 512 root entries, 16 MiB; same cluster
# count as the device, 4087). Update files are real packed images
# (stock/dm40fw.py pack); the "marker" is the decoded 12-byte header of the
# installed one (what the loader writes to 0x08008000).
# fatren (review 2026-10-13 H1: only the file equal to the marker is renamed):
#   1. old _DM40PRV.BIN + dm40_v_2_6.bin (LFN, installed) + DM40FW.BIN (staged)
#      + others: dry run, then live -> the installed file renamed, old backup
#      deleted and its clusters freed, the staged file untouched (staged=1),
#      fsck clean, contents unchanged;
#   2. same marker again: CLEAN, staged=1, nothing written; no marker: nothing
#      renamed; marker of DM40FW.BIN: it is renamed (replacing the backup);
#      then CLEAN, no block written;
#   3. cold boot with a staged stock file only (H1 scenario): not renamed, the
#      backup survives;
#   4. a long name with leading spaces ("  dm40 lfn only.bin") is renamed.
#   5. recovery trigger: one 0-byte DM40RCVR.BIN root entry, FAT untouched,
#      not written when a dm40*.bin exists; a normal run removes it.
#   (A long name matching while its 8.3 name does not is handled by
#   lfn_matches() but not exercised here: mtools derives the 8.3 name from it.)
# calbackup (write-once safety copy of CALIB.BIN, BOOTDATA.BIN, UID.BIN):
#   6. first boot writes the three files (nothing else: no OPTBYTES / OTP /
#      ROMBOOT / LOADER / BACKUP.TXT / _DM40PRV.BIN); fsck clean, contents equal;
#   7. second boot: PRESENT, no block written (11 block reads);
#   8. calibration changed after the copy: kept (differs=0x001), nothing written;
#      the owner deletes CALIB.BIN: made again; a foreign UID.BIN is kept;
#   9. a disk of an earlier build (all nine per-boot files): nothing written,
#      nothing deleted; a directory named UID.BIN = conflict; fatren on the
#      same volume; a nearly full disk: NOSPACE, nothing written.
# vdisk (src/vdisk.rs, the read-only USB volume; every sector read through
# read_block into a sparse dump, non-zero sectors twice, from a 1 MiB flash
# image = the device dump when present + the installed firmware at 0x08004000):
#   10. fsck.fat clean, label DM40, FAT16 / 512-byte sectors / 32 KiB
#      clusters, root README.TXT + INFO DEBUG LOGS SCOPE SHOTS, the 7 INFO
#      region files equal their sources, BACKUP.TXT sizes and CRC-32s match
#      zlib, read-only attributes; write_block refused;
#   11. INFO/FIRMWARE.BIN byte-identical to the installed update file (native
#      build, stock dm40_v_2_6.bin, an unpadded 480010-byte body with a
#      stored-as-is tail, a pack --pad image), dm40fw.py info + body CRC ok; no
#      valid loader record or a hazardous short last block -> no FIRMWARE.BIN;
#   12. (at the end) over a SPI image with logs, scope records, screenshots
#      and root files of earlier builds: CSV == dlog.py, BMP == the SPI file,
#      DEBUG/SPIFLASH.BIN == the image; refresh steps: a new file appended,
#      a deleted one (slot 0xE5), lock + bus reset (no compaction), unlock
#      (compacted), forget.
#   20. vwrite (src/vwrite.rs, host writes on that volume; vwrite_test.py):
#      deletes of LOGS / SHOTS / SCOPE files in five write orders (SPI files
#      deleted after both marks, nothing else, fsck clean, the volume = the
#      host's image until the Allow, then regenerated), one mark only (a
#      move: nothing deleted), a delete on the meter while frozen; firmware
#      copies (names, orders, Yes / No, into freed clusters, overwrite, a
#      non-update DM40*.BIN); refusals (new file / dir, README, INFO, rename,
#      label); a lockless host; power cuts in the staging + commit; the
#      captured kernel sequences (captures/*.vcap.gz, vreplay.py).
# upload (src/upload.rs, DBG:UPLOAD): DM40FW.BIN byte-identical, staged
#   dm40*.bin removed, _DM40PRV.BIN kept, chunk sizes, wrong CRC / repeated
#   chunk / not an update file refused, power cut before every block write.
#   Short last 128-byte block (docs/native-boot.md "Update file format"):
#   pack --pad gives whole blocks; an unpadded body whose last block is
#   hazardous (0x1F < len < 128, key position >= len) is refused by pack, a
#   hand-built such file is flagged by dm40fw.py info/unpack and rejected by
#   dm40dbg.py image.
# fatdir (src/fatdir.rs, LOGS/ SHOTS/ SCOPE/ on the SPI disk):
#   11a. native boot step: the three directories created (8 clusters each,
#      "." / ".." checked), fsck clean; again: nothing written; power cut
#      before every block write of the first boot: fsck at worst lost
#      clusters / FAT 1 ahead, the next boot completes them;
#   11b. a one-cluster LOGS/ (mtools) with files grown to 8 clusters, files
#      intact; power cut before every write of the grow (same rule);
#   11c. 130 files in SHOTS/: the next shot is S130 in the 2nd directory
#      cluster, a freed S005 slot / number is reused; an empty directory is
#      removed, a non-empty one / a file name refused.
# screenshot (src/screenshot.rs incremental writer, driven to completion by take_on; synthetic screen):
#   12. S000.BMP in the root (an earlier build's) taken -> SHOTS/S001.BMP
#      (SHOTS/ made by the writer), then S002.BMP; after deleting S000.BMP the
#      next one is SHOTS/S000.BMP; fsck clean; 307266 bytes, header equal to
#      the stock S000.BMP in the disk dump (when present), every pixel equal
#      to the stub screen with bottom-up rows; the other files untouched.
#      (The stub screen mixes run rows and noise rows, so the RLE capture fills up part way and
#      the rest is read from the "GRAM" while writing: both paths.)
#   12b. power cut / screenshot::abort_on after every slice of a shot on a NOR
#      model of write_step: no FAT / root / SHOTS/ page ever erased, fsck at
#      worst the shot's lost clusters / FAT 1 ahead.
#   12c. idle-time preparation (screenshot.rs prep_on): the first 76 free
#      clusters erased, nothing else written; the shot after it erases no
#      data block; power cuts before its erases (fsck clean, the next boot
#      completes); a sector that reads blank but does not program (the
#      shot's retry with an erase); another writer taking a cluster at every
#      step (also behind the write counter); a nearly full disk.
# Untracked inputs (device dumps, dm40_v_2_6.bin, a native build) are used when
# present, else synthetic stand-ins of the same sizes (noted in the log).
# csvlog (src/csvlog.rs non-blocking recorder of compact binary logs, DLG,
# src/dlog.rs; driven pass by pass; rows from main.rs row(i): 100 ms apart,
# values 1e-9..1e4, OL/REL/HOLD/HI/LO). Every check decodes the LOGnnn.DLG
# with tools/dlog.py and validates the CSV:
#   13. long recording, 3000 rows = 25.5 KB over 7 clusters (the old CSV:
#      76 KB): LOG000.DLG, fsck clean, every row checked by Python's csv
#      module, the decoded CSV byte-identical to the old writer's LOG000.CSV
#      (md5); NOR model: every data block erased once (appends are
#      program-only), one LOGS/ block erase per commit;
#   14. interleaved with three screenshots (one started while a commit runs):
#      both BMPs pixel-exact, the CSV complete, no commit started during a shot;
#   15. host write in progress (HOST_WRITING) for 120 rows: no block written
#      meanwhile, nothing dropped; for 1200 rows: FIFO overflow -> rows dropped,
#      the next row flagged GAP, file consistent;
#   16. power cut before every block write of a 1500-row recording (and a
#      program-only write cut half way): fsck finds at most a lost cluster, a
#      LOG chain longer than its size (truncated) or FAT 1 ahead of FAT 2;
#      the committed log always decodes to a clean prefix; CONFIG.BIN untouched;
#      Also with the chain crossing the FAT 1 block boundary (2047 -> 2048:
#      the new entry in block 1 first, the link in block 0 last);
#   17. nearly full disk (5 free clusters): recording ends Full, file valid
#      (the chunk cut at the end of the disk is a torn tail the reader skips);
#   18. gallery: list (names, sizes vs mdir, free bytes), MMEM:CAT? text,
#      delete LOG / BMP / a long-named LOG_WI~1.DLG (LFN entries removed),
#      fsck clean; a delete between two commits of a recording; deleting the
#      file being recorded is refused. (A PC can no longer mount this disk:
#      no "mounted" refusals any more.)
set -e
cd "$(dirname "$0")"
RUSTUP_HOME="$(cd ../../../toolchain/rustup && pwd)"; CARGO_HOME="$(cd ../../../toolchain/cargo && pwd)"; export RUSTUP_HOME CARGO_HOME; PATH="$CARGO_HOME/bin:$PATH"
W=$(mktemp -d)
# KEEP=<dir>: keep the work directory there (debugging).
trap '[ -n "$KEEP" ] && rm -rf "$KEEP" && cp -r "$W" "$KEEP"; rm -rf "$W"' EXIT
rustc --edition 2021 -O --cfg 'feature="recovery-file"' --cfg 'feature="vdisk"' main.rs -o "$W/t"
export MTOOLS_SKIP_CHECK=1
echo "=== MMEM names: a directory prefix must be the file's own (csvlog::parse_path)"
"$W/t" names | grep -v "^ok" || true
"$W/t" names > /dev/null || { "$W/t" names | grep FAIL; echo "FAIL: names"; exit 1; }
mkimg() {
    rm -f $1; truncate -s 16M $1                       # sparse (/tmp quota, see save() in main.rs)
    mkfs.fat -F 16 -S 4096 -s 1 -f 2 -r 512 -R 1 -n "ATK-DM40" $1 >/dev/null
}
FW=../../../stock/dm40fw.py
# mkfw <body> <out.bin>: pack a body into an update file (--pad: whole
# 128-byte blocks, as build.sh does), print its marker.
mkfw() {
    python3 -I $FW pack --pad $1 $2 2>/dev/null
    python3 -I $FW decode $2 $W/dec.tmp >/dev/null
    od -An -tx1 -N12 $W/dec.tmp | tr -d ' \n'
}
head -c 480000 /dev/urandom > $W/a.body
head -c 100000 /dev/urandom > $W/b.body
head -c 200000 /dev/urandom > $W/c.body
MA=$(mkfw $W/a.body $W/a.bin)
MB=$(mkfw $W/b.body $W/b.bin)
MC=$(mkfw $W/c.body $W/c.bin)
echo "markers: $MA $MB $MC"
head -c 50000 /dev/urandom > $W/old.bin
head -c 9000 /dev/urandom > $W/keep.bin
IMG=$W/disk.img
mkimg $IMG
mcopy -i $IMG $W/keep.bin ::CONFIG.BIN
mcopy -i $IMG $W/old.bin ::_DM40PRV.BIN
mcopy -i $IMG $W/a.bin ::dm40_v_2_6.bin
mcopy -i $IMG $W/keep.bin ::keep_dm40.bin
mcopy -i $IMG $W/b.bin ::DM40FW.BIN
mmd -i $IMG ::dm40dir.bin
mcopy -i $IMG $W/keep.bin "::A long name with spaces.txt"
echo "--- before"; mdir -i $IMG :: | grep -E "BIN|bin|TXT|txt"
"$W/t" $IMG dry $MA | tee $W/r0
grep -q "status=DryRun" $W/r0 && grep -q "changed blocks: \[\]" $W/r0 && echo "ok: dry run writes nothing" || { echo "FAIL at line $LINENO"; exit 1; }
"$W/t" $IMG live $MA | tee $W/r1
grep -q "status=Renamed" $W/r1 && grep -q "staged=1" $W/r1 && grep -q "others=1" $W/r1 && echo "ok: installed file renamed, staged one counted" || { echo "FAIL at line $LINENO"; exit 1; }
echo "--- after 1"; mdir -i $IMG :: | grep -E "BIN|bin|TXT|txt"
fsck.fat -n $IMG | tail -1
mcopy -n -i $IMG ::_DM40PRV.BIN $W/o && cmp $W/o $W/a.bin && echo "ok: _DM40PRV.BIN == former dm40_v_2_6.bin" || { echo "FAIL at line $LINENO"; exit 1; }
mcopy -n -i $IMG ::DM40FW.BIN $W/o && cmp $W/o $W/b.bin && echo "ok: DM40FW.BIN (staged) untouched" || { echo "FAIL at line $LINENO"; exit 1; }
mcopy -n -i $IMG ::CONFIG.BIN $W/o && cmp $W/o $W/keep.bin && echo "ok: CONFIG.BIN untouched" || { echo "FAIL at line $LINENO"; exit 1; }
if mdir -i $IMG :: | grep -qi "dm40_v_2_6"; then echo "FAIL: long name left"; exit 1; fi
"$W/t" $IMG live $MA | tee $W/r2
grep -q "status=Clean" $W/r2 && grep -q "staged=1" $W/r2 && grep -q "changed blocks: \[\]" $W/r2 && echo "ok: same marker again: staged file left, nothing written" || { echo "FAIL at line $LINENO"; exit 1; }
"$W/t" $IMG live | tee $W/r2b
grep -q "status=Clean" $W/r2b && grep -q "changed blocks: \[\]" $W/r2b && echo "ok: no valid marker: nothing renamed" || { echo "FAIL at line $LINENO"; exit 1; }
"$W/t" $IMG live $MB | tee $W/r2c
grep -q "status=Renamed" $W/r2c && echo "ok: DM40FW.BIN renamed once it is the installed image" || { echo "FAIL at line $LINENO"; exit 1; }
echo "--- after 2"; mdir -i $IMG :: | grep -E "BIN|bin|TXT|txt"
fsck.fat -n $IMG | tail -1
mcopy -n -i $IMG ::_DM40PRV.BIN $W/o && cmp $W/o $W/b.bin && echo "ok: backup is now the former DM40FW.BIN" || { echo "FAIL at line $LINENO"; exit 1; }
"$W/t" $IMG live $MB | tee $W/r3
grep -q "status=Clean" $W/r3 && grep -q "changed blocks: \[\]" $W/r3 && echo "ok: third run clean, nothing written" || { echo "FAIL at line $LINENO"; exit 1; }
echo "--- H1: cold boot with a staged stock file, opendm40 (MB) installed"
mcopy -i $IMG $W/c.bin ::dm40_v_2_6.bin
"$W/t" $IMG live $MB | tee $W/r3b
grep -q "status=Clean" $W/r3b && grep -q "staged=1" $W/r3b && grep -q "changed blocks: \[\]" $W/r3b && echo "ok: staged file not renamed" || { echo "FAIL at line $LINENO"; exit 1; }
mcopy -n -i $IMG ::_DM40PRV.BIN $W/o && cmp $W/o $W/b.bin && echo "ok: backup kept" || { echo "FAIL at line $LINENO"; exit 1; }
mcopy -n -i $IMG ::dm40_v_2_6.bin $W/o && cmp $W/o $W/c.bin && echo "ok: staged file intact" || { echo "FAIL at line $LINENO"; exit 1; }
"$W/t" $IMG live $MC | grep -q "status=Renamed" && echo "ok: renamed after the loader installed it" || { echo "FAIL at line $LINENO"; exit 1; }
fsck.fat -n $IMG | tail -1
IMG2=$W/disk2.img
mkimg $IMG2
mcopy -i $IMG2 $W/b.bin "::  dm40 lfn only.bin"
mdir -i $IMG2 :: | grep -i lfn
"$W/t" $IMG2 live $MB | tee $W/r4
grep -q "status=Renamed" $W/r4 && echo "ok: long-name file renamed" || { echo "FAIL at line $LINENO"; exit 1; }
fsck.fat -n $IMG2 | tail -1
echo "--- recovery trigger"
IMG3=$W/disk3.img
mkimg $IMG3
mcopy -i $IMG3 $W/keep.bin ::CONFIG.BIN
mcopy -i $IMG3 $W/keep.bin ::DEL.TXT
mdel -i $IMG3 ::DEL.TXT                   # leaves a 0xE5 slot before the end
cp $IMG3 $W/pre.img
"$W/t" $IMG3 trigger | tee $W/r5
grep -q "trigger=Created" $W/r5 && grep -q "changed blocks: \[5\]" $W/r5 && echo "ok: trigger created, only root block 5 written" || { echo "FAIL at line $LINENO"; exit 1; }
mdir -i $IMG3 :: | grep -E "BIN"
fsck.fat -n $IMG3 | tail -1
[ "$(mdir -i $IMG3 :: | grep DM40RCVR | awk '{print $3}')" = 0 ] && echo "ok: DM40RCVR.BIN is 0 bytes" || { echo "FAIL at line $LINENO"; exit 1; }
cmp -s <(dd if=$W/pre.img bs=4096 skip=1 count=4 status=none) <(dd if=$IMG3 bs=4096 skip=1 count=4 status=none) && echo "ok: FAT tables untouched" || { echo "FAIL at line $LINENO"; exit 1; }
"$W/t" $IMG3 trigger | grep -q "trigger=AlreadyThere" && echo "ok: second trigger = AlreadyThere" || { echo "FAIL at line $LINENO"; exit 1; }
IMG4=$W/disk4.img
mkimg $IMG4
mcopy -i $IMG4 $W/b.bin ::dm40_staged.bin
"$W/t" $IMG4 trigger | grep -q "trigger=AlreadyThere" && echo "ok: no trigger written when a dm40*.bin is already staged" || { echo "FAIL at line $LINENO"; exit 1; }
mcopy -i $IMG3 $W/b.bin ::dm40_v_2_6.bin
"$W/t" $IMG3 live $MB | tee $W/r6
grep -q "status=Renamed" $W/r6 && grep -q "fake_deleted=true" $W/r6 && echo "ok: normal boot removes DM40RCVR.BIN and renames the image" || { echo "FAIL at line $LINENO"; exit 1; }
mdir -i $IMG3 :: | grep -E "BIN"
fsck.fat -n $IMG3 | tail -1

echo "=== calbackup (write-once safety copy: CALIB.BIN, BOOTDATA.BIN, UID.BIN)"
# fsckok <img>: fsck.fat -n exit 0 and no "lost"/"differ" message.
fsckok() {
    fsck.fat -n $1 > $W/fsck.out 2>&1 || { cat $W/fsck.out; echo "FAIL: fsck"; exit 1; }
    if grep -qiE "free cluster|differ|lost|orphan|reclaim" $W/fsck.out; then cat $W/fsck.out; echo "FAIL: fsck"; exit 1; fi
    tail -1 $W/fsck.out
}
# same3 <img> <dir>: the three safety-copy files on <img> equal <dir>'s.
same3() {
    for f in CALIB.BIN BOOTDATA.BIN UID.BIN; do
        mcopy -n -i $1 ::$f $W/o && cmp $W/o $2/$f || { echo "FAIL: $f differs from $2"; exit 1; }
    done
    echo "ok: CALIB.BIN, BOOTDATA.BIN, UID.BIN equal $(basename $2)"
}
# Old files (earlier builds' per-boot backup) and anything else: not written.
noold() {
    for f in OPTBYTES.BIN OTP.BIN ROMBOOT.BIN LOADER.BIN BACKUP.TXT _DM40PRV.BIN; do
        mdir -i $1 ::$f >/dev/null 2>&1 && { echo "FAIL: $f written"; exit 1; }
    done
    echo "ok: no OPTBYTES / OTP / ROMBOOT / LOADER / BACKUP.TXT / _DM40PRV.BIN written"
}
mkdir $W/src
# Sources: slices of the (untracked) device dumps when present, else
# synthetic stand-ins of the same sizes (review 60605d2 L5: the suite must
# run in a clean checkout). Synthetic data is seeded random; the bytes the
# tests below change (CALIB 400 / 500, LOADER 0x1F000) are forced to 0 so
# every change is one.
DUMPS=../../../stock/re/dumps
DUMP=$DUMPS/full-20261013/flash_08000000_1MB.bin
python3 -I - $W/src <<'PY'
import random, sys
r = random.Random(258)
for name, n in [('CALIB.BIN', 4096), ('BOOTDATA.BIN', 16384), ('OPTBYTES.BIN', 16), ('OTP.BIN', 528),
                ('ROMBOOT.BIN', 16384), ('LOADER.BIN', 131072)]:
    d = bytearray(r.randbytes(n))
    for o in {'CALIB.BIN': [400, 500], 'LOADER.BIN': [0x1F000]}.get(name, []):
        d[o] = 0
    open(sys.argv[1] + '/' + name, 'wb').write(d)
PY
if [ -f $DUMPS/dump_settings.bin ]; then head -c 4096 $DUMPS/dump_settings.bin > $W/src/CALIB.BIN; else echo "note: no $DUMPS/dump_settings.bin, synthetic CALIB.BIN"; fi
if [ -f $DUMP ]; then
    dd if=$DUMP of=$W/src/ROMBOOT.BIN bs=4096 skip=0 count=4 status=none
    dd if=$DUMP of=$W/src/LOADER.BIN bs=4096 skip=$((0xA0000 / 4096)) count=32 status=none
    dd if=$DUMP of=$W/src/BOOTDATA.BIN bs=4096 skip=$((0xA4000 / 4096)) count=4 status=none
else
    echo "note: no $DUMP, synthetic ROMBOOT.BIN / LOADER.BIN / BOOTDATA.BIN"
fi
[ $(stat -c %s $W/src/ROMBOOT.BIN) = 16384 ] && [ $(stat -c %s $W/src/LOADER.BIN) = 131072 ] || { echo "FAIL: dump slices"; exit 1; }
head -c 12 /dev/urandom > $W/src/UID.BIN
for f in option_bytes_1FFFC000.bin:OPTBYTES.BIN otp_1FFF7800.bin:OTP.BIN; do
    if [ -f $DUMPS/full-20261013/${f%%:*} ]; then cp $DUMPS/full-20261013/${f%%:*} $W/src/${f##*:}; else echo "note: no ${f%%:*}, synthetic ${f##*:}"; fi
done
IMG5=$W/disk5.img
mkimg $IMG5
mcopy -i $IMG5 $W/keep.bin ::CONFIG.BIN
mcopy -i $IMG5 $W/keep.bin ::DEL.TXT
mdel -i $IMG5 ::DEL.TXT
"$W/t" $IMG5 backup $W/src | tee $W/b1
grep -q "backup=Written written=0x007 conflict=0x000 differs=0x000" $W/b1 && echo "ok: first boot writes the three files" || { echo "FAIL at line $LINENO"; exit 1; }
mdir -i $IMG5 ::
fsckok $IMG5
same3 $IMG5 $W/src
noold $IMG5
"$W/t" $IMG5 backup $W/src | tee $W/b2
grep -q "backup=Present written=0x000 conflict=0x000 differs=0x000" $W/b2 && grep -q "changed blocks: \[\]" $W/b2 && echo "ok: second boot: SAFECOPY_PRESENT, nothing written" || { echo "FAIL at line $LINENO"; exit 1; }
# Boot sector, root, the 1 + 4 + 1 data blocks of the three files and FAT 1
# once per file (each chain walk starts with an empty FAT cache).
grep -qE "reads=11 data_reads=\[[0-9]+, [0-9]+, [0-9]+, [0-9]+, [0-9]+, [0-9]+\]$" $W/b2 && echo "ok: present: 11 block reads" || { echo "FAIL at line $LINENO"; exit 1; }
echo "--- calibration changed after the first copy: never overwritten, reported"
cp -r $W/src $W/src0
printf '\x42' | dd of=$W/src/CALIB.BIN bs=1 seek=400 conv=notrunc status=none
"$W/t" $IMG5 backup $W/src | tee $W/b3
grep -q "backup=Present written=0x000 conflict=0x000 differs=0x001" $W/b3 && grep -q "changed blocks: \[\]" $W/b3 && echo "ok: CALIB.BIN kept (differs=0x001), nothing written" || { echo "FAIL at line $LINENO"; exit 1; }
same3 $IMG5 $W/src0
echo "--- the owner deletes CALIB.BIN: made again from the flash as it is now"
mdel -i $IMG5 ::CALIB.BIN
"$W/t" $IMG5 backup $W/src | tee $W/b4
grep -q "backup=Written written=0x001 conflict=0x000 differs=0x000" $W/b4 && echo "ok: missing CALIB.BIN written" || { echo "FAIL at line $LINENO"; exit 1; }
fsckok $IMG5
same3 $IMG5 $W/src
[ $(mdir -i $IMG5 :: | grep -c "CALIB") = 1 ] && echo "ok: one CALIB.BIN entry" || { echo "FAIL at line $LINENO"; exit 1; }
echo "--- a foreign / short copy is kept as it is (write-once)"
head -c 100000 /dev/urandom > $W/junk
mcopy -o -i $IMG5 $W/junk ::UID.BIN
"$W/t" $IMG5 backup $W/src | tee $W/b5
grep -q "backup=Present written=0x000 conflict=0x000 differs=0x004" $W/b5 && grep -q "changed blocks: \[\]" $W/b5 && echo "ok: foreign UID.BIN kept, differs=0x004" || { echo "FAIL at line $LINENO"; exit 1; }
mcopy -n -i $IMG5 ::UID.BIN $W/o && cmp $W/o $W/junk && echo "ok: UID.BIN untouched" || { echo "FAIL at line $LINENO"; exit 1; }
echo "--- disk of an earlier build (all nine per-boot files): nothing written, nothing deleted"
IMG6=$W/disk6.img
mkimg $IMG6
for f in CALIB.BIN BOOTDATA.BIN UID.BIN OPTBYTES.BIN OTP.BIN ROMBOOT.BIN LOADER.BIN; do mcopy -i $IMG6 $W/src0/$f ::$f; done
echo "old text" > $W/oldtxt; mcopy -i $IMG6 $W/oldtxt ::BACKUP.TXT; mcopy -i $IMG6 $W/old.bin ::_DM40PRV.BIN
mdir -i $IMG6 :: > $W/before6
"$W/t" $IMG6 backup $W/src | tee $W/b6
grep -q "backup=Present written=0x000 conflict=0x000 differs=0x001" $W/b6 && grep -q "changed blocks: \[\]" $W/b6 && echo "ok: old disk: PRESENT, nothing written" || { echo "FAIL at line $LINENO"; exit 1; }
mdir -i $IMG6 :: | diff - $W/before6 && echo "ok: directory unchanged (old files left)" || { echo "FAIL at line $LINENO"; exit 1; }
echo "--- a directory named UID.BIN = conflict; fatren on the same volume"
IMG7=$W/disk7.img
mkimg $IMG7
mmd -i $IMG7 ::UID.BIN
mcopy -i $IMG7 $W/a.bin ::dm40_v_2_6.bin
mcopy -i $IMG7 $W/old.bin ::_DM40PRV.BIN
"$W/t" $IMG7 live $MA | grep -q "status=Renamed" && echo "ok: fatren on the backup volume" || { echo "FAIL at line $LINENO"; exit 1; }
"$W/t" $IMG7 backup $W/src | tee $W/b7
grep -q "backup=Written written=0x003 conflict=0x004" $W/b7 && echo "ok: directory UID.BIN = conflict, CALIB + BOOTDATA written" || { echo "FAIL at line $LINENO"; exit 1; }
fsckok $IMG7
"$W/t" $IMG7 backup $W/src | grep -q "backup=Present written=0x000 conflict=0x004" && echo "ok: then present" || { echo "FAIL at line $LINENO"; exit 1; }
mcopy -n -i $IMG7 ::_DM40PRV.BIN $W/o && cmp $W/o $W/a.bin && echo "ok: fatren's _DM40PRV.BIN untouched" || { echo "FAIL at line $LINENO"; exit 1; }
echo "--- nearly full disk (3 free clusters < 6 needed): NOSPACE, nothing written"
IMG9=$W/disk9.img
mkimg $IMG9
FREE=$(python3 -I - $IMG9 <<'PY'
import struct, sys
d = open(sys.argv[1], 'rb').read()
bps, spc, rsvd, nf, rootn, tot16, fsz, tot32 = (struct.unpack_from('<H', d, 0x0B)[0], d[0x0D],
    struct.unpack_from('<H', d, 0x0E)[0], d[0x10], struct.unpack_from('<H', d, 0x11)[0],
    struct.unpack_from('<H', d, 0x13)[0], struct.unpack_from('<H', d, 0x16)[0], struct.unpack_from('<I', d, 0x20)[0])
data = rsvd + nf * fsz + rootn * 32 // bps
clusters = ((tot16 or tot32) - data) // spc
print(sum(1 for c in range(2, clusters + 2) if struct.unpack_from('<H', d, rsvd * bps + 2 * c)[0] == 0))
PY
)
head -c $(( (FREE - 3) * 4096 )) /dev/zero > $W/fill
mcopy -i $IMG9 $W/fill ::FILL.DAT
"$W/t" $IMG9 backup $W/src | tee $W/b9
grep -q "backup=NoSpace written=0x000" $W/b9 && grep -q "changed blocks: \[\]" $W/b9 && echo "ok: NOSPACE, nothing written" || { echo "FAIL at line $LINENO"; exit 1; }
fsckok $IMG9
mdel -i $IMG9 ::FILL.DAT
"$W/t" $IMG9 backup $W/src | grep -q "backup=Written written=0x007" && echo "ok: written once there is space" || { echo "FAIL at line $LINENO"; exit 1; }
fsckok $IMG9

echo "=== vdisk: the read-only USB volume (INFO/ from the MCU memory)"
# mkflash <fw.bin> <flash.out>: what the installed image looks like in flash
# from 0x08004000: the decoded body, the loader's 12-byte marker (= the file's
# decoded header) at +0x4000.
mkflash() {
    python3 -I - $FW $1 $2 <<'PY'
import importlib.util, sys
spec = importlib.util.spec_from_file_location('dm40fw', sys.argv[1]); f = importlib.util.module_from_spec(spec); spec.loader.exec_module(f)
dec = f.decode(open(sys.argv[2], 'rb').read())
w0, length, w2, body = f.split(dec)
body = bytearray(body)
body[f.MARKER_OFFSET:f.MARKER_OFFSET + 12] = dec[:12]
open(sys.argv[3], 'wb').write(body)
PY
}
# fwok <file> <orig.bin>: <file> is byte-identical to <orig.bin>, and
# dm40fw.py reads it as a valid update file (header words, body CRC).
fwok() {
    cmp $1 $2 || { echo "FAIL: $1 != $2"; exit 1; }
    python3 -I $FW info $1
    python3 -I - $FW $1 <<'PY' || { echo "FAIL: dm40fw.py check"; exit 1; }
import importlib.util, sys
spec = importlib.util.spec_from_file_location('dm40fw', sys.argv[1]); f = importlib.util.module_from_spec(spec); spec.loader.exec_module(f)
w0, length, w2, body = f.split(f.decode(open(sys.argv[2], 'rb').read()))
assert w0 == f.header_word0(length, w2), 'word0'
assert w2 >> 16 == 1 and w2 & 0xffff == f.body_checksum(body), 'word2 / body CRC'
print('header words and body CRC ok')
PY
    echo "ok: FIRMWARE.BIN byte-identical to $(basename $2), valid update file"
}
STOCK=../../../dm40_v_2_6.bin
NATIVE=../../opendm40.bin
# Untracked inputs (L5): stand-ins when absent.
# mkbody <len> <out>: random body with the 12 marker-gap bytes at 0x4000 0xFF.
mkbody() {
    head -c $1 /dev/urandom > $2
    printf '\377\377\377\377\377\377\377\377\377\377\377\377' | dd of=$2 bs=1 seek=$((0x4000)) conv=notrunc status=none
}
if [ ! -f $STOCK ]; then
    echo "note: no $STOCK, synthetic stand-in (body 495128 bytes)"
    mkbody 495128 $W/stock.body && python3 -I $FW pack --pad $W/stock.body $W/stock.bin 2>/dev/null && STOCK=$W/stock.bin
fi
if [ ! -f $NATIVE ]; then
    echo "note: no $NATIVE (./build.sh native), synthetic stand-in"
    mkbody 289000 $W/native.body && python3 -I $FW pack --pad $W/native.body $W/native.bin 2>/dev/null && NATIVE=$W/native.bin
fi
# t: packed WITHOUT --pad: (12 + 480010) % 128 = 22 <= 0x1F, a stored-as-is
# tail the FS-loader also leaves alone (not a hazard), reproduced as is.
head -c 480010 /dev/urandom > $W/t.body
# The 12 marker-gap bytes (body 0x4000..0x400B) are 0xFF in every real build
# (build.sh --gap-fill, the vendor's dm40_v_2_6.bin); the regenerated file
# always has 0xFF there (a file with other bytes differs only there and is
# still valid: the loader's body CRC excludes them).
printf '\377\377\377\377\377\377\377\377\377\377\377\377' | dd of=$W/t.body bs=1 seek=$((0x4000)) conv=notrunc status=none
python3 -I $FW pack $W/t.body $W/t.bin >/dev/null
for f in $STOCK $NATIVE; do [ -f $f ] || { echo "FAIL: $f missing"; exit 1; }; done
mkflash $STOCK $W/flash_stock
mkflash $NATIVE $W/flash_native
mkflash $W/t.bin $W/flash_t
# mkflash1m <fwflash|none> <out>: a 1 MiB internal-flash image: the device
# dump when present (else ROMBOOT / LOADER / CALIB of $W/src at their
# addresses), the installed firmware (mkflash output) at 0x08004000 (none:
# 0xFF there, no valid loader record).
mkflash1m() {
    python3 -I - $DUMP $W/src $1 $2 <<'PY'
import os, sys
dump, src, fw, out = sys.argv[1:]
if os.path.exists(dump):
    f = bytearray(open(dump, 'rb').read())
else:
    f = bytearray(b'\xff' * (1 << 20))
    for name, a in [('ROMBOOT.BIN', 0), ('LOADER.BIN', 0xA0000), ('CALIB.BIN', 0xC0000)]:
        d = open(src + '/' + name, 'rb').read(); f[a:a + len(d)] = d
assert len(f) == 1 << 20
f[0x4000:0xA0000] = b'\xff' * (0xA0000 - 0x4000)
if fw != 'none':
    b = open(fw, 'rb').read(); f[0x4000:0x4000 + len(b)] = b
open(out, 'wb').write(f)
PY
}
# vdisk <flash1m> <img> [step...]: the whole volume through read_block
# (main.rs vdisk_dump: a sparse dump of the 1.98 GiB volume; steps: spi
# <img> / locked / free / reset / forget <NAME>, dumped to <img>.<i>).
vdisk() {
    local fl=$1 img=$2; shift 2
    "$W/t" $img vdisk $fl $W/src/UID.BIN $W/src/OTP.BIN $W/src/OPTBYTES.BIN "$@" | tee $W/vd.out
    grep -q "present_before_init=false" $W/vd.out && grep -q "media_changed_after_init=false" $W/vd.out || { echo "FAIL: init"; exit 1; }
    grep -q "read_only=true write_ro=false rw=true write_block=false read_past_end=false" $W/vd.out && echo "ok: read-only by default (a write refused); writable: a zeroed boot sector refused; nothing past the end" || { echo "FAIL: write_block"; exit 1; }
}
# vdslow <img>: vdisk.rs `read_busy` (main.rs vdisk_dump "slow=" line of the
# last vdisk run) refuses, while a stream runs, exactly the sectors of the
# INFO/ files served from flash >= 0x08080000: CALIB, BOOTDATA, LOADER whole,
# FIRMWARE.BIN from the sector holding file offset 12 + 0x7C000 (body
# 0x08080000); nothing while no stream runs.
vdslow() {
    python3 -I - $1 $W/vd.out <<'PY' || { echo "FAIL: read_busy ranges"; exit 1; }
import re, subprocess, sys
img, out = sys.argv[1:]
line = [l for l in open(out) if l.startswith('slow=')][-1]
m = re.match(r'slow=(\S*) idle_none=(\w+) streaming_edges=(\w+)', line)
got = sorted(tuple(map(int, r.split('-'))) for r in m.group(1).split(',') if r)
assert m.group(2) == 'true' and m.group(3) == 'true', line
d = open(img, 'rb').read(512)
spc, rsvd, nf, rootn = d[13], int.from_bytes(d[14:16], 'little'), d[16], int.from_bytes(d[17:19], 'little')
fsz = int.from_bytes(d[22:24], 'little')
data = rsvd + nf * fsz + rootn * 32 // 512
want = []
ls = subprocess.run(['mdir', '-a', '-i', img, '::INFO'], capture_output=True, text=True).stdout
for name, off in (('CALIB', 0), ('BOOTDATA', 0), ('LOADER', 0), ('FIRMWARE', 12 + 0x7C000)):
    if not re.search(r'^%s +BIN' % name, ls, re.M):
        continue
    f = open(img, 'rb')
    # The directory entry: start cluster + size (INFO/ = cluster 2).
    f.seek(data * 512)
    blk = f.read(spc * 512)
    for k in range(0, len(blk), 32):
        e = blk[k:k + 32]
        if e[:11] == name.ljust(8).encode() + b'BIN':
            c, size = int.from_bytes(e[26:28], 'little'), int.from_bytes(e[28:32], 'little')
            lba0 = data + (c - 2) * spc
            if size > off:
                want.append((lba0 + off // 512, lba0 + (size + 511) // 512))
assert got == sorted(want), (got, sorted(want))
print('ok: read_busy: none without a stream; with one exactly %s (CALIB / BOOTDATA / LOADER whole, the FIRMWARE.BIN tail)' % got)
PY
}
# vdok <img> <flash1m>: fsck clean, label, geometry, every region file equal
# to its source, BACKUP.TXT CRC-32s, read-only attributes.
vdok() {
    fsckok $1
    mdir -i $1 :: | head -3 | grep -q "Volume in drive : is DM40" && echo "ok: label DM40" || { echo "FAIL: label"; exit 1; }
    mdir -i $1 ::INFO > $W/vdir
    mdir -i $1 :: | grep -qE "^README +TXT" && for d in INFO DEBUG LOGS SCOPE SHOTS; do mdir -i $1 :: | grep -qE "^$d +<DIR>" || { echo "FAIL: no $d/"; exit 1; }; done && echo "ok: root: README.TXT, INFO DEBUG LOGS SCOPE SHOTS" || { echo "FAIL: root"; exit 1; }
    python3 -I - $1 <<'PY' || { echo "FAIL: geometry"; exit 1; }
import struct, sys
d = open(sys.argv[1], 'rb').read(512)
bps, spc, rsvd, nf, rootn, tot16, media, fsz, tot32 = struct.unpack_from('<HBHBHHBH', d, 0x0B) + struct.unpack_from('<I', d, 0x20)
data = rsvd + nf * fsz + rootn * 32 // bps
clusters = ((tot16 or tot32) - data) // spc
assert (bps, spc, nf, media) == (512, 64, 2, 0xF8) and 4085 <= clusters < 65525 and d[0x36:0x3E] == b'FAT16   ', (bps, spc, clusters)
assert fsz * bps // 2 >= clusters + 2 and d[510:512] == b'\x55\xaa' and d[0x26] == 0x29
print('ok: FAT16, 512-byte sectors, %d clusters of %d bytes, %.2f GiB' % (clusters, spc * bps, (tot16 or tot32) * bps / 2**30))
PY
    python3 -I - $2 $W/src $1 <<'PY' || { echo "FAIL: region contents"; exit 1; }
import subprocess, sys, zlib
flash, src, img = sys.argv[1:]
f = open(flash, 'rb').read()
regions = {'CALIB.BIN': f[0xC0000:0xC1000], 'BOOTDATA.BIN': f[0xA4000:0xA8000], 'ROMBOOT.BIN': f[:0x4000],
           'LOADER.BIN': f[0xA0000:0xC0000], 'UID.BIN': open(src + '/UID.BIN', 'rb').read(),
           'OTP.BIN': open(src + '/OTP.BIN', 'rb').read(), 'OPTBYTES.BIN': open(src + '/OPTBYTES.BIN', 'rb').read()}
def get(n):
    return subprocess.run(['mtype', '-i', img, '::INFO/' + n], capture_output=True, check=True).stdout
txt = get('BACKUP.TXT').decode()
lines = txt.splitlines()
h = lines.index('file         address     bytes  crc32')
rows = {l.split()[0]: l.split() for l in lines[h + 1:lines.index('', h)]}
for n, want in regions.items():
    got = get(n)
    assert got == want, n
    r = rows[n]
    assert int(r[2]) == len(got) and int(r[3], 16) == zlib.crc32(got), (n, r)
if 'FIRMWARE.BIN' in rows:
    got = get('FIRMWARE.BIN'); r = rows['FIRMWARE.BIN']
    assert int(r[2]) == len(got) and int(r[3], 16) == zlib.crc32(got), r
print('ok: 7 region files equal their sources, BACKUP.TXT sizes + CRC-32s match (%d lines)' % len(rows))
PY
    mattrib -i $1 ::INFO/CALIB.BIN | grep -qE "^ *A? *R|  R " && echo "ok: read-only attribute" || { mattrib -i $1 ::INFO/CALIB.BIN; echo "FAIL: attribute"; exit 1; }
}
for f in $STOCK $NATIVE; do [ -f $f ] || { echo "FAIL: $f missing"; exit 1; }; done
mkflash $STOCK $W/flash_stock
mkflash $NATIVE $W/flash_native
mkflash $W/t.bin $W/flash_t
for k in stock native t; do mkflash1m $W/flash_$k $W/f1m_$k; done
VD=$W/vd.img
vdisk $W/f1m_native $VD
vdok $VD $W/f1m_native
vdslow $VD
mcopy -n -i $VD ::INFO/FIRMWARE.BIN $W/fw.bin && fwok $W/fw.bin $NATIVE
mtype -i $VD ::INFO/BACKUP.TXT
[ $(grep -c " BIN\| TXT" $W/vdir) = 9 ] && echo "ok: INFO/: 9 files" || { echo "FAIL at line $LINENO"; exit 1; }
vdisk $W/f1m_stock $VD
vdok $VD $W/f1m_stock
vdslow $VD
mcopy -n -i $VD ::INFO/FIRMWARE.BIN $W/fw.bin && fwok $W/fw.bin $STOCK
vdisk $W/f1m_t $VD
vdok $VD $W/f1m_t
mcopy -n -i $VD ::INFO/FIRMWARE.BIN $W/fw.bin && fwok $W/fw.bin $W/t.bin
echo "--- no valid loader record: no FIRMWARE.BIN"
mkflash1m none $W/f1m_none
vdisk $W/f1m_none $VD
vdok $VD $W/f1m_none
vdslow $VD
mdir -i $VD ::INFO/FIRMWARE.BIN >/dev/null 2>&1 && { echo "FAIL at line $LINENO: FIRMWARE.BIN"; exit 1; }
mtype -i $VD ::INFO/BACKUP.TXT | grep -q "no FIRMWARE.BIN (no valid loader record" && echo "ok: none, BACKUP.TXT says why" || { echo "FAIL at line $LINENO"; exit 1; }
echo "--- short last 128-byte block: padded images give whole blocks; a hazardous one gives no FIRMWARE.BIN"
# p: a body whose unpadded file would end in a short block; pack refuses it
# when that block is hazardous, --pad always gives whole blocks, and the
# regenerated FIRMWARE.BIN is the same whole-block file (decodes to the flash
# body incl. the 0xFF padding: fwok = byte-identical + header/CRC check).
head -c 393752 /dev/urandom > $W/p.body       # (12 + 393752) % 128 = 36
printf '\377\377\377\377\377\377\377\377\377\377\377\377' | dd of=$W/p.body bs=1 seek=$((0x4000)) conv=notrunc status=none
python3 -I $FW pack --pad $W/p.body $W/p.bin 2>/dev/null
[ $(( $(stat -c %s $W/p.bin) % 128 )) -eq 0 ] && echo "ok: pack --pad: $(stat -c %s $W/p.bin) bytes = whole 128-byte blocks" || { echo "FAIL at line $LINENO"; exit 1; }
for f in $W/t.bin $W/a.bin $W/b.bin $W/c.bin $NATIVE; do
    python3 -I $FW info $f 2>&1 | grep -q "warning: the last 128-byte block" && { echo "FAIL at line $LINENO: $f hazardous"; exit 1; }
done
echo "ok: no packed test image has a hazardous last block"
# haz: an OLD-STYLE unpadded file (built by hand: pack refuses it) whose last
# block is 84 bytes with [0x1F] = 0xBF -> key position 0x5F >= 84. Installed
# (flash + marker) it must not be regenerated (FwFile::from_flash -> None),
# and the tools must flag it.
python3 -I - $FW $W/haz.body $W/haz.bin <<'PY'
import importlib.util, os, struct, sys
spec = importlib.util.spec_from_file_location('dm40fw', sys.argv[1]); f = importlib.util.module_from_spec(spec); spec.loader.exec_module(f)
n = 128 * 3100 + 84 - 12
body = bytearray(os.urandom(n)); body[f.MARKER_OFFSET:f.MARKER_OFFSET + 12] = b'\xff' * 12
body[128 * 3100 + 0x1F - 12] = 0xBF
w2 = 0x10000 | f.body_checksum(bytes(body))
dec = struct.pack('<3I', f.header_word0(n, w2), n, w2) + bytes(body)
assert f.tail_hazard(dec) == (84, 0xBF, 0x5F)
open(sys.argv[2], 'wb').write(body)
open(sys.argv[3], 'wb').write(f.encode(dec))
PY
python3 -I $FW pack $W/haz.body $W/o 2>&1 | grep -q "refusing to pack" && echo "ok: pack refuses the hazardous body without --pad" || { echo "FAIL at line $LINENO"; exit 1; }
python3 -I $FW info $W/haz.bin 2>&1 | grep -q "warning: the last 128-byte block is short (84 bytes)" && echo "ok: dm40fw.py info warns" || { echo "FAIL at line $LINENO"; exit 1; }
python3 -I $FW unpack $W/haz.bin $W/o 2>&1 | grep -q "warning: the last 128-byte block" && echo "ok: dm40fw.py unpack warns" || { echo "FAIL at line $LINENO"; exit 1; }
python3 -I ../dm40dbg.py image $W/haz.bin > $W/h2 && { echo "FAIL at line $LINENO: dm40dbg image accepted it"; exit 1; }
grep -q "BAD: short last block (84 bytes)" $W/h2 && echo "ok: dm40dbg.py image rejects it (reload.sh refuses to stage)" || { echo "FAIL at line $LINENO"; exit 1; }
python3 -I ../dm40dbg.py image $W/p.bin | grep -q "last 128-byte block: 128 bytes (whole blocks)" && echo "ok: dm40dbg.py image: padded file = whole blocks" || { echo "FAIL at line $LINENO"; exit 1; }
mkflash $W/p.bin $W/flash_p
mkflash1m $W/flash_p $W/f1m_p
vdisk $W/f1m_p $VD
mcopy -n -i $VD ::INFO/FIRMWARE.BIN $W/fw.bin && fwok $W/fw.bin $W/p.bin
[ $(( $(stat -c %s $W/fw.bin) % 128 )) -eq 0 ] && echo "ok: FIRMWARE.BIN of a padded image is whole 128-byte blocks" || { echo "FAIL at line $LINENO"; exit 1; }
mkflash $W/haz.bin $W/flash_haz
mkflash1m $W/flash_haz $W/f1m_haz
vdisk $W/f1m_haz $VD
vdok $VD $W/f1m_haz
mdir -i $VD ::INFO/FIRMWARE.BIN >/dev/null 2>&1 && { echo "FAIL at line $LINENO: FIRMWARE.BIN of a hazardous image"; exit 1; }
echo "ok: hazardous installed image: no FIRMWARE.BIN"

echo "=== screenshot"
# bmpok <img> <name>: size, header (vs the stock dump), pixels vs lcd::pattern.
bmpok() {
    mcopy -n -i $1 ::$2 $W/shot.bmp
    python3 -I - $W/shot.bmp ../../../stock/re/dumps/spi_dumps/dm40_msc_full_2026-10-13.bin <<'PY' || { echo "FAIL: $2"; exit 1; }
import os, struct, sys
d = open(sys.argv[1], 'rb').read()
assert len(d) == 307266, len(d)
if os.path.exists(sys.argv[2]):
    stock = open(sys.argv[2], 'rb').read()[0xD000:0xD000 + 66]
    assert stock[:2] == b'BM' and d[:66] == stock, 'header differs from the stock S000.BMP'
    print('header == stock S000.BMP header')
off, w, h, bpp, comp = struct.unpack_from('<I', d, 10)[0], *struct.unpack_from('<iiHHI', d, 18)[:2], *struct.unpack_from('<HI', d, 28)
assert (off, w, h, bpp, comp) == (66, 480, 320, 16, 3), (off, w, h, bpp, comp)
for r in range(320):
    y = 319 - r                          # bottom-up
    row = struct.unpack_from('<480H', d, 66 + 960 * r)
    for x in range(480):
        want = [((x // 13) * 0x1234 + y), ((x // 3) * 0x0F0F + y), ((x // 2) * 0x3333 + y),
                ((x * 0x9E37 + y * 0x79B9) ^ (y << 11))][y % 4] & 0xFFFF
        assert row[x] == want, (x, y, row[x], want)
print('pixels == stub screen, bottom-up')
PY
}
echo "=== fatdir: LOGS/ SHOTS/ SCOPE/ on the SPI disk (create, grow, delete, power cut)"
# dirok <img> <NAME> <clusters>: <NAME> is a directory whose chain has
# <clusters> clusters, "." = its own cluster, ".." = 0 (the root), every
# other entry zero / a file (independent parse).
dirok() {
    python3 -I - $1 $2 $3 <<'PY' || { echo "FAIL: dirok $2"; exit 1; }
import struct, sys
d = open(sys.argv[1], 'rb').read()
name, want = sys.argv[2].ljust(11).encode(), int(sys.argv[3])
rsvd, nf, fsz, rootn = struct.unpack_from('<H', d, 0x0E)[0], d[0x10], struct.unpack_from('<H', d, 0x16)[0], struct.unpack_from('<H', d, 0x11)[0]
root = (rsvd + nf * fsz) * 4096
data = root + rootn * 32
ents = [d[root + 32 * i: root + 32 * i + 32] for i in range(rootn)]
e = [e for e in ents if e[:11] == name and e[0] != 0xE5]
assert len(e) == 1 and e[0][11] & 0x10, e
c = start = struct.unpack_from('<H', e[0], 0x1A)[0]
chain = []
while c < 0xFFF8:
    chain.append(c); c = struct.unpack_from('<H', d, rsvd * 4096 + 2 * c)[0]
assert len(chain) == want, chain
b = data + (start - 2) * 4096
assert d[b:b + 11] == b'.          ' and struct.unpack_from('<H', d, b + 0x1A)[0] == start
assert d[b + 32:b + 43] == b'..         ' and struct.unpack_from('<H', d, b + 32 + 0x1A)[0] == 0
print('%s: %d clusters %s' % (sys.argv[2], len(chain), chain[:3]))
PY
}
IMGD=$W/diskd.img
mkimg $IMGD
mcopy -i $IMGD $W/keep.bin ::CONFIG.BIN
cp $IMGD $W/dirbase.img
"$W/t" $IMGD fatdir boot | tee $W/d1
grep -q "fatdir boot ok=0x7 created=0x7 failed=0x0" $W/d1 && echo "ok: LOGS SHOTS SCOPE created" || { echo "FAIL at line $LINENO"; exit 1; }
fsckok $IMGD
for n in LOGS SHOTS SCOPE; do dirok $IMGD $n 8; done
NWD=$(grep -oE "writes=[0-9]+" $W/d1 | cut -d= -f2)
"$W/t" $IMGD fatdir boot | tee $W/d2
grep -q "fatdir boot ok=0x7 created=0x0 failed=0x0" $W/d2 && grep -q "changed blocks: \[\]" $W/d2 && echo "ok: second boot: nothing written" || { echo "FAIL at line $LINENO"; exit 1; }
mcopy -n -i $IMGD ::CONFIG.BIN $W/o && cmp $W/o $W/keep.bin && echo "ok: CONFIG.BIN untouched" || { echo "FAIL at line $LINENO"; exit 1; }
# dircutok <img>: fsck may only report lost clusters / FAT 1 ahead.
dircutok() {
    local rc=0
    fsck.fat -n $1 > $W/fsck.out 2>&1 || rc=$?
    [ $rc -le 1 ] || { cat $W/fsck.out; echo "FAIL: fsck rc=$rc ($2)"; exit 1; }
    if grep -vE "^fsck.fat|files, .*clusters|^$|Leaving filesystem unchanged|FATs differ but appear to be intact|Using first FAT|Reclaimed [0-9]+ unused clusters? \([0-9]+ bytes\)" $W/fsck.out | grep -q .; then
        cat $W/fsck.out; echo "FAIL: unexpected fsck finding ($2)"; exit 1
    fi
}
for k in $(seq 0 $((NWD - 1))); do
    cp $W/dirbase.img $W/cut.img
    "$W/t" $W/cut.img fatdir boot cut $k > $W/dc
    dircutok $W/cut.img "boot cut $k"
    "$W/t" $W/cut.img fatdir boot > $W/dc2
    grep -q "fatdir boot ok=0x7 " $W/dc2 || { cat $W/dc2; echo "FAIL: boot after cut $k"; exit 1; }
    dircutok $W/cut.img "boot after cut $k"
    for n in LOGS SHOTS SCOPE; do dirok $W/cut.img $n 8 >/dev/null; done
done
echo "ok: $NWD cut points of the first boot: fsck at worst lost clusters / FAT 1 ahead, the next boot completes the three directories"
echo "--- grow a one-cluster directory (mtools mmd) with files in it"
IMGG=$W/diskg.img
cp $W/dirbase.img $IMGG
mmd -i $IMGG ::LOGS
mcopy -i $IMGG $W/keep.bin ::LOGS/KEEP.BIN
mcopy -i $IMGG $W/keep.bin ::LOGS/LOG004.DLG
cp $IMGG $W/growbase.img
dirok $IMGG LOGS 1
"$W/t" $IMGG fatdir ensure LOGS | tee $W/g1d
grep -q "fatdir ensure ok clusters=" $W/g1d && echo "ok: ensure LOGS: grown" || { echo "FAIL at line $LINENO"; exit 1; }
fsckok $IMGG
dirok $IMGG LOGS 8
mcopy -n -i $IMGG ::LOGS/KEEP.BIN $W/o && cmp $W/o $W/keep.bin && echo "ok: files in the grown directory intact" || { echo "FAIL at line $LINENO"; exit 1; }
NWG=$(grep -oE "writes=[0-9]+" $W/g1d | cut -d= -f2)
for k in $(seq 0 $((NWG - 1))); do
    cp $W/growbase.img $W/cut.img
    "$W/t" $W/cut.img fatdir ensure LOGS cut $k > $W/dc
    dircutok $W/cut.img "grow cut $k"
    mcopy -n -i $W/cut.img ::LOGS/KEEP.BIN $W/o && cmp -s $W/o $W/keep.bin || { echo "FAIL: grow cut $k: KEEP.BIN"; exit 1; }
    "$W/t" $W/cut.img fatdir ensure LOGS > $W/dc2
    dircutok $W/cut.img "grow after cut $k"
    dirok $W/cut.img LOGS 8 >/dev/null
done
echo "ok: $NWG cut points of a grow: fsck at worst lost clusters / FAT 1 ahead, files intact, the next ensure completes it"
echo "--- slots across directory clusters, numbering over the root + the directory"
"$W/t" $IMGG fatdir boot >/dev/null
for i in $(seq 0 129); do printf 'x' > $W/tiny; mcopy -i $IMGG $W/tiny ::SHOTS/S$(printf %03d $i).BMP; done
"$W/t" $IMGG shot | grep -q "shot=Saved(130)" && echo "ok: S130.BMP after 130 files (the 2nd directory cluster)" || { echo "FAIL at line $LINENO"; exit 1; }
fsckok $IMGG
bmpok $IMGG SHOTS/S130.BMP
mdel -i $IMGG ::SHOTS/S005.BMP
"$W/t" $IMGG shot | grep -q "shot=Saved(5)" && echo "ok: freed S005 reused (its slot in the 1st cluster)" || { echo "FAIL at line $LINENO"; exit 1; }
fsckok $IMGG
bmpok $IMGG SHOTS/S005.BMP
echo "--- delete: an empty directory (only . / ..), a non-empty one refused"
"$W/t" $IMGG fatdir rm SCOPE | grep -q "fatdir rm Ok(true)" && echo "ok: empty SCOPE removed" || { echo "FAIL at line $LINENO"; exit 1; }
fsckok $IMGG
"$W/t" $IMGG fatdir rm SHOTS | grep -q "fatdir rm Ok(false)" && echo "ok: SHOTS (files) kept" || { echo "FAIL at line $LINENO"; exit 1; }
"$W/t" $IMGG fatdir rm "CONFIG  BIN" | grep -q "fatdir rm Err(Bad)" && echo "ok: CONFIG.BIN is a file: refused (Bad)" || { echo "FAIL at line $LINENO"; exit 1; }

echo "=== upload: the update file over USBTMC (upload.rs, DBG:UPLOAD) into the SPI root"
IMGU=$W/disku.img
mkimg $IMGU
mcopy -i $IMGU $W/keep.bin ::CONFIG.BIN
"$W/t" $IMGU fatdir boot >/dev/null
mcopy -i $IMGU $W/b.bin ::dm40_old_staged.bin            # earlier staged files: removed by BEGIN
mcopy -i $IMGU $W/a.bin ::DM40X.BIN
mcopy -i $IMGU $W/old.bin ::_DM40PRV.BIN                 # the backup stays
cp $IMGU $W/upbase.img
"$W/t" $IMGU upload $NATIVE | tee $W/u1
grep -q "upload Ok(($(stat -c %s $NATIVE), " $W/u1 && grep -q "busy=false" $W/u1 && echo "ok: uploaded ($(stat -c %s $NATIVE) bytes)" || { echo "FAIL at line $LINENO"; exit 1; }
fsckok $IMGU
mcopy -n -i $IMGU ::DM40FW.BIN $W/o && cmp $W/o $NATIVE && echo "ok: DM40FW.BIN byte-identical" || { echo "FAIL at line $LINENO"; exit 1; }
[ "$(mdir -b -i $IMGU :: | grep -ciE '/dm40[^/]*\.bin$' )" = 1 ] && mdir -i $IMGU ::_DM40PRV.BIN >/dev/null && echo "ok: the only dm40*.bin (the staged ones deleted, _DM40PRV.BIN kept)" || { mdir -i $IMGU ::; echo "FAIL at line $LINENO"; exit 1; }
M=$(python3 -I - $FW $NATIVE <<'PY'
import importlib.util, sys
spec = importlib.util.spec_from_file_location('dm40fw', sys.argv[1]); f = importlib.util.module_from_spec(spec); spec.loader.exec_module(f)
print(f.decode(open(sys.argv[2], 'rb').read())[:12].hex())
PY
)
"$W/t" $IMGU dry $M | grep -q "status=DryRun found=1 renamed='DM40FW  BIN'" && echo "ok: fatren sees it as the image the loader installs (marker = its header)" || { echo "FAIL at line $LINENO"; exit 1; }
for c in 1 4096 255; do
    cp $W/upbase.img $W/u.img
    "$W/t" $W/u.img upload $NATIVE chunk $c | grep -q "upload Ok(" && mcopy -n -i $W/u.img ::DM40FW.BIN - | cmp -s - $NATIVE || { echo "FAIL: chunk $c"; exit 1; }
done
echo "ok: chunks of 1, 255, 4096 bytes"
cp $W/upbase.img $W/u.img
"$W/t" $W/u.img upload $NATIVE crc 12345678 | grep -q 'upload Err("CRC mismatch") busy=false' && ! mdir -i $W/u.img ::DM40FW.BIN >/dev/null 2>&1 && echo "ok: wrong CRC: refused, no file" || { echo "FAIL at line $LINENO"; exit 1; }
fsckok $W/u.img
cp $W/upbase.img $W/u.img
"$W/t" $W/u.img upload $NATIVE order | grep -q 'upload Err("Data out of order") busy=false' && echo "ok: a repeated chunk: Data out of order, aborted" || { echo "FAIL at line $LINENO"; exit 1; }
"$W/t" $W/u.img upload $NATIVE order keep | grep -q 'upload Err("Data out of order") busy=false' && ! mdir -i $W/u.img ::DM40FW.BIN >/dev/null 2>&1 && echo "ok: the stream goes on after the error: later chunks dropped, END? reports the first error" || { echo "FAIL at line $LINENO"; exit 1; }
fsckok $W/u.img
head -c 300000 /dev/urandom > $W/junk.bin
cp $W/upbase.img $W/u.img
"$W/t" $W/u.img upload $W/junk.bin | grep -q 'upload Err("Not an update file")' && ! mdir -i $W/u.img ::DM40FW.BIN >/dev/null 2>&1 && echo "ok: not an update file: refused" || { echo "FAIL at line $LINENO"; exit 1; }
fsckok $W/u.img
NWU=$(grep -oE "writes=[0-9]+" $W/u1 | cut -d= -f2)
for k in $(seq 0 $((NWU - 1))); do
    cp $W/upbase.img $W/cut.img
    "$W/t" $W/cut.img upload $NATIVE cut $k > $W/uc
    dircutok $W/cut.img "upload cut $k"
    if mdir -i $W/cut.img ::DM40FW.BIN >/dev/null 2>&1; then
        mcopy -n -i $W/cut.img ::DM40FW.BIN - | cmp -s - $NATIVE || { echo "FAIL: upload cut $k: a partial DM40FW.BIN"; exit 1; }
    fi
done
echo "ok: $NWU cut points of an upload: fsck at worst lost clusters / FAT 1 ahead, DM40FW.BIN only ever complete"

IMG7=$W/disk7.img
mkimg $IMG7
mcopy -i $IMG7 $W/keep.bin ::CONFIG.BIN
mcopy -i $IMG7 $W/keep.bin ::DEL.TXT
mdel -i $IMG7 ::DEL.TXT
mcopy -i $IMG7 $W/old.bin ::S000.BMP                  # an earlier build's shot in the root
"$W/t" $IMG7 shot | tee $W/s1
grep -q "shot=Saved(1) medium_changed=true" $W/s1 && echo "ok: S000.BMP (root) taken -> SHOTS/S001.BMP (SHOTS/ made by the writer: no boot step on this image)" || { echo "FAIL at line $LINENO"; exit 1; }
fsckok $IMG7
mdir -i $IMG7 ::S000.BMP >/dev/null && echo "ok: the root's S000.BMP left where it is" || { echo "FAIL at line $LINENO"; exit 1; }
bmpok $IMG7 SHOTS/S001.BMP
"$W/t" $IMG7 shot | grep -q "shot=Saved(2)" && echo "ok: then S002.BMP" || { echo "FAIL at line $LINENO"; exit 1; }
fsckok $IMG7
bmpok $IMG7 SHOTS/S002.BMP
mdel -i $IMG7 ::S000.BMP
"$W/t" $IMG7 shot | grep -q "shot=Saved(0)" && echo "ok: first free number reused: S000.BMP" || { echo "FAIL at line $LINENO"; exit 1; }
fsckok $IMG7
bmpok $IMG7 SHOTS/S000.BMP
mcopy -n -i $IMG7 ::CONFIG.BIN $W/o && cmp $W/o $W/keep.bin && echo "ok: CONFIG.BIN untouched" || { echo "FAIL at line $LINENO"; exit 1; }
mdir -i $IMG7 ::
mdir -i $IMG7 ::SHOTS
echo "--- scope record CSV (scope_csv.rs through the screenshot job, UF-13)"
"$W/t" $IMG7 scopecsv | tee $W/sc1
grep -q "csv=Saved(0) len=5908 .*shot_refused=true shot_result=None" $W/sc1 && echo "ok: SCOP000.DLG (26624 samples of a clean sine, 5908 bytes: predictor + Rice; u16 53325, the CSV 692253), a screenshot meanwhile refused" || { echo "FAIL at line $LINENO"; exit 1; }
fsckok $IMG7
"$W/t" $IMG7 scopecsv 300 | tee $W/sc2 | grep -q "csv=Saved(1) len=[0-9]* " && echo "ok: then SCOP001.DLG (300 samples)" || { echo "FAIL at line $LINENO"; exit 1; }
fsckok $IMG7
# The CSV generated from the DLG (tools/dlog.py) == the pre-DLG writer's
# SCOP000.CSV / SCOP001.CSV for the same records (4146aae scope_csv.rs; md5).
mcopy -n -i $IMG7 ::SCOPE/SCOP000.DLG $W/s.dlg
python3 -I ../dlog.py $W/s.dlg -o $W/s.csv
mcopy -n -i $IMG7 ::SCOPE/SCOP001.DLG $W/s1.dlg
python3 -I ../dlog.py $W/s1.dlg -o $W/s1.csv
[ "$(md5sum < $W/s.csv | cut -c1-32)" = 4921f54425a8f14d910a25e65a4fc3c6 ] && [ "$(md5sum < $W/s1.csv | cut -c1-32)" = cdc42f159193ea1a1cc7b2b2b4f44dd2 ] && echo "ok: decoded SCOP000.CSV / SCOP001.CSV == the old CSV writer's files, byte for byte" || { echo "FAIL at line $LINENO"; exit 1; }
cp $IMG7 $W/scop.img
python3 -I - $W/s.csv <<'PY' || { echo "FAIL: SCOP000.CSV content"; exit 1; }
import math, sys
L = open(sys.argv[1], 'rb').read().split(b'\r\n')
assert L[0] == b'# test record' and L[1] == b'time_s,volts', L[:2]
rows = [l for l in L[2:] if l]
assert len(rows) == 26624, len(rows)
for i in (0, 1, 13311, 13312, 20000, 26623):
    t, v = map(float, rows[i].split(b','))
    code = int(512.0 + 400.0 * math.sin(i * 0.01))
    assert abs(t - (i - 13312) / 1e6) < 1e-9 + abs(t) * 2e-5, (i, t)
    assert abs(v - (-5.0 + code * 10.0 / 1023.0)) < 1e-3, (i, v)
print("ok: SCOP000.CSV parses, time_s / volts exact to the printed digits")
PY
# A current record (the Wave view in A): the column says amperes.
cp $IMG7 $W/scopa.img
"$W/t" $W/scopa.img scopecsv 300 amps | grep -q "csv=Saved(2) " || { echo "FAIL at line $LINENO"; exit 1; }
mcopy -n -i $W/scopa.img ::SCOPE/SCOP002.DLG $W/sa.dlg
python3 -I ../dlog.py $W/sa.dlg -o $W/sa.csv
[ "$(sed -n 2p $W/sa.csv | tr -d '\r')" = "time_s,amperes" ] && echo "ok: a current record's column is time_s,amperes" || { echo "FAIL at line $LINENO"; exit 1; }

echo "--- screenshot: power cut / abort between every two slices (NOR model of write_step)"
# Review 60605d2 H1 / L4: the shot driven slice by slice (screenshot::poll_on)
# on a NOR model (write_step: erase in the first call, 4 pages programmed per
# call), cut after every slice k: the image as it is then (incl. a half
# programmed data block), and the same k stopped by screenshot::abort_on
# (power_off / DBG:RESET path). Required for every k: no FAT / root page left
# all 0xFF (erased / half programmed metadata), and fsck finds at most the
# shot's 76 lost clusters and FAT 1 ahead of FAT 2. Then the shot completes
# from a clean start (fsck clean). Directory blocks (SHOTS/) count as
# metadata like the FAT and the root.
cp $IMG7 $W/cutbase.img
NS=$("$W/t" $W/cutbase.img shotcut | tee $W/sc0 | grep -oE "slices=[0-9]+" | cut -d= -f2)
cp $IMG7 $W/cutbase.img
cat $W/sc0
grep -q "result=Some(Saved(" $W/sc0 && grep -q "erased_meta_pages=\[\]" $W/sc0 || { echo "FAIL at line $LINENO"; exit 1; }
[ "$(grep -o 'metadata writes.*' $W/sc0 | grep -o '"' | wc -l)" = 6 ] && echo "ok: uncut shot: $NS slices, 3 metadata block writes (FAT 1, FAT 2, SHOTS/ block), each in one slice" || { echo "FAIL at line $LINENO"; exit 1; }
# shotcutok <img> <k>: the only fsck findings a cut / abort may leave.
shotcutok() {
    local rc=0
    fsck.fat -n $1 > $W/fsck.out 2>&1 || rc=$?
    [ $rc -le 1 ] || { cat $W/fsck.out; echo "FAIL: fsck rc=$rc ($2)"; exit 1; }
    if grep -vE "^fsck.fat|files, .*clusters|^$|Leaving filesystem unchanged|FATs differ but appear to be intact|Using first FAT|Reclaimed 76 unused clusters \(311296 bytes\)" $W/fsck.out | grep -q .; then
        cat $W/fsck.out; echo "FAIL: unexpected fsck finding ($2)"; exit 1
    fi
}
nlost=0
for k in $(seq 0 $NS); do
    for mode in cut abort; do
        cp $W/cutbase.img $W/cut.img
        "$W/t" $W/cut.img shotcut $k $mode > $W/sc
        grep -q "erased_meta_pages=\[\]" $W/sc || { cat $W/sc; echo "FAIL: $mode after slice $k: erased FAT / root page"; exit 1; }
        [ $mode = abort ] && { grep -qE "busy=false" $W/sc || { cat $W/sc; echo "FAIL: abort $k left the shot busy"; exit 1; }; }
        shotcutok $W/cut.img "$mode $k"
        grep -q "Reclaimed 76" $W/fsck.out && nlost=$((nlost + 1))
    done
done
echo "ok: $((NS + 1)) cut points x (cut, abort): no FAT / root page ever erased, fsck at worst 76 lost clusters / FAT 1 ahead ($nlost cases)"
cp $W/cutbase.img $W/cut.img
"$W/t" $W/cut.img shotcut $((NS - 1)) abort > /dev/null
"$W/t" $W/cut.img shot | grep -q "shot=Saved(" && echo "ok: the next shot after an abort succeeds" || { echo "FAIL at line $LINENO"; exit 1; }

echo "--- screenshot: idle-time preparation of the next shot's clusters (screenshot.rs prep_on)"
# The first 76 free clusters (first fit = what the next shot takes) erased
# in idle time; FAT / root / directories / allocated clusters never written.
# The image: the boot step's LOGS/ SHOTS/ SCOPE/, a file, a deleted file
# (its clusters free with data in them), the rest of the free clusters
# zeros (mkimg): every free cluster needs an erase.
IMGP=$W/prep.img
mkimg $IMGP
"$W/t" $IMGP fatdir boot > /dev/null
mcopy -i $IMGP $W/old.bin ::OLD.BIN
mcopy -i $IMGP $W/keep.bin ::KEEP.BIN
mdel -i $IMGP ::OLD.BIN
cp $IMGP $W/prepbase.img
"$W/t" $IMGP prep run | tee $W/p1
grep -q "state=2 found=76 erases=76 .* meta_same=true used_same=true changed_free=76 blank_first=76/76 " $W/p1 && echo "ok: ready: 76 erases, the first 76 free clusters blank, FAT / root / directories / allocated clusters unchanged" || { echo "FAIL at line $LINENO"; exit 1; }
fsckok $IMGP
"$W/t" $IMGP prep run | grep -q "state=2 found=76 erases=0 .* changed_free=0 " && echo "ok: again (a reboot): all blank, nothing erased" || { echo "FAIL at line $LINENO"; exit 1; }
cp $W/prepbase.img $IMGP
"$W/t" $IMGP prep shot | tee $W/p2
grep -q "state=2 erases=76 shot=Saved(0) shot_erased=0 no_compare=76 next_blank_before=0 state2=2 found2=76 erases2=76 next_blank_after=76" $W/p2 && echo "ok: the shot after it erased no data block (76 programmed without the compare); the next 76 prepared afterwards" || { echo "FAIL at line $LINENO"; exit 1; }
fsckok $IMGP
bmpok $IMGP SHOTS/S000.BMP
cp $W/prepbase.img $IMGP
"$W/t" $IMGP prep shot stale | tee $W/p2s
grep -q "shot=Saved(0) shot_erased=1 no_compare=76 " $W/p2s && echo "ok: a prepared cluster changed behind the counter: its read back fails, rewritten with an erase, Saved" || { echo "FAIL at line $LINENO"; exit 1; }
fsckok $IMGP
bmpok $IMGP SHOTS/S000.BMP
# Power cut before the k-th erase: nothing but free clusters touched (fsck
# clean: no lost clusters, FATs equal), the next boot completes the rest.
for k in 0 1 2 38 75; do
    cp $W/prepbase.img $W/pcut.img
    "$W/t" $W/pcut.img prep run cut $k > $W/pc
    grep -q "meta_same=true used_same=true .* dead=true" $W/pc || { cat $W/pc; echo "FAIL: prep cut $k"; exit 1; }
    fsckok $W/pcut.img > /dev/null
    "$W/t" $W/pcut.img prep run | grep -q "state=2 found=76 erases=$((76 - k)) .* blank_first=76/76 " || { echo "FAIL: prep after cut $k"; exit 1; }
done
echo "ok: power cut before erase 0 / 1 / 2 / 38 / 75: fsck clean, metadata and files unchanged; the next boot erases the rest"
# A sector whose erase a power cut interrupted may read blank and still not
# take a program: the shot's read back fails, the block is written again
# with an erase (once per shot).
cp $W/prepbase.img $IMGP
"$W/t" $IMGP prep weak | tee $W/p3
grep -q "prep weak shot=Saved(0) shot_erased=76 weak_left=0" $W/p3 && echo "ok: a blank-reading sector that does not program: rewritten with an erase, Saved" || { echo "FAIL at line $LINENO"; exit 1; }
fsckok $IMGP
bmpok $IMGP SHOTS/S000.BMP
# Another writer takes the first free cluster after every step of a
# preparation (FAT entry + data 0xA5): its data survives; also behind the
# write counter's back (the FAT re-read right before each erase).
cp $W/prepbase.img $IMGP
for v in "" nobump; do
    "$W/t" $IMGP prep race $v | tee $W/p4
    C=$(grep -oE "cases=[0-9]+" $W/p4 | cut -d= -f2)
    grep -q "intact=$C " $W/p4 && echo "ok: race${v:+ ($v)}: the other writer's cluster intact in all $C cases" || { echo "FAIL at line $LINENO"; exit 1; }
done
# Disk full: 20 free clusters: all of them prepared; the shot itself is
# refused (Full) as before, nothing written.
cp $W/prepbase.img $IMGP
FREE=$(fsck.fat -n $IMGP | grep -o "[0-9]*/[0-9]* clusters" | awk -F'[/ ]' '{print $2 - $1}')
head -c $(( (FREE - 20) * 4096 )) /dev/zero > $W/fill
mcopy -i $IMGP $W/fill ::FILL.DAT
"$W/t" $IMGP prep run | tee $W/p5
grep -q "state=2 found=20 erases=20 .* used_same=true changed_free=20 blank_first=20/20 free=20 " $W/p5 && echo "ok: 20 free clusters: all 20 prepared, nothing else touched" || { echo "FAIL at line $LINENO"; exit 1; }
"$W/t" $IMGP shot | grep -q "shot=Full" && echo "ok: the shot is still refused (Disk full)" || { echo "FAIL at line $LINENO"; exit 1; }
fsckok $IMGP

echo "=== csvlog"
# csvok <img> <name> <rows> <full|prefix|gap>: the binary log <name>
# (LOGnnn.DLG) decoded to CSV by tools/dlog.py (a torn tail chunk is
# skipped), then header, columns, every row recomputed (row(i) in main.rs)
# through Python's csv module.
csvok() {
    mcopy -n -i $1 ::$2 $W/log.dlg || { echo "FAIL: no $2"; exit 1; }
    python3 -I ../dlog.py $W/log.dlg -o $W/log.csv 2>$W/dlog.err || { cat $W/dlog.err; echo "FAIL: $2 does not decode"; exit 1; }
    if [ "$4" != prefix ] && [ -s $W/dlog.err ]; then cat $W/dlog.err; echo "FAIL: $2 has a torn / bad chunk"; exit 1; fi
    python3 -I - $W/log.csv $3 $4 <<'PY' || { echo "FAIL: $2 content"; exit 1; }
import csv, re, sys
path, n, mode = sys.argv[1], int(sys.argv[2]), sys.argv[3]
raw = open(path, 'rb').read()
assert raw.isascii() and raw.endswith(b'\r\n'), 'ASCII, CRLF, ends with a whole row'
lines = raw.decode().split('\r\n')
assert lines[0].startswith('# Alientek DM40A, opendm40 ') and ', function DC V, range AUTO 6 V, started uptime 5.000 s' in lines[0], lines[0]
assert lines[1].startswith('# time_s = seconds since the start'), lines[1]
rd = csv.reader(open(path, newline=''))
rows = [r for r in rd if r and not r[0].startswith('#')]
assert rows[0] == ['time_s', 'value', 'unit', 'flags'], rows[0]
P = [1e-9, 1e-8, 1e-7, 1e-6, 1e-5, 1e-4, 1e-3, 1e-2, 1e-1, 1e0, 1e1, 1e2, 1e3]
num = re.compile(r'^-?(\d+(\.\d+)?|\d\.\d{5}E[+-]\d\d)$')
prev, skipped = -1, 0
for r in rows[1:]:
    assert len(r) == 4, r
    t, v, u, fl = r
    assert re.fullmatch(r'\d+\.\d{3}', t), t
    i = round(float(t) * 10) - 1
    gap = i != prev + 1
    if mode != 'gap':
        assert not gap, (prev, r)
    assert i > prev, (prev, r)
    skipped += i - prev - 1
    prev = i
    m = (i * 7919) % 20001 - 10000
    want = m * P[i % 13]
    f = fl.split()
    exp = [x for x, c in [('OL', i % 97 == 3), ('REL', i % 5 == 0), ('HOLD', i % 7 == 0), ('HI', i % 11 == 0), ('LO', i % 13 == 1)] if c]
    if gap:
        exp.append('GAP')
    assert f == exp, (r, exp)
    assert u == 'Ohm', u
    if 'OL' in f:
        assert v == '', r
    else:
        assert num.match(v), v
        assert abs(float(v) - want) <= 2e-5 * abs(want) + 1e-30, (r, want)
got = prev + 1
if mode == 'full':
    assert got == n and len(rows) - 1 == n, (got, n)
else:
    assert got <= n
print('csv ok: %d rows (last index %d, %d skipped)' % (len(rows) - 1, prev, skipped))
PY
}
# cutok <img>: fsck may only report what a cut between the FAT and the
# directory leaves (FAT 1 ahead, a lost cluster, a LOG chain longer than its size).
cutok() {
    local rc=0
    fsck.fat -n $1 > $W/fsck.out 2>&1 || rc=$?
    [ $rc -le 1 ] || { cat $W/fsck.out; echo "FAIL: fsck rc=$rc"; exit 1; }   # 1 = findings, checked below
    if grep -vE "^fsck.fat|files, .*clusters|^$|Leaving filesystem unchanged|FATs differ but appear to be intact|Using first FAT|Reclaimed [0-9]+ unused clusters?|^/LOGS/LOG[0-9]{3}\.DLG$|File size is [0-9]+ bytes, cluster chain length is > [0-9]+ bytes|Truncating file to [0-9]+ bytes" $W/fsck.out | grep -q .; then
        cat $W/fsck.out; echo "FAIL: unexpected fsck finding ${CUTK:-}"; exit 1
    fi
}
IMGC=$W/diskc.img
mkimg $IMGC
mcopy -i $IMGC $W/keep.bin ::CONFIG.BIN
"$W/t" $IMGC fatdir boot >/dev/null                  # LOGS/ SHOTS/ SCOPE/ as after a native boot
cp $IMGC $W/csvbase.img
"$W/t" $IMGC csv rec 3000 | tee $W/c1
grep -q "csv=Done file=Some(0) rows=3000 bytes=25523 committed=25523 dropped=0 medium_changed=true result=true" $W/c1 && echo "ok: 3000 rows recorded and committed as LOG000.DLG" || { echo "FAIL at line $LINENO"; exit 1; }
grep -q "LOG:CSV? -> 3,0,3000,25523,25523,0" $W/c1 && echo "ok: LOG:CSV? reply" || { echo "FAIL at line $LINENO"; exit 1; }
grep -q "session_flag=false" $W/c1 && grep -q "data_max_per_block=1 " $W/c1 && echo "ok: host writes allowed again; no data block erased twice (tail rewrites program-only)" || { echo "FAIL at line $LINENO"; exit 1; }
grep -q "erases: fat=14 dir=8 data=7" $W/c1 && echo "ok: 8 commits = 8 LOGS/ block erases, 7 clusters = 2 x 7 FAT erases" || { echo "FAIL at line $LINENO"; exit 1; }
fsckok $IMGC
csvok $IMGC LOGS/LOG000.DLG 3000 full
# The pre-DLG writer's LOG000.CSV of the same 3000 push() calls (76028 bytes,
# commit 5a0dd55's csvlog.rs), with the header naming opendm40 instead of
# fw-rs (+3 bytes) and "empty value = overflow" (was "overload", 2026-10-30):
# the decoded CSV is otherwise byte-identical.
[ "$(md5sum < $W/log.csv | cut -c1-32)" = 8ca36a74fc33171580669fafddda6c88 ] && [ $(stat -c%s $W/log.csv) = 76031 ] && echo "ok: decoded CSV == the old CSV writer's file, byte for byte (76031 bytes from 25523: 3.0x on this worst-case decade sweep)" || { echo "FAIL at line $LINENO"; exit 1; }
[ "$(mdir -i $IMGC ::LOGS | grep LOG000 | awk '{print $3}')" = 25523 ] && echo "ok: directory size 25523" || { echo "FAIL at line $LINENO"; exit 1; }
mcopy -n -i $IMGC ::CONFIG.BIN $W/o && cmp $W/o $W/keep.bin && echo "ok: CONFIG.BIN untouched" || { echo "FAIL at line $LINENO"; exit 1; }
"$W/t" $IMGC csv rec 50 | grep -q "csv=Done file=Some(1) rows=50" && echo "ok: next recording is LOG001.DLG" || { echo "FAIL at line $LINENO"; exit 1; }
fsckok $IMGC
csvok $IMGC LOGS/LOG001.DLG 50 full
csvok $IMGC LOGS/LOG000.DLG 3000 full

# Interval mode (Record page, decision 2026-10-22 #1): one row per interval
# at its end, the mean of its readings, flags ORed, OL rows without a value,
# no row for an interval without readings, the partial last interval at
# stop; the duration ends the recording on its own.
echo "--- interval recording"
IMGI=$W/diski.img
cp $W/csvbase.img $IMGI
"$W/t" $IMGI csv ival 1 0 105 | tee $W/i1
grep -q "ival=Done file=Some(0) rows=10 dropped=0" $W/i1 && echo "ok: 1 s interval, 105 readings at 10/s, 10 with a gap: 10 rows" || { echo "FAIL at line $LINENO"; exit 1; }
fsckok $IMGI
mcopy -n -i $IMGI ::LOGS/LOG000.DLG $W/i.dlg && python3 -I ../dlog.py $W/i.dlg -o $W/i.csv || { echo "FAIL at line $LINENO"; exit 1; }
python3 -I - $W/i.csv <<'PY' || { echo "FAIL at line $LINENO: interval CSV"; exit 1; }
import sys
raw = open(sys.argv[1], 'rb').read().decode()
lines = raw.split('\r\n')
assert lines[2] == '# interval 1 s: one row per interval with readings, value = mean of its readings, time_s = end of the interval; OL = an overflow in it', lines[2]
assert lines[3] == 'time_s,value,unit,flags', lines[3]
rows = [l for l in lines[4:] if l]
want = ['1.000,4.50000,V,', '2.000,14.5000,V,', '3.000,,V,OL', '4.000,,V,OL', '5.000,44.5000,V,REL HOLD HI LO',
        '6.000,54.5000,V,', '8.000,74.5000,V,', '9.000,84.5000,V,', '10.000,94.5000,V,', '11.000,102.000,V,']
assert rows == want, rows
print('interval csv ok: %d rows' % len(rows))
PY
cp $W/csvbase.img $IMGI
"$W/t" $IMGI csv ival 2 1 7000 | tee $W/i2
grep -q "ended by its duration at reading 6000" $W/i2 && grep -q "ival=Done file=Some(0) rows=60 " $W/i2 && echo "ok: 10 s interval, 10 min duration: ends itself at 600 s with 60 rows" || { echo "FAIL at line $LINENO"; exit 1; }
mcopy -n -i $IMGI ::LOGS/LOG000.DLG $W/i.dlg && python3 -I ../dlog.py $W/i.dlg -o $W/i.csv || { echo "FAIL at line $LINENO"; exit 1; }
tail -c 200 $W/i.csv | grep -q "^600.000,5949.50,V," && echo "ok: the last row is 600.000 (mean of readings 5900..5999 = 5949.5)" || { echo "FAIL at line $LINENO"; exit 1; }
cp $W/csvbase.img $IMGI
"$W/t" $IMGI csv ival 1 1 50 | tee $W/i3
grep -q "ival=Done file=Some(0) rows=5 " $W/i3 && grep -qE "end_ms=600[0-9]{3}$" $W/i3 && echo "ok: no reading after 5 s: the duration still ends it at 600 s (poll)" || { echo "FAIL at line $LINENO"; exit 1; }
F=$(grep -o "^free=[0-9]*" $W/i3 | cut -d= -f2)
FS=$(( $(fsck.fat -n $IMGI | grep -o "[0-9]*/[0-9]* clusters" | awk -F'[/ ]' '{print $2 - $1}') * 4096 ))
E1=$(( F / 2 / 3600 > 1193 ? 1193 : F / 2 / 3600 ))
[ "$F" = "$FS" ] && grep -qE "est_1s=$E1 est_every=$(( F / 2 / 36000 )) cap=1193" $W/i3 && echo "ok: free bytes == fsck's free clusters x 4096 ($F); estimate 2 B/row, capped at 1193 h" || { echo "FAIL at line $LINENO"; exit 1; }
"$W/t" $IMGI csv ival 0 0 200 | grep -q "ival=Done file=Some(1) rows=190 " && echo "ok: every-reading mode (index 0): one row per reading" || { echo "FAIL at line $LINENO"; exit 1; }

# On change mode (Record page): a row per event, each at its reading's own
# time: the first reading, a change > N % of full scale, an OL / HI / LO
# change (not REL), a heartbeat 60 s after the last row.
for CI in 0 1 2; do
    cp $W/csvbase.img $IMGI
    "$W/t" $IMGI csv chg $CI | tee $W/e$CI
    grep -q "chg=Done file=Some(0) " $W/e$CI || { echo "FAIL at line $LINENO"; exit 1; }
    fsckok $IMGI
    mcopy -n -i $IMGI ::LOGS/LOG000.DLG $W/e.dlg && python3 -I ../dlog.py $W/e.dlg -o $W/e.csv || { echo "FAIL at line $LINENO"; exit 1; }
    python3 -I - $W/e.csv $CI <<'PY' || { echo "FAIL at line $LINENO: on-change CSV ($CI)"; exit 1; }
import sys
raw = open(sys.argv[1], 'rb').read().decode()
ci = int(sys.argv[2])
lines = raw.split('\r\n')
n = ['0.1', '1', '5'][ci]
assert lines[2] == "# event: change > %s %% of the range's full scale, an OL / HI / LO change, or 60 s since the last row; time_s = time of the reading" % n, lines[2]
assert lines[3] == 'time_s,value,unit,flags', lines[3]
rows = [l for l in lines[4:] if l]
want = ['0.000,1.00000,V,', '2.000,2.00000,V,', '3.000,,V,OL', '3.500,2.00000,V,', '4.000,2.00000,V,HI',
        '4.500,2.00000,V,', '64.500,2.00000,V,', '124.500,2.00000,V,'] + (['130.000,2.05000,V,'] if ci == 0 else []) + ['130.100,2.40000,V,']
assert rows == want, rows
print('on-change csv ok (%s %%): %d rows' % (n, len(rows)))
PY
done
echo "ok: On change 0.1 / 1 / 5 %: rows only at steps, flag changes and 60 s heartbeats, each at its reading's time"

cp $IMGC $W/logs.img                                 # for the USB volume section
mcopy -i $W/logs.img $W/keep.bin ::LOGS/LOG002.DLG     # not a DLG file: left out there

echo "--- interleaved with screenshots"
IMGS=$W/disks.img
cp $W/csvbase.img $IMGS
"$W/t" $IMGS csv rec 3000 shot 10 shotw 1000 shot 2400 | tee $W/c2
grep -qE "csv=Done file=Some\(0\) rows=3000 bytes=([0-9]+) committed=\1 dropped=0" $W/c2 && grep -q "shots=\[Saved(0), Saved(1), Saved(2)\]" $W/c2 && grep -q "lock_ok=true" $W/c2 && echo "ok: three screenshots during the recording, no commit started during a shot" || { echo "FAIL at line $LINENO"; exit 1; }
grep -q "during a commit: true" $W/c2 && grep -qE "shot_waits=[1-9]" $W/c2 && echo "ok: a shot started during a commit waited for it before its FAT scan" || { echo "FAIL at line $LINENO"; exit 1; }
fsckok $IMGS
csvok $IMGS LOGS/LOG000.DLG 3000 full
bmpok $IMGS SHOTS/S000.BMP
bmpok $IMGS SHOTS/S001.BMP
bmpok $IMGS SHOTS/S002.BMP

echo "--- host write in progress"
IMGH=$W/diskh.img
cp $W/csvbase.img $IMGH
"$W/t" $IMGH csv rec 2000 host 300 120 | tee $W/c3
grep -q "csv=Done file=Some(0) rows=2000 .* dropped=0" $W/c3 && grep -q "host_writes=0 " $W/c3 && echo "ok: 12 s host write: no block written meanwhile, nothing dropped" || { echo "FAIL at line $LINENO"; exit 1; }
fsckok $IMGH
csvok $IMGH LOGS/LOG000.DLG 2000 full
cp $W/csvbase.img $IMGH
"$W/t" $IMGH csv rec 2000 host 300 1200 | tee $W/c4
grep -q "csv=Done file=Some(0) .* dropped=[1-9]" $W/c4 && grep -q "host_writes=0 " $W/c4 && echo "ok: 120 s host write (the 8 KiB FIFO holds ~960 rows of compact log here, ~2 min): FIFO overflow, rows dropped and counted, nothing written meanwhile" || { echo "FAIL at line $LINENO"; exit 1; }
fsckok $IMGH
csvok $IMGH LOGS/LOG000.DLG 2000 gap | tee $W/c4p
grep -qE "[1-9][0-9]* skipped" $W/c4p && echo "ok: the row after the gap carries GAP" || { echo "FAIL at line $LINENO"; exit 1; }

echo "--- chain across the FAT 1 block boundary (clusters 2045..2048: link in block 0 written last)"
IMGB=$W/diskb.img
cp $W/csvbase.img $IMGB
head -c $(( (2040 - 24) * 4096 )) /dev/zero > $W/fill      # 24 = the three directories
mcopy -i $IMGB $W/fill ::FILL.DAT
cp $IMGB $W/fatbase.img
"$W/t" $IMGB csv rec 1500 | tee $W/c6
grep -q "csv=Done file=Some(0) rows=1500 " $W/c6 && echo "ok: recorded across the FAT block boundary" || { echo "FAIL at line $LINENO"; exit 1; }
fsckok $IMGB
csvok $IMGB LOGS/LOG000.DLG 1500 full
python3 -I - $IMGB <<'PY' || { echo "FAIL at line $LINENO: chain"; exit 1; }
import struct, sys
d = open(sys.argv[1], 'rb').read()
rsvd, nf, fsz, rootn = struct.unpack_from('<H', d, 0x0E)[0], d[0x10], struct.unpack_from('<H', d, 0x16)[0], struct.unpack_from('<H', d, 0x11)[0]
root = (rsvd + nf * fsz) * 4096
data = root + rootn * 32
fat = lambda c: struct.unpack_from('<H', d, rsvd * 4096 + 2 * c)[0]
def entries(c):
    while c < 0xFFF8:
        for i in range(128):
            yield d[data + (c - 2) * 4096 + 32 * i: data + (c - 2) * 4096 + 32 * i + 32]
        c = fat(c)
logs = [struct.unpack_from('<H', e, 0x1A)[0] for e in (d[root + 32 * i: root + 32 * i + 32] for i in range(rootn)) if e[:11] == b'LOGS       ']
for e in entries(logs[0]):
    if e[:11] == b'LOG000  DLG':
        c, chain = struct.unpack_from('<H', e, 0x1A)[0], []
        while c < 0xFFF8:
            chain.append(c); c = struct.unpack_from('<H', d, rsvd * 4096 + 2 * c)[0]
        print('chain', chain)
        assert chain[0] < 2048 <= chain[-1], chain
        break
else:
    sys.exit(1)
PY

# cutsweep <base> <rows>: power cut before every block write (and a
# program-only write cut half way) of a recording on a copy of <base>.
cutsweep() {
    cp $1 $W/cut0.img
    NW=$("$W/t" $W/cut0.img csv rec $2 | grep -oE "^  writes=[0-9]+" | cut -d= -f2)
    echo "uncut: $NW block writes"
    prevc=0
    for k in $(seq 0 $NW); do
        CUTK="(cut $k)"
        cp $1 $W/cut.img
        "$W/t" $W/cut.img csv rec $2 cut $k > $W/cr
        cutok $W/cut.img
        if mdir -i $W/cut.img ::LOGS | grep -q LOG000; then
            csvok $W/cut.img LOGS/LOG000.DLG $2 prefix > $W/cp || { cat $W/cp; echo "FAIL: cut $k"; exit 1; }
            c=$(mdir -i $W/cut.img ::LOGS | grep LOG000 | awk '{print $3}')
        else
            c=0
        fi
        [ $c -ge $prevc ] || { echo "FAIL: cut $k: committed size went down"; exit 1; }
        prevc=$c
        mcopy -n -i $W/cut.img ::CONFIG.BIN $W/o && cmp -s $W/o $W/keep.bin || { echo "FAIL: cut $k: CONFIG.BIN"; exit 1; }
        cp $1 $W/cut.img
        "$W/t" $W/cut.img csv rec $2 cut $k half > $W/cr
        cutok $W/cut.img
        if mdir -i $W/cut.img ::LOGS | grep -q LOG000; then
            csvok $W/cut.img LOGS/LOG000.DLG $2 prefix > $W/cp || { cat $W/cp; echo "FAIL: cut $k half"; exit 1; }
        fi
    done
    CUTK=
    echo "ok: $((NW + 1)) cut points x 2: fsck only lost / excess clusters or FAT 1 ahead, committed CSV always a clean prefix, never shrinking"
}
echo "--- power cut before every block write"
cutsweep $W/csvbase.img 1500
echo "--- power cut before every block write, chain across the FAT block boundary"
cutsweep $W/fatbase.img 1500

echo "--- nearly full disk"
IMGF=$W/diskf.img
cp $W/csvbase.img $IMGF
FREE=$(fsck.fat -n $IMGF | grep -o "[0-9]*/[0-9]* clusters" | awk -F'[/ ]' '{print $2 - $1}')
head -c $(( (FREE - 5) * 4096 )) /dev/zero > $W/fill
mcopy -i $IMGF $W/fill ::FILL.DAT
"$W/t" $IMGF csv rec 3000 | tee $W/c5
grep -q "csv=Full file=Some(0)" $W/c5 && echo "ok: disk full ends the recording (Full)" || { echo "FAIL at line $LINENO"; exit 1; }
fsckok $IMGF
csvok $IMGF LOGS/LOG000.DLG 3000 prefix
SZ=$(mdir -i $IMGF ::LOGS | grep LOG000 | awk '{print $3}')
[ $SZ = 20480 ] && echo "ok: the 5 free clusters filled ($SZ bytes); the chunk cut at the end is a torn tail, skipped by the reader" || { echo "FAIL at line $LINENO"; exit 1; }

echo "--- gallery"
mcopy -i $IMGS $W/keep.bin "::Log_with_a_long_name.dlg"
mcopy -i $IMGS $W/keep.bin ::LOG007.CSV               # text CSV: not a gallery file any more
mdir -i $IMGS ::
"$W/t" $IMGS csv list | tee $W/g1
grep -q "list n=5 total=5" $W/g1 && grep -qE "  LOG000.DLG 255[0-9][0-9] Csv" $W/g1 && grep -q "  S001.BMP 307266 Bmp" $W/g1 && grep -q "  LOG_WI~1.DLG 9000 Csv" $W/g1 && ! grep -q "LOG007" $W/g1 && grep -q "changed blocks: \[\]" $W/g1 && echo "ok: list: 3 BMP + 2 DLG with sizes, CONFIG.BIN and a text LOG007.CSV not listed, nothing written" || { echo "FAIL at line $LINENO"; exit 1; }
F0=$(grep -o "free=[0-9]*" $W/g1 | cut -d= -f2)
L0=$(grep "  LOG000.DLG" $W/g1 | awk '{print $2}')
"$W/t" $IMGS csv cat | tee $W/g0
grep -qx "cat=$((16740352 - F0)),$F0,\"LOG_WI~1.DLG,BIN,9000\",\"LOGS/LOG000.DLG,BIN,$L0\",\"SHOTS/S000.BMP,BIN,307266\",\"SHOTS/S001.BMP,BIN,307266\",\"SHOTS/S002.BMP,BIN,307266\"" $W/g0 && echo "ok: MMEM:CAT? text (root legacy file, then LOGS/, SHOTS/ with their directory)" || { echo "FAIL at line $LINENO"; exit 1; }
[ $F0 = $(( $(fsck.fat -n $IMGS | grep -o "[0-9]*/[0-9]* clusters" | awk -F'[/ ]' '{print $2 - $1}') * 4096 )) ] && echo "ok: free bytes = fsck free clusters" || { echo "FAIL at line $LINENO"; exit 1; }
"$W/t" $IMGS csv del LOG000.DLG | tee $W/g2
grep -q "del=Ok(())" $W/g2 && grep -q "free=$((F0 + 7 * 4096)) " $W/g2 && echo "ok: LOG000.DLG deleted, its 7 clusters freed" || { echo "FAIL at line $LINENO"; exit 1; }
fsckok $IMGS
"$W/t" $IMGS csv del log_wi~1.dlg | grep -q "del=Ok(())" && echo "ok: long-named file deleted" || { echo "FAIL at line $LINENO"; exit 1; }
fsckok $IMGS
mdir -i $IMGS :: | grep -qi "long_name" && { echo "FAIL at line $LINENO: long name left"; exit 1; }
"$W/t" $IMGS csv del shots/s001.bmp | grep -q "del=Ok(())" && echo "ok: SHOTS/S001.BMP deleted (path form)" || { echo "FAIL at line $LINENO"; exit 1; }
"$W/t" $IMGS csv del S001.BMP | grep -q "del=Err(NotFound)" && echo "ok: second delete = NotFound" || { echo "FAIL at line $LINENO"; exit 1; }
"$W/t" $IMGS csv del CONFIG.BIN | grep -q "del=Err(NotFound)" && echo "ok: CONFIG.BIN is not a gallery file" || { echo "FAIL at line $LINENO"; exit 1; }
"$W/t" $IMGS csv del LOG007.CSV | grep -q "del=Err(NotFound)" && echo "ok: a text LOG007.CSV is not a gallery file" || { echo "FAIL at line $LINENO"; exit 1; }
fsckok $IMGS
bmpok $IMGS SHOTS/S000.BMP
bmpok $IMGS SHOTS/S002.BMP
mcopy -n -i $IMGS ::CONFIG.BIN $W/o && cmp $W/o $W/keep.bin && echo "ok: CONFIG.BIN untouched" || { echo "FAIL at line $LINENO"; exit 1; }
"$W/t" $IMGS csv rec 1200 del 300 S002.BMP | tee $W/g4
grep -q "gallery delete at row 3[0-9][0-9]: Ok(())" $W/g4 && grep -q "csv=Done file=Some(0) rows=1200 " $W/g4 && echo "ok: S002.BMP deleted from the gallery between two commits of a recording" || { echo "FAIL at line $LINENO"; exit 1; }
fsckok $IMGS
csvok $IMGS LOGS/LOG000.DLG 1200 full
mdir -i $IMGS ::SHOTS | grep -q "^S002" && { echo "FAIL at line $LINENO"; exit 1; }
bmpok $IMGS SHOTS/S000.BMP
mdel -i $IMGS ::LOGS/LOG000.DLG
"$W/t" $IMGS csv busydel | tee $W/g3
grep -q "busydel LOG000.DLG -> Err(Busy)" $W/g3 && grep -q "then del -> Ok(())" $W/g3 && echo "ok: the file being recorded cannot be deleted; after stop it can" || { echo "FAIL at line $LINENO"; exit 1; }
fsckok $IMGS
mdir -i $IMGS ::

echo "=== the USB volume (vdisk.rs) over a SPI disk: LOGS / SCOPE / SHOTS / DEBUG, refreshes"
# SPI disk: LOGS/LOG000.DLG (3000 rows), LOG001.DLG (50 rows), a foreign
# LOGS/LOG002.DLG; SHOTS/S000..S002.BMP, SCOPE/SCOP000/001.DLG (the
# screenshot section's); earlier builds' root files LOG005.DLG (= LOG001's
# bytes) and S007.BMP; SNAP.BMP (no number: not shown).
VS=$W/vs1.img
cp $W/logs.img $VS
for f in S000 S001 S002; do mcopy -n -i $IMG7 ::SHOTS/$f.BMP $W/$f.bmp; mcopy -i $VS $W/$f.bmp ::SHOTS/$f.BMP; done
mcopy -i $VS $W/s.dlg ::SCOPE/SCOP000.DLG
mcopy -i $VS $W/s1.dlg ::SCOPE/SCOP001.DLG
mcopy -n -i $VS ::LOGS/LOG001.DLG $W/l1.dlg
mcopy -i $VS $W/l1.dlg ::LOG005.DLG
mcopy -i $VS $W/S001.bmp ::S007.BMP
mcopy -i $VS $W/S002.bmp ::SNAP.BMP
fsckok $VS
cp $VS $W/vs2.img
mcopy -i $W/vs2.img $W/l1.dlg ::LOGS/LOG003.DLG        # 2: a new log
cp $W/vs2.img $W/vs3.img
mdel -i $W/vs3.img ::LOGS/LOG001.DLG                  # 3: one deleted
vdisk $W/f1m_native $VD spi $VS spi $W/vs2.img spi $W/vs3.img locked reset free forget LOG000.DLG
grep -A1 "^step 1: " $W/vd.out | tr '\n' ' ' | grep -qE "files=9 .*media_changed=true .*again: files=9 media_changed=false" && echo "ok: step 1: 9 files (2 + LOG005 logs, 2 scope, 3 + S007 shots), media change once" || { echo "FAIL at line $LINENO"; exit 1; }
for i in 1 2 3 4 5 6 7; do fsckok $VD.$i >/dev/null || { echo "FAIL: fsck $VD.$i"; exit 1; }; done
echo "ok: fsck clean after every step"
# volok <vol> <spi>: every generated file equals its source (CSV = dlog.py
# of the DLG, BMP = the SPI file), SPIFLASH.BIN = the SPI image (fsck clean).
volok() {
    for n in $(mdir -b -i $1 ::LOGS ::SCOPE | grep -oE "(LOG|SCOP)[0-9]{3}\.CSV"); do
        b=${n%.CSV}
        case $b in LOG00[5]) src=::$b.DLG ;; LOG*) src=::LOGS/$b.DLG ;; *) src=::SCOPE/$b.DLG ;; esac
        d=LOGS; case $b in SCOP*) d=SCOPE ;; esac
        mcopy -n -i $2 $src $W/x.dlg && python3 -I ../dlog.py $W/x.dlg -o $W/x.csv && mcopy -n -i $1 ::$d/$n - | cmp -s - $W/x.csv || { echo "FAIL: $d/$n"; exit 1; }
    done
    for n in $(mdir -b -i $1 ::SHOTS | grep -oE "S[0-9]{3}\.BMP"); do
        src=::SHOTS/$n; [ $n = S007.BMP ] && src=::S007.BMP
        mcopy -n -i $2 $src - | cmp -s - <(mcopy -n -i $1 ::SHOTS/$n -) || { echo "FAIL: SHOTS/$n"; exit 1; }
    done
    mcopy -n -i $1 ::DEBUG/SPIFLASH.BIN $W/spi.bin && cmp $W/spi.bin $2 || { echo "FAIL: SPIFLASH.BIN"; exit 1; }
    fsck.fat -n $W/spi.bin >/dev/null || { echo "FAIL: fsck SPIFLASH.BIN"; exit 1; }
}
volok $VD.1 $VS && echo "ok: step 1: every CSV == dlog.py of its DLG (LOG005 from the root), BMPs == the SPI files (S007 from the root), DEBUG/SPIFLASH.BIN == the SPI image (fsck clean)" || exit 1
mdir -b -i $VD.1 ::LOGS ::SHOTS ::SCOPE | grep -qE "LOG002|SNAP" && { echo "FAIL at line $LINENO: foreign / unnumbered file shown"; exit 1; }
echo "ok: the foreign LOG002.DLG and the unnumbered SNAP.BMP are not shown"
[ "$(mcopy -n -i $VD.1 ::LOGS/LOG000.CSV - | md5sum | cut -c1-32)" = 8ca36a74fc33171580669fafddda6c88 ] && [ "$(mcopy -n -i $VD.1 ::SCOPE/SCOP000.CSV - | md5sum | cut -c1-32)" = 4921f54425a8f14d910a25e65a4fc3c6 ] && echo "ok: LOGS/LOG000.CSV and SCOPE/SCOP000.CSV == the old CSV writer's files" || { echo "FAIL at line $LINENO"; exit 1; }
mtype -i $VD.1 ::README.TXT | grep -q "docs/manual" && echo "ok: README.TXT" || { echo "FAIL at line $LINENO"; exit 1; }
# places <a> <b> <rule>: directory slot + first cluster of every LOGS /
# SCOPE / SHOTS file in two dumps (independent parse).
places() {
    python3 -I - "$@" <<'PY'
import struct, sys
def scan(path):
    f = open(path, 'rb')
    bs = f.read(512)
    bps, spc, rsvd, nf, rootn, tot16, media, fsz = struct.unpack_from('<HBHBHHBH', bs, 0x0B)
    root = (rsvd + nf * fsz) * bps
    data = root + rootn * 32
    f.seek(root); r = f.read(rootn * 32)
    out, deleted = {}, {}
    for i in range(rootn):
        e = r[32 * i:32 * i + 32]
        if e[0] == 0: break
        if e[11] & 0x10 and e[:11].strip() in (b'LOGS', b'SCOPE', b'SHOTS'):
            c = struct.unpack_from('<H', e, 26)[0]
            f.seek(data + (c - 2) * spc * bps); d = f.read(spc * bps)
            deleted[e[:11].strip().decode()] = []
            for k in range(len(d) // 32):
                x = d[32 * k:32 * k + 32]
                if x[0] == 0: break
                if x[0] == 0xE5: deleted[e[:11].strip().decode()].append(k); continue
                if x[11] & 0x10: continue
                n = x[:8].strip().decode() + '.' + x[8:11].strip().decode()
                out[n] = (e[:11].strip().decode(), k, struct.unpack_from('<H', x, 26)[0])
    return out, deleted
a, da = scan(sys.argv[1]); b, db = scan(sys.argv[2]); rule = sys.argv[3]
common = set(a) & set(b)
if rule == 'kept':
    assert all(a[n] == b[n] for n in common), [(n, a[n], b[n]) for n in common if a[n] != b[n]]
    hw = max(v[2] for v in a.values())
    for n in set(b) - set(a):
        assert b[n][2] > hw and b[n][1] == max(v[1] for v in a.values() if v[0] == b[n][0]) + 1, (n, b[n])
    for n in set(a) - set(b):
        assert a[n][1] in db[a[n][0]], (n, 'slot not shown deleted')
    print('kept: %d files at the same slot / cluster, %d new after the high-water mark, %d gone (slot deleted)' % (len(common), len(set(b) - set(a)), len(set(a) - set(b))))
elif rule == 'compact':
    assert set(a) == set(b) and not any(db.values())
    for d in ('LOGS', 'SCOPE', 'SHOTS'):
        assert sorted(v[1] for v in b.values() if v[0] == d) == list(range(2, 2 + sum(1 for v in b.values() if v[0] == d))), d
    print('compacted: %d files, slots 2.. without holes, no deleted entries' % len(b))
PY
}
grep -q "^step 2: .*files=10 .*media_changed=true" $W/vd.out && places $VD.1 $VD.2 kept && echo "ok: step 2: the new LOG003 appended, everything else in place" || { echo "FAIL at line $LINENO"; exit 1; }
grep -q "^step 3: .*files=9 .*media_changed=true" $W/vd.out && places $VD.2 $VD.3 kept && echo "ok: step 3: LOG001 gone, its slot shown deleted, the rest in place" || { echo "FAIL at line $LINENO"; exit 1; }
volok $VD.3 $W/vs3.img || exit 1
grep -q "^step 4: locked .*media_changed=false" $W/vd.out && grep -q "^step 5: reset .*media_changed=false" $W/vd.out && places $VD.3 $VD.5 kept >/dev/null && echo "ok: steps 4-5: a host holds the lock: a bus reset changes nothing yet" || { echo "FAIL at line $LINENO"; exit 1; }
grep -q "^step 6: free .*media_changed=true" $W/vd.out && places $VD.5 $VD.6 compact && echo "ok: step 6: unlocked: compacted, media change" || { echo "FAIL at line $LINENO"; exit 1; }
volok $VD.6 $W/vs3.img || exit 1
grep -q "^step 7: forget LOG000.DLG media_changed=true" $W/vd.out && ! mdir -b -i $VD.7 ::LOGS | grep -q LOG000 && echo "ok: step 7: forget (device delete): gone at once, media change" || { echo "FAIL at line $LINENO"; exit 1; }

echo "=== fastlog: the fast logger to the SPI disk (src/fastlog.rs, LOGS/FASTnnn.DLG, kind 3)"
# fastok <img> <file>: the DLG decodes (dlog.py) to exactly <img>.ref.csv
# (the records the session drained); `prefix`: a prefix of it.
fastok() {
    mcopy -n -i $1 ::LOGS/$2 $W/fx.dlg || { echo "FAIL: no LOGS/$2"; exit 1; }
    # A cut between the entry and the header's first page program (one
    # main-loop pass after the logger starts): the file is all 0xFF, not a
    # DLG (vdisk / MMEM:DATA? leave it out; it can be deleted).
    if [ "$3" = prefix ] && ! tr -d '\377' < $W/fx.dlg | head -c1 | grep -q .; then
        nblank=$((nblank + 1))
        return 0
    fi
    python3 -I ../dlog.py $W/fx.dlg -o $W/fx.csv 2>$W/fx.err || { cat $W/fx.err; echo "FAIL: dlog.py $2"; exit 1; }
    if [ "$3" = prefix ]; then
        python3 -I - $W/fx.csv $1.ref.csv <<'PY' || { echo "FAIL: $2 not a prefix of the reference ${CUTK:-}"; exit 1; }
import sys
a, b = open(sys.argv[1], 'rb').read(), open(sys.argv[2], 'rb').read()
sys.exit(0 if b.startswith(a) and a.count(b'\n') >= 2 else 1)
PY
    else
        cmp -s $W/fx.csv $1.ref.csv || { echo "FAIL: $2 CSV != reference"; exit 1; }
    fi
}
# fastcut <img>: fsck findings a cut may leave (lost clusters, FAT copies
# differing, a chain longer than the file), nothing else.
fastcut() {
    local rc=0
    fsck.fat -n $1 > $W/fsck.out 2>&1 || rc=$?
    [ $rc -le 1 ] || { cat $W/fsck.out; echo "FAIL: fsck rc=$rc ${CUTK:-}"; exit 1; }
    if grep -vE "^fsck.fat|files, .*clusters|^$|Leaving filesystem unchanged|FATs differ but appear to be intact|Using first FAT|Reclaimed [0-9]+ unused clusters? in [0-9]+ chains?\.?|Reclaimed [0-9]+ unused clusters?|^/LOGS/FAST[0-9]{3}\.DLG$|File size is [0-9]+ bytes, cluster chain length is > [0-9]+ bytes|Truncating file to [0-9]+ bytes" $W/fsck.out | grep -q .; then
        cat $W/fsck.out; echo "FAIL: unexpected fsck finding ${CUTK:-}"; exit 1
    fi
}
IMGQ=$W/diskq.img
mkimg $IMGQ
mcopy -i $IMGQ $W/keep.bin ::CONFIG.BIN
"$W/t" $IMGQ fatdir boot >/dev/null
cp $IMGQ $W/fastbase.img
"$W/t" $IMGQ fast 40000 9600 | tee $W/q1
grep -qE "^fast=Done file=Some\(0\) .* rows=40000 drained=40000 ring_drops=0 " $W/q1 && echo "ok: 40000 AD2 samples at 9600 S/s -> LOGS/FAST000.DLG" || { echo "FAIL at line $LINENO"; exit 1; }
fsckok $IMGQ
fastok $IMGQ FAST000.DLG
cp $IMGQ.ref.csv $W/q1ref.csv
B=$(grep -oE " bytes=[0-9]+" $W/q1 | cut -d= -f2)
[ "$(mdir -i $IMGQ ::LOGS | grep FAST000 | awk '{print $3}')" = $B ] && echo "ok: directory size = the bytes written ($B), the surplus clusters freed (fsck clean)" || { echo "FAIL at line $LINENO"; exit 1; }
mcopy -n -i $IMGQ ::CONFIG.BIN $W/o && cmp $W/o $W/keep.bin && echo "ok: CONFIG.BIN untouched" || { echo "FAIL at line $LINENO"; exit 1; }
"$W/t" $IMGQ fast 20000 4800 ringdrop | tee $W/q2
grep -qE "^fast=Done file=Some\(1\) " $W/q2 && ! grep -q "ring_drops=0 " $W/q2 && echo "ok: FAST001.DLG with ring-overflow holes" || { echo "FAIL at line $LINENO"; exit 1; }
fsckok $IMGQ
fastok $IMGQ FAST001.DLG
python3 -I - $W/fx.csv <<'PY' && echo "ok: the holes show as lost_before > 0 rows" || { echo "FAIL at line $LINENO"; exit 1; }
import sys
rows = [l.split(',') for l in open(sys.argv[1]).read().splitlines() if l[:1] in ' 0123456789']
lost = [int(r[-1]) for r in rows]
assert sum(1 for x in lost if x > 0) >= 1 and sum(lost) > 1000, (sum(lost))
PY
# The USB volume (vdisk.rs): LOGS/FAST000.CSV / FAST001.CSV next to the LOGs.
"$W/t" $W/vq.img vdisk $W/f1m_native $W/src/UID.BIN $W/src/OTP.BIN $W/src/OPTBYTES.BIN spi $IMGQ > $W/vq.out
mtype -i $W/vq.img.1 ::LOGS/FAST000.CSV > $W/vq.csv && cmp $W/vq.csv $W/q1ref.csv && mdir -i $W/vq.img.1 ::LOGS | grep -q "^FAST001 *CSV" && echo "ok: the USB volume serves LOGS/FAST000.CSV (== the reference, $(stat -c%s $W/vq.csv) bytes) and FAST001.CSV" || { cat $W/vq.out; mdir -i $W/vq.img.1 ::LOGS; echo "FAIL at line $LINENO"; exit 1; }
# A large fast log (1.1 M AD2 samples at 38 400 S/s: ~2.9 MB DLG, a ~50 MB
# CSV): the USB volume's LOGS/FAST000.CSV read sequentially through
# read_block == the reference; the raw SPI reads behind it (vread's
# numbers: SPI bytes per CSV byte, the device estimate of the SPI time).
IMGL=$W/diskl.img
cp $W/fastbase.img $IMGL
"$W/t" $IMGL fast 1100000 38400 | tee $W/ql
grep -qE "^fast=Done file=Some\(0\) .* rows=1100000 drained=1100000 ring_drops=0 " $W/ql && echo "ok: 1.1 M samples at 38400 S/s -> LOGS/FAST000.DLG" || { echo "FAIL at line $LINENO"; exit 1; }
"$W/t" $W/vl.csv vread $W/f1m_native $W/src/UID.BIN $W/src/OTP.BIN $W/src/OPTBYTES.BIN $IMGL LOGS FAST000.CSV | tee $W/vl.out
cmp -s $W/vl.csv $IMGL.ref.csv && echo "ok: the 50 MB-class FAST000.CSV over the volume == the reference ($(stat -c%s $W/vl.csv) bytes)" || { echo "FAIL at line $LINENO"; exit 1; }
python3 -I - $W/vl.out <<'PY' && echo "ok: CSV read amplification and refresh cost bounded" || { echo "FAIL at line $LINENO"; exit 1; }
import re, sys
t = open(sys.argv[1]).read()
amp = float(re.search(r'\(([0-9.]+) B per CSV byte', t).group(1))
kbs = float(re.search(r'= ([0-9]+) KB/s SPI-bound', t).group(1))
rs = float(re.search(r'refresh: .*?~([0-9.]+) s on the device', t).group(1))
# The DLG is ~1/22 of the CSV: each DLG byte read about once (window
# refills, chunk headers, FAT; 2c-era: 0.095 B and 171 k reads, a CRC scan
# of the whole file at refresh and at the open, ~1.5 s each on the device,
# and a walk of the FAT chain from the start at every chunk); no full-file
# scan at refresh / open (headers + the last chunks' CRCs).
reads = int(re.search(r'read: spi ([0-9]+) reads', t).group(1))
assert amp < 0.06 and kbs > 10000 and rs < 0.2 and reads < 20000, (amp, kbs, rs, reads)
PY
# The same while "an indexer" re-reads the file's head every 16 sectors
# (device 7674c85: ~2.2 SPI bytes per CSV byte, ~6 ms per sector, with one
# cursor dragged back and forth): two cursors + seek checkpoints.
"$W/t" $W/vlh.csv vread $W/f1m_native $W/src/UID.BIN $W/src/OTP.BIN $W/src/OPTBYTES.BIN $IMGL LOGS FAST000.CSV head | tee $W/vlh.out
cmp -s $W/vlh.csv $IMGL.ref.csv && python3 -I - $W/vlh.out <<'PY' && echo "ok: interleaved head reads: the copy == the reference, SPI reads bounded" || { echo "FAIL at line $LINENO"; exit 1; }
import re, sys
t = open(sys.argv[1]).read()
amp = float(re.search(r'\(([0-9.]+) B per CSV byte', t).group(1))
assert amp < 0.08, amp
PY
rm -f $W/vl.csv $W/vlh.csv $IMGL.ref.csv              # 3 x 49 MB (/tmp quota, see save() in main.rs)
"$W/t" $IMGQ fast 5000 4800 abandon | tee $W/q3
grep -qE "^fast=Done file=None .* started=false" $W/q3 && ! mdir -i $IMGQ ::LOGS | grep -q FAST002 && echo "ok: stopped while preparing: no file, nothing linked" || { echo "FAIL at line $LINENO"; exit 1; }
fsckok $IMGQ
# disk nearly full: the allocation is the free space; the session ends Full
IMGR=$W/diskr.img
cp $W/fastbase.img $IMGR
FREE=$(fsck.fat -n $IMGR | grep -o "[0-9]*/[0-9]* clusters" | awk -F'[/ ]' '{print $2 - $1}')
head -c $(( (FREE - 12) * 4096 )) /dev/zero > $W/fill
mcopy -i $IMGR $W/fill ::FILL.DAT
"$W/t" $IMGR fast 100000 9600 | tee $W/q4
grep -qE "^fast=Full file=Some\(0\) alloc=49152 " $W/q4 && echo "ok: 12 free clusters: allocated all, the session ended Full" || { echo "FAIL at line $LINENO"; exit 1; }
fsckok $IMGR
fastok $IMGR FAST000.DLG prefix
echo "ok: the full file decodes to a prefix of the samples"
# power cut before every operation (block write / page program / erase)
cp $W/fastbase.img $W/fcut0.img
NOPS=$("$W/t" $W/fcut0.img fast 6000 4800 | grep -oE " ops=[0-9]+" | cut -d= -f2)
cp $W/fcut0.img.ref.csv $W/fref.csv
echo "uncut: $NOPS operations"
nfile=0
nblank=0
for k in $(seq 0 $NOPS); do
    CUTK="(cut $k)"
    cp $W/fastbase.img $W/fcut.img
    "$W/t" $W/fcut.img fast 6000 4800 cut $k > $W/fr
    cp $W/fref.csv $W/fcut.img.ref.csv
    fastcut $W/fcut.img
    if mdir -i $W/fcut.img ::LOGS 2>/dev/null | grep -q FAST000; then
        fastok $W/fcut.img FAST000.DLG prefix
        nfile=$((nfile + 1))
    fi
    mcopy -n -i $W/fcut.img ::CONFIG.BIN $W/o && cmp -s $W/o $W/keep.bin || { echo "FAIL: cut $k: CONFIG.BIN"; exit 1; }
done
CUTK=
[ $nblank -le 1 ] || { echo "FAIL: $nblank cuts left an all-0xFF FAST000.DLG (expected: the one before the header page)"; exit 1; }
echo "ok: $((NOPS + 1)) cut points: fsck only lost clusters / FAT 1 ahead / a chain longer than the file; FAST000.DLG ($nfile cuts) decodes to a prefix of the samples ($nblank: all 0xFF, the cut right after the entry)"

echo "=== setfile: the settings' copy in _SETTING.BIN (setfile.rs, persist.rs)"
# Fresh volume with the stock's CONFIG.BIN and a backup next to it: the file
# made once (8 KiB, 2 one-block clusters), then saves rewrite only its two
# blocks; boots after a power-off / with a valid backup SRAM / a newer file;
# CRC failure, a torn and an erased slot, both bad, a v9 slot, an unknown
# version (main.rs setfile_cmd). fsck clean, CONFIG.BIN untouched.
mkimg $W/sf.img
mcopy -i $W/sf.img $W/keep.bin ::CONFIG.BIN
mcopy -i $W/sf.img $W/old.bin ::_DM40PRV.BIN
"$W/t" $W/sf.img setfile all | tee $W/sfr
grep -q FAIL $W/sfr && { echo "FAIL: setfile"; exit 1; }
fsck.fat -n $W/sf.img >/dev/null || { echo "FAIL: setfile: fsck"; exit 1; }
mdir -i $W/sf.img :: | grep -qE "^_SETTING BIN +8192" || { echo "FAIL: setfile: _SETTING.BIN not 8192 bytes"; mdir -i $W/sf.img ::; exit 1; }
mcopy -n -i $W/sf.img ::CONFIG.BIN $W/o && cmp -s $W/o $W/keep.bin || { echo "FAIL: setfile: CONFIG.BIN changed"; exit 1; }
mkimg $W/sf2.img
head -c 100 /dev/urandom > $W/sf100.bin
mcopy -i $W/sf2.img $W/sf100.bin ::_SETTING.BIN
"$W/t" $W/sf2.img setfile wrongsize | tee $W/sfr
grep -q FAIL $W/sfr && { echo "FAIL: setfile wrongsize"; exit 1; }
fsck.fat -n $W/sf2.img >/dev/null || { echo "FAIL: setfile wrongsize: fsck"; exit 1; }
mdir -i $W/sf2.img :: | grep -qE "^_SETTING BIN +8192" || { echo "FAIL: setfile wrongsize: size"; exit 1; }
echo "ok: setfile"

echo "=== vwrite: host writes on the USB volume (delete from the PC, drag-and-drop firmware update)"
# vwrite_test.py: mtools-made sector diffs written through vserve (main.rs)
# in several orders, fsck + file checks of the SPI disk after each scenario,
# power cuts in the firmware staging / commit, and the captured kernel
# sequences in captures/ (usbip_msc.py --capture; vreplay.py).
CAPS=$(ls captures/*.vcap.gz 2>/dev/null || true)
python3 -I vwrite_test.py "$W/t" $W/f1m_native $W/f1m_none $W/src $VS $NATIVE $W/a.bin $W/vw $CAPS || { echo "FAIL: vwrite"; exit 1; }
echo PASS
