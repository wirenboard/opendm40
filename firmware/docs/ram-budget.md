# RAM budget: who is live when

The big RAM users of opendm40, when each is live, what else is live then,
and which can be overlaid or shrunk; plus the two designs that removed the
screenshot buffer (streaming from the panel, idle-time pre-erase) and the
shared disk buffer arena. The memory map, the regions and the link checks
are in [native-boot.md, Memory budget](native-boot.md#memory-budget-2026-10-22-rework).

## The screenshot: streamed from the panel

There is no frame buffer. The image is read back from the ILI9488's GRAM
(RAMRD 0x2E, `lcd::read_span`: a dummy word, then RGB666 packed two pixels
per three words, converted to RGB565) while the file is written
([`src/screenshot.rs`](../src/screenshot.rs)). An earlier 48 KiB SRAM buffer
(`screenshot::CAP`, a run-length coded copy of the rows so that the screen
could unfreeze after ~0.1 s) was removed.

- Data phase, per 4 KiB block: fill it with the header bytes and the
  pixels read from the panel (`read_span`, ~2 000 pixels, ~1.3 ms), into
  `diskbuf::blocks()[0]`, then `write_step` it (compare, erase, 16 page programs,
  read back), in slices of <= ~2 ms of CPU per main-loop pass.
- Consistent frame: `frozen()` is true from the gesture until the last data
  block has been filled. [`main.rs`](../src/main.rs) passes `can_draw = false`
  and `App::gesture` drops input in that time, so nothing draws over rows
  that are not read yet. The main loop keeps running: DMM, USB, BT and the
  watchdog. The verify pass compares CRCs and does not need the panel.
  `App::toast`, the auto power-off warning and the USB firmware-file dialog
  are held off while `frozen()` (a toast is dropped, the dialog comes after
  the shot). The power-off / reset paths (`finish_disk_writes`) draw nothing
  before they finish the shot.
- Cost: the screen stays frozen while the 76 data blocks are written: ~1.4 s
  when the next section's preparation is done, ~5 s when every block needs a
  4 KiB sector erase (~45 ms typ.) first (emulator, W25Q128 timing).
- Disk held: a fast-log session or an upload holds the disk for an
  unbounded time, so a shot is refused at once with the toast "Disk busy"
  (`Shot::Busy`). A csvlog commit (a few blocks) is waited for, frozen.

Why not a smaller buffer: the RLE size of the 148 host renders of
[`tools/ui-render`](../tools/ui-render) (plain RLE / with a "row = previous
row" token; the anti-aliased fonts and traces make rows differ):

| buffer size | renders that fit (plain RLE) | with a row-repeat token |
|---|---|---|
| 8 KiB | 0 / 148 | 2 |
| 12 KiB | 4 | 5 |
| 16 KiB | 9 | 9 |
| 24 KiB | 24 | 30 |
| 32 KiB | 75 | 94 |
| 48 KiB | 134 | 135 |

The median is ~30 KiB and the largest is 68 KiB (d02-dmm-acdc): a small
buffer buys nearly nothing.

Re-rendering in strips for a shot is not possible: the screen is the
result of many diff draws (scope_ui `d_*`, the page caches), not of one
page draw. (A full page repaint is drawn in strips, see
[the third tenant of the arena](#the-disk-buffer-arena-diskbufarena-12-kib).)

## The screenshot freeze: next shot's clusters erased in idle time

Design (`screenshot.rs` "idle-time preparation"):

- **Nothing reserved.** A shot allocates first fit from FAT 1, so the
  preparation keeps the *first 76 free clusters* blank (0xFF). They stay
  ordinary free clusters: no FAT entry, no directory entry, no hidden file.
  A power cut mid-prepare cannot leave lost clusters or a FAT inconsistency
  (at worst a half-erased free sector), the USB volume shows nothing new
  (only the raw `DEBUG/SPIFLASH.BIN` shows them, as 0xFF), and any other
  writer (csvlog commit, fast-log Prep, a firmware file copied in "PC can
  edit" mode) may take them and is just faster for it. Flash wear is
  unchanged: each cluster is still erased once per reuse, only earlier.
- **Steps** (main loop, `prep_poll`, inline in region B; the step itself,
  `prep_step`, in region C): boot sector; FAT 1 entries 64 per read up to
  the next free cluster; that cluster's block checked blank 1 KiB per step
  (~0.7 ms); a dirty block erased (SE4K start, the cluster's FAT entry
  re-read in the same step first), checked again once the chip is idle (a
  block that stays dirty is skipped). Never waits for BUSY (one RDSR, then
  the next pass). Ready after 76 blank clusters, or all free ones on a
  nearly full disk.
- **Restart.** `msc::FLASH_WRITES` counts every program / erase command
  (`spiflash.rs` `write_enable`). When it moved since the preparation's last
  step, another writer touched the disk: the preparation starts over once
  the disk has been quiet for 2 s (a full re-check is ~76 x 4 KiB of reads,
  ~0.2 s of SPI in ~1 ms steps). `Off` (no disk, a read error, an erase
  timeout) retries after 60 s.
- **When it runs** (`main.rs`): vdisk builds only (a host that writes the
  raw SPI disk could be filling a free cluster); not during a shot, a
  csvlog commit, a fast-log session, an upload or a PC staging (those keep
  data in free clusters until they link it), a logger session (constant
  interval; region C code must not run under the AD2 stream ISR), nor within
  0.5 s of host USB disk I/O. SCPI `MMEM:DATA?` / the on-device gallery
  reading through an erase in flight wait for it (<= 45 ms typ., once).
- **The shot.** At the end of Alloc, if the preparation is Ready and
  nothing has written since (`prep_blank`), the shot's first clusters are
  exactly the proven-blank ones: their blocks go straight to the page
  programs (no compare, no erase). Every other block is compared / erased.
  A data block whose read back fails is written once more with a forced
  erase (an erase cut short by a power cut that still reads blank, a stale
  assumption, or a cluster the preparation skipped as dirty), once per block.
- Power-off / reset (`finish_disk_writes`): an erase in flight is waited
  for (`prep_finish`, <= 0.4 s).

Per prepared block ~18.5 ms (emulator, tPP 0.7 ms): the panel read 1.3 ms,
16 page programs ~14 ms, the read back 2.7 ms. The rest of the shot (FAT,
directory entry, verify) runs after the screen unfreezes. Host tests:
[`tools/fatren-test`](../tools/fatren-test) (power cuts during the
preparation, the retry, another writer between steps).

Gallery (screenshot viewer): no big buffer. `Gallery` decodes a BMP row by
row from `diskbuf::blocks()[0]` (data) and `[1]` (FAT) inside one `poll` while the
disk is free.

## The disk buffer arena (`diskbuf::ARENA`, 12 KiB)

`ARENA` ([`src/diskbuf.rs`](../src/diskbuf.rs)) is the shared block
buffer of every on-device disk user (accessor `diskbuf::blocks()`):

| user | uses it | across main-loop passes? |
|---|---|---|
| calbackup boot safety copy (pre_main) | buf, tmp, FAT | no (before main) |
| screenshot.rs (shot, scope record) | [0] block being written, [1] FAT cache | yes, Bpb .. Verify |
| csvlog.rs commit | [0], [1] | yes, while `writing()` |
| csvlog catalog / delete / free space (MMEM:CAT?, gallery, vwrite) | [0..2] | no |
| fastlog.rs Prep / Stop | [0..2] | yes (`holds_disk()`) |
| upload.rs (SCPI upload, USB-volume firmware staging) | [0] collects the block | yes (`busy()`) |
| vwrite.rs (host deletes, purge, commit) | [0..2] | no |
| gallery.rs | [0] data, [1] FAT | no (inside one poll) |
| fatdir.rs (SHOTS / LOGS / SCOPE created) | [0..2] | no |
| setfile.rs (settings copy on the SPI disk) | [0..1] | no |
| lcd.rs strip rendering (a page repaint; below) | 11 520 B | no |

A csvlog commit runs every 60 s while a recording is on, so the arena
cannot be overlaid on `scope::BUF`.

Second tenant: the scope FFT's 8 KiB work buffer, needed only while a
spectrum job is in flight; the job can be dropped and redone at any time.
Ownership has runtime checks:

- `blocks()` is the disk side's only accessor. Every call bumps a
  generation counter (`GEN`). A disk user never waits for the FFT.
- `fft_work(fresh)` is the FFT's only accessor. It returns None while a
  disk job that keeps data in the arena across passes is in flight
  (`disk_job()`: `screenshot::busy() || csvlog::writing() ||
  fastlog::holds_disk() || upload::busy()`). For a job already loaded
  (`fresh` false), it also returns None when `GEN` moved since the load (a
  single-pass disk user wrote the arena in between). On None,
  [`scope_page.rs`](../src/scope_page.rs) drops the job, keeps the last
  spectrum and asks for the frame again.
- Neither side keeps a reference across passes. The arena is main-loop
  only (no ISR touches it).

Visible cost: the spectrum is not updated during a disk job (a commit,
~0.1-0.5 s every 60 s while recording; a shot; a whole fast-log session).

Third tenant: the strip renderer
([`lcd.rs`](../src/lcd.rs) "strip rendering"): a full page repaint
(`App::repaint_strips`) draws the page into 12 rows x 480 px at a time
(11 520 B of the arena) and sends each strip to the panel. Inside one call:

- `diskbuf::strip_take()` refuses while `disk_job()` (the page is then
  drawn directly, as before), else bumps `GEN` like a disk user: an FFT
  job in flight is dropped and redone (one spectrum update later on a page
  change). `fft_work` returns None while the renderer holds the arena (the
  scope page's draw can step the FFT).
- `strip_give()` reports whether `GEN` moved meanwhile (a single-pass disk
  user inside a page draw; none is known): the page is then drawn again
  directly.

Two small statics serve the strip passes: `ui::GEN_SP` (TCM, 484 B: the
generator preview's trace rows, worked out once per wave and square duty
instead of per strip) and `scope_ui::COLC` (400 B, flash).

## The scope record: raw ring and coded ring

`scope::BUF` ([`src/scope.rs`](../src/scope.rs), 81 920 B in SRAM on
test/scope-chain-0.4 with 0.4.0's GEN / CH2 after the
[SRAM audit](native-boot.md#sram-audit-2026-10-30-featsram-overlays):
86 016 B before GEN / CH2) is the whole SRAM the budget leaves (8 KiB link
floor + ~0.1 KB). The
stock record is 65 520 samples; as u16 that is 131 040 B, more than the
SRAM has free. 10-bit codes packed 4 in 5 bytes
([`src/pack10.rs`](../src/pack10.rs): a little-endian bit stream, sample
i at bits 10 i .. 10 i + 9, one unaligned 32-bit load per sample) make
65 520 samples 81 900 B; BUF holds 65 520 packed (`PCAP`).

The DMA writes u16 (GPIOB IDR, or the TIM8 latch, at up to 50 MS/s), and
opendm40 has no interrupt besides the AD2 stream's (the main loop polls
everything, passes up to ~25 ms), so only the main loop can pack. That
sets two modes:

- **fs > 25 kS/s** (faster than 100 ms/div): the DMA writes a raw ring of
  `CAP` = 43 008 u16 (linear or circular as before). When the
  acquisition stops, the ring is packed in place (`pack_in_place`: group
  g's 5 bytes are written after its 8 raw bytes are read, so it never
  overtakes unread samples; ~0.7 ms for 43 008 samples, inferred from the
  instruction count). Packing during
  the capture would need the CPU at 50 MS/s (4 cycles per sample at
  200 MHz), and with the comparator trigger the main loop learns of the
  edge only at its next poll.
- **fs <= 25 kS/s** and the DMM-input wave view: the coded ring
  ([`src/cring.rs`](../src/cring.rs)) in BUF's first 77 820 B. MCX: the
  DMA fills a 2048-sample raw staging ring at BUF's top (82 ms at
  25 kS/s), every poll moves the new samples into the ring (`pk_drain`).
  A gap between two polls longer than the staging ring (time x rate)
  restarts the acquisition: a record with a hole is never kept. The wave
  view pushes the AD2 samples as it drains them (`wave_drain`).

  The ring is a sequence of 4096-sample blocks. The newest is written
  packed (4 in 5 bytes); a full one is coded in place with the DLG scope
  coder (dlog.rs, [compact-log-format.md](compact-log-format.md#scope-record-kind-2):
  the fixed predictor with the least sum of |residual|, adaptive Rice)
  when that is smaller and the coded bytes never run more than 64 codes
  (`LA`, read ahead) past the packed codes they replace (a dry run checks
  both first), else it stays packed. The next block starts after it; the
  oldest goes when room is needed (a coded one whole, a packed one 4
  samples at a time). So a record is at least about 62 250 samples (white
  noise: all packed, the old packed ring) and at most 64 blocks (262 144
  samples; `dlog::SBLK_MAX`, the Save CSV file's block table). Blocks are
  coded only for the long captures (Single, DIGitize, roll; in the Wave
  view also only at 9600 S/s and slower): a running frame's record fits
  the packed ring, and while no block was coded the ring is the plain
  packed ring (`rb` * 8 a multiple of 10), read through plain views at the
  old cost.

  Reads go through `pack10::Codes` views with `cap` 0 (`pack10::CODED` =
  `cring::at`): packed blocks directly, coded ones through one decoder
  cursor (forward reads one code each; the last 16 codes it decoded are
  kept, so the sin(x)/x taps step back for free; a read further behind
  decodes its block again from the start).

Every stored record (STOP, a shown frame, Single) is packed or coded; its users
(the render, zoom / pan, measurements, FFT, autoset, Save CSV,
`WAV:DATA?`, the software triggers, the sin(x)/x interpolation) read it
through `pack10::Codes`, a window of the ring (index `off + i`, wrapping
at the ring size), so a full circular ring is never rotated. `Codes`
also implements `pack10::Src`, the read trait those functions take (the
host tests pass u16 slices).

Single / DIGitize wait for a whole record (at most 10 s, `rec_split`),
the trigger point at the trigger position of the record. A STOP while
running (`apply`) goes on until such a capture of the screen + 0.5 s is
in, at most 0.5 s more (the deadline: then what is in, untriggered). A STOP cannot keep
the frame shown before: the capture after it has already started
overwriting it in BUF, the only record buffer. Normal / Auto
frames wait for a screen around the trigger, as before, so their records
hold whatever the ring had collected before the trigger. Linear records
(the pulse / runt triggers, the software edge search above 1.25 MS/s)
are the whole raw ring but at most max(26 624 samples, 0.1 s) in Normal /
Auto, so the slow timebases do not wait longer per frame than before.

Cost of the coded ring (emulator at --mips 110, the device's speed per
instruction; [`emu/ringperf.py`](../../emu/ringperf.py), a 50 Hz sine on
the model's input, which has no ADC noise; DBG:PROF?):

| | before (packed ring) | coded ring |
|---|---|---|
| running, 100 ms/div: acquisition / frame / measurements | 2.1 % CPU, max 1.0 ms / 6.9 ms / 19.6 ms | 3.2 % CPU, max 1.0 ms / 6.9 ms / 23.1 ms |
| roll, 200 ms/div (coding on): acquisition | 6.7 % CPU, max 6.1 ms | 4.6 % CPU, max 4.8 ms (the coding of a full block) |
| DIGitize at 100 ms/div (177 380 samples) redrawn at 10 ms/div: frame + measurements | - | 5.9 + 16.3 ms |
| the same at 100 ms/div (25 000 samples on the screen) | - | 36 + 133 ms |
| the same zoomed out to 1 s/div (the whole record) | - | 228 ms |

The coding of a full block costs ~4.5 ms in one pass, every 164 ms at
25 kS/s; the plan (the order's bits, the in-place rule) runs as the codes
come. A coded record is read ~1.3 us a code (the decoder), a packed one
as before.

Cost (emulator, emu/dragperf.py, 200 us/div): the acquisition section
max 939 -> 3 545 us per pass (the in-place pack and the trigger search
through `Codes`), a measurement of a 25 000-sample screen 3.1 -> 6.9 ms.

The bound of a record is also the scope DLG format's: 64 blocks of 4096
samples (`dlog::SBLK_MAX`), 262 144. Single / DIGitize on the coded ring
(edge trigger) want 10 s, the trigger at the trigger position; the part
after the trigger point is at most PCAP less the margins and the
screen's pre part (39 036 at the centre), so it fits the ring whatever
the signal, the part before it is what the ring keeps.

Not done: the fast timebases at 65 520 samples. That needs 131 040 B of
raw ring in one piece: BUF plus ~45 KB of buffers lent for the capture
(`logger::RING`, `calbackup::SCRATCH`, the csvlog buffers, `gen::WAVE`),
i.e. no DMM logging, no screenshot / Save CSV / FFT and no generator
while a fast capture runs. An 8-bit capture (byte DMA) is not possible
on this board: the 3PA1030's 10 bits are PB0..PB9, and a byte read of
GPIOB gives PB0..PB7 or PB8..PB15, never the top 8 bits.

## Every static of 1 KiB or more

Sizes from `llvm-nm -S` of a native release ELF (default features) of
2026-10-24 (rust-fw-poc, a few commits before this table; rebuild and
re-run to refresh). TCM = 0x1000xxxx (shared with the stack), SRAM = 0x200xxxxx.

| static | size (B) | live when | live at the same time | overlay / shrink? |
|---|---|---|---|---|
| scope::BUF (SRAM) | 69 632 | always: the scope acquires in the background in every view (measurements, SCPI); the stored record while stopped | everything | Lent to inrush (`Scope::lend_buf`) while the inrush page is open. Keep. |
| logger::RING (SRAM) | 16 540 | LOG:STAT ON sessions (AD1 / AD2 stream), the Wave view, inrush, fast-log sessions | csvlog is refused by the Wave view, inrush and `fastlog::begin`, but not by an SCPI LOG:STAT ON session | Idle in plain DMM / scope use. An overlay with the csvlog buffers would need csvlog and LOG:STAT ON to refuse each other, plus an owner token. Not done. |
| diskbuf::ARENA (SRAM) | 12 288 | any disk job (table above) + the FFT | see above | Already shared with the FFT. |
| scope_ui::GRAPH (SRAM) | 9 956 | the scope pages (Scope, combined view, Wave view): trace / persistence / math rows and their on-screen diff copies | the scope record, the FFT | The combined view rules out an overlay with anything scope-side. The math rows (`ma`, `d_ma`) could go when math is off. Not done. |
| csvlog::FIFO_BUF (SRAM) | 8 192 | a CSV recording; lent to fastlog (`csvlog::lend_fifo`, never together); otherwise its head holds the MMEM:CAT? cache | DMM, scope, BT, USB | Shared with the catalog cache ([native-boot.md, SRAM audit](native-boot.md#sram-audit-2026-10-30-featsram-overlays)). See logger::RING. |
| bt::TX (SRAM) | 6 656 | while BT is connected: the largest reply (`usbtmc::RSP_MAX` block + LF) plus stock frames, queued for DMA | everything | Shrunk from 8 192 (SRAM audit); not below one whole reply. |
| APP (TCM) | 6 672 | always (the App) | | Keep. |
| vdisk::TAB (SRAM) | 6 420 | while USB exports the generated volume (the table regenerates on sector reads) | everything | Keep. |
| csvlog::CAT | 12 | the cached file catalog (`MMEM:CAT?` reply, `CAT_MAX` = `RSP_MAX`), warmed before a stream starts | everything | The reply is in `csvlog::FIFO_BUF` (SRAM audit). |
| scope_page::CACHE (SRAM) | 4 824 | the scope page (persistence a/b, overlays) | GRAPH | Same as GRAPH. |
| scope_page::FFT (TCM) | 4 128 | the last spectrum | | Must survive a dropped job. Keep. |
| csvlog::TAIL (USBHS RAM) | 4 096 | a CSV recording | FIFO_BUF | In the USBHS FIFO RAM ([native-boot.md, Extra RAM areas](native-boot.md#extra-ram-areas-2026-10-30-featsram-extra-areas)). |
| gen::WAVE (SRAM) | 4 000 | while the generator outputs (DAC DMA, circular) | everything | Keep. |
| dmm::acdctr::TR (SRAM) | 40 | DBG:ACDC:TRACE only with the `acdc-trace` feature (64 entries, 1 804) | | One unused entry without the feature. |
| touch::LOG (TCM) | 196 | always (the touch test page; DBG:TOUCH? with `dbg-touch`, 1 924) | | 16 entries without the feature (SRAM audit). |
| fastlog::JOB (SRAM) | 1 864 | a fast-log disk session | logger::RING | Keep. |
| scope::ROLL_COLS (SRAM) | 1 600 | roll mode | | Keep. |
| vwrite::W0 (SRAM) | 1 376 | the writable USB volume session | | Keep. |
| scope_ui::dec::ANN (SRAM) | 1 236 | the decoder band's kept bytes / frames (positions in the record) | the scope record | Keep (TCM until 2026-10-30: stack headroom). |
| dmm::REC_COPY (TCM) | 1 024 | always: the RAM copy of the settings / calibration record (the record is in the slow flash bank) | | Keep. |
| bt::RX (SRAM) | 1 024 | the DMA ring, while BT is powered | | Keep. |

Never live together, so worth an owner token if RAM gets tight again:
logger::RING with the csvlog buffers (12 KiB). Everything else over 4 KiB is
always live or live with the scope.
